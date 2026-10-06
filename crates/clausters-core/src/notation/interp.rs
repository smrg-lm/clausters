//! The interpreter: what a score **sounds like**, as against what it says.
//!
//! Reading a page into events is not a conversion, and that is the whole reason
//! this is its own layer. A quarter note marked staccato is a quarter note --
//! the next attack comes where it always did -- and it *sounds* half that. A
//! dynamic written on one note governs every note after it until another one is
//! written. A crescendo is not on any note at all: it is a shape over a stretch
//! of them. None of that can be read off an item in isolation, so the
//! interpreter walks the sheet with the context the symbols need.
//!
//! **Two lengths, always.** A [`Note`] carries `dur` (what is written) and
//! `sustain` (what is heard), and they are different numbers for exactly the
//! reason a page keeps them apart: shortening the written value would move
//! every attack after it and make the score a different one. A client maps
//! the pair straight onto its event's `dur` and `sustain`.
//!
//! **The default interpretation is data, and replaceable.** Everything the
//! reading depends on -- how much a staccato shortens, what `mf` is in
//! amplitude, how far a crescendo travels, which metric positions are stressed
//! -- lives in [`Interpretation`], crosses as JSON, and is passed in. A caller
//! who disagrees edits the value and sends it; nobody edits this file to play a
//! score in another style. What the *defaults* claim is deliberately as little
//! as a player can claim and still be playing: the marks mean roughly what a
//! dictionary says, and the only metric stress is the downbeat, which is the
//! one accent common to the styles that have any. "One and three in a 4/4"
//! belongs to a style, and a style says so by passing its own accents.
//!
//! **A page is played in another order than it is read**: its repeats, its
//! endings and its jumps are played out ([`unroll`]), and every item is heard
//! once a pass. What it says about the time and the pitch is heard too -- a
//! tempo mark is the tempo, an octave line and a transposing staff move the
//! pitch, a tremolo is its repeated notes, a rolled chord is staggered, a
//! glissando is a slide, the pedal is held ([`Performance`]). A *tuplet* needs
//! no rule -- its division is already exact in the rational the item holds, so
//! onsets land on it without the interpreter knowing tuplets exist.
//!
//! **The instrument is not in the notation.** A staff does not say what plays
//! it, so every note names the `staff` and `voice` it was written on and the
//! binding is made where the score is rendered, explicitly -- the same rule
//! `docs/decisions.md` states for a buffer sounding through an instrument.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::model::{Item, Marks, Pitch, Sheet};
use crate::ratio::Ratio;

mod levels;
mod unroll;

pub use levels::{LevelPoint, StaffLevel, levels};
pub use unroll::{Played, unroll};

/// **Where a moment of the score is heard**: `beat`, in beats as the page
/// counts them from its start, at each pass the performance plays it -- once
/// for a score with no repeats, twice inside a repeated stretch.
pub fn heard_beats(sheet: &Sheet, beat_unit: i64, beat: f64) -> Vec<f64> {
    let beats = beat_unit as f64;
    let grid = &sheet.grid;
    let count = measures_of(sheet);
    // the measure the moment falls in, counting on past the music
    let mut measure = 0;
    let mut start = 0.0;
    loop {
        let len = grid.bar_len(measure).to_f64() * beats;
        if beat < start + len || len <= 0.0 || measure > count + 1024 {
            break;
        }
        start += len;
        measure += 1;
    }
    let into = beat - start;
    let passes: Vec<f64> = unroll(grid, count)
        .into_iter()
        .filter(|p| p.measure == measure)
        .map(|p| p.at.to_f64() * beats + into)
        .collect();
    if passes.is_empty() {
        vec![beat]
    } else {
        passes
    }
}

/// How long the score is, in measures, as [`unroll`] counts them.
pub fn measure_count(sheet: &Sheet) -> usize {
    measures_of(sheet)
}

/// One sounding note, as the interpreter heard it.
///
/// Times are in **beats**, where a beat is [`Interpretation::beat_unit`] -- the
/// unit a client's own sequencing runs in, so the result drops onto a timeline
/// without a second conversion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Onset, in beats from the start of the score.
    pub t: f64,
    /// The **written** value, in beats: where the music counts, and the
    /// distance to what comes next.
    pub dur: f64,
    /// How long it is **held**, in beats. Shorter than `dur` under a staccato,
    /// equal to it under a plain note.
    pub sustain: f64,
    /// MIDI note number.
    pub pitch: i32,
    /// Linear amplitude: the prevailing dynamic, ramped by any hairpin over it,
    /// stressed by its metric position and by its own accents.
    pub amp: f64,
    /// The staff it is written on, 0-based from the top -- what a caller binds
    /// an instrument to.
    pub staff: usize,
    /// The voice of that staff, 0-based.
    pub voice: usize,
    /// The model id of the item it came from, so a sounding note can be traced
    /// back to the note on the page.
    pub id: u64,
    /// Which enharmonic spelling the writer chose -- `"sharp"` or `"flat"` --
    /// where the note is altered at all; `None` on a natural, where nothing
    /// was chosen.
    ///
    /// A written fact on a sounding note, and it is here for one reason: a
    /// client that plays a score and then writes it back has only `pitch`, and
    /// a number cannot tell A flat from G sharp. Double alterations spell as
    /// their direction, which is as much as the two words can say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spelling: Option<String>,
    /// `"written"` where the accidental is one the writer wants printed even
    /// though the key already implies it -- a courtesy sign. `None` otherwise:
    /// the engraver decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accidental: Option<String>,
    /// What is written on the note beyond its pitch and value, **verbatim**.
    ///
    /// Not what the interpreter made of it: a staccato is already honoured in
    /// `sustain`, and it is still here, because the page said it and a client
    /// writing the note back writes the staccato rather than a shortened
    /// length. Empty on a note that carries nothing, which is the case a v1
    /// payload produces and the one that stays byte-identical.
    #[serde(default, skip_serializing_if = "Marks::is_empty")]
    pub marks: Marks,
    /// **A glissando**: how many semitones the pitch slides by while the note
    /// is held, to the note the slide is written to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glide: Option<f64>,
}

/// **What a score sounds like, whole**: its notes, its tempo, its pedal, and
/// when each of its items is heard -- in beats, each pass of a repeat a time
/// of its own.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Performance {
    /// The notes, in time order.
    pub notes: Vec<Note>,
    /// The tempo, as `(beat, quarter notes a minute)` from the first beat on.
    pub tempo: Vec<(f64, f64)>,
    /// The sustain pedal: `(staff, beat pressed, beat let go)`.
    pub pedals: Vec<(usize, f64, f64)>,
    /// When every item is heard, rests too: `(item id, beat)`, once a pass.
    pub heard: Vec<(u64, f64)>,
}

impl Performance {
    /// **The tempo as a map of beats to seconds**, beats being `beat_unit`ths
    /// of a whole note as the notes are.
    pub fn tempo_map(&self, beat_unit: i64) -> crate::tempomap::TempoMap {
        let per_second = |qpm: f64| qpm / 60.0 * beat_unit as f64 / 4.0;
        let mut changes = self.tempo.iter();
        let first = changes.next().map_or(120.0, |(_, qpm)| *qpm);
        let mut map = crate::tempomap::TempoMap::new(per_second(first));
        for (beat, qpm) in changes {
            // a change that restates the beat it is at is a later one's
            let _ = map.push(*beat, per_second(*qpm));
        }
        map
    }
}

/// What one articulation does to a note.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Articulation {
    /// The fraction of the written value the note is held for.
    #[serde(default = "one")]
    pub factor: f64,
    /// What it does to the amplitude.
    #[serde(default = "one")]
    pub gain: f64,
}

/// A metric stress: a position within the bar, and what being on it does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Accent {
    /// How far into the measure, in whole notes -- `0` is the downbeat, `1/2`
    /// is the third beat of a 4/4.
    pub at: Ratio,
    /// What a note starting exactly there does to its amplitude.
    pub gain: f64,
    /// The meter this applies in, `"count/unit"`. Left out, it applies in every
    /// meter -- which is right for the downbeat and wrong for anything else,
    /// since half a bar is a different place in a 4/4 and in a 3/4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter: Option<String>,
}

fn one() -> f64 {
    1.0
}

/// How the symbols are read: the whole of what the interpreter believes.
///
/// Every field has a default, so a caller overriding one sends the rest
/// unchanged and a caller overriding none sends `{}`. It is also the **parity
/// surface** for the reading: the fields cross as data through one symbol, so a
/// binding table cannot see them, and each client is contrasted against
/// [`default_interpretation`] the way the operations are contrasted against the
/// catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interpretation {
    /// Which written value is one beat, as its denominator: `4` for a quarter.
    #[serde(default = "default_beat_unit")]
    pub beat_unit: i64,
    /// The amplitude of a note under no dynamic at all.
    #[serde(default = "default_amp")]
    pub amp: f64,
    /// The fraction of its written value a plain note is held for -- a note
    /// with no articulation and under no slur. `1.0` leaves the reading
    /// neutral; a player who detaches by habit lowers it.
    #[serde(default = "one")]
    pub detach: f64,
    /// The same, for a note under a slur.
    #[serde(default = "one")]
    pub slur: f64,
    /// What a crescendo reaches by its far end, as a factor on the amplitude it
    /// started from -- used only when no dynamic is written at that end, since a
    /// written one says where it was going.
    #[serde(default = "default_crescendo")]
    pub crescendo: f64,
    /// The same for a diminuendo.
    #[serde(default = "default_diminuendo")]
    pub diminuendo: f64,
    /// What each dynamic is worth in linear amplitude.
    #[serde(default = "default_dynamics")]
    pub dynamics: BTreeMap<String, f64>,
    /// What each articulation does, by its MEI name.
    #[serde(default = "default_articulations")]
    pub articulations: BTreeMap<String, Articulation>,
    /// Which positions in the bar are stressed.
    #[serde(default = "default_accents")]
    pub accents: Vec<Accent>,
    /// **Where a dynamic and a hairpin are heard**: on each note's attack, as
    /// a curve of its voice's channel, or both.
    #[serde(default)]
    pub dynamics_as: DynamicsAs,
    /// Which controller the curve of a dynamic moves, where a renderer writes
    /// one: 11, expression, unless the reading says another.
    #[serde(default = "default_dynamics_cc")]
    pub dynamics_cc: u8,
    /// The tempo a score that states none is played at, in quarter notes a
    /// minute.
    #[serde(default = "default_tempo")]
    pub tempo: f64,
    /// How much longer than its value a note let ring is held.
    #[serde(default = "default_ring")]
    pub ring: f64,
    /// The fraction of its value a note with a breath after it is held.
    #[serde(default = "default_breath")]
    pub breath: f64,
    /// ...and with a caesura.
    #[serde(default = "default_caesura")]
    pub caesura: f64,
    /// How far apart the notes of a rolled chord start, in whole notes.
    #[serde(default = "default_roll")]
    pub roll: Ratio,
}

fn default_tempo() -> f64 {
    120.0
}

fn default_ring() -> f64 {
    2.0
}

fn default_breath() -> f64 {
    0.75
}

fn default_caesura() -> f64 {
    0.5
}

fn default_roll() -> Ratio {
    Ratio::new(1, 64)
}

/// Where a dynamic and a hairpin are heard.
///
/// A level is one fact and an instrument hears it in one of two places: a
/// struck or plucked sound in the attack of each note, a sustained one in a
/// control that moves while the note is held. The reading says which, and
/// the default says both -- the attack so every instrument answers a dynamic,
/// the curve so a crescendo over a held note moves inside it -- which is what
/// lets one render serve both kinds, each listening where it listens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DynamicsAs {
    /// On each note's attack alone: no curve is written.
    Attack,
    /// As a curve alone: a note's attack is the unmarked level, stressed by
    /// its place in the bar and its own accents.
    Curve,
    /// Both.
    #[default]
    Both,
}

fn default_dynamics_cc() -> u8 {
    11
}

fn default_beat_unit() -> i64 {
    4
}

fn default_amp() -> f64 {
    0.12
}

fn default_crescendo() -> f64 {
    1.6
}

fn default_diminuendo() -> f64 {
    0.625
}

/// The dynamics, a factor of about 1.45 apart, centred so that an unmarked
/// score and an `mf` one sound the same -- an unmarked page is not silent and
/// not loud, and saying which of the named levels it is keeps the two readings
/// from drifting apart.
fn default_dynamics() -> BTreeMap<String, f64> {
    [
        ("ppp", 0.02),
        ("pp", 0.035),
        ("p", 0.05),
        ("mp", 0.08),
        ("mf", 0.12),
        ("f", 0.17),
        ("ff", 0.25),
        ("fff", 0.36),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// The articulations, by their MEI names. A tenuto is held whole and unstressed
/// -- which is not nothing, since it overrides the player's own detachment.
fn default_articulations() -> BTreeMap<String, Articulation> {
    [
        (
            "stacc",
            Articulation {
                factor: 0.5,
                gain: 1.0,
            },
        ),
        (
            "stacciss",
            Articulation {
                factor: 0.25,
                gain: 1.0,
            },
        ),
        (
            "ten",
            Articulation {
                factor: 1.0,
                gain: 1.0,
            },
        ),
        (
            "acc",
            Articulation {
                factor: 1.0,
                gain: 1.3,
            },
        ),
        (
            "marc",
            Articulation {
                factor: 0.75,
                gain: 1.4,
            },
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// The downbeat, and nothing else. Every metric style stresses it; which of the
/// remaining positions it also stresses is what makes it a style, and a style
/// passes its own list.
fn default_accents() -> Vec<Accent> {
    vec![Accent {
        at: Ratio::ZERO,
        gain: 1.2,
        meter: None,
    }]
}

impl Default for Interpretation {
    fn default() -> Interpretation {
        Interpretation {
            beat_unit: default_beat_unit(),
            amp: default_amp(),
            detach: one(),
            slur: one(),
            crescendo: default_crescendo(),
            diminuendo: default_diminuendo(),
            dynamics: default_dynamics(),
            articulations: default_articulations(),
            accents: default_accents(),
            dynamics_as: DynamicsAs::default(),
            dynamics_cc: default_dynamics_cc(),
            tempo: default_tempo(),
            ring: default_ring(),
            breath: default_breath(),
            caesura: default_caesura(),
            roll: default_roll(),
        }
    }
}

/// The reading a caller gets when it says nothing -- and the value it starts
/// from when it wants to say something.
///
/// A client cannot write these numbers down for itself: two clients with their
/// own copies of the table play the same score at two amplitudes, and nothing
/// compares them.
pub fn default_interpretation() -> Interpretation {
    Interpretation::default()
}

/// Where an item sits: its onset, and which staff and voice it belongs to.
struct Placed<'a> {
    t: Ratio,
    item: &'a Item,
    staff: usize,
    voice: usize,
    /// Index in its voice, so a tie chain and a slur's reach can be walked.
    index: usize,
}

/// Every item of `sheet`, with where it sits. One walk, so the passes over it
/// can index by id instead of searching the staves again.
fn placed_of(sheet: &Sheet) -> Vec<Placed<'_>> {
    let mut placed: Vec<Placed> = Vec::new();
    for (si, staff) in sheet.staves.iter().enumerate() {
        for (vi, voice) in staff.voices.iter().enumerate() {
            let mut t = Ratio::ZERO;
            for (ii, item) in voice.items.iter().enumerate() {
                placed.push(Placed {
                    t,
                    item,
                    staff: si,
                    voice: vi,
                    index: ii,
                });
                t = t + item.dur();
            }
        }
    }
    placed
}

/// Read `sheet` under `interp` into the notes it sounds, in time order --
/// [`performance`]'s notes.
///
/// # Errors
/// When a spanner names an item that is not on the sheet -- the same refusal the
/// emitter makes, and for the same reason: a crescendo that governs nothing is
/// a fact the caller wants back, not one to swallow.
pub fn perform(sheet: Sheet, interp: &Interpretation) -> Result<Vec<Note>, String> {
    Ok(performance(sheet, interp)?.notes)
}

/// How many measures the sheet's music fills.
fn measures_of(sheet: &Sheet) -> usize {
    let (m, into) = sheet.grid.position(sheet.len());
    (if into.is_positive() { m + 1 } else { m }).max(1)
}

/// **Read `sheet` under `interp` into what it sounds like** (see
/// [`Performance`]): the repeats, endings and jumps played out, the tempo
/// marks as the tempo, the pedal, an octave line and a transposing staff on
/// the pitch, a tremolo as its repeated notes, a rolled chord staggered, a
/// glissando as a slide.
///
/// # Errors
/// As [`perform`].
pub fn performance(mut sheet: Sheet, interp: &Interpretation) -> Result<Performance, String> {
    sheet.assign_ids();

    let placed = placed_of(&sheet);
    let at: HashMap<u64, usize> = placed
        .iter()
        .enumerate()
        .map(|(i, p)| (p.item.id(), i))
        .collect();

    let dynamics = dynamic_map(&placed, interp);
    let (slurred, hairpins) = spanners(&sheet, &placed, &at, &dynamics, interp)?;

    // **Where each measure is heard**: once a pass, in the order the page
    // is played.
    let mut passes: HashMap<usize, Vec<Ratio>> = HashMap::new();
    for played in unroll(&sheet.grid, measures_of(&sheet)) {
        passes.entry(played.measure).or_default().push(played.at);
    }
    let heard_at = |t: Ratio| -> Vec<Ratio> {
        let (m, into) = sheet.grid.position(t);
        passes
            .get(&m)
            .map(|starts| starts.iter().map(|s| *s + into).collect())
            .unwrap_or_default()
    };
    let beats = interp.beat_unit as f64;
    let beat = |t: Ratio| t.to_f64() * beats;

    // what the lines between two notes do to what sounds
    let onset_of = |id: u64| at.get(&id).map(|&i| placed[i].t);
    let end_of = |id: u64| at.get(&id).map(|&i| placed[i].t + placed[i].item.dur());
    let mut octaves: Vec<(usize, Ratio, Ratio, i32)> = Vec::new();
    let mut glides: HashMap<u64, f64> = HashMap::new();
    let mut alternating: HashMap<u64, u64> = HashMap::new();
    let mut pedals = Vec::new();
    for spanner in &sheet.spanners {
        let (Some(from), Some(end)) = (onset_of(spanner.from), end_of(spanner.to)) else {
            continue;
        };
        let staff = at.get(&spanner.from).map_or(0, |&i| placed[i].staff);
        let first_pitch = |id: u64| {
            at.get(&id)
                .and_then(|&i| placed[i].item.pitches().first().map(|p| p.midi()))
        };
        match spanner.kind.as_str() {
            "8va" => octaves.push((staff, from, end, 12)),
            "8vb" => octaves.push((staff, from, end, -12)),
            "15ma" => octaves.push((staff, from, end, 24)),
            "15mb" => octaves.push((staff, from, end, -24)),
            "gliss" => {
                if let (Some(a), Some(b)) = (first_pitch(spanner.from), first_pitch(spanner.to)) {
                    glides.insert(spanner.from, f64::from(b - a));
                }
            }
            "ftrem" => {
                alternating.insert(spanner.from, spanner.to);
            }
            "pedal" => {
                let downs = heard_at(from);
                let ups = heard_at(end);
                for (down, up) in downs.into_iter().zip(ups) {
                    if up > down {
                        pedals.push((staff, beat(down), beat(up)));
                    }
                }
            }
            _ => {}
        }
    }
    let shifted = |staff: usize, t: Ratio| -> i32 {
        let octave: i32 = octaves
            .iter()
            .filter(|(s, a, b, _)| *s == staff && t >= *a && t < *b)
            .map(|(.., by)| *by)
            .sum();
        octave + sheet.staves.get(staff).map_or(0, |s| s.transpose)
    };

    let mut notes: Vec<Note> = Vec::new();
    let mut heard: Vec<(u64, f64)> = Vec::new();
    let mut merged: HashSet<usize> = HashSet::new();
    for (i, p) in placed.iter().enumerate() {
        for t in heard_at(p.t) {
            heard.push((p.item.id(), beat(t)));
        }
        if !p.item.sounds() || merged.contains(&i) {
            continue;
        }
        // A tie is one sound of the summed length: the chain's items are
        // consumed here so they do not attack again on their own.
        let mut written = p.item.dur();
        let mut last = i;
        while let Item::Note { tie: true, .. } = placed[last].item {
            let Some(next) = placed.get(last + 1) else {
                break;
            };
            if next.staff != p.staff || next.voice != p.voice || !next.item.sounds() {
                break;
            }
            written = written + next.item.dur();
            last += 1;
            merged.insert(last);
        }

        let marks = p.item.marks();
        let written_beats = written.to_f64() * beats;
        let mut sustain = match marks.and_then(|m| m.sounding) {
            // What the writer stated outright; no table is consulted.
            Some(sounding) => sounding.to_f64() * beats,
            None => {
                // The shortest articulation wins: a note that is both staccato
                // and something else is as short as the shortest of them says.
                let factor = marks
                    .map(|m| {
                        m.articulations
                            .iter()
                            .filter_map(|a| interp.articulations.get(a))
                            .map(|a| a.factor)
                            .fold(f64::INFINITY, f64::min)
                    })
                    .filter(|f| f.is_finite())
                    .unwrap_or(if slurred.contains(&p.item.id()) {
                        interp.slur
                    } else {
                        interp.detach
                    });
                written_beats * factor
            }
        };
        if let Some(marks) = marks {
            match marks.breath.as_deref() {
                Some("caesura") => sustain *= interp.caesura,
                Some(_) => sustain *= interp.breath,
                None => {}
            }
            if marks.ring {
                sustain = sustain.max(written_beats * interp.ring);
            }
        }

        // The level the staff is at, unless the reading hears a dynamic in
        // its curve alone: the attack is then the unmarked level.
        let mut amp = interp.amp;
        if interp.dynamics_as != DynamicsAs::Curve {
            amp = prevailing(&dynamics, p.staff, p.t).unwrap_or(interp.amp);
            for hairpin in &hairpins {
                amp *= hairpin.gain_at(p.t, p.staff);
            }
        }
        amp *= metric_gain(&sheet, p.t, interp);
        if let Some(marks) = marks {
            for a in &marks.articulations {
                if let Some(spec) = interp.articulations.get(a) {
                    amp *= spec.gain;
                }
            }
        }

        let by = shifted(p.staff, p.t);
        let note = |t: f64, dur: f64, sustain: f64, pitch: &Pitch| Note {
            t,
            dur,
            sustain,
            pitch: pitch.midi() + by,
            amp,
            staff: p.staff,
            voice: p.voice,
            id: p.item.id(),
            spelling: match pitch.alter {
                0 => None,
                n if n > 0 => Some("sharp".into()),
                _ => Some("flat".into()),
            },
            accidental: pitch.forced.then(|| "written".to_string()),
            marks: marks.cloned().unwrap_or_default(),
            glide: glides.get(&p.item.id()).copied(),
        };
        let pitches = p.item.pitches();
        // **Two notes alternating**, sixteenths of each in turn for the time
        // the two take together; the second note is heard only in it.
        let partner = alternating
            .get(&p.item.id())
            .and_then(|id| at.get(id))
            .map(|&j| placed[j].item);
        if let Some(other) = partner {
            merged.insert(at[&other.id()]);
        }
        for start in heard_at(p.t) {
            let t0 = beat(start);
            if let Some(other) = partner {
                let unit = Ratio::new(1, 16).to_f64() * beats;
                let total = written_beats + other.dur().to_f64() * beats;
                let count = (total / unit).round().max(2.0) as usize;
                for k in 0..count {
                    let these = if k % 2 == 0 { pitches } else { other.pitches() };
                    for pitch in these {
                        notes.push(note(t0 + k as f64 * unit, unit, unit, pitch));
                    }
                }
                continue;
            }
            // **A tremolo** is its repeated notes, the value each stroke
            // halves, for the note's whole length.
            if let Some(strokes) = marks.and_then(|m| m.tremolo) {
                let unit =
                    (Ratio::new(1, 4) / Ratio::from(1i64 << strokes.clamp(1, 3))).to_f64() * beats;
                let count = (written_beats / unit).floor().max(1.0) as usize;
                for k in 0..count {
                    for pitch in pitches {
                        notes.push(note(t0 + k as f64 * unit, unit, unit, pitch));
                    }
                }
                continue;
            }
            // **A rolled chord** starts each note a little after the one
            // below it -- above it, rolled down -- and lets go with the rest.
            let mut order: Vec<&Pitch> = pitches.iter().collect();
            let rolled = marks.and_then(|m| m.arpeggio.as_deref());
            if rolled == Some("down") {
                order.sort_by_key(|p| std::cmp::Reverse(p.midi()));
            } else {
                order.sort_by_key(|p| p.midi());
            }
            let step = if rolled.is_some() && order.len() > 1 {
                (interp.roll.to_f64() * beats).min(written_beats / (2.0 * order.len() as f64))
            } else {
                0.0
            };
            for (k, pitch) in order.into_iter().enumerate() {
                let late = step * k as f64;
                notes.push(note(
                    t0 + late,
                    written_beats - late,
                    (sustain - late).max(step),
                    pitch,
                ));
            }
        }
    }

    notes.sort_by(|a, b| {
        a.t.total_cmp(&b.t)
            .then(a.staff.cmp(&b.staff))
            .then(a.voice.cmp(&b.voice))
            .then(a.pitch.cmp(&b.pitch))
    });
    heard.sort_by(|a, b| a.1.total_cmp(&b.1));

    // **The tempo marks are the tempo**, wherever each is heard.
    let mut tempo: Vec<(f64, f64)> = sheet
        .controls
        .iter()
        .filter(|c| c.kind == "tempo")
        .filter_map(|c| Some((onset_of(c.on)?, c.bpm?)))
        .flat_map(|(t, bpm)| heard_at(t).into_iter().map(move |at| (beat(at), bpm)))
        .collect();
    tempo.sort_by(|a, b| a.0.total_cmp(&b.0));
    tempo.dedup_by(|a, b| a.0 == b.0);
    if tempo.first().is_none_or(|(b, _)| *b > 0.0) {
        tempo.insert(0, (0.0, interp.tempo));
    }
    Ok(Performance {
        notes,
        tempo,
        pedals,
        heard,
    })
}

/// Every dynamic written on the sheet, per staff, as `(onset, amplitude)` in
/// time order -- a dynamic governs the notes **after** it, so this is read
/// rather than applied.
///
/// Per staff because that is where the mark is written: a dynamic under the
/// left hand does not govern the right one.
fn dynamic_map(placed: &[Placed], interp: &Interpretation) -> Vec<Vec<(Ratio, f64)>> {
    let staves = placed.iter().map(|p| p.staff + 1).max().unwrap_or(0);
    let mut out = vec![Vec::new(); staves];
    for p in placed {
        if let Some(name) = p.item.marks().and_then(|m| m.dynamic.as_ref())
            && let Some(&amp) = interp.dynamics.get(name)
        {
            out[p.staff].push((p.t, amp));
        }
    }
    for staff in &mut out {
        staff.sort_by_key(|&(t, _)| t);
    }
    out
}

/// The dynamic in force at `t` on `staff`, or `None` where none has been
/// written yet.
fn prevailing(dynamics: &[Vec<(Ratio, f64)>], staff: usize, t: Ratio) -> Option<f64> {
    dynamics
        .get(staff)?
        .iter()
        .take_while(|&&(onset, _)| onset <= t)
        .last()
        .map(|&(_, amp)| amp)
}

/// A crescendo or diminuendo, resolved to the stretch of time it shapes.
struct Hairpin {
    staff: usize,
    start: Ratio,
    end: Ratio,
    /// What the amplitude is multiplied by at the far end; on the way there it
    /// is interpolated from `1.0`.
    target: f64,
    /// Whether the note the hairpin ends on is shaped by it. It is not when a
    /// dynamic is written there: that note is *at* the destination already, and
    /// ramping it as well would apply the arrival twice.
    shapes_end: bool,
}

impl Hairpin {
    /// The factor at `t` -- `1.0` outside the span, so a note anywhere else is
    /// untouched by it.
    fn gain_at(&self, t: Ratio, staff: usize) -> f64 {
        let past_end = if self.shapes_end {
            t > self.end
        } else {
            t >= self.end
        };
        if staff != self.staff || t < self.start || past_end {
            return 1.0;
        }
        let span = (self.end - self.start).to_f64();
        if span <= 0.0 {
            return self.target;
        }
        let over = (t - self.start).to_f64() / span;
        1.0 + (self.target - 1.0) * over
    }
}

/// Resolve the sheet's spanners: which items are under a slur, and what shape
/// each hairpin has.
fn spanners(
    sheet: &Sheet,
    placed: &[Placed],
    at: &HashMap<u64, usize>,
    dynamics: &[Vec<(Ratio, f64)>],
    interp: &Interpretation,
) -> Result<(HashSet<u64>, Vec<Hairpin>), String> {
    let mut slurred = HashSet::new();
    let mut hairpins = Vec::new();
    for spanner in &sheet.spanners {
        let ends = at.get(&spanner.from).zip(at.get(&spanner.to));
        let Some((&from, &to)) = ends else {
            let missing = if at.contains_key(&spanner.from) {
                spanner.to
            } else {
                spanner.from
            };
            return Err(format!(
                "the {} is written to an item ({}) that is not on this sheet",
                spanner.kind, missing
            ));
        };
        let (a, b) = placed.get(from).zip(placed.get(to)).unwrap();
        match spanner.kind.as_str() {
            "slur" => {
                // Everything from one end to the other in that voice, which is
                // what a slur reaches over -- not only the two notes named.
                for p in placed {
                    if p.staff == a.staff
                        && p.voice == a.voice
                        && p.index >= a.index.min(b.index)
                        && p.index <= a.index.max(b.index)
                    {
                        slurred.insert(p.item.id());
                    }
                }
            }
            "crescendo" | "diminuendo" => {
                let (start, end) = (a.t.min(b.t), a.t.max(b.t));
                let from_amp = prevailing(dynamics, a.staff, start).unwrap_or(interp.amp);
                // A dynamic written at the far end says where the hairpin was
                // going; without one it travels the default distance.
                let reached = b
                    .item
                    .marks()
                    .and_then(|m| m.dynamic.as_ref())
                    .and_then(|d| interp.dynamics.get(d).copied());
                let target = match reached {
                    Some(amp) if from_amp > 0.0 => amp / from_amp,
                    _ if spanner.kind == "crescendo" => interp.crescendo,
                    _ => interp.diminuendo,
                };
                hairpins.push(Hairpin {
                    staff: a.staff,
                    start,
                    end,
                    target,
                    shapes_end: reached.is_none(),
                });
            }
            // Everything else is written on the page and says nothing about how
            // it sounds; the emitter is what refuses a kind nobody knows.
            _ => {}
        }
    }
    Ok((slurred, hairpins))
}

/// What starting at `t` is worth, metrically: the product of every accent whose
/// position in the bar this is, in a meter it applies to.
fn metric_gain(sheet: &Sheet, t: Ratio, interp: &Interpretation) -> f64 {
    let (measure, offset) = sheet.grid.position(t);
    let meter = sheet.grid.meter_at(measure);
    let here = format!("{}/{}", meter.count, meter.unit);
    interp
        .accents
        .iter()
        .filter(|a| a.at == offset)
        .filter(|a| a.meter.as_ref().is_none_or(|m| *m == here))
        .map(|a| a.gain)
        .product()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::model::{Marks, Pitch, Spanner, Step, Voice};

    /// A voice of quarter notes on middle C, in one 4/4 bar per four of them.
    fn quarters(n: usize) -> Sheet {
        let mut sheet = Sheet::default();
        sheet.staves[0].voices = vec![Voice {
            items: (0..n)
                .map(|i| Item::Note {
                    id: i as u64 + 1,
                    pitches: vec![Pitch {
                        step: Step::C,
                        alter: 0,
                        octave: 4,
                        forced: false,
                    }],
                    dur: Ratio::new(1, 4),
                    tie: false,
                    marks: Marks::default(),
                })
                .collect(),
        }];
        sheet
    }

    fn mark(sheet: &mut Sheet, id: u64, marks: Marks) {
        for item in &mut sheet.staves[0].voices[0].items {
            if item.id() == id
                && let Item::Note { marks: m, .. } = item
            {
                *m = marks.clone();
            }
        }
    }

    fn spanner(kind: &str, from: u64, to: u64) -> Spanner {
        Spanner {
            kind: kind.into(),
            from,
            to,
        }
    }

    #[test]
    fn a_repeat_is_played_out_and_every_pass_is_heard() {
        let mut sheet = quarters(8);
        sheet.grid.barlines = vec![(0, "rptend".into())];
        let played = performance(sheet, &Interpretation::default()).unwrap();
        // the first bar twice, then the second
        let onsets: Vec<f64> = played.notes.iter().map(|n| n.t).collect();
        assert_eq!(onsets.len(), 12);
        assert_eq!(onsets[4], 4.0);
        assert_eq!(played.notes[4].id, 1, "the first note again");
        assert_eq!(played.notes[8].id, 5);
        assert_eq!(played.heard.iter().filter(|(id, _)| *id == 1).count(), 2);
    }

    #[test]
    fn a_tempo_mark_is_the_tempo_and_none_is_the_readings() {
        let mut sheet = quarters(8);
        let plain = performance(sheet.clone(), &Interpretation::default()).unwrap();
        assert_eq!(plain.tempo, vec![(0.0, 120.0)]);
        sheet.controls = vec![crate::notation::model::Control {
            kind: "tempo".into(),
            on: 5,
            text: "Presto".into(),
            bpm: Some(180.0),
        }];
        let fast = performance(sheet, &Interpretation::default()).unwrap();
        assert_eq!(fast.tempo, vec![(0.0, 120.0), (4.0, 180.0)]);
        let map = fast.tempo_map(4);
        // four beats at two a second, then three a second
        assert!((map.secs_at(7.0) - (2.0 + 1.0)).abs() < 1e-9);
    }

    #[test]
    fn the_lines_between_notes_are_heard() {
        let mut sheet = quarters(8);
        sheet.spanners = vec![
            spanner("8va", 1, 2),
            spanner("gliss", 3, 4),
            spanner("pedal", 5, 6),
        ];
        sheet.staves[0].transpose = -2;
        let played = performance(sheet, &Interpretation::default()).unwrap();
        assert_eq!(
            played.notes[0].pitch,
            60 + 12 - 2,
            "an octave up, a tone down"
        );
        assert_eq!(played.notes[2].pitch, 58);
        assert_eq!(played.notes[2].glide, Some(0.0), "a slide to the same C");
        assert_eq!(played.pedals, vec![(0, 4.0, 6.0)]);
    }

    #[test]
    fn a_tremolo_a_roll_and_two_notes_alternating_are_their_notes() {
        let mut sheet = quarters(4);
        mark(
            &mut sheet,
            1,
            Marks {
                tremolo: Some(2),
                ..Marks::default()
            },
        );
        if let Item::Note { pitches, marks, .. } = &mut sheet.staves[0].voices[0].items[1] {
            pitches.push(Pitch {
                step: Step::E,
                alter: 0,
                octave: 4,
                forced: false,
            });
            marks.arpeggio = Some("up".into());
        }
        sheet.spanners = vec![spanner("ftrem", 3, 4)];
        let played = performance(sheet, &Interpretation::default()).unwrap();
        // a quarter in sixteenths is four
        assert_eq!(played.notes.iter().filter(|n| n.id == 1).count(), 4);
        let rolled: Vec<&Note> = played.notes.iter().filter(|n| n.id == 2).collect();
        assert!(rolled[1].t > rolled[0].t, "the E after the C");
        // two quarters alternating in sixteenths: eight, the two in turn
        let alternate: Vec<u64> = played
            .notes
            .iter()
            .filter(|n| n.t >= 2.0)
            .map(|n| n.id)
            .collect();
        assert_eq!(alternate.len(), 8);
        assert!(
            alternate.iter().all(|id| *id == 3),
            "both heard under the first"
        );
    }

    #[test]
    fn a_breath_shortens_and_let_it_ring_holds() {
        let mut sheet = quarters(2);
        mark(
            &mut sheet,
            1,
            Marks {
                breath: Some("breath".into()),
                ..Marks::default()
            },
        );
        mark(
            &mut sheet,
            2,
            Marks {
                ring: true,
                ..Marks::default()
            },
        );
        let played = performance(sheet, &Interpretation::default()).unwrap();
        assert_eq!(played.notes[0].sustain, 0.75);
        assert_eq!(played.notes[1].sustain, 2.0);
    }

    #[test]
    fn a_plain_quarter_is_one_beat_written_and_one_beat_held() {
        let notes = perform(quarters(4), &Interpretation::default()).unwrap();
        assert_eq!(notes.len(), 4);
        assert_eq!(notes[1].t, 1.0);
        assert_eq!(notes[1].dur, 1.0);
        assert_eq!(notes[1].sustain, 1.0);
        assert_eq!(notes[1].pitch, 60);
    }

    #[test]
    fn a_staccato_shortens_the_sound_and_moves_no_attack() {
        let mut sheet = quarters(4);
        mark(
            &mut sheet,
            2,
            Marks {
                articulations: vec!["stacc".into()],
                ..Marks::default()
            },
        );
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        // The one that carries it sounds half as long...
        assert_eq!(notes[1].dur, 1.0);
        assert_eq!(notes[1].sustain, 0.5);
        // ...and every attack after it is exactly where it was.
        assert_eq!(
            notes.iter().map(|n| n.t).collect::<Vec<_>>(),
            vec![0.0, 1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn a_sounding_note_carries_what_was_written_on_it_as_it_was_written() {
        // The staccato is honoured -- and it is still on the note. A client
        // that plays the page and writes it back writes the staccato, not the
        // shortened length it produced, or the second reading shortens it
        // again.
        let mut sheet = quarters(2);
        mark(
            &mut sheet,
            1,
            Marks {
                articulations: vec!["stacc".into()],
                dynamic: Some("mf".into()),
                ..Marks::default()
            },
        );
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        assert_eq!(notes[0].sustain, 0.5);
        assert_eq!(notes[0].marks.articulations, ["stacc"]);
        assert_eq!(notes[0].marks.dynamic.as_deref(), Some("mf"));
        // A note nothing is written on carries nothing, which is what keeps a
        // plain phrase's payload what it always was.
        assert!(notes[1].marks.is_empty());
        assert!(!serde_json::to_string(&notes[1]).unwrap().contains("marks"));
    }

    #[test]
    fn a_number_cannot_tell_a_flat_from_g_sharp_and_the_note_can() {
        let mut sheet = quarters(2);
        let items = &mut sheet.staves[0].voices[0].items;
        let Item::Note { pitches, .. } = &mut items[0] else {
            panic!("a note");
        };
        pitches[0] = Pitch {
            step: Step::A,
            alter: -1,
            octave: 4,
            forced: true,
        };
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        assert_eq!(notes[0].pitch, 68);
        assert_eq!(notes[0].spelling.as_deref(), Some("flat"));
        assert_eq!(notes[0].accidental.as_deref(), Some("written"));
        // A natural chose nothing and says so.
        assert_eq!(notes[1].spelling, None);
        assert_eq!(notes[1].accidental, None);
    }

    #[test]
    fn a_written_sounding_length_beats_every_table() {
        let mut sheet = quarters(2);
        mark(
            &mut sheet,
            1,
            Marks {
                articulations: vec!["stacc".into()],
                sounding: Some(Ratio::new(1, 8)),
                ..Marks::default()
            },
        );
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        assert_eq!(notes[0].sustain, 0.5);
    }

    #[test]
    fn a_dynamic_governs_until_the_next_one() {
        let mut sheet = quarters(4);
        mark(
            &mut sheet,
            2,
            Marks {
                dynamic: Some("p".into()),
                ..Marks::default()
            },
        );
        mark(
            &mut sheet,
            4,
            Marks {
                dynamic: Some("f".into()),
                ..Marks::default()
            },
        );
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        // Before the first mark the page says nothing, so the default holds --
        // stressed here, since it is the downbeat.
        assert!((notes[0].amp - 0.12 * 1.2).abs() < 1e-12);
        assert!((notes[1].amp - 0.05).abs() < 1e-12);
        assert!((notes[2].amp - 0.05).abs() < 1e-12, "still p");
        assert!((notes[3].amp - 0.17).abs() < 1e-12);
    }

    #[test]
    fn a_crescendo_is_heard_across_its_span_and_nowhere_else() {
        let mut sheet = quarters(8);
        sheet.spanners = vec![Spanner {
            kind: "crescendo".into(),
            from: 1,
            to: 5,
        }];
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        let amps: Vec<f64> = notes.iter().map(|n| n.amp).collect();
        // Rising over the span...
        for pair in amps[..5].windows(2) {
            assert!(pair[1] > pair[0] || amps[1] < amps[0], "{amps:?}");
        }
        // ...and by the far end it has travelled the default distance (the
        // downbeat's own stress is on both ends, so it cancels in the ratio).
        assert!((amps[4] / amps[0] - 1.6).abs() < 1e-12, "{amps:?}");
        // Past it, nothing.
        assert!((amps[5] - amps[6]).abs() < 1e-12);
    }

    #[test]
    fn a_crescendo_ending_on_a_dynamic_goes_where_the_page_says() {
        let mut sheet = quarters(5);
        mark(
            &mut sheet,
            5,
            Marks {
                dynamic: Some("ff".into()),
                ..Marks::default()
            },
        );
        sheet.spanners = vec![Spanner {
            kind: "crescendo".into(),
            from: 1,
            to: 5,
        }];
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        // The written `ff` is where it was going, not the default factor -- and
        // it is reached by being written, not by being ramped to twice.
        let last = notes.last().unwrap();
        assert!((last.amp - 0.25 * 1.2).abs() < 1e-12, "{last:?}");
        // The note before it is on the way there and not yet arrived.
        assert!(
            notes[3].amp > notes[0].amp && notes[3].amp < 0.25,
            "{notes:?}"
        );
    }

    #[test]
    fn a_tie_is_one_sound_of_the_summed_length() {
        let mut sheet = quarters(3);
        if let Item::Note { tie, .. } = &mut sheet.staves[0].voices[0].items[0] {
            *tie = true;
        }
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        assert_eq!(notes.len(), 2, "the second note does not attack again");
        assert_eq!(notes[0].dur, 2.0);
        assert_eq!(notes[0].sustain, 2.0);
        assert_eq!(notes[1].t, 2.0);
    }

    #[test]
    fn a_tuplet_needs_no_rule_because_its_division_is_exact() {
        let mut sheet = Sheet::default();
        sheet.staves[0].voices = vec![Voice {
            items: (0..3)
                .map(|i| Item::Note {
                    id: i as u64 + 1,
                    pitches: vec![Pitch {
                        step: Step::C,
                        alter: 0,
                        octave: 4,
                        forced: false,
                    }],
                    dur: Ratio::new(1, 12),
                    tie: false,
                    marks: Marks::default(),
                })
                .collect(),
        }];
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        assert!((notes[2].t - 2.0 / 3.0).abs() < 1e-12);
        assert!((notes[0].dur - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn the_downbeat_is_stressed_and_the_rest_of_the_bar_is_a_style() {
        let notes = perform(quarters(8), &Interpretation::default()).unwrap();
        assert!(notes[0].amp > notes[1].amp);
        assert!(
            (notes[1].amp - notes[2].amp).abs() < 1e-12,
            "no beat 3 by default"
        );
        assert!(
            (notes[4].amp - notes[0].amp).abs() < 1e-12,
            "the next bar too"
        );

        // A style says so by passing its own accents, and edits no core.
        let mut style = Interpretation::default();
        style.accents.push(Accent {
            at: Ratio::new(1, 2),
            gain: 1.1,
            meter: Some("4/4".into()),
        });
        let notes = perform(quarters(8), &style).unwrap();
        assert!((notes[2].amp / notes[1].amp - 1.1).abs() < 1e-12);
    }

    #[test]
    fn an_accent_bound_to_a_meter_does_not_leak_into_another_one() {
        let mut style = Interpretation::default();
        style.accents.push(Accent {
            at: Ratio::new(1, 2),
            gain: 1.1,
            meter: Some("4/4".into()),
        });
        let mut sheet = quarters(6);
        sheet.grid = super::super::model::Grid::uniform(3, 4);
        let notes = perform(sheet, &style).unwrap();
        // Half a bar into a 3/4 is not a beat at all, and the rule is not for
        // this meter anyway.
        assert!((notes[1].amp - notes[2].amp).abs() < 1e-12);
    }

    #[test]
    fn a_slur_is_a_length_the_player_chooses_and_the_default_chooses_nothing() {
        let mut sheet = quarters(4);
        sheet.spanners = vec![Spanner {
            kind: "slur".into(),
            from: 1,
            to: 3,
        }];
        let plain = perform(sheet.clone(), &Interpretation::default()).unwrap();
        assert!(plain.iter().all(|n| n.sustain == n.dur));

        let played = Interpretation {
            detach: 0.6,
            slur: 1.0,
            ..Interpretation::default()
        };
        let notes = perform(sheet, &played).unwrap();
        // The slur reaches every note between its ends, not only the two named.
        assert_eq!(
            notes.iter().map(|n| n.sustain).collect::<Vec<_>>(),
            vec![1.0, 1.0, 1.0, 0.6]
        );
    }

    #[test]
    fn a_staff_says_which_line_it_is_and_never_what_plays_it() {
        let mut sheet = quarters(2);
        sheet.staves.push(sheet.staves[0].clone());
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        assert_eq!(notes.len(), 4);
        assert_eq!(
            notes.iter().map(|n| n.staff).collect::<Vec<_>>(),
            vec![0, 1, 0, 1]
        );
    }

    #[test]
    fn a_spanner_written_to_a_note_that_is_gone_is_refused_by_name() {
        let mut sheet = quarters(2);
        sheet.spanners = vec![Spanner {
            kind: "crescendo".into(),
            from: 1,
            to: 99,
        }];
        let err = perform(sheet, &Interpretation::default()).unwrap_err();
        assert!(err.contains("crescendo") && err.contains("99"), "{err}");
    }

    #[test]
    fn a_rest_sounds_nothing_and_still_takes_its_time() {
        let mut sheet = quarters(3);
        sheet.staves[0].voices[0].items[1] = Item::Rest {
            id: 2,
            dur: Ratio::new(1, 4),
        };
        let notes = perform(sheet, &Interpretation::default()).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[1].t, 2.0);
    }

    #[test]
    fn the_beat_is_the_callers_and_the_default_is_the_quarter() {
        let halved = Interpretation {
            beat_unit: 8,
            ..Interpretation::default()
        };
        let notes = perform(quarters(2), &halved).unwrap();
        assert_eq!(notes[1].t, 2.0, "a quarter is two eighth-beats");
        assert_eq!(notes[0].dur, 2.0);
    }
}
