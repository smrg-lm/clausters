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
use super::signal::{
    self,
    trace::{Trace, TraceStyle},
};
use super::theme::Theme;
use super::timeline;
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

/// What a lane reserves **left of its axis**, and what it carries there.
///
/// A lane header used to be one number in the size table (`header_w`) holding
/// one string. It is a strip of controls: a name, the mute/solo pair, a level
/// fader — so its width follows what it carries, and a lane that carries more
/// says so. The parts are presence-driven: a lane that names no `mute` prop
/// offers no mute button, so a header stays exactly the name strip it was
/// unless a script asks for more.
///
/// `w` overrides the whole calculation, because an explicit size always wins
/// over a natural one (the layout's own rule) — and because the *shared* indent
/// of a navigation group is the widest wish on it
/// ([`super::timeline::group_indents`]), so one lane declaring a wide header
/// moves the axis for the roll and the ruler stacked with it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Header {
    /// The declared width in **logical** pixels; `None` sizes it naturally.
    pub w: Option<f32>,
    /// The mute state, when the lane offers the toggle.
    pub mute: Option<bool>,
    /// The solo state, when the lane offers the toggle.
    pub solo: Option<bool>,
    /// The fader's value over `[0, 1]`, when the lane offers one.
    pub level: Option<f32>,
}

impl Header {
    /// Whether the header carries anything below its name row.
    fn has_controls(&self) -> bool {
        self.mute.is_some() || self.solo.is_some() || self.level.is_some()
    }

    /// The width this header **wants**, in the coordinates of `m`: the size
    /// table's `header_w` for a name-only strip, widened to hold the control
    /// row when it carries one. A declared `w` replaces it outright.
    pub fn width(&self, m: &Metrics) -> f32 {
        if let Some(w) = self.w {
            return super::metrics::snap_px(w, m.ui_scale).max(0.0);
        }
        if !self.has_controls() {
            return m.header_w;
        }
        let toggles = [self.mute, self.solo]
            .iter()
            .filter(|t| t.is_some())
            .count();
        let row = toggles as f32 * (m.box_side + m.pad)
            + if self.level.is_some() {
                MIN_FADER_W + m.pad
            } else {
                0.0
            };
        m.header_w.max(row + 2.0 * m.pad)
    }
}

/// The narrowest a level fader is drawn at all: below this it is dropped rather
/// than shown as a stub nobody can aim at.
const MIN_FADER_W: f32 = 28.0;

/// A header's parts, laid out inside its band. A part is `None` when the lane
/// does not offer it **or** when the band is too small to draw it — a short
/// lane keeps its name and drops the controls, the way a natural size degrades
/// everywhere else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderParts {
    pub label: Rect,
    pub mute: Option<Rect>,
    pub solo: Option<Rect>,
    pub fader: Option<Rect>,
}

/// One of a header's interactive parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderPart {
    Mute,
    Solo,
    Fader,
}

/// Lays a header's parts out inside its `band`: the name on the top row, the
/// controls on a row under it. The renderer and the hit-test both call this, so
/// a button is pressed on the pixels it is drawn on.
pub fn header_parts(band: Rect, header: &Header, m: &Metrics) -> HeaderParts {
    let inner = Rect::new(
        band.x + m.pad,
        band.y + m.pad,
        (band.w - 2.0 * m.pad).max(0.0),
        (band.h - 2.0 * m.pad).max(0.0),
    );
    let name_h = font::height(m.text_scale);
    let label = Rect::new(inner.x, inner.y, inner.w, name_h.min(inner.h));
    let mut parts = HeaderParts {
        label,
        mute: None,
        solo: None,
        fader: None,
    };
    // The control row needs a row of its own under the name; a lane too short
    // for both keeps the name.
    let row_h = m.box_side.min(inner.h - name_h - m.pad);
    if !header.has_controls() || row_h < m.box_side * 0.5 {
        return parts;
    }
    let row_y = inner.y + name_h + m.pad;
    let mut x = inner.x;
    let right = inner.x + inner.w;
    let square = |x: &mut f32| {
        let r = Rect::new(*x, row_y, m.box_side, row_h);
        (*x + m.box_side + m.pad <= right + m.pad).then(|| {
            *x += m.box_side + m.pad;
            r
        })
    };
    if header.mute.is_some() {
        parts.mute = square(&mut x);
    }
    if header.solo.is_some() {
        parts.solo = square(&mut x);
    }
    if header.level.is_some() {
        let w = right - x;
        if w >= MIN_FADER_W {
            parts.fader = Some(Rect::new(x, row_y, w, row_h));
        }
    }
    parts
}

/// The header part under `(x, y)`, if any — the press's read of
/// [`header_parts`].
pub fn header_hit(band: Rect, header: &Header, m: &Metrics, x: f64, y: f64) -> Option<HeaderPart> {
    let parts = header_parts(band, header, m);
    let over = |r: Option<Rect>| r.is_some_and(|r| r.contains(x, y));
    if over(parts.mute) {
        Some(HeaderPart::Mute)
    } else if over(parts.solo) {
        Some(HeaderPart::Solo)
    } else if over(parts.fader) {
        Some(HeaderPart::Fader)
    } else {
        None
    }
}

/// The level an x pixel of the fader `rect` names, clamped to `[0, 1]`.
pub fn level_at(rect: Rect, x: f64) -> f32 {
    (((x - rect.x as f64) / rect.w.max(1.0) as f64) as f32).clamp(0.0, 1.0)
}

/// Draws a header's controls into `band` (the name is drawn by [`draw`], which
/// owns the ellipsis against the band it actually got).
fn draw_header_controls(mesh: &mut Mesh, band: Rect, header: &Header, m: &Metrics, theme: &Theme) {
    let parts = header_parts(band, header, m);
    let mut toggle = |rect: Option<Rect>, on: bool, letter: &str, lit: super::paint::Color| {
        let Some(r) = rect else { return };
        mesh.rect(r, theme.track);
        if on {
            let inset = r.h.min(r.w) * 0.22;
            mesh.rect(
                Rect::new(
                    r.x + inset,
                    r.y + inset,
                    r.w - 2.0 * inset,
                    r.h - 2.0 * inset,
                ),
                lit,
            );
        }
        font::text_centered(mesh, letter, r, m.caption_scale, theme.text);
    };
    toggle(parts.mute, header.mute == Some(true), "M", theme.warn);
    toggle(parts.solo, header.solo == Some(true), "S", theme.hilite);
    if let (Some(r), Some(level)) = (parts.fader, header.level) {
        mesh.rect(r, theme.track);
        let w = r.w * level.clamp(0.0, 1.0);
        if w > 0.0 {
            mesh.rect(Rect::new(r.x, r.y, w, r.h), theme.accent);
        }
        mesh.border(r, m.divider_w, theme.frame);
    }
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

/// The lane body of a track's `rect`: the part right of the header band, and
/// above the time-ruler strip when the lane draws one (`ruler`). The renderer
/// and the hit-test both call this, so a clip occupies the same pixels either
/// way — pass the same flag (a lane with `Ruler::Off` reserves no strip, which
/// is the un-rulered default).
///
/// `indent` is the **group's**, not the lane's own header width (see
/// [`super::timeline::group_indent`]): a lane sharing an axis with a roll or a
/// ruler starts its body where they all do.
pub fn lane_body(rect: Rect, ruler: bool, indent: f32, m: &Metrics) -> Rect {
    let hw = indent.min(rect.w);
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

/// A clip's **own** time axis: the part of `[0, dur]` its drawn rectangle `cr`
/// shows, in clip-local units. A clip rectangle is clamped to the lane body, so
/// a clip half-scrolled off the left is drawn starting at some `t > 0` — this is
/// that window.
///
/// It is what makes a clip a coordinate system rather than a rectangle the lane
/// keeps redrawing: everything inside one (its bodies, its break-points, its
/// notes) maps through `(cr, this)` alone, with no reference to the lane's
/// gutter, the group's window or the clip's offset on it. Move the same clip to
/// another lane, another window or another zoom and it draws the same.
pub fn clip_local_view(body: Rect, nav: &View, offset: f64, dur: f64, cr: Rect) -> View {
    if dur <= 0.0 || cr.w <= 0.0 {
        return View::full(1);
    }
    // The lane's mapping, run once, at the two edges of the drawn rectangle:
    // this is the last place a clip's contents look at the lane's window.
    let at = |x: f32| {
        let sample = nav.start + nav.len * ((x - body.x) as f64 / body.w.max(1.0) as f64);
        (sample - offset).clamp(0.0, dur)
    };
    let (start, end) = (at(cr.x), at(cr.x + cr.w));
    View {
        start,
        len: (end - start).max(f64::EPSILON),
    }
}

/// The x pixel a clip-local time falls on inside the clip rect `cr`.
fn local_x(cr: Rect, local: &View, t: f64) -> f32 {
    (cr.x as f64 + (t - local.start) / local.len * cr.w as f64) as f32
}

/// The clip-local time an x pixel of `cr` falls on — the inverse of [`local_x`].
fn local_t(cr: Rect, local: &View, x: f64) -> f64 {
    local.start + local.len * (x - cr.x as f64) / cr.w.max(1.0) as f64
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
    header: &Header,
    clips: &[ClipDraw],
    ruler: bool,
    indent: f32,
    m: &Metrics,
    theme: &Theme,
) {
    // The header band on the left — the group's indent, so every member of the
    // axis starts its body at the same x. What the lane puts in that band is
    // its own (a name, and the controls it offers).
    let band = timeline::gutter_band(rect, indent);
    mesh.rect(band, theme.header);
    let parts = header_parts(band, header, m);
    if let Some(t) = label {
        font::text_ellipsis(
            mesh,
            t,
            parts.label.x,
            parts.label.y,
            parts.label.w,
            m.text_scale,
            theme.text,
        );
    }
    draw_header_controls(mesh, band, header, m, theme);
    let body = lane_body(rect, ruler, indent, m);
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
        // Everything inside the clip is drawn against the clip's **own** axis:
        // its rectangle and the slice of `[0, dur]` that rectangle shows.
        let local = clip_local_view(body, nav, clip.offset, clip.dur, cr);
        // The bodies **layer**, back to front: the take, the events over it, the
        // envelope over both — an automation drawn on top of the material it
        // shapes is one clip, not two, and each body keeps its own value axis.
        match &clip.data {
            // A loaded take (mapped file, peak cache or fetched buffer): decimated
            // through its pyramid, so a minutes-long clip costs the same as a
            // short one.
            Some(data) => draw_body(mesh, cr, &local, clip, &Trace::Data(data), m, theme),
            // An **inline** sketch sent with the def: the same drawing, a
            // cheaper source.
            None => draw_body(
                mesh,
                cr,
                &local,
                clip,
                &Trace::samples(&clip.samples, 1),
                m,
                theme,
            ),
        }
        if !clip.notes.is_empty() {
            // Notes on the clip's own axis, pitch mapped over [min, max].
            draw_piano_roll(mesh, cr, &local, clip, m, theme);
        }
        if !clip.points.is_empty() {
            draw_curve(mesh, cr, &local, clip, m, theme);
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

/// The clip-local time an x pixel falls on inside the clip rect `cr`: the
/// inverse of the clip's own axis. The curve body's editing runs through this,
/// so a point lands where the pointer is under any zoom.
pub fn curve_time_at(cr: Rect, local: &View, cx: f64) -> f64 {
    local_t(cr, local, cx).max(0.0)
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
    local: &View,
    cx: f64,
    cy: f64,
    m: &Metrics,
) -> Option<usize> {
    // The `bpf` view's grab radius, so a point is grabbed the same way wherever
    // it is drawn.
    let radius = (m.point_radius + m.hit_slop).max(6.0) as f64;
    let mut best: Option<(usize, f64)> = None;
    for (i, p) in clip.points.iter().enumerate() {
        let x = local_x(cr, local, p.time) as f64;
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
/// is what is heard — plus a disc per breakpoint. Times map through the clip's
/// own axis (`local`), exactly as the piano-roll's notes do, so the whole curve
/// moves with the clip and stays put under zoom.
fn draw_curve(
    mesh: &mut Mesh,
    cr: Rect,
    local: &View,
    clip: &ClipDraw,
    m: &Metrics,
    theme: &Theme,
) {
    if cr.w < 1.0 || cr.h <= 0.0 {
        return;
    }
    let columns = cr.w.max(1.0) as usize;
    let y_at = |v: f32| curve_y(cr, v, clip.points_min, clip.points_max, clip.exp);
    let time_at = |x: f32| local_t(cr, local, x as f64);
    let mut prev = [cr.x, y_at(bpf::value_at(&clip.points, time_at(cr.x)))];
    for c in 1..=columns {
        let x = cr.x + c as f32;
        let p = [x, y_at(bpf::value_at(&clip.points, time_at(x)))];
        mesh.line(prev, p, m.trace_w, theme.trace);
        prev = p;
    }
    for p in &clip.points {
        let x = local_x(cr, local, p.time);
        if x >= cr.x && x <= cr.x + cr.w {
            mesh.disc(x, y_at(p.value), m.point_radius, theme.point);
        }
    }
}

/// Draws a clip's notes as a compact piano-roll inside `cr`: the clip body is
/// the grid, its `[min, max]` the pitch window, and the notes ride the clip's
/// own time axis (so the whole roll moves when the clip does). The geometry is
/// the shared [`super::pianoroll::draw_notes`] primitive — the same one the
/// dedicated `pianoroll` view draws with, so a clip's roll and the editor never
/// disagree. The clip body uses only that one layer (no keyboard/lanes).
fn draw_piano_roll(
    mesh: &mut Mesh,
    cr: Rect,
    local: &View,
    clip: &ClipDraw,
    m: &Metrics,
    theme: &Theme,
) {
    // The compact pitch ruler: each C named at the clip's left edge (there is
    // no keyboard gutter here), only when the rows are tall enough to read.
    pianoroll::draw_pitch_labels(mesh, cr, clip.min, clip.max, m, theme);
    // The clip rect is both the note primitive's pixel domain and its clamp:
    // note times are clip-local, so there is no offset to subtract any more.
    pianoroll::draw_notes(
        mesh,
        cr,
        cr,
        local,
        0.0,
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
/// pixel maps back through the clip's own axis to a clip-local time, and that
/// through the clip's span to a fraction of its data. This is the whole reason a
/// waveform body scrolls and stretches *with* the view instead of squashing into
/// whatever slice of the clip is on screen: it is drawn from the source, per
/// visible pixel, exactly as the piano-roll and the curve are.
pub fn clip_source_at(cr: Rect, local: &View, dur: f64, total: f64, x: f32) -> f64 {
    if dur <= 0.0 {
        return 0.0;
    }
    (local_t(cr, local, x as f64) / dur * total).clamp(0.0, total)
}

/// Draws a clip's signal body inside the *visible* part of the clip (`cr`),
/// reading its samples through the one column source every signal view shares
/// ([`Trace`]) — a loaded take answers from its peak pyramid, an inline sketch
/// straight off its slice, and the drawing is the same either way.
///
/// The body is drawn **from the source, per visible pixel**, mapped back
/// through the clip's own axis, which is what makes it scroll and stretch with
/// the view instead of squashing into whatever slice is on screen. Never
/// resolves finer than the screen — the one graphics rule.
// mesh + rect + axis + clip + source + look: one body's draw.
#[allow(clippy::too_many_arguments)]
fn draw_body(
    mesh: &mut Mesh,
    cr: Rect,
    local: &View,
    clip: &ClipDraw,
    trace: &Trace,
    m: &Metrics,
    theme: &Theme,
) {
    let total = trace.frames() as f64;
    if total < 2.0 || cr.w < 1.0 || cr.h <= 0.0 {
        return;
    }
    let y_at = |v: f32| cr.y + cr.h * (1.0 - fraction(v, clip.min, clip.max));
    if clip.min < 0.0 && clip.max > 0.0 {
        let y = y_at(0.0);
        mesh.line([cr.x, y], [cr.x + cr.w, y], m.divider_w, theme.baseline);
    }
    signal::trace::draw_channel(
        mesh,
        cr,
        trace,
        0,
        |x| clip_source_at(cr, local, clip.dur, total, x),
        // The inverse placement: a source frame sits at its own fraction of the
        // clip's span, seen through the clip's visible window.
        |s| local_x(cr, local, s / total * clip.dur),
        y_at,
        TraceStyle {
            color: theme.selection,
            width: m.divider_w,
        },
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
    fn a_header_widens_for_what_it_carries_and_a_declared_width_wins() {
        let m = Metrics::default();
        // A name-only strip is exactly what it always was.
        assert_eq!(Header::default().width(&m), m.header_w);
        // The width a header asks for is the width its parts fit in: whatever
        // it carries, asking is enough to be able to draw it.
        let full = Header {
            mute: Some(false),
            solo: Some(false),
            level: Some(0.8),
            ..Header::default()
        };
        assert!(full.width(&m) >= m.header_w);
        let band = Rect::new(0.0, 0.0, full.width(&m), 60.0);
        let parts = header_parts(band, &full, &m);
        assert!(parts.mute.is_some() && parts.solo.is_some() && parts.fader.is_some());
        // ...and a compact table sizes it down, not the other way round: the
        // roles move together, so the parts still fit.
        let compact = Metrics::generated(0.8);
        let band = Rect::new(0.0, 0.0, full.width(&compact), 60.0);
        assert!(header_parts(band, &full, &compact).fader.is_some());
        // An explicit width wins over both, even a narrow one.
        let declared = Header {
            w: Some(40.0),
            ..full.clone()
        };
        assert_eq!(declared.width(&m), 40.0);
    }

    #[test]
    fn a_header_drops_its_controls_before_its_name_when_the_band_is_small() {
        let m = Metrics::default();
        let header = Header {
            mute: Some(true),
            solo: Some(false),
            level: Some(0.5),
            ..Header::default()
        };
        let band = Rect::new(0.0, 0.0, header.width(&m), 60.0);
        let parts = header_parts(band, &header, &m);
        assert!(parts.mute.is_some() && parts.solo.is_some() && parts.fader.is_some());
        // A lane too short for a second row keeps the name and nothing else.
        let short = header_parts(Rect::new(0.0, 0.0, band.w, 16.0), &header, &m);
        assert_eq!((short.mute, short.solo, short.fader), (None, None, None));
        assert!(short.label.h > 0.0);
        // ...and so does one too narrow for the fader, which is dropped rather
        // than drawn as a stub.
        let narrow = header_parts(Rect::new(0.0, 0.0, 60.0, 60.0), &header, &m);
        assert!(narrow.mute.is_some() && narrow.fader.is_none());
    }

    #[test]
    fn a_press_lands_on_the_control_it_is_drawn_on() {
        let m = Metrics::default();
        let header = Header {
            mute: Some(false),
            solo: Some(false),
            level: Some(0.0),
            ..Header::default()
        };
        let band = Rect::new(0.0, 0.0, header.width(&m), 60.0);
        let parts = header_parts(band, &header, &m);
        let mid = |r: Rect| ((r.x + r.w / 2.0) as f64, (r.y + r.h / 2.0) as f64);
        for (rect, part) in [
            (parts.mute.unwrap(), HeaderPart::Mute),
            (parts.solo.unwrap(), HeaderPart::Solo),
            (parts.fader.unwrap(), HeaderPart::Fader),
        ] {
            let (x, y) = mid(rect);
            assert_eq!(header_hit(band, &header, &m, x, y), Some(part));
        }
        // The name row is not a control: a press there names nothing.
        let (x, y) = mid(parts.label);
        assert_eq!(header_hit(band, &header, &m, x, y), None);
        // The fader reads its value off its own width.
        let f = parts.fader.unwrap();
        assert!((level_at(f, f.x as f64) - 0.0).abs() < 0.01);
        assert!((level_at(f, (f.x + f.w) as f64) - 1.0).abs() < 0.01);
        assert!((level_at(f, (f.x + f.w / 2.0) as f64) - 0.5).abs() < 0.02);
    }

    #[test]
    fn lane_body_reserves_the_header_strip() {
        let body = lane_body(
            lane(),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
        );
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
        let ruled = lane_body(
            lane(),
            true,
            Metrics::default().header_w,
            &Metrics::default(),
        );
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
        let body = lane_body(
            lane(),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
        );
        let nav = View::full(400);
        // Halfway through the timeline: halfway across the lane body.
        let x = playhead_x(body, &nav, 200.0).unwrap();
        assert!((x - (body.x + body.w * 0.5)).abs() < 0.5);
        // Past the end of the window: nothing to draw.
        assert!(playhead_x(body, &nav, 500.0).is_none());
    }

    #[test]
    fn clip_x_range_places_the_clip_by_offset_and_duration() {
        let body = lane_body(
            lane(),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
        );
        let nav = View::full(400); // 1 sample per pixel over the 404-wide body-ish
        // A clip at [100, 200): starts a quarter in, one-quarter wide.
        let (x0, x1) = clip_x_range(body, &nav, 100.0, 100.0).unwrap();
        let px_per = body.w as f64 / 400.0;
        assert!((x0 as f64 - (body.x as f64 + 100.0 * px_per)).abs() < 0.5);
        assert!((x1 as f64 - (body.x as f64 + 200.0 * px_per)).abs() < 0.5);
    }

    #[test]
    fn clip_x_range_clips_to_the_body_and_drops_the_invisible() {
        let body = lane_body(
            lane(),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
        );
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
            &Header::default(),
            &clips,
            false,
            Metrics::default().header_w,
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
            &Header::default(),
            std::slice::from_ref(&clip),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(!m.is_empty(), "the take draws a body");
        // One min/max column per pixel of the clip rect, not one per sample: the
        // 100k-sample take costs the same as the lane is wide.
        let body = lane_body(
            lane(),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
        );
        let (x0, x1) = clip_x_range(body, &View::full(400), 0.0, 400.0).unwrap();
        let cols = (x1 - x0) as usize;
        let mut bare = Mesh::new();
        draw(
            &mut bare,
            lane(),
            &View::full(400),
            None,
            &Header::default(),
            &[ClipDraw {
                data: None,
                ..clip.clone()
            }],
            false,
            Metrics::default().header_w,
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
            &Header::default(),
            std::slice::from_ref(&clip),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
            &Theme::default(),
        );
        let mut bare = Mesh::new();
        draw(
            &mut bare,
            lane(),
            &View::full(400),
            None,
            &Header::default(),
            &[ClipDraw {
                points: Vec::new(),
                ..clip.clone()
            }],
            false,
            Metrics::default().header_w,
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
        let body = lane_body(
            lane(),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
        );
        let nav = View::full(400);
        // The clip sits at 100 on the axis, so its point at t=100 is at 200.
        let clip = ClipDraw {
            offset: 100.0,
            dur: 200.0,
            ..curve_clip(vec![pt(0.0, 0.0), pt(100.0, 1.0), pt(200.0, 0.0)])
        };
        let (x0, x1) = clip_x_range(body, &nav, clip.offset, clip.dur).unwrap();
        let cr = clip_rect(body, x0, x1);

        // The clip's own axis - the whole clip is visible, so it is [0, dur].
        let local = clip_local_view(body, &nav, clip.offset, clip.dur, cr);
        assert!((local.start).abs() < 0.5 && (local.len - 200.0).abs() < 0.5);

        // The peak point (t=100, value=1 -> the top of the clip).
        let px = to_x(200.0, &nav, body);
        let py = cr.y as f64;
        assert_eq!(
            curve_hit(&clip, cr, &local, px, py, &Metrics::default()),
            Some(1)
        );
        // Away from any point: nothing (so the clip still moves by its body).
        assert_eq!(
            curve_hit(&clip, cr, &local, px + 40.0, py + 20.0, &Metrics::default()),
            None
        );

        // The inverse mapping an edit uses: pixels -> clip-relative time, value.
        assert!((curve_time_at(cr, &local, px) - 100.0).abs() < 1.0);
        assert!((curve_value_at(cr, 0.0, 1.0, false, py) - 1.0).abs() < 0.05);
    }

    #[test]
    fn a_body_reads_the_source_through_the_axis_under_zoom_and_pan() {
        // The bug this pins: a partially visible clip must draw the *part of its
        // take that is on screen*, not squash the whole take into the visible
        // sliver — so a pixel maps back through the axis to the source.
        let body = lane_body(
            lane(),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
        );
        let clip = ClipDraw {
            offset: 0.0,
            dur: 400.0,
            ..curve_clip(Vec::new()) // a plain clip: no curve, no notes
        };
        let total = 1000.0;

        // Fully zoomed out: the clip's ends map to the take's ends.
        let full = View::full(400);
        let (x0, x1) = clip_x_range(body, &full, clip.offset, clip.dur).unwrap();
        let cr = clip_rect(body, x0, x1);
        let local = clip_local_view(body, &full, clip.offset, clip.dur, cr);
        assert!(clip_source_at(cr, &local, clip.dur, total, x0) < 1.0);
        assert!(clip_source_at(cr, &local, clip.dur, total, x1) > total - 1.0);

        // Zoomed into the clip's second half: the lane's left edge is now the
        // middle of the take, and the visible span is the half after it. The
        // clip's own axis says so - it starts at t=200 of a 400-long clip.
        let zoomed = View {
            start: 200.0,
            len: 200.0,
        };
        let (zx0, zx1) = clip_x_range(body, &zoomed, clip.offset, clip.dur).unwrap();
        let zcr = clip_rect(body, zx0, zx1);
        let zlocal = clip_local_view(body, &zoomed, clip.offset, clip.dur, zcr);
        assert!((zlocal.start - 200.0).abs() < 1.0 && (zlocal.len - 200.0).abs() < 1.0);
        let left = clip_source_at(zcr, &zlocal, clip.dur, total, body.x);
        let right = clip_source_at(zcr, &zlocal, clip.dur, total, body.x + body.w);
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
            &Header::default(),
            std::slice::from_ref(&layered),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
            &Theme::default(),
        );
        let mut roll_only = Mesh::new();
        draw(
            &mut roll_only,
            lane(),
            &View::full(400),
            None,
            &Header::default(),
            &[ClipDraw {
                points: Vec::new(),
                ..layered.clone()
            }],
            false,
            Metrics::default().header_w,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(
            both.vertex_count() > roll_only.vertex_count(),
            "the curve draws over the notes, it does not replace them"
        );

        // The curve's points sit on the curve's range: its 200 Hz start is near the
        // bottom of the clip, not off the pitch axis.
        let body = lane_body(
            lane(),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
        );
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
            &Header::default(),
            std::slice::from_ref(&clip),
            false,
            Metrics::default().header_w,
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
            &Header::default(),
            std::slice::from_ref(&bare),
            false,
            Metrics::default().header_w,
            &Metrics::default(),
            &Theme::default(),
        );
        assert!(
            with.vertex_count() > without.vertex_count(),
            "the notes add geometry over the bare clip"
        );
    }
}
