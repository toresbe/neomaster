//! Server-pushed events. MA sends every event to every authenticated
//! client; there is no per-event subscription.

use serde_json::Value;

use crate::models::{MediaItem, Player, PlayerQueue, ServerInfo};

#[derive(Debug, Clone)]
pub enum Event {
    /// (Re)connected and authenticated. Anything mirrored should be
    /// re-validated, since events were missed while down.
    Connected(ServerInfo),
    /// The socket dropped; the client is reconnecting.
    Disconnected,
    PlayerAdded(Player),
    PlayerUpdated(Player),
    PlayerRemoved {
        player_id: String,
    },
    QueueAdded(PlayerQueue),
    QueueUpdated(PlayerQueue),
    /// The queue's contents changed; fetch them with `player_queues/items`.
    QueueItemsUpdated {
        queue_id: String,
    },
    /// Playback position, sent about once a second while playing.
    QueueTimeUpdated {
        queue_id: String,
        elapsed: f64,
    },
    MediaItemAdded {
        uri: String,
        item: Option<MediaItem>,
        raw: Value,
    },
    MediaItemUpdated {
        uri: String,
        item: Option<MediaItem>,
        raw: Value,
    },
    MediaItemDeleted {
        uri: String,
        item: Option<MediaItem>,
        raw: Value,
    },
    MediaItemPlayed {
        uri: String,
    },
    MusicSyncCompleted,
    ApplicationShutdown,
    /// Anything else, verbatim.
    Other {
        event: String,
        object_id: Option<String>,
        data: Value,
    },
}

impl Event {
    pub(crate) fn from_message(mut v: Value) -> Event {
        let event = v.get("event").and_then(Value::as_str).unwrap_or("").to_owned();
        let object_id = v.get("object_id").and_then(Value::as_str).map(str::to_owned);
        let data = v.get_mut("data").map(Value::take).unwrap_or(Value::Null);
        let oid = || object_id.clone().unwrap_or_default();
        let parse = |d: &Value| serde_json::from_value::<MediaItem>(d.clone()).ok();
        let ev = match event.as_str() {
            "player_added" => serde_json::from_value(data.clone()).ok().map(Event::PlayerAdded),
            "player_updated" => serde_json::from_value(data.clone()).ok().map(Event::PlayerUpdated),
            "player_removed" => Some(Event::PlayerRemoved { player_id: oid() }),
            "queue_added" => serde_json::from_value(data.clone()).ok().map(Event::QueueAdded),
            "queue_updated" => serde_json::from_value(data.clone()).ok().map(Event::QueueUpdated),
            "queue_items_updated" => Some(Event::QueueItemsUpdated { queue_id: oid() }),
            "queue_time_updated" => data.as_f64().map(|elapsed| Event::QueueTimeUpdated { queue_id: oid(), elapsed }),
            "media_item_added" => Some(Event::MediaItemAdded { uri: oid(), item: parse(&data), raw: data.clone() }),
            "media_item_updated" => Some(Event::MediaItemUpdated { uri: oid(), item: parse(&data), raw: data.clone() }),
            "media_item_deleted" => Some(Event::MediaItemDeleted { uri: oid(), item: parse(&data), raw: data.clone() }),
            "media_item_played" => Some(Event::MediaItemPlayed { uri: oid() }),
            "music_sync_completed" => Some(Event::MusicSyncCompleted),
            "application_shutdown" => Some(Event::ApplicationShutdown),
            _ => None,
        };
        ev.unwrap_or(Event::Other { event, object_id, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn time_update() {
        let e = Event::from_message(json!({"event": "queue_time_updated", "object_id": "q1", "data": 12.5}));
        assert!(matches!(e, Event::QueueTimeUpdated { ref queue_id, elapsed } if queue_id == "q1" && elapsed == 12.5));
    }

    #[test]
    fn unknown_passes_through() {
        let e = Event::from_message(json!({"event": "tasks_updated", "data": [1]}));
        assert!(matches!(e, Event::Other { ref event, .. } if event == "tasks_updated"));
    }
}
