//! Glyph rasterising with fontdue. Backends upload each (character, size) once and keep it;
//! [`Font::layout`] gives pen positions so text never needs measuring at draw time.

use std::collections::HashMap;

/// The face shipped in the repository (fonts/FreeSansBold.ttf, GNU FreeFont), used when the stock
/// BeoFont is not available (see `ui::assets`).
pub static DEFAULT_FONT: &[u8] = include_bytes!("../../../fonts/FreeSansBold.ttf");

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub ch: char,
    pub size: u16,
}

/// A rasterised glyph: an 8-bit coverage bitmap and its placement relative to the pen.
pub struct Glyph {
    pub width: u16,
    pub height: u16,
    /// From the pen to the bitmap's left edge.
    pub left: i16,
    /// From the baseline up to the bitmap's top edge.
    pub top: i16,
    pub advance: i16,
    pub coverage: Vec<u8>,
}

#[derive(Clone)]
pub struct Font {
    font: fontdue::Font,
    advances: HashMap<GlyphKey, f32>,
}

impl Font {
    pub fn from_bytes(bytes: &[u8]) -> Result<Font, &'static str> {
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())?;
        Ok(Font { font, advances: HashMap::new() })
    }

    pub fn default_font() -> Font {
        Self::from_bytes(DEFAULT_FONT).expect("bundled font parses")
    }

    pub fn rasterize(&self, key: GlyphKey) -> Glyph {
        let (m, coverage) = self.font.rasterize(key.ch, key.size as f32);
        Glyph {
            width: m.width as u16,
            height: m.height as u16,
            left: m.xmin as i16,
            top: (m.height as i32 + m.ymin) as i16,
            advance: m.advance_width.round() as i16,
            coverage,
        }
    }

    fn advance(&mut self, key: GlyphKey) -> f32 {
        let font = &self.font;
        *self.advances.entry(key).or_insert_with(|| font.metrics(key.ch, key.size as f32).advance_width)
    }

    /// Whether the face has a glyph for `ch` (fontdue draws a box for missing ones).
    pub fn has_glyph(&self, ch: char) -> bool {
        self.font.lookup_glyph_index(ch) != 0
    }

    /// Width of `text` in pixels at `size`.
    pub fn width(&mut self, text: &str, size: u16) -> i32 {
        text.chars().map(|ch| self.advance(GlyphKey { ch, size })).sum::<f32>().round() as i32
    }
}
