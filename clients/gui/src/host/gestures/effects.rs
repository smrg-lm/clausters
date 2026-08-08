//! What a gesture *delivers*: turning a widget's new value or edited payload
//! into the effects the front carries out.
//!
//! Every interaction that produces something the outside world should see ends
//! here, so the bound-vs-event decision (`/gui_bind` to the audio server or
//! another widget, else a `/gui_event` to the script) is made in one place
//! rather than at each gesture.

use clausters_core::osc::OscType;

use super::super::interact::{self, value_of};
use super::super::widget::Widget;
use super::super::{Host, HostEffect};
use super::GestureEffect;
use super::nav::group_view;

/// Emits `/gui_event widget_id <args…>` (as an effect for the front to send).
pub(super) fn emit(out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32, args: Vec<OscType>) {
    out.push(GestureEffect::Emit {
        def_id,
        widget_id,
        args,
    });
}

/// Routes a widget's new `value` where it is bound (`/gui_bind`: the audio
/// server on the low-latency path, or another widget's prop), or to the script
/// as a `/gui_event` otherwise. Every interaction that produces a value goes
/// through here, so a single binding check covers them all.
pub(super) fn deliver(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    value: OscType,
) {
    let mut effects = Vec::new();
    if host.forward(widget_id, value.clone(), &mut effects) {
        // Bound: the value went straight to its destination, and whatever the
        // apply behind a widget binding touched has to repaint.
        return redraws(out, effects);
    }
    emit(out, def_id, widget_id, vec![value]);
}

/// Turns the host effects an apply produced into gesture effects. A binding's
/// apply is a `/gui_set` without the wire, so the only thing it can ask for is
/// a repaint; anything else would be a window opening behind a knob turn, which
/// a binding has no business doing.
pub(super) fn redraws(out: &mut Vec<GestureEffect>, effects: Vec<HostEffect>) {
    for effect in effects {
        match effect {
            HostEffect::Redraw(root) => out.push(GestureEffect::Redraw(root)),
            other => tracing::warn!("a binding's apply asked for {other:?}, which it cannot do"),
        }
    }
}

/// Delivers a control's current value: straight to the audio server when the
/// widget is bound, otherwise as a `/gui_event` to the script.
pub(super) fn emit_value(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
) {
    if let Some(value) = host.window_def(def_id).and_then(|t| value_of(t, widget_id)) {
        deliver(host, out, def_id, widget_id, value);
    }
}

/// Delivers an edited flat structure — the edit-back pattern: a **bound**
/// widget forwards `args[1..]` (without the leading tag, which names the event
/// payload, not a server argument) straight to the audio server; an unbound one
/// emits the whole tagged list as a `/gui_event`.
pub(super) fn deliver_args(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    args: Option<Vec<OscType>>,
) {
    let Some(args) = args else {
        return;
    };
    if host.is_bound(widget_id) {
        let mut effects = Vec::new();
        host.forward_args(widget_id, args[1..].to_vec(), &mut effects);
        return redraws(out, effects);
    }
    emit(out, def_id, widget_id, args);
}

/// Delivers the tagged payload `read` finds for `widget_id` in the window's
/// tree — the whole of what every edit-back emitter below does, so each of them
/// is the name of a payload and nothing else, and the next one is a line rather
/// than another copy of this pair of statements.
pub(super) fn emit_read(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    read: impl FnOnce(&Widget, i32) -> Option<Vec<OscType>>,
) {
    let args = host.window_def(def_id).and_then(|t| read(t, widget_id));
    deliver_args(host, out, def_id, widget_id, args);
}

/// Delivers a `bpf`/automation-clip widget's edited breakpoint list.
pub(super) fn emit_points(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
) {
    emit_read(host, out, def_id, widget_id, interact::bpf_event_args);
}

/// Delivers a lane header control's new value (`"mute"`/`"solo"`/`"level"`).
pub(super) fn emit_lane(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    part: interact::HeaderPart,
) {
    emit_read(host, out, def_id, widget_id, |t, id| {
        interact::lane_event_args(t, id, part)
    });
}

/// Delivers a `clip`'s edited placement (`"clip" offset dur`).
pub(super) fn emit_clip(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
) {
    emit_read(host, out, def_id, widget_id, interact::clip_event_args);
}

/// Plays or releases one `piano` key: updates the held-key view state, drives
/// the host-managed voice when the widget is in voice mode, and delivers the
/// MIDI-shaped `"note" pitch velocity state channel` payload — to the audio
/// server when the piano is bound, to the script as a `/gui_event` otherwise.
#[allow(clippy::too_many_arguments)] // one note event, all scalars
pub(super) fn piano_note(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
    pitch: i32,
    velocity: i32,
    state: i32,
    channel: i32,
) {
    if state != 0 {
        interact::piano_press_key(host, def_id, widget_id, pitch);
        host.piano_voice_on(def_id, widget_id, pitch, velocity);
    } else {
        interact::piano_release_key(host, def_id, widget_id, pitch);
        host.piano_voice_off(widget_id, pitch);
    }
    deliver_args(
        host,
        out,
        def_id,
        widget_id,
        Some(interact::piano_note_args(pitch, velocity, state, channel)),
    );
}

/// Applies a `piano` range change (pan/zoom) and, when it actually moved,
/// emits the `"range" min max` event and repaints — the `"view"` posture on
/// the keyboard's own MIDI axis.
pub(super) fn set_piano_range(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    id: i32,
    min: i32,
    max: i32,
) {
    if let Some((min, max)) = interact::piano_set_range(host, def_id, id, min, max) {
        // Always an event, never a bound forward: a binding carries the note
        // payload, the range is view state (the timeline views' "view" posture).
        emit(
            out,
            def_id,
            id,
            vec![
                OscType::String("range".into()),
                OscType::Int(min),
                OscType::Int(max),
            ],
        );
        out.push(GestureEffect::Redraw(def_id));
    }
}

/// Delivers a piano-roll's edited notes (`"notes" start dur pitch vel ch …`).
pub(super) fn emit_notes(
    host: &mut Host,
    out: &mut Vec<GestureEffect>,
    def_id: i32,
    widget_id: i32,
) {
    emit_read(host, out, def_id, widget_id, interact::notes_event_args);
}

/// Delivers a piano-roll's edited OSC events (`"osc" time label …`).
pub(super) fn emit_osc(host: &mut Host, out: &mut Vec<GestureEffect>, def_id: i32, widget_id: i32) {
    emit_read(host, out, def_id, widget_id, interact::osc_event_args);
}

/// Repaints every window in `roots` (the windows a group mutation touched).
pub(super) fn redraw_all(out: &mut Vec<GestureEffect>, roots: &[i32]) {
    for root in roots {
        out.push(GestureEffect::Redraw(*root));
    }
}

/// Emits a timeline view's visible range as a `/gui_event id "view" start len`
/// — once per gesture step, carrying the interacted member's id (linked
/// members repaint but do not re-emit).
pub(super) fn emit_view(host: &Host, out: &mut Vec<GestureEffect>, def_id: i32, id: i32) {
    if let Some((start, len, _)) = group_view(host, id) {
        emit(
            out,
            def_id,
            id,
            vec![
                OscType::String("view".into()),
                OscType::Float(start as f32),
                OscType::Float(len as f32),
            ],
        );
    }
}
