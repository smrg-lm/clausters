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
//! exposing them. Per-widget *behavior* (drawing, hit-testing, editing) is not
//! here at all — it lives in each widget's own module (`bpf`, `pianoroll`,
//! `track`, `patch`, `textedit`, …); this module owns only the typed data and
//! its wire mapping.

use std::path::PathBuf;
use std::sync::Arc;

use clausters_core::osc::OscType;
use serde_json::Value;

use crate::spectrogram::FreqScale;
use crate::waveform::WaveformData;

use super::canvas;
use super::guidef::GuiNode;
// Sibling widget modules the wire matches reach via `super::` — re-imported here
// so the `build`/`apply` child modules resolve the same paths (a descendant sees
// the parent's private `use` items).
use super::{bpf, piano, plot, score, textedit};

mod apply;
mod build;

/// How a container arranges its children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Row,
    Col,
    Grid,
    Free,
}

impl Layout {
    /// Parses the `layout` property; defaults to `Col`.
    fn parse(props: &serde_json::Map<String, Value>) -> Layout {
        props
            .get("layout")
            .and_then(Value::as_str)
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
        match props.get("ruler").and_then(Value::as_str) {
            Some("samples") => Ruler::Samples,
            Some("beats") => Ruler::Beats,
            Some("off") | Some("none") => Ruler::Off,
            _ => Ruler::Time,
        }
    }

    fn set(&mut self, v: &Value) -> bool {
        match v.as_str() {
            Some("samples") => *self = Ruler::Samples,
            Some("beats") => *self = Ruler::Beats,
            Some("off") | Some("none") => *self = Ruler::Off,
            Some("time") => *self = Ruler::Time,
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
        match props.get("ruler_y").and_then(Value::as_str) {
            Some(s) => Self::from_str(s).unwrap_or(default),
            None => default,
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
/// all of them. Without a `link` the widget navigates alone. The selection
/// and playhead fields here are the group's mirrored copy (the group is the
/// single writer once the widget is live); only the y axis stays per-widget.
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
    pub y_start: f64,
    pub y_len: f64,
    pub link: Option<i32>,
    pub offset: f64,
}

impl EditorProps {
    /// Parses the shared chrome; `default_y` is the view's own default
    /// vertical unit (`Norm` for the waveform, `Hz` for the spectrogram).
    fn parse(props: &serde_json::Map<String, Value>, default_y: RulerY) -> EditorProps {
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
        if self.y_len <= 0.0 {
            (0.0, 1.0)
        } else {
            crate::viewport::clamp_span(self.y_start, self.y_len)
        }
    }

    /// The selection as `(start, len)` in samples, if one is active.
    pub fn selection(&self) -> Option<(f64, f64)> {
        (self.sel_len > 0.0).then_some((self.sel_start, self.sel_len))
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
    /// Device pixels per content unit (uniform on both axes), > 0.
    pub view_zoom: f64,
}

impl ScrollView {
    fn parse(props: &serde_json::Map<String, Value>) -> ScrollView {
        let f = |k: &str| props.get(k).and_then(Value::as_f64).map(|v| v as f32);
        ScrollView {
            axis: Axis::parse(props),
            zoom_enabled: props.get("zoom").and_then(truthy).unwrap_or(true),
            content_w: f("content_w"),
            content_h: f("content_h"),
            view_x: number_f64(props, "view_x", 0.0),
            view_y: number_f64(props, "view_y", 0.0),
            view_zoom: super::scroll::clamp_zoom(number_f64(props, "view_zoom", 1.0)),
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
            "view_zoom" => v
                .as_f64()
                .map(|n| self.view_zoom = super::scroll::clamp_zoom(n))
                .is_some(),
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
    /// The heavy waveform view: its samples and the peak-pyramid bucket size.
    /// The samples reach the view one of several ways, in precedence order:
    /// `cache` (a prebuilt peak-pyramid file the host maps — the most compact
    /// bulk path, raw samples never loaded), `path` (a file of raw little-endian
    /// `f32` the host maps — the bulk path for a multi-megabyte buffer, no OSC),
    /// `buffer` (an audio-server buffer number the windowed front fetches over
    /// the client leg), or inline `data`/`blob`. `channels` is the interleaved
    /// channel count of a multi-channel `path`/`data`/`blob` (default 1) —
    /// **every** channel is kept and drawn, as stacked lanes sharing the time
    /// axis by default or as `overlay` per-color traces. For `cache`/`path`/
    /// `buffer`, `samples` starts empty and is filled when the resource is
    /// mapped/fetched. `editor` carries the ruler/selection/playhead chrome.
    Waveform {
        samples: Arc<[f32]>,
        base_bucket: usize,
        buffer: Option<i32>,
        path: Option<PathBuf>,
        cache: Option<PathBuf>,
        channels: usize,
        overlay: bool,
        editor: EditorProps,
    },
    /// The heavy STFT time-frequency view, host-wired like the waveform: its
    /// samples come from a mapped `path` (raw interleaved `f32`), a prebuilt
    /// single-channel `cache` (an `Stft` cache file), a server `buffer`, or
    /// inline `data`/`blob`; `channels` de-interleaves them and each channel
    /// gets its own analysis, drawn as stacked lanes. `window_size` (a
    /// supported power of two) and `hop` shape the analysis (recompute-time
    /// props, fixed at def time); the dB window, frequency scale
    /// (`freq_scale`: linear/log/mel/bark; `log_freq` is the legacy boolean
    /// alias) and colormap are live shader uniforms (`/gui_set`).
    /// `sample_rate` places the frequency axis for `path`/inline sources (a
    /// fetched `buffer` brings its own). `editor` adds the time ruler,
    /// selection and playhead; the Hz ruler rides the left strip when
    /// `ruler_y` is not off, its ticks placed by the active `freq_scale`.
    Spectrogram {
        samples: Arc<[f32]>,
        channels: usize,
        buffer: Option<i32>,
        path: Option<PathBuf>,
        cache: Option<PathBuf>,
        window_size: usize,
        hop: usize,
        sample_rate: f64,
        db_floor: f32,
        db_ceil: f32,
        freq_scale: FreqScale,
        colormap: i32,
        editor: EditorProps,
    },
    /// A level meter reading control bus `bus` from the shared-memory segment
    /// each frame (zero messages), shown as a bar over `[min, max]`.
    Meter {
        bus: i32,
        min: f32,
        max: f32,
        label: Option<String>,
    },
    /// A time-domain scope over `[min, max]`, in one of two rates:
    /// control-rate (`tap < 0`, the default) plots the rolling history of
    /// control bus `bus`, one sample per frame tick; audio-rate (`tap >= 0`,
    /// set by a `tap` prop or `rate: "audio"`) is a real oscilloscope — a
    /// `window_ms` window of `channels` **adjacent** segment tap rings
    /// starting at `tap` (see the server's `/tap`), re-read every frame and
    /// aligned on a rising crossing of `trigger` found in the **first**
    /// channel, so the channels keep their true relative phase (free-running
    /// when no crossing is found; a lock/free read-out shows which). Channels
    /// draw as stacked lanes, or as color-coded traces in one field with
    /// `overlay`; `hold` freezes the trace. `ruler`/`ruler_y` (`"off"` to
    /// hide) are the audio-rate form's axis strips: x in milliseconds of the
    /// window, y in signal value over `[min, max]`.
    Scope {
        bus: i32,
        tap: i32,
        channels: usize,
        overlay: bool,
        window_ms: f32,
        trigger: f32,
        hold: bool,
        min: f32,
        max: f32,
        ruler: bool,
        ruler_y: bool,
        label: Option<String>,
    },
    /// A phase/goniometer view of a stereo pair of audio taps, drawn as the
    /// 45°-rotated Lissajous figure (vertical = mid `(L+R)/√2`, horizontal =
    /// side `(L−R)/√2`): mono reads vertical, anti-phase horizontal. `tap` is
    /// the left channel's ring, `tap2` the right; `window_ms` sizes the
    /// age-faded persistence trail; `hold` freezes it. A correlation read-out
    /// (Pearson r over the window) sits under the field.
    Phasescope {
        tap: i32,
        tap2: i32,
        window_ms: f32,
        hold: bool,
        label: Option<String>,
    },
    /// A live FFT magnitude curve (spectroscope) over `channels` **adjacent**
    /// audio taps starting at `tap`: one forward FFT per channel per frame of
    /// the newest `fft_size` window, magnitudes in dB over
    /// `[db_floor, db_ceil]`, the frequency axis on `freq_scale`
    /// (linear/log/mel/bark; `log_freq` is the legacy boolean alias). The
    /// channels overlay as color-coded curves in one field. `averaging`
    /// (0..1) exponentially smooths each bin so the curve does not flicker;
    /// `peak_hold` overlays a slowly decaying peak trace per channel.
    /// `ruler`/`ruler_y` (`"off"` to hide) are the axis strips: x in hertz on
    /// the active scale, y in dB. The analysis reuses the shared-core FFT +
    /// Hann window, so it agrees with the spectrogram.
    Spectrum {
        tap: i32,
        channels: usize,
        fft_size: usize,
        db_floor: f32,
        db_ceil: f32,
        freq_scale: FreqScale,
        averaging: f32,
        peak_hold: bool,
        ruler: bool,
        ruler_y: bool,
        label: Option<String>,
    },
    /// A live text view of the audio server's node tree rooted at `group`,
    /// queried over the client leg (`/g_queryTree`) and refreshed on node
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
    /// The static plot of a signal — measurement without navigation. Its
    /// samples arrive inline (`data`/`blob`) or — the bulk path for an NRT
    /// render's output — from a mapped local `path` of raw little-endian
    /// `f32`, filled when the host maps it; `channels` de-interleaves them and
    /// **every** channel is drawn (stacked lanes, or `overlay` per-color
    /// traces). `view` picks the presentation ([`super::plot::PlotView`], an
    /// extensible enum): `signal` (value against time/index, decimated to the
    /// pixel width so the whole sequence shows without visual aliasing) or
    /// `spectrum` (the averaged magnitude spectrum in dB over `freq_scale` —
    /// linear/log/mel/bark — analyzed once into `spectrum` at the widget's
    /// mutation points, never per frame). `min`/`max` bound the signal view's
    /// value axis; either side omitted (`None`) auto-fits to the data — the
    /// arbitrary-range sequence case. `ruler`/`ruler_y` switch the x/y ruler
    /// strips; `sample_rate` (0 = unknown) turns the x axis from sample counts
    /// into clock time and places the spectral frequency axis. Hovering names
    /// the exact sample (or bin) under the cursor. Unlike the heavy
    /// `waveform`, it does not zoom, pan or edit.
    Plot {
        samples: Arc<[f32]>,
        path: Option<PathBuf>,
        channels: usize,
        view: super::plot::PlotView,
        overlay: bool,
        sample_rate: f64,
        min: Option<f32>,
        max: Option<f32>,
        ruler: Ruler,
        ruler_y: bool,
        fft_size: usize,
        db_floor: f32,
        db_ceil: f32,
        freq_scale: FreqScale,
        /// The cached spectral analysis (spectrum view; recomputed by
        /// [`WidgetKind::refresh_plot_analysis`] whenever its inputs change).
        spectrum: Option<Arc<super::plot::PlotSpectrum>>,
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
    /// chrome, natively and in the browser. Read-only for now: the MEI xml:id
    /// travels on each primitive for a later interactive/edit-back pass.
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
        editor: EditorProps,
    },
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
    /// per held key: `/s_new <voice> … freq amp gate 1 <voice_args…>` on
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
        /// Extra `/s_new` control pairs appended after `freq`/`amp`/`gate`.
        voice_args: Vec<(String, f32)>,
        /// The held keys — native view state, never parsed from the wire: the
        /// press/glissando/release gestures build it, the drawing reads it.
        pressed: Vec<i32>,
        label: Option<String>,
    },
    /// One clip on a `track`: a placed rectangle spanning `[offset, offset +
    /// dur]` in timeline sample units (the graphic unit — length = duration),
    /// with a `label`. Its body is one of two: a **waveform**, or — when `notes`
    /// is non-empty — a **piano-roll** of note events (`start`/`dur` relative to
    /// the clip, `pitch` over `[min, max]`), the events-track scalar-vertical
    /// view. Interaction (drag to move `offset`, drag an edge to resize `dur`)
    /// writes back through the edit-back path. A leaf.
    ///
    /// The waveform body reaches the clip the same ways the heavy [`Waveform`]
    /// view's samples do, in the same precedence order — a real take is
    /// minutes long, so it must never ride the wire as JSON: `cache` (a prebuilt
    /// peak-pyramid file, raw samples never loaded), `path` (a file of raw
    /// little-endian `f32` the host maps — the bulk path, no OSC), `buffer` (a
    /// server buffer, fetched over the client leg), or inline `data`/`blob` for a
    /// short body. A loaded body lands in `body` as the shared [`WaveformData`],
    /// whose peak pyramid (the core's, the one every client builds) decimates it
    /// to the clip's pixel width — the same "never resolve finer than the screen"
    /// rule the editor views follow, with no GPU slot: a lane body is flat
    /// geometry, the static-view posture.
    ///
    /// [`Waveform`]: WidgetKind::Waveform
    Clip {
        offset: f64,
        dur: f64,
        samples: Arc<[f32]>,
        body: Option<Arc<WaveformData>>,
        buffer: Option<i32>,
        path: Option<PathBuf>,
        cache: Option<PathBuf>,
        channels: usize,
        base_bucket: usize,
        notes: Vec<super::track::Note>,
        /// An **automation** clip: break-points over the clip's span (times in
        /// timeline units relative to its `offset`, values over `[min, max]`),
        /// drawn as the curve body and editable in place — the `bpf` editor's
        /// model and shape math, placed on the multitrack. Takes precedence over
        /// `notes` and the waveform body.
        points: Vec<super::bpf::BpfPoint>,
        /// An exponential display scale for the curve body's value axis (a
        /// frequency-like range), as on the `bpf` view.
        exp: bool,
        /// The curve body's own value range. A clip may **layer** its bodies (an
        /// envelope drawn over the event it shapes), and they do not share an
        /// axis — a piano-roll's `min`/`max` are pitches, a curve's are its
        /// parameter's units — so the curve keeps its own. Defaults to
        /// `min`/`max`.
        points_min: f32,
        points_max: f32,
        min: f32,
        max: f32,
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

    /// The value as a 0..1 fraction of the range (for rendering).
    pub fn fraction(&self) -> f32 {
        if (self.max - self.min).abs() < f32::EPSILON {
            0.0
        } else {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }

    /// Sets the value from a 0..1 fraction of the range (for interaction).
    pub fn set_fraction(&mut self, t: f32) {
        self.value = self.min + t.clamp(0.0, 1.0) * (self.max - self.min);
    }
}

/// The default window size when a GuiDef omits `w`/`h`.
const DEFAULT_WINDOW: (u32, u32) = (640, 360);
/// The default peak-pyramid bucket for an inline waveform.
const DEFAULT_BASE_BUCKET: usize = 256;

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
    /// The resolved theme this widget draws with, produced at mutation points
    /// by [`resolve_themes`] (an [`Arc`] clone per widget, so the per-frame
    /// path reads exactly one theme and pays nothing). `None` until the first
    /// resolve — the renderer falls back to the host theme.
    pub theme: Option<Arc<super::theme::Theme>>,
    pub children: Vec<Widget>,
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

    /// Links every un-linked `track` of a window into one navigation group keyed
    /// by the window root. The multitrack's promise is **one shared time axis**
    /// (aligned lanes), and a navigation group is exactly that — so the lanes of
    /// a window navigate as one by default, zooming and panning together, and
    /// only an explicit `link` splits them (or joins lanes across windows).
    fn link_lanes(widget: &mut Widget, root_id: i32) {
        if let WidgetKind::Track { editor, .. } = &mut widget.kind
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
        let kind = build::build_kind(id, node, blobs)?;
        // Only containers carry children into the typed tree; a leaf's children
        // (if any) are ignored. A `track` carries its clips.
        let children = match kind {
            WidgetKind::Window { .. }
            | WidgetKind::Panel { .. }
            | WidgetKind::Scroll { .. }
            | WidgetKind::Track { .. } => node
                .children
                .iter()
                .map(|c| Self::build(None, c, blobs))
                .collect::<Result<Vec<_>, _>>()?,
            _ => Vec::new(),
        };
        Ok(Widget {
            id,
            kind,
            place: Place::parse(&node.props),
            theme_over: node.props.get("theme").and_then(Value::as_object).cloned(),
            color: node
                .props
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

    /// Whether this is the heavy waveform view (a convenience for the renderer).
    pub fn is_waveform(&self) -> bool {
        matches!(self.kind, WidgetKind::Waveform { .. })
    }

    /// Whether this is one of the navigable timeline views (waveform or
    /// spectrogram) — the widgets that zoom, pan, select and show a playhead.
    pub fn is_timeline(&self) -> bool {
        matches!(
            self.kind,
            WidgetKind::Waveform { .. }
                | WidgetKind::Spectrogram { .. }
                | WidgetKind::Track { .. }
                | WidgetKind::PianoRoll { .. }
        )
    }

    /// Whether this tree contains a widget whose overlay follows the pointer
    /// — the cursor readout of the timeline views and the plot. The windowed
    /// front asks on cursor motion: such a window needs a frame per move
    /// (a fully static one, like a plot's, has no other frame source).
    pub fn has_hover_readout(&self) -> bool {
        self.is_timeline()
            || matches!(self.kind, WidgetKind::Plot { .. })
            || self.children.iter().any(Widget::has_hover_readout)
    }

    /// The widget with id `id` anywhere in this tree.
    pub fn find(&self, id: i32) -> Option<&Widget> {
        if self.id == Some(id) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
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

    /// The control bus a live (shared-memory-backed) widget reads each frame, if
    /// this is one. The windowed front uses it to know which windows to animate
    /// and which bus to sample. An audio-rate scope reads a tap, not a bus.
    pub fn live_bus(&self) -> Option<i32> {
        match self {
            WidgetKind::Meter { bus, .. } => Some(*bus),
            WidgetKind::Scope { bus, tap, .. } if *tap < 0 => Some(*bus),
            _ => None,
        }
    }

    /// The first audio-tap ring an audio-rate scope reads each frame, if this
    /// is one.
    pub fn live_tap(&self) -> Option<i32> {
        match self {
            WidgetKind::Scope { tap, .. } if *tap >= 0 => Some(*tap),
            _ => None,
        }
    }

    /// Appends every audio-tap ring this widget reads each frame — `channels`
    /// adjacent rings for an audio-rate `scope` or a `spectrum`, two (left and
    /// right) for a `phasescope`. Drives the tap subscription/animation set,
    /// so all three tap consumers are covered uniformly.
    pub fn taps_read(&self, out: &mut Vec<i32>) {
        match self {
            WidgetKind::Scope { tap, channels, .. } if *tap >= 0 => {
                out.extend((0..*channels as i32).map(|k| *tap + k));
            }
            WidgetKind::Spectrum { tap, channels, .. } if *tap >= 0 => {
                out.extend((0..*channels as i32).map(|k| *tap + k));
            }
            WidgetKind::Phasescope { tap, tap2, .. } => {
                if *tap >= 0 {
                    out.push(*tap);
                }
                if *tap2 >= 0 {
                    out.push(*tap2);
                }
            }
            _ => {}
        }
    }

    /// The editor chrome of a view that carries one — a timeline view
    /// (waveform/spectrogram) or a `track` lane, which reuses the same props for
    /// its ruler and playhead. The shared read path for the frame renderer and
    /// the fronts. (Group membership is `is_timeline`, not this: a lane has the
    /// chrome but navigates with the window's clip span.)
    pub fn editor(&self) -> Option<&EditorProps> {
        match self {
            WidgetKind::Waveform { editor, .. }
            | WidgetKind::Spectrogram { editor, .. }
            | WidgetKind::Track { editor, .. }
            | WidgetKind::PianoRoll { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// Mutable access to a view's editor chrome (the selection drag writes
    /// through here).
    pub fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        match self {
            WidgetKind::Waveform { editor, .. }
            | WidgetKind::Spectrogram { editor, .. }
            | WidgetKind::Track { editor, .. }
            | WidgetKind::PianoRoll { editor, .. } => Some(editor),
            _ => None,
        }
    }

    /// Whether this widget shows a live playhead (so its window must animate:
    /// the line tracks the engine sample clock every frame).
    pub fn has_playhead(&self) -> bool {
        self.editor().is_some_and(|e| e.playhead_at >= 0.0)
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
    /// Recomputes a `plot`'s cached spectral analysis from its current samples
    /// and props — a no-op for every other widget, for the signal view and for
    /// empty samples. Called at the widget's mutation points (parse, a bulk
    /// load landing samples, a live `/gui_set` touching what the analysis
    /// reads), which keeps the per-frame render pure and allocation-light.
    pub fn refresh_plot_analysis(&mut self) {
        if let WidgetKind::Plot {
            samples,
            channels,
            view,
            sample_rate,
            fft_size,
            spectrum,
            ..
        } = self
        {
            *spectrum =
                (*view == super::plot::PlotView::Spectrum && !samples.is_empty()).then(|| {
                    Arc::new(super::plot::analyze(
                        samples,
                        *channels,
                        *fft_size,
                        *sample_rate,
                    ))
                });
        }
    }

    pub fn apply(&mut self, key: &str, v: &Value) -> bool {
        apply::apply_kind(self, key, v)
    }
}

/// Coerce a `/gui_set` value that carries an array (either already a JSON array,
/// or an array encoded as a JSON string — the scalar-wire carrier `points`/
/// `notes`/`members` use) into a one-entry props map under `key`, for the
/// `parse_*` helpers to read.
fn as_array_props(key: &str, v: &Value) -> serde_json::Map<String, Value> {
    let value = match v {
        Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or(Value::Null),
        other => other.clone(),
    };
    std::iter::once((key.to_string(), value)).collect()
}

/// Parses a piano-roll clip's `notes`: a flat `[start, dur, pitch, …]` array
/// (three numbers per note, the flat convention the `bpf` points use), each a
/// [`super::track::Note`]. A short/absent/malformed array yields no notes (the
/// clip then draws a waveform body).
fn parse_notes(props: &serde_json::Map<String, Value>) -> Vec<super::track::Note> {
    let Some(Value::Array(items)) = props.get("notes") else {
        return Vec::new();
    };
    // The canonical wire form is quintuples `start dur pitch velocity channel`
    // (what the Python builder always emits): a length that is a multiple of 5
    // is read as quintuples. Anything else is a plain `start dur pitch` triple
    // list (legacy / hand-authored), which still parses, defaulting velocity to
    // 100 on channel 0. A trailing partial group is dropped.
    let stride = if items.len() % 5 == 0 { 5 } else { 3 };
    items
        .chunks_exact(stride)
        .filter_map(|c| {
            let mut n = super::track::Note::new(
                c[0].as_f64()?.max(0.0),
                c[1].as_f64()?.max(0.0),
                c[2].as_f64()? as f32,
            );
            if stride == 5 {
                n.velocity = c[3].as_i64().unwrap_or(100) as i32;
                n.channel = c[4].as_i64().unwrap_or(0) as i32;
            }
            Some(n)
        })
        .collect()
}

/// Parse a `pianoroll`'s `osc` prop — a flat `[time, label, time, label, …]`
/// list of OSC event markers (the label a short address/tag, an empty string
/// meaning none). A trailing partial pair is dropped.
fn parse_osc(props: &serde_json::Map<String, Value>) -> Vec<super::pianoroll::OscMark> {
    let Some(Value::Array(items)) = props.get("osc") else {
        return Vec::new();
    };
    items
        .chunks_exact(2)
        .filter_map(|c| {
            let time = c[0].as_f64()?.max(0.0);
            let label = c[1].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            Some(super::pianoroll::OscMark { time, label })
        })
        .collect()
}

/// Parse a `piano`'s `voice_args` — a flat `[name, value, name, value, …]`
/// list of extra `/s_new` control pairs (the `bind`-prefix posture: names are
/// strings, values numbers). A trailing partial pair is dropped.
fn parse_voice_args(props: &serde_json::Map<String, Value>) -> Vec<(String, f32)> {
    let Some(Value::Array(items)) = props.get("voice_args") else {
        return Vec::new();
    };
    items
        .chunks_exact(2)
        .filter_map(|c| Some((c[0].as_str()?.to_string(), c[1].as_f64()? as f32)))
        .collect()
}

/// Parses a `patch` widget's patch: `members` (each a `name` plus its wired
/// control `ports`), `buses` (names, `OUT` among them) and `wires` (flat triples
/// `[member, control, bus]`). A malformed entry is skipped, so a partial patch
/// still draws.
fn parse_patch(props: &serde_json::Map<String, Value>) -> super::patch::PatchDraw {
    use super::patch::{BoxRole, Cord, Obj, PatchDraw};

    let boxes = props
        .get("boxes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|b| {
                    let role = match b.get("role").and_then(Value::as_str) {
                        Some("source") => BoxRole::Source,
                        Some("const") => BoxRole::Const,
                        _ => BoxRole::Object,
                    };
                    Some(Obj {
                        def: b.get("def")?.as_str()?.to_string(),
                        inlets: parse_ports(b.get("inlets")),
                        outlets: parse_ports(b.get("outlets")),
                        x: b.get("x").and_then(Value::as_f64).map(|n| n as f32),
                        y: b.get("y").and_then(Value::as_f64).map(|n| n as f32),
                        role,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // A cord is a flat `from_box from_outlet to_box to_inlet` quadruple.
    let cords = props
        .get("cords")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .chunks_exact(4)
                .filter_map(|c| {
                    Some(Cord {
                        from: c[0].as_u64()? as usize,
                        from_out: c[1].as_u64()? as usize,
                        to: c[2].as_u64()? as usize,
                        to_in: c[3].as_u64()? as usize,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    PatchDraw { boxes, cords }
}

/// Parses a box's port array: each entry a plain name string (audio, the
/// default) or an object `{"name": …, "rate": "audio"|"control"|"init"}`.
fn parse_ports(v: Option<&Value>) -> Vec<super::patch::Port> {
    use super::patch::Port;
    v.and_then(Value::as_array)
        .map(|ps| {
            ps.iter()
                .filter_map(|p| match p {
                    Value::String(name) => Some(Port::audio(name.clone())),
                    Value::Object(o) => {
                        let name = o.get("name")?.as_str()?.to_string();
                        Some(match o.get("rate").and_then(Value::as_str) {
                            Some("control") => Port::control(name),
                            Some("init") => Port::init(name),
                            _ => Port::audio(name),
                        })
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A ruler-strip toggle property: shown by default, hidden with the string
/// `"off"` (the plot/editor convention) or a falsy value.
fn strip_shown(props: &serde_json::Map<String, Value>, key: &str) -> bool {
    match props.get(key) {
        None => true,
        Some(v) => match v.as_str() {
            Some(s) => s != "off",
            None => truthy(v).unwrap_or(true),
        },
    }
}

/// The `freq_scale` property (`"linear"`/`"log"`/`"mel"`/`"bark"`), falling
/// back to the legacy `log_freq` boolean (default: log).
fn parse_freq_scale(props: &serde_json::Map<String, Value>) -> FreqScale {
    if let Some(s) = props
        .get("freq_scale")
        .and_then(Value::as_str)
        .and_then(freq_scale_from_str)
    {
        return s;
    }
    if props.get("log_freq").and_then(truthy) == Some(false) {
        FreqScale::Linear
    } else {
        FreqScale::Log
    }
}

/// A frequency-scale name as the widget schema spells it.
fn freq_scale_from_str(s: &str) -> Option<FreqScale> {
    Some(match s {
        "linear" | "lin" => FreqScale::Linear,
        "log" => FreqScale::Log,
        "mel" => FreqScale::Mel,
        "bark" => FreqScale::Bark,
        _ => return None,
    })
}

/// A non-negative integer dimension property, defaulted when absent.
fn dimension(props: &serde_json::Map<String, Value>, key: &str, default: u32) -> u32 {
    props
        .get(key)
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, u32::MAX as u64) as u32)
        .unwrap_or(default)
}

/// An integer property, defaulted when absent or non-integer.
fn int_prop(props: &serde_json::Map<String, Value>, key: &str, default: i32) -> i32 {
    props
        .get(key)
        .and_then(Value::as_i64)
        .map(|n| n as i32)
        .unwrap_or(default)
}

/// The `fft_size` property snapped to a supported power-of-two FFT size, or the
/// 2048 default when absent or unsupported (so an out-of-range value degrades to
/// a sane size rather than failing the whole def).
fn fft_size(props: &serde_json::Map<String, Value>) -> usize {
    props
        .get("fft_size")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .filter(|n| clausters_core::fft::supports(*n))
        .unwrap_or(2048)
}

/// An `f64` property, defaulted when absent or non-numeric — for sample
/// positions and clock values, where `f32` would lose sample accuracy on
/// buffers past a few minutes.
fn number_f64(props: &serde_json::Map<String, Value>, key: &str, default: f64) -> f64 {
    props.get(key).and_then(Value::as_f64).unwrap_or(default)
}

/// Sets an `f64` slot from a numeric JSON value, reporting whether it applied.
fn set_f64(slot: &mut f64, v: &Value) -> bool {
    match v.as_f64() {
        Some(x) => {
            *slot = x;
            true
        }
        None => false,
    }
}

/// A float property, defaulted when absent or non-numeric.
fn number(props: &serde_json::Map<String, Value>, key: &str, default: f32) -> f32 {
    props
        .get(key)
        .and_then(Value::as_f64)
        .map(|n| n as f32)
        .unwrap_or(default)
}

/// A fixed-size `[f32; N]` from a JSON array property, taking the first `N`
/// numbers and padding the rest with `default`.
fn f32_array<const N: usize>(
    props: &serde_json::Map<String, Value>,
    key: &str,
    default: f32,
) -> [f32; N] {
    let mut out = [default; N];
    if let Some(Value::Array(items)) = props.get(key) {
        for (slot, v) in out.iter_mut().zip(items) {
            if let Some(x) = v.as_f64() {
                *slot = x as f32;
            }
        }
    }
    out
}

/// A fixed-size `[i32; N]` from a JSON array property, taking the first `N`
/// integers and padding the rest with `default`.
fn i32_array<const N: usize>(
    props: &serde_json::Map<String, Value>,
    key: &str,
    default: i32,
) -> [i32; N] {
    let mut out = [default; N];
    if let Some(Value::Array(items)) = props.get(key) {
        for (slot, v) in out.iter_mut().zip(items) {
            if let Some(n) = v.as_i64() {
                *slot = n as i32;
            }
        }
    }
    out
}

/// The integer suffix of `key` after `prefix` (e.g. `"param2"` -> `2`), if `key`
/// is exactly `prefix` followed by digits.
fn index_suffix(key: &str, prefix: &str) -> Option<usize> {
    key.strip_prefix(prefix).and_then(|s| s.parse().ok())
}

/// The `text_size` property: the glyph scale text draws at (font-pixels per
/// cell pixel over the embedded 5x7 font), clamped to a legible range.
fn text_size(props: &serde_json::Map<String, Value>) -> f32 {
    clamp_text_size(number(props, "text_size", super::font::DEFAULT_SIZE))
}

fn clamp_text_size(s: f32) -> f32 {
    s.clamp(1.0, 16.0)
}

/// Sets a `text_size` slot from a numeric JSON value, clamped.
fn set_size(slot: &mut f32, v: &Value) -> bool {
    match v.as_f64() {
        Some(x) => {
            *slot = clamp_text_size(x as f32);
            true
        }
        None => false,
    }
}

/// The `label` property as an owned string, if present.
fn label(props: &serde_json::Map<String, Value>) -> Option<String> {
    props
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `options` property as a list of strings (for a menu).
fn options(props: &serde_json::Map<String, Value>) -> Vec<String> {
    match props.get("options") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// A JSON value as a boolean: real bool, or a number where non-zero is true.
fn truthy(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|x| x != 0.0),
        _ => None,
    }
}

/// Sets `slot` from a numeric JSON value, reporting whether it applied.
fn set_f(slot: &mut f32, v: &Value) -> bool {
    match v.as_f64() {
        Some(x) => {
            *slot = x as f32;
            true
        }
        None => false,
    }
}

/// Sets a ruler-strip toggle: the string `"off"` (or a falsy value) hides it,
/// anything else shows it — the live counterpart of `strip_shown`.
fn set_strip(slot: &mut bool, v: &Value) -> bool {
    match v.as_str() {
        Some(s) => {
            *slot = s != "off";
            true
        }
        None => truthy(v).map(|b| *slot = b).is_some(),
    }
}

/// An optional f32 prop: `None` when absent (the plot's auto-fit sides).
fn opt_number(props: &serde_json::Map<String, Value>, key: &str) -> Option<f32> {
    props.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

/// Sets an optional f32 from a number, or clears it from the string `"auto"`.
fn set_opt_f(slot: &mut Option<f32>, v: &Value) -> bool {
    if v.as_str() == Some("auto") {
        *slot = None;
        return true;
    }
    match v.as_f64() {
        Some(n) => {
            *slot = Some(n as f32);
            true
        }
        None => false,
    }
}

/// The plot's default spectral analysis size.
const DEFAULT_PLOT_FFT: usize = 2048;

/// Clamps a requested analysis size to a supported FFT size.
fn valid_fft_size(n: u64) -> usize {
    let n = n as usize;
    if clausters_core::fft::supports(n) {
        n
    } else {
        DEFAULT_PLOT_FFT
    }
}

/// Sets an optional label from a string JSON value.
fn set_label(slot: &mut Option<String>, v: &Value) -> bool {
    match v.as_str() {
        Some(s) => {
            *slot = Some(s.to_string());
            true
        }
        None => false,
    }
}

/// Resolves a sample-view widget's inline samples: inline `"data": [f32…]`, or
/// `"blob": <index>` into the OSC blobs carried with the def (raw little-endian
/// `f32`). Shared by `waveform` and `plot`; `kind` names the widget in errors.
fn inline_samples(
    kind: &str,
    id: Option<i32>,
    props: &serde_json::Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Result<Arc<[f32]>, String> {
    let label = id.map_or_else(|| kind.to_string(), |i| format!("{kind} {i}"));
    if let Some(Value::Array(items)) = props.get("data") {
        let samples: Vec<f32> = items
            .iter()
            .map(|v| v.as_f64().map(|x| x as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| format!("{label}: `data` must be an array of numbers"))?;
        return Ok(samples.into());
    }
    if let Some(index) = props.get("blob").and_then(Value::as_u64) {
        let blob = blobs.get(index as usize).ok_or_else(|| {
            format!(
                "{label}: `blob` {index} out of range ({} sent)",
                blobs.len()
            )
        })?;
        if blob.len() % 4 != 0 {
            return Err(format!(
                "{label}: blob length {} is not a multiple of 4",
                blob.len()
            ));
        }
        let samples: Vec<f32> = blob
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        return Ok(samples.into());
    }
    // A `buffer` (audio-server fetch) or a `path`/`cache` (mapped local
    // resource) is loaded later by the windowed front; start empty.
    Ok(Arc::from([] as [f32; 0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(json: &str) -> GuiNode {
        GuiNode::parse(json.as_bytes()).unwrap()
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
              {"id":11,"type":"panel","theme":{"accent":"#ff0000"},"children":[
                {"id":12,"type":"label","text":"in the group"},
                {"id":13,"type":"slider","color":"#0000ff"},
                {"id":14,"type":"panel","theme":{"text":"#00ff00"},"children":[
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
        let n = node(r#"{"type":"panel"}"#);
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
            r#"{"type":"window","title":"W","w":480,"h":240,"layout":"col",
                "children":[{"id":12,"type":"waveform","data":[0.0,0.5,-0.5,1.0],"base_bucket":2}]}"#,
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
        match &w.children[0].kind {
            WidgetKind::Waveform {
                samples,
                base_bucket,
                buffer,
                ..
            } => {
                assert_eq!(&samples[..], &[0.0, 0.5, -0.5, 1.0]);
                assert_eq!(*base_bucket, 2);
                assert_eq!(*buffer, None);
            }
            other => panic!("expected waveform, got {other:?}"),
        }
    }

    #[test]
    fn waveform_parses_its_placement_offset() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"waveform","data":[0.0,1.0],"offset":8.0},
                {"id":2,"type":"waveform","data":[0.0,1.0],"offset":-3.0}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        assert_eq!(w.children[0].kind.editor().unwrap().offset, 8.0);
        // A negative placement clamps to 0 (no clip starts before the timeline).
        assert_eq!(w.children[1].kind.editor().unwrap().offset, 0.0);
        // The default is un-placed.
        let n = node(r#"{"type":"window","children":[{"id":3,"type":"waveform","data":[0.0]}]}"#);
        let w = Widget::from_node(9, &n, &[]).unwrap();
        assert_eq!(w.children[0].kind.editor().unwrap().offset, 0.0);
    }

    #[test]
    fn track_carries_clips_with_their_placement() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"track","label":"drums","children":[
                    {"id":10,"type":"clip","offset":0.0,"dur":100.0,"data":[0.0,1.0],"label":"a"},
                    {"id":11,"type":"clip","offset":-5.0,"dur":50.0}
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
        match &track.children[0].kind {
            WidgetKind::Clip {
                offset,
                dur,
                samples,
                label,
                ..
            } => {
                assert_eq!((*offset, *dur), (0.0, 100.0));
                assert_eq!(&samples[..], &[0.0, 1.0]);
                assert_eq!(label.as_deref(), Some("a"));
            }
            other => panic!("expected clip, got {other:?}"),
        }
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
                {"id":1,"type":"track","ruler":"beats","tempo":2.0,"playhead_at":480.0},
                {"id":2,"type":"track"}
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
        assert!(!w.children[1].kind.has_playhead());
        // The chrome is live: `/gui_set` lands on the lane itself (a track is no
        // navigation-group member, so it does not route through the group model).
        assert!(
            w.children[1]
                .kind
                .apply("playhead_at", &serde_json::json!(96000.0))
        );
        assert!(w.children[1].kind.has_playhead());
        assert_eq!(w.children[1].kind.editor().unwrap().playhead_at, 96000.0);
    }

    #[test]
    fn a_clip_parses_its_piano_roll_notes() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"track","children":[
                    {"id":10,"type":"clip","offset":0.0,"dur":400.0,"min":48.0,"max":72.0,
                     "notes":[0.0,100.0,60.0, 100.0,100.0,67.0, 999.0]}
                ]}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].children[0].kind {
            WidgetKind::Clip { notes, .. } => {
                // Two complete triples; the trailing lone number is dropped.
                assert_eq!(notes.len(), 2);
                assert_eq!(
                    (notes[0].start, notes[0].dur, notes[0].pitch),
                    (0.0, 100.0, 60.0)
                );
                assert_eq!(notes[1].pitch, 67.0);
            }
            other => panic!("expected clip, got {other:?}"),
        }
    }

    #[test]
    fn a_pianoroll_parses_its_notes_osc_and_pitch_window() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":5,"type":"pianoroll","min":36.0,"max":84.0,"snap":100.0,
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
        let on =
            node(r#"{"type":"window","children":[{"id":5,"type":"pianoroll","midi_in":true}]}"#);
        let w = Widget::from_node(1, &on, &[]).unwrap();
        assert!(matches!(
            &w.children[0].kind,
            WidgetKind::PianoRoll { midi_in: true, .. }
        ));
        let off = node(r#"{"type":"window","children":[{"id":5,"type":"pianoroll"}]}"#);
        let w = Widget::from_node(1, &off, &[]).unwrap();
        assert!(matches!(
            &w.children[0].kind,
            WidgetKind::PianoRoll { midi_in: false, .. }
        ));
    }

    #[test]
    fn waveform_by_server_buffer_starts_empty_with_the_buffer_number() {
        let n = node(r#"{"type":"window","children":[{"id":3,"type":"waveform","buffer":7}]}"#);
        let w = Widget::from_node(1, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Waveform {
                samples, buffer, ..
            } => {
                assert!(samples.is_empty(), "no inline data yet — fetched later");
                assert_eq!(*buffer, Some(7));
            }
            other => panic!("expected waveform, got {other:?}"),
        }
    }

    #[test]
    fn waveform_by_path_and_cache_defer_with_their_props() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"waveform","path":"/tmp/buf.f32","channels":2},
                {"id":2,"type":"waveform","cache":"/tmp/buf.peaks"}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Waveform {
                samples,
                path,
                channels,
                ..
            } => {
                assert!(samples.is_empty(), "samples are mapped later, not inline");
                assert_eq!(path.as_deref(), Some(std::path::Path::new("/tmp/buf.f32")));
                assert_eq!(*channels, 2);
            }
            other => panic!("expected waveform, got {other:?}"),
        }
        match &w.children[1].kind {
            WidgetKind::Waveform { cache, .. } => {
                assert_eq!(
                    cache.as_deref(),
                    Some(std::path::Path::new("/tmp/buf.peaks"))
                );
            }
            other => panic!("expected waveform, got {other:?}"),
        }
    }

    #[test]
    fn meter_and_scope_parse_with_defaults_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"meter","bus":5,"max":2.0,"label":"out"},
                {"id":2,"type":"scope","bus":6}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Meter {
                bus,
                min,
                max,
                label,
            } => {
                assert_eq!((*bus, *min, *max), (5, 0.0, 2.0));
                assert_eq!(label.as_deref(), Some("out"));
            }
            other => panic!("expected meter, got {other:?}"),
        }
        // The scope defaults to the bipolar [-1, 1] range.
        match &w.children[1].kind {
            WidgetKind::Scope { bus, min, max, .. } => {
                assert_eq!((*bus, *min, *max), (6, -1.0, 1.0))
            }
            other => panic!("expected scope, got {other:?}"),
        }
        assert_eq!(w.children[0].kind.live_bus(), Some(5));
        // A live `/gui_set` can retarget the bus and rescale the meter.
        let meter = w.find_mut(1).unwrap();
        assert!(meter.kind.apply("bus", &Value::from(8)));
        assert!(meter.kind.apply("max", &Value::from(4.0)));
        assert_eq!(meter.kind.live_bus(), Some(8));
    }

    #[test]
    fn nodetree_and_plot_parse_with_defaults_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"nodetree","group":2,"controls":0,"label":"tree"},
                {"id":2,"type":"plot","data":[0.0,1.0,-1.0],"max":2.0,"label":"sig"}
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
        match &w.children[1].kind {
            WidgetKind::Plot {
                samples, min, max, ..
            } => {
                assert_eq!(&samples[..], &[0.0, 1.0, -1.0]);
                // An explicit side is kept; the omitted one auto-fits.
                assert_eq!((*min, *max), (None, Some(2.0)));
            }
            other => panic!("expected plot, got {other:?}"),
        }
        // Live `/gui_set` retargets the tree's group and rescales the plot.
        assert!(w.find_mut(1).unwrap().kind.apply("group", &Value::from(0)));
        assert!(w.find_mut(2).unwrap().kind.apply("max", &Value::from(1.0)));
        assert_eq!(w.children[0].kind.node_tree_group(), Some(0));
    }

    #[test]
    fn plot_parses_views_channels_and_applies_live() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"plot","data":[0.0,1.0,0.0,-1.0],"channels":2,
                 "view":"spectrum","overlay":1,"sample_rate":48000.0,
                 "fft_size":1024,"freq_scale":"mel","ruler":"time","ruler_y":"off"}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Plot {
                channels,
                view,
                overlay,
                sample_rate,
                fft_size,
                freq_scale,
                ruler,
                ruler_y,
                spectrum,
                ..
            } => {
                assert_eq!(*channels, 2);
                assert_eq!(*view, super::super::plot::PlotView::Spectrum);
                assert!(*overlay);
                assert_eq!(*sample_rate, 48_000.0);
                assert_eq!(*fft_size, 1024);
                assert_eq!(*freq_scale, FreqScale::Mel);
                assert_eq!(*ruler, Ruler::Time);
                assert!(!*ruler_y);
                // The spectrum view analyzed its (inline) samples at parse.
                let spec = spectrum.as_ref().expect("analysis cached at parse");
                assert_eq!(spec.curves.len(), 2);
                assert_eq!(spec.fft_size, 1024);
            }
            other => panic!("expected plot, got {other:?}"),
        }
        // Live `/gui_set`: back to the signal view drops the analysis; a
        // numeric `min` pins that side and the string "auto" releases it.
        let kind = &mut w.find_mut(1).unwrap().kind;
        assert!(kind.apply("view", &Value::from("signal")));
        assert!(kind.apply("min", &Value::from(-2.0)));
        match kind {
            WidgetKind::Plot { spectrum, min, .. } => {
                assert!(spectrum.is_none(), "signal view holds no analysis");
                assert_eq!(*min, Some(-2.0));
            }
            other => panic!("expected plot, got {other:?}"),
        }
        assert!(kind.apply("min", &Value::from("auto")));
        assert!(kind.apply("view", &Value::from("spectrum")));
        match kind {
            WidgetKind::Plot { spectrum, min, .. } => {
                assert_eq!(*min, None);
                assert!(spectrum.is_some(), "switching back re-analyzes");
            }
            other => panic!("expected plot, got {other:?}"),
        }
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
            r#"{"type":"window","children":[{"id":3,"type":"plot","path":"/tmp/sig.f32","channels":2}]}"#,
        );
        let w = Widget::from_node(1, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Plot {
                samples,
                path,
                channels,
                ..
            } => {
                assert!(samples.is_empty(), "mapped later, not inline");
                assert_eq!(path.as_deref(), Some(std::path::Path::new("/tmp/sig.f32")));
                assert_eq!(*channels, 2);
            }
            other => panic!("expected plot, got {other:?}"),
        }
    }

    #[test]
    fn waveform_from_blob() {
        let blob: Vec<u8> = [1.0f32, -1.0]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let n = node(r#"{"type":"window","children":[{"id":2,"type":"waveform","blob":0}]}"#);
        let w = Widget::from_node(1, &n, &[blob]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Waveform { samples, .. } => assert_eq!(&samples[..], &[1.0, -1.0]),
            other => panic!("expected waveform, got {other:?}"),
        }
    }

    #[test]
    fn phasescope_and_spectrum_parse_with_defaults_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"phasescope","tap":2},
                {"id":2,"type":"spectrum","tap":0,"fft_size":1024,"log_freq":0}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Phasescope {
                tap,
                tap2,
                window_ms,
                hold,
                ..
            } => {
                assert_eq!((*tap, *tap2), (2, 3), "tap2 defaults to tap + 1");
                assert_eq!(*window_ms, 30.0);
                assert!(!*hold);
            }
            other => panic!("expected phasescope, got {other:?}"),
        }
        // A phasescope reads both taps; it is not a single-bus/tap widget.
        let mut taps = Vec::new();
        w.children[0].kind.taps_read(&mut taps);
        assert_eq!(taps, vec![2, 3]);
        assert_eq!(w.children[0].kind.live_bus(), None);
        match &w.children[1].kind {
            WidgetKind::Spectrum {
                tap,
                fft_size,
                db_floor,
                db_ceil,
                freq_scale,
                ..
            } => {
                assert_eq!((*tap, *fft_size), (0, 1024));
                assert_eq!((*db_floor, *db_ceil), (-100.0, 0.0));
                assert_eq!(
                    *freq_scale,
                    FreqScale::Linear,
                    "legacy log_freq: 0 reads as linear"
                );
            }
            other => panic!("expected spectrum, got {other:?}"),
        }
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
        match &w.find_mut(2).unwrap().kind {
            WidgetKind::Spectrum {
                fft_size,
                freq_scale,
                ..
            } => {
                assert_eq!(*fft_size, 2048);
                assert_eq!(*freq_scale, FreqScale::Mel);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn multichannel_scope_and_spectrum_read_adjacent_taps() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"scope","tap":4,"channels":2,"overlay":1,"ruler":"off"},
                {"id":2,"type":"spectrum","tap":6,"channels":3,"ruler_y":"off"}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Scope {
                channels,
                overlay,
                ruler,
                ruler_y,
                ..
            } => {
                assert_eq!(*channels, 2);
                assert!(*overlay);
                assert!(!*ruler, "\"off\" hides the x strip");
                assert!(*ruler_y, "the y strip defaults on");
            }
            other => panic!("expected scope, got {other:?}"),
        }
        // Each consumer reads its whole adjacent run.
        let mut taps = Vec::new();
        w.children[0].kind.taps_read(&mut taps);
        assert_eq!(taps, vec![4, 5]);
        taps.clear();
        w.children[1].kind.taps_read(&mut taps);
        assert_eq!(taps, vec![6, 7, 8]);
        // Live: grow the runs and toggle a strip back on.
        assert!(
            w.find_mut(1)
                .unwrap()
                .kind
                .apply("channels", &Value::from(4))
        );
        assert!(w.find_mut(1).unwrap().kind.apply("ruler", &Value::from(1)));
        taps.clear();
        w.find_mut(1).unwrap().kind.taps_read(&mut taps);
        assert_eq!(taps, vec![4, 5, 6, 7]);
        match &w.children[1].kind {
            WidgetKind::Spectrum {
                channels, ruler_y, ..
            } => {
                assert_eq!(*channels, 3);
                assert!(!*ruler_y);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn waveform_editor_props_parse_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"waveform","data":[0.0,1.0],"channels":2,"overlay":1,
                 "ruler":"samples","sample_rate":48000.0,"sel_start":100.0,"sel_len":50.0,
                 "playhead_at":1000.0}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Waveform {
                channels,
                overlay,
                editor,
                ..
            } => {
                assert_eq!(*channels, 2);
                assert!(*overlay);
                assert_eq!(editor.ruler, Ruler::Samples);
                assert_eq!(editor.sample_rate, 48_000.0);
                assert_eq!(editor.selection(), Some((100.0, 50.0)));
                assert_eq!(editor.playhead_at, 1000.0);
            }
            other => panic!("expected waveform, got {other:?}"),
        }
        assert!(w.children[0].kind.has_playhead());
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
        assert_eq!(editor.selection(), None, "zero length clears it");
        assert!(!wf.kind.has_playhead());
        assert_eq!(editor.ruler, Ruler::Off);
    }

    #[test]
    fn editor_ruler_units_parse_and_apply() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"waveform","data":[0.0],"ruler":"beats",
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
                {"id":1,"type":"waveform","data":[0.0],"y_start":0.8,"y_len":0.5},
                {"id":2,"type":"spectrogram","data":[0.0]}
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
                {"id":1,"type":"spectrogram","path":"/tmp/a.f32","channels":2,
                 "sample_rate":44100.0},
                {"id":2,"type":"spectrogram","buffer":3,"window_size":333}
            ]}"#,
        );
        let mut w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Spectrogram {
                path,
                channels,
                window_size,
                hop,
                sample_rate,
                db_floor,
                db_ceil,
                freq_scale,
                colormap,
                editor,
                ..
            } => {
                assert_eq!(path.as_deref(), Some(std::path::Path::new("/tmp/a.f32")));
                assert_eq!((*channels, *window_size, *hop), (2, 1024, 512));
                assert_eq!(*sample_rate, 44_100.0);
                assert_eq!((*db_floor, *db_ceil), (-90.0, 0.0));
                assert_eq!(*freq_scale, FreqScale::Log, "log is the default scale");
                assert_eq!(*colormap, 0);
                assert_eq!(editor.ruler_y, RulerY::Hz, "the Hz ruler defaults on");
            }
            other => panic!("expected spectrogram, got {other:?}"),
        }
        // An unsupported window size degrades to the default.
        match &w.children[1].kind {
            WidgetKind::Spectrogram {
                buffer,
                window_size,
                ..
            } => {
                assert_eq!(*buffer, Some(3));
                assert_eq!(*window_size, 1024, "333 is not a supported FFT size");
            }
            other => panic!("expected spectrogram, got {other:?}"),
        }
        // Live `/gui_set`: the display uniforms retune with zero recompute.
        let sg = w.find_mut(1).unwrap();
        assert!(sg.kind.apply("db_floor", &Value::from(-60.0)));
        assert!(sg.kind.apply("log_freq", &Value::from(0)), "legacy alias");
        assert!(sg.kind.apply("colormap", &Value::from(1)));
        assert!(sg.kind.apply("sel_start", &Value::from(10.0)));
        match &sg.kind {
            WidgetKind::Spectrogram {
                db_floor,
                freq_scale,
                colormap,
                editor,
                ..
            } => {
                assert_eq!(*db_floor, -60.0);
                assert_eq!(*freq_scale, FreqScale::Linear, "log_freq 0 -> linear");
                assert_eq!(*colormap, 1);
                assert_eq!(editor.sel_start, 10.0);
            }
            _ => unreachable!(),
        }
        // The four-scale prop wins over the legacy alias and applies live.
        assert!(sg.kind.apply("freq_scale", &Value::from("mel")));
        assert!(!sg.kind.apply("freq_scale", &Value::from("nonesuch")));
        assert!(sg.kind.apply("ruler_y", &Value::from("off")));
        match &sg.kind {
            WidgetKind::Spectrogram {
                freq_scale, editor, ..
            } => {
                assert_eq!(*freq_scale, FreqScale::Mel);
                assert_eq!(editor.ruler_y, RulerY::Off);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn spectrogram_freq_scale_prop_parses_all_four() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"spectrogram","data":[0.0],"freq_scale":"bark"},
                {"id":2,"type":"spectrogram","data":[0.0],"freq_scale":"linear","log_freq":1}
            ]}"#,
        );
        let w = Widget::from_node(9, &n, &[]).unwrap();
        match &w.children[0].kind {
            WidgetKind::Spectrogram { freq_scale, .. } => {
                assert_eq!(*freq_scale, FreqScale::Bark)
            }
            other => panic!("expected spectrogram, got {other:?}"),
        }
        // freq_scale wins over the legacy log_freq when both are present.
        match &w.children[1].kind {
            WidgetKind::Spectrogram { freq_scale, .. } => {
                assert_eq!(*freq_scale, FreqScale::Linear)
            }
            other => panic!("expected spectrogram, got {other:?}"),
        }
    }

    #[test]
    fn bpf_parses_with_defaults_and_applies() {
        let n = node(
            r#"{"type":"window","children":[
                {"id":1,"type":"bpf","points":[0.0,0.0,1,0.0, 0.1,1.0,-4.0,0.0, 1.0,0.0,1,0.0],
                 "label":"env"},
                {"id":2,"type":"bpf","min":20.0,"max":20000.0,"exp":1,"duration":4.0}
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
            r#"{"type":"window","layout":"row","children":[
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
        let n = node(r#"{"type":"window","layout":"grid","margin":0,"gap":2,"cols":3}"#);
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
        assert!(kind.apply("layout", &serde_json::json!("row")));
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
        let n = node(r#"{"type":"window","children":[{"id":2,"type":"waveform","blob":3}]}"#);
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
        let n = node(r#"{"type":"window","children":[{"id":6,"type":"piano"}]}"#);
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
            r#"{"type":"window","children":[{"id":6,"type":"piano","min":61,"max":85,
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

    #[test]
    fn piano_apply_round_trips_and_prunes_held_keys() {
        let n = node(r#"{"type":"window","children":[{"id":6,"type":"piano","min":48,"max":84}]}"#);
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
}
