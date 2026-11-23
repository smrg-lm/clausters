//! **The verbs a window answers, written once**: each verb's name, the chords
//! that perform it unless somebody bound others, and the one sentence every
//! place that shows it uses.
//!
//! A verb is shown in several places -- a menu entry, a tool's tip, a row of
//! the key sheet -- and performed from several more: a key, a click, a
//! client's call. Every one of them is a caller of the same verb, so none of
//! them names it in words of its own. Before this table the words lived beside
//! each caller, and they drifted: one verb was "Zoom to fit" in a menu and
//! "Show whole view" on the key sheet. Now a menu, a toolbar and the host's
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
//! verbs of its own ([`MULTITRACK`], [`NOTES`], [`SCORE`], [`NOTE_ENTRY`]), which the
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
    /// **Whether it is not this window's at all**, whatever its state: a verb
    /// of the table the structure cannot do. It is refused with why, and a key
    /// sheet does not list it among what the window does.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub absent: bool,
    /// **The icon its tools show now**, by the name the host carries it
    /// under, in place of their own: the play tool shows `pause` while the
    /// transport rolls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
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
            absent: false,
            icon: None,
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

    /// Marks it as **not this window's at all**: refused with `reason`, so a key
    /// that asks for it says why, and left off the window's key sheet
    /// ([`ActionState::absent`]).
    pub fn refuse_absent(&mut self, reason: &str) {
        self.refuse(reason);
        self.absent = true;
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
    // **the window's file**, one set of words in every application: what
    // has none refuses them as absent
    verb("new_file", &["Ctrl+N"], "New"),
    verb("open_file", &["Ctrl+O"], "Open..."),
    verb("save", &["Ctrl+S"], "Save"),
    verb("save_as", &["Ctrl+Shift+S"], "Save as..."),
    verb("close_file", &[], "Close file"),
    verb("close", &[], "Close"),
    verb("view_all", &["R"], "Zoom to fit"),
    verb("play", &["Space"], "Play or stop"),
    // **the transport's two buttons**: play or pause where it stands, and
    // stop, which goes back to the position cursor
    verb("pause", &[], "Play or pause"),
    verb("stop", &[], "Stop"),
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
    // what the window's time ruler counts in, the window's own
    choice("ruler", "ruler_beats", &[], "Bars and beats"),
    choice("ruler", "ruler_time", &[], "Minutes and seconds"),
    choice("ruler", "ruler_samples", &[], "Samples"),
];

/// The audio window's key scope.
pub const AUDIO_SCOPE: &str = "audio";

/// Audio view presets; menu commands share the application's JSON verbs.
pub const AUDIO: &[Verb] = &[
    // a new take asks its shape first, which its ellipsis says: the one
    // file verb a scope restates, and only to add that
    verb("new_file", &["Ctrl+N"], "New..."),
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
];

/// **The end switch**: whether a pass that is not looped stops where the
/// contents end, or rolls on past them. Every application whose transport
/// can roll on past its contents has it -- the multitrack's regions, a
/// roll's notes, a page's score -- where a take always ends with its last
/// frame.
pub const STOP_AT_END: Verb = toggle("stop_at_end", &["Shift+L"], "Stop at end, on or off");

/// The multitrack window's scope.
pub const MULTITRACK_SCOPE: &str = "multitrack";

/// **What the multitrack editor adds**: where a pass ends, its rows' own commands, how its boxes of samples are
/// drawn, and the automatic crossfade.
pub const MULTITRACK: &[Verb] = &[
    STOP_AT_END,
    toggle("crossfade", &[], "Crossfade overlaps, on or off"),
    verb("add_track", &[], "Add track"),
    verb("reset_heights", &[], "Reset track heights"),
    verb("compact_tracks", &[], "Compact tracks"),
    // a box of samples is the audio editor's view of its take, and is drawn
    // as either of that view's pictures
    choice("views", "view_wave", &["W"], "Waveform"),
    choice("views", "view_spectrogram", &["S"], "Spectrogram"),
];

/// The notes window's scope: the roll's own verbs.
pub const NOTES_SCOPE: &str = "notes";

/// **What the notes editor adds**: how its roll edits, and the joins of a
/// line. They are the roll's in both of its modes.
pub const NOTES: &[Verb] = &[
    STOP_AT_END,
    // one note at a time in a line: a gesture shortens what it covers
    toggle("monophonic", &["M"], "Edit monophonically, on or off"),
    // the inspector of the selected notes and the list of the events,
    // beside the roll
    toggle("events_panel", &[], "Events and inspector, shown or hidden"),
    // how the roll draws what a sequence holds beyond its boxes: slurs,
    // marks, the pitch line, the notes' curves and the lanes offered
    toggle("display_settings", &[], "Display settings"),
    verb("unjoin", &["Shift+J"], "Unjoin"),
    // the selection, or every note taken, moved: a semitone in MIDI mode, a
    // step of the key in notation mode, an octave in both
    verb("step_up", &["Up"], "Step up"),
    verb("step_down", &["Down"], "Step down"),
    verb("octave_up", &["Ctrl+Up"], "Octave up"),
    verb("octave_down", &["Ctrl+Down"], "Octave down"),
    // every note sharing a pitch, a channel, a voice or a staff with the
    // selection, selected
    verb("select_pitch", &[], "Select by pitch"),
    verb("select_channel", &[], "Select by channel"),
    verb("select_voice", &[], "Select by voice"),
    verb("select_staff", &[], "Select by staff"),
    // what a plain press on the grid does: the window's tool
    choice("tool", "tool_select", &["1"], "Pointer"),
    choice("tool", "tool_draw", &["2"], "Pencil"),
    choice("tool", "tool_erase", &["3"], "Eraser"),
    choice("tool", "tool_split", &["4"], "Scissors"),
    choice("tool", "tool_stretch", &["5"], "Stretch"),
    // the channel the MIDI mode edits, or every one
    choice("channel", "channel_all", &[], "All channels"),
    choice("channel", "channel_1", &[], "Channel 1"),
    choice("channel", "channel_2", &[], "Channel 2"),
    choice("channel", "channel_3", &[], "Channel 3"),
    choice("channel", "channel_4", &[], "Channel 4"),
    choice("channel", "channel_5", &[], "Channel 5"),
    choice("channel", "channel_6", &[], "Channel 6"),
    choice("channel", "channel_7", &[], "Channel 7"),
    choice("channel", "channel_8", &[], "Channel 8"),
    choice("channel", "channel_9", &[], "Channel 9"),
    choice("channel", "channel_10", &[], "Channel 10"),
    choice("channel", "channel_11", &[], "Channel 11"),
    choice("channel", "channel_12", &[], "Channel 12"),
    choice("channel", "channel_13", &[], "Channel 13"),
    choice("channel", "channel_14", &[], "Channel 14"),
    choice("channel", "channel_15", &[], "Channel 15"),
    choice("channel", "channel_16", &[], "Channel 16"),
    // the curves the sequence's format allows, added and taken away
    verb("curve_cc1", &[], "Add modulation curve"),
    verb("curve_cc11", &[], "Add expression curve"),
    verb("curve_bend", &[], "Add channel bend"),
    verb("curve_pressure", &[], "Add channel pressure"),
    verb("note_bend", &[], "Add note bend"),
    verb("note_pressure", &[], "Add note pressure"),
    verb("note_timbre", &[], "Add note timbre"),
    verb("curve_remove", &[], "Remove shown curve"),
    // the conversions: the mode is the sequence's format's, and going to
    // another format makes a copy, opened in a roll of its own
    verb("convert_notation", &[], "Convert to notation..."),
    verb("convert_midi1", &[], "Convert to MIDI 1.0"),
    verb("convert_mpe", &[], "Convert to MPE"),
    verb("convert_midi2", &[], "Convert to MIDI 2.0"),
    // what the notation mode shows, and the voice it edits
    choice("show", "show_system", &[], "Show system"),
    choice("show", "show_part", &[], "Show part"),
    choice("show", "show_staff", &[], "Show staff"),
    verb("staff_up", &[], "Edit staff above"),
    verb("staff_down", &[], "Edit staff below"),
    choice("voice", "voice_1", &["Ctrl+Alt+1"], "Voice 1"),
    choice("voice", "voice_2", &["Ctrl+Alt+2"], "Voice 2"),
    choice("voice", "voice_3", &["Ctrl+Alt+3"], "Voice 3"),
    choice("voice", "voice_4", &["Ctrl+Alt+4"], "Voice 4"),
    // what a note's tint tells apart
    choice("color", "color_voice", &[], "Color by voice"),
    choice("color", "color_staff", &[], "Color by staff"),
    choice("color", "color_part", &[], "Color by part"),
    // how a part writes its polyphony: the same music, written two ways
    choice("writing", "write_voices", &[], "Write polyphony in voices"),
    choice("writing", "write_ties", &[], "Write polyphony with ties"),
    // the marks a page writes over the selected notes
    verb("slur", &["S"], "Slur"),
    verb("slur_join", &["Shift+S"], "Slur joined or attacked"),
    verb("tie", &["T"], "Tie"),
    verb("staccato", &["."], "Staccato"),
    verb("tenuto", &["-"], "Tenuto"),
    verb("accent", &[">"], "Accent"),
    verb("marcato", &["^"], "Marcato"),
    verb("breath", &[","], "Breath"),
    verb("pedal", &["P"], "Pedal"),
    // the dynamic written on each selected note
    verb("dynamic_pp", &[], "Pianissimo"),
    verb("dynamic_p", &[], "Piano"),
    verb("dynamic_mp", &[], "Mezzo piano"),
    verb("dynamic_mf", &[], "Mezzo forte"),
    verb("dynamic_f", &[], "Forte"),
    verb("dynamic_ff", &[], "Fortissimo"),
    verb("dynamic_none", &[], "No dynamic"),
];

/// The code editor's scope, in force in every window of its application.
pub const EDITOR_SCOPE: &str = "editor";

/// **What the code editor adds**: its files, its session, its panels and
/// its size. Evaluating is the `code` element's own, and these name it for
/// the menu and for a panel the element is not in.
pub const EDITOR: &[Verb] = &[
    verb("close_tab", &["Ctrl+W"], "Close tab"),
    verb("quit", &["Ctrl+Q"], "Quit"),
    verb("preferences", &["Ctrl+,"], "Preferences..."),
    verb(
        "evaluate_line",
        &["Shift+Enter"],
        "Evaluate line or selection",
    ),
    verb("evaluate_block", &["Ctrl+Enter"], "Evaluate block"),
    toggle(
        "eval_paragraph",
        &[],
        "Evaluate contiguous lines as one block",
    ),
    verb("interrupt", &["Ctrl+."], "Interrupt"),
    verb("restart", &[], "Restart session"),
    verb("environments", &[], "Python environments..."),
    verb("clear_post", &["Ctrl+Shift+P"], "Clear post window"),
    toggle("show_docs", &[], "Documentation"),
    toggle("split_editor", &[], "Split editor"),
    verb("move_tab", &[], "Move tab to other editor"),
    choice("theme", "theme_light", &[], "Light"),
    choice("theme", "theme_dark", &[], "Dark"),
    choice("theme", "theme_system", &[], "System"),
    verb("zoom_in", &["Ctrl++", "Ctrl+="], "Increase size"),
    verb("zoom_out", &["Ctrl+-"], "Decrease size"),
    verb("zoom_reset", &["Ctrl+0"], "Default size"),
    verb("docs", &["F1"], "Documentation"),
    verb("search_docs", &["Ctrl+Shift+F"], "Search documentation"),
    verb("docs_folder", &[], "Documentation folder..."),
];

/// The score window's own scope, in force whenever it is open.
pub const SCORE_SCOPE: &str = "score";

/// Note entry's scope, read before the window's while the mode is on.
pub const NOTE_ENTRY_SCOPE: &str = "note_entry";

/// **The score window's own keys**: over the selection, and into note entry.
/// The chords are the settled ones of notation programs.
pub const SCORE: &[Verb] = &[
    STOP_AT_END,
    toggle("entry", &["N"], "Note entry on or off"),
    // over the selection: what note entry's own rows, read first while the
    // window is in it, give the cursor and the note written
    verb(
        "delete",
        &["Delete", "Backspace"],
        "Delete what is selected",
    ),
    verb("deselect", &["Escape"], "Select nothing"),
    verb("select_left", &["Left"], "Select previous item"),
    verb("select_right", &["Right"], "Select next item"),
    verb("step_up", &["Up"], "Step up"),
    verb("step_down", &["Down"], "Step down"),
    verb("octave_up", &["Ctrl+Up"], "Octave up"),
    verb("octave_down", &["Ctrl+Down"], "Octave down"),
];

/// **Note entry's keys**: the letters write, the digits choose the value.
pub const NOTE_ENTRY: &[Verb] = &[
    toggle("entry", &["N"], "Note entry on or off"),
    verb("entry_off", &["Escape"], "Leave note entry"),
    verb("pitch_a", &["A"], "Write A"),
    verb("pitch_b", &["B"], "Write B"),
    verb("pitch_c", &["C"], "Write C"),
    verb("pitch_d", &["D"], "Write D"),
    verb("pitch_e", &["E"], "Write E"),
    verb("pitch_f", &["F"], "Write F"),
    verb("pitch_g", &["G"], "Write G"),
    verb("chord_a", &["Shift+A"], "Add A to chord"),
    verb("chord_b", &["Shift+B"], "Add B to chord"),
    verb("chord_c", &["Shift+C"], "Add C to chord"),
    verb("chord_d", &["Shift+D"], "Add D to chord"),
    verb("chord_e", &["Shift+E"], "Add E to chord"),
    verb("chord_f", &["Shift+F"], "Add F to chord"),
    verb("chord_g", &["Shift+G"], "Add G to chord"),
    verb("cursor_left", &["Left"], "Cursor back"),
    verb("cursor_right", &["Right"], "Cursor forward"),
    verb("bar_left", &["Ctrl+Left"], "Cursor to previous bar"),
    verb("bar_right", &["Ctrl+Right"], "Cursor to next bar"),
    verb("staff_up", &["Alt+Up"], "Cursor to staff above"),
    verb("staff_down", &["Alt+Down"], "Cursor to staff below"),
    verb("step_up", &["Up"], "Step up"),
    verb("step_down", &["Down"], "Step down"),
    verb("octave_up", &["Ctrl+Up"], "Octave up"),
    verb("octave_down", &["Ctrl+Down"], "Octave down"),
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

/// The words a title leaves small where they are not its first or its last
/// (or the first after a comma or a colon): articles, conjunctions and the
/// short prepositions.
const SMALL: &[&str] = &[
    "a", "an", "the", "and", "but", "or", "nor", "as", "at", "by", "for", "in", "of", "on", "to",
    "via", "per", "vs", "into", "onto",
];

/// **A label as a menu writes it**: in header capitalization, as the
/// desktop's guidelines and the specification's editor write a menu -- every
/// word starting with a capital but the small ones (`SMALL`) inside the
/// label, each part of a hyphenated word too. A word with a capital past its
/// first letter (`MIDI`) is left as it is, and so is what follows a word
/// (`...`, a comma). A description or a help text stays in sentence case,
/// so this is for a menu's entries and nothing else.
#[must_use]
pub fn title_case(label: &str) -> String {
    let words: Vec<&str> = label.split(' ').collect();
    let last = words
        .iter()
        .rposition(|w| w.chars().any(char::is_alphanumeric))
        .unwrap_or(0);
    let mut fresh = true;
    let mut out = Vec::with_capacity(words.len());
    for (i, word) in words.iter().enumerate() {
        let bare: String = word
            .trim_end_matches(|c: char| !c.is_alphanumeric())
            .to_string();
        let small = !fresh && i != last && SMALL.contains(&bare.as_str());
        // a capital past the first letter is a name's own (`MIDI`, `MPE`)
        let named = word.chars().skip(1).any(char::is_uppercase);
        let titled = if small || named {
            (*word).to_string()
        } else {
            word.split('-')
                .map(|part| {
                    let mut chars = part.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first.to_uppercase().chain(chars).collect()
                    })
                })
                .collect::<Vec<_>>()
                .join("-")
        };
        // the word after a comma or a colon starts a phrase of its own
        fresh = word.ends_with([',', ':']);
        out.push(titled);
    }
    out.join(" ")
}

/// **A menu with every label in header capitalization** ([`title_case`]):
/// its entries, its submenus' and theirs, as a window's `menu` holds them.
#[must_use]
pub fn titled(menu: Value) -> Value {
    match menu {
        Value::Array(entries) => Value::Array(entries.into_iter().map(titled).collect()),
        Value::Object(mut o) => {
            if let Some(Value::String(label)) = o.get("label") {
                let label = title_case(label);
                o.insert("label".into(), Value::from(label));
            }
            if let Some(sub) = o.remove("menu") {
                o.insert("menu".into(), titled(sub));
            }
            Value::Object(o)
        }
        other => other,
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

    /// **A menu's label in header capitalization**: the words capitalized,
    /// the small ones inside it left small, the first after a comma a word
    /// of its own, the last always, a hyphenated word in each part, and a
    /// word that holds a capital as it is.
    #[test]
    fn a_menu_label_is_in_header_capitalization() {
        for (label, titled) in [
            ("Save as...", "Save As..."),
            ("Open file...", "Open File..."),
            ("Up an octave", "Up an Octave"),
            ("Go to start", "Go to Start"),
            ("Snap to grid, on or off", "Snap to Grid, On or Off"),
            ("Sixty-fourth note", "Sixty-Fourth Note"),
            ("Convert to MIDI 2.0", "Convert to MIDI 2.0"),
            ("Evaluate line or selection", "Evaluate Line or Selection"),
            ("Close", "Close"),
            ("Evaluate lines as a block", "Evaluate Lines as a Block"),
        ] {
            assert_eq!(title_case(label), titled, "{label}");
        }
        let menu = titled(json!([{"label": "File", "menu": [{"label": "Open recent",
            "menu": [{"label": "clear recent files"}]}, "-"]}]));
        assert_eq!(menu[0]["menu"][0]["label"], "Open Recent");
        assert_eq!(menu[0]["menu"][0]["menu"][0]["label"], "Clear Recent Files");
        assert_eq!(menu[0]["menu"][1], "-");
    }

    /// A scope names each verb once, with words of its own.
    #[test]
    fn every_scope_names_each_verb_once_with_its_words() {
        for rows in [WINDOW, AUDIO, MULTITRACK, NOTES, SCORE, NOTE_ENTRY, EDITOR] {
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
        for rows in [WINDOW, AUDIO, MULTITRACK, NOTES, SCORE, NOTE_ENTRY, EDITOR] {
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

    /// **The end switch is one verb**, in every scope whose transport can
    /// roll on past its contents, and never in the audio editor's.
    #[test]
    fn the_end_switch_is_one_verb_where_a_pass_can_roll_on() {
        for rows in [MULTITRACK, NOTES, SCORE] {
            assert_eq!(find(rows, "stop_at_end"), Some(STOP_AT_END));
        }
        assert!(find(AUDIO, "stop_at_end").is_none());
        assert!(find(WINDOW, "stop_at_end").is_none());
    }

    /// **A file is named alike in every application**: its verbs are the
    /// window's, and a scope restates one only to say that it asks more
    /// first -- the window's words with an ellipsis.
    #[test]
    fn every_application_names_its_file_in_the_window_s_words() {
        const FILE: &[&str] = &["new_file", "open_file", "save", "save_as", "close_file"];
        for name in FILE {
            assert!(find(WINDOW, name).is_some(), "{name} is the window's");
        }
        for rows in [AUDIO, MULTITRACK, NOTES, SCORE, NOTE_ENTRY, EDITOR] {
            for v in rows.iter().filter(|v| FILE.contains(&v.name)) {
                let window = find(WINDOW, v.name).unwrap();
                assert_eq!(v.label, format!("{}...", window.label), "{}", v.name);
                assert_eq!(v.keys, window.keys, "{}", v.name);
            }
        }
        assert_eq!(label(MULTITRACK, "open_file"), "Open...");
        assert_eq!(label(AUDIO, "new_file"), "New...");
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
