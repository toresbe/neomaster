//! XRender backend via x11rb.
//!
//! Glyphs live in a server-side glyph set (A8), uploaded the first time each (character, size)
//! is drawn. A frame is a sequence of FillRectangles and CompositeGlyphs calls into a back-buffer
//! pixmap, then one Composite to the window. Images are uploaded once into ARGB32 pictures and
//! composited with a transform (scale, and rotation for the pointer beam) and bilinear filtering;
//! opacity is a solid alpha mask and a tint is a solid source through the image as mask.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::render::{
    self, ConnectionExt as _, CreatePictureAux, Fixed, Glyphinfo, PictOp, PictType, Pictformat, Picture, Transform,
};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, ImageFormat, PropMode, Rectangle, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::glyphs::{Font, GlyphKey};
use crate::{Align, Blend, Color, ImageDraw, Primitive, Renderer, Scene};

#[derive(Debug)]
pub struct Error(Box<dyn std::error::Error + Send + Sync>);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "X11: {}", self.0)
    }
}

impl std::error::Error for Error {}

macro_rules! from_x11 {
    ($($t:ty),*) => { $(impl From<$t> for Error { fn from(e: $t) -> Self { Error(Box::new(e)) } })* };
}
from_x11!(x11rb::errors::ConnectError, x11rb::errors::ConnectionError, x11rb::errors::ReplyError, x11rb::errors::ReplyOrIdError);

impl From<&'static str> for Error {
    fn from(e: &'static str) -> Self {
        Error(e.into())
    }
}

pub struct XRenderer {
    conn: Arc<RustConnection>,
    window: Window,
    width: u16,
    height: u16,
    back_pict: Picture,
    win_pict: Picture,
    a8: Pictformat,
    argb32: Pictformat,
    glyphset: render::Glyphset,
    images: HashMap<u64, Picture>,
    font: Font,
    glyph_ids: HashMap<GlyphKey, u32>,
    next_glyph: u32,
    solids: HashMap<Color, Picture>,
    pub wm_delete_window: u32,
}

fn xcolor(c: Color) -> render::Color {
    // XRender wants premultiplied 16-bit channels
    let a = c.a as u32;
    let pm = |v: u8| ((v as u32 * a / 255) * 257) as u16;
    render::Color { red: pm(c.r), green: pm(c.g), blue: pm(c.b), alpha: (a * 257) as u16 }
}

impl XRenderer {
    /// Connect to `$DISPLAY` and open a `width` x `height` window titled `title`: the
    /// simulator's screen. Input events (keys, buttons, motion) are selected on the window.
    pub fn open_window(title: &str, width: u16, height: u16) -> Result<XRenderer, Error> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let conn = Arc::new(conn);
        let screen = conn.setup().roots[screen_num].clone();

        let window = conn.generate_id()?;
        let events = EventMask::KEY_PRESS
            | EventMask::KEY_RELEASE
            | EventMask::BUTTON_PRESS
            | EventMask::POINTER_MOTION
            | EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY;
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(screen.black_pixel).event_mask(events),
        )?;
        conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
        let protocols = conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
        let delete = conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
        conn.change_property32(PropMode::REPLACE, window, protocols, AtomEnum::ATOM, &[delete])?;
        conn.map_window(window)?;

        let version = conn.render_query_version(0, 11)?.reply()?;
        if (version.major_version, version.minor_version) < (0, 10) {
            return Err("XRender 0.10 or later is needed (solid fills)".into());
        }
        let formats = conn.render_query_pict_formats()?.reply()?;
        let a8 = formats
            .formats
            .iter()
            .find(|f| f.type_ == PictType::DIRECT && f.depth == 8 && f.direct.alpha_mask == 0xff)
            .ok_or("no A8 picture format")?
            .id;
        let argb32 = formats
            .formats
            .iter()
            .find(|f| {
                f.type_ == PictType::DIRECT
                    && f.depth == 32
                    && f.direct.alpha_shift == 24
                    && f.direct.red_shift == 16
                    && f.direct.green_shift == 8
                    && f.direct.blue_shift == 0
            })
            .ok_or("no ARGB32 picture format")?
            .id;
        let win_format = formats
            .screens
            .iter()
            .flat_map(|s| &s.depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual == screen.root_visual)
            .ok_or("no picture format for the root visual")?
            .format;

        let back = conn.generate_id()?;
        conn.create_pixmap(screen.root_depth, back, window, width, height)?;
        let back_pict = conn.generate_id()?;
        conn.render_create_picture(back_pict, back, win_format, &CreatePictureAux::new())?;
        let win_pict = conn.generate_id()?;
        conn.render_create_picture(win_pict, window, win_format, &CreatePictureAux::new())?;
        let glyphset = conn.generate_id()?;
        conn.render_create_glyph_set(glyphset, a8)?;
        conn.flush()?;

        Ok(XRenderer {
            conn,
            window,
            width,
            height,
            back_pict,
            win_pict,
            a8,
            argb32,
            glyphset,
            images: HashMap::new(),
            font: Font::default_font(),
            glyph_ids: HashMap::new(),
            next_glyph: 1,
            solids: HashMap::new(),
            wm_delete_window: delete,
        })
    }

    /// The connection, shared with the simulator's input thread.
    pub fn connection(&self) -> Arc<RustConnection> {
        self.conn.clone()
    }

    pub fn window(&self) -> Window {
        self.window
    }

    /// Replace the face text is drawn in (glyphs already uploaded are dropped).
    pub fn set_font(&mut self, font: Font) -> Result<(), Error> {
        self.conn.render_free_glyph_set(self.glyphset)?;
        let glyphset = self.conn.generate_id()?;
        self.conn.render_create_glyph_set(glyphset, self.a8)?;
        self.glyphset = glyphset;
        self.glyph_ids.clear();
        self.next_glyph = 1;
        self.font = font;
        Ok(())
    }

    fn solid(&mut self, c: Color) -> Result<Picture, Error> {
        if let Some(p) = self.solids.get(&c) {
            return Ok(*p);
        }
        if self.solids.len() >= 1024 {
            // fades make many one-off colours; start over rather than grow without bound
            for (_, p) in self.solids.drain() {
                self.conn.render_free_picture(p)?;
            }
        }
        let p = self.conn.generate_id()?;
        self.conn.render_create_solid_fill(p, xcolor(c))?;
        self.solids.insert(c, p);
        Ok(p)
    }

    fn glyph(&mut self, key: GlyphKey) -> Result<u32, Error> {
        if let Some(id) = self.glyph_ids.get(&key) {
            return Ok(*id);
        }
        let g = self.font.rasterize(key);
        // A8 rows are padded to 4 bytes
        let stride = (g.width as usize + 3) & !3;
        let mut data = vec![0u8; stride * g.height as usize];
        for row in 0..g.height as usize {
            let w = g.width as usize;
            data[row * stride..row * stride + w].copy_from_slice(&g.coverage[row * w..row * w + w]);
        }
        let info = Glyphinfo { width: g.width, height: g.height, x: -g.left, y: g.top, x_off: g.advance, y_off: 0 };
        let id = self.next_glyph;
        self.next_glyph += 1;
        self.conn.render_add_glyphs(self.glyphset, &[id], &[info], &data)?;
        self.glyph_ids.insert(key, id);
        Ok(id)
    }

    /// The picture holding `image`, uploading it the first time.
    fn image_picture(&mut self, image: &crate::Image) -> Result<Picture, Error> {
        if let Some(p) = self.images.get(&image.id()) {
            return Ok(*p);
        }
        let (w, h) = (image.width as u16, image.height as u16);
        let pixmap = self.conn.generate_id()?;
        self.conn.create_pixmap(32, pixmap, self.window, w, h)?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, pixmap, &Default::default())?;
        let data = image.premultiplied_bgra();
        let stride = image.width as usize * 4;
        // stay well inside the maximum request length
        let rows_per = ((self.conn.maximum_request_bytes().saturating_sub(64)) / stride.max(1)).clamp(1, 4096);
        for (i, chunk) in data.chunks(stride * rows_per).enumerate() {
            let rows = (chunk.len() / stride) as u16;
            let y = (i * rows_per) as i16;
            self.conn.put_image(ImageFormat::Z_PIXMAP, pixmap, gc, w, rows, 0, y, 0, 32, chunk)?;
        }
        self.conn.free_gc(gc)?;
        let pict = self.conn.generate_id()?;
        self.conn.render_create_picture(pict, pixmap, self.argb32, &CreatePictureAux::new())?;
        self.conn.render_set_picture_filter(pict, b"bilinear", &[])?;
        self.conn.free_pixmap(pixmap)?;
        self.images.insert(image.id(), pict);
        Ok(pict)
    }

    fn draw_image(&mut self, d: &ImageDraw) -> Result<(), Error> {
        let pict = self.image_picture(&d.image)?;
        let op = match d.blend {
            Blend::Over => PictOp::OVER,
            Blend::Add => PictOp::ADD,
        };
        let sx = d.src.w as f64 / d.dst.w as f64;
        let sy = d.src.h as f64 / d.dst.h as f64;
        // The area composited, in screen pixels, and the transform from that area's pixels to
        // image pixels (XRender transforms map destination to source).
        let (bx, by, bw, bh, m) = if d.angle == 0.0 {
            let m = [sx, 0.0, d.src.x as f64, 0.0, sy, d.src.y as f64];
            (d.dst.x, d.dst.y, d.dst.w, d.dst.h, m)
        } else {
            let (cx, cy) = (d.dst.x as f64 + d.dst.w as f64 / 2.0, d.dst.y as f64 + d.dst.h as f64 / 2.0);
            let (s, c) = (d.angle as f64).sin_cos();
            let (hw, hh) = (d.dst.w as f64 / 2.0, d.dst.h as f64 / 2.0);
            let ex = (hw * c).abs() + (hh * s).abs();
            let ey = (hw * s).abs() + (hh * c).abs();
            let (bx, by) = ((cx - ex).floor(), (cy - ey).floor());
            // area pixel (i, j) -> screen (bx + i, by + j) -> rotate by -angle about the centre
            // -> dst-local -> image
            let (ox, oy) = (bx - cx, by - cy);
            let (icx, icy) = (d.src.x as f64 + d.src.w as f64 / 2.0, d.src.y as f64 + d.src.h as f64 / 2.0);
            let m = [c * sx, s * sx, (c * ox + s * oy) * sx + icx, -s * sy, c * sy, (-s * ox + c * oy) * sy + icy];
            (bx as i32, by as i32, (2.0 * ex).ceil() as u32 + 1, (2.0 * ey).ceil() as u32 + 1, m)
        };
        let fx = |v: f64| -> Fixed { (v * 65536.0).round() as Fixed };
        let t = Transform {
            matrix11: fx(m[0]),
            matrix12: fx(m[1]),
            matrix13: fx(m[2]),
            matrix21: fx(m[3]),
            matrix22: fx(m[4]),
            matrix23: fx(m[5]),
            matrix31: 0,
            matrix32: 0,
            matrix33: fx(1.0),
        };
        self.conn.render_set_picture_transform(pict, t)?;
        let (w, h) = (bw.min(u16::MAX as u32) as u16, bh.min(u16::MAX as u32) as u16);
        match d.tint {
            Some(tint) => {
                let src = self.solid(tint.with_opacity(d.opacity))?;
                self.conn.render_composite(op, src, pict, self.back_pict, 0, 0, 0, 0, bx as i16, by as i16, w, h)?;
            }
            None => {
                let mask =
                    if d.opacity >= 1.0 { x11rb::NONE } else { self.solid(Color::rgba(0, 0, 0, 255).with_opacity(d.opacity))? };
                self.conn.render_composite(op, pict, mask, self.back_pict, 0, 0, 0, 0, bx as i16, by as i16, w, h)?;
            }
        }
        Ok(())
    }

    fn draw_text(&mut self, x: i32, y: i32, size: u16, color: Color, align: Align, text: &str) -> Result<(), Error> {
        if text.is_empty() || color.a == 0 {
            return Ok(());
        }
        let x = match align {
            Align::Left => x,
            Align::Center => x - self.font.width(text, size) / 2,
            Align::Right => x - self.font.width(text, size),
        };
        let ids: Vec<u32> = text.chars().map(|ch| self.glyph(GlyphKey { ch, size })).collect::<Result<_, _>>()?;
        // GLYPHELT32: count, 3 pad bytes, dx, dy, then the ids; at most 254 glyphs per element
        let mut cmds = Vec::with_capacity(8 + ids.len() * 4);
        for (i, chunk) in ids.chunks(254).enumerate() {
            cmds.push(chunk.len() as u8);
            cmds.extend_from_slice(&[0, 0, 0]);
            let (dx, dy) = if i == 0 { (x as i16, y as i16) } else { (0, 0) };
            cmds.extend_from_slice(&dx.to_ne_bytes());
            cmds.extend_from_slice(&dy.to_ne_bytes());
            for id in chunk {
                cmds.extend_from_slice(&id.to_ne_bytes());
            }
        }
        let src = self.solid(color)?;
        self.conn.render_composite_glyphs32(PictOp::OVER, src, self.back_pict, self.a8, self.glyphset, 0, 0, &cmds)?;
        Ok(())
    }
}

impl Renderer for XRenderer {
    type Error = Error;

    fn render(&mut self, scene: &Scene) -> Result<(), Error> {
        let full = Rectangle { x: 0, y: 0, width: self.width, height: self.height };
        self.conn.render_fill_rectangles(PictOp::SRC, self.back_pict, xcolor(scene.background), &[full])?;
        for item in &scene.items {
            match item {
                Primitive::Fill { rect, color } => {
                    let r = Rectangle { x: rect.x as i16, y: rect.y as i16, width: rect.w as u16, height: rect.h as u16 };
                    self.conn.render_fill_rectangles(PictOp::OVER, self.back_pict, xcolor(*color), &[r])?;
                }
                Primitive::Text { x, y, size, color, align, text } => self.draw_text(*x, *y, *size, *color, *align, text)?,
                Primitive::Image(d) => self.draw_image(d)?,
            }
        }
        self.conn.render_composite(
            PictOp::SRC,
            self.back_pict,
            x11rb::NONE,
            self.win_pict,
            0,
            0,
            0,
            0,
            0,
            0,
            self.width,
            self.height,
        )?;
        self.conn.flush()?;
        Ok(())
    }
}
