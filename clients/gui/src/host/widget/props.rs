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
/// [`SignalElement::freq_window`](crate::host::signal::SignalElement::freq_window)
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

    /// The horizontal view window of an element that owns its x axis, read the
    /// same way [`Self::y_view`] reads the vertical one — validated here rather
    /// than in `apply`, for the same reason: one `/gui_set` carrying both keys
    /// must not depend on their order.
    pub fn x_view(&self) -> (f64, f64) {
        normalized_window(self.x_start, self.x_len)
    }

    pub(super) fn apply(&mut self, key: &str, v: &Value) -> bool {
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
    /// Sweep a selection: the shared time selection on a timeline (restricted
    /// in pitch where the axis has a vertical one), the marquee on a canvas.
    Select,
    /// Put the transport's cursor under the pointer (a timeline locate).
    Locate,
}

impl GestureStep {
    pub(super) fn from_str(s: &str) -> Option<GestureStep> {
        Some(match s {
            "element" => GestureStep::Element,
            "pan" => GestureStep::Pan,
            "select" => GestureStep::Select,
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

    /// The table a container of this kind carries unless the wire replaces it.
    ///
    /// The timeline views differ only in what a plain drag is for: a waveform
    /// has nothing placed on its axis, so it selects; a lane and a roll hand
    /// the press to the clip or note first; a free-standing ruler is a scrub
    /// strip. Shift pans on all of them — that is the convention the whole
    /// track shares — and a workspace pans with whatever is left over.
    pub fn of_kind(kind: &WidgetKind) -> GestureMap {
        use GestureStep::*;
        let (plain, shift, ctrl, alt): (&[_], &[_], &[_], &[_]) = match kind {
            WidgetKind::Track { .. } => (
                &[Element, Locate],
                &[Pan],
                &[Element, Locate],
                &[Element, Locate],
            ),
            WidgetKind::PianoRoll { .. } => {
                (&[Element, Select], &[Pan], &[Element, Select], &[Element])
            }
            WidgetKind::TimeRuler { .. } => (&[Locate], &[Pan], &[Locate], &[Locate]),
            // A navigable signal element: a plain drag selects, Shift pans.
            WidgetKind::Signal(el) if el.caps.navigable => {
                (&[Select], &[Pan], &[Select], &[Select])
            }
            // The patcher: a plain drag on the empty canvas sweeps the box
            // marquee, Shift leaves the press to the workspace under it.
            WidgetKind::Patch { .. } => (
                &[Element, Select],
                &[Element],
                &[Element, Select],
                &[Element, Select],
            ),
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
        GestureMap {
            plain: GesturePlan::of(plain),
            shift: GesturePlan::of(shift),
            ctrl: GesturePlan::of(ctrl),
            alt: GesturePlan::of(alt),
        }
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
/// value clamped to a range, with an optional label.
#[derive(Debug, Clone)]
pub struct Range {
    pub value: f32,
    pub min: f32,
    pub max: f32,
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
            label: label(props),
            text_size: text_size(props),
        }
    }

    /// This control's value axis — the range its handle travels.
    pub(crate) fn axis(&self) -> crate::viewport::Axis {
        crate::viewport::Axis::ranged(
            self.min as f64,
            self.max as f64,
            crate::viewport::Unit::Norm,
        )
    }

    /// The value as a 0..1 fraction of the range (for rendering).
    pub fn fraction(&self) -> f32 {
        self.axis().fraction_clamped(self.value as f64) as f32
    }

    /// Sets the value from a 0..1 fraction of the range (for interaction).
    pub fn set_fraction(&mut self, t: f32) {
        // Not `value_at_clamped`: a reversed range (`min > max`) is a legitimate
        // control, and the axis normalizes its bounds, so the value is read off
        // the declared ends rather than the sorted ones.
        self.value = self.min + t.clamp(0.0, 1.0) * (self.max - self.min);
    }
}
