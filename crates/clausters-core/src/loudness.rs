//! **Loudness**: how loud a programme is, measured the way broadcast measures
//! it -- ITU-R BS.1770 for the number, EBU R 128 for the time scales around it,
//! EBU Tech 3342 for its range.
//!
//! A peak says how close a signal came to full scale; it says nothing about how
//! loud it *sounds*, since a snare hit and a sustained pad can share one peak
//! and differ by 20 dB to a listener. BS.1770's answer is deliberately simple --
//! a frequency weighting, a mean square, a sum over channels -- and it is the
//! number every delivery specification, streaming service and editor's loudness
//! meter reports, which is the whole reason to compute it exactly rather than
//! approximately.
//!
//! # The measurement
//!
//! Each channel is **K-weighted** ([`KWeighting`]): a high shelf of about
//! +4 dB above 2 kHz (the head, modelled as a rigid sphere) followed by a
//! high-pass near 38 Hz (the "revised low-frequency B" curve). Its mean square
//! over an interval is `zᵢ`, and the loudness of that interval is
//!
//! ```text
//! L = -0.691 + 10·log10(Σ Gᵢ·zᵢ)      LUFS
//! ```
//!
//! with the channel weights `Gᵢ` of [`channel_weights`] -- 1.0 for the front
//! channels, 1.41 for the surrounds, 0 for the LFE, which BS.1770 excludes. The
//! −0.691 cancels the K-weighting's gain at 997 Hz, so a full-scale 997 Hz sine
//! in one front channel reads −3.01 LUFS, and a stereo one reads its peak level.
//!
//! What is built over that number, all of it in [`LoudnessMeter`]:
//!
//! | Reading | Window | Defined in |
//! |---|---|---|
//! | **Momentary** | the last 400 ms, rectangular, ungated | EBU Tech 3341 |
//! | **Short-term** | the last 3 s, rectangular, ungated | EBU Tech 3341 |
//! | **Integrated** | the whole measurement, in 400 ms blocks every 100 ms, gated at −70 LUFS and then 10 LU below the result | ITU-R BS.1770, Annex 1 |
//! | **Loudness range** | the spread of the short-term loudness, 10 per second, gated at −70 LUFS and 20 LU below their mean: the 95th percentile minus the 10th, in LU | EBU Tech 3342 |
//!
//! plus the **maximum** momentary and short-term loudness since the last reset,
//! which Tech 3341 requires an "EBU Mode" meter to display.
//!
//! # One algorithm, two faces
//!
//! [`LoudnessMeter`] is fed interleaved blocks and never allocates doing it, so
//! the same type measures a live bus and a file. The one-shot [`loudness`] is a
//! loop over it and nothing else, which is how an offline analysis and a live
//! meter cannot disagree.
//!
//! # Every rate, by redesign
//!
//! BS.1770 publishes the two filters only at 48 kHz and asks an implementation
//! at any other rate for coefficients that "provide the same frequency
//! response". So the published coefficients are **inverted** back to the
//! analogue section they are the bilinear transform of -- a centre frequency, a
//! Q and three numerator gains, recovered exactly -- and that section is
//! transformed again at the rate asked for ([`Biquad::redesign`]). At 48 kHz the
//! round trip gives back the standard's own table; elsewhere it is the same
//! response, warped only as any bilinear design is.
//!
//! This is what libebur128 does too, with the recovered constants written in as
//! literals. One difference, stated so it is not rediscovered: libebur128 keeps
//! the high-pass numerator at `[1, -2, 1]` at every rate, so its passband gain
//! drifts with the rate (by about 0.002 dB at 44.1 kHz). Here the numerator is
//! redesigned with the rest, so the gain is the standard's at every rate.
//!
//! # Where the conventions come from
//!
//! What the documents leave to the implementation is settled the way the field
//! settles it, and each call is named:
//!
//! - **The block grid.** A 400 ms gating block is "to the nearest sample", with
//!   a 75 % overlap; blocks end at `round(0.4·rate) + j·round(0.1·rate)`
//!   samples, which is exact at every rate divisible by ten.
//! - **The loudness range samples the short-term loudness 10 times a second**,
//!   the minimum Tech 3342 has required since 2016 (V3). libebur128 still takes
//!   one short-term value a second, which the 2011 text allowed.
//! - **Readings before a window is full** see the silence before the meter was
//!   reset -- a live meter's view, and libebur128's. The integrated loudness and
//!   the range use only complete windows, as BS.1770 and Tech 3342 say.
//! - **Storage without allocation.** Gating needs every block since the reset,
//!   and a meter that grows a list cannot run on an audio thread. So blocks are
//!   kept the way libebur128's histogram mode keeps them: in fixed bins of
//!   loudness, 0.01 LU wide from −70 to +30 LUFS ([`HISTOGRAM_BINS`]). Unlike a
//!   plain histogram each bin holds the **exact sum of its blocks' energies**,
//!   so every mean is exact and the only approximation left is which side of a
//!   gate a bin falls on -- decided by the bin's own mean, and exact for blocks
//!   that share a level.
//! - **Nothing to measure** reads `f64::NEG_INFINITY` (no block passed the
//!   gate) and a range of `0.0` (no spread), as libebur128 reports them.

use core::f64::consts::PI;

/// The rate BS.1770 publishes its filters at.
pub const REFERENCE_RATE: f64 = 48_000.0;

/// ITU-R BS.1770, Annex 1, Table 1: **stage 1** of the K-weighting pre-filter,
/// the high shelf that models the head, at 48 kHz. Transcribed.
pub const SHELF_48K: Biquad = Biquad {
    b0: 1.535_124_859_586_97,
    b1: -2.691_696_189_406_38,
    b2: 1.198_392_810_852_85,
    a1: -1.690_659_293_182_41,
    a2: 0.732_480_774_215_85,
};

/// ITU-R BS.1770, Annex 1, Table 2: **stage 2**, the RLB high-pass, at
/// 48 kHz. Transcribed.
pub const HIGH_PASS_48K: Biquad = Biquad {
    b0: 1.0,
    b1: -2.0,
    b2: 1.0,
    a1: -1.990_047_454_833_98,
    a2: 0.990_072_250_366_21,
};

/// The **absolute gate**, in LUFS: blocks quieter than this are silence to
/// every gated reading (BS.1770 and Tech 3342 alike).
pub const ABSOLUTE_GATE: f64 = -70.0;

/// The integrated loudness's **relative gate**, in LU below the absolute-gated
/// loudness (BS.1770; it was −8 LU before 2011).
pub const RELATIVE_GATE: f64 = -10.0;

/// The loudness range's relative gate, in LU below the absolute-gated mean of
/// the short-term loudness (Tech 3342).
pub const RANGE_GATE: f64 = -20.0;

/// The percentiles whose difference is the loudness range (Tech 3342).
pub const RANGE_PERCENTILES: (f64, f64) = (0.10, 0.95);

/// The width of a histogram bin, in LU.
pub const HISTOGRAM_GRAIN: f64 = 0.01;

/// The bins a meter keeps its blocks in: [`HISTOGRAM_GRAIN`] wide from
/// [`ABSOLUTE_GATE`] up to +30 LUFS. A block louder than that shares the top
/// bin, whose energy sum stays exact.
pub const HISTOGRAM_BINS: usize = 10_000;

/// A momentary window, in seconds.
pub const MOMENTARY_SECONDS: f64 = 0.4;
/// A short-term window, in seconds.
pub const SHORT_TERM_SECONDS: f64 = 3.0;
/// The step between gating blocks and between the short-term values the range
/// is taken over, in seconds.
pub const HOP_SECONDS: f64 = 0.1;

/// One second-order IIR section, **normalized** (`a0 = 1`): the form BS.1770
/// writes its filters in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

/// The analogue second-order section a digital [`Biquad`] is the bilinear
/// transform of, in frequency normalized to its centre: `(n2·S² + n1·S + n0) /
/// (S² + S/q + 1)` with `S = s/ω0`.
#[derive(Clone, Copy, Debug)]
struct Analogue {
    /// The centre frequency, in Hz.
    f0: f64,
    q: f64,
    n0: f64,
    n1: f64,
    n2: f64,
}

impl Biquad {
    /// The analogue section this one came from at `rate`, recovered exactly.
    ///
    /// With `K = tan(π·f0/rate)` the bilinear transform gives
    /// `a0 = 1 + K/q + K²`, `a1 = 2(K²−1)/a0`, `a2 = (1 − K/q + K²)/a0`, and
    /// the numerator `b·a0 = (n2 + n1·K + n0·K², 2(n0·K² − n2),
    /// n2 − n1·K + n0·K²)`. Sums and differences of those undo it:
    /// `K² = (1+a1+a2)/(1−a1+a2)`, `a0 = 4/(1−a1+a2)`, and the three gains out
    /// of `b0+b1+b2`, `b0−b1+b2` and `b0−b2`.
    fn analogue(self, rate: f64) -> Analogue {
        let a0 = 4.0 / (1.0 - self.a1 + self.a2);
        let k = ((1.0 + self.a1 + self.a2) / (1.0 - self.a1 + self.a2)).sqrt();
        let k_over_q = (1.0 - self.a2) * a0 / 2.0;
        let (b0, b1, b2) = (self.b0 * a0, self.b1 * a0, self.b2 * a0);
        Analogue {
            f0: k.atan() * rate / PI,
            q: k / k_over_q,
            n0: (b0 + b1 + b2) / (4.0 * k * k),
            n1: (b0 - b2) / (2.0 * k),
            n2: (b0 - b1 + b2) / 4.0,
        }
    }

    /// This filter, designed at `from_rate`, **redesigned** for `to_rate`: the
    /// same analogue section, bilinear-transformed at the new rate with its
    /// centre frequency prewarped. At `to_rate == from_rate` it is `self`, to
    /// rounding.
    pub fn redesign(self, from_rate: f64, to_rate: f64) -> Biquad {
        let s = self.analogue(from_rate);
        let k = (PI * s.f0 / to_rate).tan();
        let kk = k * k;
        let a0 = 1.0 + k / s.q + kk;
        Biquad {
            b0: (s.n2 + s.n1 * k + s.n0 * kk) / a0,
            b1: 2.0 * (s.n0 * kk - s.n2) / a0,
            b2: (s.n2 - s.n1 * k + s.n0 * kk) / a0,
            a1: 2.0 * (kk - 1.0) / a0,
            a2: (1.0 - k / s.q + kk) / a0,
        }
    }

    /// The centre frequency of the analogue section, in Hz -- about 1682 Hz for
    /// the shelf and 38 Hz for the high-pass.
    pub fn centre_frequency(self, rate: f64) -> f64 {
        self.analogue(rate).f0
    }

    /// The magnitude response at `freq` Hz, as a linear gain.
    pub fn magnitude(self, freq: f64, rate: f64) -> f64 {
        let w = 2.0 * PI * freq / rate;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num = (self.b0 + self.b1 * c1 + self.b2 * c2).hypot(-self.b1 * s1 - self.b2 * s2);
        let den = (1.0 + self.a1 * c1 + self.a2 * c2).hypot(-self.a1 * s1 - self.a2 * s2);
        num / den
    }

    /// One step of the transposed direct form II: `state` is the section's two
    /// delays, and `y` goes out.
    #[inline]
    fn step(self, state: &mut [f64; 2], x: f64) -> f64 {
        let y = self.b0 * x + state[0];
        state[0] = self.b1 * x - self.a1 * y + state[1];
        state[1] = self.b2 * x - self.a2 * y;
        y
    }
}

/// **The K-weighting filter** at one rate: BS.1770's two stages, redesigned
/// from their 48 kHz coefficients.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KWeighting {
    pub shelf: Biquad,
    pub high_pass: Biquad,
}

impl KWeighting {
    /// The filter for `rate` Hz.
    pub fn at(rate: f64) -> Self {
        Self {
            shelf: SHELF_48K.redesign(REFERENCE_RATE, rate),
            high_pass: HIGH_PASS_48K.redesign(REFERENCE_RATE, rate),
        }
    }

    /// The whole weighting's gain at `freq` Hz, in dB.
    pub fn response_db(self, freq: f64, rate: f64) -> f64 {
        20.0 * (self.shelf.magnitude(freq, rate) * self.high_pass.magnitude(freq, rate)).log10()
    }
}

/// The weight of each channel of a layout that is known only by its count, as
/// BS.1770 Table 3 weights L/R/C at 1.0, Ls/Rs at 1.41 and leaves the LFE out.
///
/// The count is read the way libebur128 reads it: 1 is mono, 2 is L R, 3 is
/// L R C, 4 is L R Ls Rs, 5 is L R C Ls Rs, and 6 or more is the SMPTE order
/// L R C LFE Ls Rs. Channels past the sixth are measured at 1.0 rather than
/// dropped -- a layout this table cannot name is the caller's to state, with
/// [`LoudnessMeter::with_weights`] (BS.1770-5 Annex 3 weights a channel by its
/// position: 1.41 between 60° and 120° of azimuth, 1.0 elsewhere).
pub fn channel_weight(channels: usize, channel: usize) -> f64 {
    const SURROUND: f64 = 1.41;
    match (channels, channel) {
        (4, 2 | 3) => SURROUND,
        (5, 3 | 4) => SURROUND,
        (6.., 3) => 0.0,
        (6.., 4 | 5) => SURROUND,
        _ => 1.0,
    }
}

/// [`channel_weight`] for every channel of the count, in order.
pub fn channel_weights(channels: usize) -> impl Iterator<Item = f64> {
    (0..channels).map(move |c| channel_weight(channels, c))
}

/// A mean square in loudness: `-0.691 + 10·log10(energy)`, in LUFS.
/// `f64::NEG_INFINITY` for silence.
pub fn energy_to_lufs(energy: f64) -> f64 {
    if energy > 0.0 {
        -0.691 + 10.0 * energy.log10()
    } else {
        f64::NEG_INFINITY
    }
}

/// The inverse of [`energy_to_lufs`].
pub fn lufs_to_energy(lufs: f64) -> f64 {
    10f64.powf((lufs + 0.691) / 10.0)
}

/// **The K-weighting and the channel weights as one stage**: frames in, the
/// weighted energy of each frame out.
///
/// It is a type rather than a pair of fields because both faces of this module
/// run it -- [`LoudnessMeter`] over a live bus and [`Profile`] over a whole take
/// -- and a second copy of "filter, square, weight, sum" is how a curve drawn
/// from a file and a meter watching the same audio would come to disagree.
#[derive(Clone)]
pub struct Weighting {
    filter: KWeighting,
    weights: Box<[f64]>,
    /// Per channel: the shelf's two delays, then the high-pass's.
    state: Box<[[f64; 4]]>,
}

impl Weighting {
    /// One weight per channel (`0.0` leaves a channel out), at `rate` Hz.
    pub fn new(weights: &[f64], rate: f64) -> Self {
        Self {
            filter: KWeighting::at(rate),
            weights: weights.into(),
            state: vec![[0.0; 4]; weights.len()].into_boxed_slice(),
        }
    }

    /// How many channels it weights.
    pub fn channels(&self) -> usize {
        self.weights.len()
    }

    /// The weights, in channel order.
    pub fn weights(&self) -> &[f64] {
        &self.weights
    }

    /// Forgets the filters' memory: a new pass, measured from silence.
    pub fn reset(&mut self) {
        self.state.fill([0.0; 4]);
    }

    /// One frame's **weighted energy**: every channel K-weighted, squared and
    /// summed with its weight. A frame shorter than the channel count is read
    /// as far as it goes.
    #[inline]
    pub fn frame(&mut self, frame: &[f32]) -> f64 {
        let mut energy = 0.0f64;
        for (c, &x) in frame.iter().enumerate().take(self.weights.len()) {
            let st = &mut self.state[c];
            let (mut shelf, mut high) = ([st[0], st[1]], [st[2], st[3]]);
            let y = self
                .filter
                .high_pass
                .step(&mut high, self.filter.shelf.step(&mut shelf, x as f64));
            // A decaying filter reaches denormals in a silent tail, where
            // arithmetic slows by orders of magnitude off an FTZ thread
            // (a Python analysis); libebur128 zeroes them the same way.
            *st = [shelf[0], shelf[1], high[0], high[1]]
                .map(|v| if v.abs() < f64::MIN_POSITIVE { 0.0 } else { v });
            energy += self.weights[c] * y * y;
        }
        energy
    }
}

/// Blocks kept as a histogram of their loudness, each bin carrying how many
/// blocks it holds and the exact sum of their energies.
#[derive(Clone)]
struct Histogram {
    count: Box<[u64]>,
    energy: Box<[f64]>,
}

impl Histogram {
    fn new() -> Self {
        Self {
            count: vec![0; HISTOGRAM_BINS].into_boxed_slice(),
            energy: vec![0.0; HISTOGRAM_BINS].into_boxed_slice(),
        }
    }

    fn clear(&mut self) {
        self.count.fill(0);
        self.energy.fill(0.0);
    }

    /// Adds one block of `energy`, unless it falls under the absolute gate.
    fn add(&mut self, energy: f64) {
        let lufs = energy_to_lufs(energy);
        if lufs.is_nan() || lufs < ABSOLUTE_GATE {
            return;
        }
        let bin = (((lufs - ABSOLUTE_GATE) / HISTOGRAM_GRAIN) as usize).min(HISTOGRAM_BINS - 1);
        self.count[bin] += 1;
        self.energy[bin] += energy;
    }

    /// The occupied bins, quietest first, as `(count, energy sum, loudness of
    /// the bin's mean)`.
    fn bins(&self) -> impl Iterator<Item = (u64, f64, f64)> + '_ {
        self.count
            .iter()
            .zip(self.energy.iter())
            .filter(|(c, _)| **c > 0)
            .map(|(&c, &e)| (c, e, energy_to_lufs(e / c as f64)))
    }

    /// The loudness of the mean energy of every bin at or over `gate` (by the
    /// bin's own mean) -- or of every bin, for `None`.
    fn mean_over(&self, gate: Option<f64>) -> Option<f64> {
        let (mut n, mut e) = (0u64, 0.0f64);
        for (c, sum, lufs) in self.bins() {
            if gate.is_none_or(|g| lufs >= g) {
                n += c;
                e += sum;
            }
        }
        (n > 0).then(|| energy_to_lufs(e / n as f64))
    }

    /// BS.1770's gated loudness: the absolute gate is already applied, the
    /// relative one `relative` LU under what it leaves.
    fn gated(&self, relative: f64) -> f64 {
        let Some(ungated) = self.mean_over(None) else {
            return f64::NEG_INFINITY;
        };
        self.mean_over(Some(ungated + relative))
            .unwrap_or(f64::NEG_INFINITY)
    }

    /// Tech 3342's spread: the gated values' high percentile minus their low
    /// one, with the index rule of its MATLAB reference, `round((n−1)·p)`.
    fn range(&self) -> f64 {
        let Some(mean) = self.mean_over(None) else {
            return 0.0;
        };
        let gate = mean + RANGE_GATE;
        let n: u64 = self
            .bins()
            .filter(|(_, _, l)| *l >= gate)
            .map(|(c, _, _)| c)
            .sum();
        if n == 0 {
            return 0.0;
        }
        let rank = |p: f64| ((n - 1) as f64 * p + 0.5).floor() as u64;
        let (low, high) = (rank(RANGE_PERCENTILES.0), rank(RANGE_PERCENTILES.1));
        let (mut seen, mut lo, mut hi) = (0u64, None, None);
        for (c, _, lufs) in self.bins().filter(|(_, _, l)| *l >= gate) {
            seen += c;
            if lo.is_none() && seen > low {
                lo = Some(lufs);
            }
            if seen > high {
                hi = Some(lufs);
                break;
            }
        }
        match (lo, hi) {
            (Some(lo), Some(hi)) => hi - lo,
            _ => 0.0,
        }
    }
}

/// **A loudness meter**: momentary, short-term, integrated and range over
/// interleaved blocks, as ITU-R BS.1770 and EBU R 128 define them.
///
/// Everything it needs is allocated by the constructor -- the filter state, 3 s
/// of per-frame energy, and two histograms -- so [`feed`](Self::feed) and every
/// reading are allocation-free and the meter can run on an audio thread. At
/// 48 kHz that is about 0.9 MB.
#[derive(Clone)]
pub struct LoudnessMeter {
    rate: f64,
    /// The K-weighting and the channel weights -- the stage [`Profile`] runs too.
    weighting: Weighting,
    /// The channel-weighted, K-weighted energy of each of the last
    /// `short_len` frames, oldest at `head`.
    ring: Box<[f32]>,
    head: usize,
    momentary_len: usize,
    short_len: usize,
    hop: usize,
    frames: u64,
    /// Running sums over the two windows, and the same sums accumulated from
    /// the start of the current window, which replace them once per window so
    /// subtraction never accumulates a rounding error.
    momentary_sum: f64,
    momentary_fresh: f64,
    short_sum: f64,
    short_fresh: f64,
    momentary_max: f64,
    short_max: f64,
    blocks: Histogram,
    short_terms: Histogram,
}

impl LoudnessMeter {
    /// A meter for `channels` interleaved channels at `rate` Hz, with the
    /// weights [`channel_weights`] reads off the count.
    pub fn new(channels: usize, rate: f64) -> Self {
        let weights: Vec<f64> = channel_weights(channels).collect();
        Self::with_weights(&weights, rate)
    }

    /// A meter with one weight per channel, stated: `0.0` leaves a channel out
    /// (an LFE), `1.41` is a surround. The channel count is the weights'.
    ///
    /// # Panics
    /// On no channels, or a rate under 10 Hz (a hop of no samples).
    pub fn with_weights(weights: &[f64], rate: f64) -> Self {
        assert!(!weights.is_empty(), "a loudness meter needs a channel");
        let samples = |seconds: f64| (seconds * rate).round() as usize;
        let (momentary_len, short_len, hop) = (
            samples(MOMENTARY_SECONDS),
            samples(SHORT_TERM_SECONDS),
            samples(HOP_SECONDS),
        );
        assert!(hop > 0, "a loudness meter needs a rate of at least 10 Hz");
        Self {
            rate,
            weighting: Weighting::new(weights, rate),
            ring: vec![0.0; short_len].into_boxed_slice(),
            head: 0,
            momentary_len,
            short_len,
            hop,
            frames: 0,
            momentary_sum: 0.0,
            momentary_fresh: 0.0,
            short_sum: 0.0,
            short_fresh: 0.0,
            momentary_max: 0.0,
            short_max: 0.0,
            blocks: Histogram::new(),
            short_terms: Histogram::new(),
        }
    }

    /// The channel count.
    pub fn channels(&self) -> usize {
        self.weighting.channels()
    }

    /// The rate, in Hz.
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// The frames fed since the last reset.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Forgets everything: the filters' memory, the windows, the maxima and
    /// every block the integrated loudness and the range are taken over. Tech
    /// 3341 resets all of them together.
    pub fn reset(&mut self) {
        self.weighting.reset();
        self.ring.fill(0.0);
        self.head = 0;
        self.frames = 0;
        self.momentary_sum = 0.0;
        self.momentary_fresh = 0.0;
        self.short_sum = 0.0;
        self.short_fresh = 0.0;
        self.momentary_max = 0.0;
        self.short_max = 0.0;
        self.blocks.clear();
        self.short_terms.clear();
    }

    /// Feeds an **interleaved** block. A trailing partial frame is ignored.
    pub fn feed(&mut self, samples: &[f32]) {
        let channels = self.weighting.channels();
        for frame in samples.chunks_exact(channels) {
            let energy = self.weighting.frame(frame);
            self.push(energy as f32);
        }
    }

    /// One frame's weighted energy into both windows, and a block or a
    /// short-term value out wherever one ends.
    #[inline]
    fn push(&mut self, e: f32) {
        let e64 = e as f64;
        let oldest = self.ring[self.head] as f64;
        let m_back = (self.head + self.short_len - self.momentary_len) % self.short_len;
        let momentary_oldest = self.ring[m_back] as f64;
        self.ring[self.head] = e;
        self.head = (self.head + 1) % self.short_len;
        self.frames += 1;

        self.short_sum += e64 - oldest;
        self.short_fresh += e64;
        if self.frames.is_multiple_of(self.short_len as u64) {
            self.short_sum = self.short_fresh;
            self.short_fresh = 0.0;
        }
        self.momentary_sum += e64 - momentary_oldest;
        self.momentary_fresh += e64;
        if self.frames.is_multiple_of(self.momentary_len as u64) {
            self.momentary_sum = self.momentary_fresh;
            self.momentary_fresh = 0.0;
        }

        let momentary = self.momentary_energy();
        let short = self.short_term_energy();
        self.momentary_max = self.momentary_max.max(momentary);
        self.short_max = self.short_max.max(short);

        let hop = self.hop as u64;
        let (m, s) = (self.momentary_len as u64, self.short_len as u64);
        if self.frames >= m && (self.frames - m).is_multiple_of(hop) {
            self.blocks.add(momentary);
        }
        if self.frames >= s && (self.frames - s).is_multiple_of(hop) {
            self.short_terms.add(short);
        }
    }

    fn momentary_energy(&self) -> f64 {
        self.momentary_sum.max(0.0) / self.momentary_len as f64
    }

    fn short_term_energy(&self) -> f64 {
        self.short_sum.max(0.0) / self.short_len as f64
    }

    /// The **momentary** loudness: the last 400 ms, in LUFS.
    pub fn momentary(&self) -> f64 {
        energy_to_lufs(self.momentary_energy())
    }

    /// The **short-term** loudness: the last 3 s, in LUFS.
    pub fn short_term(&self) -> f64 {
        energy_to_lufs(self.short_term_energy())
    }

    /// The loudest momentary reading since the reset, in LUFS -- taken at every
    /// sample, so a 400 ms event is caught whole wherever it starts.
    pub fn momentary_max(&self) -> f64 {
        energy_to_lufs(self.momentary_max)
    }

    /// The loudest short-term reading since the reset, in LUFS.
    pub fn short_term_max(&self) -> f64 {
        energy_to_lufs(self.short_max)
    }

    /// The **integrated** loudness since the reset, gated, in LUFS.
    /// `f64::NEG_INFINITY` until a block passes the gates.
    pub fn integrated(&self) -> f64 {
        self.blocks.gated(RELATIVE_GATE)
    }

    /// The **loudness range** since the reset, in LU. `0.0` until there is a
    /// short-term value to spread.
    pub fn range(&self) -> f64 {
        self.short_terms.range()
    }

    /// Every aggregate at once.
    pub fn summary(&self) -> Loudness {
        Loudness {
            integrated: self.integrated(),
            range: self.range(),
            momentary_max: self.momentary_max(),
            short_term_max: self.short_term_max(),
        }
    }
}

/// What a whole measurement reports: the programme loudness, its range, and
/// the loudest the two windows read along the way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    /// Gated integrated loudness, in LUFS.
    pub integrated: f64,
    /// Loudness range, in LU.
    pub range: f64,
    /// The maximum momentary loudness, in LUFS.
    pub momentary_max: f64,
    /// The maximum short-term loudness, in LUFS.
    pub short_term_max: f64,
}

/// **The loudness of an interleaved buffer** -- the one-shot face, a loop over
/// [`LoudnessMeter`] with the weights [`channel_weights`] reads off the count.
/// `None` for no channels or a rate under 10 Hz.
pub fn loudness(samples: &[f32], channels: usize, rate: f64) -> Option<Loudness> {
    if channels == 0 {
        return None;
    }
    let weights: Vec<f64> = channel_weights(channels).collect();
    loudness_with_weights(samples, &weights, rate)
}

/// The same with one weight per channel, stated. `None` for no weights or a
/// rate under 10 Hz.
pub fn loudness_with_weights(samples: &[f32], weights: &[f64], rate: f64) -> Option<Loudness> {
    if weights.is_empty() || (rate * HOP_SECONDS).round() < 1.0 || rate.is_nan() {
        return None;
    }
    let mut meter = LoudnessMeter::with_weights(weights, rate);
    meter.feed(samples);
    Some(meter.summary())
}

/// The step a [`Profile`] measures on, in seconds: every block of this length
/// carries the weighted energy of its frames, and every reading a profile
/// answers is a whole number of them.
///
/// 10 ms is a fortieth of a momentary window and Tech 3342's floor for the
/// range (a short-term value every 100 ms) with room to spare, so it is finer
/// than any reading taken off it. It is also what a drawing needs and no more:
/// a curve of a 400 ms sliding window cannot move appreciably inside 10 ms, and
/// a take an hour long costs 360 000 blocks -- under six megabytes with its
/// running sums, against the gigabyte its samples take.
pub const PROFILE_HOP_SECONDS: f64 = 0.01;

/// **The loudness of a take over its own time axis**, measured once and read
/// many times: the curve a view draws, and the numbers a span answers with.
///
/// A meter is fed forward and reports where it has got to; a *picture* of a
/// take asks the opposite question -- what was the loudness **there** -- at a
/// thousand places per frame, and again at every zoom. So the pass over the
/// samples happens once, into [`PROFILE_HOP_SECONDS`] blocks of weighted
/// energy with their running sums, and every reading afterwards is two
/// subtractions: a momentary or short-term value anywhere
/// ([`Profile::window_at`]), the extremes over a pixel column
/// ([`Profile::column`]), and the gated aggregates over any span
/// ([`Profile::measure`]).
///
/// It is the same arithmetic [`LoudnessMeter`] runs -- the one [`Weighting`]
/// stage, the same windows, the same gates -- so a curve drawn from a file and
/// a meter watching the same audio read the same numbers, and the tests here
/// assert exactly that rather than trusting it.
///
/// **An edit re-measures its own span and no more** ([`Profile::update`]): a
/// block depends on its samples and on the filters' memory of the few
/// milliseconds before them, so the work after a stroke is the stroke's
/// length, not the take's. The readings over it follow, because a reading is a
/// sum of blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    rate: f64,
    /// Frames per block: `PROFILE_HOP_SECONDS` at `rate`, at least one.
    hop: usize,
    frames: u64,
    weights: Box<[f64]>,
    /// The weighted energy **summed** over each block's frames. The last block
    /// may be short, which is why the sums are energies and not means.
    blocks: Vec<f64>,
    /// `prefix[i]` is the energy of the first `i` blocks, so any run of them
    /// costs one subtraction.
    prefix: Vec<f64>,
}

/// How long the filters are re-run before a re-measured span so that their
/// memory of the samples before it is the same memory they had, in seconds.
///
/// The K-weighting is two second-order sections at 1.68 kHz and 38 Hz, and the
/// slower of them settles in a handful of milliseconds; 100 ms is two orders of
/// magnitude past that, which is why an update and a fresh analysis agree to
/// more decimals than a loudness reading has.
const WARMUP_SECONDS: f64 = 0.1;

impl Profile {
    /// **Measures a take**: one source per channel, read where it lies, with
    /// the weights [`channel_weights`] reads off the count. `None` for no
    /// channels, a weight list that is not one per channel, or a rate too low
    /// to hold a block (under 50 Hz).
    pub fn analyze(sources: &[&dyn crate::peaks::Source], rate: f64) -> Option<Profile> {
        let weights: Vec<f64> = channel_weights(sources.len()).collect();
        Profile::analyze_with_weights(sources, &weights, rate)
    }

    /// The same with one weight per channel, stated.
    pub fn analyze_with_weights(
        sources: &[&dyn crate::peaks::Source],
        weights: &[f64],
        rate: f64,
    ) -> Option<Profile> {
        let hop = (PROFILE_HOP_SECONDS * rate).round() as usize;
        if sources.is_empty() || weights.len() != sources.len() || hop == 0 {
            return None;
        }
        let frames = sources.iter().map(|s| s.len()).min().unwrap_or(0) as u64;
        let mut profile = Profile {
            rate,
            hop,
            frames,
            weights: weights.into(),
            blocks: vec![0.0; frames.div_ceil(hop as u64) as usize],
            prefix: Vec::new(),
        };
        profile.measure_blocks(sources, 0, profile.blocks.len());
        profile.rebuild_prefix(0);
        Some(profile)
    }

    /// The same over an **interleaved** buffer, the shape every other function
    /// here takes.
    pub fn from_interleaved(samples: &[f32], channels: usize, rate: f64) -> Option<Profile> {
        if channels == 0 {
            return None;
        }
        let sources: Vec<crate::peaks::Interleaved<'_>> = (0..channels)
            .map(|c| crate::peaks::Interleaved::new(samples, channels, c))
            .collect();
        let refs: Vec<&dyn crate::peaks::Source> = sources
            .iter()
            .map(|s| s as &dyn crate::peaks::Source)
            .collect();
        Profile::analyze(&refs, rate)
    }

    /// The rate the profile was measured at, in Hz.
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// How many frames it covers.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// The channel count.
    pub fn channels(&self) -> usize {
        self.weights.len()
    }

    /// Whether `sources` are the samples this profile measures: as many
    /// channels, and as long. A picture built over other samples would be a
    /// curve of the wrong audio.
    pub fn describes(&self, channels: usize, frames: u64) -> bool {
        self.weights.len() == channels && self.frames == frames
    }

    /// **Re-measures the span an edit touched**, and the filters' settling
    /// after it: the blocks from `start` to `WARMUP_SECONDS` past the end of
    /// the edit are read again, with the filters warmed over the same span
    /// before them, and the running sums are rebuilt from there. Returns whether it landed (the run
    /// has to be inside the samples this profile describes).
    ///
    /// Every reading over the re-measured blocks follows from the blocks, which
    /// is the whole of the rule: a measure's affected span is the edit's span
    /// widened by the measure's memory, and here the widening costs nothing
    /// because the readings are sums rather than passes.
    pub fn update(&mut self, sources: &[&dyn crate::peaks::Source], start: u64, len: u64) -> bool {
        if sources.len() != self.weights.len() || len == 0 {
            return false;
        }
        let end = start.saturating_add(len);
        if end > self.frames {
            return false;
        }
        // **The settling after the edit is part of the edit.** The filters
        // carry a memory of the samples that changed, so the blocks just past
        // the run read differently even though their own samples did not --
        // which is the same widening the warm-up applies on the way in, and it
        // is why a re-measured profile is the profile and not a near one.
        let settle = (WARMUP_SECONDS * self.rate).round() as u64;
        let first = (start / self.hop as u64) as usize;
        let last =
            (end.saturating_add(settle).div_ceil(self.hop as u64) as usize).min(self.blocks.len());
        self.measure_blocks(sources, first, last);
        self.rebuild_prefix(first);
        true
    }

    /// The loudness of the window of `seconds` **ending** at frame `frame`, in
    /// LUFS -- which is what a meter would have read there, and the value a
    /// curve is drawn from.
    ///
    /// The window is where the reading is *taken*, so it looks backwards: the
    /// value at a point is the loudness of the audio up to it, as on a meter.
    /// A window reaching past the start of the take reads the silence before it,
    /// as a meter reset there would.
    pub fn window_at(&self, seconds: f64, frame: u64) -> f64 {
        let window = self.window_blocks(seconds);
        if window == 0 {
            return f64::NEG_INFINITY;
        }
        let end = self.block_of(frame);
        energy_to_lufs(self.window_energy(end, window))
    }

    /// The **momentary** loudness at `frame`: the 400 ms up to it.
    pub fn momentary_at(&self, frame: u64) -> f64 {
        self.window_at(MOMENTARY_SECONDS, frame)
    }

    /// The **short-term** loudness at `frame`: the 3 s up to it.
    pub fn short_term_at(&self, frame: u64) -> f64 {
        self.window_at(SHORT_TERM_SECONDS, frame)
    }

    /// **The quietest and loudest readings over `[from, to)`**, in LUFS -- one
    /// pixel column of a drawn curve, which covers many readings wherever the
    /// view is zoomed out.
    ///
    /// A column is a band rather than a sampled point for the reason a
    /// waveform's is: drawing one reading out of the hundred a column covers
    /// would make the curve's own shape follow the zoom, and hide exactly the
    /// excursion a reader is looking for. `None` where the span holds no block.
    pub fn column(&self, seconds: f64, from: u64, to: u64) -> Option<(f32, f32)> {
        let window = self.window_blocks(seconds);
        if window == 0 || to <= from || self.blocks.is_empty() {
            return None;
        }
        let first = self.block_of(from).max(1);
        let last = self.block_of(to).max(first);
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for end in first..=last {
            let value = energy_to_lufs(self.window_energy(end, window));
            lo = lo.min(value);
            hi = hi.max(value);
        }
        (hi > f64::NEG_INFINITY || lo.is_finite()).then_some(())?;
        Some((lo as f32, hi as f32))
    }

    /// **The aggregates over a span** -- the gated integrated loudness, the
    /// range, and the loudest momentary and short-term readings inside it.
    ///
    /// This is what a read-out names over a selection (or over the whole take,
    /// which is the span from 0). The gating blocks and the short-term values
    /// are laid out from the span's own start, as a meter reset there would lay
    /// them out.
    pub fn measure(&self, start: u64, frames: u64) -> Loudness {
        let momentary = self.window_blocks(MOMENTARY_SECONDS);
        let short = self.window_blocks(SHORT_TERM_SECONDS);
        let step = self.window_blocks(HOP_SECONDS).max(1);
        let first = self.block_of(start);
        let last = self.block_of(start.saturating_add(frames));
        let readings = |window: usize| -> Vec<f64> {
            let mut out = Vec::new();
            let mut end = first + window;
            while end <= last {
                out.push(self.window_energy(end, window));
                end += step;
            }
            out
        };
        let blocks = readings(momentary);
        let shorts = readings(short);
        Loudness {
            integrated: gated(&blocks, RELATIVE_GATE),
            range: spread(&shorts),
            momentary_max: blocks
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, |a, e| a.max(energy_to_lufs(e))),
            short_term_max: shorts
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, |a, e| a.max(energy_to_lufs(e))),
        }
    }

    /// How many blocks a window of `seconds` covers.
    fn window_blocks(&self, seconds: f64) -> usize {
        (seconds / PROFILE_HOP_SECONDS).round() as usize
    }

    /// The block boundary a frame falls on, as a count of whole blocks.
    fn block_of(&self, frame: u64) -> usize {
        (frame.min(self.frames) / self.hop as u64) as usize
    }

    /// The **mean** weighted energy of the `window` blocks ending at block
    /// `end`, over the window's whole duration -- so a window reaching past the
    /// start of the take averages in the silence before it, exactly as a meter
    /// reset there does.
    fn window_energy(&self, end: usize, window: usize) -> f64 {
        let end = end.min(self.blocks.len());
        let first = end.saturating_sub(window);
        let energy = self.prefix[end] - self.prefix[first];
        (energy / (window * self.hop) as f64).max(0.0)
    }

    /// Reads `[first, last)` of the blocks out of the sources, with the filters
    /// warmed over the samples before them.
    fn measure_blocks(&mut self, sources: &[&dyn crate::peaks::Source], first: usize, last: usize) {
        if first >= last {
            return;
        }
        let channels = self.weights.len();
        let mut weighting = Weighting::new(&self.weights, self.rate);
        let warmup = (WARMUP_SECONDS * self.rate).round() as u64;
        let span_start = first as u64 * self.hop as u64;
        let mut at = span_start.saturating_sub(warmup);
        let end = (last as u64 * self.hop as u64).min(self.frames);
        // One scratch window per channel, read in chunks so a take is never
        // held in memory to be measured -- the peak pyramid's own posture.
        const CHUNK: usize = 4096;
        let mut scratch = vec![vec![0.0f32; CHUNK]; channels];
        let mut frame = vec![0.0f32; channels];
        let mut block = (at / self.hop as u64) as usize;
        let mut acc = 0.0f64;
        let mut in_block = (at % self.hop as u64) as usize;
        while at < end {
            let n = CHUNK.min((end - at) as usize);
            for (c, buf) in scratch.iter_mut().enumerate() {
                sources[c].read_into(at as usize, &mut buf[..n]);
            }
            for i in 0..n {
                for (c, buf) in scratch.iter().enumerate() {
                    frame[c] = buf[i];
                }
                let energy = weighting.frame(&frame);
                // Before the span, the frames only warm the filters.
                if at + i as u64 >= span_start {
                    acc += energy;
                }
                in_block += 1;
                if in_block == self.hop {
                    if block >= first && block < last {
                        self.blocks[block] = acc;
                    }
                    block += 1;
                    acc = 0.0;
                    in_block = 0;
                }
            }
            at += n as u64;
        }
        // The take's last block may be short, and it is a block all the same.
        if in_block > 0 && block >= first && block < last {
            self.blocks[block] = acc;
        }
    }

    /// Rebuilds the running sums from block `from` onwards.
    fn rebuild_prefix(&mut self, from: usize) {
        self.prefix.resize(self.blocks.len() + 1, 0.0);
        if from == 0 {
            self.prefix[0] = 0.0;
        }
        for i in from..self.blocks.len() {
            self.prefix[i + 1] = self.prefix[i] + self.blocks[i];
        }
    }
}

/// BS.1770's gated loudness over block energies: the absolute gate, then the
/// relative one `relative` LU under what it leaves. `f64::NEG_INFINITY` when
/// nothing passes.
fn gated(blocks: &[f64], relative: f64) -> f64 {
    let absolute: Vec<f64> = blocks
        .iter()
        .copied()
        .filter(|e| energy_to_lufs(*e) >= ABSOLUTE_GATE)
        .collect();
    if absolute.is_empty() {
        return f64::NEG_INFINITY;
    }
    let mean = absolute.iter().sum::<f64>() / absolute.len() as f64;
    let gate = energy_to_lufs(mean) + relative;
    let kept: Vec<f64> = absolute
        .into_iter()
        .filter(|e| energy_to_lufs(*e) > gate)
        .collect();
    if kept.is_empty() {
        return f64::NEG_INFINITY;
    }
    energy_to_lufs(kept.iter().sum::<f64>() / kept.len() as f64)
}

/// Tech 3342's spread over short-term energies: the absolute gate, the relative
/// one 20 LU under what it leaves, then the 95th percentile minus the 10th,
/// by the index rule of the document's MATLAB reference.
fn spread(shorts: &[f64]) -> f64 {
    let mut values: Vec<f64> = shorts
        .iter()
        .map(|e| energy_to_lufs(*e))
        .filter(|l| *l >= ABSOLUTE_GATE)
        .collect();
    if values.is_empty() {
        return 0.0;
    }
    let mean = values.iter().map(|l| lufs_to_energy(*l)).sum::<f64>() / values.len() as f64;
    let gate = energy_to_lufs(mean) + RANGE_GATE;
    values.retain(|l| *l >= gate);
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite loudness"));
    let rank =
        |p: f64| (((values.len() - 1) as f64 * p + 0.5).floor() as usize).min(values.len() - 1);
    values[rank(RANGE_PERCENTILES.1)] - values[rank(RANGE_PERCENTILES.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48_000.0;

    /// An interleaved programme of segments, each `(seconds, per-channel peak
    /// levels in dBFS)` of a 1 kHz sine -- `None` for a silent channel. The
    /// phase runs on across segments, as one synthesized file's would.
    fn programme(rate: f64, segments: &[(f64, &[Option<f64>])]) -> Vec<f32> {
        let channels = segments[0].1.len();
        let mut out = Vec::new();
        let mut n = 0usize;
        for &(seconds, levels) in segments {
            let frames = (seconds * rate).round() as usize;
            for _ in 0..frames {
                let s = (2.0 * PI * 1000.0 * n as f64 / rate).sin();
                for level in levels.iter().take(channels) {
                    out.push(match level {
                        Some(db) => (10f64.powf(db / 20.0) * s) as f32,
                        None => 0.0,
                    });
                }
                n += 1;
            }
        }
        out
    }

    /// A stereo tone at `db` dBFS for `seconds`, or silence for `None`.
    fn stereo(seconds: f64, db: Option<f64>) -> (f64, [Option<f64>; 2]) {
        (seconds, [db, db])
    }

    fn stereo_programme(rate: f64, segments: &[(f64, [Option<f64>; 2])]) -> Vec<f32> {
        let s: Vec<(f64, &[Option<f64>])> =
            segments.iter().map(|(t, l)| (*t, l.as_slice())).collect();
        programme(rate, &s)
    }

    fn within(got: f64, want: f64, tol: f64, what: &str) {
        assert!(
            (got - want).abs() <= tol,
            "{what}: {got:.4} against {want} ±{tol}"
        );
    }

    // ---- the filter ----

    /// **The redesign at 48 kHz is the standard's table.** The inversion and
    /// the transform are exact inverses, so what comes back is Table 1 and
    /// Table 2 to rounding -- the check that the algebra is right, before
    /// anything is asked of another rate.
    #[test]
    fn the_redesign_at_48k_is_the_published_table() {
        for (name, table) in [("shelf", SHELF_48K), ("high-pass", HIGH_PASS_48K)] {
            let back = table.redesign(REFERENCE_RATE, REFERENCE_RATE);
            for (got, want) in [
                (back.b0, table.b0),
                (back.b1, table.b1),
                (back.b2, table.b2),
                (back.a1, table.a1),
                (back.a2, table.a2),
            ] {
                assert!((got - want).abs() < 1e-12, "{name}: {got} against {want}");
            }
        }
    }

    /// The analogue sections recovered from the tables are the ones every
    /// implementation that redesigns finds: libebur128 writes them in as
    /// `f0 = 1681.974450955533`, `Q = 0.7071752369554196` for the shelf and
    /// `f0 = 38.13547087602444`, `Q = 0.5003270373238773` for the high-pass.
    #[test]
    fn the_recovered_sections_are_the_known_ones() {
        let shelf = SHELF_48K.analogue(FS);
        within(shelf.f0, 1681.974450955533, 1e-6, "shelf f0");
        within(shelf.q, 0.7071752369554196, 1e-9, "shelf Q");
        // Unity at DC, and libebur128's Vh = 10^(G/20) at the top.
        within(shelf.n0, 1.0, 1e-9, "shelf DC gain");
        within(
            20.0 * shelf.n2.log10(),
            3.999843853973347,
            1e-6,
            "shelf high gain",
        );
        let hp = HIGH_PASS_48K.analogue(FS);
        within(hp.f0, 38.13547087602444, 1e-6, "high-pass f0");
        within(hp.q, 0.5003270373238773, 1e-9, "high-pass Q");
        assert!(hp.n0.abs() < 1e-9 && hp.n1.abs() < 1e-9, "a high-pass");
    }

    /// **The response is the same at every rate**, which is what BS.1770 asks
    /// of coefficients it did not publish. Measured against 48 kHz from 20 Hz
    /// to 15 kHz: within 0.02 dB at 44.1 kHz and 96 kHz, and a little more at
    /// 32 kHz, where 15 kHz is close to Nyquist and the bilinear warp shows.
    #[test]
    fn the_response_is_the_same_at_every_rate() {
        let reference = KWeighting::at(FS);
        for (rate, top, tol) in [
            (44_100.0, 15_000.0, 0.02),
            (96_000.0, 15_000.0, 0.02),
            (192_000.0, 15_000.0, 0.02),
            (32_000.0, 10_000.0, 0.05),
        ] {
            let k = KWeighting::at(rate);
            let mut f = 20.0;
            while f <= top {
                let d = k.response_db(f, rate) - reference.response_db(f, FS);
                assert!(d.abs() < tol, "{rate} Hz rate, {f:.0} Hz: {d:+.4} dB");
                f *= 1.1;
            }
        }
    }

    /// **The −0.691 is the weighting's gain at 997 Hz** (BS.1770, Note 1), so
    /// a full-scale 997 Hz sine in one front channel reads −3.01 LUFS -- at
    /// every rate, which is the redesign seen through the whole measurement.
    ///
    /// Measured, the gain is 0.6910 dB at 48 kHz and moves with the bilinear
    /// warp elsewhere: 0.6838 at 32 kHz, 0.6900 at 44.1 kHz, 0.6964 at
    /// 192 kHz. So the reading is −3.01 to within 0.008 LU at every rate, a
    /// tenth of the tolerance Tech 3341 allows a meter.
    #[test]
    fn a_full_scale_997_hz_sine_reads_minus_3_01_at_every_rate() {
        for rate in [32_000.0, 44_100.0, 48_000.0, 88_200.0, 96_000.0, 192_000.0] {
            within(
                KWeighting::at(rate).response_db(997.0, rate),
                0.691,
                0.008,
                &format!("gain at 997 Hz, {rate} Hz"),
            );
            let x: Vec<f32> = (0..(rate * 2.0) as usize)
                .map(|i| (2.0 * PI * 997.0 * i as f64 / rate).sin() as f32)
                .collect();
            let l = loudness(&x, 1, rate).unwrap();
            within(l.integrated, -3.0103, 0.008, &format!("{rate} Hz"));
        }
    }

    #[test]
    fn the_channel_weights_are_the_tables() {
        let w = |n| channel_weights(n).collect::<Vec<_>>();
        assert_eq!(w(1), [1.0]);
        assert_eq!(w(2), [1.0, 1.0]);
        assert_eq!(w(4), [1.0, 1.0, 1.41, 1.41]);
        assert_eq!(w(5), [1.0, 1.0, 1.0, 1.41, 1.41]);
        assert_eq!(w(6), [1.0, 1.0, 1.0, 0.0, 1.41, 1.41]);
        assert_eq!(w(8)[6..], [1.0, 1.0]);
    }

    // ---- EBU Tech 3341, Table 1: the minimum requirements ----
    //
    // Every case that is a synthesized signal. Cases 7 and 8 are "authentic
    // programme" recordings that exist only as the EBU's files, and 15-23 are
    // true peak, which `resample` answers.

    /// **Case 1** -- stereo 1 kHz at −23 dBFS for 20 s: M, S, I = −23.0 ±0.1.
    #[test]
    fn tech3341_case_1_minus_23_tone() {
        let x = stereo_programme(FS, &[stereo(20.0, Some(-23.0))]);
        let mut m = LoudnessMeter::new(2, FS);
        m.feed(&x);
        within(m.momentary(), -23.0, 0.1, "M");
        within(m.short_term(), -23.0, 0.1, "S");
        within(m.integrated(), -23.0, 0.1, "I");
    }

    /// **Case 2** -- as case 1 at −33 dBFS: M, S, I = −33.0 ±0.1.
    #[test]
    fn tech3341_case_2_minus_33_tone() {
        let x = stereo_programme(FS, &[stereo(20.0, Some(-33.0))]);
        let mut m = LoudnessMeter::new(2, FS);
        m.feed(&x);
        within(m.momentary(), -33.0, 0.1, "M");
        within(m.short_term(), -33.0, 0.1, "S");
        within(m.integrated(), -33.0, 0.1, "I");
    }

    /// **Case 3** -- 10 s at −36, 60 s at −23, 10 s at −36: I = −23.0 ±0.1.
    /// The quiet tones sit under the relative gate.
    #[test]
    fn tech3341_case_3_the_relative_gate() {
        let x = stereo_programme(
            FS,
            &[
                stereo(10.0, Some(-36.0)),
                stereo(60.0, Some(-23.0)),
                stereo(10.0, Some(-36.0)),
            ],
        );
        within(loudness(&x, 2, FS).unwrap().integrated, -23.0, 0.1, "I");
    }

    /// **Case 4** -- case 3 between two 10 s tones at −72 dBFS: I = −23.0 ±0.1.
    /// The quietest tones sit under the absolute gate too.
    #[test]
    fn tech3341_case_4_the_absolute_gate() {
        let x = stereo_programme(
            FS,
            &[
                stereo(10.0, Some(-72.0)),
                stereo(10.0, Some(-36.0)),
                stereo(60.0, Some(-23.0)),
                stereo(10.0, Some(-36.0)),
                stereo(10.0, Some(-72.0)),
            ],
        );
        within(loudness(&x, 2, FS).unwrap().integrated, -23.0, 0.1, "I");
    }

    /// **Case 5** -- 20 s at −26, 20.1 s at −20, 20 s at −26: I = −23.0 ±0.1.
    /// Nothing is gated out, so this is the energy mean itself.
    #[test]
    fn tech3341_case_5_nothing_gated() {
        let x = stereo_programme(
            FS,
            &[
                stereo(20.0, Some(-26.0)),
                stereo(20.1, Some(-20.0)),
                stereo(20.0, Some(-26.0)),
            ],
        );
        within(loudness(&x, 2, FS).unwrap().integrated, -23.0, 0.1, "I");
    }

    /// **Case 6** -- 5.0 channels of 1 kHz for 20 s, at −28 dBFS in L and R,
    /// −24 in C and −30 in Ls and Rs: I = −23.0 ±0.1. The surrounds' 1.41 is
    /// what makes it add up.
    #[test]
    fn tech3341_case_6_five_channels() {
        let levels = [
            Some(-28.0),
            Some(-28.0),
            Some(-24.0),
            Some(-30.0),
            Some(-30.0),
        ];
        let x = programme(FS, &[(20.0, &levels)]);
        within(loudness(&x, 5, FS).unwrap().integrated, -23.0, 0.1, "I");
    }

    /// **Case 9** -- (1.34 s at −20, 1.66 s at −30) five times: S = −23.0 ±0.1,
    /// constant after 3 s. Every 3 s window holds the same energy wherever it
    /// starts, so the reading must not move -- checked every 10 ms.
    #[test]
    fn tech3341_case_9_short_term_is_constant() {
        let one = [stereo(1.34, Some(-20.0)), stereo(1.66, Some(-30.0))];
        let x = stereo_programme(FS, &one.repeat(5));
        let mut m = LoudnessMeter::new(2, FS);
        let step = 2 * (FS * 0.01) as usize;
        let first = 2 * (FS * 3.0) as usize;
        m.feed(&x[..first]);
        for chunk in x[first..].chunks(step) {
            within(m.short_term(), -23.0, 0.1, "S");
            m.feed(chunk);
        }
        within(m.short_term(), -23.0, 0.1, "S at the end");
    }

    /// **Case 10**, for file-based meters -- twenty files of (i·0.15 s of
    /// silence, 3 s at −23, 1 s of silence): max S = −23.0 ±0.1 for each.
    #[test]
    fn tech3341_case_10_short_term_max_per_file() {
        for i in 0..20 {
            let x = stereo_programme(
                FS,
                &[
                    stereo(i as f64 * 0.15, None),
                    stereo(3.0, Some(-23.0)),
                    stereo(1.0, None),
                ],
            );
            within(
                loudness(&x, 2, FS).unwrap().short_term_max,
                -23.0,
                0.1,
                &format!("file {i}"),
            );
        }
    }

    /// **Case 11**, for live meters -- one signal of twenty (i·0.15 s of
    /// silence, 3 s at −38+i, 3−i·0.15 s of silence): max S reads −38, −37,
    /// …, −19 ±0.1 in succession. Read after each segment of one live meter.
    #[test]
    fn tech3341_case_11_short_term_max_live() {
        let mut m = LoudnessMeter::new(2, FS);
        for i in 0..20 {
            let silence = i as f64 * 0.15;
            m.feed(&stereo_programme(
                FS,
                &[
                    stereo(silence, None),
                    stereo(3.0, Some(-38.0 + i as f64)),
                    stereo(3.0 - silence, None),
                ],
            ));
            within(
                m.short_term_max(),
                -38.0 + i as f64,
                0.1,
                &format!("tone {i}"),
            );
        }
    }

    /// **Case 12** -- (0.18 s at −20, 0.22 s at −30) 25 times: M = −23.0 ±0.1,
    /// constant after 1 s. Checked every 5 ms.
    #[test]
    fn tech3341_case_12_momentary_is_constant() {
        let one = [stereo(0.18, Some(-20.0)), stereo(0.22, Some(-30.0))];
        let x = stereo_programme(FS, &one.repeat(25));
        let mut m = LoudnessMeter::new(2, FS);
        let step = 2 * (FS * 0.005) as usize;
        let first = 2 * FS as usize;
        m.feed(&x[..first]);
        for chunk in x[first..].chunks(step) {
            within(m.momentary(), -23.0, 0.1, "M");
            m.feed(chunk);
        }
    }

    /// **Case 13**, for file-based meters -- twenty files of (i·20 ms of
    /// silence, 400 ms at −23, 1 s of silence): max M = −23.0 ±0.1 for each.
    /// A meter that reads its momentary window only every 100 ms misses up to
    /// 40 ms of the tone (−0.46 LU) and fails this case, which is what it is
    /// for.
    #[test]
    fn tech3341_case_13_momentary_max_per_file() {
        for i in 0..20 {
            let x = stereo_programme(
                FS,
                &[
                    stereo(i as f64 * 0.02, None),
                    stereo(0.4, Some(-23.0)),
                    stereo(1.0, None),
                ],
            );
            within(
                loudness(&x, 2, FS).unwrap().momentary_max,
                -23.0,
                0.1,
                &format!("file {i}"),
            );
        }
    }

    /// **Case 14**, for live meters -- twenty (i·20 ms of silence, 400 ms at
    /// −38+i, 400−i·20 ms of silence): max M reads −38, −37, …, −19 ±0.1.
    #[test]
    fn tech3341_case_14_momentary_max_live() {
        let mut m = LoudnessMeter::new(2, FS);
        for i in 0..20 {
            let silence = i as f64 * 0.02;
            m.feed(&stereo_programme(
                FS,
                &[
                    stereo(silence, None),
                    stereo(0.4, Some(-38.0 + i as f64)),
                    stereo(0.4 - silence, None),
                ],
            ));
            within(
                m.momentary_max(),
                -38.0 + i as f64,
                0.1,
                &format!("tone {i}"),
            );
        }
    }

    // ---- EBU Tech 3342, Table 1: the loudness range ----
    //
    // Cases 1-4; 5 and 6 are authentic programmes, as in Tech 3341.

    fn range_of(levels: &[f64]) -> f64 {
        let segments: Vec<_> = levels.iter().map(|&db| stereo(20.0, Some(db))).collect();
        let x = stereo_programme(FS, &segments);
        loudness(&x, 2, FS).unwrap().range
    }

    /// **Case 1** -- 20 s at −20 dBFS then 20 s at −30: LRA = 10 ±1 LU.
    #[test]
    fn tech3342_case_1_ten_lu_apart() {
        within(range_of(&[-20.0, -30.0]), 10.0, 1.0, "LRA");
    }

    /// **Case 2** -- −20 then −15: LRA = 5 ±1 LU.
    #[test]
    fn tech3342_case_2_five_lu_apart() {
        within(range_of(&[-20.0, -15.0]), 5.0, 1.0, "LRA");
    }

    /// **Case 3** -- −40 then −20: LRA = 20 ±1 LU. The quiet tone sits right on
    /// the relative gate, 20 LU under the mean.
    #[test]
    fn tech3342_case_3_twenty_lu_apart() {
        within(range_of(&[-40.0, -20.0]), 20.0, 1.0, "LRA");
    }

    /// **Case 4** -- −50, −35, −20, −35, −50, 20 s each: LRA = 15 ±1 LU. The
    /// −50 tones fall under the relative gate.
    #[test]
    fn tech3342_case_4_five_segments() {
        within(
            range_of(&[-50.0, -35.0, -20.0, -35.0, -50.0]),
            15.0,
            1.0,
            "LRA",
        );
    }

    /// "The expected response is unchanged if the test signal is repeated one
    /// or more times in its full length" (Tech 3342, and true of every
    /// integrated reading): a statistic over the blocks, not over time.
    #[test]
    fn a_repeated_programme_measures_the_same() {
        let one = stereo_programme(FS, &[stereo(20.0, Some(-20.0)), stereo(20.0, Some(-30.0))]);
        let twice = [one.as_slice(), one.as_slice()].concat();
        let (a, b) = (
            loudness(&one, 2, FS).unwrap(),
            loudness(&twice, 2, FS).unwrap(),
        );
        within(b.range, a.range, 0.1, "LRA");
        within(b.integrated, a.integrated, 0.02, "I");
    }

    // ---- the meter as a meter ----

    /// **Fed in any blocks, one reading.** The one-shot is a loop over the
    /// meter, and a live path feeds it whatever a callback holds, so the
    /// block boundaries must not show in any number.
    #[test]
    fn the_block_size_does_not_matter() {
        let x = stereo_programme(
            44_100.0,
            &[
                stereo(5.0, Some(-30.0)),
                stereo(7.3, Some(-18.0)),
                stereo(4.0, Some(-41.0)),
            ],
        );
        let whole = loudness(&x, 2, 44_100.0).unwrap();
        for block in [2, 128, 1000, 44_100] {
            let mut m = LoudnessMeter::new(2, 44_100.0);
            for chunk in x.chunks(block) {
                m.feed(chunk);
            }
            assert_eq!(m.summary(), whole, "blocks of {block}");
        }
    }

    /// Silence is nothing to measure: no block passes the absolute gate.
    #[test]
    fn silence_reads_nothing() {
        let l = loudness(&vec![0.0; 96_000 * 2], 2, FS).unwrap();
        assert_eq!(l.integrated, f64::NEG_INFINITY);
        assert_eq!(l.range, 0.0);
        assert_eq!(l.momentary_max, f64::NEG_INFINITY);
        let mut m = LoudnessMeter::new(2, FS);
        assert_eq!(m.integrated(), f64::NEG_INFINITY);
        m.feed(&[0.5; 64]);
        assert!(m.momentary().is_finite());
    }

    /// Too short for one gating block is no integrated loudness, and a block
    /// that did not end is not used (BS.1770: "incomplete gating blocks at the
    /// end of the measurement interval are not used").
    #[test]
    fn an_incomplete_block_is_not_used() {
        let x = stereo_programme(FS, &[stereo(0.39, Some(-20.0))]);
        assert_eq!(loudness(&x, 2, FS).unwrap().integrated, f64::NEG_INFINITY);
        let x = stereo_programme(FS, &[stereo(0.4, Some(-20.0))]);
        assert!(loudness(&x, 2, FS).unwrap().integrated.is_finite());
    }

    /// A reset forgets everything, so a meter reused reads what a new one would.
    #[test]
    fn a_reset_meter_is_a_new_meter() {
        let loud = stereo_programme(FS, &[stereo(4.0, Some(-10.0))]);
        let quiet = stereo_programme(FS, &[stereo(4.0, Some(-40.0))]);
        let mut m = LoudnessMeter::new(2, FS);
        m.feed(&loud);
        m.reset();
        m.feed(&quiet);
        assert_eq!(m.summary(), loudness(&quiet, 2, FS).unwrap());
    }

    /// A weight of 0 leaves a channel out, whatever it holds -- the LFE.
    #[test]
    fn a_zero_weight_leaves_a_channel_out() {
        let with_lfe = programme(
            FS,
            &[(
                3.0,
                &[
                    Some(-20.0),
                    Some(-20.0),
                    Some(-20.0),
                    Some(0.0),
                    Some(-20.0),
                    Some(-20.0),
                ],
            )],
        );
        let silent_lfe = programme(
            FS,
            &[(
                3.0,
                &[
                    Some(-20.0),
                    Some(-20.0),
                    Some(-20.0),
                    None,
                    Some(-20.0),
                    Some(-20.0),
                ],
            )],
        );
        assert_eq!(
            loudness(&with_lfe, 6, FS),
            loudness(&silent_lfe, 6, FS),
            "the LFE is not measured"
        );
    }

    // ---- the profile: the same numbers, read anywhere ----

    /// **A profile is the meter, asked backwards.** The meter reports where it
    /// has got to; the profile answers for any point of the take. So every
    /// reading of one must be a reading of the other, and this is the test the
    /// whole type rests on: a meter stopped at a point and the profile asked
    /// about that point agree to a hundredth of a LU, which is a tenth of the
    /// tolerance Tech 3341 allows a meter at all.
    #[test]
    fn a_profile_reads_what_a_meter_reads_at_the_same_point() {
        let x = stereo_programme(
            FS,
            &[
                stereo(4.0, Some(-30.0)),
                stereo(5.0, Some(-14.0)),
                stereo(4.0, Some(-24.0)),
            ],
        );
        let profile = Profile::from_interleaved(&x, 2, FS).unwrap();
        for seconds in [3.5, 4.0, 6.25, 9.0, 12.0] {
            let frames = (seconds * FS) as usize;
            let mut meter = LoudnessMeter::new(2, FS);
            meter.feed(&x[..frames * 2]);
            within(
                profile.momentary_at(frames as u64),
                meter.momentary(),
                0.01,
                &format!("momentary at {seconds} s"),
            );
            within(
                profile.short_term_at(frames as u64),
                meter.short_term(),
                0.01,
                &format!("short-term at {seconds} s"),
            );
        }
    }

    /// And the aggregates over the whole take are the meter's too -- the gating
    /// the profile runs over its blocks is the gating the meter runs over its
    /// histogram.
    #[test]
    fn a_profile_measures_a_span_as_a_meter_measures_a_pass() {
        let x = stereo_programme(
            FS,
            &[
                stereo(6.0, Some(-36.0)),
                stereo(8.0, Some(-18.0)),
                stereo(6.0, Some(-28.0)),
            ],
        );
        let whole = loudness(&x, 2, FS).unwrap();
        let measured = Profile::from_interleaved(&x, 2, FS)
            .unwrap()
            .measure(0, (x.len() / 2) as u64);
        within(measured.integrated, whole.integrated, 0.02, "I");
        within(measured.range, whole.range, 0.2, "LRA");
        within(measured.momentary_max, whole.momentary_max, 0.05, "max M");
        within(measured.short_term_max, whole.short_term_max, 0.05, "max S");
    }

    /// **A span is measured as a meter reset there would measure it**, which is
    /// what makes the read-out over a selection mean anything: the loud half of
    /// a take reads the loud half's loudness, not the take's.
    #[test]
    fn a_span_is_measured_on_its_own() {
        let quiet = stereo_programme(FS, &[stereo(8.0, Some(-33.0))]);
        let loud = stereo_programme(FS, &[stereo(8.0, Some(-16.0))]);
        let both = [quiet.as_slice(), loud.as_slice()].concat();
        let profile = Profile::from_interleaved(&both, 2, FS).unwrap();
        let half = (8.0 * FS) as u64;
        within(
            profile.measure(half, half).integrated,
            -16.0,
            0.1,
            "the loud half",
        );
        within(
            profile.measure(0, half).integrated,
            -33.0,
            0.1,
            "the quiet half",
        );
    }

    /// **A column is a band, not a sample of the curve**: over a span holding
    /// a step, it reaches from the quiet reading to the loud one, so a zoomed
    /// out picture cannot hide the excursion between two pixels.
    #[test]
    fn a_column_spans_the_readings_it_covers() {
        let x = stereo_programme(FS, &[stereo(5.0, Some(-35.0)), stereo(5.0, Some(-15.0))]);
        let profile = Profile::from_interleaved(&x, 2, FS).unwrap();
        let (lo, hi) = profile
            .column(MOMENTARY_SECONDS, (4.0 * FS) as u64, (6.0 * FS) as u64)
            .expect("a column over the step");
        within(lo as f64, -35.0, 0.2, "the quiet edge");
        within(hi as f64, -15.0, 0.2, "the loud edge");
        // A column inside the quiet tone is the tone, top and bottom alike.
        let (lo, hi) = profile
            .column(MOMENTARY_SECONDS, (2.0 * FS) as u64, (3.0 * FS) as u64)
            .unwrap();
        within(lo as f64, -35.0, 0.1, "steady, low");
        within(hi as f64, -35.0, 0.1, "steady, high");
        assert!(profile.column(MOMENTARY_SECONDS, 100, 100).is_none());
    }

    /// **An edit re-measures its own span**, and the result is the profile a
    /// fresh pass would have built -- the curve after a stroke is not an
    /// approximation of the curve, it is the curve.
    #[test]
    fn an_edited_span_is_re_measured_exactly() {
        let mut x = stereo_programme(FS, &[stereo(10.0, Some(-24.0))]);
        let mut profile = Profile::from_interleaved(&x, 2, FS).unwrap();
        // A second of the take is replaced by a much louder tone.
        let (from, len) = ((4.0 * FS) as usize, FS as usize);
        let loud = stereo_programme(FS, &[stereo(1.0, Some(-12.0))]);
        x[from * 2..(from + len) * 2].copy_from_slice(&loud[..len * 2]);
        let sources: Vec<crate::peaks::Interleaved<'_>> = (0..2)
            .map(|c| crate::peaks::Interleaved::new(&x, 2, c))
            .collect();
        let refs: Vec<&dyn crate::peaks::Source> = sources
            .iter()
            .map(|s| s as &dyn crate::peaks::Source)
            .collect();
        assert!(profile.update(&refs, from as u64, len as u64));
        let fresh = Profile::from_interleaved(&x, 2, FS).unwrap();
        for seconds in [3.0, 4.2, 4.9, 5.4, 7.0, 10.0] {
            let at = (seconds * FS) as u64;
            within(
                profile.momentary_at(at),
                fresh.momentary_at(at),
                0.001,
                &format!("momentary at {seconds} s"),
            );
            within(
                profile.short_term_at(at),
                fresh.short_term_at(at),
                0.001,
                &format!("short-term at {seconds} s"),
            );
        }
        let n = (x.len() / 2) as u64;
        within(
            profile.measure(0, n).integrated,
            fresh.measure(0, n).integrated,
            0.001,
            "I",
        );
    }

    /// Silence has no loudness, and a profile says so rather than drawing a
    /// floor: every reading is `-inf` and the span measures nothing.
    #[test]
    fn a_silent_take_has_no_loudness_anywhere() {
        let profile = Profile::from_interleaved(&vec![0.0; 48_000 * 2], 2, FS).unwrap();
        assert_eq!(profile.momentary_at(24_000), f64::NEG_INFINITY);
        let measured = profile.measure(0, 24_000);
        assert_eq!(measured.integrated, f64::NEG_INFINITY);
        assert_eq!(measured.range, 0.0);
    }

    #[test]
    fn a_profile_states_what_it_describes() {
        let x = stereo_programme(FS, &[stereo(1.0, Some(-20.0))]);
        let profile = Profile::from_interleaved(&x, 2, FS).unwrap();
        assert!(profile.describes(2, (x.len() / 2) as u64));
        assert!(!profile.describes(1, (x.len() / 2) as u64));
        assert!(!profile.describes(2, 999));
        assert_eq!(profile.rate(), FS);
        assert!(Profile::from_interleaved(&x, 0, FS).is_none());
        assert!(Profile::from_interleaved(&x, 2, 20.0).is_none());
    }

    #[test]
    fn a_request_that_cannot_be_met_is_none() {
        assert!(loudness(&[0.0; 8], 0, FS).is_none());
        assert!(loudness(&[0.0; 8], 2, 4.0).is_none());
        assert!(loudness_with_weights(&[0.0; 8], &[], FS).is_none());
    }
}
