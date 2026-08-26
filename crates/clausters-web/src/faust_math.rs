//! The transcendentals a Faust wasm module imports, exported by the engine.
//!
//! A module the Faust wasm backend emits imports what the instruction set
//! lacks — `env._sinf`, `env._powf`, `env._fmodf` and their neighbours — and a
//! wasm function exported by one instance is a legal import of another. So the
//! page binds them to **these**, not to `Math.sin` closures: no JavaScript
//! frame on the audio path, and Faust and our own UGens go through one libm,
//! which is what keeps a parity vector between the browser and a window
//! meaningful.
//!
//! The set is Faust's own, verbatim from its `registerMathFuns` (the table its
//! native wasm host installs): unary and binary, `f32` and `f64`, plus the one
//! integer `_abs`. Exporting more than a given module imports costs nothing —
//! a missing one is what a page would hear as silence.
//!
//! These exist only in the wasm build: natively the engine links libfaust and
//! Faust's own code calls the platform's libm directly.

macro_rules! unary {
    ($($name:ident: $ty:ty = $method:ident;)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(x: $ty) -> $ty {
            x.$method()
        }
    )*};
}

macro_rules! binary {
    ($($name:ident: $ty:ty = $body:expr;)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(a: $ty, b: $ty) -> $ty {
            let f: fn($ty, $ty) -> $ty = $body;
            f(a, b)
        }
    )*};
}

unary! {
    _sinf: f32 = sin;      _cosf: f32 = cos;      _tanf: f32 = tan;
    _asinf: f32 = asin;    _acosf: f32 = acos;    _atanf: f32 = atan;
    _expf: f32 = exp;      _logf: f32 = ln;       _log10f: f32 = log10;
    _roundf: f32 = round;  _sinhf: f32 = sinh;    _coshf: f32 = cosh;
    _tanhf: f32 = tanh;    _asinhf: f32 = asinh;  _acoshf: f32 = acosh;
    _atanhf: f32 = atanh;

    _sin: f64 = sin;       _cos: f64 = cos;       _tan: f64 = tan;
    _asin: f64 = asin;     _acos: f64 = acos;     _atan: f64 = atan;
    _exp: f64 = exp;       _log: f64 = ln;        _log10: f64 = log10;
    _round: f64 = round;   _sinh: f64 = sinh;     _cosh: f64 = cosh;
    _tanh: f64 = tanh;     _asinh: f64 = asinh;   _acosh: f64 = acosh;
    _atanh: f64 = atanh;
}

binary! {
    _atan2f: f32 = |a, b| a.atan2(b);
    // Rust's `%` on floats is C's `fmod`: the sign follows the dividend.
    _fmodf: f32 = |a, b| a % b;
    _powf: f32 = |a, b| a.powf(b);
    // IEEE `remainder`, which is *not* `fmod`: the quotient rounds to nearest
    // with ties to even, so the result can be negative where `fmod` is not.
    _remainderf: f32 = |a, b| a - b * (a / b).round_ties_even();

    _atan2: f64 = |a, b| a.atan2(b);
    _fmod: f64 = |a, b| a % b;
    _pow: f64 = |a, b| a.powf(b);
    _remainder: f64 = |a, b| a - b * (a / b).round_ties_even();
}

/// Faust's one integer import. `i32::MIN` has no positive counterpart, and C's
/// `abs` leaves that case undefined; wrapping keeps it a total function
/// instead of a trap in the middle of a block.
#[unsafe(no_mangle)]
pub extern "C" fn _abs(x: i32) -> i32 {
    x.wrapping_abs()
}
