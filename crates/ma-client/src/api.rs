//! Typed helpers over [`Client::command`] for the commands neomaster uses.
//! Names and arguments follow MA's `/api-docs/commands.json` (2.10.13).

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;

use crate::client::{Client, Result};
use crate::models::*;

/// Page size for library pulls. MA streams long results in `partial`
/// chunks anyway; this bounds a single reply's memory.
pub const PAGE: usize = 500;

impl Client {
    // ---- library -------------------------------------------------------

    /// One page of `music/<collection>/library_items`, ordered by sort name.
    pub async fn library_items<T: DeserializeOwned>(&self, media_type: MediaType, offset: usize, limit: usize) -> Result<Vec<T>> {
        let cmd = format!("music/{}/library_items", media_type.collection());
        self.command(&cmd, json!({"offset": offset, "limit": limit, "order_by": "sort_name"})).await
    }

    /// Every library item of one type, paged.
    pub async fn library_all<T: DeserializeOwned>(&self, media_type: MediaType) -> Result<Vec<T>> {
        let mut out = Vec::new();
        loop {
            let page: Vec<T> = self.library_items(media_type, out.len(), PAGE).await?;
            let n = page.len();
            out.extend(page);
            if n < PAGE {
                return Ok(out);
            }
        }
    }

    pub async fn library_count(&self, media_type: MediaType) -> Result<u64> {
        self.command(&format!("music/{}/count", media_type.collection()), json!({})).await
    }

    /// Tracks of a playlist, in playlist order.
    pub async fn playlist_tracks(&self, item_id: &str, provider: &str) -> Result<Vec<Track>> {
        let v = self
            .command_raw_timeout(
                "music/playlists/playlist_tracks",
                json!({"item_id": item_id, "provider_instance_id_or_domain": provider}),
                Duration::from_secs(120),
            )
            .await?;
        // Non-track entries (radio, episodes) in a playlist are skipped.
        Ok(match v {
            Value::Array(a) => a.into_iter().filter_map(|t| serde_json::from_value(t).ok()).collect(),
            _ => Vec::new(),
        })
    }

    pub async fn album_tracks(&self, item_id: &str, provider: &str) -> Result<Vec<Track>> {
        self.command("music/albums/album_tracks", json!({"item_id": item_id, "provider_instance_id_or_domain": provider})).await
    }

    pub async fn item_by_uri(&self, uri: &str) -> Result<MediaItem> {
        self.command("music/item_by_uri", json!({"uri": uri})).await
    }

    // ---- players and queues --------------------------------------------

    pub async fn players(&self) -> Result<Vec<Player>> {
        self.command("players/all", json!({})).await
    }

    pub async fn player(&self, player_id: &str) -> Result<Player> {
        self.command("players/get", json!({"player_id": player_id})).await
    }

    /// The player MA shows for an output protocol client (a Sendspin
    /// client id, say). MA wraps such clients in a "universal" player,
    /// whose id is the queue id for `play_media`. `None` until MA has
    /// created it, a few seconds after the client first connects.
    pub async fn player_for_output(&self, output_protocol_id: &str) -> Result<Option<String>> {
        let v = self.command_raw("players/all", json!({"return_protocol_players": true})).await?;
        Ok(v.as_array().into_iter().flatten().find_map(|p| {
            let wraps = p["output_protocols"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|o| o["output_protocol_id"].as_str() == Some(output_protocol_id));
            (wraps && p["type"] == "player").then(|| p["player_id"].as_str().unwrap_or_default().to_owned())
        }))
    }

    pub async fn queue(&self, queue_id: &str) -> Result<PlayerQueue> {
        self.command("player_queues/get", json!({"queue_id": queue_id})).await
    }

    /// The queue a player is currently following (its own, or a group's).
    pub async fn active_queue(&self, player_id: &str) -> Result<PlayerQueue> {
        self.command("player_queues/get_active_queue", json!({"player_id": player_id})).await
    }

    pub async fn queue_items(&self, queue_id: &str, offset: usize, limit: usize) -> Result<Vec<QueueItem>> {
        self.command("player_queues/items", json!({"queue_id": queue_id, "offset": offset, "limit": limit})).await
    }

    pub async fn now_playing(&self, queue_id: &str) -> Result<NowPlaying> {
        Ok(NowPlaying::from_queue(&self.queue(queue_id).await?))
    }

    // ---- playback ------------------------------------------------------

    /// Play or enqueue one or more items (MA URIs such as
    /// `library://album/131`) on a queue. A player's own queue id is its
    /// player id.
    pub async fn play_media(&self, queue_id: &str, uris: &[&str], option: QueueOption) -> Result<()> {
        self.command_raw("player_queues/play_media", json!({"queue_id": queue_id, "media": uris, "option": option}))
            .await
            .map(drop)
    }

    /// Replace the queue with a container (album, playlist) and play it from one of its tracks.
    pub async fn play_media_from(&self, queue_id: &str, container_uri: &str, start_item_uri: &str) -> Result<()> {
        self.command_raw(
            "player_queues/play_media",
            json!({"queue_id": queue_id, "media": [container_uri], "option": QueueOption::Replace, "start_item": start_item_uri}),
        )
        .await
        .map(drop)
    }

    async fn queue_cmd(&self, cmd: &str, queue_id: &str, extra: Value) -> Result<()> {
        let mut args = json!({"queue_id": queue_id});
        if let (Value::Object(a), Value::Object(e)) = (&mut args, extra) {
            a.extend(e);
        }
        self.command_raw(&format!("player_queues/{cmd}"), args).await.map(drop)
    }

    pub async fn play(&self, queue_id: &str) -> Result<()> {
        self.queue_cmd("play", queue_id, json!({})).await
    }
    pub async fn pause(&self, queue_id: &str) -> Result<()> {
        self.queue_cmd("pause", queue_id, json!({})).await
    }
    pub async fn play_pause(&self, queue_id: &str) -> Result<()> {
        self.queue_cmd("play_pause", queue_id, json!({})).await
    }
    pub async fn stop(&self, queue_id: &str) -> Result<()> {
        self.queue_cmd("stop", queue_id, json!({})).await
    }
    pub async fn next(&self, queue_id: &str) -> Result<()> {
        self.queue_cmd("next", queue_id, json!({})).await
    }
    pub async fn previous(&self, queue_id: &str) -> Result<()> {
        self.queue_cmd("previous", queue_id, json!({})).await
    }
    /// Jump to an index (or `queue_item_id`) in the queue.
    pub async fn play_index(&self, queue_id: &str, index: i64) -> Result<()> {
        self.queue_cmd("play_index", queue_id, json!({"index": index})).await
    }
    pub async fn seek(&self, queue_id: &str, position_secs: i64) -> Result<()> {
        self.queue_cmd("seek", queue_id, json!({"position": position_secs})).await
    }
    pub async fn shuffle(&self, queue_id: &str, enabled: bool) -> Result<()> {
        self.queue_cmd("shuffle", queue_id, json!({"shuffle_enabled": enabled})).await
    }
    pub async fn clear_queue(&self, queue_id: &str) -> Result<()> {
        self.queue_cmd("clear", queue_id, json!({})).await
    }
    pub async fn delete_queue_item(&self, queue_id: &str, queue_item_id: &str) -> Result<()> {
        self.queue_cmd("delete_item", queue_id, json!({"item_id_or_index": queue_item_id})).await
    }

    pub async fn set_favorite(&self, uri: &str, favorite: bool) -> Result<()> {
        let cmd = if favorite { "music/favorites/add_item" } else { "music/favorites/remove_item" };
        let args = if favorite {
            json!({"item": uri})
        } else {
            json!({"media_type": uri_media_type(uri), "library_item_id": uri_item_id(uri)})
        };
        self.command_raw(cmd, args).await.map(drop)
    }
}

fn uri_media_type(uri: &str) -> &str {
    uri.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("")
}

fn uri_item_id(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or("")
}
