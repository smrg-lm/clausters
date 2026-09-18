//! **What a multitrack is made of**: a lane, a box on it, and a curve over
//! either — and the payloads each list is reported as.
//!
//! A box here is a [`boxes::Placement`](super::boxes::Placement) plus the two
//! things a placement cannot say: **where its contents come from** (a server
//! buffer, and the window onto it the box shows) and **which lane it is on**.
//! A lane is a name, a label, a height and the mixing a hand set on it; a curve
//! is a break-point list's identity and the range it is read in, hanging either
//! on a lane (a track automation, a row of its own) or on a box (an envelope,
//! a layer inside it). The break-points themselves are
//! [`points`](super::points), because a break-point is a break-point wherever
//! the curve hangs.
//!
//! **The name is the identity**, in every one of them: the client's own word
//! and never a widget id, which is what lets a report name what the script
//! already calls them and what a correction finds a box again by. A `label` is
//! a second name and never that one.
//!
//! Nothing here draws: where a box lands on a lane and how tall a row is are
//! `graphics::multitrack`'s, and the two meet at the numbers.

use serde_json::Value;

use crate::host::structures::boxes::Placement;

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
            // **Shown unless something says otherwise.** A row a multitrack drew is
            // a row a multitrack meant to be seen, and whether a *curve* is drawn is
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
    /// what makes a multitrack of six views of one recording cheap.
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

/// Where the multitrack ends: the furthest clip end, `0` when there are none.
///
/// The **end**, not the last onset — a clip dragged past everything else
/// lengthens the multitrack by its whole length, which is the number a ruler and a
/// scroll have to size themselves against.
pub fn extent(clips: &[Clip]) -> f64 {
    clips.iter().map(Clip::end).fold(0.0, f64::max)
}

/// The clips on `lane`, in the order they are held — which is the order they
/// draw in, so two that overlap stack predictably.
pub fn clips_on<'a>(clips: &'a [Clip], lane: &'a str) -> impl Iterator<Item = &'a Clip> + 'a {
    clips.iter().filter(move |c| c.lane == lane)
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
    /// and a scroll size themselves against the multitrack, and a clip dragged past
    /// everything lengthens it by its whole length.
    #[test]
    fn the_extent_is_the_furthest_end_and_not_the_last_onset() {
        assert_eq!(extent(&clips()), 144_000.0);
        assert_eq!(extent(&[]), 0.0);
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
