//! The PC2's audio routing and tone controls. Port of libpc2 `PC2Mixer`: the same state and the
//! same messages, returned as values instead of sent.

use log::debug;

use crate::Message;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MixerState {
    /// 0..0x5a, 1 dB per step, 0 = mute
    pub volume: u8,
    /// -8..+8 dB
    pub bass: i8,
    /// -8..+8 dB
    pub treble: i8,
    /// -9..+9 (+/-9 mutes a side)
    pub balance: i8,
    /// Loudness equalization
    pub loudness: bool,
    /// Headphone plug sensor
    pub headphones_plugged_in: bool,
    /// Sending to local speakers over Powerlink
    pub transmitting_locally: bool,
    /// Sending to speakers from Masterlink
    pub transmitting_from_ml: bool,
    /// Distributing local audio over Masterlink
    pub distributing_on_ml: bool,
    /// Speaker output muted
    pub speakers_muted: bool,
    /// Speaker power-on signal
    pub speakers_on: bool,
}

pub const MAX_VOLUME: u8 = 0x5a;

#[derive(Default)]
pub struct Mixer {
    pub state: MixerState,
}

impl Mixer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn speaker_mute(&mut self, muted: bool) -> Vec<Message> {
        self.state.speakers_muted = muted;
        vec![vec![0xea, if muted { 0x80 } else { 0x81 }]]
    }

    /// B&O software always unmutes after turning power on, and mutes before turning off. libpc2:
    /// "I have observed the PC2 crashing very hard if this is fudged, so I'm going to do what B&O
    /// does."
    pub fn speaker_power(&mut self, powered: bool) -> Vec<Message> {
        let mut out = Vec::new();
        if powered {
            out.push(vec![0xea, 0xff]);
            out.extend(self.speaker_mute(false));
        } else {
            out.extend(self.speaker_mute(true));
            out.push(vec![0xea, 0x00]);
        }
        self.state.speakers_on = powered;
        out
    }

    /// Relative volume in single steps (eb 80 up, eb 81 down). Gradual steps avoid a square pulse.
    pub fn adjust_volume(&mut self, adjustment: i32) -> Vec<Message> {
        let step = if adjustment > 0 { 0x80 } else { 0x81 };
        (0..adjustment.unsigned_abs()).map(|_| vec![0xeb, step]).collect()
    }

    /// Absolute volume (eb 00..5a; 1 dB per step, 0 = mute). Clamped.
    pub fn set_volume(&mut self, volume: i32) -> Vec<Message> {
        vec![vec![0xeb, volume.clamp(0, MAX_VOLUME as i32) as u8]]
    }

    /// e3: sets the volume TARGET (applied by the fade started with ea 81), bass, treble, balance
    /// and loudness. On the wire: `e3 [vol|loudness<<7] [bass] [treble] [balance]`. Firmware
    /// (reports/pc2-firmware.md §3): volume 0..0x5a, bass/treble -8..+8 (1 dB steps on the
    /// TDA7409), balance -9..+9. Out-of-range values are dropped, not clamped, by the firmware (an
    /// out-of-range tone value even blocks its loudness recomputation), so clamp here.
    /// (libpc2's argument order was volume, treble, bass; this one follows the wire.)
    pub fn set_parameters(&mut self, volume: i32, bass: i32, treble: i32, balance: i32, loudness: bool) -> Vec<Message> {
        let vol = volume.clamp(0, MAX_VOLUME as i32) as u8 | if loudness { 0x80 } else { 0 };
        vec![vec![0xe3, vol, bass.clamp(-8, 8) as i8 as u8, treble.clamp(-8, 8) as i8 as u8, balance.clamp(-9, 9) as i8 as u8]]
    }

    /// B&O's software sends E7 01 (and sets the last byte of E5) only when no audio path is
    /// active; playing Masterlink audio locally (E5 04 ...) counts as active (captures: E7 00 +
    /// E5 04 00 00 00). Firmware meaning (reports/pc2-firmware.md §2): E7 00 makes the PC2 also
    /// receive ALL_LINK (0x83) telegrams, E7 01 stops that - needed while we take part in
    /// distribution. E5: byte 0 selects the local output (01 PC, 04 Masterlink), byte 1
    /// distributes PC audio on Masterlink; bytes 2-3 are ignored by firmware V0236.
    /// Never send E7 FF: it reboots the PC2.
    pub fn routing_state(&self) -> Vec<Message> {
        let s = &self.state;
        let muted = if s.distributing_on_ml || s.transmitting_locally || s.transmitting_from_ml { 0x00 } else { 0x01 };
        let distribute = s.distributing_on_ml as u8;
        let locally = match (s.transmitting_locally, s.transmitting_from_ml) {
            (true, true) => 0x03,
            (false, true) => 0x04,
            (true, false) => 0x01,
            (false, false) => 0x00,
        };
        vec![vec![0xe7, muted], vec![0xe5, locally, distribute, 0x00, muted]]
    }

    fn set_route(&mut self, field: fn(&mut MixerState) -> &mut bool, on: bool) -> Vec<Message> {
        let f = field(&mut self.state);
        if *f == on {
            return vec![];
        }
        *f = on;
        self.routing_state()
    }

    /// Play audio arriving over Masterlink on the local speakers.
    pub fn transmit_from_ml(&mut self, on: bool) -> Vec<Message> {
        self.set_route(|s| &mut s.transmitting_from_ml, on)
    }
    /// Play the PC's audio on the local speakers.
    pub fn transmit_locally(&mut self, on: bool) -> Vec<Message> {
        self.set_route(|s| &mut s.transmitting_locally, on)
    }
    /// Distribute the PC's audio on Masterlink.
    pub fn ml_distribute(&mut self, on: bool) -> Vec<Message> {
        self.set_route(|s| &mut s.distributing_on_ml, on)
    }

    /// IN 03 / IN 1d: the PC2 reports its tone settings. `msg` keeps its 60 <len> framing.
    pub fn process_mixer_state(&mut self, msg: &[u8]) {
        if msg.len() < 7 {
            return;
        }
        self.state.volume = msg[3] & 0x7f;
        self.state.loudness = msg[3] & 0x80 != 0;
        self.state.bass = msg[4] as i8;
        self.state.treble = msg[5] as i8;
        self.state.balance = msg[6] as i8;
        debug!("PC2 mixer state: {:?}", self.state);
    }

    /// IN 06 (dead in firmware V0236, present in 1.30).
    pub fn process_headphone_state(&mut self, msg: &[u8]) {
        if msg.len() >= 4 {
            self.state.headphones_plugged_in = msg[3] == 0x01;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_to_masterlink_is_an_active_path() {
        // captures: E7 00 + E5 04 00 00 00
        let mut m = Mixer::new();
        assert_eq!(m.transmit_from_ml(true), vec![vec![0xe7, 0x00], vec![0xe5, 0x04, 0x00, 0x00, 0x00]]);
        assert_eq!(m.transmit_from_ml(true), Vec::<Message>::new());
        assert_eq!(m.transmit_from_ml(false), vec![vec![0xe7, 0x01], vec![0xe5, 0x00, 0x00, 0x00, 0x01]]);
    }

    #[test]
    fn speaker_power_order_follows_bo() {
        let mut m = Mixer::new();
        assert_eq!(m.speaker_power(true), vec![vec![0xea, 0xff], vec![0xea, 0x81]]);
        assert_eq!(m.speaker_power(false), vec![vec![0xea, 0x80], vec![0xea, 0x00]]);
    }

    #[test]
    fn parameters_are_clamped() {
        let mut m = Mixer::new();
        assert_eq!(m.set_parameters(200, -20, 3, 12, true), vec![vec![0xe3, 0xda, 0xf8, 0x03, 0x09]]);
        assert_eq!(m.set_volume(-3), vec![vec![0xeb, 0x00]]);
        assert_eq!(m.adjust_volume(-2), vec![vec![0xeb, 0x81], vec![0xeb, 0x81]]);
    }

    #[test]
    fn reads_mixer_state() {
        let mut m = Mixer::new();
        m.process_mixer_state(&[0x60, 0x05, 0x03, 0xa2, 0xfe, 0x02, 0x00, 0x61]);
        assert_eq!((m.state.volume, m.state.loudness, m.state.bass, m.state.treble), (0x22, true, -2, 2));
    }
}
