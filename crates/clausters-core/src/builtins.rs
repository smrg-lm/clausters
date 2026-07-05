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
    Sin = 2,
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
            2 => Sin,
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
            Sin => "sin",
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
    }
}

/// Applies a unary operator to a scalar.
#[inline]
pub fn apply_unary(op: UnaryOp, x: f32) -> f32 {
    use UnaryOp::*;
    match op {
        Neg => -x,
        Abs => x.abs(),
        Sin => x.sin(),
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

/// Broadcasting binary op over slices into `out` (length defines the frame
/// count). Allocation-free.
#[inline]
pub fn binary_slice(op: BinaryOp, a: &[f32], b: &[f32], out: &mut [f32]) {
    for (i, s) in out.iter_mut().enumerate() {
        *s = apply_binary(op, at(a, i), at(b, i));
    }
}

/// Broadcasting unary op over a slice into `out`. Allocation-free.
#[inline]
pub fn unary_slice(op: UnaryOp, a: &[f32], out: &mut [f32]) {
    for (i, s) in out.iter_mut().enumerate() {
        *s = apply_unary(op, at(a, i));
    }
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
    fn enum_discriminants_round_trip() {
        // Contiguous and stable: every discriminant maps back to itself, and
        // one past the end is unknown (guards an accidental gap).
        for v in 0..=34u32 {
            assert_eq!(BinaryOp::from_u32(v).map(|o| o as u32), Some(v));
        }
        assert_eq!(BinaryOp::from_u32(35), None);
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
