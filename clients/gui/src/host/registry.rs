//! The widget registry: a window/widget tree keyed by client-allocated ids.
//!
//! It reuses the audio server's node-tree shape verbatim — integer ids the
//! client owns, a parent/children hierarchy, and **subtree freeing** (freeing a
//! widget frees its descendants, the way freeing a group frees its nodes). A
//! GuiDef is flattened into one record per widget on `/gui_def`; `/gui_set`
//! mutates a record's props, `/gui_free` removes a subtree, `/gui_query` reads
//! one back. No GPU yet — this is the bookkeeping the renderers (G3+) hang off.

use std::collections::HashMap;

use serde_json::{Map, Value};
use tracing::warn;

use super::guidef::GuiNode;

/// One registered widget: its type, live properties, and place in the tree.
#[derive(Debug)]
pub struct Widget {
    pub kind: String,
    pub props: Map<String, Value>,
    pub parent: Option<i32>,
    pub children: Vec<i32>,
}

/// The flat map of widgets by id. Roots (def roots) have `parent == None`.
#[derive(Debug, Default)]
pub struct Registry {
    widgets: HashMap<i32, Widget>,
}

/// What a `/gui_def` ended up doing, for the host log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DefineOutcome {
    /// Widgets actually inserted (the root plus accepted descendants).
    pub inserted: usize,
    /// Descendants dropped because their id was missing or already taken.
    pub skipped: usize,
    /// Whether a previous def under the same root id was replaced.
    pub replaced: bool,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Total widgets currently registered (across all defs).
    pub fn len(&self) -> usize {
        self.widgets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.widgets.is_empty()
    }

    pub fn contains(&self, id: i32) -> bool {
        self.widgets.contains_key(&id)
    }

    /// Reads one widget back (for `/gui_query` -> `/gui_info`).
    pub fn get(&self, id: i32) -> Option<&Widget> {
        self.widgets.get(&id)
    }

    /// The def-root id of the subtree containing `id` (walking parents), or
    /// `None` if `id` is unknown. Used to find which window a `/gui_set` lands in.
    pub fn root_of(&self, id: i32) -> Option<i32> {
        let mut cur = id;
        loop {
            let widget = self.widgets.get(&cur)?;
            match widget.parent {
                Some(parent) => cur = parent,
                None => return Some(cur),
            }
        }
    }

    /// Installs a GuiDef rooted at `root_id`. If that id already names a def it
    /// is freed first (redefinition, like re-sending a `SynthDef`). Descendants
    /// without an id, or whose id is already taken, are skipped with a warning
    /// — the tree is otherwise installed.
    pub fn define(&mut self, root_id: i32, node: &GuiNode) -> DefineOutcome {
        let replaced = self.contains(root_id);
        if replaced {
            self.free(root_id);
        }
        let mut outcome = DefineOutcome {
            replaced,
            ..Default::default()
        };
        self.insert(root_id, None, node, &mut outcome);
        outcome
    }

    fn insert(
        &mut self,
        id: i32,
        parent: Option<i32>,
        node: &GuiNode,
        outcome: &mut DefineOutcome,
    ) {
        if self.widgets.contains_key(&id) {
            warn!("gui_def: widget id {id} already in use, skipping (and its subtree)");
            outcome.skipped += 1;
            return;
        }
        let mut child_ids = Vec::with_capacity(node.children.len());
        // Insert this node first so children can link to it as their parent.
        self.widgets.insert(
            id,
            Widget {
                kind: node.kind.clone(),
                props: node.props.clone(),
                parent,
                children: Vec::new(),
            },
        );
        outcome.inserted += 1;
        for child in &node.children {
            match child.id {
                Some(child_id) => {
                    let before = self.widgets.len();
                    self.insert(child_id, Some(id), child, outcome);
                    // Only record the edge if the child was actually inserted.
                    if self.widgets.len() > before {
                        child_ids.push(child_id);
                    }
                }
                None => {
                    warn!("gui_def: child of widget {id} has no id, skipping it");
                    outcome.skipped += 1;
                }
            }
        }
        // Re-borrow to attach the accepted children (the recursion held a
        // mutable borrow of the map, so we set this after it returns).
        if let Some(w) = self.widgets.get_mut(&id) {
            w.children = child_ids;
        }
    }

    /// Frees a widget and its whole subtree, returning the number of widgets
    /// removed (0 if `id` is unknown). Unlinks `id` from its parent's children.
    pub fn free(&mut self, id: i32) -> usize {
        if !self.widgets.contains_key(&id) {
            return 0;
        }
        // Detach from the parent so a freed non-root child leaves no dangling
        // edge behind.
        if let Some(parent) = self.widgets[&id].parent
            && let Some(p) = self.widgets.get_mut(&parent)
        {
            p.children.retain(|&c| c != id);
        }
        self.remove_subtree(id)
    }

    fn remove_subtree(&mut self, id: i32) -> usize {
        let Some(widget) = self.widgets.remove(&id) else {
            return 0;
        };
        let mut removed = 1;
        for child in widget.children {
            removed += self.remove_subtree(child);
        }
        removed
    }

    /// Updates a live widget's properties (the `/gui_set` payload). Returns
    /// false if `id` is unknown.
    pub fn set<I, K>(&mut self, id: i32, props: I) -> bool
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        let Some(widget) = self.widgets.get_mut(&id) else {
            return false;
        };
        for (k, v) in props {
            widget.props.insert(k.into(), v);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> GuiNode {
        let json = r#"{
            "type": "window", "title": "Filter",
            "children": [
                {"id": 10, "type": "knob",  "value": 800.0},
                {"id": 11, "type": "panel", "children": [
                    {"id": 20, "type": "slider", "value": 0.2}
                ]}
            ]
        }"#;
        GuiNode::parse(json.as_bytes()).unwrap()
    }

    #[test]
    fn define_flattens_the_tree_under_the_root_id() {
        let mut reg = Registry::new();
        let outcome = reg.define(1, &tree());
        assert_eq!(outcome.inserted, 4);
        assert_eq!(outcome.skipped, 0);
        assert!(!outcome.replaced);
        assert_eq!(reg.len(), 4);
        assert_eq!(reg.get(1).unwrap().kind, "window");
        assert_eq!(reg.get(1).unwrap().children, vec![10, 11]);
        assert_eq!(reg.get(20).unwrap().parent, Some(11));
    }

    #[test]
    fn free_removes_a_subtree_and_unlinks_the_parent() {
        let mut reg = Registry::new();
        reg.define(1, &tree());
        // Freeing the panel (11) takes its slider (20) with it.
        assert_eq!(reg.free(11), 2);
        assert!(!reg.contains(11));
        assert!(!reg.contains(20));
        assert_eq!(reg.get(1).unwrap().children, vec![10]);
        // Freeing the root takes the rest.
        assert_eq!(reg.free(1), 2);
        assert!(reg.is_empty());
    }

    #[test]
    fn redefining_a_root_replaces_the_old_def() {
        let mut reg = Registry::new();
        reg.define(1, &tree());
        let outcome = reg.define(1, &tree());
        assert!(outcome.replaced);
        assert_eq!(reg.len(), 4, "no leftover widgets from the first def");
    }

    #[test]
    fn duplicate_child_id_is_skipped() {
        let json = r#"{"type":"window","children":[
            {"id":10,"type":"knob"},
            {"id":10,"type":"slider"}
        ]}"#;
        let node = GuiNode::parse(json.as_bytes()).unwrap();
        let mut reg = Registry::new();
        let outcome = reg.define(1, &node);
        assert_eq!(outcome.inserted, 2); // root + first child
        assert_eq!(outcome.skipped, 1); // the duplicate
        assert_eq!(reg.get(10).unwrap().kind, "knob", "first wins");
        assert_eq!(reg.get(1).unwrap().children, vec![10]);
    }

    #[test]
    fn set_updates_props_only_for_known_ids() {
        let mut reg = Registry::new();
        reg.define(1, &tree());
        assert!(reg.set(10, [("value", Value::from(440.0))]));
        assert_eq!(reg.get(10).unwrap().props["value"], Value::from(440.0));
        assert!(!reg.set(999, [("value", Value::from(1.0))]));
    }
}
