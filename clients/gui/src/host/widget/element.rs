//! **The element seam**: one object-safe trait a leaf implements, and the
//! registry that maps a wire `type` onto it.
//!
//! The schema around this module is a closed sum type
//! ([`WidgetKind`](super::WidgetKind)) whose variants the whole renderer
//! matches on, which is right for the containers —
//! the layout pass has to know them — and wrong for the leaves: adding one
//! edits every pass, and a program *linking* this crate cannot add one at all.
//! This is the other door. A leaf implements [`Element`], registers a
//! constructor under a wire name ([`register`]), and the passes that are a
//! match arm for a built-in are a method call for it:
//!
//! | The pass a built-in spells as an arm | The element spells as |
//! |---|---|
//! | [`build`](super::build) | the registered [`Constructor`] |
//! | [`apply`](super::apply) | [`Element::set`] |
//! | [`size`](super::size) | [`Element::natural`] |
//! | the frame's flat draw | [`Element::draw`] |
//! | the frame's GPU slots | a [`SlotKind`] claimed in [`Needs`], drawn by [`Element::slot`] and fed by [`Element::fill`] |
//! | the query pass | [`Element::value`] / [`Element::info`] |
//! | the press walk | [`Element::press`] |
//! | the keyboard arms + the host's focused field | [`Element::accepts_focus`] / [`Element::key`] |
//! | the tree collectors | [`Element::needs`], with [`Element::tap_frames`] sizing a page's tap subscription |
//! | a clip's body draw | [`Element::draw_body`], or [`Element::texture_body`] for the one the frame must route to the GPU |
//! | the shared time axis' chrome | [`Element::gutter`] / [`Element::measured_gutter`] |
//! | the default drag table | [`Element::gesture_map`] |
//! | the gesture machine's reads | [`Element::lanes`] / [`Element::centres_y_zoom`], and [`Element::freq_axis`] & co. for an element that measures its own x |
//!
//! **Three things in, two things out**, and the boundary is narrow on purpose:
//! most of what looks like "what a widget needs from the host" is the widget's
//! own state coming back to it, and that stays home. What genuinely crosses is
//! the roles ([`Draw`] — the one mesh, the theme, the size table), the
//! [`World`] and the placement (both in [`Ctx`]); and back out, what the
//! element *is and needs* ([`Needs`], [`Element::value`]) and what it asks the
//! front to do ([`Claim`]). None of that grows per widget.
//!
//! **The registry is consulted only when no built-in name matched**, so a
//! built-in never changes meaning and a third party can register an element
//! today, against a host where every leaf is still an enum arm. A registry
//! *miss* stays exactly what an unrecognized type has always been —
//! [`WidgetKind::Unknown`](super::WidgetKind::Unknown), laid out and not
//! painted — which is what makes an element family compilable out of a build
//! without a new failure mode: a slim host degrades the way an old host does.
//!
//! **Two boundaries, stated rather than discovered.** A **container is not
//! extensible here**: the layout pass owns the coordinate systems (`window`,
//! `layout`, `plane`, `field`, the clip), and a third-party coordinate system
//! is a different and much larger promise. And an element sees a **press**, not
//! the drag machine's internals: the ongoing drag is a state machine over
//! typed built-in drags ([`super::super::gestures`]), so an element claims the
//! press and mutates itself, which covers a click, a toggle and a discrete
//! pick.
//!
//! **The registry is per thread.** The host core is single-threaded by design
//! — nothing here is `Send`, which is exactly what makes `Box<dyn Element>`
//! cheap — so registrations live in a `thread_local!` and an element must be
//! registered on the thread that builds the trees (natively the one running
//! the event loop; in a page, the only one there is).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use std::path::{Path, PathBuf};

use super::super::graphics::shape;
use super::super::layout::Rect;
use super::super::metrics::Metrics;
use super::super::paint::Draw;
use super::super::world::World;
use super::GestureMap;
use super::size::Natural;

/// Where an element is being drawn and what it is being drawn *into*: the
/// placement, and the [`World`] the frame reads from.
///
/// One context for the whole trait rather than a widening argument list, so a
/// later seam (the container's coordinate system) is a field here and not a
/// signature change in every element ever written.
pub struct Ctx<'a> {
    /// The read-only per-frame facts no widget owns.
    pub world: &'a World<'a>,
    /// The size roles of this placement, resolved at its scale — the same table
    /// [`Draw`] carries, for the methods that get no `Draw`.
    pub metrics: &'a Metrics,
    /// The rect this element was placed in, in the window's pixels.
    pub rect: Rect,
    /// Where this element's **shared axis** begins inside its rect: the widest
    /// gutter any member of its navigation group asked for
    /// ([`Element::gutter`]), stamped on the placement by the layout. `0.0`
    /// for an element on no shared axis, which is most of them.
    ///
    /// It is here and not derived because it is the *group's* answer: an
    /// element that measured its own would start the axis somewhere the lane
    /// beside it did not, and the same sample would sit at two different
    /// pixels.
    pub indent: f32,
    /// The clip rectangle of the container this was placed in — what a scrolled
    /// widget's drawing is cut to — or `None` outside one.
    ///
    /// The frame has already applied it, so an element that draws plainly never
    /// reads it. It is here for the one that **narrows the clip itself** (a
    /// score fits a page into its rect and cuts to the fit): the placement is
    /// data the frame holds, and an element must not have to ask the mesh what
    /// state it was left in.
    pub clip: Option<Rect>,
    /// The placement's zoom — 1.0 outside a `plane` workspace. The metrics
    /// already carry it, so it is needed for exactly one thing: the element's
    /// **own** `text_size` prop, which is a number the script sent and no table
    /// resolved (`self.text_size * scale`), matching what
    /// [`natural`](Element::natural) measured.
    pub scale: f32,
    /// The container's coordinate system, when this element was placed inside
    /// one — a clip's own time axis. `None` for an element standing on its own
    /// rectangle, which is every element outside a `clip` today.
    pub time: Option<TimeSpace>,
    /// Whether this element holds the window's keyboard focus. The host draws
    /// the focus ring itself, in the theme's `focus` role, so an element reads
    /// this only for what the ring cannot say — a field's caret and selection,
    /// which exist while it is being typed into and not otherwise.
    pub focused: bool,
}

/// **How a body that draws through a texture slot samples it**: the dB window
/// mapped onto the colormap, the shape of the frequency axis, and the colormap
/// itself.
///
/// A clip's body is drawn against the *clip's* axis and with the clip's id (a
/// body carries none), so it cannot go through the ordinary slot path — the
/// frame has to route it to the texture pass itself. This is the whole of what
/// it needs to know to do that, and an element that draws its body into the
/// mesh instead answers `None` ([`Element::texture_body`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureLook {
    pub db_floor: f32,
    pub db_ceil: f32,
    pub freq_scale: crate::spectrogram::FreqScale,
    /// The colormap index the texture pipeline resolves.
    pub colormap: i32,
}

/// **An element's own measured x axis**: the body its picture is drawn in, the
/// surface the axis answers to the pointer on, and the window it stands at.
///
/// The one axis in the host that is neither the window's shared time nor a
/// container's coordinate system — a spectrum's frequency. It is the element's
/// alone ([`Element::freq_axis`]), which is why the gesture machine asks for it
/// instead of holding it: only the element knows where inside its rectangle the
/// picture ended up, and what the analysis behind it can resolve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FreqAxis {
    /// Where the picture maps, exactly what the renderer drew through.
    pub body: Rect,
    /// Where the axis answers to the pointer: the body plus the ruler strip
    /// under it, which is the axis with the ticks drawn on it.
    pub surface: Rect,
    /// The window shown, as a normalized `(start, len)` of the whole axis.
    pub start: f64,
    pub len: f64,
    /// The rate the axis is placed by, so a hertz the gesture resolves is the
    /// hertz the frame drew — and so a zoom knows the analysis' resolution.
    pub sample_rate: f64,
}

/// A block of material an element handed over: interleaved samples, how many
/// channels they interleave, and the rate they were taken at.
///
/// The rate travels with the block and **nothing here converts it**: resampling
/// is an edit, an edit is something an owner performs and logs, and a paste
/// that quietly resampled would change data nobody asked it to change in a step
/// nothing records. That is the crate's rule for the clipboard, and this is the
/// door the rule arrives through.
#[derive(Debug, Clone, PartialEq)]
pub struct SampleBlock {
    /// The samples, interleaved by channel.
    pub samples: Vec<f32>,
    /// How many channels they interleave.
    pub channels: u32,
    /// The rate they were taken at.
    pub sample_rate: f64,
}

/// **An element's own measured y axis**: the body its lanes are cut from, the
/// domain they are drawn over and the vertical window they are seen through.
///
/// The counterpart of [`FreqAxis`] on the other axis, and asked for the same
/// reason: a marquee that restricts a selection in *value* needs the number
/// under the pointer to be the one the cursor readout names, and only the
/// element knows the domain it drew through, how many lanes it stacked and
/// where inside its rectangle the picture ended up.
///
/// It is the axis of a **trace** — amplitude, or whatever domain an element
/// declared. A time-frequency picture has a second axis too and it is not this
/// one: bins are the spectral selection's own field in the document's
/// `Selection`, deliberately separate, because an operation that understands a
/// value range need not understand a band of bins.
/// The run of samples the hand is holding, between the gesture and the
/// acknowledgement.
///
/// **A run rather than a sample**, because a grab and a stroke are the same
/// thing at two lengths: one dragged sample is a run of one, and a pencil
/// stroke is the same drawing over as many as it passed. Keeping one shape is
/// what lets the marked overlay, the emit and the drop-on-acknowledgement be
/// written once.
///
/// It lives here beside [`ValueAxis`] rather than in the element that draws it,
/// because the gesture machine sets it and the frame draws it, and neither
/// should have to know which element kind it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingEdit {
    /// Which channel — the lane the press landed in.
    pub channel: usize,
    /// The first sample of the run.
    pub start: usize,
    /// Where the hand has them now, in the element's own value domain.
    pub values: Vec<f32>,
    /// What they were when the gesture started, so the intent that leaves is
    /// **absolute and invertible** without the owner having to remember what
    /// they used to be.
    pub previous: Vec<f32>,
}

impl PendingEdit {
    /// A run of one — the dragged sample.
    pub fn one(channel: usize, frame: usize, value: f32, previous: f32) -> Self {
        Self {
            channel,
            start: frame,
            values: vec![value],
            previous: vec![previous],
        }
    }

    /// One past the last sample of the run.
    pub fn end(&self) -> usize {
        self.start + self.values.len()
    }

    /// The value this run holds for `frame`, if it covers it.
    pub fn value_at(&self, frame: usize) -> Option<f32> {
        frame
            .checked_sub(self.start)
            .and_then(|i| self.values.get(i))
            .copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueAxis {
    /// Where the picture maps: the rectangle the lanes are cut from, exactly
    /// what the renderer drew through.
    pub body: Rect,
    /// The value domain the geometry was mapped through — the element's
    /// `min`/`max`, [`crate::waveform::DEFAULT_DOMAIN`] when it names neither.
    pub domain: (f32, f32),
    /// The visible vertical window, normalized `(start, len)` of the display
    /// axis — `EditorProps::y_view`, the same pair the renderer used.
    pub y: (f64, f64),
    /// How many lanes the body is split into (1 when overlaid).
    pub lanes: usize,
}

impl ValueAxis {
    /// The value under a y pixel, in the element's own units, clamped to the
    /// domain.
    ///
    /// **Lane-relative**, like every other vertical read: a stacked view shows
    /// the same value axis in each lane, so the height is resolved within the
    /// lane the pointer is in and the answer is what the cursor readout says at
    /// that height. One inversion, shared with the readout
    /// ([`crate::waveform::display_to_value`]), so the marquee and the number
    /// beside it can never disagree.
    pub fn value_at(&self, cy: f64) -> f64 {
        let lanes = self.lanes.max(1);
        let lane = crate::host::frame::lane_rect(
            self.body,
            lanes,
            crate::host::frame::lane_at(self.body, lanes, cy),
        );
        let rel = ((cy - lane.y as f64) / lane.h.max(1.0) as f64).clamp(0.0, 1.0);
        let display = self.y.0 + (1.0 - rel) * self.y.1;
        let v = crate::waveform::display_to_value(display, self.domain.0, self.domain.1) as f64;
        let (lo, hi) = (
            self.domain.0.min(self.domain.1) as f64,
            self.domain.0.max(self.domain.1) as f64,
        );
        v.clamp(lo, hi)
    }

    /// Whether this axis covers the whole domain — a selection restricted to
    /// all of it is not restricted at all, and travels as the plain span the
    /// wire has always carried.
    pub fn is_whole(&self, min: f64, max: f64) -> bool {
        let (lo, hi) = (
            self.domain.0.min(self.domain.1) as f64,
            self.domain.0.max(self.domain.1) as f64,
        );
        min <= lo && max >= hi
    }
}

/// **What an element is fed, once per tick** — the third moment of the trait,
/// beside drawing ([`Ctx`]) and being dragged ([`Input`]).
///
/// A tick is where a live view *advances*: a rolling trace takes one sample, an
/// oscilloscope re-triggers its window, a spectrum folds a new frame into its
/// analysis. It is deliberately separate from the draw, which is why the
/// context is separate too: the tick runs at a steady rate and mutates, so a
/// window that repaints twice does not scroll twice, and one that repaints
/// never still keeps its history.
///
/// It carries only what a *reader of data* needs — the source, the rate, and
/// the retained pasts — and none of what a draw needs (the timeline groups, the
/// node trees, the pointer), because those borrow out of the host tree the tick
/// is walking mutably.
pub struct Live<'a> {
    /// The per-tick data source: control buses, tap windows, levels. `None`
    /// reads nothing, which is the no-transport case and what a test uses.
    pub bus: Option<&'a dyn super::super::BusSource>,
    /// The server's sample rate (`0.0` = unknown; a reader falls back to 48 kHz
    /// rather than dividing by zero).
    pub sample_rate: f64,
    /// The **retained past** of every bus something in this window watches. It
    /// is here rather than in the element because a history is the *bus's*: one
    /// per bus however many views watch it, and two views of one bus may
    /// analyze it differently.
    pub histories: &'a HashMap<i32, super::super::live::BusHistory>,
}

impl Live<'_> {
    /// The current value of control bus `bus` — the same rule [`World`] states
    /// for a draw, so a tick and a repaint never read one differently.
    pub fn control(&self, bus: i32) -> f32 {
        if bus < 0 {
            return 0.0;
        }
        self.bus.map_or(0.0, |s| s.control(bus as usize))
    }

    /// Fills `out` with the newest raw samples of audio bus `bus`, returning
    /// whether this source had any.
    pub fn read(&self, bus: i32, out: &mut [f32]) -> bool {
        self.bus.is_some_and(|s| s.read_bus(bus, out))
    }

    /// The retained history of `bus`, if anything asked for one.
    pub fn history(&self, bus: i32) -> Option<&super::super::live::BusHistory> {
        self.histories.get(&bus)
    }

    /// The rate to compute with: the server's, or 48 kHz when it is unknown.
    pub fn rate(&self) -> f64 {
        if self.sample_rate > 0.0 {
            self.sample_rate
        } else {
            48_000.0
        }
    }
}

/// What an element declares it reads from outside itself. Empty by default: an
/// element that draws only from its own props needs nothing.
///
/// **This is the whole declaration the tree collectors read.** Each field used
/// to be a walk of its own matching on a kind — which buses to stream, which
/// rings to record, which groups to query, whether the window animates — and
/// each of those walks now asks every widget one question instead. A collector
/// therefore learns nothing about a new element, which is what makes the
/// element addable by writing a file.
///
/// The sets are declarative and per element: the collectors merge, sort and
/// dedup them, so an element names what *it* reads and never what a window
/// subscribes to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Needs {
    /// Control buses read once per frame (the shm segment natively,
    /// `/bus_stream` in a page) — what a control-rate meter or a live trace
    /// contributes.
    pub buses: Vec<i32>,
    /// Audio buses whose **published block level** is read: one number per
    /// block out of the same source, costing neither a message nor a recording.
    pub levels: Vec<i32>,
    /// Audio buses whose **samples** are read, which the server has to record
    /// into a ring first (`/bus_tap`, `/bus_tapStream` in a page).
    pub taps: Vec<i32>,
    /// How many **seconds** of each tapped bus this element wants kept
    /// addressable — the `retention` span, `0.0` (the default) for a view that
    /// only ever reads the present.
    ///
    /// The span is declared here rather than resolved to samples, because the
    /// history is the **bus's** and not the element's: two views of one bus
    /// share one ring, sized at the longest span either asked for and resolved
    /// against the sample rate by the collector
    /// ([`live::collect_retention`](super::super::live::collect_retention)).
    pub retention: f32,
    /// Server groups whose node tree this element draws, so the client leg
    /// knows which trees to keep queried.
    pub node_groups: Vec<i32>,
    /// Whether this element must be redrawn every tick even with no data
    /// arriving — a picture driven by the clock rather than by a value.
    pub animated: bool,
    /// Whether this element reads the **engine sample clock** — a cursor that
    /// sweeps on its own from an anchor, rather than being told where to go.
    ///
    /// It is a need and not a flavour of [`animated`](Self::animated) because
    /// the two are answered by different machinery: a front with the server's
    /// shared segment mapped reads the clock for free, while a page has to ask
    /// for it (`/clock_query` per tick), and neither wants to pay when nothing
    /// in the window follows a clock.
    pub clock: bool,
    /// The GPU slot this element claims, for a view that cannot draw into the
    /// shared mesh. `None` — the default — is an element that draws.
    pub slot: Option<SlotKind>,
    /// Whether this element **reads live MIDI input**: a note played on a
    /// keyboard reaches it ([`Element::midi`]) rather than only a script's
    /// `/gui_set`.
    ///
    /// It is a need like the others because it is a *device* the front has to
    /// open — a virtual input port, native-only — and one nothing in the window
    /// asked for is one nothing opens. What arrives is the platform-neutral
    /// [`MidiNote`], the same posture [`Key`] takes: the front translates, and
    /// the element answers identically wherever it is compiled.
    pub midi: bool,
    /// The **bulk resource** this element wants resolved, and in which form.
    ///
    /// Bulk is the data too big for the wire — a minutes-long take, a peaks
    /// cache, a server buffer — and it moves through local shared resources
    /// (a mapped file natively, a `fetch` in a page), never re-encoded over
    /// OSC. This is the *declaration*; where the answer goes is not the
    /// loader's decision either: an element that claimed a [`slot`](Self::slot)
    /// is fed through it, and every other one takes the data home through
    /// [`Element::bulk`].
    pub bulk: Option<Bulk>,
}

/// **What an element wants loaded, and in which form** — the two halves of one
/// question, because the same file is a pyramid to one view and a run of
/// samples to another.
///
/// The form is the element's own business and not the resource's: a *take* is
/// minutes of audio and is summarized into peaks before it is ever drawn, while
/// a plotted *sequence* is a few thousand values that are kept whole. Neither
/// is a property of the file.
#[derive(Debug, Clone, PartialEq)]
pub enum Bulk {
    /// A prebuilt peak-pyramid cache, used as it is (no raw samples).
    PeakCache(PathBuf),
    /// Raw interleaved `f32` to de-interleave and summarize into a pyramid at
    /// `base_bucket`.
    Peaks {
        path: PathBuf,
        channels: usize,
        base_bucket: usize,
    },
    /// Raw interleaved `f32` kept **whole** (a plotted sequence): every channel
    /// is drawn, so nothing is decimated away.
    Samples { path: PathBuf, channels: usize },
    /// A prebuilt (single-channel) STFT cache.
    StftCache(PathBuf),
    /// Raw interleaved `f32` to analyze into per-channel STFT lanes.
    Stft {
        path: PathBuf,
        channels: usize,
        window_size: usize,
        hop: usize,
        sample_rate: f64,
    },
    /// A **server buffer**, pulled over the host's client leg rather than off
    /// the local filesystem — the one resource the host does not own and has to
    /// ask for.
    Buffer(i32),
}

impl Bulk {
    /// The local resource this wants, when it names one (a `Buffer` names
    /// none) — what a loader maps or fetches.
    pub fn resource(&self) -> Option<&Path> {
        match self {
            Bulk::PeakCache(p) | Bulk::StftCache(p) => Some(p),
            Bulk::Peaks { path, .. } | Bulk::Samples { path, .. } | Bulk::Stft { path, .. } => {
                Some(path)
            }
            Bulk::Buffer(_) => None,
        }
    }
}

/// **What came back**, in the form the [`Bulk`] asked for. The element takes it
/// home through [`Element::bulk`]; a loader never reaches into an element to
/// place it.
pub enum Loaded {
    /// **Raw interleaved samples**, for the element to make what it draws from
    /// — a pyramid, or the samples themselves. It is the one form a loader can
    /// hand over without knowing the drawing, which is what the server's own
    /// buffers arrive as.
    Raw { samples: Vec<f32>, channels: usize },
    /// A peak pyramid, from a cache or summarized from raw samples. It is
    /// **shared**: the element keeps it as the material it may be asked to read
    /// back, and the slot it claimed draws the same one — a pyramid is a picture
    /// *and* a body, and copying it would have made those two things.
    Peaks(std::sync::Arc<crate::waveform::WaveformData>),
    /// Per-channel STFT lanes.
    Stfts(Vec<crate::spectrogram::Stft>),
    /// Interleaved samples, kept whole.
    Samples(std::sync::Arc<[f32]>),
}

/// The GPU slot an element claims because it cannot draw into the window's one
/// mesh: it needs a texture, a vertex buffer or a shader of its own.
///
/// **The set is closed and belongs to the frame**, which owns the device, the
/// pipelines and the one-batch-per-window rule — an element *chooses* among
/// these and cannot invent one. Widening it is adding a pipeline to the frame,
/// which the cost rule already prices at once per window and only in the builds
/// that compiled it in. That is the same boundary as "a container is not
/// extensible", drawn where the hardware actually is.
#[derive(Debug, Clone, PartialEq)]
pub enum SlotKind {
    /// A user fragment shader run over the element's rect. The WGSL is a
    /// **parameter** of this slot, not a pipeline of its own, which is what
    /// makes a `canvas` an ordinary element rather than an exemption.
    Shader {
        /// The source the slot is created with; a later change arrives with the
        /// frame ([`SlotFrame::Shader`]) and recompiles in place.
        source: String,
    },
    /// A **vertex-buffer** slot: geometry rebuilt per frame, bounded by the
    /// render width in physical pixels — the resolution rule the whole crate
    /// draws signals by (never finer than the screen). What the columns are
    /// decimated *from* is a peak pyramid, which is why the bucket is the
    /// slot's parameter: it is what a load has to be summarized at before the
    /// pipeline can take it.
    Geometry {
        /// The peak pyramid's level-0 bucket, in samples.
        base_bucket: usize,
    },
    /// A **texture** slot: an analysis uploaded once and sampled one texel per
    /// pixel, so the GPU cost is constant however far the axis is zoomed. The
    /// analysis parameters ride with the slot because whoever fills it has to
    /// run that analysis, and the element is the only one that knows it.
    Texture {
        window_size: usize,
        hop: usize,
        /// The rate to place the analysis on (`0.0` = the server's).
        sample_rate: f64,
    },
}

/// The role an element fills as one of a **container's bodies** — the layered
/// contents of a `clip`: the material, the events over it, the automation over
/// both.
///
/// **The set is closed and belongs to the container**, which owns the layering,
/// the axis the bodies are drawn against and the props they are built from — an
/// element *chooses* among these and cannot invent one, exactly as it chooses a
/// [`SlotKind`] and cannot invent a pipeline. What the element says is only
/// which role it fills; where that role sits and what it is drawn through stays
/// the clip's, which is the boundary between an element and a coordinate
/// system.
///
/// The declaration order **is** the layering order, back to front: a curve set
/// on a clip that already has a take is drawn over it, which is what makes an
/// envelope over its material one clip rather than two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BodyRole {
    /// The material itself — a clip's sound, drawn as a signal.
    Take,
    /// The events over it — the notes of a roll.
    Notes,
    /// The automation over both — a break-point curve.
    Curve,
}

/// The **coordinate system a container placed an element on**: a clip's own
/// time axis, in the container's units.
///
/// A body is drawn and grabbed against this rather than against its own
/// rectangle, which is what lets one element be both the standalone view and
/// the clip's body: the same picture, mapped through the axis it was given.
/// `None` on [`Ctx`]/[`Input`] is an element standing on its own, which then
/// draws its own chrome and spans its own domain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeSpace {
    /// The visible window of the container's axis, in its own units — the
    /// slice of `0..span` the element's rectangle spans.
    pub view: crate::viewport::View,
    /// The full span of that axis (a clip's `dur`): the domain a time is
    /// clamped into, whatever part of it is on screen.
    pub span: f64,
    /// The axis' **shared time selection**, or `None` when nothing is selected
    /// — the band every linked view draws, which is the axis' state and not
    /// any one member's. An element that draws it reads it here; an element
    /// that *moves* it asks ([`Events::and_select`]).
    pub sel: Option<(f64, f64)>,
    /// Where the axis' **playhead** stands, in its units, or `None` for an
    /// axis with no transport on it.
    ///
    /// It is a **draw-time** fact: the engine sample clock is the front's, and
    /// a gesture is handed no clock, so this reads `None` in an [`Input`] and
    /// carries the position in a [`Ctx`]. Nothing a drag decides depends on
    /// where the playhead is.
    pub head: Option<f64>,
}

impl TimeSpace {
    /// A bare axis: a window over a span, with no selection and no transport on
    /// it — what a container hands a body, and what a test draws against.
    pub fn of(view: crate::viewport::View, span: f64) -> Self {
        Self {
            view,
            span,
            sel: None,
            head: None,
        }
    }
}

/// What a claimed slot is fed **this frame** — the live counterpart of
/// [`SlotKind`], produced with the world in hand so a value read from a bus is
/// resolved where every other per-frame read is.
#[derive(Debug, Clone, PartialEq)]
pub enum SlotFrame {
    /// The shader slot: where in the placement to draw, the current source
    /// (recompiled only when it changed) and the resolved uniform params.
    Shader {
        body: Rect,
        source: String,
        params: [f32; crate::canvas::PARAM_COUNT],
    },
    /// The geometry slot: a trace decimated per frame out of the peak pyramid
    /// the slot holds, drawn into `body` at the element's vertical window.
    ///
    /// What is *not* here is as deliberate as what is: the horizontal window is
    /// the **navigation group's** and the lane count is the **slot's**, so an
    /// element states neither — it would have to know its own id for the first
    /// and what reached the card for the second. `overlay` is the one thing
    /// about the lanes that is the element's: whether they stack or share one.
    Waveform {
        body: Rect,
        /// The **value domain** the geometry is mapped through — the element's
        /// `min`/`max`, [`crate::waveform::DEFAULT_DOMAIN`] when it names
        /// neither. It is the element's because the same pair decides what the
        /// mesh renderers draw, and a prop that means something in four of an
        /// element's presentations and nothing in the fifth is the divergence
        /// this closed.
        domain: (f32, f32),
        /// The amplitude window, as a normalized `(start, len)`.
        amp: (f64, f64),
        /// What the picture measures — the envelope, the level inside it, or
        /// both: one picture per measure, into the one body. It rides here
        /// rather than being read off the element at draw time for the reason
        /// the domain does: the frame draws what the element *stated*, so the
        /// picture and the chrome around it agree.
        measures: crate::host::graphics::signal::trace::Measures,
        overlay: bool,
    },
    /// The texture slot: one uploaded analysis per lane, sampled a texel per
    /// pixel, drawn into `body` at the element's frequency window and look.
    Spectrogram {
        body: Rect,
        /// The frequency window, as a normalized `(start, len)`.
        freq: (f64, f64),
        look: TextureLook,
    },
}

/// **What an element hands its claimed slot to upload**, when it has something
/// new for it — the other half of [`SlotFrame`], which says what that slot
/// *draws* once it is filled.
///
/// The two are separate because they run on different rhythms and in different
/// directions. A [`SlotFrame`] is produced per repaint out of a borrowed
/// element, and carries the frame's own reading of the world; a `SlotFill` is
/// **taken** from the element ([`Element::fill`]) at the front's tick, and is
/// the data itself. An element with nothing new hands back `None`, which is
/// what keeps a still picture at zero uploads.
///
/// The variants answer the two slot kinds an element can claim, and the third
/// is the rolling case: an analysis that grows forward and is pushed in
/// column by column rather than rebuilt.
pub enum SlotFill {
    /// A [`SlotKind::Geometry`] slot's whole content: the peak data the
    /// per-frame columns are decimated from, summarized at the element's own
    /// bucket.
    Geometry(std::sync::Arc<crate::waveform::WaveformData>),
    /// A [`SlotKind::Texture`] slot's whole content: one analysis per channel
    /// lane, uploaded once and sampled one texel per pixel.
    Texture(Vec<crate::spectrogram::Stft>),
    /// A [`SlotKind::Texture`] slot's **new columns**, frame-major and oldest
    /// first: a retained time-frequency picture grows forward, so the upload is
    /// what landed since the last fill and costs one texel write per bin —
    /// where rebuilding the transform each tick made the cost follow the whole
    /// retained span.
    ///
    /// The geometry rides along because a `/gui_set` of the analysis restarts
    /// the roll upstream: a ring built against the old one is not the same
    /// picture and is rebuilt rather than pushed into.
    Columns {
        columns: Vec<f32>,
        window_size: usize,
        hop: usize,
        sample_rate: f32,
        /// The retained span in columns, which a `/gui_set` of the retention
        /// moves under a live view.
        capacity: usize,
    },
}

impl fmt::Debug for SlotFill {
    /// The payload is megabytes of analysis and says nothing a reader of a
    /// `{:?}` wants: the shape is the state, exactly as it is for the live
    /// windows a tick accumulates.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlotFill::Geometry(data) => f
                .debug_struct("Geometry")
                .field("samples", &data.total_samples())
                .field("channels", &data.num_channels())
                .finish(),
            SlotFill::Texture(stfts) => f
                .debug_struct("Texture")
                .field("lanes", &stfts.len())
                .field("frames", &stfts.first().map(|s| s.n_frames()))
                .finish(),
            SlotFill::Columns {
                columns,
                window_size,
                hop,
                sample_rate,
                capacity,
            } => f
                .debug_struct("Columns")
                .field("values", &columns.len())
                .field("window_size", window_size)
                .field("hop", hop)
                .field("sample_rate", sample_rate)
                .field("capacity", capacity)
                .finish(),
        }
    }
}

/// Where an element is and what the pointer is doing to it — the context of a
/// **gesture**, as [`Ctx`] is the context of a draw.
///
/// It carries no [`World`], and that is a boundary rather than an omission: a
/// gesture *mutates* the host tree, while the world borrows out of it (the
/// timeline groups, the queried node trees), so nothing can hold both at once.
/// An element reads the outside when it draws, and moves itself when it is
/// dragged.
pub struct Input<'a> {
    /// The size roles of this placement, resolved at its scale — the same table
    /// the renderer drew the element with, so a grab lands on the groove that
    /// was painted.
    pub metrics: &'a Metrics,
    /// The rect the element was placed in when the press landed.
    pub rect: Rect,
    /// Where this element's **shared axis** begins inside its rect — the same
    /// group answer [`Ctx::indent`] carries, so a press lands on the pixels
    /// that were painted. `0.0` for an element on no shared axis.
    pub indent: f32,
    /// The placement's zoom (see [`Ctx::scale`]).
    pub scale: f32,
    /// The modifier keys held for this event.
    pub mods: Mods,
    /// The window in device pixels — what a popup that must stay on screen
    /// clamps itself against.
    pub viewport: (f32, f32),
    /// The container's coordinate system (see [`Ctx::time`]), so a grab lands
    /// on the axis the element was drawn against.
    pub time: Option<TimeSpace>,
}

/// The modifier keys a gesture was made with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// A **platform-neutral key**: what the fronts (winit natively, winit-on-canvas
/// in a page) translate their key events into, so a keyboard behaves identically
/// on a desktop and in a tab.
///
/// It is the editing alphabet and nothing more — a printable character and the
/// motions every field answers to — because a shortcut over a *view* (`q`
/// quantize, `r` reset) is not addressed to a focused element at all: it belongs
/// to whatever is under the cursor, and the front runs it when nothing consumed
/// the key.
///
/// The modifiers ride beside it in [`KeyInput`], not in the variants, so an
/// element writes one arm per key rather than one per combination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// A printable character to insert (already resolved from the layout).
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Enter: a newline in a multiline field, ignored in a single-line one.
    Enter,
    /// Tab: **the focus ring's**, never an element's. The machine consumes it
    /// before any element sees it (see
    /// [`Gestures::key`](super::super::gestures::Gestures::key)), which is what
    /// makes a window keyboard-navigable without every element agreeing to it.
    Tab,
}

/// A **platform-neutral MIDI note event**: what a front's live input port
/// translates its channel-voice messages into, so an element paints the same
/// note wherever it runs.
///
/// Note-on with velocity 0 is a note-off before it gets here — the parse is the
/// front's, exactly as resolving a keyboard layout into a [`Key::Char`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiNote {
    /// Start it, or release the one sounding at this pitch and channel.
    pub on: bool,
    pub channel: i32,
    pub pitch: i32,
    pub velocity: i32,
}

/// What a key arrives with: the modifiers held, and the host-wide clipboard a
/// cut/copy/paste reads and writes.
///
/// The clipboard is here rather than in the element because it is not the
/// element's: one of them serves every field, roll and view of every window
/// (the front's internal one natively, the page's swapped in around this call
/// in a browser), so a selection cut in one place pastes into another.
///
/// It is a [`Clip`](crate::host::clipboard::Clip) rather than a `String`
/// because an editor's clipboard has to carry a range of audio, and the only
/// way to keep that a string is to re-encode it. An element that deals in text
/// still reads and writes text through it — that is one of the kinds.
pub struct KeyInput<'a> {
    pub mods: Mods,
    pub clipboard: &'a mut crate::host::clipboard::Clip,
}

/// The `/gui_event` messages an element asks to be sent for it: each entry is
/// **one message's arguments**, expressed in the owner's terms.
///
/// A control reports one value ([`Events::value`]) and an edit-back reports a
/// payload. It is a list of messages rather than one because of the case that
/// is neither: a patcher's release reports one `"move"` per box it moved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Events {
    msgs: Vec<Vec<OscType>>,
    voices: Vec<Voice>,
    select: Option<SelectRequest>,
}

/// **What an element asks the container's selection to become**: the span in
/// the axis' own units, and the value range restricting it where the element
/// swept a rectangle rather than a stripe.
pub(crate) type SelectRequest = ((f64, f64), Option<(f64, f64)>);

/// **A voice an element asks the host to sound**, beside the event it reports.
///
/// The one thing a keyboard cannot do for itself: sounding a held key is a
/// `/synth_new` on the audio server, and only the host has a leg to it. So the
/// element names the pitch and the host performs it, using the
/// [`VoiceSpec`] the same element declares — the shape a pointer grab already
/// has, which is *what only the front can do, named in what the element
/// returns*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voice {
    pub pitch: i32,
    pub velocity: i32,
    /// Start it, or gate the one sounding at this pitch.
    pub on: bool,
}

impl Voice {
    pub fn on(pitch: i32, velocity: i32) -> Self {
        Voice {
            pitch,
            velocity,
            on: true,
        }
    }

    pub fn off(pitch: i32) -> Self {
        Voice {
            pitch,
            velocity: 0,
            on: false,
        }
    }
}

/// **What one of this element's voices is**: the server def it plays and the
/// extra `/synth_new` controls it is started with (appended after the
/// `freq`/`amp`/`gate` the host fills in).
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceSpec {
    pub def: String,
    pub args: Vec<(String, f32)>,
}

impl Events {
    /// Nothing to report — the default.
    pub fn none() -> Self {
        Self::default()
    }

    /// The widget's value, the one-argument case every control uses.
    pub fn value(v: OscType) -> Self {
        Self {
            msgs: vec![vec![v]],
            ..Self::default()
        }
    }

    /// One message, its arguments in the owner's terms.
    pub fn message(args: Vec<OscType>) -> Self {
        Self {
            msgs: vec![args],
            ..Self::default()
        }
    }

    /// Appends another message.
    pub fn and(mut self, args: Vec<OscType>) -> Self {
        self.msgs.push(args);
        self
    }

    /// Asks for a voice beside what is reported.
    pub fn and_voice(mut self, voice: Voice) -> Self {
        self.voices.push(voice);
        self
    }

    /// Asks the machine to move the **container's time selection** to
    /// `(start, end)` in the axis' own units (either order).
    ///
    /// The second thing an element cannot do for itself, and the same shape as
    /// [`Voice`]: a marquee swept over a roll sets the selection *every linked
    /// view follows*, which is the navigation group's state and not the roll's
    /// — so the element names the span and the machine writes it, repaints the
    /// linked windows and reports the `"selection"` the group already emits.
    pub fn and_select(self, start: f64, end: f64) -> Self {
        self.and_select_in(start, end, None)
    }

    /// The same, **restricted on the axis' second axis**: the value range the
    /// sweep covered, in that axis' own units, or `None` for a sweep that
    /// restricts nothing.
    ///
    /// One door with an argument rather than a second builder to remember,
    /// because the two halves of a marquee are written by one gesture and a
    /// range that could be set without a span would be a range of nothing.
    pub fn and_select_in(mut self, start: f64, end: f64, values: Option<(f64, f64)>) -> Self {
        self.select = Some(((start, end), values));
        self
    }

    /// Everything of `other`, after this — a gesture that both ends one thing
    /// and starts another (a glissando: a note off, then a note on).
    pub fn chain(mut self, other: Events) -> Self {
        self.msgs.extend(other.msgs);
        self.voices.extend(other.voices);
        self.select = other.select.or(self.select);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.msgs.is_empty() && self.voices.is_empty() && self.select.is_none()
    }

    /// The messages, for the gesture machine that delivers them.
    pub(crate) fn into_messages(self) -> Vec<Vec<OscType>> {
        self.msgs
    }

    /// The voices asked for, for the machine that performs them.
    pub(crate) fn voices(&self) -> &[Voice] {
        &self.voices
    }

    /// The container-selection request, for the machine that performs it.
    pub(crate) fn selection(&self) -> Option<SelectRequest> {
        self.select
    }
}

/// **The shape an element answers the pointer on**, inside the rectangle it was
/// placed in.
///
/// A placement is always a rectangle, and for most elements that *is* the
/// shape: a field, a plot, a lane fill their cell. The exceptions are the ones
/// drawn smaller or rounder than what the layout gave them — a knob's dial, a
/// slider's groove, a checkbox with a word beside it in a stretched row — and
/// each of those was acting on presses landing on blank space the layout had
/// left around them, because the routing tested containment in the cell.
///
/// So the element **declares** its shape and the gesture machine applies it
/// (`gestures::element::press`), once, for every element there is. Declaring is
/// what keeps it general: the guard is not three lines each leaf must remember
/// to write, and a leaf that says nothing keeps the rectangle it always had.
/// The slop is added by the machine, so a small target is grabbable without
/// each element deciding how much air it deserves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HitArea {
    /// The rectangle itself — the default, and what anything filling its cell
    /// answers.
    Rect(Rect),
    /// A disc: a knob's dial.
    Disc { cx: f32, cy: f32, r: f32 },
    /// The ellipse inscribed in a box.
    Ellipse(Rect),
}

impl HitArea {
    /// Whether `(x, y)` is on the shape, with `slop` of air around it.
    pub fn hit(&self, x: f64, y: f64, slop: f32) -> bool {
        match *self {
            HitArea::Rect(r) => r.grown(slop).contains(x, y),
            HitArea::Disc { cx, cy, r } => {
                shape::in_disc(x, y, cx as f64, cy as f64, (r + slop) as f64)
            }
            HitArea::Ellipse(r) => shape::in_ellipse(x, y, r.grown(slop)),
        }
    }
}

/// What an element did with a press it was offered.
#[derive(Debug, Clone, PartialEq)]
pub enum Claim {
    /// Not this element's: hand the press back to the chain, exactly as a
    /// lane's empty space or a patcher's bare canvas does.
    Decline,
    /// Consumed. The window is redrawn, whatever else the claim asks for, and
    /// the press is **held**: motion reaches [`Element::drag`] and the button
    /// coming up reaches [`Element::release`], however long that takes.
    Take(Take),
}

/// A taken press: what to report for it, and the one thing the element cannot
/// do for itself.
///
/// It is deliberately not a taxonomy of drags. The *kind* of drag — absolute
/// (a position in a rect becomes a fraction), incremental (a delta re-anchored
/// each step) or snapshotted (a press-time origin plus an axis) — is the
/// element's own business, because the element holds the state. What is left is
/// what only the front and the machine can do.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Take {
    /// What to report on `/gui_event` for the press itself.
    pub events: Events,
    /// Ask the front for a **pointer grab**: the cursor stays put and motion
    /// arrives as relative deltas ([`Element::drag_relative`]) instead of
    /// positions. What a knob wants, so a turn is not bounded by the screen.
    /// The front answers — a page has no pointer lock — and the machine routes
    /// whichever way it answered.
    ///
    /// It is one of the two things only the front and the machine can do: the
    /// *kind* of drag is the element's, because the element holds the state.
    pub grab: bool,
    /// Ask the machine to keep **ticking** this drag while the cursor is held
    /// past the edge of the axis the element sits on, panning that axis under
    /// it — what a note dragged off the right of a lane needs, since a held
    /// cursor produces no motion events and the view has to keep moving anyway.
    ///
    /// It is the machine's rather than the element's because the axis is the
    /// *group's*: panning it repaints every linked window and re-reports the
    /// view, none of which an element can reach. What the element gets back is
    /// an ordinary [`drag`](Element::drag) per tick, against a window that has
    /// moved — which is why the drag must read its axis from
    /// [`Input::time`] each step rather than snapshotting it.
    pub edge_scroll: bool,
}

impl Claim {
    /// Consumed, with nothing to report.
    pub fn take() -> Self {
        Claim::Take(Take::default())
    }

    /// Consumed, reporting the widget's value.
    pub fn value(v: OscType) -> Self {
        Claim::Take(Take {
            events: Events::value(v),
            ..Take::default()
        })
    }

    /// Consumed, reporting these events.
    pub fn events(events: Events) -> Self {
        Claim::Take(Take {
            events,
            ..Take::default()
        })
    }

    /// ...and grab the pointer for the drag this press opens.
    pub fn grabbing(self) -> Self {
        match self {
            Claim::Take(t) => Claim::Take(Take { grab: true, ..t }),
            decline => decline,
        }
    }

    /// ...and keep the axis scrolling while the drag is held past its edge.
    pub fn edge_scrolling(self) -> Self {
        match self {
            Claim::Take(t) => Claim::Take(Take {
                edge_scroll: true,
                ..t
            }),
            decline => decline,
        }
    }
}

/// A leaf the renderer draws without knowing what it is.
///
/// Object-safe and single-threaded on purpose (see the module docs).
/// [`clone_box`](Element::clone_box) is the one piece of ceremony: the widget
/// tree is `Clone` (a def is rebuilt by replacement, and the frame copies out
/// of it), so a boxed element has to be too.
///
/// Every method but [`set`](Element::set), [`draw`](Element::draw) and
/// `clone_box` has a default, so the smallest element is three methods.
pub trait Element: fmt::Debug {
    /// Applies one `/gui_set` key/value, returning whether the key was this
    /// element's. A key it does not know must return `false` — the host logs
    /// the unknown prop rather than silently dropping it.
    fn set(&mut self, key: &str, v: &Value) -> bool;

    /// Draws into the window's one mesh, inside `ctx.rect`. `d` carries the
    /// resolved theme and the size table of the placement, so an element names
    /// roles and never literals, the way every built-in does; `ctx` carries the
    /// placement and the [`World`] — the bus values, the sample clock, the
    /// pointer — that the frame reads and no widget owns.
    fn draw(&self, d: &mut Draw, ctx: &Ctx);

    /// How big this element wants to be, per axis — `None` meaning elastic.
    /// Pure over the metrics, the element's own *presentation* props and the
    /// placement's `scale`, never over its data: a size that reads the data
    /// turns a `/gui_set` into a relayout. Elastic on both axes by default.
    fn natural(&self, _m: &Metrics, _scale: f32) -> Natural {
        (None, None)
    }

    /// How big this element wants to be **when a container is being fitted to
    /// its content** — a container carrying `hug`, which is the only caller.
    ///
    /// The one place a size may read what the element draws, and the reason the
    /// method is separate from [`natural`](Element::natural): the ordinary pass
    /// must stay data-free, or a `/gui_set` would relayout the window on every
    /// message, while a container that hugs has asked for exactly that. What
    /// may be read is fixed by *where it is resolved*, not by what it is
    /// called: a prop that settles at a mutation point (a label's text, a
    /// menu's options) may size; a **value** — a number being turned, a field
    /// being typed into, a scope's samples — may not, or the widget would
    /// resize under the gesture writing it.
    ///
    /// Defaults to the natural size, which is the right answer for every
    /// element whose content is a value or a signal.
    fn hug(&self, m: &Metrics, scale: f32) -> Natural {
        self.natural(m, scale)
    }

    /// This element's current value, for `/gui_event` and `/gui_query`.
    fn value(&self) -> Option<OscType> {
        None
    }

    /// Extra `/gui_query` fields, beside the value — an element's own state a
    /// script may want to read back.
    fn info(&self) -> Vec<(String, Value)> {
        Vec::new()
    }

    /// What this element reads from outside itself.
    fn needs(&self) -> Needs {
        Needs::default()
    }

    /// **The editor chrome this element carries**, or `None` (the default) for
    /// one that carries none.
    ///
    /// [`EditorProps`](super::EditorProps) is what a member of a navigation
    /// group is made of — its ruler units, its own vertical window, its link,
    /// its offset on the shared axis — and it is read *and written* from
    /// outside: a gesture pans the group and writes the member's window, a
    /// `/gui_set` of `view_y` lands here. So the door is a borrow of the props
    /// rather than a copy of them.
    fn editor(&self) -> Option<&super::EditorProps> {
        None
    }

    /// [`editor`](Element::editor), mutably — the door a navigation gesture and
    /// a `/gui_set` of the chrome write through.
    fn editor_mut(&mut self) -> Option<&mut super::EditorProps> {
        None
    }

    /// Whether this element navigates the window's **shared time axis**, and so
    /// joins a navigation group. `false` by default.
    fn navigates_time(&self) -> bool {
        false
    }

    /// Whether this element draws an overlay that **follows the pointer** — a
    /// cursor readout over stored data. `false` by default.
    ///
    /// The window asks because it decides what a mouse move costs: a picture
    /// with a readout needs a frame per move, where a fully static one has no
    /// other frame source and a live one is being repainted anyway.
    fn hover_readout(&self) -> bool {
        false
    }

    /// **The def one of this element's voices plays**, or `None` (the default)
    /// for an element that asks for no voices.
    ///
    /// Declared separately from the [`Voice`] requests themselves because the
    /// two answer different questions: *what to play* is a prop a script sets
    /// and changes mid-hold, while *play it now* is a gesture's. The host reads
    /// this when it performs a request, so a `/gui_set` of the def takes effect
    /// on the next key rather than on the next frame.
    fn voice(&self) -> Option<VoiceSpec> {
        None
    }

    /// **The concrete element behind the trait object**, for the few callers
    /// that own it — `None` by default, which is every element that has no
    /// reason to be reached concretely.
    ///
    /// It exists for the built-ins whose *tests* assert on their own model, and
    /// for a program that registered an element and wants its own type back.
    /// Nothing in the passes uses it: a pass that needs an answer gets a door,
    /// which is the whole point of the seam.
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        None
    }

    /// Whether this element navigates a **measured x axis of its own** — a
    /// frequency axis — instead of joining the window's shared time. `false`
    /// by default.
    ///
    /// Such an axis needs no history behind it (every bin is there every
    /// frame) and no navigation group (nothing else in a window measures in
    /// hertz along x), so it is one normalized window the element carries
    /// alone.
    fn navigates_freq(&self) -> bool {
        false
    }

    /// This element's [`FreqAxis`] inside the rect it was placed in, or `None`
    /// for one that navigates no axis of its own.
    ///
    /// The gesture machine cannot work it out: where the picture sits inside
    /// the rectangle is the element's own region split — a label above it, a
    /// ruler strip below, a value strip beside — and it must be the *same* one
    /// the renderer drew through, or a zoom anchors at a hertz the reader is
    /// not pointing at.
    fn freq_axis(&self, _rect: Rect, _m: &Metrics, _sample_rate: f64) -> Option<FreqAxis> {
        None
    }

    /// **The material this element holds over a span of its own frames**, as
    /// interleaved samples with the rate they were taken at — what a copy puts
    /// on the clipboard.
    ///
    /// `None` where the element has nothing it could honestly hand over: a
    /// picture with no samples behind it (a mapped pyramid is an overview, and
    /// a block of silence is worse than declining), a live view whose data is
    /// gone the moment it is drawn, an element that is not material at all.
    /// **Read-only, and it is the host's whole part in a copy**: writing the
    /// span back is an edit, and an edit belongs to whoever owns the data.
    /// `server_rate` is what the block is stamped with when the element names
    /// no rate of its own, the same fallback [`Element::freq_axis`] takes.
    fn sample_block(&self, _start: u64, _frames: u64, _server_rate: f64) -> Option<SampleBlock> {
        None
    }

    /// The run this element is holding for the hand, if any — what the frame
    /// draws **over** the picture while an edit is in flight.
    fn pending_edit(&self) -> Option<&PendingEdit> {
        None
    }

    /// Holds a run (or lets go of one), returning whether this element is the
    /// kind that can. `None` clears it, which is what an acknowledgement does
    /// once the owner has answered.
    ///
    /// The value it holds is deliberately **not** written into the material:
    /// the host owns no data, and a pending value that entered the summary
    /// would make the overview disagree with the samples until the edit landed
    /// — besides costing a re-summarize per motion event.
    fn set_pending_edit(&mut self, _pending: Option<PendingEdit>) -> bool {
        false
    }

    /// The value of one sample of this element's material, in its own domain —
    /// what a grab reads so the intent it later emits can carry the value it
    /// started from.
    fn sample_value(&self, _channel: usize, _frame: usize) -> Option<f32> {
        None
    }

    /// This element's [`ValueAxis`] inside the rect it was placed in, or `None`
    /// for one whose vertical measures nothing a selection could name.
    ///
    /// Asked for the same reason [`Element::freq_axis`] is: the region split is
    /// the element's own, and a marquee that restricts a selection in value has
    /// to read the axis the picture was drawn through. `lanes` is what the
    /// front found in the element's slot, resolved by
    /// [`Element::lanes`] as everywhere else — the element states the domain and
    /// the window, never how many channels reached the card. `indent` is where
    /// the shared axis starts inside the rect, as everywhere else.
    fn value_axis(
        &self,
        _rect: Rect,
        _indent: f32,
        _m: &Metrics,
        _lanes: usize,
    ) -> Option<ValueAxis> {
        None
    }

    /// **What this element's measured axis would actually show**: `want` opened
    /// up wherever it is finer than the analysis behind it resolves, or its
    /// current request when `want` is `None`. `None` for an element with no
    /// such axis.
    ///
    /// Request and display are deliberately kept apart, which is why this is a
    /// question and not a stored value: the floor is a function of *where* the
    /// window sits — on a log axis a window narrow enough at 12 kHz cannot
    /// exist at 100 Hz — so writing the opening back would spend the reader's
    /// zoom on the way down the axis and never give it back.
    fn freq_window_of(&self, _sample_rate: f64, _want: Option<(f64, f64)>) -> Option<(f64, f64)> {
        None
    }

    /// The narrowest window this element's measured axis may be **asked** for
    /// at `start`, or `None` for an element with no such axis.
    ///
    /// A zoom needs it as a number rather than as a clamp applied afterwards:
    /// a step that overshot the floor and was corrected later would have
    /// anchored a window narrower than the one it ends up with, sliding the
    /// picture sideways at every further step.
    fn freq_min_span(&self, _sample_rate: f64, _start: f64) -> Option<f64> {
        None
    }

    /// **The drag table this element wants** when the wire declares none, or
    /// `None` (the default) to take the generic one — the press goes to the
    /// element and every modifier with it.
    ///
    /// An element that is placed on a **container's axis** usually wants
    /// otherwise: a navigable view lets a plain drag sweep the container's
    /// selection and Shift pan its window, because those gestures are the
    /// axis's and not the picture's.
    fn gesture_map(&self) -> Option<GestureMap> {
        None
    }

    /// **The look of a body whose picture is a texture**, or `None` (the
    /// default) for one that draws into the shared mesh
    /// ([`draw_body`](Element::draw_body)).
    ///
    /// The one body the frame cannot let draw itself: a time-frequency picture
    /// samples an uploaded texture, so it goes to the GPU pass with the clip's
    /// own axis and the clip's id — the key its slot was filled under.
    fn texture_body(&self) -> Option<TextureLook> {
        None
    }

    /// **What this element draws as a clip's body**, into the clip's rectangle
    /// and against the clip's own local axis (`dur` is the clip's span).
    ///
    /// The whole of what "a clip is a container" buys: the element says what it
    /// is, the container says where it is, and neither knows about the lane,
    /// the group's window or the clip's offset on it. It is a separate draw
    /// from [`draw`](Element::draw) because a body carries **no chrome** — no
    /// ruler, no gutter, no navigation of its own — so the two are different
    /// pictures of the same data. The default draws nothing.
    fn draw_body(&self, _d: &mut Draw, _rect: Rect, _local: &crate::viewport::View, _dur: f64) {}

    /// **Where the shared time axis lies inside this element's rect**, and
    /// whether it offers a vertical gesture surface beside it — or `None` (the
    /// default) to take the generic timeline body, which is what every
    /// element on that axis but one wants.
    ///
    /// It is a door because the roll is the one leaf whose picture is *not* the
    /// rectangle minus its chrome: strips are stacked under its grid (a
    /// velocity lane, an event lane) that read the same time and are not part
    /// of the body a sample maps into, and its keyboard gutter is a vertical
    /// surface whatever `ruler_y` says. The hit-test has to place the axis
    /// exactly where the drawing did, so it asks.
    fn axis_body(&self, _rect: Rect, _indent: f32, _m: &Metrics) -> Option<(Rect, bool)> {
        None
    }

    /// **The axis length this element's own content occupies**, or `None` (the
    /// default) for an element whose extent is registered from outside — a
    /// loaded take, a streamed history.
    ///
    /// A navigation group's timeline is the longest of its members' extents, so
    /// a surface that is *authored* rather than loaded — a roll being written
    /// note by note — has to say how far its content now reaches, or the axis
    /// stays the length it was defined with and everything painted past it
    /// lands outside the window.
    fn content_span(&self) -> Option<f64> {
        None
    }

    /// **The content extent this element drives**, in the plane's own units, or
    /// `None` for the element that drives none — which is every one but a
    /// patcher, whose graph the host lays out.
    ///
    /// It is the two-dimensional twin of [`content_span`](Element::content_span)
    /// and the deliberate opposite of [`natural`](Element::natural): a natural
    /// size is pure over the metrics and the presentation props and must never
    /// follow the data, because it resolves on the layout's main axis; this
    /// *is* the data, and it sizes the workspace a plane scrolls over — where a
    /// content extent is the one thing a container cannot compute for a child
    /// it does not interpret.
    fn content_size(&self) -> Option<(f32, f32)> {
        None
    }

    /// **What this element reserves left of its body** for chrome of its own —
    /// a value ruler — when it sits on a shared time axis. `0.0` by default.
    ///
    /// It is a *wish*, not a placement: the indent every member of a navigation
    /// group draws at is the widest wish on that axis, because the axis is
    /// shared and the same sample must sit at the same pixel in all of them.
    /// Answered from the props alone, so the layout knows it before a single
    /// rectangle exists.
    fn gutter(&self, _m: &Metrics) -> f32 {
        0.0
    }

    /// The gutter this element wants once it has been **placed**, or `None`
    /// (the default) when its wish did not depend on the placement after all.
    ///
    /// A ruler's width can be a property of the *data* rather than of the
    /// props: an amplitude axis zoomed onto a narrow range formats `-0.0625`
    /// where the same axis unzoomed formats `-1.0`, and the step it labels at
    /// depends on how tall the element ended up. That is one pass later than
    /// [`gutter`](Element::gutter), so it is a second question and not the same
    /// one — and an element answers `None` unless the measure would actually
    /// widen the band, since a second layout pass is only taken when one is
    /// owed.
    fn measured_gutter(&self, _rect: Rect, _m: &Metrics) -> Option<f32> {
        None
    }

    /// **How many lanes this element stacks on screen**, given the `uploaded`
    /// count the front found in its GPU slot — the divisor for every
    /// lane-relative y gesture.
    ///
    /// The front knows how many channels are actually on the card and nothing
    /// about how they are arranged, which is why the two halves meet here: an
    /// element that *overlays* its channels draws one lane however many it was
    /// given, and one that stacks them draws as many as there are. The default
    /// stacks.
    fn lanes(&self, uploaded: usize) -> usize {
        uploaded.max(1)
    }

    /// Whether a y zoom over this element anchors at the **centre** of a lane
    /// rather than under the pointer. `false` by default: the pointer is where
    /// a reader expects a zoom to hold still.
    ///
    /// It is a property of what the axis *measures*, because one vertical
    /// window is shared by every lane. An axis of **values** — frequency,
    /// pitch — says the same thing in each of them, so the value under the
    /// cursor is meaningful and holding it still is what the reader wants. An
    /// **amplitude** axis does not: zero sits at the centre of every lane, an
    /// anchor taken from the pointer's height means nothing in the other lanes,
    /// and any off-centre window pushes the trace out of its lane and clips it.
    fn centres_y_zoom(&self) -> bool {
        false
    }

    /// **How wide a window of a tapped bus one read has to bring**, in frames
    /// at `sample_rate`, or `0` (the default) for an element that reads no
    /// taps.
    ///
    /// It is a length and not a set: *which* buses are read is
    /// [`Needs::taps`], and the page's one `/bus_tapStream` subscription serves
    /// every consumer at the widest window any of them asks for. So an element
    /// answers for itself and never for the window — a scope's display window
    /// plus its trigger slack, a goniometer's window, a spectrum's FFT size —
    /// and a subscription that is too narrow is not a slow drawing but a blank
    /// one, since a source refuses a read it cannot fill.
    ///
    /// The sample rate is a parameter because most of these are declared in
    /// **time** and one window is not the other at 96 kHz.
    fn tap_frames(&self, _sample_rate: f64) -> usize {
        0
    }

    /// **A bulk resource this element asked for has arrived.** Returns whether
    /// it was taken, so a loader can log what it resolved for nobody.
    ///
    /// The element places the data itself, in whatever shape it draws from —
    /// which is the half of the bulk seam that cannot be a declaration: what
    /// comes back is a pyramid, a set of analyses or a run of samples, and only
    /// the element knows what it is for.
    fn bulk(&mut self, _data: Loaded) -> bool {
        false
    }

    /// The [`BodyRole`] this element fills when a container holds it as one of
    /// its bodies, or `None` (the default) for an element that is only ever
    /// itself.
    ///
    /// It is how a container **recognizes** one of its bodies, which used to be
    /// a match on the leaf's variant in every pass that layered, routed a
    /// `/gui_set`, drew or hit-tested one. The container asks the element what
    /// role it fills and learns nothing else about it — so an element family
    /// can fill a clip's curve without the clip knowing what a curve is.
    fn body_role(&self) -> Option<BodyRole> {
        None
    }

    /// What the GPU slot this element claimed draws this frame, or `None` for
    /// an element that claimed none (the default). An element that claims one
    /// usually still [`draw`](Element::draw)s — a label, a frame — into the
    /// shared mesh around it.
    fn slot(&self, _ctx: &Ctx) -> Option<SlotFrame> {
        None
    }

    /// **What the claimed slot is fed**, or `None` (the default) when the
    /// element has nothing new for it — which is every element that claimed no
    /// slot, and every frame of one whose picture did not move.
    ///
    /// It is a *taking*: the element hands the content over and marks itself
    /// clean, so the front's walk uploads once per change rather than once per
    /// tick. That is why it is separate from [`slot`](Element::slot), which
    /// describes a draw and borrows.
    ///
    /// Only the element knows when its picture moved and what shape the upload
    /// has — a pyramid at its own bucket, an analysis at its own window and
    /// hop, the columns a rolling transform just produced — so the front's walk
    /// asks every widget the same question and learns nothing about any of
    /// them.
    fn fill(&mut self) -> Option<SlotFill> {
        None
    }

    /// **The slot's contents are gone**: the window's GPU resources were
    /// rebuilt (a fresh device, a page's canvas re-attached), so whatever this
    /// element handed over is no longer on the card and the next
    /// [`fill`](Element::fill) has to hand it over again.
    ///
    /// It is the one thing a filling element cannot work out for itself — the
    /// device is the front's — and it is why a fill can be a taking at all: an
    /// element marks itself clean because the frame kept what it gave, and this
    /// is how it is told that it did not.
    fn slot_dropped(&mut self) {}

    /// Whether the wheel **falls through** this element to whatever is behind
    /// it. True for something that only puts marks on its rect and has no
    /// navigation of its own (a label): in a window with one navigation group,
    /// its pixels are that axis with something written on them. False — the
    /// default — for anything drawing a picture it owns,
    /// since turning the wheel over a goniometer must not zoom the waterfall
    /// underneath it.
    fn is_bare_surface(&self) -> bool {
        false
    }

    /// **The shape this element answers the pointer on**, inside its
    /// placement — the rectangle by default, which is what anything filling
    /// its cell wants.
    ///
    /// It is declared rather than tested here because the machine applies it
    /// before the press is offered at all (adding the hit slop), so an element
    /// drawn smaller or rounder than its cell gets the filter for free and a
    /// press outside its shape falls back to the chain, exactly as a decline
    /// does. What this is *not* is the finer question of which part of itself
    /// was hit — that stays in [`press`](Element::press), where the element has
    /// its own geometry.
    fn hit_area(&self, input: &Input) -> HitArea {
        HitArea::Rect(input.rect)
    }

    /// The press landed on this element at `at`, in the window's pixels.
    /// Declining hands it back to the chain.
    ///
    /// **Which part of itself was hit is the element's own business**, here,
    /// where it has both the point and its own geometry — a caret, a
    /// break-point, a patcher's port. The host's hit-test answers *which
    /// widget*, which is rect containment over the placements and is generic;
    /// putting a part on this trait would make the host route by a type it
    /// cannot interpret.
    ///
    /// A [`Claim::Take`] **holds the press**: everything that follows is this
    /// element's until the button comes up.
    fn press(&mut self, _at: (f64, f64), _input: &Input) -> Claim {
        Claim::Decline
    }

    /// The cursor moved while this element held the press. It mutates *itself*
    /// — the drag's state is the element's, because the element is the only
    /// thing that knows what its drag means — and reports what changed.
    fn drag(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        Events::none()
    }

    /// The same, for a drag the front **grabbed the pointer** for
    /// ([`Take::grab`]): the cursor stays put, so motion arrives as a delta
    /// rather than as a position. An element that asked for the grab must
    /// implement this one; an element that did not never sees it.
    fn drag_relative(&mut self, _delta: (f64, f64), _input: &Input) -> Events {
        Events::none()
    }

    /// The button came up. What the drag *delivers* — the edit-back an owner
    /// applies, a momentary control's zero — as against what it showed along
    /// the way.
    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        Events::none()
    }

    /// Whether this element takes the **keyboard focus** — whether it is a stop
    /// on the window's tab ring, and whether a press on it moves the focus
    /// there. `false` by default: an element that answers no key has no reason
    /// to be a stop, and a ring full of them is a ring nobody can use.
    fn accepts_focus(&self) -> bool {
        false
    }

    /// A key while this element holds the focus, with the modifiers and the
    /// host-wide clipboard in [`KeyInput`]. `Some` is **consumed** — the window
    /// repaints and whatever came back is reported — and `None` hands the key
    /// on to the front's own shortcuts, which is what a key an element has no
    /// arm for must do.
    ///
    /// It is the whole keyboard an element gets, and it never sees
    /// [`Key::Tab`]: moving the focus is the window's, not the focused
    /// element's.
    fn key(&mut self, _key: &Key, _input: &mut KeyInput) -> Option<Events> {
        None
    }

    /// A **live MIDI note** for an element that declared [`Needs::midi`].
    ///
    /// `playhead` is where the axis' transport stands, in the element's own
    /// units, or `None` when it is stopped — the one fact the element cannot
    /// read for itself (the engine clock is the front's) and the whole
    /// difference between *recording* a note at the playhead and *entering* one
    /// on a step cursor the element keeps itself.
    ///
    /// Whatever comes back is delivered exactly as a drag's is, so a painted
    /// note reports the same payload an edited one does.
    fn midi(&mut self, _note: MidiNote, _playhead: Option<f64>) -> Option<Events> {
        None
    }

    /// **One tick**: advance whatever this element keeps of the outside — a
    /// rolling history, a triggered window, an analysis state.
    ///
    /// It runs at the front's steady tick rate and **not** per repaint, which
    /// is the whole reason it is a method of its own: a scope scrolls at a rate
    /// the reader can read, whatever the window's repaint rate happens to be.
    /// Everything it produces is the element's own state, so the draw that
    /// follows only ever *draws* it.
    fn tick(&mut self, _live: &Live) {}

    /// The wheel turned over this element: `delta` in the front's scroll units,
    /// `None` to let it fall through to whatever is behind. Only reached when
    /// [`is_bare_surface`](Element::is_bare_surface) is false — a bare surface
    /// never sees the wheel, because its pixels belong to the axis under them.
    fn wheel(&mut self, _at: (f64, f64), _delta: (f64, f64), _input: &Input) -> Option<Events> {
        None
    }

    /// The area this element occupies **outside its own rect**, in window
    /// pixels — an open list, a popup — or `None` (the default) for an element
    /// that stays inside its placement.
    ///
    /// Declaring it is what makes an overlay work, and it is declared rather
    /// than flagged because two different passes need the same answer: the
    /// frame draws [`overlay`](Element::overlay) over everything else, and the
    /// press routes to this element **first**, before the tree, however the
    /// layout places what happens to be under the point. An element with an
    /// overlay open swallows the press either way — on its own area it acts,
    /// anywhere else it closes — which is what a menu everywhere else does.
    fn overlay_rect(&self) -> Option<Rect> {
        None
    }

    /// Draws the [`overlay_rect`](Element::overlay_rect) area, into the
    /// window's **overlay** mesh — the second pass, over the heavy views and
    /// over every other widget. A list that opens covers what it opens over.
    fn overlay(&self, _d: &mut Draw, _ctx: &Ctx) {}

    /// Clones this element into a fresh box (the tree is `Clone`).
    fn clone_box(&self) -> Box<dyn Element>;
}

impl Clone for Box<dyn Element> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Builds one element from the wire's props and the `/gui_def` message's
/// trailing blobs — the registered counterpart of a [`build`](super::build)
/// arm. An `Err` is a malformed node and is reported the way a built-in's is.
pub type Constructor = fn(&Map<String, Value>, &[Vec<u8>]) -> Result<Box<dyn Element>, String>;

thread_local! {
    static REGISTRY: RefCell<HashMap<String, Constructor>> = RefCell::new(HashMap::new());
}

/// Registers `ctor` under the wire `type` name, replacing any registration
/// under the same name (which is how a program overrides its own, and what
/// makes a test's registration repeatable).
///
/// A name that collides with a built-in is accepted and never consulted: the
/// built-ins are matched first, deliberately, so a registration cannot change
/// what an existing def means.
pub fn register(name: &str, ctor: Constructor) {
    REGISTRY.with(|r| r.borrow_mut().insert(name.to_string(), ctor));
}

/// Drops a registration, returning whether there was one.
pub fn unregister(name: &str) -> bool {
    REGISTRY.with(|r| r.borrow_mut().remove(name).is_some())
}

/// Builds the element registered under `name`, or `None` when nothing is —
/// a registry miss, which the caller turns into
/// [`WidgetKind::Unknown`](super::WidgetKind::Unknown).
pub(super) fn build_registered(
    name: &str,
    props: &Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Option<Result<Box<dyn Element>, String>> {
    let ctor = REGISTRY.with(|r| r.borrow().get(name).copied())?;
    Some(ctor(props, blobs))
}

#[cfg(test)]
mod tests {
    //! The seam's own suite, driven the way a third party reaches it: register
    //! a constructor, then parse a `/gui_def` document that names it and put
    //! the result through the passes — because that round trip *is* what the
    //! trait promises.
    //!
    //! The registry is per thread and the harness is parallel, so every test
    //! registers what it needs; the names are test-local for the same reason.

    use super::super::super::guidef::GuiNode;
    use super::super::{Widget, WidgetKind};
    use super::*;

    /// The smallest complete element: a counter with a label, a value a script
    /// can set and a press that increments it.
    #[derive(Debug, Clone)]
    struct Counter {
        count: i32,
        bus: i32,
    }

    impl Element for Counter {
        fn set(&mut self, key: &str, v: &Value) -> bool {
            match key {
                "count" => v.as_i64().map(|n| self.count = n as i32).is_some(),
                "bus" => v.as_i64().map(|n| self.bus = n as i32).is_some(),
                _ => false,
            }
        }

        fn draw(&self, d: &mut Draw, ctx: &Ctx) {
            let (mesh, _, theme) = d.parts();
            mesh.rect(ctx.rect, theme.panel);
        }

        fn natural(&self, m: &Metrics, scale: f32) -> Natural {
            (None, Some(m.control_h * scale))
        }

        fn value(&self) -> Option<OscType> {
            Some(OscType::Int(self.count))
        }

        fn info(&self) -> Vec<(String, Value)> {
            vec![("count".into(), Value::from(self.count))]
        }

        fn needs(&self) -> Needs {
            // Deliberately the *whole* declaration: a registered element is the
            // only thing that exercises the fields no built-in fills any more,
            // and each of them is a collector that must not have to know what a
            // counter is.
            Needs {
                buses: vec![self.bus],
                levels: vec![self.bus + 1],
                taps: vec![self.bus + 2],
                retention: 0.5,
                node_groups: vec![self.bus + 3],
                animated: true,
                clock: true,
                midi: false,
                slot: None,
                bulk: Some(Bulk::Buffer(self.bus + 4)),
            }
        }

        fn tap_frames(&self, sample_rate: f64) -> usize {
            // A hundredth of a second of the tap it declared: a length in time,
            // like every real one.
            (sample_rate / 100.0) as usize
        }

        fn gutter(&self, m: &Metrics) -> f32 {
            // A band of its own left of the body, like a value ruler's.
            m.ruler_w
        }

        fn gesture_map(&self) -> Option<GestureMap> {
            // Placed on somebody's axis: a plain drag is the axis' selection,
            // Shift its pan -- the table a navigable view wants.
            use super::super::GestureStep::*;
            Some(GestureMap::of_plans(
                &[Select],
                &[Pan],
                &[Select],
                &[Select],
            ))
        }

        fn press(&mut self, _at: (f64, f64), _input: &Input) -> Claim {
            self.count += 1;
            Claim::value(OscType::Int(self.count))
        }

        fn accepts_focus(&self) -> bool {
            true
        }

        fn key(&mut self, key: &Key, _input: &mut KeyInput) -> Option<Events> {
            // Up counts, down counts back; anything else is not this element's
            // and falls through to the front's own shortcuts.
            self.count += match key {
                Key::Up => 1,
                Key::Down => -1,
                _ => return None,
            };
            Some(Events::value(OscType::Int(self.count)))
        }

        fn clone_box(&self) -> Box<dyn Element> {
            Box::new(self.clone())
        }
    }

    fn counter(props: &Map<String, Value>, _blobs: &[Vec<u8>]) -> Result<Box<dyn Element>, String> {
        Ok(Box::new(Counter {
            count: props.get("count").and_then(Value::as_i64).unwrap_or(0) as i32,
            bus: props.get("bus").and_then(Value::as_i64).unwrap_or(-1) as i32,
        }))
    }

    fn tree(json: &str) -> Widget {
        Widget::from_node(1, &GuiNode::parse(json.as_bytes()).unwrap(), &[]).unwrap()
    }

    /// The whole promise in one test: a name nothing built in answers to
    /// reaches the registry, and what comes back goes through the passes as a
    /// widget rather than as a hole in the tree.
    #[test]
    fn a_registered_name_builds_and_answers_every_pass() {
        register("test_counter", counter);
        let mut w = tree(r#"{"id":9,"type":"test_counter","count":3,"bus":7}"#);
        assert!(matches!(w.kind, WidgetKind::Custom(_)), "{:?}", w.kind);

        assert_eq!(w.kind.event_value(), Some(OscType::Int(3)));
        assert_eq!(w.kind.needs().buses, vec![7]);
        assert_eq!(w.kind.needs().taps, vec![9]);
        assert_eq!(w.kind.needs().retention, 0.5);
        // And how wide a read of that tap has to be, which is what sizes the
        // page's one subscription: a length in time, resolved at the rate.
        assert_eq!(w.kind.tap_frames(48_000.0), 480);
        assert_eq!(w.kind.tap_frames(96_000.0), 960);
        let m = Metrics::default();
        assert_eq!(w.kind.natural_size(&m, 1.0), (None, Some(m.control_h)));

        // A `/gui_set` lands on the element's own key; one it does not know is
        // reported as unhandled rather than swallowed.
        assert!(super::super::apply_widget(
            &mut w,
            "count",
            &Value::from(11)
        ));
        assert!(!super::super::apply_widget(
            &mut w,
            "nonesuch",
            &Value::from(1)
        ));
        assert_eq!(w.kind.event_value(), Some(OscType::Int(11)));

        // The chrome and the drag are the element's too: the band it reserves
        // left of its body on a shared axis, and the table the press walk
        // reads when the wire declares none.
        assert_eq!(w.kind.gutter(&m), m.ruler_w);
        assert_eq!(
            super::super::GestureMap::of_kind(&w.kind),
            super::super::GestureMap::of_plans(
                &[super::super::GestureStep::Select],
                &[super::super::GestureStep::Pan],
                &[super::super::GestureStep::Select],
                &[super::super::GestureStep::Select],
            )
        );

        // And it is a stop on the window's tab ring, which is the whole of
        // what a keyboard costs an element: one declaration and one method.
        assert!(w.kind.accepts_focus());
        let WidgetKind::Custom(el) = &mut w.kind else {
            unreachable!()
        };
        let mut clipboard = crate::host::clipboard::Clip::default();
        let mut input = KeyInput {
            mods: Mods::default(),
            clipboard: &mut clipboard,
        };
        assert_eq!(
            el.key(&Key::Up, &mut input),
            Some(Events::value(OscType::Int(12)))
        );
        assert_eq!(
            el.key(&Key::Char('q'), &mut input),
            None,
            "a key it has no arm for falls through to the front's shortcuts"
        );

        unregister("test_counter");
    }

    /// A registry miss is what an unrecognized type has always been, which is
    /// the property that lets an element family be compiled out of a build: it
    /// degrades to the behavior of a host older than the def.
    #[test]
    fn a_miss_is_unknown_not_an_error() {
        let w = tree(r#"{"id":9,"type":"nothing_registered_here"}"#);
        assert!(
            matches!(w.kind, WidgetKind::Unknown(ref t) if t == "nothing_registered_here"),
            "{:?}",
            w.kind
        );
    }

    /// The built-ins are matched first, so a registration under a name one
    /// already answers to is inert — it cannot change what a shipped def means.
    #[test]
    fn a_registration_never_shadows_a_built_in() {
        register("label", counter);
        let w = tree(r#"{"id":9,"type":"label","text":"hello"}"#);
        // The built-in label answered: it reports its text and has no value,
        // where the registered counter would have reported a count.
        assert_eq!(w.kind.event_value(), None, "{:?}", w.kind);
        unregister("label");
    }

    /// The tree is `Clone` (a def is rebuilt by replacement and a frame copies
    /// out of it), so a boxed element clones deeply rather than aliasing.
    #[test]
    fn a_boxed_element_clones_deeply() {
        register("test_clone", counter);
        let mut w = tree(r#"{"id":9,"type":"test_clone","count":1}"#);
        let copy = w.clone();
        super::super::apply_widget(&mut w, "count", &Value::from(5));
        assert_eq!(w.kind.event_value(), Some(OscType::Int(5)));
        assert_eq!(copy.kind.event_value(), Some(OscType::Int(1)));
        unregister("test_clone");
    }

    /// A constructor that rejects its props fails the def the way a malformed
    /// built-in node does, rather than being reported as an unknown type.
    #[test]
    fn a_constructor_error_fails_the_def() {
        fn refuses(
            _props: &Map<String, Value>,
            _blobs: &[Vec<u8>],
        ) -> Result<Box<dyn Element>, String> {
            Err("no".into())
        }
        register("test_refuses", refuses);
        let node = GuiNode::parse(br#"{"id":9,"type":"test_refuses"}"#).unwrap();
        assert_eq!(Widget::from_node(1, &node, &[]).err(), Some("no".into()));
        unregister("test_refuses");
    }
}
