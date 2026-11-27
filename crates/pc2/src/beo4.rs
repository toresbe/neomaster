//! Beo4 remote key codes (libpc2 `pc2/beo4.hpp`).

pub const DIGIT_0: u8 = 0x00;
pub const DIGIT_1: u8 = 0x01;
pub const DIGIT_2: u8 = 0x02;
pub const DIGIT_3: u8 = 0x03;
pub const DIGIT_4: u8 = 0x04;
pub const DIGIT_5: u8 = 0x05;
pub const DIGIT_6: u8 = 0x06;
pub const DIGIT_7: u8 = 0x07;
pub const DIGIT_8: u8 = 0x08;
pub const DIGIT_9: u8 = 0x09;
pub const STORE: u8 = 0x0b;
pub const STANDBY: u8 = 0x0c;
pub const MUTE: u8 = 0x0d;
pub const ARROW_UP: u8 = 0x1e;
pub const ARROW_DOWN: u8 = 0x1f;
pub const ARROW_LEFT: u8 = 0x32;
pub const ARROW_RIGHT: u8 = 0x34;
pub const GO: u8 = 0x35;
pub const PLAY: u8 = 0x35;
pub const STOP: u8 = 0x36;
pub const RECORD: u8 = 0x37;
pub const SOUND: u8 = 0x44;
pub const PICTURE: u8 = 0x45;
pub const MENU: u8 = 0x5c;
pub const VOL_UP: u8 = 0x60;
pub const VOL_DOWN: u8 = 0x64;
pub const TV: u8 = 0x80;
pub const RADIO: u8 = 0x81;
pub const V_AUX: u8 = 0x82;
pub const A_AUX: u8 = 0x83;
pub const RADIO2: u8 = 0x83;
pub const V_TAPE: u8 = 0x85;
pub const SAT2: u8 = 0x86;
pub const TEXT: u8 = 0x88;
pub const SAT: u8 = 0x8a;
pub const PC: u8 = 0x8b;
pub const A_TAPE: u8 = 0x91;
pub const CD: u8 = 0x92;
pub const PHONO: u8 = 0x93;
/// A.TAPE2; the BM5 panel sends it for N.MUSIC.
pub const A_TAPE2: u8 = 0x94;
pub const N_MUSIC: u8 = A_TAPE2;
pub const LIGHT: u8 = 0x9b;
pub const AV: u8 = 0xbf;
pub const YELLOW: u8 = 0xd4;
pub const GREEN: u8 = 0xd5;
pub const BLUE: u8 = 0xd8;
pub const RED: u8 = 0xd9;

/// IN 02 key type for a held key (libpc2 acts on standby only when it is held).
pub const TYPE_HELD: u8 = 0x0f;

/// Whether a key selects a source. Standby (0x0c) is not one.
pub fn is_source_key(code: u8) -> bool {
    const SOURCE_KEYS: [u8; 25] = [
        TV, RADIO, V_AUX, A_AUX, 0x84, 0x85, SAT2, 0x87, 0x88, SAT, PC, 0x8c, 0x8d, 0x8e, 0x90, A_TAPE, CD, PHONO, A_TAPE2, 0x95,
        0x96, 0x97, 0xa8, 0x47, 0xfa,
    ];
    SOURCE_KEYS.contains(&code)
}
