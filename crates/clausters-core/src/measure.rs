//! Signal measurements and stereo-field geometry shared by the server and the
//! GUI clients.
//!
//! These are general audio tools -- not display code -- so, by the rule that an
//! algorithm useful to more than one Clausters process lives once in the shared
//! core, they belong here rather than in any single client. Two land here, both
//! read by the GUI's phasescope and both useful well beyond it (a headless
//! Python capture, a future server analysis UGen, an electroacoustic-composition
//! sketch that plots or drives from a stereo field):
//!
//! - [`correlation`] -- the stereo **correlation** (Pearson's r of the two
//!   channels), the phase-coherence number a goniometer annotates.
//! - [`channel_stats`] -- the **peak and RMS** of one channel of an interleaved
//!   buffer, the pair a render reports back so no client writes the loop.
//! - [`lissajous_point`] / [`lissajous_into`] -- the **Lissajous / goniometer**
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
//! build the identical cache -- and the way `channel_stats`, which the Python
//! client reads off every render, has one.

/// Pearson's correlation coefficient of two equal-length signals, in `[-1, 1]`.
///
/// This is the audio-engineering **stereo correlation** metric: `+1` when the
/// two channels are identical (a mono/in-phase signal), `0` when they are
/// decorrelated (a wide stereo field), `-1` when one is the negation of the
/// other (anti-phase -- the mix cancels in mono). It is computed about each
/// channel's own mean, so a DC offset does not bias it.
///
/// Returns `None` when the inputs differ in length, are empty, or either
/// channel is constant over the window (a zero variance makes the coefficient
/// undefined -- silence or pure DC, which the caller shows as "no reading"). The
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
/// - `x` (horizontal) is the **side** component `(L − R) / √2` -- the stereo
///   width;
/// - `y` (vertical) is the **mid** component `(L + R) / √2` -- the mono sum.
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
/// `false`, leaving `out` untouched, on a length mismatch. Allocation-free -- the
/// caller owns `out` -- so a real-time or per-frame caller reuses one buffer.
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
///   screen is the same whatever the level -- which is what makes the picture
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

/// **How fast a meter falls**, in decibels per second.
///
/// Twenty is the field's number for a peak meter, and it is the one number that
/// makes a meter legible as a *rate*: the slope on screen is the same whatever
/// the level, so an eye reads how fast a sound is dying rather than a shape.
/// The server publishes its bus levels already held at this rate, the `Meter`
/// UGen defaults to it, and a mark drawn in a window falls at it -- which is
/// the whole reason it is here and not three times over.
pub const METER_FALL_DB: f32 = 20.0;

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
    if amplitude <= 0.0 {
        return 0.0;
    }
    // 20·log10(a), which is `ln(a) / ln(10) · 20`.
    meter_fraction_db(20.0 * amplitude.ln() / core::f32::consts::LN_10, floor_db)
}

/// The same height, for a level already **in decibels** -- which is how the
/// marks on the scale are stated.
///
/// The two doors are one mapping: whoever draws the scale asks where -18 dB
/// falls, whoever draws the column asks where this block's peak falls, and a
/// column that stood at a different place from its own mark would be a picture
/// of nothing.
pub fn meter_fraction_db(db: f32, floor_db: f32) -> f32 {
    let floor = floor_db.min(-1.0);
    ((db - floor) / -floor).clamp(0.0, 1.0)
}

/// **Where a meter stops reading as headroom**, in decibels below full scale.
///
/// -18 dBFS is the field's alignment level -- what a nominal signal sits at, so
/// that the peaks above it have somewhere to go. Below it a meter is showing
/// a level that is *working*, and the colour says so.
pub const METER_WARN_DB: f32 = -18.0;

/// **Where a meter's column is fully amber**, in decibels below full scale.
///
/// The third mark, and the one that makes the other two readable. Green ends at
/// the alignment level ([`METER_WARN_DB`]) and red begins in the last six
/// ([`METER_HOT_DB`]); a single ramp between them would spend the whole span
/// between -18 and -6 getting there, so a signal at -12 -- which is a signal
/// **using its headroom**, the thing the colour exists to say -- still read as
/// green with a cast on it. So the amber is reached here and held until the
/// red: the bands are bands, and only the edges between them are ramps.
pub const METER_AMBER_DB: f32 = -12.0;

/// **Where a meter is warning**, in decibels below full scale.
///
/// The last six decibels before full scale: not clipping, which is a fact the
/// meter states by reaching the top, but the span where a peak that grows any
/// further will. A scale coloured at these two marks is read as three bands
/// without anybody reading a number -- which is the whole use of a meter at a
/// glance and the reason the two live here and not in a painter.
pub const METER_HOT_DB: f32 = -6.0;

/// The amplitude a level in decibels is, the inverse of the reading a meter
/// does -- what places [`METER_WARN_DB`] and [`METER_HOT_DB`] on a scale that
/// is drawn in amplitude rather than in decibels.
pub fn amplitude_of_db(db: f32) -> f32 {
    powf10(db / 20.0)
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

    /// **The scale's marks are placed by the same mapping as the column.** A
    /// meter whose colours changed at one height and whose level stood at
    /// another would be two scales in one strip.
    #[test]
    fn the_marks_stand_where_their_own_levels_do() {
        for db in [METER_WARN_DB, METER_HOT_DB, -3.0, -40.0] {
            let by_db = meter_fraction_db(db, METER_FLOOR_DB);
            let by_amplitude = meter_fraction(amplitude_of_db(db), METER_FLOOR_DB);
            assert!(
                (by_db - by_amplitude).abs() < 1e-5,
                "{db} dB: {by_db} against {by_amplitude}"
            );
        }
        assert!(
            meter_fraction_db(METER_WARN_DB, METER_FLOOR_DB)
                < meter_fraction_db(METER_HOT_DB, METER_FLOOR_DB),
            "and the warning is under the hot end"
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
    /// everything else -- which is the difference between a mark and a flicker.
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

    /// **A louder block wins immediately, however far the meter had fallen** --
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

// ---- clipping: what counts as an over, and the mark that stays ----

/// **How many consecutive samples at or over full scale count as an over.**
///
/// A single sample at full scale is not clipping. The engine runs in `f32`, so
/// a sample at 1.0 -- or past it -- destroys nothing until the signal is
/// converted to an integer format or reaches a converter; what a meter's red
/// mark is actually reporting is a waveform that was **flattened**, and the
/// signature of that is a *run*. Three is the field's usual number (a hardware
/// console's over lamp, and a DAW's default); one is the pessimistic reading a
/// small meter takes, and it is why a mastered take that legitimately touches
/// full scale lights every meter it is played on.
///
/// Inter-sample peaks are a different measurement and not a different
/// threshold: true peak (ITU-R BS.1770) oversamples and reads against -1 dBTP,
/// which is not this.
pub const CLIP_RUN: u32 = 3;

/// **Where full scale is**, in linear amplitude: the level a run of samples at
/// or above counts as an over.
pub const CLIP_CEILING: f32 = 1.0;

/// **Counting overs, sample by sample** -- the rule a meter's red mark reports.
///
/// A run of `run` consecutive samples at or above `ceiling` is **one** over,
/// however long the run goes on: a flattened peak is one event, not one per
/// sample, or a second of square wave would report forty thousand of them. The
/// count starts again when the run breaks.
///
/// It holds two integers and branches on nothing else, so the audio thread may
/// feed it sample by sample.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClipCount {
    /// Samples at or above the ceiling since the last one below it.
    run: u32,
    /// Whether the current run has already been counted.
    counted: bool,
    /// Overs since the last [`ClipCount::reset`].
    overs: u32,
}

impl ClipCount {
    /// A counter that has seen nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Overs since the counter was last reset.
    pub fn overs(self) -> u32 {
        self.overs
    }

    /// Forgets the count and the run in progress -- a new pass of the
    /// transport, which is one of the two things that clears a meter's mark.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feeds one sample. Returns `true` on the sample that **completes** an
    /// over, which is the one that took the run to `run`.
    ///
    /// A `run` of zero is read as one, so a caller that wants the pessimistic
    /// meter cannot accidentally ask for a rule that counts silence.
    pub fn feed(&mut self, sample: f32, ceiling: f32, run: u32) -> bool {
        if sample.abs() < ceiling {
            self.run = 0;
            self.counted = false;
            return false;
        }
        self.run = self.run.saturating_add(1);
        if self.counted || self.run < run.max(1) {
            return false;
        }
        self.counted = true;
        self.overs = self.overs.saturating_add(1);
        true
    }

    /// Feeds a whole block, returning how many overs it completed.
    pub fn feed_block(&mut self, block: &[f32], ceiling: f32, run: u32) -> u32 {
        let mut overs = 0;
        for &sample in block {
            if self.feed(sample, ceiling, run) {
                overs += 1;
            }
        }
        overs
    }
}

/// **The mark that stays**: a meter's clip indication, which is latched rather
/// than shown while it lasts.
///
/// An over is a handful of samples and a meter is read by a person, so a red
/// mark that lasted as long as the event would be a mark nobody ever saw. It
/// stays lit until one of two things happens -- it is [`cleared`](ClipLatch::clear)
/// by hand, or the count it is watching **goes backwards**, which is what a new
/// pass of the transport looks like from here.
///
/// While lit it also keeps the **loudest level seen since it lit**, which is
/// what the number inside the mark says: not that something clipped, which the
/// colour already said, but by how much.
///
/// The state is the reader's, not the signal's -- two windows watching one bus
/// each have their own mark and clear them separately, the way two readers of
/// one level each have their own eyes.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClipLatch {
    lit: bool,
    max: f32,
    /// The over count as last read, to notice both a rise and a reset.
    seen: Option<f32>,
}

impl ClipLatch {
    /// A mark that is not lit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the mark is lit.
    pub fn lit(self) -> bool {
        self.lit
    }

    /// The loudest level seen since it lit, in linear amplitude (`0.0` when it
    /// is not lit).
    pub fn max(self) -> f32 {
        self.max
    }

    /// Puts the mark out and forgets its level -- the hand's verb, and the
    /// only one there is.
    pub fn clear(&mut self) {
        self.lit = false;
        self.max = 0.0;
    }

    /// Advances against what this frame read.
    ///
    /// `level` is what the meter reads now and `overs` the count from a
    /// counting source ([`ClipCount`] on a control bus), when there is one. A
    /// rise in the count lights the mark; a **fall** clears it, since a count
    /// that went backwards is a counter that started again. With no count the
    /// mark lights on the level alone reaching `ceiling`, which is exact for
    /// *exceeding* full scale and cannot tell one sample from a run -- the
    /// reason the counting source exists.
    pub fn tick(&mut self, level: f32, overs: Option<f32>, ceiling: f32) {
        match overs {
            Some(count) => {
                match self.seen {
                    Some(seen) if count > seen => self.lit = true,
                    // A count that went backwards is a new pass, and a new pass
                    // is the other thing that clears the mark.
                    Some(seen) if count < seen => self.clear(),
                    _ => {}
                }
                self.seen = Some(count);
            }
            None => {
                if level.abs() >= ceiling {
                    self.lit = true;
                }
            }
        }
        if self.lit {
            self.max = self.max.max(level.abs());
        }
    }
}

/// **The dynamic range of a resolution**, in decibels below full scale -- where
/// a meter's floor belongs when it is drawn for a particular format.
///
/// Each bit is `20·log10(2)` = 6.02 dB, so 16 bits reach 96 dB down, 24 reach
/// 144 and a 32-bit integer 193. **A 32-bit float carries a 24-bit
/// significand**, so its floor is 24's: pass 24 for it, not 32.
///
/// This is a *floor*, not the only one worth drawing: [`METER_FLOOR_DB`] is the
/// 60 dB strip a mixing meter is read on, and a meter given the whole dynamic
/// range of the format spends most of its height on a span nobody mixes in. The
/// two are both right, for different questions.
pub fn floor_db_for_bits(bits: u32) -> f32 {
    -(20.0 * core::f32::consts::LN_2 / core::f32::consts::LN_10) * bits.clamp(1, 64) as f32
}

#[cfg(test)]
mod clip_tests {
    use super::*;

    /// A run is one over, however long it runs -- or a square wave would report
    /// one per sample.
    #[test]
    fn a_run_counts_once_and_a_break_starts_again() {
        let mut c = ClipCount::new();
        assert_eq!(c.feed_block(&[1.0; 10], CLIP_CEILING, CLIP_RUN), 1);
        assert_eq!(c.overs(), 1);
        c.feed_block(&[0.0, 0.5], CLIP_CEILING, CLIP_RUN);
        assert_eq!(c.feed_block(&[1.0; 3], CLIP_CEILING, CLIP_RUN), 1);
        assert_eq!(c.overs(), 2, "a second run is a second over");
    }

    /// Shorter than the run is not an over: one sample at full scale is a
    /// sample at full scale.
    #[test]
    fn a_short_run_is_not_an_over() {
        let mut c = ClipCount::new();
        c.feed_block(&[1.0, 0.0, 1.0, 1.0, 0.0], CLIP_CEILING, CLIP_RUN);
        assert_eq!(c.overs(), 0);
        // And the pessimistic rule reads the same samples as two overs: the
        // two runs, not the three samples.
        let mut one = ClipCount::new();
        one.feed_block(&[1.0, 0.0, 1.0, 1.0, 0.0], CLIP_CEILING, 1);
        assert_eq!(one.overs(), 2);
    }

    /// A run that crosses a block boundary is still one run: the state is the
    /// counter's, not the block's.
    #[test]
    fn a_run_crosses_a_block() {
        let mut c = ClipCount::new();
        assert_eq!(c.feed_block(&[1.0, 1.0], CLIP_CEILING, CLIP_RUN), 0);
        assert_eq!(c.feed_block(&[1.0], CLIP_CEILING, CLIP_RUN), 1);
    }

    /// The sign does not matter: a negative peak flattens the same way.
    #[test]
    fn an_over_is_read_on_the_magnitude() {
        let mut c = ClipCount::new();
        c.feed_block(&[-1.0, -1.2, -1.0], CLIP_CEILING, CLIP_RUN);
        assert_eq!(c.overs(), 1);
    }

    /// The mark stays lit after the over is gone, and holds the loudest level
    /// it saw -- which is the whole point of latching it.
    #[test]
    fn the_mark_stays_and_remembers_how_far_it_went() {
        let mut latch = ClipLatch::new();
        latch.tick(0.5, Some(0.0), CLIP_CEILING);
        assert!(!latch.lit());
        latch.tick(1.4, Some(1.0), CLIP_CEILING);
        assert!(latch.lit());
        latch.tick(0.2, Some(1.0), CLIP_CEILING);
        assert!(latch.lit(), "the over is over; the mark is not");
        assert!((latch.max() - 1.4).abs() < 1e-6);
        latch.clear();
        assert!(!latch.lit() && latch.max() == 0.0);
    }

    /// A count that went backwards is a new pass, and that clears the mark
    /// without anybody reaching for it.
    #[test]
    fn a_count_that_restarts_clears_the_mark() {
        let mut latch = ClipLatch::new();
        // The first count read is a baseline and lights nothing: a window
        // opened on a server that has been running is not reporting its past.
        latch.tick(0.1, Some(2.0), CLIP_CEILING);
        assert!(!latch.lit(), "the first read is a baseline");
        latch.tick(1.0, Some(3.0), CLIP_CEILING);
        assert!(latch.lit());
        latch.tick(0.1, Some(0.0), CLIP_CEILING);
        assert!(!latch.lit(), "the counter started again");
    }

    /// With no counting source the level alone lights it -- exact for
    /// exceeding full scale, blind to how many samples did.
    #[test]
    fn without_a_count_the_level_lights_it() {
        let mut latch = ClipLatch::new();
        latch.tick(0.99, None, CLIP_CEILING);
        assert!(!latch.lit());
        latch.tick(1.0, None, CLIP_CEILING);
        assert!(latch.lit());
    }

    /// The floor a resolution asks for, at the two depths anybody states.
    #[test]
    fn a_resolution_names_its_floor() {
        assert!((floor_db_for_bits(16) + 96.3).abs() < 0.1);
        assert!((floor_db_for_bits(24) + 144.5).abs() < 0.1);
        // And it places a level on that scale: -96 dB is the bottom of a
        // 16-bit strip and a long way up a 24-bit one.
        assert!(meter_fraction_db(-96.0, floor_db_for_bits(16)) < 0.01);
        assert!(meter_fraction_db(-96.0, floor_db_for_bits(24)) > 0.3);
    }
}
