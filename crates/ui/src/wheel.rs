//! The menu wheel as BeoSound5.exe turns it into list steps, from the decompiled code (28 Nov
//! 2026; addresses in the stock BeoSound5.exe).
//!
//! - Every panel report with a menu-wheel delta becomes one command, NAV_WHEEL_UP for a delta of
//!   0 or more and NAV_WHEEL_DOWN below, carrying the raw count (`A18Controller_OnHwMenuWheel`
//!   0x4fbd8a). There is no fixed counts-per-step.
//! - The view scales the counts by a gain that grows with how fast the wheel is already moving
//!   (`AView_RotateWheel` 0x446733, `WheelGain_At` 0x42b045): 1.5 degrees per count up to 7200
//!   units/s, rising linearly to 15 at 21000 units/s. `AWheel_Rotate` turns degrees into units
//!   of a tenth of a degree (x 10), so a count is 15 units on a slow turn.
//! - The wheel adds the units to an accumulator that starts again when the direction changes,
//!   and moves only by whole items, keeping the rest (`AWheel_Rotate` 0x401926). A list row is
//!   105 units (12 rows over 126 degrees, in tenths of a degree); a cover 200 (10 over 200).
//!
//! So a slow turn takes 7 counts per row and 14 per cover; a fast spin moves more than a row per
//! count.

use std::time::Duration;

/// Units per list row and per cover (`TextWheel` 12 slots over 126 degrees, `ImageWheel` 10 over
/// 200, at 10 units per degree).
pub const ROW_UNITS: i32 = 105;
pub const COVER_UNITS: i32 = 200;

/// Degrees per count (stock floats at 0x31ac7f4 and 0x32e1364), and units per degree.
const GAIN_SLOW: f32 = 1.5;
const GAIN_FAST: f32 = 15.0;
const UNITS_PER_DEGREE: f32 = 10.0;
const SPEED_LO: f32 = 7200.0;
const SPEED_HI: f32 = 21000.0;

/// Degrees per count with the wheel moving at `speed` units/s.
pub fn gain(speed: f32) -> f32 {
    if speed <= SPEED_LO {
        GAIN_SLOW
    } else if speed >= SPEED_HI {
        GAIN_FAST
    } else {
        GAIN_SLOW + (GAIN_FAST - GAIN_SLOW) * (speed - SPEED_LO) / (SPEED_HI - SPEED_LO)
    }
}

/// How long the stock wheel takes to reach a new position: three times the gap since the
/// previous turn, between 0.1 and 0.4 s.
pub fn travel_time(gap: Option<Duration>) -> f32 {
    gap.map_or(0.4, |g| (3.0 * g.as_secs_f32()).clamp(0.1, 0.4))
}

/// The accumulator between panel reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct MenuWheel {
    units: i32,
    last: Option<Duration>,
}

impl MenuWheel {
    /// One report's `counts` at `now`, with `pending_units` still to travel on the screen and
    /// `item` units per item: how many items to move (positive is down the list).
    ///
    /// The stock speed is the mean of the last ten frames' movement; here it is the distance
    /// still to travel over the time the stock would take for it, which is what that mean
    /// settles to.
    pub fn turn(&mut self, counts: i32, now: Duration, pending_units: f32, item: i32) -> i32 {
        let speed = pending_units.abs() / travel_time(self.last.map(|t| now.saturating_sub(t)));
        self.last = Some(now);
        let units = (counts as f32 * gain(speed) * UNITS_PER_DEGREE).round() as i32;
        if units != 0 && self.units != 0 && units.signum() != self.units.signum() {
            self.units = 0;
        }
        self.units += units;
        let items = self.units / item;
        self.units -= items * item;
        items
    }

    /// Forget a part-turn (the list changed under it).
    pub fn reset(&mut self) {
        self.units = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_stock_gain() {
        assert_eq!(gain(0.0), 1.5);
        assert_eq!(gain(7200.0), 1.5);
        assert!((gain(14100.0) - 8.25).abs() < 1e-4);
        assert_eq!(gain(30000.0), 15.0);
        assert_eq!(travel_time(Some(ms(10))), 0.1);
        assert!((travel_time(Some(ms(100))) - 0.3).abs() < 1e-6);
        assert_eq!(travel_time(None), 0.4);
    }

    #[test]
    fn a_slow_turn_is_7_counts_a_row() {
        let mut w = MenuWheel::default();
        let mut rows = 0;
        for i in 0..7 {
            rows += w.turn(1, ms(i * 200), 0.0, ROW_UNITS);
        }
        assert_eq!(rows, 1);
        let mut w = MenuWheel::default();
        assert_eq!(w.turn(6, ms(0), 0.0, ROW_UNITS), 0);
        assert_eq!(w.turn(1, ms(500), 0.0, ROW_UNITS), 1);
        assert_eq!(w.turn(13, ms(1000), 0.0, COVER_UNITS), 0);
        assert_eq!(w.turn(1, ms(1500), 0.0, COVER_UNITS), 1);
    }

    #[test]
    fn turning_back_drops_the_part_turn() {
        let mut w = MenuWheel::default();
        assert_eq!(w.turn(6, ms(0), 0.0, ROW_UNITS), 0);
        assert_eq!(w.turn(-1, ms(300), 0.0, ROW_UNITS), 0);
        // 7 counts back make a row: the 6 forward did not count against them
        assert_eq!(w.turn(-6, ms(600), 0.0, ROW_UNITS), -1);
    }

    #[test]
    fn a_fast_spin_accelerates() {
        let mut w = MenuWheel::default();
        // ten rows still to travel, reports 8 ms apart: 1050 units in 0.1 s = 10500 units/s
        let n = w.turn(7, ms(0), 0.0, ROW_UNITS);
        assert_eq!(n, 1);
        let n = w.turn(7, ms(8), 10.0 * ROW_UNITS as f32, ROW_UNITS);
        assert!(n >= 3, "{n} rows");
    }
}
