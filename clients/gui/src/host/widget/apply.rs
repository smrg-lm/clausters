//! Applying a `/gui_set` key/value to a live [`WidgetKind`] — the incremental
//! wire-to-schema update, one arm per widget type. Split out of the schema
//! ([`super`]) alongside [`super::build`] so the enum reads separately from the
//! two long wire matches; the shared setter helpers stay in the parent module.

use serde_json::Value;

use super::*;

/// Applies one `/gui_set` key/value to `kind`, returning whether the key was one
/// this widget accepts (and thus changed it).
pub(super) fn apply_kind(kind: &mut WidgetKind, key: &str, v: &Value) -> bool {
    match kind {
        WidgetKind::Window { layout, flow, .. } | WidgetKind::Panel { layout, flow } => match key {
            "layout" => v
                .as_str()
                .and_then(Layout::from_str)
                .map(|l| *layout = l)
                .is_some(),
            _ => flow.apply(key, v),
        },
        WidgetKind::Scroll { layout, flow, view } => match key {
            "layout" => v
                .as_str()
                .and_then(Layout::from_str)
                .map(|l| *layout = l)
                .is_some(),
            _ => view.apply(key, v) || flow.apply(key, v),
        },
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
            channels,
            overlay,
            window_ms,
            trigger,
            hold,
            min,
            max,
            ruler,
            ruler_y,
            label,
        } => match key {
            "bus" => v.as_i64().map(|n| *bus = n as i32).is_some(),
            "tap" => v.as_i64().map(|n| *tap = n as i32).is_some(),
            "channels" => v
                .as_i64()
                .map(|n| *channels = (n as usize).max(1))
                .is_some(),
            "overlay" => truthy(v).map(|b| *overlay = b).is_some(),
            "window_ms" => set_f(window_ms, v),
            "trigger" => set_f(trigger, v),
            "hold" => truthy(v).map(|b| *hold = b).is_some(),
            "min" => set_f(min, v),
            "max" => set_f(max, v),
            "ruler" => set_strip(ruler, v),
            "ruler_y" => set_strip(ruler_y, v),
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
            channels,
            fft_size,
            db_floor,
            db_ceil,
            freq_scale,
            averaging,
            peak_hold,
            ruler,
            ruler_y,
            label,
        } => match key {
            "tap" => v.as_i64().map(|n| *tap = n as i32).is_some(),
            "channels" => v
                .as_i64()
                .map(|n| *channels = (n as usize).max(1))
                .is_some(),
            "ruler" => set_strip(ruler, v),
            "ruler_y" => set_strip(ruler_y, v),
            "fft_size" => v
                .as_u64()
                .filter(|n| clausters_core::fft::supports(*n as usize))
                .map(|n| *fft_size = n as usize)
                .is_some(),
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
            view,
            overlay,
            sample_rate,
            min,
            max,
            ruler,
            ruler_y,
            fft_size,
            db_floor,
            db_ceil,
            freq_scale,
            label,
            ..
        } => {
            let handled = match key {
                // `min`/`max` also accept the string `"auto"` to give a
                // side back to the data fit.
                "min" => set_opt_f(min, v),
                "max" => set_opt_f(max, v),
                "view" => v
                    .as_str()
                    .and_then(super::plot::PlotView::parse)
                    .map(|k| *view = k)
                    .is_some(),
                "overlay" => truthy(v).map(|b| *overlay = b).is_some(),
                "sample_rate" => set_f64(sample_rate, v),
                "ruler" => ruler.set(v),
                "ruler_y" => match v.as_str() {
                    Some("off") | Some("none") => {
                        *ruler_y = false;
                        true
                    }
                    Some(_) => {
                        *ruler_y = true;
                        true
                    }
                    None => false,
                },
                "fft_size" => v.as_u64().map(|n| *fft_size = valid_fft_size(n)).is_some(),
                "db_floor" => set_f(db_floor, v),
                "db_ceil" => set_f(db_ceil, v),
                "freq_scale" => v
                    .as_str()
                    .and_then(freq_scale_from_str)
                    .map(|s| *freq_scale = s)
                    .is_some(),
                "label" => set_label(label, v),
                _ => false,
            };
            // The analysis reads the view, size and rate: keep it current.
            if handled && matches!(key, "view" | "fft_size" | "sample_rate") {
                kind.refresh_plot_analysis();
            }
            handled
        }
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
        WidgetKind::Score(data) => match key {
            // Locate the static playback cursor; a negative time hides it.
            "playhead" => v.as_f64().map(|t| data.playhead = t as f32).is_some(),
            // Anchor score time 0 to a sample-clock value: the cursor then
            // sweeps on its own, one message per pass instead of per frame.
            "playhead_at" => v.as_f64().map(|t| data.playhead_at = t).is_some(),
            "sample_rate" => v.as_f64().map(|r| data.sample_rate = r).is_some(),
            // Select an element by its MEI id; the empty string clears it.
            "selected" => v
                .as_str()
                .map(|s| data.selected = (!s.is_empty()).then(|| s.to_string()))
                .is_some(),
            _ => false,
        },
        WidgetKind::Slider { range: r, .. } | WidgetKind::Knob(r) | WidgetKind::Number(r) => {
            match key {
                "value" => set_f(&mut r.value, v),
                "min" => set_f(&mut r.min, v),
                "max" => set_f(&mut r.max, v),
                "label" => set_label(&mut r.label, v),
                "text_size" => set_size(&mut r.text_size, v),
                _ => false,
            }
        }
        WidgetKind::Toggle {
            value,
            label,
            text_size,
        } => match key {
            "value" => truthy(v).map(|b| *value = b).is_some(),
            "label" => set_label(label, v),
            "text_size" => set_size(text_size, v),
            _ => false,
        },
        WidgetKind::Text {
            value,
            label,
            text_size,
            multiline,
            caret,
        } => match key {
            "value" => v
                .as_str()
                .map(|s| {
                    *value = s.to_string();
                    // The caret/selection may now point past the new string
                    // or off a char boundary — re-land it.
                    super::textedit::clamp(value, caret);
                })
                .is_some(),
            "label" => set_label(label, v),
            "text_size" => set_size(text_size, v),
            "multiline" => truthy(v).map(|b| *multiline = b).is_some(),
            _ => false,
        },
        WidgetKind::Menu {
            index,
            options,
            label,
            text_size,
        } => match key {
            "index" => v
                .as_u64()
                .map(|n| *index = (n as usize).min(options.len().saturating_sub(1)))
                .is_some(),
            "label" => set_label(label, v),
            "text_size" => set_size(text_size, v),
            _ => false,
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
            editor,
        } => match key {
            "label" => set_label(label, v),
            "height" => set_f(height, v),
            "snap" => v.as_f64().map(|x| *snap = x.max(0.0)).is_some(),
            // The lane's chrome (`ruler`, `playhead_at`, the tick-label
            // props): a track is no timeline-group member, so these keys
            // land on the widget itself rather than routing through a group.
            _ => editor.apply(key, v),
        },
        WidgetKind::Clip {
            offset,
            dur,
            notes,
            points,
            exp,
            points_min,
            points_max,
            min,
            max,
            label,
            ..
        } => match key {
            "offset" => v.as_f64().map(|x| *offset = x.max(0.0)).is_some(),
            "dur" => v.as_f64().map(|x| *dur = x.max(0.0)).is_some(),
            "notes" => {
                *notes = parse_notes(&std::iter::once(("notes".to_string(), v.clone())).collect());
                true
            }
            "points" => match super::bpf::parse_points(v, *min, *max) {
                Some(parsed) => {
                    *points = parsed;
                    true
                }
                None => false,
            },
            "exp" => truthy(v).map(|b| *exp = b).is_some(),
            "points_min" => set_f(points_min, v),
            "points_max" => set_f(points_max, v),
            "min" => set_f(min, v),
            "max" => set_f(max, v),
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
        WidgetKind::Button { label, text_size } => match key {
            "label" => set_label(label, v),
            "text_size" => set_size(text_size, v),
            _ => false,
        },
        WidgetKind::Label {
            text,
            text_size,
            wrap,
            align,
        } => match key {
            "text" => v.as_str().map(|s| *text = s.to_string()).is_some(),
            "text_size" => set_size(text_size, v),
            "wrap" => truthy(v).map(|b| *wrap = b).is_some(),
            "align" => v
                .as_str()
                .and_then(Align::from_str)
                .map(|a| *align = a)
                .is_some(),
            _ => false,
        },
        _ => false,
    }
}
