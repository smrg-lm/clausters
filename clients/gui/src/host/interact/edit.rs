//! **The write doors**: every mutation of the host tree a gesture, a keystroke
//! or a `/gui_set` performs.
//!
//! Each element gets *one* door, and both fronts go through it — which is what
//! keeps a turned knob, a dragged break-point or a moved clip meaning the same
//! thing natively and in a page. Two shapes recur: a setter that writes the
//! value ([`set_fraction`], [`clip_set`], [`piano_set_range`]) and a
//! `…_edit`/`…_curve` door that hands a closure the element's own model
//! ([`text_edit`], [`pianoroll_notes_edit`]) so the fronts never
//! unpack a [`WidgetKind`] variant themselves.
//!
//! What a write *reports* is not here: the edit-back payloads live in
//! [`read`](super::read), so the mutation and the message it produces stay
//! separable.

use super::super::layout::{self, Rect};
use super::super::widget::WidgetKind;
use super::super::{Host, pianoroll, track};
use super::HeaderPart;
use super::coords::CanvasAt;

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

/// Runs `f` over a `text` field's `(value, caret)` in the host tree — the one
/// door every keystroke and click goes through, so the fronts never unpack the
/// variant themselves. `f`'s
/// return value is passed through (`None` when the widget is gone or not a
/// `text` field).
pub(crate) fn text_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut String, &mut super::super::textedit::Caret, bool) -> R,
) -> Option<R> {
    match host.widget_kind_mut(def_id, widget_id)? {
        WidgetKind::Text {
            value,
            caret,
            multiline,
            ..
        } => Some(f(value, caret, *multiline)),
        _ => None,
    }
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

/// Completes a cord drag on a patch: the grabbed `port` is paired with
/// the port under the cursor into a directed cord (`outlet → inlet`, matching
/// rate, either grab order), added to the patch (deduped). Returns the edit as
/// `(from_box, outlet, to_box, inlet)` with the port *names* — the payload of the
/// directed `"wire"` event a script mirrors. `None` when the release is not on a
/// compatible port (empty space, the same side, or a rate mismatch).
pub(crate) fn graph_cord(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    port: (usize, super::super::patch::Side, usize),
    at: CanvasAt,
) -> Option<(usize, String, usize, String)> {
    let WidgetKind::Patch { patch, .. } = host.widget_kind_mut(def_id, widget_id)? else {
        return None;
    };
    let drop = super::super::patch::port_hit(at.area, patch, at.cx, at.cy, at.scale)?;
    let cord = super::super::patch::cord_between(patch, port, drop)?;
    let outlet = patch
        .boxes
        .get(cord.from)?
        .outlets
        .get(cord.from_out)?
        .name
        .clone();
    let inlet = patch
        .boxes
        .get(cord.to)?
        .inlets
        .get(cord.to_in)?
        .name
        .clone();
    if !patch.cords.contains(&cord) {
        patch.cords.push(cord);
    }
    Some((cord.from, outlet, cord.to, inlet))
}

/// Sets a patch's selected set (the click/marquee gestures' write).
pub(crate) fn graph_select(host: &mut Host, def_id: i32, widget_id: i32, set: Vec<usize>) {
    if let Some(w) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(widget_id))
        && let WidgetKind::Patch { selected, .. } = &mut w.kind
    {
        *selected = set;
    }
}

/// Moves `patch` boxes to explicit canvas positions (the move drag's write):
/// each `(index, x, y)` lands on the box's `x`/`y`, making an auto-placed box's
/// position explicit from its first drag.
pub(crate) fn graph_move(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    moves: &[(usize, f32, f32)],
) {
    let Some(w) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(widget_id))
    else {
        return;
    };
    let WidgetKind::Patch { patch, .. } = &mut w.kind else {
        return;
    };
    for &(i, x, y) in moves {
        if let Some(o) = patch.boxes.get_mut(i) {
            (o.x, o.y) = (Some(x), Some(y));
        }
    }
}

/// Sets a `patch`'s selection to the boxes intersecting the marquee between
/// `a` and `b` (device pixels) — the box-selection drag, live on every move.
pub(crate) fn graph_marquee(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    area: Rect,
    a: (f64, f64),
    b: (f64, f64),
    scale: f32,
) {
    let Some(w) = host
        .window_def_mut(def_id)
        .and_then(|t| t.find_mut(widget_id))
    else {
        return;
    };
    let WidgetKind::Patch {
        patch, selected, ..
    } = &mut w.kind
    else {
        return;
    };
    let sel = super::super::gestures::corner_rect(a, b);
    let overlaps = |r: Rect| {
        r.x < sel.x + sel.w && sel.x < r.x + r.w && r.y < sel.y + sel.h && sel.y < r.y + r.h
    };
    *selected = (0..patch.boxes.len())
        .filter(|&i| overlaps(super::super::patch::obj_rect(area, patch, i, scale)))
        .collect();
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
/// tree — the drag's mutation, the sibling of [`set_fraction`].
pub(crate) fn clip_set(
    host: &mut Host,
    def_id: i32,
    clip_id: i32,
    new_offset: Option<f64>,
    new_dur: Option<f64>,
) {
    if let Some(tree) = host.window_def_mut(def_id)
        && let Some(w) = tree.find_mut(clip_id)
        && let WidgetKind::Clip { offset, dur, .. } = &mut w.kind
    {
        if let Some(o) = new_offset {
            *offset = o.max(0.0);
        }
        if let Some(d) = new_dur {
            *dur = d.max(0.0);
        }
    }
}

/// Mutate a piano-roll's note list in the host tree (the drag's write path, the
/// Returns `None` when the widget is gone or not a
/// piano-roll.
pub(crate) fn pianoroll_notes_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<pianoroll::Note>) -> R,
) -> Option<R> {
    match host.widget_kind_mut(def_id, widget_id)? {
        WidgetKind::PianoRoll { notes, .. } => Some(f(notes)),
        _ => None,
    }
}

/// Mutate a piano-roll's notes **and** its multi-note selection together (the
/// block edits' write path: a marquee fills the selection, a block move/delete/
/// velocity nudge reads it while rewriting the notes).
pub(crate) fn pianoroll_state_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<pianoroll::Note>, &mut Vec<usize>) -> R,
) -> Option<R> {
    match host.widget_kind_mut(def_id, widget_id)? {
        WidgetKind::PianoRoll {
            notes, selected, ..
        } => Some(f(notes, selected)),
        _ => None,
    }
}

/// Drops the element selection a container holds — the multi-note set of a
/// piano-roll today. The sweep's opening move: a new marquee starts from
/// nothing.
pub(crate) fn clear_element_selection(host: &mut Host, def_id: i32, id: i32) {
    pianoroll_state_edit(host, def_id, id, |_, sel| sel.clear());
}

/// Selects the container's elements inside the swept rectangle — time along the
/// shared axis, value along the vertical one. The container decides what an
/// element is: a piano-roll's notes today, and whatever a timeline container
/// places on its axis next.
pub(crate) fn select_elements_in_rect(
    host: &mut Host,
    def_id: i32,
    id: i32,
    time: (f64, f64),
    value: (f64, f64),
) {
    pianoroll_state_edit(host, def_id, id, |notes, sel| {
        *sel = pianoroll::notes_in_rect(notes, time.0, time.1, value.0 as f32, value.1 as f32);
    });
}

/// Mutate a piano-roll's OSC-event list in the host tree.
pub(crate) fn pianoroll_osc_edit<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<pianoroll::OscMark>) -> R,
) -> Option<R> {
    match host.widget_kind_mut(def_id, widget_id)? {
        WidgetKind::PianoRoll { osc, .. } => Some(f(osc)),
        _ => None,
    }
}

/// Mark a piano key held (the press/glissando write path). `true` when the key
/// was not already held.
pub(crate) fn piano_press_key(host: &mut Host, def_id: i32, widget_id: i32, pitch: i32) -> bool {
    piano_state(host, def_id, widget_id, |pressed| {
        if pressed.contains(&pitch) {
            false
        } else {
            pressed.push(pitch);
            true
        }
    })
    .unwrap_or(false)
}

/// Mark a piano key released. `true` when it was held.
pub(crate) fn piano_release_key(host: &mut Host, def_id: i32, widget_id: i32, pitch: i32) -> bool {
    piano_state(host, def_id, widget_id, |pressed| {
        let before = pressed.len();
        pressed.retain(|&p| p != pitch);
        pressed.len() != before
    })
    .unwrap_or(false)
}

/// Run `f` over a piano's held-key set in the host tree.
fn piano_state<R>(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    f: impl FnOnce(&mut Vec<i32>) -> R,
) -> Option<R> {
    match host.widget_kind_mut(def_id, widget_id)? {
        WidgetKind::Piano { pressed, .. } => Some(f(pressed)),
        _ => None,
    }
}

/// Write a piano's visible range (the pan/zoom write path): the min white-snaps,
/// held keys that left the window drop. Returns the applied range when it
/// actually changed (`None` for a no-op or a non-piano widget).
pub(crate) fn piano_set_range(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    new_min: i32,
    new_max: i32,
) -> Option<(i32, i32)> {
    let WidgetKind::Piano {
        min, max, pressed, ..
    } = host.widget_kind_mut(def_id, widget_id)?
    else {
        return None;
    };
    let nm = super::super::piano::snap_white_down(new_min.clamp(0, 127).min(new_max));
    let nx = new_max.clamp(0, 127).max(nm);
    if (nm, nx) == (*min, *max) {
        return None;
    }
    *min = nm;
    *max = nx;
    pressed.retain(|p| (nm..=nx).contains(p));
    Some((nm, nx))
}

/// Select an engraved element on a `score` by its MEI `xml:id` (`None` clears
/// the selection). Returns `true` when the selection actually changed, so a
/// re-click on the element already selected costs no repaint and no event.
pub(crate) fn score_select(host: &mut Host, def_id: i32, widget_id: i32, id: Option<&str>) -> bool {
    let Some(data) = score_data(host, def_id, widget_id) else {
        return false;
    };
    if data.selected.as_deref() == id {
        return false;
    }
    data.selected = id.map(str::to_string);
    true
}

/// Draw `element` displaced by `steps` diatonic steps — the pitch drag in
/// flight. Returns whether the displacement changed, so the crossing of each
/// step repaints and the pixels between them cost nothing.
pub(crate) fn score_drag(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
    element: &str,
    steps: i32,
) -> bool {
    let Some(data) = score_data(host, def_id, widget_id) else {
        return false;
    };
    let drag = super::super::score::ScoreDrag {
        id: element.to_string(),
        steps,
    };
    if data.drag.as_ref() == Some(&drag) {
        return false;
    }
    data.drag = Some(drag);
    true
}

/// End a pitch drag, returning the steps to report when it moved the element.
///
/// A drag that ended where it started retires here — there is nothing to ask
/// the client for. One that moved **keeps its displacement drawn**: the host
/// owns no notation, so it cannot re-engrave the page itself, and dropping the
/// preview now would show the old pitch until the client's answer arrives. The
/// page it sends back retires the preview (see the `display_list` prop).
pub(crate) fn score_drag_end(host: &mut Host, def_id: i32, widget_id: i32) -> Option<i32> {
    let data = score_data(host, def_id, widget_id)?;
    let steps = data.drag.as_ref()?.steps;
    if steps == 0 {
        data.drag = None;
        return None;
    }
    Some(steps)
}

/// The `score` widget `widget_id` in def `def_id`, if that is what it is — the
/// one lookup the score's doors share.
fn score_data(
    host: &mut Host,
    def_id: i32,
    widget_id: i32,
) -> Option<&mut super::super::score::ScoreData> {
    match host.widget_kind_mut(def_id, widget_id)? {
        WidgetKind::Score(data) => Some(data),
        _ => None,
    }
}
