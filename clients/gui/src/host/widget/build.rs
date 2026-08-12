//! Constructing a typed [`WidgetKind`] from a generic GuiDef node — the
//! wire-to-schema pass, one arm per widget type. Split out of the schema
//! ([`super`]) so the enum and the (long) construction match read separately;
//! the shared prop-reading helpers live in [`super::parse`], visible here as a
//! sibling of the `widget` module tree.

use serde_json::{Map, Value};

use super::super::elements::signal::Presentation;
use super::*;
use element::BodyRole;

/// Whether a `field` is the **free-standing ruler**: a strip of a given
/// thickness with nothing placed on it and no lane chrome. Everything else a
/// field can be draws something or names a lane, and an empty lane — which a
/// multitrack opens all the time — must not read as a ruler.
fn is_bare_ruler(props: &Map<String, Value>, has_children: bool) -> bool {
    props.contains_key("h")
        && !has_children
        && ![
            "label", "height", "header_w", "mute", "solo", "level", "snap",
        ]
        .iter()
        .any(|k| props.contains_key(*k))
}

/// Whether a container carries the `hug` prop: its size follows its content
/// rather than its container's. Off unless the wire says otherwise, so no
/// existing def moves.
fn hug(props: &Map<String, Value>) -> bool {
    props.get("hug").and_then(truthy).unwrap_or(false)
}

/// Builds the [`WidgetKind`] a GuiDef `node` names (an unknown type becomes
/// [`WidgetKind::Unknown`]). `blobs` are the `/gui_def` message's trailing bulk
/// payloads.
pub(super) fn build_kind(
    kind: &str,
    props: &Map<String, Value>,
    has_children: bool,
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
            hug: hug(props),
        },
        // A container with no axes. `stack` — one child at a time, the one
        // `index` names — is one of the arrangements rather than a type of its
        // own: a layout with a selection instead of an arrangement. Nothing
        // else about it is a container's business, so it takes only a margin.
        "layout" if flow(props) == Some("stack") => WidgetKind::Stack {
            index: int_prop(props, "index", 0),
            margin: props
                .get("margin")
                .and_then(Value::as_f64)
                .map(|v| v as f32),
            hug: hug(props),
        },
        "layout" => WidgetKind::Panel {
            layout: Layout::parse(props),
            flow: Flow::parse(props),
            hug: hug(props),
        },
        // Two axes locked to one scale. What a patcher adds to a plane is its
        // boxes and the cords between them, so their presence is what tells
        // the two constructions apart. Without the `patcher` feature the fork
        // is not compiled and both readings are the workspace below — the one
        // place a dropped family degrades to *something* rather than to
        // `Unknown`, because the wire name it shares is still a real widget.
        #[cfg(feature = "patcher")]
        "plane" if props.contains_key("boxes") || props.contains_key("cords") => {
            WidgetKind::Custom(super::super::elements::patch::build(props, blobs)?)
        }
        "plane" => WidgetKind::Scroll {
            // The workspace's natural arrangement is free placement (the
            // virtual content area sizes from the placement extents), so the
            // flow defaults to `free` here, not `col`.
            layout: flow(props)
                .and_then(Layout::from_str)
                .unwrap_or(Layout::Free),
            flow: Flow::parse(props),
            view: ScrollView::parse(props),
        },
        // Two independent axes, told apart by what is on it: a placement
        // makes it a clip on its parent's x axis, a bare strip of a given
        // thickness with nothing placed and no lane chrome is the
        // free-standing ruler, and everything else is a lane — including an
        // empty one, which a multitrack opens all the time.
        "field" if props.contains_key("offset") || props.contains_key("dur") => WidgetKind::Clip {
            offset: number_f64(props, "offset", 0.0).max(0.0),
            dur: number_f64(props, "dur", 0.0).max(0.0),
            label: label(props),
        },
        "field" if is_bare_ruler(props, has_children) => WidgetKind::TimeRuler {
            editor: EditorProps::parse(props, RulerY::Off),
        },
        "field" => WidgetKind::Track {
            label: label(props),
            height: number(props, "height", 1.0).max(0.0),
            snap: number_f64(props, "snap", 0.0).max(0.0),
            // Presence-driven: a lane that names no `mute` offers no mute
            // button, so a header stays the name strip it always was.
            header: crate::host::graphics::track::Header {
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
        // No arm above answers to this name, so it is an **element**: a
        // built-in that has moved behind the trait, else whatever a program
        // registered, else nothing at all. The order is the invariant, not a
        // detail — a built-in resolves first, so a registration can never
        // shadow one or change what a shipped def means; and a miss on both is
        // what an unrecognized type has always been.
        other => {
            let built = super::super::elements::builtin(other)
                .map(|ctor| ctor(props, blobs))
                .or_else(|| element::build_registered(other, props, blobs));
            match built {
                Some(built) => WidgetKind::Custom(built?),
                None => WidgetKind::Unknown(other.to_string()),
            }
        }
    })
}

/// The bodies a `clip` node describes, as the child widgets they are — back to
/// front, so they **layer**: the take, the events over it, the envelope over
/// both. A body the props do not describe is simply absent (a clip is not
/// obliged to carry all three, and an empty one draws nothing but its frame).
///
/// This is the one place a clip's wire props become elements. The elements
/// themselves are the ordinary ones — a signal element for the take, a
/// roll for the events, a break-point curve for the automation — so
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
    if let Some(roll) = super::super::elements::notes::body(props) {
        out.push(body_widget(WidgetKind::Custom(Box::new(roll))));
    }
    if let Some(curve) = super::super::elements::curve::body(props) {
        out.push(body_widget(WidgetKind::Custom(Box::new(curve))));
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
        opacity: None,
        radius: None,
        theme: None,
        alpha: 1.0,
        children: Vec::new(),
    }
}

/// An **empty** body filling `role`, for a clip growing one it was not built
/// with (a `/gui_set` of `points` on a clip that had only a take). The same
/// three elements, with nothing in them yet.
pub(super) fn empty_clip_body(role: BodyRole) -> Option<WidgetKind> {
    let candidates: [WidgetKind; 3] = [
        WidgetKind::Custom(Box::new(take_element(
            signal::Data {
                samples: Arc::from([] as [f32; 0]),
                channels: 1,
                buffer: None,
                path: None,
                cache: None,
                base_bucket: DEFAULT_BASE_BUCKET,
                bulk: true,
                body: None,
            },
            Presentation::Signal,
        ))),
        WidgetKind::Custom(Box::new(super::super::elements::notes::empty_body())),
        WidgetKind::Custom(Box::new(super::super::elements::curve::empty_body())),
    ];
    candidates
        .into_iter()
        .find(|k: &WidgetKind| k.body_role() == Some(role))
}

/// The signal element a clip's take is, over `source`: a stored presentation
/// with every capability off and no chrome — it is drawn against the clip's
/// axis, and the clip is what navigates.
///
/// `view` is the presentation the clip asked for: the trace (the default), or
/// the time-frequency texture — the same signal, seen the other way, placed in
/// time like any take. A presentation with nothing to draw over a stored source
/// falls back to the trace rather than leaving the clip blank.
fn take_element(source: signal::Data, view: Presentation) -> signal::SignalElement {
    let view = match view {
        Presentation::Signal | Presentation::TimeFrequency => view,
        _ => Presentation::Signal,
    };
    let mut el = signal::SignalElement::from_preset(&signal::point(view, false, true));
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
    let mut el = take_element(
        signal::Data {
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
        },
        props
            .get("view")
            .and_then(Value::as_str)
            .and_then(Presentation::parse)
            .unwrap_or(Presentation::Signal),
    );
    el.spectral = spectral_props(props, el.spectral, "window_size");
    el.value = signal::ValueRange::new(number(props, "min", -1.0), number(props, "max", 1.0));
    Ok(Some(WidgetKind::Custom(Box::new(el))))
}

/// The spectral parameters a signal names, over `base` (its preset's): the
/// analysis size under whichever name this presentation calls it (`size_prop`
/// — `fft_size` for the spectra, `window_size` for the time-frequency
/// texture), the hop, and the display. One function for the element and for a
/// clip's take, so a spectral clip is tuned exactly like a spectral view.
fn spectral_props(
    props: &Map<String, Value>,
    base: signal::Spectral,
    size_prop: &str,
) -> signal::Spectral {
    let size = props
        .get(size_prop)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .filter(|n| clausters_core::fft::supports(*n))
        .unwrap_or(base.fft_size);
    signal::Spectral {
        fft_size: size,
        hop: props
            .get("hop")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).max(1))
            .unwrap_or(size / 2),
        db_floor: number(props, "db_floor", base.db_floor),
        db_ceil: number(props, "db_ceil", base.db_ceil),
        freq_scale: parse_freq_scale(props),
        averaging: number(props, "averaging", base.averaging).clamp(0.0, 0.99),
        peak_hold: props.get("peak_hold").and_then(truthy).unwrap_or(false),
        colormap: int_prop(props, "colormap", base.colormap),
    }
}

/// Builds the signal element from the wire node: `view`, the source props and
/// `navigable` name a [`point`](signal::point) of the presentation × source ×
/// capabilities product, and the rest of the props are read over its defaults.
/// One constructor, because there is one element.
pub(crate) fn signal_element(
    props: &Map<String, Value>,
    blobs: &[Vec<u8>],
) -> Result<signal::SignalElement, String> {
    let id: Option<i32> = props.get("id").and_then(Value::as_i64).map(|n| n as i32);
    // The point of the product the props name: the presentation, whether the
    // source is forward-only, and whether the view navigates.
    let view = props
        .get("view")
        .and_then(Value::as_str)
        .and_then(Presentation::parse)
        .unwrap_or_default();
    let live = props.contains_key("bus");
    // Whether a view navigates defaults **per presentation**: the views the
    // catalog grew as editors navigate unless told not to, and a spectrum is
    // the watching spectroscope unless it is asked to navigate — its axis is
    // frequency, and a curve that pans under an idle drag is not what a
    // meter-like view has ever done.
    let navigable = props
        .get("navigable")
        .and_then(truthy)
        .unwrap_or(view != Presentation::Spectrum);
    let p = signal::point(view, live, navigable);
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
            retention: number(props, "retention", 0.0).max(0.0),
        })
    } else {
        signal::Source::Data(signal::Data {
            samples: inline_samples("signal", id, props, blobs)?,
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

    // The value axis: a named side wins, an unnamed one keeps the preset's
    // (which is `None` — auto-fitted — only where the name meant that).
    el.value = signal::ValueRange {
        min: opt_number(props, "min").or(p.value.min),
        max: opt_number(props, "max").or(p.value.max),
    };

    el.spectral = spectral_props(props, p.spectral, p.size_prop);

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
    Ok(el)
}
