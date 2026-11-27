//! Which Masterlink source a local BeoMaster 5 source is, per link role.
//!
//! Decompiled from BeoIpcML.exe 7.04 (annotated in the Ghidra project, see
//! `work/ghidra-annotations/beoipcml_aaux.tsv`):
//!
//! - `TPCsources_Create` (0x0422ecb0) makes one link device per local source. Their ML source
//!   and Beo4 key: N.MUSIC 0x7a / 0x94 (`TPCnmusicDevice_Create`), N.RADIO 0xa1 / 0x93,
//!   A.AUX 0x97 / 0x83 (`TPCauxDevice_Create`, 0x0422e544).
//! - `TPcAuxProxy_OnIpcActive` (0x0424f6f4) runs when BeoSound5.exe turns A.AUX on. In
//!   SystemConfiguration 7 (AUDIO MASTER off: the BM5 is a source center for another product's
//!   audio master) it does not use the A.AUX device at all: it switches the N.MUSIC device on,
//!   or leaves it on ("ML-wise Nmusic is active. IPC-wise PCAux is active"). A.AUX reaches the
//!   link as N.MUSIC. In every other configuration it switches the A.AUX device (0x97) on.
//!   `TPcMServiceProxy_OnIpcActive` applies the same rule to the music service.
//!
//! The N.MUSIC device's address can be moved by MLConfig.dll (A.MEM 0x79, CD 0x8d; HKCU
//! `BeoLink PC\<Slot>DefaultAddress`), so the slot address is a field, not a constant.

use crate::protocol::source as ml;

/// The sources a BeoMaster 5 plays itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocalSource {
    NMusic,
    NRadio,
    /// A.AUX: a USB sound card's line in, played through the BM5 (see the `aux` crate).
    AAux,
}

/// How the BM5 sits on the link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkRole {
    /// AUDIO MASTER on (SystemConfiguration 4, 5, 6, 9, 10): [`crate::AudioMaster`].
    AudioMaster,
    /// AUDIO MASTER off (SystemConfiguration 7): [`crate::SourceCenter`] for another master.
    SourceCenter,
}

impl LinkRole {
    /// From B&O's SystemConfiguration byte (MLConfig.dll: 7 when AUDIO MASTER is off).
    pub fn from_system_configuration(config: u8) -> Self {
        if config == 7 {
            LinkRole::SourceCenter
        } else {
            LinkRole::AudioMaster
        }
    }
}

/// Beo4 keys of the local sources (`pc2::beo4` has the same values).
pub mod beo4 {
    pub const N_MUSIC: u8 = 0x94;
    pub const N_RADIO: u8 = 0x93;
    pub const A_AUX: u8 = 0x83;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceMap {
    pub role: LinkRole,
    /// The N.MUSIC slot's ML address (0x7a unless MLConfig moved it to A.MEM or CD).
    pub n_music: u8,
    /// The N.RADIO slot's ML address (0xa1 unless moved to RADIO 0x6f).
    pub n_radio: u8,
}

impl SourceMap {
    pub fn new(role: LinkRole) -> Self {
        SourceMap { role, n_music: ml::N_MUSIC, n_radio: ml::N_RADIO }
    }

    /// The ML source a local source is announced and distributed as.
    pub fn ml_source(&self, local: LocalSource) -> u8 {
        match (local, self.role) {
            (LocalSource::NMusic, _) => self.n_music,
            (LocalSource::NRadio, _) => self.n_radio,
            (LocalSource::AAux, LinkRole::AudioMaster) => ml::A_AUX,
            (LocalSource::AAux, LinkRole::SourceCenter) => self.n_music,
        }
    }

    /// The Beo4 key that selects a local source, as the BM5 sends it to the room's master in
    /// the source center role (A.AUX rides on N.MUSIC there, so it is the N.MUSIC key).
    pub fn beo4_key(&self, local: LocalSource) -> u8 {
        match (local, self.role) {
            (LocalSource::NMusic, _) | (LocalSource::AAux, LinkRole::SourceCenter) => beo4::N_MUSIC,
            (LocalSource::NRadio, _) => beo4::N_RADIO,
            (LocalSource::AAux, LinkRole::AudioMaster) => beo4::A_AUX,
        }
    }

    /// A link room asked for ML source `src` (`Host::start_source`). Which local source plays?
    /// `active` is what plays locally now: when the N.MUSIC slot carries A.AUX (source center
    /// role) a request for it keeps A.AUX playing, as BeoIpcML does.
    pub fn local_source(&self, src: u8, active: Option<LocalSource>) -> Option<LocalSource> {
        if src == self.n_music {
            if self.role == LinkRole::SourceCenter && active == Some(LocalSource::AAux) {
                return Some(LocalSource::AAux);
            }
            return Some(LocalSource::NMusic);
        }
        if src == self.n_radio {
            return Some(LocalSource::NRadio);
        }
        if src == ml::A_AUX && self.role == LinkRole::AudioMaster {
            return Some(LocalSource::AAux);
        }
        None
    }
}
