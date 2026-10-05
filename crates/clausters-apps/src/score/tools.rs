//! **The score editor's toolbar**: the values a hand reaches for while it
//! writes, one press away.
//!
//! A row of light controls over the page: the **value** a note is written
//! with, its dot, whether it is a rest, its accidental; the common
//! articulations, a tie and a triplet for what is selected; the voice; and how
//! the window looks at the score. Each is a widget of the host's -- a
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

use std::collections::BTreeMap;

use serde_json::{Value, json};

use clausters_core::notation::View;
use clausters_core::ratio::Ratio;

use super::menu::VALUES;

/// The tools, by the names the caller numbers them under, left to right.
pub const TOOLS: &[&str] = &[
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
];

/// The caller's widget id for each tool it numbered.
pub type Ids = BTreeMap<String, i32>;

/// The accidentals the toolbar offers, as semitones of alteration; the first
/// segment is none.
pub const ACCIDENTALS: &[(&str, i32)] = &[("bb", -2), ("b", -1), ("nat", 0), ("#", 1), ("x", 2)];

/// The articulations it offers, by tool name: the label and what it says.
const ARTICULATIONS: &[(&str, &str, &str)] = &[
    ("stacc", ".", "Staccato"),
    ("acc", ">", "Accent"),
    ("ten", "-", "Tenuto"),
    ("marc", "^", "Marcato"),
];

/// What the toolbar shows of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
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
            .and_then(|alter| ACCIDENTALS.iter().position(|(_, a)| *a == alter))
            .map_or(0, |at| at + 1)
    }

    /// What each tool is set to, by name, as the prop that shows it.
    fn shown(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("value", json!({"index": self.value_index()})),
            ("dot", json!({"value": i32::from(self.dotted)})),
            ("rest", json!({"value": i32::from(self.rest)})),
            ("accidental", json!({"index": self.accidental_index()})),
            ("voice", json!({"index": self.voice.min(1)})),
            (
                "layout",
                json!({"index": usize::from(self.view == View::Continuous)}),
            ),
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

/// **The toolbar**, as a GuiDef row -- or `None` when the caller numbered no
/// tool, which is a window without one.
#[must_use]
pub fn toolbar(ids: &Ids, state: &State) -> Option<Value> {
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
        vec![
            tool(
                ids,
                "value",
                segments(
                    VALUES
                        .iter()
                        .map(|(_, (n, d))| {
                            if *d == 1 {
                                n.to_string()
                            } else {
                                format!("{n}/{d}")
                            }
                        })
                        .collect(),
                    "The value a note is written with",
                ),
            ),
            tool(ids, "dot", latch(".", "Dotted")),
            tool(ids, "rest", latch("rest", "Write rests")),
        ],
        &mut children,
    );
    group(
        vec![tool(
            ids,
            "accidental",
            segments(
                std::iter::once("-".to_string())
                    .chain(ACCIDENTALS.iter().map(|(label, _)| (*label).to_string()))
                    .collect(),
                "The accidental: of what is selected, or of the next note",
            ),
        )],
        &mut children,
    );
    group(
        ARTICULATIONS
            .iter()
            .map(|(name, label, tip)| tool(ids, name, press(label, tip)))
            .collect(),
        &mut children,
    );
    group(
        vec![
            tool(ids, "tie", press("tie", "Tie to the next note")),
            tool(
                ids,
                "tuplet",
                press("3", "Triplet: three in the time of two"),
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
    if children.is_empty() {
        return None;
    }
    // the layout goes to the far edge, past a spring
    if let Some(layout) = tool(
        ids,
        "layout",
        segments(
            vec!["page".into(), "line".into()],
            "Pages of the paper, or one continuous system",
        ),
    ) {
        children.push(json!({"type": "separator", "weight": 1, "line": false}));
        children.push(layout);
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
    let acts = matches!(name, "tie" | "tuplet") || ARTICULATIONS.iter().any(|a| a.0 == name);
    if acts {
        if tag != "click" {
            return None;
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
        "dot" => Tool::Dot(on),
        "rest" => Tool::Rest(on),
        "accidental" => Tool::Accidental(match index {
            0 => None,
            at => Some(ACCIDENTALS.get(at - 1)?.1),
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

    fn state() -> State {
        State {
            value: Ratio::new(1, 8),
            dotted: true,
            rest: false,
            accidental: Some(1),
            voice: 1,
            view: View::Continuous,
        }
    }

    #[test]
    fn the_row_holds_every_tool_under_the_callers_id_showing_the_state() {
        let row = toolbar(&ids(), &state()).expect("a toolbar");
        let children = row["children"].as_array().unwrap();
        let by_id = |id: i32| children.iter().find(|c| c["id"] == json!(id)).unwrap();
        for (i, name) in TOOLS.iter().enumerate() {
            assert!(by_id(100 + i as i32).is_object(), "{name}");
        }
        assert_eq!(by_id(100)["index"], 3, "an eighth is the fourth value");
        assert_eq!(by_id(101)["value"], 1, "dotted");
        assert_eq!(by_id(103)["index"], 4, "a sharp, past none, bb, b and nat");
        assert_eq!(by_id(110)["index"], 1, "the second voice");
        assert_eq!(by_id(111)["index"], 1, "continuous");
        // a window that numbered no tool has no toolbar
        assert!(toolbar(&Ids::new(), &state()).is_none());
    }

    #[test]
    fn a_report_is_read_as_what_it_asks() {
        assert_eq!(read("value", "3"), Some(Tool::Value(Ratio::new(1, 8))));
        assert_eq!(read("dot", "1"), Some(Tool::Dot(true)));
        assert_eq!(read("accidental", "0"), Some(Tool::Accidental(None)));
        assert_eq!(read("accidental", "2"), Some(Tool::Accidental(Some(-1))));
        assert_eq!(read("voice", "1"), Some(Tool::Voice(1)));
        assert_eq!(read("layout", "1"), Some(Tool::Layout(View::Continuous)));
        assert_eq!(read("value", "99"), None);
        assert_eq!(read("zoom", "1"), None);
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
        assert!(corrected.contains(&(100, json!({"index": 3}))));
        assert!(corrected.contains(&(101, json!({"value": 1}))));
        // an articulation shows no state, so it is not corrected
        assert!(corrected.iter().all(|(id, _)| *id != 104));
    }
}
