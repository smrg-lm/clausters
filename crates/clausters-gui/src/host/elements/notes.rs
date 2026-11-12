//! `notes` -- the editor-grade piano roll: a keyboard gutter, a note grid and its
//! OSC markers, placed on a navigation group's shared time axis. A note's velocity
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
use crate::host::ruler;
use crate::host::structures::boxes::{self, Bounds, Placements};
use crate::host::structures::notes::OscMarker;
use crate::host::structures::notes::{self, Note};
use crate::host::widget::element::{
    BodyRole, Claim, Ctx, Element, Events, Input, KeyInput, MidiNote, Needs, OnAxis, Swept, Take,
    TimeSpace, Verb,
};
use crate::host::widget::parse::{self, label, number, number_f64, set_label, truthy};
use crate::host::widget::{EditorProps, GestureMap, Ruler};
use crate::viewport::View;
use clausters_gui_draw::draw::Draw;
use clausters_gui_draw::font;
use clausters_gui_draw::metrics::Metrics;
use clausters_gui_draw::rect::Rect;

/// The default pitch compass of a roll: the range of an 88-key piano.
const PITCH_MIN: f32 = 21.0;
const PITCH_MAX: f32 = 108.0;

/// A note's height on a roll in hertz, in logical pixels.
const HZ_NOTE_H: f32 = 8.0;

/// The shortest note a resize may leave, in axis units.
const MIN_DUR: f64 = 1.0;

/// A piano roll. `drag`, `held` and `step` are native view state -- the
/// gestures and the MIDI leg build them and no `/gui_set` writes them.
/// `selected` is view state too, built by the hand, and the one a script may
/// read (`/gui_query`) and write (`/gui_set selected`): the notes it names are
/// the objects a script holds, so marking them is an edit of the picture only.
#[derive(Debug, Clone)]
pub struct Notes {
    notes: Vec<notes::Note>,
    osc: Vec<notes::OscMarker>,
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
    osc_markers: bool,
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
    /// The rows under the plane: the sequence's curves.
    rows: Vec<curves::Row>,
    /// **The lane shown, where the owner shows one at a time** (`lane`): a
    /// row's name, `velocity` for the notes' velocities, or `""` for none.
    /// Without it every row is drawn, one under another.
    lane: Option<String>,
    /// **The lanes the owner offers as tabs** (`lane_tabs`, flat `name
    /// label` pairs): drawn as a row of tabs right over the lane, a press on
    /// one asking the owner to show it. Empty for none.
    lane_tabs: Vec<(String, String)>,
    /// **What a plain press on the grid does** (`tool`).
    tool: Tool,
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
    /// **Which note goes on into which** (`joins`, index pairs) -- and,
    /// where the owner sent it at all, the sign that the roll's owner speaks
    /// the joins of a line, so `join` asks the owner to join the
    /// selection: a join is its edit.
    joins: Option<Vec<(usize, usize)>>,
    /// **The pitch each line sounds over time** (`lines`): a run of `[at,
    /// value]` points each, in view units and the domain's values, drawn over
    /// the boxes -- what joins a line's boxes and follows its bends.
    lines: Vec<Vec<[f64; 2]>>,
    /// Whether the owner follows the selection: the marks are told
    /// ([`Element::take_marks`]) whenever the hand changes them.
    tell_marks: bool,
    /// The marks last told, or set by the owner.
    told: Option<Vec<u64>>,
    /// **What a roll in notation mode draws beside its notes**, index by
    /// index: how long each sounds (`held`, in the axis' units -- a note is
    /// drawn as long as it is written, with this inside it), its `voices`
    /// (`staff voice` pairs, which colour it), whether it is `dimmed` (a voice
    /// other than the one edited, drawn faint and not taken by a press) and
    /// the dynamic it states (`labels`). Empty in MIDI mode.
    written: Written,
}

/// The lists a roll in notation mode draws beside its notes.
#[derive(Debug, Clone, Default)]
struct Written {
    held: Vec<f64>,
    voices: Vec<usize>,
    dimmed: Vec<bool>,
    labels: Vec<String>,
    /// How each note is written, as a reader says it (`spelled`).
    spelled: Vec<String>,
    /// **The marks a page writes on each note** (`marks`): its articulations,
    /// drawn over the box -- a symbol's character, or the text it reads as.
    marks: Vec<String>,
    /// The breath after each note (`breaths`), drawn at its end, or `""`.
    breaths: Vec<String>,
    /// **Each note's tint** (`tints`): its voice, staff or part as the owner
    /// says, the first untinted. Without it a note is tinted by its voice.
    tints: Vec<usize>,
    /// **The section's slurs** (`slurs`): `[from, to, joined]` by the notes'
    /// indices, drawn as `slur_style` says.
    slurs: Vec<[usize; 3]>,
    /// How a slur is drawn (`slur_style`).
    slur_style: crate::host::graphics::slurs::Style,
    /// **How a note's marks are laid out** (`mark_layout`): stacked at its
    /// start, or in a row along it.
    stacked: bool,
    /// Whether they are drawn inside the box rather than above it
    /// (`mark_place`).
    inside: bool,
}

impl Written {
    fn read(props: &Map<String, Value>) -> Self {
        let mut out = Self::default();
        for key in [
            "held",
            "voices",
            "dimmed",
            "labels",
            "spelled",
            "marks",
            "breaths",
            "tints",
            "slurs",
            "slur_style",
            "mark_layout",
            "mark_place",
        ] {
            if let Some(v) = props.get(key) {
                out.set(key, v);
            }
        }
        out
    }

    /// One of the four lists, set; `false` for a key that is none of them.
    fn set(&mut self, key: &str, v: &Value) -> bool {
        // how the marks and the slurs are drawn, each a word
        match key {
            "slur_style" => {
                self.slur_style =
                    crate::host::graphics::slurs::Style::named(v.as_str().unwrap_or(""));
                return true;
            }
            "mark_layout" => {
                self.stacked = v.as_str() == Some("stacked");
                return true;
            }
            "mark_place" => {
                self.inside = v.as_str() == Some("inside");
                return true;
            }
            _ => {}
        }
        let list = parse::as_array_props(key, v);
        let list = list
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let numbers = || list.iter().filter_map(Value::as_f64);
        match key {
            "held" => self.held = numbers().collect(),
            "voices" => self.voices = numbers().map(|n| n.max(0.0) as usize).collect(),
            "dimmed" => self.dimmed = numbers().map(|n| n != 0.0).collect(),
            "tints" => self.tints = numbers().map(|n| n.max(0.0) as usize).collect(),
            "slurs" => {
                let n: Vec<usize> = numbers().map(|n| n.max(0.0) as usize).collect();
                self.slurs = n.as_chunks::<3>().0.to_vec();
            }
            "labels" | "spelled" | "marks" | "breaths" => {
                let texts = list
                    .iter()
                    .map(|l| l.as_str().unwrap_or_default().to_string())
                    .collect();
                match key {
                    "labels" => self.labels = texts,
                    "spelled" => self.spelled = texts,
                    "marks" => self.marks = texts,
                    _ => self.breaths = texts,
                }
            }
            _ => return false,
        }
        true
    }

    /// Whether note `i` is of a voice other than the one edited.
    fn dimmed(&self, i: usize) -> bool {
        self.dimmed.get(i).copied().unwrap_or(false)
    }

    /// The `[staff, voice]` of note `i`, in notation mode.
    fn voice(&self, i: usize) -> Option<[usize; 2]> {
        Some([*self.voices.get(2 * i)?, *self.voices.get(2 * i + 1)?])
    }
}

/// **A voice's colour**, by its number in its staff: the first is the roll's
/// own, the others tinted over it, so two voices on one staff read apart.
const VOICE_TINTS: [[f32; 4]; 3] = [
    [0.95, 0.55, 0.15, 0.55],
    [0.25, 0.75, 0.35, 0.55],
    [0.65, 0.40, 0.90, 0.55],
];

mod curves;
mod picture;
mod props;
use props::*;

/// What a held press on a roll is doing. Each carries the **press-time data**
/// it is measured from and no geometry: the rectangle and the axis arrive with
/// every step, because both may move under the drag.
#[derive(Debug, Clone)]
enum Drag {
    /// **The selection stretched in time**: every note's onset and length
    /// scaled from the selection's first onset by how far the press moved its
    /// end -- `orig` is each note's `(index, start, dur)` at the press.
    Stretch {
        press_time: f64,
        anchor: f64,
        end: f64,
        orig: Vec<(usize, f64, f64)>,
    },
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
    /// The selection resized by one selected note's edge: `orig` is each
    /// note's placement at the press, the grabbed one first.
    Resize {
        part: boxes::Part,
        orig: Vec<(usize, boxes::Placement)>,
    },
    /// **An erase**: Ctrl held from a press on a note, every note the
    /// pointer crosses taken away, one edit at the release.
    Erase,
    /// Velocities nudged by the vertical distance from the press, one step a
    /// pixel: the grabbed note's, or every selected one's when it is selected.
    Level {
        press_y: f64,
        orig: Vec<(usize, i32)>,
        /// The note pressed: Shift and a click that moves nothing toggles it
        /// in the selection.
        grabbed: usize,
    },
    /// **A velocity bar in its lane**: the grabbed note's, or every selected
    /// one's when it is selected, moved by the height the pointer travels --
    /// the lane's height is the whole range.
    Bars {
        press_y: f64,
        orig: Vec<(usize, i32)>,
    },
    /// **The pencil over the velocity lane**: drawn freehand, every note
    /// shown whose bar the pointer crosses between where it last was and
    /// where it is takes the height the pointer is at there; the rest keep
    /// theirs.
    Paint { last: (f64, f64) },
    /// **A ramp across the velocity lane**: from the press, at the velocity
    /// it was at, to the pointer; every note shown that starts between the
    /// two takes the line's velocity, and the rest keep theirs.
    Ramp {
        from: (f64, f64),
        orig: Vec<(usize, i32)>,
    },
}

/// **What a plain press on the grid does**: the roll's editing tool, the
/// window's choice (`tool`). Every tool keeps the modifiers' gestures --
/// Ctrl adds or erases, Alt toggles a note in the selection, Shift and a
/// vertical drag is the velocity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tool {
    /// The pointer: a click selects a note and a drag moves it or, by an
    /// edge, resizes it; a drag over empty grid sweeps a selection, and a
    /// double click there makes a note.
    #[default]
    Select,
    /// The pencil: a press on empty grid makes a note, the drag its length;
    /// a note is taken as the pointer takes it.
    Draw,
    /// The eraser: every note pressed or crossed is taken away.
    Erase,
    /// The scissors: a note clicked is cut where it was clicked, the
    /// selection with it when it is selected.
    Split,
    /// The selection stretched in time by a drag from one of its notes.
    Stretch,
}

impl Tool {
    /// The tool a `tool` prop names, the pointer for any other word.
    fn named(name: &str) -> Tool {
        match name {
            "draw" => Tool::Draw,
            "erase" => Tool::Erase,
            "split" => Tool::Split,
            "stretch" => Tool::Stretch,
            _ => Tool::Select,
        }
    }
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
    // the outlines its marks are drawn with, into the host's one table
    if let Some(glyphs) = props.get("glyphs").and_then(Value::as_object) {
        clausters_gui_draw::font::outline::set(glyphs);
    }
    let osc = parse_osc(props);
    let mut notes = parse_notes(props);
    let ids = set_ids(&mut notes, props.get("note_ids"));
    let mut roll = Notes {
        notes,
        ids,
        id_list: None,
        // The OSC markers' strip shows when there are markers or it is
        // explicitly asked for (so an empty one can still be opened).
        osc_markers: props
            .get("osc_markers")
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
        lane: props
            .get("lane")
            .and_then(Value::as_str)
            .map(str::to_string),
        lane_tabs: props
            .get("lane_tabs")
            .map(curves::parse_lane_tabs)
            .unwrap_or_default(),
        tool: Tool::named(
            props
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        layers: curves::parse_layers(props),
        curve_points: curves::parse_points(props),
        bodies: std::collections::HashMap::new(),
        layer: None,
        holding: None,
        joins: props.get("joins").map(parse_joins),
        lines: props.get("lines").map(parse_lines).unwrap_or_default(),
        tell_marks: props.get("tell_marks").and_then(truthy).unwrap_or(false),
        told: None,
        written: Written::read(props),
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
        let mut r = pianoroll::regions(rect, self.editor.strip(), self.osc_markers, indent, m);
        // the lane's tabs sit right over it
        let rows = self.rows_h(r.grid.h, m) + self.tabs_h(m);
        r.grid.h = (r.grid.h - rows).max(0.0);
        r.keyboard.h = (r.keyboard.h - rows).max(0.0);
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
            self.editor.strip(),
            self.osc_markers,
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
        // **Inside its cell, whatever the cell's width**: the keyboard's
        // column is narrower than the words, and the ruler's first label
        // starts where it ends -- the caption's size where it fits, the
        // smallest text where that does, and cut short past that.
        let room = cell.w - 2.0 * m.pad;
        let scale = [m.caption_scale, m.micro_scale]
            .into_iter()
            .find(|scale| font::width(&self.midi, *scale) <= room)
            .unwrap_or(m.micro_scale);
        if room <= 0.0 || cell.h < font::height(scale) {
            return;
        }
        font::text_ellipsis(
            mesh,
            &self.midi,
            cell.x + m.pad,
            cell.y + (cell.h - font::height(scale)) * 0.5,
            room,
            scale,
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
            // times as doubles: a float names a frame only up to 2^24
            args.push(OscType::Double(n.start));
            args.push(OscType::Double(n.dur));
            args.push(OscType::Float(self.value_of(n.pitch)));
            args.push(OscType::Int(n.velocity));
            args.push(OscType::Int(n.channel));
        }
        Events::message(args)
    }

    /// **The selected notes cut at `at`**, by the cursor's key or the
    /// scissors. An owner that holds events cuts them, keys and curves with
    /// them: the roll names the cut and the notes, and draws what comes
    /// back. Only a bare roll cuts what it draws.
    fn split_at(&mut self, at: f64) -> Events {
        if self.joins.is_some() && self.ids {
            let inside = self
                .selected
                .iter()
                .filter_map(|i| self.notes.get(*i))
                .any(|n| at > n.start + 1e-9 && at < n.start + n.dur - 1e-9);
            if !inside {
                return Events::refused("split", "the cursor is not inside a held note");
            }
            return self.selection_event_at("split", at);
        }
        let cut = notes::split_notes(&mut self.notes, &self.selected, at);
        if cut.is_empty() {
            return Events::refused("split", "the cursor is not inside a held note");
        }
        self.selected = cut;
        self.notes_event()
    }

    /// **`tag` with the ids of the selected notes**: what an owner's edit over
    /// the selection is asked with.
    fn selection_event(&self, tag: &str) -> Events {
        self.selection_ids(vec![OscType::String(tag.into())])
    }

    /// [`selection_event`](Self::selection_event) with a position first: the
    /// `split`'s cut, in the view's units.
    fn selection_event_at(&self, tag: &str, at: f64) -> Events {
        self.selection_ids(vec![OscType::String(tag.into()), OscType::Double(at)])
    }

    /// The ids of the selected notes, low to high.
    fn marked_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self
            .selected
            .iter()
            .filter_map(|i| self.notes.get(*i))
            .map(|n| n.id)
            .collect();
        ids.sort_unstable();
        ids
    }

    fn selection_ids(&self, mut args: Vec<OscType>) -> Events {
        let mut ids: Vec<u64> = self
            .selected
            .iter()
            .filter_map(|i| self.notes.get(*i))
            .map(|n| n.id)
            .collect();
        ids.sort_unstable();
        args.extend(ids.into_iter().map(|id| OscType::Double(id as f64)));
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
        if self.osc_markers && r.osc.contains(at.0, at.1) {
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
                .flatten()
                // a note of a voice other than the one edited is not taken
                .filter(|hit| !self.written.dimmed(hit.index)),
        }
    }

    /// Inserts a note at `start`/`pitch` for the live-MIDI leg and the Ctrl+add
    /// gesture, returning its index.
    fn insert(&mut self, note: notes::Note) -> usize {
        notes::insert_note(&mut self.notes, note)
    }

    /// The length a note is painted with when nothing said otherwise: the note
    /// grid, else a visible sliver of the window.
    fn default_dur(&self, nav: &View, at: f64) -> f64 {
        let grid = self.grid();
        if grid.is_set() {
            grid.step_at(at)
        } else {
            (nav.len * 0.05).max(MIN_DUR)
        }
    }

    /// **The grid a note lands on**: the axis' `grid` in beats through its
    /// tempo map, else the roll's own `snap` in units.
    fn grid(&self) -> boxes::Grid {
        self.editor.placement_grid(self.rate(0.0), self.snap)
    }

    /// The grid a quantize moves notes onto: the axis' `grid` whether or not
    /// a drag snaps to it.
    fn quantize_grid(&self) -> boxes::Grid {
        self.editor.quantize_grid(self.rate(0.0), self.snap)
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

/// **Where a note's marks are drawn**, each its `x`, `y` and scale: one by
/// one stacked at the box's start or in a row along it, just above the box
/// or inside it. `sized` is the scale a mark is drawn at above the box; one
/// drawn inside is fitted to the box's height.
fn mark_places(
    r: Rect,
    marks: &[&str],
    sized: &dyn Fn(&str) -> f32,
    stacked: bool,
    inside: bool,
) -> Vec<(f32, f32, f32)> {
    let n = marks.len().max(1) as f32;
    marks
        .iter()
        .enumerate()
        .map(|(k, mark)| {
            let mut scale = sized(mark);
            if inside {
                let advance = font::line_advance(scale).max(1.0);
                let room = if stacked { r.h / n } else { r.h };
                scale *= (room / advance).min(1.0);
            }
            let (w, advance) = (font::width(mark, scale), font::line_advance(scale));
            let k = k as f32;
            let x = if stacked {
                r.x + 1.0
            } else {
                // a row along the note: each mark centred in its share of it
                r.x + (k + 0.5) * r.w / n - w * 0.5
            };
            let y = match (stacked, inside) {
                (true, false) => r.y - (k + 1.0) * advance,
                (true, true) => r.y + k * advance,
                (false, false) => r.y - advance,
                (false, true) => r.y + (r.h - advance).max(0.0) * 0.5,
            };
            (x, y, scale)
        })
        .collect()
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
            "held" | "voices" | "dimmed" | "labels" | "spelled" | "marks" | "breaths" | "tints"
            | "slurs" | "slur_style" | "mark_layout" | "mark_place" => self.written.set(key, v),
            // **the outlines its marks are drawn with**, as a window's
            // `glyphs`: the host's one table, which every text reads
            "glyphs" => v
                .as_object()
                .map(clausters_gui_draw::font::outline::set)
                .is_some(),
            "joins" => {
                let list = parse::as_array_props("joins", v);
                self.joins = Some(parse_joins(list.get("joins").unwrap_or(&Value::Null)));
                true
            }
            "lines" => {
                self.lines = parse_lines(v);
                true
            }
            "lane" => {
                self.lane = v.as_str().map(str::to_string);
                true
            }
            "lane_tabs" => {
                self.lane_tabs = curves::parse_lane_tabs(v);
                true
            }
            "tool" => {
                self.tool = Tool::named(v.as_str().unwrap_or_default());
                true
            }
            "tell_marks" => {
                self.tell_marks = truthy(v).unwrap_or(false);
                true
            }
            "selected" => {
                // The notes to mark, by the ids they carry -- or by index, on
                // a roll that was handed none.
                let named: Vec<u64> = parse::as_array_props("selected", v)
                    .get("selected")
                    .and_then(Value::as_array)
                    .map(|list| list.iter().filter_map(Value::as_u64).collect())
                    .unwrap_or_default();
                self.selected = (0..self.notes.len())
                    .filter(|&i| {
                        let key = if self.ids { self.notes[i].id } else { i as u64 };
                        named.contains(&key)
                    })
                    .collect();
                // the owner's own marks: nothing to tell it back
                self.told = Some(self.marked_ids());
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
            "osc_markers" => truthy(v).map(|b| self.osc_markers = b).is_some(),
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
        self.draw_bars(d, r.grid, &nav, self.rate(ctx.world.sample_rate));
        self.draw_slurs(d, r.grid, &nav, axis, true);
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
        self.draw_written(d, r.grid, &nav, axis);
        self.draw_veils(d, r.grid, &nav, axis);
        self.draw_lines(d, r.grid, &nav, axis);
        self.draw_slurs(d, r.grid, &nav, axis, false);
        if self.osc_markers {
            pianoroll::draw_osc_markers(d, r.osc, &nav, 0.0, &self.osc);
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
        if self.editor.strip() {
            // The strip sits under the grid, aligned to the grid's x range --
            // the "body" the tick math derives it from.
            let body = Rect::new(r.grid.x, ctx.rect.y, r.grid.w, r.ruler.y - ctx.rect.y);
            crate::host::frame::draw_time_ruler(d, ctx.rect, body, &nav, rate, &self.editor);
        }
        self.draw_axis_chrome(d, ctx, r.grid, &nav);
    }

    /// **The playhead**: the one line of a roll a clock moves, down the grid
    /// at where the music is.
    fn draw_live(&self, d: &mut Draw, ctx: &Ctx) {
        let Some(pos) = ctx.time.and_then(|time| time.head) else {
            return;
        };
        let grid = self.regions(ctx.rect, ctx.indent, ctx.metrics).grid;
        let nav = self.view(ctx.time);
        if pos < nav.start || pos > nav.start + nav.len {
            return;
        }
        let x = (grid.x as f64 + (pos - nav.start) / nav.len.max(1.0) * grid.w as f64) as f32;
        let (mesh, m, theme) = d.parts();
        mesh.rect(Rect::new(x, grid.y, m.trace_w, grid.h), theme.playhead);
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
            (
                "selected".into(),
                Value::from(
                    Value::from(
                        self.selected
                            .iter()
                            .filter_map(|&i| self.notes.get(i))
                            .zip(self.selected.iter())
                            .map(|(n, &i)| if self.ids { n.id } else { i as u64 })
                            .collect::<Vec<_>>(),
                    )
                    .to_string(),
                ),
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
            // The playhead is the whole of what a clock moves here, and it is
            // on the live layer.
            live: true,
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
    ///
    /// **The notes are the contents and the clip is a window onto them**, as
    /// a take is: a note's start is a frame of the sequence, and the clip's
    /// window says which frame its zero reads and how many frames one unit of
    /// its time is. So a trim from the front moves the window and leaves every
    /// note where it was played -- in the picture the moment the edge moves,
    /// before any owner answers -- and a note that starts before the window
    /// is not the clip's, as it is not heard from it.
    fn draw_body(&self, d: &mut Draw, rect: Rect, time: &TimeSpace) {
        let (lo, hi) = (self.min, self.max);
        let window = time.window;
        let rate = if window.rate > 0.0 { window.rate } else { 1.0 };
        // the visible stretch of the clip, in the sequence's frames
        let seen = View {
            start: window.start + time.view.start * rate,
            len: time.view.len * rate,
        };
        let held: Vec<Note> = self
            .notes
            .iter()
            .filter(|note| note.start >= window.start)
            .cloned()
            .collect();
        pianoroll::draw_notes(
            d,
            rect,
            rect,
            &seen,
            0.0,
            &held,
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
        self.selected.retain(|i| !self.written.dimmed(*i));
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
        if let Some(claim) = self.press_lane_tabs(at, input) {
            return claim;
        }
        // A curve's own contents first: it is drawn over what it shapes.
        if let Some(claim) = self.press_curve(at, input) {
            return claim;
        }
        if let Some(claim) = self.press_velocity(at, input) {
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
                    Some((_, s0, _)) => self.grid().snap(s0 + (time - press_time)) - s0,
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
            Some(Drag::Resize { part, orig }) => {
                let bounds = self.edit_bounds(input);
                boxes::resize_block(&mut self.notes, &orig, part, time, bounds);
                Events::none()
            }
            Some(Drag::Erase) => {
                if let Some(nh) = self.hit(at, input).note {
                    notes::remove_note(&mut self.notes, nh.index);
                    self.selected = notes::selection_after_removal(&self.selected, nh.index);
                }
                Events::none()
            }
            Some(Drag::Stretch {
                press_time,
                anchor,
                end,
                orig,
            }) => {
                let span = end - anchor;
                if span > 0.0 {
                    let to = self.grid().snap(end + (time - press_time));
                    let f = ((to - anchor) / span).max(1e-3);
                    for (i, start, dur) in orig {
                        if let Some(n) = self.notes.get_mut(i) {
                            n.start = anchor + (start - anchor) * f;
                            n.dur = (dur * f).max(MIN_DUR);
                        }
                    }
                }
                Events::none()
            }
            Some(Drag::Level { press_y, orig, .. }) => {
                let dv = ((press_y - at.1) / f64::from(input.scale.max(0.1))).round() as i32;
                notes::nudge_velocities_from(&mut self.notes, &orig, dv);
                Events::none()
            }
            Some(Drag::Bars { press_y, orig }) => {
                self.drag_bars(press_y, &orig, at, input);
                Events::none()
            }
            Some(Drag::Ramp { from, orig }) => {
                self.drag_ramp(from, &orig, at, input);
                Events::none()
            }
            Some(Drag::Paint { last }) => {
                self.paint_velocities(last, at, input);
                self.drag = Some(Drag::Paint { last: at });
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
            // **Shift and a click that moved no velocity** adds the note to
            // the selection, or takes it out
            Some(Drag::Level { orig, grabbed, .. })
                if orig
                    .iter()
                    .all(|(i, v)| self.notes.get(*i).is_some_and(|n| n.velocity == *v)) =>
            {
                notes::toggle_selected(&mut self.selected, grabbed);
                Events::none()
            }
            Some(_) => self.notes_event(),
        }
    }

    /// The block operations, addressed to whatever the pointer is over:
    /// quantize, split and join, delete the selection, and copy, cut and paste
    /// a block through the host-wide clipboard. Which keys ask for them is the
    /// host's key table (by default `q`, `e`, `j`, Delete and Ctrl+C/X/V).
    ///
    /// They are verbs rather than gestures because they act on the
    /// *selection*, which is already where the pointer has been. A verb this
    /// element has no arm for goes on to the window.
    fn verb(&mut self, verb: Verb, input: &mut KeyInput) -> Option<Events> {
        match verb {
            // **Every note, marked**: the picture's, so nothing is reported.
            Verb::SelectAll => {
                self.selected = (0..self.notes.len()).collect();
                Some(Events::none())
            }
            // Quantize the selected onsets (all of them when nothing is
            // selected) to the note grid -- the same grid a drag snaps to.
            Verb::Quantize => {
                let grid = self.quantize_grid();
                Some(if !grid.is_set() {
                    Events::refused("quantize", "this roll has no grid to quantize to")
                } else if notes::quantize_notes(&mut self.notes, &self.selected, &grid) {
                    self.notes_event()
                } else {
                    Events::refused("quantize", "these notes are already on the grid")
                })
            }
            // **Split and join**, the clip's own two verbs over notes -- same
            // verbs, same reading. A clip asks its owner to cut, because the
            // owner holds the element; a roll holds its notes and cuts them
            // itself, which is the whole of the difference.
            //
            // The cut falls on the **window's cursor**, which is where a paste
            // lands too: a roll's key gestures have no pointer to read, and the
            // window has one cursor for exactly this. Step entry's own position
            // stands in where the roll is on no axis (a bare roll nothing has
            // located yet).
            // **Only over a selection.** A roll drawn as a *clip's body* shares
            // these two verbs with the clip they belong to, and the clip is
            // what a lane's hand is on: with nothing selected the verb falls
            // through and cuts the clip, which is what a split has always meant
            // there. Selecting notes first is how you say you meant the notes.
            // It is also the sane reading on its own -- splitting every note in
            // the roll is not something anyone asks for by leaning on a key.
            Verb::Split if !self.selected.is_empty() => {
                let at = self.grid().snap(self.anchor(input)).max(0.0);
                Some(self.split_at(at))
            }
            Verb::Join if !self.selected.is_empty() => {
                // **Where the owner speaks joins, a join is its edit**: notes
                // of one pitch that touch become one there with their curves,
                // and any other pair goes on into the next without a new
                // attack -- what the boxes here do not hold.
                if self.joins.is_some() && self.ids {
                    return Some(self.selection_event("join"));
                }
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
            Verb::Delete if !self.selected.is_empty() => {
                let held = std::mem::take(&mut self.selected);
                boxes::discard(&mut self.notes, &held).then(|| self.notes_event())
            }
            // The clipboard is the host's one string, so a block travels
            // between rolls and windows -- and rides it in the same JSON form a
            // `/gui_set notes` accepts, which is the carrier every non-scalar
            // already uses.
            Verb::Copy | Verb::Cut => {
                let block = notes::copy_notes(&self.notes, &self.selected);
                if block.is_empty() {
                    return None;
                }
                input
                    .clipboard
                    .set_text(&notes::notes_json(&block).to_string());
                let cut = verb == Verb::Cut;
                if !cut {
                    // A copy changed nothing, so it reports nothing -- but it
                    // consumed the key.
                    return Some(Events::none());
                }
                notes::remove_notes(&mut self.notes, &self.selected);
                self.selected.clear();
                Some(self.notes_event())
            }
            // A paste that adds is a paste here: pasted notes are laid over
            // what is there either way.
            Verb::Paste | Verb::Mix => {
                let block = clipboard_notes(&input.clipboard.text())?;
                // **At the cursor**: what is pasted starts where the window's
                // cursor is, playing or not -- a paste has no pointer, and the
                // cursor is the one position the window keeps.
                let at = self.grid().snap(self.anchor(input)).max(0.0);
                self.selected = notes::paste_notes(&mut self.notes, &block, at);
                Some(self.notes_event())
            }
            _ => None,
        }
    }

    /// A live note: painted at the running playhead (recording), or on the step
    /// cursor when the transport is stopped (step entry -- a chord shares one
    /// step, and the last key up advances it).
    /// **The roll's own commands over its selection**, for an owner that
    /// speaks the joins of a line: `unjoin`, the marks a page writes over
    /// notes (`slur`, `slur_join`, `tie`, `staccato`, `pedal`) and the
    /// transpositions (`step_up`, `step_down`, `octave_up`, `octave_down`,
    /// which with nothing selected are of every note) -- the selected notes'
    /// ids to the owner, whose edit it is.
    fn reading(&self, ctx: &Ctx) -> Option<String> {
        self.reading_at(ctx)
    }

    fn take_marks(&mut self) -> Option<Vec<OscType>> {
        if !self.tell_marks || !self.ids {
            return None;
        }
        let ids = self.marked_ids();
        if self.told.as_ref() == Some(&ids) {
            return None;
        }
        self.told = Some(ids.clone());
        let mut args = vec![OscType::String("marked".into())];
        args.extend(ids.into_iter().map(|id| OscType::Double(id as f64)));
        Some(args)
    }

    fn command(&mut self, name: &str, _input: &mut KeyInput) -> Option<Events> {
        let moves = matches!(name, "step_up" | "step_down" | "octave_up" | "octave_down");
        let curves = clausters_editing::notes::lanes::CURVE_VERBS.contains(&name);
        let ours = moves
            || curves
            || clausters_editing::notes::notation::ARTICULATIONS.contains(&name)
            || matches!(
                name,
                "unjoin" | "select_pitch" | "select_channel" | "select_voice" | "select_staff"
            );
        if !ours || self.joins.is_none() || !self.ids || !self.editable {
            return None;
        }
        // **the curve shown is the one a removal names**
        if name == "curve_remove" {
            let shown = self
                .lane
                .as_ref()
                .filter(|lane| self.rows.iter().any(|r| &r.name == *lane));
            return Some(match shown {
                Some(lane) => Events::message(vec![
                    OscType::String(name.into()),
                    OscType::String(lane.clone()),
                ]),
                None => Events::refused(name, "no curve of the sequence is shown"),
            });
        }
        // a transposition with nothing selected is of every note taken, and a
        // channel's curve is the first channel's
        let channel_curve = name.starts_with("curve_");
        if self.selected.is_empty() && !moves && !channel_curve {
            return Some(Events::refused(name, "nothing is selected"));
        }
        Some(self.selection_event(name))
    }

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
            let grid = self.grid();
            let start = match playhead {
                Some(p) => grid.snap(p).max(0.0),
                None => self.step,
            };
            let dur = if grid.is_set() {
                grid.step_at(start)
            } else {
                MIN_DUR
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
                let grid = self.grid();
                self.step += if grid.is_set() {
                    grid.step_at(self.step)
                } else {
                    MIN_DUR
                };
            }
            None => {}
        }
        Some(self.notes_event())
    }

    /// What is held: an action over the selection reads it here first.
    fn holds_selection(&self) -> Option<bool> {
        Some(!self.selected.is_empty())
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
            grid: self.grid(),
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
                grabbed: nh.index,
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
                // Ctrl on a note removes it, and the drag goes on erasing what
                // it crosses: one edit, at the release.
                Some(nh) => {
                    notes::remove_note(&mut self.notes, nh.index);
                    self.selected = notes::selection_after_removal(&self.selected, nh.index);
                    self.drag = Some(Drag::Erase);
                    return Claim::take().edge_scrolling();
                }
                // Ctrl on empty grid adds one there, then drags its end to set
                // the length until release.
                None if !is_body => return self.press_new_note(h, at),
                None => return Claim::Decline,
            }
        }
        // **The tool says what a plain press is**; a note under the pencil
        // is taken as the pointer takes it
        match (self.tool, h.note) {
            (Tool::Draw, None) if !is_body => return self.press_new_note(h, at),
            (Tool::Erase, Some(nh)) => {
                notes::remove_note(&mut self.notes, nh.index);
                self.selected = notes::selection_after_removal(&self.selected, nh.index);
                self.drag = Some(Drag::Erase);
                return Claim::take().edge_scrolling();
            }
            (Tool::Erase, None) if !is_body => {
                self.drag = Some(Drag::Erase);
                return Claim::take().edge_scrolling();
            }
            (Tool::Split, Some(nh)) => {
                let at = self
                    .grid()
                    .snap(self.time_at(h.grid, &h.nav, at.0))
                    .max(0.0);
                if !self.selected.contains(&nh.index) {
                    self.selected = vec![nh.index];
                }
                return Claim::Take(Take {
                    events: self.split_at(at),
                    ..Take::default()
                });
            }
            (Tool::Stretch, Some(nh)) => {
                if !self.selected.contains(&nh.index) {
                    self.selected = vec![nh.index];
                }
                let orig: Vec<(usize, f64, f64)> = self
                    .selected
                    .iter()
                    .filter_map(|&i| self.notes.get(i).map(|n| (i, n.start, n.dur)))
                    .collect();
                let anchor = orig.iter().map(|o| o.1).fold(f64::INFINITY, f64::min);
                let end = orig.iter().map(|o| o.1 + o.2).fold(0.0, f64::max);
                self.drag = Some(Drag::Stretch {
                    press_time: self.time_at(h.grid, &h.nav, at.0),
                    anchor,
                    end,
                    orig,
                });
                return Claim::take().edge_scrolling();
            }
            // **A double click on empty grid makes a note**, with the pointer
            (Tool::Select, None) if !is_body && input.clicks >= 2 => {
                return self.press_new_note(h, at);
            }
            _ => {}
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
        }
        // **A click selects the note it is on**, and the drag moves it alone
        if !self.selected.contains(&nh.index) {
            self.selected = vec![nh.index];
        }
        // An edge of a **selected** note resizes the whole selection by as
        // much, as its body moves it.
        if nh.part != boxes::Part::Body
            && self.selected.len() > 1
            && self.selected.contains(&nh.index)
        {
            let mut idx = self.selected.clone();
            idx.retain(|&i| i != nh.index);
            idx.insert(0, nh.index);
            let orig = idx
                .into_iter()
                .filter(|&i| i < self.notes.len())
                .map(|i| (i, self.notes.placement(i)))
                .collect();
            self.drag = Some(Drag::Resize {
                part: nh.part,
                orig,
            });
            return Claim::take().edge_scrolling();
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

    /// **A note made where the hand pressed**, on the grid and the row under
    /// it, as long as the grid's step; the drag that follows sets its length
    /// until the release.
    fn press_new_note(&mut self, h: &Hit, at: (f64, f64)) -> Claim {
        let time = self
            .grid()
            .snap(self.time_at(h.grid, &h.nav, at.0))
            .max(0.0);
        let pitch = self.placed_pitch(h.axis, h.grid, at.1 as f32, 0.0);
        let dur = self.default_dur(&h.nav, time);
        let index = self.insert(notes::Note::new(time, dur, pitch));
        self.selected = vec![index];
        self.drag = Some(Drag::Note {
            index,
            part: boxes::Part::End,
            press_time: time,
            orig_start: time,
            orig_dur: dur,
        });
        // **The edit leaves at the release**, once: a report at the press
        // would bring the owner's correction mid-drag, its notes in its own
        // order, and the drag would go on resizing whatever note now has
        // this index
        Claim::take().edge_scrolling()
    }

    /// A press on the **OSC markers**, which show and do not write.
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
                    "the OSC markers show what a timeline holds besides notes, and do not \
                     write it: a marker is the message it sends, and its address is \
                     not something this strip can say",
                ),
                ..Take::default()
            });
        }
        let _ = h;
        Claim::Decline
    }
}

#[cfg(test)]
mod tests;
