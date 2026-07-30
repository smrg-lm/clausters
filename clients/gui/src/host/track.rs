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

use super::bpf::{self, BpfPoint};
use super::font;
use super::layout::Rect;
use super::meters::fraction;
use super::metrics::Metrics;
use super::paint::Mesh;
use super::pianoroll;
use super::theme::Theme;
use super::widget::{Widget, WidgetKind};
use crate::viewport::View;
use crate::waveform::WaveformData;

/// A piano-roll note. Re-exported from [`super::pianoroll`], the module that
/// owns the note model and the drawing/hit-test primitives — a clip's roll and
/// the dedicated `pianoroll` view share the one type so they never disagree on
/// geometry.
pub use super::pianoroll::Note;

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
    /// An automation clip's break-points (times relative to the clip): the curve
    /// body, which wins over the notes and the waveform.
    pub points: Vec<BpfPoint>,
    pub exp: bool,
    /// The curve body's own value range — a layered clip's bodies do not share an
    /// axis (a roll's `min`/`max` are pitches, a curve's are its parameter's).
    pub points_min: f32,
    pub points_max: f32,
    pub min: f32,
    pub max: f32,
    pub label: Option<String>,
}

/// The span of a widget subtree in timeline units: the longest clip end
/// (`offset + dur`) under it. A lane's extent (the "data" a lane registers with
/// its navigation group) and, over a whole window, its full time axis. `0.0`
/// when there are no clips.
pub fn clips_span(tree: &Widget) -> f64 {
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

/// The full-span navigation window of a window's tracks — the fallback for a
/// lane that is in no navigation group yet (the same defensive role
/// `frame::nav_for` plays for a timeline view). The live axis is the group's:
/// the lanes of a window share one, so they zoom and pan as one.
pub fn window_nav(tree: &Widget) -> View {
    View::full(clips_span(tree).ceil().max(1.0) as usize)
}

/// The lane body of a track's `rect`: the part right of the header strip, and
/// above the time-ruler strip when the lane draws one (`ruler`). The renderer
/// and the hit-test both call this, so a clip occupies the same pixels either
/// way — pass the same flag (a lane with `Ruler::Off` reserves no strip, which
/// is the un-rulered default).
pub fn lane_body(rect: Rect, ruler: bool, m: &Metrics) -> Rect {
    let hw = m.header_w.min(rect.w);
    let rh = if ruler { m.ruler_h.min(rect.h) } else { 0.0 };
    Rect::new(
        rect.x + hw,
        rect.y,
        (rect.w - hw).max(0.0),
        (rect.h - rh).max(0.0),
    )
}

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

/// One clip's rectangle inside the lane `body`, given the x range its span
/// occupies (`clip_x_range`) — the renderer and the hit-test both call it, so a
/// clip's body is edited on the pixels it is drawn on.
pub fn clip_rect(body: Rect, x0: f32, x1: f32) -> Rect {
    Rect::new(x0, body.y + 1.0, x1 - x0, (body.h - 2.0).max(0.0))
}

/// A `clip` widget copied out of the tree for drawing or hit-testing (`None` for
/// anything else) — the one place the typed tree becomes a [`ClipDraw`], so the
/// renderer and the interaction can never disagree about what a clip holds.
pub fn clip_draw(widget: &Widget) -> Option<ClipDraw> {
    match &widget.kind {
        WidgetKind::Clip {
            offset,
            dur,
            samples,
            body,
            notes,
            points,
            exp,
            points_min,
            points_max,
            min,
            max,
            label,
            ..
        } => Some(ClipDraw {
            id: widget.id.unwrap_or(-1),
            offset: *offset,
            dur: *dur,
            samples: Arc::clone(samples),
            data: body.clone(),
            notes: notes.clone(),
            points: points.clone(),
            exp: *exp,
            points_min: *points_min,
            points_max: *points_max,
            min: *min,
            max: *max,
            label: label.clone(),
        }),
        _ => None,
    }
}

/// Draws one track lane into `rect`: the header (with `label`), the lane field,
/// and every clip as a framed rectangle (its body decimated inside) through the
/// shared timeline `nav`. `ruler` reserves the bottom strip for the time ruler
/// (drawn by the frame renderer, which owns the tick math); the playhead is an
/// overlay, over the clips.
#[allow(clippy::too_many_arguments)] // one lane's draw: its clips, box, look
pub fn draw(
    mesh: &mut Mesh,
    rect: Rect,
    nav: &View,
    label: Option<&str>,
    clips: &[ClipDraw],
    ruler: bool,
    m: &Metrics,
    theme: &Theme,
) {
    // Header strip on the left, naming the track.
    let header = Rect::new(rect.x, rect.y, m.header_w.min(rect.w), rect.h);
    mesh.rect(header, theme.header);
    if let Some(t) = label {
        font::text(
            mesh,
            t,
            header.x + m.pad,
            rect.y + m.pad,
            m.text_scale,
            theme.text,
        );
    }
    let body = lane_body(rect, ruler, m);
    if body.w <= 0.0 || body.h <= 0.0 {
        return;
    }
    mesh.rect(body, theme.lane);
    mesh.border(body, m.divider_w, theme.frame);
    for clip in clips {
        let Some((x0, x1)) = clip_x_range(body, nav, clip.offset, clip.dur) else {
            continue;
        };
        let cr = clip_rect(body, x0, x1);
        mesh.rect(cr, theme.object_fill);
        mesh.border(cr, m.divider_w, theme.object_edge);
        // The bodies **layer**, back to front: the take, the events over it, the
        // envelope over both — an automation drawn on top of the material it
        // shapes is one clip, not two, and each body keeps its own value axis.
        match &clip.data {
            // A loaded take (mapped file, peak cache or fetched buffer): decimated
            // through its pyramid, so a minutes-long clip costs the same as a
            // short one.
            Some(data) => draw_take_body(mesh, cr, body, nav, clip, data, m, theme),
            None => draw_clip_body(mesh, cr, body, nav, clip, m, theme),
        }
        if !clip.notes.is_empty() {
            // Notes placed on the same shared axis (so the whole roll moves when
            // the clip does), pitch mapped over [min, max].
            draw_piano_roll(mesh, cr, body, nav, clip, m, theme);
        }
        if !clip.points.is_empty() {
            draw_curve(mesh, cr, body, nav, clip, m, theme);
        }
        if let Some(t) = &clip.label {
            font::text(
                mesh,
                t,
                cr.x + m.pad,
                cr.y + m.pad,
                m.caption_scale,
                theme.text,
            );
        }
    }
}

/// The clip-relative time (in timeline units) an x pixel falls on: the inverse
/// of the shared axis mapping. The curve body's editing runs through this, so a
/// point lands where the pointer is under any zoom.
pub fn curve_time_at(body: Rect, nav: &View, clip_offset: f64, cx: f64) -> f64 {
    let sample = nav.start + nav.len * ((cx - body.x as f64) / body.w.max(1.0) as f64);
    (sample - clip_offset).max(0.0)
}

/// The value a y pixel falls on inside the clip rect `cr`, over `[min, max]`
/// (honouring the exponential display scale) — the vertical inverse.
pub fn curve_value_at(cr: Rect, min: f32, max: f32, exp: bool, cy: f64) -> f32 {
    let frac = 1.0 - ((cy - cr.y as f64) / cr.h.max(1.0) as f64).clamp(0.0, 1.0);
    bpf::fraction_to_value(frac as f32, min, max, exp)
}

/// The index of the breakpoint under `(cx, cy)` in an automation clip, within a
/// pixel radius — the clip-placed twin of `bpf::hit_point` (a point is placed on
/// the *shared* axis here, not on a widget-local one).
pub fn curve_hit(
    clip: &ClipDraw,
    cr: Rect,
    body: Rect,
    nav: &View,
    cx: f64,
    cy: f64,
    m: &Metrics,
) -> Option<usize> {
    // The `bpf` view's grab radius, so a point is grabbed the same way wherever
    // it is drawn.
    let radius = (m.point_radius + m.hit_slop).max(6.0) as f64;
    let mut best: Option<(usize, f64)> = None;
    for (i, p) in clip.points.iter().enumerate() {
        let x = to_x(clip.offset + p.time, nav, body);
        let y = curve_y(cr, p.value, clip.points_min, clip.points_max, clip.exp) as f64;
        let d = ((cx - x).powi(2) + (cy - y).powi(2)).sqrt();
        if d <= radius && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// A curve value's y pixel inside the clip rect.
fn curve_y(cr: Rect, value: f32, min: f32, max: f32, exp: bool) -> f32 {
    cr.y + cr.h * (1.0 - bpf::value_fraction(value, min, max, exp))
}

/// Draws an automation clip's break-point curve inside `cr`: one column per
/// pixel of the *visible* clip rect, each evaluated through the same envelope
/// shape math the server's `EnvGen` plays (`bpf::value_at`) — so what is drawn
/// is what is heard — plus a disc per breakpoint. Times map through the shared
/// `nav`, exactly as the piano-roll's notes do, so the whole curve moves with
/// the clip and stays put under zoom.
fn draw_curve(
    mesh: &mut Mesh,
    cr: Rect,
    body: Rect,
    nav: &View,
    clip: &ClipDraw,
    m: &Metrics,
    theme: &Theme,
) {
    if cr.w < 1.0 || cr.h <= 0.0 {
        return;
    }
    let columns = cr.w.max(1.0) as usize;
    let y_at = |v: f32| curve_y(cr, v, clip.points_min, clip.points_max, clip.exp);
    let time_at = |x: f32| curve_time_at(body, nav, clip.offset, x as f64);
    let mut prev = [cr.x, y_at(bpf::value_at(&clip.points, time_at(cr.x)))];
    for c in 1..=columns {
        let x = cr.x + c as f32;
        let p = [x, y_at(bpf::value_at(&clip.points, time_at(x)))];
        mesh.line(prev, p, m.trace_w, theme.trace);
        prev = p;
    }
    for p in &clip.points {
        let x = to_x(clip.offset + p.time, nav, body) as f32;
        if x >= cr.x && x <= cr.x + cr.w {
            mesh.disc(x, y_at(p.value), m.point_radius, theme.point);
        }
    }
}

/// Draws a clip's notes as a compact piano-roll inside `cr`: the clip body is
/// the grid, its `[min, max]` the pitch window, and the notes ride the shared
/// `nav` time axis (so the whole roll moves when the clip does). The geometry is
/// the shared [`super::pianoroll::draw_notes`] primitive — the same one the
/// dedicated `pianoroll` view draws with, so a clip's roll and the editor never
/// disagree. The clip body uses only that one layer (no keyboard/lanes).
fn draw_piano_roll(
    mesh: &mut Mesh,
    cr: Rect,
    body: Rect,
    nav: &View,
    clip: &ClipDraw,
    m: &Metrics,
    theme: &Theme,
) {
    // The compact pitch ruler: each C named at the clip's left edge (there is
    // no keyboard gutter here), only when the rows are tall enough to read.
    pianoroll::draw_pitch_labels(mesh, cr, clip.min, clip.max, m, theme);
    // Notes map their x through the lane `body` (the shared nav's pixel domain,
    // like `draw_curve`) and clamp to the clip rect `cr`, so they stay pinned to
    // the clip under a pan/zoom instead of rescaling by the clip's own width.
    pianoroll::draw_notes(
        mesh,
        body,
        cr,
        nav,
        clip.offset,
        &clip.notes,
        clip.min,
        clip.max,
        false,
        &[],
        m,
        theme,
    );
}

/// The **source** sample position an x pixel of a clip's body falls on: the
/// pixel maps back through the shared axis to a timeline position, and that
/// through the clip's span to a fraction of its data. This is the whole reason a
/// waveform body scrolls and stretches *with* the view instead of squashing into
/// whatever slice of the clip is on screen: it is drawn from the source, per
/// visible pixel, exactly as the piano-roll and the curve are.
pub fn clip_source_at(body: Rect, nav: &View, clip: &ClipDraw, total: f64, x: f32) -> f64 {
    if clip.dur <= 0.0 {
        return 0.0;
    }
    let sample = nav.start + nav.len * ((x - body.x) as f64 / body.w.max(1.0) as f64);
    ((sample - clip.offset) / clip.dur * total).clamp(0.0, total)
}

/// Draws a waveform body inside the *visible* part of a clip (`cr`), reading the
/// source through `column` (a min/max over a sample span) and `at` (one sample) —
/// the two accessors a loaded take and an inline body both answer. One column per
/// visible pixel, each mapped back to the source through the shared axis, so the
/// body honours zoom and pan; when a column spans less than a couple of samples it
/// draws the polyline instead (the zoomed-in regime). Never resolves finer than
/// the screen — the one graphics rule.
// mesh + target/body rects + nav + clip + total span + the two source accessors:
// all distinct inputs to one wave-drawing pass, clearer flat than bundled.
#[allow(clippy::too_many_arguments)]
fn draw_wave_body(
    mesh: &mut Mesh,
    cr: Rect,
    body: Rect,
    nav: &View,
    clip: &ClipDraw,
    total: f64,
    column: impl Fn(f64, f64, f64) -> (f32, f32),
    at: impl Fn(f64) -> f32,
    m: &Metrics,
    theme: &Theme,
) {
    if total < 2.0 || cr.w < 1.0 || cr.h <= 0.0 {
        return;
    }
    let y_at = |v: f32| cr.y + cr.h * (1.0 - fraction(v, clip.min, clip.max));
    if clip.min < 0.0 && clip.max > 0.0 {
        let y = y_at(0.0);
        mesh.line([cr.x, y], [cr.x + cr.w, y], m.divider_w, theme.baseline);
    }
    let src = |x: f32| clip_source_at(body, nav, clip, total, x);
    let cols = cr.w.max(1.0) as usize;
    let per_px = (src(cr.x + 1.0) - src(cr.x)).max(0.0);
    if per_px >= 2.0 {
        // Zoomed out: a min/max column per pixel, read at the level the pyramid
        // (or the slice) can answer cheaply.
        for c in 0..cols {
            let x = cr.x + c as f32;
            let (s0, s1) = (src(x), src(x + 1.0));
            let (lo, hi) = column(s0, s1, per_px);
            mesh.line(
                [x + 0.5, y_at(hi)],
                [x + 0.5, y_at(lo)],
                m.divider_w,
                theme.selection,
            );
        }
    } else {
        // Zoomed in: fewer than a couple of samples per pixel — draw the trace.
        let mut prev = [cr.x, y_at(at(src(cr.x)))];
        for c in 1..=cols {
            let x = cr.x + c as f32;
            let p = [x, y_at(at(src(x)))];
            mesh.line(prev, p, m.divider_w, theme.selection);
            prev = p;
        }
    }
}

/// A **loaded take** as the clip's body: read through the take's peak pyramid
/// (`clausters_core::peaks`, via [`WaveformData::column`] — the same LOD source
/// and crossfade the heavy waveform view draws from), so a minutes-long file
/// costs a screen's worth of columns, never its samples.
#[allow(clippy::too_many_arguments)] // one body's draw: its rects, source, look
fn draw_take_body(
    mesh: &mut Mesh,
    cr: Rect,
    body: Rect,
    nav: &View,
    clip: &ClipDraw,
    data: &WaveformData,
    m: &Metrics,
    theme: &Theme,
) {
    let total = data.total_samples() as f64;
    draw_wave_body(
        mesh,
        cr,
        body,
        nav,
        clip,
        total,
        |s0, s1, per_px| data.column(0, per_px, s0, s1),
        |s| data.column(0, 1.0, s, s + 1.0).0,
        m,
        theme,
    );
}

/// A clip's **inline** body (a short sketch sent with the def), read straight off
/// the sample slice — the same drawing, a cheaper source.
fn draw_clip_body(
    mesh: &mut Mesh,
    cr: Rect,
    body: Rect,
    nav: &View,
    clip: &ClipDraw,
    m: &Metrics,
    theme: &Theme,
) {
    let samples = &clip.samples;
    let n = samples.len();
    draw_wave_body(
        mesh,
        cr,
        body,
        nav,
        clip,
        n as f64,
        |s0, s1, _per_px| {
            // The last column can land exactly on the end: keep the span
            // non-empty and inside the slice (a `clamp` with min > max panics).
            let a = (s0.floor().max(0.0) as usize).min(n.saturating_sub(1));
            let b = (s1.ceil() as usize).clamp(a + 1, n);
            let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
            for &v in &samples[a..b] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            (lo, hi)
        },
        |s| samples[(s.round().max(0.0) as usize).min(n.saturating_sub(1))],
        m,
        theme,
    );
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
        let body = lane_body(lane(), false, &Metrics::default());
        assert_eq!(
            (body.x, body.w),
            (
                Metrics::default().header_w,
                500.0 - Metrics::default().header_w
            )
        );
        assert_eq!(
            body.h,
            lane().h,
            "no ruler, no strip: the lane is full height"
        );
    }

    #[test]
    fn lane_body_reserves_the_ruler_strip_when_the_lane_has_one() {
        let ruled = lane_body(lane(), true, &Metrics::default());
        assert_eq!(ruled.h, lane().h - Metrics::default().ruler_h);
        // The header is unaffected: the strip comes off the bottom.
        assert_eq!(
            (ruled.x, ruled.w),
            (
                Metrics::default().header_w,
                500.0 - Metrics::default().header_w
            )
        );
    }

    #[test]
    fn playhead_x_places_the_clock_on_the_shared_axis() {
        let body = lane_body(lane(), false, &Metrics::default());
        let nav = View::full(400);
        // Halfway through the timeline: halfway across the lane body.
        let x = playhead_x(body, &nav, 200.0).unwrap();
        assert!((x - (body.x + body.w * 0.5)).abs() < 0.5);
        // Past the end of the window: nothing to draw.
        assert!(playhead_x(body, &nav, 500.0).is_none());
    }

    #[test]
    fn clip_x_range_places_the_clip_by_offset_and_duration() {
        let body = lane_body(lane(), false, &Metrics::default());
        let nav = View::full(400); // 1 sample per pixel over the 404-wide body-ish
        // A clip at [100, 200): starts a quarter in, one-quarter wide.
        let (x0, x1) = clip_x_range(body, &nav, 100.0, 100.0).unwrap();
        let px_per = body.w as f64 / 400.0;
        assert!((x0 as f64 - (body.x as f64 + 100.0 * px_per)).abs() < 0.5);
        assert!((x1 as f64 - (body.x as f64 + 200.0 * px_per)).abs() < 0.5);
    }

    #[test]
    fn clip_x_range_clips_to_the_body_and_drops_the_invisible() {
        let body = lane_body(lane(), false, &Metrics::default());
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
                points: Vec::new(),
                exp: false,
                points_min: -1.0,
                points_max: 1.0,
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
                points: Vec::new(),
                exp: false,
                points_min: -1.0,
                points_max: 1.0,
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
            &Metrics::default(),
            &Theme::default(),
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
            points: Vec::new(),
            exp: false,
            points_min: -1.0,
            points_max: 1.0,
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
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(!m.is_empty(), "the take draws a body");
        // One min/max column per pixel of the clip rect, not one per sample: the
        // 100k-sample take costs the same as the lane is wide.
        let body = lane_body(lane(), false, &Metrics::default());
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
            &Metrics::default(),
            &Theme::default(),
        );
        let per_line = 6u32; // two triangles per column line
        let added = m.vertex_count() - bare.vertex_count();
        assert!(
            added <= (cols as u32 + 2) * per_line,
            "the body is decimated to the clip's pixel width ({added} vertices for {cols} columns)"
        );
    }

    fn curve_clip(points: Vec<BpfPoint>) -> ClipDraw {
        ClipDraw {
            id: 1,
            offset: 0.0,
            dur: 400.0,
            samples: Arc::from([] as [f32; 0]),
            data: None,
            notes: Vec::new(),
            points,
            exp: false,
            points_min: 0.0,
            points_max: 1.0,
            min: 0.0,
            max: 1.0,
            label: Some("cutoff".into()),
        }
    }

    fn pt(time: f64, value: f32) -> BpfPoint {
        BpfPoint {
            time,
            value,
            shape: 1,
            curve: 0.0,
        }
    }

    #[test]
    fn an_automation_clip_draws_its_curve_instead_of_a_body() {
        let clip = curve_clip(vec![pt(0.0, 0.0), pt(200.0, 1.0), pt(400.0, 0.0)]);
        let mut with = Mesh::new();
        draw(
            &mut with,
            lane(),
            &View::full(400),
            None,
            std::slice::from_ref(&clip),
            false,
            &Metrics::default(),
            &Theme::default(),
        );
        let mut bare = Mesh::new();
        draw(
            &mut bare,
            lane(),
            &View::full(400),
            None,
            &[ClipDraw {
                points: Vec::new(),
                ..clip.clone()
            }],
            false,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(
            with.vertex_count() > bare.vertex_count(),
            "the curve and its breakpoints add geometry over the bare clip"
        );
    }

    #[test]
    fn a_curve_point_is_hit_where_it_is_drawn_and_maps_back_through_the_axis() {
        let body = lane_body(lane(), false, &Metrics::default());
        let nav = View::full(400);
        // The clip sits at 100 on the axis, so its point at t=100 is at 200.
        let clip = ClipDraw {
            offset: 100.0,
            dur: 200.0,
            ..curve_clip(vec![pt(0.0, 0.0), pt(100.0, 1.0), pt(200.0, 0.0)])
        };
        let (x0, x1) = clip_x_range(body, &nav, clip.offset, clip.dur).unwrap();
        let cr = clip_rect(body, x0, x1);

        // The peak point (t=100, value=1 -> the top of the clip).
        let px = to_x(200.0, &nav, body);
        let py = cr.y as f64;
        assert_eq!(
            curve_hit(&clip, cr, body, &nav, px, py, &Metrics::default()),
            Some(1)
        );
        // Away from any point: nothing (so the clip still moves by its body).
        assert_eq!(
            curve_hit(
                &clip,
                cr,
                body,
                &nav,
                px + 40.0,
                py + 20.0,
                &Metrics::default()
            ),
            None
        );

        // The inverse mapping an edit uses: pixels -> clip-relative time, value.
        assert!((curve_time_at(body, &nav, clip.offset, px) - 100.0).abs() < 1.0);
        assert!((curve_value_at(cr, 0.0, 1.0, false, py) - 1.0).abs() < 0.05);
    }

    #[test]
    fn a_body_reads_the_source_through_the_axis_under_zoom_and_pan() {
        // The bug this pins: a partially visible clip must draw the *part of its
        // take that is on screen*, not squash the whole take into the visible
        // sliver — so a pixel maps back through the axis to the source.
        let body = lane_body(lane(), false, &Metrics::default());
        let clip = ClipDraw {
            offset: 0.0,
            dur: 400.0,
            ..curve_clip(Vec::new()) // a plain clip: no curve, no notes
        };
        let total = 1000.0;

        // Fully zoomed out: the clip's ends map to the take's ends.
        let full = View::full(400);
        let (x0, x1) = clip_x_range(body, &full, clip.offset, clip.dur).unwrap();
        assert!(clip_source_at(body, &full, &clip, total, x0) < 1.0);
        assert!(clip_source_at(body, &full, &clip, total, x1) > total - 1.0);

        // Zoomed into the clip's second half: the lane's left edge is now the
        // middle of the take, and the visible span is the half after it.
        let zoomed = View {
            start: 200.0,
            len: 200.0,
        };
        let left = clip_source_at(body, &zoomed, &clip, total, body.x);
        let right = clip_source_at(body, &zoomed, &clip, total, body.x + body.w);
        assert!(
            (left - 500.0).abs() < 5.0,
            "the left edge is mid-take, not 0"
        );
        assert!((right - total).abs() < 5.0);
    }

    #[test]
    fn a_clip_layers_its_bodies_and_the_curve_keeps_its_own_axis() {
        // An envelope drawn over the event it shapes is *one* clip: both bodies
        // draw, and they do not share a value axis (notes are pitches, the curve
        // is its parameter's units).
        let notes = vec![Note::new(0.0, 200.0, 60.0)];
        let layered = ClipDraw {
            notes: notes.clone(),
            points: vec![pt(0.0, 200.0), pt(400.0, 900.0)],
            points_min: 150.0,
            points_max: 1000.0,
            min: 48.0,
            max: 72.0,
            ..curve_clip(Vec::new())
        };
        let mut both = Mesh::new();
        draw(
            &mut both,
            lane(),
            &View::full(400),
            None,
            std::slice::from_ref(&layered),
            false,
            &Metrics::default(),
            &Theme::default(),
        );
        let mut roll_only = Mesh::new();
        draw(
            &mut roll_only,
            lane(),
            &View::full(400),
            None,
            &[ClipDraw {
                points: Vec::new(),
                ..layered.clone()
            }],
            false,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(
            both.vertex_count() > roll_only.vertex_count(),
            "the curve draws over the notes, it does not replace them"
        );

        // The curve's points sit on the curve's range: its 200 Hz start is near the
        // bottom of the clip, not off the pitch axis.
        let body = lane_body(lane(), false, &Metrics::default());
        let nav = View::full(400);
        let (x0, x1) = clip_x_range(body, &nav, layered.offset, layered.dur).unwrap();
        let cr = clip_rect(body, x0, x1);
        let y = curve_y(cr, 200.0, layered.points_min, layered.points_max, false);
        assert!(y > cr.y + cr.h * 0.8, "200 Hz over [150, 1000] reads low");
    }

    #[test]
    fn a_piano_roll_clip_draws_its_notes() {
        let clip = ClipDraw {
            id: 1,
            offset: 0.0,
            dur: 400.0,
            samples: Arc::from([] as [f32; 0]),
            data: None,
            notes: vec![Note::new(0.0, 100.0, 60.0), Note::new(100.0, 100.0, 67.0)],
            points: Vec::new(),
            exp: false,
            points_min: -1.0,
            points_max: 1.0,
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
            &Metrics::default(),
            &Theme::default(),
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
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(
            with.vertex_count() > without.vertex_count(),
            "the notes add geometry over the bare clip"
        );
    }
}
