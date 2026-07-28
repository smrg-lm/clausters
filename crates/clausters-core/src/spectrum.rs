//! The per-frame spectrum analysis a magnitude display is drawn from: window,
//! forward FFT, and the normalized decibel curve.
//!
//! One step above [`crate::fft`] and [`crate::window`], which give the
//! transform and the coefficients but say nothing about the *scaling* a
//! display reads. That scaling is a choice — divide by the window's coherent
//! gain so a full-scale sine reads ~0 dB at any window size, floor at
//! [`REF_FLOOR`] — and a choice made twice is a choice made differently. So it
//! lives here, shared by the GUI host's `spectrum` widget and by a client
//! computing its own curve from a tap it streams itself.
//!
//! Allocation-free: the caller owns the window, the scratch and the output, so
//! a per-frame caller allocates once and never again. What stays *outside* is
//! everything with memory across frames — the exponential averaging and the
//! decaying peak hold are display smoothing, and belong to whoever draws.

use crate::fft;

/// The decibel floor magnitudes are clamped to, matching the spectrogram's, so
/// a spectrum curve and a spectrogram column of the same audio agree before
/// either applies its own display window.
pub const REF_FLOOR: f32 = -120.0;

/// The coherent gain of an analysis window: what a magnitude is divided by so
/// a full-scale sine reads ~0 dB whatever the window's size or shape.
pub fn coherent_gain(window: &[f32]) -> f32 {
    window.iter().sum::<f32>() * 0.5
}

/// Windows `raw` into `scratch`, transforms it, and writes one floored decibel
/// value per bin into `out_db`.
///
/// `raw`, `window` and `scratch` are all one FFT window long (a supported
/// power of two); `out_db` holds half that many bins. `gain` is the window's
/// [`coherent_gain`], passed in so a repeated caller computes it once.
/// Returns `false` — leaving `out_db` untouched — when the lengths do not
/// agree or the size is unsupported.
pub fn magnitudes_db_into(
    raw: &[f32],
    window: &[f32],
    gain: f32,
    scratch: &mut [f32],
    out_db: &mut [f32],
) -> bool {
    if window.len() != scratch.len() || out_db.len() != scratch.len() / 2 {
        return false;
    }
    for (i, s) in scratch.iter_mut().enumerate() {
        *s = raw.get(i).copied().unwrap_or(0.0) * window[i];
    }
    // The magnitudes land in `out_db` first, then become decibels in place.
    if !fft::rfft_magnitudes_into(scratch, out_db) {
        return false;
    }
    let gain = if gain.abs() > 0.0 { gain } else { 1.0 };
    for db in out_db.iter_mut() {
        let mag = *db / gain;
        *db = (20.0 * (mag + 1e-9).log10()).max(REF_FLOOR);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::Window;

    fn hann(n: usize) -> Vec<f32> {
        let mut w = vec![0.0; n];
        Window::Hann.fill(&mut w);
        w
    }

    #[test]
    fn a_full_scale_sine_reads_about_zero_db_at_its_bin() {
        for n in [256usize, 1024] {
            let bin = 8;
            let raw: Vec<f32> = (0..n)
                .map(|i| (core::f32::consts::TAU * bin as f32 * i as f32 / n as f32).sin())
                .collect();
            let w = hann(n);
            let mut scratch = vec![0.0; n];
            let mut db = vec![0.0; n / 2];
            assert!(magnitudes_db_into(
                &raw,
                &w,
                coherent_gain(&w),
                &mut scratch,
                &mut db
            ));
            assert!(
                (db[bin]).abs() < 0.5,
                "n={n}: bin {bin} reads {} dB, not ~0",
                db[bin]
            );
            // A bin well away from the tone stays far below it.
            assert!(db[bin + 20] < db[bin] - 40.0);
        }
    }

    #[test]
    fn silence_floors_at_the_reference() {
        let n = 256;
        let w = hann(n);
        let mut scratch = vec![0.0; n];
        let mut db = vec![0.0; n / 2];
        assert!(magnitudes_db_into(
            &vec![0.0; n],
            &w,
            coherent_gain(&w),
            &mut scratch,
            &mut db
        ));
        assert!(db.iter().all(|&v| v == REF_FLOOR));
    }

    #[test]
    fn mismatched_lengths_and_unsupported_sizes_report() {
        let w = hann(256);
        let mut scratch = vec![0.0; 256];
        let mut short = vec![0.0; 64];
        assert!(!magnitudes_db_into(
            &[0.0; 256],
            &w,
            1.0,
            &mut scratch,
            &mut short
        ));
        let mut odd_scratch = vec![0.0; 100];
        let mut odd_db = vec![0.0; 50];
        assert!(!magnitudes_db_into(
            &[0.0; 100],
            &vec![1.0; 100],
            1.0,
            &mut odd_scratch,
            &mut odd_db
        ));
    }
}
