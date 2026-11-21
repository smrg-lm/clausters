//! Wavetable format and buffer generators (`/buffer_gen`).
//!
//! Two things live here, both pure and off the audio thread:
//!
//! 1. **The wavetable format.** [`signal_to_wavetable`] turns one period of a
//!    signal into scsynth's interleaved layout -- for each point a pair
//!    `[2*a[i] - a[i+1], a[i+1] - a[i]]`. An interpolating oscillator then
//!    reads a sample with a single fused multiply-add ([`wt_interp`]): with the
//!    fractional phase `frac` in `[0, 1)`,
//!    `out = x0 + (1 + frac) * x1 = a[i] + frac * (a[i+1] - a[i])`. Storing the
//!    offset/slope pair instead of the raw samples is what lets the read be one
//!    madd with no branch. See `Osc`/`VOsc`/`Shaper` in `crate::dsp::osc`.
//!
//! 2. **The generators.** [`GenCommand`] is one parsed `/buffer_gen` command
//!    (`sine1`/`sine2`/`sine3` additive spectra, `cheby` waveshaping transfer
//!    functions, `copy` between buffers), with the [`GenFlags`] (normalize /
//!    wavetable / clear). [`GenCommand::apply`] runs it against the current
//!    buffer contents and returns a fresh immutable [`Buffer`] -- the network
//!    thread swaps it in through the same build-and-swap path as `/buffer_read`, so
//!    the audio thread only ever sees a finished buffer.

use std::f64::consts::TAU;
use std::sync::atomic::AtomicU32;

use clausters_core::envshape::shape_value;

use crate::dsp::buffer::Buffer;

/// The `/buffer_gen` flag bits, packed into the command's `flags` int (scsynth's
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

/// One parsed `/buffer_gen` command, fully resolved (source buffers already pulled
/// from the mirror) so it can run on the NRT thread with no further lookups.
pub enum GenCommand {
    /// `sine1 flags amp...`: additive sine partials -- `amp[k]` is the amplitude
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
    /// `[-1, 1]`, each shifted to pass through 0 at `x = 0` as scsynth's are.
    /// Read by `Shaper`. In wavetable form its `n` pairs span `[-1, 1]`
    /// inclusive.
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
    /// `prepare_partconv fftSize srcBuf`: partition `src`'s samples into the
    /// prepared-kernel layout the `Conv` UGen reads: partitions of
    /// `L = fftSize/2` samples, each zero-padded to `fftSize` and
    /// forward-transformed **here, off the audio thread** -- the RT side only
    /// ever multiplies against the ready spectra. Layout in
    /// `dsp::conv::layout`. The whole response is prepared or none of it:
    /// `/buffer_gen` refuses a target too small for it. A multichannel
    /// source contributes channel 0.
    PreparePartConv {
        src: std::sync::Arc<Buffer>,
        fft_size: usize,
    },
    /// `env level0 [level time shape curve]...`: discretize a break-point
    /// envelope across the whole buffer, evaluating each segment through
    /// `clausters_core::envshape` -- the same curve math the `EnvGen` UGen plays
    /// -- so a client's drawn/edited automation curve becomes a control buffer
    /// that reads back identically. Segment times are relative (only their
    /// proportions matter); the mono curve is written to every channel. No flags.
    Env {
        initial: f32,
        segments: Vec<EnvSegment>,
    },
}

/// One segment of a [`GenCommand::Env`] break-point envelope: interpolate from
/// the previous level to `level` over `time` (relative units), following the
/// SuperCollider shape number `shape` (`curve` is read only by the
/// custom-curvature shape). Mirrors the per-segment `EnvGen` input tuple.
pub struct EnvSegment {
    pub level: f32,
    pub time: f32,
    pub shape: i32,
    pub curve: f32,
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
        let len = current.len();

        let data = match self {
            GenCommand::Copy {
                dst_start,
                src,
                src_start,
                num,
            } => copy_samples(current, src, *dst_start, *src_start, *num),
            GenCommand::Env { initial, segments } => {
                env_curve(*initial, segments, frames, channels)
            }
            GenCommand::PreparePartConv { src, fft_size } => prepare_partconv(src, *fft_size, len),
            _ => {
                let flags = self.flags();
                if flags.wavetable {
                    // scsynth adds a command's pairs to the pairs already
                    // there, and normalizes the pairs: the format is linear in
                    // the signal, so this is the signal's sum, normalized.
                    let n = len / 2;
                    let mut data = self.wavetable(n);
                    if !flags.clear {
                        for (d, c) in data.iter_mut().zip(current.to_vec()) {
                            *d += c;
                        }
                    }
                    if flags.normalize {
                        normalize_wavetable(&mut data);
                    }
                    data.resize(len, 0.0);
                    data
                } else {
                    let mut signal = self.base_signal(current, len, flags.clear);
                    self.render_into(&mut signal);
                    if flags.normalize {
                        normalize(&mut signal);
                    }
                    signal
                }
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
            GenCommand::Copy { .. }
            | GenCommand::Env { .. }
            | GenCommand::PreparePartConv { .. } => GenFlags {
                normalize: false,
                wavetable: false,
                clear: true,
            },
        }
    }

    /// The accumulation buffer the generator writes into: zeros when clearing,
    /// otherwise the first `n` samples of the current contents (so a second
    /// `/buffer_gen` without `clear` adds to what is already there).
    fn base_signal(&self, current: &Buffer, n: usize, clear: bool) -> Vec<f32> {
        if clear {
            vec![0.0; n]
        } else {
            let mut base = vec![0.0; n];
            let src = current.to_vec();
            let take = n.min(src.len());
            base[..take].copy_from_slice(&src[..take]);
            base
        }
    }

    /// This command's contribution as `n` wavetable pairs. An oscillator
    /// table is one period, so its last pair leads back to its first value; a
    /// `cheby` transfer curve is sampled at `n + 1` points from `-1` to `1`
    /// inclusive, so its last pair ends exactly at `x = 1`, which is what
    /// `Shaper` reads at its top (scsynth's `add_wchebyshev`).
    fn wavetable(&self, n: usize) -> Vec<f32> {
        match self {
            GenCommand::Cheby { coeffs, .. } => {
                let v: Vec<f32> = (0..=n)
                    .map(|i| cheby_sum(coeffs, -1.0 + 2.0 * i as f64 / n.max(1) as f64))
                    .collect();
                let mut table = Vec::with_capacity(n * 2);
                for w in v.windows(2) {
                    table.push(2.0 * w[0] - w[1]);
                    table.push(w[1] - w[0]);
                }
                table
            }
            _ => {
                let mut signal = vec![0.0; n];
                self.render_into(&mut signal);
                signal_to_wavetable(&signal, true)
            }
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
                // scsynth's `add_chebyshev`: `n` points from `-1`, stepping
                // `2/n`, so the last falls one step short of `1`.
                for (j, s) in signal.iter_mut().enumerate() {
                    *s += cheby_sum(coeffs, -1.0 + 2.0 * j as f64 / n as f64);
                }
            }
            // Copy, Env and PreparePartConv short-circuit in `apply` and
            // never reach here.
            GenCommand::Copy { .. }
            | GenCommand::Env { .. }
            | GenCommand::PreparePartConv { .. } => {}
        }
    }
}

/// Adds `amp * sin(2pi * freq * j/N + phase)` over one period `signal[0..N]`.
fn add_sine(signal: &mut [f32], freq: f32, amp: f32, phase: f32) {
    let n = signal.len();
    let w = TAU * freq as f64 / n as f64;
    let ph = phase as f64;
    for (j, s) in signal.iter_mut().enumerate() {
        *s += amp * (w * j as f64 + ph).sin() as f32;
    }
}

/// `sum_k coeffs[k] * (T_{k+1}(x) - T_{k+1}(0))` -- the Chebyshev polynomials
/// as scsynth's `cheby` adds them, each offset so it passes through 0 at `x =
/// 0`: a silent input stays silent, whatever the even partials would add as a
/// constant. `coeffs[0]` weights `T_1` (a linear transfer, i.e. passthrough).
/// `T_k(x) = cos(k acos x)`, computed as scsynth computes it, in `f64`.
fn cheby_sum(coeffs: &[f32], x: f64) -> f32 {
    let x = x.clamp(-1.0, 1.0);
    let mut sum = 0.0;
    for (k, &amp) in coeffs.iter().enumerate() {
        if amp == 0.0 {
            continue;
        }
        let partial = (k + 1) as f64;
        let offset = -(amp as f64) * (partial * std::f64::consts::FRAC_PI_2).cos();
        sum += amp as f64 * (partial * x.acos()).cos() + offset;
    }
    sum as f32
}

/// Scales a wavetable in place so the signal it encodes peaks at 1: the value
/// at each point is the sum of its pair (scsynth's `normalize_wsamples`).
fn normalize_wavetable(table: &mut [f32]) {
    let (pairs, _) = table.as_chunks::<2>();
    let peak = pairs.iter().fold(0.0f32, |m, p| m.max((p[0] + p[1]).abs()));
    if peak > 0.0 && peak != 1.0 {
        let gain = 1.0 / peak;
        for s in table.iter_mut() {
            *s *= gain;
        }
    }
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
    let mut data = current.to_vec();
    let dst_avail = data.len().saturating_sub(dst_start);
    let src_avail = src.len().saturating_sub(src_start);
    let mut take = dst_avail.min(src_avail);
    if num >= 0 {
        take = take.min(num as usize);
    }
    // **Only the span is read**, never the whole source: an editor copies a
    // stroke's worth out of a take that may be ten minutes long, and reading
    // it through a join is a read per sample, not a copy of the join.
    for (k, out) in data[dst_start..dst_start + take].iter_mut().enumerate() {
        *out = src.at(src_start + k);
    }
    data
}

/// The `env` command: discretize a break-point envelope across `frames`
/// samples (replicated across `channels`), sampling each segment's shape through
/// [`shape_value`]. `segments` times are relative -- only their proportions
/// matter, since the buffer holds the curve *shape* and playback rate maps it
/// onto real time. Matches `EnvGen`: `frac` within a segment is
/// `elapsed / duration` and the endpoints land on the segment levels.
fn env_curve(initial: f32, segments: &[EnvSegment], frames: usize, channels: usize) -> Vec<f32> {
    let total: f32 = segments.iter().map(|s| s.time.max(0.0)).sum();
    let final_level = segments.last().map_or(initial, |s| s.level);
    let curve_at = |tpos: f32| -> f32 {
        if segments.is_empty() || total <= 0.0 {
            return final_level;
        }
        let mut start = initial;
        let mut acc = 0.0f32;
        for seg in segments {
            let dur = seg.time.max(0.0);
            if dur > 0.0 && tpos < acc + dur {
                let frac = (tpos - acc) / dur;
                return shape_value(seg.shape, seg.curve, start, seg.level, frac);
            }
            acc += dur;
            start = seg.level;
        }
        // At or past the end: hold the final level.
        final_level
    };

    let mut mono = vec![0.0f32; frames];
    if frames == 1 {
        mono[0] = curve_at(0.0);
    } else if frames > 1 {
        let span = (frames - 1) as f32;
        for (i, m) in mono.iter_mut().enumerate() {
            *m = curve_at((i as f32 / span) * total);
        }
    }

    if channels <= 1 {
        return mono;
    }
    let mut data = vec![0.0f32; frames * channels];
    for (f, &v) in mono.iter().enumerate() {
        for ch in 0..channels {
            data[f * channels + ch] = v;
        }
    }
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
///
/// Over the buffer's own cells rather than a slice of them: a table is a
/// buffer's contents, and those are written while they are read (see
/// [`crate::dsp::buffer`]). The reads are relaxed loads, which measured free in
/// this shape -- a table hot in cache is not what the optimizer was vectorizing.
#[inline(always)]
pub fn wt_interp(table: &[AtomicU32], k: usize, frac: f32) -> f32 {
    let x0 = Buffer::load(&table[2 * k]);
    let x1 = Buffer::load(&table[2 * k + 1]);
    x0 + (1.0 + frac) * x1
}

/// `prepare_partconv`: the impulse response in `src` (channel 0) partitioned
/// and forward-transformed into the [`crate::dsp::conv::layout`] a `Conv`
/// UGen reads: `[L, P, P * fft_size packed spectra]`, `L = fft_size / 2`.
/// The whole response is prepared or none of it: a target (`len` samples)
/// too small for every partition yields an all-zero (invalid) kernel, which
/// `Conv` plays as silence, never a kernel cut short. `/buffer_gen` refuses
/// that target before it gets here.
fn prepare_partconv(src: &Buffer, fft_size: usize, len: usize) -> Vec<f32> {
    use crate::dsp::conv::layout;

    let mut data = vec![0.0f32; len];
    let part = fft_size / 2;
    let channels = src.channels().max(1);
    let ir_len = src.len() / channels;
    let parts = ir_len.div_ceil(part.max(1));
    let parts_cap = len.saturating_sub(layout::HEADER) / fft_size;
    if part == 0 || parts == 0 || parts > parts_cap {
        return data;
    }
    data[0] = part as f32;
    data[1] = parts as f32;
    let mut scratch = vec![0.0f32; fft_size];
    for p in 0..parts {
        scratch.fill(0.0);
        for (k, slot) in scratch.iter_mut().enumerate().take(part) {
            let frame = p * part + k;
            if frame >= ir_len {
                break;
            }
            // Channel 0 of an interleaved buffer.
            *slot = src.at(frame * channels);
        }
        let out = &mut data[layout::HEADER + p * fft_size..layout::HEADER + (p + 1) * fft_size];
        clausters_core::fft::rfft_into(&scratch, out);
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::envshape::SHAPE_LINEAR;

    fn seg(level: f32, time: f32) -> EnvSegment {
        EnvSegment {
            level,
            time,
            shape: SHAPE_LINEAR,
            curve: 0.0,
        }
    }

    #[test]
    fn env_curve_linear_triangle() {
        // 0 -> 1 -> 0 over two equal linear segments, sampled at 5 points.
        let curve = env_curve(0.0, &[seg(1.0, 1.0), seg(0.0, 1.0)], 5, 1);
        let expected = [0.0, 0.5, 1.0, 0.5, 0.0];
        for (got, want) in curve.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "got {got}, want {want}");
        }
    }

    #[test]
    fn env_curve_endpoints_and_channels() {
        // First/last samples land exactly on the initial and final levels, and
        // the mono curve is replicated across channels.
        let curve = env_curve(0.2, &[seg(0.8, 1.0)], 4, 2);
        assert_eq!(curve.len(), 4 * 2);
        assert!((curve[0] - 0.2).abs() < 1e-6); // frame 0, channel 0
        assert!((curve[1] - 0.2).abs() < 1e-6); // frame 0, channel 1 (replicated)
        assert!((curve[curve.len() - 1] - 0.8).abs() < 1e-6); // last frame holds final
    }

    #[test]
    fn env_curve_zero_duration_holds_final() {
        // Degenerate (all-zero times) envelope holds the final level everywhere.
        let curve = env_curve(0.0, &[seg(0.7, 0.0)], 3, 1);
        for v in curve {
            assert!((v - 0.7).abs() < 1e-6);
        }
    }
}
