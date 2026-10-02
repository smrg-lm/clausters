//! **The points editor**: a curve over one parameter, seen on its own.
//!
//! What it opens is an [`Automation`] -- the document's curve, the one a
//! track, a region, a sequence and a note each hold -- with nothing around it.
//! A curve with no holder has no holder's time either, so its `at` is whatever
//! its caller means by it, which for an envelope is seconds: an envelope is
//! made here as an automation nothing holds yet.
//!
//! The editor edits the curve **in place**: it is shared with whoever holds it
//! (`Shared`), and a caller whose curve is an object of its own (a client's
//! envelope) is handed the points after every change to write back.
//!
//! # The rules
//!
//! A curve is edited inside its **ranges** ([`Rules`]): its values inside the
//! range of the parameter it automates ([`clausters_editing::points::range`],
//! the rule the roll and the multitrack read too) or the one its caller
//! declares, and its time inside the one its caller declares. A range is a
//! rule and not a picture: the axis is the range and holds, and every point a
//! gesture reports is kept inside it. A curve with no range -- an envelope
//! nobody declared one for -- is drawn on an axis that **only grows** while the
//! window is open ([`clausters_editing::points::axis`]), since a curve that
//! refits while a point is dragged moves every other point on screen.
//!
//! # The window
//!
//! One widget: the catalogue's `bpf` picture
//! ([`clausters_document::view::catalogue`]) over the curve's points, with its
//! rules **in sight**: the time ruler under it, in the curve's own seconds,
//! and the value ruler beside it, over the range.
//!
//! # A turn
//!
//! A gesture is read by [`clausters_editing::points::intake`] into the one edit
//! the `points` vocabulary has -- the points are now these -- applied to the
//! shared curve and recorded with the points as they were as its inverse. It
//! is the edit a multitrack addresses to a curve it holds (`SetAutomation`):
//! a curve edited here and in a row is the same edit.

pub mod editor;

use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use clausters_document::multitrack::Automation;
use clausters_document::view::catalogue::{self, Curve};
use clausters_editing::points;

/// A curve the editor and its holder edit together.
pub type Shared = Arc<Mutex<Automation>>;

/// **The axes a window has drawn the curve against**: the value axis and the
/// time span, kept between redraws because both only grow. View state, the
/// window's, never part of what is edited.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Held {
    /// The value axis, once drawn -- or the one the caller declared.
    pub axis: Option<(f64, f64)>,
    /// The time the curve has spanned.
    pub span: f64,
}

/// **The ranges a curve is edited inside**, where it has them: its values and
/// its time, each `(low, high)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rules {
    /// The range its values are kept in.
    pub values: Option<(f64, f64)>,
    /// The range its points' times are kept in.
    pub time: Option<(f64, f64)>,
}

impl Rules {
    /// **The rules `curve` is edited under**: what the caller `declared`, and
    /// where it declared no value range, the range of the parameter the curve
    /// automates -- none for a curve that automates nothing it says.
    pub fn of(curve: &Automation, declared: Rules) -> Rules {
        Rules {
            values: declared.values.or_else(|| {
                let says = match &curve.target.0 {
                    Value::Null => false,
                    Value::Object(target) => !target.is_empty(),
                    _ => true,
                };
                says.then(|| points::range(&curve.target.0))
            }),
            time: declared.time,
        }
    }

    /// **Flat `t v shape curve` quads kept inside the ranges**: each time and
    /// each value clamped into its own. Clamping keeps the order the times
    /// were in, so a curve that was in order stays in order.
    pub fn keep(&self, quads: &mut [f64]) {
        for quad in quads.as_chunks_mut::<{ points::QUAD }>().0 {
            if let Some((lo, hi)) = self.time {
                quad[0] = quad[0].clamp(lo, hi);
            }
            if let Some((lo, hi)) = self.values {
                quad[1] = quad[1].clamp(lo, hi);
            }
        }
    }
}

/// What the curve's header says: its name, or the word for one.
pub fn label(curve: &Automation) -> String {
    curve
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "curve".into())
}

/// **The props the curve is drawn with**: its points as the flat quads the
/// widget speaks, the value axis and the time they span -- each the range
/// `rules` keep it in, or where there is none, widened from what `held` says
/// the window has; and `held` moved on to it.
pub fn props(curve: &Automation, held: &mut Held, rules: &Rules) -> Map<String, Value> {
    let quads = points::quads(&curve.points);
    let mut out = points::props(&quads, held.axis, held.span);
    let axis = points::axis(&quads, held.axis, held.span);
    let (min, max) = rules.values.unwrap_or((axis.min, axis.max));
    let span = rules.time.map_or(axis.duration, |(_, end)| end);
    out.insert("min".into(), json!(min));
    out.insert("max".into(), json!(max));
    if span > 0.0 {
        out.insert("duration".into(), json!(span));
    }
    *held = Held {
        axis: Some((min, max)),
        span,
    };
    out
}

/// **The window**, as a GuiDef rooted at a `window` node: the curve under
/// `widget`, with the title and the size given. A script's own widgets are the
/// client's to append, as in every application here.
pub fn window(
    curve: &Automation,
    held: &mut Held,
    rules: &Rules,
    ids: Ids,
    title: &str,
    size: (i64, i64),
) -> Value {
    let widget = ids.curve;
    let drawn = props(curve, held, rules);
    let number = |key: &str| drawn.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let mut picture = catalogue::bpf(&Curve {
        points: points::quads(&curve.points),
        min: number("min"),
        max: number("max"),
        duration: number("duration"),
        label: label(curve),
    });
    picture.insert("id".into(), json!(widget));
    // **The rules in sight**: the time ruler, labelled in the curve's own
    // seconds -- a rate of one makes a unit of the axis a second -- and the
    // value ruler over the range.
    picture.insert("ruler".into(), json!("time"));
    picture.insert("ruler_y".into(), json!("value"));
    picture.insert("sample_rate".into(), json!(1.0));
    json!({
        "type": "window",
        "title": title,
        "w": size.0,
        "h": size.1,
        "flow": "col",
        "children": [Value::Object(picture), controls(ids.shape)],
    })
}

/// **The window's widget ids**, which are the caller's: the curve's, and the
/// shape menu's in the row under it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ids {
    /// The curve.
    pub curve: i32,
    /// The menu that sets the selected segment's shape.
    pub shape: Option<i32>,
}

/// How tall the row of controls under the curve is.
pub const CONTROLS_H: f64 = 40.0;

/// **The shapes a segment can take**, in the order of their numbers, so a
/// menu's index is the shape: what an envelope's segments are written with.
pub fn shapes() -> Vec<&'static str> {
    (0..=clausters_core::envshape::SHAPE_HOLD)
        .map(clausters_core::envshape::shape_name)
        .collect()
}

/// **The row of controls under the curve**: the menu that sets the selected
/// segment's shape, side by side with whatever the client appends to it, the
/// lot in a quarter of the width and the rest left empty.
fn controls(shape: Option<i32>) -> Value {
    let mut menu = json!({
        "type": "menu",
        "name": "shape",
        "options": shapes(),
        "index": clausters_core::envshape::SHAPE_LINEAR,
    });
    if let (Some(id), Some(map)) = (shape, menu.as_object_mut()) {
        map.insert("id".into(), json!(id));
    }
    json!({
        "type": "layout",
        "flow": "row",
        "h": CONTROLS_H,
        "gap": 6.0,
        "children": [
            {"type": "layout", "flow": "row", "weight": 1.0, "gap": 6.0, "children": [menu]},
            {"type": "layout", "weight": 3.0},
        ],
    })
}

/// A curve of its own, for a caller that hands the editor data rather than a
/// curve it already shares: the request's `points` -- the flat `t v shape
/// curve` quads -- under its `name`, automating its `target`.
pub fn shared_of(request: &Value) -> Shared {
    let flat: Vec<f64> = request
        .get("points")
        .and_then(Value::as_array)
        .map(|p| p.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let mut curve = Automation::new(clausters_document::NodeId(0), Default::default());
    curve.points = points::state(&flat);
    if let Some(target) = request.get("target").filter(|t| !t.is_null()) {
        curve.target = clausters_document::Opaque(target.clone());
    }
    curve.name = request
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    Arc::new(Mutex::new(curve))
}
