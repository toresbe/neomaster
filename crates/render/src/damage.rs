//! Which parts of the screen a new scene changes, so a backend redraws and presents only those.
//!
//! On the BM5 a whole 1024x768 frame costs the X server 30-50 ms, most of it proportional to
//! the area touched (pixman, and EXA copying pixmaps between video and system memory). Moving
//! the pointer changes the beam, the flare and a label or two, about a quarter of the screen.
//!
//! The rule is conservative: an item that differs from the one at the same place in the
//! previous scene damages its old and its new bounds; when items were added or removed, every
//! item between the common start and the common end does. Everything drawn inside the damage is
//! drawn again, clipped to it, so overlapping unchanged items come out right.

use crate::{Primitive, Rect, Scene};

/// Merge rectangles closer than this; a few large rectangles cost less than many small ones.
const MERGE_GAP: i64 = 16;
/// More rectangles than this are merged into one.
const MAX_RECTS: usize = 8;
/// Above this share of the screen, redraw all of it.
const FULL_SHARE: f64 = 0.6;

#[derive(Clone, Debug, PartialEq)]
pub enum Damage {
    /// Nothing changed.
    None,
    /// Redraw everything.
    Full,
    /// Redraw these rectangles (inside the screen, not overlapping by more than a merge gap).
    Rects(Vec<Rect>),
}

impl Damage {
    /// Pixels to redraw.
    pub fn area(&self, screen: Rect) -> u64 {
        match self {
            Damage::None => 0,
            Damage::Full => area(screen),
            Damage::Rects(r) => r.iter().map(|r| area(*r)).sum(),
        }
    }
}

/// What changes from `prev` (the frame on screen, if any) to `next`. `bounds` gives the screen
/// area an item can touch.
pub fn between(prev: Option<&Scene>, next: &Scene, screen: Rect, mut bounds: impl FnMut(&Primitive) -> Rect) -> Damage {
    let Some(prev) = prev else { return Damage::Full };
    if prev.background != next.background {
        return Damage::Full;
    }
    let mut rects = Vec::new();
    let (a, b) = (&prev.items, &next.items);
    if a.len() == b.len() {
        for (x, y) in a.iter().zip(b) {
            if x != y {
                rects.push(bounds(x));
                rects.push(bounds(y));
            }
        }
    } else {
        let start = a.iter().zip(b).take_while(|(x, y)| x == y).count();
        let max_end = a.len().min(b.len()) - start;
        let end = a.iter().rev().zip(b.iter().rev()).take(max_end).take_while(|(x, y)| x == y).count();
        for item in a[start..a.len() - end].iter().chain(&b[start..b.len() - end]) {
            rects.push(bounds(item));
        }
    }
    let rects: Vec<Rect> = rects.into_iter().filter_map(|r| clip(r, screen)).collect();
    if rects.is_empty() {
        return Damage::None;
    }
    let rects = merge(rects);
    if rects.iter().map(|r| area(*r)).sum::<u64>() as f64 > FULL_SHARE * area(screen) as f64 {
        return Damage::Full;
    }
    Damage::Rects(rects)
}

fn area(r: Rect) -> u64 {
    r.w as u64 * r.h as u64
}

fn edges(r: Rect) -> (i64, i64, i64, i64) {
    (r.x as i64, r.y as i64, r.x as i64 + r.w as i64, r.y as i64 + r.h as i64)
}

fn from_edges(x0: i64, y0: i64, x1: i64, y1: i64) -> Rect {
    Rect::new(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
}

fn clip(r: Rect, screen: Rect) -> Option<Rect> {
    let (ax0, ay0, ax1, ay1) = edges(r);
    let (bx0, by0, bx1, by1) = edges(screen);
    let (x0, y0, x1, y1) = (ax0.max(bx0), ay0.max(by0), ax1.min(bx1), ay1.min(by1));
    (x1 > x0 && y1 > y0).then(|| from_edges(x0, y0, x1, y1))
}

fn near(a: Rect, b: Rect) -> bool {
    let (ax0, ay0, ax1, ay1) = edges(a);
    let (bx0, by0, bx1, by1) = edges(b);
    ax0 <= bx1 + MERGE_GAP && bx0 <= ax1 + MERGE_GAP && ay0 <= by1 + MERGE_GAP && by0 <= ay1 + MERGE_GAP
}

fn union(a: Rect, b: Rect) -> Rect {
    let (ax0, ay0, ax1, ay1) = edges(a);
    let (bx0, by0, bx1, by1) = edges(b);
    from_edges(ax0.min(bx0), ay0.min(by0), ax1.max(bx1), ay1.max(by1))
}

fn merge(mut rects: Vec<Rect>) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    while let Some(mut r) = rects.pop() {
        // Grow r until nothing left in `out` is near it, then keep it.
        while let Some(i) = out.iter().position(|o| near(*o, r)) {
            r = union(r, out.swap_remove(i));
        }
        out.push(r);
    }
    if out.len() > MAX_RECTS {
        let all = out.iter().copied().reduce(union).expect("not empty");
        return vec![all];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Color, Primitive};

    const SCREEN: Rect = Rect::new(0, 0, 1024, 768);

    fn fill(x: i32, y: i32, w: u32, h: u32) -> Primitive {
        Primitive::Fill { rect: Rect::new(x, y, w, h), color: Color::WHITE }
    }

    fn scene(items: Vec<Primitive>) -> Scene {
        Scene { background: Color::BLACK, items }
    }

    fn bounds(p: &Primitive) -> Rect {
        match p {
            Primitive::Fill { rect, .. } => *rect,
            _ => SCREEN,
        }
    }

    #[test]
    fn first_frame_and_new_background_are_full() {
        let s = scene(vec![fill(0, 0, 10, 10)]);
        assert_eq!(between(None, &s, SCREEN, bounds), Damage::Full);
        let mut t = s.clone();
        t.background = Color::WHITE;
        assert_eq!(between(Some(&s), &t, SCREEN, bounds), Damage::Full);
    }

    #[test]
    fn same_scene_is_nothing() {
        let s = scene(vec![fill(0, 0, 10, 10), fill(100, 100, 10, 10)]);
        assert_eq!(between(Some(&s), &s.clone(), SCREEN, bounds), Damage::None);
    }

    #[test]
    fn a_moved_item_damages_where_it_was_and_where_it_is() {
        let a = scene(vec![fill(0, 0, 10, 10), fill(100, 100, 10, 10), fill(500, 500, 10, 10)]);
        let b = scene(vec![fill(0, 0, 10, 10), fill(300, 100, 10, 10), fill(500, 500, 10, 10)]);
        let Damage::Rects(mut r) = between(Some(&a), &b, SCREEN, bounds) else { panic!() };
        r.sort_by_key(|r| r.x);
        assert_eq!(r, vec![Rect::new(100, 100, 10, 10), Rect::new(300, 100, 10, 10)]);
    }

    #[test]
    fn an_inserted_item_damages_only_the_middle() {
        let a = scene(vec![fill(0, 0, 10, 10), fill(500, 500, 10, 10)]);
        let b = scene(vec![fill(0, 0, 10, 10), fill(200, 200, 10, 10), fill(500, 500, 10, 10)]);
        assert_eq!(between(Some(&a), &b, SCREEN, bounds), Damage::Rects(vec![Rect::new(200, 200, 10, 10)]));
        assert_eq!(between(Some(&b), &a, SCREEN, bounds), Damage::Rects(vec![Rect::new(200, 200, 10, 10)]));
    }

    #[test]
    fn near_rects_merge_and_offscreen_parts_are_cut() {
        let a = scene(vec![fill(0, 0, 10, 10), fill(-50, 700, 100, 100)]);
        let b = scene(vec![fill(15, 0, 10, 10), fill(-40, 700, 100, 100)]);
        let Damage::Rects(mut r) = between(Some(&a), &b, SCREEN, bounds) else { panic!() };
        r.sort_by_key(|r| r.y);
        assert_eq!(r, vec![Rect::new(0, 0, 25, 10), Rect::new(0, 700, 60, 68)]);
    }

    #[test]
    fn most_of_the_screen_is_full() {
        let a = scene(vec![fill(0, 0, 1000, 700)]);
        let b = scene(vec![fill(1, 0, 1000, 700)]);
        assert_eq!(between(Some(&a), &b, SCREEN, bounds), Damage::Full);
    }
}
