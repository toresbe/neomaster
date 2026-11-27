//! Masterlink "source center" behaviour: a node that only provides sources (address PC_1, 0xC2).
//!
//! Port of libpc2 `masterlink/source_center.{hpp,cpp}`. This is how a BeoMaster 5 behaves when
//! another product is the audio master (observed live on a system with a BeoSound 3000 as audio
//! master and a Beosystem 3 as video master; see reports/masterlink-protocol.md §6-§8). It never
//! arbitrates for the bus. The audio master orders it:
//!
//! ```text
//! REQUEST SOURCE_CONNECT, dest_src = source   -> start distributing that source
//! COMMAND STANDBY_0,      dest_src = source   -> stop it
//! COMMAND STANDBY_1,      dest_src = source   -> release it
//! COMMAND SOURCE_CONTROL, dest_src = source   -> a Beo4 key for it
//! ```
//!
//! Its own panel does not request sources over the link; it sends the room's master the Beo4 key a
//! remote would (COMMAND IR_SIMUL_LOW), see [`SourceCenter::send_remote_key`].

use std::time::Duration;

use log::{debug, warn};

use crate::host::Host;
use crate::protocol::{self as ml, node, payload_type as pt, telegram_type as tt};
use crate::telegram::Telegram;
use crate::Time;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Announce {
    Nothing,
    IdleLater,
    Stopped,
}

#[derive(Default)]
pub struct SourceCenter {
    source: u8,
    local_playback: bool,
    idle_status_pending: bool,
    idle_status_due: Time,
    /// [`SourceCenter::all_standby`] waits for the PC2's bus lock grant until this deadline.
    all_standby_due: Option<Time>,
}

impl SourceCenter {
    /// observed ~2 s
    pub const IDLE_STATUS_DELAY: Duration = Duration::from_millis(2000);
    /// How long [`SourceCenter::all_standby`] waits for the bus lock (as `AudioMaster`'s firmware
    /// lock mode).
    pub const LOCK_TIMEOUT: Duration = Duration::from_millis(2000);

    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::too_many_arguments)]
    fn make(dest: u8, ttype: u8, ptype: u8, subtype: u8, payload: Vec<u8>, src_src: u8, dest_src: u8, from: u8) -> Telegram {
        Telegram {
            dest_node: dest,
            src_node: from,
            telegram_type: ttype,
            dest_src,
            src_src,
            payload_type: ptype,
            subtype,
            payload,
        }
    }

    /// SOURCE_SYSTEM_STATUS to ALL_SOURCE_USERS (src_src = the source): active / stopped / idle
    /// forms, all byte for byte as captured from the BM5
    fn send_source_status(host: &mut dyn Host, src: u8, payload: Vec<u8>) {
        host.send_telegram(Self::make(
            node::ALL_SOURCE_USERS,
            tt::STATUS,
            pt::SOURCE_STATUS,
            ml::source_status::SUBTYPE_SYSTEM,
            payload,
            src,
            0,
            node::SOURCE_CENTER,
        ));
    }

    fn send_status_active(host: &mut dyn Host, src: u8) {
        Self::send_source_status(host, src, ml::source_status::system_status(src, ml::source_status::State::Playing));
    }

    fn send_status_stopped(host: &mut dyn Host, src: u8) {
        Self::send_source_status(host, src, ml::source_status::stopped(src));
    }

    fn send_status_idle(host: &mut dyn Host) {
        Self::send_source_status(
            host,
            ml::source::PC,
            ml::source_status::system_status(ml::source::PC, ml::source_status::State::Idle),
        );
    }

    /// Press a Beo4 key on another node, as the BM5 panel does towards the room's master (e.g.
    /// dest = V_MASTER, key 0x94 "N.MUSIC", 0x80 "TV", 0x64 volume down, 0x0c standby).
    /// Observed telegram: `dest C1 01 0A 00 47 00 20 05 02 | 00 01 ff ff <key>`, sent from the
    /// audio master's address C1 (as captured; see report §7). Pass `node::AUDIO_MASTER` as
    /// `from` to match.
    pub fn send_remote_key(&self, host: &mut dyn Host, dest: u8, key: u8, from: u8) {
        host.send_telegram(Self::make(
            dest,
            tt::COMMAND,
            pt::IR_SIMUL_LOW,
            ml::ir_simul::SUBTYPE,
            ml::ir_simul::key(key, ml::ir_simul::Destination::AudioSource),
            ml::source::PC,
            0,
            from,
        ));
    }

    /// Startup: broadcast our idle SOURCE_STATUS.
    pub fn start(&mut self, host: &mut dyn Host, _now: Time) {
        Self::send_status_idle(host);
    }

    /// Call periodically: the delayed idle status after a source is stopped, and the bus lock
    /// timeout of [`SourceCenter::all_standby`].
    pub fn tick(&mut self, host: &mut dyn Host, now: Time) {
        if self.idle_status_pending && now >= self.idle_status_due {
            self.idle_status_pending = false;
            Self::send_status_idle(host);
        }
        if self.all_standby_due.is_some_and(|due| now >= due) {
            warn!("Source center: no bus lock grant (IN 44) from the PC2; sending all standby anyway");
            self.send_all_standby(host, now);
        }
    }

    /// When [`SourceCenter::tick`] next has something to do, if ever.
    pub fn next_deadline(&self) -> Option<Time> {
        let idle = self.idle_status_pending.then_some(self.idle_status_due);
        [idle, self.all_standby_due].into_iter().flatten().min()
    }

    /// Long standby from this room (the panel's or a remote's "all off"): switch every node off.
    ///
    /// Inferred, not captured from a BM5 in this role: a link room does it as a lock round
    /// (LOCK_MANAGER REQUEST to ALL, which the PC2 firmware runs for us after host command 24)
    /// followed by `COMMAND STANDBY_1` subtype 1 to ALL with an empty payload (live sniffs: node
    /// 04 `80 04 01 0a 00 00 00 11 00 01`, the audio master `80 c1 ...` the same). We send it
    /// from our own address (PC_1), as every other telegram of ours, then stop our source as on
    /// a received "all standby": the stopped status, then idle.
    pub fn all_standby(&mut self, host: &mut dyn Host, now: Time) {
        if self.all_standby_due.is_some() {
            return;
        }
        self.all_standby_due = Some(now + Self::LOCK_TIMEOUT);
        host.request_bus_lock();
    }

    /// The PC2 reported IN 44 01: the firmware holds the bus lock for us.
    pub fn bus_lock_granted(&mut self, host: &mut dyn Host, now: Time) {
        if self.all_standby_due.is_some() {
            self.send_all_standby(host, now);
        }
    }

    fn send_all_standby(&mut self, host: &mut dyn Host, now: Time) {
        self.all_standby_due = None;
        host.send_telegram(Self::make(
            node::ALL,
            tt::COMMAND,
            pt::STANDBY_1,
            ml::standby::SUBTYPE_1_ALL,
            vec![],
            0,
            0,
            node::SOURCE_CENTER,
        ));
        self.local_playback = false;
        self.stop(host, Announce::Stopped, now);
        // B&O releases right after its telegrams (e4 01) rather than waiting out the firmware's hold
        host.release_bus_lock();
    }

    /// Something plays locally (e.g. picked on the panel). While set, stopping a link source does
    /// not announce the idle status: observed when the panel's N.RADIO was still active (§8), vs
    /// ~2 s later when nothing played locally (§6). Inferred rule.
    pub fn set_local_playback(&mut self, on: bool) {
        self.local_playback = on;
    }
    pub fn current_source(&self) -> u8 {
        self.source
    }
    pub fn distributing(&self) -> bool {
        self.source != 0
    }

    /// Observed (live captures): "all standby" (STANDBY_1 to ALL) -> the stopped SOURCE_STATUS,
    /// then idle; STANDBY_0 for our source -> idle SOURCE_STATUS ~2 s later (unless asked for
    /// another source meanwhile, or something plays locally); STANDBY_1 for one source -> nothing
    /// announced.
    fn stop(&mut self, host: &mut dyn Host, announce: Announce, now: Time) {
        if self.source == 0 {
            return;
        }
        if announce == Announce::Stopped {
            Self::send_status_stopped(host, self.source);
        }
        host.set_ml_distribution(false);
        host.stop_source();
        self.source = 0;
        if announce == Announce::IdleLater && !self.local_playback {
            self.idle_status_pending = true;
            self.idle_status_due = now + Self::IDLE_STATUS_DELAY;
        }
        if announce == Announce::Stopped {
            Self::send_status_idle(host); // observed ~200 ms after the stopped status
        }
    }

    pub fn handle(&mut self, host: &mut dyn Host, t: &Telegram, now: Time) {
        let to_us = t.dest_node == node::SOURCE_CENTER;

        // REQUEST SOURCE_CONNECT, dest_src = source: distribute it
        if t.telegram_type == tt::REQUEST && t.payload_type == pt::SOURCE_CONNECT && to_us {
            let src = t.dest_src;
            if src != self.source && !host.start_source(src) {
                warn!("Source center: host refused source {src:#04x}");
                return; // no answer observed for this case; stay silent
            }
            self.idle_status_pending = false;
            if self.source != 0 && self.source != src {
                host.stop_source();
            }
            self.source = src;
            let request = ml::source_connect::parse_request(&t.payload);
            if let Some(channel) = request.channel {
                host.source_channel(src, channel);
            }
            // As captured: STATUS SOURCE_CONNECT "connected" to the requester, our SOURCE_STATUS
            // to all link devices, PRI_SYSTEM_STATUS to the requester (which relays it to the room)
            host.send_telegram(Self::make(
                t.src_node,
                tt::STATUS,
                pt::SOURCE_CONNECT,
                ml::source_connect::SUBTYPE_STATUS,
                vec![ml::source_connect::CONNECTED],
                src,
                0,
                node::SOURCE_CENTER,
            ));
            host.set_ml_distribution(true);
            Self::send_status_active(host, src);
            host.send_telegram(Self::make(
                t.src_node,
                tt::STATUS,
                pt::PRI_SYSTEM_STATUS,
                ml::pri_system::SUBTYPE_AUDIO,
                ml::pri_system::playing(src),
                0,
                0,
                node::SOURCE_CENTER,
            ));
            return;
        }

        if t.telegram_type != tt::COMMAND {
            return; // only commands below
        }

        // STANDBY_1 to ALL (subtype_1_all, empty): everything off
        if t.payload_type == pt::STANDBY_1 && t.dest_node == node::ALL {
            self.stop(host, Announce::Stopped, now);
            host.link_standby();
            return;
        }
        if !to_us {
            return;
        }

        match t.payload_type {
            // stop this source (dest_src)
            pt::STANDBY_0 => {
                if t.dest_src == self.source || t.dest_src == 0 {
                    self.stop(host, Announce::IdleLater, now);
                }
            }
            // release this source (dest_src)
            pt::STANDBY_1 => {
                if t.dest_src == self.source || t.dest_src == 0 {
                    self.stop(host, Announce::Nothing, now);
                }
            }
            // "<source> <Beo4 key>"
            pt::SOURCE_CONTROL => {
                if t.payload.len() >= 2 {
                    host.source_key(t.payload[0], t.payload[1]);
                }
            }
            other => debug!("Source center: unhandled command, payload type {other:#04x}"),
        }
    }
}
