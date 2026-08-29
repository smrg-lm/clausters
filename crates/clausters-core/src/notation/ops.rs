//! Operations on the score model: the one implementation every caller binds.
//!
//! **An operation crosses as data.** It is a JSON object naming a verb and its
//! parameters — `{"op": "transpose", "semitones": 2}` — applied to a [`Sheet`]
//! that also crossed as data, returning a new one. Two things follow from that
//! shape, and both are why it was chosen:
//!
//! - **A new verb costs no ABI.** There is one symbol for every operation there
//!   will ever be, so adding one needs no `docs/bindings.md` row and no
//!   `CORE_ABI_VERSION` round. The precedent is already in the tree twice: the
//!   engraver's `edit(action, params)` and the document crate's intents.
//! - **A standalone host can edit.** A host opened on a saved session has no
//!   client language in the process at all; it holds the sheet and applies the
//!   same operations through the same symbol. That is only true while *every*
//!   verb — its arithmetic, its validation and its refusals — is here, and a
//!   client contributes nothing but the name it calls it by. An edit that works
//!   because a client computed something first is an edit a standalone cannot
//!   perform.
//!
//! The cost of the shape is that a binding table sees one symbol and no verbs,
//! so nothing fails when one client grows an operation the other lacks. That is
//! what [`catalog`] is for: it lists what exists, and each client is contrasted
//! against it.
//!
//! **A span is resolved here, never by a caller.** [`Span::Measures`] becomes a
//! stretch of exact time through the grid, which is arithmetic that changes the
//! moment a meter changes or a bar is irregular — two clients doing it
//! separately disagree about which notes an edit touches, and the disagreement
//! is invisible until someone compares two screens.

use serde::{Deserialize, Serialize};

use super::model::{Item, Pitch, Sheet, Step};
use crate::ratio::Ratio;

/// What part of the score an operation applies to.
///
/// Measures are **1-based here**, because this is the number a reader says out
/// loud and a caller types: "measures 3 to 10" is `{"measures": [3, 10]}`. The
/// model indexes them from zero internally and the conversion happens once,
/// here, rather than in every client.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Span {
    /// Everything written on every staff.
    #[default]
    All,
    /// The inclusive range of measures `[first, last]`, 1-based.
    Measures(usize, usize),
}

impl Span {
    /// The stretch of time `[start, end)` this span covers, in whole notes.
    /// `None` for [`Span::All`], which has no bounds to test against.
    ///
    /// # Errors
    /// If the measure range runs backwards or starts before measure 1.
    pub fn resolve(&self, sheet: &Sheet) -> Result<Option<(Ratio, Ratio)>, String> {
        match *self {
            Span::All => Ok(None),
            Span::Measures(first, last) => {
                if first == 0 {
                    return Err("measures are numbered from 1, so there is no measure 0".into());
                }
                if last < first {
                    return Err(format!(
                        "the measure range {first} to {last} runs backwards"
                    ));
                }
                Ok(Some(sheet.grid.span(first - 1, last - 1)))
            }
        }
    }

    /// Whether an item starting at `onset` is inside the span. An item belongs
    /// to the span it **starts** in, so a note tied across the last barline of
    /// a range is transposed whole rather than in half.
    fn holds(bounds: &Option<(Ratio, Ratio)>, onset: Ratio) -> bool {
        match bounds {
            None => true,
            Some((start, end)) => onset >= *start && onset < *end,
        }
    }
}

/// One operation on a sheet.
///
/// The JSON form is the verb under `"op"` and the parameters beside it, so
/// `{"op": "transpose", "semitones": -3, "span": {"measures": [3, 10]}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Move every note in the span by an interval.
    Transpose {
        /// The chromatic size of the interval, in semitones. Positive is up.
        semitones: i32,
        /// The diatonic size, in steps of the staff — what makes the interval a
        /// *named* one and what keeps the spelling right. Omitted, it is the
        /// ordinary reading of that many semitones (4 semitones is a major
        /// third, so 2 steps), which is the rule at [`default_steps`].
        #[serde(default)]
        steps: Option<i32>,
        /// What to move; everything, by default.
        #[serde(default)]
        span: Span,
    },
}

/// The diatonic size a chromatic interval is ordinarily read as: 4 semitones is
/// a major third (2 steps), 3 is a minor third (2 steps as well), 6 is a
/// tritone, read here as an augmented fourth (3 steps).
///
/// This is the default a caller who says only "up two semitones" gets, and it
/// is a *default*, not a law — passing `steps` explicitly is how a caller asks
/// for the diminished third nobody's shorthand means.
pub fn default_steps(semitones: i32) -> i32 {
    const TABLE: [i32; 12] = [0, 1, 1, 2, 2, 3, 3, 4, 5, 5, 6, 6];
    7 * semitones.div_euclid(12) + TABLE[semitones.rem_euclid(12) as usize]
}

/// Move one pitch by an interval given as `(steps, semitones)`, keeping the
/// spelling the interval implies: the notehead moves `steps` places on the
/// staff and the accidental is whatever makes the result sound `semitones`
/// away.
pub fn transpose_pitch(pitch: &Pitch, steps: i32, semitones: i32) -> Pitch {
    let index = pitch.step.index() + steps;
    let step = Step::ALL[index.rem_euclid(7) as usize];
    let octave = pitch.octave + index.div_euclid(7);
    // The alteration is not carried over: it is re-derived, so that the note
    // sounds exactly `semitones` away from where it was however it was spelled.
    let natural = (octave + 1) * 12 + step.semitones();
    Pitch {
        step,
        alter: pitch.midi() + semitones - natural,
        octave,
    }
}

/// Apply one operation, returning the new sheet.
///
/// The sheet is taken by value and given back changed: an operation is a
/// function from a score to a score, which is what lets them compose without
/// anybody owning an intermediate.
///
/// # Errors
/// With a sentence saying what was refused and why — never a silent no-op, and
/// never a partial application: an operation that cannot be carried out leaves
/// the caller's sheet untouched, because it was never handed over.
pub fn apply(sheet: Sheet, op: &Op) -> Result<Sheet, String> {
    match op {
        Op::Transpose {
            semitones,
            steps,
            span,
        } => transpose(sheet, *semitones, *steps, span),
    }
}

fn transpose(
    mut sheet: Sheet,
    semitones: i32,
    steps: Option<i32>,
    span: &Span,
) -> Result<Sheet, String> {
    let steps = steps.unwrap_or_else(|| default_steps(semitones));
    let bounds = span.resolve(&sheet)?;
    for voice in sheet.voices_mut() {
        let mut onset = Ratio::ZERO;
        for item in &mut voice.items {
            let dur = item.dur();
            if let Item::Note { pitches, .. } = item
                && Span::holds(&bounds, onset)
            {
                for pitch in pitches.iter_mut() {
                    *pitch = transpose_pitch(pitch, steps, semitones);
                }
            }
            onset = onset + dur;
        }
    }
    Ok(sheet)
}

/// One entry of the operation catalog.
#[derive(Debug, Clone, Serialize)]
pub struct OpSpec {
    /// The verb, as it is written under `"op"`.
    pub op: &'static str,
    /// Parameters that must be given.
    pub required: &'static [&'static str],
    /// Parameters that may be given.
    pub optional: &'static [&'static str],
}

/// Every operation this core knows, and the parameters each takes.
///
/// **This exists because the ABI cannot see the verbs.** Operations cross as
/// data through one symbol, so `tests/bindings.rs` proves nothing about which
/// of them a client actually offers — the same structural blindness the props
/// manifest has, which is how five builder divergences went unnoticed. Each
/// client is contrasted against this list instead: a verb here with no binding,
/// or a binding with no verb here, fails a test in that client.
pub fn catalog() -> &'static [OpSpec] {
    &[OpSpec {
        op: "transpose",
        required: &["semitones"],
        optional: &["steps", "span"],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation::model::{Grid, Marks, Meter, Staff, Voice};

    fn pitch(step: Step, alter: i32, octave: i32) -> Pitch {
        Pitch {
            step,
            alter,
            octave,
        }
    }

    fn sheet_of(pitches: &[Pitch], dur: Ratio) -> Sheet {
        Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice {
                    items: pitches
                        .iter()
                        .map(|p| Item::Note {
                            pitches: vec![*p],
                            dur,
                            tie: false,
                            marks: Marks::default(),
                        })
                        .collect(),
                }],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn an_interval_keeps_the_spelling_it_implies() {
        // A major third up from C is E, not F-flat: the notehead moves two
        // places and the accidental follows.
        let c = pitch(Step::C, 0, 4);
        assert_eq!(transpose_pitch(&c, 2, 4), pitch(Step::E, 0, 4));
        // A minor third up from C is E-flat -- same two steps, one semitone
        // less, so the alteration is where the difference lands.
        assert_eq!(transpose_pitch(&c, 2, 3), pitch(Step::E, -1, 4));
        // Up a semitone from B4 crosses into the next octave.
        let b = pitch(Step::B, 0, 4);
        assert_eq!(transpose_pitch(&b, 1, 1), pitch(Step::C, 0, 5));
        // Down a major second from C4 drops an octave on the staff.
        assert_eq!(transpose_pitch(&c, -1, -2), pitch(Step::B, -1, 3));
    }

    #[test]
    fn the_default_reading_of_a_semitone_count() {
        assert_eq!(default_steps(0), 0);
        assert_eq!(default_steps(4), 2); // a major third
        assert_eq!(default_steps(7), 4); // a fifth
        assert_eq!(default_steps(12), 7); // an octave
        assert_eq!(default_steps(-1), -1); // a semitone down is a step down
        assert_eq!(default_steps(-12), -7);
    }

    #[test]
    fn transpose_moves_every_note_and_sounds_the_interval() {
        let sheet = sheet_of(
            &[pitch(Step::C, 0, 4), pitch(Step::G, 0, 4)],
            Ratio::new(1, 4),
        );
        let out = apply(
            sheet,
            &Op::Transpose {
                semitones: 2,
                steps: None,
                span: Span::All,
            },
        )
        .expect("transposes");
        let voice = &out.staves[0].voices[0];
        assert_eq!(voice.items[0].pitches()[0], pitch(Step::D, 0, 4));
        assert_eq!(voice.items[1].pitches()[0], pitch(Step::A, 0, 4));
    }

    #[test]
    fn a_span_is_measured_by_the_grid_and_not_by_the_caller() {
        // Four quarter notes in 4/4 -- one per beat, all in measure 1 -- then
        // four more in measure 2. Asking for measure 2 must move only those.
        let sheet = sheet_of(&[pitch(Step::C, 0, 4); 8], Ratio::new(1, 4));
        let out = apply(
            sheet,
            &Op::Transpose {
                semitones: 12,
                steps: None,
                span: Span::Measures(2, 2),
            },
        )
        .expect("transposes");
        let voice = &out.staves[0].voices[0];
        assert_eq!(voice.items[3].pitches()[0].octave, 4, "still measure 1");
        assert_eq!(voice.items[4].pitches()[0].octave, 5, "measure 2 moved");
        assert_eq!(voice.items[7].pitches()[0].octave, 5);
    }

    #[test]
    fn the_same_span_names_different_notes_after_a_meter_change() {
        // Eight quarters, but measure 1 is 3/4: measure 2 then starts at the
        // fourth note rather than the fifth. This is the arithmetic a client
        // must not do for itself, and the case that catches it doing so.
        let mut sheet = sheet_of(&[pitch(Step::C, 0, 4); 8], Ratio::new(1, 4));
        sheet.grid = Grid::uniform(3, 4);
        let out = apply(
            sheet,
            &Op::Transpose {
                semitones: 12,
                steps: None,
                span: Span::Measures(2, 2),
            },
        )
        .expect("transposes");
        let voice = &out.staves[0].voices[0];
        assert_eq!(voice.items[2].pitches()[0].octave, 4, "still measure 1");
        assert_eq!(voice.items[3].pitches()[0].octave, 5, "measure 2 moved");
        assert_eq!(voice.items[6].pitches()[0].octave, 4, "measure 3 untouched");
    }

    #[test]
    fn an_irregular_first_bar_moves_every_span_after_it() {
        let mut sheet = sheet_of(&[pitch(Step::C, 0, 4); 8], Ratio::new(1, 4));
        sheet.grid = Grid {
            meters: vec![Meter {
                measure: 0,
                count: 4,
                unit: 4,
            }],
            // an anacrusis of one quarter
            irregular: vec![(0, Ratio::new(1, 4))],
        };
        let out = apply(
            sheet,
            &Op::Transpose {
                semitones: 12,
                steps: None,
                span: Span::Measures(1, 1),
            },
        )
        .expect("transposes");
        let voice = &out.staves[0].voices[0];
        assert_eq!(voice.items[0].pitches()[0].octave, 5, "the pickup note");
        assert_eq!(voice.items[1].pitches()[0].octave, 4, "measure 2 onward");
    }

    #[test]
    fn a_refused_operation_says_why_and_changes_nothing() {
        let sheet = sheet_of(&[pitch(Step::C, 0, 4)], Ratio::new(1, 4));
        let err = apply(
            sheet.clone(),
            &Op::Transpose {
                semitones: 1,
                steps: None,
                span: Span::Measures(4, 2),
            },
        )
        .expect_err("refuses a backwards range");
        assert!(err.contains("backwards"), "{err}");
        let err = apply(
            sheet,
            &Op::Transpose {
                semitones: 1,
                steps: None,
                span: Span::Measures(0, 1),
            },
        )
        .expect_err("refuses measure 0");
        assert!(err.contains("from 1"), "{err}");
    }

    #[test]
    fn an_operation_reads_from_its_json_form() {
        let op: Op = serde_json::from_str(
            r#"{"op": "transpose", "semitones": -3, "span": {"measures": [3, 10]}}"#,
        )
        .expect("parses");
        assert_eq!(
            op,
            Op::Transpose {
                semitones: -3,
                steps: None,
                span: Span::Measures(3, 10),
            }
        );
        // and the catalog names exactly what the form accepts
        let spec = catalog().iter().find(|s| s.op == "transpose").unwrap();
        assert_eq!(spec.required, &["semitones"]);
        assert_eq!(spec.optional, &["steps", "span"]);
    }
}
