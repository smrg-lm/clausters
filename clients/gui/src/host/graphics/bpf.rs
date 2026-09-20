//! The `bpf` widget: a drawable break-point function (envelope editor).
//!
//! The model is a sorted breakpoint list `(time, value)` plus a per-segment
//! **shape** using the server's own envelope shape numbers -- the segment
//! leaving point `i` interpolates to point `i + 1` through
//! [`clausters_core::envshape::shape_value`], the very function the server's
//! `EnvGen` plays, so what the editor draws is exactly what the server plays.
//!
//! The model is deliberately more general than an amplitude envelope, so the
//! same widget later serves automation lanes: values live in an arbitrary
//! `[min, max]` range (unipolar, bipolar, or any parameter span), an on/off
//! lane is the **hold** shape over `{0, 1}` (each point's value held until the
//! next; SC's *step* instead jumps to the target at segment start, so a step
//! segment draws -- and plays -- the *next* point's value), every standard
//! transition curve is a shape/curve pair, and frequency-like parameters get
//! an exponential display scale (`exp`, requiring a positive range). Times are in the
//! envelope's own units (seconds for an `EnvGen`) over a `[0, duration]`
//! domain that defaults to the last breakpoint's time.
//!
//! Everything here is pure display/model logic (parse, evaluate-per-column,
//! hit-test, edit ops, the flat wire form) shared by both fronts and
//! unit-tested without a window; only the shape evaluation lives in the core
//! (the placement rule). The wire form -- props, `/gui_set` and the edit-back
//! event alike -- is the flat quad list `t0 v0 shape0 curve0 t1 v1 ...` (the last
//! point's shape/curve are carried but unused), keeping ints int and floats
//! float.

use crate::host::graphics::shape;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::structures::points::{
    BpfPoint, discontinuities, fraction_to_value, insert_point, place_point, value_at,
    value_fraction,
};
use crate::viewport::View;

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
    /// The whole domain across `body` -- a curve standing on its own.
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

    /// The time x pixel `x` falls on -- the inverse of [`Self::x`].
    pub fn t(&self, x: f64) -> f64 {
        self.view.start + self.view.len * (x - self.body.x as f64) / self.body.w.max(1.0) as f64
    }

    /// The y pixel `value` falls on.
    pub fn y(&self, value: f32) -> f32 {
        self.body.y + self.body.h * (1.0 - value_fraction(value, self.lo, self.hi, self.exp))
    }

    /// The value y pixel `y` falls on, clamped into the field -- the inverse of
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
        // Squared throughout: the distance is only ever compared -- against the
        // radius, and against the best so far -- and both comparisons order the
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

    /// Whether `(cx, cy)` is **on the drawn line** -- within the grab slop of
    /// the curve's own y at that x, rather than anywhere in its column.
    ///
    /// The distinction is what makes a curve one **layer** among several over
    /// the same rectangle: [`hit_segment`](Self::hit_segment) answers the
    /// column, which is right for a bend already in hand (a vertical drag has
    /// no y to be near), and wrong for deciding whether the hand is pointing at
    /// this curve at all -- a column claim would leave the container underneath
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

/// Draws the curve into `ax`: evaluated **once per pixel column** through the
/// shared shape math (never finer than the screen), an exact vertical connector
/// at every discontinuity (the per-column polyline alone would render a jump as
/// a one-pixel slant -- or hide it entirely when two points share a time), and a
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
/// container underneath -- the gesture worked and the picture said nothing. The
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
    use crate::host::structures::points::*;

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
    fn hit_prefers_points_then_finds_the_segment_under_x() {
        let (p, ax) = (pts(), axes100());
        // Point 1 sits at t=0.5 of a 0..2 domain -> x=25, value 1.0 -> y=0.
        assert_eq!(ax.hit_point(&p, 26.0, 2.0, &Metrics::default()), Some(1));
        assert_eq!(ax.hit_point(&p, 60.0, 50.0, &Metrics::default()), None);
        assert_eq!(ax.hit_segment(&p, 10.0), Some(0));
        assert_eq!(ax.hit_segment(&p, 60.0), Some(1));
    }

    /// The same points read through a **window** of the domain rather than all
    /// of it -- a container's body -- land on the same breakpoints, which is the
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
    fn draw_emits_geometry_per_column_and_per_point() {
        let mut mesh = Mesh::new();
        draw(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            &axes100(),
            &pts(),
        );
        assert!(!mesh.is_empty());
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
}
