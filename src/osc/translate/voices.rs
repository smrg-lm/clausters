//! `Voices` nodes: N voices of one def, built here and played on the audio
//! thread (`crate::node::voices`).

use super::*;
use crate::node::voices::{VoiceMap, VoiceSlot, VoicesNode};

/// Most voices one node holds.
pub const MAX_VOICES: usize = 256;

/// What a `Voices` node was built from: its def, how many controls the def
/// has (`notes` is the next), and how many voices.
pub struct VoicesDef {
    pub inner: NodeDef,
    pub controls: usize,
    pub count: usize,
}

impl VoicesDef {
    /// A control by name: the def's, or `notes`.
    pub fn control_index(&self, name: &str) -> Option<u32> {
        match name {
            "notes" => Some(self.controls as u32),
            name => self.inner.control_index(name),
        }
    }

    /// The voice map a def starts with: `freq`, `amp` and `gate` where the
    /// def has them, and a zone's pressure and timbre as `press` and `slide`
    /// -- the built-in `default`'s -- so a def that declares them plays all
    /// three dimensions with nothing mapped.
    pub fn default_map(&self) -> VoiceMap {
        let mut map = VoiceMap::EMPTY;
        let has = |name: &str| self.inner.control_index(name);
        map.freq = has("freq");
        map.amp = has("amp");
        map.gate = has("gate");
        map.pressure = has("press");
        map.timbre = has("slide");
        map
    }
}

impl CmdTranslator {
    /// `/voices_new def id addAction target voices [control value ...]`: a
    /// `Voices` node of `voices` voices of `def` (a SynthDef or a FaustDef),
    /// all built now. The controls are the def's, set on every voice, and
    /// `notes`, the event bus it reads.
    pub(super) fn voices_new(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [
            OscType::String(name),
            OscType::Int(id),
            OscType::Int(action),
            OscType::Int(target),
            OscType::Int(count),
            pairs @ ..,
        ] = msg.args.as_slice()
        else {
            return Err("expected: def, id, addAction, targetID, voices".into());
        };
        let count = usize::try_from(*count)
            .ok()
            .filter(|n| (1..=MAX_VOICES).contains(n))
            .ok_or_else(|| format!("a Voices node holds 1 to {MAX_VOICES} voices"))?;
        let action = AddAction::from_i32(*action).ok_or("add action must be 0-4")?;
        #[cfg(all(
            any(feature = "plugin-clap", feature = "plugin-vst3"),
            target_os = "linux"
        ))]
        if self.plugin_defs.contains_key(name) {
            return Err(format!(
                "{name}: a plugin plays its own voices -- make one node of it and route notes \
                 to it"
            ));
        }
        if self.graph_defs.contains_key(name) {
            return Err(format!(
                "{name}: a GraphDef's notes are its voice slots, not a Voices node's"
            ));
        }
        let (first, inner) = self.make_synth(name)?;
        match &inner {
            #[cfg(feature = "synth")]
            NodeDef::UGen(def) => {
                if let Some(u) = def
                    .ugens
                    .iter()
                    .find(|u| matches!(u.desc.name, "DiskIn" | "DiskOut" | "TransportDiskIn"))
                {
                    return Err(format!(
                        "{name}: a voice cannot restart a {} stream, so it cannot be a voice",
                        u.desc.name
                    ));
                }
            }
            #[cfg(feature = "faust")]
            NodeDef::Faust(_) => {}
            _ => {
                return Err(format!(
                    "{name}: a Voices node holds a SynthDef or a FaustDef"
                ));
            }
        }
        let defaults = inner.control_defaults();
        let spec = VoicesDef {
            controls: defaults.len(),
            inner,
            count,
        };
        let mut voices = vec![first];
        for _ in 1..count {
            voices.push(self.make_synth(name)?.0);
        }
        let map = spec.default_map();
        let mut node = VoicesNode::new(voices, defaults.clone(), map);
        let def = NodeDef::Voices(Arc::new(spec));
        let mut controls = def.control_defaults();
        for pair in pairs.chunks(2) {
            if let (Some(index), Some(value)) = (
                control_key(&pair[0], &def),
                pair.get(1).and_then(float_value),
            ) {
                node.set_control(index, value);
                if let Some(slot) = controls.get_mut(index as usize) {
                    *slot = value;
                }
            }
        }
        let id = if *id == -1 {
            self.alloc_auto_id()?
        } else if *id > 0 {
            *id
        } else {
            return Err("node ID must be positive or -1".into());
        };
        let (usage, bus_controls) = def.usage(&controls);
        let usage = usage.clamp_audio(self.audio_bus_count);
        self.node_defs.insert(id, def);
        cmds.push(Cmd::AddSynth {
            id,
            target: *target,
            action,
            synth: Box::new(node),
            usage: crate::dsp::StageMask::of(&usage),
        });
        let body = MirrorBody::Synth {
            def_name: name.clone(),
            controls,
            usage,
            bus_controls,
            maps: Vec::new(),
        };
        if let Ok(parent) = self.mirror.insert(id, body, *target, action) {
            self.resort_from(Some(parent), cmds);
        }
        Ok(())
    }

    /// `/voices_map node selector control`: what a note sets on its voice
    /// (`crate::node::voices::VoiceMap`). The selector is `note` (the
    /// frequency), `vel`, `gate`, `bend`, `pressure`, `poly`, `timbre`,
    /// `lift` or `ccN`; an empty control clears it.
    pub(super) fn voices_map(
        &mut self,
        msg: &rosc::OscMessage,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let [
            OscType::Int(id),
            OscType::String(selector),
            OscType::String(control),
        ] = msg.args.as_slice()
        else {
            return Err("expected: node, selector, control".into());
        };
        self.voice_map_cmd(*id, selector, control, cmds)
    }

    /// One voice-map entry of node `id`, checked: a `Voices` node, a known
    /// selector, a control of its def.
    pub(crate) fn voice_map_cmd(
        &self,
        id: i32,
        selector: &str,
        control: &str,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        let Some(NodeDef::Voices(spec)) = self.node_defs.get(&id) else {
            return Err(format!("node {id} is not a Voices node"));
        };
        let slot = VoiceSlot::parse(selector)
            .ok_or_else(|| format!("unknown voice selector {selector:?}"))?;
        let index = match control {
            "" => None,
            name => Some(
                spec.inner
                    .control_index(name)
                    .ok_or_else(|| format!("node {id} has no control {name:?}"))?,
            ),
        };
        cmds.push(Cmd::VoiceMap { id, slot, index });
        Ok(())
    }
}
