//! The multitrack `track`/`clip` graphic unit: the DAW-style lane view.
//!
//! A `track` is a horizontal lane of the shared timeline; a `clip` is a placed
//! rectangle on it spanning `[offset, offset + dur]` in timeline sample units --
//! the model's **graphic unit** (length = duration). This module draws that
//! unit: a left header naming the track, the lane field, and one framed
//! rectangle per clip with its label and a body -- a decimated waveform, or a
//! **piano-roll** of note events when the clip carries `notes` (the events
//! track's scalar-vertical view). Pure over a [`Draw`] (the flat-geometry
//! [`crate::host::paint`] painter), so it is unit-testable without a window -- the
//! same posture as the static `plot`/`bpf` views.
//!
//! The tracks of one window share **one time axis** (aligned lanes): the frame
//! renderer computes the common span (the longest clip end) and maps every
//! lane's clips through the same [`View`], so a clip at offset 8 lines up
//! across tracks. Placement/geometry is display logic -- this stays gui-side.

use clausters_core::measure;

use super::meters::{self, fraction};
use super::signal::layers::Stack;
use super::signal::trace::{self, Trace, TraceStyle};
use crate::host::font;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::timeline;
use crate::host::widget::{SourceWindow, WidgetKind};
use crate::viewport::View;

/// A piano-roll note. Re-exported from [`super::pianoroll`], the module that
/// owns the note model and the drawing/hit-test primitives -- a clip's roll and
/// the dedicated `pianoroll` view share the one type so they never disagree on
/// geometry.
pub use crate::host::structures::notes::Note;

/// What a lane reserves **left of its axis**, and what it carries there.
///
/// A lane header used to be one number in the size table (`header_w`) holding
/// one string. It is a strip of controls: a name, the mute/solo pair, a level
/// fader -- so its width follows what it carries, and a lane that carries more
/// says so. The parts are presence-driven: a lane that names no `mute` prop
/// offers no mute button, so a header stays exactly the name strip it was
/// unless a script asks for more.
///
/// `w` overrides the whole calculation, because an explicit size always wins
/// over a natural one (the layout's own rule) -- and because the *shared* indent
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
    /// The level knob's value over `[0, 1]`, when the lane offers one.
    pub level: Option<f32>,
    /// **Whether this track's automation rows are shown**, when the lane offers
    /// the toggle. `None` on a lane with no automation to show or hide -- a
    /// button for rows that do not exist is a button that does nothing.
    pub curves: Option<bool>,
    /// **What the track is producing**, one entry per channel: the level and
    /// the mark that waits, both linear amplitudes as the server's ballistics
    /// left them ([`clausters_core::measure::Ballistics`]).
    ///
    /// Empty when the lane is not metered, which is the ordinary case: a multitrack
    /// nobody is playing has no meters to read. The *length* is what the layout
    /// depends on, so a hit test builds this with the same number of silent
    /// entries the drawing reads live values into -- otherwise a press would
    /// land on pixels the strip had moved.
    pub meters: Vec<(f32, f32)>,
}

impl Header {
    /// Whether the header carries anything below its name row.
    fn has_controls(&self) -> bool {
        self.mute.is_some() || self.solo.is_some() || self.level.is_some() || self.curves.is_some()
    }

    /// What the meter strip takes off the right edge of the band, in the
    /// coordinates of `m`: one thin column per channel and a hair between them,
    /// and nothing at all when the lane is not metered.
    pub fn meter_w(&self, m: &Metrics) -> f32 {
        if self.meters.is_empty() {
            return 0.0;
        }
        let column = (m.box_side * 0.25).max(2.0);
        self.meters.len() as f32 * column + (self.meters.len() - 1) as f32 * m.divider_w + m.pad
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
        let toggles = [self.mute, self.solo, self.curves]
            .iter()
            .filter(|t| t.is_some())
            .count();
        let controls = toggles + usize::from(self.level.is_some());
        m.header_w
            .max(controls as f32 * (m.box_side + m.pad) + 2.0 * m.pad)
            + self.meter_w(m)
    }
}

/// A header's parts, laid out inside its band. A part is `None` when the lane
/// does not offer it **or** when the band is too small to draw it -- a short
/// lane keeps its name and drops the controls, the way a natural size degrades
/// everywhere else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderParts {
    pub label: Rect,
    pub mute: Option<Rect>,
    pub solo: Option<Rect>,
    pub level: Option<Rect>,
    /// The automation toggle, when the lane offers it.
    pub curves: Option<Rect>,
    /// The meter strip along the right edge, when the lane is metered. Not a
    /// [`HeaderPart`]: it is the one thing in the band a hand cannot press, so
    /// a press over it falls through to the band itself.
    pub meters: Option<Rect>,
}

/// One of a header's interactive parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderPart {
    Mute,
    Solo,
    Level,
    /// **The automation toggle**: show or hide the rows under this track.
    ///
    /// A facility rather than a design -- which rows a multitrack shows is the
    /// multitrack's, and reaching every one of them from a track's header is the
    /// shortest thing that makes an arrangement with automation readable while
    /// the rules that replace it are worked out.
    Curves,
    /// **The bottom edge of the band**, where a drag resizes the row -- the
    /// vertical zoom of one track, which is what a hand reaches for when one
    /// take needs to be read closely and the rest do not.
    Edge,
    /// **The band itself**, where no control is -- what makes the track
    /// pointable at. A header is a surface and not just a shelf for three
    /// buttons: the space beside them is how a track is selected, and how one
    /// is asked for.
    Body,
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
    // **The strip comes off the right before anything is laid out**, so the
    // name and the controls have the width that is actually theirs -- a meter
    // drawn over a name would be a meter drawn over a name.
    let strip = header.meter_w(m);
    let meters = (strip > 0.0 && inner.w > strip).then(|| {
        Rect::new(
            inner.x + inner.w - strip + m.pad,
            inner.y,
            strip - m.pad,
            inner.h,
        )
    });
    let inner = Rect::new(
        inner.x,
        inner.y,
        if meters.is_some() {
            inner.w - strip
        } else {
            inner.w
        },
        inner.h,
    );
    let name_h = font::height(m.text_scale);
    let label = Rect::new(inner.x, inner.y, inner.w, name_h.min(inner.h));
    let mut parts = HeaderParts {
        label,
        mute: None,
        solo: None,
        level: None,
        curves: None,
        meters,
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
        // **A knob, not a groove.** A header is a narrow band beside a lane and
        // a horizontal groove long enough to be read takes the width the name
        // needs -- so the level control is a dial, which reads and turns in the
        // space a header actually has, and it takes a square cell like the two
        // toggles beside it.
        parts.level = square(&mut x);
    }
    if header.curves.is_some() {
        parts.curves = square(&mut x);
    }
    let _ = right;
    parts
}

/// The header part under `(x, y)`, if any -- the press's read of
/// [`header_parts`].
pub fn header_hit(band: Rect, header: &Header, m: &Metrics, x: f64, y: f64) -> Option<HeaderPart> {
    let parts = header_parts(band, header, m);
    let over = |r: Option<Rect>| r.is_some_and(|r| r.contains(x, y));
    // The edge first: it is a strip along the bottom of the band, and a control
    // that reached into it would take the press that resizes the row.
    if band.contains(x, y) && y >= f64::from(band.y + band.h) - f64::from(EDGE_PX) {
        Some(HeaderPart::Edge)
    } else if over(parts.mute) {
        Some(HeaderPart::Mute)
    } else if over(parts.solo) {
        Some(HeaderPart::Solo)
    } else if over(parts.level) {
        Some(HeaderPart::Level)
    } else if over(parts.curves) {
        Some(HeaderPart::Curves)
    } else if band.contains(x, y) {
        Some(HeaderPart::Body)
    } else {
        None
    }
}

/// **How deep the resize strip along a header's bottom edge is**, in logical
/// pixels -- the same allowance a box's own edges have, since it is the same
/// question: how close to an edge a hand has to be to mean it.
pub const EDGE_PX: f32 = 5.0;

/// **The level a vertical drag of `dy` device pixels leaves**, from the level
/// `from` the press found, clamped to `[0, 1]`.
///
/// A knob turns by a **relative** drag, and that is not a detail of the
/// drawing: a dial has no left and right end to put the pointer between, so an
/// absolute reading would jump the value to wherever the press landed. The
/// arithmetic is the one every knob in this host uses
/// ([`controls::drag_fraction_delta`]), so a header's dial and a `knob`
/// widget's turn by the same distance for the same drag.
///
/// [`controls::drag_fraction_delta`]: super::controls::drag_fraction_delta
pub fn level_after(from: f32, dy: f64, cell: Rect) -> f32 {
    (from + super::controls::drag_fraction_delta(dy, cell.h)).clamp(0.0, 1.0)
}

/// Draws a header's controls into `band` (the name is drawn by [`draw`], which
/// owns the ellipsis against the band it actually got).
fn draw_header_controls(d: &mut Draw, band: Rect, header: &Header) {
    let (mesh, m, theme) = d.parts();
    let parts = header_parts(band, header, m);
    draw_meter_strip(mesh, m, theme, parts.meters, &header.meters);
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
    // **`A` for the automation under this track**, lit when its rows are shown
    // -- the toggle reads as on when there is something to see, the way the two
    // beside it read as on when they are doing something.
    toggle(parts.curves, header.curves == Some(true), "A", theme.accent);
    if let (Some(r), Some(level)) = (parts.level, header.level) {
        // The same dial a `knob` widget draws, and deliberately: a control that
        // read one way here and another way there would be two controls.
        let radius = (r.w.min(r.h) * 0.5 - 1.0).max(2.0);
        super::controls::knob_dial(
            &mut Draw::new(mesh, m, theme),
            r.x + r.w * 0.5,
            r.y + r.h * 0.5,
            radius,
            level,
        );
    }
}

/// **A track shows what it produces**: one column per channel down the right
/// edge of the header, over the amplitude the track is making *after
/// everything has been applied* -- its clips' gains, its curves and its fader.
///
/// It is the one place in a multitrack where the picture is of the **sound** rather
/// than of the description, which is why it is worth the strip: everything else
/// in a header says what was asked for, and this says what came out.
///
/// The column stands in decibels ([`clausters_core::measure::meter_fraction`])
/// and is drawn by [`meters::draw_column`], the one column in this host -- so a
/// track's meter, a `meter` widget and a mixer's strip read alike, down to
/// where the green becomes red.
fn draw_meter_strip(
    mesh: &mut crate::host::paint::Mesh,
    m: &Metrics,
    theme: &crate::host::theme::Theme,
    rect: Option<Rect>,
    channels: &[(f32, f32)],
) {
    let Some(rect) = rect else { return };
    if channels.is_empty() || rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let gaps = (channels.len() - 1) as f32 * m.divider_w;
    let column = ((rect.w - gaps) / channels.len() as f32).max(1.0);
    let scale = meters::Scale::decibels();
    for (i, &(level, mark)) in channels.iter().enumerate() {
        let x = rect.x + i as f32 * (column + m.divider_w);
        meters::draw_column(
            mesh,
            m,
            theme,
            Rect::new(x, rect.y, column, rect.h),
            measure::meter_fraction(level, measure::METER_FLOOR_DB),
            measure::meter_fraction(mark, measure::METER_FLOOR_DB),
            scale,
        );
    }
}

/// The lane body of a track's `rect`: the part right of the header band, and
/// above the time-ruler strip when the lane draws one (`ruler`). The renderer
/// and the hit-test both call this, so a clip occupies the same pixels either
/// way -- pass the same flag (a lane with `Ruler::Off` reserves no strip, which
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
/// **A clip that is on screen is drawn, however short it is** -- as a *line*
/// when it gets that short. A span thinner than `min_w` is widened to it (kept
/// inside the body, so a clip at the far edge grows leftwards instead of hanging
/// out), and `min_w` is the **hairline** every drawn line in the host uses,
/// nothing more: the alternative is a clip that exists, plays and is addressable
/// but occupies no pixel -- nothing to see, nothing to grab, and no way back
/// except guessing where to zoom.
///
/// **The floor is a hairline and not a grabbable width**, which is the whole
/// difference. A floor wide enough to aim at (a grip's worth) is a floor that
/// *lies about the length*: the clip stops narrowing as the reader zooms out and
/// stops widening as they zoom in, so the picture says "this clip is about that
/// long" at every scale and the one thing a timeline exists to show is the one
/// thing it stops showing. A hairline says only "a clip is here" -- the line
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
/// occupies (`clip_x_range`) -- the renderer and the hit-test both call it, so a
/// clip's body is edited on the pixels it is drawn on.
pub fn clip_rect(body: Rect, x0: f32, x1: f32) -> Rect {
    Rect::new(x0, body.y + 1.0, x1 - x0, (body.h - 2.0).max(0.0))
}

/// A clip's **own** time axis: the part of `[0, dur]` its drawn rectangle `cr`
/// shows, in clip-local units. A clip rectangle is clamped to the lane body, so
/// a clip half-scrolled off the left is drawn starting at some `t > 0` -- this is
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

/// The clip-local time an x pixel of `cr` falls on -- the inverse of [`local_x`].
fn local_t(cr: Rect, local: &View, x: f64) -> f64 {
    local.start + local.len * (x - cr.x as f64) / cr.w.max(1.0) as f64
}

/// Draws one track lane into `rect`: the header (with `label` and its
/// controls) and the lane field. **Not** its clips -- those are widgets the
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
    selected: bool,
) {
    let (mesh, m, theme) = d.parts();
    // The header band on the left -- the group's indent, so every member of the
    // axis starts its body at the same x. What the lane puts in that band is
    // its own (a name, and the controls it offers).
    let band = timeline::gutter_band(rect, indent);
    mesh.rect(band, theme.header);
    // **A selected track says so on its header**, in the two roles every
    // selection in this host is drawn in: the wash a held clip has, and its
    // edge. It is a wash over the band rather than a replacement of it, so the
    // name and the three controls go on reading as themselves.
    if selected {
        mesh.rect(band, theme.selected_fill);
        mesh.border(band, m.divider_w, theme.selected_edge);
    }
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
///
/// A **selected** clip is drawn in the selection's own roles, the same two a
/// selected note is drawn in (`pianoroll::draw_notes`): one hand holding
/// several boxes looks the same whichever boxes they are.
pub fn draw_clip(d: &mut Draw, cr: Rect, selected: bool) {
    let (mesh, m, theme) = d.parts();
    let (fill, edge) = if selected {
        (theme.selected_fill, theme.selected_edge)
    } else {
        (theme.object_fill, theme.object_edge)
    };
    mesh.rect(cr, fill);
    mesh.border(cr, m.divider_w, edge);
}

/// Which **ends** of a clip are on screen, read off the clip's own axis: the
/// slice of `[0, dur]` its drawn rectangle shows. A clip scrolled half off the
/// left is drawn starting at some `t > 0`, and its start is not on screen at
/// all -- the left edge of its rectangle is the *window's* edge, not the clip's.
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
/// the strip that resizes -- the rule every other part of this module follows.
/// **A clip too narrow for two grips keeps one**, and it is the one that gets
/// the reader out of the corner: its **end**, the edge that lengthens it (the
/// start when the end is the one off screen). Two strips on a rectangle that
/// cannot hold them would overlap, so the press could not tell them apart --
/// but returning neither left a clip shrunk to a sliver with no affordance at
/// all, movable and never growable. One grip keeps every state reversible.
///
/// The one grip is **as wide as the clip and no wider**, down to the hairline a
/// collapsed clip is drawn as ([`clip_x_range`]): a grip is a promise the press
/// keeps, so it can only be offered on pixels the press can be given. A clip
/// drawn as a line therefore carries its expand grip *on the line* -- enough to
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

/// The grip a pointer at `cursor_x` is **on** -- the strip under the cursor, and
/// `None` anywhere else on the clip.
///
/// **An affordance is drawn where it acts, and nowhere else.** This used to
/// light the grip of whichever *half* the pointer was in, so a strip a dozen
/// pixels wide announced itself from the middle of the clip -- and then the
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
/// rather than asking where the pointer is -- a drag already holding an edge.
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

/// Draws one clip grip -- the affordance for the resize gesture, shown while the
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
/// says which, the arrow is where that is announced -- an outward chevron for
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

/// **The mark that a clip is held**, into whichever mesh is painted over its
/// bodies.
///
/// It is drawn twice on purpose and neither is redundant: [`draw_clip`] paints
/// the held *fill* under the bodies, where it shows through an empty clip and
/// through the air around a trace, and this paints the held *edge* over them.
/// The reason is the one the name already gives: a body drawn over the box
/// covers whatever the box drew, and the time-frequency texture -- which is not
/// mesh at all but a GPU pass after every mesh -- covers it outright. So a
/// spectral clip could be taken by a marquee, moved, and never look taken;
/// which of the two happened is exactly what a hand cannot tell.
pub fn draw_clip_selection(d: &mut Draw, cr: Rect) {
    let (mesh, m, theme) = d.parts();
    mesh.border(cr, m.divider_w, theme.selected_edge);
}

/// Draws a clip's **name**, into whichever mesh is painted over its bodies.
///
/// It is a separate call because a name has to read: drawn with the box, the
/// take's trace goes over it, and the time-frequency texture -- which is not
/// mesh at all but a GPU pass after every mesh -- hides it outright. So the box
/// is the base mesh's and the name is the overlay's, the same split the
/// playhead and the selection already take.
///
/// The name is **kept inside the box it names**: `cr` is the clip's *visible*
/// rectangle (the span clamped to the lane), so a name written at its own
/// length runs out of a clip narrower than the string -- over the neighbour that
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

/// Draws one clip **body** -- a child element of a clip -- into the clip's
/// rectangle, against the clip's own axis. This is the whole of what "a clip is
/// a container" comes to: the element says what it is, the container says where it
/// is, and neither knows about the lane, the group's window or the clip's
/// offset on it.
///
/// The bodies **layer**, back to front, because that is the order the layout
/// placed them in: the take, the events over it, the envelope over both -- an
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
/// `None` where the window is off the contents -- a clip stretched past the end
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
/// ([`Trace`]) -- a loaded take answers from its peak pyramid, an inline sketch
/// straight off its slice, and the drawing is the same either way.
///
/// The body is drawn **from the source, per visible pixel**, mapped back
/// through the clip's own axis, which is what makes it scroll and stretch with
/// the view instead of squashing into whatever slice is on screen. Never
/// resolves finer than the screen -- the one graphics rule.
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
    layers: &Stack,
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
    // stacks its rows: a clip is a picture of the contents and a stereo take
    // whose right channel is nowhere on it is a picture of half of one -- which
    // is also what an edit on that channel would land in, invisibly. `overlay`
    // is the same choice the standalone view offers, and it arrives the same
    // way (the element's own prop), so the two never disagree about what a
    // channel is.
    let rows = if overlay { 1 } else { trace.channels().max(1) };
    // **The window is drawn a run at a time**, each run a stretch of clip time
    // over which it stays inside the contents -- one run for the ordinary case,
    // one per iteration for a looping clip, and none at all where a clip
    // reaches past contents it does not loop. Each run is an *affine* window,
    // which is what lets one renderer draw all of them: the wrap lives in the
    // run list rather than in the maps, so nothing downstream has to know
    // whether a clip loops.
    let runs = window.runs(local.start, local.start + local.len, dur, total);
    for ch in 0..trace.channels().max(1) {
        let row = crate::host::frame::channel_rect(cr, rows, if overlay { 0 } else { ch });
        if row.h <= 0.0 {
            continue;
        }
        let y_at = move |v: f32| row.y + row.h * (1.0 - fraction(v, min, max));
        // The line and the fill read one rule, so a take cannot be filled to a
        // baseline that was never drawn (or drawn one it does not reach).
        if let Some(b) = crate::waveform::baseline_of(min, max) {
            let y = y_at(b);
            mesh.line([row.x, y], [row.x + row.w, y], m.divider_w, theme.baseline);
        }
        for &(from, to, source0) in &runs {
            // The run's own rectangle: the pixels its stretch of clip time
            // covers, which is what bounds the drawing to it.
            let (x0, x1) = (local_x(cr, local, from), local_x(cr, local, to));
            let run_rect = Rect::new(x0, row.y, (x1 - x0).max(0.0), row.h);
            if run_rect.w < 0.5 {
                continue;
            }
            // Inside a run the window is affine, whichever kind it is: the
            // source frame at its start plus the time since, or the fitted
            // mapping over the whole span.
            // ...at `rate` frames of source per unit of clip time, which is
            // one for a source written at the rate the clip is measured in and
            // the ratio between the two otherwise.
            let rate = window.frames_per_unit();
            let src = move |x: f32| match window.fit {
                true => (local_t(cr, local, x as f64) / dur * total).clamp(0.0, total),
                false => source0 + (local_t(cr, local, x as f64) - from) * rate,
            };
            let x_of = move |s: f64| match window.fit {
                true => local_x(cr, local, s / total * dur),
                false => local_x(cr, local, from + (s - source0) / rate),
            };
            // **The samples this run draws are the placement's, not the
            // source's.** Where a box reads its source at another rate the two
            // are different grids: the engine resamples as it plays, so what it
            // produces is the reconstruction read on the box's own samples, and
            // those are the dots worth drawing. The grid is stated from the
            // box's zero rather than from this run, so panning does not slide
            // the dots by a fraction of a sample.
            let grid = (rate - 1.0).abs().gt(&1e-9).then(|| {
                let first = source0 - from.rem_euclid(1.0) * rate;
                (first, rate)
            });
            // One picture per measure, the envelope first and the level body
            // inside it.
            for (measure, alpha) in layers.drawn_measures() {
                trace::draw_channel(
                    mesh,
                    run_rect,
                    trace,
                    ch,
                    src,
                    x_of,
                    y_at,
                    TraceStyle::new(
                        crate::host::theme::with_alpha(
                            trace::measure_color(theme, measure, theme.selection),
                            alpha,
                        ),
                        m.divider_w,
                    )
                    .with_dots(m.point_radius)
                    .with_measure(measure)
                    .with_rate(sample_rate)
                    .with_grid(grid)
                    .with_written(written),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

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
        assert!(parts.mute.is_some() && parts.solo.is_some() && parts.level.is_some());
        // ...and a compact table sizes it down, not the other way round: the
        // roles move together, so the parts still fit.
        let compact = Metrics::generated(0.8);
        let band = Rect::new(0.0, 0.0, full.width(&compact), 60.0);
        assert!(header_parts(band, &full, &compact).level.is_some());
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
        assert!(parts.mute.is_some() && parts.solo.is_some() && parts.level.is_some());
        // A lane too short for a second row keeps the name and nothing else.
        let short = header_parts(Rect::new(0.0, 0.0, band.w, 16.0), &header, &m);
        assert_eq!((short.mute, short.solo, short.level), (None, None, None));
        assert!(short.label.h > 0.0);
        // ...and so does one too narrow for the fader, which is dropped rather
        // than drawn as a stub.
        let narrow = header_parts(Rect::new(0.0, 0.0, 60.0, 60.0), &header, &m);
        assert!(narrow.mute.is_some() && narrow.level.is_none());
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

        // **A clip on screen is drawn however short it is** -- as the line the
        // floor is. A hundredth of a sample is a fortieth of a pixel here: with
        // no floor the rectangle is geometry the rasterizer has nothing to put
        // down, which is a clip that plays, answers a query and occupies no
        // pixel. The floor the layout passes is the hairline, so what comes
        // back marks where the clip is and claims nothing about its length --
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
    /// tests below draw bodies exactly as the frame does -- through
    /// `(rect, local)` and nothing else.
    fn placed(offset: f64, dur: f64, nav: &View) -> (Rect, View) {
        let m = Metrics::default();
        let body = lane_body(lane(), false, m.header_w, &m);
        let (x0, x1) = clip_x_range(body, nav, offset, dur, m.grip_w).expect("the clip is visible");
        let cr = clip_rect(body, x0, x1);
        (cr, clip_local_view(body, nav, offset, dur, cr))
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
            false,
        );
        assert!(!m.is_empty(), "the header and the lane field draw");

        // ...and a clip's box is drawn from its own placement.
        let before = m.vertex_count();
        let (cr, _) = placed(0.0, 100.0, &View::full(400));
        draw_clip(
            &mut Draw::new(&mut m, &metrics, &Theme::default()),
            cr,
            false,
        );
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

    #[test]
    fn a_body_reads_the_source_through_the_axis_under_zoom_and_pan() {
        // The bug this pins: a partially visible clip must draw the *part of its
        // take that is on screen*, not squash the whole take into the visible
        // sliver -- so a pixel maps back through the axis to the source.
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
}
