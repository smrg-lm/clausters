//! **A break-point**: the shape of an envelope, of an automation and of any
//! other curve a hand draws point by point.
//!
//! One structure for all of them, which is the same decision the wire makes: a
//! point is a time, a value, a shape and the amount that shape takes, and where
//! the curve *hangs* — a widget of its own, a row under a track, a layer inside
//! a box — changes nothing about what it is. What a hand does to one is here
//! too: place it between its neighbours, add one that inherits the segment it
//! split, remove one, bend the segment either side of it.
//!
//! The **value** axis is part of the structure and not of the picture: a curve
//! is read in a range, linearly or geometrically ([`value_fraction`] and its
//! inverse), because that is what the numbers *mean* — a gain curve read in
//! decibels is not the same curve read in amplitude, whoever draws it. Where
//! those fractions land in pixels is `graphics::bpf`'s, and the two meet at the
//! fraction.

use clausters_core::envshape::{SHAPE_CURVE, SHAPE_LINEAR, shape_value};
use clausters_core::osc::OscType;
use serde_json::Value;

use crate::viewport::{Axis, Unit};

/// The custom-curvature clamp — past this the segment is visually a step.
const CURVE_LIMIT: f32 = 32.0;

/// One breakpoint: its position, and the shape/curve of the segment *leaving*
/// it (unused on the last point).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BpfPoint {
    pub time: f64,
    pub value: f32,
    pub shape: i32,
    pub curve: f32,
}

/// Parses the `points` property: a flat `[t, v, shape, curve, …]` JSON array
/// (or that array as a JSON string, the `/gui_set` carrier — OSC key/value
/// pairs are scalars). Incomplete trailing quads are dropped; the points are
/// sorted by time and their values clamped into `[lo, hi]`. `None` when the
/// value is not an array at all.
pub fn parse_points(v: &Value, lo: f32, hi: f32) -> Option<Vec<BpfPoint>> {
    let items = match v {
        Value::Array(items) => items.as_slice(),
        Value::String(s) => {
            let parsed = serde_json::from_str::<Value>(s).ok()?;
            return match parsed {
                Value::Array(_) => parse_points(&parsed, lo, hi),
                _ => None,
            };
        }
        _ => return None,
    };
    let mut points: Vec<BpfPoint> = items
        .as_chunks::<4>()
        .0
        .iter()
        .filter_map(|q| {
            Some(BpfPoint {
                time: q[0].as_f64()?.max(0.0),
                value: (q[1].as_f64()? as f32).clamp(lo, hi),
                // **A shape written as a float is still that shape.** The
                // multitrack hands its curves their points as one `f64` run,
                // so a bent segment arrived as `5.0`, read linear, and every
                // redraw straightened what the hand had just bent.
                shape: q[2]
                    .as_i64()
                    .or_else(|| q[2].as_f64().map(|s| s as i64))
                    .unwrap_or(SHAPE_LINEAR as i64) as i32,
                curve: q[3].as_f64().unwrap_or(0.0) as f32,
            })
        })
        .collect();
    points.sort_by(|a, b| a.time.total_cmp(&b.time));
    Some(points)
}

/// The default envelope when a def names no points: a flat line at `lo` over a
/// unit domain — predictable, and immediately editable.
pub fn default_points(lo: f32) -> Vec<BpfPoint> {
    vec![
        BpfPoint {
            time: 0.0,
            value: lo,
            shape: SHAPE_LINEAR,
            curve: 0.0,
        },
        BpfPoint {
            time: 1.0,
            value: lo,
            shape: SHAPE_LINEAR,
            curve: 0.0,
        },
    ]
}

/// The drawn time domain: the `duration` prop when positive, else the last
/// breakpoint's time, else 1 (so an empty or degenerate list still lays out).
pub fn domain(points: &[BpfPoint], duration: f64) -> f64 {
    if duration > 0.0 {
        duration
    } else {
        points.last().map_or(1.0, |p| p.time).max(1e-9)
    }
}

/// The envelope's value at time `t`: the first value before the first point,
/// the last value after the last, the segment's shape interpolation between.
pub fn value_at(points: &[BpfPoint], t: f64) -> f32 {
    let Some(first) = points.first() else {
        return 0.0;
    };
    if t <= first.time {
        return first.value;
    }
    for pair in points.windows(2) {
        let (p, q) = (pair[0], pair[1]);
        if t < q.time {
            let frac = ((t - p.time) / (q.time - p.time).max(1e-12)) as f32;
            return shape_value(p.shape, p.curve, p.value, q.value, frac);
        }
    }
    points.last().map_or(0.0, |p| p.value)
}

/// The 0..1 display fraction of `value` in `[lo, hi]` — linear, or geometric
/// when `exp` (frequency-like ranges; requires `0 < lo < hi`, falling back to
/// linear otherwise). Inverse of [`fraction_to_value`].
pub fn value_fraction(value: f32, lo: f32, hi: f32, exp: bool) -> f32 {
    if exp && lo > 0.0 && hi > lo {
        ((value.max(lo) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0)
    } else {
        // The linear branch is the shared value axis; only the geometric one
        // above is this widget's own.
        Axis::ranged(lo as f64, hi as f64, Unit::Norm).fraction_clamped(value as f64) as f32
    }
}

/// A 0..1 display fraction back to a value in `[lo, hi]` (see
/// [`value_fraction`]).
pub fn fraction_to_value(t: f32, lo: f32, hi: f32, exp: bool) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if exp && lo > 0.0 && hi > lo {
        lo * (hi / lo).powf(t)
    } else {
        lo + t * (hi - lo)
    }
}

/// Places breakpoint `i` at time `t` and `value` — the mapping-free core of a
/// point drag: the time stays monotonic (clamped between its neighbours, and
/// into `[0, dom]`), the value is taken as given (the caller mapped it out of
/// its own display range). The pixel-mapped [`Axes::move_point`](crate::host::graphics::bpf::Axes::move_point) edits through
/// here, so an envelope behaves the same wherever it is drawn.
pub fn place_point(points: &mut [BpfPoint], i: usize, t: f64, value: f32, dom: f64) {
    if i >= points.len() {
        return;
    }
    let t_lo = if i == 0 { 0.0 } else { points[i - 1].time };
    let t_hi = if i + 1 < points.len() {
        points[i + 1].time
    } else {
        dom
    };
    points[i].time = t.clamp(t_lo.min(t_hi), t_hi);
    points[i].value = value;
}

/// Inserts a breakpoint at `(t, value)`, inheriting the split segment's shape
/// and curve (linear before the first point); returns its index. The
/// mapping-free core of [`Axes::add_point`](crate::host::graphics::bpf::Axes::add_point).
pub fn insert_point(points: &mut Vec<BpfPoint>, t: f64, value: f32) -> usize {
    let i = points.partition_point(|p| p.time <= t);
    let (shape, curve) = if i > 0 {
        (points[i - 1].shape, points[i - 1].curve)
    } else {
        (SHAPE_LINEAR, 0.0)
    };
    points.insert(
        i,
        BpfPoint {
            time: t,
            value,
            shape,
            curve,
        },
    );
    i
}

/// Removes breakpoint `i`, keeping at least two points (an envelope with fewer
/// cannot be edited back into shape). Returns whether it removed anything.
pub fn remove_point(points: &mut Vec<BpfPoint>, i: usize) -> bool {
    if points.len() <= 2 || i >= points.len() {
        return false;
    }
    points.remove(i);
    true
}

/// Bends segment `i` by a vertical drag: `dy_frac` is the upward cursor motion
/// as a fraction of the body height. The segment becomes the custom-curvature
/// shape and its curve moves so the midpoint follows the drag (for a rising
/// segment negative curvature lifts the middle; for a falling one it is the
/// reverse), clamped to a visually useful range.
pub fn drag_curve(points: &mut [BpfPoint], i: usize, dy_frac: f64) {
    let from = points.get(i).map_or(0.0, |p| p.curve);
    bend_curve(points, i, dy_frac, from);
}

/// Bends segment `i` to the curvature `from` plus `dy_frac` of the field —
/// **absolute against the press**, which is what `from` is for.
///
/// The relative form (each step measured from the last, like a knob) drifts,
/// and a curve is not a knob: a knob is dragged under a locked pointer with
/// nothing on screen to stay level with, while a segment is a shape the hand is
/// pointing at. Two things went wrong with it. The clamp **eats motion** — drag
/// past the limit and the steps beyond it are swallowed, so coming back leaves
/// the bend short by however far it went — and a pointer that leaves the
/// element keeps accumulating whatever motion still arrives, so the curve is out
/// of phase with the hand from then on. Anchored at the press there is one
/// answer for a given cursor position: leave the area, come back, and the shape
/// is where the pointer says it is.
pub fn bend_curve(points: &mut [BpfPoint], i: usize, dy_frac: f64, from: f32) {
    if i + 1 >= points.len() {
        return;
    }
    let rising = points[i + 1].value >= points[i].value;
    let delta = (dy_frac * 16.0) as f32;
    let p = &mut points[i];
    p.shape = SHAPE_CURVE;
    p.curve = (from + if rising { -delta } else { delta }).clamp(-CURVE_LIMIT, CURVE_LIMIT);
}

/// The breakpoint list as the flat OSC argument tail of the edit-back event
/// and the bound forward: `t v shape curve` per point, times/values/curves as
/// floats and shapes as ints.
pub fn points_args(points: &[BpfPoint]) -> Vec<OscType> {
    let mut out = Vec::with_capacity(points.len() * 4);
    for p in points {
        out.push(OscType::Float(p.time as f32));
        out.push(OscType::Float(p.value));
        out.push(OscType::Int(p.shape));
        out.push(OscType::Float(p.curve));
    }
    out
}

/// The breakpoint list as the flat JSON array the `points` prop carries (the
/// registry mirror of a live edit).
pub fn points_json(points: &[BpfPoint]) -> Value {
    let mut out = Vec::with_capacity(points.len() * 4);
    for p in points {
        out.push(Value::from(p.time));
        out.push(Value::from(p.value));
        out.push(Value::from(p.shape));
        out.push(Value::from(p.curve));
    }
    Value::Array(out)
}

/// The envelope's vertical discontinuities: for every breakpoint time where
/// the curve jumps — a zero-width segment (coincident points), a hold
/// segment's end, a step segment's start — the `(time, lo, hi)` value span
/// the jump covers, the breakpoint values sharing that time included so a
/// disc always sits on the drawn curve. Every shape is monotone within its
/// segment, so jumps can only occur at breakpoint times.
pub fn discontinuities(points: &[BpfPoint], duration: f64) -> Vec<(f64, f32, f32)> {
    let dom = domain(points, duration);
    let eps = (dom * 1e-9).max(f64::MIN_POSITIVE);
    let mut out = Vec::new();
    let mut i = 0;
    while i < points.len() {
        let t = points[i].time;
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for v in [value_at(points, t - eps), value_at(points, t + eps)] {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        while i < points.len() && points[i].time == t {
            lo = lo.min(points[i].value);
            hi = hi.max(points[i].value);
            i += 1;
        }
        if hi - lo > f32::EPSILON {
            out.push((t, lo, hi));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts() -> Vec<BpfPoint> {
        parse_points(
            &serde_json::json!([0.0, 0.0, 1, 0.0, 0.5, 1.0, 3, 0.0, 2.0, 0.25, 1, 0.0]),
            0.0,
            1.0,
        )
        .unwrap()
    }

    #[test]
    fn parse_sorts_clamps_and_accepts_a_json_string() {
        let v = serde_json::json!([2.0, 5.0, 1, 0.0, 0.0, -1.0, 1, 0.0]);
        let points = parse_points(&v, 0.0, 1.0).unwrap();
        assert_eq!(points[0].time, 0.0, "sorted by time");
        assert_eq!(points[0].value, 0.0, "clamped into [lo, hi]");
        assert_eq!(points[1].value, 1.0);
        // The /gui_set carrier: the same array as a JSON string.
        let s = Value::String("[0.0, 0.5, 1, 0.0, 1.0, 0.75, 1, 0.0]".into());
        let points = parse_points(&s, 0.0, 1.0).unwrap();
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].value, 0.75);
        // A short trailing quad is dropped; a non-array is rejected.
        assert_eq!(
            parse_points(&serde_json::json!([0.0, 0.5, 1]), 0.0, 1.0)
                .unwrap()
                .len(),
            0
        );
        assert!(parse_points(&Value::from("nope"), 0.0, 1.0).is_none());
    }

    /// **A shape written as a float is still that shape** (found 2026-09-13, by
    /// eye: in a standalone host a bent envelope straightened on every redraw).
    /// The multitrack hands its curves their points as one `f64` run, so a bent
    /// segment arrives as `5.0`.
    #[test]
    fn a_shape_written_as_a_float_is_that_shape() {
        let v = serde_json::json!([0.0, 0.0, 5.0, 4.0, 1.0, 1.0, 1.0, 0.0]);
        let points = parse_points(&v, 0.0, 1.0).unwrap();
        assert_eq!(points[0].shape, SHAPE_CURVE, "5.0 is the curve shape");
        assert_eq!(points[0].curve, 4.0);
        assert_eq!(points[1].shape, SHAPE_LINEAR);
    }

    #[test]
    fn value_at_holds_ends_and_interpolates_by_shape() {
        let p = pts();
        assert_eq!(value_at(&p, -1.0), 0.0, "holds the first value before");
        assert_eq!(value_at(&p, 5.0), 0.25, "holds the last value after");
        // The first segment is linear 0 -> 1 over 0..0.5.
        assert!((value_at(&p, 0.25) - 0.5).abs() < 1e-6);
        // The second is a sine ease 1 -> 0.25; its midpoint is the average.
        let mid = value_at(&p, 1.25);
        assert!((mid - 0.625).abs() < 1e-4, "sine midpoint, got {mid}");
    }

    #[test]
    fn exp_scale_maps_geometrically_and_round_trips() {
        // 20..20k: the geometric midpoint is ~632.5 Hz.
        let mid = fraction_to_value(0.5, 20.0, 20_000.0, true);
        assert!((mid - 632.455).abs() < 0.01);
        let t = value_fraction(mid, 20.0, 20_000.0, true);
        assert!((t - 0.5).abs() < 1e-6);
        // A non-positive range degrades to linear rather than NaN.
        assert_eq!(fraction_to_value(0.5, -1.0, 1.0, true), 0.0);
    }

    #[test]
    fn drag_curve_moves_the_midpoint_with_the_cursor() {
        let mut p = pts(); // segment 0 rises 0 -> 1, linear
        let before = value_at(&p, 0.25);
        drag_curve(&mut p, 0, 0.3); // drag up
        assert_eq!(p[0].shape, SHAPE_CURVE);
        let after = value_at(&p, 0.25);
        assert!(
            after > before,
            "dragging up lifts a rising segment's middle"
        );
        // A falling segment mirrors: segment 1 falls 1 -> 0.25.
        let before = value_at(&p, 1.25);
        drag_curve(&mut p, 1, 0.3);
        let after = value_at(&p, 1.25);
        assert!(after > before, "dragging up lifts a falling segment too");
    }

    #[test]
    fn discontinuities_cover_coincident_points_hold_and_step() {
        use clausters_core::envshape::{SHAPE_HOLD, SHAPE_STEP};
        // A smooth envelope has none.
        assert!(discontinuities(&pts(), 0.0).is_empty());
        // Two points on the same time: the jump spans their values.
        let coincident = parse_points(
            &serde_json::json!([
                0.0, 0.0, 1, 0.0, 1.0, 0.2, 1, 0.0, 1.0, 0.9, 1, 0.0, 2.0, 1.0, 1, 0.0
            ]),
            0.0,
            1.0,
        )
        .unwrap();
        assert_eq!(discontinuities(&coincident, 0.0), vec![(1.0, 0.2, 0.9)]);
        // A hold segment jumps to its target at its end.
        let hold = vec![
            BpfPoint {
                time: 0.0,
                value: 0.0,
                shape: SHAPE_HOLD,
                curve: 0.0,
            },
            BpfPoint {
                time: 1.0,
                value: 1.0,
                shape: SHAPE_LINEAR,
                curve: 0.0,
            },
        ];
        assert_eq!(discontinuities(&hold, 0.0), vec![(1.0, 0.0, 1.0)]);
        // A step segment jumps to its target at its start — the connector
        // also ties the point's own (off-curve) disc to the drawn line.
        let step = vec![
            BpfPoint {
                time: 0.0,
                value: 0.5,
                shape: SHAPE_STEP,
                curve: 0.0,
            },
            BpfPoint {
                time: 1.0,
                value: 1.0,
                shape: SHAPE_LINEAR,
                curve: 0.0,
            },
        ];
        assert_eq!(discontinuities(&step, 0.0), vec![(0.0, 0.5, 1.0)]);
    }

    #[test]
    fn wire_forms_keep_ints_int_and_floats_float() {
        let p = pts();
        let args = points_args(&p);
        assert_eq!(args.len(), 12);
        assert_eq!(args[2], OscType::Int(1), "shape rides as an int");
        assert_eq!(args[1], OscType::Float(0.0));
        // The JSON mirror parses back to the same points.
        let back = parse_points(&points_json(&p), 0.0, 1.0).unwrap();
        assert_eq!(back, p);
    }
}
