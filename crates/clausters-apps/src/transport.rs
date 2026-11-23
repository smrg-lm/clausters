//! **The transport every application has**, written once: its tools, its
//! menu, what its buttons ask of a playback and how its play tool reads.
//!
//! | Verb | What it does |
//! |---|---|
//! | `play` | the space bar: play, or stop and go back to the position cursor |
//! | `pause` | the play tool: play, or pause where it stands, a play after it going on from there |
//! | `stop` | stop, and go back to the position cursor |
//! | `to_start`, `to_end` | the position cursor to either end |
//! | `loop` | the loop switch, the window's own |
//! | `stop_at_end` | the end switch: a pass stops where the contents end, or rolls on |
//!
//! The play tool is the `pause` verb, and **it shows what a press does**:
//! the play icon while the transport stands, the pause icon while it rolls
//! ([`show`], the reading's `icon`). Every application's playback carries
//! the two buttons out the same way ([`Asked`]): the multitrack's, the
//! roll's and the score's (one playback of notes) and the audio editor's.
//!
//! **The end switch** is the transport's where a pass can roll on past the
//! contents (the multitrack's, the roll's and the score's, whose scopes
//! name it): beside the loop switch, its reading the playback's end
//! ([`show_end`]). A take always ends with its last frame, so the audio
//! editor has none.

use serde_json::{Value, json};

use clausters_editing::verbs::{self, Verb};

/// **What a transport button asks of the playback.**
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Play, or pause where it stands -- and a play after a pause goes on
    /// from there.
    Toggle,
    /// Stop, and go back to the position cursor.
    Stop,
    /// The end switch: a pass that is not looped stops where the contents
    /// end, or rolls on past them.
    StopAtEnd,
}

/// The transport button a window verb names, if it names one.
#[must_use]
pub fn asked(verb: &str) -> Option<Asked> {
    match verb {
        PAUSE => Some(Asked::Toggle),
        STOP => Some(Asked::Stop),
        STOP_AT_END => Some(Asked::StopAtEnd),
        _ => None,
    }
}

/// The play tool's verb: play, or pause.
pub const PAUSE: &str = "pause";
/// The stop tool's verb.
pub const STOP: &str = "stop";
/// The end switch's verb.
pub const STOP_AT_END: &str = "stop_at_end";

/// The icons the tools are drawn with, by the name the host carries them
/// under.
const TO_START: &str = "skip-back";
const PLAY: &str = "play";
const PAUSED: &str = "pause";
const HALT: &str = "square";
const TO_END: &str = "skip-forward";
const LOOP: &str = "repeat";
const END: &str = "arrow-right-to-line";

/// Whether `scope` has the end switch.
fn ends(scope: &[Verb]) -> bool {
    verbs::find(scope, STOP_AT_END).is_some()
}

/// **The transport's tools**, in the order every application shows them:
/// back to the start, play or pause, stop, on to the end, the loop switch,
/// and the end switch where `scope` has it. Their tips are the verbs' words
/// in `scope` (or the window's).
#[must_use]
pub fn tools(scope: &[Verb]) -> Vec<Value> {
    let end = ends(scope).then_some((STOP_AT_END, END));
    [
        ("to_start", TO_START),
        (PAUSE, PLAY),
        (STOP, HALT),
        ("to_end", TO_END),
        ("loop", LOOP),
    ]
    .into_iter()
    .chain(end)
    .map(|(verb, icon)| {
        json!({"type": "button", "flat": true, "verb": verb, "icon": icon,
            "tip": verbs::label(scope, verb)})
    })
    .collect()
}

/// **The Transport menu's entries**, every application's: play, pause and
/// stop, the cursor to either end, the loop switch and, where `scope` has
/// it, the end switch -- in the words of `scope` (or the window's).
#[must_use]
pub fn entries(scope: &[Verb]) -> Vec<Value> {
    let word = |verb: &str| crate::chrome::word(scope, verb);
    let mut entries = vec![
        word("play"),
        word(PAUSE),
        word(STOP),
        json!("-"),
        word("to_start"),
        word("to_end"),
        json!("-"),
        word("loop"),
    ];
    if ends(scope) {
        entries.push(word(STOP_AT_END));
    }
    entries
}

/// **The play tool shows what a press does**: the pause icon on the
/// window's `pause` reading while its transport rolls, the play icon while
/// it stands. `states` is a window's action readings, as an object.
pub fn show(states: &mut Value, rolling: bool) {
    if let Some(state) = states.get_mut(PAUSE).filter(|s| s.is_object()) {
        state["icon"] = json!(if rolling { PAUSED } else { PLAY });
    }
}

/// **The end switch reads where a pass ends**: on while a pass that is
/// not looped stops where the contents end, off while it rolls on. `ends`
/// is the playback's, `None` where there is none yet; a window with no
/// `stop_at_end` reading is left alone.
pub fn show_end(states: &mut Value, ends: Option<bool>) {
    if let Some(state) = states.get_mut(STOP_AT_END).filter(|s| s.is_object()) {
        state["checked"] = json!(ends.unwrap_or(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The play tool reads pause while the transport rolls and play while
    /// it stands; a window with no `pause` reading is left alone.
    #[test]
    fn the_play_tool_shows_what_a_press_does() {
        let mut states = json!({"pause": {"kind": "command"}, "play": {}});
        show(&mut states, true);
        assert_eq!(states["pause"]["icon"], "pause");
        show(&mut states, false);
        assert_eq!(states["pause"]["icon"], "play");
        assert!(states["play"].get("icon").is_none());
        let mut none = json!({});
        show(&mut none, true);
        assert_eq!(none, json!({}));
    }

    /// The two buttons are named by their verbs, and every tool has one.
    #[test]
    fn the_buttons_are_their_verbs() {
        assert_eq!(asked("pause"), Some(Asked::Toggle));
        assert_eq!(asked("stop"), Some(Asked::Stop));
        assert_eq!(asked("stop_at_end"), Some(Asked::StopAtEnd));
        assert_eq!(asked("play"), None);
        let tools = tools(&[]);
        let named: Vec<&str> = tools.iter().filter_map(|t| t["verb"].as_str()).collect();
        assert_eq!(named, ["to_start", "pause", "stop", "to_end", "loop"]);
        assert_eq!(tools[1]["tip"], "Play or pause");
    }

    /// **The end switch is the transport's where a pass can roll on**: a
    /// tool beside the loop and an entry after it, in the multitrack, the
    /// roll and the score, and nowhere in the audio editor; its reading is
    /// the playback's end.
    #[test]
    fn the_end_switch_stands_beside_the_loop_where_a_pass_can_roll_on() {
        let verbs_of = |values: Vec<Value>| -> Vec<String> {
            values
                .iter()
                .filter_map(|v| v["verb"].as_str().map(str::to_string))
                .collect()
        };
        for scope in [verbs::MULTITRACK, verbs::NOTES, verbs::SCORE] {
            let tools = verbs_of(tools(scope));
            assert_eq!(tools[4..], ["loop", "stop_at_end"]);
            let entries = verbs_of(entries(scope));
            assert_eq!(entries.last().map(String::as_str), Some("stop_at_end"));
        }
        assert!(!verbs_of(tools(verbs::AUDIO)).contains(&"stop_at_end".into()));
        assert!(!verbs_of(entries(verbs::AUDIO)).contains(&"stop_at_end".into()));
        let mut states = json!({"stop_at_end": {"kind": "toggle"}});
        show_end(&mut states, Some(true));
        assert_eq!(states["stop_at_end"]["checked"], true);
        show_end(&mut states, None);
        assert_eq!(states["stop_at_end"]["checked"], false);
        let mut none = json!({});
        show_end(&mut none, Some(true));
        assert_eq!(none, json!({}));
    }
}
