//! **What is sounding**: the piece's instance, and the steps that make it match.
//!
//! A standalone host plays a piece the way every other endpoint does — through
//! [`clausters_document::multitrack::nodes::plan`], which says what a piece
//! *needs*, [`clausters_editing::instance::Instance`], which answers the
//! **difference** between that and what a server already holds, and
//! [`clausters_editing::apply::Applier`], which turns that difference into the
//! messages and the waits that carry it out. None of the three opens a socket,
//! and that is the seam: *"the client's half is exactly: a table, an
//! allocator, and a socket"*, and **a standalone host is a client in that
//! sentence**. The table is the applier's, the allocator is the host's
//! [`IdSpaces`](clausters_core::ids::IdSpaces) (`ids.rs`), and this module is
//! the socket: it sends the steps in order and holds them where one waits for
//! the server.
//!
//! # Why the host does not carry out ops itself any more
//!
//! It did, twice over. `document::sound` was the host's own reconciler, and
//! when that went, the ops were turned into messages here while the Python
//! client turned them into messages in `playback.py` — two translations of one
//! vocabulary, and the host's was the one that sent a buffer's fill before the
//! buffer existed and went silent. The applier is that translation, once, and
//! every endpoint binds it.
//!
//! # The waits
//!
//! A step that waits (`/done` of an asynchronous command, a `/server_sync`
//! barrier) holds everything after it until the matching reply comes back
//! through [`Host::on_server_reply`], which both fronts call for every reply.
//! Nothing blocks a frame: the steps queue, and the reply path drains them.
//!
//! Nothing here decides *what* to play. A question about order, about which
//! node a port belongs to, or about what a curve's table holds is the crate's,
//! and if the answer looks wrong the fix goes there — where both clients read
//! it too.

use std::collections::{HashMap, VecDeque};

use clausters_core::ids::{IdError, IdSpaces, Space};
use clausters_core::osc::{OscMessage, OscType};
use clausters_document::SourceId;
use clausters_document::multitrack::nodes::{self, Plan, SourceInfo};
use clausters_editing::apply::{Applier, Endpoint, Step};
use clausters_editing::instance::Instance;

use crate::host::diag;
use crate::host::{Host, document};

/// **A piece, as it is playing**: what the reconciler knows, the applier's
/// tables, and the steps not yet sent.
#[derive(Debug, Default)]
pub struct Playing {
    /// What was made from the last plan — the crate's memory, not ours.
    instance: Instance,
    /// Handle → node, bus and buffer, made on the first plan: its target is
    /// the governed group, which is the host's once a player is attached.
    applier: Option<Applier>,
    /// Steps not sent yet, in order.
    queue: VecDeque<Step>,
    /// The step the queue is held behind, until its reply comes back.
    awaiting: Option<Step>,
}

impl Playing {
    /// The meters the piece is writing, as `(track id, first bus, channels)` —
    /// what a mixer strip is drawn from.
    pub fn meters(&self) -> Vec<(u64, i32, usize)> {
        let Some(applier) = self.applier.as_ref() else {
            return Vec::new();
        };
        self.instance
            .meters()
            .into_iter()
            .filter_map(|(track, handle, channels)| {
                let (bus, _) = applier.bus(&handle)?;
                Some((track, bus, channels))
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
        self.applier.as_ref().map_or(0, Applier::node_count)
    }

    /// **The difference between the plan and what is made, queued as steps**,
    /// with every node created at the tail of `target`.
    fn reconcile(
        &mut self,
        plan: &Plan,
        gain: f32,
        target: i32,
        ids: &mut IdSpaces,
    ) -> Result<(), IdError> {
        let ops = self.instance.reconcile(plan, gain);
        // **No transport binding.** The op means *the engine owns the piece's
        // time*; this host also plays a take monitor, and there is one
        // transport per server, so the governed group is the one the host
        // made when its player attached and the piece's graph goes inside it.
        // What the engine freezes is a subtree, so the intent holds exactly.
        let applier = self.applier.get_or_insert_with(|| {
            Applier::new(Endpoint {
                target,
                bind_transport: false,
                ..Endpoint::default()
            })
        });
        let steps = applier.apply(ops, ids)?;
        self.queue.extend(steps);
        Ok(())
    }

    /// Everything freed, queued as steps. The piece itself is untouched: what
    /// an instance holds is nodes, and nodes are not the composition.
    fn teardown(&mut self, ids: &mut IdSpaces) -> Result<(), IdError> {
        let ops = self.instance.teardown();
        if let Some(mut applier) = self.applier.take() {
            let steps = applier.apply(ops, ids)?;
            self.queue.extend(steps);
        }
        Ok(())
    }

    /// **The messages that may go out now**: everything up to the next step
    /// that waits, which is sent (a sync) or noted (an await) and then holds
    /// the rest.
    fn ready(&mut self) -> Vec<OscMessage> {
        let mut out = Vec::new();
        while self.awaiting.is_none() {
            match self.queue.pop_front() {
                None => break,
                Some(Step::Send(message)) => out.push(message),
                Some(Step::Sync(id)) => {
                    out.push(OscMessage {
                        addr: "/server_sync".into(),
                        args: vec![OscType::Int(id)],
                    });
                    self.awaiting = Some(Step::Sync(id));
                }
                Some(wait @ Step::AwaitDone { .. }) => self.awaiting = Some(wait),
            }
        }
        out
    }

    /// **A reply, offered to the step the queue is held behind**: whether it
    /// was the one, which releases the rest.
    ///
    /// A `/fail` of the awaited command releases it too, said out loud: a
    /// queue held forever behind a refusal is a piece that never sounds again,
    /// and the refusal already says why.
    fn reply(&mut self, msg: &OscMessage) -> bool {
        let released = match (&self.awaiting, msg.addr.as_str()) {
            (Some(Step::Sync(id)), "/server_sync.reply") => {
                msg.args.first() == Some(&OscType::Int(*id))
            }
            (Some(Step::AwaitDone { command, index }), "/done") => {
                matches!(msg.args.first(), Some(OscType::String(c)) if c == command)
                    && index.is_none_or(|index| msg.args.get(1) == Some(&OscType::Int(index)))
            }
            (Some(Step::AwaitDone { command, .. }), "/fail") => {
                let refused = matches!(msg.args.first(), Some(OscType::String(c)) if c == command);
                if refused {
                    diag::warn!("the piece's {command} was refused: {:?}", msg.args);
                }
                refused
            }
            _ => false,
        };
        if released {
            self.awaiting = None;
        }
        released
    }
}

impl Host {
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
    /// The buffer number comes from the host's one buffer space, which is
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
            let bufnum = match self.ids.alloc(Space::Buffers, 1) {
                Ok(bufnum) => bufnum as i32,
                Err(e) => {
                    diag::warn!("source {} cannot be made: {e}", minted.id.0);
                    continue;
                }
            };
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

    /// **A reply the piece may be waiting on**, offered to its queue; what it
    /// releases goes out at once. Called by [`Host::on_server_reply`].
    pub(super) fn piece_reply(&mut self, msg: &OscMessage) {
        if self.instance.reply(msg) {
            self.send_piece();
        }
    }

    /// Sends every step of the piece that may go out now.
    fn send_piece(&mut self) {
        for message in self.instance.ready() {
            self.send_to_player(message);
        }
    }

    /// **Makes what sounds be what the piece says.**
    ///
    /// One call, whether it is the first time or after any edit: the plan is
    /// derived from the piece, the reconciler answers the difference, the
    /// applier turns it into steps, and this sends them. A node that did not
    /// change costs nothing, and an edit reaches a node that is already
    /// running — so a box moved while the piece plays is heard where it was
    /// dropped, with nothing that is sounding cut.
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
        // Inside the governed group when there is one, so the transport the
        // take monitor answers to is the piece's too; the root otherwise.
        let target = self.governed.unwrap_or(0);
        if let Err(e) = self.instance.reconcile(&plan, 1.0, target, &mut self.ids) {
            diag::warn!("the piece cannot be played: {e}");
        }
        self.send_piece();
        diag::debug!("sound_piece: {} node(s)", self.instance.nodes());
        self.instance.nodes()
    }

    /// Frees everything the piece made — what closing a window owes the server,
    /// and what a host that stops owning a piece owes it.
    pub fn hush_piece(&mut self) {
        if let Err(e) = self.instance.teardown(&mut self.ids) {
            diag::warn!("the piece cannot be freed: {e}");
        }
        self.send_piece();
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
    use clausters_core::ids::{IdShare, ServerShape};
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

    fn spaces() -> IdSpaces {
        IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE)
    }

    /// Everything the queue sends, answering every wait the way a server does.
    fn drain(playing: &mut Playing) -> Vec<OscMessage> {
        let mut sent = Vec::new();
        loop {
            sent.extend(playing.ready());
            let reply = match playing.awaiting.clone() {
                None => break,
                Some(Step::Sync(id)) => OscMessage {
                    addr: "/server_sync.reply".into(),
                    args: vec![OscType::Int(id)],
                },
                Some(Step::AwaitDone { command, index }) => OscMessage {
                    addr: "/done".into(),
                    args: std::iter::once(OscType::String(command))
                        .chain(index.map(OscType::Int))
                        .collect(),
                },
                Some(Step::Send(_)) => unreachable!("a send never waits"),
            };
            assert!(
                playing.reply(&reply),
                "the server's answer releases the wait"
            );
        }
        sent
    }

    fn addrs(messages: &[OscMessage]) -> Vec<&str> {
        messages.iter().map(|m| m.addr.as_str()).collect()
    }

    /// **A piece becomes the messages that play it**, in the crate's order: the
    /// defs it is made of, the barrier that closes them, the piece's own graph
    /// inside the governed group, and its slots.
    ///
    /// What this asserts is the *shape* and not the contents: which defs a piece
    /// needs and what a clip's ports are belong to the shared mixer and to the
    /// crate's reconciler, and both are tested there. What can only go wrong
    /// here is the carrying out.
    #[test]
    fn a_piece_becomes_the_messages_that_play_it() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        playing
            .reconcile(&plan_of(&piece()), 1.0, 1234, &mut ids)
            .unwrap();
        let first = playing.ready();
        assert_eq!(
            addrs(&first).last(),
            Some(&"/server_sync"),
            "the defs go out and the barrier holds the rest: {:?}",
            addrs(&first)
        );
        assert!(
            !addrs(&first).contains(&"/graph_new"),
            "nothing names a def before the barrier is answered"
        );
        let messages = drain(&mut playing);
        let addrs = addrs(&messages);
        let graph = messages
            .iter()
            .find(|m| m.addr == "/graph_new")
            .expect("the piece is a graph");
        assert_eq!(
            graph.args[3],
            OscType::Int(1234),
            "inside the governed group"
        );
        assert!(
            !addrs.contains(&"/transport_group"),
            "the host binds one governed group, when its player attaches: {addrs:?}"
        );
        assert!(
            addrs.contains(&"/graph_addSlot"),
            "the tracks and the boxes are slots of it: {addrs:?}"
        );
    }

    /// **The nodes come from the host's one node space**, so a voice, the
    /// monitor and the piece never name the same node.
    #[test]
    fn the_nodes_it_makes_are_the_host_s_spaces() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        playing
            .reconcile(&plan_of(&piece()), 1.0, 0, &mut ids)
            .unwrap();
        assert!(playing.nodes() > 0);
        assert_eq!(ids.in_use(Space::Nodes), playing.nodes());
    }

    /// **An edit reaches a node that is already running.** The second pass over
    /// an unchanged piece says nothing at all, and over a moved box it is a
    /// `/node_set` — never a free and a new node, which would cut what is
    /// sounding on every drag.
    #[test]
    fn an_edit_sets_a_live_node_and_an_unchanged_piece_says_nothing() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        let mut piece = piece();
        playing
            .reconcile(&plan_of(&piece), 1.0, 0, &mut ids)
            .unwrap();
        drain(&mut playing);
        let made = playing.nodes();

        playing
            .reconcile(&plan_of(&piece), 1.0, 0, &mut ids)
            .unwrap();
        assert!(
            drain(&mut playing).is_empty(),
            "a piece that did not move costs nothing"
        );

        piece.tracks[0].lanes[0].regions[0].position = Beat(6.0);
        playing
            .reconcile(&plan_of(&piece), 1.0, 0, &mut ids)
            .unwrap();
        let messages = drain(&mut playing);
        let addrs = addrs(&messages);
        assert!(!messages.is_empty(), "the box moved");
        assert!(
            addrs.iter().all(|a| *a == "/node_set"),
            "a move is a set on a live node: {addrs:?}"
        );
        assert_eq!(playing.nodes(), made, "and nothing was made or freed");
    }

    /// **A table is filled after its buffer exists, never before.**
    ///
    /// The defect this pins (found 2026-09-13, by ear): the host sent
    /// `/buffer_alloc` and `/buffer_setRange` back to back, the allocation is
    /// asynchronous, and the server refused every fill. A curve over an empty
    /// table drives its gain to zero, so the whole piece went silent. The fill
    /// waits for the `/done` of that very buffer.
    #[test]
    fn a_curve_table_waits_for_its_buffer() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        let mut applier = Applier::new(Endpoint::default());
        let steps = applier
            .apply(
                vec![clausters_editing::instance::Op::Buffer {
                    handle: "curve/1".into(),
                    samples: vec![0.5, 1.0],
                }],
                &mut ids,
            )
            .unwrap();
        let bufnum = applier.buffer("curve/1").unwrap();
        playing.queue.extend(steps);
        assert_eq!(
            addrs(&playing.ready()),
            ["/buffer_alloc"],
            "the alloc alone"
        );
        let done = |n: i32| OscMessage {
            addr: "/done".into(),
            args: vec![OscType::String("/buffer_alloc".into()), OscType::Int(n)],
        };
        assert!(!playing.reply(&done(bufnum + 1)), "another buffer's answer");
        assert!(playing.ready().is_empty(), "still held");
        assert!(playing.reply(&done(bufnum)));
        assert_eq!(
            addrs(&playing.ready()),
            ["/buffer_setRange", "/server_sync"]
        );
    }

    /// **Everything freed, and the tables with it.** What an instance holds is
    /// nodes; the piece is untouched, which is why a host can hush and sound
    /// again without the composition noticing.
    #[test]
    fn a_teardown_frees_what_it_made_and_forgets_it() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        playing
            .reconcile(&plan_of(&piece()), 1.0, 0, &mut ids)
            .unwrap();
        drain(&mut playing);
        assert!(playing.is_sounding());
        playing.teardown(&mut ids).unwrap();
        let messages = drain(&mut playing);
        assert!(
            addrs(&messages).contains(&"/node_free"),
            "the groups go: {:?}",
            addrs(&messages)
        );
        assert!(!playing.is_sounding());
        assert_eq!(playing.nodes(), 0);
    }
}
