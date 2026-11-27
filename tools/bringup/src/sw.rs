//! The framebuffer path: neomaster composites on the CPU into its own XRGB8888 buffer and only
//! copies finished regions out, either into an X window with PutImage (works under any X server,
//! accelerated or not) or straight into `/dev/fb0` (no X at all). With [`Present::None`] it
//! measures the CPU cost alone.
//!
//! The blending is plain integer code, as a first fallback backend would be; it is not tuned
//! with SIMD, so it is an upper bound on what a framebuffer backend costs.

use std::collections::HashMap;
use std::fs::OpenOptions;

use render::{Color, Font, GlyphKey, Rect, SCREEN_HEIGHT, SCREEN_WIDTH};
use serde_json::{json, Value};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{ConnectionExt as _, CreateGCAux, CreateWindowAux, Gcontext, ImageFormat, Window, WindowClass};
use x11rb::rust_connection::RustConnection;

use crate::scene::{self, CoverFmt, Frame, Op, POOL};
use crate::Backend;

type R<T> = Result<T, String>;

const W: usize = SCREEN_WIDTH as usize;
const H: usize = SCREEN_HEIGHT as usize;

pub enum Present {
    /// CPU only.
    None,
    /// PutImage into a window on `$DISPLAY`.
    X11 { conn: Box<RustConnection>, window: Window, gc: Gcontext, depth: u8, max_bytes: usize },
    /// Write into the mmapped framebuffer device.
    Fb { map: *mut u8, len: usize, stride: usize, bpp: usize, xres: usize, yres: usize },
}

struct GlyphBitmap {
    w: usize,
    h: usize,
    left: i32,
    top: i32,
    advance: i32,
    coverage: Vec<u8>,
}

pub struct SwBackend {
    buf: Vec<u32>,
    present: Present,
    covers: HashMap<(CoverFmt, u16), Vec<Vec<u8>>>,
    font: Font,
    glyphs: HashMap<GlyphKey, GlyphBitmap>,
    scratch: Vec<u8>,
    info: Value,
}

fn clip(x: i32, y: i32, w: usize, h: usize) -> Option<(usize, usize, usize, usize, usize, usize)> {
    // returns (dst_x, dst_y, src_x, src_y, w, h) inside the screen
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + w as i32).min(W as i32);
    let y1 = (y + h as i32).min(H as i32);
    if x1 <= x0 as i32 || y1 <= y0 as i32 {
        return None;
    }
    Some((x0, y0, (x0 as i32 - x) as usize, (y0 as i32 - y) as usize, x1 as usize - x0, y1 as usize - y0))
}

/// dst = src + dst * (255 - a) / 255, per channel; src premultiplied 0xAARRGGBB.
#[inline]
fn over(dst: u32, src: u32) -> u32 {
    let a = src >> 24;
    if a == 255 {
        return src;
    }
    if a == 0 {
        return dst;
    }
    let ia = 255 - a;
    let rb = ((dst & 0x00ff_00ff) * ia + 0x0080_0080) >> 8 & 0x00ff_00ff;
    let g = ((dst & 0x0000_ff00) * ia + 0x0000_8000) >> 8 & 0x0000_ff00;
    ((src & 0x00ff_ffff) + rb + g) | 0xff00_0000
}

/// Scale a premultiplied pixel by `a`/255.
#[inline]
fn scale(p: u32, a: u32) -> u32 {
    let rb = ((p & 0x00ff_00ff) * a + 0x0080_0080) >> 8 & 0x00ff_00ff;
    let ag = (((p >> 8) & 0x00ff_00ff) * a + 0x0080_0080) & 0xff00_ff00;
    rb | ag
}

#[inline]
fn from565(p: u16) -> u32 {
    let p = p as u32;
    let r = (p >> 11) & 0x1f;
    let g = (p >> 5) & 0x3f;
    let b = p & 0x1f;
    0xff00_0000 | ((r << 3 | r >> 2) << 16) | ((g << 2 | g >> 4) << 8) | (b << 3 | b >> 2)
}

#[inline]
fn to565(p: u32) -> u16 {
    (((p >> 8) & 0xf800) | ((p >> 5) & 0x07e0) | ((p >> 3) & 0x001f)) as u16
}

fn premul(c: Color) -> u32 {
    let a = c.a as u32;
    let pm = |v: u8| v as u32 * a / 255;
    (a << 24) | (pm(c.r) << 16) | (pm(c.g) << 8) | pm(c.b)
}

impl SwBackend {
    fn new(present: Present, info: Value) -> SwBackend {
        SwBackend {
            buf: vec![0xff00_0000; W * H],
            present,
            covers: HashMap::new(),
            font: Font::default_font(),
            glyphs: HashMap::new(),
            scratch: Vec::new(),
            info,
        }
    }

    pub fn cpu_only() -> SwBackend {
        SwBackend::new(Present::None, json!({ "present": "none" }))
    }

    /// Present into an override-redirect window on `$DISPLAY` with PutImage.
    pub fn x11() -> R<SwBackend> {
        let (conn, n) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let screen = conn.setup().roots[n].clone();
        let depth = screen.root_depth;
        let bpp = conn.setup().pixmap_formats.iter().find(|f| f.depth == depth).map(|f| f.bits_per_pixel).unwrap_or(0);
        if !matches!((depth, bpp), (24, 32) | (16, 16)) {
            return Err(format!("sw-x11 handles depth 24/32 bpp and 16/16 bpp, not {depth}/{bpp}"));
        }
        let window = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_window(
            depth,
            window,
            screen.root,
            0,
            0,
            SCREEN_WIDTH,
            SCREEN_HEIGHT,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new().background_pixel(screen.black_pixel).override_redirect(1),
        )
        .map_err(|e| e.to_string())?;
        conn.map_window(window).map_err(|e| e.to_string())?;
        let gc = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_gc(gc, window, &CreateGCAux::new()).map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        let max_bytes = conn.maximum_request_bytes();
        let info = json!({
            "present": "x11-putimage",
            "vendor": String::from_utf8_lossy(&conn.setup().vendor),
            "root_depth": depth,
            "max_request_bytes": max_bytes,
        });
        Ok(SwBackend::new(Present::X11 { conn: Box::new(conn), window, gc, depth, max_bytes }, info))
    }

    /// Present into `/dev/fb0` (or `$NM_FBDEV`), geometry from sysfs.
    pub fn fbdev() -> R<SwBackend> {
        let dev = std::env::var("NM_FBDEV").unwrap_or_else(|_| "/dev/fb0".into());
        let name = std::path::Path::new(&dev).file_name().and_then(|n| n.to_str()).unwrap_or("fb0").to_string();
        let sys = |f: &str| std::fs::read_to_string(format!("/sys/class/graphics/{name}/{f}")).map(|s| s.trim().to_string());
        let bpp: usize = sys("bits_per_pixel").map_err(|e| format!("{dev}: {e}"))?.parse().map_err(|_| "bits_per_pixel")?;
        let stride: usize = sys("stride").map_err(|e| format!("{dev}: {e}"))?.parse().map_err(|_| "stride")?;
        let vs = sys("virtual_size").map_err(|e| format!("{dev}: {e}"))?;
        let (xres, yres) = vs.split_once(',').ok_or("virtual_size")?;
        let (xres, yres): (usize, usize) = (xres.parse().map_err(|_| "xres")?, yres.parse().map_err(|_| "yres")?);
        if bpp != 32 && bpp != 16 {
            return Err(format!("{dev}: {bpp} bpp not handled"));
        }
        let f = OpenOptions::new().read(true).write(true).open(&dev).map_err(|e| format!("{dev}: {e}"))?;
        let len = stride * yres;
        use std::os::fd::AsRawFd;
        // SAFETY: mapping a framebuffer device we opened read-write; checked for MAP_FAILED.
        let map = unsafe {
            libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, f.as_raw_fd(), 0)
        };
        if map == libc::MAP_FAILED {
            return Err(format!("{dev}: mmap failed: {}", std::io::Error::last_os_error()));
        }
        let fbname = sys("name").unwrap_or_default();
        let info = json!({ "present": "fbdev", "device": dev, "driver": fbname, "bpp": bpp, "stride": stride, "virtual_size": [xres, yres] });
        Ok(SwBackend::new(Present::Fb { map: map as *mut u8, len, stride, bpp, xres, yres }, info))
    }

    fn fill(&mut self, r: Rect, c: Color, src_copy: bool) {
        let Some((x, y, _, _, w, h)) = clip(r.x, r.y, r.w as usize, r.h as usize) else { return };
        let p = premul(c);
        for row in y..y + h {
            let line = &mut self.buf[row * W + x..row * W + x + w];
            if src_copy || c.a == 255 {
                line.fill(p | 0xff00_0000);
            } else {
                for d in line {
                    *d = over(*d, p);
                }
            }
        }
    }

    fn cover(&mut self, fmt: CoverFmt, size: u16, slot: usize, x: i32, y: i32, alpha: u8) -> R<()> {
        let n = size as usize;
        let px = &self.covers.get(&(fmt, size)).ok_or_else(|| format!("cover pool {} {} not prepared", fmt.name(), size))?
            [slot % POOL];
        let Some((dx, dy, sx, sy, w, h)) = clip(x, y, n, n) else { return Ok(()) };
        let a = alpha as u32;
        for row in 0..h {
            let d = &mut self.buf[(dy + row) * W + dx..(dy + row) * W + dx + w];
            let s0 = (sy + row) * n + sx;
            match fmt {
                CoverFmt::Xrgb => {
                    let s = &px[s0 * 4..(s0 + w) * 4];
                    if a == 255 {
                        for (d, s) in d.iter_mut().zip(s.chunks_exact(4)) {
                            *d = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                        }
                    } else {
                        for (d, s) in d.iter_mut().zip(s.chunks_exact(4)) {
                            *d = over(*d, scale(u32::from_le_bytes([s[0], s[1], s[2], s[3]]), a));
                        }
                    }
                }
                CoverFmt::Rgb565 => {
                    let s = &px[s0 * 2..(s0 + w) * 2];
                    if a == 255 {
                        for (d, s) in d.iter_mut().zip(s.chunks_exact(2)) {
                            *d = from565(u16::from_le_bytes([s[0], s[1]]));
                        }
                    } else {
                        for (d, s) in d.iter_mut().zip(s.chunks_exact(2)) {
                            *d = over(*d, scale(from565(u16::from_le_bytes([s[0], s[1]])), a));
                        }
                    }
                }
                CoverFmt::Argb => {
                    let s = &px[s0 * 4..(s0 + w) * 4];
                    for (d, s) in d.iter_mut().zip(s.chunks_exact(4)) {
                        let p = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                        *d = over(*d, if a == 255 { p } else { scale(p, a) });
                    }
                }
            }
        }
        Ok(())
    }

    fn text(&mut self, x: i32, y: i32, size: u16, color: Color, text: &str) {
        if color.a == 0 {
            return;
        }
        let p = premul(color);
        let mut pen = x;
        for ch in text.chars() {
            let key = GlyphKey { ch, size };
            if !self.glyphs.contains_key(&key) {
                let g = self.font.rasterize(key);
                self.glyphs.insert(
                    key,
                    GlyphBitmap {
                        w: g.width as usize,
                        h: g.height as usize,
                        left: g.left as i32,
                        top: g.top as i32,
                        advance: g.advance as i32,
                        coverage: g.coverage,
                    },
                );
            }
            let g = &self.glyphs[&key];
            if let Some((dx, dy, sx, sy, w, h)) = clip(pen + g.left, y - g.top, g.w, g.h) {
                for row in 0..h {
                    let cov = &g.coverage[(sy + row) * g.w + sx..(sy + row) * g.w + sx + w];
                    let d = &mut self.buf[(dy + row) * W + dx..(dy + row) * W + dx + w];
                    for (d, &c) in d.iter_mut().zip(cov) {
                        if c != 0 {
                            *d = over(*d, scale(p, c as u32));
                        }
                    }
                }
            }
            pen += g.advance;
        }
    }

    fn present(&mut self, r: Rect) -> R<()> {
        let Some((x, y, _, _, w, h)) = clip(r.x, r.y, r.w as usize, r.h as usize) else { return Ok(()) };
        match &mut self.present {
            Present::None => Ok(()),
            Present::X11 { conn, window, gc, depth, max_bytes } => {
                let bpp = if *depth == 16 { 2 } else { 4 };
                let rows_per = ((*max_bytes - 64) / (w * bpp)).clamp(1, h);
                let mut row = y;
                while row < y + h {
                    let n = rows_per.min(y + h - row);
                    self.scratch.clear();
                    for rr in row..row + n {
                        let line = &self.buf[rr * W + x..rr * W + x + w];
                        if bpp == 2 {
                            for p in line {
                                self.scratch.extend_from_slice(&to565(*p).to_le_bytes());
                            }
                        } else {
                            for p in line {
                                self.scratch.extend_from_slice(&p.to_le_bytes());
                            }
                        }
                    }
                    conn.put_image(
                        ImageFormat::Z_PIXMAP,
                        *window,
                        *gc,
                        w as u16,
                        n as u16,
                        x as i16,
                        row as i16,
                        0,
                        *depth,
                        &self.scratch,
                    )
                    .map_err(|e| e.to_string())?;
                    row += n;
                }
                Ok(())
            }
            Present::Fb { map, stride, bpp, xres, yres, .. } => {
                let w = w.min(xres.saturating_sub(x));
                for rr in y..(y + h).min(*yres) {
                    let line = &self.buf[rr * W + x..rr * W + x + w];
                    // SAFETY: rr < yres and x + w <= xres, so the row lies inside the mapping.
                    unsafe {
                        let dst = map.add(rr * *stride + x * (*bpp / 8));
                        if *bpp == 32 {
                            std::ptr::copy_nonoverlapping(line.as_ptr() as *const u8, dst, w * 4);
                        } else {
                            let d = dst as *mut u16;
                            for (i, p) in line.iter().enumerate() {
                                d.add(i).write_unaligned(to565(*p));
                            }
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

impl Drop for SwBackend {
    fn drop(&mut self) {
        if let Present::Fb { map, len, .. } = self.present {
            // SAFETY: unmapping what fbdev() mapped, once.
            unsafe { libc::munmap(map as *mut libc::c_void, len) };
        }
    }
}

impl Backend for SwBackend {
    fn name(&self) -> String {
        match self.present {
            Present::None => "sw-cpu".into(),
            Present::X11 { .. } => "sw-x11".into(),
            Present::Fb { .. } => "sw-fbdev".into(),
        }
    }

    fn info(&self) -> Value {
        self.info.clone()
    }

    fn supports(&self, _: CoverFmt) -> bool {
        true
    }

    fn prepare(&mut self, fmt: CoverFmt, size: u16) -> R<()> {
        self.covers.entry((fmt, size)).or_insert_with(|| (0..POOL).map(|s| scene::cover_pixels(fmt, size, s)).collect());
        Ok(())
    }

    fn draw(&mut self, frame: &Frame) -> R<()> {
        if let Some((r, c)) = frame.clear {
            self.fill(r, c, true);
        }
        for op in &frame.ops {
            match op {
                Op::Fill { rect, color } => self.fill(*rect, *color, false),
                Op::Cover { fmt, size, slot, x, y, alpha } => self.cover(*fmt, *size, *slot, *x, *y, *alpha)?,
                Op::Text { x, y, size, color, text } => self.text(*x, *y, *size, *color, text),
            }
        }
        if let Some(r) = frame.present {
            self.present(r)?;
        }
        Ok(())
    }

    fn sync(&mut self) -> R<()> {
        if let Present::X11 { conn, window, .. } = &self.present {
            conn.get_image(ImageFormat::Z_PIXMAP, *window, 0, 0, 1, 1, !0)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blending() {
        assert_eq!(over(0xff10_2030, 0xff40_5060), 0xff40_5060);
        assert_eq!(over(0xff10_2030, 0x0000_0000), 0xff10_2030);
        // 50 % white over black
        let p = over(0xff00_0000, 0x8080_8080);
        assert_eq!(p & 0x00ff_ffff, 0x0080_8080);
        assert_eq!(scale(0xffff_ffff, 128) >> 24, 128);
        assert_eq!(from565(0xffff), 0xffff_ffff);
        assert_eq!(to565(0xffff_ffff), 0xffff);
    }

    #[test]
    fn a_cpu_frame_draws_something() {
        let mut b = SwBackend::cpu_only();
        b.prepare(CoverFmt::Rgb565, 96).unwrap();
        for s in scene::ARC_SIZES {
            b.prepare(CoverFmt::Rgb565, s).unwrap();
        }
        b.draw(&scene::full_frame(2.3, CoverFmt::Rgb565)).unwrap();
        assert!(b.buf.iter().any(|&p| p & 0x00ff_ffff > 0x0030_3030));
    }
}
