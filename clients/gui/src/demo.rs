//! Synthetic test signal shared by the prototype binaries.

use std::f64::consts::PI;

/// Default sample rate for the demo signal.
pub const SAMPLE_RATE: f64 = 48_000.0;

/// A frequency sweep with a slow tremolo envelope plus light noise, so that the
/// waveform shows individual cycles when zoomed in and amplitude bursts when
/// zoomed out, and the spectrogram shows a clear rising ridge. No `rand`
/// dependency: a tiny xorshift suffices.
pub fn sweep(n: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    let mut rng: u64 = 0x2545_F491_4F6C_DD1D;
    for i in 0..n {
        let t = i as f64 / SAMPLE_RATE;
        let f = 80.0 + (4000.0 - 80.0) * (i as f64 / n as f64);
        phase += 2.0 * PI * f / SAMPLE_RATE;
        let env = (0.5 + 0.5 * (2.0 * PI * 0.5 * t).sin()).powi(2);
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let noise = (rng >> 40) as f64 / (1u64 << 24) as f64 - 0.5;
        v.push(((phase.sin() * 0.8 + noise * 0.05) * env) as f32);
    }
    v
}
