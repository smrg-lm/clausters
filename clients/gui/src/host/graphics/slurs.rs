//! **A slur over a roll's notes**, drawn as the owner's display says
//! (`slur_style`): an arc over the notes from the first box to the last, as a
//! page writes it; the same arc dashed where the slur attacks its notes; a
//! phrasing bracket over the highest box; or a faint band behind the notes,
//! over the slur's time and its register.
//!
//! What a slur spans is the boxes it is drawn over ([`Span`]), measured by
//! the roll; this is the geometry and the drawing, with no props and no
//! element, so it is tested without a window.

use crate::host::layout::Rect;
use crate::host::paint::{Color, Mesh};

/// **How a slur is drawn**, by the name the owner sends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    /// Not drawn.
    None,
    /// An arc over the notes, as a page writes it.
    #[default]
    Arc,
    /// The arc, dashed where the slur attacks its notes.
    Dashed,
    /// A phrasing bracket over the highest box.
    Bracket,
    /// A faint band behind the notes.
    Band,
}

impl Style {
    /// The style `name` says; an arc for a name none of them is.
    #[must_use]
    pub fn named(name: &str) -> Self {
        match name {
            "none" => Self::None,
            "dashed" => Self::Dashed,
            "bracket" => Self::Bracket,
            "band" => Self::Band,
            _ => Self::Arc,
        }
    }

    /// Whether it is drawn behind the boxes rather than over them.
    #[must_use]
    pub fn behind(self) -> bool {
        self == Self::Band
    }
}

/// **The boxes a slur is drawn over**, in the window's pixels: where its
/// first box starts and its last ends, the top of each, and the top and the
/// bottom of every box between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub x0: f32,
    pub x1: f32,
    /// The top of the first box and of the last.
    pub first_top: f32,
    pub last_top: f32,
    /// The highest top and the lowest bottom of the boxes it spans.
    pub top: f32,
    pub bottom: f32,
}

impl Span {
    /// The span of `boxes`, the slur's notes in time order, the first and the
    /// last at either end; `None` for none.
    #[must_use]
    pub fn over(first: Rect, last: Rect, boxes: &[Rect]) -> Option<Self> {
        let (x0, x1) = (first.x, last.x + last.w);
        if x1 <= x0 {
            return None;
        }
        let top = boxes
            .iter()
            .map(|b| b.y)
            .fold(first.y.min(last.y), f32::min);
        let bottom = boxes
            .iter()
            .map(|b| b.y + b.h)
            .fold((first.y + first.h).max(last.y + last.h), f32::max);
        Some(Self {
            x0,
            x1,
            first_top: first.y,
            last_top: last.y,
            top,
            bottom,
        })
    }

    /// How far an arc rises over the highest box: with its length, within
    /// bounds a short slur still reads at and a long one does not tower.
    fn rise(&self) -> f32 {
        ((self.x1 - self.x0) * 0.12).clamp(8.0, 30.0)
    }

    /// **The arc's points**, `steps` segments of a cubic from just above the
    /// first box's start to just above the last box's end, its handles over
    /// the highest box.
    #[must_use]
    pub fn arc(&self, steps: usize) -> Vec<[f32; 2]> {
        let inset = ((self.x1 - self.x0) * 0.08).min(8.0);
        let p0 = [self.x0 + inset, self.first_top - 2.0];
        let p3 = [self.x1 - inset, self.last_top - 2.0];
        let peak = self.top - self.rise();
        let reach = (self.x1 - self.x0) * 0.25;
        let p1 = [p0[0] + reach, peak];
        let p2 = [p3[0] - reach, peak];
        let steps = steps.max(2);
        (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                let u = 1.0 - t;
                let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                [
                    a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                    a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
                ]
            })
            .collect()
    }
}

/// **One slur drawn**: as `style` says, `joined` telling a dashed arc to be
/// solid, `width` its line and `color` its ink (a band takes it faint).
pub fn draw(mesh: &mut Mesh, style: Style, span: &Span, joined: bool, width: f32, color: Color) {
    match style {
        Style::None => {}
        Style::Arc | Style::Dashed => {
            let points = span.arc(24);
            let dashed = style == Style::Dashed && !joined;
            for (i, pair) in points.windows(2).enumerate() {
                // every other segment, for a dash about as long as a gap
                if dashed && i % 2 == 1 {
                    continue;
                }
                mesh.line(pair[0], pair[1], width, color);
            }
        }
        Style::Bracket => {
            let y = span.top - 8.0;
            let foot = 6.0;
            mesh.line([span.x0 + 1.0, y + foot], [span.x0 + 1.0, y], width, color);
            mesh.line([span.x0 + 1.0, y], [span.x1 - 1.0, y], width, color);
            mesh.line([span.x1 - 1.0, y], [span.x1 - 1.0, y + foot], width, color);
        }
        Style::Band => {
            let r = Rect::new(
                span.x0 - 3.0,
                span.top - 4.0,
                span.x1 - span.x0 + 6.0,
                span.bottom - span.top + 8.0,
            );
            let fill = [color[0], color[1], color[2], 0.13];
            let edge = [color[0], color[1], color[2], 0.5];
            mesh.rect(r, fill);
            let w = width.max(1.0);
            mesh.rect(Rect::new(r.x, r.y, r.w, w), edge);
            mesh.rect(Rect::new(r.x, r.y + r.h - w, r.w, w), edge);
            mesh.rect(Rect::new(r.x, r.y, w, r.h), edge);
            mesh.rect(Rect::new(r.x + r.w - w, r.y, w, r.h), edge);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span::over(
            Rect::new(10.0, 50.0, 20.0, 8.0),
            Rect::new(90.0, 60.0, 20.0, 8.0),
            &[Rect::new(40.0, 30.0, 20.0, 8.0)],
        )
        .unwrap()
    }

    /// **A span is the boxes it is drawn over**: the first's start to the
    /// last's end, the highest top and the lowest bottom between them.
    #[test]
    fn a_span_is_the_boxes_it_is_drawn_over() {
        let s = span();
        assert_eq!((s.x0, s.x1), (10.0, 110.0));
        assert_eq!((s.top, s.bottom), (30.0, 68.0));
        assert!(
            Span::over(
                Rect::new(50.0, 0.0, 0.0, 8.0),
                Rect::new(10.0, 0.0, 0.0, 8.0),
                &[]
            )
            .is_none()
        );
    }

    /// **The arc starts and ends just above its boxes and rises over the
    /// highest**, the way a page's slur clears the notes under it.
    #[test]
    fn the_arc_rises_over_the_highest_box() {
        let s = span();
        let points = s.arc(24);
        assert_eq!(points.len(), 25);
        let (first, last) = (points[0], points[24]);
        assert!(first[0] > s.x0 && first[1] < s.first_top);
        assert!(last[0] < s.x1 && last[1] < s.last_top);
        let highest = points.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        assert!(highest < s.top, "{highest} clears {}", s.top);
        assert_eq!(Style::named("band"), Style::Band);
        assert_eq!(Style::named("whatever"), Style::Arc);
        assert!(Style::Band.behind() && !Style::Dashed.behind());
    }
}
