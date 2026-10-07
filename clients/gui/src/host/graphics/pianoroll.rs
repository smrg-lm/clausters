//! The piano-roll **drawing**: a note grid, a piano keyboard gutter, a
//! velocity lane and the OSC markers, all pure over a [`Draw`] (the
//! flat-geometry [`crate::host::paint`] painter) so they are unit-testable without a
//! window -- the static-view posture of `track`/`bpf`.
//!
//! What a note **is**, and what a hand does to a list of them, is
//! [`structures::notes`](crate::host::structures::notes); this is where those
//! numbers meet pixels. The split is the layer rule: a module named for drawing
//! holds the drawing, and the structure is shared by every application that has
//! notes in it rather than by whoever draws them first.
//!
//! This module is **shared by two consumers**, on the crate's standing rule
//! that a model and its hit-test primitives are extracted once and reused --
//! the same way `points::place_point`/`insert_point` serve both the `bpf` widget
//! and the automation clip:
//!
//! - the dedicated **`pianoroll` widget** -- an editor-grade view with a
//!   keyboard, rulers, group navigation, selection and a playhead, drawing MIDI
//!   notes in the grid and OSC markers in their lane;
//! - the multitrack **`clip` body** -- a clip with `notes` draws its compact
//!   piano-roll by calling [`draw_notes`] on the clip's rect, so a note lines up
//!   on the shared time axis and the two never disagree on geometry.
//!
//! Everything here is **display logic** (pixel mapping, hit-testing, drag
//! clamps): it stays gui-side per the placement rule. The one multitrack of general
//! musical knowledge -- the MIDI-note <-> name/black-key spelling drawn on the
//! keyboard and the pitch ruler -- lives in `clausters_core::scale`.

use clausters_core::scale;

use crate::host::bands::Bands;
use crate::host::font;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::structures::boxes::{self, Part};
use crate::host::structures::notes::{Note, OscMarker};
use crate::viewport::View;

// --- Layout ---------------------------------------------------------------

/// The keyboard gutter a roll asks for, device pixels -- its *own* structural
/// geometry. What it actually gets is its navigation group's shared indent
/// (`crate::host::timeline::group_indent`), which is this when the roll is alone on
/// its axis and wider when it shares one with a lane.
pub const KEYBOARD_W: f32 = 44.0;
/// The OSC markers' strip height, device pixels.
pub const OSC_H: f32 = 16.0;
/// The smallest note bar height (a note never collapses below this even when a
/// semitone row is sub-pixel).
const NOTE_MIN_H: f32 = 2.0;

/// The regions of a `pianoroll` widget rect: the keyboard gutter (left), the
/// note grid, and the optional OSC / time-ruler strips stacked at the
/// bottom. The renderer and the hit-test both call this, so a note occupies the
/// same pixels either way.
#[derive(Clone, Copy, Debug)]
pub struct Regions {
    pub keyboard: Rect,
    pub grid: Rect,
    pub osc: Rect,
    pub ruler: Rect,
}

/// Split a widget rect into its piano-roll regions. `osc` reserves its strip
/// only when on; `ruler_on` reserves the bottom time strip.
/// `indent` is the group's shared gutter -- the keyboard fills it, so the grid
/// starts where every other member of the axis starts its body.
pub fn regions(rect: Rect, ruler_on: bool, osc_on: bool, indent: f32, m: &Metrics) -> Regions {
    let kw = indent.min(rect.w);
    let rh = if ruler_on { m.ruler_h.min(rect.h) } else { 0.0 };
    let oh = if osc_on { OSC_H } else { 0.0 };
    // Reserve from the bottom up: ruler, osc, then the grid.
    let inner_h = (rect.h - rh).max(0.0);
    let body_x = rect.x + kw;
    let body_w = (rect.w - kw).max(0.0);
    let grid_h = (inner_h - oh).max(0.0);
    let grid = Rect::new(body_x, rect.y, body_w, grid_h);
    let osc = Rect::new(body_x, rect.y + grid_h, body_w, oh);
    let ruler = Rect::new(body_x, rect.y + inner_h, body_w, rh);
    let keyboard = Rect::new(rect.x, rect.y, kw, grid_h);
    Regions {
        keyboard,
        grid,
        osc,
        ruler,
    }
}

// --- Mapping --------------------------------------------------------------

/// The x pixel a timeline sample position falls on, through the shared `nav`
/// window and the grid `body`.
pub(crate) fn to_x(s: f64, nav: &View, body: Rect) -> f64 {
    body.x as f64 + (s - nav.start) / nav.len.max(1.0) * body.w as f64
}

/// **A roll's vertical axis**: the pitch window `[lo, hi]` and what a note is
/// on it.
///
/// **Rows** are the keys: a note is a semitone band, the window shows every
/// whole row `lo..=hi` -- so the pixels span `[lo - 0.5, hi + 0.5]` and the
/// extreme rows draw in full -- and a note sits on the row of its **nearest
/// key**, with whatever it is off that key drawn inside the box as a line at its
/// exact pitch: the bend a MIDI note-on needs beside the key to sound it. A
/// **line** is a continuous axis (hertz): the window is the pixels, edge to
/// edge, and a note is a bar of a fixed height centred on its exact pitch,
/// since a row means nothing there and a bar that grew with the zoom would
/// hide where its frequency is.
///
/// Both map a pitch and a pixel linearly -- a log frequency is a linear pitch
/// -- so the two rolls over one sequence draw a note at the same height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pitches {
    /// The lowest pitch of the window.
    pub lo: f32,
    /// The highest.
    pub hi: f32,
    /// A line's bar height in pixels; `None` for rows.
    pub bar: Option<f32>,
}

impl Pitches {
    /// Semitone rows over `[lo, hi]`: the keys.
    pub fn rows(lo: f32, hi: f32) -> Self {
        Self { lo, hi, bar: None }
    }

    /// A continuous axis over `[lo, hi]`, a note a bar `bar` pixels high.
    pub fn line(lo: f32, hi: f32, bar: f32) -> Self {
        Self {
            lo,
            hi,
            bar: Some(bar),
        }
    }

    /// Whether this is a continuous axis.
    pub fn is_line(&self) -> bool {
        self.bar.is_some()
    }

    /// How far past each end of the window the pixels reach, in pitch.
    fn pad(&self) -> f32 {
        if self.is_line() { 0.0 } else { 0.5 }
    }

    /// The pitch the grid's height spans.
    pub fn span(&self) -> f32 {
        let span = self.hi - self.lo + 2.0 * self.pad();
        if self.is_line() {
            span.max(1e-3)
        } else {
            span.max(1.0)
        }
    }

    /// A pitch's y pixel, **unclamped**: outside the window it maps above or
    /// below `grid`, and whatever is drawn there is cut, not slid back in.
    pub fn y(&self, pitch: f32, grid: Rect) -> f32 {
        grid.y + (self.hi + self.pad() - pitch) * grid.h / self.span()
    }

    /// The pitch a y pixel is, clamped into the pixels the grid shows -- the
    /// inverse of [`Self::y`], so a drop lands where it is drawn, down to the
    /// grid's very edges.
    pub fn pitch(&self, y: f32, grid: Rect) -> f32 {
        let frac = ((y - grid.y) / grid.h.max(f32::EPSILON)).clamp(0.0, 1.0);
        self.hi + self.pad() - frac * self.span()
    }

    /// Where a note of `pitch` is placed: the row of its nearest key, or its
    /// own pitch on a line.
    pub fn anchor(&self, pitch: f32) -> f32 {
        if self.is_line() { pitch } else { pitch.round() }
    }

    /// Whether any of a note of `pitch` can show: a row within a whole row of
    /// the window, since half of it is enough; a line's bar is cut where it
    /// leaves ([`visible_band`]).
    pub fn visible(&self, pitch: f32) -> bool {
        self.is_line() || {
            let key = self.anchor(pitch);
            key > self.lo - 1.0 && key < self.hi + 1.0
        }
    }

    /// **How tall a note is drawn and grabbed**: a row, or a line's bar. The
    /// floor wins over the ceiling: a note never collapses below `NOTE_MIN_H`,
    /// and a grid shorter than one bar cuts it (`visible_band`) rather than
    /// shrinking it -- written as a `clamp` this inverted its own range on such
    /// a grid and panicked.
    pub fn note_height(&self, grid: Rect) -> f32 {
        self.bar
            .unwrap_or(grid.h / self.span())
            .min(grid.h)
            .max(NOTE_MIN_H)
    }
}

/// **The roll's vertical axis**: one band per semitone of the window
/// `[lo, hi]`, the top band being pitch `hi`.
///
/// The window shows every whole row `lo..=hi` -- `hi - lo + 1` of them -- so the
/// pixel axis spans `[lo - 0.5, hi + 0.5]` and the extreme rows draw in full
/// instead of being clipped at the grid edges.
///
/// It is a [`Bands::Uniform`], which is the same type a multitrack's lanes are
/// a [`Bands::Table`] of: **a semitone row and a lane are one structure**, and
/// they differ only in whether every band is the same height. A chromatic roll
/// says they are, and the arm that says so holds no memory at all.
pub fn bands(lo: f32, hi: f32, grid: Rect) -> Bands {
    let rows = (hi - lo + 1.0).max(1.0);
    Bands::uniform(rows as usize, grid.h / rows)
}

/// The height in pixels of one semitone row over the pitch window `[lo, hi]`.
pub fn row_height(lo: f32, hi: f32, grid: Rect) -> f32 {
    grid.h / Pitches::rows(lo, hi).span()
}

/// The integer pitches whose rows show in the window `[lo - 0.5, hi + 0.5]` --
/// what everything drawn *per row* iterates, so the bands, the dividers, the
/// keys and the labels are the same set of rows.
fn rows_in_view(lo: f32, hi: f32) -> std::ops::RangeInclusive<i32> {
    lo.floor() as i32..=hi.ceil() as i32
}

/// The part of a bar of height `h` centred on `yc` that falls inside `grid`,
/// as `(y, height)` -- `None` when none of it does. The vertical counterpart of
/// the note's horizontal clamp to the grid bounds.
fn visible_band(yc: f32, h: f32, grid: Rect) -> Option<(f32, f32)> {
    let y = (yc - h * 0.5).max(grid.y);
    let bottom = (yc + h * 0.5).min(grid.y + grid.h);
    (bottom > y).then_some((y, bottom - y))
}

/// Whether any part of pitch `p`'s row shows in the window `[lo - 0.5, hi + 0.5]`.
///
/// The row of `p` spans `[p - 0.5, p + 0.5]`, so it is in view while `p` is
/// within **a whole row** of the window's ends -- half of it is enough. Asking
/// for the row's *centre* to be inside instead drops a note the moment it is
/// half cut, which is exactly when it should still be half drawn.
///
/// The horizontal axis says this by construction -- a note off the time window
/// clamps to a zero-width span and is skipped -- while the vertical one has to
/// be asked.
pub fn pitch_visible(p: f32, lo: f32, hi: f32) -> bool {
    Pitches::rows(lo, hi).visible(p)
}

/// A pitch's y pixel (its row centre), **unclamped**: a pitch outside the
/// window maps above or below `grid` instead of onto its edge. Whatever is
/// placed *on* a row -- a note bar -- wants this one and cuts itself against the
/// grid, because a row leaving the view is cut, not slid back in.
pub fn row_center(pitch: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    // The band the pitch sits on, measured from the grid's top: pitch `hi` is
    // band 0. Deliberately **not** `Bands::at`, which clamps to the stack -- a
    // row leaving the view is cut where it is, not slid back in, and the caller
    // is the one that cuts it (`visible_band`).
    Pitches::rows(lo, hi).y(pitch, grid)
}

/// A pitch's y pixel (its row centre) **inside** `grid`: high pitch at the top.
/// The axis spans `[lo - 0.5, hi + 0.5]`, so pitch `hi` centres half a row
/// below the top edge and pitch `lo` half a row above the bottom -- every row is
/// fully visible. Clamped to the grid, which is what the chrome painted *per
/// row* wants (the shaded bands, the keyboard keys, the C labels): those are
/// drawn for the rows in view and must not bleed into the strip above or below.
pub fn pitch_to_y(pitch: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    row_center(pitch, lo, hi, grid).clamp(grid.y, grid.y + grid.h)
}

/// The (fractional) pitch a y pixel maps to over the `[lo - 0.5, hi + 0.5]`
/// window -- the inverse of [`pitch_to_y`], so a drop lands on the row it is
/// drawn on.
///
/// Continuous, and down to the grid's edges: counting whole bands instead
/// truncated a fractional window's rows, so the stretch past the last whole
/// one could never be reached by a drag.
pub fn y_to_pitch(y: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    Pitches::rows(lo, hi).pitch(y, grid)
}

// --- Drawing --------------------------------------------------------------

/// The grid background: black-key rows shaded, semitone lines, and a brighter
/// line at each octave (every C). `lo`/`hi` are the visible MIDI pitch window.
pub fn draw_grid_background(d: &mut Draw, grid: Rect, lo: f32, hi: f32) {
    let (mesh, m, theme) = d.parts();
    if grid.w <= 0.0 || grid.h <= 0.0 {
        return;
    }
    mesh.rect(grid, theme.lane);
    let rh = row_height(lo, hi, grid);
    // One shaded band per black-key semitone, plus a divider at each row and a
    // brighter one at each octave boundary (C). Iterate integer pitches in view.
    for p in rows_in_view(lo, hi) {
        let yc = row_center(p as f32, lo, hi, grid);
        // The band is the row's own slice of the window, cut where the grid
        // ends -- never slid inside it, which would stack the rows above the
        // window onto the top one and put the shading out of step with the keys.
        if scale::is_black_key(p)
            && rh >= 1.0
            && let Some((y, h)) = visible_band(yc, rh, grid)
        {
            mesh.rect(Rect::new(grid.x, y, grid.w, h), theme.lane_alt);
        }
        // A divider under each row when the rows are tall enough to read, a
        // brighter one at each octave boundary (below C).
        let ly = yc + rh * 0.5;
        if rh >= 4.0 && ly >= grid.y && ly <= grid.y + grid.h {
            let line = if scale::pitch_class(p) == 0 {
                theme.frame
            } else {
                theme.grid_line
            };
            mesh.rect(Rect::new(grid.x, ly, grid.w, m.divider_w), line);
        }
    }
    mesh.border(grid, m.divider_w, theme.frame);
}

/// Draw a set of notes over the pitch window `[lo, hi]` of `grid`, placed on
/// the shared `nav` time axis (offset added, so a clip's roll moves with the
/// clip). `field` is the pixel domain the `nav` window spans horizontally -- the
/// lane body for a multitrack clip, the grid itself for the dedicated view -- and
/// each note's x clamps to `grid`'s bounds; `grid` also gives the pitch rows and
/// the note height. Passing the clip rect for both would rescale the note by the
/// clip's own width, drifting the roll off its clip under a pan/zoom. The one
/// primitive both the widget and the clip body use. When `color_velocity` the
/// note fill brightens with velocity. `selected` indices draw highlighted (the
/// multi-note selection; the clip body passes none). `axis` is where a note
/// sits and how tall it is ([`Pitches`]).
#[allow(clippy::too_many_arguments)] // one time-and-pitch mapping, all scalars
pub fn draw_notes(
    d: &mut Draw,
    field: Rect,
    grid: Rect,
    nav: &View,
    offset: f64,
    notes: &[Note],
    axis: Pitches,
    color_velocity: bool,
    selected: &[usize],
) {
    let (mesh, m, theme) = d.parts();
    if grid.w <= 0.0 || grid.h <= 0.0 {
        return;
    }
    let h = axis.note_height(grid);
    let (x_lo, x_hi) = (grid.x, grid.x + grid.w);
    for (i, n) in notes.iter().enumerate() {
        // x maps through `field` -- the pixel domain the shared `nav` spans (the
        // lane body for a clip, the grid itself for the dedicated view) -- then
        // clamps to the clip's own `grid` bounds, exactly as `track::draw_curve`
        // maps its points. Using `grid` for both would rescale the note by the
        // clip's width, so notes drifted off their clip under a pan/zoom.
        let mut nx0 = to_x(offset + n.start, nav, field) as f32;
        let mut nx1 = to_x(offset + n.start + n.dur.max(0.0), nav, field) as f32;
        nx0 = nx0.clamp(x_lo, x_hi);
        nx1 = nx1.clamp(x_lo, x_hi);
        if nx1 <= nx0 || !axis.visible(n.pitch) {
            continue;
        }
        // The bar is **cut** by the grid's edge, never pushed inside it: a
        // note on its way out of the pitch window has to leave, and one shoved
        // back in would sit on a row that is not its own.
        let Some((y, h)) = visible_band(axis.y(axis.anchor(n.pitch), grid), h, grid) else {
            continue;
        };
        let is_selected = selected.contains(&i);
        let fill = if is_selected {
            theme.selected_fill
        } else if color_velocity {
            let v = (n.velocity as f32 / 127.0).clamp(0.15, 1.0);
            [
                theme.note_fill[0] * v,
                theme.note_fill[1] * v,
                theme.note_fill[2] * v,
                1.0,
            ]
        } else {
            theme.note_fill
        };
        mesh.rect(Rect::new(nx0, y, nx1 - nx0, h), fill);
        // **The exact pitch, as a line**: a bar on a continuous axis marks its
        // centre, since the height itself means nothing; a box on its key's
        // row marks how far off the key it is -- the bend beside the key --
        // and draws nothing on the key itself.
        let bend = n.pitch - axis.anchor(n.pitch);
        if axis.is_line() || bend.abs() > 1e-3 {
            let yc = axis.y(n.pitch, grid);
            if yc >= y && yc <= y + h {
                mesh.rect(
                    Rect::new(nx0, yc - m.divider_w * 0.5, nx1 - nx0, m.divider_w),
                    theme.frame,
                );
            }
        }
        if nx1 - nx0 > 3.0 && h > 3.0 {
            let edge = if is_selected {
                theme.selected_edge
            } else {
                theme.note_edge
            };
            mesh.border(Rect::new(nx0, y, nx1 - nx0, h), m.divider_w, edge);
        }
    }
}

/// **The notes a compass cannot hold, marked at its edge**: a note whose key
/// is outside `[min, max]` -- a frequency past what a MIDI key can be, written
/// from a roll in hertz -- has no row to be drawn on, so a strip at the top or
/// the bottom of the grid, as long as the note, says it is there.
#[allow(clippy::too_many_arguments)] // one time-and-pitch mapping, all scalars
pub fn draw_out_of_range(
    d: &mut Draw,
    grid: Rect,
    nav: &View,
    offset: f64,
    notes: &[Note],
    axis: Pitches,
    compass: (f32, f32),
) {
    let (mesh, m, theme) = d.parts();
    let strip = (m.divider_w * 3.0).max(3.0);
    for n in notes {
        let key = axis.anchor(n.pitch);
        let y = if key > compass.1 {
            grid.y
        } else if key < compass.0 {
            grid.y + grid.h - strip
        } else {
            continue;
        };
        let x0 = (to_x(offset + n.start, nav, grid) as f32).clamp(grid.x, grid.x + grid.w);
        let x1 = (to_x(offset + n.start + n.dur.max(0.0), nav, grid) as f32)
            .clamp(grid.x, grid.x + grid.w);
        if x1 > x0 {
            mesh.rect(Rect::new(x0, y, x1 - x0, strip), theme.selected_edge);
        }
    }
}

/// Label each C row at the left edge of a roll body -- the compact pitch ruler
/// for a roll drawn **without** a keyboard gutter (the multitrack `clip`'s
/// body; the dedicated widget names its Cs on the keyboard instead). Draws
/// only when a semitone row is tall enough to read a label.
pub fn draw_pitch_labels(d: &mut Draw, grid: Rect, lo: f32, hi: f32) {
    let (mesh, m, theme) = d.parts();
    if grid.w <= 0.0 || grid.h <= 0.0 {
        return;
    }
    let rh = row_height(lo, hi, grid);
    if rh < font::height(m.micro_scale) + 2.0 {
        return;
    }
    for p in rows_in_view(lo, hi) {
        if scale::pitch_class(p) == 0 {
            let top = row_center(p as f32, lo, hi, grid) - rh * 0.5;
            if top < grid.y || top + rh > grid.y + grid.h {
                continue; // the row is half out: its label would not fit in it
            }
            font::text(
                mesh,
                &scale::note_name(p),
                grid.x + 2.0,
                top + 1.0,
                m.micro_scale,
                theme.key_label_dim,
            );
        }
    }
}

/// Draw the keyboard gutter: a white/black key per semitone row, with a note
/// name on each C. `lo`/`hi` are the same pitch window as the grid.
pub fn draw_keyboard(d: &mut Draw, gutter: Rect, lo: f32, hi: f32) {
    let (mesh, m, theme) = d.parts();
    if gutter.w <= 0.0 || gutter.h <= 0.0 {
        return;
    }
    let rh = row_height(lo, hi, gutter);
    for p in rows_in_view(lo, hi) {
        let Some((top, rh)) = visible_band(row_center(p as f32, lo, hi, gutter), rh, gutter) else {
            continue;
        };
        let color = if scale::is_black_key(p) {
            theme.key_black
        } else {
            theme.key_white_dim
        };
        let h = rh.max(1.0).min(gutter.h);
        mesh.rect(Rect::new(gutter.x, top, gutter.w, h), color);
        // Name every C when there is room for the label.
        if scale::pitch_class(p) == 0 && rh >= font::height(m.micro_scale) + 2.0 {
            font::text(
                mesh,
                &scale::note_name(p),
                gutter.x + 2.0,
                top + 1.0,
                m.micro_scale,
                theme.key_label_dim,
            );
        }
    }
    mesh.border(gutter, m.divider_w, theme.frame);
}

// --- A roll in hertz ---------------------------------------------------------
//
// **A frequency on a log scale is a pitch on a linear one**: `midinote = 69 +
// 12 * log2(hz / 440)`. So a roll whose Y domain is hertz keeps every row,
// band and hit-test above in the MIDI coordinate -- where a note an octave up
// is twelve rows up either way -- and only what labels the axis changes: a
// ruler of round frequencies instead of the keys, and lines at those
// frequencies instead of the semitone rows. The conversion sits at the wire
// (the element reads and reports hertz), and the rows are continuous there,
// since a frequency has no semitone to snap to.

/// The pitch coordinate the hertz ruler is laid over, lowest and highest:
/// about 1 Hz to 84 kHz, so any window a roll shows is inside it. It is a
/// fixed frame because the ruler's log mapping is stated over a whole axis
/// (`ruler::hz_ticks`), and the roll's window is a slice of it.
const HZ_FLOOR: f64 = -36.0;
const HZ_CEIL: f64 = 159.0;

/// The round frequencies to mark over the pitch window `[lo, hi]` of a strip
/// `height` pixels tall, each as the pitch it sits at and its label when it
/// has room for one -- the spectrogram's own ruler, over the roll's window.
pub fn hz_ticks(axis: Pitches, height: f32, m: &Metrics) -> Vec<(f32, Option<String>)> {
    let span = HZ_CEIL - HZ_FLOOR;
    let top = f64::from(axis.hi) + f64::from(axis.pad());
    let bottom = top - f64::from(axis.span());
    let nyquist = scale::midi_to_hz(HZ_CEIL);
    let floor = scale::midi_to_hz(HZ_FLOOR) / nyquist;
    crate::host::ruler::hz_ticks(
        nyquist,
        crate::spectrogram::FreqScale::Log,
        floor,
        f64::from(height),
        (bottom - HZ_FLOOR) / span,
        (top - bottom) / span,
        m,
    )
    .into_iter()
    .map(|tick| ((bottom + tick.frac * (top - bottom)) as f32, tick.label))
    .collect()
}

/// The grid of a roll in hertz: a line at each round frequency, brighter where
/// it is labelled, over the roll's `axis`.
pub fn draw_hz_grid(d: &mut Draw, grid: Rect, axis: Pitches) {
    let ticks = hz_ticks(axis, grid.h, d.parts().1);
    let (mesh, m, theme) = d.parts();
    if grid.w <= 0.0 || grid.h <= 0.0 {
        return;
    }
    mesh.rect(grid, theme.lane);
    for (pitch, label) in ticks {
        let y = axis.y(pitch, grid);
        if y < grid.y || y > grid.y + grid.h {
            continue;
        }
        let line = if label.is_some() {
            theme.frame
        } else {
            theme.grid_line
        };
        mesh.rect(Rect::new(grid.x, y, grid.w, m.divider_w), line);
    }
    mesh.border(grid, m.divider_w, theme.frame);
}

/// The ruler of a roll in hertz, in the gutter the keys take otherwise: a tick
/// at each round frequency and its label beside it.
pub fn draw_hz_ruler(d: &mut Draw, gutter: Rect, axis: Pitches) {
    let ticks = hz_ticks(axis, gutter.h, d.parts().1);
    let (mesh, m, theme) = d.parts();
    if gutter.w <= 0.0 || gutter.h <= 0.0 {
        return;
    }
    mesh.rect(gutter, theme.lane_alt);
    let text_h = font::height(m.micro_scale);
    for (pitch, label) in ticks {
        let y = axis.y(pitch, gutter);
        if y < gutter.y || y > gutter.y + gutter.h {
            continue;
        }
        let len = if label.is_some() { 6.0 } else { 3.0 };
        mesh.rect(
            Rect::new(gutter.x + gutter.w - len, y, len, m.divider_w),
            theme.ruler_text,
        );
        if let Some(label) = label {
            let top = (y - text_h * 0.5).clamp(gutter.y, gutter.y + gutter.h - text_h);
            font::text(
                mesh,
                &label,
                gutter.x + 2.0,
                top,
                m.micro_scale,
                theme.ruler_text,
            );
        }
    }
    mesh.border(gutter, m.divider_w, theme.frame);
}

/// Draw the OSC markers: a flag at each marker's time, with its label.
pub fn draw_osc_markers(d: &mut Draw, lane: Rect, nav: &View, offset: f64, marks: &[OscMarker]) {
    let (mesh, m, theme) = d.parts();
    if lane.w <= 0.0 || lane.h <= 0.0 {
        return;
    }
    mesh.rect(lane, theme.osc_markers);
    let (x_lo, x_hi) = (lane.x, lane.x + lane.w);
    for mark in marks {
        let x = to_x(offset + mark.time, nav, lane) as f32;
        if x < x_lo || x > x_hi {
            continue;
        }
        mesh.rect(Rect::new(x, lane.y, 2.0, lane.h), theme.flag);
        mesh.disc(x, lane.y + 3.0, 3.0, theme.flag);
        if let Some(t) = &mark.label {
            font::text(
                mesh,
                t,
                x + 4.0,
                lane.y + 1.0,
                m.micro_scale,
                theme.label_dim,
            );
        }
    }
    mesh.border(lane, m.divider_w, theme.frame);
}

// --- Hit-testing ----------------------------------------------------------

/// A note hit: its index in the note list and which part.
#[derive(Clone, Copy, Debug)]
pub struct NoteHit {
    pub index: usize,
    pub part: Part,
}

/// **Where note `n` is drawn**: its box as `draw_notes` paints it, clamped to
/// what of it the grid shows -- `None` when none of it does. What a note's own
/// contents (its expression) are drawn inside, and pressed through.
pub fn note_rect(grid: Rect, nav: &View, offset: f64, n: &Note, axis: Pitches) -> Option<Rect> {
    if !axis.visible(n.pitch) {
        return None;
    }
    let (x_lo, x_hi) = (grid.x, grid.x + grid.w);
    let x0 = (to_x(offset + n.start, nav, grid) as f32).clamp(x_lo, x_hi);
    let x1 = (to_x(offset + n.start + n.dur.max(0.0), nav, grid) as f32).clamp(x_lo, x_hi);
    if x1 <= x0 {
        return None;
    }
    let (y, h) = visible_band(
        axis.y(axis.anchor(n.pitch), grid),
        axis.note_height(grid),
        grid,
    )?;
    Some(Rect::new(x0, y, x1 - x0, h))
}

/// The note under `(x, y)` in the grid, if any -- the last drawn (topmost) match
/// wins. Returns the edge part when the cursor is within `EDGE_PX` of a wide
/// enough note's start/end, else the body.
#[allow(clippy::too_many_arguments)] // one time-and-pitch mapping, all scalars
pub fn note_hit(
    grid: Rect,
    nav: &View,
    offset: f64,
    notes: &[Note],
    axis: Pitches,
    x: f32,
    y: f32,
) -> Option<NoteHit> {
    let h = axis.note_height(grid);
    let mut found: Option<NoteHit> = None;
    for (i, n) in notes.iter().enumerate() {
        if !axis.visible(n.pitch) {
            continue; // scrolled out of the pitch window: not drawn, not grabbable
        }
        let nx0 = to_x(offset + n.start, nav, grid) as f32;
        let nx1 = to_x(offset + n.start + n.dur.max(0.0), nav, grid) as f32;
        // The band actually on screen, exactly as drawn: a half-cut note is
        // grabbed by the half you can see.
        let Some((ny, nh)) = visible_band(axis.y(axis.anchor(n.pitch), grid), h, grid) else {
            continue;
        };
        if x >= nx0 && x <= nx1 && y >= ny && y <= ny + nh {
            let part = boxes::part_at(nx0, nx1, x);
            found = Some(NoteHit { index: i, part });
        }
    }
    found
}

/// The timeline (region-relative) sample position a grid x pixel maps back to --
/// the inverse of [`to_x`], for placing a dragged/added note.
pub fn time_at(grid: Rect, nav: &View, offset: f64, x: f32) -> f64 {
    let s = nav.start + nav.len * ((x - grid.x) as f64 / grid.w.max(1.0) as f64);
    s - offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

    fn grid() -> Rect {
        Rect::new(50.0, 10.0, 400.0, 240.0)
    }

    fn nav() -> View {
        View {
            start: 0.0,
            len: 1000.0,
        }
    }

    #[test]
    fn regions_reserve_only_enabled_strips() {
        let r = Rect::new(0.0, 0.0, 500.0, 400.0);
        let full = regions(r, true, true, KEYBOARD_W, &Metrics::default());
        assert_eq!(full.keyboard.w, KEYBOARD_W);
        assert!(full.ruler.h > 0.0 && full.osc.h == OSC_H);
        // The grid takes what the strips leave.
        assert!((full.grid.h - (400.0 - full.ruler.h - OSC_H)).abs() < 1e-3);
        let bare = regions(r, false, false, KEYBOARD_W, &Metrics::default());
        assert_eq!(bare.ruler.h, 0.0);
        assert_eq!(bare.osc.h, 0.0);
        assert!((bare.grid.h - 400.0).abs() < 1e-3);
    }

    #[test]
    fn pitch_maps_round_trip() {
        let g = grid();
        // High pitch is at the top (small y).
        assert!(pitch_to_y(96.0, 24.0, 96.0, g) < pitch_to_y(24.0, 24.0, 96.0, g));
        let p = y_to_pitch(pitch_to_y(60.0, 24.0, 96.0, g), 24.0, 96.0, g);
        assert!((p - 60.0).abs() < 0.5, "got {p}");
    }

    /// A note outside the visible pitch window is *gone*, not flattened against
    /// the edge -- where a zoomed-in roll would stack every note above it into
    /// one bar and let a press grab any of them at a pitch none of them has.
    #[test]
    fn a_note_outside_the_pitch_window_is_neither_drawn_nor_grabbable() {
        let g = grid();
        let nv = nav();
        let notes = vec![Note::new(100.0, 400.0, 84.0)];
        let x = (to_x(100.0, &nv, g) + to_x(500.0, &nv, g)) as f32 * 0.5;
        // Inside a window holding it: the row is hit at its own y.
        let yc = pitch_to_y(84.0, 72.0, 96.0, g);
        assert!(note_hit(g, &nv, 0.0, &notes, Pitches::rows(72.0, 96.0), x, yc).is_some());
        // Zoomed onto 48..72, the note is a whole octave above the top row.
        assert!(!pitch_visible(84.0, 48.0, 72.0));
        for y in [g.y, g.y + 1.0, g.y + g.h * 0.5, g.y + g.h - 1.0] {
            assert!(
                note_hit(g, &nv, 0.0, &notes, Pitches::rows(48.0, 72.0), x, y).is_none(),
                "grabbed at y {y}"
            );
        }
        // A row is in view while any of it is: half a row past each end.
        assert!(pitch_visible(72.0, 48.0, 72.0) && pitch_visible(48.0, 48.0, 72.0));
        assert!(pitch_visible(72.4, 48.0, 72.0), "half out is still half in");
        assert!(!pitch_visible(73.0, 48.0, 72.0), "a whole row past the end");
        // The chrome iterates exactly the rows that show, boundary included.
        let rows: Vec<i32> = rows_in_view(48.2, 52.7).collect();
        assert_eq!(rows, vec![48, 49, 50, 51, 52, 53]);
    }

    /// A row on its way out of the window is **cut** by the grid's edge. The
    /// alternative -- sliding the whole bar back inside, which is what clamping
    /// its top did -- draws the note on a row that is not its own, and the note
    /// stops moving while the axis under it keeps going.
    #[test]
    fn a_row_leaving_the_view_is_cut_rather_than_pushed_back_in() {
        let g = grid(); // y = 0, h = 300
        // A bar of 40 px centred 10 px above the top edge: 10 px of it show,
        // at the very top, and it never starts below `grid.y`.
        let (y, h) = visible_band(g.y - 10.0, 40.0, g).unwrap();
        assert_eq!((y, h), (g.y, 10.0));
        // The same on the way out of the bottom.
        let (y, h) = visible_band(g.y + g.h + 10.0, 40.0, g).unwrap();
        assert_eq!((y, h), (g.y + g.h - 10.0, 10.0));
        // Fully out: nothing to draw, on either side.
        assert!(visible_band(g.y - 30.0, 40.0, g).is_none());
        assert!(visible_band(g.y + g.h + 30.0, 40.0, g).is_none());
        // Fully in: untouched.
        assert_eq!(visible_band(g.y + 100.0, 40.0, g), Some((g.y + 80.0, 40.0)));
    }

    #[test]
    fn hit_finds_edges_before_body() {
        let g = grid();
        let nv = nav();
        let notes = vec![Note::new(100.0, 400.0, 60.0)];
        // x range of the note: 100..500 samples over 1000 across width 400 ->
        // pixels 50 + [40, 200] = [90, 250].
        let x0 = to_x(100.0, &nv, g) as f32;
        let x1 = to_x(500.0, &nv, g) as f32;
        let yc = pitch_to_y(60.0, 24.0, 96.0, g);
        // Near the start edge.
        let h = note_hit(g, &nv, 0.0, &notes, Pitches::rows(24.0, 96.0), x0 + 1.0, yc).unwrap();
        assert_eq!(h.part, Part::Start);
        // Near the end edge.
        let h = note_hit(g, &nv, 0.0, &notes, Pitches::rows(24.0, 96.0), x1 - 1.0, yc).unwrap();
        assert_eq!(h.part, Part::End);
        // In the middle -> body.
        let h = note_hit(
            g,
            &nv,
            0.0,
            &notes,
            Pitches::rows(24.0, 96.0),
            (x0 + x1) * 0.5,
            yc,
        )
        .unwrap();
        assert_eq!(h.part, Part::Body);
        // Off the note -> miss.
        assert!(
            note_hit(
                g,
                &nv,
                0.0,
                &notes,
                Pitches::rows(24.0, 96.0),
                x0 - 20.0,
                yc
            )
            .is_none()
        );
    }

    #[test]
    fn pitch_labels_draw_only_when_the_rows_can_be_read() {
        // One octave over 240px: ~20px rows -- the C label fits.
        let mut mesh = Mesh::new();
        draw_pitch_labels(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            grid(),
            55.0,
            67.0,
        );
        assert!(mesh.vertex_count() > 0, "a readable C row gets its name");
        // Eight octaves over the same height: sub-3px rows -- nothing draws.
        let mut mesh = Mesh::new();
        draw_pitch_labels(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            grid(),
            12.0,
            108.0,
        );
        assert_eq!(mesh.vertex_count(), 0);
    }

    /// **A line reaches its grid's edges, and so do the keys**: the pitches a
    /// y maps to run edge to edge -- the whole window on a line, half a row
    /// past each end on the keys -- and a fractional window of rows is reached
    /// all the way down, where counting whole bands stopped short of it.
    #[test]
    fn both_axes_reach_the_grids_edges() {
        let g = grid();
        let line = Pitches::line(60.0, 62.5, 8.0);
        assert!((line.pitch(g.y, g) - 62.5).abs() < 1e-4);
        assert!((line.pitch(g.y + g.h, g) - 60.0).abs() < 1e-4);
        assert!((line.y(61.25, g) - (g.y + g.h * 0.5)).abs() < 1e-3);
        let rows = Pitches::rows(50.8, 76.2);
        assert!(
            (rows.pitch(g.y + g.h, g) - 50.3).abs() < 1e-3,
            "the last fraction"
        );
        assert!((rows.pitch(g.y, g) - 76.7).abs() < 1e-3);
    }

    /// **On the keys a note sits on its nearest key's row**, and what it is off
    /// that key is a line inside the box; on a line it is centred on itself.
    #[test]
    fn a_note_sits_on_its_nearest_key_or_on_its_own_pitch() {
        let rows = Pitches::rows(48.0, 72.0);
        assert_eq!(rows.anchor(60.37), 60.0);
        assert_eq!(rows.anchor(60.6), 61.0);
        let line = Pitches::line(48.0, 72.0, 8.0);
        assert_eq!(line.anchor(60.37), 60.37);
        let g = grid();
        let nv = View::full(1000);
        let notes = vec![Note::new(0.0, 500.0, 60.37)];
        let x = to_x(100.0, &nv, g) as f32;
        let on_row = rows.y(60.0, g);
        assert!(note_hit(g, &nv, 0.0, &notes, rows, x, on_row).is_some());
    }

    /// **A key a compass cannot hold is marked at its edge**, and one inside it
    /// is not.
    #[test]
    fn a_key_past_the_compass_is_marked_at_its_edge() {
        let nv = View::full(1000);
        let draw = |pitch: f32| {
            let mut mesh = Mesh::new();
            draw_out_of_range(
                &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
                grid(),
                &nv,
                0.0,
                &[Note::new(0.0, 500.0, pitch)],
                Pitches::rows(100.0, 127.0),
                (0.0, 127.0),
            );
            mesh.vertex_count()
        };
        assert!(draw(131.0) > 0, "above 127");
        assert_eq!(draw(120.0), 0, "inside the compass");
    }
}
