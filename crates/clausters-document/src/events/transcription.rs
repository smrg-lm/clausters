//! **A sequence read into a score**: the way back from [`render`](super::score::render).
//!
//! A sequence is what plays -- events in beats, with their curves -- and a
//! score is what is written: exact values, voices on staves, marks. [`read`]
//! makes the second from the first, and **decides only what the events do not
//! say**. An event that carries its notation keys
//! (`clausters_core::event::notation`) is written as they say, and a sequence
//! rendered from a score also carries what is no note's in its `notation`
//! section, so that one is read back with nothing to decide. Everything else
//! is a decision, and each is a rule here:
//!
//! - **When.** An onset is snapped to the grid of the beat it falls in. A
//!   beat's grid is the smallest written value the reading names
//!   (`division`), or -- where the reading admits tuplets and they fit that
//!   beat's onsets clearly better -- the beat in that many parts. The written
//!   value is the event's `value`, else its `dur` snapped the same way, and no
//!   note outlasts the next one of its voice. Nothing here finds a meter or an
//!   irregular value in a performance: the meter is given, and a tuplet is
//!   read only where it is asked for.
//! - **Where.** A staff is what an event says (`staff`), else its channel --
//!   in MIDI 1.0 and 2.0 a channel is a line -- and in an MPE zone, where a
//!   channel is one note's, one staff for the zone. A voice is what the
//!   events say (`voice`), else found: notes that start and end together are
//!   a chord, and a note that starts under another goes to the next voice.
//! - **As what.** A pitch is the event's `pitches`, else its number spelled
//!   by the key -- the spelling nearest the tonic on the line of fifths -- or
//!   by the event's `spelling`. The key is the reading's, the section's, or
//!   the signature most of the notes are in.
//! - **How loud.** A `dynamic` an event states is the mark. Where no event of
//!   a staff states one and the sequence was not rendered from a score, the
//!   level is read back: a curve of the staff's channel on the dynamics'
//!   controller, where it is made of steps and straight ramps -- a step a
//!   dynamic, a ramp a hairpin -- and otherwise the levels of the notes, a
//!   dynamic where the level changes: a line at one level is written with
//!   none. A curve of any other shape was drawn by hand and is no hairpin: it
//!   stays a curve of the sequence.
//!
//! **The sequence is never changed.** What was played keeps the times and the
//! lengths that arrived; how finely it is read is the [`Transcription`]'s,
//! and reading it again with another one is all changing it takes.
//!
//! What comes back with the sheet is `items`: which item each event became,
//! so what is then done to the page can be found in the sequence.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use clausters_core::envshape::{SHAPE_HOLD, SHAPE_LINEAR, SHAPE_STEP};
use clausters_core::event::render::{self, Type};
use clausters_core::notation::{
    Control, Grid, Interpretation, Item, KEYS, Marks, Pitch, Sheet, Spanner, Staff, Step, Voice,
};
use clausters_core::ratio::Ratio;

use super::{CurveKind, EventSequence, MidiSpec};

/// **How a sequence is read into a score**: what the events do not say and a
/// reader has to. Every field has a default, so `{}` is a reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transcription {
    /// The meter, as `"3/4"`. Left out, the sequence's own where it was
    /// rendered from a score, else `4/4`.
    pub meter: Option<String>,
    /// The key, as a tonic name (`"F"`, `"Bb"`). Left out, the sequence's own,
    /// else the signature most of its notes are in.
    pub key: Option<String>,
    /// The clef of every staff the sequence does not say one for. Left out,
    /// each staff's is chosen by its register.
    pub clef: Option<String>,
    /// Which written value is one beat, as its denominator: `4` for a quarter.
    pub beat_unit: i64,
    /// The smallest written value an onset is snapped to, as its denominator:
    /// `16` for a sixteenth.
    pub division: i64,
    /// The tuplets a beat may be read as, each the parts it is divided in:
    /// `[3]` admits triplets. Empty, a beat is read in `division` alone --
    /// but for the tuplets the events themselves state in their `value`.
    pub tuplets: Vec<i64>,
    /// The most voices found on one staff.
    pub voices: usize,
    /// Whether levels are read back as dynamics and hairpins.
    pub dynamics: bool,
}

impl Default for Transcription {
    fn default() -> Self {
        Self {
            meter: None,
            key: None,
            clef: None,
            beat_unit: 4,
            division: 16,
            tuplets: Vec::new(),
            voices: 2,
            dynamics: true,
        }
    }
}

/// A sequence read: the sheet, and which item each event became.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Transcribed {
    /// The score.
    pub sheet: Sheet,
    /// `[event id, item id]` for every note read, in the events' order. A
    /// chord's events name one item, and a note cut in tied parts its first.
    pub items: Vec<(u64, u64)>,
}

/// One note of the sequence, as it is being read.
struct Heard {
    event: u64,
    /// Onset and length in whole notes, as played.
    at: f64,
    len: f64,
    midi: i32,
    keys: Map<String, Value>,
    channel: i64,
    staff: usize,
    /// The item of the score it was rendered from, when the sequence says.
    source: Option<u64>,
    start: Ratio,
    end: Ratio,
}

/// Notes written as one item: a note, or a chord.
struct Group {
    start: Ratio,
    end: Ratio,
    /// Indices into the staff's notes, low to high.
    notes: Vec<usize>,
    voice: Option<usize>,
}

/// How far two levels are apart, as a factor.
fn apart(a: f64, b: f64) -> f64 {
    (a.max(1e-9) / b.max(1e-9)).ln().abs()
}

/// The name of the dynamic nearest `amp`.
fn dynamic_near(interp: &Interpretation, amp: f64) -> Option<&str> {
    interp
        .dynamics
        .iter()
        .min_by(|a, b| apart(*a.1, amp).total_cmp(&apart(*b.1, amp)))
        .map(|(name, _)| name.as_str())
}

/// The key signature of a tonic name, in fifths: sharps positive.
fn fifths_of(key: &str) -> i32 {
    match KEYS.iter().position(|k| *k == key) {
        Some(at) if at <= 7 => at as i32,
        Some(at) => 7 - at as i32,
        None => 0,
    }
}

/// The tonic name of a signature.
fn key_of(fifths: i32) -> &'static str {
    if fifths >= 0 {
        KEYS[fifths.min(7) as usize]
    } else {
        KEYS[(7 - fifths.max(-7)) as usize]
    }
}

/// **The signature most of the notes are in**: the one whose scale holds the
/// most of what sounds, each note weighed by its length; of two that hold as
/// much, the one with fewer signs.
fn key_found(notes: &[Heard]) -> &'static str {
    let mut weight = [0.0f64; 12];
    for note in notes {
        weight[note.midi.rem_euclid(12) as usize] += note.len.max(1e-3);
    }
    let held = |fifths: i32| -> f64 {
        (-1..=5)
            .map(|k| weight[((fifths + k) * 7).rem_euclid(12) as usize])
            .sum()
    };
    let mut best = 0;
    for fifths in [1, -1, 2, -2, 3, -3, 4, -4, 5, -5, 6, -6, 7, -7] {
        if held(fifths) > held(best) + 1e-9 {
            best = fifths;
        }
    }
    key_of(best)
}

/// **A MIDI number spelled in a key**: of the notes it could be, the one
/// nearest the tonic on the line of fifths -- from four fifths under it to
/// seven over, which writes the flat third, sixth and seventh and the sharp
/// first and fourth of a major key.
fn spell(midi: i32, fifths: i32) -> Pitch {
    let low = fifths - 4;
    let place = (midi * 7 - low).rem_euclid(12) + low;
    // the line of fifths, F C G D A E B at -1..5
    let step = [
        Step::F,
        Step::C,
        Step::G,
        Step::D,
        Step::A,
        Step::E,
        Step::B,
    ][(place + 1).rem_euclid(7) as usize];
    let alter = (place + 1).div_euclid(7);
    Pitch {
        step,
        alter,
        octave: (midi - alter).div_euclid(12) - 1,
        forced: false,
    }
}

/// The grids of one staff: each beat's step, and how a time is snapped.
struct Grids {
    beat_unit: i64,
    division: i64,
    /// The parts of each beat read as a tuplet.
    tuplets: BTreeMap<i64, i64>,
}

impl Grids {
    /// The beat `t` (whole notes) falls in.
    fn beat_of(&self, t: f64) -> i64 {
        (t * self.beat_unit as f64 + 1e-9).floor().max(0.0) as i64
    }

    /// The step of beat `beat`.
    fn step(&self, beat: i64) -> Ratio {
        match self.tuplets.get(&beat) {
            Some(parts) => Ratio::new(1, self.beat_unit * parts),
            None => Ratio::new(1, self.division),
        }
    }

    /// `t` at the nearest point of its beat's grid.
    fn snap(&self, t: f64) -> Ratio {
        let beat = self.beat_of(t);
        let step = self.step(beat);
        let from = Ratio::new(beat, self.beat_unit);
        let steps = ((t - from.to_f64()) / step.to_f64()).round() as i64;
        from + step * Ratio::from(steps)
    }

    /// The points at which what lasts from `start` to `end` is cut: the ends
    /// of every beat read as a tuplet, since a tuplet fills its beat.
    fn cuts(&self, start: Ratio, end: Ratio) -> Vec<Ratio> {
        let mut out = vec![start];
        for beat in self.tuplets.keys() {
            for edge in [
                Ratio::new(*beat, self.beat_unit),
                Ratio::new(*beat + 1, self.beat_unit),
            ] {
                if edge > start && edge < end {
                    out.push(edge);
                }
            }
        }
        out.push(end);
        out.sort();
        out.dedup();
        out
    }
}

/// How much a note's end weighs against an onset where a beat's grid is
/// chosen: an end is played far less exactly than a start.
const END_WEIGHT: f64 = 0.25;

/// The squared distance of every point of a beat -- `(time from the beat's
/// start, weight)` -- to a grid of `step` whole notes.
fn misfit(points: &[(f64, f64)], step: f64) -> f64 {
    points
        .iter()
        .map(|(t, weight)| {
            let off = t - (t / step).round() * step;
            off * off * weight
        })
        .sum()
}

/// **The grids of a staff's beats**: `division` everywhere, and a tuplet in a
/// beat whose onsets -- and, weighing less, the ends that fall in it -- it
/// fits clearly better.
fn grids(notes: &[&Heard], how: &Transcription, stated: &BTreeSet<i64>) -> Grids {
    let mut out = Grids {
        beat_unit: how.beat_unit.max(1),
        division: how.division.max(1),
        tuplets: BTreeMap::new(),
    };
    let admitted: BTreeSet<i64> = how
        .tuplets
        .iter()
        .copied()
        .chain(stated.iter().copied())
        .filter(|parts| *parts > 1)
        .collect();
    if admitted.is_empty() {
        return out;
    }
    // each beat's onsets, and the ends that fall in it
    let mut by_beat: BTreeMap<i64, Vec<(f64, f64)>> = BTreeMap::new();
    for note in notes {
        for (t, weight) in [(note.at, 1.0), (note.at + note.len, END_WEIGHT)] {
            let beat = out.beat_of(t);
            let from = beat as f64 / out.beat_unit as f64;
            by_beat.entry(beat).or_default().push((t - from, weight));
        }
    }
    let plain = 1.0 / out.division as f64;
    for (beat, onsets) in by_beat {
        let straight = misfit(&onsets, plain);
        let best = admitted
            .iter()
            .map(|parts| {
                let step = 1.0 / (out.beat_unit * parts) as f64;
                (*parts, misfit(&onsets, step))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        // clearly better: half the misfit, of a misfit worth naming
        if let Some((parts, fit)) = best
            && straight > 1e-9
            && fit < straight * 0.5
        {
            out.tuplets.insert(beat, parts);
        }
    }
    out
}

/// The odd part of a written value's denominator: what it is a tuplet of,
/// or 1.
fn odd_part(value: Ratio) -> i64 {
    let mut odd = value.denom();
    while odd % 2 == 0 {
        odd /= 2;
    }
    odd
}

/// An event's `value`, where it states a positive one.
fn value_of(keys: &Map<String, Value>) -> Option<Ratio> {
    let value = keys.get("value")?.as_array()?;
    let (numer, denom) = (value.first()?.as_i64()?, value.get(1)?.as_i64()?);
    (numer > 0 && denom > 0).then(|| Ratio::new(numer, denom))
}

/// What the sequence's `notation` section says, each part where it reads.
#[derive(Default)]
struct Section {
    grid: Option<Grid>,
    key: Option<String>,
    staves: Vec<Value>,
    spanners: Vec<Value>,
    controls: Vec<Value>,
    marks: Vec<Value>,
    /// event id -> the item of the score it was rendered from.
    items: BTreeMap<u64, u64>,
    rest: Map<String, Value>,
}

impl Section {
    fn of(sequence: &EventSequence) -> Section {
        let Some(map) = sequence.notation.as_ref().and_then(Value::as_object) else {
            return Section::default();
        };
        let list = |key: &str| {
            map.get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        };
        Section {
            grid: map
                .get("grid")
                .and_then(|grid| serde_json::from_value(grid.clone()).ok()),
            key: map.get("key").and_then(Value::as_str).map(str::to_owned),
            staves: list("staves"),
            spanners: list("spanners"),
            controls: list("controls"),
            marks: list("marks"),
            items: list("items")
                .iter()
                .filter_map(|pair| Some((pair.get(0)?.as_u64()?, pair.get(1)?.as_u64()?)))
                .collect(),
            rest: map.clone(),
        }
    }

    /// Whether the sequence was rendered from a score.
    fn rendered(&self) -> bool {
        !self.items.is_empty()
    }
}

/// A grid as a sequence is read on it: a score's repeats are played out in
/// the sequence it rendered, so a grid that repeats is read as its first
/// meter alone, and the music stands written out.
fn grid_read(grid: Grid) -> Grid {
    let repeats = !grid.endings.is_empty()
        || !grid.marks.is_empty()
        || !grid.repeats.is_empty()
        || grid
            .barlines
            .iter()
            .any(|(_, kind)| kind.starts_with("rpt"));
    if !repeats {
        return grid;
    }
    let meter = grid.meter_at(0);
    Grid::uniform(meter.count, meter.unit)
}

/// **Read `sequence` into a score** as `how` says, hearing levels as `interp`
/// names them (see the module).
///
/// # Errors
/// When the meter is not one, or the sequence's own notes cannot be written.
pub fn read(
    sequence: &EventSequence,
    how: &Transcription,
    interp: &Interpretation,
) -> Result<Transcribed, String> {
    let section = Section::of(sequence);
    let beat_unit = how.beat_unit.max(1);
    let mpe = matches!(sequence.midi, Some(MidiSpec::Mpe { .. }));

    // The notes, in whole notes.
    let mut notes: Vec<Heard> = sequence
        .events
        .iter()
        .filter_map(|event| {
            let keys = event.keys();
            if Type::of(&keys) != Type::Note {
                return None;
            }
            let midi = render::pitch_of(&keys)
                .midinote(&render::scale_of(&keys))
                .round() as i32;
            // its written length: its `dur`, else how long it is held, else
            // the beat an event that says neither lasts
            let len = ["dur", "sustain"]
                .iter()
                .find_map(|key| keys.get(*key).and_then(Value::as_f64))
                .unwrap_or(1.0);
            Some(Heard {
                event: event.id,
                at: event.at.0 / beat_unit as f64,
                len: len.max(0.0) / beat_unit as f64,
                midi,
                channel: keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                staff: 0,
                source: section.items.get(&event.id).copied(),
                start: Ratio::ZERO,
                end: Ratio::ZERO,
                keys,
            })
        })
        .collect();

    // Where: the staff each is on.
    let channels: Vec<i64> = notes
        .iter()
        .map(|note| note.channel)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for note in &mut notes {
        let stated = note.keys.get("staff").and_then(Value::as_f64);
        note.staff = match stated {
            Some(staff) => staff as usize,
            None if mpe => 0,
            None => channels
                .iter()
                .position(|c| *c == note.channel)
                .unwrap_or(0),
        };
    }
    let staff_count = notes
        .iter()
        .map(|note| note.staff + 1)
        .max()
        .unwrap_or(1)
        .max(section.staves.len())
        .max(1);

    let key = how
        .key
        .clone()
        .or_else(|| section.key.clone())
        .unwrap_or_else(|| key_found(&notes).to_string());
    let fifths = fifths_of(&key);
    let grid = match (&how.meter, section.grid.clone()) {
        (Some(meter), _) => {
            let (count, unit) = meter
                .split_once('/')
                .and_then(|(n, d)| Some((n.trim().parse().ok()?, d.trim().parse().ok()?)))
                .filter(|(count, unit): &(i64, i64)| *count > 0 && *unit > 0)
                .ok_or_else(|| format!("{meter:?} is not a meter: it is written as \"3/4\""))?;
            Grid::uniform(count, unit)
        }
        (None, Some(grid)) => grid_read(grid),
        (None, None) => Grid::uniform(4, 4),
    };

    let mut next_id = 0u64;
    let mut mint = || {
        next_id += 1;
        next_id
    };
    let mut items_of: BTreeMap<u64, u64> = BTreeMap::new();
    // (staff, start, item id) of every note item, for what is hung on one.
    let mut placed: Vec<(usize, usize, Ratio, u64)> = Vec::new();
    let mut staves = Vec::new();
    for staff in 0..staff_count {
        let on: Vec<usize> = (0..notes.len())
            .filter(|i| notes[*i].staff == staff)
            .collect();
        // When: each onset and each end on its beat's grid.
        let stated: BTreeSet<i64> = on
            .iter()
            .filter_map(|i| value_of(&notes[*i].keys))
            .map(odd_part)
            .filter(|odd| *odd > 1)
            .collect();
        let (grids, register) = {
            let heard: Vec<&Heard> = on.iter().map(|i| &notes[*i]).collect();
            (grids(&heard, how, &stated), clef_for(&heard))
        };
        for i in &on {
            let note = &mut notes[*i];
            note.start = grids.snap(note.at);
            note.end = match value_of(&note.keys) {
                Some(value) => note.start + value,
                None => grids.snap(note.at + note.len),
            };
            if note.end <= note.start {
                note.end = note.start + grids.step(grids.beat_of(note.start.to_f64()));
            }
        }
        // Chords: what was one item of a score, else what starts and ends
        // together in one voice.
        let mut groups: Vec<Group> = Vec::new();
        let mut order = on.clone();
        order.sort_by(|a, b| {
            let (a, b) = (&notes[*a], &notes[*b]);
            a.start.cmp(&b.start).then(b.midi.cmp(&a.midi))
        });
        for i in order {
            let note = &notes[i];
            let voice = note
                .keys
                .get("voice")
                .and_then(Value::as_f64)
                .map(|v| v as usize);
            let joins = groups.iter_mut().rev().find(|group| {
                let first = &notes[group.notes[0]];
                group.start == note.start
                    && group.voice == voice
                    && match (first.source, note.source) {
                        (Some(a), Some(b)) => a == b,
                        _ => group.end == note.end,
                    }
            });
            match joins {
                Some(group) => group.notes.push(i),
                None => groups.push(Group {
                    start: note.start,
                    end: note.end,
                    notes: vec![i],
                    voice,
                }),
            }
        }
        for group in &mut groups {
            group.notes.sort_by_key(|i| notes[*i].midi);
        }
        // Voices: the one a group states, else the first that is free.
        let most = how.voices.max(1);
        let stated_voices = groups
            .iter()
            .filter_map(|g| g.voice)
            .max()
            .map_or(0, |v| v + 1);
        let mut voices: Vec<Vec<Group>> = (0..stated_voices).map(|_| Vec::new()).collect();
        for group in groups {
            let free = |voice: &Vec<Group>| voice.last().is_none_or(|last| last.end <= group.start);
            let at = match group.voice {
                Some(voice) => voice,
                None => match voices.iter().position(free) {
                    Some(voice) => voice,
                    None if voices.len() < most.max(stated_voices) => {
                        voices.push(Vec::new());
                        voices.len() - 1
                    }
                    // no voice is free: the one that ends soonest gives way
                    None => voices
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, voice)| voice.last().map(|last| last.end))
                        .map_or(0, |(at, _)| at),
                },
            };
            if voices.len() <= at {
                voices.resize_with(at + 1, Vec::new);
            }
            match voices[at].last_mut() {
                // two that start together in one voice are one chord
                Some(last) if last.start == group.start => last.notes.extend(group.notes),
                Some(last) => {
                    // no note outlasts the next one of its voice
                    last.end = last.end.min(group.start);
                    voices[at].push(group);
                }
                None => voices[at].push(group),
            }
        }
        if voices.is_empty() {
            voices.push(Vec::new());
        }

        // The items: each voice back to back, a gap a rest, everything cut
        // where a tuplet's beat ends.
        let mut written = Vec::new();
        for (v, voice) in voices.iter().enumerate() {
            let mut items = Vec::new();
            let mut cursor = Ratio::ZERO;
            for group in voice {
                if group.start > cursor {
                    for pair in grids.cuts(cursor, group.start).windows(2) {
                        items.push(Item::Rest {
                            id: mint(),
                            dur: pair[1] - pair[0],
                        });
                    }
                }
                let first = &notes[group.notes[0]];
                let pitches: Vec<Pitch> = group
                    .notes
                    .iter()
                    .map(|i| pitch_of(&notes[*i], fifths))
                    .collect();
                let cuts = grids.cuts(group.start, group.end);
                let parts = cuts.len() - 1;
                for (k, pair) in cuts.windows(2).enumerate() {
                    let id = mint();
                    if k == 0 {
                        for i in &group.notes {
                            items_of.insert(notes[*i].event, id);
                        }
                        placed.push((staff, v, group.start, id));
                    }
                    let dur = pair[1] - pair[0];
                    let last = k + 1 == parts;
                    items.push(Item::Note {
                        id,
                        pitches: pitches.clone(),
                        dur,
                        tie: !last || first.keys.get("tie").and_then(Value::as_bool) == Some(true),
                        marks: if k == 0 {
                            marks_of(group, &notes, &grids, parts == 1)
                        } else {
                            Marks::default()
                        },
                    });
                }
                cursor = group.end.max(cursor);
            }
            written.push(Voice { items });
        }

        // The staff: the section's own where it says one, its voices these.
        let mut out: Staff = section
            .staves
            .get(staff)
            .and_then(|said| {
                let mut said = said.clone();
                said.as_object_mut()?.remove("voices");
                serde_json::from_value(said).ok()
            })
            .unwrap_or_default();
        if section.staves.get(staff).is_none() {
            out.clef = how.clef.clone().unwrap_or(register);
        }
        out.voices = written;
        staves.push(out);
    }

    let mut sheet = Sheet {
        grid,
        key,
        staves,
        ..Sheet::default()
    };
    for (field, value) in &section.rest {
        match field.as_str() {
            "header" => {
                if let Ok(header) = serde_json::from_value(value.clone()) {
                    sheet.header = header;
                }
            }
            "page" => sheet.page = serde_json::from_value(value.clone()).ok(),
            "groups" => {
                if let Ok(groups) = serde_json::from_value(value.clone()) {
                    sheet.groups = groups;
                }
            }
            _ => {}
        }
    }

    // What has two ends, what is written at a point and what a note carries
    // that no key says: the section's, by the event each names.
    let item = |value: Option<&Value>| value.and_then(Value::as_u64).and_then(|e| items_of.get(&e));
    for spanner in &section.spanners {
        if let (Some(kind), Some(from), Some(to)) = (
            spanner.get("kind").and_then(Value::as_str),
            item(spanner.get("from")),
            item(spanner.get("to")),
        ) {
            sheet.spanners.push(Spanner {
                kind: kind.to_string(),
                from: *from,
                to: *to,
            });
        }
    }
    for control in &section.controls {
        if let (Some(on), Ok(mut read)) = (
            item(control.get("on")),
            serde_json::from_value::<Control>(control.clone()),
        ) {
            read.on = *on;
            sheet.controls.push(read);
        }
    }
    for pair in &section.marks {
        let (Some(id), Some(Ok(more))) = (
            item(pair.get(0)),
            pair.get(1)
                .map(|marks| serde_json::from_value::<Marks>(marks.clone())),
        ) else {
            continue;
        };
        with_marks(&mut sheet, *id, |marks| carry(marks, &more));
    }

    if how.dynamics && !section.rendered() {
        levels_read(
            sequence, &notes, &placed, &items_of, interp, beat_unit, &mut sheet,
        );
    }
    sheet.next_id = next_id;
    let items = sequence
        .events
        .iter()
        .filter_map(|event| Some((event.id, *items_of.get(&event.id)?)))
        .collect();
    Ok(Transcribed { sheet, items })
}

/// The written pitch of one note: the one its event states, else its number
/// spelled -- by the event's `spelling`, else by the key.
fn pitch_of(note: &Heard, fifths: i32) -> Pitch {
    let stated = note
        .keys
        .get("pitches")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(|pitch| serde_json::from_value::<Pitch>(pitch.clone()).ok());
    if let Some(pitch) = stated {
        return pitch;
    }
    let mut pitch = match note.keys.get("spelling").and_then(Value::as_str) {
        Some("flat") => Pitch::from_midi(note.midi, true),
        Some("sharp") => Pitch::from_midi(note.midi, false),
        _ => spell(note.midi, fifths),
    };
    pitch.forced = note.keys.get("accidental").and_then(Value::as_str) == Some("written");
    pitch
}

/// The marks of one item: what its events' notation keys say, the first to
/// say each, and -- where the item is one written value -- how long it is
/// held when that is not its value and no articulation says so.
fn marks_of(group: &Group, notes: &[Heard], grids: &Grids, whole: bool) -> Marks {
    let said = |key: &str| {
        group
            .notes
            .iter()
            .find_map(|i| notes[*i].keys.get(key).filter(|value| !value.is_null()))
    };
    let word = |key: &str| said(key).and_then(Value::as_str).map(str::to_owned);
    let mut marks = Marks {
        articulations: said("articulations")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        dynamic: word("dynamic"),
        ornament: word("ornament"),
        grace: word("grace"),
        stem: word("stem"),
        ..Marks::default()
    };
    let stated = group
        .notes
        .iter()
        .map(|i| &notes[*i])
        .find(|note| note.keys.get("sustain").and_then(Value::as_f64).is_some());
    if let Some(note) = stated
        && whole
        && marks.articulations.is_empty()
    {
        let held = render::sustain_of(&note.keys) / grids.beat_unit as f64;
        let step = grids.step(grids.beat_of(note.at));
        let steps = (held / step.to_f64()).round().max(1.0) as i64;
        let sounding = step * Ratio::from(steps);
        if sounding != group.end - group.start {
            marks.sounding = Some(sounding);
        }
    }
    marks
}

/// What `more` says that no notation key does, put on `marks`.
fn carry(marks: &mut Marks, more: &Marks) {
    let kept = marks.clone();
    *marks = more.clone();
    marks.articulations = kept.articulations;
    marks.dynamic = kept.dynamic;
    marks.ornament = kept.ornament;
    marks.grace = kept.grace;
    marks.stem = kept.stem;
    marks.sounding = kept.sounding;
}

/// Changes the marks of the item `id`.
fn with_marks(sheet: &mut Sheet, id: u64, change: impl FnOnce(&mut Marks)) {
    for voice in sheet.voices_mut() {
        for item in &mut voice.items {
            if let Item::Note { id: at, marks, .. } = item
                && *at == id
            {
                change(marks);
                return;
            }
        }
    }
}

/// The clef a staff's notes sit in: the bass clef for a register under
/// middle C.
fn clef_for(notes: &[&Heard]) -> String {
    let mean =
        notes.iter().map(|note| f64::from(note.midi)).sum::<f64>() / notes.len().max(1) as f64;
    if notes.is_empty() || mean >= 57.0 {
        "G2".into()
    } else {
        "F4".into()
    }
}

/// **The levels read back as dynamics and hairpins** (see the module), on
/// each staff none of whose events states a dynamic.
fn levels_read(
    sequence: &EventSequence,
    notes: &[Heard],
    placed: &[(usize, usize, Ratio, u64)],
    items_of: &BTreeMap<u64, u64>,
    interp: &Interpretation,
    beat_unit: i64,
    sheet: &mut Sheet,
) {
    let staves: BTreeSet<usize> = placed.iter().map(|(staff, ..)| *staff).collect();
    for staff in staves {
        let on: Vec<&Heard> = notes.iter().filter(|note| note.staff == staff).collect();
        if on.iter().any(|note| note.keys.contains_key("dynamic")) {
            continue;
        }
        let channels: BTreeSet<i64> = on.iter().map(|note| note.channel).collect();
        let lane = sequence.automation.iter().find(|curve| {
            CurveKind::of(&curve.target.0) == CurveKind::Cc(interp.dynamics_cc)
                && curve
                    .target
                    .0
                    .get("channel")
                    .and_then(Value::as_f64)
                    .is_none_or(|channel| channels.contains(&(channel as i64)))
        });
        // the staff's items in the order they start, its first voice's first
        let mut line: Vec<(Ratio, u64)> = placed
            .iter()
            .filter(|(s, ..)| *s == staff)
            .map(|(_, voice, start, id)| (*voice, *start, *id))
            .fold(
                BTreeMap::new(),
                |mut first: BTreeMap<Ratio, (usize, u64)>, (voice, start, id)| {
                    let held = first.entry(start).or_insert((voice, id));
                    if voice < held.0 {
                        *held = (voice, id);
                    }
                    first
                },
            )
            .into_iter()
            .map(|(start, (_, id))| (start, id))
            .collect();
        line.sort();
        match lane {
            Some(lane) => {
                let shape = |point: &crate::Point| {
                    point
                        .data
                        .0
                        .get("shape")
                        .and_then(Value::as_i64)
                        .map_or(SHAPE_LINEAR, |shape| shape as i32)
                };
                // a curve drawn by hand is no hairpin
                let made = lane
                    .points
                    .iter()
                    .all(|p| [SHAPE_LINEAR, SHAPE_HOLD, SHAPE_STEP].contains(&shape(p)))
                    && lane.points.len() <= 2 * line.len() + 2;
                if !made {
                    continue;
                }
                let whole = |beats: f64| beats / beat_unit as f64;
                let at_or_after = |t: f64| {
                    line.iter()
                        .find(|(start, _)| start.to_f64() >= t - 1e-6)
                        .map(|(_, id)| *id)
                };
                let at_or_before = |t: f64| {
                    line.iter()
                        .rev()
                        .find(|(start, _)| start.to_f64() <= t + 1e-6)
                        .map(|(_, id)| *id)
                };
                let mut written: Option<String> = None;
                for (i, point) in lane.points.iter().enumerate() {
                    let name = dynamic_near(interp, point.value / 127.0).map(str::to_owned);
                    if let (Some(name), Some(id)) = (name, at_or_after(whole(point.at)))
                        && written.as_deref() != Some(name.as_str())
                    {
                        with_marks(sheet, id, |marks| marks.dynamic = Some(name.clone()));
                        written = Some(name);
                    }
                    let Some(next) = lane.points.get(i + 1) else {
                        continue;
                    };
                    let ramps =
                        shape(point) == SHAPE_LINEAR && (next.value - point.value).abs() > 1e-6;
                    if let (true, Some(from), Some(to)) = (
                        ramps,
                        at_or_after(whole(point.at)),
                        at_or_before(whole(next.at)),
                    ) && from != to
                    {
                        sheet.spanners.push(Spanner {
                            kind: if next.value > point.value {
                                "crescendo".into()
                            } else {
                                "diminuendo".into()
                            },
                            from,
                            to,
                        });
                    }
                }
            }
            None => {
                // the levels the notes state: a dynamic where the level
                // changes, and stays changed -- so a line at one level is
                // written with none, as an unmarked page is
                let named: BTreeSet<&str> = on
                    .iter()
                    .filter_map(|note| dynamic_near(interp, render::level_of(&note.keys).amp()))
                    .collect();
                if named.len() < 2 {
                    continue;
                }
                let mut current: Option<(String, f64)> = None;
                let mut said: BTreeSet<u64> = BTreeSet::new();
                for note in &on {
                    let Some(id) = items_of.get(&note.event) else {
                        continue;
                    };
                    if !said.insert(*id) {
                        continue;
                    }
                    let amp = render::level_of(&note.keys).amp();
                    let Some(name) = dynamic_near(interp, amp) else {
                        continue;
                    };
                    let changed = match &current {
                        None => true,
                        Some((held, level)) => {
                            let to = interp.dynamics.get(name).copied().unwrap_or(amp);
                            held != name && apart(amp, to) < apart(amp, *level) * 0.67
                        }
                    };
                    if changed {
                        let level = interp.dynamics.get(name).copied().unwrap_or(amp);
                        current = Some((name.to_string(), level));
                        let name = name.to_string();
                        with_marks(sheet, *id, |marks| marks.dynamic = Some(name));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::events::score::render;
    use clausters_core::notation::sheet_to_mei;

    fn sequence(events: Value) -> EventSequence {
        serde_json::from_value(json!({ "events": events })).expect("a sequence")
    }

    fn read_plain(seq: &EventSequence, how: Value) -> Transcribed {
        let how: Transcription = serde_json::from_value(how).expect("a transcription");
        read(seq, &how, &Interpretation::default()).expect("reads")
    }

    /// The items of one voice, as `(midis, value)` -- no midis for a rest.
    fn line(sheet: &Sheet, staff: usize, voice: usize) -> Vec<(Vec<i32>, (i64, i64))> {
        sheet.staves[staff].voices[voice]
            .items
            .iter()
            .map(|item| {
                (
                    item.pitches().iter().map(Pitch::midi).collect(),
                    (item.dur().numer(), item.dur().denom()),
                )
            })
            .collect()
    }

    #[test]
    fn what_was_played_is_snapped_on_the_page_and_stays_as_it_was() {
        // a line played a little off the beat, a chord, a silence
        let seq = sequence(json!([
            {"at": 0.03, "data": {"midinote": 60, "dur": 0.93}},
            {"at": 1.01, "data": {"midinote": 64, "dur": 0.48}},
            {"at": 2.0, "data": {"midinote": 67, "dur": 2.02}},
            {"at": 2.02, "data": {"midinote": 72, "dur": 1.97}},
        ]));
        let before = seq.clone();
        let got = read_plain(&seq, json!({}));
        assert_eq!(
            line(&got.sheet, 0, 0),
            vec![
                (vec![60], (1, 4)),
                (vec![64], (1, 8)),
                (vec![], (1, 8)),
                (vec![67, 72], (1, 2)),
            ]
        );
        assert_eq!(seq, before, "reading changes nothing");
        // a chord's events are one item
        assert_eq!(got.items.len(), 4);
        assert_eq!(got.items[2].1, got.items[3].1);
        assert!(sheet_to_mei(&got.sheet).is_ok());
        // a coarser reading of the same notes
        let coarse = read_plain(&seq, json!({"division": 4}));
        assert_eq!(line(&coarse.sheet, 0, 0)[1], (vec![64], (1, 4)));
    }

    #[test]
    fn a_note_under_another_is_a_second_voice_and_a_channel_a_staff() {
        let seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 72, "dur": 1.0}},
            {"at": 0.0, "data": {"midinote": 60, "dur": 4.0}},
            {"at": 1.0, "data": {"midinote": 74, "dur": 1.0}},
            {"at": 0.0, "data": {"midinote": 40, "dur": 4.0, "channel": 1}},
        ]));
        let got = read_plain(&seq, json!({}));
        assert_eq!(got.sheet.staves.len(), 2);
        assert_eq!(got.sheet.staves[0].voices.len(), 2);
        assert_eq!(
            line(&got.sheet, 0, 0),
            vec![(vec![72], (1, 4)), (vec![74], (1, 4))]
        );
        assert_eq!(line(&got.sheet, 0, 1), vec![(vec![60], (1, 1))]);
        assert_eq!(got.sheet.staves[0].clef, "G2");
        assert_eq!(got.sheet.staves[1].clef, "F4");
        assert!(sheet_to_mei(&got.sheet).is_ok());
        // one voice asked for: the held note gives way to the next
        let one = read_plain(&seq, json!({"voices": 1}));
        assert_eq!(one.sheet.staves[0].voices.len(), 1);
        // and in an MPE zone a channel is a note's, not a line
        let mut zone = seq.clone();
        zone.midi = Some(MidiSpec::Mpe {
            upper: false,
            members: 15,
        });
        assert_eq!(read_plain(&zone, json!({})).sheet.staves.len(), 1);
    }

    #[test]
    fn a_number_is_spelled_by_the_key_found() {
        // D major: its F and C are sharps, and a B flat is the flat sixth
        let seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 62, "dur": 1.0}},
            {"at": 1.0, "data": {"midinote": 66, "dur": 1.0}},
            {"at": 2.0, "data": {"midinote": 69, "dur": 1.0}},
            {"at": 3.0, "data": {"midinote": 73, "dur": 1.0}},
            {"at": 4.0, "data": {"midinote": 74, "dur": 1.0}},
            {"at": 5.0, "data": {"midinote": 67, "dur": 1.0}},
            {"at": 6.0, "data": {"midinote": 64, "dur": 1.0}},
            {"at": 7.0, "data": {"midinote": 70, "dur": 0.5}},
            {"at": 7.5, "data": {"midinote": 68, "dur": 0.5, "spelling": "flat"}},
        ]));
        let got = read_plain(&seq, json!({}));
        assert_eq!(got.sheet.key, "D");
        let spelled: Vec<(Step, i32)> = got.sheet.staves[0].voices[0]
            .items
            .iter()
            .flat_map(|item| item.pitches().iter().map(|p| (p.step, p.alter)))
            .collect();
        assert_eq!(spelled[1], (Step::F, 1));
        assert_eq!(spelled[3], (Step::C, 1));
        assert_eq!(spelled[7], (Step::B, -1));
        assert_eq!(spelled[8], (Step::A, -1), "the event's own spelling");
        // the key given spells it otherwise
        let flat = read_plain(&seq, json!({"key": "Bb"}));
        assert_eq!(
            flat.sheet.staves[0].voices[0].items[1].pitches()[0].step,
            Step::G
        );
    }

    #[test]
    fn a_triplet_is_read_where_it_is_asked_for() {
        let third = 1.0 / 3.0;
        let seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 60, "dur": third}},
            {"at": third, "data": {"midinote": 62, "dur": third}},
            {"at": 2.0 * third, "data": {"midinote": 64, "dur": third}},
            {"at": 1.0, "data": {"midinote": 65, "dur": 1.0}},
        ]));
        let straight = read_plain(&seq, json!({}));
        assert!(
            line(&straight.sheet, 0, 0)
                .iter()
                .all(|(_, (_, d))| *d != 12),
            "no tuplet is found unasked"
        );
        let got = read_plain(&seq, json!({"tuplets": [3]}));
        assert_eq!(
            line(&got.sheet, 0, 0),
            vec![
                (vec![60], (1, 12)),
                (vec![62], (1, 12)),
                (vec![64], (1, 12)),
                (vec![65], (1, 4)),
            ]
        );
        assert!(sheet_to_mei(&got.sheet).unwrap().contains("<tuplet"));
        // a whole line in triplets, over a barline: a group to each beat
        let swung: Vec<Value> = (0..8)
            .flat_map(|beat| {
                let at = f64::from(beat);
                [
                    json!({"at": at, "data": {"midinote": 60, "dur": 2.0 * third}}),
                    json!({"at": at + 2.0 * third, "data": {"midinote": 62, "dur": third}}),
                ]
            })
            .collect();
        let got = read_plain(&sequence(Value::Array(swung)), json!({"tuplets": [3]}));
        let mei = sheet_to_mei(&got.sheet).expect("a page of triplets");
        assert_eq!(mei.matches("<tuplet").count(), 8, "{mei}");
    }

    #[test]
    fn levels_are_read_back_as_dynamics_and_a_made_curve_as_a_hairpin() {
        let notes = json!([
            {"at": 0.0, "data": {"midinote": 60, "dur": 1.0, "amp": 0.05}},
            {"at": 1.0, "data": {"midinote": 62, "dur": 1.0, "amp": 0.052}},
            {"at": 2.0, "data": {"midinote": 64, "dur": 1.0, "amp": 0.17}},
            {"at": 3.0, "data": {"midinote": 65, "dur": 1.0, "amp": 0.17}},
        ]);
        let seq = sequence(notes.clone());
        let got = read_plain(&seq, json!({}));
        let said: Vec<Option<String>> = got.sheet.staves[0].voices[0]
            .items
            .iter()
            .map(|item| item.marks().and_then(|m| m.dynamic.clone()))
            .collect();
        assert_eq!(said, [Some("p".into()), None, Some("f".into()), None]);
        assert!(
            read_plain(&seq, json!({"dynamics": false})).sheet.staves[0].voices[0]
                .items
                .iter()
                .all(|item| item.marks().is_none_or(|m| m.dynamic.is_none()))
        );

        // a curve of steps and a straight ramp: a dynamic, a hairpin, a dynamic
        let mut seq: EventSequence = serde_json::from_value(json!({
            "events": notes,
            "automation": [{"id": 0, "target": {"cc": 11, "channel": 0}, "points": [
                {"at": 0.0, "value": 0.05 * 127.0},
                {"at": 2.0, "value": 0.17 * 127.0, "data": {"shape": SHAPE_HOLD}},
            ]}],
        }))
        .unwrap();
        let got = read_plain(&seq, json!({}));
        let first = got.sheet.staves[0].voices[0].items[0].id();
        let third = got.sheet.staves[0].voices[0].items[2].id();
        assert_eq!(
            got.sheet.spanners,
            vec![Spanner {
                kind: "crescendo".into(),
                from: first,
                to: third
            }]
        );
        assert_eq!(
            got.sheet.staves[0].voices[0].items[2]
                .marks()
                .unwrap()
                .dynamic
                .as_deref(),
            Some("f")
        );
        // drawn by hand, it is no hairpin
        seq.automation[0].points[0].data = crate::Opaque(json!({"shape": 5, "curve": 3.0}));
        assert!(read_plain(&seq, json!({})).sheet.spanners.is_empty());
    }

    #[test]
    fn a_score_rendered_is_read_back_as_it_was_written() {
        let mei_in = clausters_core::notation::voice_to_sheet(&[], "3/4", "F4", "Eb");
        let mut score = mei_in;
        let pitch = |step, alter, octave| Pitch {
            step,
            alter,
            octave,
            forced: false,
        };
        let note = |id, p: Pitch, dur: (i64, i64), marks: Marks| Item::Note {
            id,
            pitches: vec![p],
            dur: Ratio::new(dur.0, dur.1),
            tie: false,
            marks,
        };
        score.staves[0].voices = vec![Voice {
            items: vec![
                note(
                    1,
                    pitch(Step::F, -1, 3),
                    (1, 4),
                    Marks {
                        dynamic: Some("p".into()),
                        fingering: Some("3".into()),
                        ..Marks::default()
                    },
                ),
                Item::Rest {
                    id: 2,
                    dur: Ratio::new(1, 8),
                },
                note(
                    3,
                    pitch(Step::E, 1, 3),
                    (1, 8),
                    Marks {
                        articulations: vec!["stacc".into()],
                        ..Marks::default()
                    },
                ),
                Item::Note {
                    id: 4,
                    pitches: vec![pitch(Step::C, 0, 3), pitch(Step::G, 0, 3)],
                    dur: Ratio::new(1, 4),
                    tie: false,
                    marks: Marks::default(),
                },
                note(5, pitch(Step::B, -1, 2), (3, 4), Marks::default()),
            ],
        }];
        score.spanners = vec![Spanner {
            kind: "slur".into(),
            from: 3,
            to: 5,
        }];
        score.controls = vec![Control {
            kind: "tempo".into(),
            on: 1,
            text: "Lento".into(),
            bpm: Some(60.0),
        }];
        score.header.title = "Back".into();
        let rendered = render(&score, &Interpretation::default()).unwrap();
        let got = read_plain(&rendered, json!({}));
        // the same page: its notes as written, its marks, what has two ends
        assert_eq!(
            sheet_to_mei(&got.sheet).unwrap(),
            sheet_to_mei(&{
                let mut again = score.clone();
                again.assign_ids();
                again
            })
            .unwrap()
        );
        assert_eq!(got.sheet.staves[0].clef, "F4");
        assert_eq!(got.sheet.header.title, "Back");
        let first = &got.sheet.staves[0].voices[0].items[0];
        assert_eq!(
            first.pitches()[0],
            pitch(Step::F, -1, 3),
            "an F flat stays one"
        );
        assert_eq!(first.marks().unwrap().fingering.as_deref(), Some("3"));
        assert_eq!(got.sheet.controls.len(), 1);
        assert_eq!(got.sheet.controls[0].bpm, Some(60.0));
    }
}
