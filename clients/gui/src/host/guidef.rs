//! The GuiDef document: a widget tree carried as JSON inside one OSC argument.
//!
//! A GuiDef is the GUI analogue of a `SynthDef`/`GraphDef`. The whole
//! window/widget tree rides as JSON in a single `/gui_def` argument (a string
//! or a blob), exactly as a `SynthDef` rides `/def_send synth`, so JSON is the payload
//! and OSC is the framing. serde's number handling keeps integer ids `i64` and
//! continuous values `f64` distinct across the wire — the "flat primitives at
//! the boundary" rule the rest of the project relies on.
//!
//! The node type is deliberately **generic**: `{ id, type, <props…>, children }`.
//! The catalog of widget *types* (containers, controls, the heavy GPU views)
//! grows by adding a renderer/handler in later milestones, never by changing
//! this shape — the host parses, registers and introspects any tree without
//! knowing the concrete widget types yet (no GPU here).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One node of a GuiDef tree.
///
/// The root node's `id` is supplied out of band by the `/gui_def <id> …`
/// argument (mirroring how a `SynthDef`'s name is the `/def_send synth` argument, not a
/// field inside the graph), so it is optional here; every child carries its own
/// client-allocated `id` in the JSON. Everything that is not `id`/`type`/
/// `children` is captured verbatim into [`props`](Self::props), preserving the
/// int/float distinction.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GuiNode {
    /// The client-allocated widget id. Absent on the root (it comes from the
    /// `/gui_def` argument); present on every descendant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i32>,
    /// The widget type tag (`"window"`, `"knob"`, `"waveform"`, …). Opaque to
    /// the host at this milestone: stored and reported, not yet rendered.
    #[serde(rename = "type")]
    pub kind: String,
    /// Child widgets, forming the subtree freed together by `/gui_free`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<GuiNode>,
    /// Every other property (`label`, `min`, `max`, `value`, `buffer`, …),
    /// captured as-is so the int/float distinction survives.
    #[serde(flatten)]
    pub props: Map<String, Value>,
}

impl GuiNode {
    /// Parses a GuiDef tree from the `/gui_def` JSON argument (a UTF-8 string or
    /// a raw blob — both are accepted, like `/def_send synth`).
    pub fn parse(bytes: &[u8]) -> Result<GuiNode, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// A human-readable, indented dump of the tree for the host log. The root is
    /// labelled with `root_id` (its out-of-band id from the `/gui_def`
    /// argument); descendants show their own ids.
    pub fn dump(&self, root_id: i32) -> String {
        let mut out = String::new();
        self.dump_into(&mut out, Some(root_id), 0);
        out
    }

    fn dump_into(&self, out: &mut String, id_override: Option<i32>, depth: usize) {
        let id = id_override.or(self.id);
        let id_label = id.map_or_else(|| "?".to_string(), |i| i.to_string());
        let indent = "  ".repeat(depth);
        let _ = write!(out, "{indent}[{id_label}] {}", self.kind);
        // Scalar props inline, sorted (serde_json's Map is ordered), so the dump
        // is stable across runs.
        for (k, v) in &self.props {
            if let Some(scalar) = scalar_str(v) {
                let _ = write!(out, " {k}={scalar}");
            }
        }
        out.push('\n');
        for child in &self.children {
            child.dump_into(out, None, depth + 1);
        }
    }

    /// The total number of widgets in the tree (the root plus every descendant).
    pub fn widget_count(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(GuiNode::widget_count)
            .sum::<usize>()
    }
}

/// Renders a scalar JSON value for the log; `None` for objects/arrays/null,
/// which are structural rather than displayable properties.
fn scalar_str(v: &Value) -> Option<String> {
    match v {
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(format!("{s:?}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILTER: &str = r#"{
        "type": "window", "title": "Filter", "w": 480, "h": 240, "flow": "col",
        "children": [
            {"id": 10, "type": "knob",     "label": "cutoff", "min": 20.0, "max": 20000.0, "value": 800.0},
            {"id": 11, "type": "slider",   "label": "res",    "min": 0.0,  "max": 1.0,      "value": 0.2},
            {"id": 12, "type": "signal", "view": "trace", "buffer": 0}
        ]
    }"#;

    #[test]
    fn parses_tree_and_separates_props_from_structure() {
        let node = GuiNode::parse(FILTER.as_bytes()).unwrap();
        assert_eq!(node.kind, "window");
        assert!(node.id.is_none(), "the root id comes from the OSC argument");
        assert_eq!(node.children.len(), 3);
        assert_eq!(node.widget_count(), 4);
        // `children`/`type` never leak into props.
        assert!(!node.props.contains_key("children"));
        assert!(!node.props.contains_key("type"));
        assert_eq!(node.props.get("title").unwrap(), "Filter");
    }

    #[test]
    fn keeps_the_int_float_distinction() {
        let node = GuiNode::parse(FILTER.as_bytes()).unwrap();
        // `w` is written without a decimal point -> integer; `min` has one ->
        // float. serde_json keeps them apart, which the wire relies on.
        assert!(node.props["w"].is_i64());
        let knob = &node.children[0];
        assert!(knob.props["min"].is_f64());
        assert!(knob.props["max"].is_f64());
        // The waveform's buffer reference stays an integer (a server buffer no.).
        let waveform = &node.children[2];
        assert!(waveform.props["buffer"].is_i64());
    }

    #[test]
    fn dump_is_indented_and_uses_the_root_id() {
        let node = GuiNode::parse(FILTER.as_bytes()).unwrap();
        let dump = node.dump(1);
        assert!(dump.starts_with("[1] window"));
        assert!(dump.contains("\n  [10] knob"));
        assert!(dump.contains("value=800"));
    }
}
