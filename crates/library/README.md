# library

Local mirror of the Music Assistant library and the pre-baked cover atlas
(architecture sketch: "Music Assistant: player, mirror and covers").

```text
<data_dir>/library.sqlite          items, artist links, playlist tracks, covers (WAL)
<data_dir>/covers/jpeg160/<id>.jpg source covers from /imageproxy/<id>?size=160&fmt=jpeg
<data_dir>/atlas/covers-<N>.rgb565 one mmap file per size step, slots in browse order
```

```rust
let (client, events) = ma_client::Client::connect(cfg);
let (sync, mut updates) = library::sync::spawn(client.clone(), SyncConfig::new("/var/lib/neomaster"))?;
library::sync::forward_events(sync, events, Some(ui_events_tx));

// UI thread: its own connection, never blocks on the network.
let store = Store::open("/var/lib/neomaster/library.sqlite")?;
let rows = store.list(Kind::Album, cursor - 6, 13)?;      // index range, ~0.05 ms
let letters = store.letter_index(Kind::Artist)?;         // [("#",0),("A",3),...]
let atlas = Atlas::open_for(&atlas_cfg, 80)?;            // reopen on Update::AtlasReady
let pixels = atlas.slot(rows[6].cover_slot.unwrap());    // 80*80 RGB565
```

Sync policy:

- Full pull at start, after every reconnect and after MA's
  `music_sync_completed`; then rows not seen are deleted.
- `media_item_added/updated/deleted` applied as they arrive.
- New covers fetched (4 at a time) and the atlas re-baked only when the set
  or order of covers changed. Atlas files are replaced by rename and carry a
  generation number matching `Store::atlas_generation()`.
- Playlist contents: fetched once per playlist, then refreshed on
  `media_item_updated` or an explicit `SyncHandle::full_sync()` (nightly).
  MA takes up to ~30 s for an uncached Spotify playlist, so the first fill
  runs in the background.

Atlas format: see the doc comment at the top of `src/atlas.rs`. Default
pixel format RGB565, sizes 48 (lists) and 64/72/80/88/96 (cover arc), all
covers in all sizes, centre-cropped to square.

`cargo run --release --example mirror -- <data_dir> [follow_secs] [sheet.ppm]`
mirrors the live library, prints counts, letter index, query timings and atlas
sizes, and can write a contact sheet of the first 64 album covers.
