//! The layout, measured from the stock screen (1024x768) on 27 Nov 2026.
//!
//! Everything sits on circles about one centre, (1153, 384), just off the right edge:
//! - the menu arc and its pointer at radius 893, the zone labels right-aligned 22 px left of it;
//! - list rows at radius 494 (the centre row at 499), one row per 10.4 degrees;
//! - covers at radius 379 (centres), one per 19.65 degrees, 108 px in the centre and 81 px out;
//! - the selected album's cover, artist and album in between, left edge on radius 839.
//!
//! Text is BeoFont (or the bundled stand-in) at 16 px, 18 px for the centre row and 30 px for
//! the now-playing title. Labels are grey 126 and white when current; rows are white; the
//! now-playing title is grey 140 and its caption grey 78.

use std::f32::consts::PI;
use std::sync::Arc;

use render::{Align, Blend, Color, Font, Image, ImageDraw, Rect, Scene};

use crate::{
    Item, Root, Time, Ui, ViewKind, CONTEXT_ENTRIES, FADE_BLACK, FADE_IN, FADE_OUT, LETTER_HOLD, MAX_VOLUME, VOLUME_FADE,
};

const CX: f32 = 1153.0;
const CY: f32 = 384.0;
const ARC_R: f32 = 893.0;
const ARC_X: i32 = 260;
const LABEL_GAP: f32 = 22.0;
/// The labels share out these spans of the arc (degrees, negative is up): the upper half of the
/// zones from just above the middle to the top, the lower half from just below it to the bottom.
const TOP_SPAN: (f32, f32) = (-4.4, -23.2);
const BOTTOM_SPAN: (f32, f32) = (4.3, 23.1);
/// Degrees of arc per pointer step, measured in N.MUSIC; used beyond the outermost labels.
const DEG_PER_STEP: f32 = 0.83;
const ARC_LIMIT: f32 = 24.5;

const LIST_R: f32 = 494.0;
const LIST_R_CENTRE: f32 = 499.0;
const LIST_STEP: f32 = 10.4;
const LIST_MAX_W: f32 = 195.0;
const LIST_MAX_W_CENTRE: f32 = 230.0;

const COVER_R: f32 = 379.0;
const COVER_STEP: f32 = 19.65;
const COVER_SMALL: f32 = 81.0;
const COVER_BIG: f32 = 108.0;

const SMALL: f32 = 16.0;
const CENTRE: f32 = 18.0;
const NP_SIZE: u16 = 30;

const WHITE: Color = Color::rgb(255, 255, 255);
const LABEL_GREY: Color = Color::rgb(126, 126, 126);
const ITEM_GREY: Color = Color::rgb(153, 153, 153);
const NP_GREY: Color = Color::rgb(140, 140, 140);
const CAPTION_GREY: Color = Color::rgb(78, 78, 78);
const MARKER: Color = Color::rgb(0, 64, 255);
const VOLUME_TINT: Color = Color::rgb(0, 68, 176);

fn polar(r: f32, deg: f32) -> (f32, f32) {
    let a = deg.to_radians();
    (CX - r * a.cos(), CY + r * a.sin())
}

fn arc_x(y: f32) -> f32 {
    let dy = (y - CY).clamp(-ARC_R + 1.0, ARC_R - 1.0);
    CX - (ARC_R * ARC_R - dy * dy).sqrt()
}

/// `text` cut to `max` px at `size`, ending in an ellipsis if it was cut.
pub(crate) fn fit(font: &mut Font, text: &str, size: u16, max: f32) -> String {
    if font.width(text, size) as f32 <= max {
        return text.to_string();
    }
    let dots = if font.has_glyph('…') { "…" } else { "..." };
    let mut t: Vec<char> = text.chars().collect();
    while !t.is_empty() {
        t.pop();
        let s: String = t.iter().collect::<String>().trim_end().to_string() + dots;
        if font.width(&s, size) as f32 <= max {
            return s;
        }
    }
    dots.to_string()
}

/// Greedy word wrap into at most `lines` lines of `max` px; the last one ends in an ellipsis if
/// text is left over.
fn wrap(font: &mut Font, text: &str, size: u16, max: f32, lines: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < words.len() && out.len() < lines {
        let mut line = words[i].to_string();
        i += 1;
        while i < words.len() {
            let candidate = format!("{line} {}", words[i]);
            if font.width(&candidate, size) as f32 > max {
                break;
            }
            line = candidate;
            i += 1;
        }
        out.push(line);
    }
    if i < words.len() {
        if let Some(last) = out.last_mut() {
            let rest = format!("{last} {}", words[i..].join(" "));
            *last = fit(font, &rest, size, max);
        }
    }
    for l in &mut out {
        if font.width(l, size) as f32 > max {
            *l = fit(font, l, size, max);
        }
    }
    out
}

fn rect_centred(cx: f32, cy: f32, size: f32) -> Rect {
    let s = size.round();
    Rect::new((cx - s / 2.0).round() as i32, (cy - s / 2.0).round() as i32, s as u32, s as u32)
}

/// What the frame shows while views change.
struct Phase {
    /// Whose content, if anyone's, and how opaque.
    content: Option<(usize, f32)>,
    /// The arc, its labels and the now-playing thumbnail.
    chrome: f32,
    /// Covers are still "loading": draw default covers.
    placeholders: bool,
}

impl Ui {
    fn phase(&self, now: Time) -> Phase {
        let current = self.zone_index();
        let np = |z: usize| self.zones[z].kind == ViewKind::NowPlaying;
        let Some(sw) = self.switch else {
            return Phase { content: Some((current, 1.0)), chrome: if np(current) { 0.0 } else { 1.0 }, placeholders: false };
        };
        let e = now.saturating_sub(sw.start);
        let from_np = sw.from.is_some_and(np);
        let to_np = np(current);
        let steady_chrome = !from_np && !to_np;
        if e < FADE_OUT {
            let o = 1.0 - e.as_secs_f32() / FADE_OUT.as_secs_f32();
            let chrome = if steady_chrome {
                1.0
            } else if from_np {
                0.0
            } else {
                o
            };
            Phase { content: sw.from.map(|z| (z, o)), chrome, placeholders: false }
        } else if e < FADE_OUT + FADE_BLACK {
            Phase { content: None, chrome: if steady_chrome { 1.0 } else { 0.0 }, placeholders: false }
        } else {
            let o = ((e - FADE_OUT - FADE_BLACK).as_secs_f32() / FADE_IN.as_secs_f32()).min(1.0);
            let chrome = if steady_chrome {
                1.0
            } else if to_np {
                0.0
            } else {
                o
            };
            Phase { content: Some((current, o)), chrome, placeholders: o < 1.0 }
        }
    }

    pub(crate) fn paint(&mut self, s: &mut Scene, now: Time) {
        let phase = self.phase(now);
        if phase.chrome > 0.0 {
            self.paint_arc(s, phase.chrome);
            if self.in_settings() {
                crate::settings_menu::paint_icon(s, phase.chrome);
            } else {
                let art = self.playing_art_small();
                s.image(&art, Rect::new(20, 343, 82, 82), phase.chrome);
            }
        }
        if let Some((z, o)) = phase.content {
            if let Root::Settings(id) = self.zones[z].root {
                self.paint_settings(s, id, o);
                self.paint_volume(s, now);
                return;
            }
            match self.zones[z].kind {
                ViewKind::NowPlaying => self.paint_now_playing(s, o),
                ViewKind::List | ViewKind::Menu => self.paint_list(s, z, o, now),
                ViewKind::Covers => self.paint_covers(s, z, o, phase.placeholders),
                ViewKind::Info => {}
            }
        }
        self.paint_volume(s, now);
    }

    /// The label angle (degrees) of every zone that has a label, by zone index.
    fn label_angles(&self) -> Vec<(usize, f32)> {
        let labelled: Vec<usize> = (0..self.zones.len()).filter(|i| self.zones[*i].kind != ViewKind::NowPlaying).collect();
        let n = labelled.len();
        let bottom = n / 2;
        let top = n - bottom;
        let spread = |span: (f32, f32), count: usize, k: usize| {
            if count <= 1 {
                span.0
            } else {
                span.0 + (span.1 - span.0) * k as f32 / (count - 1) as f32
            }
        };
        labelled
            .iter()
            .enumerate()
            .map(|(k, z)| {
                // zones run from the lowest pointer (the bottom of the arc) upwards
                let a = if k < bottom { spread(BOTTOM_SPAN, bottom, bottom - 1 - k) } else { spread(TOP_SPAN, top, k - bottom) };
                (*z, a)
            })
            .collect()
    }

    /// Where the pointer sits on the arc, in degrees: through each label at its zone's middle
    /// position, straight lines between, the measured slope beyond.
    fn pointer_angle(&self, labels: &[(usize, f32)]) -> f32 {
        let p = self.pointer_drawn;
        let pts: Vec<(f32, f32)> =
            labels.iter().map(|(z, a)| ((self.zones[*z].lo as f32 + self.zones[*z].hi as f32) / 2.0, *a)).collect();
        let a = match pts.iter().position(|(zp, _)| *zp >= p) {
            Some(0) => pts[0].1 + (pts[0].0 - p) * DEG_PER_STEP,
            None => {
                let last = pts[pts.len() - 1];
                last.1 - (p - last.0) * DEG_PER_STEP
            }
            Some(i) => {
                let (a, b) = (pts[i - 1], pts[i]);
                a.1 + (b.1 - a.1) * (p - a.0) / (b.0 - a.0)
            }
        };
        a.clamp(-ARC_LIMIT, ARC_LIMIT)
    }

    fn paint_arc(&mut self, s: &mut Scene, opacity: f32) {
        let arc = self.assets.arc.clone();
        s.draw(ImageDraw {
            src: arc.rect(),
            dst: Rect::new(ARC_X, 0, arc.width, arc.height),
            image: arc,
            opacity,
            blend: Blend::Add,
            tint: None,
            angle: 0.0,
        });
        let labels = self.label_angles();
        let current = self.zone_index();
        for (z, a) in &labels {
            let y = CY + ARC_R * a.to_radians().sin();
            let colour = if *z == current { WHITE } else { LABEL_GREY };
            s.text(
                (arc_x(y) - LABEL_GAP).round() as i32,
                (y + 6.0).round() as i32,
                SMALL as u16,
                colour.with_opacity(opacity),
                Align::Right,
                self.zones[*z].name,
            );
        }
        // the pointer: a beam along the radius and a white flare on the arc
        let a = self.pointer_angle(&labels);
        let (px, py) = polar(ARC_R, a);
        let beam = self.assets.beam.clone();
        let angle = (py - CY).atan2(px - CX) + PI;
        s.draw(ImageDraw {
            src: beam.rect(),
            dst: Rect::new(
                (px - beam.width as f32 / 2.0).round() as i32,
                (py - beam.height as f32 / 2.0).round() as i32,
                beam.width,
                beam.height,
            ),
            image: beam,
            opacity,
            blend: Blend::Add,
            tint: None,
            angle,
        });
        let flare = self.assets.flare.clone();
        s.draw(ImageDraw {
            src: flare.rect(),
            dst: rect_centred(px, py, 44.0),
            image: flare,
            opacity,
            blend: Blend::Add,
            tint: None,
            angle: 0.0,
        });
    }

    fn playing_art_small(&mut self) -> Arc<Image> {
        match self.playing {
            Some(t) => {
                let album = self.lib.tracks[t].album;
                self.album_art(album)
            }
            None => self.assets.cover_small.clone(),
        }
    }

    fn album_of(&self, item: Item) -> Option<usize> {
        match item {
            Item::Album(a) => Some(a),
            Item::Track(t) => Some(self.lib.tracks[t].album),
            _ => None,
        }
    }

    fn is_playing(&self, item: Item) -> bool {
        let Some(p) = self.playing else { return false };
        match item {
            Item::Track(t) => t == p,
            Item::Album(a) => self.lib.tracks[p].album == a,
            _ => false,
        }
    }

    /// The selected album's cover, artist and album, between the arc and the list.
    fn paint_selection(&mut self, s: &mut Scene, item: Item, opacity: f32, placeholder: bool) {
        let Some(album) = self.album_of(item) else { return };
        let art = if placeholder { self.assets.cover_medium.clone() } else { self.album_art(album) };
        s.image(&art, Rect::new(314, 330, COVER_BIG as u32, COVER_BIG as u32), opacity);
        let artist = self.lib.artists[self.lib.albums[album].artist].name.clone();
        let title = self.lib.albums[album].title.clone();
        let artist = fit(&mut self.font, &artist, SMALL as u16, 260.0);
        let title = fit(&mut self.font, &title, SMALL as u16, 260.0);
        s.text(323, 483, SMALL as u16, WHITE.with_opacity(opacity), Align::Left, artist);
        s.text(333, 557, SMALL as u16, WHITE.with_opacity(opacity), Align::Left, title);
    }

    /// One row on the list curve. `d` is its distance from the centre row, in rows.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_row(
        &mut self,
        s: &mut Scene,
        d: f32,
        text: &str,
        colour: Color,
        centre_colour: Color,
        opacity: f32,
        marker: bool,
    ) {
        let t = (1.0 - d.abs()).max(0.0);
        let (x, y) = polar(LIST_R + (LIST_R_CENTRE - LIST_R) * t, d * LIST_STEP);
        if !(-20.0..=788.0).contains(&y) {
            return;
        }
        let size = (SMALL + (CENTRE - SMALL) * t).round() as u16;
        let text = fit(&mut self.font, text, size, LIST_MAX_W + (LIST_MAX_W_CENTRE - LIST_MAX_W) * t);
        let colour = if t > 0.5 { centre_colour } else { colour };
        s.text(x.round() as i32, (y + 5.0).round() as i32, size, colour.with_opacity(opacity), Align::Left, text);
        if marker {
            s.fill(Rect::new(x.round() as i32 - 13, (y - 24.0).round() as i32, 2, 49), MARKER.with_opacity(opacity));
        }
    }

    fn paint_context(&mut self, s: &mut Scene, opacity: f32) {
        let scroll = self.context_scroll;
        for (i, e) in CONTEXT_ENTRIES.iter().enumerate() {
            let colour = match *e {
                "RED" => Color::rgb(255, 0, 0),
                "GREEN" => Color::rgb(0, 255, 0),
                "YELLOW" => Color::rgb(255, 255, 0),
                "BLUE" => Color::rgb(0, 0, 255),
                "ADD TO  QUEUE" => WHITE,
                _ => ITEM_GREY,
            };
            self.paint_row(s, i as f32 - scroll, e, colour, if colour == ITEM_GREY { WHITE } else { colour }, opacity, false);
        }
    }

    fn paint_list(&mut self, s: &mut Scene, z: usize, opacity: f32, now: Time) {
        let Some(level) = self.level_of(z) else { return };
        let (scroll, cursor, alphabetical) = (level.scroll, level.cursor, level.alphabetical);
        let current = level.items.get(cursor).copied();
        let rows: Vec<(usize, Item)> =
            level.items.iter().enumerate().filter(|(i, _)| (*i as f32 - scroll).abs() < 5.0).map(|(i, it)| (i, *it)).collect();
        if let Some(item) = current {
            self.paint_selection(s, item, opacity, false);
        }
        if self.context.is_some() && z == self.zone_index() {
            self.paint_context(s, opacity);
            return;
        }
        if rows.is_empty() {
            return;
        }
        for (i, item) in rows {
            let label = self.label(item);
            let colour = WHITE;
            let marker = self.is_playing(item);
            self.paint_row(s, i as f32 - scroll, &label, colour, colour, opacity, marker);
        }
        // the first letter while the wheel turns through a sorted list
        if alphabetical && now < self.last_step + LETTER_HOLD && z == self.zone_index() {
            if let Some(item) = current {
                let first: String = self.label(item).chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                s.text(610, (CY + 5.0) as i32, SMALL as u16, WHITE.with_opacity(opacity), Align::Left, first);
            }
        }
    }

    fn paint_covers(&mut self, s: &mut Scene, z: usize, opacity: f32, placeholders: bool) {
        let Some(level) = self.level_of(z) else { return };
        let scroll = level.scroll;
        let current = level.items.get(level.cursor).copied();
        let covers: Vec<(f32, Item)> =
            level.items.iter().enumerate().map(|(i, it)| (i as f32 - scroll, *it)).filter(|(d, _)| d.abs() < 4.0).collect();
        if let Some(item) = current {
            self.paint_selection(s, item, opacity, placeholders);
        }
        if self.context.is_some() && z == self.zone_index() {
            self.paint_context(s, opacity);
            return;
        }
        // far ones first, so the centre cover is on top while they slide past each other
        let mut covers = covers;
        covers.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
        for (d, item) in covers {
            let Item::Album(album) = item else { continue };
            let t = (1.0 - d.abs()).max(0.0);
            let size = COVER_SMALL + (COVER_BIG - COVER_SMALL) * t;
            let (cx, cy) = polar(COVER_R + 1.5 * t, d * COVER_STEP);
            let art = if placeholders {
                if t > 0.5 {
                    self.assets.cover_medium.clone()
                } else {
                    self.assets.cover_small.clone()
                }
            } else {
                self.album_art(album)
            };
            let r = rect_centred(cx, cy, size);
            s.image(&art, r, opacity);
            if self.is_playing(item) {
                s.fill(Rect::new(r.x - 13, (cy - 24.0).round() as i32, 2, 49), MARKER.with_opacity(opacity));
            }
        }
    }

    /// A two-column key/value page (the info sections).
    pub(crate) fn paint_info(&mut self, s: &mut Scene, rows: &[(String, String)], opacity: f32) {
        let n = rows.len() as f32;
        let centre = if rows.len() > 1 { CY + 8.0 } else { CY };
        for (i, (k, v)) in rows.iter().enumerate() {
            let y = centre + (i as f32 - (n - 1.0) / 2.0) * 33.0;
            let base = (y + 5.0).round() as i32;
            s.text(340, base, SMALL as u16, WHITE.with_opacity(opacity), Align::Left, k.clone());
            let v = fit(&mut self.font, v, SMALL as u16, 380.0);
            s.text(616, base, SMALL as u16, WHITE.with_opacity(opacity), Align::Left, v);
        }
    }

    fn paint_now_playing(&mut self, s: &mut Scene, opacity: f32) {
        let Some(t) = self.playing else {
            let cover = self.assets.cover_large.clone();
            s.image(&cover, Rect::new(55, 116, 512, 512), opacity);
            return;
        };
        let album = self.lib.tracks[t].album;
        let art = self.album_art(album);
        s.image(&art, Rect::new(55, 116, 512, 512), opacity);
        let title = self.lib.tracks[t].title.clone();
        for (i, line) in wrap(&mut self.font, &title, NP_SIZE, 240.0, 3).into_iter().enumerate() {
            s.text(608, 382 + (i as f32 * 38.5).round() as i32, NP_SIZE, NP_GREY.with_opacity(opacity), Align::Left, line);
        }
        let caption = format!("{} / {}", self.lib.artists[self.lib.albums[album].artist].name, self.lib.albums[album].title);
        let caption = fit(&mut self.font, &caption, SMALL as u16, 512.0);
        s.text(55, 652, SMALL as u16, CAPTION_GREY.with_opacity(opacity), Align::Left, caption);
    }

    /// The volume arc at the right edge, lit from the bottom up to the level (the stock level
    /// mapping is not measured yet), held after each change and then faded.
    fn paint_volume(&self, s: &mut Scene, now: Time) {
        let end = self.volume_shown_until + VOLUME_FADE;
        if now >= end || self.volume_shown_until == Time::ZERO {
            return;
        }
        let opacity = if now < self.volume_shown_until { 1.0 } else { (end - now).as_secs_f32() / VOLUME_FADE.as_secs_f32() };
        let img = self.assets.volume_arc.clone();
        let (top, bottom) = (img.height as f32 * 0.035, img.height as f32 * 0.965);
        let lit = self.volume as f32 / MAX_VOLUME as f32;
        let from = (bottom - (bottom - top) * lit).floor().max(0.0) as u32;
        let h = img.height - from;
        s.draw(ImageDraw {
            src: Rect::new(0, from as i32, img.width, h),
            dst: Rect::new(863, 100 + from as i32, img.width, h),
            image: img,
            opacity,
            blend: Blend::Add,
            tint: Some(VOLUME_TINT),
            angle: 0.0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_land_where_the_stock_ones_do() {
        let ui = Ui::new(crate::fake::Library::generate());
        let ys: Vec<(&str, i32)> = ui
            .label_angles()
            .into_iter()
            .map(|(z, a)| (ui.zones[z].name, (CY + ARC_R * a.to_radians().sin()).round() as i32))
            .collect();
        // text centres on the stock screen
        let stock = [
            ("QUEUE", 735),
            ("FAVOURITES", 593),
            ("TITLES", 451),
            ("ALBUMS", 316),
            ("ARTISTS", 222),
            ("COVERS", 128),
            ("MODE", 32),
        ];
        for ((name, y), (sname, sy)) in ys.iter().zip(stock) {
            assert_eq!(*name, sname);
            assert!((y - sy).abs() <= 5, "{name} at {y}, stock {sy}");
        }
    }

    #[test]
    fn rows_follow_the_stock_curve() {
        // left edges of the rows around the centre on the stock screen: 666, 691, 730, 784
        for (d, x) in [(1.0, 666.0), (2.0, 691.0), (3.0, 730.0), (4.0, 784.0)] {
            let (px, _) = polar(LIST_R, d * LIST_STEP);
            assert!((px - x).abs() <= 6.0, "row {d}: {px} vs {x}");
        }
    }

    #[test]
    fn fit_and_wrap() {
        let mut f = Font::default_font();
        assert_eq!(fit(&mut f, "short", 16, 200.0), "short");
        let cut = fit(&mut f, "A rather long album title that will not fit", 16, 120.0);
        assert!(cut.ends_with('…') || cut.ends_with("..."));
        assert!(f.width(&cut, 16) <= 120);
        let lines = wrap(&mut f, "9 - Tintinnabulum - 09. Mein Weg Hat Gipfel Und Täler Und Mehr", 30, 240.0, 3);
        assert_eq!(lines.len(), 3);
    }
}
