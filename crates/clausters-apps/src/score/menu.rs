//! **The score editor's menu bar**: every action the application has, as the
//! tree of entries the window carries, and what a pick of one asks for.
//!
//! A menu is a value of the window, and a pick reports the entry's **verb**.
//! So a verb here is either a word of the window's own -- `undo`, `redo`,
//! `select_all`, `layout:page`, `entry`, `value:1/8`, `dialog:text` -- or, for everything that
//! edits the score, the editor's verb itself as it is written
//! ([`super::verbs::Action`], as JSON): the menu states what it asks for, and
//! nothing between the entry and the edit has to know the two agree.
//!
//! What an entry shows of the editor's state -- which layout, whether entry is
//! on, the value in hand, the paper -- is read from [`State`] when the menu is
//! built, and the menu is sent again when that state moves.

use serde_json::{Value, json};

use clausters_core::notation::{PAPERS, View};
use clausters_core::ratio::Ratio;

use super::dialogs::Form;

/// What the menu shows of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State<'a> {
    /// How the window looks at the score.
    pub view: View,
    /// Whether a press on empty staff writes a note.
    pub entry: bool,
    /// The written value a note is entered with.
    pub value: Ratio,
    /// The paper the score is on, when it is a known one.
    pub paper: Option<&'a str>,
    /// Whether the page is wider than tall.
    pub landscape: bool,
    /// Whether the window has the dialogs: an entry that opens one is
    /// disabled in a window composed without them.
    pub dialogs: bool,
    /// Whether a pass loops.
    pub looping: bool,
}

/// The written values a note is entered with, longest first, and their names.
pub const VALUES: &[(&str, (i64, i64))] = &[
    ("Whole", (1, 1)),
    ("Half", (1, 2)),
    ("Quarter", (1, 4)),
    ("Eighth", (1, 8)),
    ("16th", (1, 16)),
    ("32nd", (1, 32)),
    ("64th", (1, 64)),
];

/// An entry that edits: its label, and the editor's verb as it is written.
fn act(label: &str, action: Value) -> Value {
    json!({"label": label, "verb": action.to_string()})
}

/// An entry of the window's own.
fn word(label: &str, verb: &str) -> Value {
    json!({"label": label, "verb": verb})
}

/// An entry that opens the form `form` ([`Form::named`]), disabled where the
/// window has no dialogs.
fn form(label: &str, form: &str, state: &State<'_>) -> Value {
    json!({"label": label, "verb": format!("dialog:{form}"), "enabled": state.dialogs})
}

/// One of several: on when it is the one in force.
fn one_of(label: &str, verb: String, group: &str, on: bool) -> Value {
    json!({"label": label, "verb": verb, "group": group, "checked": on})
}

fn sub(label: &str, entries: Vec<Value>) -> Value {
    json!({"label": label, "menu": entries})
}

/// **The menu bar**, as the window's `menu` value.
#[must_use]
pub fn menu(state: &State<'_>) -> Value {
    let sep = || json!("-");
    let mark =
        |label: &str, key: &str, name: &str| act(label, json!({"action": key, "name": name}));
    let transform = |label: &str, params: Value| {
        let mut action = json!({"action": "transform"});
        if let (Some(action), Value::Object(params)) = (action.as_object_mut(), params) {
            action.extend(params);
        }
        act(label, action)
    };
    json!([
        sub(
            "File",
            vec![
                sub(
                    "Paper",
                    PAPERS
                        .iter()
                        .map(|p| {
                            json!({
                                "label": p.name,
                                "verb": json!({"action": "page", "paper": p.name}).to_string(),
                                "group": "paper",
                                "checked": state.paper == Some(p.name),
                            })
                        })
                        .collect(),
                ),
                sub(
                    "Orientation",
                    vec![
                        json!({
                            "label": "Portrait",
                            "verb": json!({"action": "page", "landscape": false}).to_string(),
                            "group": "orientation",
                            "checked": !state.landscape,
                        }),
                        json!({
                            "label": "Landscape",
                            "verb": json!({"action": "page", "landscape": true}).to_string(),
                            "group": "orientation",
                            "checked": state.landscape,
                        }),
                    ],
                ),
                form("Page setup...", "page", state),
                sep(),
                form("Page text...", "text", state),
                sep(),
                form("Open...", "open", state),
                word("Save", "save"),
                form("Save as...", "save", state),
            ],
        ),
        sub(
            "Edit",
            vec![
                word("Undo", "undo"),
                word("Redo", "redo"),
                sep(),
                word("Select all", "select_all"),
                sep(),
                act("Delete", json!({"action": "delete"})),
                act("Silence", json!({"action": "silence"})),
            ],
        ),
        sub(
            "View",
            vec![
                one_of(
                    "Page",
                    "layout:page".into(),
                    "layout",
                    state.view == View::Page
                ),
                one_of(
                    "Continuous",
                    "layout:continuous".into(),
                    "layout",
                    state.view == View::Continuous,
                ),
            ],
        ),
        sub(
            "Play",
            vec![
                word("Play or stop", "play"),
                word("Back to the start", "rewind"),
                sep(),
                json!({"label": "Loop", "verb": "loop", "checked": state.looping}),
            ],
        ),
        sub(
            "Notes",
            vec![
                json!({"label": "Write notes", "verb": "entry", "checked": state.entry}),
                sub(
                    "Value",
                    VALUES
                        .iter()
                        .map(|(name, (n, d))| {
                            one_of(
                                name,
                                format!("value:{n}/{d}"),
                                "value",
                                state.value == Ratio::new(*n, *d),
                            )
                        })
                        .collect(),
                ),
                sep(),
                act("Step up", json!({"action": "move", "steps": 1})),
                act("Step down", json!({"action": "move", "steps": -1})),
                act("Octave up", json!({"action": "move", "steps": 7})),
                act("Octave down", json!({"action": "move", "steps": -7})),
                sep(),
                act("Longer", json!({"action": "scale", "factor": [2, 1]})),
                act("Shorter", json!({"action": "scale", "factor": [1, 2]})),
                act("Tie", json!({"action": "tie"})),
                act("Other voice", json!({"action": "voice"})),
            ],
        ),
        sub(
            "Notation",
            vec![
                act("Slur", json!({"action": "spanner", "kind": "slur"})),
                act(
                    "Crescendo",
                    json!({"action": "spanner", "kind": "crescendo"})
                ),
                act(
                    "Diminuendo",
                    json!({"action": "spanner", "kind": "diminuendo"})
                ),
                sep(),
                sub(
                    "Dynamic",
                    ["pp", "p", "mp", "mf", "f", "ff"]
                        .iter()
                        .map(|name| mark(name, "dynamic", name))
                        .chain([act("None", json!({"action": "dynamic"}))])
                        .collect(),
                ),
                sub(
                    "Articulation",
                    [
                        ("Staccato", "stacc"),
                        ("Accent", "acc"),
                        ("Tenuto", "ten"),
                        ("Marcato", "marc"),
                    ]
                    .iter()
                    .map(|(label, name)| mark(label, "articulation", name))
                    .collect(),
                ),
                sub(
                    "Ornament",
                    [
                        ("Trill", "trill"),
                        ("Mordent", "mordent"),
                        ("Turn", "turn"),
                        ("Fermata", "fermata"),
                    ]
                    .iter()
                    .map(|(label, name)| mark(label, "ornament", name))
                    .chain([act("None", json!({"action": "ornament"}))])
                    .collect(),
                ),
                sep(),
                act("Clear marks", json!({"action": "clear_marks"})),
            ],
        ),
        sub(
            "Measures",
            vec![
                act(
                    "Insert before",
                    json!({"action": "measures", "edit": "insert_before"}),
                ),
                act(
                    "Insert after",
                    json!({"action": "measures", "edit": "insert_after"}),
                ),
                act("Remove", json!({"action": "measures", "edit": "remove"})),
                sep(),
                sub(
                    "Meter",
                    [(2, 4), (3, 4), (4, 4), (6, 8), (9, 8), (12, 8)]
                        .iter()
                        .map(|(count, unit)| {
                            act(
                                &format!("{count}/{unit}"),
                                json!({"action": "meter", "count": count, "unit": unit}),
                            )
                        })
                        .collect(),
                ),
                sub(
                    "Barline",
                    [
                        ("Single", "single"),
                        ("Double", "dbl"),
                        ("Final", "end"),
                        ("Repeat start", "rptstart"),
                        ("Repeat end", "rptend"),
                        ("Repeat both", "rptboth"),
                        ("Invisible", "invis"),
                    ]
                    .iter()
                    .map(|(label, kind)| act(label, json!({"action": "barline", "kind": kind})))
                    .collect(),
                ),
                sub(
                    "Break",
                    [("System", "system"), ("Page", "page"), ("None", "none")]
                        .iter()
                        .map(|(label, kind)| act(label, json!({"action": "break", "kind": kind})))
                        .collect(),
                ),
            ],
        ),
        sub(
            "Transform",
            vec![
                transform(
                    "Transpose up an octave",
                    json!({"name": "transpose", "semitones": 12}),
                ),
                transform(
                    "Transpose down an octave",
                    json!({"name": "transpose", "semitones": -12}),
                ),
                transform(
                    "Transpose up a fifth",
                    json!({"name": "transpose", "semitones": 7, "steps": 4}),
                ),
                transform(
                    "Transpose down a fifth",
                    json!({"name": "transpose", "semitones": -7, "steps": -4}),
                ),
                sep(),
                transform("Invert", json!({"name": "invert"})),
                transform("Retrograde", json!({"name": "retrograde"})),
                sep(),
                transform("Augment", json!({"name": "stretch", "factor": [2, 1]})),
                transform("Diminish", json!({"name": "stretch", "factor": [1, 2]})),
                transform("Repeat", json!({"name": "repeat", "count": 2})),
                sep(),
                form("Transpose...", "transpose", state),
                form("Stretch...", "stretch", state),
                form("Repeat...", "repeat", state),
            ],
        ),
    ])
}

/// **What a pick asks for.**
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// An edit: the editor's verb, as it is written.
    Act(Value),
    /// A step back through the history.
    Undo,
    /// A step forward.
    Redo,
    /// Everything on the page, selected.
    SelectAll,
    /// A form, opened over the window.
    Dialog(Form),
    /// The score, written to its file.
    Save,
    /// Play, or stop what plays.
    Play,
    /// Back to the start.
    Rewind,
    /// Whether a pass loops.
    Loop(bool),
    /// The window looks at the score this way.
    Layout(View),
    /// A press on empty staff writes a note, or stops writing one.
    Entry(bool),
    /// The value a note is entered with.
    Value(Ratio),
    /// A verb this menu never wrote.
    Unknown,
}

/// **What the verb a pick reported asks for**; `state` is the entry's state
/// after the pick, where it holds one.
#[must_use]
pub fn read(verb: &str, state: Option<i64>) -> Pick {
    if verb.starts_with('{') {
        return serde_json::from_str(verb).map_or(Pick::Unknown, Pick::Act);
    }
    if let Some(form) = verb.strip_prefix("dialog:") {
        return Form::named(form).map_or(Pick::Unknown, Pick::Dialog);
    }
    if let Some(view) = verb.strip_prefix("layout:").and_then(View::parse) {
        return Pick::Layout(view);
    }
    if let Some(value) = verb.strip_prefix("value:") {
        let ratio = value.split_once('/').and_then(|(n, d)| {
            let (n, d): (i64, i64) = (n.parse().ok()?, d.parse().ok()?);
            (n > 0 && d > 0).then(|| Ratio::new(n, d))
        });
        return ratio.map_or(Pick::Unknown, Pick::Value);
    }
    match verb {
        "undo" => Pick::Undo,
        "redo" => Pick::Redo,
        "select_all" => Pick::SelectAll,
        "save" => Pick::Save,
        "play" => Pick::Play,
        "rewind" => Pick::Rewind,
        "loop" => Pick::Loop(state.is_none_or(|on| on != 0)),
        "entry" => Pick::Entry(state.is_none_or(|on| on != 0)),
        _ => Pick::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::verbs::Action;

    fn state() -> State<'static> {
        State {
            view: View::Page,
            entry: true,
            value: Ratio::new(1, 4),
            paper: Some("A4"),
            landscape: false,
            dialogs: true,
            looping: false,
        }
    }

    /// Every entry of a menu, however deep.
    fn entries(menu: &Value, out: &mut Vec<Value>) {
        for entry in menu.as_array().into_iter().flatten() {
            match entry.get("menu") {
                Some(inner) => entries(inner, out),
                None if entry.is_object() => out.push(entry.clone()),
                None => {}
            }
        }
    }

    #[test]
    fn every_entry_that_edits_is_a_verb_the_editor_reads() {
        let mut all = Vec::new();
        entries(&menu(&state()), &mut all);
        assert!(
            all.len() > 50,
            "the bar holds the application: {}",
            all.len()
        );
        for entry in &all {
            let verb = entry["verb"].as_str().expect("a verb");
            match read(verb, entry["checked"].as_bool().map(i64::from)) {
                Pick::Unknown => panic!("{verb} is a verb nothing reads"),
                Pick::Act(action) => {
                    serde_json::from_value::<Action>(action.clone())
                        .unwrap_or_else(|why| panic!("{action} is no verb of the editor: {why}"));
                }
                _ => {}
            }
        }
    }

    #[test]
    fn the_bar_shows_the_state_it_is_built_from() {
        let mut all = Vec::new();
        entries(&menu(&state()), &mut all);
        let on = |verb: &str| {
            all.iter()
                .find(|e| e["verb"] == verb)
                .and_then(|e| e["checked"].as_bool())
        };
        assert_eq!(on("layout:page"), Some(true));
        assert_eq!(on("layout:continuous"), Some(false));
        assert_eq!(on("value:1/4"), Some(true));
        assert_eq!(on("value:1/8"), Some(false));
        assert_eq!(on("entry"), Some(true));
        let a4 = all.iter().find(|e| e["label"] == "A4").unwrap();
        assert_eq!(a4["checked"], true);
    }

    #[test]
    fn a_pick_is_read_back_as_what_it_asks() {
        assert_eq!(read("undo", None), Pick::Undo);
        assert_eq!(
            read("layout:continuous", Some(1)),
            Pick::Layout(View::Continuous)
        );
        assert_eq!(read("value:1/8", Some(1)), Pick::Value(Ratio::new(1, 8)));
        assert_eq!(read("entry", Some(0)), Pick::Entry(false));
        assert_eq!(read("value:0/8", None), Pick::Unknown);
        assert_eq!(read("fold", None), Pick::Unknown);
        assert_eq!(read("dialog:page", None), Pick::Dialog(Form::Page));
        assert_eq!(read("save", None), Pick::Save);
        assert_eq!(read("play", None), Pick::Play);
        assert_eq!(read("loop", Some(1)), Pick::Loop(true));
        assert_eq!(read("dialog:fold", None), Pick::Unknown);
    }
}
