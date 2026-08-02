//! `/buffer_*`: the buffer pool, and the reads that answer immediately.
//!
//! Anything that allocates, reads a file or writes one is submitted to the NRT
//! thread ([`super::super::async_pipes`]) and answers `/done` later; the
//! queries here answer from the mirror in the same turn.

use super::super::*;

impl OscServer {
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
        let data = buffer.as_deref().map(|b| b.data()).unwrap_or(&[]);
        let mut out = vec![OscType::Int(bufnum)];
        while !args.is_empty() {
            let index = args.int()?;
            let value = usize::try_from(index)
                .ok()
                .and_then(|i| data.get(i))
                .copied()
                .unwrap_or(0.0);
            out.push(OscType::Int(index));
            out.push(OscType::Float(value));
        }
        self.reply(from, "/buffer_get.reply", out);
        Ok(())
    }

    /// `/buffer_getRange bufnum [start count]...` → `/buffer_getRange.reply bufnum start count value...`:
    /// read ranges of samples (flat, interleaved) from the buffer mirror — the
    /// client-side counterpart of `/buffer_getRange.reply`, and how a GUI client pulls a buffer
    /// to display it. `count` is clamped to what the buffer holds from `start`,
    /// so a request past the end returns only the available samples (none for an
    /// unallocated buffer). Large buffers are read in client-chosen chunks,
    /// sized to the client's transport: a stream client (TCP/WS) may ask for
    /// up to the `/server_query` frame ceiling per reply, a UDP client must
    /// stay under the datagram cap. The shm bulk path is future work.
    pub(in crate::osc::server) fn handle_buffer_get_range(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let bufnum = args.int()?;
        args.expect_groups_of(2, "(start, count) pairs")?;
        let buffer = self.mirror_buffer(bufnum);
        let data = buffer.as_deref().map(|b| b.data()).unwrap_or(&[]);
        let mut out = vec![OscType::Int(bufnum)];
        while !args.is_empty() {
            let start = args.int()?.max(0) as usize;
            let count = args.int()?.max(0) as usize;
            let end = start.saturating_add(count).min(data.len());
            let slice = data.get(start..end).unwrap_or(&[]);
            out.push(OscType::Int(start as i32));
            out.push(OscType::Int(slice.len() as i32));
            out.extend(slice.iter().map(|s| OscType::Float(*s)));
        }
        self.reply(from, "/buffer_getRange.reply", out);
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
        let mut bytes = Vec::with_capacity(buffer.data().len() * 4);
        for &s in buffer.data() {
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
