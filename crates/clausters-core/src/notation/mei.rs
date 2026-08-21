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
//! Two seams are kept deliberately narrow so the engraving-refinements work can
//! extend rather than rewrite them: the pitch spelling ([`spell`]) and the
//! beats->written-value step ([`pieces`]). This encoder reads only the written
//! duration a caller already reduced to ticks; performance nuance, tuplets and
//! full polyphony are that later pass.

use serde::Deserialize;

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

// Chromatic spelling: pitch-class -> (pname, accid), one table per accidental
// world. `accid` is "" (natural, no <accid> child), "s" (sharp) or "f" (flat).
const SHARP: [(&str, &str); 12] = [
    ("c", ""),
    ("c", "s"),
    ("d", ""),
    ("d", "s"),
    ("e", ""),
    ("f", ""),
    ("f", "s"),
    ("g", ""),
    ("g", "s"),
    ("a", ""),
    ("a", "s"),
    ("b", ""),
];
const FLAT: [(&str, &str); 12] = [
    ("c", ""),
    ("d", "f"),
    ("d", ""),
    ("e", "f"),
    ("e", ""),
    ("f", ""),
    ("g", "f"),
    ("g", ""),
    ("a", "f"),
    ("a", ""),
    ("b", "f"),
    ("b", ""),
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

impl Slot {
    fn ticks(&self) -> i32 {
        match self {
            Slot::Note { ticks, .. } | Slot::Rest { ticks } => *ticks,
        }
    }
    /// Whether this slot draws pitches. A `Note` with no pitches is a rest — the
    /// wire form has no discriminator, so the emptiness is what says so.
    fn is_note(&self) -> bool {
        matches!(self, Slot::Note { midis, .. } if !midis.is_empty())
    }
}

/// Engrave a voice into a minimal MEI document, splitting notes across barlines
/// and tying the pieces.
///
/// `meter` is `"num/den"` (e.g. `"4/4"`), `clef` is a shape+line like `"G2"`,
/// `"F4"` or `"C3"`, and `key` selects the key signature and sharp-vs-flat
/// spelling (`key_signature`, private: a link there resolves only in a build
/// documenting private items). A duration that is not a single note value
/// is written as tied notes (a dotted value when exact), and a note that
/// overruns a barline is split and tied across it.
pub fn voice_to_mei(voice: &[Slot], meter: &str, clef: &str, key: &str) -> String {
    let (num, den) = parse_meter(meter);
    let bar = num * TPW / den; // ticks per measure
    let (keysig, flats) = key_signature(key);
    let (shape, line) = parse_clef(clef);

    // Each cell is a rendered `<note>`/`<rest>`/`<chord>` string.
    let mut measures: Vec<Vec<String>> = vec![Vec::new()];
    let mut pos = 0; // ticks into the current (last) measure
    for slot in voice {
        // (value, dots, measure_index) for every piece the slot spans.
        let mut specs: Vec<(i32, i32, usize)> = Vec::new();
        let mut remaining = slot.ticks();
        while remaining > 0 {
            if pos == bar {
                measures.push(Vec::new());
                pos = 0;
            }
            let take = remaining.min(bar - pos);
            for (value, dots) in pieces(take) {
                specs.push((value, dots, measures.len() - 1));
            }
            pos += take;
            remaining -= take;
        }
        let n = specs.len();
        for (idx, (value, dots, mi)) in specs.iter().copied().enumerate() {
            // A split note ties its pieces: initial / medial / terminal.
            let tie = if slot.is_note() && n > 1 {
                Some(if idx == 0 {
                    "i"
                } else if idx == n - 1 {
                    "t"
                } else {
                    "m"
                })
            } else {
                None
            };
            measures[mi].push(element(slot, value, dots, tie, flats));
        }
    }

    if measures.iter().all(Vec::is_empty) {
        // An empty voice still needs a drawable bar of rests.
        let rest = Slot::Rest { ticks: 0 };
        measures[0] = pieces(bar)
            .into_iter()
            .map(|(value, dots)| element(&rest, value, dots, None, flats))
            .collect();
    }

    let last = measures.len() - 1;
    let body = measures
        .iter()
        .enumerate()
        .map(|(i, cells)| measure_xml(i, cells, i == last))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
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
    )
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

fn element(slot: &Slot, value: i32, dots: i32, tie: Option<&str>, flats: bool) -> String {
    let d = if dots != 0 { " dots=\"1\"" } else { "" };
    match slot {
        // A pitchless slot draws as a rest either way it was spelled.
        Slot::Rest { .. } => format!("<rest dur=\"{value}\"{d}/>"),
        Slot::Note { midis, .. } if midis.is_empty() => format!("<rest dur=\"{value}\"{d}/>"),
        Slot::Note { midis, .. } if midis.len() == 1 => {
            note_xml(midis[0], Some(value), dots, tie, flats)
        }
        Slot::Note { midis, .. } => {
            let inner: String = midis
                .iter()
                .map(|&m| note_xml(m, None, 0, tie, flats))
                .collect();
            format!("<chord dur=\"{value}\"{d}>{inner}</chord>")
        }
    }
}

fn note_xml(midi: i32, value: Option<i32>, dots: i32, tie: Option<&str>, flats: bool) -> String {
    let (pname, octave, accid) = spell(midi, flats);
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
    if accid.is_empty() {
        format!("{head}/>")
    } else {
        format!("{head}><accid accid=\"{accid}\"/></note>")
    }
}

/// A MIDI note -> `(pname, octave, accid)` in scientific pitch (60 -> c4).
fn spell(midi: i32, flats: bool) -> (&'static str, i32, &'static str) {
    let table = if flats { &FLAT } else { &SHARP };
    let (pname, accid) = table[midi.rem_euclid(12) as usize];
    (pname, midi.div_euclid(12) - 1, accid)
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
    use super::*;

    fn note(midi: i32, ticks: i32) -> Slot {
        Slot::Note {
            midis: vec![midi],
            ticks,
        }
    }

    #[test]
    fn midi_spells_to_scientific_pitch_with_the_accidental_world() {
        assert_eq!(spell(60, false), ("c", 4, "")); // middle C
        assert_eq!(spell(61, false), ("c", 4, "s")); // C#
        assert_eq!(spell(66, false), ("f", 4, "s")); // F#
        assert_eq!(spell(61, true), ("d", 4, "f")); // spelled Db
        assert_eq!(spell(72, false), ("c", 5, "")); // an octave up
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
    fn the_clef_and_key_reach_the_score_definition() {
        let mei = voice_to_mei(&[note(60, 8)], "3/4", "F4", "Bb");
        assert!(mei.contains("meter.count=\"3\" meter.unit=\"4\""));
        assert!(mei.contains("key.sig=\"2f\""));
        assert!(mei.contains("clef.shape=\"F\" clef.line=\"4\""));
    }
}
