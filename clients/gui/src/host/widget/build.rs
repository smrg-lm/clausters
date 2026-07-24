//! Constructing a typed [`WidgetKind`] from a generic GuiDef node — the
//! wire-to-schema pass, one arm per widget type. Split out of the schema
//! ([`super`]) so the enum and the (long) construction match read separately;
//! the shared prop-reading helpers live in [`super::parse`], visible here as a
//! sibling of the `widget` module tree.

use serde_json::Value;

use super::{GuiNode, *};

/// Builds the [`WidgetKind`] a GuiDef `node` names (an unknown type becomes
/// [`WidgetKind::Unknown`]). `id` is the node's resolved id (some widgets log
/// with it); `blobs` are the `/gui_def` message's trailing bulk payloads.
pub(super) fn build_kind(
    id: Option<i32>,
    node: &GuiNode,
    blobs: &[Vec<u8>],
) -> Result<WidgetKind, String> {
    Ok(match node.kind.as_str() {
        "window" => WidgetKind::Window {
            title: node
                .props
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            width: dimension(&node.props, "w", DEFAULT_WINDOW.0),
            height: dimension(&node.props, "h", DEFAULT_WINDOW.1),
            layout: Layout::parse(&node.props),
            flow: Flow::parse(&node.props),
        },
        "panel" | "box" => WidgetKind::Panel {
            layout: Layout::parse(&node.props),
            flow: Flow::parse(&node.props),
        },
        "scroll" => WidgetKind::Scroll {
            // The workspace's natural arrangement is free placement (the
            // virtual content area sizes from the placement extents), so
            // `layout` defaults to `free` here, not `col`.
            layout: node
                .props
                .get("layout")
                .and_then(Value::as_str)
                .and_then(Layout::from_str)
                .unwrap_or(Layout::Free),
            flow: Flow::parse(&node.props),
            view: ScrollView::parse(&node.props),
        },
        "label" => WidgetKind::Label {
            text: node
                .props
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            text_size: text_size(&node.props),
            wrap: node.props.get("wrap").and_then(truthy).unwrap_or(false),
            align: Align::parse(&node.props),
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
                channels: int_prop(&node.props, "channels", 1).max(1) as usize,
                overlay: node.props.get("overlay").and_then(truthy).unwrap_or(false),
                window_ms: number(&node.props, "window_ms", 20.0),
                trigger: number(&node.props, "trigger", 0.0),
                hold: node.props.get("hold").and_then(truthy).unwrap_or(false),
                min: number(&node.props, "min", -1.0),
                max: number(&node.props, "max", 1.0),
                ruler: strip_shown(&node.props, "ruler"),
                ruler_y: strip_shown(&node.props, "ruler_y"),
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
            channels: int_prop(&node.props, "channels", 1).max(1) as usize,
            fft_size: fft_size(&node.props),
            db_floor: number(&node.props, "db_floor", -100.0),
            db_ceil: number(&node.props, "db_ceil", 0.0),
            freq_scale: parse_freq_scale(&node.props),
            averaging: number(&node.props, "averaging", 0.5).clamp(0.0, 0.99),
            peak_hold: node
                .props
                .get("peak_hold")
                .and_then(truthy)
                .unwrap_or(false),
            ruler: strip_shown(&node.props, "ruler"),
            ruler_y: strip_shown(&node.props, "ruler_y"),
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
        "plot" => {
            let mut kind = WidgetKind::Plot {
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
                view: node
                    .props
                    .get("view")
                    .and_then(Value::as_str)
                    .and_then(super::plot::PlotView::parse)
                    .unwrap_or_default(),
                overlay: node.props.get("overlay").and_then(truthy).unwrap_or(false),
                sample_rate: number_f64(&node.props, "sample_rate", 0.0),
                min: opt_number(&node.props, "min"),
                max: opt_number(&node.props, "max"),
                ruler: Ruler::parse(&node.props),
                ruler_y: !matches!(
                    node.props.get("ruler_y").and_then(Value::as_str),
                    Some("off") | Some("none")
                ),
                fft_size: valid_fft_size(
                    node.props
                        .get("fft_size")
                        .and_then(Value::as_u64)
                        .unwrap_or(DEFAULT_PLOT_FFT as u64),
                ),
                db_floor: number(&node.props, "db_floor", -100.0),
                db_ceil: number(&node.props, "db_ceil", 0.0),
                freq_scale: parse_freq_scale(&node.props),
                spectrum: None,
                label: label(&node.props),
            };
            kind.refresh_plot_analysis();
            kind
        }
        "score" => WidgetKind::Score(super::score::ScoreData::parse(&node.props)),
        "slider" => WidgetKind::Slider {
            range: Range::parse(&node.props),
            vertical: node.props.get("vertical").and_then(truthy).unwrap_or(false),
        },
        "knob" => WidgetKind::Knob(Range::parse(&node.props)),
        "number" => WidgetKind::Number(Range::parse(&node.props)),
        "button" => WidgetKind::Button {
            label: label(&node.props),
            text_size: text_size(&node.props),
        },
        "toggle" => WidgetKind::Toggle {
            value: node.props.get("value").and_then(truthy).unwrap_or(false),
            label: label(&node.props),
            text_size: text_size(&node.props),
        },
        "text" => WidgetKind::Text {
            value: node
                .props
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            label: label(&node.props),
            text_size: text_size(&node.props),
            multiline: node
                .props
                .get("multiline")
                .and_then(truthy)
                .unwrap_or(false),
            caret: super::textedit::Caret::default(),
        },
        "menu" => {
            let options = options(&node.props);
            let index = node.props.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            WidgetKind::Menu {
                index: index.min(options.len().saturating_sub(1)),
                options,
                label: label(&node.props),
                text_size: text_size(&node.props),
            }
        }
        "track" => WidgetKind::Track {
            label: label(&node.props),
            height: number(&node.props, "height", 1.0).max(0.0),
            snap: number_f64(&node.props, "snap", 0.0).max(0.0),
            editor: EditorProps::parse_lane(&node.props),
        },
        "pianoroll" => {
            let osc = parse_osc(&node.props);
            WidgetKind::PianoRoll {
                notes: parse_notes(&node.props),
                selected: Vec::new(),
                // The velocity lane is on by default; the OSC lane shows when
                // there are events or it is explicitly asked for (so an empty
                // lane can still be opened to author events).
                velocity_lane: node.props.get("velocity").and_then(truthy).unwrap_or(true),
                osc_lane: node
                    .props
                    .get("osc_lane")
                    .and_then(truthy)
                    .unwrap_or(!osc.is_empty()),
                osc,
                min: number(&node.props, "min", 21.0),
                max: number(&node.props, "max", 108.0),
                snap: number_f64(&node.props, "snap", 0.0).max(0.0),
                midi_in: node.props.get("midi_in").and_then(truthy).unwrap_or(false),
                label: label(&node.props),
                editor: EditorProps::parse(&node.props, RulerY::Off),
            }
        }
        "piano" => {
            let min = number(&node.props, "min", 36.0) as i32;
            let max = number(&node.props, "max", 96.0) as i32;
            WidgetKind::Piano {
                min: super::piano::snap_white_down(min.min(max).clamp(0, 127)),
                max: max.max(min).clamp(0, 127),
                active_min: number(&node.props, "active_min", 0.0) as i32,
                active_max: number(&node.props, "active_max", 127.0) as i32,
                pan: node.props.get("pan").and_then(truthy).unwrap_or(true),
                overview: node.props.get("overview").and_then(truthy).unwrap_or(true),
                // Absent or negative = dynamic (mapped from the press height).
                velocity: node
                    .props
                    .get("velocity")
                    .and_then(Value::as_i64)
                    .filter(|&v| v >= 0)
                    .map(|v| (v as i32).clamp(1, 127)),
                channel: (number(&node.props, "channel", 0.0) as i32).clamp(0, 15),
                voice: node
                    .props
                    .get("voice")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                voice_args: parse_voice_args(&node.props),
                pressed: Vec::new(),
                label: label(&node.props),
            }
        }
        "clip" => WidgetKind::Clip {
            offset: number_f64(&node.props, "offset", 0.0).max(0.0),
            dur: number_f64(&node.props, "dur", 0.0).max(0.0),
            samples: inline_samples("clip", id, &node.props, blobs)?,
            // Filled by the host when a `cache`/`path`/`buffer` body loads.
            body: None,
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
            base_bucket: node
                .props
                .get("base_bucket")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).max(1))
                .unwrap_or(DEFAULT_BASE_BUCKET),
            notes: parse_notes(&node.props),
            points: node
                .props
                .get("points")
                .and_then(|v| {
                    // Against the *curve's* range: a layered clip's `min`/`max`
                    // belong to the body underneath (a piano-roll's pitches).
                    super::bpf::parse_points(
                        v,
                        number(&node.props, "points_min", number(&node.props, "min", -1.0)),
                        number(&node.props, "points_max", number(&node.props, "max", 1.0)),
                    )
                })
                .unwrap_or_default(),
            exp: node.props.get("exp").and_then(truthy).unwrap_or(false),
            points_min: number(&node.props, "points_min", number(&node.props, "min", -1.0)),
            points_max: number(&node.props, "points_max", number(&node.props, "max", 1.0)),
            min: number(&node.props, "min", -1.0),
            max: number(&node.props, "max", 1.0),
            label: label(&node.props),
        },
        "patch" => WidgetKind::Patch {
            selected: Vec::new(),
            patch: parse_patch(&node.props),
            label: label(&node.props),
        },
        other => WidgetKind::Unknown(other.to_string()),
    })
}
