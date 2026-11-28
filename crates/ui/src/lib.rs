//! neomaster's panel UI: the stock grammar, as captured on 27 Nov 2026. The pointer picks the
//! view, the menu wheel moves within it, LEFT drills in, RIGHT backs out, GO acts; a long GO opens
//! the context menu while held.
//!
//! [`Ui`] is plain state. [`Ui::input`] and [`Ui::tick`] change it and return [`Effect`]s for the
//! core to carry out (click the panel, play something, set the volume); [`Ui::scene`] turns it
//! into a [`render::Scene`] (the layout is in `draw.rs`). Nothing here blocks or touches a device.

pub mod arc;
pub mod assets;
mod draw;
pub mod fake;
pub mod idle;
pub mod library;
pub mod settings_menu;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bs5_panel::{Button, Input, COUNTS_PER_STEP};
use render::{Color, Image, Scene};

pub use assets::Assets;
pub use idle::IdleConfig;
use idle::{Idle, IdleAction};
pub use library::{Cover, CoverSource, Library, NowPlaying, PlayRequest, QueueEntry};

/// A monotonic timestamp, as a duration since an arbitrary fixed epoch.
pub type Time = Duration;

/// What the UI asks the rest of neomaster to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// One panel click (a list row, or the pointer crossing a zone).
    Click,
    /// Replace the play queue with this and play it.
    Play(PlayRequest),
    /// Append these (artist, album or track URIs) to the play queue.
    Enqueue(Vec<String>),
    /// Jump to this entry of the play queue.
    PlayQueueIndex(usize),
    /// Transport on the now-playing view: GO, LEFT, RIGHT.
    Transport(Transport),
    /// Absolute volume 0..=90, as the PC2 takes it.
    Volume(u8),
    SelectSource(&'static str),
    /// Blank-screen standby (false: LED on, backlight off) or wake (true: LED off, backlight on).
    Backlight(bool),
    /// Still blanked: turn the panel LED on again (the stock 10 s refresh).
    LedRefresh,
    /// Blank-screen standby stops local playback (stock `EnterBlankStandby`).
    StopLocalPlayback,
    /// The keyboard's standby key (the panel itself has none; see the open questions).
    Standby,
    /// The SETTINGS menu opened: the core should refresh [`Ui::set_facts`].
    SettingsOpened,
    /// A choice was made in the SETTINGS menu: these are the settings now. The core saves them
    /// and applies what changed.
    SettingsChanged(Box<settings::Settings>),
}

/// What the panel's buttons do on the now-playing view (Tore's ask after the first BM5 test:
/// GO plays and pauses, LEFT and RIGHT step tracks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    PlayPause,
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    NowPlaying,
    List,
    Covers,
    Menu,
    /// A two-column key/value page (SYSTEM INFO, NETWORK INFO).
    Info,
}

/// A pointer zone. Zones run from the bottom of the arc to the top: now-playing below the arc,
/// the labelled views, now-playing above it. The labelled ones sit at the stock label angles for
/// their count and the beam picks the nearest ([`arc`]).
#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub name: &'static str,
    pub kind: ViewKind,
    root: Root,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Root {
    None,
    Queue,
    Favourites,
    Titles,
    Albums,
    Artists,
    Covers,
    Mode,
    /// A SETTINGS section; its content is [`settings::menu`]'s.
    Settings(settings::menu::SectionId),
}

/// N.MUSIC as captured: now-playing at both ends, the lists in between.
pub fn n_music_zones() -> Vec<Zone> {
    use ViewKind::*;
    let z = |name, kind, root| Zone { name, kind, root };
    vec![
        z("NOW PLAYING", NowPlaying, Root::None),
        z("QUEUE", List, Root::Queue),
        z("FAVOURITES", List, Root::Favourites),
        z("TITLES", List, Root::Titles),
        z("ALBUMS", List, Root::Albums),
        z("ARTISTS", List, Root::Artists),
        z("COVERS", Covers, Root::Covers),
        z("MODE", Menu, Root::Mode),
        z("NOW PLAYING", NowPlaying, Root::None),
    ]
}

/// A pointer value that shows zone `i` of `zones`: the bottom or top end for now-playing, the
/// label's own angle for the others.
pub fn zone_pointer(zones: &[Zone], i: usize) -> u8 {
    let labelled: Vec<usize> = (0..zones.len()).filter(|z| zones[*z].kind != ViewKind::NowPlaying).collect();
    match labelled.iter().position(|z| *z == i) {
        Some(k) => arc::pointer_at(arc::label_angles(labelled.len())[labelled.len() - 1 - k]),
        None if i == 0 => 0,
        None => arc::TOP,
    }
}

/// SETTINGS: the normal menu's 7 sections or the hidden menu's 9, with MODE above them and
/// now-playing at both ends. The stock label angles put the hidden menu's sections exactly at
/// the pointer positions captured there (84 SYSTEM INFO, 79 SYSTEM SETTINGS, 75 MAINTENANCE, 71
/// and 66 MUSIC SERVICES, 61 SERVICE SETTINGS, 56 and 52 PRODUCT INFO, 46 NETWORK INFO, 41
/// NETWORK SETTINGS, 36 ROLES).
pub fn settings_zones(hidden: bool) -> Vec<Zone> {
    use settings::menu::SectionId::*;
    use ViewKind::*;
    let sections: &[settings::menu::SectionId] = if hidden {
        &[
            Roles,
            NetworkSettings,
            NetworkInfo,
            ProductInfo,
            ServiceSettings,
            MusicServices,
            Maintenance,
            SystemSettings,
            SystemInfo,
        ]
    } else {
        &[Roles, NetworkInfo, ServiceSettings, MusicServices, Maintenance, SystemSettings, SystemInfo]
    };
    let mut v = vec![Zone { name: "NOW PLAYING", kind: NowPlaying, root: Root::None }];
    for &id in sections {
        let kind = if matches!(id, SystemInfo | ProductInfo | NetworkInfo) { Info } else { Menu };
        v.push(Zone { name: id.title(), kind, root: Root::Settings(id) });
    }
    v.push(Zone { name: "MODE", kind: Menu, root: Root::Mode });
    v.push(Zone { name: "NOW PLAYING", kind: NowPlaying, root: Root::None });
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Artist(usize),
    Album(usize),
    Track(usize),
    /// An entry of the play queue (index into the queue).
    Queued(usize),
    Label(&'static str),
    Source(&'static str),
}

struct Level {
    items: Vec<Item>,
    cursor: usize,
    /// Where the list is drawn, easing towards `cursor`.
    scroll: f32,
    /// Sorted by name, so the wheel shows the first letter while it turns.
    alphabetical: bool,
    /// The album these tracks belong to (GO on one plays the album from that track).
    container: Option<String>,
}

impl Level {
    fn new(items: Vec<Item>) -> Level {
        Level { items, cursor: 0, scroll: 0.0, alphabetical: false, container: None }
    }
}

/// Art cache keys: an atlas slot, or a sleeve generated from an average colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ArtKey {
    Slot(u32),
    Rgb(u32),
}

/// The long-GO menu: four colour favourites, ADD TO QUEUE (where it opens), then playlists.
const CONTEXT_ENTRIES: [&str; 10] = ["RED", "GREEN", "YELLOW", "BLUE", "ADD TO  QUEUE", "01", "02", "03", "04", "05"];
const CONTEXT_DEFAULT: usize = 4;

/// What plays from outside the library (AirPlay): the now-playing view shows this instead of
/// the library's track while it is set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct External {
    /// The source's name, e.g. "AIRPLAY".
    pub source: String,
    /// Who is playing, e.g. a phone's name; may be empty.
    pub from: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The cover, ideally [`COVER_SIZE`] square (it is scaled to fit).
    pub cover: Option<Arc<Image>>,
    pub paused: bool,
}

/// The now-playing cover's size on screen, in pixels.
pub const COVER_SIZE: u32 = 512;
/// The MODE list when no settings say otherwise ([`Ui::set_settings`] relabels it).
const SOURCES: [&str; 4] = ["N.MUSIC", "N.RADIO", "A.AUX", "SETTINGS"];

/// A change of view: the old content fades out, the screen stays black, the new content fades
/// in (measured on the stock UI at 60 fps: about 100 ms, 230 ms and 100 ms).
#[derive(Clone, Copy, Debug)]
struct Switch {
    /// The zone whose content fades out (None after the zone set changed).
    from: Option<usize>,
    start: Time,
}

pub const FADE_OUT: Duration = Duration::from_millis(100);
pub const FADE_BLACK: Duration = Duration::from_millis(230);
pub const FADE_IN: Duration = Duration::from_millis(100);
/// How long the volume arc stays after the last change, then how long it takes to fade.
const VOLUME_HOLD: Duration = Duration::from_millis(1500);
const VOLUME_FADE: Duration = Duration::from_millis(300);
/// How long the wheel's letter stays after the last step.
const LETTER_HOLD: Duration = Duration::from_millis(1000);

pub struct Ui {
    lib: Library,
    assets: Assets,
    font: render::Font,
    /// Covers made into images so far (atlas slots, colour sleeves).
    art: HashMap<ArtKey, Arc<Image>>,
    zones: Vec<Zone>,
    /// One navigation stack per zone, so leaving a zone and coming back returns to the same place.
    stacks: Vec<Vec<Level>>,
    /// The pointer as the stock uses it: the panel's value snapped at the ends ([`arc::snap`]).
    pointer: u8,
    /// The panel's last pointer value, for the snapping's hysteresis.
    pointer_raw: u8,
    /// The beam's angle as drawn (radians, up is positive), easing towards the pointer's.
    beam: f32,
    beam_glow: arc::BeamGlow,
    menu_counts: i32,
    queue: Vec<QueueEntry>,
    now: Option<NowPlaying>,
    external: Option<External>,
    /// A source with nothing to show but its name (A.AUX): now-playing shows only that.
    plain_source: Option<String>,
    volume: u8,
    volume_shown_until: Time,
    /// Long GO: the context menu, open while GO is held.
    context: Option<usize>,
    context_scroll: f32,
    go_long: bool,
    last_step: Time,
    last_frame: Option<Time>,
    switch: Option<Switch>,
    /// The return timer fired (or a first standby press): the view shows now-playing until the
    /// pointer moves.
    idle_now_playing: bool,
    /// Return to now-playing, the standby tick and blank-screen standby, as the stock UI does.
    idle: Idle,
    /// The MODE list, top to bottom.
    sources: Vec<&'static str>,
    /// The SETTINGS menu and the settings it shows.
    settings_menu: settings_menu::SettingsMenu,
}

pub const MAX_VOLUME: u8 = 90;

impl Ui {
    /// The UI with the drawn assets and the bundled face; see [`Ui::with_assets`].
    pub fn new(lib: Library) -> Ui {
        Ui::with_assets(lib, Assets::builtin())
    }

    pub fn with_assets(lib: Library, assets: Assets) -> Ui {
        let zones = n_music_zones();
        let mut ui = Ui {
            lib,
            font: assets.font(),
            assets,
            art: HashMap::new(),
            stacks: Vec::new(),
            zones,
            pointer: 64,
            pointer_raw: 64,
            beam: arc::angle(64),
            beam_glow: arc::BeamGlow::default(),
            menu_counts: 0,
            queue: Vec::new(),
            now: None,
            external: None,
            plain_source: None,
            volume: 34, // stock default
            volume_shown_until: Time::ZERO,
            context: None,
            context_scroll: CONTEXT_DEFAULT as f32,
            go_long: false,
            last_step: Time::ZERO,
            last_frame: None,
            switch: None,
            idle_now_playing: false,
            idle: Idle::new(IdleConfig::default(), Time::ZERO),
            sources: SOURCES.to_vec(),
            settings_menu: settings_menu::SettingsMenu::default(),
        };
        ui.reset_stacks();
        ui
    }

    pub fn assets(&self) -> &Assets {
        &self.assets
    }

    fn reset_stacks(&mut self) {
        self.stacks = (0..self.zones.len()).map(|i| vec![self.root_level(self.zones[i].root)]).collect();
    }

    fn root_level(&self, root: Root) -> Level {
        let lib = &self.lib;
        let sorted = |mut l: Level| {
            l.alphabetical = true;
            l
        };
        match root {
            Root::None => Level::new(vec![]),
            Root::Queue => Level::new((0..self.queue.len()).map(Item::Queued).collect()),
            Root::Favourites => {
                Level::new(["RED", "GREEN", "YELLOW", "BLUE", "RECENTLY ADDED"].into_iter().map(Item::Label).collect())
            }
            Root::Titles => {
                let mut t: Vec<usize> = (0..lib.tracks.len()).collect();
                t.sort_by(|a, b| lib.tracks[*a].sort.cmp(&lib.tracks[*b].sort));
                sorted(Level::new(t.into_iter().map(Item::Track).collect()))
            }
            Root::Albums | Root::Covers => {
                let l = Level::new(lib.albums_in_order().into_iter().map(Item::Album).collect());
                if root == Root::Albums {
                    sorted(l)
                } else {
                    l
                }
            }
            Root::Artists => {
                let mut a: Vec<usize> = (0..lib.artists.len()).collect();
                a.sort_by(|x, y| lib.artists[*x].sort.cmp(&lib.artists[*y].sort));
                sorted(Level::new(a.into_iter().map(Item::Artist).collect()))
            }
            Root::Mode => Level::new(self.sources.iter().copied().map(Item::Source).collect()),
            // the settings sections keep their own place (settings_menu)
            Root::Settings(_) => Level::new(vec![]),
        }
    }

    fn zone_index(&self) -> usize {
        if self.idle_now_playing {
            return 0;
        }
        self.zone_at(self.pointer)
    }

    /// The zone the stock UI shows with the (snapped) pointer at `p`: now-playing once the beam is
    /// off the arc, otherwise the label nearest the beam.
    fn zone_at(&self, p: u8) -> usize {
        let a = arc::angle(p);
        let labelled = self.labelled_zones();
        if labelled.is_empty() || a < arc::BELOW_ARC {
            return 0;
        }
        if a > arc::ABOVE_ARC {
            return self.zones.len() - 1;
        }
        // labels are listed top first, zones bottom first
        let from_top = arc::label_index(a, arc::label_angles(labelled.len()));
        labelled[labelled.len() - 1 - from_top]
    }

    /// The zones with a label on the arc, bottom first.
    fn labelled_zones(&self) -> Vec<usize> {
        (0..self.zones.len()).filter(|i| self.zones[*i].kind != ViewKind::NowPlaying).collect()
    }

    /// The panel's pointer value as the stock uses it, after `raw` arrives.
    fn snapped(&self, raw: u8) -> u8 {
        arc::snap(raw, self.pointer_raw)
    }

    fn set_pointer(&mut self, raw: u8) {
        self.pointer = self.snapped(raw);
        self.pointer_raw = raw;
    }

    pub fn zone(&self) -> &Zone {
        &self.zones[self.zone_index()]
    }

    fn level_mut(&mut self) -> Option<&mut Level> {
        let z = self.zone_index();
        if matches!(self.zones[z].kind, ViewKind::NowPlaying | ViewKind::Info) {
            return None;
        }
        self.stacks[z].last_mut()
    }

    fn level(&self) -> Option<&Level> {
        self.level_of(self.zone_index())
    }

    fn level_of(&self, z: usize) -> Option<&Level> {
        if matches!(self.zones[z].kind, ViewKind::NowPlaying | ViewKind::Info) {
            return None;
        }
        self.stacks[z].last()
    }

    fn label(&self, item: Item) -> String {
        match item {
            Item::Artist(a) => self.lib.artists[a].name.clone(),
            Item::Album(a) => self.lib.albums[a].title.clone(),
            Item::Track(t) => self.lib.tracks[t].title.clone(),
            Item::Queued(q) => self.queue.get(q).map(|e| e.title.clone()).unwrap_or_default(),
            Item::Label(s) | Item::Source(s) => s.to_string(),
        }
    }

    /// The Music Assistant URI of a browsable item.
    fn uri_of(&self, item: Item) -> Option<&str> {
        match item {
            Item::Artist(a) => Some(&self.lib.artists[a].uri),
            Item::Album(a) => Some(&self.lib.albums[a].uri),
            Item::Track(t) => Some(&self.lib.tracks[t].uri),
            Item::Queued(q) => self.queue.get(q).and_then(|e| e.uri.as_deref()),
            _ => None,
        }
    }

    pub fn in_settings(&self) -> bool {
        self.zones.iter().any(|z| matches!(z.root, Root::Settings(_)))
    }

    fn drill(&self, item: Item) -> Option<Level> {
        match item {
            Item::Artist(a) => Some(Level::new(self.lib.artists[a].albums.iter().map(|x| Item::Album(*x)).collect())),
            Item::Album(a) => {
                let mut l = Level::new(self.lib.albums[a].tracks.iter().map(|x| Item::Track(*x)).collect());
                l.container = Some(self.lib.albums[a].uri.clone());
                Some(l)
            }
            Item::Label(_) => Some(Level::new(vec![])),
            _ => None,
        }
    }

    fn current_item(&self) -> Option<Item> {
        self.level().and_then(|l| l.items.get(l.cursor).copied())
    }

    fn refresh_queue(&mut self) {
        let q = self.root_level(Root::Queue);
        if let Some(z) = self.zones.iter().position(|z| z.root == Root::Queue) {
            let cursor = self.stacks[z][0].cursor.min(q.items.len().saturating_sub(1));
            self.stacks[z][0].items = q.items;
            self.stacks[z][0].cursor = cursor;
        }
    }

    /// Start (or redirect) the fade between views whenever the zone shown changes.
    fn note_view_change(&mut self, before: usize, now: Time) {
        if self.zone_index() == before {
            return;
        }
        match self.switch {
            // Still fading out or black: the new target simply replaces the old one.
            Some(sw) if now.saturating_sub(sw.start) < FADE_OUT + FADE_BLACK => {}
            // Fading in (or done): fade the shown view out again.
            _ => self.switch = Some(Switch { from: Some(before), start: now }),
        }
    }

    /// Feed one panel input.
    pub fn input(&mut self, input: Input, now: Time) -> Vec<Effect> {
        let mut fx = Vec::new();
        let before = self.zone_index();
        let source_active = self.now.as_ref().is_some_and(|n| n.playing) || self.idle.playing();
        if self.idle.input(now, source_active) {
            // Any input while blanked turns the backlight on and is otherwise swallowed, so the
            // first touch does not also scroll.
            fx.push(Effect::Backlight(true));
            if let Input::Pointer(p) = input {
                self.set_pointer(p);
                self.beam = arc::angle(self.pointer);
            }
            return fx;
        }
        if let Some(fx) = self.settings_input(input, now) {
            return fx;
        }
        match input {
            Input::Pointer(p) => {
                self.idle_now_playing = false;
                self.set_pointer(p);
                if self.zone_index() != before {
                    fx.push(Effect::Click);
                }
            }
            Input::MenuWheel(counts) => {
                self.menu_counts += counts;
                let steps = self.menu_counts / COUNTS_PER_STEP;
                self.menu_counts -= steps * COUNTS_PER_STEP;
                if steps != 0 {
                    fx.extend(self.step(steps));
                    self.last_step = now;
                }
            }
            Input::VolumeWheel(counts) => {
                let v = (self.volume as i32 + counts).clamp(0, MAX_VOLUME as i32) as u8;
                self.volume_shown_until = now + VOLUME_HOLD;
                if v != self.volume {
                    self.volume = v;
                    fx.push(Effect::Volume(v));
                }
            }
            Input::Pressed(Button::Go) => self.go_long = false,
            // a long GO on now-playing is not a play/pause
            Input::LongPress(Button::Go) if self.on_now_playing() => self.go_long = true,
            Input::LongPress(Button::Go) => {
                if matches!(self.current_item(), Some(Item::Track(_) | Item::Album(_) | Item::Artist(_))) {
                    self.go_long = true;
                    self.context = Some(CONTEXT_DEFAULT);
                    self.context_scroll = CONTEXT_DEFAULT as f32;
                    fx.push(Effect::Click);
                }
            }
            Input::Released(Button::Go) if self.context.is_none() && !self.go_long && self.on_now_playing() => {
                fx.extend(self.transport(Transport::PlayPause));
            }
            Input::Pressed(Button::Left) if self.on_now_playing() => fx.extend(self.transport(Transport::Previous)),
            Input::Pressed(Button::Right) if self.on_now_playing() => fx.extend(self.transport(Transport::Next)),
            Input::Released(Button::Go) => {
                if let Some(entry) = self.context.take() {
                    if entry == CONTEXT_DEFAULT {
                        if let Some(item) = self.current_item() {
                            fx.extend(self.enqueue(item));
                        }
                    }
                } else if !self.go_long {
                    fx.extend(self.go(now));
                }
            }
            Input::Pressed(Button::Left) => {
                if let Some(item) = self.current_item() {
                    if let Some(level) = self.drill(item) {
                        let z = self.zone_index();
                        self.stacks[z].push(level);
                        fx.push(Effect::Click);
                    }
                }
            }
            Input::Pressed(Button::Right) => {
                let z = self.zone_index();
                if self.stacks[z].len() > 1 && self.zones[z].kind != ViewKind::NowPlaying {
                    self.stacks[z].pop();
                    fx.push(Effect::Click);
                }
            }
            _ => {}
        }
        self.note_view_change(before, now);
        fx
    }

    /// A short standby (the keyboard's S in `--sim`). As captured on the stock UI: the first
    /// press while browsing only returns to now-playing; a press on now-playing is the real
    /// standby, which the core sends on and which comes back as [`Ui::blank`].
    pub fn standby(&mut self, now: Time) -> Vec<Effect> {
        if self.idle.blanked() {
            return vec![Effect::Standby];
        }
        let source_active = self.now.as_ref().is_some_and(|n| n.playing) || self.idle.playing();
        self.idle.input(now, source_active);
        self.context = None;
        if self.zone().kind != ViewKind::NowPlaying {
            let before = self.zone_index();
            self.idle_now_playing = true;
            self.note_view_change(before, now);
            return vec![];
        }
        vec![Effect::Standby]
    }

    /// Local playback started or stopped (paused counts as stopped). While something plays, the
    /// idle blank waits for the long timeout (stock: 12 h) instead of the first half-hour tick.
    pub fn set_playing(&mut self, playing: bool) {
        self.idle.set_playing(playing);
    }

    /// Show what an outside source (AirPlay) plays, or `None` to go back to the library's track.
    pub fn set_external(&mut self, external: Option<External>) {
        self.external = external;
    }

    /// A source that has no track, cover or sender (A.AUX, a line in): now-playing shows just its
    /// name. `None` goes back to the library's track (or the outside source's).
    pub fn set_plain_source(&mut self, name: Option<&str>) {
        self.plain_source = name.map(str::to_owned);
    }

    pub fn plain_source(&self) -> Option<&str> {
        self.plain_source.as_deref()
    }

    pub fn external(&self) -> Option<&External> {
        self.external.as_ref()
    }

    pub fn external_mut(&mut self) -> Option<&mut External> {
        self.external.as_mut()
    }

    /// Something started playing by itself (a device began AirPlay streaming): show
    /// now-playing, waking the panel from blank-screen standby if needed.
    pub fn show_now_playing(&mut self, now: Time) -> Vec<Effect> {
        let woke = self.idle.input(now, true);
        self.context = None;
        self.idle_now_playing = true;
        if woke {
            vec![Effect::Backlight(true)]
        } else {
            vec![]
        }
    }

    /// Our source stopped. As the stock UI does on its source-status event, blank at once unless
    /// something local still listens (`local_listener`, e.g. the audio master's speakers on TV).
    pub fn source_stopped(&mut self, local_listener: bool, now: Time) -> Vec<Effect> {
        self.idle.set_playing(false);
        if local_listener {
            return vec![];
        }
        self.blank(now)
    }

    /// The idle timeouts (stock defaults; meant to come from config).
    pub fn idle_config(&mut self) -> &mut IdleConfig {
        &mut self.idle.cfg
    }

    pub fn blanked(&self) -> bool {
        self.idle.blanked()
    }

    fn step(&mut self, steps: i32) -> Vec<Effect> {
        if let Some(c) = self.context.as_mut() {
            let new = (*c as i32 + steps).clamp(0, CONTEXT_ENTRIES.len() as i32 - 1) as usize;
            if new == *c {
                return vec![];
            }
            *c = new;
            return vec![Effect::Click];
        }
        let Some(level) = self.level_mut() else { return vec![] };
        if level.items.is_empty() {
            return vec![];
        }
        let new = (level.cursor as i32 + steps).clamp(0, level.items.len() as i32 - 1) as usize;
        if new == level.cursor {
            return vec![];
        }
        level.cursor = new;
        vec![Effect::Click]
    }

    /// The now-playing view is showing (pointer at either end, or the idle return).
    fn on_now_playing(&self) -> bool {
        self.zone().kind == ViewKind::NowPlaying
    }

    fn transport(&mut self, t: Transport) -> Vec<Effect> {
        if t == Transport::PlayPause {
            // shown at once; Music Assistant's report follows
            if let Some(np) = self.now.as_mut() {
                np.playing = !np.playing;
            }
        }
        vec![Effect::Click, Effect::Transport(t)]
    }

    fn go(&mut self, now: Time) -> Vec<Effect> {
        match self.current_item() {
            Some(Item::Source(s)) => {
                // SETTINGS swaps the arc's zones for the settings pages; any real source swaps
                // them back.
                let settings = s == "SETTINGS";
                let mut fx = vec![];
                if settings != self.in_settings() {
                    let hidden = settings && self.take_settings_combo(now);
                    self.zones = if settings { settings_zones(hidden) } else { n_music_zones() };
                    self.reset_stacks();
                    // MODE keeps its cursor where it was
                    if let Some(z) = self.zones.iter().position(|z| z.root == Root::Mode) {
                        let i = self.sources.iter().position(|x| *x == s).unwrap_or(0);
                        self.stacks[z][0].cursor = i;
                        self.stacks[z][0].scroll = i as f32;
                    }
                    self.switch = Some(Switch { from: None, start: now });
                    if settings {
                        fx.extend(self.settings_opened(hidden));
                    }
                }
                fx.push(Effect::SelectSource(s));
                fx
            }
            Some(item @ (Item::Track(_) | Item::Album(_) | Item::Artist(_))) => {
                let Some(uri) = self.uri_of(item).map(str::to_owned) else { return vec![] };
                let container = self.level().and_then(|l| l.container.clone());
                let req = match (item, container) {
                    (Item::Track(_), Some(album)) => PlayRequest { uri: album, start: Some(uri) },
                    _ => PlayRequest { uri, start: None },
                };
                // Show it at once; Music Assistant's own report replaces this when it comes.
                self.now = self.guess_now_playing(item);
                self.idle.set_playing(true);
                vec![Effect::Play(req)]
            }
            Some(Item::Queued(q)) => {
                if let Some(e) = self.queue.get(q) {
                    let np = NowPlaying {
                        title: e.title.clone(),
                        artist: e.artist.clone(),
                        uri: e.uri.clone(),
                        cover: e.cover,
                        playing: true,
                        ..Default::default()
                    };
                    self.now = Some(np);
                }
                self.idle.set_playing(true);
                vec![Effect::PlayQueueIndex(q)]
            }
            _ => vec![],
        }
    }

    /// Timers: return to now-playing, the standby tick, the LED refresh while blanked.
    pub fn tick(&mut self, now: Time) -> Vec<Effect> {
        let mut fx = Vec::new();
        for a in self.idle.tick(now) {
            match a {
                IdleAction::ReturnToNowPlaying => {
                    if self.context.is_none() {
                        let before = self.zone_index();
                        self.idle_now_playing = true;
                        self.note_view_change(before, now);
                    }
                }
                IdleAction::Blank => {
                    self.context = None;
                    self.idle_now_playing = true;
                    fx.extend(Self::blank_effects());
                }
                IdleAction::LedRefresh => fx.push(Effect::LedRefresh),
            }
        }
        fx
    }

    fn blank_effects() -> [Effect; 2] {
        [Effect::StopLocalPlayback, Effect::Backlight(false)]
    }

    /// When the next frame is due: in one frame while something moves, at the end of the volume
    /// overlay or the wheel letter, or never.
    pub fn next_frame(&self, now: Time) -> Option<Time> {
        if self.idle.blanked() {
            return None;
        }
        // One frame after the last one drawn, so a caller that checks "is it due yet" at the
        // deadline finds it due.
        let next = self.last_frame.unwrap_or(now) + Duration::from_micros(16_667);
        let soon = Some(next);
        // Judged by the last frame drawn, not by `now`: the frame that shows the end of a fade
        // is still owed when the caller asks after the fade's end.
        let drawn = self.last_frame.unwrap_or(Time::ZERO);
        let moving = self.level().is_some_and(|l| (l.scroll - l.cursor as f32).abs() > 0.002)
            || self.context.is_some_and(|c| (self.context_scroll - c as f32).abs() > 0.002)
            || self.switch.is_some()
            || self.beam != arc::angle(self.pointer);
        if moving {
            return soon;
        }
        // The beam dims a second after it stops, while the arc is showing.
        let glow =
            if self.zones[self.zone_index()].kind == ViewKind::NowPlaying { None } else { self.beam_glow.next_change(next) };
        let volume_end = self.volume_shown_until + VOLUME_FADE;
        let volume = (self.volume_shown_until > Time::ZERO && drawn < volume_end).then_some(if now < self.volume_shown_until {
            self.volume_shown_until
        } else {
            next
        });
        let letter_end = self.last_step + LETTER_HOLD;
        let letter = (drawn < letter_end && self.level().is_some_and(|l| l.alphabetical)).then_some(letter_end);
        [volume, letter, glow].into_iter().flatten().min()
    }

    /// The next timer [`Ui::tick`] needs, if any.
    pub fn next_timer(&self) -> Option<Time> {
        self.idle.next_deadline()
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// The volume as the PC2 reports it (or a neutral value where the room's master owns it).
    /// Updates what the wheel turns from; emits nothing.
    pub fn set_volume(&mut self, volume: u8) {
        self.volume = volume.min(MAX_VOLUME);
    }

    /// Blank-screen standby now (standby, long standby, or our source released): local
    /// playback stops, LED on, backlight off until the next input.
    pub fn blank(&mut self, now: Time) -> Vec<Effect> {
        if !self.idle.blank(now) {
            return vec![];
        }
        self.context = None;
        self.idle_now_playing = true;
        self.switch = None;
        Self::blank_effects().to_vec()
    }

    /// Show another library (the mirror changed). Every list keeps its place where the item
    /// it was on still exists.
    pub fn set_library(&mut self, lib: Library) {
        // per zone: for each level, the URI under its cursor
        let saved: Vec<Vec<Option<String>>> = self
            .stacks
            .iter()
            .map(|st| st.iter().map(|l| l.items.get(l.cursor).and_then(|i| self.uri_of(*i)).map(str::to_owned)).collect())
            .collect();
        self.lib = lib;
        self.art.clear();
        self.reset_stacks();
        for (z, path) in saved.into_iter().enumerate() {
            if matches!(self.zones[z].root, Root::Queue | Root::Mode | Root::Settings(_)) {
                continue;
            }
            for (depth, uri) in path.iter().enumerate() {
                let Some(uri) = uri else { break };
                let Some(level) = self.stacks[z].get_mut(depth) else { break };
                let found = level.items.iter().position(|i| match *i {
                    Item::Artist(a) => self.lib.artists[a].uri == *uri,
                    Item::Album(a) => self.lib.albums[a].uri == *uri,
                    Item::Track(t) => self.lib.tracks[t].uri == *uri,
                    _ => false,
                });
                let Some(pos) = found else { break };
                level.cursor = pos;
                level.scroll = pos as f32;
                if depth + 1 < path.len() {
                    let item = level.items[pos];
                    match self.drill(item) {
                        Some(next) => self.stacks[z].push(next),
                        None => break,
                    }
                }
            }
        }
        if let Some(np) = self.now.as_mut() {
            // the cover's atlas slot may have moved
            np.cover = Self::cover_in(&self.lib, np.uri.as_deref(), np.album_uri.as_deref()).unwrap_or(np.cover);
        }
    }

    pub fn library(&self) -> &Library {
        &self.lib
    }

    /// A cover from the library for a track or album URI.
    fn cover_in(lib: &Library, track: Option<&str>, album: Option<&str>) -> Option<Cover> {
        let from_album = album.and_then(|a| lib.album_by_uri(a)).map(|a| lib.albums[a].cover);
        let from_track = track.and_then(|t| lib.track_by_uri(t)).map(|t| lib.tracks[t].cover);
        from_album.filter(|c| c.slot.is_some()).or(from_track.filter(|c| c.slot.is_some())).or(from_album).or(from_track)
    }

    /// What Music Assistant says plays now (`None`: nothing queued). Its cover comes from the
    /// library when the report has none of its own.
    pub fn set_now_playing(&mut self, np: Option<NowPlaying>) {
        self.now = np.map(|mut np| {
            if np.cover == Cover::default() {
                np.cover = Self::cover_in(&self.lib, np.uri.as_deref(), np.album_uri.as_deref()).unwrap_or_default();
            }
            // keep the large cover while the same item plays (MA reports every state change)
            if np.art.is_none() {
                if let Some(old) = &self.now {
                    if old.uri == np.uri && old.cover == np.cover {
                        np.art = old.art.clone();
                    }
                }
            }
            np
        });
        let playing = self.now.as_ref().is_some_and(|n| n.playing);
        self.idle.set_playing(playing);
    }

    /// The large cover for the now-playing view, fetched for the item with this URI.
    pub fn set_now_playing_art(&mut self, uri: Option<&str>, art: Arc<Image>) {
        if let Some(np) = self.now.as_mut().filter(|n| n.uri.as_deref() == uri) {
            np.art = Some(art);
        }
    }

    pub fn now_playing(&self) -> Option<&NowPlaying> {
        self.now.as_ref()
    }

    /// The play queue as Music Assistant has it.
    pub fn set_queue(&mut self, queue: Vec<QueueEntry>) {
        self.queue = queue;
        self.refresh_queue();
    }

    /// The album `delta` albums from the one playing, in ALBUMS order (the remote's album
    /// step). From nothing, or an album outside the library: the first or last album.
    pub fn album_step(&self, delta: i32) -> Option<String> {
        let order = self.lib.albums_in_order();
        if order.is_empty() {
            return None;
        }
        let playing = self.now.as_ref().and_then(|n| n.album_uri.as_deref()).and_then(|u| self.lib.album_by_uri(u));
        let at = playing.and_then(|a| order.iter().position(|x| *x == a));
        let i = match at {
            Some(i) => (i as i32 + delta).rem_euclid(order.len() as i32) as usize,
            None if delta < 0 => order.len() - 1,
            None => 0,
        };
        Some(self.lib.albums[order[i]].uri.clone())
    }

    /// What GO on `item` will most likely play, until Music Assistant says.
    fn guess_now_playing(&self, item: Item) -> Option<NowPlaying> {
        let lib = &self.lib;
        let (track, album) = match item {
            Item::Track(t) => (Some(t), lib.tracks[t].album),
            Item::Album(a) => (lib.albums[a].tracks.first().copied(), Some(a)),
            Item::Artist(a) => {
                let album = lib.artists[a].albums.first().copied();
                (album.and_then(|al| lib.albums[al].tracks.first().copied()), album)
            }
            _ => return None,
        };
        let mut np = NowPlaying { playing: true, ..Default::default() };
        if let Some(t) = track {
            np.title = lib.tracks[t].title.clone();
            np.artist = lib.tracks[t].artist.clone();
            np.uri = Some(lib.tracks[t].uri.clone());
            np.cover = lib.tracks[t].cover;
        }
        if let Some(a) = album {
            np.album = lib.albums[a].title.clone();
            np.album_uri = Some(lib.albums[a].uri.clone());
            if np.artist.is_empty() {
                np.artist = lib.albums[a].artist.clone();
            }
            if np.cover.slot.is_none() {
                np.cover = lib.albums[a].cover;
            }
        }
        Some(np)
    }

    /// ADD TO QUEUE: Music Assistant appends `item`; the queue view shows the library's tracks
    /// of it until MA reports the real queue.
    fn enqueue(&mut self, item: Item) -> Vec<Effect> {
        let Some(uri) = self.uri_of(item).map(str::to_owned) else { return vec![] };
        let lib = &self.lib;
        let tracks: Vec<usize> = match item {
            Item::Track(t) => vec![t],
            Item::Album(a) => lib.albums[a].tracks.clone(),
            Item::Artist(a) => lib.artists[a].albums.iter().flat_map(|al| lib.albums[*al].tracks.clone()).collect(),
            _ => vec![],
        };
        let entries: Vec<QueueEntry> = tracks
            .into_iter()
            .map(|t| QueueEntry {
                title: lib.tracks[t].title.clone(),
                artist: lib.tracks[t].artist.clone(),
                uri: Some(lib.tracks[t].uri.clone()),
                cover: lib.tracks[t].cover,
            })
            .collect();
        self.queue.extend(entries);
        self.refresh_queue();
        vec![Effect::Enqueue(vec![uri])]
    }

    fn animate(&mut self, now: Time) {
        let dt = self.last_frame.map(|t| now.saturating_sub(t).as_secs_f32()).unwrap_or(1.0).min(0.25);
        self.last_frame = Some(now);
        // One row per wheel tick, eased over a few frames.
        let k = 1.0 - (-dt / 0.045).exp();
        let ease = |pos: &mut f32, target: f32| {
            *pos += (target - *pos) * k;
            if (*pos - target).abs() <= 0.002 {
                *pos = target;
            }
        };
        if let Some(l) = self.level_mut() {
            let target = l.cursor as f32;
            ease(&mut l.scroll, target);
        }
        if let Some(c) = self.context {
            ease(&mut self.context_scroll, c as f32);
        }
        // the beam, as SelectorBeamControl_Update does it
        let target = arc::angle(self.pointer);
        let moving = self.beam != target;
        self.beam = arc::ease(self.beam, target, dt);
        self.beam_glow.step(moving, now, dt);
        if self.switch.is_some_and(|sw| now >= sw.start + FADE_OUT + FADE_BLACK + FADE_IN) {
            self.switch = None;
        }
    }

    /// Build the frame for `now`.
    pub fn scene(&mut self, now: Time) -> Scene {
        let mut s = Scene::new(Color::BLACK);
        if self.idle.blanked() {
            return s;
        }
        self.animate(now);
        self.paint(&mut s, now);
        s
    }

    /// A cover as an image: the atlas slot, else a sleeve in the cover's average colour, else
    /// the stock default cover. Made once and kept.
    fn art(&mut self, cover: Cover) -> Arc<Image> {
        if let (Some(slot), Some(src)) = (cover.slot, self.lib.covers.clone()) {
            if let Some(img) = self.art.get(&ArtKey::Slot(slot)) {
                return img.clone();
            }
            if let Some(img) = src.cover(slot) {
                self.art.insert(ArtKey::Slot(slot), img.clone());
                return img;
            }
        }
        let Some(rgb) = cover.rgb else { return self.assets.cover_medium.clone() };
        let (r, g, b) = ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff);
        self.art
            .entry(ArtKey::Rgb(rgb))
            .or_insert_with(|| {
                let (r, g, b) = (r as f32, g as f32, b as f32);
                Image::from_fn(256, 256, |x, y| {
                    let shade = 1.25 - 0.6 * (x + y) / 512.0;
                    let ring = ((x - 176.0).hypot(y - 88.0) - 60.0).abs() < 14.0;
                    let k = if ring { shade * 1.25 } else { shade };
                    let c = |v: f32| (v * k).round().clamp(0.0, 255.0) as u8;
                    [c(r), c(g), c(b), 255]
                })
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Time {
        Duration::from_millis(n)
    }

    fn ui_at(pointer: u8) -> Ui {
        let mut ui = Ui::new(Library::generate());
        ui.input(Input::Pointer(pointer), ms(0));
        ui
    }

    fn texts(s: &Scene) -> usize {
        s.items.iter().filter(|p| matches!(p, render::Primitive::Text { .. })).count()
    }

    #[test]
    fn external_source_replaces_the_library_track_on_now_playing() {
        let mut ui = ui_at(5);
        let cover = Image::new(2, 2, vec![0; 16]);
        ui.set_external(Some(External {
            source: "AIRPLAY".into(),
            from: "Tore's iPhone".into(),
            title: "All of Me".into(),
            artist: "Django Reinhardt".into(),
            album: "Legendary Recordings".into(),
            cover: Some(cover.clone()),
            paused: true,
        }));
        let scene = ui.scene(ms(10_000));
        let texts: Vec<&str> = scene
            .items
            .iter()
            .filter_map(|p| match p {
                render::Primitive::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"AIRPLAY · Tore's iPhone · PAUSED"), "{texts:?}");
        assert!(texts.contains(&"All of Me"), "{texts:?}");
        assert!(texts.contains(&"Django Reinhardt / Legendary Recordings"), "{texts:?}");
        let drawn = |s: &Scene| s.items.iter().any(|p| matches!(p, render::Primitive::Image(d) if Arc::ptr_eq(&d.image, &cover)));
        assert!(drawn(&scene));
        ui.set_external(None);
        assert!(!drawn(&ui.scene(ms(20_000))));
    }

    #[test]
    fn plain_source_shows_only_its_name_on_now_playing() {
        let mut ui = ui_at(5);
        let texts_of = |s: &Scene| -> Vec<String> {
            s.items
                .iter()
                .filter_map(|p| match p {
                    render::Primitive::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let images = |s: &Scene| s.items.iter().filter(|p| matches!(p, render::Primitive::Image(_))).count();
        ui.set_now_playing(Some(NowPlaying {
            title: "Some Track".into(),
            artist: "Someone".into(),
            playing: true,
            ..Default::default()
        }));
        let before = ui.scene(ms(10_000));
        assert!(texts_of(&before).contains(&"Some Track".to_string()));
        ui.set_plain_source(Some("A.AUX"));
        let scene = ui.scene(ms(20_000));
        let texts = texts_of(&scene);
        assert!(texts.contains(&"A.AUX".to_string()), "{texts:?}");
        // no cover, no title, no caption
        assert!(images(&scene) < images(&before));
        assert!(!texts.iter().any(|t| t.contains("Some")), "{texts:?}");
        assert_eq!(texts.len(), texts_of(&before).len() - 1, "{texts:?}");
        // it wins over an outside source's view
        ui.set_external(Some(External { source: "AIRPLAY".into(), title: "x".into(), ..Default::default() }));
        assert!(!texts_of(&ui.scene(ms(30_000))).contains(&"x".to_string()));
        ui.set_plain_source(None);
        assert!(texts_of(&ui.scene(ms(40_000))).contains(&"x".to_string()));
    }

    #[test]
    fn show_now_playing_wakes_a_blanked_panel() {
        let mut ui = ui_at(40);
        assert_eq!(ui.zone().kind, ViewKind::List);
        assert!(!ui.blank(ms(0)).is_empty());
        assert_eq!(ui.show_now_playing(ms(100)), vec![Effect::Backlight(true)]);
        assert!(!ui.blanked());
        assert_eq!(ui.zone().kind, ViewKind::NowPlaying);
        assert_eq!(ui.show_now_playing(ms(200)), vec![]);
    }

    #[test]
    fn pointer_picks_the_view_and_clicks_on_zone_change() {
        let mut ui = ui_at(76);
        assert_eq!(ui.zone().name, "ARTISTS");
        assert_eq!(ui.input(Input::Pointer(77), ms(1)), vec![]);
        assert_eq!(ui.input(Input::Pointer(70), ms(2)), vec![Effect::Click]);
        assert_eq!(ui.zone().name, "ALBUMS");
        ui.input(Input::Pointer(120), ms(3));
        assert_eq!(ui.zone().kind, ViewKind::NowPlaying);
    }

    #[test]
    fn wheel_counts_become_steps_with_one_click_each() {
        let mut ui = ui_at(76);
        assert_eq!(ui.input(Input::MenuWheel(10), ms(1)), vec![]);
        assert_eq!(ui.input(Input::MenuWheel(10), ms(2)), vec![Effect::Click]); // 15 counts = 1 step
        assert_eq!(ui.level().unwrap().cursor, 1);
        ui.input(Input::MenuWheel(-45), ms(3)); // clamps at the top
        assert_eq!(ui.level().unwrap().cursor, 0);
    }

    #[test]
    fn left_drills_right_backs_and_zones_keep_their_place() {
        let mut ui = ui_at(76);
        ui.input(Input::MenuWheel(30), ms(1));
        ui.input(Input::Pressed(Button::Left), ms(2)); // artist -> albums
        ui.input(Input::Pressed(Button::Left), ms(3)); // album -> tracks
        assert!(matches!(ui.current_item(), Some(Item::Track(_))));
        ui.input(Input::Pointer(70), ms(4));
        ui.input(Input::Pointer(76), ms(5));
        assert!(matches!(ui.current_item(), Some(Item::Track(_))));
        ui.input(Input::Pressed(Button::Right), ms(6));
        ui.input(Input::Pressed(Button::Right), ms(7));
        assert_eq!(ui.current_item(), Some(Item::Artist(2)));
    }

    #[test]
    fn go_plays_and_long_go_enqueues() {
        let mut ui = ui_at(62);
        ui.input(Input::Pressed(Button::Go), ms(1));
        let fx = ui.input(Input::Released(Button::Go), ms(50));
        assert!(matches!(fx[..], [Effect::Play(_)]));
        ui.input(Input::Pressed(Button::Go), ms(100));
        ui.input(Input::LongPress(Button::Go), ms(900)); // opens on ADD TO QUEUE
        let fx = ui.input(Input::Released(Button::Go), ms(1000));
        assert!(matches!(fx[..], [Effect::Enqueue(ref u)] if u.len() == 1));
        ui.input(Input::Pointer(30), ms(1100));
        assert_eq!(ui.zone().name, "QUEUE");
        assert_eq!(ui.level().unwrap().items.len(), 1);
    }

    #[test]
    fn go_on_a_track_in_an_album_plays_the_album_from_it() {
        let mut ui = ui_at(70); // ALBUMS
        ui.input(Input::Pressed(Button::Left), ms(1)); // album -> its tracks
        ui.input(Input::MenuWheel(15), ms(2)); // the second track
        ui.input(Input::Pressed(Button::Go), ms(3));
        let fx = ui.input(Input::Released(Button::Go), ms(4));
        let lib = Library::generate();
        let album = &lib.albums[lib.albums_in_order()[0]];
        let track = &lib.tracks[album.tracks[1]];
        assert_eq!(fx, vec![Effect::Play(PlayRequest { uri: album.uri.clone(), start: Some(track.uri.clone()) })]);
        // shown at once, before Music Assistant reports it
        assert_eq!(ui.now_playing().unwrap().title, track.title);
        assert!(ui.is_playing(Item::Track(album.tracks[1])));
    }

    #[test]
    fn a_new_library_keeps_every_list_in_place() {
        let mut ui = ui_at(76); // ARTISTS
        ui.input(Input::MenuWheel(45), ms(1));
        ui.input(Input::Pressed(Button::Left), ms(2)); // artist -> albums
        let before = ui.current_item().map(|i| ui.uri_of(i).unwrap().to_owned());
        // the same library with one artist more, sorted first
        let mut lib = Library::generate();
        lib.artists.push(library::Artist { name: "0 First".into(), albums: vec![], uri: "x://new".into(), sort: "0".into() });
        ui.set_library(lib);
        assert_eq!(ui.stacks[ui.zone_index()].len(), 2, "still drilled into the artist");
        assert_eq!(ui.current_item().map(|i| ui.uri_of(i).unwrap().to_owned()), before);
        ui.input(Input::Pressed(Button::Right), ms(3));
        assert_eq!(ui.level().unwrap().cursor, 4, "the artist moved down one row with it");
    }

    #[test]
    fn album_step_follows_the_albums_list() {
        let mut ui = ui_at(0);
        let lib = Library::generate();
        let order = lib.albums_in_order();
        assert_eq!(ui.album_step(1), Some(lib.albums[order[0]].uri.clone()), "nothing playing: the first");
        ui.set_now_playing(Some(NowPlaying {
            album_uri: Some(lib.albums[order[5]].uri.clone()),
            playing: true,
            ..Default::default()
        }));
        assert_eq!(ui.album_step(1), Some(lib.albums[order[6]].uri.clone()));
        assert_eq!(ui.album_step(-1), Some(lib.albums[order[4]].uri.clone()));
        // a report without a cover takes the album's from the library
        assert_eq!(ui.now_playing().unwrap().cover, lib.albums[order[5]].cover);
    }

    #[test]
    fn the_queue_comes_from_music_assistant_and_go_jumps_in_it() {
        let mut ui = ui_at(30); // QUEUE
        let e = |t: &str| QueueEntry { title: t.into(), uri: Some(format!("x://{t}")), ..Default::default() };
        ui.set_queue(vec![e("one"), e("two"), e("three")]);
        ui.input(Input::MenuWheel(30), ms(1));
        ui.input(Input::Pressed(Button::Go), ms(2));
        assert_eq!(ui.input(Input::Released(Button::Go), ms(3)), vec![Effect::PlayQueueIndex(2)]);
    }

    #[test]
    fn now_playing_go_toggles_and_left_right_step_tracks() {
        let mut ui = ui_at(5); // NOW PLAYING
        ui.set_now_playing(Some(NowPlaying { title: "x".into(), playing: true, ..Default::default() }));
        ui.input(Input::Pressed(Button::Go), ms(1));
        assert_eq!(ui.input(Input::Released(Button::Go), ms(2)), vec![Effect::Click, Effect::Transport(Transport::PlayPause)]);
        assert!(!ui.now_playing().unwrap().playing, "shown paused at once");
        assert_eq!(ui.input(Input::Pressed(Button::Left), ms(3)), vec![Effect::Click, Effect::Transport(Transport::Previous)]);
        assert_eq!(ui.input(Input::Pressed(Button::Right), ms(4)), vec![Effect::Click, Effect::Transport(Transport::Next)]);
        // a long GO on now-playing opens nothing and plays nothing
        ui.input(Input::Pressed(Button::Go), ms(10));
        ui.input(Input::LongPress(Button::Go), ms(900));
        assert_eq!(ui.input(Input::Released(Button::Go), ms(1000)), vec![]);
        // elsewhere LEFT still drills in
        ui.input(Input::Pointer(76), ms(1100));
        assert_eq!(ui.input(Input::Pressed(Button::Left), ms(1200)), vec![Effect::Click]);
    }

    #[test]
    fn long_go_elsewhere_in_the_menu_does_not_enqueue() {
        let mut ui = ui_at(62);
        ui.input(Input::Pressed(Button::Go), ms(100));
        ui.input(Input::LongPress(Button::Go), ms(900));
        ui.input(Input::MenuWheel(-15), ms(950)); // to BLUE
        let fx = ui.input(Input::Released(Button::Go), ms(1000));
        assert!(fx.is_empty());
    }

    #[test]
    fn volume_is_clamped_and_reported() {
        let mut ui = ui_at(0);
        assert_eq!(ui.input(Input::VolumeWheel(3), ms(1)), vec![Effect::Volume(37)]);
        ui.input(Input::VolumeWheel(200), ms(2));
        assert_eq!(ui.volume(), MAX_VOLUME);
    }

    #[test]
    fn blanks_on_the_half_hour_tick_and_swallows_the_waking_input() {
        let mut ui = ui_at(76);
        assert_eq!(ui.tick(ms(1_799_999)), vec![]);
        assert_eq!(ui.tick(ms(1_800_000)), vec![Effect::StopLocalPlayback, Effect::Backlight(false)]);
        assert!(ui.scene(ms(1_800_001)).items.is_empty());
        assert_eq!(ui.tick(ms(1_810_000)), vec![Effect::LedRefresh]);
        assert_eq!(ui.input(Input::MenuWheel(15), ms(1_815_000)), vec![Effect::Backlight(true)]);
        assert_eq!(ui.zone().kind, ViewKind::NowPlaying);
        ui.input(Input::Pointer(77), ms(1_815_100));
        assert_eq!(ui.level().unwrap().cursor, 0); // the waking wheel turn did not scroll
    }

    #[test]
    fn returns_to_now_playing_after_three_minutes_once_something_plays() {
        let mut ui = ui_at(62);
        assert_eq!(ui.next_timer(), Some(ms(1_800_000))); // nothing playing: no return timer
        ui.input(Input::Pressed(Button::Go), ms(1));
        ui.input(Input::Released(Button::Go), ms(50));
        ui.input(Input::Pointer(76), ms(1000));
        ui.tick(ms(180_999));
        assert_eq!(ui.zone().name, "ARTISTS");
        ui.tick(ms(181_000));
        assert_eq!(ui.zone().kind, ViewKind::NowPlaying);
    }

    #[test]
    fn playing_keeps_the_screen_on_for_twelve_hours() {
        let mut ui = ui_at(62);
        ui.input(Input::Pressed(Button::Go), ms(1));
        ui.input(Input::Released(Button::Go), ms(50)); // plays (fake library)
        let t0 = 50;
        for k in 1..24u64 {
            assert!(!ui.tick(ms(t0 + k * 1_800_000)).contains(&Effect::Backlight(false)));
        }
        assert_eq!(ui.tick(ms(t0 + 24 * 1_800_000)), vec![Effect::StopLocalPlayback, Effect::Backlight(false)]);
    }

    #[test]
    fn first_standby_returns_to_now_playing_second_is_standby() {
        let mut ui = ui_at(76);
        assert_eq!(ui.standby(ms(10)), vec![]);
        assert_eq!(ui.zone().kind, ViewKind::NowPlaying);
        assert_eq!(ui.standby(ms(20)), vec![Effect::Standby]);
        // the core sends it on and the room's standby comes back as blank()
        assert_eq!(ui.blank(ms(21)), vec![Effect::StopLocalPlayback, Effect::Backlight(false)]);
        assert_eq!(ui.blank(ms(22)), vec![]);
    }

    #[test]
    fn source_stopped_blanks_unless_something_local_listens() {
        let mut ui = ui_at(0);
        ui.set_playing(true);
        assert_eq!(ui.source_stopped(true, ms(10)), vec![]);
        assert!(!ui.blanked());
        assert_eq!(ui.source_stopped(false, ms(20)), vec![Effect::StopLocalPlayback, Effect::Backlight(false)]);
        assert!(ui.blanked());
    }

    #[test]
    fn scrolling_animates_then_settles() {
        let mut ui = ui_at(76);
        // let the beam reach the pointer and dim
        for f in 0..180 {
            ui.scene(ms(17 * f));
        }
        assert_eq!(ui.next_frame(ms(3100)), None);
        ui.input(Input::MenuWheel(75), ms(3101));
        assert!(ui.next_frame(ms(3101)).is_some());
        for f in 0..60 {
            ui.scene(ms(3101 + 17 * f));
        }
        assert_eq!(ui.next_frame(ms(4200)), None);
    }

    #[test]
    fn a_view_change_fades_out_goes_black_and_fades_in() {
        let mut ui = ui_at(76);
        ui.scene(ms(0));
        ui.scene(ms(600));
        let before = texts(&ui.scene(ms(990)));
        ui.input(Input::Pointer(70), ms(1000)); // ARTISTS -> ALBUMS
        let black = texts(&ui.scene(ms(1200))); // only the arc's labels
        assert!(black < before, "content is gone while black ({before} -> {black})");
        let back = texts(&ui.scene(ms(1500)));
        assert!(back > black, "content is back after the fade ({black} -> {back})");
        // nothing more until the beam dims, a second after it stopped
        assert!(ui.next_frame(ms(1500)).is_some_and(|t| t >= ms(2000)));
    }

    #[test]
    fn the_frame_ending_a_fade_is_still_owed_after_the_fade() {
        let mut ui = ui_at(76);
        ui.scene(ms(0));
        ui.scene(ms(900));
        ui.input(Input::Pointer(70), ms(1000));
        ui.scene(ms(1425)); // the last frame inside the fade
        let due = ui.next_frame(ms(1445)).expect("one more frame");
        assert!(due <= ms(1445));
        ui.scene(ms(1445));
        assert!(ui.next_frame(ms(1460)).is_some_and(|t| t >= ms(2000)), "only the beam's dimming is left");
    }

    #[test]
    fn the_pointer_picks_zones_as_the_stock_does() {
        // N.MUSIC as captured on 27 Nov 2026 (ui/INDEX.md): 0-15 and 100+ now-playing
        for (p, name) in [
            (5u8, "NOW PLAYING"),
            (34, "QUEUE"),
            (52, "FAVOURITES"),
            (62, "TITLES"),
            (70, "ALBUMS"),
            (76, "ARTISTS"),
            (81, "COVERS"),
            (90, "MODE"),
        ] {
            assert_eq!(ui_at(p).zone().name, name, "pointer {p}");
        }
        let top = ui_at(110);
        assert_eq!(top.zone_index(), top.zones.len() - 1);
        // every zone's own pointer value shows it
        for set in [n_music_zones(), settings_zones(false), settings_zones(true)] {
            let mut ui = ui_at(64);
            ui.zones = set.clone();
            for i in 0..set.len() {
                assert_eq!(ui.zone_at(zone_pointer(&set, i)), i, "{}", set[i].name);
            }
        }
    }

    #[test]
    fn the_beam_eases_and_snaps_off_the_top() {
        let mut ui = ui_at(64);
        ui.scene(ms(0));
        ui.input(Input::Pointer(94), ms(1));
        ui.input(Input::Pointer(96), ms(2)); // moving up past 94: off the top
        assert_eq!(ui.pointer, arc::TOP);
        ui.scene(ms(17));
        assert!(ui.beam < arc::angle(arc::TOP));
        for f in 2..40 {
            ui.scene(ms(17 * f));
        }
        assert_eq!(ui.beam, arc::angle(arc::TOP));
        ui.input(Input::Pointer(110), ms(690));
        ui.input(Input::Pointer(100), ms(700)); // coming back down: not snapped
        assert_eq!(ui.pointer, 100);
        assert_eq!(ui.zone().name, "MODE");
    }

    #[test]
    fn go_on_settings_swaps_the_arc() {
        let mut ui = ui_at(92); // MODE
        ui.input(Input::MenuWheel(45), ms(1)); // to SETTINGS
        ui.input(Input::Pressed(Button::Go), ms(2));
        let fx = ui.input(Input::Released(Button::Go), ms(3));
        assert_eq!(fx, vec![Effect::SettingsOpened, Effect::SelectSource("SETTINGS")]);
        ui.input(Input::Pointer(75), ms(4));
        assert_eq!(ui.zone().name, "SYSTEM SETTINGS");
        ui.input(Input::Pointer(92), ms(5));
        ui.input(Input::MenuWheel(-45), ms(6)); // back to N.MUSIC
        ui.input(Input::Pressed(Button::Go), ms(7));
        ui.input(Input::Released(Button::Go), ms(8));
        ui.input(Input::Pointer(76), ms(9));
        assert_eq!(ui.zone().name, "ARTISTS");
    }

    #[test]
    fn every_view_draws() {
        let mut ui = ui_at(64);
        for (i, p) in [0u8, 30, 50, 62, 70, 76, 82, 92].into_iter().enumerate() {
            let t = ms(10_000 * (i as u64 + 1));
            ui.input(Input::Pointer(p), t);
            assert!(!ui.scene(t + ms(1000)).items.is_empty(), "pointer {p}");
        }
    }
}
