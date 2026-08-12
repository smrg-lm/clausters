//! Drawing and pointer-math for the standard control widgets.
//!
//! Each control is built from the painter's flat primitives plus bitmap text, so
//! it needs no GPU code of its own: a `slider` is a track with a handle, a `knob`
//! a disc with a pointer, a `button`/`toggle`/`menu`/`number` a labelled box. The
//! drawing lives here; the *value* math a drag turns into (which fraction of a
//! slider's track the cursor is at, how far a vertical drag moves a knob) is
//! exposed as pure functions so it is unit-testable without a window. The
//! interaction *state* (what is pressed, drag anchors) lives in the windowed
//! front, which calls these.

use super::textedit;
use crate::host::font;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::{Color, Draw, Mesh};
use crate::host::widget::size;
use crate::host::widget::{Align, Range};

/// The label strip height when a control carries a label, else 0 — **and 0 when
/// the cell cannot hold both**.
///
/// A squeezed control drops its label before it squeezes its body: a field box
/// shorter than the text inside it is a drawing that lies (the glyphs stand
/// clear of their own background), while a value with no caption is merely
/// terser. Every geometry here goes through this, so the strip that is not
/// reserved is also not drawn.
fn label_height(rect_h: f32, has_label: bool, text_size: f32, m: &Metrics) -> f32 {
    let strip = font::height(text_size) + m.pad;
    if has_label && rect_h - strip - 2.0 * m.pad >= font::height(text_size) {
        strip
    } else {
        0.0
    }
}

/// The control body at the host's own text scale (the views that keep the fixed
/// label scale). Shared by drawing and hit-math so they agree on the track.
pub fn body_rect(rect: Rect, has_label: bool, m: &Metrics) -> Rect {
    body_rect_at(rect, has_label, m.text_scale, m)
}

/// The control body (the rect minus its label strip — sized by the widget's
/// `text_size` — and a small inset).
pub fn body_rect_at(rect: Rect, has_label: bool, text_size: f32, m: &Metrics) -> Rect {
    let top = rect.y + label_height(rect.h, has_label, text_size, m);
    Rect::new(
        rect.x + m.pad,
        top + m.pad,
        (rect.w - 2.0 * m.pad).max(0.0),
        (rect.y + rect.h - top - 2.0 * m.pad).max(0.0),
    )
}

/// The strip a control reserves at the **bottom** of its body for its value
/// read-out: one row of text and the gap above it. The number lives there and
/// nowhere else, so it never lands on the thing it is reading — a knob's disc,
/// a slider's groove and handle.
pub fn readout_h(text_size: f32, m: &Metrics) -> f32 {
    font::height(text_size) + m.pad
}

/// A slider's **track area**: its body minus the read-out strip
/// ([`readout_h`]), the rect the groove and the handle live in.
///
/// Drawing and hit math both go through here, so a grab lands where the groove
/// is drawn. That matters most on a *vertical* slider, whose fraction runs along
/// the very axis the strip shortens: a body-wide hit would read the bottom of
/// the number row as the minimum, below where the track visibly ends.
pub fn slider_track(rect: Rect, has_label: bool, text_size: f32, m: &Metrics) -> Rect {
    let body = body_rect_at(rect, has_label, text_size, m);
    Rect::new(
        body.x,
        body.y,
        body.w,
        (body.h - readout_h(text_size, m)).max(0.0),
    )
}

/// **The band a slider is actually drawn in**, inside its track area — the
/// groove down the middle plus the handle's grip across it, which is as thick
/// as this control ever gets.
///
/// The track area is the whole cell minus the label and the read-out, and the
/// slider fills a fraction of it: a groove `track_thick` across with a grip
/// `handle_grip` long over it, centred. So the area answers for a control that
/// is drawn several times thinner than it, which is why the press reads this
/// instead — the hit is the drawing, the same rule the dial and the notehead
/// follow (`graphics::shape`).
pub fn slider_groove(body: Rect, vertical: bool, m: &Metrics) -> Rect {
    if vertical {
        let t = m.track_thick.max(m.handle_grip.min(body.w));
        Rect::new(body.x + (body.w - t) * 0.5, body.y, t, body.h)
    } else {
        let t = m.track_thick.max(m.handle_grip.min(body.h));
        Rect::new(body.x, body.y + (body.h - t) * 0.5, body.w, t)
    }
}

/// The 0..1 fraction along a horizontal track at pixel `px` (for a slider).
pub fn slider_fraction(body: Rect, px: f64) -> f32 {
    if body.w <= 0.0 {
        return 0.0;
    }
    (((px as f32) - body.x) / body.w).clamp(0.0, 1.0)
}

/// The 0..1 fraction along a vertical track at pixel `py` — bottom is 0, top is
/// 1, so dragging up raises the value (for a `vertical` slider).
pub fn slider_fraction_v(body: Rect, py: f64) -> f32 {
    if body.h <= 0.0 {
        return 0.0;
    }
    (1.0 - ((py as f32) - body.y) / body.h).clamp(0.0, 1.0)
}

/// The fraction change from a vertical drag of `dy` device pixels over a body of
/// height `body_h`: dragging up (negative `dy`) increases the value. A full body
/// height is one full range; the factor keeps fine control on tall bodies.
pub fn drag_fraction_delta(dy: f64, body_h: f32) -> f32 {
    let span = body_h.max(40.0);
    (-(dy as f32)) / span
}

/// Draws the label strip above a control body, if it has a label (clipped to
/// the cell with an ellipsis).
fn label_strip(d: &mut Draw, label: Option<&str>, rect: Rect, size: f32) {
    let (mesh, m, theme) = d.parts();
    if label_height(rect.h, label.is_some(), size, m) <= 0.0 {
        return; // no room for both: the body keeps the cell
    }
    if let Some(text) = label {
        font::text_ellipsis(
            mesh,
            text,
            rect.x + m.pad,
            rect.y + m.pad,
            (rect.w - 2.0 * m.pad).max(0.0),
            size,
            theme.text,
        );
    }
}

pub fn slider(d: &mut Draw, r: &Range, rect: Rect, vertical: bool, size: f32) {
    label_strip(d, r.label.as_deref(), rect, size);
    // The groove lives in the track area, the number in the strip under it (the
    // knob's posture): the read-out is beside what it reads, never over it.
    let body = slider_track(rect, r.label.is_some(), size, d.m);
    let (mesh, m, theme) = d.parts();
    // The track's groove, the value riding it and the handle's grip across the
    // axis: the handle is a short grip, **not** the full body span.
    let (track_thick, handle_thick) = (m.track_thick, m.handle_thick);
    let f = r.fraction();
    if vertical {
        // Track down the centre; min at the bottom, max at the top.
        let cx = body.x + body.w * 0.5;
        mesh.rect(
            Rect::new(cx - track_thick * 0.5, body.y, track_thick, body.h),
            theme.track,
        );
        let hy = body.y + body.h * (1.0 - f);
        mesh.rect(
            Rect::new(
                cx - track_thick * 0.5,
                hy,
                track_thick,
                (body.y + body.h - hy).max(0.0),
            ),
            theme.accent_dim,
        );
        let grip = m.handle_grip.min(body.w);
        mesh.rect(
            Rect::new(cx - grip * 0.5, hy - handle_thick * 0.5, grip, handle_thick),
            theme.accent,
        );
    } else {
        let mid = body.y + body.h * 0.5;
        mesh.rect(
            Rect::new(body.x, mid - track_thick * 0.5, body.w, track_thick),
            theme.track,
        );
        let hx = body.x + body.w * f;
        mesh.rect(
            Rect::new(
                body.x,
                mid - track_thick * 0.5,
                (hx - body.x).max(0.0),
                track_thick,
            ),
            theme.accent_dim,
        );
        let grip = m.handle_grip.min(body.h);
        mesh.rect(
            Rect::new(
                hx - handle_thick * 0.5,
                mid - grip * 0.5,
                handle_thick,
                grip,
            ),
            theme.accent,
        );
    }
    let readout = Rect::new(body.x, body.y + body.h, body.w, readout_h(size, m));
    value_text(d, &fmt(r.value), readout, size);
}

/// **Where a knob's dial is**, as `(cx, cy, radius)` inside its `body` — one
/// function so the drawing and the hit-test cannot disagree about the disc.
///
/// The read-out takes a strip at the bottom of the body and the disc is sized
/// in the area above it, so the number stays inside the body: it never overlaps
/// the disc nor spills past the cell into the row below.
pub fn knob_disc(body: Rect, size: f32, m: &Metrics) -> (f32, f32, f32) {
    let disc_h = (body.h - readout_h(size, m)).max(0.0);
    (
        body.x + body.w * 0.5,
        body.y + disc_h * 0.5,
        (body.w.min(disc_h) * 0.5 - 2.0).max(2.0),
    )
}

pub fn knob(d: &mut Draw, r: &Range, rect: Rect, size: f32) {
    label_strip(d, r.label.as_deref(), rect, size);
    let body = body_rect_at(rect, r.label.is_some(), size, d.m);
    let (cx, cy, radius) = knob_disc(body, size, d.m);
    let (mesh, m, theme) = d.parts();
    let text_h = readout_h(size, m);
    mesh.disc(cx, cy, radius, theme.track);
    mesh.disc(cx, cy, radius - 3.0, theme.field);
    // Pointer: 270-degree sweep, min at lower-left, max at lower-right.
    let angle = (135.0 + 270.0 * r.fraction()).to_radians();
    let tip = [cx + radius * angle.cos(), cy + radius * angle.sin()];
    mesh.line([cx, cy], tip, 3.0, theme.accent);
    let readout = Rect::new(body.x, body.y + body.h - text_h, body.w, text_h);
    value_text(d, &fmt(r.value), readout, size);
}

pub fn number(d: &mut Draw, r: &Range, rect: Rect, size: f32) {
    label_strip(d, r.label.as_deref(), rect, size);
    let body = body_rect_at(rect, r.label.is_some(), size, d.m);
    let (mesh, m, theme) = d.parts();
    mesh.rect(body, theme.field);
    // A vertical fill rising from the bottom shows the value in range, so
    // dragging up raises the green level; a border frames the field.
    let fill_h = body.h * r.fraction();
    mesh.rect(
        Rect::new(body.x, body.y + body.h - fill_h, body.w, fill_h),
        theme.accent_dim,
    );
    border(mesh, body, m.divider_w, theme.accent);
    font::text_centered(mesh, &fmt(r.value), body, size, theme.text);
}

/// Draws a `label` into its rect: the line block vertically centered, each
/// line placed by `align`. With `wrap` the text word-wraps on the font's
/// fixed advance and lines past the rect's bottom are dropped; without it the
/// single line clips with an ellipsis instead of bleeding into a neighbor.
pub fn draw_label(d: &mut Draw, text: &str, rect: Rect, size: f32, wrap: bool, align: Align) {
    let (mesh, m, theme) = d.parts();
    let left = rect.x + m.pad;
    let avail = (rect.w - 2.0 * m.pad).max(0.0);
    if wrap {
        let lines = font::wrap(text, avail, size);
        let advance = font::line_advance(size);
        let block_h = lines.len() as f32 * advance;
        let mut y = (rect.y + (rect.h - block_h) * 0.5).max(rect.y);
        for (i, line) in lines.iter().enumerate() {
            // The first line always draws; later lines drop once they overflow.
            if i > 0 && y + font::height(size) > rect.y + rect.h {
                break;
            }
            let x = align_x(align, left, avail, font::width(line, size));
            font::text(mesh, line, x, y, size, theme.text);
            y += advance;
        }
    } else {
        let y = (rect.y + (rect.h - font::height(size)) * 0.5).max(rect.y);
        let x = align_x(align, left, avail, font::width(text, size).min(avail));
        font::text_ellipsis(mesh, text, x, y, avail, size, theme.text);
    }
}

/// The x a line of width `tw` starts at inside `[left, left + avail]`.
fn align_x(align: Align, left: f32, avail: f32, tw: f32) -> f32 {
    match align {
        Align::Start => left,
        Align::Center => left + (avail - tw) * 0.5,
        Align::End => left + avail - tw,
    }
    .max(left)
}

/// A 1-color outline of `rect`, `w` pixels thick (four thin rects).
fn border(mesh: &mut Mesh, rect: Rect, w: f32, color: Color) {
    mesh.rect(Rect::new(rect.x, rect.y, rect.w, w), color);
    mesh.rect(Rect::new(rect.x, rect.y + rect.h - w, rect.w, w), color);
    mesh.rect(Rect::new(rect.x, rect.y, w, rect.h), color);
    mesh.rect(Rect::new(rect.x + rect.w - w, rect.y, w, rect.h), color);
}

pub fn button(d: &mut Draw, label: Option<&str>, rect: Rect, active: bool, size: f32) {
    let (mesh, _m, theme) = d.parts();
    // A button *is* its box, so it fills its whole cell rather than insetting a
    // `body_rect` the way a slider/field does (whose track must not touch the
    // cell edge). The layout `gap` already separates it from its neighbours, and
    // the full cell is also its hit area, so drawing and click now agree. Without
    // this the box shrank to the text height inside a control bar and floated in
    // dead space.
    mesh.rect(
        rect,
        if active {
            theme.hilite
        } else {
            theme.accent_dim
        },
    );
    font::text_centered(mesh, label.unwrap_or("BUTTON"), rect, size, theme.text);
}

/// **The box a toggle is drawn as**, inside its cell: a square of at most
/// `box_side`, centred vertically at the left of the rect.
pub fn toggle_box(rect: Rect, m: &Metrics) -> Rect {
    let side = rect.h.min(rect.w).min(m.box_side);
    Rect::new(rect.x, rect.y + (rect.h - side) * 0.5, side, side)
}

/// **What a toggle answers the pointer on**: the box, plus the label beside it
/// when there is one — the two things drawn, and nothing of the cell the
/// layout stretched around them.
///
/// A checkbox is a small square with a word next to it, and a cell it does not
/// fill on **either** axis: a row of controls is as tall as the tallest of
/// them, so a toggle beside a slider gets a column of air over and under its
/// box as well as the run of it after the label. Both are the layout's, not the
/// control's, so a click landing on them goes back to the chain — the first
/// pass here bounded only the width, which is exactly half a fix and reads as
/// none at all in the panel that showed it.
///
/// The label counts because it is drawn as part of the affordance (clicking the
/// word toggles, as everywhere else), and it counts for **what fits**: a label
/// ellipsized to the cell is hit over the part that is on screen. Its line is
/// centred on the box, so the band is the taller of the two.
pub fn toggle_hit(rect: Rect, label: Option<&str>, size: f32, m: &Metrics) -> Rect {
    let b = toggle_box(rect, m);
    let Some(text) = label else { return b };
    let tx = b.x + b.w + m.pad;
    let w = font::width(text, size).min((rect.x + rect.w - tx).max(0.0));
    let h = b.h.max(font::height(size)).min(rect.h);
    Rect::new(
        rect.x,
        rect.y + (rect.h - h) * 0.5,
        (tx + w - rect.x).max(b.w),
        h,
    )
}

pub fn toggle(d: &mut Draw, on: bool, label: Option<&str>, rect: Rect, size: f32) {
    let box_rect = toggle_box(rect, d.m);
    let (mesh, m, theme) = d.parts();
    // Like `button`, the toggle draws its box and its label at the left of the
    // cell; the layout gap does the separating.
    let body = rect;
    let box_side = box_rect.w;
    mesh.rect(box_rect, theme.track);
    if on {
        let inset = box_side * 0.22;
        mesh.rect(
            Rect::new(
                box_rect.x + inset,
                box_rect.y + inset,
                box_side - 2.0 * inset,
                box_side - 2.0 * inset,
            ),
            theme.accent,
        );
    }
    if let Some(text) = label {
        let tx = box_rect.x + box_side + m.pad;
        let ty = body.y + (body.h - font::height(size)) * 0.5;
        let avail = (body.x + body.w - tx).max(0.0);
        font::text_ellipsis(mesh, text, tx, ty, avail, size, theme.text);
    }
}

/// The editable text field. `caret` is `Some` only while the field is focused
/// (it then draws the selection and the blinking-less caret). The **layout does
/// not depend on focus**: a multiline field always lays its lines top-aligned
/// like a text editor (not a centered label), and a single-line field always
/// sits on one vertically-centered row — an unfocused field uses a caret at the
/// start (scroll offset 0), so the pre-written text reads exactly as it will
/// once clicked into. A single-line field clips overflow with an ellipsis when
/// unfocused, and scrolls to the caret when focused. (A `menu`'s read-out reuses
/// this as an unfocused single-line field.)
/// The height of one row of an open `menu`'s list.
pub fn menu_row_h(text_size: f32, m: &Metrics) -> f32 {
    (font::height(text_size) + 2.0 * m.pad).max(m.control_h)
}

/// The rectangle an open `menu`'s list occupies: the width of the menu's cell,
/// one [`menu_row_h`] per option, hanging **below** the cell — or above it when
/// there is no room below, so a menu at the bottom of a window still opens.
///
/// One function for the drawing and for the hit-test, so a click lands on the
/// row it highlighted.
pub fn menu_popup(cell: Rect, options: usize, text_size: f32, window_h: f32, m: &Metrics) -> Rect {
    let h = menu_row_h(text_size, m) * options.max(1) as f32;
    let below = cell.y + cell.h;
    let y = if below + h <= window_h || cell.y - h < 0.0 {
        below
    } else {
        cell.y - h
    };
    Rect::new(cell.x, y, cell.w, h)
}

/// The option index at `py` inside an open list (`None` outside it).
pub fn menu_row_at(popup: Rect, options: usize, px: f64, py: f64) -> Option<usize> {
    if options == 0 || !popup.contains(px, py) {
        return None;
    }
    let row = ((py - popup.y as f64) / (popup.h as f64 / options as f64)) as usize;
    Some(row.min(options - 1))
}

/// Draws an open `menu`'s list: every option, the chosen one marked and the one
/// under the cursor highlighted. It is drawn into the **overlay**, over the
/// whole window, because a list that opens has to cover whatever it opens over.
pub fn draw_menu_popup(
    d: &mut Draw,
    popup: Rect,
    options: &[String],
    index: usize,
    hover: Option<usize>,
    size: f32,
) {
    let (mesh, m, theme) = d.parts();
    mesh.rect(popup, theme.field);
    border(mesh, popup, m.divider_w, theme.accent);
    let row_h = popup.h / options.len().max(1) as f32;
    for (i, option) in options.iter().enumerate() {
        let row = Rect::new(popup.x, popup.y + i as f32 * row_h, popup.w, row_h);
        if hover == Some(i) {
            mesh.rect(row, theme.hilite);
        } else if i == index {
            mesh.rect(row, theme.accent_dim);
        }
        font::text_ellipsis(
            mesh,
            option,
            row.x + m.pad,
            row.y + (row.h - font::height(size)) * 0.5,
            (row.w - 2.0 * m.pad).max(0.0),
            size,
            theme.text,
        );
    }
}

/// Draws a `menu`: the chosen option in a field, with a **marker** in a gutter
/// at its right edge.
///
/// The marker points **down**: a press opens the option list over the window,
/// and a press on a row picks it. A menu drawn as a bare field reads as a
/// label, and then the click that changes the value comes as a surprise. The
/// gutter is reserved out of the text's width, so a long option ellipsizes
/// before it reaches the marker.
pub fn menu(d: &mut Draw, current: &str, label: Option<&str>, rect: Rect, size: f32) {
    let gutter = font::height(size) + d.m.pad;
    let text_cell = Rect::new(rect.x, rect.y, (rect.w - gutter).max(0.0), rect.h);
    field(d, current, label, text_cell, size, false, None);
    // The marker rides the body's own row, not the cell's: a labelled menu has
    // a label strip over it, and the two must not overlap.
    let body = body_rect_at(rect, label.is_some(), size, d.m);
    let (mesh, m, theme) = d.parts();
    mesh.rect(
        Rect::new(text_cell.x + text_cell.w - m.pad, body.y, gutter, body.h),
        theme.field,
    );
    let side = (font::height(size) * 0.5).min(body.h * 0.4).max(2.0);
    let cx = rect.x + rect.w - m.pad - side;
    let cy = body.y + body.h * 0.5;
    mesh.tri(
        [cx - side, cy - side * 0.5],
        [cx + side, cy - side * 0.5],
        [cx, cy + side * 0.5],
        theme.accent,
    );
}

/// Draws an editable text field: its label strip, its body, the visible text —
/// scrolled to the caret when `caret` is `Some` (the field is focused) — and,
/// then, the selection and the caret themselves.
pub fn field(
    d: &mut Draw,
    value: &str,
    label: Option<&str>,
    rect: Rect,
    size: f32,
    multiline: bool,
    caret: Option<textedit::Caret>,
) {
    label_strip(d, label, rect, size);
    let m = d.m;
    let body = body_rect_at(rect, label.is_some(), size, m);
    d.mesh.rect(body, d.theme.field);

    let text_x = body.x + m.pad;
    let text_w = (body.w - 2.0 * m.pad).max(0.0);
    // The scroll window is still counted in characters -- a caret moves by
    // characters, so what has to stay visible is a character -- while every
    // *position* inside the line is measured, which is what a proportional
    // face needs (see `visible_line`).
    let cols = font::fit_chars(text_w, size);
    // Unfocused: lay out around a caret at the start (no scroll, no caret drawn).
    let lay = caret.unwrap_or_default();

    if !multiline {
        // One row, vertically centered. Unfocused text that overflows clips with
        // an ellipsis (the label/menu look); focused, it scrolls to the caret.
        let ty = (body.y + (body.h - font::height(size)) * 0.5).max(body.y);
        let first = value.split('\n').next().unwrap_or("");
        if caret.is_none() {
            font::text_ellipsis(d.mesh, first, text_x, ty, text_w, size, d.theme.text);
            return;
        }
        let hstart = textedit::h_scroll(textedit::line_col(value, lay.pos).1, cols);
        draw_line(d, value, 0, text_x, ty, hstart, cols, text_w, size, caret);
        return;
    }

    // Multiline: a top-aligned block of rows, scrolled (when focused) to the
    // caret's line/column so it stays visible; from the top-left otherwise.
    let hstart = textedit::h_scroll(textedit::line_col(value, lay.pos).1, cols);
    let row_h = font::line_advance(size);
    let rows = (((body.h - 2.0 * m.pad) / row_h) as usize).max(1);
    let row_start = textedit::h_scroll(textedit::line_col(value, lay.pos).0, rows);
    let mut byte = 0usize; // running byte offset of each line's start
    for (i, line) in value.split('\n').enumerate() {
        if i >= row_start && i < row_start + rows {
            let ty = body.y + m.pad + (i - row_start) as f32 * row_h;
            draw_line(
                d, value, byte, text_x, ty, hstart, cols, text_w, size, caret,
            );
        }
        byte += line.len() + 1; // + the '\n'
    }
}

/// The part of `line` a field shows: from column `hstart`, at most `cols`
/// characters and never wider than `text_w` pixels.
///
/// The two limits are one under the fixed-pitch bitmap (`cols` *is* `text_w`
/// divided by the cell) and two under a proportional face, where a window of N
/// characters may be wider or narrower than the box. The width is the one that
/// binds: a field's text is not clipped by a scissor, so a line that overflowed
/// would bleed over the chrome beside it.
fn visible_line(line: &str, hstart: usize, cols: usize, text_w: f32, size: f32) -> String {
    let mut out = String::new();
    let mut w = 0.0;
    for c in line.chars().skip(hstart).take(cols) {
        let step = font::advance_of(c, size);
        if w + step > text_w && !out.is_empty() {
            break;
        }
        w += step;
        out.push(c);
    }
    out
}

/// Draws one line of a field: its visible glyphs (scrolled by `hstart` columns,
/// clipped to the body), and — only when `caret` is `Some` (focused) — the
/// selection highlight over its selected span and the caret when it falls on
/// this line. `line_byte` is the byte offset of the line's start in `value`.
#[allow(clippy::too_many_arguments)] // the line and its window, past the context
fn draw_line(
    d: &mut Draw,
    value: &str,
    line_byte: usize,
    x: f32,
    y: f32,
    hstart: usize,
    cols: usize,
    text_w: f32,
    size: f32,
    caret: Option<textedit::Caret>,
) {
    let (mesh, _m, theme) = d.parts();
    let end_byte = value[line_byte..]
        .find('\n')
        .map_or(value.len(), |i| line_byte + i);
    let line = &value[line_byte..end_byte];
    let visible = visible_line(line, hstart, cols, text_w, size);
    let shown = visible.chars().count();

    // Selection highlight (drawn under the text): the part of this line inside
    // the selection, mapped to visible columns and measured from the glyphs.
    if let Some((s, e)) = caret.and_then(|c| c.selection()) {
        let a = s.clamp(line_byte, end_byte);
        let b = e.clamp(line_byte, end_byte);
        if b > a || (s <= line_byte && e > end_byte) {
            let ca = value[line_byte..a].chars().count();
            let cb = value[line_byte..b].chars().count();
            let va = ca.saturating_sub(hstart).min(shown);
            let vb = cb.saturating_sub(hstart).min(shown);
            if vb > va {
                let x0 = font::prefix_width(&visible, va, size);
                let x1 = font::prefix_width(&visible, vb, size);
                mesh.rect(
                    Rect::new(x + x0, y, x1 - x0, font::height(size)),
                    theme.selection,
                );
            }
        }
    }

    // The visible glyphs.
    font::text(mesh, &visible, x, y, size, theme.text);

    // The caret, when focused and sitting on this line within the visible window.
    if let Some(caret) = caret {
        let cl = textedit::line_col(value, caret.pos).0;
        let this_line = value[..line_byte].bytes().filter(|&b| b == b'\n').count();
        if cl == this_line {
            let col = value[line_byte..caret.pos.clamp(line_byte, end_byte)]
                .chars()
                .count();
            if col >= hstart && col <= hstart + shown {
                let cx = x + font::prefix_width(&visible, col - hstart, size);
                mesh.rect(
                    Rect::new(cx, y, size.max(1.0), font::height(size)),
                    theme.accent,
                );
            }
        }
    }
}

/// The caret byte offset a click at `(x, y)` lands on in a text `field`,
/// reconstructing the same layout [`field`] draws (label strip, body inset,
/// horizontal/vertical scroll from `current`) so a click lands on the glyph it
/// points at. `has_label` and `size` are the widget's; `current` is the caret
/// before the click (its scroll offset is what the field is showing).
#[allow(clippy::too_many_arguments)] // a hit-test needs the field's full layout
pub fn caret_at(
    rect: Rect,
    value: &str,
    has_label: bool,
    size: f32,
    multiline: bool,
    current: textedit::Caret,
    x: f64,
    y: f64,
    m: &Metrics,
) -> usize {
    let body = body_rect_at(rect, has_label, size, m);
    let text_x = body.x + m.pad;
    let text_w = (body.w - 2.0 * m.pad).max(0.0);
    let cols = font::fit_chars(text_w, size);
    let caret_col = textedit::line_col(value, current.pos).1;
    // The column a click lands on is **measured against the glyphs actually
    // shown**, the same string `draw_line` drew — which is what makes a click
    // land on the letter it points at with a proportional face, and is exactly
    // the old cell division under the fixed-pitch one.
    let col_at = |line: &str, hstart: usize| {
        let shown = visible_line(line, hstart, cols, text_w, size);
        font::column_at(&shown, x as f32 - text_x, size)
    };
    let line_of = |i: usize| value.split('\n').nth(i).unwrap_or("");

    if !multiline {
        let hstart = textedit::h_scroll(caret_col, cols);
        return textedit::offset_of(value, 0, hstart + col_at(line_of(0), hstart));
    }

    let caret_line = textedit::line_col(value, current.pos).0;
    let row_h = font::line_advance(size);
    let rows = (((body.h - 2.0 * m.pad) / row_h) as usize).max(1);
    let row_start = textedit::h_scroll(caret_line, rows);
    let hstart = textedit::h_scroll(caret_col, cols);
    let n_lines = value.split('\n').count();
    let rel = ((y as f32 - (body.y + m.pad)) / row_h).max(0.0) as usize;
    let line = (row_start + rel).min(n_lines.saturating_sub(1));
    textedit::offset_of(value, line, hstart + col_at(line_of(line), hstart))
}

/// A value read-out at the bottom-right of a body (clipped with an ellipsis
/// when the body is narrower than the number).
fn value_text(d: &mut Draw, s: &str, body: Rect, size: f32) {
    let (mesh, m, theme) = d.parts();
    let avail = (body.w - m.pad).max(0.0);
    let w = font::width(s, size).min(avail);
    let x = (body.x + body.w - w - m.pad).max(body.x);
    let y = (body.y + body.h - font::height(size)).max(body.y);
    font::text_ellipsis(mesh, s, x, y, avail, size, theme.text);
}

/// The cell width a control's value **read-out** needs: the widest number this
/// range can ever show, plus the insets [`value_text`] draws it inside (the
/// body's on both sides and its own on the right).
///
/// It measures the **bounds**, never the current value — `min` and `max` are
/// props and the value is not, so a control fitted to its content keeps one
/// width while it is turned instead of resizing under the hand turning it.
pub(crate) fn readout_w(r: &Range, size: f32, m: &Metrics) -> f32 {
    let widest = font::width(&fmt(r.min), size).max(font::width(&fmt(r.max), size));
    widest + 3.0 * m.pad
}

/// Formats a control value compactly (drops trailing zeros within 2 decimals).
fn fmt(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e6 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

// --- Natural sizes ----------------------------------------------------------
//
// How tall a control wants to be is the same fact as how it is drawn: the
// read-out strip a slider reserves under its groove is one number, read here by
// the drawing and by the size. They lived apart, which is how the size pass came
// to import this module; they are one section now, and an element's `natural`
// calls straight into it.

/// A labelled field's height: its label strip, its body inset and one control
/// line (the read-out row).
pub fn field_h(r: &Range, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    size::label_strip(r.label.is_some(), size, m) + size::body_inset(m) + size::control_box(size, m)
}

/// A horizontal slider's thickness: the label strip, the body inset, the
/// handle's grip across the track and the read-out strip under it — the same
/// reservation the drawing makes ([`slider_track`]), so the groove
/// gets the grip it asked for and the number gets its own row.
pub fn slider_thick(r: &Range, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    size::label_strip(r.label.is_some(), size, m)
        + size::body_inset(m)
        + m.handle_grip.max(m.handle_thick)
        + readout_h(size, m)
}

/// A vertical slider's width: the grip across the track, inset in the body.
/// The value read-out shares that width and ellipsizes — a number's own length
/// is data, and no size here may follow it.
pub fn slider_across(m: &Metrics) -> f32 {
    size::body_inset(m) + m.handle_grip.max(m.box_side)
}

/// A knob's height: the label strip, the body inset, the disc and the read-out
/// strip the drawing reserves under it.
pub fn knob_h(r: &Range, m: &Metrics, scale: f32) -> f32 {
    let size = r.text_size * scale;
    size::label_strip(r.label.is_some(), size, m)
        + size::body_inset(m)
        + m.knob_d
        + readout_h(size, m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::theme::Theme;

    #[test]
    fn slider_fraction_spans_the_body() {
        let body = Rect::new(10.0, 0.0, 100.0, 20.0);
        assert_eq!(slider_fraction(body, 10.0), 0.0);
        assert_eq!(slider_fraction(body, 60.0), 0.5);
        assert_eq!(slider_fraction(body, 110.0), 1.0);
        assert_eq!(slider_fraction(body, 200.0), 1.0, "clamped past the end");
    }

    #[test]
    fn vertical_slider_fraction_runs_bottom_to_top() {
        let body = Rect::new(0.0, 10.0, 20.0, 100.0);
        assert_eq!(slider_fraction_v(body, 110.0), 0.0, "bottom edge is min");
        assert_eq!(slider_fraction_v(body, 60.0), 0.5, "centre is half");
        assert_eq!(slider_fraction_v(body, 10.0), 1.0, "top edge is max");
        assert_eq!(slider_fraction_v(body, -50.0), 1.0, "clamped above the top");
    }

    #[test]
    fn drag_up_increases_value() {
        // Dragging up (dy negative) yields a positive fraction delta.
        assert!(drag_fraction_delta(-50.0, 100.0) > 0.0);
        assert!(drag_fraction_delta(50.0, 100.0) < 0.0);
    }

    #[test]
    fn body_sits_below_the_label_strip() {
        let rect = Rect::new(0.0, 0.0, 100.0, 80.0);
        let m = Metrics::default();
        let with = body_rect(rect, true, &m);
        let without = body_rect(rect, false, &m);
        assert!(with.y > without.y, "a label pushes the body down");
    }

    #[test]
    fn text_size_scales_the_label_strip() {
        let rect = Rect::new(0.0, 0.0, 100.0, 80.0);
        let m = Metrics::default();
        let small = body_rect_at(rect, true, 1.0, &m);
        let big = body_rect_at(rect, true, 4.0, &m);
        assert!(big.y > small.y, "a bigger label strip pushes the body down");
        assert_eq!(
            body_rect(rect, true, &m),
            body_rect_at(rect, true, m.text_scale, &m)
        );
    }

    #[test]
    fn label_alignment_places_the_line() {
        let rect = Rect::new(0.0, 0.0, 200.0, 40.0);
        let mut xs = Vec::new();
        for align in [Align::Start, Align::Center, Align::End] {
            let mut mesh = Mesh::new();
            draw_label(
                &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
                "HI",
                rect,
                2.0,
                false,
                align,
            );
            xs.push(mesh.positions().map(|(x, _)| x).fold(f32::MAX, f32::min));
        }
        assert!(xs[0] < xs[1] && xs[1] < xs[2], "start < center < end");
    }

    #[test]
    fn wrapped_label_stays_inside_its_rect() {
        let rect = Rect::new(10.0, 10.0, 90.0, 60.0);
        let mut m = Mesh::new();
        draw_label(
            &mut Draw::new(&mut m, &Metrics::default(), &Theme::default()),
            "a rather long label that must wrap over several lines",
            rect,
            2.0,
            true,
            Align::Start,
        );
        assert!(!m.is_empty());
        let max_x = m.positions().map(|(x, _)| x).fold(f32::MIN, f32::max);
        let max_y = m.positions().map(|(_, y)| y).fold(f32::MIN, f32::max);
        assert!(max_x <= rect.x + rect.w, "wrap bleeds right");
        assert!(max_y <= rect.y + rect.h, "overflowing lines must drop");
    }

    #[test]
    fn unwrapped_label_clips_with_an_ellipsis() {
        let rect = Rect::new(0.0, 0.0, 60.0, 30.0);
        let mut m = Mesh::new();
        draw_label(
            &mut Draw::new(&mut m, &Metrics::default(), &Theme::default()),
            "far too long to fit here",
            rect,
            2.0,
            false,
            Align::Start,
        );
        let max_x = m.positions().map(|(x, _)| x).fold(f32::MIN, f32::max);
        assert!(max_x <= rect.x + rect.w, "single line bleeds past the rect");
    }

    #[test]
    fn knob_geometry_stays_within_its_cell() {
        // A wide, short labelled cell like gui_panel's knob row: the disc is
        // bounded by the short side, leaving little vertical room. The value
        // read-out used to be placed past the body and spilled below the cell
        // into the next row; it must now stay within the cell.
        let cell = Rect::new(12.0, 34.0, 175.0, 103.0);
        let r = Range {
            value: 800.0,
            min: 20.0,
            max: 20000.0,
            label: Some("cutoff".into()),
            text_size: Metrics::default().text_scale,
        };
        let mut mesh = Mesh::new();
        knob(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            &r,
            cell,
            r.text_size,
        );
        let bottom = cell.y + cell.h;
        let max_y = mesh.positions().map(|(_, y)| y).fold(f32::MIN, f32::max);
        assert!(
            max_y <= bottom,
            "knob geometry spills {:.1}px below the cell (max_y {max_y} > bottom {bottom})",
            max_y - bottom
        );
    }
}
