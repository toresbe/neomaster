//! AirPlay on the link: it borrows A.AUX's slot (`AIRPLAY_SLOT`), so per role it is announced
//! exactly as A.AUX is, and a room asking for that slot keeps AirPlay playing.

use masterlink::protocol::source as ml;
use masterlink::{LinkRole, LocalSource, SourceMap, AIRPLAY_SLOT};

#[test]
fn airplay_is_announced_as_its_slot() {
    assert_eq!(AIRPLAY_SLOT, LocalSource::AAux);
    for role in [LinkRole::AudioMaster, LinkRole::SourceCenter] {
        let map = SourceMap::new(role);
        assert_eq!(map.ml_source(LocalSource::AirPlay), map.ml_source(AIRPLAY_SLOT), "{role:?}");
        assert_eq!(map.beo4_key(LocalSource::AirPlay), map.beo4_key(AIRPLAY_SLOT), "{role:?}");
    }
    assert_eq!(SourceMap::new(LinkRole::AudioMaster).ml_source(LocalSource::AirPlay), ml::A_AUX);
    assert_eq!(SourceMap::new(LinkRole::SourceCenter).ml_source(LocalSource::AirPlay), ml::N_MUSIC);
}

#[test]
fn audio_master_room_asking_for_a_aux_keeps_airplay() {
    let map = SourceMap::new(LinkRole::AudioMaster);
    assert_eq!(map.local_source(ml::A_AUX, Some(LocalSource::AirPlay)), Some(LocalSource::AirPlay));
    // other sources are unaffected
    assert_eq!(map.local_source(ml::N_MUSIC, Some(LocalSource::AirPlay)), Some(LocalSource::NMusic));
    assert_eq!(map.local_source(ml::N_RADIO, Some(LocalSource::AirPlay)), Some(LocalSource::NRadio));
    // with AirPlay idle the slot is A.AUX again
    assert_eq!(map.local_source(ml::A_AUX, None), Some(LocalSource::AAux));
    assert_eq!(map.local_source(ml::A_AUX, Some(LocalSource::NMusic)), Some(LocalSource::AAux));
}

#[test]
fn source_center_room_asking_for_n_music_keeps_airplay() {
    let map = SourceMap::new(LinkRole::SourceCenter);
    assert_eq!(map.local_source(ml::N_MUSIC, Some(LocalSource::AirPlay)), Some(LocalSource::AirPlay));
    assert_eq!(map.local_source(ml::N_RADIO, Some(LocalSource::AirPlay)), Some(LocalSource::NRadio));
    assert_eq!(map.local_source(ml::N_MUSIC, None), Some(LocalSource::NMusic));
    assert_eq!(map.local_source(ml::A_AUX, Some(LocalSource::AirPlay)), None);
}
