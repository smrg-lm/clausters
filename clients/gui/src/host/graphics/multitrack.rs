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

use crate::host::layout::Rect;
use crate::host::placement::Placement;
use crate::viewport::View;

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
}

impl Clip {
    /// A clip on `lane`, placed.
    pub fn new(name: impl Into<String>, lane: impl Into<String>, place: Placement) -> Self {
        Self {
            name: name.into(),
            lane: lane.into(),
            place,
            label: String::new(),
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

/// How tall the stack is with `gap` between lanes — a scroll's content height,
/// and what says whether it scrolls at all.
pub fn content_height(lanes: &[Lane], gap: f32) -> f32 {
    if lanes.is_empty() {
        return 0.0;
    }
    let thick: f32 = lanes.iter().map(|l| l.height).sum();
    thick + gap * (lanes.len() - 1) as f32
}

/// Where each lane lands inside `rect`, scrolled down by `scroll` pixels.
///
/// One entry per lane, in stacking order, **including the ones off the top or
/// the bottom** — a caller that draws skips what does not intersect, and a
/// caller that hit-tests needs the same rects the drawing used or the two
/// disagree in exactly the cases nobody tests.
pub fn stack(lanes: &[Lane], rect: Rect, scroll: f32, gap: f32) -> Vec<Rect> {
    let mut out = Vec::with_capacity(lanes.len());
    let mut y = rect.y - scroll;
    for lane in lanes {
        out.push(Rect::new(rect.x, y, rect.w, lane.height));
        y += lane.height + gap;
    }
    out
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

/// The `lanes` wire form: the flat `name label height mute solo gain` sextuple
/// array.
///
/// The inverse of the prop's parse, so what a `/gui_query` reports is what a
/// `/gui_set` would take — the contract every non-scalar on this wire keeps.
pub fn lanes_json(lanes: &[Lane]) -> Value {
    let mut out = Vec::with_capacity(lanes.len() * 6);
    for l in lanes {
        out.push(Value::from(l.name.clone()));
        out.push(Value::from(l.label.clone()));
        out.push(Value::from(l.height));
        out.push(Value::from(i64::from(l.mute)));
        out.push(Value::from(i64::from(l.solo)));
        out.push(Value::from(l.gain));
    }
    Value::Array(out)
}

/// The `clips` wire form: the flat `name lane offset dur start label` sextuple
/// array.
pub fn clips_json(clips: &[Clip]) -> Value {
    let mut out = Vec::with_capacity(clips.len() * 6);
    for c in clips {
        out.push(Value::from(c.name.clone()));
        out.push(Value::from(c.lane.clone()));
        out.push(Value::from(c.place.offset));
        out.push(Value::from(c.place.dur));
        out.push(Value::from(c.place.start));
        out.push(Value::from(c.label.clone()));
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
        let at = stack(&lanes, rect, 0.0, 4.0);
        assert_eq!(at.len(), 2);
        assert_eq!((at[0].y, at[0].h), (20.0, 100.0));
        assert_eq!((at[1].y, at[1].h), (124.0, 60.0)); // 20 + 100 + 4
        assert_eq!(at[0].x, 10.0);

        let scrolled = stack(&lanes, rect, 30.0, 4.0);
        assert_eq!(scrolled[0].y, -10.0, "a lane off the top is still reported");
        assert_eq!(scrolled[1].y, 94.0);

        // The content height is what says whether it scrolls at all.
        assert_eq!(content_height(&lanes, 4.0), 164.0);
        assert_eq!(content_height(&[], 4.0), 0.0);
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
        assert_eq!(written.len(), 12, "six per lane");
        assert_eq!(written[0], Value::from("noise"));
        assert_eq!(written[3], Value::from(1), "muted");
        assert_eq!(written[5], Value::from(0.5));

        let Value::Array(written) = clips_json(&clips()) else {
            panic!("an array");
        };
        assert_eq!(written.len(), 12, "six per clip");
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
