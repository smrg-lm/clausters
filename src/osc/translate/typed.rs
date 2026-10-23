//! **The translator's typed doors**: the node and def commands with Rust
//! arguments, for a program that holds its engine and builds its commands
//! itself.
//!
//! An editing application embeds an engine and speaks to it with built
//! [`Cmd`]s, never OSC. Building one -- a synth from its compiled def, a
//! group, a graph's members and private buses, the tree mirror and the bus
//! order kept right -- is this translator's work, so these doors are that
//! work taken as a library; the OSC commands decode their arguments and call
//! them, and there is one implementation of each.

use rosc::OscType;

use super::{CmdTranslator, NodeDef};
use crate::node::AddAction;
use crate::osc::graph::MirrorBody;
use crate::server::engine::Cmd;

/// **A control a typed door writes**: by its index in the def, or by its
/// name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control<'a> {
    /// The control at this index.
    Index(u32),
    /// The control of this name, or the surface port of a graph.
    Name(&'a str),
}

impl<'a> Control<'a> {
    /// The control an OSC argument names: a string is a name, a non-negative
    /// int an index.
    pub fn of(arg: &'a OscType) -> Option<Self> {
        match arg {
            OscType::String(name) => Some(Control::Name(name)),
            OscType::Int(i) if *i >= 0 => Some(Control::Index(*i as u32)),
            _ => None,
        }
    }

    /// Its index in `def`, when `def` has it.
    fn index(&self, def: &NodeDef) -> Option<u32> {
        match self {
            Control::Index(i) => Some(*i),
            Control::Name(name) => def.control_index(name),
        }
    }
}

impl CmdTranslator {
    /// **A synth of def `name` as node `id`** (`-1` for one of the auto
    /// range), `action` relative to `target`, with `controls` set over the
    /// def's defaults: `/synth_new`, typed. Answers the node's id.
    pub fn new_synth(
        &mut self,
        name: &str,
        id: i32,
        action: AddAction,
        target: i32,
        controls: &[(Control, f32)],
        cmds: &mut Vec<Cmd>,
    ) -> Result<i32, String> {
        let (mut synth, def) = self.make_synth(name)?;
        let id = if id == -1 {
            self.alloc_auto_id()?
        } else if id > 0 {
            id
        } else {
            return Err("node ID must be positive or -1".into());
        };
        let mut values = def.control_defaults();
        for (control, value) in controls {
            if let Some(index) = control.index(&def) {
                synth.set_control(index, *value);
                if let Some(slot) = values.get_mut(index as usize) {
                    *slot = *value;
                }
            }
        }
        let (usage, bus_controls) = def.usage(&values);
        let usage = usage.clamp_audio(self.audio_bus_count);
        self.node_defs.insert(id, def);
        cmds.push(Cmd::AddSynth {
            id,
            target,
            action,
            synth,
            usage: crate::dsp::StageMask::of(&usage),
        });
        let body = MirrorBody::Synth {
            def_name: name.to_string(),
            controls: values,
            usage,
            bus_controls,
            maps: Vec::new(),
        };
        if let Ok(parent) = self.mirror.insert(id, body, target, action) {
            self.resort_from(Some(parent), cmds);
        }
        Ok(id)
    }

    /// **A group as node `id`**, `action` relative to `target`, born with
    /// `name` when one is given: `/group_new`, typed. A name the tree refuses
    /// refuses the group.
    pub fn add_group(
        &mut self,
        id: i32,
        action: AddAction,
        target: i32,
        name: Option<&str>,
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        if id <= 0 {
            return Err("group ID must be positive".into());
        }
        // The label is judged **before** the group exists: a refused name
        // refuses the creation, rather than leaving the caller with an
        // anonymous group it never asked for.
        if let Some(name) = name {
            self.mirror.check_new_name(target, action, name)?;
        }
        cmds.push(Cmd::AddGroup {
            id,
            target,
            action,
            group: self.new_group(),
        });
        // An empty group has no bus usage: no re-sort needed.
        let _ = self
            .mirror
            .insert(id, MirrorBody::group(false), target, action);
        if let Some(name) = name {
            self.mirror.set_name(id, name)?;
        }
        Ok(())
    }

    /// **Frees node `id`**, and what was under it: `/node_free`, typed.
    pub fn free_node(&mut self, id: i32, cmds: &mut Vec<Cmd>) {
        cmds.push(Cmd::FreeNode { id });
        // Removals keep any topological order valid: no re-sort.
        self.mirror.remove(id);
        // Freeing a GraphDef voice or instance group drops its translator
        // state and reclaims its private buses (a no-op for ordinary nodes).
        self.free_graph_node(id);
    }

    /// **Pauses (`run` false) or resumes node `id`**, its state kept:
    /// `/node_run`, typed.
    pub fn run_node(&mut self, id: i32, run: bool, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        if self.node_unknown(id) {
            return Err(format!("node {id} not found"));
        }
        cmds.push(Cmd::RunNode { id, run });
        Ok(())
    }

    /// **Writes `controls` onto node `id`**: a graph's surface ports by name,
    /// a group's subtree, or a synth's own controls: `/node_set`, typed.
    pub fn set_node(
        &mut self,
        id: i32,
        controls: &[(Control, f32)],
        cmds: &mut Vec<Cmd>,
    ) -> Result<(), String> {
        // A GraphDef instance resolves names against its named surface (never
        // the private member nodes); a plain group propagates to its
        // subtree; a synth sets itself.
        if self.is_graph(id) {
            for (control, value) in controls {
                if let Control::Name(port) = control {
                    self.apply_surface(id, port, *value, cmds);
                }
            }
            return Ok(());
        }
        self.for_each_target(id, cmds, |t, node, def, cmds| {
            let mut hit = false;
            for (control, value) in controls {
                if let Some(index) = control.index(def) {
                    hit |= t.write_control(node, index, *value, cmds);
                }
            }
            Ok(hit)
        })
    }
}
