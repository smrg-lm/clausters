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
//! The undo order is **not** here. A multitrack and the boxes entered out of it walk
//! one history, and the editors of those boxes are not applications yet, so the
//! history stays with whoever holds all of them: each turn answers the entry to
//! record ([`Record`]), and a step of that history comes back as payloads to
//! [`MultitrackEditor::apply`]. The version the host names back is the same
//! history's counter, so it comes in with every turn and goes out moved.
//!
//! What a running system does is handed back too: a source an edit minted is
//! made where the samples are, a placed cursor cues a transport, an entered box
//! opens an editor. [`Outcome`] names each, and the caller carries them out.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use clausters_core::tempoclock::samples_to_secs;
use clausters_document::multitrack::edit::MULTITRACK;
use clausters_document::multitrack::{Content, Multitrack};
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::{Opaque, SourceId, domain};
use clausters_editing::conversation::{self, Answer, Conversation, Correction, Message, Turn};
use clausters_editing::multitrack::{self as projection, Look};

use super::{Meter, Transport, TransportIds, Window};
use crate::turn::{int, number, text};

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
    /// The box a double click entered, by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enter: Option<String>,
    /// What the transport is asked to do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<TransportVerb>,
    /// Where the position cursor now is, in seconds, when the turn moved it by a
    /// verb of its own rather than by a hand on the ruler (which is `locate`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<f64>,
}

/// **What a turn asks the transport to do.** The editor decides what a button,
/// the space bar and a rewind mean; the caller sends the steps its playback
/// answers for the verb.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(tag = "verb", rename_all = "camelCase")]
pub enum TransportVerb {
    /// Play, or pause where it stands -- whichever the transport is not doing.
    Toggle,
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
    multitrack: Multitrack,
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
    meters: Vec<Meter>,
    link: Option<i64>,
    transport: Transport,
    title: String,
    size: (i64, i64),
    cursor: Option<f64>,
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
}

impl MultitrackEditor {
    /// An editor over `multitrack`, drawn on an axis of `rate` frames a second, whose
    /// history is at `version`.
    pub fn new(multitrack: Multitrack, rate: f64, version: i64) -> Self {
        Self {
            multitrack,
            rate,
            sources: HashMap::new(),
            lengths: HashMap::new(),
            segments: HashMap::new(),
            meters: Vec::new(),
            link: None,
            transport: Transport::Absent,
            title: String::new(),
            size: (0, 0),
            cursor: None,
            window: None,
            widget: None,
            ruler: None,
            conversation: Conversation::new(version),
            told: None,
            controls: None,
        }
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

    /// The multitrack.
    pub fn multitrack(&self) -> &Multitrack {
        &self.multitrack
    }

    /// Replaces the multitrack -- a caller that edited it by a route that was not a
    /// turn of this editor.
    pub fn set_multitrack(&mut self, multitrack: Multitrack) {
        self.multitrack = multitrack;
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
    /// one of its widgets, or its window's own `play` -- the space bar.
    pub fn answers(&self, widget: i32, tag: &str) -> bool {
        self.owns(widget) || (self.window == Some(widget) && tag == PLAY_KEY)
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

    /// **What a box opens as**: the source its region is a window onto, and the
    /// title a window over it carries. `None` for a name no region has; a
    /// region that is a window onto nothing loaded answers with no source.
    ///
    /// The multitrack places and a box is entered to edit. What opens is
    /// another application's -- an editor for what the box holds -- so this
    /// answers what to open and not how.
    pub fn box_contents(&self, name: &str) -> Option<BoxContents> {
        let id = name.trim().parse::<u64>().ok()?;
        let region = self
            .multitrack
            .tracks
            .iter()
            .flat_map(|track| &track.lanes)
            .flat_map(|lane| &lane.regions)
            .find(|region| region.id.0 == id)?;
        let source = match &region.content {
            Content::Window { window, .. } => window.source.samples().map(|s| s.source.0),
            _ => None,
        };
        Some(BoxContents {
            source,
            title: region.name.clone().unwrap_or_else(|| name.to_string()),
        })
    }

    /// **What the clock reads** with the multitrack at `position` seconds.
    pub fn clock(&self, position: f64) -> String {
        format!("{position:8.3} s   of {:.3} s", self.multitrack.end().0)
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

    /// **Everything `widget` should be drawing**, for a correction.
    pub fn props(&mut self, widget: i32) -> Map<String, Value> {
        let (multitrack, ruler) = (
            self.widget.unwrap_or(widget),
            self.ruler.unwrap_or(i32::MIN),
        );
        let props = self.composed(multitrack, ruler, |w| super::props(w, widget));
        if self.ruler != Some(widget) {
            self.remember_names();
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

    /// **One message from the host**, read and answered.
    pub fn event(&mut self, event: &Event, version: i64) -> Outcome {
        let args = &event.args;
        let widget = args.first().map_or(0, int);
        let tag = args.get(3).map(text).unwrap_or_default();
        let message = Message {
            addr: event.addr.clone(),
            argc: args.len(),
            widget,
            seq: args.get(1).map_or(0, int),
            against: args.get(2).map_or(0, int),
            owns: i32::try_from(widget).is_ok_and(|w| self.answers(w, &tag)),
            tag,
            version,
            is_window: self.window.is_some()
                && (args.is_empty() || i64::from(self.window.unwrap_or_default()) == widget),
        };
        let mut out = Outcome {
            version,
            ..Outcome::default()
        };
        match self.conversation.read(&message) {
            Turn::Nothing => {}
            Turn::Closed => {
                out.turn = Kind::Closed;
                self.window = None;
            }
            Turn::Step { seq, redo } => {
                out.turn = Kind::Step;
                out.seq = seq;
                out.redo = redo;
            }
            Turn::Stale {
                widget,
                seq,
                reason,
            } => {
                out.turn = Kind::Stale;
                let corrections = self.resync(widget);
                out.answer = Some(conversation::answer(
                    seq,
                    version,
                    Some(reason),
                    corrections,
                ));
            }
            Turn::Route { widget, seq } => {
                out.turn = Kind::Route;
                let values = args.get(4..).unwrap_or_default();
                let (reason, corrections) = self.route(widget, &message.tag, values, &mut out);
                self.conversation.applied(out.version);
                out.answer = Some(conversation::answer(seq, out.version, reason, corrections));
            }
        }
        out
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
            out.multitrack = serde_json::to_value(&self.multitrack).ok();
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

    /// The window over the editor's state, handed to `f`.
    fn composed<T>(&self, widget: i32, ruler: i32, f: impl FnOnce(&Window<'_>) -> T) -> T {
        let tempo = projection::tempo_map(&self.multitrack);
        let table = Table {
            buffers: &self.sources,
            lengths: &self.lengths,
            segments: &self.segments,
        };
        let look = Look {
            rate: self.rate,
            sources: &table,
        };
        f(&Window {
            multitrack: &self.multitrack,
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
        })
    }

    /// What the multitrack calls its rows, its boxes and its curves.
    fn names(&self) -> [HashSet<String>; 3] {
        let named = projection::names(&self.multitrack);
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
    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
        // **The space bar is the window's**, and it is play/pause: a multitrack's
        // readers are resident and follow the transport, so there is nothing
        // under the pointer to aim it at.
        if self.window.map(i64::from) == Some(widget) && tag == PLAY_KEY {
            out.transport = Some(TransportVerb::Toggle);
            return (None, Vec::new());
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
        if NOT_AN_EDIT.contains(&tag) {
            self.observe(tag, values, out);
            return (None, Vec::new());
        }
        // **A box was entered**: opening a window is not something a vocabulary
        // of edits can say, so it is the caller's to do.
        if tag == "enter"
            && let Some(name) = values.first()
        {
            out.enter = Some(text(name));
            return (None, Vec::new());
        }
        let taken = {
            let table = Table {
                buffers: &self.sources,
                lengths: &self.lengths,
                segments: &self.segments,
            };
            let look = Look {
                rate: self.rate,
                sources: &table,
            };
            projection::intake(&self.multitrack, tag, values, &look)
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
                legs.push(Leg {
                    forward: json!({ "edit": payload }),
                    backward,
                    key: domain::coalesce_key(MULTITRACK, &Opaque(payload.clone()))
                        .unwrap_or_default(),
                });
            }
        }
        if moved {
            if !legs.is_empty() {
                out.record = Some(Record { label, legs });
            }
            out.version += 1;
            out.changed = true;
            out.multitrack = serde_json::to_value(&self.multitrack).ok();
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
        let state = Opaque(serde_json::to_value(&self.multitrack).ok()?);
        let edited = domain::edit(MULTITRACK, &state, &Opaque(payload.clone()))?;
        let current = edited.current.map(|c| c.0);
        if !edited.applied {
            return Some((false, current));
        }
        self.multitrack = serde_json::from_value(edited.state.0).ok()?;
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

/// What a box a hand entered opens as.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BoxContents {
    /// The source the region is a window onto, when it is one.
    pub source: Option<u64>,
    /// What a window over it is called: the region's name, or the box's.
    pub title: String,
}

/// The tag the host's space bar reaches a window with.
pub const PLAY_KEY: &str = "play";

/// **The editor's tables as one**: which buffer each source was read into, how
/// many frames each take holds, and the segments each join it knows is made of.
struct Table<'a> {
    buffers: &'a HashMap<SourceId, i64>,
    lengths: &'a HashMap<SourceId, u64>,
    segments: &'a HashMap<SourceId, Vec<clausters_document::session::Part>>,
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
}

/// An [`Outcome`] as JSON.
fn outcome(outcome: &Outcome) -> String {
    serde_json::to_string(outcome).unwrap_or_else(|_| "{}".into())
}

/// An editor built from a JSON request, or `None` for one that names no multitrack.
pub fn new_json(request: &str) -> Option<MultitrackEditor> {
    let request: New = serde_json::from_str(request).ok()?;
    let multitrack: Multitrack = serde_json::from_value(request.multitrack).ok()?;
    let mut editor = MultitrackEditor::new(multitrack, request.rate, request.version);
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
    Some(editor)
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
/// - `box` -- `name`: `{"source", "title"}`, what the box of that name opens
///   as, or `null` for a name no region has.
/// - `window` -- `widget`, `ruler`: the window, as a GuiDef.
/// - `setWindow` -- `window` (an id or `null`).
/// - `props` -- `widget`.
/// - `event` -- `addr`, `args`, `version`: an [`Outcome`].
/// - `apply` -- `payload`: an [`Applied`].
/// - `resync`, `settle`, `announce` -- `version`: an [`Answer`].
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
        "box" => {
            serde_json::to_string(&editor.box_contents(get("name").as_str().unwrap_or_default()))
                .unwrap_or_else(|_| "null".into())
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
        first.lanes[0].regions = vec![region(12, 0.0), region(13, 4.0)];
        let mut second = Track::new(NodeId(20), NodeId(21));
        second.lanes[0].regions = vec![region(22, 0.0)];
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
        editor.multitrack().tracks[0].lanes[0].regions[0].position.0
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
        track.lanes[0].regions = vec![
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

    /// **The space bar is the window's play/pause**, whatever is under the
    /// pointer.
    #[test]
    fn the_space_bar_toggles() {
        let mut ed = editor();
        let out = ed.event(&event(39, 2, 1, PLAY_KEY, vec![]), 1);
        assert_eq!(out.transport, Some(TransportVerb::Toggle));
        assert!(matches!(out.answer, Some(Answer::Ack { seq: 2, .. })));
    }

    /// **A box opens as the source its region windows**, under the region's
    /// name; a name no region has opens nothing.
    #[test]
    fn a_box_opens_as_the_source_it_windows() {
        let ed = editor();
        assert_eq!(
            ed.box_contents("12"),
            Some(BoxContents {
                source: Some(1),
                title: "12".into()
            })
        );
        assert_eq!(ed.box_contents("nowhere"), None);
        assert_eq!(ed.box_contents("99"), None);
    }

    /// The clock reads the position and the multitrack's end, in the multitrack's beats.
    #[test]
    fn the_clock_reads_where_the_multitrack_is() {
        let ed = editor();
        assert_eq!(ed.clock(1.5), "   1.500 s   of 6.000 s");
    }
}
