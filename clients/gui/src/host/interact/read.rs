//! **The read doors**: what an element currently holds, and the payload that
//! reports it.
//!
//! Two kinds of reader, and they are the same question asked at two moments.
//! The live value a drag starts from ([`fraction_of`], [`value_of`],
//! [`piano_key_active`]) — and the **edit-back payload** a finished edit sends
//! ([`clip_event_args`], [`bpf_event_args`], [`notes_event_args`], …), each a
//! flat OSC list beginning with the tag that names what changed, so a script
//! and a bound forward read the same message.
//!
//! A payload is deliberately built from the tree rather than from the gesture:
//! whatever the edit did, what leaves is what the element now *is*.

use super::super::layout;
use super::super::layout::Rect;
use super::super::widget::{Axis, ScrollView, Widget, WidgetKind};
use super::super::{Host, bpf, track};
use clausters_core::osc::OscType;

/// Whether plane `id` has anywhere to pan: its content is bigger than the
/// window on an axis it may move, or it is a free plane (which always has its
/// slack). A pinned plane declines a pan so the press walks on.
pub(crate) fn plane_can_pan(
    host: &Host,
    def_id: i32,
    id: i32,
    area: Rect,
    view: ScrollView,
) -> bool {
    let metrics = *host.metrics_for(def_id);
    let Some(tree) = host.window_def(def_id) else {
        return false;
    };
    let Some(w) = tree.find(id) else {
        return false;
    };
    let content = layout::scroll_content(w, area, &metrics);
    if view.axis.slack() > 0.0 {
        return true; // a free plane is unbounded by construction
    }
    let zoom = view.zoom(&metrics);
    let room = |content: f32, viewport: f32| content as f64 > viewport as f64 / zoom + 0.5;
    match view.axis {
        Axis::X => room(content.0, area.w),
        Axis::Y => room(content.1, area.h),
        _ => room(content.0, area.w) || room(content.1, area.h),
    }
}

/// The current event value of widget `id` in `tree` (what a `/gui_event` or a
/// bound forward carries).
pub(crate) fn value_of(tree: &Widget, id: i32) -> Option<OscType> {
    tree.find(id)?.kind.event_value()
}

/// A break-point curve's edit-back payload: the `"points"` tag plus the flat
/// breakpoint list (`t v shape curve` per point) — what a `/gui_event` carries
/// to the script, and what a bound editor forwards to the audio server. Shared
/// by the `bpf` view and the **automation clip**, whose curve is the same model
/// placed on a lane: one payload, so a script (or an `Automation`) consumes an
/// edit without caring which view drew it.
pub(crate) fn bpf_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    let widget = tree.find(id)?;
    // A clip's curve is a `bpf` **body** of it, so both cases are the one
    // element — the clip is only what the script addresses.
    let points = match widget.kind_or_body(is_curve)? {
        WidgetKind::Bpf { points, .. } => points,
        _ => return None,
    };
    let mut args = vec![OscType::String("points".into())];
    args.extend(bpf::points_args(points));
    Some(args)
}

/// Whether a kind is a clip's automation-curve body.
pub(super) fn is_curve(kind: &WidgetKind) -> bool {
    matches!(kind, WidgetKind::Bpf { .. })
}

/// Whether a kind is a clip's note-roll body.
pub(super) fn is_roll(kind: &WidgetKind) -> bool {
    matches!(kind, WidgetKind::PianoRoll { .. })
}

/// A lane header control's edit-back payload: the control's own name plus its
/// value (`"mute" 0|1`, `"solo" 0|1`, `"level" f`) — flat OSC primitives, the
/// same shape as the clip's `"clip"` payload. The name is the **prop** the
/// script would set, so a driver mirrors an edit by echoing it back.
pub(crate) fn lane_event_args(
    tree: &Widget,
    id: i32,
    part: track::HeaderPart,
) -> Option<Vec<OscType>> {
    let WidgetKind::Track { header, .. } = &tree.find(id)?.kind else {
        return None;
    };
    let flag = |tag: &str, on: Option<bool>| {
        Some(vec![OscType::String(tag.into()), OscType::Int(on? as i32)])
    };
    match part {
        track::HeaderPart::Mute => flag("mute", header.mute),
        track::HeaderPart::Solo => flag("solo", header.solo),
        track::HeaderPart::Fader => Some(vec![
            OscType::String("level".into()),
            OscType::Float(header.level?),
        ]),
    }
}

/// A clip's edit-back payload: the `"clip"` tag plus the new `offset`/`dur` —
/// what a `/gui_event` carries to the script (and what a bound clip would
/// forward). Flat OSC primitives, the same pattern as the `bpf` `"points"`
/// payload.
pub(crate) fn clip_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    match &tree.find(id)?.kind {
        WidgetKind::Clip { offset, dur, .. } => Some(vec![
            OscType::String("clip".into()),
            OscType::Float(*offset as f32),
            OscType::Float(*dur as f32),
        ]),
        _ => None,
    }
}

/// A piano-roll's notes edit-back payload: the `"notes"` tag plus the flat
/// quintuple list (`start dur pitch velocity channel` per note) — the wire form
/// the `pianoroll` and `clip` share. A `/gui_event` carries it to the script; a
/// bound editor forwards it (minus the tag) to the audio server.
pub(crate) fn notes_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    let notes = match tree.find(id)?.kind_or_body(is_roll)? {
        WidgetKind::PianoRoll { notes, .. } => notes,
        _ => return None,
    };
    let mut args = vec![OscType::String("notes".into())];
    for n in notes {
        args.push(OscType::Float(n.start as f32));
        args.push(OscType::Float(n.dur as f32));
        args.push(OscType::Float(n.pitch));
        args.push(OscType::Int(n.velocity));
        args.push(OscType::Int(n.channel));
    }
    Some(args)
}

/// A piano-roll's OSC-events edit-back payload: the `"osc"` tag plus the flat
/// `time label` pairs (an empty string when a marker has no label).
pub(crate) fn osc_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    let WidgetKind::PianoRoll { osc, .. } = &tree.find(id)?.kind else {
        return None;
    };
    let mut args = vec![OscType::String("osc".into())];
    for m in osc {
        args.push(OscType::Float(m.time as f32));
        args.push(OscType::String(m.label.clone().unwrap_or_default()));
    }
    Some(args)
}

/// Whether a piano key is inside the widget's active (non-grayed) range — a
/// press outside it is inert.
pub(crate) fn piano_key_active(host: &Host, def_id: i32, widget_id: i32, pitch: i32) -> bool {
    match host.widget_kind(def_id, widget_id) {
        Some(WidgetKind::Piano {
            active_min,
            active_max,
            ..
        }) => (*active_min..=*active_max).contains(&pitch),
        _ => false,
    }
}

/// A piano note event's payload — the MIDI-shaped
/// `"note" pitch velocity state channel` flat list (state 1 = on, 0 = off): a
/// `/gui_event` carries it to the script; a bound piano forwards it (minus the
/// tag) to the audio server; a future MIDI consumer translates it 1:1 to
/// note-on/note-off.
pub(crate) fn piano_note_args(pitch: i32, velocity: i32, state: i32, channel: i32) -> Vec<OscType> {
    vec![
        OscType::String("note".into()),
        OscType::Int(pitch),
        OscType::Int(velocity),
        OscType::Int(state),
        OscType::Int(channel),
    ]
}

/// A score selection event's payload — `"element" <xml:id>`, the empty string
/// meaning the selection was cleared. The id is the MEI one the client engraved
/// from, so a driver looks the element straight up in its own score.
pub(crate) fn score_element_args(id: Option<&str>) -> Vec<OscType> {
    vec![
        OscType::String("element".into()),
        OscType::String(id.unwrap_or_default().into()),
    ]
}

/// A score pitch edit's payload — `"transpose" <xml:id> <steps>`, the element
/// moved that many **diatonic** steps up the staff (negative = down). Steps,
/// not a position: the client transposes by steps
/// (`clausters.gui.notation.Score.transpose`), and a step is exact where a
/// page coordinate would need the engraver's frame.
pub(crate) fn score_transpose_args(id: &str, steps: i32) -> Vec<OscType> {
    vec![
        OscType::String("transpose".into()),
        OscType::String(id.into()),
        OscType::Int(steps),
    ]
}
