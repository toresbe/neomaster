//! Track information for link rooms: DISPLAY_DATA (0x06) and METADATA (0x0B).
//!
//! When a link room starts one of our sources, and again whenever the track changes, the BM5
//! broadcasts INFO telegrams to ALL_SOURCE_USERS (0x83) with src_src = the source: one
//! DISPLAY_DATA with the source's display name, then six METADATA telegrams with one text field
//! each, ~70-90 ms apart. As captured live (2026-11-27, N.MUSIC 7a and N.RADIO a1):
//!
//! ```text
//! 83 c2 01 2c 00 7a 00 06 11 00 | 03 01 01 00 00 "N.MUSIC     "
//! 83 c2 01 2c 00 7a 00 0b 18 00 | 01 00 01 01 7a 05 00 00 00 ff 00 ff 00 01 "Electronic"
//! 83 c2 01 2c 00 7a 00 0b 15 00 | 02 ... "Resound"   (album)
//! 83 c2 01 2c 00 7a 00 0b 14 00 | 03 ... "Ugress"    (artist)
//! 83 c2 01 2c 00 7a 00 0b 1b 00 | 04 ... "Loungemeister" (title)
//! 83 c2 01 2c 00 7a 00 0b 12 00 | 05 ... "NONE"
//! 83 c2 01 2c 00 7a 00 0b 15 00 | 06 ... "Unknown"
//! ```
//!
//! Decompiled from BeoIpcML.exe 7.04: TPCgenericDevice_SendDisplayData (0x422c914) and
//! TPCgenericDevice_SendMetadata (FUN_0422cba0); the callers that fill the METADATA record from
//! the N.MUSIC / N.RADIO players' IPC message 0x82d are FUN_0425dc74, FUN_0425e034 (N.MUSIC) and
//! FUN_0425cb94 (N.RADIO). The source-center telegrams in [`crate::SourceCenter`] do not send
//! these by themselves: the caller does, when a source starts and when its track changes, with
//! [`telegrams`] or [`crate::SourceCenter::send_track_info`].

use crate::protocol::{node, payload_type as pt, telegram_type as tt};
use crate::telegram::Telegram;

/// DISPLAY_DATA (0x06): the source's name for a room's display.
pub mod display_data {
    pub const SUBTYPE: u8 = 0;
    /// Payload bytes 0-4, constants in BeoIpcML. TODO: meaning. When the device's "first
    /// display" flag (+0x59) is set, BeoIpcML sends the telegram a second time with byte 1 = 02;
    /// the live captures only show the 01 form.
    pub const HEADER: [u8; 5] = [0x03, 0x01, 0x01, 0x00, 0x00];
    /// The text is padded with spaces or cut to this width (BeoIpcML field +0x58, set to 12 by
    /// the constructor; with the width at 0 it would send up to 32 characters unpadded).
    pub const WIDTH: usize = 12;

    /// `03 01 01 00 00 <name, 12 characters>`
    pub fn payload(name: &str) -> Vec<u8> {
        let mut text = super::ansi(name, WIDTH);
        text.resize(WIDTH, b' ');
        let mut p = HEADER.to_vec();
        p.extend_from_slice(&text);
        p
    }
}

/// METADATA (0x0B): one text field of the current track or station.
pub mod metadata {
    pub const SUBTYPE: u8 = 0;
    /// At most this many characters of text (decompiled: longer strings are cut to 23).
    pub const MAX_TEXT: usize = 23;

    /// Payload byte 0: which field. Decompiled: the players' record has these strings at these
    /// offsets; the names follow what the N.MUSIC captures carry in each. N.RADIO fills the same
    /// slots differently (observed: 2 = genre "News", 3 = country "Germany", 4 = station name,
    /// and 1 carries non-text bytes, which looks like a B&O bug).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u8)]
    pub enum Field {
        Genre = 1,
        Album = 2,
        Artist = 3,
        Title = 4,
        /// TODO: meaning; "NONE" in every capture
        Field5 = 5,
        /// TODO: meaning; "Unknown" in every capture
        Field6 = 6,
    }

    impl Field {
        pub const ALL: [Field; 6] = [Field::Genre, Field::Album, Field::Artist, Field::Title, Field::Field5, Field::Field6];
    }

    /// Payload bytes 1-13, the same in all six telegrams. Decompiled from the record the players
    /// send; the values in [`Header::default`] are what every capture carries.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Header {
        /// Bytes 1-2, big-endian: a 16-bit number from the player's record (field 0x185 * 4),
        /// most likely the track or preset number. TODO: confirm. 0001 in every capture.
        pub number: u16,
        /// Byte 3: the player's state mapped by FUN_0425d5f0 (0->2, 1->1, 2->0, 3->3, 4->4),
        /// i.e. onto SOURCE_STATUS activity values. 01 in every capture. TODO: confirm.
        pub state: u8,
        /// Byte 5 (record field 0x209 * 4). TODO: meaning. 05 in every capture.
        pub byte5: u8,
        /// Bytes 8-13: three 16-bit numbers from the record (fields 0x186, 0x188, 0x187), in
        /// that order. TODO: meaning. 00ff 00ff 0001 in every capture.
        pub words: [u16; 3],
    }

    impl Default for Header {
        fn default() -> Self {
            Header { number: 1, state: 0x01, byte5: 0x05, words: [0x00ff, 0x00ff, 0x0001] }
        }
    }

    /// `<field> <number:2> <state> <source> <byte5> 00 00 <words:6> <text, up to 23 characters>`
    pub fn payload(field: Field, src: u8, header: &Header, text: &str) -> Vec<u8> {
        let n = header.number.to_be_bytes();
        let mut p = vec![field as u8, n[0], n[1], header.state, src, header.byte5, 0x00, 0x00];
        for w in header.words {
            p.extend_from_slice(&w.to_be_bytes());
        }
        p.extend_from_slice(&super::ansi(text, MAX_TEXT));
        p
    }
}

/// What the room displays for the current track. Empty strings are sent as they are; use the
/// constants below for what B&O's players send when nothing is known.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackInfo {
    pub genre: String,
    pub album: String,
    pub artist: String,
    pub title: String,
    pub field5: String,
    pub field6: String,
}

impl TrackInfo {
    /// B&O's N.MUSIC player for an untagged track (observed)
    pub const UNKNOWN_GENRE: &'static str = "Unknown Genre";
    pub const UNKNOWN_ALBUM: &'static str = "Unknown Album";
    pub const UNKNOWN_ARTIST: &'static str = "Unknown Artist";
    pub const UNKNOWN_TITLE: &'static str = "undefined";
    pub const FIELD5_DEFAULT: &'static str = "NONE";
    pub const FIELD6_DEFAULT: &'static str = "Unknown";

    /// The values B&O's N.MUSIC sends before it knows the track (observed at source start)
    pub fn unknown() -> Self {
        TrackInfo {
            genre: Self::UNKNOWN_GENRE.into(),
            album: Self::UNKNOWN_ALBUM.into(),
            artist: Self::UNKNOWN_ARTIST.into(),
            title: Self::UNKNOWN_TITLE.into(),
            field5: Self::FIELD5_DEFAULT.into(),
            field6: Self::FIELD6_DEFAULT.into(),
        }
    }

    pub fn field(&self, f: metadata::Field) -> &str {
        use metadata::Field::*;
        match f {
            Genre => &self.genre,
            Album => &self.album,
            Artist => &self.artist,
            Title => &self.title,
            Field5 => &self.field5,
            Field6 => &self.field6,
        }
    }
}

/// The seven telegrams in the order the BM5 sends them: DISPLAY_DATA with `display_name`, then
/// METADATA fields 1-6. `from` is our node address (SOURCE_CENTER as captured); `src` the source.
/// Space them out like B&O (~70-90 ms) if the link is busy; the captures show no reply to wait for.
pub fn telegrams(from: u8, src: u8, display_name: &str, header: &metadata::Header, info: &TrackInfo) -> Vec<Telegram> {
    let info_telegram = |ptype: u8, subtype: u8, payload: Vec<u8>| Telegram {
        dest_node: node::ALL_SOURCE_USERS,
        src_node: from,
        telegram_type: tt::INFO,
        dest_src: 0,
        src_src: src,
        payload_type: ptype,
        subtype,
        payload,
    };
    let mut out = vec![info_telegram(pt::DISPLAY_DATA, display_data::SUBTYPE, display_data::payload(display_name))];
    for f in metadata::Field::ALL {
        out.push(info_telegram(pt::METADATA, metadata::SUBTYPE, metadata::payload(f, src, header, info.field(f))));
    }
    out
}

/// Text as BeoIpcML sends it: one byte per character (Delphi AnsiString, Windows-1252 on a BM5).
/// Latin-1 characters pass through; others become '?'. Cut to `max` characters.
fn ansi(s: &str, max: usize) -> Vec<u8> {
    s.chars().take(max).map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' }).collect()
}

impl crate::SourceCenter {
    /// Broadcast DISPLAY_DATA and METADATA for `src` (see the module docs): call when a link room
    /// starts one of our sources and whenever its track changes.
    pub fn send_track_info(&self, host: &mut dyn crate::Host, src: u8, display_name: &str, info: &TrackInfo) {
        for t in telegrams(node::SOURCE_CENTER, src, display_name, &metadata::Header::default(), info) {
            host.send_telegram(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Live capture 2026-11-27 04:42:54, N.MUSIC (7a) playing "Loungemeister" by Ugress
    #[test]
    fn matches_nmusic_capture() {
        let info = TrackInfo {
            genre: "Electronic".into(),
            album: "Resound".into(),
            artist: "Ugress".into(),
            title: "Loungemeister".into(),
            field5: TrackInfo::FIELD5_DEFAULT.into(),
            field6: TrackInfo::FIELD6_DEFAULT.into(),
        };
        let got: Vec<Vec<u8>> = telegrams(node::SOURCE_CENTER, 0x7a, "N.MUSIC", &metadata::Header::default(), &info)
            .iter()
            .map(|t| t.encode_mlgw())
            .collect();
        let want: Vec<Vec<u8>> = [
            "83c2012c007a0006110003010100004e2e4d555349432020202020",
            "83c2012c007a000b1800010001017a05000000ff00ff0001456c656374726f6e6963",
            "83c2012c007a000b1500020001017a05000000ff00ff00015265736f756e64",
            "83c2012c007a000b1400030001017a05000000ff00ff0001556772657373",
            "83c2012c007a000b1b00040001017a05000000ff00ff00014c6f756e67656d656973746572",
            "83c2012c007a000b1200050001017a05000000ff00ff00014e4f4e45",
            "83c2012c007a000b1500060001017a05000000ff00ff0001556e6b6e6f776e",
        ]
        .iter()
        .map(|s| hex(s))
        .collect();
        assert_eq!(got, want);
    }

    /// Live capture 2026-11-27 04:42:52: N.MUSIC just started, nothing known yet
    #[test]
    fn matches_unknown_track_capture() {
        let got = telegrams(node::SOURCE_CENTER, 0x7a, "N.MUSIC", &metadata::Header::default(), &TrackInfo::unknown());
        assert_eq!(got[1].encode_mlgw(), hex("83c2012c007a000b1b00010001017a05000000ff00ff0001556e6b6e6f776e2047656e7265"));
        assert_eq!(got[4].encode_mlgw(), hex("83c2012c007a000b1700040001017a05000000ff00ff0001756e646566696e6564"));
    }

    /// Live capture 2026-11-27 04:46:06, N.RADIO (a1): display name and station
    #[test]
    fn matches_nradio_capture() {
        assert_eq!(
            telegrams(node::SOURCE_CENTER, 0xa1, "N.RADIO", &metadata::Header::default(), &TrackInfo::default())[0].encode_mlgw(),
            hex("83c2012c00a10006110003010100004e2e524144494f2020202020")
        );
        assert_eq!(
            metadata::payload(metadata::Field::Title, 0xa1, &metadata::Header::default(), "Deutschlandfunk"),
            hex("040001 01a1050000 00ff00ff0001 446575747363686c616e6466756e6b".replace(' ', "").as_str())
        );
    }

    #[test]
    fn cuts_and_pads_text() {
        assert_eq!(display_data::payload("A.MEM").len(), 5 + display_data::WIDTH);
        assert_eq!(&display_data::payload("ABCDEFGHIJKLMNOP")[5..], b"ABCDEFGHIJKL");
        let p = metadata::payload(metadata::Field::Title, 0x7a, &metadata::Header::default(), &"x".repeat(40));
        assert_eq!(p.len(), 14 + metadata::MAX_TEXT);
        assert_eq!(
            &metadata::payload(metadata::Field::Artist, 0x7a, &metadata::Header::default(), "Sigur Rós")[14..],
            b"Sigur R\xf3s"
        );
        assert_eq!(&metadata::payload(metadata::Field::Artist, 0x7a, &metadata::Header::default(), "坂本")[14..], b"??");
    }
}
