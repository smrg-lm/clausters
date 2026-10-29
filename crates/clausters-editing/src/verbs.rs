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

/// **How an action is presented and what its state reads as.** An action is
/// not a history entry: a play, a view switch and a dialog are actions that
/// record nothing, and the kind says only how a menu, a tool and a sheet show
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionKind {
    /// Performed once per invocation: an undo, a copy, a save.
    Command,
    /// A switch, on or off: the loop, the file list.
    Toggle,
    /// One of a group, exactly one of which is in force: the views of a take.
    Choice,
}

/// **What an invocation addresses**, resolved when it is asked rather than
/// remembered by whoever shows the action.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ActionTarget {
    /// An object a member of an editing context holds -- an audio editor's
    /// file in focus.
    Document {
        /// The member of the editing context.
        member: u32,
        /// The member's stable object number.
        object: i64,
    },
    /// A text field holding the keyboard focus, whose own editing is read
    /// before any document's.
    Text {
        /// The field's widget id.
        widget: i32,
    },
    /// The window itself: opening a file, the key sheet, the file list.
    Window,
}

/// **The destination a paste must match**: what the clipboard's contents
/// are compared with before a paste or a mix is offered, so a refusal is
/// shown instead of performed. One kind per structure a paste lands in; an
/// application whose paste lands in another adds its kind here.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PasteTarget {
    /// Samples, as an audio take holds them.
    Samples {
        /// The destination's channels; a paste needs as many.
        channels: u32,
        /// The destination's rate; a mix needs the same one.
        rate: f64,
        /// The server connection the destination's samples live on.
        connection: u64,
        /// That connection's incarnation.
        generation: u64,
    },
}

/// **An action's state as it reads now**: computed from what the owner holds
/// each time it is asked, never a second copy of application state. A menu
/// entry, a tool and the key sheet read it; a key, a pick and a script call
/// are refused by it alike.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionState {
    /// How it is presented.
    pub kind: ActionKind,
    /// The group a [`ActionKind::Choice`] belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// What an invocation now would address.
    pub target: ActionTarget,
    /// Whether it can be invoked now.
    pub enabled: bool,
    /// Why not, when it cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether a toggle is on, or a choice is the one in force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    /// The name of the choice in force in the group, for a choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Whether work the action depends on is still pending.
    #[serde(default)]
    pub pending: bool,
    /// The destination a paste or a mix is checked against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paste: Option<PasteTarget>,
    /// **Whether the only objection is that nothing is selected.** The
    /// selection is the host's first -- a sweep is drawn there and the owner
    /// hears of it after -- so a host reads its own selection for such an
    /// action and never refuses an invocation on this reading alone: the owner
    /// refuses it again if nothing is selected when it arrives.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unselected: bool,
}

impl ActionState {
    /// The state of `verb` addressing `target`: available, unchecked, with
    /// nothing pending.
    #[must_use]
    pub fn new(verb: &Verb, target: ActionTarget) -> Self {
        Self {
            kind: verb.kind,
            group: verb.group.map(str::to_string),
            target,
            enabled: true,
            reason: None,
            checked: None,
            value: None,
            pending: false,
            paste: None,
            unselected: false,
        }
    }

    /// Marks it unavailable, for `reason`. The first reason stays, except
    /// that any reason outweighs an empty selection ([`refuse_unselected`]).
    ///
    /// [`refuse_unselected`]: Self::refuse_unselected
    pub fn refuse(&mut self, reason: &str) {
        if self.enabled || self.unselected {
            self.enabled = false;
            self.unselected = false;
            self.reason = Some(reason.into());
        }
    }

    /// Marks it unavailable because nothing is selected, unless it is
    /// unavailable already ([`ActionState::unselected`]).
    pub fn refuse_unselected(&mut self) {
        if self.enabled {
            self.enabled = false;
            self.unselected = true;
            self.reason = Some("nothing is selected".into());
        }
    }
}

/// **The verbs a focused text field answers itself**, before the document
/// behind it is asked.
#[must_use]
pub fn text_action(name: &str) -> bool {
    matches!(
        name,
        "undo" | "redo" | "cut" | "copy" | "paste" | "delete" | "select_all"
    )
}

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
    /// How it is presented.
    pub kind: ActionKind,
    /// The group of a [`ActionKind::Choice`]: the verbs of one group are its
    /// alternatives.
    pub group: Option<&'static str>,
}

const fn verb(name: &'static str, keys: &'static [&'static str], label: &'static str) -> Verb {
    Verb {
        name,
        keys,
        label,
        kind: ActionKind::Command,
        group: None,
    }
}

const fn toggle(name: &'static str, keys: &'static [&'static str], label: &'static str) -> Verb {
    Verb {
        kind: ActionKind::Toggle,
        ..verb(name, keys, label)
    }
}

const fn choice(
    group: &'static str,
    name: &'static str,
    keys: &'static [&'static str],
    label: &'static str,
) -> Verb {
    Verb {
        kind: ActionKind::Choice,
        group: Some(group),
        ..verb(name, keys, label)
    }
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
    toggle("loop", &["L"], "Loop, on or off"),
    verb("to_start", &["Home"], "Go to start"),
    verb("to_end", &["End"], "Go to end"),
    verb("copy", &["Ctrl+C"], "Copy"),
    verb("cut", &["Ctrl+X"], "Cut"),
    verb("paste", &["Ctrl+V"], "Paste"),
    verb("mix", &["Ctrl+Shift+V"], "Paste mixed"),
    verb("quantize", &["Q"], "Quantize"),
    verb("split", &["E"], "Split"),
    verb("join", &["J"], "Join"),
    // the snap-to-grid switch: a drag, a paste, a split and step entry land
    // on the grid while it is on, where the hand puts them while it is off
    toggle("snap", &["G"], "Snap to grid, on or off"),
    verb("delete", &["Delete", "Backspace"], "Delete"),
    verb("select_all", &["Ctrl+A"], "Select all"),
    verb("keys", &["F1"], "Keyboard shortcuts"),
];

/// The audio window's key scope.
pub const AUDIO_SCOPE: &str = "audio";

/// Audio view presets; menu commands share the application's JSON verbs.
pub const AUDIO: &[Verb] = &[
    verb("new_file", &[], "New file..."),
    verb("open_file", &["Ctrl+O"], "Open file..."),
    verb("save_as", &["Ctrl+Shift+S"], "Save as..."),
    verb("show_all_channels", &[], "Show all channels"),
    choice("views", "view_wave", &["W"], "Waveform"),
    choice("layer", "layer_take", &[], "Edit audio"),
    choice("layer", "show_envelope", &[], "Edit gain envelope"),
    verb("apply", &[], "Apply effect"),
    verb("apply_envelope", &[], "Apply gain envelope"),
    verb("reverse", &[], "Reverse"),
    choice("views", "view_spectrogram", &["S"], "Spectrogram"),
    choice("views", "view_both", &["B"], "Waveform and spectrogram"),
    toggle("toggle_files", &[], "Files"),
    toggle("spectrogram_settings", &[], "Spectrogram settings"),
    verb("close_file", &[], "Close file"),
];

/// The multitrack window's scope.
pub const MULTITRACK_SCOPE: &str = "multitrack";

/// **What the multitrack editor adds**: its transport's two other buttons,
/// where a pass ends, its rows' own commands, how its boxes of samples and
/// of notes are drawn, and the automatic crossfade.
pub const MULTITRACK: &[Verb] = &[
    verb("pause", &[], "Pause"),
    verb("stop", &[], "Stop"),
    toggle("stop_at_end", &["Shift+L"], "Stop at end, on or off"),
    toggle("crossfade", &[], "Crossfade overlaps, on or off"),
    verb("add_track", &[], "Add track"),
    verb("reset_heights", &[], "Reset track heights"),
    verb("compact_tracks", &[], "Compact tracks"),
    // a box of samples is the audio editor's view of its take, and is drawn
    // as either of that view's pictures
    choice("views", "view_wave", &["W"], "Waveform"),
    choice("views", "view_spectrogram", &["S"], "Spectrogram"),
    choice("notes", "notes_roll", &[], "Notes as rolls"),
    choice("notes", "notes_score", &[], "Notes as scores"),
];

/// The score window's own scope, in force whenever it is open.
pub const SCORE_SCOPE: &str = "score";

/// Note entry's scope, read before the window's while the mode is on.
pub const NOTE_ENTRY_SCOPE: &str = "note_entry";

/// **The score window's own keys**: over the selection, and into note entry.
/// The chords are the settled ones of notation programs.
pub const SCORE: &[Verb] = &[
    toggle("entry", &["N"], "Note entry on or off"),
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
    toggle("entry", &["N"], "Note entry on or off"),
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
    choice("voice", "voice_1", &["Ctrl+Alt+1"], "Voice 1"),
    choice("voice", "voice_2", &["Ctrl+Alt+2"], "Voice 2"),
    choice("voice", "voice_3", &["Ctrl+Alt+3"], "Voice 3"),
    choice("voice", "voice_4", &["Ctrl+Alt+4"], "Voice 4"),
    choice("value", "value_64th", &["1"], "Sixty-fourth note"),
    choice("value", "value_32nd", &["2"], "Thirty-second note"),
    choice("value", "value_16th", &["3"], "Sixteenth note"),
    choice("value", "value_eighth", &["4"], "Eighth note"),
    choice("value", "value_quarter", &["5"], "Quarter note"),
    choice("value", "value_half", &["6"], "Half note"),
    choice("value", "value_whole", &["7"], "Whole note"),
    toggle("dot", &["."], "Dotted"),
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
                        json!({"keys": v.keys, "label": v.label, "kind": v.kind}),
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
        for rows in [WINDOW, AUDIO, MULTITRACK, SCORE, NOTE_ENTRY] {
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

    /// **An ellipsis says more is asked before the verb acts**, as menus
    /// write it: a switch or a choice acts at once, whatever it shows, so its
    /// words never end in one.
    #[test]
    fn a_switch_or_a_choice_never_ends_in_an_ellipsis() {
        for rows in [WINDOW, AUDIO, MULTITRACK, SCORE, NOTE_ENTRY] {
            for v in rows.iter().filter(|v| v.kind != ActionKind::Command) {
                assert!(!v.label.ends_with("..."), "{} acts at once", v.name);
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
