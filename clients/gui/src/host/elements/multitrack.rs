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
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::host::font;
use crate::host::graphics::multitrack::{self as model, Clip, Lane};
use crate::host::graphics::signal::trace::{Measures, Trace};
use crate::host::graphics::track;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::placement::{self, Bounds, Contents, Part, Placement, Placements};
use crate::host::widget::element::{
    Claim, Ctx, Element, Events, Input, Key, KeyInput, Loaded, Needs, Swept, Take, TimeSpace,
};
use crate::host::widget::parse::{self, label, number, number_f64, truthy};
use crate::host::widget::size::Natural;
use crate::host::widget::{EditorProps, GestureMap, RulerY, SourceWindow};
use crate::viewport::View;
use crate::waveform::WaveformData;

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
    /// **The axis the press found**, for a widget on no navigation group.
    ///
    /// Such a widget's axis is its own extent, and a drag *changes* the extent
    /// — so re-deriving it per frame stretches the pixel-to-time map under the
    /// hand, the next step reads further, and the box runs away from the
    /// pointer. On a group the axis is read live instead, because there it is
    /// the group's and pans under the drag on purpose ([`Take::edge_scroll`]).
    axis: View,
}

/// A **fader** the hand is on, kept for the same reason a clip's grab is: the
/// value is read from the pointer against the groove the press found.
#[derive(Debug, Clone, Copy)]
struct Fading {
    lane: usize,
    groove: Rect,
}

/// The block a hand took, as `(index, offset, row)` per clip — the snapshot
/// `placement::move_block` clamps against, so a block stopped at an edge does
/// not fold against it.
type Block = Vec<(usize, f64, f32)>;

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
    /// **The takes, by server buffer number** — what a clip's body is drawn
    /// from, one entry however many clips read it.
    ///
    /// It is the element's because the samples are: a buffer arrives once
    /// ([`Element::bulk_of`]) and every clip over it draws the same pyramid,
    /// which is what makes six views of one recording cost one download.
    takes: HashMap<i32, Arc<WaveformData>>,
    /// The drag in flight. **The state lives in the element**; the machine
    /// keeps only the sequence.
    grab: Option<Grab>,
    /// The clips a body drag is carrying, snapshotted at the press.
    block: Block,
    /// The fader a drag is on, when it is on one.
    fading: Option<Fading>,
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
            takes: HashMap::new(),
            grab: None,
            block: Vec::new(),
            fading: None,
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
        takes: HashMap::new(),
        grab: None,
        block: Vec::new(),
        fading: None,
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

/// The `clips` prop: the flat `name lane offset dur start label source`
/// septuple array.
fn parse_clips(props: &Map<String, Value>) -> Vec<Clip> {
    let Some(Value::Array(items)) = props.get("clips") else {
        return Vec::new();
    };
    items
        .as_chunks::<7>()
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
                source: c[6].as_i64().unwrap_or(i64::from(model::NO_SOURCE)) as i32,
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

    /// The lane a pointer is **on**, or `None` off the stack — the *press*'
    /// question, over the same bands the drawing used.
    fn lane_at(&self, rect: Rect, y: f64) -> Option<usize> {
        model::lane_at(&self.lanes, rect, self.scroll, self.gap, y)
    }

    /// The lane a hand **is heading for**, always — the *drag*'s question, and
    /// the sweep's. It answers for the gaps between lanes and clamps past
    /// either end, which is the whole of why a dragged clip neither jumps nor
    /// oscillates.
    fn lane_toward(&self, rect: Rect, y: f64) -> usize {
        model::lane_toward(&self.lanes, rect, self.scroll, self.gap, y)
    }

    /// The clip under `(x, y)`, and which part of it — **the topmost first**,
    /// since a later clip is drawn over an earlier one and the eye takes the
    /// one it can see.
    fn clip_at(&self, input: &Input, at: (f64, f64)) -> Option<(usize, Part)> {
        let i = self.lane_at(input.rect, at.1)?;
        let rect = model::stack(&self.lanes, input.rect, self.scroll, self.gap)[i];
        // A band carries its gap, and nothing of a lane is drawn there: a press
        // in it is a press on bare stack, which the container sweeps.
        if (at.1 as f32) >= rect.y + rect.h {
            return None;
        }
        let body = track::lane_body(rect, false, input.indent, input.metrics);
        let nav = self.view(input.time);
        self.clips
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, c)| c.lane == self.lanes[i].name)
            .find_map(|(n, c)| {
                let (x0, x1) = model::clip_x(c, body, &nav, MIN_CLIP_W)?;
                let cr = track::clip_rect(body, x0, x1);
                // **A grip is hit on the pixels it was drawn on.** The same
                // call the drawing made, so the handle and its hit area cannot
                // disagree — the case nobody tests.
                let local = track::clip_local_view(body, &nav, c.place.offset, c.place.dur, cr);
                let ends = track::clip_ends_on_screen(&local, c.place.dur);
                if let Some((_, side)) = track::clip_grip_at(cr, ends, input.metrics, at.0 as f32) {
                    return Some((
                        n,
                        match side {
                            track::ClipSide::Start => Part::Start,
                            track::ClipSide::End => Part::End,
                        },
                    ));
                }
                let inside = at.0 as f32 >= x0 && at.0 as f32 <= x1;
                inside.then_some((n, Part::Body))
            })
    }

    /// The time a pointer x names on the shared axis.
    ///
    /// **Never on an axis this drag is moving.** A widget on no navigation
    /// group rules itself by its own extent, which a drag changes, so during
    /// one the axis is the press'; on a group it is read live, which is what an
    /// edge-scrolled pan needs.
    fn time_at(&self, input: &Input, x: f64) -> f64 {
        let nav = match (input.time, self.grab) {
            (None, Some(grab)) => grab.axis,
            (time, _) => self.view(time),
        };
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

    /// The lane header band `y` falls in, and the part of it `(x, y)` hit.
    fn header_at(&self, input: &Input, at: (f64, f64)) -> Option<(usize, track::HeaderPart)> {
        let i = self.lane_at(input.rect, at.1)?;
        let rect = model::stack(&self.lanes, input.rect, self.scroll, self.gap)[i];
        let band = crate::host::timeline::gutter_band(rect, input.indent);
        let header = self.header(&self.lanes[i], input.indent);
        let part = track::header_hit(band, &header, input.metrics, at.0, at.1)?;
        Some((i, part))
    }

    /// **The edit-back for the mixer: the lanes as they now are.** The second
    /// of the two payloads, and separate from the clips for the reason they are
    /// two structures — a fader moved must not resend every clip.
    fn lanes_event(&self) -> Events {
        let mut args = vec![OscType::String("lanes".into())];
        if let Value::Array(flat) = model::lanes_json(&self.lanes) {
            args.extend(flat.into_iter().map(json_arg));
        }
        Events::message(args)
    }

    /// The clips the hand is holding: the selection when the grabbed clip is in
    /// it, else the grabbed one alone.
    ///
    /// **Grabbing an unselected clip lets go of the block**, which is the rule a
    /// lane already had: a hand that reaches past its selection meant the box it
    /// reached for.
    fn held(&self, clip: usize) -> Vec<usize> {
        if self.selected.contains(&clip) {
            self.selected.clone()
        } else {
            vec![clip]
        }
    }

    /// A press on a lane's header: the two toggles land on the press, the fader
    /// takes the drag.
    ///
    /// **The mixer state is the composition's**, so all three report — and they
    /// report the `"lanes"` list, not the one lane, because what a report says
    /// here is the piece as it now stands.
    fn press_header(
        &mut self,
        lane: usize,
        part: track::HeaderPart,
        at: (f64, f64),
        input: &Input,
    ) -> Claim {
        let rect = model::stack(&self.lanes, input.rect, self.scroll, self.gap)[lane];
        let band = crate::host::timeline::gutter_band(rect, input.indent);
        let parts = track::header_parts(
            band,
            &self.header(&self.lanes[lane], input.indent),
            input.metrics,
        );
        match part {
            track::HeaderPart::Mute => {
                self.lanes[lane].mute = !self.lanes[lane].mute;
                Claim::Take(Take {
                    events: self.lanes_event(),
                    ..Take::default()
                })
            }
            track::HeaderPart::Solo => {
                self.lanes[lane].solo = !self.lanes[lane].solo;
                Claim::Take(Take {
                    events: self.lanes_event(),
                    ..Take::default()
                })
            }
            track::HeaderPart::Fader => {
                let Some(groove) = parts.fader else {
                    return Claim::Decline;
                };
                // **Absolute**: a position inside the groove *is* the value,
                // snapshotted at the press because the groove may scroll under
                // the hand.
                self.lanes[lane].gain = track::level_at(groove, at.0);
                self.fading = Some(Fading { lane, groove });
                Claim::take()
            }
        }
    }

    /// A name no clip here has yet, derived from `base` — what a **split** needs
    /// and what a **paste** needs.
    ///
    /// The identity is the client's word, and a split makes one the client never
    /// said. So the host mints it the way it mints a marker's number, from the
    /// name that was there, and it comes back in the report as any other name
    /// does: the script learns it by being told, not by guessing a rule.
    fn fresh_name(&self, base: &str) -> String {
        let mut n = 2;
        loop {
            let name = format!("{base} {n}");
            if self.clip(&name).is_none() {
                return name;
            }
            n += 1;
        }
    }

    /// The clip of this name, if it is here.
    fn clip(&self, name: &str) -> Option<&Clip> {
        self.clips.iter().find(|c| c.name == name)
    }

    /// **Cut every held clip at `at`**, keeping the halves in the hand.
    ///
    /// The window over the contents moves with the cut — `placement::split_at`
    /// is the arithmetic, the same one a note's split uses — so the second half
    /// reads on from where the first stopped rather than from the source's
    /// start.
    fn split_held(&mut self, at: f64) -> bool {
        let mut made = Vec::new();
        for &i in &self.selected {
            let Some(clip) = self.clips.get(i) else {
                continue;
            };
            let Some((first, second)) = placement::split_at(clip.place, at) else {
                continue;
            };
            let name = self.fresh_name(&clip.name);
            let mut tail = clip.clone();
            tail.name = name;
            tail.place = second;
            made.push((i, first, tail));
        }
        if made.is_empty() {
            return false;
        }
        for (i, first, tail) in made {
            self.clips[i].place = first;
            self.clips.push(tail);
            self.selected.push(self.clips.len() - 1);
        }
        true
    }

    /// **Join the held clips that touch, on one lane** — a pitch is what makes
    /// two notes one voice, and a **lane** is what makes two clips joinable.
    ///
    /// A run is read over what is there, so an overlap joins as readily as a
    /// juxtaposition: two boxes sharing pixels are not two boxes to a reader.
    fn join_held(&mut self) -> bool {
        let mut held = self.selected.clone();
        held.sort_by(|a, b| {
            let (x, y) = (&self.clips[*a], &self.clips[*b]);
            (x.lane.as_str(), x.place.offset)
                .partial_cmp(&(y.lane.as_str(), y.place.offset))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut drop: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < held.len() {
            let head = held[i];
            let mut j = i + 1;
            while j < held.len()
                && self.clips[held[j]].lane == self.clips[head].lane
                && placement::adjacent(self.clips[head].place, self.clips[held[j]].place, 1.0)
            {
                self.clips[head].place =
                    placement::merge(self.clips[head].place, self.clips[held[j]].place);
                drop.push(held[j]);
                j += 1;
            }
            i = j;
        }
        if drop.is_empty() {
            return false;
        }
        drop.sort_unstable();
        for i in drop.into_iter().rev() {
            self.clips.remove(i);
        }
        self.selected.clear();
        true
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

/// **The clips are boxes on rows**, which is the one thing the box arithmetic
/// asks of whoever holds some.
///
/// The row is the **lane's index**, so `in_rect`, `move_block` and `quantize`
/// — already written and already tested against a roll's notes and a lane's
/// clips — work here unchanged. Writing the row back is what makes a block
/// dragged across the stack change the lanes its clips name: one field each,
/// with nothing removed and nothing inserted.
impl Placements for Multitrack {
    fn len(&self) -> usize {
        self.clips.len()
    }

    fn placement(&self, i: usize) -> Placement {
        self.clips[i].place
    }

    fn set_placement(&mut self, i: usize, p: Placement) {
        self.clips[i].place = p;
    }

    fn row(&self, i: usize) -> f32 {
        self.lane_of(&self.clips[i]).unwrap_or(0) as f32
    }

    fn set_row(&mut self, i: usize, r: f32) {
        let at = r.round().max(0.0) as usize;
        if let Some(lane) = self.lanes.get(at) {
            self.clips[i].lane = lane.name.clone();
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
            let local = track::clip_local_view(body, &nav, clip.place.offset, clip.place.dur, cr);
            // **The take, drawn from the source per visible pixel**, mapped
            // back through the clip's own window onto it — which is what makes
            // the picture scroll and trim *with* the box instead of squashing
            // into whatever rectangle it currently has. One pyramid however
            // many clips read it.
            if let Some(take) = self.takes.get(&clip.source) {
                let window = SourceWindow {
                    start: clip.place.start,
                    ..SourceWindow::default()
                };
                track::draw_take(
                    d,
                    cr,
                    &local,
                    &window,
                    clip.place.dur,
                    &Trace::Data(take),
                    -1.0,
                    1.0,
                    Measures::default(),
                    false,
                    ctx.world.sample_rate,
                    None,
                );
            }
            track::draw_clip_label(d, cr, clip.shown());
            // **The grips are drawn where they are grabbed.** An end that is
            // off screen has no grip, because a handle for an edge nobody can
            // see is a handle for nothing — the same rule a lane's clip keeps.
            // **A grip is an affordance, so it is shown where the hand is.**
            // Drawn always, every clip carries two marks nobody is reaching
            // for; drawn on the side the pointer is over, it says *this edge
            // moves* at the moment that is worth saying.
            let ends = track::clip_ends_on_screen(&local, clip.place.dur);
            if let Some((cx, cy)) = ctx.world.cursor
                && cy as f32 >= cr.y
                && (cy as f32) < cr.y + cr.h
                && let Some((grip, side)) = track::clip_grip_at(cr, ends, ctx.metrics, cx as f32)
            {
                track::draw_clip_grip(d, grip, side);
            }
        }
        // **The axis' own chrome, over the clips**: the shared selection band
        // and the playhead. A lane widget gets these drawn for it by the frame;
        // an element draws its own, from the same facts (`Ctx::time`).
        if let Some(time) = ctx.time {
            let over = track::lane_body(ctx.rect, false, ctx.indent, ctx.metrics);
            crate::host::graphics::selection::draw_span(
                d,
                over,
                &nav,
                time.sel,
                1,
                None,
                crate::host::graphics::selection::Vertical::Whole,
            );
            if let Some(pos) = time.head
                && let Some(x) = track::playhead_x(over, &nav, pos)
            {
                let (mesh, m, theme) = d.parts();
                mesh.rect(Rect::new(x, over.y, m.trace_w, over.h), theme.playhead);
            }
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

    /// **A press takes a clip or a header control, and declines everywhere
    /// else.** The slack between clips and beside them is the container's —
    /// that is where a click places the transport's cursor and a sweep starts a
    /// marquee — so a press that found neither goes back to the chain rather
    /// than being swallowed.
    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        self.grab = None;
        self.fading = None;
        self.block.clear();
        // The header band first: it is drawn over the gutter, and nothing of
        // the axis is there.
        if let Some((lane, part)) = self.header_at(input, at) {
            return self.press_header(lane, part, at, input);
        }
        let Some((clip, part)) = self.clip_at(input, at) else {
            return Claim::Decline;
        };
        // **Alt adds or removes that one**, the same key that adds a note to a
        // roll's selection.
        if input.mods.alt {
            placement::toggle_selected(&mut self.selected, clip);
            return Claim::take();
        }
        let Some(lane) = self.lane_of(&self.clips[clip]) else {
            return Claim::Decline;
        };
        // **An edge is always one clip's**: two clips of different lengths have
        // no one edge to pull, so a trim lets go of the block.
        self.block = match part {
            Part::Body => self
                .held(clip)
                .into_iter()
                .map(|i| (i, self.clips[i].place.offset, self.row(i)))
                .collect(),
            _ => vec![(clip, self.clips[clip].place.offset, lane as f32)],
        };
        if part != Part::Body && !self.selected.contains(&clip) {
            self.selected.clear();
        }
        self.grab = Some(Grab {
            clip,
            part,
            orig: self.clips[clip].place,
            lane,
            grabbed_at: self.time_at(input, at.0),
            axis: self.view(input.time),
        });
        Claim::Take(Take {
            // Held past the edge of the axis, the machine keeps ticking and
            // pans the group under the hand — a clip dragged off the right of
            // the window has to keep moving, and a held cursor sends nothing.
            edge_scroll: true,
            ..Take::default()
        })
    }

    /// **What a rectangle swept over the stack caught.** The marquee's one
    /// question, answered with the clips the rectangle covered — of every lane
    /// it crossed, since a selection the stack's sweep made is not one lane's.
    fn select_in(&mut self, from: (f64, f64), to: (f64, f64), input: &Input) -> Swept {
        let before = self.selected.len();
        let (t0, t1) = (self.time_at(input, from.0), self.time_at(input, to.0));
        // The same continuous answer a drag takes: a corner in a gap or past
        // an end still means the sweep passed through those lanes.
        let r0 = self.lane_toward(input.rect, from.1) as f32;
        let r1 = self.lane_toward(input.rect, to.1) as f32;
        self.selected = placement::in_rect(self, t0, t1, r0, r1);
        Swept {
            changed: before != self.selected.len() || !self.selected.is_empty(),
            // **No band.** A multitrack's second axis is the stack of lanes,
            // not a value, so a rectangle over it restricts no value range —
            // the vertical half said *which clips*, and nothing else.
            band: None,
        }
    }

    /// The clips follow the hand; **the edit leaves on release.**
    ///
    /// One gesture is one edit — a placement per frame would be an undo step
    /// per frame, and a round trip whose acknowledgement the next frame
    /// outruns. What moves here is the picture. The **fader** is the exception
    /// and is not one: it is a control, its value *is* what the hand is doing,
    /// and it reports as it goes exactly as every other control does.
    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        if let Some(f) = self.fading {
            self.lanes[f.lane].gain = track::level_at(f.groove, at.0);
            return self.lanes_event();
        }
        let Some(grab) = self.grab else {
            return Events::none();
        };
        let now = self.time_at(input, at.0);
        match grab.part {
            // **A block travels in time and across the stack, rigidly.** The
            // deltas are clamped as one, so a block stopped at an edge does not
            // fold against it, and no clip is resized.
            Part::Body => {
                let dt = placement::snap(now - grab.grabbed_at, self.snap);
                let dr = self.lane_toward(input.rect, at.1) as f32 - grab.lane as f32;
                let rows = (0.0, self.lanes.len().saturating_sub(1) as f32);
                let block = std::mem::take(&mut self.block);
                placement::move_block(self, &block, dt, dr, rows, None);
                self.block = block;
            }
            // **An edge is one clip's**, and it trims: the placement and the
            // window over the contents move together.
            part => {
                let place =
                    placement::drag(part, now, grab.orig, Contents::default(), self.bounds());
                self.clips[grab.clip].place = place;
            }
        }
        Events::none()
    }

    /// **A gesture that changed nothing is not an edit.** A press and a release
    /// with nothing in between is a click, and a drag that came back to where
    /// it began is the same thing by another road: reporting it would hand the
    /// owner an intent to apply and a document an entry to undo, so looking at
    /// four clips would cost four undos.
    fn release(&mut self, _at: (f64, f64), _inside: bool, _input: &Input) -> Events {
        if self.fading.take().is_some() {
            // Already reported on the way, like any other control.
            return Events::none();
        }
        let Some(grab) = self.grab.take() else {
            return Events::none();
        };
        let block = std::mem::take(&mut self.block);
        let moved = block
            .iter()
            .any(|&(i, offset, row)| self.clips[i].place.offset != offset || self.row(i) != row)
            || self.clips[grab.clip].place != grab.orig;
        if !moved {
            return Events::none();
        }
        self.clips_event()
    }

    fn accepts_focus(&self) -> bool {
        true
    }

    /// The verbs a hand has over what it is holding.
    ///
    /// `q` quantizes onto the lane's own `snap` grid — the grid a drag already
    /// lands on — `e` splits at the window's cursor and `j` joins a touching
    /// run, Delete removes, and `Ctrl`+`C`/`X`/`V` move a block through the
    /// host-wide clipboard. All of them act on **the held set**, across the
    /// stack, and all of them report the clips as they now stand: there is one
    /// payload here and a verb does not get to invent a second.
    ///
    /// **The letters are the ones a clip already answered to on a lane.** Which
    /// keys they are is not settled — see `clients/gui/PLAN.md`, "A shortcut is
    /// the application's, not the widget's".
    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        if self.selected.is_empty() && !matches!(key, Key::Char('v') | Key::Char('V')) {
            return None;
        }
        match key {
            Key::Char('q') | Key::Char('Q') if !input.mods.ctrl => {
                let held = self.selected.clone();
                placement::quantize(self, &held, self.snap).then(|| self.clips_event())
            }
            // **At the window's cursor**: a key gesture has no pointer to read a
            // position from, and the window has one cursor for exactly that.
            Key::Char('e') | Key::Char('E') if !input.mods.ctrl => {
                let at = placement::snap(input.cursor.unwrap_or(0.0), self.snap).max(0.0);
                self.split_held(at).then(|| self.clips_event())
            }
            Key::Char('j') | Key::Char('J') if !input.mods.ctrl => {
                self.join_held().then(|| self.clips_event())
            }
            Key::Delete | Key::Backspace => {
                let mut held = self.selected.clone();
                held.sort_unstable();
                for i in held.into_iter().rev() {
                    if i < self.clips.len() {
                        self.clips.remove(i);
                    }
                }
                self.selected.clear();
                Some(self.clips_event())
            }
            // The clipboard is the host's one string, so a block travels between
            // multitracks and windows — and rides it in the same JSON form a
            // `/gui_set clips` accepts, which is the carrier every non-scalar
            // here uses.
            Key::Char('c') | Key::Char('C') | Key::Char('x') | Key::Char('X')
                if input.mods.ctrl =>
            {
                let block: Vec<Clip> = self
                    .selected
                    .iter()
                    .filter_map(|&i| self.clips.get(i).cloned())
                    .collect();
                if block.is_empty() {
                    return None;
                }
                input
                    .clipboard
                    .set_text(&model::clips_json(&block).to_string());
                if !matches!(key, Key::Char('x') | Key::Char('X')) {
                    // A copy changed nothing, so it reports nothing — but it
                    // consumed the key.
                    return Some(Events::none());
                }
                let mut held = self.selected.clone();
                held.sort_unstable();
                for i in held.into_iter().rev() {
                    self.clips.remove(i);
                }
                self.selected.clear();
                Some(self.clips_event())
            }
            Key::Char('v') | Key::Char('V') if input.mods.ctrl => {
                let mut props = Map::new();
                props.insert(
                    "clips".into(),
                    serde_json::from_str(&input.clipboard.text()).ok()?,
                );
                let block = parse_clips(&props);
                if block.is_empty() {
                    return None;
                }
                // **At the cursor**, and keeping the block's own shape: the
                // earliest pasted clip lands there and the rest keep their
                // distances, which is what makes a pasted block the same block.
                let at = placement::snap(input.cursor.unwrap_or(0.0), self.snap).max(0.0);
                let first = block
                    .iter()
                    .map(|c| c.place.offset)
                    .fold(f64::INFINITY, f64::min);
                self.selected.clear();
                for mut clip in block {
                    clip.place.offset = (clip.place.offset - first + at).max(0.0);
                    clip.name = self.fresh_name(&clip.name);
                    self.clips.push(clip);
                    self.selected.push(self.clips.len() - 1);
                }
                Some(self.clips_event())
            }
            _ => None,
        }
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

    /// **The takes its clips are windows onto**, by server buffer number.
    ///
    /// The plural of the one source a picture asks for: this element holds
    /// boxes, and every one of them is a window onto samples the server has.
    /// Named once each, because the fetch is keyed by buffer and two clips over
    /// one recording are one download.
    /// **What a lane's header asks for, left of the axis.**
    ///
    /// It is the group's answer and not this widget's: the layout stamps the
    /// widest wish any member of the navigation group made, so a ruler stacked
    /// with these lanes starts its ticks over the same sample. Without it there
    /// is no band, and a lane draws no name and no controls at all.
    fn gutter(&self, m: &Metrics) -> f32 {
        let header = track::Header {
            w: None,
            mute: Some(false),
            solo: Some(false),
            level: Some(1.0),
        };
        header.width(m)
    }

    fn needs(&self) -> Needs {
        let mut takes: Vec<i32> = self
            .clips
            .iter()
            .map(|c| c.source)
            .filter(|n| *n >= 0)
            .collect();
        takes.sort_unstable();
        takes.dedup();
        Needs {
            takes,
            // **A swept line is a picture driven by the clock**, so the window
            // has to be told: an anchored playhead moves with no message and
            // nothing else would ask for the frame it moves on.
            clock: self.editor.playhead_at >= 0.0,
            ..Needs::default()
        }
    }

    /// One of them arrived. Every clip over it draws the same pyramid.
    fn bulk_of(&mut self, bufnum: i32, data: Loaded) -> bool {
        let peaks = match data {
            Loaded::Peaks(peaks) | Loaded::Shared(peaks) => peaks,
            Loaded::Raw { samples, channels } => Arc::new(WaveformData::from_interleaved(
                &samples,
                channels.max(1),
                crate::host::elements::signal::DEFAULT_BASE_BUCKET,
            )),
            _ => return false,
        };
        self.takes.insert(bufnum, peaks);
        true
    }

    /// **The plan a hand on the stack runs.** The element first — that is a
    /// clip and a header control — then a marquee over what it declined, which
    /// is the bare stack. Shift pans the shared axis, as it does everywhere.
    ///
    /// A **click** (a sweep that never left the slop) is a rectangle of no
    /// size: it lets go of everything, and the machine puts the transport's
    /// cursor where it pointed, which is what makes one cursor the window's.
    fn gesture_map(&self) -> Option<GestureMap> {
        use crate::host::widget::GestureStep::{Element as El, Marquee, Pan};
        Some(GestureMap::of_plans(
            &[El, Marquee],
            &[Pan],
            &[El, Marquee],
            &[El, Marquee],
        ))
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
                "clips": ["a", "noise", 0, 500, 0, "", 0, "b", "tone", 500, 500, 0, "", 0]}"#,
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
        "clips": ["a", "noise", 0, 48000, 0, "", 0,
                  "b", "tone", 96000, 48000, 0, "take 2", 0]
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
                "clips": ["a", "one", 0, 10, 0, ""]}"#,
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
                "clips": ["a", "one", 0, 10, 0, "", 0, "b", "vanished", 0, 10, 0, "", 0]}"#,
        ));
        assert_eq!(mt.clips.len(), 2);
        assert!(mt.lane_of(&mt.clips[0]).is_some());
        assert!(mt.lane_of(&mt.clips[1]).is_none());
        // It is still in what the widget reports, which is how it comes back.
        let Value::Array(written) = model::clips_json(&mt.clips) else {
            panic!("an array");
        };
        assert_eq!(written.len(), 14);
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
        assert_eq!(args.len(), 1 + 7 * 2, "the tag, then a septuple per clip");
    }
    /// **A marquee catches the clips it covered, of every lane it crossed** — a
    /// selection the stack's sweep made is not one lane's. And it writes no
    /// band: the second axis here is the stack, not a value.
    #[test]
    fn a_sweep_catches_what_it_covered_across_the_stack() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();

        let from = xy(&mt, &m, rect, 100.0, len, 0);
        let to = xy(&mt, &m, rect, 900.0, len, 1);
        let swept = mt.select_in(from, to, &input(&m, rect, len));
        assert_eq!(mt.selected.len(), 2, "one from each lane");
        assert!(swept.changed);
        assert!(swept.band.is_none(), "the stack is not a value axis");

        // A sweep over one lane's time only catches that lane's.
        let a = xy(&mt, &m, rect, 100.0, len, 0);
        let b = xy(&mt, &m, rect, 400.0, len, 0);
        mt.select_in(a, b, &input(&m, rect, len));
        assert_eq!(mt.selected, vec![0]);
    }

    /// **A block travels rigidly, and grabbing an unselected clip lets go of
    /// it** — the hand that reached past its selection meant the box it reached
    /// for.
    #[test]
    fn a_block_moves_as_one_and_an_unselected_grab_moves_alone() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        mt.selected = vec![0, 1];

        let from = xy(&mt, &m, rect, 250.0, len, 0);
        let to = xy(&mt, &m, rect, 350.0, len, 0);
        mt.press(from, &input(&m, rect, len));
        mt.drag(to, &input(&m, rect, len));
        mt.release(to, true, &input(&m, rect, len));
        assert_eq!(mt.clips[0].place.offset, 100.0);
        assert_eq!(mt.clips[1].place.offset, 600.0, "the other one came too");

        // Now grab the one that is *not* selected.
        let mut mt = piece();
        mt.selected = vec![1];
        let on_a = xy(&mt, &m, rect, 250.0, len, 0);
        let over = xy(&mt, &m, rect, 350.0, len, 0);
        mt.press(on_a, &input(&m, rect, len));
        mt.drag(over, &input(&m, rect, len));
        mt.release(over, true, &input(&m, rect, len));
        assert_eq!(mt.clips[0].place.offset, 100.0);
        assert_eq!(mt.clips[1].place.offset, 500.0, "it stayed where it was");
    }

    /// **The mixer is the second payload.** A fader and the two toggles report
    /// `"lanes"` — the lanes as they now stand — and never the clips, which is
    /// the whole reason the piece is written as two structures.
    #[test]
    fn the_header_reports_the_lanes_and_never_the_clips() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let at = model::stack(&mt.lanes, rect, mt.scroll, mt.gap);
        let band = crate::host::timeline::gutter_band(at[0], 100.0);
        let parts = track::header_parts(band, &mt.header(&mt.lanes[0], 100.0), &m);

        let mute = parts.mute.expect("the lane offers a mute");
        let claim = mt.press(
            (
                f64::from(mute.x + mute.w / 2.0),
                f64::from(mute.y + mute.h / 2.0),
            ),
            &input(&m, rect, len),
        );
        let Claim::Take(take) = claim else {
            panic!("the header takes the press")
        };
        let msgs = take.events.into_messages();
        assert_eq!(msgs[0][0], OscType::String("lanes".into()));
        assert!(mt.lanes[0].mute, "and it flipped");
        assert_eq!(
            msgs[0].len(),
            1 + 6 * 2,
            "the tag, then a sextuple per lane"
        );
    }

    /// `q` quantizes what the hand holds, across the stack, and Delete removes
    /// it — both reporting the clips as they now stand, which is the one
    /// payload every edit here has.
    #[test]
    fn the_key_verbs_act_on_the_held_set_and_report_the_piece() {
        let mut mt = piece();
        mt.snap = 400.0;
        mt.selected = vec![0, 1];
        let mut clipboard = crate::host::clipboard::Clip::default();
        fn ki(clip: &mut crate::host::clipboard::Clip) -> KeyInput<'_> {
            KeyInput {
                mods: Mods::default(),
                clipboard: clip,
                cursor: None,
            }
        }

        let quantized = mt
            .key(&Key::Char('q'), &mut ki(&mut clipboard))
            .expect("something moved");
        assert_eq!(
            quantized.into_messages()[0][0],
            OscType::String("clips".into())
        );
        assert_eq!(mt.clips[1].place.offset, 400.0, "500 onto a 400 grid");

        let removed = mt
            .key(&Key::Delete, &mut ki(&mut clipboard))
            .expect("they went");
        assert_eq!(
            removed.into_messages()[0][0],
            OscType::String("clips".into())
        );
        assert!(mt.clips.is_empty());
        assert!(mt.selected.is_empty());

        // With nothing held, the keys are not this widget's.
        assert!(mt.key(&Key::Char('q'), &mut ki(&mut clipboard)).is_none());
    }
    /// **`e` cuts at the window's cursor and `j` joins a touching run.** The
    /// window over the contents moves with the cut, so the second half reads on
    /// from where the first stopped — and a join is stated over what is there,
    /// so it puts the two back.
    #[test]
    fn a_clip_splits_at_the_cursor_and_joins_back() {
        let mut mt = piece();
        let mut clipboard = crate::host::clipboard::Clip::default();
        fn ki(clip: &mut crate::host::clipboard::Clip, cursor: Option<f64>) -> KeyInput<'_> {
            KeyInput {
                mods: Mods::default(),
                clipboard: clip,
                cursor,
            }
        }
        mt.selected = vec![0];

        let cut = mt
            .key(&Key::Char('e'), &mut ki(&mut clipboard, Some(200.0)))
            .expect("it cut");
        assert_eq!(cut.into_messages()[0][0], OscType::String("clips".into()));
        assert_eq!(mt.clips.len(), 3);
        assert_eq!(mt.clips[0].place.dur, 200.0);
        let tail = mt.clips.last().expect("the second half");
        assert_eq!((tail.place.offset, tail.place.dur), (200.0, 300.0));
        assert_eq!(
            tail.place.start, 200.0,
            "it reads on rather than restarting"
        );
        assert_eq!(
            tail.name, "a 2",
            "a name the client never said, minted here"
        );
        assert_eq!(tail.lane, "noise", "and it stayed on its lane");

        // Both halves are in the hand, so `j` puts them back.
        let joined = mt
            .key(&Key::Char('j'), &mut ki(&mut clipboard, None))
            .expect("it joined");
        assert_eq!(
            joined.into_messages()[0][0],
            OscType::String("clips".into())
        );
        assert_eq!(mt.clips.len(), 2);
        assert_eq!(mt.clips[0].place.dur, 500.0);
        assert_eq!(mt.clips[0].place.offset, 0.0);
    }

    /// A join is **a lane's**: a pitch is what makes two notes one voice, and a
    /// lane is what makes two clips joinable.
    #[test]
    fn two_clips_on_two_lanes_do_not_join() {
        let mut mt = piece();
        let mut clipboard = crate::host::clipboard::Clip::default();
        mt.clips[1].place.offset = 500.0; // it already ends where `a` does
        mt.selected = vec![0, 1];
        let joined = mt.key(
            &Key::Char('j'),
            &mut KeyInput {
                mods: Mods::default(),
                clipboard: &mut clipboard,
                cursor: None,
            },
        );
        assert!(joined.is_none(), "they touch in time and not on a lane");
        assert_eq!(mt.clips.len(), 2);
    }

    /// **A block travels through the clipboard keeping its own shape**: the
    /// earliest lands on the cursor and the rest keep their distances, which is
    /// what makes a pasted block the same block.
    #[test]
    fn a_block_is_cut_and_pasted_at_the_cursor_with_its_shape() {
        let mut clipboard = crate::host::clipboard::Clip::default();
        let mut mt = piece();
        mt.selected = vec![0, 1];
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };

        let cut = mt
            .key(
                &Key::Char('x'),
                &mut KeyInput {
                    mods: ctrl,
                    clipboard: &mut clipboard,
                    cursor: None,
                },
            )
            .expect("they left");
        assert_eq!(cut.into_messages()[0][0], OscType::String("clips".into()));
        assert!(mt.clips.is_empty());

        let pasted = mt
            .key(
                &Key::Char('v'),
                &mut KeyInput {
                    mods: ctrl,
                    clipboard: &mut clipboard,
                    cursor: Some(100.0),
                },
            )
            .expect("they came back");
        assert_eq!(
            pasted.into_messages()[0][0],
            OscType::String("clips".into())
        );
        assert_eq!(mt.clips.len(), 2);
        assert_eq!(
            mt.clips[0].place.offset, 100.0,
            "the earliest on the cursor"
        );
        assert_eq!(mt.clips[1].place.offset, 600.0, "and the shape kept");
        assert_eq!(
            mt.clips[0].lane, "noise",
            "each on the lane it was cut from"
        );
        assert_eq!(mt.clips[1].lane, "tone");
    }
    /// **A drag over the gap between two lanes must not jump.** The pointer
    /// crosses pixels no lane is drawn on, and a hit test that answers
    /// "nowhere" there makes the block snap back to the row the press found —
    /// for those frames only, so it flickers, and it jumps two rows at once
    /// when the gap is not the one it started beside. That is the glitch the
    /// window's own edges had, twice.
    ///
    /// The rule is `graphics::multitrack::bands`', and it is `gestures/nav.rs`'
    /// for the widget-tree stack: **a gap belongs to the lane above it**, so a
    /// pointer between two lanes is on one rather than on nothing.
    #[test]
    fn a_drag_through_the_gap_between_lanes_does_not_jump() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 420.0);
        let len = 1000.0;
        let mut mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1, "two", "", 100, 0, 0, 1,
                          "three", "", 100, 0, 0, 1],
                "clips": ["a", "one", 0, 500, 0, "", 0]}"#,
        ));
        mt.gap = 8.0;

        let at = model::stack(&mt.lanes, rect, mt.scroll, mt.gap);
        let from = xy(&mt, &m, rect, 250.0, len, 0);
        mt.press(from, &input(&m, rect, len));

        // The gap between the **second** and the third lane: two rows from
        // where the press was, so answering "nowhere" reads as the origin and
        // jumps two rows rather than none.
        let in_gap = f64::from(at[1].y + at[1].h + mt.gap / 2.0);
        mt.drag((from.0, in_gap), &input(&m, rect, len));
        assert_eq!(mt.clips[0].lane, "two", "the gap is the lane above's");

        // On into the third, and back through the gap: one step each way, and
        // never a return to where the press was.
        mt.drag(xy(&mt, &m, rect, 250.0, len, 2), &input(&m, rect, len));
        assert_eq!(mt.clips[0].lane, "three");
        mt.drag((from.0, in_gap), &input(&m, rect, len));
        assert_eq!(mt.clips[0].lane, "two", "and not back to \"one\"");
    }

    /// **Held past the end of the stack, a block stops rather than folding.**
    /// The continuous index is clamped, so a hand dragged off the bottom leaves
    /// the clip on the last lane instead of oscillating back to the first.
    #[test]
    fn a_drag_past_the_last_lane_stops_at_it() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 320.0);
        let len = 1000.0;
        let mut mt = piece();
        let from = xy(&mt, &m, rect, 250.0, len, 0);

        mt.press(from, &input(&m, rect, len));
        mt.drag((from.0, 10_000.0), &input(&m, rect, len));
        assert_eq!(mt.clips[0].lane, "tone", "the last one, not the first");
        mt.drag((from.0, -10_000.0), &input(&m, rect, len));
        assert_eq!(
            mt.clips[0].lane, "noise",
            "and the first going the other way"
        );
    }

    /// **A drag must never read an axis it is moving.** A widget on no
    /// navigation group rules itself by its own extent, and a drag *changes*
    /// the extent — so re-deriving the axis per frame stretches the
    /// pixel-to-time map under the hand, the next step reads further out, and
    /// the box accelerates away from the pointer.
    ///
    /// Measured before the fix: 40, 80, 400, 1600, **8675** for equal steps.
    /// It is the same miscalculation the window's own edges had.
    #[test]
    fn a_drag_off_a_group_reads_the_axis_the_press_found() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let mut bare = input(&m, rect, len);
        bare.time = None;

        let from = xy(&mt, &m, rect, 250.0, len, 0);
        mt.press(from, &bare);
        let mut seen = Vec::new();
        for step in [1.0, 2.0, 10.0, 40.0, 100.0] {
            mt.drag((from.0 + step * 20.0, from.1), &bare);
            seen.push((step, mt.clips[0].place.offset));
        }
        // Equal ratios of travel give equal ratios of offset: the map held
        // still even as the piece grew under it.
        let (first_step, first) = seen[0];
        for (step, offset) in &seen {
            let want = first * step / first_step;
            assert!(
                (offset - want).abs() < 1.0,
                "step {step}: {offset} against {want} — the axis moved"
            );
        }
        assert!(model::extent(&mt.clips) > len, "and the piece did grow");
    }
    /// **A grip is hit on the pixels it is drawn on.** The press asks the same
    /// call the drawing made, so the handle and its hit area cannot disagree —
    /// which is exactly the case nobody tests.
    #[test]
    fn an_edge_is_grabbed_by_the_grip_that_is_drawn_there() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mt = piece();

        let body = track::lane_body(
            model::stack(&mt.lanes, rect, mt.scroll, mt.gap)[0],
            false,
            100.0,
            &m,
        );
        let nav = View { start: 0.0, len };
        let (x0, x1) = model::clip_x(&mt.clips[0], body, &nav, MIN_CLIP_W).expect("on screen");
        let cr = track::clip_rect(body, x0, x1);
        let local = track::clip_local_view(
            body,
            &nav,
            mt.clips[0].place.offset,
            mt.clips[0].place.dur,
            cr,
        );
        let ends = track::clip_ends_on_screen(&local, mt.clips[0].place.dur);
        let (left, right) = track::clip_grips(cr, ends, &m);
        let left = left.expect("its start is on screen");
        let right = right.expect("and so is its end");

        let mid_y = f64::from(cr.y + cr.h / 2.0);
        let on = |r: Rect| (f64::from(r.x + r.w / 2.0), mid_y);
        assert_eq!(
            mt.clip_at(&input(&m, rect, len), on(left)),
            Some((0, Part::Start))
        );
        assert_eq!(
            mt.clip_at(&input(&m, rect, len), on(right)),
            Some((0, Part::End))
        );
        // And the middle is the body, which is what moves it.
        let middle = (f64::from(cr.x + cr.w / 2.0), mid_y);
        assert_eq!(
            mt.clip_at(&input(&m, rect, len), middle),
            Some((0, Part::Body))
        );
    }
    /// **Buffer 0 is a buffer.** It is the first one an allocator hands out, so
    /// a zero sentinel would make the first take a script loads the one take it
    /// cannot draw — which is exactly how this was found, by eye, on the first
    /// clip of a piece.
    #[test]
    fn a_clip_over_buffer_zero_has_a_source() {
        let mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1],
                "clips": ["a", "one", 0, 10, 0, "", 0,
                          "b", "one", 20, 10, 0, "", -1]}"#,
        ));
        assert_eq!(mt.clips[0].source, 0, "a window onto buffer 0");
        assert_eq!(mt.clips[1].source, model::NO_SOURCE, "and one onto nothing");
        assert_eq!(mt.needs().takes, vec![0], "so exactly one take is fetched");

        // A clip written with no source at all is a window onto nothing, not
        // onto buffer 0.
        let bare = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1], "clips": ["a", "one", 0, 10, 0, ""]}"#,
        ));
        assert!(bare.clips.is_empty(), "six fields is a partial septuple");
    }
    /// **A lane's header is a gutter the group reserves.** It is asked of the
    /// element and stamped by the layout as the widest wish on the axis, so a
    /// ruler stacked with these lanes starts its ticks over the same sample.
    /// Answering zero is a stack with no names and no controls on it.
    #[test]
    fn the_lanes_ask_for_the_band_their_headers_need() {
        let m = Metrics::default();
        let mt = piece();
        assert!(
            mt.gutter(&m) >= m.header_w,
            "a header carrying a name, two toggles and a fader is at least the role"
        );
    }

    /// **A swept line is a picture driven by the clock**, so the window has to
    /// be told: an anchored playhead moves with no message, and nothing else in
    /// a multitrack would ask for the frame it moves on.
    #[test]
    fn an_anchored_playhead_asks_the_window_for_frames() {
        let mut mt = piece();
        assert!(!mt.needs().clock, "a stopped transport drives nothing");
        mt.set("playhead_at", &Value::from(0.0));
        assert!(mt.needs().clock, "and an anchored one drives the window");
    }
}
