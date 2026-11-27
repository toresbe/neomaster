//! Music Assistant (MA) WebSocket API client.
//!
//! One [`Client`] keeps a single WebSocket to MA (`ws://host:8095/ws`),
//! authenticates with a long-lived token, matches command replies by
//! `message_id`, reassembles `partial` list results, and reconnects with
//! backoff. Server events arrive on the [`Event`] channel returned by
//! [`Client::connect`], together with `Connected`/`Disconnected` markers so a
//! consumer knows when to resync.
//!
//! Commands are plain `(name, args)` pairs ([`Client::command`]); the typed
//! helpers in [`api`] cover what neomaster needs: library listing, players,
//! queues, playback and transport.
//!
//! Checked against MA server 2.10.13 (schema 65).

pub mod api;
mod client;
pub mod events;
pub mod models;

pub use client::{Client, Config, ConnState, Error, Result};
pub use events::Event;
pub use models::*;
