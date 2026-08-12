//! **Round hit shapes**: what "the pointer is on it" means for a thing that is
//! neither a rectangle nor a line.
//!
//! A hit-test reconstructs the geometry the renderer drew through — the rule
//! the whole half is held to — and most of what the host draws is a rectangle,
//! so [`Rect::contains`](crate::host::layout::Rect::contains) answered nearly
//! everything. The exceptions are the round ones, and they were reading as
//! their bounding box: a knob's disc grabbed from the corner of its cell where
//! there is nothing drawn, a notehead picked up by the paper diagonally beside
//! it. The corner of a box around a circle is a quarter of its area that never
//! belonged to it, and at a small radius that is most of what the pointer is
//! near.
//!
//! **Squared, never rooted.** A distance is compared here and never reported,
//! and `d <= r` is `d² <= r²` for non-negative numbers — so the square root
//! that would make the number a length is work with no reader. The same holds
//! for the ellipse, whose test is the normalized sum of two squares against 1.
//! What a caller may still want is *which* of several round targets is nearest,
//! and squared distance orders identically, so [`dist2`] serves that too.
//!
//! The shapes live here rather than in each model because the round things are
//! spread across the catalog — a control's disc, a break-point, a notehead —
//! and each one had reinvented the arithmetic or skipped it.

use crate::host::layout::Rect;

/// The squared distance from `(px, py)` to `(cx, cy)`.
///
/// Squared is the whole point: it orders exactly as the distance does, so
/// "which target is nearest" and "is it within `r`" both answer off it, and
/// neither needs the square root that would turn it into a length.
pub fn dist2(px: f64, py: f64, cx: f64, cy: f64) -> f64 {
    let (dx, dy) = (px - cx, py - cy);
    dx * dx + dy * dy
}

/// Whether `(px, py)` falls inside the disc of centre `(cx, cy)` and radius
/// `r` — Pythagoras with both sides squared.
pub fn in_disc(px: f64, py: f64, cx: f64, cy: f64, r: f64) -> bool {
    dist2(px, py, cx, cy) <= r * r
}

/// Whether `(px, py)` falls inside the ellipse inscribed in `rect` — the shape
/// a round glyph fills of the box measured around it.
///
/// The normalized form of the disc: each axis is divided by its own radius
/// before it is squared, so the test is `(dx/rx)² + (dy/ry)² <= 1` and stays
/// root-free. A zero-width or zero-height rect holds nothing rather than
/// dividing by zero.
pub fn in_ellipse(px: f64, py: f64, rect: Rect) -> bool {
    let (rx, ry) = (rect.w as f64 * 0.5, rect.h as f64 * 0.5);
    if rx <= 0.0 || ry <= 0.0 {
        return false;
    }
    let dx = (px - (rect.x as f64 + rx)) / rx;
    let dy = (py - (rect.y as f64 + ry)) / ry;
    dx * dx + dy * dy <= 1.0
}

/// The disc `rect` inscribes, centred in it and as wide as its shorter side —
/// the geometry a control's dial is drawn at, so its hit-test and its drawing
/// read one function.
pub fn disc_of(rect: Rect) -> (f32, f32, f32) {
    (
        rect.x + rect.w * 0.5,
        rect.y + rect.h * 0.5,
        rect.w.min(rect.h) * 0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corner of a box around a circle is not the circle: the point that
    /// used to hit is the whole reason this module exists.
    #[test]
    fn a_disc_does_not_answer_for_the_corners_of_its_box() {
        let (cx, cy, r) = (50.0, 50.0, 20.0);
        assert!(in_disc(50.0, 50.0, cx, cy, r), "the centre");
        assert!(in_disc(70.0, 50.0, cx, cy, r), "the rim, exactly");
        assert!(!in_disc(70.0, 70.0, cx, cy, r), "the corner of the box");
        // …and the ordering the square root would have given, without it.
        assert!(dist2(55.0, 50.0, cx, cy) < dist2(60.0, 50.0, cx, cy));
    }

    /// An ellipse is the disc with one radius per axis — a notehead is wider
    /// than it is tall, and its box's corners are paper.
    #[test]
    fn an_ellipse_is_the_disc_of_a_box_that_is_not_square() {
        let rect = Rect::new(0.0, 0.0, 40.0, 20.0);
        assert!(in_ellipse(20.0, 10.0, rect), "the centre");
        assert!(in_ellipse(39.0, 10.0, rect), "along the wide axis");
        assert!(!in_ellipse(39.0, 1.0, rect), "the corner");
        assert!(!in_ellipse(20.0, 21.0, rect), "outside the box entirely");
        assert!(!in_ellipse(1.0, 1.0, Rect::new(0.0, 0.0, 0.0, 20.0)));
    }

    #[test]
    fn the_inscribed_disc_takes_the_shorter_side() {
        let (cx, cy, r) = disc_of(Rect::new(10.0, 20.0, 40.0, 30.0));
        assert_eq!((cx, cy, r), (30.0, 35.0, 15.0));
    }
}
