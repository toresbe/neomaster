//! Local mirror of the Music Assistant library for latency-free browsing,
//! plus a pre-baked cover atlas.
//!
//! * [`Store`]: SQLite (WAL) with artists, albums, tracks, playlists (and
//!   their tracks) and radio stations, each with its MA URI, display name,
//!   sort key, first letter and cover id. Readers open their own connection
//!   and never wait on the network or on the sync writer.
//! * [`sync::Syncer`]: full pull from MA on start and after every reconnect
//!   or `music_sync_completed`, incremental updates from `media_item_*`
//!   events in between.
//! * [`covers`]: fetches each cover once from `/imageproxy/<id>`, keeps the
//!   source JPEG on disk, and computes an average colour for fast-scroll
//!   placeholders.
//! * [`atlas`]: bakes every cover into fixed-size slots, one memory-mapped
//!   file per size step, in browse order (albums, then artists, playlists,
//!   radio, tracks; each by sort key), in the framebuffer's pixel format.

pub mod atlas;
pub mod covers;
mod sort;
mod store;
pub mod sync;

pub use atlas::{Atlas, AtlasConfig, PixelFormat};
pub use sort::{letter_of, sort_key};
pub use store::{Kind, LetterIndex, Row, Store};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("music assistant: {0}")]
    Ma(#[from] ma_client::Error),
    #[error("http: {0}")]
    Http(String),
    #[error("image: {0}")]
    Image(String),
    #[error("atlas: {0}")]
    Atlas(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
