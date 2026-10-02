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
//! # The window
//!
//! One widget: the catalogue's `bpf` picture
//! ([`clausters_document::view::catalogue`]) over the curve's points, on a
//! value axis and a time span that **only grow** while the window is open
//! ([`clausters_editing::points::axis`]) -- a curve that refits while a point is
//! dragged moves every other point on screen.
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

/// What the curve's header says: its name, or the word for one.
pub fn label(curve: &Automation) -> String {
    curve
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "curve".into())
}

/// **The props the curve is drawn with**: its points as the flat quads the
/// widget speaks, the value axis and the time they span -- each widened from
/// what `held` says the window has, and `held` moved on to it.
pub fn props(curve: &Automation, held: &mut Held) -> Map<String, Value> {
    let quads = points::quads(&curve.points);
    let out = points::props(&quads, held.axis, held.span);
    let axis = points::axis(&quads, held.axis, held.span);
    *held = Held {
        axis: Some((axis.min, axis.max)),
        span: axis.duration,
    };
    out
}

/// **The window**, as a GuiDef rooted at a `window` node: the curve under
/// `widget`, with the title and the size given. A script's own widgets are the
/// client's to append, as in every application here.
pub fn window(
    curve: &Automation,
    held: &mut Held,
    widget: i32,
    title: &str,
    size: (i64, i64),
) -> Value {
    let drawn = props(curve, held);
    let number = |key: &str| drawn.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    let mut picture = catalogue::bpf(&Curve {
        points: points::quads(&curve.points),
        min: number("min"),
        max: number("max"),
        duration: number("duration"),
        label: label(curve),
    });
    picture.insert("id".into(), json!(widget));
    json!({
        "type": "window",
        "title": title,
        "w": size.0,
        "h": size.1,
        "flow": "col",
        "children": [Value::Object(picture)],
    })
}

/// A curve of its own, for a caller that hands the editor data rather than a
/// curve it already shares: the request's `points` -- the flat `t v shape
/// curve` quads -- under its `name`.
pub fn shared_of(request: &Value) -> Shared {
    let flat: Vec<f64> = request
        .get("points")
        .and_then(Value::as_array)
        .map(|p| p.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let mut curve = Automation::new(clausters_document::NodeId(0), Default::default());
    curve.points = points::state(&flat);
    curve.name = request
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    Arc::new(Mutex::new(curve))
}
