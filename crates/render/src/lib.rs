//! What the UI draws and how it gets to the screen.
//!
//! Every frame is a back-to-front list of a few primitive kinds ([`Primitive`]): fills, text and
//! images. Images may be scaled, cropped, tinted, faded and (for the pointer beam only) rotated;
//! the stock UI blends its arc, flare and volume textures additively, so [`Blend::Add`] exists
//! too. Backends implement [`Renderer`]. The XRender backend
//! ([`xrender::XRenderer`]) runs unchanged on the BM5's bare Xorg (openchrome) and in a window on
//! any X desktop, which is what `neomaster --sim` uses. A framebuffer blitter is the fallback if
//! openchrome's acceleration disappoints (milestone M0 decides).

pub mod glyphs;
pub mod image;
#[cfg(feature = "xrender")]
pub mod xrender;

pub use glyphs::{Font, GlyphKey, DEFAULT_FONT};
pub use image::Image;

use std::sync::Arc;

/// Premultiplication happens in the backend; colours here are straight ARGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }
    /// The same colour at `opacity` (0.0-1.0) of its alpha.
    pub fn with_opacity(self, opacity: f32) -> Color {
        Color { a: (self.a as f32 * opacity.clamp(0.0, 1.0)).round() as u8, ..self }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// How an image lands on what is already drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Blend {
    /// Ordinary alpha blending.
    #[default]
    Over,
    /// `dst += src * alpha`: Direct3D's SrcAlpha/One, which the stock shaders use for the arc,
    /// the pointer flare and the volume arc.
    Add,
}

/// An image placed on screen. `src` (in image pixels) is scaled to `dst`; with `angle` set the
/// result turns by that many radians about the centre of `dst`.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageDraw {
    pub image: Arc<Image>,
    pub src: Rect,
    pub dst: Rect,
    pub opacity: f32,
    pub blend: Blend,
    /// Draw this colour through the image's alpha instead of the image's own colours (for white
    /// glow textures that the stock UI tints).
    pub tint: Option<Color>,
    pub angle: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    /// Solid or translucent rectangle (highlight bar, placeholder tile, dimming).
    Fill {
        rect: Rect,
        color: Color,
    },
    /// A run of text from the glyph atlas. `x`/`y` is the anchor on the baseline.
    Text {
        x: i32,
        y: i32,
        size: u16,
        color: Color,
        align: Align,
        text: String,
    },
    Image(ImageDraw),
}

/// One frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub background: Color,
    pub items: Vec<Primitive>,
}

impl Scene {
    pub fn new(background: Color) -> Scene {
        Scene { background, items: Vec::new() }
    }
    pub fn fill(&mut self, rect: Rect, color: Color) {
        self.items.push(Primitive::Fill { rect, color });
    }
    pub fn text(&mut self, x: i32, y: i32, size: u16, color: Color, align: Align, text: impl Into<String>) {
        self.items.push(Primitive::Text { x, y, size, color, align, text: text.into() });
    }
    /// The whole image scaled into `dst`, blended over.
    pub fn image(&mut self, image: &Arc<Image>, dst: Rect, opacity: f32) {
        self.draw(ImageDraw {
            src: image.rect(),
            image: image.clone(),
            dst,
            opacity,
            blend: Blend::Over,
            tint: None,
            angle: 0.0,
        });
    }
    pub fn draw(&mut self, d: ImageDraw) {
        if d.opacity > 0.0 && d.dst.w > 0 && d.dst.h > 0 && d.src.w > 0 && d.src.h > 0 {
            self.items.push(Primitive::Image(d));
        }
    }
}

/// The BeoSound 5 screen.
pub const SCREEN_WIDTH: u16 = 1024;
pub const SCREEN_HEIGHT: u16 = 768;

pub trait Renderer {
    type Error: std::error::Error + Send + Sync + 'static;
    /// Draw one frame and show it.
    fn render(&mut self, scene: &Scene) -> Result<(), Self::Error>;
}
