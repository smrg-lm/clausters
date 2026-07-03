//! Wavetable format and buffer generators (`/b_gen`), S5.
//!
//! Two things live here, both pure and off the audio thread:
//!
//! 1. **The wavetable format.** [`signal_to_wavetable`] turns one period of a
//!    signal into scsynth's interleaved layout — for each point a pair
//!    `[2*a[i] - a[i+1], a[i+1] - a[i]]`. An interpolating oscillator then
//!    reads a sample with a single fused multiply-add ([`wt_interp`]): with the
//!    fractional phase `frac` in `[0, 1)`,
//!    `out = x0 + (1 + frac) * x1 = a[i] + frac * (a[i+1] - a[i])`. Storing the
//!    offset/slope pair instead of the raw samples is what lets the read be one
//!    madd with no branch. See `Osc`/`VOsc`/`Shaper` in [`crate::dsp::osc`].
//!
//! 2. **The generators.** [`GenCommand`] is one parsed `/b_gen` command
//!    (`sine1`/`sine2`/`sine3` additive spectra, `cheby` waveshaping transfer
//!    functions, `copy` between buffers), with the [`GenFlags`] (normalize /
//!    wavetable / clear). [`GenCommand::apply`] runs it against the current
//!    buffer contents and returns a fresh immutable [`Buffer`] — the network
//!    thread swaps it in through the same build-and-swap path as `/b_read`, so
//!    the audio thread only ever sees a finished buffer.

use std::f64::consts::TAU;

use crate::dsp::buffer::Buffer;

/// The `/b_gen` flag bits, packed into the command's `flags` int (scsynth's
/// `normalize`(1) / `wavetable`(2) / `clear`(4)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenFlags {
    /// Scale the generated signal so its peak magnitude is 1.
    pub normalize: bool,
    /// Store the result in the interleaved wavetable format (halving the
    /// number of period points to fit the same sample count).
    pub wavetable: bool,
    /// Start from silence; when unset, the new signal is added on top of the
    /// buffer's current contents.
    pub clear: bool,
}

impl GenFlags {
    pub fn from_bits(bits: i32) -> Self {
        Self {
            normalize: bits & 1 != 0,
            wavetable: bits & 2 != 0,
            clear: bits & 4 != 0,
        }
    }
}

/// One parsed `/b_gen` command, fully resolved (source buffers already pulled
/// from the mirror) so it can run on the NRT thread with no further lookups.
pub enum GenCommand {
    /// `sine1 flags amp...`: additive sine partials — `amp[k]` is the amplitude
    /// of harmonic `k + 1`.
    Sine1 { flags: GenFlags, amps: Vec<f32> },
    /// `sine2 flags (freq amp)...`: partials at arbitrary (possibly fractional)
    /// harmonic numbers.
    Sine2 {
        flags: GenFlags,
        partials: Vec<(f32, f32)>,
    },
    /// `sine3 flags (freq amp phase)...`: as `sine2` with a per-partial phase
    /// (radians).
    Sine3 {
        flags: GenFlags,
        partials: Vec<(f32, f32, f32)>,
    },
    /// `cheby flags amp...`: a waveshaping transfer function, the weighted sum
    /// `sum_k amp[k] * T_{k+1}(x)` of Chebyshev polynomials over `x` in
    /// `[-1, 1]`. Read by `Shaper`. Uses the non-wrapping wavetable layout.
    Cheby { flags: GenFlags, coeffs: Vec<f32> },
    /// `copy dstStart srcBuf srcStart numSamples`: overlay `num` samples of
    /// `src` (from `src_start`) onto a copy of the current buffer at
    /// `dst_start`. No flags. `num < 0` copies to the end of the shorter side.
    Copy {
        dst_start: usize,
        src: std::sync::Arc<Buffer>,
        src_start: usize,
        num: i64,
    },
}

impl GenCommand {
    /// Runs the command against `current` and returns the replacement buffer
    /// (same shape and sample rate). The whole computation is off the audio
    /// thread, so allocation is fine.
    pub fn apply(&self, current: &Buffer) -> Buffer {
        let channels = current.channels();
        let frames = current.frames();
        let sr = current.sample_rate();
        // Wavetable generation treats the buffer as one flat signal (buffers
        // used for wavetables are mono); `len` is the total sample count.
        let len = current.data().len();

        let data = match self {
            GenCommand::Copy {
                dst_start,
                src,
                src_start,
                num,
            } => copy_samples(current, src, *dst_start, *src_start, *num),
            _ => {
                let flags = self.flags();
                // Period length: half the samples in wavetable mode (the format
                // doubles), the whole buffer otherwise.
                let n = if flags.wavetable { len / 2 } else { len };
                let mut signal = self.base_signal(current, n, flags.clear);
                self.render_into(&mut signal);
                if flags.normalize {
                    normalize(&mut signal);
                }
                let mut data = if flags.wavetable {
                    // Oscillator tables wrap (periodic); a cheby transfer curve
                    // holds its endpoint.
                    signal_to_wavetable(&signal, !matches!(self, GenCommand::Cheby { .. }))
                } else {
                    signal
                };
                data.resize(len, 0.0);
                data
            }
        };
        Buffer::new(data, channels, frames, sr)
    }

    fn flags(&self) -> GenFlags {
        match self {
            GenCommand::Sine1 { flags, .. }
            | GenCommand::Sine2 { flags, .. }
            | GenCommand::Sine3 { flags, .. }
            | GenCommand::Cheby { flags, .. } => *flags,
            GenCommand::Copy { .. } => GenFlags {
                normalize: false,
                wavetable: false,
                clear: true,
            },
        }
    }

    /// The accumulation buffer the generator writes into: zeros when clearing,
    /// otherwise the first `n` samples of the current contents (so a second
    /// `/b_gen` without `clear` adds to what is already there).
    fn base_signal(&self, current: &Buffer, n: usize, clear: bool) -> Vec<f32> {
        if clear {
            vec![0.0; n]
        } else {
            let mut base = vec![0.0; n];
            let src = current.data();
            let take = n.min(src.len());
            base[..take].copy_from_slice(&src[..take]);
            base
        }
    }

    /// Adds this command's contribution to `signal` (one period).
    fn render_into(&self, signal: &mut [f32]) {
        let n = signal.len();
        if n == 0 {
            return;
        }
        match self {
            GenCommand::Sine1 { amps, .. } => {
                for (k, &amp) in amps.iter().enumerate() {
                    add_sine(signal, (k + 1) as f32, amp, 0.0);
                }
            }
            GenCommand::Sine2 { partials, .. } => {
                for &(freq, amp) in partials {
                    add_sine(signal, freq, amp, 0.0);
                }
            }
            GenCommand::Sine3 { partials, .. } => {
                for &(freq, amp, phase) in partials {
                    add_sine(signal, freq, amp, phase);
                }
            }
            GenCommand::Cheby { coeffs, .. } => {
                let denom = (n - 1).max(1) as f32;
                for (j, s) in signal.iter_mut().enumerate() {
                    let x = 2.0 * j as f32 / denom - 1.0;
                    *s += cheby_sum(coeffs, x);
                }
            }
            GenCommand::Copy { .. } => {}
        }
    }
}

/// Adds `amp * sin(2π · freq · j/N + phase)` over one period `signal[0..N]`.
fn add_sine(signal: &mut [f32], freq: f32, amp: f32, phase: f32) {
    let n = signal.len();
    let w = TAU * freq as f64 / n as f64;
    let ph = phase as f64;
    for (j, s) in signal.iter_mut().enumerate() {
        *s += amp * (w * j as f64 + ph).sin() as f32;
    }
}

/// `sum_k coeffs[k] * T_{k+1}(x)` — Chebyshev polynomials by the recurrence
/// `T_0 = 1`, `T_1 = x`, `T_{m+1} = 2x·T_m - T_{m-1}`. `coeffs[0]` weights
/// `T_1` (a linear transfer, i.e. passthrough).
fn cheby_sum(coeffs: &[f32], x: f32) -> f32 {
    if coeffs.is_empty() {
        return 0.0;
    }
    let mut t_prev = 1.0; // T_0
    let mut t_cur = x; // T_1
    let mut sum = coeffs[0] * t_cur;
    for &c in &coeffs[1..] {
        let t_next = 2.0 * x * t_cur - t_prev;
        sum += c * t_next;
        t_prev = t_cur;
        t_cur = t_next;
    }
    sum
}

/// Scales `signal` in place so its peak magnitude is 1 (a no-op if it is all
/// zeros).
fn normalize(signal: &mut [f32]) {
    let peak = signal.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    if peak > 0.0 {
        let gain = 1.0 / peak;
        for s in signal.iter_mut() {
            *s *= gain;
        }
    }
}

/// The `copy` command: a clone of `current` with `[dst_start, dst_start+num)`
/// overwritten by `src[src_start..]`. Ranges are clamped to both buffers.
fn copy_samples(
    current: &Buffer,
    src: &Buffer,
    dst_start: usize,
    src_start: usize,
    num: i64,
) -> Vec<f32> {
    let mut data = current.data().to_vec();
    let src = src.data();
    let dst_avail = data.len().saturating_sub(dst_start);
    let src_avail = src.len().saturating_sub(src_start);
    let mut take = dst_avail.min(src_avail);
    if num >= 0 {
        take = take.min(num as usize);
    }
    data[dst_start..dst_start + take].copy_from_slice(&src[src_start..src_start + take]);
    data
}

/// Converts one period `signal` into scsynth's interleaved wavetable layout,
/// `[2*a[i] - a[i+1], a[i+1] - a[i]]` per point (see the module docs). `wrap`
/// picks the neighbour of the last point: `a[0]` for a periodic oscillator
/// table, or a held `a[n-1]` for a one-shot transfer function.
pub fn signal_to_wavetable(signal: &[f32], wrap: bool) -> Vec<f32> {
    let n = signal.len();
    let mut table = Vec::with_capacity(n * 2);
    for i in 0..n {
        let a0 = signal[i];
        let a1 = if i + 1 < n {
            signal[i + 1]
        } else if wrap {
            signal[0]
        } else {
            a0
        };
        table.push(2.0 * a0 - a1);
        table.push(a1 - a0);
    }
    table
}

/// Reads the wavetable pair at integer point `k` with fractional phase `frac`
/// in `[0, 1)`: `x0 + (1 + frac) * x1`, one fused multiply-add. `k` must be a
/// valid point (`2*k + 1 < table.len()`); the callers keep it in range.
#[inline(always)]
pub fn wt_interp(table: &[f32], k: usize, frac: f32) -> f32 {
    let x0 = table[2 * k];
    let x1 = table[2 * k + 1];
    x0 + (1.0 + frac) * x1
}
