//! Host side of the PC2's thermal loop (BeoMaster 5). Port of libpc2 `PC2FanControl`.
//!
//! The PC2 firmware reads an I2C temperature sensor every ~30 s and reports `IN 1c T ff` (T =
//! signed whole degrees C). The host answers with `f9 xx`, the duty of the fan PWM
//! ((xx >> 3) / 32). If no f9 arrives for 30-60 s the firmware falls back to almost full duty, so
//! a host that ignores this loop runs the fan at ~94 % from power-up (reports/pc2-firmware.md §4).
//!
//! B&O's BeoIpcML ("BM5FAN-Control", registry Software\Bang & Olufsen\BeoLinkMedia) also uses the
//! PC's CPU/HDD temperatures, so its exact output can't be reproduced from the case temperature
//! alone. This is an approximation fitted to its observed behaviour, with its registry defaults
//! (IdealTemp 37 C, HysTemp 5, MaxTemp 47 C): above Ideal+Hys the duty climbs by 0x19 per report,
//! a rising temperature adds 0x0c, a falling one removes 0x0c, otherwise it decays by 5; at or
//! above Max, or for implausible readings, it goes to full. Start value 0x80 (0x19 if already hot).

#[derive(Clone, Debug)]
pub struct FanControl {
    /// degrees C
    pub ideal: i32,
    /// degrees C
    pub hysteresis: i32,
    /// degrees C: at or above this, full duty
    pub maximum: i32,
    pub step_hot: i32,
    pub step_change: i32,
    pub step_decay: i32,
    fan: i32,
    previous: i32,
    skip: bool,
}

impl Default for FanControl {
    fn default() -> Self {
        FanControl {
            ideal: 37,
            hysteresis: 5,
            maximum: 47,
            step_hot: 0x19,
            step_change: 0x0c,
            step_decay: 5,
            fan: -1,
            previous: 0,
            skip: true,
        }
    }
}

impl FanControl {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one IN 1c temperature; returns the f9 duty to send. The first report after the host
    /// opens the device is stale (firmware detail): call [`FanControl::skip_next`] after (re)opening.
    pub fn on_temperature(&mut self, celsius: i8) -> u8 {
        let t = celsius as i32;
        if t < 0 || t >= self.maximum {
            // Implausible or too hot: full duty (B&O does the same for invalid/low and high readings)
            self.fan = 0xff;
        } else if self.fan < 0 {
            self.fan = if t > self.ideal + self.hysteresis { self.step_hot } else { 0x80 };
        } else if t > self.ideal + self.hysteresis {
            self.fan += self.step_hot;
        } else if t > self.previous {
            self.fan += self.step_change;
        } else if t < self.previous {
            self.fan -= self.step_change;
        } else {
            self.fan -= self.step_decay;
        }
        self.fan = self.fan.clamp(0, 0xff);
        self.previous = t;
        self.fan as u8
    }

    pub fn skip_next(&mut self) {
        self.skip = true;
    }

    /// True (once) if this report should be ignored as stale; clears the flag.
    pub fn consume_skip(&mut self) -> bool {
        std::mem::replace(&mut self.skip, false)
    }

    pub fn duty(&self) -> i32 {
        self.fan
    }
}
