//! **The score editor's keys**: the scopes its modes are -- the window's own
//! (`score`) and note entry's (`note_entry`) -- each verb's default chords,
//! and what a key sheet calls it.
//!
//! They are the application's and not the host's: the window carries them
//! (its `verbs` prop, [`verbs`]), whether or not it has any chrome, and the
//! host lays them into its key table under whatever the user bound -- so a
//! `[gui.keys.note_entry]` in a config still wins, and F1 lists them in these
//! words. The chords are the settled ones of notation programs.

use serde_json::Value;

use clausters_editing::verbs::{self, NOTE_ENTRY_SCOPE, SCORE_SCOPE};

/// The window's own scope, in force whenever it is open.
pub const SCORE: &str = SCORE_SCOPE;

/// Note entry's scope, read before the window's while the mode is on.
pub const NOTE_ENTRY: &str = NOTE_ENTRY_SCOPE;

/// **The window's `verbs` prop**: each scope to its verbs, each verb to its
/// default chords (`keys`) and its words (`label`) -- the rows of the one
/// verb table ([`clausters_editing::verbs`]).
#[must_use]
pub fn verbs() -> Value {
    verbs::declaration(&[(SCORE, verbs::SCORE), (NOTE_ENTRY, verbs::NOTE_ENTRY)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every verb the editor answers a key with is declared, and declared
    /// once per scope with words of its own.
    #[test]
    fn every_scope_names_each_verb_once_with_its_words() {
        let all = verbs();
        for (scope, rows) in [(SCORE, verbs::SCORE), (NOTE_ENTRY, verbs::NOTE_ENTRY)] {
            let declared = all[scope].as_object().expect("a scope is a table");
            assert_eq!(declared.len(), rows.len(), "{scope}: no verb twice");
            for v in rows.iter() {
                assert_eq!(declared[v.name]["keys"], json!(v.keys));
                assert!(!v.label.is_empty() && declared[v.name]["label"] == v.label);
            }
        }
    }
}
