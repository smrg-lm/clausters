//! The live FFT spectrum (spectroscope): per-frame analysis state and drawing.
//!
//! A `spectrum` widget runs one forward FFT per animation tick over the newest
//! window of a server audio tap and draws the magnitude curve. Like the
//! oscilloscope, the *signal* work is pure and shared by both fronts: the tick
//! feeds a [`SpectrumState`] the raw tap window and the render draws the stored
//! curve, so the browser is pixel-faithful. The whole per-frame analysis — the
//! Hann window, the FFT and the normalized decibel curve — comes from
//! `clausters_core::spectrum` (the same code the spectrogram and a client
//! drawing its own curve read), so every spectrum in the system agrees bin for
//! bin; only what plotting needs is computed here.
//!
//! The analysis keeps two per-bin traces across frames — an exponential average
//! (raw per-frame FFTs flicker) and an optional decaying peak-hold — because
//! both are stateful and cheap to carry between ticks. The drawing maps them to
//! the screen through a linear/log/mel/bark frequency axis (the [`FreqScale`]
//! geometry the spectrogram and its rulers share) with one curve point per
//! pixel column (never finer than the screen).

use clausters_core::spectrum as analysis;
use clausters_core::window::Window;

use crate::spectrogram::FreqScale;

use super::controls::body_rect;
use super::font;
use super::layout::Rect;
use super::meters::fraction;
use super::paint::{Color, Draw, Mesh};
use super::ruler;

/// The dB reference the magnitudes are floored at internally: the core's, so
/// the analysis agrees with the spectrogram before the display dB window is
/// applied. The lowest bin at 20 Hz on the log axis.
const REF_FLOOR: f32 = analysis::REF_FLOOR;
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
    /// This tick's raw curve in dB per bin, before the smoothing below.
    frame_db: Vec<f32>,
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
        let mut hann = vec![0.0f32; fft_size];
        Window::Hann.fill(&mut hann);
        // Coherent gain (matching the spectrogram) so a full-scale sine reads
        // ~0 dB regardless of window size.
        let win_gain = analysis::coherent_gain(&hann);
        Self {
            fft_size,
            hann,
            win_gain,
            windowed: vec![0.0; fft_size],
            frame_db: vec![0.0; n_bins],
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
        if !analysis::magnitudes_db_into(
            raw,
            &self.hann,
            self.win_gain,
            &mut self.windowed,
            &mut self.frame_db,
        ) {
            return;
        }
        let a = averaging.clamp(0.0, 0.99);
        for b in 0..self.frame_db.len() {
            let db = self.frame_db[b];
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
    pub fft_size: usize,
    pub db_floor: f32,
    pub db_ceil: f32,
    pub freq_scale: FreqScale,
    pub peak_hold: bool,
    pub ruler: bool,
    pub ruler_y: bool,
    /// The visible slice of the frequency display axis, normalized (`0, 1` =
    /// the whole axis): a navigable spectrum's own x window
    /// ([`EditorProps::x_view`](super::widget::EditorProps::x_view)).
    pub x_view: (f64, f64),
    pub label: Option<&'a str>,
}

/// The three regions of a spectrum's rectangle: the field the curve is drawn
/// in, the x-position the dB strip starts at (when it has one) and the
/// frequency strip under the body (when it has one).
///
/// Resolved here, once, because the gesture side needs the same body the
/// renderer drew through: a zoom anchored anywhere else than where the reader
/// sees the frequency would be anchored at the wrong hertz.
pub(crate) struct SpectrumRegions {
    pub body: Rect,
    pub strip_y_x: Option<f32>,
    pub strip_x: Option<Rect>,
}

/// **The absolute frequency geometry a spectrum's axis is placed by**: Nyquist,
/// and the normalized floor the log axis starts at. One function, because the
/// curve, the ruler and the gesture must not disagree about where a hertz sits;
/// an unknown rate falls back to 48 kHz so the axis is still drawable.
pub(crate) fn axis_geometry(sample_rate: f64) -> (f64, f64) {
    let sr = if sample_rate > 0.0 {
        sample_rate
    } else {
        FALLBACK_SR
    };
    let nyquist = sr * 0.5;
    let f_lo = (F_LO_HZ as f64).min(nyquist * 0.5).max(1.0);
    (nyquist, (f_lo / nyquist).clamp(1e-5, 0.5))
}

/// The fewest analysis bins a navigable frequency axis will show across its
/// whole body. Below this the curve stops being a measurement and becomes the
/// interpolation between two neighbouring bins — a straight line that no longer
/// answers to the signal, which is what zooming past the analysis buys.
const MIN_VISIBLE_BINS: f64 = 4.0;

/// **The narrowest window a frequency axis may be zoomed to**, in display
/// coordinates, at a window starting at `start`.
///
/// A display axis has no natural floor of its own — the normalized `Axis` uses
/// a fraction of its extent, which is a number about the *screen* — but a
/// spectrum's axis is over a measured domain, and that domain has a resolution:
/// one FFT bin, `sample_rate / fft_size` hertz wide. So the floor is the
/// display width of [`MIN_VISIBLE_BINS`] of them, measured through the very
/// mapping the curve and the ruler are drawn with. It is not a constant because
/// a bin is not one on a log (or mel, or bark) axis: at 500 Hz it is a
/// twentieth of the visible axis, near Nyquist a thousandth, so a fixed floor
/// is both far too coarse at the top and far too fine at the bottom.
pub(crate) fn min_display_span(
    fft_size: usize,
    sample_rate: f64,
    scale: FreqScale,
    f_lo_norm: f64,
    start: f64,
) -> f64 {
    let nyquist = sample_rate * 0.5;
    if nyquist <= 0.0 || fft_size == 0 {
        return crate::viewport::MIN_SPAN;
    }
    let bin_hz = sample_rate / fft_size as f64;
    let lo = ruler::display_to_hz(start.clamp(0.0, 1.0), nyquist, scale, f_lo_norm);
    let hi = (lo + MIN_VISIBLE_BINS * bin_hz).min(nyquist);
    let span = ruler::hz_to_display(hi, nyquist, scale, f_lo_norm) - start;
    // A window pressed against the top of the axis has nowhere forward to
    // measure; the smallest span anywhere on the axis is the one at Nyquist,
    // so fall back to measuring the last bins backwards from there.
    if span > 0.0 {
        span.min(1.0)
    } else {
        let back = (nyquist - MIN_VISIBLE_BINS * bin_hz).max(bin_hz);
        (1.0 - ruler::hz_to_display(back, nyquist, scale, f_lo_norm)).clamp(1e-9, 1.0)
    }
}

/// Splits a spectrum's rectangle into [`SpectrumRegions`], reserving a strip
/// per ruler that is on and fits.
pub(crate) fn regions(
    rect: Rect,
    label: bool,
    ruler: bool,
    ruler_y: bool,
    db_window: (f32, f32),
    m: &super::metrics::Metrics,
) -> SpectrumRegions {
    let mut body = body_rect(rect, label, m);
    // The x strip takes height and the y strip takes width, so the two are
    // independent - but the height comes first, since it is what decides how
    // finely the dB axis steps and therefore how wide its labels are.
    let takes_x = ruler && body.h > m.ruler_h * 2.0;
    let lane_h = if takes_x { body.h - m.ruler_h } else { body.h };
    // The pair as the ticks are drawn from it, not as the *trace* reads it
    // (which floors the span at a decibel): a strip has to hold the labels the
    // ruler will actually put in it.
    let (db_floor, db_ceil) = db_window;
    let want_w = ruler::value_strip_w(db_floor as f64, db_ceil as f64, lane_h, m);
    let strip_y_x = (ruler_y && body.w > want_w * 2.0).then(|| {
        let x = body.x;
        body.x += want_w;
        body.w -= want_w;
        x
    });
    let strip_x = takes_x.then(|| {
        body.h -= m.ruler_h;
        Rect::new(body.x, body.y + body.h, body.w, m.ruler_h)
    });
    SpectrumRegions {
        body,
        strip_y_x,
        strip_x,
    }
}

/// Draws a spectrum: a framed field with one smoothed magnitude polyline per
/// channel state (color-coded when there is more than one; a fainter peak
/// trace over each when `peak_hold`), one point per pixel column mapped
/// through the `freq_scale` frequency axis and the `[db_floor, db_ceil]`
/// vertical window. `ruler` is the x strip in hertz on the active scale
/// (shared with the spectrogram's rulers), `ruler_y` the dB strip.
/// `sample_rate` places the frequency axis (48 kHz assumed when unknown).
pub(crate) fn draw_spectrum(
    d: &mut Draw,
    rect: Rect,
    states: &[SpectrumState],
    p: &SpectrumParams,
) {
    let (mesh, m, theme) = d.parts();
    if let Some(text) = p.label {
        font::text(
            mesh,
            text,
            rect.x + m.pad,
            rect.y + m.pad,
            m.text_scale,
            theme.text,
        );
    }
    let SpectrumRegions {
        body,
        strip_y_x: strip_x,
        strip_x: x_strip,
    } = regions(
        rect,
        p.label.is_some(),
        p.ruler,
        p.ruler_y,
        (p.db_floor, p.db_ceil),
        m,
    );
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, theme.field);
    mesh.border(body, m.divider_w, theme.accent);
    // The FFT size and active scale, named over the view (the scope's
    // lock/free corner): log/mel/bark are not tellable apart from the tick
    // spacing at a glance. The size pads to 4 digits so the text never moves.
    let tag = format!("{:>4} {}", p.fft_size, ruler::scale_tag(p.freq_scale));
    super::meters::value_text(&mut Draw::new(mesh, m, theme), &tag, body);

    let (nyquist, f_lo_norm) = axis_geometry(p.sample_rate);
    let (nyquist, sr) = (nyquist as f32, nyquist as f32 * 2.0);
    let (x0, x_len) = p.x_view;
    if let Some(strip) = x_strip {
        let ticks = ruler::hz_ticks_h(
            nyquist as f64,
            p.freq_scale,
            f_lo_norm,
            strip.w as f64,
            x0,
            x_len,
            m,
        );
        ruler::draw_ticks_h(&mut Draw::new(mesh, m, theme), strip, &ticks);
    }
    if let Some(strip_x) = strip_x {
        let ticks = ruler::value_ticks(p.db_floor as f64, p.db_ceil as f64, body.h as f64, m);
        ruler::draw_ticks_v(
            &mut Draw::new(mesh, m, theme),
            body.x,
            strip_x,
            body,
            &ticks,
        );
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
        // geometry shared with the spectrogram and its rulers — the column's
        // position across the *visible window* of the axis, which is the one
        // remapping a navigable frequency axis costs the drawing.
        let bin_at = |c: usize| -> f32 {
            let frac = if columns <= 1 {
                0.0
            } else {
                c as f64 / (columns - 1) as f64
            };
            let d = x0 + frac * x_len;
            let hz = ruler::display_to_hz(d, nyquist as f64, p.freq_scale, f_lo_norm) as f32;
            (hz * state.fft_size as f32 / sr).clamp(0.0, (n_bins - 1) as f32)
        };
        let color = if states.len() > 1 {
            theme.series(ch)
        } else {
            theme.trace
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
                m.divider_w,
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
            m.trace_w,
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

    /// The dB strip is as wide as its own labels: the default `[-90, 0]`
    /// window labels whole decibels the role already holds, so the body starts
    /// exactly where it always did; a window zoomed onto a fraction of a
    /// decibel formats decimals and the strip asks for the room to draw them.
    #[test]
    fn the_db_strip_follows_its_window() {
        let m = super::super::metrics::Metrics::default();
        let rect = Rect::new(0.0, 0.0, 400.0, 300.0);
        let plain = regions(rect, false, true, true, (-90.0, 0.0), &m);
        let bare = regions(rect, false, true, false, (-90.0, 0.0), &m);
        assert_eq!(
            plain.body.x - bare.body.x,
            m.ruler_w,
            "an ordinary dB axis reserves the role and nothing more"
        );

        let zoomed = regions(rect, false, true, true, (-0.0625, 0.0625), &m);
        assert!(
            zoomed.body.x > plain.body.x,
            "the strip stayed at the role for labels that do not fit"
        );
        let ticks = super::super::ruler::value_ticks(-0.0625, 0.0625, zoomed.body.h as f64, &m);
        assert_eq!(
            zoomed.body.x - bare.body.x,
            super::super::ruler::ticks_width(&ticks, &m),
            "and by exactly what its widest label needs"
        );
        // The x strip below is unaffected in height and follows the new body.
        assert_eq!(zoomed.strip_x.unwrap().h, m.ruler_h);
        assert_eq!(zoomed.strip_x.unwrap().x, zoomed.body.x);
    }
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
