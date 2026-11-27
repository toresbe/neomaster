//! The Masterlink loop end to end, without a device: the PC2 state machine (framing, assembly,
//! pacing), [`Link`] and the state machines, against a [`SimPc2`] that answers like the real one.
//! The capture replays feed the captured incoming telegrams in as USB packets (9 bytes at a time,
//! as the PC2 sends them) and compare the telegrams that reach the USB OUT endpoint with what B&O's
//! software sent.

use std::time::Duration;

use masterlink::protocol::clock::LocalTime;
use masterlink::protocol::node;
use masterlink::{BusState, LinkRole, LocalSource, LockMode, Telegram, TrackInfo};
use pc2::beo4;

use super::*;
use crate::sim_pc2::SimPc2;

/// One row of a captured conversation (as in `crates/masterlink/tests/common`).
pub struct Row {
    pub t: f64,
    pub out: bool,
    pub bytes: &'static [u8],
}

mod sc_vectors {
    use super::Row;
    include!("../../masterlink/tests/vectors/source_center_live.rs");
}
mod am_vectors {
    use super::Row;
    include!("../../masterlink/tests/vectors/audio_master_capture.rs");
}

type Bytes = Vec<u8>;

fn clock_2018() -> LocalTime {
    LocalTime { hour: 14, minute: 42, second: 28, day: 25, month: 12, year: 2018 }
}

struct Harness {
    pc2: pc2::Pc2,
    link: Link,
    sim: SimPc2,
    app: Vec<AppEffect>,
    now: Time,
    offset: f64,
}

impl Harness {
    fn new(cfg: Config) -> Harness {
        let mut h = Harness {
            pc2: pc2::Pc2::new(cfg.pc2_mode(), 0),
            link: Link::new(cfg),
            sim: SimPc2::new(),
            app: vec![],
            now: Time::ZERO,
            offset: 0.0,
        };
        // What the actor and the core do on connect
        h.pc2.init(h.now);
        let out = h.link.start(h.now);
        h.apply(out);
        h.pump();
        h
    }

    fn link_source() -> Harness {
        Harness::new(Config { clock: clock_2018, ..Config::new(LinkRole::SourceCenter) })
    }

    fn audio_master() -> Harness {
        Harness::new(Config { clock: clock_2018, ..Config::new(LinkRole::AudioMaster) })
    }

    fn apply(&mut self, out: Outputs) {
        for cmd in out.pc2 {
            match cmd {
                Pc2Cmd::Message(m) => self.pc2.send(m, self.now),
                Pc2Cmd::Telegram(t) => self.pc2.send_telegram(&t, self.now).expect("the PC2 would accept it"),
            }
        }
        self.app.extend(out.app);
    }

    /// Move packets both ways until nothing moves.
    fn pump(&mut self) {
        loop {
            let mut moved = false;
            while let Some(p) = self.pc2.poll_transmit() {
                self.sim.on_out(&p);
                moved = true;
            }
            while let Some(p) = self.sim.poll_in() {
                moved = true;
                for ev in self.pc2.on_usb_packet(&p, self.now) {
                    let out = self.link.pc2_event(ev, self.now);
                    self.apply(out);
                }
            }
            if !moved {
                return;
            }
        }
    }

    fn at(&mut self, t: f64) {
        let now = Duration::from_secs_f64(t + self.offset);
        if now > self.now {
            self.now = now;
        }
        self.pc2.tick(self.now);
        let out = self.link.tick(self.now);
        self.apply(out);
        self.pump();
    }

    fn incoming(&mut self, mlgw: &[u8]) {
        let t = Telegram::decode(mlgw).expect("valid telegram");
        self.sim.telegram(&t.encode());
        self.pump();
    }

    fn ir(&mut self, kind: u8, code: u8) {
        self.sim.ir(kind, code);
        self.pump();
    }

    fn act(&mut self, f: impl FnOnce(&mut Link, Time) -> Outputs) {
        let out = f(&mut self.link, self.now);
        self.apply(out);
        self.pump();
    }

    fn clear(&mut self) {
        self.sim.received.clear();
        self.app.clear();
    }

    /// Telegrams sent since the last clear, without the INFO track telegrams (the capture
    /// vectors were cut without them) and in the MLGW form (no checksum / EOT) if `mlgw`.
    fn sent(&self, mlgw: bool) -> Vec<Bytes> {
        self.sim
            .telegrams()
            .into_iter()
            .filter(|t| t[3] != masterlink::protocol::telegram_type::INFO)
            .map(|t| if mlgw { t[..t.len() - 2].to_vec() } else { t })
            .collect()
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x} ")).collect()
}

/// "" when both hold the same telegrams, in any order.
fn diff(expected: &[Bytes], got: &[Bytes]) -> String {
    let (mut e, mut g) = (expected.to_vec(), got.to_vec());
    e.sort();
    g.sort();
    let mut out = String::new();
    for x in &e {
        if let Some(i) = g.iter().position(|y| y == x) {
            g.remove(i);
        } else {
            out += &format!("\n    missing: {}", hex(x));
        }
    }
    for y in &g {
        out += &format!("\n    extra:   {}", hex(y));
    }
    out
}

type Action = (f64, Box<dyn FnMut(&mut Link, Time) -> Outputs>);

/// Feed a window, running `actions` (panel intents) at their capture times. Rows at the times in
/// `ours` are telegrams our own panel sent (the MLGW logged them like any other): they are
/// expected from us instead of fed in.
fn replay(h: &mut Harness, rows: &[Row], mut actions: Vec<Action>, ours: &[f64]) -> Vec<Bytes> {
    let mut expected = vec![];
    actions.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut actions = actions.into_iter().peekable();
    for row in rows {
        while actions.peek().is_some_and(|(t, _)| *t <= row.t) {
            let (t, mut f) = actions.next().unwrap();
            h.at(t);
            h.act(|l, now| f(l, now));
        }
        h.at(row.t);
        if row.out || ours.iter().any(|t| (t - row.t).abs() < 1e-6) {
            expected.push(row.bytes.to_vec());
        } else {
            h.incoming(row.bytes);
        }
    }
    for (t, mut f) in actions {
        h.at(t);
        h.act(|l, now| f(l, now));
    }
    let last = rows.last().unwrap().t;
    h.at(last + 10.0);
    expected
}

/// SOURCE_CONNECT for `src` from the audio master, as the captures carry it
fn source_connect(src: u8) -> Vec<u8> {
    vec![0xc2, 0xc1, 0x01, 0x0b, src, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02, 0x00]
}

// ---------------------------------------------------------------- start-up

#[test]
fn startup_as_link_source() {
    let h = Harness::link_source();
    // F1, ML START with the source center default address 0x29, F6 option for configuration 7
    assert_eq!(h.sim.commands()[..3], [vec![0xf1], vec![0x80, 0x29, 0x00], vec![0xf6, 0x10, 0x80, 0x80, 0x83, 0x07, 0x00, 0x00]]);
    // the idle SOURCE_STATUS, as the BM5 announces itself (LIVE_KITCHEN_AND_STANDBY, 21.924 s)
    let idle = sc_vectors::LIVE_KITCHEN_AND_STANDBY.last().unwrap().bytes.to_vec();
    assert_eq!(h.sent(true), vec![idle]);
}

#[test]
fn startup_as_audio_master() {
    let h = Harness::audio_master();
    assert_eq!(h.sim.commands()[..3], [vec![0xf1], vec![0x80, 0x01, 0x00], vec![0xf6, 0x10, 0xc1, 0x80, 0x83, 0x05, 0x00, 0x00]]);
    // idle status, clock, LINK_INFO probe, one at a time through the pacer
    let sent = h.sent(false);
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[1][7], masterlink::protocol::payload_type::PROGRAMMING_STATUS);
    assert_eq!(sent[2], [0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x02, 0x00, 0x9b, 0x00]);
}

// ---------------------------------------------------------------- capture replays, link source

#[test]
fn replays_kitchen_join_and_all_standby() {
    let mut h = Harness::link_source();
    h.offset = 100.0;
    h.at(-50.0);
    h.incoming(&source_connect(0x7a));
    assert_eq!(h.app, vec![AppEffect::Play(LocalSource::NMusic)]);
    h.clear();
    let expected = replay(&mut h, sc_vectors::LIVE_KITCHEN_AND_STANDBY, vec![], &[]);
    assert_eq!(diff(&expected, &h.sent(true)), "");
    // the kitchen's GO for N.MUSIC (SOURCE_CONTROL), then the audio master's "all standby":
    // stop the player, blank the panel
    assert_eq!(
        h.app,
        vec![AppEffect::SourceKey { source: LocalSource::NMusic, key: beo4::GO }, AppEffect::Stop, AppEffect::Standby]
    );
    assert!(!h.link.mixer().distributing_on_ml);
}

#[test]
fn replays_kitchen_source_changes() {
    let mut h = Harness::link_source();
    h.offset = 100.0;
    h.clear();
    let expected = replay(&mut h, sc_vectors::LIVE_KITCHEN_SOURCES, vec![], &[]);
    assert_eq!(diff(&expected, &h.sent(true)), "");
    assert_eq!(
        h.app,
        vec![
            AppEffect::Play(LocalSource::NMusic),
            AppEffect::SourceKey { source: LocalSource::NMusic, key: beo4::GO },
            AppEffect::Stop,                      // STANDBY_0 for N.MUSIC ...
            AppEffect::Play(LocalSource::NRadio), // ... then SOURCE_CONNECT for N.RADIO
            AppEffect::Stop,
        ]
    );
}

/// The panel picks N.MUSIC, then N.RADIO: we send the Beo4 keys to the video master at the
/// captured moments, it asks the audio master, which connects us.
#[test]
fn replays_panel_via_video_master() {
    let mut h = Harness::link_source();
    h.offset = 100.0;
    h.clear();
    let expected = replay(
        &mut h,
        sc_vectors::LIVE_PANEL,
        vec![
            (16.070, Box::new(|l: &mut Link, now| l.select_source(LocalSource::NMusic, now))),
            (28.741, Box::new(|l: &mut Link, now| l.select_source(LocalSource::NRadio, now))),
        ],
        &[16.070, 28.741],
    );
    assert_eq!(diff(&expected, &h.sent(true)), "");
    assert_eq!(h.app, vec![AppEffect::Play(LocalSource::NMusic), AppEffect::Play(LocalSource::NRadio)]);
    assert_eq!(h.link.playing(), Some(LocalSource::NRadio));
}

#[test]
fn replays_tv_handover() {
    let mut h = Harness::link_source();
    h.offset = 100.0;
    h.at(-50.0);
    h.act(|l, now| l.select_source(LocalSource::NRadio, now));
    h.incoming(&source_connect(0xa1));
    h.clear();
    let expected = replay(&mut h, sc_vectors::LIVE_TV, vec![], &[]);
    assert_eq!(diff(&expected, &h.sent(true)), "");
}

// ---------------------------------------------------------------- capture replays, audio master

/// The 2018 captures: SystemConfiguration 6, no lock traffic, N.MUSIC on CD (8d), RADIO 6f.
/// Started just before `window_start`, so the periodic clock broadcast stays out of the window.
fn am_2018(window_start: f64) -> Harness {
    let mut h = Harness::new(Config {
        system_configuration: 6,
        lock_mode: LockMode::Off,
        n_music: 0x8d,
        n_radio: 0x6f,
        clock: clock_2018,
        ..Config::new(LinkRole::AudioMaster)
    });
    h.offset = 1.0 - window_start;
    h
}

#[test]
fn replays_link_room_conversation() {
    let mut h = am_2018(137.0);
    h.clear();
    let expected = replay(&mut h, am_vectors::CONVERSATION_137_420, vec![], &[]);
    assert_eq!(diff(&expected, &h.sent(false)), "");
    let am = h.link.audio_master().unwrap();
    assert_eq!(am.state(), BusState::Distributing);
    assert_eq!(am.current_source(), 0x8d);
    assert!(h.link.mixer().distributing_on_ml);
    // rooms asked for CD (our N.MUSIC slot), then RADIO (N.RADIO), then CD again
    assert_eq!(h.app.first(), Some(&AppEffect::Play(LocalSource::NMusic)));
    assert!(h.app.contains(&AppEffect::Play(LocalSource::NRadio)));
    // ... and sent SOURCE_CONTROL keys for it (0d: 1e, 1e, 1f)
    assert!(h.app.contains(&AppEffect::SourceKey { source: LocalSource::NMusic, key: 0x1e }));
}

/// The audio master picks TV (a video master source), then goes to standby (735 s - 780 s)
#[test]
fn replays_tv_and_standby() {
    let mut h = am_2018(735.0);
    h.clear();
    let expected = replay(
        &mut h,
        am_vectors::TV_735_780,
        vec![(735.9, Box::new(|l: &mut Link, now| l.select_tv(now))), (779.7, Box::new(|l: &mut Link, now| l.standby(now)))],
        &[],
    );
    assert_eq!(diff(&expected, &h.sent(false)), "");
    assert!(!h.link.mixer().transmitting_from_ml);
    assert!(!h.link.mixer().speakers_on);
    assert_eq!(h.link.audio_master().unwrap().state(), BusState::Idle);
}

#[test]
fn replays_tv_and_standby_second_time() {
    let mut h = am_2018(1627.0);
    h.clear();
    let expected = replay(
        &mut h,
        am_vectors::TV_1627_1642,
        vec![(1627.4, Box::new(|l: &mut Link, now| l.select_tv(now))), (1641.1, Box::new(|l: &mut Link, now| l.standby(now)))],
        &[],
    );
    assert_eq!(diff(&expected, &h.sent(false)), "");
}

/// While listening to the video master's TV, a room asks for A.MEM (our N.MUSIC slot in that
/// setup): we take the bus back and play it for the room (984 s)
#[test]
fn replays_taking_bus_back_from_video_master() {
    let mut h = Harness::new(Config {
        system_configuration: 6,
        lock_mode: LockMode::Off,
        n_music: ml_source::A_MEM,
        clock: clock_2018,
        ..Config::new(LinkRole::AudioMaster)
    });
    h.offset = 1.0 - 899.0;
    h.at(899.0);
    h.act(|l, now| l.select_tv(now));
    h.incoming(&[0xc1, 0xc0, 0x01, 0x14, 0x00, 0x00, 0x00, 0x44, 0x07, 0x05, 0x02, 0x0b, 0x00, 0x02, 0x01, 0x00, 0x00]);
    assert_eq!(h.link.audio_master().unwrap().state(), BusState::ListeningToVm);
    assert!(h.link.mixer().transmitting_from_ml);
    h.clear();
    let expected = replay(&mut h, am_vectors::AMEM_FROM_TV_984_986, vec![], &[]);
    assert_eq!(diff(&expected, &h.sent(false)), "");
    assert!(!h.link.mixer().transmitting_from_ml);
    assert_eq!(h.link.audio_master().unwrap().current_source(), ml_source::A_MEM);
    assert_eq!(h.app, vec![AppEffect::Play(LocalSource::NMusic)]);
}

#[test]
fn replays_all_standby_from_link_room() {
    let mut h = am_2018(1500.0);
    h.at(1500.0);
    h.incoming(&[0xc1, 0x01, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x07, 0x01, 0x02, 0x6f, 0x00, 0x02, 0x01, 0x00, 0x00]);
    h.at(1502.0);
    assert_eq!(h.link.audio_master().unwrap().state(), BusState::Distributing);
    assert_eq!(h.link.playing(), Some(LocalSource::NRadio));
    h.clear();
    let expected = replay(&mut h, am_vectors::STANDBY_ALL_1593_1595, vec![], &[]);
    assert_eq!(diff(&expected, &h.sent(false)), "");
    assert_eq!(h.app, vec![AppEffect::Stop, AppEffect::Standby]);
    assert!(!h.link.mixer().distributing_on_ml);
}

// ---------------------------------------------------------------- the role table

fn remote_key(key: u8) -> Bytes {
    // c0 c1 01 0a 00 47 00 20 05 02 | 00 01 ff ff <key>, as the panel sends it
    vec![0xc0, 0xc1, 0x01, 0x0a, 0x00, 0x47, 0x00, 0x20, 0x05, 0x02, 0x00, 0x01, 0xff, 0xff, key]
}

#[test]
fn link_source_volume_is_beo4_keys() {
    let mut h = Harness::link_source();
    h.act(|l, _| {
        l.rebase_volume(45);
        Outputs::default()
    });
    h.clear();
    h.act(|l, now| l.set_volume(48, now)); // 3 counts: not yet a key
    assert!(h.sent(true).is_empty());
    h.act(|l, now| l.set_volume(54, now)); // 9 counts: two keys, one left over
    assert_eq!(h.sent(true), vec![remote_key(beo4::VOL_UP), remote_key(beo4::VOL_UP)]);
    h.clear();
    h.act(|l, now| l.set_volume(49, now)); // -5: +1 left over, so one key down and 0 left
    assert_eq!(h.sent(true), vec![remote_key(beo4::VOL_DOWN)]);
    assert!(h.sim.commands().iter().all(|c| c[0] != 0xeb), "no PC2 volume in this role");
}

#[test]
fn audio_master_volume_goes_to_the_pc2() {
    let mut h = Harness::audio_master();
    h.clear();
    h.act(|l, now| l.set_volume(40, now));
    assert_eq!(h.sim.commands(), vec![vec![0xeb, 40]]);
    // the PC2 reports it back (IN 03), and the UI hears about it
    assert_eq!(h.app, vec![AppEffect::Volume(40)]);
    assert!(h.sent(false).is_empty());
}

#[test]
fn audio_master_source_turns_speakers_on_in_bo_order() {
    let mut h = Harness::audio_master();
    h.clear();
    h.act(|l, now| l.select_source(LocalSource::NMusic, now));
    assert_eq!(
        h.sim.commands(),
        vec![
            vec![0xe3, 0xa2, 0xff, 0x00, 0x00], // volume 34, loudness, bass -1 (B&O's)
            vec![0xe7, 0x00],
            vec![0xe5, 0x01, 0x00, 0x00, 0x00], // transmit locally
            vec![0xea, 0xff],
            vec![0xea, 0x81], // speakers on, then unmute
        ]
    );
    assert_eq!(h.app, vec![AppEffect::Play(LocalSource::NMusic), AppEffect::Volume(34)]);
    h.clear();
    h.act(|l, now| l.standby(now));
    assert_eq!(h.sim.commands(), vec![vec![0xea, 0x80], vec![0xea, 0x00], vec![0xe7, 0x01], vec![0xe5, 0x00, 0x00, 0x00, 0x01]]);
    assert_eq!(h.app, vec![AppEffect::Stop, AppEffect::Standby]);
}

#[test]
fn link_source_picks_by_beo4_key_and_plays_when_connected() {
    let mut h = Harness::link_source();
    h.clear();
    h.act(|l, now| l.select_source(LocalSource::NMusic, now));
    assert_eq!(h.sent(true), vec![remote_key(beo4::N_MUSIC)]);
    assert!(h.app.is_empty(), "nothing plays before the master connects us");
    h.incoming(&source_connect(0x7a));
    assert_eq!(h.app, vec![AppEffect::Play(LocalSource::NMusic)]);
    // A.AUX rides on the N.MUSIC slot in this role: already on the link, so it switches now
    h.clear();
    h.act(|l, now| l.select_source(LocalSource::AAux, now));
    assert_eq!(h.sent(true), vec![remote_key(beo4::N_MUSIC)]);
    assert_eq!(h.app, vec![AppEffect::Play(LocalSource::AAux)]);
}

#[test]
fn short_standby_by_role() {
    let mut h = Harness::link_source();
    h.clear();
    h.act(|l, now| l.standby(now));
    assert_eq!(h.sent(true), vec![remote_key(beo4::STANDBY)]);
    assert_eq!(h.app, vec![AppEffect::Standby]);

    // audio master with a link room listening: speakers off, but the room keeps our source
    let mut h = Harness::audio_master();
    h.act(|l, now| l.select_source(LocalSource::NMusic, now));
    h.at(10.0);
    h.incoming(&[0xc1, 0x04, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x07, 0x01, 0x02, 0x7a, 0x00, 0x02, 0x01, 0x00, 0x00]);
    h.at(12.0); // no video master: we take the bus
    assert!(h.link.audio_master().unwrap().listeners().contains(&0x04));
    h.clear();
    h.act(|l, now| l.standby(now));
    assert!(!h.link.mixer().speakers_on);
    assert!(h.link.mixer().distributing_on_ml);
    assert_eq!(h.app, vec![AppEffect::Standby], "the player keeps going for the link room");
}

#[test]
fn long_standby_as_link_source() {
    let mut h = Harness::link_source();
    h.incoming(&source_connect(0x7a));
    h.clear();
    h.act(|l, now| l.all_standby(now));
    // the firmware lock (24, granted by IN 44 01), STANDBY_1 to ALL from PC_1, release (e4 01)
    assert_eq!(h.sim.commands(), vec![vec![0x24], vec![0xe7, 0x01], vec![0xe5, 0x00, 0x00, 0x00, 0x01], vec![0xe4, 0x01]]);
    let sent = h.sent(true);
    assert_eq!(sent[0], [0x80, 0xc2, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x11, 0x00, 0x01]);
    // then the stopped and idle SOURCE_STATUS, as on a received all standby
    let captured: Vec<Bytes> =
        sc_vectors::LIVE_KITCHEN_AND_STANDBY.iter().filter(|r| r.out && r.t > 21.0).map(|r| r.bytes.to_vec()).collect();
    assert_eq!(sent[1..], captured[..]);
    assert_eq!(h.app, vec![AppEffect::Stop, AppEffect::Standby]);
}

#[test]
fn long_standby_as_audio_master() {
    let mut h = Harness::audio_master();
    h.act(|l, now| l.select_source(LocalSource::NMusic, now));
    h.clear();
    h.act(|l, now| l.all_standby(now));
    let cmds = h.sim.commands();
    assert_eq!(cmds.first(), Some(&vec![0xea, 0x80]), "speakers off first");
    assert!(cmds.contains(&vec![0x24]) && cmds.contains(&vec![0xe4, 0x01]), "under the bus lock");
    assert_eq!(h.sent(false)[0][..10], [0x80, 0xc1, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x11, 0x00, 0x01]);
    assert_eq!(h.app, vec![AppEffect::Stop, AppEffect::Standby]);
}

// ---------------------------------------------------------------- IR from the PC2

#[test]
fn ir_acts_only_as_audio_master() {
    let mut h = Harness::link_source();
    h.clear();
    for code in [beo4::N_MUSIC, beo4::VOL_UP, beo4::STANDBY] {
        h.ir(0x01, code);
    }
    h.ir(beo4::TYPE_HELD, beo4::STANDBY);
    assert!(h.sim.received.is_empty() && h.app.is_empty(), "the room's master has its own eye");

    let mut h = Harness::audio_master();
    h.clear();
    h.ir(0x01, beo4::N_MUSIC);
    assert_eq!(h.link.playing(), Some(LocalSource::NMusic));
    assert!(h.link.mixer().speakers_on);
    h.clear();
    h.ir(0x01, beo4::VOL_UP);
    assert_eq!(h.sim.commands(), vec![vec![0xeb, 35]]);
    h.clear();
    h.ir(0x01, beo4::GO); // transport keys go to the playing source
    assert_eq!(h.app, vec![AppEffect::SourceKey { source: LocalSource::NMusic, key: beo4::GO }]);
    h.clear();
    h.ir(0x01, beo4::STANDBY);
    assert!(!h.link.mixer().speakers_on);
    assert_eq!(h.app, vec![AppEffect::Stop, AppEffect::Standby]);
    h.clear();
    h.ir(beo4::TYPE_HELD, beo4::STANDBY); // held: everything off
    assert_eq!(h.sent(false)[0][..10], [0x80, 0xc1, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x11, 0x00, 0x01]);
}

#[test]
fn ir_tv_listens_to_the_video_master() {
    let mut h = Harness::audio_master();
    h.clear();
    h.ir(0x01, beo4::TV);
    // (lock) then LINK_INFO to the video master, as in the TV captures
    assert!(h.sim.commands().contains(&vec![0x24]));
    assert_eq!(h.sent(false)[0], [0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x02, 0x00, 0x9b, 0x00]);
}

// ---------------------------------------------------------------- track info (PR #10)

#[test]
fn track_info_follows_the_connect_and_track_changes() {
    let mut h = Harness::link_source();
    h.clear();
    h.incoming(&source_connect(0x7a));
    let all = h.sim.telegrams();
    let info: Vec<_> = all.iter().filter(|t| t[3] == masterlink::protocol::telegram_type::INFO).collect();
    assert_eq!(info.len(), 7, "DISPLAY_DATA and six METADATA");
    let first_info = all.iter().position(|t| t[3] == masterlink::protocol::telegram_type::INFO).unwrap();
    assert!(first_info >= 3, "after the connect replies");
    assert_eq!(info[0][..8], [node::ALL_SOURCE_USERS, node::SOURCE_CENTER, 0x01, 0x2c, 0x00, 0x7a, 0x00, 0x06]);
    h.clear();
    let track = TrackInfo { title: "Loungemeister".into(), artist: "Ugress".into(), ..TrackInfo::unknown() };
    h.act(|l, now| l.track_info(track, now));
    assert_eq!(h.sim.telegrams().len(), 7);
    // not distributing: nothing goes out
    let mut h = Harness::link_source();
    h.clear();
    h.act(|l, now| l.track_info(TrackInfo::unknown(), now));
    assert!(h.sim.telegrams().is_empty());
}

// ---------------------------------------------------------------- PC2 housekeeping

#[test]
fn temperature_is_answered_with_fan_duty() {
    let mut h = Harness::link_source();
    h.sim.temperature(40); // the first report after opening is stale and skipped
    h.pump();
    h.clear();
    h.sim.temperature(40);
    h.pump();
    assert_eq!(h.sim.commands().len(), 1);
    assert_eq!(h.sim.commands()[0][0], 0xf9);
}
