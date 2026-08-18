//! Subscriptions: the server pushing without being asked again.
//!
//! `/bus_stream` (control-bus snapshots) and `/bus_tapStream` (windows of audio
//! samples) are one subscription per client, replaced by each new request. They
//! exist for clients that cannot map the shared-memory segment -- a browser over
//! WebSocket -- and their pacing reads [`OscServer::mono_secs`], so a headless
//! server paces on the sample clock and stays deterministic.
//!
//! A tap subscription *is* the watch on its buses: it starts recording them and
//! releases them when it is replaced, cancelled, or its connection dies, so a
//! streaming client never sends `/bus_tap` itself.

use super::*;

impl OscServer {
    /// `/bus_stream periodMs busIndex...`: subscribes this client to a periodic
    /// `/bus_set` snapshot of the listed control buses — the network counterpart
    /// of reading the shared-memory segment, for clients that cannot map it (a
    /// browser GUI host's meters/scopes over WebSocket). One subscription per
    /// client, replaced on every call; `periodMs <= 0` or an empty list
    /// cancels. Acks `/done "/bus_stream"`, then sends the first snapshot
    /// immediately and the rest from the run loop. Not schedulable in timed
    /// bundles. Subscriptions die with their TCP/WS connection; UDP and ring
    /// clients cancel explicitly (same posture as `/server_notify`).
    pub(in crate::osc::server) fn handle_bus_stream(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(OscType::Int(period_ms)) = msg.args.first() else {
            return self.fail(from, "/bus_stream", "expected int periodMs");
        };
        let mut buses = Vec::with_capacity(msg.args.len().saturating_sub(1));
        for arg in &msg.args[1..] {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/bus_stream", "expected int bus indices");
            };
            if *index < 0 {
                return self.fail(from, "/bus_stream", "bus index must be non-negative");
            }
            buses.push(*index);
        }
        if buses.len() > MAX_STREAM_BUSES {
            return self.fail(
                from,
                "/bus_stream",
                format!("at most {MAX_STREAM_BUSES} bus indices per subscription"),
            );
        }
        self.streams.retain(|s| s.client != from);
        self.reply(from, "/done", vec![OscType::String("/bus_stream".into())]);
        if *period_ms > 0 && !buses.is_empty() {
            let period = Duration::from_millis(*period_ms as u64).max(MIN_STREAM_PERIOD);
            self.streams.push(BusStream {
                client: from,
                period,
                buses,
                next_due: self.mono_secs() + period.as_secs_f64(),
            });
            // The immediate snapshot: the client paints without waiting a period.
            let args = self.stream_args(self.streams.len() - 1);
            self.reply(from, "/bus_stream.reply", args);
        }
        self.retune_timeout();
    }

    /// Sends every due stream its `/bus_stream.reply` snapshot. Called once per run-loop
    /// iteration; the socket timeout is tuned so an idle loop still ticks at
    /// the fastest subscribed period (see [`Self::retune_timeout`]). Reading a
    /// control bus is one relaxed atomic load — no engine round-trip.
    pub(in crate::osc::server) fn pump_streams(&mut self) {
        if self.streams.is_empty() {
            return;
        }
        let now = self.mono_secs();
        for i in 0..self.streams.len() {
            if now < self.streams[i].next_due {
                continue;
            }
            let client = self.streams[i].client;
            let args = self.stream_args(i);
            self.reply(client, "/bus_stream.reply", args);
            // Rebase on `now` (no catch-up bursts after a stall).
            let period = self.streams[i].period;
            self.streams[i].next_due = now + period.as_secs_f64();
        }
    }

    /// The `(busIndex, value)` pairs of stream `i`'s snapshot.
    fn stream_args(&self, i: usize) -> Vec<OscType> {
        let buses = &self.streams[i].buses;
        let mut args = Vec::with_capacity(buses.len() * 2);
        for &bus in buses {
            args.push(OscType::Int(bus));
            args.push(OscType::Float(
                self.handle.control_buses().get(bus as usize),
            ));
        }
        args
    }

    /// Starts recording audio bus `bus`, if it is not already: picks a free
    /// ring, tells the engine to append that bus to it every block, and counts
    /// one more watcher. Idempotent per watcher — two views of the same bus
    /// share one ring — and the caller never learns the index: the segment's
    /// directory maps the bus to it (see [`Segment::tap_of_bus`]).
    ///
    /// [`Segment::tap_of_bus`]: crate::server::ipc::Segment::tap_of_bus
    fn watch_bus(&mut self, bus: i32) -> Result<(), String> {
        let Some(segment) = self.handle.segment() else {
            return Err("no tap region (server started with --taps 0)".into());
        };
        let taps = segment.taps();
        let audio_buses = self.handle.audio_buses;
        if bus < 0 || bus as usize >= audio_buses {
            return Err(format!("bus must be in range 0..{audio_buses}"));
        }
        self.tap_rings.resize(taps, -1);
        self.tap_refs.resize(taps, 0);
        if let Some(ring) = self.tap_rings.iter().position(|&b| b == bus) {
            self.tap_refs[ring] += 1;
            return Ok(());
        }
        let Some(ring) = self.tap_rings.iter().position(|&b| b < 0) else {
            return Err(format!("all {taps} audio taps are in use"));
        };
        if self.handle.send(Cmd::SetTap { tap: ring, bus }).is_err() {
            return Err("command FIFO full".into());
        }
        self.tap_rings[ring] = bus;
        self.tap_refs[ring] = 1;
        Ok(())
    }

    /// Drops one watcher of `bus`, freeing its ring when the last one goes.
    fn release_bus(&mut self, bus: i32) {
        let Some(ring) = self.tap_rings.iter().position(|&b| b == bus) else {
            return;
        };
        self.tap_refs[ring] = self.tap_refs[ring].saturating_sub(1);
        if self.tap_refs[ring] == 0 {
            // Best effort: a full FIFO leaves the ring recording, and the next
            // watcher of this bus reuses it rather than taking a second one.
            if self.handle.send(Cmd::SetTap { tap: ring, bus: -1 }).is_ok() {
                self.tap_rings[ring] = -1;
            } else {
                self.tap_refs[ring] = 1;
            }
        }
    }

    /// `/bus_tap bus watch`: asks the server to make audio bus `bus` readable —
    /// `watch = 1` starts, `0` stops. **The bus is the only number a client
    /// names**: which of the segment's rings carries it is the server's own
    /// bookkeeping, published in the segment's bus directory for whoever reads
    /// the samples (a GUI host's oscilloscope, with zero messages per frame).
    /// Watches count, so two views of one bus share a ring and the last one to
    /// stop frees it. No ack (the same posture as `/node_map`: it only flips
    /// routing state); sequence with `/server_sync` when needed. Fails without a tap
    /// region (server started with `--taps 0`) or when every ring is taken.
    pub(in crate::osc::server) fn handle_bus_tap(&mut self, msg: &OscMessage, from: ClientId) {
        let (Some(OscType::Int(bus)), Some(OscType::Int(watch))) =
            (msg.args.first(), msg.args.get(1))
        else {
            return self.fail(from, "/bus_tap", "expected int bus, int watch");
        };
        // Both directions answer the same way about an impossible request, so
        // a client learns it cannot watch anything from the first call.
        if self.handle.segment().is_none() {
            return self.fail(
                from,
                "/bus_tap",
                "no tap region (server started with --taps 0)",
            );
        }
        let audio_buses = self.handle.audio_buses;
        if *bus < 0 || *bus as usize >= audio_buses {
            return self.fail(
                from,
                "/bus_tap",
                format!("bus must be in range 0..{audio_buses}"),
            );
        }
        if *watch == 0 {
            return self.release_bus(*bus);
        }
        if let Err(why) = self.watch_bus(*bus) {
            self.fail(from, "/bus_tap", why);
        }
    }

    /// `/bus_tapStream periodMs frames bus...`: subscribes this client to a
    /// periodic `/bus_tapStream.reply` snapshot — the newest `frames` samples of each
    /// listed **audio bus** — the network counterpart of reading the segment's
    /// tap rings, for clients (a browser oscilloscope) that cannot map it. The
    /// subscription *is* the watch: it starts recording each bus it lists and
    /// stops when it is replaced, cancelled or its connection dies, so a
    /// streaming client never issues `/bus_tap` at all. One subscription per
    /// client, replaced on every call; `periodMs <= 0` or an empty bus list
    /// cancels. Acks `/done "/bus_tapStream"`, then sends the first snapshot
    /// immediately and the rest from the run loop. Not schedulable in timed
    /// bundles. Subscriptions die with their TCP/WS connection, like
    /// `/bus_stream`.
    pub(in crate::osc::server) fn handle_bus_tap_stream(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let (Some(OscType::Int(period_ms)), Some(OscType::Int(frames))) =
            (msg.args.first(), msg.args.get(1))
        else {
            return self.fail(from, "/bus_tapStream", "expected int periodMs, int frames");
        };
        let Some(segment) = self.handle.segment() else {
            return self.fail(
                from,
                "/bus_tapStream",
                "no tap region (server started with --taps 0)",
            );
        };
        let audio_buses = self.handle.audio_buses;
        let mut buses = Vec::with_capacity(msg.args.len().saturating_sub(2));
        for arg in &msg.args[2..] {
            let OscType::Int(bus) = arg else {
                return self.fail(from, "/bus_tapStream", "expected int bus indices");
            };
            if *bus < 0 || *bus as usize >= audio_buses {
                return self.fail(
                    from,
                    "/bus_tapStream",
                    format!("bus out of range 0..{audio_buses}"),
                );
            }
            buses.push(*bus);
        }
        if buses.len() > MAX_STREAM_TAPS {
            return self.fail(
                from,
                "/bus_tapStream",
                format!("at most {MAX_STREAM_TAPS} buses per subscription"),
            );
        }
        // Clamp, don't fail, the window: to the client's transport bound and
        // to half the tap ring (the tear-free bound of `tap_read_latest`). A
        // stream client may fill a whole frame (minus the OSC envelope); a
        // datagram-bounded one keeps the 32 KB blob cap.
        let transport_cap = match from {
            ClientId::Tcp(_) | ClientId::Ws(_) => self.max_frame.saturating_sub(256) / 4,
            ClientId::Udp(_) | ClientId::Ring(_) => MAX_TAP_WINDOW,
        };
        let frames = (*frames).max(1) as usize;
        let frames = frames.min(transport_cap).min(segment.tap_frames() / 2);
        // The new subscription's watches are taken before the old one's are
        // dropped, so re-subscribing to the same bus never stops recording it.
        let wanted = if *period_ms > 0 {
            buses.clone()
        } else {
            Vec::new()
        };
        for bus in &wanted {
            if let Err(why) = self.watch_bus(*bus) {
                for taken in wanted.iter().take_while(|b| *b != bus) {
                    self.release_bus(*taken);
                }
                return self.fail(from, "/bus_tapStream", why);
            }
        }
        self.drop_tap_streams(|s| s.client == from);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/bus_tapStream".into())],
        );
        if !wanted.is_empty() {
            let period = Duration::from_millis(*period_ms as u64).max(MIN_STREAM_PERIOD);
            self.tap_streams.push(TapStream {
                client: from,
                period,
                frames,
                buses,
                next_due: self.mono_secs() + period.as_secs_f64(),
            });
            // The immediate snapshot: the client paints without waiting a
            // period (taps that have not yet filled a window send nothing).
            self.send_tap_snapshots(self.tap_streams.len() - 1);
        }
        self.retune_timeout();
    }

    /// Removes the tap subscriptions matching `doomed` and releases the watch
    /// each held on its buses — the one place a subscription's recording stops,
    /// whether it was replaced, cancelled or lost with its connection.
    pub(in crate::osc::server) fn drop_tap_streams(&mut self, doomed: impl Fn(&TapStream) -> bool) {
        let mut released = Vec::new();
        self.tap_streams.retain(|s| {
            if doomed(s) {
                released.extend_from_slice(&s.buses);
                false
            } else {
                true
            }
        });
        for bus in released {
            self.release_bus(bus);
        }
    }

    /// Sends every due tap stream its `/bus_tapStream.reply` snapshots. Called once per
    /// run-loop iteration, like [`Self::pump_streams`]. Reading a tap ring is
    /// a lock-free shared-memory copy — no engine round-trip.
    pub(in crate::osc::server) fn pump_tap_streams(&mut self) {
        if self.tap_streams.is_empty() {
            return;
        }
        let now = self.mono_secs();
        for i in 0..self.tap_streams.len() {
            if now < self.tap_streams[i].next_due {
                continue;
            }
            self.send_tap_snapshots(i);
            // Rebase on `now` (no catch-up bursts after a stall).
            let period = self.tap_streams[i].period;
            self.tap_streams[i].next_due = now + period.as_secs_f64();
        }
    }

    /// One `/bus_tapStream.reply tap endPosition blob` per tap of stream `i` that has a
    /// full window: `endPosition` is the tap's stream position (total samples
    /// written) at the window's end — consecutive snapshots overlap or gap by
    /// exactly the position delta — and the blob is the window's raw
    /// little-endian `f32` samples.
    fn send_tap_snapshots(&mut self, i: usize) {
        let Some(segment) = self.handle.segment().cloned() else {
            return;
        };
        let client = self.tap_streams[i].client;
        let frames = self.tap_streams[i].frames;
        self.tap_buf.resize(frames, 0.0);
        for k in 0..self.tap_streams[i].buses.len() {
            let bus = self.tap_streams[i].buses[k];
            // The bus is the key here too: the ring it landed in is looked up,
            // never carried in the subscription.
            let Some(tap) = segment.tap_of_bus(bus as usize) else {
                continue;
            };
            let Some(end) = segment.tap_read_latest(tap, &mut self.tap_buf) else {
                continue;
            };
            let mut bytes = Vec::with_capacity(frames * 4);
            for s in &self.tap_buf {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            self.reply(
                client,
                "/bus_tapStream.reply",
                vec![
                    OscType::Int(bus),
                    OscType::Long(end as i64),
                    OscType::Blob(bytes),
                ],
            );
        }
    }

    /// `/buffer_stream periodMs bucket bufnum...`: subscribes this client to
    /// the **overview of material as it is written** — what a peer that can
    /// map the region reads for free, for a client that cannot.
    ///
    /// The server acks `/done "/buffer_stream"` and then sends, every
    /// `periodMs`, one `/buffer_stream.reply bufnum startFrame bucket blob`
    /// per watched buffer whose write frontier has moved past a whole bucket
    /// since the last report — and nothing at all for one that has not moved,
    /// so a still buffer costs no traffic.
    ///
    /// **The unit is the summary and not the samples**, which is the whole
    /// argument for the command: min, max and mean square per `bucket` frames
    /// per channel is 2.2 kB/s for one channel at 48 kHz, against 192 kB/s for
    /// the audio it describes. A page can watch a stereo take record for the
    /// price of a meter.
    ///
    /// Same posture as the other two: one subscription per client, replaced on
    /// each call, `periodMs <= 0` (or no buffers) cancels, it dies with a
    /// TCP/WebSocket connection, and it is not schedulable in a bundle.
    pub(in crate::osc::server) fn handle_buffer_stream(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let (Some(OscType::Int(period_ms)), Some(OscType::Int(bucket))) =
            (msg.args.first(), msg.args.get(1))
        else {
            return self.fail(from, "/buffer_stream", "expected int periodMs, int bucket");
        };
        let mut buffers = Vec::with_capacity(msg.args.len().saturating_sub(2));
        for arg in &msg.args[2..] {
            let OscType::Int(bufnum) = arg else {
                return self.fail(from, "/buffer_stream", "expected int buffer numbers");
            };
            if *bufnum < 0 {
                return self.fail(from, "/buffer_stream", "buffer number must be non-negative");
            }
            // The frontier a report starts from is where the buffer *is* now:
            // a subscription is a watch on what happens next, not a request
            // for the overview of what is already there (that is a fetch, and
            // `/buffer_getRange` is how it is spelled).
            let from_frame = self
                .handle
                .segment()
                .and_then(|seg| seg.buffer_frontier(*bufnum as usize))
                .unwrap_or(0);
            buffers.push((*bufnum, from_frame));
        }
        if buffers.len() > MAX_STREAM_BUFFERS {
            return self.fail(
                from,
                "/buffer_stream",
                format!("at most {MAX_STREAM_BUFFERS} buffers per subscription"),
            );
        }
        let bucket = (*bucket).max(1) as usize;
        self.buffer_streams.retain(|s| s.client != from);
        self.reply(
            from,
            "/done",
            vec![OscType::String("/buffer_stream".into())],
        );
        if *period_ms > 0 && !buffers.is_empty() {
            let period = Duration::from_millis(*period_ms as u64).max(MIN_STREAM_PERIOD);
            self.buffer_streams.push(BufferStream {
                client: from,
                period,
                buffers,
                bucket,
                next_due: self.mono_secs() + period.as_secs_f64(),
            });
        }
        self.retune_timeout();
    }

    /// Sends every due buffer stream what its material grew since the last
    /// report. Called once per run-loop iteration, like the other two pumps.
    pub(in crate::osc::server) fn pump_buffer_streams(&mut self) {
        if self.buffer_streams.is_empty() {
            return;
        }
        let now = self.mono_secs();
        for i in 0..self.buffer_streams.len() {
            if now < self.buffer_streams[i].next_due {
                continue;
            }
            self.send_buffer_overviews(i);
            let period = self.buffer_streams[i].period;
            self.buffer_streams[i].next_due = now + period.as_secs_f64();
        }
    }

    /// One `/buffer_stream.reply bufnum startFrame bucket blob` per buffer of
    /// stream `i` that grew by at least one whole bucket.
    ///
    /// The blob is bucket-major and channel-minor: for each bucket in order,
    /// for each channel, `min`, `max` and **mean square** as raw little-endian
    /// `f32` — the same three statistics the peak pyramid stores, in the same
    /// energy form, so a client folds them into its own summary without
    /// converting anything.
    ///
    /// Summarizing here reads the buffer's cells while the engine may be
    /// writing them, which is the buffer model's own promise (some old samples
    /// and some new, never half of one) and is what every reader of a live
    /// take gets.
    fn send_buffer_overviews(&mut self, i: usize) {
        let client = self.buffer_streams[i].client;
        let bucket = self.buffer_streams[i].bucket;
        for k in 0..self.buffer_streams[i].buffers.len() {
            let (bufnum, reported) = self.buffer_streams[i].buffers[k];
            let frontier = self
                .handle
                .segment()
                .and_then(|seg| seg.buffer_frontier(bufnum as usize))
                .unwrap_or(0);
            let start = (reported / bucket as u64) * bucket as u64;
            let whole = frontier.saturating_sub(start) / bucket as u64;
            if whole == 0 {
                continue;
            }
            let Some(buffer) = self
                .translator
                .buffers
                .get(bufnum as usize)
                .and_then(|b| b.as_ref())
            else {
                continue;
            };
            let channels = buffer.channels().max(1);
            let whole = whole.min(MAX_STREAM_BUCKETS as u64) as usize;
            let mut bytes = Vec::with_capacity(whole * channels * 3 * 4);
            for b in 0..whole {
                let from = start as usize + b * bucket;
                for ch in 0..channels {
                    let (mut lo, mut hi, mut sum) = (f32::INFINITY, f32::NEG_INFINITY, 0.0f64);
                    for f in from..from + bucket {
                        let v = buffer.sample(f, ch);
                        lo = lo.min(v);
                        hi = hi.max(v);
                        sum += (v as f64) * (v as f64);
                    }
                    let ms = (sum / bucket as f64) as f32;
                    for value in [lo, hi, ms] {
                        bytes.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
            let end = start + (whole * bucket) as u64;
            self.buffer_streams[i].buffers[k] = (bufnum, end);
            self.reply(
                client,
                "/buffer_stream.reply",
                vec![
                    OscType::Int(bufnum),
                    OscType::Long(start as i64),
                    OscType::Int(bucket as i32),
                    OscType::Blob(bytes),
                ],
            );
        }
    }
}
