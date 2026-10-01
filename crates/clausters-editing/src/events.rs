//! **The OSC markers' reading of an event**: which events a roll draws as
//! markers rather than notes, and the label it draws each with.
//!
//! What a gesture over a roll means is [`crate::notes`]'s, by id. This is the
//! one question about an event that is not a note's: a raw OSC or MIDI message
//! has no pitch, so a roll draws it in the lane under the grid, labelled by its
//! address.

use serde_json::Value;

use crate::intake::text;

/// What the OSC markers send and take per marker: the time and the label.
pub const PAIR: usize = 2;

/// The label the roll's OSC markers draw an item with, or `None` when the item
/// is not one of them.
///
/// An OSC marker (an event of type `"osc"`) labels with its address, because
/// **a marker is the message it sends** and the address is the whole of what a
/// roll can show of one; a MIDI event labels with a short tag. The spelling a
/// document used before a raw message was an event -- an `"osc"` or `"midi"`
/// key of its own -- is still read.
pub fn label_of(data: &Value) -> Option<String> {
    let data = data.as_object()?;
    match data.get("type").and_then(Value::as_str) {
        Some("osc") => return Some(data.get("addr").map(text).unwrap_or_default()),
        Some("midi") => return Some("midi".to_string()),
        Some(_) => return None,
        None => {}
    }
    if let Some(addr) = data.get("osc") {
        return Some(text(addr));
    }
    data.contains_key("midi").then(|| "midi".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A marker is an event of type `"osc"`; a document written before that
    /// named it by a key of its own, and still reads.
    #[test]
    fn a_marker_labels_by_its_type_and_by_the_older_spelling() {
        assert_eq!(
            label_of(&json!({"type": "osc", "addr": "/a"})),
            Some("/a".into())
        );
        assert_eq!(
            label_of(&json!({"type": "midi", "midicmd": "cc"})),
            Some("midi".into())
        );
        assert_eq!(label_of(&json!({"osc": "/b"})), Some("/b".into()));
        assert_eq!(
            label_of(&json!({"midi": [144, 60, 1]})),
            Some("midi".into())
        );
        assert_eq!(label_of(&json!({"type": "note", "osc": "/c"})), None);
    }
}
