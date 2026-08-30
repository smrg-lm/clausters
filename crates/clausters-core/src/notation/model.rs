//! The score model: two durational structures that do not contain each other.
//!
//! - The **metric layout** ([`Grid`]) — measures and meter changes. It does not
//!   sound. It is a structure of its own, addressable and editable, and that is
//!   what lets a caller say *"the notes of measures 3 to 10"* or *"change the
//!   meter here"* at all: a measure is an **addressing system**, a role it
//!   cannot have while it is a by-product of emission. It is also what makes
//!   **metric position** computable ([`Grid::position`]), which is meaning
//!   rather than layout — whether a note falls on a downbeat is a fact about
//!   the music, and only a grid that can be queried can answer it.
//! - The **content** ([`Staff`], [`Voice`], [`Item`]) — what sounds. It is
//!   **flat**: notes are not nested inside measures. Containment would break
//!   under every operation that changes a length (augment a phrase and the
//!   notes no longer fit the bars they were nested in), and barring is
//!   recomputed at emission anyway.
//!
//! MEI nests (`<measure><staff><layer>`), so writing the document **projects**
//! flat content onto the grid — the split-and-tie [`super::voice_to_mei`]
//! already does. A flat model and a nested document are not in conflict.
//!
//! **Durations and positions are exact [`Ratio`]s**, in whole notes: a quarter
//! is `1/4` and a triplet eighth is `1/12`. Ticks are a *boundary* — MEI's
//! `@dur` and dots, MIDI's ticks, OSC's seconds — converted at each protocol's
//! edge, never the foundation the model rests on.
//!
//! **The type is `Sheet`, not `Score`**, because [`super::Score`] is already the
//! engraver-driven document — a handle to a layout engine, with state that
//! lives in C++ or in a wasm module. This is the other thing: plain data, with
//! no engraver anywhere near it, which is exactly why it can cross to a client
//! by value and be edited by a standalone host with no client attached.

use serde::{Deserialize, Serialize};

use crate::ratio::Ratio;

/// A diatonic step name — the letter a notehead sits on, independent of any
/// accidental. `C` is 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    C,
    D,
    E,
    F,
    G,
    A,
    B,
}

impl Step {
    /// The seven steps in order, so an index can be stepped and wrapped.
    pub const ALL: [Step; 7] = [
        Step::C,
        Step::D,
        Step::E,
        Step::F,
        Step::G,
        Step::A,
        Step::B,
    ];

    /// Position in the diatonic scale, `C` = 0 through `B` = 6.
    pub fn index(self) -> i32 {
        Step::ALL.iter().position(|&s| s == self).unwrap() as i32
    }

    /// Semitones above `C` for the natural of this step.
    pub fn semitones(self) -> i32 {
        [0, 2, 4, 5, 7, 9, 11][self.index() as usize]
    }

    /// The MEI `@pname` letter.
    pub fn pname(self) -> &'static str {
        ["c", "d", "e", "f", "g", "a", "b"][self.index() as usize]
    }
}

/// A written pitch: the step it is notated on, how many semitones it is altered
/// by, and its octave in scientific pitch (`c4` is middle C, MIDI 60).
///
/// **Spelling is part of the pitch, not derived from it.** `F#` and `Gb` are one
/// MIDI number and two different notes on the page — different noteheads,
/// different accidentals, different ledger lines above a certain range — so the
/// model stores what is written and computes the MIDI number from it
/// ([`Pitch::midi`]) rather than the other way round. Going the other way
/// ([`Pitch::from_midi`]) is a *choice* of spelling and says so by taking one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pitch {
    /// The letter the notehead sits on.
    pub step: Step,
    /// Semitones of alteration: `0` natural, `1` sharp, `-1` flat, `2`/`-2` the
    /// doubles.
    #[serde(default)]
    pub alter: i32,
    /// Scientific octave — `4` is the octave of middle C.
    pub octave: i32,
    /// Whether the accidental must be **printed** even where the key signature
    /// or the measure already implies it — a courtesy or editorial accidental.
    ///
    /// Left false, the alteration is stated as the *sounding* one and the
    /// engraver decides whether to draw a sign, which is what keeps a scale in
    /// E flat from carrying a flat on every note. It is a fact about the score,
    /// not an instruction to the encoder: this pitch is one whose accidental
    /// the writer wants seen.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forced: bool,
}

impl Pitch {
    /// The MIDI note number this pitch sounds. Middle C (`c4`) is 60.
    pub fn midi(&self) -> i32 {
        (self.octave + 1) * 12 + self.step.semitones() + self.alter
    }

    /// Spell a MIDI number, choosing flats or sharps for the black keys. The
    /// only sensible thing to do when the source has no spelling of its own —
    /// a MIDI file, a client's `midinote` — and the reason a caller has to say
    /// which world it wants.
    pub fn from_midi(midi: i32, flats: bool) -> Pitch {
        // (step index, alter) per pitch class, one table per accidental world.
        const SHARP: [(usize, i32); 12] = [
            (0, 0),
            (0, 1),
            (1, 0),
            (1, 1),
            (2, 0),
            (3, 0),
            (3, 1),
            (4, 0),
            (4, 1),
            (5, 0),
            (5, 1),
            (6, 0),
        ];
        const FLAT: [(usize, i32); 12] = [
            (0, 0),
            (1, -1),
            (1, 0),
            (2, -1),
            (2, 0),
            (3, 0),
            (4, -1),
            (4, 0),
            (5, -1),
            (5, 0),
            (6, -1),
            (6, 0),
        ];
        let table = if flats { &FLAT } else { &SHARP };
        let (step, alter) = table[midi.rem_euclid(12) as usize];
        Pitch {
            step: Step::ALL[step],
            alter,
            // A spelling chosen from a bare number is never a courtesy sign:
            // the writer said nothing about wanting it printed.
            forced: false,
            // The octave is the sounding one: a `cb4` sounds in octave 3 but is
            // written in 4, so it is derived from the *natural* the spelling
            // sits on rather than from the MIDI number directly.
            octave: (midi - alter).div_euclid(12) - 1,
        }
    }
}

/// The marks a note carries beyond its pitch and value.
///
/// Every field is **a musical fact, not an instruction to the encoder** — an
/// articulation is a thing the note has, not a request to draw a dot — because
/// a fact can be read back by the interpreter that plays the page, and an
/// instruction can only ever be written. They are declared here and left empty;
/// emitting and reading them is the emission milestone's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    /// Articulations, by their MEI names (`stacc`, `acc`, `ten`, `marc`, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub articulations: Vec<String>,
    /// A dynamic attached to this note (`pp`, `mf`, `ff`, …). It is written
    /// under the staff at this note's own position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<String>,
    /// An ornament on this note (`trill`, `mordent`, `turn`, `fermata`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ornament: Option<String>,
    /// That this is a grace note, and of which kind (`acc` for an
    /// acciaccatura, `unacc` for an appoggiatura, `unknown`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace: Option<String>,
    /// A forced stem direction (`up`, `down`). Left out, the engraver decides,
    /// which is what it is for — a stem is a layout answer and only a writer
    /// overruling it is a musical statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stem: Option<String>,
    /// How long the note **sounds**, when that is not how long it is written.
    ///
    /// The written value is the item's `dur`; this is the length in the air, so
    /// a staccato quarter that sounds an eighth carries both. A performance
    /// nuance a client already has (`sustain` against `dur`) reaches the page
    /// through here, and the two are kept apart because a page that shortened
    /// the written value would be a different piece of music.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sounding: Option<Ratio>,
}

impl Marks {
    /// Whether the note carries nothing beyond its pitch and value — the case a
    /// v1 payload produces, and the one that has to stay byte-identical.
    pub fn is_empty(&self) -> bool {
        self.articulations.is_empty()
            && self.dynamic.is_none()
            && self.ornament.is_none()
            && self.grace.is_none()
            && self.stem.is_none()
            && self.sounding.is_none()
    }
}

/// One durational item of a voice: a note or chord, or a rest.
///
/// Every item carries an **id**, which is how an edit names it. Not an index:
/// an index moves when anything before it is inserted or removed, so a caller
/// holding one would be addressing a different note after every edit but its
/// own. The id is minted once, travels with the item through every operation,
/// and is what a client keeps when it wants to come back to *this* note.
///
/// A note with no pitches is not representable — that is what a rest is — so the
/// two are separate variants rather than one with an empty list. (The v1 wire
/// [`super::Slot`] does spell a rest as an empty pitch list, because a wire form
/// with no discriminator has to be total; the model is not a wire and can be
/// exact.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Item {
    /// A note (one pitch) or chord (several), lasting `dur`.
    Note {
        /// This item's identity, stable across every operation. `0` means one
        /// was never assigned, which [`Sheet::assign_ids`] fixes.
        #[serde(default)]
        id: u64,
        /// The written pitches, low to high by convention but not required.
        pitches: Vec<Pitch>,
        /// The written value, in whole notes.
        dur: Ratio,
        /// Whether this note is tied **to the next item** — a musical tie the
        /// caller asked for. The ties an emitter adds when a note crosses a
        /// barline are not this: they are made at emission, from the split.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        tie: bool,
        /// What the note carries beyond pitch and value.
        #[serde(default, skip_serializing_if = "Marks::is_empty")]
        marks: Marks,
    },
    /// A rest, lasting `dur`.
    Rest {
        /// This item's identity, stable across every operation.
        #[serde(default)]
        id: u64,
        /// The written value, in whole notes.
        dur: Ratio,
    },
}

impl Item {
    /// The written value of this item.
    pub fn dur(&self) -> Ratio {
        match self {
            Item::Note { dur, .. } | Item::Rest { dur, .. } => *dur,
        }
    }

    /// This item's identity.
    pub fn id(&self) -> u64 {
        match self {
            Item::Note { id, .. } | Item::Rest { id, .. } => *id,
        }
    }

    /// The same item with a different written value — how a duration edit and
    /// the time-scaling operations rebuild one without restating its pitches.
    pub fn with_dur(&self, dur: Ratio) -> Item {
        let mut out = self.clone();
        match &mut out {
            Item::Note { dur: d, .. } | Item::Rest { dur: d, .. } => *d = dur,
        }
        out
    }

    /// The same item under a different id — what a copy needs, since two items
    /// sharing an id would both answer to one edit.
    pub fn with_id(&self, id: u64) -> Item {
        let mut out = self.clone();
        match &mut out {
            Item::Note { id: i, .. } | Item::Rest { id: i, .. } => *i = id,
        }
        out
    }

    /// The pitches this item sounds — empty for a rest.
    pub fn pitches(&self) -> &[Pitch] {
        match self {
            Item::Note { pitches, .. } => pitches,
            Item::Rest { .. } => &[],
        }
    }

    /// The marks this item carries, if it can carry any.
    pub fn marks(&self) -> Option<&Marks> {
        match self {
            Item::Note { marks, .. } => Some(marks),
            Item::Rest { .. } => None,
        }
    }

    /// Whether this item sounds — a note with pitches, as against a rest.
    pub fn sounds(&self) -> bool {
        !self.pitches().is_empty()
    }

    /// A rest of the same length, keeping the id — what silencing a note is,
    /// as against deleting it, which would shorten the voice.
    pub fn silenced(&self) -> Item {
        Item::Rest {
            id: self.id(),
            dur: self.dur(),
        }
    }
}

/// One monophonic line: items back to back, no gaps (a gap is a rest). This is
/// exactly one MEI `<layer>`, and it stays the **composable primitive** —
/// polyphony stacks several voices rather than widening one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voice {
    /// The items, in order from the start of the score.
    #[serde(default)]
    pub items: Vec<Item>,
}

impl Voice {
    /// The total written length of the voice.
    pub fn len(&self) -> Ratio {
        self.items
            .iter()
            .fold(Ratio::ZERO, |acc, item| acc + item.dur())
    }

    /// Whether the voice holds nothing at all.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// One staff — a clef of its own and the voices written on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staff {
    /// The clef, as a shape and a line: `"G2"`, `"F4"`, `"C3"`.
    #[serde(default = "default_clef")]
    pub clef: String,
    /// The voices sharing this staff.
    #[serde(default)]
    pub voices: Vec<Voice>,
}

fn default_clef() -> String {
    "G2".to_string()
}

impl Default for Staff {
    fn default() -> Staff {
        Staff {
            clef: default_clef(),
            voices: vec![Voice::default()],
        }
    }
}

impl Staff {
    /// The length of the longest voice on the staff.
    pub fn len(&self) -> Ratio {
        self.voices
            .iter()
            .map(Voice::len)
            .max()
            .unwrap_or(Ratio::ZERO)
    }

    /// Whether the staff holds no voice with anything in it.
    pub fn is_empty(&self) -> bool {
        self.voices.iter().all(Voice::is_empty)
    }
}

/// A meter, taking effect at a measure index and standing until the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meter {
    /// The measure this meter starts at. The grid's first meter is at `0`.
    #[serde(default)]
    pub measure: usize,
    /// The numerator — beats in the bar.
    pub count: i64,
    /// The denominator — what one beat is worth.
    pub unit: i64,
}

impl Meter {
    /// The length of a full bar in this meter, in whole notes.
    pub fn bar(&self) -> Ratio {
        Ratio::new(self.count, self.unit)
    }
}

/// The metric layout: the measures the content is written over.
///
/// It does not sound and it holds no notes. What it holds is enough to answer
/// where every barline falls: the meters in force, and any bar whose length is
/// **not** its meter's — an anacrusis (which is simply the override at measure
/// 0), or an irregular bar in the middle of a piece.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid {
    /// The meters, ordered by the measure they start at; the first is at `0`.
    pub meters: Vec<Meter>,
    /// Bars whose length differs from their meter's, as `(measure, length)`.
    /// An **anacrusis is the override at measure 0** — one concept, not two.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub irregular: Vec<(usize, Ratio)>,
}

impl Default for Grid {
    fn default() -> Grid {
        Grid {
            meters: vec![Meter {
                measure: 0,
                count: 4,
                unit: 4,
            }],
            irregular: Vec::new(),
        }
    }
}

impl Grid {
    /// A grid of one meter throughout — the v1 case, and what a caller who has
    /// only a `"4/4"` string means.
    pub fn uniform(count: i64, unit: i64) -> Grid {
        Grid {
            meters: vec![Meter {
                measure: 0,
                count,
                unit,
            }],
            irregular: Vec::new(),
        }
    }

    /// The meter in force at `measure` — the last one that starts at or before
    /// it. A grid with no meters at all reads as 4/4 rather than dividing by
    /// zero: an empty list is a caller's omission, not a musical statement.
    pub fn meter_at(&self, measure: usize) -> Meter {
        self.meters
            .iter()
            .filter(|m| m.measure <= measure)
            .max_by_key(|m| m.measure)
            .copied()
            .unwrap_or(Meter {
                measure: 0,
                count: 4,
                unit: 4,
            })
    }

    /// The length of `measure`: its override if it has one, else its meter's
    /// full bar.
    pub fn bar_len(&self, measure: usize) -> Ratio {
        self.irregular
            .iter()
            .find(|(m, _)| *m == measure)
            .map(|(_, len)| *len)
            .unwrap_or_else(|| self.meter_at(measure).bar())
    }

    /// Where `measure` starts, in whole notes from the start of the score.
    /// Measure indices are 0-based here; what a reader calls "measure 1" is 0.
    pub fn measure_start(&self, measure: usize) -> Ratio {
        (0..measure).fold(Ratio::ZERO, |acc, m| acc + self.bar_len(m))
    }

    /// The span `[start, end)` covered by measures `first..=last`, in whole
    /// notes — **the addressing a client must never compute for itself**. Two
    /// clients doing this arithmetic separately round differently the moment a
    /// meter changes or a bar is irregular, and the disagreement shows up as an
    /// edit that lands on the wrong notes in one of them.
    pub fn span(&self, first: usize, last: usize) -> (Ratio, Ratio) {
        let start = self.measure_start(first);
        let end = (first..=last).fold(start, |acc, m| acc + self.bar_len(m));
        (start, end)
    }

    /// Which measure the position `t` falls in, and how far into it — the
    /// **metric position**, which is what tells a reader (and an interpreter)
    /// that a note is on a downbeat.
    ///
    /// `t` past the end of every written measure keeps counting in the last
    /// meter, since the grid is as long as the music is and a caller may ask
    /// about a position the content has not reached yet.
    pub fn position(&self, t: Ratio) -> (usize, Ratio) {
        let mut measure = 0;
        let mut start = Ratio::ZERO;
        loop {
            let len = self.bar_len(measure);
            if !len.is_positive() {
                // A zero-length bar would loop forever; treat it as the end.
                return (measure, t - start);
            }
            let next = start + len;
            if t < next {
                return (measure, t - start);
            }
            start = next;
            measure += 1;
        }
    }
}

/// Something written **between** two notes rather than on one: a slur, a
/// crescendo.
///
/// It cannot live on an item, because it has two ends — which is the whole
/// reason the sheet carries a list of them beside the staves rather than the
/// content carrying them. `from` and `to` are item ids, so a spanner survives
/// every operation that keeps those items and is refused by the emitter when
/// one of them is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spanner {
    /// What it is: `slur`, `crescendo`, `diminuendo`.
    pub kind: String,
    /// The item it starts on.
    pub from: u64,
    /// The item it ends on.
    pub to: u64,
}

/// A score as data: the metric layout, the staves written over it, and the key
/// they are read in.
///
/// This is the whole model — what crosses to a client **by value**, what an
/// operation takes and returns, and what a standalone host holds when there is
/// no client language in the process at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sheet {
    /// The next id to mint. `0` reads as "nothing has been minted yet", which
    /// is what a sheet written by hand or by an older client arrives as.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub next_id: u64,
    /// The metric layout.
    #[serde(default)]
    pub grid: Grid,
    /// The key, as a tonic name (`"C"`, `"F"`, `"Bb"`, `"F#"`). It selects the
    /// signature and the sharp-versus-flat world a spelling defaults to.
    #[serde(default = "default_key")]
    pub key: String,
    /// The staves, top to bottom.
    #[serde(default)]
    pub staves: Vec<Staff>,
    /// What is written between two notes rather than on one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spanners: Vec<Spanner>,
}

fn default_key() -> String {
    "C".to_string()
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Default for Sheet {
    fn default() -> Sheet {
        Sheet {
            next_id: 0,
            grid: Grid::default(),
            key: default_key(),
            staves: vec![Staff::default()],
            spanners: Vec::new(),
        }
    }
}

impl Sheet {
    /// The written length of the longest staff.
    pub fn len(&self) -> Ratio {
        self.staves
            .iter()
            .map(Staff::len)
            .max()
            .unwrap_or(Ratio::ZERO)
    }

    /// Whether nothing is written on any staff.
    pub fn is_empty(&self) -> bool {
        self.staves.iter().all(Staff::is_empty)
    }

    /// Every voice on the sheet, in reading order, with the staff it belongs
    /// to — how an operation walks the content without caring how it is split.
    pub fn voices_mut(&mut self) -> impl Iterator<Item = &mut Voice> {
        self.staves.iter_mut().flat_map(|s| s.voices.iter_mut())
    }

    /// Every voice on the sheet, in reading order.
    pub fn voices(&self) -> impl Iterator<Item = &Voice> {
        self.staves.iter().flat_map(|s| s.voices.iter())
    }

    /// Mint one id. Every item an operation creates takes one from here, so no
    /// two items in a sheet answer to the same edit.
    pub fn mint(&mut self) -> u64 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Give every unidentified item an id, and put the counter past every id
    /// already in use.
    ///
    /// Called before any operation, so a sheet written by hand, parsed from an
    /// older payload, or built by a caller who never thought about identity
    /// behaves exactly like one this layer minted — an edit can name any note
    /// in it, and nothing collides.
    pub fn assign_ids(&mut self) {
        let used = self
            .voices()
            .flat_map(|v| v.items.iter())
            .map(Item::id)
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(used + 1);
        let mut next = self.next_id;
        for voice in self.staves.iter_mut().flat_map(|s| s.voices.iter_mut()) {
            for item in &mut voice.items {
                if item.id() == 0 {
                    *item = item.with_id(next);
                    next += 1;
                }
            }
        }
        self.next_id = next;
    }

    /// The item with this id, and where it is — `(staff, voice, index)`.
    pub fn locate(&self, id: u64) -> Option<(usize, usize, usize)> {
        for (si, staff) in self.staves.iter().enumerate() {
            for (vi, voice) in staff.voices.iter().enumerate() {
                if let Some(ii) = voice.items.iter().position(|i| i.id() == id) {
                    return Some((si, vi, ii));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pitch_keeps_its_spelling_and_still_sounds() {
        let f_sharp = Pitch {
            step: Step::F,
            alter: 1,
            octave: 4,
            forced: false,
        };
        let g_flat = Pitch {
            step: Step::G,
            alter: -1,
            octave: 4,
            forced: false,
        };
        // One MIDI number, two notes on the page -- which is the whole reason
        // spelling is stored rather than derived.
        assert_eq!(f_sharp.midi(), g_flat.midi());
        assert_ne!(f_sharp, g_flat);
        assert_eq!(Pitch::from_midi(60, false).midi(), 60);
        assert_eq!(Pitch::from_midi(61, false).step, Step::C);
        assert_eq!(Pitch::from_midi(61, true).step, Step::D);
    }

    #[test]
    fn a_flat_written_below_c_stays_in_its_written_octave() {
        // cb4 sounds b3 but is written in octave 4: the octave follows the
        // natural the spelling sits on, not the sounding number.
        let c_flat = Pitch {
            step: Step::C,
            alter: -1,
            octave: 4,
            forced: false,
        };
        assert_eq!(c_flat.midi(), 59);
        // and the round trip through a spelling choice lands on b3, which is
        // the same sound spelled the other way.
        assert_eq!(Pitch::from_midi(59, false).octave, 3);
    }

    #[test]
    fn the_grid_addresses_measures_across_a_meter_change() {
        let grid = Grid {
            meters: vec![
                Meter {
                    measure: 0,
                    count: 4,
                    unit: 4,
                },
                Meter {
                    measure: 2,
                    count: 3,
                    unit: 4,
                },
            ],
            irregular: Vec::new(),
        };
        assert_eq!(grid.bar_len(0), Ratio::ONE);
        assert_eq!(grid.bar_len(2), Ratio::new(3, 4));
        // measures 0 and 1 are whole notes, so measure 2 starts at 2.
        assert_eq!(grid.measure_start(2), Ratio::from(2));
        // "measures 3 to 4" (0-based 2..=3) spans two 3/4 bars.
        assert_eq!(
            grid.span(2, 3),
            (Ratio::from(2), Ratio::from(2) + Ratio::new(3, 2))
        );
    }

    #[test]
    fn an_anacrusis_is_the_override_at_measure_zero() {
        let grid = Grid {
            meters: vec![Meter {
                measure: 0,
                count: 4,
                unit: 4,
            }],
            irregular: vec![(0, Ratio::new(1, 4))],
        };
        assert_eq!(grid.bar_len(0), Ratio::new(1, 4));
        assert_eq!(grid.measure_start(1), Ratio::new(1, 4));
        // A note a quarter in is on the downbeat of the first full measure.
        assert_eq!(grid.position(Ratio::new(1, 4)), (1, Ratio::ZERO));
    }

    #[test]
    fn metric_position_says_which_beat_a_note_falls_on() {
        let grid = Grid::uniform(4, 4);
        assert_eq!(grid.position(Ratio::ZERO), (0, Ratio::ZERO));
        // the third beat of the second measure
        assert_eq!(
            grid.position(Ratio::ONE + Ratio::new(1, 2)),
            (1, Ratio::new(1, 2))
        );
        // past everything written, the grid keeps counting
        assert_eq!(grid.position(Ratio::from(9)), (9, Ratio::ZERO));
    }

    #[test]
    fn json_carries_only_what_is_there() {
        let sheet = Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice {
                    items: vec![Item::Note {
                        id: 1,
                        pitches: vec![Pitch {
                            step: Step::C,
                            alter: 0,
                            octave: 4,
                            forced: false,
                        }],
                        dur: Ratio::new(1, 4),
                        tie: false,
                        marks: Marks::default(),
                    }],
                }],
            }],
            ..Default::default()
        };
        let json = serde_json::to_string(&sheet).unwrap();
        // an empty tie and empty marks do not reach the wire
        assert!(!json.contains("tie"));
        assert!(!json.contains("marks"));
        assert_eq!(serde_json::from_str::<Sheet>(&json).unwrap(), sheet);
        // and a minimal sheet reads with every default filled in
        let minimal: Sheet = serde_json::from_str("{}").unwrap();
        assert_eq!(minimal.key, "C");
        assert_eq!(minimal.grid.meter_at(0).count, 4);
    }
}
