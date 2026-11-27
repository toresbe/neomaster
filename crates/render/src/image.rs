//! Images: RGBA bitmaps the backends upload once and keep, keyed by [`Image::id`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::Rect;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Straight (not premultiplied) RGBA, row-major, no padding. Immutable once made, so a backend
/// can cache whatever it uploaded under [`Image::id`].
pub struct Image {
    id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image#{}({}x{})", self.id, self.width, self.height)
    }
}

impl PartialEq for Image {
    fn eq(&self, other: &Image) -> bool {
        self.id == other.id
    }
}

impl Image {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Arc<Image> {
        assert_eq!(rgba.len(), width as usize * height as usize * 4, "RGBA buffer size");
        Arc::new(Image { id: NEXT_ID.fetch_add(1, Ordering::Relaxed), width, height, rgba })
    }

    /// Build an image from a function of the pixel centre, returning straight RGBA.
    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(f32, f32) -> [u8; 4]) -> Arc<Image> {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend_from_slice(&f(x as f32 + 0.5, y as f32 + 0.5));
            }
        }
        Image::new(width, height, rgba)
    }

    /// Decode a PNG (any colour type and bit depth png can expand to 8-bit).
    pub fn from_png(bytes: &[u8]) -> Result<Arc<Image>, png::DecodingError> {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
        let info = reader.next_frame(&mut buf)?;
        buf.truncate(info.buffer_size());
        let (w, h) = (info.width, info.height);
        let px = (w * h) as usize;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf,
            png::ColorType::GrayscaleAlpha => {
                let mut out = Vec::with_capacity(px * 4);
                for p in buf.chunks_exact(2) {
                    out.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
                }
                out
            }
            png::ColorType::Rgb => {
                let mut out = Vec::with_capacity(px * 4);
                for p in buf.chunks_exact(3) {
                    out.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
                out
            }
            png::ColorType::Grayscale => buf.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
            png::ColorType::Indexed => unreachable!("expanded by normalize_to_color8"),
        };
        Ok(Image::new(w, h, rgba))
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Premultiplied BGRA (little-endian ARGB32), what XRender and most framebuffers take.
    pub fn premultiplied_bgra(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len());
        for p in self.rgba.chunks_exact(4) {
            let a = p[3] as u32;
            let pm = |v: u8| ((v as u32 * a + 127) / 255) as u8;
            out.extend_from_slice(&[pm(p[2]), pm(p[1]), pm(p[0]), p[3]]);
        }
        out
    }
}
