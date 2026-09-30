//! `notes` -- the editor-grade piano roll: a keyboard gutter, a note grid and an
//! OSC lane, placed on a navigation group's shared time axis. A note's velocity
//! is drawn inside it, as its fill, and edited on it: Shift and a vertical drag.
//!
//! **The leaf that is placed on somebody else's axis and edits what is drawn on
//! it**, which is why it is the last but one of the port. Everything it draws
//! is mapped through [`Ctx::time`] -- the group's window, its shared selection
//! and where its playhead stands -- so a roll linked to a lane zooms, pans and
//! plays with it, and the same element fills a clip's [`Notes`](BodyRole::Notes)
//! body by being handed the clip's axis instead.
//!
//! Its drags are all **snapshotted**: the press records the note it grabbed and
//! that note's position, and every step is measured against the *current* axis
//! rather than a press-time copy of it. That is not a stylistic choice -- a drag
//! held past the edge of a lane asks the machine to keep scrolling
//! ([`Take::edge_scroll`](crate::host::widget::element::Take::edge_scroll)), so
//! the window moves under the drag and a snapshot
//! of it would freeze the note where the axis used to be.
//!
//! **Two things it cannot do for itself**, and it asks for both the way `keys`
//! asks for a voice. The **time selection** a marquee sweeps is the navigation
//! group's -- every linked view follows it -- so the element names the span and
//! the machine writes it ([`Events::and_select`]). And **live MIDI** arrives
//! from a device only a front can open: the element declares [`Needs::midi`]
//! and paints what comes back ([`Element::midi`]), keeping the held keys and
//! the step cursor that used to live in two maps on the native front.

use clausters_core::osc::OscType;
use serde_json::{Map, Value};

use crate::host::graphics::pianoroll::{self, Pitches};
use crate::host::layout::Rect;
use crate::host::metrics::Metrics;
use crate::host::paint::Draw;
use crate::host::structures::boxes::{self, Bounds};
use crate::host::structures::notes::OscMark;
use crate::host::structures::notes::{self, Note};
use crate::host::widget::element::{
    BodyRole, Claim, Ctx, Element, Events, Input, Key, KeyInput, MidiNote, Needs, OnAxis, Swept,
    Take, TimeSpace,
};
use crate::host::widget::parse::{self, label, number, number_f64, set_label, truthy};
use crate::host::widget::{EditorProps, GestureMap, Ruler};
use crate::host::{font, ruler};
use crate::viewport::View;

/// The default pitch compass of a roll: the range of an 88-key piano.
const PITCH_MIN: f32 = 21.0;
const PITCH_MAX: f32 = 108.0;

/// A note's height on a roll in hertz, in logical pixels.
const HZ_NOTE_H: f32 = 8.0;

/// The shortest note a resize may leave, in axis units.
const MIN_DUR: f64 = 1.0;

/// A piano roll. `selected`, `drag`, `held` and `step` are native view state --
/// the gestures and the MIDI leg build them and no `/gui_set` writes them.
#[derive(Debug, Clone)]
pub struct Notes {
    notes: Vec<notes::Note>,
    osc: Vec<notes::OscMark>,
    /// The multi-note selection (note indices). It clears when a script
    /// replaces `notes`, since the indices would dangle over the new list.
    selected: Vec<usize>,
    min: f32,
    max: f32,
    snap: f64,
    /// Whether the notes carry the ids of the events they draw (`note_ids`),
    /// and so whether a report names each one.
    ids: bool,
    /// The last `note_ids` set, kept for a `notes` that arrives after it.
    ///
    /// **A set's keys come in whatever order the sender's map keeps**, and a
    /// map that sorts them puts `note_ids` before `notes` -- so a new list
    /// cleared the ids it had just been given, every note of the next report
    /// read as one the hand made, and each gesture rewrote the whole sequence
    /// as new events that had lost what the roll cannot draw. The list that
    /// arrives is named by the ids beside it whichever of the two came first.
    id_list: Option<Value>,
    osc_lane: bool,
    midi_in: bool,
    label: Option<String>,
    /// The MIDI spec the notes are written for, as a reader names it, shown
    /// beside the ruler; empty for notes for the server.
    midi: String,
    editor: EditorProps,
    drag: Option<Drag>,
    /// The live-MIDI keys currently down: `(channel, pitch)` and the note each
    /// one is writing into.
    held: Vec<((i32, i32), usize)>,
    /// Where step entry writes the next note when the transport is stopped.
    step: f64,
    /// Whether a hand may edit what is drawn here.
    ///
    /// **The picture must not follow a hand that cannot edit.** A body over
    /// samples that is a *rendering* -- the notes of a pattern -- is read-only,
    /// and an owner refusing the edit afterwards is too late: the roll has
    /// already offered the drag, drawn it for its whole duration and unwound
    /// it, which reads as a broken editor rather than as samples that cannot
    /// be edited here. So the refusal happens at the press, where it is seen.
    editable: bool,
    /// The lanes under the plane: curves over the whole sequence.
    rows: Vec<curves::Row>,
    /// The notes' own curves, each over the note it names.
    layers: Vec<curves::Layer>,
    /// The points the owner last sent, by curve.
    curve_points: std::collections::HashMap<String, Vec<f64>>,
    /// Each curve's body, by name.
    bodies: std::collections::HashMap<String, crate::host::elements::curve::Curve>,
    /// The curve last pressed: the one whose segments bend.
    layer: Option<String>,
    /// The curve a gesture holds, and its points before it.
    holding: Option<(String, Value)>,
}

mod curves;

/// What a held press on a roll is doing. Each carries the **press-time data**
/// it is measured from and no geometry: the rectangle and the axis arrive with
/// every step, because both may move under the drag.
#[derive(Debug, Clone)]
enum Drag {
    /// One note moving in time and pitch, or one of its edges resizing it.
    Note {
        index: usize,
        part: boxes::Part,
        press_time: f64,
        orig_start: f64,
        orig_dur: f64,
    },
    /// The whole selection moving rigidly: `orig` is `(index, start, pitch)`
    /// per selected note, the grabbed note leading (it is the snap anchor).
    Block {
        press_time: f64,
        press_pitch: f32,
        orig: Vec<(usize, f64, f32)>,
    },
    /// Velocities nudged by the vertical distance from the press, one step a
    /// pixel: the grabbed note's, or every selected one's when it is selected.
    Level {
        press_y: f64,
        orig: Vec<(usize, i32)>,
    },
}

/// Which region of a roll a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region {
    Grid,
    Osc,
    /// The time-ruler strip, or anything else on the axis: it reads a time and
    /// nothing else.
    Axis,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `notes` node carries, read once -- shared by the constructor, by
/// the tests beside it, and by a container that draws a roll as a **body** and
/// builds one through the same door rather than by naming fields.
pub(crate) fn from_props(props: &Map<String, Value>) -> Notes {
    let osc = parse_osc(props);
    let mut notes = parse_notes(props);
    let ids = set_ids(&mut notes, props.get("note_ids"));
    let mut roll = Notes {
        notes,
        ids,
        id_list: None,
        // The OSC lane shows when there are markers or it is explicitly asked
        // for (so an empty lane can still be opened to author them).
        osc_lane: props
            .get("osc_lane")
            .and_then(truthy)
            .unwrap_or(!osc.is_empty()),
        osc,
        selected: Vec::new(),
        min: number(props, "min", PITCH_MIN),
        max: number(props, "max", PITCH_MAX),
        snap: number_f64(props, "snap", 0.0).max(0.0),
        midi_in: props.get("midi_in").and_then(truthy).unwrap_or(false),
        label: label(props),
        midi: props
            .get("midi")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        editor: EditorProps::parse(props, crate::host::widget::RulerY::Off),
        drag: None,
        held: Vec::new(),
        step: 0.0,
        editable: props.get("editable").and_then(truthy).unwrap_or(true),
        rows: curves::parse_rows(props),
        layers: curves::parse_layers(props),
        curve_points: curves::parse_points(props),
        bodies: std::collections::HashMap::new(),
        layer: None,
        holding: None,
    };
    roll.rebuild_bodies();
    // What arrived is in the domain's unit, and the rows are pitches: a roll in
    // hertz reads its notes and its compass through the one conversion.
    if roll.hz() {
        for i in 0..roll.notes.len() {
            roll.notes[i].pitch = roll.row_of(f64::from(roll.notes[i].pitch));
        }
        if props.contains_key("min") {
            roll.min = roll.row_of(f64::from(roll.min));
        }
        if props.contains_key("max") {
            roll.max = roll.row_of(f64::from(roll.max));
        }
    }
    roll
}

impl Notes {
    /// Whether the Y domain is **hertz** (`axes.y.unit` of `"hz"`): the axis is
    /// then read and reported in hertz, ruled in round frequencies, and a
    /// pitch snaps to nothing. Every other roll is in MIDI notes.
    fn hz(&self) -> bool {
        self.editor.ruler_y == crate::host::widget::RulerY::Hz
    }

    /// The row a value of the domain sits on: the value itself for MIDI
    /// notes, the pitch a frequency is for hertz ([`pianoroll`]'s note on why
    /// that is the same axis).
    fn row_of(&self, value: f64) -> f32 {
        if self.hz() {
            clausters_core::scale::hz_to_midi(value.max(1e-3)) as f32
        } else {
            value as f32
        }
    }

    /// The value of the domain a row is -- what is reported.
    fn value_of(&self, row: f32) -> f32 {
        if self.hz() {
            clausters_core::scale::midi_to_hz(f64::from(row)) as f32
        } else {
            row
        }
    }

    /// The notes as the wire has them, each pitch in the domain's unit.
    fn wire_notes(&self) -> Vec<Note> {
        let mut out = self.notes.clone();
        for note in &mut out {
            note.pitch = self.value_of(note.pitch);
        }
        out
    }

    /// **A note's height on a continuous axis**: fixed, so zooming the
    /// frequencies does not grow the bars and the line at a bar's centre is
    /// where its frequency is; `None` on the keys, where a note is its row.
    fn bar(&self, m: &Metrics) -> Option<f32> {
        self.hz().then(|| (HZ_NOTE_H * m.ui_scale).round())
    }

    /// **The roll's vertical axis** as it is drawn now: the keys' rows, or a
    /// line in hertz with its bar ([`Pitches`]).
    fn axis(&self, m: &Metrics) -> Pitches {
        let (lo, hi) = self.pitch_window();
        match self.bar(m) {
            Some(bar) => Pitches::line(lo, hi, bar),
            None => Pitches::rows(lo, hi),
        }
    }

    /// **Where a hand at `y` puts a note** that was at `held`. On the keys:
    /// the nearest key inside the window, the note keeping how far off its key
    /// it was -- its bend -- so a microtone is transposed and never rounded. On
    /// a line: the frequency under the hand, rounded to what a pixel measures
    /// there ([`Self::round_hz`]).
    fn placed_pitch(&self, axis: Pitches, grid: Rect, y: f32, held: f32) -> f32 {
        let under = axis.pitch(y, grid);
        if axis.is_line() {
            return self.round_hz(under, axis, grid);
        }
        let (low, high) = (axis.lo.ceil(), axis.hi.floor().max(axis.lo.ceil()));
        under.round().clamp(low, high) + (held - held.round())
    }

    /// **A frequency a hand can mean**: `pitch` as hertz, rounded to the 1-2-5
    /// step just above what one pixel spans there -- so a note dragged at a
    /// wide zoom lands on 440 rather than 437.23, and a close zoom gives finer
    /// steps -- and back to its pitch, inside the window.
    fn round_hz(&self, pitch: f32, axis: Pitches, grid: Rect) -> f32 {
        use clausters_core::scale::{hz_to_midi, midi_to_hz};

        let per_px = f64::from(axis.span()) / f64::from(grid.h.max(1.0));
        let hz = midi_to_hz(f64::from(pitch));
        let spanned = hz * (2f64.powf(per_px / 12.0) - 1.0);
        let decade = 10f64.powf(spanned.max(1e-9).log10().floor());
        let step = [1.0, 2.0, 5.0, 10.0]
            .into_iter()
            .map(|k| k * decade)
            .find(|s| *s >= spanned)
            .unwrap_or(10.0 * decade);
        let rounded = ((hz / step).round() * step).max(step);
        (hz_to_midi(rounded) as f32).clamp(axis.lo, axis.hi)
    }

    /// What a pitch snaps to: a semitone, or nothing on a continuous axis.
    fn step(&self) -> f32 {
        if self.hz() { 0.0 } else { 1.0 }
    }

    /// The regions this placement is split into -- the same call the drawing and
    /// the hit-test both make, so a note is grabbed by the pixels it is
    /// painted on.
    ///
    /// The lanes, when there are any, take the bottom of the plane (and of the
    /// keyboard beside it, where their labels go).
    fn regions(&self, rect: Rect, indent: f32, m: &Metrics) -> pianoroll::Regions {
        let mut r = pianoroll::regions(
            rect,
            self.editor.ruler != Ruler::Off,
            self.osc_lane,
            indent,
            m,
        );
        let rows = self.rows_h(r.grid.h);
        r.grid.h -= rows;
        r.keyboard.h -= rows;
        r
    }

    /// The visible MIDI pitch window `[lo, hi]`: the `[min, max]` compass sliced
    /// by the vertical display window, so a pitch zoom holds the way the heavy
    /// views' amplitude and frequency windows do.
    fn pitch_window(&self) -> (f32, f32) {
        let (y0, yl) = self.editor.y_view();
        let span = (self.max - self.min) as f64;
        let lo = self.min as f64 + y0 * span;
        (lo as f32, (lo + yl * span) as f32)
    }

    /// The axis this roll is drawn against: the container's when it was placed
    /// on one, else its own content spanned over the body -- the fallback a view
    /// that has not joined a group yet still draws through.
    fn view(&self, time: Option<TimeSpace>) -> View {
        match time {
            Some(t) => t.view,
            None => View::full(self.span().ceil().max(1.0) as usize),
        }
    }

    /// **Which MIDI the notes are written for**, in the cell under the
    /// keyboard beside the ruler -- the one place in the roll a word about
    /// the whole of it belongs, and empty otherwise. Nothing when the notes
    /// are for the server, or when the roll has no such cell.
    fn draw_midi_spec(&self, d: &mut Draw, ctx: &Ctx) {
        if self.midi.is_empty() {
            return;
        }
        let full = pianoroll::regions(
            ctx.rect,
            self.editor.ruler != Ruler::Off,
            self.osc_lane,
            ctx.indent,
            ctx.metrics,
        );
        let top = full.keyboard.y + full.keyboard.h;
        let cell = Rect::new(
            full.keyboard.x,
            top,
            full.keyboard.w,
            ctx.rect.y + ctx.rect.h - top,
        );
        let (mesh, m, theme) = d.parts();
        if cell.w <= m.pad * 2.0 || cell.h < font::height(m.caption_scale) {
            return;
        }
        font::text(
            mesh,
            &self.midi,
            cell.x + m.pad,
            cell.y + (cell.h - font::height(m.caption_scale)) * 0.5,
            m.caption_scale,
            theme.ruler_text,
        );
    }

    /// How far this roll's own content reaches on the axis: the end of its last
    /// note and of its last event.
    fn span(&self) -> f64 {
        let notes = self.notes.iter().map(|n| n.start + n.dur.max(0.0));
        let events = self.osc.iter().map(|m| m.time);
        notes.chain(events).fold(0.0f64, f64::max)
    }

    /// The `"notes"` edit-back payload: the tag plus the flat `start dur pitch
    /// velocity channel` quintuple list -- the wire form the roll and the clip
    /// share, in the owner's own units -- or, for a roll whose notes carry ids,
    /// sextuples with the id first (0 for a note the hand made).
    fn notes_event(&self) -> Events {
        let mut args = vec![OscType::String("notes".into())];
        // A split or a paste copies a note whole, id included: the note that
        // repeats an earlier one's id is a new one, and says so with 0.
        let mut named = std::collections::HashSet::new();
        for n in &self.notes {
            if self.ids {
                let id = if n.id != 0 && named.insert(n.id) {
                    n.id
                } else {
                    0
                };
                // An id is at most 2^53 on a wire that carries JSON numbers,
                // and a double names every integer up to there.
                args.push(OscType::Double(id as f64));
            }
            args.push(OscType::Float(n.start as f32));
            args.push(OscType::Float(n.dur as f32));
            args.push(OscType::Float(self.value_of(n.pitch)));
            args.push(OscType::Int(n.velocity));
            args.push(OscType::Int(n.channel));
        }
        Events::message(args)
    }

    /// The axis position a cursor x maps back to, through the grid.
    fn time_at(&self, grid: Rect, nav: &View, x: f64) -> f64 {
        pianoroll::time_at(grid, nav, 0.0, x as f32)
    }

    /// Where a press landed, resolved against the placement it was drawn at.
    fn hit(&self, at: (f64, f64), input: &Input) -> Hit {
        let r = self.regions(input.rect, input.indent, input.metrics);
        let nav = self.view(input.time);
        let axis = self.axis(input.metrics);
        let (fx, fy) = (at.0 as f32, at.1 as f32);
        if self.osc_lane && r.osc.contains(at.0, at.1) {
            // No marker index: the lane shows and does not write, so which
            // marker the pointer is nearest is nobody's question here.
            return Hit {
                region: Region::Osc,
                grid: r.grid,
                nav,
                axis,
                note: None,
            };
        }
        let region = if r.grid.contains(at.0, at.1) {
            Region::Grid
        } else {
            Region::Axis
        };
        Hit {
            region,
            grid: r.grid,
            nav,
            axis,
            note: (region == Region::Grid)
                .then(|| pianoroll::note_hit(r.grid, &nav, 0.0, &self.notes, axis, fx, fy))
                .flatten(),
        }
    }

    /// Inserts a note at `start`/`pitch` for the live-MIDI leg and the Ctrl+add
    /// gesture, returning its index.
    fn insert(&mut self, note: notes::Note) -> usize {
        notes::insert_note(&mut self.notes, note)
    }

    /// The length a note is painted with when nothing said otherwise: the note
    /// grid, else a visible sliver of the window.
    fn default_dur(&self, nav: &View) -> f64 {
        if self.snap > 0.0 {
            self.snap
        } else {
            (nav.len * 0.05).max(MIN_DUR)
        }
    }
}

/// Where a press landed and what it landed on.
struct Hit {
    region: Region,
    grid: Rect,
    nav: View,
    axis: Pitches,
    note: Option<pianoroll::NoteHit>,
}

impl Element for Notes {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            // Arrays ride a `/gui_set` as their JSON -- the scalar carrier a set
            // of a non-scalar always uses.
            "editable" => {
                let Some(on) = truthy(v) else { return false };
                self.editable = on;
                true
            }
            "notes" => {
                self.notes = parse_notes(&parse::as_array_props("notes", v));
                for i in 0..self.notes.len() {
                    self.notes[i].pitch = self.row_of(f64::from(self.notes[i].pitch));
                }
                // The indices would dangle over the new list. The ids are the
                // ones set beside it, if they came first (`id_list`).
                self.selected.clear();
                self.held.clear();
                if let Some(ids) = self.id_list.take()
                    && ids
                        .as_array()
                        .is_some_and(|ids| ids.len() == self.notes.len())
                {
                    self.ids = set_ids(&mut self.notes, Some(&ids));
                }
                true
            }
            "note_ids" => {
                let ids = parse::as_array_props("note_ids", v);
                self.ids = set_ids(&mut self.notes, ids.get("note_ids"));
                self.id_list = ids.get("note_ids").cloned();
                true
            }
            "osc" => {
                self.osc = parse_osc(&parse::as_array_props("osc", v));
                true
            }
            "curves" => {
                self.rows = curves::parse_rows(&parse::as_array_props("curves", v));
                self.rebuild_bodies();
                true
            }
            "layers" => {
                self.layers = curves::parse_layers(&parse::as_array_props("layers", v));
                self.rebuild_bodies();
                true
            }
            "points" => {
                self.curve_points = curves::parse_points(&parse::as_array_props("points", v));
                self.rebuild_bodies();
                true
            }
            "min" => v.as_f64().map(|x| self.min = self.row_of(x)).is_some(),
            "max" => v.as_f64().map(|x| self.max = self.row_of(x)).is_some(),
            "snap" => v.as_f64().map(|x| self.snap = x.max(0.0)).is_some(),
            "osc_lane" => truthy(v).map(|b| self.osc_lane = b).is_some(),
            "midi_in" => truthy(v).map(|b| self.midi_in = b).is_some(),
            "label" => set_label(&mut self.label, v),
            "midi" => v
                .as_str()
                .map(|text| self.midi = text.to_string())
                .is_some(),
            _ => self.editor.apply(key, v),
        }
    }

    /// The whole picture, into the window's one mesh: the grid and its notes,
    /// the keyboard, the strips, and -- for a roll standing on its own axis --
    /// the chrome of that axis over them.
    ///
    /// The chrome is drawn here rather than by the frame because a roll's is
    /// not the heavy views': its ruler sits under a grid with two strips
    /// between them, its selection band covers the grid alone, and its readout
    /// names a pitch. What it needs from outside is the axis' own three facts,
    /// and those arrive on [`Ctx::time`].
    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        let r = self.regions(ctx.rect, ctx.indent, ctx.metrics);
        let nav = self.view(ctx.time);
        let axis = self.axis(ctx.metrics);
        let (lo, hi) = (axis.lo, axis.hi);
        if self.hz() {
            pianoroll::draw_hz_grid(d, r.grid, axis);
        } else {
            pianoroll::draw_grid_background(d, r.grid, lo, hi);
        }
        pianoroll::draw_notes(
            d,
            r.grid,
            r.grid,
            &nav,
            0.0,
            &self.notes,
            axis,
            true,
            &self.selected,
        );
        if self.hz() {
            pianoroll::draw_hz_ruler(d, r.keyboard, axis);
        } else {
            pianoroll::draw_keyboard(d, r.keyboard, lo, hi);
            // A key a MIDI note cannot have -- a frequency past the compass,
            // written from a roll in hertz -- is marked at the edge it is past.
            let compass = (self.min, self.max);
            pianoroll::draw_out_of_range(d, r.grid, &nav, 0.0, &self.notes, axis, compass);
        }
        if self.osc_lane {
            pianoroll::draw_osc_lane(d, r.osc, &nav, 0.0, &self.osc);
        }
        self.draw_curves(d, ctx);
        if let Some(text) = &self.label {
            let (mesh, m, theme) = d.parts();
            font::text(
                mesh,
                text,
                r.grid.x + m.pad,
                r.grid.y + 2.0,
                m.caption_scale,
                theme.ruler_text,
            );
        }
        self.draw_midi_spec(d, ctx);
        let rate = self.rate(ctx.world.sample_rate);
        if self.editor.ruler != Ruler::Off {
            // The strip sits under the grid, aligned to the grid's x range --
            // the "body" the tick math derives it from.
            let body = Rect::new(r.grid.x, ctx.rect.y, r.grid.w, r.ruler.y - ctx.rect.y);
            crate::host::frame::draw_time_ruler(d, ctx.rect, body, &nav, rate, &self.editor);
        }
        self.draw_axis_chrome(d, ctx, r.grid, &nav, rate);
    }

    fn info(&self) -> Vec<(String, Value)> {
        // Each list as the JSON string its own `/gui_set` accepts: a query
        // gives back exactly what a set would take.
        vec![
            (
                "notes".into(),
                Value::from(notes::notes_json(&self.wire_notes()).to_string()),
            ),
            (
                "note_ids".into(),
                Value::from(
                    Value::from(self.notes.iter().map(|n| n.id).collect::<Vec<_>>()).to_string(),
                ),
            ),
            (
                "osc".into(),
                Value::from(notes::osc_json(&self.osc).to_string()),
            ),
        ]
    }

    fn needs(&self) -> Needs {
        Needs {
            // A roll follows the transport, so the window has to keep repainting
            // **while one is running** -- which is what the anchor says, and what
            // this asked for unconditionally until 2026-08-22. A roll that is
            // merely on screen has nothing moving in it, and a window that
            // repaints anyway repaints for as long as the page is open. The
            // rule is the `score`'s, for the same reason: the element that
            // carries its own anchor is the one that can answer.
            clock: self.editor.playhead_at >= 0.0,
            midi: self.midi_in,
            ..Needs::default()
        }
    }

    fn hover_readout(&self) -> bool {
        true
    }

    fn body_role(&self) -> Option<BodyRole> {
        Some(BodyRole::Notes)
    }

    /// A clip's body: the same notes over the clip's own axis, with no keyboard,
    /// no strips and no chrome.
    fn draw_body(&self, d: &mut Draw, rect: Rect, time: &TimeSpace) {
        let (lo, hi) = (self.min, self.max);
        let local = &time.view;
        pianoroll::draw_notes(
            d,
            rect,
            rect,
            local,
            0.0,
            &self.notes,
            Pitches::rows(lo, hi),
            false,
            &[],
        );
        pianoroll::draw_pitch_labels(d, rect, lo, hi);
    }

    /// The roll takes every press on its own axis (its notes, its strips, its
    /// marquee) and leaves the modifier that is the *container's* -- Shift pans
    /// the window, which is the axis' gesture and not the picture's.
    /// A note first, then the **container's marquee** over the empty grid --
    /// the same plan a lane carries, because it is the same gesture. Shift is
    /// the axis' pan, as on every timeline view.
    ///
    /// **The plan says `marquee`, and for a while it said `select`** -- which
    /// sweeps the notes *and* writes the shared time span, so a sweep over the
    /// grid left a band behind it. A selection of notes is a selection of the
    /// rectangles the notes are, exactly as a patcher's is of its boxes; a
    /// **time range** over the same grid is the other selection, and it is
    /// asked for by name (`{"drag": "select"}`), which is the rule a lane
    /// already states.
    fn gesture_map(&self) -> Option<GestureMap> {
        use crate::host::widget::GestureStep::{Element as El, Marquee, Pan, Range};
        // **Alt sweeps a time range**, the audio editor's selection, anywhere
        // on the view -- the plain drag is the marquee over the contents, so
        // the span needs a modifier of its own -- and an Alt click that never
        // moves is still the element's toggle of what it lands on.
        Some(GestureMap::of_plans(
            &[El, Marquee],
            &[Pan],
            &[El, Marquee],
            &[Range],
        ))
    }

    /// **The notes the rectangle covered**, and the band of semitones it
    /// crossed.
    ///
    /// The whole of what this element knows about a marquee: the gesture, the
    /// anchor and the shared time span are the machine's, and this answers the
    /// one question only the roll can -- with the same call its own marquee
    /// used to make, over the same geometry the notes were drawn on.
    ///
    /// **A rectangle that crossed no whole semitone restricts nothing**, which
    /// is the click it still is vertically: a pitch axis is discrete, so the
    /// band takes the semitones the sweep passed *over*, exactly as the time
    /// axis takes the samples it did.
    ///
    /// Only the grid answers. The strips under it (velocity, markers) and the
    /// ruler beside it read time alone, so a sweep begun there sets the span
    /// and takes no notes -- and it still lets go of what was held, because a
    /// press is a marquee of no size and every one of them means the same
    /// thing.
    fn select_in(&mut self, from: (f64, f64), to: (f64, f64), input: &Input) -> Swept {
        let before = self.selected.len();
        self.selected.clear();
        let h = self.hit(from, input);
        if h.region != Region::Grid || !self.editable {
            return Swept {
                changed: before > 0,
                band: None,
            };
        }
        let (t0, t1) = (
            self.time_at(h.grid, &h.nav, from.0),
            self.time_at(h.grid, &h.nav, to.0),
        );
        let p0 = h.axis.pitch(from.1 as f32, h.grid);
        let p1 = h.axis.pitch(to.1 as f32, h.grid);
        self.selected = notes::notes_in_rect(&self.notes, t0, t1, p0, p1);
        // Whole rows on the keys; on a line, the pitches the sweep crossed.
        let (a, b) = if h.axis.is_line() {
            (p0.min(p1), p0.max(p1))
        } else {
            (p0.min(p1).ceil(), p0.max(p1).floor())
        };
        // A rectangle that never left its row restricts nothing -- the click it
        // still is vertically. The ceil/floor pair says so on its own for a
        // sweep inside a row; a press, whose two corners are one point, would
        // land exactly on a row's own pitch and read as that one semitone.
        let flat = (p0 - p1).abs() < f32::EPSILON;
        Swept {
            changed: before > 0 || !self.selected.is_empty(),
            band: (b >= a && !flat).then_some((a as f64, b as f64)),
        }
    }

    /// **The roll's own contents are its notes** -- a note's rectangle. The
    /// grid between them is the
    /// container's, which is what leaves a clip's empty roll to the clip's own
    /// move.
    ///
    /// A read-only roll answers `false` everywhere: pointing at the notes of a
    /// rendering is not a request to edit them, so the press goes to the clip
    /// and the clip moves. The refusal in [`press`](Self::press) stays for the
    /// layer a script activated on purpose.
    fn layer_hit(&self, at: (f64, f64), input: &Input) -> bool {
        if !self.editable {
            return false;
        }
        let h = self.hit(at, input);
        h.region == Region::Grid && h.note.is_some()
    }

    fn press(&mut self, at: (f64, f64), input: &Input) -> Claim {
        // A curve's own contents first: it is drawn over what it shapes.
        if let Some(claim) = self.press_curve(at, input) {
            return claim;
        }
        let h = self.hit(at, input);
        // **Read-only is answered before the drag, not after it.** The press is
        // consumed so nothing behind it turns a refused edit into a selection,
        // and it says why -- a refusal with nothing attached teaches *sometimes
        // it does not work* rather than *not here*.
        if !self.editable && h.region == Region::Grid {
            return Claim::Take(Take {
                events: Events::refused(
                    "notes",
                    "these notes are a rendering of an algorithm: render it to a track to edit them",
                ),
                ..Take::default()
            });
        }
        // **A body claims its own parts and declines everywhere else.** Inside a
        // clip the rest of the rectangle means the clip's own drag (move it,
        // resize it), so a body grabs a note and hands back anything else.
        let is_body = input.time.is_some() && !self.navigable_placement(input);
        match h.region {
            Region::Grid => self.press_grid(&h, at, input, is_body),
            Region::Osc => self.press_osc(&h, at, input),
            // The ruler strip and the slack beside the body: a time and nothing
            // else, so the sweep is time-only.
            // The ruler strip and the slack beside the body: nothing of this
            // element's is there, so the press goes back to the chain and the
            // container sweeps -- the marquee is the machine's now.
            Region::Axis => Claim::Decline,
        }
    }

    /// The notes follow the hand; **the edit leaves on release**.
    ///
    /// One gesture is one edit -- the rule `Drag::Draw` and `Drag::Sample`
    /// already state at their own release. A value per frame is a document edit
    /// per frame: an undo history of a hundred steps for one dragged note, and
    /// a hundred round trips whose acknowledgements the next frame outruns.
    fn drag(&mut self, at: (f64, f64), input: &Input) -> Events {
        if self.drag_curve(at, input) {
            return Events::none();
        }
        let r = self.regions(input.rect, input.indent, input.metrics);
        let nav = self.view(input.time);
        let axis = self.axis(input.metrics);
        let (lo, hi) = (axis.lo, axis.hi);
        let time = self.time_at(r.grid, &nav, at.0);
        let limit = self.edit_limit(input);
        match self.drag.clone() {
            Some(Drag::Note {
                index,
                part,
                press_time,
                orig_start,
                orig_dur,
            }) => {
                let bounds = self.edit_bounds(input);
                match part {
                    boxes::Part::Body => {
                        let start = orig_start + (time - press_time);
                        let held = self.notes.get(index).map_or(0.0, |n| n.pitch);
                        let pitch = self.placed_pitch(axis, r.grid, at.1 as f32, held);
                        // The duration is asserted **first**: the clamp against
                        // the far edge measures the note's tail, so a duration
                        // a `set` changed under a running drag has to be the one
                        // in hand before the start is placed against it.
                        if let Some(n) = self.notes.get_mut(index) {
                            n.dur = orig_dur;
                        }
                        // Placed already -- a key and its bend, or a round
                        // frequency -- and inside what the window shows.
                        notes::move_note(
                            &mut self.notes,
                            index,
                            start,
                            pitch,
                            lo - 0.5,
                            hi + 0.5,
                            0.0,
                            bounds,
                        );
                    }
                    other => notes::resize_note(&mut self.notes, index, other, time, bounds),
                }
                Events::none()
            }
            Some(Drag::Block {
                press_time,
                press_pitch,
                orig,
            }) => {
                // The grabbed note (the leading snapshot entry) snaps to the
                // grid and the whole selection moves rigidly by that delta --
                // the core clamps it as one.
                let dt = match orig.first() {
                    Some((_, s0, _)) => snap_to(s0 + (time - press_time), self.snap) - s0,
                    None => 0.0,
                };
                let mut dp = axis.pitch(at.1 as f32, r.grid) - press_pitch;
                // On a line the grabbed note lands on a round frequency and the
                // rest keep their distance from it; on the keys the block moves
                // by whole semitones, each note keeping its bend.
                if let Some(&(_, _, lead)) = orig.first()
                    && axis.is_line()
                {
                    dp = self.round_hz(lead + dp, axis, r.grid) - lead;
                }
                let step = self.step();
                notes::move_notes_from(&mut self.notes, &orig, dt, dp, lo, hi, step, limit);
                Events::none()
            }
            Some(Drag::Level { press_y, orig }) => {
                let dv = ((press_y - at.1) / f64::from(input.scale.max(0.1))).round() as i32;
                notes::nudge_velocities_from(&mut self.notes, &orig, dv);
                Events::none()
            }
            None => Events::none(),
        }
    }

    fn release(&mut self, at: (f64, f64), inside: bool, input: &Input) -> Events {
        if let Some(events) = self.release_curve(at, inside, input) {
            return events;
        }
        // What the drag amounts to, once -- see `drag`. A marquee edited
        // nothing: it swept a selection, which is screen state and was reported
        // as it went.
        match self.drag.take() {
            None => Events::none(),
            Some(_) => self.notes_event(),
        }
    }

    /// The block operations, addressed to whatever the pointer is over: `q`
    /// quantizes, `e` splits and `j` joins, Delete removes the selection,
    /// Ctrl+C/X/V move a block through the host-wide clipboard.
    ///
    /// They are keys rather than gestures because they act on the *selection*,
    /// which is already where the pointer has been. A key this element has no
    /// arm for falls through to the front's own shortcuts.
    fn key(&mut self, key: &Key, input: &mut KeyInput) -> Option<Events> {
        match key {
            // Quantize the selected onsets (all of them when nothing is
            // selected) to the note grid -- the same grid a drag snaps to.
            Key::Char('q') | Key::Char('Q') if !input.mods.ctrl => Some(
                if notes::quantize_notes(&mut self.notes, &self.selected, self.snap) {
                    self.notes_event()
                } else {
                    Events::refused("quantize", "these notes are already on the grid")
                },
            ),
            // **Split and join**, the clip's own two verbs over notes -- same
            // keys, same reading. A clip asks its owner to cut, because the
            // owner holds the element; a roll holds its notes and cuts them
            // itself, which is the whole of the difference.
            //
            // The cut falls on the **window's cursor**, which is where a paste
            // lands too: a roll's key gestures have no pointer to read, and the
            // window has one cursor for exactly this. Step entry's own position
            // stands in where the roll is on no axis (a bare roll nothing has
            // located yet).
            // **Only over a selection.** A roll drawn as a *clip's body* shares
            // these two letters with the clip they belong to, and the clip is
            // what a lane's hand is on: with nothing selected the key falls
            // through and cuts the clip, which is what `e` has always meant
            // there. Selecting notes first is how you say you meant the notes.
            // It is also the sane reading on its own -- splitting every note in
            // the roll is not something anyone asks for by leaning on a letter.
            Key::Char('e') | Key::Char('E') if !input.mods.ctrl && !self.selected.is_empty() => {
                let at = snap_to(self.anchor(input), self.snap).max(0.0);
                let cut = notes::split_notes(&mut self.notes, &self.selected, at);
                if cut.is_empty() {
                    return Some(Events::refused(
                        "split",
                        "the cursor is not inside a held note",
                    ));
                }
                self.selected = cut;
                Some(self.notes_event())
            }
            Key::Char('j') | Key::Char('J') if !input.mods.ctrl && !self.selected.is_empty() => {
                let before = self.notes.len();
                self.selected = notes::join_notes(&mut self.notes, &self.selected);
                Some(if self.notes.len() == before {
                    // The roll's own four conditions, said as one sentence: a
                    // join is a **pitch's**, and what joins is what touches.
                    Events::refused(
                        "join",
                        "a join is one pitch's, and these notes do not touch on one",
                    )
                } else {
                    self.notes_event()
                })
            }
            Key::Delete | Key::Backspace if !self.selected.is_empty() => {
                let held = std::mem::take(&mut self.selected);
                boxes::discard(&mut self.notes, &held).then(|| self.notes_event())
            }
            // The clipboard is the host's one string, so a block travels
            // between rolls and windows -- and rides it in the same JSON form a
            // `/gui_set notes` accepts, which is the carrier every non-scalar
            // already uses.
            Key::Char('c') | Key::Char('C') | Key::Char('x') | Key::Char('X')
                if input.mods.ctrl =>
            {
                let block = notes::copy_notes(&self.notes, &self.selected);
                if block.is_empty() {
                    return None;
                }
                input
                    .clipboard
                    .set_text(&notes::notes_json(&block).to_string());
                let cut = matches!(key, Key::Char('x') | Key::Char('X'));
                if !cut {
                    // A copy changed nothing, so it reports nothing -- but it
                    // consumed the key.
                    return Some(Events::none());
                }
                notes::remove_notes(&mut self.notes, &self.selected);
                self.selected.clear();
                Some(self.notes_event())
            }
            Key::Char('v') | Key::Char('V') if input.mods.ctrl => {
                let block = clipboard_notes(&input.clipboard.text())?;
                // **At the cursor**: what is pasted starts where the window's
                // cursor is, playing or not -- a paste has no pointer, and the
                // cursor is the one position the window keeps.
                let at = snap_to(self.anchor(input), self.snap).max(0.0);
                self.selected = notes::paste_notes(&mut self.notes, &block, at);
                Some(self.notes_event())
            }
            _ => None,
        }
    }

    /// A live note: painted at the running playhead (recording), or on the step
    /// cursor when the transport is stopped (step entry -- a chord shares one
    /// step, and the last key up advances it).
    fn midi(&mut self, note: MidiNote, playhead: Option<f64>) -> Option<Events> {
        let key = (note.channel, note.pitch);
        if note.retune {
            // An MPE bend moved: the held note is painted where it now is.
            let &(_, index) = self.held.iter().find(|(k, _)| *k == key)?;
            let n = self.notes.get_mut(index)?;
            n.pitch = note.pitch as f32 + note.bend;
            return Some(self.notes_event());
        }
        if note.on {
            let dur = if self.snap > 0.0 { self.snap } else { MIN_DUR };
            let start = match playhead {
                Some(p) => snap_to(p, self.snap).max(0.0),
                None => self.step,
            };
            let index = self.insert(notes::Note {
                id: 0,
                start,
                dur,
                pitch: note.pitch as f32 + note.bend,
                velocity: note.velocity,
                channel: note.channel,
            });
            self.held.push((key, index));
            return Some(self.notes_event());
        }
        let pos = self.held.iter().position(|(k, _)| *k == key)?;
        let (_, index) = self.held.remove(pos);
        match playhead {
            // Recording: the key was held this long.
            Some(now) => {
                if let Some(n) = self.notes.get_mut(index) {
                    n.dur = (now - n.start).max(MIN_DUR);
                }
            }
            // Step entry: the last key up advances the cursor one grid.
            None if self.held.is_empty() => {
                self.step += if self.snap > 0.0 { self.snap } else { MIN_DUR };
            }
            None => {}
        }
        Some(self.notes_event())
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn on_axis(&self) -> Option<&dyn OnAxis> {
        Some(self)
    }

    fn on_axis_mut(&mut self) -> Option<&mut dyn OnAxis> {
        Some(self)
    }
}

impl OnAxis for Notes {
    fn navigates_time(&self) -> bool {
        true
    }

    fn editor(&self) -> Option<&EditorProps> {
        Some(&self.editor)
    }

    fn editor_mut(&mut self) -> Option<&mut EditorProps> {
        Some(&mut self.editor)
    }

    /// The keyboard gutter, which is the roll's own structural geometry. What
    /// it actually gets is its group's shared indent -- this when it is alone on
    /// its axis, wider when it shares one with a lane.
    fn gutter(&self, _m: &Metrics) -> f32 {
        pianoroll::KEYBOARD_W
    }

    /// The grid is the body a sample maps into -- not the rect minus its chrome,
    /// because the velocity and event strips are stacked *under* the grid and
    /// read the same axis. The keyboard gutter is always a vertical surface, so
    /// a wheel over it navigates the pitch window whatever `ruler_y` says.
    fn axis_body(&self, rect: Rect, indent: f32, m: &Metrics) -> Option<(Rect, bool)> {
        Some((self.regions(rect, indent, m).grid, true))
    }

    fn content_span(&self) -> Option<f64> {
        Some(self.span())
    }

    /// **A roll's time goes on past its last note**, as a multitrack's does
    /// past its last clip: the bars after the notes are where the next ones
    /// are written, a range drawn across them is a range, and a loop may end
    /// there. Its extent is where the notes happen to end, not where time does.
    fn unbounded_axis(&self) -> bool {
        true
    }

    /// The keyboard's gutter scrolls through the octaves, and Ctrl zooms them.
    fn wheel_pans_y(&self) -> bool {
        true
    }
}

impl Notes {
    #[cfg(test)]
    /// The pitch window this roll is drawn in -- its own `min`/`max`, for a
    /// container that fitted them and wants to check it did.
    pub(crate) fn range(&self) -> (f32, f32) {
        (self.min, self.max)
    }

    #[cfg(test)]
    /// The multi-note selection, for the crate's own gesture suite -- which
    /// drives a real host and has no other way to see it (it is view state, so
    /// no `/gui_query` reports it).
    pub(crate) fn selected(&self) -> &[usize] {
        &self.selected
    }

    /// **Where an anchored key gesture acts**: the window's cursor, and step
    /// entry's own position where the roll is on no axis with one.
    ///
    /// One cursor is the window's rule, so a paste and a cut read it rather than
    /// keeping a position of their own. [`Notes::step`] is what is left of the
    /// old anchor: it is where step entry writes, it advances as chords are
    /// entered, and it stands in for the cursor on a roll nothing has located.
    fn anchor(&self, input: &KeyInput) -> f64 {
        input.cursor.unwrap_or(self.step)
    }

    /// Whether this placement is the roll's **own** view rather than a clip's
    /// body: a body is handed a container's axis and draws none of the chrome
    /// that would make it navigable.
    fn navigable_placement(&self, input: &Input) -> bool {
        input.indent > 0.0
    }

    /// The far edge this placement's edits stop at (see [`notes::Limit`]).
    ///
    /// **A clip's body has one and the roll's own view has none**, and the
    /// difference is what the two placements can do about a note past the end.
    /// The roll spans its own content: drag a note rightwards and the span
    /// grows, the axis has somewhere further to go, and the note is one scroll
    /// away -- nothing is lost, so nothing is stopped. A body is drawn *inside
    /// the clip's rectangle* and clipped to it: the same drag would leave the
    /// note out of every pixel the clip owns, still in the list, visible only by
    /// resizing the clip by hand. So the body stops at the clip's `dur`, and the
    /// clip's length stays what its own edge says it is -- content does not
    /// silently lengthen the thing containing it.
    fn edit_limit(&self, input: &Input) -> notes::Limit {
        match input.time {
            Some(t) if !self.navigable_placement(input) => Some(t.span),
            _ => None,
        }
    }

    /// What bounds one note drag: the roll's own snap grid, the note floor and
    /// the far edge this placement's edits stop at.
    fn edit_bounds(&self, input: &Input) -> Bounds {
        Bounds {
            grid: self.snap,
            min_dur: MIN_DUR,
            limit: self.edit_limit(input),
        }
    }

    /// The rate the ruler and the readout are placed on: the widget's own when
    /// it names one, else the server's.
    fn rate(&self, world_rate: f64) -> f64 {
        if self.editor.sample_rate > 0.0 {
            self.editor.sample_rate
        } else {
            world_rate
        }
    }

    /// The axis' chrome over the grid: the shared selection band, the playhead,
    /// and the cursor readout naming the pitch and the time under the pointer.
    fn draw_axis_chrome(&self, d: &mut Draw, ctx: &Ctx, grid: Rect, nav: &View, rate: f64) {
        let Some(time) = ctx.time else {
            return;
        };
        let (mesh, m, theme) = d.parts();
        let to_x =
            |s: f64| (grid.x as f64 + (s - nav.start) / nav.len.max(1.0) * grid.w as f64) as f32;
        // The sweep, through the one routine every view that lets a hand draw
        // one draws it with: the same half-sample edges as a waveform's, and
        // **the pitch band this roll's own marquee swept** -- which it used to
        // throw away, drawing a full-height stripe over a selection that held a
        // few semitones of it.
        let axis = self.axis(m);
        crate::host::graphics::selection::draw_span(
            &mut Draw::new(mesh, m, theme),
            grid,
            nav,
            time.sel,
            1,
            self.editor.value_range(),
            crate::host::graphics::selection::Vertical::Pitch(axis),
        );
        // The position cursor first, then the playhead over it: two lines that
        // mean two things, and where they coincide the music's is the one that
        // reads.
        for (pos, color) in [(time.cursor, theme.cursor), (time.head, theme.playhead)] {
            if let Some(pos) = pos
                && pos >= nav.start
                && pos <= nav.start + nav.len
            {
                mesh.rect(Rect::new(to_x(pos), grid.y, m.trace_w, grid.h), color);
            }
        }
        // The readout: the note name under the cursor and the time, in the
        // grid's bottom-right corner.
        let Some((cx, cy)) = ctx.world.cursor.filter(|(x, y)| grid.contains(*x, *y)) else {
            return;
        };
        let row = self.axis(m).pitch(cy as f32, grid);
        let s = nav.start + nav.len * ((cx - grid.x as f64) / grid.w.max(1.0) as f64);
        let time = match self.editor.ruler {
            Ruler::Samples => ruler::readout_samples(s),
            Ruler::Beats => ruler::readout_beats(
                s,
                rate,
                self.editor.tempo,
                self.editor.beat_at,
                self.editor.quant,
                self.editor.tempo_map.as_deref(),
                nav.len / rate / grid.w.max(1.0) as f64,
            ),
            _ => ruler::readout_time(s, rate, nav.len / rate / grid.w.max(1.0) as f64),
        };
        let value = if self.hz() {
            format!("{:.1} Hz", self.value_of(row))
        } else {
            clausters_core::scale::note_name(row.round() as i32)
        };
        let text = format!("{value}  {time}");
        // Right-aligned **inside the grid**: a roll drawn as a clip's body is
        // as wide as the clip, so a read-out placed at its own width alone
        // starts left of the box and is read over whatever is drawn there. It
        // drops its tail first -- the time -- and keeps the note name, which is
        // the half a pointer on a pitch is asking for; a grid with no room for
        // the ellipsis draws nothing.
        let room = grid.w - 2.0 * m.pad;
        let w = font::width(&text, m.caption_scale).min(room);
        let (x, y) = (
            grid.x + grid.w - w - m.pad,
            grid.y + grid.h - font::height(m.caption_scale) - 2.0,
        );
        let (scale, color) = (m.caption_scale, theme.ruler_text);
        crate::host::graphics::plate_text(d, &text, x, y, room, scale, color);
    }

    /// A press on the note grid: Alt toggles a note in or out of the selection,
    /// Ctrl adds or removes one, a note moves or resizes (a **selected** note
    /// moves the whole selection), and empty grid sweeps the marquee.
    fn press_grid(&mut self, h: &Hit, at: (f64, f64), input: &Input, is_body: bool) -> Claim {
        // **Shift and a vertical drag is the velocity**, drawn inside the note
        // it belongs to rather than in a lane of its own: the grabbed note's,
        // or the whole selection's when the grabbed note is in it.
        if input.mods.shift {
            let Some(nh) = h.note else {
                return Claim::Decline;
            };
            let which: Vec<usize> = if self.selected.contains(&nh.index) {
                self.selected.clone()
            } else {
                vec![nh.index]
            };
            let orig = which
                .iter()
                .filter_map(|&i| self.notes.get(i).map(|n| (i, n.velocity)))
                .collect();
            self.drag = Some(Drag::Level {
                press_y: at.1,
                orig,
            });
            return Claim::take();
        }
        if input.mods.alt {
            let Some(nh) = h.note else {
                return Claim::Decline;
            };
            notes::toggle_selected(&mut self.selected, nh.index);
            return Claim::take();
        }
        if input.mods.ctrl {
            match h.note {
                // Ctrl on a note removes it; the selection's indices shift down
                // past it.
                Some(nh) => {
                    notes::remove_note(&mut self.notes, nh.index);
                    self.selected = notes::selection_after_removal(&self.selected, nh.index);
                }
                // Ctrl on empty grid adds one there, then drags its end to set
                // the length until release.
                None if !is_body => {
                    let time = snap_to(self.time_at(h.grid, &h.nav, at.0), self.snap).max(0.0);
                    let pitch = self.placed_pitch(h.axis, h.grid, at.1 as f32, 0.0);
                    let dur = self.default_dur(&h.nav);
                    let index = self.insert(notes::Note::new(time, dur, pitch));
                    self.drag = Some(Drag::Note {
                        index,
                        part: boxes::Part::End,
                        press_time: time,
                        orig_start: time,
                        orig_dur: dur,
                    });
                }
                None => return Claim::Decline,
            }
            return Claim::events(self.notes_event()).edge_scrolling();
        }
        let Some(nh) = h.note else {
            // Nothing of this element's, inside a clip: the press goes back to
            // the container, whose own drag is what the rest of the rectangle
            // means. Standing alone the whole grid is the roll's, and the empty
            // part of it sweeps.
            // Empty grid is the container's sweep: this element answers what
            // the rectangle caught ([`Element::select_in`]) and holds no drag
            // of its own for it.
            return Claim::Decline;
        };
        let press_time = self.time_at(h.grid, &h.nav, at.0);
        if nh.part == boxes::Part::Body {
            // Grabbing a **selected** note moves the whole selection; grabbing
            // an unselected one drops the selection and moves singly.
            if self.selected.contains(&nh.index) {
                let mut idx = self.selected.clone();
                idx.retain(|&i| i != nh.index);
                // The grabbed note leads: it is the snap anchor.
                idx.insert(0, nh.index);
                let orig: Vec<_> = idx
                    .iter()
                    .filter_map(|&i| self.notes.get(i).map(|n| (i, n.start, n.pitch)))
                    .collect();
                if !orig.is_empty() {
                    self.drag = Some(Drag::Block {
                        press_time,
                        press_pitch: h.axis.pitch(at.1 as f32, h.grid),
                        orig,
                    });
                    return Claim::take().edge_scrolling();
                }
            }
            self.selected.clear();
        }
        let (orig_start, orig_dur) = self
            .notes
            .get(nh.index)
            .map_or((0.0, 0.0), |n| (n.start, n.dur));
        self.drag = Some(Drag::Note {
            index: nh.index,
            part: nh.part,
            press_time,
            orig_start,
            orig_dur,
        });
        Claim::take().edge_scrolling()
    }

    /// A press on the **markers lane**, which shows and does not write.
    ///
    /// A roll is the editor of things that have a **pitch**: that is what its
    /// grid is a grid of. The other items a timeline holds have none -- an OSC
    /// message, raw MIDI bytes -- so they are drawn below it as markers, which
    /// is the decision this widget was built with (*"OSC events (which
    /// have no pitch) draw as flags in a separate lane below it"*) and the one
    /// the dedicated view recorded again (*"display-only for now: the
    /// `(time, label)` marker is a lossy view of the message, so writing it
    /// back would drop the args"*).
    ///
    /// The lane grew an add/remove/move gesture against that, and it could not
    /// have worked: **a marker is the message it sends**, the lane draws only
    /// its address, and there is no way to type one here -- so an added marker
    /// was a message with no destination, and a moved one was matched back to
    /// its item *by its label*, which two messages to one address share. Both
    /// clients saw the same press and answered differently, one refusing it
    /// with a sentence and the other keeping a marker that will never send
    /// anything.
    ///
    /// So a Ctrl press -- the one that meant to edit -- is refused out loud and
    /// consumed, and every other press declines to the container, the way the
    /// axis strip beside it does. **What is not decided here** is what a real
    /// editor of messages would be: it is multidimensional (an address, typed
    /// arguments, a destination that is a MIDI port for one item and an OSC
    /// server for another) and it is not a roll's. See the plan's "Messages are
    /// not the roll's to edit".
    fn press_osc(&mut self, h: &Hit, _at: (f64, f64), input: &Input) -> Claim {
        if input.mods.ctrl {
            return Claim::Take(Take {
                events: Events::refused(
                    "osc",
                    "the markers lane shows what a timeline holds besides notes, and does \
                     not write it: a marker is the message it sends, and its address is \
                     not something this lane can say",
                ),
                ..Take::default()
            });
        }
        let _ = h;
        Claim::Decline
    }
}

/// Snaps `t` to the `grid`, the one rounding every note edit shares -- and it
/// is [`boxes::snap`], the same one a clip's edge lands on. This used to be
/// a second spelling of it whose no-grid arm returned the raw value while its
/// own doc said whole units; the axis' unit is the sample, so "no grid" is the
/// finest grid there is.
fn snap_to(t: f64, grid: f64) -> f64 {
    boxes::snap(t, grid)
}

/// The notes on the host-wide clipboard, when what is on it is a note block --
/// the same flat quintuple JSON a `/gui_set notes` takes, so a block copied out
/// of one roll pastes into another and a field's text pastes into neither.
fn clipboard_notes(text: &str) -> Option<Vec<notes::Note>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let notes = parse_notes(&parse::as_array_props("notes", &value));
    (!notes.is_empty()).then_some(notes)
}

/// Parses a piano-roll clip's `notes`: a flat `[start, dur, pitch, ...]` array
/// (three numbers per note, the flat convention the `bpf` points use), each a
/// [`Note`]. A short/absent/malformed array yields no notes (the
/// clip then draws a waveform body).
fn parse_notes(props: &serde_json::Map<String, Value>) -> Vec<Note> {
    let Some(Value::Array(items)) = props.get("notes") else {
        return Vec::new();
    };
    // The canonical wire form is quintuples `start dur pitch velocity channel`
    // (what the Python builder always emits): a length that is a multiple of 5
    // is read as quintuples. Anything else is a plain `start dur pitch` triple
    // list (legacy / hand-authored), which still parses, defaulting velocity to
    // 100 on channel 0. A trailing partial group is dropped.
    let stride = if items.len() % 5 == 0 { 5 } else { 3 };
    items
        .chunks_exact(stride)
        .filter_map(|c| {
            let mut n = Note::new(
                c[0].as_f64()?.max(0.0),
                c[1].as_f64()?.max(0.0),
                c[2].as_f64()? as f32,
            );
            // A velocity or a channel is a number wherever it came from: a
            // sender that holds its list as floats (the document's catalogue
            // does) writes `90.0`, which read as an integer only and fell to
            // the default, so every note drew and reported velocity 100.
            if stride == 5 {
                n.velocity = c[3].as_f64().map_or(100, |v| v.round() as i32);
                n.channel = c[4].as_f64().map_or(0, |v| v.round() as i32);
            }
            Some(n)
        })
        .collect()
}

/// Names each note by the id its owner gave it: `ids` is the `note_ids` list,
/// in the order of the notes. Answers whether the notes now carry ids, which
/// is whether a report names them. A list shorter than the notes leaves the
/// rest unnamed.
fn set_ids(notes: &mut [Note], ids: Option<&Value>) -> bool {
    let Some(Value::Array(ids)) = ids else {
        return false;
    };
    for (note, id) in notes.iter_mut().zip(ids) {
        note.id = id.as_f64().map_or(0, |v| v.max(0.0) as u64);
    }
    true
}

/// Parse a `pianoroll`'s `osc` prop -- a flat `[time, label, time, label, ...]`
/// list of OSC markers (the label a short address/tag, an empty string
/// meaning none). A trailing partial pair is dropped.
fn parse_osc(props: &serde_json::Map<String, Value>) -> Vec<OscMark> {
    let Some(Value::Array(items)) = props.get("osc") else {
        return Vec::new();
    };
    items
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|c| {
            let time = c[0].as_f64()?.max(0.0);
            let label = c[1].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            Some(OscMark { time, label })
        })
        .collect()
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::host::widget::element::Mods;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    fn roll(json: &str) -> Notes {
        from_props(&props(json))
    }

    /// **A roll asks the window to follow the clock only while something is
    /// sweeping.** It asked for it unconditionally once, and a page holding a
    /// roll then repainted thirty times a second for as long as it was open,
    /// with nothing moving in it -- 59% of a browser's main thread on the
    /// composer example, against 3% once the anchor decides. The rule is the
    /// `score`'s: the element carrying the anchor is the one that can answer.
    #[test]
    fn a_roll_follows_the_clock_only_while_a_playhead_is_anchored() {
        let idle = roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0]}"#);
        assert!(
            !idle.needs().clock,
            "a roll with nothing sweeping in it keeps no window awake"
        );
        let sweeping = roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0],"playhead_at":480.0}"#);
        assert!(
            sweeping.needs().clock,
            "an anchored roll does: the line has to move"
        );
    }

    /// A roll placed on a navigation group: the indent is its keyboard gutter,
    /// the axis is the group's window over its content.
    fn input<'a>(m: &'a Metrics, rect: Rect, time: Option<TimeSpace>) -> Input<'a> {
        Input {
            metrics: m,
            rect,
            indent: pianoroll::KEYBOARD_W,
            scale: 1.0,
            mods: Mods::default(),
            viewport: (rect.w, rect.h),
            clicks: 1,
            time,
        }
    }

    fn axis(len: f64) -> Option<TimeSpace> {
        Some(TimeSpace::of(View { start: 0.0, len }, len))
    }

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 500.0, 400.0)
    }

    /// The x pixel a time falls on in the grid -- what the drawing maps and what
    /// a press has to invert.
    fn x_of(r: &Notes, m: &Metrics, t: f64, len: f64) -> f64 {
        let grid = r.regions(rect(), pianoroll::KEYBOARD_W, m).grid;
        grid.x as f64 + t / len * grid.w as f64
    }

    fn y_of(r: &Notes, m: &Metrics, pitch: f32) -> f64 {
        let grid = r.regions(rect(), pianoroll::KEYBOARD_W, m).grid;
        let (lo, hi) = r.pitch_window();
        pianoroll::pitch_to_y(pitch, lo, hi, grid) as f64
    }

    #[test]
    fn parses_defaults_and_the_wire_lists() {
        let r = roll("{}");
        assert_eq!((r.min, r.max), (PITCH_MIN, PITCH_MAX));
        assert!(!r.osc_lane && !r.midi_in && !r.ids);
        assert_eq!(r.snap, 0.0);
        assert!(r.notes.is_empty() && r.osc.is_empty());

        // The canonical quintuple form, and the OSC lane opening because
        // there are markers.
        let r = roll(
            r#"{"notes":[0.0,100.0,60.0,90,2],"osc":[50.0,"hit"],
                "min":48,"max":72,"snap":25.0,"midi_in":true}"#,
        );
        assert_eq!(r.notes.len(), 1);
        assert_eq!((r.notes[0].velocity, r.notes[0].channel), (90, 2));
        assert_eq!(r.osc.len(), 1);
        // The same note written as floats, as the catalogue serializes it.
        let floats = roll(r#"{"notes":[0.0,100.0,60.0,90.0,2.0]}"#);
        assert_eq!(
            (floats.notes[0].velocity, floats.notes[0].channel),
            (90, 2),
            "a float velocity is the same velocity"
        );
        assert!(r.osc_lane, "markers open their lane");
        assert!(r.midi_in);
        assert_eq!(r.snap, 25.0);
    }

    /// A `/gui_set` of a list rides as its JSON string and drops the selection,
    /// whose indices would dangle over the new list.
    #[test]
    fn apply_replaces_the_lists_and_drops_the_selection() {
        let mut r = roll(r#"{"notes":[0.0,10.0,60.0,100,0]}"#);
        r.selected = vec![0];
        assert!(r.set(
            "notes",
            &Value::from("[0.0,10.0,64.0,100,0,20.0,10.0,67.0,100,0]")
        ));
        assert_eq!(r.notes.len(), 2);
        assert!(r.selected.is_empty());
        assert!(r.set("osc", &Value::from("[5.0,\"a\"]")));
        assert_eq!(r.osc.len(), 1);
        assert!(r.set("snap", &Value::from(50.0)));
        // The editor chrome is the element's too, so its keys apply here.
        assert!(r.set("ruler", &Value::from("beats")));
        assert!(!r.set("nonesuch", &Value::from(1)));
    }

    /// The extent a group's axis reads off a roll is its content's, which is
    /// what lets a roll being written into lengthen the timeline.
    #[test]
    fn the_content_span_is_the_end_of_the_last_thing_on_it() {
        let r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"osc":[400.0,""]}"#);
        assert_eq!(r.content_span(), Some(400.0));
        let r = roll(r#"{"notes":[100.0,300.0,60.0,100,0]}"#);
        assert_eq!(r.content_span(), Some(400.0));
        assert_eq!(roll("{}").content_span(), Some(0.0));
    }

    /// A note is grabbed by the pixels it is drawn on, and the drag reports the
    /// whole list in the owner's own units.
    #[test]
    fn a_note_drag_moves_it_in_time_and_pitch_and_reports_the_list() {
        let m = Metrics::default();
        let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"min":48,"max":72}"#);
        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
        assert!(matches!(
            r.press(at, &input(&m, rect(), axis(1000.0))),
            Claim::Take(_)
        ));
        assert!(matches!(r.drag, Some(Drag::Note { index: 0, .. })));

        let to = (x_of(&r, &m, 250.0, 1000.0), y_of(&r, &m, 64.0));
        let events = r.drag(to, &input(&m, rect(), axis(1000.0)));
        assert!((r.notes[0].start - 200.0).abs() < 1.0, "{:?}", r.notes[0]);
        assert_eq!(r.notes[0].pitch, 64.0);
        assert_eq!(r.notes[0].dur, 100.0, "a move keeps the duration");
        // The note follows the hand and says nothing: one gesture is one edit.
        assert!(events.is_empty(), "a frame of a drag is not an edit");

        // The release is the edit, and it carries the whole list.
        let msgs = r
            .release(to, true, &input(&m, rect(), axis(1000.0)))
            .into_messages();
        assert_eq!(msgs[0][0], OscType::String("notes".into()));
        assert_eq!(msgs[0].len(), 1 + 5, "the tag plus a quintuple per note");
        assert!(r.drag.is_none());
    }

    /// **The ids name the new list whichever came first**: a sender whose map
    /// sorts its keys sets `note_ids` before `notes`, and the list that
    /// arrives second must not lose the ids that arrived first -- or the next
    /// report names every note 0, as one the hand made.
    #[test]
    fn a_list_set_after_its_ids_keeps_them() {
        let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"note_ids":[7]}"#);
        assert!(r.set("note_ids", &Value::from("[7,9]")));
        assert!(r.set(
            "notes",
            &Value::from("[0.0,10.0,62.0,100,0,20.0,10.0,64.0,100,0]")
        ));
        assert_eq!(r.notes.iter().map(|n| n.id).collect::<Vec<_>>(), vec![7, 9]);
        // And the other way round.
        assert!(r.set("notes", &Value::from("[0.0,10.0,65.0,100,0]")));
        assert!(r.set("note_ids", &Value::from("[11]")));
        assert_eq!(r.notes[0].id, 11);
    }

    /// **A roll in hertz reads, draws and reports hertz**, and a pitch moves
    /// continuously: its rows are the pitches the frequencies are, so the
    /// note is where a MIDI roll would draw it, and a drag snaps to nothing.
    #[test]
    fn a_roll_in_hertz_reads_and_reports_hertz_and_moves_continuously() {
        let m = Metrics::default();
        let mut r =
            roll(r#"{"notes":[0.0,100.0,440.0,100,0],"min":110.0,"max":1760.0,"ruler_y":"hz"}"#);
        assert!((r.notes[0].pitch - 69.0).abs() < 1e-4, "{:?}", r.notes[0]);
        assert!((r.min - 45.0).abs() < 1e-4 && (r.max - 93.0).abs() < 1e-4);
        let info = r.info();
        let wire: Vec<f64> = serde_json::from_str(info[0].1.as_str().unwrap()).unwrap();
        assert!(
            (wire[2] - 440.0).abs() < 1e-2,
            "a query answers hertz: {wire:?}"
        );

        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 69.0));
        r.press(at, &input(&m, rect(), axis(1000.0)));
        let to = (at.0, y_of(&r, &m, 69.5));
        r.drag(to, &input(&m, rect(), axis(1000.0)));
        let pitch = r.notes[0].pitch;
        assert!(
            (pitch - 69.5).abs() < 0.1,
            "not snapped to a semitone: {pitch}"
        );
        let msgs = r
            .release(to, true, &input(&m, rect(), axis(1000.0)))
            .into_messages();
        let OscType::Float(hz) = msgs[0][3] else {
            panic!("a float pitch: {:?}", msgs[0]);
        };
        let expected = clausters_core::scale::midi_to_hz(f64::from(pitch));
        assert!(
            (f64::from(hz) - expected).abs() < 1e-2,
            "reported in hertz: {hz}"
        );
    }

    /// **A note in hertz keeps its height at any zoom**, and on the keys it is
    /// its row, which grows with the zoom.
    #[test]
    fn a_note_in_hertz_keeps_its_height_at_any_zoom() {
        let m = Metrics::default();
        let r = roll(r#"{"notes":[0.0,100.0,440.0,100,0],"ruler_y":"hz"}"#);
        let grid = Rect::new(0.0, 0.0, 400.0, 400.0);
        let bar = r.bar(&m);
        assert!(bar.is_some());
        let bar = bar.unwrap();
        let wide = Pitches::line(20.0, 120.0, bar).note_height(grid);
        let close = Pitches::line(60.0, 72.0, bar).note_height(grid);
        assert_eq!(wide, close, "the same bar whatever the window");
        let keys = roll(r#"{"notes":[0.0,100.0,69.0,100,0]}"#);
        assert!(keys.bar(&m).is_none());
        assert!(
            Pitches::rows(60.0, 72.0).note_height(grid)
                > Pitches::rows(20.0, 120.0).note_height(grid)
        );
    }

    /// **In hertz a drag reaches the window's edges and lands on round
    /// frequencies**: the note's centre goes where the hand is, to the very
    /// edge of a zoomed window, and its frequency is rounded to what a pixel
    /// spans there.
    #[test]
    fn a_drag_in_hertz_reaches_the_edges_on_round_frequencies() {
        let m = Metrics::default();
        let mut r = roll(
            r#"{"notes":[0.0,100.0,440.0,100,0],"min":8.175798915643707,"max":20000.0,
                "ruler_y":"hz","y_start":0.5,"y_len":0.03}"#,
        );
        let pitches = r.axis(&m);
        let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
        let x = x_of(&r, &m, 50.0, 1000.0);
        let at = (x, f64::from(pitches.y(r.notes[0].pitch, grid)));
        assert!(
            matches!(
                r.press(at, &input(&m, rect(), axis(1000.0))),
                Claim::Take(_)
            ),
            "the note is under {at:?} in a window {pitches:?}"
        );
        // Past the top: the centre is at the window's top edge.
        r.drag(
            (x, f64::from(grid.y) - 20.0),
            &input(&m, rect(), axis(1000.0)),
        );
        let per_px = pitches.span() / grid.h;
        assert!(
            (r.notes[0].pitch - pitches.hi).abs() <= per_px,
            "{}",
            r.notes[0].pitch
        );
        // Past the bottom: its bottom edge.
        r.drag(
            (x, f64::from(grid.y + grid.h) + 20.0),
            &input(&m, rect(), axis(1000.0)),
        );
        assert!(
            (r.notes[0].pitch - pitches.lo).abs() <= per_px,
            "{}",
            r.notes[0].pitch
        );
        // In the middle: a round frequency, to what a pixel spans there -- a
        // step of a half hertz at this zoom.
        r.drag(
            (x, f64::from(grid.y + grid.h * 0.37)),
            &input(&m, rect(), axis(1000.0)),
        );
        let f = clausters_core::scale::midi_to_hz(f64::from(r.notes[0].pitch));
        assert!((f * 2.0 - (f * 2.0).round()).abs() < 1e-2, "{f}");
    }

    /// **On the keys a drag transposes a microtone and keeps its bend**: the
    /// box moves by whole semitones, the cents stay.
    #[test]
    fn a_drag_on_the_keys_keeps_a_notes_bend() {
        let m = Metrics::default();
        let mut r = roll(r#"{"notes":[0.0,100.0,60.37,100,0],"min":48,"max":72}"#);
        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
        assert!(matches!(
            r.press(at, &input(&m, rect(), axis(1000.0))),
            Claim::Take(_)
        ));
        let to = (at.0, y_of(&r, &m, 62.0));
        r.drag(to, &input(&m, rect(), axis(1000.0)));
        assert!(
            (r.notes[0].pitch - 62.37).abs() < 1e-4,
            "{}",
            r.notes[0].pitch
        );
    }

    /// **A roll navigates its compass and opens on its window**: `min`/`max`
    /// are the whole domain, and the view starts on the slice `y_start` and
    /// `y_len` name -- in hertz, read as the pitches its rows are.
    #[test]
    fn a_roll_opens_on_its_window_inside_the_whole_compass() {
        let keys = roll(
            r#"{"notes":[0.0,100.0,60.0,100,0],"min":0,"max":127,
                             "y_start":0.4,"y_len":0.2}"#,
        );
        let (lo, hi) = keys.pitch_window();
        assert!(
            (lo - 50.8).abs() < 1e-3 && (hi - 76.2).abs() < 1e-3,
            "{lo} {hi}"
        );
        let hz = roll(
            r#"{"notes":[0.0,100.0,440.0,100,0],"min":8.175798915643707,
                           "max":20000.0,"ruler_y":"hz","y_start":0.5,"y_len":0.1}"#,
        );
        let (lo, hi) = hz.pitch_window();
        let top = clausters_core::scale::hz_to_midi(20_000.0) as f32;
        assert!((lo - top * 0.5).abs() < 1e-2, "{lo}");
        assert!((hi - top * 0.6).abs() < 1e-2, "{hi}");
    }

    /// The hertz ruler marks round frequencies over the roll's window, each
    /// where its pitch is.
    #[test]
    fn the_hertz_ruler_marks_round_frequencies_at_their_pitches() {
        let ticks = pianoroll::hz_ticks(Pitches::line(45.0, 93.0, 8.0), 400.0, &Metrics::default());
        let labelled: Vec<_> = ticks.iter().filter(|(_, l)| l.is_some()).collect();
        assert!(!labelled.is_empty());
        for (pitch, label) in &labelled {
            assert!((44.5..=93.5).contains(pitch), "{pitch} {label:?}");
        }
        assert!(
            ticks.iter().any(|(p, l)| l.as_deref() == Some("200")
                && (f64::from(*p) - clausters_core::scale::hz_to_midi(200.0)).abs() < 0.01),
            "200 Hz is marked at its pitch: {ticks:?}"
        );
    }

    /// **A roll given ids names every note in its report**, the id first: a
    /// note the owner named keeps its id, and one the hand made -- or a copy
    /// that repeats an id, as a split's second half does -- is 0.
    #[test]
    fn a_roll_with_ids_reports_each_note_by_its_id() {
        let m = Metrics::default();
        let mut r = roll(
            r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,90,1],"note_ids":[7,9],
                "min":48,"max":72}"#,
        );
        assert!(r.ids);
        assert_eq!((r.notes[0].id, r.notes[1].id), (7, 9));
        let copy = r.notes[1];
        r.notes.push(copy);
        let msgs = r.notes_event().into_messages();
        assert_eq!(msgs[0].len(), 1 + 3 * 6, "the tag plus a sextuple per note");
        assert_eq!(msgs[0][1], OscType::Double(7.0));
        assert_eq!(msgs[0][7], OscType::Double(9.0));
        assert_eq!(msgs[0][13], OscType::Double(0.0), "the copy is a new note");
        // A new list drops the ids until its own arrive.
        assert!(r.set("notes", &Value::from("[0.0,10.0,60.0,100,0]")));
        assert_eq!(r.notes[0].id, 0);
        assert!(r.set("note_ids", &Value::from("[3]")));
        assert_eq!(r.notes[0].id, 3);
        let _ = m;
    }

    /// **Shift and a vertical drag is a note's velocity**, drawn in the note
    /// rather than in a lane: one step a pixel, the grabbed note's alone when
    /// it is not selected, and one edit at the release.
    #[test]
    fn shift_and_a_vertical_drag_sets_the_velocity_on_the_note() {
        let m = Metrics::default();
        let mut r =
            roll(r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0],"min":48,"max":72}"#);
        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
        let mut shifted = input(&m, rect(), axis(1000.0));
        shifted.mods.shift = true;
        assert!(matches!(r.press(at, &shifted), Claim::Take(_)));
        let events = r.drag((at.0, at.1 - 20.0), &shifted);
        assert!(events.is_empty());
        assert_eq!(r.notes[0].velocity, 120);
        assert_eq!(r.notes[1].velocity, 100, "only the grabbed note");
        assert_eq!(r.notes[0].pitch, 60.0, "and it does not move");
        r.drag((at.0, at.1 + 200.0), &shifted);
        assert_eq!(r.notes[0].velocity, 1, "never a note-off");
        let msgs = r.release(at, true, &shifted).into_messages();
        assert_eq!(msgs[0][0], OscType::String("notes".into()));
    }

    /// **A note drag is measured against the axis it is handed each step**, not
    /// against a press-time copy of it -- which is what lets the machine scroll
    /// the axis under a drag held past a lane's edge.
    #[test]
    fn a_drag_follows_an_axis_that_moves_under_it() {
        let m = Metrics::default();
        let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"min":48,"max":72}"#);
        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
        r.press(at, &input(&m, rect(), axis(1000.0)));
        // The same cursor, against a window that has panned 500 forward: the
        // note lands 500 later, because the pixel now names a later sample.
        let panned = Some(TimeSpace::of(
            View {
                start: 500.0,
                len: 1000.0,
            },
            2000.0,
        ));
        r.drag(at, &input(&m, rect(), panned));
        assert!((r.notes[0].start - 500.0).abs() < 1.0, "{:?}", r.notes[0]);
    }

    /// **The marquee is the machine's; what it caught is the roll's.** The
    /// gesture, the anchor and the shared time span belong to the container --
    /// every linked view follows that selection -- and the one question left
    /// for this element is which notes the rectangle covered, and in what band
    /// of its own axis.
    ///
    /// The press declines, because there is nothing of the roll's on empty
    /// grid; and a rectangle of no size is what lets go of the notes, which is
    /// the same rule on every view that sweeps.
    #[test]
    fn the_roll_answers_what_the_rectangle_caught() {
        let m = Metrics::default();
        let mut r = roll(
            r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0,
                         500.0,100.0,80.0,100,0],"min":48,"max":84}"#,
        );
        // A press on empty grid is not the roll's: the container sweeps.
        let at = (x_of(&r, &m, 0.0, 1000.0), y_of(&r, &m, 58.0));
        assert_eq!(
            r.press(at, &input(&m, rect(), axis(1000.0))),
            Claim::Decline
        );

        let to = (x_of(&r, &m, 400.0, 1000.0), y_of(&r, &m, 66.0));
        let swept = r.select_in(at, to, &input(&m, rect(), axis(1000.0)));
        // The pitch axis is discrete, so the band is the semitones the sweep
        // passed over -- 58 to 66 read as whole steps, both ends included.
        let (lo_p, hi_p) = swept.band.expect("a rectangle restricts the pitch axis");
        assert!(
            (58.0..=59.0).contains(&lo_p) && (65.0..=66.0).contains(&hi_p),
            "{lo_p} {hi_p}"
        );
        assert!(swept.changed);
        assert_eq!(r.selected, vec![0, 1], "the third note is out of the band");

        // A rectangle of no size covers nothing, which is the press: the notes
        // are let go of, and no band is reported.
        let swept = r.select_in(at, at, &input(&m, rect(), axis(1000.0)));
        assert!(r.selected.is_empty() && swept.changed);
        assert_eq!(swept.band, None, "a click restricts nothing vertically");
    }

    /// Grabbing a **selected** note moves the whole selection rigidly; grabbing
    /// an unselected one drops the selection and moves singly.
    #[test]
    fn a_block_moves_together_and_an_unselected_note_alone() {
        let m = Metrics::default();
        let mut r =
            roll(r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0],"min":48,"max":72}"#);
        r.selected = vec![0, 1];
        let at = (x_of(&r, &m, 50.0, 1000.0), y_of(&r, &m, 60.0));
        r.press(at, &input(&m, rect(), axis(1000.0)));
        assert!(matches!(r.drag, Some(Drag::Block { .. })));
        let to = (x_of(&r, &m, 150.0, 1000.0), y_of(&r, &m, 60.0));
        r.drag(to, &input(&m, rect(), axis(1000.0)));
        assert!((r.notes[0].start - 100.0).abs() < 1.0);
        assert!((r.notes[1].start - 300.0).abs() < 1.0, "rigid");

        // The other half: an unselected note drops the set.
        let mut r =
            roll(r#"{"notes":[0.0,100.0,60.0,100,0,200.0,100.0,64.0,100,0],"min":48,"max":72}"#);
        r.selected = vec![1];
        r.press(at, &input(&m, rect(), axis(1000.0)));
        assert!(matches!(r.drag, Some(Drag::Note { index: 0, .. })));
        assert!(r.selected.is_empty());
    }

    /// Ctrl adds a note where there is none and removes the one under the
    /// cursor; both report the list, and the add drags its end.
    #[test]
    fn ctrl_adds_and_removes_a_note() {
        let m = Metrics::default();
        let mut r = roll(r#"{"min":48,"max":72,"snap":100.0}"#);
        let mut ctrl = input(&m, rect(), axis(1000.0));
        ctrl.mods = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let at = (x_of(&r, &m, 250.0, 1000.0), y_of(&r, &m, 60.0));
        assert!(matches!(r.press(at, &ctrl), Claim::Take(_)));
        assert_eq!(r.notes.len(), 1);
        assert_eq!(r.notes[0].start, 300.0, "snapped to the note grid");
        assert!(matches!(
            r.drag,
            Some(Drag::Note {
                part: boxes::Part::End,
                ..
            })
        ));

        r.drag = None;
        // The note it just added spans 300..400 on the grid.
        let on_note = (x_of(&r, &m, 350.0, 1000.0), y_of(&r, &m, 60.0));
        assert!(matches!(r.press(on_note, &ctrl), Claim::Take(_)));
        assert!(r.notes.is_empty());
    }

    /// The block keys: `q` quantizes the selection, Delete removes it, and
    /// cut/paste travel through the host-wide clipboard in the same JSON a
    /// `/gui_set notes` takes.
    #[test]
    fn the_block_keys_quantize_delete_and_travel_through_the_clipboard() {
        let mut clipboard = crate::host::clipboard::Clip::default();
        let mut r = roll(r#"{"notes":[90.0,50.0,60.0,100,0,260.0,50.0,64.0,100,0],"snap":100.0}"#);
        r.selected = vec![0];
        fn ki(clip: &mut crate::host::clipboard::Clip, ctrl: bool) -> KeyInput<'_> {
            at_cursor(clip, ctrl, None)
        }
        fn at_cursor(
            clip: &mut crate::host::clipboard::Clip,
            ctrl: bool,
            cursor: Option<f64>,
        ) -> KeyInput<'_> {
            KeyInput {
                mods: Mods {
                    ctrl,
                    ..Mods::default()
                },
                clipboard: clip,
                cursor,
            }
        }
        assert!(
            r.key(&Key::Char('q'), &mut ki(&mut clipboard, false))
                .is_some()
        );
        assert_eq!(r.notes[0].start, 100.0);
        assert_eq!(r.notes[1].start, 260.0, "unselected, untouched");

        // Cut: the block lands on the clipboard and leaves the roll.
        r.selected = vec![0];
        assert!(
            r.key(&Key::Char('x'), &mut ki(&mut clipboard, true))
                .is_some()
        );
        assert_eq!(r.notes.len(), 1);
        let block = clipboard.text();
        assert!(block.starts_with('['), "{block}");

        // ...and pastes back at the window's cursor, keeping its pitch: the
        // block starts where the cursor is and not where it was copied from.
        assert!(
            r.key(
                &Key::Char('v'),
                &mut at_cursor(&mut clipboard, true, Some(400.0))
            )
            .is_some()
        );
        assert_eq!(r.notes.len(), 2);
        assert_eq!(r.notes[1].pitch, 60.0);
        assert_eq!(
            r.notes[1].start, 400.0,
            "at the cursor, snapped to the grid"
        );
        assert_eq!(r.selected, vec![1], "the pasted block is selected");
        // A roll on no axis keeps step entry's own position as the anchor,
        // which is where this one still stands.
        notes::remove_notes(&mut r.notes, &[1]);
        r.selected.clear();
        assert!(
            r.key(&Key::Char('v'), &mut ki(&mut clipboard, true))
                .is_some()
        );
        assert_eq!(r.notes[1].start, r.step, "no cursor: the step position");

        // Delete takes the selection away; a key it has no arm for falls
        // through to the front's own shortcuts.
        assert!(
            r.key(&Key::Delete, &mut ki(&mut clipboard, false))
                .is_some()
        );
        assert_eq!(r.notes.len(), 1);
        assert!(
            r.key(&Key::Char('z'), &mut ki(&mut clipboard, false))
                .is_none()
        );
        // Text on the clipboard is not a note block, so a paste declines it.
        let mut text = crate::host::clipboard::Clip::default();
        text.set_text("hola");
        assert!(r.key(&Key::Char('v'), &mut ki(&mut text, true)).is_none());
    }

    /// Live MIDI: a note-on paints a held note, the matching note-off closes it
    /// -- at the running playhead when recording, on the step cursor when the
    /// transport is stopped (and the last key up advances it).
    /// **A lane sits under the plane and a Ctrl press on it adds a point**,
    /// reported once as every curve's points.
    #[test]
    fn a_lane_takes_the_bottom_of_the_plane_and_a_ctrl_press_adds_a_point() {
        let m = Metrics::default();
        let flat = roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0]}"#);
        let mut r = roll(
            r#"{"notes":[0.0,50.0,60.0,100.0,0.0],
                "curves":["cc74","CC 74",0.0,127.0,40.0]}"#,
        );
        let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
        let whole = flat.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
        assert_eq!(
            grid.h,
            whole.h - 40.0,
            "the lane took its height off the plane"
        );
        let (_, body) = r.row_rects(rect(), pianoroll::KEYBOARD_W, &m)[0];
        let at = (
            (body.x + body.w * 0.5) as f64,
            (body.y + body.h * 0.5) as f64,
        );
        let mut held = input(&m, rect(), axis(100.0));
        held.mods = Mods {
            ctrl: true,
            ..Mods::default()
        };
        let Claim::Take(take) = r.press(at, &held) else {
            panic!("the lane takes the press");
        };
        let args = take.events.into_messages()[0].clone();
        assert_eq!(args[0], OscType::String("points".into()));
        assert_eq!(args[1], OscType::String("cc74".into()));
        assert_eq!(r.release(at, true, &held), Events::none(), "reported once");
    }

    /// **A bend is drawn in the plane**, over the pitches its range spans from
    /// its note; any other expression inside the note's box.
    #[test]
    fn a_bend_layer_spans_its_pitches_and_another_sits_in_the_box() {
        let m = Metrics::default();
        let r = roll(
            r#"{"notes":[0.0,50.0,60.0,100.0,0.0],"note_ids":[7],
                "layers":["b","7","bend",-2.0,2.0,true,"p","7","pressure",0.0,1.0,false]}"#,
        );
        let placed = r.curves_on_screen(rect(), pianoroll::KEYBOARD_W, &m, axis(100.0));
        let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
        let axis_y = r.axis(&m);
        let (_, bend, _) = placed.iter().find(|(n, ..)| *n == "b").unwrap();
        let (_, inside, _) = placed.iter().find(|(n, ..)| *n == "p").unwrap();
        assert!((bend.y - axis_y.y(62.0, grid)).abs() < 1e-3);
        assert!((bend.y + bend.h - axis_y.y(58.0, grid)).abs() < 1e-3);
        assert!(inside.h < bend.h, "the pressure is inside the box");
    }

    /// **A roll says which MIDI its notes are written for**, set and cleared
    /// like any prop -- empty for notes for the server.
    #[test]
    fn a_roll_carries_the_midi_spec_it_shows() {
        let mut r = roll(r#"{"midi":"MPE"}"#);
        assert_eq!(r.midi, "MPE");
        assert!(r.set("midi", &Value::from("MIDI 1.0")));
        assert_eq!(r.midi, "MIDI 1.0");
        assert!(r.set("midi", &Value::from("")));
        assert!(r.midi.is_empty());
        assert!(roll("{}").midi.is_empty());
    }

    /// **A note's curve runs past its off**, into the release: the layer
    /// reaches its last point while the note's box stays its on-to-off span.
    #[test]
    fn a_layer_reaches_past_its_note_and_the_box_does_not() {
        let m = Metrics::default();
        let r = roll(
            r#"{"notes":[0.0,50.0,60.0,100.0,0.0],"note_ids":[7],
                "layers":["p","7","pressure",0.0,1.0,false],
                "points":["p",0.0,0.5,1,0.0,"p",80.0,0.0,1,0.0]}"#,
        );
        let placed = r.curves_on_screen(rect(), pianoroll::KEYBOARD_W, &m, axis(100.0));
        let grid = r.regions(rect(), pianoroll::KEYBOARD_W, &m).grid;
        let nav = r.view(axis(100.0));
        let boxed = pianoroll::note_rect(grid, &nav, 0.0, &r.notes[0], r.axis(&m)).unwrap();
        let (_, layer, space) = placed.iter().find(|(n, ..)| *n == "p").unwrap();
        assert!((layer.x - boxed.x).abs() < 1e-3);
        assert!(
            (layer.w - boxed.w * 80.0 / 50.0).abs() < 1e-2,
            "the layer reaches its last point"
        );
        assert!(space.span >= 80.0);
        assert_eq!(r.notes[0].dur, 50.0, "the note is still its on and its off");
    }

    /// **An MPE note is painted at its bend, and follows it**: a zone note
    /// starts at its key plus its bend, and a retune moves the held note.
    #[test]
    fn an_mpe_note_is_painted_at_its_bend_and_follows_it() {
        let mut r = roll(r#"{"midi_in":true}"#);
        let on = MidiNote {
            on: true,
            channel: 3,
            pitch: 60,
            velocity: 90,
            bend: 0.5,
            retune: false,
        };
        r.midi(on, Some(0.0));
        assert_eq!(r.notes[0].pitch, 60.5);
        r.midi(
            MidiNote {
                bend: 2.0,
                retune: true,
                ..on
            },
            Some(10.0),
        );
        assert_eq!(r.notes[0].pitch, 62.0);
        assert_eq!(r.notes.len(), 1, "a retune starts no note");
    }

    #[test]
    fn live_midi_records_at_the_playhead_and_steps_when_stopped() {
        let mut r = roll(r#"{"midi_in":true,"snap":100.0}"#);
        assert!(r.needs().midi);
        assert!(!roll("{}").needs().midi);

        // Recording: the key is held from 200 to 350.
        let on = MidiNote {
            on: true,
            channel: 0,
            pitch: 60,
            velocity: 90,
            bend: 0.0,
            retune: false,
        };
        assert!(r.midi(on, Some(200.0)).is_some());
        assert_eq!(r.notes[0].start, 200.0);
        assert!(
            r.midi(
                MidiNote {
                    on: false,
                    velocity: 0,
                    ..on
                },
                Some(350.0)
            )
            .is_some()
        );
        assert_eq!(r.notes[0].dur, 150.0, "the key was held this long");

        // Stopped: a chord lands on the step cursor and advances it once.
        let mut r = roll(r#"{"midi_in":true,"snap":100.0}"#);
        for pitch in [60, 64] {
            r.midi(
                MidiNote {
                    on: true,
                    channel: 0,
                    pitch,
                    velocity: 90,
                    bend: 0.0,
                    retune: false,
                },
                None,
            );
        }
        assert_eq!((r.notes[0].start, r.notes[1].start), (0.0, 0.0));
        for pitch in [60, 64] {
            r.midi(
                MidiNote {
                    on: false,
                    channel: 0,
                    pitch,
                    velocity: 0,
                    bend: 0.0,
                    retune: false,
                },
                None,
            );
        }
        assert_eq!(r.step, 100.0, "one step for the whole chord");
        // A note-off nobody is holding is tolerated, not a panic.
        assert!(
            r.midi(
                MidiNote {
                    on: false,
                    channel: 9,
                    pitch: 1,
                    velocity: 0,
                    bend: 0.0,
                    retune: false,
                },
                None
            )
            .is_none()
        );
    }

    /// **The markers lane shows and does not write.**
    ///
    /// A roll is the editor of things that have a pitch; the other items a
    /// timeline holds have none and are drawn below it as markers. The lane
    /// had grown a `Ctrl`-press that added and removed them, against the
    /// decision this widget was built with, and it could not have worked: a
    /// marker *is* the message it sends, the lane draws only its address, and
    /// there is no way to type one here, so an added marker was a message with
    /// no destination. Both clients saw the same press and answered
    /// differently -- one refusing it with a sentence, the other keeping the
    /// addressless marker -- which is how it was found.
    ///
    /// Nothing tested this lane's editing at all, which is why the gesture
    /// could contradict a recorded decision and stay.
    #[test]
    fn the_markers_lane_refuses_the_press_that_meant_to_edit_it() {
        let m = Metrics::default();
        let mut r = roll(r#"{"notes":[0.0,100.0,60.0,100,0],"osc":[50.0,"/bar"],"osc_lane":1}"#);
        assert_eq!(r.osc.len(), 1, "the marker it was given");
        let mut i = input(&m, rect(), axis(1000.0));
        let lane = r.regions(rect(), pianoroll::KEYBOARD_W, &m).osc;
        let on = (x_of(&r, &m, 50.0, 1000.0), (lane.y + lane.h * 0.5) as f64);
        let empty = (x_of(&r, &m, 700.0, 1000.0), (lane.y + lane.h * 0.5) as f64);

        // The gesture that meant to edit is told, and consumed so nothing
        // behind it turns a refused edit into a sweep.
        i.mods.ctrl = true;
        for (what, at) in [("on a marker", on), ("on empty lane", empty)] {
            let Claim::Take(take) = r.press(at, &i) else {
                panic!("{what}: a refusal consumes the press");
            };
            let msgs = take.events.into_messages();
            assert_eq!(msgs[0][0], OscType::String("refused".into()), "{what}");
            assert_eq!(msgs[0][1], OscType::String("osc".into()), "{what}");
        }
        assert_eq!(r.osc.len(), 1, "nothing was added and nothing removed");

        // And a plain press hands the lane back to the container, the way the
        // axis strip beside it does -- so a sweep across the roll still works.
        i.mods.ctrl = false;
        assert_eq!(r.press(on, &i), Claim::Decline);
        assert!(r.drag.is_none(), "and no marker is being slid");
    }

    /// The reason a verb gave for refusing, or `None` when it did the thing --
    /// the same reader the multitrack's own verb test uses, because a refusal
    /// is an ordinary event in both.
    fn refusal(events: Option<Events>) -> Option<String> {
        let msg = events?.into_messages().into_iter().next()?;
        (msg.first() == Some(&OscType::String("refused".into()))).then(|| match msg.get(2) {
            Some(OscType::String(s)) => s.clone(),
            _ => String::new(),
        })
    }

    /// **A roll's verbs say why they did nothing**, exactly as a lane's do.
    ///
    /// The drift this holds: the two implement one table of letters over one
    /// reading, and a multitrack that could not quantize said so while a roll
    /// that could not returned silence -- which is the thing two reports in one
    /// day settled as a defect rather than as a quiet success.
    #[test]
    fn a_verb_that_acts_on_nothing_says_why_rather_than_nothing() {
        let mut clipboard = crate::host::clipboard::Clip::default();
        let mut press = |roll: &mut Notes, k: char| {
            refusal(roll.key(
                &Key::Char(k),
                &mut KeyInput {
                    mods: Mods::default(),
                    clipboard: &mut clipboard,
                    cursor: Some(100.0),
                },
            ))
        };

        // Two notes on one pitch, already on the grid, and the cursor between
        // them: every verb has something to act on and nothing to do.
        let mut r =
            roll(r#"{"notes":[0.0,50.0,60.0,100.0,0.0, 200.0,50.0,60.0,100.0,0.0],"snap":100.0}"#);
        r.selected = vec![0, 1];
        assert_eq!(
            press(&mut r, 'q'),
            Some("these notes are already on the grid".to_string())
        );
        assert_eq!(
            press(&mut r, 'e'),
            Some("the cursor is not inside a held note".to_string()),
            "the cursor falls in the gap between them"
        );
        assert_eq!(
            press(&mut r, 'j'),
            Some("a join is one pitch's, and these notes do not touch on one".to_string())
        );
    }

    /// **The cut is the box arithmetic's, and the identity is the roll's.**
    ///
    /// A note's second half keeps the pitch, the velocity and the channel of
    /// the one it came from, which is what `Holder::duplicate` answers for here;
    /// everything else about the cut is what a clip's `e` does, through the
    /// same function.
    #[test]
    fn a_cut_note_leaves_two_halves_that_are_still_the_same_note() {
        let mut r = roll(r#"{"notes":[0.0,200.0,64.0,90,3]}"#);
        r.selected = vec![0];
        let cut = notes::split_notes(&mut r.notes, &r.selected, 50.0);
        assert_eq!(cut, vec![0, 1], "the head it was, and the tail it made");
        assert_eq!((r.notes[0].start, r.notes[0].dur), (0.0, 50.0));
        assert_eq!((r.notes[1].start, r.notes[1].dur), (50.0, 150.0));
        assert_eq!(
            (r.notes[1].pitch, r.notes[1].velocity, r.notes[1].channel),
            (64.0, 90, 3),
            "the tail is the same note, cut"
        );
    }
}
