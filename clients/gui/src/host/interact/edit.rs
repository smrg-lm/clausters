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

/// **Drops the clip selection** across a window's whole tree, returning whether
/// anything was holding one.
///
/// A window-wide clear rather than a lane-wide one, because a selection is one
/// hand's: pressing an unselected clip on another lane lets go of what was
/// held, exactly as pressing an unselected note does in a roll.
pub(crate) fn clear_clip_selection(host: &mut Host, def_id: i32) -> bool {
    let mut changed = false;
    if let Some(tree) = host.window_def_mut(def_id) {
        tree.walk_mut(&mut |w| {
            if w.selected {
                w.selected = false;
                changed = true;
            }
        });
    }
    changed
}

/// Sets one clip's own mark, returning whether it changed.
pub(crate) fn set_clip_selected(host: &mut Host, def_id: i32, id: i32, on: bool) -> bool {
    match host.window_def_mut(def_id).and_then(|t| t.find_mut(id)) {
        Some(w) if w.selected != on => {
            w.selected = on;
            true
        }
        _ => false,
    }
}

/// **The marquee**: selects the clips of `lane_id` whose span meets the time
/// span `[t0, t1)`, dropping whatever was held elsewhere. Returns whether the
/// selection changed.
///
/// The rule is the roll's, one level up: a sweep sets the shared time selection
/// and the boxes inside it become the selected set — `placement::in_rect` over
/// the lane's clips, the same call `notes_in_rect` is. The vertical half of the
/// rectangle is the lane itself for now; a sweep across lanes is what the
/// vertical axis is for.
pub(crate) fn select_clips_in(
    host: &mut Host,
    def_id: i32,
    lane_id: i32,
    t0: f64,
    t1: f64,
) -> bool {
    let mut changed = clear_clip_selection(host, def_id);
    let Some(lane) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(lane_id))
    else {
        return changed;
    };
    let mut clips = crate::host::graphics::track::LaneClips::of(lane, 0.0);
    for i in crate::host::placement::in_rect(&clips, t0, t1, 0.0, 0.0) {
        changed |= clips.set_selected(i, true);
    }
    changed
}

/// Writes a clip's placement (`offset`/`dur`, each clamped `>= 0`) in the host
/// tree — the drag's mutation.
pub(crate) fn clip_set(host: &mut Host, def_id: i32, clip_id: i32, placed: super::Placement) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(clip_id)
    {
        crate::host::graphics::track::set_clip_placement(w, placed);
    }
}
