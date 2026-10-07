//! **What is sounding**: the multitrack's instance, and the steps that make it match.
//!
//! A standalone host plays a multitrack the way every other endpoint does -- through
//! [`clausters_document::multitrack::nodes::plan`], which says what a multitrack
//! *needs*, [`clausters_editing::instance::Instance`], which answers the
//! **difference** between that and what a server already holds, and
//! [`clausters_editing::apply::Applier`], which turns that difference into the
//! messages and the waits that carry it out. None of the three opens a socket,
//! and that is the seam: *"the client's half is exactly: a table, an
//! allocator, and a socket"*, and **a standalone host is a client in that
//! sentence**. The table is the applier's, the allocator is the host's
//! [`IdSpaces`](clausters_core::ids::IdSpaces) (`ids.rs`), and this module is
//! the socket.
//!
//! # Why the host does not carry out ops itself any more
//!
//! It did, twice over. `document::sound` was the host's own reconciler, and
//! when that went, the ops were turned into messages here while the Python
//! client turned them into messages in `playback.py` -- two translations of one
//! vocabulary, and the host's was the one that sent a buffer's fill before the
//! buffer existed and went silent. The applier is that translation, once, and
//! every endpoint binds it.
//!
//! # The waits
//!
//! Walked by the crate's [`Runner`]: a step that waits (`/done` of an
//! asynchronous command, a `/server_sync` barrier) holds everything after it
//! until the matching reply comes back through [`Host::on_server_reply`], which
//! both fronts call for every reply, saying which link it came in on. Nothing
//! blocks a frame: the steps queue, and the reply path drains them. The host
//! used to keep a queue of its own here; it had nowhere to put a step for the
//! *other* server, so a join's stitch in the session and its attach on the
//! player were ordered by a list of buffer numbers beside it.
//!
//! Nothing here decides *what* to play. A question about order, about which
//! node a port belongs to, or about what a curve's table holds is the crate's,
//! and if the answer looks wrong the fix goes there -- where both clients read
//! it too.

use std::collections::HashMap;

use clausters_core::ids::Space;
use clausters_core::osc::{OscMessage, OscType};
use clausters_document::SourceId;
use clausters_document::multitrack::nodes::SourceInfo;
use clausters_editing::apply::{Endpoint, Step};
use clausters_editing::notes_playback::NotesPlayback;
use clausters_editing::playback::MultitrackPlayback;
use clausters_editing::run::{Reply, Runner, Server};

use crate::host::diag;
use crate::host::{Host, HostEffect, document};

/// **Which link a server's reply came in on.**
///
/// A host has at most two: the **server** leg, which is the embedded server, an
/// external `--server`, or an editor's in-process session; and the **player**,
/// where one is attached apart from it. Which [`Server`] a leg is follows from
/// whether the other one exists ([`Host::server_of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// The server leg.
    Server,
    /// A player attached apart from the server.
    Player,
}

/// **What a host told itself and asked of the playback**, in the shapes the
/// recorded exchange names (`editor_exchange` in the web client's
/// `editing-vectors.json`): held only by tests, which compare it with what a
/// client's editor was told and asked for the same turns.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Exchange {
    /// Each answer, as `[kind, seq, version, reason, [[widget, props]]]`.
    pub(crate) told: Vec<serde_json::Value>,
    /// Each verb carried out on the playback, as `[verb, ...args]`.
    pub(crate) asked: Vec<serde_json::Value>,
}

/// **A multitrack, as it is playing**: the crate's playback, and the steps not yet
/// carried out.
#[derive(Debug, Default)]
pub struct Playing {
    /// The instance, the applier and the transport -- the crate's, as every
    /// endpoint holds it. Made on the first sync, and it makes the transport's
    /// group itself.
    multitrack: Option<MultitrackPlayback>,
    /// What plays each roll's window, by the source whose sequence it is: a
    /// playback on a transport of its own, made when its window opens -- as a
    /// client's notes editor has it -- and closed with it.
    notes: HashMap<SourceId, NotesPlayback>,
    /// Each roll's position cursor, as a beat of its sequence.
    notes_cursor: HashMap<SourceId, f64>,
    /// What plays each score editor's window, by its window: the score's
    /// render, and a playback of it on a transport of its own -- as a
    /// client's score editor has it.
    #[cfg(feature = "notation")]
    scores: HashMap<i32, (clausters_document::events::EventSequence, NotesPlayback)>,
    /// The steps not carried out yet, across both servers -- the crate's walk.
    run: Runner,
}

impl Playing {
    /// The meters the multitrack is writing, as `(track id, first bus, channels)` --
    /// what a mixer strip is drawn from.
    pub fn meters(&self) -> Vec<(u64, i32, usize)> {
        self.multitrack
            .as_ref()
            .map_or_else(Vec::new, MultitrackPlayback::meters)
    }

    /// Whether anything is playing at all.
    pub fn is_sounding(&self) -> bool {
        self.multitrack
            .as_ref()
            .is_some_and(MultitrackPlayback::is_sounding)
    }

    /// How many nodes the multitrack is holding.
    pub fn nodes(&self) -> usize {
        self.multitrack
            .as_ref()
            .map_or(0, MultitrackPlayback::node_count)
    }

    /// Whether the transport was last told to roll the multitrack.
    pub fn rolling(&self) -> bool {
        self.multitrack
            .as_ref()
            .is_some_and(MultitrackPlayback::rolling)
    }

    /// The playback, made the first time.
    ///
    /// **The transport is the crate's, as it is every endpoint's**: the
    /// playback makes its group at the top, binds it and makes the multitrack
    /// inside it. The take monitor plays on a transport of its own
    /// ([`crate::host::play`]), so playing a take never moves the multitrack.
    fn playback(&mut self) -> &mut MultitrackPlayback {
        self.multitrack
            .get_or_insert_with(|| MultitrackPlayback::new(Endpoint::default()))
    }
}

impl Host {
    /// **Makes the sources an edit minted**, so what names them can be drawn
    /// and heard.
    ///
    /// A join states *there is a source N made of these spans* and rides that
    /// statement on the intent, because a source table is the session's and a
    /// multitrack is not. Three things follow from it and this does all three: the
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
            // what this source is, and what the multitrack now names is this.
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
                    // A join is one buffer at one rate, and the projection
                    // just said which.
                    rate: Some(stitch.rate).filter(|r| *r > 0.0),
                },
            );
            messages.push((bufnum, document::sources::stitch_message(bufnum, &stitch)));
        }
        // **Where the samples are, and then where the multitrack sounds.** A host
        // with two servers makes the join in the session, which owns the takes
        // and is what the picture reads, and points the player at it once the
        // session says it is made. Sent to the player instead, the join sounded
        // and the picture read a buffer the session never had: an empty box.
        // With one server the two are the same place and nothing is attached.
        let split = self.player.is_some() && self.server.is_some();
        for (bufnum, message) in messages {
            if !split {
                self.instance.run.push(Server::Sound, [Step::Send(message)]);
                continue;
            }
            self.instance.run.push(
                Server::Samples,
                [
                    Step::Send(message),
                    Step::AwaitDone {
                        command: "/buffer_stitch".into(),
                        index: Some(bufnum),
                    },
                ],
            );
            self.instance.run.push(
                Server::Sound,
                [Step::Send(OscMessage {
                    addr: "/buffer_attach".into(),
                    args: vec![OscType::Int(bufnum)],
                })],
            );
        }
        self.send_multitrack();
    }

    /// **Frees the joins the editing context let go of**: no box reads one and
    /// no undo or redo can put one back. Its buffer is freed where
    /// `mint_sources` made it -- the player's attachment first when a
    /// player is apart from the session -- its number goes back to the host's
    /// buffer space, and it leaves the session's table, so a save no longer
    /// names it.
    pub(crate) fn free_joins(&mut self, sources: &[u64]) {
        if sources.is_empty() {
            return;
        }
        let Some(owner) = self.owner.as_mut() else {
            return;
        };
        let mut freed = Vec::new();
        for source in sources {
            let id = SourceId(*source);
            if let Some(session) = owner.session.as_mut() {
                session.sources.remove(&id);
            }
            if let Some(take) = owner.takes.remove(id) {
                freed.push(take.bufnum);
            }
        }
        let split = self.player.is_some() && self.server.is_some();
        for bufnum in freed {
            let free = OscMessage {
                addr: "/buffer_free".into(),
                args: vec![OscType::Int(bufnum)],
            };
            self.instance
                .run
                .push(Server::Sound, [Step::Send(free.clone())]);
            if split {
                self.instance.run.push(Server::Samples, [Step::Send(free)]);
            }
            if let Err(e) = self.ids.release(Space::Buffers, i64::from(bufnum), 1) {
                diag::warn!("buffer {bufnum} of a freed join: {e}");
            }
        }
        self.send_multitrack();
    }

    /// **Which server a leg is**: the player sounds; the server leg holds the
    /// samples when a player is attached apart from it, and is the one server
    /// otherwise.
    pub(super) fn server_of(&self, leg: Leg) -> Server {
        match leg {
            Leg::Player => Server::Sound,
            Leg::Server if self.player.is_some() => Server::Samples,
            Leg::Server => Server::Sound,
        }
    }

    /// **A reply the multitrack may be waiting on**, offered to the runner; what it
    /// releases goes out at once. Called by [`Host::on_server_reply`].
    pub(super) fn multitrack_reply(&mut self, from: Leg, msg: &OscMessage) {
        let server = self.server_of(from);
        match self.instance.run.reply(server, msg) {
            Reply::Unrelated => return,
            Reply::Refused(args) => {
                diag::warn!("a step the multitrack waited on was refused: {args:?}")
            }
            Reply::Released => {}
        }
        self.send_multitrack();
    }

    /// **Carries steps out on the server that sounds**, behind whatever the
    /// multitrack's steps are still waiting on: the take monitor's, which the
    /// same walk releases on the same replies.
    pub(crate) fn send_sound_steps(&mut self, steps: Vec<Step>) {
        self.instance.run.push(Server::Sound, steps);
        self.send_multitrack();
    }

    /// Sends every step that may go out now, each to its server.
    fn send_multitrack(&mut self) {
        for (to, message) in self.instance.run.ready() {
            match (to, self.server.as_ref()) {
                (Server::Samples, Some(session)) => {
                    if let Err(e) = session.send(message) {
                        diag::warn!("cannot send to the server that holds the samples: {e}");
                    }
                }
                _ => self.send_to_player(message),
            }
        }
    }

    /// **Makes what sounds be what the multitrack says.**
    ///
    /// One call, whether it is the first time or after any edit: the plan is
    /// derived from the multitrack, the reconciler answers the difference, the
    /// applier turns it into steps, and this sends them. A node that did not
    /// change costs nothing, and an edit reaches a node that is already
    /// running -- so a box moved while the multitrack plays is heard where it was
    /// dropped, with nothing that is sounding cut.
    ///
    /// It is a no-op for a host with no multitrack and for one with no server -- a
    /// session opens, edits, undoes and saves without either.
    pub fn sound_multitrack(&mut self) -> usize {
        #[cfg(test)]
        self.exchange.asked.push(serde_json::json!(["sync"]));
        let Some(owner) = self.owner.as_ref() else {
            return 0;
        };
        if !owner.draws_multitrack() || self.player().is_none() {
            return 0;
        }
        let look = owner.multitrack_look();
        let sources: HashMap<SourceId, SourceInfo> = owner
            .takes
            .iter()
            .map(|(id, take)| {
                (
                    *id,
                    SourceInfo {
                        buffer: take.bufnum,
                        channels: take.channels.unwrap_or(1).max(1) as usize,
                        // Frames at the take's own rate, or the session's
                        // when the table did not say.
                        duration: take
                            .frames
                            .map(|frames| frames as f64 / take.rate.unwrap_or(look.rate)),
                    },
                )
            })
            .collect();
        // **A playback made here is told where the session's passes end**,
        // once: from then on it holds the end, since it is the one that hears
        // the contents grow past a marker.
        if self.instance.multitrack.is_none() {
            let end = owner.end;
            self.instance.playback().set_end(end);
        }
        let synced = self.instance.playback().sync(
            &owner.multitrack,
            look.rate,
            &sources,
            1.0,
            &mut self.ids,
        );
        match synced {
            Ok(steps) => self.instance.run.push(Server::Sound, steps),
            Err(e) => diag::warn!("the multitrack cannot be played: {e}"),
        }
        // **The notes regions play from the transport's event lane**: what the
        // boxes over the session's sequences read, placed by the editor that
        // is bound to them.
        let placed = owner
            .editor()
            .map(clausters_apps::multitrack::editor::MultitrackEditor::placed_notes)
            .unwrap_or_default();
        match self.instance.playback().notes(&placed, &mut self.ids) {
            Ok(steps) if !steps.is_empty() => self.instance.run.push(Server::Sound, steps),
            Ok(_) => {}
            Err(e) => diag::warn!("the notes regions cannot be played: {e}"),
        }
        // A roll played on its own hears the edit too, from whichever window
        // made it: the multitrack's history is the roll's.
        self.update_notes();
        self.send_multitrack();
        self.tell_meters();
        // What a save writes: the end as the playback now has it, a marker the
        // contents grew past carried with them.
        let end = self.instance.playback().end();
        if let Some(owner) = self.owner.as_mut() {
            owner.end = end;
        }
        diag::debug!("sound_multitrack: {} node(s)", self.instance.nodes());
        self.instance.nodes()
    }

    /// **Tells the editor where the multitrack's meters are**, and the widget with
    /// it, when that changed -- what a client's editor is handed on every turn
    /// (`sync`'s `meters`). The window is composed before anything sounds, so
    /// without this it never learns a bus and every strip reads nothing.
    fn tell_meters(&mut self) {
        use clausters_apps::multitrack::Meter;

        let meters: Vec<Meter> = self
            .instance
            .meters()
            .into_iter()
            .map(|(track, bus, channels)| Meter {
                track,
                bus,
                channels,
            })
            .collect();
        let Some(owner) = self.owner.as_mut() else {
            return;
        };
        let Some(widget) = owner.multitrack_widget() else {
            return;
        };
        let multitrack = owner.multitrack.clone();
        let Some(editor) = owner.editor_mut() else {
            return;
        };
        if editor.meters() == meters.as_slice() {
            return;
        }
        editor.set_meters(meters);
        editor.set_multitrack(multitrack);
        let Some(value) = editor.props(widget).remove("meters") else {
            return;
        };
        let mut fx = Vec::new();
        self.set_props(widget, vec![("meters".into(), value)], &mut fx);
    }

    /// Frees everything the multitrack made -- what closing a window owes the server,
    /// and what a host that stops owning a multitrack owes it.
    pub fn hush_multitrack(&mut self) {
        let Some(multitrack) = self.instance.multitrack.as_mut() else {
            return;
        };
        match multitrack.close(&mut self.ids) {
            Ok(steps) => self.instance.run.push(Server::Sound, steps),
            Err(e) => diag::warn!("the multitrack cannot be freed: {e}"),
        }
        self.send_multitrack();
    }

    /// **The position cursor was placed at `secs`**: a stopped transport is
    /// cued there and a rolling one is left alone.
    pub fn cue_multitrack(&mut self, secs: f64) {
        #[cfg(test)]
        self.exchange.asked.push(serde_json::json!(["cue", secs]));
        let Some(multitrack) = self.instance.multitrack.as_mut() else {
            return;
        };
        let steps = multitrack.cue(secs.max(0.0));
        self.instance.run.push(Server::Sound, steps);
        self.send_multitrack();
    }

    /// **Halts the multitrack and puts it back at `mark`**, in seconds: stop goes back
    /// to the mark, which is what tells it from pause.
    pub fn stop_multitrack(&mut self, mark: f64) {
        #[cfg(test)]
        self.exchange.asked.push(serde_json::json!(["stop"]));
        let Some(multitrack) = self.instance.multitrack.as_mut() else {
            return;
        };
        let steps = multitrack.stop(mark.max(0.0));
        self.instance.run.push(Server::Sound, steps);
        self.send_multitrack();
    }

    /// **The clock of a multitrack this host edits alone**, read with the transport
    /// at `position` samples of the multitrack: the label the editor names, set when
    /// what the editor says it reads changed. Answers what the write asks of
    /// the window when it did: its live layers alone, for a label that draws
    /// its text there, or the picture.
    pub fn tick_multitrack_clock(&mut self, position: f64) -> Option<HostEffect> {
        let secs = self
            .instance
            .multitrack
            .as_ref()?
            .samples_to_secs(position.max(0.0).round() as i64);
        let owner = self.owner.as_mut()?;
        // The end the clock reads is the multitrack's, which an undo can move
        // without a turn of the editor's.
        let stale = owner.editor()?.multitrack().version != owner.multitrack.version;
        let multitrack = stale.then(|| owner.multitrack.clone());
        let editor = owner.editor_mut()?;
        let (clock, window) = (editor.controls()?.clock, editor.window_id()?);
        if let Some(multitrack) = multitrack {
            editor.set_multitrack(multitrack);
        }
        let text = editor.clock(secs);
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
        fx.into_iter()
            .rfind(|effect| matches!(effect, HostEffect::Redraw(_) | HostEffect::RedrawLive(_)))
            .or(Some(HostEffect::Redraw(window)))
    }

    /// How many nodes the multitrack is playing through, for a caller reporting what
    /// it built.
    pub fn sounding_count(&self) -> usize {
        self.instance.nodes()
    }

    /// **Rolls or freezes the multitrack**, answering which it did.
    ///
    /// One verb, because the transport has one: `stop` freezes the governed
    /// group with every node's state intact and `play` thaws it, so pressing
    /// twice *continues* rather than starting the multitrack over. There is no
    /// "load" step and nothing to re-cue -- the nodes are resident and the
    /// position is the engine's.
    ///
    /// `None` when there is no multitrack sounding, which is what tells the caller
    /// to fall through to whatever else the key meant.
    pub fn roll_multitrack(&mut self) -> Option<bool> {
        let multitrack = self.instance.multitrack.as_mut()?;
        if !multitrack.is_sounding() {
            return None;
        }
        let steps = if multitrack.rolling() {
            multitrack.pause()
        } else {
            multitrack.play()
        };
        let rolling = multitrack.rolling();
        self.instance.run.push(Server::Sound, steps);
        self.send_multitrack();
        diag::info!(
            "the multitrack is {}",
            if rolling { "rolling" } else { "frozen" }
        );
        Some(rolling)
    }

    /// **The space bar's play over the multitrack**: the time range a sweep
    /// left, from its start to its end, or the loop switch over it or the
    /// whole multitrack, or a plain play from the mark.
    pub fn play_multitrack(&mut self, range: Option<(f64, f64)>, looping: bool) {
        let Some(multitrack) = self.instance.multitrack.as_mut() else {
            return;
        };
        if !multitrack.is_sounding() {
            return;
        }
        let steps = multitrack.play_pass(range, looping);
        self.instance.run.push(Server::Sound, steps);
        self.send_multitrack();
    }

    /// **The end switch** (the multitrack window's `stop_at_end`): a pass that
    /// rolled on now stops where the contents end, and one that stopped --
    /// there or at a marker -- rolls on. Answers whether it now stops.
    ///
    /// The session's, so it is flipped with no server and nothing sounding as
    /// well: the owner keeps it for the save, and a playback is told now if
    /// there is one, or when it is made.
    pub fn flip_multitrack_end(&mut self) -> Option<bool> {
        use clausters_document::End;
        let owner = self.owner.as_mut()?;
        owner.end = match owner.end {
            End::Open => End::Contents,
            End::Contents | End::At(_) => End::Open,
        };
        let end = owner.end;
        if let Some(multitrack) = self.instance.multitrack.as_mut() {
            let steps = multitrack.set_end(end);
            self.instance.run.push(Server::Sound, steps);
            self.send_multitrack();
        }
        Some(end != End::Open)
    }

    /// **The loop switch changed while the multitrack plays** (`L`): the pass
    /// in progress loops over `range` or the whole multitrack, or goes on to
    /// its end, from where it stands.
    pub fn reloop_multitrack(&mut self, range: Option<(f64, f64)>, looping: bool) {
        let Some(multitrack) = self.instance.multitrack.as_mut() else {
            return;
        };
        let steps = multitrack.set_loop(range, looping);
        self.instance.run.push(Server::Sound, steps);
    }

    /// **The loop switch changed while the roll of `source` plays** (`L`): the
    /// pass in progress loops over `range` (beats) or every note, or goes on
    /// to its end, from where it stands.
    pub fn reloop_notes(&mut self, source: SourceId, range: Option<(f64, f64)>, looping: bool) {
        let Some(owner) = self.owner.as_ref() else {
            return;
        };
        let (Some(shared), Some(playback)) = (
            owner.sequences.get(&source).cloned(),
            self.instance.notes.get_mut(&source),
        ) else {
            return;
        };
        let rate = owner.multitrack_look().rate;
        let steps = playback.set_loop(
            &shared.lock().unwrap_or_else(|e| e.into_inner()),
            range,
            looping,
            rate,
        );
        self.instance.run.push(Server::Sound, steps);
    }

    /// **Transport `transport` rolls or not, as the engine said**: the
    /// multitrack's playback or the roll's takes it as the truth.
    pub(crate) fn transport_rolled(&mut self, transport: i32, rolling: bool) {
        if let Some(multitrack) = self.instance.multitrack.as_mut()
            && multitrack.transport() == transport
        {
            multitrack.set_rolling(rolling);
        }
        for notes in self.instance.notes.values_mut() {
            if notes.transport() == Some(transport) {
                notes.set_rolling(rolling);
            }
        }
        #[cfg(feature = "notation")]
        for (_, notes) in self.instance.scores.values_mut() {
            if notes.transport() == Some(transport) {
                notes.set_rolling(rolling);
            }
        }
    }

    /// **The transport the roll of `source` plays on**, taken the first time:
    /// what its window draws its play cursor from. `None`, said out loud, when
    /// the server has none left.
    pub(crate) fn notes_transport(&mut self, source: SourceId) -> Option<i32> {
        let playback = self.instance.notes.entry(source).or_default();
        match playback.alloc_transport(&mut self.ids) {
            Ok(transport) => Some(transport),
            Err(e) => {
                diag::warn!("the roll has no play cursor and cannot be played: {e}");
                None
            }
        }
    }

    /// **The roll of `source` closed**: what its playback made is freed and
    /// its transport given back.
    pub(crate) fn close_notes(&mut self, source: SourceId) {
        self.instance.notes_cursor.remove(&source);
        let Some(mut playback) = self.instance.notes.remove(&source) else {
            return;
        };
        match playback.close(&mut self.ids) {
            Ok(steps) => self.instance.run.push(Server::Sound, steps),
            Err(e) => diag::warn!("the roll's playback cannot be closed: {e}"),
        }
        self.send_multitrack();
    }

    /// Whether the multitrack is rolling.
    pub fn multitrack_rolling(&self) -> bool {
        self.instance.rolling()
    }

    /// **The space bar over the roll of `source`**: a sounding sequence stops
    /// and goes back to the roll's position cursor, a silent one plays from
    /// there -- what a client's notes editor does with the same key.
    pub fn roll_notes(&mut self, source: SourceId, range: Option<(f64, f64)>, looping: bool) {
        #[cfg(test)]
        self.exchange.asked.push(serde_json::json!(["notes play"]));
        let Some(owner) = self.owner.as_ref() else {
            return;
        };
        let Some(shared) = owner.sequences.get(&source).cloned() else {
            return;
        };
        if self.player().is_none() {
            return;
        }
        let rate = owner.multitrack_look().rate;
        let from = self
            .instance
            .notes_cursor
            .get(&source)
            .copied()
            .unwrap_or(0.0);
        let playback = self.instance.notes.entry(source).or_default();
        let sequence = shared.lock().unwrap_or_else(|e| e.into_inner());
        // Play/stop, as a client's notes editor does with the same key: a stop
        // goes back to the roll's position cursor.
        let steps = if playback.rolling() {
            Ok(playback.stop(&sequence, from, rate))
        } else {
            playback.play_pass(&sequence, from, range, looping, rate, &mut self.ids)
        };
        drop(sequence);
        match steps {
            Ok(steps) => self.instance.run.push(Server::Sound, steps),
            Err(e) => diag::warn!("the notes cannot be played: {e}"),
        }
        self.send_multitrack();
    }

    /// **The roll of `source` placed its position cursor at `beat`**: kept as
    /// where its next play starts, and a stopped transport is cued there.
    pub fn cue_notes(&mut self, source: SourceId, beat: f64) {
        #[cfg(test)]
        self.exchange
            .asked
            .push(serde_json::json!(["notes cue", beat]));
        self.instance.notes_cursor.insert(source, beat);
        let Some(owner) = self.owner.as_ref() else {
            return;
        };
        let (Some(shared), Some(playback)) = (
            owner.sequences.get(&source).cloned(),
            self.instance.notes.get_mut(&source),
        ) else {
            return;
        };
        let rate = owner.multitrack_look().rate;
        let steps = playback.cue(
            &shared.lock().unwrap_or_else(|e| e.into_inner()),
            beat,
            rate,
        );
        self.instance.run.push(Server::Sound, steps);
        self.send_multitrack();
    }

    /// **A sequence changed**: every roll's event lane takes its sequence
    /// again and the server plays it on from where it is. Nothing for a roll
    /// that has not played, which has no lane yet.
    pub fn update_notes(&mut self) {
        let Some(owner) = self.owner.as_ref() else {
            return;
        };
        let rate = owner.multitrack_look().rate;
        for (source, playback) in &mut self.instance.notes {
            let Some(shared) = owner.sequences.get(source) else {
                continue;
            };
            match playback.update(
                &shared.lock().unwrap_or_else(|e| e.into_inner()),
                rate,
                &mut self.ids,
            ) {
                Ok(steps) => self.instance.run.push(Server::Sound, steps),
                Err(e) => diag::warn!("the roll cannot be played: {e}"),
            }
        }
        self.send_multitrack();
    }
}

/// **A score editor's window, as it plays.** The score is rendered at its
/// own tempo and played as a roll's sequence is -- an event lane on a
/// transport of its own, through the player -- so what a client's score
/// editor does with a turn's pass, this does with the same one.
#[cfg(feature = "notation")]
impl Host {
    /// **The transport the score of window `def_id` plays on**, taken the
    /// first time: what its page draws its cursor from. `None` with no player
    /// to sound it, or no transport left.
    pub(crate) fn score_transport(&mut self, def_id: i32) -> Option<i32> {
        self.player()?;
        let (_, playback) = self.instance.scores.entry(def_id).or_default();
        match playback.alloc_transport(&mut self.ids) {
            Ok(transport) => Some(transport),
            Err(e) => {
                diag::warn!("the score has no play cursor and cannot be played: {e}");
                None
            }
        }
    }

    /// **A turn asked for a pass**, `{"from", "range", "looping"}`: a score
    /// that sounds stops and goes back to where the pass began, a silent one
    /// plays `sequence`, its render, from there.
    pub(crate) fn roll_score(
        &mut self,
        def_id: i32,
        sequence: clausters_document::events::EventSequence,
        pass: &serde_json::Value,
    ) {
        if self.player().is_none() {
            return diag::warn!("the score cannot be played: this host has no player");
        }
        let Some(rate) = self.owner.as_ref().map(|o| o.multitrack_look().rate) else {
            return;
        };
        let from = pass["from"].as_f64().unwrap_or(0.0);
        let range = span(&pass["range"]);
        let looping = pass["looping"].as_bool().unwrap_or(false);
        let (held, playback) = self.instance.scores.entry(def_id).or_default();
        *held = sequence;
        let steps = if playback.rolling() {
            Ok(playback.stop(held, from, rate))
        } else {
            playback.play_pass(held, from, range, looping, rate, &mut self.ids)
        };
        match steps {
            Ok(steps) => self.instance.run.push(Server::Sound, steps),
            Err(e) => diag::warn!("the score cannot be played: {e}"),
        }
        self.send_multitrack();
    }

    /// **The loop switch changed** while the score plays: the pass in
    /// progress follows it.
    pub(crate) fn reloop_score(&mut self, def_id: i32, pass: &serde_json::Value) {
        let Some(rate) = self.owner.as_ref().map(|o| o.multitrack_look().rate) else {
            return;
        };
        let Some((held, playback)) = self.instance.scores.get_mut(&def_id) else {
            return;
        };
        let looping = pass["looping"].as_bool().unwrap_or(false);
        let steps = playback.set_loop(held, span(&pass["range"]), looping, rate);
        self.instance.run.push(Server::Sound, steps);
        self.send_multitrack();
    }

    /// **The cursor was placed at `beat`** -- a rewind: a stopped transport
    /// is cued there.
    pub(crate) fn cue_score(&mut self, def_id: i32, beat: f64) {
        let Some(rate) = self.owner.as_ref().map(|o| o.multitrack_look().rate) else {
            return;
        };
        let Some((held, playback)) = self.instance.scores.get_mut(&def_id) else {
            return;
        };
        let steps = playback.cue(held, beat, rate);
        self.instance.run.push(Server::Sound, steps);
        self.send_multitrack();
    }

    /// **The score changed**: its lane takes the new render, and the server
    /// plays it on from where it is. Nothing before the first play.
    pub(crate) fn update_score(
        &mut self,
        def_id: i32,
        sequence: clausters_document::events::EventSequence,
    ) {
        let Some(rate) = self.owner.as_ref().map(|o| o.multitrack_look().rate) else {
            return;
        };
        let Some((held, playback)) = self.instance.scores.get_mut(&def_id) else {
            return;
        };
        *held = sequence;
        match playback.update(held, rate, &mut self.ids) {
            Ok(steps) => self.instance.run.push(Server::Sound, steps),
            Err(e) => diag::warn!("the score cannot be played: {e}"),
        }
        self.send_multitrack();
    }

    /// **The score's window closed**: what its playback made is freed and
    /// its transport given back.
    pub(crate) fn close_score_playback(&mut self, def_id: i32) {
        let Some((_, mut playback)) = self.instance.scores.remove(&def_id) else {
            return;
        };
        match playback.close(&mut self.ids) {
            Ok(steps) => self.instance.run.push(Server::Sound, steps),
            Err(e) => diag::warn!("the score's playback cannot be closed: {e}"),
        }
        self.send_multitrack();
    }
}

/// A pass's `range`, `[start, end]` in beats, or `None`.
#[cfg(feature = "notation")]
fn span(range: &serde_json::Value) -> Option<(f64, f64)> {
    Some((range.get(0)?.as_f64()?, range.get(1)?.as_f64()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clausters_core::ids::{IdShare, IdSpaces, ServerShape};
    use clausters_document::multitrack::{Content, Multitrack, Region, Track};
    use clausters_document::{
        Lifetime, NodeId, Opaque, Second, SegmentRef, SegmentSource, SourceRef,
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
                duration: None,
            },
        )]
        .into_iter()
        .collect()
    }

    /// Two tracks, a box on each, over the one take.
    fn multitrack() -> Multitrack {
        let mut first = Track::new(NodeId(10), NodeId(11));
        first.name = Some("one".into());
        first.take_lanes[0].regions = vec![Region::new(
            NodeId(12),
            Second(2.0),
            Second(4.0),
            window(1, 0.5),
        )];
        let mut second = Track::new(NodeId(20), NodeId(21));
        second.take_lanes[0].regions = vec![Region::new(
            NodeId(22),
            Second(0.0),
            Second(2.0),
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

    /// The multitrack synced into the runner.
    fn sync(playing: &mut Playing, multitrack: &Multitrack, ids: &mut IdSpaces) {
        let steps = playing
            .playback()
            .sync(multitrack, 48_000.0, &sources(), 1.0, ids)
            .unwrap();
        playing.run.push(Server::Sound, steps);
    }

    /// What may go out now, without the server each goes to.
    fn ready(playing: &mut Playing) -> Vec<OscMessage> {
        playing.run.ready().into_iter().map(|(_, m)| m).collect()
    }

    /// Everything the runner sends, answering every wait the way a server does.
    fn drain(playing: &mut Playing) -> Vec<OscMessage> {
        let mut sent = Vec::new();
        loop {
            sent.extend(ready(playing));
            let reply = match playing.run.awaiting() {
                None => break,
                Some((_, Step::Sync(id))) => OscMessage {
                    addr: "/server_sync.reply".into(),
                    args: vec![OscType::Int(*id)],
                },
                Some((_, Step::AwaitDone { command, index })) => OscMessage {
                    addr: "/done".into(),
                    args: std::iter::once(OscType::String(command.clone()))
                        .chain(index.map(OscType::Int))
                        .collect(),
                },
                Some((_, Step::Send(_))) => unreachable!("a send never waits"),
            };
            assert_eq!(
                playing.run.reply(Server::Sound, &reply),
                Reply::Released,
                "the server's answer releases the wait"
            );
        }
        sent
    }

    fn addrs(messages: &[OscMessage]) -> Vec<&str> {
        messages.iter().map(|m| m.addr.as_str()).collect()
    }

    /// **A multitrack becomes the messages that play it**, in the crate's order: the
    /// defs, the barrier that closes them, the transport's group made and
    /// bound, the multitrack's graph inside it, and its slots.
    #[test]
    fn a_multitrack_becomes_the_messages_that_play_it() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        sync(&mut playing, &multitrack(), &mut ids);
        let first = ready(&mut playing);
        assert_eq!(
            addrs(&first).last(),
            Some(&"/server_sync"),
            "the defs go out and the barrier holds the rest: {:?}",
            addrs(&first)
        );
        let messages = drain(&mut playing);
        let follows = messages
            .iter()
            .find(|m| m.addr == "/transport_follow")
            .expect("the transport's group follows it, as every endpoint's does");
        let graph = messages
            .iter()
            .find(|m| m.addr == "/graph_new")
            .expect("the multitrack is a graph");
        assert_eq!(
            graph.args[3], follows.args[1],
            "the multitrack inside the transport's group"
        );
        assert_eq!(
            playing
                .multitrack
                .as_ref()
                .and_then(MultitrackPlayback::group)
                .map(OscType::Int),
            Some(follows.args[1].clone())
        );
        // **What the transport governs is the tracks' group**, a slot of the
        // multitrack's graph: the master around it goes on running.
        let tracks = messages
            .iter()
            .find(|m| m.addr == "/graph_addSlot")
            .expect("the tracks' group is the first slot");
        assert_eq!(tracks.args[0], graph.args[1], "a slot of the multitrack");
        let bound = messages
            .iter()
            .find(|m| m.addr == "/transport_group")
            .expect("and it is governed");
        assert_eq!(bound.args[1], tracks.args[2]);
        assert!(addrs(&messages).contains(&"/graph_addSlot"));
    }

    /// **The nodes come from the host's one node space**, so a voice, the
    /// monitor and the multitrack never name the same node.
    #[test]
    fn the_nodes_it_makes_are_the_host_s_spaces() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        sync(&mut playing, &multitrack(), &mut ids);
        assert!(playing.nodes() > 0);
        assert_eq!(ids.in_use(Space::Nodes), playing.nodes());
    }

    /// **An edit reaches a node that is already running**: an unchanged multitrack
    /// says nothing, and a moved box is a `/node_set`.
    #[test]
    fn an_edit_sets_a_live_node_and_an_unchanged_multitrack_says_nothing() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        let mut multitrack = multitrack();
        sync(&mut playing, &multitrack, &mut ids);
        drain(&mut playing);
        let made = playing.nodes();
        sync(&mut playing, &multitrack, &mut ids);
        assert!(
            drain(&mut playing).is_empty(),
            "a multitrack that did not move costs nothing"
        );
        multitrack.tracks[0].take_lanes[0].regions[0].position = Second(6.0);
        sync(&mut playing, &multitrack, &mut ids);
        let messages = drain(&mut playing);
        assert!(!messages.is_empty(), "the box moved");
        assert!(addrs(&messages).iter().all(|a| *a == "/node_set"));
        assert_eq!(playing.nodes(), made, "and nothing was made or freed");
    }

    /// **The transport verbs wait for their answers**, so a play sent right
    /// after the defs does not reach the server before them.
    #[test]
    fn the_transport_waits_behind_the_multitrack() {
        let (mut playing, mut ids) = (Playing::default(), spaces());
        sync(&mut playing, &multitrack(), &mut ids);
        let steps = playing.playback().play();
        playing.run.push(Server::Sound, steps);
        let first = ready(&mut playing);
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
        playing.run.push(Server::Sound, steps);
        assert_eq!(
            addrs(&ready(&mut playing)),
            ["/buffer_alloc"],
            "the alloc alone"
        );
        let done = |n: i32| OscMessage {
            addr: "/done".into(),
            args: vec![OscType::String("/buffer_alloc".into()), OscType::Int(n)],
        };
        assert_eq!(
            playing.run.reply(Server::Sound, &done(bufnum + 1)),
            Reply::Unrelated,
            "another buffer's answer"
        );
        assert_eq!(
            playing.run.reply(Server::Sound, &done(bufnum)),
            Reply::Released
        );
        assert_eq!(
            addrs(&ready(&mut playing)),
            ["/buffer_setRange", "/server_sync"]
        );
    }

    /// **A join made in the session is attached on the player once the
    /// session has made it** -- and only the session's `/done` says so, which
    /// is what the leg a reply came in on is for.
    #[test]
    fn a_wait_on_the_samples_is_released_by_the_session_and_not_the_player() {
        let mut host = Host::new();
        let session = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let player = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        let link = |socket: &std::net::UdpSocket| {
            crate::host::ServerLink::Udp(
                crate::host::ServerLeg::connect(socket.local_addr().unwrap()).unwrap(),
            )
        };
        host.set_server_link(link(&session));
        host.set_player_link(link(&player));
        host.instance.run.push(
            Server::Samples,
            [Step::AwaitDone {
                command: "/buffer_stitch".into(),
                index: Some(5),
            }],
        );
        host.instance.run.push(
            Server::Sound,
            [Step::Send(OscMessage {
                addr: "/buffer_attach".into(),
                args: vec![OscType::Int(5)],
            })],
        );
        host.send_multitrack();
        let done = OscMessage {
            addr: "/done".into(),
            args: vec![OscType::String("/buffer_stitch".into()), OscType::Int(5)],
        };
        host.multitrack_reply(Leg::Player, &done);
        assert!(
            host.instance.run.awaiting().is_some(),
            "the player did not make the join"
        );
        host.multitrack_reply(Leg::Server, &done);
        assert!(
            host.instance.run.is_idle(),
            "the session's done released the attach"
        );
    }
}
