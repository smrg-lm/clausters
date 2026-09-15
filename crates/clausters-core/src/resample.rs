//! **Reading a signal between its samples**: the band-limited reconstruction
//! the true-peak measurement and the zoomed-in waveform both need.
//!
//! A sequence of samples is not a picture of a signal — it is the signal's
//! value at a grid of instants, and the signal itself is what the
//! reconstruction filter puts back between them. Two things here ask for that
//! signal rather than for the samples:
//!
//! - **True peak.** A signal whose samples all read below full scale can still
//!   reconstruct above it: the peak fell between two samples. Every converter
//!   sees that peak, so a meter reporting the largest *sample* under-reads
//!   exactly the thing it exists to catch — by about 3 dB in the classic case
//!   and, ITU-R BS.1770-4 says, "commonly several dBs" on real transients.
//!   [`true_peak`] and [`TruePeakMeter`] answer with the reconstructed peak.
//! - **The drawing.** Past the point where a waveform view has more pixels than
//!   samples, the straight segments between the dots are a line the renderer
//!   invented. The reconstruction is the curve that is actually there, and
//!   drawing it is how an overshoot between two samples becomes visible instead
//!   of being flattened by the tool that drew it.
//!
//! # The filter
//!
//! Oversampling by an integer factor `L` is: put `L-1` zeros between every pair
//! of input samples and low-pass the result at the original Nyquist. A
//! **polyphase** decomposition is the same filter with the arithmetic nobody
//! wastes — one branch per output sub-sample, each a short FIR over the *input*
//! samples, so no multiplication by a zero is ever performed. `L` phases of
//! [`TAPS`] taps each are one linear-phase FIR of `L * TAPS` taps, split up.
//!
//! Two are carried here, and the difference between them is what they are:
//!
//! - [`BS1770`] is **the standard's own table** (ITU-R BS.1770-4, Annex 2:
//!   "order 48, 4-phase, FIR interpolating"), transcribed rather than designed,
//!   because a number this project calls dBTP should be the number the
//!   recommendation defines. Measured: symmetric to the bit, ±0.22 dB of
//!   passband ripple out to 0.8 of Nyquist, images down about 40 dB.
//! - [`FINE`] is **a design**, since the recommendation gives no table past 4×
//!   while saying plainly that "higher sampling rates and over-sampling ratios
//!   are preferred": 8 phases of the same 12 taps, a Kaiser-windowed sinc
//!   (β = 3.6, cutoff 0.99 of the input Nyquist), normalized to an overall gain
//!   of 8. Measured: ±0.1 dB out to 0.8 of Nyquist, images down 51 dB.
//!
//! # Why the factor is the accuracy, and the standard says by how much
//!
//! The filter is not where a true-peak reading loses its last fraction of a
//! decibel; the **grid** is. A peak can fall midway between two *oversampled*
//! instants, and the deepest it can hide there is a property of arithmetic, not
//! of anybody's filter — `20·log10(cos(π · fnorm / L))`, which BS.1770-4's
//! Appendix 1 tabulates:
//!
//! | Oversampling | worst under-read at 0.45 of Nyquist | at Nyquist |
//! |---|---|---|
//! | 4× ([`BS1770`]) | 0.554 dB | 0.688 dB |
//! | 8× ([`FINE`]) | 0.136 dB | 0.169 dB |
//!
//! So [`BS1770`] is what the standard asks for and [`FINE`] is what to reach
//! for when the reading has to be tight, and neither is a better *filter* than
//! the other — they are two grids.
//!
//! **What this deliberately does not do.** BS.1770 attenuates by 12.04 dB
//! before oversampling and restores the gain after. The recommendation says why
//! and when: it is headroom for **integer** arithmetic, and "this step is not
//! necessary if the calculations are performed in floating point". These are
//! `f32`, so it is skipped.
//!
//! # The guard, stated rather than hidden
//!
//! An FIR reading `TAPS` input samples produces its first legitimate output
//! only once it has that many to read. A reconstruction over a *span* of a
//! longer signal therefore needs [`GUARD`] input samples of context on each
//! side; without them the filter reads zeros past the edge and **rings** — an
//! overshoot at the ends that is the tool's and not the signal's.
//!
//! So every function here takes the guard explicitly. A caller that cannot
//! supply context — the first samples of a file, where there is nothing before
//! them — gets the documented edge policy ([`Interpolator::oversample_edge`]:
//! silence before the beginning, which is what silence before a file is) rather
//! than a silent lie about where the context came from.

/// Taps per phase: how many **input** samples each output sub-sample reads.
/// The standard's table is 12 and the design beside it keeps that length.
pub const TAPS: usize = 12;

/// The widest table here, so a caller can size a stack buffer for one input
/// sample's sub-samples without asking at runtime.
pub const MAX_FACTOR: usize = 8;

/// Input samples of context the filter needs on **each side** of a span before
/// its output is the signal rather than the filter's own edge.
///
/// `TAPS / 2`: the phases are a linear-phase FIR split up, so its group delay
/// is half its length, and half of `TAPS` input samples is what sits on either
/// side of the output being computed.
pub const GUARD: usize = TAPS / 2;

/// **The ITU-R BS.1770-4 Annex 2 table**, phase 0 first: the 4×, order-48,
/// 4-phase FIR the recommendation prints for true-peak measurement.
///
/// `PHASES_4X[p][k]` multiplies the input sample `k` back from the current one
/// when producing sub-sample `p`. Phases 2 and 3 are the reverses of 1 and 0,
/// which is what the polyphase decomposition of a **linear-phase** filter looks
/// like from here; the tests assert it, along with the passband and the
/// standard's own worst case, rather than the reader taking a transcribed
/// table on trust.
// The digits are the recommendation's own, and more of them than `f32` holds:
// trimming to the representable value would make this a *transcription of a
// rounding* rather than of the standard, which is the one property the table
// has to have. The compiler rounds each literal exactly as it would round the
// shorter form, so nothing about the arithmetic changes.
#[allow(clippy::excessive_precision)]
pub const PHASES_4X: [[f32; TAPS]; 4] = [
    [
        0.0017089843750,
        0.0109863281250,
        -0.0196533203125,
        0.0332031250000,
        -0.0594482421875,
        0.1373291015625,
        0.9721679687500,
        -0.1022949218750,
        0.0476074218750,
        -0.0266113281250,
        0.0148925781250,
        -0.0083007812500,
    ],
    [
        -0.0291748046875,
        0.0292968750000,
        -0.0517578125000,
        0.0891113281250,
        -0.1665039062500,
        0.4650878906250,
        0.7797851562500,
        -0.2003173828125,
        0.1015625000000,
        -0.0582275390625,
        0.0330810546875,
        -0.0189208984375,
    ],
    [
        -0.0189208984375,
        0.0330810546875,
        -0.0582275390625,
        0.1015625000000,
        -0.2003173828125,
        0.7797851562500,
        0.4650878906250,
        -0.1665039062500,
        0.0891113281250,
        -0.0517578125000,
        0.0292968750000,
        -0.0291748046875,
    ],
    [
        -0.0083007812500,
        0.0148925781250,
        -0.0266113281250,
        0.0476074218750,
        -0.1022949218750,
        0.9721679687500,
        0.1373291015625,
        -0.0594482421875,
        0.0332031250000,
        -0.0196533203125,
        0.0109863281250,
        0.0017089843750,
    ],
];

/// **The 8× table**, designed rather than transcribed — the recommendation
/// gives none past 4× while saying plainly that higher ratios are preferred.
///
/// It is the classic **fractional-delay** form, which is chosen for a property
/// the standard's table does not have: the reconstruction at offset `d` from
/// sample `i` is `sum_k x[i+k] · sinc(k − d) · kaiser(k − d)`, so at `d = 0`
/// every term but `x[i]` is a sinc at an integer — zero — and **phase 0 is the
/// sample itself, exactly**. A curve drawn from this table passes *through* its
/// dots; one drawn from [`PHASES_4X`] misses them by up to 0.09, because that
/// filter's centre falls between two samples and none of its phases is a
/// passthrough. It costs nothing in the measurement (a peak is a maximum over a
/// grid either way) and it is the whole difference in a picture.
///
/// Twelve taps per phase and a Kaiser window of `β = 7`, over `k = −5 ..= 6`.
/// The test beside it re-derives the table from that one formula, so the block
/// of digits in the source is a *result* rather than something nobody can
/// regenerate.
// The generator's output, printed at the precision it computed in: the test
// beside it re-derives these from the formula, so what matters is that the
// digits are the design's and not that they are the shortest spelling of the
// nearest `f32`.
#[allow(clippy::excessive_precision)]
pub const PHASES_8X: [[f32; TAPS]; 8] = [
    [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    [
        -0.000193397887112,
        0.00179619783488,
        -0.00691757581941,
        0.0192732408805,
        -0.0469983410199,
        0.129901723428,
        0.973126070621,
        -0.0965324859891,
        0.03771191086,
        -0.0152029143899,
        0.00514019474351,
        -0.00117073555969,
    ],
    [
        -0.000529114567665,
        0.00403940872548,
        -0.0147182534138,
        0.039963552247,
        -0.0970935648575,
        0.285241585165,
        0.895265241713,
        -0.156223106051,
        0.0624441690419,
        -0.0248510810622,
        0.00811881036977,
        -0.00170934736854,
    ],
    [
        -0.000971577108497,
        0.00636086958632,
        -0.0220461441305,
        0.058488513955,
        -0.142110150496,
        0.454236667861,
        0.774343798994,
        -0.180039920788,
        0.0730852278205,
        -0.0286371854276,
        0.00901027149072,
        -0.00173332219502,
    ],
    [
        -0.00142435505571,
        0.00822618924155,
        -0.02724675791,
        0.070808271732,
        -0.172796761198,
        0.622436196527,
        0.622436196527,
        -0.172796761198,
        0.070808271732,
        -0.02724675791,
        0.00822618924155,
        -0.00142435505571,
    ],
    [
        -0.00173332219502,
        0.00901027149072,
        -0.0286371854276,
        0.0730852278205,
        -0.180039920788,
        0.774343798994,
        0.454236667861,
        -0.142110150496,
        0.058488513955,
        -0.0220461441305,
        0.00636086958632,
        -0.000971577108497,
    ],
    [
        -0.00170934736854,
        0.00811881036977,
        -0.0248510810622,
        0.0624441690419,
        -0.156223106051,
        0.895265241713,
        0.285241585165,
        -0.0970935648575,
        0.039963552247,
        -0.0147182534138,
        0.00403940872548,
        -0.000529114567665,
    ],
    [
        -0.00117073555969,
        0.00514019474351,
        -0.0152029143899,
        0.03771191086,
        -0.0965324859891,
        0.973126070621,
        0.129901723428,
        -0.0469983410199,
        0.0192732408805,
        -0.00691757581941,
        0.00179619783488,
        -0.000193397887112,
    ],
];

/// **A reconstruction filter**, as a factor and its phases.
///
/// The two constants are [`BS1770`] and [`FINE`]; a caller that has its own
/// table (a different rate, a different accuracy target) builds one, since
/// nothing here is privileged but the numbers.
#[derive(Debug, Clone, Copy)]
pub struct Interpolator {
    phases: &'static [[f32; TAPS]],
}

/// The standard's 4× filter: what a reading called **dBTP** is measured with.
pub const BS1770: Interpolator = Interpolator { phases: &PHASES_4X };

/// The 8× filter: four times the grid, and a quarter of its worst under-read
/// (0.136 dB against 0.554 at 0.45 of Nyquist). What a drawing uses, since a
/// curve is read by eye and seven points between two samples is a curve.
pub const FINE: Interpolator = Interpolator { phases: &PHASES_8X };

impl Interpolator {
    /// A filter over any phase table of [`TAPS`]-tap branches.
    pub const fn new(phases: &'static [[f32; TAPS]]) -> Self {
        Self { phases }
    }

    /// How many output sub-samples each input sample becomes.
    pub fn factor(self) -> usize {
        self.phases.len()
    }

    /// One output sub-sample: phase `p` over the [`TAPS`] input samples ending
    /// at `window[TAPS - 1]` (oldest first). A short window or an unknown phase
    /// answers `0.0` rather than reading past either.
    #[inline]
    pub fn phase_at(self, window: &[f32], p: usize) -> f32 {
        if window.len() < TAPS || p >= self.phases.len() {
            return 0.0;
        }
        let taps = &self.phases[p];
        let mut acc = 0.0f32;
        for (k, &c) in taps.iter().enumerate() {
            acc += c * window[TAPS - 1 - k];
        }
        acc
    }

    /// **The reconstructed signal over a span**, at [`factor`](Self::factor)×
    /// the sample rate.
    ///
    /// `input` is the span **plus [`GUARD`] samples of context at each end**,
    /// and the output is every span sample's sub-samples — so `out.len()` must
    /// be `(input.len() - 2 * GUARD) * factor`. Returns `false`, leaving `out`
    /// untouched, when the lengths do not agree or no span is left after the
    /// guard. Allocation-free: the caller owns `out`, so a per-frame or
    /// real-time caller reuses one buffer.
    ///
    /// **The reconstruction is centred on the span sample**, not delayed behind
    /// it: sub-sample `0` of span sample `i` *is* `x[i]`, and the ones after it
    /// are the signal on the way to `x[i + 1]`. That is what a drawing needs —
    /// a curve through the dots rather than beside them — and it is the whole
    /// reason the guard is symmetric, six samples of the past and six of the
    /// future for a twelve-tap filter whose centre is between them.
    pub fn oversample_into(self, input: &[f32], out: &mut [f32]) -> bool {
        let factor = self.factor();
        let Some(span) = input.len().checked_sub(2 * GUARD).filter(|n| *n > 0) else {
            return false;
        };
        if out.len() != span * factor {
            return false;
        }
        for i in 0..span {
            // Span sample `i` sits at `i + GUARD` in the padded input, and the
            // filter reads the `TAPS` samples centred there: the `GUARD - 1`
            // before it, itself, and the `GUARD` after.
            let window = &input[i + 1..=i + TAPS];
            for p in 0..factor {
                out[i * factor + p] = self.phase_at(window, p);
            }
        }
        true
    }

    /// The same for a span with **no context to read**: the beginning or the
    /// end of a signal, where what lies past it is silence.
    ///
    /// The edge policy, stated: samples outside the slice are taken as zeros,
    /// which is what silence before a file is, so the filter's own settling
    /// over the first and last [`GUARD`] samples is part of the answer. Use
    /// [`oversample_into`](Self::oversample_into) with real context wherever
    /// there is any — the two agree exactly in the interior, and the test
    /// beside them says by how much they differ at the edge.
    pub fn oversample_edge(self, input: &[f32], out: &mut [f32]) -> bool {
        let factor = self.factor();
        if out.len() != input.len() * factor {
            return false;
        }
        let mut window = [0.0f32; TAPS];
        for i in 0..input.len() {
            // The same centred window, with whatever falls outside read as the
            // silence around the signal.
            for (k, slot) in window.iter_mut().enumerate() {
                let at = i as isize + 1 + k as isize - GUARD as isize;
                *slot = if at < 0 {
                    0.0
                } else {
                    input.get(at as usize).copied().unwrap_or(0.0)
                };
            }
            for p in 0..factor {
                out[i * factor + p] = self.phase_at(&window, p);
            }
        }
        true
    }
}

/// **A true-peak meter fed block by block** — the streaming face of the
/// measurement, and the implementation the one-shot [`true_peak`] is a loop
/// over.
///
/// It keeps the filter's own context between calls, so a signal cut into blocks
/// reads exactly as the whole of it does: the guard is the meter's state rather
/// than the caller's problem. Allocation-free and branch-light, so the audio
/// thread may feed it.
#[derive(Debug, Clone, Copy)]
pub struct TruePeakMeter {
    filter: Interpolator,
    window: [f32; TAPS],
    peak: f32,
}

impl Default for TruePeakMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl TruePeakMeter {
    /// A meter that has seen nothing, measuring the way the standard says —
    /// [`BS1770`], 4×.
    pub fn new() -> Self {
        Self::with(BS1770)
    }

    /// The same over another filter: [`FINE`] for a tighter reading, or a
    /// caller's own table.
    pub fn with(filter: Interpolator) -> Self {
        Self {
            filter,
            window: [0.0; TAPS],
            peak: 0.0,
        }
    }

    /// The largest reconstructed magnitude seen since the last
    /// [`reset`](Self::reset), in linear amplitude.
    pub fn peak(self) -> f32 {
        self.peak
    }

    /// The same in **dBTP** — decibels relative to full scale, measured over
    /// the reconstructed signal. `f32::NEG_INFINITY` for silence.
    pub fn peak_db(self) -> f32 {
        amplitude_db(self.peak)
    }

    /// Forgets the peak **and** the filter's context: a new pass, measured from
    /// nothing.
    pub fn reset(&mut self) {
        let filter = self.filter;
        *self = Self::with(filter);
    }

    /// Feeds one sample, returning the largest reconstructed magnitude among
    /// the sub-samples it produced.
    #[inline]
    pub fn feed(&mut self, sample: f32) -> f32 {
        self.window.rotate_left(1);
        self.window[TAPS - 1] = sample;
        let mut local = 0.0f32;
        for p in 0..self.filter.factor() {
            local = local.max(self.filter.phase_at(&self.window, p).abs());
        }
        // The samples themselves lie on the reconstructed curve, so a peak that
        // falls exactly on one is not missed by the sub-sample grid.
        local = local.max(sample.abs());
        self.peak = self.peak.max(local);
        local
    }

    /// Feeds a whole block, returning the largest reconstructed magnitude in
    /// it — what a meter draws for *this* block, beside the running peak.
    pub fn feed_block(&mut self, block: &[f32]) -> f32 {
        let mut local = 0.0f32;
        for &s in block {
            local = local.max(self.feed(s));
        }
        local
    }

    /// Feeds one channel of an **interleaved** block, walking by the stride so
    /// no caller deinterleaves first.
    pub fn feed_channel(&mut self, samples: &[f32], channels: usize, channel: usize) -> f32 {
        if channels == 0 || channel >= channels {
            return 0.0;
        }
        let mut local = 0.0f32;
        for &s in samples.iter().skip(channel).step_by(channels) {
            local = local.max(self.feed(s));
        }
        local
    }

    /// Feeds the silence the filter needs to push its last samples out: a
    /// one-shot measurement ends with this, or the peak inside the final
    /// [`GUARD`] samples is still in the window when the reading is taken.
    pub fn flush(&mut self) -> f32 {
        let mut local = 0.0f32;
        for _ in 0..GUARD {
            local = local.max(self.feed(0.0));
        }
        local
    }
}

/// **The true peak of one channel of an interleaved buffer**, in linear
/// amplitude — the one-shot face, a loop over [`TruePeakMeter`].
///
/// It is the reconstructed peak and not the sample peak, so it reads **above**
/// [`crate::measure::channel_stats`]'s peak wherever the signal's own maximum
/// fell between two samples. `0.0` for an empty or out-of-range request.
pub fn true_peak(samples: &[f32], channels: usize, channel: usize) -> f32 {
    true_peak_with(BS1770, samples, channels, channel)
}

/// The same over a chosen filter — [`FINE`] where the reading has to be tight.
pub fn true_peak_with(
    filter: Interpolator,
    samples: &[f32],
    channels: usize,
    channel: usize,
) -> f32 {
    let mut meter = TruePeakMeter::with(filter);
    meter.feed_channel(samples, channels, channel);
    meter.flush();
    meter.peak()
}

/// The true peak in **dBTP**. `f32::NEG_INFINITY` for silence.
pub fn true_peak_db(samples: &[f32], channels: usize, channel: usize) -> f32 {
    amplitude_db(true_peak(samples, channels, channel))
}

/// **Where a true-peak reading stops being safe**, in dBTP.
///
/// -1 dBTP is EBU R128's ceiling and the figure every delivery specification
/// repeats. Not because anything breaks at exactly 0: a converter, a
/// sample-rate conversion and a lossy encoder downstream each move the peak by
/// a fraction of a decibel, and this is where they are given room to.
pub const TRUE_PEAK_CEILING_DBTP: f32 = -1.0;

/// An amplitude in decibels relative to full scale.
fn amplitude_db(amplitude: f32) -> f32 {
    let a = amplitude.abs();
    if a <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * a.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` samples of a sine: amplitude `a`, `cycles` whole cycles over `n`
    /// samples (so it closes on itself), from phase `phase` radians.
    fn sine(n: usize, a: f32, cycles: f32, phase: f32) -> Vec<f32> {
        (0..n)
            .map(|i| a * (core::f32::consts::TAU * cycles * i as f32 / n as f32 + phase).sin())
            .collect()
    }

    fn db(x: f32) -> f32 {
        20.0 * x.log10()
    }

    /// The peak of the reconstruction **inside** a signal, read with context on
    /// both sides -- which is what a measurement of the signal is, as opposed
    /// to a measurement of the filter starting from silence.
    fn interior_peak(filter: Interpolator, x: &[f32]) -> f32 {
        let span = x.len() - 2 * GUARD;
        let mut out = vec![0.0f32; span * filter.factor()];
        assert!(filter.oversample_into(x, &mut out));
        out.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }

    /// **Both tables are linear-phase filters, split up.** Reversing a whole
    /// impulse response maps its phases onto each other, so each table shows a
    /// mirror pairing -- and the first thing a mistranscribed or mistyped
    /// coefficient breaks.
    ///
    /// The **pairing differs between the two**, and that is the delay
    /// convention rather than a discrepancy: the standard's four phases
    /// *straddle* the sample (its centre falls half a sub-sample off the grid,
    /// which is why none of them is a passthrough), so they mirror as
    /// `p <-> L-1-p`; the design's start *on* it, so phase 0 is the sample and
    /// has no mirror among the others, while the rest pair as `p <-> L-p`.
    #[test]
    fn both_tables_are_linear_phase_filters() {
        for k in 0..TAPS {
            for (a, b) in [(0usize, 3usize), (1, 2)] {
                assert_eq!(
                    PHASES_4X[a][k],
                    PHASES_4X[b][TAPS - 1 - k],
                    "4x phase {a} tap {k} against phase {b}"
                );
            }
            // Phase 0 stands alone: it is the sample itself, an impulse on
            // the tap the window's centre falls on, and its mirror would be
            // the impulse half a tap away -- which is the delay convention
            // saying, again, that this table starts on the sample.
            for (p, phase) in PHASES_8X.iter().enumerate().skip(1) {
                let q = 8 - p;
                let (a, b) = (phase[k], PHASES_8X[q][TAPS - 1 - k]);
                assert!(
                    (a - b).abs() < 1e-9,
                    "8x phase {p} tap {k}: {a} against phase {q}'s {b}"
                );
            }
        }
    }

    /// **The standard's table is the standard's**, and this is the check that
    /// says so without a reader trusting a transcription: the gain at DC, and
    /// the two phase sums the recommendation's numbers actually add up to.
    ///
    /// They are **not** 1 apiece -- the four branches sum to 1.0016, 0.9730,
    /// 0.9730 and 1.0016 -- and that is the filter the recommendation prints,
    /// not a typo here. It is why this asserts the measured values rather than
    /// the property a designed interpolator would have.
    #[test]
    fn the_standard_table_is_transcribed_exactly() {
        let sums: Vec<f32> = PHASES_4X.iter().map(|p| p.iter().sum()).collect();
        assert!((sums[0] - 1.001_587).abs() < 1e-5, "{sums:?}");
        assert!((sums[1] - 0.973_022).abs() < 1e-5, "{sums:?}");
        assert_eq!(sums[0], sums[3]);
        assert_eq!(sums[1], sums[2]);
        // The overall gain is the factor, to within the filter's own accuracy:
        // a constant signal reconstructs to itself, give or take 0.11 dB.
        let dc: f32 = sums.iter().sum::<f32>() / 4.0;
        assert!((db(dc)).abs() < 0.15, "DC gain {dc}");
    }

    /// **The designed table is reproducible from its own formula.** It is
    /// re-derived here from what its documentation states -- the
    /// fractional-delay form, `sinc(k - d)` windowed by a Kaiser of `β = 7`
    /// over `k = -5 ..= 6` -- so the block of digits in the source is a
    /// *result* rather than something nobody can regenerate.
    #[test]
    fn the_designed_table_comes_from_its_stated_design() {
        const L: usize = 8;
        let beta = 7.0f64;
        // Modified Bessel I0, by its series.
        let i0 = |x: f64| {
            let (mut sum, mut term, mut k) = (1.0f64, 1.0f64, 1.0f64);
            while term > 1e-18 * sum {
                term *= (x / 2.0).powi(2) / (k * k);
                sum += term;
                k += 1.0;
            }
            sum
        };
        let sinc = |x: f64| {
            if x == 0.0 {
                1.0
            } else {
                (core::f64::consts::PI * x).sin() / (core::f64::consts::PI * x)
            }
        };
        for (p, phase) in PHASES_8X.iter().enumerate().take(L) {
            let d = p as f64 / L as f64;
            for (j, k) in (-(GUARD as isize) + 1..=GUARD as isize).enumerate() {
                let t = k as f64 - d;
                let r = t / GUARD as f64;
                let w = if r.abs() <= 1.0 {
                    i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0(beta)
                } else {
                    0.0
                };
                let want = (sinc(t) * w) as f32;
                // The table is written in the host's order: tap `k` weights the
                // sample `k` back from the newest, which is this row reversed.
                let got = phase[TAPS - 1 - j];
                assert!(
                    (want - got).abs() < 1e-6,
                    "8x phase {p} tap {j}: table {got}, design {want}"
                );
            }
        }
    }

    /// **Phase 0 is the sample itself, exactly** -- the property the design was
    /// chosen for, and the whole difference between a curve that passes through
    /// its dots and one that misses them. The standard's own table is *not* a
    /// passthrough (its centre falls between two samples), which is why the
    /// drawing reaches for this one and the measurement does not care.
    #[test]
    fn the_designed_tables_first_phase_is_the_sample() {
        assert_eq!(PHASES_8X[0][GUARD], 1.0);
        for (k, &c) in PHASES_8X[0].iter().enumerate() {
            if k != GUARD {
                assert_eq!(c, 0.0, "tap {k} of a passthrough is zero");
            }
        }
        // And it shows: reconstructing a signal, the sub-sample at each sample
        // is that sample, to the bit.
        let x: Vec<f32> = (0..256)
            .map(|i| 0.8 * (i as f32 * 0.37).sin() + 0.2 * (i as f32 * 1.9).sin())
            .collect();
        let span = x.len() - 2 * GUARD;
        let mut out = vec![0.0f32; span * FINE.factor()];
        assert!(FINE.oversample_into(&x, &mut out));
        let worst = (0..span)
            .map(|i| (out[i * FINE.factor()] - x[i + GUARD]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "the curve passes through its dots: {worst}");

        // The standard's table, by contrast, misses them -- measured, so the
        // claim above is a comparison and not an assertion about nothing.
        let mut coarse = vec![0.0f32; span * BS1770.factor()];
        assert!(BS1770.oversample_into(&x, &mut coarse));
        let missed = (0..span)
            .map(|i| (coarse[i * BS1770.factor()] - x[i + GUARD]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            missed > 0.01,
            "the standard's phase 0 is not a passthrough: {missed}"
        );
    }

    /// **The passband is flat**: a sine inside the band reconstructs at its own
    /// amplitude, whatever its frequency and wherever its peak falls between
    /// two samples. Read in the interior, with context -- the edges are the
    /// filter settling, which the guard exists to keep out of a measurement.
    ///
    /// The bound each filter is held to is its own measured ripple plus the
    /// grid's worst under-read at that frequency, which BS.1770-4's Appendix 1
    /// tabulates: those two are the whole of the error, and neither is a fudge
    /// factor.
    #[test]
    fn a_sine_reconstructs_at_its_own_amplitude() {
        let n = 4096;
        for (filter, bound, name) in [(BS1770, 0.75f32, "4x"), (FINE, 0.25, "8x")] {
            for cycles in [7.0f32, 61.0, 211.0, 409.0, 601.0, 811.0] {
                for phase in [0.0f32, 0.3, 1.1, 2.4] {
                    let x = sine(n, 0.5, cycles, phase);
                    let read = interior_peak(filter, &x);
                    let err = db(read / 0.5);
                    assert!(
                        err.abs() < bound,
                        "{name}: {cycles} cycles at phase {phase} read {read:.5} \
                         ({err:+.3} dB off half scale)"
                    );
                }
            }
        }
    }

    /// **The standard's worst case, and the whole reason the measurement
    /// exists**: a sine at a quarter of the sample rate, sampled at 45°, reads
    /// `±1.0` at every sample -- full scale, and a meter watching samples has
    /// nothing to report. The signal between them reaches `sqrt(2)`, so its
    /// true peak is **+3.01 dBTP**. BS.1770-4's Appendix 1 names this case: "a
    /// 3 dB under-read for an unfortunately-phased tone at a quarter of the
    /// sampling frequency".
    #[test]
    fn the_peak_between_two_samples_is_the_one_that_clips() {
        let n = 1024;
        let x: Vec<f32> = (0..n)
            .map(|i| if (i / 2) % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let sample_peak = x.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert_eq!(sample_peak, 1.0, "every sample is at full scale");

        for (filter, name, bound) in [(BS1770, "4x", 0.2f32), (FINE, "8x", 0.1)] {
            let dbtp = db(interior_peak(filter, &x));
            assert!(
                (dbtp - 3.01).abs() < bound,
                "{name}: the signal between the samples is sqrt(2), read {dbtp:.3} dBTP"
            );
            assert!(dbtp > 0.0, "{name}: and it is over full scale");
        }
    }

    /// **A finer grid reads a peak *closer*** -- which is not the same as
    /// reading it higher, and the difference is worth pinning down. The
    /// standard's 4× filter has about a fifth of a decibel of passband ripple,
    /// so it can land either side of the true amplitude; the 8× design is flat
    /// to a hundredth. On a sine of known amplitude the finer reading is the
    /// nearer one, every time.
    #[test]
    fn the_finer_grid_reads_closer() {
        for cycles in [97.0f32, 311.0, 733.0] {
            for phase in [0.0f32, 0.7, 1.9] {
                let x = sine(2048, 0.8, cycles, phase);
                let coarse = (db(interior_peak(BS1770, &x) / 0.8)).abs();
                let fine = (db(interior_peak(FINE, &x) / 0.8)).abs();
                assert!(
                    fine <= coarse + 0.01,
                    "{cycles} cycles at {phase}: 4x is {coarse:.4} dB off, 8x {fine:.4}"
                );
            }
        }
    }

    /// The one-shot reads one channel of an interleaved buffer through its
    /// stride, and answers nothing for a request that cannot be met.
    #[test]
    fn a_channel_is_measured_through_its_stride() {
        let loud = sine(512, 0.9, 37.0, 0.0);
        let quiet = sine(512, 0.1, 37.0, 0.0);
        let mut interleaved = Vec::with_capacity(1024);
        for i in 0..512 {
            interleaved.push(quiet[i]);
            interleaved.push(loud[i]);
        }
        assert!(true_peak(&interleaved, 2, 0) < 0.2);
        assert!(true_peak(&interleaved, 2, 1) > 0.85);
        assert_eq!(true_peak(&interleaved, 2, 2), 0.0, "no such channel");
        assert_eq!(true_peak(&[], 1, 0), 0.0);
        assert_eq!(true_peak_db(&[], 1, 0), f32::NEG_INFINITY);
    }

    /// **The true peak is never below the sample peak.** Every sample lies on
    /// the reconstructed curve, so the measurement can only find more than a
    /// sample scan does -- and on a signal with energy near Nyquist it finds
    /// distinctly more.
    #[test]
    fn it_never_reads_below_the_sample_peak() {
        let x = sine(2048, 0.95, 500.0, 0.37);
        let sample_peak = x.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        let tp = true_peak(&x, 1, 0);
        assert!(tp >= sample_peak - 1e-6, "{tp} vs {sample_peak}");
    }

    /// **Streaming and one-shot are one algorithm.** The meter fed block by
    /// block reads exactly what the loop over the whole signal reads, because
    /// the filter's context is the meter's state and not the caller's problem.
    #[test]
    fn a_signal_cut_into_blocks_reads_the_same() {
        let x = sine(2000, 0.8, 97.0, 0.4);
        let whole = true_peak(&x, 1, 0);
        let mut meter = TruePeakMeter::new();
        for block in x.chunks(64) {
            meter.feed_block(block);
        }
        meter.flush();
        assert_eq!(
            meter.peak(),
            whole,
            "the cut is not part of the measurement"
        );
    }

    /// A reset forgets the peak **and** the context: a new pass measures its
    /// own signal, not the tail of the one before it.
    #[test]
    fn a_reset_forgets_the_pass() {
        let mut meter = TruePeakMeter::new();
        meter.feed_block(&[1.0; 32]);
        assert!(meter.peak() > 0.9);
        meter.reset();
        assert_eq!(meter.peak(), 0.0);
        meter.feed_block(&[0.0; 32]);
        assert_eq!(meter.peak(), 0.0, "silence after a reset is silence");
    }

    /// **The guard is why a span is read with context.** The same span of one
    /// signal, read with the samples around it and read as if the signal began
    /// there, agree in the middle and differ at the edges -- and the test says
    /// by how much rather than asserting they match.
    #[test]
    fn a_span_read_without_context_rings_at_its_edges() {
        let x = sine(512, 0.7, 31.0, 0.9);
        let (start, span) = (128usize, 64usize);
        let factor = BS1770.factor();

        let mut with_guard = vec![0.0f32; span * factor];
        assert!(BS1770.oversample_into(&x[start - GUARD..start + span + GUARD], &mut with_guard));

        let mut bare = vec![0.0f32; span * factor];
        assert!(BS1770.oversample_edge(&x[start..start + span], &mut bare));

        // Away from the edges the two are the same signal.
        for i in GUARD * factor..(span - GUARD) * factor {
            assert!(
                (with_guard[i] - bare[i]).abs() < 1e-5,
                "sub-sample {i} disagrees away from the edges"
            );
        }
        // The first sub-samples are the filter settling from silence, and that
        // difference is worth a number: it is the artefact a caller avoids by
        // handing over context.
        let edge = (0..factor)
            .map(|i| (with_guard[i] - bare[i]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            edge > 0.05,
            "the edge without context is the filter's own, not the signal's: {edge}"
        );
    }

    /// A request whose lengths do not agree is refused, with the output left
    /// alone rather than half written.
    #[test]
    fn a_mismatched_request_is_refused() {
        let x = vec![0.5f32; 40];
        let mut out = vec![-1.0f32; 8];
        assert!(!BS1770.oversample_into(&x, &mut out));
        assert!(out.iter().all(|s| *s == -1.0));
        assert!(
            !BS1770.oversample_into(&x[..GUARD * 2], &mut out),
            "no span"
        );
    }
}
