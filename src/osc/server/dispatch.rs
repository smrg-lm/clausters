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
        // The one command that answers by ending the loop, so it cannot be a
        // table row like the others.
        if msg.addr == "/server_quit" {
            self.reply(from, "/done", vec![OscType::String("/server_quit".into())]);
            return Flow::Quit;
        }
        match COMMANDS.binary_search_by_key(&msg.addr.as_str(), |(addr, _)| addr) {
            Ok(i) => {
                let (addr, run) = COMMANDS[i];
                if let Err(why) = run(self, addr, &msg, from) {
                    self.fail(from, &msg.addr, why);
                }
            }
            Err(_) => self.fail(from, &msg.addr, "unknown command"),
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

/// What every command handler looks like from the dispatcher's side. The
/// handlers themselves keep the shape that suits them -- some read through
/// [`Args`], some need only the client, some hand the whole message to the
/// translator -- and the row adapts.
///
/// The `&'static str` is the row's own address. A handler that needs to name
/// its command (the async buffer jobs, which carry it into the `/done` they
/// reply much later) takes it from here rather than being told it a second
/// time by its caller: the table is where the name is written once.
type Command = fn(&mut OscServer, &'static str, &OscMessage, ClientId) -> Answer;

/// **The command set, as data.** Sorted by address, because the lookup is a
/// binary search and because a sorted list is one a human can scan.
///
/// A `match` over the same addresses would dispatch just as well. What a table
/// adds is that the set becomes *enumerable*: `tests/schema.rs` walks it and
/// fails when a command the server answers is missing from `docs/schemas.md`,
/// which is the drift nothing could catch before -- a command is easy to add
/// and easy to forget to document, and the two lived in different files with
/// no way to compare them.
///
/// `/server_quit` is deliberately not here; see [`OscServer::handle_message`].
pub(super) static COMMANDS: &[(&str, Command)] = &[
    ("/buffer_alloc", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_allocRead", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_allocReadChannel", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_close", |s, _, m, f| {
        s.handle_buffer_close(Args::new(m), f)
    }),
    ("/buffer_export", |s, _, m, f| {
        s.handle_buffer_export(Args::new(m), f)
    }),
    ("/buffer_fill", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_free", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_gain", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_gen", |s, _, m, f| {
        s.handle_buffer_gen(m, f);
        Ok(())
    }),
    ("/buffer_get", |s, _, m, f| {
        s.handle_buffer_get(Args::new(m), f)
    }),
    ("/buffer_getRange", |s, _, m, f| {
        s.handle_buffer_get_range(Args::new(m), f)
    }),
    ("/buffer_query", |s, _, m, f| {
        s.handle_buffer_query(Args::new(m), f)
    }),
    ("/buffer_read", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_readChannel", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_render", |s, _, m, f| {
        s.handle_buffer_render(Args::new(m), f)
    }),
    ("/buffer_reverse", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_set", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_setChannel", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_setRange", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_setRangeChannel", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_write", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/buffer_zero", |s, addr, m, f| {
        s.handle_buffer_cmd(addr, m, f);
        Ok(())
    }),
    ("/bus_fill", |s, _, m, _| s.handle_bus_fill(Args::new(m))),
    ("/bus_get", |s, _, m, f| s.handle_bus_get(Args::new(m), f)),
    ("/bus_getRange", |s, _, m, f| {
        s.handle_bus_get_range(Args::new(m), f)
    }),
    ("/bus_set", |s, _, m, _| s.handle_bus_set(Args::new(m))),
    ("/bus_setRange", |s, _, m, _| {
        s.handle_bus_set_range(Args::new(m))
    }),
    ("/bus_stream", |s, _, m, f| {
        s.handle_bus_stream(m, f);
        Ok(())
    }),
    ("/bus_tap", |s, _, m, f| {
        s.handle_bus_tap(m, f);
        Ok(())
    }),
    ("/bus_tapStream", |s, _, m, f| {
        s.handle_bus_tap_stream(m, f);
        Ok(())
    }),
    ("/clock_query", |s, _, _, f| {
        s.handle_clock_query(f);
        Ok(())
    }),
    ("/def_free", |s, _, m, f| {
        s.handle_def_free(m, f);
        Ok(())
    }),
    ("/def_load", |s, _, m, f| s.handle_def_load(Args::new(m), f)),
    ("/def_loadDir", |s, _, m, f| {
        s.handle_def_load_dir(Args::new(m), f)
    }),
    ("/def_query", |s, _, m, f| {
        s.handle_def_query(Args::new(m), f)
    }),
    ("/def_send", |s, _, m, f| s.handle_def_send(Args::new(m), f)),
    ("/graph_new", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/graph_newVoice", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/group_deepFree", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/group_dumpGraph", |s, _, m, f| {
        s.handle_group_dump_graph(Args::new(m), f)
    }),
    ("/group_freeAll", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/group_head", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/group_name", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/group_new", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/group_parallel", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/group_query", |s, _, m, f| {
        s.handle_group_query(Args::new(m), f)
    }),
    ("/group_queryTree", |s, _, m, f| {
        s.handle_group_query_tree(Args::new(m), f)
    }),
    ("/group_sortMode", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/group_tail", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/midi_bind", |s, _, m, f| {
        s.handle_via_translate(m, f);
        s.persist_bindings();
        Ok(())
    }),
    ("/midi_map", |s, _, m, f| {
        s.handle_via_translate(m, f);
        s.persist_bindings();
        Ok(())
    }),
    ("/midi_unbind", |s, _, m, f| {
        s.handle_via_translate(m, f);
        s.persist_bindings();
        Ok(())
    }),
    ("/node_after", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_before", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_fill", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_free", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_map", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_mapAudio", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_mapAudioRange", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_mapRange", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_order", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_query", |s, _, m, f| {
        s.handle_node_query(Args::new(m), f)
    }),
    ("/node_run", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_set", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_setRange", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/node_trace", |s, _, m, _| {
        s.handle_node_trace(Args::new(m))
    }),
    ("/node_ugenCmd", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/sched_at", |s, _, m, f| {
        s.handle_sched_at(m, f);
        Ok(())
    }),
    ("/sched_atTransport", |s, _, m, f| {
        s.handle_sched_at_transport(m, f);
        Ok(())
    }),
    ("/sched_clear", |s, _, _, f| {
        s.handle_sched_clear(f);
        Ok(())
    }),
    ("/server_cmd", |s, _, m, f| {
        s.handle_server_cmd(Args::new(m), f)
    }),
    ("/server_dumpOsc", |s, _, m, f| {
        s.handle_server_dump_osc(m, f);
        Ok(())
    }),
    ("/server_errorMode", |s, _, m, _| {
        s.handle_server_error_mode(Args::new(m))
    }),
    ("/server_notify", |s, _, m, f| {
        s.handle_server_notify(m, f);
        Ok(())
    }),
    ("/server_query", |s, _, _, f| {
        s.send_server_query(f);
        Ok(())
    }),
    ("/server_status", |s, _, _, f| {
        s.send_server_status(f);
        Ok(())
    }),
    ("/server_sync", |s, _, m, f| {
        s.handle_server_sync(m, f);
        Ok(())
    }),
    ("/server_verbosity", |s, _, m, f| {
        s.handle_server_verbosity(m, f);
        Ok(())
    }),
    ("/synth_forgetId", |s, _, m, f| {
        s.handle_synth_forget_id(Args::new(m), f)
    }),
    ("/synth_get", |s, addr, m, f| {
        s.handle_synth_get(Args::new(m), f, addr.ends_with("Range"))
    }),
    ("/synth_getRange", |s, addr, m, f| {
        s.handle_synth_get(Args::new(m), f, addr.ends_with("Range"))
    }),
    ("/synth_new", |s, _, m, f| {
        s.handle_via_translate(m, f);
        Ok(())
    }),
    ("/transport_group", |s, _, m, f| {
        s.handle_transport_group(Args::new(m), f)
    }),
    ("/transport_locate", |s, _, m, f| {
        s.handle_transport_locate(Args::new(m), f)
    }),
    ("/transport_locateSample", |s, _, m, f| {
        s.handle_transport_locate_sample(Args::new(m), f)
    }),
    ("/transport_loop", |s, _, m, f| {
        s.handle_transport_loop(Args::new(m), f)
    }),
    ("/transport_play", |s, _, m, f| {
        s.handle_transport_play(Args::new(m), f)
    }),
    ("/transport_query", |s, _, _, f| {
        s.handle_transport_query(f);
        Ok(())
    }),
    ("/transport_set", |s, _, m, f| {
        s.handle_transport(Args::new(m), f)
    }),
    ("/transport_stop", |s, _, _, f| {
        s.handle_transport_stop(f);
        Ok(())
    }),
    ("/ugen_query", |s, _, m, f| {
        s.handle_ugen_query(Args::new(m), f)
    }),
];

#[cfg(test)]
mod tests {
    use super::COMMANDS;

    /// The table is searched with `binary_search_by_key`, so an entry in the
    /// wrong place is not a style problem: the command becomes **unreachable**,
    /// answering `unknown command` while sitting right there in the list. A
    /// misfiled `/buffer_gain` is what this test was written for, and nothing
    /// else would have caught it — the row exists, the handler compiles, and
    /// only the lookup fails.
    #[test]
    fn the_command_table_is_sorted() {
        let addrs: Vec<&str> = COMMANDS.iter().map(|(addr, _)| *addr).collect();
        for pair in addrs.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{} must come before {} in the command table",
                pair[1],
                pair[0]
            );
        }
    }

    /// A duplicate would shadow silently: binary search finds one of the two
    /// and the other is dead code that still reads as wired up.
    #[test]
    fn no_command_is_listed_twice() {
        let mut addrs: Vec<&str> = COMMANDS.iter().map(|(addr, _)| *addr).collect();
        let before = addrs.len();
        addrs.sort_unstable();
        addrs.dedup();
        assert_eq!(before, addrs.len(), "a command is listed twice");
    }
}
