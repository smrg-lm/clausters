//! **The chrome of the audio, multitrack and notes editors**: the menu bar and
//! the toolbar, in one vocabulary.
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
//! | `to_start`, `to_end` | the position cursor to either end |
//! | `loop` | the loop switch |
//! | `close` | the window, asked first where something would be lost |
//! | `keys` | the window's keys |
//!
//! **What each adds**: the audio editor `save` and `mix` (a paste that adds);
//! the multitrack and the roll `split`, `join` and `quantize`; the multitrack
//! the rows' own commands (`add_track`, `reset_heights`, `compact_tracks`), a
//! transport that pauses (`pause`, `stop`), how its boxes of notes are drawn
//! (`notes_roll`, `notes_score`), and `save` where the window is the work's
//! only holder.
//!
//! What has a gesture and no meaning without one -- moving a box, trimming
//! it, drawing a curve -- is the hand's, and is in no menu. What is about one
//! thing under the pointer -- which of a clip's curves are shown -- is the
//! element's own context menu, not the bar's.
//!
//! The menus follow the order audio programs keep: File, Edit, View,
//! Transport, the application's own (Track), Help.

use serde_json::{Value, json};

/// Which application a chrome is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    /// The audio editor, over a take.
    Audio,
    /// The multitrack editor.
    Multitrack,
    /// The notes editor, over a roll.
    Notes,
}

/// **Back to the start**: a bar and a triangle pointing at it, drawn here
/// since the host's own symbols have none -- in a codepoint of the editors'
/// private plane, as the window's `glyphs` carry it.
pub const TO_START: &str = "F0100";
/// **On to the end**: the same, turned.
pub const TO_END: &str = "F0101";

/// The host's own symbols the tools use.
pub(crate) const PLAY: &str = "\u{25B6}";
pub(crate) const STOP: &str = "\u{25A0}";
pub(crate) const LOOP: &str = "\u{21BB}";

/// The size a tool's symbol is drawn at: twice the words beside it, as an
/// icon stands to a caption.
pub(crate) const SYMBOL_SIZE: f64 = 2.0;

/// **The outlines the tools are drawn with**, as a window's `glyphs`: a
/// thousand to the em, `y` upward, as the engraver's own come.
#[must_use]
pub fn glyphs() -> Value {
    json!({
        TO_START: "M0 0L80 0L80 520L0 520Z M560 0L560 520L150 260Z",
        TO_END: "M480 0L560 0L560 520L480 520Z M0 0L410 260L0 520Z",
    })
}

/// The character a codepoint in hex is.
pub(crate) fn glyph(code: &str) -> String {
    u32::from_str_radix(code, 16)
        .ok()
        .and_then(char::from_u32)
        .map(String::from)
        .unwrap_or_default()
}

fn entry(label: &str, verb: &str) -> Value {
    json!({"label": label, "verb": verb})
}

fn sub(label: &str, entries: Vec<Value>) -> Value {
    json!({"label": label, "menu": entries})
}

fn sep() -> Value {
    json!("-")
}

/// **The menu bar**, as the window's `menu` value. `saves` says whether the
/// window writes its work itself -- File has Save then.
#[must_use]
pub fn menu(app: App, saves: bool) -> Value {
    let mut file = Vec::new();
    if saves {
        file.extend([entry("Save", "save"), sep()]);
    }
    file.push(entry("Close", "close"));

    let mut edit = vec![
        entry("Undo", "undo"),
        entry("Redo", "redo"),
        sep(),
        entry("Cut", "cut"),
        entry("Copy", "copy"),
        entry("Paste", "paste"),
    ];
    if app == App::Audio {
        edit.push(entry("Paste mixed", "mix"));
    }
    edit.extend([
        entry("Delete", "delete"),
        sep(),
        entry("Select all", "select_all"),
    ]);
    if app != App::Audio {
        edit.extend([
            sep(),
            entry("Split", "split"),
            entry("Join", "join"),
            entry("Quantize", "quantize"),
        ]);
    }
    if app == App::Multitrack {
        // the automatic crossfade over an overlap: the multitrack's default,
        // which a client also sets as `Multitrack.crossfade`
        edit.extend([sep(), entry("Crossfade overlaps, on or off", "crossfade")]);
    }

    let mut view = vec![entry("Zoom to fit", "view_all")];
    if app == App::Multitrack {
        view.extend([
            sep(),
            entry("Reset track heights", "reset_heights"),
            entry("Compact tracks", "compact_tracks"),
        ]);
        // how a box of notes is drawn: the window's own, like its zoom
        view.extend([
            sep(),
            entry("Notes as rolls", "notes_roll"),
            entry("Notes as scores", "notes_score"),
        ]);
    }

    let mut transport = vec![entry("Play or stop", "play")];
    if app == App::Multitrack {
        transport.extend([entry("Pause", "pause"), entry("Stop", "stop")]);
    }
    transport.extend([
        sep(),
        entry("Go to start", "to_start"),
        entry("Go to end", "to_end"),
        sep(),
        entry("Loop", "loop"),
    ]);
    if app == App::Multitrack {
        // where a pass ends, when it is not looped
        transport.push(entry("Stop at end", "stop_at_end"));
    }

    let mut bar = vec![
        sub("File", file),
        sub("Edit", edit),
        sub("View", view),
        sub("Transport", transport),
    ];
    if app == App::Multitrack {
        bar.push(sub("Track", vec![entry("Add track", "add_track")]));
    }
    bar.push(sub("Help", vec![entry("Keyboard shortcuts", "keys")]));
    Value::Array(bar)
}

/// A tool: a flat button that performs `verb`, labelled with a symbol or a
/// word.
pub(crate) fn tool(verb: &str, label: &str, symbol: bool, tip: &str) -> Value {
    let mut node =
        json!({"type": "button", "flat": true, "verb": verb, "label": label, "tip": tip});
    if symbol && let Some(map) = node.as_object_mut() {
        map.insert("text_size".into(), json!(SYMBOL_SIZE));
    }
    node
}

/// **The transport's tools**, for the two windows whose transport is the
/// host's verbs: back to the start, play or stop, on to the end, the loop.
fn transport() -> Vec<Value> {
    vec![
        tool("to_start", &glyph(TO_START), true, "Go to start"),
        tool("play", PLAY, true, "Play or stop"),
        tool("to_end", &glyph(TO_END), true, "Go to end"),
        tool("loop", LOOP, true, "Loop"),
    ]
}

/// **The edit tools** an application has beside the menu's.
fn edits(app: App) -> Vec<Value> {
    match app {
        App::Audio => vec![
            tool("cut", "Cut", false, "Cut the selection"),
            tool("copy", "Copy", false, "Copy the selection"),
            tool("paste", "Paste", false, "Paste at the cursor"),
            tool("delete", "Delete", false, "Delete the selection"),
        ],
        App::Multitrack | App::Notes => vec![
            tool("split", "Split", false, "Split at the cursor"),
            tool("join", "Join", false, "Join what is selected"),
            tool("quantize", "Quantize", false, "Quantize to the grid"),
        ],
    }
}

/// **The toolbar**, as a row: `lead` first (a transport of the
/// application's own, numbered by the caller), else the host's transport;
/// then the edit tools.
#[must_use]
pub fn toolbar(app: App, lead: Option<Vec<Value>>) -> Value {
    let mut children = lead.unwrap_or_else(transport);
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

/// **Dresses `window`**: the menu bar, the toolbar on top of what it holds,
/// the outlines the tools are drawn with, and the view the window's commands
/// address (`main`).
pub fn dress(window: &mut Value, menu: Value, toolbar: Value, main: i32) {
    let Some(map) = window.as_object_mut() else {
        return;
    };
    map.insert("menu".into(), menu);
    map.insert("main".into(), json!(main));
    map.insert("glyphs".into(), glyphs());
    if let Some(Value::Array(children)) = map.get_mut("children") {
        children.insert(0, toolbar);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verbs(menu: &Value) -> Vec<String> {
        let mut out = Vec::new();
        for title in menu.as_array().into_iter().flatten() {
            for e in title["menu"].as_array().into_iter().flatten() {
                if let Some(v) = e["verb"].as_str() {
                    out.push(v.to_string());
                }
            }
        }
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
            titles(&menu(App::Audio, true)),
            ["File", "Edit", "View", "Transport", "Help"]
        );
        assert_eq!(
            titles(&menu(App::Multitrack, false)),
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
            "to_start",
            "to_end",
            "loop",
            "keys",
        ];
        for app in [App::Audio, App::Multitrack, App::Notes] {
            let said = verbs(&menu(app, false));
            for verb in common {
                assert!(said.iter().any(|v| v == verb), "{app:?} lacks {verb}");
            }
        }
        let audio = verbs(&menu(App::Audio, true));
        assert!(audio.contains(&"save".into()) && audio.contains(&"mix".into()));
        assert!(!audio.contains(&"split".into()));
        let notes = verbs(&menu(App::Notes, false));
        assert!(!notes.contains(&"save".into()) && notes.contains(&"quantize".into()));
        let multitrack = verbs(&menu(App::Multitrack, false));
        for verb in [
            "add_track",
            "pause",
            "stop",
            "reset_heights",
            "compact_tracks",
            "notes_roll",
            "notes_score",
        ] {
            assert!(
                multitrack.contains(&verb.into()),
                "the multitrack lacks {verb}"
            );
        }
    }

    /// A tool is a verb and nothing else: no id, since what it does is the
    /// host's to perform rather than the owner's to read.
    #[test]
    fn every_tool_names_a_verb_the_menu_has() {
        for app in [App::Audio, App::Multitrack, App::Notes] {
            let said = verbs(&menu(app, true));
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

    #[test]
    fn a_dressed_window_holds_the_bar_on_top_and_names_its_view() {
        let mut window = json!({"type": "window", "children": [{"type": "notes", "id": 4}]});
        dress(
            &mut window,
            menu(App::Notes, false),
            toolbar(App::Notes, None),
            4,
        );
        assert_eq!(window["main"], 4);
        assert_eq!(window["children"][0]["type"], "layout");
        assert_eq!(window["children"][1]["id"], 4);
        assert!(window["glyphs"][TO_START].is_string());
    }
}
