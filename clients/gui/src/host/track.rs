//! The multitrack `track`/`clip` graphic unit: the DAW-style lane view.
//!
//! A `track` is a horizontal lane of the shared timeline; a `clip` is a placed
//! rectangle on it spanning `[offset, offset + dur]` in timeline sample units —
//! the model's **graphic unit** (length = duration). This module draws that
//! unit: a left header naming the track, the lane field, and one framed
//! rectangle per clip with its decimated body and label. Pure over a [`Mesh`]
//! (the flat-geometry [`super::paint`] painter), so it is unit-testable without
//! a window — the same posture as the static `plot`/`bpf` views.
//!
//! The tracks of one window share **one time axis** (aligned lanes): the frame
//! renderer computes the common span (the longest clip end) and maps every
//! lane's clips through the same [`View`], so a clip at offset 8 lines up
//! across tracks. Placement/geometry is display logic — this stays gui-side.

use std::sync::Arc;

use super::font;
use super::layout::Rect;
use super::meters::fraction;
use super::paint::{Color, Mesh};
use crate::viewport::View;

const TEXT: Color = [0.85, 0.87, 0.90, 1.0];
const HEADER_FILL: Color = [0.14, 0.16, 0.20, 1.0];
const LANE_FILL: Color = [0.09, 0.10, 0.13, 1.0];
const FRAME: Color = [0.30, 0.34, 0.42, 1.0];
const CLIP_FILL: Color = [0.16, 0.22, 0.32, 1.0];
const CLIP_EDGE: Color = [0.45, 0.60, 0.85, 1.0];
const CLIP_BODY: Color = [0.55, 0.75, 0.95, 1.0];
const BASELINE: Color = [0.28, 0.32, 0.38, 1.0];
/// The left header strip width, device pixels — shared by every lane so the
/// clip bodies of aligned tracks line up.
pub const HEADER_W: f32 = 96.0;
const PAD: f32 = 4.0;
const HEADER_SCALE: f32 = 2.0;
const CLIP_SCALE: f32 = 1.5;
const BODY_W: f32 = 1.0;

/// One clip copied out of the host tree for drawing (and hit-testing).
#[derive(Clone)]
pub struct ClipDraw {
    pub id: i32,
    pub offset: f64,
    pub dur: f64,
    pub samples: Arc<[f32]>,
    pub min: f32,
    pub max: f32,
    pub label: Option<String>,
}

/// The lane body (the part right of the header strip) of a track's `rect`.
pub fn lane_body(rect: Rect) -> Rect {
    let hw = HEADER_W.min(rect.w);
    Rect::new(rect.x + hw, rect.y, (rect.w - hw).max(0.0), rect.h)
}

/// Maps sample position `s` to an x pixel inside `body` through `nav`.
fn to_x(s: f64, nav: &View, body: Rect) -> f64 {
    body.x as f64 + (s - nav.start) / nav.len.max(1.0) * body.w as f64
}

/// The x pixel range a clip's `[offset, offset + dur]` span occupies inside the
/// lane `body` through the shared `nav`, clamped to the body. Returns `None`
/// when the clip has no duration or falls entirely outside the visible window.
pub fn clip_x_range(body: Rect, nav: &View, offset: f64, dur: f64) -> Option<(f32, f32)> {
    if dur <= 0.0 {
        return None;
    }
    let lo = body.x as f64;
    let hi = (body.x + body.w) as f64;
    let x0 = to_x(offset, nav, body).clamp(lo, hi);
    let x1 = to_x(offset + dur, nav, body).clamp(lo, hi);
    (x1 > x0).then_some((x0 as f32, x1 as f32))
}

/// Draws one track lane into `rect`: the header (with `label`), the lane field,
/// and every clip as a framed rectangle (its body decimated inside) through the
/// shared timeline `nav`.
pub fn draw(mesh: &mut Mesh, rect: Rect, nav: &View, label: Option<&str>, clips: &[ClipDraw]) {
    // Header strip on the left, naming the track.
    let header = Rect::new(rect.x, rect.y, HEADER_W.min(rect.w), rect.h);
    mesh.rect(header, HEADER_FILL);
    if let Some(t) = label {
        font::text(mesh, t, header.x + PAD, rect.y + PAD, HEADER_SCALE, TEXT);
    }
    let body = lane_body(rect);
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, LANE_FILL);
    mesh.border(body, 1.0, FRAME);
    for clip in clips {
        let Some((x0, x1)) = clip_x_range(body, nav, clip.offset, clip.dur) else {
            continue;
        };
        let cr = Rect::new(x0, body.y + 1.0, x1 - x0, (body.h - 2.0).max(0.0));
        mesh.rect(cr, CLIP_FILL);
        mesh.border(cr, 1.0, CLIP_EDGE);
        draw_clip_body(mesh, cr, &clip.samples, clip.min, clip.max);
        if let Some(t) = &clip.label {
            font::text(mesh, t, cr.x + PAD, cr.y + PAD, CLIP_SCALE, TEXT);
        }
    }
}

/// Draws a clip's inline body decimated inside its rectangle (min/max envelope
/// per column, or a polyline when it fits), no chrome — the graphic-unit body.
/// Honors the one graphics rule (never resolve finer than the screen).
fn draw_clip_body(mesh: &mut Mesh, rect: Rect, samples: &[f32], min: f32, max: f32) {
    if samples.len() < 2 || rect.w < 2.0 || rect.h <= 0.0 {
        return;
    }
    let y_at = |v: f32| rect.y + rect.h * (1.0 - fraction(v, min, max));
    if min < 0.0 && max > 0.0 {
        let y = y_at(0.0);
        mesh.line([rect.x, y], [rect.x + rect.w, y], 1.0, BASELINE);
    }
    let cols = rect.w.max(1.0) as usize;
    let n = samples.len();
    if n <= cols * 2 {
        let dx = rect.w / (n - 1) as f32;
        let mut prev = [rect.x, y_at(samples[0])];
        for (i, v) in samples.iter().enumerate().skip(1) {
            let p = [rect.x + i as f32 * dx, y_at(*v)];
            mesh.line(prev, p, BODY_W, CLIP_BODY);
            prev = p;
        }
    } else {
        let cw = rect.w / cols as f32;
        for c in 0..cols {
            let s0 = c * n / cols;
            let s1 = ((c + 1) * n / cols).max(s0 + 1).min(n);
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for &v in &samples[s0..s1] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            let x = rect.x + (c as f32 + 0.5) * cw;
            mesh.line(
                [x, y_at(hi)],
                [x, y_at(lo)],
                BODY_W.min(cw.max(1.0)),
                CLIP_BODY,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane() -> Rect {
        // A 500-wide track: header 96 + a 404-wide lane body.
        Rect::new(0.0, 0.0, 500.0, 60.0)
    }

    #[test]
    fn lane_body_reserves_the_header_strip() {
        let body = lane_body(lane());
        assert_eq!((body.x, body.w), (HEADER_W, 500.0 - HEADER_W));
    }

    #[test]
    fn clip_x_range_places_the_clip_by_offset_and_duration() {
        let body = lane_body(lane());
        let nav = View::full(400); // 1 sample per pixel over the 404-wide body-ish
        // A clip at [100, 200): starts a quarter in, one-quarter wide.
        let (x0, x1) = clip_x_range(body, &nav, 100.0, 100.0).unwrap();
        let px_per = body.w as f64 / 400.0;
        assert!((x0 as f64 - (body.x as f64 + 100.0 * px_per)).abs() < 0.5);
        assert!((x1 as f64 - (body.x as f64 + 200.0 * px_per)).abs() < 0.5);
    }

    #[test]
    fn clip_x_range_clips_to_the_body_and_drops_the_invisible() {
        let body = lane_body(lane());
        let nav = View {
            start: 150.0,
            len: 100.0,
        };
        // A clip [0, 100) ends before the window: fully invisible.
        assert!(clip_x_range(body, &nav, 0.0, 100.0).is_none());
        // A clip [100, 400) overlaps the left edge: clamped to the body start.
        let (x0, _) = clip_x_range(body, &nav, 100.0, 300.0).unwrap();
        assert_eq!(x0, body.x);
        // A zero-duration clip draws nothing.
        assert!(clip_x_range(body, &nav, 160.0, 0.0).is_none());
    }

    #[test]
    fn draw_paints_the_header_lane_and_clips() {
        let mut m = Mesh::new();
        let clips = vec![
            ClipDraw {
                id: 1,
                offset: 0.0,
                dur: 100.0,
                samples: vec![0.0, 0.5, -0.5, 1.0].into(),
                min: -1.0,
                max: 1.0,
                label: Some("a".into()),
            },
            ClipDraw {
                id: 2,
                offset: 200.0,
                dur: 100.0,
                samples: Arc::from([] as [f32; 0]),
                min: -1.0,
                max: 1.0,
                label: None,
            },
        ];
        draw(&mut m, lane(), &View::full(400), Some("drums"), &clips);
        assert!(!m.is_empty(), "the header, lane and clips draw");
    }
}
