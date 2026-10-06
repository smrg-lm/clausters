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
//! and the model has no item or field for has no entry until the model grows
//! it. An entry that needs words -- a tempo, a direction, a syllable -- opens a
//! one-field dialog (`{"dialog": ...}`), whose answer is the verb.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use clausters_core::notation::glyph_char;

use super::icons;
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
    let mark = |name: &'static str,
                text: &'static str,
                code: Option<&'static str>,
                tip: &'static str,
                field: &'static str,
                value: Value| {
        entry(
            name,
            text,
            code,
            tip,
            json!({"action": "mark", "mark": field, "value": value}),
        )
    };
    let tremolo = |strokes: u8, code: &'static str, tip: &'static str| {
        let name = ["tremolo1", "tremolo2", "tremolo3"][usize::from(strokes - 1)];
        mark(name, "trem", Some(code), tip, "tremolo", json!(strokes))
    };
    let line = |name: &'static str, text: &'static str, code: Option<&'static str>, tip| {
        entry(
            name,
            text,
            code,
            tip,
            json!({"action": "spanner", "kind": name}),
        )
    };
    let dialog =
        |name: &'static str,
         text: &'static str,
         code: Option<&'static str>,
         tip: &'static str,
         form: &'static str| { entry(name, text, code, tip, json!({"dialog": form})) };
    let navigation = |name: &'static str, text: &'static str, code: Option<&'static str>, tip| {
        entry(
            name,
            text,
            code,
            tip,
            json!({"action": "navigation", "kind": name}),
        )
    };
    let key = |name: &'static str, tip: &'static str| {
        let code = icons::KEY_SIGNS
            .iter()
            .find(|(key, ..)| *key == name)
            .map(|(_, code, _)| *code);
        entry(name, name, code, tip, json!({"action": "key", "key": name}))
    };
    let clef = |name: &'static str, code: Option<&'static str>, tip: &'static str| {
        entry(
            name,
            name,
            code,
            tip,
            json!({"action": "clef", "clef": name}),
        )
    };
    let group = |name: &'static str, code: Option<&'static str>, tip: &'static str| {
        entry(
            name,
            name,
            code,
            tip,
            json!({"action": "group", "symbol": name}),
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
                    Some(icons::APPOGGIATURA),
                    "Appoggiatura: a grace note that takes its time from the note it leans on",
                    json!({"action": "grace", "kind": "acc"}),
                ),
                entry(
                    "unacc",
                    "unacc",
                    Some(icons::ACCIACCATURA),
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
                    Some(icons::VOICE),
                    "Voice: move the selection to the other line of its staff",
                    json!({"action": "voice"}),
                ),
                tremolo(1, "E220", "Tremolo: the note repeated in eighths"),
                tremolo(2, "E221", "Tremolo: the note repeated in sixteenths"),
                tremolo(3, "E222", "Tremolo: the note repeated in thirty-seconds"),
                entry(
                    "ftrem",
                    "ftrem",
                    Some(icons::TWO_NOTE_TREMOLO),
                    "Two-note tremolo: the two selected notes, of one value, alternating",
                    json!({"action": "spanner", "kind": "ftrem"}),
                ),
                entry(
                    "beat_repeat",
                    "%",
                    Some("E504"),
                    "Beat repeat: the beat drawn as a repeat of the one before",
                    json!({"action": "beat_repeat"}),
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
                    "schleifer",
                    "schl",
                    Some("E587"),
                    "Schleifer: a slide up to the note from below",
                    json!({"action": "ornament", "name": "ornamentSchleifer"}),
                ),
                entry(
                    "haydn",
                    "haydn",
                    Some("E56F"),
                    "Haydn ornament: a turn or a trill, as the period read it",
                    json!({"action": "ornament", "name": "ornamentHaydn"}),
                ),
                mark(
                    "arpeggio_up",
                    "arp",
                    Some(icons::ARPEGGIO_UP),
                    "Arpeggio: the chord rolled from its lowest note up",
                    "arpeggio",
                    json!("up"),
                ),
                mark(
                    "arpeggio_down",
                    "arp",
                    Some(icons::ARPEGGIO_DOWN),
                    "Arpeggio: the chord rolled from its highest note down",
                    "arpeggio",
                    json!("down"),
                ),
                entry(
                    "gliss",
                    "gliss",
                    Some(icons::GLISSANDO),
                    "Glissando: a slide from the first selected note to the last",
                    json!({"action": "spanner", "kind": "gliss"}),
                ),
                mark(
                    "breath",
                    ",",
                    Some("E4CE"),
                    "Breath: the note let go a little early",
                    "breath",
                    json!("breath"),
                ),
                mark(
                    "caesura",
                    "//",
                    Some("E4D1"),
                    "Caesura: a full stop of the line",
                    "breath",
                    json!("caesura"),
                ),
                entry(
                    "none",
                    "-",
                    Some(icons::NONE),
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
                    Some(icons::SLUR),
                    "Slur: a curve over a phrase, from the first selected note to the last",
                    json!({"action": "spanner", "kind": "slur"}),
                ),
                entry(
                    "tie",
                    "tie",
                    Some(icons::TIE),
                    "Tie: two notes of one pitch joined into one sound",
                    json!({"action": "tie"}),
                ),
                entry(
                    "crescendo",
                    "<",
                    Some(icons::CRESCENDO),
                    "Crescendo: a hairpin opening, gradually louder",
                    json!({"action": "spanner", "kind": "crescendo"}),
                ),
                entry(
                    "diminuendo",
                    ">",
                    Some(icons::DIMINUENDO),
                    "Diminuendo: a hairpin closing, gradually softer",
                    json!({"action": "spanner", "kind": "diminuendo"}),
                ),
                mark(
                    "lv",
                    "l.v.",
                    Some(icons::LET_RING),
                    "Let it ring: the note held past its value",
                    "ring",
                    json!(true),
                ),
                line(
                    "phrase",
                    "phr",
                    Some(icons::PHRASE),
                    "Phrase mark: a phrase, for analysis",
                ),
                line(
                    "8va",
                    "8va",
                    Some("E511"),
                    "Ottava: the notes sound an octave higher",
                ),
                line(
                    "8vb",
                    "8vb",
                    Some("E51C"),
                    "Ottava bassa: the notes sound an octave lower",
                ),
                line(
                    "15ma",
                    "15ma",
                    Some("E515"),
                    "Quindicesima: two octaves higher",
                ),
                line(
                    "15mb",
                    "15mb",
                    Some("E51D"),
                    "Quindicesima bassa: two octaves lower",
                ),
                line(
                    "pedal",
                    "Ped.",
                    Some("E650"),
                    "Pedal: the sustain pedal held from the first selected note to the last",
                ),
                line(
                    "bracket",
                    "[ ]",
                    Some(icons::BRACKET),
                    "Bracket over a stretch of notes",
                ),
                line(
                    "beamspan",
                    "beam",
                    Some(icons::BEAM_SPAN),
                    "Beam across a barline or across staves",
                ),
            ],
        },
        Group {
            name: "text",
            title: "Text",
            folded: true,
            entries: vec![
                dialog(
                    "tempo",
                    "tempo",
                    Some("E1D5"),
                    "Tempo mark: words, and a speed",
                    "tempo",
                ),
                dialog(
                    "dir",
                    "dir.",
                    None,
                    "Direction: words at a point (dolce, pizz.)",
                    "direction",
                ),
                dialog(
                    "reh",
                    "A",
                    Some(icons::REHEARSAL),
                    "Rehearsal mark: a letter in a box",
                    "rehearsal",
                ),
                dialog(
                    "fing",
                    "1",
                    Some("ED11"),
                    "Fingering over the note",
                    "fingering",
                ),
                dialog("harm", "C7", None, "Chord symbol over the note", "harmony"),
                dialog(
                    "lyric",
                    "la",
                    None,
                    "Lyrics: a syllable under the note",
                    "lyric",
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
                    Some(icons::NONE),
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
                    Some(icons::MEASURE_BEFORE),
                    "Open an empty measure before the first selected",
                    json!({"action": "measures", "edit": "insert_before"}),
                ),
                entry(
                    "insert_after",
                    "|+",
                    Some(icons::MEASURE_AFTER),
                    "Open an empty measure after the last selected",
                    json!({"action": "measures", "edit": "insert_after"}),
                ),
                entry(
                    "remove",
                    "x",
                    Some(icons::MEASURE_REMOVE),
                    "Take out the selected measures, with what is written in them",
                    json!({"action": "measures", "edit": "remove"}),
                ),
                barline(
                    "single",
                    "|",
                    Some(icons::BARLINE_SINGLE),
                    "Barline: the ordinary end of a measure",
                ),
                barline(
                    "dbl",
                    "||",
                    Some(icons::BARLINE_DOUBLE),
                    "Double barline: the end of a section",
                ),
                barline(
                    "end",
                    "|]",
                    Some(icons::BARLINE_FINAL),
                    "Final barline: the end of the music",
                ),
                barline(
                    "rptstart",
                    "|:",
                    Some(icons::REPEAT_START),
                    "Repeat start: where a repeated passage begins",
                ),
                barline(
                    "rptend",
                    ":|",
                    Some(icons::REPEAT_END),
                    "Repeat end: back to the repeat start",
                ),
                barline(
                    "rptboth",
                    ":|:",
                    Some(icons::REPEAT_BOTH),
                    "Repeat end and start, at one barline",
                ),
                barline(
                    "invis",
                    "( )",
                    Some(icons::BARLINE_INVISIBLE),
                    "Invisible barline: a measure with no line drawn",
                ),
                entry(
                    "system",
                    "sys",
                    Some(icons::BREAK_SYSTEM),
                    "System break: the next measure starts a new line",
                    json!({"action": "break", "kind": "system"}),
                ),
                entry(
                    "page",
                    "page",
                    Some(icons::BREAK_PAGE),
                    "Page break: the next measure starts a new page",
                    json!({"action": "break", "kind": "page"}),
                ),
                entry(
                    "flow",
                    "flow",
                    Some(icons::BREAK_NONE),
                    "No break: the line and the page are the engraver's to fill",
                    json!({"action": "break", "kind": "none"}),
                ),
                entry(
                    "multirests",
                    "4",
                    Some("E4EE"),
                    "Multirests: runs of empty measures drawn as one numbered rest",
                    json!({"action": "multirests"}),
                ),
            ],
        },
        Group {
            name: "jumps",
            title: "Repeats and jumps",
            folded: true,
            entries: vec![
                entry(
                    "ending",
                    "1.",
                    Some(icons::ENDING),
                    "Ending: the selected measures, played in the passes it names",
                    json!({"dialog": "ending"}),
                ),
                entry(
                    "no_ending",
                    "-",
                    Some(icons::NONE),
                    "No ending over the selected measures",
                    json!({"action": "ending", "label": ""}),
                ),
                navigation(
                    "segno",
                    "S",
                    Some("E047"),
                    "Segno: the sign a dal segno goes back to",
                ),
                navigation("coda", "O", Some("E048"), "Coda: where a to coda goes"),
                navigation(
                    "fine",
                    "Fine",
                    None,
                    "Fine: where the piece ends after a jump",
                ),
                navigation("dacapo", "D.C.", Some("E046"), "Da capo: back to the start"),
                navigation(
                    "dalsegno",
                    "D.S.",
                    Some("E045"),
                    "Dal segno: back to the sign",
                ),
                navigation(
                    "tocoda",
                    "to O",
                    None,
                    "To coda: after a jump, on to the coda",
                ),
                navigation("none", "-", Some(icons::NONE), "No navigation mark"),
                entry(
                    "repeat",
                    "%",
                    Some("E500"),
                    "Measure repeat: the measure drawn as a repeat of the one before",
                    json!({"action": "measure_repeat"}),
                ),
            ],
        },
        Group {
            name: "keys",
            title: "Keys and clefs",
            folded: true,
            entries: vec![
                key(
                    "C",
                    "No sharps or flats, from the first selected measure on",
                ),
                key("G", "One sharp: G major, E minor"),
                key("D", "Two sharps: D major, B minor"),
                key("A", "Three sharps: A major, F sharp minor"),
                key("E", "Four sharps: E major, C sharp minor"),
                key("F", "One flat: F major, D minor"),
                key("Bb", "Two flats: B flat major, G minor"),
                key("Eb", "Three flats: E flat major, C minor"),
                key("Ab", "Four flats: A flat major, F minor"),
                clef(
                    "G2",
                    Some("E050"),
                    "Treble clef, from the first selected note on",
                ),
                clef(
                    "F4",
                    Some("E062"),
                    "Bass clef, from the first selected note on",
                ),
                clef(
                    "C3",
                    Some("E05C"),
                    "Alto clef, from the first selected note on",
                ),
                clef(
                    "C4",
                    Some("E05C"),
                    "Tenor clef, from the first selected note on",
                ),
            ],
        },
        Group {
            name: "staves",
            title: "Staves",
            folded: true,
            entries: vec![
                entry(
                    "one_line",
                    "1",
                    Some(icons::ONE_LINE),
                    "One line: a percussion staff",
                    json!({"action": "staff", "lines": 1}),
                ),
                entry(
                    "five_lines",
                    "5",
                    Some(icons::FIVE_LINES),
                    "Five lines: the ordinary staff",
                    json!({"action": "staff", "lines": 5}),
                ),
                dialog(
                    "name",
                    "name",
                    None,
                    "Name: what the staff is called, and its short name",
                    "staff_name",
                ),
                dialog(
                    "transposition",
                    "trans",
                    None,
                    "Transposition: semitones from what it writes to what it sounds",
                    "transposition",
                ),
                group(
                    "brace",
                    Some("E000"),
                    "Brace: the selected staves as one instrument",
                ),
                group(
                    "bracket",
                    Some("E002"),
                    "Bracket: the selected staves as a section",
                ),
                group(
                    "line",
                    Some(icons::GROUP_LINE),
                    "Line: the selected staves joined by a line",
                ),
                group(
                    "none",
                    Some(icons::NONE),
                    "No group over the selected staves",
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
pub const WIDTH: f64 = 204.0;

/// The room around the palettes in their column, which is also where the
/// column's scroll bar runs: wide enough that the bar covers no section.
const MARGIN: f64 = 10.0;

/// **The size an entry's symbol is drawn at**, as its `text_size`: a step over
/// a tool's ([`super::tools::SYMBOL_SIZE`]), since a palette is where a symbol
/// is looked for among others like it. The em comes to 30 logical pixels.
pub const SYMBOL_SIZE: f64 = 3.5;

/// How many entries a palette lays side by side.
const COLS: usize = 5;

/// How many the palette `group` does: one fewer where the symbols are wide --
/// a dynamic of two letters, a line a staff long.
fn cols_of(group: &str) -> usize {
    match group {
        "lines" | "dynamics" => COLS - 1,
        "text" | "staves" | "jumps" => COLS - 2,
        _ => COLS,
    }
}

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
                    let symbol = entry
                        .code
                        .filter(|code| outlines.contains_key(*code))
                        .and_then(glyph_char);
                    let mut button = json!({
                        "type": "button",
                        "id": id,
                        "flat": true,
                        "label": symbol.map_or_else(|| entry.text.to_string(), String::from),
                        "tip": entry.tip,
                    });
                    // a symbol at a symbol's size; a word -- what has no
                    // symbol, a direction, a chord, a name -- at a word's
                    if symbol.is_some() || entry.code.is_some() {
                        button["text_size"] = json!(SYMBOL_SIZE);
                    }
                    Some(button)
                })
                .collect();
            (!buttons.is_empty()).then(|| {
                json!({
                    "type": "layout",
                    "title": group.title,
                    "frame": true,
                    "collapsed": group.folded,
                    "flow": "grid",
                    "cols": cols_of(group.name),
                    "hug": true,
                    "children": buttons,
                })
            })
        })
        .collect();
    // **The column scrolls**: how tall it is depends on which sections are
    // open, which is the host's to know, so it is a plane that flows them and
    // is as long as they are -- past the window's foot it is reached by the
    // wheel, where a plain column put its last sections out of reach.
    (!sections.is_empty()).then(|| {
        json!({
            "type": "plane",
            "flow": "col",
            "axis": "y",
            "zoom": false,
            "bars": true,
            "w": WIDTH,
            "margin": MARGIN,
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
            // an entry that needs words opens the form that asks for them
            if let Some(form) = action.get("dialog") {
                assert!(
                    form.as_str()
                        .and_then(crate::score::dialogs::Form::named)
                        .is_some(),
                    "{name}: no form {form}"
                );
                continue;
            }
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
                "Text",
                "Dynamics",
                "Measures",
                "Repeats and jumps",
                "Keys and clefs",
                "Staves"
            ]
        );
        // a section folds on its title strip, and the last ones open folded
        assert!(sections.iter().all(|s| s["collapsed"].is_boolean()));
        assert_eq!(sections[7]["collapsed"], true);
        assert_eq!(sections[2]["children"].as_array().unwrap().len(), 13);
        // every entry says what the element is, and is drawn at a symbol's size
        let button = &sections[2]["children"][0];
        assert_eq!(button["label"], ".");
        assert!(button["tip"].as_str().unwrap().starts_with("Staccato: "));
        assert_eq!(button["text_size"], SYMBOL_SIZE);
        // the column is a plane that flows its sections, so it scrolls when
        // they are taller than the window; wide symbols get one cell fewer
        assert_eq!(
            (&column["type"], &column["flow"]),
            (&json!("plane"), &json!("col"))
        );
        assert_eq!(column["axis"], "y");
        assert_eq!(
            (&sections[2]["cols"], &sections[6]["cols"]),
            (&json!(5), &json!(4))
        );
    }

    /// **No entry is a word where a symbol can be drawn**: with the table
    /// completed -- as the editor completes the engraver's -- every entry of
    /// every palette that has a symbol is labelled with one character, and
    /// what is a word by nature -- a direction, a chord, a name -- is a word
    /// at a word's size.
    #[test]
    fn every_entry_has_a_symbol_once_the_table_is_completed() {
        // the engraver's own answer for the glyphs of its face, stood in for
        let mut outlines: Outlines = codes()
            .into_iter()
            .chain(icons::drawn_from())
            .filter(|code| {
                !icons::is_own(code)
                    && ![
                        icons::BARLINE_SINGLE,
                        icons::BARLINE_DOUBLE,
                        icons::BARLINE_FINAL,
                        icons::REPEAT_START,
                        icons::REPEAT_END,
                        icons::REPEAT_BOTH,
                    ]
                    .contains(code)
            })
            .filter(|code| {
                ![
                    icons::CRESCENDO,
                    icons::DIMINUENDO,
                    icons::ACCIACCATURA,
                    icons::APPOGGIATURA,
                ]
                .contains(code)
            })
            .map(|code| (code.to_string(), "M0 0h10v10h-10z".to_string()))
            .collect();
        icons::complete(&mut outlines);
        let column = column(&ids(), &outlines).expect("numbered");
        for section in column["children"].as_array().unwrap() {
            for entry in section["children"].as_array().unwrap() {
                let label = entry["label"].as_str().unwrap();
                if entry.get("text_size").is_none() {
                    assert!(label.is_ascii(), "{}: {label}", entry["tip"]);
                    continue;
                }
                assert_eq!(label.chars().count(), 1, "{}: {label}", entry["tip"]);
                assert!(
                    label.chars().all(|c| c as u32 >= 0xE000),
                    "{}",
                    entry["tip"]
                );
            }
        }
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
