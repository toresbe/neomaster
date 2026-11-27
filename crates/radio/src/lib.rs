//! N.RADIO numbered presets.
//!
//! A B&O system picks a radio station by number: Beo4 digits in the room, a link room's
//! SOURCE_CONNECT (a wake-up timer asks for "N.RADIO 8"), the room's ▲ ▼ keys. The stock
//! BeoMaster 5 numbered the stations of its N.RADIO favourites (see `doc/n-radio-presets.md`);
//! neomaster keeps the same numbers and plays each one as a Music Assistant radio station.
//!
//! - [`Presets`] is the numbered list: number, name, MA URI. It is plain serde data, so it can
//!   live in its own file ([`store`]) now and move into the settings file unchanged later.
//! - [`Tuner`] is sans-IO: channel numbers, Beo4 keys and time in, [`Tune`] (which preset to
//!   play) out. It also remembers the last preset, which N.RADIO resumes when picked without a
//!   number.
//! - [`ma`] (feature `ma`) resolves a preset that has only a name to an MA station and plays a
//!   preset on a queue.

pub mod presets;
pub mod store;
mod tuner;

#[cfg(feature = "ma")]
pub mod ma;

pub use presets::{Preset, Presets, MAX_NUMBER};
pub use tuner::{Tune, Tuner, DIGIT_TIMEOUT};
