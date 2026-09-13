//! **The multitrack editor's turns**: what one message from the host is, what
//! it does to the piece, and what the host is answered with.
//!
//! A host reports what a hand did and waits to be told what happened. The
//! editor reads the message ([`Conversation`]), reads the gesture in the
//! piece's vocabulary (the projection's intake), applies each edit with the
//! inverse read before it lands ([`domain::edit`]), keeps where the reader put
//! the position cursor, and answers — an acknowledgement, the corrections a
//! refused or overtaken gesture needs, the reason when one is owed.
//!
//! # What it hands back rather than does
//!
//! The undo order is **not** here. A piece and the boxes entered out of it walk
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
use clausters_document::multitrack::Multitrack;
use clausters_document::multitrack::edit::MULTITRACK;
use clausters_document::view::NOT_AN_EDIT;
use clausters_document::{Opaque, SourceId, domain};
use clausters_editing::conversation::{self, Answer, Conversation, Correction, Message, Turn};
use clausters_editing::multitrack::{self as projection, Look};

use super::{Meter, Transport, TransportIds, Window};

/// What kind of turn a message came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Not this editor's, or not an event at all.
    #[default]
    Nothing,
    /// This editor's window closed.
    Closed,
    /// A walk through the history, which the caller takes and then answers
    /// with [`MultitrackEditor::acknowledge`].
    Step,
    /// An edit made against a picture that is gone: refused, and the picture
    /// handed back.
    Stale,
    /// A gesture, read and answered.
    Route,
}

/// One structure's share of an entry to record: how to redo it, how to put it
/// back, and what makes two of them the same thing done the same way.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Leg {
    /// `{"edit": <payload>}`.
    pub forward: Value,
    /// The payload that puts the piece back, read before the edit landed.
    pub backward: Value,
    /// The coalesce key, empty for an edit that never coalesces.
    pub key: String,
}

/// An entry for the history the caller keeps: one gesture, however many edits
/// it took.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Record {
    /// What an undo menu calls it.
    pub label: String,
    /// The piece's legs, in the order they were applied.
    pub legs: Vec<Leg>,
}

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
    /// Whether the piece changed.
    pub changed: bool,
    /// The version after the turn.
    pub version: i64,
    /// The piece as it now stands, when it changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub piece: Option<Value>,
    /// The sources an edit minted, in the order the edits named them.
    pub minted: Vec<Value>,
    /// Where the position cursor was placed, in beats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locate: Option<f64>,
    /// The selection a sweep left, in beats.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<Value>,
    /// The box a double click entered, by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enter: Option<String>,
}

/// What one payload of a history step did to the piece.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Applied {
    /// Whether anything moved.
    pub applied: bool,
    /// The source it minted, if it minted one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minted: Option<Value>,
    /// The piece as it now stands, when it moved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub piece: Option<Value>,
}

/// One message from the host: its address and its arguments,
/// `<id> <seq> <version> <tag> <payload…>`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Event {
    /// `"/gui_event"` or `"/gui_closed"`.
    pub addr: String,
    /// The arguments, in order.
    #[serde(default)]
    pub args: Vec<Value>,
}

/// **The multitrack editor**: a piece, the window it is drawn in, and one
/// view's end of the conversation with the host.
#[derive(Clone, Debug)]
pub struct MultitrackEditor {
    piece: Multitrack,
    rate: f64,
    default_bpm: f64,
    sources: HashMap<SourceId, i64>,
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
}

impl MultitrackEditor {
    /// An editor over `piece`, drawn on an axis of `rate` frames a second and
    /// read at `default_bpm` where the piece states no tempo, whose history is
    /// at `version`.
    pub fn new(piece: Multitrack, rate: f64, default_bpm: f64, version: i64) -> Self {
        Self {
            piece,
            rate,
            default_bpm,
            sources: HashMap::new(),
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
        self.title = title.to_string();
        self.size = size;
    }

    /// The piece.
    pub fn piece(&self) -> &Multitrack {
        &self.piece
    }

    /// Replaces the piece — a caller that edited it by a route that was not a
    /// turn of this editor.
    pub fn set_piece(&mut self, piece: Multitrack) {
        self.piece = piece;
    }

    /// Which server buffer each source was read into.
    pub fn set_sources(&mut self, sources: HashMap<SourceId, i64>) {
        self.sources = sources;
    }

    /// Where each track's meters are read from.
    pub fn set_meters(&mut self, meters: Vec<Meter>) {
        self.meters = meters;
    }

    /// The position cursor, in beats.
    pub fn cursor(&self) -> Option<f64> {
        self.cursor
    }

    /// Places the position cursor, in beats — a caller's own verb, like a
    /// rewind.
    pub fn set_cursor(&mut self, beats: Option<f64>) {
        self.cursor = beats;
    }

    /// The window this editor is open in, once it is.
    pub fn set_window(&mut self, window: Option<i32>) {
        self.window = window;
    }

    /// The id of the piece's own widget, once a window has been composed.
    pub fn widget(&self) -> Option<i32> {
        self.widget
    }

    /// The id of the ruler, once a window has been composed.
    pub fn ruler(&self) -> Option<i32> {
        self.ruler
    }

    /// Whether this editor drew `widget`.
    pub fn owns(&self, widget: i32) -> bool {
        self.widget == Some(widget) || self.ruler == Some(widget)
    }

    /// **The window**, composed around the piece's widget and ruler ids, which
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
        let (piece, ruler) = (
            self.widget.unwrap_or(widget),
            self.ruler.unwrap_or(i32::MIN),
        );
        let props = self.composed(piece, ruler, |w| super::props(w, widget));
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
        let message = Message {
            addr: event.addr.clone(),
            argc: args.len(),
            widget,
            seq: args.get(1).map_or(0, int),
            against: args.get(2).map_or(0, int),
            tag: args.get(3).map(text).unwrap_or_default(),
            version,
            is_window: self.window.is_some()
                && (args.is_empty() || i64::from(self.window.unwrap_or_default()) == widget),
            owns: i32::try_from(widget).is_ok_and(|w| self.owns(w)),
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

    /// **One payload of a history step**, applied to the piece — the inverse
    /// of an edit this editor recorded, or the edit again.
    pub fn apply(&mut self, payload: &Value) -> Applied {
        let mut out = Applied {
            minted: minted(payload),
            ..Applied::default()
        };
        if let Some((true, _)) = self.edit(payload) {
            out.applied = true;
            out.piece = serde_json::to_value(&self.piece).ok();
        }
        out
    }

    /// **Every widget of the window, corrected**, with nothing to retire — what
    /// a history step leaves behind, and what a second window over the piece
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

    /// **A name the host minted is answered with the one the piece kept.**
    ///
    /// A gesture is normally answered with an acknowledgement alone, because the
    /// report described the result. Where the host **makes** something — a
    /// track from a double click, a box from a split — the host mints the word
    /// and the document mints the id, and a name the piece does not know is
    /// read as something new on the next report. So when what the host was last
    /// told differs from what the piece now holds, the piece goes back whole.
    ///
    /// Called once a changed turn has been carried out — after a minted source
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
        let tempo = projection::tempo_map(&self.piece, self.default_bpm);
        let look = Look {
            tempo: &tempo,
            rate: self.rate,
            sources: &self.sources,
        };
        f(&Window {
            piece: &self.piece,
            look: &look,
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

    /// What the piece calls its rows, its boxes and its curves.
    fn names(&self) -> [HashSet<String>; 3] {
        let named = projection::names(&self.piece);
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

    /// The beat a position on the axis falls on: the frame rounded the way
    /// every client rounds it, then the piece's own map.
    fn beats_at(&self, units: f64) -> f64 {
        let tempo = projection::tempo_map(&self.piece, self.default_bpm);
        tempo.beats_at(samples_to_secs(units.round() as i64, self.rate))
    }

    /// One gesture onto the piece: screen state, the editor's own, or an edit.
    /// Answers the reason and the corrections the acknowledgement carries.
    fn route(
        &mut self,
        widget: i64,
        tag: &str,
        values: &[Value],
        out: &mut Outcome,
    ) -> (Option<String>, Vec<Correction>) {
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
            let tempo = projection::tempo_map(&self.piece, self.default_bpm);
            let look = Look {
                tempo: &tempo,
                rate: self.rate,
                sources: &self.sources,
            };
            projection::intake(&self.piece, tag, values, &look)
        };
        if taken.payloads.is_empty() {
            // Nothing, or a refusal. A refusal says why and hands the widget back
            // what it should be drawing, so the picture stops agreeing with the
            // hand instead of with the piece.
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
            out.minted.extend(minted(payload));
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
            out.piece = serde_json::to_value(&self.piece).ok();
        }
        (None, Vec::new())
    }

    /// Applies one payload, answering whether it moved anything and the payload
    /// that puts it back. `None` for a payload the piece cannot read.
    fn edit(&mut self, payload: &Value) -> Option<(bool, Option<Value>)> {
        let state = Opaque(serde_json::to_value(&self.piece).ok()?);
        let edited = domain::edit(MULTITRACK, &state, &Opaque(payload.clone()))?;
        let current = edited.current.map(|c| c.0);
        if !edited.applied {
            return Some((false, current));
        }
        self.piece = serde_json::from_value(edited.state.0).ok()?;
        Some((true, current))
    }

    /// A tag that says what the view is looking at rather than what changed.
    fn observe(&mut self, tag: &str, values: &[Value], out: &mut Outcome) {
        match tag {
            // A click on the time ruler: the reader put the position cursor
            // there. It is not a seek -- the playhead is never placed -- and
            // what it means for a transport is the caller's.
            "locate" if !values.is_empty() => {
                let beat = self.beats_at(number(&values[0]));
                self.cursor = Some(beat);
                out.locate = Some(beat);
            }
            "selection" => {
                let at = |i: usize| values.get(i).map_or(0.0, |v| self.beats_at(number(v)));
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

/// The source a payload minted, when it names one.
fn minted(payload: &Value) -> Option<Value> {
    payload
        .get("source")
        .filter(|s| s.get("id").is_some_and(|id| !id.is_null()))
        .cloned()
}

fn int(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|f| f as i64))
        .unwrap_or(0)
}

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// What [`call_json`] builds an editor from: the piece, its axis and its
/// sources, the window's chrome, and the version its history is at.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct New {
    piece: Value,
    #[serde(default)]
    rate: f64,
    #[serde(default)]
    default_bpm: f64,
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

/// An editor built from a JSON request, or `None` for one that names no piece.
pub fn new_json(request: &str) -> Option<MultitrackEditor> {
    let request: New = serde_json::from_str(request).ok()?;
    let piece: Multitrack = serde_json::from_value(request.piece).ok()?;
    let mut editor =
        MultitrackEditor::new(piece, request.rate, request.default_bpm, request.version);
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

/// **One verb of an editor, over JSON** — the door both clients bind.
///
/// One door rather than one per verb because the verbs are the application's
/// surface, and a door per verb would be each binding restating it. `request`
/// names the `verb` and carries its arguments:
///
/// - `sync` — `piece`, `sources`, `meters`, `cursor` (beats or `null`): the
///   state a caller holds, handed over before the verbs that read it.
/// - `window` — `widget`, `ruler`: the window, as a GuiDef.
/// - `setWindow` — `window` (an id or `null`).
/// - `props` — `widget`.
/// - `event` — `addr`, `args`, `version`: an [`Outcome`].
/// - `apply` — `payload`: an [`Applied`].
/// - `resync`, `settle`, `announce` — `version`: an [`Answer`].
/// - `acknowledge` — `seq`, `version`, `reason`: an [`Answer`].
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
            if let Ok(piece) = serde_json::from_value::<Multitrack>(get("piece")) {
                editor.set_piece(piece);
            }
            if request.get("sources").is_some() {
                editor.set_sources(projection::table(&get("sources")));
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
            "{}".into()
        }
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
            serde_json::to_string(&editor.event(&event, version)).unwrap_or_else(|_| "{}".into())
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
    use clausters_document::{Beat, Lifetime, NodeId, SegmentRef, SegmentSource, SourceRef};

    const SR: f64 = 48_000.0;

    fn region(id: u64, at: f64) -> Region {
        let mut region = Region::new(
            NodeId(id),
            Beat(at),
            Beat(2.0),
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
        let piece = Multitrack {
            tracks: vec![first, second],
            ..Multitrack::default()
        };
        let mut editor = MultitrackEditor::new(piece, SR, 60.0, 1);
        editor.set_sources(HashMap::from([(SourceId(1), 7)]));
        editor.chrome(None, Transport::Unnumbered, "piece", (1000, 560));
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
        editor.piece().tracks[0].lanes[0].regions[0].position.0
    }

    /// **A move is applied, recorded with its inverse and acknowledged**, and
    /// the inverse puts the piece back.
    #[test]
    fn a_move_reaches_the_piece_and_its_inverse_puts_it_back() {
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

    /// **The position cursor is kept in beats, told, and is not an edit.**
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
    /// piece's props go back with the reason.
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

    /// **A name the host minted is answered with the one the piece kept**, and
    /// a piece whose names the host already has is answered with nothing.
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
}
