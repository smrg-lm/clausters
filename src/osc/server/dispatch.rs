//! Routing: packet to bundle to message to handler, and the two schedulers.
//!
//! [`OscServer::handle_message`] is the command table -- the one place that
//! maps an OSC address to the code that answers it. Around it sit the two ways
//! a message can arrive early: an NTP-timetagged bundle, which is converted to
//! samples here and queued on the engine, and the explicit `/sched_at` family,
//! which is already in samples on one of the two clock axes.

use super::*;

impl OscServer {
    pub(in crate::osc::server) fn handle_packet(
        &mut self,
        packet: OscPacket,
        from: ClientId,
    ) -> Flow {
        match packet {
            OscPacket::Message(msg) => self.handle_message(msg, from),
            OscPacket::Bundle(bundle) => self.handle_bundle(bundle, from),
        }
    }

    /// Bundles with the "immediately" timetag (or a past one — scsynth also
    /// runs late bundles right away) execute now; future timetags are
    /// converted to a sample target and shipped to the engine's scheduler,
    /// which fires them sample-accurately.
    fn handle_bundle(&mut self, bundle: OscBundle, from: ClientId) -> Flow {
        match self.timetag_delta_secs(bundle.timetag) {
            Some(delta) if delta > 0.0 => {
                self.schedule_bundle(bundle, delta, from);
                Flow::Continue
            }
            Some(delta) => {
                warn!("late bundle ({:.3}s): executing immediately", -delta);
                self.run_bundle_now(bundle, from)
            }
            None => self.run_bundle_now(bundle, from),
        }
    }

    fn run_bundle_now(&mut self, bundle: OscBundle, from: ClientId) -> Flow {
        for packet in bundle.content {
            if let Flow::Quit = self.handle_packet(packet, from) {
                return Flow::Quit;
            }
        }
        Flow::Continue
    }

    /// Builds every message of a timed bundle into engine commands (synths
    /// boxed, names resolved — all the allocating work happens now) and
    /// sends them as one atomic [`Cmd::Schedule`].
    fn schedule_bundle(&mut self, bundle: OscBundle, delta: f64, from: ClientId) {
        let time = self.handle.current_samples() + (delta * self.handle.sample_rate as f64) as u64;
        let mut cmds = Vec::new();
        for packet in bundle.content {
            match packet {
                OscPacket::Message(msg) => {
                    tracing::trace!(target: crate::logging::OSC_TARGET, "{} {:?} (in {delta:.3}s)", msg.addr, msg.args);
                    if let Err(e) = self.schedule_message(&msg, &mut cmds) {
                        self.fail(from, &msg.addr, e);
                    }
                }
                // A nested bundle carries its own timetag: scheduled (or
                // executed) independently.
                OscPacket::Bundle(inner) => {
                    self.handle_bundle(inner, from);
                }
            }
        }
        if !cmds.is_empty() && self.handle.send(Cmd::Schedule { time, cmds }).is_err() {
            self.fail(from, "#bundle", "command FIFO full");
        }
    }

    /// Translates one schedulable message into commands (shared with the NRT
    /// renderer). Nothing reaches the engine until the whole bundle is
    /// assembled.
    fn schedule_message(&mut self, msg: &OscMessage, cmds: &mut Vec<Cmd>) -> Result<(), String> {
        self.translator.translate(msg, cmds)
    }

    fn handle_message(&mut self, msg: OscMessage, from: ClientId) -> Flow {
        tracing::trace!(target: crate::logging::OSC_TARGET, "{} {:?}", msg.addr, msg.args);
        match msg.addr.as_str() {
            "/server_status" => self.send_server_status(from),
            "/server_query" => self.send_server_query(from),
            "/server_notify" => self.handle_server_notify(&msg, from),
            // The translator covers the whole schedulable subset (and keeps
            // the tree mirror in sync), so the immediate forms share one
            // path: translate, then ship every command.
            "/synth_new"
            | "/group_new"
            | "/group_freeAll"
            | "/group_deepFree"
            | "/node_free"
            | "/node_run"
            | "/node_set"
            | "/node_setRange"
            | "/node_fill"
            | "/node_map"
            | "/node_mapAudio"
            | "/node_mapRange"
            | "/node_mapAudioRange"
            | "/node_before"
            | "/node_after"
            | "/node_order"
            | "/group_head"
            | "/group_tail"
            | "/group_sortMode"
            | "/group_parallel"
            | "/group_name"
            | "/graph_new"
            | "/graph_newVoice" => self.handle_via_translate(&msg, from),
            "/node_trace" => self.handle_node_trace(&msg, from),
            // MIDI binding mutations also persist the binding set.
            "/midi_bind" | "/midi_unbind" | "/midi_map" => {
                self.handle_via_translate(&msg, from);
                self.persist_bindings();
            }
            "/group_queryTree" => self.handle_group_query_tree(&msg, from),
            "/group_query" => self.handle_group_query(&msg, from),
            "/node_query" => self.handle_node_query(&msg, from),
            "/group_dumpGraph" => self.handle_group_dump_graph(&msg, from),
            "/bus_set" => self.handle_bus_set(&msg, from),
            "/bus_get" => self.handle_bus_get(&msg, from),
            "/bus_setRange" => self.handle_bus_set_range(&msg, from),
            "/bus_getRange" => self.handle_bus_get_range(&msg, from),
            "/bus_fill" => self.handle_bus_fill(&msg, from),
            "/bus_stream" => self.handle_bus_stream(&msg, from),
            "/bus_tap" => self.handle_bus_tap(&msg, from),
            "/bus_tapStream" => self.handle_bus_tap_stream(&msg, from),
            "/synth_get" => self.handle_synth_get(&msg, from, false),
            "/synth_getRange" => self.handle_synth_get(&msg, from, true),
            "/synth_forgetId" => self.handle_synth_forget_id(&msg, from),
            "/buffer_close" => self.handle_buffer_close(&msg, from),
            "/def_load" => self.handle_def_load(&msg, from),
            "/def_loadDir" => self.handle_def_load_dir(&msg, from),
            "/sched_clear" => self.handle_sched_clear(from),
            "/server_errorMode" => self.handle_server_error_mode(&msg, from),
            "/server_cmd" => self.handle_server_cmd(&msg, from),
            "/node_ugenCmd" => self.handle_via_translate(&msg, from),
            "/clock_query" => self.handle_clock_query(from),
            "/sched_at" => self.handle_sched_at(&msg, from),
            "/buffer_alloc" => self.handle_buffer_cmd(&msg, from, "/buffer_alloc"),
            "/buffer_allocRead" => self.handle_buffer_cmd(&msg, from, "/buffer_allocRead"),
            "/buffer_read" => self.handle_buffer_cmd(&msg, from, "/buffer_read"),
            "/buffer_write" => self.handle_buffer_cmd(&msg, from, "/buffer_write"),
            "/buffer_zero" => self.handle_buffer_cmd(&msg, from, "/buffer_zero"),
            "/buffer_gen" => self.handle_buffer_gen(&msg, from),
            "/buffer_free" => self.handle_buffer_cmd(&msg, from, "/buffer_free"),
            "/buffer_query" => self.handle_buffer_query(&msg, from),
            "/def_query" => self.handle_def_query(&msg, from),
            "/ugen_query" => self.handle_ugen_query(&msg, from),
            "/buffer_get" => self.handle_buffer_get(&msg, from),
            "/buffer_getRange" => self.handle_buffer_get_range(&msg, from),
            "/buffer_export" => self.handle_buffer_export(&msg, from),
            "/server_sync" => self.handle_server_sync(&msg, from),
            "/def_send" => self.handle_def_send(&msg, from),
            "/def_free" => self.handle_def_free(&msg, from),
            "/server_dumpOsc" => self.handle_server_dump_osc(&msg, from),
            "/server_verbosity" => self.handle_server_verbosity(&msg, from),
            "/transport_query" => self.handle_transport_query(from),
            "/transport_set" => self.handle_transport(&msg, from),
            "/transport_play" => self.handle_transport_play(&msg, from),
            "/transport_stop" => self.handle_transport_stop(from),
            "/transport_locate" => self.handle_transport_locate(&msg, from),
            "/transport_group" => self.handle_transport_group(&msg, from),
            "/sched_atTransport" => self.handle_sched_at_transport(&msg, from),
            "/server_quit" => {
                self.reply(from, "/done", vec![OscType::String("/server_quit".into())]);
                return Flow::Quit;
            }
            other => self.fail(from, other, "unknown command"),
        }
        Flow::Continue
    }

    /// `/sched_at <int64 target> <blob packet>` — a timed bundle whose time
    /// is an absolute position on the **sample clock** instead of an NTP
    /// timetag (the OSC timetag format is NTP by spec, so sample targets get
    /// a container message rather than a reinterpreted tag; both front-ends
    /// feed the same engine queue and coexist freely). The blob is a complete
    /// OSC packet; all its leaf messages execute atomically at the target
    /// sample — nested bundle timetags inside the blob are **ignored**, one
    /// `/sched_at` is one instant. Past targets run at the start of the next
    /// block, like late NTP bundles.
    fn handle_sched_at(&mut self, msg: &OscMessage, from: ClientId) {
        let target = match msg.args.first() {
            Some(OscType::Long(t)) => *t,
            // Tolerated for hand-written clients; real targets outgrow i32
            // in under 13 hours at 48 kHz.
            Some(OscType::Int(t)) => *t as i64,
            _ => {
                return self.fail(
                    from,
                    "/sched_at",
                    "expected (int64 sampleTarget, blob packet)",
                );
            }
        };
        if target < 0 {
            return self.fail(from, "/sched_at", "sample target must be >= 0");
        }
        let Some(OscType::Blob(blob)) = msg.args.get(1) else {
            return self.fail(
                from,
                "/sched_at",
                "expected (int64 sampleTarget, blob packet)",
            );
        };
        let packet = match crate::osc::decode_packet(blob) {
            Ok(packet) => packet,
            Err(e) => return self.fail(from, "/sched_at", format!("bad packet blob: {e}")),
        };
        let mut cmds = Vec::new();
        self.sched_leaves(&packet, target, &mut cmds, from);
        if !cmds.is_empty()
            && self
                .handle
                .send(Cmd::Schedule {
                    time: target as u64,
                    cmds,
                })
                .is_err()
        {
            self.fail(from, "/sched_at", "command FIFO full");
        }
    }

    /// Translates every leaf message of a `/sched_at` blob, like
    /// [`Self::schedule_bundle`] does for NTP bundles: bad messages reply
    /// `/fail` individually, the rest still fire.
    fn sched_leaves(
        &mut self,
        packet: &OscPacket,
        target: i64,
        cmds: &mut Vec<Cmd>,
        from: ClientId,
    ) {
        match packet {
            OscPacket::Message(msg) => {
                tracing::trace!(target: crate::logging::OSC_TARGET, "{} {:?} (at sample {target})", msg.addr, msg.args);
                if let Err(e) = self.schedule_message(msg, cmds) {
                    self.fail(from, &msg.addr, e);
                }
            }
            OscPacket::Bundle(bundle) => {
                for inner in &bundle.content {
                    self.sched_leaves(inner, target, cmds, from);
                }
            }
        }
    }

    /// `/sched_clear`: flushes every pending timed bundle from the engine's
    /// schedule queue. The bundles' heap (boxed synths and the `Vec` shells)
    /// leaves through the garbage FIFO, so nothing is dropped on the audio
    /// thread. Replies `/done`.
    fn handle_sched_clear(&mut self, from: ClientId) {
        if self.handle.send(Cmd::ClearSched).is_err() {
            return self.fail(from, "/sched_clear", "command FIFO full");
        }
        self.reply(from, "/done", vec![OscType::String("/sched_clear".into())]);
    }

    /// `/sched_atTransport <int64 target> <blob packet>` — like
    /// [`Self::handle_sched_at`], but the target is a position on the
    /// **transport** clock rather than the device one.
    ///
    /// A client naming an absolute sample has to pick an axis before the server
    /// classifies the packet, and in the ordinary case it can: classification
    /// derives from the destination, which the client chose. So the value of
    /// declaring the axis is not disambiguation — it is **verification**. The
    /// server compares the declaration against its own classification and fails
    /// when they disagree, instead of playing the bundle in the wrong place,
    /// which is what a silently mismatched axis would do.
    fn handle_sched_at_transport(&mut self, msg: &OscMessage, from: ClientId) {
        const ADDR: &str = "/sched_atTransport";
        let Some(t) = self.transport else {
            return self.fail(from, ADDR, "no transport defined");
        };
        let Some(group) = t.group else {
            return self.fail(from, ADDR, "no group bound");
        };
        let target = match msg.args.first() {
            Some(OscType::Long(v)) => *v,
            Some(OscType::Int(v)) => *v as i64,
            _ => return self.fail(from, ADDR, "expected (int64 sampleTarget, blob packet)"),
        };
        if target < 0 {
            return self.fail(from, ADDR, "sample target must be >= 0");
        }
        let Some(OscType::Blob(blob)) = msg.args.get(1) else {
            return self.fail(from, ADDR, "expected (int64 sampleTarget, blob packet)");
        };
        let packet = match crate::osc::decode_packet(blob) {
            Ok(packet) => packet,
            Err(e) => return self.fail(from, ADDR, format!("bad packet blob: {e}")),
        };
        let mut cmds = Vec::new();
        self.sched_leaves(&packet, target, &mut cmds, from);
        if cmds.is_empty() {
            return;
        }
        if !self.packet_targets_group(&cmds, group) {
            return self.fail(from, ADDR, "packet is not governed by the transport");
        }
        // The engine's queue speaks the device axis and converts on arrival, so
        // hand it the device time this transport target corresponds to and let
        // its own conversion round-trip it back unchanged.
        let frozen = self.handle.current_frozen_total();
        let device = TransportSample::new(target as u64).to_device(frozen).get();
        if self
            .handle
            .send(Cmd::Schedule { time: device, cmds })
            .is_err()
        {
            self.fail(from, ADDR, "command FIFO full");
        } else {
            self.reply(from, "/done", vec![OscType::String(ADDR.into())]);
        }
    }

    /// The network-side twin of the engine's bundle classifier: whether any
    /// command in `cmds` targets a node at or under `group`, walked on the
    /// **mirror** rather than the engine's tree (the engine's is not reachable
    /// from here). Kept in step with `Engine::bundle_is_governed` by hand.
    fn packet_targets_group(&self, cmds: &[Cmd], group: i32) -> bool {
        cmds.iter().any(|cmd| {
            cmd_target_nodes(cmd)
                .iter()
                .flatten()
                .any(|id| self.mirror_is_descendant_of(*id, group))
        })
    }

    /// Walks the network-side mirror up from `id` to see whether `group` is on
    /// its parent chain. `id == group` counts, as it does engine-side.
    fn mirror_is_descendant_of(&self, id: i32, group: i32) -> bool {
        let mut current = id;
        // Bounded by the mirror's size: a parent chain cannot be longer, and an
        // unbounded walk here would hang the network thread on a corrupt tree.
        for _ in 0..=MAX_NODES {
            if current == group {
                return true;
            }
            match self.translator.mirror.parent(current) {
                Some(parent) => current = parent,
                None => return false,
            }
        }
        false
    }
}
