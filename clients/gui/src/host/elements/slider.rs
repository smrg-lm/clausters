//! `slider` — a value in a groove, dragged where you point.
//!
//! The **absolute** drag, and the leaf the family was designed against: the
//! press jumps the value to where it landed and holds the groove, and every
//! motion after it is the same question asked again. The groove is the one the
//! renderer drew (`controls::slider_track`), snapshotted at the press, which is
//! the state the gesture machine used to hold because the widget could not.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::controls;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Claim, Ctx, Element, Events, Input};
use crate::host::widget::size::{Natural, slider_across, slider_thick};
use crate::host::widget::{Range, parse};

use super::control::{self, Track};

/// A continuous value over `min`..`max`, dragged along its groove — across the
/// cell, or up it when `vertical`.
#[derive(Debug, Clone)]
pub struct Slider {
    pub range: Range,
    pub vertical: bool,
    drag: Track,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn from_props(props: &Map<String, Value>) -> Slider {
    Slider {
        range: Range::parse(props),
        vertical: props
            .get("vertical")
            .and_then(parse::truthy)
            .unwrap_or(false),
        drag: Track::default(),
    }
}

impl Slider {
    /// The groove, not the whole body: the grab has to agree with what the
    /// renderer drew, at the placement's own size table.
    fn track(&self, input: &Input) -> crate::host::layout::Rect {
        controls::slider_track(
            input.rect,
            self.range.label.is_some(),
            self.range.text_size * input.scale,
            input.metrics,
        )
    }
}

impl Element for Slider {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "vertical" => parse::truthy(v).map(|b| self.vertical = b).is_some(),
            _ => control::set(&mut self.range, key, v),
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        controls::slider(
            d,
            &self.range,
            ctx.rect,
            self.vertical,
            self.range.text_size * ctx.scale,
        );
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        if self.vertical {
            (Some(slider_across(m)), None)
        } else {
            (None, Some(slider_thick(&self.range, m, scale)))
        }
    }

    fn value(&self) -> Option<OscType> {
        control::value(&self.range)
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        let track = self.track(input);
        self.drag.press(&mut self.range, track, self.vertical, at)
    }

    fn drag(&mut self, at: (f64, f64), _input: &Input) -> Events {
        self.drag.drag(&mut self.range, at)
    }

    fn release(&mut self, _at: (f64, f64), _input: &Input) -> Events {
        self.drag.release();
        Events::none()
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::widget::element::Mods;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn input<'a>(m: &'a Metrics, rect: crate::host::layout::Rect) -> Input<'a> {
        Input {
            metrics: m,
            rect,
            scale: 1.0,
            mods: Mods::default(),
            viewport: (400.0, 300.0),
            time: None,
        }
    }

    #[test]
    fn props_parse_and_default() {
        let s = from_props(&props(r#"{"min":-1,"max":1,"value":0.5,"vertical":1}"#));
        assert_eq!((s.range.min, s.range.max, s.range.value), (-1.0, 1.0, 0.5));
        assert!(s.vertical);

        let s = from_props(&props("{}"));
        assert_eq!((s.range.min, s.range.max, s.range.value), (0.0, 1.0, 0.0));
        assert!(!s.vertical);
    }

    /// The press *is* the first step of the drag — a slider jumps to where you
    /// pointed — and every motion after it asks the same question of the same
    /// groove, which the element holds because it is the element's.
    #[test]
    fn the_press_lands_the_value_and_the_drag_follows_it() {
        let m = Metrics::default();
        let rect = crate::host::layout::Rect::new(0.0, 0.0, 200.0, 40.0);
        let mut s = from_props(&props("{}"));
        let claim = s.press((100.0, 20.0), &input(&m, rect));
        let mid = s.range.value;
        assert!(mid > 0.4 && mid < 0.6, "landed mid-groove: {mid}");
        assert_eq!(claim, Claim::value(OscType::Float(mid)));

        s.drag((0.0, 20.0), &input(&m, rect));
        assert_eq!(s.range.value, 0.0, "clamped at the low end");
        s.drag((10_000.0, 20.0), &input(&m, rect));
        assert_eq!(s.range.value, 1.0, "and at the high one");
    }

    /// A motion with no press behind it moves nothing: the groove is held only
    /// while the press is, and the release drops it.
    #[test]
    fn a_drag_without_a_press_moves_nothing() {
        let m = Metrics::default();
        let rect = crate::host::layout::Rect::new(0.0, 0.0, 200.0, 40.0);
        let mut s = from_props(&props(r#"{"value":0.25}"#));
        assert!(s.drag((180.0, 20.0), &input(&m, rect)).is_empty());
        assert_eq!(s.range.value, 0.25);

        s.press((100.0, 20.0), &input(&m, rect));
        s.release((100.0, 20.0), &input(&m, rect));
        assert!(s.drag((180.0, 20.0), &input(&m, rect)).is_empty());
    }

    /// A vertical slider reads the same drag up its own axis, and declares its
    /// width instead of its thickness.
    #[test]
    fn a_vertical_slider_reads_the_other_axis() {
        let m = Metrics::default();
        let rect = crate::host::layout::Rect::new(0.0, 0.0, 40.0, 200.0);
        let mut s = from_props(&props(r#"{"vertical":1}"#));
        s.press((20.0, 0.0), &input(&m, rect));
        assert_eq!(
            s.range.value, 1.0,
            "the top of a vertical groove is the max"
        );
        assert_eq!(s.natural(&m, 1.0), (Some(slider_across(&m)), None));
    }
}
