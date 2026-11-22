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

use serde_json::{Map, Value, json};

/// The room around the sections in their column, which is also where the
/// column's scroll bar runs: wide enough that the bar covers no section.
pub const MARGIN: f64 = 10.0;

/// **How a column's sections stand**: a style, chosen by the application.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Spacing {
    /// Apart: each section framed, with room between them and around the
    /// column, as the score editor's palettes stand. What every column was
    /// before there was a choice, and the default.
    #[default]
    Spaced,
    /// Together, as a dock's panels in a column: no frame, no room between
    /// them, each section's bar the line that parts it from the one above,
    /// so the column reads as one panel of several sections.
    Joined,
}

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

/// **The column a panel's `sections` are**, `width` wide and standing as
/// `spacing` says: a plane that flows them and is as long as they are, so
/// past the window's foot it is reached by the wheel. `None` when there is
/// no section.
pub fn column(sections: Vec<Value>, width: f64, spacing: Spacing) -> Option<Value> {
    if sections.is_empty() {
        return None;
    }
    let mut column = json!({
        "type": "plane",
        "flow": "col",
        "axis": "y",
        "zoom": false,
        "bars": true,
        "w": width,
        "margin": MARGIN,
        "pack": true,
        "children": sections,
    });
    if spacing == Spacing::Joined {
        column["margin"] = json!(0);
        column["gap"] = json!(0);
        if let Some(sections) = column["children"].as_array_mut() {
            for section in sections {
                section["frame"] = json!(false);
            }
        }
    }
    Some(column)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Vec<Value> {
        ["Analysis", "Display"]
            .into_iter()
            .map(|t| Value::Object(section(t, false, vec![json!({"type": "label"})])))
            .collect()
    }

    /// **The spacing is a style, and spaced is what a column always was**:
    /// framed sections apart in a margin; joined takes the frames and the
    /// room away and changes nothing else.
    #[test]
    fn a_column_stands_spaced_or_joined() {
        let spaced = column(two(), 200.0, Spacing::default()).unwrap();
        assert_eq!(spaced["margin"], json!(MARGIN));
        assert!(spaced.get("gap").is_none(), "the plane's own gap");
        assert!(
            spaced["children"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["frame"] == json!(true))
        );
        let joined = column(two(), 200.0, Spacing::Joined).unwrap();
        assert_eq!(
            (joined["margin"].clone(), joined["gap"].clone()),
            (json!(0), json!(0))
        );
        assert!(
            joined["children"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["frame"] == json!(false))
        );
        let mut same = joined.clone();
        same["margin"] = json!(MARGIN);
        same.as_object_mut().unwrap().remove("gap");
        for s in same["children"].as_array_mut().unwrap() {
            s["frame"] = json!(true);
        }
        assert_eq!(same, spaced, "nothing else differs");
        assert!(column(Vec::new(), 200.0, Spacing::Joined).is_none());
    }
}
