//! `/transport_*`: the shared beat grid and the rolling state.
//!
//! The server stores and broadcasts the transport; it never schedules audio
//! from it. With a group bound it also enforces it -- stop freezes that subtree
//! and the transport clock -- which is the one command family with two
//! intensities, and why `docs/decisions.md` has an entry on it.

use super::super::*;

/// Every `/transport_*` command but `/transport_set` needs a grid to act on,
/// and refuses the same way when there is none.
const NO_TRANSPORT: &str = "no transport defined";

impl OscServer {
    /// The `/transport_query.reply` payload: the grid plus the rolling state,
    /// `(origin_sample:int64, tempo:double, defined:int32, playing:int32,
    /// position:double)`. The first three fields are the original grid reply
    /// (older clients read just those); `playing`/`position` are appended.
    fn transport_reply_args(&self) -> Vec<OscType> {
        let (origin, tempo, defined, playing, position) = match self.transport {
            Some(t) => (t.origin_sample, t.tempo, 1, t.playing as i32, t.position),
            None => (0, 0.0, 0, 0, 0.0),
        };
        let group = self.transport.and_then(|t| t.group).unwrap_or(-1);
        let transport_sample = self.handle.current_transport_samples() as i64;
        vec![
            OscType::Long(origin),
            OscType::Double(tempo),
            OscType::Int(defined),
            OscType::Int(playing),
            OscType::Double(position),
            OscType::Int(group),
            OscType::Long(transport_sample),
        ]
    }

    /// Pushes the current transport state to every `/server_notify` client, so a
    /// responder on `/transport_query.reply` re-aligns or rolls its playhead live when
    /// the conductor changes the grid, plays, stops or locates — no polling.
    pub(in crate::osc::server) fn broadcast_transport(&self) {
        let push = self.transport_reply_args();
        for client in &self.clients {
            self.reply(*client, "/transport_query.reply", push.clone());
        }
    }

    /// `/transport_query` — reads the shared beat grid plus the rolling state.
    /// Replies `/transport_query.reply (origin_sample:int64, tempo:double,
    /// defined:int32, playing:int32, position:double)`, all zeros (and `defined`
    /// 0) when no grid is set.
    pub(in crate::osc::server) fn handle_transport_query(&mut self, from: ClientId) {
        let args = self.transport_reply_args();
        self.reply(from, "/transport_query.reply", args);
    }

    /// `/transport_set <origin_sample:int64> <tempo:double>` — sets the shared
    /// beat grid for phase-aligning several clients on the master sample clock
    /// (last writer wins), stopped at position 0, and replies `/done`. The grid
    /// is `beat b -> sample origin_sample + b·rate/tempo`; a client joins by
    /// reading it with [`Self::handle_transport_query`] and quantizing its start
    /// onto it. The server only stores/broadcasts it — in-memory (resets on
    /// restart), never scheduling audio from it.
    ///
    /// The rolling state (play/stop/locate) rides on top: see
    /// [`Self::handle_transport_play`]. Any change is **pushed** to every
    /// `/server_notify` client (the responder path).
    pub(in crate::osc::server) fn handle_transport(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let (origin, tempo) = (args.long()?, args.double()?);
        if origin < 0 || tempo.is_nan() || tempo <= 0.0 {
            return Err("originSample must be >= 0 and tempo > 0".into());
        }
        // Setting the grid (re)defines the transport: stopped, at position 0.
        // The governed group survives, because it is a binding to the tree, not
        // part of the grid -- and dropping it here would silently leave a frozen
        // subtree with nobody owning it.
        let group = self.transport.and_then(|t| t.group);
        self.transport = Some(Transport {
            origin_sample: origin,
            tempo,
            playing: false,
            position: 0.0,
            group,
        });
        // Redefining the grid stops the transport, so a bound group freezes.
        if group.is_some() {
            self.handle.send(Cmd::TransportRun { rolling: false }).ok();
        }
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_set".into())],
        );
        self.broadcast_transport();
        Ok(())
    }

    /// `/transport_play [position:double]` — start the transport rolling. With a
    /// `position` argument, playback starts from that song-position beat;
    /// without one, from where it last stopped/located. Every client's playhead
    /// obeys the broadcast (starting from `position`, quantized to the shared
    /// grid). Needs a grid defined first (`/transport_set`).
    pub(in crate::osc::server) fn handle_transport_play(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let Some(mut t) = self.transport else {
            return Err(NO_TRANSPORT.into());
        };
        if let Some(pos) = args.opt_double()? {
            t.position = pos;
        }
        t.playing = true;
        self.transport = Some(t);
        // With a group bound this is no longer an advisory: it thaws the
        // subtree and restarts the transport clock.
        if t.group.is_some() {
            self.handle.send(Cmd::TransportRun { rolling: true }).ok();
        }
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_play".into())],
        );
        self.broadcast_transport();
        Ok(())
    }

    /// `/transport_stop` — stop the transport. Every client's playhead halts at
    /// its current point; `position` holds for the next play.
    pub(in crate::osc::server) fn handle_transport_stop(&mut self, from: ClientId) {
        let Some(mut t) = self.transport else {
            return self.fail(from, "/transport_stop", NO_TRANSPORT);
        };
        t.playing = false;
        self.transport = Some(t);
        if t.group.is_some() {
            self.handle.send(Cmd::TransportRun { rolling: false }).ok();
        }
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_stop".into())],
        );
        self.broadcast_transport();
    }

    /// `/transport_locate <position:double>` — set the song position (where play
    /// starts or, while playing, seeks to). Every client's playhead locates to
    /// it; the `playing` flag is unchanged.
    pub(in crate::osc::server) fn handle_transport_locate(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let Some(mut t) = self.transport else {
            return Err(NO_TRANSPORT.into());
        };
        t.position = args.double()?;
        self.transport = Some(t);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_locate".into())],
        );
        self.broadcast_transport();
        Ok(())
    }

    /// `/transport_group <int32 group>` — binds the group the transport
    /// governs, or unbinds with a negative id.
    ///
    /// It is its own command rather than an argument of `/transport_set`
    /// because binding a group and defining the grid are independent decisions,
    /// and `/transport_set` redefines the whole rolling state.
    ///
    /// Unbinding **thaws** the group it governed: a frozen subtree with nobody
    /// left to resume it would be unreachable except by `/node_run`.
    pub(in crate::osc::server) fn handle_transport_group(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let Some(mut t) = self.transport else {
            return Err(NO_TRANSPORT.into());
        };
        let id = args.int()?;
        if id >= 0 && self.translator.mirror.children(id).is_none() {
            return Err(format!("unknown group {id}"));
        }
        t.group = if id >= 0 { Some(id) } else { None };
        self.transport = Some(t);
        if self.handle.send(Cmd::TransportGroup { id }).is_err() {
            return Err("command FIFO full".into());
        }
        // Binding while the transport is stopped freezes the group at once, and
        // the engine's own `TransportGroup` arm does that. Binding while it
        // rolls needs nothing further.
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_group".into())],
        );
        self.broadcast_transport();
        Ok(())
    }
}
