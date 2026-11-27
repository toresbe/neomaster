//! nm-xbench: what neomaster's panel UI will ask of the BM5's screen, measured.
//!
//!     nm-xbench [--backend xrender|sw-x11|sw-fbdev|sw-cpu] [--tests LIST] [--quick]
//!               [--seconds S] [--json FILE] [--label NAME] [--vram-max-mb MB]
//!
//! Tests (default: all that the backend can run):
//!   sync     the cost of one sync round trip (subtracted from the per-op figures)
//!   present  putting the strip and the whole screen on the glass
//!   covers   cover composites at every baked size, xrgb8888 / rgb565 / argb8888, 64 per batch
//!   fades    the same at 50 % alpha (the arc's fade towards its ends)
//!   fills    translucent fills: full-screen dims and highlight bars
//!   glyphs   a list view's text (about 250 glyphs in 16 runs)
//!   upload   PutImage of 96 px covers into pixmaps (xrender only)
//!   strip60  the cover arc scrolling, strip redraw, paced at 60 fps for --seconds
//!   full60   whole-screen redraws paced at 60 fps for --seconds
//!   ui       neomaster's own UI and XRender backend (crates/ui, crates/render), paced at 60 fps
//!   vram     how many 96 px covers stay resident before EXA starts migrating (xrender only)
//!
//! Every result is a JSON line in --json; a table goes to stdout.

use std::time::{Duration, Instant};

use bringup::scene::{self, CoverFmt, Frame};
use bringup::stats::Samples;
use bringup::sw::SwBackend;
use bringup::xr::XrBackend;
use bringup::{emit, process_cpu_seconds, system_jiffies, Backend};
use serde_json::{json, Value};

const ALL_TESTS: &[&str] =
    &["sync", "present", "covers", "fades", "fills", "glyphs", "upload", "strip60", "full60", "ui", "vram"];

struct Opts {
    backend: String,
    tests: Vec<String>,
    quick: bool,
    seconds: f64,
    json: Option<String>,
    label: String,
    vram_max_mb: f64,
    fmt: CoverFmt,
}

fn usage() -> ! {
    eprintln!(
        "{}",
        include_str!("nm-xbench.rs")
            .lines()
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.trim_start_matches("//! ").trim_start_matches("//!"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    std::process::exit(2)
}

fn parse() -> Opts {
    let mut o = Opts {
        backend: "xrender".into(),
        tests: ALL_TESTS.iter().map(|s| s.to_string()).collect(),
        quick: false,
        seconds: 10.0,
        json: None,
        label: String::new(),
        vram_max_mb: 96.0,
        fmt: CoverFmt::Rgb565,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--backend" => o.backend = val(),
            "--tests" => o.tests = val().split(',').map(str::to_string).collect(),
            "--quick" => o.quick = true,
            "--seconds" => o.seconds = val().parse().unwrap_or_else(|_| usage()),
            "--json" => o.json = Some(val()),
            "--label" => o.label = val(),
            "--vram-max-mb" => o.vram_max_mb = val().parse().unwrap_or_else(|_| usage()),
            "--format" => o.fmt = CoverFmt::parse(&val()).unwrap_or_else(|| usage()),
            _ => usage(),
        }
    }
    if o.quick {
        o.seconds = o.seconds.min(3.0);
    }
    o
}

struct Run {
    out: Option<std::fs::File>,
    label: String,
    backend: String,
    budget: Duration,
    sync_us: f64,
}

impl Run {
    fn record(&mut self, test: &str, params: Value, mut result: Value) {
        result["test"] = json!(test);
        result["params"] = params;
        result["backend"] = json!(self.backend);
        result["label"] = json!(self.label);
        emit(&mut self.out, &result);
    }
}

/// Draw + sync repeatedly for about `budget` (at least 5 times); µs per iteration.
fn measure(b: &mut dyn Backend, budget: Duration, mut make: impl FnMut(usize) -> Frame) -> Result<Samples, String> {
    let mut s = Samples::new();
    let t0 = Instant::now();
    let mut i = 0;
    while (t0.elapsed() < budget || i < 5) && i < 100_000 {
        let f = make(i);
        let t = Instant::now();
        b.draw(&f)?;
        b.sync()?;
        s.push(t.elapsed().as_secs_f64() * 1e6);
        i += 1;
    }
    Ok(s)
}

/// Frames paced at `fps` for `secs`: frame times, deadline misses, CPU.
fn paced(b: &mut dyn Backend, secs: f64, fps: f64, mut make: impl FnMut(usize) -> Frame) -> Result<Value, String> {
    let period = Duration::from_secs_f64(1.0 / fps);
    let mut s = Samples::new();
    let mut missed = 0;
    let cpu0 = process_cpu_seconds();
    let sys0 = system_jiffies();
    let t0 = Instant::now();
    let mut deadline = t0 + period;
    let mut i = 0;
    while t0.elapsed().as_secs_f64() < secs {
        let f = make(i);
        let t = Instant::now();
        b.draw(&f)?;
        b.sync()?;
        let done = Instant::now();
        s.push((done - t).as_secs_f64() * 1e6);
        if done > deadline {
            missed += 1;
            // drop the frames we are late for, as a real loop would
            while deadline < done {
                deadline += period;
            }
        } else {
            std::thread::sleep(deadline - done);
            deadline += period;
        }
        i += 1;
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let cpu = match (cpu0, process_cpu_seconds()) {
        (Some(a), Some(b)) => json!(((b - a) / elapsed * 1000.0).round() / 10.0),
        _ => Value::Null,
    };
    let sys = match (sys0, system_jiffies()) {
        (Some((b0, t0)), Some((b1, t1))) if t1 > t0 => json!((((b1 - b0) as f64 / (t1 - t0) as f64) * 1000.0).round() / 10.0),
        _ => Value::Null,
    };
    let mut v = s.json();
    v["frames"] = json!(i);
    v["missed"] = json!(missed);
    v["fps"] = json!(((i as f64 / elapsed) * 10.0).round() / 10.0);
    v["over_16_7ms"] = json!(s.over(16_667.0));
    v["client_cpu_pct"] = cpu;
    v["system_busy_pct"] = sys;
    Ok(v)
}

fn row(name: &str, s: &Samples, extra: &str) {
    println!("{name:<34} p50 {:>9.1} us  p95 {:>9.1} us  {extra}", s.pct(50.0), s.pct(95.0));
}

fn run_tests(o: &Opts, b: &mut dyn Backend, run: &mut Run) -> Result<(), String> {
    let has = |t: &str| o.tests.iter().any(|x| x == t);
    let budget = run.budget;
    let fmts: Vec<CoverFmt> = CoverFmt::ALL.into_iter().filter(|f| b.supports(*f)).collect();

    // Sync first: everything else is reported net of it.
    {
        let s = measure(b, budget, |_| Frame::default())?;
        run.sync_us = s.pct(50.0);
        if has("sync") {
            row("sync round trip", &s, "");
            run.record("sync", json!({}), s.json());
        }
    }
    let sync_us = run.sync_us;

    if has("present") {
        for (name, r) in [("strip", scene::STRIP), ("screen", scene::SCREEN)] {
            let s = measure(b, budget, |_| Frame { clear: None, ops: vec![], present: Some(r) })?;
            let mpix = (r.w * r.h) as f64 / (s.pct(50.0) - sync_us).max(1.0);
            row(&format!("present {name} {}x{}", r.w, r.h), &s, &format!("{mpix:.1} Mpix/s"));
            let mut v = s.json();
            v["mpix_per_s"] = json!((mpix * 10.0).round() / 10.0);
            run.record("present", json!({ "region": name, "w": r.w, "h": r.h }), v);
        }
    }

    for (test, alpha) in [("covers", 255u8), ("fades", 128u8)] {
        if !has(test) {
            continue;
        }
        let sizes: &[u16] = if test == "fades" { &[64, 96] } else { &scene::ALL_SIZES };
        for &fmt in &fmts {
            for &size in sizes {
                b.prepare(fmt, size)?;
                let n = 64;
                let s = measure(b, budget, |i| scene::cover_grid(n, fmt, size, alpha, i))?;
                let per = ((s.pct(50.0) - sync_us) / n as f64).max(0.0);
                let mpix = (size as f64 * size as f64) / per.max(0.001);
                row(&format!("{test} {} {size}px x{n}", fmt.name()), &s, &format!("{per:7.1} us/cover  {mpix:6.1} Mpix/s"));
                let mut v = s.json();
                v["us_per_cover"] = json!((per * 100.0).round() / 100.0);
                v["mpix_per_s"] = json!((mpix * 10.0).round() / 10.0);
                run.record(test, json!({ "format": fmt.name(), "size": size, "per_batch": n, "alpha": alpha }), v);
            }
        }
    }

    if has("fills") {
        for (name, full) in [("dim 1024x768 x4", true), ("bar 900x54 x32", false)] {
            let f = scene::fills(full);
            let s = measure(b, budget, |_| f.clone())?;
            let px: f64 = if full { 4.0 * 1024.0 * 768.0 } else { 32.0 * 900.0 * 54.0 };
            let mpix = px / (s.pct(50.0) - sync_us).max(1.0);
            row(&format!("fill {name}"), &s, &format!("{mpix:.1} Mpix/s"));
            let mut v = s.json();
            v["mpix_per_s"] = json!((mpix * 10.0).round() / 10.0);
            run.record("fills", json!({ "kind": name }), v);
        }
    }

    if has("glyphs") {
        // warm the glyph caches first, so uploads are not timed
        b.draw(&scene::glyphs(0.0))?;
        b.sync()?;
        let n = scene::glyph_count(&scene::glyphs(0.0));
        let s = measure(b, budget, |i| scene::glyphs((i % 8) as f32))?;
        let per = (s.pct(50.0) - sync_us).max(0.0) / n as f64;
        row(&format!("glyphs list view ({n} glyphs)"), &s, &format!("{per:.2} us/glyph"));
        let mut v = s.json();
        v["glyphs"] = json!(n);
        v["us_per_glyph"] = json!((per * 100.0).round() / 100.0);
        run.record("glyphs", json!({}), v);
    }

    if has("upload") {
        if let Some(x) = b.as_xr() {
            for &fmt in &fmts {
                let count = if o.quick { 256 } else { 1024 };
                let v = x.upload(fmt, 96, count)?;
                println!("{:<34} {} covers/s  {} MB/s", format!("upload {} 96px", fmt.name()), v["covers_per_s"], v["mb_per_s"]);
                run.record("upload", json!({ "format": fmt.name(), "size": 96 }), v);
            }
        }
    }

    for (test, full) in [("strip60", false), ("full60", true)] {
        if !has(test) {
            continue;
        }
        let fmt = if b.supports(o.fmt) { o.fmt } else { CoverFmt::Xrgb };
        for s in scene::ARC_SIZES {
            b.prepare(fmt, s)?;
        }
        // warm glyphs
        b.draw(&scene::full_frame(0.0, fmt))?;
        b.sync()?;
        // 2.5 covers a second: the speed of a brisk wheel turn
        let v = paced(b, o.seconds, 60.0, |i| {
            let scroll = i as f32 * (2.5 / 60.0);
            if full {
                scene::full_frame(scroll, fmt)
            } else {
                scene::strip_frame(scroll, fmt)
            }
        })?;
        println!(
            "{:<34} p50 {:>9.1} us  p99 {:>9.1} us  {} fps, missed {}/{}, cpu client {}% system {}%",
            format!("{test} {}", fmt.name()),
            v["p50_us"].as_f64().unwrap_or(f64::NAN),
            v["p99_us"].as_f64().unwrap_or(f64::NAN),
            v["fps"],
            v["missed"],
            v["frames"],
            v["client_cpu_pct"],
            v["system_busy_pct"]
        );
        run.record(test, json!({ "format": fmt.name(), "seconds": o.seconds, "target_fps": 60 }), v);
    }

    if has("vram") {
        if let Some(x) = b.as_xr() {
            for &fmt in &fmts {
                let v = x.vram(fmt, 96, o.vram_max_mb, if o.quick { 256 } else { 128 })?;
                let last = v["curve"].as_array().and_then(|c| c.last()).cloned().unwrap_or_default();
                println!(
                    "{:<34} knee {}  (tried up to {} covers / {} MB, last {} us/cover)",
                    format!("vram {} 96px", fmt.name()),
                    v["knee"],
                    last["covers"],
                    last["mb"],
                    last["us_per_cover"]
                );
                run.record("vram", json!({ "format": fmt.name(), "size": 96, "max_mb": o.vram_max_mb }), v);
            }
        }
    }
    Ok(())
}

/// neomaster's own UI (fake library) through its own XRender backend, paced at 60 fps, browsing a
/// list and the covers view.
fn ui_test(o: &Opts, run: &mut Run) -> Result<(), String> {
    use bs5_panel::Input;
    use render::Renderer;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};

    let mut r = render::xrender::XRenderer::open_window("nm-xbench ui", render::SCREEN_WIDTH, render::SCREEN_HEIGHT)
        .map_err(|e| e.to_string())?;
    let conn = r.connection();
    let win = r.window();
    for (view, pointer) in [("list", 60u8), ("covers", 82u8)] {
        let mut ui = ui::Ui::new(ui::fake::Library::generate());
        let epoch = Instant::now();
        ui.input(Input::Pointer(pointer), epoch.elapsed());
        let period = Duration::from_secs_f64(1.0 / 60.0);
        let (mut scene_s, mut render_s) = (Samples::new(), Samples::new());
        let mut missed = 0;
        let sys0 = system_jiffies();
        let t0 = Instant::now();
        let mut deadline = t0 + period;
        let mut i = 0usize;
        while t0.elapsed().as_secs_f64() < o.seconds {
            let now = epoch.elapsed();
            // a wheel step every 6 frames, reversing every 40 steps
            if i % 6 == 0 {
                let dir = if (i / 240) % 2 == 0 { 1 } else { -1 };
                ui.input(Input::MenuWheel(dir * bs5_panel::COUNTS_PER_STEP), now);
            }
            ui.tick(now);
            let t = Instant::now();
            let scene = ui.scene(now);
            let t1 = Instant::now();
            r.render(&scene).map_err(|e| e.to_string())?;
            conn.get_image(ImageFormat::Z_PIXMAP, win, 0, 0, 1, 1, !0)
                .map_err(|e| e.to_string())?
                .reply()
                .map_err(|e| e.to_string())?;
            let done = Instant::now();
            scene_s.push((t1 - t).as_secs_f64() * 1e6);
            render_s.push((done - t1).as_secs_f64() * 1e6);
            if done > deadline {
                missed += 1;
                while deadline < done {
                    deadline += period;
                }
            } else {
                std::thread::sleep(deadline - done);
                deadline += period;
            }
            i += 1;
        }
        let elapsed = t0.elapsed().as_secs_f64();
        let sys = match (sys0, system_jiffies()) {
            (Some((b0, t0)), Some((b1, t1))) if t1 > t0 => json!((((b1 - b0) as f64 / (t1 - t0) as f64) * 1000.0).round() / 10.0),
            _ => Value::Null,
        };
        row(&format!("ui {view}: scene build"), &scene_s, "");
        row(&format!("ui {view}: render + sync"), &render_s, &format!("missed {missed}/{i}, system {sys}%"));
        run.record(
            "ui",
            json!({ "view": view, "seconds": o.seconds }),
            json!({ "scene": scene_s.json(), "render": render_s.json(), "frames": i, "missed": missed,
                    "fps": ((i as f64 / elapsed) * 10.0).round() / 10.0, "system_busy_pct": sys }),
        );
    }
    let _ = conn.flush();
    Ok(())
}

fn main() {
    let o = parse();
    let out = o.json.as_ref().map(|p| {
        std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap_or_else(|e| {
            eprintln!("nm-xbench: {p}: {e}");
            std::process::exit(1)
        })
    });
    let mut run = Run {
        out,
        label: o.label.clone(),
        backend: o.backend.clone(),
        budget: Duration::from_millis(if o.quick { 250 } else { 800 }),
        sync_us: 0.0,
    };
    let result = (|| -> Result<(), String> {
        let mut backend: Box<dyn Backend> = match o.backend.as_str() {
            "xrender" => Box::new(XrBackend::open()?),
            "sw-x11" => Box::new(SwBackend::x11()?),
            "sw-fbdev" => Box::new(SwBackend::fbdev()?),
            "sw-cpu" => Box::new(SwBackend::cpu_only()),
            other => return Err(format!("unknown backend {other}")),
        };
        let b = backend.as_mut();
        let info = b.info();
        println!("== nm-xbench {} {}  {}", b.name(), o.label, info);
        run.record("info", json!({}), json!({ "info": info, "cpu": cpu_model() }));
        run_tests(&o, b, &mut run)?;
        if o.tests.iter().any(|t| t == "ui") && o.backend == "xrender" {
            ui_test(&o, &mut run)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("nm-xbench: {e}");
        emit(&mut run.out, &json!({ "test": "error", "backend": o.backend, "label": o.label, "error": e }));
        std::process::exit(1);
    }
}

fn cpu_model() -> Value {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines().find(|l| l.starts_with("model name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string())
        })
        .map(Value::from)
        .unwrap_or(Value::Null)
}
