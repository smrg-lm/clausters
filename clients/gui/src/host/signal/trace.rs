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
/// through the raw samples rather than as min/max columns — the same regime
/// boundary the GPU waveform picks (`crate::waveform`'s `LINE_THRESHOLD`), so
/// a signal looks the same whichever destination draws it.
pub const LINE_THRESHOLD: f64 = 2.0;

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

/// How a trace is inked: the color of its columns and polyline, and the line
/// width the polyline is stroked with (a column is never drawn wider than the
/// pixel column it fills).
#[derive(Debug, Clone, Copy)]
pub struct TraceStyle {
    pub color: Color,
    pub width: f32,
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
        let w = style.width.min(cw.max(1.0));
        for c in 0..cols {
            let x = rect.x + c as f32 * cw;
            let (lo, hi) = trace.column(ch, per_px, src(x), src(x + cw));
            if lo > hi {
                continue;
            }
            mesh.line(
                [x + cw * 0.5, y_at(hi)],
                [x + cw * 0.5, y_at(lo)],
                w,
                style.color,
            );
        }
    } else {
        // Few enough samples per pixel that individual ones matter: step by
        // sample, not by pixel, so nothing visible is skipped.
        let first = src(rect.x).floor().max(0.0) as usize;
        let last = (src(rect.x + rect.w).ceil().max(0.0) as usize).min(frames - 1);
        let mut prev: Option<[f32; 2]> = None;
        for f in first..=last.max(first) {
            let p = [x_of(f as f64), y_at(trace.at(ch, f as f64))];
            if let Some(q) = prev {
                mesh.line(q, p, style.width, style.color);
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
            TraceStyle {
                color: [1.0, 1.0, 1.0, 1.0],
                width: 1.0,
            },
        );
        // Two triangles (six vertices) per column, at most one column per pixel.
        assert!(mesh.vertex_count() <= (rect.w as u32 + 2) * 6);
        assert!(!mesh.is_empty());
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
            TraceStyle {
                color: [1.0, 1.0, 1.0, 1.0],
                width: 1.0,
            },
        );
        // 64 samples -> 63 segments, six vertices each.
        assert_eq!(mesh.vertex_count(), 63 * 6);
    }
}
