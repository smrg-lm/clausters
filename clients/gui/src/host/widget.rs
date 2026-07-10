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

use std::path::PathBuf;
use std::sync::Arc;

use clausters_core::osc::OscType;
use serde_json::Value;

use crate::spectrogram::FreqScale;

use super::canvas;
use super::guidef::GuiNode;

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
        match props.get("layout").and_then(Value::as_str) {
            Some("row") => Layout::Row,
            Some("grid") => Layout::Grid,
            Some("free") => Layout::Free,
            _ => Layout::Col,
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
    pub y_start: f64,
    pub y_len: f64,
    pub link: Option<i32>,
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
            y_start: number_f64(props, "y_start", 0.0),
            y_len: number_f64(props, "y_len", 1.0),
            link: props
                .get("link")
                .and_then(Value::as_i64)
                .filter(|n| *n >= 0)
                .map(|n| n as i32),
        }
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
            "y_start" => set_f64(&mut self.y_start, v),
            "y_len" => set_f64(&mut self.y_len, v),
            _ => false,
        }
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
    },
    /// A nestable container.
    Panel { layout: Layout },
    /// Static text.
    Label { text: String },
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
    /// `window_ms` window of segment tap ring `tap` (see the server's `/tap`),
    /// re-read every frame and aligned on a rising crossing of `trigger`
    /// (free-running when none is found); `hold` freezes the trace.
    Scope {
        bus: i32,
        tap: i32,
        window_ms: f32,
        trigger: f32,
        hold: bool,
        min: f32,
        max: f32,
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
    /// A live FFT magnitude curve (spectroscope) over audio tap `tap`: one
    /// forward FFT per frame of the newest `fft_size` window, magnitudes in dB
    /// over `[db_floor, db_ceil]`, on a log (`log_freq`) or linear frequency
    /// axis. `averaging` (0..1) exponentially smooths each bin so the curve does
    /// not flicker; `peak_hold` overlays a slowly decaying peak trace. The
    /// analysis reuses the shared-core FFT + Hann window, so it agrees with the
    /// spectrogram.
    Spectrum {
        tap: i32,
        fft_size: usize,
        db_floor: f32,
        db_ceil: f32,
        log_freq: bool,
        averaging: f32,
        peak_hold: bool,
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
    /// A simple static plot of a signal over `[min, max]`: a polyline when the
    /// data fits the width, a min/max envelope when it does not. Its samples
    /// arrive inline (`data`/`blob`) or — the bulk path for an NRT render's
    /// output — from a mapped local `path` of raw little-endian `f32`
    /// (`channels` de-interleaves channel 0, default 1), filled when the host
    /// maps it. Unlike the heavy `waveform`, it does not navigate.
    Plot {
        samples: Arc<[f32]>,
        path: Option<PathBuf>,
        channels: usize,
        min: f32,
        max: f32,
        label: Option<String>,
    },
    /// A continuous slider over `[min, max]`. `vertical` lays it out along the
    /// y axis (min at the bottom, max at the top) instead of the x axis.
    Slider { range: Range, vertical: bool },
    /// A rotary control over `[min, max]`.
    Knob(Range),
    /// A draggable numeric read-out over `[min, max]`.
    Number(Range),
    /// A momentary push button.
    Button { label: Option<String> },
    /// A boolean on/off control.
    Toggle { value: bool, label: Option<String> },
    /// A free-text field showing its value (script-driven at this milestone).
    Text {
        value: String,
        label: Option<String>,
    },
    /// A drop/cycle selector over `options`, holding the chosen index.
    Menu {
        index: usize,
        options: Vec<String>,
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
    pub children: Vec<Widget>,
}

impl Widget {
    /// Interprets a generic [`GuiNode`] (and the blobs carried beside it in the
    /// `/gui_def` message) into a typed widget tree. `root_id` is the def id from
    /// the OSC argument, used for the root whose JSON carries no `id`.
    pub fn from_node(root_id: i32, node: &GuiNode, blobs: &[Vec<u8>]) -> Result<Widget, String> {
        Self::build(Some(root_id), node, blobs)
    }

    fn build(id: Option<i32>, node: &GuiNode, blobs: &[Vec<u8>]) -> Result<Widget, String> {
        let id = id.or(node.id);
        let kind = match node.kind.as_str() {
            "window" => WidgetKind::Window {
                title: node
                    .props
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                width: dimension(&node.props, "w", DEFAULT_WINDOW.0),
                height: dimension(&node.props, "h", DEFAULT_WINDOW.1),
                layout: Layout::parse(&node.props),
            },
            "panel" | "box" => WidgetKind::Panel {
                layout: Layout::parse(&node.props),
            },
            "label" => WidgetKind::Label {
                text: node
                    .props
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            "waveform" => WidgetKind::Waveform {
                samples: inline_samples("waveform", id, &node.props, blobs)?,
                base_bucket: node
                    .props
                    .get("base_bucket")
                    .and_then(Value::as_u64)
                    .map(|n| (n as usize).max(1))
                    .unwrap_or(DEFAULT_BASE_BUCKET),
                buffer: node
                    .props
                    .get("buffer")
                    .and_then(Value::as_i64)
                    .map(|n| n as i32),
                path: node
                    .props
                    .get("path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from),
                cache: node
                    .props
                    .get("cache")
                    .and_then(Value::as_str)
                    .map(PathBuf::from),
                channels: node
                    .props
                    .get("channels")
                    .and_then(Value::as_u64)
                    .map(|n| (n as usize).max(1))
                    .unwrap_or(1),
                overlay: node.props.get("overlay").and_then(truthy).unwrap_or(false),
                editor: EditorProps::parse(&node.props, RulerY::Norm),
            },
            "spectrogram" => {
                let window_size = node
                    .props
                    .get("window_size")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .filter(|n| clausters_core::fft::supports(*n))
                    .unwrap_or(1024);
                WidgetKind::Spectrogram {
                    samples: inline_samples("spectrogram", id, &node.props, blobs)?,
                    channels: node
                        .props
                        .get("channels")
                        .and_then(Value::as_u64)
                        .map(|n| (n as usize).max(1))
                        .unwrap_or(1),
                    buffer: node
                        .props
                        .get("buffer")
                        .and_then(Value::as_i64)
                        .map(|n| n as i32),
                    path: node
                        .props
                        .get("path")
                        .and_then(Value::as_str)
                        .map(PathBuf::from),
                    cache: node
                        .props
                        .get("cache")
                        .and_then(Value::as_str)
                        .map(PathBuf::from),
                    window_size,
                    hop: node
                        .props
                        .get("hop")
                        .and_then(Value::as_u64)
                        .map(|n| (n as usize).max(1))
                        .unwrap_or(window_size / 2),
                    sample_rate: number_f64(&node.props, "sample_rate", 0.0),
                    db_floor: number(&node.props, "db_floor", -90.0),
                    db_ceil: number(&node.props, "db_ceil", 0.0),
                    freq_scale: parse_freq_scale(&node.props),
                    colormap: int_prop(&node.props, "colormap", 0),
                    editor: EditorProps::parse(&node.props, RulerY::Hz),
                }
            }
            "meter" => WidgetKind::Meter {
                bus: int_prop(&node.props, "bus", 0),
                min: number(&node.props, "min", 0.0),
                max: number(&node.props, "max", 1.0),
                label: label(&node.props),
            },
            "scope" => {
                // Audio-rate when a `tap` is named (or `rate: "audio"` asks
                // for the default tap 0); otherwise the control-bus history.
                let audio = node.props.contains_key("tap")
                    || node.props.get("rate").and_then(Value::as_str) == Some("audio");
                WidgetKind::Scope {
                    bus: int_prop(&node.props, "bus", 0),
                    tap: if audio {
                        int_prop(&node.props, "tap", 0)
                    } else {
                        -1
                    },
                    window_ms: number(&node.props, "window_ms", 20.0),
                    trigger: number(&node.props, "trigger", 0.0),
                    hold: node.props.get("hold").and_then(truthy).unwrap_or(false),
                    min: number(&node.props, "min", -1.0),
                    max: number(&node.props, "max", 1.0),
                    label: label(&node.props),
                }
            }
            "phasescope" => {
                let tap = int_prop(&node.props, "tap", 0);
                WidgetKind::Phasescope {
                    tap,
                    // The right channel defaults to the next ring, the natural
                    // layout for a stereo pair tapped on adjacent indices.
                    tap2: int_prop(&node.props, "tap2", tap + 1),
                    window_ms: number(&node.props, "window_ms", 30.0),
                    hold: node.props.get("hold").and_then(truthy).unwrap_or(false),
                    label: label(&node.props),
                }
            }
            "spectrum" => WidgetKind::Spectrum {
                tap: int_prop(&node.props, "tap", 0),
                fft_size: fft_size(&node.props),
                db_floor: number(&node.props, "db_floor", -100.0),
                db_ceil: number(&node.props, "db_ceil", 0.0),
                log_freq: node.props.get("log_freq").and_then(truthy).unwrap_or(true),
                averaging: number(&node.props, "averaging", 0.5).clamp(0.0, 0.99),
                peak_hold: node
                    .props
                    .get("peak_hold")
                    .and_then(truthy)
                    .unwrap_or(false),
                label: label(&node.props),
            },
            "nodetree" => WidgetKind::NodeTree {
                group: int_prop(&node.props, "group", 0),
                controls: node.props.get("controls").and_then(truthy).unwrap_or(true),
                label: label(&node.props),
            },
            "canvas" => WidgetKind::Canvas {
                shader: node
                    .props
                    .get("shader")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| canvas::DEFAULT_SHADER.to_string()),
                params: f32_array(&node.props, "params", 0.0),
                buses: i32_array(&node.props, "buses", -1),
                label: label(&node.props),
            },
            "bpf" => {
                let min = number(&node.props, "min", 0.0);
                let max = number(&node.props, "max", 1.0);
                let (lo, hi) = (min.min(max), min.max(max));
                WidgetKind::Bpf {
                    points: node
                        .props
                        .get("points")
                        .and_then(|v| super::bpf::parse_points(v, lo, hi))
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| super::bpf::default_points(lo)),
                    min: lo,
                    max: hi,
                    duration: number_f64(&node.props, "duration", 0.0),
                    exp: node.props.get("exp").and_then(truthy).unwrap_or(false),
                    label: label(&node.props),
                }
            }
            "plot" => WidgetKind::Plot {
                samples: inline_samples("plot", id, &node.props, blobs)?,
                path: node
                    .props
                    .get("path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from),
                channels: node
                    .props
                    .get("channels")
                    .and_then(Value::as_u64)
                    .map(|n| (n as usize).max(1))
                    .unwrap_or(1),
                min: number(&node.props, "min", -1.0),
                max: number(&node.props, "max", 1.0),
                label: label(&node.props),
            },
            "slider" => WidgetKind::Slider {
                range: Range::parse(&node.props),
                vertical: node.props.get("vertical").and_then(truthy).unwrap_or(false),
            },
            "knob" => WidgetKind::Knob(Range::parse(&node.props)),
            "number" => WidgetKind::Number(Range::parse(&node.props)),
            "button" => WidgetKind::Button {
                label: label(&node.props),
            },
            "toggle" => WidgetKind::Toggle {
                value: node.props.get("value").and_then(truthy).unwrap_or(false),
                label: label(&node.props),
            },
            "text" => WidgetKind::Text {
                value: node
                    .props
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                label: label(&node.props),
            },
            "menu" => {
                let options = options(&node.props);
                let index = node.props.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                WidgetKind::Menu {
                    index: index.min(options.len().saturating_sub(1)),
                    options,
                    label: label(&node.props),
                }
            }
            other => WidgetKind::Unknown(other.to_string()),
        };
        // Only containers carry children into the typed tree; a leaf's children
        // (if any) are ignored.
        let children = match kind {
            WidgetKind::Window { .. } | WidgetKind::Panel { .. } => node
                .children
                .iter()
                .map(|c| Self::build(None, c, blobs))
                .collect::<Result<Vec<_>, _>>()?,
            _ => Vec::new(),
        };
        Ok(Widget { id, kind, children })
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
            WidgetKind::Waveform { .. } | WidgetKind::Spectrogram { .. }
        )
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

    /// The audio-tap ring an audio-rate scope reads each frame, if this is one.
    pub fn live_tap(&self) -> Option<i32> {
        match self {
            WidgetKind::Scope { tap, .. } if *tap >= 0 => Some(*tap),
            _ => None,
        }
    }

    /// Appends every audio-tap ring this widget reads each frame — one for an
    /// audio-rate `scope` or a `spectrum`, two (left and right) for a
    /// `phasescope`. Drives the tap subscription/animation set, so all three tap
    /// consumers are covered uniformly.
    pub fn taps_read(&self, out: &mut Vec<i32>) {
        match self {
            WidgetKind::Scope { tap, .. } if *tap >= 0 => out.push(*tap),
            WidgetKind::Spectrum { tap, .. } if *tap >= 0 => out.push(*tap),
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

    /// The editor chrome of a timeline view (waveform/spectrogram), if this is
    /// one — the shared read path for the frame renderer and the fronts.
    pub fn editor(&self) -> Option<&EditorProps> {
        match self {
            WidgetKind::Waveform { editor, .. } | WidgetKind::Spectrogram { editor, .. } => {
                Some(editor)
            }
            _ => None,
        }
    }

    /// Mutable access to a timeline view's editor chrome (the selection drag
    /// writes through here).
    pub fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        match self {
            WidgetKind::Waveform { editor, .. } | WidgetKind::Spectrogram { editor, .. } => {
                Some(editor)
            }
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
    pub fn apply(&mut self, key: &str, v: &Value) -> bool {
        match self {
            WidgetKind::Waveform {
                overlay, editor, ..
            } => match key {
                "overlay" => truthy(v).map(|b| *overlay = b).is_some(),
                _ => editor.apply(key, v),
            },
            WidgetKind::Spectrogram {
                db_floor,
                db_ceil,
                freq_scale,
                colormap,
                editor,
                ..
            } => match key {
                "db_floor" => set_f(db_floor, v),
                "db_ceil" => set_f(db_ceil, v),
                "freq_scale" => v
                    .as_str()
                    .and_then(freq_scale_from_str)
                    .map(|s| *freq_scale = s)
                    .is_some(),
                // Legacy boolean alias: 1 -> log, 0 -> linear.
                "log_freq" => truthy(v)
                    .map(|b| *freq_scale = if b { FreqScale::Log } else { FreqScale::Linear })
                    .is_some(),
                "colormap" => v.as_i64().map(|n| *colormap = n as i32).is_some(),
                _ => editor.apply(key, v),
            },
            WidgetKind::Meter {
                bus,
                min,
                max,
                label,
            } => match key {
                "bus" => v.as_i64().map(|n| *bus = n as i32).is_some(),
                "min" => set_f(min, v),
                "max" => set_f(max, v),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Scope {
                bus,
                tap,
                window_ms,
                trigger,
                hold,
                min,
                max,
                label,
            } => match key {
                "bus" => v.as_i64().map(|n| *bus = n as i32).is_some(),
                "tap" => v.as_i64().map(|n| *tap = n as i32).is_some(),
                "window_ms" => set_f(window_ms, v),
                "trigger" => set_f(trigger, v),
                "hold" => truthy(v).map(|b| *hold = b).is_some(),
                "min" => set_f(min, v),
                "max" => set_f(max, v),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Phasescope {
                tap,
                tap2,
                window_ms,
                hold,
                label,
            } => match key {
                "tap" => v.as_i64().map(|n| *tap = n as i32).is_some(),
                "tap2" => v.as_i64().map(|n| *tap2 = n as i32).is_some(),
                "window_ms" => set_f(window_ms, v),
                "hold" => truthy(v).map(|b| *hold = b).is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Spectrum {
                tap,
                fft_size,
                db_floor,
                db_ceil,
                log_freq,
                averaging,
                peak_hold,
                label,
            } => match key {
                "tap" => v.as_i64().map(|n| *tap = n as i32).is_some(),
                "fft_size" => v
                    .as_u64()
                    .filter(|n| clausters_core::fft::supports(*n as usize))
                    .map(|n| *fft_size = n as usize)
                    .is_some(),
                "db_floor" => set_f(db_floor, v),
                "db_ceil" => set_f(db_ceil, v),
                "log_freq" => truthy(v).map(|b| *log_freq = b).is_some(),
                "averaging" => v
                    .as_f64()
                    .map(|x| *averaging = (x as f32).clamp(0.0, 0.99))
                    .is_some(),
                "peak_hold" => truthy(v).map(|b| *peak_hold = b).is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::NodeTree {
                group,
                controls,
                label,
            } => match key {
                "group" => v.as_i64().map(|n| *group = n as i32).is_some(),
                "controls" => truthy(v).map(|b| *controls = b).is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Bpf {
                points,
                min,
                max,
                duration,
                exp,
                label,
            } => match key {
                // The full breakpoint list replaces in one set — the flat
                // `[t, v, shape, curve, …]` array, or that array as a JSON
                // string (the `/gui_set` scalar carrier).
                "points" => match super::bpf::parse_points(v, *min, *max) {
                    Some(p) if !p.is_empty() => {
                        *points = p;
                        true
                    }
                    _ => false,
                },
                "min" => set_f(min, v),
                "max" => set_f(max, v),
                "duration" => set_f64(duration, v),
                "exp" => truthy(v).map(|b| *exp = b).is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Plot {
                min, max, label, ..
            } => match key {
                "min" => set_f(min, v),
                "max" => set_f(max, v),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Canvas {
                shader,
                params,
                buses,
                label,
            } => match key {
                "shader" => v.as_str().map(|s| *shader = s.to_string()).is_some(),
                "label" => set_label(label, v),
                _ => {
                    if let Some(i) = index_suffix(key, "param").filter(|i| *i < params.len()) {
                        set_f(&mut params[i], v)
                    } else if let Some(i) = index_suffix(key, "bus").filter(|i| *i < buses.len()) {
                        v.as_i64().map(|n| buses[i] = n as i32).is_some()
                    } else {
                        false
                    }
                }
            },
            WidgetKind::Slider { range: r, .. } | WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                match key {
                    "value" => set_f(&mut r.value, v),
                    "min" => set_f(&mut r.min, v),
                    "max" => set_f(&mut r.max, v),
                    "label" => set_label(&mut r.label, v),
                    _ => false,
                }
            }
            WidgetKind::Toggle { value, label } => match key {
                "value" => truthy(v).map(|b| *value = b).is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Text { value, label } => match key {
                "value" => v.as_str().map(|s| *value = s.to_string()).is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Menu {
                index,
                options,
                label,
            } => match key {
                "index" => v
                    .as_u64()
                    .map(|n| *index = (n as usize).min(options.len().saturating_sub(1)))
                    .is_some(),
                "label" => set_label(label, v),
                _ => false,
            },
            WidgetKind::Button { label } => key == "label" && set_label(label, v),
            WidgetKind::Label { text } => {
                key == "text" && v.as_str().map(|s| *text = s.to_string()).is_some()
            }
            _ => false,
        }
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
                // The plot keeps an explicit range; min defaults bipolar.
                assert_eq!((*min, *max), (-1.0, 2.0));
            }
            other => panic!("expected plot, got {other:?}"),
        }
        // Live `/gui_set` retargets the tree's group and rescales the plot.
        assert!(w.find_mut(1).unwrap().kind.apply("group", &Value::from(0)));
        assert!(w.find_mut(2).unwrap().kind.apply("max", &Value::from(1.0)));
        assert_eq!(w.children[0].kind.node_tree_group(), Some(0));
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
                log_freq,
                ..
            } => {
                assert_eq!((*tap, *fft_size), (0, 1024));
                assert_eq!((*db_floor, *db_ceil), (-100.0, 0.0));
                assert!(!*log_freq, "log_freq: 0 turns it off");
            }
            other => panic!("expected spectrum, got {other:?}"),
        }
        // Live `/gui_set`: retarget a tap, resize the FFT (only a supported size
        // takes), retune the phasescope window and freeze it.
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
        assert!(w.find_mut(1).unwrap().kind.apply("hold", &Value::from(1)));
        match &w.find_mut(2).unwrap().kind {
            WidgetKind::Spectrum { fft_size, .. } => assert_eq!(*fft_size, 2048),
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
    fn defaults_and_unknown_type() {
        // `score` is in the catalog but not yet a rendered WidgetKind variant.
        let n = node(r#"{"type":"window","children":[{"id":7,"type":"score"}]}"#);
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
        // An unrecognized type is kept (laid out), not rejected.
        match &w.children[0].kind {
            WidgetKind::Unknown(t) => assert_eq!(t, "score"),
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
}
