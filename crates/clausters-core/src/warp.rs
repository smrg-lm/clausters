//! Range mapping: SuperCollider's warp family (`linlin`, `linexp`, `explin`,
//! `expexp`, `lincurve`, `curvelin`, `range`, `exprange`), and the exponential
//! endpoint rule the envelope shapes share with it.
//!
//! **Every named function is one pair, and nothing here computes a curve
//! twice.** A map reads a position out of an input range and writes it into an
//! output range, and each half comes in the same three flavours — linear,
//! exponential (equal ratios) and curved (a bend of `curve`) — so the eight
//! public names are compositions of six primitives:
//!
//! | | read | write |
//! |---|---|---|
//! | linear | [`lin_unit`] | [`lin_value`] |
//! | exponential | [`exp_unit`] | [`exp_value`] |
//! | curved | [`curve_unit`] | [`curve_value`] |
//!
//! `linlin` is read-linear/write-linear, `linexp` read-linear/write-exponential,
//! and so on; [`range`] and [`exprange`] are the two linear reads over a
//! **bipolar** input, which is the source range a bare value cannot declare for
//! itself the way a UGen's `signalRange` does.
//!
//! Following the same trio as [`crate::builtins`], each op is also (a) a
//! `#[repr(u32)]` enum so the C ABI can pass one by integer, (b) a scalar
//! [`apply_map`] and (c) a broadcasting [`map_slice`] that writes into a
//! caller-provided output — so a client maps a whole sequence in one crossing.
//!
//! # Zero has no ratio, and that rule lives here
//!
//! An exponential map is a ratio, so an endpoint at zero (or a pair straddling
//! it) has no map at all: SuperCollider's own answer is `NaN`, or an envelope
//! the author was supposed to know not to write. This crate answers instead —
//! an endpoint within [`EXP_EPSILON`] of zero becomes that epsilon with the
//! sign it had, and a sign change falls back to the linear map — and
//! [`exp_ends`] is the **one** place that rule is written. `EnvGen`'s
//! exponential segment ([`crate::envshape`]), the server's `XLine` and every
//! exponential map here read it from there; before this module they were three
//! copies of it, which is how the same curve comes to behave differently in
//! three places.
//!
//! # Precision
//!
//! Everything is `f32`, the precision the server computes in, so a value a
//! client maps off the RT path and the same map on the audio thread agree.
//! Against **sclang** the formulas are reproduced shape for shape, but sclang
//! evaluates them in `f64` and with its own left-to-right association, so the
//! agreement is a documented tolerance rather than bit equality — the same
//! standard the Faust equivalence is held to.

/// Below this magnitude an endpoint counts as zero for an exponential map.
pub const EXP_EPSILON: f32 = 1e-5;

/// An exponential endpoint: a level within [`EXP_EPSILON`] of zero becomes that
/// epsilon, keeping the sign it had — `copysign` on a zero keeps *its* sign, so
/// a ramp from `-0.0` still goes the way its target says.
#[inline]
pub fn exp_endpoint(v: f32) -> f32 {
    if v.abs() < EXP_EPSILON {
        EXP_EPSILON.copysign(v)
    } else {
        v
    }
}

/// The two endpoints an exponential map between `a` and `b` actually runs
/// between, or `None` when there is no such map — the endpoints straddle zero,
/// where a ratio does not exist and the caller falls back to a linear step.
///
/// The single source of this rule; see the module docs for why it is not
/// sclang's.
#[inline]
pub fn exp_ends(a: f32, b: f32) -> Option<(f32, f32)> {
    let (a, b) = (exp_endpoint(a), exp_endpoint(b));
    (a.signum() == b.signum()).then_some((a, b))
}

/// What an out-of-range input is trimmed to before it is mapped — sclang's
/// `prune`, whose default is [`Clip::MinMax`].
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Clip {
    /// Trim to both ends.
    #[default]
    MinMax = 0,
    /// Trim to the low end only.
    Min = 1,
    /// Trim to the high end only.
    Max = 2,
    /// Map whatever arrives, extrapolating past both ends.
    None = 3,
}

impl Clip {
    pub fn from_u32(v: u32) -> Option<Clip> {
        Some(match v {
            0 => Clip::MinMax,
            1 => Clip::Min,
            2 => Clip::Max,
            3 => Clip::None,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Clip::MinMax => "minmax",
            Clip::Min => "min",
            Clip::Max => "max",
            Clip::None => "none",
        }
    }

    pub fn from_name(name: &str) -> Option<Clip> {
        (0..).map_while(Clip::from_u32).find(|c| c.name() == name)
    }

    /// Trims `x` to `lo..hi` under this rule. The comparisons are sequential,
    /// as sclang's are, so a **reversed** range (`lo > hi`) prunes the way
    /// sclang prunes one rather than the way a sorted clamp would.
    #[inline]
    pub fn prune(self, x: f32, lo: f32, hi: f32) -> f32 {
        match self {
            Clip::None => x,
            Clip::Min => {
                if x < lo {
                    lo
                } else {
                    x
                }
            }
            Clip::Max => {
                if x > hi {
                    hi
                } else {
                    x
                }
            }
            Clip::MinMax => {
                if x < lo {
                    lo
                } else if x > hi {
                    hi
                } else {
                    x
                }
            }
        }
    }
}

// ---- the six primitives: three ways to read a position, three to write one ----

/// Where `x` sits in `lo..hi`, as a 0..1 position on a **linear** axis.
#[inline]
pub fn lin_unit(x: f32, lo: f32, hi: f32) -> f32 {
    (x - lo) / (hi - lo)
}

/// `t` written back into `lo..hi` linearly. The inverse of [`lin_unit`].
#[inline]
pub fn lin_value(t: f32, lo: f32, hi: f32) -> f32 {
    t * (hi - lo) + lo
}

/// Where `x` sits in `lo..hi` on an **exponential** axis: the position whose
/// ratio to `lo` is `x`'s, which is what makes every octave — every decade,
/// every doubling — take the same space.
#[inline]
pub fn exp_unit(x: f32, lo: f32, hi: f32) -> f32 {
    match exp_ends(lo, hi) {
        // `x` passes the same rule the ends do: an input *at* zero on a range
        // whose low end was nudged off zero has to land on that end, not on
        // `ln(0)`. Nudging both is what makes this the exact inverse of
        // `exp_value` at the endpoints.
        Some((lo, hi)) => (exp_endpoint(x) / lo).ln() / (hi / lo).ln(),
        None => lin_unit(x, lo, hi),
    }
}

/// `t` written back into `lo..hi` exponentially — `lo·(hi/lo)^t`. The inverse
/// of [`exp_unit`], and the same curve an exponential envelope segment and an
/// `XLine` run.
#[inline]
pub fn exp_value(t: f32, lo: f32, hi: f32) -> f32 {
    match exp_ends(lo, hi) {
        Some((lo, hi)) => (hi / lo).powf(t) * lo,
        None => lin_value(t, lo, hi),
    }
}

/// Where `x` sits in `lo..hi` on an axis **bent** by `curve`: 0 is linear,
/// negative builds fast then slow — most of the range spent on the first half
/// of the input, sclang's −4 default — and positive the reverse, which is the
/// fine-at-the-bottom feel a frequency or an amplitude control wants. Unlike
/// [`exp_unit`] this one spans zero and changes sign freely, which is what
/// makes it the general control curve.
#[inline]
pub fn curve_unit(x: f32, lo: f32, hi: f32, curve: f32) -> f32 {
    let Some((a, b, _)) = curve_terms(lo, hi, curve) else {
        return lin_unit(x, lo, hi);
    };
    ((b - x) / a).ln() / curve
}

/// `t` written back into `lo..hi` along the same bend. The inverse of
/// [`curve_unit`].
#[inline]
pub fn curve_value(t: f32, lo: f32, hi: f32, curve: f32) -> f32 {
    let Some((a, b, grow)) = curve_terms(lo, hi, curve) else {
        return lin_value(t, lo, hi);
    };
    b - a * grow.powf(t)
}

/// The two coefficients a bend is written with, shared by both directions so
/// the curve and its inverse cannot drift apart. `None` where the bend is flat
/// enough to be the linear map — sclang's own 0.001 threshold, which is what
/// keeps `curve = 0` from dividing by zero.
#[inline]
fn curve_terms(lo: f32, hi: f32, curve: f32) -> Option<(f32, f32, f32)> {
    if curve.abs() < 0.001 {
        return None;
    }
    let grow = curve.exp();
    let a = (hi - lo) / (1.0 - grow);
    Some((a, lo + a, grow))
}

// ---- the eight named maps, each one pair of the primitives above ----

/// Linear in, linear out.
#[inline]
pub fn linlin(x: f32, in_lo: f32, in_hi: f32, out_lo: f32, out_hi: f32, clip: Clip) -> f32 {
    lin_value(
        lin_unit(clip.prune(x, in_lo, in_hi), in_lo, in_hi),
        out_lo,
        out_hi,
    )
}

/// Linear in, exponential out — a fader position to a frequency.
#[inline]
pub fn linexp(x: f32, in_lo: f32, in_hi: f32, out_lo: f32, out_hi: f32, clip: Clip) -> f32 {
    exp_value(
        lin_unit(clip.prune(x, in_lo, in_hi), in_lo, in_hi),
        out_lo,
        out_hi,
    )
}

/// Exponential in, linear out — a frequency to a fader position.
#[inline]
pub fn explin(x: f32, in_lo: f32, in_hi: f32, out_lo: f32, out_hi: f32, clip: Clip) -> f32 {
    lin_value(
        exp_unit(clip.prune(x, in_lo, in_hi), in_lo, in_hi),
        out_lo,
        out_hi,
    )
}

/// Exponential in, exponential out.
#[inline]
pub fn expexp(x: f32, in_lo: f32, in_hi: f32, out_lo: f32, out_hi: f32, clip: Clip) -> f32 {
    exp_value(
        exp_unit(clip.prune(x, in_lo, in_hi), in_lo, in_hi),
        out_lo,
        out_hi,
    )
}

/// Linear in, **bent** out by `curve`.
#[inline]
pub fn lincurve(
    x: f32,
    in_lo: f32,
    in_hi: f32,
    out_lo: f32,
    out_hi: f32,
    curve: f32,
    clip: Clip,
) -> f32 {
    curve_value(
        lin_unit(clip.prune(x, in_lo, in_hi), in_lo, in_hi),
        out_lo,
        out_hi,
        curve,
    )
}

/// **Bent** in by `curve`, linear out. The inverse of [`lincurve`].
#[inline]
pub fn curvelin(
    x: f32,
    in_lo: f32,
    in_hi: f32,
    out_lo: f32,
    out_hi: f32,
    curve: f32,
    clip: Clip,
) -> f32 {
    lin_value(
        curve_unit(clip.prune(x, in_lo, in_hi), in_lo, in_hi, curve),
        out_lo,
        out_hi,
    )
}

/// A **bipolar** value (−1..1) into `lo..hi`, linearly — sclang's `range`.
///
/// Nothing is pruned, because nothing declares the input bipolar: a UGen knows
/// its own `signalRange` and a bare number does not, so a value that overshoots
/// −1..1 overshoots the output range by the same proportion rather than being
/// silently trimmed to an assumption.
#[inline]
pub fn range(x: f32, lo: f32, hi: f32) -> f32 {
    linlin(x, -1.0, 1.0, lo, hi, Clip::None)
}

/// A **bipolar** value (−1..1) into `lo..hi`, exponentially — sclang's
/// `exprange`. Unpruned for the reason [`range`] gives.
#[inline]
pub fn exprange(x: f32, lo: f32, hi: f32) -> f32 {
    linexp(x, -1.0, 1.0, lo, hi, Clip::None)
}

// ---- the op table: one map by integer, over a scalar or a whole sequence ----

/// The range maps as a C-ABI operator. The discriminants are the stable
/// contract: append only, never renumber.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapOp {
    Linlin = 0,
    Linexp = 1,
    Explin = 2,
    Expexp = 3,
    Lincurve = 4,
    Curvelin = 5,
    /// `range`: the input bounds are ignored, the map is −1..1 unpruned.
    Range = 6,
    /// `exprange`: as [`MapOp::Range`], exponentially.
    Exprange = 7,
}

impl MapOp {
    pub fn from_u32(v: u32) -> Option<MapOp> {
        Some(match v {
            0 => MapOp::Linlin,
            1 => MapOp::Linexp,
            2 => MapOp::Explin,
            3 => MapOp::Expexp,
            4 => MapOp::Lincurve,
            5 => MapOp::Curvelin,
            6 => MapOp::Range,
            7 => MapOp::Exprange,
            _ => return None,
        })
    }

    /// The name every client spells this map with — SuperCollider's own, all
    /// lowercase, as the whole operator vocabulary is.
    pub fn name(self) -> &'static str {
        match self {
            MapOp::Linlin => "linlin",
            MapOp::Linexp => "linexp",
            MapOp::Explin => "explin",
            MapOp::Expexp => "expexp",
            MapOp::Lincurve => "lincurve",
            MapOp::Curvelin => "curvelin",
            MapOp::Range => "range",
            MapOp::Exprange => "exprange",
        }
    }

    /// Resolves a name to the map (the inverse of [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<MapOp> {
        (0..)
            .map_while(MapOp::from_u32)
            .find(|op| op.name() == name)
    }

    /// Whether this map reads `curve` — only the bent pair does.
    pub fn takes_curve(self) -> bool {
        matches!(self, MapOp::Lincurve | MapOp::Curvelin)
    }
}

/// One map by op. `curve` is read only by the bent pair and `in_lo`/`in_hi`
/// only by the six that have an input range; the rest ignore them, so one
/// signature serves the table.
// One op plus the six numbers a map is written with: the table's shape, the
// way `apply_binary(op, a, b)` is the binary table's.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn apply_map(
    op: MapOp,
    x: f32,
    in_lo: f32,
    in_hi: f32,
    out_lo: f32,
    out_hi: f32,
    curve: f32,
    clip: Clip,
) -> f32 {
    match op {
        MapOp::Linlin => linlin(x, in_lo, in_hi, out_lo, out_hi, clip),
        MapOp::Linexp => linexp(x, in_lo, in_hi, out_lo, out_hi, clip),
        MapOp::Explin => explin(x, in_lo, in_hi, out_lo, out_hi, clip),
        MapOp::Expexp => expexp(x, in_lo, in_hi, out_lo, out_hi, clip),
        MapOp::Lincurve => lincurve(x, in_lo, in_hi, out_lo, out_hi, curve, clip),
        MapOp::Curvelin => curvelin(x, in_lo, in_hi, out_lo, out_hi, curve, clip),
        MapOp::Range => range(x, out_lo, out_hi),
        MapOp::Exprange => exprange(x, out_lo, out_hi),
    }
}

/// The same map over a whole sequence, into a caller-provided output — the
/// shape a client maps an array with, and the one the C ABI and the wasm face
/// both call. `input` broadcasts when it holds a single value, exactly as
/// [`crate::builtins::at`] does; no allocation, so the audio thread may call it.
#[allow(clippy::too_many_arguments)]
pub fn map_slice(
    op: MapOp,
    input: &[f32],
    out: &mut [f32],
    in_lo: f32,
    in_hi: f32,
    out_lo: f32,
    out_hi: f32,
    curve: f32,
    clip: Clip,
) {
    for (i, o) in out.iter_mut().enumerate() {
        *o = apply_map(
            op,
            crate::builtins::at(input, i),
            in_lo,
            in_hi,
            out_lo,
            out_hi,
            curve,
            clip,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference values are SuperCollider's, computed in sclang and
    /// rounded — the family exists to agree with it, so the agreement is
    /// asserted rather than described.
    #[test]
    fn the_four_range_maps_agree_with_sclang() {
        let c = Clip::MinMax;
        // 0.5.linlin(0, 1, 20, 20000) == 10010
        assert!((linlin(0.5, 0.0, 1.0, 20.0, 20000.0, c) - 10010.0).abs() < 1e-2);
        // 0.5.linexp(0, 1, 20, 20000) == 632.4555
        assert!((linexp(0.5, 0.0, 1.0, 20.0, 20000.0, c) - 632.4555).abs() < 1e-2);
        // 632.4555.explin(20, 20000, 0, 1) == 0.5
        assert!((explin(632.4555, 20.0, 20000.0, 0.0, 1.0, c) - 0.5).abs() < 1e-5);
        // 632.4555.expexp(20, 20000, 1, 100) == 10
        assert!((expexp(632.4555, 20.0, 20000.0, 1.0, 100.0, c) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn the_bent_pair_agrees_with_sclang_and_inverts() {
        // 0.5.lincurve(0, 1, 0, 1, -4) == 0.8807971
        let y = lincurve(0.5, 0.0, 1.0, 0.0, 1.0, -4.0, Clip::MinMax);
        assert!((y - 0.8807971).abs() < 1e-5, "{y}");
        // and the inverse takes it back
        let x = curvelin(y, 0.0, 1.0, 0.0, 1.0, -4.0, Clip::MinMax);
        assert!((x - 0.5).abs() < 1e-4, "{x}");
    }

    #[test]
    fn a_flat_curve_is_the_linear_map() {
        for &curve in &[0.0, 0.0009, -0.0009] {
            assert_eq!(
                lincurve(0.3, 0.0, 1.0, 10.0, 20.0, curve, Clip::MinMax),
                linlin(0.3, 0.0, 1.0, 10.0, 20.0, Clip::MinMax)
            );
        }
    }

    #[test]
    fn every_map_reads_the_position_its_inverse_writes() {
        for &t in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            assert!((lin_unit(lin_value(t, -3.0, 7.0), -3.0, 7.0) - t).abs() < 1e-5);
            assert!((exp_unit(exp_value(t, 20.0, 20000.0), 20.0, 20000.0) - t).abs() < 1e-5);
            assert!((curve_unit(curve_value(t, 0.0, 1.0, -4.0), 0.0, 1.0, -4.0) - t).abs() < 1e-4);
        }
    }

    #[test]
    fn the_default_clip_trims_the_input_and_none_extrapolates() {
        assert_eq!(linlin(2.0, 0.0, 1.0, 0.0, 10.0, Clip::MinMax), 10.0);
        assert_eq!(linlin(-1.0, 0.0, 1.0, 0.0, 10.0, Clip::MinMax), 0.0);
        assert_eq!(linlin(2.0, 0.0, 1.0, 0.0, 10.0, Clip::None), 20.0);
        assert_eq!(linlin(2.0, 0.0, 1.0, 0.0, 10.0, Clip::Max), 10.0);
        assert_eq!(linlin(-1.0, 0.0, 1.0, 0.0, 10.0, Clip::Max), -10.0);
    }

    #[test]
    fn a_bipolar_value_spans_the_range_and_is_not_trimmed() {
        assert_eq!(range(-1.0, 100.0, 200.0), 100.0);
        assert_eq!(range(0.0, 100.0, 200.0), 150.0);
        assert_eq!(range(1.0, 100.0, 200.0), 200.0);
        // unpruned: nothing declares a bare value bipolar
        assert_eq!(range(2.0, 100.0, 200.0), 250.0);
        assert!((exprange(0.0, 1.0, 100.0) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn zero_has_no_ratio_so_an_endpoint_is_nudged_and_a_sign_change_is_linear() {
        // Where sclang gives NaN, this gives a very steep rise.
        assert!(exp_value(0.5, 0.0, 1.0).is_finite());
        assert_eq!(exp_endpoint(0.0), EXP_EPSILON);
        assert_eq!(exp_endpoint(-0.0), -EXP_EPSILON);
        assert_eq!(exp_ends(-1.0, 1.0), None, "a sign change has no ratio");
        // The input side takes the rule too: at the low end, not at `ln(0)`.
        assert_eq!(explin(0.0, 0.0, 1.0, 0.0, 1.0, Clip::MinMax), 0.0);
        assert!(expexp(0.0, 0.0, 1.0, 1.0, 100.0, Clip::MinMax).is_finite());
        // and the fallback is exactly the linear map
        assert_eq!(exp_value(0.25, -1.0, 1.0), lin_value(0.25, -1.0, 1.0));
    }

    #[test]
    fn the_envelope_shapes_read_the_same_curve_this_module_writes() {
        // The unification this module exists for: an exponential segment and a
        // curved one are `exp_value`/`curve_value` between two levels, so a
        // client drawing an envelope and the server playing it cannot diverge.
        for &t in &[0.0, 0.3, 0.7, 0.99] {
            assert_eq!(
                crate::envshape::shape_value(crate::envshape::SHAPE_EXPONENTIAL, 0.0, 0.2, 3.0, t),
                exp_value(t, 0.2, 3.0)
            );
            assert_eq!(
                crate::envshape::shape_value(crate::envshape::SHAPE_CURVE, -4.0, 0.0, 1.0, t),
                curve_value(t, 0.0, 1.0, -4.0)
            );
        }
    }

    #[test]
    fn a_sequence_is_mapped_in_one_crossing_and_a_single_value_broadcasts() {
        let mut out = [0.0f32; 3];
        map_slice(
            MapOp::Linlin,
            &[0.0, 0.5, 1.0],
            &mut out,
            0.0,
            1.0,
            0.0,
            10.0,
            0.0,
            Clip::MinMax,
        );
        assert_eq!(out, [0.0, 5.0, 10.0]);

        let mut out = [0.0f32; 3];
        map_slice(
            MapOp::Linlin,
            &[0.5],
            &mut out,
            0.0,
            1.0,
            0.0,
            10.0,
            0.0,
            Clip::MinMax,
        );
        assert_eq!(out, [5.0, 5.0, 5.0]);
    }

    #[test]
    fn every_op_and_clip_round_trips_its_name_and_number() {
        for op in (0..).map_while(MapOp::from_u32) {
            assert_eq!(MapOp::from_name(op.name()), Some(op));
            assert_eq!(MapOp::from_u32(op as u32), Some(op));
        }
        for clip in (0..).map_while(Clip::from_u32) {
            assert_eq!(Clip::from_name(clip.name()), Some(clip));
        }
        assert_eq!(MapOp::from_name("nope"), None);
    }
}
