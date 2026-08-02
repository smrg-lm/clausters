//! The filter core: one topology-preserving state-variable filter behind
//! every two-pole row, plus the one-pole family.
//!
//! **Why a state-variable filter and not a biquad.** scsynth realizes `LPF`,
//! `HPF`, `BPF`, `BRF`, `RLPF`, `RHPF` and `Resonz` as direct-form two-pole
//! sections, each with its own coefficient formula. This module implements the
//! *same* prototype — the bilinear transform of the analog two-pole — through
//! trapezoidal integrators instead, which changes nothing about the transfer
//! function and three things about the behaviour:
//!
//! 1. It does not blow up under audio-rate cutoff modulation. A direct-form
//!    section with interpolated coefficients can, because its state has no
//!    physical meaning between two coefficient sets; an integrator's state is
//!    the signal it has integrated, whatever the coefficients do next.
//! 2. It is far better conditioned at low cutoff, where the poles crowd `z = 1`.
//! 3. Lowpass, bandpass, highpass, notch and peak all fall out of the **same**
//!    two integrator updates as a linear mix of three taps — which is what lets
//!    one implementation carry eight scsynth names, and what makes [`SvfMode::Mix`]
//!    (a filter whose response is a signal) cost the mix and nothing else.
//!
//! **Precision.** State and coefficients are `f64`, matching scsynth's own
//! choice for exactly the same reason: at low cutoff, `f32` state truncation and
//! coefficient quantization dominate the output. Wires stay `f32`; the
//! conversion happens at the edges of `process`.
//!
//! **Coefficient rate.** The `tan` and the reciprocal that turn a cutoff into
//! integrator gains run **once per block** when the parameters arrive as scalar
//! wires. When either is audio-rate, they run twice — at the block's first and
//! last sample — and the three gains are interpolated linearly in between. That
//! is scsynth's `CALCSLOPE` idea applied one level later: interpolating the
//! *gains* rather than the cutoff avoids both a `tan` and a division per sample,
//! leaving three multiply-adds.

use std::f64::consts::PI;

use crate::dsp::{ProcessCtx, UGen, at};

/// Which linear combination of the filter's three taps leaves the UGen.
///
/// The taps are the same in every case — one pair of integrator updates — so a
/// mode costs nothing beyond its own mix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SvfMode {
    /// Lowpass, damping fixed at `sqrt(2)` (Butterworth): `LPF`.
    Lp,
    /// Highpass, Butterworth: `HPF`.
    Hp,
    /// Lowpass with the damping from input 2: `RLPF`.
    RLp,
    /// Highpass with the damping from input 2: `RHPF`.
    RHp,
    /// Bandpass normalized to **unity gain at the centre**: `BPF`, `Resonz`.
    Bp,
    /// Band reject / notch: `BRF`.
    Notch,
    /// The three tap gains are signal inputs 3, 4 and 5: `Svf`.
    Mix,
}

impl SvfMode {
    /// Wire inputs this mode reads past `in` and `freq`.
    fn extra_inputs(self) -> usize {
        match self {
            SvfMode::Lp | SvfMode::Hp => 0,
            SvfMode::Mix => 4,
            _ => 1,
        }
    }
}

/// Damping for a Butterworth response: `1/Q` with `Q = 1/sqrt(2)`.
const BUTTERWORTH_K: f64 = std::f64::consts::SQRT_2;

/// The integrator gains for one `(cutoff, damping)` pair.
#[derive(Clone, Copy)]
struct Coeffs {
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
}

impl Coeffs {
    /// `g = tan(pi*fc/sr)` is the trapezoidal integrator's gain — the bilinear
    /// transform's frequency pre-warping, which is what makes the digital
    /// cutoff land exactly on `fc` rather than near it.
    ///
    /// The cutoff is clamped to `[10 Hz, 0.49*sr]`: `tan` diverges at Nyquist,
    /// and below a few Hz the filter is a DC offset with a very long memory
    /// rather than anything musical. A damping of `0` is **not** clamped away —
    /// it is infinite Q, it is representable here without dividing by anything,
    /// and it is the reason the wire carries `rq` rather than `Q`.
    fn new(fc: f32, k: f64, sr: f32) -> Self {
        let fc = (fc as f64).clamp(10.0, sr as f64 * 0.49);
        let g = (PI * fc / sr as f64).tan();
        let k = k.max(0.0);
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        Self { a1, a2, a3, k }
    }

    #[inline]
    fn lerp(self, other: Self, t: f64) -> Self {
        Self {
            a1: self.a1 + (other.a1 - self.a1) * t,
            a2: self.a2 + (other.a2 - self.a2) * t,
            a3: self.a3 + (other.a3 - self.a3) * t,
            k: self.k + (other.k - self.k) * t,
        }
    }
}

/// A topology-preserving (trapezoidal-integrator) state-variable filter.
///
/// Inputs: 0 the signal, 1 the cutoff in Hz, then whatever the mode reads —
/// nothing for the Butterworth pair, `rq` for the resonant ones, and
/// `rq, low, band, high` for [`SvfMode::Mix`].
pub struct Svf {
    mode: SvfMode,
    /// The two integrator states. `f64` for the reason in the module docs.
    ic1: f64,
    ic2: f64,
}

impl Svf {
    pub fn new(mode: SvfMode) -> Self {
        Self {
            mode,
            ic1: 0.0,
            ic2: 0.0,
        }
    }

    /// One sample through the two integrators, returning the three taps
    /// `(lowpass, bandpass, highpass)`.
    ///
    /// The bandpass tap is the raw integrator output, whose gain at the centre
    /// is `Q` — the standard state-variable convention. The modes that want
    /// unity there scale it by `k`.
    #[inline]
    fn step(&mut self, v0: f64, c: &Coeffs) -> (f64, f64, f64) {
        let v3 = v0 - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, v0 - c.k * v1 - v2)
    }
}

/// The per-sample parameters a mode needs beyond the signal.
#[derive(Clone, Copy)]
struct Mix {
    low: f64,
    band: f64,
    high: f64,
}

impl SvfMode {
    /// How this mode weights `(lowpass, bandpass, highpass)` for a damping `k`.
    #[inline]
    fn mix(self, k: f64, from_inputs: Mix) -> Mix {
        match self {
            SvfMode::Lp | SvfMode::RLp => Mix {
                low: 1.0,
                band: 0.0,
                high: 0.0,
            },
            SvfMode::Hp | SvfMode::RHp => Mix {
                low: 0.0,
                band: 0.0,
                high: 1.0,
            },
            // The raw bandpass tap peaks at `Q`; scaling by `k = 1/Q` puts the
            // centre back at unity, which is what both `BPF` and `Resonz`
            // promise.
            SvfMode::Bp => Mix {
                low: 0.0,
                band: k,
                high: 0.0,
            },
            // A notch is the sum of the two skirts.
            SvfMode::Notch => Mix {
                low: 1.0,
                band: 0.0,
                high: 1.0,
            },
            SvfMode::Mix => from_inputs,
        }
    }
}

impl UGen for Svf {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate;
        let (sig, freq) = (inputs[0], inputs[1]);
        let extra = self.mode.extra_inputs();
        // Butterworth rows have no `rq` wire; the rest read it from input 2.
        let rq = if extra == 0 { None } else { Some(inputs[2]) };
        let k_of = |i: usize| match rq {
            None => BUTTERWORTH_K,
            Some(r) => at(r, i) as f64,
        };
        let n = output.len();
        if n == 0 {
            return;
        }

        // Block-constant parameters take the fast path: one `tan`, one
        // reciprocal, no interpolation.
        let scalar = freq.len() == 1 && rq.is_none_or(|r| r.len() == 1);
        let first = Coeffs::new(at(freq, 0), k_of(0), sr);
        let last = if scalar {
            first
        } else {
            Coeffs::new(at(freq, n - 1), k_of(n - 1), sr)
        };
        // Reciprocal of the span, so the ramp is a multiply rather than a
        // divide per sample.
        let inv = if n > 1 { 1.0 / (n - 1) as f64 } else { 0.0 };

        // Only the mix row carries tap gains; every other mode's are constants.
        let taps = (self.mode == SvfMode::Mix).then(|| (inputs[3], inputs[4], inputs[5]));
        const NO_TAPS: Mix = Mix {
            low: 0.0,
            band: 0.0,
            high: 0.0,
        };

        for (i, s) in output.iter_mut().enumerate() {
            let c = if scalar {
                first
            } else {
                first.lerp(last, i as f64 * inv)
            };
            let (lp, bp, hp) = self.step(at(sig, i) as f64, &c);
            let from_inputs = match taps {
                Some((l, b, h)) => Mix {
                    low: at(l, i) as f64,
                    band: at(b, i) as f64,
                    high: at(h, i) as f64,
                },
                None => NO_TAPS,
            };
            let m = self.mode.mix(c.k, from_inputs);
            *s = (m.low * lp + m.band * bp + m.high * hp) as f32;
        }
    }
}

/// The single-state filters: `OnePole`, `OneZero`, `LeakDC`, `Integrator`.
///
/// All four take a **coefficient**, not a frequency — scsynth's contract, and
/// the honest one: their pole is the parameter, and naming it a cutoff would
/// promise a `-3 dB` point the one-pole shape does not have in the same sense a
/// two-pole one does. Use [`Lag`](super::lag::Lag) when what you want is a time
/// constant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OneKind {
    /// `y[n] = (1 - |c|)*x[n] + c*y[n-1]` — lowpass for `c > 0`, highpass for
    /// `c < 0`; the leading factor keeps the passband at unity.
    OnePole,
    /// `y[n] = (1 - |c|)*x[n] + c*x[n-1]` — the zero-only sibling.
    OneZero,
    /// `y[n] = x[n] - x[n-1] + c*y[n-1]` — a DC blocker: a zero exactly at DC
    /// with a pole just inside it.
    LeakDc,
    /// `y[n] = x[n] + c*y[n-1]` — a leaky accumulator. The coefficient is
    /// clamped just inside `1`, so the leakiest setting still forgets rather
    /// than growing without bound: a true integrator fed any DC at all reaches
    /// infinity, and it would do so on the audio thread.
    Integrator,
}

/// One-pole / one-zero sections, state in `f64`.
pub struct OneFilter {
    kind: OneKind,
    y1: f64,
    x1: f64,
}

impl OneFilter {
    pub fn new(kind: OneKind) -> Self {
        Self {
            kind,
            y1: 0.0,
            x1: 0.0,
        }
    }
}

impl UGen for OneFilter {
    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let _ = ctx;
        let (sig, coef) = (inputs[0], inputs[1]);
        for (i, s) in output.iter_mut().enumerate() {
            let x = at(sig, i) as f64;
            // A coefficient at or past the unit circle is unstable; clamping
            // just inside keeps a mistyped control from producing NaN forever.
            let c = (at(coef, i) as f64).clamp(-0.999_999, 0.999_999);
            let y = match self.kind {
                OneKind::OnePole => (1.0 - c.abs()) * x + c * self.y1,
                OneKind::OneZero => (1.0 - c.abs()) * x + c * self.x1,
                OneKind::LeakDc => x - self.x1 + c * self.y1,
                OneKind::Integrator => x + c * self.y1,
            };
            self.x1 = x;
            self.y1 = y;
            *s = y as f32;
        }
    }
}
