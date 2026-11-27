//! Import the stock BeoSound 5 software's N.RADIO presets.
//!
//! BeoSound5.exe keeps N.RADIO in an SQLite database, `E:\Cache\Data\nradio.db` on the BM5's
//! data partition (`/dev/sda3`, label "Data"). Read from Tore's BM5 on 28 Nov 2026:
//!
//! - `favorite_list` rows 1-4 are the colour lists RED, GREEN, YELLOW, BLUE (99 items each);
//!   rows 5-103 are lists named `01` to `99` with room for one item each: these are the numbered
//!   presets a Beo4 or a link room's channel number selects.
//! - `favorite_list_item` holds the stations: `favorite_list_id`, `entity_type` 12 (a station),
//!   `entity_id` (a `station.id`), `entity_name`, `entity_url` (the stream), `entity_country`.
//!
//! Preset 8 there is Deutschlandfunk, which is what the Beosystem 3's wake-up timer got when it
//! asked for N.RADIO channel 8. The stream URL becomes the preset's URI: Music Assistant plays a
//! plain http(s) URL as a radio station (`builtin://radio/<url>`).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::{Preset, Presets, MAX_NUMBER};

/// `favorite_list_item.entity_type` of a radio station.
pub const ENTITY_STATION: i64 = 12;

/// What an import found.
#[derive(Debug, Default)]
pub struct Import {
    pub presets: Presets,
    /// Stations in the colour lists (RED, GREEN, YELLOW, BLUE), which have no numbers: listed so
    /// nothing is lost silently. `(list name, station name, url)`.
    pub colour_items: Vec<(String, String, String)>,
}

/// Read the numbered presets from a copy of `nradio.db` (opened read-only).
pub fn import(path: &Path) -> rusqlite::Result<Import> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let mut q = db.prepare(
        "SELECT l.default_name, i.entity_name, i.entity_url FROM favorite_list_item i \
         JOIN favorite_list l ON l.id = i.favorite_list_id \
         WHERE i.entity_type = ?1 ORDER BY l.id, i.id",
    )?;
    let rows = q.query_map([ENTITY_STATION], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    })?;
    let mut out = Import::default();
    for row in rows {
        let (list, name, url) = row?;
        match list.parse::<u8>() {
            Ok(n) if (1..=MAX_NUMBER).contains(&n) && list.len() == 2 => {
                // one item per numbered list; a second one would be odd, keep the first
                if out.presets.get(n).is_none() {
                    out.presets.set(Preset::new(n, name, url));
                }
            }
            _ => out.colour_items.push((list, name, url)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables as BeoSound5.exe made them, with Tore's lists 1, 5, 6 and 12 (presets 1, 8).
    #[test]
    fn numbered_lists_become_presets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nradio.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE favorite_list (id INTEGER PRIMARY KEY, default_name VARCHAR2, custom_name VARCHAR2, \
               default_size INTEGER, custom_size INTEGER, size INTEGER);
             CREATE TABLE favorite_list_item (id INTEGER PRIMARY KEY AUTOINCREMENT, favorite_list_id INTEGER, \
               added_time INTEGER, entity_type INTEGER, entity_subtype INTEGER, entity_id INTEGER, \
               entity_merge_id INTEGER, entity_checksum VARCHAR2, entity_guid VARCHAR2, entity_name VARCHAR2, \
               entity_country VARCHAR2, entity_language VARCHAR2, entity_url VARCHAR2, entity_logo VARCHAR2, \
               entity_beonet_data BLOB);
             INSERT INTO favorite_list VALUES (1,'RED','RED',99,99,1),(5,'01','01',1,1,1),(6,'02','02',1,1,0),
               (12,'08','08',1,1,1);
             INSERT INTO favorite_list_item (favorite_list_id, entity_type, entity_id, entity_name, entity_url) VALUES
               (5, 12, 499, 'BBC World Service', 'http://stream.live.vc.bbcmedia.co.uk/bbc_world_service'),
               (12, 12, 272, 'Deutschlandfunk', 'http://st01.sslstream.dlf.de/dlf/01/128/mp3/stream.mp3?aggregator=airable'),
               (1, 12, 411, 'NRK Nyheter', 'http://example/nyheter');",
        )
        .unwrap();
        drop(db);
        let i = import(&path).unwrap();
        let numbers: Vec<u8> = i.presets.iter().map(|p| p.number).collect();
        assert_eq!(numbers, [1, 8]);
        assert_eq!(i.presets.get(8).unwrap().name, "Deutschlandfunk");
        assert!(i.presets.get(8).unwrap().uri.starts_with("http://st01.sslstream.dlf.de/"));
        assert_eq!(i.colour_items, [("RED".into(), "NRK Nyheter".into(), "http://example/nyheter".into())]);
    }
}
