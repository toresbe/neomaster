//! The N.MUSIC actor: Music Assistant for the panel.
//!
//! It owns the connection to Music Assistant (MA) and everything that waits on the network,
//! so the core stays sans-IO:
//!
//! * the local library mirror and cover atlas (`library` crate), rebuilt into a
//!   [`ui::Library`] whenever the mirror changes;
//! * the BM5's own MA player: the Sendspin client (`player` crate) built in, playing through
//!   ALSA. MA wraps it in a "universal" player whose id is the queue the panel plays on;
//! * the queue: play, enqueue and transport commands from the core ([`MusicCmd`]), and what
//!   plays now and what is queued back to it ([`MusicEvent`]).
//!
//! Configuration: the settings' `[music_assistant]` `url`, the token (`MA_TOKEN` or
//! `token_file`) and `player_id` (play on that MA player instead of the built-in one's; the
//! environment's `MA_URL` and `NEOMASTER_MA_PLAYER` override them). Without a URL and a token
//! the panel browses a made-up library. The built-in player has no settings yet, only
//! environment variables:
//!
//! | Variable | Meaning |
//! |---|---|
//! | `NEOMASTER_PLAYER` | `sendspin` (default): the built-in player; `none`: an external one (Squeezelite), named by `NEOMASTER_MA_PLAYER` |
//! | `NEOMASTER_SENDSPIN_URL` | MA's Sendspin endpoint (default `ws://<MA host>:8927/sendspin`) |
//! | `NEOMASTER_PLAYER_ID`, `NEOMASTER_PLAYER_NAME` | the built-in player's id and name in MA |
//! | `NEOMASTER_PLAYER_DEVICE` | its ALSA device (`--sim`: `null`, silent, unless set) |
//! | `NEOMASTER_DATA_DIR` | where the library mirror, covers and atlas live |

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use library::{Atlas, Kind, PixelFormat, Store};
use log::{debug, info, warn};
use ma_client::{Client, QueueOption};
use tokio::sync::{mpsc, watch};
use ui::{Cover, CoverSource, NowPlaying, PlayRequest, QueueEntry};

use crate::core::Event;

/// What the core asks of Music Assistant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MusicCmd {
    /// Replace the queue and play.
    Play(PlayRequest),
    /// Append to the queue.
    Enqueue(Vec<String>),
    /// Jump to a queue entry.
    PlayIndex(usize),
    /// Play the queue where it stands (N.MUSIC selected, or PLAY).
    Resume,
    /// STOP, or N.MUSIC released: pause, so it resumes where it was.
    Pause,
    Next,
    Previous,
}

/// What the actor tells the core.
pub enum MusicEvent {
    /// The mirror changed: browse this library now.
    Library(Box<ui::Library>),
    /// What plays on the BM5's queue (`None`: nothing queued).
    NowPlaying(Option<NowPlaying>),
    /// The large cover of the item with this URI.
    Art {
        uri: Option<String>,
        image: Arc<render::Image>,
    },
    Queue(Vec<QueueEntry>),
    /// The built-in player started (true) or stopped (false) a stream.
    Stream(bool),
}

impl std::fmt::Debug for MusicEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MusicEvent::Library(l) => {
                write!(f, "Library({} artists, {} albums, {} tracks)", l.artists.len(), l.albums.len(), l.tracks.len())
            }
            MusicEvent::NowPlaying(n) => write!(f, "NowPlaying({:?})", n.as_ref().map(|n| (&n.title, n.playing))),
            MusicEvent::Art { uri, image } => write!(f, "Art({uri:?}, {}x{})", image.width, image.height),
            MusicEvent::Queue(q) => write!(f, "Queue({} entries)", q.len()),
            MusicEvent::Stream(s) => write!(f, "Stream({s})"),
        }
    }
}

/// Where the core sends [`MusicCmd`]s.
pub trait MusicOut: Send {
    fn send(&mut self, cmd: MusicCmd);
}

impl MusicOut for mpsc::UnboundedSender<MusicCmd> {
    fn send(&mut self, cmd: MusicCmd) {
        let _ = mpsc::UnboundedSender::send(self, cmd);
    }
}

/// No Music Assistant (tests, or a run without one): commands are dropped.
pub struct NoMusic;
impl MusicOut for NoMusic {
    fn send(&mut self, _: MusicCmd) {}
}

/// The built-in Sendspin player's settings.
#[derive(Clone, Debug)]
pub struct SendspinConfig {
    pub url: String,
    pub client_id: String,
    pub name: String,
    pub device: Option<String>,
}

#[derive(Clone)]
pub struct MusicConfig {
    pub url: String,
    pub token: String,
    /// A fixed player / queue id (`NEOMASTER_MA_PLAYER`).
    pub queue: Option<String>,
    /// The built-in player, unless an external one plays for us.
    pub sendspin: Option<SendspinConfig>,
    pub data_dir: PathBuf,
}

impl std::fmt::Debug for MusicConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MusicConfig")
            .field("url", &self.url)
            .field("queue", &self.queue)
            .field("sendspin", &self.sendspin)
            .field("data_dir", &self.data_dir)
            .finish()
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).ok().filter(|h| !h.is_empty()).unwrap_or("bm5".into())
}

/// The host part of an MA URL (`http://192.168.2.238:8095` -> `192.168.2.238`).
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let hostport = rest.split('/').next().unwrap_or(rest);
    match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h,
        _ => hostport,
    }
}

impl MusicConfig {
    /// From the settings and the environment (see the module docs); `Err` says why there is
    /// no MA.
    pub fn from_settings(ma: &settings::MusicAssistant, sim: bool) -> Result<MusicConfig, String> {
        Self::from_parts(ma.url.clone(), ma.token(), ma.player_id.clone(), sim, |k| {
            std::env::var(k).ok().filter(|v| !v.trim().is_empty())
        })
    }

    pub fn from_parts(
        url: String,
        token: Option<String>,
        player_id: String,
        sim: bool,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<MusicConfig, String> {
        let mut missing = Vec::new();
        if url.trim().is_empty() {
            missing.push("music_assistant.url (MA_URL)");
        }
        if token.is_none() {
            missing.push("MA_TOKEN or music_assistant.token_file");
        }
        if !missing.is_empty() {
            return Err(format!("{} not set", missing.join(", ")));
        }
        let queue = Some(player_id).filter(|p| !p.trim().is_empty());
        let sendspin = match get("NEOMASTER_PLAYER").as_deref() {
            None | Some("sendspin") => {
                let host = hostname();
                // --sim gets an id and name of its own, so it never takes over the BM5's player
                let (id, name) = if sim {
                    (format!("neomaster-sim-{host}"), format!("neomaster --sim ({host})"))
                } else {
                    (format!("neomaster-{host}"), "BeoMaster 5".to_string())
                };
                Some(SendspinConfig {
                    url: get("NEOMASTER_SENDSPIN_URL").unwrap_or_else(|| format!("ws://{}:8927/sendspin", url_host(&url))),
                    client_id: get("NEOMASTER_PLAYER_ID").unwrap_or(id),
                    name: get("NEOMASTER_PLAYER_NAME").unwrap_or(name),
                    device: get("NEOMASTER_PLAYER_DEVICE").or(sim.then(|| "null".to_string())),
                })
            }
            Some("none") | Some("external") | Some("squeezelite") => {
                if queue.is_none() {
                    return Err(
                        "NEOMASTER_PLAYER=none needs music_assistant.player_id (NEOMASTER_MA_PLAYER), the external player's id"
                            .into(),
                    );
                }
                None
            }
            Some(other) => return Err(format!("NEOMASTER_PLAYER={other:?}: expected sendspin or none")),
        };
        let data_dir = get("NEOMASTER_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| {
            if unsafe { libc::geteuid() } == 0 {
                PathBuf::from("/var/lib/neomaster")
            } else {
                let base = get("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(get("HOME").unwrap_or("/tmp".into())).join(".local/share"));
                base.join("neomaster")
            }
        });
        Ok(MusicConfig { url, token: token.unwrap_or_default(), queue, sendspin, data_dir })
    }

    fn sync_config(&self) -> library::sync::SyncConfig {
        let mut c = library::sync::SyncConfig::new(&self.data_dir);
        // The largest cover on the panel (the centre of the cover arc and the selection,
        // 108 px); the renderer scales it for the smaller places.
        c.atlas.sizes = vec![ATLAS_SIZE];
        c
    }
}

const ATLAS_SIZE: u32 = 108;
/// The now-playing cover (512 px on screen; the image proxy serves this size).
const NOW_PLAYING_SIZE: u32 = 512;
/// How long a command waits for MA and the BM5's queue (a link room can select N.MUSIC right
/// after boot).
const READY_WAIT: Duration = Duration::from_secs(20);
/// The queue entries mirrored for the QUEUE view.
const QUEUE_LIMIT: usize = 500;

/// Cover images from the atlas file of the current generation.
struct AtlasCovers(Atlas);

impl CoverSource for AtlasCovers {
    fn cover(&self, slot: u32) -> Option<Arc<render::Image>> {
        let px = self.0.slot(slot)?;
        let mut rgba = Vec::with_capacity((self.0.size * self.0.size * 4) as usize);
        match self.0.format {
            PixelFormat::Rgb565 => {
                for p in px.chunks_exact(2) {
                    let v = u16::from_le_bytes([p[0], p[1]]);
                    let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
                    rgba.extend_from_slice(&[(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8, 255]);
                }
            }
            PixelFormat::Xrgb8888 => {
                for p in px.chunks_exact(4) {
                    rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
        }
        Some(render::Image::new(self.0.size, self.0.size, rgba))
    }
}

fn cover_of(row: &library::Row) -> Cover {
    Cover { slot: row.cover_slot, rgb: row.cover_rgb }
}

/// Build the panel's library from the mirror. Blocking (SQLite and the atlas file).
pub fn load_library(cfg: &library::sync::SyncConfig) -> library::Result<ui::Library> {
    let store = Store::open(cfg.db_path())?;
    let all = |k: Kind| -> library::Result<Vec<library::Row>> { store.list(k, 0, store.count(k)?) };
    let mut lib = ui::Library::empty();
    // The atlas is only good for the generation the store's slots belong to.
    let generation = store.atlas_generation()?;
    let atlas = Atlas::open_for(&cfg.atlas, ATLAS_SIZE).ok().filter(|a| a.generation == generation);
    let have_atlas = atlas.is_some();
    lib.covers = atlas.map(|a| Arc::new(AtlasCovers(a)) as Arc<dyn CoverSource>);
    let cover = |r: &library::Row| {
        let mut c = cover_of(r);
        if !have_atlas {
            c.slot = None;
        }
        c
    };
    let mut track_index = std::collections::HashMap::new();
    for (pos, r) in all(Kind::Track)?.into_iter().enumerate() {
        track_index.insert(r.item_id.clone(), lib.tracks.len());
        lib.tracks.push(ui::library::Track {
            title: r.name.clone(),
            artist: r.subtitle.clone(),
            album: None,
            uri: r.uri.clone(),
            // the store's browse order (sort names, articles dropped)
            sort: format!("{pos:08}"),
            cover: cover(&r),
        });
    }
    let mut album_index = std::collections::HashMap::new();
    for (pos, r) in all(Kind::Album)?.into_iter().enumerate() {
        let a = lib.albums.len();
        album_index.insert(r.item_id.clone(), a);
        let tracks: Vec<usize> =
            store.album_tracks(&r.item_id)?.iter().filter_map(|t| track_index.get(&t.item_id).copied()).collect();
        for t in &tracks {
            lib.tracks[*t].album = Some(a);
        }
        lib.albums.push(ui::library::Album {
            title: r.name.clone(),
            artist: r.subtitle.clone(),
            tracks,
            uri: r.uri.clone(),
            // the store's browse order (sort names, articles dropped)
            sort: format!("{pos:08}"),
            cover: cover(&r),
        });
    }
    for (pos, r) in all(Kind::Artist)?.into_iter().enumerate() {
        let albums = store.artist_albums(&r.item_id)?.iter().filter_map(|a| album_index.get(&a.item_id).copied()).collect();
        lib.artists.push(ui::library::Artist { name: r.name.clone(), albums, uri: r.uri.clone(), sort: format!("{pos:08}") });
    }
    Ok(lib)
}

/// A now-playing cover: its proxy id and the image.
type CachedArt = (String, Arc<render::Image>);

/// What the actor needs while it runs.
struct Actor {
    client: Client,
    cfg: MusicConfig,
    sync_cfg: library::sync::SyncConfig,
    events: mpsc::UnboundedSender<Event>,
    /// The BM5's queue in MA, once known.
    queue: Option<String>,
    /// ... published for N.RADIO, which plays on the same queue.
    queue_watch: watch::Sender<Option<String>>,
    /// Where the current item is in MA's queue. MA keeps what has played before it; the QUEUE
    /// view, as the stock one, lists from the current item on.
    queue_offset: usize,
    /// The cover fetched for now-playing (proxy id), so it is fetched once.
    art_for: Option<String>,
    /// ... and the image, once there: the next track of an album has the same cover.
    art: Arc<std::sync::Mutex<Option<CachedArt>>>,
    covers: Arc<library::covers::CoverCache>,
}

impl Actor {
    fn send(&self, ev: MusicEvent) {
        let _ = self.events.send(Event::Music(ev));
    }

    async fn reload_library(&self) {
        let cfg = self.sync_cfg.clone();
        if !cfg.db_path().exists() {
            debug!("N.MUSIC: no library mirror yet in {}", cfg.data_dir.display());
            return;
        }
        match tokio::task::spawn_blocking(move || load_library(&cfg)).await {
            Ok(Ok(lib)) => {
                info!(
                    "N.MUSIC: library {} artists, {} albums, {} tracks{}",
                    lib.artists.len(),
                    lib.albums.len(),
                    lib.tracks.len(),
                    if lib.covers.is_some() { "" } else { " (no cover atlas yet)" }
                );
                self.send(MusicEvent::Library(Box::new(lib)));
            }
            Ok(Err(e)) => warn!("N.MUSIC: reading the library mirror: {e}"),
            Err(e) => warn!("N.MUSIC: library task: {e}"),
        }
    }

    /// Our queue changed state: tell the core what plays, fetch its large cover.
    /// Returns whether the current item moved in the queue (the QUEUE view starts there).
    fn queue_updated(&mut self, q: &ma_client::PlayerQueue) -> bool {
        let offset = q.current_index.unwrap_or(0).max(0) as usize;
        let moved = std::mem::replace(&mut self.queue_offset, offset) != offset;
        self.now_playing(q);
        moved
    }

    fn now_playing(&mut self, q: &ma_client::PlayerQueue) {
        let np = ma_client::NowPlaying::from_queue(q);
        let has_item = q.current_item.is_some();
        let ui_np = has_item.then(|| NowPlaying {
            title: np.title.clone(),
            artist: np.artist.clone(),
            album: np.album.clone(),
            uri: np.uri.clone(),
            album_uri: np.album_uri.clone(),
            cover: Cover::default(),
            art: None,
            playing: np.state == "playing",
        });
        self.send(MusicEvent::NowPlaying(ui_np));
        let Some(proxy) = np.cover_proxy_id.clone() else { return };
        if self.art_for.as_deref() == Some(proxy.as_str()) {
            let cached = self.art.lock().unwrap().clone();
            if let Some((p, image)) = cached.filter(|(p, _)| *p == proxy) {
                debug!("N.MUSIC: now-playing cover {p} again");
                self.send(MusicEvent::Art { uri: np.uri.clone(), image });
            }
            return;
        }
        self.art_for = Some(proxy.clone());
        let cache = self.art.clone();
        let covers = self.covers.clone();
        let events = self.events.clone();
        let uri = np.uri.clone();
        tokio::task::spawn_blocking(move || {
            let img = covers.get(&proxy, NOW_PLAYING_SIZE).and_then(|j| library::covers::decode(&j));
            match img {
                Ok(rgb) => {
                    let rgba = rgb.data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
                    let image = render::Image::new(rgb.width, rgb.height, rgba);
                    *cache.lock().unwrap() = Some((proxy, image.clone()));
                    let _ = events.send(Event::Music(MusicEvent::Art { uri, image }));
                }
                Err(e) => debug!("N.MUSIC: now-playing cover {proxy}: {e}"),
            }
        });
    }

    async fn refresh_queue_items(&self) {
        let Some(queue) = &self.queue else { return };
        let items = match self.client.queue_items(queue, self.queue_offset, QUEUE_LIMIT).await {
            Ok(i) => i,
            Err(e) => {
                warn!("N.MUSIC: reading the queue: {e}");
                return;
            }
        };
        let cfg = self.sync_cfg.clone();
        let entries = tokio::task::spawn_blocking(move || {
            let store = Store::open(cfg.db_path()).ok();
            items
                .into_iter()
                .map(|it| {
                    // MA names queue items "artist - title"; the track's own name reads as the
                    // stock queue does
                    let (title, artist, uri) = match &it.media_item {
                        Some(ma_client::MediaItem::Track(t)) => (
                            t.name.clone(),
                            t.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", "),
                            Some(t.uri.clone()),
                        ),
                        Some(mi) => (it.name.clone(), String::new(), mi.uri().map(str::to_owned)),
                        None => (it.name.clone(), String::new(), None),
                    };
                    let proxy = it.image.as_ref().and_then(|i| i.proxy_id.clone());
                    let slot = match (&store, &proxy) {
                        (Some(s), Some(p)) => s.cover_slot(p).ok().flatten(),
                        _ => None,
                    };
                    QueueEntry { title, artist, uri, cover: Cover { slot, rgb: None } }
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        self.send(MusicEvent::Queue(entries));
    }

    /// The queue to play on, waiting a while for MA and the player to be there.
    async fn ready_queue(&mut self, wait_queue: &mut mpsc::UnboundedReceiver<String>) -> Option<String> {
        if self.queue.is_none() {
            if let Ok(Some(q)) = tokio::time::timeout(READY_WAIT, wait_queue.recv()).await {
                self.set_queue(q).await;
            }
        }
        let q = self.queue.clone()?;
        tokio::time::timeout(READY_WAIT, self.client.wait_connected()).await.ok()?;
        Some(q)
    }

    async fn set_queue(&mut self, q: String) {
        if self.queue.as_deref() == Some(q.as_str()) {
            return;
        }
        info!("N.MUSIC: playing on Music Assistant queue {q}");
        self.queue = Some(q.clone());
        let _ = self.queue_watch.send(Some(q.clone()));
        match self.client.queue(&q).await {
            Ok(state) => {
                self.queue_updated(&state);
            }
            Err(e) => debug!("N.MUSIC: queue {q}: {e}"),
        }
        self.refresh_queue_items().await;
    }

    async fn command(&mut self, cmd: MusicCmd, wait_queue: &mut mpsc::UnboundedReceiver<String>) {
        let Some(q) = self.ready_queue(wait_queue).await else {
            warn!("N.MUSIC: Music Assistant or the BM5's player is not there; {cmd:?} dropped");
            return;
        };
        let c = &self.client;
        let r = match &cmd {
            MusicCmd::Play(PlayRequest { uri, start: Some(start) }) => c.play_media_from(&q, uri, start).await,
            // GO replaces the queue (ADD TO QUEUE is the long-GO alternative)
            MusicCmd::Play(PlayRequest { uri, start: None }) => c.play_media(&q, &[uri.as_str()], QueueOption::Replace).await,
            MusicCmd::Enqueue(uris) => {
                let uris: Vec<&str> = uris.iter().map(String::as_str).collect();
                c.play_media(&q, &uris, QueueOption::Add).await
            }
            MusicCmd::PlayIndex(i) => c.play_index(&q, (self.queue_offset + *i) as i64).await,
            MusicCmd::Resume => c.play(&q).await,
            MusicCmd::Pause => c.pause(&q).await,
            MusicCmd::Next => c.next(&q).await,
            MusicCmd::Previous => c.previous(&q).await,
        };
        match r {
            Ok(()) => debug!("N.MUSIC: {cmd:?} on {q}"),
            Err(e) => warn!("N.MUSIC: {cmd:?} on {q}: {e}"),
        }
    }
}

/// Find the universal player MA made for our Sendspin client; send its id once known.
async fn find_queue(client: Client, client_id: String, found: mpsc::UnboundedSender<String>) {
    let mut logged = false;
    loop {
        if tokio::time::timeout(Duration::from_secs(30), client.wait_connected()).await.is_ok() {
            match client.player_for_output(&client_id).await {
                Ok(Some(id)) => {
                    let _ = found.send(id);
                    return;
                }
                Ok(None) if !logged => {
                    info!("N.MUSIC: waiting for Music Assistant to list the player {client_id:?}");
                    logged = true;
                }
                Ok(None) => {}
                Err(e) => debug!("N.MUSIC: players: {e}"),
            }
        }
        if found.is_closed() {
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Run the actor until the core drops its command sender.
pub async fn run(
    cfg: MusicConfig,
    mut cmds: mpsc::UnboundedReceiver<MusicCmd>,
    events: mpsc::UnboundedSender<Event>,
    queue_watch: watch::Sender<Option<String>>,
) {
    let (client, mut ma_events) = Client::connect(ma_client::Config::new(&cfg.url, &cfg.token));
    let sync_cfg = cfg.sync_config();
    let covers = Arc::new(library::covers::CoverCache::new(sync_cfg.covers_dir(), client.config().http_url()));
    let mut actor = Actor {
        client: client.clone(),
        cfg: cfg.clone(),
        sync_cfg: sync_cfg.clone(),
        events,
        queue: None,
        queue_watch,
        queue_offset: 0,
        art_for: None,
        art: Default::default(),
        covers,
    };
    // What is mirrored already, before MA answers.
    actor.reload_library().await;

    let (sync, mut updates) = match library::sync::spawn(client.clone(), sync_cfg) {
        Ok(s) => s,
        Err(e) => {
            warn!("N.MUSIC: library mirror at {}: {e}", actor.cfg.data_dir.display());
            return;
        }
    };
    let (queue_tx, mut queue_rx) = mpsc::unbounded_channel::<String>();
    let (_player, mut player_events) = match &cfg.sendspin {
        Some(s) => {
            let mut pc = player::PlayerConfig::new(&s.url, &s.client_id, &s.name);
            pc.device = s.device.clone();
            info!(
                "N.MUSIC: built-in Sendspin player {:?} ({}) on {}, output {}",
                s.name,
                s.client_id,
                s.url,
                s.device.as_deref().unwrap_or("default")
            );
            let (p, rx) = player::Player::spawn(pc);
            if cfg.queue.is_none() {
                tokio::spawn(find_queue(client.clone(), s.client_id.clone(), queue_tx.clone()));
            }
            (Some(p), rx)
        }
        None => (None, mpsc::unbounded_channel().1),
    };
    if let Some(q) = &cfg.queue {
        let _ = queue_tx.send(q.clone());
    }
    let mut reload_due: Option<tokio::time::Instant> = None;
    loop {
        let reload = async {
            match reload_due {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            c = cmds.recv() => {
                let Some(c) = c else { return };
                actor.command(c, &mut queue_rx).await;
            }
            Some(q) = queue_rx.recv() => actor.set_queue(q).await,
            Some(ev) = ma_events.recv() => {
                sync.event(&ev);
                use ma_client::Event as E;
                match ev {
                    E::QueueUpdated(q) | E::QueueAdded(q) if Some(&q.queue_id) == actor.queue.as_ref() => {
                        if actor.queue_updated(&q) {
                            actor.refresh_queue_items().await;
                        }
                    }
                    E::QueueItemsUpdated { queue_id } if Some(&queue_id) == actor.queue.as_ref() => actor.refresh_queue_items().await,
                    E::Connected(_) => {
                        if let Some(q) = actor.queue.clone() {
                            if let Ok(state) = actor.client.queue(&q).await {
                                let _ = actor.queue_updated(&state);
                            }
                            actor.refresh_queue_items().await;
                        }
                    }
                    _ => {}
                }
            }
            Some(u) = updates.recv() => {
                use library::sync::Update as U;
                match u {
                    U::LibraryChanged(_) | U::AtlasReady { .. } | U::FullSyncDone { .. } => {
                        // a full sync sends many of these; read the mirror once they settle
                        reload_due = Some(tokio::time::Instant::now() + Duration::from_millis(500));
                    }
                    U::SyncFailed(e) => warn!("N.MUSIC: library sync: {e}"),
                    _ => {}
                }
            }
            Some(pe) = player_events.recv() => match pe {
                player::Event::StreamStarted { .. } => actor.send(MusicEvent::Stream(true)),
                player::Event::StreamEnded => actor.send(MusicEvent::Stream(false)),
                player::Event::OutputError(e) => warn!("N.MUSIC: audio output: {e}"),
                player::Event::Connected | player::Event::Disconnected(_) => {}
            },
            _ = reload => {
                reload_due = None;
                actor.reload_library().await;
            }
        }
    }
}

/// The BM5's queue in Music Assistant as the N.MUSIC actor knows it (`None` until MA lists the
/// built-in player). N.RADIO plays on it too.
pub type QueueWatch = watch::Receiver<Option<String>>;

/// Start the actor if Music Assistant is configured: returns the core's command sender and the
/// queue id as it becomes known.
pub fn spawn(
    rt: &tokio::runtime::Runtime,
    sim: bool,
    ma: &settings::MusicAssistant,
    events: mpsc::UnboundedSender<Event>,
) -> (Option<mpsc::UnboundedSender<MusicCmd>>, QueueWatch) {
    let (queue_tx, queue_rx) = watch::channel(None);
    let cfg = match MusicConfig::from_settings(ma, sim) {
        Ok(c) => c,
        Err(why) => {
            info!("N.MUSIC: Music Assistant not configured ({why}); browsing a made-up library");
            return (None, queue_rx);
        }
    };
    info!("N.MUSIC: Music Assistant at {}, library mirror in {}", cfg.url, cfg.data_dir.display());
    let (tx, rx) = mpsc::unbounded_channel();
    rt.spawn(run(cfg, rx, events, queue_tx));
    (Some(tx), queue_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(v: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let v: Vec<(String, String)> = v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        move |k| v.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone())
    }

    #[test]
    fn config_from_the_environment() {
        let t = || Some("t".to_string());
        assert!(MusicConfig::from_parts("http://ma:8095".into(), None, String::new(), false, vars(&[])).is_err());
        let c = MusicConfig::from_parts("http://192.168.2.238:8095".into(), t(), String::new(), true, vars(&[("HOME", "/h")]))
            .unwrap();
        let s = c.sendspin.unwrap();
        assert_eq!(s.url, "ws://192.168.2.238:8927/sendspin");
        assert!(s.client_id.starts_with("neomaster-sim-"), "{}", s.client_id);
        assert_eq!(s.device.as_deref(), Some("null"), "--sim is silent unless told otherwise");
        assert_eq!(c.queue, None);
        let c = MusicConfig::from_parts("ma:8095".into(), t(), "squeeze1".into(), false, vars(&[("NEOMASTER_PLAYER", "none")]))
            .unwrap();
        assert!(c.sendspin.is_none());
        assert_eq!(c.queue.as_deref(), Some("squeeze1"));
        assert!(MusicConfig::from_parts("ma".into(), t(), String::new(), false, vars(&[("NEOMASTER_PLAYER", "none")])).is_err());
    }

    #[test]
    fn hosts_of_urls() {
        assert_eq!(url_host("http://192.168.2.238:8095"), "192.168.2.238");
        assert_eq!(url_host("ws://ma/ws"), "ma");
        assert_eq!(url_host("ma:8095"), "ma");
    }

    /// The panel's library from a mirror: albums with their tracks, artists with their albums,
    /// in the store's order.
    #[test]
    fn library_from_the_mirror() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = library::sync::SyncConfig::new(dir.path());
        let mut store = Store::open(cfg.db_path()).unwrap();
        let item = |v: serde_json::Value| -> ma_client::MediaItem { serde_json::from_value(v).unwrap() };
        store
            .transaction(|s| {
                for v in [
                    serde_json::json!({"media_type": "artist", "item_id": "1", "uri": "library://artist/1", "name": "The Beatles", "sort_name": "beatles"}),
                    serde_json::json!({"media_type": "artist", "item_id": "2", "uri": "library://artist/2", "name": "ABBA", "sort_name": "abba"}),
                    serde_json::json!({"media_type": "album", "item_id": "10", "uri": "library://album/10", "name": "Abbey Road", "sort_name": "abbey road",
                        "artists": [{"item_id": "1", "name": "The Beatles", "uri": "library://artist/1"}]}),
                    serde_json::json!({"media_type": "track", "item_id": "100", "uri": "library://track/100", "name": "Something", "sort_name": "something",
                        "artists": [{"item_id": "1", "name": "The Beatles", "uri": "library://artist/1"}],
                        "album": {"item_id": "10", "name": "Abbey Road", "uri": "library://album/10"}, "disc_number": 1, "track_number": 2}),
                    serde_json::json!({"media_type": "track", "item_id": "101", "uri": "library://track/101", "name": "Come Together", "sort_name": "come together",
                        "artists": [{"item_id": "1", "name": "The Beatles", "uri": "library://artist/1"}],
                        "album": {"item_id": "10", "name": "Abbey Road", "uri": "library://album/10"}, "disc_number": 1, "track_number": 1}),
                ] {
                    s.upsert(&item(v), 1)?;
                }
                for k in Kind::ALL {
                    s.renumber(k)?;
                }
                Ok(())
            })
            .unwrap();
        let lib = load_library(&cfg).unwrap();
        assert_eq!(lib.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["ABBA", "The Beatles"]);
        let beatles = &lib.artists[lib.artist_by_uri("library://artist/1").unwrap()];
        assert_eq!(beatles.albums.len(), 1);
        let album = &lib.albums[beatles.albums[0]];
        assert_eq!(album.artist, "The Beatles");
        let titles: Vec<&str> = album.tracks.iter().map(|t| lib.tracks[*t].title.as_str()).collect();
        assert_eq!(titles, ["Come Together", "Something"], "disc and track order");
        assert_eq!(lib.tracks[album.tracks[0]].album, Some(beatles.albums[0]));
        assert!(lib.covers.is_none(), "no atlas baked");
    }
}
