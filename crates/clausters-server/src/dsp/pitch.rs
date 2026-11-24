//! `Pitch`: scsynth's autocorrelation pitch follower.

use crate::dsp::registry::UGenConfig;
use crate::dsp::{BLOCK_SIZE, ProcessCtx, UGen, at};

/// The longest median filter `Pitch` keeps.
const MAX_MEDIAN: usize = 32;

/// `Pitch(in, initFreq, minFreq, maxFreq, execFreq, maxBinsPerOctave, median,
/// ampThreshold, peakThreshold, downSample, clar, chan)`: the frequency of
/// `in` (`chan` 0) and whether it has one (`chan` 1), at control rate.
///
/// The input, taken every `downSample` samples, fills a window two of the
/// longest periods long, and `execFreq` times a second (no oftener than a
/// block) the follower runs over it: if any of its first period's samples
/// reaches `ampThreshold`, the autocorrelation is searched, from where it
/// first falls below `peakThreshold` of its zero-lag value, for the first
/// peak above that, at every lag up to `maxBinsPerOctave` lags an octave and
/// more sparsely beyond (then walked to the true peak). The peak's lag,
/// refined by a parabola through its neighbours, is the period; a frequency
/// between `minFreq` and `maxFreq` is taken, through a median of the last
/// `median` of them. Output 1 is 1 when the last run found one (or, with
/// `clar > 0`, the peak over the zero-lag value, its clarity) and 0 when it
/// did not; output 0 keeps the last frequency found, `initFreq` before the
/// first. Every argument but `in` is read when the synth is built, and a
/// control-rate `in` is taken once a block.
///
/// Each output is a row of its own (`chan`), and both rows of a call follow
/// the same input to the same result.
///
/// scsynth's walk to the true peak can step to lag 0 and read the sample
/// before the window, or past its longest lag and read the sample after it;
/// here a neighbour outside the lags searched is 0, as scsynth already takes
/// the neighbour of a peak at either end.
pub struct Pitch {
    chan: usize,
    /// Whether `in` is an audio signal (else one value a block).
    audio: bool,
    init_freq: f32,
    min_freq: f32,
    max_freq: f32,
    srate: f32,
    min_period: usize,
    max_period: usize,
    exec_period: usize,
    max_log2_bins: u32,
    median_size: usize,
    amp_thresh: f32,
    peak_thresh: f32,
    downsample: usize,
    clarity: bool,
    buffer: Vec<f32>,
    index: usize,
    readp: usize,
    freq: f32,
    has_freq: f32,
    values: [f32; MAX_MEDIAN],
    ages: [usize; MAX_MEDIAN],
}

/// `ceil(log2(n))`, scsynth's `LOG2CEIL`.
fn log2_ceil(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

impl Pitch {
    pub fn new(config: &UGenConfig, sr: f32, audio_input: bool) -> Self {
        let c = |k: usize, d: f32| config.constant(k, d);
        let init_freq = c(1, 440.0);
        let min_freq = c(2, 60.0).max(1e-3);
        let max_freq = c(3, 4000.0).max(min_freq);
        let exec_freq = c(4, 100.0).clamp(min_freq, max_freq);
        let max_bins = (c(5, 16.0) as i32).max(1) as usize;
        let median_size = (c(6, 1.0) as i32).clamp(0, MAX_MEDIAN as i32) as usize;
        let downsample = c(9, 1.0) as i32;
        let (downsample, srate) = if audio_input {
            let d = downsample.clamp(1, BLOCK_SIZE as i32) as usize;
            (d, sr / d as f32)
        } else {
            let d = downsample.max(1) as usize;
            (d, sr / (BLOCK_SIZE * d) as f32)
        };
        let min_period = (srate / max_freq) as usize;
        let max_period = ((srate / min_freq) as usize).max(1);
        let exec_period = ((srate / exec_freq) as usize).max(BLOCK_SIZE);
        let size = (max_period * 2).max(exec_period);
        let mut p = Self {
            chan: (c(11, 0.0).max(0.0) as usize).min(1),
            audio: audio_input,
            init_freq,
            min_freq,
            max_freq,
            srate,
            min_period,
            max_period,
            exec_period,
            max_log2_bins: log2_ceil(max_bins),
            median_size,
            amp_thresh: c(7, 0.01),
            peak_thresh: c(8, 0.5),
            downsample,
            clarity: c(10, 0.0) > 0.0,
            buffer: vec![0.0; size],
            index: 0,
            readp: 0,
            freq: init_freq,
            has_freq: 0.0,
            values: [0.0; MAX_MEDIAN],
            ages: [0; MAX_MEDIAN],
        };
        p.reset();
        p
    }

    /// The autocorrelation at `lag` over the window's first period, or 0 for
    /// a lag outside the ones searched.
    fn lag_sum(&self, lag: i64) -> f32 {
        if lag < 0 || lag as usize > self.max_period {
            return 0.0;
        }
        let lag = lag as usize;
        let mp = self.max_period;
        let b = &self.buffer;
        let mut sum = 0.0f32;
        for j in 0..mp {
            sum += b[lag + j] * b[j];
        }
        sum
    }

    /// The sparser search step past `maxBinsPerOctave` lags an octave.
    fn step_at(&self, lag: usize) -> usize {
        let octave = log2_ceil(lag);
        if octave <= self.max_log2_bins {
            1
        } else {
            1 << (octave - self.max_log2_bins)
        }
    }

    fn insert_median(&mut self, value: f32) -> f32 {
        let size = self.median_size;
        let last = size - 1;
        let mut pos = 0;
        for (i, age) in self.ages[..size].iter_mut().enumerate() {
            if *age == last {
                pos = i;
            } else {
                *age += 1;
            }
        }
        while pos != 0 && value < self.values[pos - 1] {
            self.values[pos] = self.values[pos - 1];
            self.ages[pos] = self.ages[pos - 1];
            pos -= 1;
        }
        while pos != last && value > self.values[pos + 1] {
            self.values[pos] = self.values[pos + 1];
            self.ages[pos] = self.ages[pos + 1];
            pos += 1;
        }
        self.values[pos] = value;
        self.ages[pos] = 0;
        self.values[size >> 1]
    }

    /// One run of the follower over the full window.
    fn analyse(&mut self) {
        self.has_freq = 0.0;
        let mp = self.max_period;
        let loud = self.buffer[..mp].iter().any(|s| s.abs() >= self.amp_thresh);
        if loud {
            let zero = self.lag_sum(0);
            let threshold = zero * self.peak_thresh;
            // Past the zero-lag lobe: the first lag below the threshold.
            let mut i = 1;
            while i <= mp {
                if self.lag_sum(i as i64) < threshold {
                    break;
                }
                i += self.step_at(i);
            }
            // The first peak above the threshold.
            let mut max_sum = threshold;
            let mut found = false;
            let mut period = i;
            while i <= mp {
                if i >= self.min_period {
                    let s = self.lag_sum(i as i64);
                    if s > threshold {
                        if s > max_sum {
                            found = true;
                            max_sum = s;
                            period = i;
                        }
                    } else if found {
                        break;
                    }
                }
                i += self.step_at(i);
            }
            if found {
                let mut prev = if period > 0 {
                    self.lag_sum(period as i64 - 1)
                } else {
                    0.0
                };
                let mut next = if period < mp {
                    self.lag_sum(period as i64 + 1)
                } else {
                    0.0
                };
                // Not on the peak yet when the search stepped past it.
                while prev > max_sum && period > 0 {
                    next = max_sum;
                    max_sum = prev;
                    period -= 1;
                    prev = self.lag_sum(period as i64 - 1);
                }
                while next > max_sum && period < mp {
                    prev = max_sum;
                    max_sum = next;
                    period += 1;
                    next = self.lag_sum(period as i64 + 1);
                }
                let beta = 0.5 * (next - prev);
                let gamma = 2.0 * max_sum - next - prev;
                let fperiod = period as f32 + beta / gamma;
                let freq = self.srate / fperiod;
                if freq >= self.min_freq && freq <= self.max_freq {
                    self.freq = if self.median_size > 1 {
                        self.insert_median(freq)
                    } else {
                        freq
                    };
                    self.has_freq = if self.clarity { max_sum / zero } else { 1.0 };
                }
            }
        }
        // Keep the window's tail for the next run.
        let interval = self.buffer.len() - self.exec_period;
        self.buffer.copy_within(self.exec_period.., 0);
        self.index = interval;
    }

    fn push(&mut self, z: f32) {
        self.buffer[self.index] = z;
        self.index += 1;
        if self.index >= self.buffer.len() {
            self.analyse();
        }
    }
}

impl UGen for Pitch {
    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.index = 0;
        self.readp = 0;
        self.freq = self.init_freq;
        self.has_freq = 0.0;
        for i in 0..MAX_MEDIAN {
            self.values[i] = self.init_freq;
            self.ages[i] = i;
        }
    }

    fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
        let input = inputs[0];
        if self.audio {
            // An audio input: every `downsample`th sample, across slices.
            let n = input.len();
            while self.readp < n {
                self.push(input[self.readp]);
                self.readp += self.downsample;
            }
            self.readp -= n;
        } else if ctx.offset == 0 {
            // A control input: one sample a block, every `downsample`th.
            self.readp += 1;
            if self.readp >= self.downsample {
                self.readp = 0;
                self.push(at(input, 0));
            }
        }
        output.fill(if self.chan == 0 {
            self.freq
        } else {
            self.has_freq
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log2_ceil_is_scsynths() {
        assert_eq!(
            [1, 2, 3, 4, 5, 16, 17].map(log2_ceil),
            [0, 1, 2, 2, 3, 4, 5]
        );
    }
}
