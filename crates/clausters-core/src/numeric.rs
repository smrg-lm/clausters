//! The numeric functions sclang has on a number that are not operators: no
//! UGen computes them, so they are not in the op tables and are not `f32`.
//! They are sclang's language-side functions -- computed in `f64`, as sclang
//! computes them -- written once here so both clients answer the same.
//!
//! Two tables, each one door across the C ABI and one across wasm: [`Calc`],
//! functions of a few numbers, and [`ScaleCalc`], the ones that read a scale or
//! a sorted list. The special functions sclang takes from a C++ library
//! (`erf`, `tgamma`, the Bessel functions) come from the `libm` crate here,
//! never by hand.

use crate::event::split_degree;

/// `ln(0.001)`: the decay a feedback coefficient reaches -60 dB at.
const LOG001: f64 = -6.907_755_278_982_137;

/// The functions of a few numbers. Each reads its arguments in order (see
/// [`Calc::arity`]); the discriminants are the stable C-ABI contract: append
/// only.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Calc {
    /// Degrees to radians.
    Degrad = 0,
    /// Radians to degrees.
    Raddeg = 1,
    /// Real cube root.
    Cbrt = 2,
    /// `ln(1 + x)`, exact near zero.
    Log1p = 3,
    /// `exp(x) - 1`, exact near zero.
    Expm1 = 4,
    /// The error function.
    Erf = 5,
    /// The complementary error function, `1 - erf(x)`, exact in its tail.
    Erfc = 6,
    /// The gamma function.
    Tgamma = 7,
    /// `ln |gamma(x)|`.
    Lgamma = 8,
    /// `x`, or `0` where it is a denormal, an infinity, a NaN or past 1e15:
    /// what keeps a feedback path from carrying one.
    Zapgremlins = 9,
    /// `x` wrapped into `[-1, 1)` by one shift.
    Wrap1 = 10,
    /// `x` folded into `[-1, 1]` by one reflection.
    Fold1 = 11,
    /// `1` where the whole part is even, else `0`.
    Even = 12,
    /// `1` where the whole part is odd, else `0`.
    Odd = 13,
    /// The Gray code of the whole part, `x ^ (x >> 1)`.
    Graycode = 14,
    /// The least power of `base` at or above `x` -- `(x, base)`.
    Nextpowerof = 15,
    /// The greatest power of `base` below the next one -- `(x, base)`.
    Previouspowerof = 16,
    /// The distance from `x` to `b` on a circle of circumference `mod` --
    /// `(x, b, mod)`.
    Moddif = 17,
    /// `x` pulled toward its nearest multiple of `resolution` by `strength`
    /// when it is **farther** than `margin` from it -- `(x, resolution,
    /// margin, strength)`.
    Softround = 18,
    /// `x` pulled toward its nearest multiple of `resolution` by `strength`
    /// when it is **within** `margin` of it -- `(x, resolution, margin,
    /// strength)`.
    Snap = 19,
    /// The logistic curve `a * (m*e^-x + 1) / (n*e^-x + 1)` with sclang's
    /// `tau` -- `(x, a, m, n, tau)`.
    Lcurve = 20,
    /// The Gaussian bell `a * exp(-(x - b)^2 / (2 c^2))` -- `(x, a, b, c)`.
    Gausscurve = 21,
    /// Lagrange cubic interpolation at `x1` in `0..1` between `b` and `c`,
    /// with `a` and `d` their outer neighbours -- `(x1, a, b, c, d)`.
    Lg3interp = 22,
    /// The feedback coefficient that decays a delay of `delaytime` by 60 dB
    /// in `decaytime` (negative for an inverted feedback) -- `(delaytime,
    /// decaytime)`.
    Calcfeedback = 23,
    /// A linear map with a centre: `x` read off `in_lo..in_center..in_hi`,
    /// each half mapped linearly onto its half of the output --
    /// `(x, in_center, in_lo, in_hi, out_center, out_lo, out_hi, clip)`, with
    /// `clip` a `warp::Clip` number.
    Bilin = 24,
    /// [`Calc::Bilin`] read off an exponential input.
    Biexp = 25,
    /// The Bessel function of the first kind of integer order `n` --
    /// `(n, x)`.
    Besselj = 26,
    /// The Bessel function of the second kind of integer order `n` --
    /// `(n, x)`.
    Bessely = 27,
    /// `x + frac * (y - x)`: `frac` of the way from `x` to `y` -- `(x, y,
    /// frac)`. Over two lists, the two blended point by point.
    Blend = 28,
}

impl Calc {
    /// Maps a C-ABI discriminant back to the enum.
    pub fn from_u32(v: u32) -> Option<Calc> {
        use Calc::*;
        const ALL: [Calc; 29] = [
            Degrad,
            Raddeg,
            Cbrt,
            Log1p,
            Expm1,
            Erf,
            Erfc,
            Tgamma,
            Lgamma,
            Zapgremlins,
            Wrap1,
            Fold1,
            Even,
            Odd,
            Graycode,
            Nextpowerof,
            Previouspowerof,
            Moddif,
            Softround,
            Snap,
            Lcurve,
            Gausscurve,
            Lg3interp,
            Calcfeedback,
            Bilin,
            Biexp,
            Besselj,
            Bessely,
            Blend,
        ];
        ALL.get(v as usize).copied()
    }

    /// The function's name, sclang's selector lowercased and joined.
    pub fn name(self) -> &'static str {
        use Calc::*;
        match self {
            Degrad => "degrad",
            Raddeg => "raddeg",
            Cbrt => "cbrt",
            Log1p => "log1p",
            Expm1 => "expm1",
            Erf => "erf",
            Erfc => "erfc",
            Tgamma => "tgamma",
            Lgamma => "lgamma",
            Zapgremlins => "zapgremlins",
            Wrap1 => "wrap1",
            Fold1 => "fold1",
            Even => "even",
            Odd => "odd",
            Graycode => "graycode",
            Nextpowerof => "nextpowerof",
            Previouspowerof => "previouspowerof",
            Moddif => "moddif",
            Softround => "softround",
            Snap => "snap",
            Lcurve => "lcurve",
            Gausscurve => "gausscurve",
            Lg3interp => "lg3interp",
            Calcfeedback => "calcfeedback",
            Bilin => "bilin",
            Biexp => "biexp",
            Besselj => "besselj",
            Bessely => "bessely",
            Blend => "blend",
        }
    }

    /// Resolves a name to the function (the inverse of [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<Calc> {
        (0..).map_while(Calc::from_u32).find(|c| c.name() == name)
    }

    /// How many arguments the function reads.
    pub fn arity(self) -> usize {
        use Calc::*;
        match self {
            Nextpowerof | Previouspowerof | Calcfeedback | Besselj | Bessely => 2,
            Moddif | Blend => 3,
            Softround | Snap | Gausscurve => 4,
            Lcurve | Lg3interp => 5,
            Bilin | Biexp => 8,
            _ => 1,
        }
    }
}

/// Applies a [`Calc`] to its arguments. Fewer than [`Calc::arity`] give NaN;
/// any more are ignored.
pub fn apply_calc(op: Calc, args: &[f64]) -> f64 {
    if args.len() < op.arity() {
        return f64::NAN;
    }
    let x = args[0];
    let arg = |i: usize| args[i];
    use Calc::*;
    match op {
        // sclang's own constants, which are the nearest doubles to pi/180 and
        // 180/pi.
        Degrad => x * 0.017_453_292_519_943_295,
        Raddeg => x * 57.295_779_513_082_32,
        Cbrt => x.cbrt(),
        Log1p => x.ln_1p(),
        Expm1 => x.exp_m1(),
        Erf => libm::erf(x),
        Erfc => libm::erfc(x),
        Tgamma => libm::tgamma(x),
        Lgamma => libm::lgamma(x),
        Zapgremlins => {
            let a = x.abs();
            if a > 1e-15 && a < 1e15 { x } else { 0.0 }
        }
        Wrap1 => {
            if x >= 1.0 {
                x - 2.0
            } else if x < -1.0 {
                x + 2.0
            } else {
                x
            }
        }
        Fold1 => {
            if x >= 1.0 {
                2.0 - x
            } else if x < -1.0 {
                -2.0 - x
            } else {
                x
            }
        }
        Even => ((x as i64) & 1 == 0) as u8 as f64,
        Odd => ((x as i64) & 1 == 1) as u8 as f64,
        Graycode => {
            let n = x as i64;
            (n ^ (n >> 1)) as f64
        }
        Nextpowerof => base_power(x, arg(1), 0),
        Previouspowerof => base_power(x, arg(1), -1),
        Moddif => {
            let m = arg(2);
            let diff = floored_mod((x - arg(1)).abs(), m);
            let half = m * 0.5;
            half - (diff - half).abs()
        }
        Softround => {
            let diff = round_to(x, arg(1)) - x;
            if diff.abs() > arg(2) {
                x + arg(3) * diff
            } else {
                x
            }
        }
        Snap => {
            let diff = round_to(x, arg(1)) - x;
            if diff.abs() < arg(2) {
                x + arg(3) * diff
            } else {
                x
            }
        }
        // As sclang writes it, `tau` multiplying the exponential rather than
        // dividing its argument.
        Lcurve => {
            let (a, m, n, tau) = (arg(1), arg(2), arg(3), arg(4));
            let e = (-x).exp();
            if tau == 1.0 {
                a * (m * e + 1.0) / (n * e + 1.0)
            } else {
                let r = 1.0 / tau;
                a * (m * e * r + 1.0) / (n * e * r + 1.0)
            }
        }
        Gausscurve => {
            let (a, b, c) = (arg(1), arg(2), arg(3));
            a * ((x - b) * (x - b) / (-2.0 * c * c)).exp()
        }
        Lg3interp => {
            let (a, b, c, d) = (arg(1), arg(2), arg(3), arg(4));
            let x0 = x + 1.0;
            let x2 = x - 1.0;
            let x3 = x - 2.0;
            let x03 = x0 * x3 * 0.5;
            let x12 = x * x2 / 6.0;
            x12 * (d * x0 - a * x3) + x03 * (b * x2 - c * x)
        }
        Calcfeedback => {
            let (delay, decay) = (x, arg(1));
            if delay == 0.0 || decay == 0.0 {
                0.0
            } else {
                (LOG001 * delay / decay.abs()).exp().copysign(decay)
            }
        }
        Bilin | Biexp => {
            let (centre, lo, hi) = (arg(1), arg(2), arg(3));
            let (out_centre, out_lo, out_hi) = (arg(4), arg(5), arg(6));
            let clip = crate::warp::Clip::from_u32(arg(7) as u32).unwrap_or_default();
            let (clips_lo, clips_hi) = match clip {
                crate::warp::Clip::MinMax => (true, true),
                crate::warp::Clip::Min => (true, false),
                crate::warp::Clip::Max => (false, true),
                crate::warp::Clip::None => (false, false),
            };
            if clips_lo && x <= lo {
                return out_lo;
            }
            if clips_hi && x >= hi {
                return out_hi;
            }
            let read = |x: f64, a: f64, b: f64| {
                if op == Bilin {
                    (x - a) / (b - a)
                } else {
                    (x / a).ln() / (b / a).ln()
                }
            };
            if x >= centre {
                read(x, centre, hi) * (out_hi - out_centre) + out_centre
            } else {
                read(x, lo, centre) * (out_centre - out_lo) + out_lo
            }
        }
        Blend => x + arg(2) * (arg(1) - x),
        Besselj => libm::jn(x as i32, arg(1)),
        Bessely => libm::yn(x as i32, arg(1)),
    }
}

/// sclang's `round(x, quant)`: the nearest multiple, a zero step the identity.
fn round_to(x: f64, quant: f64) -> f64 {
    if quant == 0.0 {
        x
    } else {
        (x / quant + 0.5).floor() * quant
    }
}

/// sclang's `%`: floored, so the result takes the divisor's sign.
fn floored_mod(x: f64, m: f64) -> f64 {
    if m == 0.0 {
        0.0
    } else {
        x - m * (x / m).floor()
    }
}

/// `base` to the power that reaches `x` from below (`offset` 0), or the one
/// before it (`offset` -1).
///
/// sclang computes the exponent as `ceil(ln x / ln base)`, which sends an
/// exact power past itself whenever the division rounds up -- `ln 8 / ln 2` is
/// `3.0000000000000004`, so sclang's `8.0.nextPowerOfTwo` is 16. The exponent
/// is corrected here so a power maps to itself, which is what the name says.
fn base_power(x: f64, base: f64, offset: i32) -> f64 {
    let mut k = (x.ln() / base.ln()).ceil();
    if k.is_finite() && base.powf(k - 1.0) >= x {
        k -= 1.0;
    }
    base.powf(k + f64::from(offset))
}

/// The functions that read a scale or a sorted list. The discriminants are
/// the stable C-ABI contract: append only.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScaleCalc {
    /// Scale degree to key: `degree` indexes the scale (offsets within one
    /// octave of `steps`) with octave carry, and a fraction is an alteration
    /// as SuperCollider writes it (`1.1` is degree 1 raised a semitone).
    Degkey = 0,
    /// Key to scale degree, the inverse: a key between two degrees is a
    /// fractional degree, linearly between them.
    Keydeg = 1,
    /// The scale's key nearest to `x`, in `x`'s own octave of `steps`.
    Nearestinscale = 2,
    /// The value of the sorted list nearest to `x`.
    Nearestinlist = 3,
    /// The fractional index of `x` in a sorted list, linearly between the two
    /// values around it.
    Indexinbetween = 4,
    /// The first index whose value is greater than `x`, or `-1` where none is.
    Indexofgreaterthan = 5,
}

impl ScaleCalc {
    /// Maps a C-ABI discriminant back to the enum.
    pub fn from_u32(v: u32) -> Option<ScaleCalc> {
        Some(match v {
            0 => ScaleCalc::Degkey,
            1 => ScaleCalc::Keydeg,
            2 => ScaleCalc::Nearestinscale,
            3 => ScaleCalc::Nearestinlist,
            4 => ScaleCalc::Indexinbetween,
            5 => ScaleCalc::Indexofgreaterthan,
            _ => return None,
        })
    }

    /// The function's name.
    pub fn name(self) -> &'static str {
        match self {
            ScaleCalc::Degkey => "degkey",
            ScaleCalc::Keydeg => "keydeg",
            ScaleCalc::Nearestinscale => "nearestinscale",
            ScaleCalc::Nearestinlist => "nearestinlist",
            ScaleCalc::Indexinbetween => "indexinbetween",
            ScaleCalc::Indexofgreaterthan => "indexofgreaterthan",
        }
    }

    /// Resolves a name to the function (the inverse of [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<ScaleCalc> {
        (0..)
            .map_while(ScaleCalc::from_u32)
            .find(|c| c.name() == name)
    }
}

/// Applies a [`ScaleCalc`] to `x` over `list` with `steps` per octave (which
/// [`ScaleCalc::Nearestinlist`] does not read). An empty list gives NaN.
pub fn apply_scale_calc(op: ScaleCalc, x: f64, list: &[f64], steps: f64) -> f64 {
    if list.is_empty() {
        return f64::NAN;
    }
    match op {
        ScaleCalc::Degkey => degkey(x, list, steps),
        ScaleCalc::Keydeg => {
            let n = list.len() as f64;
            let octave = (x / steps).floor() * n;
            index_in_between(list, floored_mod(x, steps)) + octave
        }
        ScaleCalc::Nearestinscale => {
            let root = (x / steps).floor() * steps;
            nearest_in_list(list, floored_mod(x, steps)) + root
        }
        ScaleCalc::Nearestinlist => nearest_in_list(list, x),
        ScaleCalc::Indexinbetween => index_in_between(list, x),
        ScaleCalc::Indexofgreaterthan => index_of_greater_than(list, x).map_or(-1.0, |i| i as f64),
    }
}

/// sclang's `degreeToKey`, with the degree read the way the rest of the core
/// reads one ([`split_degree`]): the degree it rounds to, and its fraction as
/// an alteration in tenths, scaled to the octave's steps.
fn degkey(degree: f64, scale: &[f64], steps: f64) -> f64 {
    let (whole, alter) = split_degree(degree);
    let n = scale.len() as i64;
    let d = whole as i64;
    steps * d.div_euclid(n) as f64 + scale[d.rem_euclid(n) as usize] + alter * (steps / 12.0)
}

/// The first index whose value is greater than `x` in a sorted list.
pub fn index_of_greater_than(list: &[f64], x: f64) -> Option<usize> {
    list.iter().position(|&v| v > x)
}

/// The fractional index of `x` in a sorted list, linearly between the two
/// values around it -- sclang's `indexInBetween`. Below the first value it is
/// 0, past the last the last index.
pub fn index_in_between(list: &[f64], x: f64) -> f64 {
    match index_of_greater_than(list, x) {
        None => (list.len() - 1) as f64,
        Some(0) => 0.0,
        Some(i) => {
            let (a, b) = (list[i - 1], list[i]);
            let span = b - a;
            if span == 0.0 {
                i as f64
            } else {
                (x - a) / span + i as f64 - 1.0
            }
        }
    }
}

/// The value of a sorted list nearest to `x` -- sclang's `nearestInList`, a
/// tie going to the upper one.
pub fn nearest_in_list(list: &[f64], x: f64) -> f64 {
    let i = match index_of_greater_than(list, x) {
        None => list.len() - 1,
        Some(0) => 0,
        Some(j) => {
            if x - list[j - 1] < list[j] - x {
                j - 1
            } else {
                j
            }
        }
    };
    list[i]
}

/// The list read at a fractional `index`, blended between the two values
/// around it -- sclang's `blendAt`. `method` says what an index past either end
/// reads: held at the end ([`BoundOp::Clip`](crate::builtins::BoundOp::Clip),
/// sclang's default), wrapped around, or folded back. An empty list gives NaN.
pub fn blend_at(list: &[f64], index: f64, method: crate::builtins::BoundOp) -> f64 {
    use crate::builtins::BoundOp;
    let n = list.len() as i64;
    if n == 0 {
        return f64::NAN;
    }
    let at = |i: i64| -> f64 {
        let i = match method {
            BoundOp::Clip => i.clamp(0, n - 1),
            BoundOp::Wrap => i.rem_euclid(n),
            BoundOp::Fold => {
                if n == 1 {
                    0
                } else {
                    let period = 2 * (n - 1);
                    let m = i.rem_euclid(period);
                    if m < n { m } else { period - m }
                }
            }
        };
        list[i as usize]
    };
    let lo = index.ceil() as i64 - 1;
    let (a, b) = (at(lo), at(lo + 1));
    a + (index - lo as f64).abs() * (b - a)
}

/// The functions that turn one list into another. The discriminants are the
/// stable C-ABI contract: append only.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListOp {
    /// Every value mapped linearly from the list's own range onto `a..b`; a
    /// list with no range gives `a` throughout.
    Normalize = 0,
    /// Every value divided by the list's sum, so the list sums to 1.
    Normalizesum = 1,
    /// The running sum.
    Integrate = 2,
    /// Each value less the one before it (the first less 0).
    Differentiate = 3,
    /// The list resampled to `a` values by taking the nearest.
    Resamp0 = 4,
    /// The list resampled to `a` values by linear interpolation.
    Resamp1 = 5,
    /// The table [`Rng::tablerand`](crate::rng::Rng::tablerand) reads to
    /// draw with the list as a probability shape, `a` values long.
    Asrandomtable = 6,
    /// The list and then back again, the last value once: `1 2 3 2 1`.
    Mirror = 7,
    /// The list and then back again, neither end repeated: `1 2 3 2`.
    Mirror1 = 8,
    /// The list and then reversed, both ends twice: `1 2 3 3 2 1`.
    Mirror2 = 9,
    /// Every value `a` times in a row (sclang's `stutter`, its `dupEach`).
    Stutter = 10,
    /// The list rotated `a` places to the right, wrapping.
    Rotate = 11,
    /// The list unfolded into growing and shrinking runs, by sclang's pattern
    /// type `a` (1 to 10, clipped): type 1 is every prefix, `1  1 2  1 2 3`.
    Pyramid = 12,
    /// How long each run of [`Pyramid`](Self::Pyramid) is, for the same type
    /// `a` -- the groups sclang's `pyramidg` splits it into. The values are
    /// not read, only how many there are.
    Pyramidsizes = 13,
}

impl ListOp {
    /// Maps a C-ABI discriminant back to the enum.
    pub fn from_u32(v: u32) -> Option<ListOp> {
        use ListOp::*;
        Some(match v {
            0 => Normalize,
            1 => Normalizesum,
            2 => Integrate,
            3 => Differentiate,
            4 => Resamp0,
            5 => Resamp1,
            6 => Asrandomtable,
            7 => Mirror,
            8 => Mirror1,
            9 => Mirror2,
            10 => Stutter,
            11 => Rotate,
            12 => Pyramid,
            13 => Pyramidsizes,
            _ => return None,
        })
    }

    /// The function's name.
    pub fn name(self) -> &'static str {
        use ListOp::*;
        match self {
            Normalize => "normalize",
            Normalizesum => "normalizesum",
            Integrate => "integrate",
            Differentiate => "differentiate",
            Resamp0 => "resamp0",
            Resamp1 => "resamp1",
            Asrandomtable => "asrandomtable",
            Mirror => "mirror",
            Mirror1 => "mirror1",
            Mirror2 => "mirror2",
            Stutter => "stutter",
            Rotate => "rotate",
            Pyramid => "pyramid",
            Pyramidsizes => "pyramidsizes",
        }
    }

    /// Resolves a name to the function (the inverse of [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<ListOp> {
        (0..).map_while(ListOp::from_u32).find(|o| o.name() == name)
    }

    /// How long the result is for an input of `n` values with argument `a`.
    pub fn output_len(self, n: usize, a: f64) -> usize {
        use ListOp::*;
        match self {
            Resamp0 | Resamp1 | Asrandomtable => a.max(0.0) as usize,
            Mirror => (2 * n).saturating_sub(1),
            Mirror1 => (2 * n).saturating_sub(2).max(n),
            Mirror2 => 2 * n,
            Stutter => n * a.max(0.0) as usize,
            Pyramid => pyramid_runs(n, a).iter().map(|&(lo, hi)| hi - lo).sum(),
            Pyramidsizes => pyramid_runs(n, a).len(),
            _ => n,
        }
    }
}

/// The runs of [`ListOp::Pyramid`] over a list of `n` values, as half-open
/// index ranges in order: sclang's `_ArrayPyramid` primitive, whose ten pattern
/// types (`kind`, clipped to 1..10) are its growing and shrinking prefixes and
/// suffixes and their combinations. sclang's `pyramidg` builds the same runs
/// as groups.
pub fn pyramid_runs(n: usize, kind: f64) -> Vec<(usize, usize)> {
    if n == 0 {
        return Vec::new();
    }
    let prefix = |len: usize| (0, len);
    let suffix = |len: usize| (n - len, n);
    let up = 1..=n;
    let down = (1..n).rev();
    let mut runs = Vec::new();
    // Each type is one or two passes; the second never repeats the run the
    // first ended on.
    match (kind as i64).clamp(1, 10) {
        1 => runs.extend(up.map(prefix)),
        2 => runs.extend(up.map(suffix)),
        3 => runs.extend((1..=n).rev().map(prefix)),
        4 => runs.extend((1..=n).rev().map(suffix)),
        5 => runs.extend(up.map(prefix).chain(down.map(prefix))),
        6 => runs.extend(up.map(suffix).chain(down.map(suffix))),
        7 => runs.extend((1..=n).rev().map(prefix).chain((2..=n).map(prefix))),
        8 => runs.extend((1..=n).rev().map(suffix).chain((2..=n).map(suffix))),
        9 => runs.extend(up.map(prefix).chain(down.map(suffix))),
        _ => runs.extend(up.map(suffix).chain(down.map(prefix))),
    }
    runs
}

/// Applies a [`ListOp`] to `list` with its arguments `a` and `b`, writing
/// [`ListOp::output_len`] values into `out` (which must be that long).
pub fn apply_list(op: ListOp, list: &[f64], a: f64, b: f64, out: &mut [f64]) {
    use ListOp::*;
    match op {
        Normalize => {
            let lo = list.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = list.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            for (o, &x) in out.iter_mut().zip(list) {
                // sclang's `linlin` with its default clip, which is what holds
                // a list with no range at `a` rather than dividing by zero.
                *o = if x <= lo {
                    a
                } else if x >= hi {
                    b
                } else {
                    (x - lo) / (hi - lo) * (b - a) + a
                };
            }
        }
        Normalizesum => {
            let r = 1.0 / list.iter().sum::<f64>();
            for (o, &x) in out.iter_mut().zip(list) {
                *o = x * r;
            }
        }
        Integrate => {
            let mut sum = 0.0;
            for (o, &x) in out.iter_mut().zip(list) {
                sum += x;
                *o = sum;
            }
        }
        Differentiate => {
            let mut prev = 0.0;
            for (o, &x) in out.iter_mut().zip(list) {
                *o = x - prev;
                prev = x;
            }
        }
        Resamp0 | Resamp1 => {
            if list.is_empty() {
                out.fill(f64::NAN);
                return;
            }
            let factor = (list.len() - 1) as f64 / (out.len() as f64 - 1.0).max(1.0);
            for (i, o) in out.iter_mut().enumerate() {
                let at = i as f64 * factor;
                *o = if op == Resamp0 {
                    list[((at + 0.5).floor() as usize).min(list.len() - 1)]
                } else {
                    blend_at(list, at, crate::builtins::BoundOp::Clip)
                };
            }
        }
        Asrandomtable => {
            let size = out.len();
            if list.is_empty() || size == 0 {
                out.fill(f64::NAN);
                return;
            }
            let mut shape = vec![0.0; size];
            apply_list(Resamp1, list, size as f64, 0.0, &mut shape);
            let mut running = vec![0.0; size];
            apply_list(Integrate, &shape, 0.0, 0.0, &mut running);
            let mut scaled = vec![0.0; size];
            apply_list(Normalize, &running, 0.0, (size - 1) as f64, &mut scaled);
            for (i, o) in out.iter_mut().enumerate() {
                *o = index_in_between(&scaled, i as f64) / size as f64;
            }
        }
        Mirror | Mirror1 | Mirror2 => {
            // Forward, then back from where each variant turns.
            let back: Box<dyn Iterator<Item = &f64>> = match op {
                Mirror => Box::new(list.iter().rev().skip(1)),
                Mirror1 => Box::new(list.iter().rev().skip(1).take(list.len().saturating_sub(2))),
                _ => Box::new(list.iter().rev()),
            };
            for (o, &x) in out.iter_mut().zip(list.iter().chain(back)) {
                *o = x;
            }
        }
        Stutter => {
            let times = a.max(0.0) as usize;
            for (o, &x) in out
                .iter_mut()
                .zip(list.iter().flat_map(|x| std::iter::repeat_n(x, times)))
            {
                *o = x;
            }
        }
        Rotate => {
            let n = list.len();
            if n == 0 {
                return;
            }
            let shift = (a as i64).rem_euclid(n as i64) as usize;
            for (i, &x) in list.iter().enumerate() {
                out[(i + shift) % n] = x;
            }
        }
        Pyramid => {
            let values = pyramid_runs(list.len(), a)
                .into_iter()
                .flat_map(|(lo, hi)| &list[lo..hi]);
            for (o, &x) in out.iter_mut().zip(values) {
                *o = x;
            }
        }
        Pyramidsizes => {
            for (o, (lo, hi)) in out.iter_mut().zip(pyramid_runs(list.len(), a)) {
                *o = (hi - lo) as f64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAJOR: [f64; 7] = [0.0, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0];

    fn calc(name: &str, args: &[f64]) -> f64 {
        apply_calc(Calc::from_name(name).unwrap(), args)
    }

    #[test]
    fn the_table_round_trips_and_reads_its_arity() {
        for op in (0..).map_while(Calc::from_u32) {
            assert_eq!(Calc::from_name(op.name()), Some(op));
            assert!(
                apply_calc(op, &[]).is_nan(),
                "{} with no arguments",
                op.name()
            );
        }
        assert_eq!(Calc::from_u32(29), None);
        for op in (0..).map_while(ScaleCalc::from_u32) {
            assert_eq!(ScaleCalc::from_name(op.name()), Some(op));
        }
    }

    #[test]
    fn sclangs_numbers() {
        assert!((calc("degrad", &[180.0]) - core::f64::consts::PI).abs() < 1e-15);
        assert!((calc("raddeg", &[core::f64::consts::PI]) - 180.0).abs() < 1e-12);
        assert_eq!(calc("zapgremlins", &[1e-20]), 0.0);
        assert_eq!(calc("zapgremlins", &[f64::INFINITY]), 0.0);
        assert_eq!(calc("zapgremlins", &[0.5]), 0.5);
        assert_eq!(calc("wrap1", &[1.25]), -0.75);
        assert_eq!(calc("fold1", &[1.25]), 0.75);
        assert_eq!((calc("even", &[4.0]), calc("odd", &[4.0])), (1.0, 0.0));
        assert_eq!(calc("odd", &[-3.0]), 1.0);
        assert_eq!(calc("graycode", &[5.0]), 7.0);
        assert_eq!(
            calc("nextpowerof", &[8.0, 2.0]),
            8.0,
            "a power maps to itself"
        );
        assert_eq!(calc("nextpowerof", &[9.0, 2.0]), 16.0);
        assert_eq!(calc("nextpowerof", &[9.0, 3.0]), 9.0);
        assert_eq!(calc("previouspowerof", &[9.0, 2.0]), 8.0);
        assert!(
            (calc("moddif", &[0.9, 0.1, 1.0]) - 0.2).abs() < 1e-12,
            "around the circle"
        );
        assert_eq!(calc("snap", &[1.02, 1.0, 0.05, 1.0]), 1.0);
        assert_eq!(calc("snap", &[1.2, 1.0, 0.05, 1.0]), 1.2);
        assert_eq!(calc("softround", &[1.2, 1.0, 0.05, 0.5]), 1.1);
        assert_eq!(calc("gausscurve", &[0.0, 1.0, 0.0, 1.0]), 1.0);
        assert!((calc("lcurve", &[0.0, 1.0, 0.0, 1.0, 1.0]) - 0.5).abs() < 1e-15);
        // Lagrange through four points of a line is the line.
        assert!((calc("lg3interp", &[0.25, 0.0, 1.0, 2.0, 3.0]) - 1.25).abs() < 1e-12);
        // -60 dB in one second from a delay of a second is 0.001.
        assert!((calc("calcfeedback", &[1.0, 1.0]) - 0.001).abs() < 1e-12);
        assert!((calc("calcfeedback", &[1.0, -1.0]) + 0.001).abs() < 1e-12);
        assert!((calc("erf", &[0.0])).abs() < 1e-15);
        assert!((calc("tgamma", &[5.0]) - 24.0).abs() < 1e-9);
        assert!((calc("besselj", &[0.0, 0.0]) - 1.0).abs() < 1e-15);
        // A centred map: each half onto its half.
        let bilin = |x| calc("bilin", &[x, 0.5, 0.0, 1.0, 10.0, 0.0, 100.0, 0.0]);
        assert_eq!((bilin(0.25), bilin(0.75), bilin(2.0)), (5.0, 55.0, 100.0));
        let biexp = calc("biexp", &[100.0, 100.0, 10.0, 1000.0, 0.0, -1.0, 1.0, 0.0]);
        assert_eq!(biexp, 0.0);
    }

    #[test]
    fn scale_degrees_and_keys() {
        let deg = |d| apply_scale_calc(ScaleCalc::Degkey, d, &MAJOR, 12.0);
        assert_eq!(
            (deg(0.0), deg(2.0), deg(7.0), deg(-1.0)),
            (0.0, 4.0, 12.0, -1.0)
        );
        assert_eq!(deg(1.1), 3.0, "a tenth is a semitone up");
        let key = |k| apply_scale_calc(ScaleCalc::Keydeg, k, &MAJOR, 12.0);
        assert_eq!((key(4.0), key(12.0), key(3.0)), (2.0, 7.0, 1.5));
        let near = |k| apply_scale_calc(ScaleCalc::Nearestinscale, k, &MAJOR, 12.0);
        // A tie goes to the upper key, as sclang's `indexIn` does.
        assert_eq!((near(6.0), near(13.0), near(-1.0)), (7.0, 14.0, -1.0));
        assert_eq!(
            apply_scale_calc(ScaleCalc::Nearestinlist, 3.4, &[1.0, 3.0, 7.0], 0.0),
            3.0
        );
        assert_eq!(index_in_between(&[1.0, 3.0, 7.0], 5.0), 1.5);
        assert!(apply_scale_calc(ScaleCalc::Degkey, 0.0, &[], 12.0).is_nan());
        let gt = |x| apply_scale_calc(ScaleCalc::Indexofgreaterthan, x, &[1.0, 3.0, 7.0], 0.0);
        assert_eq!((gt(2.0), gt(9.0)), (1.0, -1.0));
    }

    #[test]
    fn lists_blend_resample_and_reshape() {
        use crate::builtins::BoundOp;
        let l = [0.0, 10.0, 20.0, 30.0];
        assert_eq!(blend_at(&l, 1.25, BoundOp::Clip), 12.5);
        assert_eq!(blend_at(&l, 0.0, BoundOp::Clip), 0.0);
        assert_eq!(blend_at(&l, 3.5, BoundOp::Clip), 30.0);
        assert_eq!(blend_at(&l, 3.5, BoundOp::Wrap), 15.0);
        assert_eq!(blend_at(&l, 3.5, BoundOp::Fold), 25.0);
        assert_eq!(calc("blend", &[1.0, 3.0, 0.25]), 1.5);
        let run = |op: ListOp, list: &[f64], a: f64, b: f64| {
            let mut out = vec![0.0; op.output_len(list.len(), a)];
            apply_list(op, list, a, b, &mut out);
            out
        };
        assert_eq!(
            run(ListOp::Normalize, &[2.0, 4.0, 3.0], 0.0, 1.0),
            [0.0, 1.0, 0.5]
        );
        assert_eq!(run(ListOp::Normalize, &[5.0, 5.0], -1.0, 1.0), [-1.0, -1.0]);
        assert_eq!(
            run(ListOp::Normalizesum, &[1.0, 3.0], 0.0, 0.0),
            [0.25, 0.75]
        );
        assert_eq!(
            run(ListOp::Integrate, &[1.0, 2.0, 3.0], 0.0, 0.0),
            [1.0, 3.0, 6.0]
        );
        assert_eq!(
            run(ListOp::Differentiate, &[1.0, 3.0, 6.0], 0.0, 0.0),
            [1.0, 2.0, 3.0]
        );
        assert_eq!(
            run(ListOp::Resamp1, &[0.0, 10.0], 5.0, 0.0),
            [0.0, 2.5, 5.0, 7.5, 10.0]
        );
        assert_eq!(
            run(ListOp::Resamp0, &[0.0, 10.0, 20.0], 5.0, 0.0),
            [0.0, 10.0, 10.0, 20.0, 20.0]
        );
        // A flat shape is the identity ramp: every position equally likely.
        let table = run(ListOp::Asrandomtable, &[1.0, 1.0, 1.0, 1.0], 4.0, 0.0);
        assert_eq!(table.len(), 4);
        assert!(table.windows(2).all(|w| w[0] <= w[1]), "{table:?}");
        for name in [
            "normalize",
            "resamp1",
            "asrandomtable",
            "mirror",
            "pyramidsizes",
        ] {
            assert_eq!(ListOp::from_name(name).map(|o| o.name()), Some(name));
        }
    }

    fn reshape(op: ListOp, list: &[f64], a: f64) -> Vec<f64> {
        let mut out = vec![0.0; op.output_len(list.len(), a)];
        apply_list(op, list, a, 0.0, &mut out);
        out
    }

    #[test]
    fn the_reshaping_functions_are_sclangs() {
        let l = [1.0, 2.0, 3.0];
        assert_eq!(reshape(ListOp::Mirror, &l, 0.0), [1.0, 2.0, 3.0, 2.0, 1.0]);
        assert_eq!(reshape(ListOp::Mirror1, &l, 0.0), [1.0, 2.0, 3.0, 2.0]);
        assert_eq!(
            reshape(ListOp::Mirror2, &l, 0.0),
            [1.0, 2.0, 3.0, 3.0, 2.0, 1.0]
        );
        assert_eq!(reshape(ListOp::Mirror1, &[1.0, 2.0], 0.0), [1.0, 2.0]);
        assert_eq!(reshape(ListOp::Mirror1, &[7.0], 0.0), [7.0]);
        assert!(reshape(ListOp::Mirror, &[], 0.0).is_empty());
        assert_eq!(
            reshape(ListOp::Stutter, &[1.0, 2.0], 3.0),
            [1.0, 1.0, 1.0, 2.0, 2.0, 2.0]
        );
        assert!(reshape(ListOp::Stutter, &l, 0.0).is_empty());
        assert_eq!(reshape(ListOp::Rotate, &l, 1.0), [3.0, 1.0, 2.0]);
        assert_eq!(reshape(ListOp::Rotate, &l, -1.0), [2.0, 3.0, 1.0]);
        assert_eq!(reshape(ListOp::Rotate, &l, 4.0), [3.0, 1.0, 2.0]);
    }

    /// sclang's `_ArrayPyramid` over `[0, 1, 2, 3]`, per pattern type -- the
    /// primitive's own loops, run by hand.
    #[test]
    fn the_pyramid_runs_are_the_primitives() {
        let l = [0.0, 1.0, 2.0, 3.0];
        let expected: [&[f64]; 10] = [
            &[0., 0., 1., 0., 1., 2., 0., 1., 2., 3.],
            &[3., 2., 3., 1., 2., 3., 0., 1., 2., 3.],
            &[0., 1., 2., 3., 0., 1., 2., 0., 1., 0.],
            &[0., 1., 2., 3., 1., 2., 3., 2., 3., 3.],
            &[
                0., 0., 1., 0., 1., 2., 0., 1., 2., 3., 0., 1., 2., 0., 1., 0.,
            ],
            &[
                3., 2., 3., 1., 2., 3., 0., 1., 2., 3., 1., 2., 3., 2., 3., 3.,
            ],
            &[
                0., 1., 2., 3., 0., 1., 2., 0., 1., 0., 0., 1., 0., 1., 2., 0., 1., 2., 3.,
            ],
            &[
                0., 1., 2., 3., 1., 2., 3., 2., 3., 3., 2., 3., 1., 2., 3., 0., 1., 2., 3.,
            ],
            &[
                0., 0., 1., 0., 1., 2., 0., 1., 2., 3., 1., 2., 3., 2., 3., 3.,
            ],
            &[
                3., 2., 3., 1., 2., 3., 0., 1., 2., 3., 0., 1., 2., 0., 1., 0.,
            ],
        ];
        for (kind, want) in expected.iter().enumerate() {
            let got = reshape(ListOp::Pyramid, &l, kind as f64 + 1.0);
            assert_eq!(&got[..], *want, "type {}", kind + 1);
        }
        // Out-of-range types clip, as the primitive does.
        assert_eq!(
            reshape(ListOp::Pyramid, &l, 0.0),
            reshape(ListOp::Pyramid, &l, 1.0)
        );
        assert_eq!(
            reshape(ListOp::Pyramid, &l, 99.0),
            reshape(ListOp::Pyramid, &l, 10.0)
        );
        assert_eq!(
            reshape(ListOp::Pyramidsizes, &l, 5.0),
            [1.0, 2.0, 3.0, 4.0, 3.0, 2.0, 1.0]
        );
        assert!(reshape(ListOp::Pyramid, &[], 3.0).is_empty());
    }
}
