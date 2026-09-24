//! **Applying an instance's ops**: the handle table, every [`Op`] as the
//! messages it is on the wire, and what those messages have to wait for.
//!
//! [`Instance::reconcile`](crate::instance::Instance::reconcile) says *what* to
//! do and deliberately not how; this is the how, written once. It was written
//! three times -- the Python client's `Playback.apply`, the web client's, and
//! the GUI host's own -- and the host's copy was the one that sent a buffer's
//! fill before its allocation and silenced the multitrack. The clients had it right
//! only because their `Buffer.from_samples` waited for the allocation inside
//! itself, which is a rule nobody could read off the op.
//!
//! So the waiting is **stated**, as [`Step`]s: a message to send, a `/done` to
//! wait for, a barrier to close. An endpoint walks the steps with its own
//! socket and its own way of waiting -- a script blocks, a page awaits, the host
//! resumes on the reply -- and none of them decides what goes on the wire.
//!
//! The numbers come from the endpoint's [`IdSpaces`], which is the core's one
//! allocation policy, so what this hands out cannot collide with anything else
//! the same client allocates.
//!
//! # Taken from the reference client
//!
//! Every message here is the one the Python client's objects send for the same
//! op -- `Group.graph`, `Group`, `Group.add_slot`, `Group.move_slot`, `Synth`, `Node.set`,
//! `Buffer.from_samples`, `Buffer.free`, `Server.transport_group` -- with the
//! same add actions and the same order, so replacing that client's applier
//! with this one changes nothing a server can see.

use std::collections::HashMap;

use clausters_core::ids::{IdError, IdSpaces, Space};
use clausters_core::osc::{OscMessage, OscType};
use serde_json::{Value, json};

use crate::instance::{Handle, Op, Port, Ports};

/// Where a node goes relative to its target -- the server's add actions.
/// The transport a multitrack plays on: transport 0, the one every server
/// has. Every transport command names its transport, so this is the id the
/// multitrack's own commands carry.
pub const MULTITRACK_TRANSPORT: i32 = 0;

const ADD_TAIL: i32 = 1;
const ROOT: i32 = 0;
const ADD_BEFORE: i32 = 2;

/// **One thing an endpoint does to carry out the ops**, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Send this message.
    Send(OscMessage),
    /// Wait for `/done <command> [index]` (or its `/fail`) before the next
    /// step: the command was asynchronous and what follows needs it finished.
    AwaitDone {
        /// The command whose completion is awaited.
        command: String,
        /// The index the `/done` names, where it names one.
        index: Option<i32>,
    },
    /// Close the batch: send `/server_sync id` and wait for its reply, which
    /// the server gives only once every asynchronous command sent earlier has
    /// completed.
    Sync(i32),
}

/// **What an endpoint arranges differently**: how many samples one fill may
/// carry, which is its carrier's bound.
///
/// Where the multitrack is made and how the transport is bound used to differ too --
/// a client bound the multitrack's graph at the root, the GUI host a group of its
/// own -- and they are the same everywhere now ([`Op::Transport`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Endpoint {
    /// The most samples one `/buffer_setRange` carries -- the endpoint's
    /// transport bound.
    pub chunk: usize,
}

impl Default for Endpoint {
    fn default() -> Self {
        Endpoint { chunk: 8192 }
    }
}

/// **What the ops made**, and the steps that make more.
#[derive(Debug, Clone)]
pub struct Applier {
    endpoint: Endpoint,
    nodes: HashMap<Handle, i32>,
    buses: HashMap<Handle, (i32, usize)>,
    /// The buses of `buses` that are audio buses, which go back to their own
    /// space.
    audio: std::collections::HashSet<Handle>,
    buffers: HashMap<Handle, i32>,
    next_sync: i32,
}

impl Applier {
    /// An applier that has made nothing yet.
    pub fn new(endpoint: Endpoint) -> Applier {
        Applier {
            endpoint,
            nodes: HashMap::new(),
            buses: HashMap::new(),
            audio: std::collections::HashSet::new(),
            buffers: HashMap::new(),
            next_sync: 1,
        }
    }

    /// The node a handle became.
    pub fn node(&self, handle: &str) -> Option<i32> {
        self.nodes.get(handle).copied()
    }

    /// The first bus of the run a handle became, and how long it is -- what a
    /// meter strip is read from.
    pub fn bus(&self, handle: &str) -> Option<(i32, usize)> {
        self.buses.get(handle).copied()
    }

    /// The buffer a handle became.
    pub fn buffer(&self, handle: &str) -> Option<i32> {
        self.buffers.get(handle).copied()
    }

    /// How many nodes it is holding.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// **The ops as steps**, allocating from `ids`.
    ///
    /// The order is the crate's: a def before the graph that names it, a
    /// buffer before the reader pointed at it, a node freed before the one that
    /// replaces it. An allocation that fails stops here and says which space
    /// ran out, with the steps before it already stated.
    pub fn apply(&mut self, ops: Vec<Op>, ids: &mut IdSpaces) -> Result<Vec<Step>, IdError> {
        let mut steps = Vec::new();
        for op in ops {
            self.one(op, ids, &mut steps)?;
        }
        Ok(steps)
    }

    fn one(&mut self, op: Op, ids: &mut IdSpaces, steps: &mut Vec<Step>) -> Result<(), IdError> {
        match op {
            Op::Def { family, spec } => steps.push(send(
                "/def_send",
                vec![OscType::String(family), OscType::String(spec.to_string())],
            )),
            Op::Barrier => steps.push(self.sync()),
            Op::Graph {
                handle,
                parent,
                graph,
                ports,
            } => {
                let Some(target) = self.node(&parent) else {
                    return Ok(());
                };
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                let mut args = vec![
                    OscType::String(graph),
                    OscType::Int(node),
                    OscType::Int(ADD_TAIL),
                    OscType::Int(target),
                ];
                args.extend(self.ports(&ports));
                steps.push(send("/graph_new", args));
            }
            // A group at the top, bound: `Group` and `Server.transport_group`.
            Op::Transport { handle } => {
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                steps.push(send(
                    "/group_new",
                    vec![
                        OscType::Int(node),
                        OscType::Int(ADD_TAIL),
                        OscType::Int(ROOT),
                    ],
                ));
                steps.push(send(
                    "/transport_group",
                    vec![OscType::Int(MULTITRACK_TRANSPORT), OscType::Int(node)],
                ));
                steps.push(Step::AwaitDone {
                    command: "/transport_group".into(),
                    index: None,
                });
            }
            // A group at the top, following: `Group` and
            // `Server.transport_follow`.
            Op::Follow { handle, transport } => {
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                steps.push(send(
                    "/group_new",
                    vec![
                        OscType::Int(node),
                        OscType::Int(ADD_TAIL),
                        OscType::Int(ROOT),
                    ],
                ));
                steps.push(send(
                    "/transport_follow",
                    vec![OscType::Int(transport), OscType::Int(node)],
                ));
                steps.push(Step::AwaitDone {
                    command: "/transport_follow".into(),
                    index: None,
                });
            }
            Op::Governed {
                handle,
                parent,
                transport,
            } => {
                let Some(target) = self.node(&parent) else {
                    return Ok(());
                };
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                steps.push(send(
                    "/group_new",
                    vec![
                        OscType::Int(node),
                        OscType::Int(ADD_TAIL),
                        OscType::Int(target),
                    ],
                ));
                steps.push(send(
                    "/transport_group",
                    vec![OscType::Int(transport), OscType::Int(node)],
                ));
                steps.push(Step::AwaitDone {
                    command: "/transport_group".into(),
                    index: None,
                });
            }
            Op::Govern { handle, transport } => {
                if let Some(node) = self.node(&handle) {
                    steps.push(send(
                        "/transport_group",
                        vec![OscType::Int(transport), OscType::Int(node)],
                    ));
                    steps.push(Step::AwaitDone {
                        command: "/transport_group".into(),
                        index: None,
                    });
                }
            }
            Op::AudioBus { handle, channels } => {
                let channels = channels.max(1);
                let first = ids.alloc(Space::AudioBuses, channels)? as i32;
                self.audio.insert(handle.clone());
                self.buses.insert(handle, (first, channels));
            }
            Op::Run { handle, run } => {
                if let Some(node) = self.node(&handle) {
                    steps.push(send(
                        "/node_run",
                        vec![OscType::Int(node), OscType::Int(i32::from(run))],
                    ));
                }
            }
            Op::Group { handle, before } => {
                let Some(target) = self.node(&before) else {
                    return Ok(());
                };
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                steps.push(send(
                    "/group_new",
                    vec![
                        OscType::Int(node),
                        OscType::Int(ADD_BEFORE),
                        OscType::Int(target),
                    ],
                ));
            }
            Op::Slot {
                handle,
                target,
                slot,
                ports,
            } => {
                let Some(instance) = self.node(&target) else {
                    return Ok(());
                };
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                let mut args = vec![
                    OscType::Int(instance),
                    OscType::String(slot),
                    OscType::Int(node),
                ];
                args.extend(self.ports(&ports));
                steps.push(send("/graph_addSlot", args));
            }
            Op::Synth {
                handle,
                def,
                target,
                ports,
            } => {
                let Some(group) = self.node(&target) else {
                    return Ok(());
                };
                let node = alloc_node(ids)?;
                self.nodes.insert(handle, node);
                let mut args = vec![
                    OscType::String(def),
                    OscType::Int(node),
                    OscType::Int(ADD_TAIL),
                    OscType::Int(group),
                ];
                args.extend(self.ports(&ports));
                steps.push(send("/synth_new", args));
            }
            // A control bus is the client's to count and the server's to hold:
            // nothing goes on the wire.
            Op::Bus { handle, channels } => {
                let channels = channels.max(1);
                let first = ids.alloc(Space::ControlBuses, channels)? as i32;
                self.buses.insert(handle, (first, channels));
            }
            // **Allocate, wait, fill, close** -- `Buffer.from_samples`. The
            // allocation is asynchronous, so a fill sent before its `/done` is
            // refused and the table reads nothing.
            Op::Buffer { handle, samples } => {
                let bufnum = ids.alloc(Space::Buffers, 1)? as i32;
                self.buffers.insert(handle, bufnum);
                steps.push(send(
                    "/buffer_alloc",
                    vec![
                        OscType::Int(bufnum),
                        OscType::Int(samples.len().max(1) as i32),
                        OscType::Int(1),
                    ],
                ));
                steps.push(Step::AwaitDone {
                    command: "/buffer_alloc".into(),
                    index: Some(bufnum),
                });
                if !samples.is_empty() {
                    let chunk = self.endpoint.chunk.max(1);
                    for (i, run) in samples.chunks(chunk).enumerate() {
                        let mut blob = Vec::with_capacity(run.len() * 4);
                        for value in run {
                            blob.extend_from_slice(&value.to_le_bytes());
                        }
                        steps.push(send(
                            "/buffer_setRange",
                            vec![
                                OscType::Int(bufnum),
                                OscType::Int((i * chunk) as i32),
                                OscType::Blob(blob),
                            ],
                        ));
                    }
                    steps.push(self.sync());
                }
            }
            Op::Set { handle, ports } => {
                if let Some(node) = self.node(&handle) {
                    let mut args = vec![OscType::Int(node)];
                    args.extend(self.ports(&ports));
                    steps.push(send("/node_set", args));
                }
            }
            Op::Move { handle, target } => {
                if let (Some(slot), Some(instance)) = (self.node(&handle), self.node(&target)) {
                    steps.push(send(
                        "/graph_moveSlot",
                        vec![OscType::Int(slot), OscType::Int(instance)],
                    ));
                }
            }
            Op::Map { handle, port, bus } => {
                if let (Some(node), Some((index, _))) = (self.node(&handle), self.bus(&bus)) {
                    steps.push(send(
                        "/graph_map",
                        vec![
                            OscType::Int(node),
                            OscType::String(port),
                            OscType::Int(index),
                        ],
                    ));
                }
            }
            // A handle with nothing behind it is a node that is already gone,
            // and a message naming one would reach whatever holds that id next.
            Op::Unmap { handle, port } => {
                if let Some(node) = self.node(&handle) {
                    steps.push(send(
                        "/graph_map",
                        vec![OscType::Int(node), OscType::String(port), OscType::Int(-1)],
                    ));
                }
            }
            // The id comes back when the server says the node ended
            // (`IdSpaces::node_ended`), not here: it is in flight until then.
            Op::Free { handle, forget } => {
                if let Some(node) = self.nodes.remove(&handle) {
                    steps.push(send("/node_free", vec![OscType::Int(node)]));
                }
                // Freeing a group frees what is inside it: these only leave the
                // table, since a second free would name a node already gone.
                for handle in forget {
                    self.nodes.remove(&handle);
                }
            }
            Op::FreeBus { handle } => {
                if let Some((first, channels)) = self.buses.remove(&handle) {
                    let space = if self.audio.remove(&handle) {
                        Space::AudioBuses
                    } else {
                        Space::ControlBuses
                    };
                    // A refused release means the table and the spaces went out
                    // of step, which the spaces report; the bus is gone either way.
                    let _ = ids.release(space, i64::from(first), channels);
                }
            }
            Op::FreeBuffer { handle } => {
                if let Some(bufnum) = self.buffers.remove(&handle) {
                    steps.push(send("/buffer_free", vec![OscType::Int(bufnum)]));
                    let _ = ids.release(Space::Buffers, i64::from(bufnum), 1);
                }
            }
        }
        Ok(())
    }

    fn sync(&mut self) -> Step {
        let id = self.next_sync;
        self.next_sync += 1;
        Step::Sync(id)
    }

    /// A port list as OSC pairs, every reference resolved through the tables.
    ///
    /// A port naming a resource the table does not hold is left out and the
    /// rest of the node still arrives: the reconciler does not emit one, and a
    /// whole strip that does not come because one port could not resolve is
    /// worse than a strip with that port unset.
    fn ports(&self, ports: &Ports) -> Vec<OscType> {
        let mut args = Vec::new();
        for (name, port) in ports {
            let value = match port {
                Port::Number(value) => *value as f32,
                Port::Bus { bus, offset } => match self.bus(bus) {
                    Some((first, _)) => (first + *offset as i32) as f32,
                    None => continue,
                },
                Port::Buffer { buffer } => match self.buffer(buffer) {
                    Some(bufnum) => bufnum as f32,
                    None => continue,
                },
            };
            args.push(OscType::String(name.clone()));
            args.push(OscType::Float(value));
        }
        args
    }
}

fn alloc_node(ids: &mut IdSpaces) -> Result<i32, IdError> {
    Ok(ids.alloc(Space::Nodes, 1)? as i32)
}

fn send(addr: &str, args: Vec<OscType>) -> Step {
    Step::Send(OscMessage {
        addr: addr.into(),
        args,
    })
}

/// **The steps as JSON**, for a client that walks them in its own language.
///
/// `{"send": {"addr", "args"}}` with each argument tagged by its OSC type --
/// `{"i": n}`, `{"f": x}`, `{"s": "..."}`, and `{"b": [x, ...]}` for a blob of
/// little-endian `f32`, which the client packs; `{"await": {"command",
/// "index"}}`; `{"sync": id}`.
pub fn steps_json(steps: &[Step]) -> Value {
    Value::Array(
        steps
            .iter()
            .map(|step| match step {
                Step::Send(message) => json!({"send": {
                    "addr": message.addr,
                    "args": message.args.iter().map(arg_json).collect::<Vec<_>>(),
                }}),
                Step::AwaitDone { command, index } => {
                    json!({"await": {"command": command, "index": index}})
                }
                Step::Sync(id) => json!({ "sync": id }),
            })
            .collect(),
    )
}

/// **Steps read back** from [`steps_json`]'s own shape: what a client hands a
/// [`Runner`](crate::run::Runner) after a playback answered it. A step this
/// cannot read is left out rather than guessed, and so is an argument.
pub fn steps_from_json(steps: &Value) -> Vec<Step> {
    let Some(steps) = steps.as_array() else {
        return Vec::new();
    };
    steps
        .iter()
        .filter_map(|step| {
            if let Some(send) = step.get("send") {
                return Some(Step::Send(OscMessage {
                    addr: send.get("addr")?.as_str()?.to_string(),
                    args: send
                        .get("args")
                        .and_then(Value::as_array)
                        .map(|args| args.iter().filter_map(arg_from_json).collect())
                        .unwrap_or_default(),
                }));
            }
            if let Some(wait) = step.get("await") {
                return Some(Step::AwaitDone {
                    command: wait.get("command")?.as_str()?.to_string(),
                    index: wait.get("index").and_then(Value::as_i64).map(|i| i as i32),
                });
            }
            step.get("sync")
                .and_then(Value::as_i64)
                .map(|id| Step::Sync(id as i32))
        })
        .collect()
}

/// One argument read back from [`arg_json`]'s tagged shape.
pub fn arg_from_json(arg: &Value) -> Option<OscType> {
    if let Some(v) = arg.get("i") {
        return Some(OscType::Int(v.as_i64()? as i32));
    }
    if let Some(v) = arg.get("h") {
        return Some(OscType::Long(v.as_i64()?));
    }
    if let Some(v) = arg.get("f") {
        return Some(OscType::Float(v.as_f64()? as f32));
    }
    if let Some(v) = arg.get("s") {
        return Some(OscType::String(v.as_str()?.to_string()));
    }
    let samples = arg.get("b")?.as_array()?;
    Some(OscType::Blob(
        samples
            .iter()
            .flat_map(|v| (v.as_f64().unwrap_or(0.0) as f32).to_le_bytes())
            .collect(),
    ))
}

/// One argument in the tagged shape a client reads.
pub fn arg_json(arg: &OscType) -> Value {
    match arg {
        OscType::Int(v) => json!({ "i": v }),
        OscType::Float(v) => json!({ "f": v }),
        OscType::String(v) => json!({ "s": v }),
        OscType::Long(v) => json!({ "h": v }),
        OscType::Blob(bytes) => json!({
            "b": bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect::<Vec<_>>()
        }),
        other => json!({ "s": format!("{other:?}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::ids::{IdShare, ServerShape};

    fn ids() -> IdSpaces {
        IdSpaces::new(
            ServerShape {
                max_nodes: 1024,
                audio_buses: 1024,
                outputs: 2,
                control_buses: 16384,
                buffers: 1024,
            },
            IdShare::WHOLE,
        )
    }

    fn addrs(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .map(|s| match s {
                Step::Send(m) => m.addr.clone(),
                Step::AwaitDone { command, .. } => format!("await {command}"),
                Step::Sync(_) => "sync".into(),
            })
            .collect()
    }

    /// **A table is filled after its buffer exists, and the step says so.**
    ///
    /// The defect behind this module: an endpoint that sent the fill right
    /// after the allocation had every fill refused, and a curve over an empty
    /// table silenced the multitrack. The wait is part of the answer now.
    #[test]
    fn a_buffer_is_allocated_awaited_filled_and_closed() {
        let mut applier = Applier::new(Endpoint { chunk: 2 });
        let steps = applier
            .apply(
                vec![Op::Buffer {
                    handle: "curvebuf".into(),
                    samples: vec![0.5, 1.0, 0.25],
                }],
                &mut ids(),
            )
            .unwrap();
        assert_eq!(
            addrs(&steps),
            [
                "/buffer_alloc",
                "await /buffer_alloc",
                "/buffer_setRange",
                "/buffer_setRange",
                "sync"
            ],
            "chunked by the endpoint's bound and closed with one barrier"
        );
        assert_eq!(applier.buffer("curvebuf"), Some(0));
    }

    /// The messages are the reference client's: the transport's group at the
    /// root and bound, the graph at its tail, a group before the node named, a
    /// slot inside its instance, and node ids from the client's own space.
    #[test]
    fn nodes_are_placed_as_the_reference_client_places_them() {
        let mut applier = Applier::new(Endpoint::default());
        let mut spaces = ids();
        let steps = applier
            .apply(
                vec![
                    Op::Transport {
                        handle: "transport".into(),
                    },
                    Op::Graph {
                        handle: "multitrack".into(),
                        parent: "transport".into(),
                        graph: "mt.multitrack".into(),
                        ports: Ports::new(),
                    },
                    Op::Group {
                        handle: "curves".into(),
                        before: "multitrack".into(),
                    },
                    Op::Slot {
                        handle: "track:1".into(),
                        target: "multitrack".into(),
                        slot: "tracks".into(),
                        ports: Ports::new(),
                    },
                ],
                &mut spaces,
            )
            .unwrap();
        let Step::Send(group) = &steps[0] else {
            panic!("the transport's group first")
        };
        assert_eq!(group.addr, "/group_new");
        assert_eq!(group.args[0], OscType::Int(1000), "the client range");
        assert_eq!(group.args[1], OscType::Int(ADD_TAIL));
        assert_eq!(group.args[2], OscType::Int(ROOT), "at the root");
        assert_eq!(
            addrs(&steps)[1..],
            [
                "/transport_group",
                "await /transport_group",
                "/graph_new",
                "/group_new",
                "/graph_addSlot"
            ]
        );
        let Step::Send(graph) = &steps[3] else {
            unreachable!()
        };
        assert_eq!(
            graph.args[3],
            OscType::Int(1000),
            "the multitrack inside the transport's group"
        );
    }

    /// **A move names both nodes from the table**, and a move whose slot or
    /// instance has nothing behind it sends nothing.
    #[test]
    fn a_move_is_one_message_naming_both_nodes() {
        let mut applier = Applier::new(Endpoint::default());
        let mut spaces = ids();
        let made = applier
            .apply(
                vec![
                    Op::Transport {
                        handle: "transport".into(),
                    },
                    Op::Graph {
                        handle: "multitrack".into(),
                        parent: "transport".into(),
                        graph: "mt.multitrack.2".into(),
                        ports: Ports::new(),
                    },
                    Op::Slot {
                        handle: "track:1".into(),
                        target: "multitrack".into(),
                        slot: "tracks".into(),
                        ports: Ports::new(),
                    },
                    Op::Slot {
                        handle: "track:2".into(),
                        target: "multitrack".into(),
                        slot: "tracks".into(),
                        ports: Ports::new(),
                    },
                    Op::Slot {
                        handle: "clip:3".into(),
                        target: "track:1".into(),
                        slot: "clips.1".into(),
                        ports: Ports::new(),
                    },
                ],
                &mut spaces,
            )
            .unwrap();
        assert!(!made.is_empty());
        let steps = applier
            .apply(
                vec![
                    Op::Move {
                        handle: "clip:3".into(),
                        target: "track:2".into(),
                    },
                    Op::Move {
                        handle: "clip:9".into(),
                        target: "track:2".into(),
                    },
                ],
                &mut spaces,
            )
            .unwrap();
        let [Step::Send(moved)] = &steps[..] else {
            panic!("one message: {steps:?}")
        };
        assert_eq!(moved.addr, "/graph_moveSlot");
        assert_eq!(
            moved.args,
            vec![
                OscType::Int(applier.node("clip:3").unwrap()),
                OscType::Int(applier.node("track:2").unwrap()),
            ]
        );
    }

    /// A port resolves through the table; one naming nothing is left out.
    #[test]
    fn ports_resolve_through_the_table() {
        let mut applier = Applier::new(Endpoint::default());
        let mut spaces = ids();
        applier
            .apply(
                vec![
                    Op::Bus {
                        handle: "meter".into(),
                        channels: 2,
                    },
                    Op::Transport {
                        handle: "transport".into(),
                    },
                    Op::Graph {
                        handle: "multitrack".into(),
                        parent: "transport".into(),
                        graph: "mt.multitrack".into(),
                        ports: [
                            ("gain".to_string(), Port::Number(0.5)),
                            (
                                "out".to_string(),
                                Port::Bus {
                                    bus: "meter".into(),
                                    offset: 1,
                                },
                            ),
                            (
                                "nowhere".to_string(),
                                Port::Buffer {
                                    buffer: "missing".into(),
                                },
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    },
                ],
                &mut spaces,
            )
            .unwrap();
        let (first, _) = applier.bus("meter").unwrap();
        let steps = applier
            .apply(
                vec![Op::Set {
                    handle: "multitrack".into(),
                    ports: [(
                        "out".to_string(),
                        Port::Bus {
                            bus: "meter".into(),
                            offset: 1,
                        },
                    )]
                    .into_iter()
                    .collect(),
                }],
                &mut spaces,
            )
            .unwrap();
        let Step::Send(set) = &steps[0] else {
            unreachable!()
        };
        assert_eq!(set.args[2], OscType::Float((first + 1) as f32));
    }

    /// Freeing takes the resources back to the spaces they came from, except a
    /// node id, which is in flight until the server says the node ended.
    #[test]
    fn frees_give_back_what_they_took() {
        let mut applier = Applier::new(Endpoint::default());
        let mut spaces = ids();
        applier
            .apply(
                vec![
                    Op::Bus {
                        handle: "b".into(),
                        channels: 2,
                    },
                    Op::Buffer {
                        handle: "t".into(),
                        samples: vec![],
                    },
                    Op::Transport {
                        handle: "transport".into(),
                    },
                    Op::Graph {
                        handle: "multitrack".into(),
                        parent: "transport".into(),
                        graph: "g".into(),
                        ports: Ports::new(),
                    },
                ],
                &mut spaces,
            )
            .unwrap();
        let steps = applier
            .apply(
                vec![
                    Op::FreeBus { handle: "b".into() },
                    Op::FreeBuffer { handle: "t".into() },
                    Op::Free {
                        handle: "multitrack".into(),
                        forget: vec![],
                    },
                ],
                &mut spaces,
            )
            .unwrap();
        assert_eq!(addrs(&steps), ["/buffer_free", "/node_free"]);
        assert_eq!(spaces.in_use(Space::ControlBuses), 0);
        assert_eq!(spaces.in_use(Space::Buffers), 0);
        assert_eq!(
            spaces.in_use(Space::Nodes),
            2,
            "the transport's group and the multitrack, until their /node_end"
        );
    }

    /// The JSON a client walks: typed arguments, and a blob as its samples.
    #[test]
    fn steps_are_read_back_as_they_were_written() {
        let steps = vec![
            Step::Send(OscMessage {
                addr: "/buffer_setRange".into(),
                args: vec![
                    OscType::Int(3),
                    OscType::Long(1 << 40),
                    OscType::Float(0.5),
                    OscType::String("x".into()),
                    OscType::Blob(
                        [0.25f32, -1.0]
                            .iter()
                            .flat_map(|v| v.to_le_bytes())
                            .collect(),
                    ),
                ],
            }),
            Step::AwaitDone {
                command: "/buffer_alloc".into(),
                index: Some(3),
            },
            Step::AwaitDone {
                command: "/transport_play".into(),
                index: None,
            },
            Step::Sync(9),
        ];
        assert_eq!(steps_from_json(&steps_json(&steps)), steps);
    }

    #[test]
    fn steps_cross_as_typed_json() {
        let steps = vec![
            send(
                "/buffer_setRange",
                vec![
                    OscType::Int(3),
                    OscType::Int(0),
                    OscType::Blob(0.5f32.to_le_bytes().to_vec()),
                ],
            ),
            Step::AwaitDone {
                command: "/buffer_alloc".into(),
                index: Some(3),
            },
            Step::Sync(1),
        ];
        let json = steps_json(&steps);
        assert_eq!(json[0]["send"]["args"][2]["b"][0], 0.5);
        assert_eq!(json[1]["await"]["index"], 3);
        assert_eq!(json[2]["sync"], 1);
    }
}
