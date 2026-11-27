# ma-client

Music Assistant WebSocket client for neomaster. One socket, token auth,
`message_id` request matching, `partial` result reassembly, reconnect with
backoff, and a typed event stream.

```rust
let (client, mut events) = Client::connect(Config::new("http://192.168.2.238:8095", token));
client.wait_connected().await;
let albums: Vec<Album> = client.library_all(MediaType::Album).await?;
client.play_media("up001ff20ddb5e", &["library://album/131"], QueueOption::Play).await?;
let np = client.now_playing("up001ff20ddb5e").await?;
while let Some(ev) = events.recv().await { /* Event::QueueUpdated(..), Event::MediaItemUpdated{..} ... */ }
```

Commands fail immediately with `Error::Disconnected` while the socket is
down, so the UI never queues stale actions. MA rejects queue commands for a
player that is not available (`Server { code: 10, .. }`).

Checked against MA 2.10.13 (schema 65). The server documents every command
at `http://<ma>:8095/api-docs` (`/api-docs/commands.json`, `/api-docs/openapi.json`).

Examples (need `MA_TOKEN`, optional `MA_URL`):

- `cargo run --example ma_probe -- 30`: counts, players, queues, then tail events.
- `cargo run --example ma_play -- <queue_id> now|queue|play <uri>|add <uri>|next|...`
