//! Ports of libpc2's pc2/ tests: fragment_assembler_tests.cpp, wakeup_tests.cpp,
//! fan_control_tests.cpp and telegram_pacer_tests.cpp. The pacer tests drive time explicitly
//! instead of sleeping threads. mailbox_tests.cpp has no counterpart: channels replace the mailbox.

use std::time::Duration;

use pc2::frame::{wakeup_packet, FragmentAssembler, Wakeup};
use pc2::{FanControl, TelegramPacer, Time};

fn ms(n: u64) -> Time {
    Duration::from_millis(n)
}

// ---- PC2MessageFragmentAssembler

#[test]
fn assembler_has_no_message_when_partial() {
    let mut a = FragmentAssembler::new();
    assert_eq!(a.push(&[0x60, 0x05, 0x00]).unwrap(), None);
}

#[test]
fn assembler_returns_self_contained_message() {
    let mut a = FragmentAssembler::new();
    assert_eq!(a.push(&[0x60, 0x00, 0x61]).unwrap(), Some(vec![0x60, 0x00, 0x61]));
}

#[test]
fn assembler_rejects_bad_start() {
    let mut a = FragmentAssembler::new();
    assert!(a.push(&[0x00, 0x10, 0xe0, 0xc0, 0xc1, 0x01]).is_err());
    assert!(!a.in_progress());
}

#[test]
fn assembler_joins_fragments() {
    let mut a = FragmentAssembler::new();
    assert_eq!(a.push(&[0x60, 0x10, 0xe0, 0xc0, 0xc1, 0x01]).unwrap(), None);
    assert_eq!(a.push(&[0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01]).unwrap(), None);
    let m = a.push(&[0x00, 0x9a, 0x00, 0x61]).unwrap().unwrap();
    assert_eq!(m.len(), 0x10 + 3);
}

#[test]
fn assembler_rejects_bad_end() {
    let mut a = FragmentAssembler::new();
    a.push(&[0x60, 0x10, 0xe0, 0xc0, 0xc1, 0x01]).unwrap();
    a.push(&[0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01]).unwrap();
    assert!(a.push(&[0x00, 0x9a, 0x00, 0x62]).is_err());
    assert!(!a.in_progress());
}

// ---- PC2Wakeup

#[test]
fn recognises_firmware_wakeup_packets() {
    assert_eq!(wakeup_packet(&[0xff, 0xff, 0xff, 0xff]), Some(Wakeup::ByRemote));
    assert_eq!(wakeup_packet(&[0xff, 0xfc, 0xfe, 0xff]), Some(Wakeup::BusResumed));
}

#[test]
fn ignores_everything_else() {
    assert_eq!(wakeup_packet(&[0xff, 0xff, 0x01, 0xff, 0xff]), None); // the "bogus" message
    assert_eq!(wakeup_packet(&[0x60, 0x02, 0x0a, 0x00, 0x61]), None);
    assert_eq!(wakeup_packet(&[0xff, 0xff, 0xff]), None);
}

#[test]
fn assembler_reports_message_in_progress() {
    let mut a = FragmentAssembler::new();
    assert!(!a.in_progress());
    a.push(&[0x60, 0x0d, 0x00, 0xc1, 0xc0, 0x01, 0x14]).unwrap(); // first 7 of 16 bytes
    assert!(a.in_progress());
    a.push(&[0x00, 0x00, 0x00, 0x04, 0x02, 0x04, 0x02]).unwrap();
    let m = a.push(&[0x01, 0x61]).unwrap();
    assert!(!a.in_progress());
    assert!(m.is_some());
}

// ---- PC2FanControl

#[test]
fn fan_skips_first_report_once() {
    let mut f = FanControl::new();
    assert!(f.consume_skip());
    assert!(!f.consume_skip());
}

/// Captured (first_pc2_cap): case at 43-44 C -> 19 32 4b 64 7d 96 af c8 e1 fa ff ff
#[test]
fn fan_hot_climbs_by_0x19_to_full() {
    let mut f = FanControl::new();
    let temps = [43, 43, 43, 44, 44, 44, 44, 44, 44, 44, 44, 44];
    let want = [0x19, 0x32, 0x4b, 0x64, 0x7d, 0x96, 0xaf, 0xc8, 0xe1, 0xfa, 0xff, 0xff];
    for (t, w) in temps.iter().zip(want) {
        assert_eq!(f.on_temperature(*t), w);
    }
}

/// Captured (BM5asAM): 35 C -> 80, 35 -> 7b, 36 (rising) -> 87, 36 -> 82 ...
#[test]
fn fan_in_band_starts_at_0x80_and_decays() {
    let mut f = FanControl::new();
    assert_eq!(f.on_temperature(35), 0x80);
    assert_eq!(f.on_temperature(35), 0x7b);
    assert_eq!(f.on_temperature(36), 0x87);
    assert_eq!(f.on_temperature(36), 0x82);
}

#[test]
fn fan_full_on_overheat_or_bad_reading() {
    let mut f = FanControl::new();
    f.on_temperature(35);
    assert_eq!(f.on_temperature(47), 0xff);
    let mut g = FanControl::new();
    assert_eq!(g.on_temperature(-1), 0xff);
}

#[test]
fn fan_never_below_zero() {
    let mut f = FanControl::new();
    for _ in 0..41 {
        f.on_temperature(30);
    }
    assert_eq!(f.duty(), 0);
}

// ---- PC2TelegramPacer

#[test]
fn pacer_sends_one_telegram_at_a_time() {
    let mut p = TelegramPacer::new(ms(5000));
    assert_eq!(p.enqueue(vec![0xe0, 0x01], ms(0)), Some(vec![0xe0, 0x01]));
    assert_eq!(p.enqueue(vec![0xe0, 0x02], ms(0)), None, "second telegram must wait for the PC2's status answer");
    assert_eq!(p.enqueue(vec![0xe0, 0x03], ms(0)), None);
    assert_eq!(p.tick(ms(50)), None);
    assert_eq!(p.acknowledge(ms(60)), Some(vec![0xe0, 0x02]));
    assert_eq!(p.acknowledge(ms(70)), Some(vec![0xe0, 0x03]));
    assert_eq!(p.timeouts(), 0);
}

#[test]
fn pacer_timeout_releases_next_telegram() {
    let mut p = TelegramPacer::new(ms(30));
    p.enqueue(vec![0xe0, 0x01], ms(0));
    p.enqueue(vec![0xe0, 0x02], ms(0));
    assert_eq!(p.tick(ms(29)), None);
    assert_eq!(p.tick(ms(30)), Some(vec![0xe0, 0x02]));
    assert!(p.timeouts() >= 1);
}

#[test]
fn pacer_stray_acknowledge_does_not_skip_ahead() {
    let mut p = TelegramPacer::new(ms(5000));
    assert_eq!(p.acknowledge(ms(0)), None); // nothing outstanding: ignored
    assert_eq!(p.enqueue(vec![0xe0, 0x01], ms(1)), Some(vec![0xe0, 0x01]));
    assert_eq!(p.enqueue(vec![0xe0, 0x02], ms(1)), None);
    assert_eq!(p.tick(ms(50)), None);
    assert_eq!(p.pending(), 1);
}
