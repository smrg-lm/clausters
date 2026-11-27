//! **The value ruler**: the time ruler turned on its side, a `field` that
//! rules the vertical of the view it names.
//!
//! A view on a time axis rules its own vertical in a strip it reserves at its
//! left, unless an application stands one of these beside it instead
//! (`axes: {y: {rules: <id>, dir: "left"}}`) and turns the view's own strip
//! off (`axes.y.strip` false). Then the view reserves nothing and this element
//! is where its vertical is drawn and grabbed: the frame draws the view's own
//! ticks -- its unit, its window, its rows, its folded channels -- into this
//! rectangle, at the heights of the view's rows, and a press, a drag or a
//! wheel here is the view's vertical gesture. Named, a pane rules through the
//! body in hand, as its own strip does.
//!
//! It faces the view (`dir`): `left` when it stands on the view's right, its
//! marks along its left edge, and `right` when it stands on the view's left --
//! the mirror, as the time ruler under a view is the mirror of the one over
//! it. The drawing is the one ruler's (`host::ruler`), so a ruler here and a
//! strip of the view's own are the same marks.
//!
//! Its width is its labels', which are the data's: the role, or what the
//! view's labels need as it was placed (`Element::widen`, measured by the
//! layout after its first pass, the way a view's own strip is measured).

use std::cell::Cell;

use serde_json::{Map, Value};

use crate::host::widget::RulerDir;
use crate::host::widget::element::{Ctx, Element};
use crate::host::widget::size::Natural;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::metrics::Metrics;

/// A free-standing ruler of one view's vertical.
#[derive(Debug, Clone)]
pub struct ValueRuler {
    /// The widget id of the view it rules.
    pub rules: i32,
    /// The way it faces that view.
    pub dir: RulerDir,
    /// The width its labels were last measured to need, physical pixels.
    measured: Cell<f32>,
}

pub(crate) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    let rules = props
        .get("rules")
        .and_then(Value::as_i64)
        .ok_or("a value ruler names the view it rules (`rules`)")?;
    Ok(Box::new(ValueRuler {
        rules: rules as i32,
        dir: vertical(RulerDir::parse(props, RulerDir::Right)),
        measured: Cell::new(0.0),
    }))
}

/// A value ruler faces left or right; a `dir` that says up or down is read as
/// the strip a view keeps of its own, on its left.
fn vertical(dir: RulerDir) -> RulerDir {
    match dir {
        RulerDir::Left => RulerDir::Left,
        _ => RulerDir::Right,
    }
}

impl Element for ValueRuler {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "rules" => v.as_i64().map(|id| self.rules = id as i32).is_some(),
            "dir" => {
                let mut props = Map::new();
                props.insert("dir".into(), v.clone());
                self.dir = vertical(RulerDir::parse(&props, self.dir));
                true
            }
            _ => false,
        }
    }

    /// Nothing of its own: the ticks are the view's, drawn by the frame into
    /// this rectangle with the view's rows (`rules_y`).
    fn draw(&self, _d: &mut Draw, _ctx: &Ctx) {}

    /// As wide as its labels, elastic along the view it stands beside.
    fn natural(&self, m: &Metrics, _scale: f32) -> Natural {
        (Some(m.ruler_w.max(self.measured.get())), None)
    }

    fn rules_y(&self) -> Option<(i32, RulerDir)> {
        Some((self.rules, self.dir))
    }

    fn widen(&self, px: f32) {
        self.measured.set(px);
    }

    fn info(&self) -> Vec<(String, Value)> {
        vec![
            ("rules".into(), Value::from(self.rules)),
            (
                "dir".into(),
                Value::from(if self.dir == RulerDir::Left {
                    "left"
                } else {
                    "right"
                }),
            ),
        ]
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `field` naming a view is a value ruler of it, facing the way it
    /// says, and as wide as its labels once measured.
    #[test]
    fn a_value_ruler_rules_the_view_it_names() {
        let mut props = Map::new();
        props.insert("rules".into(), Value::from(12));
        props.insert("dir".into(), Value::from("left"));
        let ruler = build(&props, &[]).unwrap();
        assert_eq!(ruler.rules_y(), Some((12, RulerDir::Left)));
        let m = Metrics::default();
        assert_eq!(ruler.natural(&m, 1.0), (Some(m.ruler_w), None));
        ruler.widen(m.ruler_w + 9.0);
        assert_eq!(ruler.natural(&m, 1.0), (Some(m.ruler_w + 9.0), None));
        props.remove("rules");
        assert!(build(&props, &[]).is_err(), "it names what it rules");
    }
}
