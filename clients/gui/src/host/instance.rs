//! **What is sounding through a server**: the steps a host's own playbacks
//! ask for, and the walk that carries them out.
//!
//! A score editor this host owns plays its render on a transport of its own
//! ([`clausters_editing::notes_playback::NotesPlayback`]), and the take
//! monitor plays a take ([`crate::host::play`]); both answer *steps* -- the
//! messages and the waits that carry an op out -- and neither opens a socket.
//! This module is the socket: the crate's [`Runner`] holds a step that waits
//! (`/done` of an asynchronous command, a `/server_sync` barrier) and
//! everything after it until the matching reply comes back through
//! [`Host::on_server_reply`], which both fronts call for every reply, saying
//! which link it came in on. Nothing blocks a frame: the steps queue, and the
//! reply path drains them.

#[cfg(feature = "notation")]
use std::collections::HashMap;

use clausters_core::osc::OscMessage;
use clausters_editing::apply::Step;
#[cfg(feature = "notation")]
use clausters_editing::notes_playback::NotesPlayback;
use clausters_editing::run::{Reply, Runner, Server};

use crate::host::Host;
use crate::host::diag;

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

/// **What a host plays through a server**: the score editors' playbacks,
/// and the steps not yet carried out.
#[derive(Debug, Default)]
pub struct Playing {
    /// What plays each score editor's window, by its window: the score's
    /// render, and a playback of it on a transport of its own -- as a
    /// client's score editor has it.
    #[cfg(feature = "notation")]
    scores: HashMap<i32, (clausters_document::events::EventSequence, NotesPlayback)>,
    /// The steps not carried out yet, across both servers -- the crate's walk.
    run: Runner,
}

/// The rate a score is rendered at, in samples per second: the in-process
/// server's (`attach_server`, in the binary).
#[cfg(feature = "notation")]
const RATE: f64 = 48_000.0;

impl Host {
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

    /// **A reply a step may be waiting on**, offered to the runner; what it
    /// releases goes out at once. Called by [`Host::on_server_reply`].
    pub(super) fn steps_reply(&mut self, from: Leg, msg: &OscMessage) {
        let server = self.server_of(from);
        match self.instance.run.reply(server, msg) {
            Reply::Unrelated => return,
            Reply::Refused(args) => diag::warn!("a step waited on was refused: {args:?}"),
            Reply::Released => {}
        }
        self.send_steps();
    }

    /// **Carries steps out on the server that sounds**, behind whatever the
    /// steps before them are still waiting on.
    pub(crate) fn send_sound_steps(&mut self, steps: Vec<Step>) {
        self.instance.run.push(Server::Sound, steps);
        self.send_steps();
    }

    /// Sends every step that may go out now, each to its server.
    ///
    /// **This host renders nothing offline**: an offline render a step asks
    /// for (`Step::Render`) is refused, which releases what was held behind
    /// it, rather than holding the queue for ever.
    fn send_steps(&mut self) {
        loop {
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
            if !matches!(
                self.instance.run.awaiting(),
                Some((_, Step::Render(_) | Step::Move { .. }))
            ) {
                return;
            }
            diag::warn!("an offline render or a file move was asked of a host that does neither");
            self.instance
                .run
                .rendered(Some("this host renders and moves no file".into()));
        }
    }

    /// **Transport `transport` rolls or not, as the engine said**: a score's
    /// playback takes it as the truth.
    pub(crate) fn transport_rolled(&mut self, transport: i32, rolling: bool) {
        #[cfg(feature = "notation")]
        for (_, notes) in self.instance.scores.values_mut() {
            if notes.transport() == Some(transport) {
                notes.set_rolling(rolling);
            }
        }
        #[cfg(not(feature = "notation"))]
        let _ = (transport, rolling);
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
        let rate = RATE;
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
        self.send_steps();
    }

    /// **The loop switch changed** while the score plays: the pass in
    /// progress follows it.
    pub(crate) fn reloop_score(&mut self, def_id: i32, pass: &serde_json::Value) {
        let rate = RATE;
        let Some((held, playback)) = self.instance.scores.get_mut(&def_id) else {
            return;
        };
        let looping = pass["looping"].as_bool().unwrap_or(false);
        let steps = playback.set_loop(held, span(&pass["range"]), looping, rate);
        self.instance.run.push(Server::Sound, steps);
        self.send_steps();
    }

    /// **The cursor was placed at `beat`** -- a rewind: a stopped transport
    /// is cued there.
    pub(crate) fn cue_score(&mut self, def_id: i32, beat: f64) {
        let rate = RATE;
        let Some((held, playback)) = self.instance.scores.get_mut(&def_id) else {
            return;
        };
        let steps = playback.cue(held, beat, rate);
        self.instance.run.push(Server::Sound, steps);
        self.send_steps();
    }

    /// **The score changed**: its lane takes the new render, and the server
    /// plays it on from where it is. Nothing before the first play.
    pub(crate) fn update_score(
        &mut self,
        def_id: i32,
        sequence: clausters_document::events::EventSequence,
    ) {
        let rate = RATE;
        let Some((held, playback)) = self.instance.scores.get_mut(&def_id) else {
            return;
        };
        *held = sequence;
        match playback.update(held, rate, &mut self.ids) {
            Ok(steps) => self.instance.run.push(Server::Sound, steps),
            Err(e) => diag::warn!("the score cannot be played: {e}"),
        }
        self.send_steps();
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
        self.send_steps();
    }
}

/// A pass's `range`, `[start, end]` in beats, or `None`.
#[cfg(feature = "notation")]
fn span(range: &serde_json::Value) -> Option<(f64, f64)> {
    Some((range.get(0)?.as_f64()?, range.get(1)?.as_f64()?))
}
