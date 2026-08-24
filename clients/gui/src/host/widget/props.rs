//! **The prop bundles**: the groups of props a widget carries beside its kind,
//! and the small vocabularies they are made of.
//!
//! A bundle is what several kinds share, named once. The layout props any
//! widget may carry ([`Place`], [`Flow`]); the editor chrome every timeline
//! view carries ([`EditorProps`], with [`Ruler`]/[`RulerY`]); the window a
//! `scroll` shows its content through ([`ScrollView`], with [`Axis`]); the
//! gesture table a container declares ([`GestureMap`], with [`GesturePlan`] and
//! [`GestureStep`]); the value the continuous controls share ([`Range`]); and
//! the two words the wire uses to say how a container arranges its children
//! ([`Layout`], [`Align`]) and how a data view reads its bus ([`Rate`]).
//!
//! They live here rather than beside [`WidgetKind`] for the same reason the
//! enum is worth reading on its own: a bundle is *not* part of the model's
//! shape — it is a detail of a variant's payload — and a reader looking for the
//! model should not walk 800 lines of them to reach it. Each one owns its own
//! `apply`/`parse` where it has one, so a new prop on a bundle is one edit.

use clausters_core::warp;
use serde_json::Value;

use super::WidgetKind;
use super::parse::*;

/// How a container arranges its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Row,
    Col,
    Grid,
    Free,
}

impl Layout {
    /// Parses the `flow` property; defaults to `Col`. (`flow`, not `layout`:
    /// the model spends that word on the container type itself.)
    pub(super) fn parse(props: &serde_json::Map<String, Value>) -> Layout {
        flow(props)
            .and_then(Layout::from_str)
            .unwrap_or(Layout::Col)
    }

    pub(super) fn from_str(s: &str) -> Option<Layout> {
        match s {
            "row" => Some(Layout::Row),
            "col" => Some(Layout::Col),
            "grid" => Some(Layout::Grid),
            "free" => Some(Layout::Free),
            _ => None,
        }
    }
}

/// Horizontal alignment of a `label`'s text inside its rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

impl Align {
    /// Parses the `align` property; defaults to `Start` (today's left edge).
    pub(crate) fn parse(props: &serde_json::Map<String, Value>) -> Align {
        props
            .get("align")
            .and_then(Value::as_str)
            .and_then(Align::from_str)
            .unwrap_or(Align::Start)
    }

    pub(crate) fn from_str(s: &str) -> Option<Align> {
        match s {
            "start" => Some(Align::Start),
            "center" => Some(Align::Center),
            "end" => Some(Align::End),
            _ => None,
        }
    }
}

/// How an editor-grade view labels its time (x) ruler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ruler {
    /// Adaptive clock time (`h:mm:ss.mmm`), falling back to sample counts
    /// when no sample rate is known. The default.
    Time,
    /// Plain sample counts.
    Samples,
    /// Musical time on the client's beat grid: `bar:beat` labels from the
    /// `tempo`/`beat_at`/`quant` props (falls back to sample counts when no
    /// rate or tempo is known).
    Beats,
    /// No ruler strip at all.
    Off,
}

impl Ruler {
    pub(super) fn parse(props: &serde_json::Map<String, Value>) -> Ruler {
        Self::parse_with(props, Ruler::Time)
    }

    /// The `ruler` prop over a presentation's own default — absent keeps the
    /// default, and a **boolean** switches the strip off or back on, which is
    /// how the live views have always spelled it (their x unit is not
    /// selectable, so only on/off was ever meaningful there).
    pub(crate) fn parse_with(props: &serde_json::Map<String, Value>, default: Ruler) -> Ruler {
        match props.get("ruler") {
            None => default,
            Some(v) => match v.as_str() {
                Some("samples") => Ruler::Samples,
                Some("beats") => Ruler::Beats,
                Some("off") | Some("none") => Ruler::Off,
                Some(_) => Ruler::Time,
                None => match truthy(v) {
                    Some(false) => Ruler::Off,
                    _ => Ruler::Time,
                },
            },
        }
    }

    pub(super) fn set(&mut self, v: &Value) -> bool {
        match v.as_str() {
            Some("samples") => *self = Ruler::Samples,
            Some("beats") => *self = Ruler::Beats,
            Some("off") | Some("none") => *self = Ruler::Off,
            Some("time") => *self = Ruler::Time,
            // The live views' on/off spelling.
            None => match truthy(v) {
                Some(b) => *self = if b { Ruler::Time } else { Ruler::Off },
                None => return false,
            },
            _ => return false,
        }
        true
    }
}

/// The vertical (y) ruler of an editor-grade view: the unit its side strip
/// labels, or `Off` for no strip at all. The waveform reads the amplitude
/// units (`Norm`/`Db`/`Bits`/`Percent`, default `Norm`); the spectrogram uses
/// `Hz` (default) or `Off` — its tick *positions* follow the widget's
/// `freq_scale`, the labels stay in hertz.
///
/// **The unit labels the axis; it never maps it.** The geometry is linear in
/// amplitude on every one of these, and `Db` is a ladder of rungs placed at the
/// amplitudes those decibels are (`ruler::amp_ticks`), not a logarithmic body.
/// So the value under a height is the same whichever unit is printed beside it,
/// and editing is in linear amplitude and only there — a decision, recorded at
/// "A take is drawn in amplitude and heard in decibels" in the GUI plan and
/// held by `signal::tests::the_amplitude_unit_labels_the_axis_and_never_maps_it`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulerY {
    /// No vertical ruler strip.
    Off,
    /// Normalized amplitude in [-1, 1] (the waveform default).
    Norm,
    /// dBFS (0 at full scale, symmetric about the zero line).
    Db,
    /// Integer sample values at the `bit_depth` prop's resolution.
    Bits,
    /// Amplitude as a 0-100% proportion of full scale.
    Percent,
    /// Frequency in hertz (the spectrogram default).
    Hz,
}

impl RulerY {
    pub(super) fn parse(props: &serde_json::Map<String, Value>, default: RulerY) -> RulerY {
        match props.get("ruler_y") {
            None => default,
            Some(v) => match v.as_str() {
                Some(s) => Self::from_str(s).unwrap_or(default),
                // A boolean switches the strip off or back on — the live
                // views' spelling, where the unit is the presentation's.
                None => match truthy(v) {
                    Some(false) => RulerY::Off,
                    _ => default,
                },
            },
        }
    }

    pub(super) fn from_str(s: &str) -> Option<RulerY> {
        Some(match s {
            "off" | "none" => RulerY::Off,
            "norm" | "amp" => RulerY::Norm,
            "db" | "dbfs" => RulerY::Db,
            "bits" | "samples" => RulerY::Bits,
            "percent" => RulerY::Percent,
            "hz" => RulerY::Hz,
            _ => return None,
        })
    }

    pub(super) fn set(&mut self, v: &Value) -> bool {
        match v.as_str().and_then(Self::from_str) {
            Some(u) => {
                *self = u;
                true
            }
            None => false,
        }
    }
}

/// One view window as a valid display-axis slice: a non-positive length is the
/// whole axis, anything else clamps into `[0, 1]` with the shared zoom floor.
/// The one reading both of an element's own axes go through.
fn normalized_window(start: f64, len: f64) -> (f64, f64) {
    let mut axis = crate::viewport::Axis::normalized(crate::viewport::Unit::Norm);
    axis.set_span(start, len);
    axis.span()
}

/// The editor chrome both heavy views share: the time-ruler (x) mode and the
/// vertical (y) ruler unit — each independently switchable off, each drawn in
/// its own strip beside the body — the sample rate placing the time labels
/// (0 = unknown), the beat grid of the `beats` ruler (`tempo` in beats per
/// second — the client `Clock` convention — `beat_at` the beat position of
/// buffer sample 0, `quant` the beats per bar), the `bit_depth` the `bits`
/// amplitude unit quantizes to, a `[sel_start, sel_len)` selection in sample
/// units (`sel_len <= 0` = none; drawn as an overlay, dragged with the
/// pointer, round-tripped as a `"selection"` event / `/gui_set`), and the
/// playhead origin `playhead_at` — the engine sample-clock value that maps to
/// buffer sample 0 (negative = no playhead; the line then tracks
/// `sample_clock - playhead_at` with zero messages natively) — and the
/// **vertical view window** `y_start`/`y_len` in normalized display units
/// (`0, 1` = the full axis, the default): the visible slice of the amplitude
/// axis (waveform) or of the frequency display axis (spectrogram), zoomed and
/// panned with the pointer on the y-ruler strip, settable via `/gui_set` and
/// reported live as a `"view_y"` event (a non-positive `y_len` resets to the
/// full axis).
///
/// `x_start`/`x_len` are the **horizontal** window of an element that owns its
/// own x axis — a navigable spectrum, whose x measures frequency rather than
/// the window's time — in the same normalized display units and with the same
/// rule (`0, 1` = the whole axis, a non-positive length resets to it), reported
/// as a `"view_x"` event. They arrive on the wire as the x axis' own
/// `view_start`/`view_len` (`axes.x.start`/`len`), which is the same *question*
/// a timeline member's window answers and the reason it is not a second pair of
/// names; what differs is who owns the answer. On a member of a navigation
/// group those keys never reach here — the group model takes them, in samples
/// (see `host::timeline`) — so exactly one of the two readings is ever live for
/// a given widget. Over a frequency axis this pair is the window that was
/// **asked** for and not necessarily the one on the screen: the analysis has a
/// resolution, and
/// [`SignalElement::freq_window`](crate::host::elements::signal::SignalElement::freq_window)
/// opens the request wherever it is finer than the bins are where it sits.
///
/// `link` is the widget's **navigation group** (see `host::timeline`): every
/// timeline view declaring the same link id shares one horizontal view,
/// selection and playhead — a gesture or `/gui_set` on any member applies to
/// all of them. Without a `link` the widget navigates alone. The selection and
/// playhead fields here are the **def-time seed** of that group and nothing
/// more: once the group exists it holds those values, every reader takes them
/// from it, and these fields no longer move. Only the y axis stays per-widget.
///
/// `offset` is the widget's **placement** on its group's shared timeline (in
/// timeline sample units): the view's own data sample 0 sits at timeline
/// position `offset`, so a clip starting late draws shifted right and lengthens
/// its group's timeline to `offset + data_len`. It is per-member (unlike the
/// group-wide `link`/`sel_*`/`view_*`), but a change still re-clamps the group
/// window and repaints every member, so it routes through the group model too.
/// All members are at `offset = 0` until a multitrack layout places them.
#[derive(Debug, Clone)]
pub struct EditorProps {
    pub ruler: Ruler,
    pub ruler_y: RulerY,
    pub sample_rate: f64,
    pub bit_depth: u32,
    pub tempo: f64,
    pub beat_at: f64,
    pub quant: f64,
    pub sel_start: f64,
    pub sel_len: f64,
    pub playhead_at: f64,
    /// A **static** playhead: the timeline position of the transport's cursor
    /// when nothing is playing (`< 0` = none). `playhead_at` anchors the line to
    /// the engine clock and *sweeps*; this one stands still — a located, stopped
    /// transport has a cursor, and it must not drift with the clock.
    pub playhead: f64,
    /// The sweep's **loop region**, in the same sample units as `playhead`:
    /// with `playhead_loop_len > 0` the swept line wraps inside
    /// `[playhead_loop_start, + len)` instead of running straight past it, so
    /// a repeating region can be followed on the same one anchor and still
    /// costs no message per frame. A non-positive length is the straight pass.
    pub playhead_loop_start: f64,
    pub playhead_loop_len: f64,
    pub y_start: f64,
    pub y_len: f64,
    /// The selection's **second axis**: the value range it is restricted to, in
    /// the element's own units (`sel_max <= sel_min` = the whole domain, which
    /// is no restriction at all).
    ///
    /// Per-widget and not the group's, unlike `sel_start`/`sel_len`, for the
    /// reason the y window is: a group is one *time* axis shared by views that
    /// measure different things vertically, so a range in it would restrict a
    /// spectrogram in hertz by a waveform's amplitudes.
    pub sel_min: f64,
    pub sel_max: f64,
    pub x_start: f64,
    pub x_len: f64,
    pub link: Option<i32>,
    pub offset: f64,
}

impl EditorProps {
    /// Parses the shared chrome; `default_y` is the view's own default
    /// vertical unit (`Norm` for the waveform, `Hz` for the spectrogram).
    pub(crate) fn parse(props: &serde_json::Map<String, Value>, default_y: RulerY) -> EditorProps {
        EditorProps {
            ruler: Ruler::parse(props),
            ruler_y: RulerY::parse(props, default_y),
            sample_rate: number_f64(props, "sample_rate", 0.0),
            bit_depth: props
                .get("bit_depth")
                .and_then(Value::as_u64)
                .map(|n| (n as u32).clamp(2, 32))
                .unwrap_or(16),
            tempo: number_f64(props, "tempo", 1.0),
            beat_at: number_f64(props, "beat_at", 0.0),
            quant: number_f64(props, "quant", 4.0),
            sel_start: number_f64(props, "sel_start", 0.0),
            sel_len: number_f64(props, "sel_len", 0.0),
            playhead_at: number_f64(props, "playhead_at", -1.0),
            playhead: number_f64(props, "playhead", -1.0),
            playhead_loop_start: number_f64(props, "playhead_loop_start", 0.0),
            playhead_loop_len: number_f64(props, "playhead_loop_len", 0.0),
            y_start: number_f64(props, "y_start", 0.0),
            y_len: number_f64(props, "y_len", 1.0),
            sel_min: number_f64(props, "sel_min", 0.0),
            sel_max: number_f64(props, "sel_max", 0.0),
            x_start: number_f64(props, "view_start", 0.0),
            x_len: number_f64(props, "view_len", 1.0),
            link: props
                .get("link")
                .and_then(Value::as_i64)
                .filter(|n| *n >= 0)
                .map(|n| n as i32),
            offset: number_f64(props, "offset", 0.0).max(0.0),
        }
    }

    /// The chrome of a **clip body**: none of it. A body is drawn against the
    /// axes of the clip holding it, so it owns no ruler, no selection, no
    /// playhead and no navigation group — everything a container answers for.
    pub(crate) fn body() -> EditorProps {
        EditorProps {
            ruler: Ruler::Off,
            ruler_y: RulerY::Off,
            ..EditorProps::parse(&serde_json::Map::new(), RulerY::Off)
        }
    }

    /// The chrome of a `track` lane: the same props, but the time ruler is
    /// **off** unless asked for (a lane reserves no ruler strip by default, so
    /// an un-rulered multitrack keeps the layout it had) and it carries no
    /// vertical ruler. The lane uses `ruler`/`playhead_at` (plus the `tempo`/
    /// `beat_at`/`quant`/`sample_rate` the tick labels read); the rest is inert.
    pub(super) fn parse_lane(props: &serde_json::Map<String, Value>) -> EditorProps {
        let mut editor = EditorProps::parse(props, RulerY::Off);
        if !props.contains_key("ruler") {
            editor.ruler = Ruler::Off;
        }
        editor
    }

    /// The vertical view window as a valid display-axis slice: a non-positive
    /// length resets to the full axis, anything else clamps into `[0, 1]`
    /// (with the shared zoom floor). The raw `y_start`/`y_len` props are kept
    /// as set and validated only here, at read time — clamping inside
    /// `apply` would make one `/gui_set` carrying both keys order-dependent
    /// (`y_start` would clamp against the *old* `y_len` before the new one
    /// lands).
    pub fn y_view(&self) -> (f64, f64) {
        normalized_window(self.y_start, self.y_len)
    }

    /// The selection's value range, or `None` where it is not restricted on
    /// that axis — the pair read the way [`Self::y_view`] reads the window,
    /// with the ordering done here rather than in `apply`, so one `/gui_set`
    /// carrying both keys does not depend on their order.
    ///
    /// An empty or inverted pair is *no restriction*, deliberately the same
    /// convention `sel_len <= 0` uses on the time axis: a selection that names
    /// no range holds the whole domain, and travels as the plain two-number
    /// span the wire has always carried.
    pub fn value_range(&self) -> Option<(f64, f64)> {
        (self.sel_max > self.sel_min).then_some((self.sel_min, self.sel_max))
    }

    /// The horizontal view window of an element that owns its x axis, read the
    /// same way [`Self::y_view`] reads the vertical one — validated here rather
    /// than in `apply`, for the same reason: one `/gui_set` carrying both keys
    /// must not depend on their order.
    pub fn x_view(&self) -> (f64, f64) {
        normalized_window(self.x_start, self.x_len)
    }

    pub(crate) fn apply(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "ruler" => self.ruler.set(v),
            "ruler_y" => self.ruler_y.set(v),
            "sample_rate" => set_f64(&mut self.sample_rate, v),
            "bit_depth" => v
                .as_u64()
                .map(|n| self.bit_depth = (n as u32).clamp(2, 32))
                .is_some(),
            "tempo" => set_f64(&mut self.tempo, v),
            "beat_at" => set_f64(&mut self.beat_at, v),
            "quant" => set_f64(&mut self.quant, v),
            "sel_start" => set_f64(&mut self.sel_start, v),
            "sel_len" => set_f64(&mut self.sel_len, v),
            "playhead_at" => set_f64(&mut self.playhead_at, v),
            "playhead" => set_f64(&mut self.playhead, v),
            "playhead_loop_start" => set_f64(&mut self.playhead_loop_start, v),
            "playhead_loop_len" => set_f64(&mut self.playhead_loop_len, v),
            "y_start" => set_f64(&mut self.y_start, v),
            "y_len" => set_f64(&mut self.y_len, v),
            "sel_min" => set_f64(&mut self.sel_min, v),
            "sel_max" => set_f64(&mut self.sel_max, v),
            "view_start" => set_f64(&mut self.x_start, v),
            "view_len" => set_f64(&mut self.x_len, v),
            _ => false,
        }
    }
}

/// A container's flow tuning: the inner `margin`, the `gap` between children,
/// and a fixed `cols` count for the `grid` layout. Absent values keep the
/// defaults the layout engine always used (margin 6, gap 6, near-square grid).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Flow {
    pub margin: Option<f32>,
    pub gap: Option<f32>,
    pub cols: Option<u32>,
}

impl Flow {
    pub(super) fn parse(props: &serde_json::Map<String, Value>) -> Flow {
        let f = |k: &str| props.get(k).and_then(Value::as_f64).map(|v| v as f32);
        Flow {
            margin: f("margin"),
            gap: f("gap"),
            cols: props
                .get("cols")
                .and_then(Value::as_u64)
                .map(|n| (n as u32).max(1)),
        }
    }

    /// Applies one `/gui_set` key. `true` if the key is a flow prop.
    pub fn apply(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "margin" => {
                self.margin = v.as_f64().map(|n| n as f32);
                true
            }
            "gap" => {
                self.gap = v.as_f64().map(|n| n as f32);
                true
            }
            "cols" => {
                self.cols = v.as_u64().map(|n| (n as u32).max(1));
                true
            }
            _ => false,
        }
    }
}

/// Which axes a `scroll` workspace pans along. The default is the full 2D
/// workspace (`Both`); the constrained scroll views degrade from it by
/// configuration — `axis: "y"` is a plain vertical scroll view, `axis: "x"` a
/// horizontal strip. One widget, one gesture path; the axis only gates the
/// *panning* gestures, never the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Both,
    X,
    Y,
}

impl Axis {
    /// The overscroll this axis configuration allows past the content edges
    /// (a fraction of the visible size, for [`super::super::scroll::clamp_pan`]). The
    /// free plane is unbounded and gets slack; a constrained scroll view is a
    /// bounded document and gets none.
    pub fn slack(self) -> f64 {
        match self {
            Axis::Both => super::super::scroll::SLACK,
            Axis::X | Axis::Y => 0.0,
        }
    }

    pub(super) fn parse(props: &serde_json::Map<String, Value>) -> Axis {
        props
            .get("axis")
            .and_then(Value::as_str)
            .and_then(Axis::from_str)
            .unwrap_or(Axis::Both)
    }

    pub(super) fn from_str(s: &str) -> Option<Axis> {
        match s {
            "both" | "xy" => Some(Axis::Both),
            "x" => Some(Axis::X),
            "y" => Some(Axis::Y),
            _ => None,
        }
    }
}

/// One thing a **container** does with a pointer press on it, independent of
/// what is drawn inside it.
///
/// These are the gestures that belong to the coordinate system rather than to
/// the element: panning is panning whether the axis carries a waveform, a lane
/// of clips or a piano-roll, and a container that owns an axis owns them all.
/// [`GestureStep::Element`] is where the element under the cursor gets its
/// turn — a note dragged, a clip grabbed, a knob turned — which is why a plan
/// is an *order* rather than a single action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureStep {
    /// Hand the press to whatever is under the cursor: the widget the hit
    /// found, or — inside a container that draws its contents rather than
    /// laying them out — the clip, note or box it placed there. It may decline
    /// (empty space), and then the plan goes on.
    Element,
    /// Pan the container's axis: time on a timeline, the plane on a workspace.
    Pan,
    /// Sweep a selection: the shared time selection on a timeline, restricted
    /// in pitch where the axis has a vertical one. A selection that is the
    /// *element's* — a patcher's box marquee — is not this: the element claims
    /// the press and sweeps it itself, since nothing outside it can say what
    /// the rectangle caught.
    Select,
    /// Sweep a selection **restricted on the container's second axis** — a
    /// rectangle rather than a stripe, over a view that measures a value.
    ///
    /// A step of its own rather than a widening of [`GestureStep::Select`],
    /// because a plain drag over a waveform means *this stretch of time* in
    /// every editor there has ever been, and a marquee that also cut a band of
    /// amplitudes out of it would be answering a question nobody asked. What is
    /// a band of values good for is a script's business — gate this range,
    /// copy only these peaks — so the script asks for it, which is the track's
    /// own rule: a mode is a plan, not a state the host decides.
    ///
    /// It **declines** where the view under it measures no value, so
    /// `"select_box select"` is the honest plan for a mixed stack: a rectangle
    /// where the picture has two axes, the plain span where it has one.
    SelectBox,
    /// Grab the **sample** under the pointer and drag it vertically — the
    /// smallest destructive edit there is, and the one that proves the whole
    /// route (gesture to intent to owner to redraw).
    ///
    /// It **declines where a sample is not a thing on screen**: below the zoom
    /// at which the trace marks each sample with a disc there is nothing to
    /// grab, and a plan naming it falls through to whatever it names next.
    /// That is the same rule as the drawing's, read from the same place, so
    /// what can be grabbed is exactly what is drawn.
    Sample,
    /// **Draw** over the samples: a press-drag writes the value under the
    /// pointer for every sample it passes, and one intent leaves on release.
    ///
    /// Stricter than [`GestureStep::Sample`]: it is refused where a pixel is
    /// more than one sample, because a stroke there would write values the
    /// reader cannot see. The refusal is **visible** (`"refused" "draw" …`)
    /// rather than a silent decline — a pencil that sometimes does nothing
    /// teaches that it sometimes does not work.
    Draw,
    /// Put the transport's cursor under the pointer (a timeline locate).
    Locate,
}

impl GestureStep {
    pub(super) fn from_str(s: &str) -> Option<GestureStep> {
        Some(match s {
            "element" => GestureStep::Element,
            "pan" => GestureStep::Pan,
            "select" => GestureStep::Select,
            "select_box" => GestureStep::SelectBox,
            "sample" => GestureStep::Sample,
            "draw" => GestureStep::Draw,
            "locate" => GestureStep::Locate,
            _ => return None,
        })
    }
}

/// What one modifier does on a container: an ordered plan of up to three
/// steps, each of which may decline, the first that consumes the press winning.
///
/// The order is the whole point. `[Element, Locate]` is a multitrack lane —
/// grab the clip under the cursor, and if there is none, locate the transport;
/// `[Select]` is a waveform, which has nothing under the cursor to grab. A plan
/// that consumes nothing falls **outward** to the enclosing container's plan,
/// which is how Shift+drag on a patcher's empty canvas still pans the workspace
/// around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GesturePlan([Option<GestureStep>; 3]);

impl GesturePlan {
    /// The plan's steps, in order.
    pub fn steps(&self) -> impl Iterator<Item = GestureStep> + '_ {
        self.0.iter().flatten().copied()
    }

    /// A plan from its steps (a longer list is truncated).
    pub(super) fn of(steps: &[GestureStep]) -> GesturePlan {
        let mut plan = GesturePlan::default();
        for (slot, step) in plan.0.iter_mut().zip(steps) {
            *slot = Some(*step);
        }
        plan
    }

    /// Parses a plan's wire form: the step names in order, separated by
    /// whitespace or commas (`"element locate"`). `"none"` (or an empty
    /// string) is the plan that does nothing, so a container's default can be
    /// switched off. An unknown name makes the whole value invalid.
    pub(super) fn parse(s: &str) -> Option<GesturePlan> {
        let mut plan = GesturePlan::default();
        let mut slots = plan.0.iter_mut();
        for name in s.split([' ', ',', '\t']).filter(|t| !t.is_empty()) {
            if name == "none" {
                continue;
            }
            *slots.next()? = Some(GestureStep::from_str(name)?);
        }
        Some(plan)
    }
}

/// A container's **gesture table**: which plan each modifier runs.
///
/// Every container has one, defaulted from what it is ([`GestureMap::of_kind`])
/// and overridable from the wire (the `gestures` prop), so a timeline can be
/// made to pan on a plain drag without touching any element's code. The
/// modifiers
/// are read in order — `ctrl`, `alt`, `shift`, then plain — so a press with
/// several modifiers held resolves to exactly one plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GestureMap {
    pub plain: GesturePlan,
    pub shift: GesturePlan,
    pub ctrl: GesturePlan,
    pub alt: GesturePlan,
}

impl GestureMap {
    /// The plan for a modifier (`ctrl` and `alt` win over `shift`, which wins over
    /// the plain drag).
    pub fn plan(&self, shift: bool, ctrl: bool, alt: bool) -> GesturePlan {
        if ctrl {
            self.ctrl
        } else if alt {
            self.alt
        } else if shift {
            self.shift
        } else {
            self.plain
        }
    }

    /// A table from its four plans, one per modifier — how an element declares
    /// the drag it wants when the wire says nothing
    /// ([`Element::gesture_map`](super::Element::gesture_map)).
    pub fn of_plans(
        plain: &[GestureStep],
        shift: &[GestureStep],
        ctrl: &[GestureStep],
        alt: &[GestureStep],
    ) -> GestureMap {
        GestureMap {
            plain: GesturePlan::of(plain),
            shift: GesturePlan::of(shift),
            ctrl: GesturePlan::of(ctrl),
            alt: GesturePlan::of(alt),
        }
    }

    /// The table a container of this kind carries unless the wire replaces it.
    ///
    /// The timeline views differ only in what a plain drag is for: a waveform
    /// has nothing placed on its axis, so it selects; a lane and a roll hand
    /// the press to the clip or note first; a free-standing ruler is a scrub
    /// strip. Shift pans on all of them — that is the convention the whole
    /// track shares — and a workspace pans with whatever is left over.
    pub fn of_kind(kind: &WidgetKind) -> GestureMap {
        use GestureStep::*;
        // An element answers for itself; the arms below are the containers'.
        if let Some(map) = kind.element_gesture_map() {
            return map;
        }
        let (plain, shift, ctrl, alt): (&[_], &[_], &[_], &[_]) = match kind {
            WidgetKind::Track { .. } => (
                &[Element, Locate],
                &[Pan],
                &[Element, Locate],
                &[Element, Locate],
            ),
            WidgetKind::TimeRuler { .. } => (&[Locate], &[Pan], &[Locate], &[Locate]),
            // A workspace claims nothing: whatever no element and no inner
            // container took pans the plane.
            WidgetKind::Scroll { .. } => (
                &[Element, Pan],
                &[Element, Pan],
                &[Element, Pan],
                &[Element, Pan],
            ),
            // A **clip** takes the plain drag (grab it, move it, resize it) and
            // lets every other modifier fall straight through to the lane around
            // it. It must not answer for `pan`: a clip is a container of its
            // own local `[0, dur]` axis, so panning *it* would mean panning
            // that, while Shift+drag on a timeline means the lane's shared
            // window — which is why an empty plan here is the point and not an
            // omission. Without it a lane could only be panned where no clip
            // was drawn, which on a busy arrangement is nowhere.
            WidgetKind::Clip { .. } => (&[Element], &[], &[Element], &[Element]),
            _ => (&[Element], &[Element], &[Element], &[Element]),
        };
        GestureMap::of_plans(plain, shift, ctrl, alt)
    }

    /// Overlays the `gestures` prop on this table: an object keyed by modifier
    /// (`drag`/`shift`/`ctrl`/`alt`), each value a plan (`"element locate"`).
    /// A bare string sets the plain drag alone. Returns whether the value was
    /// usable at all; an unreadable modifier is warned about and skipped, so one
    /// typo does not drop the rest of the table.
    pub(super) fn overlay(&mut self, v: &Value) -> bool {
        // A string is either a bare plan for the plain drag or the **scalar
        // carrier** of the table (the `theme`/`points` convention, which is how
        // a `/gui_set` sends an object).
        let carried;
        let table = match v {
            Value::Object(table) => table,
            Value::String(s) => match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(t)) => {
                    carried = t;
                    &carried
                }
                _ => {
                    return match GesturePlan::parse(s) {
                        Some(plan) => {
                            self.plain = plan;
                            true
                        }
                        None => false,
                    };
                }
            },
            _ => return false,
        };
        for (modifier, value) in table {
            let Some(plan) = value.as_str().and_then(GesturePlan::parse) else {
                tracing::warn!("gestures: unreadable plan for {modifier:?}");
                continue;
            };
            match modifier.as_str() {
                "drag" | "plain" => self.plain = plan,
                "shift" => self.shift = plan,
                "ctrl" => self.ctrl = plan,
                "alt" => self.alt = plan,
                other => tracing::warn!("gestures: unknown modifier {other:?}"),
            }
        }
        true
    }
}

/// A `scroll` container's 2D window onto its virtual content area: the pan
/// offsets and the scale (all view state, settable via `/gui_set` and emitted
/// as the `"view"` payload when a gesture moves them), plus the configuration
/// that constrains the workspace — the pannable [`Axis`], whether wheel zoom
/// is enabled (`zoom: 0` disables it), and an explicit content size (absent,
/// the content area sizes from the children's free-placement extents, or the
/// widget's own area).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollView {
    pub axis: Axis,
    /// Whether the wheel zooms (`zoom: 0` degrades the workspace to a plain
    /// scroll view; the wheel then pans along the axis).
    pub zoom_enabled: bool,
    pub content_w: Option<f32>,
    pub content_h: Option<f32>,
    /// The content coordinate at the widget's left edge.
    pub view_x: f64,
    /// The content coordinate at the widget's top edge.
    pub view_y: f64,
    /// Physical pixels per content unit (uniform on both axes), > 0 —
    /// `None` until something names one, which is not the same as `1.0`; see
    /// [`zoom`](Self::zoom). A `/gui_set view_zoom` of `0` (or of any
    /// non-number) puts it back to `None`.
    pub view_zoom: Option<f64>,
}

impl ScrollView {
    /// The navigation a gesture wrote — where the plane is and how far in it is
    /// zoomed, under the keys that set them.
    ///
    /// `view_zoom` is reported only when something named one: `None` is "the
    /// density of the screen it is drawn on", which is not a number the plane
    /// holds and not one a script has to send back.
    pub fn info(&self) -> Vec<(String, serde_json::Value)> {
        let mut out = vec![
            ("view_x".into(), serde_json::Value::from(self.view_x)),
            ("view_y".into(), serde_json::Value::from(self.view_y)),
        ];
        if let Some(zoom) = self.view_zoom {
            out.push(("view_zoom".into(), serde_json::Value::from(zoom)));
        }
        out
    }

    /// This plane's scale in physical pixels per content unit, resolved against
    /// the window's metrics.
    ///
    /// The default is the window's **UI scale**, not `1.0`, because a plane's
    /// content unit is a *display* unit: a patcher's box is 96 units wide
    /// because that is how wide a box should look, so one content unit is one
    /// **logical** pixel and the plane starts at the density it is drawn on.
    /// (The alternative — fitting the zoom to the content — was rejected: it
    /// would make a box's apparent size follow *how many boxes there are*, and
    /// re-zoom the plane on every edit. Zoom-to-fit is a command, not a
    /// default.)
    ///
    /// Naming one — in the wire, or by turning the wheel — makes it literal
    /// from then on: this number is physical pixels, the unit the pan and the
    /// hit math are written in.
    pub fn zoom(&self, m: &super::super::metrics::Metrics) -> f64 {
        super::super::scroll::clamp_zoom(self.view_zoom.unwrap_or(m.ui_scale as f64))
    }

    pub(super) fn parse(props: &serde_json::Map<String, Value>) -> ScrollView {
        let f = |k: &str| props.get(k).and_then(Value::as_f64).map(|v| v as f32);
        ScrollView {
            axis: Axis::parse(props),
            zoom_enabled: props.get("zoom").and_then(truthy).unwrap_or(true),
            content_w: f("content_w"),
            content_h: f("content_h"),
            view_x: number_f64(props, "view_x", 0.0),
            view_y: number_f64(props, "view_y", 0.0),
            // Same rule as the setter: a positive number names the scale,
            // anything else leaves the plane at its default.
            view_zoom: props
                .get("view_zoom")
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite() && *n > 0.0)
                .map(super::super::scroll::clamp_zoom),
        }
    }

    /// Applies one `/gui_set` key. `true` if the key is a scroll-view prop.
    pub(super) fn apply(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "axis" => v
                .as_str()
                .and_then(Axis::from_str)
                .map(|a| self.axis = a)
                .is_some(),
            "zoom" => truthy(v).map(|b| self.zoom_enabled = b).is_some(),
            "content_w" => {
                self.content_w = v.as_f64().map(|n| n as f32);
                true
            }
            "content_h" => {
                self.content_h = v.as_f64().map(|n| n as f32);
                true
            }
            "view_x" => set_f64(&mut self.view_x, v),
            "view_y" => set_f64(&mut self.view_y, v),
            // A positive number names the scale; **anything else clears it** —
            // `0`, an empty string, a null — and the plane goes back to its
            // default (the window's density). The wire has no other way to ask
            // for a default it cannot name, the same shape `theme` uses for
            // dropping an overlay.
            "view_zoom" => {
                self.view_zoom = v
                    .as_f64()
                    .filter(|n| n.is_finite() && *n > 0.0)
                    .map(super::super::scroll::clamp_zoom);
                true
            }
            _ => false,
        }
    }
}

/// The generic layout props **any** widget may carry, applied by the layout
/// engine: a fixed main-axis size in a `row`/`col` (`w`/`h`, device pixels), a
/// `weight` for the shared remainder (absent = 1), and a `free`-layout
/// position (`x`/`y`, with `w`/`h` as the size). All optional; a widget with
/// none of them lays out exactly as before (an even share, or the full free
/// overlay).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Place {
    pub w: Option<f32>,
    pub h: Option<f32>,
    pub weight: Option<f32>,
    pub x: Option<f32>,
    pub y: Option<f32>,
}

impl Place {
    pub(super) fn parse(props: &serde_json::Map<String, Value>) -> Place {
        let f = |k: &str| props.get(k).and_then(Value::as_f64).map(|v| v as f32);
        Place {
            w: f("w"),
            h: f("h"),
            weight: f("weight"),
            x: f("x"),
            y: f("y"),
        }
    }

    /// Applies one `/gui_set` key. `true` if the key is a place prop.
    pub fn apply(&mut self, key: &str, v: &Value) -> bool {
        let slot = match key {
            "w" => &mut self.w,
            "h" => &mut self.h,
            "weight" => &mut self.weight,
            "x" => &mut self.x,
            "y" => &mut self.y,
            _ => return false,
        };
        match v.as_f64() {
            Some(n) => *slot = Some(n as f32),
            None => *slot = None, // a non-number (e.g. "auto") releases the prop
        }
        true
    }
}
/// The rate a data view reads its bus at. A bus is a bus — the rate says how
/// its values are obtained, not what kind of thing it is: audio-rate buses are
/// recorded into the segment by the server on demand, control-rate buses live
/// in the segment permanently.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Rate {
    /// The default for every live data view: the meters, the oscilloscope, the
    /// goniometer and the spectroscope all watch audio unless told otherwise.
    #[default]
    Audio,
    Control,
}

impl Rate {
    /// Parses the wire's `rate` prop. Absent or unrecognized reads as the
    /// default, audio rate — so a typo shows the common case rather than
    /// nothing.
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("control") | Some("kr") => Rate::Control,
            _ => Rate::Audio,
        }
    }

    pub fn is_audio(self) -> bool {
        self == Rate::Audio
    }
}

/// The shared payload of the continuous controls (`slider`/`knob`/`number`): a
/// value clamped to a range, with an optional label — plus the two rules that
/// say *how* the handle's travel becomes that value, the **curve** it is read
/// along and the **step** it lands on.
#[derive(Debug, Clone)]
pub struct Range {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    /// The bend of the axis between `min` and `max`: `0` is linear, negative
    /// spends most of the range on the first half of the travel, positive on
    /// the last half — which is the fine-at-the-bottom feel a frequency or an
    /// amplitude control wants. It is
    /// [`clausters_core::warp`]'s curve — the same one an envelope segment and
    /// a client's `lincurve` run — so a control does not feel one way here and
    /// another where the value was computed.
    pub curve: f32,
    /// The grid a **drag** lands on, in the value's own units: `0` is
    /// continuous, `1` over `0..127` is the integers `\midinote` wants, and a
    /// Faust parameter arrives with the one its `hslider` declared. A value the
    /// script *sends* is drawn as sent — the step is a rule about the hand, not
    /// a constraint on the document.
    pub step: f32,
    pub label: Option<String>,
    /// The glyph scale the control's label and value read-out draw at.
    pub text_size: f32,
}

impl Range {
    pub(crate) fn parse(props: &serde_json::Map<String, Value>) -> Range {
        let min = number(props, "min", 0.0);
        let max = number(props, "max", 1.0);
        let value = number(props, "value", min).clamp(min.min(max), min.max(max));
        Range {
            value,
            min,
            max,
            curve: number(props, "curve", 0.0),
            step: number(props, "step", 0.0),
            label: label(props),
            text_size: text_size(props),
        }
    }

    /// The value as a 0..1 fraction of the range (for rendering): where the
    /// handle sits along the bend the drag reads.
    ///
    /// The **exact inverse** of [`set_fraction`](Self::set_fraction), curve and
    /// all, so the handle is drawn where a drag would have to leave it — which
    /// a reversed range (`min > max`, a legitimate control) did not use to get:
    /// this read the value off a normalized axis while the write read it off
    /// the declared ends, and the handle came out mirrored.
    pub fn fraction(&self) -> f32 {
        warp::curve_unit(self.value, self.min, self.max, self.curve).clamp(0.0, 1.0)
    }

    /// Sets the value from a 0..1 fraction of the range (for interaction):
    /// along the curve, then onto the step's grid.
    pub fn set_fraction(&mut self, t: f32) {
        let v = warp::curve_value(t.clamp(0.0, 1.0), self.min, self.max, self.curve);
        self.value = self.snap(v);
    }

    /// `v` on the step's grid, counted from `min` and never past `max` — the
    /// clamp is a step count rather than a clamp on the value, so a grid that
    /// does not divide the range (`0..10` by `3`) ends on `9` instead of an
    /// off-grid `10`. Unstepped, `v` unchanged.
    fn snap(&self, v: f32) -> f32 {
        let step = self.step.abs().copysign(self.max - self.min);
        if step == 0.0 || !step.is_finite() {
            return v;
        }
        let last = ((self.max - self.min) / step).floor().max(0.0);
        let n = ((v - self.min) / step).round().clamp(0.0, last);
        self.min + n * step
    }
}

/// **The window a placement shows of the data behind it**: where in the source
/// its own time zero reads, and what happens where the window runs off the
/// samples.
///
/// This is what makes a clip *a view of the data* rather than a rectangle the
/// samples are stretched into. A clip is a window onto a segment of a
/// buffer — the memory-view idea, and the reason trimming one hides samples
/// instead of squeezing them: shortening the window leaves the samples
/// exactly as it is and shows less of it, and lengthening it again brings the
/// hidden samples back. Splitting a clip in two is the same statement twice,
/// with two windows over one source.
///
/// One timeline sample is one source frame, deliberately: making it anything
/// else is a **time stretch**, which resamples or re-synthesizes the samples
/// and is a rendering rather than a placement. [`fit`](Self::fit) is where that
/// will land when it exists, and until then it is what a picture deliberately
/// scaled into its rectangle asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceWindow {
    /// The source frame this placement's time zero reads (`start`). Negative
    /// values are as meaningful as positive ones on a **looping** window: they
    /// are the tail of the iteration before this one.
    pub start: f64,
    /// Whether the window **wraps** around the samples: past the end it
    /// begins again, and before the beginning it shows the samples' own tail
    /// — which is what stretching an edge past the source means when a loop is
    /// what the placement is. Off, the window shows the samples where it has
    /// any and nothing where it has none.
    pub looping: bool,
    /// Whether the samples are **fitted** to the placement's span instead of
    /// read frame for sample — the picture a time stretch would produce,
    /// which nothing here produces yet. Off by default: an edge drag is a trim.
    pub fit: bool,
}

impl Default for SourceWindow {
    fn default() -> Self {
        Self {
            start: 0.0,
            looping: false,
            fit: false,
        }
    }
}

impl SourceWindow {
    /// The window a placement's props declare, or `None` when they declare
    /// none — which is what "the container's window, or the identity" means and
    /// the difference between a body that reads its own segment and one that is
    /// drawn through the clip's.
    pub(crate) fn declared(props: &serde_json::Map<String, Value>) -> Option<Self> {
        ["start", "loop", "fit"]
            .iter()
            .any(|k| props.contains_key(*k))
            .then(|| Self::parse(props))
    }

    /// The window a placement's props declare.
    pub(crate) fn parse(props: &serde_json::Map<String, Value>) -> Self {
        Self {
            start: number_f64(props, "start", 0.0),
            looping: props.get("loop").and_then(truthy).unwrap_or(false),
            fit: props.get("fit").and_then(truthy).unwrap_or(false),
        }
    }

    /// Applies one `/gui_set` key, returning whether it was one of these.
    pub(crate) fn apply(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "start" => v.as_f64().map(|x| self.start = x).is_some(),
            "loop" => truthy(v).map(|b| self.looping = b).is_some(),
            "fit" => truthy(v).map(|b| self.fit = b).is_some(),
            _ => false,
        }
    }

    /// The source frame a placement-local time `t` reads, over `total` frames
    /// of samples and a placement spanning `dur`.
    ///
    /// `None` where the window is off the samples — which only a window that
    /// neither loops nor fits can be, and which is the honest answer there:
    /// nothing was recorded at that time, so nothing is drawn and nothing is
    /// read.
    pub fn source_at(&self, t: f64, dur: f64, total: f64) -> Option<f64> {
        if total <= 0.0 {
            return None;
        }
        if self.fit {
            return (dur > 0.0).then(|| (t / dur * total).clamp(0.0, total));
        }
        let s = self.start + t;
        if self.looping {
            return Some(s.rem_euclid(total));
        }
        (s >= 0.0 && s <= total).then_some(s)
    }

    /// The placement-local time a source frame is drawn at — the inverse of
    /// [`source_at`](Self::source_at) **within one pass over the samples**,
    /// which is what a looping window is drawn as (see [`runs`](Self::runs)).
    pub fn time_at(&self, source: f64, dur: f64, total: f64) -> f64 {
        match self.fit {
            true if total > 0.0 => source / total * dur,
            true => 0.0,
            false => source - self.start,
        }
    }

    /// The **runs** a placement's `[from, to]` local span breaks into, each one
    /// a stretch of time over which the window stays inside the samples: the
    /// local time it begins at, the local time it ends at, and the source frame
    /// its beginning reads.
    ///
    /// A window that fits, or one that stays inside the samples, is one run.
    /// A **looping** one is a run per iteration, which is what lets the same
    /// affine drawing be used for all of them; a window running off samples it
    /// does not loop contributes only the part that is on it, so the picture
    /// stops where the samples end instead of clamping into a flat line
    /// nothing recorded.
    pub fn runs(&self, from: f64, to: f64, dur: f64, total: f64) -> Vec<(f64, f64, f64)> {
        if total <= 0.0 || to <= from {
            return Vec::new();
        }
        if self.fit {
            return vec![(from, to, self.source_at(from, dur, total).unwrap_or(0.0))];
        }
        if !self.looping {
            // The part of the window that is on the samples, and nothing else.
            let lo = from.max(-self.start);
            let hi = to.min(total - self.start);
            return if hi > lo {
                vec![(lo, hi, self.start + lo)]
            } else {
                Vec::new()
            };
        }
        let mut runs = Vec::new();
        let mut t = from;
        // A window whose span is many iterations long is drawn as many; the
        // count is bounded by the pixels the caller will draw it into, since a
        // run thinner than a pixel is still one run.
        while t < to && runs.len() < MAX_LOOP_RUNS {
            let source = (self.start + t).rem_euclid(total);
            let end = (t + (total - source)).min(to);
            runs.push((t, end, source));
            t = end;
        }
        runs
    }
}

/// The most iterations a looping window is drawn as. A placement stretched over
/// thousands of loops of a short buffer is a picture of a texture, not of the
/// samples: past this it is drawn as far as it goes and the rest is left
/// blank, which is visible and cheap, rather than pretending to draw a million
/// runs of two pixels each.
const MAX_LOOP_RUNS: usize = 512;

#[cfg(test)]
mod window_tests {
    use super::*;

    /// **A clip is a window onto a segment of data.** Trimming it shows less of
    /// the samples and moves nothing; the frames it hides are still there and
    /// come back when the window is opened again — which is the property split
    /// and join are built on.
    #[test]
    fn a_window_reads_the_samples_frame_for_sample() {
        let w = SourceWindow {
            start: 200.0,
            ..SourceWindow::default()
        };
        assert_eq!(w.source_at(0.0, 300.0, 1000.0), Some(200.0));
        assert_eq!(w.source_at(300.0, 300.0, 1000.0), Some(500.0));
        // Off the samples: nothing was recorded there, so there is no frame to
        // name — not the last one over and over.
        assert_eq!(w.source_at(900.0, 300.0, 1000.0), None);
        // A **fitted** window is the other statement: the samples scaled into
        // the span, which is the picture a time stretch would make.
        let fitted = SourceWindow {
            fit: true,
            ..SourceWindow::default()
        };
        assert_eq!(fitted.source_at(150.0, 300.0, 1000.0), Some(500.0));
    }

    /// A **looping** window wraps both ways: past the end is the beginning
    /// again, and before frame zero is the samples' own tail — the samples of
    /// the iteration before this one.
    #[test]
    fn a_looping_window_wraps_at_both_ends() {
        let w = SourceWindow {
            start: -100.0,
            looping: true,
            fit: false,
        };
        assert_eq!(w.source_at(0.0, 400.0, 1000.0), Some(900.0));
        assert_eq!(w.source_at(100.0, 400.0, 1000.0), Some(0.0));
        assert_eq!(w.source_at(1200.0, 400.0, 1000.0), Some(100.0));
    }

    /// The **runs** are what makes one affine renderer draw all of it: a run per
    /// pass over the samples, the part that is on it when there is no loop, and
    /// nothing at all where there is no samples under the window.
    #[test]
    fn the_runs_break_a_window_where_the_samples_do() {
        let plain = SourceWindow::default();
        assert_eq!(
            plain.runs(0.0, 500.0, 500.0, 1000.0),
            vec![(0.0, 500.0, 0.0)]
        );
        // Past the end without a loop: only the part that is on the samples.
        let late = SourceWindow {
            start: 800.0,
            ..plain
        };
        assert_eq!(
            late.runs(0.0, 500.0, 500.0, 1000.0),
            vec![(0.0, 200.0, 800.0)]
        );
        // Entirely past it: no runs, so nothing is drawn.
        let past = SourceWindow {
            start: 2000.0,
            ..plain
        };
        assert!(past.runs(0.0, 500.0, 500.0, 1000.0).is_empty());
        // Looping: one run per iteration, each starting where the samples begin.
        let looped = SourceWindow {
            start: 900.0,
            looping: true,
            fit: false,
        };
        assert_eq!(
            looped.runs(0.0, 2100.0, 2100.0, 1000.0),
            vec![
                (0.0, 100.0, 900.0),
                (100.0, 1100.0, 0.0),
                (1100.0, 2100.0, 0.0)
            ]
        );
    }
}
