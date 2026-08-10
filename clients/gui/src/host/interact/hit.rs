//! **What is under a point**: the one layout pass that answers a pointer
//! question, and the per-element hit-tests that read the answer finer.
//!
//! [`hit`] lays the window out once and hands back the deepest interactive
//! widget plus the [`Frame`] chain of containers over it — the containment the
//! layout already resolved ([`chain_of`]), each container's coordinate system
//! resolved with it ([`time_axis`], [`view_of`]). Everything else here is the
//! second question a gesture asks once it knows *which* element it hit: which
//! part of a clip ([`clip_hit`]), which header control ([`header_hit`]), which
//! note or region of a piano-roll ([`pianoroll_hit`]), which caret offset in a
//! text field ([`text_caret_at`]).
//!
//! The rule that keeps these honest is that they reconstruct **the geometry the
//! renderer drew through**, never a parallel derivation of it: a note is grabbed
//! by the pixels it was drawn on.

use super::super::layout::{self, Rect};
use super::super::widget::WidgetKind;
use super::super::widget::element::BodyRole;
use super::super::{Host, controls, pianoroll, track};
use super::coords::{Coords, Frame, Hit, TimeAxis, YAxis, clip_part};
use super::{ClipPart, HeaderPart};
use crate::viewport::View;

/// The [`Hit`] under `(x, y)` in window `def_id`. Containers (`window`/`panel`)
/// are not hit targets — except `scroll`, whose empty area is the pan gesture's
/// surface (its children, laid out through its view transform, still win over
/// it). A widget scrolled out of its container's window (outside its clip) is
/// not hit. `fb_w`/`fb_h` is the window's framebuffer size in device pixels.
///
/// `lanes` answers how many channel lanes a stacked heavy view draws — the one
/// datum the host tree does not hold (it lives in the front's GPU slots), and
/// the divisor a vertical axis is panned through.
///
/// [`Placed::scale`]: super::super::layout::Placed::scale
pub(crate) fn hit(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    x: f64,
    y: f64,
    lanes: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Option<Hit> {
    let placed = host.layout_window(def_id, fb_w, fb_h)?;
    let mut found = None;
    for (i, p) in placed.iter().enumerate() {
        if p.rect.contains(x, y)
            && p.clip.is_none_or(|c| c.contains(x, y))
            && p.widget.id.is_some()
            && !matches!(
                p.widget.kind,
                WidgetKind::Window { .. } | WidgetKind::Panel { .. } | WidgetKind::Stack { .. }
            )
        {
            found = Some(i);
        }
    }
    let i = found?;
    let p = placed[i];
    Some(Hit {
        id: p.widget.id?,
        rect: p.rect,
        scale: p.scale,
        kind: p.widget.kind.clone(),
        chain: chain_of(host, def_id, &placed, i, lanes),
    })
}

/// The window's **one** navigation group, when it has exactly one: a member's
/// id and axis (they share the window and the gutter, so any member answers for
/// all of them), plus every **lane** on it.
///
/// It is what a gesture falls back to when the pointer is not over a timeline
/// at all — the gap between two lanes, the slack under the last one, a
/// container's margin. In a window built around one axis those pixels are not a
/// third thing the user meant: they are the axis with nothing drawn on them.
/// With two groups there is no such answer, so there is no fallback either.
pub(crate) struct SoleAxis {
    pub id: i32,
    pub axis: TimeAxis,
    pub lanes: Vec<i32>,
}

pub(crate) fn sole_time_axis(
    host: &Host,
    def_id: i32,
    fb_w: u32,
    fb_h: u32,
    lanes: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Option<SoleAxis> {
    let placed = host.layout_window(def_id, fb_w, fb_h)?;
    let mut key = None;
    let mut found: Option<(i32, TimeAxis)> = None;
    let mut lane_ids = Vec::new();
    for p in &placed {
        let (Some(id), true) = (p.widget.id, p.widget.is_timeline()) else {
            continue;
        };
        let Some(editor) = p.widget.kind.editor() else {
            continue;
        };
        let this = super::super::timeline::group_key(id, editor.link);
        match key {
            Some(k) if k != this => return None, // two axes: nothing to fall back to
            Some(_) => {}
            None => key = Some(this),
        }
        if matches!(p.widget.kind, WidgetKind::Track { .. }) {
            lane_ids.push(id);
        }
        if found.is_none() {
            found = time_axis(host, def_id, p, p.indent, lanes).map(|axis| (id, axis));
        }
    }
    let (id, axis) = found?;
    Some(SoleAxis {
        id,
        axis,
        lanes: lane_ids,
    })
}

/// The containers from the window down to `i`, `i` itself included when it is
/// one — walked back through [`super::super::layout::Placed::parent`], which is the containment the
/// layout pass already resolved.
fn chain_of(
    host: &Host,
    def_id: i32,
    placed: &[layout::Placed],
    i: usize,
    lanes: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Vec<Frame> {
    let mut chain = Vec::new();
    let mut at = Some(i);
    while let Some(j) = at {
        let p = placed[j];
        let coords = match &p.widget.kind {
            WidgetKind::Window { .. } | WidgetKind::Panel { .. } | WidgetKind::Stack { .. } => {
                Some(Coords::Layout)
            }
            WidgetKind::Scroll { view, .. } => Some(Coords::Plane(*view)),
            WidgetKind::Patch { .. } => Some(Coords::Canvas),
            // A clip carries the axis the layout gave it, which is the whole
            // point of the layout placing clips: the rectangle and the window
            // are one fact, resolved once, read by the renderer and by this.
            WidgetKind::Clip { .. } => p.time.map(|nav| {
                Coords::Local(TimeAxis {
                    body: p.rect,
                    nav,
                    y: None,
                })
            }),
            // Where the body begins is the **group's** call, not this widget's:
            // every member of one axis starts it at the same x, and the layout
            // already resolved it (`Placed::indent`).
            _ if p.widget.is_timeline() => {
                time_axis(host, def_id, &p, p.indent, lanes).map(Coords::Time)
            }
            _ => None,
        };
        if let Some(coords) = coords {
            chain.push(Frame {
                id: p.widget.id,
                rect: p.rect,
                scale: p.scale,
                map: p.widget.gesture_map(),
                coords,
            });
        }
        at = p.parent;
    }
    chain.reverse();
    chain
}

/// The [`TimeAxis`] of a placed timeline view — the geometry the renderer drew
/// through, resolved once here rather than by each gesture from the kind it
/// happens to have hit. The body is the strip samples map onto; the vertical
/// axis is the band left of it, when the view has one.
fn time_axis(
    host: &Host,
    def_id: i32,
    p: &layout::Placed,
    indent: f32,
    lanes: &dyn Fn(i32, &WidgetKind) -> usize,
) -> Option<TimeAxis> {
    let metrics = host.metrics_for(def_id);
    let ruler_on = p.widget.kind.editor()?.ruler != super::super::widget::Ruler::Off;
    // The body samples map onto, whether the axis has a vertical gesture
    // surface, and the vertical axis' own window when it measures something.
    let (body, y_surface, window) = match &p.widget.kind {
        WidgetKind::Track { .. } => (
            track::lane_body(p.rect, ruler_on, indent, metrics),
            false,
            None,
        ),
        WidgetKind::TimeRuler { .. } => (
            super::super::frame::ruler_strip_body(p.rect, indent),
            false,
            None,
        ),
        WidgetKind::PianoRoll {
            osc_lane,
            velocity_lane,
            min,
            max,
            editor,
            ..
        } => {
            let (lo, hi) = pitch_window(editor, *min, *max);
            (
                super::super::pianoroll::regions(
                    p.rect,
                    ruler_on,
                    *osc_lane,
                    *velocity_lane,
                    indent,
                    metrics,
                )
                .grid,
                // The keyboard gutter is the roll's vertical axis surface,
                // always drawn (there is no `ruler_y: off` for a piano-roll).
                true,
                // ...and pitch is a domain, so a sweep on this axis picks notes
                // by a rectangle rather than by a time span alone.
                Some((lo as f64, hi as f64)),
            )
        }
        kind => (
            super::super::frame::timeline_body(p.rect, kind.editor()?, indent, metrics),
            kind.editor()?.ruler_y != super::super::widget::RulerY::Off,
            None,
        ),
    };
    let (start, len) = p.widget.kind.editor()?.y_view();
    Some(TimeAxis {
        body,
        nav: view_of(host, def_id, p, body),
        y: y_surface.then(|| YAxis {
            // The whole band left of the body, full height: the strip is where
            // the axis is grabbed, and a press beside it at any height means the
            // same axis.
            strip: Rect::new(p.rect.x, p.rect.y, (body.x - p.rect.x).max(0.0), p.rect.h),
            start,
            len,
            lane_h: (body.h as f64
                / p.widget.id.map_or(1, |id| lanes(id, &p.widget.kind)).max(1) as f64)
                .max(1.0),
            window,
        }),
    })
}

/// The navigation window a placed timeline view is seen through: its group's,
/// or — while it is in none — the fallback its own contents imply, so a gesture
/// on an ungrouped view still measures against something the renderer agrees
/// with.
fn view_of(host: &Host, def_id: i32, p: &layout::Placed, body: Rect) -> View {
    if let Some((nav, _total)) = p.widget.id.and_then(|id| host.timeline_nav(id)) {
        return nav;
    }
    match &p.widget.kind {
        WidgetKind::Track { .. } => host
            .window_def(def_id)
            .map_or(View::full(1), track::window_nav),
        WidgetKind::PianoRoll { notes, osc, .. } => {
            View::full(pianoroll_span(notes, osc).ceil().max(1.0) as usize)
        }
        _ => View::full(body.w.max(1.0) as usize),
    }
}

/// The caret byte offset a click at `(cx, cy)` lands on in the `text` field
/// `widget_id` (its `rect` and workspace `scale` as the front hit-tested them).
/// `None` when the widget is gone or not a text field.
pub(crate) fn text_caret_at(
    host: &Host,
    def_id: i32,
    widget_id: i32,
    rect: Rect,
    scale: f32,
    cx: f64,
    cy: f64,
) -> Option<usize> {
    match host.widget_kind(def_id, widget_id)? {
        WidgetKind::Text {
            value,
            label,
            text_size,
            multiline,
            caret,
        } => Some(controls::caret_at(
            rect,
            value,
            label.is_some(),
            *text_size * scale,
            *multiline,
            *caret,
            cx,
            cy,
            // The placement's table, the one the field was drawn with.
            &host.metrics_for(def_id).at(scale),
        )),
        _ => None,
    }
}

/// A press on a lane's header: which control it landed on, and — for the fader
/// — the rectangle the drag maps its value through.
pub(crate) struct HeaderHit {
    pub part: HeaderPart,
    pub fader: Option<Rect>,
}

/// The header control under `(cx, cy)` on the placed lane `rect`, whose axis
/// begins at `body_x` (so the band beside it is the header). `None` when the
/// press is on the axis, or on the band's empty space — which names no sample
/// and no control, and so means nothing.
pub(crate) fn header_hit(
    host: &Host,
    def_id: i32,
    lane_id: i32,
    rect: Rect,
    body_x: f32,
    cx: f64,
    cy: f64,
) -> Option<HeaderHit> {
    let WidgetKind::Track { header, .. } = host.widget_kind(def_id, lane_id)? else {
        return None;
    };
    let band = super::super::timeline::gutter_band(rect, body_x - rect.x);
    let m = host.metrics_for(def_id);
    let part = track::header_hit(band, header, m, cx, cy)?;
    Some(HeaderHit {
        part,
        fader: track::header_parts(band, header, m).fader,
    })
}

/// A clip press: the clip id, its current placement (`offset`/`dur`), the lane
/// body and the shared navigation window the drag maps through (so the front
/// turns cursor pixels into timeline samples), and which part was hit.
pub(crate) struct ClipHit {
    pub id: i32,
    /// The lane the clip sits on. A clip is not itself a navigation-group
    /// member — the *lane* is — so anything that has to reach the shared axis
    /// (the drag's cursor mapping, the edge scroll) asks through this id.
    pub lane: i32,
    pub offset: f64,
    pub dur: f64,
    pub body: Rect,
    /// The clip's own rectangle (the body the curve of an automation clip is
    /// drawn in, so its point edits map onto the pixels drawn).
    pub rect: Rect,
    pub nav: View,
    /// The clip's **own** axis: the window of its `[0, dur]` span that `rect`
    /// shows. Every edit inside the clip (a break-point today, its child
    /// elements next) maps through `(rect, local)`; `body`/`nav` above are only
    /// what the clip's *placement* on the lane is dragged through.
    pub local: View,
    pub part: ClipPart,
    /// The break-point under the cursor on an automation clip: a point wins over
    /// the clip's body (as it wins over a segment in the `bpf` view), so the
    /// curve is edited in place while the clip still moves by its empty space.
    pub point: Option<usize>,
    /// Whether the clip carries a curve at all (an automation clip), which is
    /// what decides a Ctrl+press *adds* a point rather than doing nothing. The
    /// hit knows it — it looked for a point on that curve — so the press does
    /// not go back to the tree to ask.
    pub has_curve: bool,
}

/// The [`ClipHit`] of the `clip` the pointer landed on: the clip the layout
/// **placed** (its id and its rectangle, straight off the hit) read against the
/// lane's time axis, which is what the placement was computed from.
///
/// Nothing is re-derived here any more. The clip used to be found by walking
/// the lane's children and re-running `clip_x_range` on each, because a clip
/// was not a placed widget and there was nothing else to ask; now it is one, so
/// the topmost-wins rule is the layout's (later children are placed later, and
/// the hit takes the last match) and the rectangle is the one that was drawn.
/// Native-only, like the other edit-back gestures.
pub(crate) fn clip_hit(
    host: &Host,
    def_id: i32,
    lane: (i32, TimeAxis),
    clip: (i32, TimeAxis),
    x: f64,
    y: f64,
) -> Option<ClipHit> {
    let (lane_id, TimeAxis { body, nav, .. }) = lane;
    let (id, local) = clip;
    let rect = local.body;
    let widget = host.window_def(def_id)?.find(id)?;
    let WidgetKind::Clip { offset, dur, .. } = widget.kind else {
        return None;
    };
    // A break-point is grabbed on the clip's **own** axis, the one it was drawn
    // on — the lane's window below is only what the clip's placement drags in.
    let curve = match widget.clip_body(BodyRole::Curve) {
        Some(WidgetKind::Bpf {
            points,
            min,
            max,
            exp,
            ..
        }) if !points.is_empty() => Some((points, *min, *max, *exp)),
        _ => None,
    };
    let has_curve = curve.is_some();
    let point = curve.and_then(|(points, min, max, exp)| {
        track::curve_hit(
            points,
            rect,
            &local.nav,
            min,
            max,
            exp,
            x,
            y,
            host.metrics_for(def_id),
        )
    });
    Some(ClipHit {
        id,
        lane: lane_id,
        offset,
        dur,
        body,
        rect,
        nav,
        local: local.nav,
        part: clip_part(rect.x, rect.x + rect.w, x as f32),
        point,
        has_curve,
    })
}

// --- Piano-roll interaction (native gestures) -----------------------------

/// Which region of a `pianoroll` a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrRegion {
    Grid,
    Velocity,
    Osc,
}

/// A piano-roll press: the widget id, the reconstructed grid rect and shared
/// navigation window the drag maps through, the visible pitch window, the note
/// snap grid, which region was hit, and the note/OSC-marker under the cursor (if
/// any). The renderer's geometry is reconstructed here so a note is grabbed by
/// the pixels it is drawn on, exactly as `clip_hit` does for a clip.
pub(crate) struct PianoRollHit {
    pub grid: Rect,
    /// The rect of the region that was hit (the grid, the velocity lane or the
    /// OSC lane) — the velocity drag maps the cursor's height through it.
    pub region_rect: Rect,
    pub nav: View,
    pub lo: f32,
    pub hi: f32,
    pub snap: f64,
    pub region: PrRegion,
    pub note: Option<pianoroll::NoteHit>,
    pub osc_index: Option<usize>,
}

/// The pitch window `[lo, hi]` a piano-roll draws through — its `[min, max]`
/// axis sliced by the vertical display window (`y_start`/`y_len`), the same math
/// the renderer's `pitch_window` uses so the hit-test matches the pixels.
fn pitch_window(editor: &super::super::widget::EditorProps, min: f32, max: f32) -> (f32, f32) {
    let (y0, yl) = editor.y_view();
    let mut axis =
        crate::viewport::Axis::ranged(min as f64, max as f64, crate::viewport::Unit::Pitch);
    axis.slice_normalized(y0, yl);
    let (start, len) = axis.span();
    (start as f32, (start + len) as f32)
}

/// The content extent (samples) of a piano-roll's notes and OSC events — the
/// fallback navigation window when the widget is in no group yet.
fn pianoroll_span(notes: &[pianoroll::Note], osc: &[pianoroll::OscMark]) -> f64 {
    let mut span = 0.0f64;
    for n in notes {
        span = span.max(n.start + n.dur);
    }
    for m in osc {
        span = span.max(m.time);
    }
    span
}

/// Hit-test a press against the `pianoroll` `roll` — its id, the rectangle the
/// hit placed it at and the time axis the hit resolved — against the same
/// regions and navigation window the renderer drew. Native-only, the edit-back
/// gesture posture.
pub(crate) fn pianoroll_hit(
    host: &Host,
    def_id: i32,
    roll: (i32, Rect, TimeAxis),
    x: f64,
    y: f64,
) -> Option<PianoRollHit> {
    let (id, rect, axis) = roll;
    let WidgetKind::PianoRoll {
        notes,
        osc,
        min,
        max,
        snap,
        velocity_lane,
        osc_lane,
        editor,
        ..
    } = host.widget_kind(def_id, id)?
    else {
        return None;
    };
    let ruler_on = editor.ruler != super::super::widget::Ruler::Off;
    let r = pianoroll::regions(
        rect,
        ruler_on,
        *osc_lane,
        *velocity_lane,
        // The band the hit already resolved: a roll's keyboard fills its
        // group's indent, so the strip beside the grid *is* that indent.
        axis.y.map_or(0.0, |y| y.strip.w),
        host.metrics_for(def_id),
    );
    let nav = axis.nav;
    let (lo, hi) = pitch_window(editor, *min, *max);
    let (fx, fy) = (x as f32, y as f32);
    let (region, note, osc_index) = if *osc_lane && r.osc.contains(x, y) {
        (PrRegion::Osc, None, nearest_osc(r.osc, &nav, osc, fx))
    } else if *velocity_lane && r.velocity.contains(x, y) {
        // A velocity-lane press picks the note whose bar it is nearest; the
        // hit rides in `note` as a body hit so the caller reads its index.
        let picked = nearest_note(r.velocity, &nav, notes, fx).map(|index| pianoroll::NoteHit {
            index,
            part: pianoroll::NotePart::Body,
        });
        (PrRegion::Velocity, picked, None)
    } else {
        let note = pianoroll::note_hit(r.grid, &nav, 0.0, notes, lo, hi, fx, fy);
        (PrRegion::Grid, note, None)
    };
    let region_rect = match region {
        PrRegion::Grid => r.grid,
        PrRegion::Velocity => r.velocity,
        PrRegion::Osc => r.osc,
    };
    Some(PianoRollHit {
        grid: r.grid,
        region_rect,
        nav,
        lo,
        hi,
        snap: *snap,
        region,
        note,
        osc_index,
    })
}

/// The index of the note whose start is nearest the cursor x (within a small
/// pixel tolerance) — the velocity lane's bar picker.
fn nearest_note(lane: Rect, nav: &View, notes: &[pianoroll::Note], x: f32) -> Option<usize> {
    let to_x = |s: f64| lane.x + ((s - nav.start) / nav.len.max(1.0) * lane.w as f64) as f32;
    notes
        .iter()
        .enumerate()
        .map(|(i, n)| (i, (to_x(n.start) - x).abs()))
        .filter(|(_, d)| *d <= 5.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The index of the OSC marker whose time is nearest the cursor x.
fn nearest_osc(lane: Rect, nav: &View, marks: &[pianoroll::OscMark], x: f32) -> Option<usize> {
    let to_x = |s: f64| lane.x + ((s - nav.start) / nav.len.max(1.0) * lane.w as f64) as f32;
    marks
        .iter()
        .enumerate()
        .map(|(i, m)| (i, (to_x(m.time) - x).abs()))
        .filter(|(_, d)| *d <= 5.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}
