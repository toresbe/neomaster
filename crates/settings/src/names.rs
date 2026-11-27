//! Source names and the Roles rules, as MLConfig.dll (the ConfigurationServer plugin of the stock
//! software) applies them. Decompiled 27 Nov 2026; the function names and addresses below are
//! annotated in the Ghidra project `work/ghidra-proj/bs5`, and the whole rule set is written up
//! in "BM5 source names and Roles: rules for neomaster".
//!
//! The BM5 has four renameable name slots. A slot's name fixes the Masterlink source it is
//! announced as and the Beo4 key that selects it, so only the names are stored and the
//! addresses are derived ([`SourceName::ml_source`], [`SourceName::beo4_key`]).
//!
//! - AUDIO MASTER off (SystemConfiguration 7): the names are saved, then forced to the link
//!   defaults (`FUN_10004a6c`).
//! - AUDIO MASTER on: the saved names come back and SystemConfiguration follows the AM option
//!   (`FUN_10003bd5`).
//! - The three "music" slots (N.MUSIC, PC CD, music service) never share a name: setting one
//!   gives the slot that held it the name that was freed (`FUN_10004030`; the exact swap per
//!   case is inferred, see the rules doc).

use masterlink::protocol::source as ml;
use serde::{Deserialize, Serialize};

/// B&O's `enum SOURCE_NAME`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceName {
    #[serde(rename = "A.MEM")]
    AMem,
    #[serde(rename = "CD")]
    Cd,
    #[serde(rename = "N.MUSIC")]
    NMusic,
    #[serde(rename = "RADIO")]
    Radio,
    #[serde(rename = "N.RADIO")]
    NRadio,
}

impl SourceName {
    pub const ALL: [SourceName; 5] =
        [SourceName::AMem, SourceName::Cd, SourceName::NMusic, SourceName::Radio, SourceName::NRadio];

    /// The label on the panel and on link rooms' displays.
    pub fn label(self) -> &'static str {
        match self {
            SourceName::AMem => "A.MEM",
            SourceName::Cd => "CD",
            SourceName::NMusic => "N.MUSIC",
            SourceName::Radio => "RADIO",
            SourceName::NRadio => "N.RADIO",
        }
    }

    pub fn from_label(label: &str) -> Option<SourceName> {
        SourceName::ALL.into_iter().find(|n| n.label() == label)
    }

    /// The Masterlink source the slot is announced as (`<Slot>DefaultAddress`).
    pub fn ml_source(self) -> u8 {
        match self {
            SourceName::AMem => ml::A_MEM,
            SourceName::Cd => ml::CD,
            SourceName::NMusic => ml::N_MUSIC,
            SourceName::Radio => ml::RADIO,
            SourceName::NRadio => ml::N_RADIO,
        }
    }

    /// The Beo4 key that selects it (`<Slot>DefaultLSAddress`).
    pub fn beo4_key(self) -> u8 {
        match self {
            SourceName::AMem => 0x91,
            SourceName::Cd => 0x92,
            SourceName::NMusic => 0x94,
            SourceName::Radio => 0x81,
            SourceName::NRadio => 0x93,
        }
    }
}

/// The four name slots (MLConfig keys 0xa8-0xab).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    /// 0xa9: the N.MUSIC player (Music Assistant library here).
    NMusic,
    /// 0xaa: the N.RADIO player.
    NRadio,
    /// 0xab: PC CD (CD play), only with the B&O USB CD drive.
    CdPlay,
    /// 0xa8: the music service (Spotify on the stock software).
    MusicService,
}

/// The names the three "music" slots may take (MLConfig's table at 0x10070608).
pub const MUSIC_NAMES: [SourceName; 3] = [SourceName::AMem, SourceName::Cd, SourceName::NMusic];
/// The names the N.RADIO slot may take.
pub const RADIO_NAMES: [SourceName; 2] = [SourceName::Radio, SourceName::NRadio];

impl Slot {
    pub const MUSIC: [Slot; 3] = [Slot::NMusic, Slot::CdPlay, Slot::MusicService];

    /// Every name the slot can hold.
    pub fn allowed(self) -> &'static [SourceName] {
        match self {
            Slot::NRadio => &RADIO_NAMES,
            _ => &MUSIC_NAMES,
        }
    }
}

/// The names of the four slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SlotNames {
    pub n_music: SourceName,
    pub n_radio: SourceName,
    pub cd_play: SourceName,
    pub music_service: SourceName,
}

impl Default for SlotNames {
    /// The link defaults (what AUDIO MASTER off forces).
    fn default() -> Self {
        SlotNames {
            n_music: SourceName::NMusic,
            n_radio: SourceName::NRadio,
            cd_play: SourceName::Cd,
            music_service: SourceName::AMem,
        }
    }
}

impl SlotNames {
    pub fn get(&self, slot: Slot) -> SourceName {
        match slot {
            Slot::NMusic => self.n_music,
            Slot::NRadio => self.n_radio,
            Slot::CdPlay => self.cd_play,
            Slot::MusicService => self.music_service,
        }
    }

    fn get_mut(&mut self, slot: Slot) -> &mut SourceName {
        match slot {
            Slot::NMusic => &mut self.n_music,
            Slot::NRadio => &mut self.n_radio,
            Slot::CdPlay => &mut self.cd_play,
            Slot::MusicService => &mut self.music_service,
        }
    }

    /// Name a slot. A name the slot cannot hold is refused (as MLConfig refuses it). If another
    /// music slot already has the name, it gets the name this slot gives up, so the three stay
    /// distinct.
    pub fn set(&mut self, slot: Slot, name: SourceName) -> bool {
        if !slot.allowed().contains(&name) {
            return false;
        }
        let old = self.get(slot);
        if slot != Slot::NRadio {
            if let Some(other) = Slot::MUSIC.into_iter().find(|s| *s != slot && self.get(*s) == name) {
                *self.get_mut(other) = old;
            }
        }
        *self.get_mut(slot) = name;
        true
    }

    /// Repair a hand-edited file: wrong-family names reset to the default, duplicates among the
    /// music slots get a free name.
    pub fn normalise(&mut self) {
        let def = SlotNames::default();
        for slot in [Slot::NMusic, Slot::NRadio, Slot::CdPlay, Slot::MusicService] {
            if !slot.allowed().contains(&self.get(slot)) {
                *self.get_mut(slot) = def.get(slot);
            }
        }
        for (i, slot) in Slot::MUSIC.into_iter().enumerate() {
            let taken: Vec<SourceName> = Slot::MUSIC[..i].iter().map(|s| self.get(*s)).collect();
            if taken.contains(&self.get(slot)) {
                let free = MUSIC_NAMES.into_iter().find(|n| !taken.contains(n)).unwrap_or(def.get(slot));
                *self.get_mut(slot) = free;
            }
        }
    }
}

/// Which slot held N.MUSIC when the master was switched on (`SavedSourceCenterPrimarySourceName`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primary {
    #[default]
    NMusic,
    MusicService,
}

/// The AM options MLConfig knows, and the SystemConfiguration each gives.
pub const AM_OPTIONS: [(u8, u8); 5] = [(0, 5), (1, 4), (2, 6), (5, 9), (6, 10)];

/// SystemConfiguration for AUDIO MASTER on with this AM option (unknown options as option 0).
pub fn system_configuration(audio_master: bool, am_option: u8) -> u8 {
    if !audio_master {
        return 7;
    }
    AM_OPTIONS.iter().find(|(o, _)| *o == am_option).map(|(_, c)| *c).unwrap_or(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_table_matches_mlconfig() {
        let t: Vec<(u8, u8)> = SourceName::ALL.iter().map(|n| (n.ml_source(), n.beo4_key())).collect();
        assert_eq!(t, [(0x79, 0x91), (0x8d, 0x92), (0x7a, 0x94), (0x6f, 0x81), (0xa1, 0x93)]);
    }

    #[test]
    fn music_slots_stay_a_permutation() {
        let mut n = SlotNames::default();
        assert!(n.set(Slot::NMusic, SourceName::AMem));
        assert_eq!((n.n_music, n.music_service, n.cd_play), (SourceName::AMem, SourceName::NMusic, SourceName::Cd));
        assert!(n.set(Slot::CdPlay, SourceName::AMem));
        assert_eq!((n.n_music, n.cd_play), (SourceName::Cd, SourceName::AMem));
        assert!(!n.set(Slot::NMusic, SourceName::Radio));
        assert!(!n.set(Slot::NRadio, SourceName::Cd));
        assert!(n.set(Slot::NRadio, SourceName::Radio));
    }

    #[test]
    fn normalise_repairs_duplicates_and_wrong_families() {
        let mut n = SlotNames {
            n_music: SourceName::Cd,
            n_radio: SourceName::AMem,
            cd_play: SourceName::Cd,
            music_service: SourceName::Cd,
        };
        n.normalise();
        assert_eq!(n.n_radio, SourceName::NRadio);
        let mut m = [n.n_music, n.cd_play, n.music_service].map(|x| x.label());
        m.sort();
        assert_eq!(m, ["A.MEM", "CD", "N.MUSIC"]);
    }

    #[test]
    fn system_configuration_follows_the_am_option() {
        assert_eq!(system_configuration(false, 2), 7);
        assert_eq!([0, 1, 2, 5, 6].map(|o| system_configuration(true, o)), [5, 4, 6, 9, 10]);
    }
}
