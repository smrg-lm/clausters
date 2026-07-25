//! Signal measurements shared by the UGen test suites.
//!
//! These are the asserts of the U track: a UGen is judged by *measuring* what
//! it produces and comparing against the analytic value the DSP claims, not by
//! diffing it against a stored buffer. A golden file pins a regression; it
//! cannot tell you the filter's −3 dB point is where it should be.
//!
//! Every function here documents the quantity it estimates and the conditions
//! under which the estimate is exact rather than approximate, because a number
//! whose error bars are unknown is not an assert. The module's own tests drive
//! each one with a synthetic signal of known content.
//!
//! **On measuring a spectrum exactly.** Two of these ([`alias_snr_db`],
//! [`amplitude_at`]) are exact only when the analysis window spans a whole
//! number of the signal's periods — *coherent sampling*. Under that condition a
//! rectangular window has no spectral leakage at all, so a component's energy
//! sits in one bin and nothing bleeds into its neighbours. The alternative,
//! windowing a non-coherent signal, buries anything below the window's sidelobe
//! floor: the best window `clausters_core` offers is Blackman at about −58 dB,
//! which is above the alias floor of a decent oscillator and would measure the
//! window instead of the UGen. So the tests pick their frequencies with
//! [`coherent_freq`] and use no window.
//!
//! Include it with `#[path = "common/signal.rs"] mod signal;`.

// A shared test-support module: each suite uses the handful of measurements it
// needs, so anything unused *there* is still used elsewhere. Suppressed once
// here rather than per function.
#![allow(dead_code)]

use clausters_core::fft;
use clausters_core::window::Window;

/// Root mean square of a block — the amplitude measure to compare against an
/// analytic gain. A sine of peak amplitude `a` has an RMS of `a / sqrt(2)`.
pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

/// Largest absolute sample.
pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, &v| m.max(v.abs()))
}

/// Mean of the block — its DC component.
pub fn dc(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|&v| v as f64).sum::<f64>() / x.len() as f64) as f32
}

/// Panics if any sample is NaN or infinite, naming the first offender. Worth
/// calling in every render test: a single NaN poisons the whole graph
/// downstream and an RMS assert alone will not say *where*.
pub fn assert_finite(x: &[f32], what: &str) {
    if let Some((i, v)) = x.iter().enumerate().find(|(_, v)| !v.is_finite()) {
        panic!("{what}: sample {i} is not finite ({v})");
    }
}

// ---- single-frequency analysis ----

/// One bin of the DFT evaluated at an **arbitrary** frequency, as
/// `(real, imaginary)`, accumulated in `f64`.
///
/// This is the tool for "what is the gain at exactly this frequency": an FFT
/// bin lands where `sr/n` puts it, which is almost never the frequency under
/// test, and reading the nearest bin measures leakage as much as signal.
///
/// Exact when `x` spans a whole number of periods of `hz` (see the module
/// note); otherwise the estimate carries the leakage of the implicit
/// rectangular window.
pub fn dft_at(x: &[f32], hz: f32, sr: f32) -> (f64, f64) {
    let w = std::f64::consts::TAU * hz as f64 / sr as f64;
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in x.iter().enumerate() {
        let (s, c) = (w * i as f64).sin_cos();
        re += v as f64 * c;
        im -= v as f64 * s;
    }
    (re, im)
}

/// Peak amplitude of the sinusoidal component at `hz` — the scale a real
/// sinusoid of amplitude `a` reports as `a`.
pub fn amplitude_at(x: &[f32], hz: f32, sr: f32) -> f32 {
    let (re, im) = dft_at(x, hz, sr);
    (2.0 * (re * re + im * im).sqrt() / x.len() as f64) as f32
}

/// Phase of the component at `hz`, in radians in `(-pi, pi]`.
pub fn phase_at(x: &[f32], hz: f32, sr: f32) -> f32 {
    let (re, im) = dft_at(x, hz, sr);
    im.atan2(re) as f32
}

/// The complex response a filter applied to a signal, measured from an actual
/// input/output pair at one frequency: `(gain, phase_shift_radians)`.
///
/// Feed it the *steady-state* portion of both buffers — a filter's first
/// samples are its transient, and including them measures the transient too.
pub fn response_at(input: &[f32], output: &[f32], hz: f32, sr: f32) -> (f32, f32) {
    let (ir, ii) = dft_at(input, hz, sr);
    let (or_, oi) = dft_at(output, hz, sr);
    let gain = ((or_ * or_ + oi * oi) / (ir * ir + ii * ii)).sqrt() as f32;
    let mut phase = oi.atan2(or_) - ii.atan2(ir);
    while phase <= -std::f64::consts::PI {
        phase += std::f64::consts::TAU;
    }
    while phase > std::f64::consts::PI {
        phase -= std::f64::consts::TAU;
    }
    (gain, phase as f32)
}

/// The frequency nearest `target` for which an `n`-sample window holds a whole
/// number of periods **and** whose bin index is odd.
///
/// Both conditions matter for [`alias_snr_db`]. Whole periods make the analysis
/// leak-free. An *odd* bin index `k` is coprime to the power-of-two `n`, which
/// is what keeps aliased partials off the harmonic bins: a partial at `m·k`
/// folds to `|m·k − j·n|` bins, a multiple of `gcd(k, n)` — with `gcd = k` the
/// aliases would land exactly on top of the harmonics and be invisible.
pub fn coherent_freq(target: f32, sr: f32, n: usize) -> f32 {
    let exact = target as f64 * n as f64 / sr as f64;
    let mut k = exact.round() as i64;
    if k % 2 == 0 {
        // Step to whichever odd neighbour is closer to the requested frequency.
        k += if exact > k as f64 { 1 } else { -1 };
    }
    let k = k.clamp(1, (n / 2 - 1) as i64);
    k as f32 * sr / n as f32
}

/// Ratio, in dB, between the energy of a periodic signal's harmonics and
/// everything else below Nyquist — the standard figure for how much an
/// oscillator aliases. Higher is cleaner.
///
/// `x.len()` must be a size [`clausters_core::fft`] supports and `f0` must come
/// from [`coherent_freq`] at that same size; both are checked, because either
/// one silently violated turns the result into a measurement of the analysis
/// rather than of the signal. DC is excluded (a waveform's offset is not
/// aliasing).
pub fn alias_snr_db(x: &[f32], f0: f32, sr: f32) -> f32 {
    let n = x.len();
    assert!(
        fft::supports(n),
        "alias_snr_db needs one of {:?} samples, got {n}",
        fft::SUPPORTED_SIZES
    );
    let k_exact = f0 as f64 * n as f64 / sr as f64;
    let k = k_exact.round() as usize;
    assert!(
        (k_exact - k as f64).abs() < 1e-6 && k % 2 == 1 && k >= 1,
        "f0 {f0} Hz is not an odd whole number of periods in {n} samples at \
         {sr} Hz (bin {k_exact}); build it with coherent_freq"
    );

    let mut mags = vec![0.0f32; n / 2];
    assert!(fft::rfft_magnitudes_into(x, &mut mags), "rfft failed");

    // Harmonic bins, each with its immediate neighbours: coherent sampling puts
    // a partial entirely in one bin, and the +/-1 margin only absorbs
    // floating-point dribble rather than real alias energy.
    let mut harmonic = vec![false; n / 2];
    let mut h = k;
    while h < n / 2 {
        harmonic[h.saturating_sub(1)..(h + 2).min(n / 2)].fill(true);
        h += k;
    }

    let (mut sig, mut noise) = (0.0f64, 0.0f64);
    // Skip bin 0 and 1: DC and its neighbour are the waveform's offset.
    for b in 2..n / 2 {
        let p = mags[b] as f64 * mags[b] as f64;
        if harmonic[b] { sig += p } else { noise += p }
    }
    if noise <= 0.0 {
        return f32::INFINITY;
    }
    (10.0 * (sig / noise).log10()) as f32
}

// ---- broadband analysis ----

/// Welch-averaged power spectrum: `x` split into `n`-sample frames overlapping
/// by half, each windowed, the magnitudes squared and averaged over frames.
///
/// Averaging is what makes a *noise* floor assertable — a single frame's bins
/// are themselves random with 100 % relative standard deviation, so a slope fit
/// over one frame is meaningless. Returns `n / 2` bins, bin `b` centred at
/// `b · sr / n`.
pub fn power_spectrum(x: &[f32], n: usize, win: Window) -> Vec<f32> {
    assert!(fft::supports(n), "unsupported analysis size {n}");
    assert!(x.len() >= n, "need at least {n} samples, got {}", x.len());
    let mut w = vec![0.0f32; n];
    win.fill(&mut w);
    let mut acc = vec![0.0f64; n / 2];
    let (mut frame, mut mags) = (vec![0.0f32; n], vec![0.0f32; n / 2]);
    let mut frames = 0usize;
    let mut start = 0;
    while start + n <= x.len() {
        for i in 0..n {
            frame[i] = x[start + i] * w[i];
        }
        assert!(fft::rfft_magnitudes_into(&frame, &mut mags), "rfft failed");
        for (a, &m) in acc.iter_mut().zip(mags.iter()) {
            *a += m as f64 * m as f64;
        }
        frames += 1;
        start += n / 2;
    }
    acc.iter().map(|&a| (a / frames as f64) as f32).collect()
}

/// Least-squares slope of the power spectrum in **dB per octave**, fitted over
/// octave bands between `lo_hz` and `hi_hz`.
///
/// Banding before fitting is deliberate: a per-bin fit weights the spectrum by
/// bin density, which is uniform in frequency and therefore heavily biased
/// toward the top octave. Octave bands weight each octave once, which is what
/// "dB per octave" means. White noise measures 0, pink noise −3.01.
pub fn spectral_slope_db_per_octave(x: &[f32], sr: f32, lo_hz: f32, hi_hz: f32) -> f32 {
    const N: usize = 4096;
    let spec = power_spectrum(x, N, Window::Hann);
    let bin_hz = sr / N as f32;
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    let mut lo = lo_hz;
    while lo * 2.0 <= hi_hz {
        let hi = lo * 2.0;
        let (b0, b1) = ((lo / bin_hz) as usize, (hi / bin_hz) as usize);
        let b1 = b1.min(spec.len());
        if b1 > b0 {
            let mean = spec[b0..b1].iter().map(|&p| p as f64).sum::<f64>() / (b1 - b0) as f64;
            if mean > 0.0 {
                // The band's geometric centre is its midpoint in log frequency.
                xs.push(((lo * hi).sqrt() as f64).log2());
                ys.push(10.0 * mean.log10());
            }
        }
        lo = hi;
    }
    assert!(
        xs.len() >= 2,
        "need at least two octave bands to fit a slope"
    );
    let n = xs.len() as f64;
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let num: f64 = xs.iter().zip(&ys).map(|(a, b)| (a - mx) * (b - my)).sum();
    let den: f64 = xs.iter().map(|a| (a - mx) * (a - mx)).sum();
    (num / den) as f32
}

/// Delay of `y` relative to `x` in samples, to sub-sample resolution.
///
/// Cross-correlates the two and refines the peak with a parabola through its
/// two neighbours — the standard estimator, exact for a symmetric peak and
/// accurate to a small fraction of a sample otherwise. Searches lags in
/// `0..max_lag`, so it measures a *delay*, not a lead.
pub fn group_delay_samples(x: &[f32], y: &[f32], max_lag: usize) -> f32 {
    let corr = |lag: usize| -> f64 {
        let n = x.len().min(y.len().saturating_sub(lag));
        (0..n).map(|i| x[i] as f64 * y[i + lag] as f64).sum()
    };
    let mut best = 0usize;
    let mut best_v = f64::NEG_INFINITY;
    for lag in 0..=max_lag {
        let v = corr(lag);
        if v > best_v {
            (best_v, best) = (v, lag);
        }
    }
    if best == 0 || best == max_lag {
        return best as f32;
    }
    let (a, b, c) = (corr(best - 1), best_v, corr(best + 1));
    let denom = a - 2.0 * b + c;
    let offset = if denom.abs() < 1e-30 {
        0.0
    } else {
        0.5 * (a - c) / denom
    };
    best as f32 + offset as f32
}
