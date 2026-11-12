//! **The read doors**: what an element currently holds, and the payload that
//! reports it.
//!
//! Two kinds of reader, and they are the same question asked at two moments:
//! the live value a drag starts from ([`plane_can_pan`]), and the **edit-back
//! payload** a finished edit sends -- a flat OSC list beginning with the tag
//! that names what changed, so a script and a bound forward read the same
//! message. The heavy views build their own; what is left here is the
//! containers'.
//!
//! A payload is deliberately built from the tree rather than from the gesture:
//! whatever the edit did, what leaves is what the element now *is*.

use super::super::Host;
use super::super::layout;
use super::super::layout::Rect;
use super::super::widget::{Axis, ScrollView};

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
