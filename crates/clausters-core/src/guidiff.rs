//! The difference between two pictures: what to send so a host draws the new
//! one without rebuilding it.
//!
//! A GuiDef is re-sent whenever a view redraws, and re-sending it whole is
//! expensive in a way that has nothing to do with bytes: `/gui_def` frees the
//! old subtree and builds a new one, so every widget's **screen state** goes
//! with it — a scroll position, a zoom, a selection in flight — and everything
//! the host had pending is dropped. Doing that because one number changed is
//! what makes a window flicker under a hand that is not even in it.
//!
//! So a redraw is sent as a difference when the two pictures have the same
//! **shape**: one `/gui_set` per widget whose props moved, and nothing freed or
//! built. Only a change of shape redefines.
//!
//! # What counts as shape
//!
//! Everything the wire cannot express as a set, and each of these is a
//! limitation of the protocol rather than a policy:
//!
//! - a widget that **appeared or went** — there is no insert or remove;
//! - a changed **type** or **name** — a name is what a handle resolves by, so
//!   moving one silently would leave a caller's handle pointing elsewhere;
//! - a **removed** prop — there is no value that means "unset";
//! - a widget with **no id** that is not identical in both pictures. An id-less
//!   node is stamped by the host inside the copy it sends, so the client does
//!   not know what number it got and cannot address it; one that did not move
//!   needs no message, and refusing it outright would make every tree holding a
//!   ruler or a spacer a redefine.
//!
//! Two pictures line up **by id**, which is only meaningful because a widget id
//! names what it draws ([`crate::widgetids`]) rather than being leased for one
//! draw. Against leased ids no two draws ever match and every redraw is a
//! redefine — which is exactly what this is here to stop.
//!
//! ```
//! use clausters_core::guidiff::{difference, Update};
//! use serde_json::json;
//!
//! let was = json!({"type": "window", "children": [
//!     {"id": 10, "type": "knob", "value": 0.0}]});
//! let now = json!({"type": "window", "children": [
//!     {"id": 10, "type": "knob", "value": 0.5}]});
//!
//! match difference(&was, &now, 1) {
//!     Update::Sets(sets) => assert_eq!(sets[0].0, 10),
//!     Update::Define => panic!("one prop moving is not a shape change"),
//! }
//! ```

use serde_json::{Map, Value};

/// Keys that say what a node **is** rather than what it shows. A change in any
/// of them is a change of shape.
const STRUCTURE: [&str; 4] = ["type", "id", "name", "children"];

/// How to make a host draw the new picture.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// Send the tree whole: the shape changed, and `/gui_set` cannot say so.
    Define,
    /// Send one `/gui_set` per entry, in tree order: the widget's id and the
    /// props that moved. Empty means the two pictures are identical and there
    /// is nothing to send at all.
    Sets(Vec<(i64, Map<String, Value>)>),
}

/// What to send so a host drawing `old` draws `new` instead.
///
/// `root_id` is the id the root answers to — a `window`-rooted document carries
/// no id of its own, because it is the `/gui_def` argument.
pub fn difference(old: &Value, new: &Value, root_id: i64) -> Update {
    let (Some(old), Some(new)) = (old.as_object(), new.as_object()) else {
        return Update::Define;
    };
    let mut sets = Vec::new();
    if walk(old, new, Some(root_id), &mut sets) {
        Update::Sets(sets)
    } else {
        Update::Define
    }
}

/// One node and its children, collecting what moved. `false` the moment
/// anything is not expressible as a set.
fn walk(
    old: &Map<String, Value>,
    new: &Map<String, Value>,
    id: Option<i64>,
    sets: &mut Vec<(i64, Map<String, Value>)>,
) -> bool {
    if old.get("type") != new.get("type") || old.get("name") != new.get("name") {
        return false;
    }
    let Some(id) = id else { return false };
    let Some(changed) = props(old, new) else {
        return false;
    };
    if !changed.is_empty() {
        sets.push((id, changed));
    }
    let old_kids = children(old);
    let new_kids = children(new);
    if old_kids.len() != new_kids.len() {
        return false;
    }
    for (was, is_now) in old_kids.iter().zip(new_kids.iter()) {
        let (Some(was), Some(is_now)) = (was.as_object(), is_now.as_object()) else {
            return false;
        };
        if was.get("id") != is_now.get("id") {
            return false;
        }
        match node_id(is_now) {
            Some(id) => {
                if !walk(was, is_now, Some(id), sets) {
                    return false;
                }
            }
            // An id-less widget may stay, as long as it did not move: it cannot
            // be addressed, but a node identical in both pictures needs no
            // message and the picture around it is still a difference.
            None => {
                if was != is_now {
                    return false;
                }
            }
        }
    }
    true
}

/// A node's children, or nothing when it has none.
fn children(node: &Map<String, Value>) -> &[Value] {
    match node.get("children") {
        Some(Value::Array(kids)) => kids,
        _ => &[],
    }
}

/// A node's own id, when it declares one this wire can carry.
fn node_id(node: &Map<String, Value>) -> Option<i64> {
    node.get("id")?.as_i64()
}

/// What of `new`'s props differ from `old`'s, or `None` when a prop was
/// **removed** — which a set cannot express.
fn props(old: &Map<String, Value>, new: &Map<String, Value>) -> Option<Map<String, Value>> {
    let gone = old
        .keys()
        .any(|key| !STRUCTURE.contains(&key.as_str()) && !new.contains_key(key));
    if gone {
        return None;
    }
    let mut changed = Map::new();
    for (key, is_now) in new {
        if STRUCTURE.contains(&key.as_str()) {
            continue;
        }
        if old.get(key) != Some(is_now) {
            changed.insert(key.clone(), is_now.clone());
        }
    }
    Some(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree(left: f64, right: f64) -> Value {
        json!({"type": "window", "children": [
            {"id": 10, "type": "number", "value": left},
            {"id": 11, "type": "number", "value": right}]})
    }

    fn sets(update: Update) -> Vec<(i64, Map<String, Value>)> {
        match update {
            Update::Sets(sets) => sets,
            Update::Define => panic!("expected a difference, got a redefine"),
        }
    }

    #[test]
    fn the_same_picture_twice_sends_nothing() {
        assert!(sets(difference(&tree(0.0, 0.0), &tree(0.0, 0.0), 1)).is_empty());
    }

    #[test]
    fn one_prop_moving_is_one_set_on_the_widget_that_moved() {
        let out = sets(difference(&tree(0.0, 0.0), &tree(0.0, 0.5), 1));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 11);
        assert_eq!(out[0].1.get("value"), Some(&json!(0.5)));
    }

    #[test]
    fn a_widget_that_was_not_there_can_only_arrive_whole() {
        let mut grown = tree(0.0, 0.0);
        grown["children"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": 12, "type": "number", "value": 1.0}));
        assert_eq!(difference(&tree(0.0, 0.0), &grown, 1), Update::Define);
    }

    #[test]
    fn a_widget_that_went_can_only_go_whole() {
        let mut shrunk = tree(0.0, 0.0);
        shrunk["children"].as_array_mut().unwrap().pop();
        assert_eq!(difference(&tree(0.0, 0.0), &shrunk, 1), Update::Define);
    }

    #[test]
    fn a_prop_that_went_away_redefines() {
        // There is no value on the wire that means "unset".
        let mut bare = tree(0.0, 0.0);
        bare["children"][0].as_object_mut().unwrap().remove("value");
        assert_eq!(difference(&tree(0.0, 0.0), &bare, 1), Update::Define);
    }

    #[test]
    fn a_changed_type_or_name_redefines() {
        let mut retyped = tree(0.0, 0.0);
        retyped["children"][0]["type"] = json!("knob");
        assert_eq!(difference(&tree(0.0, 0.0), &retyped, 1), Update::Define);

        let mut renamed = tree(0.0, 0.0);
        renamed["children"][0]["name"] = json!("cutoff");
        assert_eq!(difference(&tree(0.0, 0.0), &renamed, 1), Update::Define);
    }

    #[test]
    fn moved_ids_are_a_shape_change() {
        // Against leased ids this is every redraw, which is why a widget id has
        // to name what it draws for any of this to be worth anything.
        let mut moved = tree(0.0, 0.0);
        moved["children"][0]["id"] = json!(99);
        assert_eq!(difference(&tree(0.0, 0.0), &moved, 1), Update::Define);
    }

    #[test]
    fn chrome_with_no_id_may_stay_as_long_as_it_did_not_move() {
        let with_ruler = |value: f64| {
            json!({"type": "window", "children": [
                {"id": 10, "type": "number", "value": value},
                {"type": "timeruler", "ruler": "beats"}]})
        };
        let out = sets(difference(&with_ruler(0.0), &with_ruler(0.5), 1));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 10);

        let mut moved = with_ruler(0.5);
        moved["children"][1]["ruler"] = json!("time");
        assert_eq!(difference(&with_ruler(0.5), &moved, 1), Update::Define);
    }

    #[test]
    fn the_root_is_addressed_by_the_id_it_was_defined_under() {
        let was = json!({"type": "window", "title": "one"});
        let now = json!({"type": "window", "title": "two"});
        let out = sets(difference(&was, &now, 7));
        assert_eq!(
            out,
            vec![(7, {
                let mut props = Map::new();
                props.insert("title".into(), json!("two"));
                props
            })]
        );
    }

    #[test]
    fn a_nested_widget_is_reached_through_its_parents() {
        let nest = |value: f64| {
            json!({"type": "window", "children": [
                {"id": 10, "type": "col", "children": [
                    {"id": 11, "type": "number", "value": value}]}]})
        };
        let out = sets(difference(&nest(0.0), &nest(1.0), 1));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 11);
    }

    #[test]
    fn anything_that_is_not_a_pair_of_objects_redefines() {
        assert_eq!(difference(&json!([]), &json!({}), 1), Update::Define);
        assert_eq!(difference(&json!({}), &json!("window"), 1), Update::Define);
    }
}
