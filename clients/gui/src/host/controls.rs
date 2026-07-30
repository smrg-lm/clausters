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

use super::font;
use super::layout::Rect;
use super::metrics::Metrics;
use super::paint::{Color, Mesh};
use super::textedit;
use super::theme::Theme;
use super::widget::{Align, Range, WidgetKind};

/// The label strip height when a control carries a label, else 0.
fn label_height(has_label: bool, text_size: f32, m: &Metrics) -> f32 {
    if has_label {
        font::height(text_size) + m.pad
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
    let top = rect.y + label_height(has_label, text_size, m);
    Rect::new(
        rect.x + m.pad,
        top + m.pad,
        (rect.w - 2.0 * m.pad).max(0.0),
        (rect.y + rect.h - top - 2.0 * m.pad).max(0.0),
    )
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

/// Draws a control into `mesh`. `active` highlights a pressed/dragged control;
/// `scale` is the placement's accumulated workspace zoom ([`Placed::scale`]),
/// which the text sizes pick up so a zoomed box keeps its proportions.
///
/// [`Placed::scale`]: super::layout::Placed::scale
#[allow(clippy::too_many_arguments)] // one control's draw: its box, state, look
pub fn draw(
    mesh: &mut Mesh,
    kind: &WidgetKind,
    rect: Rect,
    active: bool,
    focused: bool,
    scale: f32,
    m: &Metrics,
    theme: &Theme,
) {
    match kind {
        WidgetKind::Slider { range, vertical } => slider(
            mesh,
            range,
            rect,
            *vertical,
            range.text_size * scale,
            m,
            theme,
        ),
        WidgetKind::Knob(r) => knob(mesh, r, rect, r.text_size * scale, m, theme),
        WidgetKind::Number(r) => number(mesh, r, rect, r.text_size * scale, m, theme),
        WidgetKind::Button { label, text_size } => button(
            mesh,
            label.as_deref(),
            rect,
            active,
            *text_size * scale,
            theme,
        ),
        WidgetKind::Toggle {
            value,
            label,
            text_size,
        } => toggle(
            mesh,
            *value,
            label.as_deref(),
            rect,
            *text_size * scale,
            m,
            theme,
        ),
        WidgetKind::Text {
            value,
            label,
            text_size,
            multiline,
            caret,
        } => field(
            mesh,
            value,
            label.as_deref(),
            rect,
            *text_size * scale,
            *multiline,
            focused.then_some(*caret),
            m,
            theme,
        ),
        WidgetKind::Menu {
            index,
            options,
            label,
            text_size,
        } => {
            let current = options.get(*index).map(String::as_str).unwrap_or("");
            field(
                mesh,
                current,
                label.as_deref(),
                rect,
                *text_size * scale,
                false,
                None, // a menu's read-out is never an editable focus target
                m,
                theme,
            );
        }
        _ => {}
    }
}

/// Draws the label strip above a control body, if it has a label (clipped to
/// the cell with an ellipsis).
fn label_strip(
    mesh: &mut Mesh,
    label: Option<&str>,
    rect: Rect,
    size: f32,
    m: &Metrics,
    theme: &Theme,
) {
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

fn slider(
    mesh: &mut Mesh,
    r: &Range,
    rect: Rect,
    vertical: bool,
    size: f32,
    m: &Metrics,
    theme: &Theme,
) {
    label_strip(mesh, r.label.as_deref(), rect, size, m, theme);
    let body = body_rect_at(rect, r.label.is_some(), size, m);
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
    value_text(mesh, &fmt(r.value), body, size, m, theme);
}

fn knob(mesh: &mut Mesh, r: &Range, rect: Rect, size: f32, m: &Metrics, theme: &Theme) {
    label_strip(mesh, r.label.as_deref(), rect, size, m, theme);
    let body = body_rect_at(rect, r.label.is_some(), size, m);
    // Reserve a strip at the bottom of the body for the value read-out and size
    // the disc in the area above it, so the number stays inside the body — it
    // never overlaps the disc nor spills past the cell into the row below.
    let text_h = font::height(size) + m.pad;
    let disc_h = (body.h - text_h).max(0.0);
    let radius = (body.w.min(disc_h) * 0.5 - 2.0).max(2.0);
    let cx = body.x + body.w * 0.5;
    let cy = body.y + disc_h * 0.5;
    mesh.disc(cx, cy, radius, theme.track);
    mesh.disc(cx, cy, radius - 3.0, theme.field);
    // Pointer: 270-degree sweep, min at lower-left, max at lower-right.
    let angle = (135.0 + 270.0 * r.fraction()).to_radians();
    let tip = [cx + radius * angle.cos(), cy + radius * angle.sin()];
    mesh.line([cx, cy], tip, 3.0, theme.accent);
    value_text(
        mesh,
        &fmt(r.value),
        Rect::new(body.x, body.y + body.h - text_h, body.w, text_h),
        size,
        m,
        theme,
    );
}

fn number(mesh: &mut Mesh, r: &Range, rect: Rect, size: f32, m: &Metrics, theme: &Theme) {
    label_strip(mesh, r.label.as_deref(), rect, size, m, theme);
    let body = body_rect_at(rect, r.label.is_some(), size, m);
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
#[allow(clippy::too_many_arguments)] // the text, its box, its look
pub fn draw_label(
    mesh: &mut Mesh,
    text: &str,
    rect: Rect,
    size: f32,
    wrap: bool,
    align: Align,
    m: &Metrics,
    theme: &Theme,
) {
    let left = rect.x + m.pad;
    let avail = (rect.w - 2.0 * m.pad).max(0.0);
    if wrap {
        let cols = font::fit_chars(avail, size).max(1);
        let lines = font::wrap(text, cols);
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

fn button(
    mesh: &mut Mesh,
    label: Option<&str>,
    rect: Rect,
    active: bool,
    size: f32,
    theme: &Theme,
) {
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

fn toggle(
    mesh: &mut Mesh,
    on: bool,
    label: Option<&str>,
    rect: Rect,
    size: f32,
    m: &Metrics,
    theme: &Theme,
) {
    // Like `button`, the toggle owns its whole cell (its box and label fill it);
    // the layout gap does the separating.
    let body = rect;
    let box_side = body.h.min(body.w).min(m.box_side);
    let box_rect = Rect::new(
        body.x,
        body.y + (body.h - box_side) * 0.5,
        box_side,
        box_side,
    );
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
#[allow(clippy::too_many_arguments)] // a widget's draw: its content plus its box
fn field(
    mesh: &mut Mesh,
    value: &str,
    label: Option<&str>,
    rect: Rect,
    size: f32,
    multiline: bool,
    caret: Option<textedit::Caret>,
    m: &Metrics,
    theme: &Theme,
) {
    label_strip(mesh, label, rect, size, m, theme);
    let body = body_rect_at(rect, label.is_some(), size, m);
    mesh.rect(body, theme.field);

    let text_x = body.x + m.pad;
    let text_w = (body.w - 2.0 * m.pad).max(0.0);
    let cols = font::fit_chars(text_w, size);
    let cell = font::width(" ", size); // one glyph cell advance in device px
    // Unfocused: lay out around a caret at the start (no scroll, no caret drawn).
    let lay = caret.unwrap_or_default();

    if !multiline {
        // One row, vertically centered. Unfocused text that overflows clips with
        // an ellipsis (the label/menu look); focused, it scrolls to the caret.
        let ty = (body.y + (body.h - font::height(size)) * 0.5).max(body.y);
        let first = value.split('\n').next().unwrap_or("");
        if caret.is_none() {
            font::text_ellipsis(mesh, first, text_x, ty, text_w, size, theme.text);
            return;
        }
        let hstart = textedit::h_scroll(textedit::line_col(value, lay.pos).1, cols);
        draw_line(
            mesh, value, 0, text_x, ty, hstart, cols, cell, size, caret, theme,
        );
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
                mesh, value, byte, text_x, ty, hstart, cols, cell, size, caret, theme,
            );
        }
        byte += line.len() + 1; // + the '\n'
    }
}

/// Draws one line of a field: its visible glyphs (scrolled by `hstart` columns,
/// clipped to `cols`), and — only when `caret` is `Some` (focused) — the
/// selection highlight over its selected span and the caret when it falls on
/// this line. `line_byte` is the byte offset of the line's start in `value`.
#[allow(clippy::too_many_arguments)] // the line, its window, the caret, the look
fn draw_line(
    mesh: &mut Mesh,
    value: &str,
    line_byte: usize,
    x: f32,
    y: f32,
    hstart: usize,
    cols: usize,
    cell: f32,
    size: f32,
    caret: Option<textedit::Caret>,
    theme: &Theme,
) {
    let end_byte = value[line_byte..]
        .find('\n')
        .map_or(value.len(), |i| line_byte + i);
    let line = &value[line_byte..end_byte];

    // Selection highlight (drawn under the text): the part of this line inside
    // the selection, mapped to visible columns.
    if let Some((s, e)) = caret.and_then(|c| c.selection()) {
        let a = s.clamp(line_byte, end_byte);
        let b = e.clamp(line_byte, end_byte);
        if b > a || (s <= line_byte && e > end_byte) {
            let ca = value[line_byte..a].chars().count();
            let cb = value[line_byte..b].chars().count();
            let va = ca.max(hstart).saturating_sub(hstart);
            let vb = cb.min(hstart + cols).saturating_sub(hstart);
            if vb > va {
                mesh.rect(
                    Rect::new(
                        x + va as f32 * cell,
                        y,
                        (vb - va) as f32 * cell,
                        font::height(size),
                    ),
                    theme.selection,
                );
            }
        }
    }

    // The visible glyphs.
    let visible: String = line.chars().skip(hstart).take(cols).collect();
    font::text(mesh, &visible, x, y, size, theme.text);

    // The caret, when focused and sitting on this line within the visible window.
    if let Some(caret) = caret {
        let cl = textedit::line_col(value, caret.pos).0;
        let this_line = value[..line_byte].bytes().filter(|&b| b == b'\n').count();
        if cl == this_line {
            let col = value[line_byte..caret.pos.clamp(line_byte, end_byte)]
                .chars()
                .count();
            if col >= hstart && col <= hstart + cols {
                let cx = x + (col - hstart) as f32 * cell;
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
    let cell = font::width(" ", size).max(1.0);
    let col_at = |lx: f64| (((lx as f32 - text_x) / cell).round().max(0.0)) as usize;
    let caret_col = textedit::line_col(value, current.pos).1;

    if !multiline {
        let hstart = textedit::h_scroll(caret_col, cols);
        return textedit::offset_of(value, 0, hstart + col_at(x));
    }

    let caret_line = textedit::line_col(value, current.pos).0;
    let row_h = font::line_advance(size);
    let rows = (((body.h - 2.0 * m.pad) / row_h) as usize).max(1);
    let row_start = textedit::h_scroll(caret_line, rows);
    let hstart = textedit::h_scroll(caret_col, cols);
    let n_lines = value.split('\n').count();
    let rel = ((y as f32 - (body.y + m.pad)) / row_h).max(0.0) as usize;
    let line = (row_start + rel).min(n_lines.saturating_sub(1));
    textedit::offset_of(value, line, hstart + col_at(x))
}

/// A value read-out at the bottom-right of a body (clipped with an ellipsis
/// when the body is narrower than the number).
fn value_text(mesh: &mut Mesh, s: &str, body: Rect, size: f32, m: &Metrics, theme: &Theme) {
    let avail = (body.w - m.pad).max(0.0);
    let w = font::width(s, size).min(avail);
    let x = (body.x + body.w - w - m.pad).max(body.x);
    let y = (body.y + body.h - font::height(size)).max(body.y);
    font::text_ellipsis(mesh, s, x, y, avail, size, theme.text);
}

/// Formats a control value compactly (drops trailing zeros within 2 decimals).
fn fmt(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e6 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                &mut mesh,
                "HI",
                rect,
                2.0,
                false,
                align,
                &Metrics::default(),
                &Theme::default(),
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
            &mut m,
            "a rather long label that must wrap over several lines",
            rect,
            2.0,
            true,
            Align::Start,
            &Metrics::default(),
            &Theme::default(),
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
            &mut m,
            "far too long to fit here",
            rect,
            2.0,
            false,
            Align::Start,
            &Metrics::default(),
            &Theme::default(),
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
            &mut mesh,
            &r,
            cell,
            r.text_size,
            &Metrics::default(),
            &Theme::default(),
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
