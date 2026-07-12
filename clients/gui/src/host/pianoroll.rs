//! The piano-roll graphic primitives: a note grid, a piano keyboard gutter, a
//! velocity lane and an OSC-event lane, all pure over a [`Mesh`] (the
//! flat-geometry [`super::paint`] painter) so they are unit-testable without a
//! window — the static-view posture of `track`/`bpf`.
//!
//! This module is **shared by two consumers**, the same discipline G22h took
//! when it extracted `bpf::place_point`/`insert_point` for both the `bpf`
//! widget and the automation clip:
//!
//! - the dedicated **`pianoroll` widget** (`WidgetKind::PianoRoll`) — an
//!   editor-grade view with a keyboard, rulers, group navigation, selection and
//!   a playhead, drawing MIDI notes in the grid and OSC events in their lane;
//! - the multitrack **`clip` body** — a clip with `notes` draws its compact
//!   piano-roll by calling [`draw_notes`] on the clip's rect, so a note lines up
//!   on the shared time axis and the two never disagree on geometry.
//!
//! Everything here is **display logic** (pixel mapping, hit-testing, drag
//! clamps): it stays gui-side per the placement rule. The one piece of general
//! musical knowledge — the MIDI-note ↔ name/black-key spelling drawn on the
//! keyboard and the pitch ruler — lives in `clausters_core::scale`.

use clausters_core::scale;

use super::font;
use super::layout::Rect;
use super::paint::{Color, Mesh};
use super::track::RULER_H;
use crate::viewport::View;

/// One note: its `start`/`dur` in timeline sample units (relative to the owning
/// region's offset), `pitch` as a MIDI note number (kept `f32` so a clip can map
/// it over an arbitrary `[min, max]` range), and the MIDI `velocity` (`0..127`)
/// and `channel` (`0..15`) that make it a real MIDI note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Note {
    pub start: f64,
    pub dur: f64,
    pub pitch: f32,
    pub velocity: i32,
    pub channel: i32,
}

impl Note {
    /// A note with the default velocity (100) on channel 0 — the plain
    /// `(start, dur, pitch)` triple's reading.
    pub fn new(start: f64, dur: f64, pitch: f32) -> Self {
        Note {
            start,
            dur,
            pitch,
            velocity: 100,
            channel: 0,
        }
    }
}

/// One OSC event marker on the event lane: its `time` (timeline samples,
/// relative to the region offset) and an optional short `label` (an address or
/// tag) drawn beside the flag.
#[derive(Clone, Debug, PartialEq)]
pub struct OscMark {
    pub time: f64,
    pub label: Option<String>,
}

// --- Layout ---------------------------------------------------------------

/// The keyboard gutter width, device pixels.
pub const KEYBOARD_W: f32 = 44.0;
/// The velocity lane height, device pixels.
pub const VELOCITY_H: f32 = 52.0;
/// The OSC event lane height, device pixels.
pub const OSC_H: f32 = 16.0;
/// The smallest note bar height (a note never collapses below this even when a
/// semitone row is sub-pixel).
const NOTE_MIN_H: f32 = 2.0;

const GRID_BG: Color = [0.09, 0.10, 0.13, 1.0];
const ROW_BLACK: Color = [0.07, 0.08, 0.10, 1.0];
const ROW_LINE: Color = [0.16, 0.18, 0.22, 1.0];
const OCTAVE_LINE: Color = [0.30, 0.34, 0.42, 1.0];
const KEY_WHITE: Color = [0.82, 0.84, 0.88, 1.0];
const KEY_BLACK: Color = [0.10, 0.11, 0.14, 1.0];
const KEY_LABEL: Color = [0.30, 0.32, 0.38, 1.0];
const FRAME: Color = [0.30, 0.34, 0.42, 1.0];
const NOTE_FILL: Color = [0.55, 0.80, 0.62, 1.0];
const NOTE_EDGE: Color = [0.78, 0.95, 0.82, 1.0];
const SELECTED_FILL: Color = [0.80, 0.90, 0.98, 1.0];
const SELECTED_EDGE: Color = [1.00, 1.00, 1.00, 1.0];
const VEL_BAR: Color = [0.70, 0.55, 0.90, 1.0];
const VEL_BG: Color = [0.07, 0.08, 0.10, 1.0];
const OSC_BG: Color = [0.10, 0.09, 0.13, 1.0];
const OSC_FLAG: Color = [0.95, 0.75, 0.45, 1.0];
const LABEL: Color = [0.60, 0.63, 0.70, 1.0];

const KEY_SCALE: f32 = 1.0;

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
pub fn regions(rect: Rect, ruler_on: bool, osc_on: bool, vel_on: bool) -> Regions {
    let kw = KEYBOARD_W.min(rect.w);
    let rh = if ruler_on { RULER_H.min(rect.h) } else { 0.0 };
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

/// The height in pixels of one semitone row over the pitch window `[lo, hi]`.
pub fn row_height(lo: f32, hi: f32, grid: Rect) -> f32 {
    let rows = (hi - lo).max(1.0);
    grid.h / rows
}

/// A pitch's y pixel (its row center): high pitch at the top.
pub fn pitch_to_y(pitch: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    let frac = ((pitch - lo) / (hi - lo).max(1.0)).clamp(0.0, 1.0);
    grid.y + grid.h * (1.0 - frac)
}

/// The (fractional) pitch a y pixel maps to over `[lo, hi]`.
pub fn y_to_pitch(y: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    let frac = ((y - grid.y) / grid.h.max(1.0)).clamp(0.0, 1.0);
    hi - frac * (hi - lo)
}

// --- Drawing --------------------------------------------------------------

/// The grid background: black-key rows shaded, semitone lines, and a brighter
/// line at each octave (every C). `lo`/`hi` are the visible MIDI pitch window.
pub fn draw_grid_background(mesh: &mut Mesh, grid: Rect, lo: f32, hi: f32) {
    if grid.w <= 0.0 || grid.h <= 0.0 {
        return;
    }
    mesh.rect(grid, GRID_BG);
    let rh = row_height(lo, hi, grid);
    // One shaded band per black-key semitone, plus a divider at each row and a
    // brighter one at each octave boundary (C). Iterate integer pitches in view.
    let p0 = lo.floor() as i32;
    let p1 = hi.ceil() as i32;
    for p in p0..=p1 {
        // The row band spans [p-0.5, p+0.5]; its top is the y of p+0.5.
        let top = pitch_to_y(p as f32 + 0.5, lo, hi, grid);
        if scale::is_black_key(p) && rh >= 1.0 {
            mesh.rect(Rect::new(grid.x, top, grid.w, rh), ROW_BLACK);
        }
        // A divider under each row when the rows are tall enough to read, a
        // brighter one at each octave boundary (below C).
        if rh >= 4.0 {
            let line = if scale::pitch_class(p) == 0 {
                OCTAVE_LINE
            } else {
                ROW_LINE
            };
            let ly = pitch_to_y(p as f32 - 0.5, lo, hi, grid);
            mesh.rect(Rect::new(grid.x, ly, grid.w, 1.0), line);
        }
    }
    mesh.border(grid, 1.0, FRAME);
}

/// Draw a set of notes into `grid`, placed on the shared `nav` time axis
/// (offset added, so a clip's roll moves with the clip) and over the pitch
/// window `[lo, hi]`. The one primitive both the widget and the clip body use.
/// When `color_velocity` the note fill brightens with velocity. `selected`
/// indices draw highlighted (the multi-note selection; the clip body passes
/// none).
#[allow(clippy::too_many_arguments)] // one time-and-pitch mapping, all scalars
pub fn draw_notes(
    mesh: &mut Mesh,
    grid: Rect,
    nav: &View,
    offset: f64,
    notes: &[Note],
    lo: f32,
    hi: f32,
    color_velocity: bool,
    selected: &[usize],
) {
    if grid.w <= 0.0 || grid.h <= 0.0 {
        return;
    }
    let rh = row_height(lo, hi, grid);
    let h = rh.clamp(NOTE_MIN_H, grid.h).max(NOTE_MIN_H);
    let (x_lo, x_hi) = (grid.x, grid.x + grid.w);
    for (i, n) in notes.iter().enumerate() {
        let mut nx0 = to_x(offset + n.start, nav, grid) as f32;
        let mut nx1 = to_x(offset + n.start + n.dur.max(0.0), nav, grid) as f32;
        nx0 = nx0.clamp(x_lo, x_hi);
        nx1 = nx1.clamp(x_lo, x_hi);
        if nx1 <= nx0 {
            continue;
        }
        let yc = pitch_to_y(n.pitch, lo, hi, grid);
        let y = (yc - h * 0.5).clamp(grid.y, grid.y + grid.h - h);
        let is_selected = selected.contains(&i);
        let fill = if is_selected {
            SELECTED_FILL
        } else if color_velocity {
            let v = (n.velocity as f32 / 127.0).clamp(0.15, 1.0);
            [NOTE_FILL[0] * v, NOTE_FILL[1] * v, NOTE_FILL[2] * v, 1.0]
        } else {
            NOTE_FILL
        };
        mesh.rect(Rect::new(nx0, y, nx1 - nx0, h), fill);
        if nx1 - nx0 > 3.0 && h > 3.0 {
            let edge = if is_selected {
                SELECTED_EDGE
            } else {
                NOTE_EDGE
            };
            mesh.border(Rect::new(nx0, y, nx1 - nx0, h), 1.0, edge);
        }
    }
}

/// Label each C row at the left edge of a roll body — the compact pitch ruler
/// for a roll drawn **without** a keyboard gutter (the multitrack `clip`'s
/// body; the dedicated widget names its Cs on the keyboard instead). Draws
/// only when a semitone row is tall enough to read a label.
pub fn draw_pitch_labels(mesh: &mut Mesh, grid: Rect, lo: f32, hi: f32) {
    if grid.w <= 0.0 || grid.h <= 0.0 {
        return;
    }
    let rh = row_height(lo, hi, grid);
    if rh < font::height(KEY_SCALE) + 2.0 {
        return;
    }
    let p0 = lo.floor() as i32;
    let p1 = hi.ceil() as i32;
    for p in p0..=p1 {
        if scale::pitch_class(p) == 0 {
            let top = pitch_to_y(p as f32 + 0.5, lo, hi, grid);
            font::text(
                mesh,
                &scale::note_name(p),
                grid.x + 2.0,
                top + 1.0,
                KEY_SCALE,
                KEY_LABEL,
            );
        }
    }
}

/// Draw the keyboard gutter: a white/black key per semitone row, with a note
/// name on each C. `lo`/`hi` are the same pitch window as the grid.
pub fn draw_keyboard(mesh: &mut Mesh, gutter: Rect, lo: f32, hi: f32) {
    if gutter.w <= 0.0 || gutter.h <= 0.0 {
        return;
    }
    let rh = row_height(lo, hi, gutter);
    let p0 = lo.floor() as i32;
    let p1 = hi.ceil() as i32;
    for p in p0..=p1 {
        let top = pitch_to_y(p as f32 + 0.5, lo, hi, gutter);
        let color = if scale::is_black_key(p) {
            KEY_BLACK
        } else {
            KEY_WHITE
        };
        let h = rh.max(1.0).min(gutter.h);
        mesh.rect(Rect::new(gutter.x, top, gutter.w, h), color);
        // Name every C when there is room for the label.
        if scale::pitch_class(p) == 0 && rh >= font::height(KEY_SCALE) + 2.0 {
            font::text(
                mesh,
                &scale::note_name(p),
                gutter.x + 2.0,
                top + 1.0,
                KEY_SCALE,
                KEY_LABEL,
            );
        }
    }
    mesh.border(gutter, 1.0, FRAME);
}

/// Draw the velocity lane: one bar per note at the note's start, its height the
/// velocity fraction. Shares the grid's time axis so a bar sits under its note.
pub fn draw_velocity_lane(mesh: &mut Mesh, lane: Rect, nav: &View, offset: f64, notes: &[Note]) {
    if lane.w <= 0.0 || lane.h <= 0.0 {
        return;
    }
    mesh.rect(lane, VEL_BG);
    let (x_lo, x_hi) = (lane.x, lane.x + lane.w);
    for n in notes {
        let x = to_x(offset + n.start, nav, lane) as f32;
        if x < x_lo || x > x_hi {
            continue;
        }
        let frac = (n.velocity as f32 / 127.0).clamp(0.0, 1.0);
        let bh = lane.h * frac;
        mesh.rect(Rect::new(x, lane.y + lane.h - bh, 2.0, bh), VEL_BAR);
    }
    mesh.border(lane, 1.0, FRAME);
}

/// Draw the OSC event lane: a flag at each marker's time, with its label.
pub fn draw_osc_lane(mesh: &mut Mesh, lane: Rect, nav: &View, offset: f64, marks: &[OscMark]) {
    if lane.w <= 0.0 || lane.h <= 0.0 {
        return;
    }
    mesh.rect(lane, OSC_BG);
    let (x_lo, x_hi) = (lane.x, lane.x + lane.w);
    for m in marks {
        let x = to_x(offset + m.time, nav, lane) as f32;
        if x < x_lo || x > x_hi {
            continue;
        }
        mesh.rect(Rect::new(x, lane.y, 2.0, lane.h), OSC_FLAG);
        mesh.disc(x, lane.y + 3.0, 3.0, OSC_FLAG);
        if let Some(t) = &m.label {
            font::text(mesh, t, x + 4.0, lane.y + 1.0, KEY_SCALE, LABEL);
        }
    }
    mesh.border(lane, 1.0, FRAME);
}

// --- Hit-testing ----------------------------------------------------------

/// Which part of a note the cursor grabbed — the start/end edges resize it, the
/// body moves it, exactly the clip's `Start`/`End`/`Body` split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePart {
    Body,
    Start,
    End,
}

/// A note hit: its index in the note list and which part.
#[derive(Clone, Copy, Debug)]
pub struct NoteHit {
    pub index: usize,
    pub part: NotePart,
}

/// The device-pixel grab margin for a note's start/end edges.
const EDGE_PX: f32 = 4.0;

/// The note under `(x, y)` in the grid, if any — the last drawn (topmost) match
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
    let h = rh.clamp(NOTE_MIN_H, grid.h).max(NOTE_MIN_H);
    let mut found: Option<NoteHit> = None;
    for (i, n) in notes.iter().enumerate() {
        let nx0 = to_x(offset + n.start, nav, grid) as f32;
        let nx1 = to_x(offset + n.start + n.dur.max(0.0), nav, grid) as f32;
        let yc = pitch_to_y(n.pitch, lo, hi, grid);
        let ny = (yc - h * 0.5).clamp(grid.y, grid.y + grid.h - h);
        if x >= nx0 && x <= nx1 && y >= ny && y <= ny + h {
            let part = note_part(nx0, nx1, x);
            found = Some(NoteHit { index: i, part });
        }
    }
    found
}

/// The part of a `[x0, x1]` note bar `x` falls on (edges before the body, and
/// only when the bar is wide enough to grab an edge).
fn note_part(x0: f32, x1: f32, x: f32) -> NotePart {
    if x1 - x0 < 3.0 * EDGE_PX {
        return NotePart::Body;
    }
    if x - x0 <= EDGE_PX {
        NotePart::Start
    } else if x1 - x <= EDGE_PX {
        NotePart::End
    } else {
        NotePart::Body
    }
}

/// The timeline (region-relative) sample position a grid x pixel maps back to —
/// the inverse of [`to_x`], for placing a dragged/added note.
pub fn time_at(grid: Rect, nav: &View, offset: f64, x: f32) -> f64 {
    let s = nav.start + nav.len * ((x - grid.x) as f64 / grid.w.max(1.0) as f64);
    s - offset
}

// --- Editing (pure, mapping-free) -----------------------------------------

/// Move the note at `index` to a new start (clamped `>= 0`) and pitch (clamped
/// into `[lo, hi]`, rounded to the nearest semitone). The duration is kept.
pub fn move_note(notes: &mut [Note], index: usize, start: f64, pitch: f32, lo: f32, hi: f32) {
    if let Some(n) = notes.get_mut(index) {
        n.start = start.max(0.0);
        n.pitch = pitch.round().clamp(lo, hi);
    }
}

/// Resize the note at `index` by dragging one edge to timeline-relative `t`.
/// `Start` moves the onset (keeping the end fixed), `End` moves the end; a note
/// never shrinks below `min_dur`.
pub fn resize_note(notes: &mut [Note], index: usize, part: NotePart, t: f64, min_dur: f64) {
    if let Some(n) = notes.get_mut(index) {
        match part {
            NotePart::End => n.dur = (t - n.start).max(min_dur),
            NotePart::Start => {
                let end = n.start + n.dur;
                let start = t.min(end - min_dur).max(0.0);
                n.start = start;
                n.dur = end - start;
            }
            NotePart::Body => {}
        }
    }
}

/// Set the velocity (clamped `0..127`) of the note at `index`.
pub fn set_velocity(notes: &mut [Note], index: usize, velocity: i32) {
    if let Some(n) = notes.get_mut(index) {
        n.velocity = velocity.clamp(0, 127);
    }
}

/// Insert a note, returning its index (appended; the list is not kept sorted —
/// draw order is insertion order, matching the clip's).
pub fn insert_note(notes: &mut Vec<Note>, note: Note) -> usize {
    notes.push(note);
    notes.len() - 1
}

/// Remove the note at `index` (a no-op out of range).
pub fn remove_note(notes: &mut Vec<Note>, index: usize) {
    if index < notes.len() {
        notes.remove(index);
    }
}

// --- Multi-note selection and block edits (pure, mapping-free) -------------
//
// The selection is a set of note indices — view state, native-side. The
// marquee is the shared time selection restricted in pitch: dragging the empty
// grid keeps setting the linked views' time selection, and the notes inside
// the time × pitch rectangle become the selected set.

/// The indices of the notes intersecting the time span `[t0, t1)` whose row
/// touches the pitch band `[p_lo, p_hi]` (a note's row spans half a semitone
/// either side of its pitch). Either range may come reversed (a marquee drags
/// both ways).
pub fn notes_in_rect(notes: &[Note], t0: f64, t1: f64, p_lo: f32, p_hi: f32) -> Vec<usize> {
    let (t0, t1) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
    let (p_lo, p_hi) = if p_lo <= p_hi {
        (p_lo, p_hi)
    } else {
        (p_hi, p_lo)
    };
    notes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.start < t1
                && n.start + n.dur.max(0.0) > t0
                && n.pitch + 0.5 >= p_lo
                && n.pitch - 0.5 <= p_hi
        })
        .map(|(i, _)| i)
        .collect()
}

/// Move a block of notes rigidly from a press-time snapshot: `orig` is
/// `(index, start, pitch)` per selected note, `dt`/`dp` the drag deltas. The
/// deltas are clamped **as one** — no start below zero, no pitch outside
/// `[lo, hi]` — so the block stops at an edge instead of folding against it.
/// Durations are kept.
pub fn move_notes_from(
    notes: &mut [Note],
    orig: &[(usize, f64, f32)],
    dt: f64,
    dp: f32,
    lo: f32,
    hi: f32,
) {
    if orig.is_empty() {
        return;
    }
    let min_start = orig
        .iter()
        .map(|(_, s, _)| *s)
        .fold(f64::INFINITY, f64::min);
    let dt = dt.max(-min_start);
    let (min_p, max_p) = orig
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), (_, _, p)| {
            (a.min(p.round()), b.max(p.round()))
        });
    // A block already wider than the window cannot move rigidly in pitch.
    let dp = if lo - min_p <= hi - max_p {
        dp.round().clamp(lo - min_p, hi - max_p)
    } else {
        0.0
    };
    for (i, s, p) in orig {
        if let Some(n) = notes.get_mut(*i) {
            n.start = s + dt;
            n.pitch = (p.round() + dp).clamp(lo, hi);
        }
    }
}

/// Remove a set of notes by index (any order, duplicates tolerated).
pub fn remove_notes(notes: &mut Vec<Note>, indices: &[usize]) {
    let mut sorted: Vec<usize> = indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    for i in sorted.into_iter().rev() {
        if i < notes.len() {
            notes.remove(i);
        }
    }
}

/// Nudge a block of velocities relatively from a press-time snapshot: `orig`
/// is `(index, velocity)` per selected note, `dv` the common delta — each note
/// clamps to `0..127` on its own (a saturated bar stays put, the rest keep
/// moving, and reversing restores the original spread).
pub fn nudge_velocities_from(notes: &mut [Note], orig: &[(usize, i32)], dv: i32) {
    for (i, v) in orig {
        if let Some(n) = notes.get_mut(*i) {
            n.velocity = (v + dv).clamp(0, 127);
        }
    }
}

/// The selection re-mapped after the note at `removed` left the list: the
/// removed index drops out, higher indices shift down one.
pub fn selection_after_removal(selected: &[usize], removed: usize) -> Vec<usize> {
    selected
        .iter()
        .filter(|&&i| i != removed)
        .map(|&i| if i > removed { i - 1 } else { i })
        .collect()
}

/// Toggle a note in or out of the selection (Alt+click: a non-rectangular
/// selection built one note at a time).
pub fn toggle_selected(selected: &mut Vec<usize>, index: usize) {
    match selected.iter().position(|&i| i == index) {
        Some(p) => {
            selected.remove(p);
        }
        None => selected.push(index),
    }
}

/// Quantize note onsets to the `grid` (timeline samples): each start snaps to
/// the nearest grid line, durations untouched. `indices` picks the notes (the
/// selection); empty quantizes them all. A zero/negative grid is a no-op.
/// Returns whether anything moved.
pub fn quantize_notes(notes: &mut [Note], indices: &[usize], grid: f64) -> bool {
    if grid <= 0.0 {
        return false;
    }
    let snap = |s: f64| (s / grid).round() * grid;
    let mut moved = false;
    let mut apply = |n: &mut Note| {
        let s = snap(n.start).max(0.0);
        moved |= s != n.start;
        n.start = s;
    };
    if indices.is_empty() {
        notes.iter_mut().for_each(&mut apply);
    } else {
        for &i in indices {
            if let Some(n) = notes.get_mut(i) {
                apply(n);
            }
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let full = regions(r, true, true, true);
        assert_eq!(full.keyboard.w, KEYBOARD_W);
        assert!(full.ruler.h > 0.0 && full.osc.h == OSC_H && full.velocity.h == VELOCITY_H);
        // The grid takes what the strips leave.
        assert!((full.grid.h - (400.0 - full.ruler.h - OSC_H - VELOCITY_H)).abs() < 1e-3);
        let bare = regions(r, false, false, false);
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
        assert_eq!(h.part, NotePart::Start);
        // Near the end edge.
        let h = note_hit(g, &nv, 0.0, &notes, 24.0, 96.0, x1 - 1.0, yc).unwrap();
        assert_eq!(h.part, NotePart::End);
        // In the middle → body.
        let h = note_hit(g, &nv, 0.0, &notes, 24.0, 96.0, (x0 + x1) * 0.5, yc).unwrap();
        assert_eq!(h.part, NotePart::Body);
        // Off the note → miss.
        assert!(note_hit(g, &nv, 0.0, &notes, 24.0, 96.0, x0 - 20.0, yc).is_none());
    }

    #[test]
    fn move_clamps_pitch_and_start() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, -50.0, 200.7, 24.0, 96.0);
        assert_eq!(notes[0].start, 0.0);
        assert_eq!(notes[0].pitch, 96.0); // clamped to hi, rounded
        assert_eq!(notes[0].dur, 200.0); // duration kept
    }

    #[test]
    fn resize_respects_min_dur_from_either_edge() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        // Drag the end back past the start → clamped to min_dur.
        resize_note(&mut notes, 0, NotePart::End, 50.0, 10.0);
        assert_eq!(notes[0].dur, 10.0);
        // Drag the start forward past the end → clamped.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)]; // end = 300
        resize_note(&mut notes, 0, NotePart::Start, 400.0, 10.0);
        assert_eq!(notes[0].start, 290.0);
        assert_eq!(notes[0].dur, 10.0);
    }

    #[test]
    fn insert_and_remove() {
        let mut notes = vec![Note::new(0.0, 100.0, 60.0)];
        let i = insert_note(&mut notes, Note::new(200.0, 50.0, 64.0));
        assert_eq!(i, 1);
        assert_eq!(notes.len(), 2);
        remove_note(&mut notes, 0);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, 64.0);
        remove_note(&mut notes, 9); // out of range, no-op
        assert_eq!(notes.len(), 1);
    }

    // --- selection + block edits ---

    fn three_notes() -> Vec<Note> {
        vec![
            Note::new(0.0, 100.0, 60.0),
            Note::new(200.0, 100.0, 64.0),
            Note::new(400.0, 100.0, 72.0),
        ]
    }

    #[test]
    fn a_marquee_selects_by_time_and_pitch_and_tolerates_reversed_ranges() {
        let notes = three_notes();
        // The middle note only: its time span, its pitch band.
        assert_eq!(notes_in_rect(&notes, 150.0, 350.0, 62.0, 66.0), vec![1]);
        // A reversed drag selects the same.
        assert_eq!(notes_in_rect(&notes, 350.0, 150.0, 66.0, 62.0), vec![1]);
        // The full time span but a pitch band excluding the top note.
        assert_eq!(notes_in_rect(&notes, 0.0, 500.0, 58.0, 65.0), vec![0, 1]);
        // A note intersecting the span's edge is in (its tail crosses t0).
        assert_eq!(notes_in_rect(&notes, 50.0, 60.0, 59.0, 61.0), vec![0]);
        // An empty rect selects nothing.
        assert!(notes_in_rect(&notes, 120.0, 130.0, 60.0, 60.0).is_empty());
    }

    #[test]
    fn a_block_move_is_rigid_and_clamps_as_one() {
        // Free move: both notes shift by the same delta.
        let mut notes = three_notes();
        let orig = vec![(0, 0.0, 60.0f32), (1, 200.0, 64.0f32)];
        move_notes_from(&mut notes, &orig, 50.0, 2.4, 24.0, 96.0);
        assert_eq!((notes[0].start, notes[0].pitch), (50.0, 62.0));
        assert_eq!((notes[1].start, notes[1].pitch), (250.0, 66.0));
        // Clamped at time zero: the whole block stops, keeping the spread.
        let mut notes = three_notes();
        move_notes_from(&mut notes, &orig, -80.0, 0.0, 24.0, 96.0);
        assert_eq!((notes[0].start, notes[1].start), (0.0, 200.0));
        // Clamped at the pitch top: the highest note pins the block.
        let mut notes = three_notes();
        move_notes_from(&mut notes, &orig, 0.0, 40.0, 24.0, 96.0);
        assert_eq!((notes[0].pitch, notes[1].pitch), (92.0, 96.0));
        // Durations are never touched.
        assert_eq!(notes[0].dur, 100.0);
    }

    #[test]
    fn a_block_wider_than_the_pitch_window_does_not_fold() {
        let mut notes = vec![Note::new(0.0, 10.0, 20.0), Note::new(0.0, 10.0, 100.0)];
        let orig = vec![(0, 0.0, 20.0f32), (1, 0.0, 100.0f32)];
        move_notes_from(&mut notes, &orig, 0.0, 5.0, 24.0, 96.0);
        // The rigid pitch move is refused; the pitches only clamp into range.
        assert_eq!((notes[0].pitch, notes[1].pitch), (24.0, 96.0));
    }

    #[test]
    fn a_block_removal_takes_any_order_and_duplicates() {
        let mut notes = three_notes();
        remove_notes(&mut notes, &[2, 0, 0]);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].pitch, 64.0);
    }

    #[test]
    fn a_velocity_nudge_is_relative_and_saturates_per_note() {
        let mut notes = three_notes();
        notes[0].velocity = 120;
        notes[1].velocity = 60;
        let orig = vec![(0, 120), (1, 60)];
        nudge_velocities_from(&mut notes, &orig, 20);
        assert_eq!((notes[0].velocity, notes[1].velocity), (127, 80));
        // Reversing from the same snapshot restores the original spread.
        nudge_velocities_from(&mut notes, &orig, -20);
        assert_eq!((notes[0].velocity, notes[1].velocity), (100, 40));
    }

    #[test]
    fn pitch_labels_draw_only_when_the_rows_can_be_read() {
        // One octave over 240px: ~20px rows — the C label fits.
        let mut mesh = Mesh::new();
        draw_pitch_labels(&mut mesh, grid(), 55.0, 67.0);
        assert!(mesh.vertex_count() > 0, "a readable C row gets its name");
        // Eight octaves over the same height: sub-3px rows — nothing draws.
        let mut mesh = Mesh::new();
        draw_pitch_labels(&mut mesh, grid(), 12.0, 108.0);
        assert_eq!(mesh.vertex_count(), 0);
    }

    #[test]
    fn quantize_snaps_the_selection_or_everything_and_reports_movement() {
        // The selection only: the third note keeps its offbeat start.
        let mut notes = vec![
            Note::new(90.0, 50.0, 60.0),
            Note::new(260.0, 50.0, 64.0),
            Note::new(430.0, 50.0, 67.0),
        ];
        assert!(quantize_notes(&mut notes, &[0, 1], 100.0));
        assert_eq!(
            (notes[0].start, notes[1].start, notes[2].start),
            (100.0, 300.0, 430.0)
        );
        // No selection: everything snaps; durations never move.
        assert!(quantize_notes(&mut notes, &[], 100.0));
        assert_eq!(notes[2].start, 400.0);
        assert_eq!(notes[2].dur, 50.0);
        // Already on the grid (or no grid): nothing to report.
        assert!(!quantize_notes(&mut notes, &[], 100.0));
        assert!(!quantize_notes(&mut notes, &[], 0.0));
    }

    #[test]
    fn the_selection_follows_a_single_removal_and_toggles() {
        assert_eq!(selection_after_removal(&[0, 1, 2], 1), vec![0, 1]);
        assert_eq!(selection_after_removal(&[2], 2), Vec::<usize>::new());
        let mut sel = vec![0];
        toggle_selected(&mut sel, 2);
        assert_eq!(sel, vec![0, 2]);
        toggle_selected(&mut sel, 0);
        assert_eq!(sel, vec![2]);
    }
}
