//! Typed subset of MA's data models: only the fields neomaster uses.
//! Unknown fields are ignored and missing or `null` fields fall back to
//! defaults, so newer servers keep decoding.

use serde::{Deserialize, Deserializer, Serialize};

/// `null` → `T::default()`.
fn nullable<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ServerInfo {
    pub server_id: String,
    pub server_version: String,
    pub schema_version: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub base_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Artist,
    Album,
    Track,
    Playlist,
    Radio,
    Audiobook,
    Podcast,
    PodcastEpisode,
    Genre,
    Folder,
    #[serde(other)]
    Unknown,
}

impl MediaType {
    /// The path segment MA uses for this type in `music/<x>/...` commands.
    pub fn collection(self) -> &'static str {
        match self {
            MediaType::Artist => "artists",
            MediaType::Album => "albums",
            MediaType::Track => "tracks",
            MediaType::Playlist => "playlists",
            MediaType::Radio => "radios",
            MediaType::Audiobook => "audiobooks",
            MediaType::Podcast => "podcasts",
            MediaType::Genre => "genres",
            _ => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct MediaItemImage {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default, deserialize_with = "nullable")]
    pub path: String,
    #[serde(default, deserialize_with = "nullable")]
    pub provider: String,
    #[serde(default)]
    pub remotely_accessible: bool,
    /// Key for `GET /imageproxy/<proxy_id>?size=N&fmt=jpeg`.
    #[serde(default)]
    pub proxy_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct Metadata {
    #[serde(default, deserialize_with = "nullable")]
    pub images: Vec<MediaItemImage>,
}

/// Best cover in a list of images: the first `thumb` with a proxy id, else
/// any image with a proxy id.
pub fn pick_cover(images: &[MediaItemImage]) -> Option<&MediaItemImage> {
    images.iter().find(|i| i.kind == "thumb" && i.proxy_id.is_some()).or_else(|| images.iter().find(|i| i.proxy_id.is_some()))
}

/// A lightweight reference to another item (album of a track, artists of
/// an album).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct ItemMapping {
    pub item_id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub sort_name: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default, deserialize_with = "nullable")]
    pub version: String,
    #[serde(default)]
    pub image: Option<MediaItemImage>,
    #[serde(default)]
    pub year: Option<i32>,
}

macro_rules! common_fields {
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $(#[$m])*
        pub struct $name {
            pub item_id: String,
            #[serde(default)]
            pub provider: String,
            #[serde(default, deserialize_with = "nullable")]
            pub name: String,
            #[serde(default, deserialize_with = "nullable")]
            pub sort_name: String,
            #[serde(default, deserialize_with = "nullable")]
            pub version: String,
            #[serde(default)]
            pub uri: String,
            #[serde(default)]
            pub favorite: bool,
            #[serde(default, deserialize_with = "nullable")]
            pub metadata: Metadata,
            $($body)*
        }
    };
}

common_fields! {
    #[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
    pub struct Artist {}
}

common_fields! {
    #[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
    pub struct Album {
        #[serde(default)]
        pub year: Option<i32>,
        #[serde(default, deserialize_with = "nullable")]
        pub artists: Vec<ItemMapping>,
        #[serde(default, deserialize_with = "nullable")]
        pub album_type: String,
    }
}

common_fields! {
    #[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
    pub struct Track {
        /// Seconds.
        #[serde(default)]
        pub duration: Option<f64>,
        #[serde(default, deserialize_with = "nullable")]
        pub artists: Vec<ItemMapping>,
        #[serde(default)]
        pub album: Option<ItemMapping>,
        #[serde(default)]
        pub disc_number: Option<i32>,
        #[serde(default)]
        pub track_number: Option<i32>,
        /// Position within a playlist, when listed from one.
        #[serde(default)]
        pub position: Option<i64>,
    }
}

common_fields! {
    #[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
    pub struct Playlist {
        #[serde(default, deserialize_with = "nullable")]
        pub owner: String,
        #[serde(default)]
        pub is_dynamic: bool,
        #[serde(default)]
        pub is_editable: bool,
    }
}

common_fields! {
    #[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
    pub struct Radio {}
}

/// Any media item, dispatched on its `media_type`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[allow(clippy::large_enum_variant)] // a decoded value, matched on everywhere
#[serde(tag = "media_type", rename_all = "snake_case")]
pub enum MediaItem {
    Artist(Artist),
    Album(Album),
    Track(Track),
    Playlist(Playlist),
    Radio(Radio),
    #[serde(other)]
    Other,
}

impl MediaItem {
    pub fn media_type(&self) -> MediaType {
        match self {
            MediaItem::Artist(_) => MediaType::Artist,
            MediaItem::Album(_) => MediaType::Album,
            MediaItem::Track(_) => MediaType::Track,
            MediaItem::Playlist(_) => MediaType::Playlist,
            MediaItem::Radio(_) => MediaType::Radio,
            MediaItem::Other => MediaType::Unknown,
        }
    }

    pub fn uri(&self) -> Option<&str> {
        Some(match self {
            MediaItem::Artist(x) => &x.uri,
            MediaItem::Album(x) => &x.uri,
            MediaItem::Track(x) => &x.uri,
            MediaItem::Playlist(x) => &x.uri,
            MediaItem::Radio(x) => &x.uri,
            MediaItem::Other => return None,
        })
    }

    pub fn name(&self) -> &str {
        match self {
            MediaItem::Artist(x) => &x.name,
            MediaItem::Album(x) => &x.name,
            MediaItem::Track(x) => &x.name,
            MediaItem::Playlist(x) => &x.name,
            MediaItem::Radio(x) => &x.name,
            MediaItem::Other => "",
        }
    }

    /// Cover image: the item's own, else (for tracks) its album's.
    pub fn cover(&self) -> Option<&MediaItemImage> {
        match self {
            MediaItem::Artist(x) => pick_cover(&x.metadata.images),
            MediaItem::Album(x) => pick_cover(&x.metadata.images),
            MediaItem::Track(x) => pick_cover(&x.metadata.images)
                .or_else(|| x.album.as_ref().and_then(|a| a.image.as_ref()).filter(|i| i.proxy_id.is_some())),
            MediaItem::Playlist(x) => pick_cover(&x.metadata.images),
            MediaItem::Radio(x) => pick_cover(&x.metadata.images),
            MediaItem::Other => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct Player {
    pub player_id: String,
    #[serde(default, deserialize_with = "nullable")]
    pub provider: String,
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
    #[serde(default, deserialize_with = "nullable")]
    pub display_name: String,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub powered: Option<bool>,
    /// idle, playing, paused
    #[serde(default, deserialize_with = "nullable")]
    pub playback_state: String,
    #[serde(default)]
    pub volume_level: Option<i32>,
    #[serde(default)]
    pub active_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct QueueItem {
    #[serde(default)]
    pub queue_id: String,
    pub queue_item_id: String,
    #[serde(default, deserialize_with = "nullable")]
    pub name: String,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub image: Option<MediaItemImage>,
    #[serde(default)]
    pub media_item: Option<MediaItem>,
    #[serde(default = "yes")]
    pub available: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, Default)]
pub struct PlayerQueue {
    pub queue_id: String,
    #[serde(default, deserialize_with = "nullable")]
    pub display_name: String,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub active: bool,
    /// idle, playing, paused
    #[serde(default, deserialize_with = "nullable")]
    pub state: String,
    /// Number of items in the queue.
    #[serde(default)]
    pub items: i64,
    #[serde(default)]
    pub current_index: Option<i64>,
    #[serde(default)]
    pub elapsed_time: Option<f64>,
    /// Unix time (seconds) when `elapsed_time` was sampled.
    #[serde(default)]
    pub elapsed_time_last_updated: Option<f64>,
    #[serde(default)]
    pub current_item: Option<QueueItem>,
    #[serde(default)]
    pub next_item: Option<QueueItem>,
    #[serde(default)]
    pub shuffle_enabled: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub repeat_mode: String,
}

/// What `player_queues/play_media` does with the existing queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueOption {
    /// Play now: insert at the current position and play, keeping the rest of the queue.
    Play,
    /// Replace the whole queue and play it.
    Replace,
    /// Insert after the current item.
    Next,
    /// Replace everything after the current item.
    ReplaceNext,
    /// Append to the end.
    Add,
}

/// Flattened view of what a queue is playing, for the now-playing screen.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NowPlaying {
    pub queue_id: String,
    pub state: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub uri: Option<String>,
    /// The track's album (`library://album/…` when it is in the library).
    pub album_uri: Option<String>,
    pub duration: Option<f64>,
    pub elapsed: Option<f64>,
    pub elapsed_sampled_at: Option<f64>,
    pub cover_proxy_id: Option<String>,
    pub queue_index: Option<i64>,
    pub queue_len: i64,
}

impl NowPlaying {
    pub fn from_queue(q: &PlayerQueue) -> Self {
        let mut np = NowPlaying {
            queue_id: q.queue_id.clone(),
            state: q.state.clone(),
            elapsed: q.elapsed_time,
            elapsed_sampled_at: q.elapsed_time_last_updated,
            queue_index: q.current_index,
            queue_len: q.items,
            ..Default::default()
        };
        if let Some(ci) = &q.current_item {
            np.title = ci.name.clone();
            np.duration = ci.duration;
            np.cover_proxy_id = ci.image.as_ref().and_then(|i| i.proxy_id.clone());
            if let Some(mi) = &ci.media_item {
                np.uri = mi.uri().map(str::to_owned);
                if let MediaItem::Track(t) = mi {
                    np.title = t.name.clone();
                    np.artist = t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
                    np.album = t.album.as_ref().map(|a| a.name.clone()).unwrap_or_default();
                    np.album_uri = t.album.as_ref().map(|a| a.uri.clone()).filter(|u| !u.is_empty());
                } else {
                    np.title = mi.name().to_owned();
                }
                if np.cover_proxy_id.is_none() {
                    np.cover_proxy_id = mi.cover().and_then(|i| i.proxy_id.clone());
                }
            }
        }
        np
    }

    /// Playback position now, extrapolated from the last sample while
    /// playing. `now` is Unix time in seconds.
    pub fn position_at(&self, now: f64) -> Option<f64> {
        let e = self.elapsed?;
        if self.state == "playing" {
            let t = self.elapsed_sampled_at.map(|s| (now - s).max(0.0)).unwrap_or(0.0);
            let p = e + t;
            Some(self.duration.map(|d| p.min(d)).unwrap_or(p))
        } else {
            Some(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_track_with_nulls() {
        let v = json!({
            "item_id": "1700", "provider": "library", "name": "1000 søte damer!",
            "version": null, "sort_name": "1000 sote damer!", "uri": "library://track/1700",
            "media_type": "track", "metadata": {"images": null}, "favorite": true,
            "duration": 176, "artists": [{"item_id": "160", "name": "Lillebjørn Nilsen", "uri": "library://artist/160"}],
            "album": {"item_id": "148", "name": "Postkort fra Lillebjørn", "uri": "library://album/148",
                      "image": {"type": "thumb", "path": "x", "provider": "spotify", "proxy_id": "abc"}},
            "disc_number": 1, "track_number": 3, "some_new_field": {"x": 1}
        });
        let item: MediaItem = serde_json::from_value(v).unwrap();
        assert_eq!(item.media_type(), MediaType::Track);
        assert_eq!(item.cover().unwrap().proxy_id.as_deref(), Some("abc"));
        let MediaItem::Track(t) = item else { panic!() };
        assert_eq!(t.duration, Some(176.0));
        assert_eq!(t.version, "");
    }

    #[test]
    fn unknown_media_type() {
        let item: MediaItem = serde_json::from_value(json!({"media_type": "sound_effect", "item_id": "1"})).unwrap();
        assert_eq!(item, MediaItem::Other);
    }

    #[test]
    fn now_playing_extrapolates() {
        let np = NowPlaying {
            state: "playing".into(),
            elapsed: Some(10.0),
            elapsed_sampled_at: Some(100.0),
            duration: Some(12.0),
            ..Default::default()
        };
        assert_eq!(np.position_at(101.5), Some(11.5));
        assert_eq!(np.position_at(200.0), Some(12.0));
    }
}
