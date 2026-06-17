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
}

impl BinaryOp {
    /// Maps a C-ABI discriminant back to the enum.
    pub fn from_u32(v: u32) -> Option<BinaryOp> {
        use BinaryOp::*;
        Some(match v {
            0 => Add, 1 => Sub, 2 => Mul, 3 => Div, 4 => Mod, 5 => Pow,
            6 => Min, 7 => Max, 8 => Atan2, 9 => Gt, 10 => Lt, 11 => Ge,
            12 => Le, 13 => Eq, 14 => Ne, 15 => And, 16 => Or, 17 => Xor,
            18 => Lsh, 19 => Rsh,
            _ => return None,
        })
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
}

impl UnaryOp {
    pub fn from_u32(v: u32) -> Option<UnaryOp> {
        use UnaryOp::*;
        Some(match v {
            0 => Neg, 1 => Abs, 2 => Sin, 3 => Cos, 4 => Tan, 5 => Asin,
            6 => Acos, 7 => Atan, 8 => Exp, 9 => Exp10, 10 => Log, 11 => Log10,
            12 => Sqrt, 13 => Floor, 14 => Ceil, 15 => Rint, 16 => IntCast,
            17 => FloatCast,
            _ => return None,
        })
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
        Min => if a < b { a } else { b },
        Max => if a > b { a } else { b },
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

#[cfg(test)]
mod tests {
    use super::*;

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
        for v in 0..=19u32 {
            assert_eq!(BinaryOp::from_u32(v).map(|o| o as u32), Some(v));
        }
        for v in 0..=17u32 {
            assert_eq!(UnaryOp::from_u32(v).map(|o| o as u32), Some(v));
        }
    }
}
