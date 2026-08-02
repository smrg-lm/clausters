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
    pub(in crate::osc::server) fn handle_buffer_close(&mut self, msg: &OscMessage, from: ClientId) {
        let Some(OscType::Int(index)) = msg.args.first() else {
            return self.fail(from, "/buffer_close", "expected int buffer index");
        };
        let live = usize::try_from(*index)
            .ok()
            .and_then(|i| self.translator.buffers.get(i))
            .is_some_and(Option::is_some);
        if live {
            self.reply(
                from,
                "/done",
                vec![
                    OscType::String("/buffer_close".into()),
                    OscType::Int(*index),
                ],
            );
        } else {
            self.fail(
                from,
                "/buffer_close",
                format!("buffer {index} not allocated"),
            );
        }
    }

    /// Any of the async `/buffer_*` commands: parsing is shared with the NRT
    /// renderer; the job runs on the NRT thread. `/buffer_free` also travels
    /// through the queue so it cannot overtake a pending alloc/read on the
    /// same index.
    pub(in crate::osc::server) fn handle_buffer_cmd(
        &mut self,
        msg: &OscMessage,
        from: ClientId,
        cmd: &'static str,
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
    pub(in crate::osc::server) fn handle_buffer_query(&mut self, msg: &OscMessage, from: ClientId) {
        // No argument lists every allocated buffer (M30): the patcher shows
        // real buffers as objects, and a client that never allocated them (the
        // pool outlives a session) has no other way to learn they exist.
        if msg.args.is_empty() {
            let args = self.translator.buffer_list();
            return self.reply(from, "/buffer_query.reply", args);
        }
        let mut args = Vec::with_capacity(msg.args.len() * 4);
        for arg in &msg.args {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/buffer_query", "expected int buffer indices");
            };
            let info = self.mirror_buffer(*index);
            // An unallocated slot answers with `frames = -1`: absence is a
            // state reported in the record, like `/node_query`'s `isGroup = -1`
            // and `/def_query`'s empty family, so one dead index does not abort
            // the batch.
            args.push(OscType::Int(*index));
            args.push(OscType::Int(
                info.as_ref().map_or(-1, |b| b.frames() as i32),
            ));
            args.push(OscType::Int(
                info.as_ref().map_or(0, |b| b.channels() as i32),
            ));
            args.push(OscType::Float(
                info.as_ref().map_or(0.0, |b| b.sample_rate() as f32),
            ));
        }
        self.reply(from, "/buffer_query.reply", args);
    }

    /// `/buffer_get bufnum index...` → `/buffer_get.reply bufnum index value...`: read single
    /// samples (flat, interleaved) from the buffer mirror. Out-of-range indices
    /// (and any index into an unallocated buffer) read as `0.0`, mirroring how
    /// `Buffer::sample` and the audio-rate UGens treat them. Synchronous, like
    /// `/buffer_query`.
    pub(in crate::osc::server) fn handle_buffer_get(&mut self, msg: &OscMessage, from: ClientId) {
        let Some((OscType::Int(bufnum), indices)) = msg.args.split_first() else {
            return self.fail(
                from,
                "/buffer_get",
                "expected bufnum then int sample indices",
            );
        };
        let buffer = self.mirror_buffer(*bufnum);
        let data = buffer.as_deref().map(|b| b.data()).unwrap_or(&[]);
        let mut args = vec![OscType::Int(*bufnum)];
        for arg in indices {
            let OscType::Int(index) = arg else {
                return self.fail(from, "/buffer_get", "expected int sample indices");
            };
            let value = usize::try_from(*index)
                .ok()
                .and_then(|i| data.get(i))
                .copied()
                .unwrap_or(0.0);
            args.push(OscType::Int(*index));
            args.push(OscType::Float(value));
        }
        self.reply(from, "/buffer_get.reply", args);
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
        msg: &OscMessage,
        from: ClientId,
    ) {
        let Some((OscType::Int(bufnum), pairs)) = msg.args.split_first() else {
            return self.fail(
                from,
                "/buffer_getRange",
                "expected bufnum then (start, count) pairs",
            );
        };
        if pairs.len() % 2 != 0 {
            return self.fail(from, "/buffer_getRange", "expected (start, count) pairs");
        }
        let buffer = self.mirror_buffer(*bufnum);
        let data = buffer.as_deref().map(|b| b.data()).unwrap_or(&[]);
        let mut args = vec![OscType::Int(*bufnum)];
        for pair in pairs.chunks_exact(2) {
            let (OscType::Int(start), OscType::Int(count)) = (&pair[0], &pair[1]) else {
                return self.fail(from, "/buffer_getRange", "expected int start and count");
            };
            let start = (*start).max(0) as usize;
            let count = (*count).max(0) as usize;
            let end = start.saturating_add(count).min(data.len());
            let slice = data.get(start..end).unwrap_or(&[]);
            args.push(OscType::Int(start as i32));
            args.push(OscType::Int(slice.len() as i32));
            args.extend(slice.iter().map(|s| OscType::Float(*s)));
        }
        self.reply(from, "/buffer_getRange.reply", args);
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
        msg: &OscMessage,
        from: ClientId,
    ) {
        let (Some(OscType::Int(bufnum)), Some(OscType::String(path))) =
            (msg.args.first(), msg.args.get(1))
        else {
            return self.fail(from, "/buffer_export", "expected bufnum then a path string");
        };
        let Some(buffer) = self.mirror_buffer(*bufnum) else {
            return self.fail(from, "/buffer_export", "no such buffer");
        };
        let mut bytes = Vec::with_capacity(buffer.data().len() * 4);
        for &s in buffer.data() {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        match std::fs::write(path, &bytes) {
            Ok(()) => self.reply(
                from,
                "/done",
                vec![
                    OscType::String("/buffer_export".into()),
                    OscType::Int(*bufnum),
                ],
            ),
            Err(e) => self.fail(from, "/buffer_export", format!("write {path}: {e}")),
        }
    }

    fn mirror_buffer(&self, index: i32) -> Option<Arc<crate::dsp::buffer::Buffer>> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.translator.buffers.get(i))
            .and_then(|b| b.as_ref().map(Arc::clone))
    }
}
