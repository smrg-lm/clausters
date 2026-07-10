//! Pointer-interaction primitives over the widget tree — the value/hit logic
//! shared by both fronts.
//!
//! Hit-testing a point, reading and writing a control's value, flipping a toggle,
//! cycling a menu: all of it is pure work on the [`Host`]'s typed tree plus the
//! [`layout`] and [`controls`] math, with no platform dependency. The native
//! windowed front ([`super::gui`]) and the browser front ([`super::web`]) both
//! call these, so a turned knob updates the tree and decides bound-vs-event the
//! same way on either platform — only the event *source* (winit vs browser
//! pointer events) and the event *sink* (a socket vs the binding surface) differ.

use clausters_core::osc::OscType;

#[cfg(not(target_arch = "wasm32"))]
use super::bpf;
use super::layout::{self, Rect};
use super::widget::{Widget, WidgetKind};
use super::{Host, controls};

/// The deepest interactive widget under `(x, y)` in window `def_id`: its id, its
/// laid-out rect and a clone of its kind. Containers (`window`/`panel`) are not
/// hit targets. `fb_w`/`fb_h` is the window's framebuffer size in device pixels.
pub(crate) fn hit(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
) -> Option<(i32, Rect, WidgetKind)> {
    let tree = host.window_def(def_id)?;
    let area = Rect::new(0.0, 0.0, fb_w as f32, fb_h as f32);
    let mut found = None;
    for p in layout::layout(area, tree) {
        if p.rect.contains(x, y)
            && let Some(id) = p.widget.id
            && !matches!(
                p.widget.kind,
                WidgetKind::Window { .. } | WidgetKind::Panel { .. }
            )
        {
            found = Some((id, p.rect, p.widget.kind.clone()));
        }
    }
    found
}

/// The current 0..1 fraction of a continuous control (slider/knob/number) in the
/// host tree — the live value used to drive an incremental drag.
pub(crate) fn fraction_of(host: &Host, def_id: i32, widget_id: i32) -> Option<f32> {
    fn walk(w: &Widget, id: i32) -> Option<f32> {
        if w.id == Some(id) {
            return match &w.kind {
                WidgetKind::Slider { range: r, .. }
                | WidgetKind::Knob(r)
                | WidgetKind::Number(r) => Some(r.fraction()),
                _ => None,
            };
        }
        w.children.iter().find_map(|c| walk(c, id))
    }
    walk(host.window_def(def_id)?, widget_id)
}

/// Sets a continuous control's value from a 0..1 fraction, in the host tree.
pub(crate) fn set_fraction(host: &mut Host, def_id: i32, widget_id: i32, t: f32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(widget_id)
    {
        match &mut w.kind {
            WidgetKind::Slider { range: r, .. } | WidgetKind::Knob(r) | WidgetKind::Number(r) => {
                r.set_fraction(t)
            }
            _ => {}
        }
    }
}

/// Flips a `toggle`'s boolean state in the host tree.
pub(crate) fn flip_toggle(host: &mut Host, def_id: i32, id: i32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(id)
        && let WidgetKind::Toggle { value, .. } = &mut w.kind
    {
        *value = !*value;
    }
}

/// Advances a `menu`'s selected option (wrapping) in the host tree.
pub(crate) fn cycle_menu(host: &mut Host, def_id: i32, id: i32) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(id)
        && let WidgetKind::Menu { index, options, .. } = &mut w.kind
        && !options.is_empty()
    {
        *index = (*index + 1) % options.len();
    }
}

/// The current event value of widget `id` in `tree` (what a `/gui_event` or a
/// bound forward carries).
pub(crate) fn value_of(tree: &Widget, id: i32) -> Option<OscType> {
    fn walk(w: &Widget, id: i32) -> Option<OscType> {
        if w.id == Some(id) {
            return w.kind.event_value();
        }
        w.children.iter().find_map(|c| walk(c, id))
    }
    walk(tree, id)
}

/// Runs `f` over a `bpf` widget's model in the host tree — the one door every
/// envelope edit goes through, so the fronts never unpack the variant
/// themselves. `f` gets the breakpoints and the display mapping (the time
/// domain, the value range and the `exp` scale); its return value is passed
/// through (`None` when the widget is not a `bpf`). Editing gestures are
/// native-only today (the browser keeps display + `/gui_set` parity, the
/// editor-view posture), so the helpers are compiled out of the wasm build.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn bpf_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<bpf::BpfPoint>, f64, f32, f32, bool) -> R,
) -> Option<R> {
    let w = host.window_def_mut(def_id)?.find_mut(widget_id)?;
    match &mut w.kind {
        WidgetKind::Bpf {
            points,
            min,
            max,
            duration,
            exp,
            ..
        } => Some(f(points, *duration, *min, *max, *exp)),
        _ => None,
    }
}

/// A `bpf` widget's edit-back payload: the `"points"` tag plus the flat
/// breakpoint list (`t v shape curve` per point) — what a `/gui_event` carries
/// to the script, and what a bound editor forwards to the audio server.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn bpf_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    match &tree.find(id)?.kind {
        WidgetKind::Bpf { points, .. } => {
            let mut args = vec![OscType::String("points".into())];
            args.extend(bpf::points_args(points));
            Some(args)
        }
        _ => None,
    }
}

/// The 0..1 fraction a slider press/drag at `(cx, cy)` maps to, by orientation:
/// the cursor x along a horizontal track, or y (bottom = 0, top = 1) on a
/// `vertical` one.
pub(crate) fn slider_t(body: Rect, cx: f64, cy: f64, vertical: bool) -> f32 {
    if vertical {
        controls::slider_fraction_v(body, cy)
    } else {
        controls::slider_fraction(body, cx)
    }
}
