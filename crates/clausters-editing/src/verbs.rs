//! **The verbs a window answers, written once**: each verb's name, the chords
//! that perform it unless somebody bound others, and the one sentence every
//! place that shows it uses.
//!
//! A verb is shown in several places -- a menu entry, a tool's tip, a row of
//! the key sheet -- and performed from several more: a key, a click, a
//! client's call. Every one of them is a caller of the same verb, so none of
//! them names it in words of its own. Before this table the words lived beside
//! each caller, and they drifted: one verb was "Zoom to fit" in a menu and
//! "Show the whole view" on the key sheet. Now a menu, a toolbar and the host's
//! key table read the row, and a verb added is added here.
//!
//! **A client's handler is one more caller**, and the one nothing here ties to
//! the table: `docs/verbs.md` names, row by row, the members of the two
//! clients that do what each verb does -- or that none does yet -- and
//! `clients/python/tests/test_verbs.py` holds it to this table and to both
//! clients.
//!
//! **What a row is not**: where a verb sits in a menu. That is the menu's
//! business -- a menu is a view over the verbs and orders them as it likes --
//! so a row carries no menu path.
//!
//! The rows are grouped by **scope**, the host key table's own unit: the
//! window's verbs ([`WINDOW`]), which every window answers and the host lays
//! down as its table's own rows, and one scope per application mode that adds
//! verbs of its own ([`MULTITRACK`], [`SCORE`], [`NOTE_ENTRY`]), which the
//! window declares through its `verbs` prop ([`declaration`]). The same name
//! in two scopes is two rows: `delete` over a score's selection says what it
//! deletes there.

use serde_json::{Map, Value, json};

/// One verb of a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verb {
    /// What the verb is called on the wire: a menu entry's `verb`, a key
    /// table's row, the tag an application is handed.
    pub name: &'static str,
    /// The chords that perform it unless the user bound others -- none for a
    /// verb only a menu or a tool reaches by default.
    pub keys: &'static [&'static str],
    /// What every place that shows it calls it: a menu entry, a tool's tip, a
    /// row of the key sheet.
    pub label: &'static str,
}

const fn verb(name: &'static str, keys: &'static [&'static str], label: &'static str) -> Verb {
    Verb { name, keys, label }
}

/// **The window's own verbs**: what every window answers, the host's key
/// table's own rows. The host performs most of them; `undo`, `redo`, `save`
/// and `close` it hands to the window's owner.
pub const WINDOW: &[Verb] = &[
    verb("undo", &["Ctrl+Z"], "Undo"),
    // Ctrl+Shift+Z is the spelling that works on a keyboard with no Y where
    // an English one has one.
    verb("redo", &["Ctrl+Shift+Z", "Ctrl+Y"], "Redo"),
    verb("save", &["Ctrl+S"], "Save"),
    verb("close", &[], "Close"),
    verb("view_all", &["R"], "Zoom to fit"),
    verb("play", &["Space"], "Play or stop"),
    verb("loop", &["L"], "Loop, on or off"),
    verb("to_start", &["Home"], "Go to start"),
    verb("to_end", &["End"], "Go to end"),
    verb("copy", &["Ctrl+C"], "Copy"),
    verb("cut", &["Ctrl+X"], "Cut"),
    verb("paste", &["Ctrl+V"], "Paste"),
    verb("mix", &["Ctrl+Shift+V"], "Paste mixed"),
    verb("quantize", &["Q"], "Quantize"),
    verb("split", &["E"], "Split"),
    verb("join", &["J"], "Join"),
    verb("delete", &["Delete", "Backspace"], "Delete"),
    verb("select_all", &["Ctrl+A"], "Select all"),
    verb("keys", &["F1"], "Keyboard shortcuts"),
];

/// The multitrack window's scope.
pub const MULTITRACK_SCOPE: &str = "multitrack";

/// **What the multitrack editor adds**: its transport's two other buttons,
/// where a pass ends, its rows' own commands, how its boxes of notes are
/// drawn, and the automatic crossfade.
pub const MULTITRACK: &[Verb] = &[
    verb("pause", &[], "Pause"),
    verb("stop", &[], "Stop"),
    verb("stop_at_end", &["Shift+L"], "Stop at end, on or off"),
    verb("crossfade", &[], "Crossfade overlaps, on or off"),
    verb("add_track", &[], "Add track"),
    verb("reset_heights", &[], "Reset track heights"),
    verb("compact_tracks", &[], "Compact tracks"),
    verb("notes_roll", &[], "Notes as rolls"),
    verb("notes_score", &[], "Notes as scores"),
];

/// The score window's own scope, in force whenever it is open.
pub const SCORE_SCOPE: &str = "score";

/// Note entry's scope, read before the window's while the mode is on.
pub const NOTE_ENTRY_SCOPE: &str = "note_entry";

/// **The score window's own keys**: over the selection, and into note entry.
/// The chords are the settled ones of notation programs.
pub const SCORE: &[Verb] = &[
    verb("entry", &["N"], "Note entry on or off"),
    // over the selection: what note entry's own rows, read first while the
    // window is in it, give the cursor and the note written
    verb(
        "delete",
        &["Delete", "Backspace"],
        "Delete what is selected",
    ),
    verb("deselect", &["Escape"], "Select nothing"),
    verb("select_left", &["Left"], "Select the item before"),
    verb("select_right", &["Right"], "Select the item after"),
    verb("step_up", &["Up"], "Up a step"),
    verb("step_down", &["Down"], "Down a step"),
    verb("octave_up", &["Ctrl+Up"], "Up an octave"),
    verb("octave_down", &["Ctrl+Down"], "Down an octave"),
];

/// **Note entry's keys**: the letters write, the digits choose the value.
pub const NOTE_ENTRY: &[Verb] = &[
    verb("entry", &["N"], "Note entry on or off"),
    verb("entry_off", &["Escape"], "Leave note entry"),
    verb("pitch_a", &["A"], "Write an A"),
    verb("pitch_b", &["B"], "Write a B"),
    verb("pitch_c", &["C"], "Write a C"),
    verb("pitch_d", &["D"], "Write a D"),
    verb("pitch_e", &["E"], "Write an E"),
    verb("pitch_f", &["F"], "Write an F"),
    verb("pitch_g", &["G"], "Write a G"),
    verb("chord_a", &["Shift+A"], "Add an A to the chord"),
    verb("chord_b", &["Shift+B"], "Add a B to the chord"),
    verb("chord_c", &["Shift+C"], "Add a C to the chord"),
    verb("chord_d", &["Shift+D"], "Add a D to the chord"),
    verb("chord_e", &["Shift+E"], "Add an E to the chord"),
    verb("chord_f", &["Shift+F"], "Add an F to the chord"),
    verb("chord_g", &["Shift+G"], "Add a G to the chord"),
    verb("cursor_left", &["Left"], "Cursor back"),
    verb("cursor_right", &["Right"], "Cursor forward"),
    verb("bar_left", &["Ctrl+Left"], "Cursor to the bar before"),
    verb("bar_right", &["Ctrl+Right"], "Cursor to the bar after"),
    verb("staff_up", &["Alt+Up"], "Cursor to the staff above"),
    verb("staff_down", &["Alt+Down"], "Cursor to the staff below"),
    verb("step_up", &["Up"], "Up a step"),
    verb("step_down", &["Down"], "Down a step"),
    verb("octave_up", &["Ctrl+Up"], "Up an octave"),
    verb("octave_down", &["Ctrl+Down"], "Down an octave"),
    verb("voice_1", &["Ctrl+Alt+1"], "Voice 1"),
    verb("voice_2", &["Ctrl+Alt+2"], "Voice 2"),
    verb("voice_3", &["Ctrl+Alt+3"], "Voice 3"),
    verb("voice_4", &["Ctrl+Alt+4"], "Voice 4"),
    verb("value_64th", &["1"], "Sixty-fourth note"),
    verb("value_32nd", &["2"], "Thirty-second note"),
    verb("value_16th", &["3"], "Sixteenth note"),
    verb("value_eighth", &["4"], "Eighth note"),
    verb("value_quarter", &["5"], "Quarter note"),
    verb("value_half", &["6"], "Half note"),
    verb("value_whole", &["7"], "Whole note"),
    verb("dot", &["."], "Dotted"),
    verb("enter_rest", &["0"], "Rest"),
];

/// **The row `name` is in `rows`**, if it is there.
#[must_use]
pub fn find(rows: &[Verb], name: &str) -> Option<Verb> {
    rows.iter().find(|v| v.name == name).copied()
}

/// **What a window with `scope`'s verbs calls `name`**: the scope's own row
/// first, then the window's, then the name read as words (`select_all` is
/// "Select all") for a verb neither has -- an application's, bound by a
/// user.
#[must_use]
pub fn label(scope: &[Verb], name: &str) -> String {
    find(scope, name)
        .or_else(|| find(WINDOW, name))
        .map_or_else(|| words(name), |v| v.label.to_string())
}

/// A name as words: underscores are spaces and the first letter is a
/// capital.
#[must_use]
pub fn words(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// **A window's `verbs` prop**: each scope to its verbs, each verb to its
/// default chords (`keys`) and its words (`label`) -- what the host lays into
/// its key table under whatever the user bound.
#[must_use]
pub fn declaration(scopes: &[(&str, &[Verb])]) -> Value {
    let table: Map<String, Value> = scopes
        .iter()
        .map(|(scope, rows)| {
            let verbs: Map<String, Value> = rows
                .iter()
                .map(|v| {
                    (
                        v.name.to_string(),
                        json!({"keys": v.keys, "label": v.label}),
                    )
                })
                .collect();
            ((*scope).to_string(), Value::Object(verbs))
        })
        .collect();
    Value::Object(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scope names each verb once, with words of its own.
    #[test]
    fn every_scope_names_each_verb_once_with_its_words() {
        for rows in [WINDOW, MULTITRACK, SCORE, NOTE_ENTRY] {
            for (i, v) in rows.iter().enumerate() {
                assert!(!v.label.is_empty(), "{} has no words", v.name);
                assert!(
                    rows[i + 1..].iter().all(|w| w.name != v.name),
                    "{} is in a scope twice",
                    v.name
                );
            }
        }
    }

    /// An application's scope adds verbs and does not rename the window's:
    /// a verb the window has is shown in the window's words wherever it is.
    #[test]
    fn the_multitrack_adds_to_the_window_and_renames_nothing() {
        for v in MULTITRACK {
            assert!(find(WINDOW, v.name).is_none(), "{} is the window's", v.name);
        }
        assert_eq!(label(MULTITRACK, "view_all"), "Zoom to fit");
        assert_eq!(label(MULTITRACK, "stop_at_end"), "Stop at end, on or off");
        assert_eq!(label(MULTITRACK, "make_coffee"), "Make coffee");
    }

    /// The declaration is the table, row for row.
    #[test]
    fn the_declaration_is_the_table() {
        let declared = declaration(&[(MULTITRACK_SCOPE, MULTITRACK)]);
        let scope = declared[MULTITRACK_SCOPE].as_object().unwrap();
        assert_eq!(scope.len(), MULTITRACK.len());
        assert_eq!(scope["stop_at_end"]["keys"], json!(["Shift+L"]));
        assert_eq!(scope["add_track"]["label"], json!("Add track"));
    }
}
