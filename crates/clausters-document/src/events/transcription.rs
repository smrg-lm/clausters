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
//!   channel is one note's, one staff for the zone. Where the sequence states
//!   its parts, a channel is its part's: a part is on its consecutive staves,
//!   a staff to each channel it is played on, and the last of its channels
//!   splits at middle C onto the staff below where the part has more staves
//!   than channels. A voice is what the events say (`voice`), else found:
//!   notes that start and end together are a chord, and a note that starts
//!   while another still **sounds** goes to the next voice -- one that starts
//!   after it was let go follows it in its own, and cuts its written value
//!   there -- up to as many voices a staff as the part writes.
//! - **With ties.** A part written with ties is one voice of chords on each
//!   of its staves: every note cut where another starts or ends and tied
//!   across the cut, the `voice` its events say aside. It is the same music
//!   as the part written in voices and sounds the same; which one a part is
//!   written in is its `writing`, and changing it changes no event.
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

use serde::Serialize;
use serde_json::{Map, Value};

use clausters_core::envshape::{SHAPE_HOLD, SHAPE_LINEAR, SHAPE_STEP};
use clausters_core::event::render::{self, Type};
use clausters_core::notation::{
    Control, Grid, Interpretation, Item, KEYS, Marks, Pitch, Sheet, Spanner, Staff, Step, Voice,
};
use clausters_core::ratio::Ratio;

use super::part::{Part, Polyphony, Writing};
use super::{CurveKind, EventSequence, MidiSpec};

pub use super::reading::Transcription;

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
    /// How long it sounds, in whole notes.
    held: f64,
    midi: i32,
    keys: Map<String, Value>,
    channel: i64,
    staff: usize,
    /// The item of the score it was rendered from, when the sequence says.
    source: Option<u64>,
    start: Ratio,
    end: Ratio,
    /// Where it stops sounding, on its beat's grid.
    until: Ratio,
}

/// Notes written as one item: a note, or a chord.
struct Group {
    start: Ratio,
    end: Ratio,
    /// Where the last of them stops sounding.
    until: Ratio,
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
                held: render::sustain_of(&keys).max(0.0) / beat_unit as f64,
                midi,
                channel: keys.get("channel").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                staff: 0,
                source: section.items.get(&event.id).copied(),
                start: Ratio::ZERO,
                end: Ratio::ZERO,
                until: Ratio::ZERO,
                keys,
            })
        })
        .collect();

    // Where: the staff each is on -- by its part, where the sequence states
    // its parts.
    let channels = sequence.note_channels();
    let layout = (!sequence.parts.is_empty()).then(|| Layout::of(&sequence.parts, &channels));
    for note in &mut notes {
        let stated = note.keys.get("staff").and_then(Value::as_f64);
        note.staff = match (stated, &layout) {
            (Some(staff), _) => staff as usize,
            (None, Some(layout)) => layout.staff_of(note.channel, note.midi),
            (None, None) if mpe => 0,
            (None, None) => channels
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
        .max(layout.as_ref().map_or(0, |layout| layout.staves))
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
            // where it stops sounding: no later than it is written to
            note.until = grids.snap(note.at + note.held).min(note.end);
        }
        // How the staff's part writes its polyphony: in one voice of
        // chords, each note cut and tied where another starts or ends, or in
        // voices.
        let writing = layout.as_ref().and_then(|layout| layout.writing_of(staff));
        let written = if writing.is_some_and(|w| w.polyphony == Polyphony::Ties) {
            vec![tied_voice(
                &on,
                &notes,
                &grids,
                fifths,
                &mut mint,
                &mut items_of,
                &mut placed,
                staff,
            )]
        } else {
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
                    Some(group) => {
                        group.until = group.until.max(note.until);
                        group.notes.push(i);
                    }
                    None => groups.push(Group {
                        start: note.start,
                        end: note.end,
                        until: note.until,
                        notes: vec![i],
                        voice,
                    }),
                }
            }
            for group in &mut groups {
                group.notes.sort_by_key(|i| notes[*i].midi);
            }
            // Voices: the one a group states, else the first that is free.
            let most = writing
                .map_or(how.voices, |w| usize::from(w.voices_per_staff))
                .max(1);
            let stated_voices = groups
                .iter()
                .filter_map(|g| g.voice)
                .max()
                .map_or(0, |v| v + 1);
            let mut voices: Vec<Vec<Group>> = (0..stated_voices).map(|_| Vec::new()).collect();
            for group in groups {
                // A voice is free once its last note has stopped **sounding**: a
                // note written long and let go early -- an event's `dur` is the
                // time to the next of a line, not what it holds -- leaves its
                // voice to the next one, and its value is cut where that starts.
                let free =
                    |voice: &Vec<Group>| voice.last().is_none_or(|last| last.until <= group.start);
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
                            .min_by_key(|(_, voice)| voice.last().map(|last| last.until))
                            .map_or(0, |(at, _)| at),
                    },
                };
                if voices.len() <= at {
                    voices.resize_with(at + 1, Vec::new);
                }
                match voices[at].last_mut() {
                    // two that start together in one voice are one chord
                    Some(last) if last.start == group.start => {
                        last.until = last.until.max(group.until);
                        last.notes.extend(group.notes);
                        // a chord's notes low to high, however they met
                        last.notes.sort_by_key(|i| notes[*i].midi);
                    }
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
                            tie: !last
                                || first.keys.get("tie").and_then(Value::as_bool) == Some(true),
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

            written
        };

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

    // What a staff is called and how it transposes: its part's, where the
    // sequence states its parts.
    let mut first = 0;
    for part in &sequence.parts {
        for (i, staff) in staves.iter_mut().skip(first).take(part.staves).enumerate() {
            if i == 0 {
                staff.label.clone_from(&part.name);
                staff.abbr.clone_from(&part.abbr);
            }
            staff.transpose = part.transpose;
        }
        first += part.staves;
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
                join: spanner.get("join").and_then(Value::as_bool),
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

/// **Where a sequence's stated parts put its notes**: each part on its
/// consecutive staves, each played on its channels, or on an MPE zone, or --
/// for a part that names neither -- on the channels no other part claims, one
/// a staff, in order. A channel no part plays reads as a staff of its own,
/// after the parts'.
struct Layout {
    /// Each staff's part's writing, for the parts' staves.
    writing: Vec<Writing>,
    /// A channel's part: its first staff, the channel's rank among the
    /// part's, and whether its notes are split by register onto the staff
    /// below -- the last channel of a part with more staves than channels.
    channels: BTreeMap<i64, (usize, usize, usize, bool)>,
    /// The zone's part, its first staff and how many it has.
    zone: Option<(usize, usize)>,
    /// The channels no part plays, each a staff after the parts'.
    loose: Vec<i64>,
    /// The parts' staves: a loose channel's is there where a note is on it.
    staves: usize,
}

impl Layout {
    fn of(parts: &[Part], heard: &[i64]) -> Self {
        let claimed: BTreeSet<i64> = parts.iter().flat_map(|p| p.channels.clone()).collect();
        let has_zone = parts.iter().any(|p| p.zone.is_some());
        let mut free = heard.iter().filter(|c| !claimed.contains(c)).copied();
        let mut out = Self {
            writing: Vec::new(),
            channels: BTreeMap::new(),
            zone: None,
            loose: Vec::new(),
            staves: 0,
        };
        let mut first = 0;
        for part in parts {
            let staves = part.staves.max(1);
            let played: Vec<i64> = if !part.channels.is_empty() {
                part.channels.clone()
            } else if part.zone.is_some() {
                if out.zone.is_none() {
                    out.zone = Some((first, staves));
                }
                Vec::new()
            } else if has_zone {
                // what no part names is the zone's
                Vec::new()
            } else {
                free.by_ref().take(staves).collect()
            };
            for (rank, channel) in played.iter().enumerate() {
                let split = played.len() < staves && rank + 1 == played.len();
                out.channels.entry(*channel).or_insert((
                    first,
                    rank.min(staves - 1),
                    staves,
                    split,
                ));
            }
            out.writing
                .extend(std::iter::repeat_n(part.writing, staves));
            first += staves;
        }
        if !has_zone {
            out.loose = free.collect();
        }
        out.staves = first;
        out
    }

    /// The staff a note on `channel` sounding `midi` is written on. A part
    /// with more staves than channels splits its last channel's notes at
    /// middle C, the lower onto the staff below.
    fn staff_of(&self, channel: i64, midi: i32) -> usize {
        let low = usize::from(midi < 60);
        if let Some(&(first, rank, staves, split)) = self.channels.get(&channel) {
            let rank = if split { rank + low } else { rank };
            return first + rank.min(staves - 1);
        }
        if let Some((first, staves)) = self.zone {
            return first + low.min(staves - 1);
        }
        let parts = self.writing.len();
        self.loose
            .iter()
            .position(|c| *c == channel)
            .map_or(0, |at| parts + at)
    }

    /// How the part of `staff` writes its polyphony, for a part's staff.
    fn writing_of(&self, staff: usize) -> Option<Writing> {
        self.writing.get(staff).copied()
    }
}

/// **One staff written in one voice of chords**: every note cut where
/// another starts or ends, and tied across the cut -- the polyphony of a part
/// written with ties. A note lasts as it is written, but not past the next
/// note of its pitch; a pitch that ends where the same pitch starts again is
/// attacked again, so the tie across that cut names the pitches it holds.
#[allow(clippy::too_many_arguments)]
fn tied_voice(
    on: &[usize],
    notes: &[Heard],
    grids: &Grids,
    fifths: i32,
    mint: &mut impl FnMut() -> u64,
    items_of: &mut BTreeMap<u64, u64>,
    placed: &mut Vec<(usize, usize, Ratio, u64)>,
    staff: usize,
) -> Voice {
    // (note, start, end), a note cut where the next of its pitch starts
    let mut extent: Vec<(usize, Ratio, Ratio)> = on
        .iter()
        .map(|i| (*i, notes[*i].start, notes[*i].end))
        .collect();
    extent.sort_by(|a, b| a.1.cmp(&b.1).then(notes[a.0].midi.cmp(&notes[b.0].midi)));
    for a in 0..extent.len() {
        let (i, start, end) = extent[a];
        let next = extent
            .iter()
            .filter(|(j, s, _)| *j != i && notes[*j].midi == notes[i].midi && *s > start)
            .map(|(_, s, _)| *s)
            .min();
        if let Some(next) = next
            && next < end
        {
            extent[a].2 = next;
        }
    }
    let points: Vec<Ratio> = extent
        .iter()
        .flat_map(|(_, s, e)| [*s, *e])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let sounding = |a: Ratio| -> Vec<(usize, Ratio, Ratio)> {
        let mut here: Vec<_> = extent
            .iter()
            .filter(|(_, s, e)| *s <= a && *e > a)
            .copied()
            .collect();
        here.sort_by_key(|(i, ..)| notes[*i].midi);
        here
    };
    let mut items = Vec::new();
    let rests = |items: &mut Vec<Item>, mint: &mut dyn FnMut() -> u64, from: Ratio, to: Ratio| {
        for pair in grids.cuts(from, to).windows(2) {
            items.push(Item::Rest {
                id: mint(),
                dur: pair[1] - pair[0],
            });
        }
    };
    if let Some(first) = points.first()
        && *first > Ratio::ZERO
    {
        rests(&mut items, mint, Ratio::ZERO, *first);
    }
    for (w, pair) in points.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        let here = sounding(a);
        if here.is_empty() {
            rests(&mut items, mint, a, b);
            continue;
        }
        let pitches: Vec<Pitch> = here
            .iter()
            .map(|(i, ..)| pitch_of(&notes[*i], fifths))
            .collect();
        let starting: Vec<usize> = here
            .iter()
            .filter(|(_, s, _)| *s == a)
            .map(|(i, ..)| *i)
            .collect();
        // what sounds on into the next cut, and whether the next cut also
        // attacks a pitch this one has, which the tie must then not hold
        let held: Vec<Pitch> = here
            .iter()
            .zip(&pitches)
            .filter(|((_, _, e), _)| *e > b)
            .map(|(_, p)| *p)
            .collect();
        let next: Vec<Pitch> = points
            .get(w + 2)
            .map(|_| sounding(b))
            .unwrap_or_default()
            .iter()
            .map(|(i, ..)| pitch_of(&notes[*i], fifths))
            .collect();
        let shared = pitches
            .iter()
            .filter(|p| next.iter().any(|q| q.same_note(p)))
            .count();
        let tied = if held.len() < shared {
            held.clone()
        } else {
            Vec::new()
        };
        let cuts = grids.cuts(a, b);
        let parts = cuts.len() - 1;
        let whole = parts == 1
            && starting
                .iter()
                .all(|i| extent.iter().any(|(j, _, e)| j == i && *e == b));
        let marks = if starting.is_empty() {
            Marks::default()
        } else {
            let group = Group {
                start: a,
                end: b,
                until: b,
                notes: starting.clone(),
                voice: Some(0),
            };
            marks_of(&group, notes, grids, whole)
        };
        for (k, cut) in cuts.windows(2).enumerate() {
            let id = mint();
            if k == 0 && !starting.is_empty() {
                for i in &starting {
                    items_of.insert(notes[*i].event, id);
                }
                placed.push((staff, 0, a, id));
            }
            let last = k + 1 == parts;
            let mut carried = if k == 0 {
                marks.clone()
            } else {
                Marks::default()
            };
            if last {
                carried.tied.clone_from(&tied);
            }
            items.push(Item::Note {
                id,
                pitches: pitches.clone(),
                dur: cut[1] - cut[0],
                tie: !last || !held.is_empty(),
                marks: carried,
            });
        }
    }
    Voice { items }
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
                let shape = |point: &crate::Point<crate::Beat>| {
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
                    if let (Some(name), Some(id)) = (name, at_or_after(whole(point.at.0)))
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
                        at_or_after(whole(point.at.0)),
                        at_or_before(whole(next.at.0)),
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
                            join: None,
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
    fn a_note_let_go_before_the_next_is_in_its_voice_and_cut_there() {
        // each written a beat long, as an event that states no length is,
        // held four tenths of one and half a beat from the next: one line
        let seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 60, "dur": 1.0, "sustain": 0.4}},
            {"at": 0.5, "data": {"midinote": 64, "dur": 1.0, "sustain": 0.4}},
            {"at": 1.0, "data": {"midinote": 67, "dur": 1.0, "sustain": 0.4}},
            {"at": 1.5, "data": {"midinote": 72, "dur": 1.0, "sustain": 0.4}},
        ]));
        let got = read_plain(&seq, json!({}));
        assert_eq!(got.sheet.staves[0].voices.len(), 1);
        assert_eq!(
            line(&got.sheet, 0, 0),
            vec![
                (vec![60], (1, 8)),
                (vec![64], (1, 8)),
                (vec![67], (1, 8)),
                (vec![72], (1, 4)),
            ]
        );
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
                to: third,
                join: None
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
            join: None,
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

    #[test]
    fn a_staff_is_named_and_transposed_by_its_part() {
        let mut seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 72, "dur": 1.0, "channel": 0}},
            {"at": 0.0, "data": {"midinote": 48, "dur": 1.0, "channel": 1}},
            {"at": 0.0, "data": {"midinote": 62, "dur": 1.0, "channel": 2}},
        ]));
        seq.parts = vec![
            crate::events::Part {
                name: "Piano".into(),
                abbr: "Pno.".into(),
                staves: 2,
                ..crate::events::Part::default()
            },
            crate::events::Part {
                name: "Clarinet in B flat".into(),
                transpose: -2,
                ..crate::events::Part::default()
            },
        ];
        let sheet = read_plain(&seq, json!({})).sheet;
        let named: Vec<(&str, &str, i32)> = sheet
            .staves
            .iter()
            .map(|s| (s.label.as_str(), s.abbr.as_str(), s.transpose))
            .collect();
        assert_eq!(
            named,
            vec![
                ("Piano", "Pno.", 0),
                ("", "", 0),
                ("Clarinet in B flat", "", -2)
            ]
        );
    }

    fn part(value: Value) -> crate::events::Part {
        serde_json::from_value(value).expect("a part")
    }

    /// The notes of a sequence as `(at, midinote, dur, sustain)`, in order.
    fn heard(seq: &EventSequence) -> Vec<(f64, i64, f64, f64)> {
        let mut out: Vec<(f64, i64, f64, f64)> = seq
            .events
            .iter()
            .map(|event| {
                let keys = event.keys();
                let number = |key: &str| keys.get(key).and_then(Value::as_f64).unwrap_or(0.0);
                (
                    event.at.0,
                    number("midinote") as i64,
                    number("dur"),
                    number("sustain"),
                )
            })
            .collect();
        out.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out
    }

    #[test]
    fn a_part_reads_its_channels_onto_its_staves() {
        let mut seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 72, "dur": 1.0, "channel": 3}},
            {"at": 0.0, "data": {"midinote": 48, "dur": 1.0, "channel": 3}},
            {"at": 0.0, "data": {"midinote": 79, "dur": 1.0, "channel": 0}},
            {"at": 0.0, "data": {"midinote": 60, "dur": 1.0, "channel": 5}},
        ]));
        seq.parts = vec![
            part(json!({"name": "Flute", "channels": [0]})),
            part(json!({"name": "Piano", "staves": 2, "channels": [3]})),
        ];
        let sheet = read_plain(&seq, json!({})).sheet;
        let staves: Vec<Vec<i32>> = sheet
            .staves
            .iter()
            .map(|staff| {
                staff.voices[0]
                    .items
                    .iter()
                    .flat_map(|item| item.pitches().iter().map(Pitch::midi))
                    .collect()
            })
            .collect();
        // the flute's staff, the piano's two split at middle C, and a channel
        // no part plays on a staff of its own
        assert_eq!(staves, vec![vec![79], vec![72], vec![48], vec![60]]);
        assert_eq!(sheet.staves[1].label, "Piano");
    }

    #[test]
    fn a_part_writes_as_many_voices_a_staff_as_it_says() {
        let mut seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 60, "dur": 4.0}},
            {"at": 0.0, "data": {"midinote": 72, "dur": 1.0}},
            {"at": 1.0, "data": {"midinote": 74, "dur": 1.0}},
        ]));
        assert_eq!(read_plain(&seq, json!({})).sheet.staves[0].voices.len(), 2);
        seq.parts = vec![part(
            json!({"writing": {"polyphony": "voices", "voices_per_staff": 1}}),
        )];
        assert_eq!(read_plain(&seq, json!({})).sheet.staves[0].voices.len(), 1);
    }

    /// **A part written with ties is one voice of chords**: each note cut
    /// where another starts or ends and tied across, a pitch repeated under
    /// a held one attacked again -- and it sounds as the same part written in
    /// voices, reads back as itself, and changes nothing of the events.
    #[test]
    fn a_part_written_with_ties_is_one_voice_of_chords() {
        // C held a whole note, E twice under it, then G
        let mut seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 60, "dur": 4.0}},
            {"at": 0.0, "data": {"midinote": 64, "dur": 1.0}},
            {"at": 1.0, "data": {"midinote": 64, "dur": 1.0}},
            {"at": 2.0, "data": {"midinote": 67, "dur": 2.0}},
        ]));
        let events = seq.events.clone();
        let voices = read_plain(&seq, json!({}));
        seq.edit(crate::events::EventsIntent::SetParts {
            parts: vec![part(
                json!({"writing": {"polyphony": "ties", "voices_per_staff": 1}}),
            )],
        })
        .unwrap();
        assert_eq!(seq.events, events, "the writing changes no event");
        let ties = read_plain(&seq, json!({}));
        assert_eq!(ties.sheet.staves[0].voices.len(), 1);
        assert_eq!(
            line(&ties.sheet, 0, 0),
            vec![
                (vec![60, 64], (1, 4)),
                (vec![60, 64], (1, 4)),
                (vec![60, 67], (1, 2)),
            ]
        );
        let tie_of = |item: &Item| match item {
            Item::Note { tie, marks, .. } => (*tie, marks.tied.iter().map(Pitch::midi).collect()),
            Item::Rest { .. } => (false, Vec::new()),
        };
        let items = &ties.sheet.staves[0].voices[0].items;
        let said: Vec<(bool, Vec<i32>)> = items.iter().map(tie_of).collect();
        assert_eq!(
            said,
            vec![(true, vec![60]), (true, Vec::new()), (false, Vec::new())],
            "the repeated E is attacked again"
        );
        assert!(sheet_to_mei(&ties.sheet).is_ok());

        // the two writings sound the same
        let interp = Interpretation::default();
        let as_voices = render(&voices.sheet, &interp).unwrap();
        let as_ties = render(&ties.sheet, &interp).unwrap();
        assert_eq!(heard(&as_voices), heard(&as_ties));

        // what the page renders reads back as the page
        let mut again = as_ties.clone();
        again.parts.clone_from(&seq.parts);
        let back = read_plain(&again, json!({}));
        assert_eq!(
            sheet_to_mei(&back.sheet).unwrap(),
            sheet_to_mei(&ties.sheet).unwrap()
        );

        // and back to voices: the same events, read as before
        seq.edit(crate::events::EventsIntent::SetParts { parts: Vec::new() })
            .unwrap();
        assert_eq!(seq.events, events);
        assert_eq!(read_plain(&seq, json!({})).sheet, voices.sheet);
    }

    /// **What a page renders reads back as that page**, for any sequence
    /// written as notation: a sequence read into a page, rendered and read
    /// again gives the same page, written in voices or with ties. The
    /// sequences are made by a seeded generator, so a failure names its seed.
    #[test]
    fn a_page_rendered_reads_back_as_itself() {
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        for seed in 0..200 {
            let count = 2 + next(10);
            let events: Vec<Value> = (0..count)
                .map(|_| {
                    json!({
                        "at": next(32) as f64 / 4.0,
                        "data": {
                            "midinote": 48 + next(36),
                            "dur": (1 + next(8)) as f64 / 4.0,
                            "channel": next(2),
                        }
                    })
                })
                .collect();
            for polyphony in ["voices", "ties"] {
                let mut seq = sequence(Value::Array(events.clone()));
                seq.parts = vec![part(json!({
                    "staves": 2,
                    "writing": {"polyphony": polyphony, "voices_per_staff": 2},
                }))];
                let page = read_plain(&seq, json!({})).sheet;
                let mut rendered = render(&page, &Interpretation::default())
                    .unwrap_or_else(|why| panic!("seed {seed}, {polyphony}: {why}"));
                rendered.parts.clone_from(&seq.parts);
                let back = read_plain(&rendered, json!({})).sheet;
                assert_eq!(
                    sheet_to_mei(&back).unwrap(),
                    sheet_to_mei(&page).unwrap(),
                    "seed {seed}, {polyphony}: {events:?}"
                );
            }
        }
    }

    #[test]
    fn a_sequence_is_read_as_its_stored_reading_says() {
        let mut seq = sequence(json!([
            {"at": 0.0, "data": {"midinote": 60, "dur": 1.0}},
            {"at": 3.0, "data": {"midinote": 62, "dur": 1.0}},
        ]));
        assert_eq!(Transcription::of(&seq), Transcription::default());
        seq.edit(crate::events::EventsIntent::SetReading {
            reading: Some(json!({"meter": "3/4"})),
        })
        .unwrap();
        let how = Transcription::of(&seq);
        assert_eq!(how.meter.as_deref(), Some("3/4"));
        assert_eq!(how.division, 16, "what it leaves out keeps its default");
        let sheet = read(&seq, &how, &Interpretation::default()).unwrap().sheet;
        assert_eq!(sheet.grid, Grid::uniform(3, 4));
        // an opener's options go over it, one by one
        let given = Transcription::over(&seq, &json!({"division": 8})).unwrap();
        assert_eq!((given.meter.as_deref(), given.division), (Some("3/4"), 8));
        assert_eq!(Transcription::over(&seq, &json!({})).unwrap(), how);
        assert_eq!(Transcription::over(&seq, &Value::Null).unwrap(), how);
        assert!(Transcription::over(&seq, &json!([])).is_err());
        // and a reading the options refuse is not stored
        assert!(
            seq.edit(crate::events::EventsIntent::SetReading {
                reading: Some(json!({"division": "sixteen"})),
            })
            .unwrap_err()
            .contains("reading")
        );
    }
}
