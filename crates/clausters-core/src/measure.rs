//! Signal measurements and stereo-field geometry shared by the server and the
//! GUI clients.
//!
//! These are general audio tools — not display code — so, by the rule that an
//! algorithm useful to more than one Clausters process lives once in the shared
//! core, they belong here rather than in any single client. Two land here, both
//! read by the GUI's phasescope and both useful well beyond it (a headless
//! Python capture, a future server analysis UGen, an electroacoustic-composition
//! sketch that plots or drives from a stereo field):
//!
//! - [`correlation`] — the stereo **correlation** (Pearson's r of the two
//!   channels), the phase-coherence number a goniometer annotates.
//! - [`channel_stats`] — the **peak and RMS** of one channel of an interleaved
//!   buffer, the pair a render reports back so no client writes the loop.
//! - [`lissajous_point`] / [`lissajous_into`] — the **Lissajous / goniometer**
//!   transform: a stereo `(L, R)` pair mapped to the 45°-rotated mid/side plane
//!   the classic goniometer draws. It is the shape an audio engineer or
//!   electroacoustic composer reads a stereo image from, so the geometry lives
//!   here once rather than only inside the GUI's drawing code.
//!
//! `no_std`-friendly and allocation-free; each function is a single pass over
//! the input slices.
//!
//! `correlation` and the Lissajous transform have no FFI export: the phasescope
//! computes them host-side (native and wasm both link this crate directly), and
//! no non-Rust client consumes them yet. The export follows the concrete
//! consumer, the way `peaks` grew one only when the Python client needed to
//! build the identical cache — and the way `channel_stats`, which the Python
//! client reads off every render, has one.

/// Pearson's correlation coefficient of two equal-length signals, in `[-1, 1]`.
///
/// This is the audio-engineering **stereo correlation** metric: `+1` when the
/// two channels are identical (a mono/in-phase signal), `0` when they are
/// decorrelated (a wide stereo field), `-1` when one is the negation of the
/// other (anti-phase — the mix cancels in mono). It is computed about each
/// channel's own mean, so a DC offset does not bias it.
///
/// Returns `None` when the inputs differ in length, are empty, or either
/// channel is constant over the window (a zero variance makes the coefficient
/// undefined — silence or pure DC, which the caller shows as "no reading"). The
/// result is clamped to `[-1, 1]` against rounding error.
pub fn correlation(x: &[f32], y: &[f32]) -> Option<f32> {
    if x.is_empty() || x.len() != y.len() {
        return None;
    }
    let n = x.len() as f64;
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    for (&a, &b) in x.iter().zip(y) {
        sx += a as f64;
        sy += b as f64;
    }
    let (mx, my) = (sx / n, sy / n);
    let (mut cov, mut vx, mut vy) = (0.0f64, 0.0f64, 0.0f64);
    for (&a, &b) in x.iter().zip(y) {
        let (dx, dy) = (a as f64 - mx, b as f64 - my);
        cov += dx * dy;
        vx += dx * dx;
        vy += dy * dy;
    }
    if vx <= 0.0 || vy <= 0.0 {
        return None; // a constant channel: correlation is undefined
    }
    Some(((cov / (vx * vy).sqrt()) as f32).clamp(-1.0, 1.0))
}

/// The Lissajous / goniometer coordinate of one stereo sample pair.
///
/// The audio-engineering goniometer plots the stereo signal as a Lissajous
/// figure rotated 45° into the **mid/side** plane, so a mono signal reads as a
/// vertical line and an anti-phase one as horizontal:
///
/// - `x` (horizontal) is the **side** component `(L − R) / √2` — the stereo
///   width;
/// - `y` (vertical) is the **mid** component `(L + R) / √2` — the mono sum.
///
/// The `1/√2` keeps the transform an isometry (a hard-panned channel reaches
/// the same distance from the origin as a centered one of equal level), so the
/// figure's shape is read directly. Returned as `[x, y]`.
pub fn lissajous_point(left: f32, right: f32) -> [f32; 2] {
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    [(left - right) * INV_SQRT2, (left + right) * INV_SQRT2]
}

/// Maps a block of stereo pairs to their Lissajous / goniometer coordinates.
///
/// `left`, `right` and `out` must have the same length; `out[i]` receives
/// [`lissajous_point`]`(left[i], right[i])` (`[x, y]` = side, mid). Returns
/// `false`, leaving `out` untouched, on a length mismatch. Allocation-free — the
/// caller owns `out` — so a real-time or per-frame caller reuses one buffer.
pub fn lissajous_into(left: &[f32], right: &[f32], out: &mut [[f32; 2]]) -> bool {
    if left.len() != right.len() || out.len() != left.len() {
        return false;
    }
    for (o, (&l, &r)) in out.iter_mut().zip(left.iter().zip(right)) {
        *o = lissajous_point(l, r);
    }
    true
}

/// Peak magnitude and RMS of one channel of an **interleaved** buffer.
///
/// `samples` is the whole interleaved frame sequence, `channels` its channel
/// count and `channel` the one to measure; the stride walk is what lets a
/// caller measure without deinterleaving first. Returns `(peak, rms)`, both
/// `0.0` for an empty or out-of-range request.
///
/// This is the measurement a render reports back: the peak answers "did it
/// clip", the RMS answers "how loud is it", and both are one pass over data
/// the renderer has already produced, so no caller needs its own loop.
pub fn channel_stats(samples: &[f32], channels: usize, channel: usize) -> (f32, f32) {
    if channels == 0 || channel >= channels || samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut peak = 0.0f32;
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for &s in samples.iter().skip(channel).step_by(channels) {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        sum += (s as f64) * (s as f64);
        n += 1;
    }
    if n == 0 {
        return (peak, 0.0);
    }
    (peak, (sum / n as f64).sqrt() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_maps_to_the_vertical_axis() {
        // Identical channels (mono): side is 0, the figure is a vertical line.
        let [x, y] = lissajous_point(0.7, 0.7);
        assert!(x.abs() < 1e-6, "mono has no side component");
        assert!((y - 0.7 * std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn anti_phase_maps_to_the_horizontal_axis() {
        let [x, y] = lissajous_point(0.7, -0.7);
        assert!(y.abs() < 1e-6, "anti-phase has no mid component");
        assert!((x - 0.7 * std::f32::consts::SQRT_2).abs() < 1e-6);
    }

    #[test]
    fn a_hard_panned_channel_keeps_its_magnitude() {
        // The rotation is an isometry: |[x, y]| == |[L, R]|.
        let panned = lissajous_point(1.0, 0.0);
        let mag = (panned[0] * panned[0] + panned[1] * panned[1]).sqrt();
        assert!((mag - 1.0).abs() < 1e-6, "isometric, got {mag}");
    }

    #[test]
    fn lissajous_into_matches_the_pointwise_form() {
        let l = [0.1f32, -0.4, 0.9];
        let r = [0.2f32, 0.5, -0.3];
        let mut out = [[0.0f32; 2]; 3];
        assert!(lissajous_into(&l, &r, &mut out));
        for i in 0..3 {
            assert_eq!(out[i], lissajous_point(l[i], r[i]));
        }
        // A length mismatch is rejected, out untouched.
        let mut short = [[0.0f32; 2]; 2];
        assert!(!lissajous_into(&l, &r, &mut short));
    }

    #[test]
    fn identical_channels_are_perfectly_correlated() {
        let x: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).sin()).collect();
        let r = correlation(&x, &x).unwrap();
        assert!((r - 1.0).abs() < 1e-5, "mono reads +1, got {r}");
    }

    #[test]
    fn negated_channel_is_anti_correlated() {
        let x: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).sin()).collect();
        let neg: Vec<f32> = x.iter().map(|s| -s).collect();
        let r = correlation(&x, &neg).unwrap();
        assert!((r + 1.0).abs() < 1e-5, "anti-phase reads -1, got {r}");
    }

    #[test]
    fn orthogonal_channels_are_uncorrelated() {
        // A sine and a cosine of the same frequency over whole periods are
        // decorrelated: r ~ 0.
        let n = 400;
        let x: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * i as f32 / 100.0).sin())
            .collect();
        let y: Vec<f32> = (0..n)
            .map(|i| (std::f32::consts::TAU * i as f32 / 100.0).cos())
            .collect();
        let r = correlation(&x, &y).unwrap();
        assert!(r.abs() < 0.05, "quadrature reads ~0, got {r}");
    }

    #[test]
    fn a_dc_offset_does_not_bias_it() {
        let x: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).sin()).collect();
        let shifted: Vec<f32> = x.iter().map(|s| s + 5.0).collect();
        let r = correlation(&x, &shifted).unwrap();
        assert!((r - 1.0).abs() < 1e-5, "correlation is mean-centered");
    }

    #[test]
    fn degenerate_inputs_return_none() {
        assert_eq!(correlation(&[], &[]), None, "empty");
        assert_eq!(correlation(&[1.0, 2.0], &[1.0]), None, "length mismatch");
        assert_eq!(
            correlation(&[0.7, 0.7, 0.7], &[0.1, 0.2, 0.3]),
            None,
            "a constant channel has undefined correlation"
        );
    }
}

#[cfg(test)]
mod stats_tests {
    use super::channel_stats;

    #[test]
    fn peak_and_rms_walk_one_channel_of_an_interleaved_buffer() {
        // L is a square at +-1 (peak 1, rms 1); R is constant 0.5.
        let buf = [1.0, 0.5, -1.0, 0.5, 1.0, 0.5, -1.0, 0.5];
        let (peak_l, rms_l) = channel_stats(&buf, 2, 0);
        let (peak_r, rms_r) = channel_stats(&buf, 2, 1);
        assert_eq!(peak_l, 1.0);
        assert!((rms_l - 1.0).abs() < 1e-6, "rms {rms_l}");
        assert_eq!(peak_r, 0.5);
        assert!((rms_r - 0.5).abs() < 1e-6, "rms {rms_r}");
    }

    #[test]
    fn an_impossible_request_reads_zero_rather_than_panicking() {
        let buf = [1.0, 2.0];
        assert_eq!(channel_stats(&buf, 2, 2), (0.0, 0.0)); // channel out of range
        assert_eq!(channel_stats(&buf, 0, 0), (0.0, 0.0)); // no channels
        assert_eq!(channel_stats(&[], 2, 0), (0.0, 0.0)); // no samples
    }
}

/// **A meter's ballistics**: instant attack, a declared fall, and a peak that
/// stays put long enough to be read.
///
/// A meter that drew the raw peak of each block would be unreadable: it
/// flickers, and a transient shows for one frame of the screen or none at all.
/// Every meter anybody has ever read answers the same three rules instead, and
/// they are here rather than in a drawing routine because two clients drawing
/// two different falls off one signal is two answers to a question that has one.
///
/// - **The attack is instantaneous.** A meter that smoothed its way up would
///   under-read exactly the thing it exists to catch.
/// - **The fall is a fixed number of decibels per second**, so the slope on
///   screen is the same whatever the level — which is what makes the picture
///   readable as a rate rather than as a shape.
/// - **A peak is held** for a declared time and then falls at the same rate, so
///   the mark is still there when an eye gets to it.
///
/// One state per channel; [`Ballistics::tick`] takes the peak of a block and
/// the seconds it lasted, and answers what to show. It allocates nothing and
/// branches on nothing but its own state, so the audio thread may call it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ballistics {
    /// What is shown now, in linear amplitude.
    level: f32,
    /// How long the current value has been held, in seconds.
    held: f32,
}

impl Ballistics {
    /// A meter showing silence.
    pub fn new() -> Self {
        Self::default()
    }

    /// What is shown now.
    pub fn level(self) -> f32 {
        self.level
    }

    /// Advances by `seconds` against a block whose peak was `peak`.
    ///
    /// `decay_db` is the fall in decibels per second (the field's value for a
    /// peak meter is about 20) and `hold` the seconds a new peak stays before
    /// it begins to fall. `hold` of zero is the ordinary meter; a second or two
    /// is the mark that waits to be read.
    pub fn tick(&mut self, peak: f32, seconds: f32, decay_db: f32, hold: f32) -> f32 {
        let peak = peak.abs();
        if peak >= self.level {
            // Instantaneous attack, and the hold starts again from here.
            self.level = peak;
            self.held = 0.0;
            return self.level;
        }
        self.held += seconds;
        if self.held < hold {
            return self.level;
        }
        // `x dB` of fall is a factor of `10^(-x/20)`; over `seconds` at
        // `decay_db` per second that is `10^(-decay_db * seconds / 20)`.
        let factor = powf10(-decay_db * seconds / 20.0);
        self.level = (self.level * factor).max(peak);
        self.level
    }
}

/// **Where a meter's floor is**, in decibels. Sixty below unity is the field's
/// span for a peak meter: quiet enough that a fade reads as a fade to the end,
/// short enough that the loud half of the scale keeps most of the strip.
pub const METER_FLOOR_DB: f32 = -60.0;

/// **How high a column stands for an amplitude**, over `0..1`.
///
/// A meter is read in decibels and not in amplitude, and the difference is the
/// whole of whether it is legible: half of unity is -6 dB, which is a tenth of
/// the way down a 60 dB strip and not half of it, and a linear column spends
/// nine tenths of its height on the top 20 dB of a signal nobody mixes in.
///
/// Here rather than in whoever paints, for the reason [`Ballistics`] is here:
/// the same amplitude has to stand the same height in every window that draws
/// it, and a mapping written twice is two answers to one question.
pub fn meter_fraction(amplitude: f32, floor_db: f32) -> f32 {
    let amplitude = amplitude.abs();
    let floor = floor_db.min(-1.0);
    if amplitude <= 0.0 {
        return 0.0;
    }
    // 20·log10(a), which is `ln(a) / ln(10) · 20`, against a floor that is the
    // bottom of the strip.
    let db = 20.0 * amplitude.ln() / core::f32::consts::LN_10;
    ((db - floor) / -floor).clamp(0.0, 1.0)
}

/// `10^x`, written as `exp(x · ln 10)` so the constant is visible rather than
/// hidden inside a `powf` whose base nobody can see.
#[inline]
fn powf10(x: f32) -> f32 {
    (x * core::f32::consts::LN_10).exp()
}

#[cfg(test)]
mod ballistics_tests {
    use super::*;

    /// **A meter is read in decibels**: unity is the top, the floor is the
    /// bottom, and half the amplitude is a tenth of the way down rather than
    /// halfway.
    #[test]
    fn a_column_stands_where_the_decibels_say() {
        assert_eq!(meter_fraction(1.0, METER_FLOOR_DB), 1.0);
        assert_eq!(
            meter_fraction(0.0, METER_FLOOR_DB),
            0.0,
            "silence is the floor"
        );
        let half = meter_fraction(0.5, METER_FLOOR_DB);
        assert!(
            (half - 0.9).abs() < 0.01,
            "-6 dB of 60 is nine tenths up: {half}"
        );
        assert_eq!(
            meter_fraction(2.0, METER_FLOOR_DB),
            1.0,
            "and past unity it is the top, not past it"
        );
    }

    /// **It rises at once and falls at the rate it was given.** Twenty decibels
    /// a second means a tenth of the amplitude after one second, whatever the
    /// level was.
    #[test]
    fn it_attacks_instantly_and_falls_at_the_declared_rate() {
        let mut meter = Ballistics::new();
        assert_eq!(meter.tick(0.8, 0.001, 20.0, 0.0), 0.8, "up in one block");
        // A second of silence at 20 dB/s.
        for _ in 0..100 {
            meter.tick(0.0, 0.01, 20.0, 0.0);
        }
        let after = meter.level();
        assert!(
            (after - 0.08).abs() < 0.005,
            "a tenth of the amplitude after a second: {after}"
        );
    }

    /// **A peak stays put long enough to be read**, and then falls like
    /// everything else — which is the difference between a mark and a flicker.
    #[test]
    fn a_held_peak_waits_before_it_falls() {
        let mut meter = Ballistics::new();
        meter.tick(1.0, 0.01, 20.0, 1.0);
        for _ in 0..50 {
            meter.tick(0.0, 0.01, 20.0, 1.0);
        }
        assert_eq!(meter.level(), 1.0, "half a second in, it has not moved");
        for _ in 0..100 {
            meter.tick(0.0, 0.01, 20.0, 1.0);
        }
        assert!(meter.level() < 1.0, "past the hold it falls");
    }

    /// **A louder block wins immediately, however far the meter had fallen** —
    /// the attack is the one rule with no exception.
    #[test]
    fn a_new_peak_takes_it_back_at_once() {
        let mut meter = Ballistics::new();
        meter.tick(1.0, 0.01, 20.0, 0.0);
        for _ in 0..200 {
            meter.tick(0.0, 0.01, 20.0, 0.0);
        }
        assert!(meter.level() < 0.2);
        assert_eq!(meter.tick(0.5, 0.01, 20.0, 0.0), 0.5);
    }
}
