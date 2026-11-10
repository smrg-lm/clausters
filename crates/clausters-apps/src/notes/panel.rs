//! **The notes editor's panel**: the inspector of the selected notes and the
//! list of the sequence's events, beside the roll (`crate::panels`).
//!
//! Both show what the sequence holds and follow the roll's selection both
//! ways: the roll tells what the hand marked (`marked`), the inspector shows
//! those notes and the list selects their rows, and a row chosen in the list
//! marks its note on the roll. A value typed in the inspector is written on
//! every selected note **as the roll would write it**: the notes are reported
//! to the roll's own reading (`clausters_editing::notes::intake`) with that
//! value changed, so each mode keeps its rules -- a written value on the
//! reading's grid in notation mode, the page's keys dropped where MIDI mode
//! contradicts them -- and the edit is one entry of the sequence's history.
//!
//! What the inspector shows of the selection is one value a field when the
//! notes agree, and the first note's value with its label saying `varies`
//! when they do not. The panel's widgets are named here ([`names`]) and
//! numbered by the caller with the dialog's, so a window composed without
//! them has no panel.

use serde_json::{Map, Value, json};

use clausters_core::scale::note_name;
use clausters_document::EventSequence;
use clausters_editing::conversation::Correction;
use clausters_editing::events::label_of;
use clausters_editing::notes::{self as roll, Axis, Editing, Mode, YDomain};

/// How wide the panel's column is.
pub const WIDTH: f64 = 360.0;

/// The prefix the inspector's fields are named under.
pub const INSPECT: &str = "inspect";

/// The name of the list of events.
pub const LIST: &str = "events:list";

/// The inspector's fields: a name, a label, the least and the most, a step.
const FIELDS: [(&str, &str, f64, f64, f64); 5] = [
    ("at", "Time (beats)", 0.0, 1.0e6, 0.25),
    ("length", "Length (beats)", 0.0, 1.0e4, 0.25),
    ("pitch", "Pitch", 0.0, 127.0, 1.0),
    ("velocity", "Velocity", 1.0, 127.0, 1.0),
    ("channel", "Channel", 1.0, 16.0, 1.0),
];

/// **Every widget of the panel, by name**, for a caller to number.
#[must_use]
pub fn names() -> Vec<String> {
    FIELDS
        .iter()
        .map(|(name, ..)| format!("{INSPECT}:{name}"))
        .chain(std::iter::once(LIST.to_string()))
        .collect()
}

/// The ids the caller numbered the panel's widgets with.
pub type Ids = crate::forms::Ids;

/// One note the roll draws, as the inspector reads it: its id, its onset and
/// length in beats, its value on the roll's axis, its velocity and its
/// channel counted from 1 -- and its place in the roll's report, in view
/// units, which an inspector edit writes back.
#[derive(Clone, Debug, PartialEq)]
struct Drawn {
    id: u64,
    values: [f64; 5],
    report: [f64; 6],
}

/// The notes the roll draws, in its report's order.
fn drawn(sequence: &EventSequence, domain: &YDomain, axis: &Axis, editing: &Editing) -> Vec<Drawn> {
    let projection = roll::project(sequence, domain, axis, editing);
    projection
        .note_ids
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let q = projection.notes.get(i * 5..i * 5 + 5)?;
            let at = axis.beat(q[0]);
            let length = axis.beat(q[0] + q[1]) - at;
            Some(Drawn {
                id: *id,
                values: [at, length, q[2], q[3], q[4] + 1.0],
                report: [*id as f64, q[0], q[1], q[2], q[3], q[4]],
            })
        })
        .collect()
}

/// What each field shows of the notes `marked`: the value they share, or the
/// first's and `true` where they differ. `None` with nothing marked.
fn shown(notes: &[Drawn], marked: &[u64]) -> Option<[(f64, bool); 5]> {
    let mut of = notes.iter().filter(|n| marked.contains(&n.id));
    let first = of.next()?;
    let mut out = first.values.map(|v| (v, false));
    for note in of {
        for (field, value) in out.iter_mut().zip(note.values) {
            if (field.0 - value).abs() > 1e-6 {
                field.1 = true;
            }
        }
    }
    Some(out)
}

/// The label a field shows: its own, saying so where the notes differ.
fn label(field: usize, varies: bool) -> String {
    let own = FIELDS[field].1;
    if varies {
        format!("{own} (varies)")
    } else {
        own.to_string()
    }
}

/// **Where a beat is, as a reader counts it**: `bar:beat`, the bar and the
/// beat in it counted from 1, through the sequence's bars (`notes::bars`,
/// `beat length number` runs), or bars of four with none.
fn bar_beat(runs: &[f64], beat: f64) -> String {
    let mut bar_at = 0.0;
    let (mut length, mut number) = (4.0, 1.0);
    for [at, len, first] in runs.as_chunks::<3>().0 {
        if *at <= beat + 1e-9 {
            (bar_at, length, number) = (*at, len.max(1e-6), *first);
        }
    }
    let bars = ((beat - bar_at) / length + 1e-9).floor();
    let inside = beat - bar_at - bars * length + 1.0;
    format!("{}:{:.2}", (number + bars) as i64, inside)
}

/// **The list's rows**: every event of the sequence in order, a note with its
/// time, its pitch, its length, its velocity and channel, and in notation
/// mode its written value, staff and voice; a marker with what it sends.
/// Answers the rows and the id of each.
fn rows(sequence: &EventSequence, editing: &Editing) -> (Vec<Value>, Vec<u64>) {
    let runs = roll::bars(sequence);
    let notation = editing.mode == Mode::Notation;
    let mut rows = Vec::new();
    let mut ids = Vec::new();
    for event in &sequence.events {
        let keys = event.keys();
        let at = bar_beat(&runs, event.at.0);
        let row = if let Some(label) = label_of(&event.data.0) {
            let mut row = vec![json!(at), json!("marker"), json!(label)];
            row.extend((0..if notation { 4 } else { 3 }).map(|_| json!("")));
            row
        } else {
            let pitch = clausters_core::event::render::pitch_of(&keys)
                .midinote(&clausters_core::event::render::scale_of(&keys));
            let level = clausters_core::event::render::level_of(&keys);
            let channel = keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0);
            let held = clausters_core::event::render::sustain_of(&keys);
            let mut row = vec![
                json!(at),
                json!("note"),
                json!(format!(
                    "{} ({})",
                    note_name(pitch.round() as i32),
                    round(pitch)
                )),
                json!(round(held)),
                json!(level.velocity().round() as i64),
                json!(channel as i64 + 1),
            ];
            if notation {
                let value = keys.get("value").and_then(Value::as_array).map_or_else(
                    String::new,
                    |v| match (v.first(), v.get(1)) {
                        (Some(n), Some(d)) => format!("{n}/{d}"),
                        _ => String::new(),
                    },
                );
                let [staff, voice] = roll::staff_voice(&keys);
                row.push(json!(format!("{value} {}:{}", staff + 1, voice + 1)));
            }
            row
        };
        rows.push(Value::Array(row));
        ids.push(event.id);
    }
    (rows, ids)
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The list's columns.
fn columns(editing: &Editing) -> Value {
    let mut columns = vec![
        json!({"title": "Time", "w": 60}),
        json!({"title": "Kind", "w": 46}),
        json!("Pitch"),
        json!({"title": "Length", "w": 50}),
        json!({"title": "Vel", "w": 40}),
        json!({"title": "Ch", "w": 30}),
    ];
    if editing.mode == Mode::Notation {
        columns.push(json!({"title": "Written", "w": 64}));
    }
    Value::Array(columns)
}

/// **What the panel shows now**, for the sequence as it stands and the
/// notes `marked`: each widget's props by its id.
fn props(
    sequence: &EventSequence,
    domain: &YDomain,
    axis: &Axis,
    editing: &Editing,
    marked: &[u64],
    ids: &Ids,
) -> Vec<(i32, Map<String, Value>)> {
    let notes = drawn(sequence, domain, axis, editing);
    let fields = shown(&notes, marked);
    let mut out = Vec::new();
    for (i, (name, ..)) in FIELDS.iter().enumerate() {
        let Some(id) = ids.get(&format!("{INSPECT}:{name}")) else {
            continue;
        };
        let (value, varies) = fields.map_or((0.0, false), |f| f[i]);
        let mut props = Map::new();
        props.insert("value".into(), json!(round(value)));
        props.insert("label".into(), json!(label(i, varies)));
        props.insert("enabled".into(), json!(fields.is_some()));
        out.push((*id, props));
    }
    if let Some(id) = ids.get(LIST) {
        let (rows, row_ids) = rows(sequence, editing);
        let selected: Vec<usize> = row_ids
            .iter()
            .enumerate()
            .filter(|(_, id)| marked.contains(id))
            .map(|(i, _)| i)
            .collect();
        let mut props = Map::new();
        props.insert("rows".into(), Value::Array(rows));
        props.insert("columns".into(), columns(editing));
        props.insert("selected".into(), json!(selected));
        out.push((*id, props));
    }
    out
}

/// **The panel**, its widgets showing the sequence and the notes `marked`,
/// or `None` when the caller numbered none of them.
#[must_use]
pub fn panel(
    sequence: &EventSequence,
    domain: &YDomain,
    axis: &Axis,
    editing: &Editing,
    marked: &[u64],
    ids: &Ids,
) -> Option<Value> {
    let shown = props(sequence, domain, axis, editing, marked, ids);
    let widget = |id: i32, props: &Map<String, Value>, kind: Value| {
        let mut widget = props.clone();
        widget.insert("id".into(), json!(id));
        if let Value::Object(more) = kind {
            widget.extend(more);
        }
        Value::Object(widget)
    };
    let mut fields = Vec::new();
    let mut list = Vec::new();
    for (id, props) in &shown {
        let name = ids.iter().find(|(_, i)| *i == id).map(|(n, _)| n.as_str());
        match name {
            Some(LIST) => list.push(widget(
                *id,
                props,
                json!({"type": "table", "multiple": true, "h": 320}),
            )),
            Some(name) => {
                let field = name.strip_prefix(INSPECT).and_then(|n| n.strip_prefix(':'));
                let Some((_, _, min, max, step)) = FIELDS.iter().find(|(n, ..)| Some(*n) == field)
                else {
                    continue;
                };
                fields.push(widget(
                    *id,
                    props,
                    json!({"type": "number", "stepper": true,
                        "min": min, "max": max, "step": step}),
                ));
            }
            None => {}
        }
    }
    let sections: Vec<Value> = [("Selection", fields), ("Events", list)]
        .into_iter()
        .filter(|(_, widgets)| !widgets.is_empty())
        .map(|(title, widgets)| {
            let mut section = crate::panels::section(title, false, widgets);
            section.insert("flow".into(), json!("col"));
            Value::Object(section)
        })
        .collect();
    crate::panels::column(sections, WIDTH)
}

/// **What the panel's widgets are corrected with**, for the sequence as it
/// stands and the notes `marked`.
#[must_use]
pub fn corrections(
    sequence: &EventSequence,
    domain: &YDomain,
    axis: &Axis,
    editing: &Editing,
    marked: &[u64],
    ids: &Ids,
) -> Vec<Correction> {
    props(sequence, domain, axis, editing, marked, ids)
        .into_iter()
        .map(|(id, props)| Correction {
            widget: i64::from(id),
            props: Value::Object(props),
        })
        .collect()
}

/// **The ids of the events the rows `rows` show**, as the list numbers them.
#[must_use]
pub fn ids_of_rows(sequence: &EventSequence, rows: &[usize]) -> Vec<u64> {
    rows.iter()
        .filter_map(|r| sequence.events.get(*r))
        .map(|e| e.id)
        .collect()
}

/// **The roll's report with field `name` set to `value` on every note of
/// `marked`**: the `notes` sextuples the roll's reading takes, every other
/// note as it is drawn. `None` for a name that is no field, or nothing
/// marked that the roll draws.
#[must_use]
pub fn report(
    sequence: &EventSequence,
    domain: &YDomain,
    axis: &Axis,
    editing: &Editing,
    marked: &[u64],
    name: &str,
    value: f64,
) -> Option<Vec<Value>> {
    let field = name.strip_prefix(INSPECT)?.strip_prefix(':')?;
    let notes = drawn(sequence, domain, axis, editing);
    if !notes.iter().any(|n| marked.contains(&n.id)) {
        return None;
    }
    let mut out = Vec::new();
    for note in notes {
        let mut q = note.report;
        if marked.contains(&note.id) {
            let at = note.values[0];
            match field {
                "at" => {
                    let start = axis.units(value.max(0.0));
                    let end = axis.units(value.max(0.0) + note.values[1]);
                    q[1] = start;
                    q[2] = end - start;
                }
                "length" => q[2] = axis.units(at + value.max(1e-6)) - q[1],
                "pitch" => q[3] = value,
                "velocity" => q[4] = value.round().clamp(1.0, 127.0),
                "channel" => q[5] = (value.round() - 1.0).clamp(0.0, 15.0),
                _ => return None,
            }
        }
        out.extend(q.map(|v| json!(v)));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::events::Event;

    fn sequence() -> EventSequence {
        EventSequence::new(vec![
            Event::new(0.0, json!({"midinote": 60, "sustain": 1.0, "velocity": 80})),
            Event::new(1.0, json!({"midinote": 64, "sustain": 1.0, "velocity": 90})),
            Event::new(2.0, json!({"type": "osc", "addr": "/cue", "args": [1]})),
        ])
    }

    fn numbered() -> Ids {
        names()
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, 100 + i as i32))
            .collect()
    }

    #[test]
    fn the_inspector_shows_what_the_selection_shares_and_says_what_varies() {
        let s = sequence();
        let ids: Vec<u64> = s.events.iter().map(|e| e.id).collect();
        let axis = Axis::constant(100.0);
        let shown = corrections(
            &s,
            &YDomain::midi(),
            &axis,
            &Editing::default(),
            &ids[..2],
            &numbered(),
        );
        let field = |i: i64| shown.iter().find(|c| c.widget == i).unwrap().props.clone();
        assert_eq!(field(101)["value"], json!(1.0), "both last a beat");
        assert_eq!(field(101)["label"], json!("Length (beats)"));
        assert_eq!(field(102)["label"], json!("Pitch (varies)"));
        assert_eq!(field(102)["value"], json!(60.0), "the first's");
        let list = field(105);
        assert_eq!(list["rows"].as_array().unwrap().len(), 3);
        assert_eq!(list["rows"][0][0], json!("1:1.00"));
        assert_eq!(list["rows"][2][1], json!("marker"));
        assert_eq!(list["selected"], json!([0, 1]));
    }

    #[test]
    fn a_value_typed_is_reported_on_every_selected_note() {
        let s = sequence();
        let ids: Vec<u64> = s.events.iter().map(|e| e.id).collect();
        let axis = Axis::constant(100.0);
        let values = report(
            &s,
            &YDomain::midi(),
            &axis,
            &Editing::default(),
            &ids[..1],
            "inspect:velocity",
            100.0,
        )
        .unwrap();
        assert_eq!(values.len(), 12);
        assert_eq!(values[4], json!(100.0));
        assert_eq!(values[10], json!(90.0), "the other note as it is");
        assert!(
            report(
                &s,
                &YDomain::midi(),
                &axis,
                &Editing::default(),
                &[],
                "inspect:pitch",
                1.0
            )
            .is_none()
        );
    }

    #[test]
    fn a_beat_is_read_as_bar_and_beat() {
        assert_eq!(bar_beat(&[], 5.5), "2:2.50");
        // a run of 3/4 from the start
        assert_eq!(bar_beat(&[0.0, 3.0, 1.0], 4.0), "2:2.00");
    }
}
