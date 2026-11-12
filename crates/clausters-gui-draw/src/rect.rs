//! The rectangle every part of the host places, clips and hits by.
//!
//! It is the drawing base's first word: the layout pass assigns one to every
//! widget, a mesh clips to one, a glyph's outline is bounded by one and a hit
//! test asks one, so it sits below all of them and reads nothing.

/// A rectangle in physical pixels, top-left origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Grows the rectangle by `m` on every side -- a hit region's slop around
    /// the shape that was drawn.
    pub fn grown(self, m: f32) -> Rect {
        Rect {
            x: self.x - m,
            y: self.y - m,
            w: self.w + 2.0 * m,
            h: self.h + 2.0 * m,
        }
    }

    /// Shrinks the rectangle by `m` on every side (never below zero size).
    pub fn inset(self, m: f32) -> Rect {
        Rect {
            x: self.x + m,
            y: self.y + m,
            w: (self.w - 2.0 * m).max(0.0),
            h: (self.h - 2.0 * m).max(0.0),
        }
    }

    /// Whether `(px, py)` (physical pixels) falls inside the rectangle.
    pub fn contains(&self, px: f64, py: f64) -> bool {
        let (px, py) = (px as f32, py as f32);
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// The intersection with `other` (zero-sized when they do not overlap).
    pub fn intersect(&self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Rect {
            x,
            y,
            w: ((self.x + self.w).min(other.x + other.w) - x).max(0.0),
            h: ((self.y + self.h).min(other.y + other.h) - y).max(0.0),
        }
    }
}
