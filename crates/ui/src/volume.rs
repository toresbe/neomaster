//! The volume wheel and the volume arc as BeoSound5.exe does them, from the decompiled code (28
//! Nov 2026; addresses in the stock BeoSound5.exe).
//!
//! The wheel (`A18Controller_OnHwVolumeWheel` 0x4fbe3b): the counts add up, and once more than
//! 3.47 have gathered and at least 100 ms have passed since the last step, the whole steps in
//! them go out as one volume change and the fraction stays. A change is at most 2 steps, and only
//! 1 when the wheel was still for a second or more. `BS5System_OnHWVolumeChange` (0x4f2e07)
//! unmutes first, then sets the volume to the current one plus the steps.
//!
//! The arc (`VolumeWheel`, 0x42a57b): VolumeLightArc.png stays put, tinted with the menu arc's
//! colour at the beam, and a black mask (VolumeFadeOverlay.png) turns about a point to uncover it
//! from the bottom: by 19 + 144 x level degrees, where the level is 0 for volume 0 and otherwise
//! 6.2 % + 81.6 % x volume/90 (`BS5System_OnAudioVolumeChange` 0x4f2bc6, 0x513556). The level
//! moves to a new value in 0.1 s; the arc fades in over 0.1 s, stays 2 s after the last change
//! and fades out over 0.4 s.

use std::time::Duration;

/// Counts per volume step (stock double 3.4666667 at 0x32e12f0).
pub const COUNTS_PER_STEP: f32 = 3.466_666_7;
const MIN_GAP: Duration = Duration::from_millis(100);
const BURST: Duration = Duration::from_millis(1000);

/// The arc's timing.
pub const LEVEL_TIME: Duration = Duration::from_millis(100);
pub const FADE_IN: Duration = Duration::from_millis(100);
pub const HOLD: Duration = Duration::from_millis(2000);
pub const FADE_OUT: Duration = Duration::from_millis(400);

/// The wheel's part-step between reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct VolumeWheel {
    counts: f32,
    last: Option<Duration>,
}

impl VolumeWheel {
    /// One report's `counts` at `now`: the volume steps to apply (positive is up).
    pub fn turn(&mut self, counts: i32, now: Duration) -> i32 {
        self.counts += counts as f32;
        let gap = self.last.map_or(Duration::MAX, |t| now.saturating_sub(t));
        if self.counts.abs() <= COUNTS_PER_STEP || gap < MIN_GAP {
            return 0;
        }
        let q = self.counts / COUNTS_PER_STEP;
        let whole = q.trunc();
        self.counts = (q - whole) * COUNTS_PER_STEP;
        let n = whole as i32;
        let sign = if n > 0 { 1 } else { -1 };
        self.last = Some(now);
        if gap >= BURST {
            sign
        } else if n * sign > 2 {
            2 * sign
        } else {
            n
        }
    }
}

/// The arc's level (0..1) for a volume of 0..=`max`.
pub fn level(volume: u8, max: u8) -> f32 {
    if volume == 0 {
        0.0
    } else {
        (81.6 * volume as f32 / max as f32 + 6.2) / 100.0
    }
}

/// Where the uncovered part of VolumeLightArc.png ends, in the texture's rows: the mask's edge
/// crosses the arc (radius 270 about (284, 280) in the texture) at 63.2 - 135.0 x level degrees
/// below level (fitted to the stock mask turned about its pivot, within 0.5 degrees).
pub fn lit_from_row(level: f32) -> f32 {
    let phi = (63.193 - 134.957 * level).to_radians();
    280.0 + 270.0 * phi.sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn steps_need_3_5_counts_and_100_ms() {
        let mut w = VolumeWheel::default();
        assert_eq!(w.turn(3, ms(0)), 0);
        assert_eq!(w.turn(1, ms(10)), 1, "4 counts, first step after a rest");
        assert_eq!(w.turn(4, ms(50)), 0, "within 100 ms");
        assert_eq!(w.turn(4, ms(160)), 2, "the 8.5 counts since: 2 steps");
        assert_eq!(w.turn(20, ms(300)), 2, "at most 2 at a time");
        assert_eq!(w.turn(-5, ms(1500)), -1);
    }

    #[test]
    fn the_arc_matches_the_stock_capture() {
        assert_eq!(level(0, 90), 0.0);
        assert!((level(90, 90) - 0.878).abs() < 1e-6);
        // the stock arc at full volume was lit up to 55.2 degrees above level (27 Nov capture)
        let top = lit_from_row(level(90, 90));
        let deg = ((top - 280.0) / 270.0).asin().to_degrees();
        assert!((deg + 55.2).abs() < 0.5, "{deg}");
        assert!(lit_from_row(0.0) > 515.0);
    }
}
