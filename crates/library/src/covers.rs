//! Fetching and decoding covers from MA's image proxy.
//!
//! Each cover is fetched once at [`SOURCE_SIZE`] and kept as JPEG under
//! `<dir>/jpeg<size>/<proxy_id>.jpg`, so the atlas can be re-baked at new
//! size steps without touching the network. Larger sizes (512 for
//! now-playing) are fetched on demand and cached the same way.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fast_image_resize::{images::Image, FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
use zune_jpeg::JpegDecoder;

use crate::{Error, Result};

/// Sizes MA's `/imageproxy` accepts.
pub const PROXY_SIZES: [u32; 5] = [80, 160, 256, 512, 1024];
/// Size fetched for the atlas.
pub const SOURCE_SIZE: u32 = 160;

/// An RGB888 image.
#[derive(Debug, Clone)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

pub struct CoverCache {
    dir: PathBuf,
    http_base: String,
    agent: ureq::Agent,
}

impl CoverCache {
    /// `http_base` is MA's HTTP root, e.g. `http://192.168.2.238:8095`.
    pub fn new(dir: impl Into<PathBuf>, http_base: impl Into<String>) -> CoverCache {
        let agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().new_agent();
        CoverCache { dir: dir.into(), http_base: http_base.into().trim_end_matches('/').to_owned(), agent }
    }

    pub fn path(&self, proxy_id: &str, size: u32) -> PathBuf {
        self.dir.join(format!("jpeg{size}")).join(format!("{proxy_id}.jpg"))
    }

    /// Cached JPEG bytes, if present.
    pub fn cached(&self, proxy_id: &str, size: u32) -> Option<Vec<u8>> {
        std::fs::read(self.path(proxy_id, size)).ok()
    }

    /// JPEG bytes for a cover, from disk or (blocking) from MA. Call from a
    /// blocking context (`spawn_blocking`).
    pub fn get(&self, proxy_id: &str, size: u32) -> Result<Vec<u8>> {
        if let Some(b) = self.cached(proxy_id, size) {
            return Ok(b);
        }
        if !PROXY_SIZES.contains(&size) {
            return Err(Error::Http(format!("imageproxy does not serve size {size}")));
        }
        if !proxy_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err(Error::Http("unexpected characters in proxy id".into()));
        }
        let url = format!("{}/imageproxy/{proxy_id}?size={size}&fmt=jpeg", self.http_base);
        let resp = self.agent.get(&url).call().map_err(|e| Error::Http(e.to_string()))?;
        let mut bytes = Vec::new();
        resp.into_body().into_reader().take(4 << 20).read_to_end(&mut bytes)?;
        // Validate before caching so a bad reply is not kept forever.
        decode(&bytes)?;
        let path = self.path(proxy_id, size);
        write_atomic(&path, &bytes)?;
        Ok(bytes)
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Decode a JPEG to RGB888.
pub fn decode(jpeg: &[u8]) -> Result<Rgb> {
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut d = JpegDecoder::new_with_options(jpeg, opts);
    let data = d.decode().map_err(|e| Error::Image(format!("{e:?}")))?;
    let (w, h) = d.dimensions().ok_or_else(|| Error::Image("no dimensions".into()))?;
    if data.len() < w * h * 3 {
        return Err(Error::Image("short decode".into()));
    }
    Ok(Rgb { width: w as u32, height: h as u32, data })
}

/// Mean colour as 0xRRGGBB.
pub fn average_rgb(img: &Rgb) -> u32 {
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for p in img.data.chunks_exact(3) {
        r += p[0] as u64;
        g += p[1] as u64;
        b += p[2] as u64;
    }
    let n = (img.data.len() / 3).max(1) as u64;
    (((r / n) as u32) << 16) | (((g / n) as u32) << 8) | (b / n) as u32
}

/// Scale to a `size`×`size` square, centre-cropping non-square sources
/// (MA covers are often 160×159 or similar).
pub fn square(img: &Rgb, size: u32, resizer: &mut Resizer) -> Result<Rgb> {
    let src =
        Image::from_vec_u8(img.width, img.height, img.data.clone(), PixelType::U8x3).map_err(|e| Error::Image(e.to_string()))?;
    let mut dst = Image::new(size, size, PixelType::U8x3);
    let opts =
        ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::CatmullRom)).fit_into_destination(Some((0.5, 0.5)));
    resizer.resize(&src, &mut dst, &opts).map_err(|e| Error::Image(e.to_string()))?;
    Ok(Rgb { width: size, height: size, data: dst.into_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_and_square() {
        let img = Rgb { width: 4, height: 2, data: [[255u8, 0, 0], [0, 0, 255]].repeat(4).concat() };
        assert_eq!(average_rgb(&img), 0x7f007f);
        let mut r = Resizer::new();
        let s = square(&img, 2, &mut r).unwrap();
        assert_eq!((s.width, s.height, s.data.len()), (2, 2, 12));
    }
}
