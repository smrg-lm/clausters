//! **The dock's arrangement kept between runs**: the layout the dock
//! answers names its panes by id, and a file's window takes another id the
//! next time, so what is kept names each pane by its **role** -- `"post"`,
//! `"docs"`, `"file:<path>"` -- and is translated back to the ids of the
//! run that restores it.
//!
//! A layout is the dock's tree: a split `{"split", "shares", "parts"}` and
//! a group `{"tabs", "active"}`. A pane the other side has no name for (an
//! untitled file, a window of the session) is left out on the way, and the
//! tree is pruned of what that empties: a group with no tab goes, a split
//! with one part left is that part, and the shares of what stays are
//! scaled to sum to one again.

use serde_json::{Value, json};

/// `layout` with every pane renamed by `rename`, the panes it has no name
/// for left out; `None` when nothing is left.
pub fn rename(layout: &Value, rename: &dyn Fn(&Value) -> Option<Value>) -> Option<Value> {
    if let Some(tabs) = layout.get("tabs").and_then(Value::as_array) {
        let tabs: Vec<Value> = tabs.iter().filter_map(rename).collect();
        let first = tabs.first()?.clone();
        let active = layout
            .get("active")
            .and_then(rename)
            .filter(|a| tabs.contains(a))
            .unwrap_or(first);
        return Some(json!({"tabs": tabs, "active": active}));
    }
    let split = layout.get("split")?.clone();
    let parts = layout.get("parts")?.as_array()?;
    let shares = layout.get("shares").and_then(Value::as_array);
    let mut kept: Vec<(Value, f64)> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if let Some(part) = self::rename(part, rename) {
            let share = shares
                .and_then(|s| s.get(i))
                .and_then(Value::as_f64)
                .unwrap_or(1.0 / parts.len() as f64);
            kept.push((part, share.max(0.0)));
        }
    }
    match kept.len() {
        0 => None,
        1 => kept.pop().map(|(part, _)| part),
        n => {
            let total: f64 = kept.iter().map(|(_, s)| s).sum();
            let shares: Vec<f64> = kept
                .iter()
                .map(|(_, s)| {
                    if total > 0.0 {
                        s / total
                    } else {
                        1.0 / n as f64
                    }
                })
                .collect();
            let parts: Vec<Value> = kept.into_iter().map(|(p, _)| p).collect();
            Some(json!({"split": split, "shares": shares, "parts": parts}))
        }
    }
}

/// Every pane `layout` names, in the order its groups list them.
pub fn panes(layout: &Value) -> Vec<Value> {
    if let Some(tabs) = layout.get("tabs").and_then(Value::as_array) {
        return tabs.clone();
    }
    layout
        .get("parts")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().flat_map(panes).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Value {
        json!({"split": "row", "shares": [0.6, 0.4], "parts": [
            {"tabs": [10, 11, 12], "active": 11},
            {"split": "col", "shares": [0.7, 0.3], "parts": [
                {"tabs": [2], "active": 2},
                {"tabs": [1, 99], "active": 99},
            ]},
        ]})
    }

    fn role(id: &Value) -> Option<Value> {
        match id.as_i64()? {
            1 => Some(json!("post")),
            2 => Some(json!("docs")),
            10 => Some(json!("file:/a.py")),
            11 => Some(json!("file:/b.py")),
            _ => None,
        }
    }

    /// The panes with a role keep their places and the others go: an
    /// untitled file's tab, a session's window, and the active tab falls to
    /// the group's first when it was one of them.
    #[test]
    fn panes_are_named_by_role() {
        let roles = rename(&layout(), &role).unwrap();
        assert_eq!(
            roles,
            json!({"split": "row", "shares": [0.6, 0.4], "parts": [
                {"tabs": ["file:/a.py", "file:/b.py"], "active": "file:/b.py"},
                {"split": "col", "shares": [0.7, 0.3], "parts": [
                    {"tabs": ["docs"], "active": "docs"},
                    {"tabs": ["post"], "active": "post"},
                ]},
            ]})
        );
        assert_eq!(panes(&roles).len(), 4);
    }

    /// A pane missing on the way back empties its group, which goes, and
    /// the split it leaves with one part is that part.
    #[test]
    fn what_is_missing_is_pruned() {
        let roles = rename(&layout(), &role).unwrap();
        let back = rename(&roles, &|r| match r.as_str()? {
            "post" => Some(json!(5)),
            "file:/a.py" => Some(json!(7)),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            back,
            json!({"split": "row", "shares": [0.6, 0.4], "parts": [
                {"tabs": [7], "active": 7},
                {"tabs": [5], "active": 5},
            ]})
        );
        let only = rename(&roles, &|r| (r.as_str() == Some("post")).then(|| json!(5))).unwrap();
        assert_eq!(only, json!({"tabs": [5], "active": 5}));
        assert!(rename(&roles, &|_| None).is_none());
    }

    /// The shares of what stays sum to one again.
    #[test]
    fn shares_are_scaled_again() {
        let three = json!({"split": "row", "shares": [0.5, 0.25, 0.25], "parts": [
            {"tabs": [1], "active": 1}, {"tabs": [2], "active": 2}, {"tabs": [3], "active": 3},
        ]});
        let two = rename(&three, &|id| (id.as_i64() != Some(1)).then(|| id.clone())).unwrap();
        assert_eq!(two["shares"], json!([0.5, 0.5]));
    }
}
