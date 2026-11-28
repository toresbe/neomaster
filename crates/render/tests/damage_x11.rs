//! Drawing only the damage must leave the same pixels as drawing everything. Needs an X server,
//! so it is ignored by default; run it under Xvfb:
//!
//! ```sh
//! xvfb-run -s "-screen 0 1024x768x24" cargo test -p render --test damage_x11 -- --ignored
//! ```

#![cfg(feature = "xrender")]

use render::xrender::XRenderer;
use render::{Align, Blend, Color, Image, ImageDraw, Rect, Renderer, Scene, SCREEN_HEIGHT, SCREEN_WIDTH};
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

fn scene(step: u32, beam: &std::sync::Arc<Image>, tile: &std::sync::Arc<Image>) -> Scene {
    let mut s = Scene::new(Color::rgb(10, 10, 20));
    s.fill(Rect::new(0, 0, 1024, 40), Color::rgb(40, 40, 60));
    // A list that scrolls every fourth step, with a translucent highlight over it.
    let scroll = (step / 4) as i32 * 7;
    for i in 0..8 {
        s.text(40, 120 + i * 60 - scroll, 28, Color::rgb(200, 200, 200), Align::Left, format!("Row {i} of the list"));
    }
    s.fill(Rect::new(30, 150, 500, 50), Color::rgba(255, 255, 255, 60));
    // Labels on the right; the one nearest the beam is lit, and one comes and goes.
    for i in 0..7 {
        let lit = i == (step % 7) as i32;
        let c = if lit { Color::WHITE } else { Color::rgb(128, 128, 128) };
        s.text(900, 120 + i * 80, 24, c, Align::Right, format!("LABEL {i}"));
    }
    if step % 3 == 0 {
        s.text(512, 700, 32, Color::rgb(255, 200, 0), Align::Center, "now and then");
    }
    // The beam turns and changes tint; a tile moves beneath it.
    let angle = -0.6 + step as f32 * 0.09;
    s.image(tile, Rect::new(600 + (step as i32 * 13) % 300, 300, 120, 120), 0.8);
    s.draw(ImageDraw {
        image: beam.clone(),
        src: beam.rect(),
        dst: Rect::new(450, 334, 601, 101),
        opacity: 0.9,
        blend: Blend::Add,
        tint: Some(Color::rgb(0, (200 - step * 5).min(255) as u8, 255)),
        angle,
    });
    s
}

fn grab(r: &XRenderer) -> Vec<u8> {
    let conn = r.connection();
    conn.get_image(ImageFormat::Z_PIXMAP, r.window(), 0, 0, SCREEN_WIDTH, SCREEN_HEIGHT, !0).unwrap().reply().unwrap().data
}

#[test]
#[ignore = "needs an X server (run under xvfb-run)"]
fn damage_redraw_matches_full_redraw() {
    let mut r = XRenderer::open_window("damage test", SCREEN_WIDTH, SCREEN_HEIGHT).unwrap();
    let beam = Image::from_fn(601, 101, |x, y| {
        let a = ((1.0 - (y - 0.5).abs() * 2.0) * (1.0 - x)).clamp(0.0, 1.0);
        [255, 255, 255, (a * 255.0) as u8]
    });
    let tile = Image::from_fn(64, 64, |x, y| [(x * 255.0) as u8, (y * 255.0) as u8, 128, 255]);
    let mut partial_frames = 0;
    for step in 0..30 {
        let s = scene(step, &beam, &tile);
        r.render(&s).unwrap();
        r.sync().unwrap();
        if step == 0 {
            continue;
        }
        if r.damaged() < SCREEN_WIDTH as u64 * SCREEN_HEIGHT as u64 {
            partial_frames += 1;
        }
        let partial = grab(&r);
        r.invalidator().store(true, std::sync::atomic::Ordering::Relaxed);
        r.render(&s).unwrap();
        r.sync().unwrap();
        let full = grab(&r);
        let differ = partial.chunks(4).zip(full.chunks(4)).filter(|(a, b)| a[..3] != b[..3]).count();
        assert_eq!(differ, 0, "step {step}: {differ} pixels differ from a full redraw (last damage {} px)", r.damaged());
    }
    assert!(partial_frames > 20, "only {partial_frames} frames were partial");
}
