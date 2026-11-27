//! neomaster's panel UI: the stock grammar, as captured on 27 Nov 2026. The pointer picks the
//! view, the menu wheel moves within it, LEFT drills in, RIGHT backs out, GO acts; a long GO opens
//! the context menu while held.
//!
//! [`Ui`] is plain state. [`Ui::input`] and [`Ui::tick`] change it and return [`Effect`]s for the
//! core to carry out (click the panel, play something, set the volume); [`Ui::scene`] turns it
//! into a [`render::Scene`]. Nothing here blocks or touches a device.

pub mod fake;

use std::time::Duration;

use bs5_panel::{Button, Input, COUNTS_PER_STEP};
use render::{Align, Color, Rect, Scene, SCREEN_HEIGHT, SCREEN_WIDTH};

use fake::Library;

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
    Backlight(bool),
    /// The keyboard's standby key (the panel itself has none; see the open questions).
    Standby,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    NowPlaying,
    List,
    Covers,
    Menu,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Artist(usize),
    Album(usize),
    Track(usize),
    Label(&'static str),
    Source(&'static str),
}

struct Level {
    title: String,
    items: Vec<Item>,
    cursor: usize,
    /// Where the list is drawn, easing towards `cursor`.
    scroll: f32,
}

impl Level {
    fn new(title: impl Into<String>, items: Vec<Item>) -> Level {
        Level { title: title.into(), items, cursor: 0, scroll: 0.0 }
    }
}

const CONTEXT_ENTRIES: [&str; 5] = ["RED", "GREEN", "YELLOW", "BLUE", "ADD TO QUEUE"];
const SOURCES: [&str; 4] = ["N.MUSIC", "N.RADIO", "A.AUX", "SETTINGS"];

pub struct Ui {
    lib: Library,
    zones: Vec<Zone>,
    /// One navigation stack per zone, so leaving a zone and coming back returns to the same place.
    stacks: Vec<Vec<Level>>,
    pointer: u8,
    menu_counts: i32,
    queue: Vec<usize>,
    playing: Option<usize>,
    volume: u8,
    volume_shown_until: Time,
    /// Long GO: the context menu, open while GO is held.
    context: Option<usize>,
    go_long: bool,
    last_input: Time,
    last_frame: Option<Time>,
    /// After `idle_to_now_playing` without input the view returns to now-playing until the
    /// pointer moves.
    idle_now_playing: bool,
    blanked: bool,
    pub idle_to_now_playing: Duration,
    /// Blank-screen standby (stock: 4 min 52 s measured).
    pub idle_to_blank: Duration,
}

pub const MAX_VOLUME: u8 = 90;
const ROW: f32 = 54.0;
const CENTRE_Y: f32 = SCREEN_HEIGHT as f32 / 2.0;
const TEXT: Color = Color::rgb(235, 235, 235);
const DIM: Color = Color::rgb(150, 150, 150);
const ACCENT: Color = Color::rgb(70, 140, 230);

impl Ui {
    pub fn new(lib: Library) -> Ui {
        let zones = n_music_zones();
        let mut ui = Ui {
            lib,
            stacks: Vec::new(),
            zones,
            pointer: 64,
            menu_counts: 0,
            queue: Vec::new(),
            playing: None,
            volume: 34, // stock default
            volume_shown_until: Time::ZERO,
            context: None,
            go_long: false,
            last_input: Time::ZERO,
            last_frame: None,
            idle_now_playing: false,
            blanked: false,
            idle_to_now_playing: Duration::from_secs(180),
            idle_to_blank: Duration::from_secs(292),
        };
        ui.stacks = (0..ui.zones.len()).map(|i| vec![ui.root_level(ui.zones[i].root)]).collect();
        ui
    }

    fn root_level(&self, root: Root) -> Level {
        let lib = &self.lib;
        match root {
            Root::None => Level::new("", vec![]),
            Root::Queue => Level::new("QUEUE", self.queue.iter().map(|t| Item::Track(*t)).collect()),
            Root::Favourites => Level::new(
                "FAVOURITES",
                ["RED", "GREEN", "YELLOW", "BLUE", "RECENTLY ADDED"].into_iter().map(Item::Label).collect(),
            ),
            Root::Titles => {
                let mut t: Vec<usize> = (0..lib.tracks.len()).collect();
                t.sort_by_cached_key(|i| lib.tracks[*i].title.to_lowercase());
                Level::new("TITLES", t.into_iter().map(Item::Track).collect())
            }
            Root::Albums | Root::Covers => {
                let mut a: Vec<usize> = (0..lib.albums.len()).collect();
                a.sort_by_cached_key(|i| lib.albums[*i].title.to_lowercase());
                Level::new(if root == Root::Albums { "ALBUMS" } else { "COVERS" }, a.into_iter().map(Item::Album).collect())
            }
            Root::Artists => Level::new("ARTISTS", (0..lib.artists.len()).map(Item::Artist).collect()),
            Root::Mode => Level::new("MODE", SOURCES.into_iter().map(Item::Source).collect()),
        }
    }

    fn zone_index(&self) -> usize {
        if self.idle_now_playing {
            return 0;
        }
        self.zones.iter().position(|z| (z.lo..=z.hi).contains(&self.pointer)).unwrap_or(0)
    }

    /// The screen is blank (idle or standby).
    pub fn blanked(&self) -> bool {
        self.blanked
    }

    pub fn zone(&self) -> &Zone {
        &self.zones[self.zone_index()]
    }

    fn level_mut(&mut self) -> Option<&mut Level> {
        let z = self.zone_index();
        if self.zones[z].kind == ViewKind::NowPlaying {
            return None;
        }
        self.stacks[z].last_mut()
    }

    fn level(&self) -> Option<&Level> {
        let z = self.zone_index();
        if self.zones[z].kind == ViewKind::NowPlaying {
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

    fn drill(&self, item: Item) -> Option<Level> {
        match item {
            Item::Artist(a) => Some(Level::new(
                self.lib.artists[a].name.clone(),
                self.lib.artists[a].albums.iter().map(|x| Item::Album(*x)).collect(),
            )),
            Item::Album(a) => Some(Level::new(
                self.lib.albums[a].title.clone(),
                self.lib.albums[a].tracks.iter().map(|x| Item::Track(*x)).collect(),
            )),
            Item::Label(l) => Some(Level::new(l, vec![])),
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

    /// Feed one panel input.
    pub fn input(&mut self, input: Input, now: Time) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.last_input = now;
        if self.blanked {
            // Any input while blanked turns the backlight on and is otherwise swallowed, so the
            // first touch does not also scroll.
            self.blanked = false;
            fx.push(Effect::Backlight(true));
            if let Input::Pointer(p) = input {
                self.pointer = p;
            }
            return fx;
        }
        match input {
            Input::Pointer(p) => {
                let before = self.zone_index();
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
                }
            }
            Input::VolumeWheel(counts) => {
                let v = (self.volume as i32 + counts).clamp(0, MAX_VOLUME as i32) as u8;
                self.volume_shown_until = now + Duration::from_millis(1500);
                if v != self.volume {
                    self.volume = v;
                    fx.push(Effect::Volume(v));
                }
            }
            Input::Pressed(Button::Go) => self.go_long = false,
            Input::LongPress(Button::Go) => {
                if matches!(self.current_item(), Some(Item::Track(_) | Item::Album(_) | Item::Artist(_))) {
                    self.go_long = true;
                    self.context = Some(0);
                    fx.push(Effect::Click);
                }
            }
            Input::Released(Button::Go) => {
                if let Some(entry) = self.context.take() {
                    if CONTEXT_ENTRIES[entry] == "ADD TO QUEUE" {
                        if let Some(item) = self.current_item() {
                            let t = self.tracks_of(item);
                            self.queue.extend(&t);
                            self.refresh_queue();
                            fx.push(Effect::Enqueue(t));
                        }
                    }
                } else if !self.go_long {
                    fx.extend(self.go());
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
        fx
    }

    /// The keyboard's standby key in `--sim`.
    pub fn standby(&mut self, now: Time) -> Vec<Effect> {
        self.last_input = now;
        vec![Effect::Standby]
    }

    fn step(&mut self, steps: i32) -> Vec<Effect> {
        if let Some(c) = self.context.as_mut() {
            *c = (*c as i32 + steps).clamp(0, CONTEXT_ENTRIES.len() as i32 - 1) as usize;
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

    fn go(&mut self) -> Vec<Effect> {
        match self.current_item() {
            Some(Item::Source(s)) => vec![Effect::SelectSource(s)],
            Some(item @ (Item::Track(_) | Item::Album(_) | Item::Artist(_))) => {
                let t = self.tracks_of(item);
                self.playing = t.first().copied();
                vec![Effect::Play(t)]
            }
            _ => vec![],
        }
    }

    /// Timers: return to now-playing, blank the screen.
    pub fn tick(&mut self, now: Time) -> Vec<Effect> {
        let idle = now.saturating_sub(self.last_input);
        if !self.idle_now_playing && idle >= self.idle_to_now_playing && self.context.is_none() {
            self.idle_now_playing = true;
        }
        if !self.blanked && idle >= self.idle_to_blank {
            self.blanked = true;
            return vec![Effect::Backlight(false)];
        }
        vec![]
    }

    /// When the next frame is due: in one frame while something moves, at the end of the volume
    /// overlay, or never.
    pub fn next_frame(&self, now: Time) -> Option<Time> {
        if let Some(l) = self.level() {
            if (l.scroll - l.cursor as f32).abs() > 0.002 {
                return Some(now + Duration::from_micros(16_667));
            }
        }
        (self.volume_shown_until > now).then_some(self.volume_shown_until)
    }

    /// The next timer [`Ui::tick`] needs, if any.
    pub fn next_timer(&self) -> Option<Time> {
        if self.blanked {
            None
        } else if self.idle_now_playing {
            Some(self.last_input + self.idle_to_blank)
        } else {
            Some(self.last_input + self.idle_to_now_playing.min(self.idle_to_blank))
        }
    }

    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// The volume as the PC2 reports it (or a neutral value where the room's master owns it).
    /// Updates what the wheel turns from; emits nothing.
    pub fn set_volume(&mut self, volume: u8) {
        self.volume = volume.min(MAX_VOLUME);
    }

    /// Blank-screen standby now (the room went to standby): the backlight goes off until the
    /// next input, as after the idle timeout.
    pub fn blank(&mut self) -> Vec<Effect> {
        if self.blanked {
            return vec![];
        }
        self.blanked = true;
        vec![Effect::Backlight(false)]
    }

    fn animate(&mut self, now: Time) {
        let dt = self.last_frame.map(|t| now.saturating_sub(t).as_secs_f32()).unwrap_or(1.0).min(0.25);
        self.last_frame = Some(now);
        let k = 1.0 - (-dt / 0.045).exp();
        if let Some(l) = self.level_mut() {
            l.scroll += (l.cursor as f32 - l.scroll) * k;
            if (l.scroll - l.cursor as f32).abs() <= 0.002 {
                l.scroll = l.cursor as f32;
            }
        }
    }

    /// Build the frame for `now`.
    pub fn scene(&mut self, now: Time) -> Scene {
        let mut s = Scene::new(Color::BLACK);
        if self.blanked {
            return s;
        }
        self.animate(now);
        match self.zone().kind {
            ViewKind::NowPlaying => self.draw_now_playing(&mut s),
            ViewKind::List | ViewKind::Menu => self.draw_list(&mut s),
            ViewKind::Covers => self.draw_covers(&mut s),
        }
        self.draw_arc(&mut s);
        if let Some(c) = self.context {
            self.draw_context(&mut s, c);
        }
        if now < self.volume_shown_until {
            self.draw_volume(&mut s);
        }
        s
    }

    fn pointer_y(p: f32) -> f32 {
        64.0 + p * (640.0 / 127.0)
    }

    fn arc_x(y: f32) -> f32 {
        let (cx, cy, r) = (1400.0f32, CENTRE_Y, 520.0f32);
        let dy = (y - cy).clamp(-r + 1.0, r - 1.0);
        cx - (r * r - dy * dy).sqrt()
    }

    fn draw_arc(&self, s: &mut Scene) {
        let current = self.zone_index();
        for (i, z) in self.zones.iter().enumerate() {
            if i == self.zones.len() - 1 {
                continue; // the second now-playing zone shares the label
            }
            let centre = (z.lo as f32 + z.hi as f32) / 2.0;
            let y = Self::pointer_y(centre);
            let on = i == current || (current == self.zones.len() - 1 && i == 0);
            s.text(
                Self::arc_x(y) as i32 - 28,
                y as i32 + 8,
                if on { 24 } else { 20 },
                if on { TEXT } else { DIM.with_opacity(0.7) },
                Align::Right,
                z.name,
            );
        }
        let y = Self::pointer_y(self.pointer as f32);
        let x = Self::arc_x(y);
        s.fill(Rect::new(x as i32 - 14, y as i32 - 3, 22, 6), ACCENT);
    }

    fn draw_list(&self, s: &mut Scene) {
        let Some(level) = self.level() else { return };
        let z = self.zone_index();
        let depth = self.stacks[z].len();
        let crumb = if depth > 1 { format!("{}  ›  {}", self.zones[z].name, level.title) } else { level.title.clone() };
        s.text(60, 70, 22, DIM, Align::Left, crumb);
        if level.items.is_empty() {
            s.text(400, CENTRE_Y as i32, 28, DIM, Align::Center, "Empty");
            return;
        }
        let first = (level.scroll - 7.0).floor().max(0.0) as usize;
        let last = ((level.scroll + 7.0).ceil() as usize).min(level.items.len() - 1);
        for i in first..=last {
            let d = i as f32 - level.scroll;
            let y = CENTRE_Y + d * ROW;
            let near = 1.0 - d.abs().min(1.0);
            let size = (26.0 + 10.0 * near) as u16;
            let alpha = (1.0 - d.abs() / 7.5).clamp(0.0, 1.0);
            let item = level.items[i];
            if let (Item::Track(t), Some(p)) = (item, self.playing) {
                if t == p {
                    s.fill(Rect::new(60, (y - 22.0) as i32, 6, 34), ACCENT.with_opacity(alpha));
                }
            }
            let colour = if i == level.cursor { TEXT } else { DIM };
            s.text(90, y as i32 + size as i32 / 3, size, colour.with_opacity(alpha), Align::Left, self.label(item));
            if let Item::Track(t) = item {
                if depth == 1 && i == level.cursor {
                    let tr = &self.lib.tracks[t];
                    s.text(90, y as i32 + 34, 18, DIM.with_opacity(alpha), Align::Left, self.lib.artists[tr.artist].name.clone());
                }
            }
        }
    }

    fn draw_covers(&self, s: &mut Scene) {
        let Some(level) = self.level() else { return };
        let first = (level.scroll - 6.0).floor().max(0.0) as usize;
        let last = ((level.scroll + 6.0).ceil() as usize).min(level.items.len().saturating_sub(1));
        for i in first..=last {
            let d = i as f32 - level.scroll;
            let size = (96.0 - d.abs().min(5.0) * 8.0).max(56.0);
            let y = CENTRE_Y + d * 104.0;
            let x = 560.0 - d * d * 9.0;
            let Item::Album(a) = level.items[i] else { continue };
            let (r, g, b) = self.lib.albums[a].colour;
            let alpha = (1.0 - d.abs() / 6.5).clamp(0.0, 1.0);
            s.fill(
                Rect::new((x - size / 2.0) as i32, (y - size / 2.0) as i32, size as u32, size as u32),
                Color::rgb(r, g, b).with_opacity(alpha),
            );
        }
        if let Some(Item::Album(a)) = level.items.get(level.cursor) {
            let al = &self.lib.albums[*a];
            s.text(60, CENTRE_Y as i32 - 6, 32, TEXT, Align::Left, al.title.clone());
            s.text(60, CENTRE_Y as i32 + 30, 22, DIM, Align::Left, self.lib.artists[al.artist].name.clone());
        }
    }

    fn draw_now_playing(&self, s: &mut Scene) {
        let Some(t) = self.playing else {
            s.text(420, CENTRE_Y as i32, 30, DIM, Align::Center, "Nothing playing");
            return;
        };
        let tr = &self.lib.tracks[t];
        let al = &self.lib.albums[tr.album];
        let (r, g, b) = al.colour;
        s.fill(Rect::new(90, 214, 340, 340), Color::rgb(r, g, b));
        s.text(470, 350, 38, TEXT, Align::Left, tr.title.clone());
        s.text(470, 398, 28, DIM, Align::Left, self.lib.artists[tr.artist].name.clone());
        s.text(470, 438, 24, DIM.with_opacity(0.7), Align::Left, al.title.clone());
    }

    fn draw_context(&self, s: &mut Scene, cursor: usize) {
        s.fill(Rect::new(0, 0, SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32), Color::rgba(0, 0, 0, 170));
        for (i, e) in CONTEXT_ENTRIES.iter().enumerate() {
            let y = 250 + i as i32 * 60;
            if i == cursor {
                s.fill(Rect::new(300, y - 36, 420, 50), ACCENT.with_opacity(0.6));
            }
            s.text(512, y, 30, TEXT, Align::Center, *e);
        }
    }

    fn draw_volume(&self, s: &mut Scene) {
        let w = 400u32;
        let x = (SCREEN_WIDTH as u32 - w) / 2;
        s.fill(Rect::new(x as i32, 690, w, 8), Color::rgb(60, 60, 60));
        s.fill(Rect::new(x as i32, 690, w * self.volume as u32 / MAX_VOLUME as u32, 8), ACCENT);
        s.text(512, 676, 22, TEXT, Align::Center, format!("VOLUME {}", self.volume));
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
        assert_eq!(ui.input(Input::MenuWheel(10), ms(2)), vec![Effect::Click]); // 20 counts = 1 step
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
        ui.input(Input::LongPress(Button::Go), ms(900));
        ui.input(Input::MenuWheel(60), ms(950)); // to ADD TO QUEUE
        let fx = ui.input(Input::Released(Button::Go), ms(1000));
        assert!(matches!(fx[..], [Effect::Enqueue(_)]));
        ui.input(Input::Pointer(30), ms(1100));
        assert_eq!(ui.zone().name, "QUEUE");
        assert_eq!(ui.level().unwrap().items.len(), 1);
    }

    #[test]
    fn volume_is_clamped_and_reported() {
        let mut ui = ui_at(0);
        assert_eq!(ui.input(Input::VolumeWheel(3), ms(1)), vec![Effect::Volume(37)]);
        ui.input(Input::VolumeWheel(200), ms(2));
        assert_eq!(ui.volume(), MAX_VOLUME);
    }

    #[test]
    fn blanks_when_idle_and_swallows_the_waking_input() {
        let mut ui = ui_at(76);
        assert_eq!(ui.tick(ms(292_000)), vec![Effect::Backlight(false)]);
        assert!(ui.scene(ms(292_001)).items.is_empty());
        assert_eq!(ui.input(Input::MenuWheel(15), ms(300_000)), vec![Effect::Backlight(true)]);
        assert_eq!(ui.zone().kind, ViewKind::NowPlaying); // idle returned to now-playing
        ui.input(Input::Pointer(77), ms(300_100));
        assert_eq!(ui.level().unwrap().cursor, 0); // the waking wheel turn did not scroll
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
}
