//! Forward and inverse real FFT shared by the server and the GUI clients.
//!
//! Both the GUI spectrogram (its STFT) and the server's `FFT`/`IFFT` UGens
//! need a forward FFT over a power-of-two window. Keeping the algorithm
//! here means **one implementation, identical results**, in the shared core —
//! the rule that an algorithm used by more than one process lives once. It
//! wraps [`microfft`], which is `no_std` and **zero-allocation** with
//! compile-time power-of-two sizes (exactly the STFT's window sizes), so a
//! real-time caller never allocates inside `process` — the property the future
//! UGens require.
//!
//! `microfft` provides **both** directions: `real::rfft_*` (forward, real
//! input) and `inverse::ifft_*` (inverse complex FFT, normalized by `1/N`).
//! This module exposes three surfaces over them:
//!
//! - [`rfft_magnitudes_into`] — the half-spectrum magnitudes the spectrogram
//!   draws.
//! - [`rfft_into`] — a **forward** transform packing the complex frame in the
//!   canonical spectral-buffer layout `[dc, nyquist, re₁, im₁, …]` (scsynth's
//!   `FFT` buffer format), the wire the server's `FFT`→`PV_*`→`IFFT` chain
//!   passes between its UGens.
//! - [`irfft_into`] — the matching **inverse**: it reconstructs the full
//!   Hermitian-symmetric spectrum from that packed half-frame and runs
//!   `microfft::inverse`, producing the real time-domain frame for overlap-add
//!   resynthesis.
//!
//! All three are zero-allocation (the transforms run in stack buffers), so the
//! server's UGens call them per hop on the audio thread without allocating.

use microfft::{Complex32, inverse, real};

/// The window sizes [`rfft_magnitudes_into`] accepts: the powers of two
/// `microfft` is built for, up to its default `size-4096`.
pub const SUPPORTED_SIZES: &[usize] = &[256, 512, 1024, 2048, 4096];

/// Whether a window of `n` samples is a supported FFT size.
pub fn supports(n: usize) -> bool {
    SUPPORTED_SIZES.contains(&n)
}

/// Forward real FFT magnitudes of `input` into `mags`.
///
/// `input.len()` must be a [supported](SUPPORTED_SIZES) power of two `n` and
/// `mags.len()` must be `n / 2`. On success `mags[b]` is the magnitude of bin
/// `b` for `b` in `0..n/2`: bin 0 is the DC magnitude `|X[0]|` (the real Nyquist
/// term, which `microfft` packs into the DC bin's imaginary part, is not
/// returned — matching the half-spectrum the spectrogram draws). Returns
/// `false`, leaving `mags` untouched, if the size is unsupported or `mags` has
/// the wrong length. Zero-allocation: the transform runs in a stack buffer.
pub fn rfft_magnitudes_into(input: &[f32], mags: &mut [f32]) -> bool {
    let n = input.len();
    if mags.len() != n / 2 {
        return false;
    }
    macro_rules! arm {
        ($size:literal, $f:path) => {{
            let mut buf = [0.0f32; $size];
            buf.copy_from_slice(input);
            let spec = $f(&mut buf);
            // spec[0] packs (DC, Nyquist) in (re, im); the rest are ordinary
            // bins. We expose the DC magnitude and bins 1..n/2.
            mags[0] = spec[0].re.abs();
            for b in 1..$size / 2 {
                mags[b] = (spec[b].re * spec[b].re + spec[b].im * spec[b].im).sqrt();
            }
            true
        }};
    }
    match n {
        256 => arm!(256, real::rfft_256),
        512 => arm!(512, real::rfft_512),
        1024 => arm!(1024, real::rfft_1024),
        2048 => arm!(2048, real::rfft_2048),
        4096 => arm!(4096, real::rfft_4096),
        _ => false,
    }
}

/// Forward real FFT of `input` into the packed complex `frame`.
///
/// `input.len()` must be a [supported](SUPPORTED_SIZES) power of two `n` and
/// `frame.len()` must be `n`. On success `frame` holds the half-spectrum in the
/// canonical spectral-buffer layout — the same one scsynth's `FFT` buffer uses:
///
/// ```text
/// frame = [ DC, Nyquist, re₁, im₁, re₂, im₂, …, re_{n/2-1}, im_{n/2-1} ]
/// ```
///
/// The two purely-real terms (DC and Nyquist) share the first two slots; bins
/// `1..n/2` follow as interleaved real/imaginary pairs. Returns `false`,
/// leaving `frame` untouched, on a size mismatch. Zero-allocation.
pub fn rfft_into(input: &[f32], frame: &mut [f32]) -> bool {
    let n = input.len();
    if frame.len() != n {
        return false;
    }
    macro_rules! arm {
        ($size:literal, $f:path) => {{
            let mut buf = [0.0f32; $size];
            buf.copy_from_slice(input);
            let spec = $f(&mut buf);
            // spec[0] packs (DC, Nyquist) in (re, im); spec[1..n/2] are bins.
            frame[0] = spec[0].re;
            frame[1] = spec[0].im;
            for b in 1..$size / 2 {
                frame[2 * b] = spec[b].re;
                frame[2 * b + 1] = spec[b].im;
            }
            true
        }};
    }
    match n {
        256 => arm!(256, real::rfft_256),
        512 => arm!(512, real::rfft_512),
        1024 => arm!(1024, real::rfft_1024),
        2048 => arm!(2048, real::rfft_2048),
        4096 => arm!(4096, real::rfft_4096),
        _ => false,
    }
}

/// Inverse of [`rfft_into`]: the packed complex `frame` back to a real time
/// frame in `output`.
///
/// `output.len()` must be a [supported](SUPPORTED_SIZES) power of two `n` and
/// `frame.len()` must be `n` (the [`rfft_into`] layout). The full length-`n`
/// spectrum is rebuilt from the half-frame by Hermitian symmetry
/// (`X[n-b] = conj(X[b])`), transformed by `microfft::inverse` (already scaled
/// by `1/n`), and its real part written to `output`. Returns `false` on a size
/// mismatch. Zero-allocation: the complex spectrum is a stack buffer.
pub fn irfft_into(frame: &[f32], output: &mut [f32]) -> bool {
    let n = output.len();
    if frame.len() != n {
        return false;
    }
    macro_rules! arm {
        ($size:literal, $f:path) => {{
            let mut spec = [Complex32::new(0.0, 0.0); $size];
            // DC and Nyquist are purely real.
            spec[0] = Complex32::new(frame[0], 0.0);
            spec[$size / 2] = Complex32::new(frame[1], 0.0);
            for b in 1..$size / 2 {
                let c = Complex32::new(frame[2 * b], frame[2 * b + 1]);
                spec[b] = c;
                spec[$size - b] = c.conj();
            }
            let time = $f(&mut spec);
            for (o, c) in output.iter_mut().zip(time.iter()) {
                *o = c.re;
            }
            true
        }};
    }
    match n {
        256 => arm!(256, inverse::ifft_256),
        512 => arm!(512, inverse::ifft_512),
        1024 => arm!(1024, inverse::ifft_1024),
        2048 => arm!(2048, inverse::ifft_2048),
        4096 => arm!(4096, inverse::ifft_4096),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn impulse_is_a_flat_spectrum() {
        // FFT of a unit impulse is all-ones magnitude.
        let mut input = [0.0f32; 256];
        input[0] = 1.0;
        let mut mags = [0.0f32; 128];
        assert!(rfft_magnitudes_into(&input, &mut mags));
        for &m in &mags {
            assert!((m - 1.0).abs() < 1e-4, "flat spectrum, got {m}");
        }
    }

    #[test]
    fn cosine_peaks_at_its_bin() {
        let n = 1024usize;
        let k0 = 40usize;
        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * k0 as f32 * i as f32 / n as f32).cos())
            .collect();
        let mut mags = vec![0.0f32; n / 2];
        assert!(rfft_magnitudes_into(&input, &mut mags));
        let peak = (0..n / 2)
            .max_by(|&a, &b| mags[a].partial_cmp(&mags[b]).unwrap())
            .unwrap();
        assert_eq!(peak, k0, "energy concentrates in the cosine's bin");
    }

    #[test]
    fn forward_inverse_round_trip_reconstructs_the_signal() {
        // A windowed real frame survives rfft_into -> irfft_into intact (no
        // overlap-add: a single frame reconstructs exactly up to FFT rounding).
        let n = 512usize;
        let signal: Vec<f32> = (0..n)
            .map(|i| {
                let x = i as f32 / n as f32;
                0.6 * (2.0 * PI * 5.0 * x).sin() + 0.3 * (2.0 * PI * 17.0 * x).cos()
            })
            .collect();
        let mut frame = vec![0.0f32; n];
        assert!(rfft_into(&signal, &mut frame));
        let mut back = vec![0.0f32; n];
        assert!(irfft_into(&frame, &mut back));
        for (a, b) in signal.iter().zip(back.iter()) {
            assert!((a - b).abs() < 1e-3, "round-trip {a} vs {b}");
        }
    }

    #[test]
    fn packed_dc_and_nyquist_match_the_definition() {
        // DC = sum of samples; Nyquist = alternating-sign sum. Packed in slots
        // 0 and 1 of the frame.
        let n = 256usize;
        let signal: Vec<f32> = (0..n).map(|i| (i % 4) as f32 - 1.5).collect();
        let mut frame = vec![0.0f32; n];
        assert!(rfft_into(&signal, &mut frame));
        let dc: f32 = signal.iter().sum();
        let nyq: f32 = signal
            .iter()
            .enumerate()
            .map(|(i, &s)| if i % 2 == 0 { s } else { -s })
            .sum();
        assert!((frame[0] - dc).abs() < 1e-2, "DC {} vs {dc}", frame[0]);
        assert!(
            (frame[1] - nyq).abs() < 1e-2,
            "Nyquist {} vs {nyq}",
            frame[1]
        );
    }

    #[test]
    fn rejects_unsupported_size_or_bad_output_length() {
        // Not a supported power of two.
        let input = [0.0f32; 100];
        let mut mags = [0.0f32; 50];
        assert!(!rfft_magnitudes_into(&input, &mut mags));
        // Right size, wrong output length.
        let input = [0.0f32; 256];
        let mut wrong = [0.0f32; 100];
        assert!(!rfft_magnitudes_into(&input, &mut wrong));
        assert!(supports(1024) && !supports(1000));
    }
}
