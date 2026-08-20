//! Building a GuiDef tree **from Rust**, without writing a document against
//! yourself and parsing it back.
//!
//! The host's one door is [`Host::handle_packet`](crate::host::Host::handle_packet)
//! — OSC framing around JSON — which is right for a language client on the far
//! end of a socket and absurd for a program that *links* this crate: it would
//! serialize a document only so the parser beside it could take the string
//! apart again. This module is the other end. A [`Node`] is assembled with Rust
//! values and handed to [`Host::define`](crate::host::Host::define), which does
//! everything `/gui_def` does from the point where the JSON has already been
//! parsed — so the two paths meet at [`GuiNode`], the generic wire node, and
//! there is exactly one construction path below it.
//!
//! ```
//! use clausters_gui::{host::Host, tree};
//!
//! let mut host = Host::new();
//! host.define(
//!     1,
//!     tree::window()
//!         .prop("title", "Mixer")
//!         .child(tree::node("knob").id(2).prop("label", "amp").prop("max", 2.0))
//!         .child(tree::node("meter").id(3).prop("bus", 0)),
//! );
//! ```
//!
//! **It does not mirror the widget catalog, deliberately.** There is no
//! `knob()` returning a typed knob: a leaf is `node("knob")` with its props,
//! and a container is one of the four below. Three reasons, recorded so this is
//! not later "fixed" into a fourth catalog. The props of every widget are
//! already declared in three surfaces that a test holds together
//! (`docs/gui-props.md` against the host, the Python builder and the web
//! builder); a fourth would have to be declared there and would drift the day
//! nobody did. A registered element ([`crate::Element`]) has props no catalog
//! in this crate can know, and it is a first-class caller of this builder — so
//! the open door has to exist regardless, and once it exists a typed twin of it
//! checks spelling, not safety. And the client whose surface *is* the catalog is
//! the Python one, by the project's own reference-client rule.
//!
//! What is typed here is what a JSON string loses: a prop is a Rust value, an
//! `i32` id stays an integer and a `f64` stays continuous — the int/float
//! distinction the wire depends on, kept by construction rather than by
//! remembering to write `2.0` instead of `2`.

use serde_json::{Map, Value};

use crate::host::guidef::GuiNode;

/// A node under construction: the generic `{id, type, props, children}` of the
/// wire, built by chaining.
///
/// Every method takes and returns `self`, so a tree is one expression. It
/// converts into the [`GuiNode`] the JSON parser produces — the *same* type,
/// not a parallel one, which is what makes the two paths impossible to drift
/// apart.
#[derive(Debug, Clone)]
pub struct Node {
    inner: GuiNode,
}

/// Starts a node of the given wire type — the one door for every widget the
/// catalog has and every element a program registered.
pub fn node(kind: &str) -> Node {
    Node {
        inner: GuiNode {
            id: None,
            kind: kind.to_string(),
            children: Vec::new(),
            props: Map::new(),
        },
    }
}

/// A `window` root: the top-level document a front opens a window for.
pub fn window() -> Node {
    node("window")
}

/// A `layout` container: the arranging box (`row`, `col`, `grid` or `stack`
/// through its `flow` prop).
pub fn layout() -> Node {
    node("layout")
}

/// A `plane` container: the 2D workspace — a virtual content area seen through
/// a scrolling, zooming window.
pub fn plane() -> Node {
    node("plane")
}

/// A `field` container: the time strip — a lane, a clip or a bare ruler,
/// according to the props it carries.
pub fn field() -> Node {
    node("field")
}

impl Node {
    /// Sets the client-allocated widget id. The root's comes from
    /// [`Host::define`](crate::host::Host::define)'s argument instead, the way
    /// it comes from the `/gui_def` argument on the wire, so a root needs none.
    pub fn id(mut self, id: i32) -> Self {
        self.inner.id = Some(id);
        self
    }

    /// Sets one prop. The value is any Rust type `serde_json` converts —
    /// `&str`, `f64`, `i32`, `bool`, a `Vec` of them — and it keeps the type it
    /// was written with, which is the distinction the host reads props by.
    pub fn prop(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.inner.props.insert(key.to_string(), value.into());
        self
    }

    /// Appends a child.
    pub fn child(mut self, child: Node) -> Self {
        self.inner.children.push(child.inner);
        self
    }

    /// Appends every child of an iterator — the door for a tree built in a
    /// loop, which is most of what a program building one does.
    pub fn children(mut self, children: impl IntoIterator<Item = Node>) -> Self {
        self.inner
            .children
            .extend(children.into_iter().map(|c| c.inner));
        self
    }
}

impl From<Node> for GuiNode {
    fn from(n: Node) -> GuiNode {
        n.inner
    }
}

#[cfg(test)]
mod tests {
    //! **Parity is the suite**: for each tree, the builder's node and the
    //! parser's node are compared as documents. That is the whole promise —
    //! the two doors into the host produce the same thing — and it is cheap
    //! precisely because both produce a [`GuiNode`], so the comparison is over
    //! one type rather than across a translation.

    use super::*;

    /// The two nodes, as the documents they are. Serializing is the exact
    /// comparison here (`GuiNode` round-trips through serde by construction),
    /// and it prints a readable diff when it fails.
    fn same(built: Node, json: &str) {
        let built = serde_json::to_value(GuiNode::from(built)).unwrap();
        let parsed = serde_json::to_value(GuiNode::parse(json.as_bytes()).unwrap()).unwrap();
        assert_eq!(built, parsed);
    }

    #[test]
    fn a_leaf_with_its_props_matches_the_document() {
        same(
            node("knob").id(2).prop("label", "amp").prop("max", 2.0),
            r#"{"id":2,"type":"knob","label":"amp","max":2.0}"#,
        );
    }

    /// The int/float distinction is what a hand-written document gets wrong and
    /// what the host reads props by, so it is pinned rather than assumed: an
    /// `i32` stays an integer and an `f64` stays continuous through the
    /// builder, exactly as through the parser.
    fn number_kinds(v: &Value) -> (bool, bool) {
        (v.is_i64(), v.is_f64())
    }

    #[test]
    fn a_prop_keeps_the_number_kind_it_was_written_with() {
        let built = GuiNode::from(node("meter").prop("bus", 3).prop("min", 3.0));
        assert_eq!(number_kinds(&built.props["bus"]), (true, false));
        assert_eq!(number_kinds(&built.props["min"]), (false, true));

        let parsed = GuiNode::parse(br#"{"type":"meter","bus":3,"min":3.0}"#).unwrap();
        assert_eq!(
            number_kinds(&parsed.props["bus"]),
            number_kinds(&built.props["bus"])
        );
        assert_eq!(
            number_kinds(&parsed.props["min"]),
            number_kinds(&built.props["min"])
        );
    }

    #[test]
    fn the_containers_nest_the_way_the_document_does() {
        same(
            window()
                .prop("title", "Mixer")
                .child(
                    layout()
                        .id(2)
                        .prop("flow", "row")
                        .children((0..2).map(|k| node("knob").id(10 + k).prop("bus", k))),
                )
                .child(plane().id(3).child(field().id(4).prop("h", 20.0))),
            r#"{"type":"window","title":"Mixer","children":[
                {"id":2,"type":"layout","flow":"row","children":[
                    {"id":10,"type":"knob","bus":0},
                    {"id":11,"type":"knob","bus":1}]},
                {"id":3,"type":"plane","children":[
                    {"id":4,"type":"field","h":20.0}]}]}"#,
        );
    }

    /// A registered element is built through the same door as a built-in: the
    /// builder has no catalog to be missing from, which is the property that
    /// makes it usable for an element this crate has never heard of.
    #[test]
    fn a_registered_element_needs_nothing_added_here() {
        same(
            node("my_element").id(9).prop("whatever", vec![1.0, 2.0]),
            r#"{"id":9,"type":"my_element","whatever":[1.0,2.0]}"#,
        );
    }
}
