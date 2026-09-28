//! **The notes editor**: an event sequence on a roll, edited note by note.
//!
//! What it opens is an [`EventSequence`] -- the document's, with an id per
//! event -- and it edits it **in place**: the sequence is shared with whoever
//! holds it (a client's handle, a session's source), so there is no copy to
//! write back and nothing to reconcile. A timeline is opened by rendering it
//! first, which is the client's `edit(timeline)`: what the roll edits is the
//! events the timeline produced, never the timeline.
//!
//! # The window
//!
//! One widget, the roll: the catalogue's `pianoroll` picture
//! ([`clausters_document::view::catalogue`]) over the sequence's notes on its
//! own beats, with the ids of the events beside them (`note_ids`) so every
//! gesture comes back naming the notes it touched. The Y axis is a domain
//! ([`clausters_editing::notes::YDomain`]); the first is MIDI notes.
//!
//! # A turn
//!
//! A gesture is read by [`clausters_editing::notes::intake`] into one edit in
//! the sequence's vocabulary, applied to the shared sequence, and recorded with
//! the sequence as it was as its inverse. The answer corrects the roll, since a
//! note the hand made has an id only once the sequence has given it one.

pub mod editor;

use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use clausters_document::EventSequence;
use clausters_document::view::catalogue::{self, Roll};
use clausters_editing::notes::{self, Axis, YDomain};

/// A sequence the editor and its holder edit together.
pub type Shared = Arc<Mutex<EventSequence>>;

/// **The props the roll is drawn with**: the notes, their ids and the markers,
/// on the sequence's beats through its tempo map at `rate` samples a second;
/// read-only when `editable` is `false`.
pub fn props(
    sequence: &EventSequence,
    domain: &YDomain,
    rate: f64,
    editable: bool,
) -> Map<String, Value> {
    let axis = Axis::of(sequence, rate);
    let drawn = notes::project(sequence, domain, &axis);
    let mut out = catalogue::pianoroll(&Roll {
        notes: drawn.notes,
        osc: drawn.osc,
        ruler: "beats".into(),
        tempo_map: serde_json::to_string(&axis.map).ok(),
        sample_rate: axis.rate,
        editable,
        ..Roll::default()
    });
    out.insert("note_ids".into(), json!(drawn.note_ids));
    out.remove("type");
    out
}

/// **What the roll is corrected with** after the sequence changed: the notes,
/// their ids, the markers and the tempo map, each stated even when empty -- a
/// roll whose last note was removed is told so. The pitch window is left as
/// the hand has it.
pub fn correction(sequence: &EventSequence, domain: &YDomain, rate: f64) -> Map<String, Value> {
    let axis = Axis::of(sequence, rate);
    let drawn = notes::project(sequence, domain, &axis);
    let mut out = Map::new();
    out.insert("notes".into(), json!(drawn.notes));
    out.insert("note_ids".into(), json!(drawn.note_ids));
    out.insert("osc".into(), json!(drawn.osc));
    if let Ok(map) = serde_json::to_string(&axis.map) {
        out.insert("tempo_map".into(), json!(map));
    }
    out
}

/// **The window**, as a GuiDef rooted at a `window` node: the roll under
/// `widget`, with the title and the size given. A script's own widgets are the
/// client's to append, as in every application here.
pub fn window(roll: Map<String, Value>, widget: i32, title: &str, size: (i64, i64)) -> Value {
    let mut picture = roll;
    picture.insert("type".into(), json!("notes"));
    picture.insert("id".into(), json!(widget));
    json!({
        "type": "window",
        "title": title,
        "w": size.0,
        "h": size.1,
        "flow": "col",
        // **The space bar is the application's**: it plays the sequence
        // through the editor's own playback, so the host's monitor stays out.
        "plays": true,
        "children": [Value::Object(picture)],
    })
}
