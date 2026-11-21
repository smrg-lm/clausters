//! The filter core: one topology-preserving state-variable filter behind
//! every two-pole row, plus the one-pole family.
//!
//! **What each row computes is scsynth's.** scsynth realizes `LPF`, `HPF`,
//! `BPF`, `BRF`, `RLPF`, `RHPF` and `Resonz` as direct-form two-pole sections,
//! each with its own coefficient formula, and those formulas are the rows'
//! definitions: `RLPF` and `RHPF` are not the bilinear transform of the analog
//! resonant pair, and `Resonz` is a different resonator from `BPF`. Each row
//! here computes scsynth's own coefficients ([`Biquad`]) and then realizes that
//! transfer function through trapezoidal integrators instead of a direct form
//! ([`Biquad::svf`]). Every two-pole digital filter is the bilinear transform
//! of exactly one analog two-pole, and a state-variable filter realizes that
//! analog section exactly, so the response is the same function, not an
//! approximation of it. What the realization changes is behavior, not
//! response:
//!
//! 1. It does not blow up under audio-rate cutoff modulation. A direct-form
//!    section with interpolated coefficients can, because its state has no
//!    physical meaning between two coefficient sets; an integrator's state is
//!    the signal it has integrated, whatever the coefficients do next.
//! 2. It is far better conditioned at low cutoff, where the poles crowd `z = 1`.
//! 3. Lowpass, bandpass and highpass fall out of the **same** two integrator
//!    updates as a linear mix of three taps, so every row is a mix of them and
//!    `Svf` (a filter whose response is a signal) costs the mix and nothing
//!    else.
//!
//! **Precision.** State and coefficients are `f64`, matching scsynth's own
//! choice for exactly the same reason: at low cutoff, `f32` state truncation and
//! coefficient quantization dominate the output. Wires stay `f32`; the
//! conversion happens at the edges of `process`.
//!
//! **Coefficient rate.** The coefficients are computed once per slice, from
//! the parameters at its last sample, and the integrator gains and tap mix
//! move in a straight line from where the last slice left them -- scsynth's
//! `CALCSLOPE`, which is what keeps a control-rate sweep from stepping at the
//! block rate. Interpolating the *gains* rather than the cutoff avoids a `tan`
//! and a division per sample, leaving three multiply-adds and the mix.

use std::f64::consts::PI;

use crate::dsp::{ProcessCtx, UGen, at};

/// Which filter a row is: scsynth's coefficient formula for the named rows,
/// or the tap gains as inputs for `Svf`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SvfMode {
    /// Butterworth lowpass: `LPF`.
    Lp,
    /// Butterworth highpass: `HPF`.
    Hp,
    /// Resonant lowpass, `rq` from input 2: `RLPF`.
    RLp,
    /// Resonant highpass: `RHPF`.
    RHp,
    /// Bandpass with unity gain at the centre, `rq` the bandwidth: `BPF`.
    Bp,
    /// scsynth's two-pole resonator with zeros at DC and Nyquist: `Resonz`.
    Resonz,
    /// Band reject: `BRF`.
    Notch,
    /// The three tap gains are signal inputs 3, 4 and 5: `Svf`.
    Mix,
}

/// A two-pole section as scsynth writes one: `w[n] = x[n] + b1 w[n-1] + b2
/// w[n-2]` and `y[n] = n0 w[n] + n1 w[n-1] + n2 w[n-2]`, so
/// `H(z) = (n0 + n1 z^-1 + n2 z^-2) / (1 - b1 z^-1 - b2 z^-2)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    pub n: [f64; 3],
    pub b1: f64,
    pub b2: f64,
}

impl Biquad {
    /// scsynth's coefficients for `mode` at cutoff `freq` (Hz) and `rq`, at
    /// `sr`. `freq` is kept inside `(0, sr/2)`, where every formula is
    /// defined.
    pub fn scsynth(mode: SvfMode, freq: f64, rq: f64, sr: f64) -> Self {
        let freq = freq.clamp(1e-3, sr * 0.4999);
        let w = freq * 2.0 * PI / sr;
        // scsynth's `sqrt2_f`: the float, not the double.
        let sqrt2 = std::f32::consts::SQRT_2 as f64;
        match mode {
            SvfMode::Lp => {
                let c = 1.0 / (w * 0.5).tan();
                let a0 = 1.0 / (1.0 + sqrt2 * c + c * c);
                Self {
                    n: [a0, 2.0 * a0, a0],
                    b1: -2.0 * (1.0 - c * c) * a0,
                    b2: -(1.0 - sqrt2 * c + c * c) * a0,
                }
            }
            SvfMode::Hp => {
                let c = (w * 0.5).tan();
                let a0 = 1.0 / (1.0 + sqrt2 * c + c * c);
                Self {
                    n: [a0, -2.0 * a0, a0],
                    b1: 2.0 * (1.0 - c * c) * a0,
                    b2: -(1.0 - sqrt2 * c + c * c) * a0,
                }
            }
            SvfMode::RLp | SvfMode::RHp => {
                let q = rq.max(0.001);
                let d = (w * q * 0.5).tan();
                let c = (1.0 - d) / (1.0 + d);
                let b1 = (1.0 + c) * w.cos();
                let b2 = -c;
                if mode == SvfMode::RLp {
                    let a0 = (1.0 + c - b1) * 0.25;
                    Self {
                        n: [a0, 2.0 * a0, a0],
                        b1,
                        b2,
                    }
                } else {
                    let a0 = (1.0 + c + b1) * 0.25;
                    Self {
                        n: [a0, -2.0 * a0, a0],
                        b1,
                        b2,
                    }
                }
            }
            SvfMode::Bp => {
                let c = 1.0 / (rq * w * 0.5).tan();
                let d = 2.0 * w.cos();
                let a0 = 1.0 / (1.0 + c);
                Self {
                    n: [a0, 0.0, -a0],
                    b1: c * d * a0,
                    b2: (1.0 - c) * a0,
                }
            }
            SvfMode::Notch => {
                let c = (rq * w * 0.5).tan();
                let d = 2.0 * w.cos();
                let a0 = 1.0 / (1.0 + c);
                let a1 = -d * a0;
                Self {
                    n: [a0, a1, a0],
                    b1: -a1,
                    b2: -(1.0 - c) * a0,
                }
            }
            SvfMode::Resonz => {
                let b = w * rq;
                let r = 1.0 - b * 0.5;
                let r2 = r * r;
                let cost = (2.0 * r * w.cos()) / (1.0 + r2);
                let a0 = (1.0 - r2) * 0.5;
                Self {
                    n: [a0, 0.0, -a0],
                    b1: 2.0 * r * cost,
                    b2: -r2,
                }
            }
            SvfMode::Mix => unreachable!("Svf takes its taps as inputs"),
        }
    }

    /// The same transfer function as a state-variable filter: the integrator
    /// gain `g`, the damping `k` and the three tap gains.
    ///
    /// The bilinear transform is a bijection between analog and digital
    /// two-poles, so this is exact: substituting `z = (1 + g s)/(1 - g s)`
    /// into the section gives an analog two-pole whose denominator, normalized
    /// to `s^2 + k s + 1`, fixes `g` and `k`, and whose numerator is a mix of
    /// the three taps `1`, `s`, `s^2`.
    pub fn svf(self) -> Params {
        let [n0, n1, n2] = self.n;
        // The denominator at z = 1 and z = -1. Both are positive for a
        // section with its poles inside the circle and away from its ends.
        let p = (1.0 - self.b1 - self.b2).max(1e-30);
        let q = (1.0 + self.b1 - self.b2).max(1e-30);
        let g = (p / q).sqrt();
        let k = 2.0 * (1.0 + self.b2) / (g * q);
        Params::new(
            g,
            k,
            [
                (n0 + n1 + n2) / p,
                2.0 * g * (n0 - n2) / p,
                (n0 - n1 + n2) / q,
            ],
        )
    }
}

/// Everything one sample of the filter reads: the integrator gains for a
/// `(g, k)` pair and the tap mix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
    /// Gains of the `(lowpass, bandpass, highpass)` taps.
    mix: [f64; 3],
}

impl Params {
    fn new(g: f64, k: f64, mix: [f64; 3]) -> Self {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        Self { a1, a2, a3, k, mix }
    }

    /// `Svf`'s own parameterization: the cutoff prewarped, `rq` the damping.
    /// A damping of `0` is infinite Q, representable here without dividing by
    /// anything, which is the reason the wire carries `rq` rather than `Q`.
    fn cutoff(fc: f64, rq: f64, sr: f64) -> Self {
        let fc = fc.clamp(1e-3, sr * 0.4999);
        Self::new((PI * fc / sr).tan(), rq.max(0.0), [0.0; 3])
    }

    #[inline]
    fn step_toward(&mut self, d: &Self) {
        self.a1 += d.a1;
        self.a2 += d.a2;
        self.a3 += d.a3;
        self.k += d.k;
        for (m, dm) in self.mix.iter_mut().zip(d.mix) {
            *m += dm;
        }
    }

    /// The per-sample step that moves `self` to `to` in `n` samples.
    fn delta(&self, to: &Self, n: usize) -> Self {
        let inv = 1.0 / n.max(1) as f64;
        let mut mix = [0.0; 3];
        for (i, m) in mix.iter_mut().enumerate() {
            *m = (to.mix[i] - self.mix[i]) * inv;
        }
        Self {
            a1: (to.a1 - self.a1) * inv,
            a2: (to.a2 - self.a2) * inv,
            a3: (to.a3 - self.a3) * inv,
            k: (to.k - self.k) * inv,
            mix,
        }
    }
}

/// A topology-preserving (trapezoidal-integrator) state-variable filter.
///
/// Inputs: 0 the signal, 1 the cutoff in Hz, then whatever the mode reads --
/// nothing for the Butterworth pair, `rq` for the resonant ones, and
/// `rq, low, band, high` for [`SvfMode::Mix`].
pub struct Svf {
    mode: SvfMode,
    /// The two integrator states. `f64` for the reason in the module docs.
    ic1: f64,
    ic2: f64,
    /// Where the last slice left the parameters, and the inputs they came
    /// from (recomputed only when those move).
    params: Option<(Params, f32, f32)>,
}

impl Svf {
    pub fn new(mode: SvfMode) -> Self {
        Self {
            mode,
            ic1: 0.0,
            ic2: 0.0,
            params: None,
        }
    }

    /// One sample through the two integrators, returning the three taps
    /// `(lowpass, bandpass, highpass)`.
    #[inline]
    fn step(&mut self, v0: f64, c: &Params) -> (f64, f64, f64) {
        let v3 = v0 - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, v0 - c.k * v1 - v2)
    }

    fn params(&self, freq: f32, rq: f32, sr: f64) -> Params {
        match self.mode {
            SvfMode::Mix => Params::cutoff(freq as f64, rq as f64, sr),
            mode => Biquad::scsynth(mode, freq as f64, rq as f64, sr).svf(),
        }
    }
}

impl UGen for Svf {
    fn reset(&mut self) {
        *self = Self::new(self.mode);
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let sr = ctx.sample_rate as f64;
        let (sig, freq) = (inputs[0], inputs[1]);
        let rq = match self.mode {
            SvfMode::Lp | SvfMode::Hp => &[0.0][..],
            _ => inputs[2],
        };
        let n = output.len();
        if n == 0 {
            return;
        }
        let (f, r) = (freq[freq.len() - 1], rq[rq.len() - 1]);
        let target = match self.params {
            Some((p, pf, pr)) if pf == f && pr == r => p,
            _ => self.params(f, r, sr),
        };
        let mut c = self.params.map_or(target, |(p, _, _)| p);
        let d = c.delta(&target, n);
        let ramp = c != target;
        self.params = Some((target, f, r));

        // Only the mix row carries tap gains as inputs.
        let taps = (self.mode == SvfMode::Mix).then(|| (inputs[3], inputs[4], inputs[5]));
        for (i, s) in output.iter_mut().enumerate() {
            if ramp {
                c.step_toward(&d);
            }
            let (lp, bp, hp) = self.step(at(sig, i) as f64, &c);
            let m = match taps {
                Some((l, b, h)) => [at(l, i) as f64, at(b, i) as f64, at(h, i) as f64],
                None => c.mix,
            };
            *s = (m[0] * lp + m[1] * bp + m[2] * hp) as f32;
        }
    }
}

/// The single-state filters: `OnePole`, `OneZero`, `LeakDC`, `Integrator`.
///
/// All four take a **coefficient**, not a frequency -- scsynth's contract, and
/// the honest one: their pole is the parameter, and naming it a cutoff would
/// promise a `-3 dB` point the one-pole shape does not have in the same sense a
/// two-pole one does. Use [`Lag`](super::lag::Lag) when what you want is a time
/// constant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OneKind {
    /// `y[n] = (1 - |c|)*x[n] + c*y[n-1]` -- lowpass for `c > 0`, highpass for
    /// `c < 0`; the leading factor keeps the passband at unity.
    OnePole,
    /// `y[n] = (1 - |c|)*x[n] + c*x[n-1]` -- the zero-only sibling.
    OneZero,
    /// `y[n] = x[n] - x[n-1] + c*y[n-1]` -- a DC blocker: a zero exactly at DC
    /// with a pole just inside it.
    LeakDc,
    /// `y[n] = x[n] + c*y[n-1]` -- a leaky accumulator. The coefficient is
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
    fn reset(&mut self) {
        *self = Self::new(self.kind);
    }

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
