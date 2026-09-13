//! **What is sounding**: the piece's instance, and the ops that make it match.
//!
//! A standalone host plays a piece the way every other endpoint does — through
//! [`clausters_document::multitrack::nodes::plan`], which says what a piece
//! *needs*, and [`clausters_editing::instance::Instance`], which answers the
//! **difference** between that and what a server already holds. Neither of them
//! opens a socket or allocates anything, and that is the seam: `instance.rs`'s
//! own header says *"the client's half is exactly: a table, an allocator, and a
//! socket"*, and **a standalone host is a client in that sentence**. This
//! module is those three.
//!
//! # What it replaced, and why that was the bug factory
//!
//! `document::sound` was the host's own reconciler: a reader per region and
//! channel, diffed against a remembered `Reading`, played through the host's
//! one buffer-player def. It worked, and it was a **second implementation of
//! the multitrack application** — its own module doc said *"it lives here
//! rather than in a script because there is one of it"* while
//! `clausters-editing` said, of the crate's reconciler, *"usable by a
//! standalone host with no client in the process"*. Two modules, each claiming
//! to be the only one. Every defect of 2026-09-12 was a slice of that split
//! found one at a time.
//!
//! What the piece gains by binding the real one is everything the strip is and
//! the readers were not: a **clip's own gain and mute** before the track's, a
//! **track's** gain, mute and the mixer's rule about solo, **automation heard**
//! rather than only drawn (a curve is a table in a buffer driving a port), the
//! widths rule that picks a mono or a stereo clip def, and the **meters** the
//! plan allocates. None of that was reachable from a reader with a `chan` and
//! an `out`.
//!
//! # The three halves, and what each one is allowed to know
//!
//! - **The table** ([`Playing`]): handle → node id, control bus, buffer. An op
//!   never carries a number this host did not make, so the table is the only
//!   place a handle becomes one.
//! - **The allocator**: node ids from [`play::PIECE_NODE`] up (past the
//!   monitor's fixed window), control buses and buffers from a base the caller
//!   gives it. It is the host's because *"a bus allocator is a property of a
//!   running session and not of a piece"*.
//! - **The socket**: [`Host::send_to_player`], the same one every other
//!   message to this host's server goes through.
//!
//! Nothing here decides *what* to play. A question about order, about which
//! node a port belongs to, or about what a curve's table holds is the crate's,
//! and if the answer looks wrong the fix goes there — where both clients read
//! it too.

use std::collections::HashMap;

use clausters_core::osc::{OscMessage, OscType};
use clausters_document::SourceId;
use clausters_document::multitrack::nodes::{self, Plan, SourceInfo};
use clausters_editing::instance::{Handle, Instance, Op, Port, Ports};

use crate::host::diag;
use crate::host::play;
use crate::host::{Host, document, instance};

/// How many control buses a run may hold before this refuses it — a guard on
/// arithmetic rather than a policy: a plan asking for a thousand-channel meter
/// is a plan that has gone wrong somewhere else.
const MAX_BUS_RUN: usize = 64;

/// **A piece, as it is playing**: what the reconciler knows, plus the numbers
/// only a running session has.
#[derive(Debug)]
pub struct Playing {
    /// What was made from the last plan — the crate's memory, not ours.
    instance: Instance,
    /// The node a handle became.
    nodes: HashMap<Handle, i32>,
    /// The first control bus of the run a handle became, and how long it is.
    buses: HashMap<Handle, (i32, usize)>,
    /// The buffer a handle became.
    buffers: HashMap<Handle, i32>,
    /// The next node id, counted up from the piece's base.
    next_node: i32,
    /// The next control bus, counted up from the base the caller set.
    next_bus: i32,
    /// The next buffer number, likewise.
    next_buffer: i32,
}

impl Default for Playing {
    /// An instance allocating from the piece's own node base and from zero for
    /// the rest — what a host with no session behind it would use, and what
    /// [`Playing::new`] overrides the moment a session says where its own
    /// buffers stopped.
    fn default() -> Self {
        Playing {
            instance: Instance::default(),
            nodes: HashMap::new(),
            buses: HashMap::new(),
            buffers: HashMap::new(),
            // **Never zero.** Node 0 is the root group, and a table that minted
            // it would free the server's whole tree on the first reconcile.
            next_node: play::PIECE_NODE,
            next_bus: 0,
            next_buffer: 0,
        }
    }
}

impl Playing {
    /// An instance that has made nothing yet, allocating from these bases.
    ///
    /// The buffer base is the caller's because a session's **sources** are
    /// already in buffers by the time anything plays: the curve tables go above
    /// them, and a host that guessed would write a table over a take.
    pub fn new(first_bus: i32, first_buffer: i32) -> Self {
        Playing {
            next_bus: first_bus,
            next_buffer: first_buffer,
            ..Playing::default()
        }
    }

    /// The meters the piece is writing, as `(track id, first bus, channels)` —
    /// what a mixer strip is drawn from.
    pub fn meters(&self) -> Vec<(u64, i32, usize)> {
        self.instance
            .meters()
            .into_iter()
            .filter_map(|(track, handle, channels)| {
                let (bus, _) = self.buses.get(&handle)?;
                Some((track, *bus, channels))
            })
            .collect()
    }

    /// Whether anything is playing at all.
    pub fn is_sounding(&self) -> bool {
        self.instance.is_sounding()
    }

    /// How many nodes the piece is holding — what a caller reports when it says
    /// it built something.
    pub fn nodes(&self) -> usize {
        self.nodes.len()
    }

    /// **The difference between the plan and what is made, as messages.**
    ///
    /// The order is the crate's and this only carries it out, which is the
    /// whole reason a second endpoint cannot carry it out differently.
    fn reconcile(&mut self, plan: &Plan, gain: f32) -> Vec<OscMessage> {
        let ops = self.instance.reconcile(plan, gain);
        self.apply(ops)
    }

    /// Everything freed, as messages. The piece itself is untouched: what an
    /// instance holds is nodes, and nodes are not the composition.
    fn teardown(&mut self) -> Vec<OscMessage> {
        let ops = self.instance.teardown();
        let out = self.apply(ops);
        self.nodes.clear();
        self.buses.clear();
        self.buffers.clear();
        out
    }

    /// One op as the messages it is on this wire.
    ///
    /// The **only** place this host turns the crate's vocabulary into the
    /// server's. An op it cannot carry out is said out loud rather than
    /// skipped: a handle with nothing behind it means a table and a plan that
    /// have gone out of step, and a piece that half-plays is worse than one
    /// that says why.
    fn apply(&mut self, ops: Vec<Op>) -> Vec<OscMessage> {
        let mut out = Vec::new();
        for op in ops {
            match op {
                Op::Def { family, spec } => out.push(message(
                    "/def_send",
                    vec![OscType::String(family), OscType::String(spec.to_string())],
                )),
                // **Sent, not waited on.** A barrier exists because a def send
                // answers `/done` and a client that waits must not take another
                // command's; this host's messages reach its server in order (a
                // ring for an embedded one, one socket for an attached one), so
                // the sync is sent for whoever is counting and nothing blocks
                // the frame that produced the edit.
                Op::Barrier => out.push(message("/server_sync", vec![OscType::Int(0)])),
                Op::Graph {
                    handle,
                    graph,
                    ports,
                } => {
                    let node = self.mint_node(&handle);
                    let mut args = vec![
                        OscType::String(graph),
                        OscType::Int(node),
                        // At the tail of the group the transport governs, which
                        // is where the monitor's readers live too: one group,
                        // because there is one transport.
                        OscType::Int(1),
                        OscType::Int(play::take_group()),
                    ];
                    args.extend(self.port_args(&ports));
                    out.push(message("/graph_new", args));
                }
                // **The one op this endpoint answers differently, and it says
                // why.** The op means *the engine owns the piece's time*, and
                // a client with nothing else playing satisfies it by binding
                // the piece's own graph. This host also plays a **take
                // monitor**, and there is one transport per server: binding the
                // piece would release the group the monitor lives in, so the
                // monitor would run free of the transport that is supposed to
                // start, stop and locate it.
                //
                // So the governed group stays the one this host made at boot
                // (`play::take_group`) and the piece's graph is created
                // **inside** it -- which satisfies the op's intent exactly,
                // since what the engine freezes is a subtree. Ignoring it
                // silently would be the divergence; this is the endpoint's own
                // arrangement of its nodes, which is the half a client owns.
                Op::Transport { handle } => {
                    if let Some(node) = self.node(&handle) {
                        diag::debug!(
                            "the piece is node {node}, inside the group the transport already \
                             governs ({}) -- this host binds no second one",
                            play::take_group()
                        );
                    }
                }
                Op::Group { handle, before } => {
                    let Some(target) = self.node(&before) else {
                        continue;
                    };
                    let node = self.mint_node(&handle);
                    out.push(message(
                        "/group_new",
                        vec![
                            OscType::Int(node),
                            // Before the node named, which is what the op says.
                            OscType::Int(2),
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
                        continue;
                    };
                    let node = self.mint_node(&handle);
                    let mut args = vec![
                        OscType::Int(instance),
                        OscType::String(slot),
                        OscType::Int(node),
                    ];
                    args.extend(self.port_args(&ports));
                    out.push(message("/graph_addSlot", args));
                }
                Op::Synth {
                    handle,
                    def,
                    target,
                    ports,
                } => {
                    let Some(group) = self.node(&target) else {
                        continue;
                    };
                    let node = self.mint_node(&handle);
                    let mut args = vec![
                        OscType::String(def),
                        OscType::Int(node),
                        OscType::Int(1),
                        OscType::Int(group),
                    ];
                    args.extend(self.port_args(&ports));
                    out.push(message("/synth_new", args));
                }
                // **A control bus is allocated and never asked for.** The
                // server holds a pool and the client owns the indices, which is
                // the same arrangement every other client here has.
                Op::Bus { handle, channels } => {
                    let channels = channels.clamp(1, MAX_BUS_RUN);
                    let first = self.next_bus;
                    self.next_bus += channels as i32;
                    self.buses.insert(handle, (first, channels));
                }
                Op::Buffer { handle, samples } => {
                    let bufnum = self.next_buffer;
                    self.next_buffer += 1;
                    self.buffers.insert(handle, bufnum);
                    out.push(message(
                        "/buffer_alloc",
                        vec![
                            OscType::Int(bufnum),
                            OscType::Int(samples.len().max(1) as i32),
                            OscType::Int(1),
                        ],
                    ));
                    if !samples.is_empty() {
                        let mut bytes = Vec::with_capacity(samples.len() * 4);
                        for value in &samples {
                            bytes.extend_from_slice(&value.to_le_bytes());
                        }
                        out.push(message(
                            "/buffer_setRange",
                            vec![OscType::Int(bufnum), OscType::Int(0), OscType::Blob(bytes)],
                        ));
                    }
                }
                Op::Set { handle, ports } => {
                    let Some(node) = self.node(&handle) else {
                        continue;
                    };
                    let mut args = vec![OscType::Int(node)];
                    args.extend(self.port_args(&ports));
                    out.push(message("/node_set", args));
                }
                Op::Map { handle, port, bus } => {
                    let (Some(node), Some((index, _))) =
                        (self.node(&handle), self.buses.get(&bus).copied())
                    else {
                        continue;
                    };
                    out.push(message(
                        "/graph_map",
                        vec![
                            OscType::Int(node),
                            OscType::String(port),
                            OscType::Int(index),
                        ],
                    ));
                }
                // **A handle with nothing behind it is a node that is already
                // gone**, and a message naming one would reach whatever holds
                // that id next. The reconciler does not emit these -- freeing a
                // node takes its map with it -- and this is the other half of
                // that, said the same way the Python client says it.
                Op::Unmap { handle, port } => {
                    if let Some(node) = self.node(&handle) {
                        out.push(message(
                            "/graph_map",
                            vec![OscType::Int(node), OscType::String(port), OscType::Int(-1)],
                        ));
                    }
                }
                Op::Free { handle, forget } => {
                    if let Some(node) = self.nodes.remove(&handle) {
                        out.push(message("/node_free", vec![OscType::Int(node)]));
                    }
                    // Freeing a group frees what is inside it, so these only
                    // leave the table: a second free would name a node that is
                    // already gone.
                    for handle in forget {
                        self.nodes.remove(&handle);
                    }
                }
                Op::FreeBus { handle } => {
                    self.buses.remove(&handle);
                }
                Op::FreeBuffer { handle } => {
                    if let Some(bufnum) = self.buffers.remove(&handle) {
                        out.push(message("/buffer_free", vec![OscType::Int(bufnum)]));
                    }
                }
            }
        }
        out
    }

    /// The node a handle is, saying so when it is nothing.
    fn node(&self, handle: &Handle) -> Option<i32> {
        match self.nodes.get(handle) {
            Some(node) => Some(*node),
            None => {
                diag::warn!("the piece names {handle}, which this host never made");
                None
            }
        }
    }

    /// **A buffer number from the one allocator**, for something made outside
    /// a plan — a join a hand minted. Here rather than beside the sources
    /// because there is one space and this is what counts through it.
    pub(crate) fn mint_buffer(&mut self) -> i32 {
        let bufnum = self.next_buffer;
        self.next_buffer += 1;
        bufnum
    }

    /// A fresh node id for a handle, remembered.
    fn mint_node(&mut self, handle: &Handle) -> i32 {
        let node = self.next_node;
        self.next_node += 1;
        self.nodes.insert(handle.clone(), node);
        node
    }

    /// A port list as OSC pairs, resolving every reference through the tables.
    ///
    /// A port naming a resource this host did not make is **dropped**, and the
    /// rest of the node is still set: the alternative is a whole strip that
    /// does not arrive because one curve's buffer went missing.
    fn port_args(&self, ports: &Ports) -> Vec<OscType> {
        let mut args = Vec::new();
        for (name, port) in ports {
            let value = match port {
                Port::Number(value) => *value as f32,
                Port::Bus { bus, offset } => match self.buses.get(bus) {
                    Some((first, _)) => (first + *offset as i32) as f32,
                    None => continue,
                },
                Port::Buffer { buffer } => match self.buffers.get(buffer) {
                    Some(bufnum) => *bufnum as f32,
                    None => continue,
                },
            };
            args.push(OscType::String(name.clone()));
            args.push(OscType::Float(value));
        }
        args
    }
}

/// One message, spelled the way every other one in this host is.
fn message(addr: &str, args: Vec<OscType>) -> OscMessage {
    OscMessage {
        addr: addr.into(),
        args,
    }
}

impl Host {
    /// **Where this host's piece allocates from**, once a session has said
    /// where its own buffers stopped.
    ///
    /// One allocator over one space, which is the whole point of taking the
    /// number rather than guessing a base: the session's sources are in buffers
    /// already, and a curve's table written over a take is a piece that plays
    /// somebody else's audio through a fader.
    pub fn play_piece_from(&mut self, first_bus: i32, first_buffer: i32) {
        self.instance = instance::Playing::new(first_bus, first_buffer);
    }

    /// **Makes the sources an edit minted**, so what names them can be drawn
    /// and heard.
    ///
    /// A join states *there is a source N made of these spans* and rides that
    /// statement on the intent, because a source table is the session's and a
    /// piece is not. Three things follow from it and this does all three: the
    /// **session** learns it (so a save carries it), the **table** learns which
    /// buffer it is (so a box draws and the plan can play it), and the
    /// **server** is told to make it.
    ///
    /// The buffer number comes from the one allocator this host has, which is
    /// what keeps a minted join from being written over a take.
    pub(crate) fn mint_sources(
        &mut self,
        intents: &[(
            clausters_document::multitrack::edit::MultitrackIntent,
            &'static str,
        )],
    ) {
        use clausters_document::multitrack::edit::MultitrackIntent;

        let mut made = Vec::new();
        for (intent, _) in intents {
            let MultitrackIntent::JoinRegions {
                source: Some(minted),
                ..
            } = intent
            else {
                continue;
            };
            made.push(minted.clone());
        }
        if made.is_empty() {
            return;
        }
        let Some(owner) = self.owner.as_mut() else {
            return;
        };
        let mut messages = Vec::new();
        for minted in made {
            // **Written over rather than skipped**: the document has just said
            // what this source is, and what the piece now names is this.
            if let Some(session) = owner.session.as_mut() {
                session.sources.insert(minted.id, minted.source.clone());
            }
            let held = document::sources::held_takes(&owner.takes);
            let Some(stitch) = clausters_editing::sources::stitch(&minted.source, &held) else {
                diag::warn!(
                    "source {} is a join over samples this host has not loaded",
                    minted.id.0
                );
                continue;
            };
            let bufnum = self.instance.mint_buffer();
            owner.takes.insert(
                minted.id,
                document::sources::Take {
                    bufnum,
                    channels: Some(stitch.channels as u32),
                    frames: Some(stitch.frames),
                },
            );
            messages.push(document::sources::stitch_message(bufnum, &stitch));
        }
        for message in messages {
            self.send_to_player(message);
        }
    }

    /// **Makes what sounds be what the piece says.**
    ///
    /// One call, whether it is the first time or after any edit: the plan is
    /// derived from the piece, the reconciler answers the difference, and this
    /// sends it. A node that did not change costs nothing, and an edit reaches
    /// a node that is already running — so a box moved while the piece plays is
    /// heard where it was dropped, with nothing that is sounding cut.
    ///
    /// It is a no-op for a host with no piece and for one with no server — a
    /// session opens, edits, undoes and saves without either.
    pub fn sound_piece(&mut self) -> usize {
        let Some(owner) = self.owner.as_ref() else {
            return 0;
        };
        if !owner.draws_piece() || self.player().is_none() {
            return 0;
        }
        let look = owner.piece_look();
        let sources: HashMap<SourceId, SourceInfo> = owner
            .takes
            .iter()
            .map(|(id, take)| {
                (
                    *id,
                    SourceInfo {
                        buffer: take.bufnum,
                        channels: take.channels.unwrap_or(1).max(1) as usize,
                    },
                )
            })
            .collect();
        // The default tempo is the picture's own, so a piece that states no
        // tempo sounds at the rate it is drawn at rather than at two.
        let plan = nodes::plan(
            &owner.piece,
            look.rate,
            document::piece::DEFAULT_TEMPO * 60.0,
            &sources,
        );
        let messages = self.instance.reconcile(&plan, 1.0);
        diag::debug!(
            "sound_piece: {} message(s), {} node(s)",
            messages.len(),
            self.instance.nodes()
        );
        for message in messages {
            self.send_to_player(message);
        }
        self.instance.nodes()
    }

    /// Frees everything the piece made — what closing a window owes the server,
    /// and what a host that stops owning a piece owes it.
    pub fn hush_piece(&mut self) {
        for message in self.instance.teardown() {
            self.send_to_player(message);
        }
    }

    /// How many nodes the piece is playing through, for a caller reporting what
    /// it built.
    pub fn sounding_count(&self) -> usize {
        self.instance.nodes()
    }

    /// **Rolls or freezes the piece**, answering which it did.
    ///
    /// One verb, because the transport has one: `stop` freezes the governed
    /// group with every node's state intact and `play` thaws it, so pressing
    /// twice *continues* rather than starting the piece over. There is no
    /// "load" step and nothing to re-cue -- the nodes are resident and the
    /// position is the engine's.
    ///
    /// `None` when there is no piece sounding, which is what tells the caller
    /// to fall through to whatever else the key meant.
    pub fn roll_piece(&mut self) -> Option<bool> {
        if !self.instance.is_sounding() {
            return None;
        }
        self.piece_rolling = !self.piece_rolling;
        diag::info!(
            "the piece is {}",
            if self.piece_rolling {
                "rolling"
            } else {
                "frozen"
            }
        );
        self.send_to_player(OscMessage {
            addr: if self.piece_rolling {
                "/transport_play".into()
            } else {
                "/transport_stop".into()
            },
            args: vec![],
        });
        Some(self.piece_rolling)
    }

    /// Whether the piece is rolling.
    pub fn piece_rolling(&self) -> bool {
        self.piece_rolling
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_document::multitrack::{Content, Multitrack, Region, Track};
    use clausters_document::{
        Beat, Lifetime, NodeId, Opaque, SegmentRef, SegmentSource, SourceRef,
    };

    fn window(source: u64, start: f64) -> Content {
        Content::Window {
            window: SegmentRef {
                source: SegmentSource::Samples(SourceRef {
                    source: SourceId(source),
                    lifetime: Lifetime::Session,
                    generation: 0,
                    range: None,
                }),
                start,
                duration: 2.0,
            },
            playrate: 1.0,
            args: Opaque::none(),
            looping: false,
        }
    }

    /// One stereo take, in buffer 7.
    fn sources() -> HashMap<SourceId, SourceInfo> {
        [(
            SourceId(1),
            SourceInfo {
                buffer: 7,
                channels: 2,
            },
        )]
        .into_iter()
        .collect()
    }

    /// Two tracks, a box on each, over the one take.
    fn piece() -> Multitrack {
        let mut first = Track::new(NodeId(10), NodeId(11));
        first.name = Some("one".into());
        first.lanes[0].regions = vec![Region::new(
            NodeId(12),
            Beat(2.0),
            Beat(4.0),
            window(1, 0.5),
        )];
        let mut second = Track::new(NodeId(20), NodeId(21));
        second.lanes[0].regions = vec![Region::new(
            NodeId(22),
            Beat(0.0),
            Beat(2.0),
            window(1, 0.0),
        )];
        Multitrack {
            tracks: vec![first, second],
            ..Multitrack::default()
        }
    }

    fn plan_of(piece: &Multitrack) -> Plan {
        nodes::plan(piece, 48_000.0, 120.0, &sources())
    }

    fn addrs(messages: &[OscMessage]) -> Vec<&str> {
        messages.iter().map(|m| m.addr.as_str()).collect()
    }

    /// **A piece becomes the messages that play it**, in the crate's order: the
    /// defs it is made of, the barrier that closes them, the piece's own graph,
    /// and the transport binding that makes the engine own its time.
    ///
    /// What this asserts is the *shape* and not the contents: which defs a piece
    /// needs and what a clip's ports are belong to the shared mixer and to the
    /// crate's reconciler, and both are tested there. What can only go wrong
    /// here is the translation.
    #[test]
    fn a_piece_becomes_the_messages_that_play_it() {
        let mut playing = Playing::new(0, 100);
        let messages = playing.reconcile(&plan_of(&piece()), 1.0);
        let addrs = addrs(&messages);
        assert!(
            addrs.starts_with(&["/def_send"]),
            "the defs come first: {addrs:?}"
        );
        let barrier = addrs
            .iter()
            .position(|a| *a == "/server_sync")
            .expect("a barrier closes the defs");
        let graph = addrs
            .iter()
            .position(|a| *a == "/graph_new")
            .expect("the piece is a graph");
        assert!(barrier < graph, "nothing names a def before it is sent");
        // **No second transport group.** The piece's graph is created inside
        // the one this host already governs, which is what keeps the take
        // monitor frozen and thawed by the same transport the piece is.
        assert!(
            !addrs.contains(&"/transport_group"),
            "the host binds one governed group, at boot: {addrs:?}"
        );
        assert!(
            addrs.contains(&"/graph_addSlot"),
            "the tracks and the boxes are slots of it: {addrs:?}"
        );
    }

    /// **Never node 0.** The root group is node 0, so an allocator that started
    /// there would free the server's whole tree on the first thing it reaped.
    #[test]
    fn the_nodes_it_mints_start_past_the_monitors_own() {
        let mut playing = Playing::new(0, 100);
        playing.reconcile(&plan_of(&piece()), 1.0);
        let mut minted: Vec<i32> = playing.nodes.values().copied().collect();
        minted.sort_unstable();
        assert!(!minted.is_empty());
        assert!(
            minted[0] >= play::PIECE_NODE,
            "a piece's nodes begin past the take monitor's fixed window: {minted:?}"
        );
    }

    /// **An edit reaches a node that is already running.** The second pass over
    /// an unchanged piece says nothing at all, and over a moved box it is a
    /// `/node_set` — never a free and a new node, which would cut what is
    /// sounding on every drag.
    #[test]
    fn an_edit_sets_a_live_node_and_an_unchanged_piece_says_nothing() {
        let mut playing = Playing::new(0, 100);
        let mut piece = piece();
        playing.reconcile(&plan_of(&piece), 1.0);
        let made = playing.nodes.len();

        assert!(
            playing.reconcile(&plan_of(&piece), 1.0).is_empty(),
            "a piece that did not move costs nothing"
        );

        piece.tracks[0].lanes[0].regions[0].position = Beat(6.0);
        let messages = playing.reconcile(&plan_of(&piece), 1.0);
        let addrs = addrs(&messages);
        assert!(!messages.is_empty(), "the box moved");
        assert!(
            addrs.iter().all(|a| *a == "/node_set"),
            "a move is a set on a live node: {addrs:?}"
        );
        assert_eq!(playing.nodes.len(), made, "and nothing was made or freed");
    }

    /// **A port names a resource this host made, and is resolved through the
    /// table that made it.** One naming nothing is dropped and the rest of the
    /// node still arrives — a whole strip that does not come because one
    /// curve's buffer went missing is worse than a strip with a port unset.
    #[test]
    fn a_port_resolves_through_the_table_and_a_missing_one_is_dropped() {
        let mut playing = Playing::new(64, 100);
        playing.buses.insert("meter/1".into(), (64, 2));
        playing.buffers.insert("curve/1".into(), 103);
        let ports: Ports = [
            ("gain".to_string(), Port::Number(0.5)),
            (
                "out".to_string(),
                Port::Bus {
                    bus: "meter/1".into(),
                    offset: 1,
                },
            ),
            (
                "buf".to_string(),
                Port::Buffer {
                    buffer: "curve/1".into(),
                },
            ),
            (
                "nowhere".to_string(),
                Port::Buffer {
                    buffer: "curve/9".into(),
                },
            ),
        ]
        .into_iter()
        .collect();
        let args = playing.port_args(&ports);
        let named: Vec<String> = args
            .chunks(2)
            .filter_map(|pair| match pair.first() {
                Some(OscType::String(name)) => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            named,
            ["buf", "gain", "out"],
            "the one with no resource left"
        );
        assert!(args.contains(&OscType::Float(65.0)), "the bus, one along");
        assert!(args.contains(&OscType::Float(103.0)), "the buffer's number");
    }

    /// **Everything freed, and the tables with it.** What an instance holds is
    /// nodes; the piece is untouched, which is why a host can hush and sound
    /// again without the composition noticing.
    #[test]
    fn a_teardown_frees_what_it_made_and_forgets_it() {
        let mut playing = Playing::new(0, 100);
        playing.reconcile(&plan_of(&piece()), 1.0);
        assert!(playing.is_sounding());
        let messages = playing.teardown();
        assert!(
            addrs(&messages).contains(&"/node_free"),
            "the groups go: {:?}",
            addrs(&messages)
        );
        assert!(!playing.is_sounding());
        assert!(playing.nodes.is_empty() && playing.buses.is_empty());
    }
}
