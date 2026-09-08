//! The `multitrack` widget: one element that owns a stack of lanes and the
//! clips on them.
//!
//! It is the `pianoroll`'s shape applied to a piece. A roll is one widget
//! holding its notes; this is one widget holding its lanes and clips, drawing
//! its own headers, its own stack and its own boxes on the shared time axis.
//! What that replaces is a *tree* of `Track` widgets under whatever generic
//! container a script picked — a shape with nobody in it that owned the piece,
//! so a gesture had nowhere to report *the piece* and reported what the hand
//! did to whichever widget it touched. `clients/gui/PLAN.md`'s `G34` carries
//! the whole argument.
//!
//! **The lanes and the clips are props**, flat like a roll's `notes`: a client
//! describes the piece and never composes a tree of it, never registers a
//! handler per box, and never learns a widget id. Identity is the client's own
//! name, so what comes back names what the script already knows.
//!
//! **It places; a clip is entered to edit.** A clip's contents draw read-only
//! here — this widget owns *where* things are, not what is inside them — and
//! editing one is opening it in the editor its structure asks for. That is what
//! keeps a heavy widget from becoming every widget, and it is the line the three
//! applications are drawn on: the multitrack places, the audio editor and the
//! score editor edit.

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use crate::host::font;
use crate::host::graphics::multitrack::{self as model, Clip, Lane};
use crate::host::graphics::track;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::placement::{self, Bounds, Contents, Part, Placement};
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input, Take, TimeSpace};
use crate::host::widget::parse::{self, label, number, number_f64, truthy};
use crate::host::widget::size::Natural;
use crate::host::widget::{EditorProps, RulerY};
use crate::viewport::View;

/// One JSON scalar as the OSC primitive it is — **flat primitives at the
/// boundary**, which is what every edit-back payload here rides as.
fn json_arg(v: Value) -> OscType {
    match v {
        Value::String(s) => OscType::String(s),
        Value::Number(n) if n.is_i64() => OscType::Int(n.as_i64().unwrap_or(0) as i32),
        other => OscType::Float(other.as_f64().unwrap_or(0.0) as f32),
    }
}

/// The gap between lanes, in logical pixels, when the props name none.
const GAP: f32 = 4.0;

/// A lane's thickness when nothing says otherwise.
const LANE_H: f32 = 96.0;

/// The narrowest a clip's box is drawn at, so one nobody can see never becomes
/// one nobody can grab.
const MIN_CLIP_W: f32 = 3.0;

/// **What the hand took at the press, kept until it lets go.**
///
/// The snapshotted form of drag ([`crate::host::widget::element::Take`] says
/// why there are three): a press-time origin plus the axis and the grid, so a
/// clamped edge never drifts and a drag that comes back to where it began is
/// exactly where it began.
#[derive(Debug, Clone, Copy)]
struct Grab {
    /// The clip the hand has, by index.
    clip: usize,
    /// Which part of it — the body, or one of the two edges.
    part: Part,
    /// Where it sat when the press landed.
    orig: Placement,
    /// The lane it was on when the press landed, by index.
    lane: usize,
    /// The pointer's time at the press, so a body drag moves by the travel
    /// rather than by where inside the box the hand grabbed it.
    grabbed_at: f64,
}

/// The stack of lanes and the clips on them.
#[derive(Debug, Clone)]
pub struct Multitrack {
    /// The lanes, top to bottom.
    pub(crate) lanes: Vec<Lane>,
    /// The clips, each naming the lane it is on.
    pub(crate) clips: Vec<Clip>,
    /// Which clips the hand is holding, by index. **The hand's, not the
    /// piece's**: nothing on the wire sets or reports it, exactly as nothing
    /// reports which notes a roll has selected.
    pub(crate) selected: Vec<usize>,
    /// How far the stack is scrolled, in logical pixels. Screen state, so it
    /// survives a redefine rather than being restated by every def.
    pub(crate) scroll: f32,
    /// The space between lanes.
    pub(crate) gap: f32,
    /// The grid a placement lands on, in timeline samples; `0` is no grid.
    pub(crate) snap: f64,
    /// The shared axis, the selection and the playhead — the same props every
    /// timeline view carries.
    pub(crate) editor: EditorProps,
    /// A caption drawn in the corner.
    pub(crate) label: Option<String>,
    /// The drag in flight. **The state lives in the element**; the machine
    /// keeps only the sequence.
    grab: Option<Grab>,
}

impl Default for Multitrack {
    fn default() -> Self {
        Self {
            lanes: Vec::new(),
            clips: Vec::new(),
            selected: Vec::new(),
            scroll: 0.0,
            gap: GAP,
            snap: 0.0,
            editor: EditorProps::body(),
            label: None,
            grab: None,
        }
    }
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `multitrack` node carries, read once — shared by the constructor
/// and by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Multitrack {
    Multitrack {
        lanes: parse_lanes(props),
        clips: parse_clips(props),
        selected: Vec::new(),
        scroll: 0.0,
        gap: number(props, "gap", GAP).max(0.0),
        snap: number_f64(props, "snap", 0.0).max(0.0),
        editor: EditorProps::parse(props, RulerY::Off),
        label: label(props),
        grab: None,
    }
}

/// The `lanes` prop: the flat `name label height mute solo gain` sextuple array.
///
/// A trailing partial group is dropped rather than half-read, which is the rule
/// every flat payload here follows.
fn parse_lanes(props: &Map<String, Value>) -> Vec<Lane> {
    let Some(Value::Array(items)) = props.get("lanes") else {
        return Vec::new();
    };
    items
        .as_chunks::<6>()
        .0
        .iter()
        .filter_map(|c| {
            Some(Lane {
                name: c[0].as_str()?.to_string(),
                label: c[1].as_str().unwrap_or_default().to_string(),
                height: c[2].as_f64().unwrap_or(f64::from(LANE_H)) as f32,
                mute: truthy(&c[3]).unwrap_or(false),
                solo: truthy(&c[4]).unwrap_or(false),
                gain: c[5].as_f64().unwrap_or(1.0) as f32,
            })
        })
        .collect()
}

/// The `clips` prop: the flat `name lane offset dur start label` sextuple array.
fn parse_clips(props: &Map<String, Value>) -> Vec<Clip> {
    let Some(Value::Array(items)) = props.get("clips") else {
        return Vec::new();
    };
    items
        .as_chunks::<6>()
        .0
        .iter()
        .filter_map(|c| {
            Some(Clip {
                name: c[0].as_str()?.to_string(),
                lane: c[1].as_str().unwrap_or_default().to_string(),
                place: Placement {
                    offset: c[2].as_f64().unwrap_or(0.0).max(0.0),
                    dur: c[3].as_f64().unwrap_or(0.0).max(0.0),
                    start: c[4].as_f64().unwrap_or(0.0),
                },
                label: c[5].as_str().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

impl Multitrack {
    /// The axis this draws against: the navigation group's window when it is on
    /// one, else its own whole extent.
    fn view(&self, time: Option<TimeSpace>) -> View {
        match time {
            Some(t) => t.view,
            None => View::full(model::extent(&self.clips).ceil().max(1.0) as usize),
        }
    }

    /// The lane a clip sits on, by index — `None` for a clip naming a lane that
    /// is not here.
    ///
    /// **A clip is kept rather than dropped**, because what cannot be placed can
    /// still be reported: a script that renamed a lane gets its clips back to
    /// re-home rather than silently losing them.
    fn lane_of(&self, clip: &Clip) -> Option<usize> {
        self.lanes.iter().position(|l| l.name == clip.lane)
    }

    /// The lane whose row `y` fell in, by index — the same rects the drawing
    /// used, because a hit test that measured its own would catch a lane the
    /// eye does not see there.
    fn lane_at(&self, rect: Rect, y: f64) -> Option<usize> {
        model::stack(&self.lanes, rect, self.scroll, self.gap)
            .iter()
            .position(|r| y as f32 >= r.y && (y as f32) < r.y + r.h)
    }

    /// The clip under `(x, y)`, and which part of it — **the topmost first**,
    /// since a later clip is drawn over an earlier one and the eye takes the
    /// one it can see.
    fn clip_at(&self, input: &Input, at: (f64, f64)) -> Option<(usize, Part)> {
        let i = self.lane_at(input.rect, at.1)?;
        let rect = model::stack(&self.lanes, input.rect, self.scroll, self.gap)[i];
        let body = track::lane_body(rect, false, input.indent, input.metrics);
        let nav = self.view(input.time);
        self.clips
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, c)| c.lane == self.lanes[i].name)
            .find_map(|(n, c)| {
                let (x0, x1) = model::clip_x(c, body, &nav, MIN_CLIP_W)?;
                let inside = at.0 as f32 >= x0 && at.0 as f32 <= x1;
                inside.then(|| (n, placement::part_at(x0, x1, at.0 as f32)))
            })
    }

    /// The time a pointer x names on the shared axis.
    fn time_at(&self, input: &Input, x: f64) -> f64 {
        let nav = self.view(input.time);
        let body = track::lane_body(input.rect, false, input.indent, input.metrics);
        if body.w <= 0.0 {
            return nav.start;
        }
        nav.start + (x - f64::from(body.x)) / f64::from(body.w) * nav.len
    }

    /// What bounds a clip's drag here: the lane's grid, and a floor no shorter
    /// than a box a hand can still find.
    fn bounds(&self) -> Bounds {
        Bounds {
            grid: self.snap,
            ..Bounds::default()
        }
    }

    /// **The edit-back: the clips as they now are.** One payload for every
    /// gesture there is — a move, a trim, a lane crossed, a block — because
    /// what is reported is the piece and not what the hand did to it.
    fn clips_event(&self) -> Events {
        let mut args = vec![OscType::String("clips".into())];
        if let Value::Array(flat) = model::clips_json(&self.clips) {
            args.extend(flat.into_iter().map(json_arg));
        }
        Events::message(args)
    }

    /// The lane header a lane's own props ask for. Presence-driven, like every
    /// header here: a lane that carries no mixer state offers no controls.
    fn header(&self, lane: &Lane, indent: f32) -> track::Header {
        track::Header {
            w: (indent > 0.0).then_some(indent),
            mute: Some(lane.mute),
            solo: Some(lane.solo),
            level: Some(lane.gain),
        }
    }
}

impl Element for Multitrack {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // A non-scalar rides a `/gui_set` as its JSON string, the carrier
            // every structure on this wire uses.
            "lanes" => {
                self.lanes = parse_lanes(&parse::as_array_props("lanes", v));
                true
            }
            "clips" => {
                self.clips = parse_clips(&parse::as_array_props("clips", v));
                self.selected.clear();
                true
            }
            "gap" => {
                self.gap = v.as_f64().unwrap_or(f64::from(GAP)).max(0.0) as f32;
                true
            }
            "snap" => {
                self.snap = v.as_f64().unwrap_or(0.0).max(0.0);
                true
            }
            "label" => {
                self.label = v.as_str().map(str::to_string);
                true
            }
            _ => self.editor.apply(key, v),
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let nav = self.view(ctx.time);
        let at = model::stack(&self.lanes, ctx.rect, self.scroll, self.gap);
        // A lane scrolled off either end is skipped rather than drawn and
        // clipped: `stack` reports every lane so a hit test reads the same
        // rects, and the drawing is what decides it has nothing to do.
        let shown = |r: Rect| r.y + r.h >= ctx.rect.y && r.y <= ctx.rect.y + ctx.rect.h;
        for (i, lane) in self.lanes.iter().enumerate() {
            if shown(at[i]) {
                track::draw(
                    d,
                    at[i],
                    Some(lane.shown()),
                    &self.header(lane, ctx.indent),
                    false,
                    ctx.indent,
                );
            }
        }
        // **One pass over the clips, each onto the lane it names.** A clip
        // whose lane is gone draws nowhere and is still held, which is what
        // lets it come back in a report to be re-homed.
        for (n, clip) in self.clips.iter().enumerate() {
            let Some(i) = self.lane_of(clip) else {
                continue;
            };
            if !shown(at[i]) {
                continue;
            }
            let body = track::lane_body(at[i], false, ctx.indent, ctx.metrics);
            if body.w <= 0.0 || body.h <= 0.0 {
                continue;
            }
            let Some((x0, x1)) = model::clip_x(clip, body, &nav, MIN_CLIP_W) else {
                continue;
            };
            let cr = track::clip_rect(body, x0, x1);
            track::draw_clip(d, cr, self.selected.contains(&n));
            track::draw_clip_label(d, cr, clip.shown());
        }
        if let Some(text) = &self.label {
            let (mesh, m, theme) = d.parts();
            font::text(
                mesh,
                text,
                ctx.rect.x + ctx.indent + m.pad,
                ctx.rect.y + 2.0,
                m.caption_scale,
                theme.ruler_text,
            );
        }
    }

    /// **A press takes a clip, and declines everywhere else.** The slack
    /// between clips and beside them is the container's — that is where a click
    /// places the transport's cursor and a sweep starts a marquee — so a press
    /// that found no box goes back to the chain rather than being swallowed.
    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let Some((clip, part)) = self.clip_at(input, at) else {
            self.grab = None;
            return Claim::Decline;
        };
        let Some(lane) = self.lane_of(&self.clips[clip]) else {
            return Claim::Decline;
        };
        self.grab = Some(Grab {
            clip,
            part,
            orig: self.clips[clip].place,
            lane,
            grabbed_at: self.time_at(input, at.0),
        });
        Claim::Take(Take {
            // Held past the edge of the axis, the machine keeps ticking and
            // pans the group under the hand — a clip dragged off the right of
            // the window has to keep moving, and a held cursor sends nothing.
            edge_scroll: true,
            ..Take::default()
        })
    }

    /// The clip follows the hand; **the edit leaves on release.**
    ///
    /// One gesture is one edit — a placement per frame would be an undo step
    /// per frame, and a round trip whose acknowledgement the next frame
    /// outruns. What moves here is the picture.
    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        let Some(grab) = self.grab else {
            return Events::none();
        };
        let now = self.time_at(input, at.0);
        // A body is pulled by the **travel**, so the box keeps the grip the
        // hand took it by; an edge is pulled to where the pointer is.
        let target = match grab.part {
            Part::Body => grab.orig.offset + (now - grab.grabbed_at),
            Part::Start => now,
            Part::End => now,
        };
        let place = placement::drag(
            grab.part,
            target,
            grab.orig,
            Contents::default(),
            self.bounds(),
        );
        // **The vertical half of a body drag is the lane it lands on**, and it
        // is one field: the clip names another lane and nothing is removed or
        // inserted. An edge drag says nothing about which lane a clip is on.
        if grab.part == Part::Body
            && let Some(i) = self.lane_at(input.rect, at.1)
        {
            self.clips[grab.clip].lane = self.lanes[i].name.clone();
        }
        self.clips[grab.clip].place = place;
        Events::none()
    }

    /// **A gesture that changed nothing is not an edit.** A press and a release
    /// with nothing in between is a click, and a drag that came back to where
    /// it began is the same thing by another road: reporting it would hand the
    /// owner an intent to apply and a document an entry to undo, so looking at
    /// four clips would cost four undos.
    fn release(&mut self, _at: (f64, f64), _inside: bool, _input: &Input) -> Events {
        let Some(grab) = self.grab.take() else {
            return Events::none();
        };
        let moved = self.clips[grab.clip].place != grab.orig
            || self.lane_of(&self.clips[grab.clip]) != Some(grab.lane);
        if !moved {
            return Events::none();
        }
        self.clips_event()
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }

    /// **Elastic on both axes.** It is a surface whose extent is the caller's,
    /// and its own content must never size it: a lane added by a `/gui_set`
    /// would then relayout the window, which is both a visible jump and a cost
    /// per message.
    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        Natural::default()
    }

    fn editor(&self) -> Option<&EditorProps> {
        Some(&self.editor)
    }

    fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        Some(&mut self.editor)
    }

    fn navigates_time(&self) -> bool {
        true
    }

    /// How far the piece reaches on the axis — what an autofit and a scroll
    /// size themselves against.
    fn content_span(&self) -> Option<f64> {
        Some(model::extent(&self.clips))
    }

    /// What `/gui_query` overlays: the two structures, each as the string its
    /// own `/gui_set` would take.
    fn info(&self) -> Vec<(String, Value)> {
        vec![
            ("lanes".into(), model::lanes_json(&self.lanes)),
            ("clips".into(), model::clips_json(&self.clips)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::widget::element::{Mods, TimeSpace};

    /// A widget placed on a navigation group whose window is the whole piece,
    /// with a header gutter wide enough to have a body beside it.
    fn input<'a>(m: &'a Metrics, rect: Rect, len: f64) -> Input<'a> {
        Input {
            metrics: m,
            rect,
            indent: 100.0,
            scale: 1.0,
            mods: Mods::default(),
            viewport: (rect.w, rect.h),
            time: Some(TimeSpace::of(View { start: 0.0, len }, len)),
        }
    }

    /// Two lanes of 100, and a clip on each: `a` over the first half of the
    /// piece on `noise`, `b` over the second on `tone`.
    fn piece() -> Multitrack {
        from_props(&props(
            r#"{"lanes": ["noise", "", 100, 0, 0, 1, "tone", "", 100, 0, 0, 1],
                "clips": ["a", "noise", 0, 500, 0, "", "b", "tone", 500, 500, 0, ""]}"#,
        ))
    }

    /// The x a time lands at, and the y the middle of lane `i` is at.
    fn xy(mt: &Multitrack, m: &Metrics, rect: Rect, t: f64, len: f64, i: usize) -> (f64, f64) {
        let body = track::lane_body(rect, false, 100.0, m);
        let x = f64::from(body.x) + t / len * f64::from(body.w);
        let at = model::stack(&mt.lanes, rect, mt.scroll, mt.gap);
        (x, f64::from(at[i].y + at[i].h / 2.0))
    }

    fn props(json: &str) -> Map<String, Value> {
        match serde_json::from_str(json).expect("valid JSON") {
            Value::Object(m) => m,
            _ => panic!("an object"),
        }
    }

    const TWO: &str = r#"{
        "lanes": ["noise", "", 100, 0, 0, 0.8, "tone", "Lead", 60, 1, 0, 0.5],
        "clips": ["a", "noise", 0, 48000, 0, "", "b", "tone", 96000, 48000, 0, "take 2"]
    }"#;

    /// **A client describes the piece; it does not compose a tree of it.** The
    /// two structures arrive as flat arrays, like a roll's notes, and the widget
    /// is the one thing that holds them.
    #[test]
    fn the_lanes_and_the_clips_are_props_of_one_widget() {
        let mt = from_props(&props(TWO));
        assert_eq!(mt.lanes.len(), 2);
        assert_eq!(mt.clips.len(), 2);

        assert_eq!(mt.lanes[1].name, "tone");
        assert_eq!(
            mt.lanes[1].shown(),
            "Lead",
            "the label draws, the name addresses"
        );
        assert!(mt.lanes[1].mute);
        assert_eq!(mt.lanes[1].gain, 0.5);

        assert_eq!(mt.clips[1].lane, "tone");
        assert_eq!(mt.clips[1].place.offset, 96_000.0);
        assert_eq!(mt.clips[1].shown(), "take 2");
    }

    /// A trailing partial group is dropped rather than half-read, and a lane
    /// with no name is not a lane — the one field that is the identity.
    #[test]
    fn a_partial_group_is_dropped_rather_than_half_read() {
        let mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1, "two", ""],
                "clips": ["a", "one", 0, 10, 0]}"#,
        ));
        assert_eq!(mt.lanes.len(), 1, "the second group is short");
        assert!(mt.clips.is_empty(), "so is the only clip");

        let unnamed = from_props(&props(r#"{"lanes": [7, "", 100, 0, 0, 1]}"#));
        assert!(unnamed.lanes.is_empty(), "a lane with no name is not one");
    }

    /// **A `/gui_set` of a structure is the same parse**, so what a query
    /// reports is what a set would take — the round trip every non-scalar here
    /// keeps.
    #[test]
    fn a_set_reads_what_a_query_reported() {
        let mut mt = from_props(&props(TWO));
        let reported: Map<String, Value> = mt.info().into_iter().collect();

        let mut empty = Multitrack::default();
        assert!(empty.set("lanes", &reported["lanes"]));
        assert!(empty.set("clips", &reported["clips"]));
        assert_eq!(empty.lanes, mt.lanes);
        assert_eq!(empty.clips, mt.clips);

        // And the same through the string carrier a `/gui_set` actually uses.
        let as_text = Value::from(serde_json::to_string(&reported["clips"]).unwrap());
        mt.clips.clear();
        assert!(mt.set("clips", &as_text));
        assert_eq!(mt.clips.len(), 2);
    }

    /// A clip naming a lane that is not here is **kept**, not dropped: what
    /// cannot be placed can still be reported, so a script that renamed a lane
    /// gets its clips back to re-home instead of losing them.
    #[test]
    fn a_clip_on_a_lane_that_is_gone_is_kept_and_not_placed() {
        let mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1],
                "clips": ["a", "one", 0, 10, 0, "", "b", "vanished", 0, 10, 0, ""]}"#,
        ));
        assert_eq!(mt.clips.len(), 2);
        assert!(mt.lane_of(&mt.clips[0]).is_some());
        assert!(mt.lane_of(&mt.clips[1]).is_none());
        // It is still in what the widget reports, which is how it comes back.
        let Value::Array(written) = model::clips_json(&mt.clips) else {
            panic!("an array");
        };
        assert_eq!(written.len(), 12);
    }

    /// **Its size is the caller's, never its content's.** A lane added by a
    /// `/gui_set` must not relayout the window.
    #[test]
    fn a_lane_added_never_changes_how_big_the_widget_wants_to_be() {
        let m = Metrics::default();
        let mut mt = Multitrack::default();
        let bare = mt.natural(&m, 1.0);
        mt.set("lanes", &props(TWO)["lanes"]);
        assert_eq!(mt.natural(&m, 1.0), bare);
        // What *does* follow the content is how far the axis reaches.
        assert_eq!(mt.content_span(), Some(0.0));
        mt.set("clips", &props(TWO)["clips"]);
        assert_eq!(mt.content_span(), Some(144_000.0));
    }
    /// **The three tags are gone.** A move, a trim and a lane crossing each
    /// leave as one `"clips"` payload carrying the piece as it now stands — so
    /// there is nothing for a reader to choose between, and no state a gesture
    /// can put it in where the report changes shape.
    #[test]
    fn every_gesture_reports_the_piece_and_never_the_gesture() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;

        let tag = |ev: Events| match ev.into_messages().first() {
            Some(args) => match &args[0] {
                OscType::String(s) => s.clone(),
                _ => "?".into(),
            },
            None => "<nothing>".into(),
        };

        // A move inside its lane.
        let mut mt = piece();
        let from = xy(&mt, &m, rect, 250.0, len, 0);
        let to = xy(&mt, &m, rect, 350.0, len, 0);
        assert!(matches!(
            mt.press(from, &input(&m, rect, len)),
            Claim::Take(_)
        ));
        mt.drag(to, &input(&m, rect, len));
        let moved = mt.release(to, true, &input(&m, rect, len));
        assert_eq!(tag(moved), "clips");
        assert_eq!(mt.clips[0].place.offset, 100.0, "it moved by the travel");

        // A trim, by the right edge.
        let mut mt = piece();
        let edge = xy(&mt, &m, rect, 500.0, len, 0);
        let pulled = xy(&mt, &m, rect, 400.0, len, 0);
        assert!(matches!(
            mt.press(edge, &input(&m, rect, len)),
            Claim::Take(_)
        ));
        mt.drag(pulled, &input(&m, rect, len));
        let trimmed = mt.release(pulled, true, &input(&m, rect, len));
        assert_eq!(tag(trimmed), "clips");
        assert!(mt.clips[0].place.dur < 500.0, "it got shorter");
        assert_eq!(mt.clips[0].place.offset, 0.0, "and stayed where it began");

        // A lane crossed.
        let mut mt = piece();
        let from = xy(&mt, &m, rect, 250.0, len, 0);
        let down = xy(&mt, &m, rect, 250.0, len, 1);
        assert!(matches!(
            mt.press(from, &input(&m, rect, len)),
            Claim::Take(_)
        ));
        mt.drag(down, &input(&m, rect, len));
        let crossed = mt.release(down, true, &input(&m, rect, len));
        assert_eq!(tag(crossed), "clips", "the same one tag, again");
        assert_eq!(
            mt.clips[0].lane, "tone",
            "one field, not a remove and an add"
        );
    }

    /// **A gesture that changed nothing is not an edit.** A press and a release
    /// with nothing between them is a click, and a drag that came back is the
    /// same thing by another road: looking at four clips must not cost four
    /// undos.
    #[test]
    fn a_drag_that_came_back_reports_nothing() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let from = xy(&mt, &m, rect, 250.0, len, 0);
        let away = xy(&mt, &m, rect, 400.0, len, 0);

        mt.press(from, &input(&m, rect, len));
        assert!(mt.release(from, true, &input(&m, rect, len)).is_empty());

        mt.press(from, &input(&m, rect, len));
        mt.drag(away, &input(&m, rect, len));
        mt.drag(from, &input(&m, rect, len));
        assert!(
            mt.release(from, true, &input(&m, rect, len)).is_empty(),
            "it is where the press found it"
        );
    }

    /// **A press that found no clip goes back to the chain.** The slack between
    /// clips is the container's: that is where a click places the transport's
    /// cursor and a sweep starts a marquee, and swallowing the press would take
    /// both away.
    #[test]
    fn a_press_on_bare_lane_declines_rather_than_swallowing_it() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        // Past `a`'s end, on its lane — bare lane, and `b` is on the other one.
        let bare = xy(&mt, &m, rect, 800.0, len, 0);
        assert!(matches!(
            mt.press(bare, &input(&m, rect, len)),
            Claim::Decline
        ));
        assert!(mt.grab.is_none());
    }

    /// The report is the same list a `/gui_set clips` would take, so applying
    /// what came back is the identity — which is what makes the payload a
    /// *state* rather than a description of a gesture.
    #[test]
    fn what_comes_back_is_what_a_set_would_take() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let from = xy(&mt, &m, rect, 250.0, len, 0);
        let to = xy(&mt, &m, rect, 350.0, len, 0);
        mt.press(from, &input(&m, rect, len));
        mt.drag(to, &input(&m, rect, len));
        let reported = mt.release(to, true, &input(&m, rect, len));

        let mut echo = piece();
        let msgs = reported.into_messages();
        let args = msgs.first().expect("an edit");
        assert!(echo.set("clips", &model::clips_json(&mt.clips)));
        assert_eq!(echo.clips, mt.clips);
        assert_eq!(args.len(), 1 + 6 * 2, "the tag, then a sextuple per clip");
    }
}
