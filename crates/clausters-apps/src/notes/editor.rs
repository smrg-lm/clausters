//! **The notes editor, one turn at a time**: what a gesture on the roll does to
//! the sequence, the entry it leaves, and what the window is corrected with.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use clausters_document::events::{self, EventsIntent};
use clausters_document::history::Editable;
use clausters_document::{EventSequence, Opaque};
use clausters_editing::conversation::{self, Answer, Conversation, Correction};
use clausters_editing::notes::{self as roll, Axis, YDomain};

use super::{Shared, correction, props, window};
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
}

turn::turned!(Outcome);

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
}

/// What a notes editor is opened with, as the context's door reads it.
#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Opened {
    rate: f64,
    editable: bool,
    domain: Option<YDomain>,
    title: String,
    w: i64,
    h: i64,
    version: i64,
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
        let drawn = props(&self.held(), &self.domain, self.rate, self.editable);
        window(drawn, widget, &self.title, self.size)
    }

    /// What `widget` is corrected with, or nothing for one that is not the
    /// roll.
    fn resync_widget(&self, widget: i64) -> Vec<Correction> {
        if self.widget.map(i64::from) != Some(widget) {
            return Vec::new();
        }
        vec![Correction {
            widget,
            props: Value::Object(correction(&self.held(), &self.domain, self.rate)),
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
        if !matches!(tag, "notes" | "osc") {
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
            let intake = roll::intake(&sequence, tag, values, &axis, &self.domain);
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
    }

    fn owns(&self, widget: i64, _tag: &str) -> bool {
        self.widget.map(i64::from) == Some(widget)
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
        self.gesture(widget, tag, values, out)
    }
}

/// **A notes editor from JSON**: `{"rate", "editable", "domain", "title", "w",
/// "h", "version"}` over `sequence`.
pub fn new_json(sequence: Shared, request: &str) -> NotesEditor {
    let opened: Opened = serde_json::from_str(request).unwrap_or_default();
    let mut editor = NotesEditor::new(sequence, opened.rate, opened.version);
    editor.editable = opened.editable;
    if let Some(domain) = opened.domain {
        editor.domain = domain;
    }
    editor.title = opened.title;
    editor.size = (opened.w, opened.h);
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
/// - `window` -- `widget`: the GuiDef, the roll under that id.
/// - `props` -- `widget`: what it is corrected with (`{}` for another widget).
/// - `sync` -- `window` (the id it is open in, or `null`), `rate`, `editable`,
///   `domain`, `title`, `w`, `h`: `{}`.
/// - `state` -- the sequence, whole.
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
        "window" => editor.window(widget).to_string(),
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
            if let Some(domain) = request
                .get("domain")
                .and_then(|d| serde_json::from_value(d.clone()).ok())
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
        _ => "{}".into(),
    }
}

#[cfg(test)]
mod tests;
