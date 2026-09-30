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
    out.insert("curves".into(), json!(drawn.curves));
    out.insert("layers".into(), json!(drawn.layers));
    out.insert("points".into(), json!(drawn.points));
    out.remove("type");
    let notes: Vec<f64> = out
        .get("notes")
        .and_then(Value::as_array)
        .map(|n| n.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    y_axis(&mut out, &notes, domain);
    // **The play cursor is the transport's position**: anchored at 0, the
    // counter the window's playheads read (the playback's transport) puts the
    // line on the sample the lane is playing, stopped or rolling.
    if let Some(Value::Object(x)) = out.get_mut("axes").and_then(|axes| axes.get_mut("x")) {
        x.insert("playhead_at".into(), json!(0.0));
    }
    out
}

/// **The Y axis a roll navigates, and the window it opens on.**
///
/// The compass is the domain's whole range -- MIDI notes 0 to 127, or the
/// hertz the domain spans, which reach past the highest MIDI note -- so the
/// hand scrolls to any octave; and the window is the notes with some air
/// around them, as a box's roll is fitted (`catalogue::pitch_window`), sent as
/// the slice of the compass the view starts on. A roll in hertz says so, and
/// its compass is in hertz; its window is fitted in pitch, where the air
/// around the notes is semitones, since a log frequency is a linear pitch.
fn y_axis(props: &mut Map<String, Value>, notes: &[f64], domain: &YDomain) {
    use clausters_core::scale::hz_to_midi;

    let hz = domain.ruler == "hz";
    let row = |value: f64| {
        if hz {
            hz_to_midi(value.max(1e-3))
        } else {
            value
        }
    };
    let (floor, ceiling) = (row(domain.min), row(domain.max));
    let span = (ceiling - floor).max(1.0);
    let pitches: Vec<f64> = notes
        .as_chunks::<5>()
        .0
        .iter()
        .flat_map(|n| [n[0], n[1], row(n[2]), n[3], n[4]])
        .collect();
    let (low, high) = catalogue::pitch_window(&pitches);
    let (low, high) = (low.clamp(floor, ceiling), high.clamp(floor, ceiling));
    let mut y = Map::new();
    if hz {
        y.insert("unit".into(), json!("hz"));
    }
    y.insert("min".into(), json!(domain.min));
    y.insert("max".into(), json!(domain.max));
    y.insert("start".into(), json!((low - floor) / span));
    y.insert("len".into(), json!(((high - low) / span).max(1.0 / span)));
    if let Some(axes) = props
        .entry("axes")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        axes.insert("y".into(), Value::Object(y));
    }
}

/// **What the roll is corrected with** after the sequence changed: the notes,
/// their ids, the markers, the curves and their points and the tempo map, each stated even when empty -- a
/// roll whose last note was removed is told so. The pitch window is left as
/// the hand has it.
pub fn correction(sequence: &EventSequence, domain: &YDomain, rate: f64) -> Map<String, Value> {
    let axis = Axis::of(sequence, rate);
    let drawn = notes::project(sequence, domain, &axis);
    let mut out = Map::new();
    out.insert("notes".into(), json!(drawn.notes));
    out.insert("note_ids".into(), json!(drawn.note_ids));
    out.insert("osc".into(), json!(drawn.osc));
    out.insert("curves".into(), json!(drawn.curves));
    out.insert("layers".into(), json!(drawn.layers));
    out.insert("points".into(), json!(drawn.points));
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
