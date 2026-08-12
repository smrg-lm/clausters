//! The signal presentation's **one** column source and its mesh renderer.
//!
//! Every view that draws a signal against time answers the same two questions
//! per pixel — *what is the min/max over the span this pixel covers* and *what
//! is the sample at this position* — and the catalog used to answer them three
//! times over: the heavy waveform through [`WaveformData::column`], a clip's
//! inline body with its own slice fold, and the static plot with a third one
//! over an interleaved buffer. [`Trace`] is that one answer, with an arm per
//! source shape: raw interleaved samples, or a [`WaveformData`]'s peak pyramid.
//!
//! [`draw_channel`] is the mesh half of the renderer split the measurements
//! behind this track fixed: a *navigable* signal view owns a GPU slot and
//! builds its columns into a vertex buffer ([`crate::waveform`]), while a view
//! that draws into the shared triangle mesh — a plot, a clip's body — takes
//! this path and allocates no slot at all. Both read their columns from
//! `Trace`, so the two destinations share the arithmetic and differ only in
//! where the vertices land.

use crate::waveform::WaveformData;

use crate::host::layout::Rect;
use crate::host::paint::{Color, Mesh};

/// At or below this many samples per pixel the trace is drawn as a polyline
/// through the raw samples rather than as min/max columns.
///
/// It **is** the GPU waveform's threshold, not a copy of it: the two were two
/// literals with a comment asking them not to drift, which is the weakest form
/// a shared constant can take.
pub use crate::waveform::LINE_THRESHOLD;

/// A signal's samples, read per pixel column. The two arms are the two shapes
/// a source arrives in: an interleaved buffer held in full (an inline body, a
/// plot's array), or a [`WaveformData`] whose peak pyramid answers a zoomed-out
/// column without touching the samples.
pub enum Trace<'a> {
    /// Raw interleaved samples: frame `f` of channel `ch` is
    /// `samples[f * channels + ch]`.
    Samples { samples: &'a [f32], channels: usize },
    /// A pyramid-backed source — the editor-grade path, where a column costs a
    /// pyramid read rather than the samples it summarizes.
    Data(&'a WaveformData),
}

impl<'a> Trace<'a> {
    /// An interleaved buffer of `channels` channels.
    pub fn samples(samples: &'a [f32], channels: usize) -> Self {
        Trace::Samples {
            samples,
            channels: channels.max(1),
        }
    }

    /// How many frames (per-channel samples) the source holds.
    pub fn frames(&self) -> usize {
        match self {
            Trace::Samples { samples, channels } => samples.len() / channels,
            Trace::Data(data) => data.total_samples(),
        }
    }

    /// How many channels the source holds.
    pub fn channels(&self) -> usize {
        match self {
            Trace::Samples { channels, .. } => *channels,
            Trace::Data(data) => data.num_channels(),
        }
    }

    /// Min/max of channel `ch` over the source span `[s0, s1)` — the span one
    /// pixel column covers, at `samples_per_px`. An empty or out-of-range span
    /// reads as silence rather than as nothing, so a column always draws.
    pub fn column(&self, ch: usize, samples_per_px: f64, s0: f64, s1: f64) -> (f32, f32) {
        match self {
            Trace::Samples { samples, channels } => {
                let frames = samples.len() / channels;
                if frames == 0 || ch >= *channels {
                    return (0.0, 0.0);
                }
                // The last column can land exactly on the end: keep the span
                // non-empty and inside the buffer (a `clamp` with min > max
                // panics).
                let a = (s0.floor().max(0.0) as usize).min(frames - 1);
                let b = (s1.ceil() as usize).clamp(a + 1, frames);
                let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
                for f in a..b {
                    let v = samples[f * channels + ch];
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                (lo, hi)
            }
            Trace::Data(data) => data.column(ch, samples_per_px, s0, s1),
        }
    }

    /// One sample of channel `ch` at frame position `s`, clamped to the source.
    pub fn at(&self, ch: usize, s: f64) -> f32 {
        match self {
            Trace::Samples { samples, channels } => {
                let frames = samples.len() / channels;
                if frames == 0 || ch >= *channels {
                    return 0.0;
                }
                let f = (s.round().max(0.0) as usize).min(frames - 1);
                samples[f * channels + ch]
            }
            // A pyramid-only source has no addressable sample: the finest
            // column over one frame is what it can answer.
            Trace::Data(data) => data.column(ch, 1.0, s, s + 1.0).0,
        }
    }
}

/// How a trace is inked: the color of its columns and polyline, and the
/// trace's **weight** — the width the polyline is stroked with, and the least a
/// column is ever inked, so a signal keeps one optical weight across the regime
/// boundary (a column is as wide as the pixel column it fills, which is what
/// makes the columns tile).
#[derive(Debug, Clone, Copy)]
pub struct TraceStyle {
    pub color: Color,
    pub width: f32,
    /// The radius a **sample dot** is drawn at once the samples are far enough
    /// apart to carry one ([`dots_fit`]); `0` draws none.
    ///
    /// It is `point_radius` — the role a break-point is drawn at — and that is
    /// the point rather than a coincidence: a dot says *this is a sample, and
    /// it is a thing you could take hold of*, which is what sample-level
    /// editing will grab. Sizing it as a curve's break-point means the two
    /// affordances read as the same kind of target the day the second one
    /// becomes draggable.
    pub dot_radius: f32,
}

/// Whether sample dots are drawn at `spacing` pixels apart: they need to read
/// as separate points, so a dot is drawn only once its neighbour is three radii
/// away — a full diameter of air between them. Below that the line is the
/// picture and a row of touching dots would just thicken it.
pub fn dots_fit(spacing: f32, radius: f32) -> bool {
    radius > 0.0 && spacing >= 3.0 * radius
}

impl TraceStyle {
    /// A trace inked at `width`, with no sample dots.
    pub fn new(color: Color, width: f32) -> Self {
        TraceStyle {
            color,
            width,
            dot_radius: 0.0,
        }
    }

    /// The same trace, marking each sample once they are far enough apart.
    pub fn with_dots(mut self, radius: f32) -> Self {
        self.dot_radius = radius;
        self
    }
}

/// Draws one channel of `trace` into `rect`, resolved to the rect's own pixel
/// width and never finer — the project's one graphics rule.
///
/// The two coordinate maps are the caller's, because they are what differs
/// between the views that share this renderer: `src` takes an x pixel to the
/// source frame it falls on (through a clip's placement and the navigation
/// window, or straight down the whole buffer), `x_of` is its inverse, and
/// `y_at` maps a sample value to a y pixel inside the lane. Above
/// [`LINE_THRESHOLD`] samples per pixel it draws one min/max column per pixel;
/// below it, the polyline through every visible sample.
// The rect, the source, the channel, three coordinate maps and a style: all
// distinct inputs to one drawing pass, clearer flat than bundled.
#[allow(clippy::too_many_arguments)]
pub fn draw_channel(
    mesh: &mut Mesh,
    rect: Rect,
    trace: &Trace,
    ch: usize,
    src: impl Fn(f32) -> f64,
    x_of: impl Fn(f64) -> f32,
    y_at: impl Fn(f32) -> f32,
    style: TraceStyle,
) {
    let frames = trace.frames();
    if frames < 2 || rect.w < 1.0 || rect.h <= 0.0 {
        return;
    }
    let cols = rect.w.max(1.0) as usize;
    let cw = rect.w / cols as f32;
    let per_px = (src(rect.x + cw) - src(rect.x)).max(0.0);
    if per_px >= LINE_THRESHOLD {
        for c in 0..cols {
            let x = rect.x + c as f32 * cw;
            let (lo, hi) = trace.column(ch, per_px, src(x), src(x + cw));
            if lo > hi {
                continue;
            }
            // A column is a quad — the shape the GPU waveform emits for this
            // same regime — and it is **never inked thinner than the trace's
            // weight in either direction**: at least the pixel column it fills,
            // so columns tile into a solid band on a dense signal, and at least
            // `style.width`, so a signal keeps one optical weight across the
            // regime boundary. The centred stroke this replaces was thinner on
            // both counts: capped to the column width it came out below the
            // weight the polyline uses a pixel away, and where the signal
            // barely moves inside one column it was a zero-length line, which
            // draws *nothing* — so the flat stretch of an envelope disappeared
            // exactly where it is most readable. Overlapping neighbours is the
            // price of the second floor, and it is what a stroke does anyway.
            let (top, bottom) = (y_at(hi), y_at(lo));
            let (w, h) = (cw.max(style.width), (bottom - top).max(style.width));
            let (cx, cy) = (x + cw * 0.5, (top + bottom) * 0.5);
            mesh.rect(Rect::new(cx - w * 0.5, cy - h * 0.5, w, h), style.color);
        }
    } else {
        // Few enough samples per pixel that individual ones matter: step by
        // sample, not by pixel, so nothing visible is skipped.
        let first = src(rect.x).floor().max(0.0) as usize;
        let last = (src(rect.x + rect.w).ceil().max(0.0) as usize).min(frames - 1);
        // Where the samples land decides whether each one is marked: the line
        // is an interpolation the drawing invents, and a dot is what says which
        // points of it are data.
        let spacing = (x_of(1.0) - x_of(0.0)).abs();
        let dots = dots_fit(spacing, style.dot_radius);
        let mut prev: Option<[f32; 2]> = None;
        for f in first..=last.max(first) {
            let p = [x_of(f as f64), y_at(trace.at(ch, f as f64))];
            if let Some(q) = prev {
                mesh.line(q, p, style.width, style.color);
            }
            if dots {
                mesh.disc(p[0], p[1], style.dot_radius, style.color);
            }
            prev = Some(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interleaved_column_reads_its_own_channel() {
        // Two channels: channel 0 rises, channel 1 is its negative.
        let samples: Vec<f32> = (0..8).flat_map(|i| [i as f32, -(i as f32)]).collect();
        let trace = Trace::samples(&samples, 2);
        assert_eq!(trace.frames(), 8);
        assert_eq!(trace.channels(), 2);
        assert_eq!(trace.column(0, 4.0, 0.0, 4.0), (0.0, 3.0));
        assert_eq!(trace.column(1, 4.0, 0.0, 4.0), (-3.0, 0.0));
        assert_eq!(trace.at(0, 5.0), 5.0);
        assert_eq!(trace.at(1, 5.0), -5.0);
    }

    #[test]
    fn a_column_landing_on_the_end_is_still_one_sample_wide() {
        let samples = [1.0f32, 2.0, 3.0];
        let trace = Trace::samples(&samples, 1);
        // s0 exactly at the last frame, s1 past it: the span clamps inside the
        // buffer instead of panicking or reading nothing.
        assert_eq!(trace.column(0, 1.0, 3.0, 4.0), (3.0, 3.0));
        assert_eq!(trace.at(0, 99.0), 3.0);
    }

    /// The pyramid arm answers the same envelope as the raw one for a source
    /// that has both — the property that lets a take and an inline body be one
    /// renderer.
    #[test]
    fn the_pyramid_arm_agrees_with_the_raw_arm_on_the_same_signal() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32 * 0.05).sin()).collect();
        let data = WaveformData::new(samples.clone().into(), 256);
        let raw = Trace::samples(&samples, 1);
        let pyr = Trace::Data(&data);
        for c in 0..16 {
            let (s0, s1) = (c as f64 * 256.0, (c + 1) as f64 * 256.0);
            // Below the base bucket both read the raw samples, so they agree
            // exactly; the crossfade above it is the pyramid's own business.
            let a = raw.column(0, 128.0, s0, s0 + 128.0);
            let b = pyr.column(0, 128.0, s0, s0 + 128.0);
            assert_eq!(a, b, "column {c} over [{s0}, {s1})");
        }
    }

    /// Zoomed out, the trace costs the rect's pixels — not the source's
    /// samples. This is the rule the three implementations each restated.
    #[test]
    fn a_long_source_costs_the_rect_width_not_its_samples() {
        let samples: Vec<f32> = (0..200_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 300.0, 100.0);
        let n = samples.len() as f64;
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| (x - rect.x) as f64 / rect.w as f64 * n,
            |s| rect.x + (s / n) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        // Two triangles (six vertices) per column, at most one column per pixel.
        assert!(mesh.vertex_count() <= (rect.w as u32 + 2) * 6);
        assert!(!mesh.is_empty());
    }

    /// A column the signal barely moves in is still inked, at the trace's own
    /// weight in **both** directions. It used to be a zero-length line — which
    /// draws nothing at all — so a slow curve faded out exactly where it
    /// flattened: the sustain of an envelope, the tail of a decay. And a column
    /// narrower than the weight read thinner than the polyline the same
    /// function draws a pixel the other side of the threshold. The regime
    /// decides how a signal is resolved, never how heavily it is inked.
    #[test]
    fn a_flat_column_still_inks_the_traces_own_weight() {
        // A constant signal, long enough to be well inside the column regime.
        let samples = vec![0.5f32; 40_000];
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let n = samples.len() as f64;
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| (x - rect.x) as f64 / rect.w as f64 * n,
            |s| rect.x + (s / n) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.5),
        );
        // Every column drew: six vertices each, none collapsed away.
        assert_eq!(mesh.vertex_count(), rect.w as u32 * 6);
        // ...and each one is a quad at least the trace's weight both ways: a
        // flat signal over a 200 px rect inks a band 1.5 px thick, not a
        // hairline and not nothing.
        let inked = mesh.extent().expect("the flat signal drew");
        assert!(
            (inked.h - 1.5).abs() < 1e-3,
            "a flat column inks the trace weight vertically, got {}",
            inked.h
        );
        assert!(
            inked.w >= rect.w + 0.5 - 1e-3,
            "columns span the rect, widened to the weight, got {}",
            inked.w
        );
    }

    /// **A column is its own envelope, never a fill to the baseline** — the
    /// divergence this closed, and it closed the other way from the first
    /// attempt. The GPU pipeline used to clamp every column to zero; clamping
    /// everywhere would have inked a band the signal was never in.
    ///
    /// The two cases are the whole argument. A signal sitting at +0.8 draws a
    /// thin band at +0.8 whatever its domain says, because that is where it
    /// was; and a signal that swings across zero draws the solid body every
    /// editor draws, because *the data fills it* — no rule, no threshold, and
    /// nothing that has to know the zoom.
    #[test]
    fn a_column_is_its_own_envelope_and_the_data_is_what_fills_it() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let draw = |samples: &[f32], min: f32, max: f32| {
            let trace = Trace::samples(samples, 1);
            let n = samples.len() as f64;
            let mut mesh = Mesh::new();
            draw_channel(
                &mut mesh,
                rect,
                &trace,
                0,
                |x| (x - rect.x) as f64 / rect.w as f64 * n,
                |s| rect.x + (s / n) as f32 * rect.w,
                move |v| {
                    rect.y + rect.h * (1.0 - crate::host::graphics::meters::fraction(v, min, max))
                },
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
            );
            mesh.extent().expect("the signal drew").h
        };
        // A signal that never comes near zero is a band at its own level, in a
        // bipolar domain exactly as in a unipolar one.
        let offset = vec![0.8f32; 4_000];
        assert!(
            draw(&offset, -1.0, 1.0) < rect.h * 0.05,
            "an offset signal is not filled from the baseline"
        );
        assert!(
            draw(&offset, 0.0, 1.0) < rect.h * 0.05,
            "...and its domain does not change that"
        );
        // A signal that does cross zero fills, and the data is what fills it:
        // every column's own min/max spans the lane.
        let swinging: Vec<f32> = (0..4_000)
            .map(|i| if i % 2 == 0 { 0.9 } else { -0.9 })
            .collect();
        assert!(
            draw(&swinging, -1.0, 1.0) > rect.h * 0.8,
            "audio at overview zoom is the solid body it always was"
        );
    }

    /// A **subsonic** signal is the case that proves the zoom could not have
    /// been the criterion: a cycle a second has far more samples than the
    /// screen has pixels — deep in the column regime — and is a curve, not a
    /// body. Every column is a thin band, and the bands trace the wave.
    #[test]
    fn a_subsonic_signal_draws_a_curve_not_a_body() {
        // One cycle of a 1 Hz sine at 48 kHz: 48000 samples over 100 px.
        let samples: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 / 48_000.0 * std::f32::consts::TAU).sin())
            .collect();
        let trace = Trace::samples(&samples, 1);
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let n = samples.len() as f64;
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| (x - rect.x) as f64 / rect.w as f64 * n,
            |s| rect.x + (s / n) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        // Well past the polyline threshold — this is the column regime.
        assert!(n / rect.w as f64 > LINE_THRESHOLD * 100.0);
        // The column at the peak barely moves: a thin band near the top, not a
        // slab reaching down to the zero line.
        let peak_col = (rect.w * 0.25) as usize;
        let x = rect.x + peak_col as f32;
        let per_px = n / rect.w as f64;
        let (lo, hi) = trace.column(0, per_px, x as f64 * per_px, (x as f64 + 1.0) * per_px);
        assert!(lo > 0.9 && hi <= 1.0, "the peak column is [{lo}, {hi}]");
        assert!(
            (hi - lo) < 0.05,
            "a slow cycle hardly moves inside one column"
        );
    }

    /// **Sample dots**: once the samples stand far enough apart to read as
    /// separate points, each one is marked. The line between them is an
    /// interpolation the drawing invents — the dot is what says which points of
    /// it are data, and what sample-level editing will take hold of, which is
    /// why it is sized as a curve's break-point.
    #[test]
    fn samples_are_marked_once_they_stand_apart() {
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let draw = |n: usize, radius: f32| {
            let samples: Vec<f32> = (0..n).map(|i| (i as f32 * 0.3).sin()).collect();
            let trace = Trace::samples(&samples, 1);
            let span = (n - 1) as f64;
            let mut mesh = Mesh::new();
            draw_channel(
                &mut mesh,
                rect,
                &trace,
                0,
                |x| (x - rect.x) as f64 / rect.w as f64 * span,
                |s| rect.x + (s / span) as f32 * rect.w,
                |v| rect.y + rect.h * 0.5 * (1.0 - v),
                TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0).with_dots(radius),
            );
            mesh.vertex_count()
        };
        // 20 samples over 200 px: 10 px apart, past three radii — marked.
        let marked = draw(20, 3.0);
        let bare = draw(20, 0.0);
        assert!(
            marked > bare,
            "deep zoom marks each sample: {marked} vs {bare}"
        );
        // 100 samples over the same width: 2 px apart, so a row of dots would
        // just thicken the line. None are drawn, and the picture is the line
        // it was.
        assert_eq!(
            draw(100, 3.0),
            draw(100, 0.0),
            "dots that would touch are not drawn"
        );
    }

    /// The rule itself, stated where both renderers read it: three radii of
    /// separation, so a dot has a full diameter of air around it.
    #[test]
    fn dots_need_a_diameter_of_air() {
        assert!(!dots_fit(5.0, 0.0), "no radius, no dots");
        assert!(!dots_fit(8.0, 3.0));
        assert!(dots_fit(9.0, 3.0));
        assert!(dots_fit(40.0, 4.0));
    }

    /// Zoomed in past the threshold, every sample in range is a polyline
    /// vertex — the regime where a pixel-stepped loop would drop samples.
    #[test]
    fn zoomed_in_the_polyline_visits_every_visible_sample() {
        let samples: Vec<f32> = (0..64)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let trace = Trace::samples(&samples, 1);
        // 64 samples over 200 px: well under the threshold.
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let n = samples.len() as f64;
        let mut mesh = Mesh::new();
        draw_channel(
            &mut mesh,
            rect,
            &trace,
            0,
            |x| (x - rect.x) as f64 / rect.w as f64 * n,
            |s| rect.x + (s / n) as f32 * rect.w,
            |v| rect.y + rect.h * 0.5 * (1.0 - v),
            TraceStyle::new([1.0, 1.0, 1.0, 1.0], 1.0),
        );
        // 64 samples -> 63 segments, six vertices each.
        assert_eq!(mesh.vertex_count(), 63 * 6);
    }
}
