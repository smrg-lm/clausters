//! Numeric builtins on scalars and slices.
//!
//! Two operator families, each as (a) a `#[repr(u32)]` enum so the C ABI can
//! pass an op by integer, (b) a scalar `apply_*` function, and (c) a
//! broadcasting slice `*_slice` function that writes into a caller-provided
//! output (no allocation — the audio thread and the FFI both call these).
//!
//! Semantics: `Add`/`Sub`/`Mul`/`Div` are exactly the server's `dsp::binop`
//! (so the refactored server stays bit-identical). The remaining ops mirror
//! Faust's Signal API (`sin`, `log`, `min`, `pow`, …, see the server's
//! `faust::signals`) with the same formula; they are *not* guaranteed
//! bit-identical to Faust's LLVM codegen — that is the documented tolerance.
//!
//! Slice broadcasting matches the server's [`at`] rule: a length-1 input is a
//! constant broadcast over the block, any other length is indexed per frame.

/// Reads input `i` from a block or a single-sample (constant) slice — the
/// server's `dsp::at`, kept in lockstep so slice ops broadcast identically.
#[inline(always)]
pub fn at(input: &[f32], i: usize) -> f32 {
    if input.len() == 1 { input[0] } else { input[i] }
}

/// Binary operators. The discriminants are the stable C-ABI contract: append
/// only, never renumber.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinaryOp {
    Add = 0,
    Sub = 1,
    Mul = 2,
    Div = 3,
    /// Floating remainder (truncated, like C `fmod` / Rust `%`).
    Mod = 4,
    Pow = 5,
    Min = 6,
    Max = 7,
    Atan2 = 8,
    Gt = 9,
    Lt = 10,
    Ge = 11,
    Le = 12,
    Eq = 13,
    Ne = 14,
    /// Bitwise ops act on the `i32` casts of the operands, as Faust does.
    And = 15,
    Or = 16,
    Xor = 17,
    /// Logical left/right shift by the truncated integer second operand.
    Lsh = 18,
    Rsh = 19,
    /// `sqrt(a*a + b*b)`.
    Hypot = 20,
    /// `a*b + a`.
    Ring1 = 21,
    /// `a*b + a + b`.
    Ring2 = 22,
    /// `a*a*b`.
    Ring3 = 23,
    /// `a*a*b - a*b*b`.
    Ring4 = 24,
    /// `a*a + b*b`.
    Sumsqr = 25,
    /// `a*a - b*b`.
    Difsqr = 26,
    /// `(a+b)*(a+b)`.
    Sqrsum = 27,
    /// `(a-b)*(a-b)`.
    Sqrdif = 28,
    /// `abs(a - b)`.
    Absdif = 29,
    /// `a < b ? 0 : a` (gate `a` below threshold `b`).
    Thresh = 30,
    /// Clip `a` to the symmetric range `[-b, b]`.
    Clip2 = 31,
    /// `a - clip2(a, b)` — the part of `a` outside `[-b, b]`.
    Excess = 32,
    /// Round `a` to the nearest multiple of `b` (`b == 0` passes `a` through).
    Round = 33,
    /// Truncate `a` toward zero-of-grid to a multiple of `b` (`b == 0` = `a`).
    Trunc = 34,
    /// Fold `a` into the symmetric range `[-b, b]` (reflecting at both ends,
    /// repeatedly). The bilateral form of scsynth's `sc_fold`.
    Fold2 = 35,
    /// Wrap `a` into the symmetric range `[-b, b]` (modulo, not reflecting).
    /// The bilateral form of scsynth's `sc_wrap`.
    Wrap2 = 36,
    /// Greatest common divisor of the **integer truncations** of `a` and `b`.
    Gcd = 37,
    /// Least common multiple of the **integer truncations** of `a` and `b`.
    Lcm = 38,
    /// A cheap approximation of `hypot` (see [`apply_binary`] for the exact
    /// formula and its error bound).
    HypotApx = 39,
}

impl BinaryOp {
    /// Maps a C-ABI discriminant back to the enum.
    pub fn from_u32(v: u32) -> Option<BinaryOp> {
        use BinaryOp::*;
        Some(match v {
            0 => Add,
            1 => Sub,
            2 => Mul,
            3 => Div,
            4 => Mod,
            5 => Pow,
            6 => Min,
            7 => Max,
            8 => Atan2,
            9 => Gt,
            10 => Lt,
            11 => Ge,
            12 => Le,
            13 => Eq,
            14 => Ne,
            15 => And,
            16 => Or,
            17 => Xor,
            18 => Lsh,
            19 => Rsh,
            20 => Hypot,
            21 => Ring1,
            22 => Ring2,
            23 => Ring3,
            24 => Ring4,
            25 => Sumsqr,
            26 => Difsqr,
            27 => Sqrsum,
            28 => Sqrdif,
            29 => Absdif,
            30 => Thresh,
            31 => Clip2,
            32 => Excess,
            33 => Round,
            34 => Trunc,
            35 => Fold2,
            36 => Wrap2,
            37 => Gcd,
            38 => Lcm,
            39 => HypotApx,
            _ => return None,
        })
    }

    /// The operator's **wire name** — the public identifier a def uses
    /// (`BinaryOpUGen`'s `"op"` field). The numeric discriminant above is an
    /// internal C-ABI detail; names are what cross the wire and appear in docs.
    pub fn name(self) -> &'static str {
        use BinaryOp::*;
        match self {
            Add => "add",
            Sub => "sub",
            Mul => "mul",
            Div => "div",
            Mod => "mod",
            Pow => "pow",
            Min => "min",
            Max => "max",
            Atan2 => "atan2",
            Gt => "gt",
            Lt => "lt",
            Ge => "ge",
            Le => "le",
            Eq => "eq",
            Ne => "ne",
            And => "bitand",
            Or => "bitor",
            Xor => "bitxor",
            Lsh => "lshift",
            Rsh => "rshift",
            Hypot => "hypot",
            Ring1 => "ring1",
            Ring2 => "ring2",
            Ring3 => "ring3",
            Ring4 => "ring4",
            Sumsqr => "sumsqr",
            Difsqr => "difsqr",
            Sqrsum => "sqrsum",
            Sqrdif => "sqrdif",
            Absdif => "absdif",
            Thresh => "thresh",
            Clip2 => "clip2",
            Excess => "excess",
            Round => "round",
            Trunc => "trunc",
            Fold2 => "fold2",
            Wrap2 => "wrap2",
            Gcd => "gcd",
            Lcm => "lcm",
            // Lowercase/snake_case like every other name in this table, which
            // is our spelling convention even where scsynth's selector is
            // camelCase (`asInteger` is `as_int` here for the same reason).
            HypotApx => "hypot_apx",
        }
    }

    /// Resolves a wire name to the operator (the inverse of [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<BinaryOp> {
        (0..)
            .map_while(BinaryOp::from_u32)
            .find(|op| op.name() == name)
    }
}

/// Unary operators. Discriminants are the stable C-ABI contract.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnaryOp {
    Neg = 0,
    Abs = 1,
    Sine = 2,
    Cos = 3,
    Tan = 4,
    Asin = 5,
    Acos = 6,
    Atan = 7,
    Exp = 8,
    Exp10 = 9,
    Log = 10,
    Log10 = 11,
    Sqrt = 12,
    Floor = 13,
    Ceil = 14,
    /// Round to nearest, ties to even (C `rint`).
    Rint = 15,
    /// Truncating cast to `i32` and back (C `int(x)`).
    IntCast = 16,
    /// Identity for `f32` input (Faust `float(x)`); kept for op-table symmetry.
    FloatCast = 17,
    /// `x*x`.
    Squared = 18,
    /// `x*x*x`.
    Cubed = 19,
    /// `1.0 / x`.
    Recip = 20,
    /// Fractional part `x - floor(x)`.
    Frac = 21,
    /// `-1`, `0` or `1` by the sign of `x`.
    Sign = 22,
    /// Base-2 logarithm.
    Log2 = 23,
    Sinh = 24,
    Cosh = 25,
    Tanh = 26,
    /// MIDI note number to frequency in Hz (12-TET, A4 = 69 = 440 Hz).
    Midicps = 27,
    /// Frequency in Hz to MIDI note number.
    Cpsmidi = 28,
    /// Semitone interval to a frequency ratio (`2^(x/12)`).
    Midiratio = 29,
    /// Frequency ratio to a semitone interval (`12*log2(x)`).
    Ratiomidi = 30,
    /// Decibels to a linear amplitude (`10^(x/20)`).
    Dbamp = 31,
    /// Linear amplitude to decibels (`20*log10(x)`).
    Ampdb = 32,
    /// Decimal octave (A440 = 4.75) to frequency in Hz.
    Octcps = 33,
    /// Frequency in Hz to decimal octave.
    Cpsoct = 34,
    /// Soft distortion `x / (1 + |x|)` (range ±1).
    Distort = 35,
    /// Cubic softclip: linear for `|x| <= 0.5`, saturating beyond.
    Softclip = 36,
}

impl UnaryOp {
    pub fn from_u32(v: u32) -> Option<UnaryOp> {
        use UnaryOp::*;
        Some(match v {
            0 => Neg,
            1 => Abs,
            2 => Sine,
            3 => Cos,
            4 => Tan,
            5 => Asin,
            6 => Acos,
            7 => Atan,
            8 => Exp,
            9 => Exp10,
            10 => Log,
            11 => Log10,
            12 => Sqrt,
            13 => Floor,
            14 => Ceil,
            15 => Rint,
            16 => IntCast,
            17 => FloatCast,
            18 => Squared,
            19 => Cubed,
            20 => Recip,
            21 => Frac,
            22 => Sign,
            23 => Log2,
            24 => Sinh,
            25 => Cosh,
            26 => Tanh,
            27 => Midicps,
            28 => Cpsmidi,
            29 => Midiratio,
            30 => Ratiomidi,
            31 => Dbamp,
            32 => Ampdb,
            33 => Octcps,
            34 => Cpsoct,
            35 => Distort,
            36 => Softclip,
            _ => return None,
        })
    }

    /// The operator's **wire name** — see [`BinaryOp::name`].
    pub fn name(self) -> &'static str {
        use UnaryOp::*;
        match self {
            Neg => "neg",
            Abs => "abs",
            Sine => "sin",
            Cos => "cos",
            Tan => "tan",
            Asin => "asin",
            Acos => "acos",
            Atan => "atan",
            Exp => "exp",
            Exp10 => "exp10",
            Log => "log",
            Log10 => "log10",
            Sqrt => "sqrt",
            Floor => "floor",
            Ceil => "ceil",
            Rint => "rint",
            IntCast => "as_int",
            FloatCast => "as_float",
            Squared => "squared",
            Cubed => "cubed",
            Recip => "recip",
            Frac => "frac",
            Sign => "sign",
            Log2 => "log2",
            Sinh => "sinh",
            Cosh => "cosh",
            Tanh => "tanh",
            Midicps => "midicps",
            Cpsmidi => "cpsmidi",
            Midiratio => "midiratio",
            Ratiomidi => "ratiomidi",
            Dbamp => "dbamp",
            Ampdb => "ampdb",
            Octcps => "octcps",
            Cpsoct => "cpsoct",
            Distort => "distort",
            Softclip => "softclip",
        }
    }

    /// Resolves a wire name to the operator (the inverse of [`name`](Self::name)).
    pub fn from_name(name: &str) -> Option<UnaryOp> {
        (0..)
            .map_while(UnaryOp::from_u32)
            .find(|op| op.name() == name)
    }
}

/// `sqrt(2) - 1`, the coefficient of [`BinaryOp::HypotApx`]. Derived in `f64`
/// and then rounded once, which is what scsynth's `kFSQRT2M1` does — computing
/// it in `f32` throughout would land a ULP away and cost the bit-parity the
/// operator table exists for.
const SQRT2_MINUS_1: f32 = (core::f64::consts::SQRT_2 - 1.0) as f32;

/// Wraps `x` into `[lo, hi)`, scsynth's `sc_wrap` — including its two
/// single-shift fast paths, which are not merely an optimization: the general
/// branch below runs on the *already shifted* value, so a faithful port has to
/// keep the same shape.
#[inline]
fn wrap(x: f32, lo: f32, hi: f32) -> f32 {
    let range = hi - lo;
    let x = if x >= hi {
        let shifted = x - range;
        if shifted < hi {
            return shifted;
        }
        shifted
    } else if x < lo {
        let shifted = x + range;
        if shifted >= lo {
            return shifted;
        }
        shifted
    } else {
        return x;
    };
    if hi == lo {
        return lo;
    }
    x - range * ((x - lo) / range).floor()
}

/// Folds `x` into `[lo, hi]`, scsynth's `sc_fold` — reflecting at both ends as
/// many times as needed. Note the general branch measures from the **original**
/// `x`, not from the once-reflected value, as scsynth's does.
///
/// Public because the bounded random walks (`Dbrown`/`Dibrown`) turn their
/// steps around with exactly this, and a walk that folded differently from
/// `fold2` would be a second definition of the same word.
#[inline]
pub fn fold(x: f32, lo: f32, hi: f32) -> f32 {
    let offset = x - lo;
    if x >= hi {
        let reflected = hi + hi - x;
        if reflected >= lo {
            return reflected;
        }
    } else if x < lo {
        let reflected = lo + lo - x;
        if reflected < hi {
            return reflected;
        }
    } else {
        return x;
    }
    if hi == lo {
        return lo;
    }
    let range = hi - lo;
    let range2 = range + range;
    let c = offset - range2 * (offset / range2).floor();
    (if c >= range { range2 - c } else { c }) + lo
}

/// Greatest common divisor with scsynth's sign convention: the result is
/// negative only when **both** operands are non-positive, and a zero operand
/// returns the other one unchanged.
///
/// Euclid runs on the unsigned magnitudes so that an operand of `i64::MIN`
/// (reachable, since a float-to-int cast saturates) cannot overflow its own
/// negation.
#[inline]
fn gcd_i64(a: i64, b: i64) -> i64 {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    let negative = a <= 0 && b <= 0;
    let (mut x, mut y) = (a.unsigned_abs(), b.unsigned_abs());
    if x == 1 || y == 1 {
        return if negative { -1 } else { 1 };
    }
    while y > 0 {
        let t = x % y;
        x = y;
        y = t;
    }
    let g = x.min(i64::MAX as u64) as i64;
    if negative { -g } else { g }
}

/// Applies a binary operator to two scalars.
#[inline]
pub fn apply_binary(op: BinaryOp, a: f32, b: f32) -> f32 {
    use BinaryOp::*;
    match op {
        Add => a + b,
        Sub => a - b,
        Mul => a * b,
        Div => a / b,
        Mod => a % b,
        Pow => a.powf(b),
        // Explicit comparison (Faust's `select2(a<b, …)`) for a deterministic
        // result independent of platform fmin/fmax NaN handling.
        Min => {
            if a < b {
                a
            } else {
                b
            }
        }
        Max => {
            if a > b {
                a
            } else {
                b
            }
        }
        Atan2 => a.atan2(b),
        Gt => (a > b) as i32 as f32,
        Lt => (a < b) as i32 as f32,
        Ge => (a >= b) as i32 as f32,
        Le => (a <= b) as i32 as f32,
        Eq => (a == b) as i32 as f32,
        Ne => (a != b) as i32 as f32,
        And => ((a as i32) & (b as i32)) as f32,
        Or => ((a as i32) | (b as i32)) as f32,
        Xor => ((a as i32) ^ (b as i32)) as f32,
        Lsh => ((a as i32).wrapping_shl(b as i32 as u32)) as f32,
        Rsh => ((a as i32).wrapping_shr(b as i32 as u32)) as f32,
        Hypot => (a * a + b * b).sqrt(),
        Ring1 => a * b + a,
        Ring2 => a * b + a + b,
        Ring3 => a * a * b,
        Ring4 => a * a * b - a * b * b,
        Sumsqr => a * a + b * b,
        Difsqr => a * a - b * b,
        Sqrsum => (a + b) * (a + b),
        Sqrdif => (a - b) * (a - b),
        Absdif => (a - b).abs(),
        Thresh => {
            if a < b {
                0.0
            } else {
                a
            }
        }
        Clip2 => {
            if a < -b {
                -b
            } else if a > b {
                b
            } else {
                a
            }
        }
        Excess => {
            let clipped = if a < -b {
                -b
            } else if a > b {
                b
            } else {
                a
            };
            a - clipped
        }
        // scsynth `sc_round`/`sc_trunc`: snap to a grid of step `b`; a zero step
        // is the identity (no grid).
        Round => {
            if b == 0.0 {
                a
            } else {
                (a / b + 0.5).floor() * b
            }
        }
        Trunc => {
            if b == 0.0 {
                a
            } else {
                (a / b).floor() * b
            }
        }
        // scsynth's bilateral `sc_fold2`/`sc_wrap2`: the ranged fold/wrap over
        // the symmetric interval `[-b, b]`.
        Fold2 => fold(a, -b, b),
        Wrap2 => wrap(a, -b, b),
        // scsynth truncates both operands to integers first, so `gcd(6.7, 4.2)`
        // is `gcd(6, 4)` = 2. The sign convention is scsynth's: negative only
        // when *both* operands are non-positive.
        Gcd => gcd_i64(a as i64, b as i64) as f32,
        Lcm => {
            let (x, y) = (a as i64, b as i64);
            if x == 0 || y == 0 {
                0.0
            } else {
                // scsynth computes `(x*y)/gcd`; dividing first keeps the same
                // value while pushing the overflow threshold far out, and the
                // saturation makes the worst case a finite number instead of a
                // debug-build panic — this runs on the audio thread.
                let g = gcd_i64(x, y);
                (x / g).saturating_mul(y) as f32
            }
        }
        // scsynth's `sc_hypotx`: `|a| + |b| - (sqrt(2) - 1)*min(|a|, |b|)`.
        // Deliberately approximate and always **greater than or equal to** the
        // true hypotenuse: exact on the axes (one operand zero), +12.1% on the
        // diagonal, and worst at `atan(2 - sqrt(2))` ~ 30.4 deg, where the
        // ratio is `sqrt(1 + (2 - sqrt(2))^2)` ~ +15.9%. (The diagonal is the
        // intuitive guess and it is not the maximum — the sweep in the tests
        // is what establishes the bound.)
        // Reproduced as scsynth defines it rather than "corrected", because the
        // whole contract of the operator is to be the cheap one and a def
        // ported from sclang must not change value. (scsynth's own comment
        // above the function describes a *different* quantity — the octagonal
        // distance `max + (sqrt(2)-1)*min` — which its formula does not
        // compute; the formula is what both implementations agree on.)
        HypotApx => {
            let (x, y) = (a.abs(), b.abs());
            x + y - SQRT2_MINUS_1 * x.min(y)
        }
    }
}

/// Applies a unary operator to a scalar.
#[inline]
pub fn apply_unary(op: UnaryOp, x: f32) -> f32 {
    use UnaryOp::*;
    match op {
        Neg => -x,
        Abs => x.abs(),
        Sine => x.sin(),
        Cos => x.cos(),
        Tan => x.tan(),
        Asin => x.asin(),
        Acos => x.acos(),
        Atan => x.atan(),
        Exp => x.exp(),
        Exp10 => 10.0f32.powf(x),
        Log => x.ln(),
        Log10 => x.log10(),
        Sqrt => x.sqrt(),
        Floor => x.floor(),
        Ceil => x.ceil(),
        Rint => x.round_ties_even(),
        IntCast => x as i32 as f32,
        FloatCast => x,
        Squared => x * x,
        Cubed => x * x * x,
        Recip => 1.0 / x,
        Frac => x - x.floor(),
        Sign => {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        }
        Log2 => x.log2(),
        Sinh => x.sinh(),
        Cosh => x.cosh(),
        Tanh => x.tanh(),
        // Music-theory conversions (12-TET, A4 = 440 Hz, decimal octave A440 =
        // 4.75). These are the single source of truth the client's off-RT
        // helpers and the server's `midi` path both consume, so they match to
        // the bit.
        Midicps => 440.0 * (2.0f32).powf((x - 69.0) / 12.0),
        Cpsmidi => 69.0 + 12.0 * (x / 440.0).log2(),
        Midiratio => (2.0f32).powf(x / 12.0),
        Ratiomidi => 12.0 * x.log2(),
        Dbamp => (10.0f32).powf(x * 0.05),
        Ampdb => x.log10() * 20.0,
        Octcps => 440.0 * (2.0f32).powf(x - 4.75),
        Cpsoct => (x * (1.0 / 440.0)).log2() + 4.75,
        Distort => x / (1.0 + x.abs()),
        Softclip => {
            let a = x.abs();
            if a <= 0.5 { x } else { (a - 0.25) / x }
        }
    }
}

/// Applies `f` element-wise over two broadcasting inputs.
///
/// The **broadcast shape is resolved once, before the loop**: each arm slices
/// its inputs to `out.len()` up front, so the body is a flat `f(x, y)` over
/// slices of statically equal length — no per-sample branch, no bounds check,
/// and a shape the autovectorizer can take. That is the whole point of this
/// helper; it is what [`at`] cannot give, since its branch is per sample.
///
/// A length other than 1 or `out.len()` panics, as indexing did before.
#[inline(always)]
fn map2<F: Fn(f32, f32) -> f32>(a: &[f32], b: &[f32], out: &mut [f32], f: F) {
    let n = out.len();
    if n == 0 {
        return;
    }
    match (a.len() == 1, b.len() == 1) {
        (true, true) => out.fill(f(a[0], b[0])),
        (true, false) => {
            let x = a[0];
            for (o, &y) in out.iter_mut().zip(&b[..n]) {
                *o = f(x, y);
            }
        }
        (false, true) => {
            let y = b[0];
            for (o, &x) in out.iter_mut().zip(&a[..n]) {
                *o = f(x, y);
            }
        }
        (false, false) => {
            for (o, (&x, &y)) in out.iter_mut().zip(a[..n].iter().zip(&b[..n])) {
                *o = f(x, y);
            }
        }
    }
}

/// [`map2`] with one input.
#[inline(always)]
fn map1<F: Fn(f32) -> f32>(a: &[f32], out: &mut [f32], f: F) {
    let n = out.len();
    if n == 0 {
        return;
    }
    if a.len() == 1 {
        out.fill(f(a[0]));
    } else {
        for (o, &x) in out.iter_mut().zip(&a[..n]) {
            *o = f(x);
        }
    }
}

/// `a*b + c` over broadcasting inputs — the fused multiply-accumulate, computed
/// as `add(mul(a, b), c)` so it is bit-identical to the two operators applied in
/// that order. Allocation-free.
///
/// The body is deliberately the naive one, [`at`] per sample and all. Hoisting
/// the broadcast shape out of it — one `const bool` per input, so the decision
/// is made at compile time — was written, measured and reverted: it is worth
/// 1.03-1.23x on the operator alone and **nothing measurable at the engine**,
/// because an arithmetic row is 5-10 ns of a block that spends ~135 ns in one
/// `Sine`. `docs/decisions.md` carries the figures. Anyone tempted again should
/// beat that measurement first; the operator match in [`binary_slice`], which
/// *was* worth hoisting, is the contrasting case.
#[inline]
pub fn mul_add_slice(a: &[f32], b: &[f32], c: &[f32], out: &mut [f32]) {
    for (i, s) in out.iter_mut().enumerate() {
        let prod = apply_binary(BinaryOp::Mul, at(a, i), at(b, i));
        *s = apply_binary(BinaryOp::Add, prod, at(c, i));
    }
}

/// `a + b + c` over broadcasting inputs, added left to right. Allocation-free.
#[inline]
pub fn sum3_slice(a: &[f32], b: &[f32], c: &[f32], out: &mut [f32]) {
    for (i, s) in out.iter_mut().enumerate() {
        let ab = apply_binary(BinaryOp::Add, at(a, i), at(b, i));
        *s = apply_binary(BinaryOp::Add, ab, at(c, i));
    }
}

/// `a + b + c + d` over broadcasting inputs, added left to right.
/// Allocation-free.
#[inline]
pub fn sum4_slice(a: &[f32], b: &[f32], c: &[f32], d: &[f32], out: &mut [f32]) {
    for (i, s) in out.iter_mut().enumerate() {
        let ab = apply_binary(BinaryOp::Add, at(a, i), at(b, i));
        let abc = apply_binary(BinaryOp::Add, ab, at(c, i));
        *s = apply_binary(BinaryOp::Add, abc, at(d, i));
    }
}

/// Generates `binary_slice`/`unary_slice` as a match that picks the operator
/// **once**, before the loop, and hands [`map2`]/[`map1`] a closure that is
/// monomorphic in it.
///
/// The closure still calls `apply_*`, so the scalar function stays the single
/// definition of every operator's arithmetic and the slice path cannot drift
/// from it; with the operator a constant at each call site it folds away, which
/// is what leaves the loop body flat. Written out by hand this would be one arm
/// per operator saying the same thing 77 times. A variant missing from the list
/// below fails to compile — the generated match is exhaustive.
macro_rules! slice_dispatch {
    (binary: $($b:ident),+ $(,)?; unary: $($u:ident),+ $(,)?) => {
        /// Broadcasting binary op over slices into `out` (length defines the
        /// frame count). Allocation-free.
        #[inline]
        pub fn binary_slice(op: BinaryOp, a: &[f32], b: &[f32], out: &mut [f32]) {
            match op {
                $(BinaryOp::$b => map2(a, b, out, |x, y| apply_binary(BinaryOp::$b, x, y)),)+
            }
        }

        /// Broadcasting unary op over a slice into `out`. Allocation-free.
        #[inline]
        pub fn unary_slice(op: UnaryOp, a: &[f32], out: &mut [f32]) {
            match op {
                $(UnaryOp::$u => map1(a, out, |x| apply_unary(UnaryOp::$u, x)),)+
            }
        }
    };
}

slice_dispatch! {
    binary:
        Add, Sub, Mul, Div, Mod, Pow, Min, Max, Atan2,
        Gt, Lt, Ge, Le, Eq, Ne,
        And, Or, Xor, Lsh, Rsh,
        Hypot, Ring1, Ring2, Ring3, Ring4,
        Sumsqr, Difsqr, Sqrsum, Sqrdif, Absdif,
        Thresh, Clip2, Excess, Round, Trunc, Fold2, Wrap2,
        Gcd, Lcm, HypotApx;
    unary:
        Neg, Abs, Sine, Cos, Tan, Asin, Acos, Atan,
        Exp, Exp10, Log, Log10, Log2, Sqrt,
        Floor, Ceil, Rint, IntCast, FloatCast,
        Squared, Cubed, Recip, Frac, Sign,
        Sinh, Cosh, Tanh,
        Midicps, Cpsmidi, Midiratio, Ratiomidi,
        Dbamp, Ampdb, Octcps, Cpsoct,
        Distort, Softclip,
}

/// Scale-degree → MIDI note number: `degree` indexes `scale` (semitone offsets
/// within one octave) in the pitch space `octave`/`root`, wrapping with octave
/// carry — degree −1 on a 7-note scale is the 7th one octave down (floored
/// division, sclang semantics). An empty `scale` yields middle C (60). The
/// event-value math every client's `Event` shares.
pub fn degree_to_midinote(degree: f64, octave: f64, root: f64, scale: &[f32]) -> f64 {
    let n = scale.len() as i64;
    if n == 0 {
        return 60.0;
    }
    let d = degree as i64;
    let step = scale[d.rem_euclid(n) as usize] as f64;
    12.0 * octave + root + step + 12.0 * d.div_euclid(n) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degree_wraps_with_octave_carry() {
        let major = [0.0f32, 2.0, 4.0, 5.0, 7.0, 9.0, 11.0];
        assert_eq!(degree_to_midinote(0.0, 5.0, 0.0, &major), 60.0);
        assert_eq!(degree_to_midinote(7.0, 5.0, 0.0, &major), 72.0); // next octave
        assert_eq!(degree_to_midinote(-1.0, 5.0, 0.0, &major), 59.0); // 7th, down
        assert_eq!(degree_to_midinote(1.0, 5.0, 2.0, &major), 64.0); // root shift
        assert_eq!(degree_to_midinote(3.0, 5.0, 0.0, &[]), 60.0);
    }

    #[test]
    fn arithmetic_matches_the_server_binop() {
        // The exact expressions dsp::binop computes, kept here as the contract.
        let (x, y) = (1.5f32, -0.25f32);
        assert_eq!(apply_binary(BinaryOp::Add, x, y), x + y);
        assert_eq!(apply_binary(BinaryOp::Sub, x, y), x - y);
        assert_eq!(apply_binary(BinaryOp::Mul, x, y), x * y);
        assert_eq!(apply_binary(BinaryOp::Div, x, y), x / y);
    }

    #[test]
    fn comparisons_are_zero_or_one() {
        assert_eq!(apply_binary(BinaryOp::Gt, 2.0, 1.0), 1.0);
        assert_eq!(apply_binary(BinaryOp::Gt, 1.0, 2.0), 0.0);
        assert_eq!(apply_binary(BinaryOp::Eq, 1.0, 1.0), 1.0);
    }

    #[test]
    fn unary_round_ties_to_even() {
        assert_eq!(apply_unary(UnaryOp::Rint, 0.5), 0.0);
        assert_eq!(apply_unary(UnaryOp::Rint, 1.5), 2.0);
        assert_eq!(apply_unary(UnaryOp::Rint, 2.5), 2.0);
    }

    #[test]
    fn slice_ops_broadcast_a_constant() {
        let a = [2.0f32]; // length-1: a broadcast constant
        let b = [10.0f32, 20.0, 30.0];
        let mut out = [0.0f32; 3];
        binary_slice(BinaryOp::Add, &a, &b, &mut out);
        assert_eq!(out, [12.0, 22.0, 32.0]);
    }

    #[test]
    fn slice_ops_are_the_scalar_ops_element_by_element() {
        // The invariant the dispatch rests on: hoisting the operator match and
        // the broadcast shape out of the loop must not move a single bit. The
        // reference here is the naive formulation the loops used to be —
        // `apply_*` under `at` — checked bit-exactly (`to_bits`, so a NaN must
        // be the *same* NaN) over every operator, every broadcast shape, and
        // operands that reach the edge cases: zero and signed zero, negatives
        // (`Pow`, `Sqrt`, `Log`), the shift and gcd integer casts, infinities
        // and NaN itself.
        let vals = [
            0.0f32,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -2.5,
            3.0,
            1e-30,
            1e30,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
        ];
        let n = vals.len();
        let bin = (0..40).map(|v| BinaryOp::from_u32(v).unwrap());
        for op in bin {
            for shape in 0..4 {
                let (a, b) = match shape {
                    0 => (vals.to_vec(), vals.to_vec()),
                    1 => (vec![vals[5]], vals.to_vec()),
                    2 => (vals.to_vec(), vec![vals[5]]),
                    _ => (vec![vals[3]], vec![vals[5]]),
                };
                let mut got = vec![0.0f32; n];
                binary_slice(op, &a, &b, &mut got);
                for (i, g) in got.iter().enumerate() {
                    let want = apply_binary(op, at(&a, i), at(&b, i));
                    assert_eq!(
                        g.to_bits(),
                        want.to_bits(),
                        "{op:?} shape {shape} frame {i}: {g} != {want}"
                    );
                }
            }
        }
        for op in (0..37).map(|v| UnaryOp::from_u32(v).unwrap()) {
            for a in [vals.to_vec(), vec![vals[5]]] {
                let mut got = vec![0.0f32; n];
                unary_slice(op, &a, &mut got);
                for (i, g) in got.iter().enumerate() {
                    let want = apply_unary(op, at(&a, i));
                    assert_eq!(g.to_bits(), want.to_bits(), "{op:?} frame {i}");
                }
            }
        }
    }

    #[test]
    fn fused_ops_are_the_scalar_ops_element_by_element() {
        // Pins the fused rows' **contract** over every broadcast shape: the
        // operand order (`add(mul(a,b),c)`, sums left to right) and `at`'s
        // broadcasting. Float addition does not associate, so a reordering
        // shows up here bit-exactly.
        //
        // Today the bodies *are* this formulation, so the assert is a
        // restatement — deliberately. It is the harness for the next attempt at
        // rewriting them (the reverted broadcast hoist split on exactly these
        // sixteen shapes), which is when a restatement becomes a test.
        let vals = [0.0f32, -0.0, 1.5, -2.5, 1e30, f32::INFINITY, f32::NAN, 0.25];
        let n = vals.len();
        let pick =
            |k: bool, off: usize| -> Vec<f32> { if k { vec![vals[off]] } else { vals.to_vec() } };
        for shape in 0..16u32 {
            let (ka, kb, kc, kd) = (
                shape & 1 != 0,
                shape & 2 != 0,
                shape & 4 != 0,
                shape & 8 != 0,
            );
            let (a, b, c, d) = (pick(ka, 2), pick(kb, 3), pick(kc, 7), pick(kd, 2));
            let mut got = vec![0.0f32; n];

            mul_add_slice(&a, &b, &c, &mut got);
            for (i, g) in got.iter().enumerate() {
                let prod = apply_binary(BinaryOp::Mul, at(&a, i), at(&b, i));
                let want = apply_binary(BinaryOp::Add, prod, at(&c, i));
                assert_eq!(
                    g.to_bits(),
                    want.to_bits(),
                    "MulAdd shape {shape} frame {i}"
                );
            }

            sum3_slice(&a, &b, &c, &mut got);
            for (i, g) in got.iter().enumerate() {
                let ab = apply_binary(BinaryOp::Add, at(&a, i), at(&b, i));
                let want = apply_binary(BinaryOp::Add, ab, at(&c, i));
                assert_eq!(g.to_bits(), want.to_bits(), "Sum3 shape {shape} frame {i}");
            }

            sum4_slice(&a, &b, &c, &d, &mut got);
            for (i, g) in got.iter().enumerate() {
                let ab = apply_binary(BinaryOp::Add, at(&a, i), at(&b, i));
                let abc = apply_binary(BinaryOp::Add, ab, at(&c, i));
                let want = apply_binary(BinaryOp::Add, abc, at(&d, i));
                assert_eq!(g.to_bits(), want.to_bits(), "Sum4 shape {shape} frame {i}");
            }
        }
    }

    #[test]
    fn slice_ops_accept_an_empty_frame_count() {
        // Reachable from the C ABI (`n == 0` with length-1 or empty inputs) and
        // a no-op there before the shapes were hoisted; it must stay one.
        let mut out: [f32; 0] = [];
        binary_slice(BinaryOp::Add, &[], &[], &mut out);
        unary_slice(UnaryOp::Neg, &[], &mut out);
        mul_add_slice(&[], &[], &[], &mut out);
        sum3_slice(&[], &[], &[], &mut out);
        sum4_slice(&[], &[], &[], &[], &mut out);
    }

    #[test]
    fn enum_discriminants_round_trip() {
        // Contiguous and stable: every discriminant maps back to itself, and
        // one past the end is unknown (guards an accidental gap).
        for v in 0..=39u32 {
            assert_eq!(BinaryOp::from_u32(v).map(|o| o as u32), Some(v));
        }
        assert_eq!(BinaryOp::from_u32(40), None);
        for v in 0..=36u32 {
            assert_eq!(UnaryOp::from_u32(v).map(|o| o as u32), Some(v));
        }
        assert_eq!(UnaryOp::from_u32(37), None);
    }

    #[test]
    fn operator_names_round_trip() {
        // Every operator's wire name resolves back to it, and names are unique.
        let mut seen = std::collections::HashSet::new();
        for op in (0..).map_while(BinaryOp::from_u32) {
            assert_eq!(BinaryOp::from_name(op.name()), Some(op));
            assert!(
                seen.insert(op.name()),
                "duplicate binary name {}",
                op.name()
            );
        }
        for op in (0..).map_while(UnaryOp::from_u32) {
            assert_eq!(UnaryOp::from_name(op.name()), Some(op));
            assert!(seen.insert(op.name()), "duplicate unary name {}", op.name());
        }
        assert_eq!(BinaryOp::from_name("nope"), None);
        assert_eq!(UnaryOp::from_name("nope"), None);
    }

    #[test]
    fn extended_binary_ops() {
        use BinaryOp::*;
        assert_eq!(apply_binary(Hypot, 3.0, 4.0), 5.0);
        assert_eq!(apply_binary(Ring1, 2.0, 3.0), 2.0 * 3.0 + 2.0);
        assert_eq!(apply_binary(Sumsqr, 2.0, 3.0), 13.0);
        assert_eq!(apply_binary(Sqrsum, 2.0, 3.0), 25.0);
        assert_eq!(apply_binary(Absdif, 2.0, 5.0), 3.0);
        assert_eq!(apply_binary(Thresh, 0.3, 0.5), 0.0);
        assert_eq!(apply_binary(Thresh, 0.7, 0.5), 0.7);
        assert_eq!(apply_binary(Clip2, 5.0, 1.0), 1.0);
        assert_eq!(apply_binary(Clip2, -5.0, 1.0), -1.0);
        assert_eq!(apply_binary(Excess, 5.0, 1.0), 4.0);
        assert_eq!(apply_binary(Round, 1.2, 0.5), 1.0);
        assert_eq!(apply_binary(Round, 1.3, 0.5), 1.5);
        assert_eq!(apply_binary(Trunc, 1.9, 1.0), 1.0);
        assert_eq!(apply_binary(Round, 1.7, 0.0), 1.7); // zero step = identity
    }

    /// `fold2` reflects at both ends of `[-b, b]`, as many times as needed —
    /// the values below are hand-unfolded, not captured from the code.
    #[test]
    fn fold2_reflects_repeatedly() {
        use BinaryOp::Fold2;
        assert_eq!(apply_binary(Fold2, 0.5, 1.0), 0.5); // inside, untouched
        assert_eq!(apply_binary(Fold2, 1.5, 1.0), 0.5); // one reflection at +1
        assert_eq!(apply_binary(Fold2, -1.5, 1.0), -0.5); // one at -1
        assert_eq!(apply_binary(Fold2, 2.5, 1.0), -0.5); // 2.5 -> -0.5
        // 3.5 -> reflect at +1 -> -1.5 -> reflect at -1 -> -0.5.
        assert_eq!(apply_binary(Fold2, 3.5, 1.0), -0.5);
        assert_eq!(apply_binary(Fold2, 4.5, 1.0), 0.5);
        // The fold is symmetric, and the fixed points are the bounds.
        assert_eq!(apply_binary(Fold2, 1.0, 1.0), 1.0);
        assert_eq!(apply_binary(Fold2, -1.0, 1.0), -1.0);
    }

    /// `wrap2` is modulo into the half-open `[-b, b)`, never reflecting.
    #[test]
    fn wrap2_is_modulo_not_reflection() {
        use BinaryOp::Wrap2;
        assert_eq!(apply_binary(Wrap2, 0.5, 1.0), 0.5);
        assert_eq!(apply_binary(Wrap2, 1.5, 1.0), -0.5); // one period down
        assert_eq!(apply_binary(Wrap2, -1.5, 1.0), 0.5); // one period up
        assert_eq!(apply_binary(Wrap2, 3.5, 1.0), -0.5); // two periods down
        assert_eq!(apply_binary(Wrap2, -3.5, 1.0), 0.5);
        // The range is half-open: the upper bound wraps, the lower does not.
        assert_eq!(apply_binary(Wrap2, 1.0, 1.0), -1.0);
        assert_eq!(apply_binary(Wrap2, -1.0, 1.0), -1.0);
    }

    /// `gcd`/`lcm` truncate to integers first and carry scsynth's sign rule:
    /// the result is negative only when **both** operands are non-positive.
    #[test]
    fn gcd_and_lcm_truncate_and_keep_scsynth_signs() {
        use BinaryOp::{Gcd, Lcm};
        assert_eq!(apply_binary(Gcd, 6.0, 4.0), 2.0);
        assert_eq!(apply_binary(Gcd, 6.7, 4.2), 2.0); // truncated to 6 and 4
        assert_eq!(apply_binary(Gcd, -6.0, 4.0), 2.0); // one negative: positive
        assert_eq!(apply_binary(Gcd, -6.0, -4.0), -2.0); // both: negative
        assert_eq!(apply_binary(Gcd, 0.0, 5.0), 5.0); // a zero passes the other
        assert_eq!(apply_binary(Gcd, 7.0, 1.0), 1.0); // coprime
        assert_eq!(apply_binary(Gcd, 17.0, 5.0), 1.0);
        assert_eq!(apply_binary(Gcd, 48.0, 18.0), 6.0);

        assert_eq!(apply_binary(Lcm, 4.0, 6.0), 12.0);
        assert_eq!(apply_binary(Lcm, 0.0, 6.0), 0.0);
        assert_eq!(apply_binary(Lcm, -4.0, 6.0), -12.0);
        assert_eq!(apply_binary(Lcm, -4.0, -6.0), -12.0);
        // The identity that defines the pair, on a case with a shared factor.
        for (a, b) in [(12.0f32, 18.0f32), (35.0, 21.0), (9.0, 28.0)] {
            let g = apply_binary(Gcd, a, b);
            let l = apply_binary(Lcm, a, b);
            assert_eq!(g * l, a * b, "gcd*lcm == a*b for ({a}, {b})");
        }
    }

    /// `hypot_apx` is the cheap approximation, reproduced from scsynth's
    /// formula. It never under-estimates; its error is 0 on the axes, +12.1 %
    /// on the diagonal, and peaks at +15.9 % near 30.4 deg — the maximum is
    /// *not* on the diagonal, which is why the bound is swept rather than
    /// assumed.
    #[test]
    fn hypot_apx_error_stays_within_its_documented_bound() {
        use BinaryOp::HypotApx;
        assert_eq!(apply_binary(HypotApx, 3.0, 0.0), 3.0); // exact on an axis
        assert_eq!(apply_binary(HypotApx, 0.0, -4.0), 4.0); // and symmetric
        // 3, 4: 7 - (sqrt(2)-1)*3.
        let k = (core::f64::consts::SQRT_2 - 1.0) as f32;
        assert!((apply_binary(HypotApx, 3.0, 4.0) - (7.0 - k * 3.0)).abs() < 1e-6);
        // The diagonal, where the intuitive worst case sits: 2 - (sqrt(2)-1).
        assert!((apply_binary(HypotApx, 1.0, 1.0) / 2.0f32.sqrt() - 1.1213).abs() < 1e-4);

        // Sweep a quadrant on the unit circle, where the true hypotenuse is 1:
        // check the one-sided claim everywhere and locate the actual maximum.
        // Analytically the ratio is `cos t + (2 - sqrt(2)) sin t`, maximal at
        // `tan t = 2 - sqrt(2)` with value `sqrt(1 + (2 - sqrt(2))^2)`.
        let beta = 2.0 - core::f32::consts::SQRT_2;
        let peak = (1.0 + beta * beta).sqrt();
        let (mut worst, mut worst_deg) = (0.0f32, 0.0f32);
        for i in 0..=9000 {
            let deg = i as f32 / 100.0;
            let (y, x) = deg.to_radians().sin_cos();
            let ratio = apply_binary(HypotApx, x, y) / x.hypot(y);
            assert!(ratio >= 1.0 - 1e-6, "under-estimated at {deg} deg: {ratio}");
            if ratio - 1.0 > worst {
                (worst, worst_deg) = (ratio - 1.0, deg);
            }
        }
        assert!(
            (worst - (peak - 1.0)).abs() < 1e-4,
            "measured worst error {worst} != analytic {}",
            peak - 1.0
        );
        assert!(
            (worst_deg - beta.atan().to_degrees()).abs() < 0.05,
            "worst case at {worst_deg} deg, expected {} deg",
            beta.atan().to_degrees()
        );
    }

    #[test]
    fn extended_unary_ops() {
        use UnaryOp::*;
        assert_eq!(apply_unary(Squared, 3.0), 9.0);
        assert_eq!(apply_unary(Cubed, 2.0), 8.0);
        assert_eq!(apply_unary(Recip, 4.0), 0.25);
        assert_eq!(apply_unary(Sign, -2.0), -1.0);
        assert_eq!(apply_unary(Sign, 0.0), 0.0);
        // A4 round-trips through the pitch conversions.
        assert!((apply_unary(Midicps, 69.0) - 440.0).abs() < 1e-3);
        assert!((apply_unary(Cpsmidi, 440.0) - 69.0).abs() < 1e-4);
        assert!((apply_unary(Midiratio, 12.0) - 2.0).abs() < 1e-5);
        assert!((apply_unary(Dbamp, 0.0) - 1.0).abs() < 1e-6);
        assert!((apply_unary(Ampdb, 1.0)).abs() < 1e-4);
        // Distort/softclip stay bounded and pass small inputs through.
        assert_eq!(apply_unary(Softclip, 0.25), 0.25);
        assert!(apply_unary(Distort, 100.0) < 1.0);
    }
}
