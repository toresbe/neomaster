//! Idle and blank-screen standby, as the stock `BeoSound5.exe` does it (decompiled 27 Nov 2026;
//! addresses below are in that binary, annotated in the Ghidra project `work/ghidra-bs5exe`).
//!
//! The stock `A18Controller` keeps two timers that every panel input restarts
//! (`ResetTimers`, 0x4e2576):
//!
//! - a 180 s one-shot that returns the UI to the source's default view (0x4e8130). It is only
//!   restarted while a source is active;
//! - a 1,800 s repeating tick (`OnStandbyTimeout`, 0x4e5101). On a tick with nothing playing
//!   the panel goes to blank-screen standby. While playing, ticks are counted, and standby comes
//!   on tick `ActivePlaybackTimeOut × 2` (registry default 12 h, so the 24th tick).
//!
//! Blank-screen standby (`EnterBlankStandby`, 0x4e5836) stops the local source, turns the panel
//! LED on and the backlight off, and stops the tick. While blanked, a 10 s timer turns the LED on
//! again (0x4e62a2). Any panel input wakes it (LED off, backlight on).
//!
//! Besides the tick, three events blank at once: a standby (Beo4 STANDBY or Masterlink, events
//! 0x0b/0x20), a long standby (0x0c/0x21, after the all-off), and the active source reporting
//! that it stopped with nobody local holding it (0x10). Those arrive here as calls to
//! [`Idle::blank`]; detecting them is the Masterlink side's job.
//!
//! No stock code dims the screen, so there is no dim step.

use std::time::Duration;

/// A monotonic timestamp, as a duration since an arbitrary fixed epoch.
pub type Time = Duration;

/// The stock defaults; all of them belong in config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdleConfig {
    /// Return to the default view (now-playing) this long after the last input, if a source is
    /// active. Stock: 180 s.
    pub return_after: Duration,
    /// The standby tick. Stock: 1,800 s.
    pub standby_tick: Duration,
    /// While something plays, blank only after this long without input. Stock:
    /// `ActivePlaybackTimeOut` = 12 h; counted in whole ticks.
    pub playing_standby_after: Duration,
    /// While blanked, turn the panel LED on again this often. Stock: 10 s.
    pub led_refresh: Duration,
}

impl Default for IdleConfig {
    fn default() -> Self {
        IdleConfig {
            return_after: Duration::from_secs(180),
            standby_tick: Duration::from_secs(1800),
            playing_standby_after: Duration::from_secs(12 * 3600),
            led_refresh: Duration::from_secs(10),
        }
    }
}

impl IdleConfig {
    /// The tick on which standby comes while playing (stock: `ActivePlaybackTimeOut × 2`).
    fn playing_ticks(&self) -> u32 {
        let tick = self.standby_tick.as_millis().max(1);
        (self.playing_standby_after.as_millis() / tick).max(1) as u32
    }
}

/// What the idle logic asks the UI to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleAction {
    /// Leave the browse view for now-playing.
    ReturnToNowPlaying,
    /// Enter blank-screen standby: stop local playback, LED on, backlight off.
    Blank,
    /// Still blanked: turn the LED on again.
    LedRefresh,
}

#[derive(Debug)]
pub struct Idle {
    pub cfg: IdleConfig,
    return_due: Option<Time>,
    tick_due: Option<Time>,
    ticks: u32,
    playing: bool,
    blanked: bool,
    led_due: Option<Time>,
}

impl Idle {
    pub fn new(cfg: IdleConfig, now: Time) -> Idle {
        Idle {
            tick_due: Some(now + cfg.standby_tick),
            cfg,
            return_due: None,
            ticks: 0,
            playing: false,
            blanked: false,
            led_due: None,
        }
    }

    pub fn blanked(&self) -> bool {
        self.blanked
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Local playback started or stopped (the stock "playback active" flag; paused counts as
    /// stopped).
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    /// A panel input. Returns true if it woke the panel from blank-screen standby (the caller
    /// then swallows the input). `source_active`: a source is selected, which is when the stock
    /// UI restarts the return timer.
    pub fn input(&mut self, now: Time, source_active: bool) -> bool {
        let woke = self.blanked;
        self.blanked = false;
        self.led_due = None;
        self.return_due = source_active.then(|| now + self.cfg.return_after);
        self.tick_due = Some(now + self.cfg.standby_tick);
        self.ticks = 0;
        woke
    }

    /// Enter blank-screen standby now (standby, long standby, our source released, or the
    /// tick). Returns false if already blanked.
    pub fn blank(&mut self, now: Time) -> bool {
        if self.blanked {
            return false;
        }
        self.blanked = true;
        self.playing = false;
        self.return_due = None;
        self.tick_due = None;
        self.ticks = 0;
        self.led_due = Some(now + self.cfg.led_refresh);
        true
    }

    /// Fire every timer that is due by `now`.
    pub fn tick(&mut self, now: Time) -> Vec<IdleAction> {
        let mut out = Vec::new();
        if self.blanked {
            if let Some(due) = self.led_due {
                if now >= due {
                    out.push(IdleAction::LedRefresh);
                    let mut next = due + self.cfg.led_refresh;
                    if next <= now {
                        next = now + self.cfg.led_refresh;
                    }
                    self.led_due = Some(next);
                }
            }
            return out;
        }
        if let Some(due) = self.return_due {
            if now >= due {
                self.return_due = None;
                out.push(IdleAction::ReturnToNowPlaying);
            }
        }
        while let Some(due) = self.tick_due {
            if now < due {
                break;
            }
            self.ticks += 1;
            if !self.playing || self.ticks >= self.cfg.playing_ticks() {
                if self.blank(now) {
                    out.push(IdleAction::Blank);
                }
                break;
            }
            self.tick_due = Some(due + self.cfg.standby_tick);
        }
        out
    }

    /// The next moment [`Idle::tick`] has something to do.
    pub fn next_deadline(&self) -> Option<Time> {
        if self.blanked {
            return self.led_due;
        }
        [self.return_due, self.tick_due].into_iter().flatten().min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> Time {
        Duration::from_secs(n)
    }

    #[test]
    fn returns_to_now_playing_after_180_s_only_with_a_source() {
        let mut i = Idle::new(IdleConfig::default(), s(0));
        i.input(s(0), false);
        assert_eq!(i.tick(s(180)), vec![]);
        i.input(s(200), true);
        assert_eq!(i.next_deadline(), Some(s(380)));
        assert_eq!(i.tick(s(379)), vec![]);
        assert_eq!(i.tick(s(380)), vec![IdleAction::ReturnToNowPlaying]);
        assert_eq!(i.tick(s(600)), vec![]); // one-shot
    }

    #[test]
    fn blanks_on_the_first_tick_when_nothing_plays() {
        let mut i = Idle::new(IdleConfig::default(), s(0));
        i.input(s(10), true);
        assert_eq!(i.tick(s(1809)), vec![IdleAction::ReturnToNowPlaying]);
        assert_eq!(i.tick(s(1810)), vec![IdleAction::Blank]);
        assert!(i.blanked());
    }

    #[test]
    fn input_restarts_the_tick() {
        let mut i = Idle::new(IdleConfig::default(), s(0));
        i.input(s(1700), false);
        assert_eq!(i.tick(s(1800)), vec![]);
        assert_eq!(i.tick(s(3500)), vec![IdleAction::Blank]);
    }

    #[test]
    fn playing_blanks_after_12_hours_without_input() {
        let mut i = Idle::new(IdleConfig::default(), s(0));
        i.input(s(0), true);
        i.set_playing(true);
        let mut t = 0;
        for _ in 0..23 {
            t += 1800;
            assert!(!i.tick(s(t)).contains(&IdleAction::Blank), "blanked early at {t} s");
        }
        assert!(!i.blanked());
        assert_eq!(i.tick(s(12 * 3600)), vec![IdleAction::Blank]);
        assert!(!i.playing(), "blank stops local playback");
    }

    #[test]
    fn playback_stopping_blanks_on_the_next_tick() {
        let mut i = Idle::new(IdleConfig::default(), s(0));
        i.input(s(0), true);
        i.set_playing(true);
        i.tick(s(1800));
        i.tick(s(3600));
        i.set_playing(false);
        assert_eq!(i.tick(s(5400)), vec![IdleAction::Blank]);
    }

    #[test]
    fn blanked_refreshes_the_led_every_10_s_and_input_wakes() {
        let mut i = Idle::new(IdleConfig::default(), s(0));
        assert!(i.blank(s(100)));
        assert!(!i.blank(s(101)));
        assert_eq!(i.next_deadline(), Some(s(110)));
        assert_eq!(i.tick(s(109)), vec![]);
        assert_eq!(i.tick(s(110)), vec![IdleAction::LedRefresh]);
        assert_eq!(i.tick(s(120)), vec![IdleAction::LedRefresh]);
        assert!(i.input(s(125), false));
        assert!(!i.blanked());
        assert!(!i.input(s(126), false));
        assert_eq!(i.next_deadline(), Some(s(126 + 1800)));
    }
}
