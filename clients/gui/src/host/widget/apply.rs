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
    if matches!(widget.kind, WidgetKind::Clip { .. }) && !CLIP_OWN.contains(&key) {
        return apply_clip_body(widget, key, v);
    }
    apply_kind(&mut widget.kind, key, v)
}

/// The keys a `clip` answers for itself; everything else it accepts belongs to
/// one of its bodies.
const CLIP_OWN: [&str; 3] = ["offset", "dur", "label"];

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
        WidgetKind::Window { layout, flow, .. } | WidgetKind::Panel { layout, flow } => match key {
            "flow" => v
                .as_str()
                .and_then(Layout::from_str)
                .map(|l| *layout = l)
                .is_some(),
            _ => flow.apply(key, v),
        },
        // The page shown, live: this is the prop a bound toggle or menu drives.
        // A non-number leaves it alone rather than blanking the stack.
        WidgetKind::Stack { index, margin } => match key {
            "index" => v.as_i64().map(|n| *index = n as i32).is_some(),
            "margin" => {
                *margin = v.as_f64().map(|n| n as f32);
                true
            }
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
        WidgetKind::Patch {
            patch,
            selected,
            label,
        } => match key {
            // The whole patch at once (its parts are arrays, and a `/gui_set`
            // value is a scalar — so they ride as their JSON, like `points`).
            "boxes" | "cords" => {
                // A `/gui_set` value is a scalar, so an array rides as its
                // JSON string (the `points` carrier, again).
                let value = match v {
                    Value::String(s) => match serde_json::from_str::<Value>(s) {
                        Ok(parsed) => parsed,
                        Err(_) => return false,
                    },
                    other => other.clone(),
                };
                let props = std::iter::once((key.to_string(), value)).collect();
                let parsed = parse_patch(&props);
                match key {
                    "boxes" if !parsed.boxes.is_empty() => patch.boxes = parsed.boxes,
                    "cords" => patch.cords = parsed.cords,
                    _ => return false,
                }
                // The box selection would dangle over a replaced `boxes` list.
                if key == "boxes" {
                    selected.clear();
                }
                true
            }
            "label" => set_label(label, v),
            _ => false,
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
        WidgetKind::PianoRoll {
            notes,
            osc,
            selected,
            min,
            max,
            snap,
            velocity_lane,
            osc_lane,
            midi_in,
            label,
            editor,
        } => match key {
            // Arrays ride a `/gui_set` as their JSON (a scalar wire value),
            // exactly like the clip's `notes`/`points` and the patch's parts.
            "notes" => {
                *notes = parse_notes(&as_array_props("notes", v));
                // The indices would dangle over the new list.
                selected.clear();
                true
            }
            "osc" => {
                *osc = parse_osc(&as_array_props("osc", v));
                true
            }
            "min" => set_f(min, v),
            "max" => set_f(max, v),
            "snap" => v.as_f64().map(|x| *snap = x.max(0.0)).is_some(),
            "velocity" => truthy(v).map(|b| *velocity_lane = b).is_some(),
            "osc_lane" => truthy(v).map(|b| *osc_lane = b).is_some(),
            "midi_in" => truthy(v).map(|b| *midi_in = b).is_some(),
            "label" => set_label(label, v),
            // The editor chrome (ruler, selection, playhead, the pitch
            // window `y_start`/`y_len`, `link`, view keys) — routed to the
            // group model by the host `on_set` for the timeline keys.
            _ => editor.apply(key, v),
        },
        // A free-standing ruler is its editor chrome and nothing else: the
        // unit it labels (`ruler`), the rate and the beat grid, the link that
        // joins it to the lanes. Without this arm a `/gui_set` of any of them
        // was recorded in the registry and never reached the drawing — a
        // script could not change the unit of the strip it had just built.
        WidgetKind::TimeRuler { editor } => editor.apply(key, v),
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
            label,
        } => match key {
            // A range change re-normalizes (min white-snapped) and drops
            // held keys that left the visible window (their rects are gone;
            // the release gesture tolerates the miss).
            "min" => v
                .as_i64()
                .map(|n| {
                    *min = super::piano::snap_white_down((n as i32).clamp(0, 127).min(*max));
                    pressed.retain(|p| *p >= *min);
                })
                .is_some(),
            "max" => v
                .as_i64()
                .map(|n| {
                    *max = (n as i32).clamp(0, 127).max(*min);
                    pressed.retain(|p| *p <= *max);
                })
                .is_some(),
            "active_min" => v.as_i64().map(|n| *active_min = n as i32).is_some(),
            "active_max" => v.as_i64().map(|n| *active_max = n as i32).is_some(),
            "pan" => truthy(v).map(|b| *pan = b).is_some(),
            "overview" => truthy(v).map(|b| *overview = b).is_some(),
            // A negative velocity restores the dynamic (press-height) map.
            "velocity" => v
                .as_i64()
                .map(|n| *velocity = (n >= 0).then(|| (n as i32).clamp(1, 127)))
                .is_some(),
            "channel" => v
                .as_i64()
                .map(|n| *channel = (n as i32).clamp(0, 15))
                .is_some(),
            // An empty string leaves voice mode (events only).
            "voice" => v
                .as_str()
                .map(|s| *voice = (!s.is_empty()).then(|| s.to_string()))
                .is_some(),
            "voice_args" => {
                *voice_args = parse_voice_args(&as_array_props("voice_args", v));
                true
            }
            "label" => set_label(label, v),
            _ => false,
        },
        // A registered element answers for its own props, with the same
        // contract every arm above has: `false` is "not my key", which the
        // host logs rather than swallows.
        WidgetKind::Custom(el) => el.set(key, v),
        _ => false,
    }
}
