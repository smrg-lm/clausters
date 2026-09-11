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

use serde_json::{Map, Value};

use crate::host::elements::notes::Notes;
use crate::host::elements::signal::{Presentation, SignalElement};
use crate::host::font;
use crate::host::graphics::multitrack::{self as model, Clip, Lane};
use crate::host::graphics::track;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::placement::{self, Bounds, Contents, Part, Placement, Placements};
use crate::host::widget::element::{
    Claim, Ctx, Element, Events, Input, Key, KeyInput, Loaded, Needs, SlotFill, SlotKey, Swept,
    Take, TextureBody, TimeSpace,
};
use crate::host::widget::parse::{self, label, number, number_f64, truthy};
use crate::host::widget::size::Natural;
use crate::host::widget::{EditorProps, GestureMap, Rate, RulerY, SourceWindow};
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

/// **How far a drag reaches for a neighbour's edge**, in device pixels.
///
/// The same order as the grab margin a box's own edges have
/// ([`placement::EDGE_PX`]) and for the same reason: it is what a hand's aim is
/// worth on screen. Wider and a box could not be placed near another without
/// being pulled onto it; narrower and two boxes could not be made to meet
/// without zooming to the sample.
const SNAP_PX: f32 = 8.0;

/// **How short and how tall a row may be pulled**, in logical pixels. The floor
/// is a band that still holds a header's two rows; the ceiling is there so a
/// slip of the hand cannot leave a stack nobody can scroll back out of.
const MIN_LANE_H: f32 = 40.0;
const MAX_LANE_H: f32 = 8.0 * LANE_H;

/// An automation row's thickness when nothing says otherwise — shorter than a
/// lane, because what it draws is one line and not a stack of boxes.
const CURVE_H: f32 = 40.0;

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

/// The row a hand is resizing, and what it was when the press landed.
#[derive(Debug, Clone, Copy)]
struct Sizing {
    lane: usize,
    from: f32,
    at: f64,
}

/// A **level knob** the hand is on, kept for the same reason a clip's grab is:
/// the value is read from the pointer against what the press found.
#[derive(Debug, Clone, Copy)]
struct Fading {
    lane: usize,
    /// The knob's own cell — how far a full turn is, in pixels.
    cell: Rect,
    /// The level the press found: a turn is measured from it.
    from: f32,
    /// The y the press landed at, which the drag is measured against.
    at: f64,
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
    /// **The track automations**: each a row of its own under the lane it
    /// names, as long as the timeline is. A track's gain does not begin and
    /// end with a box, so it is not drawn inside one.
    pub(crate) curves: Vec<model::Curve>,
    /// **The clip envelopes**: each a layer drawn inside the box it names, over
    /// whatever that box draws and lasting exactly as long as it does.
    ///
    /// The same type as a track's automation and the same element draws it —
    /// what differs is where it hangs, which is the whole of the distinction.
    pub(crate) layers: Vec<model::Curve>,
    /// **The break-point element per curve, by name** — rows and layers alike,
    /// each the `curve` element in its body form.
    ///
    /// A curve is the one content here a hand may *edit*, so unlike a take or a
    /// roll its element is asked for presses as well as for pixels: the light
    /// views are editable layers over a read-only base, which is what a box
    /// being a window onto a picture leaves room for.
    bodies: HashMap<String, crate::host::elements::curve::Curve>,
    /// **Which layer the hand is on**, by curve name; `None` is the placement —
    /// the boxes themselves.
    ///
    /// One layer is active at a time and it is the only one that acts or offers
    /// an affordance. Here a layer has a name, so it is named: the `points:1`
    /// ordinal is what a container whose layers are anonymous falls back to.
    layer: Option<String>,
    /// **How tall each track is drawn**, by lane name — the vertical zoom a
    /// hand set by pulling a header's bottom edge.
    ///
    /// Screen state, like the scroll and the box selection: nothing on the wire
    /// sets or reports it, so it is kept here and laid over whatever a `lanes`
    /// payload says. A client that redraws its piece says `height` on every row
    /// because the wire has always carried one, and a reader who zoomed a track
    /// in must not lose it to the next fader move.
    zoom: HashMap<String, f32>,
    /// **Where each metered track's level is read from**, by lane name.
    ///
    /// A meter is a *bus*, not a value: the host reads it every frame, straight
    /// out of the shared segment, so a level that moves every block costs no
    /// message at all. What the client says is where to look — which is the
    /// only half of it a client could know, since it is the client that
    /// allocated the buses and put the meters on the track.
    ///
    /// Empty is the ordinary state: a piece nobody is playing has no meters,
    /// and a header with nothing to read draws no strip.
    meters: HashMap<String, LaneMeter>,
    /// **Which boxes wrap**, by name — the `loops` prop, a name set exactly as
    /// `hidden` is.
    ///
    /// Not a field of the `clips` septuple, because nothing here changes it: a
    /// box loops because the *piece* says so, and a report that carried the
    /// flag would be reporting a fact this widget cannot edit. What it decides
    /// here is what an edge drag may do and how the samples are drawn under a
    /// box longer than they are.
    loops: Vec<String>,
    /// Which layers are **not drawn**, by curve name. What is hidden is not
    /// edited either, so hiding the layer in hand hands it back to the
    /// placement.
    hidden: Vec<String>,
    /// The curve a press handed the drag to, and its break-points as they stood
    /// when the press landed — what says on release whether anything changed.
    holding: Option<(String, Value)>,
    /// Which clips the hand is holding, by index. **The hand's, not the
    /// piece's**: nothing on the wire sets or reports it, exactly as nothing
    /// reports which notes a roll has selected.
    pub(crate) selected: Vec<usize>,
    /// Which **track** the hand is on, by row index — the second coordinate a
    /// paste needs (the position cursor says *when*, this says *where*), and
    /// what Delete acts on when there is one.
    ///
    /// The hand's, like the box selection and for the same reason: nothing on
    /// the wire sets or reports it, because it is not a fact about the piece.
    /// One at a time — a paste has one anchor, and a mixer strip wanting
    /// several is a different question than this one.
    pub(crate) track: Option<usize>,
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
    /// **How a box of samples is drawn** — the presentation its body element
    /// takes: `"trace"` (the default) or `"spectrogram"`, the same signal seen
    /// the other way.
    ///
    /// The widget's and not the box's: every box drawn the same way is the
    /// normal case, and a box that wanted its own would be a prop nobody has
    /// asked for. It reaches the bodies through the element they are.
    pub(crate) view: Presentation,
    /// **The take bodies, by server buffer number** — what a box's picture is
    /// drawn from, one entry however many boxes read it.
    ///
    /// It is the element's because the samples are: a buffer arrives once
    /// ([`Element::bulk_of`]) and every box over it draws the same pyramid,
    /// which is what makes six views of one recording cost one download.
    ///
    /// Each is a **signal element in its body form** — the very element that
    /// stands on its own elsewhere, drawn through
    /// [`Element::draw_body`] against
    /// the box's own axis and with no chrome of its own. A box is a window onto
    /// a picture, not a second implementation of one: what changes between the
    /// standalone view and this is the axis it is handed, and nothing else.
    takes: HashMap<i32, SignalElement>,
    /// **The roll bodies, by box name** — a box whose contents are notes rather
    /// than samples.
    ///
    /// Per box and not per source, because notes are the box's: two boxes over
    /// one phrase are two windows onto it, and the wire states each whole. The
    /// element is the roll that stands on its own elsewhere, drawn through its
    /// body door with no keyboard, no strips and no chrome.
    rolls: HashMap<String, Notes>,
    /// **The samples a spectral box owes its slot**, by buffer — kept when they
    /// land and transformed by the next [`Element::fills`], which takes them.
    ///
    /// A time-frequency picture is a texture, and a texture is uploaded rather
    /// than drawn. The samples are kept rather than the analysis because the
    /// element is cloned on a redefine and an analysis is neither cloneable nor
    /// worth cloning; they are held only until the next tick asks.
    ///
    /// It is the multitrack's and not the body element's because a body over a
    /// *server buffer* resolves its samples as a pyramid — the right answer for
    /// a trace, and nothing a transform can read.
    pending: HashMap<i32, (Vec<f32>, usize)>,
    /// The drag in flight. **The state lives in the element**; the machine
    /// keeps only the sequence.
    grab: Option<Grab>,
    /// The clips a body drag is carrying, snapshotted at the press.
    block: Block,
    /// The fader a drag is on, when it is on one.
    fading: Option<Fading>,
    /// The row a drag is resizing, when it is on one.
    sizing: Option<Sizing>,
}

impl Default for Multitrack {
    fn default() -> Self {
        Self {
            lanes: Vec::new(),
            clips: Vec::new(),
            curves: Vec::new(),
            layers: Vec::new(),
            bodies: HashMap::new(),
            layer: None,
            hidden: Vec::new(),
            holding: None,
            loops: Vec::new(),
            meters: HashMap::new(),
            zoom: HashMap::new(),
            selected: Vec::new(),
            track: None,
            scroll: 0.0,
            gap: GAP,
            snap: 0.0,
            editor: EditorProps::body(),
            label: None,
            view: Presentation::Signal,
            takes: HashMap::new(),
            rolls: HashMap::new(),
            pending: HashMap::new(),
            grab: None,
            block: Vec::new(),
            fading: None,
            sizing: None,
        }
    }
}

/// The **body element** a box of samples is drawn through: the signal element
/// this build already has, named onto one server buffer and given the
/// presentation the widget asked for.
///
/// It is built through the ordinary constructor rather than by naming fields,
/// so a box's picture and a standalone `signal` are the same product of the
/// same props — which is what keeps them from drifting when either grows a
/// prop. A body carries no chrome: the ruler, the gutter and the navigation
/// belong to the view that placed it.
fn take_body(bufnum: i32, view: Presentation) -> SignalElement {
    let mut props = Map::new();
    props.insert("buffer".into(), Value::from(bufnum));
    props.insert("view".into(), Value::from(view.name()));
    crate::host::widget::signal_element(&props, &[]).unwrap_or_else(|_| {
        // The props above are this function's own and cannot be refused; the
        // fallback exists so a constructor that grows a rule does not take the
        // whole picture down with it.
        crate::host::widget::signal_element(&Map::new(), &[]).expect("a bare signal element")
    })
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
    let curves = parse_curves(props);
    let layers = parse_layers(props);
    Multitrack {
        lanes: parse_lanes(props),
        clips: parse_clips(props),
        track: None,
        bodies: curve_bodies(&curves, &layers, &parse_points(props)),
        curves,
        layers,
        layer: props.get("layer").and_then(Value::as_str).and_then(named),
        hidden: parse_hidden(props),
        loops: parse_names(props, "loops"),
        meters: parse_meters(props),
        zoom: HashMap::new(),
        holding: None,
        selected: Vec::new(),
        scroll: 0.0,
        gap: number(props, "gap", GAP).max(0.0),
        snap: number_f64(props, "snap", 0.0).max(0.0),
        editor: EditorProps::parse(props, RulerY::Off),
        sizing: None,
        label: label(props),
        view: props
            .get("view")
            .and_then(Value::as_str)
            .and_then(Presentation::parse)
            .unwrap_or(Presentation::Signal),
        takes: HashMap::new(),
        rolls: parse_notes(props)
            .into_iter()
            .map(|(name, notes)| (name, roll_body(&notes)))
            .collect(),
        pending: HashMap::new(),
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

/// **What one track's meter is read out of**: two runs of control buses, one
/// value per channel each.
///
/// Two buses and not one because a meter shows two things — the level, and the
/// mark that waits to be read — and they are the same measurement with
/// different ballistics, which is why the server writes both rather than
/// letting two clients invent two falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaneMeter {
    /// The first control bus of the level run.
    level: i32,
    /// The first control bus of the held-peak run; negative for no mark.
    mark: i32,
    /// How many channels the track has, which is how long each run is.
    channels: usize,
}

/// The `meters` prop: the flat `lane level mark channels` quadruple array.
///
/// Its own prop rather than two more fields on `lanes`, because a meter is not
/// a thing this widget reports back: the `lanes` payload is an **edit** a hand
/// made, and a bus number in it would be a bus number the host was expected to
/// return unchanged.
fn parse_meters(props: &Map<String, Value>) -> HashMap<String, LaneMeter> {
    let Some(Value::Array(items)) = props.get("meters") else {
        return HashMap::new();
    };
    items
        .as_chunks::<4>()
        .0
        .iter()
        .filter_map(|c| {
            let channels = c[3].as_i64().unwrap_or(0).max(0) as usize;
            if channels == 0 {
                return None;
            }
            Some((
                c[0].as_str()?.to_string(),
                LaneMeter {
                    level: c[1].as_i64().unwrap_or(-1) as i32,
                    mark: c[2].as_i64().unwrap_or(-1) as i32,
                    channels,
                },
            ))
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

/// The `curves` prop: the flat `name lane label min max height` sextuple array
/// — a **track automation**, a row of its own under the lane it names.
fn parse_curves(props: &Map<String, Value>) -> Vec<model::Curve> {
    let Some(Value::Array(items)) = props.get("curves") else {
        return Vec::new();
    };
    items
        .as_chunks::<6>()
        .0
        .iter()
        .filter_map(|c| {
            Some(model::Curve {
                name: c[0].as_str()?.to_string(),
                owner: c[1].as_str().unwrap_or_default().to_string(),
                label: c[2].as_str().unwrap_or_default().to_string(),
                min: c[3].as_f64().unwrap_or(0.0) as f32,
                max: c[4].as_f64().unwrap_or(1.0) as f32,
                height: c[5].as_f64().unwrap_or(f64::from(CURVE_H)) as f32,
            })
        })
        .collect()
}

/// The `layers` prop: the flat `name box label min max` quintuple array — a
/// **clip envelope**, drawn inside the box it names.
///
/// It carries no height, and that is the shape saying what it is: a layer is as
/// tall as the box it is on, and a row is as tall as it asks.
fn parse_layers(props: &Map<String, Value>) -> Vec<model::Curve> {
    let Some(Value::Array(items)) = props.get("layers") else {
        return Vec::new();
    };
    items
        .as_chunks::<5>()
        .0
        .iter()
        .filter_map(|c| {
            Some(model::Curve {
                name: c[0].as_str()?.to_string(),
                owner: c[1].as_str().unwrap_or_default().to_string(),
                label: c[2].as_str().unwrap_or_default().to_string(),
                min: c[3].as_f64().unwrap_or(0.0) as f32,
                max: c[4].as_f64().unwrap_or(1.0) as f32,
                height: CURVE_H,
            })
        })
        .collect()
}

/// The `points` prop: the flat `curve time value shape amount` quintuples,
/// gathered into the `time value shape amount` quads a curve reads.
///
/// **One list for every curve there is**, rows and layers alike, because a
/// break-point is a break-point wherever the curve hangs — the same carrier a
/// roll's `notes` rides, with the curve's name in front the way a note names
/// its box. A point naming a curve that is not there is dropped.
fn parse_points(props: &Map<String, Value>) -> HashMap<String, Vec<f64>> {
    let Some(Value::Array(items)) = props.get("points") else {
        return HashMap::new();
    };
    let mut out: HashMap<String, Vec<f64>> = HashMap::new();
    for q in items.as_chunks::<5>().0 {
        let Some(name) = q[0].as_str() else {
            continue;
        };
        out.entry(name.to_string())
            .or_default()
            .extend(q[1..].iter().map(|v| v.as_f64().unwrap_or(0.0)));
    }
    out
}

/// The `hidden` prop: the layers that are not drawn, space-separated.
fn parse_hidden(props: &Map<String, Value>) -> Vec<String> {
    parse_names(props, "hidden")
}

/// A **name set** prop: the space-separated names under `key`, which is how
/// this widget spells a flag that belongs to some of what it holds — the layers
/// that are not drawn, the boxes that wrap.
fn parse_names(props: &Map<String, Value>, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A name set as a `/gui_set` value: the same space-separated list the prop
/// takes.
fn names_of(v: &Value) -> Vec<String> {
    v.as_str()
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A layer name, or `None` for the two spellings that mean the placement — the
/// boxes themselves, which is what a multitrack owns when no curve is in hand.
fn named(v: &str) -> Option<String> {
    (!v.is_empty() && v != "placement").then(|| v.to_string())
}

/// The **body element** every curve is drawn and edited through: the `curve`
/// element this build already has, over that curve's own points and value
/// domain, with no chrome of its own.
///
/// Built once per curve rather than per placement, because the element *is* the
/// curve: a row and a layer differ in the rectangle and the span they are
/// handed at draw time, and in nothing they hold.
fn curve_bodies(
    rows: &[model::Curve],
    layers: &[model::Curve],
    points: &HashMap<String, Vec<f64>>,
) -> HashMap<String, crate::host::elements::curve::Curve> {
    rows.iter()
        .chain(layers)
        .map(|c| {
            let mut props = Map::new();
            props.insert("min".into(), Value::from(c.min));
            props.insert("max".into(), Value::from(c.max));
            if let Some(flat) = points.get(&c.name) {
                props.insert("points".into(), Value::from(flat.clone()));
            }
            (c.name.clone(), crate::host::elements::curve::body(&props))
        })
        .collect()
}

/// The notes each box carries, by box name — the flat
/// `box start dur pitch velocity channel` sextuples the wire takes, gathered
/// into the `start dur pitch velocity channel` quintuples a roll reads.
///
/// One list for the widget rather than one per box, because that is the shape
/// every payload here has: a flat list whose first fields are the identity. A
/// note naming a box that is not there is dropped — unlike a clip, which is
/// kept and drawn nowhere, because a clip is what a report is *about* and a
/// note is what one holds.
fn parse_notes(props: &Map<String, Value>) -> HashMap<String, Vec<f64>> {
    let Some(Value::Array(items)) = props.get("notes") else {
        return HashMap::new();
    };
    let mut out: HashMap<String, Vec<f64>> = HashMap::new();
    for n in items.as_chunks::<6>().0 {
        let Some(box_name) = n[0].as_str() else {
            continue;
        };
        out.entry(box_name.to_string())
            .or_default()
            .extend(n[1..].iter().map(|v| v.as_f64().unwrap_or(0.0)));
    }
    out
}

/// The **body element** a box of notes is drawn through: the roll this build
/// already has, over the notes of that one box and with every lane it draws on
/// its own turned off.
///
/// The pitch window is the crate's rule
/// ([`pitch_window`](clausters_document::view::catalogue::pitch_window)), the
/// same one a standalone roll's picture is fitted with — so a box and a window
/// over the same notes agree about how tall they are.
fn roll_body(notes: &[f64]) -> Notes {
    let (min, max) = clausters_document::view::catalogue::pitch_window(notes);
    let mut props = Map::new();
    props.insert("notes".into(), Value::from(notes.to_vec()));
    props.insert("min".into(), Value::from(min));
    props.insert("max".into(), Value::from(max));
    // A body has no chrome: no velocity lane, no marker lane, no ruler.
    props.insert("velocity".into(), Value::from(false));
    props.insert("osc_lane".into(), Value::from(false));
    props.insert("ruler".into(), Value::from("off"));
    // Read-only here, which is the line the whole widget is drawn on: the
    // multitrack places, and a box is **entered** to edit what is in it.
    props.insert("editable".into(), Value::from(false));
    crate::host::elements::notes::from_props(&props)
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
        self.stack().lane_at(rect, self.scroll, y)
    }

    /// **The vertical axis**: the lanes and the automation rows under them, in
    /// the order they are drawn. Built per ask rather than kept, because it is
    /// derived from two lists a `/gui_set` replaces whole.
    fn stack(&self) -> model::Stack {
        model::Stack::new(&self.lanes, &self.curves, self.gap)
    }

    /// Where each **lane** lands, by lane index.
    fn lane_rects(&self, rect: Rect) -> Vec<Rect> {
        self.stack().lane_rects(rect, self.scroll, self.lanes.len())
    }

    /// The lane a hand **is heading for**, always — the *drag*'s question, and
    /// the sweep's. It answers for the gaps between lanes and clamps past
    /// either end, which is the whole of why a dragged clip neither jumps nor
    /// oscillates.
    fn lane_toward(&self, rect: Rect, y: f64) -> usize {
        self.stack().lane_toward(rect, self.scroll, y)
    }

    /// The clip under `(x, y)`, and which part of it — **the topmost first**,
    /// since a later clip is drawn over an earlier one and the eye takes the
    /// one it can see.
    fn clip_at(&self, input: &Input, at: (f64, f64)) -> Option<(usize, Part)> {
        let i = self.lane_at(input.rect, at.1)?;
        let rect = self.lane_rects(input.rect)[i];
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
        let rect = self.lane_rects(input.rect)[i];
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
        let rect = self.lane_rects(input.rect)[lane];
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
            track::HeaderPart::Level => {
                let Some(cell) = parts.level else {
                    return Claim::Decline;
                };
                // **Relative**: a knob turns by the distance a drag travels,
                // and that is not a detail of the drawing -- a dial has no left
                // and right end to put the pointer between, so an absolute
                // reading would jump the value to wherever the press landed.
                // The press itself changes nothing; what it takes is the level
                // it found and the pixel it found it at.
                self.fading = Some(Fading {
                    lane,
                    cell,
                    from: self.lanes[lane].gain,
                    at: at.1,
                });
                Claim::take()
            }
            // **The bottom edge is the row's own height** — the vertical zoom
            // of one track, which is what a hand reaches for when one take
            // needs to be read closely and the rest do not. Screen state, like
            // the scroll: nothing on the wire sets or reports it.
            track::HeaderPart::Edge => {
                self.sizing = Some(Sizing {
                    lane,
                    from: self.lanes[lane].height,
                    at: at.1,
                });
                Claim::take()
            }
            // **The space beside the controls is the track itself.** A click
            // selects it -- the second coordinate a paste needs -- and a double
            // click makes one, the gesture a desktop already spends on "open
            // this" spent here on "make one", which is what a stack has no
            // other way to ask for. The new track goes **after** the one that
            // was pointed at, which is where a hand asking for one from this
            // row means it.
            track::HeaderPart::Body => {
                if input.clicks >= 2 {
                    return self.add_lane(lane + 1);
                }
                self.track = Some(lane);
                Claim::take()
            }
        }
    }

    /// **Adds a track at `at`**, reported as the rows now stand.
    ///
    /// The name is minted here, the way a split's is: it is a name the client
    /// never said, and what makes it a *new* track to whoever reads the report
    /// is precisely that it names no track they know -- the same rule a new box
    /// travels under. The client mints the id; the host mints the word.
    fn add_lane(&mut self, at: usize) -> Claim {
        let at = at.min(self.lanes.len());
        let height = self.lanes.first().map_or(LANE_H, |l| l.height);
        self.lanes
            .insert(at, Lane::new(self.fresh_lane_name(), height));
        // The hand keeps hold of what it asked for, and the boxes it was
        // holding are on rows that may have moved under them.
        self.track = Some(at);
        self.selected.clear();
        Claim::Take(Take {
            events: self.lanes_event(),
            ..Take::default()
        })
    }

    /// **Removes the selected track**, and everything on it, reported as the
    /// rows now stand.
    ///
    /// The boxes go with it and nothing says so: the rows report is the piece's
    /// tracks, and a track that is not in it is gone with its contents. So this
    /// sends one payload where a removal per box would send two and undo in
    /// two steps.
    fn remove_lane(&mut self) -> Option<Events> {
        let at = self.track?;
        if at >= self.lanes.len() {
            return None;
        }
        let name = self.lanes.remove(at).name;
        self.clips.retain(|c| c.lane != name);
        self.selected.clear();
        self.track = (!self.lanes.is_empty()).then(|| at.min(self.lanes.len() - 1));
        Some(self.lanes_event())
    }

    /// **Which of clip `n`'s grips is lit**, and where — the affordance for the
    /// resize gesture, or `None` for a box nobody is reaching for.
    ///
    /// Two answers in one, and the order is the whole of the fix. **A held edge
    /// draws its grip wherever the pointer has got to**: pulling an edge takes
    /// the pointer off the box within a pixel or two — that is what pulling an
    /// edge *is* — so asking where the pointer is made the mark blink out under
    /// the hand that was using it. What the hand is holding is known here, so it
    /// is asked first. With nothing held it is the pointer's own side, which is
    /// where an affordance belongs: lit always, every box carries two marks
    /// nobody is reaching for.
    fn lit_grip(
        &self,
        n: usize,
        cr: Rect,
        ends: (bool, bool),
        m: &Metrics,
        cursor: Option<(f64, f64)>,
    ) -> Option<(Rect, track::ClipSide)> {
        let held = self
            .grab
            .filter(|g| g.clip == n)
            .and_then(|g| match g.part {
                Part::Start => Some(track::ClipSide::Start),
                Part::End => Some(track::ClipSide::End),
                Part::Body => None,
            })
            .and_then(|side| track::clip_grip_on(cr, ends, m, side));
        held.or_else(|| {
            cursor
                .filter(|(_, cy)| *cy as f32 >= cr.y && (*cy as f32) < cr.y + cr.h)
                .and_then(|(cx, _)| track::clip_grip_at(cr, ends, m, cx as f32))
        })
    }

    /// **Lays the hand's own row heights back over what a payload says.**
    ///
    /// How tall a track is drawn is this window's and the wire carries none of
    /// it — but a `lanes` payload states a height on every row, because the
    /// prop has always had one — so a fader moved or a track added would
    /// otherwise take a reader's vertical zoom away with it. The same rule the
    /// scroll and the box selection follow, applied where the payload lands.
    fn zoom_rows(&mut self) {
        for lane in &mut self.lanes {
            if let Some(h) = self.zoom.get(&lane.name) {
                lane.height = *h;
            }
        }
        // A lane that is gone takes its height with it, the way every other
        // table here is pruned by what the piece now holds.
        self.zoom
            .retain(|name, _| self.lanes.iter().any(|l| &l.name == name));
    }

    /// **What kind of row a y is on** — a lane, an automation row, or nothing
    /// at all past either end of the stack.
    fn row_kind(&self, input: &Input, y: f64) -> Option<model::Row> {
        let stack = self.stack();
        stack.row(stack.row_at(input.rect, self.scroll, y)?)
    }

    /// **How far a snap reaches**, in the axis' own units: a few device pixels
    /// crossed to time, so it feels the same at every zoom.
    ///
    /// A **screen** distance and not a musical one, because what it does is
    /// screen work: it is the allowance a hand gets for meaning *this edge*,
    /// the same kind of number as the hit slop, and a tolerance in samples
    /// would be unreachable zoomed out and enormous zoomed in.
    fn snap_reach(&self, input: &Input) -> f64 {
        (self.time_at(input, f64::from(SNAP_PX)) - self.time_at(input, 0.0)).abs()
    }

    /// **The correction that lands a moving edge on a neighbour's**, or zero
    /// when nothing is near enough.
    ///
    /// A snap to **content**, which is what makes two boxes meetable at the
    /// sample: with no quantization a hand never lands one box exactly where
    /// another ends, so `j` never had two boxes to join. It stands beside
    /// `snap` rather than replacing it — a grid says where a beat is, this says
    /// where the music already is — and a hand that keeps pulling past the
    /// tolerance goes on through and overlaps them, which is a crossfade and
    /// legal.
    ///
    /// `moving` are the edges the hand is carrying, `held` what it is carrying
    /// them on (a box does not snap to itself), and `row` the lane whose boxes
    /// are the neighbours: an edge on another lane is another lane's business.
    fn pull_to_edge(&self, input: &Input, row: f32, held: &[usize], moving: &[f64]) -> f64 {
        let reach = self.snap_reach(input);
        if reach <= 0.0 {
            return 0.0;
        }
        let mut best = 0.0;
        let mut nearest = f64::INFINITY;
        for (i, clip) in self.clips.iter().enumerate() {
            if held.contains(&i) || (self.row(i) - row).abs() > f32::EPSILON {
                continue;
            }
            for edge in [clip.place.offset, clip.place.offset + clip.place.dur] {
                for m in moving {
                    let d = edge - m;
                    if d.abs() <= reach && d.abs() < nearest {
                        nearest = d.abs();
                        best = d;
                    }
                }
            }
        }
        best
    }

    /// **What the samples behind box `n` allow an edge to do**: how many frames
    /// there are, and whether the window may run off them.
    ///
    /// A box is a **window** onto a source, so pulling an edge past what the
    /// source holds has to answer for what is there. Three cases and one rule:
    /// a box that **loops** may be pulled anywhere (past the end is the
    /// beginning again); one that does not **stops at the last frame**, because
    /// past it there is nothing to show and nothing to play; and a box over
    /// samples nobody loaded stops at nothing, since there is no length to stop
    /// at — which is the silence the edge used to leave in every case.
    fn contents_of(&self, n: usize) -> Contents {
        let Some(clip) = self.clips.get(n) else {
            return Contents::default();
        };
        Contents {
            total: self
                .takes
                .get(&clip.source)
                .and_then(SignalElement::sample_shape)
                .map(|(_, frames)| frames as f64),
            looping: self.wraps(&clip.name),
        }
    }

    /// **Whether box `name`'s window wraps** — what the `loops` prop names.
    fn wraps(&self, name: &str) -> bool {
        self.loops.iter().any(|n| n == name)
    }

    /// A name no lane here has yet — a word, since the client's own names are
    /// ids and a word can never be mistaken for one.
    fn fresh_lane_name(&self) -> String {
        let mut n = 1;
        loop {
            let name = format!("track {n}");
            if !self.lanes.iter().any(|l| l.name == name) {
                return name;
            }
            n += 1;
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

    /// **Join the held clips that touch and read on from each other, on one
    /// lane** — a pitch is what makes two notes one voice, and a **lane** is
    /// what makes two clips joinable.
    ///
    /// A run is read over what is there, so an overlap joins as readily as a
    /// juxtaposition: two boxes sharing pixels are not two boxes to a reader.
    ///
    /// **Touching is not enough.** A join states one window over the whole
    /// span, reading the source from where the earlier box read, so two boxes
    /// that read different runs of it — fragments put back in another order, a
    /// piece whose edge was pulled to show more — cannot be said that way:
    /// joined anyway, the box played straight through material the pieces
    /// skipped and ran into silence past the end of what it read. So a pair
    /// that does not continue is left alone ([`placement::continues`]), and
    /// joining such a run is the cut the server stitches rather than a
    /// placement — see `clients/gui/PLAN.md`, "Join over fragments".
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
                && self.clips[held[j]].source == self.clips[head].source
                && placement::adjacent(self.clips[head].place, self.clips[held[j]].place, 1.0)
                && placement::continues(self.clips[head].place, self.clips[held[j]].place, 1.0)
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

    /// **Where each box is on screen, and the slice of its own span it shows** —
    /// the geometry the drawing and the texture pass both read, so a picture
    /// drawn on the mesh and one uploaded to the GPU land on the same pixels.
    ///
    /// A box whose lane is gone, whose lane is scrolled off, or which is off
    /// the window is absent rather than reported at zero size: what a caller
    /// wants is what it can draw.
    fn boxes_on_screen(
        &self,
        rect: Rect,
        indent: f32,
        metrics: &Metrics,
        time: Option<TimeSpace>,
    ) -> Vec<(usize, Rect, View)> {
        let nav = self.view(time);
        let at = self.lane_rects(rect);
        let shown = |r: Rect| r.y + r.h >= rect.y && r.y <= rect.y + rect.h;
        let mut out = Vec::new();
        for (n, clip) in self.clips.iter().enumerate() {
            let Some(i) = self.lane_of(clip).filter(|i| shown(at[*i])) else {
                continue;
            };
            let body = track::lane_body(at[i], false, indent, metrics);
            if body.w <= 0.0 || body.h <= 0.0 {
                continue;
            }
            let Some((x0, x1)) = model::clip_x(clip, body, &nav, MIN_CLIP_W) else {
                continue;
            };
            let cr = track::clip_rect(body, x0, x1);
            let local = track::clip_local_view(body, &nav, clip.place.offset, clip.place.dur, cr);
            out.push((n, cr, local));
        }
        out
    }

    /// **Where every drawn curve is, and the space it is drawn against** — the
    /// rows under their lanes and the layers inside their boxes, in the order
    /// they are drawn.
    ///
    /// One answer for the drawing and for the hit test, which is what keeps a
    /// break-point grabbed on the pixels it was painted on. A hidden layer is
    /// absent: what is not drawn is not edited either.
    ///
    /// The two placements differ in exactly two facts, and this is where they
    /// are decided. A **row** spans the whole timeline — a track's gain does not
    /// begin and end with a box — so it is handed the shared window over the
    /// piece's own extent. A **layer** spans its box, so it is handed the box's
    /// local window over the box's own duration, the same [`TimeSpace`] the base
    /// view under it draws through.
    fn curves_on_screen(
        &self,
        rect: Rect,
        indent: f32,
        metrics: &Metrics,
        time: Option<TimeSpace>,
    ) -> Vec<(&str, Rect, TimeSpace)> {
        let nav = self.view(time);
        let stack = self.stack();
        let rows = stack.rects(rect, self.scroll);
        let shown = |r: Rect| r.y + r.h >= rect.y && r.y <= rect.y + rect.h;
        let span = model::extent(&self.clips).max(nav.start + nav.len);
        let mut out = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let Some(model::Row::Curve(n)) = stack.row(i) else {
                continue;
            };
            let Some(curve) = self.curves.get(n).filter(|c| !self.is_hidden(&c.name)) else {
                continue;
            };
            if !shown(*row) {
                continue;
            }
            let body = track::lane_body(*row, false, indent, metrics);
            if body.w <= 0.0 || body.h <= 0.0 {
                continue;
            }
            out.push((
                curve.name.as_str(),
                body,
                self.space(nav, span, &curve.name),
            ));
        }
        for (n, cr, local) in self.boxes_on_screen(rect, indent, metrics, time) {
            let clip = &self.clips[n];
            for curve in &self.layers {
                if curve.owner != clip.name || self.is_hidden(&curve.name) {
                    continue;
                }
                let mut space = self.space(local, clip.place.dur, &curve.name);
                space.window = SourceWindow {
                    start: clip.place.start,
                    looping: self.wraps(&clip.name),
                    ..SourceWindow::default()
                };
                out.push((curve.name.as_str(), cr, space));
            }
        }
        out
    }

    /// The space a curve is drawn against, with the one fact a container
    /// decides for its layers: **whether this is the active one**.
    fn space(&self, view: View, span: f64, name: &str) -> TimeSpace {
        let mut space = TimeSpace::of(view, span);
        space.active = self.layer.as_deref() == Some(name);
        space
    }

    /// Whether a layer is one of the ones that are not drawn.
    fn is_hidden(&self, name: &str) -> bool {
        self.hidden.iter().any(|h| h == name)
    }

    /// The gesture a curve reads its own geometry from: the rectangle it was
    /// drawn in, and the axis it was drawn against. A body has no gutter of its
    /// own — the header band is the multitrack's — so the indent is zero.
    fn on_curve<'a>(input: &Input<'a>, rect: Rect, space: TimeSpace) -> Input<'a> {
        Input {
            rect,
            indent: 0.0,
            time: Some(space),
            ..*input
        }
    }

    /// **The edit-back for the curves: every break-point of every one of
    /// them.** The third of the payloads, and the whole list for the same
    /// reason the other two are whole: applying what came back is the identity,
    /// and its own inverse is the list that was there.
    fn points_event(&self) -> Events {
        Events::message(self.points_args())
    }

    /// The `"points"` payload's arguments, so a press that both moves the layer
    /// and edits it reports two messages rather than choosing one.
    fn points_args(&self) -> Vec<OscType> {
        let mut args = vec![OscType::String("points".into())];
        for curve in self.curves.iter().chain(&self.layers) {
            let Some(body) = self.bodies.get(&curve.name) else {
                continue;
            };
            for p in body.points() {
                args.push(OscType::String(curve.name.clone()));
                args.push(OscType::Float(p.time as f32));
                args.push(OscType::Float(p.value));
                args.push(OscType::Int(p.shape));
                args.push(OscType::Float(p.curve));
            }
        }
        args
    }

    /// The `points` prop as a `/gui_set` would take it: every break-point of
    /// every curve, each naming the curve it is on.
    fn points_json(&self) -> Value {
        let mut out = Vec::new();
        for curve in self.curves.iter().chain(&self.layers) {
            let Some(body) = self.bodies.get(&curve.name) else {
                continue;
            };
            for p in body.points() {
                out.push(Value::from(curve.name.clone()));
                out.push(Value::from(p.time));
                out.push(Value::from(p.value));
                out.push(Value::from(p.shape));
                out.push(Value::from(p.curve));
            }
        }
        Value::Array(out)
    }

    /// **The layer the hand is on**, reported when a press moved it.
    fn layer_args(&self) -> Vec<OscType> {
        vec![
            OscType::String("layer".into()),
            OscType::String(
                self.layer
                    .clone()
                    .unwrap_or_else(|| "placement".to_string()),
            ),
        ]
    }

    /// Where a curve by name was drawn, and the space it was drawn against —
    /// the geometry a gesture on it is read with, taken from the one answer the
    /// drawing used.
    fn curve_place(&self, name: &str, input: &Input) -> Option<(Rect, TimeSpace)> {
        self.curves_on_screen(input.rect, input.indent, input.metrics, input.time)
            .into_iter()
            .find(|(n, ..)| *n == name)
            .map(|(_, rect, space)| (rect, space))
    }

    /// The break-points of a curve as one comparable value — what says on
    /// release whether the gesture changed anything, since a drag that came
    /// back to where it began is not an edit.
    fn points_of(&self, name: &str) -> Value {
        self.bodies
            .get(name)
            .map(|b| crate::host::graphics::bpf::points_json(b.points()))
            .unwrap_or(Value::Null)
    }

    /// The bodies a new list of curves gets: **the elements that survive keep
    /// their points**, so renaming a lane or adding a curve does not flatten
    /// the ones that were already drawn.
    fn rebuilt(
        &self,
        rows: &[model::Curve],
        layers: &[model::Curve],
    ) -> HashMap<String, crate::host::elements::curve::Curve> {
        let kept = rows
            .iter()
            .chain(layers)
            .filter_map(|c| {
                let flat = self
                    .bodies
                    .get(&c.name)?
                    .points()
                    .iter()
                    .flat_map(|p| {
                        [
                            p.time,
                            f64::from(p.value),
                            f64::from(p.shape),
                            f64::from(p.curve),
                        ]
                    })
                    .collect();
                Some((c.name.clone(), flat))
            })
            .collect();
        curve_bodies(rows, layers, &kept)
    }

    /// **A press on a curve's own contents** — a break-point, or the line
    /// between two of them — never on the rectangle it shares with what is
    /// under it.
    ///
    /// That is what leaves the background to the container: a press on a box's
    /// empty pixels moves the box and takes the hand off the envelope drawn
    /// across it. **The active layer is asked first**, so what is already in
    /// hand keeps the pixels it draws on.
    fn curve_at(&self, at: (f64, f64), input: &Input) -> Option<String> {
        let drawn = self.curves_on_screen(input.rect, input.indent, input.metrics, input.time);
        let ask = |name: &str, rect: Rect, space: TimeSpace| {
            let body = self.bodies.get(name)?;
            body.layer_hit(at, &Self::on_curve(input, rect, space))
                .then(|| name.to_string())
        };
        drawn
            .iter()
            .find(|(name, ..)| self.layer.as_deref() == Some(*name))
            .and_then(|&(name, rect, space)| ask(name, rect, space))
            .or_else(|| {
                drawn
                    .iter()
                    .rev()
                    .find_map(|&(name, rect, space)| ask(name, rect, space))
            })
    }

    /// A press the curves answered, or `None` for one none of them wanted.
    ///
    /// The layer moves to whatever was pressed and is reported once; the edit
    /// itself leaves on release, as every gesture here does. What the curve
    /// reports for itself is dropped: its payload is its own points, and the
    /// payload here is **every** curve's, so forwarding one would hand an owner
    /// a list that is not the piece.
    fn press_curve(&mut self, at: (f64, f64), input: &Input) -> Option<Claim> {
        let name = self.curve_at(at, input)?;
        let (rect, mut space) = self.curve_place(&name, input)?;
        let moved = self.layer.as_deref() != Some(name.as_str());
        // The press is read as the active layer's, since that is what it just
        // became -- the curve offers a segment's bend only when it is in hand.
        space.active = true;
        self.layer = Some(name.clone());
        let before = self.points_of(&name);
        let sub = Self::on_curve(input, rect, space);
        let claim = self.bodies.get_mut(&name)?.press(at, &sub);
        let Claim::Take(take) = claim else {
            // The curve wanted none of it after all: the press goes on to the
            // box, and the layer it moved to stays where it moved.
            return moved.then(|| Claim::events(Events::message(self.layer_args())));
        };
        self.holding = Some((name.clone(), before.clone()));
        let mut events = Events::none();
        if moved {
            events = events.and(self.layer_args());
        }
        if self.points_of(&name) != before {
            events = events.and(self.points_args());
        }
        Some(Claim::Take(Take {
            events,
            edge_scroll: true,
            ..take
        }))
    }

    /// The lane header a lane's own props ask for. Presence-driven, like every
    /// header here: a lane that carries no mixer state offers no controls.
    fn header(&self, lane: &Lane, indent: f32) -> track::Header {
        track::Header {
            w: (indent > 0.0).then_some(indent),
            mute: Some(lane.mute),
            solo: Some(lane.solo),
            level: Some(lane.gain),
            // **Silent, and the right length**: the strip's width follows the
            // channel count and nothing else, so a hit test lays the header out
            // exactly where the drawing did without reading a bus.
            meters: self
                .meters
                .get(&lane.name)
                .map_or_else(Vec::new, |m| vec![(0.0, 0.0); m.channels]),
        }
    }

    /// The same header with the **levels read**, which only a draw can do: the
    /// values are in the shared segment and are one atomic load each, so a
    /// meter costs a frame's read rather than a message.
    fn live_header(&self, lane: &Lane, ctx: &Ctx) -> track::Header {
        let mut header = self.header(lane, ctx.indent);
        let Some(meter) = self.meters.get(&lane.name) else {
            return header;
        };
        for (channel, slot) in header.meters.iter_mut().enumerate() {
            let channel = channel as i32;
            *slot = (
                ctx.world.level(meter.level + channel, Rate::Control),
                if meter.mark < 0 {
                    0.0
                } else {
                    ctx.world.level(meter.mark + channel, Rate::Control)
                },
            );
        }
        header
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
                self.zoom_rows();
                true
            }
            "clips" => {
                self.clips = parse_clips(&parse::as_array_props("clips", v));
                self.selected.clear();
                true
            }
            // **The track automations**: rows of their own under the lanes
            // they name, replaced whole like every other list here.
            "curves" => {
                self.curves = parse_curves(&parse::as_array_props("curves", v));
                self.bodies = self.rebuilt(&self.curves, &self.layers);
                true
            }
            // **The clip envelopes**: layers inside the boxes they name.
            "layers" => {
                self.layers = parse_layers(&parse::as_array_props("layers", v));
                self.bodies = self.rebuilt(&self.curves, &self.layers);
                true
            }
            // The break-points of every curve there is, in one list: a curve
            // the list says nothing about is emptied, because the payload is
            // the whole of them and its own inverse is what was there.
            "points" => {
                let points = parse_points(&parse::as_array_props("points", v));
                for (name, body) in &mut self.bodies {
                    let flat = points.get(name).cloned().unwrap_or_default();
                    body.set("points", &Value::from(flat));
                }
                true
            }
            "layer" => {
                self.layer = v.as_str().and_then(named);
                true
            }
            "hidden" => {
                self.hidden = names_of(v);
                true
            }
            "loops" => {
                self.loops = names_of(v);
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
        let at = self.lane_rects(ctx.rect);
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
                    &self.live_header(lane, ctx),
                    false,
                    ctx.indent,
                    self.track == Some(i),
                );
            }
        }
        // **A track automation is a row of its own**, under the lane it names
        // and with no boxes on it: what it draws runs the whole timeline the
        // track does, so it is a row and not a layer.
        let stack = self.stack();
        for (i, row) in stack.rects(ctx.rect, self.scroll).iter().enumerate() {
            let Some(model::Row::Curve(n)) = stack.row(i) else {
                continue;
            };
            let Some(curve) = self.curves.get(n) else {
                continue;
            };
            if shown(*row) {
                track::draw(
                    d,
                    *row,
                    Some(curve.shown()),
                    &track::Header {
                        w: (ctx.indent > 0.0).then_some(ctx.indent),
                        mute: None,
                        solo: None,
                        level: None,
                        meters: Vec::new(),
                    },
                    false,
                    ctx.indent,
                    // A curve's row is the track's picture, not the track: what
                    // is selected is the track, and its own header says so.
                    false,
                );
            }
        }
        // **One pass over the clips, each onto the lane it names.** A clip
        // whose lane is gone draws nowhere and is still held, which is what
        // lets it come back in a report to be re-homed.
        for (n, cr, local) in self.boxes_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time) {
            let clip = &self.clips[n];
            track::draw_clip(d, cr, self.selected.contains(&n));
            // **The take, drawn from the source per visible pixel**, mapped
            // back through the clip's own window onto it — which is what makes
            // the picture scroll and trim *with* the box instead of squashing
            // into whatever rectangle it currently has. One pyramid however
            // many clips read it.
            // **The base view is what its contents are.** Samples draw as the
            // signal element's body, notes as the roll's — the very elements
            // that stand on their own elsewhere, handed the box's own axis and
            // drawing no chrome of their own. A box with neither draws its
            // frame and nothing in it, which is the honest picture of a window
            // onto something nobody loaded.
            let space = TimeSpace::of(local, clip.place.dur).with_window(SourceWindow {
                start: clip.place.start,
                // A box longer than its samples **wraps** where the piece says
                // it loops, and shows nothing past their end where it does not:
                // the picture is what the box reads, and it reads this.
                looping: self.wraps(&clip.name),
                ..SourceWindow::default()
            });
            if let Some(take) = self.takes.get(&clip.source) {
                take.draw_body(d, cr, &space);
            } else if let Some(roll) = self.rolls.get(&clip.name) {
                roll.draw_body(d, cr, &space);
            }
            track::draw_clip_label(d, cr, clip.shown());
            // **The grips are drawn where they are grabbed.** An end that is
            // off screen has no grip, because a handle for an edge nobody can
            // see is a handle for nothing — the same rule a lane's clip keeps.
            // **A grip is an affordance, so it is shown where the hand is.**
            // Drawn always, every clip carries two marks nobody is reaching
            // for; drawn on the side the pointer is over, it says *this edge
            // moves* at the moment that is worth saying.
            //
            // **And a held edge draws its grip wherever the pointer has got
            // to.** Pulling an edge takes the pointer off the box within a
            // pixel or two -- that is what pulling an edge *is* -- so asking
            // where the pointer is made the mark disappear under the hand that
            // was using it. What the hand is holding is known here, so it is
            // asked first: the affordance stops lying about being reachable.
            let ends = track::clip_ends_on_screen(&local, clip.place.dur);
            if let Some((grip, side)) = self.lit_grip(n, cr, ends, ctx.metrics, ctx.world.cursor) {
                track::draw_clip_grip(d, grip, side);
            }
        }
        // **The light views, over the base ones.** A curve is drawn last of
        // the contents, whether it is a row of its own or a layer inside a box:
        // it is the one thing here a hand may edit, and it has to be on top of
        // what it shapes to be reached.
        for (name, rect, space) in
            self.curves_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time)
        {
            if let Some(body) = self.bodies.get(name) {
                body.draw_body(d, rect, &space);
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
            // **Two lines, and they mean two things**: the position cursor is
            // where the reader put the mark, the playhead is where the music
            // is. The cursor goes down first, so where they coincide it is the
            // playhead that reads.
            for (pos, role) in [(time.cursor, false), (time.head, true)] {
                if let Some(pos) = pos
                    && let Some(x) = track::playhead_x(over, &nav, pos)
                {
                    let (mesh, m, theme) = d.parts();
                    let color = if role { theme.playhead } else { theme.cursor };
                    mesh.rect(Rect::new(x, over.y, m.trace_w, over.h), color);
                }
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
        self.sizing = None;
        self.holding = None;
        self.block.clear();
        // The header band first: it is drawn over the gutter, and nothing of
        // the axis is there.
        if let Some((lane, part)) = self.header_at(input, at) {
            return self.press_header(lane, part, at, input);
        }
        if at.0 < f64::from(input.rect.x + input.indent) {
            // **An automation row's header is the track's picture, not the
            // track.** A curve is drawn in a row of its own under the lane it
            // belongs to, and the band beside it is that row's label -- so a
            // press there addresses no track: it selects none, lets go of none
            // and asks for none. The press is consumed rather than declined,
            // because the header band is this widget's whatever is drawn in it.
            if matches!(self.row_kind(input, at.1), Some(model::Row::Curve(_))) {
                return Claim::take();
            }
            // **The band under the last header**, where there is no track to
            // point at -- so nothing else could be meant by a double click
            // there than *make one*, and it goes at the end. A single click
            // lets go of the track the hand had, the way a click on bare stack
            // lets go of the boxes.
            if input.clicks >= 2 {
                return self.add_lane(self.lanes.len());
            }
            self.track = None;
            return Claim::take();
        }
        // **A press selects the layer it lands on**, and what lands on a curve
        // is its own points and the line between them — never the rectangle it
        // shares with the box under it. So an envelope drawn across a box
        // leaves that box draggable by every pixel the line is not on.
        if let Some(claim) = self.press_curve(at, input) {
            return claim;
        }
        let Some((clip, part)) = self.clip_at(input, at) else {
            return Claim::Decline;
        };
        // **A box is entered to edit it**, and entering is a double click —
        // the gesture a desktop already spends on "open this". What leaves is
        // the box's name and nothing else: which editor that box asks for is a
        // question about its *contents*, and this widget owns where things are
        // rather than what is inside them.
        if input.clicks >= 2 {
            return Claim::events(Events::message(vec![
                OscType::String("enter".into()),
                OscType::String(self.clips[clip].name.clone()),
            ]));
        }
        // **Alt adds or removes that one**, the same key that adds a note to a
        // roll's selection. A plain click selects it alone, and that is decided
        // on release (see [`Element::release`]): a press is not yet a gesture.
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
        // A curve in hand follows it, and reports nothing on the way: what it
        // says for itself is one curve's points, and one gesture is one edit.
        if let Some((name, _)) = self.holding.clone()
            && let Some((rect, space)) = self.curve_place(&name, input)
        {
            let sub = Self::on_curve(input, rect, space);
            if let Some(body) = self.bodies.get_mut(&name) {
                body.drag(at, &sub);
            }
            return Events::none();
        }
        if let Some(s) = self.sizing {
            let height = (s.from + (at.1 - s.at) as f32).clamp(MIN_LANE_H, MAX_LANE_H);
            self.lanes[s.lane].height = height;
            self.zoom.insert(self.lanes[s.lane].name.clone(), height);
            return Events::none();
        }
        if let Some(f) = self.fading {
            self.lanes[f.lane].gain = track::level_after(f.from, at.1 - f.at, f.cell);
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
                // **The grabbed box's own two edges look for a neighbour.** The
                // box under the hand is what the hand is aiming with, so it is
                // the one that snaps; the rest of a block travels with it, as
                // it does for everything else a block drag does.
                let orig = grab.orig;
                let dt = dt
                    + self.pull_to_edge(
                        input,
                        self.row(grab.clip) + dr,
                        &self.block.iter().map(|&(i, ..)| i).collect::<Vec<_>>(),
                        &[orig.offset + dt, orig.offset + orig.dur + dt],
                    );
                let rows = (0.0, self.lanes.len().saturating_sub(1) as f32);
                let block = std::mem::take(&mut self.block);
                placement::move_block(self, &block, dt, dr, rows, None);
                self.block = block;
            }
            // **An edge is one clip's**, and it trims: the placement and the
            // window over the contents move together.
            part => {
                // An edge looks for a neighbour too: that is how a gap is
                // closed by trimming rather than by moving.
                let now = now + self.pull_to_edge(input, self.row(grab.clip), &[grab.clip], &[now]);
                let contents = self.contents_of(grab.clip);
                self.clips[grab.clip].place =
                    placement::drag(part, now, grab.orig, contents, self.bounds());
            }
        }
        Events::none()
    }

    /// **A gesture that changed nothing is not an edit.** A press and a release
    /// with nothing in between is a click, and a drag that came back to where
    /// it began is the same thing by another road: reporting it would hand the
    /// owner an intent to apply and a document an entry to undo, so looking at
    /// four clips would cost four undos.
    fn release(&mut self, at: (f64, f64), inside: bool, input: &Input) -> Events {
        if let Some((name, before)) = self.holding.take() {
            if let Some((rect, space)) = self.curve_place(&name, input) {
                let sub = Self::on_curve(input, rect, space);
                if let Some(body) = self.bodies.get_mut(&name) {
                    body.release(at, inside, &sub);
                }
            }
            return if self.points_of(&name) == before {
                Events::none()
            } else {
                self.points_event()
            };
        }
        if self.sizing.take().is_some() {
            // Nothing leaves: how tall a row is drawn is this window's, and the
            // piece is not asked about it.
            return Events::none();
        }
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
            // **A press that moved nothing is a click, and a click selects the
            // box it landed on** — alone, whatever was held before, which is
            // what makes a hand able to point at one clip and then act on it
            // (place the cursor, split it, delete it). Alt is still the
            // additive one, and it answered at the press.
            //
            // It is decided here rather than at the press because a press is
            // not yet a gesture: the same movement is a click or a drag
            // depending on what happens next, and collapsing the selection at
            // the press would let go of a block the hand was about to move.
            //
            // Nothing leaves: a selection is the hand's, not the composition's.
            self.selected = vec![grab.clip];
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
    /// run, Delete removes (the **selected track**, with everything on it, when
    /// no box is held), and `Ctrl`+`C`/`X`/`V` move a block through the
    /// host-wide clipboard. All of them act on **the held set**, across the
    /// stack, and all of them report the clips as they now stand: there is one
    /// payload here and a verb does not get to invent a second.
    ///
    /// **The letters are the ones a clip already answered to on a lane.** Which
    /// keys they are is not settled — see `clients/gui/PLAN.md`, "A shortcut is
    /// the application's, not the widget's".
    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        // **Delete acts on what is in hand, and a track can be in hand.** The
        // header is what puts one there, so with a track selected Delete is the
        // track's -- it and everything on it -- and with none it is the held
        // boxes', which is what it has always been. The ordinary rule, and the
        // reason the selected track is not merely decoration.
        if matches!(key, Key::Delete | Key::Backspace)
            && self.selected.is_empty()
            && self.track.is_some()
        {
            return self.remove_lane();
        }
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
                // **A paste needs two coordinates**, and the second is the
                // selected track: the position cursor says *when* and the
                // header says *where*. The earliest box lands on the selected
                // track and the rest keep their distances from it, in rows as
                // in time -- a block pasted onto a track is the same block, so
                // what is kept is its shape and not the row numbers it was cut
                // from. With no track selected the rows are the ones it came
                // from, which is what a paste back into the same piece means.
                let rows: Vec<usize> = block.iter().map(|c| self.lane_of(c).unwrap_or(0)).collect();
                let earliest = block
                    .iter()
                    .position(|c| c.place.offset <= first + f64::EPSILON)
                    .unwrap_or(0);
                let base = rows.get(earliest).copied().unwrap_or(0);
                let onto = self.track.unwrap_or(base);
                let last = self.lanes.len().saturating_sub(1);
                self.selected.clear();
                for (i, mut clip) in block.into_iter().enumerate() {
                    clip.place.offset = (clip.place.offset - first + at).max(0.0);
                    let row = (rows[i] + onto).saturating_sub(base).min(last);
                    if let Some(lane) = self.lanes.get(row) {
                        clip.lane = lane.name.clone();
                    }
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

    /// **The empty bars after the last clip are ordinary time.** A view of a
    /// signal stops at its last sample; a piece is composed into the space
    /// after what it already holds, so the axis is not bounded by the boxes on
    /// it — which is also where its authoring headroom comes from.
    fn unbounded_axis(&self) -> bool {
        true
    }

    /// **The spectral boxes**, each over its own take's texture: a
    /// time-frequency picture samples one, so it is drawn in the GPU pass and
    /// not into the mesh — and this element holds several, one per buffer its
    /// boxes are windows onto, which is why they are named by a key.
    ///
    /// Empty unless the widget's `view` asks for one, and empty for a box whose
    /// take has not arrived: a picture of nothing is the frame around it.
    fn texture_bodies(&self, ctx: &Ctx) -> Vec<TextureBody> {
        self.boxes_on_screen(ctx.rect, ctx.indent, ctx.metrics, ctx.time)
            .into_iter()
            .filter_map(|(n, rect, local)| {
                let clip = self.clips.get(n)?;
                let look = self.takes.get(&clip.source)?.texture_body()?;
                Some(TextureBody {
                    key: SlotKey(i64::from(clip.source)),
                    rect,
                    local,
                    look,
                })
            })
            .collect()
    }

    /// **What each take body has for its slot**, under the buffer it draws.
    ///
    /// One entry per source rather than per box: the analysis is the take's, so
    /// six boxes over one recording are one upload — the same rule that makes
    /// them one download.
    fn fills(&mut self) -> Vec<(SlotKey, SlotFill)> {
        let pending: Vec<(i32, (Vec<f32>, usize))> = self.pending.drain().collect();
        pending
            .into_iter()
            .filter_map(|(bufnum, (samples, channels))| {
                let body = self.takes.get(&bufnum)?;
                let stfts = crate::host::frame::stft_channels(
                    crate::host::frame::deinterleave(&samples, channels),
                    body.spectral.fft_size,
                    body.spectral.hop,
                    body.editor.sample_rate,
                );
                Some((SlotKey(i64::from(bufnum)), SlotFill::Texture(stfts)))
            })
            .collect()
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
        // The strip is part of the band, so the widest metered track is part of
        // what the group's indent has to hold -- otherwise the meters would be
        // drawn over the names rather than beside them.
        let widest = self.meters.values().map(|m| m.channels).max().unwrap_or(0);
        let header = track::Header {
            w: None,
            mute: Some(false),
            solo: Some(false),
            level: Some(1.0),
            meters: vec![(0.0, 0.0); widest],
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
        // **A metered track asks for its buses**, which is what makes the
        // window animate: the declaration is the subscription, the same way a
        // `meter` widget's rate is.
        let mut buses = Vec::new();
        for meter in self.meters.values() {
            for channel in 0..meter.channels as i32 {
                buses.push(meter.level + channel);
                if meter.mark >= 0 {
                    buses.push(meter.mark + channel);
                }
            }
        }
        buses.sort_unstable();
        buses.dedup();
        Needs {
            takes,
            buses,
            // **A swept line is a picture driven by the clock**, so the window
            // has to be told: an anchored playhead moves with no message and
            // nothing else would ask for the frame it moves on.
            clock: self.editor.playhead_at >= 0.0,
            ..Needs::default()
        }
    }

    /// One of them arrived. Every box over it draws the same pyramid.
    ///
    /// The samples are handed to the **body element** for that buffer, built
    /// here on first sight: it is a signal element like any other, so what
    /// resolving a pyramid means is its answer and not a second copy of one.
    fn bulk_of(&mut self, bufnum: i32, data: Loaded) -> bool {
        let view = self.view;
        // **The transform happens here for a spectral box**, because a body
        // over a server buffer resolves its samples as a pyramid — the right
        // answer for a trace, and nothing a transform can read. The loader does
        // exactly this for a standalone spectral view, from the same samples.
        if view == Presentation::TimeFrequency
            && let Loaded::Raw { samples, channels } = &data
        {
            self.pending
                .insert(bufnum, (samples.clone(), (*channels).max(1)));
        }
        // **The unlabelled door**, because a body element asked for one source
        // and not for several: the plural ask is this widget's, and by the time
        // the samples reach the body they are the only ones it wanted.
        self.takes
            .entry(bufnum)
            .or_insert_with(|| take_body(bufnum, view))
            .bulk(data)
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
            ("curves".into(), model::curves_json(&self.curves)),
            ("layers".into(), model::layers_json(&self.layers)),
            ("points".into(), self.points_json()),
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
            clicks: 1,
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
        let at = mt.lane_rects(rect);
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

    /// **A metered track asks for its buses and reads them.** The declaration
    /// is the subscription -- nothing else would ask the window for the frames
    /// a level moves on -- and what a strip shows is the bus, read where it
    /// stands rather than sent per block.
    #[test]
    fn a_metered_track_declares_its_buses_and_reads_them() {
        struct Buses;
        impl crate::host::BusSource for Buses {
            fn control(&self, index: usize) -> f32 {
                // Bus 10 is unity, 11 is silence, 12 is the mark above them.
                match index {
                    10 => 1.0,
                    12 => 1.0,
                    _ => 0.0,
                }
            }
            fn level(&self, _bus: i32) -> f32 {
                0.0
            }
        }

        let mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1.0, "two", "", 100, 0, 0, 1.0],
                "meters": ["one", 10, 12, 2]}"#,
        ));
        assert_eq!(
            mt.needs().buses,
            vec![10, 11, 12, 13],
            "both runs, both channels, and nothing for the track with no meter"
        );

        let buses = Buses;
        let world = crate::host::world::World {
            bus: Some(&buses),
            ..Default::default()
        };
        let metrics = crate::host::metrics::Metrics::default();
        let ctx = Ctx {
            world: &world,
            metrics: &metrics,
            rect: Rect::new(0.0, 0.0, 800.0, 200.0),
            indent: 120.0,
            scale: 1.0,
            time: None,
            clip: None,
            focused: false,
        };
        let live = mt.live_header(&mt.lanes[0], &ctx);
        assert_eq!(live.meters, vec![(1.0, 1.0), (0.0, 0.0)]);
        assert!(
            mt.live_header(&mt.lanes[1], &ctx).meters.is_empty(),
            "a track with no meter draws no strip"
        );
    }

    /// **The strip is laid out from the channel count alone**, which is what
    /// lets a press land on the pixels a control was drawn on: the hit test
    /// never reads a bus, so its header must still be the width the drawing's
    /// was.
    #[test]
    fn a_meter_takes_its_width_from_the_channels_and_not_from_the_level() {
        let mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1.0], "meters": ["one", 10, 12, 2]}"#,
        ));
        let m = crate::host::metrics::Metrics::default();
        let plain = from_props(&props(r#"{"lanes": ["one", "", 100, 0, 0, 1.0]}"#));
        assert!(
            mt.gutter(&m) > plain.gutter(&m),
            "the band holds the strip beside the name rather than over it"
        );

        let mut quiet = mt.header(&mt.lanes[0], 0.0);
        let loud = {
            let mut h = quiet.clone();
            h.meters = vec![(1.0, 1.0); 2];
            h
        };
        assert_eq!(
            quiet.width(&m),
            loud.width(&m),
            "a level is not a size: a moving meter would move the name under it"
        );
        quiet.meters.clear();
        assert!(
            quiet.width(&m) < loud.width(&m),
            "and no meter takes no room"
        );
    }

    /// **A box's base view is what its contents are**, and it is drawn by the
    /// element that draws it anywhere else: samples through the signal
    /// element's body door, notes through the roll's. What the widget adds is
    /// the axis — a box is a window onto a picture, never a second
    /// implementation of one.
    #[test]
    fn a_box_of_notes_is_a_roll_and_a_box_of_samples_is_a_take() {
        let mt = from_props(&props(
            r#"{
            "lanes": ["one", "", 100, 0, 0, 1.0],
            "clips": ["a", "one", 0, 48000, 0, "", 0,
                      "b", "one", 96000, 48000, 0, "", -1],
            "notes": ["b", 0.0, 4800.0, 60.0, 100.0, 0.0,
                      "b", 4800.0, 4800.0, 72.0, 100.0, 0.0]
        }"#,
        ));
        assert!(mt.rolls.contains_key("b"), "the box the notes named");
        assert!(!mt.rolls.contains_key("a"), "and no other");
        assert!(
            mt.takes.is_empty(),
            "a take body is built when its samples arrive, not before"
        );
    }

    /// **A spectral box is a texture of its own**, named by the take it draws:
    /// a time-frequency picture samples one, so it goes to the GPU pass rather
    /// than into the mesh, and this element holds one per buffer its boxes are
    /// windows onto.
    #[test]
    fn a_spectral_box_names_the_take_its_texture_is() {
        use crate::host::widget::element::SlotKey;
        let mut mt = from_props(&props(
            r#"{"view": "spectrogram",
                "lanes": ["one", "", 100, 0, 0, 1.0],
                "clips": ["a", "one", 0, 48000, 0, "", 3]}"#,
        ));
        let world = crate::host::world::World::default();
        let metrics = crate::host::metrics::Metrics::default();
        let ctx = Ctx {
            world: &world,
            metrics: &metrics,
            rect: Rect::new(0.0, 0.0, 800.0, 200.0),
            indent: 0.0,
            scale: 1.0,
            time: None,
            clip: None,
            focused: false,
        };
        assert!(
            mt.texture_bodies(&ctx).is_empty(),
            "a picture of nothing is the frame around it"
        );
        mt.bulk_of(
            3,
            Loaded::Raw {
                samples: vec![0.0; 4096],
                channels: 1,
            },
        );
        let bodies = mt.texture_bodies(&ctx);
        assert_eq!(bodies.len(), 1, "one box, one picture");
        assert_eq!(bodies[0].key, SlotKey(3), "named by the take it draws");
        let fills = mt.fills();
        assert_eq!(fills.len(), 1, "and one upload, however many boxes read it");
        assert_eq!(fills[0].0, SlotKey(3));
    }

    /// The pitch window a roll body is fitted to is the crate's rule, so a box
    /// and a window over the same notes are the same height.
    #[test]
    fn a_roll_body_is_fitted_by_the_crates_own_rule() {
        let notes = [0.0, 4800.0, 60.0, 100.0, 0.0];
        let (min, max) = clausters_document::view::catalogue::pitch_window(&notes);
        let mt = from_props(&props(
            r#"{"clips": ["b", "one", 0, 48000, 0, "", -1],
                "notes": ["b", 0.0, 4800.0, 60.0, 100.0, 0.0]}"#,
        ));
        let roll = mt.rolls.get("b").expect("the roll");
        assert_eq!((roll.range().0, roll.range().1), (min as f32, max as f32));
    }

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

    /// A piece with both kinds of curve: a track automation under `noise` and
    /// an envelope inside the box `a`.
    fn curved() -> Multitrack {
        from_props(&props(
            r#"{"lanes": ["noise", "", 100, 0, 0, 1, "tone", "", 100, 0, 0, 1],
                "clips": ["a", "noise", 0, 500, 0, "", 0, "b", "tone", 500, 500, 0, "", 0],
                "curves": ["gain", "noise", "Gain", 0, 1, 40],
                "layers": ["env", "a", "", 0, 1],
                "points": ["gain", 0, 1, 1, 0, "gain", 1000, 0, 1, 0,
                           "env", 0, 0, 1, 0, "env", 500, 1, 1, 0]}"#,
        ))
    }

    /// **The same element in two places, and the places are the difference.**
    /// A track automation takes a row of its own under its lane and runs the
    /// whole timeline; a clip envelope is a layer inside its box and runs as
    /// long as the box does.
    #[test]
    fn an_automation_is_a_row_and_an_envelope_is_a_layer() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
        let mt = curved();
        assert_eq!(mt.bodies.len(), 2, "one element per curve, however placed");

        // The row is under `noise`, and the second lane sits below it.
        let stack = mt.stack();
        assert_eq!(stack.len(), 3);
        assert_eq!(stack.row(1), Some(model::Row::Curve(0)));
        assert_eq!(mt.lane_rects(rect)[1].y, 100.0 + 40.0 + 2.0 * GAP);

        let drawn = mt.curves_on_screen(rect, 100.0, &m, mt_time(500.0));
        let row = drawn.iter().find(|(n, ..)| *n == "gain").expect("the row");
        let layer = drawn.iter().find(|(n, ..)| *n == "env").expect("the layer");
        assert_eq!(row.2.span, 1000.0, "a row is as long as the piece");
        assert_eq!(layer.2.span, 500.0, "a layer is as long as its box");
        assert!(layer.1.w < row.1.w, "the box is narrower than the timeline");
    }

    /// The shared axis a curved piece is read against.
    fn mt_time(len: f64) -> Option<TimeSpace> {
        Some(TimeSpace::of(
            View {
                start: 0.0,
                len: len * 2.0,
            },
            len * 2.0,
        ))
    }

    /// **A hidden layer is not drawn, and what is not drawn is not edited.**
    #[test]
    fn hiding_a_layer_takes_it_out_of_the_picture_and_out_of_reach() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
        let mut mt = curved();
        assert!(mt.set("hidden", &Value::from("env gain")));
        assert!(
            mt.curves_on_screen(rect, 100.0, &m, mt_time(500.0))
                .is_empty(),
            "neither is drawn"
        );
    }

    /// **A press lands on a curve's own points, never on the rectangle it
    /// shares** — so an envelope drawn across a box leaves the box draggable,
    /// and the press that misses the line moves the box instead.
    #[test]
    fn a_press_beside_the_line_still_moves_the_box() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
        let len = 1000.0;
        let mut mt = curved();
        // The envelope on `a` runs from its floor to its ceiling, so the top
        // left corner of the box is far from the line.
        let at = mt.lane_rects(rect);
        let body = track::lane_body(at[0], false, 100.0, &m);
        let corner = (f64::from(body.x) + 2.0, f64::from(at[0].y) + 3.0);
        assert!(matches!(
            mt.press(corner, &input(&m, rect, len)),
            Claim::Take(_)
        ));
        assert!(mt.grab.is_some(), "the box took it");
        assert!(mt.holding.is_none(), "and no curve did");
        assert_eq!(mt.layer, None, "the placement is still what is in hand");
    }

    /// **A break-point is grabbed on the pixels it was drawn on**, and moving
    /// one reports every curve there is — the payload is the piece's, so a
    /// `/gui_set points` of what came back is the identity.
    #[test]
    fn dragging_a_point_reports_every_curve() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
        let len = 1000.0;
        let mut mt = curved();
        let (_, row, space) = *mt
            .curves_on_screen(rect, 100.0, &m, mt_time(500.0))
            .iter()
            .find(|(n, ..)| *n == "gain")
            .expect("the row");
        // The first point of `gain` is at time zero and value one: the top
        // left of its row.
        let start = space.view.start;
        let x = f64::from(row.x) + (0.0 - start) / space.view.len * f64::from(row.w);
        let from = (x, f64::from(row.y) + 1.0);
        let inp = Input {
            time: mt_time(500.0),
            ..input(&m, rect, len)
        };
        assert!(matches!(mt.press(from, &inp), Claim::Take(_)));
        assert_eq!(
            mt.layer.as_deref(),
            Some("gain"),
            "the press took the layer"
        );
        let to = (x, f64::from(row.y + row.h) - 1.0);
        mt.drag(to, &inp);
        let msgs = mt.release(to, true, &inp).into_messages();
        let args = msgs.first().expect("an edit");
        assert_eq!(args[0], OscType::String("points".into()));
        assert_eq!(
            args.len(),
            1 + 5 * 4,
            "the tag, then a quintuple per point of every curve"
        );
        assert_eq!(args[1], OscType::String("gain".into()));
        assert_eq!(args[11], OscType::String("env".into()), "the layer too");
    }

    /// **A box is entered with a double click**, and what leaves is its name.
    ///
    /// The widget owns *where* things are and not what is inside them, so it
    /// says which box was opened and stops there: which editor that box asks
    /// for is a question about its contents, and whoever holds the piece is
    /// the one that can answer it.
    #[test]
    fn a_second_press_on_a_box_enters_it() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let on_a = xy(&mt, &m, rect, 250.0, len, 0);

        let once = input(&m, rect, len);
        assert!(matches!(mt.press(on_a, &once), Claim::Take(_)));
        assert!(mt.grab.is_some(), "one press grabs the box");

        let twice = Input {
            clicks: 2,
            ..input(&m, rect, len)
        };
        let Claim::Take(take) = mt.press(on_a, &twice) else {
            panic!("the second press is taken");
        };
        let msgs = take.events.into_messages();
        let args = msgs.first().expect("one message");
        assert_eq!(args[0], OscType::String("enter".into()));
        assert_eq!(args[1], OscType::String("a".into()));
        assert!(mt.grab.is_none(), "and nothing is being dragged");
    }

    /// **A track is zoomed vertically by pulling its header's bottom edge**, and
    /// the height a hand set survives what the piece says next: a `lanes`
    /// payload states a height on every row, so a fader moved or a track added
    /// would otherwise take the zoom away with it.
    #[test]
    fn a_track_is_zoomed_by_its_bottom_edge_and_keeps_it() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
        let len = 1000.0;
        let mut mt = piece();
        let inp = input(&m, rect, len);
        let band = crate::host::timeline::gutter_band(mt.lane_rects(rect)[0], 100.0);
        let edge = (f64::from(band.x) + 4.0, f64::from(band.y + band.h) - 1.0);
        let was = mt.lanes[0].height;

        assert!(matches!(mt.press(edge, &inp), Claim::Take(_)));
        assert_eq!(mt.lanes[0].height, was, "the press resizes nothing");
        mt.drag((edge.0, edge.1 + 60.0), &inp);
        assert_eq!(mt.lanes[0].height, was + 60.0, "the row follows the hand");
        assert_eq!(mt.lanes[1].height, was, "and only that row");
        assert!(
            mt.release((edge.0, edge.1 + 60.0), true, &inp)
                .into_messages()
                .is_empty(),
            "how tall a row is drawn is this window's: the piece is not asked"
        );

        // The client redraws its rows -- a fader moved, a track added -- and
        // the height a hand set is still the height.
        assert!(mt.set(
            "lanes",
            &serde_json::json!(["noise", "", 100, 0, 0, 0.5, "tone", "", 100, 0, 0, 1]),
        ));
        assert_eq!(
            mt.lanes[0].height,
            was + 60.0,
            "the zoom outlived the payload"
        );
        assert_eq!(mt.lanes[0].gain, 0.5, "and the payload landed");

        // A row that is gone takes its height with it.
        assert!(mt.set("lanes", &serde_json::json!(["tone", "", 100, 0, 0, 1])));
        assert!(mt.zoom.is_empty());
    }

    /// **A paste needs two coordinates, and the second is the selected track.**
    /// The position cursor says *when* and the header says *where*, so a block
    /// lands on the track a hand pointed at and keeps its own shape from there
    /// — in rows as in time.
    #[test]
    fn a_paste_lands_on_the_selected_track_and_keeps_its_shape() {
        let mut clipboard = crate::host::clipboard::Clip::default();
        let mut mt = piece();
        mt.selected = vec![0, 1];
        let ctrl = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let mut keys = |mt: &mut Multitrack, key: Key, cursor: Option<f64>| {
            mt.key(
                &key,
                &mut KeyInput {
                    mods: ctrl,
                    clipboard: &mut clipboard,
                    cursor,
                },
            )
        };
        keys(&mut mt, Key::Char('c'), None).expect("copied");

        // With a track in hand the block lands on it: the earliest box on the
        // selected track, the rest keeping their distance from it.
        mt.track = Some(1);
        keys(&mut mt, Key::Char('v'), Some(100.0)).expect("pasted");
        assert_eq!(mt.clips.len(), 4);
        assert_eq!(mt.clips[2].lane, "tone", "the earliest onto the selection");
        assert_eq!(
            mt.clips[3].lane, "tone",
            "and the second kept its distance -- there is no third lane to fall on"
        );
        assert_eq!(mt.clips[2].place.offset, 100.0, "on the cursor");

        // With none, the rows are the ones it came from: a paste back into the
        // same piece is the block where it was.
        let mut mt = piece();
        mt.selected = vec![0, 1];
        keys(&mut mt, Key::Char('c'), None).expect("copied");
        mt.track = None;
        keys(&mut mt, Key::Char('v'), Some(0.0)).expect("pasted");
        assert_eq!(mt.clips[2].lane, "noise");
        assert_eq!(mt.clips[3].lane, "tone");
    }

    /// **An automation row's header is the track's picture, not the track.** A
    /// curve is drawn in a row of its own under the lane it belongs to, so a
    /// press on the band beside it addresses no track: it selects none, lets go
    /// of none, and asks for none.
    #[test]
    fn an_automation_rows_header_addresses_no_track() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 400.0);
        let len = 1000.0;
        let mut mt = curved();
        let inp = input(&m, rect, len);
        // The curve row is the second entry of the stack: `noise`, its `gain`
        // row, then `tone`.
        let rows = mt.stack().rects(rect, 0.0);
        let curve_row = rows[1];
        let on = (f64::from(rect.x) + 4.0, f64::from(curve_row.y) + 2.0);

        mt.track = Some(0);
        assert!(matches!(mt.press(on, &inp), Claim::Take(_)));
        assert_eq!(mt.track, Some(0), "the track a hand had is still in hand");
        assert_eq!(mt.lanes.len(), 2, "and nothing was added");

        // Nor does a double click there ask for a track: the band under the
        // *last* header is where there is nothing to point at.
        let twice = Input {
            clicks: 2,
            ..input(&m, rect, len)
        };
        mt.press(on, &twice);
        assert_eq!(mt.lanes.len(), 2, "a curve row is not empty header space");
    }

    /// **The header's level is a knob, and a knob turns by a drag.** A header
    /// is a narrow band and a groove long enough to be read takes the width the
    /// name needs; a dial reads and turns in the space there actually is. The
    /// press changes nothing -- a dial has no left and right end to put the
    /// pointer between -- and the turn is measured from where it landed.
    #[test]
    fn the_headers_level_is_a_knob_and_turns_by_the_drag() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        mt.lanes[0].gain = 0.5;
        let inp = input(&m, rect, len);
        let at = mt.lane_rects(rect);
        let band = crate::host::timeline::gutter_band(at[0], 100.0);
        let parts = track::header_parts(band, &mt.header(&mt.lanes[0], 100.0), &m);
        let cell = parts.level.expect("the lane offers a level");
        assert!(
            (cell.w - cell.h).abs() < 1.0,
            "a square cell, like the toggles beside it"
        );

        let on = (
            f64::from(cell.x + cell.w * 0.5),
            f64::from(cell.y + cell.h * 0.5),
        );
        assert!(matches!(mt.press(on, &inp), Claim::Take(_)));
        assert_eq!(mt.lanes[0].gain, 0.5, "the press turns nothing");

        // Up raises, and by the distance travelled rather than to where the
        // pointer is.
        mt.drag((on.0, on.1 - f64::from(cell.h)), &inp);
        assert!(mt.lanes[0].gain > 0.5, "a turn upward raises it");
        let raised = mt.lanes[0].gain;
        mt.drag((on.0, on.1 + f64::from(cell.h)), &inp);
        assert!(mt.lanes[0].gain < raised, "and back down lowers it");
        // It reports as it goes, like every other control.
        assert!(!mt.lanes_event().into_messages().is_empty());
    }

    /// **A drag snaps to the edges of the boxes already on the lane**, which is
    /// what makes two of them meetable at the sample: with no quantization a
    /// hand never lands one box exactly where another ends, so `j` never had
    /// two boxes to join. A hand that keeps pulling past the tolerance goes on
    /// through and overlaps them, which is a crossfade and legal.
    #[test]
    fn a_drag_meets_the_box_beside_it_and_j_joins_the_two() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        // The two halves of a cut: `b` reads on from where `a` stops, which is
        // the other half of what a join needs.
        let mut mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1],
                "clips": ["a", "one", 0, 200, 0, "", 0, "b", "one", 500, 200, 200, "", 0]}"#,
        ));
        let inp = input(&m, rect, len);

        // Drag `b` back to *near* where `a` ends: near enough to mean it, and
        // not near enough for a hand to land it by aim.
        let from = xy(&mt, &m, rect, 550.0, len, 0);
        let to = xy(&mt, &m, rect, 253.0, len, 0);
        assert!(matches!(mt.press(from, &inp), Claim::Take(_)));
        mt.drag(to, &inp);
        assert_eq!(
            mt.clips[1].place.offset, 200.0,
            "it met the box beside it exactly"
        );

        // Pull well past the tolerance and it goes on through: an overlap is a
        // crossfade, not a refusal.
        let over = xy(&mt, &m, rect, 400.0, len, 0);
        mt.drag(over, &inp);
        assert_eq!(
            mt.clips[1].place.offset, 350.0,
            "the hand went on through: an overlap is a crossfade, not a refusal"
        );
        mt.release(over, true, &inp);

        // Meeting is what `j` needs: with the two touching, one box comes out.
        mt.clips[1].place.offset = 200.0;
        mt.selected = vec![0, 1];
        let mut clipboard = crate::host::clipboard::Clip::default();
        let joined = mt
            .key(
                &Key::Char('j'),
                &mut KeyInput {
                    mods: Mods::default(),
                    clipboard: &mut clipboard,
                    cursor: None,
                },
            )
            .expect("two boxes that touch join");
        assert_eq!(
            joined.into_messages()[0][0],
            OscType::String("clips".into())
        );
        assert_eq!(mt.clips.len(), 1);
        assert_eq!(mt.clips[0].place.offset, 0.0);
        assert_eq!(mt.clips[0].place.dur, 400.0, "the two spans, whole");
    }

    /// **Touching is not enough**: two boxes that read *different* runs of a
    /// source cannot be said in one window, so `j` leaves them alone.
    ///
    /// A join states one window over the whole span, reading the source from
    /// where the earlier box read. Joined anyway, fragments put back in another
    /// order played straight through material they skipped and ran into silence
    /// past the end of what they read -- audio nobody asked for, in place of
    /// audio somebody cut.
    #[test]
    fn boxes_that_do_not_read_on_from_each_other_do_not_join() {
        let mut mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1],
                "clips": ["a", "one", 0, 200, 400, "", 0, "b", "one", 200, 200, 0, "", 0]}"#,
        ));
        mt.selected = vec![0, 1];
        let mut clipboard = crate::host::clipboard::Clip::default();
        assert!(
            mt.key(
                &Key::Char('j'),
                &mut KeyInput {
                    mods: Mods::default(),
                    clipboard: &mut clipboard,
                    cursor: None,
                },
            )
            .is_none(),
            "the second reads the source's head where the first left off at 600"
        );
        assert_eq!(mt.clips.len(), 2, "and both boxes are still there");

        // Two over **different sources** are the same case: a box is a window
        // onto one of them.
        let mut mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1],
                "clips": ["a", "one", 0, 200, 0, "", 0, "b", "one", 200, 200, 200, "", 1]}"#,
        ));
        mt.selected = vec![0, 1];
        assert!(
            mt.key(
                &Key::Char('j'),
                &mut KeyInput {
                    mods: Mods::default(),
                    clipboard: &mut clipboard,
                    cursor: None,
                },
            )
            .is_none(),
            "one window names one source"
        );
        assert_eq!(mt.clips.len(), 2);
    }

    /// **A box is a window onto a source, and an edge stops where the source
    /// does** -- unless the piece says the box wraps, where past the end is the
    /// beginning again. The `loops` prop is that statement, a name set like
    /// `hidden`, and what it decides is what a drag may do and what is drawn
    /// under a box longer than its samples.
    #[test]
    fn a_box_that_loops_may_be_pulled_past_its_source_and_one_that_does_not_may_not() {
        let mt = from_props(&props(
            r#"{"lanes": ["one", "", 100, 0, 0, 1],
                "clips": ["a", "one", 0, 500, 0, "", 0, "b", "one", 500, 500, 0, "", 0],
                "loops": "a"}"#,
        ));
        assert!(mt.wraps("a"), "the piece said this one wraps");
        assert!(!mt.wraps("b"));
        assert!(mt.contents_of(0).looping);
        assert!(!mt.contents_of(1).looping);
        // Nobody loaded the samples here, so there is no length to stop at --
        // which is the silence an edge used to leave in every case.
        assert!(mt.contents_of(1).total.is_none());
    }

    /// **The trim grip stops blinking.** An edge drag takes the pointer off the
    /// box it is resizing -- that is what pulling an edge is -- so a mark drawn
    /// only where the pointer is disappeared under the hand that was using it.
    #[test]
    fn a_held_edge_draws_its_grip_wherever_the_pointer_went() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let inp = input(&m, rect, len);
        let (cr, ends) = {
            let (n, cr, local) = mt
                .boxes_on_screen(rect, 100.0, &m, inp.time)
                .into_iter()
                .find(|(n, ..)| *n == 0)
                .expect("the first box is on screen");
            assert_eq!(n, 0);
            (
                cr,
                track::clip_ends_on_screen(&local, mt.clips[0].place.dur),
            )
        };
        let ends_x = f64::from(cr.x + cr.w) - 1.0;
        let midy = f64::from(cr.y + cr.h * 0.5);

        // Nothing held: the pointer's own side, and nothing off the box.
        assert!(mt.lit_grip(0, cr, ends, &m, Some((ends_x, midy))).is_some());
        assert!(mt.lit_grip(0, cr, ends, &m, None).is_none());
        let outside = (ends_x + 40.0, midy + 80.0);
        assert!(mt.lit_grip(0, cr, ends, &m, Some(outside)).is_none());

        // Held: the edge in hand keeps its mark wherever the pointer got to.
        assert!(matches!(mt.press((ends_x, midy), &inp), Claim::Take(_)));
        assert_eq!(mt.grab.expect("an edge").part, Part::End);
        let (_, side) = mt
            .lit_grip(0, cr, ends, &m, Some(outside))
            .expect("the held edge is still lit");
        assert_eq!(side, track::ClipSide::End);
        // And the box that is *not* held draws nothing from someone else's grab.
        assert!(mt.lit_grip(1, cr, ends, &m, Some(outside)).is_none());
    }

    /// **The header is a surface, and the track is what it addresses.** A click
    /// on the space beside its three controls selects that track -- the second
    /// coordinate a paste needs, since the position cursor only says *when* --
    /// a double click there makes one, and Delete takes the selected one away
    /// with everything on it.
    #[test]
    fn the_header_selects_a_track_makes_one_and_delete_takes_it_away() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let inp = input(&m, rect, len);
        // The band beside the controls, on the second lane's row: the top of it,
        // which is where the name is drawn and no control is.
        let at = mt.lane_rects(rect);
        let beside = |i: usize| (f64::from(rect.x) + 4.0, f64::from(at[i].y) + 2.0);

        assert!(matches!(mt.press(beside(1), &inp), Claim::Take(_)));
        assert_eq!(
            mt.track,
            Some(1),
            "a click on the header points at the track"
        );

        // A double click on a header asks for a track, and it lands after the
        // one the hand was pointing at.
        let twice = Input {
            clicks: 2,
            ..input(&m, rect, len)
        };
        let Claim::Take(take) = mt.press(beside(0), &twice) else {
            panic!("the header takes it");
        };
        let msgs = take.events.into_messages();
        let args = msgs.first().expect("the rows as they now stand");
        assert_eq!(args[0], OscType::String("lanes".into()));
        assert_eq!(args.len(), 1 + 3 * 6, "three rows, six numbers each");
        assert_eq!(mt.lanes.len(), 3);
        assert_eq!(mt.lanes[1].name, "track 1", "a word, never an id");
        assert_eq!(mt.track, Some(1), "and the hand holds what it asked for");

        // Delete with a track in hand is the track's, and the boxes on it go
        // with it -- one payload, because the rows report is the piece's tracks
        // and a track that is not in it is gone with its contents.
        mt.track = Some(0);
        let mut clipboard = crate::host::clipboard::Clip::default();
        let events = mt
            .key(
                &Key::Delete,
                &mut KeyInput {
                    mods: Mods::default(),
                    clipboard: &mut clipboard,
                    cursor: None,
                },
            )
            .expect("a track in hand is what Delete acts on");
        let msgs = events.into_messages();
        assert_eq!(msgs[0][0], OscType::String("lanes".into()));
        assert_eq!(mt.lanes.len(), 2);
        assert!(
            !mt.clips.iter().any(|c| c.lane == "noise"),
            "and the boxes on it went with it"
        );
    }

    /// A double click on bare stack enters nothing: there is no box there, and
    /// the press goes back to the container the way a single one does.
    #[test]
    fn a_double_press_off_the_boxes_still_declines() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let bare = xy(&mt, &m, rect, 800.0, len, 0);
        let twice = Input {
            clicks: 2,
            ..input(&m, rect, len)
        };
        assert!(matches!(mt.press(bare, &twice), Claim::Decline));
    }

    /// A `/gui_set` of what a query reported is the identity, for the curves as
    /// for the two lists that were here before them.
    #[test]
    fn the_curves_read_back_as_they_were_reported() {
        let mt = curved();
        let reported: Map<String, Value> = mt.info().into_iter().collect();
        let mut echo = Multitrack::default();
        assert!(echo.set("lanes", &reported["lanes"]));
        assert!(echo.set("clips", &reported["clips"]));
        assert!(echo.set("curves", &reported["curves"]));
        assert!(echo.set("layers", &reported["layers"]));
        assert!(echo.set("points", &reported["points"]));
        assert_eq!(echo.curves, mt.curves);
        assert_eq!(echo.layers, mt.layers);
        assert_eq!(echo.points_json(), mt.points_json());
    }

    /// A curve that survives a new list keeps its points: replacing the
    /// declarations is not an edit of what they hold.
    #[test]
    fn a_curve_that_survives_a_redeclaration_keeps_its_points() {
        let mut mt = curved();
        let before = mt.points_of("gain");
        assert!(mt.set(
            "curves",
            &Value::from(r#"["gain", "noise", "Gain", 0, 1, 60, "pan", "tone", "", -1, 1, 30]"#)
        ));
        assert_eq!(mt.points_of("gain"), before);
        assert_eq!(mt.curves[0].height, 60.0, "and takes its new row height");
        assert!(mt.bodies.contains_key("pan"), "the new one is built empty");
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

    /// **A click selects the box it landed on, alone.** A press is not yet a
    /// gesture — the same movement is a click or a drag depending on what
    /// happens next — so it is decided on release: a press that moved nothing
    /// meant *this one*, and a hand that can point at a box is a hand that can
    /// then place the cursor and split it.
    #[test]
    fn a_click_on_a_box_selects_that_one_and_nothing_leaves() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        mt.selected = vec![0, 1];

        // Press and release on the same pixel: the block is let go of and the
        // box under the hand is what is held.
        let on_a = xy(&mt, &m, rect, 250.0, len, 0);
        mt.press(on_a, &input(&m, rect, len));
        let events = mt.release(on_a, true, &input(&m, rect, len));
        assert_eq!(mt.selected, vec![0], "the one it landed on, alone");
        assert!(
            events.into_messages().is_empty(),
            "a selection is the hand's, not the composition's"
        );
        assert_eq!(mt.clips[0].place.offset, 0.0, "and nothing moved");

        // A press that *did* move is a drag, and a drag reports the piece.
        let over = xy(&mt, &m, rect, 350.0, len, 0);
        mt.press(on_a, &input(&m, rect, len));
        mt.drag(over, &input(&m, rect, len));
        let events = mt.release(over, true, &input(&m, rect, len));
        assert!(!events.into_messages().is_empty(), "a move is an edit");
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
        let at = mt.lane_rects(rect);
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

    /// **A cut half is a clip like any other, and it changes lanes.** The old
    /// projection stopped emitting lane changes once a split had happened,
    /// because the half was a box the view had minted and the gesture state
    /// still named the lane the press had captured. Here the halves are clips
    /// on the stack the drag reads, so the second one drags across like the
    /// first.
    #[test]
    fn a_half_left_by_a_split_still_changes_lanes() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();
        let mut clipboard = crate::host::clipboard::Clip::default();
        mt.selected = vec![0];
        mt.key(
            &Key::Char('e'),
            &mut KeyInput {
                mods: Mods::default(),
                clipboard: &mut clipboard,
                cursor: Some(200.0),
            },
        )
        .expect("it cut");

        let from = xy(&mt, &m, rect, 350.0, len, 0);
        let to = xy(&mt, &m, rect, 350.0, len, 1);
        mt.press(from, &input(&m, rect, len));
        mt.drag(to, &input(&m, rect, len));
        mt.release(to, true, &input(&m, rect, len));

        let half = mt
            .clips
            .iter()
            .find(|c| c.name == "a 2")
            .expect("the second half");
        assert_eq!(half.lane, "tone", "the half went to the lane under it");
        assert_eq!(half.place.offset, 200.0, "and it did not move in time");
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

        let at = mt.lane_rects(rect);
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

        let body = track::lane_body(mt.lane_rects(rect)[0], false, 100.0, &m);
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
    /// **A held grip is shown because it is held.** The edge moves under the
    /// hand, so asking where the pointer is each frame makes the mark blink as
    /// the box catches up with it — which is what a trim looked like.
    #[test]
    fn the_grip_a_drag_is_holding_does_not_depend_on_the_pointer() {
        let m = Metrics::default();
        let rect = Rect::new(0.0, 0.0, 600.0, 220.0);
        let len = 1000.0;
        let mut mt = piece();

        let body = track::lane_body(mt.lane_rects(rect)[0], false, 100.0, &m);
        let nav = View { start: 0.0, len };
        let (x0, x1) = model::clip_x(&mt.clips[0], body, &nav, MIN_CLIP_W).expect("on screen");
        let cr = track::clip_rect(body, x0, x1);
        let (_, right) = {
            let local = track::clip_local_view(
                body,
                &nav,
                mt.clips[0].place.offset,
                mt.clips[0].place.dur,
                cr,
            );
            let ends = track::clip_ends_on_screen(&local, mt.clips[0].place.dur);
            track::clip_grips(cr, ends, &m)
        };
        let right = right.expect("its end is on screen");
        let on_grip = (
            f64::from(right.x + right.w / 2.0),
            f64::from(cr.y + cr.h / 2.0),
        );

        mt.press(on_grip, &input(&m, rect, len));
        assert!(matches!(mt.grab.map(|g| g.part), Some(Part::End)));
        // Pull it well left: the edge is now nowhere near where the press was,
        // and the drag still knows which side it took.
        mt.drag((on_grip.0 - 60.0, on_grip.1), &input(&m, rect, len));
        assert!(mt.clips[0].place.dur < 500.0);
        assert!(
            matches!(mt.grab.map(|g| g.part), Some(Part::End)),
            "the side is the drag's, not the pointer's"
        );
    }
}
