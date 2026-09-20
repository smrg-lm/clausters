//! **The write doors**: every mutation of the host tree a gesture, a keystroke
//! or a `/gui_set` performs.
//!
//! Each element gets *one* door, and both fronts go through it -- which is what
//! keeps a turned knob, a dragged break-point or a moved box meaning the same
//! thing natively and in a page. Two shapes recur: a setter that writes the
//! value ([`scroll_set_view`]) and a `…_edit` door that hands a closure the
//! element's own model, so the fronts never unpack a [`WidgetKind`] variant
//! themselves.
//!
//! What a write *reports* is not here: the edit-back payloads live in
//! [`read`](super::read), so the mutation and the message it produces stay
//! separable.

use super::super::Host;
use super::super::layout::{self, Rect};
use super::super::widget::WidgetKind;

/// Sets a `scroll`'s view state (clamped against its content in `area`),
/// returning the clamped `(view_x, view_y, view_zoom)` when something actually
/// moved -- the one door every scroll navigation goes through, so a gesture and
/// a `/gui_set` clamp identically.
pub(crate) fn scroll_set_view(
    host: &mut Host,
    def_id: i32,
    id: i32,
    area: Rect,
    (vx, vy, zoom): (f64, f64, f64),
) -> Option<(f64, f64, f64)> {
    let metrics = *host.metrics_for(def_id);
    let zoom = super::super::scroll::clamp_zoom(zoom);
    let tree = host.window_def_mut(def_id)?;
    let content = layout::scroll_content(tree.find(id)?, area, &metrics);
    let w = tree.find_mut(id)?;
    let WidgetKind::Scroll { view, .. } = &mut w.kind else {
        return None;
    };
    let slack = view.axis.slack();
    let next = (
        super::super::scroll::clamp_pan(vx, area.w, zoom, content.0, slack),
        super::super::scroll::clamp_pan(vy, area.h, zoom, content.1, slack),
        zoom,
    );
    if next == (view.view_x, view.view_y, view.zoom(&metrics)) {
        return None;
    }
    // Writing the zoom makes it explicit: from here on this plane's scale is the
    // number, not the window's density (see `ScrollView::zoom`).
    (view.view_x, view.view_y, view.view_zoom) = (next.0, next.1, Some(next.2));
    Some(next)
}
