//! Applying a `/gui_set` key/value to a live [`WidgetKind`] — the incremental
//! wire-to-schema update, one arm per widget type. Split out of the schema
//! ([`super`]) alongside [`super::build`] so the enum reads separately from the
//! two long wire matches; the shared setter helpers live in [`super::parse`].

use serde_json::Value;

use super::*;

/// Applies one `/gui_set` key/value to `widget` — its kind's own keys, plus,
/// for a `clip`, the props of the **bodies it holds as children**.
///
/// The routing exists because the wire has not moved yet: a script still
/// describes a clip as a thing with a take, notes and a curve, so `points`
/// arrives addressed to the clip and has to reach the child that owns it. A
/// body prop naming a body the clip does not have **creates** it, which is how
/// a script draws a curve over a take without rebuilding the def.
pub(super) fn apply_widget(widget: &mut Widget, key: &str, v: &Value) -> bool {
    if matches!(widget.kind, WidgetKind::Clip { .. }) {
        // The **active edit layer** is the one clip prop that is neither the
        // clip's own scalar nor a body's: it names one of the bodies, so it is
        // resolved against the container that holds them.
        // Which layers are **drawn**, the visualization half of the same
        // question: named the same way, resolved against the same stack.
        if key == "hidden" {
            let Some(names) = v.as_str() else {
                return false;
            };
            let Some(mut sel) = crate::host::layers::Selection::of(widget) else {
                return false;
            };
            sel.set_hidden(names);
            return true;
        }
        if key == "layer" {
            let Some(name) = v.as_str() else { return false };
            let Some(mut sel) = crate::host::layers::Selection::of(widget) else {
                return false;
            };
            let Some(layer) = sel.parse(name) else {
                return false;
            };
            sel.set(layer);
            return true;
        }
        // The **window** onto the samples is the node's, not the kind's: a
        // body may have one of its own, so it is set where both can be set the
        // same way.
        if matches!(key, "start" | "loop" | "fit") {
            return widget
                .window
                .get_or_insert_with(SourceWindow::default)
                .apply(key, v);
        }
        if !CLIP_OWN.contains(&key) {
            return apply_clip_body(widget, key, v);
        }
    }
    apply_kind(&mut widget.kind, key, v)
}

/// The keys a `clip` answers for itself; everything else it accepts belongs to
/// one of its bodies.
const CLIP_OWN: [&str; 8] = [
    "offset", "dur", "label", "layer", "hidden", "start", "loop", "fit",
];

/// Routes a body prop into the child that owns it, building that child first
/// when the clip does not have it yet. The **value axis** props are the awkward
/// ones and are stated here rather than guessed: `min`/`max` reach whichever
/// body measures with them (the take's amplitude, the roll's pitches), and
/// `points_min`/`points_max` are the curve's own.
fn apply_clip_body(widget: &mut Widget, key: &str, v: &Value) -> bool {
    use element::BodyRole::{Curve, Notes, Take};
    match key {
        // A source prop, the take's own axis, or a spectral take's **display**
        // — the dB window, the frequency scale and the colormap are shader
        // uniforms, so they are live on a clip exactly as they are on a view.
        // What is not here is what the picture is built from: `view`, the
        // analysis size and the hop are read when the clip is built, since the
        // texture is computed and allocated then.
        "data" | "blob" | "path" | "cache" | "buffer" | "channels" | "base_bucket" | "db_floor"
        | "db_ceil" | "freq_scale" | "log_freq" | "colormap" => {
            widget.ensure_body(Take);
            widget
                .clip_body_mut(Take)
                .is_some_and(|k| apply_kind(k, key, v))
        }
        "notes" => {
            widget.ensure_body(Notes);
            widget
                .clip_body_mut(Notes)
                .is_some_and(|k| apply_kind(k, key, v))
        }
        // **Whether a hand may edit this clip's bodies**, live: a lane locked
        // while it plays, a generator's clip that becomes editable the moment
        // it is rendered to a track. It reaches *every* body the clip carries,
        // because it is a statement about the clip and not about one of them.
        "editable" => {
            let mut landed = false;
            for role in [Notes, Curve] {
                if let Some(kind) = widget.clip_body_mut(role) {
                    landed |= apply_kind(kind, key, v);
                }
            }
            landed
        }
        // Each body's **own** editability, against the clip-wide `editable`
        // above: a roll that is a rendering of a generator cannot be written
        // while the envelope drawn over it can, and one key for both bodies
        // could not say it.
        "notes_editable" | "points_editable" => {
            let role = if key == "notes_editable" {
                Notes
            } else {
                Curve
            };
            widget
                .clip_body_mut(role)
                .is_some_and(|k| apply_kind(k, "editable", v))
        }
        "points" | "exp" => {
            widget.ensure_body(Curve);
            widget
                .clip_body_mut(Curve)
                .is_some_and(|k| apply_kind(k, key, v))
        }
        "points_min" | "points_max" => {
            let axis = if key == "points_min" { "min" } else { "max" };
            widget.ensure_body(Curve);
            widget
                .clip_body_mut(Curve)
                .is_some_and(|k| apply_kind(k, axis, v))
        }
        // The shared value axis: whichever bodies measure with it take it, and
        // the curve does not — it has `points_min`/`points_max` of its own.
        "min" | "max" => {
            let mut hit = false;
            for body in &mut widget.children {
                if matches!(body.kind.body_role(), Some(Take | Notes)) {
                    hit |= apply_kind(&mut body.kind, key, v);
                }
            }
            hit
        }
        _ => false,
    }
}

/// Applies one `/gui_set` key/value to `kind`, returning whether the key was one
/// this widget accepts (and thus changed it).
pub(super) fn apply_kind(kind: &mut WidgetKind, key: &str, v: &Value) -> bool {
    match kind {
        WidgetKind::Window {
            layout, flow, hug, ..
        }
        | WidgetKind::Panel { layout, flow, hug } => match key {
            "flow" => v
                .as_str()
                .and_then(Layout::from_str)
                .map(|l| *layout = l)
                .is_some(),
            "hug" => truthy(v).map(|b| *hug = b).is_some(),
            _ => flow.apply(key, v),
        },
        // The page shown, live: this is the prop a bound toggle or menu drives.
        // A non-number leaves it alone rather than blanking the stack.
        WidgetKind::Stack { index, margin, hug } => match key {
            "index" => v.as_i64().map(|n| *index = n as i32).is_some(),
            "margin" => {
                *margin = v.as_f64().map(|n| n as f32);
                true
            }
            "hug" => truthy(v).map(|b| *hug = b).is_some(),
            _ => false,
        },
        WidgetKind::Scroll { layout, flow, view } => match key {
            "flow" => v
                .as_str()
                .and_then(Layout::from_str)
                .map(|l| *layout = l)
                .is_some(),
            _ => view.apply(key, v) || flow.apply(key, v),
        },
        WidgetKind::Track {
            label,
            height,
            snap,
            header,
            editor,
        } => match key {
            "label" => set_label(label, v),
            "height" => set_f(height, v),
            "snap" => v.as_f64().map(|x| *snap = x.max(0.0)).is_some(),
            // The header: its width, and the controls it carries. Setting one
            // of the controls also *adds* it to a lane that had none, which is
            // how a script grows a header without rebuilding the def.
            "header_w" => {
                header.w = v.as_f64().map(|w| w as f32);
                true
            }
            "mute" => truthy(v).map(|b| header.mute = Some(b)).is_some(),
            "solo" => truthy(v).map(|b| header.solo = Some(b)).is_some(),
            "level" => v
                .as_f64()
                .map(|x| header.level = Some((x as f32).clamp(0.0, 1.0)))
                .is_some(),
            // The lane's chrome (`ruler`, `playhead_at`, the tick-label
            // props): a track is no timeline-group member, so these keys
            // land on the widget itself rather than routing through a group.
            _ => editor.apply(key, v),
        },
        // A clip's own props are its placement and its name; its bodies are
        // children, and their props route there — see `apply_clip`, which is
        // reached through `Widget::apply_kind` because it needs them.
        WidgetKind::Clip { offset, dur, label } => match key {
            "offset" => v.as_f64().map(|x| *offset = x.max(0.0)).is_some(),
            "dur" => v.as_f64().map(|x| *dur = x.max(0.0)).is_some(),
            "label" => set_label(label, v),
            _ => false,
        },
        // A free-standing ruler is its editor chrome and nothing else: the
        // unit it labels (`ruler`), the rate and the beat grid, the link that
        // joins it to the lanes. Without this arm a `/gui_set` of any of them
        // was recorded in the registry and never reached the drawing — a
        // script could not change the unit of the strip it had just built.
        WidgetKind::TimeRuler { editor } => editor.apply(key, v),
        WidgetKind::Custom(el) => el.set(key, v),
        _ => false,
    }
}
