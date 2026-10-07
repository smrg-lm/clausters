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
use clausters_document::multitrack::edit::MULTITRACK;
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::{Opaque, SourceId, domain};
use clausters_editing::conversation::{self, Answer, Conversation, Correction};
use clausters_editing::multitrack::{self as projection, Look};

use super::{Meter, Transport, TransportIds, Window};
use crate::turn::{self, Converse, int, number};

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
    /// The selection a sweep left, in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Value>,
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
    /// **Whether to close the window** ([`crate::closing`]): a close asked of
    /// it, with nothing to lose or nothing to ask in.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub close: bool,
}

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
    }

    fn asks_to_close(&self) -> bool {
        self.asks
    }

    fn unsaved(&self) -> bool {
        self.unsaved
    }

    fn ask_to_close(&mut self, _out: &mut Outcome) -> (Option<String>, Vec<Correction>) {
        match self.close_form {
            Some(ids) => (None, crate::closing::shown(&ids, true)),
            None => (
                Some(
                    "this window has changes that are not saved, and no form to ask about them"
                        .into(),
                ),
                Vec::new(),
            ),
        }
    }

    fn resync(&mut self, widget: i64) -> Vec<Correction> {
        MultitrackEditor::resync(self, widget)
    }

    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        // **The close form's answer**: Save writes and then closes, Don't
        // save closes, Cancel keeps the window -- and the form goes down
        if let Some(ids) = self.close_form.filter(|ids| ids.contains(widget)) {
            use crate::closing::Choice;
            match ids.read(widget, tag) {
                Some(Choice::Save) => {
                    out.save = true;
                    out.close = true;
                }
                Some(Choice::Discard) => out.close = true,
                Some(Choice::Cancel) => {}
                None => return (None, Vec::new()),
            }
            return (None, crate::closing::shown(&ids, false));
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
    /// Cue a stopped transport at `secs`, and leave a rolling one alone.
    Cue {
        /// Where, in seconds.
        secs: f64,
    },
}

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
    /// **The multitrack, shared with whoever holds it** -- a script's handle
    /// over it edits what this editor draws, and a turn here is what that
    /// handle reads.
    multitrack: super::Shared,
    rate: f64,
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
    /// **The close form's widgets**, when the holder numbered them
    /// ([`crate::closing`]); and whether it is up.
    close_form: Option<crate::closing::Ids>,
    /// Whether the window is the work's one holder, so a close with
    /// something unsaved asks first -- a standalone host's, never a client's.
    asks: bool,
    /// Whether the holder's work has changes its file does not hold, as the
    /// holder last said: the session is the holder's, not this editor's.
    unsaved: bool,
    /// Whether the window is composed with no chrome ([`Window::bare`]).
    bare: bool,
    /// **How a box of notes is drawn**: `"roll"`, or `"score"` -- its page,
    /// each note at its time. The window's own, like its zoom.
    notes_view: String,
    /// **The engraver the boxes' pages are engraved with**: a score its
    /// holder hands over for that and nothing else ([`bind_engraver`]). With
    /// none, a box of notes is drawn as its roll whatever the view.
    ///
    /// [`bind_engraver`]: MultitrackEditor::bind_engraver
    #[cfg(feature = "notation")]
    engraver: Option<Engraver>,
    /// The pages engraved, by the source each is of, with the sequence each
    /// was engraved from: a page is engraved again when its sequence is no
    /// longer that one, and not once a correction.
    #[cfg(feature = "notation")]
    pages: std::cell::RefCell<HashMap<SourceId, Engraved>>,
}

/// The score a holder handed over to engrave with.
#[cfg(feature = "notation")]
#[derive(Clone)]
struct Engraver(crate::score::Shared);

#[cfg(feature = "notation")]
impl std::fmt::Debug for Engraver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Engraver")
    }
}

/// A sequence's page, as it was engraved.
#[cfg(feature = "notation")]
#[derive(Clone, Debug)]
struct Engraved {
    /// The sequence it is the page of.
    of: clausters_document::events::EventSequence,
    /// The display list a `score` widget takes.
    page: Map<String, Value>,
    /// Each item of the page that sounds: the id it is drawn under, and the
    /// beat it starts at.
    onsets: Vec<(String, f64)>,
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
            multitrack,
            rate,
            sources: HashMap::new(),
            lengths: HashMap::new(),
            segments: HashMap::new(),
            sequences: HashMap::new(),
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
            close_form: None,
            asks: false,
            unsaved: false,
            bare: false,
            notes_view: ROLL.into(),
            #[cfg(feature = "notation")]
            engraver: None,
            #[cfg(feature = "notation")]
            pages: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// **How a box of notes is drawn**: `"roll"`, or `"score"`. A word that
    /// is neither leaves it as it is, and says so.
    pub fn set_notes_view(&mut self, view: &str) -> bool {
        if view != ROLL && view != SCORE {
            return false;
        }
        self.notes_view = view.to_string();
        true
    }

    /// How a box of notes is drawn.
    pub fn notes_view(&self) -> &str {
        &self.notes_view
    }

    /// **Hands the editor the engraver its boxes' pages are engraved with**:
    /// a score nobody else reads, which the editor loads with each sequence
    /// in turn. The engraver is a port a holder has and this crate does not.
    #[cfg(feature = "notation")]
    pub fn bind_engraver(&mut self, score: crate::score::Shared) {
        self.engraver = Some(Engraver(score));
        self.pages.borrow_mut().clear();
    }

    /// **The pages of the boxes of notes**, a box's name to its page with
    /// the time of each of its notes in the sequence's frames, which the box
    /// reads through its window -- or `None` where the boxes are drawn
    /// as rolls, or nothing engraves here.
    #[cfg(feature = "notation")]
    fn scores(&self) -> Option<Value> {
        use clausters_core::notation::{PageSetup, View, default_interpretation, layout_options};
        use clausters_core::tempomap::TempoMap;
        use clausters_document::events::transcription::{Transcription, read};
        use clausters_document::multitrack::picture;

        if self.notes_view != SCORE {
            return None;
        }
        let engraver = &self.engraver.as_ref()?.0;
        let how = Transcription::default();
        let beat_unit = how.beat_unit.max(1) as f64;
        let mut out = Map::new();
        let multitrack = self.multitrack();
        for box_ in picture::boxes(&multitrack) {
            let Some(source) = box_.source else {
                continue;
            };
            let Some(shared) = self.sequences.get(&source) else {
                continue;
            };
            let sequence = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let mut pages = self.pages.borrow_mut();
            let stale = pages.get(&source).is_none_or(|kept| kept.of != sequence);
            if stale {
                let Ok(got) = read(&sequence, &how, &default_interpretation()) else {
                    continue;
                };
                let Ok(mei) = clausters_core::notation::sheet_to_mei(&got.sheet) else {
                    continue;
                };
                let mut held = engraver.lock().unwrap_or_else(|e| e.into_inner());
                if !held.load(&mei) {
                    continue;
                }
                // one system as long as the music: the page of a box
                held.relayout(&layout_options(&PageSetup::default(), View::Continuous));
                let mut page = crate::score::drawing(&held.pages(0.0, false));
                page.remove("cursors");
                // every item that sounds, at the beat the page writes it
                let mut onsets = Vec::new();
                for voice in got.sheet.voices() {
                    let mut at = clausters_core::ratio::Ratio::ZERO;
                    for item in &voice.items {
                        if item.sounds() {
                            let beat = at.to_f64() * beat_unit;
                            onsets.push((format!("n{}", item.id()), beat));
                            // a chord is drawn as its pitches
                            onsets.push((format!("n{}-p1", item.id()), beat));
                        }
                        at = at + item.dur();
                    }
                }
                pages.insert(
                    source,
                    Engraved {
                        of: sequence.clone(),
                        page,
                        onsets,
                    },
                );
            }
            let Some(kept) = pages.get(&source) else {
                continue;
            };
            // where each note stands: its second in the sequence, in the
            // sequence's frames, as a note of the box's roll is -- the box
            // reads both through its window
            let map = sequence
                .tempo_map
                .clone()
                .unwrap_or_else(|| TempoMap::new(1.0));
            let anchors: Vec<Value> = kept
                .onsets
                .iter()
                .flat_map(|(id, beat)| [json!(id), json!(map.secs_at(*beat) * self.rate)])
                .collect();
            let mut page = kept.page.clone();
            page.insert("anchors".into(), Value::Array(anchors));
            out.insert(box_.region.0.to_string(), Value::Object(page));
        }
        Some(Value::Object(out))
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
    /// that numbers id-less widgets on the way out hands back.
    pub fn set_controls(&mut self, ids: Option<TransportIds>) {
        self.controls = ids;
    }

    /// The transport row's ids, once they are known.
    pub fn controls(&self) -> Option<TransportIds> {
        self.controls
    }

    /// The multitrack, held while the guard lives.
    pub fn multitrack(&self) -> std::sync::MutexGuard<'_, Multitrack> {
        self.multitrack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The multitrack as it is shared: what a handle opened over this editor's
    /// structure holds.
    pub fn shared(&self) -> super::Shared {
        self.multitrack.clone()
    }

    /// Replaces the multitrack -- a caller that edited it by a route that was not a
    /// turn of this editor.
    pub fn set_multitrack(&mut self, multitrack: Multitrack) {
        *self.multitrack() = multitrack;
    }

    /// **The joins a caller holds**, by source -- the segments each is made
    /// of, added to the ones this editor minted itself: what a standalone host
    /// opening a session with joins in it hands over.
    pub fn set_segments(
        &mut self,
        segments: HashMap<SourceId, Vec<clausters_document::session::Part>>,
    ) {
        self.segments.extend(segments);
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
            self.segments.insert(made.id, parts);
        }
    }

    /// **The joins an edit names**: the sources `payload` makes something read
    /// that are joins this editor knows. What a leg holds, so a join stays
    /// while an undo or a redo can still name it and comes back to be freed
    /// once none can. A take is not one of them: it is the session's, loaded
    /// from a file, and outlives every edit over it.
    fn joins_named(&self, payload: &Value) -> Vec<u64> {
        clausters_document::multitrack::edit::intent_of(&Opaque(payload.clone()))
            .map(|intent| intent.sources())
            .unwrap_or_default()
            .into_iter()
            .filter(|source| self.segments.contains_key(source))
            .map(|source| source.0)
            .collect()
    }

    /// Whether `source` is a join this editor knows: one it minted, or one a
    /// session it was opened with held.
    pub fn is_join(&self, source: SourceId) -> bool {
        self.segments.contains_key(&source)
    }

    /// **The joins the multitrack reads now**: what a released join is checked
    /// against before it is freed, since a region still windowing it is a
    /// root the history does not know about.
    pub fn joins_read(&self) -> Vec<SourceId> {
        self.multitrack()
            .regions()
            .filter_map(|region| region.content.source())
            .filter(|source| self.segments.contains_key(source))
            .collect()
    }

    /// **Forgets a join** nothing reaches any more: it leaves the editor's
    /// table with its segments, so a later edit cannot read through it.
    pub fn forget_join(&mut self, source: SourceId) {
        self.segments.remove(&source);
        self.sources.remove(&source);
        self.lengths.remove(&source);
    }

    /// **Binds source `source` to a sequence** the caller shares: a region
    /// over that source draws the sequence's notes, as it stands when drawn.
    pub fn bind_sequence(&mut self, source: SourceId, sequence: crate::notes::Shared) {
        self.sequences.insert(source, sequence);
    }

    /// The sequence bound to `source`, when one is.
    pub fn sequence(&self, source: SourceId) -> Option<&crate::notes::Shared> {
        self.sequences.get(&source)
    }

    /// Which buffer each source was read into.
    pub fn set_sources(&mut self, sources: HashMap<SourceId, i64>) {
        self.sources = sources;
    }

    /// How many frames each take holds. A take left out is one whose length
    /// is not known, and a join over it is not checked against it.
    pub fn set_lengths(&mut self, lengths: HashMap<SourceId, u64>) {
        self.lengths = lengths;
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

    /// Places the position cursor, in seconds -- a caller's own verb, like a
    /// rewind.
    pub fn set_cursor(&mut self, secs: Option<f64>) {
        self.cursor = secs;
    }

    /// The window this editor is open in, once it is.
    pub fn set_window(&mut self, window: Option<i32>) {
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
    /// bar and `L` -- and the `pause` and `stop` of its menu.
    pub fn answers(&self, widget: i32, tag: &str) -> bool {
        self.owns(widget)
            || (self.window == Some(widget)
                && matches!(
                    tag,
                    PLAY_KEY
                        | LOOP_KEY
                        | PAUSE_VERB
                        | STOP_VERB
                        | NOTES_ROLL_VERB
                        | NOTES_SCORE_VERB
                ))
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
        let def = self.composed(widget, ruler, super::window);
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
        let mut props = self.composed(multitrack, ruler, |w| super::props(w, widget));
        if widget == multitrack && !props.is_empty() {
            // how a box of notes is drawn, and its page where it is one
            props.insert("notes_view".into(), json!(self.notes_view));
            #[cfg(feature = "notation")]
            if let Some(scores) = self.scores() {
                props.insert("scores".into(), scores);
            }
            // The time range is drawn where the hand sweeps one, so a span set
            // from the client shows as the band a sweep leaves.
            let (start, len) = self
                .range
                .map_or((0.0, 0.0), |(a, b)| (a * self.rate, (b - a) * self.rate));
            props.insert("sel_start".into(), json!(start));
            props.insert("sel_len".into(), json!(len));
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

    /// **Every widget of the window, corrected**, with nothing to retire -- what
    /// a history step leaves behind, and what a second window over the multitrack
    /// is told when this one edited it.
    pub fn resync_all(&mut self, version: i64) -> Answer {
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
            buffers: &self.sources,
            lengths: &self.lengths,
            segments: &self.segments,
            sequences: &self.sequences,
        };
        projection::placed_notes(&self.multitrack(), &table)
    }

    /// The window over the editor's state, handed to `f`.
    fn composed<T>(&self, widget: i32, ruler: i32, f: impl FnOnce(&Window<'_>) -> T) -> T {
        let multitrack = self.multitrack();
        let tempo = projection::tempo_map(&multitrack);
        let table = Table {
            buffers: &self.sources,
            lengths: &self.lengths,
            segments: &self.segments,
            sequences: &self.sequences,
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
            link: self.link,
            cursor: self.cursor,
            meters: &self.meters,
            transport: self.transport,
            title: &self.title,
            size: self.size,
            close_form: self.close_form,
            asks: self.asks,
            bare: self.bare,
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
            out.transport = Some(TransportVerb::Loop {
                range: self.range,
                looping: values.first().is_some_and(|v| number(v) != 0.0),
            });
            return (None, Vec::new());
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
        // **The View menu's two ways to draw a box of notes**: the window's
        // own state, so nothing is recorded and the picture is corrected
        if self.window.map(i64::from) == Some(widget)
            && matches!(tag, NOTES_ROLL_VERB | NOTES_SCORE_VERB)
        {
            self.set_notes_view(if tag == NOTES_SCORE_VERB { SCORE } else { ROLL });
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
        // **A double click on a box of notes asks for its roll.** Nothing is
        // edited here: the caller opens a notes editor over the sequence the
        // box reads, and what that editor does is what reaches a history.
        if tag == "open" {
            let named = values.first().map(crate::turn::text).unwrap_or_default();
            out.open = clausters_document::multitrack::picture::boxes(&self.multitrack())
                .into_iter()
                .find(|b| b.region.0.to_string() == named)
                .and_then(|b| b.source)
                .filter(|source| self.sequences.contains_key(source))
                .map(|source| source.0);
            return (None, Vec::new());
        }
        if NOT_AN_EDIT.contains(&tag) {
            self.observe(tag, values, out);
            return (None, Vec::new());
        }
        let taken = {
            let table = Table {
                buffers: &self.sources,
                lengths: &self.lengths,
                segments: &self.segments,
                sequences: &self.sequences,
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
        let mut legs = Vec::new();
        let mut moved = false;
        for payload in &taken.payloads {
            if let Some(source) = minted(payload) {
                self.learn(&source);
                out.minted.push(source);
            }
            let Some((applied, current)) = self.edit(payload) else {
                continue;
            };
            if !applied {
                continue;
            }
            moved = true;
            if let Some(backward) = current {
                let holds_forward = self.joins_named(payload);
                let holds_backward = self.joins_named(&backward);
                legs.push(Leg {
                    forward: json!({ "edit": payload }),
                    backward,
                    key: domain::coalesce_key(MULTITRACK, &Opaque(payload.clone()))
                        .unwrap_or_default(),
                    holds_forward,
                    holds_backward,
                });
            }
        }
        if moved {
            if !legs.is_empty() {
                out.record = Some(Record { label, legs });
            }
            out.version += 1;
            out.changed = true;
            out.multitrack = serde_json::to_value(&*self.multitrack()).ok();
        }
        (None, Vec::new())
    }

    /// The cursor put back at the top and the transport cued there; answers the
    /// correction that tells the host where the cursor now is.
    fn rewound(&mut self, out: &mut Outcome) -> Vec<Correction> {
        self.cursor = Some(0.0);
        out.cursor = Some(0.0);
        out.transport = Some(TransportVerb::Cue { secs: 0.0 });
        let Some(widget) = self.widget else {
            return Vec::new();
        };
        // The host owns where the cursor *is*, so it is told rather than left
        // to find out on the next redraw.
        let units = self.composed(widget, self.ruler.unwrap_or(i32::MIN), |w| w.cursor_units());
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

/// A box of notes drawn as a roll: the `notes_view` word, and the window's
/// verb that asks for it.
pub const ROLL: &str = "roll";
/// ...and drawn as its page.
pub const SCORE: &str = "score";
/// The window verb that draws the boxes of notes as rolls.
pub const NOTES_ROLL_VERB: &str = "notes_roll";
/// The window verb that draws them as pages.
pub const NOTES_SCORE_VERB: &str = "notes_score";

/// **The window's stop**: halt and go back to the mark -- the menu's Stop.
pub const STOP_VERB: &str = "stop";

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
    /// How a box of notes is drawn: `"roll"`, the default, or `"score"`.
    #[serde(default)]
    notes_view: Option<String>,
}

fn yes() -> bool {
    true
}

/// An [`Outcome`] as JSON.
fn outcome(outcome: &Outcome) -> String {
    serde_json::to_string(outcome).unwrap_or_else(|_| "{}".into())
}

/// An editor built from a JSON request, or `None` for one that names no multitrack.
pub fn new_json(request: &str) -> Option<MultitrackEditor> {
    let parsed: New = serde_json::from_str(request).ok()?;
    let multitrack: Multitrack = serde_json::from_value(parsed.multitrack.clone()).ok()?;
    Some(built(super::shared(multitrack), parsed))
}

/// **An editor over a multitrack a caller shares** -- a binding's handle -- built
/// from the rest of a JSON request; a `multitrack` in it is not read. `None`
/// for a request that does not read.
pub fn over_json(multitrack: super::Shared, request: &str) -> Option<MultitrackEditor> {
    Some(built(multitrack, serde_json::from_str(request).ok()?))
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
    editor.set_bare(!request.chrome);
    if let Some(view) = &request.notes_view {
        editor.set_notes_view(view);
    }
    editor
}

/// **One verb of an editor, over JSON** -- the door both clients bind.
///
/// One door rather than one per verb because the verbs are the application's
/// surface, and a door per verb would be each binding restating it. `request`
/// names the `verb` and carries its arguments:
///
/// - `sync` -- `multitrack`, `sources`, `meters`, `cursor` (beats or `null`),
///   `window`, `controls` (the transport row's ids): the state a caller holds,
///   handed over before the verbs that read it.
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
    let get = |key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let version = get("version").as_i64().unwrap_or(0);
    let answer = |a: Answer| serde_json::to_string(&a).unwrap_or_else(|_| "{}".into());
    match request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "sync" => {
            if let Ok(multitrack) = serde_json::from_value::<Multitrack>(get("multitrack")) {
                editor.set_multitrack(multitrack);
            }
            if request.get("sources").is_some() {
                editor.set_sources(projection::table(&get("sources")));
                editor.set_lengths(projection::lengths(&get("sources")));
            }
            if let Ok(meters) = serde_json::from_value::<Vec<Meter>>(get("meters")) {
                editor.set_meters(meters);
            }
            if request.get("cursor").is_some() {
                editor.set_cursor(get("cursor").as_f64());
            }
            if request.get("window").is_some() {
                editor.set_window(get("window").as_i64().map(|w| w as i32));
            }
            if request.get("controls").is_some() {
                editor.set_controls(serde_json::from_value(get("controls")).ok());
            }
            "{}".into()
        }
        "rewind" => outcome(&editor.rewind(version)),
        "toggle" => outcome(&editor.toggle(version)),
        "stop" => outcome(&editor.stop(version)),
        "clock" => json!({ "text": editor.clock(number(&get("position"))) }).to_string(),
        "window" => editor
            .window(int(&get("widget")) as i32, int(&get("ruler")) as i32)
            .to_string(),
        "setWindow" => {
            editor.set_window(get("window").as_i64().map(|w| w as i32));
            "{}".into()
        }
        "props" => Value::Object(editor.props(int(&get("widget")) as i32)).to_string(),
        "notes" => json!({ "placed": editor.placed_notes() }).to_string(),
        "notesView" => {
            let set = get("view")
                .as_str()
                .is_none_or(|view| editor.set_notes_view(view));
            json!({ "view": editor.notes_view(), "set": set }).to_string()
        }
        "event" => {
            let event = Event {
                addr: get("addr").as_str().unwrap_or_default().to_string(),
                args: get("args").as_array().cloned().unwrap_or_default(),
            };
            outcome(&editor.event(&event, version))
        }
        "apply" => {
            serde_json::to_string(&editor.apply(&get("payload"))).unwrap_or_else(|_| "{}".into())
        }
        "resync" => answer(editor.resync_all(version)),
        "span" => {
            editor.range = get("span")
                .as_array()
                .and_then(|r| Some((r.first()?.as_f64()?, r.get(1)?.as_f64()?)))
                .filter(|(a, b)| b > a);
            "{}".into()
        }
        "settle" => answer(editor.settle(version)),
        "announce" => answer(editor.announce(version)),
        "acknowledge" => answer(editor.acknowledge(
            int(&get("seq")),
            version,
            get("reason").as_str().map(str::to_string),
        )),
        _ => "{}".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::multitrack::{Content, Region, Track};
    use clausters_document::{Lifetime, NodeId, Second, SegmentRef, SegmentSource, SourceRef};

    const SR: f64 = 48_000.0;

    fn region(id: u64, at: f64) -> Region {
        let mut region = Region::new(
            NodeId(id),
            Second(at),
            Second(2.0),
            Content::Unknown(Value::Null),
        );
        region.content = Content::window(SegmentRef {
            source: SegmentSource::Samples(SourceRef {
                source: SourceId(1),
                lifetime: Lifetime::Session,
                generation: 0,
                range: None,
            }),
            start: 0.0,
            duration: 2.0,
        });
        region
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
            region.content = Content::window(SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(1),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
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
        editor.set_sources(HashMap::from([(SourceId(1), 7)]));
        editor.chrome(None, Transport::Unnumbered, "multitrack", (1000, 560));
        editor.window(40, 41);
        editor.set_window(Some(39));

        let first = editor.event(&event(40, 1, 0, "join", vec![json!("12"), json!("13")]), 1);
        let [made] = first.minted.as_slice() else {
            panic!("the first join mints: {:?}", first.minted);
        };
        let made = made["id"].clone();

        // **And the window is told what the join is made of**, so it draws the
        // joined box from the take it reads rather than waiting for a buffer
        // the server has yet to build: two spans of buffer 7, the take's second
        // half and then its first, in the order they play.
        let props = editor.props(40);
        let segments = props["segments"]
            .as_array()
            .unwrap_or_else(|| panic!("segments in {props:?}"));
        // Frames, and frames are whole: a span is a count of them. The fifth
        // of each quintuple is how many frames of the take one frame of the
        // join is -- one here, since the take is at the session's own rate.
        let half = json!(SR as u64);
        assert_eq!(
            segments.as_slice(),
            [
                json!("12"),
                json!(7),
                half.clone(),
                half.clone(),
                json!(1.0),
                json!("12"),
                json!(7),
                json!(0),
                half,
                json!(1.0),
            ]
            .as_slice(),
            "the joined box, drawn from the take it is spans of"
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
    #[test]
    fn a_locate_places_the_cursor_and_edits_nothing() {
        let mut ed = editor();
        let out = ed.event(&event(41, 2, 1, "locate", vec![json!(4.0 * SR)]), 1);
        assert_eq!(out.locate, Some(4.0));
        assert_eq!(ed.cursor(), Some(4.0));
        assert!(!out.changed);
        assert_eq!(
            Value::Object(ed.props(41)),
            json!({ "cursor": 4.0 * SR }),
            "the ruler reports the editor's own copy"
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
        assert!(ed.event(&menu(crate::closing::VERB), 1).close);
        // a widget's own `menu` is not the window's
        assert!(
            ed.event(&event(40, 6, 1, crate::turn::MENU, vec![json!("stop")]), 1)
                .transport
                .is_none()
        );
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

    /// **`L` is the window's too**: the loop switch's new state and the time
    /// range, for the pass in progress.
    #[test]
    fn the_loop_key_asks_the_pass_in_progress_to_follow() {
        let mut ed = editor();
        let out = ed.event(&event(39, 2, 1, LOOP_KEY, vec![json!(1)]), 1);
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
