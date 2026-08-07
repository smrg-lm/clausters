//! Applying a `/gui_set` key/value to a live [`WidgetKind`] — the incremental
//! wire-to-schema update, one arm per widget type. Split out of the schema
//! ([`super`]) alongside [`super::build`] so the enum reads separately from the
//! two long wire matches; the shared setter helpers live in [`super::parse`].

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
        // Every signal element, in one arm: the props are the model's own —
        // the source, the value axis, the spectral parameters, the chrome —
        // and a key a presentation does not read is simply not one of them.
        WidgetKind::Signal(el) => apply_signal(el, key, v),
        WidgetKind::Meter {
            bus,
            rate,
            min,
            max,
            label,
        } => match key {
            "bus" => v.as_i64().map(|n| *bus = n as i32).is_some(),
            "rate" => set_rate(rate, v),
            "min" => set_f(min, v),
            "max" => set_f(max, v),
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
            // Replace the engraved page in place — the answer to an edit, and
            // the reason a score does not have to be redefined to change. Only
            // the drawing travels: the chrome (playhead, selection) is the
            // host's own state and survives, so the note the user is editing
            // stays selected across the round trip. The drag preview is what
            // this page *is* now, so it retires here.
            "display_list" => match as_props(v) {
                Some(props) => {
                    let page = super::score::ScoreData::parse(&props);
                    let keep = std::mem::replace(data, page);
                    data.playhead = keep.playhead;
                    data.playhead_at = keep.playhead_at;
                    data.playhead_loop_start = keep.playhead_loop_start;
                    data.playhead_loop_len = keep.playhead_loop_len;
                    data.sample_rate = keep.sample_rate;
                    data.selected = keep.selected;
                    // A re-engraved page carries only the drawing; whether the
                    // widget edits is the host's own state, like the chrome, so
                    // an editor stays an editor across the round trip.
                    data.editable = keep.editable;
                    true
                }
                None => false,
            },
            // Locate the static playback cursor; a negative time hides it.
            "playhead" => v.as_f64().map(|t| data.playhead = t as f32).is_some(),
            // Anchor score time 0 to a sample-clock value: the cursor then
            // sweeps on its own, one message per pass instead of per frame.
            "playhead_at" => v.as_f64().map(|t| data.playhead_at = t).is_some(),
            // Wrap the sweep inside a repeated passage (ms; <= 0 length = the
            // straight pass).
            "playhead_loop_start" => v
                .as_f64()
                .map(|t| data.playhead_loop_start = t as f32)
                .is_some(),
            "playhead_loop_len" => v
                .as_f64()
                .map(|t| data.playhead_loop_len = t as f32)
                .is_some(),
            "sample_rate" => v.as_f64().map(|r| data.sample_rate = r).is_some(),
            // Select an element by its MEI id; the empty string clears it.
            "selected" => v
                .as_str()
                .map(|s| data.selected = (!s.is_empty()).then(|| s.to_string()))
                .is_some(),
            // Turn editing on or off live (a view that becomes an editor, or the
            // reverse). A drag only transposes while this is true.
            "editable" => v.as_bool().map(|b| data.editable = b).is_some(),
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

/// Applies one `/gui_set` key/value to a signal element. The keys are grouped
/// the way the model is — source, value axis, spectral parameters, chrome —
/// so a key lands wherever it means something, whatever the element's wire
/// name was. The analysis inputs re-run the cached analysis at the end, which
/// is the only mutation point a `/gui_set` can be.
fn apply_signal(el: &mut signal::SignalElement, key: &str, v: &Value) -> bool {
    let handled = match key {
        // The source.
        "bus" => match el.source.bus_mut() {
            Some(b) => v.as_i64().map(|n| b.bus = n as i32).is_some(),
            None => false,
        },
        "rate" => match el.source.bus_mut() {
            Some(b) => set_rate(&mut b.rate, v),
            None => false,
        },
        "window_ms" => match el.source.bus_mut() {
            Some(b) => set_f(&mut b.window_ms, v),
            None => false,
        },
        "trigger" => match el.source.bus_mut() {
            Some(b) => set_f(&mut b.trigger, v),
            None => false,
        },
        "hold" => match el.source.bus_mut() {
            Some(b) => truthy(v).map(|x| b.hold = x).is_some(),
            None => false,
        },
        "channels" => match v.as_i64() {
            Some(n) => {
                let n = (n as usize).max(1);
                match &mut el.source {
                    signal::Source::Bus(b) => b.channels = n,
                    signal::Source::Data(d) => d.channels = n,
                }
                true
            }
            None => false,
        },
        // The presentation, where the element's name reads one.
        "view" => v
            .as_str()
            .and_then(super::plot::PlotView::parse)
            .map(|view| {
                el.presentation = match view {
                    super::plot::PlotView::Signal => Presentation::Signal,
                    super::plot::PlotView::Spectrum => Presentation::Spectrum,
                };
            })
            .is_some(),
        // The value axis. Either side also accepts the string `"auto"`, giving
        // it back to the data fit.
        "min" => set_opt_f(&mut el.value.min, v),
        "max" => set_opt_f(&mut el.value.max, v),
        // The spectral parameters. The analysis size answers to both names —
        // the spectral views say `fft_size`, the time-frequency one
        // `window_size` — since one field is behind them.
        "fft_size" | "window_size" => v
            .as_u64()
            .filter(|n| clausters_core::fft::supports(*n as usize))
            .map(|n| el.spectral.fft_size = n as usize)
            .is_some(),
        "db_floor" => set_f(&mut el.spectral.db_floor, v),
        "db_ceil" => set_f(&mut el.spectral.db_ceil, v),
        "freq_scale" => v
            .as_str()
            .and_then(freq_scale_from_str)
            .map(|s| el.spectral.freq_scale = s)
            .is_some(),
        // Legacy boolean alias: 1 -> log, 0 -> linear.
        "log_freq" => truthy(v)
            .map(|b| el.spectral.freq_scale = if b { FreqScale::Log } else { FreqScale::Linear })
            .is_some(),
        "averaging" => v
            .as_f64()
            .map(|x| el.spectral.averaging = (x as f32).clamp(0.0, 0.99))
            .is_some(),
        "peak_hold" => truthy(v).map(|b| el.spectral.peak_hold = b).is_some(),
        "colormap" => v
            .as_i64()
            .map(|n| el.spectral.colormap = n as i32)
            .is_some(),
        // The chrome.
        "overlay" => truthy(v).map(|b| el.display.overlay = b).is_some(),
        "label" => set_label(&mut el.display.label, v),
        _ => el.editor.apply(key, v),
    };
    // The cached analysis reads the presentation, the size and the rate.
    if handled && matches!(key, "view" | "fft_size" | "window_size" | "sample_rate") {
        el.refresh_analysis();
    }
    handled
}
