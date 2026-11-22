//! Beat tracking: `BeatTrack` (Davies and Plumbley's context-dependent
//! tracker over an `FFT` chain) and `BeatTrack2` (template matching over
//! features on control buses), both Nick Collins's, as scsynth ships them.
//!
//! Both spread their search over the control blocks after each analysis,
//! one step a block, as scsynth's do. A block's step is taken on the slice
//! that starts it (`ProcessCtx::offset` 0), so however the engine cuts the
//! block the steps fall on the same blocks; the phase, and with it the
//! triggers, advances by the slice's samples.
//!
//! Each output is a row of its own (`chan`): the trackers are deterministic,
//! so the rows of one call keep the same state. The departures from
//! scsynth's code are named where they are made.

use crate::dsp::features::Geometry;
use crate::dsp::registry::UGenConfig;
use crate::dsp::spectral::SpectralChain;
use crate::dsp::{BLOCK_SIZE, ProcessCtx, UGen, at};

/// The detection-function frames one analysis reads (about 6 s at scsynth's
/// hop of 512 samples at 44.1 kHz).
const DF_LEN: usize = 512;
/// The detection-function frames kept.
const DF_STORE: usize = 700;
/// The beat periods searched, in detection-function frames.
const LAGS: usize = 128;
/// Frames between analyses.
const SKIP: u64 = 128;
/// The variance of the context model's Gaussian, in frames squared; its
/// square root bounds a change of period the tracker takes as one.
const CONTEXT_VARIANCE: f32 = 3.9017;

/// The general model's weight of period `j + 1` frames: a Rayleigh
/// distribution peaking at 43 frames (scsynth's `g_m`, which is this to its
/// eight digits).
fn rayleigh(j: usize) -> f32 {
    let (t, b) = ((j + 1) as f32, 43.0f32);
    t / (b * b) * (-t * t / (2.0 * b * b)).exp()
}

/// The context model's weight `d` frames from the period it is centred on:
/// a Gaussian (scsynth's `g_mg`, which is this to its eight digits).
fn context(d: f32) -> f32 {
    (-d * d / (2.0 * CONTEXT_VARIANCE)).exp() / (std::f32::consts::TAU * CONTEXT_VARIANCE).sqrt()
}

/// `BeatTrack(chain, lock, chan)`: the beat of the audio the chain analyses.
/// Output `chan` 0 is a trigger on each beat, 1 on each half beat, 2 on each
/// quarter, and 3 the tempo in beats per second. While `lock` is above 0.5
/// the outputs keep the period and phase they had, and the tracker goes on
/// following.
///
/// Every frame gives a complex-domain detection function (each bin's
/// distance from its prediction by the last two frames, summed and peak-
/// picked over the last 15 frames). Every 128 frames the last 512 of it are
/// autocorrelated, a comb over the autocorrelation finds the period (in the
/// general state weighted towards about 43 frames, in the context state
/// around the period it holds), a consistent change of period over three
/// analyses moves the context, and the beat's phase is the one the
/// detection function lines up with best (in the context state near where
/// the beat is expected).
///
/// scsynth turns its periods into seconds at a hop of 512 samples at 44.1 kHz
/// (half of it at twice the rate); here at the chain's own hop, so the tempo
/// is right through any chain (the models are tuned to about 11.6 ms a
/// frame). Its comb searches the general period with the context model and
/// the context period with the general one, its two arrays swapped against
/// the comments that name them and against Davies and Plumbley's states
/// (and the first search reads the context array before anything has set
/// it); here each state uses its own model. It multiplies the
/// autocorrelation by the number of terms each lag has where the
/// unbiased estimate divides by it; here it divides. Its expected-phase
/// weighting measures the distance across the period's end the wrong way,
/// and its period search reads past the autocorrelation at the longest
/// periods; here the distance is circular and the search stops at the end.
pub struct BeatTrack {
    chan: usize,
    half: usize,
    frame_period: f32,
    sr: f32,
    prev_mag: Vec<f32>,
    prev_phase: Vec<f32>,
    predict: Vec<f32>,
    frame_count: u64,
    df: Box<[f32; DF_STORE]>,
    df_at: usize,
    df_memory: [f32; 15],
    df_memory_at: usize,
    acf: Box<[f32; DF_LEN]>,
    /// The context model, centred on the period it holds.
    mg: Box<[f32; LAGS]>,
    best_tor_sum: f32,
    best_column: usize,
    tor: f32,
    tor_round: i32,
    period_p: f32,
    period_g: f32,
    flagstep: i32,
    prev_period_p: [f32; 3],
    best_phase_score: f32,
    best_phase: i32,
    /// The standard deviation of the expected-phase weighting, set at the
    /// last change of context.
    phase_sigma: f32,
    tempo: f32,
    curr_phase: f32,
    phase: f32,
    out_phase: f32,
    out_tempo: f32,
    out_rate: f32,
    half_trig: bool,
    q1_trig: bool,
    q2_trig: bool,
    state: u8,
    count: i32,
    length: i32,
    steps: u64,
    context_state: bool,
    timesig: i32,
    store_from: usize,
    store_end: usize,
}

impl BeatTrack {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        let g = Geometry::new(config, sr);
        let chan = (config.constant(2, 0.0).max(0.0) as usize).min(3);
        let mut t = Self {
            chan,
            half: g.half,
            frame_period: g.frame_period(),
            sr,
            prev_mag: vec![0.0; g.half],
            prev_phase: vec![0.0; g.half],
            predict: vec![0.0; g.half],
            frame_count: 1,
            df: Box::new([0.0; DF_STORE]),
            df_at: DF_STORE - 1,
            df_memory: [0.0; 15],
            df_memory_at: 14,
            acf: Box::new([0.0; DF_LEN]),
            mg: Box::new([0.0; LAGS]),
            best_tor_sum: -1000.0,
            best_column: 0,
            tor: 0.0,
            tor_round: 0,
            period_p: 0.0,
            period_g: 0.0,
            flagstep: 0,
            prev_period_p: [0.0; 3],
            best_phase_score: -1000.0,
            best_phase: 0,
            phase_sigma: 1.0,
            tempo: 2.0,
            curr_phase: 0.0,
            phase: 0.0,
            out_phase: 0.0,
            out_tempo: 2.0,
            out_rate: 2.0,
            half_trig: false,
            q1_trig: false,
            q2_trig: false,
            state: 0,
            count: 0,
            length: 0,
            steps: 0,
            context_state: false,
            timesig: 4,
            store_from: 0,
            store_end: 0,
        };
        t.reset();
        t
    }

    /// The complex-domain detection function of a frame, peak-picked.
    fn detection(&mut self, frame: &[f32]) {
        let mut sum = 0.0f32;
        for k in 1..self.half {
            let (re, im) = (frame[2 * k], frame[2 * k + 1]);
            let mag = re.hypot(im);
            let qmag = self.prev_mag[k];
            self.prev_mag[k] = mag;
            let phase = im.atan2(re);
            let old = self.predict[k];
            self.predict[k] = 2.0 * phase - self.prev_phase[k];
            self.prev_phase[k] = phase;
            let d = phase - old;
            let (real, imag) = (qmag - mag * d.cos(), mag * d.sin());
            sum += real.hypot(imag);
        }
        self.df_memory_at = (self.df_memory_at + 1) % 15;
        self.df_memory[self.df_memory_at] = sum;
        let refpos = self.df_memory_at + 15;
        let centre = self.df_memory[(refpos - 7) % 15];
        let mut rating = 0.0f32;
        for k in 0..15 {
            let mut v = centre - self.df_memory[(refpos - k) % 15];
            if v < 0.0 {
                v *= 10.0;
            }
            rating += v;
        }
        self.df_at = (self.df_at + 1) % DF_STORE;
        self.df[self.df_at] = rating.max(0.0) * 0.1;
    }

    /// Four lags of the autocorrelation of the stored detection function.
    fn autocorr(&mut self, j: usize) {
        let base = self.store_from + DF_STORE;
        for k in 0..4 {
            let lag = 4 * j + k;
            let mut sum = 0.0f32;
            for i in lag..DF_LEN {
                sum += self.df[(i + base) % DF_STORE] * self.df[(i + base - lag) % DF_STORE];
            }
            self.acf[lag] = sum / (DF_LEN - lag) as f32;
        }
    }

    /// One period of the comb over the autocorrelation, weighted by the
    /// context model or the general one.
    fn beat_period(&mut self, j: usize, context_model: bool) {
        let mut sum = 0.0f32;
        for i in 1..=self.timesig as usize {
            let num = 2 * i - 1;
            let wt = 1.0 / num as f32;
            for k in 0..num {
                let pos = k + i * j;
                if pos < DF_LEN {
                    sum += self.acf[pos] * wt;
                }
            }
        }
        sum *= if context_model {
            self.mg[j]
        } else {
            rayleigh(j)
        };
        if sum > self.best_tor_sum {
            self.best_tor_sum = sum;
            self.best_column = j;
        }
    }

    /// The period the best comb column points at, refined by the peaks of
    /// the autocorrelation at its multiples (scsynth's arithmetic, offsets
    /// and all).
    fn find_tor(&self) -> f32 {
        let ind = self.best_column as i32 + 1;
        // `acf` read one-based, as scsynth reads it.
        let acf = |i: i32| -> Option<f32> {
            let k = i - 1;
            (k >= 0 && (k as usize) < DF_LEN).then(|| self.acf[k as usize])
        };
        let peak = |lo: i32, hi: i32| {
            let (mut best, mut at) = (-1000.0f32, 0);
            for i in lo..=hi {
                if let Some(v) = acf(i)
                    && v > best
                {
                    best = v;
                    at = i - lo + 1;
                }
            }
            at
        };
        let ind2 = peak(2 * ind - 1, 2 * ind + 1) + 2 * (ind + 1) - 2;
        let ind3 = peak(3 * ind - 2, 3 * ind + 2) + 3 * ind - 4;
        if self.timesig == 4 {
            let ind4 = peak(4 * ind - 3, 4 * ind + 3) + 4 * ind - 9;
            (ind as f32 + ind2 as f32 * 0.5 + ind3 as f32 / 3.0 + ind4 as f32 * 0.25) * 0.25
        } else {
            (ind as f32 + ind2 as f32 * 0.5 + ind3 as f32 / 3.0) * 0.333_333_3
        }
    }

    /// One candidate phase of the beat against the detection function.
    fn find_phase(&mut self, j: i32, expect: bool, predicted: i32) {
        let period = self.tor_round;
        let base = (self.store_end + DF_STORE) as i32;
        let fits = if period != 0 {
            DF_LEN as i32 / period - 1
        } else {
            -1
        };
        let mut sum = 0.0f32;
        for k in 0..fits.max(0) {
            let at = (base - period * k - j).rem_euclid(DF_STORE as i32) as usize;
            sum += self.df[at] / (k + 1) as f32;
        }
        if expect && period > 0 {
            let d = (predicted - j).rem_euclid(period);
            let diff = d.min(period - d) as f32;
            let s = self.phase_sigma;
            sum *= (-(diff * diff) / (2.0 * s * s)).exp() / (2.506_628_3 * s);
        }
        if sum > self.best_phase_score {
            self.best_phase_score = sum;
            self.best_phase = j;
        }
    }

    fn period_changed(&mut self) -> bool {
        if self.flagstep == 0 {
            if (self.period_g - self.period_p).abs() > CONTEXT_VARIANCE {
                self.flagstep = 3;
            }
        } else {
            self.flagstep -= 1;
        }
        if self.flagstep != 0 {
            self.prev_period_p[(self.flagstep - 1) as usize] = self.period_p;
        }
        if self.flagstep == 1 {
            self.flagstep = 0;
            let p = &self.prev_period_p;
            if (2.0 * p[0] - p[1] - p[2]).abs() < 2.0 * CONTEXT_VARIANCE {
                return true;
            }
        }
        false
    }

    fn start_comb(&mut self, state: u8) {
        self.state = state;
        self.length = LAGS as i32;
        self.count = 0;
        self.best_column = 0;
        self.best_tor_sum = -1000.0;
    }

    /// The step of the search a control block takes.
    fn step(&mut self) {
        self.steps += 1;
        match self.state {
            1 => {
                self.autocorr(self.count as usize);
                self.count += 1;
                if self.count == self.length {
                    self.start_comb(2);
                }
            }
            2 => {
                self.beat_period(self.count as usize, false);
                self.count += 1;
                if self.count == self.length {
                    self.period_p = self.find_tor();
                    if self.context_state {
                        self.start_comb(3);
                    } else {
                        self.period_g = -1000.0;
                        self.state = 4;
                    }
                }
            }
            3 => {
                self.beat_period(self.count as usize, true);
                self.count += 1;
                if self.count == self.length {
                    self.period_g = self.find_tor();
                    self.state = 4;
                }
            }
            4 => {
                if self.period_changed() {
                    self.start_comb(5);
                    self.context_state = true;
                    self.timesig = 4;
                    let centre = (self.period_p + 0.5) as i32;
                    for (i, m) in self.mg.iter_mut().enumerate() {
                        *m = context((i as i32 - centre) as f32);
                    }
                } else {
                    self.tor = if self.context_state {
                        self.period_g
                    } else {
                        self.period_p
                    };
                    self.tor_round = (self.tor + 0.5) as i32;
                    self.state = 7;
                    self.length = self.tor_round;
                    self.count = 0;
                }
            }
            5 => {
                self.beat_period(self.count as usize, true);
                self.count += 1;
                if self.count == self.length {
                    self.period_g = self.find_tor();
                    self.tor = self.period_g;
                    self.tor_round = (self.tor + 0.5) as i32;
                    self.state = 6;
                    self.length = self.tor_round;
                    self.count = 0;
                    self.phase_sigma = self.tor_round as f32 * 0.25;
                }
            }
            6 | 7 => {
                if self.length < 1 {
                    // No period to phase against: keep the beat as it is.
                    self.state = 0;
                    return;
                }
                let expect = self.state == 7 && self.context_state;
                let predicted = (self.curr_phase * self.tor_round as f32 + 0.5) as i32;
                self.find_phase(self.count, expect, predicted);
                self.count += 1;
                if self.count == self.length {
                    self.state = 8;
                }
            }
            8 => {
                self.tempo = 1.0 / (self.tor * self.frame_period);
                let elapsed =
                    self.steps as f32 * BLOCK_SIZE as f32 / self.sr + 7.0 * self.frame_period;
                let before = self.best_phase as f32 / self.tor_round as f32;
                self.phase = (before + elapsed * self.tempo) % 1.0;
                self.curr_phase = self.phase;
                self.state = 0;
            }
            _ => {}
        }
    }
}

impl UGen for BeatTrack {
    fn reset(&mut self) {
        self.prev_mag.fill(0.0);
        self.prev_phase.fill(0.0);
        self.predict.fill(0.0);
        self.frame_count = 1;
        self.df.fill(0.0);
        self.df_at = DF_STORE - 1;
        self.df_memory = [0.0; 15];
        self.df_memory_at = 14;
        self.acf.fill(0.0);
        self.mg.fill(0.0);
        self.flagstep = 0;
        self.prev_period_p = [0.0; 3];
        self.phase_sigma = 1.0;
        self.tempo = 2.0;
        self.curr_phase = 0.0;
        self.phase = 0.0;
        self.out_phase = 0.0;
        self.out_tempo = 2.0;
        self.out_rate = 2.0;
        self.half_trig = false;
        self.q1_trig = false;
        self.q2_trig = false;
        self.state = 0;
        self.steps = 0;
        self.context_state = false;
        self.timesig = 4;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(0.0);
    }

    fn process_spectral(
        &mut self,
        ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if ctx.offset == 0 {
            self.step();
        }
        if chain.ready {
            self.frame_count += 1;
            self.detection(&chain.frame);
            if self.frame_count.is_multiple_of(SKIP) {
                self.start_comb(1);
                self.best_phase_score = -1000.0;
                self.best_phase = 0;
                self.steps = 0;
                self.store_from = self.df_at + DF_STORE - DF_LEN;
                self.store_end = self.df_at;
                self.curr_phase = self.phase;
            }
        }
        let dt = ctx.frames as f32 / self.sr;
        self.phase += self.tempo * dt;
        let lock = inputs.get(1).map_or(0.0, |i| at(i, 0));
        if lock < 0.5 {
            self.out_phase = self.phase;
            self.out_tempo = self.tempo;
            self.out_rate = self.tempo;
        } else {
            self.out_phase += self.out_rate * dt;
        }
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let triggers = beat_triggers(
            &mut self.out_phase,
            &mut self.half_trig,
            &mut self.q1_trig,
            &mut self.q2_trig,
            0.0,
        );
        output.fill(match self.chan {
            3 => self.out_tempo,
            c => triggers[c],
        });
    }
}

/// The beat, half-beat and quarter-beat triggers of an output phase, which
/// wraps here: 1 on the slice it crosses each point. `groove` moves the
/// quarter points (`BeatTrack2`'s swing).
fn beat_triggers(
    phase: &mut f32,
    half: &mut bool,
    q1: &mut bool,
    q2: &mut bool,
    groove: f32,
) -> [f32; 3] {
    let mut out = [0.0; 3];
    if *phase >= 1.0 {
        *phase -= 1.0;
        out = [1.0; 3];
        *half = false;
        *q1 = false;
        *q2 = false;
    }
    if *phase >= 0.5 && !*half {
        out[1] = 1.0;
        out[2] = 1.0;
        *half = true;
    }
    if *phase >= 0.25 + groove && !*q1 {
        out[2] = 1.0;
        *q1 = true;
    }
    if *phase >= 0.75 + groove && !*q2 {
        out[2] = 1.0;
        *q2 = true;
    }
    out
}

/// The tempi `BeatTrack2` tries: 60 to 179 beats a minute, as beat periods
/// in seconds.
const TEMPI: usize = 120;

fn period_of(tempo: usize) -> f32 {
    60.0 / (60 + tempo) as f32
}

/// Where in a beat the four template events fall, for the straight and the
/// swung groove.
const SEP: [f32; 8] = [0.0, 0.25, 0.5, 0.75, 0.0, 0.32, 0.5, 0.82];
/// The weight of each template event.
const EVENT_WEIGHT: [f32; 4] = [1.0, 0.5, 0.9, 0.6];
/// The weight of each of the nine blocks around an event.
const SPREAD: [f32; 9] = [0.05, 0.1, 0.3, 0.7, 1.0, 0.7, 0.3, 0.1, 0.05];

/// How `BeatTrack2` weighs its tempi.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TempoWeights {
    Flat,
    /// By one over the events each tempo fits in the window.
    Compensated,
    /// From the buffer of 120 weights the def names.
    Buffer(usize),
}

/// `BeatTrack2(busindex, numfeatures, windowsize, phaseaccuracy, lock,
/// weightingscheme, chan)`: the beat of `numfeatures` features on the
/// control buses from `busindex`. Output `chan` 0 is a trigger on each
/// beat, 1 on each half beat, 2 on each quarter (late by the groove), 3 the
/// tempo in beats per second, 4 the beat's phase (0 to 1) and 5 the groove
/// (0 straight, 1 swung). Twice a second every feature's last `windowsize`
/// seconds are matched against a template of four events a beat, straight
/// and swung, at each of the 120 tempi and every phase to `phaseaccuracy`
/// seconds; the tempo the features agree on most, if it is the one the
/// last match predicted, is taken. `lock` above 0.5 holds the outputs, as
/// `BeatTrack`'s. `weightingscheme` is -2.5 for flat weights, -1.5 to
/// compensate each tempo for the events that fit the window, or a buffer
/// of 120 weights. `numfeatures`, `windowsize`, `phaseaccuracy` and
/// `weightingscheme` are read when the synth is built.
///
/// scsynth's test for the compensated scheme is never true (`bufnum < 0 ?
/// (bufnum < 2 ? 0 : 1)`), so it always weighs flat; here -1.5 compensates.
/// It reads a buffer number past the pool as buffer 0; here a missing
/// buffer weighs flat. It never takes the groove it matches, so its groove
/// output is always 0 and its quarter triggers never swing; here the
/// groove is the match's, taken with its tempo. It measures a match's
/// phase, and the next one's predicted phase, in beats of the period it
/// holds rather than the match's own, so a match at another tempo lands at
/// a phase unrelated to the one predicted for it and is never taken: the
/// tracker stays at the 120 beats a minute it starts at. Here a phase is
/// the match's own, compared across the beat's end, and the period taken
/// is the match's.
pub struct BeatTrack2 {
    chan: usize,
    sr: f32,
    numfeatures: usize,
    window: f32,
    accuracy: f32,
    numphases: [usize; TEMPI],
    block: f32,
    weights: TempoWeights,
    size: usize,
    past: Vec<f32>,
    counter: usize,
    start: usize,
    schedule: f32,
    period: f32,
    groove: i32,
    tempo: f32,
    phase: f32,
    rate: f32,
    out_phase: f32,
    out_tempo: f32,
    out_groove: f32,
    out_rate: f32,
    predict_phase: f32,
    predict_period: f32,
    scores: Vec<f32>,
    best_score: Vec<f32>,
    best_phase: Vec<usize>,
    best_tempo: Vec<usize>,
    best_groove: Vec<usize>,
    calculating: bool,
    count: usize,
    half_trig: bool,
    q1_trig: bool,
    q2_trig: bool,
}

/// The most features a `BeatTrack2` reads: the control buses there are.
const MAX_FEATURES: usize = crate::dsp::NUM_CONTROL_BUSES;

/// Seconds between two matches.
const CALC_PERIOD: f32 = 0.5;

impl BeatTrack2 {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        let numfeatures =
            ((config.constant(1, 1.0) + 0.001) as i32).clamp(1, MAX_FEATURES as i32) as usize;
        let window = config.constant(2, 2.0).max(0.0);
        let accuracy = config.constant(3, 0.02).max(1e-4);
        let scheme = (config.constant(5, -2.1) + 0.001) as i32;
        let weights = match scheme {
            s if s < -1 => TempoWeights::Flat,
            s if s < 0 => TempoWeights::Compensated,
            s => TempoWeights::Buffer(s as usize),
        };
        let block = BLOCK_SIZE as f32 / sr;
        let size = (((window + 1.1) / block) as usize).max(1);
        let mut numphases = [0; TEMPI];
        for (j, n) in numphases.iter_mut().enumerate() {
            *n = (period_of(j) / accuracy) as usize;
        }
        let chan = (config.constant(6, 0.0).max(0.0) as usize).min(5);
        let mut t = Self {
            chan,
            sr,
            numfeatures,
            window,
            accuracy,
            numphases,
            block,
            weights,
            size,
            past: vec![0.0; numfeatures * size],
            counter: 0,
            start: 0,
            schedule: 0.0,
            period: 0.5,
            groove: 0,
            tempo: 2.0,
            phase: 0.0,
            rate: 2.0,
            out_phase: 0.0,
            out_tempo: 2.0,
            out_groove: 0.0,
            out_rate: 2.0,
            predict_phase: 0.4,
            predict_period: 0.3,
            scores: vec![0.0; 2 * numfeatures],
            best_score: vec![0.0; 4 * numfeatures],
            best_phase: vec![0; 4 * numfeatures],
            best_tempo: vec![0; 4 * numfeatures],
            best_groove: vec![0; 4 * numfeatures],
            calculating: false,
            count: 0,
            half_trig: false,
            q1_trig: false,
            q2_trig: false,
        };
        t.reset();
        t
    }

    /// One tempo and groove of the template against every feature.
    fn template(&mut self, ctx: &ProcessCtx, which: usize, groove: usize) {
        let nf = self.numfeatures;
        let period = period_of(which);
        let beats = (self.window / period) as usize;
        let weight = match self.weights {
            TempoWeights::Flat => 1.0,
            TempoWeights::Compensated if beats > 0 => 1.0 / (beats * 4) as f32,
            TempoWeights::Compensated => 0.0,
            TempoWeights::Buffer(b) => ctx
                .buffer(b)
                .filter(|buf| which < buf.frames())
                .map_or(1.0, |buf| buf.sample(which, 0)),
        };
        let size = self.size;
        for i in 0..self.numphases[which] {
            for k in 0..nf {
                self.scores[2 * k + groove] = 0.0;
            }
            let add = i as f32 * self.accuracy;
            for h in 0..beats {
                for l in 0..4 {
                    let sep = add + h as f32 * period + SEP[groove * 4 + l] * period;
                    let blocks = (sep / self.block + 0.5) as usize;
                    let index = (self.start + size * 2 - blocks % size) % size;
                    for (m, &spread) in SPREAD.iter().enumerate() {
                        let at = (index + size * 2 + m - 4) % size;
                        let w = EVENT_WEIGHT[l] * spread;
                        for k in 0..nf {
                            self.scores[2 * k + groove] += w * self.past[k * size + at];
                        }
                    }
                }
            }
            for k in 0..nf {
                let score = self.scores[2 * k + groove] * weight;
                let second = nf + k;
                if score > self.best_score[k] {
                    self.best_score[second] = self.best_score[k];
                    self.best_phase[second] = self.best_phase[k];
                    self.best_tempo[second] = self.best_tempo[k];
                    self.best_groove[second] = self.best_groove[k];
                    self.best_score[k] = score;
                    self.best_phase[k] = i;
                    self.best_tempo[k] = which;
                    self.best_groove[k] = groove;
                } else if score > self.best_score[second] {
                    self.best_score[second] = score;
                    self.best_phase[second] = i;
                    self.best_tempo[second] = which;
                    self.best_groove[second] = groove;
                }
            }
        }
    }

    fn decide(&mut self) {
        let nf = self.numfeatures;
        let (mut best, mut best_sum) = (0, 0);
        for i in 0..nf {
            let second = self.best_score[nf + i];
            let excess = if second != 0.0 {
                self.best_score[i] / second
            } else {
                self.best_score[i]
            };
            let tempo = self.best_tempo[i] as i32;
            let mut sum = 0;
            for j in 0..nf {
                if j != i && (self.best_tempo[j] as i32 - tempo).abs() < 5 {
                    sum += 1;
                }
                if (self.best_tempo[2 * nf + j] as i32 - tempo).abs() < 5 {
                    sum += 1;
                }
            }
            if second != 0.0 {
                sum += excess as i32;
            }
            if sum > best_sum {
                best = i;
                best_sum = sum;
            }
        }
        let span = self.block * (2 * TEMPI) as f32;
        let at = self.best_phase[best] as f32 * self.accuracy;
        // The phase of the beat the match found, at the end of the search:
        // the time since its last beat over its own period.
        let period = period_of(self.best_tempo[best]);
        let phase = ((at + span) / period) % 1.0;
        let off = (phase - self.predict_phase).abs();
        if off.min(1.0 - off) < 2.0 * self.accuracy / self.predict_period
            && (period - self.predict_period).abs() < 0.04
        {
            self.period = period;
            self.phase = phase;
            self.groove = self.best_groove[best] as i32;
            self.tempo = 1.0 / self.period;
            self.rate = self.tempo;
        }
        self.predict_period = period;
        self.predict_phase = ((at + span + CALC_PERIOD) / period) % 1.0;
    }

    /// The block's step: the features in, a match started or carried on.
    fn step(&mut self, ctx: &ProcessCtx, bus: usize) {
        self.counter = (self.counter + 1) % self.size;
        for k in 0..self.numfeatures {
            self.past[k * self.size + self.counter] = ctx.buses.control.get(bus + k);
        }
        self.schedule += self.block;
        if self.schedule > CALC_PERIOD {
            self.schedule -= CALC_PERIOD;
            let nf = self.numfeatures;
            for i in 0..2 {
                let (to, from) = ((2 + i) * nf, i * nf);
                for j in 0..nf {
                    self.best_score[to + j] = self.best_score[from + j];
                    self.best_score[from + j] = -9999.0;
                    self.best_phase[to + j] = self.best_phase[from + j];
                    self.best_phase[from + j] = 0;
                    self.best_tempo[to + j] = self.best_tempo[from + j];
                    self.best_tempo[from + j] = 60;
                }
            }
            self.calculating = true;
            self.count = 0;
            self.start = self.counter;
        }
        if self.calculating {
            if self.count < 2 * TEMPI {
                self.template(ctx, self.count >> 1, self.count % 2);
                self.count += 1;
            } else {
                self.decide();
                self.calculating = false;
            }
        }
    }
}

impl UGen for BeatTrack2 {
    fn reset(&mut self) {
        self.past.fill(0.0);
        self.counter = 0;
        self.start = 0;
        self.schedule = 0.0;
        self.period = 0.5;
        self.groove = 0;
        self.tempo = 2.0;
        self.phase = 0.0;
        self.rate = 2.0;
        self.out_phase = 0.0;
        self.out_tempo = 2.0;
        self.out_groove = 0.0;
        self.out_rate = 2.0;
        self.predict_phase = 0.4;
        self.predict_period = 0.3;
        self.best_score.fill(-9999.0);
        self.best_phase.fill(0);
        self.best_tempo.fill(60);
        self.best_groove.fill(0);
        self.calculating = false;
        self.count = 0;
        self.half_trig = false;
        self.q1_trig = false;
        self.q2_trig = false;
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let read = |k: usize| inputs.get(k).map_or(0.0, |i| at(i, 0));
        if ctx.offset == 0 {
            let bus = (read(0) + 0.001).max(0.0) as usize;
            self.step(ctx, bus);
        }
        let dt = ctx.frames as f32 / self.sr;
        self.phase += self.rate * dt;
        if read(4) < 0.5 {
            self.out_phase = self.phase;
            self.out_tempo = self.tempo;
            self.out_groove = self.groove as f32;
            self.out_rate = self.rate;
        } else {
            self.out_phase += self.out_rate * dt;
        }
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let phase_out = self.out_phase;
        let triggers = beat_triggers(
            &mut self.out_phase,
            &mut self.half_trig,
            &mut self.q1_trig,
            &mut self.q2_trig,
            self.out_groove * 0.07,
        );
        output.fill(match self.chan {
            3 => self.out_tempo,
            4 => phase_out,
            5 => self.out_groove,
            c => triggers[c],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_models_peak_where_scsynths_tables_do() {
        let peak = (0..LAGS)
            .max_by(|&a, &b| rayleigh(a).total_cmp(&rayleigh(b)))
            .unwrap();
        assert_eq!(peak, 42);
        assert!((rayleigh(0) - 0.000_540_69).abs() < 1e-8);
        assert!((context(0.0) - 0.201_968_26).abs() < 1e-7);
        assert!((context(1.0) - 0.177_675_93).abs() < 1e-7);
    }

    #[test]
    fn the_triggers_fire_once_each() {
        let (mut p, mut h, mut a, mut b) = (0.0, false, false, false);
        let mut counts = [0; 3];
        for _ in 0..400 {
            p += 0.01;
            let t = beat_triggers(&mut p, &mut h, &mut a, &mut b, 0.0);
            for k in 0..3 {
                counts[k] += t[k] as i32;
            }
        }
        assert_eq!(counts, [3, 7, 15]);
    }
}
