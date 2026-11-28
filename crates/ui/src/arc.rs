//! The pointer arc as BeoSound5.exe drives it, from the decompiled code (28 Nov 2026).
//!
//! Angles are radians about the arc's centre, positive up the screen. Addresses are in the stock
//! BeoSound5.exe:
//! - `A18SelectorLayer_OnBeamMove` (0x512dfa) turns the panel's pointer byte into a target angle:
//!   it snaps the ends with some hysteresis ([`snap`]), then maps 0..=125 linearly ([`angle`]).
//! - `SelectorBeamControl_Update` (0x41875a) eases the drawn angle towards it every frame
//!   ([`ease`]), tints the beam with the arc's colour at that angle ([`colour_at`]), sets the beam's
//!   opacity ([`BeamGlow`]) and rotates beam and flare about the centre. Past ±0.85 rad the beam is
//!   off the arc and the view is now-playing.
//! - `SelectorListControl` (0x419e03, 0x41a11d) puts the labels at fixed angles per label count
//!   ([`LABEL_ANGLES`]), picks the label nearest the beam ([`label_index`]) and lights it
//!   ([`label_light`]).

use render::Color;

/// Beam swing for pointer 0..=125 (stock double at 0x32e11a0) and its offset (0x32e1190).
const RAD_PER_125: f32 = 1.812_456;
const ANGLE_AT_ZERO: f32 = 0.918_198;
/// The pointer value the top end snaps to.
pub const TOP: u8 = 125;

/// Past these the beam is off the arc: now-playing (stock signals at +-0.85 rad).
pub const ABOVE_ARC: f32 = 0.85;
pub const BELOW_ARC: f32 = -0.85;

/// Snap the panel's pointer byte as the stock does: near the bottom it drops to 0 when moving
/// down (or below 20), near the top it jumps to 125 when moving up (or above 105).
pub fn snap(raw: u8, last: u8) -> u8 {
    if raw < 30 {
        if raw < last || raw < 20 {
            return 0;
        }
    } else if raw > 94 && (raw > last || raw > 105) {
        return TOP;
    }
    raw
}

/// The beam's target angle for a snapped pointer value: 0.8308 degrees per step, level at 63.3.
pub fn angle(p: u8) -> f32 {
    p as f32 * RAD_PER_125 / 125.0 - ANGLE_AT_ZERO
}

/// The pointer value that puts the beam nearest `a`.
pub fn pointer_at(a: f32) -> u8 {
    ((a + ANGLE_AT_ZERO) * 125.0 / RAD_PER_125).round().clamp(0.0, TOP as f32) as u8
}

/// One frame of the stock easing: a fraction 20*dt (at most all) of the way, and there once
/// within 0.001 rad.
pub fn ease(shown: f32, target: f32, dt: f32) -> f32 {
    let d = target - shown;
    if d.abs() <= 0.001 {
        target
    } else {
        shown + (20.0 * dt).clamp(0.0, 1.0) * d
    }
}

/// The label angles for `n` labels, top first (stock table at 0x3583260; up to 11 labels).
pub const LABEL_ANGLES: [&[f32]; 12] = [
    &[],
    &[0.3913],
    &[0.3913, 0.2295],
    &[0.3913, 0.2295, 0.0737],
    &[0.3913, 0.2295, 0.0737, -0.0727],
    &[0.3913, 0.2295, 0.0737, -0.0727, -0.2284],
    &[0.3913, 0.2295, 0.0737, -0.0727, -0.2284, -0.3901],
    &[0.3913, 0.2810, 0.1764, 0.0737, -0.0727, -0.2284, -0.3901],
    &[0.3913, 0.2810, 0.1764, 0.0737, -0.0727, -0.1753, -0.2810, -0.3901],
    &[0.3913, 0.3094, 0.2295, 0.1512, 0.0737, -0.0727, -0.1753, -0.2810, -0.3901],
    &[0.3913, 0.3094, 0.2295, 0.1512, 0.0737, -0.0727, -0.1501, -0.2284, -0.3094, -0.3901],
    &[0.3913, 0.3278, 0.2643, 0.2008, 0.1372, 0.0737, -0.0727, -0.1501, -0.2284, -0.3094, -0.3901],
];

/// The angles of `n` labels, top first. The stock UI shows at most 11.
pub fn label_angles(n: usize) -> &'static [f32] {
    LABEL_ANGLES[n.min(11)]
}

/// Which label (index from the top) the beam at `a` picks: the stock interpolates the label
/// index linearly between label angles, clamped at the ends, and rounds it, so the boundaries
/// are the midpoints.
pub fn label_index(a: f32, labels: &[f32]) -> usize {
    let n = labels.len();
    if n <= 1 || a >= labels[0] {
        return 0;
    }
    if a <= labels[n - 1] {
        return n - 1;
    }
    let i = labels.iter().position(|&l| l <= a).unwrap_or(n - 1);
    let (hi, lo) = (labels[i - 1], labels[i]);
    let f = (i - 1) as f32 + (hi - a) / (hi - lo);
    (f + 0.5).floor() as usize
}

/// How lit the picked label is with the beam `d` rad away: cos(20 d), none past 4.5 degrees.
pub fn label_light(d: f32) -> f32 {
    (20.0 * d.abs()).min(std::f32::consts::PI).cos().clamp(0.0, 1.0)
}

/// The arc's colour at `a`: cyan at the bottom to blue at the top over +-0.45 rad (beam tint and
/// the current label's colour).
pub fn colour_at(a: f32) -> Color {
    let t = (0.5 + a / 0.45 * 0.5).clamp(0.0, 1.0);
    Color::rgb(0, (255.0 * (1.0 - t)).round() as u8, 255)
}

/// `from` blended towards `to` by `t`.
pub fn mix(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color::rgba(m(from.r, to.r), m(from.g, to.g), m(from.b, to.b), m(from.a, to.a))
}

/// The beam's opacity: towards 0.9 while it moves (the stock re-issues a 0.25 s animation each
/// frame), and 1 s after it stops, down to 0.6 over 0.75 s.
#[derive(Clone, Copy, Debug)]
pub struct BeamGlow {
    pub value: f32,
    /// When the beam stopped, and the opacity then.
    rest: Option<(std::time::Duration, f32)>,
}

pub const GLOW_MOVING: f32 = 0.9;
pub const GLOW_REST: f32 = 0.6;
const GLOW_MOVING_TIME: f32 = 0.25;
pub const REST_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);
pub const REST_FADE: std::time::Duration = std::time::Duration::from_millis(750);

impl Default for BeamGlow {
    fn default() -> Self {
        BeamGlow { value: GLOW_REST, rest: None }
    }
}

impl BeamGlow {
    /// One frame: `moving` if the beam was not yet where it is going.
    pub fn step(&mut self, moving: bool, now: std::time::Duration, dt: f32) {
        if moving {
            self.rest = None;
            self.value += (GLOW_MOVING - self.value) * (dt / GLOW_MOVING_TIME).min(1.0);
            return;
        }
        let (since, from) = *self.rest.get_or_insert((now, self.value));
        let fade = now.saturating_sub(since).saturating_sub(REST_DELAY);
        if now >= since + REST_DELAY {
            let t = (fade.as_secs_f32() / REST_FADE.as_secs_f32()).min(1.0);
            self.value = from + (GLOW_REST - from) * t;
        }
    }

    /// When a frame is next owed for the dimming, if it is not done.
    pub fn next_change(&self, soon: std::time::Duration) -> Option<std::time::Duration> {
        if (self.value - GLOW_REST).abs() < 0.002 {
            return None;
        }
        match self.rest {
            Some((since, _)) if soon < since + REST_DELAY => Some(since + REST_DELAY),
            _ => Some(soon),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stock_angles() {
        // screenshots with a known pointer (27 Nov 2026): flare angle above level, degrees
        for (p, deg) in [(82u8, 15.55f32), (72, 7.30), (62, -0.95), (60, -2.60)] {
            assert!((angle(p).to_degrees() - deg).abs() < 0.2, "pointer {p}: {}", angle(p).to_degrees());
        }
        assert!(angle(0) < BELOW_ARC && angle(TOP) > ABOVE_ARC);
        assert!(angle(5) > BELOW_ARC && angle(121) < ABOVE_ARC);
    }

    #[test]
    fn snapping_has_hysteresis() {
        assert_eq!(snap(19, 0), 0);
        assert_eq!(snap(25, 20), 25); // going up from the bottom
        assert_eq!(snap(25, 26), 0); // going down near the bottom
        assert_eq!(snap(95, 94), TOP); // going up near the top
        assert_eq!(snap(100, 125), 100); // coming down from the top
        assert_eq!(snap(106, 125), TOP);
        assert_eq!(snap(60, 61), 60);
    }

    #[test]
    fn labels_split_at_the_midpoints() {
        let l = label_angles(7);
        for (i, a) in l.iter().enumerate() {
            assert_eq!(label_index(*a, l), i);
        }
        assert_eq!(label_index(0.5, l), 0);
        assert_eq!(label_index(-0.5, l), 6);
        let mid = (l[2] + l[3]) / 2.0;
        assert_eq!(label_index(mid + 0.001, l), 2);
        assert_eq!(label_index(mid - 0.001, l), 3);
        // the hidden settings menu's captured pointer positions (27 Nov 2026; one capture at
        // 56 showed PRODUCT INFO, taken on the way up, but 56 is past the 55.6 boundary)
        let l = label_angles(10);
        let names = ["MODE", "SYS INFO", "SYS SET", "MAINT", "MUSIC", "SERVICE", "PRODUCT", "NET INFO", "NET SET", "ROLES"];
        for (p, want) in [(84u8, 1), (79, 2), (75, 3), (71, 4), (66, 4), (61, 5), (55, 6), (52, 6), (46, 7), (41, 8), (36, 9)] {
            assert_eq!(names[label_index(angle(p), l)], names[want], "pointer {p}");
        }
    }

    #[test]
    fn easing_and_light() {
        assert_eq!(ease(0.0, 0.5, 1.0), 0.5);
        assert!((ease(0.0, 0.5, 0.01) - 0.1).abs() < 1e-6);
        assert_eq!(ease(0.4995, 0.5, 0.0), 0.5);
        assert_eq!(label_light(0.0), 1.0);
        assert_eq!(label_light(0.08), 0.0);
        assert_eq!(colour_at(-1.0), Color::rgb(0, 255, 255));
        assert_eq!(colour_at(1.0), Color::rgb(0, 0, 255));
    }

    #[test]
    fn the_beam_dims_after_a_second() {
        let ms = std::time::Duration::from_millis;
        let mut g = BeamGlow::default();
        for i in 0..30 {
            g.step(true, ms(i * 16), 0.016);
        }
        assert!(g.value > 0.85);
        g.step(false, ms(500), 0.016);
        g.step(false, ms(1400), 0.016);
        assert!(g.value > 0.85, "still bright within the second");
        assert_eq!(g.next_change(ms(1416)), Some(ms(1500)));
        g.step(false, ms(1875), 0.016);
        assert!(g.value < 0.8 && g.value > 0.6);
        g.step(false, ms(2300), 0.016);
        assert!((g.value - GLOW_REST).abs() < 1e-6);
        assert_eq!(g.next_change(ms(2316)), None);
    }
}
