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
use super::widget::{Range, WidgetKind};

const TEXT: Color = [0.85, 0.87, 0.90, 1.0];
const TRACK: Color = [0.10, 0.11, 0.14, 1.0];
const ACCENT: Color = [0.30, 0.78, 0.55, 1.0];
const ACCENT_DIM: Color = [0.22, 0.50, 0.40, 1.0];
const FIELD: Color = [0.14, 0.15, 0.19, 1.0];
const HILITE: Color = [0.40, 0.85, 0.62, 1.0];

const PAD: f32 = 4.0;
const TEXT_SCALE: f32 = 2.0;

/// The label strip height when a control carries a label, else 0.
fn label_height(has_label: bool) -> f32 {
    if has_label {
        font::height(TEXT_SCALE) + PAD
    } else {
        0.0
    }
}

/// The control body (the rect minus its label strip and a small inset). Shared
/// by drawing and hit-math so they agree on where the track is.
pub fn body_rect(rect: Rect, has_label: bool) -> Rect {
    let top = rect.y + label_height(has_label);
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

/// The fraction change from a vertical drag of `dy` device pixels over a body of
/// height `body_h`: dragging up (negative `dy`) increases the value. A full body
/// height is one full range; the factor keeps fine control on tall bodies.
pub fn drag_fraction_delta(dy: f64, body_h: f32) -> f32 {
    let span = body_h.max(40.0);
    (-(dy as f32)) / span
}

/// Draws a control into `mesh`. `active` highlights a pressed/dragged control.
pub fn draw(mesh: &mut Mesh, kind: &WidgetKind, rect: Rect, active: bool) {
    match kind {
        WidgetKind::Slider(r) => slider(mesh, r, rect),
        WidgetKind::Knob(r) => knob(mesh, r, rect),
        WidgetKind::Number(r) => number(mesh, r, rect),
        WidgetKind::Button { label } => button(mesh, label.as_deref(), rect, active),
        WidgetKind::Toggle { value, label } => toggle(mesh, *value, label.as_deref(), rect),
        WidgetKind::Text { value, label } => field(mesh, value, label.as_deref(), rect),
        WidgetKind::Menu {
            index,
            options,
            label,
        } => {
            let current = options.get(*index).map(String::as_str).unwrap_or("");
            field(mesh, current, label.as_deref(), rect);
        }
        _ => {}
    }
}

/// Draws the label strip above a control body, if it has a label.
fn label_strip(mesh: &mut Mesh, label: Option<&str>, rect: Rect) {
    if let Some(text) = label {
        font::text(mesh, text, rect.x + PAD, rect.y + PAD, TEXT_SCALE, TEXT);
    }
}

fn slider(mesh: &mut Mesh, r: &Range, rect: Rect) {
    label_strip(mesh, r.label.as_deref(), rect);
    let body = body_rect(rect, r.label.is_some());
    let mid = body.y + body.h * 0.5;
    let track_h = 4.0;
    mesh.rect(
        Rect::new(body.x, mid - track_h * 0.5, body.w, track_h),
        TRACK,
    );
    let hx = body.x + body.w * r.fraction();
    mesh.rect(
        Rect::new(body.x, mid - track_h * 0.5, (hx - body.x).max(0.0), track_h),
        ACCENT_DIM,
    );
    let handle_w = 8.0;
    mesh.rect(
        Rect::new(hx - handle_w * 0.5, body.y, handle_w, body.h),
        ACCENT,
    );
    value_text(mesh, &fmt(r.value), body);
}

fn knob(mesh: &mut Mesh, r: &Range, rect: Rect) {
    label_strip(mesh, r.label.as_deref(), rect);
    let body = body_rect(rect, r.label.is_some());
    let radius = (body.w.min(body.h) * 0.5 - 2.0).max(2.0);
    let cx = body.x + body.w * 0.5;
    let cy = body.y + body.h * 0.5;
    mesh.disc(cx, cy, radius, TRACK);
    mesh.disc(cx, cy, radius - 3.0, FIELD);
    // Pointer: 270-degree sweep, min at lower-left, max at lower-right.
    let angle = (135.0 + 270.0 * r.fraction()).to_radians();
    let tip = [cx + radius * angle.cos(), cy + radius * angle.sin()];
    mesh.line([cx, cy], tip, 3.0, ACCENT);
    value_text(
        mesh,
        &fmt(r.value),
        Rect::new(body.x, cy + radius * 0.5, body.w, body.h * 0.5),
    );
}

fn number(mesh: &mut Mesh, r: &Range, rect: Rect) {
    label_strip(mesh, r.label.as_deref(), rect);
    let body = body_rect(rect, r.label.is_some());
    mesh.rect(body, FIELD);
    // A vertical fill rising from the bottom shows the value in range, so
    // dragging up raises the green level; a border frames the field.
    let fill_h = body.h * r.fraction();
    mesh.rect(
        Rect::new(body.x, body.y + body.h - fill_h, body.w, fill_h),
        ACCENT_DIM,
    );
    border(mesh, body, 1.0, ACCENT);
    font::text_centered(mesh, &fmt(r.value), body, TEXT_SCALE, TEXT);
}

/// A 1-color outline of `rect`, `w` pixels thick (four thin rects).
fn border(mesh: &mut Mesh, rect: Rect, w: f32, color: Color) {
    mesh.rect(Rect::new(rect.x, rect.y, rect.w, w), color);
    mesh.rect(Rect::new(rect.x, rect.y + rect.h - w, rect.w, w), color);
    mesh.rect(Rect::new(rect.x, rect.y, w, rect.h), color);
    mesh.rect(Rect::new(rect.x + rect.w - w, rect.y, w, rect.h), color);
}

fn button(mesh: &mut Mesh, label: Option<&str>, rect: Rect, active: bool) {
    let body = body_rect(rect, false);
    mesh.rect(body, if active { HILITE } else { ACCENT_DIM });
    font::text_centered(mesh, label.unwrap_or("BUTTON"), body, TEXT_SCALE, TEXT);
}

fn toggle(mesh: &mut Mesh, on: bool, label: Option<&str>, rect: Rect) {
    let body = body_rect(rect, false);
    let box_side = body.h.min(body.w).min(24.0);
    let box_rect = Rect::new(
        body.x,
        body.y + (body.h - box_side) * 0.5,
        box_side,
        box_side,
    );
    mesh.rect(box_rect, TRACK);
    if on {
        let inset = box_side * 0.22;
        mesh.rect(
            Rect::new(
                box_rect.x + inset,
                box_rect.y + inset,
                box_side - 2.0 * inset,
                box_side - 2.0 * inset,
            ),
            ACCENT,
        );
    }
    if let Some(text) = label {
        let tx = box_rect.x + box_side + PAD;
        let ty = body.y + (body.h - font::height(TEXT_SCALE)) * 0.5;
        font::text(mesh, text, tx, ty, TEXT_SCALE, TEXT);
    }
}

fn field(mesh: &mut Mesh, value: &str, label: Option<&str>, rect: Rect) {
    label_strip(mesh, label, rect);
    let body = body_rect(rect, label.is_some());
    mesh.rect(body, FIELD);
    let ty = body.y + (body.h - font::height(TEXT_SCALE)) * 0.5;
    font::text(mesh, value, body.x + PAD, ty.max(body.y), TEXT_SCALE, TEXT);
}

/// A value read-out at the bottom-right of a body.
fn value_text(mesh: &mut Mesh, s: &str, body: Rect) {
    let w = font::width(s, TEXT_SCALE);
    let x = (body.x + body.w - w - PAD).max(body.x);
    let y = (body.y + body.h - font::height(TEXT_SCALE)).max(body.y);
    font::text(mesh, s, x, y, TEXT_SCALE, TEXT);
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
}
