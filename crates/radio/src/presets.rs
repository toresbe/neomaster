//! The numbered preset list.

use serde::{Deserialize, Serialize};

/// The highest preset number. The stock N.RADIO capped each favourite (colour) list at
/// `MaxItemsInColorPlaylist` = 99 items, and two Beo4 digits reach 99.
pub const MAX_NUMBER: u8 = 99;

/// One numbered station.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preset {
    /// 1..=[`MAX_NUMBER`]: the channel number a Beo4 or a link room asks for.
    pub number: u8,
    /// The station's name, shown on the panel and to link rooms. With no `uri`, it is also what
    /// Music Assistant is searched for (see [`crate::ma::resolve`]).
    pub name: String,
    /// The Music Assistant URI to play, e.g. `library://radio/12` or
    /// `radiobrowser://radio/<uuid>`. Empty until resolved from `name`.
    pub uri: String,
}

impl Preset {
    pub fn new(number: u8, name: impl Into<String>, uri: impl Into<String>) -> Self {
        Preset { number, name: name.into(), uri: uri.into() }
    }

    /// What to show for it: the name, else the URI.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.uri
        } else {
            &self.name
        }
    }
}

/// The N.RADIO presets, kept sorted by number with no duplicates. Serialises as
///
/// ```toml
/// last = 8
///
/// [[preset]]
/// number = 8
/// name = "Deutschlandfunk"
/// uri = "library://radio/3"
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Presets {
    /// The preset played last (0: none yet). N.RADIO resumes it when picked without a number.
    pub last: u8,
    #[serde(rename = "preset")]
    pub presets: Vec<Preset>,
}

impl Presets {
    pub fn get(&self, number: u8) -> Option<&Preset> {
        self.presets.binary_search_by_key(&number, |p| p.number).ok().map(|i| &self.presets[i])
    }

    /// Add or replace preset `p.number`. Returns false (and changes nothing) for a number
    /// outside 1..=[`MAX_NUMBER`].
    pub fn set(&mut self, p: Preset) -> bool {
        if !(1..=MAX_NUMBER).contains(&p.number) {
            return false;
        }
        match self.presets.binary_search_by_key(&p.number, |x| x.number) {
            Ok(i) => self.presets[i] = p,
            Err(i) => self.presets.insert(i, p),
        }
        true
    }

    pub fn remove(&mut self, number: u8) -> Option<Preset> {
        let i = self.presets.binary_search_by_key(&number, |p| p.number).ok()?;
        Some(self.presets.remove(i))
    }

    /// Fill in the URI a name resolved to. False if the preset is gone or already has one.
    pub fn set_uri(&mut self, number: u8, uri: &str) -> bool {
        match self.presets.binary_search_by_key(&number, |p| p.number) {
            Ok(i) if self.presets[i].uri.is_empty() => {
                self.presets[i].uri = uri.to_string();
                true
            }
            _ => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Preset> {
        self.presets.iter()
    }

    /// The next preset after `from` (`step` 1) or before it (`step` -1), wrapping around; the
    /// first or last one when `from` is not a preset.
    pub fn step(&self, from: u8, step: i8) -> Option<&Preset> {
        let (first, last) = (self.presets.first()?, self.presets.last()?);
        Some(if step >= 0 {
            self.presets.iter().find(|p| p.number > from).unwrap_or(first)
        } else {
            self.presets.iter().rev().find(|p| p.number < from).unwrap_or(last)
        })
    }

    /// Whether any preset number starts with the digits typed so far and is longer, i.e. a
    /// Beo4 digit entry of `prefix` could still go on (1 → 10..19).
    pub fn has_longer(&self, prefix: u8) -> bool {
        prefix != 0 && self.presets.iter().any(|p| p.number / 10 == prefix)
    }

    /// Sort, drop numbers out of range and later duplicates. Returns what was dropped, for the
    /// log. A hand-edited file goes through this on load.
    pub fn normalise(&mut self) -> Vec<String> {
        let mut dropped = Vec::new();
        self.presets.sort_by_key(|p| p.number);
        let mut seen = 0u8;
        self.presets.retain(|p| {
            let keep = (1..=MAX_NUMBER).contains(&p.number) && p.number != seen;
            if keep {
                seen = p.number;
            } else {
                dropped.push(format!("preset {} ({})", p.number, p.label()));
            }
            keep
        });
        if self.last > MAX_NUMBER {
            self.last = 0;
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Presets {
        let mut p = Presets::default();
        for (n, name) in [(8, "Deutschlandfunk"), (1, "NRK P1"), (12, "Radio Paradise")] {
            assert!(p.set(Preset::new(n, name, "")));
        }
        p
    }

    #[test]
    fn kept_sorted_and_replaced_by_number() {
        let mut p = sample();
        assert_eq!(p.iter().map(|p| p.number).collect::<Vec<_>>(), [1, 8, 12]);
        p.set(Preset::new(8, "DLF Kultur", "x"));
        assert_eq!(p.get(8).unwrap().name, "DLF Kultur");
        assert_eq!(p.presets.len(), 3);
        assert!(!p.set(Preset::new(0, "zero", "")));
        assert!(!p.set(Preset::new(100, "hundred", "")));
        assert_eq!(p.remove(1).unwrap().name, "NRK P1");
        assert!(p.get(1).is_none());
    }

    #[test]
    fn steps_wrap_around() {
        let p = sample();
        assert_eq!(p.step(1, 1).unwrap().number, 8);
        assert_eq!(p.step(12, 1).unwrap().number, 1);
        assert_eq!(p.step(1, -1).unwrap().number, 12);
        assert_eq!(p.step(5, -1).unwrap().number, 1);
        assert_eq!(p.step(0, 1).unwrap().number, 1);
        assert!(Presets::default().step(1, 1).is_none());
    }

    #[test]
    fn longer_numbers_after_a_prefix() {
        let p = sample();
        assert!(p.has_longer(1)); // 12
        assert!(!p.has_longer(8));
        assert!(!p.has_longer(0));
    }

    #[test]
    fn toml_round_trip_and_normalise() {
        let text = r#"
last = 8
[[preset]]
number = 12
name = "Radio Paradise"
uri = "radioparadise://radio/0"
[[preset]]
number = 8
name = "Deutschlandfunk"
[[preset]]
number = 8
name = "duplicate"
[[preset]]
number = 0
name = "out of range"
"#;
        let mut p: Presets = toml::from_str(text).unwrap();
        let dropped = p.normalise();
        assert_eq!(dropped.len(), 2, "{dropped:?}");
        assert_eq!(p.get(8).unwrap().name, "Deutschlandfunk");
        assert_eq!(p.get(8).unwrap().uri, "");
        let back: Presets = toml::from_str(&toml::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.last, 8);
    }
}
