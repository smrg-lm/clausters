//! **What is sounding through a server**: the steps a host's own playbacks
//! ask for, and the walk that carries them out.
//!
//! The take monitor plays a take ([`crate::host::play`]), and answers *steps* --
//! the messages and the waits that carry an op out -- and opens no socket.
//! This module is the socket: the crate's [`Runner`] holds a step that waits
//! (`/done` of an asynchronous command, a `/server_sync` barrier) and
//! everything after it until the matching reply comes back through
//! [`Host::on_server_reply`], which both fronts call for every reply, saying
//! which link it came in on. Nothing blocks a frame: the steps queue, and the
//! reply path drains them.

use clausters_core::osc::OscMessage;
use clausters_editing::apply::Step;
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

/// **What a host plays through a server**: the steps not yet carried out.
#[derive(Debug, Default)]
pub struct Playing {
    /// The steps not carried out yet, across both servers -- the crate's walk.
    run: Runner,
}

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
                Some((
                    _,
                    Step::Render(_) | Step::Measure { .. } | Step::Move { .. } | Step::Store { .. }
                ))
            ) {
                return;
            }
            diag::warn!("a file operation was asked of a host without a file executor");
            self.instance
                .run
                .rendered(Some("this host renders and moves no file".into()));
        }
    }
}
