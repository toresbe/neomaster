//! The Pc2 state machine: start-up bytes, pacing on IN 01, the fan loop and message dispatch.

use std::time::Duration;

use pc2::{Event, Mode, Pc2, Time, Wakeup};

fn ms(n: u64) -> Time {
    Duration::from_millis(n)
}

fn drain(p: &mut Pc2) -> Vec<Vec<u8>> {
    std::iter::from_fn(|| p.poll_transmit()).collect()
}

#[test]
fn init_matches_libpc2_for_each_mode() {
    let mut p = Pc2::new(Mode::SourceCenter, 0);
    p.init(ms(0));
    assert_eq!(
        drain(&mut p),
        vec![
            vec![0x60, 0x01, 0xf1, 0x61],
            vec![0x60, 0x03, 0x80, 0x29, 0x00, 0x61],
            vec![0x60, 0x08, 0xf6, 0x10, 0x80, 0x80, 0x83, 0x07, 0x00, 0x00, 0x61],
        ]
    );
    let mut p = Pc2::new(Mode::AudioMaster, 0);
    p.init(ms(0));
    let out = drain(&mut p);
    assert_eq!(out[1], vec![0x60, 0x03, 0x80, 0x01, 0x00, 0x61]);
    assert_eq!(out[2], vec![0x60, 0x08, 0xf6, 0x10, 0xc1, 0x80, 0x83, 0x05, 0x00, 0x00, 0x61]);
}

#[test]
fn telegrams_wait_for_in_01_but_other_messages_do_not() {
    let mut p = Pc2::new(Mode::AudioMaster, 0);
    let t1 = [0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, 0x9a, 0x00];
    p.send_telegram(&t1, ms(0)).unwrap();
    p.send_telegram(&t1, ms(0)).unwrap();
    p.send(vec![0xeb, 0x20], ms(0));
    let out = drain(&mut p);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0][..3], [0x60, 0x10, 0xe0]);
    assert_eq!(out[1], vec![0x60, 0x02, 0xeb, 0x20, 0x61]);
    // the PC2's status answer releases the second telegram
    p.on_usb_packet(&[0x60, 0x04, 0x01, 0x01, 0x01, 0x02, 0x61], ms(5));
    assert_eq!(drain(&mut p).len(), 1);
}

#[test]
fn pacer_times_out_through_tick() {
    let mut p = Pc2::new(Mode::AudioMaster, 0);
    let t1 = [0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x08, 0x00, 0x01, 0x96, 0x00];
    p.send_telegram(&t1, ms(0)).unwrap();
    p.send_telegram(&t1, ms(0)).unwrap();
    drain(&mut p);
    assert_eq!(p.next_deadline(), Some(ms(1000)));
    p.tick(ms(999));
    assert!(p.poll_transmit().is_none());
    p.tick(ms(1000));
    assert!(p.poll_transmit().is_some());
}

#[test]
fn refuses_what_the_firmware_would_drop() {
    let mut p = Pc2::new(Mode::AudioMaster, 0);
    let bad_type = [0xc0, 0xc1, 0x01, 0x87, 0, 0, 0, 0x04, 0x00, 0x04, 0x00, 0x00];
    assert!(p.send_telegram(&bad_type, ms(0)).is_err());
    let mut long = vec![0xc0, 0xc1, 0x01, 0x14, 0, 0, 0, 0x87, 41, 0x04];
    long.extend(std::iter::repeat_n(0, 43));
    assert!(p.send_telegram(&long, ms(0)).is_err());
    assert!(p.poll_transmit().is_none());
}

#[test]
fn temperature_is_answered_after_the_stale_first_report() {
    let mut p = Pc2::new(Mode::SourceCenter, 0);
    p.init(ms(0));
    drain(&mut p);
    assert_eq!(p.on_usb_packet(&[0x60, 0x03, 0x1c, 35, 0xff, 0x61], ms(1)), vec![Event::Temperature(35)]);
    assert!(p.poll_transmit().is_none());
    p.on_usb_packet(&[0x60, 0x03, 0x1c, 35, 0xff, 0x61], ms(30_000));
    assert_eq!(drain(&mut p), vec![vec![0x60, 0x02, 0xf9, 0x80, 0x61]]);
}

#[test]
fn dispatches_incoming_messages() {
    let mut p = Pc2::new(Mode::SourceCenter, 0);
    // a Masterlink telegram split over two packets
    let mut ev = p.on_usb_packet(&[0x60, 0x0d, 0x00, 0xc1, 0xc0, 0x01, 0x14], ms(0));
    assert!(ev.is_empty());
    ev = p.on_usb_packet(&[0x00, 0x00, 0x00, 0x04, 0x02, 0x04, 0x02, 0x01, 0x61], ms(0));
    assert_eq!(ev, vec![Event::Masterlink(vec![0xc1, 0xc0, 0x01, 0x14, 0x00, 0x00, 0x00, 0x04, 0x02, 0x04, 0x02, 0x01])]);
    // Beo4 key, held standby
    assert_eq!(
        p.on_usb_packet(&[0x60, 0x04, 0x02, 0x00, 0x0f, 0x00, 0x0c, 0x61], ms(0)),
        vec![Event::Beo4Key { kind: 0x0f, code: 0x0c }]
    );
    assert_eq!(p.on_usb_packet(&[0x60, 0x02, 0x44, 0x01, 0x61], ms(0)), vec![Event::BusLockGranted]);
    assert_eq!(p.on_usb_packet(&[0xff, 0xff, 0xff, 0xff], ms(0)), vec![Event::Wakeup(Wakeup::ByRemote)]);
    assert!(p.on_usb_packet(&[0xff, 0xff, 0x01, 0xff, 0xff], ms(0)).is_empty());
    assert!(p.on_usb_packet(&[0x12, 0x34, 0x56], ms(0)).is_empty()); // malformed: dropped, not fatal
}

#[test]
fn adopts_node_address_from_ml_status() {
    let mut p = Pc2::new(Mode::SourceCenter, 0);
    assert_eq!(p.node_address(), 0x29);
    let ev = p.on_usb_packet(&[0x60, 0x04, 0x41, 0x01, 0x01, 0x04, 0x61], ms(0));
    assert_eq!(ev, vec![Event::NodeAddress(0x04), Event::Other(vec![0x60, 0x04, 0x41, 0x01, 0x01, 0x04, 0x61])]);
    assert_eq!(p.node_address(), 0x04);
}
