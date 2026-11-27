//! nm-player-bench: the Sendspin player (crates/player) on the C7, playing for real.
//!
//!     MA_TOKEN=... nm-player-bench --media URI [--media2 URI] [--seconds S] [--trials N]
//!         [--device ALSA] [--lead MS] [--client-id ID] [--json FILE] [--label NAME]
//!
//! MA_URL (default http://192.168.2.238:8095) and SENDSPIN_URL (default
//! ws://192.168.2.238:8927/sendspin) pick the server. The player registers as `--client-id`
//! (default neomaster-bm5, the id neomaster will use, so no stray players pile up in MA), plays
//! through ALSA `--device` (default: the system default), and MA is driven with `ma-client`:
//!
//! 1. connect: Sendspin connected, MA lists the player
//! 2. idle: CPU and memory while connected and silent
//! 3. latency: `--trials` x (play_media → stream start, next → stream restart), then stop
//! 4. steady: play `--seconds`, sampling process CPU, whole-system CPU and RSS every second,
//!    counting output errors and the player's warnings (underruns, rejected buffers)
//!
//! Stream start is when the player gets the first stream; sound follows `--lead` ms later. For
//! the scheduled-sound latency without ALSA, run the player crate's `latency_probe` example.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bringup::stats::Samples;
use bringup::{emit, process_cpu_seconds, rss_kb, system_jiffies};
use player::{Event, Player, PlayerConfig};
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedReceiver;

/// Passes records to env_logger and keeps count of warnings, with the first few kept.
struct CountingLogger {
    inner: env_logger::Logger,
    warnings: AtomicUsize,
    first: Mutex<Vec<String>>,
}

impl log::Log for CountingLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn || self.inner.enabled(m)
    }
    fn log(&self, r: &log::Record) {
        if r.level() <= log::Level::Warn {
            self.warnings.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut f) = self.first.lock() {
                if f.len() < 20 {
                    f.push(format!("{} {}: {}", r.level(), r.target(), r.args()));
                }
            }
        }
        if self.inner.matches(r) {
            self.inner.log(r);
        }
    }
    fn flush(&self) {}
}

fn logger() -> &'static CountingLogger {
    let inner = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    let max = inner.filter().max(log::LevelFilter::Warn);
    let l: &'static CountingLogger =
        Box::leak(Box::new(CountingLogger { inner, warnings: AtomicUsize::new(0), first: Mutex::new(Vec::new()) }));
    let _ = log::set_logger(l);
    log::set_max_level(max);
    l
}

struct Opts {
    media: String,
    media2: Option<String>,
    seconds: u64,
    trials: usize,
    device: Option<String>,
    lead: u32,
    client_id: String,
    json: Option<String>,
    label: String,
}

fn parse() -> Opts {
    let mut o = Opts {
        media: String::new(),
        media2: None,
        seconds: 60,
        trials: 3,
        device: None,
        lead: 200,
        client_id: "neomaster-bm5".into(),
        json: None,
        label: String::new(),
    };
    let mut it = std::env::args().skip(1);
    let usage = || -> ! {
        eprintln!("usage: nm-player-bench --media URI [--media2 URI] [--seconds S] [--trials N] [--device ALSA] [--lead MS] [--client-id ID] [--json FILE] [--label NAME]");
        std::process::exit(2)
    };
    while let Some(a) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--media" => o.media = v(),
            "--media2" => o.media2 = Some(v()),
            "--seconds" => o.seconds = v().parse().unwrap_or_else(|_| usage()),
            "--trials" => o.trials = v().parse().unwrap_or_else(|_| usage()),
            "--device" => o.device = Some(v()),
            "--lead" => o.lead = v().parse().unwrap_or_else(|_| usage()),
            "--client-id" => o.client_id = v(),
            "--json" => o.json = Some(v()),
            "--label" => o.label = v(),
            _ => usage(),
        }
    }
    if o.media.is_empty() {
        usage();
    }
    o
}

/// Wait for an event matching `want`, up to `timeout`; counts errors on the way.
async fn wait_for(
    rx: &mut UnboundedReceiver<Event>,
    timeout: Duration,
    errors: &mut Vec<String>,
    want: impl Fn(&Event) -> bool,
) -> Option<Duration> {
    let t0 = Instant::now();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(ev)) => {
                log::debug!("player: {ev:?}");
                match &ev {
                    Event::OutputError(e) => errors.push(format!("output: {e}")),
                    Event::Disconnected(e) => errors.push(format!("disconnected: {e}")),
                    _ => {}
                }
                if want(&ev) {
                    return Some(t0.elapsed());
                }
            }
            Ok(None) | Err(_) => return None,
        }
    }
}

fn ms(d: Option<Duration>) -> Value {
    d.map(|d| json!(d.as_millis() as u64)).unwrap_or(Value::Null)
}

/// Sample process CPU, system CPU and RSS once a second for `secs`, draining player events.
async fn sample(rx: &mut UnboundedReceiver<Event>, secs: u64, errors: &mut Vec<String>) -> Value {
    let (mut cpu, mut sys, mut rss) = (Samples::new(), Samples::new(), Samples::new());
    let mut ends = 0;
    let mut last_cpu = process_cpu_seconds();
    let mut last_sys = system_jiffies();
    let mut last_t = Instant::now();
    for _ in 0..secs {
        let next = tokio::time::Instant::now() + Duration::from_secs(1);
        while let Ok(Some(ev)) = tokio::time::timeout_at(next, rx.recv()).await {
            match ev {
                Event::OutputError(e) => errors.push(format!("output: {e}")),
                Event::Disconnected(e) => errors.push(format!("disconnected: {e}")),
                Event::StreamEnded => ends += 1,
                _ => {}
            }
        }
        let now = Instant::now();
        let dt = (now - last_t).as_secs_f64();
        last_t = now;
        if let (Some(a), Some(b)) = (last_cpu, process_cpu_seconds()) {
            cpu.push((b - a) / dt * 100.0);
            last_cpu = Some(b);
        }
        if let (Some((b0, t0)), Some((b1, t1))) = (last_sys, system_jiffies()) {
            if t1 > t0 {
                sys.push((b1 - b0) as f64 / (t1 - t0) as f64 * 100.0);
            }
            last_sys = Some((b1, t1));
        }
        if let Some(k) = rss_kb() {
            rss.push(k as f64);
        }
    }
    // Samples::json reports "_us" keys; these are percent and kB.
    let pct = |s: &Samples| json!({ "mean": (s.mean() * 10.0).round() / 10.0, "p95": (s.pct(95.0) * 10.0).round() / 10.0, "max": (s.max() * 10.0).round() / 10.0 });
    json!({ "seconds": secs, "process_cpu_pct": pct(&cpu), "system_busy_pct": pct(&sys),
            "rss_kb": { "mean": rss.mean().round(), "max": rss.max() }, "unexpected_stream_ends": ends })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let o = parse();
    let log = logger();
    let mut out = o.json.as_ref().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok());
    let token = std::env::var("MA_TOKEN").unwrap_or_else(|_| {
        eprintln!("nm-player-bench: MA_TOKEN is not set");
        std::process::exit(2)
    });
    let ma_url = std::env::var("MA_URL").unwrap_or("http://192.168.2.238:8095".into());
    let ss_url = std::env::var("SENDSPIN_URL").unwrap_or("ws://192.168.2.238:8927/sendspin".into());
    let mut errors = Vec::new();
    let mut result = json!({ "test": "player", "label": o.label, "client_id": o.client_id, "device": o.device,
                             "lead_ms": o.lead, "media": o.media, "sendspin": ss_url });

    // 1. connect
    let t0 = Instant::now();
    let mut cfg = PlayerConfig::new(ss_url, o.client_id.clone(), "BeoMaster 5 (neomaster)");
    cfg.device = o.device.clone();
    cfg.lead_ms = o.lead;
    let (_player, mut rx) = Player::spawn(cfg);
    let connected = wait_for(&mut rx, Duration::from_secs(20), &mut errors, |e| *e == Event::Connected).await;
    println!("sendspin connected: {:?}", connected);
    result["connect_ms"] = ms(connected);
    let (ma, _ma_events) = ma_client::Client::connect(ma_client::Config::new(ma_url, token));
    if tokio::time::timeout(Duration::from_secs(20), ma.wait_connected()).await.is_err() {
        result["error"] = json!("Music Assistant did not answer in 20 s");
        println!("{}", result["error"]);
        emit(&mut out, &result);
        std::process::exit(1);
    }
    let mut queue = None;
    for _ in 0..80 {
        if let Ok(Some(q)) = ma.player_for_output(&o.client_id).await {
            queue = Some(q);
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let Some(queue) = queue else {
        result["error"] = json!("MA does not list the Sendspin player");
        println!("{}", result["error"]);
        emit(&mut out, &result);
        std::process::exit(1);
    };
    result["ma_listed_ms"] = json!(t0.elapsed().as_millis() as u64);
    result["queue"] = json!(queue);
    println!("MA queue {queue} after {} ms", t0.elapsed().as_millis());

    // 2. idle
    result["idle"] = sample(&mut rx, 5, &mut errors).await;
    println!("idle: {}", result["idle"]);

    // 3. latency
    let mut uris = vec![o.media.as_str()];
    if let Some(m2) = &o.media2 {
        uris.push(m2);
    }
    let (mut starts, mut skips) = (Samples::new(), Samples::new());
    let mut trials = Vec::new();
    for i in 0..o.trials {
        let t = Instant::now();
        if let Err(e) = ma.play_media(&queue, &uris, ma_client::QueueOption::Play).await {
            errors.push(format!("play_media: {e}"));
        }
        let start = wait_for(&mut rx, Duration::from_secs(15), &mut errors, |e| matches!(e, Event::StreamStarted { .. }))
            .await
            .map(|_| t.elapsed());
        tokio::time::sleep(Duration::from_secs(4)).await;
        let t2 = Instant::now();
        if let Err(e) = ma.next(&queue).await {
            errors.push(format!("next: {e}"));
        }
        // A skip may arrive as a new stream (StreamStarted) or as a clear inside the same one,
        // which the player does not report; None means the latter, not a failure.
        let skip = wait_for(&mut rx, Duration::from_secs(5), &mut errors, |e| matches!(e, Event::StreamStarted { .. }))
            .await
            .map(|_| t2.elapsed());
        let _ = ma.stop(&queue).await;
        let _ = wait_for(&mut rx, Duration::from_secs(5), &mut errors, |e| *e == Event::StreamEnded).await;
        println!("trial {i}: play_media → stream {:?}, next → new stream {:?}", start, skip);
        if let Some(s) = start {
            starts.push(s.as_secs_f64() * 1e6);
        }
        if let Some(s) = skip {
            skips.push(s.as_secs_f64() * 1e6);
        }
        trials.push(json!({ "play_to_stream_ms": ms(start), "next_to_stream_ms": ms(skip) }));
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    result["trials"] = json!(trials);
    result["play_to_stream"] = starts.json();
    result["next_to_stream"] = skips.json();

    // 4. steady playback
    if let Err(e) = ma.play_media(&queue, &uris, ma_client::QueueOption::Play).await {
        errors.push(format!("play_media: {e}"));
    }
    let started = wait_for(&mut rx, Duration::from_secs(15), &mut errors, |e| matches!(e, Event::StreamStarted { .. })).await;
    if started.is_some() {
        let w0 = log.warnings.load(Ordering::Relaxed);
        result["steady"] = sample(&mut rx, o.seconds, &mut errors).await;
        result["steady"]["warnings"] = json!(log.warnings.load(Ordering::Relaxed) - w0);
        println!("steady: {}", result["steady"]);
    } else {
        errors.push("steady: no stream started".into());
    }
    let _ = ma.stop(&queue).await;
    let _ = wait_for(&mut rx, Duration::from_secs(5), &mut errors, |e| *e == Event::StreamEnded).await;

    result["errors"] = json!(errors);
    result["warnings_total"] = json!(log.warnings.load(Ordering::Relaxed));
    result["first_warnings"] = json!(log.first.lock().map(|f| f.clone()).unwrap_or_default());
    result["ok"] = json!(starts.len() == o.trials && started.is_some() && errors.is_empty());
    emit(&mut out, &result);
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
}
