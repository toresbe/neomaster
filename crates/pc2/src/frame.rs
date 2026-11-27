//! USB framing (`60 <len> ... 61`), reassembly of messages split over several USB packets, and
//! the raw wake-up packets. Port of libpc2 `pc2/fragment_assembler.*`, `pc2/wakeup.hpp` and the
//! framing in `PC2DeviceIO::write`.

use std::fmt;

pub const START: u8 = 0x60;
pub const END: u8 = 0x61;

/// Frame one message for the OUT endpoint: `60 <len> message 61`.
pub fn encode(message: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(message.len() + 3);
    v.push(START);
    v.push(message.len() as u8);
    v.extend_from_slice(message);
    v.push(END);
    v
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    TooShort,
    BadStart(u8),
    BadEnd(u8),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooShort => write!(f, "received message too short to be valid"),
            FrameError::BadStart(b) => write!(f, "message fragment starts with {b:#04x}, not 0x60"),
            FrameError::BadEnd(b) => write!(f, "assembled message ends with {b:#04x}, not 0x61"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Reassembles messages that the PC2 splits over several IN packets (libpc2
/// `PC2MessageFragmentAssembler`). A complete message keeps its `60 <len> ... 61` framing.
#[derive(Default)]
pub struct FragmentAssembler {
    remaining: usize,
    buffer: Vec<u8>,
}

impl FragmentAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while a multi-packet message is being reassembled.
    pub fn in_progress(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Add one IN packet. Returns the message once it is complete.
    pub fn push(&mut self, fragment: &[u8]) -> Result<Option<Vec<u8>>, FrameError> {
        // FIXME (from libpc2): a 60 ... 61 message can occur _inside_ a multi-part message.
        if !self.buffer.is_empty() {
            // continuation
            self.buffer.extend_from_slice(fragment);
            self.remaining = self.remaining.saturating_sub(fragment.len());
            if self.remaining == 0 {
                let msg = std::mem::take(&mut self.buffer);
                return Self::check_end(msg).map(Some);
            }
            return Ok(None);
        }
        // The minimum valid message is 3 bytes
        if fragment.len() < 3 {
            return Err(FrameError::TooShort);
        }
        if fragment[0] != START {
            return Err(FrameError::BadStart(fragment[0]));
        }
        let msg_length = fragment[1] as usize + 3;
        if msg_length > fragment.len() {
            self.buffer.extend_from_slice(fragment);
            self.remaining = msg_length - fragment.len();
            Ok(None)
        } else {
            Self::check_end(fragment.to_vec()).map(Some)
        }
    }

    fn check_end(msg: Vec<u8>) -> Result<Vec<u8>, FrameError> {
        match msg.last() {
            Some(&END) => Ok(msg),
            Some(&b) => Err(FrameError::BadEnd(b)),
            None => Err(FrameError::TooShort),
        }
    }
}

/// Raw 4-byte packets the PC2 sends around USB suspend, without `60 ... 61` framing.
///
/// From the PC2 firmware (BLMedia.hex v0236, see reports/pc2-control.md §9):
/// - `FF FF FF FF`: the PC2 woke the PC (USB remote wakeup) because of a remote key that its
///   option allows to wake it (A.OPT rules / CompatabilityEnabled);
/// - `FF FC FE FF`: the USB bus resumed (the PC woke up by itself).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wakeup {
    ByRemote,
    BusResumed,
}

pub fn wakeup_packet(fragment: &[u8]) -> Option<Wakeup> {
    match fragment {
        [0xff, 0xff, 0xff, 0xff] => Some(Wakeup::ByRemote),
        [0xff, 0xfc, 0xfe, 0xff] => Some(Wakeup::BusResumed),
        _ => None,
    }
}

/// libpc2 `PC2DeviceIO::read_callback` drops this packet ("Got bogus message; ignoring...").
pub const BOGUS_PACKET: [u8; 5] = [0xff, 0xff, 0x01, 0xff, 0xff];
