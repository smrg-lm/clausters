//! **One undo order over every application open in it**: the editing context.
//!
//! A piece, a take and whatever else is on screen walk one history, and no
//! application holds one of its own. An editor is always opened *in* an
//! [`Editing`]; one opened alone is an `Editing` with one member, and two opened
//! in the same one walk one order — which is the whole of how two applications
//! share an undo.
//!
//! # What the context does, and what it hands back
//!
//! It holds the [`History`], the version a host names back, and its
//! **members**: the editors it opened, and the structures the crate does not
//! apply (a client's curve, its notes) as external members. A **turn** is its:
//! the member reads the gesture, the context records the entry under that
//! member's structure and moves the version. A **step** is its too: it walks the
//! pile, hands each leg to the member that owns the structure, puts the cursor
//! back when nothing could apply it, and moves the version.
//!
//! What a running system does is handed back ([`Effect`]): a piece to write
//! back, the steps a write takes on a buffer, the payloads an external member
//! applies, and the answers every window is corrected with.
//!
//! # A structure is named by its key
//!
//! A member declares what it edits — a take by its buffer, a piece by the piece
//! — and a second member declaring the same key is the same structure in the
//! order. Two windows over one take are one structure, so an undo in either
//! walks the edit the other made.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_document::Opaque;
use clausters_document::history::{Direction, Entry, History, Step, StructureId};
use clausters_document::multitrack::edit::MULTITRACK;
use clausters_document::samples::SAMPLES;
use clausters_editing::conversation::Answer;

use crate::multitrack::editor::{self as multitrack, MultitrackEditor};
use crate::samples::editor::{self as samples, SamplesEditor};
use crate::turn::{Event, Kind, Record, int};

/// The version an unedited context is at. One rather than zero, because zero is
/// what an edit means by *unstated* when it names the state it was made against.
pub const FIRST_VERSION: i64 = 1;

/// The most values one write carries, where the caller did not say.
pub const DEFAULT_CHUNK: usize = 8192;

/// A member's number in its context.
pub type MemberId = u32;

/// **What sits in a context.**
#[derive(Clone, Debug)]
pub enum Member {
    /// A multitrack editor over a piece.
    Multitrack(Box<MultitrackEditor>),
    /// A samples editor over a take.
    Samples(SamplesEditor),
    /// A structure the crate does not apply: the context records and walks for
    /// it, and hands its legs back to be applied.
    External {
        /// The vocabulary its payloads are written in.
        domain: String,
    },
}

impl Member {
    fn domain(&self) -> String {
        match self {
            Member::Multitrack(_) => MULTITRACK.into(),
            Member::Samples(_) => SAMPLES.into(),
            Member::External { domain } => domain.clone(),
        }
    }
}

/// One member, and the structure it is in the order.
#[derive(Clone, Debug)]
struct Seat {
    member: Member,
    structure: StructureId,
}

/// **What a member's turn came to**, in that member's own terms.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Outcome {
    /// A multitrack editor's.
    Multitrack(multitrack::Outcome),
    /// A samples editor's.
    Samples(samples::Outcome),
}

impl Outcome {
    fn kind(&self) -> Kind {
        match self {
            Outcome::Multitrack(o) => o.turn,
            Outcome::Samples(o) => o.turn,
        }
    }

    fn record(&self) -> Option<&Record> {
        match self {
            Outcome::Multitrack(o) => o.record.as_ref(),
            Outcome::Samples(o) => o.record.as_ref(),
        }
    }

    fn changed(&self) -> bool {
        match self {
            Outcome::Multitrack(o) => o.changed,
            Outcome::Samples(o) => o.changed,
        }
    }

    fn version(&self) -> i64 {
        match self {
            Outcome::Multitrack(o) => o.version,
            Outcome::Samples(o) => o.version,
        }
    }

    fn step(&self) -> (i64, bool) {
        match self {
            Outcome::Multitrack(o) => (o.seq, o.redo),
            Outcome::Samples(o) => (o.seq, o.redo),
        }
    }

    fn answer(&mut self, answer: Answer) {
        match self {
            Outcome::Multitrack(o) => o.answer = Some(answer),
            Outcome::Samples(o) => o.answer = Some(answer),
        }
    }
}

/// **What one message to a member came to.**
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Turned {
    /// The member's own outcome. Its entry is already recorded; its answer is
    /// the acknowledgement to send.
    pub outcome: Outcome,
    /// The step of the history the message asked for, when it was an undo or a
    /// redo, already taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stepped: Option<Stepped>,
    /// What every **other** member's window is corrected with, when the turn
    /// changed what they draw.
    pub corrections: Vec<Corrected>,
    /// The version after the turn.
    pub version: i64,
}

/// One member's window, and what it is told.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Corrected {
    /// The member.
    pub member: MemberId,
    /// The answer to send its window.
    pub answer: Answer,
}

/// **What a step does**, for the caller to carry out.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Effect {
    /// A multitrack editor applied a payload to its piece.
    #[serde(rename_all = "camelCase")]
    Multitrack {
        /// The member.
        member: MemberId,
        /// What applying it did: the piece as it now stands, a source minted.
        applied: multitrack::Applied,
    },
    /// A write to a take's buffer, as the steps a runner walks.
    #[serde(rename_all = "camelCase")]
    Samples {
        /// The member.
        member: MemberId,
        /// The steps.
        steps: Value,
    },
    /// Payloads an external member applies, in order.
    #[serde(rename_all = "camelCase")]
    External {
        /// The member.
        member: MemberId,
        /// The payloads.
        payloads: Vec<Value>,
    },
}

/// **What a step of the history came to.**
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stepped {
    /// Whether anything moved.
    pub stepped: bool,
    /// Why nothing did, where it is worth saying: the entry named structures no
    /// member could apply it to, and it is still there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// What each member carries out.
    pub effects: Vec<Effect>,
    /// What every member's window is corrected with.
    pub corrections: Vec<Corrected>,
    /// The version after the step.
    pub version: i64,
}

/// **The editing context**: a history, its version, and the members it holds.
#[derive(Debug)]
pub struct Editing {
    history: History,
    version: i64,
    chunk: usize,
    seats: Vec<Seat>,
    keys: HashMap<String, StructureId>,
}

impl Default for Editing {
    fn default() -> Self {
        Self::new(DEFAULT_CHUNK)
    }
}

impl Editing {
    /// An empty context, whose writes carry at most `chunk` values a message.
    pub fn new(chunk: usize) -> Self {
        Self {
            history: History::new(),
            version: FIRST_VERSION,
            chunk: chunk.max(1),
            seats: Vec::new(),
            keys: HashMap::new(),
        }
    }

    /// The version a host names back.
    pub fn version(&self) -> i64 {
        self.version
    }

    /// Whether there is an edit to step back over.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether there is an undone edit to step forward into.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// What an undo would be called.
    pub fn undo_label(&self) -> Option<String> {
        self.history.undo_label()
    }

    /// What a redo would be called.
    pub fn redo_label(&self) -> Option<String> {
        self.history.redo_label()
    }

    /// **Takes a member in**, as the structure `key` names: the identity a
    /// member with the same key already has, or a new one.
    pub fn join(&mut self, key: &str, member: Member) -> MemberId {
        let history = &mut self.history;
        let structure = *self
            .keys
            .entry(key.to_string())
            .or_insert_with(|| history.register(member.domain()));
        self.seats.push(Seat { member, structure });
        (self.seats.len() - 1) as MemberId
    }

    /// A member, to read or to hand the facts it draws from.
    pub fn member_mut(&mut self, member: MemberId) -> Option<&mut Member> {
        self.seats
            .get_mut(member as usize)
            .map(|seat| &mut seat.member)
    }

    /// **Records an entry** under `member`'s structure — what an external
    /// member hands over for an edit it applied itself. Answers whether the
    /// history took it, and moves the version when it did.
    pub fn record(&mut self, member: MemberId, record: &Record) -> bool {
        let Some(structure) = self.seats.get(member as usize).map(|s| s.structure) else {
            return false;
        };
        let taken = self.record_at(structure, record);
        if taken {
            self.version += 1;
        }
        taken
    }

    fn record_at(&mut self, structure: StructureId, record: &Record) -> bool {
        let mut entry: Option<Entry> = None;
        for leg in &record.legs {
            let Ok(forward) = serde_json::from_value::<Step>(leg.forward.clone()) else {
                continue;
            };
            let backward = Opaque(leg.backward.clone());
            let next = match entry.take() {
                Some(e) => e.and(structure, forward, backward),
                None => Entry::new(record.label.clone(), structure, forward, backward),
            };
            entry = Some(if leg.key.is_empty() {
                next
            } else {
                next.keyed(leg.key.clone())
            });
        }
        entry.is_some_and(|entry| self.history.record(entry))
    }

    /// **One message to a member**, read, recorded and answered.
    ///
    /// `None` for a member that is not there or does not read messages (an
    /// external one).
    pub fn event(&mut self, member: MemberId, event: &Event) -> Option<Turned> {
        let version = self.version;
        let seat = self.seats.get_mut(member as usize)?;
        let structure = seat.structure;
        let mut outcome = match &mut seat.member {
            Member::Multitrack(editor) => Outcome::Multitrack(editor.event(event, version)),
            Member::Samples(editor) => Outcome::Samples(editor.event(event, version)),
            Member::External { .. } => return None,
        };
        if let Some(record) = outcome.record().cloned() {
            self.record_at(structure, &record);
        }
        let mut corrections = Vec::new();
        if outcome.changed() {
            self.version = outcome.version();
            corrections = self.corrections(Some(member));
        }
        let mut stepped = None;
        if outcome.kind() == Kind::Step {
            let (seq, redo) = outcome.step();
            let step = self.step(if redo {
                Direction::Redo
            } else {
                Direction::Undo
            });
            let version = self.version;
            let reason = step.reason.clone();
            if let Some(seat) = self.seats.get(member as usize) {
                match &seat.member {
                    Member::Multitrack(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::Samples(editor) => {
                        outcome.answer(editor.acknowledge(seq, version, reason));
                    }
                    Member::External { .. } => {}
                }
            }
            stepped = Some(step);
        }
        Some(Turned {
            outcome,
            stepped,
            corrections,
            version: self.version,
        })
    }

    /// **One step of the history**, handed round the members.
    ///
    /// Each leg goes to the members holding its structure: every multitrack
    /// editor over the piece applies it to the piece it draws; a write to a take
    /// and an external member's payloads are carried out once, by the caller. A
    /// step nothing could apply is not a step: the cursor goes back, and the
    /// entry is still there.
    pub fn step(&mut self, direction: Direction) -> Stepped {
        let label = match direction {
            Direction::Undo => self.history.undo_label(),
            Direction::Redo => self.history.redo_label(),
        }
        .unwrap_or_default();
        let mut out = Stepped {
            stepped: false,
            reason: None,
            effects: Vec::new(),
            corrections: Vec::new(),
            version: self.version,
        };
        let Some(walked) = self.history.walk(direction) else {
            return out;
        };
        let chunk = self.chunk;
        let mut applied = false;
        for (structure, payloads) in &walked.legs {
            let mut written = false;
            for (id, seat) in self.seats.iter_mut().enumerate() {
                if seat.structure != *structure {
                    continue;
                }
                let member = id as MemberId;
                match &mut seat.member {
                    Member::Multitrack(editor) => {
                        for payload in payloads {
                            let done = editor.apply(&payload.0);
                            applied |= done.applied;
                            out.effects.push(Effect::Multitrack {
                                member,
                                applied: done,
                            });
                        }
                    }
                    Member::Samples(editor) if !written => {
                        written = true;
                        for payload in payloads {
                            let steps = editor.write(&payload.0, chunk);
                            applied |= steps.as_array().is_some_and(|s| !s.is_empty());
                            out.effects.push(Effect::Samples { member, steps });
                        }
                    }
                    Member::External { .. } if !written => {
                        written = true;
                        applied |= !payloads.is_empty();
                        out.effects.push(Effect::External {
                            member,
                            payloads: payloads.iter().map(|p| p.0.clone()).collect(),
                        });
                    }
                    _ => {}
                }
            }
        }
        if !applied {
            self.history.step(match direction {
                Direction::Undo => Direction::Redo,
                Direction::Redo => Direction::Undo,
            });
            out.effects.clear();
            out.reason = Some(format!("{label}: nothing here can put that edit back"));
            return out;
        }
        self.version += 1;
        out.stepped = true;
        out.version = self.version;
        out.corrections = self.corrections(None);
        out
    }

    /// What every member's window but `except` is corrected with.
    fn corrections(&mut self, except: Option<MemberId>) -> Vec<Corrected> {
        let version = self.version;
        let mut out = Vec::new();
        for (id, seat) in self.seats.iter_mut().enumerate() {
            let member = id as MemberId;
            if Some(member) == except {
                continue;
            }
            let answer = match &mut seat.member {
                Member::Multitrack(editor) => editor.resync_all(version),
                Member::Samples(editor) => editor.resync_all(version),
                Member::External { .. } => continue,
            };
            if answer != Answer::Silent {
                out.push(Corrected { member, answer });
            }
        }
        out
    }
}

/// A record as a client hands one over for an external member.
#[derive(Deserialize)]
struct Recorded {
    #[serde(default)]
    label: String,
    #[serde(default)]
    legs: Vec<RecordedLeg>,
}

#[derive(Deserialize)]
struct RecordedLeg {
    forward: Value,
    backward: Value,
    #[serde(default)]
    key: Option<String>,
}

/// **One verb of a context, over JSON** — the door both clients bind.
///
/// `request` names the `verb` and carries its arguments:
///
/// - `openMultitrack` — `key`, and what a multitrack editor is built from
///   (`clausters_apps::multitrack::editor::new_json`, the version aside):
///   `{"member"}`, or `{"error"}`.
/// - `openSamples` — `key`, and what a samples editor is built from
///   (`clausters_apps::samples::editor::new_json`): `{"member"}`, or
///   `{"error"}`.
/// - `external` — `key`, `domain`: `{"member"}`.
/// - `event` — `member`, `addr`, `args`: a [`Turned`], or `null`.
/// - `step` — `direction` (`"undo"` or `"redo"`): a [`Stepped`].
/// - `record` — `member`, `label`, `legs` (`forward`, `backward`, `key`):
///   `{"recorded"}`.
/// - `member` — `member`, and a verb of that member's own door with its
///   arguments, the version filled in: what that door answers. `event` and
///   `apply` are the context's and answer `{}` here.
/// - `state` — `{"version", "canUndo", "canRedo", "undoLabel", "redoLabel"}`.
///
/// An unknown verb answers `{}`.
pub fn call_json(editing: &mut Editing, request: &str) -> String {
    let Ok(mut request) = serde_json::from_str::<Value>(request) else {
        return "{}".into();
    };
    let get = |request: &Value, key: &str| request.get(key).cloned().unwrap_or(Value::Null);
    let member = int(&get(&request, "member")) as MemberId;
    let key = get(&request, "key")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let verb = request
        .get("verb")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(map) = request.as_object_mut() {
        map.insert("version".into(), json!(editing.version));
    }
    match verb.as_str() {
        "openMultitrack" => match multitrack::new_json(&request.to_string()) {
            Some(editor) => {
                json!({ "member": editing.join(&key, Member::Multitrack(Box::new(editor))) })
                    .to_string()
            }
            None => json!({ "error": "the request names no piece" }).to_string(),
        },
        "openSamples" => match samples::new_json(&request.to_string()) {
            Ok(editor) => {
                json!({ "member": editing.join(&key, Member::Samples(editor)) }).to_string()
            }
            Err(error) => json!({ "error": error }).to_string(),
        },
        "external" => {
            let domain = get(&request, "domain")
                .as_str()
                .unwrap_or_default()
                .to_string();
            json!({ "member": editing.join(&key, Member::External { domain }) }).to_string()
        }
        "event" => {
            let event = serde_json::from_value::<Event>(request.clone()).unwrap_or_default();
            to_json(&editing.event(member, &event))
        }
        "step" => {
            let direction = get(&request, "direction")
                .as_str()
                .and_then(Direction::parse)
                .unwrap_or(Direction::Undo);
            to_json(&editing.step(direction))
        }
        "record" => {
            let recorded = serde_json::from_value::<Recorded>(request.clone()).ok();
            let taken = recorded.is_some_and(|r| {
                let record = Record {
                    label: r.label,
                    legs: r
                        .legs
                        .into_iter()
                        .map(|leg| crate::turn::Leg {
                            forward: leg.forward,
                            backward: leg.backward,
                            key: leg.key.unwrap_or_default(),
                        })
                        .collect(),
                };
                editing.record(member, &record)
            });
            json!({ "recorded": taken }).to_string()
        }
        "member" => {
            let inner = get(&request, "call")
                .get("verb")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if matches!(inner.as_str(), "event" | "apply") {
                return "{}".into();
            }
            let mut call = get(&request, "call");
            if let Some(map) = call.as_object_mut() {
                map.insert("version".into(), json!(editing.version));
            }
            match editing.member_mut(member) {
                Some(Member::Multitrack(editor)) => {
                    multitrack::call_json(editor, &call.to_string())
                }
                Some(Member::Samples(editor)) => samples::call_json(editor, &call.to_string()),
                _ => "{}".into(),
            }
        }
        "state" => json!({
            "version": editing.version(),
            "canUndo": editing.can_undo(),
            "canRedo": editing.can_redo(),
            "undoLabel": editing.undo_label(),
            "redoLabel": editing.redo_label(),
        })
        .to_string(),
        _ => "{}".into(),
    }
}

/// A turn's answer as JSON.
fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::multitrack::{Content, Multitrack, Region, Track};
    use clausters_document::{
        Beat, Lifetime, NodeId, SegmentRef, SegmentSource, SourceId, SourceRef,
    };

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

    /// A piece of one track holding box 12, drawn by widget 40 in window 39.
    fn a_piece() -> Member {
        let mut track = Track::new(NodeId(10), NodeId(11));
        track.lanes[0].regions = vec![region(12, 0.0)];
        let piece = Multitrack {
            tracks: vec![track],
            ..Multitrack::default()
        };
        let mut editor = MultitrackEditor::new(piece, SR, 60.0, FIRST_VERSION);
        editor.set_sources(HashMap::from([(SourceId(1), 7)]));
        editor.chrome(
            None,
            crate::multitrack::Transport::Unnumbered,
            "piece",
            (1000, 560),
        );
        editor.window(40, 41);
        editor.set_window(Some(39));
        Member::Multitrack(Box::new(editor))
    }

    /// A mono take in buffer 7, drawn by widget 50 in window 49.
    fn a_take() -> Member {
        let mut editor = samples::new_json(r#"{"buffer": 7, "version": 1}"#).unwrap();
        samples::call_json(&mut editor, r#"{"verb": "window", "widget": 50}"#);
        samples::call_json(&mut editor, r#"{"verb": "sync", "window": 49}"#);
        Member::Samples(editor)
    }

    fn event(widget: i64, seq: i64, tag: &str, values: Vec<Value>) -> Event {
        let mut args = vec![json!(widget), json!(seq), json!(0), json!(tag)];
        args.extend(values);
        Event {
            addr: "/gui_event".into(),
            args,
        }
    }

    fn moved_to(at: f64) -> Vec<Value> {
        vec![
            json!("12"),
            json!("10"),
            json!(at * SR),
            json!(2.0 * SR),
            json!(0.0),
            json!(""),
            json!(7),
        ]
    }

    fn stroke(value: f64, was: f64) -> Vec<Value> {
        vec![json!(0), json!(2), json!([value]), json!([was])]
    }

    fn position(editing: &mut Editing, member: MemberId) -> f64 {
        match editing.member_mut(member) {
            Some(Member::Multitrack(editor)) => {
                editor.piece().tracks[0].lanes[0].regions[0].position.0
            }
            _ => f64::NAN,
        }
    }

    fn written(stepped: &Stepped) -> Vec<Value> {
        stepped
            .effects
            .iter()
            .filter_map(|e| match e {
                Effect::Samples { steps, .. } => Some(steps[0]["send"]["args"][2].clone()),
                _ => None,
            })
            .collect()
    }

    /// **An editor open alone is a context of one**: its stroke is recorded
    /// there, and an undo hands back the write that puts the take back.
    #[test]
    fn an_editor_open_alone_undoes_through_a_context_of_one() {
        let mut editing = Editing::default();
        let take = editing.join("buffer:7", a_take());
        let turned = editing
            .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        assert!(turned.outcome.changed());
        assert_eq!(turned.version, 2);
        assert!(editing.can_undo());
        assert_eq!(editing.undo_label().as_deref(), Some("draw the samples"));

        let undone = editing.step(Direction::Undo);
        assert!(undone.stepped);
        assert_eq!(undone.version, 3);
        assert_eq!(written(&undone), [json!({"b": [0.0]})]);
        assert_eq!(
            undone.corrections,
            [Corrected {
                member: take,
                answer: Answer::Push {
                    seq: 0,
                    doc_version: 3,
                    reason: None,
                    corrections: vec![clausters_editing::conversation::Correction {
                        widget: 50,
                        props: json!({"reload": 1}),
                    }],
                },
            }],
            "the window reads the take again"
        );
        let redone = editing.step(Direction::Redo);
        assert_eq!(written(&redone), [json!({"b": [0.5]})]);
    }

    /// **Two applications in one context walk one order**: a box moved, a
    /// stroke drawn, a box moved again, and undo walks the three back in that
    /// order, each handed to its own member.
    #[test]
    fn two_applications_in_one_context_walk_one_order() {
        let mut editing = Editing::default();
        let piece = editing.join("piece", a_piece());
        let take = editing.join("buffer:7", a_take());

        let first = editing
            .event(piece, &event(40, 1, "clips", moved_to(2.0)))
            .unwrap();
        assert!(first.outcome.changed());
        assert!(
            first.corrections.iter().any(|c| c.member == take),
            "the other window is corrected"
        );
        editing
            .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        editing
            .event(piece, &event(40, 2, "clips", moved_to(4.0)))
            .unwrap();
        assert_eq!(position(&mut editing, piece), 4.0);

        let back = editing.step(Direction::Undo);
        assert!(matches!(back.effects[..], [Effect::Multitrack { .. }]));
        assert_eq!(position(&mut editing, piece), 2.0);

        let back = editing.step(Direction::Undo);
        assert_eq!(
            written(&back),
            [json!({"b": [0.0]})],
            "the stroke, in between"
        );
        assert_eq!(position(&mut editing, piece), 2.0);

        editing.step(Direction::Undo);
        assert_eq!(position(&mut editing, piece), 0.0);
        assert!(!editing.can_undo());

        editing.step(Direction::Redo);
        let forward = editing.step(Direction::Redo);
        assert_eq!(written(&forward), [json!({"b": [0.5]})]);
    }

    /// **Two windows over one take are one structure**: a stroke in one is
    /// undone once, whichever asks.
    #[test]
    fn two_windows_over_one_take_are_one_structure() {
        let mut editing = Editing::default();
        let left = editing.join("buffer:7", a_take());
        let right = editing.join("buffer:7", a_take());
        editing
            .event(left, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        let undone = editing.step(Direction::Undo);
        assert_eq!(written(&undone).len(), 1, "one write, not one per window");
        assert!(undone.corrections.iter().any(|c| c.member == right));
    }

    /// **An external member's legs are handed back**, in the same order as the
    /// rest.
    #[test]
    fn an_external_members_legs_are_handed_back() {
        let mut editing = Editing::default();
        let curve = editing.join(
            "curve",
            Member::External {
                domain: "points".into(),
            },
        );
        let take = editing.join("buffer:7", a_take());
        let record = Record {
            label: "draw the curve".into(),
            legs: vec![crate::turn::Leg {
                forward: json!({"edit": {"points": [1.0]}}),
                backward: json!({"points": [0.0]}),
                key: String::new(),
            }],
        };
        assert!(editing.record(curve, &record));
        assert_eq!(editing.version(), 2);
        editing
            .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();

        editing.step(Direction::Undo);
        let back = editing.step(Direction::Undo);
        assert_eq!(
            back.effects,
            [Effect::External {
                member: curve,
                payloads: vec![json!({"points": [0.0]})],
            }]
        );
    }

    /// **A step nothing could apply is not a step**: the cursor goes back, the
    /// entry is still on top, and the reason names it.
    #[test]
    fn a_step_nothing_could_apply_puts_the_cursor_back() {
        let mut editing = Editing::default();
        let piece = editing.join("piece", a_piece());
        let record = Record {
            label: "move nothing".into(),
            legs: vec![crate::turn::Leg {
                forward: json!({"edit": {"nonsense": true}}),
                backward: json!({"nonsense": true}),
                key: String::new(),
            }],
        };
        editing.record(piece, &record);
        let version = editing.version();
        let refused = editing.step(Direction::Undo);
        assert!(!refused.stepped);
        assert!(
            refused
                .reason
                .as_deref()
                .unwrap()
                .starts_with("move nothing")
        );
        assert!(editing.can_undo(), "still there");
        assert_eq!(editing.version(), version);
    }

    /// **An undo asked of a window is the context's step**, answered with the
    /// acknowledgement of the key that asked.
    #[test]
    fn an_undo_from_a_window_steps_the_context() {
        let mut editing = Editing::default();
        let take = editing.join("buffer:7", a_take());
        editing
            .event(take, &event(50, 1, "draw", stroke(0.5, 0.0)))
            .unwrap();
        let undo = Event {
            addr: "/gui_event".into(),
            args: vec![json!(49), json!(2), json!(0), json!("undo")],
        };
        let turned = editing.event(take, &undo).unwrap();
        let stepped = turned.stepped.expect("a step");
        assert!(stepped.stepped);
        let Outcome::Samples(outcome) = &turned.outcome else {
            panic!()
        };
        assert_eq!(
            outcome.answer,
            Some(Answer::Ack {
                seq: 2,
                doc_version: 3,
                reason: None
            })
        );
    }

    /// The JSON door opens members, turns, steps and reports the state.
    #[test]
    fn the_door_opens_turns_and_steps() {
        let mut editing = Editing::default();
        let call = |editing: &mut Editing, request: Value| -> Value {
            serde_json::from_str(&call_json(editing, &request.to_string())).unwrap()
        };
        let take = call(
            &mut editing,
            json!({"verb": "openSamples", "key": "buffer:7", "buffer": 7}),
        )["member"]
            .clone();
        call(
            &mut editing,
            json!({"verb": "member", "member": take, "call": {"verb": "window", "widget": 50}}),
        );
        let turned = call(
            &mut editing,
            json!({"verb": "event", "member": take, "addr": "/gui_event",
                   "args": [50, 1, 0, "draw", 0, 2, [0.5], [0.0]]}),
        );
        assert_eq!(turned["version"], 2);
        assert_eq!(turned["outcome"]["edit"]["values"], json!([0.5]));
        let state = call(&mut editing, json!({"verb": "state"}));
        assert_eq!(state["undoLabel"], "draw the samples");
        let stepped = call(&mut editing, json!({"verb": "step", "direction": "undo"}));
        assert_eq!(stepped["effects"][0]["kind"], "samples");
        assert_eq!(
            call(
                &mut editing,
                json!({"verb": "openSamples", "key": "x", "layers": []})
            )["error"]
                .as_str()
                .map(|e| e.contains("measures something")),
            Some(true)
        );
    }
}
