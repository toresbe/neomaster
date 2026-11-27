//! nm-panel-probe: the BeoSound 5 panel (USB 0cd4:1112) on its own, before neomaster has a
//! panel actor.
//!
//!     nm-panel-probe [--record FILE] [--summary FILE] [--unattended SECS] [--no-output]
//!
//! Takes the panel from the kernel's HID driver, then walks whoever stands at the BM5 through
//! every control (both wheels both ways, the pointer arc end to end, GO / LEFT / RIGHT, a long
//! GO) and checks what arrives, decoded by `bs5-panel` as neomaster decodes it. Then it blanks
//! and wakes the backlight and sends clicks, and asks whether they happened.
//!
//! --unattended SECS skips the prompts: it records whatever arrives for SECS seconds and checks
//! that the output reports are accepted.
//!
//! The recording uses the debug connection's record format (`hello`, `panel_in`, `panel_input`,
//! `panel_out`, `mark`), so `neomaster debug replay FILE` feeds it into a running neomaster.
//!
//! Afterwards the panel stays detached from usbhid until it is replugged or the BM5 reboots;
//! neomaster takes it the same way.

use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use bringup::emit;
use bringup::stats::Samples;
use bs5_panel::{Button, Decoder, Input, Output, Report};
use nusb::transfer::{ControlOut, ControlType, In, Interrupt, Recipient, TransferError};
use nusb::MaybeFuture;
use serde_json::{json, Value};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

struct Probe {
    iface: nusb::Interface,
    ep: nusb::Endpoint<Interrupt, In>,
    decoder: Decoder,
    output: Output,
    epoch: Instant,
    rec: Option<std::fs::File>,
    reports: usize,
    intervals: Samples,
    last_report: Option<Instant>,
    out_ok: usize,
    out_err: Vec<String>,
}

impl Probe {
    fn t(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn record(&mut self, kind: &str, mut fields: Value) {
        fields["t"] = json!((self.t() * 1e6).round() / 1e6);
        fields["kind"] = json!(kind);
        emit(&mut self.rec, &fields);
    }

    fn mark(&mut self, text: &str) {
        self.record("mark", json!({ "text": text }));
    }

    /// Wait up to `timeout` for reports; returns the inputs decoded.
    fn poll(&mut self, timeout: Duration) -> Vec<Input> {
        let mut out = Vec::new();
        if let Some(c) = self.ep.wait_next_complete(timeout) {
            let now = self.epoch.elapsed();
            match c.status {
                Ok(()) => {
                    let bytes = c.buffer[..c.actual_len].to_vec();
                    let t = Instant::now();
                    if let Some(last) = self.last_report.replace(t) {
                        self.intervals.push((t - last).as_secs_f64() * 1e6);
                    }
                    self.reports += 1;
                    let mut v = json!({ "raw": hex(&bytes) });
                    if let Some(r) = Report::parse(&bytes) {
                        v["menu"] = json!(r.menu);
                        v["volume"] = json!(r.volume);
                        v["pointer"] = json!(r.pointer);
                        v["buttons"] = json!(format!("{:02x}", r.buttons));
                        self.record("panel_in", v);
                        out.extend(self.decoder.feed(r, now));
                    } else {
                        self.record("panel_in", v);
                    }
                }
                Err(TransferError::Disconnected) => {
                    eprintln!("nm-panel-probe: the panel went away");
                    std::process::exit(1);
                }
                Err(e) => eprintln!("nm-panel-probe: IN transfer: {e}"),
            }
            let mut buf = c.buffer;
            buf.clear();
            buf.set_requested_len(self.ep.max_packet_size());
            self.ep.submit(buf);
        }
        out.extend(self.decoder.tick(self.epoch.elapsed()));
        for i in &out {
            self.record("panel_input", json!({ "input": format!("{i:?}") }));
        }
        out
    }

    /// SET_REPORT (output report 0) on interface 0, as BeoSound5.exe sends it.
    fn send(&mut self, report: [u8; 2]) {
        let r = self
            .iface
            .control_out(
                ControlOut {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: 0x09,
                    value: 0x0200,
                    index: 0,
                    data: &report,
                },
                Duration::from_millis(500),
            )
            .wait();
        self.record("panel_out", json!({ "raw": hex(&report) }));
        match r {
            Ok(()) => self.out_ok += 1,
            Err(e) => {
                eprintln!("nm-panel-probe: OUT report {}: {e}", hex(&report));
                self.out_err.push(format!("{}: {e}", hex(&report)));
            }
        }
    }
}

fn ask(q: &str) -> bool {
    eprint!("{q} [y/n] ");
    let _ = std::io::stderr().flush();
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s.trim().eq_ignore_ascii_case("y")
}

/// One guided step: prompt, then watch inputs until `done` says so or the time runs out.
fn step(p: &mut Probe, name: &str, prompt: &str, mut done: impl FnMut(&Input) -> bool) -> Value {
    eprintln!("\n>>> {prompt}  (30 s)");
    p.mark(name);
    let t0 = Instant::now();
    let mut seen = 0;
    while t0.elapsed() < Duration::from_secs(30) {
        for i in p.poll(Duration::from_millis(20)) {
            seen += 1;
            if done(&i) {
                let secs = t0.elapsed().as_secs_f64();
                eprintln!("    ok ({secs:.1} s)");
                return json!({ "step": name, "ok": true, "secs": (secs * 10.0).round() / 10.0, "inputs": seen });
            }
        }
    }
    eprintln!("    FAILED: not seen in 30 s ({seen} other inputs)");
    json!({ "step": name, "ok": false, "inputs": seen })
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let mut record = None;
    let mut summary = None;
    let mut unattended: Option<f64> = None;
    let mut output_test = true;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--record" => record = it.next(),
            "--summary" => summary = it.next(),
            "--unattended" => unattended = it.next().and_then(|s| s.parse().ok()),
            "--no-output" => output_test = false,
            _ => {
                eprintln!("usage: nm-panel-probe [--record FILE] [--summary FILE] [--unattended SECS] [--no-output]");
                std::process::exit(2);
            }
        }
    }

    let info = match nusb::list_devices()
        .wait()
        .ok()
        .and_then(|mut l| l.find(|d| d.vendor_id() == bs5_panel::VENDOR_ID && d.product_id() == bs5_panel::PRODUCT_ID))
    {
        Some(i) => i,
        None => {
            eprintln!("nm-panel-probe: no BeoSound 5 panel (USB 0cd4:1112) found");
            let mut out = summary.as_ref().and_then(|p| std::fs::File::create(p).ok());
            emit(&mut out, &json!({ "found": false }));
            std::process::exit(1);
        }
    };
    let dev_info = json!({
        "bus": info.bus_id(), "address": info.device_address(), "speed": format!("{:?}", info.speed()),
        "manufacturer": info.manufacturer_string(), "product": info.product_string(),
        "interfaces": info.interfaces().map(|i| json!({ "number": i.interface_number(), "class": i.class(), "subclass": i.subclass() })).collect::<Vec<_>>(),
    });
    eprintln!("Panel: {dev_info}");
    let device = info.open().wait().unwrap_or_else(|e| {
        eprintln!("nm-panel-probe: open: {e} (run as root, or add a udev rule)");
        std::process::exit(1)
    });
    // usbhid holds it on a stock kernel
    let iface = device.detach_and_claim_interface(0).wait().unwrap_or_else(|e| {
        eprintln!("nm-panel-probe: claim interface 0: {e}");
        std::process::exit(1)
    });
    let endpoints: Vec<Value> = iface
        .descriptors()
        .flat_map(|d| d.endpoints().map(|e| json!({ "address": format!("{:02x}", e.address()), "max_packet": e.max_packet_size(), "interval": e.interval() })).collect::<Vec<_>>())
        .collect();
    let mut ep = iface.endpoint::<Interrupt, In>(bs5_panel::ENDPOINT_IN).unwrap_or_else(|e| {
        eprintln!("nm-panel-probe: endpoint 81: {e}");
        std::process::exit(1)
    });
    let size = ep.max_packet_size();
    while ep.pending() < 4 {
        let b = ep.allocate(size);
        ep.submit(b);
    }
    let rec = record.as_ref().map(|p| {
        std::fs::File::create(p).unwrap_or_else(|e| {
            eprintln!("nm-panel-probe: {p}: {e}");
            std::process::exit(1)
        })
    });
    let mut p = Probe {
        iface,
        ep,
        decoder: Decoder::new(),
        output: Output::new(),
        epoch: Instant::now(),
        rec,
        reports: 0,
        intervals: Samples::new(),
        last_report: None,
        out_ok: 0,
        out_err: Vec::new(),
    };
    p.record("hello", json!({ "protocol": "neomaster-debug", "version": 1, "mode": "panel-probe", "role": null, "pc2_device": null, "inject": false, "panel": dev_info }));

    // Backlight on first: a panel left blanked by the stock software reports nothing visible.
    for r in p.output.wake() {
        p.send(r);
    }

    let mut steps = Vec::new();
    match unattended {
        Some(secs) => {
            eprintln!("Recording the panel for {secs} s; move anything you like.");
            p.mark("unattended");
            let t0 = Instant::now();
            let mut inputs = 0;
            while t0.elapsed().as_secs_f64() < secs {
                inputs += p.poll(Duration::from_millis(50)).len();
            }
            steps.push(json!({ "step": "unattended", "ok": true, "secs": secs, "inputs": inputs }));
        }
        None => {
            let mut acc = 0i32;
            steps.push(step(&mut p, "menu_cw", "Turn the MENU wheel (the big one) clockwise", |i| {
                if let Input::MenuWheel(n) = i {
                    acc += n;
                }
                acc >= 30
            }));
            acc = 0;
            steps.push(step(&mut p, "menu_ccw", "Turn the MENU wheel anticlockwise", |i| {
                if let Input::MenuWheel(n) = i {
                    acc += n;
                }
                acc <= -30
            }));
            acc = 0;
            steps.push(step(&mut p, "volume_cw", "Turn the VOLUME wheel clockwise", |i| {
                if let Input::VolumeWheel(n) = i {
                    acc += n;
                }
                acc >= 8
            }));
            acc = 0;
            steps.push(step(&mut p, "volume_ccw", "Turn the VOLUME wheel anticlockwise", |i| {
                if let Input::VolumeWheel(n) = i {
                    acc += n;
                }
                acc <= -8
            }));
            let (mut lo, mut hi) = (u8::MAX, 0u8);
            steps.push(step(&mut p, "pointer_sweep", "Move the pointer arc all the way to both ends", |i| {
                if let Input::Pointer(v) = i {
                    lo = lo.min(*v);
                    hi = hi.max(*v);
                }
                lo <= 5 && hi >= bs5_panel::POINTER_MAX - 5
            }));
            for (name, b, label) in [("go", Button::Go, "GO"), ("left", Button::Left, "LEFT"), ("right", Button::Right, "RIGHT")]
            {
                let mut pressed = false;
                steps.push(step(&mut p, name, &format!("Press and release {label}"), |i| {
                    match i {
                        Input::Pressed(x) if *x == b => pressed = true,
                        Input::Released(x) if *x == b && pressed => return true,
                        _ => {}
                    }
                    false
                }));
            }
            steps.push(step(&mut p, "go_long", "Hold GO for two seconds", |i| matches!(i, Input::LongPress(Button::Go))));
        }
    }

    if output_test {
        p.mark("output");
        let blank = p.output.blank();
        for r in blank {
            p.send(r);
        }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(1500) {
            p.poll(Duration::from_millis(20));
        }
        for r in p.output.wake() {
            p.send(r);
        }
        for k in 0..5 {
            let now = p.epoch.elapsed() + Duration::from_secs(k);
            if let Some(r) = p.output.click(now) {
                p.send(r);
            }
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(250) {
                p.poll(Duration::from_millis(20));
            }
        }
        if unattended.is_none() {
            let lit = ask("Did the backlight go off for a moment and come back?");
            let clicks = ask("Did you hear five clicks?");
            steps.push(json!({ "step": "backlight", "ok": lit }));
            steps.push(json!({ "step": "clicks", "ok": clicks }));
        }
    }

    let ok = steps.iter().all(|s| s["ok"] == true) && p.out_err.is_empty();
    let result = json!({
        "found": true,
        "device": dev_info,
        "endpoints": endpoints,
        "ok": ok,
        "reports": p.reports,
        "report_interval": p.intervals.json(),
        "out_reports_ok": p.out_ok,
        "out_errors": p.out_err,
        "steps": steps,
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default());
    let mut out = summary.as_ref().and_then(|p| std::fs::File::create(p).ok());
    emit(&mut out, &result);
    std::process::exit(if ok { 0 } else { 1 });
}
