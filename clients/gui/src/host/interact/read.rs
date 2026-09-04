//! **The read doors**: what an element currently holds, and the payload that
//! reports it.
//!
//! Two kinds of reader, and they are the same question asked at two moments.
//! The live value a drag starts from ([`plane_can_pan`]) —
//! and the **edit-back payload** a finished edit sends
//! ([`clip_event_args`], [`lane_event_args`], …), each a
//! flat OSC list beginning with the tag that names what changed, so a script
//! and a bound forward read the same message.
//!
//! A payload is deliberately built from the tree rather than from the gesture:
//! whatever the edit did, what leaves is what the element now *is*.

use super::super::Host;
use super::super::layout;
use super::super::layout::Rect;
use super::super::widget::{Axis, ScrollView, Widget, WidgetKind};
use crate::host::graphics::track;
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
/// **What a lane's selected clips report after a block edit**: `"clips" id
/// offset dur start …`, one quadruple per clip, in the order the lane draws
/// them.
///
/// It is the plural of `"clip"` and means exactly the same thing about each
/// clip it names — the same three numbers, read the same way. It is a payload
/// of its own rather than one `"clip"` per clip because **one gesture is one
/// edit**: a block moved by one hand undoes in one step, and a run of separate
/// messages gives the owner no way to know that. The owner applies them as one
/// transaction.
pub(crate) fn clips_event_args(tree: &Widget, lanes: &[i32]) -> Option<Vec<OscType>> {
    let mut args = vec![OscType::String("clips".into())];
    for lane in lanes.iter().filter_map(|id| tree.find(*id)) {
        for w in &lane.children {
            let WidgetKind::Clip { offset, dur, .. } = &w.kind else {
                continue;
            };
            let (Some(id), true) = (w.id, w.selected) else {
                continue;
            };
            args.push(OscType::Int(id));
            args.push(OscType::Float(*offset as f32));
            args.push(OscType::Float(*dur as f32));
            args.push(OscType::Float(w.window.unwrap_or_default().start as f32));
        }
    }
    (args.len() > 1).then_some(args)
}

/// **Where a clip now is**: `"lane" lane offset dur start` — the placement plus
/// the lane it has moved to.
///
/// It is `"clip"` with the lane in front, and it is a payload of its own for
/// the reason the plural one is: what the owner has to do differs. A `"clip"`
/// is one placement inside the aggregate it already belonged to; this is the
/// clip **leaving** one aggregate and joining another, which is two
/// `setmembers` in one transaction — and an owner that read it as a plain move
/// would put the clip at the right time on the wrong lane.
pub(crate) fn clip_lane_event_args(tree: &Widget, id: i32, lane_id: i32) -> Option<Vec<OscType>> {
    let mut args = clip_event_args(tree, id)?;
    args[0] = OscType::String("lane".into());
    args.insert(1, OscType::Int(lane_id));
    Some(args)
}

pub(crate) fn clip_event_args(tree: &Widget, id: i32) -> Option<Vec<OscType>> {
    let widget = tree.find(id)?;
    let window = widget.window.unwrap_or_default();
    match &widget.kind {
        WidgetKind::Clip { offset, dur, .. } => Some(vec![
            OscType::String("clip".into()),
            OscType::Float(*offset as f32),
            OscType::Float(*dur as f32),
            // Where the clip now reads its contents: a trim moves it, and an
            // owner told only the offset and the duration would re-cut the
            // wrong part of the source.
            OscType::Float(window.start as f32),
        ]),
        _ => None,
    }
}
