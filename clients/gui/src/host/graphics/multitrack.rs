//! What a `multitrack` owns: its lanes, its clips, and where they land.
//!
//! This is the model half of the multitrack widget ([`crate::host::elements`]),
//! and the thing that tells it from [`super::track`]: `track` draws a *widget
//! tree* — a `Track` container holding `Clip` children, one widget per box —
//! while this holds the multitrack as **data** the way [`super::pianoroll`]
//! holds a roll's notes. A lane is a row of this structure, not a widget, so it
//! cannot sit in a void and there is exactly one thing that owns it.
//!
//! That is the whole reason the type exists. With the lanes spread over N
//! widgets there was nobody to report *the multitrack*, so a gesture reported
//! what the hand did to whichever widget it touched — under one of three tags,
//! chosen by the gesture rather than by the clip, which two independent readers
//! got wrong (`clients/gui/PLAN.md`, `G34`). One owner answers with the result
//! instead, exactly as a roll answers with its notes.
//!
//! Pure over geometry and numbers: no `Host`, no props map, no wire. What a
//! *mixer* makes of a lane's mute, solo and gain is not decided here either —
//! that is the client's rule, as it is the document's (`clausters-document`
//! says a solo is the mixer's rule and not the model's). This carries the
//! numbers and places the boxes.

use crate::host::bands::Bands;
use crate::host::layout::Rect;
use crate::host::structures::clips::{Clip, Curve, Lane};
use crate::viewport::View;

/// Where a clip's box lands on `body` under `nav`, or `None` when it falls
/// entirely outside the window.
///
/// `min_w` is what keeps a very short clip from vanishing at a wide zoom: a box
/// narrower than that is drawn at that width, because a clip nobody can see is
/// a clip nobody can grab.
pub fn clip_x(clip: &Clip, body: Rect, nav: &View, min_w: f32) -> Option<(f32, f32)> {
    super::track::clip_x_range(body, nav, clip.place.offset, clip.place.dur, min_w)
}

/// What one row of the stack is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// A lane, by index into the lanes.
    Lane(usize),
    /// A track automation drawn under that lane, by index into the curves.
    Curve(usize),
}

/// **The stack as rows**: every lane, each followed by the automation rows that
/// name it.
///
/// The vertical axis of a multitrack was the lanes and is now the rows, because
/// a track automation is a row of its own — a lane of curve under the lane of
/// boxes, spanning the whole timeline the way the track does. Everything that
/// reads the vertical axis reads it here, so the drawing and the hit test
/// cannot disagree about where a row begins.
///
/// A curve naming a lane that is not here is **kept and drawn nowhere**, the
/// rule a clip already keeps: what cannot be placed can still be reported.
#[derive(Debug, Clone)]
pub struct Stack {
    /// One entry per row, top to bottom: what it is and how tall it is.
    entries: Vec<(Row, f32)>,
    gap: f32,
}

impl Stack {
    /// The rows the lanes and the curves make, in the order they are drawn.
    ///
    /// **Every curve in the list takes a row**; a caller that hides some says
    /// so with [`Stack::shown`].
    pub fn new(lanes: &[Lane], curves: &[Curve], gap: f32) -> Stack {
        Self::shown(lanes, curves, gap, |_| true)
    }

    /// The rows, with the curves `shown` answers `false` for **left out
    /// altogether** *(found 2026-09-12 by the user: "la A sigue sin ocultar ni
    /// mostrar")*.
    ///
    /// A hidden curve is not a curve drawn as nothing — it is a row that is not
    /// there. Reserving its band and skipping the drawing leaves a hole exactly
    /// where the row was, which is a picture that does not change when a hand
    /// hides one and does not change when it shows one either: the same gap,
    /// with or without a line in it. The indices `Row::Curve` carries are still
    /// **into the whole list**, so a caller looks a row up the way it always
    /// did.
    pub fn shown(
        lanes: &[Lane],
        curves: &[Curve],
        gap: f32,
        shown: impl Fn(&Curve) -> bool,
    ) -> Stack {
        let mut entries = Vec::with_capacity(lanes.len() + curves.len());
        for (i, lane) in lanes.iter().enumerate() {
            entries.push((Row::Lane(i), lane.height));
            for (n, curve) in curves.iter().enumerate() {
                if curve.owner == lane.name && shown(curve) {
                    entries.push((Row::Curve(n), curve.height));
                }
            }
        }
        Stack { entries, gap }
    }

    /// The bands the rows make, each carrying its own gap.
    ///
    /// The gap is *inside* the band rather than between two of them, and that
    /// is the whole of how a stack has no holes: a **gap belongs to the row
    /// above it**, so a pointer between two rows is on one rather than on
    /// nothing (`gestures/nav.rs` states the same rule for the widget-tree
    /// stack). A hit test that answers "nowhere" there is what makes a dragged
    /// clip snap back for those frames and jump again on the far side.
    ///
    /// [`Bands`] is the shared vertical axis a roll's semitone rows use, which
    /// is why the clamping past either end comes with it rather than being
    /// written again here.
    fn bands(&self) -> Bands {
        Bands::table(self.entries.iter().map(|(_, h)| h + self.gap))
    }

    /// How many rows there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// What row `i` is.
    pub fn row(&self, i: usize) -> Option<Row> {
        self.entries.get(i).map(|(r, _)| *r)
    }

    /// How tall the stack is — a scroll's content height, and what says whether
    /// it scrolls at all. The last band's trailing gap is not counted: it is
    /// the room a drop below the stack lands in, not room the stack occupies.
    pub fn content_height(&self) -> f32 {
        if self.entries.is_empty() {
            return 0.0;
        }
        self.bands().total() - self.gap
    }

    /// Where each row lands inside `rect`, scrolled down by `scroll` pixels —
    /// **including the ones off either end**, so a caller that hit-tests reads
    /// the same rects the drawing used.
    pub fn rects(&self, rect: Rect, scroll: f32) -> Vec<Rect> {
        let bands = self.bands();
        self.entries
            .iter()
            .enumerate()
            .map(|(i, (_, h))| {
                let (y, _) = bands.band(i);
                Rect::new(rect.x, rect.y - scroll + y, rect.w, *h)
            })
            .collect()
    }

    /// The row a pointer is **on**, or `None` off the stack entirely — the
    /// *press*' question.
    pub fn row_at(&self, rect: Rect, scroll: f32, y: f64) -> Option<usize> {
        self.bands().index_at(y as f32 - rect.y + scroll)
    }

    /// The **lane** a pointer is on, or `None` off the stack and `None` on an
    /// automation row: nothing of a lane is drawn there, so a press that lands
    /// on one is not a press on the lane above it.
    pub fn lane_at(&self, rect: Rect, scroll: f32, y: f64) -> Option<usize> {
        match self.row(self.row_at(rect, scroll, y)?)? {
            Row::Lane(i) => Some(i),
            Row::Curve(_) => None,
        }
    }

    /// The lane a hand **is heading for**, always: the nearest row's lane,
    /// clamped to the stack at both ends.
    ///
    /// A drag has to answer for every pixel the pointer crosses — the gaps, the
    /// automation rows, the space past either end. An automation row answers
    /// with the lane it belongs to, which is the only lane a clip dropped there
    /// could sensibly mean.
    pub fn lane_toward(&self, rect: Rect, scroll: f32, y: f64) -> usize {
        if self.entries.is_empty() {
            return 0;
        }
        let at = self.bands().index_of(y as f32 - rect.y + scroll);
        let i = (at.floor() as usize).min(self.entries.len() - 1);
        // Walk back to the lane that owns the row: the entries are built lane
        // first, so there is always one at or above any curve row.
        self.entries[..=i]
            .iter()
            .rev()
            .find_map(|(r, _)| match r {
                Row::Lane(n) => Some(*n),
                Row::Curve(_) => None,
            })
            .unwrap_or(0)
    }

    /// Where each **lane** lands, by lane index — what places the clips.
    pub fn lane_rects(&self, rect: Rect, scroll: f32, lanes: usize) -> Vec<Rect> {
        let rects = self.rects(rect, scroll);
        let mut out = vec![Rect::new(rect.x, rect.y, rect.w, 0.0); lanes];
        for (i, (row, _)) in self.entries.iter().enumerate() {
            if let Row::Lane(n) = row
                && let Some(slot) = out.get_mut(*n)
            {
                *slot = rects[i];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lanes() -> Vec<Lane> {
        vec![Lane::new("noise", 100.0), Lane::new("tone", 60.0)]
    }

    /// The stack is the lanes at their own thicknesses, and a scroll moves all
    /// of them by one number.
    #[test]
    fn the_stack_lays_the_lanes_at_their_own_heights() {
        let lanes = lanes();
        let rect = Rect::new(10.0, 20.0, 400.0, 300.0);
        let stack = Stack::new(&lanes, &[], 4.0);
        let at = stack.rects(rect, 0.0);
        assert_eq!(at.len(), 2);
        assert_eq!((at[0].y, at[0].h), (20.0, 100.0));
        assert_eq!((at[1].y, at[1].h), (124.0, 60.0)); // 20 + 100 + 4
        assert_eq!(at[0].x, 10.0);

        let scrolled = stack.rects(rect, 30.0);
        assert_eq!(scrolled[0].y, -10.0, "a lane off the top is still reported");
        assert_eq!(scrolled[1].y, 94.0);

        // The content height is what says whether it scrolls at all.
        assert_eq!(stack.content_height(), 164.0);
        assert_eq!(Stack::new(&[], &[], 4.0).content_height(), 0.0);
    }

    fn curve(name: &str, owner: &str, height: f32) -> Curve {
        Curve {
            name: name.into(),
            owner: owner.into(),
            label: String::new(),
            min: 0.0,
            max: 1.0,
            height,
        }
    }

    /// **A track automation is a row of its own, under the lane it names** —
    /// not a layer on it and not a lane of clips. So the vertical axis is the
    /// rows, and the lane below an automation is where the automation left it.
    #[test]
    fn an_automation_row_sits_under_its_lane_and_pushes_the_next_one_down() {
        let lanes = lanes();
        let curves = vec![curve("gain", "noise", 40.0)];
        let rect = Rect::new(0.0, 0.0, 400.0, 300.0);
        let stack = Stack::new(&lanes, &curves, 4.0);
        assert_eq!(stack.len(), 3);
        assert_eq!(stack.row(1), Some(Row::Curve(0)));

        let at = stack.rects(rect, 0.0);
        assert_eq!((at[1].y, at[1].h), (104.0, 40.0));
        assert_eq!(at[2].y, 148.0, "the second lane is below the row");

        // The lanes are still addressed by lane index.
        let lane_rects = stack.lane_rects(rect, 0.0, lanes.len());
        assert_eq!(lane_rects[1].y, 148.0);
    }

    /// **A press on an automation row is not a press on a lane** — nothing of
    /// a lane is drawn there — but a *drag* still has to answer, and it answers
    /// with the lane the row belongs to.
    #[test]
    fn a_curve_row_answers_a_drag_and_not_a_press() {
        let lanes = lanes();
        let curves = vec![curve("gain", "noise", 40.0)];
        let rect = Rect::new(0.0, 0.0, 400.0, 300.0);
        let stack = Stack::new(&lanes, &curves, 4.0);
        assert_eq!(stack.lane_at(rect, 0.0, 50.0), Some(0));
        assert_eq!(stack.lane_at(rect, 0.0, 120.0), None, "an automation row");
        assert_eq!(stack.lane_toward(rect, 0.0, 120.0), 0);
        assert_eq!(stack.lane_toward(rect, 0.0, 160.0), 1);
        assert_eq!(stack.lane_toward(rect, 0.0, 9_000.0), 1, "clamped");
    }

    /// A curve naming a lane that is not here is kept and drawn nowhere, the
    /// rule a clip already keeps.
    #[test]
    fn a_curve_naming_no_lane_takes_no_row() {
        let stack = Stack::new(&lanes(), &[curve("gain", "vanished", 40.0)], 4.0);
        assert_eq!(stack.len(), 2);
    }
}
