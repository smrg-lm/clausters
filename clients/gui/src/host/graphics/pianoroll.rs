//! The piano-roll graphic primitives: a note grid, a piano keyboard gutter, a
//! velocity lane and an OSC lane, all pure over a [`Draw`] (the
//! flat-geometry [`crate::host::paint`] painter) so they are unit-testable without a
//! window — the static-view posture of `track`/`bpf`.
//!
//! This module is **shared by two consumers**, on the crate's standing rule
//! that a model and its hit-test primitives are extracted once and reused —
//! the same way `bpf::place_point`/`insert_point` serve both the `bpf` widget
//! and the automation clip:
//!
//! - the dedicated **`pianoroll` widget** (`WidgetKind::PianoRoll`) — an
//!   editor-grade view with a keyboard, rulers, group navigation, selection and
//!   a playhead, drawing MIDI notes in the grid and OSC markers in their lane;
//! - the multitrack **`clip` body** — a clip with `notes` draws its compact
//!   piano-roll by calling [`draw_notes`] on the clip's rect, so a note lines up
//!   on the shared time axis and the two never disagree on geometry.
//!
//! Everything here is **display logic** (pixel mapping, hit-testing, drag
//! clamps): it stays gui-side per the placement rule. The one piece of general
//! musical knowledge — the MIDI-note ↔ name/black-key spelling drawn on the
//! keyboard and the pitch ruler — lives in `clausters_core::scale`.

use clausters_core::scale;
use serde_json::Value;

use crate::host::font;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::placement::{self, Bounds, Contents, Part, Placement, Placements};
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

/// The `notes` wire form of a note list: the flat `start dur pitch velocity
/// channel` quintuple array, as JSON.
///
/// The inverse of the `notes` prop's parse, so what a `/gui_query` reports is
/// what a `/gui_set` would take — which is the whole contract of reporting a
/// non-scalar as its own string carrier.
pub fn notes_json(notes: &[Note]) -> Value {
    let mut out = Vec::with_capacity(notes.len() * 5);
    for n in notes {
        out.push(Value::from(n.start));
        out.push(Value::from(n.dur));
        out.push(Value::from(n.pitch));
        out.push(Value::from(n.velocity));
        out.push(Value::from(n.channel));
    }
    Value::Array(out)
}

/// The `osc` wire form of a marker list: the flat `time label` pair array, as
/// JSON — the inverse of the `osc` prop's parse.
pub fn osc_json(marks: &[OscMark]) -> Value {
    let mut out = Vec::with_capacity(marks.len() * 2);
    for m in marks {
        out.push(Value::from(m.time));
        out.push(Value::from(m.label.clone().unwrap_or_default()));
    }
    Value::Array(out)
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

/// One marker on the OSC lane: its `time` (timeline samples,
/// relative to the region offset) and an optional short `label` (an address or
/// tag) drawn beside the flag.
#[derive(Clone, Debug, PartialEq)]
pub struct OscMark {
    pub time: f64,
    pub label: Option<String>,
}

// --- Layout ---------------------------------------------------------------

/// The keyboard gutter a roll asks for, device pixels — its *own* structural
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
/// `indent` is the group's shared gutter — the keyboard fills it, so the grid
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

/// The height in pixels of one semitone row over the pitch window `[lo, hi]`.
/// The window shows every whole row `lo..=hi` — `hi - lo + 1` of them — so the
/// pixel axis spans `[lo - 0.5, hi + 0.5]` and the extreme rows draw in full
/// instead of being clipped at the grid edges.
pub fn row_height(lo: f32, hi: f32, grid: Rect) -> f32 {
    let rows = (hi - lo + 1.0).max(1.0);
    grid.h / rows
}

/// The integer pitches whose rows show in the window `[lo - 0.5, hi + 0.5]` —
/// what everything drawn *per row* iterates, so the bands, the dividers, the
/// keys and the labels are the same set of rows.
fn rows_in_view(lo: f32, hi: f32) -> std::ops::RangeInclusive<i32> {
    lo.floor() as i32..=hi.ceil() as i32
}

/// The part of a bar of height `h` centred on `yc` that falls inside `grid`,
/// as `(y, height)` — `None` when none of it does. The vertical counterpart of
/// the note's horizontal clamp to the grid bounds.
fn visible_band(yc: f32, h: f32, grid: Rect) -> Option<(f32, f32)> {
    let y = (yc - h * 0.5).max(grid.y);
    let bottom = (yc + h * 0.5).min(grid.y + grid.h);
    (bottom > y).then_some((y, bottom - y))
}

/// Whether any part of pitch `p`'s row shows in the window `[lo - 0.5, hi + 0.5]`.
///
/// The row of `p` spans `[p - 0.5, p + 0.5]`, so it is in view while `p` is
/// within **a whole row** of the window's ends — half of it is enough. Asking
/// for the row's *centre* to be inside instead drops a note the moment it is
/// half cut, which is exactly when it should still be half drawn.
///
/// The horizontal axis says this by construction — a note off the time window
/// clamps to a zero-width span and is skipped — while the vertical one has to
/// be asked.
pub fn pitch_visible(p: f32, lo: f32, hi: f32) -> bool {
    p > lo - 1.0 && p < hi + 1.0
}

/// A pitch's y pixel (its row centre), **unclamped**: a pitch outside the
/// window maps above or below `grid` instead of onto its edge. Whatever is
/// placed *on* a row — a note bar — wants this one and cuts itself against the
/// grid, because a row leaving the view is cut, not slid back in.
pub fn row_center(pitch: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    let rows = (hi - lo + 1.0).max(1.0);
    grid.y + grid.h * (hi + 0.5 - pitch) / rows
}

/// A pitch's y pixel (its row centre) **inside** `grid`: high pitch at the top.
/// The axis spans `[lo - 0.5, hi + 0.5]`, so pitch `hi` centres half a row
/// below the top edge and pitch `lo` half a row above the bottom — every row is
/// fully visible. Clamped to the grid, which is what the chrome painted *per
/// row* wants (the shaded bands, the keyboard keys, the C labels): those are
/// drawn for the rows in view and must not bleed into the strip above or below.
pub fn pitch_to_y(pitch: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    row_center(pitch, lo, hi, grid).clamp(grid.y, grid.y + grid.h)
}

/// The (fractional) pitch a y pixel maps to over the `[lo - 0.5, hi + 0.5]`
/// window — the inverse of [`pitch_to_y`], so a drop lands on the row it is
/// drawn on.
pub fn y_to_pitch(y: f32, lo: f32, hi: f32, grid: Rect) -> f32 {
    let rows = (hi - lo + 1.0).max(1.0);
    let frac = ((y - grid.y) / grid.h.max(1.0)).clamp(0.0, 1.0);
    hi + 0.5 - frac * rows
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
        // ends — never slid inside it, which would stack the rows above the
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
/// clip). `field` is the pixel domain the `nav` window spans horizontally — the
/// lane body for a multitrack clip, the grid itself for the dedicated view — and
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
    // panicked — reachable by dragging a window's corner in.
    let h = rh.min(grid.h).max(NOTE_MIN_H);
    let (x_lo, x_hi) = (grid.x, grid.x + grid.w);
    for (i, n) in notes.iter().enumerate() {
        // x maps through `field` — the pixel domain the shared `nav` spans (the
        // lane body for a clip, the grid itself for the dedicated view) — then
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

/// Label each C row at the left edge of a roll body — the compact pitch ruler
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
    // The floor wins over the ceiling, which is what the trailing `max`
    // always said: a note never collapses below `NOTE_MIN_H`, and a grid
    // shorter than one bar cuts it (`visible_band`) rather than shrinking it.
    // Written as a `clamp` this inverted its own range on such a grid and
    // panicked — reachable by dragging a window's corner in.
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
            let part = placement::part_at(nx0, nx1, x);
            found = Some(NoteHit { index: i, part });
        }
    }
    found
}

/// The timeline (region-relative) sample position a grid x pixel maps back to —
/// the inverse of [`to_x`], for placing a dragged/added note.
pub fn time_at(grid: Rect, nav: &View, offset: f64, x: f32) -> f64 {
    let s = nav.start + nav.len * ((x - grid.x) as f64 / grid.w.max(1.0) as f64);
    s - offset
}

// --- Editing (pure, mapping-free) -----------------------------------------

/// **Indexed access to the note list**, so every block edit a note shares with
/// a clip is written once (`crate::host::placement`) and both call it.
///
/// A note's row is its pitch and its `Placement::start` is always zero: there
/// is no source behind a note to window, so an edge drag's trim has nowhere to
/// travel and the accessor drops it.
impl Placements for [Note] {
    fn len(&self) -> usize {
        <[Note]>::len(self)
    }

    fn placement(&self, i: usize) -> Placement {
        let n = self[i];
        Placement {
            offset: n.start,
            dur: n.dur,
            start: 0.0,
        }
    }

    fn set_placement(&mut self, i: usize, p: Placement) {
        self[i].start = p.offset;
        self[i].dur = p.dur;
    }

    fn row(&self, i: usize) -> f32 {
        self[i].pitch
    }

    fn set_row(&mut self, i: usize, r: f32) {
        self[i].pitch = r;
    }
}

/// Move the note at `index` to a new start (clamped into the bounds' domain,
/// tail included) and pitch (clamped into `[lo, hi]`, rounded to the nearest
/// semitone). The duration is kept.
pub fn move_note(
    notes: &mut [Note],
    index: usize,
    start: f64,
    pitch: f32,
    lo: f32,
    hi: f32,
    bounds: Bounds,
) {
    if index >= notes.len() {
        return;
    }
    let orig = notes.placement(index);
    let placed = placement::drag(Part::Body, start, orig, Contents::default(), bounds);
    notes.set_placement(index, placed);
    notes.set_row(index, pitch.round().clamp(lo, hi));
}

/// Resize the note at `index` by dragging one edge to timeline-relative `t` —
/// the clip's edge drag, over a note.
///
/// `Start` moves the onset (keeping the end fixed), `End` moves the end; a note
/// never shrinks below the bounds' floor, and never grows past their domain.
pub fn resize_note(notes: &mut [Note], index: usize, part: Part, t: f64, bounds: Bounds) {
    if index >= notes.len() || part == Part::Body {
        return;
    }
    let orig = notes.placement(index);
    let placed = placement::drag(part, t, orig, Contents::default(), bounds);
    notes.set_placement(index, placed);
}

/// Set the velocity (clamped `0..127`) of the note at `index`.
pub fn set_velocity(notes: &mut [Note], index: usize, velocity: i32) {
    if let Some(n) = notes.get_mut(index) {
        n.velocity = velocity.clamp(0, 127);
    }
}

/// The 0..127 velocity a cursor height maps to within the velocity lane
/// (lane bottom = 0, lane top = 127; clamped) — the inverse of the lane's bar
/// drawing, shared by the single-bar and block velocity drags.
pub fn velocity_at(lane: Rect, y: f64) -> i32 {
    let frac = ((lane.y + lane.h - y as f32) / lane.h.max(1.0)).clamp(0.0, 1.0);
    (frac * 127.0).round() as i32
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
/// touches the pitch band `[p_lo, p_hi]` — [`placement::in_rect`] over the note
/// list, the same marquee a lane's clips answer.
pub fn notes_in_rect(notes: &[Note], t0: f64, t1: f64, p_lo: f32, p_hi: f32) -> Vec<usize> {
    placement::in_rect(notes, t0, t1, p_lo, p_hi)
}

/// Move a block of notes rigidly from a press-time snapshot — the shared
/// [`placement::move_block`], with the pitch window as the row bounds.
pub fn move_notes_from(
    notes: &mut [Note],
    orig: &[(usize, f64, f32)],
    dt: f64,
    dp: f32,
    lo: f32,
    hi: f32,
    limit: Limit,
) {
    placement::move_block(notes, orig, dt, dp, (lo, hi), limit);
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

pub use crate::host::placement::{Limit, selection_after_removal, toggle_selected};

/// Copy a selection of notes, normalized so the block's earliest onset is 0 —
/// the clipboard form [`paste_notes`] re-places (pitches stay absolute).
pub fn copy_notes(notes: &[Note], indices: &[usize]) -> Vec<Note> {
    let mut out: Vec<Note> = indices
        .iter()
        .filter_map(|&i| notes.get(i).copied())
        .collect();
    let t0 = out.iter().map(|n| n.start).fold(f64::INFINITY, f64::min);
    if t0.is_finite() {
        for n in &mut out {
            n.start -= t0;
        }
    }
    out
}

/// Paste a clipboard block with its first onset at `at`: the notes append
/// (original pitches and spread kept), and the new indices come back — the
/// pasted block becomes the selection, ready to drag into place.
pub fn paste_notes(notes: &mut Vec<Note>, clip: &[Note], at: f64) -> Vec<usize> {
    let at = at.max(0.0);
    clip.iter()
        .map(|n| {
            let mut n = *n;
            n.start += at;
            insert_note(notes, n)
        })
        .collect()
}

/// **Split notes at `at`** (a region-relative time): every named note the time
/// falls strictly inside becomes two, the second carrying the same pitch,
/// velocity and channel. `indices` picks the notes (the selection); empty
/// splits them all. Returns the selection the cut leaves — both halves of every
/// note that was cut — so the block stays in hand.
///
/// The clip's `e` verb, over notes. A clip asks its owner to cut, because the
/// owner holds the element; a roll holds its own notes and cuts them.
pub fn split_notes(notes: &mut Vec<Note>, indices: &[usize], at: f64) -> Vec<usize> {
    let targets: Vec<usize> = if indices.is_empty() {
        (0..notes.len()).collect()
    } else {
        let mut t = indices.to_vec();
        t.sort_unstable();
        t.dedup();
        t
    };
    let mut out = Vec::new();
    for i in targets {
        let Some(n) = notes.get(i).copied() else {
            continue;
        };
        let Some((head, tail)) = placement::split_at(notes.placement(i), at) else {
            continue;
        };
        notes.set_placement(i, head);
        let mut second = n;
        second.start = tail.offset;
        second.dur = tail.dur;
        out.push(i);
        out.push(insert_note(notes, second));
    }
    out
}

/// **Join the named notes**: on each pitch, a run of notes that touch or
/// overlap becomes one, spanning from the first onset to the last end and
/// keeping the first note's velocity and channel. `indices` picks the notes;
/// empty joins over the whole list. Returns the selection that is left.
///
/// The clip's `j` verb, over notes — and the same reading of "juxtaposed": what
/// joins is what touches, so no second selection model is needed to say which
/// two. A pitch is what makes two notes the same voice, which is the roll's
/// answer to the lane a clip's join is confined to.
pub fn join_notes(notes: &mut Vec<Note>, indices: &[usize]) -> Vec<usize> {
    let mut targets: Vec<usize> = if indices.is_empty() {
        (0..notes.len()).collect()
    } else {
        let mut t = indices.to_vec();
        t.sort_unstable();
        t.dedup();
        t
    };
    // Earliest first within a pitch, so a run is walked in the order it sounds.
    targets.sort_by(|&a, &b| {
        let (na, nb) = (notes[a], notes[b]);
        na.pitch
            .total_cmp(&nb.pitch)
            .then(na.start.total_cmp(&nb.start))
    });
    let mut absorbed: Vec<usize> = Vec::new();
    let mut head: Option<usize> = None;
    for i in targets {
        match head {
            Some(h)
                if notes[h].pitch == notes[i].pitch
                    && placement::adjacent(notes.placement(h), notes.placement(i), JOIN_TOL) =>
            {
                let joined = placement::merge(notes.placement(h), notes.placement(i));
                notes.set_placement(h, joined);
                absorbed.push(i);
            }
            _ => head = Some(i),
        }
    }
    if absorbed.is_empty() {
        return indices.to_vec();
    }
    // The survivors, re-indexed after the absorbed ones leave the list.
    let left: Vec<usize> = (0..notes.len()).filter(|i| !absorbed.contains(i)).collect();
    let kept: Vec<usize> = indices
        .iter()
        .filter(|i| !absorbed.contains(i))
        .map(|&i| left.iter().position(|&j| j == i).unwrap_or(i))
        .collect();
    remove_notes(notes, &absorbed);
    kept
}

/// How near two notes' edges must be to count as touching — half a sample, the
/// same tolerance every other "did this actually move" question uses.
const JOIN_TOL: f64 = 0.5;

/// Quantize note onsets to the `grid` (timeline samples) — the shared
/// [`placement::quantize`], which a lane's clips run the same way.
pub fn quantize_notes(notes: &mut [Note], indices: &[usize], grid: f64) -> bool {
    placement::quantize(notes, indices, grid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::paint::Mesh;
    use crate::host::theme::Theme;

    /// The bounds of an edit inside a domain that ends at `l` — a clip's body.
    fn limited(l: f64) -> Bounds {
        Bounds {
            limit: Some(l),
            ..Bounds::default()
        }
    }

    /// The bounds of an edit with its own floor.
    fn floor(min_dur: f64, limit: Limit) -> Bounds {
        Bounds {
            grid: 0.0,
            min_dur,
            limit,
        }
    }

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
    /// the edge — where a zoomed-in roll would stack every note above it into
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
    /// alternative — sliding the whole bar back inside, which is what clamping
    /// its top did — draws the note on a row that is not its own, and the note
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
    fn move_clamps_pitch_and_start() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, -50.0, 200.7, 24.0, 96.0, Bounds::default());
        assert_eq!(notes[0].start, 0.0);
        assert_eq!(notes[0].pitch, 96.0); // clamped to hi, rounded
        assert_eq!(notes[0].dur, 200.0); // duration kept
    }

    #[test]
    fn a_move_inside_a_limit_keeps_the_whole_note_in() {
        // Unbounded (a roll's own view): the note goes where it is dropped, and
        // the roll's span grows with it.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, 5000.0, 60.0, 24.0, 96.0, Bounds::default());
        assert_eq!(notes[0].start, 5000.0);
        // Bounded (a clip's body): the **tail** stops at the edge, so the last
        // start is limit - dur and the note stays whole and visible.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, 5000.0, 60.0, 24.0, 96.0, limited(1000.0));
        assert_eq!((notes[0].start, notes[0].dur), (800.0, 200.0));
        // A note longer than the clip pins to the near edge: its tail cannot
        // fit, so the edge that can be honoured is zero.
        let mut notes = vec![Note::new(0.0, 400.0, 60.0)];
        move_note(&mut notes, 0, 300.0, 60.0, 24.0, 96.0, limited(200.0));
        assert_eq!(notes[0].start, 0.0);
    }

    #[test]
    fn resize_respects_min_dur_from_either_edge() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        // Drag the end back past the start → clamped to min_dur.
        resize_note(&mut notes, 0, Part::End, 50.0, floor(10.0, None));
        assert_eq!(notes[0].dur, 10.0);
        // Drag the start forward past the end → clamped.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)]; // end = 300
        resize_note(&mut notes, 0, Part::Start, 400.0, floor(10.0, None));
        assert_eq!(notes[0].start, 290.0);
        assert_eq!(notes[0].dur, 10.0);
    }

    #[test]
    fn a_resize_stops_the_tail_at_the_limit() {
        // The end edge dragged past the clip's own length stops there.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        resize_note(&mut notes, 0, Part::End, 5000.0, floor(10.0, Some(1000.0)));
        assert_eq!(notes[0].dur, 900.0);
        // The start edge holds the end still, so a note already inside stays
        // inside with no far edge of its own.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        resize_note(&mut notes, 0, Part::Start, 50.0, floor(10.0, Some(1000.0)));
        assert_eq!((notes[0].start, notes[0].dur), (50.0, 250.0));
    }

    /// A cut leaves two notes end to end, and joining them back leaves what
    /// was there — the roll's `e` and `j`, which are the clip's own two verbs
    /// over a list the host holds itself.
    #[test]
    fn a_split_and_a_join_are_inverses() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        let sel = split_notes(&mut notes, &[], 150.0);
        assert_eq!(sel, vec![0, 1]);
        assert_eq!((notes[0].start, notes[0].dur), (100.0, 50.0));
        assert_eq!((notes[1].start, notes[1].dur), (150.0, 150.0));
        assert_eq!(notes[1].pitch, 60.0, "the second half is the same note");
        let sel = join_notes(&mut notes, &sel);
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0].start, notes[0].dur), (100.0, 200.0));
        assert_eq!(sel, vec![0]);
    }

    /// A cut on an edge is not a cut, and neither is one outside the note.
    #[test]
    fn a_cut_that_would_leave_nothing_is_no_cut() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        assert!(split_notes(&mut notes, &[], 100.0).is_empty());
        assert!(split_notes(&mut notes, &[], 300.0).is_empty());
        assert!(split_notes(&mut notes, &[], 50.0).is_empty());
        assert_eq!(notes.len(), 1);
    }

    /// **A pitch is what makes two notes one voice**, which is the roll's
    /// answer to the lane a clip's join is confined to: notes that touch join,
    /// notes on another pitch do not, and neither do notes with a gap.
    #[test]
    fn a_join_takes_what_touches_on_the_same_pitch() {
        let mut notes = vec![
            Note::new(0.0, 100.0, 60.0),
            Note::new(100.0, 100.0, 60.0), // touches the first
            Note::new(100.0, 100.0, 64.0), // another voice
            Note::new(400.0, 100.0, 60.0), // a gap
        ];
        let sel = join_notes(&mut notes, &[]);
        assert_eq!(notes.len(), 3);
        assert_eq!((notes[0].start, notes[0].dur), (0.0, 200.0));
        assert!(
            sel.is_empty(),
            "nothing was selected, nothing is left selected"
        );
        assert!(notes.iter().any(|n| n.pitch == 64.0 && n.dur == 100.0));
        assert!(notes.iter().any(|n| n.start == 400.0));
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
        move_notes_from(&mut notes, &orig, 50.0, 2.4, 24.0, 96.0, None);
        assert_eq!((notes[0].start, notes[0].pitch), (50.0, 62.0));
        assert_eq!((notes[1].start, notes[1].pitch), (250.0, 66.0));
        // Clamped at time zero: the whole block stops, keeping the spread.
        let mut notes = three_notes();
        move_notes_from(&mut notes, &orig, -80.0, 0.0, 24.0, 96.0, None);
        assert_eq!((notes[0].start, notes[1].start), (0.0, 200.0));
        // Clamped at the pitch top: the highest note pins the block.
        let mut notes = three_notes();
        move_notes_from(&mut notes, &orig, 0.0, 40.0, 24.0, 96.0, None);
        assert_eq!((notes[0].pitch, notes[1].pitch), (92.0, 96.0));
        // Durations are never touched.
        assert_eq!(notes[0].dur, 100.0);
    }

    #[test]
    fn a_block_wider_than_the_pitch_window_does_not_fold() {
        let mut notes = vec![Note::new(0.0, 10.0, 20.0), Note::new(0.0, 10.0, 100.0)];
        let orig = vec![(0, 0.0, 20.0f32), (1, 0.0, 100.0f32)];
        move_notes_from(&mut notes, &orig, 0.0, 5.0, 24.0, 96.0, None);
        // The rigid pitch move is refused; the pitches only clamp into range.
        assert_eq!((notes[0].pitch, notes[1].pitch), (24.0, 96.0));
    }

    #[test]
    fn a_block_move_stops_its_last_tail_at_the_limit() {
        // The block's far end is its last note's **tail** (400 + 100 = 500), so
        // inside a 600-long clip it may only move 100 further, spread intact.
        let mut notes = three_notes();
        let orig = vec![(0, 0.0, 60.0f32), (2, 400.0, 72.0f32)];
        move_notes_from(&mut notes, &orig, 5000.0, 0.0, 24.0, 96.0, Some(600.0));
        assert_eq!((notes[0].start, notes[2].start), (100.0, 500.0));
        assert_eq!(notes[2].dur, 100.0); // durations are never touched
        // A block wider than the clip pins to zero: the near edge is applied
        // last, the same choice a single over-long note makes.
        let mut notes = three_notes();
        move_notes_from(&mut notes, &orig, 50.0, 0.0, 24.0, 96.0, Some(200.0));
        assert_eq!((notes[0].start, notes[2].start), (0.0, 400.0));
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
        draw_pitch_labels(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            grid(),
            55.0,
            67.0,
        );
        assert!(mesh.vertex_count() > 0, "a readable C row gets its name");
        // Eight octaves over the same height: sub-3px rows — nothing draws.
        let mut mesh = Mesh::new();
        draw_pitch_labels(
            &mut Draw::new(&mut mesh, &Metrics::default(), &Theme::default()),
            grid(),
            12.0,
            108.0,
        );
        assert_eq!(mesh.vertex_count(), 0);
    }

    #[test]
    fn copy_normalizes_the_block_and_paste_replaces_it_selected() {
        let notes = three_notes();
        // Copy the last two: the block's first onset normalizes to 0.
        let clip = copy_notes(&notes, &[1, 2]);
        assert_eq!(clip.len(), 2);
        assert_eq!((clip[0].start, clip[0].pitch), (0.0, 64.0));
        assert_eq!((clip[1].start, clip[1].pitch), (200.0, 72.0));
        // Paste at 1000: appended with the spread kept, new indices returned.
        let mut notes = three_notes();
        let sel = paste_notes(&mut notes, &clip, 1000.0);
        assert_eq!(sel, vec![3, 4]);
        assert_eq!((notes[3].start, notes[4].start), (1000.0, 1200.0));
        assert_eq!(notes[4].pitch, 72.0);
        // A negative paste point clamps to the timeline start.
        let sel = paste_notes(&mut notes, &clip, -50.0);
        assert_eq!(notes[sel[0]].start, 0.0);
        // Copying nothing yields an empty clipboard.
        assert!(copy_notes(&notes, &[]).is_empty());
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
