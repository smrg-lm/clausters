//! **An application's panels**: a column of titled sections beside what the
//! window works on, one mechanism for every application that has any.
//!
//! A section is a framed group with a title that folds it on its strip, which
//! is the host's to remember; the column scrolls, since how tall it is
//! depends on which sections are open; and it stands beside the work with a
//! divider a drag moves. The widgets inside are the application's, named by
//! it and numbered by the caller, as the tools and the dialogs are, so a
//! window composed without them has no panel. The score editor's palettes
//! are its first one, the audio editor's spectrogram settings its second.
//!
//! **A panel of several parts is one section** ([`parted`]): the
//! spectrogram's settings (Analysis, Display), the roll's inspector
//! (Selection, Events) and its display settings are each one panel that
//! folds whole, its parts headed and parted by a line inside it, as a
//! menu's groups are. The score's palettes are a column of palettes, each
//! its own section.

use serde_json::{Map, Value, json};

/// The room around the sections in their column, which is also where the
/// column's scroll bar runs: wide enough that the bar covers no section.
pub const MARGIN: f64 = 10.0;

/// **A section of a panel**: `children` in a frame under `title`, folded or
/// open to begin with. The caller adds how they are laid out (`flow`, `cols`)
/// to what it answers.
pub fn section(title: &str, folded: bool, children: Vec<Value>) -> Map<String, Value> {
    let mut section = Map::new();
    section.insert("type".into(), json!("layout"));
    section.insert("title".into(), json!(title));
    section.insert("frame".into(), json!(true));
    section.insert("collapsed".into(), json!(folded));
    section.insert("hug".into(), json!(true));
    section.insert("children".into(), Value::Array(children));
    section
}

/// **The column a panel's `sections` are**, `width` wide: a plane that
/// flows them, framed and apart, and is as long as they are, so past the
/// window's foot it is reached by the wheel. `None` when there is no
/// section.
pub fn column(sections: Vec<Value>, width: f64) -> Option<Value> {
    if sections.is_empty() {
        return None;
    }
    Some(json!({
        "type": "plane",
        "flow": "col",
        "axis": "y",
        "zoom": false,
        "bars": true,
        "w": width,
        "margin": MARGIN,
        "pack": true,
        "children": sections,
    }))
}

/// **A panel of several `parts`, one section under `title`**: it folds
/// whole, and inside it each part is headed by its title and parted from
/// the one above by a line, as a menu's groups are. A part with no widget
/// is left out, and a panel of one part has no heading but its own; `None`
/// when no part has any, so a window composed without them has no panel.
pub fn parted(title: &str, parts: Vec<(&str, Vec<Value>)>, width: f64) -> Option<Value> {
    let parts: Vec<(&str, Vec<Value>)> = parts
        .into_iter()
        .filter(|(_, widgets)| !widgets.is_empty())
        .collect();
    let headed = parts.len() > 1;
    let mut children = Vec::new();
    for (i, (heading, widgets)) in parts.into_iter().enumerate() {
        if i > 0 {
            children.push(json!({"type": "separator"}));
        }
        if headed {
            children.push(json!({"type": "label", "text": heading, "muted": 1}));
        }
        children.extend(widgets);
    }
    if children.is_empty() {
        return None;
    }
    let mut section = section(title, false, children);
    section.insert("flow".into(), json!("col"));
    column(vec![Value::Object(section)], width)
}

/// Which side of the work a panel stands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// **`panel` beside `work`**, on `side`, with a divider a drag moves between
/// them: the work keeps the weight it had, and the pair takes the whole of it.
pub fn beside(work: Value, panel: Value, side: Side) -> Value {
    let children = match side {
        Side::Left => vec![panel, work],
        Side::Right => vec![work, panel],
    };
    json!({
        "type": "layout",
        "flow": "row",
        "margin": 0,
        "split": true,
        "weight": 1,
        "children": children,
    })
}

/// **The parts of a [`parted`] panel**, each heading with its widgets:
/// what a test reads it back as.
#[cfg(test)]
pub(crate) fn parts_of(panel: &Value) -> Vec<(String, Vec<Value>)> {
    let mut parts: Vec<(String, Vec<Value>)> = Vec::new();
    let children = panel["children"][0]["children"].as_array().cloned();
    for child in children.unwrap_or_default() {
        match child["type"].as_str() {
            Some("separator") => {}
            Some("label") if child["muted"] == 1 => {
                parts.push((child["text"].as_str().unwrap_or("").into(), Vec::new()));
            }
            _ => match parts.last_mut() {
                Some((_, widgets)) => widgets.push(child),
                None => parts.push((String::new(), vec![child])),
            },
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A panel of several parts is one section that folds whole**: each
    /// part headed and parted from the one above by a line, an empty part
    /// left out, and one part alone headed by the panel's title only.
    #[test]
    fn a_panel_of_parts_is_one_section() {
        let widget = || json!({"type": "toggle"});
        let panel = parted(
            "Spectrogram settings",
            vec![
                ("Analysis", vec![widget(), widget()]),
                ("Empty", Vec::new()),
                ("Display", vec![widget()]),
            ],
            200.0,
        )
        .unwrap();
        assert_eq!(panel["margin"], json!(MARGIN));
        let sections = panel["children"].as_array().unwrap();
        assert_eq!(sections.len(), 1, "one panel");
        let section = &sections[0];
        assert_eq!(section["title"], "Spectrogram settings");
        assert_eq!(section["frame"], true);
        assert_eq!(section["collapsed"], false);
        let kinds: Vec<(&str, &str)> = section["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["type"].as_str().unwrap(),
                    c["text"].as_str().unwrap_or(""),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("label", "Analysis"),
                ("toggle", ""),
                ("toggle", ""),
                ("separator", ""),
                ("label", "Display"),
                ("toggle", ""),
            ]
        );
        let one = parted("Inspector", vec![("Selection", vec![widget()])], 200.0).unwrap();
        assert_eq!(one["children"][0]["children"], json!([widget()]));
        assert!(parted("Inspector", vec![("Selection", Vec::new())], 200.0).is_none());
    }
}
