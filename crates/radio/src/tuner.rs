//! Channel numbers and Beo4 keys to presets, sans-IO.

use std::time::Duration;

use log::{debug, info};

use crate::presets::{Preset, Presets};

/// How long a Beo4 digit waits for a second one before it counts on its own. Only used when a
/// longer preset could still follow (1 then 2 = 12); otherwise a digit plays at once. B&O
/// products wait about this long; BeoNetRadio's own value is not decoded yet.
pub const DIGIT_TIMEOUT: Duration = Duration::from_millis(1500);

// Beo4 codes (pc2::beo4); repeated so this crate does not depend on the PC2 crate.
const DIGIT_9: u8 = 0x09;
const ARROW_UP: u8 = 0x1e;
const ARROW_DOWN: u8 = 0x1f;

/// Play this preset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tune {
    pub preset: Preset,
}

/// Tracks the playing preset and Beo4 digit entry. Feed it what arrives with `now`; each call
/// returns the preset to play, if any.
#[derive(Debug, Default)]
pub struct Tuner {
    presets: Presets,
    /// The number playing (0: none).
    current: u8,
    /// Digits typed so far and when the entry times out.
    entry: Option<(u8, Duration)>,
    /// `last` changed since [`Tuner::take_dirty`].
    dirty: bool,
}

impl Tuner {
    pub fn new(presets: Presets) -> Self {
        Tuner { presets, current: 0, entry: None, dirty: false }
    }

    pub fn presets(&self) -> &Presets {
        &self.presets
    }

    /// Change the list (an edit from the panel or a resolved URI). The playing number stays.
    pub fn presets_mut(&mut self) -> &mut Presets {
        self.dirty = true;
        &mut self.presets
    }

    /// The preset playing, if it is one.
    pub fn current(&self) -> Option<&Preset> {
        self.presets.get(self.current)
    }

    /// True once after the list or the last-played number changed: time to save.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    fn tune(&mut self, number: u8) -> Option<Tune> {
        let Some(p) = self.presets.get(number).cloned() else {
            info!("N.RADIO: no preset {number}");
            return None;
        };
        if number == self.current {
            debug!("N.RADIO: preset {number} again");
        } else {
            info!("N.RADIO: preset {number}, {}", p.label());
        }
        self.current = number;
        if self.presets.last != number {
            self.presets.last = number;
            self.dirty = true;
        }
        Some(Tune { preset: p })
    }

    /// A channel number from a link room (SOURCE_CONNECT, GOTO_SOURCE) or the panel.
    pub fn channel(&mut self, number: u8) -> Option<Tune> {
        self.entry = None;
        self.tune(number)
    }

    /// N.RADIO was picked without a number: the last preset, else the first one.
    pub fn resume(&mut self) -> Option<Tune> {
        self.entry = None;
        let n = match self.presets.get(self.presets.last) {
            Some(p) => p.number,
            None => self.presets.iter().next()?.number,
        };
        self.tune(n)
    }

    /// N.RADIO stopped.
    pub fn stopped(&mut self) {
        self.current = 0;
        self.entry = None;
    }

    /// A Beo4 key while N.RADIO plays (from the local IR eye or a link room's SOURCE_CONTROL).
    /// Digits pick a preset (two digits for 10-99), ▲ ▼ step through them. Other keys: `None`.
    pub fn key(&mut self, code: u8, now: Duration) -> Option<Tune> {
        match code {
            0..=DIGIT_9 => {
                let typed = match self.entry.take() {
                    Some((d, due)) if now < due => d * 10 + code,
                    _ => code,
                };
                if typed < 10 && self.presets.has_longer(typed) {
                    debug!("N.RADIO: digit {typed}, waiting for a second one");
                    self.entry = Some((typed, now + DIGIT_TIMEOUT));
                    return None;
                }
                self.tune(typed)
            }
            ARROW_UP | ARROW_DOWN => {
                self.entry = None;
                let step = if code == ARROW_UP { 1 } else { -1 };
                let n = self.presets.step(self.current, step)?.number;
                self.tune(n)
            }
            _ => None,
        }
    }

    /// When [`Tuner::tick`] needs calling: a single digit waiting for a second one.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.entry.map(|(_, due)| due)
    }

    pub fn tick(&mut self, now: Duration) -> Option<Tune> {
        match self.entry {
            Some((d, due)) if now >= due => {
                self.entry = None;
                self.tune(d)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    fn tuner() -> Tuner {
        let mut p = Presets::default();
        for (n, name) in [(1, "NRK P1"), (2, "P2"), (8, "Deutschlandfunk"), (12, "Radio Paradise")] {
            p.set(Preset::new(n, name, format!("library://radio/{n}")));
        }
        Tuner::new(p)
    }

    fn num(t: Option<Tune>) -> Option<u8> {
        t.map(|t| t.preset.number)
    }

    #[test]
    fn link_channel_plays_that_preset() {
        let mut t = tuner();
        // this morning's wake-up timer: SOURCE_CONNECT channel 8
        assert_eq!(t.channel(8).unwrap().preset.name, "Deutschlandfunk");
        assert_eq!(t.current().unwrap().number, 8);
        assert!(t.take_dirty(), "last changed");
        assert!(!t.take_dirty());
        assert!(t.channel(9).is_none(), "no preset 9");
        assert_eq!(t.current().unwrap().number, 8, "a missing preset keeps the station");
    }

    #[test]
    fn a_digit_without_longer_presets_plays_at_once() {
        let mut t = tuner();
        assert_eq!(num(t.key(8, MS(0))), Some(8));
        assert_eq!(num(t.key(2, MS(100))), Some(2), "2: no 20-29");
    }

    #[test]
    fn two_digits_within_the_timeout() {
        let mut t = tuner();
        assert_eq!(t.key(1, MS(0)), None, "1 could become 12");
        assert_eq!(t.next_deadline(), Some(DIGIT_TIMEOUT));
        assert_eq!(num(t.key(2, MS(800))), Some(12));
        assert_eq!(t.next_deadline(), None);
    }

    #[test]
    fn a_lone_digit_plays_when_the_timeout_runs_out() {
        let mut t = tuner();
        assert_eq!(t.key(1, MS(0)), None);
        assert_eq!(t.tick(MS(1000)), None);
        assert_eq!(num(t.tick(DIGIT_TIMEOUT)), Some(1));
        // and a digit after the timeout starts a new entry
        assert_eq!(t.key(1, MS(5000)), None);
        assert_eq!(num(t.key(8, MS(5000) + DIGIT_TIMEOUT)), Some(8));
    }

    #[test]
    fn arrows_step_and_wrap() {
        let mut t = tuner();
        t.channel(8);
        assert_eq!(num(t.key(ARROW_UP, MS(0))), Some(12));
        assert_eq!(num(t.key(ARROW_UP, MS(0))), Some(1));
        assert_eq!(num(t.key(ARROW_DOWN, MS(0))), Some(12));
        assert_eq!(t.key(0x35, MS(0)), None, "GO is not ours");
    }

    #[test]
    fn resume_plays_the_last_preset_or_the_first() {
        let mut t = tuner();
        assert_eq!(num(t.resume()), Some(1));
        t.channel(12);
        t.stopped();
        assert!(t.current().is_none());
        assert_eq!(num(t.resume()), Some(12));
        assert!(Tuner::new(Presets::default()).resume().is_none());
    }
}
