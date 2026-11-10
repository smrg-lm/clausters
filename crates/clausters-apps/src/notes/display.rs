//! **How the roll draws what a sequence holds beyond its boxes**, held by
//! the notes editor for its window and shown in a panel beside the roll
//! (`crate::panels`), as the audio editor's spectrogram settings are.
//!
//! Five sections: the **slurs** (not drawn, or one of four drawings), the
//! **articulation glyphs** (not drawn, or stacked at a note's start or in a
//! row along it, inside its box or just above), the **pitch line** between
//! the boxes, the **notes' own curves** and the **lanes** offered as tabs
//! under the roll. The first two are a page's marks and draw in notation mode
//! alone, so in MIDI mode their widgets are disabled. All of it is view
//! state, outside the history, and it reaches the roll as what the roll is
//! sent: its `slur_style` and `mark_layout`, and the lines, curves and lanes
//! it is sent or not.
//!
//! The panel's widgets are named here ([`names`]) and numbered by the caller
//! with the dialogs', so a window composed without them has no panel, and a
//! report from one is read here ([`Display::read`]). A lane's switch is one
//! of [`LANE_SLOTS`] slots, the n-th lane of the sequence in each.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How wide the panel's column is.
pub const WIDTH: f64 = 260.0;

/// The prefix the panel's widgets are named under.
pub const PREFIX: &str = "display";

/// **How a slur is drawn**, by name and as the panel says it, in the order
/// offered: not at all, an arc over the notes from the first box to the
/// last as a page writes it, the same arc dashed where the slur attacks its
/// notes, a phrasing bracket over the highest box, a band behind the notes.
pub const SLURS: &[(&str, &str)] = &[
    ("none", "None"),
    ("arc", "Arc"),
    ("dashed", "Arc, dashed if attacked"),
    ("bracket", "Bracket"),
    ("band", "Band"),
];

/// **How a note's articulations are laid out**: not drawn, stacked at its
/// start, or in a row along its length.
pub const MARK_LAYOUTS: &[(&str, &str)] = &[
    ("none", "None"),
    ("stacked", "Stacked at the start"),
    ("row", "In a row along the note"),
];

/// **Where a note's articulations are drawn**: just above its box, or
/// inside it.
pub const MARK_PLACES: &[(&str, &str)] =
    &[("above", "Above the note"), ("inside", "Inside the note")];

/// How many lanes the panel offers a switch for: the sequence's first ones.
pub const LANE_SLOTS: usize = 16;

/// **How the roll draws what a sequence holds beyond its boxes.**
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    /// How a slur is drawn, by name ([`SLURS`]).
    pub slurs: String,
    /// How a note's articulations are laid out, by name ([`MARK_LAYOUTS`]).
    pub marks: String,
    /// Where they are drawn, by name ([`MARK_PLACES`]).
    pub marks_place: String,
    /// Whether the pitch line is drawn between the boxes.
    pub pitch_line: bool,
    /// Whether the notes' own curves are drawn against their boxes.
    pub note_curves: bool,
    /// **The lanes not offered as tabs**, by name (`velocity`, or a curve's
    /// id): a lane taken off the tabs is not drawn.
    pub hidden_lanes: Vec<String>,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            slurs: "arc".into(),
            marks: "row".into(),
            marks_place: "above".into(),
            pitch_line: true,
            note_curves: true,
            hidden_lanes: Vec::new(),
        }
    }
}

/// The widgets of the panel, by name.
#[must_use]
pub fn names() -> Vec<String> {
    ["slurs", "marks", "marks_place", "pitch_line", "note_curves"]
        .iter()
        .map(|n| format!("{PREFIX}:{n}"))
        .chain((0..LANE_SLOTS).map(|i| format!("{PREFIX}:lane:{i}")))
        .collect()
}

/// The caller's widget id for each widget of the panel it numbered.
pub type Ids = crate::forms::Ids;

/// The index of `name` among `table`'s names.
fn index_of(table: &[(&str, &str)], name: &str) -> usize {
    table.iter().position(|(n, _)| *n == name).unwrap_or(0)
}

impl Display {
    /// **Whether lane `name` is offered as a tab.**
    #[must_use]
    pub fn shows_lane(&self, name: &str) -> bool {
        !self.hidden_lanes.iter().any(|n| n == name)
    }

    /// **Changes the settings a request names**, refusing a value none of
    /// them takes and changing nothing then. `lanes` names the lanes `{name:
    /// shown}` sets, a switch each; a name the sequence does not have is
    /// kept, for the curve it may have again after an undo.
    pub fn set(&mut self, request: &Value) -> Result<(), String> {
        let mut next = self.clone();
        let named = |value: &Value, table: &[(&str, &str)], what: &str| {
            let word = value.as_str().ok_or_else(|| format!("a {what} is named"))?;
            table
                .iter()
                .any(|(n, _)| *n == word)
                .then(|| word.to_string())
                .ok_or_else(|| {
                    let offered: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
                    format!("{word:?} is no {what}: one of {offered:?}")
                })
        };
        let switch = |value: &Value, what: &str| {
            value
                .as_bool()
                .or_else(|| value.as_f64().map(|n| n != 0.0))
                .ok_or_else(|| format!("{what} is on or off"))
        };
        if let Some(slurs) = request.get("slurs") {
            next.slurs = named(slurs, SLURS, "slur drawing")?;
        }
        if let Some(marks) = request.get("marks") {
            next.marks = named(marks, MARK_LAYOUTS, "marks layout")?;
        }
        if let Some(place) = request.get("marks_place") {
            next.marks_place = named(place, MARK_PLACES, "marks place")?;
        }
        if let Some(on) = request.get("pitch_line") {
            next.pitch_line = switch(on, "the pitch line")?;
        }
        if let Some(on) = request.get("note_curves") {
            next.note_curves = switch(on, "the notes' curves")?;
        }
        if let Some(lanes) = request.get("lanes") {
            let lanes = lanes
                .as_object()
                .ok_or("lanes are {name: shown} switches")?;
            for (name, shown) in lanes {
                let shown = switch(shown, "a lane")?;
                next.hidden_lanes.retain(|n| n != name);
                if !shown {
                    next.hidden_lanes.push(name.clone());
                }
            }
        }
        *self = next;
        Ok(())
    }

    /// **What a report from the panel's widget `name` changes**, read as the
    /// request [`Display::set`] takes: a choice reports the index of what was
    /// chosen and a switch its state; a lane's switch is the slot's lane of
    /// `lanes`, the sequence's by name. `None` for a report that is not a
    /// value.
    #[must_use]
    pub fn read(name: &str, value: &Value, lanes: &[(String, String)]) -> Option<Value> {
        let part = name.strip_prefix(PREFIX)?.strip_prefix(':')?;
        let number = value
            .as_f64()
            .or_else(|| value.as_str().and_then(|s| s.parse().ok()))?;
        let index = number.round().max(0.0) as usize;
        if let Some(slot) = part.strip_prefix("lane:") {
            let (lane, _) = lanes.get(slot.parse::<usize>().ok()?)?;
            return Some(json!({"lanes": {lane.clone(): number != 0.0}}));
        }
        Some(match part {
            "slurs" => json!({"slurs": SLURS.get(index)?.0}),
            "marks" => json!({"marks": MARK_LAYOUTS.get(index)?.0}),
            "marks_place" => json!({"marks_place": MARK_PLACES.get(index)?.0}),
            "pitch_line" => json!({"pitch_line": number != 0.0}),
            "note_curves" => json!({"note_curves": number != 0.0}),
            _ => return None,
        })
    }

    /// **The panel**, its widgets showing these settings over the sequence's
    /// `lanes` (name and label), or `None` when the caller numbered none of
    /// them. `notation` enables what a page's marks are.
    #[must_use]
    pub fn panel(&self, ids: &Ids, lanes: &[(String, String)], notation: bool) -> Option<Value> {
        let sections: Vec<Value> = self
            .widgets(ids, lanes, notation)
            .into_iter()
            .filter_map(|(title, widgets)| {
                (!widgets.is_empty()).then(|| {
                    let mut section = crate::panels::section(title, false, widgets);
                    section.insert("flow".into(), json!("col"));
                    Value::Object(section)
                })
            })
            .collect();
        crate::panels::column(sections, WIDTH)
    }

    /// **What the panel's widgets are corrected with**: each one's value and
    /// whether it takes a hand, as [`Display::panel`] draws them.
    #[must_use]
    pub fn corrections(
        &self,
        ids: &Ids,
        lanes: &[(String, String)],
        notation: bool,
    ) -> Vec<clausters_editing::conversation::Correction> {
        self.widgets(ids, lanes, notation)
            .into_iter()
            .flat_map(|(_, widgets)| widgets)
            .filter_map(|w| {
                let widget = w["id"].as_i64()?;
                let mut props = serde_json::Map::new();
                for key in ["index", "value", "enabled", "label"] {
                    if let Some(v) = w.get(key) {
                        props.insert(key.into(), v.clone());
                    }
                }
                Some(clausters_editing::conversation::Correction {
                    widget,
                    props: Value::Object(props),
                })
            })
            .collect()
    }

    /// **The roll's props, drawn as these settings say**: what is not drawn
    /// is sent empty (the slurs, the marks, the pitch lines, the notes'
    /// curves), and how the rest is drawn rides beside it (`slur_style`,
    /// `mark_layout`, `mark_place`).
    pub fn apply(&self, props: &mut serde_json::Map<String, Value>) {
        let empty = |props: &mut serde_json::Map<String, Value>, key: &str| {
            if props.contains_key(key) {
                props.insert(key.into(), json!([]));
            }
        };
        if self.slurs == "none" {
            empty(props, "slurs");
        }
        if !self.pitch_line {
            empty(props, "lines");
        }
        if !self.note_curves {
            empty(props, "layers");
        }
        if self.marks == "none"
            && let Some(Value::Array(marks)) = props.get_mut("marks")
        {
            for mark in marks.iter_mut() {
                *mark = json!("");
            }
        }
        props.insert("slur_style".into(), json!(self.slurs));
        props.insert("mark_layout".into(), json!(self.marks));
        props.insert("mark_place".into(), json!(self.marks_place));
    }

    /// The panel's sections, each its title and its widgets.
    fn widgets(
        &self,
        ids: &Ids,
        lanes: &[(String, String)],
        notation: bool,
    ) -> Vec<(&'static str, Vec<Value>)> {
        let id = |name: &str| ids.get(&format!("{PREFIX}:{name}")).copied();
        let choice = |name: &str, label: &str, table: &[(&str, &str)], at: &str, on: bool| {
            id(name).map(|id| {
                json!({"type": "choice", "id": id, "label": label,
                    "options": table.iter().map(|(_, l)| *l).collect::<Vec<_>>(),
                    "index": index_of(table, at), "enabled": u8::from(on)})
            })
        };
        let toggle = |name: &str, label: &str, on: bool| {
            id(name).map(
                |id| json!({"type": "toggle", "id": id, "label": label, "value": u8::from(on)}),
            )
        };
        let slurs = vec![choice("slurs", "Drawn as", SLURS, &self.slurs, notation)];
        let marks = vec![
            choice("marks", "Laid out", MARK_LAYOUTS, &self.marks, notation),
            choice(
                "marks_place",
                "Where",
                MARK_PLACES,
                &self.marks_place,
                notation && self.marks != "none",
            ),
        ];
        let lines = vec![
            toggle("pitch_line", "Pitch line between notes", self.pitch_line),
            toggle("note_curves", "Notes' own curves", self.note_curves),
        ];
        let tabs = lanes
            .iter()
            .take(LANE_SLOTS)
            .enumerate()
            .map(|(i, (name, label))| toggle(&format!("lane:{i}"), label, self.shows_lane(name)))
            .collect();
        [
            ("Slurs", slurs),
            ("Articulations", marks),
            ("Lines and curves", lines),
            ("Lanes", tabs),
        ]
        .into_iter()
        .map(|(title, widgets)| (title, widgets.into_iter().flatten().collect()))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Ids {
        names()
            .into_iter()
            .enumerate()
            .map(|(i, n)| (n, 700 + i as i32))
            .collect()
    }

    fn lanes() -> Vec<(String, String)> {
        vec![
            ("velocity".into(), "velocity".into()),
            ("9".into(), "CC 1".into()),
        ]
    }

    /// **A request is read whole or not at all**, and a lane is switched by
    /// its name.
    #[test]
    fn a_request_is_read_whole_or_not_at_all() {
        let mut d = Display::default();
        d.set(
            &json!({"slurs": "band", "marks": "stacked", "marks_place": "inside",
            "pitch_line": false, "note_curves": 0, "lanes": {"9": false}}),
        )
        .unwrap();
        assert_eq!(
            (d.slurs.as_str(), d.marks.as_str(), d.marks_place.as_str()),
            ("band", "stacked", "inside")
        );
        assert!(!d.pitch_line && !d.note_curves);
        assert!(d.shows_lane("velocity") && !d.shows_lane("9"));
        let before = d.clone();
        assert!(d.set(&json!({"slurs": "arc", "marks": "spiral"})).is_err());
        assert!(d.set(&json!({"lanes": {"9": "maybe"}})).is_err());
        assert_eq!(d, before);
        d.set(&json!({"lanes": {"9": true}})).unwrap();
        assert!(d.shows_lane("9"));
    }

    /// **The panel shows the settings, a page's marks only in notation
    /// mode, and a widget's report is the request it comes to.**
    #[test]
    fn the_panel_shows_the_settings_and_a_report_is_read_as_a_request() {
        let d = Display::default();
        let panel = d.panel(&ids(), &lanes(), false).expect("a panel");
        let sections = panel["children"].as_array().unwrap();
        let titles: Vec<&Value> = sections.iter().map(|s| &s["title"]).collect();
        assert_eq!(
            titles,
            ["Slurs", "Articulations", "Lines and curves", "Lanes"]
        );
        assert_eq!(sections[0]["children"][0]["index"], 1, "an arc");
        assert_eq!(
            sections[0]["children"][0]["enabled"], 0,
            "a page's, in MIDI mode"
        );
        assert_eq!(sections[3]["children"].as_array().unwrap().len(), 2);
        assert_eq!(sections[3]["children"][1]["label"], "CC 1");
        let notation = d.panel(&ids(), &lanes(), true).unwrap();
        assert_eq!(notation["children"][0]["children"][0]["enabled"], 1);
        assert_eq!(
            Display::read("display:slurs", &json!(3), &lanes()),
            Some(json!({"slurs": "bracket"}))
        );
        assert_eq!(
            Display::read("display:marks_place", &json!(1), &lanes()),
            Some(json!({"marks_place": "inside"}))
        );
        assert_eq!(
            Display::read("display:lane:1", &json!(0), &lanes()),
            Some(json!({"lanes": {"9": false}}))
        );
        assert_eq!(Display::read("display:lane:5", &json!(0), &lanes()), None);
        assert_eq!(Display::read("display:slurs", &json!(9), &lanes()), None);
        assert!(d.panel(&Ids::new(), &lanes(), true).is_none());
    }
}
