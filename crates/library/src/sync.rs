//! Keeps the [`Store`] and the atlas in step with Music Assistant.
//!
//! * Full pull when first connected, after every reconnect (events were
//!   missed) and after MA's own `music_sync_completed`: every library
//!   item is upserted with a new generation number, then rows from older
//!   generations are dropped.
//! * Incremental updates from `media_item_added/updated/deleted` between.
//! * After either, new covers are fetched and the atlas is re-baked if the
//!   set or order of covers changed (debounced, in the background).
//! * Playlist contents are mirrored after the main pull (optional; this
//!   asks MA for each playlist's tracks and can take a while).
//!
//! The UI never waits on any of this; it reads the store through its own
//! [`Store`] connection and listens for [`Update`]s to know when to
//! re-query or reopen the atlas.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use log::{debug, info, warn};
use ma_client::{Client, Event, MediaItem};
use tokio::sync::{mpsc, Semaphore};
use tokio::time::Instant;

use crate::atlas::{self, AtlasConfig};
use crate::covers::{self, CoverCache, SOURCE_SIZE};
use crate::store::{Kind, Store};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// Holds `library.sqlite`, `covers/` (source JPEGs) and `atlas/`.
    pub data_dir: PathBuf,
    pub atlas: AtlasConfig,
    pub mirror_playlist_tracks: bool,
    /// Concurrent image proxy requests.
    pub cover_fetch_parallel: usize,
}

impl SyncConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        SyncConfig {
            atlas: AtlasConfig::new(data_dir.join("atlas")),
            data_dir,
            mirror_playlist_tracks: true,
            cover_fetch_parallel: 4,
        }
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("library.sqlite")
    }

    pub fn covers_dir(&self) -> PathBuf {
        self.data_dir.join("covers")
    }
}

/// What changed, for the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// Rows of these kinds may have changed; re-query visible lists.
    LibraryChanged(Vec<Kind>),
    /// A playlist's mirrored tracks changed.
    PlaylistTracksChanged(String),
    /// A new atlas generation is in place; reopen the atlas files.
    AtlasReady {
        generation: u64,
        slots: usize,
    },
    /// A full sync finished.
    FullSyncDone {
        items: usize,
        millis: u64,
    },
    /// A full cycle (sync, covers, atlas, playlists) has finished.
    Idle,
    SyncFailed(String),
}

/// Handle for feeding MA events to the sync task.
#[derive(Clone)]
pub struct SyncHandle {
    tx: mpsc::UnboundedSender<Cmd>,
}

#[allow(clippy::large_enum_variant)]
enum Cmd {
    Event(Event),
    FullSync,
}

impl SyncHandle {
    /// Pass every MA event here (the task ignores ones it does not need).
    pub fn event(&self, ev: &Event) {
        let relevant = matches!(
            ev,
            Event::Connected(_)
                | Event::MusicSyncCompleted
                | Event::MediaItemAdded { .. }
                | Event::MediaItemUpdated { .. }
                | Event::MediaItemDeleted { .. }
        );
        if relevant {
            let _ = self.tx.send(Cmd::Event(ev.clone()));
        }
    }

    /// Ask for a full resync including every playlist's contents (e.g. the
    /// nightly safety net).
    pub fn full_sync(&self) {
        let _ = self.tx.send(Cmd::FullSync);
    }
}

struct Inner {
    client: Client,
    cfg: SyncConfig,
    store: Arc<Mutex<Store>>,
    cache: Arc<CoverCache>,
    updates: mpsc::UnboundedSender<Update>,
    /// Serialises cover fetching and atlas baking.
    covers_lock: tokio::sync::Mutex<()>,
    /// Set while a playlist fill runs in the background.
    playlists_running: std::sync::atomic::AtomicBool,
}

/// Start the sync task. Feed it MA events through the returned handle
/// (or use [`forward_events`]); it starts with a full sync once the
/// client is connected.
pub fn spawn(client: Client, cfg: SyncConfig) -> Result<(SyncHandle, mpsc::UnboundedReceiver<Update>)> {
    std::fs::create_dir_all(&cfg.data_dir)?;
    let store = Store::open(cfg.db_path())?;
    let cache = CoverCache::new(cfg.covers_dir(), client.config().http_url());
    let (tx, rx) = mpsc::unbounded_channel();
    let (up_tx, up_rx) = mpsc::unbounded_channel();
    let inner = Arc::new(Inner {
        client,
        cfg,
        store: Arc::new(Mutex::new(store)),
        cache: Arc::new(cache),
        updates: up_tx,
        covers_lock: tokio::sync::Mutex::new(()),
        playlists_running: std::sync::atomic::AtomicBool::new(false),
    });
    tokio::spawn(run(inner, rx));
    Ok((SyncHandle { tx }, up_rx))
}

/// Convenience: forward every event from `events` to the sync task and
/// on to `out` (for consumers that also want player/queue events).
pub fn forward_events(
    handle: SyncHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
    out: Option<mpsc::UnboundedSender<Event>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(ev) = events.recv().await {
            handle.event(&ev);
            if let Some(o) = &out {
                let _ = o.send(ev);
            }
        }
    })
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl Inner {
    async fn db<T: Send + 'static>(&self, f: impl FnOnce(&mut Store) -> Result<T> + Send + 'static) -> Result<T> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let mut s = store.lock().unwrap_or_else(|p| p.into_inner());
            f(&mut s)
        })
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))?
    }

    fn notify(&self, u: Update) {
        let _ = self.updates.send(u);
    }

    async fn full_sync(&self) -> Result<usize> {
        let t0 = Instant::now();
        let gen: i64 = self.db(|s| Ok(s.meta("sync_gen")?.and_then(|v| v.parse().ok()).unwrap_or(0) + 1)).await?;
        let mut total = 0;
        let mut changed = Vec::new();
        for kind in Kind::ALL {
            let items: Vec<MediaItem> = self.client.library_all(kind.media_type()).await?;
            let n = items.len();
            total += n;
            let removed = self
                .db(move |s| {
                    s.transaction(|s| {
                        for it in &items {
                            s.upsert(it, gen)?;
                        }
                        let removed = s.delete_unseen(kind, gen)?;
                        s.renumber(kind)?;
                        Ok(removed)
                    })
                })
                .await?;
            debug!("synced {n} {kind:?} items, removed {removed}");
            changed.push(kind);
        }
        self.db(move |s| s.set_meta("sync_gen", &gen.to_string())).await?;
        let millis = t0.elapsed().as_millis() as u64;
        info!("full library sync: {total} items in {millis} ms");
        self.notify(Update::LibraryChanged(changed));
        self.notify(Update::FullSyncDone { items: total, millis });
        Ok(total)
    }

    async fn mirror_playlist(&self, playlist_id: String) -> Result<()> {
        let tracks = self.client.playlist_tracks(&playlist_id, "library").await?;
        let id = playlist_id.clone();
        self.db(move |s| s.transaction(|s| s.replace_playlist_tracks(&id, &tracks))).await?;
        self.notify(Update::PlaylistTracksChanged(playlist_id));
        Ok(())
    }

    async fn mirror_playlists(&self, only_missing: bool) -> Result<()> {
        let ids = self.db(|s| s.playlist_ids()).await?;
        let t0 = Instant::now();
        let mut n = 0;
        for id in ids {
            if only_missing {
                let i = id.clone();
                if self.db(move |s| s.has_playlist_tracks(&i)).await? {
                    continue;
                }
            }
            match self.mirror_playlist(id.clone()).await {
                Ok(()) => n += 1,
                Err(Error::Ma(ma_client::Error::Disconnected)) => return Err(Error::Ma(ma_client::Error::Disconnected)),
                Err(e) => {
                    warn!("playlist {id}: {e}");
                    let (i, ts) = (id.clone(), now());
                    self.db(move |s| s.mark_playlist_fetched(&i, ts)).await?;
                }
            }
        }
        info!("mirrored {n} playlists in {} ms", t0.elapsed().as_millis());
        Ok(())
    }

    /// Fetch wanted covers, then re-bake the atlas if its contents changed.
    async fn refresh_covers(&self, retry_failed: bool) -> Result<()> {
        let _guard = self.covers_lock.lock().await;
        let wanted = self.db(move |s| s.wanted_covers(retry_failed)).await?;
        if !wanted.is_empty() {
            let t0 = Instant::now();
            let sem = Arc::new(Semaphore::new(self.cfg.cover_fetch_parallel.max(1)));
            let mut tasks = Vec::new();
            for id in wanted {
                let permit = sem.clone().acquire_owned().await.expect("semaphore open");
                let cache = self.cache.clone();
                tasks.push(tokio::task::spawn_blocking(move || {
                    let _p = permit;
                    let r = cache.get(&id, SOURCE_SIZE).and_then(|j| covers::decode(&j)).map(|img| covers::average_rgb(&img));
                    (id, r)
                }));
            }
            let mut results = Vec::new();
            for t in tasks {
                if let Ok(r) = t.await {
                    results.push(r);
                }
            }
            let (ok, failed) = results.iter().fold((0, 0), |(o, f), (_, r)| if r.is_ok() { (o + 1, f) } else { (o, f + 1) });
            for (id, r) in results.iter().filter(|(_, r)| r.is_err()).take(3) {
                warn!("cover {id}: {}", r.as_ref().unwrap_err());
            }
            let ts = now();
            self.db(move |s| {
                s.transaction(|s| {
                    for (id, r) in &results {
                        match r {
                            Ok(rgb) => s.set_cover_fetched(id, *rgb, ts)?,
                            Err(_) => s.set_cover_failed(id, ts)?,
                        }
                    }
                    Ok(())
                })
            })
            .await?;
            info!("fetched {ok} covers ({failed} failed) in {} ms", t0.elapsed().as_millis());
        }
        self.rebake_if_changed().await
    }

    async fn rebake_if_changed(&self) -> Result<()> {
        let order = self.db(|s| s.covers_in_browse_order()).await?;
        let mut h = DefaultHasher::new();
        order.hash(&mut h);
        format!("{:?}{:?}", self.cfg.atlas.format, self.cfg.atlas.sizes).hash(&mut h);
        let hash = format!("{:016x}", h.finish());
        let (old_hash, old_gen) = self.db(|s| Ok((s.meta("atlas_hash")?, s.atlas_generation()?))).await?;
        let files_ok = self
            .cfg
            .atlas
            .sizes
            .iter()
            .all(|&sz| atlas::Atlas::open_for(&self.cfg.atlas, sz).map(|a| a.generation == old_gen).unwrap_or(false));
        if old_hash.as_deref() == Some(hash.as_str()) && files_ok {
            return Ok(());
        }
        let gen = old_gen + 1;
        let t0 = Instant::now();
        let (cfg, cache, ord) = (self.cfg.atlas.clone(), self.cache.clone(), order.clone());
        let report = tokio::task::spawn_blocking(move || atlas::bake(&cfg, &cache, &ord, gen))
            .await
            .map_err(|e| Error::Io(std::io::Error::other(e)))??;
        self.db(move |s| {
            s.set_cover_slots(&order, gen)?;
            s.set_meta("atlas_hash", &hash)
        })
        .await?;
        info!(
            "baked atlas generation {gen}: {} covers x {} sizes in {} ms ({} unreadable)",
            report.slots,
            self.cfg.atlas.sizes.len(),
            t0.elapsed().as_millis(),
            report.failed.len()
        );
        self.notify(Update::AtlasReady { generation: gen, slots: report.slots });
        Ok(())
    }

    /// Apply one incremental event. Returns whether covers may need work.
    async fn apply(&self, ev: Event) -> Result<bool> {
        match ev {
            Event::MediaItemAdded { uri, item, .. } | Event::MediaItemUpdated { uri, item, .. } => {
                let Some((kind, id)) = Kind::parse_uri(&uri) else {
                    return Ok(false);
                };
                let id = id.to_owned();
                let item = match item {
                    Some(i) if i.uri() == Some(uri.as_str()) => i,
                    _ => self.client.item_by_uri(&uri).await?,
                };
                let gen: i64 = self.db(|s| Ok(s.meta("sync_gen")?.and_then(|v| v.parse().ok()).unwrap_or(0))).await?;
                let stored = self
                    .db(move |s| {
                        s.transaction(|s| {
                            let stored = s.upsert(&item, gen)?;
                            s.renumber(kind)?;
                            Ok(stored)
                        })
                    })
                    .await?;
                if stored {
                    self.notify(Update::LibraryChanged(vec![kind]));
                    if kind == Kind::Playlist && self.cfg.mirror_playlist_tracks {
                        if let Err(e) = self.mirror_playlist(id).await {
                            warn!("playlist refresh: {e}");
                        }
                    }
                }
                Ok(stored)
            }
            Event::MediaItemDeleted { uri, .. } => {
                if let Some((kind, id)) = Kind::parse_uri(&uri) {
                    let id = id.to_owned();
                    self.db(move |s| {
                        s.transaction(|s| {
                            s.delete(kind, &id)?;
                            s.renumber(kind)
                        })
                    })
                    .await?;
                    self.notify(Update::LibraryChanged(vec![kind]));
                    return Ok(true);
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Full pull, covers and atlas; then playlist contents in a separate
    /// background task, so incremental events keep flowing meanwhile.
    /// Playlists that are already mirrored are only re-read on an explicit
    /// full sync (MA can take ~30 s per uncached Spotify playlist); in
    /// between they follow `media_item_updated`.
    async fn full_cycle(self: &Arc<Self>, refresh_all_playlists: bool) {
        let r: Result<()> = async {
            self.full_sync().await?;
            self.refresh_covers(true).await
        }
        .await;
        if let Err(e) = r {
            warn!("library sync failed: {e}");
            self.notify(Update::SyncFailed(e.to_string()));
            self.notify(Update::Idle);
            return;
        }
        use std::sync::atomic::Ordering;
        if !self.cfg.mirror_playlist_tracks || self.playlists_running.swap(true, Ordering::SeqCst) {
            self.notify(Update::Idle);
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let r = async {
                me.mirror_playlists(!refresh_all_playlists).await?;
                me.refresh_covers(false).await
            }
            .await;
            if let Err(e) = r {
                warn!("playlist mirror: {e}");
                me.notify(Update::SyncFailed(e.to_string()));
            }
            me.playlists_running.store(false, Ordering::SeqCst);
            me.notify(Update::Idle);
        });
    }
}

async fn run(inner: Arc<Inner>, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    inner.client.wait_connected().await;
    // Make whatever is already mirrored usable at once, then resync.
    if let Err(e) = inner.rebake_if_changed().await {
        warn!("atlas check: {e}");
    }
    let mut full_due: Option<Instant> = Some(Instant::now());
    let mut explicit = false;
    let mut covers_due: Option<Instant> = None;
    let mut seen_connect = false;
    loop {
        let next = [full_due, covers_due].into_iter().flatten().min();
        let cmd = match next {
            Some(t) => tokio::select! {
                c = rx.recv() => c,
                _ = tokio::time::sleep_until(t) => {
                    let now = Instant::now();
                    if full_due.is_some_and(|t| t <= now) {
                        full_due = None;
                        covers_due = None;
                        inner.full_cycle(std::mem::take(&mut explicit)).await;
                    } else if covers_due.is_some_and(|t| t <= now) {
                        covers_due = None;
                        if let Err(e) = inner.refresh_covers(false).await {
                            warn!("cover refresh: {e}");
                        }
                    }
                    continue;
                }
            },
            None => rx.recv().await,
        };
        let Some(cmd) = cmd else { return };
        match cmd {
            Cmd::FullSync => {
                full_due = Some(Instant::now());
                explicit = true;
            }
            Cmd::Event(Event::Connected(_)) => {
                // The first Connected may precede or follow our start; a
                // full sync is already scheduled for it.
                if seen_connect {
                    full_due = Some(Instant::now() + Duration::from_secs(2));
                }
                seen_connect = true;
            }
            Cmd::Event(Event::MusicSyncCompleted) => {
                full_due = Some(Instant::now() + Duration::from_secs(5));
            }
            Cmd::Event(ev) => {
                if full_due.is_some() {
                    continue; // the coming full sync covers it
                }
                match inner.apply(ev).await {
                    Ok(true) => {
                        covers_due.get_or_insert(Instant::now() + Duration::from_secs(5));
                    }
                    Ok(false) => {}
                    Err(e) => warn!("library event: {e}"),
                }
            }
        }
    }
}
