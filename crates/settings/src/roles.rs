//! Settings › Roles: AUDIO MASTER, AM OPTION, DELAY and the source-name slots, with
//! MLConfig.dll's rules for switching the master on and off (see [`crate::names`]).

use masterlink::{LinkRole, SourceMap};
use serde::{Deserialize, Serialize};

use crate::names::{self, Primary, Slot, SlotNames, SourceName};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roles {
    /// AUDIO MASTER: the BM5 drives its own speakers and arbitrates the link. Off (the stock
    /// BM5 default) makes it a source center for another product's audio master.
    pub audio_master: bool,
    /// AM OPTION (0, 1, 2, 5 or 6), used while the master is on.
    pub am_option: u8,
    /// DELAY (shown only when not audio master or AM option 0). Its unit and range are not
    /// decoded yet, so it is stored but not offered.
    pub delay: u8,
    /// The slots' names now. While the master is off these are the forced link defaults.
    pub names: SlotNames,
    /// The names chosen as audio master, kept while the master is off
    /// (`BeoPlayer\...\Saved*SourceName`).
    pub saved: SlotNames,
    /// Which slot held N.MUSIC when the master was switched off.
    pub primary: Primary,
}

impl Default for Roles {
    fn default() -> Self {
        Roles {
            audio_master: false,
            am_option: 0,
            delay: 0,
            names: SlotNames::default(),
            saved: SlotNames::default(),
            primary: Primary::NMusic,
        }
    }
}

impl Roles {
    pub fn link_role(&self) -> LinkRole {
        if self.audio_master {
            LinkRole::AudioMaster
        } else {
            LinkRole::SourceCenter
        }
    }

    /// B&O's SystemConfiguration byte.
    pub fn system_configuration(&self) -> u8 {
        names::system_configuration(self.audio_master, self.am_option)
    }

    /// Where the local sources sit on the link.
    pub fn source_map(&self) -> SourceMap {
        let mut m = SourceMap::new(self.link_role());
        m.n_music = self.names.n_music.ml_source();
        m.n_radio = self.names.n_radio.ml_source();
        m
    }

    /// AUDIO MASTER on or off, with MLConfig's save/restore (`FUN_10003caf`, names = true).
    /// Returns false if nothing changed.
    pub fn set_audio_master(&mut self, on: bool) -> bool {
        if on == self.audio_master {
            return false;
        }
        if on {
            // Restore what was chosen as master, in MLConfig's order.
            self.names = self.saved;
            self.names.normalise();
        } else {
            self.saved = self.names;
            self.primary = if self.names.music_service == SourceName::NMusic { Primary::MusicService } else { Primary::NMusic };
            self.names = Self::link_defaults(self.primary);
        }
        self.audio_master = on;
        true
    }

    /// What AUDIO MASTER off forces (`FUN_10004a6c`).
    pub fn link_defaults(primary: Primary) -> SlotNames {
        let (n_music, music_service) = match primary {
            Primary::MusicService => (SourceName::AMem, SourceName::NMusic),
            Primary::NMusic => (SourceName::NMusic, SourceName::AMem),
        };
        SlotNames { n_music, n_radio: SourceName::NRadio, cd_play: SourceName::Cd, music_service }
    }

    /// The names the Roles menu offers for a slot. Not as audio master, N.MUSIC naming only
    /// offers A.MEM and N.MUSIC (BS5 `ConfigReader::GetCurrentRolesMenu`).
    pub fn offered(&self, slot: Slot) -> Vec<SourceName> {
        match (slot, self.audio_master) {
            (Slot::NMusic, false) => vec![SourceName::AMem, SourceName::NMusic],
            _ => slot.allowed().to_vec(),
        }
    }

    /// Name a slot from the menu. The naming items are only in the Roles menu as audio master
    /// (static read of `GetCurrentRolesMenu`; see the rules doc's unknowns), so this refuses
    /// while the master is off, where the names are the forced link defaults.
    pub fn set_name(&mut self, slot: Slot, name: SourceName) -> bool {
        if !self.audio_master || !self.offered(slot).contains(&name) {
            return false;
        }
        self.names.set(slot, name)
    }

    /// The MODE list labels for the local sources, top to bottom, as BS5 builds it
    /// (FUN_005f4fdb): the N.MUSIC slot, the N.RADIO slot, PC CD (only with a CD drive and as
    /// audio master), A.AUX. Spotify is left out: neomaster has no music-service player.
    pub fn source_labels(&self, cd_drive: bool) -> Vec<(&'static str, Slot)> {
        let mut v = vec![(self.names.n_music.label(), Slot::NMusic), (self.names.n_radio.label(), Slot::NRadio)];
        if cd_drive && self.audio_master {
            v.push((self.names.cd_play.label(), Slot::CdPlay));
        }
        v
    }

    pub(crate) fn normalise(&mut self) -> Vec<String> {
        let mut fixed = Vec::new();
        if !names::AM_OPTIONS.iter().any(|(o, _)| *o == self.am_option) {
            fixed.push(format!("roles.am_option {} -> 0", self.am_option));
            self.am_option = 0;
        }
        let before = (self.names, self.saved);
        self.names.normalise();
        self.saved.normalise();
        if !self.audio_master {
            // Not master: the names are the link defaults, whatever the file says.
            self.names = Self::link_defaults(self.primary);
        }
        if (self.names, self.saved) != before {
            fixed.push("roles: source names repaired".into());
        }
        fixed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_off_saves_and_forces_link_defaults() {
        let mut r = Roles { audio_master: true, ..Roles::default() };
        r.set_name(Slot::NMusic, SourceName::AMem); // music service takes N.MUSIC
        r.set_name(Slot::NRadio, SourceName::Radio);
        let chosen = r.names;
        assert!(r.set_audio_master(false));
        assert_eq!(r.system_configuration(), 7);
        assert_eq!(r.primary, Primary::MusicService);
        assert_eq!(
            (r.names.n_music, r.names.music_service, r.names.n_radio),
            (SourceName::AMem, SourceName::NMusic, SourceName::NRadio)
        );
        assert!(r.set_audio_master(true));
        assert_eq!(r.names, chosen);
        assert_eq!(r.system_configuration(), 5);
        assert!(!r.set_audio_master(true));
    }

    #[test]
    fn source_map_follows_the_names() {
        let mut r = Roles { audio_master: true, ..Roles::default() };
        r.set_name(Slot::NMusic, SourceName::Cd);
        r.set_name(Slot::NRadio, SourceName::Radio);
        let m = r.source_map();
        assert_eq!((m.role, m.n_music, m.n_radio), (LinkRole::AudioMaster, 0x8d, 0x6f));
        assert_eq!(r.source_labels(false), vec![("CD", Slot::NMusic), ("RADIO", Slot::NRadio)]);
    }

    #[test]
    fn names_are_only_chosen_as_master() {
        let mut r = Roles::default();
        assert_eq!(r.offered(Slot::NMusic), vec![SourceName::AMem, SourceName::NMusic]);
        assert!(!r.set_name(Slot::NMusic, SourceName::AMem)); // names are forced while not master
        r.set_audio_master(true);
        assert_eq!(r.offered(Slot::NMusic).len(), 3);
        assert!(r.set_name(Slot::NMusic, SourceName::Cd));
    }

    #[test]
    fn cd_only_with_a_drive_as_master() {
        let mut r = Roles::default();
        assert_eq!(r.source_labels(true).len(), 2);
        r.set_audio_master(true);
        assert_eq!(r.source_labels(true).last(), Some(&("CD", Slot::CdPlay)));
    }
}
