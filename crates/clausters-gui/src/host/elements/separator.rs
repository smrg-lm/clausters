//! `separator` -- a line between groups, and the spring that pushes what
//! follows it to the far edge.
//!
//! It has one thickness and no orientation of its own: in a row its cell is
//! narrow and tall, so it draws an upright line, and in a column it is the
//! other way round -- the cell it was given says which. With a `weight` it
//! stops being a line and becomes the leftover of its strip, which is how a
//! row of tools puts its last group against the right edge.

use serde_json::{Map, Value};

use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Ctx, Element};
use crate::host::widget::parse;
use crate::host::widget::size::Natural;

/// A line between two groups of a strip.
#[derive(Debug, Clone)]
pub struct Separator {
    /// The `line` prop: whether the line is drawn. Off, the separator is only
    /// its space -- a gap, or with a `weight` a spring.
    pub line: bool,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(Separator {
        line: props.get("line").and_then(parse::truthy).unwrap_or(true),
    }))
}

/// The room a separator takes along its strip: a hairline with a pad on either
/// side of it.
pub fn thickness(m: &Metrics) -> f32 {
    2.0 * m.pad + m.divider_w
}

/// The line itself inside `rect`: along the longer side, across the middle of
/// the shorter one.
pub fn line_rect(rect: Rect, m: &Metrics) -> Rect {
    if rect.h >= rect.w {
        Rect::new(
            rect.x + (rect.w - m.divider_w) * 0.5,
            rect.y + m.pad,
            m.divider_w,
            (rect.h - 2.0 * m.pad).max(0.0),
        )
    } else {
        Rect::new(
            rect.x + m.pad,
            rect.y + (rect.h - m.divider_w) * 0.5,
            (rect.w - 2.0 * m.pad).max(0.0),
            m.divider_w,
        )
    }
}

impl Element for Separator {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "line" => parse::truthy(v).map(|b| self.line = b).is_some(),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        if !self.line {
            return;
        }
        let (mesh, m, theme) = d.parts();
        mesh.rect(line_rect(ctx.rect, m), theme.separator);
    }

    /// The same on both axes: whichever one the strip runs along takes it, and
    /// the other fills.
    fn natural(&self, m: &Metrics, _scale: f32) -> Natural {
        (Some(thickness(m)), Some(thickness(m)))
    }

    /// It is not a thing to point at: the wheel and the press go to whatever
    /// is around it.
    fn is_bare_surface(&self) -> bool {
        true
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_runs_along_the_longer_side_of_its_cell() {
        let m = Metrics::default();
        let upright = line_rect(Rect::new(0.0, 0.0, thickness(&m), 40.0), &m);
        assert!(upright.h > upright.w, "in a row it stands");
        let flat = line_rect(Rect::new(0.0, 0.0, 200.0, thickness(&m)), &m);
        assert!(flat.w > flat.h, "in a column it lies");
    }

    #[test]
    fn it_takes_one_thickness_on_whichever_axis_its_strip_runs() {
        let m = Metrics::default();
        let s = Separator { line: true };
        assert_eq!(
            s.natural(&m, 1.0),
            (Some(thickness(&m)), Some(thickness(&m)))
        );
    }
}
