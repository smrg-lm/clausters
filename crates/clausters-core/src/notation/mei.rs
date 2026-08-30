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

use super::model::{Grid, Item, Marks, Pitch, Sheet, Staff, Voice};
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
        .enumerate()
        .map(|(i, slot)| {
            let id = i as u64 + 1;
            let dur = |ticks: &i32| Ratio::from_ticks(*ticks as i64, TPW as i64);
            match slot {
                Slot::Note { midis, ticks } if !midis.is_empty() => Item::Note {
                    id,
                    pitches: midis.iter().map(|&m| Pitch::from_midi(m, flats)).collect(),
                    dur: dur(ticks),
                    tie: false,
                    marks: Default::default(),
                },
                Slot::Note { ticks, .. } | Slot::Rest { ticks } => Item::Rest {
                    id,
                    dur: dur(ticks),
                },
            }
        })
        .collect();
    Sheet {
        next_id: voice.len() as u64 + 1,
        grid: Grid::uniform(num as i64, den as i64),
        key: key.to_string(),
        staves: vec![Staff {
            clef: clef.to_string(),
            voices: vec![Voice { items }],
        }],
        spanners: Vec::new(),
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
/// **Every element carries the id of the item it was written from** — the
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
    let (keysig, _) = key_signature(&sheet.key);
    let default_staff = Staff::default();
    let staves: Vec<&Staff> = if sheet.staves.is_empty() {
        vec![&default_staff]
    } else {
        sheet.staves.iter().collect()
    };
    let meter = sheet.grid.meter_at(0);
    let (num, den) = (meter.count, meter.unit);

    // How many measures the longest voice needs; at least one, so an empty
    // score still draws a bar of rests.
    let count = measure_count(sheet)?;
    // Which accidentals are drawn, and which measure each item falls in: both
    // are questions about a whole staff, so both are answered before any voice
    // is projected.
    let (printed, placed) = layout(sheet);
    let attached = attachments(sheet, &placed)?;

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
        for voice in voices {
            per_voice.push(project(voice, &sheet.grid, count, &printed)?);
        }
        projected.push(per_voice);
    }

    let body = (0..count)
        .map(|m| {
            let extra = attached.get(&m).map(String::as_str).unwrap_or("");
            measure_xml(m, &projected, extra, m + 1 == count)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // A single staff keeps the shape it always had; several take a brace, which
    // is what makes two staves read as one instrument rather than two.
    let defs = staves
        .iter()
        .enumerate()
        .map(|(i, staff)| {
            let (shape, line) = parse_clef(&staff.clef);
            format!(
                "<staffDef n=\"{}\" lines=\"5\" clef.shape=\"{shape}\" clef.line=\"{line}\"/>",
                i + 1
            )
        })
        .collect::<String>();
    let group = if staves.len() > 1 {
        format!("<staffGrp symbol=\"brace\" bar.thru=\"true\">{defs}</staffGrp>")
    } else {
        format!("<staffGrp>{defs}</staffGrp>")
    };

    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <mei xmlns=\"http://www.music-encoding.org/ns/mei\" meiversion=\"5.0\">\n\
         \x20<meiHead><fileDesc><titleStmt><title/></titleStmt>\
         <pubStmt/></fileDesc></meiHead>\n\
         \x20<music><body><mdiv><score>\n\
         \x20\x20<scoreDef meter.count=\"{num}\" meter.unit=\"{den}\" key.sig=\"{keysig}\">\n\
         \x20\x20\x20{group}\n\
         \x20\x20</scoreDef>\n\
         \x20\x20<section>\n{body}\n\x20\x20</section>\n\
         \x20</score></mdiv></body></music>\n\
         </mei>\n"
    ))
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
    /// `num` in the time of `numbase` — MEI's own way of putting it.
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
/// a tuplet — and that odd factor is how many notes are in the time of the
/// nearest power of two below it. A triplet eighth is `1/12`: the odd part of
/// 12 is 3, so it is 3 in the time of 2, and its *written* value is
/// `1/12 × 3/2 = 1/8`, an eighth. This is what having exact rationals is for:
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
/// each measure.
fn project(
    voice: &Voice,
    grid: &Grid,
    count: usize,
    printed: &std::collections::HashSet<(u64, usize)>,
) -> Result<Vec<Vec<String>>, String> {
    let mut measures: Vec<Vec<String>> = vec![Vec::new(); count.max(1)];
    let mut measure = 0;
    let mut pos = 0; // ticks into the current measure
    let mut bar = bar_ticks(grid, 0)?;
    // Whether the previous item tied into this one, so a tie the caller wrote
    // and a tie a barline forced compose instead of overwriting each other.
    let mut tied_in = false;

    for unit in units(&voice.items) {
        if pos == bar && measure + 1 < measures.len() {
            measure += 1;
            pos = 0;
            bar = bar_ticks(grid, measure)?;
        }
        match unit {
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
            // case, not the exception — an empty staff under a written one is
            // one long rest — so every full measure it covers is written this
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
                        for (value, dots) in pieces(take) {
                            measures[measure]
                                .push(element(item, value, dots, None, suffix, printed)?);
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
                // (value, dots, measure) for every piece the item spans.
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
                    for (value, dots) in pieces(take) {
                        specs.push((value, dots, measure));
                    }
                    pos += take;
                    remaining -= take;
                }
                let sounds = item.sounds();
                let tied_out = matches!(item, Item::Note { tie: true, .. });
                let n = specs.len();
                for (idx, (value, dots, m)) in specs.iter().copied().enumerate() {
                    // A piece opens a tie when it is not the last of a split
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
                    )?);
                }
                tied_in = tied_out && sounds;
            }
        }
    }

    // A voice that ran out before the score did keeps its place with rests, so
    // the staves stay aligned and no measure is left empty of everything.
    while pos < bar || measure + 1 < measures.len() {
        if pos == bar {
            measure += 1;
            pos = 0;
            bar = bar_ticks(grid, measure)?;
            continue;
        }
        if pos == 0 {
            // the same rule for the emitter's own filler: a whole measure of
            // silence is one centred `<mRest/>`
            measures[measure].push("<mRest/>".to_string());
        } else {
            let rest = Item::Rest {
                id: 0,
                dur: Ratio::ZERO,
            };
            for (value, dots) in pieces(bar - pos) {
                measures[measure].push(element(&rest, value, dots, None, None, printed)?);
            }
        }
        pos = bar;
    }
    Ok(measures)
}

/// MEI's `@tie` from the two facts a piece knows: whether a tie starts here and
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
) -> String {
    let right = if last { " right=\"end\"" } else { "" };
    let staves: String = projected
        .iter()
        .enumerate()
        .map(|(si, voices)| {
            let layers: String = voices
                .iter()
                .enumerate()
                .map(|(vi, measures)| {
                    let cells = measures.get(index).map(|c| c.concat()).unwrap_or_default();
                    format!("<layer n=\"{}\">{cells}</layer>", vi + 1)
                })
                .collect();
            format!("<staff n=\"{}\">{layers}</staff>", si + 1)
        })
        .collect();
    format!(
        "   <measure n=\"{}\"{right}>{staves}{attached}</measure>",
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

/// What hangs off a measure rather than off a note: a dynamic, an ornament, and
/// the two-ended things — a slur, a hairpin.
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
fn attachments(
    sheet: &Sheet,
    placed: &std::collections::HashMap<u64, (usize, usize)>,
) -> Result<std::collections::HashMap<usize, String>, String> {
    let mut out: std::collections::HashMap<usize, String> = std::collections::HashMap::new();

    for staff in &sheet.staves {
        for item in staff.voices.iter().flat_map(|v| v.items.iter()) {
            let Some(marks) = item.marks() else { continue };
            let Some(&(measure, si)) = placed.get(&item.id()) else {
                continue;
            };
            let at = format!(" staff=\"{}\" startid=\"#n{}\"", si + 1, item.id());
            if let Some(dynamic) = &marks.dynamic {
                out.entry(measure)
                    .or_default()
                    .push_str(&format!("<dynam{at} place=\"below\">{dynamic}</dynam>"));
            }
            if let Some(ornament) = &marks.ornament {
                // An ornament is its own element in MEI, named for what it is.
                out.entry(measure)
                    .or_default()
                    .push_str(&format!("<{ornament}{at}/>"));
            }
        }
    }

    for spanner in &sheet.spanners {
        let &(measure, si) = placed.get(&spanner.from).ok_or_else(|| {
            format!(
                "a {} starts on item {}, which is not in this score",
                spanner.kind, spanner.from
            )
        })?;
        if !placed.contains_key(&spanner.to) {
            return Err(format!(
                "a {} ends on item {}, which is not in this score",
                spanner.kind, spanner.to
            ));
        }
        let ends = format!(
            " staff=\"{}\" startid=\"#n{}\" endid=\"#n{}\"",
            si + 1,
            spanner.from,
            spanner.to
        );
        let xml = match spanner.kind.as_str() {
            "slur" => format!("<slur{ends}/>"),
            "crescendo" => format!("<hairpin form=\"cres\"{ends}/>"),
            "diminuendo" => format!("<hairpin form=\"dim\"{ends}/>"),
            other => {
                return Err(format!(
                    "\"{other}\" is not something this layer knows how to write \
                     between two notes; it writes a slur, a crescendo and a \
                     diminuendo"
                ));
            }
        };
        out.entry(measure).or_default().push_str(&xml);
    }
    Ok(out)
}

/// Which steps a key signature alters, indexed by step (`C` = 0 … `B` = 6).
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
/// — including where the key signature already implies it and where the same
/// note was altered earlier in the bar — while `@accid.ges` is never drawn at
/// all. So an F sharp in C major written as the sounding form comes out as a
/// plain F: a wrong score that looks right, which is the one failure this layer
/// must never produce.
///
/// The decision is therefore ours, and it needs both halves: the **key
/// signature**, and a **per-measure memory** of what has already been printed.
/// An accidental holds for the rest of its measure at its own step and octave,
/// and a new measure starts again from the armature — the ordinary convention,
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
    let (keysig, _) = key_signature(&sheet.key);
    let armature = key_alterations(keysig);
    let mut out = std::collections::HashSet::new();
    let mut where_ = std::collections::HashMap::new();

    for (si, staff) in sheet.staves.iter().enumerate() {
        // Every sounding item of the staff, in the order a reader meets them.
        let mut timed: Vec<(Ratio, usize, &Item)> = Vec::new();
        for (vi, voice) in staff.voices.iter().enumerate() {
            let mut onset = Ratio::ZERO;
            for item in &voice.items {
                if item.sounds() {
                    timed.push((onset, vi, item));
                }
                onset = onset + item.dur();
            }
        }
        timed.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        let mut measure = usize::MAX;
        let mut printed: std::collections::HashMap<(i32, i32), i32> =
            std::collections::HashMap::new();
        for (onset, _, item) in timed {
            let (m, _) = sheet.grid.position(onset);
            if m != measure {
                measure = m;
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
/// `n7` is item 7; `n7-2` is the second piece of an item split across a
/// barline; `n7-p1` is the first pitch of a chord. Every one of them maps back
/// to exactly one item, which is what a gesture on the page needs and what a
/// re-engraving has to preserve. Item `0` is the emitter's own filler — a rest
/// written to keep a short voice in step — and carries no id at all, since
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
) -> Result<String, String> {
    let d = if dots != 0 { " dots=\"1\"" } else { "" };
    let id = element_id(item.id(), suffix);
    let marks = item.marks();
    // **A sounding length never reaches the page.** `@dur.ges` looks like the
    // way to say "written a quarter, sounds an eighth", and it is not: an
    // engraver reads it as the note's real duration and advances its own clock
    // by it, so a staccato quarter written that way does not merely sound short
    // — every attack after it moves a quarter-beat earlier and the measure comes
    // out short. Shortening a staccato is a *performance* decision and belongs
    // to whoever plays the page. The model keeps the fact
    // ([`super::model::Marks::sounding`]); the interpreter is what honours it.
    Ok(match item.pitches() {
        // Nothing to sound draws as a rest, however the caller spelled it.
        [] => format!("<rest{id} dur=\"{value}\"{d}/>"),
        // Only the first piece of a split item prints its accidental: the tie
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
                    )
                })
                .collect::<Result<String, _>>()?;
            let inner = format!("{}{inner}", articulations_xml(marks));
            format!(
                "<chord{id} dur=\"{value}\"{d}{}>{inner}</chord>",
                stem_xml(marks)
            )
        }
    })
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
    let inner = format!("{accid}{}", articulations_xml(marks));
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
        assert!(
            mei.contains("<rest xml:id=\"n1\" dur=\"4\"/>"),
            "a quarter rest"
        );
        assert!(!mei.contains("<chord"), "never an empty chord");
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
    /// existed — the model's own acceptance was that not one byte moved — and
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
    ///   **centred in the bar**, where a reader looks for it — a run of values
    ///   adding up to a measure hangs at its start instead.
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
            },
            Staff {
                clef: "F4".into(),
                voices: vec![voice(vec![note(Step::C, Ratio::ONE, 2)])],
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
                voice(vec![note(Step::C, Ratio::from(2), 1)]),
                voice(vec![note(Step::E, Ratio::ONE, 2)]),
            ],
        }]);
        let mei = sheet_to_mei(&mine).expect("writes it");
        assert!(mei.contains("<mRest/>"), "{mei}");
        assert!(
            !mei.contains("<rest dur=\"1\""),
            "not a decomposed whole rest"
        );
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
            mei.contains("<slur staff=\"1\" startid=\"#n1\" endid=\"#n2\"/>"),
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
            kind: "octave".into(),
            from: 1,
            to: 2,
        }];
        let err = sheet_to_mei(&mine).expect_err("refuses an unknown kind");
        assert!(err.contains("slur, a crescendo"), "{err}");
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
            mei.contains("<dynam staff=\"1\" startid=\"#n1\" place=\"below\">mf</dynam>"),
            "{mei}"
        );
    }
}
