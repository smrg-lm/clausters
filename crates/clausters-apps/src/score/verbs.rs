//! **The editor's verbs**: what a hand asks of the selected items, as model
//! operations.
//!
//! A verb is asked of the **selection** -- the items a hand picked on the page
//! -- and reads the sheet as it stands, because most of them depend on what is
//! already written: an articulation is toggled against the ones the note has,
//! a length scaled from the one it is written with, a note moved into the
//! *other* voice. That reading is what every client would otherwise write for
//! itself, so it is here, once, and a client's editor calls it by name.
//!
//! Everything here is pure: a sheet and a selection in, the operations out,
//! applied by the editor as **one** step of the history however many there
//! are.

use serde::Deserialize;
use serde_json::{Map, Value};

use clausters_core::notation::{
    FIELDS, Halign, Item, Marks, NOTE, Op, PageSetup, Pages, Pitch, Region, Sheet, Valign,
    default_place, paper,
};
use clausters_core::ratio::Ratio;

/// **One verb, as a client names it**: `{"action": ..., <its arguments>}`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Move the selected notes `steps` diatonic steps along their staves, up
    /// when positive -- each takes the key signature's alteration for the
    /// letter it lands on.
    Move { steps: i32 },
    /// Scale the selected items' written values by `factor` (`[2, 1]` is twice
    /// as long), against the barlines already there.
    Scale { factor: Ratio },
    /// Give the selected notes an articulation (by its MEI name), or take it
    /// away when every one of them already has it.
    Articulation { name: String },
    /// Put a dynamic under the first selected note, or take it away with none.
    Dynamic {
        #[serde(default)]
        name: Option<String>,
    },
    /// Give the selected notes an ornament, or take it away with none.
    Ornament {
        #[serde(default)]
        name: Option<String>,
    },
    /// Take every mark off the selected notes.
    ClearMarks,
    /// Tie the selected notes to the next, or untie them when the first is
    /// tied already.
    Tie,
    /// Turn the selected notes into rests of the same length.
    Silence,
    /// Remove the selected items; what follows them moves earlier.
    Delete,
    /// Move the selection to the other voice of its staff, or to voice `to`
    /// (from zero) when one is named; rests are left where it was.
    Voice {
        #[serde(default)]
        to: Option<usize>,
    },
    /// **Open another document in this editor**: `data` is the document's
    /// text, in any format the engraver reads (MEI, MusicXML, ABC, Plaine &
    /// Easie), and it replaces the score whole -- as one entry, so the score
    /// that was there is a step back.
    Open { data: String },
    /// Make the selected notes grace notes -- `acc`, an appoggiatura, or
    /// `unacc`, an acciaccatura -- or notes of the bar again, with none.
    Grace {
        #[serde(default)]
        kind: Option<String>,
    },
    /// Give the selected notes an accidental: `alter` semitones from the
    /// letter (`1` a sharp, `-1` a flat, `0` a natural), printed whatever the
    /// key says.
    Accidental { alter: i32 },
    /// A slur, a crescendo or a diminuendo from the first selected item to the
    /// last, in time.
    Spanner { kind: String },
    /// A model operation, whole -- the escape hatch for what has no verb here.
    Op { op: Box<Op> },
    /// **Lay the score out on another page**: a `paper` by name (turned with
    /// `landscape`), or a `width` and `height` of its own, the `margins` (top,
    /// right, bottom, left) and the `staff` height -- lengths in tenths of a
    /// millimetre, the staff in hundredths. What is left out stays as it is.
    Page {
        #[serde(default)]
        paper: Option<String>,
        #[serde(default)]
        landscape: Option<bool>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        margins: Option<[u32; 4]>,
        #[serde(default)]
        staff: Option<u32>,
    },
    /// **Open or take out measures at the selection**: `insert_before` and
    /// `insert_after` open `count` empty ones (one, left out) before the first
    /// selected measure or after the last, and `remove` takes out the measures
    /// the selection covers, with what is written in them.
    Measures {
        edit: String,
        #[serde(default)]
        count: Option<usize>,
    },
    /// Give the last selected measure a right barline: `single`, `dbl`, `end`,
    /// `rptstart`, `rptend`, `rptboth` or `invis`.
    Barline { kind: String },
    /// Break the line or the page before the first selected measure
    /// (`system`, `page`), or take the break back (`none`).
    Break { kind: String },
    /// Change the meter from the first selected measure on: `count` beats of
    /// `unit` (3 and 4 is three quarters).
    Meter { count: i64, unit: i64 },
    /// **Write a text of the page, or move it**: `field` is `title`,
    /// `subtitle`, `composer`, `arranger`, `lyricist`, `translator`,
    /// `copyright` or `note` (a footnote: `index` says which, from zero, and
    /// none adds one). `text` writes it -- empty takes it away -- and `region`
    /// (`head`, `foot`), `halign`, `valign` and `pages` (`first`, `all`) put it
    /// in a cell; what is left out stays as it is.
    Text {
        field: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        index: Option<usize>,
        #[serde(default)]
        region: Option<Region>,
        #[serde(default)]
        halign: Option<Halign>,
        #[serde(default)]
        valign: Option<Valign>,
        #[serde(default)]
        pages: Option<Pages>,
    },
    /// **A mark on the selected notes**, by its field of the model's marks:
    /// `tremolo` (strokes, 1 to 3), `arpeggio` (`up`, `down`), `breath`
    /// (`breath`, `caesura`), `ring` (true), `fingering` and `harmony` (text).
    /// A value every selected note already has takes it away, and so does
    /// none.
    Mark {
        mark: String,
        #[serde(default)]
        value: Value,
    },
    /// A syllable of the lyrics under the first selected note, in verse
    /// `verse` (from 1); empty takes it away.
    Lyric {
        #[serde(default = "first_verse")]
        verse: usize,
        #[serde(default)]
        text: String,
    },
    /// Draw the selected notes as **repeats of the beat before** each, which
    /// they then hold; again, as themselves.
    BeatRepeat,
    /// **Write at the first selected item** a `tempo` (with its `bpm`), a
    /// `dir` or a `reh`; with no text and no speed, take it back.
    Control {
        kind: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        bpm: Option<f64>,
    },
    /// Change the key from the first selected measure on, or take a change
    /// back with `none`.
    Key { key: String },
    /// Change the clef where the first selected item starts, on its staff, or
    /// take the change back with `none`.
    Clef { clef: String },
    /// Mark the selected measures as an ending played in the passes `label`
    /// names; empty takes it back.
    Ending {
        #[serde(default)]
        label: String,
    },
    /// A navigation mark: `segno` and `coda` on the first selected measure,
    /// `fine`, `dacapo`, `dalsegno` and `tocoda` on the last; `none` takes
    /// them off both.
    Navigation { kind: String },
    /// Write each selected measure as a repeat of the one before, or as
    /// itself again when every one already is.
    MeasureRepeat,
    /// Draw runs of empty measures as one numbered rest, or each as itself.
    Multirests,
    /// Say what the selected staves are -- the first staff, with nothing
    /// selected: their `lines`, their name (`label`, `abbr`), their
    /// transposition in semitones.
    Staff {
        #[serde(default)]
        lines: Option<u8>,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        abbr: Option<String>,
        #[serde(default)]
        transpose: Option<i32>,
    },
    /// Group the staves the selection covers under a `brace`, a `bracket` or
    /// a `line`; `none` takes away the groups over them.
    Group { symbol: String },
    /// **A transformation over the measures the selection covers** -- or over
    /// everything, with nothing selected: `transpose` (`semitones`, `steps`),
    /// `invert` (`axis`), `retrograde`, `stretch` (`factor`) or `repeat`
    /// (`count`), its parameters beside the name.
    Transform {
        name: String,
        #[serde(flatten)]
        params: Map<String, Value>,
    },
}

fn first_verse() -> usize {
    1
}

/// The transformations a selection's span is handed to.
pub const TRANSFORMS: &[&str] = &["transpose", "invert", "retrograde", "stretch", "repeat"];

impl Action {
    /// What an undo menu calls it.
    pub fn label(&self) -> String {
        match self {
            Action::Move { steps } if *steps > 0 => "move up".into(),
            Action::Move { .. } => "move down".into(),
            Action::Scale { .. } => "change the length".into(),
            Action::Articulation { name } => format!("articulation {name}"),
            Action::Dynamic { .. } => "dynamic".into(),
            Action::Ornament { .. } => "ornament".into(),
            Action::ClearMarks => "clear the marks".into(),
            Action::Tie => "tie".into(),
            Action::Silence => "silence".into(),
            Action::Delete => "delete".into(),
            Action::Voice { .. } => "move to the other voice".into(),
            Action::Accidental { .. } => "accidental".into(),
            Action::Grace { .. } => "grace note".into(),
            Action::Open { .. } => "open".into(),
            Action::Spanner { kind } => kind.clone(),
            Action::Transform { name, .. } => name.clone(),
            Action::Page { .. } => "page setup".into(),
            Action::Measures { edit, .. } if edit == "remove" => "remove measures".into(),
            Action::Measures { .. } => "insert measures".into(),
            Action::Barline { .. } => "barline".into(),
            Action::Break { .. } => "break".into(),
            Action::Meter { .. } => "meter".into(),
            Action::Text { field, .. } => format!("page text: {field}"),
            Action::Mark { mark, .. } => mark.clone(),
            Action::Lyric { .. } => "lyrics".into(),
            Action::BeatRepeat => "beat repeat".into(),
            Action::Control { kind, .. } => match kind.as_str() {
                "tempo" => "tempo".into(),
                "reh" => "rehearsal mark".into(),
                _ => "direction".into(),
            },
            Action::Key { .. } => "key".into(),
            Action::Clef { .. } => "clef".into(),
            Action::Ending { .. } => "ending".into(),
            Action::Navigation { kind } => kind.clone(),
            Action::MeasureRepeat => "measure repeat".into(),
            Action::Multirests => "multirests".into(),
            Action::Staff { .. } => "staff".into(),
            Action::Group { .. } => "staff group".into(),
            Action::Op { op } => serde_json::to_value(op)
                .ok()
                .and_then(|v| v.get("op").and_then(|o| o.as_str()).map(str::to_string))
                .unwrap_or_else(|| "edit the score".into()),
        }
    }
}

/// Where one item sits: its staff, its voice, its onset in whole notes, and the
/// item itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Located<'a> {
    pub staff: usize,
    pub voice: usize,
    pub onset: Ratio,
    pub item: &'a Item,
}

/// **Where the item `id` sits** in `sheet`, or `None` when no item has it.
pub fn locate(sheet: &Sheet, id: u64) -> Option<Located<'_>> {
    for (staff, s) in sheet.staves.iter().enumerate() {
        for (voice, v) in s.voices.iter().enumerate() {
            let mut onset = Ratio::ZERO;
            for item in &v.items {
                if item.id() == id {
                    return Some(Located {
                        staff,
                        voice,
                        onset,
                        item,
                    });
                }
                onset = onset + item.dur();
            }
        }
    }
    None
}

/// **The selection in time order**: the ids that name items, earliest first
/// (then by staff and voice), each once.
pub fn in_time(sheet: &Sheet, selection: &[u64]) -> Vec<u64> {
    let mut found: Vec<(Ratio, usize, usize, u64)> = selection
        .iter()
        .filter_map(|&id| locate(sheet, id).map(|l| (l.onset, l.staff, l.voice, id)))
        .collect();
    found.sort();
    found.dedup_by_key(|f| f.3);
    found.into_iter().map(|f| f.3).collect()
}

/// **The operations `action` comes to** over `selection` in `sheet`, in the
/// order they are applied -- or why there are none.
pub fn ops(sheet: &Sheet, selection: &[u64], action: &Action) -> Result<Vec<Op>, String> {
    if let Action::Op { op } = action {
        return Ok(vec![(**op).clone()]);
    }
    if let Action::Transform { name, params } = action {
        return transform(sheet, selection, name, params).map(|op| vec![op]);
    }
    if let Action::Text {
        field,
        text,
        index,
        region,
        halign,
        valign,
        pages,
    } = action
    {
        let mut header = sheet.header.clone();
        if field == NOTE {
            if let Some(text) = text {
                match index {
                    Some(at) if *at < header.notes.len() && text.is_empty() => {
                        header.notes.remove(*at);
                    }
                    Some(at) if *at < header.notes.len() => header.notes[*at] = text.clone(),
                    Some(at) => return Err(format!("there is no footnote {}", at + 1)),
                    None if text.is_empty() => {}
                    None => header.notes.push(text.clone()),
                }
            }
        } else {
            let slot = header.text_mut(field).ok_or_else(|| {
                format!(
                    "there is no page text called {field}; it is one of {}, {NOTE}",
                    FIELDS.join(", ")
                )
            })?;
            if let Some(text) = text {
                *slot = text.clone();
            }
        }
        if region.is_some() || halign.is_some() || valign.is_some() || pages.is_some() {
            let mut place = header.place(field);
            place.region = region.unwrap_or(place.region);
            place.halign = halign.unwrap_or(place.halign);
            place.valign = valign.unwrap_or(place.valign);
            place.pages = pages.unwrap_or(place.pages);
            if place == default_place(field) {
                header.places.remove(field);
            } else {
                header.places.insert(field.clone(), place);
            }
        }
        return Ok(vec![Op::SetHeader { header }]);
    }
    if let Action::Page {
        paper: name,
        landscape,
        width,
        height,
        margins,
        staff,
    } = action
    {
        let mut page = sheet.page.unwrap_or_default();
        // a paper is named portrait; which way up it goes is said beside it,
        // and stays as it was where it is not said
        let turned = landscape.unwrap_or(page.landscape());
        if let Some(name) = name {
            let found = paper(name).ok_or_else(|| format!("there is no paper called {name}"))?;
            page = PageSetup {
                margins: page.margins,
                staff: page.staff,
                ..PageSetup::on(found, turned)
            };
        } else if turned != page.landscape() {
            (page.width, page.height) = (page.height, page.width);
        }
        page.width = width.unwrap_or(page.width);
        page.height = height.unwrap_or(page.height);
        page.margins = margins.unwrap_or(page.margins);
        page.staff = staff.unwrap_or(page.staff);
        return Ok(vec![Op::SetPage { page: Some(page) }]);
    }
    let ids = in_time(sheet, selection);
    if let Action::Multirests = action {
        return Ok(vec![Op::SetMultirests {
            on: !sheet.grid.multirests,
        }]);
    }
    // the staves the selection covers, or the first
    let staves: Vec<usize> = {
        let mut on: Vec<usize> = ids
            .iter()
            .filter_map(|&id| locate(sheet, id).map(|l| l.staff))
            .collect();
        on.sort_unstable();
        on.dedup();
        if on.is_empty() { vec![0] } else { on }
    };
    if let Action::Staff {
        lines,
        label,
        abbr,
        transpose,
    } = action
    {
        return Ok(staves
            .iter()
            .map(|&staff| Op::SetStaff {
                staff,
                lines: *lines,
                label: label.clone(),
                abbr: abbr.clone(),
                transpose: *transpose,
            })
            .collect());
    }
    if let Action::Group { symbol } = action {
        let (a, b) = (staves[0], staves[staves.len() - 1]);
        let mut groups: Vec<clausters_core::notation::Group> = sheet
            .groups
            .iter()
            .filter(|g| g.last < a || g.first > b)
            .cloned()
            .collect();
        if symbol != "none" {
            groups.push(clausters_core::notation::Group {
                first: a,
                last: b,
                symbol: symbol.clone(),
            });
            groups.sort_by_key(|g| (g.first, std::cmp::Reverse(g.last)));
        }
        return Ok(vec![Op::SetGroups { groups }]);
    }
    if ids.is_empty() {
        return Err("select a note first".into());
    }
    // the measures the selection covers, 1-based, for the verbs asked of them
    let measure_of = |id: u64| locate(sheet, id).map(|l| sheet.grid.position(l.onset).0 + 1);
    let (first, last) = (
        ids.first().copied().and_then(measure_of).unwrap_or(1),
        ids.last().copied().and_then(measure_of).unwrap_or(1),
    );
    match action {
        Action::Measures { edit, count } => {
            let count = count.unwrap_or(1);
            return match edit.as_str() {
                "insert_before" => Ok(vec![Op::InsertMeasures { at: first, count }]),
                "insert_after" => Ok(vec![Op::InsertMeasures {
                    at: last + 1,
                    count,
                }]),
                "remove" => Ok(vec![Op::RemoveMeasures { first, last }]),
                other => Err(format!(
                    "measures are opened with insert_before or insert_after and taken out with remove, not {other}"
                )),
            };
        }
        Action::Barline { kind } => {
            return Ok(vec![Op::SetBarline {
                measure: last,
                kind: kind.clone(),
            }]);
        }
        Action::Break { kind } => {
            return Ok(vec![Op::SetBreak {
                measure: first,
                kind: kind.clone(),
            }]);
        }
        Action::Meter { count, unit } => {
            return Ok(vec![Op::SetMeter {
                measure: first,
                count: *count,
                unit: *unit,
            }]);
        }
        Action::Key { key } => {
            return Ok(vec![Op::SetKey {
                measure: first,
                key: key.clone(),
            }]);
        }
        Action::Ending { label } => {
            return Ok(vec![Op::SetEnding {
                first,
                last,
                label: label.clone(),
            }]);
        }
        Action::Navigation { kind } => {
            return Ok(match kind.as_str() {
                "segno" | "coda" => vec![Op::SetMark {
                    measure: first,
                    kind: kind.clone(),
                }],
                "none" => {
                    let mut ops = vec![Op::SetMark {
                        measure: first,
                        kind: "none".into(),
                    }];
                    if last != first {
                        ops.push(Op::SetMark {
                            measure: last,
                            kind: "none".into(),
                        });
                    }
                    ops
                }
                _ => vec![Op::SetMark {
                    measure: last,
                    kind: kind.clone(),
                }],
            });
        }
        Action::MeasureRepeat => {
            let measures: Vec<usize> = (first.max(2)..=last).collect();
            if measures.is_empty() {
                return Err("the first measure has no measure before it to repeat".into());
            }
            let on = !measures
                .iter()
                .all(|m| sheet.grid.repeats.contains(&(m - 1)));
            return Ok(measures
                .into_iter()
                .map(|measure| Op::SetRepeat { measure, on })
                .collect());
        }
        Action::Clef { clef } => {
            let at = locate(sheet, ids[0]).ok_or("select a note first")?;
            return Ok(vec![Op::SetClef {
                staff: at.staff,
                at: at.onset,
                clef: clef.clone(),
            }]);
        }
        Action::Control { kind, text, bpm } => {
            let on = ids[0];
            return Ok(if text.trim().is_empty() && bpm.is_none() {
                vec![Op::RemoveControl {
                    kind: kind.clone(),
                    on,
                }]
            } else {
                vec![Op::AddControl {
                    kind: kind.clone(),
                    on,
                    text: text.trim().to_string(),
                    bpm: *bpm,
                }]
            });
        }
        _ => {}
    }
    let notes: Vec<u64> = ids
        .iter()
        .copied()
        .filter(|&id| locate(sheet, id).is_some_and(|l| l.item.sounds()))
        .collect();
    let marks_of = |id: u64| {
        locate(sheet, id)
            .and_then(|l| l.item.marks().cloned())
            .unwrap_or_default()
    };
    let need_notes = |what: &str| {
        if notes.is_empty() {
            Err(format!("{what} goes on a note, and none is selected"))
        } else {
            Ok(())
        }
    };
    Ok(match action {
        Action::Move { steps } => {
            need_notes("a move")?;
            notes
                .iter()
                .map(|&id| Op::MoveSteps { id, steps: *steps })
                .collect()
        }
        Action::Scale { factor } => {
            if !factor.is_positive() {
                return Err("a length is scaled by a positive factor".into());
            }
            ids.iter()
                .filter_map(|&id| {
                    let dur = locate(sheet, id)?.item.dur();
                    Some(Op::SetDur {
                        id,
                        dur: dur * *factor,
                    })
                })
                .collect()
        }
        Action::Articulation { name } => {
            need_notes("an articulation")?;
            let all = notes
                .iter()
                .all(|&id| marks_of(id).articulations.contains(name));
            notes
                .iter()
                .map(|&id| {
                    let mut marks = marks_of(id);
                    marks.articulations.retain(|a| a != name);
                    if !all {
                        marks.articulations.push(name.clone());
                    }
                    Op::SetMarks { id, marks }
                })
                .collect()
        }
        Action::Dynamic { name } => {
            need_notes("a dynamic")?;
            let id = notes[0];
            let mut marks = marks_of(id);
            marks.dynamic = name.clone();
            vec![Op::SetMarks { id, marks }]
        }
        Action::Ornament { name } => {
            need_notes("an ornament")?;
            notes
                .iter()
                .map(|&id| {
                    let mut marks = marks_of(id);
                    marks.ornament = name.clone();
                    Op::SetMarks { id, marks }
                })
                .collect()
        }
        Action::ClearMarks => {
            need_notes("a mark")?;
            notes
                .iter()
                .map(|&id| Op::SetMarks {
                    id,
                    marks: Marks::default(),
                })
                .collect()
        }
        Action::Tie => {
            need_notes("a tie")?;
            let tied = !matches!(
                locate(sheet, notes[0]).map(|l| l.item),
                Some(Item::Note { tie: true, .. })
            );
            notes.iter().map(|&id| Op::Tie { id, tied }).collect()
        }
        Action::Silence => {
            need_notes("silencing")?;
            notes.iter().map(|&id| Op::Silence { id }).collect()
        }
        Action::Delete => ids.iter().map(|&id| Op::Delete { id }).collect(),
        Action::Voice { to } => {
            let first = locate(sheet, ids[0]).map(|l| l.voice).unwrap_or(0);
            let voice = to.unwrap_or(if first == 0 { 1 } else { 0 });
            if voice == first {
                // already there: the state asked for holds
                return Ok(Vec::new());
            }
            vec![Op::ToVoice {
                ids: ids.clone(),
                voice,
            }]
        }
        Action::Grace { kind } => {
            need_notes("a grace note")?;
            if let Some(kind) = kind
                && !matches!(kind.as_str(), "acc" | "unacc")
            {
                return Err(format!(
                    "a grace note is acc, an appoggiatura, or unacc, an acciaccatura, not {kind}"
                ));
            }
            notes
                .iter()
                .map(|&id| {
                    let mut marks = marks_of(id);
                    marks.grace = kind.clone();
                    Op::SetMarks { id, marks }
                })
                .collect()
        }
        Action::Accidental { alter } => {
            need_notes("an accidental")?;
            if !(-2..=2).contains(alter) {
                return Err(format!(
                    "an accidental is two flats to two sharps, -2 to 2, not {alter}"
                ));
            }
            notes
                .iter()
                .filter_map(|&id| {
                    let pitches = locate(sheet, id)?
                        .item
                        .pitches()
                        .iter()
                        .map(|p| Pitch {
                            alter: *alter,
                            forced: true,
                            ..*p
                        })
                        .collect();
                    Some(Op::SetPitches { id, pitches })
                })
                .collect()
        }
        Action::Spanner { kind } => {
            let (Some(&from), Some(&to)) = (ids.first(), ids.last()) else {
                return Err("select where it starts and where it ends".into());
            };
            if from == to {
                return Err(format!(
                    "a {kind} runs between two notes: select where it starts and where it ends"
                ));
            }
            vec![Op::AddSpanner {
                kind: kind.clone(),
                from,
                to,
            }]
        }
        Action::Mark { mark, value } => {
            need_notes("a mark")?;
            let set = |marks: &mut Marks, value: &Value| -> Result<(), String> {
                let text = || value.as_str().map(str::to_string).filter(|t| !t.is_empty());
                match mark.as_str() {
                    "tremolo" => {
                        marks.tremolo = match value.as_u64() {
                            Some(n @ 1..=3) => Some(n as u8),
                            None if value.is_null() => None,
                            _ => return Err("a tremolo has one to three strokes".into()),
                        }
                    }
                    "arpeggio" => marks.arpeggio = text(),
                    "breath" => marks.breath = text(),
                    "ring" => marks.ring = value.as_bool().unwrap_or(false),
                    "fingering" => marks.fingering = text(),
                    "harmony" => marks.harmony = text(),
                    other => {
                        return Err(format!(
                            "there is no mark called {other}; it is tremolo, arpeggio, breath, \
                             ring, fingering or harmony"
                        ));
                    }
                }
                Ok(())
            };
            // a value every note already has is taken away
            let all = notes.iter().all(|&id| {
                let mut marks = marks_of(id);
                let before = marks.clone();
                set(&mut marks, value).is_ok() && marks == before
            });
            let value = if all && !value.is_null() {
                match value {
                    Value::Bool(_) => Value::Bool(false),
                    _ => Value::Null,
                }
            } else {
                value.clone()
            };
            notes
                .iter()
                .map(|&id| {
                    let mut marks = marks_of(id);
                    set(&mut marks, &value)?;
                    Ok(Op::SetMarks { id, marks })
                })
                .collect::<Result<Vec<_>, String>>()?
        }
        Action::Lyric { verse, text } => {
            need_notes("a syllable")?;
            let id = notes[0];
            let mut marks = marks_of(id);
            let at = verse.max(&1) - 1;
            while marks.lyrics.len() <= at {
                marks.lyrics.push(String::new());
            }
            marks.lyrics[at] = text.trim().to_string();
            while marks.lyrics.last().is_some_and(String::is_empty) {
                marks.lyrics.pop();
            }
            vec![Op::SetMarks { id, marks }]
        }
        Action::BeatRepeat => {
            need_notes("a beat repeat")?;
            let all = notes.iter().all(|&id| marks_of(id).beat_repeat);
            let mut ops = Vec::new();
            for &id in &notes {
                let mut marks = marks_of(id);
                marks.beat_repeat = !all;
                if !all {
                    // what it repeats: the item before it, in its voice
                    let at = locate(sheet, id).ok_or("select a note first")?;
                    let before = sheet.staves[at.staff].voices[at.voice]
                        .items
                        .iter()
                        .take_while(|i| i.id() != id)
                        .last()
                        .filter(|i| i.sounds() && i.dur() == at.item.dur())
                        .ok_or_else(|| {
                            "a beat repeat repeats the note before it, of the same value"
                                .to_string()
                        })?;
                    ops.push(Op::SetPitches {
                        id,
                        pitches: before.pitches().to_vec(),
                    });
                }
                ops.push(Op::SetMarks { id, marks });
            }
            ops
        }
        Action::Open { .. } => {
            unreachable!("a document is opened by the editor, which holds the engraver")
        }
        Action::Op { .. }
        | Action::Transform { .. }
        | Action::Page { .. }
        | Action::Text { .. }
        | Action::Measures { .. }
        | Action::Barline { .. }
        | Action::Break { .. }
        | Action::Meter { .. }
        | Action::Key { .. }
        | Action::Clef { .. }
        | Action::Ending { .. }
        | Action::Navigation { .. }
        | Action::MeasureRepeat
        | Action::Multirests
        | Action::Staff { .. }
        | Action::Group { .. }
        | Action::Control { .. } => {
            unreachable!("answered above")
        }
    })
}

/// **The transformation `name` over the span the selection covers**: the
/// measures from the one its earliest item starts in to the one its latest
/// starts in, or everything with nothing selected.
fn transform(
    sheet: &Sheet,
    selection: &[u64],
    name: &str,
    params: &Map<String, Value>,
) -> Result<Op, String> {
    if !TRANSFORMS.contains(&name) {
        return Err(format!(
            "there is no transformation called {name}; it is one of {}",
            TRANSFORMS.join(", ")
        ));
    }
    let onsets: Vec<Ratio> = in_time(sheet, selection)
        .into_iter()
        .filter_map(|id| locate(sheet, id).map(|l| l.onset))
        .collect();
    let mut op = params.clone();
    op.insert("op".into(), Value::from(name));
    if let (Some(first), Some(last)) = (onsets.iter().min(), onsets.iter().max()) {
        let first = sheet.grid.position(*first).0 + 1;
        let last = sheet.grid.position(*last).0 + 1;
        op.insert(
            "span".into(),
            serde_json::json!({ "measures": [first, last] }),
        );
    }
    serde_json::from_value(Value::Object(op)).map_err(|why| format!("{name}: {why}"))
}

/// **The items of measure `measure`** (1-based), on staff `staff` (1-based)
/// or on every staff -- each item that starts inside the bar, in every voice.
pub fn measure_items(sheet: &Sheet, measure: usize, staff: Option<usize>) -> Vec<u64> {
    if measure == 0 {
        return Vec::new();
    }
    let (start, end) = sheet.grid.span(measure - 1, measure - 1);
    items_where(sheet, |at| {
        at.onset >= start && at.onset < end && staff.is_none_or(|s| at.staff + 1 == s)
    })
}

/// **Everything between two items**: the items that start from the earlier
/// one's onset to the later one's, on the staves from the higher of the two to
/// the lower -- what a Shift+click extends a selection to.
pub fn range(sheet: &Sheet, from: u64, to: u64) -> Vec<u64> {
    let (Some(a), Some(b)) = (locate(sheet, from), locate(sheet, to)) else {
        return Vec::new();
    };
    let (lo, hi) = (a.onset.min(b.onset), a.onset.max(b.onset));
    let (top, bottom) = (a.staff.min(b.staff), a.staff.max(b.staff));
    items_where(sheet, |at| {
        at.onset >= lo && at.onset <= hi && at.staff >= top && at.staff <= bottom
    })
}

/// The ids of every item `keep` answers yes for, in time order.
fn items_where(sheet: &Sheet, keep: impl Fn(&Located<'_>) -> bool) -> Vec<u64> {
    let all: Vec<u64> = sheet
        .staves
        .iter()
        .flat_map(|s| s.voices.iter())
        .flat_map(|v| v.items.iter().map(Item::id))
        .collect();
    let kept: Vec<u64> = all
        .into_iter()
        .filter(|&id| locate(sheet, id).is_some_and(|at| keep(&at)))
        .collect();
    in_time(sheet, &kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::notation::{Pitch, Staff, Step, Voice};

    fn note(id: u64, dur: (i64, i64)) -> Item {
        Item::Note {
            id,
            pitches: vec![Pitch {
                step: Step::C,
                alter: 0,
                octave: 4,
                forced: false,
            }],
            dur: Ratio::new(dur.0, dur.1),
            tie: false,
            marks: Marks::default(),
        }
    }

    /// One staff, one voice: four quarters, ids 1 to 4, and a rest, id 5.
    fn sheet() -> Sheet {
        let mut items: Vec<Item> = (1..=4).map(|id| note(id, (1, 4))).collect();
        items.push(Item::Rest {
            id: 5,
            dur: Ratio::new(1, 4),
        });
        Sheet {
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice { items }],
                ..Staff::default()
            }],
            ..Sheet::default()
        }
    }

    #[test]
    fn the_selection_is_read_in_time_order() {
        assert_eq!(in_time(&sheet(), &[3, 1, 99, 3]), vec![1, 3]);
        assert_eq!(locate(&sheet(), 3).map(|l| l.onset), Some(Ratio::new(1, 2)));
    }

    #[test]
    fn a_spanner_runs_from_the_first_selected_to_the_last() {
        let ops = ops(
            &sheet(),
            &[4, 2],
            &Action::Spanner {
                kind: "slur".into(),
            },
        )
        .unwrap();
        assert_eq!(
            ops,
            vec![Op::AddSpanner {
                kind: "slur".into(),
                from: 2,
                to: 4
            }]
        );
        assert!(
            super::ops(
                &sheet(),
                &[2],
                &Action::Spanner {
                    kind: "slur".into()
                }
            )
            .is_err(),
            "one note is no span"
        );
    }

    #[test]
    fn an_articulation_toggles_across_the_selection() {
        let mut marked = sheet();
        if let Item::Note { marks, .. } = &mut marked.staves[0].voices[0].items[0] {
            marks.articulations.push("stacc".into());
        }
        let action = Action::Articulation {
            name: "stacc".into(),
        };
        // one of two has it: both get it, and the one that had it keeps one
        let both = ops(&marked, &[1, 2], &action).unwrap();
        for op in &both {
            let Op::SetMarks { marks, .. } = op else {
                panic!("{op:?}")
            };
            assert_eq!(marks.articulations, vec!["stacc".to_string()]);
        }
        // the one that has it alone: it goes
        let off = ops(&marked, &[1], &action).unwrap();
        let Op::SetMarks { marks, .. } = &off[0] else {
            panic!()
        };
        assert!(marks.articulations.is_empty());
    }

    #[test]
    fn a_rest_takes_no_note_verb_and_says_why() {
        let why = ops(&sheet(), &[5], &Action::Move { steps: 1 }).unwrap_err();
        assert!(why.contains("note"), "{why}");
        // a length is a rest's too
        assert_eq!(
            ops(
                &sheet(),
                &[5],
                &Action::Scale {
                    factor: Ratio::new(2, 1)
                }
            )
            .unwrap(),
            vec![Op::SetDur {
                id: 5,
                dur: Ratio::new(1, 2)
            }]
        );
    }

    #[test]
    fn nothing_selected_is_refused_with_a_reason() {
        assert!(ops(&sheet(), &[], &Action::Delete).is_err());
    }

    #[test]
    fn a_voice_move_goes_to_the_other_voice() {
        assert_eq!(
            ops(&sheet(), &[2], &Action::Voice { to: None }).unwrap(),
            vec![Op::ToVoice {
                ids: vec![2],
                voice: 1
            }]
        );
    }

    #[test]
    fn a_measure_and_a_range_are_read_off_the_model() {
        let mut sheet = sheet();
        // a second bar: the rest and three quarters more
        sheet.staves[0].voices[0]
            .items
            .extend((6..=8).map(|id| note(id, (1, 4))));
        sheet.grid = clausters_core::notation::Grid::uniform(4, 4);
        assert_eq!(measure_items(&sheet, 1, None), vec![1, 2, 3, 4]);
        assert_eq!(measure_items(&sheet, 2, Some(1)), vec![5, 6, 7, 8]);
        assert!(
            measure_items(&sheet, 2, Some(2)).is_empty(),
            "no second staff"
        );
        assert_eq!(range(&sheet, 7, 3), vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn a_transformation_takes_the_span_the_selection_covers() {
        let mut sheet = sheet();
        sheet.grid = clausters_core::notation::Grid::uniform(2, 4);
        let action: Action =
            serde_json::from_str(r#"{"action": "transform", "name": "transpose", "semitones": 2}"#)
                .unwrap();
        let ops = ops(&sheet, &[3, 4], &action).unwrap();
        let Op::Transpose {
            semitones, span, ..
        } = &ops[0]
        else {
            panic!("{ops:?}")
        };
        assert_eq!(*semitones, 2);
        assert_eq!(*span, clausters_core::notation::Span::Measures(2, 2));
        // with nothing selected, everything
        let ops = super::ops(&sheet, &[], &action).unwrap();
        assert!(matches!(
            &ops[0],
            Op::Transpose {
                span: clausters_core::notation::Span::All,
                ..
            }
        ));
        let bad: Action =
            serde_json::from_str(r#"{"action": "transform", "name": "fold"}"#).unwrap();
        assert!(super::ops(&sheet, &[], &bad).is_err());
    }

    #[test]
    fn the_measure_verbs_act_on_the_measures_selected() {
        let mut sheet = sheet();
        sheet.grid = clausters_core::notation::Grid::uniform(2, 4);
        // items 3 and 4 are the second bar of two quarters
        let act = |json: &str| {
            let action: Action = serde_json::from_str(json).unwrap();
            ops(&sheet, &[3, 4], &action)
        };
        assert_eq!(
            act(r#"{"action": "measures", "edit": "insert_before"}"#).unwrap(),
            vec![Op::InsertMeasures { at: 2, count: 1 }]
        );
        assert_eq!(
            act(r#"{"action": "measures", "edit": "insert_after", "count": 2}"#).unwrap(),
            vec![Op::InsertMeasures { at: 3, count: 2 }]
        );
        assert_eq!(
            act(r#"{"action": "measures", "edit": "remove"}"#).unwrap(),
            vec![Op::RemoveMeasures { first: 2, last: 2 }]
        );
        assert_eq!(
            act(r#"{"action": "barline", "kind": "dbl"}"#).unwrap(),
            vec![Op::SetBarline {
                measure: 2,
                kind: "dbl".into()
            }]
        );
        assert_eq!(
            act(r#"{"action": "break", "kind": "system"}"#).unwrap(),
            vec![Op::SetBreak {
                measure: 2,
                kind: "system".into()
            }]
        );
        assert!(act(r#"{"action": "measures", "edit": "fold"}"#).is_err());
    }

    #[test]
    fn an_accidental_is_written_on_every_selected_note_and_printed() {
        let sheet = sheet();
        let action: Action =
            serde_json::from_str(r#"{"action": "accidental", "alter": 1}"#).unwrap();
        let planned = ops(&sheet, &[1], &action).unwrap();
        let [Op::SetPitches { id: 1, pitches }] = planned.as_slice() else {
            panic!("one note, set: {planned:?}")
        };
        assert!(pitches.iter().all(|p| p.alter == 1 && p.forced));
        let out_of_range: Action =
            serde_json::from_str(r#"{"action": "accidental", "alter": 3}"#).unwrap();
        assert!(ops(&sheet, &[1], &out_of_range).is_err());
    }

    #[test]
    fn a_voice_named_is_gone_to_and_one_already_held_is_left() {
        let sheet = sheet();
        let to = |voice: usize| ops(&sheet, &[2], &Action::Voice { to: Some(voice) }).unwrap();
        assert_eq!(
            to(1),
            vec![Op::ToVoice {
                ids: vec![2],
                voice: 1
            }]
        );
        assert!(to(0).is_empty(), "it is in the first voice already");
    }

    #[test]
    fn a_grace_note_is_a_mark_of_the_note_and_is_taken_back_with_none() {
        let sheet = sheet();
        let grace = |json: &str| {
            let action: Action = serde_json::from_str(json).unwrap();
            ops(&sheet, &[1], &action)
        };
        let planned = grace(r#"{"action": "grace", "kind": "unacc"}"#).unwrap();
        let [Op::SetMarks { id: 1, marks }] = planned.as_slice() else {
            panic!("one note, marked: {planned:?}")
        };
        assert_eq!(marks.grace.as_deref(), Some("unacc"));
        let planned = grace(r#"{"action": "grace"}"#).unwrap();
        let [Op::SetMarks { marks, .. }] = planned.as_slice() else {
            panic!("one note: {planned:?}")
        };
        assert_eq!(marks.grace, None);
        assert!(grace(r#"{"action": "grace", "kind": "slash"}"#).is_err());
    }

    #[test]
    fn a_verb_is_read_from_its_json() {
        let action: Action =
            serde_json::from_str(r#"{"action": "scale", "factor": [2, 1]}"#).unwrap();
        assert_eq!(
            action,
            Action::Scale {
                factor: Ratio::new(2, 1)
            }
        );
        let action: Action = serde_json::from_str(r#"{"action": "clear_marks"}"#).unwrap();
        assert_eq!(action, Action::ClearMarks);
    }
}
