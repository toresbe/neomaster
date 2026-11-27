//! The Masterlink telegram codec (libpc2 `MasterlinkTelegram`).
//!
//! On the wire, as BeoIpcML builds it (see [`crate::protocol`]):
//!
//! ```text
//! dest src 01 TYPE dest_src src_src 00 PAYLOAD_TYPE SIZE SUBTYPE payload[SIZE] checksum 00
//! ```
//!
//! [`Telegram::encode`] produces exactly that, checksum and EOT included. [`Telegram::decode`] reads
//! the same layout and tolerates a missing checksum/EOT (MLGW captures leave them out).

use std::fmt;

/// Byte 2 of every telegram.
pub const SOT: u8 = 0x01;
/// Last byte of every telegram.
pub const EOT: u8 = 0x00;
/// Bytes before the payload.
pub const HEADER_LEN: usize = 10;

/// One Masterlink telegram. Telegram and payload types are kept as raw bytes so that unknown
/// values survive a decode/encode round trip; [`crate::protocol`] names the known ones.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Telegram {
    pub dest_node: u8,
    pub src_node: u8,
    pub telegram_type: u8,
    /// Some telegrams are addressed to a node's specific source ID.
    pub dest_src: u8,
    /// Some telegrams carry a specific source ID.
    pub src_src: u8,
    pub payload_type: u8,
    /// B&O's payload subtype; libpc2 calls it `payload_version`.
    pub subtype: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TooShort(usize),
    BadSot(u8),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::TooShort(n) => write!(f, "telegram too short ({n} bytes)"),
            DecodeError::BadSot(b) => write!(f, "byte 2 is {b:#04x}, not SOT"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Sum of all bytes, modulo 256 (libpc2 `MasterlinkTelegram::checksum`).
pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

impl Telegram {
    /// Decode a telegram starting at `dest`. A payload shorter than its SIZE byte is cut to what
    /// is there (libpc2 read past the end in that case). SOT is not checked: libpc2 never did and
    /// some sniffers report other values.
    pub fn decode(bytes: &[u8]) -> Result<Telegram, DecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(DecodeError::TooShort(bytes.len()));
        }
        let size = bytes[8] as usize;
        let end = (HEADER_LEN + size).min(bytes.len());
        Ok(Telegram {
            dest_node: bytes[0],
            src_node: bytes[1],
            telegram_type: bytes[3],
            dest_src: bytes[4],
            src_src: bytes[5],
            payload_type: bytes[7],
            subtype: bytes[9],
            payload: bytes[HEADER_LEN..end].to_vec(),
        })
    }

    /// Like [`Telegram::decode`], but also insists on SOT in byte 2.
    pub fn decode_strict(bytes: &[u8]) -> Result<Telegram, DecodeError> {
        if bytes.len() >= 3 && bytes[2] != SOT {
            return Err(DecodeError::BadSot(bytes[2]));
        }
        Self::decode(bytes)
    }

    /// Encode with checksum and EOT (libpc2 `MasterlinkTelegram::serialize`).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(HEADER_LEN + self.payload.len() + 2);
        v.extend_from_slice(&[
            self.dest_node,
            self.src_node,
            SOT,
            self.telegram_type,
            self.dest_src,
            self.src_src,
            0x00, // spare
            self.payload_type,
            self.payload.len() as u8,
            self.subtype,
        ]);
        v.extend_from_slice(&self.payload);
        v.push(checksum(&v));
        v.push(EOT);
        v
    }

    /// The encoding without checksum and EOT, as the MLGW reports telegrams.
    pub fn encode_mlgw(&self) -> Vec<u8> {
        let mut v = self.encode();
        v.truncate(v.len() - 2);
        v
    }
}

impl fmt::Debug for Telegram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} -> {} type {:02x} ptype {:02x} v{} [",
            crate::protocol::node_name(self.src_node),
            crate::protocol::node_name(self.dest_node),
            self.telegram_type,
            self.payload_type,
            self.subtype
        )?;
        for (i, b) in self.payload.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{b:02x}")?;
        }
        f.write_str("]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_checksum() {
        // REQUEST LINK_INFO to the video master, as captured: ... 9a 00
        let bytes = [0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, 0x9a, 0x00];
        let t = Telegram::decode(&bytes).unwrap();
        assert_eq!(t.payload, vec![0x01, 0x01, 0x00]);
        assert_eq!(t.encode(), bytes);
    }

    #[test]
    fn parses_dest_source_from_right_byte() {
        // VM -> PC_1 SOURCE_CONTROL with dest_src = 0x47 (captured)
        let t = Telegram::decode(&[0xc2, 0xc0, 0x01, 0x0a, 0x47, 0x00, 0x00, 0x0d, 0x06, 0x01, 0x47, 0x1f, 0x02, 0x01, 0x02, 0x00])
            .unwrap();
        assert_eq!(t.dest_src, 0x47);
        assert_eq!(t.src_src, 0x00);
    }

    #[test]
    fn short_payload_is_cut_not_overread() {
        let t = Telegram::decode(&[0xc1, 0x01, 0x01, 0x0b, 0, 0, 0, 0x45, 0x07, 0x01, 0x02]).unwrap();
        assert_eq!(t.payload, vec![0x02]);
        assert!(Telegram::decode(&[0; 9]).is_err());
    }
}
