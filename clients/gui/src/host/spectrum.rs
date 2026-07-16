//! The live FFT spectrum (spectroscope): per-frame analysis state and drawing.
//!
//! A `spectrum` widget runs one forward FFT per animation tick over the newest
//! window of a server audio tap and draws the magnitude curve. Like the
//! oscilloscope, the *signal* work is pure and shared by both fronts: the tick
//! feeds a [`SpectrumState`] the raw tap window and the render draws the stored
//! curve, so the browser is pixel-faithful. The FFT and the Hann window come
//! from `clausters_core` (the same code the spectrogram uses), so the two views
//! agree bin for bin; only what plotting needs is computed here.
//!
//! The analysis keeps two per-bin traces across frames — an exponential average
//! (raw per-frame FFTs flicker) and an optional decaying peak-hold — because
//! both are stateful and cheap to carry between ticks. The drawing maps them to
//! the screen through a linear/log/mel/bark frequency axis (the [`FreqScale`]
//! geometry the spectrogram and its rulers share) with one curve point per
//! pixel column (never finer than the screen).

use clausters_core::fft;

use crate::spectrogram::FreqScale;
use crate::waveform::CHANNEL_COLORS;

use super::controls::body_rect;
use super::font;
use super::frame::{RULER_H, RULER_W};
use super::layout::Rect;
use super::meters::fraction;
use super::paint::{Color, Mesh};
use super::ruler;

const TEXT: Color = [0.85, 0.87, 0.90, 1.0];
const FIELD: Color = [0.14, 0.15, 0.19, 1.0];
const FRAME: Color = [0.30, 0.78, 0.55, 1.0];
const TRACE: Color = [0.40, 0.85, 0.62, 1.0];
const PAD: f32 = 4.0;
const TEXT_SCALE: f32 = 2.0;

/// The dB reference the magnitudes are floored at internally, matching the
/// spectrogram's `REF_FLOOR`, so the analysis agrees with it before the display
/// dB window is applied. The lowest bin at 20 Hz on the log axis.
const REF_FLOOR: f32 = -120.0;
const F_LO_HZ: f32 = 20.0;
/// Sample rate assumed before the server publishes one (the browser has no
/// segment to read it from), matching the oscilloscope's fallback.
const FALLBACK_SR: f64 = 48_000.0;
/// Peak-hold decay per tick (~30 fps), so a peak fades over roughly a second.
const PEAK_DECAY_DB: f32 = 0.6;

/// Persistent per-widget spectrum state, carried across animation ticks. Holds
/// the smoothed and peak-hold dB curves (one entry per bin) the render draws,
/// plus the scratch buffers the FFT reuses so a tick never allocates.
pub struct SpectrumState {
    fft_size: usize,
    hann: Vec<f32>,
    win_gain: f32,
    windowed: Vec<f32>,
    mags: Vec<f32>,
    /// Exponentially-smoothed magnitude in dB per bin (`fft_size / 2` entries).
    pub avg_db: Vec<f32>,
    /// Decaying peak-hold in dB per bin.
    pub peak_db: Vec<f32>,
    initialized: bool,
}

impl SpectrumState {
    /// A fresh state for `fft_size` (a supported power of two), its curves at the
    /// floor so the first frames rise into view.
    pub fn new(fft_size: usize) -> Self {
        let n_bins = fft_size / 2;
        let hann: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / fft_size as f32).cos())
            .collect();
        // Coherent gain (matching the spectrogram) so a full-scale sine reads
        // ~0 dB regardless of window size.
        let win_gain = hann.iter().sum::<f32>() * 0.5;
        Self {
            fft_size,
            hann,
            win_gain,
            windowed: vec![0.0; fft_size],
            mags: vec![0.0; n_bins],
            avg_db: vec![REF_FLOOR; n_bins],
            peak_db: vec![REF_FLOOR; n_bins],
            initialized: false,
        }
    }

    /// Rebuilds for a new `fft_size` if it changed (a live `/gui_set fft_size`).
    pub fn ensure_size(&mut self, fft_size: usize) {
        if fft_size != self.fft_size {
            *self = Self::new(fft_size);
        }
    }

    /// How many raw samples one update needs (a full FFT window).
    pub fn window_len(&self) -> usize {
        self.fft_size
    }

    /// Folds one newest tap window (`raw.len() == fft_size`) into the smoothed
    /// and peak-hold curves. `averaging` in `[0, 1)` weights the previous frame
    /// (0 = instant, →1 = very smooth); `peak_hold` advances the decaying peak.
    pub fn update(&mut self, raw: &[f32], averaging: f32, peak_hold: bool) {
        for (i, w) in self.windowed.iter_mut().enumerate() {
            *w = raw.get(i).copied().unwrap_or(0.0) * self.hann[i];
        }
        if !fft::rfft_magnitudes_into(&self.windowed, &mut self.mags) {
            return;
        }
        let a = averaging.clamp(0.0, 0.99);
        for b in 0..self.mags.len() {
            let mag = self.mags[b] / self.win_gain;
            let db = (20.0 * (mag + 1e-9).log10()).max(REF_FLOOR);
            self.avg_db[b] = if self.initialized {
                a * self.avg_db[b] + (1.0 - a) * db
            } else {
                db
            };
            if peak_hold {
                self.peak_db[b] = (self.peak_db[b] - PEAK_DECAY_DB).max(self.avg_db[b]);
            } else {
                self.peak_db[b] = self.avg_db[b];
            }
        }
        self.initialized = true;
    }
}

/// The display parameters of one spectrum draw.
pub(crate) struct SpectrumParams<'a> {
    pub sample_rate: f64,
    pub db_floor: f32,
    pub db_ceil: f32,
    pub freq_scale: FreqScale,
    pub peak_hold: bool,
    pub ruler: bool,
    pub ruler_y: bool,
    pub label: Option<&'a str>,
}

/// Draws a spectrum: a framed field with one smoothed magnitude polyline per
/// channel state (color-coded when there is more than one; a fainter peak
/// trace over each when `peak_hold`), one point per pixel column mapped
/// through the `freq_scale` frequency axis and the `[db_floor, db_ceil]`
/// vertical window. `ruler` is the x strip in hertz on the active scale
/// (shared with the spectrogram's rulers), `ruler_y` the dB strip.
/// `sample_rate` places the frequency axis (48 kHz assumed when unknown).
pub(crate) fn draw_spectrum(
    mesh: &mut Mesh,
    rect: Rect,
    states: &[SpectrumState],
    p: &SpectrumParams,
) {
    if let Some(text) = p.label {
        font::text(mesh, text, rect.x + PAD, rect.y + PAD, TEXT_SCALE, TEXT);
    }
    let mut body = body_rect(rect, p.label.is_some());
    let strip_x = (p.ruler_y && body.w > RULER_W * 2.0).then(|| {
        let x = body.x;
        body.x += RULER_W;
        body.w -= RULER_W;
        x
    });
    let x_strip = (p.ruler && body.h > RULER_H * 2.0).then(|| {
        body.h -= RULER_H;
        Rect::new(body.x, body.y + body.h, body.w, RULER_H)
    });
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, FIELD);
    mesh.border(body, 1.0, FRAME);

    let sr = if p.sample_rate > 0.0 {
        p.sample_rate as f32
    } else {
        FALLBACK_SR as f32
    };
    let nyquist = sr * 0.5;
    let f_lo = F_LO_HZ.min(nyquist * 0.5).max(1.0);
    let f_lo_norm = (f_lo as f64 / nyquist as f64).clamp(1e-5, 0.5);
    if let Some(strip) = x_strip {
        let ticks = ruler::hz_ticks_h(nyquist as f64, p.freq_scale, f_lo_norm, strip.w as f64);
        ruler::draw_ticks_h(mesh, strip, &ticks);
    }
    if let Some(strip_x) = strip_x {
        let ticks = ruler::value_ticks(p.db_floor as f64, p.db_ceil as f64, body.h as f64);
        ruler::draw_ticks_v(mesh, body.x, strip_x, body, &ticks);
    }
    let columns = body.w.max(1.0) as usize;
    let db_ceil = p.db_ceil.max(p.db_floor + 1.0);
    let y_at = |db: f32| body.y + body.h * (1.0 - fraction(db, p.db_floor, db_ceil));
    for (ch, state) in states.iter().enumerate() {
        let n_bins = state.avg_db.len();
        if n_bins == 0 {
            continue;
        }
        // The bin (fractional) a screen column maps to, through the display→Hz
        // geometry shared with the spectrogram and its rulers.
        let bin_at = |c: usize| -> f32 {
            let frac = if columns <= 1 {
                0.0
            } else {
                c as f64 / (columns - 1) as f64
            };
            let hz = ruler::display_to_hz(frac, nyquist as f64, p.freq_scale, f_lo_norm) as f32;
            (hz * state.fft_size as f32 / sr).clamp(0.0, (n_bins - 1) as f32)
        };
        let color = if states.len() > 1 {
            CHANNEL_COLORS[ch % CHANNEL_COLORS.len()]
        } else {
            TRACE
        };
        if p.peak_hold {
            let faint = [color[0], color[1], color[2], 0.55];
            polyline(
                mesh,
                &body,
                columns,
                &bin_at,
                &y_at,
                &state.peak_db,
                faint,
                1.0,
            );
        }
        polyline(
            mesh,
            &body,
            columns,
            &bin_at,
            &y_at,
            &state.avg_db,
            color,
            1.5,
        );
    }
}

/// One curve of a per-bin dB array, sampled at each pixel column and drawn as a
/// polyline. Factored so the live curve, the peak-hold trace and the static
/// `plot`'s spectrum view share it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn polyline(
    mesh: &mut Mesh,
    body: &Rect,
    columns: usize,
    bin_at: &impl Fn(usize) -> f32,
    y_at: &impl Fn(f32) -> f32,
    db: &[f32],
    color: Color,
    width: f32,
) {
    let sample = |c: usize| -> [f32; 2] {
        let bf = bin_at(c);
        let b0 = bf.floor() as usize;
        let b1 = (b0 + 1).min(db.len() - 1);
        let t = bf - b0 as f32;
        let v = db[b0] * (1.0 - t) + db[b1] * t;
        [body.x + c as f32, y_at(v)]
    };
    let mut prev = sample(0);
    for c in 1..columns {
        let p = sample(c);
        mesh.line(prev, p, width, color);
        prev = p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bin whose smoothed magnitude is largest — where a pure tone peaks.
    fn peak_bin(state: &SpectrumState) -> usize {
        state
            .avg_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap()
    }

    #[test]
    fn a_sine_peaks_at_its_bin() {
        // A 1 kHz sine at 48 kHz over a 1024 FFT peaks at bin 1000*1024/48000 ~ 21.
        let fft_size = 1024;
        let sr = 48_000.0f32;
        let freq = 1000.0f32;
        let raw: Vec<f32> = (0..fft_size)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / sr).sin())
            .collect();
        let mut state = SpectrumState::new(fft_size);
        state.update(&raw, 0.0, false);
        let expected = (freq * fft_size as f32 / sr).round() as usize;
        assert!(
            (peak_bin(&state) as i32 - expected as i32).abs() <= 1,
            "peak at bin {}, expected ~{expected}",
            peak_bin(&state)
        );
        // A full-scale sine reads near 0 dB at its bin (the coherent-gain
        // normalization), well above the floor.
        assert!(state.avg_db[expected] > -6.0, "full-scale sine ~0 dB");
    }

    #[test]
    fn averaging_smooths_towards_the_new_frame() {
        let fft_size = 256;
        let mut state = SpectrumState::new(fft_size);
        let loud: Vec<f32> = (0..fft_size)
            .map(|i| (std::f32::consts::TAU * i as f32 / 8.0).sin())
            .collect();
        // First frame initializes exactly; a strong average then lags a step.
        state.update(&loud, 0.0, false);
        let bin = peak_bin(&state);
        let first = state.avg_db[bin];
        state.update(&vec![0.0; fft_size], 0.9, false);
        assert!(
            state.avg_db[bin] < first && state.avg_db[bin] > REF_FLOOR + 1.0,
            "silence pulls the average down but not instantly"
        );
    }

    #[test]
    fn peak_hold_decays_but_lags_the_live_curve() {
        let fft_size = 256;
        let mut state = SpectrumState::new(fft_size);
        let loud: Vec<f32> = (0..fft_size)
            .map(|i| (std::f32::consts::TAU * i as f32 / 8.0).sin())
            .collect();
        state.update(&loud, 0.0, true);
        let bin = peak_bin(&state);
        let held = state.peak_db[bin];
        // Silence: the live average drops, the held peak only decays a little.
        state.update(&vec![0.0; fft_size], 0.0, true);
        assert!(
            state.peak_db[bin] > state.avg_db[bin],
            "peak lags below-peak"
        );
        assert!(state.peak_db[bin] < held, "and decays");
        assert!(
            (held - state.peak_db[bin] - PEAK_DECAY_DB).abs() < 1e-3,
            "by one decay step"
        );
    }

    #[test]
    fn ensure_size_rebuilds_only_on_change() {
        let mut state = SpectrumState::new(1024);
        assert_eq!(state.avg_db.len(), 512);
        state.avg_db[0] = -3.0;
        state.ensure_size(1024);
        assert_eq!(state.avg_db[0], -3.0, "same size keeps the state");
        state.ensure_size(2048);
        assert_eq!(state.avg_db.len(), 1024, "new size rebuilds");
        assert_eq!(state.avg_db[0], REF_FLOOR);
    }
}
