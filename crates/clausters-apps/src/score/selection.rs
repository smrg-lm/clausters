//! **What a selection is**: the elements a hand picked on the page, read as
//! what each is in the model.
//!
//! A page names every element it draws and a press reports the name, so what
//! a verb acts on is decided here, by **what was picked**, and not by where
//! the pointer was: an item (`n7`), the items of a measure (`m3s1`), a text
//! of the page (`t-title`), or something written beside the notes
//! ([`Attachment`]: a slur, a hairpin, a dynamic, a tempo mark). That is the
//! rule the field's editors share -- everything drawn can be picked, and a
//! verb means what the selection makes it mean:
//!
//! - **Delete takes away what is selected**, whatever it is: the slur and
//!   not its notes, the dynamic and not the note it is under.
//! - **A verb that asks for notes reads the notes the selection is attached
//!   to**: with a dynamic selected, another dynamic replaces it; with a slur
//!   selected, a hairpin is written over the same notes.
//!
//! Everything here is pure, as the verbs are: a sheet and a selection in,
//! operations or a sentence out.

use clausters_core::notation::{Attachment, Item, Marks, Op, Sheet, attachment_id};

/// What of the selection is written beside the notes, each once, in the
/// order it was picked. A part of a line in another run of pages, and the
/// sign a pedal is let go with, are the line they belong to.
pub fn attached(selection: &[String]) -> Vec<Attachment> {
    let mut out: Vec<Attachment> = Vec::new();
    for found in selection.iter().filter_map(|id| attachment_id(id)) {
        if !out.contains(&found) {
            out.push(found);
        }
    }
    out
}

/// **The items a verb that asks for notes reads**, where what is selected is
/// attached to them: the item a mark or a sign is written at, the two ends of
/// a line.
pub fn anchors(attached: &[Attachment]) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    for found in attached {
        let items = match found {
            Attachment::Spanner { from, to, .. } => vec![*from, *to],
            Attachment::Mark { item, .. } => vec![*item],
            Attachment::Control { on, .. } => vec![*on],
        };
        for id in items {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// **Taking away what is selected**: the line, the sign, or the one mark of
/// its item -- the item's other marks stay, and so does the item.
pub fn removal(sheet: &Sheet, attached: &[Attachment]) -> Vec<Op> {
    // several marks of one item go in one writing of its marks
    let mut marked: Vec<(u64, Marks)> = Vec::new();
    let mut out: Vec<Op> = Vec::new();
    for found in attached {
        match found {
            Attachment::Spanner { kind, from, to } => out.push(Op::RemoveSpanner {
                kind: kind.clone(),
                from: *from,
                to: *to,
            }),
            Attachment::Control { kind, on } => out.push(Op::RemoveControl {
                kind: kind.clone(),
                on: *on,
            }),
            Attachment::Mark { mark, item } => {
                let at = match marked.iter().position(|(id, _)| id == item) {
                    Some(at) => at,
                    None => {
                        let Some(marks) = marks_of(sheet, *item) else {
                            continue;
                        };
                        marked.push((*item, marks));
                        marked.len() - 1
                    }
                };
                clear(&mut marked[at].1, mark);
            }
        }
    }
    out.extend(
        marked
            .into_iter()
            .map(|(id, marks)| Op::SetMarks { id, marks }),
    );
    out
}

/// The marks `item` carries, where it is a note.
fn marks_of(sheet: &Sheet, item: u64) -> Option<Marks> {
    sheet
        .voices()
        .flat_map(|voice| voice.items.iter())
        .find(|i| i.id() == item)
        .and_then(Item::marks)
        .cloned()
}

/// Take the mark named `mark` off `marks`.
fn clear(marks: &mut Marks, mark: &str) {
    match mark {
        "dynamic" => marks.dynamic = None,
        "ornament" => marks.ornament = None,
        "arpeggio" => marks.arpeggio = None,
        "breath" => marks.breath = None,
        "ring" => marks.ring = false,
        "fingering" => marks.fingering = None,
        "harmony" => marks.harmony = None,
        _ => {}
    }
}

/// **The item beside `item` in its voice**: the one after it, `forward`, or
/// the one before -- what the arrows move a selection to. `None` at either
/// end of the voice, where the selection stays.
pub fn beside(sheet: &Sheet, item: u64, forward: bool) -> Option<u64> {
    let (si, vi, index) = sheet.locate(item)?;
    let items = &sheet.staves[si].voices[vi].items;
    let index = if forward {
        index + 1
    } else {
        index.checked_sub(1)?
    };
    items.get(index).map(Item::id)
}

/// **What the status line says of one of them**: what it is and where it is
/// written, in the model's own words.
pub fn describe(sheet: &Sheet, found: &Attachment) -> String {
    match found {
        Attachment::Spanner { kind, from, to } => {
            format!("{kind} from item {from} to item {to} -- Delete takes it away")
        }
        Attachment::Control { kind, on } => {
            let text = sheet
                .controls
                .iter()
                .find(|c| &c.kind == kind && c.on == *on)
                .map(|c| c.text.as_str())
                .unwrap_or_default();
            let name = match kind.as_str() {
                "tempo" => "tempo mark",
                "reh" => "rehearsal mark",
                _ => "direction",
            };
            format!("{name} at item {on}: \"{text}\" -- Delete takes it away")
        }
        Attachment::Mark { mark, item } => {
            let marks = marks_of(sheet, *item).unwrap_or_default();
            let said = match mark.as_str() {
                "dynamic" => marks.dynamic,
                "ornament" => marks.ornament,
                "arpeggio" => marks.arpeggio,
                "breath" => marks.breath,
                "fingering" => marks.fingering,
                "harmony" => marks.harmony,
                _ => None,
            };
            let name = if mark == "ring" { "let it ring" } else { mark };
            match said {
                Some(said) => format!("{name} of item {item}: {said} -- Delete takes it away"),
                None => format!("{name} of item {item} -- Delete takes it away"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::notation::{Control, Pitch, Spanner, Staff, Step, Voice};
    use clausters_core::ratio::Ratio;

    fn sheet() -> Sheet {
        let note = |id: u64| Item::Note {
            id,
            pitches: vec![Pitch {
                step: Step::C,
                alter: 0,
                octave: 4,
                forced: false,
            }],
            dur: Ratio::new(1, 4),
            tie: false,
            marks: Marks {
                dynamic: (id == 2).then(|| "mf".to_string()),
                ornament: (id == 2).then(|| "trill".to_string()),
                ring: id == 2,
                ..Marks::default()
            },
        };
        Sheet {
            next_id: 5,
            staves: vec![Staff {
                clef: "G2".into(),
                voices: vec![Voice {
                    items: (1..=4).map(note).collect(),
                }],
                ..Staff::default()
            }],
            spanners: vec![Spanner {
                kind: "slur".into(),
                from: 1,
                to: 4,
            }],
            controls: vec![Control {
                kind: "tempo".into(),
                on: 1,
                text: "Allegro".into(),
                bpm: None,
            }],
            ..Sheet::default()
        }
    }

    fn picked(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn a_selection_is_read_as_what_each_element_is() {
        // an item and an id the engraver minted are nothing attached; the
        // sign a pedal is let go with is the pedal, once
        let found = attached(&picked(&[
            "n2",
            "a-pedal-1-3",
            "a-pedal-1-3-up",
            "a-dynamic-n2",
            "x1y2",
        ]));
        assert_eq!(
            found,
            vec![
                Attachment::Spanner {
                    kind: "pedal".into(),
                    from: 1,
                    to: 3
                },
                Attachment::Mark {
                    mark: "dynamic".into(),
                    item: 2
                },
            ]
        );
        assert_eq!(anchors(&found), vec![1, 3, 2]);
    }

    #[test]
    fn taking_away_a_mark_leaves_the_items_other_marks() {
        let sheet = sheet();
        let found = attached(&picked(&[
            "a-slur-1-4",
            "a-dynamic-n2",
            "a-ring-n2",
            "a-tempo-n1",
        ]));
        let ops = removal(&sheet, &found);
        let kept = Marks {
            ornament: Some("trill".into()),
            ..Marks::default()
        };
        assert_eq!(
            ops,
            vec![
                Op::RemoveSpanner {
                    kind: "slur".into(),
                    from: 1,
                    to: 4
                },
                Op::RemoveControl {
                    kind: "tempo".into(),
                    on: 1
                },
                // the two marks of item 2 in one writing, its trill kept
                Op::SetMarks { id: 2, marks: kept },
            ]
        );
    }

    #[test]
    fn the_arrows_reach_the_item_beside_and_stop_at_the_ends() {
        let sheet = sheet();
        assert_eq!(beside(&sheet, 2, true), Some(3));
        assert_eq!(beside(&sheet, 2, false), Some(1));
        assert_eq!(beside(&sheet, 1, false), None);
        assert_eq!(beside(&sheet, 4, true), None);
        assert_eq!(beside(&sheet, 99, true), None);
    }

    #[test]
    fn each_is_described_in_the_models_words() {
        let sheet = sheet();
        let said = |id: &str| describe(&sheet, &attachment_id(id).unwrap());
        assert!(said("a-slur-1-4").starts_with("slur from item 1 to item 4"));
        assert!(said("a-dynamic-n2").starts_with("dynamic of item 2: mf"));
        assert!(said("a-ring-n2").starts_with("let it ring of item 2"));
        assert!(said("a-tempo-n1").starts_with("tempo mark at item 1: \"Allegro\""));
    }
}
