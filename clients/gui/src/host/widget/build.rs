//! Constructing a typed [`WidgetKind`] from a generic GuiDef node — the
//! wire-to-schema pass, one arm per widget type. Split out of the schema
//! ([`super`]) so the enum and the (long) construction match read separately;
//! the shared prop-reading helpers live in [`super::parse`], visible here as a
//! sibling of the `widget` module tree.

use serde_json::{Map, Value};

use super::*;

/// The default pitch window of a note view — a piano's compass (A0..C8),
/// shared by the `pianoroll` widget and by a `clip` whose body is a roll, so
/// the two cannot drift apart.
const PITCH_MIN: f32 = 21.0;
const PITCH_MAX: f32 = 108.0;

/// Builds the [`WidgetKind`] a GuiDef `node` names (an unknown type becomes
/// [`WidgetKind::Unknown`]). `id` is the node's resolved id (some widgets log
/// with it); `blobs` are the `/gui_def` message's trailing bulk payloads.
pub(super) fn build_kind(
    id: Option<i32>,
    kind: &str,
    props: &Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Result<WidgetKind, String> {
    Ok(match kind {
        "window" => WidgetKind::Window {
            title: props
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            width: dimension(props, "w", DEFAULT_WINDOW.0),
            height: dimension(props, "h", DEFAULT_WINDOW.1),
            layout: Layout::parse(props),
            flow: Flow::parse(props),
        },
        "panel" | "box" => WidgetKind::Panel {
            layout: Layout::parse(props),
            flow: Flow::parse(props),
        },
        // One child at a time: the container has no arrangement to name, since
        // the shown child fills it — only which one, and the margin around it.
        "stack" => WidgetKind::Stack {
            index: int_prop(props, "index", 0),
            margin: props
                .get("margin")
                .and_then(Value::as_f64)
                .map(|v| v as f32),
        },
        "scroll" => WidgetKind::Scroll {
            // The workspace's natural arrangement is free placement (the
            // virtual content area sizes from the placement extents), so
            // `layout` defaults to `free` here, not `col`.
            layout: props
                .get("layout")
                .and_then(Value::as_str)
                .and_then(Layout::from_str)
                .unwrap_or(Layout::Free),
            flow: Flow::parse(props),
            view: ScrollView::parse(props),
        },
        "label" => WidgetKind::Label {
            text: props
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            text_size: text_size(props),
            wrap: props.get("wrap").and_then(truthy).unwrap_or(false),
            align: Align::parse(props),
        },
        // Every signal element: the six wire names are presets of one
        // element ([`super::signal::preset`]), so they parse in one arm.
        name if signal::preset(name).is_some() => build_signal(id, name, props, blobs)?,
        "meter" => WidgetKind::Meter {
            bus: int_prop(props, "bus", 0),
            rate: Rate::parse(props.get("rate").and_then(Value::as_str)),
            min: number(props, "min", 0.0),
            max: number(props, "max", 1.0),
            label: label(props),
        },
        "nodetree" => WidgetKind::NodeTree {
            group: int_prop(props, "group", 0),
            controls: props.get("controls").and_then(truthy).unwrap_or(true),
            label: label(props),
        },
        "canvas" => WidgetKind::Canvas {
            shader: props
                .get("shader")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| canvas::DEFAULT_SHADER.to_string()),
            params: f32_array(props, "params", 0.0),
            buses: i32_array(props, "buses", -1),
            label: label(props),
        },
        "bpf" => {
            let min = number(props, "min", 0.0);
            let max = number(props, "max", 1.0);
            let (lo, hi) = (min.min(max), min.max(max));
            WidgetKind::Bpf {
                points: props
                    .get("points")
                    .and_then(|v| super::bpf::parse_points(v, lo, hi))
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| super::bpf::default_points(lo)),
                min: lo,
                max: hi,
                duration: number_f64(props, "duration", 0.0),
                exp: props.get("exp").and_then(truthy).unwrap_or(false),
                label: label(props),
            }
        }
        "score" => WidgetKind::Score(super::score::ScoreData::parse(props)),
        "slider" => WidgetKind::Slider {
            range: Range::parse(props),
            vertical: props.get("vertical").and_then(truthy).unwrap_or(false),
        },
        "knob" => WidgetKind::Knob(Range::parse(props)),
        "number" => WidgetKind::Number(Range::parse(props)),
        "button" => WidgetKind::Button {
            label: label(props),
            text_size: text_size(props),
        },
        "toggle" => WidgetKind::Toggle {
            value: props.get("value").and_then(truthy).unwrap_or(false),
            label: label(props),
            text_size: text_size(props),
        },
        "text" => WidgetKind::Text {
            value: props
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            label: label(props),
            text_size: text_size(props),
            multiline: props.get("multiline").and_then(truthy).unwrap_or(false),
            caret: super::textedit::Caret::default(),
        },
        "menu" => {
            let options = options(props);
            let index = props.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            WidgetKind::Menu {
                index: index.min(options.len().saturating_sub(1)),
                options,
                label: label(props),
                text_size: text_size(props),
            }
        }
        "track" => WidgetKind::Track {
            label: label(props),
            height: number(props, "height", 1.0).max(0.0),
            snap: number_f64(props, "snap", 0.0).max(0.0),
            // Presence-driven: a lane that names no `mute` offers no mute
            // button, so a header stays the name strip it always was.
            header: crate::host::track::Header {
                w: props
                    .get("header_w")
                    .and_then(Value::as_f64)
                    .map(|w| w as f32),
                mute: props.get("mute").and_then(truthy),
                solo: props.get("solo").and_then(truthy),
                level: props
                    .get("level")
                    .and_then(Value::as_f64)
                    .map(|v| (v as f32).clamp(0.0, 1.0)),
            },
            editor: EditorProps::parse_lane(props),
        },
        "pianoroll" => {
            let osc = parse_osc(props);
            WidgetKind::PianoRoll {
                notes: parse_notes(props),
                selected: Vec::new(),
                // The velocity lane is on by default; the OSC lane shows when
                // there are events or it is explicitly asked for (so an empty
                // lane can still be opened to author events).
                velocity_lane: props.get("velocity").and_then(truthy).unwrap_or(true),
                osc_lane: props
                    .get("osc_lane")
                    .and_then(truthy)
                    .unwrap_or(!osc.is_empty()),
                osc,
                min: number(props, "min", PITCH_MIN),
                max: number(props, "max", PITCH_MAX),
                snap: number_f64(props, "snap", 0.0).max(0.0),
                midi_in: props.get("midi_in").and_then(truthy).unwrap_or(false),
                label: label(props),
                editor: EditorProps::parse(props, RulerY::Off),
            }
        }
        "piano" => {
            let min = number(props, "min", 36.0) as i32;
            let max = number(props, "max", 96.0) as i32;
            WidgetKind::Piano {
                min: super::piano::snap_white_down(min.min(max).clamp(0, 127)),
                max: max.max(min).clamp(0, 127),
                active_min: number(props, "active_min", 0.0) as i32,
                active_max: number(props, "active_max", 127.0) as i32,
                pan: props.get("pan").and_then(truthy).unwrap_or(true),
                overview: props.get("overview").and_then(truthy).unwrap_or(true),
                // Absent or negative = dynamic (mapped from the press height).
                velocity: props
                    .get("velocity")
                    .and_then(Value::as_i64)
                    .filter(|&v| v >= 0)
                    .map(|v| (v as i32).clamp(1, 127)),
                channel: (number(props, "channel", 0.0) as i32).clamp(0, 15),
                voice: props
                    .get("voice")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                voice_args: parse_voice_args(props),
                pressed: Vec::new(),
                label: label(props),
            }
        }
        // The free-standing ruler: a strip the document places, reading the
        // group's axis. Its chrome parses like any timeline widget's, so
        // `ruler` (the unit), `tempo`/`beat_at`/`quant` (the beats grid),
        // `sample_rate` and `link` all mean what they mean everywhere else.
        "timeruler" => WidgetKind::TimeRuler {
            editor: EditorProps::parse(props, RulerY::Off),
        },
        "clip" => WidgetKind::Clip {
            offset: number_f64(props, "offset", 0.0).max(0.0),
            dur: number_f64(props, "dur", 0.0).max(0.0),
            label: label(props),
        },
        "patch" => WidgetKind::Patch {
            selected: Vec::new(),
            patch: parse_patch(props),
            label: label(props),
        },
        other => WidgetKind::Unknown(other.to_string()),
    })
}

/// The bodies a `clip` node describes, as the child widgets they are — back to
/// front, so they **layer**: the take, the events over it, the envelope over
/// both. A body the props do not describe is simply absent (a clip is not
/// obliged to carry all three, and an empty one draws nothing but its frame).
///
/// This is the one place a clip's wire props become elements. The elements
/// themselves are the ordinary ones — a signal element for the take, a
/// piano-roll for the events, a break-point curve for the automation — so
/// nothing here re-describes what they are; it only says which props feed
/// which, and with what default axis.
pub(super) fn clip_bodies(
    props: &Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Result<Vec<Widget>, String> {
    let mut out = Vec::new();
    if let Some(take) = clip_take(props, blobs)? {
        out.push(body_widget(take));
    }
    if let Some(roll) = clip_roll(props) {
        out.push(body_widget(roll));
    }
    if let Some(curve) = clip_curve(props) {
        out.push(body_widget(curve));
    }
    Ok(out)
}

/// A clip body as a tree node: a widget with **no id** (the clip is what a
/// script addresses) and no place props (a body fills the clip's rectangle —
/// they layer rather than divide it, which is the layout's rule for a time
/// container's contents, not a prop of theirs).
pub(super) fn body_widget(kind: WidgetKind) -> Widget {
    Widget {
        id: None,
        kind,
        place: Place::default(),
        gestures: None,
        theme_over: None,
        color: None,
        theme: None,
        children: Vec::new(),
    }
}

/// An **empty** body of the kind `is` names, for a clip growing one it was not
/// built with (a `/gui_set` of `points` on a clip that had only a take). The
/// same three elements, with nothing in them yet.
pub(super) fn empty_clip_body(is: fn(&WidgetKind) -> bool) -> Option<WidgetKind> {
    let candidates: [WidgetKind; 3] = [
        WidgetKind::Signal(Box::new(take_element(signal::Data {
            samples: Arc::from([] as [f32; 0]),
            channels: 1,
            buffer: None,
            path: None,
            cache: None,
            base_bucket: DEFAULT_BASE_BUCKET,
            bulk: true,
            body: None,
        }))),
        WidgetKind::PianoRoll {
            notes: Vec::new(),
            osc: Vec::new(),
            selected: Vec::new(),
            min: PITCH_MIN,
            max: PITCH_MAX,
            snap: 0.0,
            velocity_lane: false,
            osc_lane: false,
            midi_in: false,
            label: None,
            editor: EditorProps::body(),
        },
        WidgetKind::Bpf {
            points: Vec::new(),
            min: -1.0,
            max: 1.0,
            duration: 0.0,
            exp: false,
            label: None,
        },
    ];
    candidates.into_iter().find(|k: &WidgetKind| is(k))
}

/// The signal element a clip's take is, over `source`: the `waveform` preset
/// with every capability off and no chrome — it is drawn against the clip's
/// axis, and the clip is what navigates.
fn take_element(source: signal::Data) -> signal::SignalElement {
    let mut el = signal::SignalElement::from_preset(
        &signal::preset("waveform").expect("the waveform preset exists"),
    );
    el.caps = signal::Caps::default();
    el.editor.ruler = Ruler::Off;
    el.editor.ruler_y = RulerY::Off;
    el.source = signal::Source::Data(source);
    el
}

/// A clip's **take**: a signal element over the clip's source props, with every
/// capability off — it is drawn against the clip's axis, and the clip is what
/// navigates. `bulk`, because a take is a take: it resolves as a peak pyramid,
/// never as an array of samples, however long the material turns out to be.
fn clip_take(props: &Map<String, Value>, blobs: &[Vec<u8>]) -> Result<Option<WidgetKind>, String> {
    let samples = inline_samples("clip", None, props, blobs)?;
    let (buffer, path, cache) = (
        props
            .get("buffer")
            .and_then(Value::as_i64)
            .map(|n| n as i32),
        props.get("path").and_then(Value::as_str).map(PathBuf::from),
        props
            .get("cache")
            .and_then(Value::as_str)
            .map(PathBuf::from),
    );
    if samples.is_empty() && buffer.is_none() && path.is_none() && cache.is_none() {
        return Ok(None);
    }
    let mut el = take_element(signal::Data {
        samples,
        channels: props
            .get("channels")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).max(1))
            .unwrap_or(1),
        buffer,
        path,
        cache,
        base_bucket: props
            .get("base_bucket")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).max(1))
            .unwrap_or(DEFAULT_BASE_BUCKET),
        bulk: true,
        body: None,
    });
    el.value = signal::ValueRange::new(number(props, "min", -1.0), number(props, "max", 1.0));
    Ok(Some(WidgetKind::Signal(Box::new(el))))
}

/// A clip's **roll**: the note events over a pitch window. The window defaults
/// to the `pianoroll` widget's own compass rather than to an amplitude range —
/// a pitch axis of `[-1, 1]` would clamp every note to the clip's top edge,
/// silently, since nothing about the drawing would say why.
fn clip_roll(props: &Map<String, Value>) -> Option<WidgetKind> {
    let notes = parse_notes(props);
    if notes.is_empty() {
        return None;
    }
    Some(WidgetKind::PianoRoll {
        notes,
        osc: Vec::new(),
        selected: Vec::new(),
        min: number(props, "min", PITCH_MIN),
        max: number(props, "max", PITCH_MAX),
        snap: 0.0,
        velocity_lane: false,
        osc_lane: false,
        midi_in: false,
        label: None,
        editor: EditorProps::body(),
    })
}

/// A clip's **automation curve**: break-points over the clip's span, against
/// the curve's *own* value range (`points_min`/`points_max`), because a layered
/// clip's bodies do not share an axis — an envelope's units are not the pitches
/// under it.
fn clip_curve(props: &Map<String, Value>) -> Option<WidgetKind> {
    let min = number(props, "points_min", number(props, "min", -1.0));
    let max = number(props, "points_max", number(props, "max", 1.0));
    let points = props
        .get("points")
        .and_then(|v| super::bpf::parse_points(v, min, max))
        .filter(|p| !p.is_empty())?;
    Some(WidgetKind::Bpf {
        points,
        min,
        max,
        // A clip's curve is placed on the clip's own axis, so its domain is the
        // clip's span rather than a `duration` of its own.
        duration: 0.0,
        exp: props.get("exp").and_then(truthy).unwrap_or(false),
        label: None,
    })
}

/// Builds a [`WidgetKind::Signal`] from the wire node: the type name picks the
/// [`preset`](signal::preset) — the point of the presentation × source ×
/// capabilities product that name has always meant — and the props are read
/// over it. One arm for all six names, because there is one element.
fn build_signal(
    id: Option<i32>,
    name: &str,
    props: &Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Result<WidgetKind, String> {
    let p = signal::preset(name).expect("caller matched a preset name");
    let mut el = signal::SignalElement::from_preset(&p);

    let channels = props
        .get("channels")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).max(1))
        .unwrap_or(1);
    el.source = if p.live {
        signal::Source::Bus(signal::Bus {
            bus: int_prop(props, "bus", 0),
            rate: Rate::parse(props.get("rate").and_then(Value::as_str)),
            channels,
            window_ms: number(props, "window_ms", p.window_ms),
            trigger: number(props, "trigger", 0.0),
            hold: props.get("hold").and_then(truthy).unwrap_or(false),
        })
    } else {
        signal::Source::Data(signal::Data {
            samples: inline_samples(name, id, props, blobs)?,
            channels,
            buffer: props
                .get("buffer")
                .and_then(Value::as_i64)
                .map(|n| n as i32),
            path: props.get("path").and_then(Value::as_str).map(PathBuf::from),
            cache: props
                .get("cache")
                .and_then(Value::as_str)
                .map(PathBuf::from),
            base_bucket: props
                .get("base_bucket")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).max(1))
                .unwrap_or(DEFAULT_BASE_BUCKET),
            bulk: p.bulk,
            body: None,
        })
    };

    // The `view` prop, where the name reads one: the static plot's
    // signal/spectrum switch.
    if p.view_prop
        && let Some(view) = props.get("view").and_then(Value::as_str)
        && let Some(view) = super::plot::PlotView::parse(view)
    {
        el.presentation = match view {
            super::plot::PlotView::Signal => Presentation::Signal,
            super::plot::PlotView::Spectrum => Presentation::Spectrum,
        };
    }

    // The value axis: a named side wins, an unnamed one keeps the preset's
    // (which is `None` — auto-fitted — only where the name meant that).
    el.value = signal::ValueRange {
        min: opt_number(props, "min").or(p.value.min),
        max: opt_number(props, "max").or(p.value.max),
    };

    let size = props
        .get(p.size_prop)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .filter(|n| clausters_core::fft::supports(*n))
        .unwrap_or(p.spectral.fft_size);
    el.spectral = signal::Spectral {
        fft_size: size,
        hop: props
            .get("hop")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).max(1))
            .unwrap_or(size / 2),
        db_floor: number(props, "db_floor", p.spectral.db_floor),
        db_ceil: number(props, "db_ceil", p.spectral.db_ceil),
        freq_scale: parse_freq_scale(props),
        averaging: number(props, "averaging", p.spectral.averaging).clamp(0.0, 0.99),
        peak_hold: props.get("peak_hold").and_then(truthy).unwrap_or(false),
        colormap: int_prop(props, "colormap", p.spectral.colormap),
    };

    el.display = signal::Display {
        overlay: props.get("overlay").and_then(truthy).unwrap_or(false),
        label: label(props),
    };
    el.editor = EditorProps::parse(props, p.ruler_y);
    el.editor.ruler = Ruler::parse_with(props, p.ruler);

    // The capabilities the preset welded to the name, as the props they are:
    // whether a view navigates, carries a selection or edits back is a choice
    // over any presentation, and the six names only ever froze six of them.
    let cap = |key: &str, default: bool| props.get(key).and_then(truthy).unwrap_or(default);
    el.caps = signal::Caps {
        navigable: cap("navigable", el.caps.navigable),
        selectable: cap("selectable", el.caps.selectable),
        editable: cap("editable", el.caps.editable),
    };

    el.refresh_analysis();
    Ok(WidgetKind::Signal(Box::new(el)))
}
