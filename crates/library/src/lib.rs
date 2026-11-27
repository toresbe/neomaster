//! Local mirror of the Music Assistant library and the cover atlas (milestone M3, not started).
//!
//! Planned: SQLite with artists, albums, tracks, playlists, stations and favourites (MA item URI,
//! display name, precomputed sort key, cover id, average colour), a letter index per list, and a
//! cover atlas pre-scaled into fixed size steps from MA's `/imageproxy`, so that drawing never
//! scales or decodes.
