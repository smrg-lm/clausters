//! Onset detection over an `FFT` chain: `Onsets` (Dan Stowell's OnsetsDS,
//! which scsynth embeds) and the two older detectors, `PV_HainsworthFoote`
//! and `PV_JensenAndersen`.
//!
//! Each is a chain sink: it reads each fresh frame, keeps what it needs of
//! the last ones, and leaves the frame as it was. The departures from
//! scsynth's code are named where they are made.

use std::f32::consts::{PI, TAU};

use crate::dsp::features::Geometry;
use crate::dsp::registry::UGenConfig;
use crate::dsp::spectral::SpectralChain;
use crate::dsp::{ProcessCtx, UGen, at};

/// Which onset detection function an [`Onsets`] computes: scsynth's
/// `odftype`, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Odf {
    /// The frame's power.
    Power,
    /// The sum of its magnitudes.
    MagSum,
    /// Each bin's distance from the one its last two frames predict.
    Complex,
    /// [`Complex`](Odf::Complex) over the bins that rose.
    RComplex,
    /// Each bin's phase deviation from its prediction.
    Phase,
    /// [`Phase`](Odf::Phase) weighted by the bin's magnitude.
    WPhase,
    /// The modified Kullback-Leibler distance from the last frame.
    Mkl,
}

impl Odf {
    fn from_index(i: f32) -> Odf {
        match i as i32 {
            0 => Odf::Power,
            1 => Odf::MagSum,
            2 => Odf::Complex,
            4 => Odf::Phase,
            5 => Odf::WPhase,
            6 => Odf::Mkl,
            _ => Odf::RComplex,
        }
    }

    /// OnsetsDS's per-type constants: the magnitude a bin must pass to count
    /// (its `odfparam`) and the factor that brings the function to about 0
    /// to 1 for a transform of `n` points.
    fn constants(self, n: usize) -> (f32, f32) {
        let nf = n as f32;
        let bins = (n / 2 + 1) as f32;
        match self {
            Odf::Power => (0.01, 2560.0 / (bins * nf)),
            Odf::MagSum => (0.01, 113.137_09 / (bins * nf.sqrt())),
            Odf::Complex | Odf::RComplex => (0.01, 231.704_75 / nf.powf(1.5)),
            Odf::Phase => (0.01, 5.12 / nf),
            Odf::WPhase => (0.0001, 115.852_38 / nf.powf(1.5)),
            Odf::Mkl => (0.01, 7.68 * 0.25 / nf),
        }
    }
}

/// A phase brought into `(-pi, pi)`, OnsetsDS's rewrap.
fn rewrap(phase: f32) -> f32 {
    if phase > -PI && phase < PI {
        phase
    } else {
        phase + TAU * (1.0 + ((-PI - phase) / TAU).floor())
    }
}

/// `Onsets(chain, threshold, odftype, relaxtime, floor, mingap, medianspan,
/// whtype, rawodf)`: 1 on the frames an onset is found in, held to the next
/// frame, by OnsetsDS. Each frame's magnitudes are whitened (`whtype` 1:
/// each bin divided by its recent peak, which falls by 60 dB in
/// `relaxtime` seconds, and by at least `floor`); an onset detection
/// function of the whitened frame (`odftype`, [`Odf`]) has the median of
/// its last `medianspan` values taken off, and an onset is where that
/// crosses `threshold` upwards, at least `mingap` frames after the last
/// one. `rawodf > 0` outputs the detection function instead. `odftype` and
/// `medianspan` are read when the synth is built.
///
/// scsynth turns `relaxtime` into frames at a hop of half the frame; here
/// at the chain's own hop. Its complex-domain distance between a bin and
/// its prediction leaves the 2 out of the law of cosines, so a steady
/// partial reads its own magnitude rather than 0; here it is the distance.
/// Its magnitudes and phases come through its polar tables; here they are
/// exact.
pub struct Onsets {
    odf: Odf,
    half: usize,
    /// The hop over the rate, for the whitening's relaxation.
    hop_seconds: f32,
    odfparam: f32,
    normfactor: f32,
    /// The whitening's peak trace: DC, the bins, Nyquist.
    peaks: Vec<f32>,
    /// The frame in polar form: DC, the bins' magnitudes, Nyquist (signed
    /// reals, as OnsetsDS keeps them), and the bins' phases.
    mags: Vec<f32>,
    phases: Vec<f32>,
    /// What the detection functions keep of the last frames, per bin.
    prev_mag: Vec<f32>,
    prev_phase: Vec<f32>,
    prev_dphase: Vec<f32>,
    /// The detection function's last `medianspan` values, newest first, and
    /// scratch to take their median.
    odfvals: Vec<f32>,
    sorted: Vec<f32>,
    post: f32,
    gapleft: i32,
    value: f32,
}

impl Onsets {
    pub fn new(config: &UGenConfig, sr: f32) -> Self {
        let g = Geometry::new(config, sr);
        let odf = Odf::from_index(config.constant(2, 3.0));
        let medspan = (config.constant(6, 11.0) as i32).max(1) as usize;
        let (odfparam, normfactor) = odf.constants(g.n);
        let bins = g.half + 1;
        Self {
            odf,
            half: g.half,
            hop_seconds: g.hop as f32 / sr,
            odfparam,
            normfactor,
            peaks: vec![0.0; bins],
            mags: vec![0.0; bins],
            phases: vec![0.0; bins],
            prev_mag: vec![0.0; bins],
            prev_phase: vec![0.0; bins],
            prev_dphase: vec![0.0; bins],
            odfvals: vec![0.0; medspan],
            sorted: vec![0.0; medspan],
            post: 0.0,
            gapleft: 0,
            value: 0.0,
        }
    }

    /// The detection function of the whitened frame now in `mags`/`phases`.
    fn odf(&mut self) -> f32 {
        let bins = 1..self.half;
        let (dc, nyq) = (self.mags[0], self.mags[self.half]);
        match self.odf {
            Odf::Power => {
                dc * dc + nyq * nyq + bins.map(|k| self.mags[k] * self.mags[k]).sum::<f32>()
            }
            Odf::MagSum => dc.abs() + nyq.abs() + bins.map(|k| self.mags[k].abs()).sum::<f32>(),
            Odf::Complex | Odf::RComplex => {
                let rectify = self.odf == Odf::RComplex;
                let mut total = 0.0f64;
                for k in bins.clone() {
                    let cur = self.mags[k].abs();
                    let predicted = self.prev_mag[k];
                    if cur > self.odfparam && (!rectify || cur >= predicted) {
                        let phase = self.prev_phase[k] + self.prev_dphase[k] - self.phases[k];
                        let d2 = predicted * predicted + cur * cur
                            - 2.0 * predicted * cur * rewrap(phase).cos();
                        total += d2.max(0.0).sqrt() as f64;
                    }
                }
                for k in bins {
                    self.prev_mag[k] = self.mags[k].abs();
                    self.prev_dphase[k] = rewrap(self.phases[k] - self.prev_phase[k]);
                    self.prev_phase[k] = self.phases[k];
                }
                total as f32
            }
            Odf::Phase | Odf::WPhase => {
                let weighted = self.odf == Odf::WPhase;
                let mut total = 0.0f64;
                for k in bins.clone() {
                    let mag = self.mags[k].abs();
                    if mag > self.odfparam {
                        let d = rewrap(self.phases[k] - self.prev_phase[k] - self.prev_dphase[k]);
                        total += if weighted { (d * mag).abs() } else { d.abs() } as f64;
                    }
                }
                for k in bins {
                    self.prev_dphase[k] = rewrap(self.phases[k] - self.prev_phase[k]);
                    self.prev_phase[k] = self.phases[k];
                }
                total as f32
            }
            Odf::Mkl => {
                let mut total = 0.0f64;
                for k in bins {
                    let cur = self.mags[k].abs();
                    total += (1.0 + cur / (self.prev_mag[k].abs() + self.odfparam)).ln() as f64;
                    self.prev_mag[k] = cur;
                }
                total as f32
            }
        }
    }
}

impl UGen for Onsets {
    fn reset(&mut self) {
        for v in [
            &mut self.peaks,
            &mut self.mags,
            &mut self.phases,
            &mut self.prev_mag,
            &mut self.prev_phase,
            &mut self.prev_dphase,
            &mut self.odfvals,
        ] {
            v.fill(0.0);
        }
        self.post = 0.0;
        self.gapleft = 0;
        self.value = 0.0;
    }

    fn process(&mut self, _ctx: &mut ProcessCtx, _inputs: &[&[f32]], output: &mut [f32]) {
        output.fill(self.value);
    }

    fn process_spectral(
        &mut self,
        ctx: &mut ProcessCtx,
        inputs: &[&[f32]],
        output: &mut [f32],
        chain: &mut SpectralChain,
    ) {
        if chain.ready {
            let read = |k: usize, d: f32| inputs.get(k).map_or(d, |i| at(i, 0));
            let thresh = read(1, 0.5);
            let relaxtime = read(3, 1.0);
            let floor = read(4, 0.1);
            let mingap = read(5, 10.0) as i32;
            let whiten = read(7, 1.0) as i32 != 0;
            let rawodf = read(8, 0.0) > 0.0;
            let half = self.half;
            let frame = &chain.frame;
            self.mags[0] = frame[0];
            self.mags[half] = frame[1];
            for k in 1..half {
                let (re, im) = (frame[2 * k], frame[2 * k + 1]);
                self.mags[k] = re.hypot(im);
                self.phases[k] = im.atan2(re);
            }
            if whiten {
                let relax = if relaxtime == 0.0 {
                    0.0
                } else {
                    (0.1f32.ln() * self.hop_seconds / relaxtime).exp()
                };
                for (m, p) in self.mags.iter_mut().zip(self.peaks.iter_mut()) {
                    let mut v = m.abs();
                    if v < *p {
                        v += (*p - v) * relax;
                    }
                    *p = v;
                    *m /= floor.max(v);
                }
            }
            let span = self.odfvals.len();
            self.odfvals.copy_within(0..span - 1, 1);
            self.odfvals[0] = self.odf() * self.normfactor;
            let prev = self.post;
            self.sorted.copy_from_slice(&self.odfvals);
            self.sorted.sort_unstable_by(f32::total_cmp);
            let median = if span % 2 == 1 {
                self.sorted[(span - 1) / 2]
            } else {
                (self.sorted[span / 2] + self.sorted[span / 2 - 1]) * 0.5
            };
            self.post = self.odfvals[0] - median;
            let detected = if self.gapleft != 0 {
                self.gapleft -= 1;
                false
            } else {
                let d = self.post > thresh && prev <= thresh;
                if d {
                    self.gapleft = mingap;
                }
                d
            };
            self.value = if rawodf {
                self.odfvals[0]
            } else if detected {
                1.0
            } else {
                0.0
            };
        }
        let _ = ctx;
        output.fill(self.value);
    }
}

/// Which feature combination a [`PvOnset`] detects with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnsetKind {
    /// `PV_HainsworthFoote`: Hainsworth's rise in log magnitude between 30 Hz
    /// and 5 kHz, and Foote's spectral dissimilarity.
    HainsworthFoote,
    /// `PV_JensenAndersen`: the changes in spectral centroid, high-frequency
    /// energy, high-frequency content and spectral flux.
    JensenAndersen,
}

/// `PV_HainsworthFoote(chain, proph, propf, threshold, waittime)` and
/// `PV_JensenAndersen(chain, propsc, prophfe, prophfc, propsf, threshold,
/// waittime)`: an audio-rate 1 over the slice of a frame whose weighted sum
/// of features passes `threshold`, then nothing for `waittime` seconds.
/// The features are scsynth's: for `PV_HainsworthFoote`, the mean of the
/// positive `log2` rises of each bin's magnitude between 30 Hz and 5 kHz,
/// and one minus the cosine similarity of the frame's magnitudes with the
/// last frame's; for `PV_JensenAndersen`, the frame-to-frame changes of the
/// spectral centroid, the energy above 4 kHz, the high-frequency content
/// (magnitudes weighted by the bin squared) and the spectral flux, each
/// normalized by the number of bins.
///
/// scsynth places its bands at `f / sr * (n / 2 - 1)` bins, half the bin of
/// the frequency it names, and an operator precedence makes
/// `PV_JensenAndersen`'s 4 kHz bin 0 (its high-frequency energy is every
/// bin's) and `PV_HainsworthFoote`'s 30 Hz bin 0 up to about 85 kHz; here
/// each band is at its frequency. Its centroid of a silent frame is not a
/// number, which keeps the next frame from detecting; here it is 0.
pub struct PvOnset {
    kind: OnsetKind,
    geometry: Geometry,
    /// The bins of Hainsworth's band, `low..high`.
    band: (usize, usize),
    /// The first bin above 4 kHz.
    high: usize,
    prev: Vec<f32>,
    prev_norm: f32,
    /// The last frame's Jensen-Andersen features.
    features: [f32; 4],
    waiting: bool,
    wait_done: usize,
    wait_len: usize,
}

impl PvOnset {
    pub fn new(config: &UGenConfig, sr: f32, kind: OnsetKind) -> Self {
        let g = Geometry::new(config, sr);
        let last = g.half - 1;
        let at = |hz: f64| (g.bin(hz).ceil() as usize).clamp(1, last + 1);
        Self {
            kind,
            geometry: g,
            band: (at(30.0), at(5000.0)),
            high: (g.bin(4000.0).floor() as usize + 1).max(1),
            prev: vec![0.0; g.half],
            prev_norm: 1.0,
            features: [0.0; 4],
            waiting: false,
            wait_done: 0,
            wait_len: 0,
        }
    }

    /// The frame's weighted feature sum; keeps its magnitudes for the next.
    fn measure(&mut self, frame: &[f32], weight: impl Fn(usize) -> f32) -> f32 {
        let half = self.geometry.half;
        let numbins = half - 1;
        let mag = |k: usize| frame[2 * k].hypot(frame[2 * k + 1]);
        let sum = match self.kind {
            OnsetKind::HainsworthFoote => {
                let (low, high) = self.band;
                let (mut rise, mut norm, mut dot) = (0.0f32, 0.0f32, 0.0f32);
                for k in 1..half {
                    let (m, q) = (mag(k), self.prev[k]);
                    if k >= low && k < high {
                        let d = (m / q.max(0.0001)).log2();
                        if d > 0.0 {
                            rise += d;
                        }
                    }
                    norm += m * m;
                    dot += m * q;
                }
                let count = high.saturating_sub(low);
                let rise = if count == 0 { 0.0 } else { rise / count as f32 };
                let div = (norm.sqrt() * self.prev_norm.sqrt()).max(0.0001);
                let foote = 1.0 - dot / div;
                self.prev_norm = norm;
                weight(1) * rise + weight(2) * foote
            }
            OnsetKind::JensenAndersen => {
                let (mut s, mut sk, mut skk, mut flux, mut hfe) = (0.0f32, 0.0, 0.0, 0.0, 0.0);
                for k in 1..half {
                    let m = mag(k);
                    let kf = k as f32;
                    s += m;
                    sk += kf * m;
                    skk += kf * kf * m;
                    flux += (m - self.prev[k]).abs();
                    if k >= self.high {
                        hfe += m;
                    }
                }
                let unit = 1.0 / numbins as f32;
                let now = [
                    if s > 0.0 { sk / s * unit } else { 0.0 },
                    hfe * unit,
                    skk * unit * unit * unit,
                    flux * unit,
                ];
                let mut sum = 0.0;
                for (i, (&n, p)) in now.iter().zip(self.features.iter_mut()).enumerate() {
                    sum += weight(i + 1) * (n - *p);
                    *p = n;
                }
                sum
            }
        };
        for k in 1..half {
            self.prev[k] = mag(k);
        }
        sum
    }
}

impl UGen for PvOnset {
    fn reset(&mut self) {
        self.prev.fill(0.0);
        self.prev_norm = 1.0;
        self.features = [0.0; 4];
        self.waiting = false;
        self.wait_done = 0;
        self.wait_len = 0;
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
        let frames = ctx.frames;
        if self.waiting {
            self.wait_done += frames;
            if self.wait_done >= self.wait_len {
                self.waiting = false;
            }
        }
        let mut out = 0.0;
        if chain.ready {
            let read = |k: usize| inputs.get(k).map_or(0.0, |i| at(i, 0));
            let (thresh_at, wait_at) = match self.kind {
                OnsetKind::HainsworthFoote => (3, 4),
                OnsetKind::JensenAndersen => (5, 6),
            };
            let sum = self.measure(&chain.frame, read);
            if sum > read(thresh_at) && !self.waiting {
                out = 1.0;
                self.waiting = true;
                self.wait_done = frames;
                self.wait_len = (read(wait_at) * self.geometry.sr).max(0.0) as usize;
            }
        }
        output.fill(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrap_lands_in_the_open_interval() {
        for p in [-10.0f32, -4.0, -3.0, 0.0, 3.0, 4.0, 10.0] {
            let r = rewrap(p);
            assert!(r > -PI - 1e-5 && r < PI + 1e-5, "{p} -> {r}");
            assert!(((p - r) / TAU - ((p - r) / TAU).round()).abs() < 1e-4);
        }
    }
}
