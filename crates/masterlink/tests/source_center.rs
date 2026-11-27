//! SourceCenter tests: replay live captures of a BeoMaster 5 (original software) acting as source
//! center for a BeoSound 3000 audio master, and check we answer like it did.
//! Port of libpc2 masterlink/source_center_tests.cpp.

mod common;

use common::*;
use masterlink::protocol::node;
use masterlink::{SourceCenter, Time};

mod vectors {
    use super::common::Row;
    include!("vectors/source_center_live.rs");
}
use vectors::*;

fn at(t: f64) -> Time {
    at_offset(t, 100_000)
}

fn mlgw_host() -> FakeHost {
    FakeHost { mlgw: true, ..Default::default() }
}

fn replay(sc: &mut SourceCenter, host: &mut FakeHost, w: &[Row]) -> Vec<Bytes> {
    let mut expected = Vec::new();
    host.sent.clear();
    for row in w {
        sc.tick(host, at(row.t));
        if row.out {
            expected.push(row.bytes.to_vec());
        } else {
            sc.handle(host, &incoming(row.bytes), at(row.t));
        }
    }
    sc.tick(host, at(w.last().unwrap().t + 10.0));
    expected
}

/// Put the source center into "distributing <src>" before a replay
fn distribute(sc: &mut SourceCenter, host: &mut FakeHost, src: u8) {
    let req = incoming(&[
        0xc2, 0xc1, 0x01, 0x0b, src, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02, 0x00,
    ]);
    sc.handle(host, &req, at(-50.0));
    host.sent.clear();
}

/// Kitchen joins N.MUSIC while the office plays it; then the audio master sends "all standby" (§6)
#[test]
fn replays_kitchen_join_and_all_standby() {
    let mut host = mlgw_host();
    let mut sc = SourceCenter::new();
    distribute(&mut sc, &mut host, 0x7a);
    let expected = replay(&mut sc, &mut host, LIVE_KITCHEN_AND_STANDBY);
    assert_eq!(diff(&expected, &host.sent), "");
    assert!(host.standby_seen);
    assert!(!sc.distributing());
}

/// Kitchen: N.MUSIC from standby -> N.RADIO -> the audio master's CD -> off (§6)
#[test]
fn replays_kitchen_source_changes() {
    let mut host = mlgw_host();
    let mut sc = SourceCenter::new();
    let expected = replay(&mut sc, &mut host, LIVE_KITCHEN_SOURCES);
    assert_eq!(diff(&expected, &host.sent), "");
    assert_eq!(host.started, vec![0x7a, 0xa1]);
    assert!(!sc.distributing());
}

/// Panel picks N.MUSIC, then N.RADIO: the video master asks the audio master, which orders us (§7)
#[test]
fn replays_panel_via_video_master() {
    let mut host = mlgw_host();
    let mut sc = SourceCenter::new();
    sc.set_local_playback(true);
    let expected = replay(&mut sc, &mut host, LIVE_PANEL);
    assert_eq!(diff(&expected, &host.sent), "");
    assert_eq!(sc.current_source(), 0xa1);
}

/// TV takes the bus from our N.RADIO, then the kitchen takes it back for N.MUSIC (§8)
#[test]
fn replays_tv_handover() {
    let mut host = mlgw_host();
    let mut sc = SourceCenter::new();
    sc.set_local_playback(true);
    distribute(&mut sc, &mut host, 0xa1);
    let expected = replay(&mut sc, &mut host, LIVE_TV);
    assert_eq!(diff(&expected, &host.sent), "");
}

#[test]
fn remote_key_matches_panel_telegram() {
    // captured: c0 c1 01 0a 00 47 00 20 05 02 00 01 ff ff 94  ("N.Music" to the video master)
    let mut host = mlgw_host();
    let sc = SourceCenter::new();
    sc.send_remote_key(&mut host, 0xc0, 0x94, node::AUDIO_MASTER);
    assert_eq!(host.sent, vec![vec![0xc0, 0xc1, 0x01, 0x0a, 0x00, 0x47, 0x00, 0x20, 0x05, 0x02, 0x00, 0x01, 0xff, 0xff, 0x94]]);
}

#[test]
fn idle_status_only_after_delay_and_without_local_playback() {
    let mut host = mlgw_host();
    let mut sc = SourceCenter::new();
    distribute(&mut sc, &mut host, 0x7a);
    let stop = incoming(&[0xc2, 0xc1, 0x01, 0x0a, 0x7a, 0x00, 0x00, 0x10, 0x00, 0x03]);
    sc.handle(&mut host, &stop, at(0.0));
    sc.tick(&mut host, at(1.0));
    assert_eq!(host.sent.len(), 0);
    sc.tick(&mut host, at(2.1));
    assert_eq!(host.sent.len(), 1); // idle SOURCE_STATUS 47
}
