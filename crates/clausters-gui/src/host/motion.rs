//! **Motion**: what opens over a window comes in rather than appearing. A
//! list, a tip, a hover card and a dialog fade in and grow from 95 % of
//! their size over [`ENTER_MS`], the design system's timing, and a list, a
//! tip or a card slides the last [`Metrics::motion_slide`] toward where it
//! stands from the side of what it opens off; a dialog grows from its middle
//! and the scrim behind it fades in with it.
//!
//! **It costs nothing at rest.** A front draws frames only while something
//! is coming in ([`Entrance::moving`]), at its usual frame pace, and none
//! once it stands: a window with nothing opening draws exactly what it drew
//! before motion existed. Only the coming in is drawn: what closes goes at
//! once, because a list that faded out would still be over what the hand
//! does next and its state is already gone.
//!
//! **The picture is moved, not drawn again differently**: each part is
//! drawn where it stands, as it always was, and the run of the batch it
//! drew is then moved ([`Mesh::move_since`]) -- so its anatomy, its hit
//! test and its picture at rest are the one drawing, and the motion is a
//! transform over it.
//!
//! **The host decides whether anything moves** ([`super::Host::set_motion`]):
//! on by default, off from the configuration (`[gui] motion = false`), the
//! command line (`--no-motion`) or the page's bridge. A capture is always
//! drawn at rest, so a picture a script takes right after opening a menu
//! shows the menu.

use std::cell::RefCell;

use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::paint::{Mark, Mesh, Motion};
use clausters_gui_draw::rect::Rect;

use super::popup::Owner;

/// How long a part takes to come in, in milliseconds.
pub const ENTER_MS: f64 = 100.0;

/// The size a part comes in from, against the size it stands at.
pub const FROM_SCALE: f32 = 0.95;

/// **What comes in**, by what it is: a part keeps its time from its first
/// frame for as long as it stays open, and a new one starts its own.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// A list of a stack: its owner and the rows down to it, so a submenu
    /// opened in place of another comes in again and a list whose rows
    /// change does not.
    List(Owner, Vec<usize>),
    /// The tip of this widget.
    Tip(i32),
    /// The note (or hover card) of this element.
    Note(i32),
    /// The dialog of this group, and its scrim.
    Dialog(i32),
}

/// **Where a window's parts came in**, and the frame being drawn.
///
/// A front begins a frame with the time ([`begin`](Self::begin)), the frame
/// asks each part it draws how far in it is ([`progress`](Self::progress)),
/// and the front ends the frame ([`end`](Self::end)), which forgets the
/// parts a whole frame did not draw. Asking takes a shared reference, since
/// the frame reads its inputs through one.
#[derive(Debug, Default)]
pub struct Entrance {
    /// Each part drawn, when its first frame was, and whether this frame
    /// drew it.
    parts: RefCell<Vec<(Part, f64, bool)>>,
    /// The time of the frame being drawn, on the front's clock.
    now: f64,
    /// Whether anything moves in this frame.
    on: bool,
}

impl Entrance {
    /// **A frame starts at `now_ms`**, with motion `on` or not. Off, every
    /// part is drawn standing and the record is left as it is.
    pub fn begin(&mut self, now_ms: f64, on: bool) {
        self.now = now_ms;
        self.on = on;
        for part in self.parts.get_mut() {
            part.2 = false;
        }
    }

    /// **How far in `part` is**, eased, from `0` to `1`: `1` with motion off,
    /// `0` on its first frame.
    pub fn progress(&self, part: &Part) -> f32 {
        if !self.on {
            return 1.0;
        }
        let mut parts = self.parts.borrow_mut();
        let since = match parts.iter_mut().find(|(p, _, _)| p == part) {
            Some(found) => {
                found.2 = true;
                found.1
            }
            None => {
                parts.push((part.clone(), self.now, true));
                self.now
            }
        };
        ease(((self.now - since) / ENTER_MS).clamp(0.0, 1.0) as f32)
    }

    /// **The frame ended**: a whole one forgets every part it did not draw,
    /// so a part that closes and opens again comes in again; a frame of the
    /// live layers alone drew none of them and forgets nothing.
    pub fn end(&mut self, whole: bool) {
        if self.on && whole {
            self.parts.get_mut().retain(|p| p.2);
        }
    }

    /// **Whether a part is still coming in at `now_ms`**: what keeps a
    /// front drawing frames, and nothing else does.
    pub fn moving(&self, now_ms: f64) -> bool {
        self.on
            && self
                .parts
                .borrow()
                .iter()
                .any(|(_, since, _)| now_ms - since < ENTER_MS)
    }
}

/// **The design system's `ease`** (a cubic Bezier through `(0.25, 0.1)` and
/// `(0.25, 1)`), at `t` of the way through.
pub fn ease(t: f32) -> f32 {
    const X1: f32 = 0.25;
    const Y1: f32 = 0.1;
    const X2: f32 = 0.25;
    const Y2: f32 = 1.0;
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let bezier = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    // x grows with s on this curve, so halving finds the s whose x is t
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if bezier(X1, X2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(Y1, Y2, 0.5 * (lo + hi))
}

/// **How a part standing in `rect` is moved at `t`** of its way in, when it
/// opens off `from` (what it is the list, the tip or the card of): grown
/// toward its full size from the edge facing `from`, and slid toward its
/// place from that side by `slide`. A part that stands over what it opens
/// off (a dialog, a list standing on its own) grows from its middle and
/// does not slide.
pub fn motion(t: f32, rect: Rect, from: Option<Rect>, slide: f32) -> Motion {
    if t >= 1.0 {
        return Motion::REST;
    }
    let (cx, cy) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    let near = |v: f32, lo: f32, len: f32| v.clamp(lo, lo + len);
    const SLACK: f32 = 1.0;
    let (origin, toward) = match from {
        Some(f) if rect.y >= f.y + f.h - SLACK => {
            ([near(f.x + f.w * 0.5, rect.x, rect.w), rect.y], [0.0, -1.0])
        }
        Some(f) if rect.y + rect.h <= f.y + SLACK => (
            [near(f.x + f.w * 0.5, rect.x, rect.w), rect.y + rect.h],
            [0.0, 1.0],
        ),
        Some(f) if rect.x >= f.x + f.w - SLACK => {
            ([rect.x, near(f.y + f.h * 0.5, rect.y, rect.h)], [-1.0, 0.0])
        }
        Some(f) if rect.x + rect.w <= f.x + SLACK => (
            [rect.x + rect.w, near(f.y + f.h * 0.5, rect.y, rect.h)],
            [1.0, 0.0],
        ),
        _ => ([cx, cy], [0.0, 0.0]),
    };
    let left = 1.0 - t;
    Motion {
        origin,
        scale: FROM_SCALE + (1.0 - FROM_SCALE) * t,
        shift: [toward[0] * slide * left, toward[1] * slide * left],
        alpha: t,
    }
}

/// **A part's coming in, around its drawing**: takes the batch's mark,
/// lets `draw` draw the part where it stands and returns where it stands,
/// then moves what it drew by how far in `part` is.
pub fn enter(
    mesh: &mut Mesh,
    entrance: Option<&Entrance>,
    part: Part,
    from: Option<Rect>,
    m: &Metrics,
    draw: impl FnOnce(&mut Mesh) -> Option<Rect>,
) {
    let mark: Mark = mesh.mark();
    let rect = draw(mesh);
    let (Some(entrance), Some(rect)) = (entrance, rect) else {
        return;
    };
    let t = entrance.progress(&part);
    mesh.move_since(mark, motion(t, rect, from, m.motion_slide));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ease_runs_from_rest_to_rest_and_never_back() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        let mut last = 0.0;
        for i in 1..100 {
            let y = ease(i as f32 / 100.0);
            assert!(y >= last, "{i}");
            last = y;
        }
        // the design system's ease is well past half way at half the time
        assert!(ease(0.5) > 0.75 && ease(0.5) < 0.85, "{}", ease(0.5));
    }

    /// A list under what opened it grows from its top edge and comes down
    /// the last of the slide; a submenu at the right comes in from its
    /// left; a dialog grows from its middle and does not slide.
    #[test]
    fn a_part_comes_in_from_the_side_of_what_it_opens_off() {
        let title = Rect::new(10.0, 0.0, 40.0, 20.0);
        let list = Rect::new(10.0, 20.0, 200.0, 100.0);
        let at = motion(0.0, list, Some(title), 8.0);
        assert_eq!(at.origin, [30.0, 20.0]);
        assert_eq!(at.shift, [0.0, -8.0]);
        assert_eq!(at.alpha, 0.0);
        assert!((at.scale - FROM_SCALE).abs() < 1e-6);
        let row = Rect::new(10.0, 40.0, 200.0, 20.0);
        let sub = Rect::new(210.0, 36.0, 160.0, 80.0);
        let at = motion(0.5, sub, Some(row), 8.0);
        assert_eq!(at.origin, [210.0, 50.0]);
        assert_eq!(at.shift, [-4.0, 0.0]);
        let above = Rect::new(10.0, -60.0, 100.0, 50.0);
        assert_eq!(motion(0.0, above, Some(title), 8.0).shift, [0.0, 8.0]);
        let dialog = Rect::new(100.0, 100.0, 200.0, 100.0);
        let at = motion(0.0, dialog, None, 8.0);
        assert_eq!(at.origin, [200.0, 150.0]);
        assert_eq!(at.shift, [0.0, 0.0]);
        assert_eq!(motion(1.0, dialog, None, 8.0), Motion::REST);
    }

    /// A part comes in over its first hundred milliseconds and then stands;
    /// one a whole frame did not draw is forgotten and comes in again; a
    /// frame of the live layers forgets nothing; off, everything stands
    /// and nothing is kept moving.
    #[test]
    fn a_part_comes_in_once_and_again_after_it_closes() {
        let mut e = Entrance::default();
        let menu = Part::Tip(7);
        e.begin(1000.0, true);
        assert_eq!(e.progress(&menu), 0.0);
        e.end(true);
        assert!(e.moving(1000.0));
        e.begin(1050.0, true);
        let half = e.progress(&menu);
        assert!(half > 0.5 && half < 1.0, "{half}");
        e.end(true);
        e.begin(1100.0, true);
        assert_eq!(e.progress(&menu), 1.0);
        e.end(true);
        assert!(!e.moving(1100.0), "at rest no frame is asked for");
        // a live frame draws no part and forgets none
        e.begin(1200.0, true);
        e.end(false);
        e.begin(1210.0, true);
        assert_eq!(e.progress(&menu), 1.0);
        e.end(true);
        // a whole frame without it: it closed, and comes in again
        e.begin(1300.0, true);
        e.end(true);
        e.begin(1400.0, true);
        assert_eq!(e.progress(&menu), 0.0);
        e.end(true);
        // off: standing, and nothing moves
        e.begin(1410.0, false);
        assert_eq!(e.progress(&Part::Dialog(3)), 1.0);
        e.end(true);
        assert!(!e.moving(1410.0));
    }
}
