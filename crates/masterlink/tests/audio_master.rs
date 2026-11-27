//! AudioMaster tests: replay conversations captured from B&O's own audio master (a BeoSound 5)
//! and check that we send the same Masterlink telegrams, byte for byte.
//! Port of libpc2 masterlink/audio_master_tests.cpp.

mod common;

use std::collections::BTreeSet;

use common::*;
use masterlink::{AudioMaster, BusState, LockMode, Telegram, Time};

mod vectors {
    use super::common::Row;
    include!("vectors/audio_master_capture.rs");
}
use vectors::*;

fn at(t: f64) -> Time {
    at_offset(t, 0)
}

/// An action at a capture time: a local API call.
type Action<'a> = (f64, Box<dyn FnMut(&mut AudioMaster, &mut FakeHost, Time) + 'a>);

/// Feed the IN telegrams of a captured window; run `actions` at their times; return what we sent
/// and what B&O's software sent in the same window.
fn replay(am: &mut AudioMaster, host: &mut FakeHost, window: &[Row], mut actions: Vec<Action>) -> (Vec<Bytes>, Vec<Bytes>) {
    let mut expected = Vec::new();
    host.sent.clear();
    actions.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut actions = actions.into_iter().peekable();
    for row in window {
        while let Some((t, _)) = actions.peek() {
            if *t > row.t {
                break;
            }
            let (t, mut f) = actions.next().unwrap();
            f(am, host, at(t));
        }
        am.tick(host, at(row.t));
        if row.out {
            expected.push(row.bytes.to_vec());
        } else {
            am.handle(host, &incoming(row.bytes), at(row.t));
        }
    }
    for (t, mut f) in actions {
        f(am, host, at(t));
    }
    (host.sent.clone(), expected)
}

/// The captures were taken in SystemConfiguration 6, with no lock traffic (2018).
fn am_2018() -> (AudioMaster, FakeHost) {
    let mut am = AudioMaster::new(6);
    am.set_lock_participation(false);
    (am, FakeHost::default())
}

#[test]
fn serialized_checksum_matches_capture() {
    // REQUEST LINK_INFO to the video master, as captured: ... 9a 00
    let (mut am, mut host) = am_2018();
    am.listen_to_video_source(&mut host, 0x0b, at(0.0));
    assert!(!host.sent.is_empty());
    assert_eq!(host.sent[0], [0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, 0x9a, 0x00]);
}

#[test]
fn link_info_request_follows_system_configuration() {
    // SystemConfiguration 5 (what libpc2 sends in audio-master mode): "01 02 00", as in every
    // configuration 5 USB capture (e.g. PC2_AM_ML_source_changes, 0.656 s)
    let mut host = FakeHost::default();
    let mut am = AudioMaster::default();
    am.start(&mut host, Time::ZERO);
    assert_eq!(host.sent.len(), 3);
    assert_eq!(host.sent[2], [0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x02, 0x00, 0x9b, 0x00]);
}

/// Link rooms connect, switch source, leave and come back (capture 137 s - 420 s): bus
/// arbitration with the video master, listener tracking, source change notices.
#[test]
fn replays_link_room_conversation() {
    let (mut am, mut host) = am_2018();
    let (got, expected) = replay(&mut am, &mut host, CONVERSATION_137_420, vec![]);
    assert_eq!(got.len(), expected.len(), "{}", diff(&expected, &got));
    assert_eq!(diff(&expected, &got), "");
    assert_eq!(am.state(), BusState::Distributing);
    assert_eq!(am.current_source(), 0x8d);
    assert_eq!(am.listeners(), &BTreeSet::from([0x01, 0x04]));
    assert!(host.distributing);
}

/// The audio master picks TV (a video master source), then goes to standby (735 s - 780 s)
#[test]
fn replays_tv_and_standby() {
    let (mut am, mut host) = am_2018();
    let mut listening_before_standby = false;
    let (got, expected) = replay(
        &mut am,
        &mut host,
        TV_735_780,
        vec![
            (735.9, Box::new(|am: &mut AudioMaster, h: &mut FakeHost, now| am.listen_to_video_source(h, 0x0b, now))),
            (
                779.7,
                Box::new(|am: &mut AudioMaster, h: &mut FakeHost, now| {
                    listening_before_standby = h.listening;
                    am.local_standby(h, now)
                }),
            ),
        ],
    );
    assert_eq!(diff(&expected, &got), "");
    assert!(listening_before_standby);
    assert!(!host.listening);
    assert_eq!(am.state(), BusState::Idle);
}

#[test]
fn replays_tv_and_standby_second_time() {
    let (mut am, mut host) = am_2018();
    let (got, expected) = replay(
        &mut am,
        &mut host,
        TV_1627_1642,
        vec![
            (1627.4, Box::new(|am: &mut AudioMaster, h: &mut FakeHost, now| am.listen_to_video_source(h, 0x0b, now))),
            (1641.1, Box::new(|am: &mut AudioMaster, h: &mut FakeHost, now| am.local_standby(h, now))),
        ],
    );
    assert_eq!(diff(&expected, &got), "");
}

/// A link room sends "all standby" while we distribute radio (1593 s)
#[test]
fn replays_all_standby_from_link_room() {
    let (mut am, mut host) = am_2018();
    // pre-state: distributing RADIO (0x6f) to node 01, no video master answering
    let req = incoming(&[0xc1, 0x01, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x07, 0x01, 0x02, 0x6f, 0x00, 0x02, 0x01, 0x00, 0x00]);
    am.handle(&mut host, &req, at(1500.0));
    am.tick(&mut host, at(1502.0));
    assert_eq!(am.state(), BusState::Distributing);
    let (got, expected) = replay(&mut am, &mut host, STANDBY_ALL_1593_1595, vec![]);
    assert_eq!(diff(&expected, &got), "");
    assert!(host.standby_seen);
    assert!(!host.distributing);
    assert!(am.listeners().is_empty());
}

/// While listening to the video master's TV, a room asks for A.MEM: we take the bus back (984 s)
#[test]
fn replays_taking_bus_back_from_video_master() {
    let (mut am, mut host) = am_2018();
    am.listen_to_video_source(&mut host, 0x0b, at(899.0));
    let confirm =
        incoming(&[0xc1, 0xc0, 0x01, 0x14, 0x00, 0x00, 0x00, 0x44, 0x07, 0x05, 0x02, 0x0b, 0x00, 0x02, 0x01, 0x00, 0x00]);
    am.handle(&mut host, &confirm, at(900.0));
    assert_eq!(am.state(), BusState::ListeningToVm);
    let (got, expected) = replay(&mut am, &mut host, AMEM_FROM_TV_984_986, vec![]);
    assert_eq!(diff(&expected, &got), "");
    assert!(!host.listening);
    assert_eq!(am.current_source(), 0x79);
}

#[test]
fn startup_and_periodic_clock() {
    let (mut am, mut host) = am_2018();
    am.start(&mut host, at(0.0));
    // idle SOURCE_STATUS from PC_1, CLOCK, LINK_INFO probe
    assert_eq!(host.sent.len(), 3);
    // clock as captured at 0.656 s: 80 c1 01 14 ... 40 0b 0b 0a 00 03 14 42 28 00 25 12 18 02 88 00
    assert_eq!(
        host.sent[1],
        [
            0x80, 0xc1, 0x01, 0x14, 0x00, 0x00, 0x00, 0x40, 0x0b, 0x0b, 0x0a, 0x00, 0x03, 0x14, 0x42, 0x28, 0x00, 0x25, 0x12,
            0x18, 0x02, 0x88, 0x00
        ]
    );
    host.sent.clear();
    am.tick(&mut host, at(400.0));
    assert_eq!(host.sent.len(), 0);
    am.tick(&mut host, at(431.0));
    assert_eq!(host.sent.len(), 1);
}

#[test]
fn no_video_master_means_take_bus_after_timeout() {
    let (mut am, mut host) = am_2018();
    let req = incoming(&[0xc1, 0x04, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x07, 0x01, 0x02, 0x8d, 0x00, 0x02, 0x01, 0x00, 0x00]);
    am.handle(&mut host, &req, at(10.0));
    assert_eq!(am.state(), BusState::Arbitrating);
    am.tick(&mut host, at(10.5));
    assert_eq!(am.state(), BusState::Arbitrating);
    am.tick(&mut host, at(11.1));
    assert_eq!(am.state(), BusState::Distributing);
    assert_eq!(am.listeners(), &BTreeSet::from([0x04]));
}

#[test]
fn local_standby_keeps_distributing_to_listeners() {
    let (mut am, mut host) = am_2018();
    let req = incoming(&[0xc1, 0x01, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x07, 0x01, 0x02, 0x8d, 0x00, 0x02, 0x01, 0x00, 0x00]);
    am.handle(&mut host, &req, at(10.0));
    am.tick(&mut host, at(12.0));
    am.local_standby(&mut host, at(13.0));
    assert_eq!(am.state(), BusState::Distributing);
    assert!(host.distributing);
}

// ---- Bus handover and lock, checked against what the BeoSound 3000 audio master sent (live, 2026-11-27)

/// MLGW bytes have no checksum/EOT: compare without them
fn strip(b: &[u8]) -> Vec<u8> {
    b[..b.len() - 2].to_vec()
}
fn sent_contains(h: &FakeHost, mlgw: &[u8]) -> bool {
    h.sent.iter().any(|b| strip(b) == mlgw)
}

/// The video master wants the bus for TV while we distribute N.RADIO (capture §8, 04:02:02)
#[test]
fn handover_video_master_takes_the_bus() {
    let (mut am, mut host) = am_2018();
    let req = incoming(&[0xc1, 0x04, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x07, 0x01, 0x02, 0xa1, 0x00, 0x02, 0x01, 0x00, 0x00]);
    am.handle(&mut host, &req, at(0.0));
    am.tick(&mut host, at(2.0)); // no VM answer: take the bus
    assert_eq!(am.state(), BusState::Distributing);
    host.sent.clear();
    // VM: REQUEST 0x44 v3 "02 a1 00 02 01 00 03 01 00 00 00 02 0b"
    let change = incoming(&[
        0xc1, 0xc0, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x44, 0x0d, 0x03, 0x02, 0xa1, 0x00, 0x02, 0x01, 0x00, 0x03, 0x01, 0x00, 0x00,
        0x00, 0x02, 0x0b,
    ]);
    am.handle(&mut host, &change, at(3.0));
    // BS3000 answered: c0 c1 01 14 00 00 00 44 0d 07 02 a1 00 02 01 00 03 01 00 00 00 03 0b
    assert!(sent_contains(
        &host,
        &[
            0xc0, 0xc1, 0x01, 0x14, 0x00, 0x00, 0x00, 0x44, 0x0d, 0x07, 0x02, 0xa1, 0x00, 0x02, 0x01, 0x00, 0x03, 0x01, 0x00,
            0x00, 0x00, 0x03, 0x0b
        ]
    ));
    host.sent.clear();
    let claim = incoming(&[0xc1, 0xc0, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x10, 0x03, 0x03, 0x01, 0x00, 0x01]);
    am.handle(&mut host, &claim, at(3.2));
    assert_eq!(am.state(), BusState::Idle);
    assert!(!host.distributing);
    // BS3000: c0 c1 01 0a 00 00 00 1c 01 02 01
    assert!(sent_contains(&host, &[0xc0, 0xc1, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x1c, 0x01, 0x02, 0x01]));
}

/// A room wants N.MUSIC while the video master plays TV on the bus (capture §8, 04:02:32)
#[test]
fn handover_claim_bus_from_busy_video_master() {
    let (mut am, mut host) = am_2018();
    let req = incoming(&[0xc1, 0x04, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x07, 0x01, 0x02, 0x7a, 0x00, 0x02, 0x01, 0x00, 0x00]);
    am.handle(&mut host, &req, at(0.0));
    let li = incoming(&[0xc1, 0xc0, 0x01, 0x14, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x02, 0x01, 0x01]);
    am.handle(&mut host, &li, at(0.03));
    host.sent.clear();
    let busy = incoming(&[0xc1, 0xc0, 0x01, 0x14, 0x00, 0x00, 0x00, 0x08, 0x05, 0x06, 0x02, 0x01, 0x00, 0x0b, 0x01]);
    am.handle(&mut host, &busy, at(0.08));
    // BS3000: c0 c1 01 0b 00 00 00 44 0d 03 02 0b 00 02 01 00 03 01 00 00 00 ff 7a
    assert!(sent_contains(
        &host,
        &[
            0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x44, 0x0d, 0x03, 0x02, 0x0b, 0x00, 0x02, 0x01, 0x00, 0x03, 0x01, 0x00,
            0x00, 0x00, 0xff, 0x7a
        ]
    ));
    assert_eq!(am.state(), BusState::Arbitrating);
    host.sent.clear();
    let notice = incoming(&[
        0xc1, 0xc0, 0x01, 0x14, 0x00, 0x00, 0x00, 0x44, 0x0d, 0x07, 0x02, 0x0b, 0x00, 0x02, 0x01, 0x00, 0x02, 0x01, 0x00, 0x00,
        0x00, 0x00, 0x7a,
    ]);
    am.handle(&mut host, &notice, at(0.25));
    // BS3000: c0 c1 01 0a 00 00 00 10 03 03 01 00 01
    assert!(sent_contains(&host, &[0xc0, 0xc1, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x10, 0x03, 0x03, 0x01, 0x00, 0x01]));
    assert_eq!(am.state(), BusState::Arbitrating);
    let released = incoming(&[0xc1, 0xc0, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x1c, 0x02, 0x01, 0x01, 0x04]);
    am.handle(&mut host, &released, at(0.27));
    assert_eq!(am.state(), BusState::Distributing);
    assert_eq!(am.listeners(), &BTreeSet::from([0x04]));
    assert!(host.distributing);
}

#[test]
fn handover_change_request_times_out_into_claim() {
    let (mut am, mut host) = am_2018();
    let req = incoming(&[0xc1, 0x04, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x07, 0x01, 0x02, 0x7a, 0x00, 0x02, 0x01, 0x00, 0x00]);
    am.handle(&mut host, &req, at(0.0));
    let li = incoming(&[0xc1, 0xc0, 0x01, 0x14, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x02, 0x01, 0x01]);
    am.handle(&mut host, &li, at(0.03));
    let busy = incoming(&[0xc1, 0xc0, 0x01, 0x14, 0x00, 0x00, 0x00, 0x08, 0x05, 0x06, 0x02, 0x01, 0x00, 0x0b, 0x01]);
    am.handle(&mut host, &busy, at(0.08));
    am.tick(&mut host, at(1.2)); // no 0x44 v7: claim anyway
    am.tick(&mut host, at(2.4)); // no 0x1C: take the bus
    assert_eq!(am.state(), BusState::Distributing);
}

/// LOCK_MANAGER: we hand the lock over when we hold it, and take it before picking TV
/// (capture §8, 04:02:17)
#[test]
fn lock_take_before_video_source_and_hand_it_over() {
    let mut host = FakeHost::default();
    let mut am = AudioMaster::new(6);
    am.set_lock_mode(LockMode::Software);
    am.listen_to_video_source(&mut host, 0x0b, at(0.0));
    assert_eq!(host.sent.len(), 1);
    // BS3000: 80 c1 01 0b 00 00 00 5c 01 01 01
    assert_eq!(strip(&host.sent[0]), [0x80, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x5c, 0x01, 0x01, 0x01]);
    host.sent.clear();
    let holder = incoming(&[0xc1, 0x04, 0x01, 0x14, 0x00, 0x00, 0x00, 0x5c, 0x01, 0x02, 0x01]);
    am.handle(&mut host, &holder, at(0.02));
    // BS3000: 04 c1 01 0a 00 00 00 5c 01 04 01, then LINK_INFO and LINK_CONNECT_1 to the VM
    assert_eq!(host.sent.len(), 3);
    assert_eq!(strip(&host.sent[0]), [0x04, 0xc1, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x5c, 0x01, 0x04, 0x01]);
    assert!(am.holds_bus_lock());
    host.sent.clear();
    // A room asks for the lock: we answer (BS3000: 04 c1 01 14 00 00 00 5c 01 02 01) and give it up
    let ask = incoming(&[0x80, 0x04, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x5c, 0x01, 0x01, 0x01]);
    am.handle(&mut host, &ask, at(10.0));
    assert_eq!(host.sent.len(), 1);
    assert_eq!(strip(&host.sent[0]), [0x04, 0xc1, 0x01, 0x14, 0x00, 0x00, 0x00, 0x5c, 0x01, 0x02, 0x01]);
    let take = incoming(&[0xc1, 0x04, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x5c, 0x01, 0x04, 0x01]);
    am.handle(&mut host, &take, at(10.02));
    assert!(!am.holds_bus_lock());
}

/// The holder answers TRANSFER_IMPOSSIBLE (STATUS 3, as BeoIpcML does while a local user still owns
/// the key): that is not a grant, so we send no KEY_RECEIVED and wait for the lock timeout
#[test]
fn lock_transfer_impossible_is_not_a_grant() {
    let mut host = FakeHost::default();
    let mut am = AudioMaster::new(6);
    am.set_lock_mode(LockMode::Software);
    am.listen_to_video_source(&mut host, 0x0b, at(0.0));
    host.sent.clear();
    let refused = incoming(&[0xc1, 0x04, 0x01, 0x14, 0x00, 0x00, 0x00, 0x5c, 0x01, 0x03, 0x01]);
    am.handle(&mut host, &refused, at(0.02));
    assert_eq!(host.sent.len(), 0);
    assert!(!am.holds_bus_lock());
}

#[test]
fn lock_nobody_holds_the_lock() {
    let mut host = FakeHost::default();
    let mut am = AudioMaster::new(6);
    am.set_lock_mode(LockMode::Software);
    am.listen_to_video_source(&mut host, 0x0b, at(0.0));
    am.tick(&mut host, at(0.6));
    assert!(am.holds_bus_lock());
    assert_eq!(host.sent.len(), 3); // lock request, LINK_INFO, LINK_CONNECT_1
}

/// Firmware lock (default): the PC2 runs LOCK_MANAGER itself. B&O's sequence on TV: 24 -> IN 44 01
/// -> telegrams -> e4 01 (capture BM5asAM 735.844-736.906).
#[test]
fn lock_firmware_request_grant_release() {
    let mut host = FakeHost::default();
    let mut am = AudioMaster::new(6);
    am.listen_to_video_source(&mut host, 0x0b, at(0.0));
    assert_eq!(host.lock_requests, 1);
    assert_eq!(host.sent.len(), 0); // nothing on the bus until the lock is granted
    am.bus_lock_granted(&mut host);
    assert_eq!(host.sent.len(), 2); // LINK_INFO + LINK_CONNECT_1 to the video master
    assert_eq!(host.lock_releases, 1);
    assert!(!am.holds_bus_lock());
}

#[test]
fn lock_firmware_times_out() {
    let mut host = FakeHost::default();
    let mut am = AudioMaster::new(6);
    am.all_standby(&mut host, at(0.0));
    am.tick(&mut host, at(1.5));
    assert_eq!(host.sent.len(), 0);
    am.tick(&mut host, at(2.1));
    assert_eq!(host.sent.len(), 1); // STANDBY_1 to ALL, sent anyway
}

#[test]
fn lock_firmware_mode_ignores_lock_telegrams() {
    let mut host = FakeHost::default();
    let mut am = AudioMaster::new(6);
    let ask = incoming(&[0x80, 0x04, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x5c, 0x01, 0x01, 0x01]);
    am.handle(&mut host, &ask, at(0.0));
    assert_eq!(host.sent.len(), 0);
}

/// GOTO_SOURCE (LINK_CONNECT_COMMAND_1) byte 2 is the channel: a Beosystem 3 wake-up timer
/// asked for N.RADIO preset 8 this way (work/ml_wake_timer_0700.txt, 06:59:52.812, MLGW form).
#[test]
fn goto_source_passes_the_channel_number() {
    let (mut am, mut host) = am_2018();
    let goto = [
        0xc1, 0xc0, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x45, 0x0d, 0x01, 0x02, 0xa1, 0x08, 0x02, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x03, 0x0c, 0x00,
    ];
    let t = Telegram::decode(&goto).unwrap();
    am.handle(&mut host, &t, at(0.0));
    assert_eq!(host.started, vec![0xa1]);
    assert_eq!(host.channels, vec![(0xa1, 8)]);
    // our own requests carry 00 there: no channel
    use masterlink::protocol::link_connect::{connect_request, requested_channel};
    assert_eq!(requested_channel(&connect_request(0xa1)), None);
}
