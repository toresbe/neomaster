//! neomaster's settings: everything that used to live in environment variables and constants,
//! in one typed, versioned file that is saved atomically.
//!
//! - [`Settings`] is the whole configuration. It reads and writes TOML; every field has the
//!   stock default, so an empty or missing file is a valid configuration and a file only needs
//!   the lines that differ.
//! - [`store`] finds the file, loads it (migrating older versions), and saves it atomically
//!   (temporary file, fsync, rename, fsync of the directory).
//! - [`env`] applies the old environment variables on top, for this run only.
//! - [`roles`] is AUDIO MASTER, the AM option and the source-name slots, with MLConfig.dll's
//!   save/restore and uniqueness rules ([`names`]).
//! - [`menu`] is the stock SETTINGS menu as data: sections, items, choices, and
//!   [`Settings::apply`] for a choice made on the panel. The `ui` crate draws it.
//!
//! Nothing here touches a device or blocks except [`store`]'s file IO.

pub mod env;
pub mod menu;
pub mod names;
pub mod roles;
pub mod store;

use std::time::Duration;

use masterlink::protocol::node;
use serde::{Deserialize, Serialize};

pub use names::{SlotNames, SourceName};
pub use roles::Roles;

/// The file format version this build writes. [`store::load`] migrates older ones.
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// File format version ([`VERSION`]).
    pub version: u32,
    pub roles: Roles,
    pub system: System,
    pub idle: Idle,
    pub service: Service,
    pub network: Network,
    pub music_assistant: MusicAssistant,
    pub aux: Aux,
    pub link: Link,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: VERSION,
            roles: Roles::default(),
            system: System::default(),
            idle: Idle::default(),
            service: Service::default(),
            network: Network::default(),
            music_assistant: MusicAssistant::default(),
            aux: Aux::default(),
            link: Link::default(),
        }
    }
}

/// Settings › System settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct System {
    /// LANGUAGE, as the stock picker lists it (DANSK, DEUTSCH, ENGLISH, ...). The panel is English
    /// only for now; this is stored and shown.
    pub language: String,
    /// TIME ZONE, an IANA name. Stored and shown; the host's own zone is not changed.
    pub time_zone: String,
    /// IR TIMEOUT (hidden menu), 2-10, stock 6 (`SourceCenterIRTimeout`).
    pub ir_timeout: u8,
    /// POWER SAVING (hidden menu): AUTOMATIC or OFF.
    pub power_saving: PowerSaving,
    /// DEBUG LOGGING (hidden menu): raises the log level to debug while on.
    pub debug_logging: bool,
}

impl Default for System {
    fn default() -> Self {
        System {
            language: "ENGLISH".into(),
            time_zone: "Europe/Copenhagen".into(),
            ir_timeout: 6,
            power_saving: PowerSaving::Automatic,
            debug_logging: false,
        }
    }
}

pub const IR_TIMEOUT_RANGE: std::ops::RangeInclusive<u8> = 2..=10;
pub const LANGUAGES: [&str; 7] = ["DANSK", "DEUTSCH", "ENGLISH", "ESPAÑOL", "FRANÇAIS", "ITALIANO", "NEDERLANDS"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerSaving {
    #[default]
    Automatic,
    Off,
}

/// The idle and blank-screen timers (BeoSound5.exe `A18Controller`; not in the stock menu).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Idle {
    /// Back to now-playing after this long without input. Stock 180 s.
    pub return_after_s: u32,
    /// The standby tick: blank on a tick with nothing playing. Stock 1800 s.
    pub standby_tick_s: u32,
    /// While playing, blank after this many hours without input (`ActivePlaybackTimeOut`,
    /// stock 12).
    pub active_playback_timeout_h: u32,
}

impl Default for Idle {
    fn default() -> Self {
        Idle { return_after_s: 180, standby_tick_s: 1800, active_playback_timeout_h: 12 }
    }
}

impl Idle {
    pub fn return_after(&self) -> Duration {
        Duration::from_secs(self.return_after_s.max(1).into())
    }
    pub fn standby_tick(&self) -> Duration {
        Duration::from_secs(self.standby_tick_s.max(1).into())
    }
    pub fn active_playback_timeout(&self) -> Duration {
        Duration::from_secs(u64::from(self.active_playback_timeout_h.max(1)) * 3600)
    }
}

/// Settings › Service settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Service {
    /// N.RADIO BUFFER, 2-6 (seconds of stream buffer). Captured value 2.
    pub n_radio_buffer: u8,
    /// DISABLE MOTS ("more of the same" queue filling).
    pub disable_mots: bool,
}

impl Default for Service {
    fn default() -> Self {
        Service { n_radio_buffer: 2, disable_mots: false }
    }
}

pub const N_RADIO_BUFFER_RANGE: std::ops::RangeInclusive<u8> = 2..=6;

/// Settings › Network settings (hidden menu). Shown on Network info; the host is not changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Network {
    /// HOST NAME as shown; empty = the machine's own.
    pub host_name: String,
    /// LOCATION, a free label.
    pub location: String,
}

/// The Music Assistant server (N.MUSIC, N.RADIO). Shown as MUSIC SERVER in the hidden menu.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicAssistant {
    /// e.g. "http://192.168.2.238:8095"; empty = not configured.
    pub url: String,
    /// The BM5's player (and queue) id in Music Assistant.
    pub player_id: String,
    /// A file holding the long-lived token. The token itself is never stored in the settings
    /// file, which the panel rewrites; `MA_TOKEN` in the environment also works.
    pub token_file: String,
}

impl MusicAssistant {
    /// The token: `MA_TOKEN` if set, else the first line of `token_file`.
    pub fn token(&self) -> Option<String> {
        if let Ok(t) = std::env::var("MA_TOKEN") {
            return Some(t);
        }
        if self.token_file.is_empty() {
            return None;
        }
        let text = std::fs::read_to_string(&self.token_file).ok()?;
        text.lines().next().map(|l| l.trim().to_string()).filter(|t| !t.is_empty())
    }
}

/// A.AUX: the USB sound card's line in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Aux {
    /// ALSA card id to use (e.g. "CODEC"); empty = the first "USB Audio CODEC", as stock.
    pub card: String,
    /// ALSA playback device the loop plays to (the output the PC2 hears).
    pub playback: String,
    /// Loop latency in microseconds (alsaloop -t).
    pub latency_us: u32,
}

impl Default for Aux {
    fn default() -> Self {
        Aux { card: String::new(), playback: "default".into(), latency_us: 50_000 }
    }
}

/// Masterlink and PC2 details that are not in the stock menu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    /// Where a link source sends its Beo4 keys: the room's master node (0xc0 = V_MASTER).
    pub room_master: u8,
    /// Volume when the audio master turns its speakers on (stock 34).
    pub start_volume: u8,
    pub bass: i8,
    pub treble: i8,
    pub loudness: bool,
}

impl Default for Link {
    fn default() -> Self {
        Link { room_master: node::VIDEO_MASTER, start_volume: 34, bass: -1, treble: 0, loudness: true }
    }
}

impl Settings {
    /// Pull hand-edited values back into range. Returns what was changed, for the log.
    pub fn normalise(&mut self) -> Vec<String> {
        let mut fixed = Vec::new();
        let mut clamp = |name: &str, v: &mut u8, r: std::ops::RangeInclusive<u8>| {
            let c = (*v).clamp(*r.start(), *r.end());
            if c != *v {
                fixed.push(format!("{name} {v} -> {c}"));
                *v = c;
            }
        };
        clamp("system.ir_timeout", &mut self.system.ir_timeout, IR_TIMEOUT_RANGE);
        clamp("service.n_radio_buffer", &mut self.service.n_radio_buffer, N_RADIO_BUFFER_RANGE);
        clamp("link.start_volume", &mut self.link.start_volume, 0..=90);
        fixed.extend(self.roles.normalise());
        fixed
    }

    /// Serialise as the settings file.
    pub fn to_toml(&self) -> String {
        let mut s = String::from(
            "# neomaster settings. The panel's SETTINGS menu rewrites this file; edits made while\n\
             # neomaster runs are read at the next start. Missing keys take the stock defaults.\n\n",
        );
        s.push_str(&toml::to_string_pretty(self).expect("settings serialise"));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_the_defaults() {
        let s: Settings = toml::from_str("").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn round_trips() {
        let mut s = Settings::default();
        s.roles.set_audio_master(true);
        s.music_assistant.url = "http://ma:8095".into();
        s.aux.card = "CODEC".into();
        let back: Settings = toml::from_str(&s.to_toml()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let s: Settings = toml::from_str("[system]\nir_timeout = 8\n").unwrap();
        assert_eq!(s.system.ir_timeout, 8);
        assert_eq!(s.system.language, "ENGLISH");
        assert_eq!(s.idle, Idle::default());
    }

    #[test]
    fn normalise_clamps() {
        let mut s = Settings::default();
        s.system.ir_timeout = 40;
        s.service.n_radio_buffer = 0;
        assert_eq!(s.normalise().len(), 2);
        assert_eq!((s.system.ir_timeout, s.service.n_radio_buffer), (10, 2));
    }
}
