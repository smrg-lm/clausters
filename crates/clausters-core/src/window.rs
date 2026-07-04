//! Smoothing windows shared by the server's FFT chain and the clients.
//!
//! An analysis/synthesis window is applied before a forward FFT (and again on
//! overlap-add resynthesis). Because both the server's `FFT`/`IFFT` UGens and a
//! client that pre-analyses audio off-line must agree **bit for bit** on the
//! window shape, the coefficients live here in the shared core — the same rule
//! that keeps the [`fft`](crate::fft) algorithm single-sourced. `no_std`-friendly
//! and allocation-free: [`Window::fill`] writes into a caller-provided slice, so
//! a real-time caller fills a window once at synth init and never again.
//!
//! The windows are **periodic** (DFT-even: the divisor is `n`, not `n - 1`), the
//! correct form for spectral analysis and overlap-add, where the window tiles
//! the signal rather than tapering a single isolated frame.

use std::f32::consts::PI;

/// Smoothing window shapes, selected on the wire by the integer `wintype` an
/// `FFT`/`IFFT` carries. The values match scsynth's convention where it has one
/// (`-1` rectangular, `0` the default), and extend it with the other classic
/// windows. [`Hann`](Window::Hann) is the default — a good general analysis
/// window with well-behaved overlap-add at 50% hop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum Window {
    /// No windowing (all ones). Leaks heavily; useful only for block-aligned
    /// content or measurement.
    Rectangular = -1,
    /// Hann (raised cosine). The default.
    #[default]
    Hann = 0,
    /// Sine (cosine) window — the square root of Hann; power-complementary at
    /// 50% overlap.
    Sine = 1,
    /// Welch (parabolic) window.
    Welch = 2,
    /// Hamming (raised cosine with a pedestal).
    Hamming = 3,
    /// Blackman (three-term cosine); lowest side lobes of the set.
    Blackman = 4,
}

impl Window {
    /// Maps the wire `wintype` integer to a window; unknown values fall back to
    /// the default [`Hann`](Window::Hann), matching how scsynth treats an
    /// out-of-range window type as its default rather than an error.
    pub fn from_wintype(wintype: i32) -> Window {
        match wintype {
            -1 => Window::Rectangular,
            0 => Window::Hann,
            1 => Window::Sine,
            2 => Window::Welch,
            3 => Window::Hamming,
            4 => Window::Blackman,
            _ => Window::Hann,
        }
    }

    /// The wire `wintype` integer for this window.
    pub fn wintype(self) -> i32 {
        self as i32
    }

    /// Fills `out` with this window's coefficients (`out.len()` samples). The
    /// window is periodic in `out.len()`. Allocation-free.
    pub fn fill(self, out: &mut [f32]) {
        let n = out.len();
        if n == 0 {
            return;
        }
        let nf = n as f32;
        for (i, w) in out.iter_mut().enumerate() {
            let x = i as f32;
            *w = match self {
                Window::Rectangular => 1.0,
                Window::Hann => 0.5 - 0.5 * (2.0 * PI * x / nf).cos(),
                Window::Sine => (PI * x / nf).sin(),
                Window::Welch => {
                    let t = (x - 0.5 * (nf - 1.0)) / (0.5 * (nf - 1.0));
                    1.0 - t * t
                }
                Window::Hamming => 0.54 - 0.46 * (2.0 * PI * x / nf).cos(),
                Window::Blackman => {
                    0.42 - 0.5 * (2.0 * PI * x / nf).cos() + 0.08 * (4.0 * PI * x / nf).cos()
                }
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_is_symmetric_and_bounded() {
        let mut w = [0.0f32; 64];
        Window::Hann.fill(&mut w);
        assert!((w[0]).abs() < 1e-6, "periodic Hann starts at 0");
        // Symmetry about the centre (periodic window: w[i] == w[n-i]).
        for i in 1..32 {
            assert!((w[i] - w[64 - i]).abs() < 1e-6);
        }
        for &v in &w {
            assert!((0.0..=1.0).contains(&v));
        }
    }

    #[test]
    fn rectangular_is_all_ones() {
        let mut w = [0.0f32; 16];
        Window::Rectangular.fill(&mut w);
        assert!(w.iter().all(|&v| v == 1.0));
    }

    #[test]
    fn from_wintype_round_trips_and_defaults() {
        for wt in [-1, 0, 1, 2, 3, 4] {
            assert_eq!(Window::from_wintype(wt).wintype(), wt);
        }
        assert_eq!(Window::from_wintype(999), Window::Hann);
    }

    #[test]
    fn sine_is_sqrt_of_hann() {
        let (mut s, mut h) = ([0.0f32; 128], [0.0f32; 128]);
        Window::Sine.fill(&mut s);
        Window::Hann.fill(&mut h);
        for i in 0..128 {
            assert!((s[i] * s[i] - h[i]).abs() < 1e-5, "sine^2 == hann at {i}");
        }
    }
}
