//! The static signal `plot`: a simple line/envelope view of a sample array.
//!
//! The lightweight counterpart of the heavy navigable `waveform`. Where the
//! waveform owns a GPU pipeline and a peak pyramid for editor-grade zoom/pan,
//! the plot just draws a signal once through the flat-geometry painter
//! ([`super::paint`]) — the case the catalog calls "a simple static plot of an
//! NRT-generated signal/file". It honors the project's one graphics rule (never
//! resolve finer than the screen) by decimating to the pixel width: a polyline
//! when the data fits the width, a min/max envelope (one vertical bar per pixel
//! column) when it does not. Pure over a [`Mesh`], so it is unit-testable
//! without a window.

use super::controls::body_rect;
use super::font;
use super::layout::Rect;
use super::meters::fraction;
use super::paint::{Color, Mesh};

const TEXT: Color = [0.85, 0.87, 0.90, 1.0];
const FIELD: Color = [0.10, 0.11, 0.14, 1.0];
const FRAME: Color = [0.45, 0.55, 0.70, 1.0];
const TRACE: Color = [0.55, 0.75, 0.95, 1.0];
const BASELINE: Color = [0.28, 0.32, 0.38, 1.0];
const PAD: f32 = 4.0;
const TEXT_SCALE: f32 = 2.0;
const TRACE_W: f32 = 1.5;

/// Draws `samples` over `[min, max]` into `rect`: a framed field with a label
/// strip, a baseline at value 0 when it falls in range, and the trace. With
/// more samples than the body is wide, the trace is a per-column min/max
/// envelope; otherwise it is a polyline through every sample.
pub fn draw(mesh: &mut Mesh, rect: Rect, samples: &[f32], min: f32, max: f32, label: Option<&str>) {
    if let Some(text) = label {
        font::text(mesh, text, rect.x + PAD, rect.y + PAD, TEXT_SCALE, TEXT);
    }
    let body = body_rect(rect, label.is_some());
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, FIELD);
    mesh.border(body, 1.0, FRAME);

    let y_at = |v: f32| body.y + body.h * (1.0 - fraction(v, min, max));
    // A zero baseline, when 0 is within the displayed range.
    if min < 0.0 && max > 0.0 {
        let y = y_at(0.0);
        mesh.line([body.x, y], [body.x + body.w, y], 1.0, BASELINE);
    }
    if samples.len() < 2 {
        return;
    }

    let cols = body.w.max(1.0) as usize;
    if samples.len() <= cols * 2 {
        // Few enough to draw every sample as a connected polyline.
        let dx = body.w / (samples.len() - 1) as f32;
        let mut prev = [body.x, y_at(samples[0])];
        for (i, v) in samples.iter().enumerate().skip(1) {
            let p = [body.x + i as f32 * dx, y_at(*v)];
            mesh.line(prev, p, TRACE_W, TRACE);
            prev = p;
        }
    } else {
        // Too many: one vertical min/max bar per pixel column (the envelope).
        let n = samples.len();
        let cw = body.w / cols as f32;
        for c in 0..cols {
            let s0 = c * n / cols;
            let s1 = ((c + 1) * n / cols).max(s0 + 1).min(n);
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for &v in &samples[s0..s1] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            let x = body.x + (c as f32 + 0.5) * cw;
            mesh.line(
                [x, y_at(hi)],
                [x, y_at(lo)],
                TRACE_W.min(cw.max(1.0)),
                TRACE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_polyline_is_drawn_for_a_short_signal() {
        let mut m = Mesh::new();
        draw(
            &mut m,
            Rect::new(0.0, 0.0, 200.0, 100.0),
            &[0.0, 0.5, -0.5, 1.0, -1.0],
            -1.0,
            1.0,
            Some("sig"),
        );
        assert!(!m.is_empty(), "a short signal draws a polyline");
    }

    #[test]
    fn a_long_signal_decimates_to_the_width() {
        // Far more samples than pixels: the envelope path, bounded by the width.
        let big: Vec<f32> = (0..100_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut m = Mesh::new();
        draw(
            &mut m,
            Rect::new(0.0, 0.0, 100.0, 80.0),
            &big,
            -1.0,
            1.0,
            None,
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
        let mut m = Mesh::new();
        draw(
            &mut m,
            Rect::new(0.0, 0.0, 100.0, 80.0),
            &[0.5],
            0.0,
            1.0,
            None,
        );
        // The field + border still draw; the empty-range baseline does not (0
        // is the min here). No trace.
        let chrome = m.vertex_count();
        let mut m2 = Mesh::new();
        draw(
            &mut m2,
            Rect::new(0.0, 0.0, 100.0, 80.0),
            &[],
            0.0,
            1.0,
            None,
        );
        assert_eq!(chrome, m2.vertex_count(), "one sample adds no trace");
    }
}
