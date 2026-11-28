//! Sans-IO driver for the BeoSound 5 panel (USB HID, 0cd4:1112).
//!
//! Report format from the 27 Nov 2026 USB captures (rea-local/work/bm5-ui/usb_capture_notes.md):
//!
//! | Direction | Byte | Meaning |
//! |---|---|---|
//! | IN, EP 0x81 | 0 | menu wheel, signed delta (+1 clockwise); the stock UI scales it by speed (see `ui::wheel`) |
//! | IN | 1 | volume wheel, signed delta |
//! | IN | 2 | pointer arc, absolute 0-127 |
//! | IN | 3 | buttons: 0x40 GO, 0x20 LEFT, 0x10 RIGHT |
//! | IN | 4, 5 | constant 0xc0; lagging copy of the pointer (ignored) |
//! | OUT, SET_REPORT, 2 bytes | 0 | 0x40 backlight, 0x80 LED solid, 0x10 LED blink, low nibble = one click |
//! | OUT | 1 | IR flags: c0 (both captured blanks ended on c0) |
//!
//! The panel has no detents and no long press in hardware: [`Decoder`] times holds, and
//! [`Output`] coalesces clicks to at most one per frame. Wheel deltas pass through raw; the
//! UI turns them into rows the stock way (`ui::wheel`), which also drops a part-turn when the
//! direction changes, the "first tick after a direction change" seen over VNC. (The old C++ panel.cpp used `&&` where it meant
//! `&` when reading the buttons, so any button read as all three.)

use std::time::Duration;

/// A monotonic timestamp, as a duration since an arbitrary fixed epoch.
pub type Time = Duration;

pub const VENDOR_ID: u16 = 0x0cd4;
pub const PRODUCT_ID: u16 = 0x1112;
pub const ENDPOINT_IN: u8 = 0x81;
pub const REPORT_LEN: usize = 6;

pub const BUTTON_GO: u8 = 0x40;
pub const BUTTON_LEFT: u8 = 0x20;
pub const BUTTON_RIGHT: u8 = 0x10;

/// Menu-wheel counts for one list row on a slow turn: the stock UI moves 1.5 units per count
/// (more when the list is already moving fast) and a row is 105 units. See `ui::wheel`.
pub const COUNTS_PER_STEP: i32 = 70;
/// Highest pointer arc position.
pub const POINTER_MAX: u8 = 127;

/// One IN report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub menu: i8,
    pub volume: i8,
    pub pointer: u8,
    pub buttons: u8,
}

impl Report {
    pub fn parse(bytes: &[u8]) -> Option<Report> {
        if bytes.len() < 4 {
            return None;
        }
        Some(Report { menu: bytes[0] as i8, volume: bytes[1] as i8, pointer: bytes[2] & 0x7f, buttons: bytes[3] })
    }

    /// The bytes the panel would send (used by the simulator).
    pub fn encode(&self) -> [u8; REPORT_LEN] {
        [self.menu as u8, self.volume as u8, self.pointer, self.buttons, 0xc0, self.pointer]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Go,
    Left,
    Right,
}

impl Button {
    pub const ALL: [Button; 3] = [Button::Go, Button::Left, Button::Right];

    pub fn mask(self) -> u8 {
        match self {
            Button::Go => BUTTON_GO,
            Button::Left => BUTTON_LEFT,
            Button::Right => BUTTON_RIGHT,
        }
    }

    fn index(self) -> usize {
        match self {
            Button::Go => 0,
            Button::Left => 1,
            Button::Right => 2,
        }
    }
}

/// What the panel did, in the vocabulary the core uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// Raw counts; the UI accumulates them into steps.
    MenuWheel(i32),
    VolumeWheel(i32),
    /// The pointer moved to this absolute position.
    Pointer(u8),
    Pressed(Button),
    Released(Button),
    /// The button has been held for [`Decoder::long_press`]; `Released` still follows.
    LongPress(Button),
}

/// Turns reports into [`Input`]s: wheel deltas, pointer changes, button edges and long presses.
pub struct Decoder {
    /// Hold time for [`Input::LongPress`]. The stock UI showed the long-GO menu within about 2 s;
    /// the exact threshold is still to be measured.
    pub long_press: Duration,
    pointer: Option<u8>,
    buttons: u8,
    held_since: [Option<Time>; 3],
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder { long_press: Duration::from_millis(800), pointer: None, buttons: 0, held_since: [None; 3] }
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, report: Report, now: Time) -> Vec<Input> {
        let mut out = Vec::new();
        if report.menu != 0 {
            out.push(Input::MenuWheel(report.menu as i32));
        }
        if report.volume != 0 {
            out.push(Input::VolumeWheel(report.volume as i32));
        }
        if self.pointer != Some(report.pointer) {
            self.pointer = Some(report.pointer);
            out.push(Input::Pointer(report.pointer));
        }
        for b in Button::ALL {
            let was = self.buttons & b.mask() != 0;
            let is = report.buttons & b.mask() != 0;
            if is && !was {
                self.held_since[b.index()] = Some(now);
                out.push(Input::Pressed(b));
            } else if was && !is {
                self.held_since[b.index()] = None;
                out.push(Input::Released(b));
            }
        }
        self.buttons = report.buttons;
        out.extend(self.tick(now));
        out
    }

    /// Call periodically while a button is held (see [`Decoder::next_deadline`]).
    pub fn tick(&mut self, now: Time) -> Vec<Input> {
        let mut out = Vec::new();
        for b in Button::ALL {
            if let Some(since) = self.held_since[b.index()] {
                if now >= since + self.long_press {
                    self.held_since[b.index()] = None; // fire once per hold
                    out.push(Input::LongPress(b));
                }
            }
        }
        out
    }

    /// When the next long press would fire, if a button is held.
    pub fn next_deadline(&self) -> Option<Time> {
        self.held_since.iter().flatten().map(|s| *s + self.long_press).min()
    }

    pub fn pointer(&self) -> Option<u8> {
        self.pointer
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Led {
    #[default]
    Off,
    Solid,
    Blink,
}

/// The OUT report: backlight, LED, IR flags, and clicks.
pub struct Output {
    pub backlight: bool,
    pub led: Led,
    /// Byte 1, the IR flags. Both captured blanks ended on c0; 80 was seen only briefly.
    pub ir_flags: u8,
    /// At most one click per this interval (one frame at 60 Hz): a fast spin must not flood the
    /// bus (the stock UI sent about 60 clicks in 3 s).
    pub click_interval: Duration,
    last_click: Option<Time>,
}

pub const IR_AWAKE: u8 = 0xc0;
/// Seen briefly in one capture ("40 80"); not used.
pub const IR_IDLE: u8 = 0x80;

impl Default for Output {
    fn default() -> Self {
        Output {
            backlight: true,
            led: Led::Off,
            ir_flags: IR_AWAKE,
            click_interval: Duration::from_micros(16_667),
            last_click: None,
        }
    }
}

impl Output {
    pub fn new() -> Self {
        Self::default()
    }

    fn byte0(&self) -> u8 {
        (if self.backlight { 0x40 } else { 0 })
            | match self.led {
                Led::Off => 0,
                Led::Solid => 0x80,
                Led::Blink => 0x10,
            }
    }

    /// The report for the current state, without a click.
    pub fn report(&self) -> [u8; 2] {
        [self.byte0(), self.ir_flags]
    }

    /// The report with one click, or None if a click already went out this frame.
    pub fn click(&mut self, now: Time) -> Option<[u8; 2]> {
        if let Some(last) = self.last_click {
            if now < last + self.click_interval {
                return None;
            }
        }
        self.last_click = Some(now);
        Some([self.byte0() | 0x01, self.ir_flags])
    }

    /// Blank-screen standby: LED solid, then backlight off, as `BeoSound5.exe` does it
    /// (HwSource SetLed(1) then SetBacklight(0)): "c0 c0", "80 c0". Send both, in order.
    pub fn blank(&mut self) -> [[u8; 2]; 2] {
        self.ir_flags = IR_AWAKE;
        self.led = Led::Solid;
        let first = self.report();
        self.backlight = false;
        [first, self.report()]
    }

    /// While blanked, the stock UI turns the LED on again every 10 s (SetLed(1)).
    pub fn led_refresh(&mut self) -> [u8; 2] {
        self.led = Led::Solid;
        self.report()
    }

    /// Wake from blank-screen standby: LED off, then backlight on (SetLed(0), SetBacklight(1)):
    /// "00 c0", "40 c0". Send both, in order.
    pub fn wake(&mut self) -> [[u8; 2]; 2] {
        self.ir_flags = IR_AWAKE;
        self.led = Led::Off;
        let first = self.report();
        self.backlight = true;
        [first, self.report()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Time {
        Duration::from_millis(n)
    }

    #[test]
    fn decodes_wheels_pointer_and_button_edges() {
        let mut d = Decoder::new();
        let r = Report::parse(&[0x01, 0xff, 0x40, 0x00, 0xc0, 0x40]).unwrap();
        assert_eq!(d.feed(r, ms(0)), vec![Input::MenuWheel(1), Input::VolumeWheel(-1), Input::Pointer(0x40)]);
        let r = Report::parse(&[0x00, 0x00, 0x40, 0x20, 0xc0, 0x40]).unwrap();
        assert_eq!(d.feed(r, ms(10)), vec![Input::Pressed(Button::Left)]);
        let r = Report::parse(&[0x00, 0x00, 0x40, 0x00, 0xc0, 0x40]).unwrap();
        assert_eq!(d.feed(r, ms(20)), vec![Input::Released(Button::Left)]);
    }

    #[test]
    fn one_button_is_not_all_three() {
        // panel.cpp's `buttons && 0x40` bug read GO as GO + LEFT + RIGHT
        let mut d = Decoder::new();
        let r = Report { buttons: BUTTON_GO, ..Default::default() };
        let got = d.feed(r, ms(0));
        assert!(got.contains(&Input::Pressed(Button::Go)));
        assert!(!got.contains(&Input::Pressed(Button::Left)));
    }

    #[test]
    fn long_press_fires_once_while_held() {
        let mut d = Decoder::new();
        d.feed(Report { buttons: BUTTON_GO, ..Default::default() }, ms(0));
        assert_eq!(d.next_deadline(), Some(ms(800)));
        assert!(d.tick(ms(799)).is_empty());
        assert_eq!(d.tick(ms(800)), vec![Input::LongPress(Button::Go)]);
        assert!(d.tick(ms(2000)).is_empty());
        assert_eq!(d.feed(Report::default(), ms(2100)), vec![Input::Released(Button::Go)]);
    }

    #[test]
    fn output_matches_captured_bytes() {
        let mut o = Output::new();
        assert_eq!(o.report(), [0x40, 0xc0]);
        assert_eq!(o.click(ms(0)), Some([0x41, 0xc0])); // "41 c0"
        assert_eq!(o.click(ms(5)), None); // coalesced within the frame
        assert_eq!(o.click(ms(17)), Some([0x41, 0xc0]));
        assert_eq!(o.blank(), [[0xc0, 0xc0], [0x80, 0xc0]]); // 19:50:30 capture
        assert_eq!(o.led_refresh(), [0x80, 0xc0]);
        assert_eq!(o.wake(), [[0x00, 0xc0], [0x40, 0xc0]]); // 19:52:36 capture
    }
}
