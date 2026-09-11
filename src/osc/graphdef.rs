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
    /// **Declares that this bus is meant to be provided**: a graph nested
    /// inside another does not invent its own output, it is handed one, and the
    /// parent names which of *its* buses that is (see [`MemberKind::Graph`]).
    ///
    /// It is a declaration and not a switch — what actually decides is whether
    /// the instantiation was handed this name, and a graph instantiated on its
    /// own by `/graph_new` is handed nothing and allocates everything. So the
    /// same def works standalone and nested, and this is here to say which
    /// buses a parent is expected to bind.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub external: bool,
}

fn one() -> usize {
    1
}

/// A control value of a member: a literal `f32`, or the *name* of an internal
/// bus (resolved to its allocated first index at instantiation). The bus form
/// is how a member is wired — its bus-selecting control (`out`/`in` on a
/// Faust def, or whatever control feeds an `Out`/`In` UGen) is set to a
/// private bus, uniformly for SynthDef and FaustDef members.
///
/// **A bus name may name one of its channels**, `"mix:1"`, resolving to the
/// bus's first index plus 1 — and `"OUT:1"` names the hardware's second
/// channel, so a master's own stereo output is written the same way. A UGen has one output, so a stereo writer is two
/// `Out` rows and each of them needs *its own* bus index — and a member cannot
/// compute one, because a bus-selecting input must be a control or a constant
/// for the wiring to be readable at all. Without this, a multichannel private
/// bus could be allocated and only its first channel could ever be reached,
/// which is to say a mixer could not be written.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ControlValue {
    Num(f32),
    Bus(String),
}

/// Splits a bus reference into its name and its channel offset: `"mix"` is
/// channel 0 of `mix`, `"mix:1"` its second channel.
///
/// A name with a `:` that is not a number is left whole, so a bus called
/// `a:b` stays findable and the failure is "unknown bus" rather than a silent
/// wire to somewhere else.
pub fn bus_channel(reference: &str) -> (&str, usize) {
    match reference.rsplit_once(':') {
        Some((name, channel)) => match channel.parse::<usize>() {
            Ok(channel) => (name, channel),
            Err(_) => (reference, 0),
        },
        None => (reference, 0),
    }
}

/// What a member is an instance **of**.
///
/// A graph made only of defs is one level of wiring, and one level is not what
/// a piece is: a track holds clips, a clip holds an effect chain, and each of
/// those is itself a wiring with a surface of its own. So a member may be
/// another GraphDef, and "an effect chain" and "a nested graph" stop being two
/// mechanisms.
///
/// The nesting is **of authoring, not of execution**: instantiating a graph
/// member is a subgroup inside the instance group, and its surface is
/// re-exported flat, so setting a port still costs the `/node_set`s it resolves
/// to and the audio thread learns nothing new.
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub enum MemberKind {
    /// A SynthDef or FaustDef: one node.
    #[default]
    Def,
    /// Another GraphDef: a subgroup with its own private buses and its own
    /// surface.
    Graph,
}

impl MemberKind {
    /// Whether this is the ordinary one-node kind (so serde can omit it).
    pub fn is_def(&self) -> bool {
        matches!(self, MemberKind::Def)
    }
}

/// The slot `voice: true` names. `/graph_newVoice` is `/graph_addSlot` on it.
pub const VOICE_SLOT: &str = "voice";

/// A member node: an instance of an existing SynthDef/FaustDef wired into the
/// graph. Members are listed in any order; the instance group is auto-sorted
/// so the execution order follows the bus connections.
#[derive(Clone, Serialize, Deserialize)]
pub struct GraphMember {
    /// SynthDef, FaustDef or GraphDef name (resolved at instantiation).
    pub def: String,
    /// Whether `def` names one node or a whole nested graph.
    #[serde(default, skip_serializing_if = "MemberKind::is_def")]
    pub kind: MemberKind,
    /// Initial control values by name (literals or internal-bus references).
    #[serde(default)]
    pub controls: HashMap<String, ControlValue>,
    /// Extra control→bus maps applied as `/node_map` (control name → internal
    /// control-bus name), for controls fed continuously by a bus.
    #[serde(default)]
    pub maps: HashMap<String, String>,
    /// `true` = a **per-voice** member. The original spelling of a slot, and
    /// still read: it means exactly `slot: "voice"`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub voice: bool,
    /// **A slot: a member there is a changing number of.**
    ///
    /// A shared member (no slot) is instantiated once at `/graph_new` — the
    /// always-on part: buses, mixer, effects. A slot member is instantiated on
    /// demand by `/graph_addSlot`, once per thing there is one of, wired to the
    /// same private buses: a voice of a synth, a clip on a track, an effect in a
    /// chain. They are one mechanism because they are one question — how many of
    /// these are there right now — and the answer changes while the graph is
    /// sounding, which is why it cannot be a member list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
}

impl GraphMember {
    /// The slot this member belongs to, or `None` when it is shared. `voice:
    /// true` is the slot named `"voice"`, spelled the way it was before slots
    /// had names.
    pub fn slot(&self) -> Option<&str> {
        match (&self.slot, self.voice) {
            (Some(name), _) => Some(name.as_str()),
            (None, true) => Some(VOICE_SLOT),
            (None, false) => None,
        }
    }
}

/// One inner target of a surface port: a member's control — or, for a member
/// that is itself a graph, one of **its** ports — with optional linear scaling
/// applied to the incoming value (`mul`·x + `add`).
///
/// Two scalings compose the way two functions do: the outer one runs first, so
/// a port re-exported from a child ends up at `mul_inner·mul_outer·x +
/// (mul_inner·add_outer + add_inner)`, resolved once at instantiation. Nothing
/// of the nesting survives into the running graph.
#[derive(Clone, Serialize, Deserialize)]
pub struct SurfaceTarget {
    /// Index into [`GraphDefSpec::members`].
    pub member: usize,
    /// The member's control, for a [`MemberKind::Def`] member.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub control: String,
    /// The member's own surface **port**, for a [`MemberKind::Graph`] member —
    /// which is what re-exporting a nested graph's interface is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
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
                if let ControlValue::Bus(reference) = v {
                    let (name, channel) = bus_channel(reference);
                    // `OUT` is the hardware, and `OUT:1` its second channel:
                    // the master's own output is a stereo write like any other.
                    if name == "OUT" {
                        continue;
                    }
                    let Some(bus) = self.buses.iter().find(|b| b.name == name) else {
                        return Err(format!("member {i}: unknown internal bus '{reference}'"));
                    };
                    if channel >= bus.channels.max(1) {
                        return Err(format!(
                            "member {i}: bus '{name}' has {} channels and '{reference}' asks for {}",
                            bus.channels.max(1),
                            channel + 1
                        ));
                    }
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
                // A target names a control of a node or a port of a nested
                // graph, and which of the two it is follows from what the
                // member is. Saying the wrong one is a def that would resolve
                // to nothing at instantiation and say nothing about why.
                match self.members[t.member].kind {
                    MemberKind::Def if t.port.is_some() => {
                        return Err(format!(
                            "surface port '{port}': member {} is a def, so name a control, not a port",
                            t.member
                        ));
                    }
                    MemberKind::Def if t.control.is_empty() => {
                        return Err(format!(
                            "surface port '{port}': member {} needs a control",
                            t.member
                        ));
                    }
                    MemberKind::Graph if t.port.is_none() => {
                        return Err(format!(
                            "surface port '{port}': member {} is a graph, so name one of its ports",
                            t.member
                        ));
                    }
                    _ => {}
                }
            }
            // A port drives either shared members or the members of **one**
            // slot, never a mix: a shared port resolves at /graph_new and a
            // slot port at /graph_addSlot, so they cannot share one name.
            let mut slots = targets.iter().map(|t| self.members[t.member].slot());
            let first = slots.next().flatten();
            if !targets
                .iter()
                .all(|t| self.members[t.member].slot() == first)
            {
                return Err(format!(
                    "surface port '{port}': its targets are in different slots"
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

    /// The slot a port drives, or `None` when it is a shared port — which is
    /// what says whether its default is applied at `/graph_new` or at
    /// `/graph_addSlot`. A port with no targets is shared.
    pub fn port_slot(&self, port: &str) -> Option<&str> {
        self.surface
            .get(port)
            .and_then(|ts| ts.first())
            .and_then(|t| self.members[t.member].slot())
    }

    /// `true` if any member belongs to `slot` (so `/graph_addSlot` on that name
    /// has something to build).
    pub fn has_slot(&self, slot: &str) -> bool {
        self.members.iter().any(|m| m.slot() == Some(slot))
    }

    /// `true` if any member is per-voice (so `/graph_newVoice` / MIDI notes
    /// apply). The voice slot's own spelling of [`GraphDefSpec::has_slot`].
    pub fn has_voice_members(&self) -> bool {
        self.has_slot(VOICE_SLOT)
    }
}

use std::collections::HashSet;
use std::sync::Arc;

/// A resolved surface: port name → `(member node id, control index, mul, add)`.
pub type ResolvedSurface = HashMap<String, Vec<(i32, u32, f32, f32)>>;

/// How deeply GraphDefs may be nested inside each other. A cycle — a graph that
/// contains itself, however indirectly — is a def that cannot be instantiated
/// at all, and this is what says so instead of recursing until the stack ends.
pub const MAX_GRAPH_DEPTH: usize = 8;

/// A live GraphDef instance: the group holding its shared members, the private
/// buses to reclaim on free, the resolved shared surface, the nested graph
/// members it built, and the slot sub-groups spawned inside it.
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
    /// Member index → the sub-instance group id, for members that are
    /// themselves graphs. Freed with this one, and what a re-exported port
    /// resolves through.
    pub children: HashMap<usize, i32>,
    /// The slot sub-group ids spawned inside this instance (voices included —
    /// a voice is the slot named `"voice"`).
    pub voices: HashSet<i32>,
}

/// A live slot sub-graph spawned by `/graph_addSlot` (`/graph_newVoice`, or a
/// MIDI note, for the voice slot): its own resolved surface, which slot it is,
/// the nested graph members it built, and the instance it belongs to.
pub struct GraphVoice {
    pub instance: i32,
    /// Which slot this fills. A voice is `"voice"`.
    pub slot: String,
    pub surface: ResolvedSurface,
    /// Member index → sub-instance group id, as in [`GraphInstance::children`].
    pub children: HashMap<usize, i32>,
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
