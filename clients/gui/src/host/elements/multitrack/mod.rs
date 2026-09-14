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
use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::host::elements::notes::Notes;
use crate::host::elements::signal::{Presentation, SignalElement};
use crate::host::font;
use crate::host::graphics::multitrack as stack;
use crate::host::graphics::track;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::structures::boxes::{self, Bounds, Contents, Part, Placement, Placements};
use crate::host::structures::clips::{self as model, Clip, Lane};
use crate::host::widget::element::{
    Claim, Ctx, Element, Events, Input, Key, KeyInput, Loaded, Needs, OnAxis, Samples, SlotFill,
    SlotKey, Slotted, Swept, Take, TextureBody, TimeSpace,
};
use crate::host::widget::parse::{self, label, number, number_f64, truthy};
use crate::host::widget::size::Natural;
use crate::host::widget::{EditorProps, GestureMap, Rate, RulerY, SourceWindow};
use crate::viewport::View;

// **One application, five questions.** The file was 4650 lines holding a model,
// its props, its picture, its hit-testing, its gestures, its verbs and its
// tests -- every one of them there for a reason, and none of them findable.
// Split the way `interact` and `gestures` were split: by the question a reader
// arrives with, since that is what a reader has one of at a time. `mod.rs` is
// the type, what it holds and the table of what it answers; the bodies live
// with their question.
mod draw;
mod hand;
mod props;
mod rows;
mod verbs;

// The children share one type and see each other's helpers: what one question
// needs of another's answer is named rather than re-derived.
use hand::{Block, Fading, Grab, Sizing};
pub(crate) use props::build;
use props::{LaneMeter, curve_bodies, json_arg, parse_clips, take_body};

#[cfg(test)]
mod tests;

/// The gap between lanes, in logical pixels, when the props name none.
const GAP: f32 = 4.0;

/// A lane's thickness when nothing says otherwise.
const LANE_H: f32 = 96.0;

/// **How far a drag reaches for a neighbour's edge**, in device pixels.
///
/// The same order as the grab margin a box's own edges have
/// ([`boxes::EDGE_PX`]) and for the same reason: it is what a hand's aim is
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

/// **How short an automation row may be pulled.** Lower than a lane's floor,
/// which has to hold a header's two rows: a curve row draws one line and a
/// label, and what it must not become is a row nobody can put a point on.
const MIN_CURVE_H: f32 = 16.0;

/// How far one wheel notch moves the stack, in logical pixels. A fraction of a
/// row rather than a row: a stack scrolls under the hand, and a wheel that
/// jumped a whole track would make a tall one unreachable in the middle.
const WHEEL_ROWS: f32 = 48.0;

/// The narrowest a clip's box is drawn at, so one nobody can see never becomes
/// one nobody can grab.
const MIN_CLIP_W: f32 = 3.0;

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
    /// **How tall each automation row is drawn**, by curve name — the same
    /// screen state [`Multitrack::zoom`] is, for the other kind of row.
    ///
    /// A table of its own rather than one keyed by "whatever the row is called"
    /// because the two names come out of one id space: a lane is named by its
    /// track's id and a curve by its automation's, and nothing stops a piece
    /// from having both. One table would make zooming a row silently resize an
    /// unrelated one, which is the kind of defect nobody finds by reading.
    curve_zoom: HashMap<String, f32>,
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
    /// (`Element::bulk_of`) and every box over it draws the same pyramid,
    /// which is what makes six views of one recording cost one download.
    ///
    /// Each is a **signal element in its body form** — the very element that
    /// stands on its own elsewhere, drawn through
    /// [`Element::draw_body`] against
    /// the box's own axis and with no chrome of its own. A box is a window onto
    /// a picture, not a second implementation of one: what changes between the
    /// standalone view and this is the axis it is handed, and nothing else.
    takes: HashMap<i32, SignalElement>,
    /// **The takes already asked for**, by server buffer number.
    ///
    /// A front's per-repaint walk asks for what is new in here and nothing
    /// else ([`Samples::ask_takes`]): asking for every take on every frame
    /// re-mapped and re-summarized each one per repaint, and on a leg that
    /// downloads, started the download again the moment it finished. A take
    /// leaves this set when its samples are known to have changed under it
    /// ([`Samples::forget_take`]), which is how it is asked for again.
    asked: HashSet<i32>,
    /// **The roll bodies, by box name** — a box whose contents are notes rather
    /// than samples.
    ///
    /// Per box and not per source, because notes are the box's: two boxes over
    /// one phrase are two windows onto it, and the wire states each whole. The
    /// element is the roll that stands on its own elsewhere, drawn through its
    /// body door with no keyboard, no strips and no chrome.
    rolls: HashMap<String, Notes>,
    /// **The samples a spectral box owes its slot**, by buffer — kept when they
    /// land and transformed by the next [`Slotted::fills`], which takes them.
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
            curve_zoom: HashMap::new(),
            selected: Vec::new(),
            track: None,
            scroll: 0.0,
            gap: GAP,
            snap: 0.0,
            editor: EditorProps::body(),
            label: None,
            view: Presentation::Signal,
            takes: HashMap::new(),
            asked: HashSet::new(),
            rolls: HashMap::new(),
            pending: HashMap::new(),
            grab: None,
            block: Vec::new(),
            fading: None,
            sizing: None,
        }
    }
}

impl Multitrack {}

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

/// **And the two questions a verb asks of the list itself.**
///
/// A clip's identity is its **name**, which is what a report names it by and
/// what a correction finds it again by, so a second box may not be a copy of
/// the first in the one field that says which box it is. `fresh_name` is that
/// rule and it is the whole of what is specific here — the cut itself, the
/// halves' spans and the window each keeps onto its source are the arithmetic
/// every box on a time axis shares.
impl boxes::Holder for Multitrack {
    fn duplicate(&mut self, i: usize) -> Option<usize> {
        let mut copy = self.clips.get(i)?.clone();
        copy.name = self.fresh_name(&copy.name);
        self.clips.push(copy);
        Some(self.clips.len() - 1)
    }

    fn discard(&mut self, indices: &[usize]) {
        let mut held: Vec<usize> = indices.to_vec();
        held.sort_unstable();
        held.dedup();
        for i in held.into_iter().rev() {
            if i < self.clips.len() {
                self.clips.remove(i);
            }
        }
    }
}

/// **What the widget answers**, and where each answer lives.
///
/// The table rather than the answers: a trait method that is more than a few
/// lines delegates to an inherent one in the module that holds its question --
/// `props` for what a `/gui_set` means, `draw` for the picture, `hand` for
/// every phase of a gesture. What stays here is what is one line long, plus the
/// four facet doors, so a reader can see *what this element is* without reading
/// what it does.
impl Element for Multitrack {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        self.apply_prop(key, v)
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        self.paint(d, ctx)
    }
    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        self.press_at(at, input)
    }
    fn select_in(&mut self, from: (f64, f64), to: (f64, f64), input: &Input) -> Swept {
        self.swept(from, to, input)
    }
    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        self.dragged(at, input)
    }
    fn release(&mut self, at: (f64, f64), inside: bool, input: &Input) -> Events {
        self.released(at, inside, input)
    }
    fn wheel(&mut self, at: (f64, f64), delta: (f64, f64), input: &Input) -> Option<Events> {
        self.wheeled(at, delta, input)
    }

    fn accepts_focus(&self) -> bool {
        true
    }
    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        self.keyed(key, input)
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

    fn samples(&self) -> Option<&dyn Samples> {
        Some(self)
    }

    fn samples_mut(&mut self) -> Option<&mut dyn Samples> {
        Some(self)
    }

    fn on_axis(&self) -> Option<&dyn OnAxis> {
        Some(self)
    }

    fn on_axis_mut(&mut self) -> Option<&mut dyn OnAxis> {
        Some(self)
    }

    fn slotted(&self) -> Option<&dyn Slotted> {
        Some(self)
    }

    fn slotted_mut(&mut self) -> Option<&mut dyn Slotted> {
        Some(self)
    }
}

impl OnAxis for Multitrack {
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
            curves: Some(true),
            meters: vec![(0.0, 0.0); widest],
        };
        header.width(m)
    }

    /// How far the piece reaches on the axis — what an autofit and a scroll
    /// size themselves against.
    fn content_span(&self) -> Option<f64> {
        Some(model::extent(&self.clips))
    }
}

impl Slotted for Multitrack {
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
}

impl Samples for Multitrack {
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

    fn ask_takes(&mut self, all: bool) -> Vec<i32> {
        let takes = self.needs().takes;
        let fresh = takes
            .iter()
            .copied()
            .filter(|bufnum| all || !self.asked.contains(bufnum))
            .collect();
        self.asked.extend(takes);
        fresh
    }

    fn forget_take(&mut self, bufnum: i32) -> bool {
        self.asked.remove(&bufnum)
    }
}
