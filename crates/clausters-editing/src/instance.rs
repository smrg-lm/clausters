//! **What is sounding, and the difference that makes it match.**
//!
//! The third projection, and the one with memory. The other two are functions
//! of a structure alone — the props it is drawn with, the payloads a gesture
//! becomes — and this one is a function of the structure *and of what a server
//! already holds*: a multitrack plays itself from the transport, so the nodes have
//! to **stay**. Rebuilding the tree on every edit would restart everything that
//! is sounding, and a hand dragging a box would hear its own gesture as a
//! stutter.
//!
//! # The shape: a reconciler, and the client is the host config
//!
//! [`clausters_document::multitrack::nodes::plan`] says what a multitrack *needs* —
//! which tracks, which clips, which readers, at which frames, with which
//! levels — and it is pure. [`Instance`] holds what was made from the last one,
//! and [`Instance::reconcile`] answers the **difference** as a list of
//! [`Op`]: send this def, add this slot, set these ports, free that node. It
//! opens no socket, awaits nothing and allocates no resource. That is what
//! makes it testable with no server in the room, identical under NRT, and
//! usable by a standalone host with no client in the process.
//!
//! It is the shape React's reconciler has and for the same reason: the tree
//! that *should* be is cheap to describe, the tree that *is* costs real
//! resources, and everything hard is in the diff between them. What React calls
//! a host config — who actually makes a node, who allocates — is here the
//! client, and it stays there because a bus allocator is a property of a
//! running session and not of a multitrack.
//!
//! # Handles: the crate names things it cannot make
//!
//! An op never carries a node id, a bus index or a buffer number, because this
//! allocates none of them. It carries a **handle** — a string this mints from
//! the document's own ids — and the client keeps one table from handle to
//! whatever it made. A port that has to name a resource names it the same way
//! ([`Port::Bus`], [`Port::Buffer`]), resolved by the client as it applies.
//!
//! So the client's half is exactly: a table, an allocator, and a socket. That
//! is the acceptance of this milestone stated as a sentence.
//!
//! # What a `set` cannot express is torn down, and only that
//!
//! One thing: a clip whose source changed **width** is a different wiring — a
//! mono take is panned into its track and a stereo one is balanced — so it is
//! another clip def and is made again. A clip that changed **track** is not
//! torn down: a clip is a slot *inside* a track's group, and the server moves a
//! slot to another instance and re-wires it there (`/graph_moveSlot`), so its
//! readers and the curves mapped onto its ports go with it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use clausters_core::mixer;
use clausters_document::multitrack::nodes::{
    Plan, PlannedClip, PlannedCurve, PlannedTrack, SourceInfo,
};

/// What the crate calls a thing it asked a client to make.
///
/// Minted from the document's own ids, so the same multitrack reconciled twice names
/// the same things — which is what lets an `Instance` be compared in a test
/// instead of being watched through a server.
pub type Handle = String;

/// One value of one port, as an op states it.
///
/// A plain number where it is one, and a **reference** where the value is a
/// resource this did not allocate: the client resolves it out of the same table
/// it filled when it applied the op that made it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Port {
    /// A number, which is what most ports are.
    Number(f64),
    /// The index of a control bus, `offset` channels along its run.
    Bus {
        /// The bus's handle.
        bus: Handle,
        /// How many channels past its first this port wants.
        #[serde(default, skip_serializing_if = "is_zero")]
        offset: usize,
    },
    /// A buffer's number.
    Buffer {
        /// The buffer's handle.
        buffer: Handle,
    },
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl From<f64> for Port {
    fn from(value: f64) -> Self {
        Port::Number(value)
    }
}

impl From<f32> for Port {
    fn from(value: f32) -> Self {
        Port::Number(f64::from(value))
    }
}

/// The ports of one node, in name order so two of them compare and print the
/// same way whoever built them.
pub type Ports = BTreeMap<String, Port>;

/// **One thing a client does to a server** to make what sounds be what is
/// drawn.
///
/// Deliberately small and deliberately not OSC: the crate defines edits and
/// does not encode them, so what an `AddSlot` *is* on the wire stays the
/// client's, and a client that speaks to an embedded server over a ring rather
/// than a socket applies the same list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    /// Send a def of `family` (`"synth"` or `"graph"`).
    ///
    /// In the order they come: a graph never names one that has not been sent,
    /// and getting that wrong fails in another process at instantiation with
    /// nothing to point at.
    Def {
        /// `"synth"` or `"graph"`.
        family: String,
        /// The def itself.
        spec: Value,
    },
    /// Wait for the defs just sent before anything else goes out.
    ///
    /// **One barrier for a whole batch, and only when something was sent.** A
    /// def send answers `/done`, so a `/done` left in flight is one the next
    /// command that waits for one takes as its own.
    Barrier,
    /// Instantiate a graph at the tail of a group: the multitrack itself, inside the
    /// transport's.
    #[serde(rename_all = "camelCase")]
    Graph {
        /// What to call it.
        handle: Handle,
        /// The group it is made at the tail of.
        parent: Handle,
        /// The graph's name.
        graph: String,
        /// Its ports.
        ports: Ports,
    },
    /// **Make the transport's group** at the top and bind it — the subtree the
    /// engine freezes on a stop and thaws on a play.
    ///
    /// Every endpoint makes the same one. A client used to bind the multitrack's own
    /// graph and the GUI host a group of its own that its take monitor shared,
    /// so one multitrack was governed two ways; what an endpoint needs beside the
    /// multitrack now goes inside this group.
    Transport {
        /// What to call it.
        handle: Handle,
    },
    /// A plain group, immediately **before** another node.
    Group {
        /// What to call it.
        handle: Handle,
        /// The node it goes before.
        before: Handle,
    },
    /// Add a slot instance to a graph instance.
    Slot {
        /// What to call it.
        handle: Handle,
        /// The instance it goes in.
        target: Handle,
        /// The slot's name in that graph's surface.
        slot: String,
        /// Its ports.
        ports: Ports,
    },
    /// Move a slot instance into another graph instance, re-wired there and
    /// not made again: what hangs off its nodes stays.
    Move {
        /// The slot.
        handle: Handle,
        /// The instance it moves into.
        target: Handle,
    },
    /// A synth of `def`, in `target`.
    Synth {
        /// What to call it.
        handle: Handle,
        /// The def's name.
        def: String,
        /// The group it goes in.
        target: Handle,
        /// Its ports.
        ports: Ports,
    },
    /// Allocate a control bus of `channels`.
    Bus {
        /// What to call it.
        handle: Handle,
        /// How wide.
        channels: usize,
    },
    /// Allocate a buffer holding `samples`.
    Buffer {
        /// What to call it.
        handle: Handle,
        /// Its contents.
        samples: Vec<f32>,
    },
    /// Write ports onto a node that is already there.
    Set {
        /// The node.
        handle: Handle,
        /// Only what moved: a resend states what is already there.
        ports: Ports,
    },
    /// Map a port of a node onto a control bus, so a curve drives it.
    Map {
        /// The node whose surface names the port.
        handle: Handle,
        /// The port.
        port: String,
        /// The bus.
        bus: Handle,
    },
    /// Give a port back to the hand.
    ///
    /// **Not optional.** A port left mapped to a bus nobody writes holds
    /// whatever was in it, so a curve that was deleted would go on driving the
    /// control it drove, at the last value it happened to say.
    Unmap {
        /// The node.
        handle: Handle,
        /// The port.
        port: String,
    },
    /// Free a node.
    ///
    /// **Freeing a group frees what is inside it**, so `forget` names the
    /// handles that went with it — the readers of a clip, the clips and meters
    /// of a track. A client drops those from its table without sending
    /// anything: a second free would name a node that is already gone, and
    /// keeping them would grow the table by every box a session ever removed.
    Free {
        /// The node.
        handle: Handle,
        /// What went with it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        forget: Vec<Handle>,
    },
    /// Give a control bus back.
    FreeBus {
        /// The bus.
        handle: Handle,
    },
    /// Give a buffer back.
    FreeBuffer {
        /// The buffer.
        handle: Handle,
    },
}

/// The multitrack's own instance.
pub const MULTITRACK: &str = "multitrack";

/// The group the curve nodes live in, **before** the multitrack so a value is
/// written in the block it is read.
pub const CURVES: &str = "curves";

/// **The transport's group**: made at the top by every endpoint alike, bound as
/// the transport's, and the multitrack's graph is made inside it -- so what a stop
/// freezes is the multitrack and whatever an endpoint puts beside it in there.
pub const TRANSPORT: &str = "transport";

fn track_handle(id: u64) -> Handle {
    format!("track:{id}")
}

fn meter_bus_handle(id: u64) -> Handle {
    format!("meterbus:{id}")
}

fn meter_handle(id: u64, n: usize) -> Handle {
    format!("meter:{id}:{n}")
}

fn clip_handle(id: u64) -> Handle {
    format!("clip:{id}")
}

fn reader_handle(region: u64, channel: usize) -> Handle {
    format!("reader:{region}:{channel}")
}

fn curve_handle(id: u64) -> Handle {
    format!("curve:{id}")
}

fn curve_bus_handle(id: u64) -> Handle {
    format!("curvebus:{id}")
}

/// A curve's table is **replaced** rather than written into — its length
/// changes with its first and last point — so each one is its own buffer and
/// the generation is what keeps the old handle addressable until the new table
/// is in.
fn curve_buffer_handle(id: u64, generation: u32) -> Handle {
    format!("curvebuf:{id}:{generation}")
}

/// What a track is, once it is sounding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TrackState {
    channels: usize,
    ports: Ports,
}

/// What a clip is, once it is sounding.
///
/// The slot is kept because a source of another width is another clip def,
/// which is the one change a `set` cannot express; the track is kept so a track
/// that went away takes its clips out of the table with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct ClipState {
    slot: String,
    ports: Ports,
    track: u64,
}

/// What a curve is, once it is sounding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CurveState {
    owner: Handle,
    /// Whether that owner is a box rather than a track — which table to look
    /// in to find out whether it is still there.
    owner_is_clip: bool,
    /// The owner's own id, so the lookup needs no handle to be parsed back.
    owner_id: u64,
    /// Which making of that owner the port was mapped on. See
    /// [`ClipState::generation`].
    owner_generation: u32,
    port: String,
    table: Vec<f32>,
    generation: u32,
}

/// **What a server already holds of one multitrack**, and the diff that keeps it
/// right.
///
/// Held by whoever is playing the multitrack, across edits. It is state and says so:
/// the other two projections are functions of a structure and this one is a
/// function of a structure *and* of what was made from the last one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    /// The def names already sent. A multitrack asks for the widths it uses, and a
    /// take of another width arriving later asks for more.
    sent: BTreeSet<String>,
    /// Whether the multitrack's own instance is up.
    multitrack: bool,
    /// Whether the curve group is up.
    curve_group: bool,
    tracks: BTreeMap<u64, TrackState>,
    clips: BTreeMap<u64, ClipState>,
    readers: BTreeMap<String, Ports>,
    curves: BTreeMap<u64, CurveState>,
    /// **How many times each box has been made.**
    ///
    /// A handle is a name for "the node that plays region 3" and stays the same
    /// across a rebuild, which is what a client's table wants — so a curve
    /// mapped onto the node that went cannot tell by the name alone that it
    /// went. This is what it tells by.
    ///
    /// It is kept apart from [`ClipState`] because the two have different
    /// lives: a box dragged to another track leaves its old track's table
    /// before it reaches the new one's, so the state it was in is gone by the
    /// time the making matters. A counter per region, never decremented.
    makings: BTreeMap<u64, u32>,
}

/// The ports the **hand** writes: everything a curve is not driving.
///
/// A mapped control is taken back by a plain `set` — that is the protocol's own
/// rule, and the right one, since it is what gives the fader back when a curve
/// is deleted. It also means that anything sending a value for a port a curve
/// drives **silences that curve**, and a multitrack reconciles on every edit, so
/// adding a box to a track was enough to stop its automation from being heard.
///
/// So the two stop competing. A curve owns the port it names and the hand's
/// value is not sent for it, which is also what a mixer means by an automation
/// in read: touching the fader under a curve does nothing until the curve is
/// gone. Writing *through* a curve — touch, latch — is a mode nothing has yet,
/// and it would be this function's answer changing rather than a set slipping
/// past.
fn hand_ports(ports: &[(&str, f32)], curves: &[PlannedCurve]) -> Ports {
    ports
        .iter()
        .filter(|(port, _)| !curves.iter().any(|curve| curve.port == *port))
        .map(|(port, value)| ((*port).to_string(), Port::from(*value)))
        .collect()
}

/// Only what moved: a resend states what is already there and is heard by
/// nobody.
fn moved(now: &Ports, sent: &Ports) -> Ports {
    now.iter()
        .filter(|(port, value)| sent.get(*port) != Some(value))
        .map(|(port, value)| (port.clone(), value.clone()))
        .collect()
}

impl Instance {
    /// Nothing is sounding yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether anything of a multitrack is believed to be sounding.
    ///
    /// The defs are not part of it: a def sent is on the server whatever this
    /// holds, and a torn-down instance that sent it again would be sending what
    /// is already there.
    pub fn is_sounding(&self) -> bool {
        self.multitrack
            || self.curve_group
            || !self.tracks.is_empty()
            || !self.clips.is_empty()
            || !self.readers.is_empty()
            || !self.curves.is_empty()
    }

    /// Which control bus run each track's meters write, by track — what a host
    /// reads every frame, and the reason a level that moves every block costs
    /// no message.
    ///
    /// A run of `2 * channels`: the level first and the mark that waits after
    /// it. The handle is the client's to resolve, like every other.
    pub fn meters(&self) -> Vec<(u64, Handle, usize)> {
        self.tracks
            .iter()
            .map(|(id, track)| (*id, meter_bus_handle(*id), track.channels))
            .collect()
    }

    /// [`Instance::meters`] as the JSON both doors carry.
    pub fn meters_json(&self) -> String {
        let rows: Vec<Value> = self
            .meters()
            .into_iter()
            .map(|(track, bus, channels)| {
                serde_json::json!({ "track": track, "bus": bus, "channels": channels })
            })
            .collect();
        serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
    }

    /// [`Instance::teardown`] as the JSON both doors carry.
    pub fn teardown_json(&mut self) -> String {
        serde_json::to_string(&self.teardown()).unwrap_or_else(|_| "[]".into())
    }

    /// **The difference between what is sounding and what `plan` says.**
    ///
    /// Everything already right is left alone, which is what lets a hand drag a
    /// box without hearing the rest of the multitrack restart. `gain` is the
    /// master's own level, which is the caller's and not the multitrack's.
    pub fn reconcile(&mut self, plan: &Plan, gain: f32) -> Vec<Op> {
        let mut ops = Vec::new();
        self.defs(plan, &mut ops);
        if !self.multitrack {
            // **The transport's group first, and the multitrack inside it**: from
            // here the engine freezes that subtree on a stop and thaws it on a
            // play, and every reader's position is the engine's own rather than
            // a number kept in step by a client.
            ops.push(Op::Transport {
                handle: TRANSPORT.into(),
            });
            ops.push(Op::Graph {
                handle: MULTITRACK.into(),
                parent: TRANSPORT.into(),
                graph: plan.graph.clone(),
                ports: [("gain".to_string(), Port::from(gain))]
                    .into_iter()
                    .collect(),
            });
            self.multitrack = true;
        }
        if !self.curve_group {
            ops.push(Op::Group {
                handle: CURVES.into(),
                before: MULTITRACK.into(),
            });
            self.curve_group = true;
        }
        self.tracks(&plan.tracks, &mut ops);
        self.reap_curves(plan, &mut ops);
        ops
    }

    /// **Everything, freed.** The multitrack itself is untouched: what an instance
    /// holds is nodes, and nodes are not the document.
    pub fn teardown(&mut self) -> Vec<Op> {
        let mut ops = Vec::new();
        // Everything the two groups take with them, read before the tables are
        // emptied: a client drops these from its own table without sending
        // anything, since freeing a group frees what is inside it.
        let curves = self.curve_handles();
        let mut under: Vec<Handle> = self
            .tracks
            .keys()
            .flat_map(|id| {
                [
                    track_handle(*id),
                    meter_handle(*id, 0),
                    meter_handle(*id, 1),
                ]
            })
            .collect();
        under.extend(self.clips.keys().map(|id| clip_handle(*id)));
        under.extend(self.readers.keys().cloned());

        for (id, curve) in std::mem::take(&mut self.curves) {
            ops.push(Op::FreeBuffer {
                handle: curve_buffer_handle(id, curve.generation),
            });
            ops.push(Op::FreeBus {
                handle: curve_bus_handle(id),
            });
        }
        if std::mem::take(&mut self.curve_group) {
            ops.push(Op::Free {
                handle: CURVES.into(),
                forget: curves,
            });
        }
        for id in std::mem::take(&mut self.tracks).into_keys() {
            ops.push(Op::FreeBus {
                handle: meter_bus_handle(id),
            });
        }
        self.readers.clear();
        self.clips.clear();
        self.makings.clear();
        if std::mem::take(&mut self.multitrack) {
            // One free: everything the multitrack holds is inside the transport's
            // group, the multitrack's graph included.
            let mut under = under;
            under.push(MULTITRACK.into());
            ops.push(Op::Free {
                handle: TRANSPORT.into(),
                forget: under,
            });
        }
        ops
    }

    /// The defs this multitrack's widths need, and only the ones not sent.
    fn defs(&mut self, plan: &Plan, ops: &mut Vec<Op>) {
        let Ok(defs) = mixer::defs_for(&plan.widths, plan.channels) else {
            return;
        };
        let before = ops.len();
        for (family, specs) in [("synth", &defs.synth), ("graph", &defs.graph)] {
            for spec in specs {
                let Some(name) = spec.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if !self.sent.insert(name.to_string()) {
                    continue;
                }
                ops.push(Op::Def {
                    family: family.to_string(),
                    spec: spec.clone(),
                });
            }
        }
        if ops.len() > before {
            ops.push(Op::Barrier);
        }
    }

    fn tracks(&mut self, planned: &[PlannedTrack], ops: &mut Vec<Op>) {
        let clips: BTreeSet<u64> = planned
            .iter()
            .flat_map(|track| track.clips.iter().map(|clip| clip.region.0))
            .collect();
        let mut seen = BTreeSet::new();
        for track in planned {
            let id = track.track.0;
            seen.insert(id);
            let ports = hand_ports(&[("gain", track.gain), ("mute", track.mute)], &track.curves);
            match self.tracks.get(&id) {
                None => {
                    ops.push(Op::Slot {
                        handle: track_handle(id),
                        target: MULTITRACK.into(),
                        slot: "tracks".into(),
                        ports: ports.clone(),
                    });
                    self.meter(id, track.channels.max(1), ops);
                    self.tracks.insert(
                        id,
                        TrackState {
                            channels: track.channels.max(1),
                            ports: ports.clone(),
                        },
                    );
                }
                Some(held) => {
                    let moved = moved(&ports, &held.ports);
                    if !moved.is_empty() {
                        ops.push(Op::Set {
                            handle: track_handle(id),
                            ports: moved,
                        });
                    }
                    let channels = held.channels;
                    self.tracks.insert(
                        id,
                        TrackState {
                            channels,
                            ports: ports.clone(),
                        },
                    );
                }
            }
            self.curves(&track_handle(id), false, id, 0, &track.curves, ops);
            self.clips(id, &track.clips, &clips, ops);
        }
        let gone: Vec<u64> = self
            .tracks
            .keys()
            .copied()
            .filter(|id| !seen.contains(id))
            .collect();
        for id in gone {
            self.free_track(id, ops);
        }
    }

    /// Put this track's meters on it, and remember where they write.
    ///
    /// **Two of them**, which is one def twice: with no hold it is the level,
    /// with the core's hold it is the mark that stays up long enough to be
    /// read. Both are slot instances, so a multitrack nobody meters holds none.
    fn meter(&mut self, id: u64, channels: usize, ops: &mut Vec<Op>) {
        ops.push(Op::Bus {
            handle: meter_bus_handle(id),
            channels: 2 * channels,
        });
        for (n, (run, hold)) in [(0, 0.0), (channels, mixer::METER_HOLD)]
            .into_iter()
            .enumerate()
        {
            let mut ports: Ports = BTreeMap::new();
            ports.insert(
                "meter/out0".into(),
                Port::Bus {
                    bus: meter_bus_handle(id),
                    offset: run,
                },
            );
            ports.insert("meter/hold".into(), Port::from(hold));
            if channels > 1 {
                ports.insert(
                    "meter/out1".into(),
                    Port::Bus {
                        bus: meter_bus_handle(id),
                        offset: run + 1,
                    },
                );
            }
            ops.push(Op::Slot {
                handle: meter_handle(id, n),
                target: track_handle(id),
                slot: "meters".into(),
                ports,
            });
        }
    }

    fn clips(
        &mut self,
        track: u64,
        planned: &[PlannedClip],
        planned_all: &BTreeSet<u64>,
        ops: &mut Vec<Op>,
    ) {
        let mut seen = BTreeSet::new();
        for clip in planned {
            let id = clip.region.0;
            seen.insert(id);
            let ports = hand_ports(&[("gain", clip.gain), ("mute", clip.mute)], &clip.curves);
            let held = self.clips.get(&id).cloned();
            // **What a `set` cannot express.** A source of another width is
            // another clip def, so that clip is made again. A clip that changed
            // track is a slot in another group -- the node carries no track id
            // to update, so a set would leave it sounding through the track it
            // came from -- and the server moves it there, re-wired, with its
            // readers and the maps on its ports still on it.
            let held = match held {
                Some(held) if held.slot != clip.slot => {
                    self.free_clip(id, true, ops);
                    None
                }
                Some(held) if held.track != track => {
                    ops.push(Op::Move {
                        handle: clip_handle(id),
                        target: track_handle(track),
                    });
                    Some(held)
                }
                other => other,
            };
            let generation = match held {
                None => {
                    ops.push(Op::Slot {
                        handle: clip_handle(id),
                        target: track_handle(track),
                        slot: clip.slot.clone(),
                        ports: ports.clone(),
                    });
                    let made = self.makings.entry(id).or_default();
                    *made += 1;
                    *made
                }
                Some(held) => {
                    let moved = moved(&ports, &held.ports);
                    if !moved.is_empty() {
                        ops.push(Op::Set {
                            handle: clip_handle(id),
                            ports: moved,
                        });
                    }
                    self.makings.get(&id).copied().unwrap_or(1)
                }
            };
            self.clips.insert(
                id,
                ClipState {
                    slot: clip.slot.clone(),
                    ports: ports.clone(),
                    track,
                },
            );
            self.curves(&clip_handle(id), true, id, generation, &clip.curves, ops);
            self.readers(id, clip, ops);
        }
        // A clip the plan holds on another track is not gone: it moves when
        // that track is reached, whichever comes first.
        let gone: Vec<u64> = self
            .clips
            .iter()
            .filter(|(id, held)| {
                held.track == track && !seen.contains(id) && !planned_all.contains(id)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.free_clip(id, true, ops);
        }
    }

    fn readers(&mut self, region: u64, clip: &PlannedClip, ops: &mut Vec<Op>) {
        let mut seen = BTreeSet::new();
        for reader in &clip.readers {
            let handle = reader_handle(region, reader.channel);
            seen.insert(handle.clone());
            let ports: Ports = [
                ("at", reader.at),
                ("buf", f64::from(reader.buffer)),
                ("chan", reader.channel as f64),
                ("loop", f64::from(u8::from(reader.looping))),
                ("span", reader.span),
                ("start", reader.start),
            ]
            .into_iter()
            .map(|(port, value)| (port.to_string(), Port::Number(value)))
            .collect();
            match self.readers.get(&handle) {
                None => ops.push(Op::Slot {
                    handle: handle.clone(),
                    target: clip_handle(region),
                    slot: "source".into(),
                    ports: ports.clone(),
                }),
                Some(sent) => {
                    // Every one of these is an ordinary control, `buf`
                    // included, so a box that was re-cut over a different
                    // buffer keeps sounding.
                    let moved = moved(&ports, sent);
                    if !moved.is_empty() {
                        ops.push(Op::Set {
                            handle: handle.clone(),
                            ports: moved,
                        });
                    }
                }
            }
            self.readers.insert(handle, ports);
        }
        let prefix = format!("reader:{region}:");
        let gone: Vec<String> = self
            .readers
            .keys()
            .filter(|handle| handle.starts_with(&prefix) && !seen.contains(*handle))
            .cloned()
            .collect();
        for handle in gone {
            self.readers.remove(&handle);
            ops.push(Op::Free {
                handle,
                forget: Vec::new(),
            });
        }
    }

    /// Put each curve's table on the server and map the port to it.
    ///
    /// A curve is **not** a member of the multitrack's graph, and that is the point:
    /// it writes a control bus, the port is mapped to that bus, and the port's
    /// own member ids stay private. The table is read at the transport's own
    /// position, so a locate costs no message at all — which is the whole
    /// reason a curve is a table and not a stream of sets.
    fn curves(
        &mut self,
        owner: &str,
        owner_is_clip: bool,
        owner_id: u64,
        generation: u32,
        planned: &[PlannedCurve],
        ops: &mut Vec<Op>,
    ) {
        for curve in planned {
            let id = curve.id.0;
            match self.curves.get(&id).cloned() {
                None => {
                    ops.push(Op::Buffer {
                        handle: curve_buffer_handle(id, 0),
                        samples: curve.table.clone(),
                    });
                    ops.push(Op::Bus {
                        handle: curve_bus_handle(id),
                        channels: 1,
                    });
                    ops.push(Op::Synth {
                        handle: curve_handle(id),
                        def: mixer::curve_name().to_string(),
                        target: CURVES.into(),
                        ports: [
                            ("at".to_string(), Port::Number(curve.at)),
                            (
                                "buf".to_string(),
                                Port::Buffer {
                                    buffer: curve_buffer_handle(id, 0),
                                },
                            ),
                            (
                                "out".to_string(),
                                Port::Bus {
                                    bus: curve_bus_handle(id),
                                    offset: 0,
                                },
                            ),
                            ("step".to_string(), Port::Number(curve.step)),
                        ]
                        .into_iter()
                        .collect(),
                    });
                    ops.push(Op::Map {
                        handle: owner.to_string(),
                        port: curve.port.clone(),
                        bus: curve_bus_handle(id),
                    });
                    self.curves.insert(
                        id,
                        CurveState {
                            owner: owner.to_string(),
                            owner_is_clip,
                            owner_id,
                            owner_generation: generation,
                            port: curve.port.clone(),
                            table: curve.table.clone(),
                            generation: 0,
                        },
                    );
                }
                Some(held) => {
                    if held.owner != owner || held.owner_generation != generation {
                        // **The map belongs to the node, not to the curve.** A
                        // clip whose source changed width is a new node, and
                        // the port that was mapped went away with the old one
                        // while the curve went on writing a bus nobody reads.
                        // The hand does not send that port either (`hand_ports`
                        // leaves it to the curve), so the box would come back
                        // at the def's own default -- which is how a clip
                        // dragged to another track played flat out, when that
                        // was a rebuild too.
                        ops.push(Op::Map {
                            handle: owner.to_string(),
                            port: curve.port.clone(),
                            bus: curve_bus_handle(id),
                        });
                    }
                    let mut table_generation = held.generation;
                    let mut ports: Ports = [
                        ("at".to_string(), Port::Number(curve.at)),
                        ("step".to_string(), Port::Number(curve.step)),
                    ]
                    .into_iter()
                    .collect();
                    if held.table != curve.table {
                        // A curve whose points moved is a new table, and a
                        // table is **replaced** rather than written into -- its
                        // length changes with its first and last point. `buf`
                        // is an ordinary control, so the reader follows without
                        // stopping.
                        table_generation += 1;
                        ops.push(Op::Buffer {
                            handle: curve_buffer_handle(id, table_generation),
                            samples: curve.table.clone(),
                        });
                        ports.insert(
                            "buf".to_string(),
                            Port::Buffer {
                                buffer: curve_buffer_handle(id, table_generation),
                            },
                        );
                    }
                    ops.push(Op::Set {
                        handle: curve_handle(id),
                        ports,
                    });
                    if table_generation != held.generation {
                        ops.push(Op::FreeBuffer {
                            handle: curve_buffer_handle(id, held.generation),
                        });
                    }
                    self.curves.insert(
                        id,
                        CurveState {
                            owner: owner.to_string(),
                            owner_is_clip,
                            owner_id,
                            owner_generation: generation,
                            port: curve.port.clone(),
                            table: curve.table.clone(),
                            generation: table_generation,
                        },
                    );
                }
            }
        }
    }

    /// Free the curves the multitrack no longer has, and give their ports back.
    fn reap_curves(&mut self, plan: &Plan, ops: &mut Vec<Op>) {
        let mut alive = BTreeSet::new();
        for track in &plan.tracks {
            alive.extend(track.curves.iter().map(|curve| curve.id.0));
            for clip in &track.clips {
                alive.extend(clip.curves.iter().map(|curve| curve.id.0));
            }
        }
        let gone: Vec<u64> = self
            .curves
            .keys()
            .copied()
            .filter(|id| !alive.contains(id))
            .collect();
        for id in gone {
            let curve = self.curves.remove(&id).expect("just listed");
            // **A node that is gone has nothing to give back.** Freeing it took
            // its map with it, so an unmap naming it is a message about a node
            // the server no longer has -- and in a client it is a handle whose
            // table entry went with the free, which is a lookup that has no
            // answer. The curve's own node, buffer and bus still go: those are
            // this instance's and outlive whatever the port belonged to.
            let owner_here = if curve.owner_is_clip {
                self.clips.contains_key(&curve.owner_id)
            } else {
                self.tracks.contains_key(&curve.owner_id)
            };
            if owner_here {
                ops.push(Op::Unmap {
                    handle: curve.owner,
                    port: curve.port,
                });
            }
            ops.push(Op::Free {
                handle: curve_handle(id),
                forget: Vec::new(),
            });
            ops.push(Op::FreeBuffer {
                handle: curve_buffer_handle(id, curve.generation),
            });
            ops.push(Op::FreeBus {
                handle: curve_bus_handle(id),
            });
        }
    }

    /// The handles of the readers of one box.
    fn reader_handles(&self, region: u64) -> Vec<Handle> {
        let prefix = format!("reader:{region}:");
        self.readers
            .keys()
            .filter(|handle| handle.starts_with(&prefix))
            .cloned()
            .collect()
    }

    /// The handles of every curve node, which go with the curve group.
    fn curve_handles(&self) -> Vec<Handle> {
        self.curves.keys().map(|id| curve_handle(*id)).collect()
    }

    /// A clip leaves the table, and its readers with it. `freeing` is false
    /// when its track's group is going, since freeing a group frees what is
    /// inside it.
    fn free_clip(&mut self, id: u64, freeing: bool, ops: &mut Vec<Op>) {
        let prefix = format!("reader:{id}:");
        let readers: Vec<Handle> = self
            .readers
            .keys()
            .filter(|handle| handle.starts_with(&prefix))
            .cloned()
            .collect();
        for handle in &readers {
            self.readers.remove(handle);
        }
        self.clips.remove(&id);
        if freeing {
            ops.push(Op::Free {
                handle: clip_handle(id),
                forget: readers,
            });
        }
    }

    /// Freeing the group frees everything inside it, so the clips and the
    /// meters only have to leave the table — which is what the track id in them
    /// is for. The buses are not the group's, so they are given back.
    fn free_track(&mut self, id: u64, ops: &mut Vec<Op>) {
        let mine: Vec<u64> = self
            .clips
            .iter()
            .filter(|(_, held)| held.track == id)
            .map(|(region, _)| *region)
            .collect();
        let mut forget = vec![meter_handle(id, 0), meter_handle(id, 1)];
        for region in mine {
            forget.push(clip_handle(region));
            forget.extend(self.reader_handles(region));
            self.free_clip(region, false, ops);
        }
        self.tracks.remove(&id);
        ops.push(Op::Free {
            handle: track_handle(id),
            forget,
        });
        ops.push(Op::FreeBus {
            handle: meter_bus_handle(id),
        });
    }
}

/// **A source table as JSON** — `{"<source id>": {"buffer", "channels"}}`,
/// the shape both clients send — read into what the plan takes. An id that is
/// not a number, or a table that is not one, reads as nothing rather than as
/// an error: a source the plan does not know is simply not playing yet.
pub fn sources_table(
    sources: &str,
) -> std::collections::HashMap<clausters_document::SourceId, SourceInfo> {
    serde_json::from_str::<std::collections::HashMap<String, SourceInfo>>(sources)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, info)| {
            id.parse::<u64>()
                .ok()
                .map(|id| (clausters_document::SourceId(id), info))
        })
        .collect()
}

/// [`Instance::reconcile`] against a multitrack and a source table given as JSON,
/// which is how the two client doors carry them.
///
/// The multitrack rather than the plan: the plan is a pure function of the multitrack and
/// crossing it out and back in would carry every curve's table twice for
/// nothing. [`clausters_document::multitrack::nodes::plan`] stays a door of its
/// own because it is worth reading on its own — this is the pair of calls a
/// client actually makes, as one.
///
/// An unreadable multitrack answers an empty list: there is no multitrack to say what
/// should be sounding, and tearing down what is would be an edit nobody made.
pub fn reconcile_json(
    instance: &mut Instance,
    multitrack: &str,
    sample_rate: f64,
    sources: &str,
    gain: f32,
) -> String {
    let Ok(multitrack) =
        serde_json::from_str::<clausters_document::multitrack::Multitrack>(multitrack)
    else {
        return "[]".into();
    };
    let table = sources_table(sources);
    let plan = clausters_document::multitrack::nodes::plan(&multitrack, sample_rate, &table);
    let ops = instance.reconcile(&plan, gain);
    serde_json::to_string(&ops).unwrap_or_else(|_| "[]".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use clausters_document::multitrack::nodes::plan;
    use clausters_document::multitrack::{Automation, Content, Multitrack, Region, Track};
    use clausters_document::points::Point;
    use clausters_document::{
        Lifetime, NodeId, Opaque, Second, SegmentRef, SegmentSource, SourceRef,
    };
    use serde_json::json;

    const RATE: f64 = 48_000.0;

    /// Two tracks; the first holds one box over a mono source.
    fn multitrack() -> Multitrack {
        let mut multitrack = Multitrack::default();
        multitrack.tracks.push(track(1, 2, vec![region(3, 77)]));
        multitrack.tracks.push(track(10, 11, Vec::new()));
        multitrack
    }

    fn track(id: u64, lane: u64, regions: Vec<Region>) -> Track {
        let mut track = Track::new(NodeId(id), NodeId(lane));
        track.channels = 1;
        track.lanes[0].regions = regions;
        track
    }

    fn region(id: u64, source: u64) -> Region {
        Region::new(
            NodeId(id),
            Second(0.0),
            Second(4.0),
            Content::Window {
                window: SegmentRef {
                    source: SegmentSource::Samples(SourceRef {
                        source: clausters_document::SourceId(source),
                        lifetime: Lifetime::Session,
                        generation: 0,
                        range: None,
                    }),
                    start: 0.0,
                    duration: 4.0,
                },
                playrate: 1.0,
                args: Opaque::none(),
                looping: false,
            },
        )
    }

    /// A gain curve, which is the only target the plan can hear.
    fn gain_curve(id: u64, top: f64) -> Automation {
        let mut curve = Automation::new(NodeId(id), Opaque(json!({ "port": "gain" })));
        curve.points = vec![
            Point {
                at: 0.0,
                value: 0.0,
                data: Opaque::none(),
            },
            Point {
                at: 4.0,
                value: top,
                data: Opaque::none(),
            },
        ];
        curve
    }

    fn sources() -> HashMap<clausters_document::SourceId, SourceInfo> {
        [
            (
                clausters_document::SourceId(77),
                SourceInfo {
                    buffer: 12,
                    channels: 1,
                },
            ),
            (
                clausters_document::SourceId(78),
                SourceInfo {
                    buffer: 13,
                    channels: 2,
                },
            ),
        ]
        .into_iter()
        .collect()
    }

    fn planned(multitrack: &Multitrack) -> Plan {
        plan(multitrack, RATE, &sources())
    }

    /// The ops of one kind, in order.
    fn of<'a>(ops: &'a [Op], name: &str) -> Vec<&'a Op> {
        ops.iter()
            .filter(|op| {
                serde_json::to_value(op)
                    .ok()
                    .and_then(|v| v["op"].as_str().map(|s| s == name))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// The whole point, and the one thing every other test rests on: a multitrack
    /// that did not move needs nothing done to it.
    #[test]
    fn a_multitrack_that_did_not_move_is_left_alone() {
        let multitrack = multitrack();
        let mut instance = Instance::new();
        let first = instance.reconcile(&planned(&multitrack), 0.5);
        assert!(!first.is_empty());
        let again = instance.reconcile(&planned(&multitrack), 0.5);
        assert_eq!(again, Vec::new(), "a resend is heard by nobody");
    }

    /// The defs go once, the barrier goes with them, and a second pass sends
    /// neither.
    #[test]
    fn the_defs_go_once_and_the_barrier_goes_with_them() {
        let multitrack = multitrack();
        let mut instance = Instance::new();
        let ops = instance.reconcile(&planned(&multitrack), 0.5);
        assert!(!of(&ops, "def").is_empty());
        assert_eq!(of(&ops, "barrier").len(), 1);
        let last = ops
            .iter()
            .position(|op| matches!(op, Op::Barrier))
            .expect("a barrier");
        assert!(
            ops[..last].iter().all(|op| matches!(op, Op::Def { .. })),
            "the batch is closed before anything else is sent"
        );
        assert!(of(&instance.reconcile(&planned(&multitrack), 0.5), "def").is_empty());
    }

    /// **The hand does not write a port a curve drives** *(2026-09-11)*. A
    /// mapped control is taken back by a plain set, so a multitrack reconciling on
    /// every edit was enough to silence an automation: adding a box to a track
    /// sent that track's gain and the curve went on writing a bus nobody read.
    #[test]
    fn the_hand_does_not_write_a_port_a_curve_drives() {
        let mut multitrack = multitrack();
        multitrack.tracks[0].automation.push(gain_curve(4, 1.0));
        let mut instance = Instance::new();
        let ops = instance.reconcile(&planned(&multitrack), 0.5);

        let track = of(&ops, "slot")
            .into_iter()
            .find(|op| matches!(op, Op::Slot { handle, .. } if handle == "track:1"))
            .expect("the track");
        let Op::Slot { ports, .. } = track else {
            unreachable!()
        };
        assert!(!ports.contains_key("gain"), "the curve owns it");
        assert!(ports.contains_key("mute"), "and only it");

        // And no later pass reaches for it either.
        let mut moved = multitrack.clone();
        moved.tracks[0].level = 0.25;
        for op in instance.reconcile(&planned(&moved), 0.5) {
            if let Op::Set { handle, ports } = op {
                assert!(
                    handle != "track:1" || !ports.contains_key("gain"),
                    "the hand reached for a driven port"
                );
            }
        }
    }

    /// The same box, re-pointed at a source of another width: another clip
    /// def, so the one change a move cannot carry.
    fn rewidened(multitrack: &Multitrack) -> Multitrack {
        let mut multitrack = multitrack.clone();
        let held = &mut multitrack.tracks[0].lanes[0].regions[0];
        let automation = std::mem::take(&mut held.automation);
        *held = region(3, 78);
        held.automation = automation;
        multitrack
    }

    /// **A clip that changed track is moved there, not set and not made
    /// again.** A clip is a slot inside its track's group, so the node carries
    /// no track id a set could update: a box dragged onto a muted track stayed
    /// audible and one dragged off it stayed silent (2026-09-11). It used to be
    /// freed and made again, which took with it everything hanging off the
    /// node; the server moves a slot now.
    #[test]
    fn a_clip_that_changed_track_is_moved_there() {
        let multitrack = multitrack();
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let mut moved = multitrack.clone();
        let region = moved.tracks[0].lanes[0].regions.remove(0);
        moved.tracks[1].lanes[0].regions.push(region);
        let ops = instance.reconcile(&planned(&moved), 0.5);

        assert!(
            ops.iter().any(|op| matches!(op, Op::Move { handle, target }
                                         if handle == "clip:3" && target == "track:10")),
            "moved onto the new track: {ops:?}"
        );
        assert!(
            !ops.iter().any(|op| matches!(op,
                Op::Free { handle, .. } | Op::Slot { handle, .. } | Op::Set { handle, .. }
                    if handle == "clip:3")),
            "neither freed, made again nor set: {ops:?}"
        );
        assert_eq!(
            instance.reconcile(&planned(&moved), 0.5),
            Vec::new(),
            "and the table says where it is now"
        );
    }

    /// **A move does not depend on which track is reached first.** A box going
    /// to an earlier track is met on the new track before the old one lets go
    /// of it, and a box going to a later one is left by the old track first —
    /// neither of which may read as the box being gone.
    #[test]
    fn a_clip_moved_to_an_earlier_track_is_moved_too() {
        let mut multitrack = multitrack();
        let region = multitrack.tracks[0].lanes[0].regions.remove(0);
        multitrack.tracks[1].lanes[0].regions.push(region);
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let mut moved = multitrack.clone();
        let region = moved.tracks[1].lanes[0].regions.remove(0);
        moved.tracks[0].lanes[0].regions.push(region);
        let ops = instance.reconcile(&planned(&moved), 0.5);

        assert!(
            ops.iter().any(|op| matches!(op, Op::Move { handle, target }
                                         if handle == "clip:3" && target == "track:1")),
            "{ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Free { handle, .. } if handle == "clip:3")),
            "{ops:?}"
        );
    }

    /// **What hangs off a moved clip stays on it**: its readers are inside its
    /// group, and a curve's map is on its port, so neither is made again — which
    /// is the whole reason for a move rather than a rebuild.
    #[test]
    fn a_moved_clip_keeps_its_readers_and_its_curve() {
        let mut multitrack = multitrack();
        multitrack.tracks[0].lanes[0].regions[0]
            .automation
            .push(gain_curve(5, 1.0));
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let mut moved = multitrack.clone();
        let region = moved.tracks[0].lanes[0].regions.remove(0);
        moved.tracks[1].lanes[0].regions.push(region);
        let ops = instance.reconcile(&planned(&moved), 0.5);

        assert!(
            !ops.iter().any(|op| matches!(op,
                Op::Free { handle, .. } | Op::Slot { handle, .. }
                    if handle.starts_with("reader:3:") || handle == "curve:5")),
            "the readers and the curve node stay: {ops:?}"
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Map { .. } | Op::Unmap { .. })),
            "the map is still on the port: {ops:?}"
        );
    }

    /// **A clip whose source changed width is made again**: a mono take is
    /// panned into its track and a stereo one balanced, so it is another clip
    /// def — the one change neither a set nor a move expresses.
    #[test]
    fn a_clip_of_another_width_is_made_again() {
        let multitrack = multitrack();
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let ops = instance.reconcile(&planned(&rewidened(&multitrack)), 0.5);

        let freed = ops
            .iter()
            .position(|op| matches!(op, Op::Free { handle, .. } if handle == "clip:3"))
            .expect("the old node goes");
        let made = ops
            .iter()
            .position(|op| {
                matches!(op, Op::Slot { handle, target, slot, .. }
                                    if handle == "clip:3" && target == "track:1"
                                    && *slot == mixer::clip_slot(2))
            })
            .expect("and it is made again as a stereo clip");
        assert!(freed < made, "freed before it is made again");
    }

    /// **A rebuilt clip's readers are made again with it** *(2026-09-11)*. They
    /// are slots inside the clip's group, so freeing it frees them — and a
    /// table that still held them would `set` a node that is gone.
    #[test]
    fn a_rebuilt_clip_s_readers_go_with_it() {
        let multitrack = multitrack();
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let ops = instance.reconcile(&planned(&rewidened(&multitrack)), 0.5);

        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Slot { handle, target, .. }
                                         if handle == "reader:3:0" && target == "clip:3")),
            "the reader is made again inside the new clip"
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Set { handle, .. } if handle == "reader:3:0")),
            "and never set against the node that was freed"
        );
    }

    /// **A curve whose owner was rebuilt is mapped again** *(2026-09-11)*. The
    /// map belongs to the node and not to the curve: the port that was mapped
    /// went away with the old clip while the curve went on writing a bus nobody
    /// reads, and since the hand does not send that port either, the box came
    /// back at the def's own default and played flat out.
    #[test]
    fn a_curve_whose_owner_was_rebuilt_is_mapped_again() {
        let mut multitrack = multitrack();
        multitrack.tracks[0].lanes[0].regions[0]
            .automation
            .push(gain_curve(5, 1.0));
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let ops = instance.reconcile(&planned(&rewidened(&multitrack)), 0.5);

        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Map { handle, port, bus }
                                         if handle == "clip:3" && port == "gain"
                                         && bus == "curvebus:5")),
            "the new node is mapped to the bus the curve still writes: {ops:?}"
        );
        // And the curve itself was not rebuilt: it is the same node and the
        // same bus, so nothing of it stopped.
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Free { handle, .. } if handle == "curve:5")),
            "the curve node stays"
        );
    }

    /// A curve whose points moved is a new table, and the old buffer is given
    /// back **after** the reader has been pointed at the new one.
    #[test]
    fn a_moved_curve_gets_a_new_table_and_the_old_one_back() {
        let mut multitrack = multitrack();
        multitrack.tracks[0].automation.push(gain_curve(4, 1.0));
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let mut drawn = multitrack.clone();
        drawn.tracks[0].automation[0] = gain_curve(4, 0.25);
        let ops = instance.reconcile(&planned(&drawn), 0.5);

        let made = ops
            .iter()
            .position(|op| matches!(op, Op::Buffer { handle, .. } if handle == "curvebuf:4:1"))
            .expect("a new table");
        let set = ops
            .iter()
            .position(|op| {
                matches!(op, Op::Set { handle, ports }
                                    if handle == "curve:4" && ports.contains_key("buf"))
            })
            .expect("the reader follows it");
        let freed = ops
            .iter()
            .position(|op| matches!(op, Op::FreeBuffer { handle } if handle == "curvebuf:4:0"))
            .expect("and the old one goes back");
        assert!(made < set && set < freed, "{ops:?}");
    }

    /// A curve the multitrack no longer has gives its port back. **Unmapping is not
    /// optional**: a port left mapped to a bus nobody writes holds whatever was
    /// in it, so a deleted curve would go on driving the control it drove, at
    /// the last value it happened to say.
    #[test]
    fn a_deleted_curve_gives_its_port_back() {
        let mut multitrack = multitrack();
        multitrack.tracks[0].automation.push(gain_curve(4, 1.0));
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let mut gone = multitrack.clone();
        gone.tracks[0].automation.clear();
        let ops = instance.reconcile(&planned(&gone), 0.5);
        assert!(ops.iter().any(|op| matches!(op, Op::Unmap { handle, port }
                                            if handle == "track:1" && port == "gain")));
        assert_eq!(of(&ops, "freeBus").len(), 1);
        assert_eq!(of(&ops, "freeBuffer").len(), 1);
        // The fader is the hand's again, and this is the pass that says so.
        assert!(ops.iter().any(|op| matches!(op, Op::Set { handle, ports }
                                            if handle == "track:1" && ports.contains_key("gain"))));
    }

    /// A track that went away takes its clips out of the table with it, and
    /// gives its meter buses back — the group frees what is inside it, the
    /// buses are not the group's.
    #[test]
    fn a_track_that_went_away_takes_its_clips_and_gives_its_buses_back() {
        let multitrack = multitrack();
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let mut gone = multitrack.clone();
        gone.tracks.remove(0);
        let ops = instance.reconcile(&planned(&gone), 0.5);
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Free { handle, .. } if handle == "track:1"))
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::FreeBus { handle } if handle == "meterbus:1"))
        );
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Free { handle, .. } if handle == "clip:3")),
            "the group freed it; a second free would name a node that is gone"
        );
        // And the client is told to drop them, so its table does not grow by
        // every box a session ever removed.
        let track = ops
            .iter()
            .find(|op| matches!(op, Op::Free { handle, .. } if handle == "track:1"))
            .expect("the track");
        let Op::Free { forget, .. } = track else {
            unreachable!()
        };
        assert!(forget.contains(&"clip:3".to_string()));
        assert!(forget.contains(&"reader:3:0".to_string()));
        assert!(forget.contains(&"meter:1:0".to_string()));

        // And the multitrack is whole afterwards: nothing of the vanished track is
        // still believed to be sounding.
        assert_eq!(instance.reconcile(&planned(&gone), 0.5), Vec::new());
    }

    /// The meters are two slots over one run of `2 * channels`, and the run is
    /// what a host reads.
    #[test]
    fn a_track_is_metered_twice_over_one_run() {
        let multitrack = multitrack();
        let mut instance = Instance::new();
        let ops = instance.reconcile(&planned(&multitrack), 0.5);
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Bus { handle, channels }
                                   if handle == "meterbus:1" && *channels == 2))
        );
        let meters: Vec<&Op> = of(&ops, "slot")
            .into_iter()
            .filter(|op| matches!(op, Op::Slot { slot, .. } if slot == "meters"))
            .collect();
        assert_eq!(meters.len(), 4, "two per track, two tracks");
        assert_eq!(
            instance.meters(),
            vec![
                (1, "meterbus:1".to_string(), 1),
                (10, "meterbus:10".to_string(), 1),
            ]
        );
    }

    /// Everything made is given back, and an instance torn down believes
    /// nothing is sounding.
    #[test]
    fn a_teardown_gives_back_what_it_made() {
        let mut multitrack = multitrack();
        multitrack.tracks[0].automation.push(gain_curve(4, 1.0));
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let ops = instance.teardown();
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Free { handle, .. } if handle == TRANSPORT))
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Free { handle, .. } if handle == CURVES))
        );
        assert_eq!(of(&ops, "freeBus").len(), 3, "two meters and one curve");
        assert!(!instance.is_sounding());
        // The defs stay known: they are on the server whatever this holds, and
        // sending them again would be sending what is already there.
        assert!(
            instance
                .reconcile(&planned(&multitrack), 0.5)
                .iter()
                .all(|op| !matches!(op, Op::Def { .. }))
        );
    }

    /// An op is one JSON object with `op` naming it, which is what both doors
    /// carry and what a client matches on.
    #[test]
    fn an_op_is_named_by_what_it_is() {
        let map = serde_json::to_value(Op::Map {
            handle: "clip:3".into(),
            port: "gain".into(),
            bus: "curvebus:5".into(),
        })
        .expect("JSON");
        assert_eq!(
            map,
            json!({ "op": "map", "handle": "clip:3", "port": "gain", "bus": "curvebus:5" })
        );

        let slot = serde_json::to_value(Op::Slot {
            handle: "meter:1:0".into(),
            target: "track:1".into(),
            slot: "meters".into(),
            ports: [
                (
                    "meter/out1".to_string(),
                    Port::Bus {
                        bus: "meterbus:1".into(),
                        offset: 1,
                    },
                ),
                ("meter/hold".to_string(), Port::Number(0.0)),
            ]
            .into_iter()
            .collect(),
        })
        .expect("JSON");
        assert_eq!(
            slot["ports"]["meter/out1"],
            json!({ "bus": "meterbus:1", "offset": 1 })
        );
        assert_eq!(slot["ports"]["meter/hold"], json!(0.0));
    }

    /// **A box that went takes its envelope's map with it** *(found 2026-09-12
    /// by the user, deleting a box in `edit_multitrack`)*. The curve's own
    /// node, buffer and bus still go back — those are the instance's and
    /// outlive whatever port they drove — but the unmap named a node that had
    /// just been freed, which is a handle whose table entry went with it.
    #[test]
    fn a_curve_whose_owner_is_gone_is_not_unmapped() {
        let mut multitrack = multitrack();
        multitrack.tracks[0].lanes[0].regions[0]
            .automation
            .push(gain_curve(5, 1.0));
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);

        let mut gone = multitrack.clone();
        gone.tracks[0].lanes[0].regions.clear();
        let ops = instance.reconcile(&planned(&gone), 0.5);
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Free { handle, .. } if handle == "clip:3")),
            "the box goes"
        );
        assert!(
            !ops.iter().any(|op| matches!(op, Op::Unmap { .. })),
            "and nothing names it afterwards: {ops:?}"
        );
        // What the curve itself holds is still given back.
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::Free { handle, .. } if handle == "curve:5"))
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Op::FreeBus { handle } if handle == "curvebus:5"))
        );

        // And a track that goes is the same case.
        let mut multitrack = super::tests::multitrack();
        multitrack.tracks[0].automation.push(gain_curve(4, 1.0));
        let mut instance = Instance::new();
        instance.reconcile(&planned(&multitrack), 0.5);
        let mut gone = multitrack.clone();
        gone.tracks.remove(0);
        let ops = instance.reconcile(&planned(&gone), 0.5);
        assert!(
            !ops.iter().any(|op| matches!(op, Op::Unmap { .. })),
            "{ops:?}"
        );
    }
}
