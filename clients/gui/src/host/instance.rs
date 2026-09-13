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

use clausters_core::ids::Space;
use clausters_core::osc::{OscMessage, OscType};
use clausters_document::SourceId;
use clausters_document::multitrack::nodes::SourceInfo;
use clausters_editing::apply::{Endpoint, Step};
use clausters_editing::playback::PiecePlayback;

use crate::host::diag;
use crate::host::{Host, document};

/// **A piece, as it is playing**: the crate's playback, and the steps not yet
/// sent.
#[derive(Debug, Default)]
pub struct Playing {
    /// The instance, the applier and the transport — the crate's, as every
    /// endpoint holds it. Made on the first sync: its target is the governed
    /// group, which is the host's once a player is attached.
    piece: Option<PiecePlayback>,
    /// Steps not sent yet, in order.
    queue: VecDeque<Step>,
    /// The step the queue is held behind, until its reply comes back.
    awaiting: Option<Step>,
}

impl Playing {
    /// The meters the piece is writing, as `(track id, first bus, channels)` —
    /// what a mixer strip is drawn from.
    pub fn meters(&self) -> Vec<(u64, i32, usize)> {
        self.piece
            .as_ref()
            .map_or_else(Vec::new, PiecePlayback::meters)
    }

    /// Whether anything is playing at all.
    pub fn is_sounding(&self) -> bool {
        self.piece.as_ref().is_some_and(PiecePlayback::is_sounding)
    }

    /// How many nodes the piece is holding.
    pub fn nodes(&self) -> usize {
        self.piece.as_ref().map_or(0, PiecePlayback::node_count)
    }

    /// Whether the transport was last told to roll the piece.
    pub fn rolling(&self) -> bool {
        self.piece.as_ref().is_some_and(PiecePlayback::rolling)
    }

    /// The playback, made at the tail of `target` the first time.
    ///
    /// **No transport binding.** This host also plays a take monitor, and
    /// there is one transport per server, so the governed group is the one the
    /// host made when its player attached and the piece's graph goes inside
    /// it. What the engine freezes is a subtree, so the intent holds exactly.
    fn playback(&mut self, target: i32) -> &mut PiecePlayback {
        self.piece.get_or_insert_with(|| {
            PiecePlayback::new(Endpoint {
                target,
                bind_transport: false,
                ..Endpoint::default()
            })
        })
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
        made: &[clausters_document::multitrack::edit::MintedSource],
    ) {
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
        // Inside the governed group when there is one, so the transport the
        // take monitor answers to is the piece's too; the root otherwise.
        let target = self.governed.unwrap_or(0);
        let synced = self.instance.playback(target).sync(
            &owner.piece,
            look.rate,
            &sources,
            1.0,
            &mut self.ids,
        );
        match synced {
            Ok(steps) => self.instance.queue.extend(steps),
            Err(e) => diag::warn!("the piece cannot be played: {e}"),
        }
        self.send_piece();
        diag::debug!("sound_piece: {} node(s)", self.instance.nodes());
        self.instance.nodes()
    }

    /// Frees everything the piece made — what closing a window owes the server,
    /// and what a host that stops owning a piece owes it.
    pub fn hush_piece(&mut self) {
        let Some(piece) = self.instance.piece.as_mut() else {
            return;
        };
        match piece.close(&mut self.ids) {
            Ok(steps) => self.instance.queue.extend(steps),
            Err(e) => diag::warn!("the piece cannot be freed: {e}"),
        }
        self.send_piece();
    }

    /// **The position cursor was placed at `beat`**: a stopped transport is
    /// cued there and a rolling one is left alone. The beat is the editor's,
    /// read off the axis through the piece's own tempo map.
    pub fn cue_piece(&mut self, beat: f64) {
        let Some(piece) = self.instance.piece.as_mut() else {
            return;
        };
        let steps = piece.cue(beat.max(0.0));
        self.instance.queue.extend(steps);
        self.send_piece();
    }

    /// **Halts the piece and puts it back at `mark`**, in beats: stop goes back
    /// to the mark, which is what tells it from pause.
    pub fn stop_piece(&mut self, mark: f64) {
        let Some(piece) = self.instance.piece.as_mut() else {
            return;
        };
        let steps = piece.stop(mark.max(0.0));
        self.instance.queue.extend(steps);
        self.send_piece();
    }

    /// **The clock of a piece this host edits alone**, read with the transport
    /// at `position` samples of the piece: the label the editor names, set when
    /// what the editor says it reads changed. Answers the window to repaint when
    /// it did.
    pub fn tick_piece_clock(&mut self, position: f64) -> Option<i32> {
        let beat = self
            .instance
            .piece
            .as_ref()?
            .samples_to_beats(position.max(0.0).round() as i64);
        let owner = self.owner.as_mut()?;
        let editor = owner.editor.as_mut()?;
        let (clock, window) = (editor.controls()?.clock, editor.window_id()?);
        // The end the clock reads is the piece's, which an undo can move
        // without a turn of the editor's.
        if editor.piece().version != owner.piece.version {
            editor.set_piece(owner.piece.clone());
        }
        let text = editor.clock(beat);
        if self.clock_shown.as_deref() == Some(text.as_str()) {
            return None;
        }
        let mut fx = Vec::new();
        self.set_props(
            clock,
            vec![("text".into(), serde_json::Value::from(text.as_str()))],
            &mut fx,
        );
        self.clock_shown = Some(text);
        Some(window)
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
        let piece = self.instance.piece.as_mut()?;
        if !piece.is_sounding() {
            return None;
        }
        let steps = if piece.rolling() {
            piece.pause()
        } else {
            piece.play()
        };
        let rolling = piece.rolling();
        self.instance.queue.extend(steps);
        self.send_piece();
        diag::info!(
            "the piece is {}",
            if rolling { "rolling" } else { "frozen" }
        );
        Some(rolling)
    }

    /// Whether the piece is rolling.
    pub fn piece_rolling(&self) -> bool {
        self.instance.rolling()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
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

    fn spaces() -> IdSpaces {
        IdSpaces::new(ServerShape::DEFAULT, IdShare::WHOLE)
    }

    /// The piece synced into the queue, made at the tail of `target`.
    fn sync(playing: &mut Playing, piece: &Multitrack, target: i32, ids: &mut IdSpaces) {
        let steps = playing
            .playback(target)
            .sync(piece, 48_000.0, &sources(), 1.0, ids)
            .unwrap();
        playing.queue.extend(steps);
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
    /// defs, the barrier that closes them, the piece's graph inside the
    /// governed group, and its slots.
    #[test]
    fn a_piece_becomes_the_messages_that_play_it() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        sync(&mut playing, &piece(), 1234, &mut ids);
        let first = playing.ready();
        assert_eq!(
            addrs(&first).last(),
            Some(&"/server_sync"),
            "the defs go out and the barrier holds the rest: {:?}",
            addrs(&first)
        );
        let messages = drain(&mut playing);
        let graph = messages
            .iter()
            .find(|m| m.addr == "/graph_new")
            .expect("the piece is a graph");
        assert_eq!(
            graph.args[3],
            OscType::Int(1234),
            "inside the governed group"
        );
        assert!(!addrs(&messages).contains(&"/transport_group"));
        assert!(addrs(&messages).contains(&"/graph_addSlot"));
    }

    /// **The nodes come from the host's one node space**, so a voice, the
    /// monitor and the piece never name the same node.
    #[test]
    fn the_nodes_it_makes_are_the_host_s_spaces() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        sync(&mut playing, &piece(), 0, &mut ids);
        assert!(playing.nodes() > 0);
        assert_eq!(ids.in_use(Space::Nodes), playing.nodes());
    }

    /// **An edit reaches a node that is already running**: an unchanged piece
    /// says nothing, and a moved box is a `/node_set`.
    #[test]
    fn an_edit_sets_a_live_node_and_an_unchanged_piece_says_nothing() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        let mut piece = piece();
        sync(&mut playing, &piece, 0, &mut ids);
        drain(&mut playing);
        let made = playing.nodes();
        sync(&mut playing, &piece, 0, &mut ids);
        assert!(
            drain(&mut playing).is_empty(),
            "a piece that did not move costs nothing"
        );
        piece.tracks[0].lanes[0].regions[0].position = Beat(6.0);
        sync(&mut playing, &piece, 0, &mut ids);
        let messages = drain(&mut playing);
        assert!(!messages.is_empty(), "the box moved");
        assert!(addrs(&messages).iter().all(|a| *a == "/node_set"));
        assert_eq!(playing.nodes(), made, "and nothing was made or freed");
    }

    /// **The transport verbs wait for their answers**, so a play sent right
    /// after the defs does not reach the server before them.
    #[test]
    fn the_transport_waits_behind_the_piece() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        sync(&mut playing, &piece(), 0, &mut ids);
        let steps = playing.playback(0).play();
        playing.queue.extend(steps);
        let first = playing.ready();
        assert!(
            !addrs(&first).contains(&"/transport_play"),
            "held behind the barrier"
        );
        let rest = drain(&mut playing);
        assert_eq!(addrs(&rest).last(), Some(&"/transport_play"));
        assert!(playing.rolling());
    }

    /// **A table is filled after its buffer exists, never before** (found
    /// 2026-09-13, by ear): the fill waits for the `/done` of that very buffer.
    #[test]
    fn a_curve_table_waits_for_its_buffer() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        let mut applier = clausters_editing::apply::Applier::new(Endpoint::default());
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
        assert!(playing.reply(&done(bufnum)));
        assert_eq!(
            addrs(&playing.ready()),
            ["/buffer_setRange", "/server_sync"]
        );
    }
}
