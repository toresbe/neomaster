//! A captured metadata pipe (Music Assistant's AirPlay sender to shairport-sync 3.3.8, bookworm
//! i386; cover art swapped for a small test JPEG): play, next, pause, play, stop.

use std::time::Duration;

use airplay::{cover, Event, Parser, Session, Track};

const CAPTURE: &str = include_str!("data/ma_session.txt");

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

#[test]
fn ma_session_replays_to_the_expected_events() {
    let (items, bad) = Parser::new().feed(CAPTURE.as_bytes());
    assert!(bad.is_empty(), "{bad:?}");
    assert_eq!(items.len(), 41);

    let mut s = Session::new();
    let mut events = Vec::new();
    for item in &items {
        events.extend(s.item(item, secs(10)));
    }
    let names: Vec<String> = events
        .iter()
        .map(|e| match e {
            Event::Started(who) => format!("Started({})", who.label()),
            Event::Track(t) => format!("Track({})", t.title),
            Event::Cover(c) => format!("Cover({})", c.len()),
            e => format!("{e:?}"),
        })
        .collect();
    assert_eq!(
        names,
        [
            "Started(192.168.2.238)",
            // the first flush comes before anything plays
            "Paused",
            "Track(All of Me)",
            "Cover(374)",
            "Resumed",
            "Track(All In Love Is Fair)",
            "Cover(374)",
            "Paused",
            "Resumed",
            "Paused",
        ]
    );
    assert_eq!(
        s.track(),
        &Track {
            title: "All In Love Is Fair".into(),
            artist: "Stevie Wonder".into(),
            album: "Innervisions (Reissue)".into(),
            genre: String::new(),
        }
    );
    // pend: still active through the grace period, then released
    assert!(s.is_active());
    assert_eq!(s.next_deadline(), Some(secs(10) + Session::END_GRACE));
    assert!(s.tick(secs(12)).is_empty());
    assert_eq!(s.tick(secs(13)), vec![Event::Ended]);
    assert!(!s.is_active());
    assert_eq!(s.next_deadline(), None);
}

#[test]
fn a_new_session_within_the_grace_period_keeps_the_source() {
    let (items, _) = Parser::new().feed(CAPTURE.as_bytes());
    let mut s = Session::new();
    for item in &items {
        s.item(item, secs(10));
    }
    let ev = s.item(&airplay::Item::new(b"ssnc", b"pbeg", vec![]), secs(11));
    assert!(ev.is_empty(), "no second Started: {ev:?}");
    assert!(s.tick(secs(20)).is_empty());
    assert!(s.is_active());
}

#[test]
fn a_pipe_opened_mid_session_starts_one() {
    let mut s = Session::new();
    let ev = s.item(&airplay::Item::new(b"ssnc", b"prsm", vec![]), secs(1));
    assert!(matches!(ev[..], [Event::Started(_)]), "{ev:?}");
    assert_eq!(s.receiver_restarted(), vec![Event::Ended]);
    assert!(s.receiver_restarted().is_empty());
}

#[test]
fn captured_cover_decodes() {
    let jpeg = include_bytes!("data/cover.jpg");
    let img = cover::decode_square(jpeg, 32).unwrap();
    assert_eq!((img.width, img.height, img.data.len()), (32, 32, 32 * 32 * 3));
    // gradient from red (top) to blue (bottom)
    let px = |y: usize| &img.data[(y * 32 + 16) * 3..][..3];
    assert!(px(0)[0] > px(0)[2] && px(31)[2] > px(31)[0], "{:?} {:?}", px(0), px(31));
}
