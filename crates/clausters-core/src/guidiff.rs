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
//! So a redraw is sent as a difference: one `/gui_set` per widget whose props
//! moved, and nothing freed or built. Where the **shape** did move, only the
//! smallest subtree that holds the change is redefined — `/gui_def` names any
//! widget, not only a window, so a clip appearing in one lane costs that lane
//! and leaves every other one exactly as the hand left it. Redefining the whole
//! window for it is the same failure one size up: an edit in one lane taking
//! the screen state of every other.
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
//! use clausters_core::guidiff::difference;
//! use serde_json::json;
//!
//! let was = json!({"type": "window", "children": [
//!     {"id": 10, "type": "knob", "value": 0.0}]});
//! let now = json!({"type": "window", "children": [
//!     {"id": 10, "type": "knob", "value": 0.5}]});
//!
//! let update = difference(&was, &now, 1);
//! assert!(!update.whole && update.redefine.is_empty());
//! assert_eq!(update.sets[0].0, 10);
//! ```

use serde_json::{Map, Value};

/// Keys that say what a node **is** rather than what it shows. A change in any
/// of them is a change of shape.
const STRUCTURE: [&str; 4] = ["type", "id", "name", "children"];

/// How to make a host draw the new picture.
///
/// The three fields are read together and in this order: send the whole tree if
/// `whole`; otherwise redefine each id in `redefine`, then apply `sets`. All
/// three empty means the two pictures are identical.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Update {
    /// The root itself has to go whole — its own shape moved, or the two
    /// documents are not comparable at all.
    pub whole: bool,
    /// Widgets whose **subtree** changed shape, in tree order and never nested
    /// inside one another: one `/gui_def` of that widget each, and nothing
    /// under it needs a set.
    pub redefine: Vec<i64>,
    /// One `/gui_set` per widget whose props moved, in tree order — only for
    /// widgets outside every redefined subtree, since a redefine already
    /// carries what those draw.
    pub sets: Vec<(i64, Map<String, Value>)>,
}

impl Update {
    /// The answer that sends everything: what an incomparable pair of documents
    /// gets, and what a caller falls back to.
    pub fn everything() -> Self {
        Self {
            whole: true,
            ..Self::default()
        }
    }

    /// Whether there is nothing at all to send.
    pub fn is_empty(&self) -> bool {
        !self.whole && self.redefine.is_empty() && self.sets.is_empty()
    }
}

/// What to send so a host drawing `old` draws `new` instead.
///
/// `root_id` is the id the root answers to — a `window`-rooted document carries
/// no id of its own, because it is the `/gui_def` argument.
pub fn difference(old: &Value, new: &Value, root_id: i64) -> Update {
    let (Some(old), Some(new)) = (old.as_object(), new.as_object()) else {
        return Update::everything();
    };
    let mut update = Update::default();
    if !walk(old, new, Some(root_id), &mut update) {
        return Update::everything();
    }
    update
}

/// One node and its children, collecting what moved.
///
/// `false` means **this node cannot be patched** and whoever called has to
/// redefine it — which its parent answers by putting this node's id in
/// `redefine`, or, at the root, by sending the whole tree.
fn walk(
    old: &Map<String, Value>,
    new: &Map<String, Value>,
    id: Option<i64>,
    update: &mut Update,
) -> bool {
    if old.get("type") != new.get("type") || old.get("name") != new.get("name") {
        return false;
    }
    let Some(id) = id else { return false };
    let Some(changed) = props(old, new) else {
        return false;
    };
    let old_kids = children(old);
    let new_kids = children(new);
    if old_kids.len() != new_kids.len() {
        // A widget appeared or went. There is no insert on the wire, so this
        // node goes whole — and its own props ride with it, which is why the
        // set collected above is dropped rather than sent.
        return false;
    }
    // Held until the children are known to be patchable: a set for a node that
    // turns out to need redefining is a message about a widget that is about to
    // be rebuilt.
    let mine = if changed.is_empty() {
        None
    } else {
        Some((id, changed))
    };
    let mut below = Update::default();
    for (was, is_now) in old_kids.iter().zip(new_kids.iter()) {
        let (Some(was), Some(is_now)) = (was.as_object(), is_now.as_object()) else {
            return false;
        };
        if was.get("id") != is_now.get("id") {
            return false;
        }
        match node_id(is_now) {
            Some(child) => {
                if !walk(was, is_now, Some(child), &mut below) {
                    // **The child goes whole, and this node does not.** That is
                    // the difference between an edit costing one lane and an
                    // edit costing the window.
                    below.redefine.push(child);
                }
            }
            // An id-less widget may stay, as long as it did not move: it cannot
            // be addressed, but a node identical in both pictures needs no
            // message and the picture around it is still a difference. One that
            // moved has no id to redefine either, so its parent is what goes.
            None => {
                if was != is_now {
                    return false;
                }
            }
        }
    }
    if let Some(mine) = mine {
        update.sets.push(mine);
    }
    update.sets.extend(below.sets);
    update.redefine.extend(below.redefine);
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

    /// A window of two lanes, each holding its own clips — the shape the
    /// multitrack draws, and the one the narrow redefine is for.
    fn stack(left: Vec<Value>, right: Vec<Value>) -> Value {
        json!({"type": "window", "children": [
            {"id": 20, "type": "field", "children": left},
            {"id": 21, "type": "field", "children": right}]})
    }

    fn clip(id: i64, offset: f64) -> Value {
        json!({"id": id, "type": "clip", "offset": offset})
    }

    #[test]
    fn the_same_picture_twice_sends_nothing() {
        assert!(difference(&tree(0.0, 0.0), &tree(0.0, 0.0), 1).is_empty());
    }

    #[test]
    fn one_prop_moving_is_one_set_on_the_widget_that_moved() {
        let out = difference(&tree(0.0, 0.0), &tree(0.0, 0.5), 1);
        assert_eq!(out.sets.len(), 1);
        assert_eq!(out.sets[0].0, 11);
        assert_eq!(out.sets[0].1.get("value"), Some(&json!(0.5)));
        assert!(out.redefine.is_empty() && !out.whole);
    }

    #[test]
    fn a_widget_that_appears_in_one_lane_costs_that_lane_and_no_other() {
        // The whole point. An edit in one lane used to redefine the window,
        // which took the screen state of every other lane with it -- the zoom,
        // the scroll, the selection -- for a clip that arrived somewhere else.
        let was = stack(vec![clip(30, 0.0)], vec![clip(40, 0.0)]);
        let now = stack(vec![clip(30, 0.0)], vec![clip(40, 0.0), clip(41, 4.0)]);
        let out = difference(&was, &now, 1);
        assert!(!out.whole, "the window stays");
        assert_eq!(out.redefine, vec![21], "only the lane that grew");
        assert!(out.sets.is_empty());
    }

    #[test]
    fn a_clip_moving_between_lanes_costs_both_lanes_and_no_other() {
        let was = stack(vec![clip(30, 0.0), clip(31, 4.0)], vec![clip(40, 0.0)]);
        let now = stack(vec![clip(30, 0.0)], vec![clip(40, 0.0), clip(31, 4.0)]);
        let out = difference(&was, &now, 1);
        assert!(!out.whole);
        assert_eq!(out.redefine, vec![20, 21], "both, in tree order");
    }

    #[test]
    fn a_lane_that_only_moved_a_clip_is_a_set_while_its_neighbour_is_redefined() {
        let was = stack(vec![clip(30, 0.0)], vec![clip(40, 0.0)]);
        let now = stack(vec![clip(30, 2.0)], vec![clip(40, 0.0), clip(41, 4.0)]);
        let out = difference(&was, &now, 1);
        assert_eq!(out.redefine, vec![21]);
        assert_eq!(out.sets.len(), 1);
        assert_eq!(out.sets[0].0, 30, "the clip that moved, not its lane");
    }

    #[test]
    fn nothing_under_a_redefined_widget_is_also_set() {
        // A redefine carries what its subtree draws, so a set for a widget
        // inside it is a message about something about to be rebuilt.
        let was = stack(vec![], vec![clip(40, 0.0)]);
        let now = stack(vec![], vec![clip(40, 9.0), clip(41, 4.0)]);
        let out = difference(&was, &now, 1);
        assert_eq!(out.redefine, vec![21]);
        assert!(out.sets.is_empty(), "40 moved, and 21 carries it");
    }

    #[test]
    fn a_prop_that_went_away_redefines_the_widget_that_held_it() {
        // There is no value on the wire that means "unset".
        let was = stack(vec![clip(30, 0.0)], vec![]);
        let mut now = stack(vec![clip(30, 0.0)], vec![]);
        now["children"][0]["children"][0]
            .as_object_mut()
            .unwrap()
            .remove("offset");
        let out = difference(&was, &now, 1);
        assert_eq!(out.redefine, vec![30]);
    }

    #[test]
    fn a_changed_type_or_name_redefines_that_widget() {
        let was = stack(vec![clip(30, 0.0)], vec![]);
        let mut retyped = was.clone();
        retyped["children"][0]["children"][0]["type"] = json!("knob");
        assert_eq!(difference(&was, &retyped, 1).redefine, vec![30]);

        let mut renamed = was.clone();
        renamed["children"][0]["children"][0]["name"] = json!("cutoff");
        assert_eq!(difference(&was, &renamed, 1).redefine, vec![30]);
    }

    #[test]
    fn the_root_growing_a_lane_is_the_one_case_that_costs_the_window() {
        let was = stack(vec![], vec![]);
        let mut now = stack(vec![], vec![]);
        now["children"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": 22, "type": "field", "children": []}));
        let out = difference(&was, &now, 1);
        assert!(out.whole, "the root has no parent to be redefined by");
    }

    #[test]
    fn moved_ids_redefine_the_parent() {
        // Against leased ids this was every redraw, which is why a widget id
        // has to name what it draws for any of this to be worth anything.
        let was = stack(vec![clip(30, 0.0)], vec![]);
        let mut moved = was.clone();
        moved["children"][0]["children"][0]["id"] = json!(99);
        assert_eq!(difference(&was, &moved, 1).redefine, vec![20]);
    }

    #[test]
    fn chrome_with_no_id_may_stay_as_long_as_it_did_not_move() {
        let with_ruler = |value: f64| {
            json!({"type": "window", "children": [
                {"id": 10, "type": "number", "value": value},
                {"type": "timeruler", "ruler": "beats"}]})
        };
        let out = difference(&with_ruler(0.0), &with_ruler(0.5), 1);
        assert_eq!(out.sets.len(), 1);
        assert_eq!(out.sets[0].0, 10);

        let mut moved = with_ruler(0.5);
        moved["children"][1]["ruler"] = json!("time");
        assert!(
            difference(&with_ruler(0.5), &moved, 1).whole,
            "it has no id to redefine, so its parent goes -- here, the root"
        );
    }

    #[test]
    fn the_root_is_addressed_by_the_id_it_was_defined_under() {
        let was = json!({"type": "window", "title": "one"});
        let now = json!({"type": "window", "title": "two"});
        let out = difference(&was, &now, 7);
        assert_eq!(out.sets.len(), 1);
        assert_eq!(out.sets[0].0, 7);
    }

    #[test]
    fn a_nested_widget_is_reached_through_its_parents() {
        let nest = |value: f64| {
            json!({"type": "window", "children": [
                {"id": 10, "type": "col", "children": [
                    {"id": 11, "type": "number", "value": value}]}]})
        };
        let out = difference(&nest(0.0), &nest(1.0), 1);
        assert_eq!(out.sets.len(), 1);
        assert_eq!(out.sets[0].0, 11);
    }

    #[test]
    fn anything_that_is_not_a_pair_of_objects_goes_whole() {
        assert!(difference(&json!([]), &json!({}), 1).whole);
        assert!(difference(&json!({}), &json!("window"), 1).whole);
    }
}
