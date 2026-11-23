//! **The chrome of the audio, multitrack, notes and points editors**: the menu
//! bar and the toolbar, in one vocabulary.
//!
//! The three applications share most of what a hand asks of them, so they
//! share its words: a verb names the same act in every window -- `cut` is a
//! cut of samples, of boxes or of notes, whichever the window holds -- and an
//! entry or a tool names the verb and nothing else. The host performs a verb
//! it knows on what the window's focus is on (or the window's `main` view),
//! and hands the owner what it does not; so a menu entry, its key and its tool
//! are one command, and none of them is written twice.
//!
//! **The common vocabulary**, every one of them the host's or the history's:
//!
//! | Verb | What it does |
//! |---|---|
//! | `undo`, `redo` | a walk through the history |
//! | `cut`, `copy`, `paste`, `delete` | the clipboard over what is selected |
//! | `select_all` | everything the view holds, selected |
//! | `view_all` | the whole extent in view |
//! | `play` | play, or stop and go back to the position cursor |
//! | `pause` | the play tool: play, or pause where it stands |
//! | `stop` | stop, and go back to the position cursor |
//! | `to_start`, `to_end` | the position cursor to either end |
//! | `loop` | the loop switch |
//! | `snap` | the snap-to-grid switch (not the audio editor's) |
//! | `close` | the window, asked first where something would be lost |
//! | `keys` | the window's keys |
//!
//! **What each adds**: the audio editor `save`, `mix` (a paste that adds)
//! and its processes (`reverse`, the gain curve and the choice of the body
//! the hand edits), under a Process menu of its own;
//! the multitrack and the roll `split`, `join` and `quantize`; the
//! multitrack, the roll and the score where a pass ends (`stop_at_end`, the
//! transport's end switch); the multitrack
//! the rows' own commands (`add_track`, `reset_heights`, `compact_tracks`),
//! how its boxes of samples are
//! drawn (`view_wave`, `view_spectrogram`), and `save` where the window is the work's
//! only holder. The points editor adds nothing and has no tools: its bar holds
//! the window's own verbs and the history, and what a curve does not do is
//! refused with why.
//!
//! What has a gesture and no meaning without one -- moving a box, trimming
//! it, drawing a curve -- is the hand's, and is in no menu. What is about one
//! thing under the pointer -- which of a clip's curves are shown -- is the
//! element's own context menu, not the bar's.
//!
//! The menus follow the order audio programs keep: File, Edit, View,
//! Transport, the application's own (Process, Track), Help.

use serde_json::{Value, json};

use clausters_editing::verbs::{self, ActionState, ActionTarget, Verb};

/// Which application a chrome is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    /// The audio editor, over a take.
    Audio,
    /// The multitrack editor.
    Multitrack,
    /// The notes editor, over a roll.
    Notes,
    /// The points editor, over a curve: no transport, no grid and no
    /// clipboard, so its bar holds the window's own and the history.
    Points,
}

/// **The edit tools' icons**, by the name the host carries them under:
/// the roll's tools (the pointer, the pencil, the eraser, the scissors and
/// the stretch), which its pointer is drawn from too, the snap switch, and
/// the audio editor's clipboard.
const POINTER: &str = "mouse-pointer-2";
const PENCIL: &str = "pencil";
const ERASER: &str = "eraser";
const SCISSORS: &str = "scissors";
const STRETCH: &str = "unfold-horizontal";
const SNAP: &str = "magnet";
const CUT: &str = "scissors";
const COPY: &str = "copy";
const PASTE: &str = "clipboard-paste";
const DELETE: &str = "trash";

/// The host's own symbols the tools use.
pub(crate) const PLAY: &str = "\u{25B6}";

/// The verbs `app` adds to the window's: where its entries and tools find
/// their words.
fn scope(app: App) -> &'static [Verb] {
    match app {
        App::Multitrack => verbs::MULTITRACK,
        App::Audio => verbs::AUDIO,
        App::Notes => verbs::NOTES,
        App::Points => &[],
    }
}

/// A menu entry: the verb, in the words the verb table gives it.
fn entry(app: App, verb: &str) -> Value {
    word(scope(app), verb)
}

fn sub(label: &str, entries: Vec<Value>) -> Value {
    json!({"label": label, "menu": entries})
}

fn sep() -> Value {
    json!("-")
}

/// **What a window's File menu holds.** Every application's File menu is
/// built here, the score's and the code editor's with the others', so its
/// section on the window's file is the same section everywhere, in one
/// order and in the verb table's words:
///
/// | | |
/// |---|---|
/// | the file | New, Open..., Open Recent (where the application keeps its recent files), Close File |
/// | its writing | Save, Save As... |
/// | the application's own | each group between separators: a page's setup, an export, a conversion |
/// | the window | Close, or the program's Quit |
///
/// What a window does not have is left out rather than disabled: a curve's
/// window has no file, and a roll a client holds writes none.
pub struct File<'a> {
    /// The verbs whose words the entries take: the application's scope,
    /// and the window's under it.
    pub scope: &'a [Verb],
    /// Whether the window has a file of its own: New, Open, Close File.
    pub files: bool,
    /// Whether it writes its work itself: Save and Save As.
    pub saves: bool,
    /// The recent files, as the entries of Open Recent, where the
    /// application keeps them.
    pub recent: Option<Vec<Value>>,
    /// The verb that lets go of the file in view: `close_file`, or a tab's.
    pub close_file: &'a str,
    /// The application's own entries, each group between separators.
    pub own: Vec<Vec<Value>>,
    /// What ends the menu: the window's `close`, by default.
    pub last: Vec<Value>,
}

impl<'a> File<'a> {
    /// A File menu with nothing but the window's close, in `scope`'s words.
    #[must_use]
    pub fn new(scope: &'a [Verb]) -> Self {
        File {
            scope,
            files: false,
            saves: false,
            recent: None,
            close_file: "close_file",
            own: Vec::new(),
            last: vec![word(scope, "close")],
        }
    }
}

/// An entry of `scope`'s: the verb, in the words the table gives it there.
#[must_use]
pub fn word(scope: &[Verb], verb: &str) -> Value {
    json!({"label": verbs::label(scope, verb), "verb": verb})
}

/// **The File menu** ([`File`]), as an entry of the bar.
#[must_use]
pub fn file_menu(file: &File<'_>) -> Value {
    let word = |verb: &str| word(file.scope, verb);
    let mut groups: Vec<Vec<Value>> = Vec::new();
    if file.files {
        let mut group = vec![word("new_file"), word("open_file")];
        if let Some(recent) = &file.recent {
            group.push(sub("Open recent", recent.clone()));
        }
        group.push(word(file.close_file));
        groups.push(group);
    }
    if file.saves {
        groups.push(vec![word("save"), word("save_as")]);
    }
    groups.extend(file.own.iter().cloned());
    groups.push(file.last.clone());
    let mut entries = Vec::new();
    for group in groups.into_iter().filter(|g| !g.is_empty()) {
        if !entries.is_empty() {
            entries.push(sep());
        }
        entries.extend(group);
    }
    sub("File", entries)
}

/// **The menu bar**, as the window's `menu` value, its labels in header
/// capitalization (`verbs::titled`). `files` says whether the window has a
/// document of its own -- File has New, Open... and Close File then, whoever
/// holds it: only a view of another window's document has none -- and
/// `saves` whether it writes its work itself -- File has Save then.
#[must_use]
pub fn menu(app: App, files: bool, saves: bool) -> Value {
    verbs::titled(entries(app, files, saves))
}

/// The menu bar's entries, in the verb table's words.
fn entries(app: App, files: bool, saves: bool) -> Value {
    // a curve is drawn and walked through, and nothing else: what the bar
    // would offer of the others' is refused where it is asked (`actions`)
    if app == App::Points {
        return json!([
            file_menu(&File::new(scope(app))),
            sub("Edit", vec![entry(app, "undo"), entry(app, "redo")]),
            sub("Help", vec![entry(app, "keys")]),
        ]);
    }
    // a conversion opens a copy in another format in a roll of its own
    let own = if app == App::Notes {
        vec![
            [
                "convert_notation",
                "convert_midi1",
                "convert_mpe",
                "convert_midi2",
            ]
            .map(|v| entry(app, v))
            .to_vec(),
        ]
    } else {
        Vec::new()
    };
    let file = file_menu(&File {
        files,
        saves,
        own,
        ..File::new(scope(app))
    });

    let mut edit = vec![
        entry(app, "undo"),
        entry(app, "redo"),
        sep(),
        entry(app, "cut"),
        entry(app, "copy"),
        entry(app, "paste"),
    ];
    if app == App::Audio {
        edit.push(entry(app, "mix"));
    }
    edit.extend([entry(app, "delete"), sep(), entry(app, "select_all")]);
    if app != App::Audio {
        edit.extend([sep(), entry(app, "split"), entry(app, "join")]);
        if app == App::Notes {
            edit.push(entry(app, "unjoin"));
        }
        edit.push(entry(app, "quantize"));
    }
    if app == App::Notes {
        // the selection moved, and the selection by what it holds
        edit.push(sep());
        edit.extend(["step_up", "step_down", "octave_up", "octave_down"].map(|v| entry(app, v)));
        edit.push(sep());
        edit.extend(
            [
                "select_pitch",
                "select_channel",
                "select_voice",
                "select_staff",
            ]
            .map(|v| entry(app, v)),
        );
        // what a plain press does, as the toolbar's tools say it
        edit.push(sub(
            "Tool",
            clausters_editing::notes::TOOLS
                .iter()
                .map(|(v, _)| entry(app, v))
                .collect(),
        ));
        // the channel the MIDI mode edits, as the notation mode edits a voice
        edit.push(sub(
            "Channel",
            clausters_editing::notes::CHANNEL_VERBS
                .iter()
                .map(|v| entry(app, v))
                .collect(),
        ));
    }
    if app == App::Notes {
        // the curves the sequence's format allows, a channel's and a note's
        edit.push(sep());
        edit.push(sub(
            "Curves",
            [
                "curve_cc1",
                "curve_cc11",
                "curve_bend",
                "curve_pressure",
                "note_bend",
                "note_pressure",
                "note_timbre",
                "curve_remove",
            ]
            .map(|v| entry(app, v))
            .to_vec(),
        ));
        // how a gesture edits a line: the window's, like the snap switch
        edit.extend([sep(), entry(app, "monophonic")]);
    }
    if app == App::Multitrack {
        // the automatic crossfade over an overlap: the multitrack's default,
        // which a client also sets as `Multitrack.crossfade`
        edit.extend([sep(), entry(app, "crossfade")]);
    }

    let mut view = vec![entry(app, "view_all")];
    if app != App::Points {
        // what the window's time ruler counts in
        view.push(sub(
            "Ruler",
            crate::ruler::VERBS
                .iter()
                .map(|(v, _)| entry(app, v))
                .collect(),
        ));
    }
    if app == App::Audio {
        view.extend([
            sep(),
            entry(app, "view_wave"),
            entry(app, "view_spectrogram"),
            entry(app, "view_both"),
            entry(app, "toggle_files"),
            entry(app, "spectrogram_settings"),
        ]);
    }
    if app != App::Audio {
        // the snap-to-grid switch: a take has no grid
        view.push(entry(app, "snap"));
    }
    if app == App::Notes {
        view.push(entry(app, "events_panel"));
        view.push(entry(app, "display_settings"));
    }
    if app == App::Multitrack {
        view.extend([
            sep(),
            entry(app, "reset_heights"),
            entry(app, "compact_tracks"),
        ]);
        // how a box of samples is drawn, the audio editor's two pictures of
        // a take: the window's own, like its zoom
        view.extend([
            sep(),
            entry(app, "view_wave"),
            entry(app, "view_spectrogram"),
        ]);
    }

    // **the one transport** ([`crate::transport`])
    let transport = crate::transport::entries(scope(app));

    let mut bar = vec![
        file,
        sub("Edit", edit),
        sub("View", view),
        sub("Transport", transport),
    ];
    if app == App::Audio {
        // what renders into new samples: the selection reversed, and the
        // gain curve, with the choice of the body the hand edits beside the
        // curve it chooses
        bar.push(sub(
            "Process",
            vec![
                entry(app, "reverse"),
                sep(),
                entry(app, "layer_take"),
                entry(app, "show_envelope"),
                entry(app, "apply_envelope"),
            ],
        ));
    }
    if app == App::Multitrack {
        bar.push(sub("Track", vec![entry(app, "add_track")]));
    }
    if app == App::Notes {
        // what the notation mode shows, edits and writes: the mode is the
        // sequence's format's, and a conversion is the File menu's
        let mut notation = vec![
            entry(app, "show_system"),
            entry(app, "show_part"),
            entry(app, "show_staff"),
            entry(app, "staff_up"),
            entry(app, "staff_down"),
            sep(),
        ];
        notation.extend(["voice_1", "voice_2", "voice_3", "voice_4"].map(|v| entry(app, v)));
        notation.push(sep());
        notation.extend(["color_voice", "color_staff", "color_part"].map(|v| entry(app, v)));
        notation.extend([
            sep(),
            entry(app, "write_voices"),
            entry(app, "write_ties"),
            sep(),
            entry(app, "slur"),
            entry(app, "slur_join"),
            entry(app, "tie"),
            entry(app, "staccato"),
            entry(app, "tenuto"),
            entry(app, "accent"),
            entry(app, "marcato"),
            entry(app, "breath"),
            entry(app, "pedal"),
            sub(
                "Dynamic",
                [
                    "dynamic_pp",
                    "dynamic_p",
                    "dynamic_mp",
                    "dynamic_mf",
                    "dynamic_f",
                    "dynamic_ff",
                    "dynamic_none",
                ]
                .map(|v| entry(app, v))
                .to_vec(),
            ),
        ]);
        bar.push(sub("Notation", notation));
    }
    bar.push(sub("Help", vec![entry(app, "keys")]));
    Value::Array(bar)
}

/// **The actions of a window over a grid** -- the multitrack's and the
/// roll's -- as they read now: every verb of the window's and of `app`'s,
/// a paste that adds (`mix`) aside, available but for a walk through a history
/// that has nothing to walk over, the file verbs of a window with no document
/// of its own (`files`) and a `save` that writes nowhere (`saves`);
/// the snap switch, and whatever `checked` reads of the application's own
/// switches and choices. A switch the host keeps (`loop`) is read where it
/// is kept.
pub(crate) fn grid_actions(
    app: App,
    files: bool,
    saves: bool,
    undo: bool,
    redo: bool,
    checked: &dyn Fn(&str) -> Option<bool>,
) -> std::collections::BTreeMap<&'static str, ActionState> {
    verbs::WINDOW
        .iter()
        .chain(scope(app))
        .filter(|verb| verb.name != "mix")
        .map(|verb| {
            let mut state = ActionState::new(verb, ActionTarget::Window);
            state.checked = checked(verb.name);
            match verb.name {
                "undo" if !undo => state.refuse("there is nothing to undo"),
                "redo" if !redo => state.refuse("there is nothing to redo"),
                // a curve's, and a roll's over another window's sequence:
                // never a file of theirs
                "new_file" | "open_file" | "save" | "save_as" | "close_file" if !files => {
                    state.refuse_absent("this window has no file of its own");
                }
                "save" | "save_as" if !saves => state.refuse("this window writes no file"),
                // a curve's window has an axis and no time ruler
                _ if app == App::Points && crate::ruler::Unit::of_verb(verb.name).is_some() => {
                    state.refuse_absent("this window has no time ruler");
                }
                _ => {}
            }
            (verb.name, state)
        })
        .collect()
}

/// **A tool drawn as an icon the host carries** (`icon`, by its name), its
/// verb's words its tip: the edit tools, square and as large as the
/// interface's icons.
pub(crate) fn icon_tool(app: App, verb: &str, icon: &str) -> Value {
    let words = verbs::label(scope(app), verb);
    json!({"type": "button", "flat": true, "verb": verb, "icon": icon, "tip": words})
}

/// **The transport's tools**, every application's ([`crate::transport`]):
/// back to the start, play or pause, stop, on to the end, the loop.
fn transport(app: App) -> Vec<Value> {
    crate::transport::tools(scope(app))
}

/// **The edit tools** an application has beside the menu's: the audio
/// editor's clipboard, and where there is a grid the switch a hand's edits
/// snap to it by. What has a key and a menu entry and no state to show
/// (`split`, `join`, `quantize`) stays there.
fn edits(app: App) -> Vec<Value> {
    match app {
        App::Audio => [
            ("cut", CUT),
            ("copy", COPY),
            ("paste", PASTE),
            ("delete", DELETE),
        ]
        .iter()
        .map(|(verb, icon)| icon_tool(app, verb, icon))
        .collect(),
        App::Multitrack => vec![icon_tool(app, "snap", SNAP)],
        // the roll's tools, then its snap switch
        App::Notes => [
            ("tool_select", POINTER),
            ("tool_draw", PENCIL),
            ("tool_erase", ERASER),
            ("tool_split", SCISSORS),
            ("tool_stretch", STRETCH),
            ("snap", SNAP),
        ]
        .iter()
        .map(|(verb, icon)| icon_tool(app, verb, icon))
        .collect(),
        App::Points => Vec::new(),
    }
}

/// **The toolbar**, as a row: `lead` first (a transport of the
/// application's own, numbered by the caller), else the host's transport;
/// then the edit tools.
#[must_use]
pub fn toolbar(app: App, lead: Option<Vec<Value>>) -> Value {
    let mut children = lead.unwrap_or_else(|| transport(app));
    if !children.is_empty() {
        children.push(json!({"type": "separator"}));
    }
    children.extend(edits(app));
    json!({
        "type": "layout",
        "flow": "row",
        "hug": true,
        "pack": true,
        "children": children,
    })
}

/// **Dresses `window`**: the menu bar, the toolbar on top of what it holds
/// (none for `null`), the outlines the tools are drawn with, and the view
/// the window's commands address (`main`).
/// **The channel entries of an audio window's View menu**, for a take of
/// `channels`: show them all, and a submenu each to fold one (or unfold it) and
/// to show one alone. The verbs carry the channel after a colon
/// (`fold_channel:2`), since an entry names a verb and nothing else.
pub fn channel_entries(menu: &mut Value, channels: u32) {
    if channels < 2 {
        return;
    }
    let Some(view) = menu
        .as_array_mut()
        .and_then(|titles| titles.iter_mut().find(|t| t["label"] == "View"))
        .and_then(|t| t.get_mut("menu"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let each = |verb: &str| -> Vec<Value> {
        (0..channels)
            .map(|c| json!({"label": format!("Channel {}", c + 1), "verb": format!("{verb}:{c}")}))
            .collect()
    };
    view.extend([
        sep(),
        entry(App::Audio, "show_all_channels"),
        sub("Fold channel", each("fold_channel")),
        sub("Solo channel", each("solo_channel")),
    ]);
}

impl App {
    /// **The page an application's window opens on in a dock**, named by
    /// the application's short name: a window with the application's chrome
    /// is the application, alone on a page of its own; one with none is a
    /// view, a panel among others.
    pub fn page(self) -> &'static str {
        match self {
            App::Audio => "Audio",
            App::Multitrack => "Tracks",
            App::Notes => "Roll",
            App::Points => "Points",
        }
    }

    /// **The key the host keeps the application's window size under**
    /// (`remember`): the application's kind, apart from the name its page
    /// shows, so renaming a page never loses the sizes the host remembers.
    pub fn remember(self) -> &'static str {
        match self {
            App::Audio => "Audio",
            App::Multitrack => "Multitrack",
            App::Notes => "Notes",
            App::Points => "Points",
        }
    }
}

/// **A window dressed as `app`**: its menu bar, its toolbar on top, the
/// view the menu acts on (`main`), and the page of its
/// own it opens on in a dock ([`App::page`]), and the key the host keeps
/// the window's size under ([`App::remember`]), so an editor opens at the
/// size it was last left at.
pub fn dress(window: &mut Value, app: App, menu: Value, toolbar: Value, main: i32) {
    let Some(map) = window.as_object_mut() else {
        return;
    };
    map.insert("dock_page".into(), json!(app.page()));
    map.insert("remember".into(), json!(app.remember()));
    map.insert("menu".into(), menu);
    map.insert("main".into(), json!(main));
    // a window with no tools (a curve's) has no row for them
    if !toolbar.is_null()
        && let Some(Value::Array(children)) = map.get_mut("children")
    {
        children.insert(0, toolbar);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Every verb a menu names, its submenus' with them.
    fn verbs(menu: &Value) -> Vec<String> {
        fn walk(entries: &Value, out: &mut Vec<String>) {
            for e in entries.as_array().into_iter().flatten() {
                if let Some(v) = e["verb"].as_str() {
                    out.push(v.to_string());
                }
                walk(&e["menu"], out);
            }
        }
        let mut out = Vec::new();
        walk(menu, &mut out);
        out
    }

    fn titles(menu: &Value) -> Vec<&str> {
        menu.as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t["label"].as_str())
            .collect()
    }

    /// The bars keep the order audio programs keep, and the application's
    /// own menu stands before Help.
    #[test]
    fn the_menus_keep_the_field_s_order() {
        assert_eq!(
            titles(&menu(App::Audio, true, true)),
            ["File", "Edit", "View", "Transport", "Process", "Help"]
        );
        assert_eq!(
            titles(&menu(App::Multitrack, true, false)),
            ["File", "Edit", "View", "Transport", "Track", "Help"]
        );
    }

    /// The vocabulary is one: what the three share is spelled alike, and each
    /// adds only its own words.
    #[test]
    fn the_three_share_their_words() {
        let common = [
            "close",
            "undo",
            "redo",
            "cut",
            "copy",
            "paste",
            "delete",
            "select_all",
            "view_all",
            "play",
            "pause",
            "stop",
            "to_start",
            "to_end",
            "loop",
            "keys",
        ];
        for app in [App::Audio, App::Multitrack, App::Notes] {
            let said = verbs(&menu(app, true, false));
            for verb in common {
                assert!(said.iter().any(|v| v == verb), "{app:?} lacks {verb}");
            }
        }
        let audio = verbs(&menu(App::Audio, true, true));
        assert!(audio.contains(&"save".into()) && audio.contains(&"mix".into()));
        // the processes, in a menu of the audio editor's own
        assert!(audio.contains(&"reverse".into()));
        let bar = menu(App::Audio, true, true);
        let process = bar
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["label"] == "Process")
            .expect("a Process menu");
        let processes: Vec<&str> = process["menu"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["verb"].as_str())
            .collect();
        assert_eq!(
            processes,
            ["reverse", "layer_take", "show_envelope", "apply_envelope"]
        );
        let edit = bar
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["label"] == "Edit")
            .unwrap();
        assert!(
            !edit.to_string().contains("envelope"),
            "the gain curve is a process"
        );
        assert!(!audio.contains(&"split".into()));
        let notes = verbs(&menu(App::Notes, true, false));
        assert!(!notes.contains(&"save".into()) && notes.contains(&"quantize".into()));
        let multitrack = verbs(&menu(App::Multitrack, true, false));
        for verb in [
            "add_track",
            "reset_heights",
            "compact_tracks",
            "view_wave",
            "view_spectrogram",
        ] {
            assert!(
                multitrack.contains(&verb.into()),
                "the multitrack lacks {verb}"
            );
        }
    }

    /// **The File menu of a window**, as its labels read: what every
    /// application's begins with, written once for the tests of each.
    pub(crate) fn file_labels(bar: &Value) -> Vec<String> {
        let file = bar
            .as_array()
            .and_then(|titles| titles.iter().find(|t| t["label"] == "File"))
            .expect("a File menu");
        file["menu"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["label"].as_str().or(e.as_str()).unwrap_or("").to_string())
            .collect()
    }

    /// **What every application's File menu begins with**, the score's and
    /// the code editor's too, in this order and these words.
    pub(crate) const FILE_SECTION: [&str; 6] =
        ["New", "Open...", "Close File", "-", "Save", "Save As..."];

    /// **A file is handled alike everywhere**: the same entries, in the same
    /// order and words, and the window's close at the end.
    #[test]
    fn every_application_s_file_menu_starts_alike() {
        for (app, file) in [
            (App::Multitrack, FILE_SECTION.to_vec()),
            (App::Notes, FILE_SECTION.to_vec()),
        ] {
            let labels = file_labels(&menu(app, true, true));
            assert_eq!(labels[..file.len()], file, "{app:?}");
            assert_eq!(labels.last().map(String::as_str), Some("Close"), "{app:?}");
        }
        // a new take asks its shape first, and says so
        let audio = file_labels(&menu(App::Audio, true, true));
        assert_eq!(audio[0], "New...");
        assert_eq!(audio[1..FILE_SECTION.len()], FILE_SECTION[1..]);
        // a window that does not write its work -- a client holds it -- has
        // the file and no save, the roll as the multitrack
        let file = ["New", "Open...", "Close File", "-"];
        let multitrack = file_labels(&menu(App::Multitrack, true, false));
        assert_eq!(multitrack, [&file[..], &["Close"]].concat());
        let roll = file_labels(&menu(App::Notes, true, false));
        assert_eq!(roll[..file.len()], file);
        assert_eq!(roll[file.len()], "Convert to Notation...");
        assert_eq!(roll.last().map(String::as_str), Some("Close"));
        // a view of another window's document has none: what the
        // application adds and the close
        let view = file_labels(&menu(App::Notes, false, false));
        assert_eq!(view[0], "Convert to Notation...");
        assert_eq!(view.last().map(String::as_str), Some("Close"));
        assert_eq!(file_labels(&menu(App::Points, false, false)), ["Close"]);
    }

    /// A tool is a verb and nothing else: no id, since what it does is the
    /// host's to perform rather than the owner's to read.
    #[test]
    fn every_tool_names_a_verb_the_menu_has() {
        for app in [App::Audio, App::Multitrack, App::Notes] {
            let said = verbs(&menu(app, true, true));
            let bar = toolbar(app, None);
            for tool in bar["children"].as_array().into_iter().flatten() {
                if tool["type"] == "button" {
                    let verb = tool["verb"].as_str().expect("a verb");
                    assert!(said.iter().any(|v| v == verb), "{verb} is in no menu");
                    assert!(tool.get("id").is_none());
                }
            }
        }
    }

    /// **A page shows the application's short name, and the size is kept
    /// under its kind**: the key the host remembered a window's size under
    /// before the pages were renamed is the one it still reads.
    #[test]
    fn a_page_is_the_short_name_and_the_size_key_the_kind() {
        let pages = [App::Audio, App::Multitrack, App::Notes].map(App::page);
        assert_eq!(pages, ["Audio", "Tracks", "Roll"]);
        let kept = [App::Audio, App::Multitrack, App::Notes, App::Points].map(App::remember);
        assert_eq!(kept, ["Audio", "Multitrack", "Notes", "Points"]);
    }

    #[test]
    fn a_dressed_window_holds_the_bar_on_top_and_names_its_view() {
        let mut window = json!({"type": "window", "children": [{"type": "notes", "id": 4}]});
        dress(
            &mut window,
            App::Notes,
            menu(App::Notes, true, false),
            toolbar(App::Notes, None),
            4,
        );
        assert_eq!(window["main"], 4);
        assert_eq!(window["dock_page"], "Roll");
        assert_eq!(window["remember"], "Notes");
        assert_eq!(window["children"][0]["type"], "layout");
        assert_eq!(window["children"][1]["id"], 4);
    }
}
