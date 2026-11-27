//! Helpers shared by the capture replays (libpc2's FakeHost, incoming(), at() and diff()).
#![allow(dead_code)]

use masterlink::protocol::clock::LocalTime;
use masterlink::{Host, Telegram, Time};

/// One row of a captured conversation: `out` = sent by B&O's software, else received by it.
pub struct Row {
    pub t: f64,
    pub out: bool,
    pub bytes: &'static [u8],
}

pub type Bytes = Vec<u8>;

#[derive(Default)]
pub struct FakeHost {
    pub sent: Vec<Bytes>,
    /// Record telegrams without checksum / EOT, as the MLGW reports them.
    pub mlgw: bool,
    pub distributing: bool,
    pub listening: bool,
    pub standby_seen: bool,
    pub started: Vec<u8>,
    pub keys: Vec<u8>,
    pub stops: u32,
    pub lock_requests: u32,
    pub lock_releases: u32,
}

impl Host for FakeHost {
    fn send_telegram(&mut self, t: Telegram) {
        self.sent.push(if self.mlgw { t.encode_mlgw() } else { t.encode() });
    }
    fn set_ml_distribution(&mut self, on: bool) {
        self.distributing = on;
    }
    fn set_ml_listening(&mut self, on: bool) {
        self.listening = on;
    }
    fn start_source(&mut self, src: u8) -> bool {
        self.started.push(src);
        true
    }
    fn stop_source(&mut self) {
        self.stops += 1;
    }
    fn source_key(&mut self, _src: u8, key: u8) {
        self.keys.push(key);
    }
    fn link_standby(&mut self) {
        self.standby_seen = true;
    }
    fn request_bus_lock(&mut self) {
        self.lock_requests += 1;
    }
    fn release_bus_lock(&mut self) {
        self.lock_releases += 1;
    }
    /// 2018-12-25 14:42:28, the clock in the BM5asAM capture
    fn local_time(&mut self) -> LocalTime {
        LocalTime { hour: 14, minute: 42, second: 28, day: 25, month: 12, year: 2018 }
    }
}

pub fn incoming(ml: &[u8]) -> Telegram {
    Telegram::decode(ml).expect("valid telegram")
}

/// libpc2: `milliseconds((long long)(t * 1000))`, shifted by `offset_ms` so negative times work.
pub fn at_offset(t: f64, offset_ms: i64) -> Time {
    Time::from_millis(((t * 1000.0) as i64 + offset_ms) as u64)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x} ")).collect()
}

/// Multiset difference, as libpc2's diff(): "" when both sides hold the same telegrams.
pub fn diff(expected: &[Bytes], got: &[Bytes]) -> String {
    let mut e = expected.to_vec();
    let mut g = got.to_vec();
    e.sort();
    g.sort();
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < e.len() || j < g.len() {
        if j == g.len() || (i < e.len() && e[i] < g[j]) {
            out += &format!("\n    missing: {}", hex(&e[i]));
            i += 1;
        } else if i == e.len() || g[j] < e[i] {
            out += &format!("\n    extra:   {}", hex(&g[j]));
            j += 1;
        } else {
            i += 1;
            j += 1;
        }
    }
    out
}
