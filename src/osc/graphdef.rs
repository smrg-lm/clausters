//! GraphDef: persistent node-graph definitions ("programs").
//!
//! Where a SynthDef/FaustDef persists a *single* synthesis node, a GraphDef
//! persists a whole **configuration of nodes wired by buses**: an effect
//! chain, a mixer, a layered instrument. It is a network-thread /
//! translation-time abstraction only — instantiating one expands into the
//! primitives that already exist (a group, member `/synth_new`s, `/node_map`
//! wiring), so the audio thread never learns the word "GraphDef" and
//! RT-safety is untouched.
//!
//! A GraphDef exposes a **named parameter surface**: ports that map to inner
//! member controls (with optional linear scaling). All external actuation
//! (`/node_set`, ...) targets the surface, *never* the private member node ids
//! — the same encapsulation a composite SynthDef would give. The instance's
//! internal buses are private to each instantiation, allocated from a
//! reserved range at the top of the bus space (away from client-owned buses,
//! the same idea as the reserved auto node-id range).
//!
//! The spec ([`GraphDefSpec`]) is the transparent source of truth, persisted
//! verbatim as `defs/graphdefs/<name>.json`. There is no compiled artifact
//! to cache — a GraphDef references other defs, which carry their own.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Width of the reserved private-bus range for GraphDef instances, at the top
/// of each bus space so it never collides with client-allocated buses. The
/// base is `bus_count - reserved`, computed from the live counts in
/// `CmdTranslator::new` (so it tracks `--audio-buses`/`--control-buses`).
/// The constants live in `clausters_core::registry` — the shared resource
/// model — so client allocators subtract the same reservation they were built
/// against. Documented in `docs/schemas.md`.
pub use clausters_core::registry::{GRAPH_AUDIO_BUS_RESERVED, GRAPH_CONTROL_BUS_RESERVED};

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
/// so the execution order follows the bus connections.
#[derive(Clone, Serialize, Deserialize)]
pub struct GraphMember {
    /// SynthDef or FaustDef name (resolved at instantiation; both kinds are
    /// instantiated identically).
    pub def: String,
    /// Initial control values by name (literals or internal-bus references).
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    /// Extra control→bus maps applied as `/node_map` (control name → internal
    /// control-bus name), for controls fed continuously by a bus.
    #[serde(default)]
    pub maps: HashMap<String, String>,
    /// `true` = a **per-voice** member, instantiated once per `/graph_newVoice`
    /// (or per MIDI note) inside the instance, wired to the same private
    /// buses. `false` (default) = a **shared** member, instantiated once at
    /// `/graph_new` (the always-on part: buses, mixer, effects).
    #[serde(default)]
    pub voice: bool,
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
    /// Structural validation done at load/`/def_send graph` time (member def
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
            // A port maps either to shared members or to voice members, never
            // a mix: a shared port resolves at /graph_new, a voice port at
            // /graph_newVoice, so they cannot share one name.
            let any_voice = targets.iter().any(|t| self.members[t.member].voice);
            let any_shared = targets.iter().any(|t| !self.members[t.member].voice);
            if any_voice && any_shared {
                return Err(format!(
                    "surface port '{port}': mixes shared and per-voice members"
                ));
            }
        }
        for port in self.defaults.keys() {
            if !self.surface.contains_key(port) {
                return Err(format!("default for unknown surface port '{port}'"));
            }
        }
        Ok(())
    }

    /// `true` if the port's targets are per-voice members (so its default is
    /// applied at `/graph_newVoice`, not `/graph_new`). A port with no targets or
    /// only shared targets is shared.
    pub fn is_voice_port(&self, port: &str) -> bool {
        self.surface
            .get(port)
            .and_then(|ts| ts.first())
            .is_some_and(|t| self.members[t.member].voice)
    }

    /// `true` if any member is per-voice (so `/graph_newVoice` / MIDI notes apply).
    pub fn has_voice_members(&self) -> bool {
        self.members.iter().any(|m| m.voice)
    }
}

use std::collections::HashSet;
use std::sync::Arc;

/// A resolved surface: port name → `(member node id, control index, mul, add)`.
pub type ResolvedSurface = HashMap<String, Vec<(i32, u32, f32, f32)>>;

/// A live GraphDef instance: the group holding its shared members, the private
/// buses to reclaim on free, the resolved shared surface, and the per-voice
/// sub-groups spawned inside it.
pub struct GraphInstance {
    /// The def, kept so `/graph_newVoice` can instantiate its per-voice members.
    pub def: Arc<GraphDefSpec>,
    /// Shared member index → node id (per-voice members are absent).
    pub shared_nodes: HashMap<usize, i32>,
    /// Resolved internal bus name → first index, shared by all voices.
    pub bus_index: HashMap<String, usize>,
    /// Private audio buses `(first, width)` to free on teardown.
    pub audio_buses: Vec<(usize, usize)>,
    /// Private control buses `(first, width)` to free on teardown.
    pub control_buses: Vec<(usize, usize)>,
    /// Resolved shared surface (`/node_set` against the instance group id).
    pub surface: ResolvedSurface,
    /// The voice sub-group ids spawned inside this instance.
    pub voices: HashSet<i32>,
}

/// A live per-voice sub-graph spawned by `/graph_newVoice` (or a MIDI note): its
/// own resolved surface, and the instance it belongs to.
pub struct GraphVoice {
    pub instance: i32,
    pub surface: ResolvedSurface,
}

/// One entry of the boot preset (`boot.json`): a standalone GraphDef to
/// instantiate at startup (an always-on FX bus, a drone, a mixer), with
/// initial surface-port values. Authored by the user / a client; read-only at
/// boot.
#[derive(Clone, Serialize, Deserialize)]
pub struct BootInstance {
    pub graph: String,
    #[serde(default)]
    pub ports: HashMap<String, f32>,
}
