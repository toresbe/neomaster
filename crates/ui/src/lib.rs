//! neomaster's panel UI: the stock grammar, as captured on 27 Nov 2026. The pointer picks the
//! view, the menu wheel moves within it, LEFT drills in, RIGHT backs out, GO acts; a long GO opens
//! the context menu while held.
//!
//! [`Ui`] is plain state. [`Ui::input`] and [`Ui::tick`] change it and return [`Effect`]s for the
//! core to carry out (click the panel, play something, set the volume); [`Ui::scene`] turns it
//! into a [`render::Scene`] (the layout is in `draw.rs`). Nothing here blocks or touches a device.

pub mod assets;
mod draw;
pub mod fake;
pub mod idle;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bs5_panel::{Button, Input, COUNTS_PER_STEP};
use render::{Color, Image, Scene};

pub use assets::Assets;
use fake::Library;
pub use idle::IdleConfig;
use idle::{Idle, IdleAction};

/// A monotonic timestamp, as a duration since an arbitrary fixed epoch.
pub type Time = Duration;

/// What the UI asks the rest of neomaster to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// One panel click (a list row, or the pointer crossing a zone).
    Click,
    /// Play these tracks, the first one now (fake library track indices until M3).
    Play(Vec<usize>),
    /// Append to the play queue.
    Enqueue(Vec<usize>),
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

/// A pointer zone: positions `lo..=hi` show `view`. The boundaries are guesses between the
/// captured centres and belong in config, to be tuned on the panel.
#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub name: &'static str,
    pub lo: u8,
    pub hi: u8,
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
    Settings(&'static [&'static str]),
    SystemInfo,
    NetworkInfo,
}

/// N.MUSIC as captured: now-playing at both ends, the lists in between.
pub fn n_music_zones() -> Vec<Zone> {
    use ViewKind::*;
    let z = |name, lo, hi, kind, root| Zone { name, lo, hi, kind, root };
    vec![
        z("NOW PLAYING", 0, 15, NowPlaying, Root::None),
        z("QUEUE", 16, 42, List, Root::Queue),
        z("FAVOURITES", 43, 56, List, Root::Favourites),
        z("TITLES", 57, 65, List, Root::Titles),
        z("ALBUMS", 66, 72, List, Root::Albums),
        z("ARTISTS", 73, 78, List, Root::Artists),
        z("COVERS", 79, 85, Covers, Root::Covers),
        z("MODE", 86, 99, Menu, Root::Mode),
        z("NOW PLAYING", 100, 127, NowPlaying, Root::None),
    ]
}

/// SETTINGS, as captured (the normal menu, not the hidden one). The zone boundaries are not
/// measured: they are the midpoints between the labels' places on the arc, mapped back to pointer
/// positions with the slope measured in N.MUSIC (0.83 degrees per step).
pub fn settings_zones() -> Vec<Zone> {
    use ViewKind::*;
    let z = |name, lo, hi, kind, root| Zone { name, lo, hi, kind, root };
    vec![
        z("NOW PLAYING", 0, 15, NowPlaying, Root::None),
        z("ROLES", 16, 38, Menu, Root::Settings(&["AUDIO MASTER", "AM OPTION"])),
        z("NETWORK INFO", 39, 46, Info, Root::NetworkInfo),
        z("SERVICE SETTINGS", 47, 53, Menu, Root::Settings(&["N.RADIO BUFFER", "DISABLE MOTS"])),
        z("MUSIC SERVICES", 54, 62, Menu, Root::Settings(&["SPOTIFY"])),
        z(
            "MAINTENANCE",
            63,
            71,
            Menu,
            Root::Settings(&["UPDATE SOFTWARE", "AUTOMATIC SOFTWARE UPDATE", "LATEST RELEASE INFORMATION", "ABOUT"]),
        ),
        z("SYSTEM SETTINGS", 72, 79, Menu, Root::Settings(&["LANGUAGE", "TIME ZONE", "TIME"])),
        z("SYSTEM INFO", 80, 87, Info, Root::SystemInfo),
        z("MODE", 88, 99, Menu, Root::Mode),
        z("NOW PLAYING", 100, 127, NowPlaying, Root::None),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Artist(usize),
    Album(usize),
    Track(usize),
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
}

impl Level {
    fn new(items: Vec<Item>) -> Level {
        Level { items, cursor: 0, scroll: 0.0, alphabetical: false }
    }
}

/// The long-GO menu: four colour favourites, ADD TO QUEUE (where it opens), then playlists.
const CONTEXT_ENTRIES: [&str; 10] = ["RED", "GREEN", "YELLOW", "BLUE", "ADD TO  QUEUE", "01", "02", "03", "04", "05"];
const CONTEXT_DEFAULT: usize = 4;
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
    /// Generated art for the fake library's albums.
    art: HashMap<usize, Arc<Image>>,
    zones: Vec<Zone>,
    /// One navigation stack per zone, so leaving a zone and coming back returns to the same place.
    stacks: Vec<Vec<Level>>,
    pointer: u8,
    /// The pointer as drawn, easing towards `pointer`.
    pointer_drawn: f32,
    menu_counts: i32,
    queue: Vec<usize>,
    playing: Option<usize>,
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
            pointer_drawn: 64.0,
            menu_counts: 0,
            queue: Vec::new(),
            playing: None,
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
            Root::None | Root::SystemInfo | Root::NetworkInfo => Level::new(vec![]),
            Root::Queue => Level::new(self.queue.iter().map(|t| Item::Track(*t)).collect()),
            Root::Favourites => {
                Level::new(["RED", "GREEN", "YELLOW", "BLUE", "RECENTLY ADDED"].into_iter().map(Item::Label).collect())
            }
            Root::Titles => {
                let mut t: Vec<usize> = (0..lib.tracks.len()).collect();
                t.sort_by_cached_key(|i| lib.tracks[*i].title.to_lowercase());
                sorted(Level::new(t.into_iter().map(Item::Track).collect()))
            }
            Root::Albums | Root::Covers => {
                let mut a: Vec<usize> = (0..lib.albums.len()).collect();
                a.sort_by_cached_key(|i| lib.albums[*i].title.to_lowercase());
                let l = Level::new(a.into_iter().map(Item::Album).collect());
                if root == Root::Albums {
                    sorted(l)
                } else {
                    l
                }
            }
            Root::Artists => sorted(Level::new((0..lib.artists.len()).map(Item::Artist).collect())),
            Root::Mode => Level::new(SOURCES.into_iter().map(Item::Source).collect()),
            Root::Settings(items) => Level::new(items.iter().copied().map(Item::Label).collect()),
        }
    }

    fn zone_index(&self) -> usize {
        if self.idle_now_playing {
            return 0;
        }
        self.zones.iter().position(|z| (z.lo..=z.hi).contains(&self.pointer)).unwrap_or(0)
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
            Item::Label(s) | Item::Source(s) => s.to_string(),
        }
    }

    fn tracks_of(&self, item: Item) -> Vec<usize> {
        match item {
            Item::Track(t) => vec![t],
            Item::Album(a) => self.lib.albums[a].tracks.clone(),
            Item::Artist(a) => self.lib.artists[a].albums.iter().flat_map(|al| self.lib.albums[*al].tracks.clone()).collect(),
            _ => vec![],
        }
    }

    fn in_settings(&self) -> bool {
        self.zones.iter().any(|z| matches!(z.root, Root::Settings(_)))
    }

    fn drill(&self, item: Item) -> Option<Level> {
        match item {
            Item::Artist(a) => Some(Level::new(self.lib.artists[a].albums.iter().map(|x| Item::Album(*x)).collect())),
            Item::Album(a) => Some(Level::new(self.lib.albums[a].tracks.iter().map(|x| Item::Track(*x)).collect())),
            // settings pages are not built yet
            Item::Label(_) if !self.in_settings() => Some(Level::new(vec![])),
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
        let source_active = self.playing.is_some() || self.idle.playing();
        if self.idle.input(now, source_active) {
            // Any input while blanked turns the backlight on and is otherwise swallowed, so the
            // first touch does not also scroll.
            fx.push(Effect::Backlight(true));
            if let Input::Pointer(p) = input {
                self.pointer = p;
                self.pointer_drawn = p as f32;
            }
            return fx;
        }
        match input {
            Input::Pointer(p) => {
                self.idle_now_playing = false;
                self.pointer = p;
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
            Input::LongPress(Button::Go) => {
                if matches!(self.current_item(), Some(Item::Track(_) | Item::Album(_) | Item::Artist(_))) {
                    self.go_long = true;
                    self.context = Some(CONTEXT_DEFAULT);
                    self.context_scroll = CONTEXT_DEFAULT as f32;
                    fx.push(Effect::Click);
                }
            }
            Input::Released(Button::Go) => {
                if let Some(entry) = self.context.take() {
                    if entry == CONTEXT_DEFAULT {
                        if let Some(item) = self.current_item() {
                            let t = self.tracks_of(item);
                            self.queue.extend(&t);
                            self.refresh_queue();
                            fx.push(Effect::Enqueue(t));
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
        let source_active = self.playing.is_some() || self.idle.playing();
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

    fn go(&mut self, now: Time) -> Vec<Effect> {
        match self.current_item() {
            Some(Item::Source(s)) => {
                // SETTINGS swaps the arc's zones for the settings pages; any real source swaps
                // them back.
                let settings = s == "SETTINGS";
                if settings != self.in_settings() {
                    self.zones = if settings { settings_zones() } else { n_music_zones() };
                    self.reset_stacks();
                    // MODE keeps its cursor where it was
                    if let Some(z) = self.zones.iter().position(|z| z.root == Root::Mode) {
                        let i = SOURCES.iter().position(|x| *x == s).unwrap_or(0);
                        self.stacks[z][0].cursor = i;
                        self.stacks[z][0].scroll = i as f32;
                    }
                    self.switch = Some(Switch { from: None, start: now });
                }
                vec![Effect::SelectSource(s)]
            }
            Some(item @ (Item::Track(_) | Item::Album(_) | Item::Artist(_))) => {
                let t = self.tracks_of(item);
                self.playing = t.first().copied();
                // The fake library plays at once; with Music Assistant the core reports playback.
                self.idle.set_playing(true);
                vec![Effect::Play(t)]
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
        let soon = Some(self.last_frame.unwrap_or(now) + Duration::from_micros(16_667));
        // Judged by the last frame drawn, not by `now`: the frame that shows the end of a fade
        // is still owed when the caller asks after the fade's end.
        let drawn = self.last_frame.unwrap_or(Time::ZERO);
        let moving = self.level().is_some_and(|l| (l.scroll - l.cursor as f32).abs() > 0.002)
            || self.context.is_some_and(|c| (self.context_scroll - c as f32).abs() > 0.002)
            || self.switch.is_some()
            || (self.pointer_drawn - self.pointer as f32).abs() > 0.01;
        if moving {
            return soon;
        }
        let volume_end = self.volume_shown_until + VOLUME_FADE;
        if self.volume_shown_until > Time::ZERO && drawn < volume_end {
            return if now < self.volume_shown_until { Some(self.volume_shown_until) } else { soon };
        }
        let letter_end = self.last_step + LETTER_HOLD;
        (drawn < letter_end && self.level().is_some_and(|l| l.alphabetical)).then_some(letter_end)
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
        let kp = 1.0 - (-dt / 0.03).exp();
        self.pointer_drawn += (self.pointer as f32 - self.pointer_drawn) * kp;
        if (self.pointer_drawn - self.pointer as f32).abs() <= 0.01 {
            self.pointer_drawn = self.pointer as f32;
        }
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

    /// The fake library's art for `album`: a generated gradient sleeve, made once.
    fn album_art(&mut self, album: usize) -> Arc<Image> {
        let (r, g, b) = self.lib.albums[album].colour;
        self.art
            .entry(album)
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
        assert!(matches!(fx[..], [Effect::Enqueue(_)]));
        ui.input(Input::Pointer(30), ms(1100));
        assert_eq!(ui.zone().name, "QUEUE");
        assert_eq!(ui.level().unwrap().items.len(), 1);
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
        ui.scene(ms(0));
        ui.input(Input::MenuWheel(75), ms(1));
        assert!(ui.next_frame(ms(1)).is_some());
        for f in 0..60 {
            ui.scene(ms(17 * f));
        }
        assert_eq!(ui.next_frame(ms(1100)), None);
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
        assert!(ui.next_frame(ms(1500)).is_none());
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
        assert_eq!(ui.next_frame(ms(1460)), None);
    }

    #[test]
    fn go_on_settings_swaps_the_arc() {
        let mut ui = ui_at(92); // MODE
        ui.input(Input::MenuWheel(45), ms(1)); // to SETTINGS
        ui.input(Input::Pressed(Button::Go), ms(2));
        let fx = ui.input(Input::Released(Button::Go), ms(3));
        assert_eq!(fx, vec![Effect::SelectSource("SETTINGS")]);
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
