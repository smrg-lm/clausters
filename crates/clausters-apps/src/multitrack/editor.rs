//! **The multitrack editor's turns**: what one message from the host is, what
//! it does to the multitrack, and what the host is answered with.
//!
//! A host reports what a hand did and waits to be told what happened. The
//! editor reads the message ([`Conversation`]), reads the gesture in the
//! multitrack's vocabulary (the projection's intake), applies each edit with the
//! inverse read before it lands ([`domain::edit`]), keeps where the reader put
//! the position cursor, and answers -- an acknowledgement, the corrections a
//! refused or overtaken gesture needs, the reason when one is owed.
//!
//! # What it hands back rather than does
//!
//! The undo order is **not** here: each turn answers the entry to record
//! ([`Record`]) to the editing context the multitrack sits in, and a step of
//! that history comes back as payloads to [`MultitrackEditor::apply`]. The
//! version the host names back is the same history's counter, so it comes in
//! with every turn and goes out moved.
//!
//! What a running system does is handed back too: a source an edit minted is
//! made where the samples are, and a placed cursor cues a transport.
//! [`Outcome`] names each, and the caller carries them out.
//!
//! **A box of samples is not entered to be edited.** The multitrack edits
//! non-destructively -- where things are, never the samples they read -- so
//! nothing here opens an editor over a take: an audio editor is another
//! application, with a history of its own. **A box of notes is the one
//! exception**: a double click on one asks for the roll over the sequence it
//! reads ([`Outcome::open`]), since the roll and the box edit the same notes
//! and the edit is one history.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use clausters_core::tempoclock::samples_to_secs;
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::edit::{MULTITRACK, MultitrackIntent};
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::{Opaque, SourceId, domain};
use clausters_editing::conversation::{self, Answer, Conversation, Correction};
use clausters_editing::multitrack::{self as projection, Look};

use super::{Meter, Transport, TransportIds, Window, dialogs};
use crate::turn::{self, Converse, number};

pub use crate::turn::{Event, Kind, Leg, Record};

/// **What one turn came to.**
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// What kind of turn the message was.
    pub turn: Kind,
    /// What to send the host, `silent` for nothing.
    pub answer: Option<Answer>,
    /// The stamp a [`Kind::Step`] is answered with.
    pub seq: i64,
    /// Whether a [`Kind::Step`] walks forward.
    pub redo: bool,
    /// The entry to record, when the turn edited something recordable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<Record>,
    /// Whether the multitrack changed.
    pub changed: bool,
    /// The version after the turn.
    pub version: i64,
    /// The multitrack as it now stands, when it changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multitrack: Option<Value>,
    /// The sources an edit minted, in the order the edits named them.
    pub minted: Vec<Value>,
    /// Where the position cursor was placed, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locate: Option<f64>,
    /// A hand on the control of a parameter a curve drives: the track, the
    /// parameter, and the value it holds, or `None` as it lets go.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_hold: Option<(u64, String, Option<f32>)>,
    /// The selection a sweep left, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Value>,
    /// **The selection was redrawn**: a loop in progress repeats over it --
    /// or over everything, with none -- and the head goes into it when the
    /// sweep is let go (`true`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow: Option<bool>,
    /// What the transport is asked to do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportVerb>,
    /// Where the position cursor now is, in seconds, when the turn moved it by a
    /// verb of its own rather than by a hand on the ruler (which is `locate`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<f64>,
    /// **The source whose roll a double click asked for**: a box over a bound
    /// sequence, which the caller opens a notes editor over, in this editor's
    /// context. The one box the multitrack opens: it edits no take.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<u64>,
    /// **Save before closing**: the close form's Save. The holder writes
    /// the session where it saves it, and closes the window once it is
    /// written -- a save that fails keeps the window.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub save: bool,
    /// **Save under this path**: the Save as form's `OK`. The holder writes
    /// the session there, and saves there from then on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_as: Option<String>,
    /// Captured session and destination after storage confirmation.
    #[serde(skip)]
    pub saved: Option<Box<super::operations::Saved>>,
    /// **The session the window let go of** in this turn, whose holder is
    /// handed what the window left it as.
    #[serde(skip)]
    pub released: Option<Box<Released>>,
    /// **Whether to close the window** ([`crate::closing`]): a close asked of
    /// it, with nothing to lose or nothing to ask in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
    /// **The window's session to change** (File > New, Open, Close file),
    /// with nothing to lose or once the close form was answered. With
    /// [`save`](Self::save) the change waits for the session to be written.
    #[serde(skip)]
    pub file: Option<FileChange>,
}

pub use crate::closing::{FileChange, Leaving};

turn::turned!(Outcome);

impl Converse for MultitrackEditor {
    type Outcome = Outcome;

    fn conversation(&mut self) -> &mut Conversation {
        &mut self.conversation
    }

    fn window_id(&self) -> Option<i32> {
        self.window
    }

    fn closed(&mut self) {
        self.window = None;
    }

    fn owns(&self, widget: i64, tag: &str) -> bool {
        i32::try_from(widget).is_ok_and(|w| self.answers(w, tag))
            || self.close_form.is_some_and(|ids| ids.contains(widget))
            || self.dialogs.name(widget).is_some()
    }

    fn asks_to_close(&self) -> bool {
        self.asks
    }

    fn unsaved(&self) -> bool {
        self.unsaved && self.has_session()
    }

    fn ask_before(
        &mut self,
        leaving: Leaving,
        _out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        self.ask_with_form(leaving)
    }

    fn change_now(&mut self, change: FileChange, out: &mut Outcome) {
        out.file = Some(change);
    }

    fn resync(&mut self, widget: i64) -> Vec<Correction> {
        MultitrackEditor::resync(self, widget)
    }

    /// **The window's File > New, Open and Close file**: its session
    /// changes, asking first when that loses work. They name no data a
    /// gesture was made against, so they are never stale: a menu picked
    /// after a script's edit is not refused as overtaken.
    fn window_verb(
        &mut self,
        message: &conversation::Message,
        _args: &[Value],
        out: &mut Outcome,
    ) -> bool {
        if message.addr != "/gui_event"
            || !message.is_window
            || !matches!(
                message.tag.as_str(),
                NEW_FILE_VERB | OPEN_FILE_VERB | CLOSE_FILE_VERB
            )
        {
            return false;
        }
        out.turn = Kind::Route;
        let (reason, corrections) = match message.tag.as_str() {
            NEW_FILE_VERB => crate::closing::change_file(self, FileChange::New, out),
            OPEN_FILE_VERB => self.open_session_form(),
            _ if !self.has_session() => (Some("no file is open".into()), Vec::new()),
            _ => crate::closing::change_file(self, FileChange::Close, out),
        };
        out.answer = Some(conversation::answer(
            message.seq,
            out.version,
            reason,
            corrections,
        ));
        true
    }

    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        // **The close form's answer**: Save writes and then does what the
        // form was up for (closes the window, changes its session), Don't
        // save does it now, Cancel keeps the window as it is -- and the form
        // goes down
        if let Some(ids) = self.close_form.filter(|ids| ids.contains(widget)) {
            use crate::closing::Choice;
            let Some(choice) = ids.read(widget, tag) else {
                return (None, Vec::new());
            };
            let leaving = self.leaving.take().unwrap_or(Leaving::Window);
            if choice != Choice::Cancel {
                out.save = choice == Choice::Save;
                match leaving {
                    Leaving::Window => out.close = true,
                    Leaving::File(change) => out.file = Some(change),
                }
            }
            return (None, crate::closing::shown(&ids, false));
        }
        if let Some(name) = self.dialogs.name(widget).map(str::to_string) {
            return self.form_event(&name, tag, values, out);
        }
        self.gesture(widget, tag, values, out)
    }
}

/// **What a turn asks the transport to do.** The editor decides what a button,
/// the space bar and a rewind mean; the caller sends the steps its playback
/// answers for the verb.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "verb", rename_all = "camelCase")]
pub enum TransportVerb {
    /// Play, or pause where it stands -- whichever the transport is not doing.
    Toggle,
    /// **The space bar**: play, or -- when it is rolling -- stop and go back
    /// to the mark, so the play cursor lands on the position cursor again. A
    /// play is the audio editor's pass: from the time range a sweep left to its
    /// end, or from the mark, and with the loop switch over the range or the
    /// whole multitrack.
    PlayStop {
        /// The mark, in seconds.
        mark: f64,
        /// The time range a sweep left, `[start, end]` in seconds.
        range: Option<(f64, f64)>,
        /// Whether the loop switch is on.
        looping: bool,
    },
    /// **The loop switch changed** (`L`): a pass in progress now loops over
    /// the time range a sweep left or the whole multitrack, or goes on to its
    /// end -- from where it stands. Stopped, nothing: the next play reads it.
    Loop {
        /// The time range a sweep left, `[start, end]` in seconds.
        range: Option<(f64, f64)>,
        /// Whether the loop switch is on.
        looping: bool,
    },
    /// Halt and go back to the mark: the position cursor, not the top.
    Stop {
        /// The mark, in seconds.
        mark: f64,
    },
    /// **The end switch changed** (the window's `stop_at_end`): a pass that
    /// rolled on past the contents now stops where they end, and one that
    /// stopped there rolls on. The caller flips its playback's end -- open,
    /// or the contents -- which is where the switch is kept; the editor holds
    /// no copy of it.
    StopAtEnd,
    /// Cue a stopped transport at `secs`, and leave a rolling one alone.
    Cue {
        /// Where, in seconds.
        secs: f64,
    },
}

/// What a multitrack editor is put back to when a step it took part in is
/// refused elsewhere ([`MultitrackEditor::before_step`]).
pub(crate) struct Kept(
    clausters_document::multitrack::Multitrack,
    HashMap<SourceId, Vec<clausters_document::session::Part>>,
);

/// What one payload of a history step did to the multitrack.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Applied {
    /// Whether anything moved.
    pub applied: bool,
    /// The source it minted, if it minted one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minted: Option<Value>,
    /// The multitrack as it now stands, when it moved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multitrack: Option<Value>,
}

/// **The multitrack editor**: a multitrack, the window it is drawn in, and one
/// view's end of the conversation with the host.
#[derive(Clone, Debug)]
pub struct MultitrackEditor {
    rate: f64,
    session: SessionState,
    next_session: i64,
    session_open: bool,
    displayed_session: Option<i64>,
    binding_generation: u64,
    /// **The checkpoint's generation**: moved by each [`set_checkpoint`],
    /// for [`unsaved_at`] to tell a new checkpoint from the one it read.
    ///
    /// [`set_checkpoint`]: Self::set_checkpoint
    /// [`unsaved_at`]: Self::unsaved_at
    checkpoint: u64,
    /// What [`unsaved_at`](Self::unsaved_at) last answered, and what it
    /// read it at.
    dirty: std::cell::Cell<Option<(DirtyKey, bool)>>,
    meters: Vec<Meter>,
    link: Option<i64>,
    transport: Transport,
    title: String,
    size: (i64, i64),
    cursor: Option<f64>,
    /// The time range a sweep left, `[start, end)` in seconds, while there is
    /// one: what the space bar plays.
    range: Option<(f64, f64)>,
    window: Option<i32>,
    widget: Option<i32>,
    ruler: Option<i32>,
    conversation: Conversation,
    /// What the host was last told the rows, the boxes and the curves are
    /// called.
    told: Option<[HashSet<String>; 3]>,
    /// The transport row's ids, once they are known: numbered here, or learned
    /// by name after the window opened.
    controls: Option<TransportIds>,
    /// **The loop switch**, as the host last said it: the host keeps it, and the
    /// window's menu entry reads it here.
    looping: bool,
    /// **The close form's widgets**, when the holder numbered them
    /// ([`crate::closing`]); and whether it is up.
    close_form: Option<crate::closing::Ids>,
    /// Whether the window is the work's one holder, so a close with
    /// something unsaved asks first -- a standalone host's, never a client's.
    asks: bool,
    /// What the close form is up for, while it is.
    leaving: Option<Leaving>,
    /// Whether the holder's work has changes its file does not hold, as the
    /// holder last said: the session is the holder's, not this editor's.
    unsaved: bool,
    /// **Whether the window saves its session** ([`Window::saves`]), as the
    /// holder said: not one it opened read-only.
    saves: bool,
    /// **Whether the session has no file yet**, as the holder last said: a
    /// Save then asks where, in the Save as form.
    untitled: bool,
    /// The window's dialogs ([`dialogs`]): the Save as form.
    dialogs: dialogs::Dialogs,
    /// Whether the window is composed with no chrome ([`Window::bare`]).
    bare: bool,
    /// **The grid a box lands on, in beats** ([`crate::DEFAULT_GRID`] until
    /// somebody chooses one): what a drag snaps to and a quantize writes into
    /// the multitrack, read through its tempo map. `0` for none.
    grid: f64,
    /// **Whether a hand snaps to that grid** -- the snap-to-grid switch, on
    /// until somebody turns it off: off, a drag, a paste and a split land
    /// where the hand puts them, and a quantize still moves onto the grid.
    snap: bool,
    /// **How a box of samples is drawn**: `"trace"`, its waveform, or
    /// `"spectrogram"` -- the audio editor's view of its take, which draws
    /// either. The window's own, like its zoom.
    samples_view: &'static str,
    /// **What the window's time ruler counts in** ([`crate::ruler`]): the
    /// window's own, bars and beats until another is chosen.
    ruler_unit: crate::ruler::Unit,
}

/// The file facts of one session, retained with its contents and history.
#[derive(Clone, Debug, Default)]
pub(crate) struct SessionFile {
    pub session: Option<clausters_document::session::Session>,
    pub path: Option<String>,
    pub save_to: Option<Option<String>>,
    pub saved: Option<clausters_document::session::Session>,
}

/// What an unsaved reading was read at ([`MultitrackEditor::unsaved_at`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirtyKey {
    version: i64,
    multitrack: u64,
    session: i64,
    checkpoint: u64,
}

/// **A session the window let go of**: its identity, whose order closes,
/// and what the window left it as, which its holder is handed.
#[derive(Clone, Debug, PartialEq)]
pub struct Released {
    /// The session's identity in the window.
    pub id: i64,
    /// What the window left it as.
    pub snapshot: super::operations::Snapshot,
}

/// Contents and resolved sources of the session a window holds.
#[derive(Clone, Debug)]
pub(crate) struct SessionState {
    file: SessionFile,
    /// **The multitrack, shared with whoever holds it** -- a script's handle
    /// over it edits what this editor draws, and a turn here is what that
    /// handle reads.
    multitrack: super::Shared,
    sources: HashMap<SourceId, i64>,
    /// How many frames each take holds, where the caller said: what refuses a
    /// join over a box that reads past the end of its take.
    lengths: HashMap<SourceId, u64>,
    /// **The segments each join this editor knows is made of**: the ones it
    /// minted, and the ones a caller opened a session with. Read-only objects
    /// -- a join is replaced, never edited -- so they are kept as they were
    /// made, and a later join over a box that windows one reads through them.
    segments: HashMap<SourceId, Vec<clausters_document::session::Part>>,
    /// **The event sequences its notes regions read**, shared with whoever
    /// holds them -- a notes editor opened on one edits what these boxes draw.
    sequences: HashMap<SourceId, crate::notes::Shared>,
    /// **What each take is on disk**, where the application that holds the
    /// editor resolved it: the file a track's reader reads its regions from.
    takes: HashMap<SourceId, clausters_document::parts::TakeFile>,
    // A session replacement has its own identity even when it reuses source
    // ids and file paths. Measurement replies name the original binding.
    id: i64,
}

impl SessionState {
    fn new(id: i64, multitrack: super::Shared) -> Self {
        Self {
            id,
            multitrack,
            file: SessionFile::default(),
            sources: HashMap::new(),
            lengths: HashMap::new(),
            segments: HashMap::new(),
            sequences: HashMap::new(),
            takes: HashMap::new(),
        }
    }

    fn snapshot(&self) -> clausters_document::session::Session {
        use clausters_document::session::{Location, Session, Source};
        use clausters_document::{Document, Lifetime};
        let mut session = self
            .file
            .session
            .clone()
            .unwrap_or_else(|| Session::new(Document::empty()));
        session.multitrack = self
            .multitrack
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for (id, file) in &self.takes {
            session.sources.entry(*id).or_insert_with(|| {
                Source::file(file.path.to_string(), Lifetime::Session).shaped(
                    file.channels,
                    file.frames,
                    file.rate,
                )
            });
        }
        for (id, shared) in &self.sequences {
            let sequence = Box::new(shared.lock().unwrap_or_else(|e| e.into_inner()).clone());
            if let Some(source) = session.sources.get_mut(id) {
                source.location = Location::Events { sequence };
            } else {
                let mut source = Source::file("", Lifetime::Session);
                source.location = Location::Events { sequence };
                session.sources.insert(*id, source);
            }
        }
        for source in session
            .multitrack
            .regions()
            .filter_map(|region| region.content.source())
        {
            if let Some(parts) = self.segments.get(&source) {
                session.sources.entry(source).or_insert_with(|| {
                    let mut entry = Source::file("", Lifetime::Session);
                    entry.location = Location::Segments {
                        parts: parts.clone(),
                    };
                    entry
                });
            }
        }
        session
    }

    fn file_snapshot(&self) -> super::operations::Snapshot {
        super::operations::Snapshot {
            session: self.snapshot(),
            path: self.file.path.clone(),
            saved: self.file.saved.clone(),
        }
    }
}

impl MultitrackEditor {
    /// An editor over `multitrack`, drawn on an axis of `rate` frames a second, whose
    /// history is at `version`.
    pub fn new(multitrack: Multitrack, rate: f64, version: i64) -> Self {
        Self::over(super::shared(multitrack), rate, version)
    }

    /// An editor over a multitrack it shares with whoever handed it: what a
    /// change through that holder makes is what this editor draws next.
    pub fn over(multitrack: super::Shared, rate: f64, version: i64) -> Self {
        Self {
            rate,
            session: SessionState::new(0, multitrack),
            next_session: 1,
            session_open: true,
            displayed_session: Some(0),
            binding_generation: 0,
            checkpoint: 0,
            dirty: std::cell::Cell::new(None),
            meters: Vec::new(),
            link: None,
            transport: Transport::Absent,
            title: String::new(),
            size: (0, 0),
            cursor: None,
            range: None,
            window: None,
            widget: None,
            ruler: None,
            conversation: Conversation::new(version),
            told: None,
            controls: None,
            looping: false,
            close_form: None,
            asks: false,
            leaving: None,
            unsaved: false,
            saves: false,
            untitled: false,
            dialogs: dialogs::Dialogs::default(),
            bare: false,
            grid: crate::DEFAULT_GRID,
            snap: true,
            samples_view: WAVE,
            ruler_unit: crate::ruler::Unit::Beats,
        }
    }

    /// **The window's actions as they read now**, by verb: `undo` and `redo`
    /// as the context's history reads, the snap switch, the automatic
    /// crossfade, how a box of samples is drawn, and where a pass ends
    /// (`stops_at_end`, the transport's, which its holder reads).
    #[must_use]
    pub fn actions(
        &self,
        undo: bool,
        redo: bool,
        stops_at_end: Option<bool>,
    ) -> std::collections::BTreeMap<&'static str, clausters_editing::verbs::ActionState> {
        let crossfade = self.multitrack().defaults.crossfade;
        // with no session, what refuses a verb is that nothing is open, not
        // what the history reads: the first reason given is the one kept
        let open = self.has_session();
        let mut actions = crate::chrome::grid_actions(
            crate::chrome::App::Multitrack,
            self.saves,
            undo || !open,
            redo || !open,
            &|verb| match verb {
                crate::notes::editor::SNAP_VERB => Some(self.snap),
                CROSSFADE_VERB => Some(crossfade),
                STOP_AT_END_VERB => stops_at_end,
                LOOP_KEY => Some(self.looping),
                VIEW_WAVE_VERB => Some(self.samples_view == WAVE),
                VIEW_SPECTROGRAM_VERB => Some(self.samples_view == SPECTROGRAM),
                _ => self.ruler_unit.chosen_by(verb),
            },
        );
        if !self.has_session() {
            for (verb, state) in &mut actions {
                if !matches!(*verb, "close" | "keys" | "new_file" | "open_file") {
                    state.refuse("no file is open");
                }
            }
        }
        actions
    }

    /// **A window with no chrome** -- no menu bar, no tools -- for a client
    /// that composes its own around it. Said before [`window`](Self::window).
    pub fn set_bare(&mut self, bare: bool) {
        self.bare = bare;
    }

    /// **The close form's widgets**, numbered by the holder: the window then
    /// holds the form, to ask in before a close lets unsaved work go.
    /// Said before [`window`](Self::window).
    pub fn set_close_form(&mut self, ids: Option<crate::closing::Ids>) {
        self.close_form = ids;
    }

    /// **The window is the work's one holder** -- a standalone host's -- and
    /// asks before a close lets unsaved work go ([`crate::closing`]). Said
    /// before [`window`](Self::window).
    pub fn set_asks_to_close(&mut self, asks: bool) {
        self.asks = asks;
    }

    /// Whether the holder's work has changes its file does not hold: what a
    /// close asks about. The holder says so before each turn.
    pub fn set_unsaved(&mut self, unsaved: bool) {
        self.unsaved = unsaved;
    }

    /// **Whether the window saves its session itself**: File has Save and
    /// Save as. Said before [`window`](Self::window).
    pub fn set_saves(&mut self, saves: bool) {
        self.saves = saves;
    }

    /// Whether the session has no file yet: what a Save asks about. The
    /// holder says so before each turn.
    pub fn set_untitled(&mut self, untitled: bool) {
        self.untitled = untitled;
    }

    /// **The dialogs' widgets, by name** ([`dialogs::names`]), numbered by the
    /// holder: the window then holds the Save as form. Said before
    /// [`window`](Self::window).
    pub fn set_dialogs(&mut self, ids: crate::forms::Ids) {
        self.dialogs.ids = ids;
    }

    /// **A save from the window**: where the session saves, or -- for one
    /// with no file yet, in a window with dialogs -- the Save as form, which
    /// asks where.
    fn save_from_window(&mut self, out: &mut Outcome) -> (Option<String>, Vec<Correction>) {
        if !self.saves {
            return (Some("this window writes no file".into()), Vec::new());
        }
        if self.untitled && self.dialogs.numbered(dialogs::forms()) {
            return self.open_save_as();
        }
        out.save = true;
        (None, Vec::new())
    }

    /// **The Save as form, up**, its field holding nothing: a session is
    /// named by where it is written.
    fn open_save_as(&mut self) -> (Option<String>, Vec<Correction>) {
        if !self.saves {
            return (Some("this window writes no file".into()), Vec::new());
        }
        if !self.dialogs.numbered(dialogs::forms()) {
            return (Some("this window has no dialogs".into()), Vec::new());
        }
        let shown = self.dialogs.open(dialogs::Form::SaveAs, Default::default());
        (None, crate::forms::corrections(shown))
    }

    /// **Whether `event` is said by one of the window's forms**: its
    /// dialogs or its close form, which answer with no session open.
    pub(crate) fn holds_form(&self, event: &Event) -> bool {
        let Some(widget) = event.args.first().and_then(Value::as_i64) else {
            return false;
        };
        self.close_form.is_some_and(|ids| ids.contains(widget))
            || self.dialogs.name(widget).is_some()
    }

    /// **The close form, up for `leaving`**, or why it cannot be: a window
    /// with no form to ask in stays as it is.
    fn ask_with_form(&mut self, leaving: Leaving) -> (Option<String>, Vec<Correction>) {
        let Some(ids) = self.close_form else {
            return (
                Some(
                    "this window has changes that are not saved, and no form to ask about them"
                        .into(),
                ),
                Vec::new(),
            );
        };
        self.leaving = Some(leaving);
        (None, crate::closing::shown(&ids, true))
    }

    /// **The Open form, up**, its chooser where the hand last left it.
    fn open_session_form(&mut self) -> (Option<String>, Vec<Correction>) {
        if !self.dialogs.numbered(dialogs::forms()) {
            return (Some("this window has no dialogs".into()), Vec::new());
        }
        let shown = self.dialogs.open(dialogs::Form::Open, Default::default());
        (None, crate::forms::corrections(shown))
    }

    /// **What a widget of the dialogs said**: a field typed into, a
    /// chooser's selection or the directory it went into, the form
    /// cancelled, or `OK` -- the path the session is saved under, which the
    /// outcome names for the holder to write, or the session to open, which
    /// is opened as File > Open opens it. A pick in the chooser (a double
    /// click or Enter on a file) opens it as `OK` does. A form refused
    /// stays up.
    fn form_event(
        &mut self,
        name: &str,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        use crate::forms::Said;
        let Some(said) = self.dialogs.read(name, tag, values) else {
            return (None, Vec::new());
        };
        let Some(open) = self.dialogs.up().cloned() else {
            return (None, Vec::new());
        };
        match said {
            Said::Cancel | Said::Discard => (None, crate::forms::corrections(self.dialogs.hide())),
            Said::Picked(_, paths) if open.form == dialogs::Form::Open => {
                self.open_chosen(&paths, out)
            }
            Said::Accept if open.form == dialogs::Form::Open => {
                let paths = self.dialogs.chosen("path").to_vec();
                self.open_chosen(&paths, out)
            }
            Said::Accept => {
                let path = open.value("path").trim().to_string();
                if path.is_empty() {
                    return (Some("a session is saved under a path".into()), Vec::new());
                }
                out.save_as = Some(path);
                (None, crate::forms::corrections(self.dialogs.hide()))
            }
            _ => (None, Vec::new()),
        }
    }

    /// **The session chosen in the Open form, opened**: the form goes down
    /// and the window's session changes, asking first when it has unsaved
    /// work. With none chosen the form stays up and says so.
    fn open_chosen(
        &mut self,
        paths: &[String],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        let Some(path) = paths.first().cloned() else {
            return (Some("select a session to open".into()), Vec::new());
        };
        let mut corrections = crate::forms::corrections(self.dialogs.hide());
        let (reason, asked) = crate::closing::change_file(self, FileChange::Open(path), out);
        corrections.extend(asked);
        (reason, corrections)
    }

    /// The window's own settings: the navigation group, the transport row, the
    /// title and the size.
    pub fn chrome(
        &mut self,
        link: Option<i64>,
        transport: Transport,
        title: &str,
        size: (i64, i64),
    ) {
        self.link = link;
        self.transport = transport;
        if let Transport::Numbered(ids) = transport {
            self.controls = Some(ids);
        }
        self.title = title.to_string();
        self.size = size;
    }

    /// The transport row's ids, learned after the window opened -- what a client
    /// that numbers id-less widgets on the way out hands back, or a host that
    /// numbered them for the window it draws again: a row that was drawn
    /// unnumbered is drawn with them from then on.
    pub fn set_controls(&mut self, ids: Option<TransportIds>) {
        self.controls = ids;
        if let (Some(ids), Transport::Unnumbered) = (ids, self.transport) {
            self.transport = Transport::Numbered(ids);
        }
    }

    /// The transport row's ids, once they are known.
    pub fn controls(&self) -> Option<TransportIds> {
        self.controls
    }

    /// The multitrack, held while the guard lives.
    pub fn multitrack(&self) -> std::sync::MutexGuard<'_, Multitrack> {
        self.session
            .multitrack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The multitrack as it is shared: what a handle opened over this editor's
    /// structure holds.
    pub fn shared(&self) -> super::Shared {
        self.session.multitrack.clone()
    }

    /// Replaces the multitrack -- a caller that edited it by a route that was not a
    /// turn of this editor.
    pub fn set_multitrack(&mut self, multitrack: Multitrack) {
        *self.multitrack() = multitrack;
    }

    /// File facts retained with the active session.
    pub(crate) fn file(&self) -> &SessionFile {
        &self.session.file
    }
    pub(crate) fn file_mut(&mut self) -> &mut SessionFile {
        &mut self.session.file
    }

    /// Capture the current multitrack, source table and shared sequences.
    pub(crate) fn session_snapshot(&self) -> clausters_document::session::Session {
        self.session.snapshot()
    }

    pub(crate) fn file_snapshot(&self) -> super::operations::Snapshot {
        self.session.file_snapshot()
    }

    /// **The session's confirmed checkpoint**: what it was last opened or
    /// saved as, or none.
    pub(crate) fn set_checkpoint(&mut self, saved: Option<clausters_document::session::Session>) {
        self.session.file.saved = saved;
        self.checkpoint += 1;
    }

    /// **Whether there is unsaved work**, read at the context's `version`:
    /// [`unsaved_contents`] again only when something it reads may have
    /// moved (the context, the multitrack, the session, the checkpoint),
    /// and its last answer otherwise. A window asks at every event, and
    /// building the session to compare it is not a pointer's cost.
    ///
    /// [`unsaved_contents`]: Self::unsaved_contents
    pub(crate) fn unsaved_at(&self, version: i64) -> bool {
        let key = DirtyKey {
            version,
            multitrack: self.multitrack().version,
            session: self.session.id,
            checkpoint: self.checkpoint,
        };
        if let Some((read, dirty)) = self.dirty.get()
            && read == key
        {
            return dirty;
        }
        let dirty = self.unsaved_contents();
        self.dirty.set(Some((key, dirty)));
        dirty
    }

    /// Whether contents differ from the confirmed checkpoint. Revision
    /// counters and event-id watermarks advance through undo as well; they
    /// invalidate readers but do not make restored contents dirty.
    pub(crate) fn unsaved_contents(&self) -> bool {
        let Some(saved) = &self.file().saved else {
            return false;
        };
        let mut current = self.session_snapshot();
        current.multitrack.version = saved.multitrack.version;
        current.document.version = saved.document.version;
        for (id, source) in &mut current.sources {
            use clausters_document::session::Location;
            if let Location::Events { sequence } = &mut source.location
                && let Some(clausters_document::session::Source {
                    location: Location::Events { sequence: previous },
                    ..
                }) = saved.sources.get(id)
            {
                sequence.next_id = previous.next_id;
            }
        }
        current != *saved
    }

    /// **What the client that opened the window is handed when it goes**:
    /// the session the window shows, or `null` with none.
    pub(crate) fn closing_snapshot(&self) -> Value {
        if self.has_session() {
            serde_json::to_value(self.file_snapshot()).unwrap_or(Value::Null)
        } else {
            Value::Null
        }
    }

    /// The active session binding, independent of the conversation version.
    pub(crate) fn session_binding(&self) -> i64 {
        self.session.id
    }

    /// Retire pending results without freeing the retained multitrack.
    pub(crate) fn invalidate_session_binding(&mut self) {
        self.displayed_session = None;
        self.binding_generation += 1;
    }

    pub(crate) fn binding_generation(&self) -> u64 {
        self.binding_generation
    }

    /// **A new session in the window**, with an identity of its own: the
    /// one it showed, if any, is let go and answered, for its order to be
    /// closed and its holder to be handed what the window left it as. One
    /// session file per window: nothing keeps the one let go.
    pub(crate) fn replace_session_binding(&mut self) -> Option<Released> {
        // where the one it replaces saved is that one's: a new session
        // saves by its own path, or asks for one
        let next = SessionState::new(self.next_session, super::shared(Multitrack::new()));
        self.next_session += 1;
        let previous = std::mem::replace(&mut self.session, next);
        let released = self.session_open.then(|| Released {
            id: previous.id,
            snapshot: previous.file_snapshot(),
        });
        self.session_open = true;
        self.binding_generation += 1;
        self.displayed_session = Some(self.session.id);
        released
    }

    /// The title its window was opened with.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Whether this window has a session, including a new empty session.
    pub fn has_session(&self) -> bool {
        self.session_open
    }

    /// Whether object work belongs to the session this window displays.
    pub(crate) fn observes_session(&self) -> bool {
        self.session_open && self.displayed_session == Some(self.session.id)
    }

    /// **The window left with no session**: the one it showed is let go,
    /// as [`replace_session_binding`] lets it go.
    ///
    /// [`replace_session_binding`]: Self::replace_session_binding
    pub(crate) fn unbind_session(&mut self) -> Option<Released> {
        if !self.session_open {
            return None;
        }
        let released = self.replace_session_binding();
        self.session_open = false;
        self.displayed_session = None;
        self.cursor = None;
        self.range = None;
        self.told = None;
        released
    }

    /// A transition invalidates content events drawn for the previous binding.
    pub(crate) fn binding_changed(&mut self, version: i64) {
        self.conversation.raise_floor(version);
        self.cursor = None;
        self.range = None;
        self.told = None;
    }

    /// **Work on session `id`**, when it is the one the window holds: a
    /// history leg or a reply naming a session the window let go of
    /// reaches nothing.
    pub(crate) fn over_session<R>(
        &mut self,
        id: i64,
        apply: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        (self.session_open && self.session.id == id).then(|| apply(self))
    }

    /// **The joins a caller holds**, by source -- the segments each is made
    /// of, added to the ones this editor minted itself: what a standalone host
    /// opening a session with joins in it hands over.
    pub fn set_segments(
        &mut self,
        segments: HashMap<SourceId, Vec<clausters_document::session::Part>>,
    ) {
        for id in segments.keys() {
            self.number_join(*id);
        }
        self.session.segments.extend(segments);
    }

    /// **Names a join by its own id**, as a take is named, where the
    /// application draws the takes from their files: its boxes name it by
    /// that number, and the window draws it from its list ([`Self::lists`]).
    fn number_join(&mut self, id: SourceId) {
        if !self.session.takes.is_empty() {
            self.session.sources.entry(id).or_insert(id.0 as i64);
        }
    }

    /// **Remembers the segments a minted join is made of**, so a join over a
    /// box that windows it reads through to its takes.
    fn learn(&mut self, minted: &Value) {
        let Ok(made) = serde_json::from_value::<clausters_document::multitrack::edit::MintedSource>(
            minted.clone(),
        ) else {
            return;
        };
        if let clausters_document::session::Location::Segments { parts } = made.source.location {
            self.number_join(made.id);
            self.session.segments.insert(made.id, parts);
        }
    }

    /// **The joins an edit names**: the sources `payload` makes something read
    /// that are joins this editor knows. What a leg holds, so a join stays
    /// while an undo or a redo can still name it and comes back to be freed
    /// once none can. A take is not one of them: it is the session's, loaded
    /// from a file, and outlives every edit over it.
    pub(crate) fn joins_named(&self, payload: &Value) -> Vec<u64> {
        clausters_document::multitrack::edit::intent_of(&Opaque(payload.clone()))
            .map(|intent| self.joins_in_intent(&intent))
            .unwrap_or_default()
    }

    /// Joins held by a decoded intent, before it becomes a history payload.
    pub(crate) fn joins_in_intent(&self, intent: &MultitrackIntent) -> Vec<u64> {
        intent
            .sources()
            .into_iter()
            .filter(|source| self.session.segments.contains_key(source))
            .map(|source| source.0)
            .collect()
    }

    /// Whether `source` is a join this editor knows: one it minted, or one a
    /// session it was opened with held.
    pub fn is_join(&self, source: SourceId) -> bool {
        self.session.segments.contains_key(&source)
    }

    /// **The joins the multitrack reads now**: what a released join is checked
    /// against before it is freed, since a region still windowing it is a
    /// root the history does not know about.
    pub fn joins_read(&self) -> Vec<SourceId> {
        self.multitrack()
            .regions()
            .filter_map(|region| region.content.source())
            .filter(|source| self.session.segments.contains_key(source))
            .collect()
    }

    /// **Forgets a join** nothing reaches any more: it leaves the editor's
    /// table with its segments, so a later edit cannot read through it.
    pub fn forget_join(&mut self, source: SourceId) {
        self.session.segments.remove(&source);
        self.session.sources.remove(&source);
        self.session.lengths.remove(&source);
    }

    /// **Binds source `source` to a sequence** the caller shares: a region
    /// over that source draws the sequence's notes, as it stands when drawn.
    pub fn bind_sequence(&mut self, source: SourceId, sequence: crate::notes::Shared) {
        self.session.sequences.insert(source, sequence);
    }

    /// **The sequence the box of `region` reads**, by its source, when the
    /// box is one of notes: what its roll opens over, from a double click or
    /// from a client's `roll`.
    pub fn roll_source(&self, region: &str) -> Option<u64> {
        clausters_document::multitrack::picture::boxes(&self.multitrack())
            .into_iter()
            .find(|b| b.region.0.to_string() == region)
            .and_then(|b| b.source)
            .filter(|source| self.session.sequences.contains_key(source))
            .map(|source| source.0)
    }

    /// The sequence bound to `source`, when one is.
    pub fn sequence(&self, source: SourceId) -> Option<&crate::notes::Shared> {
        self.session.sequences.get(&source)
    }

    /// **What each take is on disk**: the files the application reads the
    /// tracks' regions from, by source.
    pub fn set_takes(&mut self, takes: HashMap<SourceId, clausters_document::parts::TakeFile>) {
        // a take is drawn from its file, named by its own id
        for (id, file) in &takes {
            self.session.sources.insert(*id, id.0 as i64);
            self.session.lengths.insert(*id, file.frames);
        }
        self.session.takes = takes;
    }

    /// **The takes the window draws from their files**: each take's file and
    /// its peaks, and each join's parts over the takes' files, as the
    /// `segments` list a signal element draws, by the number its boxes name
    /// it by; `None` while the application resolved none.
    ///
    /// A join is a take like any other here: the audio editor's view of a
    /// list of parts, read from the files of the takes it is made of. One
    /// whose takes are not all resolved is left out, and its boxes draw
    /// their frames and nothing in them until they are.
    pub fn lists(&self) -> Option<Value> {
        use clausters_document::session::Part;
        if self.session.takes.is_empty() {
            return None;
        }
        let files: crate::audio::editor::sources::Sources = self
            .session
            .takes
            .iter()
            .map(|(id, file)| {
                (
                    *id,
                    crate::audio::editor::sources::FileSource {
                        path: file.path.to_string(),
                        peaks: file.peaks.as_deref().unwrap_or_default().to_string(),
                        frames: file.frames,
                        channels: file.channels,
                        rate: file.rate,
                        render: false,
                    },
                )
            })
            .collect();
        // a part with no range reads its take whole, as a list's part names
        // its span of the file
        let spanned = |part: &Part| {
            let mut part = part.clone();
            if part.source.range.is_none() {
                let frames = self
                    .session
                    .takes
                    .get(&part.source.source)
                    .map_or(0, |t| t.frames);
                part.source.range = Some(clausters_document::Range {
                    start: 0,
                    end: frames,
                });
            }
            part
        };
        let whole = |id: &SourceId, _: &clausters_document::parts::TakeFile| {
            vec![spanned(&Part {
                source: clausters_document::SourceRef {
                    source: *id,
                    lifetime: clausters_document::Lifetime::External,
                    generation: 0,
                    range: None,
                },
                fade_in: 0,
                fade_out: 0,
                fade_in_shape: Default::default(),
                fade_out_shape: Default::default(),
                channels: None,
            })]
        };
        let mut lists: Map<String, Value> = self
            .session
            .takes
            .iter()
            .map(|(id, file)| {
                let list = crate::audio::editor::sources::wire_list(
                    &whole(id, file),
                    &files,
                    file.channels.max(1),
                    file.rate,
                );
                (id.0.to_string(), list)
            })
            .collect();
        for (id, parts) in &self.session.segments {
            let mut takes = parts
                .iter()
                .map(|p| self.session.takes.get(&p.source.source));
            if takes.clone().any(|t| t.is_none()) {
                continue;
            }
            let Some(Some(first)) = takes.next() else {
                continue;
            };
            let channels = parts
                .iter()
                .filter_map(|p| self.session.takes.get(&p.source.source))
                .map(|t| t.channels)
                .max()
                .unwrap_or(1);
            let parts: Vec<Part> = parts.iter().map(spanned).collect();
            lists.insert(
                id.0.to_string(),
                crate::audio::editor::sources::wire_list(
                    &parts,
                    &files,
                    channels.max(1),
                    first.rate,
                ),
            );
        }
        Some(Value::Object(lists))
    }

    /// One take's file, resolved again (its peaks measured).
    pub fn set_take(&mut self, source: SourceId, file: clausters_document::parts::TakeFile) {
        self.session.sources.insert(source, source.0 as i64);
        self.session.lengths.insert(source, file.frames);
        self.session.takes.insert(source, file);
    }

    /// The parts a join this editor knows is made of.
    pub fn segments_of(&self, source: SourceId) -> Option<Vec<clausters_document::session::Part>> {
        self.session.segments.get(&source).cloned()
    }

    /// The file a take is, when the application resolved it.
    pub fn take(&self, source: SourceId) -> Option<&clausters_document::parts::TakeFile> {
        self.session.takes.get(&source)
    }

    /// **What the tracks' readers read**: each source's shape, and what it
    /// is -- a take's file, or a join's parts over takes' files, read back to
    /// back at the rate of its first part's take. A join over a take the
    /// application has not resolved is left out, and so are the regions over
    /// it.
    pub fn read_sources(
        &self,
    ) -> (
        HashMap<SourceId, clausters_document::multitrack::read::SourceShape>,
        std::collections::BTreeMap<SourceId, crate::takes::PlacedSource>,
    ) {
        use crate::takes::PlacedSource;
        use clausters_document::multitrack::read::SourceShape;
        use clausters_document::parts::{FileList, Rates, Tree};
        let mut shapes = HashMap::new();
        let mut sources = std::collections::BTreeMap::new();
        for (id, file) in &self.session.takes {
            shapes.insert(
                *id,
                SourceShape {
                    channels: file.channels as usize,
                    frames: file.frames,
                    rate: file.rate,
                },
            );
            sources.insert(*id, PlacedSource::File(file.clone()));
        }
        for (id, parts) in &self.session.segments {
            let files: Option<std::collections::BTreeMap<SourceId, _>> = parts
                .iter()
                .map(|part| {
                    let source = part.source.source;
                    self.session
                        .takes
                        .get(&source)
                        .map(|file| (source, file.clone()))
                })
                .collect();
            let Some(files) = files else {
                continue;
            };
            let Some(first) = parts.first().and_then(|p| files.get(&p.source.source)) else {
                continue;
            };
            let rate = first.rate;
            let channels = files.values().map(|f| f.channels).max().unwrap_or(1);
            let rates: std::collections::BTreeMap<SourceId, f64> =
                files.iter().map(|(id, f)| (*id, f.rate)).collect();
            let Ok(tree) = Tree::from_parts(
                parts,
                &Rates {
                    join: rate,
                    of: &rates,
                },
            ) else {
                continue;
            };
            let frames = tree.frames();
            shapes.insert(
                *id,
                SourceShape {
                    channels: channels as usize,
                    frames,
                    rate,
                },
            );
            sources.insert(
                *id,
                PlacedSource::Parts(FileList {
                    channels,
                    rate,
                    tree,
                    files: std::sync::Arc::new(files),
                }),
            );
        }
        (shapes, sources)
    }

    /// Which buffer each source was read into.
    pub fn set_sources(&mut self, sources: HashMap<SourceId, i64>) {
        self.session.sources = sources;
    }

    /// How many frames each take holds. A take left out is one whose length
    /// is not known, and a join over it is not checked against it.
    pub fn set_lengths(&mut self, lengths: HashMap<SourceId, u64>) {
        self.session.lengths = lengths;
    }

    /// Where each track's meters are read from, as last told.
    pub fn meters(&self) -> &[Meter] {
        &self.meters
    }

    /// Where each track's meters are read from.
    pub fn set_meters(&mut self, meters: Vec<Meter>) {
        self.meters = meters;
    }

    /// The position cursor, in seconds.
    pub fn cursor(&self) -> Option<f64> {
        self.cursor
    }

    pub(super) fn set_range(&mut self, span: Option<(f64, f64)>) {
        self.range = span;
    }

    /// The time range a sweep left, `[start, end]` in seconds.
    pub fn range(&self) -> Option<(f64, f64)> {
        self.range
    }

    /// The grid a box lands on, in beats; `0` for none.
    pub fn grid(&self) -> f64 {
        self.grid
    }

    /// **Sets the grid a box lands on**, in beats -- `0` for none. The
    /// window's own, like its zoom; what it changes in the multitrack is what
    /// a drag or a quantize then writes.
    pub fn set_grid(&mut self, beats: f64) {
        self.grid = beats.max(0.0);
    }

    /// Whether a hand snaps to the grid.
    pub fn snap(&self) -> bool {
        self.snap
    }

    /// **Turns the snap-to-grid switch on or off.** The window's own, like
    /// its grid: what it changes is where the next drag lands.
    pub fn set_snap(&mut self, on: bool) {
        self.snap = on;
    }

    /// Places the position cursor, in seconds -- a caller's own verb, like a
    /// rewind.
    pub fn set_cursor(&mut self, secs: Option<f64>) {
        self.cursor = secs;
    }

    /// The window this editor is open in, once it is.
    pub fn set_window(&mut self, window: Option<i32>) {
        if self.window != window {
            self.binding_generation += 1;
            self.displayed_session = window.and(self.session_open.then_some(self.session.id));
        }
        self.window = window;
    }

    /// The window this editor is open in, if it is.
    pub fn window_id(&self) -> Option<i32> {
        self.window
    }

    /// The id of the multitrack's own widget, once a window has been composed.
    pub fn widget(&self) -> Option<i32> {
        self.widget
    }

    /// The id of the ruler, once a window has been composed.
    pub fn ruler(&self) -> Option<i32> {
        self.ruler
    }

    /// Whether this editor drew `widget`: the multitrack, its ruler, or a button of
    /// the transport row.
    pub fn owns(&self, widget: i32) -> bool {
        self.widget == Some(widget)
            || self.ruler == Some(widget)
            || self
                .controls
                .is_some_and(|c| [c.rewind, c.play, c.stop].contains(&widget))
    }

    /// Whether a message on `widget` tagged `tag` is this editor's to answer:
    /// one of its widgets, or its window's own `play` and `loop` -- the space
    /// bar and `L` -- and the verbs of its menu that are the editor's.
    pub fn answers(&self, widget: i32, tag: &str) -> bool {
        self.owns(widget)
            || (self.window == Some(widget)
                && matches!(
                    tag,
                    PLAY_KEY
                        | LOOP_KEY
                        | SAVE_VERB
                        | SAVE_AS_VERB
                        | NEW_FILE_VERB
                        | OPEN_FILE_VERB
                        | CLOSE_FILE_VERB
                        | PAUSE_VERB
                        | STOP_VERB
                        | STOP_AT_END_VERB
                        | CROSSFADE_VERB
                        | crate::notes::editor::SNAP_VERB
                        | VIEW_WAVE_VERB
                        | VIEW_SPECTROGRAM_VERB
                )
                || crate::ruler::Unit::of_verb(tag).is_some())
    }

    /// **Rewind**: the position cursor back at the top, and a stopped
    /// transport cued there. The cursor's own verb, not the transport's -- it is
    /// where the next play starts, and stop goes back to it rather than to the
    /// top.
    pub fn rewind(&mut self, version: i64) -> Outcome {
        let mut out = Outcome {
            turn: Kind::Route,
            version,
            ..Outcome::default()
        };
        let corrections = self.rewound(&mut out);
        out.answer = Some(conversation::answer(0, version, None, corrections));
        out
    }

    /// **Play, or pause where it stands.**
    pub fn toggle(&self, version: i64) -> Outcome {
        Outcome {
            turn: Kind::Route,
            version,
            transport: Some(TransportVerb::Toggle),
            ..Outcome::default()
        }
    }

    /// **Halt and go back to the mark.**
    pub fn stop(&self, version: i64) -> Outcome {
        Outcome {
            turn: Kind::Route,
            version,
            transport: Some(self.stopped()),
            ..Outcome::default()
        }
    }

    /// **What the clock reads** with the multitrack at `position` seconds.
    pub fn clock(&self, position: f64) -> String {
        format!("{position:8.3} s   of {:.3} s", self.multitrack().end().0)
    }

    /// **The window**, composed around the multitrack's widget and ruler ids, which
    /// are the ones a hand's gestures come back on.
    pub fn window(&mut self, widget: i32, ruler: i32) -> Value {
        self.widget = Some(widget);
        self.ruler = Some(ruler);
        let stack = self.dialogs.stack(dialogs::forms());
        let mut def = self.composed(widget, ruler, stack.as_ref(), super::window);
        if let Some(lists) = self.lists() {
            set_on(&mut def, widget, "lists", &lists);
        }
        self.remember_names();
        def
    }

    /// **Everything `widget` should be drawing**, for a correction: the
    /// [`picture`](Self::picture), and the names it tells the host the rows,
    /// the boxes and the curves are called, kept as told.
    pub fn props(&mut self, widget: i32) -> Map<String, Value> {
        let props = self.picture(widget);
        if self.ruler != Some(widget) {
            self.remember_names();
        }
        props
    }

    /// **Everything `widget` should be drawing**, with nothing kept of having
    /// said it: what a holder that redraws the window itself reads, while the
    /// names it tells stay this editor's to settle ([`settle`](Self::settle)).
    pub fn picture(&self, widget: i32) -> Map<String, Value> {
        let (multitrack, ruler) = (
            self.widget.unwrap_or(widget),
            self.ruler.unwrap_or(i32::MIN),
        );
        let mut props = self.composed(multitrack, ruler, None, |w| super::props(w, widget));
        if widget == multitrack && !props.is_empty() {
            // how a box of samples is drawn: its take's waveform or spectrogram
            props.insert("view".into(), json!(self.samples_view));
            // The time range is drawn where the hand sweeps one, so a span set
            // from the client shows as the band a sweep leaves.
            let (start, len) = self
                .range
                .map_or((0.0, 0.0), |(a, b)| (a * self.rate, (b - a) * self.rate));
            props.insert("sel_start".into(), json!(start));
            props.insert("sel_len".into(), json!(len));
            if let Some(lists) = self.lists() {
                props.insert("lists".into(), lists);
            }
        }
        props
    }

    /// Tells the host which version it is drawing, before any edit: a stamp of
    /// zero retires nothing, so this is purely the version, and it is what
    /// keeps the first gesture checked like every later one.
    pub fn announce(&self, version: i64) -> Answer {
        Answer::Ack {
            seq: 0,
            doc_version: version,
            reason: None,
        }
    }

    /// **One message from the host**, read and answered ([`turn::turn`]).
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        turn::turn(self, event, version)
    }

    /// **What a history step is put back to** when another structure of the
    /// same step refuses it: the multitrack and the joins this editor knows.
    pub(crate) fn before_step(&self) -> Kept {
        Kept(self.multitrack().clone(), self.session.segments.clone())
    }

    /// Puts back what [`before_step`](Self::before_step) kept.
    pub(crate) fn put_back(&mut self, kept: Kept) {
        *self.multitrack() = kept.0;
        self.session.segments = kept.1;
    }

    /// **One payload of a history step**, applied to the multitrack -- the inverse
    /// of an edit this editor recorded, or the edit again.
    pub fn apply(&mut self, payload: &Value) -> Applied {
        let mut out = Applied {
            minted: minted(payload),
            ..Applied::default()
        };
        if let Some(source) = &out.minted {
            self.learn(source);
        }
        if let Some((true, _)) = self.edit(payload) {
            out.applied = true;
            out.multitrack = serde_json::to_value(&*self.multitrack()).ok();
        }
        out
    }

    /// **The payloads of one history step, as one**: all of them or none,
    /// through the same door a gesture lands by (`domain::edit_all`), so an
    /// undo never leaves a multitrack half of the way back. One [`Applied`]
    /// per payload that moved anything, each with the multitrack as the
    /// whole step left it; none when the step was refused.
    pub fn apply_step(&mut self, payloads: &[Value]) -> Vec<Applied> {
        let state = serde_json::to_value(&*self.multitrack()).ok().map(Opaque);
        let loads: Vec<Opaque> = payloads.iter().cloned().map(Opaque).collect();
        let Some(gesture) = state.and_then(|state| domain::edit_all(MULTITRACK, &state, &loads))
        else {
            return Vec::new();
        };
        if gesture.reason.is_some() || !gesture.applied {
            return Vec::new();
        }
        let Ok(after) = serde_json::from_value(gesture.state.0.clone()) else {
            return Vec::new();
        };
        *self.multitrack() = after;
        gesture
            .pairs
            .iter()
            .map(|pair| {
                let minted = minted(&pair.forward.0);
                if let Some(source) = &minted {
                    self.learn(source);
                }
                Applied {
                    applied: true,
                    minted,
                    multitrack: Some(gesture.state.0.clone()),
                }
            })
            .collect()
    }

    /// **Every widget of the window, corrected**, with nothing to retire -- what
    /// a history step leaves behind, and what a second window over the multitrack
    /// is told when this one edited it.
    pub fn resync_all(&mut self, version: i64) -> Answer {
        if self.has_session() && !self.observes_session() {
            return Answer::Silent;
        }
        let mut corrections = Vec::new();
        for widget in [self.widget, self.ruler].into_iter().flatten() {
            corrections.extend(self.resync(i64::from(widget)));
        }
        conversation::answer(0, version, None, corrections)
    }

    /// Answers the stamp a [`Kind::Step`] carried, once the caller has walked.
    pub fn acknowledge(&self, seq: i64, version: i64, reason: Option<String>) -> Answer {
        conversation::answer(seq, version, reason, Vec::new())
    }

    /// **A name the host minted is answered with the one the multitrack kept.**
    ///
    /// A gesture is normally answered with an acknowledgement alone, because the
    /// report described the result. Where the host **makes** something -- a
    /// track from a double click, a box from a split -- the host mints the word
    /// and the document mints the id, and a name the multitrack does not know is
    /// read as something new on the next report. So when what the host was last
    /// told differs from what the multitrack now holds, the multitrack goes back whole.
    ///
    /// Called once a changed turn has been carried out -- after a minted source
    /// has a buffer, so the box that windows it draws.
    pub fn settle(&mut self, version: i64) -> Answer {
        let (Some(_), Some(widget), Some(told)) = (self.window, self.widget, self.told.as_ref())
        else {
            return Answer::Silent;
        };
        if *told == self.names() {
            return Answer::Silent;
        }
        let corrections = self.resync(i64::from(widget));
        conversation::answer(0, version, None, corrections)
    }

    /// **What the boxes over its bound sequences play**, placed in seconds of
    /// the multitrack (`clausters_editing::multitrack::placed_notes`): what
    /// the multitrack's playback hands its event lane.
    pub fn placed_notes(&self) -> clausters_editing::notes_playback::Placement {
        let table = Table {
            buffers: &self.session.sources,
            lengths: &self.session.lengths,
            segments: &self.session.segments,
            sequences: &self.session.sequences,
        };
        projection::placed_notes(&self.multitrack(), &table)
    }

    /// The window over the editor's state, handed to `f`.
    fn composed<T>(
        &self,
        widget: i32,
        ruler: i32,
        dialogs: Option<&Value>,
        f: impl FnOnce(&Window<'_>) -> T,
    ) -> T {
        let multitrack = self.multitrack();
        let tempo = projection::tempo_map(&multitrack);
        let table = Table {
            buffers: &self.session.sources,
            lengths: &self.session.lengths,
            segments: &self.session.segments,
            sequences: &self.session.sequences,
        };
        let look = Look {
            rate: self.rate,
            sources: &table,
        };
        f(&Window {
            multitrack: &multitrack,
            look: &look,
            tempo: &tempo,
            widget,
            ruler,
            unit: self.ruler_unit,
            link: self.link,
            cursor: self.cursor,
            meters: &self.meters,
            transport: self.transport,
            title: &self.title,
            size: self.size,
            close_form: self.close_form,
            asks: self.asks,
            saves: self.saves,
            dialogs,
            bare: self.bare,
            grid: self.grid,
            snap: self.snap,
        })
    }

    /// What the multitrack calls its rows, its boxes and its curves.
    fn names(&self) -> [HashSet<String>; 3] {
        let named = projection::names(&self.multitrack());
        let set = |key: &str| -> HashSet<String> {
            named[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        [set("rows"), set("boxes"), set("curves")]
    }

    fn remember_names(&mut self) {
        self.told = Some(self.names());
    }

    /// What `widget` should be drawing, as the one correction it needs.
    fn resync(&mut self, widget: i64) -> Vec<Correction> {
        let Ok(id) = i32::try_from(widget) else {
            return Vec::new();
        };
        let props = self.props(id);
        if props.is_empty() {
            return Vec::new();
        }
        vec![Correction {
            widget,
            props: Value::Object(props),
        }]
    }

    /// The second a position on the axis falls on: the frame rounded the way
    /// every client rounds it.
    fn secs_at(&self, units: f64) -> f64 {
        samples_to_secs(units.round() as i64, self.rate)
    }

    /// One gesture onto the multitrack: screen state, the editor's own, or an edit.
    /// Answers the reason and the corrections the acknowledgement carries.
    fn gesture(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        // **The space bar is the window's**, and it is play/stop: a stop goes
        // back to the position cursor, so the play cursor is where the reader
        // left the mark rather than wherever the pass got to. A multitrack's
        // readers are resident and follow the transport, so there is nothing
        // under the pointer to aim it at.
        if self.window.map(i64::from) == Some(widget) && tag == PLAY_KEY {
            out.transport = Some(TransportVerb::PlayStop {
                mark: self.cursor.unwrap_or(0.0),
                range: self.range,
                looping: values.first().is_some_and(|v| number(v) != 0.0),
            });
            return (None, Vec::new());
        }
        if self.window.map(i64::from) == Some(widget) && tag == LOOP_KEY {
            self.looping = values.first().is_some_and(|v| number(v) != 0.0);
            out.transport = Some(TransportVerb::Loop {
                range: self.range,
                looping: self.looping,
            });
            return (None, Vec::new());
        }
        // **The window's Save and Save as** (Ctrl+S, Ctrl+Shift+S, File)
        if self.window.map(i64::from) == Some(widget) && tag == SAVE_VERB {
            return self.save_from_window(out);
        }
        if self.window.map(i64::from) == Some(widget) && tag == SAVE_AS_VERB {
            return self.open_save_as();
        }
        // **The menu's pause and stop**, the transport row's two buttons by
        // their verbs
        if self.window.map(i64::from) == Some(widget) && tag == PAUSE_VERB {
            out.transport = Some(TransportVerb::Toggle);
            return (None, Vec::new());
        }
        if self.window.map(i64::from) == Some(widget) && tag == STOP_VERB {
            out.transport = Some(self.stopped());
            return (None, Vec::new());
        }
        // **Whether a pass stops where the contents end**: the transport's
        // own switch, flipped by the key, the menu's entry and the tool
        if self.window.map(i64::from) == Some(widget) && tag == STOP_AT_END_VERB {
            out.transport = Some(TransportVerb::StopAtEnd);
            return (None, Vec::new());
        }
        // **The automatic crossfade, on or off**: the multitrack's own default,
        // so an edit -- recorded and undone. It moves no fade: it decides what
        // the edits that follow write.
        if self.window.map(i64::from) == Some(widget) && tag == CROSSFADE_VERB {
            let mut defaults = self.multitrack().defaults.clone();
            defaults.crossfade = !defaults.crossfade;
            let payload = serde_json::to_value(MultitrackIntent::SetDefaults { defaults })
                .unwrap_or_default();
            return self.edited(vec![payload], "crossfade overlaps".into(), out);
        }
        // **The snap-to-grid switch** (G, the View menu): the window's own
        // state, so nothing is recorded and the picture is corrected
        if self.window.map(i64::from) == Some(widget) && tag == crate::notes::editor::SNAP_VERB {
            self.snap = !self.snap;
            let corrections = self
                .widget
                .map(|id| {
                    vec![Correction {
                        widget: i64::from(id),
                        props: Value::Object(self.picture(id)),
                    }]
                })
                .unwrap_or_default();
            return (None, corrections);
        }
        // **The View menu's two ways to draw a box of samples**, the audio
        // editor's two views of a take: the window's own state, so nothing
        // is recorded and the picture is corrected
        if self.window.map(i64::from) == Some(widget)
            && matches!(tag, VIEW_WAVE_VERB | VIEW_SPECTROGRAM_VERB)
        {
            self.samples_view = if tag == VIEW_SPECTROGRAM_VERB {
                SPECTROGRAM
            } else {
                WAVE
            };
            let corrections = self
                .widget
                .map(|id| {
                    vec![Correction {
                        widget: i64::from(id),
                        props: Value::Object(self.picture(id)),
                    }]
                })
                .unwrap_or_default();
            return (None, corrections);
        }
        // **What the window's time ruler counts in**: the window's own, so
        // nothing is recorded, and the strip and the multitrack are corrected
        if self.window.map(i64::from) == Some(widget)
            && let Some(unit) = crate::ruler::Unit::of_verb(tag)
        {
            self.ruler_unit = unit;
            let corrections = [self.widget, self.ruler]
                .into_iter()
                .flatten()
                .map(|id| Correction {
                    widget: i64::from(id),
                    props: Value::Object(self.picture(id)),
                })
                .collect();
            return (None, corrections);
        }
        if tag == "click"
            && let Some(controls) = self.controls
        {
            let id = i32::try_from(widget).unwrap_or(i32::MIN);
            if id == controls.rewind {
                return (None, self.rewound(out));
            }
            if id == controls.play {
                out.transport = Some(TransportVerb::Toggle);
                return (None, Vec::new());
            }
            if id == controls.stop {
                out.transport = Some(self.stopped());
                return (None, Vec::new());
            }
        }
        // **A double click on a box of notes asks for its roll**. Nothing is edited here: the caller opens an editor over the
        // sequence the box reads, and what that editor does is what reaches
        // a history.
        if tag == "open" {
            let named = values.first().map(crate::turn::text).unwrap_or_default();
            out.open = self.roll_source(&named);
            return (None, Vec::new());
        }
        if NOT_AN_EDIT.contains(&tag) {
            self.observe(tag, values, out);
            // a selection let go moved the position cursor to its start, and
            // the window draws it there with the play cursor
            if out.follow == Some(true) && self.range.is_some() {
                return (None, self.cursor_corrections());
            }
            return (None, Vec::new());
        }
        let taken = {
            let table = Table {
                buffers: &self.session.sources,
                lengths: &self.session.lengths,
                segments: &self.session.segments,
                sequences: &self.session.sequences,
            };
            let look = Look {
                rate: self.rate,
                sources: &table,
            };
            projection::intake(&self.multitrack(), tag, values, &look)
        };
        if taken.payloads.is_empty() {
            // Nothing, or a refusal. A refusal says why and hands the widget back
            // what it should be drawing, so the picture stops agreeing with the
            // hand instead of with the multitrack.
            return match taken.refusal {
                Some(why) => (Some(why), self.resync(widget)),
                None => (None, Vec::new()),
            };
        }
        let label = if taken.label.is_empty() {
            "edit".to_string()
        } else {
            taken.label
        };
        self.edited(taken.payloads, label, out)
    }

    /// **Applies `payloads` as one edit labelled `label`**: recorded with the
    /// payloads that put it back, the version moved, and answered with what
    /// the owner works out from it when that moved.
    fn edited(
        &mut self,
        payloads: Vec<Value>,
        label: String,
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        // **What the owner works out from what the hand moved**, before the
        // edit: see `derived` below.
        let before = self.widget.map(|id| self.derived(id));
        // **The gesture lands whole or not at all** (`domain::edit_all`): an
        // edit of it refused leaves the multitrack as it was, and the window
        // is handed back what it should be drawing, with the reason.
        let state = serde_json::to_value(&*self.multitrack()).ok().map(Opaque);
        let loads: Vec<Opaque> = payloads.iter().cloned().map(Opaque).collect();
        let Some(gesture) = state.and_then(|state| domain::edit_all(MULTITRACK, &state, &loads))
        else {
            return (None, Vec::new());
        };
        if let Some(why) = gesture.reason {
            let corrections = self
                .widget
                .map_or_else(Vec::new, |id| self.resync(i64::from(id)));
            return (Some(why), corrections);
        }
        let moved = gesture.applied;
        if moved {
            let Ok(after) = serde_json::from_value(gesture.state.0) else {
                return (None, Vec::new());
            };
            *self.multitrack() = after;
            // what a join minted is learned only once the join stands, and
            // before its legs name what they hold
            for pair in &gesture.pairs {
                if let Some(source) = minted(&pair.forward.0) {
                    self.learn(&source);
                    out.minted.push(source);
                }
            }
            // a lone edit with no inverse moves the version and leaves no
            // entry; `edit_all` has refused one among several
            let legs: Vec<Leg> = gesture
                .pairs
                .into_iter()
                .filter_map(|pair| Some((pair.forward.0, pair.backward?.0)))
                .map(|(forward, backward)| Leg {
                    holds_forward: self.joins_named(&forward),
                    holds_backward: self.joins_named(&backward),
                    key: domain::coalesce_key(MULTITRACK, &Opaque(forward.clone()))
                        .unwrap_or_default(),
                    forward: json!({ "edit": forward }),
                    backward,
                })
                .collect();
            if !legs.is_empty() {
                out.record = Some(Record { label, legs });
            }
            out.version += 1;
            out.changed = true;
            out.multitrack = serde_json::to_value(&*self.multitrack()).ok();
        }
        // **What the hand did not draw is told**: a gesture that applies
        // cleanly is acknowledged and the window keeps its own picture, which
        // is right for what the hand moved and wrong for what the owner works
        // out from it -- a box's fade envelope, which the edit wrote a
        // crossfade into because of the box it was dragged over. Those are corrected whenever the edit
        // moved them.
        // When the names the host was last told no longer match, `settle`
        // sends the whole picture after this turn, these props with it, and a
        // second push here would only repeat them.
        let resent = self.told.as_ref().is_some_and(|told| *told != self.names());
        let corrections = match (moved && !resent, before, self.widget) {
            (true, Some(before), Some(id)) => {
                let after = self.derived(id);
                if after == before {
                    Vec::new()
                } else {
                    vec![Correction {
                        widget: i64::from(id),
                        props: Value::Object(after),
                    }]
                }
            }
            _ => Vec::new(),
        };
        (None, corrections)
    }

    /// **The props of the multitrack the owner works out rather than the hand
    /// draws**: the curve layers and every curve's points, a box's fade
    /// envelope among them -- whose crossfade the edit writes from where the
    /// boxes beside it are, which no gesture on the box draws.
    fn derived(&self, widget: i32) -> Map<String, Value> {
        let picture = self.picture(widget);
        ["layers", "points"]
            .into_iter()
            .filter_map(|key| Some((key.to_string(), picture.get(key)?.clone())))
            .collect()
    }

    /// The cursor put back at the top and the transport cued there; answers the
    /// correction that tells the host where the cursor now is.
    fn rewound(&mut self, out: &mut Outcome) -> Vec<Correction> {
        self.cursor = Some(0.0);
        out.cursor = Some(0.0);
        out.transport = Some(TransportVerb::Cue { secs: 0.0 });
        self.cursor_corrections()
    }

    /// The correction that tells the host where the position cursor now is.
    fn cursor_corrections(&self) -> Vec<Correction> {
        let Some(widget) = self.widget else {
            return Vec::new();
        };
        // The host owns where the cursor *is*, so it is told rather than left
        // to find out on the next redraw.
        let units = self.composed(widget, self.ruler.unwrap_or(i32::MIN), None, |w| {
            w.cursor_units()
        });
        vec![Correction {
            widget: i64::from(widget),
            props: json!({ "cursor": units }),
        }]
    }

    /// A stop, back to the mark the position cursor is on.
    fn stopped(&self) -> TransportVerb {
        TransportVerb::Stop {
            mark: self.cursor.unwrap_or(0.0),
        }
    }

    /// Applies one payload, answering whether it moved anything and the payload
    /// that puts it back. `None` for a payload the multitrack cannot read.
    fn edit(&mut self, payload: &Value) -> Option<(bool, Option<Value>)> {
        let state = Opaque(serde_json::to_value(&*self.multitrack()).ok()?);
        let edited = domain::edit(MULTITRACK, &state, &Opaque(payload.clone()))?;
        let current = edited.current.map(|c| c.0);
        if !edited.applied {
            return Some((false, current));
        }
        *self.multitrack() = serde_json::from_value(edited.state.0).ok()?;
        Some((true, current))
    }

    /// A tag that says what the view is looking at rather than what changed.
    fn observe(&mut self, tag: &str, values: &[Value], out: &mut Outcome) {
        match tag {
            // A click on the time ruler: the reader put the position cursor
            // there. It is not a seek -- the playhead is never placed -- and
            // what it means for a transport is the caller's.
            "locate" if !values.is_empty() => {
                let secs = self.secs_at(number(&values[0]));
                self.cursor = Some(secs);
                out.locate = Some(secs);
            }
            // Heard, and never written: the curve takes the parameter back when
            // the hand lets go.
            "port_hold" => {
                let text = |i: usize| values.get(i).map(crate::turn::text).unwrap_or_default();
                if let Ok(track) = text(0).parse() {
                    let value = values.get(2).map(|v| number(v) as f32);
                    out.port_hold = Some((track, text(1), value));
                }
            }
            "selection" => {
                let at = |i: usize| values.get(i).map_or(0.0, |v| self.secs_at(number(v)));
                let mut selection = json!({ "start": at(0), "len": at(1) });
                self.range = (at(1) > 0.0).then(|| (at(0), at(0) + at(1)));
                if values.len() >= 4 {
                    // The sweep restricted the value axis too, carried as it
                    // came: no unit of this editor's applies to it.
                    selection["value"] =
                        json!({ "min": number(&values[2]), "max": number(&values[3]) });
                }
                out.selection = Some(selection);
                // **A selection let go puts the cursor at its start**, and the
                // music with it, whether it plays or not; a loop in progress
                // repeats over the new span.
                let released = values.last().and_then(Value::as_bool) == Some(true);
                if released && let Some((start, _)) = self.range {
                    self.cursor = Some(start);
                    out.locate = Some(start);
                }
                out.follow = Some(released);
            }
            _ => {}
        }
    }
}

/// The tag the host's space bar reaches a window with.
pub const PLAY_KEY: &str = "play";

/// The tag the host's `L` reaches a window with, the loop switch's new state
/// beside it.
pub const LOOP_KEY: &str = "loop";

/// **The window's pause**: play, or pause where it stands -- the menu's
/// Pause, what the transport row's play/pause button does.
pub const PAUSE_VERB: &str = "pause";

/// **The window's Save**: the session written where it saves, or the Save as
/// form for one with no file yet.
pub const SAVE_VERB: &str = "save";
/// **The window's Save as**: the form that asks where the session is written.
pub const SAVE_AS_VERB: &str = "save_as";
/// **The window's File > New**: a new, untitled session in its place.
pub const NEW_FILE_VERB: &str = "new_file";
/// **The window's File > Open**: the form that chooses the session to open.
pub const OPEN_FILE_VERB: &str = "open_file";
/// **The window's File > Close file**: the window goes on with no session.
pub const CLOSE_FILE_VERB: &str = "close_file";

/// A box of samples drawn as its take's waveform: the `view` word.
const WAVE: &str = "trace";
/// ...and as its spectrogram.
const SPECTROGRAM: &str = "spectrogram";
/// The window verb that draws the boxes of samples as waveforms, the audio
/// editor's own.
pub const VIEW_WAVE_VERB: &str = "view_wave";
/// The window verb that draws them as spectrograms.
pub const VIEW_SPECTROGRAM_VERB: &str = "view_spectrogram";

/// **The window's stop**: halt and go back to the mark -- the menu's Stop.
pub const STOP_VERB: &str = "stop";

/// **The window's end switch**: whether a pass stops where the contents end
/// or rolls on -- a key, the Transport menu's entry and the tool beside Loop.
pub const STOP_AT_END_VERB: &str = "stop_at_end";
/// **The automatic crossfade, on or off**, as the window asks for it: a flip
/// of [`Defaults::crossfade`](clausters_document::multitrack::Defaults), the
/// multitrack's own state, through the same `SetDefaults` edit a client's
/// `Multitrack.crossfade` makes. The Edit menu's entry is one caller of it.
pub const CROSSFADE_VERB: &str = "crossfade";

/// The scope of the key table the multitrack window's own verbs are in.
pub const KEYS: &str = clausters_editing::verbs::MULTITRACK_SCOPE;

/// **The window's `verbs` prop**: the verbs this application adds to the
/// host's, each with its default chords and its words -- the rows of the one
/// verb table ([`clausters_editing::verbs::MULTITRACK`]).
#[must_use]
pub fn verbs() -> Value {
    clausters_editing::verbs::declaration(&[(KEYS, clausters_editing::verbs::MULTITRACK)])
}

/// Sets `key` on the node of `def` whose id is `widget`.
fn set_on(def: &mut Value, widget: i32, key: &str, value: &Value) -> bool {
    if def["id"].as_i64() == Some(i64::from(widget))
        && let Some(map) = def.as_object_mut()
    {
        map.insert(key.into(), value.clone());
        return true;
    }
    let children = def.get_mut("children").and_then(Value::as_array_mut);
    for child in children.into_iter().flatten() {
        if set_on(child, widget, key, value) {
            return true;
        }
    }
    false
}

/// **The editor's tables as one**: which buffer each source was read into, how
/// many frames each take holds, and the segments each join it knows is made of.
struct Table<'a> {
    buffers: &'a HashMap<SourceId, i64>,
    lengths: &'a HashMap<SourceId, u64>,
    segments: &'a HashMap<SourceId, Vec<clausters_document::session::Part>>,
    sequences: &'a HashMap<SourceId, crate::notes::Shared>,
}

impl projection::Buffers for Table<'_> {
    fn bufnum(&self, source: SourceId) -> i64 {
        projection::Buffers::bufnum(self.buffers, source)
    }

    /// A join's id is taken whether or not a buffer is behind it yet: ids are
    /// never reused, so nothing may mint one a join already has.
    fn taken(&self) -> Vec<SourceId> {
        let mut taken = projection::Buffers::taken(self.buffers);
        taken.extend(self.segments.keys().copied());
        taken.extend(self.sequences.keys().copied());
        taken
    }

    fn source(&self, bufnum: i64) -> Option<SourceId> {
        projection::Buffers::source(self.buffers, bufnum)
    }

    fn parts(&self, source: SourceId) -> Option<Vec<clausters_document::session::Part>> {
        self.segments.get(&source).cloned()
    }

    fn frames(&self, source: SourceId) -> Option<u64> {
        self.lengths.get(&source).copied()
    }

    fn sequence(&self, source: SourceId) -> Option<clausters_document::EventSequence> {
        let shared = self.sequences.get(&source)?;
        Some(shared.lock().unwrap_or_else(|e| e.into_inner()).clone())
    }
}

/// The source a payload minted, when it names one.
fn minted(payload: &Value) -> Option<Value> {
    payload
        .get("source")
        .filter(|s| s.get("id").is_some_and(|id| !id.is_null()))
        .cloned()
}

/// What [`call_json`] builds an editor from: the multitrack, its axis and its
/// sources, the window's chrome, and the version its history is at.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct New {
    #[serde(default)]
    multitrack: Value,
    #[serde(default)]
    empty: bool,
    #[serde(default)]
    rate: f64,
    #[serde(default)]
    version: i64,
    #[serde(default)]
    link: Option<i64>,
    #[serde(default)]
    transport: Value,
    #[serde(default)]
    title: String,
    #[serde(default)]
    w: i64,
    #[serde(default)]
    h: i64,
    /// `false` for a window with no menu bar and no tools ([`Window::bare`]).
    #[serde(default = "yes")]
    chrome: bool,
}

fn yes() -> bool {
    true
}

/// An editor built from a JSON request, or `None` for one that names no multitrack.
pub fn new_json(request: &str) -> Option<MultitrackEditor> {
    let parsed: New = serde_json::from_str(request).ok()?;
    let multitrack = if parsed.empty {
        Multitrack::new()
    } else {
        serde_json::from_value(parsed.multitrack.clone()).ok()?
    };
    Some(built(super::shared(multitrack), parsed))
}

fn built(multitrack: super::Shared, request: New) -> MultitrackEditor {
    let mut editor = MultitrackEditor::over(multitrack, request.rate, request.version);
    let transport = match request.transport {
        Value::Bool(true) => Transport::Unnumbered,
        Value::Object(_) => serde_json::from_value::<TransportIds>(request.transport)
            .map_or(Transport::Unnumbered, Transport::Numbered),
        _ => Transport::Absent,
    };
    editor.chrome(
        request.link,
        transport,
        &request.title,
        (request.w, request.h),
    );
    editor.session_open = !request.empty;
    editor.displayed_session = (!request.empty).then_some(0);
    editor.set_bare(!request.chrome);
    editor
}

/// **One verb of an editor, over JSON** -- the door both clients bind.
///
/// One door rather than one per verb because the verbs are the application's
/// surface, and a door per verb would be each binding restating it. `request`
/// names the `verb` and carries its arguments:
///
/// - `sync` -- `multitrack`, `sources`, `meters`, `cursor` (beats or `null`),
///   `window`, `controls` (the transport row's ids), `closeForm` (the close
///   form's: `stack`, `dialog`, `save`, `discard`, `cancel`), `asks` (whether
///   the window asks before a close loses work), `grid` (beats, `0` for
///   none), `snap` (the snap-to-grid switch): the state a caller holds,
///   handed over before the verbs that read it.
/// - `grid` -- `{"grid": beats, "snap": bool}`: the grid a box lands on, and
///   whether a hand snaps to it.
/// - `rewind`, `toggle`, `stop` -- `version`: the transport row's verbs, as a
///   script calls them, each an [`Outcome`].
/// - `clock` -- `position` (beats): `{"text"}`, what the clock reads.
/// - `window` -- `widget`, `ruler`: the window, as a GuiDef.
/// - `setWindow` -- `window` (an id or `null`).
/// - `props` -- `widget`.
/// - `notes` -- `{"placed": [...]}`, what the boxes over bound sequences play
///   ([`MultitrackEditor::placed_notes`]).
/// - `event` -- `addr`, `args`, `version`: an [`Outcome`].
/// - `apply` -- `payload`: an [`Applied`].
/// - `resync`, `settle`, `announce` -- `version`: an [`Answer`].
/// - `span` -- `span`: `[start, end]` in seconds, or `null`: the time range
///   the space bar plays and the multitrack draws, as a sweep leaves it. `{}`.
/// - `acknowledge` -- `seq`, `version`, `reason`: an [`Answer`].
///
/// An unknown verb answers `{}`.
pub fn call_json(editor: &mut MultitrackEditor, request: &str) -> String {
    let Ok(request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    call_value(editor, &request).to_string()
}

/// Execute a decoded editor call using the same verbs as [`call_json`].
pub fn call_value(editor: &mut MultitrackEditor, request: &Value) -> Value {
    let call = super::calls::Call::read(request);
    call.execute(
        editor,
        request.get("version").and_then(Value::as_i64).unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::multitrack::Window;
    use clausters_document::multitrack::{Content, Region, Track};
    use clausters_document::{Lifetime, NodeId, Second, SourceRef};

    const SR: f64 = 48_000.0;

    fn region(id: u64, at: f64) -> Region {
        let mut region = Region::new(
            NodeId(id),
            Second(at),
            Second(2.0),
            Content::Unknown(Value::Null),
        );
        region.content = Content::window(Window {
            source: SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            },
            start: 0.0,
            duration: 2.0,
        });
        region
    }

    /// **A window asks whether there is unsaved work at every event**, and
    /// the session is built to compare only when something it reads moved.
    #[test]
    fn an_unsaved_reading_is_read_again_only_when_what_it_reads_moved() {
        let mut editor = editor();
        editor.set_checkpoint(Some(editor.session_snapshot()));
        assert!(!editor.unsaved_at(1));
        // a change that moves no counter is not seen until one moves
        editor.multitrack().channels = 6;
        assert!(!editor.unsaved_at(1), "the same reading at the same key");
        assert!(editor.unsaved_at(2), "the context moved: read again");
        editor.set_checkpoint(Some(editor.session_snapshot()));
        assert!(!editor.unsaved_at(2), "a new checkpoint is read again");
    }

    #[test]
    fn a_restored_sequence_checkpoint_is_clean_while_its_id_watermark_is_kept() {
        use clausters_document::events::{Event, EventSequence, EventsIntent};
        let mut editor = MultitrackEditor::new(Multitrack::new(), SR, 0);
        let sequence = std::sync::Arc::new(std::sync::Mutex::new(EventSequence::default()));
        editor.bind_sequence(SourceId(7), sequence.clone());
        editor.set_checkpoint(Some(editor.session_snapshot()));
        let before = sequence.lock().unwrap().state();
        sequence
            .lock()
            .unwrap()
            .edit(EventsIntent::Add {
                event: Event::new(0.0, json!({"midinote":60})),
            })
            .unwrap();
        assert!(editor.unsaved_contents());
        sequence.lock().unwrap().edit(before).unwrap();
        assert!(
            !editor.unsaved_contents(),
            "undo restores the checkpoint's contents"
        );
        assert_eq!(sequence.lock().unwrap().next_id, 1);
        assert!(!editor.unsaved_contents());
        let released = editor.replace_session_binding().unwrap();
        let clausters_document::session::Location::Events { sequence } =
            &released.snapshot.session.sources[&SourceId(7)].location
        else {
            panic!()
        };
        assert_eq!(
            sequence.next_id, 1,
            "the written id watermark must not go back"
        );
    }

    /// Two tracks: the first holding boxes 12 and 13, the second box 22.
    fn editor() -> MultitrackEditor {
        let mut first = Track::new(NodeId(10), NodeId(11));
        first.take_lanes[0].regions = vec![region(12, 0.0), region(13, 4.0)];
        let mut second = Track::new(NodeId(20), NodeId(21));
        second.take_lanes[0].regions = vec![region(22, 0.0)];
        let multitrack = Multitrack {
            tracks: vec![first, second],
            ..Multitrack::default()
        };
        let mut editor = MultitrackEditor::new(multitrack, SR, 1);
        editor.set_sources(HashMap::from([(SourceId(1), 7)]));
        editor.chrome(None, Transport::Unnumbered, "multitrack", (1000, 560));
        editor.window(40, 41);
        editor.set_window(Some(39));
        editor
    }

    /// **A gesture lands whole or not at all**: one whose second edit is
    /// refused leaves the first unmade, records nothing, keeps the version,
    /// and hands the window back what to draw with the reason.
    #[test]
    fn a_gesture_with_a_refused_edit_leaves_the_multitrack_as_it_was() {
        let mut ed = editor();
        let start = ed.multitrack().clone();
        let place = |region: u64, track: u64, lane: u64, at: f64| {
            serde_json::to_value(MultitrackIntent::PlaceRegion {
                region: NodeId(region),
                track: NodeId(track),
                take_lane: NodeId(lane),
                position: Second(at),
                layer: 0,
            })
            .unwrap()
        };
        let mut out = Outcome::default();
        let (why, corrections) = ed.edited(
            vec![place(12, 10, 11, 8.0), place(99, 10, 11, 1.0)],
            "move".into(),
            &mut out,
        );
        assert!(why.is_some(), "the refusal says why");
        assert!(!corrections.is_empty(), "and the window is handed back");
        assert_eq!(
            *ed.multitrack(),
            start,
            "the first move is not left standing"
        );
        assert!(out.record.is_none());
        assert_eq!(out.version, 0);

        // the same move alone lands, recorded as one entry
        let mut out = Outcome::default();
        let (why, _) = ed.edited(vec![place(12, 10, 11, 8.0)], "move".into(), &mut out);
        assert_eq!(why, None);
        assert_eq!(out.record.map(|r| r.legs.len()), Some(1));
    }

    /// **The window that is the session's one holder asks before it closes
    /// with something unsaved**, in the form it holds: Save writes and
    /// closes, Don't save closes, Cancel keeps it. A window a client holds,
    /// or one with nothing unsaved, closes at once.
    #[test]
    fn the_sessions_one_window_asks_before_a_close_loses_work() {
        let ids = crate::closing::Ids {
            stack: 50,
            dialog: 51,
            save: 52,
            discard: 53,
            cancel: 54,
        };
        let close = || event(39, 1, 1, crate::closing::VERB, vec![]);
        // a client's: no form, no asking, and closing loses nothing
        let mut client = editor();
        assert_eq!(client.window(40, 41)["ask_close"], false);
        client.set_unsaved(true);
        assert!(client.event(&close(), 1).close);
        // the standalone host's: the form is in the window, and it asks
        let mut alone = editor();
        alone.set_close_form(Some(ids));
        alone.set_asks_to_close(true);
        let window = alone.window(40, 41);
        assert_eq!(window["ask_close"], true);
        let held = window["children"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(held["id"], 50);
        assert!(alone.event(&close(), 1).close, "nothing unsaved: at once");
        alone.set_unsaved(true);
        let out = alone.event(&close(), 1);
        assert!(!out.close);
        let Some(Answer::Push { corrections, .. }) = out.answer else {
            panic!("the form shown")
        };
        assert_eq!(corrections[0].props, json!({"index": 1}));
        // Cancel keeps the window; Don't save closes; Save saves, then closes
        let said = |editor: &mut MultitrackEditor, widget: i64| {
            editor.event(&event(widget, 2, 1, "click", vec![]), 1)
        };
        let out = said(&mut alone, 54);
        assert!(!out.close && !out.save);
        alone.event(&close(), 1);
        let out = said(&mut alone, 53);
        assert!(out.close && !out.save);
        alone.event(&close(), 1);
        let out = said(&mut alone, 52);
        assert!(out.close && out.save);
    }

    fn event(widget: i64, seq: i64, against: i64, tag: &str, values: Vec<Value>) -> Event {
        let mut args = vec![json!(widget), json!(seq), json!(against), json!(tag)];
        args.extend(values);
        Event {
            addr: "/gui_event".into(),
            args,
        }
    }

    fn boxes(entries: &[(&str, &str, f64)]) -> Vec<Value> {
        entries
            .iter()
            .flat_map(|(name, row, at)| {
                [
                    json!(name),
                    json!(row),
                    json!(at * SR),
                    json!(2.0 * SR),
                    json!(0.0),
                    json!(""),
                    json!(7),
                ]
            })
            .collect()
    }

    fn position(editor: &MultitrackEditor) -> f64 {
        editor.multitrack().tracks[0].take_lanes[0].regions[0]
            .position
            .0
    }

    /// **The editor joins cuts of its own joins flat** *(found 2026-09-13 by
    /// the user: a join of joins was refused as nested more than four deep)*.
    /// The first join mints a source and the editor keeps its segments; a
    /// second join over the joined box reads through them, so what it mints
    /// names the take and never the first join.
    #[test]
    fn the_editor_joins_cuts_of_its_own_joins_flat() {
        let over_take = |id: u64, at: f64, start: f64| {
            let mut region = Region::new(
                NodeId(id),
                Second(at),
                Second(1.0),
                Content::Unknown(Value::Null),
            );
            region.content = Content::window(Window {
                source: SourceRef {
                    source: SourceId(1),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                },
                start,
                duration: 1.0,
            });
            region
        };
        // The take's halves swapped, and behind them a box that does not read
        // on from the second.
        let mut track = Track::new(NodeId(10), NodeId(11));
        track.take_lanes[0].regions = vec![
            over_take(12, 0.0, 1.0),
            over_take(13, 1.0, 0.0),
            over_take(14, 2.0, 1.5),
        ];
        let multitrack = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let mut editor = MultitrackEditor::new(multitrack, SR, 1);
        editor.set_takes(HashMap::from([(
            SourceId(1),
            clausters_document::parts::TakeFile {
                path: "/work/take.wav".into(),
                peaks: Some("/work/take.peaks".into()),
                channels: 1,
                // long enough for the box behind the halves to read on
                frames: 3 * SR as u64,
                rate: SR,
            },
        )]));
        editor.chrome(None, Transport::Unnumbered, "multitrack", (1000, 560));
        editor.window(40, 41);
        editor.set_window(Some(39));

        let first = editor.event(&event(40, 1, 0, "join", vec![json!("12"), json!("13")]), 1);
        let [made] = first.minted.as_slice() else {
            panic!("the first join mints: {:?}", first.minted);
        };
        let made = made["id"].clone();

        // **And the window is told what the join is made of**: a list of
        // two parts of the take's file, its second half and then its first,
        // in the order they play -- drawn as the audio editor draws a file
        // made of parts, from the take's own peaks.
        let props = editor.props(40);
        let list = &props["lists"][made.as_u64().expect("an id").to_string()];
        let parts: Vec<(Value, Value, Value)> = list["parts"]
            .as_array()
            .unwrap_or_else(|| panic!("the join is listed: {props:?}"))
            .iter()
            .map(|p| (p["path"].clone(), p["start"].clone(), p["frames"].clone()))
            .collect();
        let half = json!(SR as u64);
        assert_eq!(
            parts,
            [
                (json!("/work/take.wav"), half.clone(), half.clone()),
                (json!("/work/take.wav"), json!(0), half),
            ],
            "the joined box, drawn from the parts of the take it is made of"
        );
        assert_eq!(list["parts"][0]["peaks"], json!("/work/take.peaks"));
        // ...and its box names it by its own id, as a take's names the take.
        let clips = props["clips"].as_array().expect("clips");
        assert!(
            clips.chunks(7).any(|c| c[0] == json!("12") && c[6] == made),
            "the joined box names the join: {clips:?}"
        );

        let second = editor.event(
            &event(40, 2, 0, "join", vec![json!("12"), json!("14")]),
            first.version,
        );
        let [minted] = second.minted.as_slice() else {
            panic!("the second join mints: {:?}", second.minted);
        };
        assert_ne!(
            minted["id"], made,
            "a new object, not the first join edited"
        );
        let parts = minted["location"]["parts"]
            .as_array()
            .unwrap_or_else(|| panic!("segments in {minted}"));
        assert_eq!(
            parts.len(),
            3,
            "the first join's two spans, then the take's"
        );
        assert!(
            parts.iter().all(|p| p["source"]["source"] == json!(1)),
            "every segment names the take, none the first join: {parts:?}"
        );
    }

    /// **A move is applied, recorded with its inverse and acknowledged**, and
    /// the inverse puts the multitrack back.
    #[test]
    fn a_move_reaches_the_multitrack_and_its_inverse_puts_it_back() {
        let mut ed = editor();
        let moved = boxes(&[("12", "10", 2.0), ("13", "10", 4.0), ("22", "20", 0.0)]);
        let out = ed.event(&event(40, 5, 1, "clips", moved), 1);
        assert_eq!(out.turn, Kind::Route);
        assert!(out.changed);
        assert_eq!(out.version, 2);
        assert_eq!(position(&ed), 2.0);
        assert_eq!(
            out.answer,
            Some(Answer::Ack {
                seq: 5,
                doc_version: 2,
                reason: None
            })
        );
        let record = out.record.expect("recorded");
        assert_eq!(record.legs.len(), 1, "one box moved");
        assert!(ed.apply(&record.legs[0].backward).applied);
        assert_eq!(position(&ed), 0.0);
        assert!(ed.apply(&record.legs[0].forward["edit"]).applied);
        assert_eq!(position(&ed), 2.0);
    }

    /// **A box dragged over another is answered with the crossfade**: the hand
    /// drew the move, and the owner tells the window what follows from it --
    /// the fade envelopes of both boxes, stretched over the overlap -- which
    /// no gesture on the box draws. The longer the overlap, the longer the
    /// crossfade.
    #[test]
    fn a_box_dragged_over_another_is_answered_with_the_crossfade() {
        let fades_of = |props: &Value, name: &str| -> Vec<f64> {
            props["points"]
                .as_array()
                .unwrap()
                .chunks(5)
                .filter(|p| p[0] == json!(name))
                .map(|p| p[1].as_f64().unwrap())
                .collect()
        };
        for (at, overlap) in [(3.0, 1.0), (3.5, 1.5)] {
            let mut ed = editor();
            // 12 is two seconds long, and 13 starts at four: 12 at three
            // overlaps it by one second, at three and a half by one and a half.
            let moved = boxes(&[("12", "10", at), ("13", "10", 4.0), ("22", "20", 0.0)]);
            let out = ed.event(&event(40, 5, 1, "clips", moved), 1);
            let Some(Answer::Push { corrections, .. }) = out.answer else {
                panic!("the crossfade is told: {:?}", out.answer);
            };
            assert_eq!(corrections[0].widget, 40);
            let props = &corrections[0].props;
            // 12 fades out over the overlap, from where 13 starts to its end...
            let twelve = fades_of(props, "fade:12");
            assert!(
                (twelve[3] - twelve[2] - overlap * SR).abs() < 1.0,
                "{twelve:?}"
            );
            // ...and 13 fades in over the same span.
            let thirteen = fades_of(props, "fade:13");
            assert!(
                (thirteen[1] - thirteen[0] - overlap * SR).abs() < 1.0,
                "{thirteen:?}"
            );
        }
    }

    /// **Snap to grid is the window's switch**, from G or the View menu: no
    /// edit, and the multitrack is told which way it now is.
    #[test]
    fn the_snap_to_grid_switch_is_the_window_s_verb() {
        let mut ed = editor();
        assert!(ed.snap(), "on unless turned off");
        let menu = event(39, 6, 1, crate::turn::MENU, vec![json!("snap")]);
        let out = ed.event(&menu, 1);
        assert!(!ed.snap());
        assert!(!out.changed && out.record.is_none(), "no edit");
        let Some(Answer::Push { corrections, .. }) = out.answer else {
            panic!("the window is told: {:?}", out.answer);
        };
        assert_eq!(corrections[0].props["grid_snap"], json!(false));
    }

    /// **The window's crossfade verb is an edit of the multitrack's
    /// defaults** -- recorded, undone -- and it acts on the edits that follow:
    /// the overlap a move made while it was on keeps the crossfade the move
    /// wrote into its fades, and a move made while it is off writes none. The
    /// state is the document's: the verb arrives here from the Edit menu, and
    /// a client sets the same default through `Multitrack.crossfade` with no
    /// window at all.
    #[test]
    fn the_crossfade_switch_acts_on_the_edits_that_follow() {
        let mut ed = editor();
        let fade_out = |ed: &MultitrackEditor, id: u64| {
            let multitrack = ed.multitrack();
            let (_, _, region) = multitrack.locate(NodeId(id)).unwrap();
            region.fade_out.clone()
        };
        let over = boxes(&[("12", "10", 3.0), ("13", "10", 4.0), ("22", "20", 0.0)]);
        let moved = ed.event(&event(40, 5, 1, "clips", over), 1);
        assert!(ed.multitrack().defaults.crossfade, "on unless turned off");
        let legs = moved.record.expect("the move is recorded").legs;
        assert!(legs.len() > 1, "the move and its crossfade are one entry");
        let crossed = fade_out(&ed, 12).expect("the move wrote the crossfade");
        assert!(crossed.crossfade && crossed.length.get() > 0.5);

        let menu = event(39, 6, 2, crate::turn::MENU, vec![json!(CROSSFADE_VERB)]);
        let out = ed.event(&menu, 2);
        assert!(!ed.multitrack().defaults.crossfade);
        assert!(out.changed);
        let record = out.record.expect("recorded, so it can be undone");
        assert_eq!(
            fade_out(&ed, 12),
            Some(crossed),
            "turning it off changes no region"
        );
        let apart = boxes(&[("12", "10", 9.0), ("13", "10", 4.0), ("22", "20", 0.0)]);
        ed.event(&event(40, 7, 3, "clips", apart), 3);
        let crossed = fade_out(&ed, 12).expect("off, a move leaves the fades it finds");
        assert!(crossed.crossfade);
        assert!(ed.apply(&record.legs[0].backward).applied);
        assert!(ed.multitrack().defaults.crossfade, "undone");
    }

    /// A report of what already holds is not an edit: nothing recorded, the
    /// version unmoved, and the stamp still answered.
    #[test]
    fn a_report_of_what_holds_is_not_an_edit() {
        let mut ed = editor();
        let held = boxes(&[("12", "10", 0.0), ("13", "10", 4.0), ("22", "20", 0.0)]);
        let out = ed.event(&event(40, 3, 1, "clips", held), 1);
        assert!(!out.changed && out.record.is_none());
        assert_eq!(out.version, 1);
        assert!(matches!(out.answer, Some(Answer::Ack { seq: 3, .. })));
    }

    /// **The position cursor is kept in seconds, told, and is not an edit.**
    /// **The multitrack's time ruler counts in the window's unit**: bars and
    /// beats at first, over the multitrack and facing it; a ruler verb is the
    /// window's, corrects the strip and the multitrack, and records nothing.
    #[test]
    fn the_ruler_counts_in_the_unit_the_window_chooses() {
        let mut ed = editor();
        let window = ed.window(40, 41);
        let ruler = &window["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == 41)
            .cloned()
            .unwrap_or_else(|| window.to_string().into());
        assert_eq!(ruler["axes"]["x"]["unit"], "beats", "{window}");
        assert_eq!(ruler["axes"]["x"]["dir"], "down", "facing the multitrack");
        assert_eq!(
            ed.actions(false, false, None)["ruler_beats"].checked,
            Some(true)
        );
        let out = ed.event(&event(39, 2, 1, "ruler_samples", vec![]), 1);
        assert!(!out.changed, "the window's, no edit");
        let told = serde_json::to_string(&out.answer).unwrap();
        assert!(told.contains("\"ruler\":\"samples\""), "{told}");
        assert!(told.contains("\"ruler_strip\":false"), "{told}");
        assert_eq!(
            ed.actions(false, false, None)["ruler_samples"].checked,
            Some(true)
        );
    }

    #[test]
    fn a_locate_places_the_cursor_and_edits_nothing() {
        let mut ed = editor();
        let out = ed.event(&event(41, 2, 1, "locate", vec![json!(4.0 * SR)]), 1);
        assert_eq!(out.locate, Some(4.0));
        assert_eq!(ed.cursor(), Some(4.0));
        assert!(!out.changed);
        assert_eq!(
            Value::Object(ed.props(41)),
            json!({ "cursor": 4.0 * SR, "ruler": "beats" }),
            "the ruler reports the editor's own copy"
        );
    }

    /// **A box of samples is drawn as either of the audio editor's pictures
    /// of its take**: the View menu's two verbs set the window's `view`,
    /// which edits nothing, and the menu shows which is on.
    #[test]
    fn the_view_menu_draws_the_boxes_of_samples_as_waveforms_or_spectrograms() {
        let mut ed = editor();
        assert_eq!(ed.picture(40)["view"], "trace");
        let out = ed.event(&event(39, 2, 1, VIEW_SPECTROGRAM_VERB, vec![]), 1);
        assert!(!out.changed && out.record.is_none());
        assert_eq!(ed.picture(40)["view"], "spectrogram");
        let told = serde_json::to_value(&out.answer).unwrap();
        assert!(told.to_string().contains("spectrogram"), "{told}");
        let actions = ed.actions(false, false, None);
        assert!(actions[VIEW_SPECTROGRAM_VERB].checked == Some(true));
        assert!(actions[VIEW_WAVE_VERB].checked == Some(false));
        ed.event(&event(39, 3, 1, VIEW_WAVE_VERB, vec![]), 1);
        assert_eq!(ed.picture(40)["view"], "trace");
    }

    /// **A selection let go puts the position cursor at its start**, the
    /// window told, and asks a loop in progress to follow it with the head
    /// placed in it; a sweep still moving asks only the loop.
    #[test]
    fn a_selection_let_go_puts_the_cursor_at_its_start() {
        let mut ed = editor();
        let moving = ed.event(
            &event(40, 2, 1, "selection", vec![json!(2.0 * SR), json!(SR)]),
            1,
        );
        assert_eq!(moving.follow, Some(false));
        assert_eq!(moving.locate, None, "the cursor stays while it sweeps");
        let let_go = ed.event(
            &event(
                40,
                3,
                1,
                "selection",
                vec![json!(2.0 * SR), json!(SR), json!(true)],
            ),
            1,
        );
        assert_eq!(let_go.follow, Some(true));
        assert_eq!(let_go.locate, Some(2.0));
        assert_eq!(ed.cursor(), Some(2.0));
        assert_eq!(ed.range(), Some((2.0, 3.0)));
        let told = serde_json::to_value(&let_go.answer).unwrap();
        assert!(
            told.to_string().contains("\"cursor\""),
            "the window is told: {told}"
        );
    }

    /// **An edit made against a picture that is gone is refused**, and the
    /// multitrack's props go back with the reason.
    #[test]
    fn an_overtaken_edit_is_handed_the_picture_back() {
        let mut ed = editor();
        // The version moved to 3 by a route no event of this view took.
        let moved = boxes(&[("12", "10", 2.0), ("13", "10", 4.0), ("22", "20", 0.0)]);
        let out = ed.event(&event(40, 4, 2, "clips", moved), 3);
        assert_eq!(out.turn, Kind::Stale);
        assert_eq!(position(&ed), 0.0, "not applied");
        match out.answer {
            Some(Answer::Push {
                seq,
                reason,
                corrections,
                ..
            }) => {
                assert_eq!(seq, 4);
                assert!(reason.is_some());
                assert_eq!(corrections[0].widget, 40);
                assert!(corrections[0].props.get("clips").is_some());
            }
            other => panic!("a push, not {other:?}"),
        }
    }

    /// An undo is the window's, and the caller walks it.
    #[test]
    fn a_step_is_handed_to_the_caller() {
        let mut ed = editor();
        let out = ed.event(&event(39, 9, 1, "undo", vec![]), 1);
        assert_eq!((out.turn, out.seq, out.redo), (Kind::Step, 9, false));
        assert!(
            ed.event(&event(77, 1, 1, "clips", vec![]), 1).turn == Kind::Nothing,
            "a widget this editor did not draw"
        );
    }

    /// **A name the host minted is answered with the one the multitrack kept**, and
    /// a multitrack whose names the host already has is answered with nothing.
    #[test]
    fn a_minted_name_is_answered_with_the_picture() {
        let mut ed = editor();
        assert_eq!(ed.settle(1), Answer::Silent);
        let split = vec![
            json!("12"),
            json!("10"),
            json!(0.0),
            json!(SR),
            json!(0.0),
            json!(""),
            json!(7),
            json!("12 2"),
            json!("10"),
            json!(SR),
            json!(SR),
            json!(0.0),
            json!(""),
            json!(7),
        ]
        .into_iter()
        .chain(boxes(&[("13", "10", 4.0), ("22", "20", 0.0)]))
        .collect();
        let out = ed.event(&event(40, 6, 1, "clips", split), 1);
        assert!(out.changed);
        assert!(
            matches!(ed.settle(out.version), Answer::Push { seq: 0, .. }),
            "the new box's id goes back"
        );
        assert_eq!(ed.settle(out.version), Answer::Silent, "and only once");
    }

    /// The JSON door reaches the same verbs.
    #[test]
    fn the_door_reaches_the_verbs() {
        let mut ed = editor();
        let out: Value = serde_json::from_str(&call_json(
            &mut ed,
            &json!({"verb": "event", "addr": "/gui_event",
                    "args": [41, 2, 1, "locate", 96000.0], "version": 1})
            .to_string(),
        ))
        .unwrap();
        assert_eq!(out["locate"], json!(2.0));
        assert_eq!(out["turn"], json!("route"));
        let ack: Value =
            serde_json::from_str(&call_json(&mut ed, r#"{"verb": "announce", "version": 4}"#))
                .unwrap();
        assert_eq!(ack["answer"], json!("ack"));
        assert_eq!(ack["docVersion"], json!(4));
        assert_eq!(call_json(&mut ed, r#"{"verb": "nope"}"#), "{}");
    }

    fn with_controls() -> MultitrackEditor {
        let mut ed = editor();
        ed.set_controls(Some(TransportIds {
            row: 0,
            rewind: 50,
            play: 51,
            stop: 52,
            clock: 53,
        }));
        ed
    }

    /// **The transport row's buttons are the editor's**, and each is one verb:
    /// play/pause toggles, stop goes back to the mark.
    #[test]
    fn the_buttons_are_the_transport_verbs() {
        let mut ed = with_controls();
        ed.set_cursor(Some(3.0));
        let play = ed.event(&event(51, 7, 1, "click", vec![]), 1);
        assert_eq!(play.transport, Some(TransportVerb::Toggle));
        assert!(matches!(play.answer, Some(Answer::Ack { seq: 7, .. })));
        let stop = ed.event(&event(52, 8, 1, "click", vec![]), 1);
        assert_eq!(stop.transport, Some(TransportVerb::Stop { mark: 3.0 }));
        assert!(
            ed.event(&event(51, 9, 1, "press", vec![]), 1)
                .transport
                .is_none(),
            "a press is not a click"
        );
    }

    /// **The menu's entries are the window's verbs**: a pick reported as
    /// `"menu" <verb>` is read as the verb, so Pause and Stop do what the
    /// transport row's buttons do, and a close from the menu is a close.
    #[test]
    fn the_menu_s_entries_are_the_window_s_verbs() {
        let mut ed = editor();
        ed.set_cursor(Some(3.0));
        let menu = |verb: &str| event(39, 5, 1, crate::turn::MENU, vec![json!(verb)]);
        assert_eq!(
            ed.event(&menu("pause"), 1).transport,
            Some(TransportVerb::Toggle)
        );
        assert_eq!(
            ed.event(&menu("stop"), 1).transport,
            Some(TransportVerb::Stop { mark: 3.0 })
        );
        assert_eq!(
            ed.event(&menu(STOP_AT_END_VERB), 1).transport,
            Some(TransportVerb::StopAtEnd)
        );
        // ...and the key, which reaches the window as the bare verb
        assert_eq!(
            ed.event(&event(39, 6, 1, STOP_AT_END_VERB, vec![]), 1)
                .transport,
            Some(TransportVerb::StopAtEnd)
        );
        assert!(ed.event(&menu(crate::closing::VERB), 1).close);
        // a widget's own `menu` is not the window's
        assert!(
            ed.event(&event(40, 6, 1, crate::turn::MENU, vec![json!("stop")]), 1)
                .transport
                .is_none()
        );
    }

    /// **A Save with no file asks where**: an untitled session's Save opens
    /// the Save as form, whose `OK` names the path the holder writes; a
    /// session with a file is saved at once, and a read-only window saves
    /// nothing and says so.
    #[test]
    fn an_untitled_save_opens_the_save_as_form() {
        let mut ed = editor();
        let ids: crate::forms::Ids = dialogs::names()
            .into_iter()
            .enumerate()
            .map(|(k, name)| (name, 60 + k as i32))
            .collect();
        ed.set_dialogs(ids.clone());
        ed.set_saves(true);
        ed.set_untitled(true);
        let window = ed.window(40, 41);
        let stack = &window["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == ids["stack"])
            .expect("the window holds the dialogs' stack");
        assert_eq!(stack["index"], 0);
        assert!(
            window["menu"].to_string().contains("save_as"),
            "File has Save as"
        );
        let shown = ed.event(&event(39, 1, 1, SAVE_VERB, vec![]), 1);
        assert!(!shown.save, "nothing written yet");
        let Some(Answer::Push { corrections, .. }) = shown.answer else {
            panic!("the form shown: {:?}", shown.answer)
        };
        assert!(
            corrections
                .iter()
                .any(|c| c.widget == i64::from(ids["stack"]) && c.props["index"] == 1),
            "the form is up: {corrections:?}"
        );
        ed.event(
            &event(i64::from(ids["file:path"]), 2, 1, "/songs/a.json", vec![]),
            1,
        );
        let accepted = ed.event(&event(i64::from(ids["file:ok"]), 3, 1, "click", vec![]), 1);
        assert_eq!(accepted.save_as.as_deref(), Some("/songs/a.json"));
        // with a file, Save writes at once
        ed.set_untitled(false);
        assert!(ed.event(&event(39, 4, 1, SAVE_VERB, vec![]), 1).save);
        // Save as asks again, whatever the session's file
        assert!(
            ed.event(&event(39, 5, 1, SAVE_AS_VERB, vec![]), 1)
                .answer
                .is_some()
        );
        // a read-only window writes nothing
        ed.set_saves(false);
        let refused = ed.event(&event(39, 6, 1, SAVE_VERB, vec![]), 1);
        assert!(!refused.save);
    }

    /// **A curve shown from the context menu is an edit**: the `shown` report
    /// is read into the multitrack's own verb, and it is recorded so an undo
    /// hides it again.
    #[test]
    fn a_curve_shown_from_the_menu_is_recorded() {
        use clausters_document::Opaque;
        use clausters_document::multitrack::Automation;
        let mut ed = editor();
        ed.multitrack().tracks[0]
            .automation
            .push(Automation::new(NodeId(30), Opaque(json!({"port": "gain"}))));
        let out = ed.event(&event(40, 7, 1, "shown", vec![json!("30"), json!(1)]), 1);
        let record = out.record.expect("recorded");
        assert_eq!(record.label, "show a curve");
        assert!(ed.multitrack().automation(NodeId(30)).unwrap().visible);
    }

    /// **Rewind puts the mark at the top**, cues the transport there, and tells
    /// the host where the cursor now is.
    #[test]
    fn rewind_puts_the_mark_at_the_top() {
        let mut ed = with_controls();
        ed.set_cursor(Some(12.0));
        let out = ed.event(&event(50, 4, 1, "click", vec![]), 1);
        assert_eq!(ed.cursor(), Some(0.0));
        assert_eq!(out.cursor, Some(0.0));
        assert_eq!(out.transport, Some(TransportVerb::Cue { secs: 0.0 }));
        match out.answer {
            Some(Answer::Push { corrections, .. }) => {
                assert_eq!(corrections[0].widget, 40);
                assert_eq!(corrections[0].props, json!({ "cursor": 0.0 }));
            }
            other => panic!("a push, not {other:?}"),
        }
        assert_eq!(
            ed.rewind(1).transport,
            Some(TransportVerb::Cue { secs: 0.0 })
        );
    }

    /// **The space bar is the window's play/stop**, whatever is under the
    /// pointer, and a stop goes back to the position cursor.
    #[test]
    fn the_space_bar_plays_and_stops() {
        let mut ed = editor();
        let out = ed.event(&event(39, 2, 1, PLAY_KEY, vec![]), 1);
        assert_eq!(
            out.transport,
            Some(TransportVerb::PlayStop {
                mark: 0.0,
                range: None,
                looping: false
            })
        );
        assert!(matches!(out.answer, Some(Answer::Ack { seq: 2, .. })));
    }

    /// **A hand on a control over a curve is heard and never written**: it asks
    /// the playback to hold the parameter, and nothing reaches the history.
    #[test]
    fn a_hand_held_parameter_is_no_edit() {
        let mut ed = editor();
        let before = ed.multitrack().clone();
        let held = ed.event(
            &event(
                41,
                2,
                1,
                "port_hold",
                vec![json!("10"), json!("gain"), json!(0.5)],
            ),
            1,
        );
        assert_eq!(held.port_hold, Some((10, "gain".to_string(), Some(0.5))));
        assert!(!held.changed && held.record.is_none());
        let gone = ed.event(
            &event(41, 3, 1, "port_hold", vec![json!("10"), json!("gain")]),
            1,
        );
        assert_eq!(gone.port_hold, Some((10, "gain".to_string(), None)));
        assert_eq!(*ed.multitrack(), before);
    }

    /// **`L` is the window's too**: the loop switch's new state and the time
    /// range, for the pass in progress.
    #[test]
    fn the_loop_key_asks_the_pass_in_progress_to_follow() {
        let mut ed = editor();
        let out = ed.event(&event(39, 2, 1, LOOP_KEY, vec![json!(1)]), 1);
        assert_eq!(ed.actions(false, false, None)["loop"].checked, Some(true));
        assert_eq!(
            out.transport,
            Some(TransportVerb::Loop {
                range: None,
                looping: true
            })
        );
    }

    /// **The span is one state, the hand's and the client's**: the client
    /// sets it through `span`, the multitrack draws it as the band a sweep
    /// leaves, and the space bar plays it.
    #[test]
    fn a_span_set_by_the_client_is_drawn_and_played_as_a_sweep_is() {
        let mut ed = editor();
        call_json(&mut ed, r#"{"verb": "span", "span": [1.0, 3.0]}"#);
        let props = ed.props(40);
        assert_eq!(
            (props["sel_start"].clone(), props["sel_len"].clone()),
            (json!(SR), json!(2.0 * SR))
        );
        let out = ed.event(&event(39, 3, 1, PLAY_KEY, vec![json!(0)]), 1);
        assert_eq!(
            out.transport,
            Some(TransportVerb::PlayStop {
                mark: 0.0,
                range: Some((1.0, 3.0)),
                looping: false
            })
        );
        call_json(&mut ed, r#"{"verb": "span", "span": null}"#);
        assert_eq!(ed.props(40)["sel_len"], json!(0.0), "no span, no band");
    }

    /// **A sweep's time range is what the space bar plays**, with the loop
    /// switch the host sends beside it; a range of no length is none.
    #[test]
    fn the_space_bar_plays_the_time_range_a_sweep_left() {
        let mut ed = editor();
        ed.event(
            &event(41, 2, 1, "selection", vec![json!(SR), json!(2.0 * SR)]),
            1,
        );
        let out = ed.event(&event(39, 3, 1, PLAY_KEY, vec![json!(1)]), 1);
        assert_eq!(
            out.transport,
            Some(TransportVerb::PlayStop {
                mark: 0.0,
                range: Some((1.0, 3.0)),
                looping: true
            })
        );
        ed.event(
            &event(41, 4, 1, "selection", vec![json!(SR), json!(0.0)]),
            1,
        );
        let out = ed.event(&event(39, 5, 1, PLAY_KEY, vec![json!(0)]), 1);
        assert!(matches!(
            out.transport,
            Some(TransportVerb::PlayStop {
                range: None,
                looping: false,
                ..
            })
        ));
    }

    /// The clock reads the position and the multitrack's end, in the multitrack's beats.
    #[test]
    fn the_clock_reads_where_the_multitrack_is() {
        let ed = editor();
        assert_eq!(ed.clock(1.5), "   1.500 s   of 6.000 s");
    }
}
