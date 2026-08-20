//! `/buffer_*`: the buffer pool, and the reads that answer immediately.
//!
//! Anything that allocates, reads a file or writes one is submitted to the NRT
//! thread ([`super::super::async_pipes`]) and answers `/done` later; the
//! queries here answer from the mirror in the same turn.

use super::super::*;

impl OscServer {
    /// `/buffer_attach bufnum` — map the shared buffer `bufnum` out of the
    /// segment this server attached to, so its engine plays the owner's very
    /// cells.
    ///
    /// The command exists because **samples never travel and allocation
    /// always does**: a peer editing a take writes into memory this server
    /// already reads, but a take that did not exist when this server started
    /// has to be pointed at. It is the RT server's half of the editor's
    /// arrangement — the editor allocates through the session that owns the
    /// samples, then tells the player where it is.
    pub(in crate::osc::server) fn handle_buffer_attach(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let index = args.index()?;
        self.attach_shared_buffer(index)?;
        self.reply(
            from,
            "/done",
            vec![
                OscType::String("/buffer_attach".into()),
                OscType::Int(index as i32),
            ],
        );
        Ok(())
    }

    /// `/buffer_touch bufnum channel start frames` — **a peer says it wrote
    /// samples**, so every other client learns the span changed.
    ///
    /// A local peer edits a shared buffer by storing into the mapped cells, and
    /// nothing about that reaches the wire: that is the point of mapping it,
    /// and it is also why a second client holding a picture of the same take
    /// would never find out. This is the announcement — the span and not the
    /// samples, four integers whoever cares re-reads with `/buffer_getRange`.
    ///
    /// It is a **notification, not a command**: nothing is answered to the
    /// sender, and the broadcast goes to every `/server_notify` client but the
    /// one that wrote, which already knows. A page gets it too, which is the
    /// point — a browser cannot map a file, so a message is the only way it can
    /// hear about an edit at all.
    pub(in crate::osc::server) fn handle_buffer_touch(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let index = args.index()?;
        let channel = args.int()?;
        let start = args.int()?;
        let frames = args.int()?;
        if !self
            .translator
            .buffers
            .get(index)
            .is_some_and(Option::is_some)
        {
            return Err(format!("buffer {index} not allocated"));
        }
        // **The overview beside the region is a reader like any other**, and
        // the only one that is this server's own: a peer wrote into the cells
        // and said where, so the summary over that span is what is stale.
        if let (Ok(start), Ok(frames)) = (usize::try_from(start), usize::try_from(frames))
            && let Some(buffer) = self
                .translator
                .buffers
                .get(index)
                .and_then(|b| b.as_ref().cloned())
        {
            self.overviews.wrote(index, &buffer, start, frames);
        }
        let payload = vec![
            OscType::Int(index as i32),
            OscType::Int(channel),
            OscType::Int(start),
            OscType::Int(frames),
        ];
        for client in self.clients.clone() {
            if client != from {
                self.reply(client, "/buffer_touched", payload.clone());
            }
        }
        Ok(())
    }

    /// `/buffer_close bufnum`: closes a soundfile a streaming buffer left open
    /// (scsynth pairs this with `DiskIn`/`DiskOut`). Clausters has no streaming
    /// buffers yet — every `/buffer_read`/`/buffer_write` reads or writes the whole file
    /// and closes it — so there is never an open handle: this validates the
    /// buffer is live and acknowledges, forward-compatible with the future
    /// streaming UGens.
    pub(in crate::osc::server) fn handle_buffer_close(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let index = args.index()?;
        if !self
            .translator
            .buffers
            .get(index)
            .is_some_and(Option::is_some)
        {
            return Err(format!("buffer {index} not allocated"));
        }
        self.reply(
            from,
            "/done",
            vec![
                OscType::String("/buffer_close".into()),
                OscType::Int(index as i32),
            ],
        );
        Ok(())
    }

    /// `/buffer_render bufnum frames`: run the graph for `frames` frames and
    /// install what came out of the output buses into `bufnum` — `/buffer_gen`'s
    /// sibling, generating into a buffer by *playing* rather than by formula,
    /// and the operation an editor means by "apply this def to this selection".
    ///
    /// **Only an offline server answers it.** Running the graph means driving
    /// `Engine::process_block`, and in a real-time server the audio device
    /// drives it against a wall clock nobody else may advance; there the
    /// command fails rather than pretending. Offline, the driver owns the clock
    /// and performs the request between commands (`server::nrtsession`), which
    /// is why this only queues one — see [`OfflineRender`](crate::osc::server::OfflineRender).
    ///
    /// The buffer must already exist and its shape is what it was allocated
    /// with: the caller says how long the operation is and how many channels it
    /// keeps by allocating for it, exactly as `/buffer_gen` keeps the shape it
    /// is given.
    pub(in crate::osc::server) fn handle_buffer_render(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let index = args.index()?;
        let frames = args.int()?;
        if self.offline.is_none() {
            return Err(
                "this server has no offline driver: /buffer_render needs a server whose \
                 clock it owns (an NRT session), not one driven by an audio device"
                    .into(),
            );
        }
        if frames <= 0 {
            return Err(format!("frames must be positive, got {frames}"));
        }
        if !self
            .translator
            .buffers
            .get(index)
            .is_some_and(Option::is_some)
        {
            return Err(format!("buffer {index} not allocated"));
        }
        // Queued, not performed: the answer goes out when the driver has run it.
        self.offline
            .as_mut()
            .expect("checked above")
            .push(crate::osc::server::OfflineRender {
                index,
                frames: frames as u64,
                client: from,
            });
        Ok(())
    }

    /// Any of the async `/buffer_*` commands: parsing is shared with the NRT
    /// renderer; the job runs on the NRT thread. `/buffer_free` also travels
    /// through the queue so it cannot overtake a pending alloc/read on the
    /// same index.
    pub(in crate::osc::server) fn handle_buffer_cmd(
        &mut self,
        cmd: &'static str,
        msg: &OscMessage,
        from: ClientId,
    ) {
        let (index, job) = match parse_buffer_msg(
            cmd,
            &msg.args,
            &self.translator.buffers,
            self.info.nominal_sample_rate,
        ) {
            Ok(parsed) => parsed,
            Err(e) => return self.fail(from, cmd, e),
        };
        self.submit_nrt(cmd, index, from, job);
    }

    /// `/buffer_gen bufnum cmd ...`: fills a buffer through the wavetable/generator
    /// path (see [`parse_buffer_gen`]). Async on the NRT queue, in submission order
    /// with the other `/buffer_*` commands, replying `/done`/`/fail` like them.
    pub(in crate::osc::server) fn handle_buffer_gen(&mut self, msg: &OscMessage, from: ClientId) {
        let (index, job) = match parse_buffer_gen(&msg.args, &self.translator.buffers) {
            Ok(parsed) => parsed,
            Err(e) => return self.fail(from, "/buffer_gen", e),
        };
        self.submit_nrt("/buffer_gen", index, from, job);
    }

    /// `/buffer_query bufnum...` → `/buffer_query.reply` with (bufnum, frames, channels,
    /// sampleRate) per buffer; zeros for unallocated indices. Synchronous,
    /// answered from the mirror (= state as of the last completed command).
    pub(in crate::osc::server) fn handle_buffer_query(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        // No argument lists every allocated buffer: the patcher shows
        // real buffers as objects, and a client that never allocated them (the
        // pool outlives a session) has no other way to learn they exist.
        if args.is_empty() {
            let all = self.translator.buffer_list();
            self.reply(from, "/buffer_query.reply", all);
            return Ok(());
        }
        let mut out = Vec::with_capacity(args.len() * 4);
        while !args.is_empty() {
            let index = args.int()?;
            let info = self.mirror_buffer(index);
            // An unallocated slot answers with `frames = -1`: absence is a
            // state reported in the record, like `/node_query`'s `isGroup = -1`
            // and `/def_query`'s empty family, so one dead index does not abort
            // the batch.
            out.push(OscType::Int(index));
            out.push(OscType::Int(
                info.as_ref().map_or(-1, |b| b.frames() as i32),
            ));
            out.push(OscType::Int(
                info.as_ref().map_or(0, |b| b.channels() as i32),
            ));
            out.push(OscType::Float(
                info.as_ref().map_or(0.0, |b| b.sample_rate() as f32),
            ));
        }
        self.reply(from, "/buffer_query.reply", out);
        Ok(())
    }

    /// `/buffer_get bufnum index...` → `/buffer_get.reply bufnum index value...`: read single
    /// samples (flat, interleaved) from the buffer mirror. Out-of-range indices
    /// (and any index into an unallocated buffer) read as `0.0`, mirroring how
    /// `Buffer::sample` and the audio-rate UGens treat them. Synchronous, like
    /// `/buffer_query`.
    pub(in crate::osc::server) fn handle_buffer_get(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let bufnum = args.int()?;
        let buffer = self.mirror_buffer(bufnum);
        let data = buffer.as_deref();
        let mut out = vec![OscType::Int(bufnum)];
        while !args.is_empty() {
            let index = args.int()?;
            let value = usize::try_from(index)
                .ok()
                .zip(data)
                .map_or(0.0, |(i, b)| b.at(i));
            out.push(OscType::Int(index));
            out.push(OscType::Float(value));
        }
        self.reply(from, "/buffer_get.reply", out);
        Ok(())
    }

    /// `/buffer_getRange bufnum [start count]...` → `/buffer_getRange.reply bufnum [start blob]...`:
    /// read ranges of samples (flat, interleaved) from the buffer mirror — how a
    /// GUI client pulls a buffer to display it, and the read half of
    /// `/buffer_setRange`. The request asks in samples; the reply carries each
    /// range as one **little-endian `f32` blob**, so its length is what actually
    /// came back and no declared count can disagree with it. `count` is clamped
    /// to what the buffer holds from `start`, so a request past the end returns
    /// only the available samples (an empty blob for an unallocated buffer).
    /// Large buffers are read in client-chosen chunks, sized to the client's
    /// transport: a stream client (TCP/WS) may ask for up to the `/server_query`
    /// frame ceiling per reply, a UDP client must stay under the datagram cap.
    /// The shm bulk path is future work.
    pub(in crate::osc::server) fn handle_buffer_get_range(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let bufnum = args.int()?;
        args.expect_groups_of(2, "(start, count) pairs")?;
        let buffer = self.mirror_buffer(bufnum);
        let data = buffer.as_deref();
        let held = data.map_or(0, |b| b.len());
        let mut out = vec![OscType::Int(bufnum)];
        while !args.is_empty() {
            let start = args.int()?.max(0) as usize;
            let count = args.int()?.max(0) as usize;
            let end = start.saturating_add(count).min(held);
            let mut blob = Vec::with_capacity(end.saturating_sub(start) * 4);
            for i in start..end {
                let s = data.map_or(0.0, |b| b.at(i));
                blob.extend_from_slice(&s.to_le_bytes());
            }
            out.push(OscType::Int(start as i32));
            out.push(OscType::Blob(blob));
        }
        self.reply(from, "/buffer_getRange.reply", out);
        Ok(())
    }

    /// `/buffer_peaks bufnum [bucket=256] [start=0] [frames=-1]` →
    /// `/buffer_peaks.reply bufnum startFrame bucket blob` (one or more):
    /// **the overview of a buffer that is standing still.**
    ///
    /// `/buffer_stream`'s sibling, and the pair is a distinction in the
    /// *material* and not in the client: a recording's overview is pushed as
    /// it is written, and a buffer nothing is writing has one that can simply
    /// be asked for. Same blob either way — bucket-major, channel-minor, min,
    /// max and mean square as little-endian `f32` — so the receiving half is
    /// the one both already have (`peaks::MultiPyramid::write_buckets`), and
    /// the folding code does not fork.
    ///
    /// **What it is for is the round trip it replaces.** A view of a server
    /// buffer that cannot map it had two ways to get a picture: download every
    /// sample (230 MB for a ten-minute stereo take, and the page's own bulk
    /// read gives up well below that), or have nothing until something records
    /// into it. This is the third: about a hundredth of the bandwidth, enough
    /// to draw the whole take at once, and the spans under a zoom read back
    /// with `/buffer_getRange` as they are needed.
    ///
    /// `bucket` is the summary's finest resolution and should be the one the
    /// asking pyramid was built at (256 unless it says otherwise), so the two
    /// grids agree by construction; `start` is rounded **down** to a whole
    /// bucket for the same reason. `frames < 0` runs to the end.
    ///
    /// **One request, one reply, and the reply's own length says how much
    /// came** — the chunk conversation `/buffer_getRange` already has, for the
    /// same reason: at most [`MAX_STREAM_BUCKETS`] buckets are answered at
    /// once, so no message is bounded by how long the take is, and a client
    /// walking a long take asks again from where the blob ended. Nothing is
    /// remembered between requests.
    ///
    /// Synchronous on the network thread, like `/buffer_get`, `/buffer_getRange`
    /// and `/buffer_export`: it reads the span once, a bucket at a time, and
    /// allocates only the summary.
    pub(in crate::osc::server) fn handle_buffer_peaks(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let bufnum = args.int()?;
        let bucket = args.opt_int()?.unwrap_or(256).max(1) as usize;
        let start = args.opt_int()?.unwrap_or(0).max(0) as usize;
        let asked = args.opt_int()?.unwrap_or(-1);
        let Some(buffer) = self.mirror_buffer(bufnum) else {
            return Err(format!("buffer {bufnum} not allocated"));
        };
        let frames = buffer.frames();
        // Rounded to the grid the answer is folded into: a bucket summarized
        // from part of itself would report a peak the samples do not have.
        let first = (start / bucket) * bucket;
        let end = match asked {
            n if n < 0 => frames,
            n => (start + n as usize).min(frames),
        };
        let buckets = end.saturating_sub(first) / bucket;
        if buckets == 0 {
            // Nothing whole to answer with, and the honest reply is an empty
            // one rather than silence: the asker learns the span held no
            // bucket, which is different from a request that went missing.
            self.reply(
                from,
                "/buffer_peaks.reply",
                vec![
                    OscType::Int(bufnum),
                    OscType::Long(first as i64),
                    OscType::Int(bucket as i32),
                    OscType::Blob(Vec::new()),
                ],
            );
            return Ok(());
        }
        let buckets = buckets.min(MAX_STREAM_BUCKETS);
        // **Out of the summary when there is one**, which is what the overview
        // beside the region is for: answering this without reading the samples
        // at all. Its grid is the file's, so a request at another bucket falls
        // back to the samples rather than being answered off a grid it does
        // not describe.
        let bytes = match self
            .overviews
            .span(bufnum.max(0) as usize, first, bucket, buckets)
        {
            Some(stats) => {
                let mut bytes = Vec::with_capacity(stats.len() * 4);
                for value in stats {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
                bytes
            }
            None => super::super::streams::overview_blob(&buffer, first, bucket, buckets),
        };
        self.reply(
            from,
            "/buffer_peaks.reply",
            vec![
                OscType::Int(bufnum),
                OscType::Long(first as i64),
                OscType::Int(bucket as i32),
                OscType::Blob(bytes),
            ],
        );
        Ok(())
    }

    /// `/buffer_export bufnum path` → `/done /buffer_export bufnum`: write the buffer's raw
    /// samples (flat, interleaved, little-endian `f32`) to `path` as a **local
    /// shared resource**, so a same-machine client (the GUI host) can map and read
    /// a multi-megabyte buffer with no per-sample OSC traffic — the bulk-data path,
    /// the efficient counterpart of `/buffer_getRange`'s chunked over-the-wire reads. The
    /// reader pairs it with the buffer's channel count (from `/buffer_query`) to
    /// de-interleave. Synchronous on the network thread (not the audio thread),
    /// like `/buffer_get`/`/buffer_getRange`; replies `/fail` on a missing buffer or a write
    /// error.
    pub(in crate::osc::server) fn handle_buffer_export(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let (bufnum, path) = (args.int()?, args.str()?);
        let Some(buffer) = self.mirror_buffer(bufnum) else {
            return Err(format!("buffer {bufnum} not allocated"));
        };
        // One snapshot, then the encode: an export is a reading of the buffer
        // at a moment, and taking it in one pass keeps it from straddling a
        // recording UGen's write head more than it has to.
        let samples = buffer.to_vec();
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, &bytes).map_err(|e| format!("write {path}: {e}"))?;
        self.reply(
            from,
            "/done",
            vec![
                OscType::String("/buffer_export".into()),
                OscType::Int(bufnum),
            ],
        );
        Ok(())
    }

    fn mirror_buffer(&self, index: i32) -> Option<Arc<crate::dsp::buffer::Buffer>> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.translator.buffers.get(i))
            .and_then(|b| b.as_ref().map(Arc::clone))
    }
}
