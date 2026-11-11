//! **"This note goes on into that one", in its two shapes**, kept in
//! agreement.
//!
//! A note that goes on into the next without a new attack says so with
//! `joins`: the id of the note it goes on from, which the playback reads and
//! which is MIDI data -- a line played legato. A page says it with a slur
//! whose `join` is true. A tie is neither: a note written with ties is one
//! event, and the page writes the ties where it cuts a value.
//!
//! **The shape an edit changed wins**, and the other is derived again for what
//! the edit touched, in the same edit:
//!
//! - a join edited ([`join`], [`unjoin`]) writes the slurs of the lines that
//!   are written as notation ([`slurs_from_joins`]) -- a run of joins is one
//!   slur with `join` true, whatever its pitches;
//! - a slur edited on the page writes the joins over its span
//!   ([`joins_from_slur`]): joined, or taken away where it is attacked.
//!
//! **MIDI has no join**: a note-on there is always an attack. So a sequence
//! written in a MIDI format holds none -- a join into the same pitch is one
//! note held on, and any other is an attack ([`said_in_midi`]) -- and only a
//! page and an event sequence for the server hold them.
//!
//! A slur written before `join` existed reads as joined where its notes carry
//! joins, and as attacked otherwise. A line is the notes of one `(staff,
//! voice)` where they say either, else of one channel; a line is **written as
//! notation** where a note of it carries a written `value`, and a line that
//! is not (a pattern's voice, a take) gains no slur.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use clausters_core::event::render::{self, Type};

use super::EventSequence;
use crate::multitrack::nodes::join_points;

/// The line a note is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Line {
    /// The notes of one staff and voice, as they say.
    Written {
        /// The staff.
        staff: i64,
        /// The voice.
        voice: i64,
    },
    /// The notes of one channel, where they say neither.
    Channel(i64),
}

fn number(keys: &Map<String, Value>, key: &str) -> Option<i64> {
    keys.get(key).and_then(Value::as_f64).map(|n| n as i64)
}

/// The line a note with these keys is on.
#[must_use]
pub fn line_of(keys: &Map<String, Value>) -> Line {
    let (staff, voice) = (number(keys, "staff"), number(keys, "voice"));
    if staff.is_some() || voice.is_some() {
        Line::Written {
            staff: staff.unwrap_or(0),
            voice: voice.unwrap_or(0),
        }
    } else {
        Line::Channel(number(keys, "channel").unwrap_or(0))
    }
}

/// **The sequence's lines**: each line's notes, by onset and then by id.
#[must_use]
pub fn lines(sequence: &EventSequence) -> BTreeMap<Line, Vec<u64>> {
    let mut out: BTreeMap<Line, Vec<(f64, u64)>> = BTreeMap::new();
    for event in &sequence.events {
        let keys = event.keys();
        if Type::of(&keys) == Type::Note {
            out.entry(line_of(&keys))
                .or_default()
                .push((event.at.0, event.id));
        }
    }
    out.into_iter()
        .map(|(line, mut notes)| {
            notes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            (line, notes.into_iter().map(|(_, id)| id).collect())
        })
        .collect()
}

/// The id a note's `joins` names, if it names one.
fn joined_from(sequence: &EventSequence, id: u64) -> Option<u64> {
    sequence.get(id)?.keys().get("joins")?.as_u64()
}

/// Writes or takes away `joins` on one note.
fn set_joins(sequence: &mut EventSequence, id: u64, from: Option<u64>) {
    let Some(event) = sequence.events.iter_mut().find(|e| e.id == id) else {
        return;
    };
    let mut keys = event.keys();
    match from {
        Some(from) => keys.insert("joins".into(), json!(from)),
        None => keys.remove("joins"),
    };
    event.data = crate::Opaque(Value::Object(keys));
}

/// The notes of `ids`, gathered by onset, each onset low to high.
fn by_onset(sequence: &EventSequence, ids: &[u64]) -> Vec<Vec<u64>> {
    let mut out: Vec<(f64, Vec<(f64, u64)>)> = Vec::new();
    for id in ids {
        let Some(event) = sequence.get(*id) else {
            continue;
        };
        let keys = event.keys();
        let pitch = render::pitch_of(&keys)
            .midinote(&render::scale_of(&keys))
            .round();
        match out.last_mut() {
            Some((at, notes)) if *at == event.at.0 => notes.push((pitch, *id)),
            _ => out.push((event.at.0, vec![(pitch, *id)])),
        }
    }
    out.into_iter()
        .map(|(_, mut notes)| {
            notes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            notes.into_iter().map(|(_, id)| id).collect()
        })
        .collect()
}

/// Each note of one onset with the note of the next it goes on into: by rank
/// when the two hold as many, else the first of each.
fn pairs(a: &[u64], b: &[u64]) -> Vec<(u64, u64)> {
    if a.len() == b.len() {
        a.iter().copied().zip(b.iter().copied()).collect()
    } else {
        a.first()
            .copied()
            .zip(b.first().copied())
            .into_iter()
            .collect()
    }
}

/// The section's spanners, as written.
fn spanners(sequence: &EventSequence) -> Vec<Value> {
    sequence
        .notation
        .as_ref()
        .and_then(|n| n.get("spanners"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Writes the section's spanners: the list goes where there is none, and so
/// does a section left empty.
fn set_spanners(sequence: &mut EventSequence, list: Vec<Value>) {
    let mut section = match sequence.notation.take() {
        Some(Value::Object(map)) => map,
        Some(other) => {
            sequence.notation = Some(other);
            return;
        }
        None => Map::new(),
    };
    if list.is_empty() {
        section.remove("spanners");
    } else {
        section.insert("spanners".into(), Value::Array(list));
    }
    sequence.notation = (!section.is_empty()).then_some(Value::Object(section));
}

/// The notes of a line a slur from `from` to `to` reaches, in order; none
/// where the two are not on one line.
fn span(lines: &BTreeMap<Line, Vec<u64>>, from: u64, to: u64) -> Vec<u64> {
    for notes in lines.values() {
        let (Some(a), Some(b)) = (
            notes.iter().position(|id| *id == from),
            notes.iter().position(|id| *id == to),
        ) else {
            continue;
        };
        let (a, b) = (a.min(b), a.max(b));
        return notes[a..=b].to_vec();
    }
    Vec::new()
}

/// Whether a slur written as `slur` joins its notes, in a sequence whose
/// lines are `lines`: what it says, and for one that says nothing, whether
/// the notes after its first all carry joins.
fn joins_its_notes(
    sequence: &EventSequence,
    lines: &BTreeMap<Line, Vec<u64>>,
    slur: &Value,
) -> bool {
    if let Some(join) = slur.get("join").and_then(Value::as_bool) {
        return join;
    }
    let ends = (
        slur.get("from").and_then(Value::as_u64),
        slur.get("to").and_then(Value::as_u64),
    );
    let (Some(from), Some(to)) = ends else {
        return false;
    };
    let onsets = by_onset(sequence, &span(lines, from, to));
    onsets.len() > 1
        && onsets[1..]
            .iter()
            .flatten()
            .all(|id| joined_from(sequence, *id).is_some())
}

/// **The section's slurs**, each from its first note to its last with
/// whether it joins them (what it says, and for one that says nothing,
/// whether its notes carry joins): what a roll draws of them.
#[must_use]
pub fn slurs(sequence: &EventSequence) -> Vec<(u64, u64, bool)> {
    let lines = lines(sequence);
    spanners(sequence)
        .iter()
        .filter(|s| s.get("kind").and_then(Value::as_str) == Some("slur"))
        .filter_map(|s| {
            let from = s.get("from")?.as_u64()?;
            let to = s.get("to")?.as_u64()?;
            Some((from, to, joins_its_notes(sequence, &lines, s)))
        })
        .collect()
}

/// Whether a line is written as notation: a note of it carries a `value`.
fn written(sequence: &EventSequence, notes: &[u64]) -> bool {
    notes
        .iter()
        .filter_map(|id| sequence.get(*id))
        .any(|e| e.keys().contains_key("value"))
}

/// **The slurs of the lines `touched`, written again from their joins**:
/// in each line written as notation, the slurs that join (or that read as
/// joining) go, and each run of joins is one slur with `join` true, from its
/// first note to its last. A slur that is attacked stays.
pub fn slurs_from_joins(sequence: &mut EventSequence, touched: &BTreeSet<Line>) {
    let lines = lines(sequence);
    let mut list = spanners(sequence);
    let before = list.clone();
    for line in touched {
        let Some(notes) = lines.get(line) else {
            continue;
        };
        if !written(sequence, notes) {
            continue;
        }
        let held: BTreeSet<u64> = notes.iter().copied().collect();
        list.retain(|s| {
            let ours = s.get("kind").and_then(Value::as_str) == Some("slur")
                && s.get("from")
                    .and_then(Value::as_u64)
                    .is_some_and(|f| held.contains(&f));
            !(ours && joins_its_notes(sequence, &lines, s))
        });
        // runs: a note no one in the line joins into begins one
        let next: BTreeMap<u64, u64> = notes
            .iter()
            .filter_map(|b| Some((joined_from(sequence, *b).filter(|a| held.contains(a))?, *b)))
            .collect();
        let into: BTreeSet<u64> = next.values().copied().collect();
        for first in notes
            .iter()
            .filter(|id| next.contains_key(id) && !into.contains(id))
        {
            let mut last = *first;
            while let Some(b) = next.get(&last) {
                last = *b;
            }
            list.push(json!({"kind": "slur", "from": first, "to": last, "join": true}));
        }
    }
    if list != before {
        set_spanners(sequence, list);
    }
}

/// **The joins over a slur's span, written as the slur says**: each note
/// after its first goes on from the one before when `join`, and is attacked
/// when not. Notes of one onset pair with the next onset's by rank.
pub fn joins_from_slur(sequence: &mut EventSequence, from: u64, to: u64, join: bool) {
    let lines = lines(sequence);
    let onsets = by_onset(sequence, &span(&lines, from, to));
    for pair in onsets.windows(2) {
        let paired = pairs(&pair[0], &pair[1]);
        for b in &pair[1] {
            let a = paired.iter().find(|(_, x)| x == b).map(|(a, _)| *a);
            set_joins(sequence, *b, if join { a } else { None });
        }
    }
}

/// **The joins written again for the slurs an edit changed**: `before` and
/// `after` are the section's spanners on either side of it, each naming the
/// sequence's events. A slur added or changed writes its span as it says (one
/// that says nothing is left as it is); a slur taken away that joined
/// takes its joins with it.
pub fn joins_from_page(sequence: &mut EventSequence, before: &[Value], after: &[Value]) {
    let slurs = |list: &[Value]| -> BTreeMap<(u64, u64), Value> {
        list.iter()
            .filter(|s| s.get("kind").and_then(Value::as_str) == Some("slur"))
            .filter_map(|s| {
                let ends = (s.get("from")?.as_u64()?, s.get("to")?.as_u64()?);
                Some((ends, s.clone()))
            })
            .collect()
    };
    let (was, is) = (slurs(before), slurs(after));
    let lines = lines(sequence);
    for ((from, to), slur) in &was {
        if !is.contains_key(&(*from, *to)) && joins_its_notes(sequence, &lines, slur) {
            joins_from_slur(sequence, *from, *to, false);
        }
    }
    for ((from, to), slur) in &is {
        if was.get(&(*from, *to)) == Some(slur) {
            continue;
        }
        if let Some(join) = slur.get("join").and_then(Value::as_bool) {
            joins_from_slur(sequence, *from, *to, join);
        }
    }
}

/// The notes of `ids` by line, each line's in order; refused where one is not
/// a note of the sequence.
fn selected(sequence: &EventSequence, ids: &[u64]) -> Result<BTreeMap<Line, Vec<u64>>, String> {
    let chosen: BTreeSet<u64> = ids.iter().copied().collect();
    for id in &chosen {
        let event = sequence
            .get(*id)
            .ok_or_else(|| format!("the sequence holds no event {id}"))?;
        if Type::of(&event.keys()) != Type::Note {
            return Err(format!(
                "event {id} is not a note, and only notes are joined"
            ));
        }
    }
    Ok(lines(sequence)
        .into_iter()
        .map(|(line, notes)| {
            let mine: Vec<u64> = notes.into_iter().filter(|id| chosen.contains(id)).collect();
            (line, mine)
        })
        .filter(|(_, notes)| !notes.is_empty())
        .collect())
}

/// **Join the notes `ids`**: in each line, each onset of them goes on into
/// the next without a new attack -- a chord into a chord of as many notes
/// note by note, low to high -- and the slurs of the lines written as
/// notation follow. A note goes on into one next only, so a join that named
/// one of them before from elsewhere goes.
///
/// **Two notes of one pitch that touch are one note** ([`hold`]), in every
/// format: on a page that is a tie. **In a MIDI format a join holds one
/// pitch on**, touching or not: MIDI has no join, so notes of one pitch are
/// made one note, and notes of different pitches are refused.
///
/// # Errors
/// When an id is not a note, or no line holds two onsets of the selection;
/// in a MIDI format, when two notes it would join sound different pitches.
pub fn join(sequence: &mut EventSequence, ids: &[u64]) -> Result<(), String> {
    let midi = matches!(sequence.format(), super::Format::Midi(_));
    let chosen = selected(sequence, ids)?;
    let mut touched = BTreeSet::new();
    let mut held = Vec::new();
    for (line, notes) in &chosen {
        let onsets = by_onset(sequence, notes);
        if onsets.len() < 2 {
            continue;
        }
        for pair in onsets.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.len() != b.len() {
                return Err(format!(
                    "a chord of {} notes is joined to one of as many, not of {}",
                    a.len(),
                    b.len()
                ));
            }
            for (from, into) in pairs(a, b) {
                let one = holds_one_pitch(sequence, from, into);
                if one && (midi || touches(sequence, from, into)) {
                    held.push((from, into));
                    continue;
                }
                if midi {
                    return Err("in MIDI a join holds one pitch on: \
                         notes of different pitches are not joined"
                        .into());
                }
                // what went on from `from` before goes on from it no more
                let others: Vec<u64> = sequence
                    .events
                    .iter()
                    .filter(|e| {
                        e.id != into && e.keys().get("joins").and_then(Value::as_u64) == Some(from)
                    })
                    .map(|e| e.id)
                    .collect();
                for other in others {
                    set_joins(sequence, other, None);
                }
                set_joins(sequence, into, Some(from));
            }
        }
        touched.insert(*line);
    }
    if touched.is_empty() {
        return Err("select two notes or more of one line, one after the other, to join".into());
    }
    hold(sequence, &held);
    slurs_from_joins(sequence, &touched);
    Ok(())
}

/// **Take the joins among the notes `ids` away**: every join from one of
/// them into another -- or, for one note alone, the join into it and the one
/// out of it -- and the slurs of the lines written as notation follow.
///
/// # Errors
/// When an id is not a note of the sequence.
pub fn unjoin(sequence: &mut EventSequence, ids: &[u64]) -> Result<(), String> {
    let chosen = selected(sequence, ids)?;
    let all: BTreeSet<u64> = chosen.values().flatten().copied().collect();
    let alone = all.len() == 1;
    let mut touched = BTreeSet::new();
    let notes: Vec<(u64, Option<u64>, Line)> = sequence
        .events
        .iter()
        .filter(|e| Type::of(&e.keys()) == Type::Note)
        .map(|e| {
            let keys = e.keys();
            (
                e.id,
                keys.get("joins").and_then(Value::as_u64),
                line_of(&keys),
            )
        })
        .collect();
    for (id, from, line) in notes {
        let Some(from) = from else {
            continue;
        };
        let cut = if alone {
            all.contains(&id) || all.contains(&from)
        } else {
            all.contains(&id) && all.contains(&from)
        };
        if cut {
            set_joins(sequence, id, None);
            touched.insert(line);
        }
    }
    slurs_from_joins(sequence, &touched);
    Ok(())
}

/// Within this of each other, two notes sound one pitch.
const SAME_PITCH: f64 = 1e-6;

/// Whether `into` starts after `from` and sounds its pitch: what a join holds
/// on as one note.
fn holds_one_pitch(sequence: &EventSequence, from: u64, into: u64) -> bool {
    let (Some(a), Some(b)) = (sequence.get(from), sequence.get(into)) else {
        return false;
    };
    let pitch = |keys: &Map<String, Value>| {
        (Type::of(keys) == Type::Note)
            .then(|| render::pitch_of(keys).midinote(&render::scale_of(keys)))
    };
    let (Some(x), Some(y)) = (pitch(&a.keys()), pitch(&b.keys())) else {
        return false;
    };
    b.at.0 > a.at.0 && (x - y).abs() < SAME_PITCH
}

/// How far a note reaches: its written length where it is written, else
/// what it sounds.
fn reach(keys: &Map<String, Value>) -> f64 {
    let sounds = render::sustain_of(keys).max(0.0);
    if keys.contains_key("value") {
        keys.get("dur").and_then(Value::as_f64).unwrap_or(sounds)
    } else {
        sounds
    }
}

/// Whether `into` starts where `from` ends, or before: two notes that touch.
fn touches(sequence: &EventSequence, from: u64, into: u64) -> bool {
    let (Some(a), Some(b)) = (sequence.get(from), sequence.get(into)) else {
        return false;
    };
    b.at.0 <= a.at.0 + reach(&a.keys()) + SAME_PITCH
}

/// **`into` made part of `from`**, the note before it at its own pitch (the
/// inverse of a note's split, `super::split`): `from` sounds on to where
/// `into` ended, and its `dur` reaches as far as `into`'s did -- written
/// again as a value where it is written, taken away where no value is that
/// long; each curve of the two is one curve, the first's last value held
/// until the second's first point ([`crate::multitrack::nodes::join_points`]);
/// and what went on from `into`, and a spanner on it, is on `from`. `into`
/// goes.
fn hold_on(sequence: &mut EventSequence, from: u64, into: u64, beat_unit: i64) {
    let Some(b) = sequence.index(into) else {
        return;
    };
    let gone = sequence.events.remove(b);
    let Some(a) = sequence.index(from) else {
        sequence.events.insert(b, gone);
        return;
    };
    let offset = gone.at.0 - sequence.events[a].at.0;
    let theirs = gone.keys();
    let sounds = render::sustain_of(&theirs).max(0.0);
    let mut keys = sequence.events[a].keys();
    let ends = render::sustain_of(&keys).max(0.0);
    if keys.contains_key("dur") {
        let dur = theirs.get("dur").and_then(Value::as_f64).unwrap_or(sounds);
        keys.insert("dur".into(), json!(offset + dur));
        if keys.contains_key("value") {
            match super::reading::written_value(offset + dur, beat_unit) {
                Some(value) => keys.insert("value".into(), value),
                None => keys.remove("value"),
            };
        }
    }
    render::set_key(&mut keys, "sustain", json!(offset + sounds));
    sequence.events[a].data = crate::Opaque(Value::Object(keys));
    let mut second = gone.automation;
    let mine = std::mem::take(&mut sequence.events[a].automation);
    let mut curves = Vec::with_capacity(mine.len() + second.len());
    for curve in mine {
        let other = second
            .iter()
            .position(|c| c.target == curve.target)
            .map(|i| second.remove(i));
        let points = other.as_ref().map_or(&[][..], |c| &c.points[..]);
        curves.push(crate::multitrack::Automation {
            points: join_points(&curve.points, ends, points, offset),
            ..curve
        });
    }
    for curve in second {
        curves.push(crate::multitrack::Automation {
            points: join_points(&[], 0.0, &curve.points, offset),
            ..curve
        });
    }
    sequence.events[a].automation = curves;
    let after: Vec<u64> = sequence
        .events
        .iter()
        .filter(|e| e.keys().get("joins").and_then(Value::as_u64) == Some(into))
        .map(|e| e.id)
        .collect();
    for id in after {
        set_joins(sequence, id, Some(from));
    }
    let mut list = spanners(sequence);
    if !list.is_empty() {
        for spanner in &mut list {
            for end in ["from", "to"] {
                if spanner.get(end).and_then(Value::as_u64) == Some(into) {
                    spanner[end] = json!(from);
                }
            }
        }
        list.retain(|s| s.get("from") != s.get("to"));
        set_spanners(sequence, list);
    }
}

/// **Each pair `(from, into)` made one note** (`hold_on`), in order: a
/// note made part of another answers for it from then on, so a run of them
/// is one note.
pub fn hold(sequence: &mut EventSequence, pairs: &[(u64, u64)]) {
    if pairs.is_empty() {
        return;
    }
    let beat_unit = super::reading::Transcription::of(sequence).beat_unit;
    let mut became: BTreeMap<u64, u64> = BTreeMap::new();
    for (from, into) in pairs {
        let from = became.get(from).copied().unwrap_or(*from);
        hold_on(sequence, from, *into, beat_unit);
        became.insert(*into, from);
    }
}

/// **Every join into a note of the same pitch made one note** ([`hold`]),
/// in onset order, so a run of them is one note. A note held on into its own
/// pitch is one note in every format -- on a page one value, tied where the
/// metre cuts it -- so a conversion makes it one before anything else. A
/// join between different notes is left as it is.
pub fn held_on(sequence: &mut EventSequence) {
    let mut joined: Vec<(f64, u64, u64)> = sequence
        .events
        .iter()
        .filter_map(|e| Some((e.at.0, e.keys().get("joins")?.as_u64()?, e.id)))
        .collect();
    joined.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
    let mut became: BTreeMap<u64, u64> = BTreeMap::new();
    let mut pairs = Vec::new();
    for (_, from, into) in joined {
        let from = became.get(&from).copied().unwrap_or(from);
        if holds_one_pitch(sequence, from, into) {
            pairs.push((from, into));
            became.insert(into, from);
        }
    }
    hold(sequence, &pairs);
}

/// **A sequence as MIDI says it**, which has no join (a note-on there is
/// always an attack): a join into the same pitch is one note held on
/// ([`held_on`]), and every other join goes, the note it went into attacked.
/// What a sequence written in a MIDI format holds, whether it was converted
/// into one or edited in one.
pub fn said_in_midi(sequence: &mut EventSequence) {
    held_on(sequence);
    let joined: Vec<u64> = sequence
        .events
        .iter()
        .filter(|e| e.keys().contains_key("joins"))
        .map(|e| e.id)
        .collect();
    for id in joined {
        set_joins(sequence, id, None);
    }
}

/// **The joins of `performance` carried onto `page`**, the page it was
/// written as, and each written as a slur that joins: one between two notes
/// the page holds under the performance's ids, on one of its lines and in
/// order. A join into the same pitch is no longer there to carry: it was
/// made one note before the page was read ([`held_on`]).
pub fn carry_joins(page: &mut EventSequence, performance: &EventSequence) {
    let at: BTreeMap<u64, (f64, Line)> = page
        .events
        .iter()
        .map(|e| (e.id, (e.at.0, line_of(&e.keys()))))
        .collect();
    let mut touched = BTreeSet::new();
    for event in &performance.events {
        let Some(from) = event.keys().get("joins").and_then(Value::as_u64) else {
            continue;
        };
        let (Some((begins, line)), Some((starts, into_line))) = (at.get(&from), at.get(&event.id))
        else {
            continue;
        };
        if line == into_line && starts > begins {
            set_joins(page, event.id, Some(from));
            touched.insert(*line);
        }
    }
    slurs_from_joins(page, &touched);
}

/// **A join that no longer says "goes on into the next" goes**: one naming a
/// note the sequence no longer holds -- what an edit that removes notes
/// leaves, whoever made it -- and one into a note that does not start after
/// the note it goes on from, which is what a note moved before it leaves. A
/// join is made in onset order ([`join`]), so a pair the hand has reordered
/// joins nothing, and its line's slurs follow. A sequence written in a MIDI
/// format holds none at all ([`said_in_midi`]).
pub fn prune(sequence: &mut EventSequence) {
    if matches!(sequence.format(), super::Format::Midi(_)) {
        said_in_midi(sequence);
        return;
    }
    let at: BTreeMap<u64, f64> = sequence.events.iter().map(|e| (e.id, e.at.0)).collect();
    let mut stale = Vec::new();
    let mut touched = BTreeSet::new();
    for event in &sequence.events {
        let keys = event.keys();
        let Some(from) = keys.get("joins").and_then(Value::as_u64) else {
            continue;
        };
        match at.get(&from) {
            None => stale.push(event.id),
            Some(&begins) if event.at.0 <= begins => {
                stale.push(event.id);
                touched.insert(line_of(&keys));
            }
            Some(_) => {}
        }
    }
    for id in stale {
        set_joins(sequence, id, None);
    }
    if !touched.is_empty() {
        slurs_from_joins(sequence, &touched);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Event, EventsIntent};
    use crate::timebase::Beat;

    /// A line played: four notes on channel 0, the middle two of one pitch,
    /// none touching the next.
    fn played() -> EventSequence {
        let note = |id: u64, at: f64, midinote: i64| Event {
            id,
            ..Event::new(at, json!({"midinote": midinote, "sustain": 0.5}))
        };
        EventSequence::new(vec![
            note(1, 0.0, 60),
            note(2, 1.0, 64),
            note(3, 2.0, 64),
            note(4, 3.0, 67),
        ])
    }

    /// The same line written as notation: a staff, a voice, written values.
    fn written_line() -> EventSequence {
        let mut sequence = played();
        for event in &mut sequence.events {
            let mut keys = event.keys();
            keys.insert("staff".into(), json!(0));
            keys.insert("voice".into(), json!(0));
            keys.insert("value".into(), json!([1, 4]));
            event.data = crate::Opaque(Value::Object(keys));
        }
        sequence
    }

    fn joins(sequence: &EventSequence) -> Vec<Option<u64>> {
        sequence
            .events
            .iter()
            .map(|e| e.keys().get("joins").and_then(Value::as_u64))
            .collect()
    }

    #[test]
    fn a_join_in_a_played_line_is_its_joins_and_nothing_of_a_page() {
        let mut sequence = played();
        sequence
            .edit(EventsIntent::Join { ids: vec![1, 2, 3] })
            .unwrap();
        // two notes of one pitch joined are two notes, joined
        assert_eq!(joins(&sequence), vec![None, Some(1), Some(2), None]);
        assert_eq!(sequence.notation, None, "no slur, no tie");
        assert!(
            sequence
                .events
                .iter()
                .all(|e| !e.keys().contains_key("tie"))
        );
        sequence
            .edit(EventsIntent::Unjoin { ids: vec![2] })
            .unwrap();
        assert_eq!(joins(&sequence), vec![None, None, None, None]);
    }

    #[test]
    fn a_join_in_a_written_line_is_a_slur_that_joins() {
        let mut sequence = written_line();
        sequence
            .edit(EventsIntent::Join {
                ids: vec![1, 2, 3, 4],
            })
            .unwrap();
        assert_eq!(joins(&sequence), vec![None, Some(1), Some(2), Some(3)]);
        assert_eq!(
            spanners(&sequence),
            vec![json!({"kind": "slur", "from": 1, "to": 4, "join": true})]
        );
        // a join taken out of the middle cuts the slur in two runs
        sequence
            .edit(EventsIntent::Unjoin { ids: vec![2, 3] })
            .unwrap();
        assert_eq!(joins(&sequence), vec![None, Some(1), None, Some(3)]);
        assert_eq!(
            spanners(&sequence),
            vec![
                json!({"kind": "slur", "from": 1, "to": 2, "join": true}),
                json!({"kind": "slur", "from": 3, "to": 4, "join": true}),
            ]
        );
        // and all of them taken away take the slurs, and the section
        sequence
            .edit(EventsIntent::Unjoin {
                ids: vec![1, 2, 3, 4],
            })
            .unwrap();
        assert_eq!(sequence.notation, None);
    }

    #[test]
    fn an_attacked_slur_stays_and_an_old_one_over_joins_reads_as_joining() {
        let mut sequence = written_line();
        sequence.notation = Some(json!({"spanners": [
            {"kind": "slur", "from": 1, "to": 2, "join": false},
            {"kind": "slur", "from": 3, "to": 4},
        ]}));
        set_joins(&mut sequence, 4, Some(3));
        // the old slur over 3-4 reads as joined, since 4 carries a join
        sequence
            .edit(EventsIntent::Join { ids: vec![2, 3] })
            .unwrap();
        assert_eq!(
            spanners(&sequence),
            vec![
                json!({"kind": "slur", "from": 1, "to": 2, "join": false}),
                json!({"kind": "slur", "from": 2, "to": 4, "join": true}),
            ]
        );
    }

    /// **A slur is read with whether it joins**: what it says, and for one
    /// that says nothing, whether the notes after its first carry joins.
    #[test]
    fn the_slurs_are_read_with_whether_they_join() {
        let mut sequence = written_line();
        sequence.notation = Some(json!({"spanners": [
            {"kind": "slur", "from": 1, "to": 2, "join": false},
            {"kind": "slur", "from": 3, "to": 4},
            {"kind": "hairpin", "from": 1, "to": 4},
        ]}));
        assert_eq!(slurs(&sequence), vec![(1, 2, false), (3, 4, false)]);
        set_joins(&mut sequence, 4, Some(3));
        assert_eq!(slurs(&sequence), vec![(1, 2, false), (3, 4, true)]);
    }

    #[test]
    fn a_join_is_refused_where_it_has_nothing_to_join() {
        let mut sequence = played();
        assert!(
            sequence
                .edit(EventsIntent::Join { ids: vec![2] })
                .unwrap_err()
                .contains("two notes")
        );
        assert!(
            sequence
                .edit(EventsIntent::Join { ids: vec![2, 99] })
                .unwrap_err()
                .contains("99")
        );
        // a chord of two into one note
        sequence
            .edit(EventsIntent::Add {
                event: Event {
                    id: 9,
                    ..Event::new(1.0, json!({"midinote": 67}))
                },
            })
            .unwrap();
        assert!(
            sequence
                .edit(EventsIntent::Join { ids: vec![2, 9, 3] })
                .unwrap_err()
                .contains("chord")
        );
    }

    #[test]
    fn a_chord_joins_a_chord_low_to_high() {
        let note = |id: u64, at: f64, midinote: i64| Event {
            id,
            ..Event::new(at, json!({"midinote": midinote}))
        };
        let mut sequence = EventSequence::new(vec![
            note(1, 0.0, 64),
            note(2, 0.0, 60),
            note(3, 1.0, 62),
            note(4, 1.0, 65),
        ]);
        sequence
            .edit(EventsIntent::Join {
                ids: vec![1, 2, 3, 4],
            })
            .unwrap();
        let of = |id: u64| joined_from(&sequence, id);
        assert_eq!((of(3), of(4)), (Some(2), Some(1)));
    }

    #[test]
    fn a_note_taken_away_leaves_no_join_naming_it() {
        let mut sequence = played();
        sequence
            .edit(EventsIntent::Join {
                ids: vec![1, 2, 3, 4],
            })
            .unwrap();
        sequence.edit(EventsIntent::Remove { id: 2 }).unwrap();
        assert_eq!(joins(&sequence), vec![None, None, Some(3)]);
        // and a whole list without a note, as a roll sends it
        let kept: Vec<Event> = sequence
            .events
            .iter()
            .filter(|e| e.id != 3)
            .cloned()
            .collect();
        sequence
            .edit(EventsIntent::SetEvents { events: kept })
            .unwrap();
        assert_eq!(joins(&sequence), vec![None, None]);
    }

    /// **A note moved before the one it went on from joins it no more**: a
    /// join is made in onset order, so the pair a hand reordered is two
    /// notes, and the slur of a written line follows. A move that keeps the
    /// order keeps the join, a legato that overlaps included.
    #[test]
    fn a_note_moved_before_the_one_it_went_on_from_is_joined_no_more() {
        let joined = |s: &EventSequence, id: u64| {
            s.get(id)
                .and_then(|e| e.keys().get("joins").and_then(Value::as_u64))
        };
        let mut sequence = written_line();
        sequence
            .edit(EventsIntent::Join {
                ids: vec![1, 2, 3, 4],
            })
            .unwrap();
        // 3 moved into 2's span still starts after it: still joined
        sequence
            .edit(EventsIntent::Move {
                id: 3,
                at: Beat(1.5),
            })
            .unwrap();
        assert_eq!(joined(&sequence, 3), Some(2));
        // 3 moved before 2: neither the join into it nor its slur stays
        sequence
            .edit(EventsIntent::Move {
                id: 3,
                at: Beat(0.5),
            })
            .unwrap();
        assert_eq!(joined(&sequence, 3), None);
        assert_eq!(joined(&sequence, 2), Some(1));
        assert_eq!(joined(&sequence, 4), Some(3), "4 still follows 3");
        assert_eq!(
            spanners(&sequence),
            vec![
                json!({"kind": "slur", "from": 1, "to": 2, "join": true}),
                json!({"kind": "slur", "from": 3, "to": 4, "join": true}),
            ]
        );
        // and the whole list, as a roll sends it after a drag
        let mut swapped = sequence.events.clone();
        for event in &mut swapped {
            if event.id == 4 {
                event.at = Beat(0.25);
            }
        }
        sequence
            .edit(EventsIntent::SetEvents { events: swapped })
            .unwrap();
        assert_eq!(joined(&sequence, 4), None);
    }

    #[test]
    fn a_slur_on_the_page_writes_or_takes_its_joins() {
        let mut sequence = written_line();
        let slur = json!({"kind": "slur", "from": 1, "to": 3, "join": true});
        joins_from_page(&mut sequence, &[], std::slice::from_ref(&slur));
        assert_eq!(joins(&sequence), vec![None, Some(1), Some(2), None]);
        // the same slur, now attacked
        let attacked = json!({"kind": "slur", "from": 1, "to": 3, "join": false});
        joins_from_page(&mut sequence, std::slice::from_ref(&slur), &[attacked]);
        assert_eq!(joins(&sequence), vec![None, None, None, None]);
        // and one that joined, taken away, takes its joins
        joins_from_page(&mut sequence, &[], std::slice::from_ref(&slur));
        joins_from_page(&mut sequence, &[slur], &[]);
        assert_eq!(joins(&sequence), vec![None, None, None, None]);
    }

    /// The line played, written for MIDI 1.0.
    fn midi_line() -> EventSequence {
        let mut sequence = played();
        sequence.midi = Some(crate::events::MidiSpec::Midi1);
        sequence
    }

    /// **In MIDI a join holds one pitch on**: two notes of one pitch become
    /// one note that sounds to where the second ended, and what went on from
    /// the second goes on from it; notes of different pitches are refused.
    #[test]
    fn a_join_in_midi_holds_one_pitch_on_and_refuses_two() {
        let mut sequence = midi_line();
        let refused = sequence.edit(EventsIntent::Join { ids: vec![1, 2] });
        assert!(refused.unwrap_err().contains("one pitch"));
        assert_eq!(sequence, midi_line(), "a refusal changes nothing");
        sequence
            .edit(EventsIntent::Join { ids: vec![2, 3] })
            .unwrap();
        let ids: Vec<u64> = sequence.events.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![1, 2, 4], "the second note is part of the first");
        let held = sequence.get(2).unwrap().keys();
        assert_eq!(render::sustain_of(&held), 1.5);
        assert_eq!(joins(&sequence), vec![None, None, None]);
    }

    /// **A MIDI sequence holds no join**, whoever states one: an edit that
    /// brings one says it as MIDI does, and so does a spec given to a
    /// sequence that held some.
    #[test]
    fn a_midi_sequence_holds_no_join() {
        let mut joined = played();
        joined
            .edit(EventsIntent::Join {
                ids: vec![1, 2, 3, 4],
            })
            .unwrap();
        let mut sequence = midi_line();
        sequence
            .edit(EventsIntent::SetEvents {
                events: joined.events.clone(),
            })
            .unwrap();
        assert_eq!(sequence.events.len(), 3, "64 held on into 64");
        assert_eq!(joins(&sequence), vec![None, None, None]);
        joined
            .edit(EventsIntent::Midi {
                midi: Some(crate::events::MidiSpec::Midi2),
            })
            .unwrap();
        assert_eq!(joined.events, sequence.events);
    }

    /// **A run of joins into one pitch is one note**, with the curves of the
    /// notes it took after its own, and a join between different notes is
    /// left for the caller.
    #[test]
    fn a_run_held_on_is_one_note_with_its_curves() {
        let mut sequence = played();
        sequence.events.push(Event {
            id: 5,
            ..Event::new(4.0, json!({"midinote": 64, "sustain": 0.5}))
        });
        for id in [3, 4] {
            let i = sequence.index(id).unwrap();
            sequence.events[i].automation = vec![
                serde_json::from_value(json!({"id": 10 + id, "target": {"bend": true},
                    "points": [{"at": 0.0, "value": 0.0}, {"at": 0.5, "value": 1.0}]}))
                .unwrap(),
            ];
        }
        set_joins(&mut sequence, 3, Some(2));
        set_joins(&mut sequence, 4, Some(3));
        set_joins(&mut sequence, 5, Some(4));
        held_on(&mut sequence);
        let ids: Vec<u64> = sequence.events.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![1, 2, 4, 5]);
        let held = sequence.get(2).unwrap();
        assert_eq!(render::sustain_of(&held.keys()), 1.5);
        let points: Vec<f64> = held.automation[0].points.iter().map(|p| p.at.0).collect();
        assert_eq!(points, vec![1.0, 1.5], "the curve moved with its note");
        assert_eq!(joined_from(&sequence, 4), Some(2), "what went on from 3");
        assert_eq!(joined_from(&sequence, 5), Some(4), "between two pitches");
    }

    /// **Two notes of one pitch that touch are one note**, in every format:
    /// the curves of both one curve, the first's last value held until the
    /// second's first point; on a page that is a tie, one written value.
    #[test]
    fn notes_of_one_pitch_that_touch_join_as_one_note_with_their_curves() {
        let bend = |id: u64,
                    points: Value|
         -> crate::multitrack::Automation<crate::timebase::Beat> {
            serde_json::from_value(json!({"id": id, "target": {"bend": true}, "points": points}))
                .unwrap()
        };
        let mut sequence = EventSequence::new(vec![
            Event {
                id: 1,
                automation: vec![bend(
                    10,
                    json!([{"at": 0.0, "value": 0.0}, {"at": 0.5, "value": 1.0}]),
                )],
                ..Event::new(0.0, json!({"midinote": 64, "sustain": 1.0}))
            },
            Event {
                id: 2,
                automation: vec![bend(11, json!([{"at": 0.25, "value": -1.0}]))],
                ..Event::new(1.0, json!({"midinote": 64, "sustain": 1.0}))
            },
        ]);
        sequence
            .edit(EventsIntent::Join { ids: vec![1, 2] })
            .unwrap();
        assert_eq!(sequence.events.len(), 1);
        let note = sequence.get(1).unwrap();
        assert_eq!(render::sustain_of(&note.keys()), 2.0);
        let curve = &note.automation[0].points;
        let said = |at: f64| crate::multitrack::nodes::value_at(curve, Beat(at));
        assert_eq!(said(1.0), 1.0, "the first's last value");
        assert_eq!(said(1.2), 1.0, "held to the second's first point");
        assert_eq!(said(1.25), -1.0);
        // on a page: a tie, one value
        let mut page = written_line();
        for event in &mut page.events {
            let mut keys = event.keys();
            keys.insert("dur".into(), json!(1.0));
            event.data = crate::Opaque(Value::Object(keys));
        }
        page.edit(EventsIntent::Join { ids: vec![2, 3] }).unwrap();
        let ids: Vec<u64> = page.events.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![1, 2, 4]);
        assert_eq!(page.get(2).unwrap().keys()["value"], json!([1, 2]));
        assert_eq!(joins(&page), vec![None, None, None]);
    }
}
