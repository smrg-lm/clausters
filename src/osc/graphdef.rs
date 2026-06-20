//! GraphDef: persistent node-graph definitions ("programs") — M18.
//!
//! Where a SynthDef/FaustDef persists a *single* synthesis node, a GraphDef
//! persists a whole **configuration of nodes wired by buses**: an effect
//! chain, a mixer, a layered instrument. It is a network-thread /
//! translation-time abstraction only — instantiating one expands into the
//! primitives that already exist (a group, member `/s_new`s, `/n_map`
//! wiring), so the audio thread never learns the word "GraphDef" and
//! RT-safety is untouched.
//!
//! A GraphDef exposes a **named parameter surface**: ports that map to inner
//! member controls (with optional linear scaling). All external actuation
//! (`/n_set`, ...) targets the surface, *never* the private member node ids
//! — the same encapsulation a composite SynthDef would give. The instance's
//! internal buses are private to each instantiation, allocated from a
//! reserved range at the top of the bus space (away from client-owned buses,
//! the same idea as the reserved auto node-id range).
//!
//! The spec ([`GraphDefSpec`]) is the transparent source of truth, persisted
//! verbatim as `graphdefs/<name>.json` (M16). There is no compiled artifact
//! to cache — a GraphDef references other defs, which carry their own.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::dsp::{NUM_AUDIO_BUSES, NUM_CONTROL_BUSES};

/// Reserved private-bus ranges for GraphDef instances, at the top of the bus
/// space so they never collide with client-allocated buses. Documented in
/// `docs/schemas.md`.
pub const GRAPH_AUDIO_BUS_BASE: usize = NUM_AUDIO_BUSES - 32; // 96..128
pub const GRAPH_CONTROL_BUS_BASE: usize = NUM_CONTROL_BUSES - 128; // 896..1024

/// Rate of an internal GraphDef bus.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub enum BusRate {
    #[default]
    Audio,
    Control,
}

/// A bus internal to a GraphDef instance, private to each instantiation.
#[derive(Clone, Serialize, Deserialize)]
pub struct GraphBus {
    pub name: String,
    #[serde(default)]
    pub rate: BusRate,
    #[serde(default = "one")]
    pub channels: usize,
}

fn one() -> usize {
    1
}

/// A control value of a member: a literal `f32`, or the *name* of an internal
/// bus (resolved to its allocated first index at instantiation). The bus form
/// is how a member is wired — its bus-selecting control (`out`/`in` on a
/// Faust def, or whatever control feeds an `Out`/`In` UGen) is set to a
/// private bus, uniformly for SynthDef and FaustDef members.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Num(f32),
    Bus(String),
}

/// A member node: an instance of an existing SynthDef/FaustDef wired into the
/// graph. Members are listed in any order; the instance group is auto-sorted
/// (M12) so the execution order follows the bus connections.
#[derive(Clone, Serialize, Deserialize)]
pub struct GraphMember {
    /// SynthDef or FaustDef name (resolved at instantiation; both kinds are
    /// instantiated identically).
    pub def: String,
    /// Initial control values by name (literals or internal-bus references).
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    /// Extra control→bus maps applied as `/n_map` (control name → internal
    /// control-bus name), for controls fed continuously by a bus.
    #[serde(default)]
    pub maps: HashMap<String, String>,
}

/// One inner target of a surface port: a member's control, with optional
/// linear scaling applied to the incoming value (`mul`·x + `add`).
#[derive(Clone, Serialize, Deserialize)]
pub struct SurfaceTarget {
    /// Index into [`GraphDefSpec::members`].
    pub member: usize,
    pub control: String,
    #[serde(default = "one_f32")]
    pub mul: f32,
    #[serde(default)]
    pub add: f32,
}

fn one_f32() -> f32 {
    1.0
}

/// The persisted spec of a GraphDef.
#[derive(Clone, Serialize, Deserialize)]
pub struct GraphDefSpec {
    pub name: String,
    #[serde(default)]
    pub buses: Vec<GraphBus>,
    pub members: Vec<GraphMember>,
    /// Named parameter surface: port name → inner targets.
    #[serde(default)]
    pub surface: HashMap<String, Vec<SurfaceTarget>>,
    /// Initial surface-port values applied at instantiation (overridable by
    /// `/graph_new` args).
    #[serde(default)]
    pub defaults: HashMap<String, f32>,
}

impl GraphDefSpec {
    /// Structural validation done at load/`/d_graph` time (member def
    /// existence is checked later, at instantiation, so load order between
    /// defs and graphdefs does not matter). Checks that every internal-bus
    /// reference resolves and every surface target points at a real member.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("GraphDef needs a name".into());
        }
        let bus_names: std::collections::HashSet<&str> =
            self.buses.iter().map(|b| b.name.as_str()).collect();
        for (i, m) in self.members.iter().enumerate() {
            for v in m.controls.values() {
                if let ControlValue::Bus(name) = v
                    && name != "OUT"
                    && !bus_names.contains(name.as_str())
                {
                    return Err(format!("member {i}: unknown internal bus '{name}'"));
                }
            }
            for bus in m.maps.values() {
                if !bus_names.contains(bus.as_str()) {
                    return Err(format!("member {i}: unknown internal bus '{bus}'"));
                }
            }
        }
        for (port, targets) in &self.surface {
            for t in targets {
                if t.member >= self.members.len() {
                    return Err(format!(
                        "surface port '{port}': member {} out of range",
                        t.member
                    ));
                }
            }
        }
        for port in self.defaults.keys() {
            if !self.surface.contains_key(port) {
                return Err(format!("default for unknown surface port '{port}'"));
            }
        }
        Ok(())
    }
}

/// Allocates contiguous runs of buses from a fixed reserved range. A
/// `Vec<bool>` busy map: small (≤128 entries) and only touched on
/// instantiate/free on the network thread.
pub struct RangeAllocator {
    base: usize,
    used: Vec<bool>,
}

impl RangeAllocator {
    pub fn new(base: usize, len: usize) -> Self {
        Self {
            base,
            used: vec![false; len],
        }
    }

    /// Allocates `width` contiguous buses; returns the first index.
    pub fn alloc(&mut self, width: usize) -> Option<usize> {
        let w = width.max(1);
        let n = self.used.len();
        let mut i = 0;
        while i + w <= n {
            if self.used[i..i + w].iter().all(|&b| !b) {
                self.used[i..i + w].iter_mut().for_each(|b| *b = true);
                return Some(self.base + i);
            }
            i += 1;
        }
        None
    }

    /// Returns a previously allocated run to the pool.
    pub fn free(&mut self, first: usize, width: usize) {
        let start = first.saturating_sub(self.base);
        for b in self.used.iter_mut().skip(start).take(width.max(1)) {
            *b = false;
        }
    }
}

/// A live GraphDef instance: the group holding it, the private buses to
/// reclaim on free, and the resolved surface used to translate a `/n_set`
/// against the instance into concrete member control writes.
pub struct GraphInstance {
    /// Member node ids, in member order (parallel to `def.members`).
    pub members: Vec<i32>,
    /// Private audio buses `(first, width)` to free on teardown.
    pub audio_buses: Vec<(usize, usize)>,
    /// Private control buses `(first, width)` to free on teardown.
    pub control_buses: Vec<(usize, usize)>,
    /// Resolved surface: port → `(member node id, control index, mul, add)`.
    pub surface: HashMap<String, Vec<(i32, u32, f32, f32)>>,
}
