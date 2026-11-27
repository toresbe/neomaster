//! Sans-IO Masterlink for neomaster: bytes and time in, effects out.
//!
//! A port of libpc2's `masterlink/` (toresbe/libpc2 9656831). [`AudioMaster`] and
//! [`SourceCenter`] keep libpc2's structure (`handle(telegram, now)`, `tick(now)`, a host for
//! everything they do) so that the capture replays in `tests/` carry over byte for byte. The C++
//! mutexes are gone: one task owns each state machine.
//!
//! Time is a [`Time`]: a monotonic timestamp measured from any fixed epoch (process start in
//! neomaster, zero in tests).

pub mod audio_master;
pub mod host;
pub mod protocol;
pub mod source_center;
pub mod sources;
pub mod telegram;
pub mod track_info;

pub use audio_master::{AudioMaster, BusState, LockMode};
pub use host::{Effect, Effects, Host};
pub use source_center::SourceCenter;
pub use sources::{LinkRole, LocalSource, SourceMap, AIRPLAY_SLOT};
pub use telegram::Telegram;
pub use track_info::TrackInfo;

/// A monotonic timestamp, as a duration since an arbitrary fixed epoch.
pub type Time = std::time::Duration;
