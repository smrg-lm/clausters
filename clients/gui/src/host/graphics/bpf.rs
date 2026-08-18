//! The `bpf` widget: a drawable break-point function (envelope editor).
//!
//! The model is a sorted breakpoint list `(time, value)` plus a per-segment
//! **shape** using the server's own envelope shape numbers — the segment
//! leaving point `i` interpolates to point `i + 1` through
//! [`clausters_core::envshape::shape_value`], the very function the server's
//! `EnvGen` plays, so what the editor draws is exactly what the server plays.
//!
//! The model is deliberately more general than an amplitude envelope, so the
//! same widget later serves automation lanes: values live in an arbitrary
//! `[min, max]` range (unipolar, bipolar, or any parameter span), an on/off
//! lane is the **hold** shape over `{0, 1}` (each point's value held until the
//! next; SC's *step* instead jumps to the target at segment start, so a step
//! segment draws — and plays — the *next* point's value), every standard
//! transition curve is a shape/curve pair, and frequency-like parameters get
//! an exponential display scale (`exp`, requiring a positive range). Times are in the
//! envelope's own units (seconds for an `EnvGen`) over a `[0, duration]`
//! domain that defaults to the last breakpoint's time.
//!
//! Everything here is pure display/model logic (parse, evaluate-per-column,
//! hit-test, edit ops, the flat wire form) shared by both fronts and
//! unit-tested without a window; only the shape evaluation lives in the core
//! (the placement rule). The wire form — props, `/gui_set` and the edit-back
//! event alike — is the flat quad list `t0 v0 shape0 curve0 t1 v1 …` (the last
//! point's shape/curve are carried but unused), keeping ints int and floats
//! float.

use clausters_core::envshape::{SHAPE_CURVE, SHAPE_LINEAR, shape_value};
use clausters_core::osc::OscType;
use serde_json::Value;

use crate::host::graphics::shape;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::viewport::View;

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
        .chunks_exact(4)
        .filter_map(|q| {
            Some(BpfPoint {
                time: q[0].as_f64()?.max(0.0),
                value: (q[1].as_f64()? as f32).clamp(lo, hi),
                shape: q[2].as_i64().unwrap_or(SHAPE_LINEAR as i64) as i32,
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
        super::meters::fraction(value, lo, hi)
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

/// The **display mapping** a break-point curve is drawn and grabbed through:
/// the field it occupies, the slice of its time domain that field spans, and
/// the value axis over it.
///
/// One type, because the curve is drawn in two places and has to behave
/// identically in both: standing on its own, where the field spans the whole
/// domain ([`spanning`](Axes::spanning)), and as a container's **body**, where
/// the container hands it the visible window of its axis and the span behind
/// it. Every hit-test and every pixel-mapped edit goes through it, so a
/// breakpoint is grabbed by the pixels it was drawn on wherever it is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axes {
    /// The field the curve is drawn in.
    pub body: Rect,
    /// The visible slice of the domain that field spans.
    pub view: View,
    /// The full time domain, whatever part of it is on screen.
    pub dom: f64,
    /// The value range, and whether it is read geometrically.
    pub lo: f32,
    pub hi: f32,
    pub exp: bool,
}

impl Axes {
    /// The whole domain across `body` — a curve standing on its own.
    pub fn spanning(body: Rect, dom: f64, lo: f32, hi: f32, exp: bool) -> Self {
        Self {
            body,
            view: View {
                start: 0.0,
                len: dom.max(1e-9),
            },
            dom,
            lo,
            hi,
            exp,
        }
    }

    /// The x pixel time `t` falls on.
    pub fn x(&self, t: f64) -> f32 {
        (self.body.x as f64 + (t - self.view.start) / self.view.len.max(1e-9) * self.body.w as f64)
            as f32
    }

    /// The time x pixel `x` falls on — the inverse of [`Self::x`].
    pub fn t(&self, x: f64) -> f64 {
        self.view.start + self.view.len * (x - self.body.x as f64) / self.body.w.max(1.0) as f64
    }

    /// The y pixel `value` falls on.
    pub fn y(&self, value: f32) -> f32 {
        self.body.y + self.body.h * (1.0 - value_fraction(value, self.lo, self.hi, self.exp))
    }

    /// The value y pixel `y` falls on, clamped into the field — the inverse of
    /// [`Self::y`].
    pub fn value(&self, y: f64) -> f32 {
        let frac = 1.0 - ((y - self.body.y as f64) / self.body.h.max(1.0) as f64).clamp(0.0, 1.0);
        fraction_to_value(frac as f32, self.lo, self.hi, self.exp)
    }

    /// The index of the breakpoint under `(cx, cy)`, within a device-pixel
    /// radius (points win over segments; the nearest wins among overlaps).
    pub fn hit_point(&self, points: &[BpfPoint], cx: f64, cy: f64, m: &Metrics) -> Option<usize> {
        // The grab radius: the drawn point plus its slop, so a small target
        // stays clickable.
        let radius = (m.point_radius + m.hit_slop).max(6.0) as f64;
        // Squared throughout: the distance is only ever compared — against the
        // radius, and against the best so far — and both comparisons order the
        // same squared (see `shape`).
        let r2 = radius * radius;
        let mut best: Option<(usize, f64)> = None;
        for (i, p) in points.iter().enumerate() {
            let d = shape::dist2(cx, cy, self.x(p.time) as f64, self.y(p.value) as f64);
            if d <= r2 && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Whether `(cx, cy)` is **on the drawn line** — within the grab slop of
    /// the curve's own y at that x, rather than anywhere in its column.
    ///
    /// The distinction is what makes a curve one **layer** among several over
    /// the same rectangle: [`hit_segment`](Self::hit_segment) answers the
    /// column, which is right for a bend already in hand (a vertical drag has
    /// no y to be near), and wrong for deciding whether the hand is pointing at
    /// this curve at all — a column claim would leave the container underneath
    /// no pixels of its own anywhere along the curve.
    pub fn on_line(&self, points: &[BpfPoint], cx: f64, cy: f64, m: &Metrics) -> bool {
        if points.is_empty() {
            return false;
        }
        let slop = (m.point_radius + m.hit_slop).max(6.0) as f64;
        let y = self.y(value_at(points, self.t(cx))) as f64;
        (cy - y).abs() <= slop
    }

    /// The segment under x pixel `cx`: the index of the point it leaves from,
    /// when the cursor sits strictly between two breakpoints.
    pub fn hit_segment(&self, points: &[BpfPoint], cx: f64) -> Option<usize> {
        let t = self.t(cx);
        points
            .windows(2)
            .position(|pair| t >= pair[0].time && t < pair[1].time)
    }

    /// Moves breakpoint `i` to the cursor: the time clamped monotonic between
    /// its neighbors (and into the domain), the value clamped into the range.
    pub fn move_point(&self, points: &mut [BpfPoint], i: usize, cx: f64, cy: f64) {
        if i >= points.len() {
            return;
        }
        place_point(points, i, self.t(cx), self.value(cy), self.dom);
    }

    /// Inserts a breakpoint at the cursor, returning its index (see
    /// [`insert_point`]).
    pub fn add_point(&self, points: &mut Vec<BpfPoint>, cx: f64, cy: f64) -> usize {
        insert_point(points, self.t(cx).clamp(0.0, self.dom), self.value(cy))
    }
}

/// Places breakpoint `i` at time `t` and `value` — the mapping-free core of a
/// point drag: the time stays monotonic (clamped between its neighbours, and
/// into `[0, dom]`), the value is taken as given (the caller mapped it out of
/// its own display range). The pixel-mapped [`Axes::move_point`] edits through
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
/// mapping-free core of [`Axes::add_point`].
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
    if i + 1 >= points.len() {
        return;
    }
    let rising = points[i + 1].value >= points[i].value;
    let delta = (dy_frac * 16.0) as f32;
    let p = &mut points[i];
    p.shape = SHAPE_CURVE;
    p.curve = (p.curve + if rising { -delta } else { delta }).clamp(-CURVE_LIMIT, CURVE_LIMIT);
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

/// Draws the curve into `ax`: evaluated **once per pixel column** through the
/// shared shape math (never finer than the screen), an exact vertical connector
/// at every discontinuity (the per-column polyline alone would render a jump as
/// a one-pixel slant — or hide it entirely when two points share a time), and a
/// disc per breakpoint.
///
/// Only the curve: the field it sits in and whatever names it are the *view's*,
/// and a container's body has neither.
pub fn draw(d: &mut Draw, ax: &Axes, points: &[BpfPoint]) {
    draw_with(d, ax, points, None)
}

/// The same, with one **segment lit**: the part a vertical drag would bend.
///
/// A curve is one trace of one weight end to end, so nothing distinguished *a
/// place you can bend* from a place a Ctrl-click adds a point to, or from the
/// container underneath — the gesture worked and the picture said nothing. The
/// vocabulary is the clip grip's, applied here: the affordance belongs to what
/// is **held** rather than to where the pointer is, so a bend in flight keeps
/// its segment lit even when the pointer has drifted off it.
pub fn draw_with(d: &mut Draw, ax: &Axes, points: &[BpfPoint], lit: Option<usize>) {
    let (mesh, m, theme) = d.parts();
    if ax.body.w < 1.0 || ax.body.h <= 0.0 || points.is_empty() {
        return;
    }
    let span = lit.and_then(|i| Some((points.get(i)?.time, points.get(i + 1)?.time)));
    let columns = ax.body.w.max(1.0) as usize;
    let mut prev = [ax.body.x, ax.y(value_at(points, ax.t(ax.body.x as f64)))];
    for c in 1..=columns {
        let x = ax.body.x + c as f32;
        let p = [x, ax.y(value_at(points, ax.t(x as f64)))];
        // The lit segment is the accent at the weight a handle has, so it reads
        // as *grabbable* rather than as a second signal drawn over the first.
        let inside = span.is_some_and(|(a, b)| {
            let t = ax.t(x as f64);
            t >= a && t < b
        });
        let (w, color) = if inside {
            (m.trace_w + m.divider_w, theme.accent)
        } else {
            (m.trace_w, theme.trace)
        };
        mesh.line(prev, p, w, color);
        prev = p;
    }
    for (t, v_lo, v_hi) in discontinuities(points, ax.dom) {
        let x = ax.x(t);
        if x < ax.body.x || x > ax.body.x + ax.body.w {
            continue;
        }
        let (y0, y1) = (ax.y(v_hi), ax.y(v_lo));
        mesh.rect(
            Rect::new(x - m.trace_w * 0.5, y0, m.trace_w, (y1 - y0).max(1.0)),
            theme.trace,
        );
    }
    for p in points {
        let x = ax.x(p.time);
        if x >= ax.body.x && x <= ax.body.x + ax.body.w {
            mesh.disc(x, ax.y(p.value), m.point_radius, theme.point);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

    fn pts() -> Vec<BpfPoint> {
        parse_points(
            &serde_json::json!([0.0, 0.0, 1, 0.0, 0.5, 1.0, 3, 0.0, 2.0, 0.25, 1, 0.0]),
            0.0,
            1.0,
        )
        .unwrap()
    }

    /// The default mapping the tests below read through: a 100x100 field
    /// spanning the points' own domain (0..2), so t=0.5 lands at x=25.
    fn axes100() -> Axes {
        Axes::spanning(
            Rect::new(0.0, 0.0, 100.0, 100.0),
            domain(&pts(), 0.0),
            0.0,
            1.0,
            false,
        )
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
    fn hit_prefers_points_then_finds_the_segment_under_x() {
        let (p, ax) = (pts(), axes100());
        // Point 1 sits at t=0.5 of a 0..2 domain -> x=25, value 1.0 -> y=0.
        assert_eq!(ax.hit_point(&p, 26.0, 2.0, &Metrics::default()), Some(1));
        assert_eq!(ax.hit_point(&p, 60.0, 50.0, &Metrics::default()), None);
        assert_eq!(ax.hit_segment(&p, 10.0), Some(0));
        assert_eq!(ax.hit_segment(&p, 60.0), Some(1));
    }

    /// The same points read through a **window** of the domain rather than all
    /// of it — a container's body — land on the same breakpoints, which is the
    /// property that lets one element be the view and the body both.
    #[test]
    fn a_windowed_axis_hits_the_same_points_where_they_are_drawn() {
        let p = pts();
        // The second half of the domain across the same field: t=1.0 is now the
        // left edge and t=2.0 the right, so the last point is at x=100.
        let ax = Axes {
            body: Rect::new(0.0, 0.0, 100.0, 100.0),
            view: View {
                start: 1.0,
                len: 1.0,
            },
            dom: 2.0,
            lo: 0.0,
            hi: 1.0,
            exp: false,
        };
        assert_eq!(ax.x(2.0), 100.0);
        assert_eq!(
            ax.hit_point(&p, 100.0, ax.y(0.25) as f64, &Metrics::default()),
            Some(2)
        );
        // ...and the point that scrolled off the left is not under the cursor.
        assert_eq!(ax.hit_point(&p, 0.0, 0.0, &Metrics::default()), None);
    }

    #[test]
    fn move_clamps_monotonic_between_neighbors() {
        let (mut p, ax) = (pts(), axes100());
        // Dragging point 1 (t=0.5) past point 2 (t=2.0) clamps to it, and the
        // value tracks the cursor height.
        ax.move_point(&mut p, 1, 150.0, 100.0);
        assert_eq!(p[1].time, 2.0);
        assert_eq!(p[1].value, 0.0);
        // Dragging point 0 left of the domain clamps to 0.
        ax.move_point(&mut p, 0, -20.0, 0.0);
        assert_eq!(p[0].time, 0.0);
        assert_eq!(p[0].value, 1.0, "top of the body is the range top");
    }

    #[test]
    fn add_inherits_the_split_segment_and_remove_keeps_two() {
        let (mut p, ax) = (pts(), axes100());
        // Split the sine segment (leaving point 1): the new point inherits it.
        let i = ax.add_point(&mut p, 60.0, 50.0);
        assert_eq!(i, 2);
        assert_eq!(p.len(), 4);
        assert_eq!(p[2].shape, 3, "inherits the sine shape");
        assert!((p[2].time - 1.2).abs() < 1e-9);
        assert!(remove_point(&mut p, 2));
        assert_eq!(p.len(), 3);
        // Never below two points.
        let mut two = default_points(0.0);
        assert!(!remove_point(&mut two, 0));
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

    #[test]
    fn draw_emits_geometry_per_column_and_per_point() {
        let mut mesh = Mesh::new();
        draw(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            &axes100(),
            &pts(),
        );
        assert!(!mesh.is_empty());
    }
}
