//! The static signal `plot`: framed views of a sample array, with rulers and a
//! cursor readout — still the lightweight counterpart of the heavy navigable
//! `waveform`.
//!
//! Where the waveform owns a GPU pipeline and a peak pyramid for editor-grade
//! zoom/pan, the plot draws a signal once through the flat-geometry painter
//! ([`super::paint`]) — the case the catalog calls "a simple static plot of an
//! NRT-generated signal/file". It does not navigate or edit; what it adds over
//! a bare trace is measurement: adjustable x/y rulers, multichannel lanes, an
//! auto-fitted value range for arbitrary numeric sequences, and a hover
//! readout naming the exact sample (or spectral bin) under the cursor.
//!
//! The plot has **views** — [`PlotView`], an enum on purpose so future forms
//! (a histogram, a phase plot) extend it:
//!
//! - **Signal** — value against time/index. It honors the project's one
//!   graphics rule (never resolve finer than the screen) by decimating to the
//!   pixel width: a polyline when the data fits the width, a min/max envelope
//!   (one vertical bar per pixel column) when it does not — the *whole*
//!   sequence always contributes, so there is no visual aliasing.
//! - **Spectrum** — the averaged magnitude spectrum of the (short) signal:
//!   one [`crate::spectrogram::Stft`] pass per channel (the shared-core FFT
//!   and Hann window, so it agrees with the spectrogram bin for bin), frames
//!   averaged in the power domain (Welch), drawn as a dB curve over the same
//!   four frequency scales the spectrogram displays (linear/log/mel/bark,
//!   through the identical [`super::ruler::display_to_hz`] geometry).
//!
//! Everything here is pure over a [`Mesh`], so it is unit-testable without a
//! window. The analysis ([`analyze`]) is computed **once** at the widget's
//! mutation points (parse, bulk load, a live `/gui_set`), never per frame.

use crate::spectrogram::{FreqScale, Stft};

use super::controls::body_rect;
use super::font;
use super::frame::{lane_at, lane_rect};
use super::layout::Rect;
use super::meters::fraction;
use super::metrics::Metrics;
use super::paint::Mesh;
use super::ruler::{self, TimeUnit};
use super::signal::{
    self,
    trace::{Trace, TraceStyle},
};
use super::theme::{Theme, with_alpha};
use super::widget::Ruler;

/// Sample rate assumed for the spectrum axis when the source brings none,
/// matching the live views' fallback.
const FALLBACK_SR: f64 = 48_000.0;
/// The log axis floor of the spectral view, matching the spectrogram (~20 Hz).
const F_LO_HZ: f64 = 20.0;
/// The normalized-magnitude floor `Stft` maps from, in dB (its `REF_FLOOR`).
const STFT_FLOOR: f32 = -120.0;
/// Headroom added to an auto-fitted value range, as a fraction of the span.
const AUTO_MARGIN: f32 = 0.04;

/// How a `plot` presents its samples. An enum on purpose: future views extend
/// it without touching the widget's wiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlotView {
    /// Value against time/index (the classic trace).
    #[default]
    Signal,
    /// The averaged magnitude spectrum of the whole signal, in dB.
    Spectrum,
}

impl PlotView {
    /// Parses the wire form (`"signal"`/`"spectrum"`).
    pub fn parse(s: &str) -> Option<PlotView> {
        Some(match s {
            "signal" => PlotView::Signal,
            "spectrum" => PlotView::Spectrum,
            _ => return None,
        })
    }
}

/// The one-shot analysis behind a plot's spectrum view: per channel, the
/// Welch-averaged magnitude curve in dB (one entry per bin, coherent-gain
/// normalized so a full-scale sine reads ~0 dB at its bin).
#[derive(Debug, Clone, PartialEq)]
pub struct PlotSpectrum {
    /// One dB curve per channel, `fft_size / 2` entries each.
    pub curves: Vec<Vec<f32>>,
    /// The analysis FFT size.
    pub fft_size: usize,
    /// Top of the frequency axis, Hz.
    pub nyquist: f64,
}

/// Averages the magnitude spectrum of interleaved `samples` (`channels`-way),
/// one curve per channel: an [`Stft`] per channel (the shared-core FFT — the
/// same analysis the spectrogram draws), its frames averaged in the **power**
/// domain and expressed in dB. `sample_rate <= 0` assumes 48 kHz, like the
/// live views.
pub fn analyze(
    samples: &[f32],
    channels: usize,
    fft_size: usize,
    sample_rate: f64,
) -> PlotSpectrum {
    let channels = channels.max(1);
    let sr = if sample_rate > 0.0 {
        sample_rate
    } else {
        FALLBACK_SR
    };
    let frames = samples.len() / channels;
    let n_bins = fft_size / 2;
    let mut curves = Vec::with_capacity(channels);
    let mut chan = vec![0.0f32; frames];
    for ch in 0..channels {
        for (f, slot) in chan.iter_mut().enumerate() {
            *slot = samples[f * channels + ch];
        }
        let stft = Stft::compute(&chan, fft_size, fft_size / 2, sr as f32);
        let mags = stft.magnitudes();
        let n_frames = stft.n_frames().max(1);
        let mut curve = vec![0.0f32; n_bins];
        for (b, out) in curve.iter_mut().enumerate() {
            let mut power = 0.0f64;
            for f in 0..n_frames {
                // Invert the Stft's normalized [0, 1] form back to dB, then
                // average in the power domain (Welch).
                let db = mags[f * n_bins + b] * -STFT_FLOOR + STFT_FLOOR;
                power += 10f64.powf(db as f64 / 10.0);
            }
            *out = (10.0 * (power / n_frames as f64).log10()) as f32;
        }
        curves.push(curve);
    }
    PlotSpectrum {
        curves,
        fft_size,
        nyquist: sr * 0.5,
    }
}

/// Everything one plot draw needs, view-independent — the widget's parsed
/// props plus its (possibly bulk-loaded) samples and cached analysis.
pub struct PlotParams<'a> {
    /// Interleaved samples (`channels`-way frames).
    pub samples: &'a [f32],
    pub channels: usize,
    pub view: PlotView,
    /// Overlaid per-color traces instead of stacked lanes.
    pub overlay: bool,
    /// 0 = unknown (the x axis then reads in sample/index counts).
    pub sample_rate: f64,
    /// Explicit value range; `None` auto-fits to the data (per side).
    pub min: Option<f32>,
    pub max: Option<f32>,
    /// The x ruler mode (`Beats` is not meaningful here and reads as samples).
    pub ruler: Ruler,
    /// Whether the y (value) ruler strip is drawn.
    pub ruler_y: bool,
    /// The cached spectral analysis (spectrum view only).
    pub spectrum: Option<&'a PlotSpectrum>,
    pub db_floor: f32,
    pub db_ceil: f32,
    pub freq_scale: FreqScale,
    pub label: Option<&'a str>,
}

/// The plot's inner geometry: the traced `body` after the label strip and the
/// ruler strips are carved out of `rect`, plus where those strips sit.
struct Geom {
    body: Rect,
    /// x of the y-ruler strip (the widget edge), when the strip is on.
    strip_x: Option<f32>,
    /// The x-ruler strip under the body, when the ruler is on.
    x_strip: Option<Rect>,
    lanes: usize,
}

fn geometry(rect: Rect, p: &PlotParams, m: &Metrics) -> Geom {
    let mut body = body_rect(rect, p.label.is_some(), m);
    let strip_x = (p.ruler_y && body.w > m.ruler_w * 2.0).then(|| {
        let x = body.x;
        body.x += m.ruler_w;
        body.w -= m.ruler_w;
        x
    });
    let x_strip = (p.ruler != Ruler::Off && body.h > m.ruler_h * 2.0).then(|| {
        body.h -= m.ruler_h;
        Rect::new(body.x, body.y + body.h, body.w, m.ruler_h)
    });
    let lanes = if p.overlay {
        1
    } else {
        p.channels.max(1).min(frames_of(p).max(1))
    };
    Geom {
        body,
        strip_x,
        x_strip,
        lanes,
    }
}

/// Frame count of the interleaved samples (a trailing partial frame ignored).
fn frames_of(p: &PlotParams) -> usize {
    p.samples.len() / p.channels.max(1)
}

/// The signal view's value range: each side explicit when given, auto-fitted
/// to the data otherwise (with a little headroom), and always non-degenerate.
pub(crate) fn value_range(p: &PlotParams) -> (f32, f32) {
    let (mut dlo, mut dhi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &v in p.samples {
        if v.is_finite() {
            dlo = dlo.min(v);
            dhi = dhi.max(v);
        }
    }
    if !dlo.is_finite() || !dhi.is_finite() {
        (dlo, dhi) = (-1.0, 1.0);
    }
    let margin = ((dhi - dlo) * AUTO_MARGIN).max(f32::MIN_POSITIVE);
    let lo = p.min.unwrap_or(dlo - margin);
    let hi = p.max.unwrap_or(dhi + margin);
    if hi > lo {
        (lo, hi)
    } else {
        (lo - 1.0, lo + 1.0)
    }
}

/// The x-axis time unit: sample counts unless the ruler asks for clock time
/// and a rate is known (`Beats` has no grid here and reads as samples).
fn x_unit(p: &PlotParams) -> TimeUnit {
    if p.ruler == Ruler::Time && p.sample_rate > 0.0 {
        TimeUnit::Seconds
    } else {
        TimeUnit::Samples
    }
}

/// Draws a plot into `mesh`: the label strip, the framed field, the rulers and
/// the view's traces (stacked per-channel lanes, or overlaid when asked).
pub fn draw(mesh: &mut Mesh, rect: Rect, p: &PlotParams, m: &Metrics, theme: &Theme) {
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
    let g = geometry(rect, p, m);
    if g.body.w <= 0.0 || g.body.h <= 0.0 {
        return;
    }
    mesh.rect(g.body, theme.track);
    mesh.border(g.body, m.divider_w, theme.frame_plot);
    for lane in 1..g.lanes {
        let r = lane_rect(g.body, g.lanes, lane);
        mesh.rect(Rect::new(r.x, r.y, r.w, m.divider_w), theme.lane_divider);
    }
    match p.view {
        PlotView::Signal => draw_signal(mesh, &g, p, m, theme),
        PlotView::Spectrum => draw_spectrum(mesh, &g, p, m, theme),
    }
}

fn draw_signal(mesh: &mut Mesh, g: &Geom, p: &PlotParams, m: &Metrics, theme: &Theme) {
    let channels = p.channels.max(1);
    let n = frames_of(p);
    let (lo, hi) = value_range(p);
    if let Some(strip) = g.x_strip {
        let ticks = ruler::time_ticks(0.0, n as f64, strip.w as f64, p.sample_rate, x_unit(p), m);
        ruler::draw_ticks_h(mesh, strip, &ticks, m, theme);
    }
    for ch in 0..channels {
        let lane = lane_rect(g.body, g.lanes, if p.overlay { 0 } else { ch });
        if ch == 0 || !p.overlay {
            if let Some(strip_x) = g.strip_x {
                let ticks = ruler::value_ticks(lo as f64, hi as f64, lane.h as f64, m);
                ruler::draw_ticks_v(mesh, g.body.x, strip_x, lane, &ticks, m, theme);
            }
            // A zero baseline, when 0 is within the displayed range.
            if lo < 0.0 && hi > 0.0 {
                let y = lane.y + lane.h * (1.0 - fraction(0.0, lo, hi));
                mesh.line(
                    [lane.x, y],
                    [lane.x + lane.w, y],
                    m.divider_w,
                    theme.baseline,
                );
            }
        }
        if n < 2 {
            continue;
        }
        // The whole sequence over the lane's width, through the one column
        // source every signal view reads: a polyline while samples are wider
        // than a couple of pixels, the min/max envelope once they are not.
        let span = (n - 1) as f64;
        signal::trace::draw_channel(
            mesh,
            lane,
            &Trace::samples(p.samples, channels),
            ch,
            |x| (x - lane.x) as f64 / lane.w.max(1.0) as f64 * span,
            |s| lane.x + (s / span) as f32 * lane.w,
            |v| lane.y + lane.h * (1.0 - fraction(v, lo, hi)),
            TraceStyle {
                color: theme.series(ch),
                width: m.trace_w,
            },
        );
    }
}

fn draw_spectrum(mesh: &mut Mesh, g: &Geom, p: &PlotParams, m: &Metrics, theme: &Theme) {
    let Some(spec) = p.spectrum else {
        return;
    };
    // The FFT size and active scale, named over the view (the live views'
    // corner slot); the size pads to 4 digits so the text never moves.
    let tag = format!("{:>4} {}", spec.fft_size, ruler::scale_tag(p.freq_scale));
    super::meters::value_text(mesh, &tag, g.body, m, theme);
    let nyquist = spec.nyquist.max(1.0);
    let f_lo = (F_LO_HZ / nyquist).clamp(1e-5, 0.5);
    if let Some(strip) = g.x_strip {
        let ticks = ruler::hz_ticks_h(nyquist, p.freq_scale, f_lo, strip.w as f64, m);
        ruler::draw_ticks_h(mesh, strip, &ticks, m, theme);
    }
    let (dlo, dhi) = (p.db_floor, p.db_ceil.max(p.db_floor + 1.0));
    for (ch, curve) in spec.curves.iter().enumerate() {
        if curve.is_empty() {
            continue;
        }
        let lane = lane_rect(g.body, g.lanes, if p.overlay { 0 } else { ch % g.lanes });
        if (ch == 0 || !p.overlay)
            && let Some(strip_x) = g.strip_x
        {
            let ticks = ruler::value_ticks(dlo as f64, dhi as f64, lane.h as f64, m);
            ruler::draw_ticks_v(mesh, g.body.x, strip_x, lane, &ticks, m, theme);
        }
        let color = theme.series(ch);
        let columns = lane.w.max(1.0) as usize;
        let bin_at = |c: usize| bin_at_column(c, columns, spec, p.freq_scale, f_lo);
        let y_at = |db: f32| lane.y + lane.h * (1.0 - fraction(db, dlo, dhi));
        super::spectrum::polyline(
            mesh, &lane, columns, &bin_at, &y_at, curve, color, m.trace_w,
        );
    }
}

/// The (fractional) bin a spectrum-view pixel column maps to, through the
/// display→Hz geometry shared with the spectrogram and its rulers.
fn bin_at_column(
    c: usize,
    columns: usize,
    spec: &PlotSpectrum,
    scale: FreqScale,
    f_lo: f64,
) -> f32 {
    let frac = if columns <= 1 {
        0.0
    } else {
        c as f64 / (columns - 1) as f64
    };
    let hz = ruler::display_to_hz(frac, spec.nyquist, scale, f_lo);
    let n_bins = spec.fft_size / 2;
    ((hz * spec.fft_size as f64 / (spec.nyquist * 2.0)) as f32).clamp(0.0, (n_bins - 1) as f32)
}

/// Draws the hover readout into the overlay mesh: a hairline at the cursor, a
/// marker dot on the trace under it, and the x/y values named in the body's
/// bottom-right corner — the exact sample (index/time and value) on the signal
/// view, the bin (frequency per the scale, level in dB) on the spectrum view.
pub fn draw_readout(
    over: &mut Mesh,
    rect: Rect,
    p: &PlotParams,
    cursor: (f64, f64),
    m: &Metrics,
    theme: &Theme,
) {
    let g = geometry(rect, p, m);
    let (cx, cy) = cursor;
    if g.body.w <= 0.0 || g.body.h <= 0.0 || !g.body.contains(cx, cy) {
        return;
    }
    let frac = ((cx - g.body.x as f64) / g.body.w.max(1.0) as f64).clamp(0.0, 1.0);
    let lane_i = lane_at(g.body, g.lanes, cy);
    let lane = lane_rect(g.body, g.lanes, lane_i);
    let text = match p.view {
        PlotView::Signal => {
            let channels = p.channels.max(1);
            let n = frames_of(p);
            if n == 0 {
                return;
            }
            let i = ((frac * (n - 1) as f64).round() as usize).min(n - 1);
            let ch = channel_under_cursor(p, &g, lane_i, i, cy);
            let v = p.samples[i * channels + ch];
            let (lo, hi) = value_range(p);
            let x = lane.x + lane.w * (i as f64 / (n - 1).max(1) as f64) as f32;
            let y = lane.y + lane.h * (1.0 - fraction(v, lo, hi));
            hairline_and_dot(over, lane, x, y, theme);
            let pos = match x_unit(p) {
                TimeUnit::Seconds => {
                    let secs_per_px = (n - 1) as f64 / p.sample_rate / g.body.w.max(1.0) as f64;
                    ruler::readout_time(i as f64, p.sample_rate, secs_per_px)
                }
                _ => ruler::readout_samples(i as f64),
            };
            let value = ruler::readout_value(v as f64, (hi - lo) as f64);
            if channels > 1 {
                format!("{pos}  CH{ch} {value}")
            } else {
                format!("{pos}  {value}")
            }
        }
        PlotView::Spectrum => {
            let Some(spec) = p.spectrum else {
                return;
            };
            let nyquist = spec.nyquist.max(1.0);
            let f_lo = (F_LO_HZ / nyquist).clamp(1e-5, 0.5);
            let hz = ruler::display_to_hz(frac, nyquist, p.freq_scale, f_lo);
            let ch = if p.overlay {
                0
            } else {
                lane_i.min(spec.curves.len().saturating_sub(1))
            };
            let curve = match spec.curves.get(ch) {
                Some(c) if !c.is_empty() => c,
                _ => return,
            };
            let bin = ((hz * spec.fft_size as f64 / (nyquist * 2.0)).round() as usize)
                .min(curve.len() - 1);
            let db = curve[bin];
            let (dlo, dhi) = (p.db_floor, p.db_ceil.max(p.db_floor + 1.0));
            let y = lane.y + lane.h * (1.0 - fraction(db, dlo, dhi));
            hairline_and_dot(over, lane, cx as f32, y, theme);
            let tag = if spec.curves.len() > 1 {
                format!("CH{ch} ")
            } else {
                String::new()
            };
            format!("{} HZ  {tag}{db:.1} DB", hz.round() as i64)
        }
    };
    let w = font::width(&text, m.caption_scale);
    let x = (g.body.x + g.body.w - w - m.pad).max(g.body.x);
    let y = g.body.y + g.body.h - font::height(m.caption_scale) - 3.0;
    font::text(
        over,
        &text,
        x,
        y.max(g.body.y),
        m.caption_scale,
        with_alpha(theme.text, 0.9),
    );
}

/// The channel the readout names: the lane's own channel when stacked; with
/// overlaid traces, the channel whose value at sample `i` is nearest the
/// cursor's height.
fn channel_under_cursor(p: &PlotParams, g: &Geom, lane_i: usize, i: usize, cy: f64) -> usize {
    let channels = p.channels.max(1);
    if !p.overlay {
        return lane_i.min(channels - 1);
    }
    let lane = lane_rect(g.body, g.lanes, 0);
    let (lo, hi) = value_range(p);
    (0..channels)
        .min_by(|&a, &b| {
            let dist = |ch: usize| {
                let v = p.samples[i * channels + ch];
                let y = lane.y + lane.h * (1.0 - fraction(v, lo, hi));
                (y as f64 - cy).abs()
            };
            dist(a).total_cmp(&dist(b))
        })
        .unwrap_or(0)
}

/// The cursor hairline spanning the lane, plus a marker dot on the trace.
fn hairline_and_dot(over: &mut Mesh, lane: Rect, x: f32, y: f32, theme: &Theme) {
    over.rect(
        Rect::new(x, lane.y, 1.0, lane.h),
        with_alpha(theme.text, 0.35),
    );
    over.rect(
        Rect::new(x - 2.0, y - 2.0, 5.0, 5.0),
        with_alpha(theme.text, 0.9),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params<'a>(samples: &'a [f32], channels: usize) -> PlotParams<'a> {
        PlotParams {
            samples,
            channels,
            view: PlotView::Signal,
            overlay: false,
            sample_rate: 0.0,
            min: None,
            max: None,
            ruler: Ruler::Samples,
            ruler_y: true,
            spectrum: None,
            db_floor: -100.0,
            db_ceil: 0.0,
            freq_scale: FreqScale::Log,
            label: None,
        }
    }

    #[test]
    fn a_polyline_is_drawn_for_a_short_signal() {
        let mut m = Mesh::new();
        let samples = [0.0, 0.5, -0.5, 1.0, -1.0];
        let mut p = params(&samples, 1);
        p.min = Some(-1.0);
        p.max = Some(1.0);
        p.label = Some("sig");
        draw(
            &mut m,
            Rect::new(0.0, 0.0, 300.0, 150.0),
            &p,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(!m.is_empty(), "a short signal draws a polyline");
    }

    #[test]
    fn a_long_signal_decimates_to_the_width() {
        // Far more samples than pixels: the envelope path, bounded by the width.
        let big: Vec<f32> = (0..100_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut m = Mesh::new();
        let mut p = params(&big, 1);
        p.ruler = Ruler::Off;
        p.ruler_y = false;
        draw(
            &mut m,
            Rect::new(0.0, 0.0, 100.0, 80.0),
            &p,
            &Metrics::default(),
            &Theme::default(),
        );
        // One bar per column (<= width), each a quad (6 verts): far below the
        // 100k-sample count — proof we never resolve finer than the screen.
        assert!(m.vertex_count() > 0);
        assert!(
            m.vertex_count() < 100 * 6 + 64,
            "decimated to the pixel width"
        );
    }

    #[test]
    fn fewer_than_two_samples_draws_only_chrome() {
        let one = [0.5];
        let none: [f32; 0] = [];
        let mut m = Mesh::new();
        let mut p = params(&one, 1);
        p.min = Some(0.0);
        p.max = Some(1.0);
        p.ruler = Ruler::Off;
        p.ruler_y = false;
        draw(
            &mut m,
            Rect::new(0.0, 0.0, 100.0, 80.0),
            &p,
            &Metrics::default(),
            &Theme::default(),
        );
        let chrome = m.vertex_count();
        let mut m2 = Mesh::new();
        let mut p2 = params(&none, 1);
        p2.min = Some(0.0);
        p2.max = Some(1.0);
        p2.ruler = Ruler::Off;
        p2.ruler_y = false;
        draw(
            &mut m2,
            Rect::new(0.0, 0.0, 100.0, 80.0),
            &p2,
            &Metrics::default(),
            &Theme::default(),
        );
        assert_eq!(chrome, m2.vertex_count(), "one sample adds no trace");
    }

    #[test]
    fn stacked_channels_draw_one_lane_each() {
        // Two interleaved channels: the stacked draw has a divider and two
        // traces, so it carries more geometry than the same data as mono.
        let two: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.1).sin()).collect();
        let mut stacked = Mesh::new();
        draw(
            &mut stacked,
            Rect::new(0.0, 0.0, 200.0, 160.0),
            &params(&two, 2),
            &Metrics::default(),
            &Theme::default(),
        );
        let mut mono = Mesh::new();
        draw(
            &mut mono,
            Rect::new(0.0, 0.0, 200.0, 160.0),
            &params(&two, 1),
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(stacked.vertex_count() > mono.vertex_count());
        // Overlay folds both traces into one lane, still drawing both.
        let mut over = Mesh::new();
        let mut p = params(&two, 2);
        p.overlay = true;
        draw(
            &mut over,
            Rect::new(0.0, 0.0, 200.0, 160.0),
            &p,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(over.vertex_count() > 0);
    }

    #[test]
    fn value_range_auto_fits_and_respects_explicit_sides() {
        // An arbitrary (non-normalized) sequence auto-fits with headroom.
        let seq = [40.0f32, 47.0, 60.0, 100.0];
        let p = params(&seq, 1);
        let (lo, hi) = value_range(&p);
        assert!(lo < 40.0 && lo > 35.0, "{lo}");
        assert!(hi > 100.0 && hi < 105.0, "{hi}");
        // An explicit side overrides only that side.
        let mut p2 = params(&seq, 1);
        p2.min = Some(0.0);
        let (lo2, hi2) = value_range(&p2);
        assert_eq!(lo2, 0.0);
        assert_eq!(hi2, hi);
        // A constant signal still yields a drawable range.
        let flat = [5.0f32; 8];
        let (flo, fhi) = value_range(&params(&flat, 1));
        assert!(fhi > flo);
        // Empty data falls back to the bipolar range.
        let none: [f32; 0] = [];
        let (elo, ehi) = value_range(&params(&none, 1));
        assert!(elo < 0.0 && ehi > 0.0);
    }

    #[test]
    fn analyze_peaks_each_channel_at_its_own_tone() {
        // Channel 0 at 1 kHz, channel 1 at 6 kHz, 48 kHz, FFT 1024.
        let sr = 48_000.0f32;
        let n = 8192;
        let mut interleaved = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f32 / sr;
            interleaved.push((std::f32::consts::TAU * 1000.0 * t).sin());
            interleaved.push((std::f32::consts::TAU * 6000.0 * t).sin());
        }
        let spec = analyze(&interleaved, 2, 1024, sr as f64);
        assert_eq!(spec.curves.len(), 2);
        assert_eq!(spec.nyquist, 24_000.0);
        for (ch, freq) in [(0usize, 1000.0f32), (1, 6000.0)] {
            let peak = spec.curves[ch]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(b, _)| b)
                .unwrap();
            let expected = (freq * 1024.0 / sr).round() as i32;
            assert!(
                (peak as i32 - expected).abs() <= 1,
                "ch{ch}: peak at bin {peak}, expected ~{expected}"
            );
            // Full-scale sine ~0 dB at its bin (coherent-gain normalization).
            assert!(spec.curves[ch][expected as usize] > -6.0);
        }
    }

    #[test]
    fn spectrum_view_draws_curves_and_rulers() {
        let sr = 48_000.0;
        let sig: Vec<f32> = (0..4096)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / sr as f32).sin())
            .collect();
        let spec = analyze(&sig, 1, 1024, sr);
        let mut p = params(&sig, 1);
        p.view = PlotView::Spectrum;
        p.spectrum = Some(&spec);
        p.sample_rate = sr;
        let mut m = Mesh::new();
        draw(
            &mut m,
            Rect::new(0.0, 0.0, 400.0, 200.0),
            &p,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(!m.is_empty(), "the spectrum view draws");
        // Without the analysis, only the chrome draws.
        let mut p2 = params(&sig, 1);
        p2.view = PlotView::Spectrum;
        let mut m2 = Mesh::new();
        draw(
            &mut m2,
            Rect::new(0.0, 0.0, 400.0, 200.0),
            &p2,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(m2.vertex_count() < m.vertex_count());
    }

    #[test]
    fn spectrum_geometry_follows_the_freq_scale() {
        // A live freq_scale change must move the drawn curve and ruler: the
        // same analysis drawn under each scale yields different geometry.
        let sr = 48_000.0;
        let sig: Vec<f32> = (0..8192)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / sr as f32).sin())
            .collect();
        let spec = analyze(&sig, 1, 1024, sr);
        let mesh_for = |scale: FreqScale| {
            let mut p = params(&sig, 1);
            p.view = PlotView::Spectrum;
            p.spectrum = Some(&spec);
            p.sample_rate = sr;
            p.freq_scale = scale;
            let mut m = Mesh::new();
            draw(
                &mut m,
                Rect::new(0.0, 0.0, 400.0, 200.0),
                &p,
                &Metrics::default(),
                &Theme::default(),
            );
            m.positions().collect::<Vec<_>>()
        };
        let lin = mesh_for(FreqScale::Linear);
        let mel = mesh_for(FreqScale::Mel);
        let log = mesh_for(FreqScale::Log);
        assert_ne!(lin, mel, "linear vs mel must draw differently");
        assert_ne!(mel, log, "mel vs log must draw differently");
        assert_ne!(lin, log, "linear vs log must draw differently");
    }

    #[test]
    fn readout_draws_only_inside_the_body_and_names_the_sample() {
        let samples: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let p = params(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 400.0, 200.0);
        let mut over = Mesh::new();
        // Outside the widget: nothing.
        draw_readout(
            &mut over,
            rect,
            &p,
            (1000.0, 1000.0),
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(over.is_empty());
        // Inside the body: hairline + dot + text.
        let g_probe = Rect::new(200.0, 100.0, 0.0, 0.0);
        draw_readout(
            &mut over,
            rect,
            &p,
            (g_probe.x as f64, g_probe.y as f64),
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(!over.is_empty(), "hover inside the body draws the readout");
    }

    #[test]
    fn plot_view_parses_the_wire_names() {
        assert_eq!(PlotView::parse("signal"), Some(PlotView::Signal));
        assert_eq!(PlotView::parse("spectrum"), Some(PlotView::Spectrum));
        assert_eq!(PlotView::parse("histogram"), None);
        assert_eq!(PlotView::default(), PlotView::Signal);
    }
}
