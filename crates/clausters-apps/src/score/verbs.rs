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
    FIELDS, Halign, Item, Marks, NOTE, Op, PageSetup, Pages, Region, Sheet, Valign, default_place,
    paper,
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
    /// Move the selected items into the other voice of their staff, leaving
    /// rests where they were.
    Voice,
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
            Action::Voice => "move to the other voice".into(),
            Action::Spanner { kind } => kind.clone(),
            Action::Transform { name, .. } => name.clone(),
            Action::Page { .. } => "page setup".into(),
            Action::Text { field, .. } => format!("page text: {field}"),
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
    if ids.is_empty() {
        return Err("select a note first".into());
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
        Action::Voice => {
            let first = locate(sheet, ids[0]).map(|l| l.voice).unwrap_or(0);
            vec![Op::ToVoice {
                ids: ids.clone(),
                voice: if first == 0 { 1 } else { 0 },
            }]
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
        Action::Op { .. }
        | Action::Transform { .. }
        | Action::Page { .. }
        | Action::Text { .. } => {
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
            ops(&sheet(), &[2], &Action::Voice).unwrap(),
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
