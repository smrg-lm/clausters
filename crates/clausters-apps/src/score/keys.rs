//! **The score editor's keys**: the scopes its modes are -- the window's own
//! (`score`) and note entry's (`note_entry`) -- each verb's default chords,
//! and what a key sheet calls it.
//!
//! They are the application's and not the host's: the window carries them
//! (its `verbs` prop, [`verbs`]), whether or not it has any chrome, and the
//! host lays them into its key table under whatever the user bound -- so a
//! `[gui.keys.note_entry]` in a config still wins, and F1 lists them in these
//! words. The chords are the settled ones of notation programs.

use serde_json::{Map, Value, json};

/// One verb of a scope: its name, its default chords, and its words.
type Row = (&'static str, &'static [&'static str], &'static str);

/// The window's own scope, in force whenever it is open.
pub const SCORE: &str = "score";

/// Note entry's scope, read before the window's while the mode is on.
pub const NOTE_ENTRY: &str = "note_entry";

/// The window's own keys: over the selection, and into note entry.
const SCORE_ROWS: &[Row] = &[
    ("entry", &["N"], "Note entry on or off"),
    // over the selection: what note entry's own rows, read first while the
    // window is in it, give the cursor and the note written
    (
        "delete",
        &["Delete", "Backspace"],
        "Delete what is selected",
    ),
    ("deselect", &["Escape"], "Select nothing"),
    ("select_left", &["Left"], "Select the item before"),
    ("select_right", &["Right"], "Select the item after"),
    ("step_up", &["Up"], "Up a step"),
    ("step_down", &["Down"], "Down a step"),
    ("octave_up", &["Ctrl+Up"], "Up an octave"),
    ("octave_down", &["Ctrl+Down"], "Down an octave"),
];

/// Note entry's keys: the letters write, the digits choose the value.
const NOTE_ENTRY_ROWS: &[Row] = &[
    ("entry", &["N"], "Note entry on or off"),
    ("entry_off", &["Escape"], "Leave note entry"),
    ("pitch_a", &["A"], "Write an A"),
    ("pitch_b", &["B"], "Write a B"),
    ("pitch_c", &["C"], "Write a C"),
    ("pitch_d", &["D"], "Write a D"),
    ("pitch_e", &["E"], "Write an E"),
    ("pitch_f", &["F"], "Write an F"),
    ("pitch_g", &["G"], "Write a G"),
    ("chord_a", &["Shift+A"], "Add an A to the chord"),
    ("chord_b", &["Shift+B"], "Add a B to the chord"),
    ("chord_c", &["Shift+C"], "Add a C to the chord"),
    ("chord_d", &["Shift+D"], "Add a D to the chord"),
    ("chord_e", &["Shift+E"], "Add an E to the chord"),
    ("chord_f", &["Shift+F"], "Add an F to the chord"),
    ("chord_g", &["Shift+G"], "Add a G to the chord"),
    ("cursor_left", &["Left"], "Cursor back"),
    ("cursor_right", &["Right"], "Cursor forward"),
    ("bar_left", &["Ctrl+Left"], "Cursor to the bar before"),
    ("bar_right", &["Ctrl+Right"], "Cursor to the bar after"),
    ("staff_up", &["Alt+Up"], "Cursor to the staff above"),
    ("staff_down", &["Alt+Down"], "Cursor to the staff below"),
    ("step_up", &["Up"], "Up a step"),
    ("step_down", &["Down"], "Down a step"),
    ("octave_up", &["Ctrl+Up"], "Up an octave"),
    ("octave_down", &["Ctrl+Down"], "Down an octave"),
    ("voice_1", &["Ctrl+Alt+1"], "Voice 1"),
    ("voice_2", &["Ctrl+Alt+2"], "Voice 2"),
    ("voice_3", &["Ctrl+Alt+3"], "Voice 3"),
    ("voice_4", &["Ctrl+Alt+4"], "Voice 4"),
    ("value_64th", &["1"], "Sixty-fourth note"),
    ("value_32nd", &["2"], "Thirty-second note"),
    ("value_16th", &["3"], "Sixteenth note"),
    ("value_eighth", &["4"], "Eighth note"),
    ("value_quarter", &["5"], "Quarter note"),
    ("value_half", &["6"], "Half note"),
    ("value_whole", &["7"], "Whole note"),
    ("dot", &["."], "Dotted"),
    ("enter_rest", &["0"], "Rest"),
];

/// **The window's `verbs` prop**: each scope to its verbs, each verb to its
/// default chords (`keys`) and its words (`label`).
#[must_use]
pub fn verbs() -> Value {
    let scope = |rows: &[Row]| {
        let verbs: Map<String, Value> = rows
            .iter()
            .map(|(verb, keys, label)| ((*verb).to_string(), json!({"keys": keys, "label": label})))
            .collect();
        Value::Object(verbs)
    };
    json!({SCORE: scope(SCORE_ROWS), NOTE_ENTRY: scope(NOTE_ENTRY_ROWS)})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every verb the editor answers a key with is declared, and declared
    /// once per scope with words of its own.
    #[test]
    fn every_scope_names_each_verb_once_with_its_words() {
        let all = verbs();
        for (scope, rows) in [(SCORE, SCORE_ROWS), (NOTE_ENTRY, NOTE_ENTRY_ROWS)] {
            let declared = all[scope].as_object().expect("a scope is a table");
            assert_eq!(declared.len(), rows.len(), "{scope}: no verb twice");
            for (verb, keys, label) in rows {
                assert_eq!(declared[*verb]["keys"], json!(keys));
                assert!(!label.is_empty() && declared[*verb]["label"] == *label);
            }
        }
    }
}
