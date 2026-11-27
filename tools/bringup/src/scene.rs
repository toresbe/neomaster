//! The benchmark's frames: the same primitive kinds the UI will draw (fills, covers, glyph runs),
//! laid out like the cover arc and list views, independent of the backend that draws them.

use render::{Color, Rect, SCREEN_HEIGHT, SCREEN_WIDTH};

/// How a cover is stored in the atlas and uploaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CoverFmt {
    /// 24-bit opaque (the atlas's XRGB8888), depth 24 pixmap.
    Xrgb,
    /// 16-bit opaque (the atlas's RGB565), depth 16 pixmap.
    Rgb565,
    /// 32-bit premultiplied with alpha (rounded corners, baked shadows), depth 32 pixmap.
    Argb,
}

impl CoverFmt {
    pub const ALL: [CoverFmt; 3] = [CoverFmt::Xrgb, CoverFmt::Rgb565, CoverFmt::Argb];

    pub fn name(self) -> &'static str {
        match self {
            CoverFmt::Xrgb => "xrgb8888",
            CoverFmt::Rgb565 => "rgb565",
            CoverFmt::Argb => "argb8888",
        }
    }

    pub fn parse(s: &str) -> Option<CoverFmt> {
        CoverFmt::ALL.into_iter().find(|f| f.name() == s || f.name().starts_with(s))
    }

    pub fn bytes_per_pixel(self) -> usize {
        match self {
            CoverFmt::Rgb565 => 2,
            _ => 4,
        }
    }
}

/// The atlas's baked sizes: list rows (48) and the cover arc steps (64-96).
pub const LIST_SIZE: u16 = 48;
pub const ARC_SIZES: [u16; 5] = [64, 72, 80, 88, 96];
pub const ALL_SIZES: [u16; 6] = [48, 64, 72, 80, 88, 96];

/// Distinct covers kept per (format, size), so the benchmark is not one hot texture.
pub const POOL: usize = 24;

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Fill {
        rect: Rect,
        color: Color,
    },
    /// Cover `slot` of the (fmt, size) pool, drawn OVER at `alpha` (255 = opaque).
    Cover {
        fmt: CoverFmt,
        size: u16,
        slot: usize,
        x: i32,
        y: i32,
        alpha: u8,
    },
    Text {
        x: i32,
        y: i32,
        size: u16,
        color: Color,
        text: String,
    },
}

/// One frame: an optional clear, the ops back to front, and the region to put on screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub clear: Option<(Rect, Color)>,
    pub ops: Vec<Op>,
    pub present: Option<Rect>,
}

pub const SCREEN: Rect = Rect::new(0, 0, SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32);
/// The band the cover arc lives in (the covers view's column, labels included).
pub const STRIP: Rect = Rect::new(340, 0, 440, SCREEN_HEIGHT as u32);

const BG: Color = Color::rgb(8, 8, 10);
const TEXT: Color = Color::rgb(235, 235, 235);
const DIM: Color = Color::rgb(150, 150, 150);
const ACCENT: Color = Color::rgb(70, 140, 230);
const CENTRE_Y: f32 = SCREEN_HEIGHT as f32 / 2.0;

const TITLES: [&str; 8] = [
    "Kind of Blue",
    "The Köln Concert",
    "Blue Train",
    "A Love Supreme",
    "Mingus Ah Um",
    "Waltz for Debby",
    "Moanin'",
    "Somethin' Else",
];

/// The cover arc at scroll position `scroll` (fractional: mid-animation), as the covers view lays
/// it out: 13 covers, the size stepping down by distance from the centre through the baked steps,
/// fading out towards the ends, with the centre cover's title and artist.
pub fn cover_arc(scroll: f32, fmt: CoverFmt, ops: &mut Vec<Op>) {
    let base = scroll.floor() as i64;
    for i in -6i64..=6 {
        let d = (base + i) as f32 - scroll;
        let step = (d.abs().round() as usize).min(ARC_SIZES.len() - 1);
        let size = ARC_SIZES[ARC_SIZES.len() - 1 - step];
        let y = CENTRE_Y + d * 104.0;
        let x = 560.0 - d * d * 9.0;
        let alpha = ((1.0 - d.abs() / 6.5).clamp(0.0, 1.0) * 255.0) as u8;
        if alpha == 0 {
            continue;
        }
        let slot = (base + i).rem_euclid(POOL as i64) as usize;
        let h = size as f32 / 2.0;
        ops.push(Op::Cover { fmt, size, slot, x: (x - h) as i32, y: (y - h) as i32, alpha });
    }
    let t = TITLES[base.rem_euclid(TITLES.len() as i64) as usize];
    ops.push(Op::Text { x: 620, y: CENTRE_Y as i32 - 6, size: 24, color: TEXT, text: t.into() });
    ops.push(Op::Text { x: 620, y: CENTRE_Y as i32 + 22, size: 18, color: DIM, text: "Miles Davis".into() });
}

/// A list view's rows at scroll position `scroll`: 15 rows, larger and brighter near the cursor.
pub fn list_rows(scroll: f32, ops: &mut Vec<Op>) {
    ops.push(Op::Text { x: 60, y: 70, size: 22, color: DIM, text: "ALBUMS  ›  Miles Davis".into() });
    let base = scroll.floor() as i64;
    for i in -7i64..=7 {
        let d = (base + i) as f32 - scroll;
        let y = CENTRE_Y + d * 54.0;
        let near = 1.0 - d.abs().min(1.0);
        let size = (26.0 + 10.0 * near) as u16;
        let alpha = (1.0 - d.abs() / 7.5).clamp(0.0, 1.0);
        let c = if i == 0 { TEXT } else { DIM };
        let t = TITLES[(base + i).rem_euclid(TITLES.len() as i64) as usize];
        ops.push(Op::Text { x: 90, y: y as i32 + size as i32 / 3, size, color: c.with_opacity(alpha), text: t.into() });
    }
}

/// The pointer arc's zone labels and the pointer itself.
pub fn arc_labels(pointer: f32, ops: &mut Vec<Op>) {
    const ZONES: [&str; 9] = ["NOW PLAYING", "QUEUE", "FAVOURITES", "TITLES", "ALBUMS", "ARTISTS", "COVERS", "MODE", "SOURCES"];
    let arc_x = |y: f32| {
        let (cx, r) = (1400.0f32, 520.0f32);
        let dy = (y - CENTRE_Y).clamp(-r + 1.0, r - 1.0);
        cx - (r * r - dy * dy).sqrt()
    };
    for (i, z) in ZONES.iter().enumerate() {
        let y = 64.0 + i as f32 * 80.0;
        let on = (pointer / 16.0) as usize == i;
        let (size, c) = if on { (24, TEXT) } else { (20, DIM.with_opacity(0.7)) };
        // Right-aligned in the UI; the benchmark only needs the same glyph count.
        let x = arc_x(y) as i32 - 28 - (z.len() as i32 * size as i32 * 6 / 10);
        ops.push(Op::Text { x, y: y as i32 + 8, size, color: c, text: (*z).into() });
    }
    let y = 64.0 + pointer * (640.0 / 127.0);
    ops.push(Op::Fill { rect: Rect::new(arc_x(y) as i32 - 14, y as i32 - 3, 22, 6), color: ACCENT });
}

/// A frame of the cover arc animating, redrawing only its strip.
pub fn strip_frame(scroll: f32, fmt: CoverFmt) -> Frame {
    let mut ops = Vec::new();
    cover_arc(scroll, fmt, &mut ops);
    Frame { clear: Some((STRIP, BG)), ops, present: Some(STRIP) }
}

/// A whole-screen frame: list text, the cover arc, the zone labels, and a translucent volume
/// overlay; the worst case a single frame will see.
pub fn full_frame(scroll: f32, fmt: CoverFmt) -> Frame {
    let mut ops = Vec::new();
    list_rows(scroll * 0.5, &mut ops);
    cover_arc(scroll, fmt, &mut ops);
    arc_labels((scroll * 7.0).rem_euclid(127.0), &mut ops);
    ops.push(Op::Fill { rect: Rect::new(312, 600, 400, 60), color: Color::rgba(0, 0, 0, 160) });
    ops.push(Op::Fill { rect: Rect::new(322, 625, 230, 10), color: ACCENT });
    Frame { clear: Some((SCREEN, BG)), ops, present: Some(SCREEN) }
}

/// `n` covers spread over the screen (micro benchmark: composite throughput).
pub fn cover_grid(n: usize, fmt: CoverFmt, size: u16, alpha: u8, phase: usize) -> Frame {
    let per_row = (SCREEN_WIDTH as usize - size as usize) / (size as usize + 4);
    let ops = (0..n)
        .map(|i| {
            let k = i % (per_row * 6);
            Op::Cover {
                fmt,
                size,
                slot: (i + phase) % POOL,
                x: ((k % per_row) * (size as usize + 4)) as i32,
                y: ((k / per_row) * (size as usize + 4)) as i32 % (SCREEN_HEIGHT as i32 - size as i32),
                alpha,
            }
        })
        .collect();
    Frame { clear: None, ops, present: None }
}

/// Translucent fills: four full-screen dims (blanking fade) or 32 highlight bars.
pub fn fills(full_screen: bool) -> Frame {
    let ops = if full_screen {
        (0..4).map(|_| Op::Fill { rect: SCREEN, color: Color::rgba(0, 0, 0, 64) }).collect()
    } else {
        (0..32).map(|i| Op::Fill { rect: Rect::new(40, (i * 23) % 700, 900, 54), color: ACCENT.with_opacity(0.4) }).collect()
    };
    Frame { clear: None, ops, present: None }
}

/// One list view's text (16 glyph runs, about 250 glyphs).
pub fn glyphs(scroll: f32) -> Frame {
    let mut ops = Vec::new();
    list_rows(scroll, &mut ops);
    Frame { clear: None, ops, present: None }
}

pub fn glyph_count(f: &Frame) -> usize {
    f.ops.iter().map(|o| if let Op::Text { text, .. } = o { text.chars().count() } else { 0 }).sum()
}

/// Synthetic cover pixels for pool slot `slot`: a diagonal gradient in a colour of its own, with
/// a rounded-corner alpha edge for ARGB (premultiplied). Bytes as X11 and fbdev take them on x86.
pub fn cover_pixels(fmt: CoverFmt, size: u16, slot: usize) -> Vec<u8> {
    let n = size as usize;
    let (br, bg, bb) = (((slot * 67) % 200) as u32 + 40, ((slot * 131) % 200) as u32 + 40, ((slot * 29) % 200) as u32 + 40);
    let mut out = Vec::with_capacity(n * n * fmt.bytes_per_pixel());
    let radius = (n / 10) as i32;
    for y in 0..n {
        for x in 0..n {
            let g = ((x + y) * 255 / (2 * n)) as u32;
            let (r, gg, b) = ((br + g) / 2, (bg + 255 - g) / 2, (bb + g / 2) / 2);
            match fmt {
                CoverFmt::Rgb565 => {
                    let p = (((r >> 3) << 11) | ((gg >> 2) << 5) | (b >> 3)) as u16;
                    out.extend_from_slice(&p.to_le_bytes());
                }
                CoverFmt::Xrgb => out.extend_from_slice(&(0xff00_0000 | (r << 16) | (gg << 8) | b).to_le_bytes()),
                CoverFmt::Argb => {
                    // distance outside the rounded corner's circle → transparent
                    let cx = (x as i32).clamp(radius, n as i32 - 1 - radius);
                    let cy = (y as i32).clamp(radius, n as i32 - 1 - radius);
                    let dist = (((x as i32 - cx).pow(2) + (y as i32 - cy).pow(2)) as f32).sqrt();
                    let a = ((radius as f32 + 0.5 - dist).clamp(0.0, 1.0) * 255.0) as u32;
                    let pm = |v: u32| v * a / 255;
                    out.extend_from_slice(&((a << 24) | (pm(r) << 16) | (pm(gg) << 8) | pm(b)).to_le_bytes());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arc_uses_only_baked_sizes_and_stays_on_screen() {
        for k in 0..40 {
            let f = strip_frame(k as f32 * 0.13, CoverFmt::Xrgb);
            let covers: Vec<_> =
                f.ops.iter().filter_map(|o| if let Op::Cover { size, y, .. } = o { Some((*size, *y)) } else { None }).collect();
            assert!(covers.len() >= 12);
            assert!(covers.iter().all(|(s, _)| ARC_SIZES.contains(s)));
            assert!(covers.iter().any(|(s, _)| *s == 96));
        }
    }

    #[test]
    fn cover_pixels_have_the_right_length() {
        for fmt in CoverFmt::ALL {
            for size in ALL_SIZES {
                assert_eq!(cover_pixels(fmt, size, 3).len(), size as usize * size as usize * fmt.bytes_per_pixel());
            }
        }
    }

    #[test]
    fn a_list_frame_has_a_few_hundred_glyphs() {
        let n = glyph_count(&glyphs(3.4));
        assert!((150..400).contains(&n), "{n}");
    }
}
