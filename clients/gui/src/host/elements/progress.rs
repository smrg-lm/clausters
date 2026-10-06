//! `progress` -- how far something has got, or that it is still going.
//!
//! A bar over a `value` in `0..1`. With no value it is **indeterminate**: a
//! band that sweeps the bar, which says "working" without claiming how far --
//! and that is the only time it asks for the window's tick, since a bar with a
//! value is a still picture until somebody sets the next one.

use serde_json::{Map, Value};

use clausters_core::osc::OscType;

use crate::host::font;
use crate::host::graphics::controls;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::widget::element::{Ctx, Element, Live, Needs};
use crate::host::widget::parse;
use crate::host::widget::size::{Natural, body_inset, label_strip};

/// How long the indeterminate band takes to cross the bar, in seconds.
const SWEEP_S: f64 = 1.4;

/// A progress bar.
#[derive(Debug, Clone)]
pub struct Progress {
    /// `0..1`, or `None` for the indeterminate form.
    pub value: Option<f32>,
    pub label: Option<String>,
    pub text_size: f32,
    /// Where the indeterminate band is in its sweep, `0..1`, advanced by the
    /// tick.
    phase: f64,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

fn value_of(v: &Value) -> Option<f32> {
    v.as_f64()
        .filter(|n| n.is_finite())
        .map(|n| (n as f32).clamp(0.0, 1.0))
}

fn from_props(props: &Map<String, Value>) -> Progress {
    Progress {
        value: props.get("value").and_then(value_of),
        label: parse::label(props),
        text_size: parse::text_size(props),
        phase: 0.0,
    }
}

/// The bar's thickness: the groove a slider runs in, doubled -- it is read
/// from across a window, not grabbed.
fn bar_h(m: &Metrics) -> f32 {
    2.0 * m.track_thick
}

/// The filled part of `bar`: from the left up to `value`, or -- with none --
/// a band a quarter of the bar wide at `phase` of its sweep, entering from the
/// left edge and leaving by the right.
pub fn fill(bar: Rect, value: Option<f32>, phase: f64) -> Rect {
    match value {
        Some(v) => Rect::new(bar.x, bar.y, bar.w * v.clamp(0.0, 1.0), bar.h),
        None => {
            let band = bar.w * 0.25;
            let x = bar.x - band + (bar.w + band) * phase.clamp(0.0, 1.0) as f32;
            let left = x.max(bar.x);
            let right = (x + band).min(bar.x + bar.w);
            Rect::new(left, bar.y, (right - left).max(0.0), bar.h)
        }
    }
}

impl Progress {
    fn paint(&self, d: &mut Draw, ctx: &Ctx) {
        let size = self.text_size * ctx.scale;
        controls::label_strip(d, self.label.as_deref(), ctx.rect, size);
        let body = controls::body_rect_at(ctx.rect, self.label.is_some(), size, d.m);
        let (mesh, m, theme) = d.parts();
        let h = bar_h(m).min(body.h);
        let bar = Rect::new(body.x, body.y + (body.h - h) * 0.5, body.w, h);
        mesh.rect(bar, theme.track);
        mesh.rect(fill(bar, self.value, self.phase), theme.accent);
    }
}

impl Element for Progress {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // Anything that is not a number takes it back to indeterminate:
            // there is no fraction to show.
            "value" => {
                self.value = value_of(v);
                true
            }
            "label" => parse::set_label(&mut self.label, v),
            "text_size" => parse::set_size(&mut self.text_size, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        if self.value.is_some() {
            self.paint(d, ctx);
        }
    }

    /// A bar that sweeps is drawn on the live layer, where the tick moves it;
    /// one that shows a fraction is the window's picture, drawn again when it
    /// is set.
    fn draw_live(&self, d: &mut Draw, ctx: &Ctx) {
        if self.value.is_none() {
            self.paint(d, ctx);
        }
    }

    fn natural(&self, m: &Metrics, scale: f32) -> Natural {
        let size = self.text_size * scale;
        let label = label_strip(self.label.is_some(), size, m);
        // With a label it is a labelled control and lines up with the others;
        // bare it is only its bar.
        let body = if self.label.is_some() {
            font::height(size).max(bar_h(m))
        } else {
            bar_h(m)
        };
        (None, Some(label + body_inset(m) + body))
    }

    fn value(&self) -> Option<OscType> {
        self.value.map(OscType::Float)
    }

    /// The tick is asked for only while the bar is **moving** -- which is while
    /// it has no value. A bar that shows a fraction repaints when it is set.
    fn needs(&self) -> Needs {
        Needs {
            animated: self.value.is_none(),
            live: self.value.is_none(),
            ..Needs::default()
        }
    }

    fn tick(&mut self, live: &Live) {
        if self.value.is_none() {
            self.phase = (self.phase + live.dt / SWEEP_S).fract();
        }
    }

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

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn the_fill_follows_the_value_and_stays_inside_the_bar() {
        let bar = Rect::new(10.0, 0.0, 200.0, 8.0);
        assert_eq!(fill(bar, Some(0.25), 0.0).w, 50.0);
        assert_eq!(
            fill(bar, Some(7.0), 0.0).w,
            200.0,
            "a value past the end stops there"
        );
        for phase in [0.0, 0.1, 0.5, 0.9, 1.0] {
            let f = fill(bar, None, phase);
            assert!(f.x >= bar.x && f.x + f.w <= bar.x + bar.w + 0.01, "{f:?}");
        }
    }

    /// It asks for the tick only while it has nothing to show: a bar with a
    /// value is a still picture.
    #[test]
    fn only_the_indeterminate_bar_animates() {
        let mut p = from_props(&props("{}"));
        assert!(p.needs().animated);
        assert!(p.set("value", &serde_json::json!(0.4)));
        assert!(!p.needs().animated);
        assert_eq!(p.value(), Some(OscType::Float(0.4)));
        // Not a number: back to indeterminate.
        assert!(p.set("value", &Value::Null));
        assert!(p.needs().animated);
    }
}
