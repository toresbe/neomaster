//! A.AUX: device matching (BeoSound5.exe's regex), /proc/asound/cards parsing and the source's
//! start/stop behaviour.
//!
//! `cards_no_codec.txt` is a real /proc/asound/cards (a desktop with an Audient EVO8).
//! `cards_bm5.txt` is the BeoMaster 5 itself (Debian 10, 2026-11-27): its A.AUX input is a
//! built-in TI PCM2902 (USB 08bb:2902) next to the VIA VT1708 onboard audio.

use a_aux::device::{find_usb_audio_codec, is_usb_audio_codec, parse_cards};
use a_aux::loopback::LoopbackConfig;
use a_aux::{AuxSource, Card, Effect};

const PCM2902: &str = include_str!("data/cards_bm5.txt");
const NO_CODEC: &str = include_str!("data/cards_no_codec.txt");

#[test]
fn matches_like_beosound5_regex() {
    // USB\s+AUDIO\s+CODEC, case-insensitive, searched anywhere in the name
    for yes in [
        "USB Audio CODEC",
        "USB Audio CODEC ", // the DirectShow filter name BeoSound5.exe adds
        "usb  audio\tcodec",
        "Burr-Brown from TI USB Audio CODEC at usb-0000:00:10.1-1, full speed",
        "Microphone (USB Audio CODEC)",
    ] {
        assert!(is_usb_audio_codec(yes), "{yes:?}");
    }
    for no in ["USB Audio", "USB AUDIO DEVICE", "USBAudio CODEC", "USB Audio CODE", "EVO8", "HD-Audio Generic"] {
        assert!(!is_usb_audio_codec(no), "{no:?}");
    }
}

#[test]
fn parses_proc_asound_cards() {
    let cards = parse_cards(PCM2902);
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[1].index, 1);
    assert_eq!(cards[1].id, "CODEC");
    assert_eq!(cards[1].driver, "USB-Audio");
    assert_eq!(cards[1].name, "USB Audio CODEC");
    assert!(cards[1].long_name.starts_with("Burr-Brown from TI USB Audio CODEC"));

    let cards = parse_cards(NO_CODEC);
    assert_eq!(cards.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["NVidia", "Generic", "EVO8"]);
    assert_eq!(parse_cards(""), vec![]);
    assert_eq!(parse_cards("--- no soundcards ---\n"), vec![]);
}

#[test]
fn finds_the_codec_card() {
    let card = find_usb_audio_codec(&parse_cards(PCM2902)).unwrap();
    assert_eq!(card.id, "CODEC");
    assert_eq!(card.capture_device(), "plughw:CARD=CODEC,DEV=0");
    assert_eq!(find_usb_audio_codec(&parse_cards(NO_CODEC)), None);
}

fn codec() -> Card {
    find_usb_audio_codec(&parse_cards(PCM2902)).unwrap()
}

fn start() -> Effect {
    Effect::Start { capture: "plughw:CARD=CODEC,DEV=0".into() }
}

#[test]
fn plays_while_selected_and_present() {
    let mut a = AuxSource::new();
    assert_eq!(a.set_card(Some(codec())), vec![]); // plugging in alone does not play (as BS5)
    assert_eq!(a.select(), vec![start()]);
    assert!(a.is_playing());
    assert_eq!(a.select(), vec![]); // already playing
    assert_eq!(a.set_card(Some(codec())), vec![]); // same card polled again
    assert_eq!(a.deselect(), vec![Effect::Stop]);
    assert_eq!(a.deselect(), vec![]);
    assert!(!a.is_playing());
}

#[test]
fn waits_for_the_card_and_follows_replugging() {
    let mut a = AuxSource::new();
    assert_eq!(a.select(), vec![]); // nothing to play yet
    assert!(a.is_selected() && !a.is_playing());
    assert_eq!(a.set_card(Some(codec())), vec![start()]);
    assert_eq!(a.set_card(None), vec![Effect::Stop]); // unplugged
    assert_eq!(a.set_card(Some(codec())), vec![start()]); // back
    let other = Card { index: 2, id: "CODEC_1".into(), ..codec() };
    assert_eq!(a.set_card(Some(other)), vec![Effect::Stop, Effect::Start { capture: "plughw:CARD=CODEC_1,DEV=0".into() }]);
}

#[test]
fn restarts_a_loop_that_died() {
    let mut a = AuxSource::new();
    a.set_card(Some(codec()));
    a.select();
    assert_eq!(a.loop_ended(), vec![start()]);
    a.deselect();
    assert_eq!(a.loop_ended(), vec![]); // nothing was meant to run
}

#[test]
fn alsaloop_arguments() {
    let c = LoopbackConfig { playback: "hw:Audio,0".into(), ..LoopbackConfig::default() };
    assert_eq!(c.args("plughw:CARD=CODEC,DEV=0"), ["-C", "plughw:CARD=CODEC,DEV=0", "-P", "hw:Audio,0", "-t", "50000"]);
}

#[test]
fn loopback_notices_a_loop_that_exits() {
    use a_aux::loopback::Loopback;
    // `true` stands in for an alsaloop that fails at once (e.g. the card vanished)
    let mut l = Loopback::new(LoopbackConfig { program: "true".into(), ..LoopbackConfig::default() });
    assert!(!l.ended());
    l.apply(&start());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !l.ended() {
        assert!(std::time::Instant::now() < deadline, "exit not noticed");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!l.ended()); // reported once
}
