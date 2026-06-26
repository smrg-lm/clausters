//! Forward real FFT shared by the server and the GUI clients.
//!
//! Both the GUI spectrogram (its STFT) and the server's coming `FFT`/`IFFT`
//! UGens need a forward FFT over a power-of-two window. Keeping the algorithm
//! here means **one implementation, identical results**, in the shared core —
//! the rule that an algorithm used by more than one process lives once. It
//! wraps [`microfft`], which is `no_std` and **zero-allocation** with
//! compile-time power-of-two sizes (exactly the STFT's window sizes), so a
//! real-time caller never allocates inside `process` — the property the future
//! UGens require.
//!
//! `microfft` is **forward-only**. An inverse transform (for resynthesis
//! UGens) will pick a crate with that capability behind this same module's API
//! when those UGens land; for now the public surface is the forward magnitude
//! spectrum the spectrogram consumes.

use microfft::real;

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
