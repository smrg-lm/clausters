//! **The score editor's toolbar**: the values a hand reaches for while it
//! writes, one press away.
//!
//! A row of light controls over the page: the **value** a note is written
//! with, its dot, whether it is a rest, its accidental; the common
//! articulations, a tie and a triplet for what is selected; the voice; and how
//! the window looks at the score; and, at the far edge, the transport. Each is a widget of the host's -- a
//! `choice` drawn as segments, a `toggle` drawn as a button, a flat `button`
//! -- named here ([`TOOLS`]) and numbered by the caller, as every widget id
//! is: a window composed with no ids for them has no toolbar.
//!
//! A tool that holds state reports its **value** -- a choice its index, a
//! toggle 0 or 1 -- and one that acts reports a `"click"`; [`read`] says what
//! either asks of the editor. What a tool shows is the editor's state
//! ([`State`]), so the row is corrected whenever that moves.
//!
//! Three of them are the **input state**, what the next note written on the
//! page takes: its value, its dot, whether it is a rest. The accidental is
//! both: with notes selected it is theirs, and with none it is armed for the
//! next note and let go once that note is written. The voice is the
//! selection's.
//!
//! **A tool is drawn with the engraver's own symbol** where it has one: its
//! label is the SMuFL character ([`codes`]), and the window carries the
//! outline of each ([`Outlines`], asked of the engraver once), so the host
//! draws the shape the page would. Where the engraver handed none out -- a
//! font without that glyph, an engraver that draws nothing -- the label is the
//! text it always was.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use clausters_core::notation::{View, glyph_char};
use clausters_core::ratio::Ratio;

use super::icons;
use super::menu::VALUES;

/// The tools, by the names the caller numbers them under, left to right.
pub const TOOLS: &[&str] = &[
    "entry",
    "value",
    "dot",
    "rest",
    "accidental",
    "stacc",
    "acc",
    "ten",
    "marc",
    "tie",
    "tuplet",
    "voice",
    "layout",
    "rewind",
    "play",
    "loop",
];

/// The caller's widget id for each tool it numbered.
pub type Ids = BTreeMap<String, i32>;

/// The engraver's outlines, by SMuFL codepoint: what
/// [`Score::outlines`](clausters_core::notation::Score::outlines) answers.
pub type Outlines = BTreeMap<String, String>;

/// The written values' symbols, in [`VALUES`]' order: a note of each, alone.
const VALUE_CODES: [&str; 7] = ["E1D2", "E1D3", "E1D5", "E1D7", "E1D9", "E1DB", "E1DD"];

/// **The size a tool's symbol is drawn at**, as the `text_size` of the tool
/// that shows one. A symbol is a glyph, and a glyph's size is its text's; a
/// tool's is nearly twice the size of the words beside it, the proportion an
/// icon has to a caption, so that an accidental stands as tall as a capital
/// and a half and a note with its stem and its flags stays inside the bar.
/// The em comes to 22 logical pixels.
pub const SYMBOL_SIZE: f64 = 2.5;

/// The augmentation dot, a quarter rest and the triplet's figure.
const DOT: &str = "E1E7";
const REST: &str = "E4E5";
const TRIPLET: &str = "E883";

/// The accidentals the toolbar offers: the text label, the symbol, and the
/// semitones of alteration; the first segment is none.
pub const ACCIDENTALS: &[(&str, &str, i32)] = &[
    ("bb", "E264", -2),
    ("b", "E260", -1),
    ("nat", "E261", 0),
    ("#", "E262", 1),
    ("x", "E263", 2),
];

/// The articulations it offers, by tool name: the text label, the symbol, and
/// what it says.
const ARTICULATIONS: &[(&str, &str, &str, &str)] = &[
    ("stacc", ".", "E4A2", "Staccato"),
    ("acc", ">", "E4A0", "Accent"),
    ("ten", "-", "E4A4", "Tenuto"),
    ("marc", "^", "E4AC", "Marcato"),
];

/// **Every codepoint a tool is drawn with**: what the editor asks the
/// engraver's outlines for.
#[must_use]
pub fn codes() -> Vec<&'static str> {
    VALUE_CODES
        .into_iter()
        .chain([DOT, REST, TRIPLET])
        .chain(ACCIDENTALS.iter().map(|a| a.1))
        .chain(ARTICULATIONS.iter().map(|a| a.2))
        .chain([icons::TIE, icons::NONE, icons::REWIND, icons::ENTRY])
        .collect()
}

/// What a tool shows: the symbol `code`, as its character, when the engraver
/// handed its outline out, and `text` otherwise.
fn shown(outlines: &Outlines, code: &str, text: &str) -> String {
    outlines
        .contains_key(code)
        .then(|| glyph_char(code))
        .flatten()
        .map_or_else(|| text.to_string(), String::from)
}

/// What the toolbar shows of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    /// Whether the window is in note entry.
    pub entry: bool,
    /// The written value a note is entered with, undotted.
    pub value: Ratio,
    /// Whether the value is dotted.
    pub dotted: bool,
    /// Whether a press on empty staff writes a rest.
    pub rest: bool,
    /// The accidental the next note takes, in semitones, when one is armed.
    pub accidental: Option<i32>,
    /// The voice of what is selected, from zero.
    pub voice: usize,
    /// How the window looks at the score.
    pub view: View,
    /// Whether a pass loops.
    pub looping: bool,
}

impl State {
    fn value_index(&self) -> usize {
        VALUES
            .iter()
            .position(|(_, (n, d))| Ratio::new(*n, *d) == self.value)
            .unwrap_or(2)
    }

    fn accidental_index(&self) -> usize {
        self.accidental
            .and_then(|alter| ACCIDENTALS.iter().position(|(_, _, a)| *a == alter))
            .map_or(0, |at| at + 1)
    }

    /// What each tool is set to, by name, as the prop that shows it.
    fn shown(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("entry", json!({"value": i32::from(self.entry)})),
            ("value", json!({"index": self.value_index()})),
            ("dot", json!({"value": i32::from(self.dotted)})),
            ("rest", json!({"value": i32::from(self.rest)})),
            ("accidental", json!({"index": self.accidental_index()})),
            ("voice", json!({"index": self.voice.min(1)})),
            (
                "layout",
                json!({"index": usize::from(self.view == View::Continuous)}),
            ),
            ("loop", json!({"value": i32::from(self.looping)})),
        ]
    }
}

/// One tool's widget, as a GuiDef node, or nothing for a tool with no id.
fn tool(ids: &Ids, name: &str, mut node: Value) -> Option<Value> {
    let id = *ids.get(name)?;
    node.as_object_mut()?.insert("id".into(), json!(id));
    Some(node)
}

fn segments(options: Vec<String>, tip: &str) -> Value {
    json!({"type": "choice", "view": "segmented", "options": options, "tip": tip})
}

fn latch(label: &str, tip: &str) -> Value {
    json!({"type": "toggle", "view": "button", "label": label, "tip": tip})
}

fn press(label: &str, tip: &str) -> Value {
    json!({"type": "button", "flat": true, "label": label, "tip": tip})
}

/// `node` as a tool that shows symbols: drawn at [`SYMBOL_SIZE`].
fn symbols(mut node: Value) -> Value {
    if let Some(map) = node.as_object_mut() {
        map.insert("text_size".into(), json!(SYMBOL_SIZE));
    }
    node
}

/// **The toolbar**, as a GuiDef row -- or `None` when the caller numbered no
/// tool, which is a window without one. A tool whose symbol is in `outlines`
/// is labelled with it.
#[must_use]
pub fn toolbar(ids: &Ids, state: &State, outlines: &Outlines) -> Option<Value> {
    let shown = |code: &str, text: &str| shown(outlines, code, text);
    let sep = || json!({"type": "separator"});
    let mut children: Vec<Value> = Vec::new();
    let group = |tools: Vec<Option<Value>>, children: &mut Vec<Value>| {
        let tools: Vec<Value> = tools.into_iter().flatten().collect();
        if tools.is_empty() {
            return;
        }
        if !children.is_empty() {
            children.push(sep());
        }
        children.extend(tools);
    };
    group(
        vec![tool(
            ids,
            "entry",
            symbols(latch(
                &shown(icons::ENTRY, "N"),
                "Note entry: write notes at the cursor (N)",
            )),
        )],
        &mut children,
    );
    group(
        vec![
            tool(
                ids,
                "value",
                symbols(segments(
                    VALUES
                        .iter()
                        .zip(VALUE_CODES)
                        .map(|((_, (n, d)), code)| {
                            let text = if *d == 1 {
                                n.to_string()
                            } else {
                                format!("{n}/{d}")
                            };
                            shown(code, &text)
                        })
                        .collect(),
                    "The value a note is written with",
                )),
            ),
            tool(ids, "dot", symbols(latch(&shown(DOT, "."), "Dotted"))),
            tool(
                ids,
                "rest",
                symbols(latch(&shown(REST, "rest"), "Write rests")),
            ),
        ],
        &mut children,
    );
    group(
        vec![tool(
            ids,
            "accidental",
            symbols(segments(
                std::iter::once(shown(icons::NONE, "-"))
                    .chain(ACCIDENTALS.iter().map(|(text, code, _)| shown(code, text)))
                    .collect(),
                "The accidental: of what is selected, or of the next note",
            )),
        )],
        &mut children,
    );
    group(
        ARTICULATIONS
            .iter()
            .map(|(name, text, code, tip)| tool(ids, name, symbols(press(&shown(code, text), tip))))
            .collect(),
        &mut children,
    );
    group(
        vec![
            tool(
                ids,
                "tie",
                symbols(press(&shown(icons::TIE, "tie"), "Tie to the next note")),
            ),
            tool(
                ids,
                "tuplet",
                symbols(press(
                    &shown(TRIPLET, "3"),
                    "Triplet: three in the time of two",
                )),
            ),
        ],
        &mut children,
    );
    group(
        vec![tool(
            ids,
            "voice",
            segments(vec!["v1".into(), "v2".into()], "The voice"),
        )],
        &mut children,
    );
    group(
        vec![tool(
            ids,
            "layout",
            segments(
                vec!["page".into(), "line".into()],
                "Pages of the paper, or one continuous system",
            ),
        )],
        &mut children,
    );
    // the transport goes to the far edge, past a spring: the host's own
    // symbols, which every face draws, at the size of the other tools'
    let transport: Vec<Value> = [
        tool(
            ids,
            "rewind",
            symbols(press(&shown(icons::REWIND, "|<"), "Back to the start")),
        ),
        tool(
            ids,
            "play",
            symbols(press("\u{25B6}", "Play, or stop (the space bar)")),
        ),
        tool(
            ids,
            "loop",
            symbols(latch("\u{21BB}", "Loop the selection, or the score (L)")),
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !transport.is_empty() {
        children.push(json!({"type": "separator", "weight": 1, "line": false}));
        children.extend(transport);
    }
    if children.is_empty() {
        return None;
    }
    let mut row = json!({
        "type": "layout",
        "flow": "row",
        "hug": true,
        "pack": true,
        "children": children,
    });
    // the state each tool opens on
    if let Some(children) = row.get_mut("children").and_then(Value::as_array_mut) {
        for (name, props) in state.shown() {
            let Some(id) = ids.get(name) else { continue };
            for child in children.iter_mut().filter(|c| c["id"] == json!(id)) {
                if let (Some(child), Value::Object(props)) = (child.as_object_mut(), props.clone())
                {
                    child.extend(props);
                }
            }
        }
    }
    Some(row)
}

/// **What the tools are corrected with** when the editor's state moved: each
/// one that shows state, as `(widget, props)`.
#[must_use]
pub fn corrections(ids: &Ids, state: &State) -> Vec<(i32, Value)> {
    state
        .shown()
        .into_iter()
        .filter_map(|(name, props)| Some((*ids.get(name)?, props)))
        .collect()
}

/// **What a tool's report asks of the editor.**
#[derive(Clone, Debug, PartialEq)]
pub enum Tool {
    /// Into note entry, or out of it.
    Entry(bool),
    /// The value a note is written with.
    Value(Ratio),
    /// Whether it is dotted.
    Dot(bool),
    /// Whether a press writes a rest.
    Rest(bool),
    /// The accidental: of the selection, or armed for the next note.
    Accidental(Option<i32>),
    /// A verb over the selection, as the editor writes it.
    Act(Value),
    /// The voice the selection goes to, from zero.
    Voice(usize),
    /// How the window looks at the score.
    Layout(View),
    /// Back to the start.
    Rewind,
    /// Play, or stop what plays.
    Play,
    /// Whether a pass loops.
    Loop(bool),
}

/// **What the tool `name` reporting `tag` asks for** -- or `None` for a name
/// that is no tool, and for a report that asks nothing.
///
/// A tool that holds state reports its value where a tag would be: a choice's
/// index, a toggle's 0 or 1. One that acts is a button, and a button says two
/// things -- its value, which is a control signal and rises and falls with the
/// hand, and `"click"`, which is the command -- so only the click is read.
#[must_use]
pub fn read(name: &str, tag: &str) -> Option<Tool> {
    let acts = matches!(name, "tie" | "tuplet" | "rewind" | "play")
        || ARTICULATIONS.iter().any(|a| a.0 == name);
    if acts {
        if tag != "click" {
            return None;
        }
        match name {
            "rewind" => return Some(Tool::Rewind),
            "play" => return Some(Tool::Play),
            _ => {}
        }
        return Some(Tool::Act(match name {
            "tie" => json!({"action": "tie"}),
            "tuplet" => json!({"action": "scale", "factor": [2, 3]}),
            articulation => json!({"action": "articulation", "name": articulation}),
        }));
    }
    let value: f64 = tag.parse().ok()?;
    let index = value.max(0.0) as usize;
    let on = value != 0.0;
    Some(match name {
        "value" => {
            let (_, (n, d)) = VALUES.get(index)?;
            Tool::Value(Ratio::new(*n, *d))
        }
        "entry" => Tool::Entry(on),
        "dot" => Tool::Dot(on),
        "loop" => Tool::Loop(on),
        "rest" => Tool::Rest(on),
        "accidental" => Tool::Accidental(match index {
            0 => None,
            at => Some(ACCIDENTALS.get(at - 1)?.2),
        }),
        "voice" => Tool::Voice(index.min(1)),
        "layout" => Tool::Layout(if index == 0 {
            View::Page
        } else {
            View::Continuous
        }),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Ids {
        TOOLS
            .iter()
            .enumerate()
            .map(|(i, name)| ((*name).to_string(), 100 + i as i32))
            .collect()
    }

    /// The id the test numbers the tool `name` under.
    fn id(name: &str) -> i32 {
        ids()[name]
    }

    fn state() -> State {
        State {
            entry: true,
            value: Ratio::new(1, 8),
            dotted: true,
            rest: false,
            accidental: Some(1),
            voice: 1,
            view: View::Continuous,
            looping: true,
        }
    }

    #[test]
    fn the_row_holds_every_tool_under_the_callers_id_showing_the_state() {
        let row = toolbar(&ids(), &state(), &Outlines::new()).expect("a toolbar");
        let children = row["children"].as_array().unwrap();
        let by = |name: &str| {
            children
                .iter()
                .find(|c| c["id"] == json!(id(name)))
                .unwrap()
        };
        for name in TOOLS {
            assert!(by(name).is_object(), "{name}");
        }
        assert_eq!(by("entry")["value"], 1, "in note entry");
        assert_eq!(by("value")["index"], 3, "an eighth is the fourth value");
        assert_eq!(by("dot")["value"], 1, "dotted");
        assert_eq!(
            by("accidental")["index"],
            4,
            "a sharp, past none, bb, b and nat"
        );
        assert_eq!(by("voice")["index"], 1, "the second voice");
        assert_eq!(by("layout")["index"], 1, "continuous");
        assert_eq!(by("loop")["value"], 1, "looping");
        // the transport is past the spring, at the far edge
        let spring = children.iter().position(|c| c["weight"] == 1).unwrap();
        assert_eq!(children[spring + 1]["id"], id("rewind"));
        // a window that numbered no tool has no toolbar
        assert!(toolbar(&Ids::new(), &state(), &Outlines::new()).is_none());
    }

    #[test]
    fn a_tool_is_labelled_with_its_symbol_where_the_engraver_has_it() {
        // the engraver handed out a quarter and a sharp, and nothing else
        let outlines: Outlines = [("E1D5", "M0 0h1v1z"), ("E262", "M0 0h1v1z")]
            .into_iter()
            .map(|(code, path)| (code.to_string(), path.to_string()))
            .collect();
        let row = toolbar(&ids(), &state(), &outlines).expect("a toolbar");
        let children = row["children"].as_array().unwrap();
        let by = |name: &str| {
            children
                .iter()
                .find(|c| c["id"] == json!(id(name)))
                .unwrap()
        };
        let values = by("value")["options"].as_array().unwrap();
        assert_eq!(values[2], "\u{E1D5}", "the quarter is its symbol");
        assert_eq!(values[3], "1/8", "the eighth has none and stays text");
        assert_eq!(by("accidental")["options"][4], "\u{E262}");
        assert_eq!(by("accidental")["options"][0], "-", "and so does none");
        assert_eq!(by("stacc")["label"], ".");
        assert_eq!(by("entry")["label"], "N", "no pencil was handed out");
        // a tool that shows symbols is drawn at their size, and one that
        // shows words at the words'
        assert_eq!(by("value")["text_size"], SYMBOL_SIZE);
        assert!(by("voice").get("text_size").is_none(), "the voice is words");
        // every symbol asked for is a codepoint, each once
        let asked = codes();
        assert_eq!(asked.len(), 23);
        assert!(asked.iter().all(|code| glyph_char(code).is_some()));
    }

    #[test]
    fn a_report_is_read_as_what_it_asks() {
        assert_eq!(read("value", "3"), Some(Tool::Value(Ratio::new(1, 8))));
        assert_eq!(read("dot", "1"), Some(Tool::Dot(true)));
        assert_eq!(read("entry", "1"), Some(Tool::Entry(true)));
        assert_eq!(read("accidental", "0"), Some(Tool::Accidental(None)));
        assert_eq!(read("accidental", "2"), Some(Tool::Accidental(Some(-1))));
        assert_eq!(read("voice", "1"), Some(Tool::Voice(1)));
        assert_eq!(read("layout", "1"), Some(Tool::Layout(View::Continuous)));
        assert_eq!(read("value", "99"), None);
        assert_eq!(read("zoom", "1"), None);
        // the transport: two commands and a switch
        assert_eq!(read("play", "click"), Some(Tool::Play));
        assert_eq!(read("rewind", "click"), Some(Tool::Rewind));
        assert_eq!(read("play", "1"), None);
        assert_eq!(read("loop", "1"), Some(Tool::Loop(true)));
    }

    #[test]
    fn a_tool_that_acts_is_read_from_its_click_alone() {
        let staccato = Tool::Act(json!({"action": "articulation", "name": "stacc"}));
        assert_eq!(read("stacc", "click"), Some(staccato));
        assert_eq!(
            read("tie", "click"),
            Some(Tool::Act(json!({"action": "tie"})))
        );
        // its value rises and falls with the hand, and is no command
        assert_eq!(read("stacc", "1"), None);
        assert_eq!(read("stacc", "0"), None);
        assert_eq!(read("tie", "press"), None);
        // and a tool that holds state has no click
        assert_eq!(read("dot", "click"), None);
    }

    #[test]
    fn the_state_that_moved_is_what_the_tools_are_corrected_with() {
        let corrected = corrections(&ids(), &state());
        assert!(corrected.contains(&(id("value"), json!({"index": 3}))));
        assert!(corrected.contains(&(id("dot"), json!({"value": 1}))));
        assert!(corrected.contains(&(id("entry"), json!({"value": 1}))));
        // an articulation shows no state, so it is not corrected
        assert!(corrected.iter().all(|(at, _)| *at != id("stacc")));
    }
}
