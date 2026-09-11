//! **A break-point curve as the props a host draws it with.**
//!
//! The smallest projection there is, and the shape all of them have: the
//! structure comes in as the flat quads the `bpf` widget speaks, whatever the
//! view is already holding comes in beside it, and what goes back is the props
//! — the points, the value axis, and the time the curve spans.
//!
//! # Both axes only grow, and that is the whole of the rule
//!
//! A range recomputed on every redraw makes an edit rescale the picture, so
//! dragging one point visibly moves every other one. The value axis is
//! [`clausters_core::envshape::curve_axis`]'s — the data's range with headroom
//! the first time, widened afterwards only where the data stopped fitting. The
//! time axis is the same rule with nothing to pad: the last point's time, and
//! never shorter than it has been.
//!
//! # What the caller still holds
//!
//! The axis and the span it settled on, which come back in the props and go in
//! again next time. They are **view state** — what a window is looking at —
//! and view state belongs to whoever is looking, not to a projection.

use serde_json::{Map, Value, json};

use clausters_core::envshape::curve_axis;

/// What the `bpf` widget sends and takes: flat `t v shape curve` quads.
pub const QUAD: usize = 4;

/// The value axis and the time span a curve is drawn against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis {
    pub min: f64,
    pub max: f64,
    /// The time the curve spans: its last point, never shorter than `held`.
    pub duration: f64,
}

/// The axis `points` is drawn against, given the one the view already has.
///
/// `kept` is `None` for a curve being drawn for the first time; `held` is the
/// span in hand, `0.0` for the same case.
pub fn axis(points: &[f64], kept: Option<(f64, f64)>, held: f64) -> Axis {
    let mut values = Vec::with_capacity(points.len() / QUAD);
    let mut duration = held;
    for quad in points.as_chunks::<QUAD>().0 {
        duration = duration.max(quad[0]);
        values.push(quad[1]);
    }
    let (min, max) = curve_axis(&values, kept);
    Axis { min, max, duration }
}

/// The props a `bpf` is drawn with: the points, the axis they stand on, and
/// the time they span.
///
/// `duration` is written only when there is one — a curve with a single point
/// at time zero spans nothing, and stating a zero duration would pin the widget
/// to an axis of no width rather than letting it keep the one it has.
pub fn props(points: &[f64], kept: Option<(f64, f64)>, held: f64) -> Map<String, Value> {
    let axis = axis(points, kept, held);
    let mut out = Map::new();
    out.insert("points".into(), json!(points));
    out.insert("min".into(), json!(axis.min));
    out.insert("max".into(), json!(axis.max));
    if axis.duration > 0.0 {
        out.insert("duration".into(), json!(axis.duration));
    }
    out
}

/// [`props`] as a JSON object, which is what the two doors carry.
pub fn props_json(points: &[f64], kept: Option<(f64, f64)>, held: f64) -> String {
    Value::Object(props(points, kept, held)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trailing partial quad is dropped rather than guessed at — the rule
    /// every flat payload in this system follows.
    #[test]
    fn a_partial_quad_is_not_a_point() {
        let whole = axis(&[0.0, 1.0, 1.0, 0.0, 2.0, 3.0, 1.0, 0.0], None, 0.0);
        let ragged = axis(&[0.0, 1.0, 1.0, 0.0, 2.0, 3.0, 1.0], None, 0.0);
        assert_eq!(whole.duration, 2.0);
        assert_eq!(ragged.duration, 0.0, "the half point says nothing");
        let alone = axis(&[0.0, 1.0, 1.0, 0.0], None, 0.0);
        assert_eq!(
            (ragged.min, ragged.max),
            (alone.min, alone.max),
            "and its value is not read either"
        );
    }

    /// **Neither axis narrows.** A point dragged down and back up leaves the
    /// drawing where it was, and a curve shortened keeps the time it had.
    #[test]
    fn an_axis_in_hand_is_widened_and_never_narrowed() {
        let first = axis(&[0.0, 0.0, 1.0, 0.0, 4.0, 1.0, 1.0, 0.0], None, 0.0);
        let kept = Some((first.min, first.max));
        let smaller = axis(
            &[0.0, 0.0, 1.0, 0.0, 2.0, 0.5, 1.0, 0.0],
            kept,
            first.duration,
        );
        assert_eq!((smaller.min, smaller.max), (first.min, first.max));
        assert_eq!(smaller.duration, 4.0, "the span it has been is the span");

        let bigger = axis(
            &[0.0, 0.0, 1.0, 0.0, 9.0, 4.0, 1.0, 0.0],
            kept,
            first.duration,
        );
        assert!(bigger.max > first.max, "and it grows where the data left");
        assert_eq!(bigger.duration, 9.0);
    }

    /// A curve that spans nothing states no duration, rather than stating zero.
    #[test]
    fn a_curve_that_spans_nothing_says_nothing_about_time() {
        let flat = props(&[0.0, 0.5, 1.0, 0.0], None, 0.0);
        assert!(!flat.contains_key("duration"));
        assert!(flat.contains_key("min") && flat.contains_key("max"));

        let spanning = props(&[0.0, 0.5, 1.0, 0.0, 3.0, 0.5, 1.0, 0.0], None, 0.0);
        assert_eq!(spanning["duration"], json!(3.0));
    }
}
