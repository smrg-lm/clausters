//! The compiler's own JSON, as Faust emits it beside a compiled binary.
//!
//! Only what a node needs is read: the DSP struct's byte size, the I/O arity
//! and the UI tree, whose leaves carry each parameter's byte offset inside the
//! struct.
//!
//! It is parsed **here**, in the shared crate, rather than in whichever host
//! happens to hold the JSON. Which UI elements become controls and in what
//! order is the same rule the libfaust backend applies while walking
//! `buildUserInterface` (`faust::synth`'s `collect_ui`): group structure
//! flattened, bare labels, first declaration wins, passive widgets ignored. One
//! rule with two implementations is how the same def comes to have different
//! control indices in a tab and in a window, and nothing would report it —
//! `/node_set "freq"` would simply set something else.
//!
//! Compiled on every target: the wasm backend is its only caller today, and it
//! is tested where the tests run.

use crate::faust::ParamSpec;

/// The top level of the compiler's JSON.
#[derive(serde::Deserialize)]
pub struct FaustJson {
    /// Byte size of one DSP struct.
    pub size: usize,
    pub inputs: usize,
    pub outputs: usize,
    #[serde(default)]
    pub ui: Vec<UiNode>,
}

/// One node of the UI tree: a group with items, or a widget.
#[derive(serde::Deserialize)]
pub struct UiNode {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub items: Vec<UiNode>,
    /// Byte offset of the zone inside the DSP struct. Absent on a group.
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub init: f32,
    #[serde(default)]
    pub min: f32,
    #[serde(default)]
    pub max: f32,
    #[serde(default)]
    pub step: f32,
}

impl FaustJson {
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("malformed Faust JSON: {e}"))
    }

    /// The parameters in declaration order, with their zone offsets.
    pub fn params(&self) -> (Vec<ParamSpec>, Vec<usize>) {
        let mut specs = Vec::new();
        let mut offsets = Vec::new();
        for node in &self.ui {
            walk(node, &mut specs, &mut offsets);
        }
        (specs, offsets)
    }
}

fn walk(node: &UiNode, specs: &mut Vec<ParamSpec>, offsets: &mut Vec<usize>) {
    match node.kind.as_str() {
        "vgroup" | "hgroup" | "tgroup" => {
            for item in &node.items {
                walk(item, specs, offsets);
            }
        }
        // A button and a checkbox declare no range in the JSON; both are 0/1,
        // which is what the libfaust walk's `add_button` gives them.
        "button" | "checkbox" => {
            if let Some(index) = node.index {
                specs.push(ParamSpec {
                    name: node.label.clone(),
                    init: 0.0,
                    min: 0.0,
                    max: 1.0,
                    step: 1.0,
                });
                offsets.push(index);
            }
        }
        "vslider" | "hslider" | "nentry" => {
            if let Some(index) = node.index {
                specs.push(ParamSpec {
                    name: node.label.clone(),
                    init: node.init,
                    min: node.min,
                    max: node.max,
                    step: node.step,
                });
                offsets.push(index);
            }
        }
        // A bargraph is passive and a soundfile is not a control: ignored,
        // exactly as the libfaust walk ignores them.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape libfaust emits, trimmed to the fields read here: nested
    /// groups, one of each widget kind, and a bargraph among them.
    const JSON: &str = r#"{
        "name": "probe", "size": 262192, "inputs": 1, "outputs": 2,
        "ui": [{"type": "vgroup", "label": "probe", "items": [
            {"type": "hslider", "label": "freq", "address": "/probe/freq",
             "index": 4, "init": 440, "min": 50, "max": 2000, "step": 0.5},
            {"type": "hgroup", "label": "inner", "items": [
                {"type": "button", "label": "gate", "index": 8},
                {"type": "hbargraph", "label": "level", "index": 12,
                 "min": 0, "max": 1},
                {"type": "nentry", "label": "chan", "index": 16,
                 "init": 1, "min": 0, "max": 7, "step": 1}]},
            {"type": "checkbox", "label": "bypass", "index": 20}]}]
    }"#;

    #[test]
    fn groups_flatten_and_passive_widgets_are_left_out() {
        let parsed = FaustJson::parse(JSON).unwrap();
        assert_eq!((parsed.size, parsed.inputs, parsed.outputs), (262192, 1, 2));
        let (specs, offsets) = parsed.params();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        // Declaration order, groups flattened, the bargraph absent.
        assert_eq!(names, ["freq", "gate", "chan", "bypass"]);
        assert_eq!(offsets, [4, 8, 16, 20]);
    }

    #[test]
    fn a_slider_keeps_its_range_and_a_button_is_zero_or_one() {
        let (specs, _) = FaustJson::parse(JSON).unwrap().params();
        let freq = &specs[0];
        assert_eq!(
            (freq.init, freq.min, freq.max, freq.step),
            (440.0, 50.0, 2000.0, 0.5)
        );
        let gate = &specs[1];
        assert_eq!(
            (gate.init, gate.min, gate.max, gate.step),
            (0.0, 0.0, 1.0, 1.0)
        );
    }

    #[test]
    fn a_def_with_no_controls_has_no_parameters() {
        let parsed = FaustJson::parse(r#"{"size": 16, "inputs": 0, "outputs": 1}"#).unwrap();
        let (specs, offsets) = parsed.params();
        assert!(specs.is_empty() && offsets.is_empty());
    }

    #[test]
    fn a_json_that_is_not_one_is_reported_rather_than_defaulted() {
        assert!(FaustJson::parse("not json at all").is_err());
        // `size` is not optional: a def whose struct has no size cannot be
        // allocated, and defaulting it to zero would allocate nothing.
        assert!(FaustJson::parse(r#"{"inputs": 0, "outputs": 1}"#).is_err());
    }
}
