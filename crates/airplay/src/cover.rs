//! Cover art from the metadata pipe (`ssnc PICT`), made ready for the panel: decoded and cropped
//! to a centred square. Senders send JPEG (Music Assistant: ~100 kB, 600x600 or so; iOS likewise);
//! PNG is recognised but not decoded.
//!
//! Decoding a 600x600 JPEG and scaling it takes tens of milliseconds on the BM5's C7, so the
//! caller runs this off the core task.

use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// An RGB image, 3 bytes per pixel, rows top to bottom.
#[derive(Clone, PartialEq, Eq)]
pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Rgb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rgb({}x{})", self.width, self.height)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Empty,
    Png,
    Unknown,
    Decode(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Empty => write!(f, "no cover art"),
            Error::Png => write!(f, "PNG cover art is not supported yet"),
            Error::Unknown => write!(f, "cover art in an unknown format"),
            Error::Decode(e) => write!(f, "cover art: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// Decode `bytes`, crop the middle square out of a non-square cover, and scale it to `size`.
pub fn decode_square(bytes: &[u8], size: u32) -> Result<Rgb, Error> {
    match bytes {
        [] => return Err(Error::Empty),
        [0xff, 0xd8, ..] => {}
        [0x89, b'P', b'N', b'G', ..] => return Err(Error::Png),
        _ => return Err(Error::Unknown),
    }
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut d = JpegDecoder::new_with_options(bytes, opts);
    let data = d.decode().map_err(|e| Error::Decode(format!("{e:?}")))?;
    let (w, h) = d.dimensions().ok_or_else(|| Error::Decode("no dimensions".into()))?;
    if w == 0 || h == 0 || data.len() < w * h * 3 {
        return Err(Error::Decode("short decode".into()));
    }
    let src = Image::from_vec_u8(w as u32, h as u32, data, PixelType::U8x3).map_err(|e| Error::Decode(e.to_string()))?;
    let mut dst = Image::new(size, size, PixelType::U8x3);
    let opts =
        ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::CatmullRom)).fit_into_destination(Some((0.5, 0.5)));
    Resizer::new().resize(&src, &mut dst, &opts).map_err(|e| Error::Decode(e.to_string()))?;
    Ok(Rgb { width: size, height: size, data: dst.into_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_what_it_cannot_decode() {
        assert_eq!(decode_square(&[], 10), Err(Error::Empty));
        assert_eq!(decode_square(b"\x89PNG\r\n", 10), Err(Error::Png));
        assert_eq!(decode_square(b"GIF89a", 10), Err(Error::Unknown));
        assert!(matches!(decode_square(&[0xff, 0xd8, 0xff, 0x00], 10), Err(Error::Decode(_))));
    }
}
