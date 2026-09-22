//! Constructing a typed [`WidgetKind`] from a generic GuiDef node -- the
//! wire-to-schema pass, one arm per widget type. Split out of the schema
//! ([`super`]) so the enum and the (long) construction match read separately;
//! the shared prop-reading helpers live in [`super::parse`], visible here as a
//! sibling of the `widget` module tree.

use serde_json::{Map, Value};

use super::super::elements::signal::Presentation;
use super::*;

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
    _has_children: bool,
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
            // On unless the window says otherwise: see `host::status::shown`.
            status: props.get("status").and_then(truthy).unwrap_or(true),
        },
        // A container with no axes. `stack` -- one child at a time, the one
        // `index` names -- is one of the arrangements rather than a type of its
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
        // is not compiled and both readings are the workspace below -- the one
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
        // **One meaning left**: the free-standing ruler over a navigation
        // group. `field` used to be three things told apart by what was on it
        // -- a lane, a clip placed on its parent's axis, or this -- and the
        // first two are `multitrack` props now: a lane cannot sit in a
        // void, so it is always inside the view that owns it.
        "field" => WidgetKind::TimeRuler {
            editor: EditorProps::parse_ruler(props),
        },
        // No arm above answers to this name, so it is an **element**: a
        // built-in that has moved behind the trait, else whatever a program
        // registered, else nothing at all. The order is the invariant, not a
        // detail -- a built-in resolves first, so a registration can never
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

/// The spectral parameters a signal names, over `base` (its preset's): the
/// analysis size under whichever name this presentation calls it (`size_prop`
/// -- `fft_size` for the spectra, `window_size` for the time-frequency
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
/// `navigable` name a [`point`](signal::point) of the presentation * source *
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
    // the watching spectroscope unless it is asked to navigate -- its axis is
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
    // (which is `None` -- auto-fitted -- only where the name meant that).
    el.value = signal::ValueRange {
        min: opt_number(props, "min").or(p.value.min),
        max: opt_number(props, "max").or(p.value.max),
    };

    el.spectral = spectral_props(props, p.spectral, p.size_prop);

    // **The layer stack**: the pictures drawn on the one body, back to front.
    // Two names for one field, as `fft_size`/`window_size` already are -- the
    // general `layers` and `measure`, which is what a stack of nothing but
    // measures has always been called. An unknown name keeps the preset's,
    // which is the protocol's posture everywhere else: a value a build does
    // not know reads as a prop that was not set.
    if let Some(stack) = props
        .get("layers")
        .or_else(|| props.get("measure"))
        .and_then(signal::Stack::parse)
    {
        // **The axis claim is checked here, and it is the one thing a stack
        // can get wrong.** Two layers on the body's vertical measuring two
        // different quantities is refused rather than resolved: whichever of
        // them lost would be drawn on a scale that is not its own, which is a
        // picture that lies about what it shows.
        stack
            .axis_domain()
            .map_err(|why| format!("signal: {why}"))?;
        el.layers = stack;
    }

    // The loudness layer's scale and guides, read at construction the way
    // `/gui_set` reads them: the parameters of a measure live with the measure.
    if let Some(x) = opt_number(props, "loudness_target") {
        el.loudness.target = x as f64;
    }
    if let Some(x) = opt_number(props, "loudness_scale").filter(|x| *x > 0.0) {
        el.loudness.scale = x as f64;
    }
    if let Some(b) = props.get("loudness_ruler").and_then(truthy) {
        el.loudness.ruler = b;
    }
    if let Some(b) = props.get("loudness_stats").and_then(truthy) {
        el.loudness.stats = b;
    }

    el.display = signal::Display {
        overlay: props.get("overlay").and_then(truthy).unwrap_or(false),
        label: label(props),
    };
    // The y strip's default unit is the axis' own: hertz where a texture is on
    // it, the value unit where the traces are -- so a stack that put a wave on
    // a spectrogram's body is ruled in the unit the wave is read in.
    let ruler_y = match el.axis_domain() {
        signal::Domain::Frequency => RulerY::Hz,
        _ if p.ruler_y == RulerY::Hz => RulerY::Norm,
        _ => p.ruler_y,
    };
    el.editor = EditorProps::parse(props, ruler_y);
    el.editor.ruler = Ruler::parse_with(props, p.ruler);
    // **Air above full scale, on the axis that measures amplitude.** A
    // time-frequency picture's vertical is hertz and stops at Nyquist; a
    // trace's is a value, and a floating-point signal can leave the +/-1 the
    // domain names -- so that one axis can be opened past its domain.
    // It follows the **stack's axis**, not the presentation: a body ruled in
    // hertz is a body with a texture on its axis, whatever else is drawn over
    // it.
    el.editor.y_headroom = if el.axis_domain() == signal::Domain::Frequency {
        1.0
    } else {
        crate::viewport::AMP_HEADROOM
    };

    // The capabilities the preset welded to the name, as the props they are:
    // whether a view navigates, carries a selection or edits back is a choice
    // over any presentation, and the six names only ever froze six of them.
    let cap = |key: &str, default: bool| props.get(key).and_then(truthy).unwrap_or(default);
    // Not a capability but the same shape of answer: whether the buffer is
    // being written into as it is drawn, which only the client knows.
    el.fills = props.get("fills").and_then(truthy).unwrap_or(false);
    el.caps = signal::Caps {
        navigable: cap("navigable", el.caps.navigable),
        selectable: cap("selectable", el.caps.selectable),
        editable: cap("editable", el.caps.editable),
    };

    el.refresh_analysis();
    // Inline samples are measured here, which is the element's first mutation
    // point: a def that names a loudness measure draws its curve on the frame
    // it is built for, not on the one after.
    el.refresh_loudness();
    Ok(el)
}
