//! The measures scsynth reads off an `FFT` chain: `SpecCentroid`,
//! `SpecFlatness`, `SpecPcile`, `Loudness`, `MFCC` and `KeyTrack`.
//!
//! Each is a chain **sink**: it reads the frame on the slices a fresh one is
//! ready, leaves it as it was, and holds its value between frames, as
//! scsynth's hold theirs. The chain's geometry -- its transform size, frame,
//! hop and window -- reaches each one through its configuration, so the
//! tables a measure needs are built for that geometry and the engine's
//! rate, on the network thread.
//!
//! scsynth carries those tables precomputed for one transform at 44.1 or 48
//! kHz (`Loudness` and `MFCC` a 1024-point one, `KeyTrack` a 4096-point
//! one), takes any other rate for 48 kHz, and reads them at the same bins
//! whatever the chain. Here each table is computed from the formula that
//! generated it -- every one reproduces scsynth's at its own transform and
//! rate -- so the measure follows the chain. The departures from scsynth's
//! code are named where they are made.

use clausters_core::window::Window;

use crate::dsp::registry::UGenConfig;
use crate::dsp::spectral::{
    SpectralChain, bin_mag, get_bin, resolve_fft_size, resolve_frame, resolve_hop,
};
use crate::dsp::{ProcessCtx, UGen, at};

/// The geometry of the chain a sink reads, from its configuration (the
/// compiler copies the chain's into it).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Geometry {
    /// The transform size.
    pub n: usize,
    /// `n / 2`: the Nyquist bin.
    pub half: usize,
    /// The frame windowed, `<= n`.
    pub frame: usize,
    /// The hop, in samples.
    pub hop: usize,
    /// The engine's rate, in hertz.
    pub sr: f32,
}

impl Geometry {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        let n = resolve_fft_size(config.fft_size);
        let frame = resolve_frame(n, config.winsize);
        Self {
            n,
            half: n / 2,
            frame,
            hop: resolve_hop(frame, config.hop),
            sr,
        }
    }

    /// The frequency of bin `k`, in hertz.
    pub fn hz(&self, k: f32) -> f32 {
        k * self.sr / self.n as f32
    }

    /// The bin a frequency falls at, fractional.
    pub fn bin(&self, hz: f64) -> f64 {
        hz * self.n as f64 / self.sr as f64
    }

    /// The time between two frames, in seconds.
    pub fn frame_period(&self) -> f32 {
        self.hop as f32 / self.sr
    }
}

/// The sum of a window's coefficients over `len` samples: a sinusoid's
/// magnitude in a frame is its amplitude times half of it.
fn window_sum(window: Window, len: usize) -> f64 {
    let mut w = vec![0.0; len];
    window.fill(&mut w);
    w.iter().map(|&x| x as f64).sum()
}

/// The factor that brings a frame's **powers** to the level scsynth
/// calibrated `Loudness` and `MFCC` at: a 1024-point frame under its default
/// window, the sine. A sinusoid's power in a frame grows with the square of
/// the window's sum, so a frame of another size or window is scaled by the
/// square of the ratio; at scsynth's own frame it is 1. One per window, for
/// the window the chain carries at each frame (an `FFT` can change it).
struct PowerScale {
    by_window: [f32; 6],
}

impl PowerScale {
    fn new(frame: usize) -> Self {
        let reference = window_sum(Window::Sine, 1024);
        let mut by_window = [1.0; 6];
        for (k, s) in by_window.iter_mut().enumerate() {
            let sum = window_sum(Window::from_wintype(k as i32 - 1), frame);
            *s = ((reference / sum) * (reference / sum)) as f32;
        }
        Self { by_window }
    }

    fn of(&self, window: Window) -> f32 {
        self.by_window[(window.wintype() + 1) as usize]
    }
}

/// `SpecCentroid(chain)`: the spectrum's centre of mass in hertz, every bin's
/// frequency weighted by its magnitude. Silence reads 0.
///
/// scsynth leaves DC out of the denominator, which moves the centroid of a
/// signal with a DC component up; here every bin counts in both sums.
pub struct SpecCentroid {
    geometry: Geometry,
    value: f32,
}

impl SpecCentroid {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        Self {
            geometry: Geometry::new(config, sr),
            value: 0.0,
        }
    }
}

impl UGen for SpecCentroid {
    fn reset(&mut self) {
        self.value = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(self.value);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        _inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let half = self.geometry.half;
            let (mut num, mut den) = (0.0f64, 0.0f64);
            for k in 0..=half {
                let m = bin_mag(&chain.frame, k, half) as f64;
                num += m * k as f64;
                den += m;
            }
            self.value = if den == 0.0 {
                0.0
            } else {
                self.geometry.hz((num / den) as f32)
            };
        }
        output.fill(self.value);
    }
}

/// `SpecFlatness(chain)`: the geometric mean of the magnitudes over their
/// arithmetic mean, 1 for white noise and towards 0 for a few partials.
/// A bin of magnitude 0 is left out of the logarithms (its logarithm is not
/// finite) and counted in the mean, as scsynth counts it. Silence reads
/// 0.8, scsynth's figure for very quiet white noise.
///
/// scsynth takes the logarithm of DC's and Nyquist's magnitudes whatever
/// they are, so a frame with either at exactly 0 reads 0; here they follow
/// the rule of every other bin.
pub struct SpecFlatness {
    half: usize,
    value: f32,
}

impl SpecFlatness {
    pub fn new(config: &UGenConfig) -> Self {
        Self {
            half: resolve_fft_size(config.fft_size) / 2,
            value: 0.0,
        }
    }
}

impl UGen for SpecFlatness {
    fn reset(&mut self) {
        self.value = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(self.value);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        _inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let half = self.half;
            let (mut logs, mut sum) = (0.0f64, 0.0f64);
            for k in 0..=half {
                let m = bin_mag(&chain.frame, k, half);
                if m != 0.0 {
                    logs += (m as f64).ln();
                    sum += m as f64;
                }
            }
            let count = (half + 1) as f64;
            let mean = sum / count;
            self.value = if mean == 0.0 {
                0.8
            } else {
                ((logs / count).exp() / mean) as f32
            };
        }
        output.fill(self.value);
    }
}

/// `SpecPcile(chain, fraction, interpolate, binout)`: the frequency below
/// which `fraction` of the spectrum's magnitude lies (0.5 the median, 0.95
/// a rolloff), in hertz, or as a bin number when `binout > 0`.
/// `interpolate > 0` places it between the two bins the cumulative sum
/// crosses the target between. A target at or below DC's share reads 0.
///
/// scsynth's sum runs from DC to Nyquist but its search only over the bins
/// between them, so a target in DC's share reads the first bin and one in
/// Nyquist's reads 0 Hz; it converts a bin to hertz at `sr / (n + 2)` rather
/// than `sr / n`, and its bin output is one bin low and interpolates the
/// wrong way. Here the search covers every bin, a bin is `sr / n` hertz,
/// and the two outputs are one position in two units.
pub struct SpecPcile {
    geometry: Geometry,
    value: f32,
}

impl SpecPcile {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        Self {
            geometry: Geometry::new(config, sr),
            value: 0.0,
        }
    }
}

impl UGen for SpecPcile {
    fn reset(&mut self) {
        self.value = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(self.value);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let read = |k: usize, d: f32| inputs.get(k).map_or(d, |i| at(i, 0));
            let (fraction, interpolate, binout) =
                (read(1, 0.5), read(2, 0.0) > 0.0, read(3, 0.0) > 0.0);
            let half = self.geometry.half;
            let frame = &chain.frame;
            let total: f32 = (0..=half).map(|k| bin_mag(frame, k, half)).sum();
            let target = total * fraction;
            let mut position = 0.0;
            let mut below = 0.0f32;
            for k in 0..=half {
                let cumul = below + bin_mag(frame, k, half);
                if cumul >= target {
                    position = if interpolate && k > 0 && cumul > below {
                        k as f32 - (cumul - target) / (cumul - below)
                    } else {
                        k as f32
                    };
                    break;
                }
                below = cumul;
            }
            self.value = if binout {
                position
            } else {
                self.geometry.hz(position)
            };
        }
        output.fill(self.value);
    }
}

/// The band edges of scsynth's loudness model, in bins of a 1024-point
/// transform at 44.1 kHz: band `k` is `EDGES[k] .. EDGES[k] + SIZES[k]`.
const LOUDNESS_EDGES: [u16; 42] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 13, 15, 17, 19, 22, 25, 28, 32, 36, 41, 46, 52, 58, 65, 73, 82,
    92, 103, 116, 129, 144, 161, 180, 201, 225, 251, 280, 312, 348, 388, 433, 483,
];
const LOUDNESS_SIZES: [u16; 42] = [
    1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 8, 9, 10, 11, 13, 13, 15,
    17, 19, 21, 24, 26, 29, 32, 36, 40, 45, 50, 29,
];

/// The equal-loudness contours of scsynth's model: band `k`'s level in dB
/// at each of [`PHONS`].
#[rustfmt::skip]
const CONTOURS: [[f32; 11]; 42] = [
    [47.88, 59.68, 68.55, 75.48, 81.71, 87.54, 93.24, 98.84, 104.44, 109.94, 115.31],
    [29.04, 41.78, 51.98, 60.18, 67.51, 74.54, 81.34, 87.97, 94.61, 101.21, 107.74],
    [20.72, 32.83, 43.44, 52.18, 60.24, 67.89, 75.34, 82.70, 89.97, 97.23, 104.49],
    [15.87, 27.14, 37.84, 46.94, 55.44, 63.57, 71.51, 79.34, 87.14, 94.97, 102.37],
    [12.64, 23.24, 33.91, 43.27, 52.07, 60.57, 68.87, 77.10, 85.24, 93.44, 100.90],
    [10.31, 20.43, 31.03, 40.54, 49.59, 58.33, 66.89, 75.43, 83.89, 92.34, 100.80],
    [8.51, 18.23, 28.83, 38.41, 47.65, 56.59, 65.42, 74.16, 82.89, 91.61, 100.33],
    [7.14, 16.55, 27.11, 36.79, 46.16, 55.27, 64.29, 73.24, 82.15, 91.06, 99.97],
    [5.52, 14.58, 25.07, 34.88, 44.40, 53.73, 62.95, 72.18, 81.31, 90.44, 99.57],
    [3.98, 12.69, 23.10, 32.99, 42.69, 52.27, 61.66, 71.15, 80.54, 89.93, 99.31],
    [2.99, 11.43, 21.76, 31.73, 41.49, 51.22, 60.88, 70.51, 80.11, 89.70, 99.30],
    [2.35, 10.58, 20.83, 30.86, 40.68, 50.51, 60.33, 70.08, 79.83, 89.58, 99.32],
    [2.05, 10.12, 20.27, 30.35, 40.22, 50.10, 59.97, 69.82, 79.67, 89.52, 99.38],
    [2.00, 9.93, 20.00, 30.07, 40.00, 49.93, 59.87, 69.80, 79.73, 89.67, 99.60],
    [2.19, 10.00, 20.00, 30.00, 40.00, 50.00, 59.99, 69.99, 79.98, 89.98, 99.97],
    [2.71, 10.56, 20.61, 30.71, 40.76, 50.81, 60.86, 70.96, 81.01, 91.06, 101.17],
    [3.11, 11.05, 21.19, 31.41, 41.53, 51.64, 61.75, 71.95, 82.05, 92.15, 102.33],
    [2.39, 10.69, 21.14, 31.52, 41.73, 51.95, 62.11, 72.31, 82.46, 92.56, 102.59],
    [1.50, 10.11, 20.82, 31.32, 41.62, 51.92, 62.12, 72.32, 82.52, 92.63, 102.56],
    [-0.17, 8.50, 19.27, 29.77, 40.07, 50.37, 60.57, 70.77, 80.97, 91.13, 101.23],
    [-1.80, 6.96, 17.77, 28.29, 38.61, 48.91, 59.13, 69.33, 79.53, 89.71, 99.86],
    [-3.42, 5.49, 16.36, 26.94, 37.31, 47.61, 57.88, 68.08, 78.28, 88.41, 98.39],
    [-4.73, 4.38, 15.34, 25.99, 36.39, 46.71, 57.01, 67.21, 77.41, 87.51, 97.41],
    [-5.73, 3.63, 14.74, 25.48, 35.88, 46.26, 56.56, 66.76, 76.96, 87.06, 96.96],
    [-6.24, 3.33, 14.59, 25.39, 35.84, 46.22, 56.52, 66.72, 76.92, 87.04, 97.00],
    [-6.09, 3.62, 15.03, 25.83, 36.37, 46.70, 57.00, 67.20, 77.40, 87.57, 97.68],
    [-5.32, 4.44, 15.90, 26.70, 37.28, 47.60, 57.90, 68.10, 78.30, 88.52, 98.78],
    [-3.49, 6.17, 17.52, 28.32, 38.85, 49.22, 59.52, 69.72, 79.92, 90.20, 100.61],
    [-0.81, 8.58, 19.73, 30.44, 40.90, 51.24, 61.52, 71.69, 81.87, 92.15, 102.63],
    [2.91, 11.82, 22.64, 33.17, 43.53, 53.73, 63.96, 74.09, 84.22, 94.45, 104.89],
    [6.68, 15.19, 25.71, 36.03, 46.25, 56.31, 66.45, 76.49, 86.54, 96.72, 107.15],
    [10.43, 18.65, 28.94, 39.02, 49.01, 58.98, 68.93, 78.78, 88.69, 98.83, 109.36],
    [13.56, 21.65, 31.78, 41.68, 51.45, 61.31, 71.07, 80.73, 90.48, 100.51, 111.01],
    [14.36, 22.91, 33.19, 43.09, 52.71, 62.37, 71.92, 81.38, 90.88, 100.56, 110.56],
    [15.06, 23.90, 34.23, 44.05, 53.48, 62.90, 72.21, 81.43, 90.65, 99.93, 109.34],
    [15.36, 23.90, 33.89, 43.31, 52.40, 61.42, 70.29, 79.18, 88.00, 96.69, 105.17],
    [15.60, 23.90, 33.60, 42.70, 51.50, 60.20, 68.70, 77.30, 85.80, 94.00, 101.70],
    [15.60, 23.90, 33.60, 42.70, 51.50, 60.20, 68.70, 77.30, 85.80, 94.00, 101.70],
    [15.60, 23.90, 33.60, 42.70, 51.50, 60.20, 68.70, 77.30, 85.80, 94.00, 101.70],
    [15.60, 23.90, 33.60, 42.70, 51.50, 60.20, 68.70, 77.30, 85.80, 94.00, 101.70],
    [15.60, 23.90, 33.60, 42.70, 51.50, 60.20, 68.70, 77.30, 85.80, 94.00, 101.70],
    [15.60, 23.90, 33.60, 42.70, 51.50, 60.20, 68.70, 77.30, 85.80, 94.00, 101.70],
];

/// The loudness levels the contours are given at, in phons.
const PHONS: [f32; 11] = [
    2.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0,
];

/// The multiplier scsynth's model reads a band's power at, so a full-scale
/// tone reads one sone.
const LOUDNESS_CALIBRATION: f32 = 76_032.94;

/// `Loudness(chain, smask, tmask)`: perceived loudness in sones, by scsynth's
/// model. The spectrum's power is summed in 42 bands of about an ERB each,
/// each bin masked by the one below it at `smask` of its power; each band's
/// level in dB is turned to phons through the equal-loudness contours, held
/// against its previous level minus `tmask` phons a frame, and the bands'
/// powers are summed back into a level whose sone value is
/// `2^((phons - 40) / 10)`.
///
/// scsynth reads its bands at fixed bins of the chain, right for a
/// 1024-point frame at 44.1 kHz (twice the frequencies at 88.2 kHz and 2048
/// points), and its calibration holds for that frame under its sine window.
/// Here the bands are at their frequencies whatever the transform and the
/// rate, and the power is scaled to that calibration by the window's sum
/// (`PowerScale`), so a tone reads the same loudness through any chain;
/// at 44.1 kHz, 1024 points and the sine window it is scsynth's. scsynth
/// adds a band below hearing (0 phons) to the sum as a power of 1 (its
/// `10^(0.1 * 0) - 0.001`, which undoes an offset of the dB scale on the
/// phon one), so silence reads 0.19 sones and a quiet sound is louder by the
/// bands it does not reach; here such a band adds nothing, and silence is 0.
pub struct Loudness {
    /// Each band's bins, `start..end`.
    bands: [(u32, u32); 42],
    half: usize,
    scale: PowerScale,
    /// Each band's level after temporal masking, in phons.
    levels: [f32; 42],
    value: f32,
}

impl Loudness {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        let g = Geometry::new(config, sr);
        let hz_per_ref_bin = 44100.0 / 1024.0;
        let at =
            |b: u16| (g.bin(b as f64 * hz_per_ref_bin).round() as usize).min(g.half + 1) as u32;
        let mut bands = [(0, 0); 42];
        for (k, band) in bands.iter_mut().enumerate() {
            let start = at(LOUDNESS_EDGES[k]).max(1);
            let end = at(LOUDNESS_EDGES[k] + LOUDNESS_SIZES[k]).max(start);
            *band = (start, end);
        }
        Self {
            bands,
            half: g.half,
            scale: PowerScale::new(g.frame),
            levels: [0.0; 42],
            value: 0.0,
        }
    }
}

/// A band's level in dB turned to phons through its equal-loudness contour.
fn phons(contour: &[f32; 11], db: f32) -> f32 {
    if db < contour[0] {
        return 0.0;
    }
    if db > contour[10] {
        return PHONS[10];
    }
    for j in 1..11 {
        if db < contour[j] {
            let prop = (db - contour[j - 1]) / (contour[j] - contour[j - 1]);
            return (1.0 - prop) * PHONS[j - 1] + prop * PHONS[j];
        }
    }
    PHONS[10]
}

impl UGen for Loudness {
    fn reset(&mut self) {
        self.levels = [0.0; 42];
        self.value = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(self.value);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let read = |k: usize, d: f32| inputs.get(k).map_or(d, |i| at(i, 0));
            let (smask, tmask) = (read(1, 0.25), read(2, 1.0));
            let gain = LOUDNESS_CALIBRATION * self.scale.of(chain.window);
            let mut loudsum = 0.0f64;
            for (k, &(start, end)) in self.bands.iter().enumerate() {
                let mut sum = 0.0f32;
                let mut last = 0.0f32;
                for h in start as usize..end as usize {
                    let (re, im) = get_bin(&chain.frame, h, self.half);
                    let power = (re * re + im * im).max(last * smask);
                    last = power;
                    sum += power;
                }
                let db = 10.0 * (sum * gain + 0.001).log10();
                let level = phons(&CONTOURS[k], db).max(self.levels[k] - tmask);
                self.levels[k] = level;
                // An inaudible band (0 phons) adds no loudness.
                if level > 0.0 {
                    loudsum += 10f64.powf(0.1 * level as f64);
                }
            }
            self.value = if loudsum == 0.0 {
                0.0
            } else {
                2f64.powf((10.0 * loudsum.log10() - 40.0) / 10.0) as f32
            };
        }
        output.fill(self.value);
    }
}

/// The most coefficients an `MFCC` gives: one per band.
pub const MFCC_BANDS: usize = 42;

/// `MFCC(chain, numcoeff, chan)`: coefficient `chan` (from the first, not
/// the zeroth) of the spectrum's mel-frequency cepstrum, of `numcoeff`
/// (1 to 42) the def asks for. The power is summed in 42 bands spaced
/// evenly in mels from 80 Hz to 18 kHz, each a triangle reaching its
/// neighbours' centres; each band's level is `10 log10(power) + 50` dB
/// (floored at -50 dB of power), and the coefficients are the cosine
/// transform of those levels, scaled into about 0 to 1 as scsynth scales
/// them (`0.25 * (0.01 * sum + 1)`). Each coefficient is a row of its own
/// (`chan`); a client builds `numcoeff` of them.
///
/// scsynth carries its bands as tables for a 1024-point frame at 44.1 kHz
/// and at 48 kHz (and takes any other rate for 48 kHz); here they are the
/// formula scsynth's tables were generated with, for the chain's transform
/// and the engine's rate, so they are scsynth's at its two, and the power
/// is scaled to its calibration as `Loudness`'s is.
pub struct Mfcc {
    /// Each band's first bin and how many it spans.
    bands: [(u32, u32); MFCC_BANDS],
    /// Every band's weights, one per bin it spans, one band after another.
    weights: Vec<f32>,
    /// The cosine transform's row for this coefficient.
    basis: [f32; MFCC_BANDS],
    half: usize,
    scale: PowerScale,
    value: f32,
}

/// The bin of each of an MFCC's band centres (and the edges beyond the first
/// and the last), as scsynth's generator places them: evenly spaced in mels,
/// each truncated to a bin, none past Nyquist.
fn mel_peaks(g: &Geometry) -> [usize; MFCC_BANDS + 2] {
    let to_mel = |f: f64| 1127.0 * (1.0 + f / 700.0).ln();
    let from_mel = |m: f64| 700.0 * ((m / 1127.0).exp() - 1.0);
    let (low, high) = (to_mel(80.0), to_mel(18000.0));
    let step = (high - low) / MFCC_BANDS as f64;
    let mut peaks = [0; MFCC_BANDS + 2];
    let mut mel = low;
    peaks[0] = g.bin(80.0) as usize;
    for p in peaks.iter_mut().skip(1) {
        mel += step;
        *p = (g.bin(from_mel(mel)) as usize).min(g.half);
    }
    peaks
}

impl Mfcc {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        let g = Geometry::new(config, sr);
        let peaks = mel_peaks(&g);
        let mut bands = [(0, 0); MFCC_BANDS];
        let mut weights = Vec::new();
        for (i, band) in bands.iter_mut().enumerate() {
            let start = if i == 0 { 0 } else { peaks[i - 1] + 1 };
            let (centre, end) = (peaks[i], peaks[i + 1]);
            // A rising edge to the centre and a falling one after it, each
            // a run of equal steps: `(j + 1) / (rise + 1)` up, then
            // `1 - (j + 1) / (fall + 1)` down, ending a bin before the next
            // centre.
            for j in start..end {
                weights.push(if j <= centre {
                    (j + 1 - start) as f32 / (centre + 1 - start) as f32
                } else {
                    1.0 - (j - centre) as f32 / (end - centre) as f32
                });
            }
            let span = end.saturating_sub(start);
            *band = (start as u32, span as u32);
        }
        let numcoeff = (config.constant(1, 13.0) as i32).clamp(1, MFCC_BANDS as i32) as usize;
        let chan = (config.constant(2, 0.0).max(0.0) as usize).min(numcoeff - 1);
        let mut basis = [0.0; MFCC_BANDS];
        for (j, b) in basis.iter_mut().enumerate() {
            *b = (std::f64::consts::PI / MFCC_BANDS as f64 * (j as f64 + 0.5) * (chan + 1) as f64)
                .cos() as f32;
        }
        Self {
            bands,
            weights,
            basis,
            half: g.half,
            scale: PowerScale::new(g.frame),
            value: 0.0,
        }
    }
}

impl UGen for Mfcc {
    fn reset(&mut self) {
        self.value = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(self.value);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        _inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let scale = self.scale.of(chain.window);
            let mut sum = 0.0f32;
            let mut w = 0;
            for (k, &(start, span)) in self.bands.iter().enumerate() {
                let mut power = 0.0f32;
                for j in start as usize..(start + span) as usize {
                    let (re, im) = get_bin(&chain.frame, j, self.half);
                    power += (re * re + im * im) * self.weights[w];
                    w += 1;
                }
                let level = 10.0 * ((power * scale).max(1e-5).log10() + 5.0);
                sum += self.basis[k] * level;
            }
            self.value = 0.25 * (sum * 0.01 + 1.0);
        }
        output.fill(self.value);
    }
}

/// The notes a `KeyTrack` listens for: five octaves of semitones from A1
/// (55 Hz).
const KEY_NOTES: usize = 60;
/// The partials of each note it weighs.
const KEY_PARTIALS: usize = 6;

/// A diatonic profile: each scale degree's weight in a key.
const MAJOR_PROFILE: [f32; 12] = [5.0, 0.0, 3.5, 0.0, 4.5, 4.0, 0.0, 4.5, 0.0, 3.5, 0.0, 4.0];
const MINOR_PROFILE: [f32; 12] = [5.0, 0.0, 3.5, 4.5, 0.0, 4.0, 0.0, 4.5, 3.5, 0.0, 0.0, 4.0];
const MAJOR_DEGREES: [usize; 7] = [0, 2, 4, 5, 7, 9, 11];
const MINOR_DEGREES: [usize; 7] = [0, 2, 3, 5, 7, 8, 11];

/// `KeyTrack(chain, keydecay, chromaleak)`: the key the music is in, 0 to
/// 11 the major keys from C and 12 to 23 the minor ones. Each frame the
/// power at the first six partials of 60 notes (A1 to G#6, each partial
/// shared between the two bins around it and weighted by one over its
/// number) is gathered into a chroma, which keeps `chromaleak` of the last
/// one; the chroma is matched against a major and a minor diatonic profile
/// in every key, and the matches accumulate in a histogram whose memory
/// falls to 1% over `keydecay` seconds. The key is the histogram's highest.
///
/// scsynth carries the bins and weights as tables for a 4096-point frame at
/// 44.1 and 48 kHz, and turns `keydecay` into frames at a hop of half that
/// frame. Here the table is the formula it was generated with, for the
/// chain's transform and the engine's rate (scsynth's at its two; a partial
/// past Nyquist is left out), and `keydecay` is turned into frames at the
/// chain's own hop.
pub struct KeyTrack {
    /// Per note and partial, the lower of its two bins and its weights.
    partials: Vec<(u32, f32, f32)>,
    frame_period: f32,
    half: usize,
    chroma: [f32; 12],
    histogram: [f32; 24],
    key: f32,
}

impl KeyTrack {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        let g = Geometry::new(config, sr);
        let mut partials = Vec::with_capacity(KEY_NOTES * KEY_PARTIALS);
        for note in 0..KEY_NOTES {
            let hz = 55.0 * 2f64.powf(note as f64 / 12.0);
            for h in 1..=KEY_PARTIALS {
                let x = g.bin(h as f64 * hz);
                let low = x as usize;
                let frac = x - low as f64;
                let (wl, wh) = ((1.0 - frac) / h as f64, frac / h as f64);
                if low < g.half {
                    partials.push((low as u32, wl as f32, wh as f32));
                } else {
                    partials.push((0, 0.0, 0.0));
                }
            }
        }
        Self {
            partials,
            frame_period: g.frame_period(),
            half: g.half,
            chroma: [0.0; 12],
            histogram: [0.0; 24],
            key: 0.0,
        }
    }
}

impl UGen for KeyTrack {
    fn reset(&mut self) {
        self.chroma = [0.0; 12];
        self.histogram = [0.0; 24];
        self.key = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(self.key);
    }

    fn process_spectral(
        &mut self,
        _ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let read = |k: usize, d: f32| inputs.get(k).map_or(d, |i| at(i, 0));
            let (keydecay, chromaleak) = (read(1, 2.0), read(2, 0.5));
            let power = |k: usize| {
                let (re, im) = get_bin(&chain.frame, k, self.half);
                re * re + im * im
            };
            for c in self.chroma.iter_mut() {
                *c *= chromaleak;
            }
            for note in 0..KEY_NOTES {
                let mut sum = 0.0;
                for &(low, wl, wh) in &self.partials[note * KEY_PARTIALS..(note + 1) * KEY_PARTIALS]
                {
                    if wl != 0.0 || wh != 0.0 {
                        sum += wl * power(low as usize) + wh * power(low as usize + 1);
                    }
                }
                self.chroma[(note + 9) % 12] += sum;
            }
            let frames = (keydecay / self.frame_period).max(0.001);
            let leak = 0.01f32.powf(1.0 / frames);
            let (mut best, mut best_score) = (0, 0.0);
            for (i, h) in self.histogram.iter_mut().enumerate() {
                let (profile, degrees) = if i < 12 {
                    (&MAJOR_PROFILE, &MAJOR_DEGREES)
                } else {
                    (&MINOR_PROFILE, &MINOR_DEGREES)
                };
                let tonic = i % 12;
                let fit: f32 = degrees
                    .iter()
                    .map(|&d| self.chroma[(tonic + d) % 12] * profile[d])
                    .sum();
                *h = leak * *h + fit;
                if *h > best_score {
                    best_score = *h;
                    best = i;
                }
            }
            self.key = best as f32;
        }
        output.fill(self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(n: usize) -> UGenConfig {
        UGenConfig {
            fft_size: Some(n),
            ..Default::default()
        }
    }

    /// scsynth's MFCC tables at 44.1 kHz, the first and last band: the
    /// generator reproduces them.
    #[test]
    fn the_mel_bands_are_scsynths_at_its_rates() {
        let g = Geometry::new(&config(1024), 44100.0);
        let peaks = mel_peaks(&g);
        // scsynth's `g_startbin44100` is `0, peaks[0] + 1, ...` and its
        // `g_endbin44100` one past `peaks[k + 1] - 1`.
        assert_eq!(peaks[1] - 1 + 1, 3);
        assert_eq!(peaks[MFCC_BANDS - 2] + 1, 357);
        assert_eq!(peaks[MFCC_BANDS], 417);
        let m = Mfcc::new(&config(1024), 44100.0);
        assert_eq!(m.weights.len(), 761);
        assert_eq!(m.bands[0], (0, 3));
        let g48 = Geometry::new(&config(1024), 48000.0);
        assert_eq!(mel_peaks(&g48)[MFCC_BANDS], 384);
        assert_eq!(Mfcc::new(&config(1024), 48000.0).weights.len(), 696);
    }

    /// The first note of scsynth's KeyTrack table at 44.1 kHz.
    #[test]
    fn the_key_partials_are_scsynths_at_its_rates() {
        let k = KeyTrack::new(&config(4096), 44100.0);
        let (low, wl, wh) = k.partials[0];
        assert_eq!(low, 5);
        assert!((wl - 0.891_609_97).abs() < 1e-6 && (wh - 0.108_390_03).abs() < 1e-6);
        let (low, wl, wh) = k.partials[5];
        assert_eq!(low, 30);
        assert!(
            (wl - 0.058_276_64).abs() < 1e-6
                && (wh - 0.108_390_03 / 6.0 * 6.0 / 6.0 * 1.0).abs() < 1.0
        );
    }

    /// At scsynth's own frame the loudness bands are its table.
    #[test]
    fn the_loudness_bands_are_scsynths_at_its_frame() {
        let l = Loudness::new(&config(1024), 44100.0);
        for k in 0..42 {
            assert_eq!(l.bands[k].0 as u16, LOUDNESS_EDGES[k]);
            assert_eq!(l.bands[k].1 as u16, LOUDNESS_EDGES[k] + LOUDNESS_SIZES[k]);
        }
        assert!((l.scale.of(Window::Sine) - 1.0).abs() < 1e-6);
    }
}
