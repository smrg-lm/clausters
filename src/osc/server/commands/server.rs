//! `/server_*` and `/clock_query`: what the server says about itself.
//!
//! Status and capability reports, the logging controls, and the catalogue
//! queries a client uses to discover what this build can do (`/ugen_query`,
//! which is why a client never hardcodes the UGen set).

use super::super::*;
use crate::server::engine::Counters;
use crate::server::meters::Meters;

/// The three figures of `/server_status.reply` that come from timing a block
/// -- average and peak CPU as percentages of the block budget, and the late
/// blocks since boot -- or three nils for a server that cannot time one.
/// Reading the peak resets its window either way.
fn cpu_args(counters: &Counters, timed: bool) -> [OscType; 3] {
    let peak = counters.take_peak_cpu();
    if !timed {
        return [OscType::Nil, OscType::Nil, OscType::Nil];
    }
    [
        OscType::Float(counters.avg_cpu() * 100.0),
        OscType::Float(peak * 100.0),
        OscType::Int(counters.late_blocks() as i32),
    ]
}

/// The arguments of `/server_load.reply`: `uptime, n, n x (role, index, busy,
/// calls)`, the seconds nil for a table that is not timed.
fn load_args(meters: &Meters) -> Vec<OscType> {
    let seconds = |value: f64| {
        if meters.timed() {
            OscType::Double(value)
        } else {
            OscType::Nil
        }
    };
    let mut args = vec![seconds(meters.uptime())];
    let report = meters.report();
    args.push(OscType::Int(report.len() as i32));
    for load in report {
        args.push(OscType::String(load.role.as_str().into()));
        args.push(OscType::Int(load.index as i32));
        args.push(seconds(load.busy));
        args.push(OscType::Long(load.calls as i64));
    }
    args
}

impl OscServer {
    /// `/server_dumpOsc flag`: toggles the OSC-traffic log overlay (the `clausters::osc`
    /// trace target). Unlike scsynth's console dump, this routes through the
    /// logging system the client also controls with `/server_verbosity`; output is on
    /// the server's stderr. Replies `/done`.
    pub(in crate::osc::server) fn handle_server_dump_osc(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let on = args.opt_int()?.unwrap_or(0) != 0;
        crate::logging::set_osc_dump(on)?;
        self.done(from, "/server_dumpOsc");
        Ok(())
    }

    /// `/server_verbosity level`: the client retunes the server's log level live.
    /// `level` is an int (`-1` errors, `0` warn, `1` info, `2` debug, `3+`
    /// trace) or a string `EnvFilter` directive (e.g. `"clausters::osc=trace"`).
    /// Replies `/done`. (Uncommon, but it lets a client steer server logs
    /// without restarting; the initial level comes from `-v`/`RUST_LOG`.)
    pub(in crate::osc::server) fn handle_server_verbosity(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        match args.one()? {
            OscType::Int(n) => crate::logging::set_verbosity(*n as i8)?,
            OscType::String(s) => crate::logging::set_base(s)?,
            _ => return Err("expected an int level or a string filter directive".into()),
        }
        self.done(from, "/server_verbosity");
        Ok(())
    }

    pub(in crate::osc::server) fn send_server_status(&mut self, to: ClientId) {
        let counters = self.handle.counters();
        let num_defs = self.translator.def_count();
        // avg/peak CPU are the engine's per-block load as a *percentage* of
        // the block budget (scsynth's convention). Peak is per poll window:
        // reading it resets it. The trailing int is the late blocks since boot,
        // our engine-side xrun proxy.
        //
        // **No leading pad.** scsynth's reply opens with an unused `1` and this
        // one used to copy it, which left the counters one index off every
        // other reply in this protocol -- `/server_query.reply` and the node
        // queries all start at their first real field. The clients read this
        // into a `ServerStatus` now, so the padding named nothing and only cost
        // a reader the off-by-one.
        //
        // **A server with no clock says nil, not zero.** The three figures
        // that come from timing a block are measurements; where nothing can
        // take them, a zero would read as an idle engine.
        let [avg, peak, late] = cpu_args(counters, self.handle.meters().timed());
        let args = vec![
            OscType::Int(counters.ugens.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.synths.load(Ordering::Relaxed) as i32),
            OscType::Int(counters.groups.load(Ordering::Relaxed) as i32),
            OscType::Int(num_defs as i32),
            avg,
            peak,
            OscType::Double(self.info.nominal_sample_rate),
            OscType::Double(self.info.actual_sample_rate),
            late,
        ];
        self.reply(to, "/server_status.reply", args);
    }

    /// Reports where the server's time has gone, one row per role
    /// (`server::meters`): `/server_load.reply [uptime, n, n x (role, index,
    /// busy, calls)]`, with `uptime` and `busy` in seconds **since boot**.
    ///
    /// **Cumulative on purpose.** A window would have to be reset by whoever
    /// read it, which is what makes `/server_status`'s peak wrong for two
    /// pollers at once; here every client differences two replies and owns its
    /// own interval. `busy` is time the work was in progress, not per cent of
    /// a core: a DSP worker spinning for its next stage is burning a core and
    /// is idle by this reading.
    ///
    /// **A server with no clock says nil where a second would be** -- `uptime`
    /// and every `busy` -- and still counts: `calls` is how many times the
    /// work ran, which needs no clock.
    pub(in crate::osc::server) fn send_server_load(&mut self, to: ClientId) {
        let args = load_args(self.handle.meters());
        self.reply(to, "/server_load.reply", args);
    }

    /// Reports the server's static configuration so a client can size its own
    /// bus/allocator state from the server instead of hardcoding it:
    /// `/server_query.reply [audio_buses, control_buses, output_channels,
    /// block_size, nominal_sr, actual_sr, input_channels, max_nodes,
    /// max_buffers, max_graph_children, max_ugen_inputs, taps, tap_frames,
    /// max_frame, max_stream_buses, transports]`. The first six fields are
    /// stable; the
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
            OscType::Int(self.transports.len() as i32),
        ];
        self.reply(to, "/server_query.reply", args);
    }

    /// The sample-clock query. Replies `/clock_query.reply` with the engine's
    /// sample counter (int64 `h`), the actual sample rate (double `d`) and the
    /// server's OSC/NTP time captured with the counter (timetag `t`). The
    /// `(osc_time, sample)` pair is the master-clock **anchor**: a client maps
    /// its logical OSC time `T` to this server's sample axis with
    /// `S0 + (T - T0)*rate` and schedules with `/sched_at` ([`Self::handle_sched_at`])
    /// directly in samples -- see `docs/sample-clock.md`. Clients that only want
    /// the older two-field form ignore the trailing timetag. The counter counts
    /// *processed* samples: it runs a device buffer ahead of the speakers and
    /// pauses on xruns.
    pub(in crate::osc::server) fn handle_clock_query(&mut self, from: ClientId) {
        // The anchor pairs the counter with the instant that sample falls on,
        // through the same line a wall-clock timetag is placed with, so a
        // client mapping between the two axes agrees with the server. With no
        // line published, the counter and the wall clock read back-to-back.
        let sample = self.handle.current_samples();
        let osc = self
            .handle
            .device_epoch()
            .get()
            .filter(|_| matches!(self.clock, TimeSource::Wall { .. }))
            .map_or_else(
                || self.now_ntp(),
                |epoch| unix_to_ntp(epoch + sample as f64 / self.handle.sample_rate as f64),
            );
        let args = vec![
            OscType::Long(sample as i64),
            OscType::Double(self.info.actual_sample_rate),
            OscType::Time(osc),
        ];
        self.reply(from, "/clock_query.reply", args);
    }

    /// `/server_errorMode mode`: sets the error-posting mode. `1` posts command errors to
    /// the server console (the default), `0` silences them. The `/fail` OSC
    /// reply is always sent regardless -- clients rely on it; only the
    /// server-side console logging is gated. scsynth's bundle-local `-1`/`-2`
    /// are not separately supported (deliberate deviation): the persistent
    /// `0`/`1` toggle is the model that fits our logging.
    pub(in crate::osc::server) fn handle_server_error_mode(&mut self, mut args: Args) -> Answer {
        self.post_errors = args.int()? != 0;
        Ok(())
    }

    /// `/server_cmd name args...`: a server-wide, typed command -- the discoverable
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
            "ping" => self.done_with(from, "/server_cmd", vec![OscType::String("ping".into())]),
            other => return Err(format!("unknown server command {other:?}")),
        }
        Ok(())
    }

    /// `/ugen_query [kind...]` -> one `/ugen_query.reply` per UGen, then `/done "/ugen_query"`
    ///: the catalog straight from the `dsp::registry` descriptors, so a
    /// palette derives from the server's truth instead of a client-side copy.
    /// An unknown kind replies with an empty rate set and no inputs.
    ///
    /// Faust primitives are deliberately absent: that vocabulary is Faust's
    /// own and already lives in the client builders.
    ///
    /// Built without the `synth` feature there is no UGen catalog at all, and
    /// the honest reply is an **empty** listing rather than a `/fail` -- the
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
        self.done(from, "/ugen_query");
        Ok(())
    }

    pub(in crate::osc::server) fn handle_server_notify(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        match args.int()? {
            1 => {
                let id = match self.clients.iter().find(|(c, _)| *c == from) {
                    Some((_, id)) => *id,
                    None => {
                        let id = self.next_notify_id;
                        self.next_notify_id += 1;
                        self.clients.push((from, id));
                        // The first subscriber shortens the loop's tick: a
                        // node event comes from the audio thread, which cannot
                        // wake it (`NOTIFY_INTERVAL`).
                        self.retune_timeout();
                        id
                    }
                };
                self.done_with(from, "/server_notify", vec![OscType::Int(id)]);
            }
            0 => {
                self.clients.retain(|(c, _)| *c != from);
                // And the last one hands the idle tick back.
                self.retune_timeout();
                self.done(from, "/server_notify");
            }
            other => return Err(format!("expected 0 or 1, got {other}")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::meters::Role;

    /// **A server with no clock says so, field by field.** Zero seconds reads
    /// as idle; nil reads as what it is. The counts are still there, since a
    /// run is counted and not timed.
    #[test]
    fn a_load_reply_without_a_clock_carries_nil_seconds_and_real_counts() {
        let meters = Meters::untimed(0);
        meters.add(Role::Audio, 0, 0);
        meters.add(Role::Audio, 0, 0);
        let args = load_args(&meters);
        assert_eq!(args[0], OscType::Nil, "no uptime");
        let OscType::Int(rows) = args[1] else {
            panic!("a row count");
        };
        assert_eq!(args.len(), 2 + 4 * rows as usize, "the same shape");
        assert_eq!(args[2], OscType::String("audio".into()));
        assert_eq!(args[4], OscType::Nil, "no seconds for the audio role");
        assert_eq!(args[5], OscType::Long(2), "and both its runs counted");
        assert!(
            args[2..].chunks(4).all(|row| row[2] == OscType::Nil),
            "nor for any other"
        );
    }

    /// And a timed one is the reply it always was.
    #[test]
    fn a_load_reply_with_a_clock_carries_seconds() {
        let meters = Meters::new(0);
        meters.add(Role::Audio, 0, 2_000_000);
        let args = load_args(&meters);
        assert!(matches!(args[0], OscType::Double(_)));
        assert_eq!(args[4], OscType::Double(0.002));
        assert_eq!(args[5], OscType::Long(1));
    }

    #[test]
    fn the_cpu_figures_are_nil_without_a_clock() {
        let counters = Counters::default();
        assert_eq!(
            cpu_args(&counters, false),
            [OscType::Nil, OscType::Nil, OscType::Nil]
        );
        assert_eq!(
            cpu_args(&counters, true),
            [OscType::Float(0.0), OscType::Float(0.0), OscType::Int(0)]
        );
    }
}
