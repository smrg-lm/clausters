//! **The write doors**: every mutation of the host tree a gesture, a keystroke
//! or a `/gui_set` performs.
//!
//! Each element gets *one* door, and both fronts go through it — which is what
//! keeps a turned knob, a dragged break-point or a moved clip meaning the same
//! thing natively and in a page. Two shapes recur: a setter that writes the
//! value ([`clip_set`], [`header_set`]) and a
//! `…_edit` door that hands a closure the element's own model, so the fronts
//! never unpack a [`WidgetKind`] variant themselves.
//!
//! What a write *reports* is not here: the edit-back payloads live in
//! [`read`](super::read), so the mutation and the message it produces stay
//! separable.

use super::super::Host;
use super::super::layout::{self, Rect};
use super::super::widget::WidgetKind;
use super::HeaderPart;
use crate::host::graphics::track;

/// Sets a `scroll`'s view state (clamped against its content in `area`),
/// returning the clamped `(view_x, view_y, view_zoom)` when something actually
/// moved — the one door every scroll navigation goes through, so a gesture and
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

/// The thickness a lane may be dragged between (logical pixels): thin enough to
/// pack a big arrangement into a window, tall enough that a clip's body is still
/// a drawing.
const LANE_H: (f32, f32) = (24.0, 600.0);

/// Scales a **lane's** thickness by `factor` (Ctrl+wheel), returning its new
/// `h` in logical pixels — `None` when `id` is not a lane, or when the change
/// would leave the bounds it is already at.
///
/// `drawn` is the height it is on screen, in logical pixels, which is what a
/// lane that never named an `h` is measured from: its thickness becomes a
/// number on the wire the moment the wheel gives it one, and from then on it is
/// the number that moves.
pub(crate) fn lane_resize(
    host: &mut Host,
    def_id: i32,
    id: i32,
    drawn: f32,
    factor: f32,
) -> Option<f32> {
    let tree = host.window_def_mut(def_id)?;
    let widget = tree.find_mut(id)?;
    if !matches!(widget.kind, WidgetKind::Track { .. }) {
        return None;
    }
    let from = widget.place.h.unwrap_or(drawn).max(1.0);
    let to = (from * factor).clamp(LANE_H.0, LANE_H.1);
    if (to - from).abs() < 0.5 {
        return None;
    }
    widget.place.h = Some(to);
    Some(to)
}

/// Runs `f` over a lane's header in the host tree — the one door a header
/// control's edit goes through.
pub(crate) fn lane_header<R>(
    host: &mut Host,
    def_id: i32,
    lane_id: i32,
    f: impl FnOnce(&mut track::Header) -> R,
) -> Option<R> {
    match host.widget_kind_mut(def_id, lane_id)? {
        WidgetKind::Track { header, .. } => Some(f(header)),
        _ => None,
    }
}

/// Toggles a lane's mute or solo, or sets its level from a cursor x over the
/// fader `rect` — the header's three writes, through the one door.
pub(crate) fn header_set(
    host: &mut Host,
    def_id: i32,
    lane_id: i32,
    part: HeaderPart,
    fader: Option<(Rect, f64)>,
) {
    lane_header(host, def_id, lane_id, |h| match part {
        HeaderPart::Mute => h.mute = Some(h.mute != Some(true)),
        HeaderPart::Solo => h.solo = Some(h.solo != Some(true)),
        HeaderPart::Fader => {
            if let Some((rect, cx)) = fader {
                h.level = Some(track::level_at(rect, cx));
            }
        }
    });
}

/// Writes a clip's placement (`offset`/`dur`, each clamped `>= 0`) in the host
/// tree — the drag's mutation.
pub(crate) fn clip_set(host: &mut Host, def_id: i32, clip_id: i32, placed: super::Placement) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(clip_id)
    {
        if let WidgetKind::Clip { offset, dur, .. } = &mut w.kind {
            *offset = placed.offset.max(0.0);
            *dur = placed.dur.max(0.0);
        } else {
            return;
        }
        // The window travels with the placement: a trimmed start shows less of
        // the contents from further in, which is the whole difference between
        // trimming a clip and squeezing it.
        w.window
            .get_or_insert_with(crate::host::widget::SourceWindow::default)
            .start = placed.start;
    }
}
