//! The debug connection: a live stream of what neomaster sees and does, for bringing it up on
//! the BM5, and a way to drive it from another machine.
//!
//! Off unless asked for (`--debug`, `--debug-tcp`, `--debug-record` or their `NEOMASTER_DEBUG*`
//! variables, see [`Config`]). When on:
//!
//! - **Stream.** The core, the PC2 actor and the logger emit records into a [`Tap`]: raw and
//!   decoded panel reports, every Masterlink telegram in and out (decoded with the `masterlink`
//!   crate), Beo4 keys, other PC2 messages, raw USB packets, UI and player effects, core state
//!   changes, timings and log lines. [`server`] serves them as JSON Lines on a Unix socket and,
//!   optionally, a TCP port.
//! - **Injection.** A client sends commands ([`inject::Command`]) to fake panel input, Beo4 keys
//!   from the IR eye, incoming telegrams, or to put telegrams and PC2 messages on the bus. They
//!   reach the core as [`crate::core::Event::Debug`], so they take the same path as the real
//!   thing. Over TCP injection is refused unless `--debug-tcp-inject` is given.
//! - **Recording.** `--debug-record FILE` (or `neomaster debug record FILE`) writes the stream to
//!   a JSON Lines file; `neomaster debug vectors FILE NAME` turns its telegrams into a `&[Row]`
//!   vector in the format of `crates/masterlink/tests/vectors`, so a session seen on the box can
//!   become a capture-replay test. `neomaster debug replay FILE` feeds its inputs back in.
//!
//! [`client`] is the `neomaster debug` command line client. The wire protocol is documented in
//! `doc/debug-connection.md`.
//!
//! When the tap is off, emitting costs one branch: records are built only when something
//! listens.

pub mod client;
pub mod inject;
pub mod logger;
pub mod record;
pub mod server;
pub mod vectors;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use serde_json::{json, Map, Value};
use tokio::sync::broadcast;

pub use inject::Inject;
pub use server::Config;

/// Bumped when a record or command changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 1;

/// Records a slow client may fall behind by before it is told it missed some.
const CAPACITY: usize = 4096;

/// One record, serialised once and shared by every listener.
#[derive(Clone, Debug)]
pub struct Line {
    pub kind: &'static str,
    pub json: Arc<str>,
}

struct TapInner {
    epoch: Instant,
    tx: broadcast::Sender<Line>,
}

/// Where records go. Cheap to clone; [`Tap::off`] drops everything without building it.
#[derive(Clone, Default)]
pub struct Tap(Option<Arc<TapInner>>);

static GLOBAL: OnceLock<Tap> = OnceLock::new();

impl Tap {
    pub fn off() -> Tap {
        Tap(None)
    }

    pub fn new() -> Tap {
        let (tx, _) = broadcast::channel(CAPACITY);
        Tap(Some(Arc::new(TapInner { epoch: Instant::now(), tx })))
    }

    /// Make this the tap that [`Tap::global`] returns (the PC2 actor, the logger, and cores
    /// built after this). Only the first call counts.
    pub fn install_global(&self) {
        let _ = GLOBAL.set(self.clone());
    }

    pub fn global() -> Tap {
        GLOBAL.get().cloned().unwrap_or_default()
    }

    /// Something is listening: a client or the recorder.
    pub fn listening(&self) -> bool {
        self.0.as_ref().is_some_and(|i| i.tx.receiver_count() > 0)
    }

    pub fn subscribe(&self) -> Option<broadcast::Receiver<Line>> {
        self.0.as_ref().map(|i| i.tx.subscribe())
    }

    /// Seconds since the tap was made: the `t` of every record.
    pub fn t(&self) -> f64 {
        self.0.as_ref().map(|i| i.epoch.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    /// Build a record (`t`, `kind`, then the fields `f` returns) and send it to every listener.
    pub fn emit(&self, kind: &'static str, f: impl FnOnce() -> Value) {
        if !self.listening() {
            return;
        }
        let line = self.line(kind, f());
        if let Some(i) = &self.0 {
            let _ = i.tx.send(line);
        }
    }

    /// A record as [`Tap::emit`] would send it, for one client only (replies, hello).
    pub fn line(&self, kind: &'static str, fields: Value) -> Line {
        Line { kind, json: record(self.t(), kind, fields).to_string().into() }
    }
}

/// `{"t": .., "kind": .., ...fields}`.
pub fn record(t: f64, kind: &str, fields: Value) -> Value {
    let mut m = Map::new();
    m.insert("t".into(), json!((t * 1e6).round() / 1e6));
    m.insert("kind".into(), json!(kind));
    if let Value::Object(o) = fields {
        m.extend(o);
    }
    Value::Object(m)
}

// ------------------------------------------------------------------ record contents

/// `c1 c2 01 14 ...`
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Bytes from hex, as people type them: `c1 c2 01`, `c1c201`, `0xc1,0xc2`, `c1:c2`.
pub fn parse_hex(s: &str) -> Result<Vec<u8>, String> {
    let mut out = vec![];
    for word in s.split(|c: char| c.is_whitespace() || c == ',' || c == ':' || c == '[' || c == ']') {
        let w = word.trim_start_matches("0x").trim_start_matches("0X");
        if w.is_empty() {
            continue;
        }
        if w.len() % 2 != 0 {
            return Err(format!("odd number of hex digits in {word:?}"));
        }
        for i in (0..w.len()).step_by(2) {
            out.push(u8::from_str_radix(&w[i..i + 2], 16).map_err(|_| format!("not hex: {word:?}"))?);
        }
    }
    Ok(out)
}

/// The fields of a telegram record: raw bytes, the MLGW form, and the decoded header.
pub fn telegram_fields(raw: &[u8]) -> Value {
    use masterlink::protocol::{node_name, payload_type_name, telegram_type_name};
    match masterlink::Telegram::decode(raw) {
        Ok(t) => {
            let mlgw = t.encode_mlgw();
            let checksum_ok = raw.len() < mlgw.len() + 1 || raw[mlgw.len()] == masterlink::telegram::checksum(&mlgw);
            json!({
                "from": node_name(t.src_node),
                "to": node_name(t.dest_node),
                "type": telegram_type_name(t.telegram_type).map(String::from).unwrap_or(format!("{:02x}", t.telegram_type)),
                "payload_type": payload_type_name(t.payload_type).map(String::from).unwrap_or(format!("{:02x}", t.payload_type)),
                "subtype": t.subtype,
                "src_src": format!("{:02x}", t.src_src),
                "dest_src": format!("{:02x}", t.dest_src),
                "payload": hex(&t.payload),
                "mlgw": hex(&mlgw),
                "raw": hex(raw),
                "checksum_ok": checksum_ok,
            })
        }
        Err(e) => json!({ "raw": hex(raw), "error": e.to_string() }),
    }
}

/// Beo4 key names for the CLI and the stream (`pc2::beo4`).
pub const BEO4_KEYS: &[(&str, u8)] = {
    use pc2::beo4::*;
    &[
        ("0", DIGIT_0),
        ("1", DIGIT_1),
        ("2", DIGIT_2),
        ("3", DIGIT_3),
        ("4", DIGIT_4),
        ("5", DIGIT_5),
        ("6", DIGIT_6),
        ("7", DIGIT_7),
        ("8", DIGIT_8),
        ("9", DIGIT_9),
        ("store", STORE),
        ("standby", STANDBY),
        ("mute", MUTE),
        ("up", ARROW_UP),
        ("down", ARROW_DOWN),
        ("left", ARROW_LEFT),
        ("right", ARROW_RIGHT),
        ("go", GO),
        ("stop", STOP),
        ("record", RECORD),
        ("sound", SOUND),
        ("picture", PICTURE),
        ("menu", MENU),
        ("vol+", VOL_UP),
        ("vol-", VOL_DOWN),
        ("tv", TV),
        ("radio", RADIO),
        ("v.aux", V_AUX),
        ("a.aux", A_AUX),
        ("v.tape", V_TAPE),
        ("sat2", SAT2),
        ("text", TEXT),
        ("sat", SAT),
        ("pc", PC),
        ("a.tape", A_TAPE),
        ("cd", CD),
        ("phono", PHONO),
        ("n.music", N_MUSIC),
        ("light", LIGHT),
        ("av", AV),
        ("yellow", YELLOW),
        ("green", GREEN),
        ("blue", BLUE),
        ("red", RED),
    ]
};

pub fn beo4_name(code: u8) -> Option<&'static str> {
    BEO4_KEYS.iter().find(|(_, c)| *c == code).map(|(n, _)| *n)
}

/// A Beo4 key by name (`n.music`, `standby`, ...) or number (`0x94`, `148`).
pub fn beo4_code(s: &str) -> Option<u8> {
    let l = s.to_ascii_lowercase();
    BEO4_KEYS.iter().find(|(n, _)| *n == l).map(|(_, c)| *c).or_else(|| parse_u8(&l))
}

/// `0x94`, `94h` or `148`.
pub fn parse_u8(s: &str) -> Option<u8> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_suffix('h')) {
        u8::from_str_radix(h, 16).ok()
    } else {
        s.parse().ok()
    }
}

/// The default socket: `$NEOMASTER_DEBUG_SOCKET`, else `$XDG_RUNTIME_DIR/neomaster-debug.sock`,
/// else `/tmp/neomaster-debug.sock`. The server and the client work it out the same way.
pub fn default_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("NEOMASTER_DEBUG_SOCKET") {
        return p.into();
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => PathBuf::from(d).join("neomaster-debug.sock"),
        None => PathBuf::from("/tmp/neomaster-debug.sock"),
    }
}

/// Default TCP port for `--debug-tcp` given without one.
pub const DEFAULT_TCP_PORT: u16 = 7155;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_in_the_forms_people_type() {
        assert_eq!(parse_hex("c1 c2 01").unwrap(), [0xc1, 0xc2, 0x01]);
        assert_eq!(parse_hex("c1c201").unwrap(), [0xc1, 0xc2, 0x01]);
        assert_eq!(parse_hex("0xc1, 0xc2,0x01").unwrap(), [0xc1, 0xc2, 0x01]);
        assert_eq!(parse_hex("[c1:c2:01]").unwrap(), [0xc1, 0xc2, 0x01]);
        assert!(parse_hex("c1 c").is_err());
        assert!(parse_hex("zz").is_err());
        assert_eq!(hex(&[0xc1, 0x02]), "c1 02");
    }

    #[test]
    fn telegram_records_name_the_header() {
        // REQUEST LINK_INFO to the video master, with checksum and EOT
        let v = telegram_fields(&[0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, 0x9a, 0x00]);
        assert_eq!(v["from"], "A_MASTER");
        assert_eq!(v["to"], "V_MASTER");
        assert_eq!(v["type"], "REQUEST");
        assert_eq!(v["payload_type"], "LINK_INFO");
        assert_eq!(v["mlgw"], "c0 c1 01 0b 00 00 00 04 03 04 01 01 00");
        assert_eq!(v["checksum_ok"], true);
        let bad = telegram_fields(&[0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, 0x9b, 0x00]);
        assert_eq!(bad["checksum_ok"], false);
        assert!(telegram_fields(&[1, 2, 3])["error"].is_string());
    }

    #[test]
    fn beo4_keys_by_name_or_number() {
        assert_eq!(beo4_code("N.MUSIC"), Some(0x94));
        assert_eq!(beo4_code("0x0c"), Some(0x0c));
        assert_eq!(beo4_code("12"), Some(12));
        assert_eq!(beo4_code("nope"), None);
        assert_eq!(beo4_name(0x0c), Some("standby"));
    }

    #[test]
    fn records_are_built_only_when_someone_listens() {
        let tap = Tap::new();
        let mut built = false;
        tap.emit("x", || {
            built = true;
            json!({})
        });
        assert!(!built);
        let mut rx = tap.subscribe().unwrap();
        tap.emit("mark", || json!({ "text": "hi" }));
        let line = rx.try_recv().unwrap();
        assert_eq!(line.kind, "mark");
        let v: Value = serde_json::from_str(&line.json).unwrap();
        assert_eq!(v["kind"], "mark");
        assert_eq!(v["text"], "hi");
        assert!(v["t"].is_number());
        // the off tap does nothing at all
        Tap::off().emit("x", || unreachable!());
    }
}
