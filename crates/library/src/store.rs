use std::path::Path;

use ma_client::{Album, Artist, ItemMapping, MediaItem, MediaType, Playlist, Radio, Track};
use rusqlite::{params, Connection, OptionalExtension};

use crate::sort::{letter_of, sort_key};
use crate::Result;

/// The browsable item kinds. Stored as integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Artist = 1,
    Album = 2,
    Track = 3,
    Playlist = 4,
    Radio = 5,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Artist, Kind::Album, Kind::Track, Kind::Playlist, Kind::Radio];

    pub fn from_i64(v: i64) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| *k as i64 == v)
    }

    pub fn media_type(self) -> MediaType {
        match self {
            Kind::Artist => MediaType::Artist,
            Kind::Album => MediaType::Album,
            Kind::Track => MediaType::Track,
            Kind::Playlist => MediaType::Playlist,
            Kind::Radio => MediaType::Radio,
        }
    }

    pub fn from_media_type(mt: MediaType) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.media_type() == mt)
    }

    /// `library://album/131` → (Album, "131").
    pub fn parse_uri(uri: &str) -> Option<(Kind, &str)> {
        let rest = uri.strip_prefix("library://")?;
        let (t, id) = rest.split_once('/')?;
        let kind = match t {
            "artist" => Kind::Artist,
            "album" => Kind::Album,
            "track" => Kind::Track,
            "playlist" => Kind::Playlist,
            "radio" => Kind::Radio,
            _ => return None,
        };
        Some((kind, id))
    }
}

/// One row as the UI shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: Kind,
    pub item_id: String,
    /// MA URI to pass to `play_media`.
    pub uri: String,
    pub name: String,
    /// Artist names (albums, tracks), owner (playlists).
    pub subtitle: String,
    pub letter: String,
    pub year: Option<i32>,
    /// Seconds.
    pub duration: Option<f64>,
    pub favorite: bool,
    /// MA image proxy id of the cover.
    pub cover: Option<String>,
    /// Slot of the cover in the atlas files of the current generation.
    pub cover_slot: Option<u32>,
    /// Average cover colour, 0xRRGGBB, for fast-scroll placeholders.
    pub cover_rgb: Option<u32>,
}

/// Letter-jump index: each letter with the list position of its first row.
pub type LetterIndex = Vec<(String, u32)>;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS items (
    kind      INTEGER NOT NULL,
    item_id   TEXT NOT NULL,
    uri       TEXT NOT NULL,
    name      TEXT NOT NULL,
    version   TEXT NOT NULL DEFAULT '',
    sort_key  TEXT NOT NULL,
    letter    TEXT NOT NULL,
    subtitle  TEXT NOT NULL DEFAULT '',
    album_id  TEXT,
    disc      INTEGER,
    track_no  INTEGER,
    year      INTEGER,
    duration  REAL,
    favorite  INTEGER NOT NULL DEFAULT 0,
    cover     TEXT,
    seen      INTEGER NOT NULL DEFAULT 0,
    pos       INTEGER,                      -- list position, see renumber()
    PRIMARY KEY (kind, item_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS items_sort ON items (kind, sort_key, name, item_id);
CREATE INDEX IF NOT EXISTS items_pos ON items (kind, pos);
CREATE INDEX IF NOT EXISTS items_album ON items (album_id, disc, track_no) WHERE album_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS item_artists (
    kind      INTEGER NOT NULL,
    item_id   TEXT NOT NULL,
    pos       INTEGER NOT NULL,
    artist_id TEXT NOT NULL,
    PRIMARY KEY (kind, item_id, pos)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS item_artists_artist ON item_artists (artist_id, kind);
CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id TEXT NOT NULL,
    pos         INTEGER NOT NULL,
    uri         TEXT NOT NULL,
    name        TEXT NOT NULL,
    subtitle    TEXT NOT NULL DEFAULT '',
    duration    REAL,
    cover       TEXT,
    PRIMARY KEY (playlist_id, pos)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS covers (
    proxy_id   TEXT PRIMARY KEY,
    state      INTEGER NOT NULL DEFAULT 0,  -- 0 wanted, 1 fetched, 2 failed
    avg_rgb    INTEGER,
    slot       INTEGER,
    fetched_at INTEGER
) WITHOUT ROWID;
"#;

/// Album/track ordering used everywhere a list is shown.
const ORDER: &str = "ORDER BY i.sort_key, i.name, i.item_id";

const ROW_COLS: &str =
    "i.kind, i.item_id, i.uri, i.name, i.subtitle, i.letter, i.year, i.duration, i.favorite, i.cover, c.slot, c.avg_rgb";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Row> {
    Ok(Row {
        kind: Kind::from_i64(r.get(0)?).unwrap_or(Kind::Track),
        item_id: r.get(1)?,
        uri: r.get(2)?,
        name: r.get(3)?,
        subtitle: r.get(4)?,
        letter: r.get(5)?,
        year: r.get(6)?,
        duration: r.get(7)?,
        favorite: r.get::<_, i64>(8)? != 0,
        cover: r.get(9)?,
        cover_slot: r.get::<_, Option<i64>>(10)?.map(|v| v as u32),
        cover_rgb: r.get::<_, Option<i64>>(11)?.map(|v| v as u32),
    })
}

/// A connection to the mirror. Open one for the sync writer and one per
/// reading thread; WAL lets readers run while the writer commits.
pub struct Store {
    conn: Connection,
}

/// Fields common to all item kinds, flattened for upsert.
struct Flat<'a> {
    kind: Kind,
    item_id: &'a str,
    uri: &'a str,
    name: &'a str,
    version: &'a str,
    sort_name: &'a str,
    subtitle: String,
    album_id: Option<&'a str>,
    disc: Option<i32>,
    track_no: Option<i32>,
    year: Option<i32>,
    duration: Option<f64>,
    favorite: bool,
    cover: Option<&'a str>,
    artists: &'a [ItemMapping],
}

fn artist_names(a: &[ItemMapping]) -> String {
    a.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
}

fn flatten(item: &MediaItem) -> Option<Flat<'_>> {
    let cover = item.cover().and_then(|i| i.proxy_id.as_deref());
    Some(match item {
        MediaItem::Artist(Artist { item_id, uri, name, version, sort_name, favorite, .. }) => Flat {
            kind: Kind::Artist,
            item_id,
            uri,
            name,
            version,
            sort_name,
            subtitle: String::new(),
            album_id: None,
            disc: None,
            track_no: None,
            year: None,
            duration: None,
            favorite: *favorite,
            cover,
            artists: &[],
        },
        MediaItem::Album(Album { item_id, uri, name, version, sort_name, favorite, year, artists, .. }) => Flat {
            kind: Kind::Album,
            item_id,
            uri,
            name,
            version,
            sort_name,
            subtitle: artist_names(artists),
            album_id: None,
            disc: None,
            track_no: None,
            year: *year,
            duration: None,
            favorite: *favorite,
            cover,
            artists,
        },
        MediaItem::Track(Track {
            item_id,
            uri,
            name,
            version,
            sort_name,
            favorite,
            duration,
            artists,
            album,
            disc_number,
            track_number,
            ..
        }) => Flat {
            kind: Kind::Track,
            item_id,
            uri,
            name,
            version,
            sort_name,
            subtitle: artist_names(artists),
            album_id: album.as_ref().filter(|a| a.uri.starts_with("library://")).map(|a| a.item_id.as_str()),
            disc: *disc_number,
            track_no: *track_number,
            year: album.as_ref().and_then(|a| a.year),
            duration: *duration,
            favorite: *favorite,
            cover,
            artists,
        },
        MediaItem::Playlist(Playlist { item_id, uri, name, version, sort_name, favorite, owner, .. }) => Flat {
            kind: Kind::Playlist,
            item_id,
            uri,
            name,
            version,
            sort_name,
            subtitle: owner.clone(),
            album_id: None,
            disc: None,
            track_no: None,
            year: None,
            duration: None,
            favorite: *favorite,
            cover,
            artists: &[],
        },
        MediaItem::Radio(Radio { item_id, uri, name, version, sort_name, favorite, .. }) => Flat {
            kind: Kind::Radio,
            item_id,
            uri,
            name,
            version,
            sort_name,
            subtitle: String::new(),
            album_id: None,
            disc: None,
            track_no: None,
            year: None,
            duration: None,
            favorite: *favorite,
            cover,
            artists: &[],
        },
        MediaItem::Other => return None,
    })
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    // ---- meta ----------------------------------------------------------

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Generation of the atlas files that `cover_slot` refers to.
    pub fn atlas_generation(&self) -> Result<u64> {
        Ok(self.meta("atlas_generation")?.and_then(|v| v.parse().ok()).unwrap_or(0))
    }

    // ---- writing -------------------------------------------------------

    /// Insert or update one item (and its artist links), tagging it with
    /// sync generation `seen`. Items not in the library (no `library://`
    /// URI) are ignored. Returns whether it was stored.
    pub fn upsert(&self, item: &MediaItem, seen: i64) -> Result<bool> {
        let Some(f) = flatten(item) else {
            return Ok(false);
        };
        if !f.uri.starts_with("library://") {
            return Ok(false);
        }
        let key = sort_key(f.sort_name, f.name);
        let letter = letter_of(&key);
        let mut st = self.conn.prepare_cached(
            "INSERT INTO items (kind, item_id, uri, name, version, sort_key, letter, subtitle, album_id, disc, track_no, year, duration, favorite, cover, seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(kind, item_id) DO UPDATE SET uri=excluded.uri, name=excluded.name, version=excluded.version,
               sort_key=excluded.sort_key, letter=excluded.letter, subtitle=excluded.subtitle, album_id=excluded.album_id,
               disc=excluded.disc, track_no=excluded.track_no, year=excluded.year, duration=excluded.duration,
               favorite=excluded.favorite, cover=excluded.cover, seen=excluded.seen",
        )?;
        st.execute(params![
            f.kind as i64,
            f.item_id,
            f.uri,
            f.name,
            f.version,
            key,
            letter,
            f.subtitle,
            f.album_id,
            f.disc,
            f.track_no,
            f.year,
            f.duration,
            f.favorite,
            f.cover,
            seen
        ])?;
        let kind = f.kind as i64;
        self.conn.prepare_cached("DELETE FROM item_artists WHERE kind=?1 AND item_id=?2")?.execute(params![kind, f.item_id])?;
        let mut ins = self
            .conn
            .prepare_cached("INSERT OR IGNORE INTO item_artists (kind, item_id, pos, artist_id) VALUES (?1, ?2, ?3, ?4)")?;
        for (pos, a) in f.artists.iter().enumerate().filter(|(_, a)| a.uri.starts_with("library://")) {
            ins.execute(params![kind, f.item_id, pos as i64, a.item_id])?;
        }
        if let Some(c) = f.cover {
            self.conn.prepare_cached("INSERT OR IGNORE INTO covers (proxy_id) VALUES (?1)")?.execute([c])?;
        }
        Ok(true)
    }

    pub fn delete(&self, kind: Kind, item_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM items WHERE kind=?1 AND item_id=?2", params![kind as i64, item_id])?;
        self.conn.execute("DELETE FROM item_artists WHERE kind=?1 AND item_id=?2", params![kind as i64, item_id])?;
        if kind == Kind::Playlist {
            self.conn.execute("DELETE FROM playlist_tracks WHERE playlist_id=?1", [item_id])?;
        }
        Ok(())
    }

    /// Drop every item of `kind` not seen in sync generation `seen`.
    pub fn delete_unseen(&self, kind: Kind, seen: i64) -> Result<usize> {
        let k = kind as i64;
        self.conn.execute(
            "DELETE FROM item_artists WHERE kind=?1 AND item_id IN (SELECT item_id FROM items WHERE kind=?1 AND seen<?2)",
            params![k, seen],
        )?;
        if kind == Kind::Playlist {
            self.conn.execute(
                "DELETE FROM playlist_tracks WHERE playlist_id IN (SELECT item_id FROM items WHERE kind=?1 AND seen<?2)",
                params![k, seen],
            )?;
        }
        Ok(self.conn.execute("DELETE FROM items WHERE kind=?1 AND seen<?2", params![k, seen])?)
    }

    /// Recompute list positions of one kind. Called after every batch of
    /// writes, so list windows are an index range instead of an OFFSET scan.
    pub fn renumber(&self, kind: Kind) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET pos = r.rn FROM (
                SELECT item_id, ROW_NUMBER() OVER (ORDER BY sort_key, name, item_id) - 1 AS rn
                FROM items WHERE kind = ?1) AS r
             WHERE items.kind = ?1 AND items.item_id = r.item_id AND items.pos IS NOT r.rn",
            [kind as i64],
        )?;
        Ok(())
    }

    pub fn replace_playlist_tracks(&self, playlist_id: &str, tracks: &[Track]) -> Result<()> {
        self.conn.execute("DELETE FROM playlist_tracks WHERE playlist_id=?1", [playlist_id])?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        self.mark_playlist_fetched(playlist_id, now)?;
        let mut st = self.conn.prepare_cached(
            "INSERT OR REPLACE INTO playlist_tracks (playlist_id, pos, uri, name, subtitle, duration, cover) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut cov = self.conn.prepare_cached("INSERT OR IGNORE INTO covers (proxy_id) VALUES (?1)")?;
        for (i, t) in tracks.iter().enumerate() {
            let item = MediaItem::Track(t.clone());
            let cover = item.cover().and_then(|c| c.proxy_id.clone());
            let pos = t.position.unwrap_or(i as i64);
            st.execute(params![playlist_id, pos, t.uri, t.name, artist_names(&t.artists), t.duration, cover])?;
            if let Some(c) = &cover {
                cov.execute([c])?;
            }
        }
        Ok(())
    }

    /// Run `f` in one transaction (much faster for bulk writes).
    pub fn transaction<T>(&mut self, f: impl FnOnce(&Store) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match f(self) {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    // ---- covers --------------------------------------------------------

    /// Covers referenced by the library that have not been fetched yet
    /// (`retry_failed` also returns ones that failed before).
    pub fn wanted_covers(&self, retry_failed: bool) -> Result<Vec<String>> {
        let sql = if retry_failed {
            "SELECT proxy_id FROM covers WHERE state != 1"
        } else {
            "SELECT proxy_id FROM covers WHERE state = 0"
        };
        let mut st = self.conn.prepare(sql)?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn set_cover_fetched(&self, proxy_id: &str, avg_rgb: u32, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO covers (proxy_id, state, avg_rgb, fetched_at) VALUES (?1, 1, ?2, ?3)
             ON CONFLICT(proxy_id) DO UPDATE SET state=1, avg_rgb=excluded.avg_rgb, fetched_at=excluded.fetched_at",
            params![proxy_id, avg_rgb as i64, now],
        )?;
        Ok(())
    }

    pub fn set_cover_failed(&self, proxy_id: &str, now: i64) -> Result<()> {
        self.conn.execute("UPDATE covers SET state=2, fetched_at=?2 WHERE proxy_id=?1", params![proxy_id, now])?;
        Ok(())
    }

    /// Fetched covers in browse order, each once: albums, artists,
    /// playlists, radio, then tracks and playlist entries whose cover is
    /// not already in (singles); each group by sort key.
    pub fn covers_in_browse_order(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT i.cover FROM items i JOIN covers c ON c.proxy_id = i.cover AND c.state = 1
             ORDER BY CASE i.kind WHEN {a} THEN 0 WHEN {r} THEN 1 WHEN {p} THEN 2 WHEN {ra} THEN 3 ELSE 4 END,
                      i.sort_key, i.name, i.item_id",
            a = Kind::Album as i64,
            r = Kind::Artist as i64,
            p = Kind::Playlist as i64,
            ra = Kind::Radio as i64
        ))?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for c in st.query_map([], |r| r.get::<_, String>(0))? {
            let c = c?;
            if seen.insert(c.clone()) {
                out.push(c);
            }
        }
        let mut st = self.conn.prepare(
            "SELECT p.cover FROM playlist_tracks p JOIN covers c ON c.proxy_id = p.cover AND c.state = 1 ORDER BY p.playlist_id, p.pos",
        )?;
        for c in st.query_map([], |r| r.get::<_, String>(0))? {
            let c = c?;
            if seen.insert(c.clone()) {
                out.push(c);
            }
        }
        Ok(out)
    }

    /// Record the slots of a freshly baked atlas generation.
    pub fn set_cover_slots(&mut self, order: &[String], generation: u64) -> Result<()> {
        self.transaction(|s| {
            s.conn.execute("UPDATE covers SET slot = NULL", [])?;
            let mut st = s.conn.prepare_cached("UPDATE covers SET slot=?2 WHERE proxy_id=?1")?;
            for (i, c) in order.iter().enumerate() {
                st.execute(params![c, i as i64])?;
            }
            s.set_meta("atlas_generation", &generation.to_string())
        })
    }

    pub fn cover_slot(&self, proxy_id: &str) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row("SELECT slot FROM covers WHERE proxy_id=?1", [proxy_id], |r| r.get::<_, Option<i64>>(0))
            .optional()?
            .flatten()
            .map(|v| v as u32))
    }

    // ---- reading -------------------------------------------------------

    pub fn count(&self, kind: Kind) -> Result<u32> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM items WHERE kind=?1", [kind as i64], |r| r.get(0))?)
    }

    /// Rows `offset..offset+limit` of a whole list in browse order. The UI
    /// asks only for the window around its cursor.
    pub fn list(&self, kind: Kind, offset: u32, limit: u32) -> Result<Vec<Row>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM items i LEFT JOIN covers c ON c.proxy_id = i.cover
             WHERE i.kind=?1 AND i.pos >= ?2 AND i.pos < ?3 ORDER BY i.pos"
        ))?;
        let v =
            st.query_map(params![kind as i64, offset, offset as i64 + limit as i64], row)?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn favorites(&self, kind: Kind) -> Result<Vec<Row>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM items i LEFT JOIN covers c ON c.proxy_id = i.cover
             WHERE i.kind=?1 AND i.favorite=1 {ORDER}"
        ))?;
        let v = st.query_map(params![kind as i64], row)?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub fn get(&self, kind: Kind, item_id: &str) -> Result<Option<Row>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {ROW_COLS} FROM items i LEFT JOIN covers c ON c.proxy_id = i.cover WHERE i.kind=?1 AND i.item_id=?2"
                ),
                params![kind as i64, item_id],
                row,
            )
            .optional()?)
    }

    /// List position of an item (for restoring a cursor).
    pub fn position(&self, kind: Kind, item_id: &str) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row("SELECT pos FROM items WHERE kind=?1 AND item_id=?2", params![kind as i64, item_id], |r| r.get(0))
            .optional()?)
    }

    /// Letters of a list with the position of each letter's first row.
    pub fn letter_index(&self, kind: Kind) -> Result<LetterIndex> {
        let mut st = self.conn.prepare_cached("SELECT letter, pos FROM items WHERE kind=?1 ORDER BY pos")?;
        let mut out: LetterIndex = Vec::new();
        for r in st.query_map([kind as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))? {
            let (l, i) = r?;
            if out.last().map(|(p, _)| p != &l).unwrap_or(true) {
                out.push((l, i));
            }
        }
        Ok(out)
    }

    /// Albums by an artist (album artist or credited), newest first.
    pub fn artist_albums(&self, artist_id: &str) -> Result<Vec<Row>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM items i LEFT JOIN covers c ON c.proxy_id = i.cover
             WHERE i.kind={album} AND i.item_id IN (SELECT item_id FROM item_artists WHERE artist_id=?1 AND kind={album})
             ORDER BY i.year DESC NULLS LAST, i.sort_key, i.item_id",
            album = Kind::Album as i64
        ))?;
        let v = st.query_map([artist_id], row)?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Library tracks by an artist, alphabetically.
    pub fn artist_tracks(&self, artist_id: &str) -> Result<Vec<Row>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM items i LEFT JOIN covers c ON c.proxy_id = i.cover
             WHERE i.kind={t} AND i.item_id IN (SELECT item_id FROM item_artists WHERE artist_id=?1 AND kind={t}) {ORDER}",
            t = Kind::Track as i64
        ))?;
        let v = st.query_map([artist_id], row)?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Library tracks of an album in disc/track order. Only tracks that are
    /// in the library; use `ma_client::Client::album_tracks` for the rest.
    pub fn album_tracks(&self, album_id: &str) -> Result<Vec<Row>> {
        let mut st = self.conn.prepare_cached(&format!(
            "SELECT {ROW_COLS} FROM items i LEFT JOIN covers c ON c.proxy_id = i.cover
             WHERE i.kind={t} AND i.album_id=?1 ORDER BY i.disc, i.track_no, i.sort_key",
            t = Kind::Track as i64
        ))?;
        let v = st.query_map([album_id], row)?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Mirrored tracks of a playlist, in playlist order.
    pub fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Row>> {
        let mut st = self.conn.prepare_cached(
            "SELECT p.uri, p.name, p.subtitle, p.duration, p.cover, c.slot, c.avg_rgb
             FROM playlist_tracks p LEFT JOIN covers c ON c.proxy_id = p.cover
             WHERE p.playlist_id=?1 ORDER BY p.pos",
        )?;
        let v = st
            .query_map([playlist_id], |r| {
                let uri: String = r.get(0)?;
                let item_id = uri.rsplit('/').next().unwrap_or("").to_owned();
                let name: String = r.get(1)?;
                Ok(Row {
                    kind: Kind::Track,
                    item_id,
                    letter: letter_of(&name.to_lowercase()),
                    uri,
                    name,
                    subtitle: r.get(2)?,
                    year: None,
                    duration: r.get(3)?,
                    favorite: false,
                    cover: r.get(4)?,
                    cover_slot: r.get::<_, Option<i64>>(5)?.map(|v| v as u32),
                    cover_rgb: r.get::<_, Option<i64>>(6)?.map(|v| v as u32),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    /// Whether a playlist's contents have been fetched (or tried), even if
    /// MA returned nothing.
    pub fn has_playlist_tracks(&self, playlist_id: &str) -> Result<bool> {
        Ok(self.meta(&format!("playlist_fetched:{playlist_id}"))?.is_some())
    }

    /// Record that a playlist was fetched (or failed) at `now`, so a
    /// resync does not ask MA again until an explicit full sync.
    pub fn mark_playlist_fetched(&self, playlist_id: &str, now: i64) -> Result<()> {
        self.set_meta(&format!("playlist_fetched:{playlist_id}"), &now.to_string())
    }

    /// Non-dynamic playlists: (item_id, provider of the library item).
    pub fn playlist_ids(&self) -> Result<Vec<String>> {
        let mut st =
            self.conn.prepare(&format!("SELECT i.item_id FROM items i WHERE i.kind={} {ORDER}", Kind::Playlist as i64))?;
        let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(v: serde_json::Value) -> MediaItem {
        serde_json::from_value(v).unwrap()
    }

    fn sample(s: &mut Store) {
        s.transaction(|s| {
            for v in [
                json!({"media_type": "artist", "item_id": "1", "uri": "library://artist/1", "name": "ABBA", "sort_name": "abba"}),
                json!({"media_type": "artist", "item_id": "2", "uri": "library://artist/2", "name": "50 Cent", "sort_name": "50 cent"}),
                json!({"media_type": "artist", "item_id": "3", "uri": "library://artist/3", "name": "Aha", "sort_name": "aha"}),
                json!({"media_type": "artist", "item_id": "4", "uri": "library://artist/4", "name": "Björk", "sort_name": "bjork"}),
                json!({"media_type": "album", "item_id": "10", "uri": "library://album/10", "name": "Arrival", "sort_name": "arrival", "year": 1976,
                       "artists": [{"item_id": "1", "name": "ABBA", "uri": "library://artist/1"}],
                       "metadata": {"images": [{"type": "thumb", "proxy_id": "cov10"}]}}),
                json!({"media_type": "track", "item_id": "100", "uri": "library://track/100", "name": "Dancing Queen", "sort_name": "dancing queen",
                       "duration": 231, "track_number": 2, "disc_number": 1,
                       "artists": [{"item_id": "1", "name": "ABBA", "uri": "library://artist/1"}],
                       "album": {"item_id": "10", "name": "Arrival", "uri": "library://album/10", "image": {"type": "thumb", "proxy_id": "cov10"}}}),
                json!({"media_type": "track", "item_id": "101", "uri": "library://track/101", "name": "When I Kissed the Teacher", "track_number": 1, "disc_number": 1,
                       "artists": [{"item_id": "1", "name": "ABBA", "uri": "library://artist/1"}],
                       "album": {"item_id": "10", "name": "Arrival", "uri": "library://album/10"}}),
                json!({"media_type": "track", "item_id": "x", "uri": "spotify://track/x", "name": "Not in library"}),
            ] {
                s.upsert(&item(v), 1)?;
            }
            for k in Kind::ALL {
                s.renumber(k)?;
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn lists_and_letters() {
        let mut s = Store::open_in_memory().unwrap();
        sample(&mut s);
        assert_eq!(s.count(Kind::Artist).unwrap(), 4);
        assert_eq!(s.count(Kind::Track).unwrap(), 2);
        let names: Vec<_> = s.list(Kind::Artist, 0, 10).unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["50 Cent", "ABBA", "Aha", "Björk"]);
        assert_eq!(s.list(Kind::Artist, 1, 2).unwrap().len(), 2);
        assert_eq!(s.letter_index(Kind::Artist).unwrap(), vec![("#".to_string(), 0), ("A".to_string(), 1), ("B".to_string(), 3)]);
        assert_eq!(s.position(Kind::Artist, "4").unwrap(), Some(3));
        let tracks: Vec<_> = s.album_tracks("10").unwrap().into_iter().map(|r| r.item_id).collect();
        assert_eq!(tracks, ["101", "100"]);
        assert_eq!(s.artist_albums("1").unwrap()[0].subtitle, "ABBA");
        assert_eq!(s.artist_tracks("1").unwrap().len(), 2);
        let t = s.get(Kind::Track, "100").unwrap().unwrap();
        assert_eq!(t.cover.as_deref(), Some("cov10"));
        assert_eq!(t.year, None);
        assert_eq!(s.wanted_covers(false).unwrap(), vec!["cov10".to_string()]);
    }

    #[test]
    fn resync_drops_unseen_and_slots() {
        let mut s = Store::open_in_memory().unwrap();
        sample(&mut s);
        s.upsert(&item(json!({"media_type": "artist", "item_id": "1", "uri": "library://artist/1", "name": "ABBA"})), 2).unwrap();
        assert_eq!(s.delete_unseen(Kind::Artist, 2).unwrap(), 3);
        s.renumber(Kind::Artist).unwrap();
        assert_eq!(s.position(Kind::Artist, "1").unwrap(), Some(0));
        assert_eq!(s.count(Kind::Artist).unwrap(), 1);

        assert!(s.covers_in_browse_order().unwrap().is_empty());
        s.set_cover_fetched("cov10", 0x112233, 0).unwrap();
        let order = s.covers_in_browse_order().unwrap();
        assert_eq!(order, vec!["cov10".to_string()]);
        s.set_cover_slots(&order, 7).unwrap();
        assert_eq!(s.atlas_generation().unwrap(), 7);
        let a = s.get(Kind::Album, "10").unwrap().unwrap();
        assert_eq!((a.cover_slot, a.cover_rgb), (Some(0), Some(0x112233)));
    }

    #[test]
    fn uri_parsing() {
        assert_eq!(Kind::parse_uri("library://album/131"), Some((Kind::Album, "131")));
        assert_eq!(Kind::parse_uri("spotify://album/131"), None);
    }
}
