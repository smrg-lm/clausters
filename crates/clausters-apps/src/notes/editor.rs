//! **The notes editor, one turn at a time**: what a gesture on the roll does to
//! the sequence, the entry it leaves, and what the window is corrected with.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_document::events::{self, EventsIntent};
use clausters_document::history::Editable;
use clausters_document::{EventSequence, Opaque};
use clausters_editing::conversation::{self, Answer, Conversation, Correction};
use clausters_editing::notes::{self as roll, Axis, Editing, Mode, Scope, YDomain};

use super::{Shared, correction, dialogs, props, window};
use crate::forms::{self, Said};
use crate::turn::{self, Converse, Event, Kind, Leg, Record};

/// The vocabulary the editor's structure is registered under.
pub const DOMAIN: &str = events::EVENTS;

/// **What one turn came to.**
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// What kind of turn the message was.
    pub turn: Kind,
    /// What to send the host.
    pub answer: Option<Answer>,
    /// The stamp a [`Kind::Step`] is answered with.
    pub seq: i64,
    /// Whether a [`Kind::Step`] walks forward.
    pub redo: bool,
    /// The entry to record, when the turn edited the sequence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// Whether the sequence changed.
    pub changed: bool,
    /// The version after the turn.
    pub version: i64,
    /// What the space bar asks of the playback: `{"looping", "range"}`, when
    /// it was pressed over the window -- `range` the time range a sweep left,
    /// `[start, end]` in beats, or `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play: Option<Value>,
    /// **What `L` asks of a pass in progress**: `{"looping", "range"}`, as
    /// for [`Outcome::play`] -- the caller changes the loop of what is playing,
    /// and a stopped playback reads the switch on its next play.
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub relooped: Option<Value>,
    /// Where the position cursor was placed, as a beat of the sequence -- a
    /// click on the roll's ruler. Not an edit: the caller cues a stopped
    /// playback there, and a play starts from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locate: Option<f64>,
    /// **The time range a sweep left**, when the hand moved it: `[start, end]`
    /// in beats, or `null` once it was cleared. Not an edit: the caller hands
    /// it to the playback, whose span it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Value>,
    /// **Whether to close the window** ([`crate::closing`]): a close asked of
    /// it, with nothing to lose or nothing to ask in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
}

turn::turned!(Outcome);

/// **The snap-to-grid switch**, as the window asks for it: a key (G), the
/// View menu's entry, or a client's `verb("snap")`.
pub const SNAP_VERB: &str = "snap";

/// **The monophonic switch**, as the window asks for it: a key (M), the Edit
/// menu's entry, or a client's `monophonic`.
pub const MONOPHONIC_VERB: &str = "monophonic";

/// **The verbs of the roll's modes, its view and its writing**: the mode
/// itself, then what only the notation mode answers -- which part, staff or
/// system it shows, the voice it edits, how a part writes its polyphony.
pub const MODE_VERBS: &[&str] = &[
    "midi_mode",
    "notation_mode",
    "show_system",
    "show_part",
    "show_staff",
    "staff_up",
    "staff_down",
    "voice_1",
    "voice_2",
    "voice_3",
    "voice_4",
    "write_voices",
    "write_ties",
];

/// The verbs only the notation mode answers: [`MODE_VERBS`] past the mode,
/// and the marks a page writes over the selected notes.
pub const NOTATION_VERBS: &[&str] = &[
    "show_system",
    "show_part",
    "show_staff",
    "staff_up",
    "staff_down",
    "voice_1",
    "voice_2",
    "voice_3",
    "voice_4",
    "write_voices",
    "write_ties",
    "slur",
    "slur_join",
    "tie",
    "staccato",
    "pedal",
];

/// **A notes editor**: a shared sequence, the domain its roll is drawn in,
/// and its end of the conversation.
#[derive(Clone, Debug)]
pub struct NotesEditor {
    sequence: Shared,
    domain: YDomain,
    rate: f64,
    editable: bool,
    conversation: Conversation,
    window: Option<i32>,
    widget: Option<i32>,
    title: String,
    size: (i64, i64),
    /// The time range a sweep left, `[start, end)` in beats, while there is
    /// one: what the space bar plays.
    range: Option<(f64, f64)>,
    /// Whether the window is composed with no chrome: no menu bar, no tools.
    bare: bool,
    /// **Whether the roll paints what is played** (its `midi_in`): the host
    /// opens its MIDI input and writes each incoming note into the roll -- at
    /// the play cursor while the transport rolls, on the step cursor stopped
    /// -- and reports the notes as a hand's edit, which is what they are to
    /// the sequence: recorded, and undone, like any other.
    midi_in: bool,
    /// **The grid a note lands on, in beats** ([`crate::DEFAULT_GRID`] until
    /// somebody chooses one): what a drag snaps to and a quantize writes
    /// into the sequence, read through its tempo map. `0` for none.
    grid: f64,
    /// **Whether a hand snaps to that grid** -- the snap-to-grid switch, on
    /// until somebody turns it off: off, a drag, a paste and step entry land
    /// where the hand puts them, and a quantize still moves onto the grid.
    snap: bool,
    /// **The loop switch**, as the host last said it: the host keeps it, and the
    /// window's tool and menu entry read it here.
    looping: bool,
    /// **How the roll edits** -- monophonically or not: the window's own, as
    /// snap is, and never the sequence's.
    editing: Editing,
    /// The dialog: its widgets by name, and the form that is up.
    dialogs: dialogs::Dialogs,
}

/// What a notes editor is opened with, as the context's door reads it.
#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Opened {
    rate: f64,
    editable: bool,
    domain: Option<Domain>,
    title: String,
    w: i64,
    h: i64,
    version: i64,
    /// `false` for a window with no menu bar and no tools.
    chrome: bool,
    /// Whether the roll paints incoming MIDI ([`NotesEditor::midi_in`]).
    #[serde(rename = "midi_in")]
    midi_in: bool,
    /// The grid in beats ([`NotesEditor::grid`]).
    grid: f64,
}

/// The Y domain a caller names: a word -- `"midi"`, or `"hz"` from MIDI note
/// 0 to 20 kHz -- or the whole [`YDomain`].
#[derive(Deserialize)]
#[serde(untagged)]
enum Domain {
    Named(String),
    Full(YDomain),
}

impl Domain {
    fn resolve(self) -> Option<YDomain> {
        match self {
            Domain::Full(domain) => Some(domain),
            Domain::Named(name) => match name.as_str() {
                "midi" => Some(YDomain::midi()),
                // MIDI note 0 to the top of hearing: past the highest MIDI
                // note, which a frequency is free to be.
                "hz" => Some(YDomain::hz(
                    clausters_core::scale::midi_to_hz(0.0),
                    20_000.0,
                )),
                _ => None,
            },
        }
    }
}

impl Default for Opened {
    fn default() -> Self {
        Self {
            rate: 48_000.0,
            editable: true,
            domain: None,
            title: "Notes".into(),
            w: 1000,
            h: 520,
            version: 1,
            chrome: true,
            midi_in: false,
            grid: crate::DEFAULT_GRID,
        }
    }
}

impl NotesEditor {
    /// An editor over `sequence`, drawing it at `rate` samples a second.
    pub fn new(sequence: Shared, rate: f64, version: i64) -> Self {
        Self {
            sequence,
            domain: YDomain::midi(),
            rate,
            editable: true,
            conversation: Conversation::new(version),
            window: None,
            widget: None,
            title: "Notes".into(),
            size: (1000, 520),
            range: None,
            bare: false,
            midi_in: false,
            grid: crate::DEFAULT_GRID,
            snap: true,
            looping: false,
            editing: Editing::default(),
            dialogs: dialogs::Dialogs::default(),
        }
    }

    /// How the roll edits.
    pub fn editing(&self) -> &Editing {
        &self.editing
    }

    /// The grid a note lands on, in beats; `0` for none.
    pub fn grid(&self) -> f64 {
        self.grid
    }

    /// Whether a hand snaps to the grid.
    pub fn snap(&self) -> bool {
        self.snap
    }

    /// **The window's actions as they read now**, by verb: `undo` and `redo`
    /// as the context's history reads, and the snap switch.
    #[must_use]
    pub fn actions(
        &self,
        undo: bool,
        redo: bool,
    ) -> std::collections::BTreeMap<&'static str, clausters_editing::verbs::ActionState> {
        crate::chrome::grid_actions(
            crate::chrome::App::Notes,
            false,
            undo,
            redo,
            &|verb| match verb {
                SNAP_VERB => Some(self.snap),
                MONOPHONIC_VERB => Some(self.editing.monophonic),
                "loop" => Some(self.looping),
                "midi_mode" => Some(self.editing.mode == Mode::Midi),
                "notation_mode" => Some(self.editing.mode == Mode::Notation),
                "show_system" => Some(self.editing.scope == Scope::System),
                "show_part" => Some(matches!(self.editing.scope, Scope::Part(_))),
                "show_staff" => Some(matches!(self.editing.scope, Scope::Staff(_))),
                "voice_1" | "voice_2" | "voice_3" | "voice_4" => {
                    Some(verb.ends_with(&(self.editing.voice[1] + 1).to_string()))
                }
                "write_voices" | "write_ties" => {
                    let ties = self.writes_with_ties();
                    Some((verb == "write_ties") == ties)
                }
                _ => None,
            },
        )
        .into_iter()
        .map(|(verb, mut state)| {
            // what is a page's is the notation mode's
            if NOTATION_VERBS.contains(&verb) && self.editing.mode != Mode::Notation {
                state.refuse("switch the roll to notation first");
            }
            (verb, state)
        })
        .collect()
    }

    /// Whether the sequence's first part writes its polyphony with ties.
    fn writes_with_ties(&self) -> bool {
        self.held()
            .parts_read()
            .first()
            .is_some_and(|p| p.writing.polyphony == clausters_document::events::Polyphony::Ties)
    }

    /// **One edit of the sequence a verb of the window makes**: applied,
    /// recorded under `label`, and the roll corrected. The refusal, if any.
    fn record(&mut self, intent: EventsIntent, label: &str, out: &mut Outcome) -> Option<String> {
        let payload = serde_json::to_value(&intent).unwrap_or(Value::Null);
        let mut sequence = self.held();
        let backward = events::payload(&sequence.state());
        match sequence.edit(intent) {
            Ok(change) if change.applied => {
                out.record = Some(Record {
                    label: label.into(),
                    legs: vec![Leg {
                        forward: json!({ "edit": payload }),
                        backward: backward.0,
                        key: events::coalesce_key(&Opaque(payload.clone())).unwrap_or_default(),
                        ..Leg::default()
                    }],
                });
                out.changed = true;
                out.version += 1;
                None
            }
            Ok(_) => None,
            Err(why) => Some(why),
        }
    }

    /// **A verb of the roll's modes, its view and its writing** (see
    /// [`NOTATION_VERBS`]): the window's state changed, or the one edit it
    /// makes -- a conversion entering notation over a sequence written as
    /// none, a writing of every part. The refusal, if any.
    fn mode_verb(
        &mut self,
        verb: &str,
        out: &mut Outcome,
        shown: &mut Vec<Correction>,
    ) -> Option<String> {
        let staves = || {
            self.held()
                .parts_read()
                .iter()
                .map(|p| p.staves.max(1))
                .sum::<usize>()
                .max(1)
        };
        match verb {
            "midi_mode" => self.editing.mode = Mode::Midi,
            "notation_mode" => {
                let written = self.held().written_as_notation();
                // **a window with the dialog asks how first**, and converts
                // at its OK
                if !written && self.dialogs.numbered(dialogs::forms()) {
                    let held = dialogs::notation_of(&self.held());
                    shown.extend(forms::corrections(
                        self.dialogs.open(dialogs::Form::Notation, held),
                    ));
                    return None;
                }
                if !written
                    && let Some(why) = self.record(
                        EventsIntent::ToNotation {
                            reading: None,
                            writing: None,
                            interpretation: None,
                        },
                        "write as notation",
                        out,
                    )
                {
                    return Some(why);
                }
                self.editing.mode = Mode::Notation;
            }
            _ if self.editing.mode != Mode::Notation => {
                return Some("switch the roll to notation first".into());
            }
            "show_system" => self.editing.scope = Scope::System,
            "show_staff" => self.editing.scope = Scope::Staff(self.editing.voice[0]),
            "show_part" => {
                let staff = self.editing.voice[0];
                let parts = self.held().parts_read();
                let mut first = 0;
                let part = parts
                    .iter()
                    .position(|p| {
                        let here = (first..first + p.staves.max(1)).contains(&staff);
                        first += p.staves.max(1);
                        here
                    })
                    .unwrap_or(0);
                self.editing.scope = Scope::Part(part);
            }
            "staff_up" | "staff_down" => {
                let staff = self.editing.voice[0];
                let next = if verb == "staff_up" {
                    staff.saturating_sub(1)
                } else {
                    (staff + 1).min(staves() - 1)
                };
                self.editing.voice = [next, self.editing.voice[1]];
                if let Scope::Staff(_) = self.editing.scope {
                    self.editing.scope = Scope::Staff(next);
                }
            }
            "voice_1" | "voice_2" | "voice_3" | "voice_4" => {
                let voice = verb[6..].parse::<usize>().unwrap_or(1) - 1;
                self.editing.voice = [self.editing.voice[0], voice];
            }
            "write_voices" | "write_ties" => {
                let ties = verb == "write_ties";
                let mut parts = self.held().parts_read();
                for part in &mut parts {
                    part.writing.polyphony = if ties {
                        clausters_document::events::Polyphony::Ties
                    } else {
                        clausters_document::events::Polyphony::Voices
                    };
                }
                let label = if ties {
                    "write the polyphony with ties"
                } else {
                    "write the polyphony in voices"
                };
                return self.record(EventsIntent::SetParts { parts }, label, out);
            }
            _ => {}
        }
        None
    }

    /// **The conversion `reading` and `writing` say**, made as one entry, and
    /// the roll put in notation mode. The refusal, if any.
    fn convert(
        &mut self,
        reading: Value,
        writing: clausters_document::events::Writing,
        out: &mut Outcome,
    ) -> Option<String> {
        let intent = EventsIntent::ToNotation {
            reading: Some(reading),
            writing: Some(writing),
            interpretation: None,
        };
        if let Some(why) = self.record(intent, "write as notation", out) {
            return Some(why);
        }
        self.editing.mode = Mode::Notation;
        None
    }

    /// **What a widget of the dialog said**: a field kept as it is typed or
    /// chosen, `OK` the conversion it holds -- the dialog left up with the
    /// reason when it is refused -- and `Cancel` the dialog taken down, the
    /// roll left in its mode.
    fn form_said(
        &mut self,
        name: &str,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let roll = self.widget.map_or(0, i64::from);
        match self.dialogs.read(name, tag, values) {
            Some(Said::Accept) => {
                let Some(open) = self.dialogs.up().cloned() else {
                    return (None, Vec::new());
                };
                let converted =
                    dialogs::conversion(&open).and_then(|(reading, writing)| {
                        match self.convert(reading, writing, out) {
                            Some(why) => Err(why),
                            None => Ok(()),
                        }
                    });
                if let Err(why) = converted {
                    return (Some(why), Vec::new());
                }
                let mut corrections = forms::corrections(self.dialogs.hide());
                corrections.extend(self.resync_widget(roll));
                (None, corrections)
            }
            Some(Said::Cancel | Said::Discard) => (None, forms::corrections(self.dialogs.hide())),
            _ => (None, Vec::new()),
        }
    }

    /// The sequence it edits.
    pub fn sequence(&self) -> &Shared {
        &self.sequence
    }

    fn held(&self) -> std::sync::MutexGuard<'_, EventSequence> {
        // A poisoned lock is a panic elsewhere while the sequence was held;
        // the data is still the sequence, and refusing it here would lose it.
        self.sequence.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **The window**, with the roll under `widget` -- which is then the one
    /// the editor answers for.
    pub fn window(&mut self, widget: i32) -> Value {
        self.widget = Some(widget);
        let mut drawn = props(
            &self.held(),
            &self.domain,
            self.rate,
            self.editable,
            &self.editing,
        );
        // the grid is an axis' own, as its tempo map is
        if let Some(Value::Object(x)) = drawn.get_mut("axes").and_then(|a| a.get_mut("x")) {
            x.insert("grid".into(), json!(self.grid));
            x.insert("grid_snap".into(), json!(self.snap));
        }
        if self.midi_in {
            drawn.insert("midi_in".into(), json!(1));
        }
        let mut window = window(drawn, widget, &self.title, self.size);
        if !self.bare {
            use crate::chrome::{self, App};
            chrome::dress(
                &mut window,
                chrome::menu(App::Notes, false),
                chrome::toolbar(App::Notes, None),
                widget,
            );
            // **the dialog stands over the window**, on a page of its stack
            if let (Some(stack), Some(children)) = (
                self.dialogs.stack(dialogs::forms()),
                window.get_mut("children").and_then(Value::as_array_mut),
            ) {
                children.push(stack);
            }
        }
        window
    }

    /// What `widget` is corrected with, or nothing for one that is not the
    /// roll.
    fn resync_widget(&self, widget: i64) -> Vec<Correction> {
        if self.widget.map(i64::from) != Some(widget) {
            return Vec::new();
        }
        let held = self.held();
        let mut props = correction(&held, &self.domain, self.rate, &self.editing);
        // The time range is drawn where the hand sweeps one, so a span set
        // from the client shows as the band a sweep leaves.
        let axis = Axis::of(&held, self.rate);
        let (start, len) = self.range.map_or((0.0, 0.0), |(a, b)| {
            let start = axis.units(a);
            (start, axis.units(b) - start)
        });
        props.insert("sel_start".into(), json!(start));
        props.insert("sel_len".into(), json!(len));
        // the switch is the window's own, so a correction states it too
        props.insert("midi_in".into(), json!(i32::from(self.midi_in)));
        props.insert("grid".into(), json!(self.grid));
        props.insert("grid_snap".into(), json!(self.snap));
        vec![Correction {
            widget,
            props: Value::Object(props),
        }]
    }

    /// **Every widget of the window, corrected** -- what a history step leaves
    /// behind.
    pub fn resync_all(&self, version: i64) -> Answer {
        let corrections = self
            .widget
            .map_or_else(Vec::new, |w| self.resync_widget(i64::from(w)));
        conversation::answer(0, version, None, corrections)
    }

    /// Answers the stamp a [`Kind::Step`] carried, once the caller has walked.
    pub fn acknowledge(&self, seq: i64, version: i64, reason: Option<String>) -> Answer {
        conversation::answer(seq, version, reason, Vec::new())
    }

    /// **One message from the host**, read and answered ([`turn::turn`]).
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        turn::turn(self, event, version)
    }

    /// **What a history step is put back to** when another structure of the
    /// same step refuses it: the sequence.
    pub(crate) fn before_step(&self) -> EventSequence {
        self.held().clone()
    }

    /// Puts back what [`before_step`](Self::before_step) kept.
    pub(crate) fn put_back(&mut self, kept: EventSequence) {
        *self.held() = kept;
    }

    /// **One payload of a history step**, applied to the sequence. Answers
    /// whether it changed.
    pub fn apply(&mut self, payload: &Value) -> bool {
        self.held().apply(&Opaque(payload.clone())).applied
    }

    fn gesture(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if tag == "selection" {
            // A sweep's time range, on the axis's samples, kept in beats: the
            // span the space bar plays. A range of no length is none.
            let axis = Axis::of(&self.held(), self.rate);
            let at = |i: usize| {
                values
                    .get(i)
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    .max(0.0)
            };
            let (start, len) = (at(0), at(1));
            self.range = (len > 0.0).then(|| (axis.beat(start), axis.beat(start + len)));
            out.span = Some(self.range.map_or(Value::Null, |(a, b)| json!([a, b])));
            return (None, Vec::new());
        }
        if tag == "locate" {
            // A click on the ruler: the reader put the position cursor there,
            // on the axis's sample, which the sequence's tempo map reads as a
            // beat. It is not a seek -- the playhead is never placed.
            if let Some(units) = values.first().and_then(Value::as_f64) {
                out.locate = Some(Axis::of(&self.held(), self.rate).beat(units.max(0.0)));
            }
            return (None, Vec::new());
        }
        if !matches!(
            tag,
            "notes"
                | "osc"
                | "points"
                | "join"
                | "unjoin"
                | "slur"
                | "slur_join"
                | "tie"
                | "staccato"
                | "pedal"
        ) {
            return (None, Vec::new());
        }
        if !self.editable {
            return (
                Some("these notes are a rendering of an algorithm: render it to a track to edit them".into()),
                self.resync_widget(widget),
            );
        }
        let recorded = {
            let mut sequence = self.held();
            let axis = Axis::of(&sequence, self.rate);
            let intake = roll::intake(&sequence, tag, values, &axis, &self.domain, &self.editing);
            if let Some(why) = intake.refusal {
                drop(sequence);
                return (Some(why), self.resync_widget(widget));
            }
            let Some(payload) = intake.payloads.into_iter().next() else {
                return (None, Vec::new());
            };
            let Ok(intent) = serde_json::from_value::<EventsIntent>(payload.clone()) else {
                return (None, Vec::new());
            };
            let backward = events::payload(&sequence.state());
            match sequence.edit(intent) {
                Ok(change) if change.applied => Some(Record {
                    label: intake.label,
                    legs: vec![Leg {
                        forward: json!({ "edit": payload }),
                        backward: backward.0,
                        key: events::coalesce_key(&Opaque(payload.clone())).unwrap_or_default(),
                        ..Leg::default()
                    }],
                }),
                Ok(_) => None,
                Err(why) => {
                    drop(sequence);
                    return (Some(why), self.resync_widget(widget));
                }
            }
        };
        if let Some(record) = recorded {
            out.record = Some(record);
            out.changed = true;
            out.version += 1;
        }
        // The roll learns the ids the sequence gave the notes the hand made.
        (None, self.resync_widget(widget))
    }
}

impl Converse for NotesEditor {
    type Outcome = Outcome;

    fn conversation(&mut self) -> &mut Conversation {
        &mut self.conversation
    }

    fn window_id(&self) -> Option<i32> {
        self.window
    }

    fn closed(&mut self) {
        self.window = None;
        self.dialogs.closed();
    }

    fn owns(&self, widget: i64, _tag: &str) -> bool {
        self.widget.map(i64::from) == Some(widget) || self.dialogs.name(widget).is_some()
    }

    fn resync(&mut self, widget: i64) -> Vec<Correction> {
        self.resync_widget(widget)
    }

    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if let Some(name) = self.dialogs.name(widget).map(str::to_string) {
            return self.form_said(&name, tag, values, out);
        }
        self.gesture(widget, tag, values, out)
    }

    /// **The space bar over the window is a play**, the window's own verb
    /// (the window says `plays`): what it asks of the playback, with the loop
    /// switch the host sends beside it and the time range a sweep left -- the
    /// audio editor's pass, over a roll.
    fn window_verb(
        &mut self,
        message: &conversation::Message,
        args: &[Value],
        out: &mut Outcome,
    ) -> bool {
        if message.addr != "/gui_event" || !message.is_window {
            return false;
        }
        // **The snap-to-grid switch** (G, the View menu): the window's own
        // state, so nothing is recorded and the roll is told
        if MODE_VERBS.contains(&message.tag.as_str()) {
            let tag = message.tag.clone();
            out.turn = Kind::Route;
            let mut shown = Vec::new();
            let why = self.mode_verb(&tag, out, &mut shown);
            let roll = self.widget.map_or(0, i64::from);
            shown.extend(self.resync_widget(roll));
            out.answer = Some(conversation::answer(message.seq, out.version, why, shown));
            return true;
        }
        if message.tag == SNAP_VERB || message.tag == MONOPHONIC_VERB {
            if message.tag == SNAP_VERB {
                self.snap = !self.snap;
            } else {
                self.editing.monophonic = !self.editing.monophonic;
            }
            out.turn = Kind::Route;
            let roll = self.widget.map_or(0, i64::from);
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                None,
                self.resync_widget(roll),
            ));
            return true;
        }
        if !matches!(message.tag.as_str(), "play" | "loop") {
            return false;
        }
        out.turn = Kind::Route;
        let looping = args.get(4).and_then(Value::as_i64).is_some_and(|v| v != 0);
        self.looping = looping;
        let pass = json!({
            "looping": looping,
            "range": self.range.map(|(a, b)| json!([a, b])),
        });
        // The space bar plays or stops; `L` changes the pass in progress.
        if message.tag == "play" {
            out.play = Some(pass);
        } else {
            out.relooped = Some(pass);
        }
        out.answer = Some(conversation::answer(
            message.seq,
            out.version,
            None,
            Vec::new(),
        ));
        true
    }
}

/// **A notes editor from JSON**: `{"rate", "editable", "domain", "title", "w",
/// "h", "version", "chrome", "midi_in", "grid"}` over `sequence` -- `chrome` `false`
/// for a window with no menu bar and no tools, `midi_in` `true` for a roll
/// that paints what is played.
pub fn new_json(sequence: Shared, request: &str) -> NotesEditor {
    let opened: Opened = serde_json::from_str(request).unwrap_or_default();
    let mut editor = NotesEditor::new(sequence, opened.rate, opened.version);
    editor.editable = opened.editable;
    if let Some(domain) = opened.domain.and_then(Domain::resolve) {
        editor.domain = domain;
    }
    editor.title = opened.title;
    editor.size = (opened.w, opened.h);
    editor.bare = !opened.chrome;
    editor.midi_in = opened.midi_in;
    editor.grid = opened.grid.max(0.0);
    editor
}

/// A sequence of its own, for a caller that hands the editor data rather than
/// a sequence it already shares: the request's `sequence`, or an empty one.
pub fn shared_of(request: &str) -> Shared {
    let sequence = serde_json::from_str::<Value>(request)
        .ok()
        .and_then(|r| r.get("sequence").cloned())
        .and_then(|s| serde_json::from_value::<EventSequence>(s).ok())
        .unwrap_or_default();
    Arc::new(Mutex::new(sequence))
}

/// **One verb of the editor's own door**, over JSON:
///
/// - `window` -- `widget`, `dialogs` (the id of each widget of the dialog,
///   by its name; without all of them the window has none): the GuiDef, the
///   roll under that id.
/// - `dialogs` -- `{"dialogs"}`: the names of the dialog's widgets, for a
///   caller to number and hand to `window`; none for a bare editor.
/// - `props` -- `widget`: what it is corrected with (`{}` for another widget).
/// - `sync` -- `window` (the id it is open in, or `null`), `rate`, `editable`,
///   `midi_in`, `grid` (in beats, `0` for none), `snap` (the snap-to-grid
///   switch), `monophonic`, `scope` (`{"show": "system"}`, or `"part"` or
///   `"staff"` with its `index`), `voice` (`[staff, voice]`), `domain`,
///   `title`, `w`, `h`: `{}`.
/// - `grid` -- `{"grid": beats, "snap": bool}`: the grid a note lands on, and
///   whether a hand snaps to it.
/// - `editing` -- how the roll edits ([`Editing`]): `{"monophonic", "mode",
///   "scope", "voice"}`, and `written`, whether the sequence is written as
///   notation already -- entering notation over one that is not converts
///   it, or asks how in a window with the dialog. The mode is changed by its verbs (`midi_mode`,
///   `notation_mode`), since entering notation may convert the sequence,
///   which is an edit.
/// - `state` -- the sequence, whole.
/// - `span` -- `span`: `[start, end]` in beats, or `null`: the time range the
///   space bar plays and the roll draws, as a sweep leaves it. `{}`.
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut NotesEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let widget = request.get("widget").and_then(Value::as_i64).unwrap_or(0) as i32;
    match request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "window" => {
            // what the caller numbered, by the names the crate gave: a name
            // that is none of them numbers nothing
            let names = dialogs::names();
            editor.dialogs.ids = request
                .get("dialogs")
                .and_then(Value::as_object)
                .filter(|_| !editor.bare)
                .map(|map| {
                    map.iter()
                        .filter(|(name, _)| names.contains(name))
                        .filter_map(|(name, id)| Some((name.clone(), id.as_i64()? as i32)))
                        .collect()
                })
                .unwrap_or_default();
            editor.window(widget).to_string()
        }
        "dialogs" if editor.bare => json!({"dialogs": []}).to_string(),
        "dialogs" => json!({"dialogs": dialogs::names()}).to_string(),
        "props" => match editor.resync_widget(i64::from(widget)).into_iter().next() {
            Some(c) => c.props.to_string(),
            None => "{}".into(),
        },
        "sync" => {
            if let Some(window) = request.get("window") {
                editor.window = window.as_i64().map(|w| w as i32);
            }
            if let Some(rate) = request.get("rate").and_then(Value::as_f64) {
                editor.rate = rate;
            }
            if let Some(editable) = request.get("editable").and_then(Value::as_bool) {
                editor.editable = editable;
            }
            if let Some(midi_in) = request.get("midi_in").and_then(Value::as_bool) {
                editor.midi_in = midi_in;
            }
            if let Some(grid) = request.get("grid").and_then(Value::as_f64) {
                editor.grid = grid.max(0.0);
            }
            if let Some(snap) = request.get("snap").and_then(Value::as_bool) {
                editor.snap = snap;
            }
            if let Some(on) = request.get("monophonic").and_then(Value::as_bool) {
                editor.editing.monophonic = on;
            }
            if let Some(scope) = request
                .get("scope")
                .and_then(|s| serde_json::from_value::<Scope>(s.clone()).ok())
            {
                editor.editing.scope = scope;
            }
            if let Some(voice) = request
                .get("voice")
                .and_then(|v| serde_json::from_value::<[usize; 2]>(v.clone()).ok())
            {
                editor.editing.voice = voice;
            }
            if let Some(domain) = request
                .get("domain")
                .and_then(|d| serde_json::from_value::<Domain>(d.clone()).ok())
                .and_then(Domain::resolve)
            {
                editor.domain = domain;
            }
            if let Some(title) = request.get("title").and_then(Value::as_str) {
                editor.title = title.into();
            }
            if let (Some(w), Some(h)) = (
                request.get("w").and_then(Value::as_i64),
                request.get("h").and_then(Value::as_i64),
            ) {
                editor.size = (w, h);
            }
            "{}".into()
        }
        "state" => serde_json::to_string(&*editor.held()).unwrap_or_else(|_| "{}".into()),
        "range" => json!({ "range": editor.range }).to_string(),
        "grid" => json!({ "grid": editor.grid, "snap": editor.snap }).to_string(),
        "editing" => {
            let mut said = json!(editor.editing);
            said["written"] = json!(editor.held().written_as_notation());
            said.to_string()
        }
        "span" => {
            editor.range = request
                .get("span")
                .and_then(Value::as_array)
                .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)))
                .filter(|(a, b)| b > a);
            "{}".into()
        }
        _ => "{}".into(),
    }
}

#[cfg(test)]
mod tests;
