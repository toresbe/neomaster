//! The SETTINGS "source", as captured on the BM5 (27 Nov 2026, `work/bm5-ui/shots/0090`-`0199`).
//!
//! GO on SETTINGS in the MODE list swaps the arc's zones for the settings sections
//! ([`crate::settings_zones`]); RIGHT LEFT RIGHT LEFT and then GO on SETTINGS opens the hidden
//! menu instead. The arc, the pointer, MODE and the fades are the normal UI's. This module adds
//! what a section shows and how it is navigated:
//!
//! - Info sections are a two-column table. Other sections list their items on the list curve;
//!   a highlighted read-only item shows its value on the left (stock TIME).
//! - LEFT opens a choice's picker: the item's name on the left, the current value, and the
//!   choices on the list curve with the selected one in the middle. The wheel moves; GO (or LEFT
//!   again) keeps the choice, RIGHT leaves the picker without changing anything. LEFT on a
//!   submenu item goes in, RIGHT comes back.
//!
//! The content is [`settings::menu`]; a choice that changes something comes out as
//! [`Effect::SettingsChanged`] for the core to save and apply.

use std::collections::HashMap;

use bs5_panel::{Button, Input, COUNTS_PER_STEP};
use render::{Align, Color, Rect, Scene};
use settings::menu::{self, Body, Facts, Item, ItemKind, SectionId};
use settings::Settings;

use crate::{Effect, Item as UiItem, Root, Time, Ui, ViewKind};

/// How long the RIGHT LEFT RIGHT LEFT combo may take, ending with GO.
const COMBO_WINDOW: Time = Time::from_secs(4);
const COMBO: [Button; 4] = [Button::Right, Button::Left, Button::Right, Button::Left];

const WHITE: Color = Color::rgb(255, 255, 255);
/// Items that need the unit to be audio master are greyed out, as captured (AM OPTION).
const ITEM_GREY: Color = Color::rgb(153, 153, 153);
const CY: f32 = 384.0;
/// The picker's item name and current value, and a highlighted item's value (stock positions).
const PICK_LABEL_X: i32 = 314;
const PICK_VALUE_RIGHT: i32 = 632;
const VALUE_X: i32 = 340;

#[derive(Default)]
pub struct SettingsMenu {
    settings: Settings,
    facts: Facts,
    hidden: bool,
    /// Per section, the cursor at each level (more than one entry inside a submenu).
    cursors: HashMap<SectionId, Vec<usize>>,
    /// The open picker: its section and highlighted choice.
    picker: Option<(SectionId, usize)>,
    /// RIGHT / LEFT presses with SETTINGS highlighted in the MODE list.
    combo: Vec<(Button, Time)>,
}

impl SettingsMenu {
    fn section_items(&self, id: SectionId) -> Option<Vec<Item>> {
        match menu::section(id, &self.settings, &self.facts, self.hidden).body {
            Body::Items(v) => Some(v),
            Body::Info(_) => None,
        }
    }

    /// The items at the current level of a section, and the cursor into them.
    fn level(&self, id: SectionId) -> Option<(Vec<Item>, usize)> {
        let mut items = self.section_items(id)?;
        let cursors = self.cursors.get(&id).cloned().unwrap_or_else(|| vec![0]);
        for &c in &cursors[..cursors.len() - 1] {
            match items.get(c).map(|i| i.kind.clone()) {
                Some(ItemKind::Submenu(sub)) => items = sub,
                _ => break,
            }
        }
        let c = (*cursors.last().unwrap_or(&0)).min(items.len().saturating_sub(1));
        Some((items, c))
    }

    fn cursors_mut(&mut self, id: SectionId) -> &mut Vec<usize> {
        self.cursors.entry(id).or_insert_with(|| vec![0])
    }
}

impl Ui {
    /// The settings as they are now (the core's copy), and what the menu should show.
    pub fn set_settings(&mut self, settings: Settings) {
        self.settings_menu.settings = settings;
        self.refresh_sources();
    }

    pub fn settings(&self) -> &Settings {
        &self.settings_menu.settings
    }

    /// Machine facts for the info sections.
    pub fn set_facts(&mut self, facts: Facts) {
        self.settings_menu.facts = facts;
        self.refresh_sources();
    }

    /// The SETTINGS arc is up.
    pub fn settings_open(&self) -> bool {
        self.in_settings()
    }

    /// The MODE list follows the source names (A.MEM, RADIO, ...) and the CD drive.
    fn refresh_sources(&mut self) {
        let m = &self.settings_menu;
        let mut v: Vec<&'static str> = m.settings.roles.source_labels(m.facts.cd_drive).into_iter().map(|(l, _)| l).collect();
        v.extend(["A.AUX", "SETTINGS"]);
        if v == self.sources {
            return;
        }
        self.sources = v;
        if let Some(z) = self.zones.iter().position(|z| z.root == Root::Mode) {
            let mut level = self.root_level(Root::Mode);
            level.cursor = self.stacks[z][0].cursor.min(level.items.len() - 1);
            level.scroll = level.cursor as f32;
            self.stacks[z] = vec![level];
        }
    }

    fn settings_highlighted_in_mode(&self) -> bool {
        self.zone().kind == ViewKind::Menu
            && self.zone().root == Root::Mode
            && self.current_item() == Some(UiItem::Source("SETTINGS"))
    }

    /// GO on SETTINGS: was it the end of RIGHT LEFT RIGHT LEFT (the hidden menu)?
    pub(crate) fn take_settings_combo(&mut self, now: Time) -> bool {
        let combo = std::mem::take(&mut self.settings_menu.combo);
        combo.len() >= COMBO.len() && {
            let tail = &combo[combo.len() - COMBO.len()..];
            tail.iter().map(|(b, _)| *b).eq(COMBO) && now.saturating_sub(tail[0].1) <= COMBO_WINDOW
        }
    }

    /// The settings arc just came up.
    pub(crate) fn settings_opened(&mut self, hidden: bool) -> Vec<Effect> {
        let m = &mut self.settings_menu;
        m.hidden = hidden;
        m.cursors.clear();
        m.picker = None;
        vec![Effect::SettingsOpened]
    }

    fn settings_section(&self) -> Option<SectionId> {
        if self.idle_now_playing {
            return None;
        }
        match self.zone().root {
            Root::Settings(id) => Some(id),
            _ => None,
        }
    }

    /// Panel input in a settings section, and the RIGHT LEFT RIGHT LEFT combo in the MODE list.
    /// `None` lets the normal UI handle the input (pointer, volume, MODE, now-playing).
    pub(crate) fn settings_input(&mut self, input: Input, now: Time) -> Option<Vec<Effect>> {
        if !self.in_settings() {
            match input {
                Input::Pressed(b @ (Button::Left | Button::Right)) if self.settings_highlighted_in_mode() => {
                    let combo = &mut self.settings_menu.combo;
                    combo.push((b, now));
                    if combo.len() > COMBO.len() {
                        combo.remove(0);
                    }
                }
                Input::Pointer(_) | Input::MenuWheel(_) => self.settings_menu.combo.clear(),
                _ => {}
            }
            return None;
        }
        if let Input::Pointer(p) = input {
            // Leaving a section closes its picker; the normal UI moves the pointer and fades.
            let after = if self.idle_now_playing { 0 } else { self.zone_at(self.snapped(p)) };
            if after != self.zone_index() {
                self.settings_menu.picker = None;
            }
            return None;
        }
        let id = self.settings_section()?;
        let mut fx = Vec::new();
        match input {
            Input::VolumeWheel(_) => return None,
            Input::MenuWheel(counts) => {
                self.menu_counts += counts;
                let steps = self.menu_counts / COUNTS_PER_STEP;
                self.menu_counts -= steps * COUNTS_PER_STEP;
                if steps != 0 && self.settings_step(id, steps) {
                    self.last_step = now;
                    fx.push(Effect::Click);
                }
            }
            Input::Pressed(Button::Left) => fx.extend(self.settings_left(id)),
            Input::Pressed(Button::Right) => {
                if self.settings_menu.picker.take().is_some() {
                    fx.push(Effect::Click);
                } else {
                    let c = self.settings_menu.cursors_mut(id);
                    if c.len() > 1 {
                        c.pop();
                        fx.push(Effect::Click);
                    }
                }
            }
            Input::Released(Button::Go) => fx.extend(self.settings_commit(id)),
            _ => {}
        }
        Some(fx)
    }

    /// Turn the wheel in the picker or the item list. True if something moved.
    fn settings_step(&mut self, id: SectionId, steps: i32) -> bool {
        let move_to = |cur: usize, len: usize| (cur as i32 + steps).clamp(0, len.saturating_sub(1) as i32) as usize;
        let Some((items, cur)) = self.settings_menu.level(id) else { return false };
        if let Some((_, p)) = self.settings_menu.picker {
            let Some(ItemKind::Choice { options, .. }) = items.get(cur).map(|i| &i.kind) else { return false };
            let new = move_to(p, options.len());
            self.settings_menu.picker = Some((id, new));
            return new != p;
        }
        let new = move_to(cur, items.len());
        *self.settings_menu.cursors_mut(id).last_mut().unwrap() = new;
        new != cur
    }

    fn settings_left(&mut self, id: SectionId) -> Vec<Effect> {
        if self.settings_menu.picker.is_some() {
            return self.settings_commit(id);
        }
        let Some((items, cur)) = self.settings_menu.level(id) else { return vec![] };
        match items.get(cur).map(|i| &i.kind) {
            Some(ItemKind::Choice { current, .. }) => {
                self.settings_menu.picker = Some((id, *current));
                vec![Effect::Click]
            }
            Some(ItemKind::Submenu(_)) => {
                self.settings_menu.cursors_mut(id).push(0);
                vec![Effect::Click]
            }
            _ => vec![],
        }
    }

    /// Keep the picker's choice.
    fn settings_commit(&mut self, id: SectionId) -> Vec<Effect> {
        let Some((_, choice)) = self.settings_menu.picker else { return vec![] };
        let Some((items, cur)) = self.settings_menu.level(id) else { return vec![] };
        self.settings_menu.picker = None;
        let mut fx = vec![Effect::Click];
        if let Some(ItemKind::Choice { key, .. }) = items.get(cur).map(|i| &i.kind) {
            if self.settings_menu.settings.apply(*key, choice) {
                self.refresh_sources();
                // The item list may have changed (Roles): keep the cursor on the same item.
                if let Some((after, _)) = self.settings_menu.level(id) {
                    if let Some(i) = after.iter().position(|it| it.label == items[cur].label) {
                        *self.settings_menu.cursors_mut(id).last_mut().unwrap() = i;
                    }
                }
                fx.push(Effect::SettingsChanged(Box::new(self.settings_menu.settings.clone())));
            }
        }
        fx
    }

    /// Draw a settings section's content (the arc and the rest are the normal UI's).
    pub(crate) fn paint_settings(&mut self, s: &mut Scene, id: SectionId, opacity: f32) {
        let m = &self.settings_menu;
        let body = menu::section(id, &m.settings, &m.facts, m.hidden).body;
        if let Body::Info(rows) = body {
            self.paint_info(s, &rows, opacity);
            return;
        }
        let Some((items, cur)) = m.level(id) else { return };
        let picker = m.picker.filter(|(p, _)| *p == id).map(|(_, c)| c);
        let audio_master = m.settings.roles.audio_master;
        let base = (CY + 5.0).round() as i32;
        if let (Some(p), Some(item)) = (picker, items.get(cur)) {
            if let ItemKind::Choice { options, current, .. } = &item.kind {
                s.text(PICK_LABEL_X, base, 18, WHITE.with_opacity(opacity), Align::Left, item.label.clone());
                s.text(PICK_VALUE_RIGHT, base, 18, WHITE.with_opacity(opacity), Align::Right, options[*current].clone());
                for (i, o) in options.iter().enumerate() {
                    self.paint_row(s, i as f32 - p as f32, o, WHITE, WHITE, opacity, false);
                }
                return;
            }
        }
        for (i, item) in items.iter().enumerate() {
            let colour = if item.label == "AM OPTION" && !audio_master { ITEM_GREY } else { WHITE };
            self.paint_row(s, i as f32 - cur as f32, &item.label, colour, colour, opacity, false);
        }
        match items.get(cur).map(|i| &i.kind) {
            Some(ItemKind::Value(v)) => {
                let v = crate::draw::fit(&mut self.font, v, 16, 290.0);
                s.text(VALUE_X, base, 16, WHITE.with_opacity(opacity), Align::Left, v);
            }
            Some(ItemKind::Unavailable) => {
                s.text(VALUE_X, base, 16, ITEM_GREY.with_opacity(opacity), Align::Left, "NOT AVAILABLE");
            }
            _ => {}
        }
    }
}

/// The settings icon left of the arc's middle, where the other sources show the playing cover:
/// a blue tile with text lines.
pub(crate) fn paint_icon(s: &mut Scene, opacity: f32) {
    s.fill(Rect::new(20, 343, 82, 82), Color::rgb(25, 118, 210).with_opacity(opacity));
    for (i, w) in [32u32, 44, 50, 42, 32].into_iter().enumerate() {
        s.fill(Rect::new(38, 357 + i as i32 * 12, w, 5), Color::rgb(215, 220, 230).with_opacity(opacity));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::Library;
    use std::time::Duration;

    fn ms(n: u64) -> Time {
        Duration::from_millis(n)
    }

    /// The UI with SETTINGS highlighted in the MODE list.
    fn at_settings() -> Ui {
        let mut ui = Ui::new(Library::generate());
        ui.input(Input::Pointer(92), ms(0)); // MODE
        ui.input(Input::MenuWheel(3 * COUNTS_PER_STEP), ms(1));
        assert!(ui.settings_highlighted_in_mode());
        ui
    }

    fn go(ui: &mut Ui, t: u64) -> Vec<Effect> {
        ui.input(Input::Pressed(Button::Go), ms(t));
        ui.input(Input::Released(Button::Go), ms(t + 50))
    }

    fn open_hidden(ui: &mut Ui) {
        for (i, b) in COMBO.iter().enumerate() {
            ui.input(Input::Pressed(*b), ms(100 + i as u64 * 100));
            ui.input(Input::Released(*b), ms(150 + i as u64 * 100));
        }
        go(ui, 600);
    }

    fn section_titles(ui: &Ui) -> Vec<&'static str> {
        ui.zones.iter().filter(|z| matches!(z.root, Root::Settings(_))).map(|z| z.name).collect()
    }

    fn current(ui: &Ui, id: SectionId) -> String {
        ui.settings_menu.level(id).map(|(i, c)| i[c].label.clone()).unwrap_or_default()
    }

    #[test]
    fn go_on_settings_opens_the_normal_menu() {
        let mut ui = at_settings();
        assert_eq!(go(&mut ui, 10), vec![Effect::SettingsOpened, Effect::SelectSource("SETTINGS")]);
        assert!(ui.settings_open());
        assert_eq!(section_titles(&ui).len(), 7);
        assert_eq!(ui.zone().name, "MODE");
    }

    #[test]
    fn right_left_right_left_go_opens_the_hidden_menu() {
        let mut ui = at_settings();
        open_hidden(&mut ui);
        assert_eq!(section_titles(&ui).len(), 9);
        // Too slow: the normal menu.
        let mut ui = at_settings();
        for (i, b) in COMBO.iter().enumerate() {
            ui.input(Input::Pressed(*b), ms(100 + i as u64 * 2000));
        }
        go(&mut ui, 9000);
        assert_eq!(section_titles(&ui).len(), 7);
    }

    #[test]
    fn hidden_zones_match_the_captured_pointer_positions() {
        let mut ui = at_settings();
        open_hidden(&mut ui);
        let expect = [
            (84, "SYSTEM INFO"),
            (79, "SYSTEM SETTINGS"),
            (75, "MAINTENANCE"),
            (71, "MUSIC SERVICES"),
            (66, "MUSIC SERVICES"),
            (61, "SERVICE SETTINGS"),
            // captured at 56 on the way up; the stock boundary is at 55.6
            (55, "PRODUCT INFO"),
            (52, "PRODUCT INFO"),
            (46, "NETWORK INFO"),
            (41, "NETWORK SETTINGS"),
            (36, "ROLES"),
            (90, "MODE"),
        ];
        for (p, title) in expect {
            ui.input(Input::Pointer(p), ms(1000 + p as u64));
            assert_eq!(ui.zone().name, title, "p {p}");
        }
    }

    #[test]
    fn picker_keeps_with_go_and_cancels_with_right() {
        let mut ui = at_settings();
        open_hidden(&mut ui);
        ui.input(Input::Pointer(79), ms(700)); // SYSTEM SETTINGS
        ui.input(Input::MenuWheel(3 * COUNTS_PER_STEP), ms(710)); // IR TIMEOUT
        assert_eq!(current(&ui, SectionId::SystemSettings), "IR TIMEOUT");
        ui.input(Input::Pressed(Button::Left), ms(720));
        ui.input(Input::MenuWheel(2 * COUNTS_PER_STEP), ms(730)); // 6 -> 8
        assert_eq!(ui.input(Input::Pressed(Button::Right), ms(740)), vec![Effect::Click]);
        assert_eq!(ui.settings().system.ir_timeout, 6);
        ui.input(Input::Pressed(Button::Left), ms(750));
        ui.input(Input::MenuWheel(2 * COUNTS_PER_STEP), ms(760));
        let fx = go(&mut ui, 770);
        assert!(matches!(&fx[..], [Effect::Click, Effect::SettingsChanged(s)] if s.system.ir_timeout == 8));
        assert_eq!(ui.settings().system.ir_timeout, 8);
    }

    #[test]
    fn audio_master_relabels_the_mode_list() {
        let mut ui = at_settings();
        go(&mut ui, 10);
        ui.input(Input::Pointer(30), ms(100)); // ROLES
        ui.input(Input::Pressed(Button::Left), ms(110)); // AUDIO MASTER picker, at NO
        ui.input(Input::MenuWheel(-COUNTS_PER_STEP), ms(120)); // YES
        let fx = go(&mut ui, 130);
        assert!(matches!(&fx[..], [_, Effect::SettingsChanged(s)] if s.roles.audio_master));
        // N.MUSIC naming is offered now; name it A.MEM.
        ui.input(Input::MenuWheel(3 * COUNTS_PER_STEP), ms(200));
        assert_eq!(current(&ui, SectionId::Roles), "N.MUSIC");
        ui.input(Input::Pressed(Button::Left), ms(210));
        ui.input(Input::MenuWheel(-5 * COUNTS_PER_STEP), ms(220)); // to the top: A.MEM
        go(&mut ui, 230);
        assert_eq!(ui.sources[0], "A.MEM");
        // MODE lists it; GO on it leaves settings and selects it.
        ui.input(Input::Pointer(92), ms(300));
        ui.input(Input::MenuWheel(-5 * COUNTS_PER_STEP), ms(310));
        assert_eq!(go(&mut ui, 320), vec![Effect::SelectSource("A.MEM")]);
        assert!(!ui.settings_open());
    }

    #[test]
    fn submenus_go_in_and_out() {
        let mut ui = at_settings();
        go(&mut ui, 10);
        ui.input(Input::Pointer(58), ms(100)); // MUSIC SERVICES (normal menu)
        assert_eq!(ui.zone().name, "MUSIC SERVICES");
        ui.input(Input::Pressed(Button::Left), ms(110)); // SPOTIFY
        assert_eq!(current(&ui, SectionId::MusicServices), "ADD ACCOUNT");
        ui.input(Input::Pressed(Button::Right), ms(120));
        assert_eq!(current(&ui, SectionId::MusicServices), "SPOTIFY");
    }

    #[test]
    fn scene_draws_sections_and_info() {
        let mut ui = at_settings();
        go(&mut ui, 10);
        ui.set_facts(Facts { system_info: vec![("AVAILABLE STORAGE".into(), "1234 MB".into())], ..Facts::default() });
        let texts = |ui: &mut Ui, t: u64| -> Vec<String> {
            ui.scene(ms(t))
                .items
                .iter()
                .filter_map(|p| match p {
                    render::Primitive::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        ui.input(Input::Pointer(76), ms(20)); // SYSTEM SETTINGS
        let t = texts(&mut ui, 2000);
        assert!(t.contains(&"ROLES".to_string()) && t.contains(&"LANGUAGE".to_string()), "{t:?}");
        ui.input(Input::Pointer(84), ms(2100)); // SYSTEM INFO
        let t = texts(&mut ui, 4000);
        assert!(t.contains(&"1234 MB".to_string()), "{t:?}");
    }
}
