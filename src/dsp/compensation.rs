//! Off-thread latency planning and allocation-free, per-node bus views.
//!
//! A writer's contribution is delayed before mixing, never the already mixed
//! bus. Input delays align every input of an opaque processor, including its
//! sidechains. Musical delays are absent from SynthNode::latency().

use std::cell::UnsafeCell;
use std::collections::BTreeMap;

use super::{BLOCK_SIZE, Block, BusUsage, Buses, ProcessCtx, StageMask};
use crate::node::SynthNode;

/// Maximum aggregate delay history per published plan, in samples (64 MiB).
pub const MAX_HISTORY: usize = 16 * 1024 * 1024;

/// One statically analyzed synth in execution order, prepared outside RT.
pub struct AnalyzedNode {
    pub id: i32,
    pub usage: BusUsage,
    pub latency: usize,
    pub automatic: bool,
    pub domain: i32,
    pub runtime_usage: StageMask,
    pub identity: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelaySpec {
    pub id: i32,
    pub identity: u64,
    pub latency: usize,
    pub reads: Vec<(usize, usize)>,
    pub writes: Vec<(usize, usize)>,
}

/// A routing control or mapping in the producer snapshot.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct RouteState {
    pub index: usize,
    pub value: Option<u32>,
    pub bus: i32,
    pub audio: bool,
}

/// Immutable report, kept by the translator as well as the runtime plan.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct Report {
    pub nodes: Vec<DelaySpec>,
    pub excluded: Vec<(i32, &'static str)>,
    pub graph: Vec<(i32, usize, StageMask)>,
    pub objects: Vec<usize>,
    pub groups: Vec<(i32, i32, bool)>,
    pub routes: Vec<Vec<RouteState>>,
}

/// Computes a global plan over the admitted execution order. Unknown boundaries
/// taint their connected audio paths: compensation must not guess their timing.
pub fn analyze(nodes: &[AnalyzedNode]) -> Report {
    let mut writers: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut readers: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut excluded = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if !node.automatic {
            excluded.insert(i, "manual boundary");
        }
        if node.usage.dynamic {
            excluded.insert(i, "dynamic boundary");
        }
        for bus in node.usage.writes() {
            writers.entry(bus).or_default().push(i);
        }
        for bus in node.usage.reads() {
            readers.entry(bus).or_default().push(i);
        }
    }
    if nodes.iter().any(|n| n.usage.dynamic) {
        return Report {
            groups: Vec::new(),
            objects: Vec::new(),
            routes: Vec::new(),
            graph: nodes
                .iter()
                .map(|n| (n.id, n.latency, n.runtime_usage))
                .collect(),
            nodes: Vec::new(),
            excluded: nodes
                .iter()
                .map(|n| (n.id, "dynamic bus boundary"))
                .collect(),
        };
    }
    for (bus, sources) in &writers {
        if let Some(destinations) = readers.get(bus) {
            for &reader in destinations {
                if sources
                    .iter()
                    .any(|&writer| nodes[writer].domain != nodes[reader].domain)
                {
                    excluded.insert(reader, "manual group boundary");
                }
                if sources.iter().any(|&writer| writer >= reader) {
                    excluded.insert(reader, "feedback or execution-order boundary");
                    for &writer in sources {
                        excluded.insert(writer, "feedback or execution-order boundary");
                    }
                }
            }
        }
    }
    for sources in writers.values() {
        if sources
            .iter()
            .any(|&i| nodes[i].domain != nodes[sources[0]].domain)
        {
            for &i in sources {
                excluded.insert(i, "manual group boundary");
            }
        }
    }
    // Also taint parallel contributions on the same bus: an unknown writer
    // invalidates the arrival time of their sum, even without an In node.
    loop {
        let before = excluded.len();
        for (bus, sources) in &writers {
            let connected = sources.iter().chain(readers.get(bus).into_iter().flatten());
            if connected.clone().any(|i| excluded.contains_key(i)) {
                for &i in connected {
                    excluded
                        .entry(i)
                        .or_insert("uncompensated connected boundary");
                }
            }
        }
        if excluded.len() == before {
            break;
        }
    }
    let mut arrivals = BTreeMap::<usize, usize>::new();
    let mut specs = Vec::new();
    let mut output_times = BTreeMap::new();
    let mut overflow = false;
    for (i, node) in nodes.iter().enumerate() {
        if excluded.contains_key(&i) {
            continue;
        }
        let input = node
            .usage
            .reads()
            .map(|b| arrivals.get(&b).copied().unwrap_or(0))
            .max()
            .unwrap_or(0);
        let Some(output) = input.checked_add(node.latency) else {
            overflow = true;
            break;
        };
        output_times.insert(node.id, output);
        let reads = node
            .usage
            .reads()
            .map(|b| (b, input - arrivals.get(&b).copied().unwrap_or(0)))
            .collect();
        for bus in node.usage.writes() {
            arrivals
                .entry(bus)
                .and_modify(|a| *a = (*a).max(output))
                .or_insert(output);
        }
        specs.push(DelaySpec {
            id: node.id,
            identity: node.identity,
            latency: node.latency,
            reads,
            writes: Vec::new(),
        });
    }
    let mut history = 0usize;
    for spec in &mut specs {
        let node = nodes.iter().find(|n| n.id == spec.id).unwrap();
        let output = output_times[&spec.id];
        spec.writes = node
            .usage
            .writes()
            .map(|b| (b, arrivals[&b] - output))
            .collect();
        for &(_, delay) in spec.reads.iter().chain(&spec.writes) {
            history = history.saturating_add(delay);
        }
    }
    if overflow || history > MAX_HISTORY {
        return Report {
            groups: Vec::new(),
            objects: Vec::new(),
            routes: Vec::new(),
            graph: nodes
                .iter()
                .map(|n| (n.id, n.latency, n.runtime_usage))
                .collect(),
            nodes: Vec::new(),
            excluded: nodes
                .iter()
                .map(|n| (n.id, "delay storage capacity"))
                .collect(),
        };
    }
    Report {
        groups: Vec::new(),
        objects: Vec::new(),
        routes: Vec::new(),
        graph: nodes
            .iter()
            .map(|n| (n.id, n.latency, n.runtime_usage))
            .collect(),
        nodes: specs,
        excluded: excluded
            .into_iter()
            .map(|(i, reason)| (nodes[i].id, reason))
            .collect(),
    }
}

struct Channel {
    bus: usize,
    samples: Block,
    history: Vec<f32>,
    position: usize,
    replace: bool,
    modes: Vec<bool>,
    initialized: bool,
}
impl Channel {
    fn new(bus: usize, delay: usize) -> Self {
        Self {
            bus,
            samples: Block::SILENCE,
            history: vec![0.0; delay],
            position: 0,
            replace: false,
            modes: vec![false; delay],
            initialized: false,
        }
    }
    fn delay(&mut self, sample: f32, replace: bool) -> (f32, bool) {
        if self.history.is_empty() {
            return (sample, replace);
        }
        if !self.initialized {
            self.modes.fill(replace);
            self.initialized = true;
        }
        let result = (self.history[self.position], self.modes[self.position]);
        self.history[self.position] = sample;
        self.modes[self.position] = replace;
        self.position = (self.position + 1) % self.history.len();
        result
    }
}

/// One exclusive synth visit owns all of these mutable channels.
pub struct NodeDelay {
    spec: DelaySpec,
    reads: Vec<UnsafeCell<Channel>>,
    writes: Vec<UnsafeCell<Channel>>,
}
impl NodeDelay {
    fn new(spec: DelaySpec) -> Self {
        Self {
            reads: spec
                .reads
                .iter()
                .map(|&(bus, delay)| UnsafeCell::new(Channel::new(bus, delay)))
                .collect(),
            writes: spec
                .writes
                .iter()
                .map(|&(bus, delay)| UnsafeCell::new(Channel::new(bus, delay)))
                .collect(),
            spec,
        }
    }
    pub(crate) fn read(&self, bus: usize) -> Option<&[f32; BLOCK_SIZE]> {
        self.reads.iter().find_map(|cell| {
            let channel = unsafe { &*cell.get() };
            (channel.bus == bus).then_some(&channel.samples.0)
        })
    }
    #[allow(clippy::mut_from_ref)] // Each write channel has its own UnsafeCell.
    pub(crate) fn write(&self, bus: usize, replace: bool) -> Option<&mut [f32; BLOCK_SIZE]> {
        self.writes.iter().find_map(|cell| {
            let channel = unsafe { &mut *cell.get() };
            if channel.bus == bus {
                channel.replace |= replace;
                Some(&mut channel.samples.0)
            } else {
                None
            }
        })
    }
    fn process(&mut self, node: &mut dyn SynthNode, ctx: &ProcessCtx) {
        let range = ctx.offset..ctx.offset + ctx.frames;
        for cell in &mut self.reads {
            let channel = cell.get_mut();
            for frame in range.clone() {
                channel.samples.0[frame] =
                    channel.delay(ctx.buses.audio(channel.bus)[frame], false).0;
            }
        }
        for cell in &mut self.writes {
            let channel = cell.get_mut();
            channel.samples.0[range.clone()].fill(0.0);
            channel.replace = false;
        }
        // SAFETY: this stack-local view exists only for the exclusive synth
        // visit; the scheduler still uses the original global bus masks.
        let view = unsafe { Buses::compensated(ctx.buses, self) };
        node.process(&mut ProcessCtx {
            buses: &view,
            ..*ctx
        });
        drop(view);
        for cell in &mut self.writes {
            let channel = cell.get_mut();
            for frame in range.clone() {
                let (sample, replace) = channel.delay(channel.samples.0[frame], channel.replace);
                // SAFETY: the stage scheduler grants this node exclusive
                // access to every global bus it writes.
                let output = unsafe { ctx.buses.audio_mut(channel.bus) };
                if replace {
                    output[frame] = sample;
                } else {
                    output[frame] += sample;
                }
            }
        }
    }
}

/// Allocated outside RT. The scheduler grants one visitor per node id.
pub struct Plan {
    nodes: BTreeMap<i32, UnsafeCell<NodeDelay>>,
    graph: Vec<(i32, usize, StageMask)>,
    objects: Vec<usize>,
    groups: Vec<(i32, i32, bool)>,
    routes: Vec<Vec<RouteState>>,
}
// SAFETY: mutable access is restricted to the corresponding exclusive synth
// visit; installation and retirement happen only outside a processing stage.
unsafe impl Send for Plan {}
unsafe impl Sync for Plan {}
impl Plan {
    pub fn new(report: &Report) -> Self {
        Self {
            graph: report.graph.clone(),
            objects: report.objects.clone(),
            groups: report.groups.clone(),
            routes: report.routes.clone(),
            nodes: report
                .nodes
                .iter()
                .filter(|s| {
                    s.reads
                        .iter()
                        .chain(&s.writes)
                        .any(|&(_, delay)| delay != 0)
                })
                .cloned()
                .map(|s| (s.id, UnsafeCell::new(NodeDelay::new(s))))
                .collect(),
        }
    }
    pub(crate) fn has_delays(&self) -> bool {
        !self.nodes.is_empty()
    }

    pub(crate) fn groups(&self) -> &[(i32, i32, bool)] {
        &self.groups
    }

    /// Checks an exclusive live node against its prepared routing snapshot.
    /// Object identity and routing controls guard information a folded stage
    /// mask cannot distinguish, including future edits and reused node IDs.
    pub fn matches_node(
        &self,
        index: usize,
        id: i32,
        node: &dyn SynthNode,
        usage: StageMask,
    ) -> bool {
        if self.graph.get(index).copied() != Some((id, node.latency(), usage)) {
            return false;
        }
        let object = node as *const dyn SynthNode as *const () as usize;
        if self
            .objects
            .get(index)
            .is_some_and(|&expected| expected != object)
        {
            return false;
        }
        self.routes.get(index).is_none_or(|routes| {
            routes.iter().all(|route| {
                node.control_state(route.index).is_some_and(|(value, map)| {
                    route
                        .value
                        .is_none_or(|expected| value.to_bits() == expected)
                        && map.bus == route.bus
                        && (map.bus < 0 || map.audio == route.audio)
                })
            })
        })
    }
    pub(crate) fn len(&self) -> usize {
        self.graph.len()
    }

    /// Preserve delay history when an unrelated topology change leaves a
    /// channel's specification intact. Old storage is retired outside RT.
    pub fn retain_history(&mut self, old: &mut Self) {
        for (id, next) in &mut self.nodes {
            if let Some(prior) = old.nodes.get_mut(id) {
                let (next, prior) = (next.get_mut(), prior.get_mut());
                if next.spec == prior.spec {
                    std::mem::swap(next, prior);
                }
            }
        }
    }
    /// # Safety
    /// Exactly one visitor owns this node, and the plan is not being replaced.
    pub unsafe fn process(&self, id: i32, node: &mut dyn SynthNode, ctx: &ProcessCtx) {
        if let Some(delay) = self.nodes.get(&id) {
            let delay = unsafe { &mut *delay.get() };
            // A declaration changed without a published plan: never apply
            // stale compensation. The next off-thread rebuild supplies it.
            if delay.spec.latency == node.latency() {
                delay.process(node, ctx);
                return;
            }
        }
        let mut local = *ctx;
        node.process(&mut local);
    }
}
