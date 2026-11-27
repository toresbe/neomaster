//! Cover atlas: covers pre-scaled to fixed size steps, in the display's
//! pixel format, one memory-mapped file per step. Drawing never decodes
//! or scales; a cover is a slice of an mmap.
//!
//! # File format (`covers-<size>.<fmt>`, all integers little-endian)
//!
//! | offset | type     | field                                   |
//! |--------|----------|-----------------------------------------|
//! | 0      | [u8; 8]  | magic `NMATLAS1`                        |
//! | 8      | u32      | format version (1)                      |
//! | 12     | u32      | pixel format: 1 = RGB565, 2 = XRGB8888  |
//! | 16     | u32      | size in px (slots are square)           |
//! | 20     | u32      | slot count                              |
//! | 24     | u32      | slot stride in bytes                    |
//! | 28     | u32      | offset of slot 0 (4096)                 |
//! | 32     | u64      | generation                              |
//!
//! Slot `i` starts at `offset + i * stride`; its pixels are row-major,
//! `size * bytes_per_pixel` bytes per row, no padding. Slots are in
//! browse order, so the covers around a list cursor are contiguous and
//! page in together. RGB565 pixels are `u16` `rrrrrggggggbbbbb`;
//! XRGB8888 pixels are `u32` `0xffRRGGBB` (bytes B, G, R, X in memory),
//! which is what X11 and `/dev/fb0` expect at 24/32 bpp on x86.
//!
//! A new atlas is written to a temporary file and renamed into place,
//! so an open [`Atlas`] keeps its old mapping until it is reopened. The
//! generation in the header matches `Store::atlas_generation()`, which
//! is what the slot numbers in the store refer to.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use fast_image_resize::Resizer;
use memmap2::Mmap;

use crate::covers::{self, CoverCache, SOURCE_SIZE};
use crate::{Error, Result};

const MAGIC: &[u8; 8] = b"NMATLAS1";
const VERSION: u32 = 1;
const DATA_OFFSET: u32 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb565 = 1,
    Xrgb8888 = 2,
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgb565 => 2,
            PixelFormat::Xrgb8888 => 4,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            PixelFormat::Rgb565 => "rgb565",
            PixelFormat::Xrgb8888 => "xrgb8888",
        }
    }

    fn from_u32(v: u32) -> Option<Self> {
        match v {
            1 => Some(PixelFormat::Rgb565),
            2 => Some(PixelFormat::Xrgb8888),
            _ => None,
        }
    }

    /// Convert RGB888 pixels to this format, appending to `out`.
    pub fn encode(self, rgb: &[u8], out: &mut Vec<u8>) {
        match self {
            PixelFormat::Rgb565 => {
                for p in rgb.chunks_exact(3) {
                    // Round to nearest rather than truncate.
                    let r = (p[0] as u16 * 31 + 127) / 255;
                    let g = (p[1] as u16 * 63 + 127) / 255;
                    let b = (p[2] as u16 * 31 + 127) / 255;
                    out.extend_from_slice(&((r << 11) | (g << 5) | b).to_le_bytes());
                }
            }
            PixelFormat::Xrgb8888 => {
                for p in rgb.chunks_exact(3) {
                    out.extend_from_slice(&[p[2], p[1], p[0], 0xff]);
                }
            }
        }
    }
}

/// Which size steps to bake, and in which pixel format.
#[derive(Debug, Clone)]
pub struct AtlasConfig {
    pub dir: PathBuf,
    pub format: PixelFormat,
    /// Square sizes in px. Default: the list row (48) and the cover arc
    /// steps (64–96) from the architecture sketch.
    pub sizes: Vec<u32>,
}

impl AtlasConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        AtlasConfig { dir: dir.into(), format: PixelFormat::Rgb565, sizes: vec![48, 64, 72, 80, 88, 96] }
    }

    pub fn path(&self, size: u32) -> PathBuf {
        self.dir.join(format!("covers-{size}.{}", self.format.extension()))
    }
}

fn stride(size: u32, format: PixelFormat) -> usize {
    let raw = (size * size) as usize * format.bytes_per_pixel();
    raw.div_ceil(64) * 64
}

fn header(format: PixelFormat, size: u32, slots: u32, generation: u64) -> Vec<u8> {
    let mut h = Vec::with_capacity(DATA_OFFSET as usize);
    h.extend_from_slice(MAGIC);
    for v in [VERSION, format as u32, size, slots, stride(size, format) as u32, DATA_OFFSET] {
        h.extend_from_slice(&v.to_le_bytes());
    }
    h.extend_from_slice(&generation.to_le_bytes());
    h.resize(DATA_OFFSET as usize, 0);
    h
}

/// Result of a bake.
#[derive(Debug, Clone)]
pub struct BakeReport {
    pub generation: u64,
    pub slots: usize,
    /// Covers whose cached JPEG was missing or undecodable; their slots
    /// are left black (the UI uses the average colour instead).
    pub failed: Vec<String>,
}

/// Bake every size step for `order` (proxy ids in browse order) from the
/// JPEGs in `cache`. Blocking; run it with `spawn_blocking`.
pub fn bake(cfg: &AtlasConfig, cache: &CoverCache, order: &[String], generation: u64) -> Result<BakeReport> {
    std::fs::create_dir_all(&cfg.dir)?;
    let mut writers = Vec::new();
    for &size in &cfg.sizes {
        let tmp = cfg.path(size).with_extension("tmp");
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
        w.write_all(&header(cfg.format, size, order.len() as u32, generation))?;
        writers.push((size, tmp, w));
    }
    let mut resizer = Resizer::new();
    let mut failed = Vec::new();
    let mut px = Vec::new();
    for id in order {
        let img = cache.cached(id, SOURCE_SIZE).ok_or(Error::Image("not cached".into())).and_then(|j| covers::decode(&j));
        if img.is_err() {
            failed.push(id.clone());
        }
        for (size, _, w) in writers.iter_mut() {
            let st = stride(*size, cfg.format);
            px.clear();
            if let Ok(img) = &img {
                let sq = covers::square(img, *size, &mut resizer)?;
                cfg.format.encode(&sq.data, &mut px);
            }
            px.resize(st, 0);
            w.write_all(&px)?;
        }
    }
    for (size, tmp, w) in writers {
        let f = w.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        std::fs::rename(&tmp, cfg.path(size))?;
    }
    Ok(BakeReport { generation, slots: order.len(), failed })
}

/// A read-only, memory-mapped atlas file for one size step.
pub struct Atlas {
    map: Mmap,
    pub format: PixelFormat,
    pub size: u32,
    pub slots: u32,
    pub generation: u64,
    stride: usize,
    offset: usize,
}

impl Atlas {
    pub fn open(path: impl AsRef<Path>) -> Result<Atlas> {
        let f = File::open(path)?;
        // SAFETY: atlas files are only ever replaced by rename, never
        // modified in place, so the mapping stays valid and unchanged.
        let map = unsafe { Mmap::map(&f)? };
        let bad = |m: &str| Error::Atlas(m.to_owned());
        if map.len() < 40 || &map[0..8] != MAGIC {
            return Err(bad("not an atlas file"));
        }
        let u32_at = |o: usize| u32::from_le_bytes(map[o..o + 4].try_into().unwrap());
        if u32_at(8) != VERSION {
            return Err(bad("unsupported atlas version"));
        }
        let format = PixelFormat::from_u32(u32_at(12)).ok_or_else(|| bad("unknown pixel format"))?;
        let (size, slots, stride, offset) = (u32_at(16), u32_at(20), u32_at(24) as usize, u32_at(28) as usize);
        let generation = u64::from_le_bytes(map[32..40].try_into().unwrap());
        if stride < (size * size) as usize * format.bytes_per_pixel() || map.len() < offset + slots as usize * stride {
            return Err(bad("truncated atlas"));
        }
        Ok(Atlas { map, format, size, slots, generation, stride, offset })
    }

    pub fn open_for(cfg: &AtlasConfig, size: u32) -> Result<Atlas> {
        Atlas::open(cfg.path(size))
    }

    /// Pixels of one slot: `size` rows of `size * bytes_per_pixel` bytes.
    pub fn slot(&self, i: u32) -> Option<&[u8]> {
        if i >= self.slots {
            return None;
        }
        let start = self.offset + i as usize * self.stride;
        let len = (self.size * self.size) as usize * self.format.bytes_per_pixel();
        Some(&self.map[start..start + len])
    }

    /// Hint the kernel to page in slots `range` ahead of a scroll.
    pub fn prefetch(&self, first: u32, count: u32) {
        let first = first.min(self.slots);
        let end = (first + count).min(self.slots);
        if end > first {
            let start = self.offset + first as usize * self.stride;
            let len = (end - first) as usize * self.stride;
            let _ = self.map.advise_range(memmap2::Advice::WillNeed, start, len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_encoding() {
        let mut out = Vec::new();
        PixelFormat::Rgb565.encode(&[255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255], &mut out);
        let px: Vec<u16> = out.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(px, [0xffff, 0xf800, 0x07e0, 0x001f]);
        let mut out = Vec::new();
        PixelFormat::Xrgb8888.encode(&[1, 2, 3], &mut out);
        assert_eq!(out, [3, 2, 1, 0xff]);
    }

    #[test]
    fn header_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.rgb565");
        let mut bytes = header(PixelFormat::Rgb565, 4, 2, 9);
        bytes.extend(vec![0xab; stride(4, PixelFormat::Rgb565) * 2]);
        std::fs::write(&p, bytes).unwrap();
        let a = Atlas::open(&p).unwrap();
        assert_eq!((a.size, a.slots, a.generation, a.format), (4, 2, 9, PixelFormat::Rgb565));
        assert_eq!(a.slot(1).unwrap().len(), 32);
        assert!(a.slot(2).is_none());
    }
}
