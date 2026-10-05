//! **The score editor's palettes**: what can be written on a score, one kind
//! of element to a group, beside the page.
//!
//! A palette offers the elements the engraver draws, under the names MEI
//! gives them, and each entry is one of the editor's verbs over what is
//! selected ([`super::verbs::Action`]): the palette states what it writes,
//! and a press is read by what reads a method call. An entry is a flat button
//! labelled with the engraver's own symbol where it has one ([`codes`]), and
//! what it says when the pointer rests on it is what the element is.
//!
//! The groups are titled sections that fold on their title strip, which is
//! the host's to remember; the entries are named here ([`names`]) and
//! numbered by the caller, as the tools are, and a window composed without
//! them has no palettes.
//!
//! **Only what the model holds is offered.** An element the engraver draws
//! and the model has no item or field for -- a tremolo, an arpeggio, a pedal,
//! lyrics -- has no entry until the model grows it; the list of those is the
//! plan's, not this file's.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use clausters_core::notation::glyph_char;

use super::tools::Outlines;

/// The caller's widget id for each entry it numbered, by the entry's name.
pub type Ids = BTreeMap<String, i32>;

/// One entry of a palette.
struct Entry {
    /// Its name within the group; the widget's is `group:name`.
    name: &'static str,
    /// What it is labelled where the engraver has no symbol for it.
    text: &'static str,
    /// The SMuFL codepoint it is drawn with, when there is one.
    code: Option<&'static str>,
    /// What the element is, in a sentence.
    tip: &'static str,
    /// The editor's verb a press asks for.
    action: Value,
}

/// One palette: a kind of element, and what it offers.
struct Group {
    name: &'static str,
    title: &'static str,
    /// Whether it opens folded.
    folded: bool,
    entries: Vec<Entry>,
}

fn entry(
    name: &'static str,
    text: &'static str,
    code: Option<&'static str>,
    tip: &'static str,
    action: Value,
) -> Entry {
    Entry {
        name,
        text,
        code,
        tip,
        action,
    }
}

/// The palettes, in the order the window shows them.
fn groups() -> Vec<Group> {
    let artic = |name: &'static str, text: &'static str, code: &'static str, tip: &'static str| {
        entry(
            name,
            text,
            Some(code),
            tip,
            json!({"action": "articulation", "name": name}),
        )
    };
    let accid = |name: &'static str, text: &'static str, code: &'static str, alter: i32, tip| {
        entry(
            name,
            text,
            Some(code),
            tip,
            json!({"action": "accidental", "alter": alter}),
        )
    };
    let ornament = |name: &'static str, text: &'static str, code: &'static str, tip| {
        entry(
            name,
            text,
            Some(code),
            tip,
            json!({"action": "ornament", "name": name}),
        )
    };
    let dynamic = |name: &'static str, code: &'static str, tip| {
        entry(
            name,
            name,
            Some(code),
            tip,
            json!({"action": "dynamic", "name": name}),
        )
    };
    let barline = |name: &'static str, text: &'static str, code: Option<&'static str>, tip| {
        entry(
            name,
            text,
            code,
            tip,
            json!({"action": "barline", "kind": name}),
        )
    };
    vec![
        Group {
            name: "notes",
            title: "Notes",
            folded: false,
            entries: vec![
                entry(
                    "triplet",
                    "3",
                    Some("E883"),
                    "Tuplet: three notes in the time of two",
                    json!({"action": "scale", "factor": [2, 3]}),
                ),
                entry(
                    "acc",
                    "acc",
                    Some("E562"),
                    "Appoggiatura: a grace note that takes its time from the note it leans on",
                    json!({"action": "grace", "kind": "acc"}),
                ),
                entry(
                    "unacc",
                    "unacc",
                    Some("E560"),
                    "Acciaccatura: a grace note crushed in before the beat",
                    json!({"action": "grace", "kind": "unacc"}),
                ),
                entry(
                    "plain",
                    "note",
                    Some("E1D5"),
                    "A note of the bar again: no longer a grace note",
                    json!({"action": "grace"}),
                ),
                entry(
                    "voice",
                    "voice",
                    None,
                    "Voice: move the selection to the other line of its staff",
                    json!({"action": "voice"}),
                ),
            ],
        },
        Group {
            name: "accidentals",
            title: "Accidentals",
            folded: false,
            entries: vec![
                accid(
                    "double_flat",
                    "bb",
                    "E264",
                    -2,
                    "Double flat: two semitones below the letter",
                ),
                accid("flat", "b", "E260", -1, "Flat: a semitone below the letter"),
                accid(
                    "natural",
                    "nat",
                    "E261",
                    0,
                    "Natural: the letter itself, whatever the key says",
                ),
                accid(
                    "sharp",
                    "#",
                    "E262",
                    1,
                    "Sharp: a semitone above the letter",
                ),
                accid(
                    "double_sharp",
                    "x",
                    "E263",
                    2,
                    "Double sharp: two semitones above the letter",
                ),
            ],
        },
        Group {
            name: "articulations",
            title: "Articulations",
            folded: false,
            entries: vec![
                artic(
                    "stacc",
                    ".",
                    "E4A2",
                    "Staccato: short, detached from the next note",
                ),
                artic(
                    "stacciss",
                    "'",
                    "E4A6",
                    "Staccatissimo: as short as it can be played",
                ),
                artic(
                    "spicc",
                    "v",
                    "E4A8",
                    "Spiccato: the bow bounced off the string",
                ),
                artic("acc", ">", "E4A0", "Accent: a stronger attack"),
                artic("marc", "^", "E4AC", "Marcato: a heavy, marked attack"),
                artic("ten", "-", "E4A4", "Tenuto: held for its whole value"),
                artic(
                    "stress",
                    "/",
                    "E4B6",
                    "Stress: a metrical weight on the note",
                ),
                artic(
                    "upbow",
                    "V",
                    "E612",
                    "Up-bow: the stroke that starts at the tip",
                ),
                artic(
                    "dnbow",
                    "n",
                    "E610",
                    "Down-bow: the stroke that starts at the frog",
                ),
                artic(
                    "harm",
                    "o",
                    "E614",
                    "Harmonic: the string touched, not stopped",
                ),
                artic(
                    "snap",
                    "(|)",
                    "E631",
                    "Snap pizzicato: the string let go against the fingerboard",
                ),
                artic(
                    "open",
                    "o",
                    "E5E7",
                    "Open: an open string, or an unstopped horn",
                ),
                artic(
                    "stop",
                    "+",
                    "E5E5",
                    "Stopped: a hand-stopped horn, or a left-hand pizzicato",
                ),
            ],
        },
        Group {
            name: "ornaments",
            title: "Ornaments",
            folded: false,
            entries: vec![
                ornament(
                    "trill",
                    "tr",
                    "E566",
                    "Trill: a rapid alternation with the note above",
                ),
                ornament(
                    "mordent",
                    "m",
                    "E56C",
                    "Mordent: one quick alternation with a neighbour note",
                ),
                ornament(
                    "turn",
                    "~",
                    "E567",
                    "Turn: the four-note figure around the note",
                ),
                ornament("fermata", "U", "E4C0", "Fermata: a hold of no fixed length"),
                entry(
                    "none",
                    "-",
                    None,
                    "No ornament",
                    json!({"action": "ornament"}),
                ),
            ],
        },
        Group {
            name: "lines",
            title: "Lines",
            folded: false,
            entries: vec![
                entry(
                    "slur",
                    "slur",
                    None,
                    "Slur: a curve over a phrase, from the first selected note to the last",
                    json!({"action": "spanner", "kind": "slur"}),
                ),
                entry(
                    "tie",
                    "tie",
                    None,
                    "Tie: two notes of one pitch joined into one sound",
                    json!({"action": "tie"}),
                ),
                entry(
                    "crescendo",
                    "<",
                    Some("E53E"),
                    "Crescendo: a hairpin opening, gradually louder",
                    json!({"action": "spanner", "kind": "crescendo"}),
                ),
                entry(
                    "diminuendo",
                    ">",
                    Some("E53F"),
                    "Diminuendo: a hairpin closing, gradually softer",
                    json!({"action": "spanner", "kind": "diminuendo"}),
                ),
            ],
        },
        Group {
            name: "dynamics",
            title: "Dynamics",
            folded: false,
            entries: vec![
                dynamic("pp", "E52B", "Pianissimo: very soft"),
                dynamic("p", "E520", "Piano: soft"),
                dynamic("mp", "E52C", "Mezzo piano: moderately soft"),
                dynamic("mf", "E52D", "Mezzo forte: moderately loud"),
                dynamic("f", "E522", "Forte: loud"),
                dynamic("ff", "E52F", "Fortissimo: very loud"),
                dynamic("sf", "E536", "Sforzando: a sudden strong accent"),
                dynamic("fp", "E534", "Fortepiano: loud, then at once soft"),
                entry(
                    "none",
                    "-",
                    None,
                    "No dynamic",
                    json!({"action": "dynamic"}),
                ),
            ],
        },
        Group {
            name: "measures",
            title: "Measures",
            folded: true,
            entries: vec![
                entry(
                    "insert_before",
                    "+|",
                    None,
                    "Open an empty measure before the first selected",
                    json!({"action": "measures", "edit": "insert_before"}),
                ),
                entry(
                    "insert_after",
                    "|+",
                    None,
                    "Open an empty measure after the last selected",
                    json!({"action": "measures", "edit": "insert_after"}),
                ),
                entry(
                    "remove",
                    "x",
                    None,
                    "Take out the selected measures, with what is written in them",
                    json!({"action": "measures", "edit": "remove"}),
                ),
                barline(
                    "single",
                    "|",
                    Some("E030"),
                    "Barline: the ordinary end of a measure",
                ),
                barline(
                    "dbl",
                    "||",
                    Some("E031"),
                    "Double barline: the end of a section",
                ),
                barline(
                    "end",
                    "|]",
                    Some("E032"),
                    "Final barline: the end of the music",
                ),
                barline(
                    "rptstart",
                    "|:",
                    Some("E040"),
                    "Repeat start: where a repeated passage begins",
                ),
                barline(
                    "rptend",
                    ":|",
                    Some("E041"),
                    "Repeat end: back to the repeat start",
                ),
                barline(
                    "rptboth",
                    ":|:",
                    Some("E042"),
                    "Repeat end and start, at one barline",
                ),
                barline(
                    "invis",
                    "( )",
                    None,
                    "Invisible barline: a measure with no line drawn",
                ),
                entry(
                    "system",
                    "sys",
                    Some("E4CE"),
                    "System break: the next measure starts a new line",
                    json!({"action": "break", "kind": "system"}),
                ),
                entry(
                    "page",
                    "page",
                    None,
                    "Page break: the next measure starts a new page",
                    json!({"action": "break", "kind": "page"}),
                ),
                entry(
                    "flow",
                    "flow",
                    None,
                    "No break: the line and the page are the engraver's to fill",
                    json!({"action": "break", "kind": "none"}),
                ),
            ],
        },
    ]
}

/// The widget name of entry `name` of group `group`.
fn widget_name(group: &str, name: &str) -> String {
    format!("{group}:{name}")
}

/// **Every entry of every palette, by name** (`articulations:stacc`), for a
/// caller to number.
#[must_use]
pub fn names() -> Vec<String> {
    groups()
        .iter()
        .flat_map(|group| {
            group
                .entries
                .iter()
                .map(|entry| widget_name(group.name, entry.name))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// **Every codepoint an entry is drawn with**: what the editor asks the
/// engraver's outlines for, beside the tools'.
#[must_use]
pub fn codes() -> Vec<&'static str> {
    groups()
        .iter()
        .flat_map(|group| group.entries.iter().filter_map(|entry| entry.code))
        .collect()
}

/// How wide the column of palettes is.
pub const WIDTH: f64 = 196.0;

/// How many entries a palette lays side by side.
const COLS: usize = 5;

/// **The palettes**, as the column the window holds beside the page -- or
/// `None` when the caller numbered no entry. An entry whose symbol is in
/// `outlines` is labelled with it; a palette none of whose entries was
/// numbered is left out.
#[must_use]
pub fn column(ids: &Ids, outlines: &Outlines) -> Option<Value> {
    let sections: Vec<Value> = groups()
        .into_iter()
        .filter_map(|group| {
            let buttons: Vec<Value> = group
                .entries
                .iter()
                .filter_map(|entry| {
                    let id = *ids.get(&widget_name(group.name, entry.name))?;
                    let label = entry
                        .code
                        .filter(|code| outlines.contains_key(*code))
                        .and_then(glyph_char)
                        .map_or_else(|| entry.text.to_string(), String::from);
                    Some(json!({
                        "type": "button",
                        "id": id,
                        "flat": true,
                        "label": label,
                        "tip": entry.tip,
                    }))
                })
                .collect();
            (!buttons.is_empty()).then(|| {
                json!({
                    "type": "layout",
                    "title": group.title,
                    "frame": true,
                    "collapsed": group.folded,
                    "flow": "grid",
                    "cols": COLS,
                    "hug": true,
                    "children": buttons,
                })
            })
        })
        .collect();
    (!sections.is_empty()).then(|| {
        json!({
            "type": "layout",
            "flow": "col",
            "w": WIDTH,
            "pack": true,
            "children": sections,
        })
    })
}

/// **The editor's verb the entry `name` asks for when it reports `tag`** --
/// or `None` for a name that is no entry, and for a report that is no command
/// (a button's value rises and falls with the hand; its `"click"` is the
/// press that counted).
#[must_use]
pub fn read(name: &str, tag: &str) -> Option<Value> {
    if tag != "click" {
        return None;
    }
    let (group, entry) = name.split_once(':')?;
    groups()
        .into_iter()
        .find(|g| g.name == group)?
        .entries
        .into_iter()
        .find(|e| e.name == entry)
        .map(|e| e.action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::verbs::Action;

    fn ids() -> Ids {
        names()
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name, 400 + i as i32))
            .collect()
    }

    #[test]
    fn every_entry_is_a_verb_the_editor_reads_under_a_name_of_its_own() {
        let names = names();
        assert!(
            names.len() > 45,
            "the palettes hold the model: {}",
            names.len()
        );
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "no two entries share a name");
        for name in &names {
            let action = read(name, "click").unwrap_or_else(|| panic!("{name} asks nothing"));
            serde_json::from_value::<Action>(action.clone())
                .unwrap_or_else(|why| panic!("{name}: {action} is no verb of the editor: {why}"));
            assert_eq!(read(name, "1"), None, "{name}: a value is no command");
        }
        assert_eq!(read("articulations:fold", "click"), None);
        assert_eq!(read("stacc", "click"), None);
    }

    #[test]
    fn the_column_holds_a_folding_section_per_palette() {
        let column = column(&ids(), &Outlines::new()).expect("numbered");
        let sections = column["children"].as_array().unwrap();
        let titles: Vec<&str> = sections
            .iter()
            .filter_map(|s| s["title"].as_str())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Notes",
                "Accidentals",
                "Articulations",
                "Ornaments",
                "Lines",
                "Dynamics",
                "Measures"
            ]
        );
        // a section folds on its title strip, and the last opens folded
        assert!(sections.iter().all(|s| s["collapsed"].is_boolean()));
        assert_eq!(sections[6]["collapsed"], true);
        assert_eq!(sections[2]["children"].as_array().unwrap().len(), 13);
        // every entry says what the element is
        let button = &sections[2]["children"][0];
        assert_eq!(button["label"], ".");
        assert!(button["tip"].as_str().unwrap().starts_with("Staccato: "));
    }

    #[test]
    fn only_what_was_numbered_is_in_it_and_a_symbol_labels_what_has_one() {
        let some: Ids = [("articulations:stacc", 7), ("lines:slur", 8)]
            .into_iter()
            .map(|(name, id)| (name.to_string(), id))
            .collect();
        let outlines: Outlines = [("E4A2".to_string(), "M0 0h1v1z".to_string())].into();
        let column = column(&some, &outlines).expect("two entries");
        let sections = column["children"].as_array().unwrap();
        assert_eq!(sections.len(), 2, "a palette with no entry is left out");
        assert_eq!(sections[0]["children"][0]["label"], "\u{E4A2}");
        assert_eq!(sections[1]["children"][0]["label"], "slur");
        assert!(super::column(&Ids::new(), &outlines).is_none());
        // every symbol asked for is a codepoint
        assert!(codes().iter().all(|code| glyph_char(code).is_some()));
    }
}
