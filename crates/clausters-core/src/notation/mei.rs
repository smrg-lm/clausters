//! The voice -> MEI encoder: lay a monophonic-per-slot voice out into barred,
//! tied measures and wrap it in a minimal MEI document.
//!
//! A **voice** is one monophonic line -- a note or chord at a time, back to back
//! -- which is exactly one MEI `<layer>`. It is deliberately a **composable
//! primitive**, not a ceiling: full polyphony is *several* voices (and staves),
//! so the refinement pass composes voices above this encoder rather than
//! redefining the voice. This function is the single-voice case of that; a
//! polyphonic entry point would sit over it, not replace it.
//!
//! MEI is the target because it is explicit -- every note spells its pitch
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
//! than rewrites them: the value decomposition ([`parts`]) and the projection
//! of flat content onto the grid ([`sheet_to_mei`]).

use serde::Deserialize;

use super::model::{Grid, Header, Item, Marks, Pitch, Sheet, Staff, Step, Voice};
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

/// The keys a signature names, by their tonic.
pub const KEYS: [&str; 15] = [
    "C", "G", "D", "A", "E", "B", "F#", "C#", "F", "Bb", "Eb", "Ab", "Db", "Gb", "Cb",
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
/// per-layer primitive -- polyphony stacks several, it never widens the slot.
///
/// As JSON (what a binding sends) a slot is an object: `{"midis": [60, 64],
/// "ticks": 8}` is a chord, `{"ticks": 8}` a rest -- **a slot with no pitches
/// *is* a rest**, which keeps the wire form total without a discriminator. The
/// model says the same thing with two variants, because a model is not a wire
/// and can be exact; here totality is worth more, and unknown keys are refused
/// so that a misspelt mark is an error rather than a silent rest.
///
/// # What a slot may carry beyond a pitch and a value
///
/// Every field below is optional and every one is **a musical fact, not an
/// instruction to the encoder** -- the same rule [`Marks`] states, and the
/// reason these can be read back by the interpreter rather than only written.
/// A slot carrying none of them produces exactly the item it always did.
///
/// | field | what it says |
/// |---|---|
/// | `articulations` | `["stacc"]`, `["ten", "acc"]` -- MEI's own names |
/// | `dynamic` | a dynamic written at this note, governing the ones after it |
/// | `ornament` | `trill`, `mordent`, `turn`, `fermata` |
/// | `grace` | that this is a grace note: `acc`, `unacc` |
/// | `stem` | a stem direction the writer forced: `up`, `down` |
/// | `sounding` | how long it is *held*, in ticks, when no symbol says it |
/// | `spelling` | `sharp` or `flat`: which enharmonic, against the key's own |
/// | `accidental` | `written` for a sign to be printed, else `sounding` |
/// | `tie` | that it ties into the next slot |
///
/// Marks are a **note's**: a slot with no pitches ignores them, because a rest
/// carries none in the model either.
///
/// What a slot deliberately cannot say is anything that is not one note's:
/// a slur, a hairpin, a meter change, a barline or a title span notes or the
/// document, and they are written *beside* the voice -- with the model's own
/// verbs, on the sheet this builds. The **nth slot becomes the item with id
/// `n + 1`**, which is what makes that addressable from the client's own
/// indices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    /// The MIDI pitches sounding: one for a note, several for a chord, none
    /// for a rest.
    #[serde(default)]
    pub midis: Vec<i32>,
    /// How long it lasts, in 32nd-notes.
    pub ticks: i32,
    /// Articulations, by their MEI names (`stacc`, `acc`, `ten`, `marc`, ...).
    #[serde(default)]
    pub articulations: Vec<String>,
    /// A dynamic written at this note (`pp`, `mf`, `ff`, ...).
    #[serde(default)]
    pub dynamic: Option<String>,
    /// An ornament on this note (`trill`, `mordent`, `turn`, `fermata`).
    #[serde(default)]
    pub ornament: Option<String>,
    /// That this is a grace note, and of which kind (`acc`, `unacc`).
    #[serde(default)]
    pub grace: Option<String>,
    /// A stem direction the writer forced (`up`, `down`).
    #[serde(default)]
    pub stem: Option<String>,
    /// How long the note is **held**, in ticks, when that is not its written
    /// value and no articulation already says so.
    #[serde(default)]
    pub sounding: Option<i32>,
    /// Which enharmonic spelling to give the altered pitches -- `"sharp"` or
    /// `"flat"` -- against the world the key implies. A bare MIDI number
    /// cannot say it, and this is the only thing about it a *number* can:
    /// a caller who knows the letter writes the pitch itself, with
    /// [`Op::SetPitches`](super::Op::SetPitches) on the sheet.
    #[serde(default)]
    pub spelling: Option<String>,
    /// `"written"` where the accidental is to be printed even though the key
    /// or the measure already implies it -- a courtesy sign. `"sounding"`, or
    /// left out, leaves the decision to the engraver.
    #[serde(default)]
    pub accidental: Option<String>,
    /// That this note ties into the next slot: one sound across both values.
    #[serde(default)]
    pub tie: bool,
    /// **The pitches as they are written** -- the `pitches` notation key.
    /// Where a slot has them they are the note, letter and accidental as its
    /// writer chose, and `midis`, `spelling` and `accidental` are not asked:
    /// those spell a number, and this is not one.
    #[serde(default)]
    pub pitches: Vec<Pitch>,
    /// **The written value** -- the `value` notation key -- where `ticks`
    /// cannot say it: a triplet eighth is a twelfth of a whole note, which is
    /// no count of 32nds.
    #[serde(default)]
    pub value: Option<Ratio>,
    /// The staff and the voice the event names (the `staff` and `voice`
    /// notation keys). A voice built from slots is one line on one staff, so
    /// they place nothing here: they are read past, which lets a client hand
    /// an event's notation keys over whole, and they are what a caller splits
    /// its events by before it builds each voice.
    #[serde(default)]
    pub staff: Option<usize>,
    #[serde(default)]
    pub voice: Option<usize>,
}

impl Slot {
    /// A note or chord of `ticks` 32nd-notes, carrying no marks.
    pub fn note(midis: Vec<i32>, ticks: i32) -> Slot {
        Slot {
            midis,
            ticks,
            ..Default::default()
        }
    }

    /// A rest of `ticks` 32nd-notes.
    pub fn rest(ticks: i32) -> Slot {
        Slot {
            ticks,
            ..Default::default()
        }
    }

    /// The marks this slot puts on its note, in the model's own terms -- ticks
    /// become an exact [`Ratio`], and everything else is carried verbatim.
    fn marks(&self) -> Marks {
        Marks {
            articulations: self.articulations.clone(),
            dynamic: self.dynamic.clone(),
            ornament: self.ornament.clone(),
            grace: self.grace.clone(),
            stem: self.stem.clone(),
            sounding: self
                .sounding
                .map(|t| Ratio::from_ticks(t as i64, TPW as i64)),
            ..Marks::default()
        }
    }
}

/// Engrave a voice into a minimal MEI document, splitting notes across barlines
/// and tying the parts.
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
/// written out. That is deliberate rather than tidy -- it is the standing proof
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
/// The [`Slot`] stays what it always was -- a total, discriminator-free form
/// where a slot with no pitches *is* a rest -- and this is where it stops being
/// the ceiling: ticks become exact durations, MIDI numbers become spelled
/// pitches (in the accidental world `key` implies, which is the only choice a
/// bare number leaves), and the `meter`/`clef`/`key` a caller used to pass at
/// every call become part of the sheet.
pub fn voice_to_sheet(voice: &[Slot], meter: &str, clef: &str, key: &str) -> Sheet {
    let (num, den) = parse_meter(meter);
    let (_, key_flats) = key_signature(key);
    let items = voice
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            let id = i as u64 + 1;
            // the written value where the slot states it, else its ticks
            let dur = slot
                .value
                .filter(Ratio::is_positive)
                .unwrap_or_else(|| Ratio::from_ticks(slot.ticks as i64, TPW as i64));
            if slot.midis.is_empty() && slot.pitches.is_empty() {
                return Item::Rest { id, dur };
            }
            // written pitches are the note as it stands: nothing is spelled
            if !slot.pitches.is_empty() {
                return Item::Note {
                    id,
                    pitches: slot.pitches.clone(),
                    dur,
                    tie: slot.tie,
                    marks: slot.marks(),
                };
            }
            // Which accidental world this note is spelled into: the key's,
            // unless the slot chose one for itself.
            let flats = match slot.spelling.as_deref() {
                Some("flat") => true,
                Some("sharp") => false,
                _ => key_flats,
            };
            let forced = slot.accidental.as_deref() == Some("written");
            Item::Note {
                id,
                pitches: slot
                    .midis
                    .iter()
                    .map(|&m| Pitch {
                        forced,
                        ..Pitch::from_midi(m, flats)
                    })
                    .collect(),
                dur,
                tie: slot.tie,
                marks: slot.marks(),
            }
        })
        .collect();
    Sheet {
        next_id: voice.len() as u64 + 1,
        grid: Grid::uniform(num as i64, den as i64),
        key: key.to_string(),
        header: Header::default(),
        staves: vec![Staff {
            clef: clef.to_string(),
            voices: vec![Voice { items }],
            ..Staff::default()
        }],
        ..Sheet::default()
    }
}

/// Write a [`Sheet`] out as a minimal MEI document: project the flat content
/// onto the grid, splitting and tying across every barline the grid puts in the
/// way.
///
/// This is the **boundary where exact durations become note values**, and where
/// the two structures the model keeps apart are put back together: MEI nests
/// (`<measure><staff><layer>`) and the model does not, so every voice is
/// projected onto the same measures and the measures are assembled from the
/// projections.
///
/// **Every element carries the id of the item it was written from** -- the
/// model's own, not one the engraver minted. That is what lets a gesture on the
/// page name a note in the model, and what keeps a selection across a
/// re-engraving: an item split across a barline writes `n7`, `n7-2`, and the
/// pitches of a chord `n7-p1`, `n7-p2`, so every drawn thing maps back to the
/// one item it belongs to.
///
/// What it still refuses, by name: a tuplet that would cross a barline (which
/// cannot be split without ceasing to be a tuplet), a group whose written
/// values do not add up to a value at all, and an accidental past a double.
pub fn sheet_to_mei(sheet: &Sheet) -> Result<String, String> {
    Ok(documents(sheet, false)?.remove(0))
}

/// **The score cut at its written page breaks**: one document per run of
/// pages, each the measures from one page break to the next -- what a page
/// view lays out one by one, since the engraver turns a page where the paper
/// is full and nowhere else. The measures keep their own numbers and every
/// element its id, so a run's pages name what the whole score names. A run
/// after the first opens with the meter in force there, and its first page is
/// not the score's: it carries only what is written on every page, and the
/// page number ([`super::pagetext::PAGE_NUMBER`]), which the engraver counts
/// from one in each document. A score with no page break is one document,
/// the one [`sheet_to_mei`] writes.
///
/// # Errors
/// As [`sheet_to_mei`].
pub fn sheet_to_mei_pages(sheet: &Sheet) -> Result<Vec<String>, String> {
    documents(sheet, true)
}

/// The score as one document, or -- `split` -- as one per run of pages.
fn documents(sheet: &Sheet, split: bool) -> Result<Vec<String>, String> {
    let default_staff = Staff::default();
    let staves: Vec<&Staff> = if sheet.staves.is_empty() {
        vec![&default_staff]
    } else {
        sheet.staves.iter().collect()
    };
    let grid = &sheet.grid;

    // How many measures the longest voice needs; at least one, so an empty
    // score still draws a bar of rests.
    let count = measure_count(sheet)?;
    // Which accidentals are drawn, and which measure each item falls in: both
    // are questions about a whole staff, so both are answered before any voice
    // is projected.
    let (printed, placed) = layout(sheet);
    // where a run of pages starts: the first measure, and each one a page
    // break was written before
    let mut starts = vec![0];
    if split {
        starts.extend(
            grid.breaks
                .iter()
                .filter(|(m, kind)| kind == "page" && *m > 0 && *m < count)
                .map(|(m, _)| *m),
        );
        starts.sort_unstable();
        starts.dedup();
    }
    let (mut attached, timed) = attachments(sheet, &placed, &starts, count)?;
    for (m, kind) in &grid.marks {
        let xml = repeat_mark_xml(grid, *m, kind)?;
        attached.entry(*m).or_default().push_str(&xml);
    }
    let words = words_of(sheet);
    let ftrem: std::collections::HashSet<u64> = sheet
        .spanners
        .iter()
        .filter(|s| s.kind == "ftrem")
        .map(|s| s.from)
        .collect();

    // [staff][voice][measure] -> the rendered elements of that cell.
    let mut projected: Vec<Vec<Vec<Vec<String>>>> = Vec::new();
    for staff in &staves {
        let default_voice = Voice::default();
        let voices: Vec<&Voice> = if staff.voices.is_empty() {
            vec![&default_voice]
        } else {
            staff.voices.iter().collect()
        };
        let mut per_voice = Vec::new();
        for (v, voice) in voices.into_iter().enumerate() {
            let clefs: &[(Ratio, String)] = if v == 0 { &staff.clefs } else { &[] };
            let mut cells = project(voice, grid, count, &printed, v == 0, clefs, &ftrem, &words)?;
            beam(&mut cells, voice, sheet)?;
            per_voice.push(cells);
        }
        projected.push(per_voice);
    }

    // **Each measure, or a run of empty ones as one numbered rest**: where
    // runs are condensed, a measure with nothing written on any staff, no
    // change, no mark and nothing hanging off it is taken into the run it
    // continues -- and a run of one is the measure it was.
    let empty = |m: usize| {
        !attached.contains_key(&m)
            && !grid.repeats.contains(&m)
            && projected.iter().all(|voices| {
                voices.iter().all(|cells| {
                    cells.get(m).is_none_or(|cell| {
                        cell.iter().all(|e| {
                            !["<note", "<chord", "<beatRpt", "<clef"]
                                .iter()
                                .any(|tag| e.contains(tag))
                        })
                    })
                })
            })
    };
    let quiet_inside = |m: usize| {
        !starts.contains(&m)
            && !grid.breaks.iter().any(|(b, _)| *b == m)
            && !grid.meters.iter().any(|meter| meter.measure == m)
            && !grid.keys.iter().any(|(k, _)| *k == m)
            && !grid.endings.iter().any(|(a, b, _)| (*a..=*b).contains(&m))
            && grid.bar_len(m) == grid.bar_len(m - 1)
    };
    // (first measure, how many it stands for)
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut m = 0;
    while m < count {
        let mut k = 1;
        if grid.multirests && empty(m) {
            while m + k < count
                && empty(m + k)
                && quiet_inside(m + k)
                && !grid.barlines.iter().any(|(b, _)| *b == m + k - 1)
            {
                k += 1;
            }
        }
        pieces.push((m, k));
        m += k;
    }
    // a line that ends at a beat says how many measures on that is, and a
    // run of empty ones written as one numbered rest is one
    for line in timed {
        let across = pieces
            .iter()
            .filter(|(at, _)| *at > line.measure && *at <= line.to)
            .count();
        attached.entry(line.measure).or_default().push_str(&format!(
            "{} tstamp2=\"{across}m+{}\"{}",
            line.head, line.beat, line.tail
        ));
    }
    let piece_xml = |first: usize, k: usize, run_start: usize| -> String {
        // a change of meter or key stands before the measure it starts at,
        // except where a run of pages opens on it and says it there
        let def = if first > 0 && first != run_start {
            change_xml(sheet, first)
        } else {
            String::new()
        };
        let extra = attached.get(&first).map(String::as_str).unwrap_or("");
        if k > 1 {
            multirest_xml(first, k, staves.len(), grid, def, first + k == count)
        } else {
            measure_xml(first, &projected, extra, first + 1 == count, grid, &def)
        }
    };

    let groups = staff_groups(sheet, &staves);
    // whether empty measures are drawn as numbered rests is the section's
    // way of being drawn, kept where there is no run to show it
    let section = if grid.multirests {
        " type=\"multirests\""
    } else {
        ""
    };
    let head = header_xml(&sheet.header);
    let page = sheet.page.as_ref().map(page_attrs).unwrap_or_default();
    let ends = starts.iter().skip(1).copied().chain([count]);
    Ok(starts
        .iter()
        .zip(ends)
        .map(|(&first, end)| {
            let running = if first == 0 {
                super::pagetext::running_xml(&sheet.header, escape)
            } else {
                super::pagetext::continued_xml(&sheet.header, escape)
            };
            let meter = grid.meter_at(first);
            let (num, den) = (meter.count, meter.unit);
            let (keysig, _) = key_signature(sheet.key_at(first));
            // the run's measures, the endings wrapped around theirs
            let mut body: Vec<String> = Vec::new();
            for &(at, k) in pieces.iter().filter(|(at, _)| (first..end).contains(at)) {
                let mut xml = piece_xml(at, k, first);
                if let Some((_, _, label)) = grid.endings.iter().find(|(a, _, _)| *a == at) {
                    xml = format!(
                        "   <ending xml:id=\"e{}\" n=\"{}\">\n{xml}",
                        at + 1,
                        escape(label)
                    );
                }
                if grid.endings.iter().any(|(_, b, _)| *b >= at && *b < at + k) {
                    xml.push_str("\n   </ending>");
                }
                body.push(xml);
            }
            let body = body.join("\n");
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <mei xmlns=\"http://www.music-encoding.org/ns/mei\" meiversion=\"5.0\">\n\
                 \x20<meiHead><fileDesc><titleStmt>{head}</titleStmt>\
                 <pubStmt/></fileDesc></meiHead>\n\
                 \x20<music><body><mdiv><score>\n\
                 \x20\x20<scoreDef meter.count=\"{num}\" meter.unit=\"{den}\" key.sig=\"{keysig}\"{page}>\n\
                 \x20\x20\x20{groups}{running}\n\
                 \x20\x20</scoreDef>\n\
                 \x20\x20<section{section}>\n{body}\n\x20\x20</section>\n\
                 \x20</score></mdiv></body></music>\n\
                 </mei>\n"
            )
        })
        .collect())
}

/// **The staves' definitions, grouped**: each staff's clef, line count, name
/// and transposition, inside the groups the sheet writes -- or, with none
/// written, the one a single staff and a brace over several have always had.
fn staff_groups(sheet: &Sheet, staves: &[&Staff]) -> String {
    let def = |i: usize, staff: &Staff| {
        let (shape, line) = parse_clef(&staff.clef);
        let lines = staff.lines.unwrap_or(5);
        let trans = if staff.transpose != 0 {
            format!(
                " trans.semi=\"{}\" trans.diat=\"{}\"",
                staff.transpose,
                super::ops::default_steps(staff.transpose)
            )
        } else {
            String::new()
        };
        let mut names = String::new();
        if !staff.label.is_empty() {
            names.push_str(&format!("<label>{}</label>", escape(&staff.label)));
        }
        if !staff.abbr.is_empty() {
            names.push_str(&format!("<labelAbbr>{}</labelAbbr>", escape(&staff.abbr)));
        }
        let head = format!(
            "<staffDef n=\"{}\" lines=\"{lines}\" clef.shape=\"{shape}\" clef.line=\"{line}\"{trans}",
            i + 1
        );
        if names.is_empty() {
            format!("{head}/>")
        } else {
            format!("{head}>{names}</staffDef>")
        }
    };
    if sheet.groups.is_empty() {
        let defs: String = staves.iter().enumerate().map(|(i, s)| def(i, s)).collect();
        // A single staff keeps the shape it always had; several take a brace,
        // which is what makes two staves read as one instrument rather than
        // two.
        return if staves.len() > 1 {
            format!("<staffGrp symbol=\"brace\" bar.thru=\"true\">{defs}</staffGrp>")
        } else {
            format!("<staffGrp>{defs}</staffGrp>")
        };
    }
    let mut out = String::new();
    for (i, staff) in staves.iter().enumerate() {
        for group in sheet.groups.iter().filter(|g| g.first == i) {
            out.push_str(&format!(
                "<staffGrp symbol=\"{}\" bar.thru=\"true\">",
                escape(&group.symbol)
            ));
        }
        out.push_str(&def(i, staff));
        for _ in sheet.groups.iter().filter(|g| g.last == i) {
            out.push_str("</staffGrp>");
        }
    }
    format!("<staffGrp>{out}</staffGrp>")
}

/// What changes at `measure` -- the meter, the key -- as a score definition
/// standing before it, or nothing.
fn change_xml(sheet: &Sheet, measure: usize) -> String {
    let mut attrs = String::new();
    if let Some(meter) = sheet.grid.meters.iter().find(|m| m.measure == measure) {
        attrs.push_str(&format!(
            " meter.count=\"{}\" meter.unit=\"{}\"",
            meter.count, meter.unit
        ));
    }
    if sheet.grid.keys.iter().any(|(m, _)| *m == measure) {
        let (keysig, _) = key_signature(sheet.key_at(measure));
        attrs.push_str(&format!(" key.sig=\"{keysig}\""));
    }
    if attrs.is_empty() {
        String::new()
    } else {
        format!("<scoreDef{attrs}/>")
    }
}

/// A navigation mark of `measure`, as MEI writes it: at the measure's start
/// for a sign to come back to, at its end for an instruction to go.
fn repeat_mark_xml(grid: &Grid, measure: usize, kind: &str) -> Result<String, String> {
    let end = grid.meter_at(measure).count.max(1) as f64 + 0.5;
    let (func, tstamp, text) = match kind {
        "segno" => ("segno", 1.0, ""),
        "coda" => ("coda", 1.0, ""),
        "fine" => ("fine", end, "Fine"),
        "dacapo" => ("daCapo", end, "D.C."),
        "dalsegno" => ("dalSegno", end, "D.S."),
        "tocoda" => ("coda", end, "To Coda"),
        other => {
            return Err(format!(
                "there is no navigation mark called {other}; it is one of {}",
                REPEAT_MARKS.join(", ")
            ));
        }
    };
    let label = if kind == "tocoda" {
        " label=\"tocoda\""
    } else {
        ""
    };
    Ok(format!(
        "<repeatMark staff=\"1\" tstamp=\"{tstamp}\" func=\"{func}\" place=\"above\"{label}>{text}</repeatMark>"
    ))
}

/// The navigation marks a measure can carry.
pub const REPEAT_MARKS: [&str; 6] = ["segno", "coda", "fine", "dacapo", "dalsegno", "tocoda"];

/// `k` empty measures from `first` as **one numbered rest**, on every staff.
fn multirest_xml(
    first: usize,
    k: usize,
    staves: usize,
    grid: &Grid,
    def: String,
    last: bool,
) -> String {
    let lastm = first + k - 1;
    let right = match grid.barlines.iter().find(|(m, _)| *m == lastm) {
        Some((_, kind)) => format!(" right=\"{kind}\""),
        None if last => " right=\"end\"".to_string(),
        None => String::new(),
    };
    let brk = match grid.breaks.iter().find(|(m, _)| *m == first) {
        Some((_, kind)) if kind == "page" => "<pb/>",
        Some(_) => "<sb/>",
        None => "",
    };
    let body: String = (0..staves)
        .map(|si| {
            format!(
                "<staff xml:id=\"m{}s{}\" n=\"{}\"><layer n=\"1\"><multiRest num=\"{k}\"/></layer></staff>",
                first + 1,
                si + 1,
                si + 1
            )
        })
        .collect();
    format!(
        "{brk}{def}   <measure xml:id=\"m{}\" n=\"{}\"{right}>{body}</measure>",
        first + 1,
        first + 1
    )
}

/// **The page setup as the score definition's attributes**: MEI's own places
/// for a page's size and margins (`page.width`, `page.topmar`, ...) and for the
/// size of the staff (`vu.height`, the virtual unit -- half a staff space).
/// Lengths are written in millimetres, which is what a reader of the file
/// expects of a page; [`super::read`] reads them back to the tenth.
fn page_attrs(page: &super::PageSetup) -> String {
    let mm = |tenths: u32| format!("{}.{}mm", tenths / 10, tenths % 10);
    let [top, right, bottom, left] = page.margins;
    format!(
        " page.width=\"{}\" page.height=\"{}\" page.topmar=\"{}\" page.rightmar=\"{}\" \
         page.botmar=\"{}\" page.leftmar=\"{}\" vu.height=\"{}mm\"",
        mm(page.width),
        mm(page.height),
        mm(top),
        mm(right),
        mm(bottom),
        mm(left),
        f64::from(page.staff) / 800.0,
    )
}

/// Wrap each written beam around the elements it covers.
///
/// It runs **after** the projection rather than inside it because a beam is a
/// fact about items and the projection is about measures, and the two do not
/// line up: an item can be split across a barline, and a beam cannot cross one
/// -- a beam is a visual group within a bar, so a run that spans a barline is
/// beamed on each side of it rather than refused. Matching is by the `xml:id`
/// each element already carries, which is exactly what those ids were put there
/// for.
///
/// # Errors
/// When a beam names an item that is not in this score -- the same refusal every
/// other spanner makes.
fn beam(cells: &mut [Vec<String>], voice: &Voice, sheet: &Sheet) -> Result<(), String> {
    for spanner in sheet.spanners.iter().filter(|s| s.kind == "beam") {
        let (from, to) = (spanner.from, spanner.to);
        if sheet.locate(from).is_none() || sheet.locate(to).is_none() {
            let missing = if sheet.locate(from).is_none() {
                from
            } else {
                to
            };
            return Err(format!(
                "the beam is written to an item ({missing}) that is not on this sheet"
            ));
        }
        let (Some(a), Some(b)) = (index_of(voice, from), index_of(voice, to)) else {
            continue; // the beam belongs to another voice
        };
        let covered: Vec<u64> = voice.items[a.min(b)..=a.max(b)]
            .iter()
            .map(Item::id)
            .collect();
        for measure in cells.iter_mut() {
            let run: Vec<usize> = measure
                .iter()
                .enumerate()
                .filter(|(_, cell)| covered.iter().any(|id| cell_is(cell, *id)))
                .map(|(i, _)| i)
                .collect();
            // Two notes at least: one note under a beam is a flag, and MEI
            // reads a one-element `<beam>` as an error rather than as nothing.
            if run.len() < 2 {
                continue;
            }
            let (first, last) = (run[0], run[run.len() - 1]);
            measure[first].insert_str(0, "<beam>");
            measure[last].push_str("</beam>");
        }
    }
    Ok(())
}

/// Where `id` sits in `voice`, if it is in this one at all.
fn index_of(voice: &Voice, id: u64) -> Option<usize> {
    voice.items.iter().position(|i| i.id() == id)
}

/// Whether this rendered element is `id`'s -- its own `xml:id`, or one of the
/// suffixed ids a split part of it carries.
fn cell_is(cell: &str, id: u64) -> bool {
    let stem = format!("xml:id=\"n{id}");
    match cell.find(&stem) {
        None => false,
        Some(at) => matches!(
            cell[at + stem.len()..].chars().next(),
            Some('"') | Some('-')
        ),
    }
}

/// What is written above the music, as MEI's `<titleStmt>` children.
///
/// An empty header writes the bare `<title/>` the encoder has always written,
/// so a score that names nothing is byte-identical to what a v1 document was.
fn header_xml(header: &Header) -> String {
    if header.is_empty() {
        return "<title/>".to_string();
    }
    let mut out = String::new();
    if header.subtitle.is_empty() {
        out.push_str(&header_field("title", &header.title, ""));
    } else {
        out.push_str(&format!(
            "<title>{}{}</title>",
            header_field("title", &header.title, " type=\"main\""),
            header_field("title", &header.subtitle, " type=\"subordinate\"")
        ));
    }
    out.push_str(&header_field("composer", &header.composer, ""));
    out.push_str(&header_field("lyricist", &header.lyricist, ""));
    out
}

/// One header element, or nothing when the field was left empty.
fn header_field(name: &str, text: &str, attrs: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    format!("<{name}{attrs}>{}</{name}>", escape(text))
}

/// The five XML entities, so a title carrying an ampersand does not end the
/// document early.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// How many measures the score needs: enough for its longest voice, and never
/// fewer than one.
fn measure_count(sheet: &Sheet) -> Result<usize, String> {
    let len = sheet.len();
    if !len.is_positive() {
        return Ok(1);
    }
    let mut measures = 0;
    let mut covered = Ratio::ZERO;
    while covered < len {
        let bar = sheet.grid.bar_len(measures);
        if !bar.is_positive() {
            return Err(format!("measure {} has no length", measures + 1));
        }
        covered = covered + bar;
        measures += 1;
    }
    Ok(measures)
}

/// One durational unit of a voice as the emitter sees it: either a plain item,
/// which may be split across barlines and tied, or a **tuplet group**, which
/// may not.
enum Unit<'a> {
    Plain(&'a Item),
    /// `num` in the time of `numbase` -- MEI's own way of putting it.
    Tuplet {
        num: i64,
        numbase: i64,
        items: &'a [Item],
    },
}

/// The tuplet a duration belongs to, or `None` when it is a plain value.
///
/// A written value is always a power-of-two fraction of a whole note, possibly
/// dotted, so a duration whose denominator carries any **odd** factor is inside
/// a tuplet -- and that odd factor is how many notes are in the time of the
/// nearest power of two below it. A triplet eighth is `1/12`: the odd part of
/// 12 is 3, so it is 3 in the time of 2, and its *written* value is
/// `1/12 * 3/2 = 1/8`, an eighth. This is what having exact rationals is for:
/// the fact is in the number, and nothing had to be guessed or snapped.
fn tuplet_ratio(dur: Ratio) -> Option<(i64, i64)> {
    let mut odd = dur.denom();
    while odd % 2 == 0 {
        odd /= 2;
    }
    if odd == 1 {
        return None;
    }
    // The nearest power of two below the count: 3 in the time of 2, 5 in the
    // time of 4, 7 in the time of 4.
    let mut base = 1;
    while base * 2 < odd {
        base *= 2;
    }
    Some((odd, base))
}

/// Split a voice into the units the emitter writes: consecutive items sharing
/// one tuplet ratio are one group, everything else is itself.
fn units(items: &[Item]) -> Vec<Unit<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        match tuplet_ratio(items[i].dur()) {
            None => {
                out.push(Unit::Plain(&items[i]));
                i += 1;
            }
            Some((num, numbase)) => {
                let mut j = i + 1;
                while j < items.len() && tuplet_ratio(items[j].dur()) == Some((num, numbase)) {
                    j += 1;
                }
                out.push(Unit::Tuplet {
                    num,
                    numbase,
                    items: &items[i..j],
                });
                i = j;
            }
        }
    }
    out
}

/// Lay one voice out over `count` measures, returning the rendered elements of
/// each measure. `first` is whether it is its staff's first voice, which is
/// the one that keeps a measure it does not reach with a rest.
#[allow(clippy::too_many_arguments)]
fn project(
    voice: &Voice,
    grid: &Grid,
    count: usize,
    printed: &std::collections::HashSet<(u64, usize)>,
    first: bool,
    clefs: &[(Ratio, String)],
    ftrem: &std::collections::HashSet<u64>,
    words: &Words,
) -> Result<Vec<Vec<String>>, String> {
    let mut measures: Vec<Vec<String>> = vec![Vec::new(); count.max(1)];
    let mut measure = 0;
    let mut pos = 0; // ticks into the current measure
    let mut bar = bar_ticks(grid, 0)?;
    // Whether the previous item tied into this one, so a tie the caller wrote
    // and a tie a barline forced compose instead of overwriting each other.
    let mut tied_in = false;
    // where the unit starts, in whole notes: what a change of clef is at
    let mut onset = Ratio::ZERO;
    // the second note of a two-note tremolo, written with the first
    let mut skip: Option<u64> = None;

    let all = units(&voice.items);
    for (u, unit) in all.iter().enumerate() {
        let span = match unit {
            Unit::Plain(item) => item.dur(),
            Unit::Tuplet { items, .. } => items.iter().fold(Ratio::ZERO, |acc, i| acc + i.dur()),
        };
        let starts = onset;
        onset = onset + span;
        if let Unit::Plain(item) = unit
            && skip == Some(item.id())
        {
            continue;
        }
        if pos == bar && measure + 1 < measures.len() {
            measure += 1;
            pos = 0;
            bar = bar_ticks(grid, measure)?;
        }
        // a change of clef stands before the item it starts at
        if let Some((_, clef)) = clefs.iter().find(|(at, _)| *at == starts) {
            let (shape, line) = parse_clef(clef);
            measures[measure].push(format!("<clef shape=\"{shape}\" line=\"{line}\"/>"));
        }
        // **A two-note tremolo** is written around its two notes, each with
        // the value of the two together -- MEI's spelling of an alternation.
        if let Unit::Plain(item) = unit
            && ftrem.contains(&item.id())
            && let Some(Unit::Plain(next)) = all.get(u + 1)
            && item.sounds()
            && next.sounds()
        {
            let total = ticks(item.dur() + next.dur())?;
            if let (Some((value, dots)), true) = (single_value(total), total <= bar - pos) {
                let a = element(item, value, dots, None, None, printed, words)?;
                let b = element(next, value, dots, None, None, printed, words)?;
                measures[measure].push(format!("<fTrem unitdur=\"16\">{a}{b}</fTrem>"));
                pos += total;
                onset = onset + next.dur();
                skip = Some(next.id());
                tied_in = false;
                continue;
            }
        }
        match *unit {
            Unit::Tuplet {
                num,
                numbase,
                items,
            } => {
                let sounding = items.iter().fold(Ratio::ZERO, |acc, i| acc + i.dur());
                let total = ticks(sounding).map_err(|_| {
                    format!(
                        "a group of {} in the time of {} lasts {}, which is not a \
                         written value; a tuplet has to fill one",
                        num, numbase, sounding
                    )
                })?;
                if total > bar - pos {
                    return Err(format!(
                        "a tuplet of {num} in the time of {numbase} would cross the \
                         barline of measure {}; a tuplet cannot be split, so move it \
                         or change the meter",
                        measure + 1
                    ));
                }
                let mut inner = String::new();
                let last = items.len() - 1;
                for (k, item) in items.iter().enumerate() {
                    let written = item.dur() * Ratio::new(num, numbase);
                    let (value, dots) = single_value(ticks(written)?).ok_or_else(|| {
                        format!(
                            "inside a tuplet, {written} is not a single written value; \
                             every note of a group has to be one"
                        )
                    })?;
                    let opens = item.sounds() && matches!(item, Item::Note { tie: true, .. });
                    let closes = item.sounds() && k == 0 && tied_in;
                    inner.push_str(&element(
                        item,
                        value,
                        dots,
                        tie_of(opens, closes),
                        None,
                        printed,
                        words,
                    )?);
                    tied_in = k == last && opens;
                }
                measures[measure].push(format!(
                    "<tuplet num=\"{num}\" numbase=\"{numbase}\">{inner}</tuplet>"
                ));
                pos += total;
            }
            // Silence has its own path, because **a measure of it is one
            // element**, not a run of values that adds up to a measure. MEI has
            // `<mRest/>` for exactly this and an engraver draws it *centred in
            // the bar*, which is where a reader looks for it; a decomposed whole
            // rest hangs at the start and reads as a rest on the downbeat with
            // something after it. A rest longer than a measure is the ordinary
            // case, not the exception -- an empty staff under a written one is
            // one long rest -- so every full measure it covers is written this
            // way and only its ragged ends are decomposed.
            Unit::Plain(item) if !item.sounds() => {
                let mut remaining = ticks(item.dur())?;
                let mut first = true;
                while remaining > 0 {
                    if pos == bar {
                        if measure + 1 >= measures.len() {
                            measures.push(Vec::new());
                        }
                        measure += 1;
                        pos = 0;
                        bar = bar_ticks(grid, measure)?;
                    }
                    let take = remaining.min(bar - pos);
                    if pos == 0 && take == bar {
                        // the id goes on the first measure it covers, so the
                        // rest a caller wrote is still one thing to name
                        let id = if first {
                            element_id(item.id(), None)
                        } else {
                            String::new()
                        };
                        measures[measure].push(format!("<mRest{id}/>"));
                    } else {
                        let suffix = (!first).then_some(2);
                        for (value, dots) in parts(take) {
                            measures[measure]
                                .push(element(item, value, dots, None, suffix, printed, words)?);
                        }
                    }
                    first = false;
                    pos += take;
                    remaining -= take;
                }
                tied_in = false;
            }
            Unit::Plain(item) => {
                let total = ticks(item.dur())?;
                // (value, dots, measure) for every part the item spans.
                let mut specs: Vec<(i32, i32, usize)> = Vec::new();
                let mut remaining = total;
                while remaining > 0 {
                    if pos == bar {
                        if measure + 1 >= measures.len() {
                            measures.push(Vec::new());
                        }
                        measure += 1;
                        pos = 0;
                        bar = bar_ticks(grid, measure)?;
                    }
                    let take = remaining.min(bar - pos);
                    for (value, dots) in parts(take) {
                        specs.push((value, dots, measure));
                    }
                    pos += take;
                    remaining -= take;
                }
                let sounds = item.sounds();
                let tied_out = matches!(item, Item::Note { tie: true, .. });
                let n = specs.len();
                for (idx, (value, dots, m)) in specs.iter().copied().enumerate() {
                    // A part opens a tie when it is not the last of a split
                    // item, or when the caller tied this item to the next; it
                    // closes one when it is not the first, or when the previous
                    // item tied into this one.
                    let opens = sounds && (idx + 1 < n || (idx + 1 == n && tied_out));
                    let closes = sounds && (idx > 0 || (idx == 0 && tied_in));
                    let suffix = (idx > 0).then_some(idx + 1);
                    measures[m].push(element(
                        item,
                        value,
                        dots,
                        tie_of(opens, closes),
                        suffix,
                        printed,
                        words,
                    )?);
                }
                tied_in = tied_out && sounds;
            }
        }
    }

    // A voice that ran out before the score did keeps its place with rests, so
    // the staves stay aligned and no measure is left empty of everything.
    //
    // **A second voice keeps it with nothing.** It completes the measure it
    // ended in, as a voice does, and the whole measures past it are empty
    // space rather than a rest each: a second line written into one bar is
    // not a rest in every bar after it, which is what the page drew.
    while pos < bar || measure + 1 < measures.len() {
        if pos == bar {
            measure += 1;
            pos = 0;
            bar = bar_ticks(grid, measure)?;
            continue;
        }
        if pos == 0 && !first {
            measures[measure].push("<mSpace/>".to_string());
        } else if pos == 0 {
            // the same rule for the emitter's own filler: a whole measure of
            // silence is one centred `<mRest/>`
            measures[measure].push("<mRest/>".to_string());
        } else {
            let rest = Item::Rest {
                id: 0,
                dur: Ratio::ZERO,
            };
            for (value, dots) in parts(bar - pos) {
                measures[measure].push(element(&rest, value, dots, None, None, printed, words)?);
            }
        }
        pos = bar;
    }
    Ok(measures)
}

/// MEI's `@tie` from the two facts a part knows: whether a tie starts here and
/// whether one ends here.
fn tie_of(opens: bool, closes: bool) -> Option<&'static str> {
    match (opens, closes) {
        (true, true) => Some("m"),
        (true, false) => Some("i"),
        (false, true) => Some("t"),
        (false, false) => None,
    }
}

/// One measure, with every staff's every voice as its own `<layer>`.
fn measure_xml(
    index: usize,
    projected: &[Vec<Vec<Vec<String>>>],
    attached: &str,
    last: bool,
    grid: &Grid,
    def: &str,
) -> String {
    // a measure drawn as a repeat of the one before is its sign, whatever
    // it holds
    let repeated = grid.repeats.contains(&index);
    // A barline somebody chose wins over the final one the last measure gets by
    // default: a score that ends on a repeat ends on a repeat.
    let right = match grid.barlines.iter().find(|(m, _)| *m == index) {
        Some((_, kind)) => format!(" right=\"{kind}\""),
        None if last => " right=\"end\"".to_string(),
        None => String::new(),
    };
    let brk = match grid.breaks.iter().find(|(m, _)| *m == index) {
        Some((_, kind)) if kind == "page" => "<pb/>",
        Some(_) => "<sb/>",
        None => "",
    };
    let staves: String = projected
        .iter()
        .enumerate()
        .map(|(si, voices)| {
            let layers: String = voices
                .iter()
                .enumerate()
                .map(|(vi, measures)| {
                    let cells = match (repeated, vi) {
                        (true, 0) => "<mRpt/>".to_string(),
                        (true, _) => "<mSpace/>".to_string(),
                        _ => measures.get(index).map(|c| c.concat()).unwrap_or_default(),
                    };
                    format!("<layer n=\"{}\">{cells}</layer>", vi + 1)
                })
                .collect();
            // **A staff of a measure is named by the two**, as the model counts
            // them: a press on its lines then says which measure of which staff
            // it was, which is what selecting a measure is.
            format!(
                "<staff xml:id=\"m{}s{}\" n=\"{}\">{layers}</staff>",
                index + 1,
                si + 1,
                si + 1
            )
        })
        .collect();
    format!(
        "{brk}{def}   <measure xml:id=\"m{}\" n=\"{}\"{right}>{staves}{attached}</measure>",
        index + 1,
        index + 1
    )
}

/// A duration as an integer count of 32nds, or a refusal naming what it is that
/// the grid cannot hold. This is the one place the conversion happens.
fn ticks(dur: Ratio) -> Result<i32, String> {
    dur.as_ticks(TPW as i64)
        .and_then(|t| i32::try_from(t).ok())
        .ok_or_else(|| {
            format!(
                "the duration {dur} is not an exact number of 32nd notes, so it \
                 cannot be written as a plain note value; a value like this one \
                 belongs to a tuplet, and a tuplet has to be a run of them that \
                 fills a written value"
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
fn parts(mut ticks: i32) -> Vec<(i32, i32)> {
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

/// What hangs off a measure rather than off a note: a dynamic, an ornament, and
/// the two-ended things -- a slur, a hairpin.
///
/// MEI writes these as children of `<measure>` pointing at notes with
/// `@startid`, not as children of the note, which is why they are gathered here
/// instead of by [`element`]. They are keyed by the measure the note they start
/// on falls in, and a spanner whose ends are in different measures still
/// belongs to the measure it *starts* in.
///
/// # Errors
/// When a spanner names an item that is not in the score. Silently dropping it
/// would leave a caller with a crescendo that never appears and no reason why.
#[allow(clippy::type_complexity)]
fn attachments(
    sheet: &Sheet,
    placed: &std::collections::HashMap<u64, (usize, usize)>,
    starts: &[usize],
    count: usize,
) -> Result<(std::collections::HashMap<usize, String>, Vec<Timed>), String> {
    let mut out: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut timed: Vec<Timed> = Vec::new();
    let grid = &sheet.grid;
    // each item's voice and where it starts, which is what a beat is read from
    let mut times: std::collections::HashMap<u64, (usize, Ratio)> =
        std::collections::HashMap::new();
    for staff in &sheet.staves {
        for (vi, voice) in staff.voices.iter().enumerate() {
            let mut onset = Ratio::ZERO;
            for item in &voice.items {
                times.insert(item.id(), (vi, onset));
                onset = onset + item.dur();
            }
        }
    }
    // **An item of a measure drawn as a repeat has no element on the page**
    // -- the sign stands for it -- so what is written at it is written at
    // its beat of that measure, which the sign's measure still has.
    let anchor = |id: u64, layer: bool| -> Option<(Anchor, usize, usize)> {
        let &(measure, si) = placed.get(&id)?;
        if !grid.repeats.contains(&measure) {
            return Some((Anchor::Item(id), measure, si));
        }
        let &(vi, onset) = times.get(&id)?;
        let (_, offset) = grid.position(onset);
        let beat = beat_of(offset, grid.meter_at(measure).unit);
        let at = Anchor::Beat {
            measure,
            beat,
            layer: layer.then_some(vi),
        };
        Some((at, measure, si))
    };
    let run_of = |measure: usize| starts.iter().rposition(|s| *s <= measure).unwrap_or(0);
    // the ties of the notes let ring, each with the measure it is written in
    let mut rings: Vec<(usize, String)> = Vec::new();

    for staff in &sheet.staves {
        for voice in &staff.voices {
            for (index, item) in voice.items.iter().enumerate() {
                let Some(marks) = item.marks() else { continue };
                let Some(&(measure, si)) = placed.get(&item.id()) else {
                    continue;
                };
                let at = format!(" staff=\"{}\" startid=\"#n{}\"", si + 1, item.id());
                // each is named after the mark it draws, so a press on it
                // names the mark
                let id = |mark: &str| format!(" xml:id=\"{}\"", mark_id(mark, item.id()));
                let here = out.entry(measure).or_default();
                if let Some(dynamic) = &marks.dynamic {
                    here.push_str(&format!(
                        "<dynam{}{at} place=\"below\">{dynamic}</dynam>",
                        id("dynamic")
                    ));
                }
                if let Some(ornament) = &marks.ornament {
                    // An ornament is its own element in MEI, named for what it
                    // is; any other is an `ornam` named by its glyph.
                    let id = id("ornament");
                    if ORNAMENTS.contains(&ornament.as_str()) {
                        here.push_str(&format!("<{ornament}{id}{at}/>"));
                    } else {
                        here.push_str(&format!(
                            "<ornam{id}{at} glyph.auth=\"smufl\" glyph.name=\"{}\"/>",
                            escape(ornament)
                        ));
                    }
                }
                if let Some(order) = &marks.arpeggio {
                    here.push_str(&format!(
                        "<arpeg{}{at} order=\"{}\"/>",
                        id("arpeggio"),
                        escape(order)
                    ));
                }
                if let Some(kind) = &marks.breath {
                    let kind = if kind == "caesura" {
                        "caesura"
                    } else {
                        "breath"
                    };
                    here.push_str(&format!("<{kind}{}{at}/>", id("breath")));
                }
                if marks.ring
                    && let Some(&(_, onset)) = times.get(&item.id())
                {
                    rings.extend(ring_xml(grid, voice, index, onset, si));
                }
                if let Some(fingering) = &marks.fingering {
                    here.push_str(&format!(
                        "<fing{}{at} place=\"above\">{}</fing>",
                        id("fingering"),
                        escape(fingering)
                    ));
                }
                if let Some(harmony) = &marks.harmony {
                    here.push_str(&format!(
                        "<harm{}{at} place=\"above\">{}</harm>",
                        id("harmony"),
                        escape(harmony)
                    ));
                }
            }
        }
    }

    for (measure, xml) in rings {
        out.entry(measure).or_default().push_str(&xml);
    }

    let mut seen: std::collections::HashSet<(String, u64)> = std::collections::HashSet::new();
    for control in &sheet.controls {
        let (point, measure, si) = anchor(control.on, false).ok_or_else(|| {
            format!(
                "a {} is written at item {}, which is not in this score",
                control.kind, control.on
            )
        })?;
        // the first of its kind at an item is named after it; a second one
        // there is the same to the model, which takes them back together
        let named = (control.kind.clone(), control.on);
        let id = if seen.contains(&named) {
            String::new()
        } else {
            format!(" xml:id=\"{}\"", control_id(&control.kind, control.on))
        };
        seen.insert(named);
        let at = format!("{id} staff=\"{}\"{}", si + 1, point.start());
        let text = escape(&control.text);
        let xml = match control.kind.as_str() {
            "tempo" => {
                let bpm = control
                    .bpm
                    .map(|bpm| format!(" midi.bpm=\"{bpm}\""))
                    .unwrap_or_default();
                format!("<tempo{at} place=\"above\"{bpm}>{text}</tempo>")
            }
            "dir" => format!("<dir{at} place=\"above\">{text}</dir>"),
            "reh" => format!("<reh{at} place=\"above\"><rend rend=\"box\">{text}</rend></reh>"),
            other => {
                return Err(format!(
                    "\"{other}\" is not something this layer writes at a point; it \
                     writes a tempo, a direction and a rehearsal mark"
                ));
            }
        };
        out.entry(measure).or_default().push_str(&xml);
    }

    for spanner in &sheet.spanners {
        // A beam and a two-note tremolo are written *around* their elements,
        // inside the layer, so they are not things that hang off the measure.
        if spanner.kind == "beam" || spanner.kind == "ftrem" {
            continue;
        }
        let (from, measure, si) = anchor(spanner.from, true).ok_or_else(|| {
            format!(
                "a {} starts on item {}, which is not in this score",
                spanner.kind, spanner.from
            )
        })?;
        let Some((to, to_measure, _)) = anchor(spanner.to, true) else {
            return Err(format!(
                "a {} ends on item {}, which is not in this score",
                spanner.kind, spanner.to
            ));
        };
        // named after the line it draws, in every part of it: a press on any
        // of them names the one line
        let named = spanner_id(&spanner.kind, spanner.from, spanner.to);
        let staff = format!(" xml:id=\"{named}\" staff=\"{}\"", si + 1);
        let plain = format!(" staff=\"{}\"", si + 1);
        // the element's name with what it says before its ends, and after
        let (name, tail) = match spanner.kind.as_str() {
            "slur" => ("slur".to_string(), String::new()),
            "crescendo" => ("hairpin form=\"cres\"".to_string(), String::new()),
            "diminuendo" => ("hairpin form=\"dim\"".to_string(), String::new()),
            "phrase" => ("phrase".to_string(), String::new()),
            "gliss" => ("gliss".to_string(), String::new()),
            "bracket" => ("bracketSpan".to_string(), " lform=\"solid\"".to_string()),
            "beamspan" => {
                let plist = items_between(sheet, spanner.from, spanner.to)
                    .iter()
                    .map(|id| format!("#n{id}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                ("beamSpan".to_string(), format!(" plist=\"{plist}\""))
            }
            octave if OCTAVES.iter().any(|(name, ..)| *name == octave) => {
                let (_, dis, place) = OCTAVES
                    .iter()
                    .find(|(name, ..)| *name == octave)
                    .copied()
                    .unwrap_or(("8va", 8, "above"));
                (
                    "octave".to_string(),
                    format!(" dis=\"{dis}\" dis.place=\"{place}\""),
                )
            }
            // pressed at the first note and let go at the last: two signs,
            // the second in the measure it is let go in
            "pedal" => {
                out.entry(measure)
                    .or_default()
                    .push_str(&format!("<pedal{staff}{} dir=\"down\"/>", from.start()));
                out.entry(to_measure).or_default().push_str(&format!(
                    "<pedal xml:id=\"{named}-up\"{plain}{} dir=\"up\"/>",
                    to.start()
                ));
                continue;
            }
            other => {
                return Err(format!(
                    "\"{other}\" is not something this layer knows how to write \
                     between two notes; it writes {}",
                    SPANNERS.join(", ")
                ));
            }
        };
        // **A line across a written page break is written once in each run
        // of pages it is in**: to the end of the run it starts in, through
        // the whole of one it passes, and from the start of the run it ends
        // in -- as the engraver itself draws one across a system break, open
        // at the edge. A glissando is a line between two noteheads and a
        // beam between its notes, so neither has an end the page could
        // stand in for.
        let (first, last) = (run_of(measure), run_of(to_measure));
        let cut = first < last && !matches!(spanner.kind.as_str(), "gliss" | "beamspan");
        let parts: Vec<(Anchor, Anchor)> = if cut {
            (first..=last)
                .map(|run| {
                    let opens = starts[run];
                    let closes = starts.get(run + 1).copied().unwrap_or(count) - 1;
                    let start = if run == first {
                        from.clone()
                    } else {
                        Anchor::Beat {
                            measure: opens,
                            beat: "0".to_string(),
                            layer: None,
                        }
                    };
                    let end = if run == last {
                        to.clone()
                    } else {
                        Anchor::Beat {
                            measure: closes,
                            beat: (grid.meter_at(closes).count + 1).to_string(),
                            layer: None,
                        }
                    };
                    (start, end)
                })
                .collect()
        } else {
            vec![(from, to)]
        };
        for (start, end) in parts {
            let at = match &start {
                Anchor::Item(_) => measure,
                Anchor::Beat { measure, .. } => *measure,
            };
            let head = format!("<{name}{staff}{}", start.start());
            let here = out.entry(at).or_default();
            match end {
                Anchor::Item(id) => here.push_str(&format!("{head} endid=\"#n{id}\"{tail}/>")),
                Anchor::Beat { measure, beat, .. } => timed.push(Timed {
                    measure: at,
                    head,
                    to: measure,
                    beat,
                    tail: format!("{tail}/>"),
                }),
            }
        }
    }
    Ok((out, timed))
}

/// **Let it ring**: a tie that leaves the notehead and goes nowhere -- a rest
/// after the note is the common case, which is where the sign comes from.
///
/// The engraver draws one only inside a measure and only from a note, so it
/// is written from each notehead of the item's last written part to a beat
/// of that part's measure, with no item at its end: a beat on, or half the
/// way to the next note or to the barline where either is nearer -- so it
/// reaches neither. A chord split across a barline keeps its pitches' ids
/// in every part, so its ties hang from the first.
fn ring_xml(
    grid: &Grid,
    voice: &Voice,
    index: usize,
    onset: Ratio,
    si: usize,
) -> Vec<(usize, String)> {
    let item = &voice.items[index];
    let pitches = item.pitches().len();
    let (measure, part, from) = match last_part(grid, onset, item.dur()) {
        (_, part, _) if part > 1 && pitches > 1 => (grid.position(onset).0, 1, onset),
        last => last,
    };
    let (opens, closes) = grid.span(measure, measure);
    let unit = grid.meter_at(measure).unit;
    // where the next note of the voice sounds, if one does
    let mut next = onset + item.dur();
    let mut sounds = false;
    for later in &voice.items[index + 1..] {
        if later.sounds() {
            sounds = true;
            break;
        }
        next = next + later.dur();
    }
    let beat = Ratio::new(1, unit.max(1));
    // what stops it: the next note, which a tie that goes nowhere never
    // reaches, or the barline
    let limit = if sounds && next > from && next < closes {
        next
    } else {
        closes
    };
    let end = from + beat.min((limit - from) * Ratio::new(1, 2));
    let end = beat_of(end - opens, unit);
    let id = item.id();
    let heads: Vec<String> = match (pitches, part) {
        (2.., _) => (1..=pitches).map(|p| format!("n{id}-p{p}")).collect(),
        (_, 1) => vec![format!("n{id}")],
        (_, part) => vec![format!("n{id}-{part}")],
    };
    heads
        .into_iter()
        .enumerate()
        .map(|(k, head)| {
            // each tie is the one mark, named after it
            let named = match k {
                0 => mark_id("ring", id),
                k => format!("{}-p{}", mark_id("ring", id), k + 1),
            };
            let xml = format!(
                "<lv xml:id=\"{named}\" staff=\"{}\" startid=\"#{head}\" tstamp2=\"0m+{end}\"/>",
                si + 1
            );
            (measure, xml)
        })
        .collect()
}

/// The last written part of an item that starts at `onset`: the measure it
/// is in, which part of the item it is (from one), and where it starts. An
/// item is one part unless a barline or its own value splits it, as
/// [`project`] writes it.
fn last_part(grid: &Grid, onset: Ratio, dur: Ratio) -> (usize, usize, Ratio) {
    let (mut measure, offset) = grid.position(onset);
    let whole = (measure, 1, onset);
    if tuplet_ratio(dur).is_some() {
        return whole;
    }
    let (Ok(mut pos), Ok(mut remaining)) = (ticks(offset), ticks(dur)) else {
        return whole;
    };
    let (mut count, mut last) = (0, 0);
    while remaining > 0 {
        let Ok(bar) = bar_ticks(grid, measure) else {
            return whole;
        };
        if pos >= bar {
            measure += 1;
            pos = 0;
            continue;
        }
        let take = remaining.min(bar - pos);
        let written = parts(take);
        count += written.len();
        // a dotted value is its value and half of it
        last = written.last().map_or(take, |&(value, dots)| {
            let plain = TPW / value;
            plain + if dots != 0 { plain / 2 } else { 0 }
        });
        pos += take;
        remaining -= take;
    }
    let start = grid.measure_start(measure) + Ratio::from_ticks((pos - last) as i64, TPW as i64);
    (measure, count.max(1), start)
}

/// Where an end of something written beside the notes is: at an item, or --
/// where the page holds no element for the item, or the end is a page's own
/// -- at a beat of a measure.
#[derive(Clone)]
enum Anchor {
    Item(u64),
    Beat {
        measure: usize,
        beat: String,
        /// The voice the item is in, where the beat is an item's.
        layer: Option<usize>,
    },
}

impl Anchor {
    /// The attributes that say something starts here.
    fn start(&self) -> String {
        match self {
            Anchor::Item(id) => format!(" startid=\"#n{id}\""),
            Anchor::Beat { beat, layer, .. } => {
                let layer = layer
                    .map(|vi| format!(" layer=\"{}\"", vi + 1))
                    .unwrap_or_default();
                format!(" tstamp=\"{beat}\"{layer}")
            }
        }
    }
}

/// A line between notes whose end is written as a beat of a measure. How
/// many measures on that measure is depends on how the measures between are
/// written, so the element is finished once that is known: `head`, the end,
/// `tail`.
struct Timed {
    /// The measure the element is written in.
    measure: usize,
    head: String,
    /// The measure it ends in, and the beat of it.
    to: usize,
    beat: String,
    tail: String,
}

/// A place in a measure as MEI counts it: in the meter's unit, from one.
fn beat_of(offset: Ratio, unit: i64) -> String {
    let beat = 1.0 + (offset * Ratio::new(unit, 1)).to_f64();
    let text = format!("{beat:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// **What is written beside the notes, as the model holds it**: a line
/// between two items, a mark one item carries, or something written at an
/// item. It is what an engraved element is *of*, where it is no item itself
/// -- so a press on a slur names the slur, and a verb over it acts on the
/// slur ([`attachment_id`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attachment {
    /// A [`Spanner`](super::model::Spanner): its kind and its two items.
    Spanner { kind: String, from: u64, to: u64 },
    /// One of an item's [`Marks`], by the field's name ([`MARKS`]).
    Mark { mark: String, item: u64 },
    /// A [`Control`](super::model::Control): its kind and the item it is at.
    Control { kind: String, on: u64 },
}

/// The marks of an item the page draws as an element of their own, by the
/// field of [`Marks`] each is.
pub const MARKS: [&str; 7] = [
    "dynamic",
    "ornament",
    "arpeggio",
    "breath",
    "ring",
    "fingering",
    "harmony",
];

/// What is written at a point, by its kind.
pub const CONTROLS: [&str; 3] = ["tempo", "dir", "reh"];

/// The id a line between two items is written under.
fn spanner_id(kind: &str, from: u64, to: u64) -> String {
    format!("a-{kind}-{from}-{to}")
}

/// The id a mark of `item` is written under.
fn mark_id(mark: &str, item: u64) -> String {
    format!("a-{mark}-n{item}")
}

/// The id what is written at `on` is written under.
fn control_id(kind: &str, on: u64) -> String {
    format!("a-{kind}-n{on}")
}

/// **What the engraved element `element_id` is of the model's**, where it is
/// something written beside the notes -- `a-slur-2-11` is the slur from item
/// 2 to item 11, `a-dynamic-n3` the dynamic of item 3, `a-tempo-n5` the tempo
/// mark at item 5 -- or `None` where it was not written as one.
///
/// The emitter's own spelling, read back where it is written, as
/// [`item_id`](super::item_id) reads an item's: a part of one line in another
/// run of pages, the sign a pedal is let go with and the tie of a chord's
/// second notehead carry the same name with something after it, and are the
/// same line and the same mark.
pub fn attachment_id(element_id: &str) -> Option<Attachment> {
    let mut parts = element_id.strip_prefix("a-")?.split('-');
    let kind = parts.next()?;
    let first = parts.next()?;
    if let Some(item) = first.strip_prefix('n') {
        let item: u64 = item.parse().ok()?;
        return if CONTROLS.contains(&kind) {
            Some(Attachment::Control {
                kind: kind.to_string(),
                on: item,
            })
        } else if MARKS.contains(&kind) {
            Some(Attachment::Mark {
                mark: kind.to_string(),
                item,
            })
        } else {
            None
        };
    }
    let from = first.parse().ok()?;
    let to = parts.next()?.parse().ok()?;
    SPANNERS.contains(&kind).then(|| Attachment::Spanner {
        kind: kind.to_string(),
        from,
        to,
    })
}

/// The ornaments MEI names an element for; any other is an `ornam`.
pub const ORNAMENTS: [&str; 4] = ["trill", "mordent", "turn", "fermata"];

/// The octave lines: the spanner's kind, how far they move, and which way.
pub const OCTAVES: [(&str, i32, &str); 4] = [
    ("8va", 8, "above"),
    ("8vb", 8, "below"),
    ("15ma", 15, "above"),
    ("15mb", 15, "below"),
];

/// Every kind of spanner this layer writes.
pub const SPANNERS: [&str; 14] = [
    "slur",
    "crescendo",
    "diminuendo",
    "beam",
    "phrase",
    "gliss",
    "pedal",
    "8va",
    "8vb",
    "15ma",
    "15mb",
    "bracket",
    "beamspan",
    "ftrem",
];

/// The ids of the items of `from`'s voice from it to `to`, both included.
fn items_between(sheet: &Sheet, from: u64, to: u64) -> Vec<u64> {
    for voice in sheet.voices() {
        let ids: Vec<u64> = voice.items.iter().map(Item::id).collect();
        if let (Some(a), Some(b)) = (
            ids.iter().position(|i| *i == from),
            ids.iter().position(|i| *i == to),
        ) {
            return ids[a.min(b)..=a.max(b)].to_vec();
        }
    }
    vec![from, to]
}

/// What the key signature alters this step by: the accidental a note written on
/// that letter carries without printing one.
///
/// It is here because the key-name table is, and it is **public** because
/// moving a note along the staff needs it: a note dragged onto a letter the
/// armature alters arrives altered, which is what reading in a key means. A
/// client computing this for itself would be a second answer to a question the
/// engraver and the model already agree on.
pub fn key_alteration(key: &str, step: Step) -> i32 {
    let (keysig, _) = key_signature(key);
    key_alterations(keysig)[step.index() as usize]
}

/// Which steps a key signature alters, indexed by step (`C` = 0 ... `B` = 6).
///
/// Sharps arrive in the order F C G D A E B and flats in the reverse, which is
/// what "3 sharps" and "2 flats" name.
fn key_alterations(keysig: &str) -> [i32; 7] {
    const SHARPS: [usize; 7] = [3, 0, 4, 1, 5, 2, 6]; // f c g d a e b
    const FLATS: [usize; 7] = [6, 2, 5, 1, 4, 0, 3]; // b e a d g c f
    let mut out = [0; 7];
    let count: usize = keysig.trim_end_matches(['s', 'f']).parse().unwrap_or(0);
    let (order, alter) = if keysig.ends_with('f') {
        (&FLATS, -1)
    } else {
        (&SHARPS, 1)
    };
    for &step in order.iter().take(count.min(7)) {
        out[step] = alter;
    }
    out
}

/// Which pitches have to have their accidental **printed**.
///
/// **Verovio infers nothing here**, which was worth measuring rather than
/// assuming: engraving one phrase both ways says that `<accid>` is always drawn
/// -- including where the key signature already implies it and where the same
/// note was altered earlier in the bar -- while `@accid.ges` is never drawn at
/// all. So an F sharp in C major written as the sounding form comes out as a
/// plain F: a wrong score that looks right, which is the one failure this layer
/// must never produce.
///
/// The decision is therefore ours, and it needs both halves: the **key
/// signature**, and a **per-measure memory** of what has already been printed.
/// An accidental holds for the rest of its measure at its own step and octave,
/// and a new measure starts again from the armature -- the ordinary convention,
/// and the one a reader is reading with.
///
/// Three things print: an alteration the armature does not already give, a
/// return to the natural of a step the armature alters (which needs a natural
/// sign, not silence), and anything a caller marked `forced`, which is what a
/// courtesy accidental is.
///
/// The memory is per **staff**, not per voice: two voices on one staff share a
/// bar, and the second one does not restate what the first already printed.
/// The two questions a whole-staff pass answers at once: which accidentals are
/// printed, and which measure (and staff) each item falls in.
#[allow(clippy::type_complexity)]
fn layout(
    sheet: &Sheet,
) -> (
    std::collections::HashSet<(u64, usize)>,
    std::collections::HashMap<u64, (usize, usize)>,
) {
    let armature_at = |measure: usize| key_alterations(key_signature(sheet.key_at(measure)).0);
    let mut out = std::collections::HashSet::new();
    let mut where_ = std::collections::HashMap::new();

    for (si, staff) in sheet.staves.iter().enumerate() {
        // Every item of the staff, in the order a reader meets them: a rest
        // is placed too, since a tempo mark may stand on one.
        let mut timed: Vec<(Ratio, usize, &Item)> = Vec::new();
        for (vi, voice) in staff.voices.iter().enumerate() {
            let mut onset = Ratio::ZERO;
            for item in &voice.items {
                timed.push((onset, vi, item));
                onset = onset + item.dur();
            }
        }
        timed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        let mut measure = usize::MAX;
        let mut armature = armature_at(0);
        let mut printed: std::collections::HashMap<(i32, i32), i32> =
            std::collections::HashMap::new();
        for (onset, _, item) in timed {
            let (m, _) = sheet.grid.position(onset);
            if m != measure {
                measure = m;
                armature = armature_at(m);
                printed.clear();
            }
            where_.insert(item.id(), (m, si));
            for (pi, pitch) in item.pitches().iter().enumerate() {
                let key = (pitch.step.index(), pitch.octave);
                let standing = printed
                    .get(&key)
                    .copied()
                    .unwrap_or(armature[pitch.step.index() as usize]);
                if pitch.alter != standing || pitch.forced {
                    out.insert((item.id(), pi));
                    printed.insert(key, pitch.alter);
                }
            }
        }
    }
    (out, where_)
}

/// An alteration as MEI writes it. `n` is the natural, which is a *sign* and
/// not the absence of one: a C in a key that sharpens C has to say so.
fn accid_of(alter: i32) -> Result<&'static str, i32> {
    match alter {
        0 => Ok("n"),
        1 => Ok("s"),
        -1 => Ok("f"),
        2 => Ok("x"),
        -2 => Ok("ff"),
        other => Err(other),
    }
}

/// The `xml:id` an element is written under: the model's own item id, with a
/// suffix when one item draws more than one thing.
///
/// `n7` is item 7; `n7-2` is the second part of an item split across a
/// barline; `n7-p1` is the first pitch of a chord. Every one of them maps back
/// to exactly one item, which is what a gesture on the page needs and what a
/// re-engraving has to preserve. Item `0` is the emitter's own filler -- a rest
/// written to keep a short voice in step -- and carries no id at all, since
/// nothing in the model answers for it.
fn element_id(id: u64, suffix: Option<usize>) -> String {
    match (id, suffix) {
        (0, _) => String::new(),
        (id, None) => format!(" xml:id=\"n{id}\""),
        (id, Some(n)) => format!(" xml:id=\"n{id}-{n}\""),
    }
}

/// One written element: a rest, a note, or a chord of them.
#[allow(clippy::too_many_arguments)]
fn element(
    item: &Item,
    value: i32,
    dots: i32,
    tie: Option<&str>,
    suffix: Option<usize>,
    printed: &std::collections::HashSet<(u64, usize)>,
    words: &Words,
) -> Result<String, String> {
    let d = if dots != 0 { " dots=\"1\"" } else { "" };
    let id = element_id(item.id(), suffix);
    let marks = item.marks();
    // **A sounding length never reaches the page.** `@dur.ges` looks like the
    // way to say "written a quarter, sounds an eighth", and it is not: an
    // engraver reads it as the note's real duration and advances its own clock
    // by it, so a staccato quarter written that way does not merely sound short
    // -- every attack after it moves a quarter-beat earlier and the measure comes
    // out short. Shortening a staccato is a *performance* decision and belongs
    // to whoever plays the page. The model keeps the fact
    // ([`super::model::Marks::sounding`]); the interpreter is what honours it.
    // **A beat repeat is drawn as its sign**, the item it holds being what it
    // repeats: the sign takes the place of the note, the duration is the
    // beat's.
    if marks.is_some_and(|m| m.beat_repeat) && suffix.is_none() {
        return Ok(format!("<beatRpt{id}/>"));
    }
    // the lyrics go on the note, or on a chord's first note, and only on the
    // part that starts the item
    let verses = if suffix.is_none() {
        verses_xml(item.id(), marks, words)
    } else {
        String::new()
    };
    let drawn = match item.pitches() {
        // Nothing to sound draws as a rest, however the caller spelled it.
        [] => format!("<rest{id} dur=\"{value}\"{d}/>"),
        // Only the first part of a split item prints its accidental: the tie
        // carries it across the barline, and restating it is what a reader
        // reads as a second, different alteration.
        [one] => note_xml(
            one,
            Some(value),
            dots,
            tie,
            &id,
            marks,
            suffix.is_none() && printed.contains(&(item.id(), 0)),
            &verses,
        )?,
        many => {
            let inner = many
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    // the pitches of a chord are named apart, so one of them can
                    // still be selected and moved on its own
                    let pid = if item.id() == 0 {
                        String::new()
                    } else {
                        format!(" xml:id=\"n{}-p{}\"", item.id(), i + 1)
                    };
                    note_xml(
                        p,
                        None,
                        0,
                        tie,
                        &pid,
                        None,
                        suffix.is_none() && printed.contains(&(item.id(), i)),
                        if i == 0 { &verses } else { "" },
                    )
                })
                .collect::<Result<String, _>>()?;
            let inner = format!("{}{inner}", articulations_xml(marks));
            format!(
                "<chord{id} dur=\"{value}\"{d}{}>{inner}</chord>",
                stem_xml(marks)
            )
        }
    };
    // a tremolo on the note: the strokes through its stem, as the value each
    // stroke halves
    Ok(match marks.and_then(|m| m.tremolo) {
        Some(strokes) if item.sounds() => {
            let strokes = u32::from(strokes.clamp(1, 3));
            format!("<bTrem unitdur=\"{}\">{drawn}</bTrem>", 8 << (strokes - 1))
        }
        _ => drawn,
    })
}

/// Where each syllable stands in its word, by item and verse: `i` the first,
/// `m` one inside, `t` the last -- what draws the dash between two.
type Words = std::collections::HashMap<(u64, usize), &'static str>;

/// The place of every syllable in its word, read along each voice.
fn words_of(sheet: &Sheet) -> Words {
    let mut out = Words::new();
    for voice in sheet.voices() {
        let mut open: Vec<bool> = Vec::new();
        for item in voice.items.iter().filter(|i| i.sounds()) {
            let Some(marks) = item.marks() else { continue };
            for (verse, syl) in marks.lyrics.iter().enumerate() {
                if syl.is_empty() {
                    continue;
                }
                while open.len() <= verse {
                    open.push(false);
                }
                let goes_on = syl.ends_with('-');
                let place = match (open[verse], goes_on) {
                    (false, true) => "i",
                    (true, true) => "m",
                    (true, false) => "t",
                    (false, false) => "",
                };
                if !place.is_empty() {
                    out.insert((item.id(), verse), place);
                }
                open[verse] = goes_on;
            }
        }
    }
    out
}

/// The lyrics a note carries, verse by verse: a syllable each, one that ends
/// in `-` continued by a dash into the next.
fn verses_xml(id: u64, marks: Option<&Marks>, words: &Words) -> String {
    marks
        .map(|m| {
            m.lyrics
                .iter()
                .enumerate()
                .filter(|(_, syl)| !syl.is_empty())
                .map(|(n, syl)| {
                    let (text, con) = match syl.strip_suffix('-') {
                        Some(head) => (head, " con=\"d\""),
                        None => (syl.as_str(), ""),
                    };
                    let place = words
                        .get(&(id, n))
                        .map(|w| format!(" wordpos=\"{w}\""))
                        .unwrap_or_default();
                    format!(
                        "<verse n=\"{}\"><syl{place}{con}>{}</syl></verse>",
                        n + 1,
                        escape(text)
                    )
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// The `<artic>` children a note carries, if any.
fn articulations_xml(marks: Option<&Marks>) -> String {
    marks
        .map(|m| {
            m.articulations
                .iter()
                .map(|a| format!("<artic artic=\"{a}\"/>"))
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// The stem direction a caller forced, as an attribute.
fn stem_xml(marks: Option<&Marks>) -> String {
    match marks.and_then(|m| m.stem.as_deref()) {
        Some(dir) => format!(" stem.dir=\"{dir}\""),
        None => String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn note_xml(
    pitch: &Pitch,
    value: Option<i32>,
    dots: i32,
    tie: Option<&str>,
    id: &str,
    marks: Option<&Marks>,
    print_accid: bool,
    verses: &str,
) -> Result<String, String> {
    // A pitch already carries its spelling. Which accidental world a bare MIDI
    // number was spelled into was decided on the way in, before this point.
    let (pname, octave) = (pitch.step.pname(), pitch.octave);
    // MEI writes up to a double accidental. Anything past that is refused
    // rather than dropped: a triple sharp silently written as a natural is a
    // wrong score that looks right, which is the one failure this layer must
    // never produce.
    let accid = accid_of(pitch.alter).map_err(|alter| {
        format!(
            "the pitch {pname}{octave} is altered by {alter} semitones, and MEI \
             writes at most a double accidental; respell it"
        )
    })?;
    let mut head = match value {
        Some(v) => format!("<note{id} dur=\"{v}\""),
        None => format!("<note{id}"),
    };
    if dots != 0 {
        head.push_str(" dots=\"1\"");
    }
    head.push_str(&stem_xml(marks));
    if let Some(g) = marks.and_then(|m| m.grace.as_deref()) {
        head.push_str(&format!(" grace=\"{g}\""));
    }
    head.push_str(&format!(" oct=\"{octave}\" pname=\"{pname}\""));
    if let Some(tie) = tie {
        head.push_str(&format!(" tie=\"{tie}\""));
    }
    // Printed or merely sounding: `<accid>` is drawn and `@accid.ges` is not,
    // and which one this pitch takes was decided for the whole staff at once.
    let accid = if print_accid {
        format!("<accid accid=\"{accid}\"/>")
    } else if pitch.alter != 0 {
        format!("<accid accid.ges=\"{accid}\"/>")
    } else {
        String::new()
    };
    let inner = format!("{accid}{}{verses}", articulations_xml(marks));
    Ok(if inner.is_empty() {
        format!("{head}/>")
    } else {
        format!("{head}>{inner}</note>")
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
        Slot::note(vec![midi], ticks)
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
        assert_eq!(parts(8), vec![(4, 0)]); // a quarter (one beat)
        assert_eq!(parts(4), vec![(8, 0)]); // an eighth
        assert_eq!(parts(16), vec![(2, 0)]); // a half
        assert_eq!(parts(12), vec![(4, 1)]); // 1.5 beats -> a dotted quarter
        assert_eq!(parts(20), vec![(2, 0), (8, 0)]); // 2.5 beats -> half + eighth
    }

    #[test]
    fn voice_to_mei_writes_a_monophonic_melody() {
        // 60 for 1 beat, 62 for 0.5, 64 for 1.5, a 1-beat rest, 65 for 1 beat,
        // at beat_unit 4 (a quarter = 8 ticks).
        let voice = vec![
            note(60, 8),
            note(62, 4),
            note(64, 12),
            Slot::rest(8),
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
        let voice = vec![Slot::note(vec![60, 64, 67], 8)];
        let mei = voice_to_mei(&voice, "4/4", "G2", "C");
        assert!(mei.contains("<chord xml:id=\"n1\" dur=\"4\">"));
        assert_eq!(mei.matches("<note").count(), 3);
        // each pitch of the chord is named apart, so one of them can still be
        // selected and moved on its own
        assert!(mei.contains("xml:id=\"n1-p1\"") && mei.contains("xml:id=\"n1-p3\""));
    }

    #[test]
    fn an_empty_voice_still_draws_a_bar_of_rests() {
        let mei = voice_to_mei(&[], "4/4", "G2", "C");
        // a bar with nothing in it is one `<mRest/>`, drawn centred
        assert!(mei.contains("<mRest/>"), "{mei}");
        assert_eq!(mei.matches("<measure").count(), 1);
    }

    #[test]
    fn a_voice_deserializes_from_the_wire_form() {
        let voice: Vec<Slot> = serde_json::from_str(
            r#"[{"midis": [60, 64], "ticks": 8}, {"ticks": 8}, {"midis": [], "ticks": 8}]"#,
        )
        .expect("parses");
        assert_eq!(voice[0], Slot::note(vec![60, 64], 8));
        assert_eq!(voice[1], Slot::rest(8));
        // A pitchless slot is a rest however it was spelled, so it draws as one.
        let mei = voice_to_mei(&voice[2..], "4/4", "G2", "C");
        assert!(
            mei.contains("<rest xml:id=\"n1\" dur=\"4\"/>"),
            "a quarter rest"
        );
        assert!(!mei.contains("<chord"), "never an empty chord");
    }

    #[test]
    fn a_slot_carries_what_is_written_on_the_note_and_nothing_it_cannot() {
        let voice: Vec<Slot> = serde_json::from_str(
            r#"[{"midis": [60], "ticks": 8, "articulations": ["stacc"],
                 "dynamic": "mf", "ornament": "trill", "stem": "up",
                 "sounding": 4, "tie": true}]"#,
        )
        .expect("parses");
        let sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        let Item::Note { marks, tie, .. } = &sheet.staves[0].voices[0].items[0] else {
            panic!("a note");
        };
        assert!(tie, "the slot tied into the next");
        assert_eq!(marks.articulations, ["stacc"]);
        assert_eq!(marks.dynamic.as_deref(), Some("mf"));
        assert_eq!(marks.ornament.as_deref(), Some("trill"));
        assert_eq!(marks.stem.as_deref(), Some("up"));
        // Ticks are the slot's unit and the model's is a rational: an eighth.
        assert_eq!(marks.sounding, Some(Ratio::new(1, 8)));
    }

    #[test]
    fn a_misspelt_mark_is_an_error_rather_than_a_silent_rest() {
        // The wire form is total -- a slot with no pitches *is* a rest -- so a
        // key it does not know would otherwise be dropped and the note with it.
        let refused: Result<Vec<Slot>, _> =
            serde_json::from_str(r#"[{"midis": [60], "ticks": 8, "articulation": ["stacc"]}]"#);
        assert!(refused.is_err(), "an unknown key is refused");
    }

    #[test]
    fn a_slot_spells_against_the_key_and_asks_for_the_sign() {
        // C major spells the black keys with sharps; this note says otherwise,
        // and asks for the flat to be printed as well.
        let voice: Vec<Slot> = serde_json::from_str(
            r#"[{"midis": [63], "ticks": 8, "spelling": "flat", "accidental": "written"}]"#,
        )
        .expect("parses");
        let sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        let pitch = sheet.staves[0].voices[0].items[0].pitches()[0];
        assert_eq!(pitch.step, Step::E);
        assert_eq!(pitch.alter, -1);
        assert!(pitch.forced);
        // Without the slot saying so, the same number is a D sharp.
        let plain = voice_to_sheet(&[Slot::note(vec![63], 8)], "4/4", "G2", "C");
        assert_eq!(
            plain.staves[0].voices[0].items[0].pitches()[0].step,
            Step::D
        );
    }

    #[test]
    fn a_slot_that_states_its_written_pitch_and_value_is_written_as_they_are() {
        // an F flat, which no spelling of a number reaches, as a triplet
        // eighth, which no count of 32nds holds; the staff and the voice the
        // event named are read past
        let voice: Vec<Slot> = serde_json::from_str(
            r#"[{"midis": [64], "ticks": 3, "spelling": "sharp",
                 "pitches": [{"step": "f", "alter": -1, "octave": 4}],
                 "value": [1, 12], "staff": 1, "voice": 1},
                {"ticks": 4, "pitches": [{"step": "c", "octave": 4}, {"step": "g", "octave": 4}]}]"#,
        )
        .expect("parses");
        let sheet = voice_to_sheet(&voice, "4/4", "G2", "C");
        let items = &sheet.staves[0].voices[0].items;
        let written = items[0].pitches()[0];
        assert_eq!(
            (written.step, written.alter, written.octave),
            (Step::F, -1, 4)
        );
        assert_eq!(items[0].dur(), Ratio::new(1, 12));
        // a slot with written pitches and no numbers is a note, not a rest
        assert_eq!(items[1].pitches().len(), 2);
        assert_eq!(items[1].dur(), Ratio::new(1, 8));
    }

    #[test]
    fn what_the_model_can_hold_and_mei_cannot_write_is_refused_by_name() {
        use super::super::model::{Marks, Staff, Voice};
        let note = |alter, dur| Item::Note {
            id: 1,
            pitches: vec![Pitch {
                step: Step::C,
                alter,
                octave: 4,
                forced: false,
            }],
            dur,
            tie: false,
            marks: Marks::default(),
        };
        let sheet_of = |items| Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice { items }],
                ..Staff::default()
            }],
            ..Default::default()
        };

        // A lone triplet eighth is not a tuplet: a tuplet is a run of them that
        // fills a written value, and one third of a quarter fills nothing.
        let err = sheet_to_mei(&sheet_of(vec![note(0, Ratio::new(1, 12))]))
            .expect_err("refuses an incomplete group");
        assert!(err.contains("3 in the time of 2"), "{err}");

        // A triple sharp is data the model can hold and MEI cannot spell.
        let err =
            sheet_to_mei(&sheet_of(vec![note(3, Ratio::new(1, 4))])).expect_err("refuses a triple");
        assert!(err.contains("double accidental"), "{err}");
    }

    /// The six cases below are the **byte-for-byte** record of what this encoder
    /// writes. It began as what the encoder wrote before the score model
    /// existed -- the model's own acceptance was that not one byte moved -- and
    /// it was **re-recorded once**, deliberately, when the emission milestone
    /// changed three things about every page:
    ///
    /// - every element carries the **id of the item it came from**, so a
    ///   gesture on the page names a note in the model and a selection survives
    ///   a re-engraving;
    /// - a measure that ran short is **completed with rests**, where it used to
    ///   be left partly empty;
    /// - an accidental is printed or merely sounded by **the rule measured
    ///   against the engraver**, which fixed a silent wrong: a C in a key that
    ///   sharpens C used to be written with no sign at all, and read as C
    ///   sharp;
    /// - a rest that fills a measure is `<mRest/>`, which an engraver draws
    ///   **centred in the bar**, where a reader looks for it -- a run of values
    ///   adding up to a measure hangs at its start instead;
    /// - every measure, and every staff of it, carries an **id of its own**
    ///   (`m3`, `m3s1`), so a press on a staff's lines names the measure and
    ///   the staff it fell in -- which is what selecting a measure is.
    ///
    /// A diff here is either another deliberate change to the engraving, which
    /// has to be re-recorded with a reason like those, or something being lost
    /// on the way through.
    #[test]
    fn the_model_writes_the_bytes_the_wire_form_always_wrote() {
        let cases: Vec<(&str, Vec<Slot>, &str, &str, &str)> = vec![
            (
                "melody",
                vec![
                    note(60, 8),
                    note(62, 4),
                    note(64, 12),
                    Slot::rest(8),
                    note(65, 8),
                ],
                "4/4",
                "G2",
                "C",
            ),
            ("split", vec![note(60, 16), note(67, 24)], "4/4", "G2", "C"),
            (
                "chord",
                vec![Slot::note(vec![60, 64, 67], 8)],
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

#[cfg(test)]
mod emission {
    //! What the emission milestone taught the encoder to write: several voices
    //! and staves, tuplets, and the marks a note carries.

    use super::super::model::{Grid, Marks, Staff, Step, Voice};
    use super::*;

    fn pitch(step: Step, octave: i32) -> Pitch {
        Pitch {
            step,
            alter: 0,
            octave,
            forced: false,
        }
    }

    fn note(step: Step, dur: Ratio, id: u64) -> Item {
        Item::Note {
            id,
            pitches: vec![pitch(step, 4)],
            dur,
            tie: false,
            marks: Marks::default(),
        }
    }

    fn voice(items: Vec<Item>) -> Voice {
        Voice { items }
    }

    fn sheet(staves: Vec<Staff>) -> Sheet {
        let mut sheet = Sheet {
            staves,
            ..Default::default()
        };
        sheet.assign_ids();
        sheet
    }

    #[test]
    fn two_voices_on_one_staff_are_two_layers() {
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![
                voice(vec![
                    note(Step::C, Ratio::new(1, 2), 1),
                    note(Step::D, Ratio::new(1, 2), 2),
                ]),
                voice(vec![note(Step::E, Ratio::ONE, 3)]),
            ],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes polyphony");
        assert_eq!(mei.matches("<layer").count(), 2);
        assert_eq!(mei.matches("<staff ").count(), 1);
        // one measure holds both lines, and the second layer is the whole note
        assert!(mei.contains("<layer n=\"2\"><note xml:id=\"n3\" dur=\"1\""));
    }

    #[test]
    fn two_staves_take_a_brace_and_a_bar_through() {
        let mine = sheet(vec![
            Staff {
                clef: "G2".into(),
                voices: vec![voice(vec![note(Step::C, Ratio::ONE, 1)])],
                ..Staff::default()
            },
            Staff {
                clef: "F4".into(),
                voices: vec![voice(vec![note(Step::C, Ratio::ONE, 2)])],
                ..Staff::default()
            },
        ]);
        let mei = sheet_to_mei(&mine).expect("writes a grand staff");
        assert!(mei.contains("symbol=\"brace\"") && mei.contains("bar.thru=\"true\""));
        assert_eq!(mei.matches("<staffDef").count(), 2);
        assert!(mei.contains("clef.shape=\"F\""));
        // both staves are inside the one measure, which is what a brace means
        assert_eq!(mei.matches("<measure").count(), 1);
        assert_eq!(mei.matches("<staff ").count(), 2);
    }

    #[test]
    fn a_short_voice_keeps_its_place_with_rests() {
        // one voice lasts a whole note, the other a quarter: the second is
        // filled out so the staves stay in step and no measure is half empty
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![
                voice(vec![note(Step::C, Ratio::ONE, 1)]),
                voice(vec![note(Step::E, Ratio::new(1, 4), 2)]),
            ],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes it");
        let second = mei.split("<layer n=\"2\">").nth(1).unwrap();
        assert!(second.contains("<rest"), "the short voice is filled out");
    }

    #[test]
    fn a_rest_longer_than_a_measure_writes_each_one_as_one() {
        // The ordinary case, not the exception: an empty staff under a written
        // one is *one long rest*, and every full measure it covers has to be an
        // `<mRest/>` or the whole run comes out as whole rests hanging at the
        // start of each bar.
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![
                voice(vec![note(Step::C, Ratio::from(3), 1)]),
                voice(vec![Item::Rest {
                    id: 2,
                    dur: Ratio::from(3),
                }]),
            ],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes it");
        assert_eq!(mei.matches("<mRest").count(), 3, "{mei}");
        assert!(!mei.contains("<rest dur=\"1\""));
    }

    #[test]
    fn a_rest_that_fills_a_measure_is_written_as_one() {
        // MEI has an element for exactly this and an engraver draws it centred
        // in the bar, which is where a reader looks for it. A run of values that
        // happens to add up to a measure hangs at the start of it instead.
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![
                voice(vec![note(Step::E, Ratio::ONE, 2)]),
                voice(vec![note(Step::C, Ratio::from(2), 1)]),
            ],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes it");
        assert!(mei.contains("<mRest/>"), "{mei}");
        assert!(
            !mei.contains("<rest dur=\"1\""),
            "not a decomposed whole rest"
        );
    }

    #[test]
    fn a_score_is_cut_into_runs_of_pages_at_its_page_breaks() {
        let mut mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(vec![note(Step::C, Ratio::from(4), 1)])],
            ..Staff::default()
        }]);
        mine.header.title = "A title".into();
        // no page break: one document, the one the score is written as
        assert_eq!(
            sheet_to_mei_pages(&mine).unwrap(),
            vec![sheet_to_mei(&mine).unwrap()]
        );
        mine.grid.breaks = vec![(2, "page".into()), (3, "system".into())];
        let runs = sheet_to_mei_pages(&mine).unwrap();
        assert_eq!(runs.len(), 2);
        // the measures keep their numbers, and the title is the first run's
        assert!(runs[0].contains("<measure xml:id=\"m2\""), "{}", runs[0]);
        assert!(!runs[0].contains("xml:id=\"m3\""));
        assert!(runs[1].contains("<measure xml:id=\"m3\""));
        assert!(runs[1].contains("<sb/>"), "a system break past it stays");
        // (each names it in its head; only the first writes it on the page)
        assert_eq!(runs[0].matches("A title").count(), 2);
        assert_eq!(runs[1].matches("A title").count(), 1);
        // a run after the first opens with the page number
        assert!(runs[1].contains("pgHead func=\"first\""), "{}", runs[1]);
        assert!(runs[1].contains(super::super::PAGE_NUMBER));
    }

    /// A slur or a hairpin across a written page break is in both runs of
    /// pages: each holds the part of it that is its own, ending where the
    /// run ends and starting where the next one opens.
    #[test]
    fn a_line_across_a_page_break_is_written_in_each_run_it_is_in() {
        use super::super::model::Spanner;
        let quarter = Ratio::new(1, 4);
        let mut mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(
                (1..=12).map(|id| note(Step::C, quarter, id)).collect(),
            )],
            ..Staff::default()
        }]);
        let line = |kind: &str, from, to| Spanner {
            kind: kind.into(),
            from,
            to,
        };
        mine.spanners = vec![line("slur", 2, 11), line("crescendo", 2, 6)];
        // one document: the two ends are the two notes
        let whole = sheet_to_mei(&mine).unwrap();
        assert!(
            whole.contains(
                "<slur xml:id=\"a-slur-2-11\" staff=\"1\" startid=\"#n2\" endid=\"#n11\"/>"
            )
        );
        mine.grid.breaks = vec![(1, "page".into()), (2, "page".into())];
        assert_eq!(sheet_to_mei(&mine).unwrap().matches("<slur").count(), 1);

        let runs = sheet_to_mei_pages(&mine).unwrap();
        assert_eq!(runs.len(), 3);
        // from its note to the end of the first run's last measure
        let first = "<slur xml:id=\"a-slur-2-11\" staff=\"1\" startid=\"#n2\" tstamp2=\"0m+5\"/>";
        assert!(runs[0].contains(first), "{}", runs[0]);
        // through the whole of the run it passes
        let through = "<slur xml:id=\"a-slur-2-11\" staff=\"1\" tstamp=\"0\" tstamp2=\"0m+5\"/>";
        assert!(runs[1].contains(through), "{}", runs[1]);
        // and from where the last opens to its note
        let last = "<slur xml:id=\"a-slur-2-11\" staff=\"1\" tstamp=\"0\" endid=\"#n11\"/>";
        assert!(runs[2].contains(last), "{}", runs[2]);
        // the hairpin ends in the second run and is no part of the third
        let opens = "<hairpin form=\"cres\" xml:id=\"a-crescendo-2-6\" staff=\"1\" startid=\"#n2\" tstamp2=\"0m+5\"/>";
        let closes = "<hairpin form=\"cres\" xml:id=\"a-crescendo-2-6\" staff=\"1\" tstamp=\"0\" endid=\"#n6\"/>";
        assert!(runs[0].contains(opens), "{}", runs[0]);
        assert!(runs[1].contains(closes), "{}", runs[1]);
        assert!(!runs[2].contains("<hairpin"));
    }

    /// Let it ring is a tie from the notehead to nowhere: it ends at a beat
    /// of its own measure, short of the next note and of the barline, hangs
    /// from the last part of a note tied across one, and from each notehead
    /// of a chord.
    #[test]
    fn let_it_ring_is_a_short_tie_that_ends_inside_its_measure() {
        let ring = |item: Item| match item {
            Item::Note {
                id,
                pitches,
                dur,
                tie,
                ..
            } => Item::Note {
                id,
                pitches,
                dur,
                tie,
                marks: Marks {
                    ring: true,
                    ..Marks::default()
                },
            },
            other => other,
        };
        let quarter = Ratio::new(1, 4);
        let chord = Item::Note {
            id: 7,
            pitches: vec![pitch(Step::C, 4), pitch(Step::E, 4)],
            dur: quarter,
            tie: false,
            marks: Marks::default(),
        };
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(vec![
                // a quarter before a rest: a beat on
                ring(note(Step::C, quarter, 1)),
                Item::Rest {
                    id: 2,
                    dur: quarter,
                },
                // an eighth before a note: half the way to it
                ring(note(Step::C, Ratio::new(1, 8), 3)),
                note(Step::D, Ratio::new(1, 8), 4),
                // the last quarter of the measure: half the way to the barline
                ring(note(Step::C, quarter, 5)),
                note(Step::C, Ratio::new(1, 2), 6),
                // a chord before a note: each notehead's, half the way to it
                ring(chord),
                // a whole note from the last beat: tied across the barline,
                // and the tie hangs from its second part, a beat on
                ring(note(Step::C, Ratio::ONE, 8)),
            ])],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes it");
        for tie in [
            "<lv xml:id=\"a-ring-n1\" staff=\"1\" startid=\"#n1\" tstamp2=\"0m+2\"/>",
            "<lv xml:id=\"a-ring-n3\" staff=\"1\" startid=\"#n3\" tstamp2=\"0m+3.25\"/>",
            "<lv xml:id=\"a-ring-n5\" staff=\"1\" startid=\"#n5\" tstamp2=\"0m+4.5\"/>",
            "<lv xml:id=\"a-ring-n7\" staff=\"1\" startid=\"#n7-p1\" tstamp2=\"0m+3.5\"/>",
            "<lv xml:id=\"a-ring-n7-p2\" staff=\"1\" startid=\"#n7-p2\" tstamp2=\"0m+3.5\"/>",
            "<lv xml:id=\"a-ring-n8\" staff=\"1\" startid=\"#n8-2\" tstamp2=\"0m+2\"/>",
        ] {
            assert!(mei.contains(tie), "{tie} in {mei}");
        }
        assert!(
            !mei.contains("<lv xml:id=\"a-ring-n8\" staff=\"1\" startid=\"#n8\""),
            "{mei}"
        );
        // the second measure holds the chord's two, the third the last part's
        let third = mei.split("xml:id=\"m3\"").nth(1).unwrap();
        assert_eq!(third.matches("<lv").count(), 1, "{third}");
    }

    /// What is written beside the notes is named after what it is in the
    /// model, and the name is read back: a press on a slur names the slur.
    #[test]
    fn what_is_written_beside_the_notes_is_named_after_the_model() {
        use super::super::model::{Control, Spanner};
        let quarter = Ratio::new(1, 4);
        let mut items: Vec<Item> = (1..=4).map(|id| note(Step::C, quarter, id)).collect();
        if let Item::Note { marks, .. } = &mut items[2] {
            marks.dynamic = Some("mf".into());
            marks.ornament = Some("trill".into());
        }
        let mut mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(items)],
            ..Staff::default()
        }]);
        let line = |kind: &str, from, to| Spanner {
            kind: kind.into(),
            from,
            to,
        };
        mine.spanners = vec![line("slur", 1, 4), line("8va", 2, 3), line("pedal", 1, 2)];
        mine.controls = vec![Control {
            kind: "tempo".into(),
            on: 1,
            text: "Allegro".into(),
            bpm: Some(120.0),
        }];
        let mei = sheet_to_mei(&mine).unwrap();
        let named = [
            (
                "a-slur-1-4",
                Attachment::Spanner {
                    kind: "slur".into(),
                    from: 1,
                    to: 4,
                },
            ),
            (
                "a-8va-2-3",
                Attachment::Spanner {
                    kind: "8va".into(),
                    from: 2,
                    to: 3,
                },
            ),
            (
                "a-pedal-1-2",
                Attachment::Spanner {
                    kind: "pedal".into(),
                    from: 1,
                    to: 2,
                },
            ),
            // the sign the pedal is let go with is the same pedal
            (
                "a-pedal-1-2-up",
                Attachment::Spanner {
                    kind: "pedal".into(),
                    from: 1,
                    to: 2,
                },
            ),
            (
                "a-dynamic-n3",
                Attachment::Mark {
                    mark: "dynamic".into(),
                    item: 3,
                },
            ),
            (
                "a-ornament-n3",
                Attachment::Mark {
                    mark: "ornament".into(),
                    item: 3,
                },
            ),
            (
                "a-tempo-n1",
                Attachment::Control {
                    kind: "tempo".into(),
                    on: 1,
                },
            ),
        ];
        for (id, what) in named {
            assert!(mei.contains(&format!("xml:id=\"{id}\"")), "{id} in {mei}");
            assert_eq!(attachment_id(id), Some(what), "{id}");
        }
        // an item, a measure, a page's text and an id the engraver minted are
        // none of them
        for id in [
            "n3",
            "n3-2",
            "m1s1",
            "t-title",
            "s1x9k2",
            "a-nothing-n3",
            "a-slur-1",
        ] {
            assert_eq!(attachment_id(id), None, "{id}");
        }
    }

    /// The end of a run is counted in measures as they are written: a run of
    /// empty ones drawn as one numbered rest is one measure on.
    #[test]
    fn a_line_to_the_end_of_a_run_counts_a_numbered_rest_as_one_measure() {
        use super::super::model::Spanner;
        let quarter = Ratio::new(1, 4);
        let mut items: Vec<Item> = (1..=4).map(|id| note(Step::C, quarter, id)).collect();
        items.push(Item::Rest {
            id: 5,
            dur: Ratio::from(3),
        });
        items.extend((6..=9).map(|id| note(Step::C, quarter, id)));
        let mut mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(items)],
            ..Staff::default()
        }]);
        mine.grid.multirests = true;
        mine.grid.breaks = vec![(4, "page".into())];
        mine.spanners = vec![Spanner {
            kind: "slur".into(),
            from: 3,
            to: 7,
        }];
        let runs = sheet_to_mei_pages(&mine).unwrap();
        assert!(runs[0].contains("<multiRest num=\"3\"/>"), "{}", runs[0]);
        let first = "<slur xml:id=\"a-slur-3-7\" staff=\"1\" startid=\"#n3\" tstamp2=\"1m+5\"/>";
        assert!(runs[0].contains(first), "{}", runs[0]);
    }

    #[test]
    fn a_second_voice_leaves_the_measures_past_it_empty() {
        // a second line written into the first bar completes it with a rest,
        // and is no rest at all in the bars after it
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![
                voice(vec![note(Step::C, Ratio::from(3), 1)]),
                voice(vec![note(Step::E, Ratio::new(1, 2), 2)]),
            ],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes it");
        let second: Vec<&str> = mei.split("<layer n=\"2\">").skip(1).collect();
        assert_eq!(second.len(), 3, "{mei}");
        assert!(second[0].contains("<rest"), "its own bar is completed");
        assert!(second[1].starts_with("<mSpace/>"), "{}", second[1]);
        assert!(second[2].starts_with("<mSpace/>"), "{}", second[2]);
        // and read back, the voice is as long as what was written in it: the
        // filler is the page's
        let read = super::super::mei_to_sheet(&mei).expect("reads it");
        assert_eq!(read.staves[0].voices[1].len(), Ratio::new(1, 2));
    }

    #[test]
    fn three_in_the_time_of_two_is_a_bracketed_triplet() {
        // three triplet eighths fill a quarter: 1/12 each, which is exact as a
        // rational and impossible on any grid of 32nds
        let triplet = Ratio::new(1, 12);
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(vec![
                note(Step::C, triplet, 1),
                note(Step::D, triplet, 2),
                note(Step::E, triplet, 3),
                note(Step::F, Ratio::new(3, 4), 4),
            ])],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes a triplet");
        assert!(mei.contains("<tuplet num=\"3\" numbase=\"2\">"), "{mei}");
        // the notes inside are written as eighths -- their value, not their length
        let group = mei.split("<tuplet").nth(1).unwrap();
        assert_eq!(
            group[..group.find("</tuplet>").unwrap()]
                .matches("dur=\"8\"")
                .count(),
            3
        );
        assert_eq!(mei.matches("<measure").count(), 1);
    }

    #[test]
    fn a_quintuplet_is_five_in_the_time_of_four() {
        let fifth = Ratio::new(1, 20); // five in the time of four sixteenths
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice((0..5).map(|i| note(Step::C, fifth, i + 1)).collect())],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes a quintuplet");
        assert!(mei.contains("num=\"5\" numbase=\"4\""), "{mei}");
    }

    #[test]
    fn a_tuplet_that_would_cross_a_barline_is_refused_and_says_so() {
        // a 4/4 bar with a whole note in it, then a triplet: the group starts
        // where the bar ends, so it cannot be written whole
        let triplet = Ratio::new(1, 12);
        let mut mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(vec![
                note(Step::C, Ratio::new(7, 8), 1),
                note(Step::D, triplet, 2),
                note(Step::E, triplet, 3),
                note(Step::F, triplet, 4),
            ])],
            ..Staff::default()
        }]);
        mine.grid = Grid::uniform(4, 4);
        let err = sheet_to_mei(&mine).expect_err("refuses to split a tuplet");
        assert!(err.contains("cross the barline"), "{err}");
    }

    #[test]
    fn what_is_written_between_two_notes_hangs_off_the_measure() {
        use super::super::model::Spanner;
        let mut mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(vec![
                note(Step::C, Ratio::new(1, 2), 1),
                note(Step::G, Ratio::new(1, 2), 2),
            ])],
            ..Staff::default()
        }]);
        mine.spanners = vec![
            Spanner {
                kind: "slur".into(),
                from: 1,
                to: 2,
            },
            Spanner {
                kind: "crescendo".into(),
                from: 1,
                to: 2,
            },
        ];
        let mei = sheet_to_mei(&mine).expect("writes them");
        assert!(
            mei.contains("<slur xml:id=\"a-slur-1-2\" staff=\"1\" startid=\"#n1\" endid=\"#n2\"/>"),
            "{mei}"
        );
        assert!(mei.contains("<hairpin form=\"cres\""), "{mei}");
        // they are children of the measure, not of the note
        assert!(mei.contains("</staff><slur") || mei.contains("</staff><hairpin"));

        // one that names a note the score does not have is refused, rather than
        // quietly never appearing
        mine.spanners = vec![Spanner {
            kind: "slur".into(),
            from: 1,
            to: 99,
        }];
        let err = sheet_to_mei(&mine).expect_err("refuses a dangling end");
        assert!(
            err.contains("99") && err.contains("not in this score"),
            "{err}"
        );

        // and so is a kind this layer cannot write
        mine.spanners = vec![Spanner {
            kind: "zigzag".into(),
            from: 1,
            to: 2,
        }];
        let err = sheet_to_mei(&mine).expect_err("refuses an unknown kind");
        assert!(err.contains("slur, crescendo"), "{err}");
    }

    #[test]
    fn the_marks_a_note_carries_reach_the_page() {
        let marked = Item::Note {
            id: 1,
            pitches: vec![pitch(Step::C, 4)],
            dur: Ratio::new(1, 4),
            tie: false,
            marks: Marks {
                articulations: vec!["stacc".into()],
                dynamic: Some("mf".into()),
                ornament: None,
                grace: None,
                stem: Some("up".into()),
                // written a quarter, sounding an eighth -- kept in the model
                // and deliberately *not* written to the page
                sounding: Some(Ratio::new(1, 8)),
                ..Marks::default()
            },
        };
        let grace = Item::Note {
            id: 2,
            pitches: vec![pitch(Step::D, 4)],
            dur: Ratio::new(1, 8),
            tie: false,
            marks: Marks {
                grace: Some("acc".into()),
                ..Default::default()
            },
        };
        let mine = sheet(vec![Staff {
            clef: "G2".into(),
            voices: vec![voice(vec![
                marked,
                grace,
                note(Step::E, Ratio::new(5, 8), 3),
            ])],
            ..Staff::default()
        }]);
        let mei = sheet_to_mei(&mine).expect("writes the marks");
        assert!(mei.contains("<artic artic=\"stacc\"/>"), "{mei}");
        assert!(mei.contains("stem.dir=\"up\""));
        assert!(mei.contains("grace=\"acc\""));
        // The written value is all that reaches the page. A sounding length is
        // a performance fact, and writing it as `@dur.ges` made an engraver's
        // own clock advance by it -- shortening the note *and* pulling every
        // attack after it earlier, which is a corrupted performance rather than
        // a nuance. The model keeps it for the interpreter.
        assert!(mei.contains("dur=\"4\""));
        assert!(
            !mei.contains("dur.ges"),
            "a sounding length is not written: {mei}"
        );
        // a dynamic and an ornament hang off the measure, pointing at the note
        assert!(
            mei.contains(
                "<dynam xml:id=\"a-dynamic-n1\" staff=\"1\" startid=\"#n1\" place=\"below\">mf</dynam>"
            ),
            "{mei}"
        );
    }
}
