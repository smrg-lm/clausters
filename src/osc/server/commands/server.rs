//! `/server_*` and `/clock_query`: what the server says about itself.
//!
//! Status and capability reports, the logging controls, and the catalogue
//! queries a client uses to discover what this build can do (`/ugen_query`,
//! which is why a client never hardcodes the UGen set).

use super::super::*;

impl OscServer {
    /// `/server_dumpOsc flag`: toggles the OSC-traffic log overlay (the `clausters::osc`
    /// trace target). Unlike scsynth's console dump, this routes through the
    /// logging system the client also controls with `/server_verbosity`; output is on
    /// the server's stderr. Replies `/done`.
    pub(in crate::osc::server) fn handle_server_dump_osc(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let on = matches!(msg.args.first(), Some(OscType::Int(n)) if *n != 0);
        match crate::logging::set_osc_dump(on) {
            Ok(()) => self.reply(
                from,
                "/done",
                vec![OscType::String("/server_dumpOsc".into())],
            ),
            Err(e) => self.fail(from, "/server_dumpOsc", e),
        }
    }

    /// `/server_verbosity level`: the client retunes the server's log level live.
    /// `level` is an int (`-1` errors, `0` warn, `1` info, `2` debug, `3+`
    /// trace) or a string `EnvFilter` directive (e.g. `"clausters::osc=trace"`).
    /// Replies `/done`. (Uncommon, but it lets a client steer server logs
    /// without restarting; the initial level comes from `-v`/`RUST_LOG`.)
    pub(in crate::osc::server) fn handle_server_verbosity(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let result = match msg.args.first() {
            Some(OscType::Int(n)) => crate::logging::set_verbosity(*n as i8),
            Some(OscType::String(s)) => crate::logging::set_base(s),
            _ => Err("expected an int level or a string filter directive".to_string()),
        };
        match result {
            Ok(()) => self.reply(
                from,
                "/done",
                vec![OscType::String("/server_verbosity".into())],
            ),
            Err(e) => self.fail(from, "/server_verbosity", e),
        }
    }

    pub(in crate::osc::server) fn send_server_status(&mut self, to: ClientId) {
        let counters = self.handle.counters();
        let num_defs = self.translator.def_count();
        // avg/peak CPU are the engine's per-block load as a *percentage* of
        // the block budget (scsynth's convention). Peak is per poll window:
        // reading it resets it. The trailing int (late blocks since boot, our
        // engine-side xrun proxy) is appended after the scsynth-shaped fields,
        // so positional readers keep working.
        let args = vec![
            OscType::Int(1),
            OscType::Int(counters.ugens.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.synths.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.groups.load(Ordering::Relaxed) as i32),
            OscType::Int(num_defs as i32),
            OscType::Float(counters.avg_cpu() * 100.0),
            OscType::Float(counters.take_peak_cpu() * 100.0),
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
            OscType::Int(counters.late_blocks() as i32),
        ];
        self.reply(to, "/server_status.reply", args);
    }

    /// Reports the server's static configuration so a client can size its own
    /// bus/allocator state from the server instead of hardcoding it:
    /// `/server_query.reply [audio_buses, control_buses, output_channels,
    /// block_size, nominal_sr, actual_sr, input_channels, max_nodes,
    /// max_buffers, max_graph_children, max_ugen_inputs, taps, tap_frames,
    /// max_frame, max_stream_buses]`. The first six fields are stable; the
    /// boot-time capacities, the tap region shape, the stream-transport frame
    /// ceiling (what a client should size bulk requests like
    /// `/buffer_getRange` chunks from) and the `/bus_stream` bus ceiling **as
    /// it applies to the asking client's carrier** are appended so older
    /// clients that read only the six keep working.
    pub(in crate::osc::server) fn send_server_query(&mut self, to: ClientId) {
        let limits = self.handle.limits;
        let (taps, tap_frames) = self
            .handle
            .segment()
            .map_or((0, 0), |s| (s.taps(), s.tap_frames()));
        let args = vec![
            OscType::Int(self.handle.audio_buses as i32),
            OscType::Int(self.handle.control_buses().len() as i32),
            OscType::Int(self.handle.channels as i32),
            OscType::Int(crate::dsp::BLOCK_SIZE as i32),
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
            OscType::Int(self.handle.input_channels as i32),
            OscType::Int(limits.max_nodes as i32),
            OscType::Int(limits.max_buffers as i32),
            OscType::Int(limits.max_group_children as i32),
            OscType::Int(limits.max_ugen_inputs as i32),
            OscType::Int(taps as i32),
            OscType::Int(tap_frames as i32),
            OscType::Int(self.max_frame.min(i32::MAX as usize) as i32),
            // Per client, not per server: the same ceiling reaches a page over
            // the ring and a native client over TCP as two different numbers,
            // and the one a client can act on is its own.
            OscType::Int(self.stream_bus_cap(to).min(i32::MAX as usize) as i32),
        ];
        self.reply(to, "/server_query.reply", args);
    }

    /// the sample-clock query. Replies `/clock_query.reply` with the engine's
    /// sample counter (int64 `h`), the actual sample rate (double `d`) and the
    /// server's OSC/NTP time captured with the counter (timetag `t`). The
    /// `(osc_time, sample)` pair is the master-clock **anchor**: a client maps
    /// its logical OSC time `T` to this server's sample axis with
    /// `S0 + (T − T0)·rate` and schedules with `/sched_at` ([`Self::handle_sched_at`])
    /// directly in samples — see `docs/sample-clock.md`. Clients that only want
    /// the older two-field form ignore the trailing timetag. The counter counts
    /// *processed* samples: it runs a device buffer ahead of the speakers and
    /// pauses on xruns.
    pub(in crate::osc::server) fn handle_clock_query(&mut self, from: ClientId) {
        // Read the counter and the wall clock back-to-back so the published
        // anchor pairs the same instant (the sub-microsecond gap is negligible).
        let sample = self.handle.current_samples();
        let args = vec![
            OscType::Long(sample as i64),
            OscType::Double(self.info.actual_sample_rate),
            OscType::Time(self.now_ntp()),
        ];
        self.reply(from, "/clock_query.reply", args);
    }

    /// `/server_errorMode mode`: sets the error-posting mode. `1` posts command errors to
    /// the server console (the default), `0` silences them. The `/fail` OSC
    /// reply is always sent regardless — clients rely on it; only the
    /// server-side console logging is gated. scsynth's bundle-local `-1`/`-2`
    /// are not separately supported (deliberate deviation): the persistent
    /// `0`/`1` toggle is the model that fits our logging.
    pub(in crate::osc::server) fn handle_server_error_mode(&mut self, mut args: Args) -> Answer {
        self.post_errors = args.int()? != 0;
        Ok(())
    }

    /// `/server_cmd name args...`: a server-wide, typed command — the discoverable
    /// replacement for scsynth's untyped `/server_cmd`. `name` selects a handler from
    /// the built-in registry; unknown names `/fail` with the offending name.
    /// The mechanism exists for future server commands; the built-in `ping`
    /// (replies `/done /server_cmd ping`) proves the surface.
    pub(in crate::osc::server) fn handle_server_cmd(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        match args.str()? {
            "ping" => self.reply(
                from,
                "/done",
                vec![
                    OscType::String("/server_cmd".into()),
                    OscType::String("ping".into()),
                ],
            ),
            other => return Err(format!("unknown server command {other:?}")),
        }
        Ok(())
    }

    /// `/ugen_query [kind...]` → one `/ugen_query.reply` per UGen, then `/done "/ugen_query"`
    ///: the catalog straight from the `dsp::registry` descriptors, so a
    /// palette derives from the server's truth instead of a client-side copy.
    /// An unknown kind replies with an empty rate set and no inputs.
    ///
    /// Faust primitives are deliberately absent: that vocabulary is Faust's
    /// own and already lives in the client builders.
    ///
    /// Built without the `synth` feature there is no UGen catalog at all, and
    /// the honest reply is an **empty** listing rather than a `/fail` — the
    /// same way `/def_query` on such a build simply lists no synth defs.
    pub(in crate::osc::server) fn handle_ugen_query(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let mut names = Vec::with_capacity(args.len());
        while !args.is_empty() {
            names.push(args.str()?.to_string());
        }
        #[cfg(feature = "synth")]
        for info in ugen_infos(&names) {
            self.reply(from, "/ugen_query.reply", info);
        }
        self.reply(from, "/done", vec![OscType::String("/ugen_query".into())]);
        Ok(())
    }

    pub(in crate::osc::server) fn handle_server_notify(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        match msg.args.first() {
            Some(OscType::Int(1)) => {
                let id = match self.clients.iter().position(|c| *c == from) {
                    Some(i) => i + 1,
                    None => {
                        self.clients.push(from);
                        // The first subscriber shortens the loop's tick: a
                        // node event comes from the audio thread, which cannot
                        // wake it (`NOTIFY_INTERVAL`).
                        self.retune_timeout();
                        self.clients.len()
                    }
                };
                self.reply(
                    from,
                    "/done",
                    vec![
                        OscType::String("/server_notify".into()),
                        OscType::Int(id as i32),
                    ],
                );
            }
            Some(OscType::Int(0)) => {
                self.clients.retain(|c| *c != from);
                // And the last one hands the idle tick back.
                self.retune_timeout();
                self.reply(
                    from,
                    "/done",
                    vec![OscType::String("/server_notify".into())],
                );
            }
            _ => self.fail(from, "/server_notify", "expected int argument 0 or 1"),
        }
    }
}
