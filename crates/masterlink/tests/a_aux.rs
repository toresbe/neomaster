//! A.AUX on the link: which ML source it is per role (BeoIpcML.exe, see `sources.rs`), and a
//! replay of the source center captures with A.AUX playing locally. In the source center role
//! A.AUX rides on the N.MUSIC slot, so the link must see exactly what it saw from the original
//! software playing N.MUSIC.

mod common;

use common::*;
use masterlink::protocol::clock::LocalTime;
use masterlink::protocol::source as ml;
use masterlink::{Host, LinkRole, LocalSource, SourceCenter, SourceMap, Telegram, Time};

#[allow(dead_code)]
mod vectors {
    use super::common::Row;
    include!("vectors/source_center_live.rs");
}
use vectors::*;

#[test]
fn audio_master_role_uses_the_a_aux_source() {
    let map = SourceMap::new(LinkRole::AudioMaster);
    assert_eq!(map.ml_source(LocalSource::AAux), 0x97);
    assert_eq!(map.beo4_key(LocalSource::AAux), 0x83);
    assert_eq!(map.local_source(0x97, None), Some(LocalSource::AAux));
    // N.MUSIC stays N.MUSIC even while A.AUX plays: they are separate sources here
    assert_eq!(map.local_source(0x7a, Some(LocalSource::AAux)), Some(LocalSource::NMusic));
}

#[test]
fn source_center_role_carries_a_aux_as_n_music() {
    let map = SourceMap::new(LinkRole::SourceCenter);
    assert_eq!(map.ml_source(LocalSource::AAux), ml::N_MUSIC);
    assert_eq!(map.beo4_key(LocalSource::AAux), 0x94);
    assert_eq!(map.local_source(0x7a, Some(LocalSource::AAux)), Some(LocalSource::AAux));
    assert_eq!(map.local_source(0x7a, Some(LocalSource::NRadio)), Some(LocalSource::NMusic));
    assert_eq!(map.local_source(0x7a, None), Some(LocalSource::NMusic));
    // no A.AUX device on the link in this role
    assert_eq!(map.local_source(0x97, Some(LocalSource::AAux)), None);
}

#[test]
fn moved_n_music_slot_carries_a_aux_too() {
    // MLConfig.dll can put the N.MUSIC slot on A.MEM (0x79)
    let map = SourceMap { n_music: ml::A_MEM, ..SourceMap::new(LinkRole::SourceCenter) };
    assert_eq!(map.ml_source(LocalSource::AAux), 0x79);
    assert_eq!(map.local_source(0x79, Some(LocalSource::AAux)), Some(LocalSource::AAux));
    assert_eq!(map.local_source(0x7a, Some(LocalSource::AAux)), None);
}

#[test]
fn role_from_system_configuration() {
    assert_eq!(LinkRole::from_system_configuration(7), LinkRole::SourceCenter);
    for c in [4, 5, 6, 9, 10] {
        assert_eq!(LinkRole::from_system_configuration(c), LinkRole::AudioMaster);
    }
}

/// FakeHost, plus the local source each link request resolved to.
struct MapHost {
    inner: FakeHost,
    map: SourceMap,
    active: Option<LocalSource>,
    played: Vec<LocalSource>,
}

impl Host for MapHost {
    fn send_telegram(&mut self, t: Telegram) {
        self.inner.send_telegram(t)
    }
    fn set_ml_distribution(&mut self, on: bool) {
        self.inner.set_ml_distribution(on)
    }
    fn set_ml_listening(&mut self, on: bool) {
        self.inner.set_ml_listening(on)
    }
    fn start_source(&mut self, src: u8) -> bool {
        self.inner.start_source(src);
        match self.map.local_source(src, self.active) {
            Some(local) => {
                self.active = Some(local);
                self.played.push(local);
                true
            }
            None => false,
        }
    }
    fn stop_source(&mut self) {
        self.inner.stop_source()
    }
    fn link_standby(&mut self) {
        self.inner.link_standby()
    }
    fn local_time(&mut self) -> LocalTime {
        self.inner.local_time()
    }
}

fn at(t: f64) -> Time {
    at_offset(t, 100_000)
}

fn replay(sc: &mut SourceCenter, host: &mut MapHost, w: &[Row]) -> Vec<Bytes> {
    let mut expected = Vec::new();
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

fn a_aux_host() -> MapHost {
    MapHost {
        inner: FakeHost { mlgw: true, ..Default::default() },
        map: SourceMap::new(LinkRole::SourceCenter),
        active: Some(LocalSource::AAux),
        played: Vec::new(),
    }
}

/// A.AUX plays on the panel; the kitchen asks for N.MUSIC, then N.RADIO, then the master's CD.
/// The link sees the original software's N.MUSIC answers byte for byte; locally A.AUX keeps
/// playing until N.RADIO is asked for.
#[test]
fn replays_kitchen_source_changes_with_a_aux_playing() {
    let mut host = a_aux_host();
    let mut sc = SourceCenter::new();
    let expected = replay(&mut sc, &mut host, LIVE_KITCHEN_SOURCES);
    assert_eq!(diff(&expected, &host.inner.sent), "");
    assert_eq!(host.inner.started, vec![0x7a, 0xa1]);
    assert_eq!(host.played, vec![LocalSource::AAux, LocalSource::NRadio]);
}

/// The kitchen joins while A.AUX plays as N.MUSIC; then "all standby" (capture §6).
#[test]
fn replays_kitchen_join_and_all_standby_with_a_aux_playing() {
    let mut host = a_aux_host();
    let mut sc = SourceCenter::new();
    let req = incoming(&[
        0xc2, 0xc1, 0x01, 0x0b, 0x7a, 0x00, 0x00, 0x6c, 0x0a, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x01, 0xff, 0x02, 0x00,
    ]);
    sc.handle(&mut host, &req, at(-50.0));
    host.inner.sent.clear();
    let expected = replay(&mut sc, &mut host, LIVE_KITCHEN_AND_STANDBY);
    assert_eq!(diff(&expected, &host.inner.sent), "");
    assert_eq!(host.played, vec![LocalSource::AAux]);
    assert!(host.inner.standby_seen);
}
