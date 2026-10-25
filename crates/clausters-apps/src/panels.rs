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

/// **The column a panel's `sections` are**, `width` wide: a plane that flows
/// them and is as long as they are, so past the window's foot it is reached by
/// the wheel. `None` when there is no section.
pub fn column(sections: Vec<Value>, width: f64) -> Option<Value> {
    (!sections.is_empty()).then(|| {
        json!({
            "type": "plane",
            "flow": "col",
            "axis": "y",
            "zoom": false,
            "bars": true,
            "w": width,
            "margin": MARGIN,
            "pack": true,
            "children": sections,
        })
    })
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
