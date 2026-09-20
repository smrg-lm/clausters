//! The piano-roll **drawing**: a note grid, a piano keyboard gutter, a
//! velocity lane and an OSC lane, all pure over a [`Draw`] (the
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
//! musical knowledge -- the MIDI-note ↔ name/black-key spelling drawn on the
//! keyboard and the pitch ruler -- lives in `clausters_core::scale`.

use clausters_core::scale;

use crate::host::bands::Bands;
use crate::host::font;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::structures::boxes::{self, Part};
use crate::host::structures::notes::{Note, OscMark};
use crate::viewport::View;

// --- Layout ---------------------------------------------------------------

/// The keyboard gutter a roll asks for, device pixels -- its *own* structural
/// geometry. What it actually gets is its navigation group's shared indent
/// (`crate::host::timeline::group_indent`), which is this when the roll is alone on
/// its axis and wider when it shares one with a lane.
pub const KEYBOARD_W: f32 = 44.0;
/// The velocity lane height, device pixels.
pub const VELOCITY_H: f32 = 52.0;
/// The OSC lane height, device pixels.
pub const OSC_H: f32 = 16.0;
/// The smallest note bar height (a note never collapses below this even when a
/// semitone row is sub-pixel).
const NOTE_MIN_H: f32 = 2.0;

/// The regions of a `pianoroll` widget rect: the keyboard gutter (left), the
/// note grid, and the optional OSC / velocity / time-ruler strips stacked at the
/// bottom. The renderer and the hit-test both call this, so a note occupies the
/// same pixels either way.
#[derive(Clone, Copy, Debug)]
pub struct Regions {
    pub keyboard: Rect,
    pub grid: Rect,
    pub osc: Rect,
    pub velocity: Rect,
    pub ruler: Rect,
}

/// Split a widget rect into its piano-roll regions. `osc`/`velocity` reserve
/// their strips only when on; `ruler_on` reserves the bottom time strip.
/// `indent` is the group's shared gutter -- the keyboard fills it, so the grid
/// starts where every other member of the axis starts its body.
pub fn regions(
    rect: Rect,
    ruler_on: bool,
    osc_on: bool,
    vel_on: bool,
    indent: f32,
    m: &Metrics,
) -> Regions {
    let kw = indent.min(rect.w);
    let rh = if ruler_on { m.ruler_h.min(rect.h) } else { 0.0 };
    let vh = if vel_on { VELOCITY_H } else { 0.0 };
    let oh = if osc_on { OSC_H } else { 0.0 };
    // Reserve from the bottom up: ruler, velocity, osc, then the grid.
    let inner_h = (rect.h - rh).max(0.0);
    let body_x = rect.x + kw;
    let body_w = (rect.w - kw).max(0.0);
    let grid_h = (inner_h - vh - oh).max(0.0);
    let grid = Rect::new(body_x, rect.y, body_w, grid_h);
    let osc = Rect::new(body_x, rect.y + grid_h, body_w, oh);
    let velocity = Rect::new(body_x, rect.y + grid_h + oh, body_w, vh);
    let ruler = Rect::new(body_x, rect.y + inner_h, body_w, rh);
    let keyboard = Rect::new(rect.x, rect.y, kw, grid_h);
    Regions {
        keyboard,
        grid,
        osc,
        velocity,
        ruler,
    }
}

// --- Mapping --------------------------------------------------------------

/// The x pixel a timeline sample position falls on, through the shared `nav`
/// window and the grid `body`.
fn to_x(s: f64, nav: &View, body: Rect) -> f64 {
    body.x as f64 + (s - nav.start) / nav.len.max(1.0) * body.w as f64
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
    let rows = (hi - lo + 1.0).max(1.0);
    grid.h / rows
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
    p > lo - 1.0 && p < hi + 1.0
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
    grid.y + (hi + 0.5 - pitch) * row_height(lo, hi, grid)
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
pub fn y_to_pitch(y: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    hi + 0.5 - bands(lo, hi, grid).index_of(y - grid.y)
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
/// multi-note selection; the clip body passes none).
#[allow(clippy::too_many_arguments)] // one time-and-pitch mapping, all scalars
pub fn draw_notes(
    d: &mut Draw,
    field: Rect,
    grid: Rect,
    nav: &View,
    offset: f64,
    notes: &[Note],
    lo: f32,
    hi: f32,
    color_velocity: bool,
    selected: &[usize],
) {
    let (mesh, m, theme) = d.parts();
    if grid.w <= 0.0 || grid.h <= 0.0 {
        return;
    }
    let rh = row_height(lo, hi, grid);
    // The floor wins over the ceiling, which is what the trailing `max`
    // always said: a note never collapses below `NOTE_MIN_H`, and a grid
    // shorter than one bar cuts it (`visible_band`) rather than shrinking it.
    // Written as a `clamp` this inverted its own range on such a grid and
    // panicked -- reachable by dragging a window's corner in.
    let h = rh.min(grid.h).max(NOTE_MIN_H);
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
        if nx1 <= nx0 || !pitch_visible(n.pitch, lo, hi) {
            continue;
        }
        // The bar is **cut** by the grid's edge, never pushed inside it: a
        // note on its way out of the pitch window has to leave, and one shoved
        // back in would sit on a row that is not its own.
        let Some((y, h)) = visible_band(row_center(n.pitch, lo, hi, grid), h, grid) else {
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

/// Draw the velocity lane: one bar per note at the note's start, its height the
/// velocity fraction. Shares the grid's time axis so a bar sits under its note.
pub fn draw_velocity_lane(d: &mut Draw, lane: Rect, nav: &View, offset: f64, notes: &[Note]) {
    let (mesh, m, theme) = d.parts();
    if lane.w <= 0.0 || lane.h <= 0.0 {
        return;
    }
    mesh.rect(lane, theme.lane_alt);
    let (x_lo, x_hi) = (lane.x, lane.x + lane.w);
    for n in notes {
        let x = to_x(offset + n.start, nav, lane) as f32;
        if x < x_lo || x > x_hi {
            continue;
        }
        let frac = (n.velocity as f32 / 127.0).clamp(0.0, 1.0);
        let bh = lane.h * frac;
        mesh.rect(Rect::new(x, lane.y + lane.h - bh, 2.0, bh), theme.velocity);
    }
    mesh.border(lane, m.divider_w, theme.frame);
}

/// Draw the OSC lane: a flag at each marker's time, with its label.
pub fn draw_osc_lane(d: &mut Draw, lane: Rect, nav: &View, offset: f64, marks: &[OscMark]) {
    let (mesh, m, theme) = d.parts();
    if lane.w <= 0.0 || lane.h <= 0.0 {
        return;
    }
    mesh.rect(lane, theme.osc_lane);
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

/// The note under `(x, y)` in the grid, if any -- the last drawn (topmost) match
/// wins. Returns the edge part when the cursor is within `EDGE_PX` of a wide
/// enough note's start/end, else the body.
#[allow(clippy::too_many_arguments)] // one time-and-pitch mapping, all scalars
pub fn note_hit(
    grid: Rect,
    nav: &View,
    offset: f64,
    notes: &[Note],
    lo: f32,
    hi: f32,
    x: f32,
    y: f32,
) -> Option<NoteHit> {
    let rh = row_height(lo, hi, grid);
    // The floor wins over the ceiling, which is what the trailing `max`
    // always said: a note never collapses below `NOTE_MIN_H`, and a grid
    // shorter than one bar cuts it (`visible_band`) rather than shrinking it.
    // Written as a `clamp` this inverted its own range on such a grid and
    // panicked -- reachable by dragging a window's corner in.
    let h = rh.min(grid.h).max(NOTE_MIN_H);
    let mut found: Option<NoteHit> = None;
    for (i, n) in notes.iter().enumerate() {
        if !pitch_visible(n.pitch, lo, hi) {
            continue; // scrolled out of the pitch window: not drawn, not grabbable
        }
        let nx0 = to_x(offset + n.start, nav, grid) as f32;
        let nx1 = to_x(offset + n.start + n.dur.max(0.0), nav, grid) as f32;
        // The band actually on screen, exactly as drawn: a half-cut note is
        // grabbed by the half you can see.
        let Some((ny, nh)) = visible_band(row_center(n.pitch, lo, hi, grid), h, grid) else {
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

/// The 0..127 velocity a cursor height maps to within the velocity lane
/// (lane bottom = 0, lane top = 127; clamped) -- the inverse of the lane's bar
/// drawing, shared by the single-bar and block velocity drags.
pub fn velocity_at(lane: Rect, y: f64) -> i32 {
    let frac = ((lane.y + lane.h - y as f32) / lane.h.max(1.0)).clamp(0.0, 1.0);
    (frac * 127.0).round() as i32
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
    fn velocity_at_maps_lane_height_to_0_127() {
        let lane = Rect::new(0.0, 100.0, 400.0, 60.0);
        assert_eq!(velocity_at(lane, 160.0), 0); // lane bottom
        assert_eq!(velocity_at(lane, 100.0), 127); // lane top
        assert_eq!(velocity_at(lane, 130.0), 64); // midway, rounded
        assert_eq!(velocity_at(lane, 500.0), 0); // below: clamped
        assert_eq!(velocity_at(lane, 0.0), 127); // above: clamped
    }

    #[test]
    fn regions_reserve_only_enabled_strips() {
        let r = Rect::new(0.0, 0.0, 500.0, 400.0);
        let full = regions(r, true, true, true, KEYBOARD_W, &Metrics::default());
        assert_eq!(full.keyboard.w, KEYBOARD_W);
        assert!(full.ruler.h > 0.0 && full.osc.h == OSC_H && full.velocity.h == VELOCITY_H);
        // The grid takes what the strips leave.
        assert!((full.grid.h - (400.0 - full.ruler.h - OSC_H - VELOCITY_H)).abs() < 1e-3);
        let bare = regions(r, false, false, false, KEYBOARD_W, &Metrics::default());
        assert_eq!(bare.ruler.h, 0.0);
        assert_eq!(bare.osc.h, 0.0);
        assert_eq!(bare.velocity.h, 0.0);
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
        assert!(note_hit(g, &nv, 0.0, &notes, 72.0, 96.0, x, yc).is_some());
        // Zoomed onto 48..72, the note is a whole octave above the top row.
        assert!(!pitch_visible(84.0, 48.0, 72.0));
        for y in [g.y, g.y + 1.0, g.y + g.h * 0.5, g.y + g.h - 1.0] {
            assert!(
                note_hit(g, &nv, 0.0, &notes, 48.0, 72.0, x, y).is_none(),
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
        // x range of the note: 100..500 samples over 1000 across width 400 →
        // pixels 50 + [40, 200] = [90, 250].
        let x0 = to_x(100.0, &nv, g) as f32;
        let x1 = to_x(500.0, &nv, g) as f32;
        let yc = pitch_to_y(60.0, 24.0, 96.0, g);
        // Near the start edge.
        let h = note_hit(g, &nv, 0.0, &notes, 24.0, 96.0, x0 + 1.0, yc).unwrap();
        assert_eq!(h.part, Part::Start);
        // Near the end edge.
        let h = note_hit(g, &nv, 0.0, &notes, 24.0, 96.0, x1 - 1.0, yc).unwrap();
        assert_eq!(h.part, Part::End);
        // In the middle → body.
        let h = note_hit(g, &nv, 0.0, &notes, 24.0, 96.0, (x0 + x1) * 0.5, yc).unwrap();
        assert_eq!(h.part, Part::Body);
        // Off the note → miss.
        assert!(note_hit(g, &nv, 0.0, &notes, 24.0, 96.0, x0 - 20.0, yc).is_none());
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
}
