//! `/transport_*`: the shared beat grid and the rolling state.
//!
//! The server stores and broadcasts the transport; it never schedules audio
//! from it. With a group bound it also enforces it -- stop freezes that subtree
//! and the transport clock -- which is the one command family with two
//! intensities, and why `docs/decisions.md` has an entry on it.

use super::super::*;

/// What the two commands that speak **beats** refuse with when no grid has been
/// defined: `/transport_locate`, and `/transport_play` given a position.
///
/// Nothing else here needs one. Rolling, stopping, saying where the piece is
/// and looping a span of it are all in samples, and an audio editor has no
/// tempo to declare — asking it to invent one so that `/transport_play` will
/// answer is asking it to write down a number nobody reads.
const NO_GRID: &str = "no beat grid defined (/transport_set)";

impl OscServer {
    /// The `/transport_query.reply` payload: the grid plus the rolling state,
    /// `(origin_sample:int64, tempo:double, defined:int32, playing:int32,
    /// position:double)`. The first three fields are the original grid reply
    /// (older clients read just those); `playing`/`position` are appended.
    fn transport_reply_args(&self) -> Vec<OscType> {
        let t = self.transport;
        let (origin, tempo, defined, playing, position) = (
            t.origin_sample,
            t.tempo,
            t.defined as i32,
            t.playing as i32,
            t.position,
        );
        let group = t.group.unwrap_or(-1);
        let transport_sample = self.handle.current_transport_samples() as i64;
        // The position is read from the engine and the loop from here: the
        // first moves every block and only the audio thread knows it, the
        // second only changes when a client sets it.
        let position_sample = self.handle.current_transport_position() as i64;
        let (loop_start, loop_end) = t.loop_span.unwrap_or((0, 0));
        vec![
            OscType::Long(origin),
            OscType::Double(tempo),
            OscType::Int(defined),
            OscType::Int(playing),
            OscType::Double(position),
            OscType::Int(group),
            OscType::Long(transport_sample),
            OscType::Long(position_sample),
            OscType::Long(loop_start),
            OscType::Long(loop_end),
        ]
    }

    /// Beat `b` of the **piece** as a sample of the piece.
    ///
    /// Deliberately **not** through `origin_sample`: that origin anchors the
    /// beat grid on the *device* axis, which is what lets several clients
    /// phase-align on one running server. The piece's own axis starts at its
    /// own 0 by definition, so a song position in beats is just
    /// `b * rate / tempo`. Keeping the two apart is also what keeps the open
    /// T2 (whose subject is that origin) out of this conversion.
    fn beats_to_piece_samples(&self, beats: f64) -> u64 {
        let t = self.transport;
        if !t.defined || t.tempo <= 0.0 || !beats.is_finite() || beats <= 0.0 {
            return 0;
        }
        (beats * self.info.nominal_sample_rate / t.tempo).round() as u64
    }

    /// Sends the engine a locate, so the piece moves and not only the number
    /// this server broadcasts.
    fn locate_engine(&mut self, position: u64) {
        self.handle.send(Cmd::TransportLocate { position }).ok();
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
        // Setting the grid resets the rolling state: stopped, at position 0.
        self.transport = Transport {
            defined: true,
            origin_sample: origin,
            tempo,
            playing: false,
            position: 0.0,
            // The loop and the governed group survive: both are bindings to
            // samples and to the tree, not part of the grid, and dropping
            // either here would leave the engine holding something no client
            // could see.
            loop_span: self.transport.loop_span,
            group: self.transport.group,
        };
        // Redefining the grid puts the piece back at its start, which is what
        // "stopped at position 0" has always meant -- it just had nowhere to
        // say it before.
        self.locate_engine(0);
        // Redefining the grid stops the transport, so a bound group freezes.
        if self.transport.group.is_some() {
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
    /// grid).
    ///
    /// **Only the argument needs a grid.** `position` is a beat, so playing
    /// *from* one refuses until `/transport_set` has said what a beat is;
    /// playing from where the transport already stands needs no beats at all,
    /// which is how an audio editor drives it (with
    /// [`handle_transport_locate_sample`](Self::handle_transport_locate_sample)
    /// before it).
    pub(in crate::osc::server) fn handle_transport_play(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let located = args.opt_double()?;
        if located.is_some() && !self.transport.defined {
            return Err(NO_GRID.into());
        }
        if let Some(pos) = located {
            self.transport.position = pos;
        }
        self.transport.playing = true;
        // A play *from* a position is a locate and then a roll, in that order:
        // the engine must be standing at the right sample before time starts
        // moving, or the first block plays from wherever it was.
        if let Some(pos) = located {
            let sample = self.beats_to_piece_samples(pos);
            self.locate_engine(sample);
        }
        // With a group bound this is no longer an advisory: it thaws the
        // subtree and restarts the transport clock.
        if self.transport.group.is_some() {
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
        self.transport.playing = false;
        if self.transport.group.is_some() {
            self.handle.send(Cmd::TransportRun { rolling: false }).ok();
        }
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_stop".into())],
        );
        self.broadcast_transport();
    }

    /// `/transport_locate <position:double>` — set the song position **in
    /// beats** (where play starts or, while playing, seeks to). Every client's
    /// playhead locates to it; the `playing` flag is unchanged.
    ///
    /// This is the one locate that needs a grid, because a beat means nothing
    /// without one. The frame spelling is
    /// [`handle_transport_locate_sample`](Self::handle_transport_locate_sample)
    /// and it needs none.
    pub(in crate::osc::server) fn handle_transport_locate(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        if !self.transport.defined {
            return Err(NO_GRID.into());
        }
        let beats = args.double()?;
        self.transport.position = beats;
        let sample = self.beats_to_piece_samples(beats);
        self.locate_engine(sample);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_locate".into())],
        );
        self.broadcast_transport();
        Ok(())
    }

    /// `/transport_locateSample <sample:int64>` — locate on the piece's own
    /// **sample** axis, which is what an audio editor addresses.
    ///
    /// The sibling of [`Self::handle_transport_locate`] and not a replacement:
    /// a sequencer locates by beat and an editor by frame, and converting
    /// either into the other on the client is how a rounding error gets into
    /// a seek. A negative sample clamps to 0, the same floor the position
    /// itself has.
    ///
    /// **It needs no grid**: a frame is a frame. With one defined the beat
    /// reading follows, so the two spellings of the position never disagree;
    /// without one it stays 0, where the sample spelling is the whole truth.
    pub(in crate::osc::server) fn handle_transport_locate_sample(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let sample = args.long()?.max(0) as u64;
        let t = self.transport;
        // The beat-position field follows, so a client reading either one sees
        // the same place: they are two spellings of one position, and letting
        // them disagree is the two-owner problem in miniature.
        self.transport.position =
            match t.defined && t.tempo > 0.0 && self.info.nominal_sample_rate > 0.0 {
                true => sample as f64 * t.tempo / self.info.nominal_sample_rate,
                false => 0.0,
            };
        self.locate_engine(sample);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_locateSample".into())],
        );
        self.broadcast_transport();
        Ok(())
    }

    /// `/transport_loop [<start:int64> <end:int64>]` — the span of the piece
    /// the position wraps inside, in samples; **no arguments turns looping
    /// off**.
    ///
    /// Two forms rather than a third `enabled` argument: what a loop toggle
    /// needs to remember is the span it last used, and that is the client's to
    /// keep — the server holding a disabled span would be a second copy of a
    /// number the client already has, which is the one thing this protocol
    /// avoids everywhere else.
    ///
    /// The span is **half-open**: the end sample is the first one not played,
    /// so a loop of `0..n` over an `n`-sample take plays every frame exactly
    /// once and joins its own start with no repeat. An empty or inverted span
    /// fails rather than being silently ignored — it is always a mistake, and
    /// the engine's wrap would not terminate over one.
    ///
    /// Turning a loop on does **not** move the piece: it keeps playing from
    /// where it is and wraps when it first reaches the end.
    pub(in crate::osc::server) fn handle_transport_loop(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let span = match args.opt_long()? {
            None => None,
            Some(start) => {
                let end = args.long()?;
                if start < 0 || end <= start {
                    return Err("a loop needs 0 <= start < end".into());
                }
                Some((start, end))
            }
        };
        self.transport.loop_span = span;
        self.handle
            .send(Cmd::TransportLoop {
                span: span.map(|(s, e)| s as u64..e as u64),
            })
            .ok();
        self.reply(
            from,
            "/done",
            vec![OscType::String("/transport_loop".into())],
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
        let id = args.int()?;
        if id >= 0 && self.translator.mirror.children(id).is_none() {
            return Err(format!("unknown group {id}"));
        }
        self.transport.group = if id >= 0 { Some(id) } else { None };
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
