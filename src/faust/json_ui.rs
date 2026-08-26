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

/// One `soundfile` a def declares: the buffer its name asks for (`None` when
/// the name is not a number) and the byte offset of its `Soundfile*` field
/// inside the DSP struct.
pub struct SoundfileSlot {
    pub bufnum: Option<usize>,
    pub offset: usize,
}

/// Everything a node needs to know about a compiled def's DSP struct: how big
/// it is, what its parameters are and where each one's zone sits, its I/O
/// arity, and the soundfile fields the host has to fill.
pub struct DefLayout {
    pub struct_bytes: usize,
    pub params: Vec<ParamSpec>,
    /// Byte offset of each parameter's zone, aligned with `params`.
    pub offsets: Vec<usize>,
    pub num_inputs: usize,
    pub num_outputs: usize,
    pub soundfiles: Vec<SoundfileSlot>,
}

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
    /// A soundfile's `url`, the fallback for its buffer number when the label
    /// is not one — the same two places the libfaust walk looks.
    #[serde(default)]
    pub url: String,
}

impl FaustJson {
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("malformed Faust JSON: {e}"))
    }

    /// The parameters in declaration order, with their zone offsets.
    pub fn params(&self) -> (Vec<ParamSpec>, Vec<usize>) {
        let layout = self.layout();
        (layout.params, layout.offsets)
    }

    /// The whole reading of the UI tree: parameters in declaration order, and
    /// the `soundfile`s beside them.
    ///
    /// A soundfile is **not** a control — it takes no index in `params` and
    /// `/node_set` cannot reach it — so it is collected apart. Its buffer
    /// number comes from the label (`soundfile("3", 1)`) and falls back to the
    /// url, which is the order the libfaust walk's `add_soundfile` reads them
    /// in — and both are read the same plain way it reads them, so Faust's own
    /// `{'…'}` url spelling is not a number here any more than it is there. A
    /// name that is not a number binds nothing and the field gets a silent
    /// placeholder, on both backends.
    pub fn layout(&self) -> DefLayout {
        let mut params = Vec::new();
        let mut offsets = Vec::new();
        let mut soundfiles = Vec::new();
        for node in &self.ui {
            walk(node, &mut params, &mut offsets, &mut soundfiles);
        }
        DefLayout {
            struct_bytes: self.size,
            params,
            offsets,
            num_inputs: self.inputs,
            num_outputs: self.outputs,
            soundfiles,
        }
    }
}

fn walk(
    node: &UiNode,
    specs: &mut Vec<ParamSpec>,
    offsets: &mut Vec<usize>,
    soundfiles: &mut Vec<SoundfileSlot>,
) {
    match node.kind.as_str() {
        "vgroup" | "hgroup" | "tgroup" => {
            for item in &node.items {
                walk(item, specs, offsets, soundfiles);
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
        // A soundfile is not a control either, but it *is* a field the host
        // has to fill: it goes on its own list, keyed by the buffer its name
        // asks for.
        "soundfile" => {
            if let Some(offset) = node.index {
                let number = |s: &str| s.trim().parse::<usize>().ok();
                soundfiles.push(SoundfileSlot {
                    bufnum: number(&node.label).or_else(|| number(&node.url)),
                    offset,
                });
            }
        }
        // A bargraph is passive: ignored, exactly as the libfaust walk ignores
        // it.
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

    /// A def that declares two soundfiles, as the wasm backend emits them: the
    /// `Soundfile*` fields are moved to the front of the struct, they carry no
    /// range and they are **not** controls.
    const SOUNDFILE_JSON: &str = r#"{
        "name": "sf", "size": 128, "inputs": 0, "outputs": 1,
        "ui": [{"type": "vgroup", "label": "sf", "items": [
            {"type": "soundfile", "label": "3", "url": "{'3'}",
             "address": "/sf/3", "index": 0},
            {"type": "soundfile", "label": "12", "url": "{'12'}",
             "address": "/sf/12", "index": 4},
            {"type": "hslider", "label": "gain", "index": 8,
             "init": 1, "min": 0, "max": 2, "step": 0.01}]}]
    }"#;

    #[test]
    fn a_soundfile_is_read_apart_from_the_controls() {
        let layout = FaustJson::parse(SOUNDFILE_JSON).unwrap().layout();
        // One control, and its index is 0 -- a soundfile takes no place in the
        // parameter order, so `/node_set "gain"` is control 0 and not control 2.
        let names: Vec<&str> = layout.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["gain"]);
        assert_eq!(layout.offsets, [8]);
        let slots: Vec<(Option<usize>, usize)> = layout
            .soundfiles
            .iter()
            .map(|s| (s.bufnum, s.offset))
            .collect();
        // Both name their buffer in the label, which is where a
        // `soundfile("<bufnum>", n)` puts it.
        assert_eq!(slots, [(Some(3), 0), (Some(12), 4)]);
    }

    /// A name that is not a buffer number binds nothing — and that includes
    /// Faust's own `url` spelling, `{'kick.wav'}`, which is not a number
    /// either. The libfaust walk parses both the same plain way
    /// (`faust::synth`'s `add_soundfile`), and a page must not resolve a name a
    /// window would not: the field is still filled, with the silent
    /// placeholder, on both.
    #[test]
    fn a_soundfile_named_after_nothing_binds_nothing() {
        let json = r#"{"size": 8, "inputs": 0, "outputs": 1, "ui": [
            {"type": "soundfile", "label": "kick", "url": "{'kick.wav'}", "index": 0}]}"#;
        let layout = FaustJson::parse(json).unwrap().layout();
        assert_eq!(layout.soundfiles.len(), 1);
        assert!(layout.soundfiles[0].bufnum.is_none());
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
