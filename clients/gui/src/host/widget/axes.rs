//! The **axis pair**: the chrome of a two-axis container, and how it reaches
//! the props the rest of the host reads.
//!
//! A ruler, a navigation window, a selection, a playhead and a value range
//! describe the **container's axes**, not each element drawn against them, so
//! the wire declares them nested under one `axes` key — not bare `x`/`y`,
//! which are already the free-placement props, and a container that is placed
//! *and* owns axes would have no way to say which it meant.
//!
//! Inside the host they are flat (`view_start`, `ruler_y`, `y_len`, …): one
//! prop per key, which is what [`EditorProps`](super::EditorProps) parses, what
//! a `/gui_set` addresses and what a `/gui_info` can answer — an OSC reply is
//! flat arguments, so a structural prop cannot be reported at all. So a pair is
//! **flattened at the door**, once, before anything records the node; under an
//! axis a property drops the axis marker, so `x.start` is `view_start` and
//! `y.unit` is `ruler_y`.

use serde_json::{Map, Value};

use super::GuiNode;

/// The key an axis pair rides under.
pub(crate) const AXES: &str = "axes";

/// Flattens every `axes` pair in a tree, in place — the pass a def makes on
/// the way in, before the registry records the node or the renderer reads it.
/// The node's `type` is untouched: only the chrome moves, so a `/gui_query`
/// still answers in the vocabulary the tree was written in.
pub(crate) fn flatten_tree(node: &mut GuiNode) {
    if let Some(Value::Object(axes)) = node.props.remove(AXES) {
        let mut flat = Map::new();
        flatten(&axes, &mut flat);
        for (key, value) in flat {
            node.props.entry(key).or_insert(value);
        }
    }
    for child in &mut node.children {
        flatten_tree(child);
    }
}

/// Flattens an `axes` pair into the props each axis is spelled as today,
/// without overwriting a flat prop the node also names — a node that says both
/// is mid-migration, and the spelling it is being migrated *from* is the one
/// its author most recently meant.
pub(crate) fn flatten(axes: &Map<String, Value>, out: &mut Map<String, Value>) {
    for (axis, table) in [("x", X_AXIS), ("y", Y_AXIS)] {
        let Some(keys) = axes.get(axis).and_then(Value::as_object) else {
            continue;
        };
        for (key, value) in keys {
            let Some((_, flat)) = table.iter().find(|(k, _)| k == key) else {
                tracing::debug!("axes.{axis}: no axis property {key:?}");
                continue;
            };
            out.entry(flat.to_string()).or_insert_with(|| value.clone());
        }
    }
}

/// The x axis' properties, and the prop each is spelled as today. `start` and
/// `len` are the axis' window; the rest already read as the axis' own.
const X_AXIS: &[(&str, &str)] = &[
    ("start", "view_start"),
    ("len", "view_len"),
    ("ruler", "ruler"),
    ("unit", "ruler"),
    ("tempo", "tempo"),
    ("tempo_map", "tempo_map"),
    ("beat_at", "beat_at"),
    ("quant", "quant"),
    ("autofit", "autofit"),
    ("sample_rate", "sample_rate"),
    ("link", "link"),
    ("sel_start", "sel_start"),
    ("sel_len", "sel_len"),
    ("playhead", "playhead"),
    ("playhead_at", "playhead_at"),
    ("playhead_loop_start", "playhead_loop_start"),
    ("playhead_loop_len", "playhead_loop_len"),
    ("markers", "markers"),
];

/// The y axis' properties. `min`/`max` are the value range five widgets carry
/// on themselves today; `ruler` is the `ruler_y` strip.
const Y_AXIS: &[(&str, &str)] = &[
    ("start", "y_start"),
    ("len", "y_len"),
    ("ruler", "ruler_y"),
    ("unit", "ruler_y"),
    ("min", "min"),
    ("max", "max"),
    ("bit_depth", "bit_depth"),
    ("sel_min", "sel_min"),
    ("sel_max", "sel_max"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::guidef::GuiNode;

    /// The chrome nests on the axis it belongs to and lands on the props the
    /// host reads, which is the half of the model that moved a prop rather
    /// than a name.
    #[test]
    fn an_axis_pair_flattens_onto_the_props_the_host_reads() {
        let mut node = GuiNode::parse(
            br#"{"type":"field","h":24.0,
                 "axes":{"x":{"unit":"beats","tempo":2.0,"start":100.0,"len":900.0},
                         "y":{"unit":"db","min":-1.0,"max":1.0}}}"#,
        )
        .unwrap();
        flatten_tree(&mut node);
        let props = &node.props;
        assert!(!props.contains_key(AXES), "the key itself is consumed");
        assert_eq!(props.get("ruler").and_then(Value::as_str), Some("beats"));
        assert_eq!(props.get("view_start").and_then(Value::as_f64), Some(100.0));
        assert_eq!(props.get("view_len").and_then(Value::as_f64), Some(900.0));
        assert_eq!(props.get("ruler_y").and_then(Value::as_str), Some("db"));
        assert_eq!(props.get("min").and_then(Value::as_f64), Some(-1.0));
    }

    /// The pass reaches the whole tree: a lane declares its axis and the clips
    /// on it declare their own.
    #[test]
    fn every_node_of_a_tree_is_flattened() {
        let mut node = GuiNode::parse(
            br#"{"type":"window","children":[
                 {"id":2,"type":"field","axes":{"x":{"link":7}},"children":[
                  {"id":3,"type":"field","dur":8.0,"axes":{"y":{"min":-1.0}}}]}]}"#,
        )
        .unwrap();
        flatten_tree(&mut node);
        let lane = &node.children[0];
        assert_eq!(lane.props.get("link").and_then(Value::as_i64), Some(7));
        assert_eq!(
            lane.children[0].props.get("min").and_then(Value::as_f64),
            Some(-1.0)
        );
    }

    /// A flat prop beside a pair wins: a node saying both is mid-edit, and the
    /// spelling the author last touched is the one they meant.
    #[test]
    fn a_flat_prop_beside_an_axis_pair_keeps_its_value() {
        let mut node = GuiNode::parse(
            br#"{"type":"field","h":24.0,"view_start":7.0,"axes":{"x":{"start":9.0}}}"#,
        )
        .unwrap();
        flatten_tree(&mut node);
        assert_eq!(
            node.props.get("view_start").and_then(Value::as_f64),
            Some(7.0)
        );
    }

    /// An axis property the host does not have is ignored, not an error: the
    /// wire is open by design, and an unknown key on an axis is no different
    /// from an unknown prop on a node.
    #[test]
    fn an_unknown_axis_property_is_ignored() {
        let mut node =
            GuiNode::parse(br#"{"type":"field","axes":{"x":{"nope":1.0,"start":2.0}}}"#).unwrap();
        flatten_tree(&mut node);
        assert!(!node.props.contains_key("nope"));
        assert_eq!(
            node.props.get("view_start").and_then(Value::as_f64),
            Some(2.0)
        );
    }
}
