//! **The notes editor, one turn at a time**: what a gesture on the roll does to
//! the sequence, the entry it leaves, and what the window is corrected with.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_document::events::{self, EventsIntent};
use clausters_document::history::Editable;
use clausters_document::{EventSequence, Opaque};
use clausters_editing::conversation::{self, Answer, Conversation, Correction};
use clausters_editing::notes::{self as roll, Axis, ColorBy, Editing, Mode, Scope, YDomain};

use super::{Shared, correction, dialogs, display, panel, props, window};
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
    /// **Whether the window is drawn again** (the panel shown or hidden): the
    /// caller asks for its tree anew, and the host keeps what is its own.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub restructure: bool,
    /// **The file to save the sequence to**, for a roll that has a file
    /// ([`crate::file`]): whoever drives the editor writes it there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save: Option<String>,
    /// **The window's document to change** (File > New, Open, Close file),
    /// with nothing to lose or once the close form was answered. With a file
    /// to save, the change waits for it to be written.
    #[serde(skip)]
    pub file: Option<crate::closing::FileChange>,
    /// The revision a store confirmed, when the runtime settled one.
    #[serde(skip)]
    pub saved: Option<Box<crate::file::Saved>>,
    /// What the window left its document as, when it let it go.
    #[serde(skip)]
    pub left: Option<Value>,
    /// **A conversion's copy**, `{"sequence", "title"}`: the sequence written
    /// in another format, which the caller opens in a roll of its own. The
    /// sequence this editor edits is not changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub converted: Option<Value>,
}

turn::turned!(Outcome);
crate::file::outcome!(Outcome);

/// **The snap-to-grid switch**, as the window asks for it: a key (G), the
/// View menu's entry, or a client's `verb("snap")`.
pub const SNAP_VERB: &str = "snap";

/// **The monophonic switch**, as the window asks for it: a key (M), the Edit
/// menu's entry, or a client's `monophonic`.
pub const MONOPHONIC_VERB: &str = "monophonic";

/// **The roll's file verbs**, answered by a roll that has a file
/// ([`crate::file`]).
pub const FILE_VERBS: &[&str] = &["new_file", "open_file", "save", "save_as", "close_file"];

/// Why a roll whose sequence another holds has no file.
const NO_FILE: &str = "this roll edits a sequence its holder keeps: it has no file of its own";

/// **The conversions**, each into a copy in another format, which opens in a
/// roll of its own: the page (asked how in a window with the dialog) and the
/// three MIDI specs.
pub const CONVERSIONS: &[&str] = &[
    "convert_notation",
    "convert_midi1",
    "convert_mpe",
    "convert_midi2",
];

/// **The verbs of the roll's view and its writing**: the conversions, then
/// what only the notation mode answers -- which part, staff or system it
/// shows, the voice it edits, how a part writes its polyphony.
pub const MODE_VERBS: &[&str] = &[
    "convert_notation",
    "convert_midi1",
    "convert_mpe",
    "convert_midi2",
    "show_system",
    "show_part",
    "show_staff",
    "staff_up",
    "staff_down",
    "voice_1",
    "voice_2",
    "voice_3",
    "voice_4",
    "color_voice",
    "color_staff",
    "color_part",
    "write_voices",
    "write_ties",
];

/// The verbs only the notation mode answers: [`MODE_VERBS`] past the
/// conversions;
/// the marks a page writes over the selected notes are the editing crate's
/// ([`clausters_editing::notes::notation::ARTICULATIONS`]), and only the
/// notation mode answers them too (`only_in_notation`).
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
    "color_voice",
    "color_staff",
    "color_part",
    "write_voices",
    "write_ties",
];

/// Whether `verb` is one only the notation mode answers.
fn only_in_notation(verb: &str) -> bool {
    NOTATION_VERBS.contains(&verb)
        || clausters_editing::notes::notation::ARTICULATIONS.contains(&verb)
}

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
    /// **Whether the panel is shown**: the inspector and the list of events
    /// beside the roll ([`panel`]), the window's own.
    panel: bool,
    /// The notes the roll last said the hand marked, by id.
    marked: Vec<u64>,
    /// **The lane shown under the roll**, by name -- the velocities, or a
    /// curve of the sequence by its id: chosen in the tabs the roll draws over it,
    /// the window's own.
    lane: String,
    /// **The symbols the roll draws a page's marks with**, asked of the
    /// engraver the application handed in when the roll first enters
    /// notation ([`super::Symbols`]).
    symbols: super::Symbols,
    /// **How the roll draws what the sequence holds beyond its boxes**
    /// ([`display`]): the window's own, outside the history.
    display: display::Display,
    /// Whether the display panel is shown beside the roll.
    display_panel: bool,
    /// **Whether the window is its sequence's only holder** -- a roll the
    /// host opened over a file or over a conversion's copy -- so it has a
    /// file, and asks before a close loses what its file does not hold
    /// ([`crate::closing`]). A client's roll and a box's have none.
    asks: bool,
    /// The file the sequence is saved to: the one it was read from, or the
    /// last one a save named. `None` for one with no file yet.
    path: Option<String>,
    /// **The sequence as its file holds it**: what it was when the window
    /// opened, or was last saved, opened or made new.
    saved: EventSequence,
    /// How many times `saved` was written: with the context's version, what
    /// [`NotesEditor::unsaved_at`] reads again by.
    saves: u64,
    /// The last [`NotesEditor::unsaved_at`] answer, by the context's version
    /// and `saves`.
    dirty: std::cell::Cell<Option<((i64, u64), bool)>>,
    /// **Whether the window has a document**: Close file leaves it with none.
    document: bool,
    /// **The identity of the document in the window**: one let go of takes
    /// its order with it, and a store prepared from one is not confirmed
    /// over another.
    binding: i64,
    /// What the close form was asked about, when it is up for something.
    leaving: Option<crate::closing::Leaving>,
    /// **The engraver's reading**, which a score opened in the roll is read
    /// with ([`super::file::read`]).
    scores: super::file::Scores,
}

/// **The panel's switch**, as the window asks for it: the View menu's
/// entry, or a client's `verb("events_panel")`.
pub const PANEL_VERB: &str = "events_panel";

/// **The display panel's switch**, as the window asks for it: the View
/// menu's entry, or a client's `verb("display_settings")`.
pub const DISPLAY_VERB: &str = "display_settings";

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
        let saved = sequence.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Self {
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
            panel: false,
            marked: Vec::new(),
            lane: "velocity".into(),
            symbols: super::Symbols::default(),
            display: display::Display::default(),
            display_panel: false,
            asks: false,
            path: None,
            saved,
            saves: 0,
            dirty: std::cell::Cell::new(None),
            document: true,
            binding: 1,
            leaving: None,
            scores: super::file::Scores::default(),
            sequence,
        }
    }

    /// **Where the roll's marks get their outlines**: the engraver the
    /// application holds, asked only once the roll shows a page.
    pub fn set_outlines(&mut self, source: Option<super::OutlineSource>) {
        self.symbols = super::Symbols::with(source);
    }

    /// **What a score opened in the roll is read with**: the engraver the
    /// application holds ([`super::file::ScoreReader`]).
    pub fn set_scores(&mut self, scores: Option<super::file::ScoreReader>) {
        self.scores = super::file::Scores(scores);
    }

    /// **The window is its sequence's only holder** ([`NotesEditor::asks`]):
    /// it has a file, and asks before a close lets unsaved changes go.
    pub fn set_asks_to_close(&mut self, asks: bool) {
        self.asks = asks;
    }

    /// The title its window was opened with.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The file the sequence was read from or last saved to, if any.
    pub fn file_path(&self) -> Option<String> {
        self.path.clone()
    }

    /// **Whether the sequence has changes its file does not hold.**
    #[must_use]
    pub fn unsaved(&self) -> bool {
        self.document && *self.held() != self.saved
    }

    /// **Whether there is unsaved work**, read at the context's `version`:
    /// [`Self::unsaved`] again only when an edit or a save may have moved
    /// it, and its last answer otherwise, since the window's title asks after
    /// every turn and the comparison walks the whole sequence.
    pub(crate) fn unsaved_at(&self, version: i64) -> bool {
        let key = (version, self.saves);
        if let Some((read, dirty)) = self.dirty.get()
            && read == key
        {
            return dirty;
        }
        let dirty = self.unsaved();
        self.dirty.set(Some((key, dirty)));
        dirty
    }

    /// The sequence its file holds, from now on.
    fn set_saved(&mut self, saved: EventSequence) {
        self.saved = saved;
        self.saves += 1;
    }

    /// What the window leaves its document as, for whoever held it; nothing
    /// when it has none.
    fn left(&self) -> Option<Value> {
        self.document
            .then(|| json!({"sequence": *self.held(), "path": self.path}))
    }

    /// Nothing of the document that went stays on the window.
    fn start_over(&mut self) {
        self.binding += 1;
        self.leaving = None;
        self.range = None;
        self.marked.clear();
        let saved = self.held().clone();
        self.set_saved(saved);
    }

    /// **A file verb of the window** (File > New, Open..., Save, Save as...,
    /// Close file), for a roll that has a file: what it asks of the turn, or
    /// the form it opens. `None` for a verb that is no file verb.
    fn file_verb(
        &mut self,
        verb: &str,
        out: &mut Outcome,
    ) -> Option<(Option<String>, Vec<Correction>)> {
        use crate::closing::FileChange;
        Some(match verb {
            "save" => crate::file::save(self, out),
            "save_as" => self.open_file_form(dialogs::Form::SaveAs),
            "open_file" => self.open_file_form(dialogs::Form::Open),
            "new_file" => crate::file::change_file(self, FileChange::New, out),
            "close_file" => crate::file::change_file(self, FileChange::Close, out),
            _ => return None,
        })
    }

    /// **A file form opened**, holding the file the window has: or why it
    /// cannot be.
    fn open_file_form(&mut self, form: dialogs::Form) -> (Option<String>, Vec<Correction>) {
        use crate::file::FileWindow;
        if let Some(why) = self.no_file() {
            return (Some(why.into()), Vec::new());
        }
        if !self.dialogs.numbered(dialogs::forms()) {
            return (Some("this window has no dialogs".into()), Vec::new());
        }
        let offered = match form {
            dialogs::Form::SaveAs => self.path.clone().unwrap_or_else(|| {
                let format = self.held().format();
                format!("untitled.{}", super::file::extension(format))
            }),
            _ => String::new(),
        };
        let values = [("path".to_string(), offered)].into();
        (None, forms::corrections(self.dialogs.open(form, values)))
    }

    /// **The outlines the roll's marks are drawn with, owed to it**: asked
    /// of the engraver the first time the roll is in notation mode, and handed
    /// to the roll once, as its `glyphs`.
    fn glyphs_owed(&mut self) -> Option<Correction> {
        if !self.notation() {
            return None;
        }
        let widget = i64::from(self.widget?);
        let found = self.symbols.ask()?;
        (!found.is_empty()).then(|| Correction {
            widget,
            props: json!({ "glyphs": found }),
        })
    }

    /// **How the roll edits**, its mode the one the sequence's format allows.
    pub fn editing(&self) -> Editing {
        self.editing_over(&self.held())
    }

    /// How the roll edits `held`, the sequence it holds: the window's state,
    /// in the mode its format allows ([`Mode::of`]).
    fn editing_over(&self, held: &EventSequence) -> Editing {
        let mode = Mode::of(held);
        Editing {
            mode,
            // a channel is chosen only where one can be
            channel: self
                .editing
                .channel
                .filter(|_| roll::channel_refused(held, mode).is_none()),
            ..self.editing.clone()
        }
    }

    /// **The channel the MIDI mode edits, chosen** (`None` for every one),
    /// or why it is not: a page is edited by its voice, and under MPE a
    /// channel is one note's. The window's, like the voice: no edit.
    fn choose_channel(&mut self, channel: Option<u8>) -> Option<String> {
        if channel.is_some() {
            let held = self.held();
            if let Some(why) = roll::channel_refused(&held, Mode::of(&held)) {
                return Some(why.into());
            }
        }
        self.editing.channel = channel.map(|c| c.min(15));
        None
    }

    /// Whether the roll is in notation mode: its sequence is written as a page.
    fn notation(&self) -> bool {
        Mode::of(&self.held()) == Mode::Notation
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
        let format = self.held().format();
        let notation = format == events::Format::Notation;
        let channel_refused = {
            let held = self.held();
            roll::channel_refused(&held, Mode::of(&held))
        };
        crate::chrome::grid_actions(crate::chrome::App::Notes, self.asks, undo, redo, &|verb| {
            match verb {
                SNAP_VERB => Some(self.snap),
                MONOPHONIC_VERB => Some(self.editing.monophonic),
                PANEL_VERB => Some(self.panel),
                DISPLAY_VERB => Some(self.display_panel),
                "loop" => Some(self.looping),
                "show_system" => Some(self.editing.scope == Scope::System),
                "show_part" => Some(matches!(self.editing.scope, Scope::Part(_))),
                "show_staff" => Some(matches!(self.editing.scope, Scope::Staff(_))),
                "voice_1" | "voice_2" | "voice_3" | "voice_4" => {
                    Some(verb.ends_with(&(self.editing.voice[1] + 1).to_string()))
                }
                _ if roll::CHANNEL_VERBS.contains(&verb) => {
                    Some(roll::channel_chosen(verb) == Some(self.editing().channel))
                }
                "write_voices" | "write_ties" => {
                    let ties = self.writes_with_ties();
                    Some((verb == "write_ties") == ties)
                }
                "color_voice" => Some(self.editing.color_by == ColorBy::Voice),
                "color_staff" => Some(self.editing.color_by == ColorBy::Staff),
                "color_part" => Some(self.editing.color_by == ColorBy::Part),
                _ => None,
            }
        })
        .into_iter()
        .map(|(verb, mut state)| {
            // what is a page's is the notation mode's
            if only_in_notation(verb) && !notation {
                state.refuse(
                    "the sequence is not written as notation: convert a copy to edit a page",
                );
            }
            if let Some(why) = conversion_refused(verb, format) {
                state.refuse(&why);
            }
            if roll::CHANNEL_VERBS.contains(&verb)
                && let Some(why) = channel_refused
            {
                state.refuse(why);
            }
            if FILE_VERBS.contains(&verb)
                && let Some(why) = crate::file::FileWindow::no_file(self)
            {
                state.refuse(why);
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
    /// recorded under `label`, and the roll corrected. The refusal, if any,
    /// or what the edit renumbered of what it brought in.
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
                events::ids::describe(&change.renumbered)
            }
            Ok(_) => None,
            Err(why) => Some(why),
        }
    }

    /// **A verb of the roll's view and its writing** (see [`MODE_VERBS`]):
    /// a conversion into a copy, the window's state changed, or the one edit
    /// it makes, a writing of every part. The refusal, if any.
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
        if let Some(channel) = roll::channel_chosen(verb) {
            return self.choose_channel(channel);
        }
        match verb {
            _ if CONVERSIONS.contains(&verb) => return self.convert_by(verb, out, shown),
            _ if !self.notation() => {
                return Some(
                    "the sequence is not written as notation: convert a copy to edit a page".into(),
                );
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
                    staff.checked_sub(1)
                } else {
                    Some(staff + 1).filter(|n| *n < staves())
                };
                // the edge says so rather than doing nothing
                let Some(next) = next else {
                    return Some(if verb == "staff_up" {
                        "this is the top staff".into()
                    } else {
                        "this is the bottom staff".into()
                    });
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
            "color_voice" => self.editing.color_by = ColorBy::Voice,
            "color_staff" => self.editing.color_by = ColorBy::Staff,
            "color_part" => self.editing.color_by = ColorBy::Part,
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

    /// **A conversion asked by its verb**: into notation, asked how first in
    /// a window with the dialog and read with what the sequence stores
    /// otherwise; into a MIDI spec, at once. The refusal, if any.
    fn convert_by(
        &mut self,
        verb: &str,
        out: &mut Outcome,
        shown: &mut Vec<Correction>,
    ) -> Option<String> {
        let format = self.held().format();
        if let Some(why) = conversion_refused(verb, format) {
            return Some(why);
        }
        if verb == "convert_notation" {
            if self.dialogs.numbered(dialogs::forms()) {
                let held = dialogs::notation_of(&self.held());
                shown.extend(forms::corrections(
                    self.dialogs.open(dialogs::Form::Notation, held),
                ));
                return None;
            }
            let copy = self.held().notated(None, None, None);
            return self.converted(copy, "notation", out);
        }
        let (spec, name) = spec_of(verb)?;
        let copy = self.held().as_midi(spec);
        self.converted(copy, name, out)
    }

    /// **The conversion into notation `reading` and `writing` say**, made
    /// into a copy. The refusal, if any.
    fn convert(
        &mut self,
        reading: Value,
        writing: clausters_document::events::Writing,
        out: &mut Outcome,
    ) -> Option<String> {
        let copy = self.held().notated(Some(reading), Some(writing), None);
        self.converted(copy, "notation", out)
    }

    /// **A conversion's copy handed to the caller** to open in a roll of its
    /// own, titled after this one and the format it is written in; the
    /// sequence here is left as it was. The refusal, if any.
    fn converted(
        &self,
        copy: Result<EventSequence, String>,
        name: &str,
        out: &mut Outcome,
    ) -> Option<String> {
        match copy {
            Ok(sequence) => {
                out.converted = Some(json!({
                    "sequence": sequence,
                    "title": format!("{} ({name})", self.title),
                }));
                None
            }
            Err(why) => Some(why),
        }
    }

    /// **What a widget of the dialogs said**: a field kept as it is typed or
    /// chosen; `OK` what the form up holds -- the conversion into a copy, the
    /// file opened, the path saved to, the close form's Save -- the form left
    /// up with the reason when it is refused; a pick in the Open form's
    /// chooser as its `OK`; `Cancel` the form taken down; and the close
    /// form's `Don't save` what it was asked about let go.
    fn form_said(
        &mut self,
        name: &str,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        use dialogs::Form;
        let roll = self.widget.map_or(0, i64::from);
        let Some(said) = self.dialogs.read(name, tag, values) else {
            return (None, Vec::new());
        };
        let Some(open) = self.dialogs.up().cloned() else {
            return (None, Vec::new());
        };
        let refused = match said {
            Said::Cancel => {
                self.leaving = None;
                return (None, forms::corrections(self.dialogs.hide()));
            }
            // closing without saving: what was not saved is let go
            Said::Discard => {
                crate::file::let_what_waits_go(self, out);
                return (None, forms::corrections(self.dialogs.hide()));
            }
            Said::Picked(_, paths) if open.form == Form::Open => self.open_chosen(&paths, out),
            Said::Accept => match open.form {
                Form::Notation => match dialogs::conversion(&open) {
                    Ok((reading, writing)) => self.convert(reading, writing, out),
                    Err(why) => Some(why),
                },
                Form::Open => {
                    let paths = self.dialogs.chosen("path").to_vec();
                    self.open_chosen(&paths, out)
                }
                Form::SaveAs => crate::file::path_given(self, false, open.value("path"), out),
                Form::Close => crate::file::save_then_go(self, out),
            },
            _ => return (None, Vec::new()),
        };
        if let Some(why) = refused {
            return (Some(why), Vec::new());
        }
        // a save the close form asked for, with no file yet, asks for one:
        // the Save as form is now the one up
        if let Some(up) = self.dialogs.up().filter(|up| up.form != open.form) {
            return (None, forms::corrections(up.shown(&self.dialogs.ids)));
        }
        let mut corrections = forms::corrections(self.dialogs.hide());
        corrections.extend(self.resync_widget(roll));
        (None, corrections)
    }

    /// **The file chosen in the Open form, opened** by the rule a change of
    /// document follows; with none chosen the form stays up and says so.
    fn open_chosen(&mut self, paths: &[String], out: &mut Outcome) -> Option<String> {
        let Some(path) = paths.first() else {
            return Some("select a file to open".into());
        };
        crate::file::path_given(self, true, path, out)
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
        let glyphs = self.glyphs_owed();
        let mut drawn = {
            let held = self.held();
            props(
                &held,
                &self.domain,
                self.rate,
                self.editable,
                &self.editing_over(&held),
                &self.symbols,
            )
        };
        if let Some(glyphs) = glyphs {
            drawn.insert("glyphs".into(), glyphs.props["glyphs"].clone());
        }
        self.display.apply(&mut drawn);
        // the grid is an axis' own, as its tempo map is
        if let Some(Value::Object(x)) = drawn.get_mut("axes").and_then(|a| a.get_mut("x")) {
            x.insert("grid".into(), json!(self.grid));
            x.insert("grid_snap".into(), json!(self.snap));
        }
        if self.midi_in {
            drawn.insert("midi_in".into(), json!(1));
        }
        if self.tabbed() {
            drawn.insert("lane".into(), json!(self.lane_shown()));
            drawn.insert("lane_tabs".into(), self.lane_tabs());
        }
        let mut window = window(drawn, widget, &self.title, self.size);
        if !self.bare {
            use crate::chrome::{self, App};
            chrome::dress(
                &mut window,
                chrome::menu(App::Notes, self.asks),
                chrome::toolbar(App::Notes, None),
                widget,
            );
            self.compose_panel(&mut window);
            self.compose_display(&mut window);
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

    /// **The lane shown**: the one chosen, or the velocities where the curve
    /// chosen is gone.
    fn lane_shown(&self) -> String {
        let lanes = self.lanes_offered();
        if lanes.iter().any(|(name, _)| *name == self.lane) {
            self.lane.clone()
        } else {
            lanes
                .into_iter()
                .next()
                .map(|(name, _)| name)
                .unwrap_or_default()
        }
    }

    /// **The lanes offered as tabs**: the sequence's, but those the display
    /// takes off.
    fn lanes_offered(&self) -> Vec<(String, String)> {
        roll::lanes(&self.held())
            .into_iter()
            .filter(|(name, _)| self.display.shows_lane(name))
            .collect()
    }

    /// **Whether the roll shows one lane at a time, with its tabs**: a window
    /// composed whole, whose caller numbered its widgets; a roll alone draws
    /// its rows one under another.
    fn tabbed(&self) -> bool {
        !self.dialogs.ids.is_empty()
    }

    /// **The tabs the roll draws over its lane**, one per lane offered: flat
    /// `name label` pairs (the roll's `lane_tabs`).
    fn lane_tabs(&self) -> Value {
        self.lanes_offered()
            .into_iter()
            .flat_map(|(name, label)| [json!(name), json!(label)])
            .collect()
    }

    /// **A tab pressed on the roll** (`lane` and its name): the lane it names
    /// shown, recorded nowhere.
    fn lane_said(&mut self, values: &[Value]) -> (Option<String>, Vec<Correction>) {
        let name = values.first().and_then(Value::as_str).unwrap_or_default();
        if self.lanes_offered().iter().any(|(n, _)| n == name) {
            self.lane = name.into();
        }
        let roll = self.widget.map_or(0, i64::from);
        (None, self.resync_widget(roll))
    }

    /// **The display panel beside the roll**, when it is shown and the
    /// window numbered its widgets: how the roll draws what the sequence
    /// holds beyond its boxes ([`display`]).
    fn compose_display(&self, window: &mut Value) {
        if !self.display_panel {
            return;
        }
        let lanes = roll::lanes(&self.held());
        let notation = self.notation();
        let Some(column) = self.display.panel(&self.dialogs.ids, &lanes, notation) else {
            return;
        };
        let Some(children) = window.get_mut("children").and_then(Value::as_array_mut) else {
            return;
        };
        // the work, wherever the chrome and the other panel put it: the
        // child that takes the window's room
        if let Some(index) = children
            .iter()
            .position(|c| c["weight"].as_f64().is_some_and(|w| w > 0.0))
            .or_else(|| {
                let roll = self.widget.map(|w| json!(w));
                children
                    .iter()
                    .position(|c| roll.as_ref().is_some_and(|r| &c["id"] == r))
            })
        {
            let work = children.remove(index);
            children.insert(
                index,
                crate::panels::beside(work, column, crate::panels::Side::Right),
            );
        }
    }

    /// What the display panel's widgets are corrected with: none while it is
    /// hidden.
    fn display_corrections(&self) -> Vec<Correction> {
        if !self.display_panel {
            return Vec::new();
        }
        let lanes = roll::lanes(&self.held());
        let notation = self.notation();
        self.display
            .corrections(&self.dialogs.ids, &lanes, notation)
    }

    /// **The display changed as `request` says**: the roll drawn again as it
    /// now says, and the lanes' tabs and the panel with it, recorded nowhere.
    /// The refusal, if any.
    fn set_display(&mut self, request: &Value) -> (Option<String>, Vec<Correction>) {
        if let Err(why) = self.display.set(request) {
            return (Some(why), self.display_corrections());
        }
        let roll = self.widget.map_or(0, i64::from);
        (None, self.resync_widget(roll))
    }

    /// **A report from the display panel**, read as the change it asks.
    fn display_said(
        &mut self,
        name: &str,
        tag: &str,
        values: &[Value],
    ) -> (Option<String>, Vec<Correction>) {
        let value = values
            .first()
            .cloned()
            .or_else(|| tag.parse::<f64>().ok().map(Value::from));
        let lanes = roll::lanes(&self.held());
        let Some(request) = value.and_then(|v| display::Display::read(name, &v, &lanes)) else {
            return (None, Vec::new());
        };
        self.set_display(&request)
    }

    /// **The panel beside the roll**, when it is shown and the window
    /// numbered its widgets; the roll then tells what the hand marks.
    fn compose_panel(&self, window: &mut Value) {
        if !self.panel {
            return;
        }
        let held = self.held();
        let axis = Axis::of(&held, self.rate);
        let ids = &self.dialogs.ids;
        let Some(column) = panel::panel(
            &held,
            &self.domain,
            &axis,
            &self.editing_over(&held),
            &self.marked,
            ids,
        ) else {
            return;
        };
        drop(held);
        let Some(children) = window.get_mut("children").and_then(Value::as_array_mut) else {
            return;
        };
        // the roll, wherever the chrome put it
        let roll = self.widget.map(|w| json!(w));
        if let Some(index) = children
            .iter()
            .position(|c| roll.as_ref().is_some_and(|r| &c["id"] == r))
        {
            let mut work = children.remove(index);
            work["tell_marks"] = json!(1);
            children.insert(
                index,
                crate::panels::beside(work, column, crate::panels::Side::Right),
            );
        }
    }

    /// What the panel's widgets are corrected with: none while it is hidden.
    fn panel_corrections(&self) -> Vec<Correction> {
        if !self.panel {
            return Vec::new();
        }
        let held = self.held();
        let axis = Axis::of(&held, self.rate);
        panel::corrections(
            &held,
            &self.domain,
            &axis,
            &self.editing_over(&held),
            &self.marked,
            &self.dialogs.ids,
        )
    }

    /// **A report from the panel**: a value typed in the inspector, written
    /// on every marked note as the roll would write it, or rows chosen in the
    /// list, marked on the roll.
    fn panel_said(
        &mut self,
        name: &str,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let roll = self.widget.map_or(0, i64::from);
        if name == panel::LIST {
            if tag != "select" {
                return (None, Vec::new());
            }
            let rows: Vec<usize> = values
                .iter()
                .filter_map(Value::as_f64)
                .map(|v| v.max(0.0) as usize)
                .collect();
            let marked = panel::ids_of_rows(&self.held(), &rows);
            self.marked = marked;
            let mut corrections = vec![Correction {
                widget: roll,
                props: json!({ "selected": self.marked }),
            }];
            corrections.extend(self.panel_corrections());
            return (None, corrections);
        }
        if tag == "focus" {
            return (None, Vec::new());
        }
        let value = values
            .first()
            .and_then(Value::as_f64)
            .or_else(|| tag.parse::<f64>().ok());
        let Some(value) = value else {
            return (None, Vec::new());
        };
        let report = {
            let held = self.held();
            let axis = Axis::of(&held, self.rate);
            panel::report(
                &held,
                &self.domain,
                &axis,
                &self.editing_over(&held),
                &self.marked,
                name,
                value,
            )
        };
        let Some(report) = report else {
            return (Some("no note is selected".into()), self.panel_corrections());
        };
        self.gesture(roll, "notes", &report, out)
    }

    /// What `widget` is corrected with, or nothing for one that is not the
    /// roll.
    fn resync_widget(&self, widget: i64) -> Vec<Correction> {
        if self.widget.map(i64::from) != Some(widget) {
            return Vec::new();
        }
        let held = self.held();
        let editing = self.editing_over(&held);
        let mut props = correction(&held, &self.domain, self.rate, &editing, &self.symbols);
        self.display.apply(&mut props);
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
        // the panel reads the sequence again
        drop(held);
        if self.tabbed() {
            props.insert("lane".into(), json!(self.lane_shown()));
            props.insert("lane_tabs".into(), self.lane_tabs());
        }
        let mut corrections = vec![Correction {
            widget,
            props: Value::Object(props),
        }];
        corrections.extend(self.panel_corrections());
        corrections.extend(self.display_corrections());
        corrections
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
        if tag == "lane" {
            return self.lane_said(values);
        }
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
        if tag == "marked" {
            // what the hand marked on the roll: the panel follows it
            self.marked = values
                .iter()
                .filter_map(Value::as_f64)
                .map(|v| v.max(0.0) as u64)
                .collect();
            return (None, self.panel_corrections());
        }
        if roll::SELECTIONS.contains(&tag) {
            // what the hand marks is the window's: no edit, the roll's
            // selection corrected
            let ids: Vec<u64> = values
                .iter()
                .filter_map(Value::as_f64)
                .map(|v| v.max(0.0) as u64)
                .collect();
            let held = self.held();
            let chosen =
                roll::select_like(&held, tag, &ids, &self.domain, &self.editing_over(&held));
            drop(held);
            if chosen.is_empty() {
                return (Some("nothing is selected".into()), Vec::new());
            }
            let mut corrections = vec![Correction {
                widget,
                props: json!({ "selected": chosen }),
            }];
            self.marked = chosen;
            corrections.extend(self.panel_corrections());
            return (None, corrections);
        }
        if !matches!(
            tag,
            "notes"
                | "osc"
                | "points"
                | "join"
                | "unjoin"
                | "split"
                | "step_up"
                | "step_down"
                | "octave_up"
                | "octave_down"
        ) && !clausters_editing::notes::notation::ARTICULATIONS.contains(&tag)
            && !clausters_editing::notes::lanes::CURVE_VERBS.contains(&tag)
        {
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
            let editing = self.editing_over(&sequence);
            let intake = roll::intake(&sequence, tag, values, &axis, &self.domain, &editing);
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
                Ok(change) if change.applied => Some((
                    events::ids::describe(&change.renumbered),
                    Record {
                        label: intake.label,
                        legs: vec![Leg {
                            forward: json!({ "edit": payload }),
                            backward: backward.0,
                            key: events::coalesce_key(&Opaque(payload.clone())).unwrap_or_default(),
                            ..Leg::default()
                        }],
                    },
                )),
                Ok(_) => None,
                Err(why) => {
                    drop(sequence);
                    return (Some(why), self.resync_widget(widget));
                }
            }
        };
        let mut said = None;
        if let Some((renumbered, record)) = recorded {
            said = renumbered;
            out.record = Some(record);
            out.changed = true;
            out.version += 1;
        }
        // The roll learns the ids the sequence gave the notes the hand made.
        (said, self.resync_widget(widget))
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
        self.leaving = None;
    }

    fn asks_to_close(&self) -> bool {
        self.asks
    }

    fn unsaved(&self) -> bool {
        NotesEditor::unsaved(self)
    }

    fn ask_before(
        &mut self,
        leaving: crate::closing::Leaving,
        _out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        if !self.dialogs.numbered(dialogs::forms()) {
            return (
                Some(
                    "this window has changes that are not saved, and no form to ask about them"
                        .into(),
                ),
                Vec::new(),
            );
        }
        self.leaving = Some(leaving);
        (
            None,
            forms::corrections(
                self.dialogs
                    .open(dialogs::Form::Close, std::collections::BTreeMap::new()),
            ),
        )
    }

    fn change_now(&mut self, change: crate::closing::FileChange, out: &mut Outcome) {
        out.file = Some(change);
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
            if panel::names().contains(&name) {
                return self.panel_said(&name, tag, values, out);
            }
            if display::names().contains(&name) {
                return self.display_said(&name, tag, values);
            }
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
        // **The roll's file**, for a roll that has one
        if let Some((why, shown)) = self.file_verb(&message.tag.clone(), out) {
            out.turn = Kind::Route;
            out.answer = Some(conversation::answer(message.seq, out.version, why, shown));
            return true;
        }
        // **The snap-to-grid switch** (G, the View menu): the window's own
        // state, so nothing is recorded and the roll is told
        if MODE_VERBS.contains(&message.tag.as_str())
            || roll::CHANNEL_VERBS.contains(&message.tag.as_str())
        {
            let tag = message.tag.clone();
            out.turn = Kind::Route;
            let mut shown = Vec::new();
            let why = self.mode_verb(&tag, out, &mut shown);
            shown.extend(self.glyphs_owed());
            let roll = self.widget.map_or(0, i64::from);
            shown.extend(self.resync_widget(roll));
            out.answer = Some(conversation::answer(message.seq, out.version, why, shown));
            return true;
        }
        // **The panel shown or hidden**: the window is drawn again, with it
        // or without it
        if message.tag == PANEL_VERB || message.tag == DISPLAY_VERB {
            if message.tag == PANEL_VERB {
                self.panel = !self.panel;
            } else {
                self.display_panel = !self.display_panel;
            }
            out.turn = Kind::Route;
            out.restructure = true;
            out.answer = Some(conversation::answer(
                message.seq,
                out.version,
                None,
                Vec::new(),
            ));
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
///   `"staff"` with its `index`), `voice` (`[staff, voice]`), `channel` (the
///   channel the MIDI mode edits, from 0, or `null` for every one: answered
///   `{"refused": why}` where none is chosen, in notation mode and under
///   MPE), `color_by`
///   (`"voice"`, `"staff"` or `"part"`), `display` (a change of how the roll
///   draws, [`display::Display::set`]: answered `{"refused": why}` for a
///   value none of the settings takes, changing nothing), `domain`,
///   `title`, `w`, `h`: `{}`.
/// - `grid` -- `{"grid": beats, "snap": bool}`: the grid a note lands on, and
///   whether a hand snaps to it.
/// - `editing` -- how the roll edits ([`Editing`]): `{"monophonic", "mode",
///   "scope", "voice", "channel", "color_by"}`, `panel`, whether the panel is shown,
///   `display` and `display_panel`, how the roll draws what is beyond its boxes
///   ([`display`]) and whether its panel is shown, and `format`, what the
///   sequence is written as ([`events::Format::name`]). The mode is the
///   format's: no verb changes it, and a conversion makes a copy
///   ([`CONVERSIONS`]).
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
            let mut names = dialogs::names();
            names.extend(panel::names());
            names.extend(display::names());
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
        "dialogs" => {
            let mut names = dialogs::names();
            names.extend(panel::names());
            names.extend(display::names());
            json!({ "dialogs": names }).to_string()
        }
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
            if let Some(channel) = request.get("channel") {
                let channel = channel.as_f64().map(|c| c.clamp(0.0, 15.0) as u8);
                if let Some(why) = editor.choose_channel(channel) {
                    return json!({ "refused": why }).to_string();
                }
            }
            if let Some(lane) = request.get("lane").and_then(Value::as_str) {
                editor.lane = lane.into();
            }
            if let Some(display) = request.get("display")
                && let Err(why) = editor.display.set(display)
            {
                return json!({ "refused": why }).to_string();
            }
            if let Some(color_by) = request
                .get("color_by")
                .and_then(|c| serde_json::from_value::<ColorBy>(c.clone()).ok())
            {
                editor.editing.color_by = color_by;
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
            if let Some(asks) = request.get("asks").and_then(Value::as_bool) {
                editor.asks = asks;
            }
            if let Some(path) = request.get("path") {
                editor.path = path.as_str().map(str::to_string);
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
            let mut said = json!(editor.editing());
            said["format"] = json!(editor.held().format().name());
            said["panel"] = json!(editor.panel);
            said["lane"] = json!(editor.lane_shown());
            said["display"] = json!(editor.display);
            said["display_panel"] = json!(editor.display_panel);
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

crate::file::application!(NotesEditor);

impl crate::file::FileWindow for NotesEditor {
    fn binding(&self) -> i64 {
        self.binding
    }

    fn file_path(&self) -> Option<String> {
        self.path.clone()
    }

    fn has_document(&self) -> bool {
        self.document
    }

    fn no_file(&self) -> Option<&'static str> {
        (!self.asks).then_some(NO_FILE)
    }

    fn confirm_saved(&mut self, saved: &crate::file::Saved) {
        self.path = Some(saved.path.clone());
        if let Ok(sequence) = serde_json::from_slice(&saved.revision) {
            self.set_saved(sequence);
        }
    }

    fn replace_document(&mut self, contents: &[u8], path: Option<String>) -> Option<Value> {
        let left = self.left();
        *self.held() = serde_json::from_slice(contents).unwrap_or_default();
        self.document = true;
        self.path = path;
        self.start_over();
        left
    }

    fn let_go(&mut self) -> Option<Value> {
        let left = self.left();
        *self.held() = EventSequence::default();
        self.document = false;
        self.path = None;
        self.start_over();
        left
    }

    /// The file of the sequence's format ([`super::file::write`]), and the
    /// sequence itself, which the window compares itself with.
    fn write_file(&self) -> Result<(Vec<u8>, Vec<u8>), String> {
        let held = self.held();
        let contents = super::file::write(&held)?;
        let revision = serde_json::to_vec(&*held).map_err(|e| e.to_string())?;
        Ok((contents, revision))
    }

    fn read_file(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        let sequence = super::file::read(bytes, self.scores.0.as_ref())?;
        serde_json::to_vec(&sequence).map_err(|e| e.to_string())
    }

    /// An empty sequence written for MIDI 1.0.
    fn blank(&self) -> Result<Vec<u8>, String> {
        let blank = EventSequence {
            midi: Some(events::MidiSpec::Midi1),
            ..EventSequence::default()
        };
        serde_json::to_vec(&blank).map_err(|e| e.to_string())
    }

    fn leaving(&mut self) -> &mut Option<crate::closing::Leaving> {
        &mut self.leaving
    }

    fn ask_where(&mut self) -> (Option<String>, Vec<Correction>) {
        self.open_file_form(dialogs::Form::SaveAs)
    }
}

/// **What a conversion `verb` writes**, the MIDI spec and its name; `None`
/// for the conversion into notation, or a verb that is no conversion.
fn spec_of(verb: &str) -> Option<(events::MidiSpec, &'static str)> {
    Some(match verb {
        "convert_midi1" => (events::MidiSpec::Midi1, "MIDI 1.0"),
        "convert_mpe" => (
            events::MidiSpec::Mpe {
                upper: false,
                members: 15,
            },
            "MPE",
        ),
        "convert_midi2" => (events::MidiSpec::Midi2, "MIDI 2.0"),
        _ => return None,
    })
}

/// **Why conversion `verb` is refused over a sequence in `format`**: it is
/// written so already. `None` for a conversion that makes a copy in another
/// format, or a verb that is no conversion.
fn conversion_refused(verb: &str, format: events::Format) -> Option<String> {
    let same = match verb {
        "convert_notation" => format == events::Format::Notation,
        _ => spec_of(verb).is_some_and(|(spec, _)| {
            matches!(
                (format, spec),
                (
                    events::Format::Midi(events::MidiSpec::Midi1),
                    events::MidiSpec::Midi1
                ) | (
                    events::Format::Midi(events::MidiSpec::Mpe { .. }),
                    events::MidiSpec::Mpe { .. }
                ) | (
                    events::Format::Midi(events::MidiSpec::Midi2),
                    events::MidiSpec::Midi2
                )
            )
        }),
    };
    same.then(|| {
        let name = spec_of(verb).map_or("notation", |(_, name)| name);
        format!("the sequence is written as {name} already")
    })
}

#[cfg(test)]
mod tests;
