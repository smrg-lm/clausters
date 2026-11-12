//! **A roll's props read off the wire**: its notes, its lines, its joins and
//! what it sends, and the notes a clipboard holds.

use super::*;

/// The notes on the host-wide clipboard, when what is on it is a note block --
/// the same flat quintuple JSON a `/gui_set notes` takes, so a block copied out
/// of one roll pastes into another and a field's text pastes into neither.
pub(super) fn clipboard_notes(text: &str) -> Option<Vec<notes::Note>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let notes = parse_notes(&parse::as_array_props("notes", &value));
    (!notes.is_empty()).then_some(notes)
}

/// Parses a piano-roll clip's `notes`: a flat `[start, dur, pitch, ...]` array
/// (three numbers per note, the flat convention the `bpf` points use), each a
/// [`Note`]. A short/absent/malformed array yields no notes (the
/// clip then draws a waveform body).
pub(super) fn parse_notes(props: &serde_json::Map<String, Value>) -> Vec<Note> {
    let Some(Value::Array(items)) = props.get("notes") else {
        return Vec::new();
    };
    // The canonical wire form is quintuples `start dur pitch velocity channel`
    // (what the Python builder always emits): a length that is a multiple of 5
    // is read as quintuples. Anything else is a plain `start dur pitch` triple
    // list (legacy / hand-authored), which still parses, defaulting velocity to
    // 100 on channel 0. A trailing partial group is dropped.
    let stride = if items.len() % 5 == 0 { 5 } else { 3 };
    items
        .chunks_exact(stride)
        .filter_map(|c| {
            let mut n = Note::new(
                c[0].as_f64()?.max(0.0),
                c[1].as_f64()?.max(0.0),
                c[2].as_f64()? as f32,
            );
            // A velocity or a channel is a number wherever it came from: a
            // sender that holds its list as floats (the document's catalogue
            // does) writes `90.0`, which read as an integer only and fell to
            // the default, so every note drew and reported velocity 100.
            if stride == 5 {
                n.velocity = c[3].as_f64().map_or(100, |v| v.round() as i32);
                n.channel = c[4].as_f64().map_or(0, |v| v.round() as i32);
            }
            Some(n)
        })
        .collect()
}

/// Names each note by the id its owner gave it: `ids` is the `note_ids` list,
/// in the order of the notes. Answers whether the notes now carry ids, which
/// is whether a report names them. A list shorter than the notes leaves the
/// rest unnamed.
pub(super) fn set_ids(notes: &mut [Note], ids: Option<&Value>) -> bool {
    let Some(Value::Array(ids)) = ids else {
        return false;
    };
    for (note, id) in notes.iter_mut().zip(ids) {
        note.id = id.as_f64().map_or(0, |v| v.max(0.0) as u64);
    }
    true
}

/// Parse a `pianoroll`'s `osc` prop -- a flat `[time, label, time, label, ...]`
/// list of OSC markers (the label a short address/tag, an empty string
/// meaning none). A trailing partial pair is dropped.
/// The `lines` prop: a list of runs, each flat `at value` pairs.
pub(super) fn parse_lines(v: &Value) -> Vec<Vec<[f64; 2]>> {
    let v = match v {
        // a `/gui_set` carries a list as a JSON string
        Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
        v => v.clone(),
    };
    v.as_array()
        .map(|runs| {
            runs.iter()
                .map(|run| {
                    let flat: Vec<f64> = run
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_f64).collect())
                        .unwrap_or_default();
                    flat.as_chunks::<2>().0.to_vec()
                })
                .filter(|run| run.len() > 1)
                .collect()
        })
        .unwrap_or_default()
}

/// The `joins` prop: flat `from into` pairs of note indices.
pub(super) fn parse_joins(v: &Value) -> Vec<(usize, usize)> {
    let flat: Vec<usize> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .map(|n| n as usize)
                .collect()
        })
        .unwrap_or_default();
    flat.as_chunks::<2>()
        .0
        .iter()
        .map(|[a, b]| (*a, *b))
        .collect()
}

pub(super) fn parse_osc(props: &serde_json::Map<String, Value>) -> Vec<OscMarker> {
    let Some(Value::Array(items)) = props.get("osc") else {
        return Vec::new();
    };
    items
        .as_chunks::<2>()
        .0
        .iter()
        .filter_map(|c| {
            let time = c[0].as_f64()?.max(0.0);
            let label = c[1].as_str().filter(|s| !s.is_empty()).map(str::to_string);
            Some(OscMarker { time, label })
        })
        .collect()
}
