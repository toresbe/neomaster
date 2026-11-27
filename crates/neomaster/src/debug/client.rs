//! `neomaster debug ...`: tail, filter, record and inject over the debug connection.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde_json::{json, Value};

pub const USAGE: &str = "usage: neomaster debug [--socket PATH | --tcp HOST[:PORT]] COMMAND

Watching:
  tail [--kinds K,K..] [--all] [--json] [--grep TEXT]
                    print the stream (default: everything but raw USB; --all includes it)
  record FILE [--kinds K,K..]
                    write the stream (all of it, unless --kinds) to a JSON Lines file
  state             print the core's state once

Driving (refused over TCP unless neomaster runs with --debug-tcp-inject):
  wheel N           turn the menu wheel N counts (15 per list row; negative = up)
  volume N          turn the volume wheel N counts
  pointer P         move the pointer arc to P (0-127)
  press B | release B
                    a panel button down / up (go, left, right)
  click B [MS]      press and release after MS ms (default 100)
  hold B [MS]       press, wait MS ms (default 1500), release
  beo4 KEY [--held] a Beo4 key from the IR eye: a name (n.music, standby, go, vol+, 1, ...) or code
  telegram in|out HEX..
                    a Masterlink telegram as if received, or sent on the link (with or without
                    checksum / EOT; MLGW logs paste as is)
  pc2 HEX..         a raw PC2 message (no USB framing)
  standby | all-standby
  mark TEXT..       put a note in the stream and in recordings
  replay FILE [--speed X] [--telegrams]
                    feed a recording's panel reports and IR keys back in, in time
                    (--telegrams: its incoming telegrams too)

Offline:
  vectors FILE NAME [--from S] [--to S]
                    print a recording's telegrams as a capture-replay vector (&[Row]) for
                    crates/masterlink/tests/vectors

Records are JSON Lines: {\"t\": seconds, \"kind\": ..}. Kinds: hello, log, panel_in, panel_input,
panel_out, ui_effect, app_effect, ir, ml_in, ml_out, pc2_in, pc2_out, pc2_refused, pc2_session,
usb_in, usb_out, core_event, inject, state, timing, mark, lagged, ok, error.
The socket defaults to $NEOMASTER_DEBUG_SOCKET, $XDG_RUNTIME_DIR/neomaster-debug.sock or
/tmp/neomaster-debug.sock; the TCP port to 7155.
";

enum Target {
    Unix(PathBuf),
    Tcp(String),
}

struct Conn {
    reader: BufReader<Box<dyn std::io::Read>>,
    writer: Box<dyn Write>,
}

impl Conn {
    fn open(target: &Target) -> Result<Conn, String> {
        match target {
            Target::Unix(p) => {
                let s =
                    UnixStream::connect(p).map_err(|e| format!("{}: {e} (is neomaster running with --debug?)", p.display()))?;
                let r = s.try_clone().map_err(|e| e.to_string())?;
                Ok(Conn { reader: BufReader::new(Box::new(r)), writer: Box::new(s) })
            }
            Target::Tcp(a) => {
                let addr = if a.contains(':') { a.clone() } else { format!("{a}:{}", super::DEFAULT_TCP_PORT) };
                let sa =
                    addr.to_socket_addrs().map_err(|e| format!("{addr}: {e}"))?.next().ok_or(format!("{addr}: no address"))?;
                let s = TcpStream::connect_timeout(&sa, Duration::from_secs(5)).map_err(|e| format!("{addr}: {e}"))?;
                let _ = s.set_nodelay(true);
                let r = s.try_clone().map_err(|e| e.to_string())?;
                Ok(Conn { reader: BufReader::new(Box::new(r)), writer: Box::new(s) })
            }
        }
    }

    fn next(&mut self) -> Result<Option<(String, Value)>, String> {
        let mut line = String::new();
        if self.reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(None);
        }
        let v = serde_json::from_str(line.trim_end()).map_err(|e| format!("bad record {line:?}: {e}"))?;
        Ok(Some((line.trim_end().to_string(), v)))
    }

    fn send(&mut self, cmd: &Value) -> Result<(), String> {
        writeln!(self.writer, "{cmd}").and_then(|_| self.writer.flush()).map_err(|e| e.to_string())
    }

    /// Send a command and wait for its answer, skipping stream records.
    fn request(&mut self, cmd: Value) -> Result<(), String> {
        self.send(&cmd)?;
        loop {
            match self.next()? {
                None => return Err("neomaster closed the connection".into()),
                Some((_, v)) if v["kind"] == "ok" => return Ok(()),
                Some((_, v)) if v["kind"] == "error" => return Err(v["error"].as_str().unwrap_or("error").to_string()),
                Some(_) => {}
            }
        }
    }

    /// Connect for commands only: no stream.
    fn quiet(target: &Target) -> Result<Conn, String> {
        let mut c = Conn::open(target)?;
        c.request(json!({ "cmd": "filter", "kinds": [] }))?;
        Ok(c)
    }
}

pub fn main(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => {
            eprint!("{USAGE}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("neomaster debug: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--flag VALUE` out of `args`.
fn take_opt(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    match args.iter().position(|a| a == flag) {
        None => Ok(None),
        Some(i) if i + 1 < args.len() => {
            let v = args.remove(i + 1);
            args.remove(i);
            Ok(Some(v))
        }
        Some(_) => Err(format!("{flag} needs a value")),
    }
}

fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    match args.iter().position(|a| a == flag) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

fn kinds(list: Option<String>) -> Option<Vec<String>> {
    list.map(|l| l.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect())
}

fn int(s: Option<&String>) -> Result<i64, String> {
    let s = s.ok_or("")?;
    s.parse().map_err(|_| format!("{s:?} is not a number"))
}

fn run(args: &[String]) -> Result<(), String> {
    let mut args = args.to_vec();
    let target = match (take_opt(&mut args, "--socket")?, take_opt(&mut args, "--tcp")?) {
        (Some(_), Some(_)) => return Err("--socket or --tcp, not both".into()),
        (Some(p), None) => Target::Unix(p.into()),
        (None, Some(a)) => Target::Tcp(a),
        (None, None) => Target::Unix(super::default_socket()),
    };
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print!("{USAGE}");
        return Ok(());
    }
    let cmd = args.remove(0);
    let button = |args: &[String]| -> Result<String, String> {
        let b = args.first().ok_or("")?;
        super::inject::button(b)?;
        Ok(b.to_ascii_lowercase())
    };
    match cmd.as_str() {
        "tail" => {
            let all = take_flag(&mut args, "--all");
            let raw = take_flag(&mut args, "--json");
            let grep = take_opt(&mut args, "--grep")?;
            let k = kinds(take_opt(&mut args, "--kinds")?).or(all.then(|| vec!["all".into()]));
            let mut c = Conn::open(&target)?;
            c.send(&json!({ "cmd": "filter", "kinds": k }))?;
            let out = std::io::stdout();
            while let Some((line, v)) = c.next()? {
                if v["kind"] == "ok" || grep.as_ref().is_some_and(|g| !line.contains(g.as_str())) {
                    continue;
                }
                let text = if raw { line } else { pretty(&v) };
                if writeln!(out.lock(), "{text}").is_err() {
                    return Ok(()); // e.g. piped into head
                }
            }
            Err("neomaster closed the connection".into())
        }
        "record" => {
            let k = kinds(take_opt(&mut args, "--kinds")?).unwrap_or(vec!["all".into()]);
            let path = args.first().ok_or("")?;
            let mut f = std::io::BufWriter::new(std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?);
            let mut c = Conn::open(&target)?;
            c.send(&json!({ "cmd": "filter", "kinds": k }))?;
            eprintln!("Recording to {path}; Ctrl-C to stop");
            let mut n = 0u64;
            while let Some((line, v)) = c.next()? {
                if v["kind"] == "ok" {
                    continue;
                }
                writeln!(f, "{line}").and_then(|_| f.flush()).map_err(|e| e.to_string())?;
                n += 1;
                if v["kind"] == "mark" {
                    eprintln!("{n} records; mark: {}", v["text"].as_str().unwrap_or(""));
                }
            }
            eprintln!("{n} records; neomaster closed the connection");
            Ok(())
        }
        "state" => {
            let mut c = Conn::open(&target)?;
            c.request(json!({ "cmd": "filter", "kinds": ["state"] }))?;
            c.send(&json!({ "cmd": "state" }))?;
            while let Some((_, v)) = c.next()? {
                if v["kind"] == "state" {
                    println!("{}", serde_json::to_string_pretty(&v["state"]).unwrap_or_default());
                    return Ok(());
                }
                if v["kind"] == "error" {
                    return Err(v["error"].as_str().unwrap_or("error").into());
                }
            }
            Err("neomaster closed the connection".into())
        }
        "wheel" => Conn::quiet(&target)?.request(json!({ "cmd": "panel", "menu": int(args.first())? })),
        "volume" => Conn::quiet(&target)?.request(json!({ "cmd": "panel", "volume": int(args.first())? })),
        "pointer" => Conn::quiet(&target)?.request(json!({ "cmd": "panel", "pointer": int(args.first())? })),
        "press" | "release" => {
            Conn::quiet(&target)?.request(json!({ "cmd": "button", "button": button(&args)?, "down": cmd == "press" }))
        }
        "click" | "hold" => {
            let b = button(&args)?;
            let ms = match args.get(1) {
                Some(_) => int(args.get(1))?.max(0) as u64,
                None if cmd == "click" => 100,
                None => 1500,
            };
            let mut c = Conn::quiet(&target)?;
            c.request(json!({ "cmd": "button", "button": b, "down": true }))?;
            std::thread::sleep(Duration::from_millis(ms));
            c.request(json!({ "cmd": "button", "button": b, "down": false }))
        }
        "beo4" => {
            let held = take_flag(&mut args, "--held");
            let key = args.first().ok_or("")?;
            if super::beo4_code(key).is_none() {
                return Err(format!("no Beo4 key {key:?}"));
            }
            Conn::quiet(&target)?.request(json!({ "cmd": "beo4", "code": key, "held": held }))
        }
        "telegram" => {
            let dir = args.first().filter(|d| *d == "in" || *d == "out").ok_or("")?.clone();
            let hex = args[1..].join(" ");
            super::inject::telegram(&hex)?;
            Conn::quiet(&target)?.request(json!({ "cmd": "telegram", "dir": dir, "hex": hex }))
        }
        "pc2" => Conn::quiet(&target)?.request(json!({ "cmd": "pc2", "hex": args.join(" ") })),
        "standby" => Conn::quiet(&target)?.request(json!({ "cmd": "standby" })),
        "all-standby" => Conn::quiet(&target)?.request(json!({ "cmd": "all_standby" })),
        "mark" => Conn::quiet(&target)?.request(json!({ "cmd": "mark", "text": args.join(" ") })),
        "replay" => {
            let speed: f64 = take_opt(&mut args, "--speed")?
                .map(|s| s.parse().map_err(|_| "--speed takes a number"))
                .transpose()?
                .unwrap_or(1.0);
            let telegrams = take_flag(&mut args, "--telegrams");
            let path = args.first().ok_or("")?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let cmds = replay_commands(&text, telegrams)?;
            eprintln!("Replaying {} inputs from {path}", cmds.len());
            let mut c = Conn::quiet(&target)?;
            let start = std::time::Instant::now();
            for (t, cmd) in cmds {
                let at = Duration::from_secs_f64((t / speed.max(0.01)).max(0.0));
                if let Some(wait) = at.checked_sub(start.elapsed()) {
                    std::thread::sleep(wait);
                }
                c.request(cmd)?;
            }
            Ok(())
        }
        "vectors" => {
            let from =
                take_opt(&mut args, "--from")?.map(|s| s.parse::<f64>().map_err(|_| "--from takes seconds")).transpose()?;
            let to = take_opt(&mut args, "--to")?.map(|s| s.parse::<f64>().map_err(|_| "--to takes seconds")).transpose()?;
            let (path, name) = (args.first().ok_or("")?, args.get(1).ok_or("")?);
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let rows = super::vectors::rows(&text, from, to)?;
            let source = std::path::Path::new(path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            print!("{}", super::vectors::render(&super::vectors::const_name(name), &rows, &source));
            eprintln!("{} telegrams", rows.len());
            Ok(())
        }
        _ => Err(String::new()),
    }
}

/// The inputs of a recording as commands, with their times from the first one.
pub fn replay_commands(jsonl: &str, telegrams: bool) -> Result<Vec<(f64, Value)>, String> {
    let mut out = vec![];
    for (n, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        let t = v["t"].as_f64().unwrap_or(0.0);
        let cmd = match v["kind"].as_str() {
            Some("panel_in") => json!({ "cmd": "panel_raw", "hex": v["raw"] }),
            Some("ir") => json!({ "cmd": "beo4", "code": format!("0x{}", v["code"].as_str().unwrap_or("")),
                                  "kind": format!("0x{}", v["key_type"].as_str().unwrap_or("")) }),
            Some("ml_in") if telegrams && v["raw"].is_string() => json!({ "cmd": "telegram", "dir": "in", "hex": v["raw"] }),
            _ => continue,
        };
        out.push((t, cmd));
    }
    if let Some(t0) = out.first().map(|(t, _)| *t) {
        for (t, _) in &mut out {
            *t -= t0;
        }
    }
    Ok(out)
}

/// One record as a line for people.
pub fn pretty(v: &Value) -> String {
    let t = v["t"].as_f64().unwrap_or(0.0);
    let kind = v["kind"].as_str().unwrap_or("?");
    let s = |k: &str| v[k].as_str().map(String::from).unwrap_or_else(|| v[k].to_string());
    let body = match kind {
        "ml_in" | "ml_out" | "pc2_refused" if v["from"].is_string() => {
            let mut b =
                format!("{} -> {} {} {} v{} [{}]", s("from"), s("to"), s("type"), s("payload_type"), v["subtype"], s("payload"));
            if v["src_src"] != "00" || v["dest_src"] != "00" {
                b += &format!(" src {} dest_src {}", s("src_src"), s("dest_src"));
            }
            if v["checksum_ok"] == false {
                b += " BAD CHECKSUM";
            }
            if v["reason"].is_string() {
                b += &format!(" ({})", s("reason"));
            }
            b
        }
        "panel_in" => format!("menu {} vol {} ptr {} btn {}  [{}]", v["menu"], v["volume"], v["pointer"], s("buttons"), s("raw")),
        "ir" => format!(
            "{} ({}) kind {}{}",
            v["key"].as_str().unwrap_or("?"),
            s("code"),
            s("key_type"),
            if v["held"] == true { " held" } else { "" }
        ),
        "log" => format!("{:<5} {}: {}", s("level"), s("target"), s("msg")),
        "timing" => format!(
            "{} n={} handle {} us, draw {} us{}",
            s("trigger"),
            v["events"],
            v["handle_us"],
            v["draw_us"],
            if v["redraw"] == true { "" } else { " (no redraw)" }
        ),
        "state" => match v["changed"].as_object() {
            Some(c) if !c.is_empty() => {
                c.iter().map(|(k, d)| format!("{k}: {} -> {}", d[0], d[1])).collect::<Vec<_>>().join(", ")
            }
            _ => v["state"].to_string(),
        },
        _ => {
            let mut rest = v.clone();
            if let Value::Object(m) = &mut rest {
                m.remove("t");
                m.remove("kind");
                if m.len() == 1 {
                    let (_, only) = m.iter().next().unwrap();
                    return format!("{t:>10.3} {kind:<11} {}", only.as_str().map(String::from).unwrap_or(only.to_string()));
                }
            }
            rest.to_string()
        }
    };
    format!("{t:>10.3} {kind:<11} {body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::{record, telegram_fields};

    #[test]
    fn pretty_lines() {
        let v = record(
            1.5,
            "ml_in",
            telegram_fields(&[0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, 0x9a, 0x00]),
        );
        assert_eq!(pretty(&v), "     1.500 ml_in       A_MASTER -> V_MASTER REQUEST LINK_INFO v4 [01 01 00]");
        let v = record(2.0, "ir", json!({ "key_type": "0f", "code": "0c", "key": "standby", "held": true }));
        assert_eq!(pretty(&v), "     2.000 ir          standby (0c) kind 0f held");
        let v = record(3.0, "state", json!({ "changed": { "playing": [null, "N.MUSIC"] }, "state": {} }));
        assert_eq!(pretty(&v), "     3.000 state       playing: null -> \"N.MUSIC\"");
        let v = record(4.0, "ui_effect", json!({ "effect": "Click" }));
        assert_eq!(pretty(&v), "     4.000 ui_effect   Click");
    }

    #[test]
    fn replays_take_inputs_only() {
        let jsonl = [
            record(5.0, "hello", json!({})).to_string(),
            record(10.0, "panel_in", json!({ "raw": "0f 00 28 00 c0 28" })).to_string(),
            record(10.25, "ir", json!({ "key_type": "01", "code": "94" })).to_string(),
            record(11.0, "ml_in", json!({ "raw": "c0 c1 01 0b 00 00 00 04 03 04 01 01 00 9a 00" })).to_string(),
            record(12.0, "ml_out", json!({ "raw": "c0 c1" })).to_string(),
        ]
        .join("\n");
        let cmds = replay_commands(&jsonl, false).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0], (0.0, json!({ "cmd": "panel_raw", "hex": "0f 00 28 00 c0 28" })));
        assert_eq!(cmds[1].0, 0.25);
        // the commands parse as the server would parse them
        for (_, c) in &cmds {
            let c: crate::debug::inject::Command = serde_json::from_value(c.clone()).unwrap();
            assert!(c.to_inject().unwrap().is_some());
        }
        assert_eq!(replay_commands(&jsonl, true).unwrap().len(), 3);
    }
}
