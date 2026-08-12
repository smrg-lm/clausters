//! `nodes` — the audio server's node tree, mirrored.
//!
//! The second thing the world carries, and a different shape from a meter's:
//! not a number this frame but a **document somebody else queried**, plus the
//! one fact that tells an empty tree from an unreachable one. The host holds no
//! node tree of its own — the client leg queries the groups the trees declare
//! and files the replies in the world — so this element reads and never owns,
//! exactly as it does for a score or a set of samples.

use serde_json::{Map, Value};

use crate::host::graphics::nodetree;
use crate::host::paint::Draw;
use crate::host::widget::element::{Ctx, Element, Needs};
use crate::host::widget::parse;

/// A view of one server group's node tree. `controls` lists each node's
/// current control values under it.
#[derive(Debug, Clone)]
pub struct Nodes {
    pub group: i32,
    pub controls: bool,
    pub label: Option<String>,
}

pub(super) fn build(
    props: &Map<String, Value>,
    _blobs: &[Vec<u8>],
) -> Result<Box<dyn Element>, String> {
    Ok(Box::new(from_props(props)))
}

/// The props a `nodes` node carries, read once — shared by the constructor and
/// by the tests beside it.
fn from_props(props: &Map<String, Value>) -> Nodes {
    Nodes {
        group: parse::int_prop(props, "group", 0),
        controls: props
            .get("controls")
            .and_then(parse::truthy)
            .unwrap_or(true),
        label: parse::label(props),
    }
}

impl Element for Nodes {
    fn set(&mut self, key: &str, v: &Value) -> bool {
        match key {
            "group" => v.as_i64().map(|n| self.group = n as i32).is_some(),
            "controls" => parse::truthy(v).map(|b| self.controls = b).is_some(),
            "label" => parse::set_label(&mut self.label, v),
            _ => false,
        }
    }

    fn draw(&self, d: &mut Draw, ctx: &Ctx) {
        nodetree::draw(
            d,
            ctx.rect,
            ctx.world.node_tree(self.group),
            self.controls,
            self.label.as_deref(),
            ctx.world.server_attached,
        );
    }

    fn needs(&self) -> Needs {
        Needs {
            node_groups: vec![self.group],
            ..Default::default()
        }
    }

    fn clone_box(&self) -> Box<dyn Element> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(json: &str) -> Map<String, Value> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn props_parse_and_default() {
        let n = from_props(&props(r#"{"group":2,"controls":0,"label":"tree"}"#));
        assert_eq!((n.group, n.controls), (2, false));
        assert_eq!(n.label.as_deref(), Some("tree"));

        let n = from_props(&props("{}"));
        assert_eq!((n.group, n.controls), (0, true));
        assert_eq!(n.label, None);
    }

    #[test]
    fn a_set_lands_on_its_own_key_and_declines_the_rest() {
        let mut n = from_props(&props("{}"));
        assert!(n.set("group", &Value::from(5)));
        assert!(n.set("controls", &Value::from(0)));
        assert!(n.set("label", &Value::from("t")));
        assert_eq!((n.group, n.controls), (5, false));
        assert!(!n.set("nonesuch", &Value::from(1)));
    }

    /// The group is a **declaration**, not a draw-time lookup the host guesses
    /// at: it is what makes the client leg query that tree in the first place,
    /// and it follows a live `/gui_set` of `group`.
    #[test]
    fn the_group_it_draws_is_the_group_it_declares() {
        let mut n = from_props(&props(r#"{"group":2}"#));
        assert_eq!(n.needs().node_groups, vec![2]);
        assert!(n.set("group", &Value::from(9)));
        assert_eq!(n.needs().node_groups, vec![9]);
    }
}
