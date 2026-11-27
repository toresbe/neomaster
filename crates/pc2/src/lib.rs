//! Sans-IO driver for the B&O PC2 (the BeoMaster 5's Masterlink/Powerlink board, USB 0cd4:0101).
//!
//! A port of libpc2's `pc2/` (toresbe/libpc2 9656831). libpc2 ran a libusb thread, an event
//! thread, a pacer thread and a mailbox; here [`Pc2`] is a plain state machine. The USB actor
//! feeds it every IN packet with [`Pc2::on_usb_packet`], calls [`Pc2::tick`] now and then, and
//! writes whatever [`Pc2::poll_transmit`] returns to the OUT endpoint. Masterlink telegrams are
//! raw bytes here; the `masterlink` crate decodes and builds them.

pub mod beo4;
pub mod device;
pub mod fan;
pub mod frame;
pub mod mixer;
pub mod pacer;

pub use device::{Event, Mode, Pc2, Refused};
pub use fan::FanControl;
pub use frame::{FragmentAssembler, FrameError, Wakeup};
pub use mixer::{Mixer, MixerState};
pub use pacer::TelegramPacer;

/// One PC2 message without the `60 <len> ... 61` USB framing (libpc2 `PC2Message`).
pub type Message = Vec<u8>;

/// A monotonic timestamp, as a duration since an arbitrary fixed epoch.
pub type Time = std::time::Duration;

/// USB IDs of the PC2.
pub const VENDOR_ID: u16 = 0x0cd4;
pub const PRODUCT_ID: u16 = 0x0101;
/// Interrupt IN endpoint.
pub const ENDPOINT_IN: u8 = 0x81;
/// libpc2 writes to endpoint 0x01 (its comment: "USB_ENDPOINT_OUT is the wrong value. Why?").
pub const ENDPOINT_OUT: u8 = 0x01;
