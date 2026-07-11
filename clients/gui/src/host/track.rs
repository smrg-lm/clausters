//! The multitrack `track`/`clip` graphic unit: the DAW-style lane view.
//!
//! A `track` is a horizontal lane of the shared timeline; a `clip` is a placed
//! rectangle on it spanning `[offset, offset + dur]` in timeline sample units —
//! the model's **graphic unit** (length = duration). This module draws that
//! unit: a left header naming the track, the lane field, and one framed
//! rectangle per clip with its label and a body — a decimated waveform, or a
//! **piano-roll** of note events when the clip carries `notes` (the events
//! track's scalar-vertical view). Pure over a [`Mesh`] (the flat-geometry
//! [`super::paint`] painter), so it is unit-testable without a window — the
//! same posture as the static `plot`/`bpf` views.
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
use super::widget::{Widget, WidgetKind};
use crate::viewport::View;
use crate::waveform::WaveformData;

const TEXT: Color = [0.85, 0.87, 0.90, 1.0];
const HEADER_FILL: Color = [0.14, 0.16, 0.20, 1.0];
const LANE_FILL: Color = [0.09, 0.10, 0.13, 1.0];
const FRAME: Color = [0.30, 0.34, 0.42, 1.0];
const CLIP_FILL: Color = [0.16, 0.22, 0.32, 1.0];
const CLIP_EDGE: Color = [0.45, 0.60, 0.85, 1.0];
const CLIP_BODY: Color = [0.55, 0.75, 0.95, 1.0];
const NOTE_FILL: Color = [0.60, 0.85, 0.65, 1.0];
const BASELINE: Color = [0.28, 0.32, 0.38, 1.0];
/// The left header strip width, device pixels — shared by every lane so the
/// clip bodies of aligned tracks line up.
pub const HEADER_W: f32 = 96.0;
const PAD: f32 = 4.0;
const HEADER_SCALE: f32 = 2.0;
const CLIP_SCALE: f32 = 1.5;
const BODY_W: f32 = 1.0;

/// One note of a piano-roll clip: its `start`/`dur` **relative to the clip's
/// offset** (timeline samples), and its `pitch` (mapped over the clip's value
/// range — `min`/`max` read as the low/high pitch).
#[derive(Clone, Copy, Debug)]
pub struct Note {
    pub start: f64,
    pub dur: f64,
    pub pitch: f32,
}

/// One clip copied out of the host tree for drawing (and hit-testing). A clip
/// with `notes` draws a piano-roll body (its samples ignored); one without draws
/// the decimated waveform body — from `data` (a loaded `cache`/`path`/`buffer`
/// take, decimated through its peak pyramid) when the host has one, else from
/// the inline `samples`.
#[derive(Clone)]
pub struct ClipDraw {
    pub id: i32,
    pub offset: f64,
    pub dur: f64,
    pub samples: Arc<[f32]>,
    pub data: Option<Arc<WaveformData>>,
    pub notes: Vec<Note>,
    pub min: f32,
    pub max: f32,
    pub label: Option<String>,
}

/// The span of a window's shared time axis: the longest clip end (`offset +
/// dur`) over every track in `tree`. Clips only exist under tracks, so a plain
/// tree walk finds them all. `0.0` when the window has no clips.
pub fn window_span(tree: &Widget) -> f64 {
    fn walk(w: &Widget, acc: &mut f64) {
        if let WidgetKind::Clip { offset, dur, .. } = w.kind {
            *acc = acc.max(offset + dur);
        }
        for c in &w.children {
            walk(c, acc);
        }
    }
    let mut span = 0.0;
    walk(tree, &mut span);
    span
}

/// The shared full-span navigation window of a window's tracks (aligned lanes).
/// The render and the hit-test both read it, so a clip maps to the same pixels
/// either way.
pub fn window_nav(tree: &Widget) -> View {
    View::full(window_span(tree).ceil().max(1.0) as usize)
}

/// The lane body of a track's `rect`: the part right of the header strip, and
/// above the time-ruler strip when the lane draws one (`ruler`). The renderer
/// and the hit-test both call this, so a clip occupies the same pixels either
/// way — pass the same flag (a lane with `Ruler::Off` reserves no strip, which
/// is the un-rulered default).
pub fn lane_body(rect: Rect, ruler: bool) -> Rect {
    let hw = HEADER_W.min(rect.w);
    let rh = if ruler { RULER_H.min(rect.h) } else { 0.0 };
    Rect::new(
        rect.x + hw,
        rect.y,
        (rect.w - hw).max(0.0),
        (rect.h - rh).max(0.0),
    )
}

/// The height of a lane's time-ruler strip, device pixels (the tick marks plus
/// one row of labels — the same budget the timeline views' ruler strip gets).
pub const RULER_H: f32 = 18.0;

/// The x pixel of a timeline sample position inside the lane `body`, or `None`
/// when it falls outside the visible window. The playhead reads it: the engine
/// clock is a timeline position like any other, so it lands on the same axis the
/// clips are placed on.
pub fn playhead_x(body: Rect, nav: &View, pos: f64) -> Option<f32> {
    (pos >= nav.start && pos <= nav.start + nav.len).then(|| to_x(pos, nav, body) as f32)
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
/// shared timeline `nav`. `ruler` reserves the bottom strip for the time ruler
/// (drawn by the frame renderer, which owns the tick math); the playhead is an
/// overlay, over the clips.
pub fn draw(
    mesh: &mut Mesh,
    rect: Rect,
    nav: &View,
    label: Option<&str>,
    clips: &[ClipDraw],
    ruler: bool,
) {
    // Header strip on the left, naming the track.
    let header = Rect::new(rect.x, rect.y, HEADER_W.min(rect.w), rect.h);
    mesh.rect(header, HEADER_FILL);
    if let Some(t) = label {
        font::text(mesh, t, header.x + PAD, rect.y + PAD, HEADER_SCALE, TEXT);
    }
    let body = lane_body(rect, ruler);
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
        if clip.notes.is_empty() {
            match &clip.data {
                // A loaded take (mapped file, peak cache or fetched buffer):
                // decimated through its pyramid, so a minutes-long clip costs
                // the same as a short one.
                Some(data) => draw_take_body(mesh, cr, data, clip.min, clip.max),
                None => draw_clip_body(mesh, cr, &clip.samples, clip.min, clip.max),
            }
        } else {
            // A piano-roll clip: notes placed on the same shared axis (so the
            // whole roll moves when the clip does), pitch mapped over [min, max].
            draw_piano_roll(mesh, cr, body, nav, clip);
        }
        if let Some(t) = &clip.label {
            font::text(mesh, t, cr.x + PAD, cr.y + PAD, CLIP_SCALE, TEXT);
        }
    }
}

/// The minimum drawn height of a note rectangle, device pixels.
const NOTE_MIN_H: f32 = 2.0;

/// Draws a clip's notes as a piano-roll inside `cr`: each note is a rectangle
/// placed in time on the shared `nav` (through the lane `body`, so it lines up
/// with waveform clips) and in pitch over the clip's `[min, max]` range. New
/// geometry, deliberately the reused-body sibling of `draw_clip_body`. Pitch
/// mapping is linear here; a note-name ruler would read `clausters_core::scale`.
fn draw_piano_roll(mesh: &mut Mesh, cr: Rect, body: Rect, nav: &View, clip: &ClipDraw) {
    let (lo, hi) = (clip.min, clip.max);
    let x_lo = cr.x;
    let x_hi = cr.x + cr.w;
    // Each pitch step gets a row; a note is a bar filling most of its row.
    let rows = (hi - lo).max(1.0);
    let row_h = (cr.h / rows).clamp(0.0, cr.h);
    for n in &clip.notes {
        let mut nx0 = to_x(clip.offset + n.start, nav, body) as f32;
        let mut nx1 = to_x(clip.offset + n.start + n.dur.max(0.0), nav, body) as f32;
        nx0 = nx0.clamp(x_lo, x_hi);
        nx1 = nx1.clamp(x_lo, x_hi);
        if nx1 <= nx0 {
            continue;
        }
        // Pitch → y within the clip rect (high pitch at the top).
        let frac = fraction(n.pitch, lo, hi);
        let y = cr.y + cr.h * (1.0 - frac as f32);
        let h = row_h.max(NOTE_MIN_H).min(cr.h);
        let y = (y - h * 0.5).clamp(cr.y, cr.y + cr.h - h);
        mesh.rect(Rect::new(nx0, y, nx1 - nx0, h), NOTE_FILL);
    }
}

/// Draws a **loaded take** as the clip's body: one min/max column per pixel,
/// read from the take's peak pyramid (`clausters_core::peaks`, through
/// [`WaveformData::column`] — the same LOD source, crossfade and all, the heavy
/// waveform view draws from). The take is decimated to the clip's pixel width,
/// so a minutes-long file costs a screen's worth of columns, never its samples:
/// the one graphics rule (never resolve finer than the screen), and the reason a
/// long clip needs no GPU slot of its own.
fn draw_take_body(mesh: &mut Mesh, rect: Rect, data: &WaveformData, min: f32, max: f32) {
    let total = data.total_samples();
    if total == 0 || rect.w < 2.0 || rect.h <= 0.0 {
        return;
    }
    let y_at = |v: f32| rect.y + rect.h * (1.0 - fraction(v, min, max));
    if min < 0.0 && max > 0.0 {
        let y = y_at(0.0);
        mesh.line([rect.x, y], [rect.x + rect.w, y], 1.0, BASELINE);
    }
    // The whole take spans the clip rectangle (the clip's `dur` is its length on
    // the timeline; its body is the take, summarized to fit).
    let cols = rect.w.max(1.0) as usize;
    let cw = rect.w / cols as f32;
    let per_px = total as f64 / cols as f64;
    for c in 0..cols {
        let s0 = c as f64 * per_px;
        let s1 = s0 + per_px;
        let (lo, hi) = data.column(0, per_px, s0, s1);
        let x = rect.x + (c as f32 + 0.5) * cw;
        mesh.line(
            [x, y_at(hi)],
            [x, y_at(lo)],
            BODY_W.min(cw.max(1.0)),
            CLIP_BODY,
        );
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
        let body = lane_body(lane(), false);
        assert_eq!((body.x, body.w), (HEADER_W, 500.0 - HEADER_W));
        assert_eq!(
            body.h,
            lane().h,
            "no ruler, no strip: the lane is full height"
        );
    }

    #[test]
    fn lane_body_reserves_the_ruler_strip_when_the_lane_has_one() {
        let ruled = lane_body(lane(), true);
        assert_eq!(ruled.h, lane().h - RULER_H);
        // The header is unaffected: the strip comes off the bottom.
        assert_eq!((ruled.x, ruled.w), (HEADER_W, 500.0 - HEADER_W));
    }

    #[test]
    fn playhead_x_places_the_clock_on_the_shared_axis() {
        let body = lane_body(lane(), false);
        let nav = View::full(400);
        // Halfway through the timeline: halfway across the lane body.
        let x = playhead_x(body, &nav, 200.0).unwrap();
        assert!((x - (body.x + body.w * 0.5)).abs() < 0.5);
        // Past the end of the window: nothing to draw.
        assert!(playhead_x(body, &nav, 500.0).is_none());
    }

    #[test]
    fn clip_x_range_places_the_clip_by_offset_and_duration() {
        let body = lane_body(lane(), false);
        let nav = View::full(400); // 1 sample per pixel over the 404-wide body-ish
        // A clip at [100, 200): starts a quarter in, one-quarter wide.
        let (x0, x1) = clip_x_range(body, &nav, 100.0, 100.0).unwrap();
        let px_per = body.w as f64 / 400.0;
        assert!((x0 as f64 - (body.x as f64 + 100.0 * px_per)).abs() < 0.5);
        assert!((x1 as f64 - (body.x as f64 + 200.0 * px_per)).abs() < 0.5);
    }

    #[test]
    fn clip_x_range_clips_to_the_body_and_drops_the_invisible() {
        let body = lane_body(lane(), false);
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
                data: None,
                notes: Vec::new(),
                min: -1.0,
                max: 1.0,
                label: Some("a".into()),
            },
            ClipDraw {
                id: 2,
                offset: 200.0,
                dur: 100.0,
                samples: Arc::from([] as [f32; 0]),
                data: None,
                notes: Vec::new(),
                min: -1.0,
                max: 1.0,
                label: None,
            },
        ];
        draw(
            &mut m,
            lane(),
            &View::full(400),
            Some("drums"),
            &clips,
            false,
        );
        assert!(!m.is_empty(), "the header, lane and clips draw");
    }

    #[test]
    fn a_loaded_take_draws_decimated_through_its_pyramid() {
        // A "long" take: many more samples than the clip has pixels. The body
        // must cost pixels, not samples — it is read through the peak pyramid.
        let samples: Vec<f32> = (0..100_000)
            .map(|i| (i as f32 * 0.01).sin())
            .collect::<Vec<_>>();
        let data = Arc::new(WaveformData::new(samples.into(), 256));
        let clip = ClipDraw {
            id: 1,
            offset: 0.0,
            dur: 400.0,
            samples: Arc::from([] as [f32; 0]),
            data: Some(Arc::clone(&data)),
            notes: Vec::new(),
            min: -1.0,
            max: 1.0,
            label: Some("take".into()),
        };
        let mut m = Mesh::new();
        draw(
            &mut m,
            lane(),
            &View::full(400),
            None,
            std::slice::from_ref(&clip),
            false,
        );
        assert!(!m.is_empty(), "the take draws a body");
        // One min/max column per pixel of the clip rect, not one per sample: the
        // 100k-sample take costs the same as the lane is wide.
        let body = lane_body(lane(), false);
        let (x0, x1) = clip_x_range(body, &View::full(400), 0.0, 400.0).unwrap();
        let cols = (x1 - x0) as usize;
        let mut bare = Mesh::new();
        draw(
            &mut bare,
            lane(),
            &View::full(400),
            None,
            &[ClipDraw {
                data: None,
                ..clip.clone()
            }],
            false,
        );
        let per_line = 6u32; // two triangles per column line
        let added = m.vertex_count() - bare.vertex_count();
        assert!(
            added <= (cols as u32 + 2) * per_line,
            "the body is decimated to the clip's pixel width ({added} vertices for {cols} columns)"
        );
    }

    #[test]
    fn a_piano_roll_clip_draws_its_notes() {
        let clip = ClipDraw {
            id: 1,
            offset: 0.0,
            dur: 400.0,
            samples: Arc::from([] as [f32; 0]),
            data: None,
            notes: vec![
                Note {
                    start: 0.0,
                    dur: 100.0,
                    pitch: 60.0,
                },
                Note {
                    start: 100.0,
                    dur: 100.0,
                    pitch: 67.0,
                },
            ],
            min: 48.0, // pitch range low
            max: 72.0, // pitch range high
            label: Some("theme".into()),
        };
        // With notes, the clip draws a piano-roll (not a waveform body).
        let mut with = Mesh::new();
        draw(
            &mut with,
            lane(),
            &View::full(400),
            None,
            std::slice::from_ref(&clip),
            false,
        );
        // The same clip with no notes and no samples: only the clip frame.
        let mut without = Mesh::new();
        let bare = ClipDraw {
            notes: Vec::new(),
            ..clip
        };
        draw(
            &mut without,
            lane(),
            &View::full(400),
            None,
            std::slice::from_ref(&bare),
            false,
        );
        assert!(
            with.vertex_count() > without.vertex_count(),
            "the notes add geometry over the bare clip"
        );
    }
}
