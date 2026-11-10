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
//! A slur written before `join` existed reads as joined where its notes carry
//! joins, and as attacked otherwise. A line is the notes of one `(staff,
//! voice)` where they say either, else of one channel; a line is **written as
//! notation** where a note of it carries a written `value`, and a line that
//! is not (a pattern's voice, a take) gains no slur.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use clausters_core::event::render::{self, Type};

use super::EventSequence;

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
/// # Errors
/// When an id is not a note, or no line holds two onsets of the selection.
pub fn join(sequence: &mut EventSequence, ids: &[u64]) -> Result<(), String> {
    let chosen = selected(sequence, ids)?;
    let mut touched = BTreeSet::new();
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

/// **A join that no longer says "goes on into the next" goes**: one naming a
/// note the sequence no longer holds -- what an edit that removes notes
/// leaves, whoever made it -- and one into a note that does not start after
/// the note it goes on from, which is what a note moved before it leaves. A
/// join is made in onset order ([`join`]), so a pair the hand has reordered
/// joins nothing, and its line's slurs follow.
pub fn prune(sequence: &mut EventSequence) {
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

    /// A line played: four notes on channel 0, the middle two of one pitch.
    fn played() -> EventSequence {
        let note = |id: u64, at: f64, midinote: i64| Event {
            id,
            ..Event::new(at, json!({"midinote": midinote, "sustain": 1.0}))
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
}
