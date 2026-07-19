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
use super::paint::{Color, Mesh};
use super::theme::Theme;
use super::widget::{Align, Range, WidgetKind};

const PAD: f32 = 4.0;
const TEXT_SCALE: f32 = font::DEFAULT_SIZE;

/// The label strip height when a control carries a label, else 0.
fn label_height(has_label: bool, text_size: f32) -> f32 {
    if has_label {
        font::height(text_size) + PAD
    } else {
        0.0
    }
}

/// The control body at the default text size (the views that keep the fixed
/// label scale). Shared by drawing and hit-math so they agree on the track.
pub fn body_rect(rect: Rect, has_label: bool) -> Rect {
    body_rect_at(rect, has_label, TEXT_SCALE)
}

/// The control body (the rect minus its label strip — sized by the widget's
/// `text_size` — and a small inset).
pub fn body_rect_at(rect: Rect, has_label: bool, text_size: f32) -> Rect {
    let top = rect.y + label_height(has_label, text_size);
    Rect::new(
        rect.x + PAD,
        top + PAD,
        (rect.w - 2.0 * PAD).max(0.0),
        (rect.y + rect.h - top - 2.0 * PAD).max(0.0),
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

/// Draws a control into `mesh`. `active` highlights a pressed/dragged control.
pub fn draw(mesh: &mut Mesh, kind: &WidgetKind, rect: Rect, active: bool, theme: &Theme) {
    match kind {
        WidgetKind::Slider { range, vertical } => slider(mesh, range, rect, *vertical, theme),
        WidgetKind::Knob(r) => knob(mesh, r, rect, theme),
        WidgetKind::Number(r) => number(mesh, r, rect, theme),
        WidgetKind::Button { label, text_size } => {
            button(mesh, label.as_deref(), rect, active, *text_size, theme)
        }
        WidgetKind::Toggle {
            value,
            label,
            text_size,
        } => toggle(mesh, *value, label.as_deref(), rect, *text_size, theme),
        WidgetKind::Text {
            value,
            label,
            text_size,
        } => field(mesh, value, label.as_deref(), rect, *text_size, theme),
        WidgetKind::Menu {
            index,
            options,
            label,
            text_size,
        } => {
            let current = options.get(*index).map(String::as_str).unwrap_or("");
            field(mesh, current, label.as_deref(), rect, *text_size, theme);
        }
        _ => {}
    }
}

/// Draws the label strip above a control body, if it has a label (clipped to
/// the cell with an ellipsis).
fn label_strip(mesh: &mut Mesh, label: Option<&str>, rect: Rect, size: f32, theme: &Theme) {
    if let Some(text) = label {
        font::text_ellipsis(
            mesh,
            text,
            rect.x + PAD,
            rect.y + PAD,
            (rect.w - 2.0 * PAD).max(0.0),
            size,
            theme.text,
        );
    }
}

/// Slider track thickness, handle thickness along the travel axis, and handle
/// length across it. The handle is a short grip, **not** the full body span.
const TRACK_THICK: f32 = 4.0;
const HANDLE_THICK: f32 = 8.0;
const HANDLE_GRIP: f32 = 18.0;

fn slider(mesh: &mut Mesh, r: &Range, rect: Rect, vertical: bool, theme: &Theme) {
    label_strip(mesh, r.label.as_deref(), rect, r.text_size, theme);
    let body = body_rect_at(rect, r.label.is_some(), r.text_size);
    let f = r.fraction();
    if vertical {
        // Track down the centre; min at the bottom, max at the top.
        let cx = body.x + body.w * 0.5;
        mesh.rect(
            Rect::new(cx - TRACK_THICK * 0.5, body.y, TRACK_THICK, body.h),
            theme.track,
        );
        let hy = body.y + body.h * (1.0 - f);
        mesh.rect(
            Rect::new(
                cx - TRACK_THICK * 0.5,
                hy,
                TRACK_THICK,
                (body.y + body.h - hy).max(0.0),
            ),
            theme.accent_dim,
        );
        let grip = HANDLE_GRIP.min(body.w);
        mesh.rect(
            Rect::new(cx - grip * 0.5, hy - HANDLE_THICK * 0.5, grip, HANDLE_THICK),
            theme.accent,
        );
    } else {
        let mid = body.y + body.h * 0.5;
        mesh.rect(
            Rect::new(body.x, mid - TRACK_THICK * 0.5, body.w, TRACK_THICK),
            theme.track,
        );
        let hx = body.x + body.w * f;
        mesh.rect(
            Rect::new(
                body.x,
                mid - TRACK_THICK * 0.5,
                (hx - body.x).max(0.0),
                TRACK_THICK,
            ),
            theme.accent_dim,
        );
        let grip = HANDLE_GRIP.min(body.h);
        mesh.rect(
            Rect::new(
                hx - HANDLE_THICK * 0.5,
                mid - grip * 0.5,
                HANDLE_THICK,
                grip,
            ),
            theme.accent,
        );
    }
    value_text(mesh, &fmt(r.value), body, r.text_size, theme);
}

fn knob(mesh: &mut Mesh, r: &Range, rect: Rect, theme: &Theme) {
    label_strip(mesh, r.label.as_deref(), rect, r.text_size, theme);
    let body = body_rect_at(rect, r.label.is_some(), r.text_size);
    // Reserve a strip at the bottom of the body for the value read-out and size
    // the disc in the area above it, so the number stays inside the body — it
    // never overlaps the disc nor spills past the cell into the row below.
    let text_h = font::height(r.text_size) + PAD;
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
        r.text_size,
        theme,
    );
}

fn number(mesh: &mut Mesh, r: &Range, rect: Rect, theme: &Theme) {
    label_strip(mesh, r.label.as_deref(), rect, r.text_size, theme);
    let body = body_rect_at(rect, r.label.is_some(), r.text_size);
    mesh.rect(body, theme.field);
    // A vertical fill rising from the bottom shows the value in range, so
    // dragging up raises the green level; a border frames the field.
    let fill_h = body.h * r.fraction();
    mesh.rect(
        Rect::new(body.x, body.y + body.h - fill_h, body.w, fill_h),
        theme.accent_dim,
    );
    border(mesh, body, 1.0, theme.accent);
    font::text_centered(mesh, &fmt(r.value), body, r.text_size, theme.text);
}

/// Draws a `label` into its rect: the line block vertically centered, each
/// line placed by `align`. With `wrap` the text word-wraps on the font's
/// fixed advance and lines past the rect's bottom are dropped; without it the
/// single line clips with an ellipsis instead of bleeding into a neighbor.
pub fn draw_label(
    mesh: &mut Mesh,
    text: &str,
    rect: Rect,
    size: f32,
    wrap: bool,
    align: Align,
    theme: &Theme,
) {
    let left = rect.x + PAD;
    let avail = (rect.w - 2.0 * PAD).max(0.0);
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
    let body = body_rect_at(rect, false, size);
    mesh.rect(
        body,
        if active {
            theme.hilite
        } else {
            theme.accent_dim
        },
    );
    font::text_centered(mesh, label.unwrap_or("BUTTON"), body, size, theme.text);
}

fn toggle(mesh: &mut Mesh, on: bool, label: Option<&str>, rect: Rect, size: f32, theme: &Theme) {
    let body = body_rect_at(rect, false, size);
    let box_side = body.h.min(body.w).min(24.0);
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
        let tx = box_rect.x + box_side + PAD;
        let ty = body.y + (body.h - font::height(size)) * 0.5;
        let avail = (body.x + body.w - tx).max(0.0);
        font::text_ellipsis(mesh, text, tx, ty, avail, size, theme.text);
    }
}

fn field(mesh: &mut Mesh, value: &str, label: Option<&str>, rect: Rect, size: f32, theme: &Theme) {
    label_strip(mesh, label, rect, size, theme);
    let body = body_rect_at(rect, label.is_some(), size);
    mesh.rect(body, theme.field);
    let ty = body.y + (body.h - font::height(size)) * 0.5;
    font::text_ellipsis(
        mesh,
        value,
        body.x + PAD,
        ty.max(body.y),
        (body.w - 2.0 * PAD).max(0.0),
        size,
        theme.text,
    );
}

/// A value read-out at the bottom-right of a body (clipped with an ellipsis
/// when the body is narrower than the number).
fn value_text(mesh: &mut Mesh, s: &str, body: Rect, size: f32, theme: &Theme) {
    let avail = (body.w - PAD).max(0.0);
    let w = font::width(s, size).min(avail);
    let x = (body.x + body.w - w - PAD).max(body.x);
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
        let with = body_rect(rect, true);
        let without = body_rect(rect, false);
        assert!(with.y > without.y, "a label pushes the body down");
    }

    #[test]
    fn text_size_scales_the_label_strip() {
        let rect = Rect::new(0.0, 0.0, 100.0, 80.0);
        let small = body_rect_at(rect, true, 1.0);
        let big = body_rect_at(rect, true, 4.0);
        assert!(big.y > small.y, "a bigger label strip pushes the body down");
        assert_eq!(body_rect(rect, true), body_rect_at(rect, true, TEXT_SCALE));
    }

    #[test]
    fn label_alignment_places_the_line() {
        let rect = Rect::new(0.0, 0.0, 200.0, 40.0);
        let mut xs = Vec::new();
        for align in [Align::Start, Align::Center, Align::End] {
            let mut m = Mesh::new();
            draw_label(&mut m, "HI", rect, 2.0, false, align, &Theme::default());
            xs.push(m.positions().map(|(x, _)| x).fold(f32::MAX, f32::min));
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
            text_size: TEXT_SCALE,
        };
        let mut mesh = Mesh::new();
        knob(&mut mesh, &r, cell, &Theme::default());
        let bottom = cell.y + cell.h;
        let max_y = mesh.positions().map(|(_, y)| y).fold(f32::MIN, f32::max);
        assert!(
            max_y <= bottom,
            "knob geometry spills {:.1}px below the cell (max_y {max_y} > bottom {bottom})",
            max_y - bottom
        );
    }
}
