//! The voice -> MEI encoder: lay a monophonic-per-slot voice out into barred,
//! tied measures and wrap it in a minimal MEI document.
//!
//! A **voice** is one monophonic line — a note or chord at a time, back to back
//! — which is exactly one MEI `<layer>`. It is deliberately a **composable
//! primitive**, not a ceiling: full polyphony is *several* voices (and staves),
//! so the refinement pass composes voices above this encoder rather than
//! redefining the voice. This function is the single-voice case of that; a
//! polyphonic entry point would sit over it, not replace it.
//!
//! MEI is the target because it is explicit — every note spells its pitch
//! (pname/oct/accid) and value (dur/dots), with none of ABC's contextual traps
//! (accidentals persisting through a bar, spacing-driven beaming). No `xml:id`s
//! are emitted: verovio mints them on load, so id stability across editing is
//! unchanged.
//!
//! **Ticks live here and nowhere above.** The model counts in exact [`Ratio`]s;
//! this encoder is the boundary where a duration becomes MEI's `@dur` and
//! `@dots`, and `TPW` is the resolution *this* conversion works at, not a
//! foundation anything else rests on. A duration that does not land on that
//! grid is refused by name rather than snapped, because a triplet silently
//! rounded to a 32nd is a wrong score that looks right.
//!
//! Two seams stay deliberately narrow so the emission milestone extends rather
//! than rewrites them: the value decomposition ([`pieces`]) and the projection
//! of flat content onto the grid ([`sheet_to_mei`]).

use serde::Deserialize;

use super::model::{Grid, Item, Pitch, Sheet, Staff, Voice};
use crate::ratio::Ratio;

// 32nd-note resolution: every duration is an integer number of these, so
// barline splitting and tie decomposition are exact integer arithmetic.
const TPW: i32 = 32; // ticks per whole note

/// The MEI `@dur` note values, longest first, paired with the ticks each lasts:
/// whole(1)..32nd(32).
const VALUES: [(i32, i32); 6] = [
    (1, TPW),
    (2, TPW / 2),
    (4, TPW / 4),
    (8, TPW / 8),
    (16, TPW / 16),
    (32, TPW / 32),
];

/// A key name -> (MEI `key.sig`, prefer flats when spelling chromatic notes).
/// Anything unrecognized falls back to C major (`"0"`, sharps).
fn key_signature(key: &str) -> (&'static str, bool) {
    match key {
        "C" => ("0", false),
        "G" => ("1s", false),
        "D" => ("2s", false),
        "A" => ("3s", false),
        "E" => ("4s", false),
        "B" => ("5s", false),
        "F#" => ("6s", false),
        "C#" => ("7s", false),
        "F" => ("1f", true),
        "Bb" => ("2f", true),
        "Eb" => ("3f", true),
        "Ab" => ("4f", true),
        "Db" => ("5f", true),
        "Gb" => ("6f", true),
        "Cb" => ("7f", true),
        _ => ("0", false),
    }
}

/// One slot of a monophonic-per-slot voice: a note or chord (one or more MIDI
/// pitches) or a rest, lasting `ticks` 32nd-notes. This is the flat, agnostic
/// stream a client reduces its own sequencing data to; [`voice_to_mei`] lays it
/// out into barred, tied measures. A voice (a `&[Slot]`) is the composable
/// per-layer primitive — polyphony stacks several, it never widens the slot.
///
/// As JSON (what a binding sends) a slot is an object: `{"midis": [60, 64],
/// "ticks": 8}` is a chord, `{"ticks": 8}` a rest — a slot with no pitches *is*
/// a rest, which keeps the wire form total without a discriminator.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Slot {
    /// A note (one pitch) or chord (several), lasting `ticks`.
    Note { midis: Vec<i32>, ticks: i32 },
    /// A rest, lasting `ticks`.
    Rest { ticks: i32 },
}

/// Engrave a voice into a minimal MEI document, splitting notes across barlines
/// and tying the pieces.
///
/// The **v1 wire form**, kept as it was: `meter` is `"num/den"` (e.g. `"4/4"`),
/// `clef` is a shape+line like `"G2"`, `"F4"` or `"C3"`, and `key` selects the
/// key signature and sharp-vs-flat spelling (`key_signature`, private: a link
/// there resolves only in a build documenting private items). A duration that
/// is not a single note value is written as tied notes (a dotted value when
/// exact), and a note that overruns a barline is split and tied across it.
///
/// It is now a thin front door on [`voice_to_sheet`] + [`sheet_to_mei`]: the
/// slots become a one-staff, one-voice [`Sheet`] and the model is what is
/// written out. That is deliberate rather than tidy — it is the standing proof
/// that the model can represent everything the wire form could, since any
/// divergence shows up as a difference in these bytes.
pub fn voice_to_mei(voice: &[Slot], meter: &str, clef: &str, key: &str) -> String {
    // A voice that came in as ticks is on the tick grid by construction, and
    // one staff with one voice is what `voice_to_sheet` builds, so neither
    // refusal can fire here.
    sheet_to_mei(&voice_to_sheet(voice, meter, clef, key)).expect("a v1 voice is always writable")
}

/// Lift a v1 voice into the score model: the bridge between the wire form a
/// client already reduces to and the model everything above now speaks.
///
/// The [`Slot`] stays what it always was — a total, discriminator-free form
/// where a slot with no pitches *is* a rest — and this is where it stops being
/// the ceiling: ticks become exact durations, MIDI numbers become spelled
/// pitches (in the accidental world `key` implies, which is the only choice a
/// bare number leaves), and the `meter`/`clef`/`key` a caller used to pass at
/// every call become part of the sheet.
pub fn voice_to_sheet(voice: &[Slot], meter: &str, clef: &str, key: &str) -> Sheet {
    let (num, den) = parse_meter(meter);
    let (_, flats) = key_signature(key);
    let items = voice
        .iter()
        .map(|slot| match slot {
            Slot::Note { midis, ticks } if !midis.is_empty() => Item::Note {
                pitches: midis.iter().map(|&m| Pitch::from_midi(m, flats)).collect(),
                dur: Ratio::from_ticks(*ticks as i64, TPW as i64),
                tie: false,
                marks: Default::default(),
            },
            Slot::Note { ticks, .. } | Slot::Rest { ticks } => Item::Rest {
                dur: Ratio::from_ticks(*ticks as i64, TPW as i64),
            },
        })
        .collect();
    Sheet {
        grid: Grid::uniform(num as i64, den as i64),
        key: key.to_string(),
        staves: vec![Staff {
            clef: clef.to_string(),
            voices: vec![Voice { items }],
        }],
    }
}

/// Write a [`Sheet`] out as a minimal MEI document: project the flat content
/// onto the grid, splitting and tying across every barline the grid puts in the
/// way.
///
/// This is the **boundary where exact durations become note values**. A
/// duration that is not an integer count of 32nds — a triplet, which is exactly
/// what rationals exist to keep exact — is refused by name rather than snapped
/// onto the grid, and so is the polyphony the model can hold and this emitter
/// cannot yet write. Both refusals say which milestone owes the work, because a
/// caller reading "cannot" needs to know whether it is wrong or early.
pub fn sheet_to_mei(sheet: &Sheet) -> Result<String, String> {
    let (keysig, _) = key_signature(&sheet.key);
    let staff = match sheet.staves.as_slice() {
        [] => &Staff::default(),
        [one] => one,
        _ => {
            return Err("writing more than one staff is the emission milestone; \
                        this sheet has several"
                .to_string());
        }
    };
    let voice = match staff.voices.as_slice() {
        [] => &Voice::default(),
        [one] => one,
        _ => {
            return Err("writing more than one voice on a staff is the emission \
                        milestone; this staff has several"
                .to_string());
        }
    };
    let (shape, line) = parse_clef(&staff.clef);
    let meter = sheet.grid.meter_at(0);
    let (num, den) = (meter.count, meter.unit);

    // Each cell is a rendered `<note>`/`<rest>`/`<chord>` string.
    let mut measures: Vec<Vec<String>> = vec![Vec::new()];
    let mut pos = 0; // ticks into the current (last) measure
    let mut bar = bar_ticks(&sheet.grid, 0)?;
    // Whether the previous item tied into this one, so a tie the caller wrote
    // and a tie a barline forced compose instead of overwriting each other.
    let mut tied_in = false;
    for item in &voice.items {
        let total = ticks(item.dur())?;
        // (value, dots, measure_index) for every piece the item spans.
        let mut specs: Vec<(i32, i32, usize)> = Vec::new();
        let mut remaining = total;
        while remaining > 0 {
            if pos == bar {
                measures.push(Vec::new());
                pos = 0;
                bar = bar_ticks(&sheet.grid, measures.len() - 1)?;
            }
            let take = remaining.min(bar - pos);
            for (value, dots) in pieces(take) {
                specs.push((value, dots, measures.len() - 1));
            }
            pos += take;
            remaining -= take;
        }
        let sounds = !item.pitches().is_empty();
        let tied_out = matches!(item, Item::Note { tie: true, .. });
        let n = specs.len();
        for (idx, (value, dots, mi)) in specs.iter().copied().enumerate() {
            // A piece opens a tie when it is not the last of a split note, or
            // when the caller tied this item to the next; it closes one when it
            // is not the first, or when the previous item tied into this one.
            let opens = sounds && (idx + 1 < n || (idx + 1 == n && tied_out));
            let closes = sounds && (idx > 0 || (idx == 0 && tied_in));
            let tie = match (opens, closes) {
                (true, true) => Some("m"),
                (true, false) => Some("i"),
                (false, true) => Some("t"),
                (false, false) => None,
            };
            measures[mi].push(element(item.pitches(), value, dots, tie)?);
        }
        tied_in = tied_out && sounds;
    }

    if measures.iter().all(Vec::is_empty) {
        // An empty voice still needs a drawable bar of rests.
        measures[0] = pieces(bar_ticks(&sheet.grid, 0)?)
            .into_iter()
            .map(|(value, dots)| element(&[], value, dots, None))
            .collect::<Result<Vec<_>, _>>()?;
    }

    let last = measures.len() - 1;
    let body = measures
        .iter()
        .enumerate()
        .map(|(i, cells)| measure_xml(i, cells, i == last))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <mei xmlns=\"http://www.music-encoding.org/ns/mei\" meiversion=\"5.0\">\n\
         \x20<meiHead><fileDesc><titleStmt><title/></titleStmt>\
         <pubStmt/></fileDesc></meiHead>\n\
         \x20<music><body><mdiv><score>\n\
         \x20\x20<scoreDef meter.count=\"{num}\" meter.unit=\"{den}\" key.sig=\"{keysig}\">\n\
         \x20\x20\x20<staffGrp><staffDef n=\"1\" lines=\"5\" clef.shape=\"{shape}\"\
         \x20clef.line=\"{line}\"/></staffGrp>\n\
         \x20\x20</scoreDef>\n\
         \x20\x20<section>\n{body}\n\x20\x20</section>\n\
         \x20</score></mdiv></body></music>\n\
         </mei>\n"
    ))
}

/// A duration as an integer count of 32nds, or a refusal naming what it is that
/// the grid cannot hold. This is the one place the conversion happens.
fn ticks(dur: Ratio) -> Result<i32, String> {
    dur.as_ticks(TPW as i64)
        .and_then(|t| i32::try_from(t).ok())
        .ok_or_else(|| {
            format!(
                "the duration {dur} is not an exact number of 32nd notes, so it \
                 cannot be written as plain note values; tuplets are the emission \
                 milestone"
            )
        })
}

/// The length of one measure of the grid, in ticks.
fn bar_ticks(grid: &Grid, measure: usize) -> Result<i32, String> {
    ticks(grid.bar_len(measure))
}

/// Decompose a tick count (within one bar) into `(mei_dur, dots)` note values,
/// largest-first, to be tied. A count that is one plain or dotted value is that
/// single value; otherwise the largest value that fits is split off and the
/// remainder decomposed on.
fn pieces(mut ticks: i32) -> Vec<(i32, i32)> {
    if let Some(single) = single_value(ticks) {
        return vec![single];
    }
    let mut out = Vec::new();
    while ticks > 0 {
        if let Some(single) = single_value(ticks) {
            out.push(single);
            break;
        }
        for (value, vt) in VALUES {
            if vt <= ticks {
                out.push((value, 0));
                ticks -= vt;
                break;
            }
        }
    }
    out
}

/// `(mei_dur, dots)` if `ticks` is exactly one plain or single-dotted note
/// value, else `None`.
fn single_value(ticks: i32) -> Option<(i32, i32)> {
    for (value, vt) in VALUES {
        if ticks == vt {
            return Some((value, 0));
        }
        if vt % 2 == 0 && ticks == vt + vt / 2 {
            // dotted: 1.5x, and dottable (an even tick count)
            return Some((value, 1));
        }
    }
    None
}

fn measure_xml(index: usize, cells: &[String], last: bool) -> String {
    let right = if last { " right=\"end\"" } else { "" };
    let inner = cells.concat();
    format!(
        "   <measure n=\"{}\"{right}><staff n=\"1\"><layer n=\"1\">{inner}</layer></staff></measure>",
        index + 1
    )
}

fn element(pitches: &[Pitch], value: i32, dots: i32, tie: Option<&str>) -> Result<String, String> {
    let d = if dots != 0 { " dots=\"1\"" } else { "" };
    Ok(match pitches {
        // Nothing to sound draws as a rest, however the caller spelled it.
        [] => format!("<rest dur=\"{value}\"{d}/>"),
        [one] => note_xml(one, Some(value), dots, tie)?,
        many => {
            let inner = many
                .iter()
                .map(|p| note_xml(p, None, 0, tie))
                .collect::<Result<String, _>>()?;
            format!("<chord dur=\"{value}\"{d}>{inner}</chord>")
        }
    })
}

fn note_xml(
    pitch: &Pitch,
    value: Option<i32>,
    dots: i32,
    tie: Option<&str>,
) -> Result<String, String> {
    // A pitch already carries its spelling. Which accidental world a bare MIDI
    // number was spelled into was decided on the way in, before this point.
    let (pname, octave) = (pitch.step.pname(), pitch.octave);
    // MEI writes up to a double accidental. Anything past that is refused
    // rather than dropped: a triple sharp silently written as a natural is a
    // wrong score that looks right, which is the one failure this layer must
    // never produce.
    let accid = match pitch.alter {
        0 => "",
        1 => "s",
        -1 => "f",
        2 => "x",
        -2 => "ff",
        alter => {
            return Err(format!(
                "the pitch {pname}{octave} is altered by {alter} semitones, and \
                 MEI writes at most a double accidental; respell it"
            ));
        }
    };
    let mut head = match value {
        Some(v) => format!("<note dur=\"{v}\""),
        None => "<note".to_string(),
    };
    if dots != 0 {
        head.push_str(" dots=\"1\"");
    }
    head.push_str(&format!(" oct=\"{octave}\" pname=\"{pname}\""));
    if let Some(tie) = tie {
        head.push_str(&format!(" tie=\"{tie}\""));
    }
    Ok(if accid.is_empty() {
        format!("{head}/>")
    } else {
        format!("{head}><accid accid=\"{accid}\"/></note>")
    })
}

fn parse_meter(meter: &str) -> (i32, i32) {
    let (num, den) = meter.split_once('/').unwrap_or(("4", "4"));
    (
        num.trim().parse().unwrap_or(4),
        den.trim().parse().unwrap_or(4),
    )
}

fn parse_clef(clef: &str) -> (String, i32) {
    let shape = clef.get(..1).unwrap_or("G").to_uppercase();
    let line = clef.get(1..).and_then(|s| s.parse().ok()).unwrap_or(2);
    (shape, line)
}

#[cfg(test)]
mod tests {
    use super::super::model::Step;
    use super::*;

    fn note(midi: i32, ticks: i32) -> Slot {
        Slot::Note {
            midis: vec![midi],
            ticks,
        }
    }

    #[test]
    fn midi_spells_to_scientific_pitch_with_the_accidental_world() {
        // The spelling rule is the model's `Pitch::from_midi` and there is one
        // of it: this encoder used to carry a second copy of the same tables.
        let spelled = |midi, flats| {
            let p = Pitch::from_midi(midi, flats);
            (p.step.pname(), p.octave, p.alter)
        };
        assert_eq!(spelled(60, false), ("c", 4, 0)); // middle C
        assert_eq!(spelled(61, false), ("c", 4, 1)); // C#
        assert_eq!(spelled(66, false), ("f", 4, 1)); // F#
        assert_eq!(spelled(61, true), ("d", 4, -1)); // spelled Db
        assert_eq!(spelled(72, false), ("c", 5, 0)); // an octave up
    }

    #[test]
    fn a_duration_decomposes_into_tied_note_values() {
        // ticks: whole=32, half=16, quarter=8, eighth=4 (32nd-note resolution)
        assert_eq!(pieces(8), vec![(4, 0)]); // a quarter (one beat)
        assert_eq!(pieces(4), vec![(8, 0)]); // an eighth
        assert_eq!(pieces(16), vec![(2, 0)]); // a half
        assert_eq!(pieces(12), vec![(4, 1)]); // 1.5 beats -> a dotted quarter
        assert_eq!(pieces(20), vec![(2, 0), (8, 0)]); // 2.5 beats -> half + eighth
    }

    #[test]
    fn voice_to_mei_writes_a_monophonic_melody() {
        // 60 for 1 beat, 62 for 0.5, 64 for 1.5, a 1-beat rest, 65 for 1 beat,
        // at beat_unit 4 (a quarter = 8 ticks).
        let voice = vec![
            note(60, 8),
            note(62, 4),
            note(64, 12),
            Slot::Rest { ticks: 8 },
            note(65, 8),
        ];
        let mei = voice_to_mei(&voice, "4/4", "G2", "C");
        assert!(mei.contains("<rest")); // the rest
        assert!(mei.contains("dots=\"1\"")); // the dotted 1.5-beat note
        assert!(mei.contains("pname=\"c\"") && mei.contains("pname=\"e\""));
    }

    #[test]
    fn a_note_crossing_a_barline_splits_and_ties() {
        // 2 beats, then 3 beats starting on beat 2 of 4/4 (bar = 32 ticks): the
        // 3-beat note spans the barline and is written as two tied notes.
        let voice = vec![note(60, 16), note(67, 24)];
        let mei = voice_to_mei(&voice, "4/4", "G2", "C");
        assert!(mei.contains("tie=\"i\"") && mei.contains("tie=\"t\""));
        // two measures: the split note ends the first and opens the second
        assert_eq!(mei.matches("<measure").count(), 2);
    }

    #[test]
    fn a_chord_stacks_notes_under_one_value() {
        let voice = vec![Slot::Note {
            midis: vec![60, 64, 67],
            ticks: 8,
        }];
        let mei = voice_to_mei(&voice, "4/4", "G2", "C");
        assert!(mei.contains("<chord dur=\"4\">"));
        assert_eq!(mei.matches("<note").count(), 3);
    }

    #[test]
    fn an_empty_voice_still_draws_a_bar_of_rests() {
        let mei = voice_to_mei(&[], "4/4", "G2", "C");
        // a full 4/4 bar of rest is one whole rest
        assert!(mei.contains("<rest dur=\"1\"/>"));
        assert_eq!(mei.matches("<measure").count(), 1);
    }

    #[test]
    fn a_voice_deserializes_from_the_wire_form() {
        let voice: Vec<Slot> = serde_json::from_str(
            r#"[{"midis": [60, 64], "ticks": 8}, {"ticks": 8}, {"midis": [], "ticks": 8}]"#,
        )
        .expect("parses");
        assert_eq!(
            voice[0],
            Slot::Note {
                midis: vec![60, 64],
                ticks: 8
            }
        );
        assert_eq!(voice[1], Slot::Rest { ticks: 8 });
        // A pitchless slot is a rest however it was spelled, so it draws as one.
        let mei = voice_to_mei(&voice[2..], "4/4", "G2", "C");
        assert!(mei.contains("<rest dur=\"4\"/>"), "a quarter rest");
        assert!(!mei.contains("<chord"), "never an empty chord");
    }

    #[test]
    fn what_the_model_can_hold_and_mei_cannot_write_is_refused_by_name() {
        use super::super::model::{Marks, Staff, Voice};
        let note = |alter, dur| Item::Note {
            pitches: vec![Pitch {
                step: Step::C,
                alter,
                octave: 4,
            }],
            dur,
            tie: false,
            marks: Marks::default(),
        };
        let sheet_of = |items| Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice { items }],
            }],
            ..Default::default()
        };

        // A triplet eighth is exact in the model and has no note value here.
        let err = sheet_to_mei(&sheet_of(vec![note(0, Ratio::new(1, 12))]))
            .expect_err("refuses a tuplet");
        assert!(err.contains("1/12") && err.contains("tuplet"), "{err}");

        // A triple sharp is data the model can hold and MEI cannot spell.
        let err =
            sheet_to_mei(&sheet_of(vec![note(3, Ratio::new(1, 4))])).expect_err("refuses a triple");
        assert!(err.contains("double accidental"), "{err}");

        // And two voices are representable long before they are writable.
        let two = Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice::default(), Voice::default()],
            }],
            ..Default::default()
        };
        let err = sheet_to_mei(&two).expect_err("refuses polyphony");
        assert!(err.contains("emission milestone"), "{err}");
    }

    /// The six cases below are the **byte-for-byte** record of what this encoder
    /// wrote before the score model existed, captured from the previous commit
    /// and compared against what it writes now that `voice_to_mei` builds a
    /// `Sheet` and emits *that*. It is the acceptance the model rests on: the
    /// model can represent everything the v1 wire form could, and the proof is
    /// that not one byte moves. A diff here is either a real change to the
    /// engraving (which has to be deliberate and re-recorded) or the model
    /// losing something on the way through.
    #[test]
    fn the_model_writes_the_bytes_the_wire_form_always_wrote() {
        let cases: Vec<(&str, Vec<Slot>, &str, &str, &str)> = vec![
            (
                "melody",
                vec![
                    note(60, 8),
                    note(62, 4),
                    note(64, 12),
                    Slot::Rest { ticks: 8 },
                    note(65, 8),
                ],
                "4/4",
                "G2",
                "C",
            ),
            ("split", vec![note(60, 16), note(67, 24)], "4/4", "G2", "C"),
            (
                "chord",
                vec![Slot::Note {
                    midis: vec![60, 64, 67],
                    ticks: 8,
                }],
                "4/4",
                "G2",
                "C",
            ),
            ("empty", vec![], "4/4", "G2", "C"),
            ("flats", vec![note(61, 8), note(66, 8)], "3/4", "F4", "Bb"),
            // an odd meter and durations that do not fit its bar, so the split
            // and the decomposition both have to land where they always did
            ("odd", vec![note(60, 5), note(62, 27)], "7/8", "C3", "F#"),
        ];
        let mut out = String::new();
        for (name, voice, meter, clef, key) in cases {
            out.push_str(&format!(
                "=== {name}\n{}",
                voice_to_mei(&voice, meter, clef, key)
            ));
        }
        assert_eq!(out, include_str!("testdata/voice_to_mei.txt"));
    }

    #[test]
    fn the_clef_and_key_reach_the_score_definition() {
        let mei = voice_to_mei(&[note(60, 8)], "3/4", "F4", "Bb");
        assert!(mei.contains("meter.count=\"3\" meter.unit=\"4\""));
        assert!(mei.contains("key.sig=\"2f\""));
        assert!(mei.contains("clef.shape=\"F\" clef.line=\"4\""));
    }
}
