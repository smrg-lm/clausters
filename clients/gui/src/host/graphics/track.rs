//! The multitrack `track`/`clip` graphic unit: the DAW-style lane view.
//!
//! A `track` is a horizontal lane of the shared timeline; a `clip` is a placed
//! rectangle on it spanning `[offset, offset + dur]` in timeline sample units —
//! the model's **graphic unit** (length = duration). This module draws that
//! unit: a left header naming the track, the lane field, and one framed
//! rectangle per clip with its label and a body — a decimated waveform, or a
//! **piano-roll** of note events when the clip carries `notes` (the events
//! track's scalar-vertical view). Pure over a [`Draw`] (the flat-geometry
//! [`crate::host::paint`] painter), so it is unit-testable without a window — the
//! same posture as the static `plot`/`bpf` views.
//!
//! The tracks of one window share **one time axis** (aligned lanes): the frame
//! renderer computes the common span (the longest clip end) and maps every
//! lane's clips through the same [`View`], so a clip at offset 8 lines up
//! across tracks. Placement/geometry is display logic — this stays gui-side.

use super::meters::fraction;
use super::signal::trace::{self, Measures, Trace, TraceStyle};
use crate::host::font;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::timeline;
use crate::host::widget::{SourceWindow, Widget, WidgetKind};
use crate::viewport::View;

/// A piano-roll note. Re-exported from [`super::pianoroll`], the module that
/// owns the note model and the drawing/hit-test primitives — a clip's roll and
/// the dedicated `pianoroll` view share the one type so they never disagree on
/// geometry.
pub use super::pianoroll::Note;

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
/// ([`crate::host::timeline::group_indents`]), so one lane declaring a wide header
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
            return crate::host::metrics::snap_px(w, m.ui_scale).max(0.0);
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
fn draw_header_controls(d: &mut Draw, band: Rect, header: &Header) {
    let (mesh, m, theme) = d.parts();
    let parts = header_parts(band, header, m);
    let mut toggle =
        |rect: Option<Rect>, on: bool, letter: &str, lit: crate::host::paint::Color| {
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
    tree.descendants()
        .filter_map(|w| match w.kind {
            WidgetKind::Clip { offset, dur, .. } => Some(offset + dur),
            _ => None,
        })
        .fold(0.0f64, f64::max)
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
/// [`crate::host::timeline::group_indents`]): a lane sharing an axis with a roll or a
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
///
/// **A clip that is on screen is drawn, however short it is** — as a *line*
/// when it gets that short. A span thinner than `min_w` is widened to it (kept
/// inside the body, so a clip at the far edge grows leftwards instead of hanging
/// out), and `min_w` is the **hairline** every drawn line in the host uses,
/// nothing more: the alternative is a clip that exists, plays and is addressable
/// but occupies no pixel — nothing to see, nothing to grab, and no way back
/// except guessing where to zoom.
///
/// **The floor is a hairline and not a grabbable width**, which is the whole
/// difference. A floor wide enough to aim at (a grip's worth) is a floor that
/// *lies about the length*: the clip stops narrowing as the reader zooms out and
/// stops widening as they zoom in, so the picture says "this clip is about that
/// long" at every scale and the one thing a timeline exists to show is the one
/// thing it stops showing. A hairline says only "a clip is here" — the line
/// tracks the zoom the whole way down, and zooming *in* is what brings it back
/// to a width the hand can take (where the grip is over the line, since a clip
/// this narrow is all grip). What is not floored at all is a clip off the window
/// entirely: a line at the edge would claim a clip is there.
pub fn clip_x_range(
    body: Rect,
    nav: &View,
    offset: f64,
    dur: f64,
    min_w: f32,
) -> Option<(f32, f32)> {
    if dur <= 0.0 {
        return None;
    }
    let lo = body.x as f64;
    let hi = (body.x + body.w) as f64;
    let (ux0, ux1) = (to_x(offset, nav, body), to_x(offset + dur, nav, body));
    if ux1 <= lo || ux0 >= hi {
        return None; // off the window: not drawn at all
    }
    let (x0, x1) = (ux0.clamp(lo, hi), ux1.clamp(lo, hi));
    let w = (x1 - x0).max(min_w as f64).min(hi - lo);
    let x0 = x0.min(hi - w);
    (w > 0.0).then_some((x0 as f32, (x0 + w) as f32))
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

/// Draws one track lane into `rect`: the header (with `label` and its
/// controls) and the lane field. **Not** its clips — those are widgets the
/// layout places, drawn from their own placements ([`draw_clip`]), so the lane
/// draws what a lane is and nothing else.
///
/// `ruler` reserves the bottom strip for the time ruler (drawn by the frame
/// renderer, which owns the tick math); the playhead is an overlay over the
/// clips.
pub fn draw(
    d: &mut Draw,
    rect: Rect,
    label: Option<&str>,
    header: &Header,
    ruler: bool,
    indent: f32,
) {
    let (mesh, m, theme) = d.parts();
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
    draw_header_controls(&mut Draw::new(mesh, m, theme), band, header);
    let body = lane_body(rect, ruler, indent, m);
    if body.w > 0.0 && body.h > 0.0 {
        mesh.rect(body, theme.lane);
        mesh.border(body, m.divider_w, theme.frame);
    }
}

/// Draws one clip's own box into the rectangle the layout placed it at: its
/// fill, its edge and its `label`. Its **bodies** are children, drawn after it
/// from their own placements ([`draw_body_widget`]), so they land over it.
pub fn draw_clip(d: &mut Draw, cr: Rect) {
    let (mesh, m, theme) = d.parts();
    mesh.rect(cr, theme.object_fill);
    mesh.border(cr, m.divider_w, theme.object_edge);
}

/// Which **ends** of a clip are on screen, read off the clip's own axis: the
/// slice of `[0, dur]` its drawn rectangle shows. A clip scrolled half off the
/// left is drawn starting at some `t > 0`, and its start is not on screen at
/// all — the left edge of its rectangle is the *window's* edge, not the clip's.
///
/// This is what a grip has to ask before it draws: an affordance at the pixel a
/// clamp landed on says "the clip ends here", which is a lie, and it was read
/// as one before there was a grip at all (the plain border did it).
pub fn clip_ends_on_screen(local: &View, dur: f64) -> (bool, bool) {
    // A pixel of slack: the clamp is float arithmetic, and an end exactly at
    // the window's edge is on screen.
    let eps = (dur * 1e-6).max(0.5);
    (local.start <= eps, local.start + local.len >= dur - eps)
}

/// The two **grips** of a clip drawn at `cr`: the strips at its ends that
/// resize it, `None` where the end is off screen ([`clip_ends_on_screen`]) or
/// where the clip is too narrow to hold two of them and stays all body.
///
/// The renderer and the hit-test both call it, so the strip that lights up is
/// the strip that resizes — the rule every other part of this module follows.
/// **A clip too narrow for two grips keeps one**, and it is the one that gets
/// the reader out of the corner: its **end**, the edge that lengthens it (the
/// start when the end is the one off screen). Two strips on a rectangle that
/// cannot hold them would overlap, so the press could not tell them apart —
/// but returning neither left a clip shrunk to a sliver with no affordance at
/// all, movable and never growable. One grip keeps every state reversible.
///
/// The one grip is **as wide as the clip and no wider**, down to the hairline a
/// collapsed clip is drawn as ([`clip_x_range`]): a grip is a promise the press
/// keeps, so it can only be offered on pixels the press can be given. A clip
/// drawn as a line therefore carries its expand grip *on the line* — enough to
/// take once the zoom has widened it, and never a plate hanging over the trace
/// that is not the clip's.
pub fn clip_grips(cr: Rect, ends: (bool, bool), m: &Metrics) -> (Option<Rect>, Option<Rect>) {
    let w = m.grip_w;
    let strip = |x: f32, w: f32| Rect::new(x, cr.y, w, cr.h);
    if cr.w < 2.0 * w {
        let w = w.min(cr.w);
        return match ends {
            (_, true) => (None, Some(strip(cr.x + cr.w - w, w))),
            (true, false) => (Some(strip(cr.x, w)), None),
            (false, false) => (None, None),
        };
    }
    (
        ends.0.then(|| strip(cr.x, w)),
        ends.1.then(|| strip(cr.x + cr.w - w, w)),
    )
}

/// Which end of a clip the pointer is asking about: the half it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipSide {
    Start,
    End,
}

/// The grip a pointer at `cursor_x` is **on** — the strip under the cursor, and
/// `None` anywhere else on the clip.
///
/// **An affordance is drawn where it acts, and nowhere else.** This used to
/// light the grip of whichever *half* the pointer was in, so a strip a dozen
/// pixels wide announced itself from the middle of the clip — and then the
/// press there did not resize, because the middle of a clip is its body and the
/// body is what a body element (a roll's notes, a curve's points) is grabbed
/// through. A grip lit that far from its own pixels is a promise the press
/// cannot keep, and the reader learns to distrust the mark rather than the
/// distance. Lit only over its own strip, the two agree: what is lit is what
/// the press takes (`interact::clip_part` reads the same [`clip_grips`]), and a
/// pointer over the body lights nothing because nothing there resizes.
pub fn clip_grip_at(
    cr: Rect,
    ends: (bool, bool),
    m: &Metrics,
    cursor_x: f32,
) -> Option<(Rect, ClipSide)> {
    let (start, end) = clip_grips(cr, ends, m);
    let on = |r: &Rect| cursor_x >= r.x && cursor_x <= r.x + r.w;
    start
        .filter(on)
        .map(|r| (r, ClipSide::Start))
        .or_else(|| end.filter(on).map(|r| (r, ClipSide::End)))
}

/// The grip on a **named** side, for a caller that knows which one it wants
/// rather than asking where the pointer is — a drag already holding an edge.
pub fn clip_grip_on(
    cr: Rect,
    ends: (bool, bool),
    m: &Metrics,
    side: ClipSide,
) -> Option<(Rect, ClipSide)> {
    let (start, end) = clip_grips(cr, ends, m);
    match side {
        ClipSide::Start => start,
        ClipSide::End => end,
    }
    .map(|r| (r, side))
}

/// Draws one clip grip — the affordance for the resize gesture, shown while the
/// pointer is on that side of the clip.
///
/// It is a **plate**, the same translucent ground a caption over a picture sits
/// on ([`plate_text`](super::plate_text)), for the same reason: what is under
/// it is the take, and an opaque strip would cut a hole in the take rather than
/// mark an edge of it. The arrow says which way the edge moves and is centred
/// on the strip's height, so it reads at any lane thickness.
///
/// **The symbol is a parameter of the gesture, not of the clip.** An edge drag
/// means *trim* on one arrangement and *stretch* on another, and the day a lane
/// says which, the arrow is where that is announced — an outward chevron for
/// the edge that moves, another mark for the contents that stretches under it.
pub fn draw_clip_grip(d: &mut Draw, grip: Rect, side: ClipSide) {
    let (mesh, m, theme) = d.parts();
    mesh.round_rect(grip, m.plate_radius, theme.plate);
    // The arrow: a triangle pointing out of the clip, half the strip wide and
    // centred on it.
    let cy = grip.y + grip.h * 0.5;
    let half = (grip.w * 0.30).min(grip.h * 0.30);
    let (tip, base) = match side {
        ClipSide::Start => (grip.x + grip.w * 0.30, grip.x + grip.w * 0.70),
        ClipSide::End => (grip.x + grip.w * 0.70, grip.x + grip.w * 0.30),
    };
    mesh.tri(
        [tip, cy],
        [base, cy - half],
        [base, cy + half],
        theme.object_edge,
    );
}

/// Draws a clip's **name**, into whichever mesh is painted over its bodies.
///
/// It is a separate call because a name has to read: drawn with the box, the
/// take's trace goes over it, and the time-frequency texture — which is not
/// mesh at all but a GPU pass after every mesh — hides it outright. So the box
/// is the base mesh's and the name is the overlay's, the same split the
/// playhead and the selection already take.
///
/// The name is **kept inside the box it names**: `cr` is the clip's *visible*
/// rectangle (the span clamped to the lane), so a name written at its own
/// length runs out of a clip narrower than the string — over the neighbour that
/// starts there, which is the one place it must never be. It truncates with the
/// ellipsis instead, the rule every other single line in the host follows, and
/// a clip with no room for a glyph draws no name rather than a stray mark.
pub fn draw_clip_label(d: &mut Draw, cr: Rect, label: &str) {
    let (pad, scale, color) = (d.m.pad, d.m.caption_scale, d.theme.text);
    super::plate_text(
        d,
        label,
        cr.x + pad,
        cr.y + pad,
        cr.w - 2.0 * pad,
        scale,
        color,
    );
}

/// Draws one clip **body** — a child element of a clip — into the clip's
/// rectangle, against the clip's own axis. This is the whole of what "a clip is
/// a container" buys: the element says what it is, the container says where it
/// is, and neither knows about the lane, the group's window or the clip's
/// offset on it.
///
/// The bodies **layer**, back to front, because that is the order the layout
/// placed them in: the take, the events over it, the envelope over both — an
/// automation drawn on top of the contents it shapes is one clip, not two, and
/// each body keeps its own value axis.
pub fn draw_body_widget(
    d: &mut Draw,
    kind: &WidgetKind,
    cr: Rect,
    time: &crate::host::widget::element::TimeSpace,
) {
    let (mesh, m, theme) = d.parts();
    // Every leaf answers for itself; a widget that fills no body role draws
    // nothing here.
    if let WidgetKind::Custom(el) = kind {
        el.draw_body(&mut Draw::new(mesh, m, theme), cr, time);
    }
}

/// The **source** sample position an x pixel of a clip's body falls on: the
/// pixel maps back through the clip's own axis to a clip-local time, and that
/// through the clip's **window** onto the contents ([`SourceWindow`]).
///
/// `None` where the window is off the contents — a clip stretched past the end
/// of a buffer it does not loop. Nothing was recorded there, so nothing is
/// drawn and nothing is read; the alternative is a flat line that looks like
/// silence somebody recorded.
///
/// This is the whole reason a take's picture scrolls and trims *with* the clip
/// instead of squashing into whatever rectangle the clip currently has: it is
/// drawn from the source, per visible pixel, through a window that says which
/// part of the source that is.
pub fn clip_source_at(
    cr: Rect,
    local: &View,
    window: &SourceWindow,
    dur: f64,
    total: f64,
    x: f32,
) -> Option<f64> {
    if dur <= 0.0 {
        return None;
    }
    window.source_at(local_t(cr, local, x as f64), dur, total)
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
// mesh + rect + axis + span + source + range + look: one body's draw.
// The rect, the axis it is placed on, the source, its domain and what it
// measures: distinct inputs to one drawing pass, as in `draw_channel` below it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_take(
    d: &mut Draw,
    cr: Rect,
    local: &View,
    window: &SourceWindow,
    dur: f64,
    trace: &Trace,
    min: f32,
    max: f32,
    measures: Measures,
    overlay: bool,
    sample_rate: f64,
    written: Option<u64>,
) {
    let (mesh, m, theme) = d.parts();
    let total = trace.frames() as f64;
    if total < 2.0 || cr.w < 1.0 || cr.h <= 0.0 {
        return;
    }
    // **Every channel is drawn**, stacked, exactly as the standalone view
    // stacks its lanes: a clip is a picture of the contents and a stereo take
    // whose right channel is nowhere on it is a picture of half of one — which
    // is also what an edit on that channel would land in, invisibly. `overlay`
    // is the same choice the standalone view offers, and it arrives the same
    // way (the element's own prop), so the two never disagree about what a
    // channel is.
    let lanes = if overlay { 1 } else { trace.channels().max(1) };
    // **The window is drawn a run at a time**, each run a stretch of clip time
    // over which it stays inside the contents — one run for the ordinary case,
    // one per iteration for a looping clip, and none at all where a clip
    // reaches past contents it does not loop. Each run is an *affine* window,
    // which is what lets one renderer draw all of them: the wrap lives in the
    // run list rather than in the maps, so nothing downstream has to know
    // whether a clip loops.
    let runs = window.runs(local.start, local.start + local.len, dur, total);
    for ch in 0..trace.channels().max(1) {
        let lane = crate::host::frame::lane_rect(cr, lanes, if overlay { 0 } else { ch });
        if lane.h <= 0.0 {
            continue;
        }
        let y_at = move |v: f32| lane.y + lane.h * (1.0 - fraction(v, min, max));
        // The line and the fill read one rule, so a take cannot be filled to a
        // baseline that was never drawn (or drawn one it does not reach).
        if let Some(b) = crate::waveform::baseline_of(min, max) {
            let y = y_at(b);
            mesh.line(
                [lane.x, y],
                [lane.x + lane.w, y],
                m.divider_w,
                theme.baseline,
            );
        }
        for &(from, to, source0) in &runs {
            // The run's own rectangle: the pixels its stretch of clip time
            // covers, which is what bounds the drawing to it.
            let (x0, x1) = (local_x(cr, local, from), local_x(cr, local, to));
            let run_rect = Rect::new(x0, lane.y, (x1 - x0).max(0.0), lane.h);
            if run_rect.w < 0.5 {
                continue;
            }
            // Inside a run the window is affine, whichever kind it is: the
            // source frame at its start plus the time since, or the fitted
            // mapping over the whole span.
            let src = move |x: f32| match window.fit {
                true => (local_t(cr, local, x as f64) / dur * total).clamp(0.0, total),
                false => source0 + (local_t(cr, local, x as f64) - from),
            };
            let x_of = move |s: f64| match window.fit {
                true => local_x(cr, local, s / total * dur),
                false => local_x(cr, local, from + (s - source0)),
            };
            // One picture per measure, the envelope first and the level body
            // inside it.
            for measure in measures.iter() {
                trace::draw_channel(
                    mesh,
                    run_rect,
                    trace,
                    ch,
                    src,
                    x_of,
                    y_at,
                    TraceStyle::new(
                        trace::measure_color(theme, measure, theme.selection),
                        m.divider_w,
                    )
                    .with_dots(m.point_radius)
                    .with_measure(measure)
                    .with_rate(sample_rate)
                    .with_written(written),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::host::elements::signal;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;
    use crate::waveform::WaveformData;

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
        let (x0, x1) = clip_x_range(body, &nav, 100.0, 100.0, 0.0).unwrap();
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
        // A clip [0, 100) ends before the window: fully invisible. It stays
        // invisible whatever floor the drawing carries -- a sliver at the edge
        // would claim a clip is there.
        assert!(clip_x_range(body, &nav, 0.0, 100.0, 0.0).is_none());
        assert!(clip_x_range(body, &nav, 0.0, 100.0, 12.0).is_none());
        // A clip [100, 400) overlaps the left edge: clamped to the body start.
        let (x0, _) = clip_x_range(body, &nav, 100.0, 300.0, 0.0).unwrap();
        assert_eq!(x0, body.x);
        // A zero-duration clip draws nothing.
        assert!(clip_x_range(body, &nav, 160.0, 0.0, 12.0).is_none());

        // **A clip on screen is drawn however short it is** — as the line the
        // floor is. A hundredth of a sample is a fortieth of a pixel here: with
        // no floor the rectangle is geometry the rasterizer has nothing to put
        // down, which is a clip that plays, answers a query and occupies no
        // pixel. The floor the layout passes is the hairline, so what comes
        // back marks where the clip is and claims nothing about its length —
        // widen it to a grabbable strip instead and the clip would stop
        // narrowing as the reader zooms out.
        let (x0, x1) = clip_x_range(body, &nav, 160.0, 0.01, 0.0).expect("still a clip");
        assert!(x1 - x0 < 1.0, "under a pixel: {}", x1 - x0);
        let (x0, x1) = clip_x_range(body, &nav, 160.0, 0.01, 1.0).expect("drawn");
        assert!((x1 - x0 - 1.0).abs() < 0.01, "{x0}..{x1}");
        // A span the floor does not reach is untouched: the drawing follows the
        // zoom everywhere above the line, which is the whole picture.
        let (x0, x1) = clip_x_range(body, &nav, 160.0, 10.0, 1.0).expect("drawn");
        let px_per = body.w as f64 / 100.0;
        assert!(((x1 - x0) as f64 - 10.0 * px_per).abs() < 0.5, "{x0}..{x1}");
        // ...and it is kept inside the lane: at the far edge it grows leftwards
        // rather than hanging out of the body it belongs to.
        let (x0, x1) = clip_x_range(body, &nav, 249.99, 0.01, 12.0).expect("drawn");
        assert!(
            x1 <= body.x + body.w + 0.01,
            "{x1} past {}",
            body.x + body.w
        );
        assert!((x1 - x0 - 12.0).abs() < 0.01);
    }

    /// The geometry a clip is drawn with, for a lane spanning `nav`: the
    /// rectangle the layout would place it at and the clip's own axis. The
    /// tests below draw bodies exactly as the frame does — through
    /// `(rect, local)` and nothing else.
    fn placed(offset: f64, dur: f64, nav: &View) -> (Rect, View) {
        let m = Metrics::default();
        let body = lane_body(lane(), false, m.header_w, &m);
        let (x0, x1) = clip_x_range(body, nav, offset, dur, m.grip_w).expect("the clip is visible");
        let cr = clip_rect(body, x0, x1);
        (cr, clip_local_view(body, nav, offset, dur, cr))
    }

    fn body_mesh(kind: &WidgetKind, dur: f64) -> Mesh {
        let (cr, local) = placed(0.0, dur, &View::full(dur.max(1.0) as usize));
        let mut mesh = Mesh::new();
        draw_body_widget(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            kind,
            cr,
            &crate::host::widget::element::TimeSpace::of(local, dur),
        );
        mesh
    }

    fn take_body(data: signal::Data) -> WidgetKind {
        let mut el = signal::SignalElement::from_preset(&signal::point(
            crate::host::elements::signal::Presentation::Signal,
            false,
            true,
        ));
        el.caps = signal::Caps::default();
        el.source = signal::Source::Data(data);
        WidgetKind::Custom(Box::new(el))
    }

    fn inline_take(samples: Vec<f32>) -> signal::Data {
        signal::Data {
            samples: samples.into(),
            channels: 1,
            buffer: None,
            path: None,
            cache: None,
            base_bucket: 256,
            bulk: true,
            body: None,
        }
    }

    /// **A stereo take draws both channels**, stacked inside the clip: a clip
    /// is a picture of the contents, and a right channel drawn nowhere is a
    /// picture of half of it — which is also where an edit on that channel
    /// would land, invisibly.
    #[test]
    fn a_stereo_take_draws_a_lane_per_channel_inside_the_clip() {
        // Interleaved, and deliberately unlike: the left channel swings, the
        // right one is flat. Each lane must show its own.
        let frames = 4_000;
        let samples: Vec<f32> = (0..frames)
            .flat_map(|i| [0.9 * (i as f32 * 0.05).sin(), 0.0])
            .collect();
        let mut data = inline_take(samples);
        data.channels = 2;
        let mesh = body_mesh(&take_body(data), frames as f64);
        let drawn = mesh.extent().expect("the take drew");

        let (cr, _) = placed(0.0, frames as f64, &View::full(frames));
        let mid = cr.y + cr.h / 2.0;
        assert!(
            drawn.y < mid && drawn.y + drawn.h > mid,
            "the picture spans both halves of the clip: {drawn:?} against {mid}"
        );
        // The swinging channel is the top lane, so the ink above the divide is
        // taller than the flat one's below it.
        let above = mid - drawn.y;
        let below = drawn.y + drawn.h - mid;
        assert!(
            above > below,
            "the lanes hold their own channel: {above} above, {below} below"
        );
    }

    /// **One body, a picture per measure** — the classic editor picture, and
    /// the correction the first attempt earned. Two elements measuring
    /// differently on one rectangle are *not* layers: each paints its own field
    /// before it draws, so the second hides the first. The layering is a set on
    /// the element instead, and this is what that has to deliver — the level body
    /// drawn inside the envelope, by the one renderer placed twice, in one
    /// drawing that the axis, the ruler and the upload are shared by.
    #[test]
    fn one_body_draws_the_level_inside_the_envelope() {
        // Audio: zero-mean, which is the case the picture is a convention for.
        // (A signal carrying DC is the interesting other one, and it is the
        // trace's own test that states what happens there.)
        let samples: Vec<f32> = (0..20_000).map(|i| 0.9 * (i as f32 * 0.05).sin()).collect();
        let dur = samples.len() as f64;
        let measured = |measures: Measures| {
            let mut el = signal::SignalElement::from_preset(&signal::point(
                crate::host::elements::signal::Presentation::Signal,
                false,
                true,
            ));
            el.caps = signal::Caps::default();
            el.source = signal::Source::Data(inline_take(samples.clone()));
            el.measures = measures;
            body_mesh(&WidgetKind::Custom(Box::new(el)), dur)
        };
        let peak = measured(Measures::of(trace::Measure::Peak));
        let rms = measured(Measures::of(trace::Measure::Rms));
        let envelope = peak.extent().expect("the envelope drew");
        let body = rms.extent().expect("the body drew");
        assert!(
            body.h < envelope.h,
            "the body sits inside the envelope: {} vs {}",
            body.h,
            envelope.h
        );
        assert!(
            body.y > envelope.y - 0.01 && body.y + body.h < envelope.y + envelope.h + 0.01,
            "and inside it on the axis, not merely shorter"
        );
        assert!((body.x - envelope.x).abs() < 0.01);

        // And asking for both draws both, into the one body: the envelope still
        // bounds the picture, and the geometry is the two drawings together.
        let both = measured(Measures::parse("peak rms").expect("two known names"));
        let together = both.extent().expect("the picture drew");
        assert!(
            (together.y - envelope.y).abs() < 0.01 && (together.h - envelope.h).abs() < 0.01,
            "the envelope is what the picture reaches"
        );
        // Both drawings, over the one zero line the body draws once rather than
        // once per measure -- which is the whole difference between a picture
        // that layers and two pictures that would each bring their own chrome.
        let (sum, drawn) = (
            peak.vertex_count() + rms.vertex_count(),
            both.vertex_count(),
        );
        assert!(
            drawn > peak.vertex_count().max(rms.vertex_count()) && drawn >= sum - 12,
            "it is both drawings and not one of them: {drawn} against {sum}"
        );
    }

    #[test]
    fn a_lane_draws_its_header_and_field_and_no_clip() {
        // What a lane is: a header band and a field. Its clips are widgets the
        // layout places, so they are not the lane's to draw.
        let mut m = Mesh::new();
        let metrics = Metrics::default();
        draw(
            &mut Draw::new(&mut m, &metrics, &Theme::default()),
            lane(),
            Some("drums"),
            &Header::default(),
            false,
            metrics.header_w,
        );
        assert!(!m.is_empty(), "the header and the lane field draw");

        // ...and a clip's box is drawn from its own placement.
        let before = m.vertex_count();
        let (cr, _) = placed(0.0, 100.0, &View::full(400));
        draw_clip(&mut Draw::new(&mut m, &metrics, &Theme::default()), cr);
        assert!(m.vertex_count() > before);
        // The name is the overlay's, so it is not in that count: drawn with the
        // box, a take's trace (or a spectral clip's texture) would bury it.
        let mut over = Mesh::new();
        draw_clip_label(
            &mut Draw::new(&mut over, &metrics, &Theme::default()),
            cr,
            "a",
        );
        assert!(!over.is_empty(), "the name draws over the bodies");
    }

    /// A clip's name stays inside the clip: `cr` is the *visible* rectangle, so
    /// a name at its own length runs over whatever starts where this clip ends.
    #[test]
    fn a_clip_name_is_truncated_to_the_box_it_names() {
        let metrics = Metrics::default();
        let theme = Theme::default();
        let wide = Rect::new(0.0, 0.0, 400.0, 40.0);
        let narrow = Rect::new(0.0, 0.0, 40.0, 40.0);

        let mut full = Mesh::new();
        draw_clip_label(&mut Draw::new(&mut full, &metrics, &theme), wide, "a take");
        let right = |m: &Mesh| m.positions().map(|(x, _)| x).fold(f32::MIN, f32::max);
        assert!(right(&full) <= wide.w, "a name that fits stays put");

        let mut cut = Mesh::new();
        draw_clip_label(&mut Draw::new(&mut cut, &metrics, &theme), narrow, "a take");
        assert!(!cut.is_empty(), "a narrow clip still says what it can");
        assert!(
            right(&cut) <= narrow.w,
            "the name bleeds past the clip ({} > {})",
            right(&cut),
            narrow.w
        );

        // No room for a glyph: no stray mark where a name would have been.
        let mut none = Mesh::new();
        draw_clip_label(
            &mut Draw::new(&mut none, &metrics, &theme),
            Rect::new(0.0, 0.0, 2.0, 40.0),
            "a take",
        );
        assert!(none.is_empty(), "a sliver of a clip draws no name");
    }

    /// A grip is drawn where the clip **ends**, never where the window cut it,
    /// and the strip that lights up is the strip that resizes.
    #[test]
    fn a_grip_stands_at_an_end_that_is_on_screen() {
        let m = Metrics::default();
        let cr = Rect::new(100.0, 0.0, 200.0, 40.0);

        // A clip whose whole span is drawn: a grip at each end, each `grip_w`
        // wide and flush with its edge.
        let (a, b) = clip_grips(cr, (true, true), &m);
        let a = a.expect("the start is on screen");
        let b = b.expect("the end is on screen");
        assert_eq!((a.x, a.w), (cr.x, m.grip_w));
        assert_eq!((b.x, b.w), (cr.x + cr.w - m.grip_w, m.grip_w));

        // Scrolled half off the left: the rectangle's left edge is the
        // window's, so there is nothing to grab there.
        let local = View {
            start: 500.0,
            len: 500.0,
        };
        let ends = clip_ends_on_screen(&local, 1000.0);
        assert_eq!(
            ends,
            (false, true),
            "the start is off screen, the end is not"
        );
        assert!(clip_grips(cr, ends, &m).0.is_none());

        // ...and a clip narrower than two grips keeps **one**, the end — the
        // edge that lengthens it, which is the way out of a clip shrunk to a
        // sliver. Two strips would overlap on a rectangle this size and the
        // press could not tell them apart; none at all is what left a sliver
        // movable and never growable.
        let narrow = Rect::new(0.0, 0.0, m.grip_w, 40.0);
        assert_eq!(
            clip_grips(narrow, (true, true), &m),
            (None, Some(Rect::new(0.0, 0.0, m.grip_w, 40.0)))
        );
        // Narrower than the grip itself: the grip is the whole clip.
        let sliver = Rect::new(0.0, 0.0, 4.0, 40.0);
        assert_eq!(
            clip_grips(sliver, (true, true), &m).1.map(|r| (r.x, r.w)),
            Some((0.0, 4.0))
        );
        // With the end off screen it is the start that is grabbable instead —
        // one grip, and the one that is there.
        assert_eq!(
            clip_grips(narrow, (true, false), &m).0.map(|r| r.x),
            Some(0.0)
        );
        // Neither end on screen: nothing to grab, as before.
        assert_eq!(clip_grips(narrow, (false, false), &m), (None, None));

        // One at a time, and only the strip the pointer is **on** — not the
        // half it is in. A grip lit from the middle of a clip announces a
        // resize that the press there does not give (the middle is the body's,
        // and a body element grabs whatever it finds under those pixels).
        let left = clip_grip_at(cr, (true, true), &m, cr.x + 1.0);
        let right = clip_grip_at(cr, (true, true), &m, cr.x + cr.w - 1.0);
        assert_eq!(left.map(|(_, s)| s), Some(ClipSide::Start));
        assert_eq!(right.map(|(_, s)| s), Some(ClipSide::End));
        assert_eq!(left.unwrap().0.w, m.grip_w, "it is lit over its own width");
        // Just past the strip, and anywhere else on the clip: nothing.
        assert!(clip_grip_at(cr, (true, true), &m, cr.x + m.grip_w + 1.0).is_none());
        assert!(clip_grip_at(cr, (true, true), &m, cr.x + cr.w * 0.5).is_none());
        assert!(
            clip_grip_at(cr, (true, true), &m, cr.x + cr.w - m.grip_w - 1.0).is_none(),
            "the right half alone is not the end grip"
        );
        // ...and nothing on the side whose end is off screen.
        assert!(clip_grip_at(cr, (false, true), &m, cr.x + 1.0).is_none());

        // The drawing is a plate with a mark on it, and it draws only what it
        // was given.
        let mut mesh = Mesh::new();
        draw_clip_grip(
            &mut Draw::new(&mut mesh, &m, &Theme::default()),
            left.unwrap().0,
            ClipSide::Start,
        );
        assert!(!mesh.is_empty(), "the grip is drawn");
    }

    #[test]
    fn a_loaded_take_draws_decimated_through_its_pyramid() {
        // A "long" take: many more samples than the clip has pixels. The body
        // must cost pixels, not samples — it is read through the peak pyramid.
        let samples: Vec<f32> = (0..100_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut data = inline_take(Vec::new());
        data.body = Some(Arc::new(WaveformData::new(samples.into(), 256)));
        // Placed 1:1 — one timeline sample per source frame, which is what a
        // clip's window is — so the whole take is what the clip shows.
        let drawn = body_mesh(&take_body(data), 100_000.0);
        assert!(!drawn.is_empty(), "the take draws a body");

        // One min/max column per pixel of the clip rect, not one per sample: the
        // 100k-sample take costs the same as the lane is wide.
        let (cr, _) = placed(0.0, 100_000.0, &View::full(100_000));
        let cols = cr.w as usize;
        let per_line = 6u32; // two triangles per column line
        assert!(
            drawn.vertex_count() <= (cols as u32 + 2) * per_line,
            "the body is decimated to the clip's pixel width ({} vertices for {cols} columns)",
            drawn.vertex_count()
        );
    }

    fn roll_body(notes: Vec<Note>, min: f32, max: f32) -> WidgetKind {
        let mut props = serde_json::Map::new();
        props.insert(
            "notes".into(),
            serde_json::Value::Array(
                notes
                    .iter()
                    .flat_map(|n| {
                        [
                            serde_json::Value::from(n.start),
                            serde_json::Value::from(n.dur),
                            serde_json::Value::from(n.pitch),
                            serde_json::Value::from(n.velocity),
                            serde_json::Value::from(n.channel),
                        ]
                    })
                    .collect(),
            ),
        );
        props.insert("min".into(), serde_json::Value::from(min));
        props.insert("max".into(), serde_json::Value::from(max));
        // No notes at all is a clip that has none: the body it grows is the
        // empty one, exactly as `clip_bodies` builds it.
        WidgetKind::Custom(match crate::host::elements::notes::body(&props) {
            Some(roll) => Box::new(roll),
            None => Box::new(crate::host::elements::notes::empty_body()),
        })
    }

    #[test]
    fn each_body_draws_only_what_it_is() {
        // Two bodies, two drawings, no precedence between them: a body draws
        // its own data and nothing else's.
        let roll = body_mesh(
            &roll_body(vec![Note::new(0.0, 100.0, 60.0)], 48.0, 72.0),
            400.0,
        );
        let take = body_mesh(&take_body(inline_take(vec![0.0, 0.5, -0.5, 1.0])), 400.0);
        for (what, mesh) in [("roll", &roll), ("take", &take)] {
            assert!(!mesh.is_empty(), "the {what} body draws");
        }
        // An empty body of any kind draws nothing at all.
        assert!(body_mesh(&roll_body(Vec::new(), 48.0, 72.0), 400.0).is_empty());
    }

    /// The seam the placement exists for: what the layout hands a clip's body
    /// — its rectangle and the clip's own window — is what the **element**
    /// grabs through, so a break-point is hit where it was drawn on a lane.
    #[test]
    fn a_body_element_grabs_through_the_axis_the_clip_placed_it_on() {
        use crate::host::elements::curve;
        use crate::host::widget::element::{Claim, Element, Input, Mods, TimeSpace};

        let nav = View::full(400);
        let m = Metrics::default();
        let lane_rect = lane_body(lane(), false, m.header_w, &m);
        // The clip sits at 100 on the axis, so its point at t=100 is at 200.
        let (cr, local) = placed(100.0, 200.0, &nav);
        // The clip's own axis - the whole clip is visible, so it is [0, dur].
        assert!(local.start.abs() < 0.5 && (local.len - 200.0).abs() < 0.5);

        let mut el = curve::body(
            &serde_json::from_str(
                r#"{"points":[0.0,0.0,1,0.0,100.0,1.0,1,0.0,200.0,0.0,1,0.0],
                    "points_min":0.0,"points_max":1.0}"#,
            )
            .unwrap(),
        )
        .expect("the props carry a curve");
        let input = Input {
            metrics: &m,
            indent: 0.0,
            rect: cr,
            scale: 1.0,
            mods: Mods::default(),
            viewport: (lane_rect.w, lane_rect.h),
            time: Some(TimeSpace::of(local, 200.0)),
        };

        // The peak point (t=100, value=1 -> the top of the clip).
        let px = to_x(200.0, &nav, lane_rect);
        let py = cr.y as f64;
        assert!(matches!(el.press((px, py), &input), Claim::Take(_)));

        // Anywhere that is not this curve's own contents, a body **declines**:
        // it shares its rectangle with the clip, whose own drag is what the
        // rest of it means, so the press falls back to the container's move.
        // Held as the **active layer** the curve also bends the segment under
        // the cursor, which is why the declining press is made with the layer
        // handed to somebody else.
        let mut el2 = el.clone();
        let inactive = Input {
            time: input.time.map(|t| t.with_active(false)),
            ..input
        };
        assert!(matches!(
            el2.press((px + 40.0, py + 20.0), &inactive),
            Claim::Decline
        ));
        assert!(
            matches!(el2.press((px + 40.0, py + 20.0), &input), Claim::Take(_)),
            "the layer in hand bends the segment it is over"
        );

        // The drag maps pixels back through the same axis: dropping the peak on
        // the clip's left edge takes it to t=0, where it is grabbed next time.
        el.drag((cr.x as f64, py), &input);
        el.release((cr.x as f64, py), &input);
        assert!(matches!(
            el.press((cr.x as f64, py), &input),
            Claim::Take(_)
        ));
    }

    #[test]
    fn a_body_reads_the_source_through_the_axis_under_zoom_and_pan() {
        // The bug this pins: a partially visible clip must draw the *part of its
        // take that is on screen*, not squash the whole take into the visible
        // sliver — so a pixel maps back through the axis to the source.
        let m = Metrics::default();
        let lane_rect = lane_body(lane(), false, m.header_w, &m);
        let (dur, total) = (400.0, 1000.0);

        // A **fitted** clip: 400 units of timeline showing 1000 frames of
        // contents, which is the picture a time stretch would make. Fully
        // zoomed out its ends map to the take's ends.
        let fit = SourceWindow {
            fit: true,
            ..SourceWindow::default()
        };
        let at = |cr, local: &View, x| clip_source_at(cr, local, &fit, dur, total, x).unwrap();
        let (cr, local) = placed(0.0, dur, &View::full(400));
        assert!(at(cr, &local, cr.x) < 1.0);
        assert!(at(cr, &local, cr.x + cr.w) > total - 1.0);

        // Zoomed into the clip's second half: the lane's left edge is now the
        // middle of the take, and the visible span is the half after it. The
        // clip's own axis says so - it starts at t=200 of a 400-long clip.
        let zoomed = View {
            start: 200.0,
            len: 200.0,
        };
        let (zcr, zlocal) = placed(0.0, dur, &zoomed);
        assert!((zlocal.start - 200.0).abs() < 1.0 && (zlocal.len - 200.0).abs() < 1.0);
        let left = at(zcr, &zlocal, lane_rect.x);
        let right = at(zcr, &zlocal, lane_rect.x + lane_rect.w);
        assert!(
            (left - 500.0).abs() < 5.0,
            "the left edge is mid-take, not 0"
        );
        assert!((right - total).abs() < 5.0);
    }

    /// **A clip's take may be several segments**, each over its own stretch of
    /// the clip and each reading its own window onto its own contents — which
    /// is what joining fragments of two different files makes. The placement is
    /// the same mapping a lane uses for a clip, one level down, so each segment
    /// is drawn on its own part of the rectangle rather than over the whole of
    /// it.
    #[test]
    fn a_take_of_two_segments_draws_each_on_its_own_stretch() {
        use crate::host::widget::element::TimeSpace;
        use crate::host::widget::{SourceWindow, Widget};

        // A clip 400 long holding two takes, each over half of it.
        let node = crate::host::guidef::GuiNode::parse(
            br#"{"id":1,"type":"field","offset":0.0,"dur":400.0,"children":[
                 {"type":"signal","view":"trace","at":0.0,"dur":200.0,
                  "data":[0.0,1.0,0.0,-1.0]},
                 {"type":"signal","view":"trace","at":200.0,"dur":200.0,
                  "start":2.0,"data":[0.0,0.5,1.0,0.5]}]}"#,
        )
        .unwrap();
        let clip = Widget::from_node(1, &node, &[]).unwrap();
        assert_eq!(clip.children.len(), 2, "two takes, two layers");
        assert_eq!(clip.children[0].span, Some((0.0, 200.0)));
        assert_eq!(clip.children[1].span, Some((200.0, 200.0)));
        // The second reads its own window; the first says nothing and is drawn
        // through the clip's.
        assert_eq!(
            clip.children[1].window,
            Some(SourceWindow {
                start: 2.0,
                looping: false,
                fit: false
            })
        );
        assert_eq!(clip.children[0].window, None);

        // Each draws into its own half of the clip's rectangle.
        let (cr, local) = placed(0.0, 400.0, &View::full(400));
        let halves: Vec<Mesh> = clip
            .children
            .iter()
            .map(|child| {
                let (at, len) = child.span.unwrap();
                let (x0, x1) =
                    clip_x_range(cr, &local, at, len, Metrics::default().divider_w).unwrap();
                let rect = Rect::new(x0, cr.y, x1 - x0, cr.h);
                let view = clip_local_view(cr, &local, at, len, rect);
                let mut mesh = Mesh::new();
                draw_body_widget(
                    &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
                    &child.kind,
                    rect,
                    &TimeSpace::of(view, len).with_window(child.window.unwrap_or_default()),
                );
                mesh
            })
            .collect();
        assert!(halves.iter().all(|m| !m.is_empty()), "both draw");
        assert_ne!(
            halves[0].vertex_count(),
            0,
            "the first segment fills the left half"
        );
    }

    #[test]
    fn a_layered_clip_draws_every_body_and_each_keeps_its_own_axis() {
        // An envelope drawn over the event it shapes is *one* clip: both bodies
        // draw, and they do not share a value axis (notes are pitches, the curve
        // is its parameter's units).
        use crate::host::elements::curve;
        use crate::host::widget::element::{Ctx, Element, TimeSpace};
        use crate::host::world::World;

        let roll = roll_body(vec![Note::new(0.0, 200.0, 60.0)], 48.0, 72.0);
        let curve = curve::body(
            &serde_json::from_str(
                r#"{"points":[0.0,200.0,1,0.0,400.0,900.0,1,0.0],
                    "points_min":150.0,"points_max":1000.0}"#,
            )
            .unwrap(),
        )
        .expect("the props carry a curve");
        let (cr, local) = placed(0.0, 400.0, &View::full(400));
        let metrics = Metrics::default();
        let theme = Theme::default();

        let mut roll_only = Mesh::new();
        draw_body_widget(
            &mut Draw::new(&mut roll_only, &metrics, &theme),
            &roll,
            cr,
            &TimeSpace::of(local, 400.0),
        );
        let mut both = Mesh::new();
        draw_body_widget(
            &mut Draw::new(&mut both, &metrics, &theme),
            &roll,
            cr,
            &TimeSpace::of(local, 400.0),
        );
        let world = World::default();
        curve.draw(
            &mut Draw::new(&mut both, &metrics, &theme),
            &Ctx {
                world: &world,
                metrics: &metrics,
                rect: cr,
                indent: 0.0,
                scale: 1.0,
                time: Some(TimeSpace::of(local, 400.0)),
                clip: None,
                focused: false,
            },
        );
        assert!(
            both.vertex_count() > roll_only.vertex_count(),
            "the curve draws over the notes, it does not replace them"
        );
    }
}
