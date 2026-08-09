//! The typed widget schema: a renderer's interpretation of a GuiDef tree.
//!
//! `host::guidef::GuiNode` is the **generic** wire form (any `{id, type, props,
//! children}`), kept deliberately open so the protocol never changes when a
//! widget type is added. This module is the other half of that principle: the
//! *renderer* turns a `GuiNode` into a **typed** [`Widget`] it knows how to lay
//! out and draw. Adding a widget type is a new [`WidgetKind`] variant plus a
//! handler here and in the renderer — not a protocol change. An unrecognized
//! type is not an error: it becomes [`WidgetKind::Unknown`], laid out (it
//! reserves its space) but not painted, so a host built today renders the parts
//! of a newer GuiDef it understands and ignores the rest.
//!
//! The standardized widgets at this milestone are `window` + `panel`/layout
//! (`row`/`col`/`grid`/`free`) + `label`, plus the heavy `waveform` view, fed
//! its samples either inline (`"data": [f32…]`) or — for bulk — from an OSC blob
//! carried alongside the JSON in the same `/gui_def` message (`"blob": <index>`).
//! Both keep the int/float distinction and the "flat primitives at the boundary"
//! rule; a server buffer reference (`"buffer"`) is recognized but deferred to the
//! milestone where the host attaches to the audio server.
//!
//! **Module layout.** This file is the *schema*: the [`WidgetKind`] enum (the
//! closed sum type the whole renderer matches on), the shared prop bundles
//! (`Layout`/`Flow`/`EditorProps`/`Range`/…) and the private prop-reading
//! helpers. The two long *wire* passes live in child modules so the schema reads
//! on its own: [`build`] turns a `GuiNode` into a `WidgetKind` (construction),
//! and [`apply`] applies a `/gui_set` key to a live one (mutation). Each is one
//! arm per widget type; both are descendants, so they share the helpers without
//! exposing them. [`size`] is the third such pass, in the other direction: how
//! big a kind wants to be ([`WidgetKind::natural_size`]), which the layout
//! resolves against the tree's explicit sizes. Per-widget *behavior* (drawing, hit-testing, editing) is not
//! here at all — it lives in each widget's own module (`bpf`, `pianoroll`,
//! `track`, `patch`, `textedit`, …); this module owns only the typed data and
//! its wire mapping.

use std::path::PathBuf;
use std::sync::Arc;

use clausters_core::osc::OscType;
use serde_json::Value;

use crate::spectrogram::FreqScale;

use super::canvas;
use super::guidef::GuiNode;
// Sibling widget modules the wire matches reach via `super::` — re-imported here
// so the `build`/`apply` child modules resolve the same paths (a descendant sees
// the parent's private `use` items).
use super::signal::{Presentation, SignalElement};
use super::{bpf, piano, plot, score, signal, textedit};

mod apply;
mod axes;
mod build;
mod parse;
mod size;

pub(super) use axes::{AXES, flatten as flatten_axes, flatten_tree as flatten_tree_axes};

use parse::*;

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
    fn parse(props: &serde_json::Map<String, Value>) -> Layout {
        flow(props)
            .and_then(Layout::from_str)
            .unwrap_or(Layout::Col)
    }

    fn from_str(s: &str) -> Option<Layout> {
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
    fn parse(props: &serde_json::Map<String, Value>) -> Align {
        props
            .get("align")
            .and_then(Value::as_str)
            .and_then(Align::from_str)
            .unwrap_or(Align::Start)
    }

    fn from_str(s: &str) -> Option<Align> {
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
    fn parse(props: &serde_json::Map<String, Value>) -> Ruler {
        Self::parse_with(props, Ruler::Time)
    }

    /// The `ruler` prop over a presentation's own default — absent keeps the
    /// default, and a **boolean** switches the strip off or back on, which is
    /// how the live views have always spelled it (their x unit is not
    /// selectable, so only on/off was ever meaningful there).
    pub(super) fn parse_with(props: &serde_json::Map<String, Value>, default: Ruler) -> Ruler {
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

    fn set(&mut self, v: &Value) -> bool {
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
    fn parse(props: &serde_json::Map<String, Value>, default: RulerY) -> RulerY {
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

    fn from_str(s: &str) -> Option<RulerY> {
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

    fn set(&mut self, v: &Value) -> bool {
        match v.as_str().and_then(Self::from_str) {
            Some(u) => {
                *self = u;
                true
            }
            None => false,
        }
    }
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
    pub link: Option<i32>,
    pub offset: f64,
}

impl EditorProps {
    /// Parses the shared chrome; `default_y` is the view's own default
    /// vertical unit (`Norm` for the waveform, `Hz` for the spectrogram).
    pub(super) fn parse(props: &serde_json::Map<String, Value>, default_y: RulerY) -> EditorProps {
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
    pub(super) fn body() -> EditorProps {
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
    fn parse_lane(props: &serde_json::Map<String, Value>) -> EditorProps {
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
        let mut axis = crate::viewport::Axis::normalized(crate::viewport::Unit::Norm);
        axis.set_span(self.y_start, self.y_len);
        axis.span()
    }

    fn apply(&mut self, key: &str, v: &Value) -> bool {
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
    fn parse(props: &serde_json::Map<String, Value>) -> Flow {
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
    /// (a fraction of the visible size, for [`super::scroll::clamp_pan`]). The
    /// free plane is unbounded and gets slack; a constrained scroll view is a
    /// bounded document and gets none.
    pub fn slack(self) -> f64 {
        match self {
            Axis::Both => super::scroll::SLACK,
            Axis::X | Axis::Y => 0.0,
        }
    }

    fn parse(props: &serde_json::Map<String, Value>) -> Axis {
        props
            .get("axis")
            .and_then(Value::as_str)
            .and_then(Axis::from_str)
            .unwrap_or(Axis::Both)
    }

    fn from_str(s: &str) -> Option<Axis> {
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
    fn from_str(s: &str) -> Option<GestureStep> {
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
    fn of(steps: &[GestureStep]) -> GesturePlan {
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
    fn parse(s: &str) -> Option<GesturePlan> {
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
    fn overlay(&mut self, v: &Value) -> bool {
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
    pub fn zoom(&self, m: &super::metrics::Metrics) -> f64 {
        super::scroll::clamp_zoom(self.view_zoom.unwrap_or(m.ui_scale as f64))
    }

    fn parse(props: &serde_json::Map<String, Value>) -> ScrollView {
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
                .map(super::scroll::clamp_zoom),
        }
    }

    /// Applies one `/gui_set` key. `true` if the key is a scroll-view prop.
    fn apply(&mut self, key: &str, v: &Value) -> bool {
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
                    .map(super::scroll::clamp_zoom);
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
    fn parse(props: &serde_json::Map<String, Value>) -> Place {
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

/// The typed kind of a widget, with the fields the renderer needs.
#[derive(Debug, Clone)]
pub enum WidgetKind {
    /// A top-level window (a GuiDef root): title, requested size, child layout.
    Window {
        title: Option<String>,
        width: u32,
        height: u32,
        layout: Layout,
        flow: Flow,
    },
    /// A nestable container.
    Panel { layout: Layout, flow: Flow },
    /// A container showing **one child at a time**: the one at `index`, filling
    /// the container's area (its `flow`'s margin inset). The others are hidden
    /// — skipped by the layout, so they are neither drawn nor hit — but they
    /// stay in the tree, which is what makes a switch cheap: a hidden heavy
    /// element keeps its GPU slot and its bus watch, since both are collected
    /// from the tree and not from the placements, so flipping back re-uploads
    /// nothing.
    ///
    /// An `index` outside the children shows nothing, deliberately: it is a
    /// blank page, not a clamped one, so a pager cannot silently show the wrong
    /// child. With `index` bound to a `toggle` or a `menu`
    /// ([`bind`](super::bind)) this is the whole of tabs, a pager, and
    /// alternating two views of one signal — composition, not a widget.
    ///
    /// It carries a `margin` rather than a whole [`Flow`]: a stack makes no
    /// arrangement, so the `gap` between children and a `grid`'s column count
    /// have nothing to mean here.
    Stack { index: i32, margin: Option<f32> },
    /// The 2D workspace: a container whose children live in a **virtual
    /// content area** seen through a scrolling, zooming window ([`ScrollView`]).
    /// General first — the default pans both axes and zooms at the cursor; the
    /// constrained scroll views (`axis`, `zoom: 0`) degrade from it by
    /// configuration. `layout` arranges the children *inside* the content
    /// area (default `free`), exactly as a panel does inside its rect.
    Scroll {
        layout: Layout,
        flow: Flow,
        view: ScrollView,
    },
    /// Static text. `wrap` word-wraps it on the font's fixed advance (off, a
    /// single line clipped with an ellipsis); `align` places each line in the
    /// rect (`start`, the default left edge / `center` / `end`).
    Label {
        text: String,
        text_size: f32,
        wrap: bool,
        align: Align,
    },
    /// The **signal element**: every view of a signal, in one widget.
    ///
    /// A presentation (the trace, a magnitude spectrum, the time-frequency
    /// texture, the phase of a stereo pair) of a source (addressable samples,
    /// or a bus read forward-only), with the capabilities the view offers over
    /// it — see [`super::signal`], which is where the model and the wire-name
    /// presets live. The six names the catalog grew (`waveform`, `plot`,
    /// `scope`, `spectrum`, `spectrogram`, `phasescope`) are six points of that
    /// product, so this one arm answers for all of them.
    Signal(Box<SignalElement>),
    /// A level meter reading bus `bus` from the shared-memory segment each
    /// frame (zero messages), shown as a bar over `[min, max]`. At `rate`
    /// audio (the default) it reads the bus's published block level, the
    /// console meter over a hardware output or any mix bus; at control rate it
    /// reads the control bus's current value.
    Meter {
        bus: i32,
        rate: Rate,
        min: f32,
        max: f32,
        label: Option<String>,
    },
    /// A live text view of the audio server's node tree rooted at `group`,
    /// queried over the client leg (`/group_queryTree`) and refreshed on node
    /// lifecycle notifications and a low-rate poll. `controls` shows each
    /// synth's control name/value pairs. A read-only client-of-the-server view.
    NodeTree {
        group: i32,
        controls: bool,
        label: Option<String>,
    },
    /// A script-supplied WGSL shader run over the widget area. `shader` is the
    /// user's `shade` source; `params` are four floats fed to the shader, each
    /// set from the script (`/gui_set param0…`) and/or overwritten every frame by
    /// the control bus named in `buses` (a `-1` slot is script-only), read from
    /// shared memory like a meter — so the shader animates from OSC parameters
    /// and from live server audio at once.
    Canvas {
        shader: String,
        params: [f32; canvas::PARAM_COUNT],
        buses: [i32; canvas::PARAM_COUNT],
        label: Option<String>,
    },
    /// A drawable break-point function (envelope editor): breakpoints
    /// `(time, value)` plus a per-segment shape/curve **using the server's own
    /// envelope shape numbers** (evaluated through the shared core, so what it
    /// draws is what an `EnvGen` plays). Values live in `[min, max]` — any
    /// automation range: unipolar, bipolar, an on/off lane via the hold shape —
    /// with an optional exponential display scale (`exp`) for frequency-like
    /// params; times span `[0, duration]` (0 = fit the last point). Edits flow
    /// back as a `"points"` event (or the bound forward) carrying the flat
    /// `t v shape curve …` list — see [`super::bpf`].
    Bpf {
        points: Vec<super::bpf::BpfPoint>,
        min: f32,
        max: f32,
        duration: f64,
        exp: bool,
        label: Option<String>,
    },
    /// An engraved music-notation page. The rendering client (verovio, in the
    /// Python `clausters.gui` submodule) engraves a score and sends a semantic
    /// display list — a glyph-outline table keyed by SMuFL codepoint plus placed
    /// primitives in verovio page units (see [`super::score::ScoreData`]). The
    /// host fits the page into the widget rect and tessellates every primitive
    /// into the shared triangle mesh (glyph outlines and engraving fills through
    /// lyon; staff lines/stems/ledger lines as thick-line quads), so notation
    /// draws through the same one-upload/one-draw pipeline as the rest of the
    /// chrome, natively and in the browser. The playback cursor follows the
    /// display list's own timemap, either located statically (`playhead`, in
    /// ms) or sweeping off the engine sample clock (`playhead_at`), exactly as
    /// the timeline views do. Read-only for now: the MEI xml:id travels on each
    /// primitive for a later interactive/edit-back pass.
    Score(super::score::ScoreData),
    /// A continuous slider over `[min, max]`. `vertical` lays it out along the
    /// y axis (min at the bottom, max at the top) instead of the x axis.
    Slider { range: Range, vertical: bool },
    /// A rotary control over `[min, max]`.
    Knob(Range),
    /// A draggable numeric read-out over `[min, max]`.
    Number(Range),
    /// A momentary push button.
    Button {
        label: Option<String>,
        text_size: f32,
    },
    /// A boolean on/off control.
    Toggle {
        value: bool,
        label: Option<String>,
        text_size: f32,
    },
    /// An editable text-entry field. `value` is the string (the event value it
    /// emits on every edit, exactly as a numeric control emits on every drag —
    /// never gated on a key); `multiline` allows embedded newlines (Enter
    /// inserts one) and a growing field. `caret` is **native view state** — the
    /// insertion point and selection while the field is focused, never parsed
    /// from or sent over the wire (the `PianoRoll::selected` precedent).
    Text {
        value: String,
        label: Option<String>,
        text_size: f32,
        multiline: bool,
        caret: super::textedit::Caret,
    },
    /// A drop/cycle selector over `options`, holding the chosen index.
    Menu {
        index: usize,
        options: Vec<String>,
        label: Option<String>,
        text_size: f32,
    },
    /// A multitrack lane: a horizontal strip of the shared timeline holding
    /// `clip` children placed by their `offset`/`dur`. A container (its clips
    /// are its children); `label` names the track in a left header, `height`
    /// its lane weight when several tracks stack under one time axis. The
    /// **graphic unit** — the clip rectangles and the track header — is drawn
    /// by [`super::track`]; the clips share one time axis (aligned tracks), the
    /// span being the longest clip end over the window's tracks. `snap` is the
    /// drag grid in timeline samples (0 = snap to whole samples) a clip's
    /// move/resize rounds to. `editor` is the shared chrome, of which a lane
    /// uses the time `ruler` (a strip under the lane, off by default) and the
    /// `playhead_at` anchor (the engine sample-clock value at timeline sample 0,
    /// so the playhead sweeps the clips as the composition plays) — the same
    /// props, parsing and `/gui_set` keys the heavy timeline views use. A lane
    /// joins no navigation group (its axis is the window's shared clip span), so
    /// those keys apply to the widget itself.
    Track {
        label: Option<String>,
        height: f32,
        snap: f64,
        /// The lane's gutter: how wide it is and what it carries there (see
        /// [`super::track::Header`]).
        header: super::track::Header,
        editor: EditorProps,
    },
    /// A **free-standing time ruler**: the shared axis of a navigation group,
    /// drawn as a strip the *document* places — the DAW's ruler above its
    /// tracks.
    ///
    /// It exists because a ruler over a multitrack belongs to the **axis**, not
    /// to any one lane. A `track`'s own `ruler` strip is reserved out of that
    /// lane's height, so ruling a stack of lanes meant picking one to carry it
    /// (and to pay for it), and the strip then sat wherever that lane happened
    /// to be — between two lanes, unless it was the last. This widget owns its
    /// own box instead: put it above the lanes (or below) and no lane loses a
    /// pixel.
    ///
    /// It is a timeline widget like any other (`is_timeline`): it joins the
    /// group named by `editor.link` and reads that group's window, so it labels
    /// exactly what the lanes show and moves with them. A press locates the
    /// transport, as a lane's own ruler strip does. Its thickness is the `h`
    /// place prop, like any other widget's — the builders default it.
    TimeRuler { editor: EditorProps },
    /// The dedicated editor-grade piano-roll view: a keyboard gutter, a note
    /// grid, and optional velocity / OSC-event strips — the editor sibling of
    /// the compact `clip` roll, sharing its drawing/hit-test primitives
    /// ([`super::pianoroll`]). MIDI `notes` (`start`/`dur` in timeline samples,
    /// `pitch` a MIDI note over `[min, max]`, plus velocity/channel) draw in the
    /// grid; `osc` events draw as flags in their lane. A timeline widget
    /// (`is_timeline`): it joins a navigation group and carries the ruler /
    /// selection / playhead chrome in `editor`, so it zooms/pans/plays in lockstep
    /// with sibling views. Editing (drag a note, resize an edge, Ctrl+click
    /// add/remove) flows back per the edit-back pattern.
    PianoRoll {
        notes: Vec<super::track::Note>,
        osc: Vec<super::pianoroll::OscMark>,
        /// The multi-note selection (note indices) — native view state, never
        /// parsed from the wire: the marquee/Alt+click gestures build it, block
        /// edits (move, delete, velocity) consume it, and it clears when the
        /// script replaces `notes` (the indices would dangle).
        selected: Vec<usize>,
        min: f32,
        max: f32,
        snap: f64,
        velocity_lane: bool,
        osc_lane: bool,
        /// Live MIDI input: when on, the native host opens its virtual MIDI
        /// input port and **paints** incoming notes into this roll — at the
        /// running playhead, or step-entry on the snap grid when stopped.
        midi_in: bool,
        label: Option<String>,
        editor: EditorProps,
    },
    /// The playable virtual piano keyboard, laid out with real piano
    /// proportions ([`super::piano`]) so it resizes freely. `min`/`max` are
    /// the visible MIDI range (min snapped down to a white key); the
    /// `overview` strip — a miniature of the full `0..=127` range with the
    /// visible window marked — is the keyboard's zoom/pan "ruler", and `pan`
    /// gates all range navigation (drag/wheel) when off. Keys outside
    /// `active_min..=active_max` draw grayed and are inert — the visual of
    /// the mapped range. Pressing a key emits the MIDI-shaped
    /// `"note" pitch velocity state channel` event (state 1 on press, 0 on
    /// release; dragging across keys glissandos), with `velocity` fixed by
    /// prop or mapped from the press height (front of the key = louder).
    /// With `voice` set, the **host** additionally manages one server voice
    /// per held key: `/synth_new <voice> … freq amp gate 1 <voice_args…>` on
    /// press, `gate 0` on release (the def frees itself).
    Piano {
        min: i32,
        max: i32,
        active_min: i32,
        active_max: i32,
        pan: bool,
        overview: bool,
        /// A fixed press velocity; `None` maps velocity from the press height.
        velocity: Option<i32>,
        /// The MIDI channel carried in the `"note"` event (0..15).
        channel: i32,
        /// Host-voice mode: the server def one voice per held key plays.
        voice: Option<String>,
        /// Extra `/synth_new` control pairs appended after `freq`/`amp`/`gate`.
        voice_args: Vec<(String, f32)>,
        /// The held keys — native view state, never parsed from the wire: the
        /// press/glissando/release gestures build it, the drawing reads it.
        pressed: Vec<i32>,
        label: Option<String>,
    },
    /// One clip on a `track`: a placed rectangle spanning `[offset, offset +
    /// dur]` in timeline sample units (the graphic unit — length = duration),
    /// with a `label`. Interaction (drag to move `offset`, drag an edge to
    /// resize `dur`) writes back through the edit-back path.
    ///
    /// **A clip is a container, and its bodies are its children.** A take is a
    /// [`Signal`] element, a roll of events a [`PianoRoll`], an automation
    /// curve a [`Bpf`] — the same elements that stand on their own elsewhere,
    /// composed here rather than reimplemented, and **layered** back to front
    /// rather than selected by precedence: an envelope drawn over the material
    /// it shapes is one clip, not two. Each keeps its own value axis, because a
    /// roll's `min`/`max` are pitches and a curve's are its parameter's.
    ///
    /// They are built from the clip's own props (`data`/`blob`/`path`/`cache`/
    /// `buffer`, `notes`, `points`) because the wire still describes a clip as
    /// a thing with bodies; moving the wire onto the containment is a separate
    /// step. So they carry **no id**: a script addresses the clip, and a
    /// `/gui_set` of a body prop routes into the child that owns it.
    ///
    /// [`Signal`]: WidgetKind::Signal
    /// [`PianoRoll`]: WidgetKind::PianoRoll
    /// [`Bpf`]: WidgetKind::Bpf
    Clip {
        offset: f64,
        dur: f64,
        label: Option<String>,
    },
    /// A **directed, typed** patcher (a GraphDef at level 1, a SynthDef/FaustDef
    /// at level 2): boxes with inlets on their top edge and outlets on their
    /// bottom, and a cord per `outlet → inlet` connection, weighted by rate (audio
    /// heavy, control thin, init dashed). Dragging an outlet to an inlet (either
    /// grab order) draws a cord, refusing a rate mismatch; the edit leaves as a
    /// flat directed `"wire"` event. At level 1 the buses are not drawn — a cord
    /// *is* a bus (the client names them); at level 2 a cord is an internal wire.
    /// A leaf.
    Patch {
        patch: super::patch::PatchDraw,
        /// The multi-box selection (box indices) — native view state, never
        /// parsed from the wire: the click/marquee gestures build it, the move
        /// drag consumes it, and it clears when the script replaces `boxes`
        /// (the indices would dangle).
        selected: Vec<usize>,
        label: Option<String>,
    },
    /// A type this build does not render yet. Laid out so it reserves space, but
    /// not painted. Carries the type tag for logs.
    Unknown(String),
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
    fn parse(props: &serde_json::Map<String, Value>) -> Range {
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
    fn axis(&self) -> crate::viewport::Axis {
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

/// The default window size when a GuiDef omits `w`/`h`.
const DEFAULT_WINDOW: (u32, u32) = (640, 360);
/// The default peak-pyramid bucket for an inline signal element.
use super::signal::DEFAULT_BASE_BUCKET;

/// A typed widget node: its id (the root's comes from the `/gui_def` argument),
/// its kind, and its children (only containers have any).
#[derive(Debug, Clone)]
pub struct Widget {
    pub id: Option<i32>,
    pub kind: WidgetKind,
    /// The generic layout props (`w`/`h`/`weight`/`x`/`y`) this widget carries.
    pub place: Place,
    /// The `theme` prop: a partial role table (`role -> "#rrggbb[aa]"`, the
    /// same shape as the TOML style file) overlaying the parent's theme for
    /// this widget's whole subtree — a **theme group**.
    pub theme_over: Option<serde_json::Map<String, Value>>,
    /// The `color` prop: the single-color shorthand — an overlay of just the
    /// roles that carry this widget's function (see
    /// [`Theme::accent_seeded`](super::theme::Theme::accent_seeded)).
    pub color: Option<super::paint::Color>,
    /// The `gestures` prop: the container's own (modifier → plan) table, replacing
    /// the default its kind carries ([`GestureMap::of_kind`]). `None` on the
    /// overwhelming majority of widgets, which are not containers and whose
    /// press is the element's.
    pub gestures: Option<GestureMap>,
    /// The resolved theme this widget draws with, produced at mutation points
    /// by [`resolve_themes`] (an [`Arc`] clone per widget, so the per-frame
    /// path reads exactly one theme and pays nothing). `None` until the first
    /// resolve — the renderer falls back to the host theme.
    pub theme: Option<Arc<super::theme::Theme>>,
    pub children: Vec<Widget>,
}

/// Applies one `/gui_set` key/value to a widget: its kind's own keys, plus —
/// for a `clip` — the props of the bodies it holds as children. See
/// [`apply::apply_widget`].
pub fn apply_widget(widget: &mut Widget, key: &str, v: &Value) -> bool {
    apply::apply_widget(widget, key, v)
}

/// Resolves every widget's theme reference: walking from `base` (the host
/// theme), a `theme` prop overlays the inherited table for its subtree and a
/// `color` prop re-seeds the function roles for its one widget — both at this
/// **mutation point**, never per frame. Recursive and cheap by construction:
/// a widget with neither prop shares its parent's `Arc`.
pub fn resolve_themes(widget: &mut Widget, base: &Arc<super::theme::Theme>) {
    let group = match &widget.theme_over {
        Some(table) => {
            let mut t = (**base).clone();
            for warning in t.overlay_json(table) {
                tracing::warn!("widget {:?}: {warning}", widget.id);
            }
            Arc::new(t)
        }
        None => base.clone(),
    };
    widget.theme = Some(match widget.color {
        Some(c) => Arc::new(super::theme::Theme::accent_seeded(&group, c)),
        None => group.clone(),
    });
    for child in &mut widget.children {
        resolve_themes(child, &group);
    }
}

impl Widget {
    /// Interprets a generic [`GuiNode`] (and the blobs carried beside it in the
    /// `/gui_def` message) into a typed widget tree. `root_id` is the def id from
    /// the OSC argument, used for the root whose JSON carries no `id`.
    pub fn from_node(root_id: i32, node: &GuiNode, blobs: &[Vec<u8>]) -> Result<Widget, String> {
        let mut widget = Self::build(Some(root_id), node, blobs)?;
        Self::link_lanes(&mut widget, root_id);
        Ok(widget)
    }

    /// Links every un-linked `track` — and every un-linked free-standing
    /// `timeruler` — of a window into one navigation group keyed by the window
    /// root. The multitrack's promise is **one shared time axis** (aligned
    /// lanes), and a navigation group is exactly that — so the lanes of a
    /// window navigate as one by default, zooming and panning together, and
    /// only an explicit `link` splits them (or joins lanes across windows).
    ///
    /// The ruler is in for the same reason and not by analogy: a free-standing
    /// ruler exists to rule the lanes beside it, so one dropped into a window
    /// of lanes with nothing said is asking for *their* axis. Every other
    /// timeline view stays out — a `waveform` in a window of lanes is showing
    /// its own buffer, and joining it to the composition's axis would be a
    /// guess.
    fn link_lanes(widget: &mut Widget, root_id: i32) {
        if let WidgetKind::Track { editor, .. } | WidgetKind::TimeRuler { editor } =
            &mut widget.kind
            && editor.link.is_none()
        {
            editor.link = Some(root_id);
        }
        for child in &mut widget.children {
            Self::link_lanes(child, root_id);
        }
    }

    fn build(id: Option<i32>, node: &GuiNode, blobs: &[Vec<u8>]) -> Result<Widget, String> {
        let id = id.or(node.id);
        let props = &node.props;
        let kind = build::build_kind(
            id,
            node.kind.as_str(),
            props,
            !node.children.is_empty(),
            blobs,
        )?;
        // Only containers carry children into the typed tree; a leaf's children
        // (if any) are ignored. A `track` carries its clips.
        let children = match kind {
            WidgetKind::Window { .. }
            | WidgetKind::Panel { .. }
            | WidgetKind::Scroll { .. }
            | WidgetKind::Stack { .. }
            | WidgetKind::Track { .. } => node
                .children
                .iter()
                .map(|c| Self::build(None, c, blobs))
                .collect::<Result<Vec<_>, _>>()?,
            // A clip is a container too, but its children are not on the wire:
            // the wire still describes a clip as a thing with bodies, so the
            // bodies are built from its own props (see `build::clip_bodies`).
            // Anything nested under a `clip` node is ignored, as under a leaf.
            WidgetKind::Clip { .. } => build::clip_bodies(props, blobs)?,
            _ => Vec::new(),
        };
        let gestures = props.get("gestures").and_then(|v| {
            let mut map = GestureMap::of_kind(&kind);
            map.overlay(v).then_some(map)
        });
        Ok(Widget {
            id,
            kind,
            place: Place::parse(props),
            gestures,
            theme_over: props.get("theme").and_then(Value::as_object).cloned(),
            color: props
                .get("color")
                .and_then(Value::as_str)
                .and_then(super::theme::parse_hex),
            theme: None,
            children,
        })
    }

    /// Applies a `/gui_set` of the style props (`theme`, `color`) to this
    /// widget. A `theme` value rides as a JSON object or its string carrier
    /// (the scalar wire, like `points`); an empty string (or empty object)
    /// clears the group, an empty `color` clears the accent. Returns whether
    /// the key was a style key that applied — the caller re-resolves the
    /// window's themes.
    pub fn style_apply(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "color" => match v.as_str() {
                Some("") => {
                    self.color = None;
                    true
                }
                Some(hex) => match super::theme::parse_hex(hex) {
                    Some(c) => {
                        self.color = Some(c);
                        true
                    }
                    None => false,
                },
                None => false,
            },
            "theme" => {
                let value = match v {
                    Value::String(s) if s.is_empty() => Value::Object(Default::default()),
                    Value::String(s) => match serde_json::from_str::<Value>(s) {
                        Ok(parsed) => parsed,
                        Err(_) => return false,
                    },
                    other => other.clone(),
                };
                match value.as_object() {
                    Some(table) if table.is_empty() => {
                        self.theme_over = None;
                        true
                    }
                    Some(table) => {
                        self.theme_over = Some(table.clone());
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// This widget's gesture table: the `gestures` prop when it carries one,
    /// else the default its kind implies. The one door the press walk reads, so
    /// a container never has to know whether it was configured.
    pub fn gesture_map(&self) -> GestureMap {
        self.gestures
            .unwrap_or_else(|| GestureMap::of_kind(&self.kind))
    }

    /// Applies a `/gui_set gestures` to this container: the same overlay the
    /// prop takes at build time, on top of the kind's defaults — so a set names
    /// only the modifiers it changes and an empty table restores the defaults.
    /// Returns whether the value was usable.
    pub fn gestures_apply(&mut self, v: &Value) -> bool {
        let mut map = GestureMap::of_kind(&self.kind);
        if !map.overlay(v) {
            return false;
        }
        self.gestures = Some(map);
        true
    }

    /// The signal element this widget is, if it is one.
    pub fn signal(&self) -> Option<&SignalElement> {
        self.kind.signal()
    }

    /// Whether this is a navigable signal element — the view that zooms, pans,
    /// selects and shows a playhead over its own samples.
    pub fn is_nav_signal(&self) -> bool {
        self.signal().is_some_and(|el| el.caps.navigable)
    }

    /// Whether this widget navigates the window's shared time axis: a navigable
    /// signal element, or one of the containers placed on that axis.
    pub fn is_timeline(&self) -> bool {
        self.is_nav_signal()
            || matches!(
                self.kind,
                WidgetKind::Track { .. }
                    | WidgetKind::PianoRoll { .. }
                    | WidgetKind::TimeRuler { .. }
            )
    }

    /// Whether this tree contains a widget whose overlay follows the pointer —
    /// the cursor readout a signal element over *stored* samples draws, and the
    /// timeline containers'. The windowed front asks on cursor motion: such a
    /// window needs a frame per move (a fully static one, like a plot's, has no
    /// other frame source; a live one is already redrawn every tick).
    pub fn has_hover_readout(&self) -> bool {
        self.descendants()
            .any(|w| w.is_timeline() || w.signal().is_some_and(|el| !el.is_live()))
    }

    /// Every widget in this subtree, `self` first and each child's subtree in
    /// order — the tree's one traversal.
    ///
    /// Nearly everything a pass wants from the tree is a filter over this: the
    /// live buses a window reads, the timeline views on an axis, the ids the
    /// server leg must query. Writing each of those as its own recursion is
    /// what the walk-shaped helper functions used to be, and every one of them
    /// had to re-state the same two lines to get the order right.
    ///
    /// Order is **pre-order**, which is the order the layout emits and the
    /// order the drawing depends on: a parent is seen before the children it
    /// contains.
    pub fn descendants(&self) -> Descendants<'_> {
        Descendants { stack: vec![self] }
    }

    /// The widget with id `id` anywhere in this tree.
    pub fn find(&self, id: i32) -> Option<&Widget> {
        self.descendants().find(|w| w.id == Some(id))
    }

    /// The widget with id `id` anywhere in this tree, mutably (for `/gui_set`
    /// and interaction).
    pub fn find_mut(&mut self, id: i32) -> Option<&mut Widget> {
        if self.id == Some(id) {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }
}

/// The pre-order walk of a widget subtree ([`Widget::descendants`]).
///
/// An explicit stack rather than a recursion, so a caller can `filter`,
/// `find` or `any` over the tree and stop where it likes — a deep tree costs
/// no call frames.
pub struct Descendants<'a> {
    stack: Vec<&'a Widget>,
}

impl<'a> Iterator for Descendants<'a> {
    type Item = &'a Widget;

    fn next(&mut self) -> Option<&'a Widget> {
        let widget = self.stack.pop()?;
        // Reversed, so the pop order is the children's own order.
        self.stack.extend(widget.children.iter().rev());
        Some(widget)
    }
}

impl WidgetKind {
    /// The current value as an OSC primitive for a `/gui_event`, or `None` for a
    /// non-interactive widget. A `button` reports `1` (it is momentary; the press
    /// is the event).
    pub fn event_value(&self) -> Option<OscType> {
        match self {
            WidgetKind::Slider { range: r, .. } | WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                Some(OscType::Float(r.value))
            }
            WidgetKind::Toggle { value, .. } => Some(OscType::Int(*value as i32)),
            WidgetKind::Menu { index, .. } => Some(OscType::Int(*index as i32)),
            WidgetKind::Text { value, .. } => Some(OscType::String(value.clone())),
            WidgetKind::Button { .. } => Some(OscType::Int(1)),
            _ => None,
        }
    }

    /// The control bus a live (shared-memory-backed) widget reads each frame,
    /// if this is one. The windowed front uses it to know which windows to
    /// animate and which bus to sample. An audio-rate view reads recorded
    /// samples or a published level instead — see [`Self::audio_buses_read`]
    /// and [`Self::level_bus`].
    pub fn live_bus(&self) -> Option<i32> {
        match self {
            WidgetKind::Meter { bus, rate, .. } if !rate.is_audio() => Some(*bus),
            WidgetKind::Signal(el) if el.presentation == Presentation::Signal => el
                .source
                .bus()
                .filter(|b| !b.rate.is_audio())
                .map(|b| b.bus),
            _ => None,
        }
    }

    /// The audio bus whose **published level** this widget reads each frame, if
    /// this is one. A meter wants one number per block, not samples, so it
    /// reads the segment's level table and asks the server to record nothing.
    pub fn level_bus(&self) -> Option<i32> {
        match self {
            WidgetKind::Meter { bus, rate, .. } if rate.is_audio() => Some(*bus),
            _ => None,
        }
    }

    /// Appends every audio bus whose **samples** this widget reads each frame —
    /// `channels` adjacent buses for an audio-rate `scope` or a `spectrum`, two
    /// (left and right) for a `phasescope`. This is the set the host asks the
    /// server to record (`/bus_tap`) and the set it animates for, so all three
    /// sample consumers are covered uniformly. A meter is deliberately absent:
    /// its level costs no recording.
    pub fn audio_buses_read(&self, out: &mut Vec<i32>) {
        let Some(el) = self.signal() else { return };
        let Some(bus) = el.source.bus() else { return };
        match el.presentation {
            // The phase view is a stereo pair by construction: a bus and the
            // one beside it, whatever `channels` says.
            Presentation::Phase => out.extend([bus.bus, bus.bus + 1]),
            // A control-rate trace is read as a bus value, not as samples.
            Presentation::Signal if !bus.rate.is_audio() => {}
            _ => out.extend((0..bus.channels as i32).map(|k| bus.bus + k)),
        }
    }

    /// The editor chrome of a view that carries one — a timeline view
    /// (waveform/spectrogram) or a `track` lane, which reuses the same props for
    /// its ruler and playhead. The shared read path for the frame renderer and
    /// the fronts. (Group membership is `is_timeline`, not this: a lane has the
    /// chrome but navigates with the window's clip span.)
    pub fn editor(&self) -> Option<&EditorProps> {
        match self {
            WidgetKind::Signal(el) => Some(&el.editor),
            WidgetKind::Track { editor, .. }
            | WidgetKind::PianoRoll { editor, .. }
            | WidgetKind::TimeRuler { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// Mutable access to a view's editor chrome (the selection drag writes
    /// through here).
    pub fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        match self {
            WidgetKind::Signal(el) => Some(&mut el.editor),
            WidgetKind::Track { editor, .. }
            | WidgetKind::PianoRoll { editor, .. }
            | WidgetKind::TimeRuler { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// The server group a `nodetree` widget mirrors, if this is one. The windowed
    /// front uses it to know which groups to query and which windows to refresh.
    pub fn node_tree_group(&self) -> Option<i32> {
        match self {
            WidgetKind::NodeTree { group, .. } => Some(*group),
            _ => None,
        }
    }

    /// Applies one `/gui_set` key/value to a live widget, returning whether it
    /// changed anything the renderer cares about.
    /// The signal element this kind is, if it is one.
    pub fn signal(&self) -> Option<&SignalElement> {
        match self {
            WidgetKind::Signal(el) => Some(el),
            _ => None,
        }
    }

    /// The signal element this kind is, mutably — a bulk load and a `/gui_set`
    /// both write through here.
    pub fn signal_mut(&mut self) -> Option<&mut SignalElement> {
        match self {
            WidgetKind::Signal(el) => Some(el),
            _ => None,
        }
    }

    /// Recomputes a stored spectrum's cached analysis from its current samples
    /// and props — a no-op for every other widget and every other
    /// presentation. Called at the element's mutation points (parse, a bulk
    /// load landing samples, a live `/gui_set` touching what the analysis
    /// reads), which keeps the per-frame render pure and allocation-light.
    pub fn refresh_analysis(&mut self) {
        if let WidgetKind::Signal(el) = self {
            el.refresh_analysis();
        }
    }

    pub fn apply(&mut self, key: &str, v: &Value) -> bool {
        apply::apply_kind(self, key, v)
    }
}

impl Widget {
    /// The signal element this widget draws with: its own, or — for a `clip` —
    /// the **take** among its bodies.
    ///
    /// A clip's bodies carry no id, so everything that resolves a widget by id
    /// and then wants its samples (a bulk load landing, a buffer fetch coming
    /// back) lands on the clip and reaches the take through here. That is the
    /// containment stated once: a body's id *is* its container's.
    pub fn signal_target(&self) -> Option<&SignalElement> {
        match &self.kind {
            WidgetKind::Signal(el) => Some(el),
            WidgetKind::Clip { .. } => self.children.iter().find_map(|c| match &c.kind {
                WidgetKind::Signal(el) => Some(&**el),
                _ => None,
            }),
            _ => None,
        }
    }

    /// [`signal_target`](Self::signal_target), mutably — the door a bulk load
    /// writes its samples or its pyramid through.
    pub fn signal_target_mut(&mut self) -> Option<&mut SignalElement> {
        match &mut self.kind {
            WidgetKind::Signal(el) => Some(el),
            WidgetKind::Clip { .. } => self.children.iter_mut().find_map(|c| match &mut c.kind {
                WidgetKind::Signal(el) => Some(&mut **el),
                _ => None,
            }),
            _ => None,
        }
    }

    /// The body of `kind` among a clip's children, mutably — the door a
    /// `/gui_set` of a body prop and an edit-back both write through.
    pub(crate) fn clip_body_mut(&mut self, is: fn(&WidgetKind) -> bool) -> Option<&mut WidgetKind> {
        self.children
            .iter_mut()
            .map(|c| &mut c.kind)
            .find(|k| is(k))
    }

    /// Adds the body `is` names to this clip when it has none yet, empty, so a
    /// `/gui_set` that introduces a body has somewhere to land. Layering order
    /// is take → notes → curve, and a body added later keeps it: an envelope
    /// set on a clip that already has a take is drawn *over* it, which is the
    /// whole point of the bodies being a composition.
    pub(crate) fn ensure_body(&mut self, is: fn(&WidgetKind) -> bool) {
        if !matches!(self.kind, WidgetKind::Clip { .. }) || self.clip_body(is).is_some() {
            return;
        }
        let Some(kind) = build::empty_clip_body(is) else {
            return;
        };
        let rank = |k: &WidgetKind| match k {
            WidgetKind::Signal(_) => 0,
            WidgetKind::PianoRoll { .. } => 1,
            _ => 2,
        };
        let at = self
            .children
            .iter()
            .position(|c| rank(&c.kind) > rank(&kind))
            .unwrap_or(self.children.len());
        self.children.insert(at, build::body_widget(kind));
    }

    /// This widget's own kind when `is` names it, else the body of that kind
    /// among its children. The reader's half of the routing `apply_widget`
    /// does for writes: an edit-back payload asks the widget it was addressed
    /// to, and a clip answers with the body that owns the data.
    pub(crate) fn kind_or_body(&self, is: fn(&WidgetKind) -> bool) -> Option<&WidgetKind> {
        if is(&self.kind) {
            return Some(&self.kind);
        }
        self.clip_body(is)
    }

    /// The body of `kind` among a clip's children.
    pub(crate) fn clip_body(&self, is: fn(&WidgetKind) -> bool) -> Option<&WidgetKind> {
        self.children.iter().map(|c| &c.kind).find(|k| is(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(json: &str) -> GuiNode {
        GuiNode::parse(json.as_bytes()).unwrap()
    }

    /// The traversal is pre-order and keeps each level's own order: a parent
    /// before the children it contains, siblings left to right. Every pass that
    /// reads the tree through it inherits that order, so it is pinned here
    /// rather than in each of them.
    #[test]
    fn descendants_walk_parents_before_children_in_order() {
        let tree = Widget::from_node(
            1,
            &node(
                r#"{"id":1,"type":"window","children":[
                    {"id":2,"type":"layout","children":[
                        {"id":3,"type":"label","text":"a"},
                        {"id":4,"type":"label","text":"b"}]},
                    {"id":5,"type":"layout","children":[
                        {"id":6,"type":"label","text":"c"}]}]}"#,
            ),
            &[],
        )
        .unwrap();
        let ids: Vec<i32> = tree.descendants().filter_map(|w| w.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
    }

    /// A plane's zoom is nameable *and* clearable: a positive number is the
    /// scale, and anything else — `0`, an empty string — puts it back to the
    /// default, which is the only way the wire can ask for a default it has no
    /// number for.
    #[test]
    fn a_plane_zoom_is_named_or_cleared() {
        let m = crate::host::metrics::Metrics::default().resolved(2.0);
        let view = |json: &str| match Widget::from_node(1, &node(json), &[]).unwrap().kind {
            WidgetKind::Scroll { view, .. } => view,
            other => panic!("not a scroll: {other:?}"),
        };
        assert_eq!(view(r#"{"type":"plane"}"#).view_zoom, None);
        assert_eq!(view(r#"{"type":"plane","view_zoom":0}"#).view_zoom, None);
        assert_eq!(
            view(r#"{"type":"plane","view_zoom":3}"#).view_zoom,
            Some(3.0)
        );

        let mut kind = Widget::from_node(1, &node(r#"{"type":"plane"}"#), &[])
            .unwrap()
            .kind;
        let zoom_of = |kind: &WidgetKind| match kind {
            WidgetKind::Scroll { view, .. } => view.zoom(&m),
            other => panic!("not a scroll: {other:?}"),
        };
        let set = |kind: &mut WidgetKind, v: Value| super::apply::apply_kind(kind, "view_zoom", &v);
        assert!(set(&mut kind, serde_json::json!(3.0)));
        assert_eq!(zoom_of(&kind), 3.0);
        for cleared in [serde_json::json!(0), serde_json::json!("")] {
            assert!(set(&mut kind, serde_json::json!(3.0)));
            assert!(set(&mut kind, cleared.clone()), "{cleared} is handled");
            assert_eq!(
                zoom_of(&kind),
                m.ui_scale as f64,
                "{cleared} restores the default"
            );
        }
    }

    #[test]
    fn label_text_props_parse_and_default() {
        let n = node(r#"{"type":"label","text":"hi","text_size":3.5,"wrap":1,"align":"center"}"#);
        match Widget::from_node(1, &n, &[]).unwrap().kind {
            WidgetKind::Label {
                text_size,
                wrap,
                align,
                ..
            } => {
                assert_eq!(text_size, 3.5);
                assert!(wrap);
                assert_eq!(align, Align::Center);
            }
            other => panic!("expected label, got {other:?}"),
        }
        let n = node(r#"{"type":"label","text":"hi"}"#);
        match Widget::from_node(1, &n, &[]).unwrap().kind {
            WidgetKind::Label {
                text_size,
                wrap,
                align,
                ..
            } => {
                assert_eq!(text_size, super::super::font::DEFAULT_SIZE);
                assert!(!wrap);
                assert_eq!(align, Align::Start);
            }
            other => panic!("expected label, got {other:?}"),
        }
    }

    #[test]
    fn themes_resolve_recursively_at_the_mutation_point() {
        // A theme group on a container overlays its whole subtree; a nested
        // group overlays the *inherited* table; a `color` re-seeds one widget;
        // a widget with neither shares its parent's Arc.
        let n = node(
            r##"{"type":"window","children":[
              {"id":11,"type":"layout","theme":{"accent":"#ff0000"},"children":[
                {"id":12,"type":"label","text":"in the group"},
                {"id":13,"type":"slider","color":"#0000ff"},
                {"id":14,"type":"layout","theme":{"text":"#00ff00"},"children":[
                  {"id":15,"type":"label","text":"nested"}]}]},
              {"id":16,"type":"label","text":"outside"}]}"##,
        );
        let mut w = Widget::from_node(1, &n, &[]).unwrap();
        let base = Arc::new(super::super::theme::Theme::default());
        resolve_themes(&mut w, &base);
        let theme_of = |id: i32| w.find(id).unwrap().theme.clone().unwrap();
        let red = [1.0, 0.0, 0.0, 1.0];
        assert_eq!(theme_of(12).accent, red, "the group reaches the subtree");
        assert_eq!(
            theme_of(13).accent,
            [0.0, 0.0, 1.0, 1.0],
            "color wins on its widget"
        );
        assert_eq!(
            theme_of(13).text,
            base.text,
            "color leaves the group's other roles"
        );
        let nested = theme_of(15);
        assert_eq!(
            nested.accent, red,
            "a nested group inherits the outer overlay"
        );
        assert_eq!(nested.text, [0.0, 1.0, 0.0, 1.0], "and adds its own");
        assert!(
            Arc::ptr_eq(&theme_of(16), &base),
            "outside any group the host theme is shared, not cloned"
        );
    }

    #[test]
    fn style_props_set_live_and_clear() {
        let n = node(r#"{"type":"layout"}"#);
        let mut w = Widget::from_node(1, &n, &[]).unwrap();
        assert!(w.style_apply("color", &Value::from("#112233")));
        assert!(w.color.is_some());
        assert!(w.style_apply("color", &Value::from("")), "empty clears");
        assert!(w.color.is_none());
        // `theme` rides as a JSON object or its string carrier.
        assert!(w.style_apply("theme", &Value::from(r##"{"accent":"#ff0000"}"##)));
        assert!(w.theme_over.is_some());
        assert!(
            w.style_apply("theme", &Value::from("")),
            "empty clears the group"
        );
        assert!(w.theme_over.is_none());
        assert!(!w.style_apply("color", &Value::from("nonsense")));
        assert!(!w.style_apply("value", &Value::from(1)), "not a style key");
    }

    #[test]
    fn text_size_sets_live_and_clamps() {
        let n = node(r#"{"type":"slider","label":"amp"}"#);
        let mut w = Widget::from_node(1, &n, &[]).unwrap();
        assert!(w.kind.apply("text_size", &Value::from(4.0)));
        match &w.kind {
            WidgetKind::Slider { range, .. } => assert_eq!(range.text_size, 4.0),
            other => panic!("expected slider, got {other:?}"),
        }
        // Out-of-range sizes clamp instead of degenerating the strip math.
        assert!(w.kind.apply("text_size", &Value::from(0.0)));
        match &w.kind {
            WidgetKind::Slider { range, .. } => assert_eq!(range.text_size, 1.0),
            other => panic!("expected slider, got {other:?}"),
        }
        // A bad align value is rejected, the good ones apply.
        let n = node(r#"{"type":"label","text":"hi"}"#);
        let mut w = Widget::from_node(2, &n, &[]).unwrap();
        assert!(!w.kind.apply("align", &Value::from("sideways")));
        assert!(w.kind.apply("align", &Value::from("end")));
        assert!(w.kind.apply("wrap", &Value::from(1)));
    }

    #[test]
    fn window_with_inline_waveform() {
        let n = node(
            r#"{"type":"window","title":"W","w":480,"h":240,"flow":"col",
                "children":[{"id":12,"type":"signal","view":"trace","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
        );
        let w = Widget::from_node(1, &n, &[]).unwrap();
        assert_eq!(w.id, Some(1));
        match w.kind {
            WidgetKind::Window {
                title,
                width,
                height,
                layout,
                ..
            } => {
                assert_eq!(title.as_deref(), Some("W"));
                assert_eq!((width, height), (480, 240));
                assert_eq!(layout, Layout::Col);
            }
            other => panic!("expected window, got {other:?}"),
        }
        assert_eq!(w.children.len(), 1);
        let data = w.children[0]
            .signal()
            .and_then(|el| el.source.data())
            .expect("a waveform is a signal element over addressable samples");
        assert_eq!(&data.samples[..], &[0.0, 0.5, -0.5, 1.0]);
        assert_eq!(data.base_bucket, 2);
        assert_eq!(data.buffer, None);
    }

    #[test]
    fn waveform_parses_its_placement_offset() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"trace","data":[0.0,1.0],"offset":8.0},
                {"id":2,"type":"signal","view":"trace","data":[0.0,1.0],"offset":-3.0}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        assert_eq!(w.children[0].kind.editor().unwrap().offset, 8.0);
        // A negative placement clamps to 0 (no clip starts before the timeline).
        assert_eq!(w.children[1].kind.editor().unwrap().offset, 0.0);
        // The default is un-placed.
        let n = node(
            r#"{"type":"window","children":[{"id":3,"type":"signal","view":"trace","data":[0.0]}]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        assert_eq!(w.children[0].kind.editor().unwrap().offset, 0.0);
    }

    #[test]
    fn track_carries_clips_with_their_placement() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"field","label":"drums","children":[
                    {"id":10,"type":"field","offset":0.0,"dur":100.0,"data":[0.0,1.0],"label":"a"},
                    {"id":11,"type":"field","offset":-5.0,"dur":50.0}
                ]}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        let track = &w.children[0];
        match &track.kind {
            WidgetKind::Track { label, .. } => assert_eq!(label.as_deref(), Some("drums")),
            other => panic!("expected track, got {other:?}"),
        }
        assert_eq!(track.children.len(), 2, "a track carries its clips");
        let clip = &track.children[0];
        match &clip.kind {
            WidgetKind::Clip { offset, dur, label } => {
                assert_eq!((*offset, *dur), (0.0, 100.0));
                assert_eq!(label.as_deref(), Some("a"));
            }
            other => panic!("expected clip, got {other:?}"),
        }
        // The take is a **child** of the clip, and an ordinary signal element:
        // the clip is a container, so what it holds is elements.
        assert_eq!(clip.children.len(), 1, "one body: the take");
        let take = clip.signal_target().expect("the clip holds a take");
        assert_eq!(&take.source.data().unwrap().samples[..], &[0.0, 1.0]);
        assert!(
            !take.caps.navigable,
            "a body navigates nothing: the clip does"
        );
        assert!(take.source.data().unwrap().bulk, "a take is bulk");
        // A clip with no source at all holds no take: a body a clip does not
        // describe is simply absent.
        assert!(track.children[1].children.is_empty());
        // A negative offset clamps to 0 (no clip starts before the timeline).
        match &track.children[1].kind {
            WidgetKind::Clip { offset, .. } => assert_eq!(*offset, 0.0),
            other => panic!("expected clip, got {other:?}"),
        }
    }

    #[test]
    fn a_lane_carries_the_ruler_and_playhead_chrome() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"field","ruler":"beats","tempo":2.0,"playhead_at":480.0},
                {"id":2,"type":"field"}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let lane = w.children[0].kind.editor().unwrap();
        assert_eq!(lane.ruler, Ruler::Beats);
        assert_eq!((lane.tempo, lane.playhead_at), (2.0, 480.0));
        // A lane asks for no ruler by default (it reserves no strip), and shows
        // no playhead until one is anchored.
        let plain = w.children[1].kind.editor().unwrap();
        assert_eq!(plain.ruler, Ruler::Off);
        assert!(plain.playhead_at < 0.0);
        // The chrome parses live too: `/gui_set` reaches these fields (what a
        // lane *draws* is its navigation group's playhead, which these seed).
        assert!(
            w.children[1]
                .kind
                .apply("playhead_at", &serde_json::json!(96000.0))
        );
        assert_eq!(w.children[1].kind.editor().unwrap().playhead_at, 96000.0);
    }

    #[test]
    fn a_clip_parses_its_piano_roll_notes() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"field","children":[
                    {"id":10,"type":"field","offset":0.0,"dur":400.0,"min":48.0,"max":72.0,
                     "notes":[0.0,100.0,60.0, 100.0,100.0,67.0, 999.0]}
                ]}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        let clip = &w.children[0].children[0];
        match clip.children.first().map(|c| &c.kind) {
            Some(WidgetKind::PianoRoll {
                notes, min, max, ..
            }) => {
                // Two complete triples; the trailing lone number is dropped.
                assert_eq!(notes.len(), 2);
                assert_eq!(
                    (notes[0].start, notes[0].dur, notes[0].pitch),
                    (0.0, 100.0, 60.0)
                );
                assert_eq!(notes[1].pitch, 67.0);
                assert_eq!((*min, *max), (48.0, 72.0));
            }
            other => panic!("expected a roll body, got {other:?}"),
        }
    }

    #[test]
    fn a_pianoroll_parses_its_notes_osc_and_pitch_window() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":5,"type":"notes","min":36.0,"max":84.0,"snap":100.0,
                 "notes":[0.0,200.0,60.0,90,0, 200.0,200.0,64.0,110,1],
                 "osc":[400.0,"/trig", 800.0,""]}
            ]}"#,
        );
        let w = Widget::from_node(1, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::PianoRoll {
                notes,
                osc,
                min,
                max,
                snap,
                velocity_lane,
                osc_lane,
                ..
            } => {
                assert_eq!(notes.len(), 2);
                assert_eq!(
                    (notes[0].pitch, notes[0].velocity, notes[0].channel),
                    (60.0, 90, 0)
                );
                assert_eq!((notes[1].velocity, notes[1].channel), (110, 1));
                assert_eq!(osc.len(), 2);
                assert_eq!(osc[0].label.as_deref(), Some("/trig"));
                assert_eq!(osc[1].label, None); // the empty string is no label
                assert_eq!((*min, *max, *snap), (36.0, 84.0, 100.0));
                assert!(*velocity_lane, "the velocity lane is on by default");
                assert!(*osc_lane, "the OSC lane opens because there are events");
            }
            other => panic!("expected pianoroll, got {other:?}"),
        }
    }

    #[test]
    fn a_pianoroll_midi_in_parses_and_defaults_off() {
        let on = node(r#"{"type":"window","children":[{"id":5,"type":"notes","midi_in":true}]}"#);
        let w = Widget::from_node(1, &on, &[]).unwrap();
        assert!(matches!(
            &w.children[0].kind,
            WidgetKind::PianoRoll { midi_in: true, .. }
        ));
        let off = node(r#"{"type":"window","children":[{"id":5,"type":"notes"}]}"#);
        let w = Widget::from_node(1, &off, &[]).unwrap();
        assert!(matches!(
            &w.children[0].kind,
            WidgetKind::PianoRoll { midi_in: false, .. }
        ));
    }

    #[test]
    fn waveform_by_server_buffer_starts_empty_with_the_buffer_number() {
        let n = node(
            r#"{"type":"window","children":[{"id":3,"type":"signal","view":"trace","buffer":7}]}"#,
        );
        let w = Widget::from_node(1, &n, &[]).unwrap();
        let data = w.children[0]
            .signal()
            .and_then(|el| el.source.data())
            .unwrap();
        assert!(
            data.samples.is_empty(),
            "no inline data yet — fetched later"
        );
        assert_eq!(data.buffer, Some(7));
    }

    #[test]
    fn waveform_by_path_and_cache_defer_with_their_props() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"trace","path":"/tmp/buf.f32","channels":2},
                {"id":2,"type":"signal","view":"trace","cache":"/tmp/buf.peaks"}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        let data = w.children[0]
            .signal()
            .and_then(|el| el.source.data())
            .unwrap();
        assert!(
            data.samples.is_empty(),
            "samples are mapped later, not inline"
        );
        assert_eq!(
            data.path.as_deref(),
            Some(std::path::Path::new("/tmp/buf.f32"))
        );
        assert_eq!(data.channels, 2);
        let data = w.children[1]
            .signal()
            .and_then(|el| el.source.data())
            .unwrap();
        assert_eq!(
            data.cache.as_deref(),
            Some(std::path::Path::new("/tmp/buf.peaks"))
        );
    }

    #[test]
    fn meter_and_scope_parse_with_defaults_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"meter","bus":5,"max":2.0,"label":"out"},
                {"id":2,"type":"signal","view":"trace","bus":6}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Meter {
                bus,
                rate,
                min,
                max,
                label,
            } => {
                assert_eq!((*bus, *min, *max), (5, 0.0, 2.0));
                assert_eq!(*rate, Rate::Audio, "a meter watches audio unless told");
                assert_eq!(label.as_deref(), Some("out"));
            }
            other => panic!("expected meter, got {other:?}"),
        }
        // The scope is a signal element over a forward-only source, and
        // defaults to the bipolar [-1, 1] range.
        let el = w.children[1].signal().expect("a scope is a signal element");
        assert_eq!(el.source.bus().unwrap().bus, 6);
        assert_eq!((el.value.min, el.value.max), (Some(-1.0), Some(1.0)));
        // An audio-rate meter reads a published level, not a control bus.
        assert_eq!(w.children[0].kind.live_bus(), None);
        assert_eq!(w.children[0].kind.level_bus(), Some(5));
        // A live `/gui_set` can retarget the bus, rescale the meter, and move
        // it between the rates.
        let meter = w.find_mut(1).unwrap();
        assert!(meter.kind.apply("bus", &Value::from(8)));
        assert!(meter.kind.apply("max", &Value::from(4.0)));
        assert_eq!(meter.kind.level_bus(), Some(8));
        assert!(meter.kind.apply("rate", &Value::from("control")));
        assert_eq!(meter.kind.live_bus(), Some(8));
        assert_eq!(meter.kind.level_bus(), None);
    }

    #[test]
    fn nodetree_and_plot_parse_with_defaults_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"nodes","group":2,"controls":0,"label":"tree"},
                {"id":2,"type":"signal","navigable":0,"data":[0.0,1.0,-1.0],"max":2.0,"label":"sig"}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::NodeTree {
                group,
                controls,
                label,
            } => {
                assert_eq!((*group, *controls), (2, false));
                assert_eq!(label.as_deref(), Some("tree"));
            }
            other => panic!("expected nodetree, got {other:?}"),
        }
        assert_eq!(w.children[0].kind.node_tree_group(), Some(2));
        // A nodetree is non-interactive and reads no bus.
        assert_eq!(w.children[0].kind.event_value(), None);
        assert_eq!(w.children[0].kind.live_bus(), None);
        let el = w.children[1].signal().expect("a plot is a signal element");
        assert_eq!(&el.source.data().unwrap().samples[..], &[0.0, 1.0, -1.0]);
        // An explicit side is kept; the omitted one auto-fits.
        assert_eq!((el.value.min, el.value.max), (None, Some(2.0)));
        // A plot is the point of the product with every capability off.
        assert_eq!(el.caps, signal::Caps::default());
        // Live `/gui_set` retargets the tree's group and rescales the plot.
        assert!(w.find_mut(1).unwrap().kind.apply("group", &Value::from(0)));
        assert!(w.find_mut(2).unwrap().kind.apply("max", &Value::from(1.0)));
        assert_eq!(w.children[0].kind.node_tree_group(), Some(0));
    }

    #[test]
    fn plot_parses_views_channels_and_applies_live() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","navigable":0,"data":[0.0,1.0,0.0,-1.0],"channels":2,
                 "view":"spectrum","overlay":1,"sample_rate":48000.0,
                 "fft_size":1024,"freq_scale":"mel","ruler":"time","ruler_y":"off"}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let el = w.children[0].signal().unwrap();
        assert_eq!(el.channels(), 2);
        assert_eq!(el.presentation, Presentation::Spectrum);
        assert!(el.display.overlay);
        assert_eq!(el.editor.sample_rate, 48_000.0);
        assert_eq!(el.spectral.fft_size, 1024);
        assert_eq!(el.spectral.freq_scale, FreqScale::Mel);
        assert_eq!(el.editor.ruler, Ruler::Time);
        assert_eq!(el.editor.ruler_y, RulerY::Off);
        // The spectrum presentation analyzed its (inline) samples at parse.
        let spec = el.analysis.as_ref().expect("analysis cached at parse");
        assert_eq!(spec.curves.len(), 2);
        assert_eq!(spec.fft_size, 1024);
        // Live `/gui_set`: back to the signal view drops the analysis; a
        // numeric `min` pins that side and the string "auto" releases it.
        let kind = &mut w.find_mut(1).unwrap().kind;
        assert!(kind.apply("view", &Value::from("signal")));
        assert!(kind.apply("min", &Value::from(-2.0)));
        let el = kind.signal().unwrap();
        assert!(
            el.analysis.is_none(),
            "the signal presentation holds no analysis"
        );
        assert_eq!(el.value.min, Some(-2.0));
        assert!(kind.apply("min", &Value::from("auto")));
        assert!(kind.apply("view", &Value::from("spectrum")));
        let el = kind.signal().unwrap();
        assert_eq!(el.value.min, None);
        assert!(el.analysis.is_some(), "switching back re-analyzes");
        // An unknown view name is rejected (the prop keeps its value).
        assert!(!kind.apply("view", &Value::from("histogram")));
    }

    #[test]
    fn canvas_parses_shader_params_buses_and_applies() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"canvas","shader":"fn shade(){}","params":[0.5,0.25],"buses":[7]}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Canvas {
                shader,
                params,
                buses,
                ..
            } => {
                assert_eq!(shader, "fn shade(){}");
                // The given params/buses fill the front of the fixed arrays; the
                // rest default (0.0 / -1).
                assert_eq!(*params, [0.5, 0.25, 0.0, 0.0]);
                assert_eq!(*buses, [7, -1, -1, -1]);
            }
            other => panic!("expected canvas, got {other:?}"),
        }
        // A canvas is non-interactive and reads no single bus.
        assert_eq!(w.children[0].kind.event_value(), None);
        assert_eq!(w.children[0].kind.live_bus(), None);
        // Live `/gui_set`: a param from the script, a bus remap, a new shader.
        let c = w.find_mut(1).unwrap();
        assert!(c.kind.apply("param1", &Value::from(0.75)));
        assert!(c.kind.apply("bus0", &Value::from(9)));
        assert!(c.kind.apply("shader", &Value::from("fn shade2(){}")));
        assert!(
            !c.kind.apply("param9", &Value::from(1.0)),
            "out-of-range slot"
        );
        match &c.kind {
            WidgetKind::Canvas {
                shader,
                params,
                buses,
                ..
            } => {
                assert_eq!(params[1], 0.75);
                assert_eq!(buses[0], 9);
                assert_eq!(shader, "fn shade2(){}");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn canvas_without_a_shader_gets_the_default() {
        let n = node(r#"{"type":"window","children":[{"id":1,"type":"canvas"}]}"#);
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Canvas { shader, .. } => {
                assert!(
                    shader.contains("fn shade"),
                    "falls back to the default shader"
                )
            }
            other => panic!("expected canvas, got {other:?}"),
        }
    }

    #[test]
    fn plot_by_path_defers_empty_with_its_props() {
        let n = node(
            r#"{"type":"window","children":[{"id":3,"type":"signal","navigable":0,"path":"/tmp/sig.f32","channels":2}]}"#,
        );
        let w = Widget::from_node(1, &n, &[]).unwrap();
        let data = w.children[0]
            .signal()
            .and_then(|el| el.source.data())
            .unwrap();
        assert!(data.samples.is_empty(), "mapped later, not inline");
        assert_eq!(
            data.path.as_deref(),
            Some(std::path::Path::new("/tmp/sig.f32"))
        );
        assert_eq!(data.channels, 2);
    }

    #[test]
    fn waveform_from_blob() {
        let blob: Vec<u8> = [1.0f32, -1.0]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let n = node(
            r#"{"type":"window","children":[{"id":2,"type":"signal","view":"trace","blob":0}]}"#,
        );
        let w = Widget::from_node(1, &n, &[blob]).unwrap();
        let data = w.children[0]
            .signal()
            .and_then(|el| el.source.data())
            .unwrap();
        assert_eq!(&data.samples[..], &[1.0, -1.0]);
    }

    #[test]
    fn phasescope_and_spectrum_parse_with_defaults_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"phase","bus":2},
                {"id":2,"type":"signal","view":"spectrum","bus":0,"fft_size":1024,"log_freq":0}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let el = w.children[0].signal().unwrap();
        assert_eq!(el.presentation, Presentation::Phase);
        let bus = el.source.bus().unwrap();
        assert_eq!(bus.bus, 2, "the right channel is the next bus");
        assert_eq!(bus.window_ms, 30.0);
        assert!(!bus.hold);
        // A phasescope reads both buses; it is not a single-bus widget.
        let mut buses = Vec::new();
        w.children[0].kind.audio_buses_read(&mut buses);
        assert_eq!(buses, vec![2, 3]);
        assert_eq!(w.children[0].kind.live_bus(), None);
        let el = w.children[1].signal().unwrap();
        assert_eq!(el.presentation, Presentation::Spectrum);
        assert_eq!(
            (el.source.bus().unwrap().bus, el.spectral.fft_size),
            (0, 1024)
        );
        assert_eq!((el.spectral.db_floor, el.spectral.db_ceil), (-100.0, 0.0));
        assert_eq!(
            el.spectral.freq_scale,
            FreqScale::Linear,
            "legacy log_freq: 0 reads as linear"
        );
        // Live `/gui_set`: retarget a tap, resize the FFT (only a supported size
        // takes), reshape the frequency axis, retune the phasescope window and
        // freeze it.
        assert!(
            w.find_mut(2)
                .unwrap()
                .kind
                .apply("fft_size", &Value::from(2048))
        );
        assert!(
            !w.find_mut(2)
                .unwrap()
                .kind
                .apply("fft_size", &Value::from(1000))
        );
        assert!(
            w.find_mut(2)
                .unwrap()
                .kind
                .apply("freq_scale", &Value::from("mel"))
        );
        assert!(
            !w.find_mut(2)
                .unwrap()
                .kind
                .apply("freq_scale", &Value::from("nope"))
        );
        assert!(w.find_mut(1).unwrap().kind.apply("hold", &Value::from(1)));
        let el = w.find_mut(2).unwrap().signal().unwrap();
        assert_eq!(el.spectral.fft_size, 2048);
        assert_eq!(el.spectral.freq_scale, FreqScale::Mel);
    }

    #[test]
    fn multichannel_scope_and_spectrum_read_adjacent_buses() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"trace","bus":4,"channels":2,"overlay":1,"ruler":"off"},
                {"id":2,"type":"signal","view":"spectrum","bus":6,"channels":3,"ruler_y":"off"}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let el = w.children[0].signal().unwrap();
        assert_eq!(el.channels(), 2);
        assert!(el.display.overlay);
        assert_eq!(el.editor.ruler, Ruler::Off, "\"off\" hides the x strip");
        assert_eq!(
            el.editor.ruler_y,
            RulerY::Norm,
            "the y strip defaults on, in the presentation's own unit"
        );
        // Each consumer reads its whole adjacent run of buses.
        let mut buses = Vec::new();
        w.children[0].kind.audio_buses_read(&mut buses);
        assert_eq!(buses, vec![4, 5]);
        buses.clear();
        w.children[1].kind.audio_buses_read(&mut buses);
        assert_eq!(buses, vec![6, 7, 8]);
        // Live: grow the runs and toggle a strip back on.
        assert!(
            w.find_mut(1)
                .unwrap()
                .kind
                .apply("channels", &Value::from(4))
        );
        assert!(w.find_mut(1).unwrap().kind.apply("ruler", &Value::from(1)));
        buses.clear();
        w.find_mut(1).unwrap().kind.audio_buses_read(&mut buses);
        assert_eq!(buses, vec![4, 5, 6, 7]);
        let el = w.children[1].signal().unwrap();
        assert_eq!(el.channels(), 3);
        assert_eq!(el.editor.ruler_y, RulerY::Off);
    }

    #[test]
    fn waveform_editor_props_parse_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"trace","data":[0.0,1.0],"channels":2,"overlay":1,
                 "ruler":"samples","sample_rate":48000.0,"sel_start":100.0,"sel_len":50.0,
                 "playhead_at":1000.0}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let el = w.children[0].signal().unwrap();
        assert_eq!(el.channels(), 2);
        assert!(el.display.overlay);
        assert_eq!(el.editor.ruler, Ruler::Samples);
        assert_eq!(el.editor.sample_rate, 48_000.0);
        assert_eq!((el.editor.sel_start, el.editor.sel_len), (100.0, 50.0));
        assert_eq!(el.editor.playhead_at, 1000.0);
        assert!(w.children[0].is_timeline());
        // The vertical ruler defaults to the normalized amplitude axis.
        assert_eq!(w.children[0].kind.editor().unwrap().ruler_y, RulerY::Norm);
        assert_eq!(w.children[0].kind.editor().unwrap().bit_depth, 16);
        // Live `/gui_set`: retune the selection, clear the playhead, switch
        // the ruler off.
        let wf = w.find_mut(1).unwrap();
        assert!(wf.kind.apply("sel_start", &Value::from(0.0)));
        assert!(wf.kind.apply("sel_len", &Value::from(0.0)));
        assert!(wf.kind.apply("playhead_at", &Value::from(-1.0)));
        assert!(wf.kind.apply("ruler", &Value::from("off")));
        assert!(!wf.kind.apply("ruler", &Value::from("nonesuch")));
        let editor = wf.kind.editor().unwrap();
        assert_eq!(editor.sel_len, 0.0, "zero length clears it");
        assert!(editor.playhead_at < 0.0);
        assert_eq!(editor.ruler, Ruler::Off);
    }

    #[test]
    fn editor_ruler_units_parse_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"trace","data":[0.0],"ruler":"beats",
                 "sample_rate":48000.0,"tempo":2.0,"beat_at":8.0,"quant":3.0,
                 "ruler_y":"db","bit_depth":24}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let editor = w.children[0].kind.editor().unwrap();
        assert_eq!(editor.ruler, Ruler::Beats);
        assert_eq!(
            (editor.tempo, editor.beat_at, editor.quant),
            (2.0, 8.0, 3.0)
        );
        assert_eq!(editor.ruler_y, RulerY::Db);
        assert_eq!(editor.bit_depth, 24);
        // Every unit is live via `/gui_set` (the button-wiring path).
        let wf = w.find_mut(1).unwrap();
        assert!(wf.kind.apply("ruler_y", &Value::from("bits")));
        assert!(wf.kind.apply("bit_depth", &Value::from(8)));
        assert!(wf.kind.apply("tempo", &Value::from(1.5)));
        assert!(wf.kind.apply("quant", &Value::from(4.0)));
        assert!(wf.kind.apply("beat_at", &Value::from(0.0)));
        assert!(!wf.kind.apply("ruler_y", &Value::from("nonesuch")));
        let editor = wf.kind.editor().unwrap();
        assert_eq!(editor.ruler_y, RulerY::Bits);
        assert_eq!(editor.bit_depth, 8);
        assert_eq!((editor.tempo, editor.quant), (1.5, 4.0));
    }

    #[test]
    fn editor_y_view_parses_clamps_and_applies() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"trace","data":[0.0],"y_start":0.8,"y_len":0.5},
                {"id":2,"type":"signal","view":"spectrogram","data":[0.0]}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        // The read-time window clamps inside the axis: 0.8 + 0.5 spills, so
        // the start pulls back to 0.5.
        let editor = w.children[0].kind.editor().unwrap();
        assert_eq!(editor.y_view(), (0.5, 0.5));
        // The default is the full axis.
        let editor = w.children[1].kind.editor().unwrap();
        assert_eq!(editor.y_view(), (0.0, 1.0));
        // Live `/gui_set` zooms and pans; a non-positive length resets.
        let wf = w.find_mut(1).unwrap();
        assert!(wf.kind.apply("y_len", &Value::from(0.25)));
        assert!(wf.kind.apply("y_start", &Value::from(0.7)));
        let editor = wf.kind.editor().unwrap();
        assert_eq!(editor.y_view(), (0.7, 0.25));
        // One set carrying both keys must not depend on key order: applying
        // y_start before y_len used to clamp it against the old full-axis
        // length and silently zero it (the "zoom lands in the wrong half"
        // regression).
        assert!(wf.kind.apply("y_start", &Value::from(0.5)));
        assert!(wf.kind.apply("y_len", &Value::from(0.5)));
        let editor = wf.kind.editor().unwrap();
        assert_eq!(editor.y_view(), (0.5, 0.5));
        assert!(wf.kind.apply("y_len", &Value::from(0.0)));
        let editor = wf.kind.editor().unwrap();
        assert_eq!(editor.y_view(), (0.0, 1.0));
    }

    #[test]
    fn spectrogram_parses_with_defaults_and_applies() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"spectrogram","path":"/tmp/a.f32","channels":2,
                 "sample_rate":44100.0},
                {"id":2,"type":"signal","view":"spectrogram","buffer":3,"window_size":333}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let el = w.children[0].signal().unwrap();
        assert_eq!(el.presentation, Presentation::TimeFrequency);
        let data = el.source.data().unwrap();
        assert_eq!(
            data.path.as_deref(),
            Some(std::path::Path::new("/tmp/a.f32"))
        );
        assert_eq!(
            (data.channels, el.spectral.fft_size, el.spectral.hop),
            (2, 1024, 512)
        );
        assert_eq!(el.editor.sample_rate, 44_100.0);
        assert_eq!((el.spectral.db_floor, el.spectral.db_ceil), (-90.0, 0.0));
        assert_eq!(
            el.spectral.freq_scale,
            FreqScale::Log,
            "log is the default scale"
        );
        assert_eq!(el.spectral.colormap, 0);
        assert_eq!(el.editor.ruler_y, RulerY::Hz, "the Hz ruler defaults on");
        // An unsupported window size degrades to the default.
        let el = w.children[1].signal().unwrap();
        assert_eq!(el.source.data().unwrap().buffer, Some(3));
        assert_eq!(
            el.spectral.fft_size, 1024,
            "333 is not a supported FFT size"
        );
        // Live `/gui_set`: the display uniforms retune with zero recompute.
        let sg = w.find_mut(1).unwrap();
        assert!(sg.kind.apply("db_floor", &Value::from(-60.0)));
        assert!(sg.kind.apply("log_freq", &Value::from(0)), "legacy alias");
        assert!(sg.kind.apply("colormap", &Value::from(1)));
        assert!(sg.kind.apply("sel_start", &Value::from(10.0)));
        let el = sg.signal().unwrap();
        assert_eq!(el.spectral.db_floor, -60.0);
        assert_eq!(
            el.spectral.freq_scale,
            FreqScale::Linear,
            "log_freq 0 -> linear"
        );
        assert_eq!(el.spectral.colormap, 1);
        assert_eq!(el.editor.sel_start, 10.0);
        // The four-scale prop wins over the legacy alias and applies live.
        assert!(sg.kind.apply("freq_scale", &Value::from("mel")));
        assert!(!sg.kind.apply("freq_scale", &Value::from("nonesuch")));
        assert!(sg.kind.apply("ruler_y", &Value::from("off")));
        let el = sg.signal().unwrap();
        assert_eq!(el.spectral.freq_scale, FreqScale::Mel);
        assert_eq!(el.editor.ruler_y, RulerY::Off);
    }

    /// The six wire names land on six configurations of one element. This is
    /// the parse-level half of the preset table's own test: what the wire says
    /// and what the model holds, in one place, so a name cannot quietly change
    /// what it configures.
    #[test]
    fn the_six_names_parse_to_their_point_of_the_product() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"trace","data":[0.0,1.0]},
                {"id":2,"type":"signal","view":"spectrogram","data":[0.0,1.0]},
                {"id":3,"type":"signal","navigable":0,"data":[0.0,1.0]},
                {"id":4,"type":"signal","view":"trace","bus":0},
                {"id":5,"type":"signal","view":"spectrum","bus":0},
                {"id":6,"type":"signal","view":"phase","bus":0}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        let point = |i: usize| {
            let el = w.children[i].signal().expect("a signal element");
            (el.presentation, el.is_live(), el.caps.navigable)
        };
        assert_eq!(point(0), (Presentation::Signal, false, true));
        assert_eq!(point(1), (Presentation::TimeFrequency, false, true));
        assert_eq!(point(2), (Presentation::Signal, false, false));
        assert_eq!(point(3), (Presentation::Signal, true, false));
        assert_eq!(point(4), (Presentation::Spectrum, true, false));
        assert_eq!(point(5), (Presentation::Phase, true, false));
        // Only the navigable ones join the window's time axis.
        let timelines: Vec<bool> = (0..6).map(|i| w.children[i].is_timeline()).collect();
        assert_eq!(timelines, [true, true, false, false, false, false]);
    }

    /// A `/gui_set` key lands on the part of the model it names, and is refused
    /// where that part does not exist — the source keys on a stored element,
    /// the analysis size under either of its two wire names.
    #[test]
    fn a_set_lands_on_the_part_of_the_model_it_names() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"trace","data":[0.0,1.0]},
                {"id":2,"type":"signal","view":"trace","bus":3}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        // A source key means nothing to an element that reads samples.
        assert!(!w.find_mut(1).unwrap().kind.apply("bus", &Value::from(2)));
        assert!(!w.find_mut(1).unwrap().kind.apply("hold", &Value::from(1)));
        // It means everything to one that reads a bus.
        assert!(w.find_mut(2).unwrap().kind.apply("bus", &Value::from(9)));
        assert_eq!(w.children[1].signal().unwrap().source.bus().unwrap().bus, 9);
        // One analysis size, under both of the names the wire has for it.
        assert!(
            w.find_mut(1)
                .unwrap()
                .kind
                .apply("window_size", &Value::from(512))
        );
        assert_eq!(w.children[0].signal().unwrap().spectral.fft_size, 512);
        assert!(
            w.find_mut(1)
                .unwrap()
                .kind
                .apply("fft_size", &Value::from(256))
        );
        assert_eq!(w.children[0].signal().unwrap().spectral.fft_size, 256);
    }

    #[test]
    fn spectrogram_freq_scale_prop_parses_all_four() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"signal","view":"spectrogram","data":[0.0],"freq_scale":"bark"},
                {"id":2,"type":"signal","view":"spectrogram","data":[0.0],"freq_scale":"linear","log_freq":1}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        assert_eq!(
            w.children[0].signal().unwrap().spectral.freq_scale,
            FreqScale::Bark
        );
        // freq_scale wins over the legacy log_freq when both are present.
        assert_eq!(
            w.children[1].signal().unwrap().spectral.freq_scale,
            FreqScale::Linear
        );
    }

    #[test]
    fn bpf_parses_with_defaults_and_applies() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"curve","points":[0.0,0.0,1,0.0, 0.1,1.0,-4.0,0.0, 1.0,0.0,1,0.0],
                 "label":"env"},
                {"id":2,"type":"curve","min":20.0,"max":20000.0,"exp":1,"duration":4.0}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Bpf {
                points,
                min,
                max,
                exp,
                label,
                ..
            } => {
                assert_eq!(points.len(), 3);
                assert_eq!((*min, *max), (0.0, 1.0), "the range defaults unipolar");
                assert!(!*exp);
                assert_eq!(label.as_deref(), Some("env"));
            }
            other => panic!("expected bpf, got {other:?}"),
        }
        // No points: the predictable default flat line, still editable.
        match &w.children[1].kind {
            WidgetKind::Bpf {
                points,
                min,
                max,
                duration,
                exp,
                ..
            } => {
                assert_eq!(points.len(), 2);
                assert_eq!((*min, *max), (20.0, 20_000.0));
                assert_eq!(*duration, 4.0);
                assert!(*exp);
            }
            other => panic!("expected bpf, got {other:?}"),
        }
        // A bpf is neither a timeline view nor a scalar-value control: its
        // edit-back event carries the flat list instead.
        assert!(!w.children[0].is_timeline());
        assert_eq!(w.children[0].kind.event_value(), None);
        // Live `/gui_set`: replace the whole breakpoint list (array or its
        // JSON-string carrier), retune the range and the domain.
        let b = w.find_mut(1).unwrap();
        assert!(
            b.kind
                .apply("points", &Value::from("[0.0,0.5,1,0.0, 2.0,0.25,3,0.0]"))
        );
        assert!(b.kind.apply("duration", &Value::from(3.0)));
        assert!(!b.kind.apply("points", &Value::from("nonesuch")));
        match &b.kind {
            WidgetKind::Bpf {
                points, duration, ..
            } => {
                assert_eq!(points.len(), 2);
                assert_eq!(points[1].shape, 3);
                assert_eq!(*duration, 3.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn place_props_parse_and_apply() {
        let n = node(
            r#"{"type":"window","flow":"row","children":[
            {"id":7,"type":"label","text":"a","w":100.5,"weight":2,"x":4,"y":8}]}"#,
        );
        let w = Widget::from_node(1, &n, &[]).unwrap();
        let child = &w.children[0];
        assert_eq!(child.place.w, Some(100.5));
        assert_eq!(child.place.h, None);
        assert_eq!(child.place.weight, Some(2.0));
        assert_eq!((child.place.x, child.place.y), (Some(4.0), Some(8.0)));
        // Live /gui_set: numbers set, a non-number releases the prop.
        let mut place = child.place;
        assert!(place.apply("h", &serde_json::json!(20)));
        assert_eq!(place.h, Some(20.0));
        assert!(place.apply("w", &serde_json::json!("auto")));
        assert_eq!(place.w, None, "a non-number releases a place prop");
        assert!(
            !place.apply("min", &serde_json::json!(1)),
            "not a place key"
        );
    }

    #[test]
    fn flow_props_parse_and_apply() {
        let n = node(r#"{"type":"window","flow":"grid","margin":0,"gap":2,"cols":3}"#);
        let w = Widget::from_node(1, &n, &[]).unwrap();
        let WidgetKind::Window { mut flow, .. } = w.kind else {
            unreachable!()
        };
        assert_eq!(
            (flow.margin, flow.gap, flow.cols),
            (Some(0.0), Some(2.0), Some(3))
        );
        assert!(flow.apply("cols", &serde_json::json!(0)));
        assert_eq!(flow.cols, Some(1), "cols clamps to at least 1");
        // The container arm of the kind apply routes layout and flow keys.
        let mut kind = WidgetKind::Panel {
            layout: Layout::Col,
            flow: Flow::default(),
        };
        assert!(kind.apply("flow", &serde_json::json!("row")));
        assert!(kind.apply("gap", &serde_json::json!(10)));
        assert!(matches!(
            kind,
            WidgetKind::Panel {
                layout: Layout::Row,
                flow: Flow { gap: Some(g), .. }
            } if g == 10.0
        ));
    }

    #[test]
    fn defaults_and_unknown_type() {
        // An unrecognized type is laid out but kept as `Unknown`, never
        // rejected (the protocol's forward-compatibility rule).
        let n = node(r#"{"type":"window","children":[{"id":7,"type":"no_such_widget"}]}"#);
        let w = Widget::from_node(1, &n, &[]).unwrap();
        // Window size defaults when w/h are omitted.
        match w.kind {
            WidgetKind::Window {
                width,
                height,
                layout,
                ..
            } => {
                assert_eq!((width, height), DEFAULT_WINDOW);
                assert_eq!(layout, Layout::Col);
            }
            _ => unreachable!(),
        }
        match &w.children[0].kind {
            WidgetKind::Unknown(t) => assert_eq!(t, "no_such_widget"),
            other => panic!("expected unknown, got {other:?}"),
        }
    }

    #[test]
    fn bad_blob_index_is_an_error() {
        let n = node(
            r#"{"type":"window","children":[{"id":2,"type":"signal","view":"trace","blob":3}]}"#,
        );
        assert!(Widget::from_node(1, &n, &[]).is_err());
    }

    #[test]
    fn parses_controls_and_clamps_value() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"slider","min":20.0,"max":2000.0,"value":5000.0,"label":"cut"},
                {"id":2,"type":"toggle","value":1},
                {"id":3,"type":"menu","options":["a","b","c"],"index":1}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Slider { range: r, .. } => {
                assert_eq!(r.value, 2000.0, "value clamps into the range");
                assert_eq!(r.label.as_deref(), Some("cut"));
                assert_eq!(r.fraction(), 1.0);
            }
            other => panic!("expected slider, got {other:?}"),
        }
        assert!(matches!(
            w.children[1].kind,
            WidgetKind::Toggle { value: true, .. }
        ));
        assert!(matches!(
            &w.children[2].kind,
            WidgetKind::Menu { index: 1, .. }
        ));
    }

    #[test]
    fn slider_orientation_parses() {
        let n = GuiNode::parse(br#"{"type":"slider","vertical":true}"#).unwrap();
        let w = Widget::from_node(7, &n, &[]).unwrap();
        assert!(matches!(w.kind, WidgetKind::Slider { vertical: true, .. }));
        // Default (no `vertical`) is horizontal.
        let h = GuiNode::parse(br#"{"type":"slider"}"#).unwrap();
        let wh = Widget::from_node(8, &h, &[]).unwrap();
        assert!(matches!(
            wh.kind,
            WidgetKind::Slider {
                vertical: false,
                ..
            }
        ));
    }

    #[test]
    fn apply_updates_value_and_event_value_reports_it() {
        let n =
            node(r#"{"type":"window","children":[{"id":5,"type":"knob","min":0.0,"max":10.0}]}"#);
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let knob = w.find_mut(5).unwrap();
        assert!(knob.kind.apply("value", &Value::from(4.0)));
        assert_eq!(knob.kind.event_value(), Some(OscType::Float(4.0)));
        // An unknown key is a no-op.
        assert!(!knob.kind.apply("nonesuch", &Value::from(1.0)));
    }

    #[test]
    fn piano_parses_defaults_and_normalizes_the_range() {
        let n = node(r#"{"type":"window","children":[{"id":6,"type":"keys"}]}"#);
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Piano {
                min,
                max,
                active_min,
                active_max,
                pan,
                overview,
                velocity,
                channel,
                voice,
                voice_args,
                pressed,
                ..
            } => {
                assert_eq!((*min, *max), (36, 96));
                assert_eq!((*active_min, *active_max), (0, 127));
                assert!(*pan && *overview);
                assert_eq!(*velocity, None); // dynamic (press-height) velocity
                assert_eq!(*channel, 0);
                assert!(voice.is_none() && voice_args.is_empty());
                assert!(pressed.is_empty());
            }
            other => panic!("expected piano, got {other:?}"),
        }
        // A black-key min snaps down to its white key; voice props parse.
        let n = node(
            r#"{"type":"window","children":[{"id":6,"type":"keys","min":61,"max":85,
                "velocity":90,"channel":3,"voice":"pv","voice_args":["pan",0.5]}]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Piano {
                min,
                velocity,
                channel,
                voice,
                voice_args,
                ..
            } => {
                assert_eq!(*min, 60);
                assert_eq!(*velocity, Some(90));
                assert_eq!(*channel, 3);
                assert_eq!(voice.as_deref(), Some("pv"));
                assert_eq!(voice_args, &[("pan".to_string(), 0.5f32)]);
            }
            other => panic!("expected piano, got {other:?}"),
        }
    }

    /// The wire has not moved: a script still sets a **body's** prop on the
    /// clip, so the set has to reach the child that owns it — and build that
    /// child when the clip does not have it yet, which is how an envelope is
    /// drawn over a take without rebuilding the def.
    #[test]
    fn a_clip_routes_a_body_prop_into_the_body_that_owns_it() {
        let is_take = |k: &WidgetKind| matches!(k, WidgetKind::Signal(_));
        let is_roll = |k: &WidgetKind| matches!(k, WidgetKind::PianoRoll { .. });
        let is_curve = |k: &WidgetKind| matches!(k, WidgetKind::Bpf { .. });

        let n = node(
            r#"{"type":"window","children":[{"id":1,"type":"field","children":[
                {"id":10,"type":"field","offset":0.0,"dur":400.0,"data":[0.0,1.0]}]}]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let clip = w.find_mut(10).unwrap();
        assert_eq!(clip.children.len(), 1, "built with a take and nothing else");

        // A body prop for a body it does not have **grows** that body...
        assert!(apply_widget(
            clip,
            "points",
            &serde_json::json!([0.0, 0.0, 1, 0.0, 400.0, 1.0, 1, 0.0])
        ));
        let clip = w.find_mut(10).unwrap();
        assert!(matches!(
            clip.clip_body(is_curve),
            Some(WidgetKind::Bpf { points, .. }) if points.len() == 2
        ));
        // ...in layering order: the curve draws over the take, not under it.
        assert!(is_take(&clip.children[0].kind));
        assert!(is_curve(&clip.children[1].kind));

        // The same for notes, which land between the two.
        assert!(apply_widget(
            clip,
            "notes",
            &serde_json::json!([0.0, 100.0, 60.0])
        ));
        let clip = w.find_mut(10).unwrap();
        assert_eq!(clip.children.len(), 3);
        assert!(is_roll(&clip.children[1].kind));

        // The curve's own range goes to the curve; `min`/`max` reach the bodies
        // that measure with them and leave the curve alone.
        assert!(apply_widget(clip, "points_min", &serde_json::json!(150.0)));
        assert!(apply_widget(clip, "min", &serde_json::json!(48.0)));
        let clip = w.find_mut(10).unwrap();
        assert!(
            matches!(clip.clip_body(is_curve), Some(WidgetKind::Bpf { min, .. }) if *min == 150.0)
        );
        assert!(
            matches!(clip.clip_body(is_roll), Some(WidgetKind::PianoRoll { min, .. }) if *min == 48.0)
        );
        assert_eq!(
            clip.signal_target().unwrap().value.min,
            Some(48.0),
            "the take measures with min/max too"
        );

        // The clip's own props still land on the clip, and an unknown one is
        // still refused.
        assert!(apply_widget(clip, "offset", &serde_json::json!(96.0)));
        assert!(matches!(clip.kind, WidgetKind::Clip { offset, .. } if offset == 96.0));
        assert!(!apply_widget(clip, "sideways", &serde_json::json!(1)));
    }

    /// Each body keeps **its own** value axis, and takes its default from what
    /// it measures. A pitch axis defaulting to amplitude would clamp every note
    /// to the clip's top edge — a roll drawn as a solid band, with nothing
    /// saying why — and an envelope's units are not the pitches under it.
    #[test]
    fn each_clip_body_keeps_its_own_value_axis() {
        let axis_of = |json: &str, is: fn(&WidgetKind) -> bool| {
            let w = Widget::from_node(1, &node(json), &[]).unwrap();
            match w.clip_body(is) {
                Some(WidgetKind::PianoRoll { min, max, .. })
                | Some(WidgetKind::Bpf { min, max, .. }) => (*min, *max),
                Some(WidgetKind::Signal(el)) => el.value.resolved(0.0, 0.0),
                other => panic!("no such body: {other:?}"),
            }
        };
        let is_roll = |k: &WidgetKind| matches!(k, WidgetKind::PianoRoll { .. });
        let is_curve = |k: &WidgetKind| matches!(k, WidgetKind::Bpf { .. });
        let is_take = |k: &WidgetKind| matches!(k, WidgetKind::Signal(_));

        // A roll's axis is pitch; a take's is amplitude.
        assert_eq!(
            axis_of(
                r#"{"type":"field","dur":100.0,"notes":[0.0,10.0,60.0]}"#,
                is_roll
            ),
            (21.0, 108.0)
        );
        assert_eq!(
            axis_of(r#"{"type":"field","dur":100.0,"buffer":3}"#, is_take),
            (-1.0, 1.0)
        );
        // An explicit `min`/`max` wins, and reaches every body that measures
        // with it.
        let named = r#"{"type":"field","dur":100.0,"notes":[0.0,10.0,60.0],
                        "buffer":3,"min":48.0,"max":72.0}"#;
        assert_eq!(axis_of(named, is_roll), (48.0, 72.0));
        assert_eq!(axis_of(named, is_take), (48.0, 72.0));
        // The curve's own range is untouched by either: a layered clip's bodies
        // do not share an axis.
        let layered = r#"{"type":"field","dur":100.0,"notes":[0.0,10.0,60.0],
                          "points":[0.0,0.5,1,0.0],"points_min":0.0,"points_max":1.0}"#;
        assert_eq!(axis_of(layered, is_roll), (21.0, 108.0));
        assert_eq!(axis_of(layered, is_curve), (0.0, 1.0));
    }

    #[test]
    fn piano_apply_round_trips_and_prunes_held_keys() {
        let n = node(r#"{"type":"window","children":[{"id":6,"type":"keys","min":48,"max":84}]}"#);
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        let p = w.find_mut(6).unwrap();
        if let WidgetKind::Piano { pressed, .. } = &mut p.kind {
            pressed.extend([50, 80]);
        }
        // A narrowed range white-snaps its min and drops held keys outside it.
        assert!(p.kind.apply("min", &Value::from(61)));
        assert!(p.kind.apply("max", &Value::from(72)));
        match &p.kind {
            WidgetKind::Piano {
                min, max, pressed, ..
            } => {
                assert_eq!((*min, *max), (60, 72));
                assert!(pressed.is_empty());
            }
            other => panic!("expected piano, got {other:?}"),
        }
        // A negative velocity restores the dynamic map; an empty voice unsets.
        assert!(p.kind.apply("velocity", &Value::from(100)));
        assert!(p.kind.apply("velocity", &Value::from(-1)));
        assert!(p.kind.apply("voice", &Value::from("pv")));
        assert!(p.kind.apply("voice", &Value::from("")));
        assert!(p.kind.apply("pan", &Value::from(0)));
        assert!(p.kind.apply("active_min", &Value::from(40)));
        // `voice_args` rides as the JSON-string scalar carrier, like `notes`.
        assert!(p.kind.apply("voice_args", &Value::from("[\"pan\",0.25]")));
        match &p.kind {
            WidgetKind::Piano {
                velocity,
                voice,
                voice_args,
                pan,
                active_min,
                ..
            } => {
                assert_eq!(*velocity, None);
                assert!(voice.is_none());
                assert_eq!(voice_args, &[("pan".to_string(), 0.25f32)]);
                assert!(!*pan);
                assert_eq!(*active_min, 40);
            }
            other => panic!("expected piano, got {other:?}"),
        }
    }
    /// A free-standing ruler answers for its own chrome. Its unit is the one
    /// thing a script changes at run time — a transport read-out switching
    /// between bars, seconds and samples wants the strip to follow — and it
    /// used to be recorded and dropped.
    #[test]
    fn a_free_standing_ruler_changes_its_unit_live() {
        let mut w = Widget::from_node(
            1,
            &node(r#"{"id":9,"type":"field","h":22,"ruler":"beats"}"#),
            &[],
        )
        .unwrap();
        let unit = |w: &Widget| match &w.kind {
            WidgetKind::TimeRuler { editor } => editor.ruler,
            other => panic!("not a ruler: {other:?}"),
        };
        assert_eq!(unit(&w), Ruler::Beats);
        assert!(apply_widget(&mut w, "ruler", &serde_json::json!("samples")));
        assert_eq!(unit(&w), Ruler::Samples);
        assert!(apply_widget(&mut w, "ruler", &serde_json::json!("time")));
        assert_eq!(unit(&w), Ruler::Time);
    }
}
