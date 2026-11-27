//! Textures, default covers and the face the UI draws with.
//!
//! The stock look comes from a handful of B&O files on the BM5's own Windows disk: BeoFont.ttf,
//! the arc and glow textures and the default covers. Those are B&O's copyright and are not in
//! this repository. Point `NEOMASTER_STOCK_ASSETS` at that disk (or at the BeoMaster 5 install
//! folder, or at a folder holding copies of the files) to use them. Anything missing is drawn
//! here instead, from measurements of the stock screen, so the UI looks close without them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use log::{info, warn};
use render::{Image, DEFAULT_FONT};

/// The environment variable naming where the stock files are.
pub const STOCK_ASSETS_ENV: &str = "NEOMASTER_STOCK_ASSETS";

const INSTALL: &str = "Program Files/Bang & Olufsen/BeoMaster 5";
const TEXTURES: &str = "Resources/Texture/A18";
const DEFAULTS: &str = "Resources/Defaults";

#[derive(Clone)]
pub struct Assets {
    /// TrueType bytes for every text run.
    pub font: Arc<[u8]>,
    /// The menu arc, 90x768, drawn at x = 260 with additive blending (Arc.png).
    pub arc: Arc<Image>,
    /// White glow at the pointer, drawn additively and scaled to about 44 px (ArcFlare.png).
    pub flare: Arc<Image>,
    /// The soft blue beam through the pointer, along the arc's radius. Always drawn here: the
    /// stock one is made in code, and no texture on the disk matches it.
    pub beam: Arc<Image>,
    /// The volume arc, white, tinted blue when drawn (VolumeLightArc.png, 198x560).
    pub volume_arc: Arc<Image>,
    /// Default covers for items without art: 512 (now-playing), 108 (centre), 81 (others).
    pub cover_large: Arc<Image>,
    pub cover_medium: Arc<Image>,
    pub cover_small: Arc<Image>,
    /// How many of the stock files were found.
    pub stock_files: usize,
}

impl Assets {
    /// Everything drawn here; nothing read from disk.
    pub fn builtin() -> Assets {
        Assets {
            font: Arc::from(DEFAULT_FONT),
            arc: draw_arc(),
            flare: draw_flare(),
            beam: draw_beam(),
            volume_arc: draw_volume_arc(),
            cover_large: draw_cover(512),
            cover_medium: draw_cover(108),
            cover_small: draw_cover(81),
            stock_files: 0,
        }
    }

    /// The stock files under `root` where they exist, drawn fallbacks for the rest. `root` may
    /// be the BM5's Windows disk (C:), its BeoMaster 5 install folder, or a flat folder of copies.
    pub fn load(root: &Path) -> Assets {
        let mut a = Assets::builtin();
        let mut found = 0;
        let mut png = |name: &str, sub: &str, slot: &mut Arc<Image>| {
            let Some(path) = find(root, sub, name) else { return };
            match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| Image::from_png(&b).map_err(|e| e.to_string())) {
                Ok(img) => {
                    *slot = img;
                    found += 1;
                }
                Err(e) => warn!("{}: {e}; drawing it instead", path.display()),
            }
        };
        png("Arc.png", TEXTURES, &mut a.arc);
        png("ArcFlare.png", TEXTURES, &mut a.flare);
        png("VolumeLightArc.png", TEXTURES, &mut a.volume_arc);
        png("LargeCover.png", DEFAULTS, &mut a.cover_large);
        png("MediumCover.png", DEFAULTS, &mut a.cover_medium);
        png("SmallCover.png", DEFAULTS, &mut a.cover_small);
        let font = [root.join("WINDOWS/Fonts/BeoFont.ttf"), root.join("BeoFont.ttf")].into_iter().find(|p| p.is_file());
        if let Some(path) = font {
            match std::fs::read(&path) {
                Ok(bytes) if render::Font::from_bytes(&bytes).is_ok() => {
                    a.font = Arc::from(bytes);
                    found += 1;
                }
                _ => warn!("{}: not a usable font; using the bundled one", path.display()),
            }
        }
        a.stock_files = found;
        info!("Stock UI assets: {found} of 7 found under {}", root.display());
        a
    }

    /// [`Assets::load`] from `$NEOMASTER_STOCK_ASSETS`, or [`Assets::builtin`] if it is unset.
    pub fn from_env() -> Assets {
        match std::env::var_os(STOCK_ASSETS_ENV) {
            Some(dir) if !dir.is_empty() => Assets::load(Path::new(&dir)),
            _ => Assets::builtin(),
        }
    }

    pub fn font(&self) -> render::Font {
        render::Font::from_bytes(&self.font).expect("checked when loaded")
    }
}

fn find(root: &Path, sub: &str, name: &str) -> Option<PathBuf> {
    [root.join(INSTALL).join(sub).join(name), root.join(sub).join(name), root.join(name)].into_iter().find(|p| p.is_file())
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn byte(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// The menu arc: a thin line on the circle about (1153, 384) with radius 893, blue at the top
/// to cyan at the bottom, fading out at both ends. Colours from the stock screen at y = 100,
/// 200, 384, 500, 600 and 700.
fn draw_arc() -> Arc<Image> {
    const STOPS: [(f32, f32); 7] =
        [(0.0, 0.0), (100.0, 5.0), (200.0, 55.0), (384.0, 136.0), (500.0, 197.0), (600.0, 239.0), (768.0, 245.0)];
    let (cx, cy, r) = (893.0f32, 384.0f32, 893.0f32);
    Image::from_fn(90, 768, |x, y| {
        let d = ((x - cx).hypot(y - cy) - r).abs();
        let line = (-(d / 1.5).powi(2)).exp();
        let i = STOPS.iter().position(|s| s.0 > y).unwrap_or(STOPS.len() - 1).max(1);
        let (a, b) = (STOPS[i - 1], STOPS[i]);
        let g = lerp(a.1, b.1, (y - a.0) / (b.0 - a.0));
        // the line thins out over its last 40 px at each end
        let ends = ((y - 18.0) / 40.0).min((750.0 - y) / 40.0).clamp(0.0, 1.0);
        [0, byte(g), 255, byte(255.0 * line * ends)]
    })
}

/// The pointer's glow: a white point with an exponential halo, the size of the stock texture
/// (177 px, drawn at a quarter of that).
fn draw_flare() -> Arc<Image> {
    Image::from_fn(177, 177, |x, y| {
        let r = (x - 88.5).hypot(y - 88.5);
        let a = (0.74 * (-r / 24.0).exp() + 0.26 * (-(r / 5.0).powi(2)).exp()).min(1.0);
        [255, 255, 255, byte(255.0 * a)]
    })
}

/// The beam: Gaussian along (sigma 200 px) and across (sigma 15 px), peaking at rgb(0, 12, 42)
/// once added to black, as measured through the pointer on the stock screen.
fn draw_beam() -> Arc<Image> {
    Image::from_fn(640, 96, |x, y| {
        let (t, s) = (x - 320.0, y - 48.0);
        let a = (-(t / 200.0).powi(2)).exp() * (-(s / 15.0).powi(2)).exp() * (42.0 / 255.0);
        [0, 73, 255, byte(255.0 * a)]
    })
}

/// The volume arc: a glowing white line on a circle of radius 270 centred at (284, 280).
fn draw_volume_arc() -> Arc<Image> {
    Image::from_fn(198, 560, |x, y| {
        let d = ((x - 284.0).hypot(y - 280.0) - 270.0).abs();
        let a = (-(d / 1.4).powi(2)).exp() + 0.35 * (-(d / 4.0).powi(2)).exp();
        let ends = ((y - 8.0) / 30.0).min((552.0 - y) / 30.0).clamp(0.0, 1.0);
        [255, 255, 255, byte(255.0 * a.min(1.0) * ends)]
    })
}

/// A plain record sleeve: dark grain-free grey, a darker disc peeking out on the right and a
/// light one-pixel frame, in the spirit of the stock default cover.
fn draw_cover(size: u32) -> Arc<Image> {
    let s = size as f32;
    Image::from_fn(size, size, |x, y| {
        let edge = x < 1.0 || y < 1.0 || x > s - 1.0 || y > s - 1.0;
        if edge {
            return [228, 229, 230, 255];
        }
        let (dx, dy) = (x - s, y - s / 2.0);
        let r = dx.hypot(dy);
        let disc = s * 0.49;
        let v = if r < s * 0.12 {
            88.0
        } else if r < s * 0.17 {
            52.0
        } else if (r - disc).abs() < (s / 300.0).max(0.6) {
            80.0
        } else if r < disc {
            40.0
        } else {
            46.0 - 6.0 * (y / s)
        };
        [byte(v), byte(v), byte(v), 255]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_assets_have_the_stock_sizes() {
        let a = Assets::builtin();
        assert_eq!((a.arc.width, a.arc.height), (90, 768));
        assert_eq!((a.volume_arc.width, a.volume_arc.height), (198, 560));
        assert_eq!(a.cover_large.width, 512);
        assert_eq!(a.cover_medium.width, 108);
        assert_eq!(a.cover_small.width, 81);
        assert!(a.font().has_glyph('A'));
    }

    #[test]
    fn missing_stock_folder_falls_back() {
        let a = Assets::load(Path::new("/nonexistent/neomaster"));
        assert_eq!(a.stock_files, 0);
    }
}
