//! **A `nodetree` follows the server's tree**, in a window and in a page.
//!
//! The host asks the server for the tree of every group a `nodetree` of the
//! open windows shows, with `/group_queryTree group 1`, and keeps each answer
//! by group; a window showing a group draws again when its tree changed.
//! Both fronts ask on the read-back's beat ([`READ_BACK_MS`]), since a tree
//! is read back as a bound control is: a `/node_set` raises no notification,
//! so only asking again sees it. A native window also asks at once on a
//! `/node_start` or a `/node_end`.
//!
//! The rule is written once: which groups, what is asked, what an answer
//! does to the trees. A front only says when.
//!
//! [`READ_BACK_MS`]: crate::host::readback::READ_BACK_MS

use super::*;
use crate::host::graphics::nodetree::NodeTree;

/// Whether `widget`'s tree holds a `nodetree` view, which makes its window
/// ask for the server's tree.
pub fn shows_node_tree(widget: &Widget) -> bool {
    widget
        .descendants()
        .any(|w| !w.kind.needs().node_groups.is_empty())
}

/// Appends the distinct server groups every `nodetree` in `tree` mirrors.
fn collect_groups(tree: &Widget, out: &mut Vec<i32>) {
    for group in tree.descendants().flat_map(|w| w.kind.needs().node_groups) {
        if !out.contains(&group) {
            out.push(group);
        }
    }
}

impl Host {
    /// The distinct server groups the `nodetree`s of the windows in `open`
    /// (their def ids) mirror.
    pub fn tree_groups(&self, open: &[i32]) -> Vec<i32> {
        let mut groups = Vec::new();
        for id in open {
            if let Some(tree) = self.window_def(*id) {
                collect_groups(tree, &mut groups);
            }
        }
        groups
    }

    /// **What the windows in `open` ask the server for**: a
    /// `/group_queryTree group 1` per group their `nodetree`s show.
    pub fn tree_asks(&self, open: &[i32]) -> Vec<OscMessage> {
        self.tree_groups(open)
            .into_iter()
            .map(|group| OscMessage {
                addr: "/group_queryTree".into(),
                args: vec![OscType::Int(group), OscType::Int(1)],
            })
            .collect()
    }

    /// **Reads a `/group_queryTree.reply`** into the trees, answering the
    /// windows that show its group when it changed (none when it did not, so
    /// a tree asked for a few times a second draws nothing while it stands
    /// still). `None` for any other message, which a front reads on.
    pub fn read_tree(&mut self, msg: &OscMessage) -> Option<Vec<i32>> {
        if msg.addr != "/group_queryTree.reply" {
            return None;
        }
        let Some(tree) = NodeTree::parse(&msg.args) else {
            diag::warn!("malformed /group_queryTree.reply ({} args)", msg.args.len());
            return Some(Vec::new());
        };
        let group = tree.group;
        if self.node_trees.get(&group) == Some(&tree) {
            return Some(Vec::new());
        }
        self.node_trees.insert(group, tree);
        let showing = self
            .window_defs
            .keys()
            .copied()
            .filter(|id| self.tree_groups(&[*id]).contains(&group))
            .collect();
        Some(showing)
    }

    /// The trees last read, by group: what a frame's `World` draws a
    /// `nodetree` from.
    pub fn node_trees(&self) -> &HashMap<i32, NodeTree> {
        &self.node_trees
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_reply(group: i32, synth: i32) -> OscMessage {
        OscMessage {
            addr: "/group_queryTree.reply".into(),
            args: vec![
                OscType::Int(1),
                OscType::Int(group),
                OscType::Int(1),
                OscType::String(String::new()),
                OscType::Int(synth),
                OscType::Int(-1),
                OscType::String("sine".into()),
                OscType::Int(0),
            ],
        }
    }

    /// **A window with a `nodetree` asks for its group, and an answer that
    /// changed the tree draws that window again**; the same answer twice
    /// draws nothing, and a window without one asks nothing.
    #[test]
    fn a_node_tree_asks_for_its_group_and_redraws_when_it_changed() {
        let mut host = Host::new();
        let define = |host: &mut Host, id, tree| {
            host.define(id, serde_json::from_value::<GuiNode>(tree).unwrap());
        };
        define(
            &mut host,
            1,
            serde_json::json!({"type": "window", "children": [
                {"id": 2, "type": "nodes", "group": 0}]}),
        );
        define(
            &mut host,
            3,
            serde_json::json!({"type": "window", "children": [
                {"id": 4, "type": "label", "text": "none"}]}),
        );
        let asks = host.tree_asks(&[1, 3]);
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].addr, "/group_queryTree");
        assert_eq!(asks[0].args, vec![OscType::Int(0), OscType::Int(1)]);
        assert!(host.tree_asks(&[3]).is_empty());

        assert_eq!(host.read_tree(&tree_reply(0, 1000)), Some(vec![1]));
        assert_eq!(host.read_tree(&tree_reply(0, 1000)), Some(vec![]));
        assert_eq!(host.read_tree(&tree_reply(0, 1001)), Some(vec![1]));
        assert_eq!(host.node_trees()[&0].root[0].id, 1001);
        assert_eq!(
            host.read_tree(&OscMessage {
                addr: "/node_set".into(),
                args: vec![]
            }),
            None
        );
    }
}
