//! The environment variables neomaster read before it had a settings file. They still work and
//! win over the file, but only for the run: [`Overrides`] remembers what they replaced so the
//! panel never saves an environment value into the file.
//!
//! | Variable | Setting |
//! |---|---|
//! | `NEOMASTER_ML_ROLE` (audio-master / link-source) | `roles.audio_master` |
//! | `NEOMASTER_AUX_CARD` | `aux.card` |
//! | `NEOMASTER_AUX_PLAYBACK` | `aux.playback` |
//! | `NEOMASTER_AUX_LATENCY_US` | `aux.latency_us` |
//! | `MA_URL` | `music_assistant.url` |
//! | `NEOMASTER_MA_PLAYER` | `music_assistant.player_id` |
//!
//! `MA_TOKEN` is read where the token is needed ([`crate::MusicAssistant::token`]).

use crate::Settings;

/// Parse a role name as `--role` and `NEOMASTER_ML_ROLE` take it: true for audio master.
pub fn parse_role(s: &str) -> Option<bool> {
    match s {
        "audio-master" | "audio_master" | "am" => Some(true),
        // source_center: the C++ neomaster's NEOMASTER_ML_ROLE value
        "link-source" | "link_source" | "source-center" | "source_center" | "sc" => Some(false),
        _ => None,
    }
}

/// Values the environment (or the command line) put over the file, and what they replaced.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    file: Option<Settings>,
    pub applied: Vec<&'static str>,
}

impl Overrides {
    /// Apply the environment to `s`, using `get` to read variables (`std::env::var` normally).
    pub fn from_env(s: &mut Settings, get: impl Fn(&str) -> Option<String>) -> Overrides {
        let file = s.clone();
        let mut applied = Vec::new();
        if let Some(on) = get("NEOMASTER_ML_ROLE").and_then(|r| parse_role(&r)) {
            s.roles.set_audio_master(on);
            applied.push("NEOMASTER_ML_ROLE");
        }
        let mut set = |var: &'static str, field: &mut String| {
            if let Some(v) = get(var) {
                *field = v;
                applied.push(var);
            }
        };
        set("NEOMASTER_AUX_CARD", &mut s.aux.card);
        set("NEOMASTER_AUX_PLAYBACK", &mut s.aux.playback);
        set("MA_URL", &mut s.music_assistant.url);
        set("NEOMASTER_MA_PLAYER", &mut s.music_assistant.player_id);
        if let Some(t) = get("NEOMASTER_AUX_LATENCY_US").and_then(|t| t.parse().ok()) {
            s.aux.latency_us = t;
            applied.push("NEOMASTER_AUX_LATENCY_US");
        }
        Overrides { file: Some(file), applied }
    }

    /// `--role` on the command line: the same as `NEOMASTER_ML_ROLE`, and wins over it.
    pub fn role(&mut self, s: &mut Settings, audio_master: bool) {
        if self.file.is_none() {
            self.file = Some(s.clone());
        }
        s.roles.set_audio_master(audio_master);
        self.applied.push("--role");
    }

    /// What to save: the live settings with every overridden field put back to the file's value,
    /// unless the panel changed that field since (then the panel's choice is saved).
    pub fn for_saving(&self, live: &Settings, at_start: &Settings) -> Settings {
        let Some(file) = &self.file else { return live.clone() };
        let mut out = live.clone();
        if live.roles == at_start.roles {
            out.roles = file.roles.clone();
        }
        if live.aux == at_start.aux {
            out.aux = file.aux.clone();
        }
        if live.music_assistant == at_start.music_assistant {
            out.music_assistant = file.music_assistant.clone();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let v: Vec<(String, String)> = pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        move |k| v.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone())
    }

    #[test]
    fn env_applies_but_is_not_saved() {
        let mut s = Settings::default();
        let o = Overrides::from_env(&mut s, env(&[("NEOMASTER_ML_ROLE", "audio-master"), ("NEOMASTER_AUX_CARD", "CODEC")]));
        assert!(s.roles.audio_master);
        assert_eq!(s.aux.card, "CODEC");
        let at_start = s.clone();
        let mut live = s.clone();
        live.system.ir_timeout = 8;
        let save = o.for_saving(&live, &at_start);
        assert!(!save.roles.audio_master);
        assert_eq!(save.aux.card, "");
        assert_eq!(save.system.ir_timeout, 8);
    }

    #[test]
    fn a_panel_change_to_an_overridden_field_is_saved() {
        let mut s = Settings::default();
        let o = Overrides::from_env(&mut s, env(&[("NEOMASTER_ML_ROLE", "am")]));
        let at_start = s.clone();
        let mut live = s.clone();
        live.roles.am_option = 2;
        assert_eq!(o.for_saving(&live, &at_start).roles.am_option, 2);
        assert!(o.for_saving(&live, &at_start).roles.audio_master);
    }
}
