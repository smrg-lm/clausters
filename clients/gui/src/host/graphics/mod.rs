//! **The models**: what a visual thing is shaped like, how it is drawn, and
//! where a click on that drawing lands.
//!
//! A leaf of the widget catalog is two files, and the split is by *who asks*.
//! The [`elements`](crate::host::elements) half is the surface the passes see: an
//! `impl Element` answering `set`, `needs`, `natural`, `slot`, `body_role` —
//! one method per question the frame, the gesture machine or a `/gui_query`
//! puts to it at a given moment. This half is what that element **knows**: the
//! data shape, the geometry, the drawing over a [`Draw`],
//! and the hit-test primitives that invert that drawing.
//!
//! The boundary is stated negatively, and it is what makes the half worth
//! separating: nothing here mentions [`Host`](super::Host), the
//! [`Element`](crate::host::widget::element::Element) trait, a props map, OSC or a
//! GPU device. Every module in it is unit-testable without a window, which is
//! the property the whole crate leans on for its coverage.
//!
//! The dependency runs **one way** — an element reads its model, never the
//! reverse — and the cardinality is why this is a parallel tree rather than a
//! file beside each element: a model may serve several elements (the standard
//! controls are one drawing for eight of them), may serve an element *and* a
//! container's body (a piano roll draws the `notes` leaf and a clip's inside),
//! and may serve no element at all (a `track` is a container, and its lane is
//! still drawn here).

pub mod bpf;
pub mod controls;
pub mod meters;
pub mod nodetree;
#[cfg(feature = "patcher")]
pub mod patch;
pub mod piano;
pub mod pianoroll;
#[cfg(feature = "notation")]
pub mod score;
pub mod shape;
pub mod signal;
pub mod textedit;
pub mod track;

use crate::host::font;
use crate::host::layout::Rect;
use crate::host::paint::Draw;

/// A read-out in a body's **top-right corner** — the slot a scope's
/// `lock`/`free` state, a spectral view's scale tag and the frame's own overlay
/// all put a short string in.
///
/// It sits here rather than in whichever model happened to write it first: four
/// callers across three modules and the frame's draw pass share one corner, and
/// a helper with that reach is the models' own vocabulary, not one widget's.
pub(crate) fn corner_text(d: &mut Draw, s: &str, body: Rect) {
    let (mesh, m, theme) = d.parts();
    let w = font::width(s, m.text_scale);
    let x = (body.x + body.w - w - m.pad).max(body.x);
    font::text(mesh, s, x, body.y + m.pad, m.text_scale, theme.text);
}

/// A short line drawn **over a picture**, on the translucent rounded ground
/// that makes it readable: a clip's name over its take, the cursor read-out
/// over a roll's notes. Truncated to `max_w` (the box that holds it) and
/// returning the plate it laid down, so a caller that stacks two of them knows
/// where the first one ended.
///
/// The plate is the answer to a defect an eye pass found twice: a caption over
/// a signal is written in one color and the signal draws in another, so
/// wherever the two meet the text disappears into the trace — and the denser
/// the drawing the less of the name survives. A ground of its own is what a
/// label over a picture needs, and it is one piece rather than one per widget
/// because the pixels a clip's name sits on and the ones a read-out sits on are
/// the same problem. The alpha keeps the picture legible *through* it (the
/// plate says "text here", it does not erase the picture under it), and the corners are
/// rounded so a small box over a dense drawing reads as one object rather than
/// as a hole cut in it.
///
/// `None` when the box cannot hold even the ellipsis: no plate under nothing.
// A string, where it goes, the room it has, and how it is inked: one drawing
// pass, clearer flat than bundled.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plate_text(
    d: &mut Draw,
    s: &str,
    x: f32,
    y: f32,
    max_w: f32,
    scale: f32,
    color: crate::host::paint::Color,
) -> Option<Rect> {
    let (mesh, m, theme) = d.parts();
    if s.is_empty() || max_w <= 0.0 {
        return None;
    }
    let w = font::width(s, scale).min(max_w);
    let h = font::height(scale);
    if w <= 0.0 {
        return None;
    }
    // The plate is padded around the glyphs by half the spacing role: enough
    // that the text is not touching an edge, and never so much that a caption
    // in a narrow clip loses room to its own ground.
    let inset = m.pad * 0.5;
    let plate = Rect::new(x - inset, y - inset, w + 2.0 * inset, h + 2.0 * inset);
    mesh.round_rect(plate, m.plate_radius, theme.plate);
    font::text_ellipsis(mesh, s, x, y, max_w, scale, color);
    Some(plate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::metrics::Metrics;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

    /// A plate is a **ground**, and the two properties that make it one: it is
    /// under the glyphs (so more geometry than the text alone), and it does not
    /// reach outside the room it was given (so a caption's ground cannot spill
    /// onto the neighbour the caption itself is truncated away from).
    #[test]
    fn a_text_plate_grounds_the_line_without_leaving_its_room() {
        let (m, theme) = (Metrics::default(), Theme::default());
        let room = 120.0;

        let mut bare = Mesh::new();
        crate::host::font::text(&mut bare, "take", 10.0, 10.0, m.caption_scale, theme.text);

        let mut plated = Mesh::new();
        let rect = plate_text(
            &mut Draw::new(&mut plated, &m, &theme),
            "take",
            10.0,
            10.0,
            room,
            m.caption_scale,
            theme.text,
        )
        .expect("a line with room draws");
        assert!(
            plated.vertex_count() > bare.vertex_count(),
            "the plate is drawn under the same glyphs"
        );
        assert!(
            rect.x >= 10.0 - m.pad && rect.x + rect.w <= 10.0 + room,
            "the plate leaves its room: {rect:?}"
        );

        // Truncated text takes a truncated plate: the ground follows the line
        // it grounds, never the string it would have drawn.
        let narrow = plate_text(
            &mut Draw::new(&mut Mesh::new(), &m, &theme),
            "a much longer name",
            10.0,
            10.0,
            40.0,
            m.caption_scale,
            theme.text,
        )
        .expect("a narrow box still says what it can");
        assert!(narrow.w <= 40.0 + m.pad, "the plate is as wide as the line");

        // No room at all: no plate under nothing.
        assert!(
            plate_text(
                &mut Draw::new(&mut Mesh::new(), &m, &theme),
                "take",
                10.0,
                10.0,
                0.0,
                m.caption_scale,
                theme.text,
            )
            .is_none()
        );
    }
}
