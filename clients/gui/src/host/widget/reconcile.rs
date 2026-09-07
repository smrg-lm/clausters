//! **A def says what to look like, not what to destroy.**
//!
//! A `/gui_def` over a tree the host is already drawing used to replace it
//! whole: every widget freed, every widget built again, and everything the host
//! itself had put on them gone with it. That is the branch's oldest complaint at
//! its root — *the zoom is not restored, it is never destroyed* — and it is why
//! a clip appearing in one lane took the zoom, the scroll and the selection of
//! every other lane in the window with it.
//!
//! So the host **reconciles**: it walks the tree it holds beside the tree it was
//! handed, matches widget to widget, and carries its own state across. This is
//! the field's standard answer (the DOM's reconciler, Qt's `DelegateModel`) and
//! it arrived here from a bug report rather than from a book.
//!
//! # What decides that two widgets are the same widget
//!
//! **Identity by id, order by index, both** — Tracktion's rule for finding a
//! clip, and the same one here. A widget with an id is matched by it, wherever
//! it moved to, which is what makes re-parenting a clip cheap: an id survives a
//! move and a path does not, and re-parenting is the multitrack's most common
//! gesture. A widget with **no** id — a clip's bodies, which the wire
//! deliberately does not address — is matched by position among its siblings of
//! the same kind.
//!
//! A match also requires the same **kind**, read as the wire spells it — the
//! registry already holds one type string per registered id, so the comparison
//! is the def's own word against the def's own word and needs nothing new
//! stored. An id reused for a different widget is a different widget, and
//! carrying a waveform's zoom onto a knob would be the "keeps too much" failure
//! this design was warned about.
//!
//! # What survives, and why it is exactly this
//!
//! The host's own state, and nothing else:
//!
//! - **The window on the axis** (`view_start`/`view_len`, `y_start`/`y_len`) —
//!   the zoom and the scroll, which a navigation gesture writes every frame and
//!   which nothing reports, because screen state is the host's.
//! - **The selection** (`sel_start`/`sel_len`, `sel_min`/`sel_max`, and the
//!   per-widget mark a marquee left) — what the hand is holding.
//! - **The active layer and what is hidden** — which of a container's layered
//!   contents the hand is on, and which of them are drawn.
//!
//! # And the def still wins where it says something
//!
//! Carrying the host's value *over* a value the def stated would take away the
//! one channel a script has for moving a view. So the rule is the wire's own —
//! **nothing said is nothing written**: a key the def states is the def's, a key
//! it leaves out keeps what the host had. A script that wants to move the window
//! says `view_start`; a script redrawing a lane says nothing about it and the
//! reader's window stands.
//!
//! That is the division this milestone exists to draw: **the client says what it
//! redrew; the host decides what that costs.**

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::super::guidef::GuiNode;
use super::{Widget, WidgetKind};

/// The prop keys a surviving widget keeps unless the def states them.
///
/// Every one is written by the host itself — a drag, a wheel, a marquee — and
/// reported by nothing, which is what makes them the host's to keep. The list is
/// short on purpose: keeping too much is worse than the defect it replaces.
const HOST_OWNED: [&str; 8] = [
    "view_start",
    "view_len",
    "y_start",
    "y_len",
    "sel_start",
    "sel_len",
    "sel_min",
    "sel_max",
];

/// What the def said about each widget it named: the props it stated, and the
/// type it called it.
///
/// Built once per def rather than searched per widget: a reconcile is on the
/// path of every redraw, and a tree with a thousand clips would otherwise walk
/// the document a thousand times.
#[derive(Default)]
pub struct Said<'a> {
    props: HashMap<i32, &'a Map<String, Value>>,
    kinds: HashMap<i32, &'a str>,
}

impl<'a> Said<'a> {
    /// Reads a def.
    pub fn of(node: &'a GuiNode, root_id: i32) -> Self {
        let mut said = Self::default();
        said.collect(node, Some(root_id));
        said
    }

    fn collect(&mut self, node: &'a GuiNode, id: Option<i32>) {
        if let Some(id) = id {
            self.props.insert(id, &node.props);
            self.kinds.insert(id, node.kind.as_str());
        }
        for child in &node.children {
            self.collect(child, child.id);
        }
    }
}

/// Brings the tree the host holds up to the tree it was handed, keeping what is
/// the host's.
///
/// `held` is what is on screen, `fresh` the tree just built from `def`, and
/// `root_id` the def's own id (the root's, which its JSON does not carry).
/// `was` is the type each id was registered under **before** this def replaced
/// it — the registry's own record, read before it is overwritten, which is what
/// makes "the same kind" a comparison of the wire's word with the wire's word.
/// `fresh` is edited in place and is what the host then draws.
pub fn reconcile(
    held: &Widget,
    fresh: &mut Widget,
    def: &GuiNode,
    root_id: i32,
    was: &HashMap<i32, String>,
) {
    let said = Said::of(def, root_id);
    // Every widget the host holds, by id, so a match is by **identity** and not
    // by where the widget used to sit. That is the whole reason this is a
    // reconcile and not an addressing scheme: a clip dragged to another lane is
    // the same clip, and a path to it went stale exactly when it moved.
    let mut index = HashMap::new();
    index_by_id(held, &mut index);
    let empty = Map::new();
    let top = said.props.get(&root_id).copied().unwrap_or(&empty);
    carry(held, fresh, &said, was, &index, top);
}

fn index_by_id<'a>(widget: &'a Widget, into: &mut HashMap<i32, &'a Widget>) {
    if let Some(id) = widget.id {
        into.insert(id, widget);
    }
    for child in &widget.children {
        index_by_id(child, into);
    }
}

fn carry<'a>(
    held: &Widget,
    fresh: &mut Widget,
    said: &Said<'a>,
    was: &HashMap<i32, String>,
    index: &HashMap<i32, &Widget>,
    inherited: &'a Map<String, Value>,
) {
    // A widget with no id of its own is a body, and what the wire said about it
    // was said on the container that holds it -- so the container's stated keys
    // are the ones that count here.
    let says = fresh
        .id
        .and_then(|id| said.props.get(&id).copied())
        .unwrap_or(inherited);

    if let (Some(was), Some(now)) = (held.kind.editor(), fresh.kind.editor_mut()) {
        for key in HOST_OWNED {
            if says.contains_key(key) {
                continue;
            }
            match key {
                "view_start" => now.x_start = was.x_start,
                "view_len" => now.x_len = was.x_len,
                "y_start" => now.y_start = was.y_start,
                "y_len" => now.y_len = was.y_len,
                "sel_start" => now.sel_start = was.sel_start,
                "sel_len" => now.sel_len = was.sel_len,
                "sel_min" => now.sel_min = was.sel_min,
                "sel_max" => now.sel_max = was.sel_max,
                _ => {}
            }
        }
    }
    // Nothing on the wire sets the mark a marquee left, so there is no "the def
    // said so" case for it: it is the host's, always.
    fresh.selected = held.selected;
    if !says.contains_key("layer") {
        fresh.layer = held.layer;
    }
    if !says.contains_key("hidden") {
        fresh.visible = held.visible;
    }

    // The children: by id where there is one, by position among the same kind
    // where there is not.
    let mut taken = vec![false; held.children.len()];
    for child in fresh.children.iter_mut() {
        match child.id {
            // By id, **anywhere** in the tree the host holds.
            Some(id) => {
                if let Some(before) = index.get(&id)
                    && same_kind(said, was, id, &before.kind, &child.kind)
                {
                    carry(before, child, said, was, index, says);
                }
            }
            // A body, which the wire does not address: the first child of its
            // kind, among this parent's, that nothing has claimed. Order is the
            // only key it has, and it has no reason to move between parents.
            None => {
                let at = held.children.iter().position(|c| {
                    c.id.is_none()
                        && std::mem::discriminant(&c.kind) == std::mem::discriminant(&child.kind)
                });
                if let Some(at) = at
                    && !taken[at]
                {
                    taken[at] = true;
                    carry(&held.children[at], child, said, was, index, says);
                }
            }
        }
    }
}

/// Whether the widget this id named before and the one it names now are the
/// same **kind** of widget.
///
/// The wire's word against the wire's word where both are known, since that is
/// the only comparison that separates two elements the typed tree spells the
/// same way. Where the registry has no record — an id it never held — the typed
/// variant is the honest fallback.
fn same_kind(
    said: &Said<'_>,
    was: &HashMap<i32, String>,
    id: i32,
    held: &WidgetKind,
    fresh: &WidgetKind,
) -> bool {
    match (was.get(&id), said.kinds.get(&id)) {
        (Some(before), Some(now)) => before == now,
        _ => std::mem::discriminant(held) == std::mem::discriminant(fresh),
    }
}

#[cfg(test)]
mod tests;
