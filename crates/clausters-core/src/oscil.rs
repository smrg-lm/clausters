//! The audio-rate oscilloscope's signal logic: window sizing and trigger
//! alignment. Pure — no GPU, no shared memory — so it is unit-testable and
//! shared by every drawer of a triggered trace: the GUI host's two fronts (the
//! only difference between them being where the raw tap samples come from — the
//! shm segment vs `/bus_tapStream.reply` snapshots), and a client script drawing its own
//! oscilloscope from a tap it streams itself.
//!
//! It lives here for the reason [`crate::fft`] does: the moment a second
//! process computes the same trace, the algorithm has to be the one algorithm,
//! or the two draw subtly different pictures of one signal.

/// Cap on the display window in samples: half the default tap ring (the
/// tear-free read bound) and the server's `/bus_tapStream` window cap, so the
/// same widget works over both sources.
pub const MAX_DISPLAY: usize = 4096;

/// Display window in samples for `window_ms` at `sample_rate` (falling back
/// to 48 kHz before the rate is known), clamped to a sane interactive range.
pub fn display_frames(window_ms: f32, sample_rate: f64) -> usize {
    let sr = if sample_rate > 0.0 {
        sample_rate
    } else {
        48_000.0
    };
    let frames = (window_ms.max(0.1) as f64 / 1000.0 * sr) as usize;
    frames.clamp(16, MAX_DISPLAY)
}

/// How many raw samples one display window needs: a full window of slack
/// before it, so the trigger search has somewhere to look.
pub fn raw_frames(display: usize) -> usize {
    display * 2
}

/// Start index of the triggered display window inside `raw`, and whether the
/// trigger actually fired (`true` = locked; the read-out the scope shows).
/// The start is the **latest** rising crossing of `level` that still leaves a
/// full `display` window after it. The trigger re-arms only after the signal
/// dips below `level` minus a hysteresis of 2% of the window's peak-to-peak,
/// so noise riding on the level does not fire mid-cycle. Falls back to the
/// newest window (free-run, `false`) when no crossing exists — silence, DC,
/// or a window without a rising edge — so the scope always draws something.
pub fn align(raw: &[f32], display: usize, level: f32) -> (usize, bool) {
    if raw.len() <= display {
        return (0, false);
    }
    let newest = raw.len() - display;
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &s in raw {
        lo = lo.min(s);
        hi = hi.max(s);
    }
    let arm = level - ((hi - lo) * 0.02).max(1e-6);
    let mut armed = false;
    let mut found = None;
    for (i, &s) in raw.iter().enumerate() {
        if armed && s >= level {
            if i <= newest {
                found = Some(i);
            }
            armed = false;
        }
        if s < arm {
            armed = true;
        }
    }
    (found.unwrap_or(newest), found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(len: usize, period: usize, phase: f32) -> Vec<f32> {
        (0..len)
            .map(|i| (core::f32::consts::TAU * (i as f32 / period as f32) + phase).sin())
            .collect()
    }

    #[test]
    fn window_sizing_clamps() {
        assert_eq!(display_frames(20.0, 48_000.0), 960);
        assert_eq!(display_frames(20.0, 0.0), 960, "unknown rate assumes 48k");
        assert_eq!(display_frames(0.0, 48_000.0), 16, "floor");
        assert_eq!(display_frames(10_000.0, 48_000.0), MAX_DISPLAY, "cap");
        assert_eq!(raw_frames(960), 1920);
    }

    #[test]
    fn trigger_aligns_a_sine_regardless_of_phase() {
        // Two captures of the same sine at arbitrary phases: after alignment
        // the display windows show the same waveform.
        let display = 256;
        let a = sine(raw_frames(display), 128, 0.3);
        let b = sine(raw_frames(display), 128, 2.1);
        let ((sa, la), (sb, lb)) = (align(&a, display, 0.0), align(&b, display, 0.0));
        assert!(la && lb, "a periodic signal locks");
        for i in 0..display {
            assert!(
                (a[sa + i] - b[sb + i]).abs() < 0.06,
                "sample {i}: {} vs {}",
                a[sa + i],
                b[sb + i]
            );
        }
        // Both windows start at the rising zero crossing.
        assert!(a[sa] >= 0.0 && a[sa] < 0.1, "starts at the crossing");
        assert!(a[sa + 1] > a[sa], "and rising");
    }

    #[test]
    fn trigger_prefers_the_latest_crossing() {
        // A crossing exists early and late; the late one (still leaving a
        // full window) must win, so the display shows the freshest data.
        let display = 64;
        let mut raw = vec![0.0f32; raw_frames(display)];
        // Dips and rises at 10 and at 60 (both <= newest = 64).
        for (at, _) in [(10usize, ()), (60, ())] {
            raw[at - 2] = -0.5;
            raw[at - 1] = -0.5;
            raw[at] = 0.5;
        }
        assert_eq!(align(&raw, display, 0.0), (60, true));
    }

    #[test]
    fn silence_and_dc_free_run_at_the_newest_window() {
        let display = 64;
        let silent = vec![0.0f32; raw_frames(display)];
        assert_eq!(align(&silent, display, 0.0), (display, false), "newest");
        let dc = vec![0.7f32; raw_frames(display)];
        assert_eq!(align(&dc, display, 0.0), (display, false));
        let short = vec![0.0f32; display / 2];
        assert_eq!(align(&short, display, 0.0), (0, false), "short data at 0");
    }
}
