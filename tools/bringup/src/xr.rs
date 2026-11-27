//! The XRender path, as `render::xrender` does it and as covers will: server-side pixmaps
//! composited into a back-buffer pixmap, glyphs from a server-side A8 glyph set, then one
//! Composite of the presented region to the window.
//!
//! Timing needs a real end point: X requests are asynchronous, and with EXA a reply only means
//! the server has queued the work for the GPU. [`XrBackend::sync`] reads one pixel back with
//! GetImage, which makes EXA wait for the engine (or migrate the pixmap) first.

use std::collections::HashMap;
use std::time::Instant;

use ::render::{Color, Font, GlyphKey, SCREEN_HEIGHT, SCREEN_WIDTH};
use serde_json::{json, Value};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::render::{
    self as xrr, ConnectionExt as _, CreatePictureAux, Glyphinfo, PictOp, PictType, Pictformat, Picture,
};
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateGCAux, CreateWindowAux, Drawable, Gcontext, ImageFormat, Pixmap, Rectangle, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::scene::{self, CoverFmt, Frame, Op, POOL};
use crate::stats::Samples;
use crate::Backend;

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    format!("X11: {err}")
}

fn xcolor(c: Color) -> xrr::Color {
    let a = c.a as u32;
    let pm = |v: u8| ((v as u32 * a / 255) * 257) as u16;
    xrr::Color { red: pm(c.r), green: pm(c.g), blue: pm(c.b), alpha: (a * 257) as u16 }
}

struct Cover {
    pixmap: Pixmap,
    pict: Picture,
}

pub struct XrBackend {
    conn: RustConnection,
    window: Window,
    root_depth: u8,
    back: Pixmap,
    back_pict: Picture,
    win_pict: Picture,
    a8: Pictformat,
    formats: HashMap<CoverFmt, (u8, Pictformat)>,
    gcs: HashMap<u8, Gcontext>,
    covers: HashMap<(CoverFmt, u16), Vec<Cover>>,
    glyphset: xrr::Glyphset,
    font: Font,
    glyph_ids: HashMap<GlyphKey, u32>,
    solids: HashMap<Color, Picture>,
    masks: HashMap<u8, Picture>,
    info: Value,
    presented: bool,
}

impl XrBackend {
    /// Connect to `$DISPLAY` and cover the screen with an override-redirect window (no window
    /// manager on the BM5; on a desktop it simply sits on top).
    pub fn open() -> R<XrBackend> {
        let (conn, screen_num) = x11rb::connect(None).map_err(e)?;
        let screen = conn.setup().roots[screen_num].clone();
        let (w, h) = (SCREEN_WIDTH, SCREEN_HEIGHT);

        let version = conn.render_query_version(0, 11).map_err(e)?.reply().map_err(e)?;
        if (version.major_version, version.minor_version) < (0, 10) {
            return Err("XRender 0.10 or later is needed".into());
        }
        let pf = conn.render_query_pict_formats().map_err(e)?.reply().map_err(e)?;
        let find = |depth: u8, red: (u16, u16), alpha: (u16, u16)| {
            pf.formats
                .iter()
                .find(|f| {
                    f.type_ == PictType::DIRECT
                        && f.depth == depth
                        && (f.direct.red_shift, f.direct.red_mask) == red
                        && (f.direct.alpha_shift, f.direct.alpha_mask) == alpha
                })
                .map(|f| f.id)
        };
        let a8 = find(8, (0, 0), (0, 0xff)).ok_or("no A8 picture format")?;
        let pixmap_depths: Vec<u8> = conn.setup().pixmap_formats.iter().map(|f| f.depth).collect();
        let mut formats = HashMap::new();
        for (fmt, depth, red, alpha) in [
            (CoverFmt::Xrgb, 24u8, (16u16, 0xffu16), (0u16, 0u16)),
            (CoverFmt::Rgb565, 16, (11, 0x1f), (0, 0)),
            (CoverFmt::Argb, 32, (16, 0xff), (24, 0xff)),
        ] {
            if let (Some(f), true) = (find(depth, red, alpha), pixmap_depths.contains(&depth)) {
                formats.insert(fmt, (depth, f));
            }
        }
        let win_format = pf
            .screens
            .iter()
            .flat_map(|s| &s.depths)
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual == screen.root_visual)
            .ok_or("no picture format for the root visual")?
            .format;

        let window = conn.generate_id().map_err(e)?;
        conn.create_window(
            screen.root_depth,
            window,
            screen.root,
            0,
            0,
            w,
            h,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(screen.black_pixel).override_redirect(1),
        )
        .map_err(e)?;
        conn.map_window(window).map_err(e)?;

        let back = conn.generate_id().map_err(e)?;
        conn.create_pixmap(screen.root_depth, back, window, w, h).map_err(e)?;
        let back_pict = conn.generate_id().map_err(e)?;
        conn.render_create_picture(back_pict, back, win_format, &CreatePictureAux::new()).map_err(e)?;
        let win_pict = conn.generate_id().map_err(e)?;
        conn.render_create_picture(win_pict, window, win_format, &CreatePictureAux::new()).map_err(e)?;
        let glyphset = conn.generate_id().map_err(e)?;
        conn.render_create_glyph_set(glyphset, a8).map_err(e)?;

        let ext = |name: &'static str| conn.extension_information(name).ok().flatten().is_some();
        let info = json!({
            "vendor": String::from_utf8_lossy(&conn.setup().vendor),
            "release": conn.setup().release_number,
            "render": format!("{}.{}", version.major_version, version.minor_version),
            "root_depth": screen.root_depth,
            "screen": [screen.width_in_pixels, screen.height_in_pixels],
            "pixmap_depths": pixmap_depths,
            "cover_formats": formats.keys().map(|f| f.name()).collect::<Vec<_>>(),
            "max_request_bytes": conn.maximum_request_bytes(),
            "mit_shm": ext("MIT-SHM"),
            "present": ext("Present"),
            "composite": ext("Composite"),
        });
        conn.flush().map_err(e)?;
        let mut b = XrBackend {
            conn,
            window,
            root_depth: screen.root_depth,
            back,
            back_pict,
            win_pict,
            a8,
            formats,
            gcs: HashMap::new(),
            covers: HashMap::new(),
            glyphset,
            font: Font::default_font(),
            glyph_ids: HashMap::new(),
            solids: HashMap::new(),
            masks: HashMap::new(),
            info,
            presented: false,
        };
        // Clear the window once so the screen shows the benchmark, not leftovers.
        b.conn
            .render_fill_rectangles(
                PictOp::SRC,
                b.win_pict,
                xcolor(Color::BLACK),
                &[Rectangle { x: 0, y: 0, width: w, height: h }],
            )
            .map_err(e)?;
        b.sync()?;
        Ok(b)
    }

    fn gc(&mut self, depth: u8, drawable: Drawable) -> R<Gcontext> {
        if let Some(g) = self.gcs.get(&depth) {
            return Ok(*g);
        }
        let g = self.conn.generate_id().map_err(e)?;
        self.conn.create_gc(g, drawable, &CreateGCAux::new()).map_err(e)?;
        self.gcs.insert(depth, g);
        Ok(g)
    }

    fn new_cover(&mut self, fmt: CoverFmt, size: u16, pixels: &[u8]) -> R<Cover> {
        let (depth, format) = *self.formats.get(&fmt).ok_or_else(|| format!("{} pixmaps not supported", fmt.name()))?;
        let pixmap = self.conn.generate_id().map_err(e)?;
        self.conn.create_pixmap(depth, pixmap, self.window, size, size).map_err(e)?;
        let gc = self.gc(depth, pixmap)?;
        self.conn.put_image(ImageFormat::Z_PIXMAP, pixmap, gc, size, size, 0, 0, 0, depth, pixels).map_err(e)?;
        let pict = self.conn.generate_id().map_err(e)?;
        self.conn.render_create_picture(pict, pixmap, format, &CreatePictureAux::new()).map_err(e)?;
        Ok(Cover { pixmap, pict })
    }

    fn free_cover(&mut self, c: Cover) -> R<()> {
        self.conn.render_free_picture(c.pict).map_err(e)?;
        self.conn.free_pixmap(c.pixmap).map_err(e)?;
        Ok(())
    }

    fn solid(&mut self, c: Color) -> R<Picture> {
        if let Some(p) = self.solids.get(&c) {
            return Ok(*p);
        }
        let p = self.conn.generate_id().map_err(e)?;
        self.conn.render_create_solid_fill(p, xcolor(c)).map_err(e)?;
        self.solids.insert(c, p);
        Ok(p)
    }

    /// A solid alpha-only mask (cover fades), quantised to 32 levels as a UI cache would.
    fn mask(&mut self, alpha: u8) -> R<Picture> {
        let q = alpha | 0x07;
        if let Some(p) = self.masks.get(&q) {
            return Ok(*p);
        }
        let p = self.conn.generate_id().map_err(e)?;
        self.conn.render_create_solid_fill(p, xrr::Color { red: 0, green: 0, blue: 0, alpha: q as u16 * 257 }).map_err(e)?;
        self.masks.insert(q, p);
        Ok(p)
    }

    fn glyph(&mut self, key: GlyphKey) -> R<u32> {
        if let Some(id) = self.glyph_ids.get(&key) {
            return Ok(*id);
        }
        let g = self.font.rasterize(key);
        let stride = (g.width as usize + 3) & !3;
        let mut data = vec![0u8; stride * g.height as usize];
        for row in 0..g.height as usize {
            let w = g.width as usize;
            data[row * stride..row * stride + w].copy_from_slice(&g.coverage[row * w..row * w + w]);
        }
        let info = Glyphinfo { width: g.width, height: g.height, x: -g.left, y: g.top, x_off: g.advance, y_off: 0 };
        let id = self.glyph_ids.len() as u32 + 1;
        self.conn.render_add_glyphs(self.glyphset, &[id], &[info], &data).map_err(e)?;
        self.glyph_ids.insert(key, id);
        Ok(id)
    }

    fn text(&mut self, x: i32, y: i32, size: u16, color: Color, text: &str) -> R<()> {
        if text.is_empty() || color.a == 0 {
            return Ok(());
        }
        let ids: Vec<u32> = text.chars().map(|ch| self.glyph(GlyphKey { ch, size })).collect::<R<_>>()?;
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
        self.conn.render_composite_glyphs32(PictOp::OVER, src, self.back_pict, self.a8, self.glyphset, 0, 0, &cmds).map_err(e)?;
        Ok(())
    }

    /// Cover uploads: `count` PutImages of one (fmt, size) cover into pool pixmaps, synced every
    /// 32. What scrolling into covers that are not resident yet costs.
    pub fn upload(&mut self, fmt: CoverFmt, size: u16, count: usize) -> R<Value> {
        self.prepare(fmt, size)?;
        let (depth, _) = self.formats[&fmt];
        let pixels: Vec<Vec<u8>> = (0..4).map(|s| scene::cover_pixels(fmt, size, s)).collect();
        let pool: Vec<Pixmap> = self.covers[&(fmt, size)].iter().map(|c| c.pixmap).collect();
        let gc = self.gc(depth, pool[0])?;
        let mut batches = Samples::new();
        let t0 = Instant::now();
        let mut done = 0;
        while done < count {
            let t = Instant::now();
            for i in 0..32 {
                let p = pool[(done + i) % pool.len()];
                self.conn.put_image(ImageFormat::Z_PIXMAP, p, gc, size, size, 0, 0, 0, depth, &pixels[i % 4]).map_err(e)?;
            }
            self.conn.get_image(ImageFormat::Z_PIXMAP, pool[0], 0, 0, 1, 1, !0).map_err(e)?.reply().map_err(e)?;
            batches.push(t.elapsed().as_secs_f64() * 1e6 / 32.0);
            done += 32;
        }
        let secs = t0.elapsed().as_secs_f64();
        let bytes = done * size as usize * size as usize * fmt.bytes_per_pixel();
        Ok(json!({
            "covers_per_s": (done as f64 / secs).round(),
            "mb_per_s": ((bytes as f64 / secs / 1e6) * 10.0).round() / 10.0,
            "per_cover": batches.json(),
        }))
    }

    /// Video memory: keep allocating `size` covers of `fmt` in batches, composite every one of them
    /// once per pass, and time the pass. While the pixmaps fit in EXA's offscreen area a pass costs
    /// the same per cover; once they do not, EXA migrates pixmaps between system and video memory
    /// and the per-cover time jumps. The knee is how many covers can stay resident.
    pub fn vram(&mut self, fmt: CoverFmt, size: u16, max_mb: f64, batch: usize) -> R<Value> {
        if !self.supports(fmt) {
            return Err(format!("{} not supported", fmt.name()));
        }
        let per = size as usize * size as usize * fmt.bytes_per_pixel();
        let max = ((max_mb * 1e6) as usize / per).max(batch);
        let pixels: Vec<Vec<u8>> = (0..8).map(|s| scene::cover_pixels(fmt, size, s)).collect();
        let mut held: Vec<Cover> = Vec::new();
        let mut curve = Vec::new();
        let mut baseline: Option<f64> = None;
        let mut knee = None;
        let op = if fmt == CoverFmt::Argb { PictOp::OVER } else { PictOp::SRC };
        while held.len() < max {
            for i in 0..batch {
                let c = self.new_cover(fmt, size, &pixels[i % pixels.len()])?;
                held.push(c);
            }
            // warm pass (first use migrates), then two timed passes
            let mut best = f64::INFINITY;
            for pass in 0..3 {
                let t = Instant::now();
                for (i, c) in held.iter().enumerate() {
                    let x = ((i * 7) % (SCREEN_WIDTH as usize - size as usize)) as i16;
                    let y = ((i * 13) % (SCREEN_HEIGHT as usize - size as usize)) as i16;
                    self.conn
                        .render_composite(op, c.pict, x11rb::NONE, self.back_pict, 0, 0, 0, 0, x, y, size, size)
                        .map_err(e)?;
                }
                self.sync_on(self.back)?;
                if pass > 0 {
                    best = best.min(t.elapsed().as_secs_f64() * 1e6 / held.len() as f64);
                }
            }
            let mb = (held.len() * per) as f64 / 1e6;
            curve.push(
                json!({ "covers": held.len(), "mb": (mb * 10.0).round() / 10.0, "us_per_cover": (best * 100.0).round() / 100.0 }),
            );
            if curve.len() == 3 {
                baseline = Some(best);
            }
            if let (Some(b), true) = (baseline, knee.is_none()) {
                if best > 3.0 * b {
                    knee = Some(json!({ "covers": held.len(), "mb": (mb * 10.0).round() / 10.0 }));
                }
            }
            // a knee confirmed by two more batches is enough: past it each pass gets slow
            if knee.is_some()
                && curve.len() > 3
                && held.len() >= knee.as_ref().and_then(|k| k["covers"].as_u64()).unwrap_or(0) as usize + 2 * batch
            {
                break;
            }
        }
        for c in held.drain(..) {
            self.free_cover(c)?;
        }
        self.sync()?;
        Ok(json!({ "bytes_per_cover": per, "knee": knee, "curve": curve }))
    }

    fn sync_on(&self, d: Drawable) -> R<()> {
        self.conn.get_image(ImageFormat::Z_PIXMAP, d, 0, 0, 1, 1, !0).map_err(e)?.reply().map_err(e)?;
        Ok(())
    }
}

impl Backend for XrBackend {
    fn name(&self) -> String {
        "xrender".into()
    }

    fn info(&self) -> Value {
        self.info.clone()
    }

    fn supports(&self, fmt: CoverFmt) -> bool {
        self.formats.contains_key(&fmt)
    }

    fn prepare(&mut self, fmt: CoverFmt, size: u16) -> R<()> {
        if self.covers.contains_key(&(fmt, size)) {
            return Ok(());
        }
        let mut pool = Vec::with_capacity(POOL);
        for slot in 0..POOL {
            let px = scene::cover_pixels(fmt, size, slot);
            pool.push(self.new_cover(fmt, size, &px)?);
        }
        self.covers.insert((fmt, size), pool);
        // Composite each once so first-use migration is not in the timings.
        let picts: Vec<Picture> = self.covers[&(fmt, size)].iter().map(|c| c.pict).collect();
        for p in picts {
            self.conn.render_composite(PictOp::OVER, p, x11rb::NONE, self.back_pict, 0, 0, 0, 0, 0, 0, size, size).map_err(e)?;
        }
        self.sync()
    }

    fn draw(&mut self, frame: &Frame) -> R<()> {
        if let Some((r, c)) = frame.clear {
            let rect = Rectangle { x: r.x as i16, y: r.y as i16, width: r.w as u16, height: r.h as u16 };
            self.conn.render_fill_rectangles(PictOp::SRC, self.back_pict, xcolor(c), &[rect]).map_err(e)?;
        }
        for op in &frame.ops {
            match op {
                Op::Fill { rect, color } => {
                    let r = Rectangle { x: rect.x as i16, y: rect.y as i16, width: rect.w as u16, height: rect.h as u16 };
                    let pop = if color.a == 255 { PictOp::SRC } else { PictOp::OVER };
                    self.conn.render_fill_rectangles(pop, self.back_pict, xcolor(*color), &[r]).map_err(e)?;
                }
                Op::Cover { fmt, size, slot, x, y, alpha } => {
                    let pict = self
                        .covers
                        .get(&(*fmt, *size))
                        .ok_or_else(|| format!("cover pool {} {} not prepared", fmt.name(), size))?[*slot % POOL]
                        .pict;
                    // An opaque cover at full alpha is a plain copy; anything else blends.
                    let (pop, mask) = match (fmt, alpha) {
                        (CoverFmt::Argb, 255) => (PictOp::OVER, x11rb::NONE),
                        (_, 255) => (PictOp::SRC, x11rb::NONE),
                        (_, a) => (PictOp::OVER, self.mask(*a)?),
                    };
                    self.conn
                        .render_composite(pop, pict, mask, self.back_pict, 0, 0, 0, 0, *x as i16, *y as i16, *size, *size)
                        .map_err(e)?;
                }
                Op::Text { x, y, size, color, text } => self.text(*x, *y, *size, *color, text)?,
            }
        }
        if let Some(r) = frame.present {
            self.conn
                .render_composite(
                    PictOp::SRC,
                    self.back_pict,
                    x11rb::NONE,
                    self.win_pict,
                    r.x as i16,
                    r.y as i16,
                    0,
                    0,
                    r.x as i16,
                    r.y as i16,
                    r.w as u16,
                    r.h as u16,
                )
                .map_err(e)?;
            self.presented = true;
        }
        Ok(())
    }

    fn as_xr(&mut self) -> Option<&mut XrBackend> {
        Some(self)
    }

    fn sync(&mut self) -> R<()> {
        let d = if std::mem::take(&mut self.presented) { self.window } else { self.back };
        self.sync_on(d)
    }
}

impl XrBackend {
    pub fn root_depth(&self) -> u8 {
        self.root_depth
    }
}
