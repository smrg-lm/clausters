//! **The roll in notation mode**: a gesture read by the rules of notation.
//!
//! In notation mode a roll is a view of a page: a note belongs to a staff and
//! a voice, its onset lands on the reading's grid and its length on a written
//! value, and what it sounds is derived from what is written
//! (`clausters_document::events::realize`). So an edit here writes what a page
//! writes -- `at`, `value`, `dur`, the editing voice's `staff` and `voice`,
//! `pitches` spelled in the key, a `dynamic` where the velocity was moved --
//! and never `sustain` or a level, which the derived performance writes in the
//! same edit, one undo step.
//!
//! - **When.** An onset lands on the reading's division, or on a tuplet it
//!   admits where that is nearer; a length on a written value: plain, dotted,
//!   or a tuplet's. A value across a barline stays one note, which the page
//!   writes with ties.
//! - **In a voice.** Where a part writes its polyphony in voices, two notes
//!   of one voice that overlap are a chord when they start and end together
//!   and are refused with the reason otherwise; where it writes it with ties
//!   they are kept, since one voice holds them all.
//! - **The articulations** (`slur`, `slur_join`, `tie`, `staccato`, `pedal`)
//!   are the page's own marks over the selected notes, heard at once.
//!
//! Nothing is refused for what an instrument cannot play: that is a model of
//! instruments', which this is not.

use serde_json::{Map, Value, json};

/// The roll's verbs over a selection that only notation answers.
pub const ARTICULATIONS: &[&str] = &["slur", "slur_join", "tie", "staccato", "pedal"];

/// **The written value of `beats`**, in whole notes, as `[n, d]`, counted in
/// `beat_unit` beats a whole note -- `None` where no written value is that
/// long (a plain or dotted value, or a tuplet's of 3, 5, 6 or 7).
#[must_use]
pub fn value_of(beats: f64, beat_unit: i64) -> Option<Value> {
    let whole = beats / beat_unit.max(1) as f64;
    if whole <= 0.0 {
        return None;
    }
    for base in [1_i64, 3, 5, 7] {
        let mut d = base;
        while d <= 1024 * base {
            let n = (whole * d as f64).round();
            if n >= 1.0 && (n / d as f64 - whole).abs() < 1e-9 {
                let n = n as i64;
                let g = gcd(n, d);
                return Some(json!([n / g, d / g]));
            }
            d *= 2;
        }
    }
    None
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 {
        a.abs().max(1)
    } else {
        gcd(b, a % b)
    }
}

/// The written value a note states, `[n, d]` as a ratio.
#[cfg_attr(not(feature = "notation"), allow(dead_code))]
fn stated(keys: &Map<String, Value>) -> Option<f64> {
    let pair = keys.get("value")?.as_array()?;
    let (n, d) = (pair.first()?.as_f64()?, pair.get(1)?.as_f64()?);
    (d != 0.0).then(|| n / d)
}

#[cfg(feature = "notation")]
pub use gated::intake;

#[cfg(feature = "notation")]
mod gated {
    use std::collections::{BTreeMap, BTreeSet};

    use serde_json::{Map, Value, json};

    use clausters_core::event::render::{self, Type};
    use clausters_core::notation::{Interpretation, Pitch, default_interpretation};
    use clausters_document::events::reading::Transcription;
    use clausters_document::events::{Event, EventSequence, EventsIntent, Polyphony};
    use clausters_document::{Beat, Opaque};

    use super::super::{Axis, Editing, SEXTUPLE, YDomain, monophonic, staff_voice};
    use super::{stated, value_of};
    use crate::intake::{Intake, groups, number};

    /// **What a gesture of the roll in notation mode means** (see the
    /// module): one edit that puts the sequence as the gesture leaves it, its
    /// performance derived from what is written.
    pub fn intake(
        sequence: &EventSequence,
        tag: &str,
        values: &[Value],
        axis: &Axis,
        domain: &YDomain,
        editing: &Editing,
    ) -> Intake {
        let interp = default_interpretation();
        let (after, label) = match tag {
            "notes" => (
                notes(sequence, values, axis, domain, editing, &interp),
                "edit the notes",
            ),
            "slur" => (articulate(sequence, tag, values, editing), "write a slur"),
            "slur_join" => (
                articulate(sequence, tag, values, editing),
                "join or attack a slur",
            ),
            "tie" => (articulate(sequence, tag, values, editing), "tie"),
            "staccato" => (articulate(sequence, tag, values, editing), "staccato"),
            "pedal" => (articulate(sequence, tag, values, editing), "pedal"),
            _ => return Intake::nothing(),
        };
        let mut after = match after {
            Ok(after) => after,
            Err(why) => return Intake::refused(why),
        };
        // what it sounds, from what is written
        after = match clausters_document::events::realize::realize(&after, &interp) {
            Ok(realized) => realized,
            Err(why) => return Intake::refused(why),
        };
        let intent = EventsIntent::Restore {
            sequence: Box::new(after),
        };
        Intake::edit(serde_json::to_value(&intent).unwrap_or(Value::Null), label)
    }

    /// **The grid of the reading**: where an onset lands and what a length
    /// is written as, in beats.
    struct Grid {
        beat_unit: i64,
        /// The division's step, in beats.
        step: f64,
        /// The tuplets a beat may be read as.
        tuplets: Vec<i64>,
    }

    impl Grid {
        fn of(how: &Transcription) -> Self {
            let beat_unit = how.beat_unit.max(1);
            Self {
                beat_unit,
                step: beat_unit as f64 / how.division.max(1) as f64,
                tuplets: how.tuplets.iter().copied().filter(|n| *n > 1).collect(),
            }
        }

        /// An onset on the division, or on a tuplet's grid where nearer.
        fn onset(&self, beat: f64) -> f64 {
            let snap = |step: f64| (beat / step).round() * step;
            let mut best = snap(self.step).max(0.0);
            for n in &self.tuplets {
                let candidate = snap(1.0 / *n as f64).max(0.0);
                if (candidate - beat).abs() + 1e-9 < (best - beat).abs() {
                    best = candidate;
                }
            }
            best
        }

        /// **The written value nearest `beats`**: its length in beats, and
        /// the value `[n, d]`.
        fn length(&self, beats: f64) -> (f64, Value) {
            let whole = self.beat_unit as f64;
            let mut candidates: Vec<(i64, i64)> = Vec::new();
            // plain and dotted, from a breve to the division's step
            let smallest = (whole / self.step).round().max(1.0) as i64;
            let mut d = 1;
            while d <= smallest.max(1) {
                candidates.push((1, d));
                if 2 * d <= smallest {
                    candidates.push((3, 2 * d));
                }
                d *= 2;
            }
            candidates.push((2, 1));
            // a tuplet's values, of one beat in n
            for n in &self.tuplets {
                candidates.push((1, self.beat_unit * n));
                candidates.push((2, self.beat_unit * n));
            }
            let beats = beats.max(1e-9);
            let (n, d) = candidates
                .into_iter()
                .min_by(|a, b| {
                    let off =
                        |(n, d): &(i64, i64)| ((*n as f64 / *d as f64 * whole) / beats).ln().abs();
                    off(a).total_cmp(&off(b))
                })
                .unwrap_or((1, 4));
            (n as f64 / d as f64 * whole, json!([n, d]))
        }
    }

    /// The dynamic whose level is nearest `velocity`.
    fn dynamic_near(interp: &Interpretation, velocity: f64) -> Option<String> {
        let amp = clausters_core::event::amp_of_velocity(velocity).max(1e-9);
        interp
            .dynamics
            .iter()
            .min_by(|a, b| {
                let off = |level: f64| (level.max(1e-9) / amp).ln().abs();
                off(*a.1).total_cmp(&off(*b.1))
            })
            .map(|(name, _)| name.clone())
    }

    /// The key a page of the sequence is in, and whether it spells with flats.
    fn flats(sequence: &EventSequence, how: &Transcription) -> bool {
        let key = how
            .key
            .clone()
            .or_else(|| {
                sequence
                    .notation
                    .as_ref()
                    .and_then(|n| n.get("key"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "C".into());
        matches!(key.as_str(), "F" | "Bb" | "Eb" | "Ab" | "Db" | "Gb" | "Cb")
    }

    /// The channel the voice `[staff, voice]` plays on: a note of it says,
    /// else the voice's rank among the voices the sequence writes, as a
    /// render numbers them.
    fn channel_of(sequence: &EventSequence, voice: [usize; 2]) -> i64 {
        let notes: Vec<Map<String, Value>> = sequence
            .events
            .iter()
            .map(Event::keys)
            .filter(|keys| Type::of(keys) == Type::Note)
            .collect();
        if let Some(keys) = notes.iter().find(|keys| staff_voice(keys) == voice) {
            return keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64;
        }
        let mut voices: BTreeSet<[usize; 2]> = notes.iter().map(staff_voice).collect();
        voices.insert(voice);
        voices.iter().position(|v| *v == voice).unwrap_or(0) as i64
    }

    /// **The sequence after a `notes` gesture in notation mode**: each note
    /// reported on the reading's grid and written values, a new one in the
    /// editing voice, a velocity moved written as the dynamic it is nearest;
    /// refused where two notes of a voice overlap in a part written in
    /// voices.
    fn notes(
        sequence: &EventSequence,
        values: &[Value],
        axis: &Axis,
        domain: &YDomain,
        editing: &Editing,
        interp: &Interpretation,
    ) -> Result<EventSequence, String> {
        let how = Transcription::of(sequence);
        let grid = Grid::of(&how);
        let drawn = |event: &Event| {
            let keys = event.keys();
            domain.value(&keys).is_some()
                && crate::events::label_of(&event.data.0).is_none()
                && editing.shows(sequence, &keys)
        };
        let mut events: Vec<Event> = sequence
            .events
            .iter()
            .filter(|e| !drawn(e))
            .cloned()
            .collect();
        let flats = flats(sequence, &how);
        for sextuple in groups(values, SEXTUPLE) {
            let id = number(&sextuple[0]).max(0.0) as u64;
            let start = number(&sextuple[1]);
            let played = axis.beat(start + number(&sextuple[2])) - axis.beat(start);
            let at = grid.onset(axis.beat(start));
            let y = number(&sextuple[3]);
            let velocity = number(&sextuple[4]).round();
            let held = (id != 0)
                .then(|| sequence.get(id))
                .flatten()
                .filter(|e| drawn(e));
            let event = match held {
                Some(was) => {
                    let mut keys = was.keys();
                    if domain.value(&keys).is_none_or(|v| (v - y).abs() > 1e-4) {
                        render::set_key(&mut keys, &domain.key, json!(y));
                    }
                    let length = keys.get("dur").and_then(Value::as_f64);
                    if length.is_none_or(|l| (l - played).abs() > 1e-4) {
                        let (beats, value) = grid.length(played);
                        keys.insert("dur".into(), json!(beats));
                        keys.insert("value".into(), value);
                    }
                    if render::level_of(&keys).velocity() != velocity
                        && let Some(name) = dynamic_near(interp, velocity)
                    {
                        keys.insert("dynamic".into(), json!(name));
                    }
                    Event {
                        at: Beat(at),
                        data: Opaque(Value::Object(keys)),
                        ..was.clone()
                    }
                }
                None => {
                    let (beats, value) = grid.length(played);
                    let mut keys = Map::new();
                    keys.insert(domain.key.clone(), json!(y));
                    keys.insert("dur".into(), json!(beats));
                    keys.insert("value".into(), value);
                    keys.insert("staff".into(), json!(editing.voice[0]));
                    keys.insert("voice".into(), json!(editing.voice[1]));
                    keys.insert("channel".into(), json!(channel_of(sequence, editing.voice)));
                    if domain.key == "midinote" {
                        let pitch = Pitch::from_midi(y.round() as i32, flats);
                        keys.insert("pitches".into(), json!([pitch]));
                    }
                    Event::new(at, Value::Object(keys))
                }
            };
            events.push(event);
        }
        if editing.monophonic {
            monophonic(sequence, &mut events, true);
        }
        refuse_overlaps(sequence, &events)?;
        let mut after = sequence.clone();
        after.edit(EventsIntent::SetEvents { events })?;
        Ok(after)
    }

    /// **Two notes of one voice may not overlap** where its part writes its
    /// polyphony in voices, but as a chord: notes that start and end together.
    fn refuse_overlaps(sequence: &EventSequence, events: &[Event]) -> Result<(), String> {
        let parts = sequence.parts_read();
        let mut ties = BTreeSet::new();
        let mut first = 0;
        for part in &parts {
            if part.writing.polyphony == Polyphony::Ties {
                ties.extend(first..first + part.staves.max(1));
            }
            first += part.staves.max(1);
        }
        let mut voices: BTreeMap<[usize; 2], Vec<(f64, f64, i64)>> = BTreeMap::new();
        for event in events {
            let keys = event.keys();
            if Type::of(&keys) != Type::Note {
                continue;
            }
            let length = keys
                .get("dur")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| render::sustain_of(&keys));
            let midi = render::pitch_of(&keys)
                .midinote(&render::scale_of(&keys))
                .round() as i64;
            voices.entry(staff_voice(&keys)).or_default().push((
                event.at.0,
                event.at.0 + length,
                midi,
            ));
        }
        for ([staff, voice], mut notes) in voices {
            if ties.contains(&staff) {
                continue;
            }
            notes.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (i, a) in notes.iter().enumerate() {
                for b in &notes[i + 1..] {
                    if b.0 >= a.1 - 1e-9 {
                        break;
                    }
                    let chord = (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9;
                    if !chord {
                        return Err(format!(
                            "two notes of voice {} of staff {} overlap at beat {}: a voice holds \
                             one note at a time, or a chord of notes that start and end \
                             together -- write one in another voice, or the part with ties",
                            voice + 1,
                            staff + 1,
                            b.0
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// **An articulation over the selected notes** (the ids in `values`),
    /// written as a page writes it: a staccato on each, a slur or the pedal
    /// from the first to the last, a slur's join turned, two notes of one
    /// pitch that touch tied into one.
    fn articulate(
        sequence: &EventSequence,
        tag: &str,
        values: &[Value],
        editing: &Editing,
    ) -> Result<EventSequence, String> {
        let ids: BTreeSet<u64> = values.iter().map(|v| number(v).max(0.0) as u64).collect();
        let mut chosen: Vec<&Event> = sequence
            .events
            .iter()
            .filter(|e| ids.contains(&e.id) && Type::of(&e.keys()) == Type::Note)
            .collect();
        if chosen.is_empty() {
            return Err("nothing is selected".into());
        }
        chosen.sort_by(|a, b| a.at.0.total_cmp(&b.at.0));
        let mut after = sequence.clone();
        match tag {
            "staccato" => {
                let all = chosen
                    .iter()
                    .all(|e| articulations(&e.keys()).contains(&"stacc".to_string()));
                for id in chosen.iter().map(|e| e.id).collect::<Vec<_>>() {
                    let Some(event) = after.events.iter_mut().find(|e| e.id == id) else {
                        continue;
                    };
                    let mut keys = event.keys();
                    let mut marks = articulations(&keys);
                    marks.retain(|m| m != "stacc");
                    if !all {
                        marks.push("stacc".into());
                    }
                    if marks.is_empty() {
                        keys.remove("articulations");
                    } else {
                        keys.insert("articulations".into(), json!(marks));
                    }
                    event.data = Opaque(Value::Object(keys));
                }
            }
            "slur" | "pedal" => {
                let (from, to) = (chosen[0].id, chosen[chosen.len() - 1].id);
                if from == to {
                    return Err(format!("a {tag} runs from one note to another: select two"));
                }
                let mut spanners = spanners(&after);
                let held = spanners.iter().position(|s| {
                    s.get("kind").and_then(Value::as_str) == Some(tag)
                        && s.get("from").and_then(Value::as_u64) == Some(from)
                        && s.get("to").and_then(Value::as_u64) == Some(to)
                });
                match held {
                    // the same one again takes it away
                    Some(at) => {
                        spanners.remove(at);
                    }
                    None => {
                        let mut said = json!({"kind": tag, "from": from, "to": to});
                        if tag == "slur" {
                            // joined where the roll edits one note at a time
                            said["join"] = json!(editing.monophonic);
                        }
                        spanners.push(said);
                    }
                }
                let before = self::spanners(sequence);
                set_spanners(&mut after, spanners.clone());
                if tag == "slur" {
                    clausters_document::events::articulation::joins_from_page(
                        &mut after, &before, &spanners,
                    );
                }
            }
            "slur_join" => {
                let mut spanners = spanners(&after);
                let mut turned = false;
                for s in &mut spanners {
                    let named = |end: &str| {
                        s.get(end)
                            .and_then(Value::as_u64)
                            .is_some_and(|id| ids.contains(&id))
                    };
                    if s.get("kind").and_then(Value::as_str) == Some("slur")
                        && (named("from") || named("to"))
                    {
                        let join = s.get("join").and_then(Value::as_bool).unwrap_or(false);
                        s["join"] = json!(!join);
                        turned = true;
                    }
                }
                if !turned {
                    return Err("no slur starts or ends on what is selected".into());
                }
                let before = self::spanners(sequence);
                set_spanners(&mut after, spanners.clone());
                clausters_document::events::articulation::joins_from_page(
                    &mut after, &before, &spanners,
                );
            }
            "tie" => {
                let mut merged = false;
                let mut gone: BTreeSet<u64> = BTreeSet::new();
                let beat_unit = Transcription::of(sequence).beat_unit;
                for (i, a) in chosen.iter().enumerate() {
                    if gone.contains(&a.id) {
                        continue;
                    }
                    let mut length = written(&a.keys());
                    for b in &chosen[i + 1..] {
                        let (ka, kb) = (a.keys(), b.keys());
                        let same = render::pitch_of(&ka).midinote(&render::scale_of(&ka))
                            == render::pitch_of(&kb).midinote(&render::scale_of(&kb))
                            && staff_voice(&ka) == staff_voice(&kb);
                        if same && !gone.contains(&b.id) && (a.at.0 + length - b.at.0).abs() < 1e-6
                        {
                            length += written(&kb);
                            gone.insert(b.id);
                            merged = true;
                        }
                    }
                    if let Some(event) = after.events.iter_mut().find(|e| e.id == a.id) {
                        let mut keys = event.keys();
                        keys.insert("dur".into(), json!(length));
                        if let Some(value) = value_of(length, beat_unit) {
                            keys.insert("value".into(), value);
                        }
                        event.data = Opaque(Value::Object(keys));
                    }
                }
                if !merged {
                    return Err(
                        "a tie holds one pitch across two notes of a voice that touch".into(),
                    );
                }
                let events: Vec<Event> = after
                    .events
                    .iter()
                    .filter(|e| !gone.contains(&e.id))
                    .cloned()
                    .collect();
                after.edit(EventsIntent::SetEvents { events })?;
            }
            _ => {}
        }
        Ok(after)
    }

    /// A note's written length, in beats.
    fn written(keys: &Map<String, Value>) -> f64 {
        keys.get("dur")
            .and_then(Value::as_f64)
            .or_else(|| stated(keys).map(|v| v * 4.0))
            .unwrap_or_else(|| render::sustain_of(keys))
    }

    fn articulations(keys: &Map<String, Value>) -> Vec<String> {
        keys.get("articulations")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The section's spanners.
    fn spanners(sequence: &EventSequence) -> Vec<Value> {
        sequence
            .notation
            .as_ref()
            .and_then(|n| n.get("spanners"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The section with these spanners.
    fn set_spanners(sequence: &mut EventSequence, spanners: Vec<Value>) {
        let mut section = sequence
            .notation
            .as_ref()
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        section.insert("spanners".into(), Value::Array(spanners));
        sequence.notation = Some(Value::Object(section));
    }
}
