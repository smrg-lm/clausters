//! What a `multitrack` owns: its lanes, its clips, and where they land.
//!
//! This is the model half of the multitrack widget ([`crate::host::elements`]),
//! and the thing that tells it from [`super::track`]: `track` draws a *widget
//! tree* — a `Track` container holding `Clip` children, one widget per box —
//! while this holds the piece as **data** the way [`super::pianoroll`]
//! holds a roll's notes. A lane is a row of this structure, not a widget, so it
//! cannot sit in a void and there is exactly one thing that owns it.
//!
//! That is the whole reason the type exists. With the lanes spread over N
//! widgets there was nobody to report *the piece*, so a gesture reported
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

use serde_json::Value;

use crate::host::bands::Bands;
use crate::host::layout::Rect;
use crate::host::placement::Placement;
use crate::viewport::View;

/// What a clip's `source` says when it is a window onto nothing.
pub const NO_SOURCE: i32 = -1;

/// One row of the multitrack: a track's lane, named and sized.
///
/// The name is the **identity** and it is the client's own word, not a widget
/// id. That is what lets an edit-back name a lane the script already knows,
/// rather than an address the script has to keep a table for.
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    /// Its identity, and the client's own name for it.
    pub name: String,
    /// What is drawn in the header. Empty draws the name.
    pub label: String,
    /// How thick the lane is, in logical pixels.
    pub height: f32,
    /// Silenced. Carried, never interpreted.
    pub mute: bool,
    /// Soloed. Carried, never interpreted — what a solo does to *other* lanes
    /// is the mixer's rule and the client's.
    pub solo: bool,
    /// The fader, over `[0, 1]`.
    pub gain: f32,
    /// **Whether this track's automation rows are shown.**
    ///
    /// Carried, never interpreted: which rows there are is the `curves` prop's
    /// and what a hidden one means is the `hidden` set's. This is the one
    /// statement a *track* makes about them — the header's toggle — and it is a
    /// field of the lane because that is the row the toggle is drawn on.
    pub curves: bool,
}

impl Lane {
    /// A lane of this name, at the default thickness the caller gives.
    pub fn new(name: impl Into<String>, height: f32) -> Self {
        Self {
            name: name.into(),
            label: String::new(),
            height,
            mute: false,
            solo: false,
            gain: 1.0,
            // **Shown unless something says otherwise.** A row a piece drew is
            // a row a piece meant to be seen, and whether a *curve* is drawn is
            // the `hidden` set's answer, not this one's.
            curves: true,
        }
    }

    /// What the header shows: the label when it has one, else the name.
    pub fn shown(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }
}

/// One placed box: which lane it is on, and where it sits there.
///
/// The placement is [`Placement`] — the same `offset`/`dur`/`start` triple every
/// box in this crate is measured by, so a clip and a note are placed by one
/// type and a drag over either is the same arithmetic.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// Its identity, and the client's own name for it.
    pub name: String,
    /// The lane it is on, by that lane's name.
    pub lane: String,
    /// Where it sits and how long it lasts, in timeline sample units.
    pub place: Placement,
    /// What is drawn on it. Empty draws the name.
    pub label: String,
    /// **The server buffer this box is a window onto**, or a **negative** number
    /// for a box with no contents — a placeholder, a region of something the
    /// host cannot draw.
    ///
    /// Negative and not zero, because **buffer 0 is a buffer**: it is the first
    /// one an allocator hands out, so a zero sentinel makes the first take a
    /// script loads the one take it cannot draw. The wire spells "none" with a
    /// negative number everywhere it has to (`playhead_at`, the transport's
    /// group), and this is the same word.
    ///
    /// A number and not a payload: the samples are the *server's*, and the host
    /// either maps them out of the shared segment or fetches them over the leg.
    /// So two clips over one take cost one download and one pyramid, which is
    /// what makes a piece of six views of one recording cheap.
    pub source: i32,
}

impl Clip {
    /// A clip on `lane`, placed.
    pub fn new(name: impl Into<String>, lane: impl Into<String>, place: Placement) -> Self {
        Self {
            name: name.into(),
            lane: lane.into(),
            place,
            label: String::new(),
            source: NO_SOURCE,
        }
    }

    /// What the box shows: the label when it has one, else the name.
    pub fn shown(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }

    /// Where it ends on the timeline.
    pub fn end(&self) -> f64 {
        self.place.offset + self.place.dur
    }
}

/// Where the piece ends: the furthest clip end, `0` when there are none.
///
/// The **end**, not the last onset — a clip dragged past everything else
/// lengthens the piece by its whole length, which is the number a ruler and a
/// scroll have to size themselves against.
pub fn extent(clips: &[Clip]) -> f64 {
    clips.iter().map(Clip::end).fold(0.0, f64::max)
}

/// The clips on `lane`, in the order they are held — which is the order they
/// draw in, so two that overlap stack predictably.
pub fn clips_on<'a>(clips: &'a [Clip], lane: &'a str) -> impl Iterator<Item = &'a Clip> + 'a {
    clips.iter().filter(move |c| c.lane == lane)
}

/// Where a clip's box lands on `body` under `nav`, or `None` when it falls
/// entirely outside the window.
///
/// `min_w` is what keeps a very short clip from vanishing at a wide zoom: a box
/// narrower than that is drawn at that width, because a clip nobody can see is
/// a clip nobody can grab.
pub fn clip_x(clip: &Clip, body: Rect, nav: &View, min_w: f32) -> Option<(f32, f32)> {
    super::track::clip_x_range(body, nav, clip.place.offset, clip.place.dur, min_w)
}

/// A break-point automation, and **the same element in two places**.
///
/// A curve that names a **lane** is a track automation: a row of its own under
/// that lane, as tall as it asks and as long as the timeline — a track's gain
/// does not begin and end with a box. A curve that names a **box** is a clip
/// envelope: a layer drawn inside that box's rectangle, over whatever the box
/// draws, and lasting exactly as long as the box does.
///
/// The distinction is where it hangs and nothing else. Both are drawn by the
/// `curve` element in its body form, both take the same break-points, and both
/// report through the same `"points"` payload — which is why they are one type
/// with two lists rather than two types.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    /// Its identity, the client's own name — what a point names to say which
    /// curve it is on, and what a report names it back with.
    pub name: String,
    /// The lane this is a row under, or the box this is a layer on.
    pub owner: String,
    /// What is written on it; the name is drawn when this is empty.
    pub label: String,
    /// The value domain the points are read and drawn over.
    pub min: f32,
    pub max: f32,
    /// How tall its row is — a **row's** only; a layer is as tall as the box
    /// it is drawn on.
    pub height: f32,
}

impl Curve {
    /// What is drawn on it: its label, or its name when it carries none.
    pub fn shown(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }
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

/// The `curves` wire form: the flat `name lane label min max height` sextuple
/// array. The inverse of the prop's parse, as every non-scalar here is.
pub fn curves_json(curves: &[Curve]) -> Value {
    let mut out = Vec::with_capacity(curves.len() * 6);
    for c in curves {
        out.push(Value::from(c.name.clone()));
        out.push(Value::from(c.owner.clone()));
        out.push(Value::from(c.label.clone()));
        out.push(Value::from(c.min));
        out.push(Value::from(c.max));
        out.push(Value::from(c.height));
    }
    Value::Array(out)
}

/// The `layers` wire form: the flat `name box label min max` quintuple array —
/// a layer has no height of its own, since it is as tall as the box it is on.
pub fn layers_json(layers: &[Curve]) -> Value {
    let mut out = Vec::with_capacity(layers.len() * 5);
    for c in layers {
        out.push(Value::from(c.name.clone()));
        out.push(Value::from(c.owner.clone()));
        out.push(Value::from(c.label.clone()));
        out.push(Value::from(c.min));
        out.push(Value::from(c.max));
    }
    Value::Array(out)
}

/// The `lanes` wire form: the flat `name label height mute solo gain` sextuple
/// array.
///
/// The inverse of the prop's parse, so what a `/gui_query` reports is what a
/// `/gui_set` would take — the contract every non-scalar on this wire keeps.
pub fn lanes_json(lanes: &[Lane]) -> Value {
    let mut out = Vec::with_capacity(lanes.len() * 7);
    for l in lanes {
        out.push(Value::from(l.name.clone()));
        out.push(Value::from(l.label.clone()));
        out.push(Value::from(l.height));
        out.push(Value::from(i64::from(l.mute)));
        out.push(Value::from(i64::from(l.solo)));
        out.push(Value::from(l.gain));
        out.push(Value::from(i64::from(l.curves)));
    }
    Value::Array(out)
}

/// The `clips` wire form: the flat `name lane offset dur start label source`
/// septuple array.
pub fn clips_json(clips: &[Clip]) -> Value {
    let mut out = Vec::with_capacity(clips.len() * 7);
    for c in clips {
        out.push(Value::from(c.name.clone()));
        out.push(Value::from(c.lane.clone()));
        out.push(Value::from(c.place.offset));
        out.push(Value::from(c.place.dur));
        out.push(Value::from(c.place.start));
        out.push(Value::from(c.label.clone()));
        out.push(Value::from(i64::from(c.source)));
    }
    Value::Array(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lanes() -> Vec<Lane> {
        vec![Lane::new("noise", 100.0), Lane::new("tone", 60.0)]
    }

    fn clips() -> Vec<Clip> {
        let at = |offset, dur| Placement {
            offset,
            dur,
            start: 0.0,
        };
        vec![
            Clip::new("a", "noise", at(0.0, 48_000.0)),
            Clip::new("b", "tone", at(96_000.0, 48_000.0)),
        ]
    }

    /// **A clip names its lane, it is not nested inside one.** That is what
    /// makes a lane change one field of one clip rather than a removal and an
    /// insertion — the edit that had no way to report itself before.
    #[test]
    fn a_clip_changes_lane_by_naming_another_one() {
        let mut clips = clips();
        clips[1].lane = "noise".into();
        assert_eq!(clips_on(&clips, "noise").count(), 2);
        assert_eq!(clips_on(&clips, "tone").count(), 0);
        // And nothing else moved: the placement is the clip's own.
        assert_eq!(clips[1].place.offset, 96_000.0);
    }

    /// **The extent is where the last clip ends**, not where it starts: a ruler
    /// and a scroll size themselves against the piece, and a clip dragged past
    /// everything lengthens it by its whole length.
    #[test]
    fn the_extent_is_the_furthest_end_and_not_the_last_onset() {
        assert_eq!(extent(&clips()), 144_000.0);
        assert_eq!(extent(&[]), 0.0);
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

    /// The two lists report independently: a fader moved resends the lanes and
    /// not every clip, which is the whole reason they are two.
    #[test]
    fn each_list_reports_as_a_set_take_would_read_it() {
        let mut lanes = lanes();
        lanes[0].gain = 0.5;
        lanes[0].mute = true;

        let Value::Array(written) = lanes_json(&lanes) else {
            panic!("an array");
        };
        assert_eq!(written.len(), 14, "seven per lane");
        assert_eq!(written[0], Value::from("noise"));
        assert_eq!(written[3], Value::from(1), "muted");
        assert_eq!(written[5], Value::from(0.5));

        let Value::Array(written) = clips_json(&clips()) else {
            panic!("an array");
        };
        assert_eq!(written.len(), 14, "seven per clip");
        assert_eq!(written[0], Value::from("a"));
        assert_eq!(written[1], Value::from("noise"), "the lane it is on");
        assert_eq!(written[3], Value::from(48_000.0));
    }

    /// A lane's header shows its label, and falls back to the name it is
    /// addressed by rather than drawing nothing.
    #[test]
    fn a_label_is_a_second_name_and_never_the_identity() {
        let mut lane = Lane::new("noise", 100.0);
        assert_eq!(lane.shown(), "noise");
        lane.label = "Drums".into();
        assert_eq!(lane.shown(), "Drums");
        assert_eq!(lane.name, "noise", "what an edit-back still names it by");
    }
}
