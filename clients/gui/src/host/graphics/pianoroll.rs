//! The piano-roll graphic primitives: a note grid, a piano keyboard gutter, a
//! velocity lane and an OSC-event lane, all pure over a [`Draw`] (the
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
use serde_json::Value;

use crate::host::font;
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
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

/// One OSC event marker on the event lane: its `time` (timeline samples,
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
/// The OSC event lane height, device pixels.
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

/// Draw the OSC event lane: a flag at each marker's time, with its label.
pub fn draw_osc_lane(d: &mut Draw, lane: Rect, nav: &View, offset: f64, marks: &[OscMark]) {
    let (mesh, m, theme) = d.parts();
    if lane.w <= 0.0 || lane.h <= 0.0 {
        return;
    }
    mesh.rect(lane, theme.event_lane);
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

/// The far edge an edit stops at: the length of the domain the notes live in,
/// or `None` for a domain with no far edge.
///
/// A roll standing on its own has no far edge — its content *is* what it spans,
/// so a note dragged rightwards simply lengthens it. A roll drawn as a **clip's
/// body** has one: the clip's own `dur`, past which a note would still exist and
/// no longer be drawn, since the body is clipped to the rectangle. What is
/// edited has to stay visible, so the note stops at the edge and the clip's
/// length is changed the way a clip's length is changed — by its own edge.
pub type Limit = Option<f64>;

/// Where a note of `dur` may start inside `limit`: past zero, and near enough
/// the far edge that its **tail** still lands inside.
///
/// A note is clamped whole rather than by its onset, which is the difference
/// between a note that stops at the edge and one whose head stops there while
/// the rest of it goes over — the part that would vanish being exactly the part
/// being dragged. A note longer than the whole domain pins to zero: its tail
/// cannot fit, so the near edge is the one that can be honoured.
fn clamp_start(start: f64, dur: f64, limit: Limit) -> f64 {
    let last = limit.map_or(f64::INFINITY, |l| l - dur.max(0.0));
    start.min(last).max(0.0)
}

/// Move the note at `index` to a new start (clamped into `0..limit`, tail
/// included — see [`Limit`]) and pitch (clamped into `[lo, hi]`, rounded to the
/// nearest semitone). The duration is kept.
pub fn move_note(
    notes: &mut [Note],
    index: usize,
    start: f64,
    pitch: f32,
    lo: f32,
    hi: f32,
    limit: Limit,
) {
    if let Some(n) = notes.get_mut(index) {
        n.start = clamp_start(start, n.dur, limit);
        n.pitch = pitch.round().clamp(lo, hi);
    }
}

/// Resize the note at `index` by dragging one edge to timeline-relative `t`.
/// `Start` moves the onset (keeping the end fixed), `End` moves the end; a note
/// never shrinks below `min_dur`, and never grows past `limit`.
///
/// `Start` needs no far edge of its own: it holds the end still, so an edge
/// already inside stays inside. A note a script placed *over* the edge is left
/// where it is rather than dragged in — an edit moves what it is given hold of,
/// and pulling the far end in would be an edit nobody asked for.
pub fn resize_note(
    notes: &mut [Note],
    index: usize,
    part: NotePart,
    t: f64,
    min_dur: f64,
    limit: Limit,
) {
    if let Some(n) = notes.get_mut(index) {
        match part {
            NotePart::End => {
                let end = limit.map_or(t, |l| t.min(l));
                n.dur = (end - n.start).max(min_dur);
            }
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
/// deltas are clamped **as one** — no start below zero, no tail past `limit`,
/// no pitch outside `[lo, hi]` — so the block stops at an edge instead of
/// folding against it. Durations are kept.
pub fn move_notes_from(
    notes: &mut [Note],
    orig: &[(usize, f64, f32)],
    dt: f64,
    dp: f32,
    lo: f32,
    hi: f32,
    limit: Limit,
) {
    if orig.is_empty() {
        return;
    }
    let min_start = orig
        .iter()
        .map(|(_, s, _)| *s)
        .fold(f64::INFINITY, f64::min);
    // The block's far end is its last **tail**, so it stops where a single note
    // would. Read from the snapshot's starts and the notes' current durations:
    // a block move never touches a duration.
    let max_end = orig
        .iter()
        .filter_map(|(i, s, _)| notes.get(*i).map(|n| s + n.dur.max(0.0)))
        .fold(f64::NEG_INFINITY, f64::max);
    // The near edge is applied last, so a block longer than the whole domain
    // pins to zero rather than to a negative start — the same choice a single
    // over-long note makes.
    let dt = match limit {
        Some(l) if max_end.is_finite() => dt.min(l - max_end).max(-min_start),
        _ => dt.max(-min_start),
    };
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
        move_note(&mut notes, 0, -50.0, 200.7, 24.0, 96.0, None);
        assert_eq!(notes[0].start, 0.0);
        assert_eq!(notes[0].pitch, 96.0); // clamped to hi, rounded
        assert_eq!(notes[0].dur, 200.0); // duration kept
    }

    #[test]
    fn a_move_inside_a_limit_keeps_the_whole_note_in() {
        // Unbounded (a roll's own view): the note goes where it is dropped, and
        // the roll's span grows with it.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, 5000.0, 60.0, 24.0, 96.0, None);
        assert_eq!(notes[0].start, 5000.0);
        // Bounded (a clip's body): the **tail** stops at the edge, so the last
        // start is limit - dur and the note stays whole and visible.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        move_note(&mut notes, 0, 5000.0, 60.0, 24.0, 96.0, Some(1000.0));
        assert_eq!((notes[0].start, notes[0].dur), (800.0, 200.0));
        // A note longer than the clip pins to the near edge: its tail cannot
        // fit, so the edge that can be honoured is zero.
        let mut notes = vec![Note::new(0.0, 400.0, 60.0)];
        move_note(&mut notes, 0, 300.0, 60.0, 24.0, 96.0, Some(200.0));
        assert_eq!(notes[0].start, 0.0);
    }

    #[test]
    fn resize_respects_min_dur_from_either_edge() {
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        // Drag the end back past the start → clamped to min_dur.
        resize_note(&mut notes, 0, NotePart::End, 50.0, 10.0, None);
        assert_eq!(notes[0].dur, 10.0);
        // Drag the start forward past the end → clamped.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)]; // end = 300
        resize_note(&mut notes, 0, NotePart::Start, 400.0, 10.0, None);
        assert_eq!(notes[0].start, 290.0);
        assert_eq!(notes[0].dur, 10.0);
    }

    #[test]
    fn a_resize_stops_the_tail_at_the_limit() {
        // The end edge dragged past the clip's own length stops there.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        resize_note(&mut notes, 0, NotePart::End, 5000.0, 10.0, Some(1000.0));
        assert_eq!(notes[0].dur, 900.0);
        // The start edge holds the end still, so a note already inside stays
        // inside with no far edge of its own.
        let mut notes = vec![Note::new(100.0, 200.0, 60.0)];
        resize_note(&mut notes, 0, NotePart::Start, 50.0, 10.0, Some(1000.0));
        assert_eq!((notes[0].start, notes[0].dur), (50.0, 250.0));
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
