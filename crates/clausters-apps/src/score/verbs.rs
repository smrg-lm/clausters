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

use clausters_core::notation::{Item, Marks, Op, Sheet};
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
}

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
        Action::Op { .. } => unreachable!("answered above"),
    })
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
