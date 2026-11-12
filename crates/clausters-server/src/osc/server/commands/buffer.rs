//! `/buffer_*`: the buffer pool, and the reads that answer immediately.
//!
//! Anything that allocates, reads a file or writes one is submitted to the NRT
//! thread ([`super::super::async_pipes`]) and answers `/done` later; the
//! queries here answer from the mirror in the same turn.

use super::super::*;
use crate::osc::translate::{allocated, mirror_buffer, readable};
use clausters_core::osc::{push_sample, sample_blob};

impl OscServer {
    /// `/buffer_attach bufnum` -- map the shared buffer `bufnum` out of the
    /// segment this server attached to, so its engine plays the owner's very
    /// cells.
    ///
    /// The command exists because **samples never travel and allocation
    /// always does**: a peer editing a take writes into memory this server
    /// already reads, but a take that did not exist when this server started
    /// has to be pointed at. It is the RT server's half of the editor's
    /// arrangement -- the editor allocates through the session that owns the
    /// samples, then tells the player where it is.
    pub(in crate::osc::server) fn handle_buffer_attach(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let index = args.index()?;
        self.attach_shared_buffer(index)?;
        self.done_with(from, "/buffer_attach", vec![OscType::Int(index as i32)]);
        Ok(())
    }

    /// `/buffer_touch bufnum channel start frames` -- **a peer says it wrote
    /// samples**, so every other client learns the span changed.
    ///
    /// A local peer edits a shared buffer by storing into the mapped cells, and
    /// nothing about that reaches the wire: that is the point of mapping it,
    /// and it is also why a second client holding a picture of the same take
    /// would never find out. This is the announcement -- the span and not the
    /// samples, four integers whoever cares re-reads with `/buffer_getRange`.
    ///
    /// It is a **notification, not a command**: nothing is answered to the
    /// sender, and the broadcast goes to every `/server_notify` client but the
    /// one that wrote, which already knows. A page gets it too, which is the
    /// point -- a browser cannot map a file, so a message is the only way it can
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
        allocated(&self.translator.buffers, index as i32)?;
        let payload = vec![
            OscType::Int(index as i32),
            OscType::Int(channel),
            OscType::Int(start),
            OscType::Int(frames),
        ];
        self.notify_but(from, "/buffer_touched", payload);
        Ok(())
    }

    /// **Tells every picture but the writer's that a span went stale**
    /// (`/buffer_touched bufnum channel start frames`).
    ///
    /// The span and not the samples: whoever cares re-reads it. `channel` is
    /// `-1` because a write that arrives as samples has already been resolved
    /// to frames by the time it lands -- a reader re-reads every channel of
    /// the span, which is what it would have done for a strided write anyway.
    ///
    /// The writer is left out: it knows, and telling it would make an editor
    /// re-read what it just sent.
    pub(in crate::osc::server) fn announce_write(
        &mut self,
        index: i32,
        start: usize,
        frames: usize,
        from: ClientId,
    ) {
        let (Ok(start), Ok(frames)) = (i32::try_from(start), i32::try_from(frames)) else {
            return;
        };
        if frames <= 0 {
            return;
        }
        let payload = vec![
            OscType::Int(index),
            OscType::Int(-1),
            OscType::Int(start),
            OscType::Int(frames),
        ];
        self.notify_but(from, "/buffer_touched", payload);
    }

    /// `/buffer_close bufnum`: closes a soundfile a streaming buffer left open
    /// (scsynth pairs this with `DiskIn`/`DiskOut`). Clausters has no streaming
    /// buffers yet -- every `/buffer_read`/`/buffer_write` reads or writes the whole file
    /// and closes it -- so there is never an open handle: this validates the
    /// buffer is live and acknowledges, forward-compatible with the future
    /// streaming UGens.
    pub(in crate::osc::server) fn handle_buffer_close(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let index = args.index()?;
        allocated(&self.translator.buffers, index as i32)?;
        self.done_with(from, "/buffer_close", vec![OscType::Int(index as i32)]);
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
    ) -> Answer {
        let (index, job) = parse_buffer_msg(
            cmd,
            &msg.args,
            &self.translator.buffers,
            // What a buffer allocated here is sampled at: the engine's rate.
            self.info.actual_sample_rate,
        )?;
        self.submit_nrt(cmd, index, from, job);
        Ok(())
    }

    /// `/buffer_gen bufnum cmd ...`: fills a buffer through the wavetable/generator
    /// path (see [`parse_buffer_gen`]). Async on the NRT queue, in submission order
    /// with the other `/buffer_*` commands, replying `/done`/`/fail` like them.
    pub(in crate::osc::server) fn handle_buffer_gen(
        &mut self,
        args: Args,
        from: ClientId,
    ) -> Answer {
        let (index, job) = parse_buffer_gen(
            args.rest(),
            &self.translator.buffers,
            // The rate the engine runs at, which a partition's span of
            // samples is measured in -- not the one that was asked for.
            self.info.actual_sample_rate,
        )?;
        self.submit_nrt("/buffer_gen", index, from, job);
        Ok(())
    }

    /// `/buffer_query bufnum...` -> `/buffer_query.reply` with (bufnum, frames, channels,
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
            let info = mirror_buffer(&self.translator.buffers, index);
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

    /// `/buffer_get bufnum index...` -> `/buffer_get.reply bufnum index value...`: read single
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
        let buffer = mirror_buffer(&self.translator.buffers, bufnum);
        if let Some(b) = buffer.as_deref() {
            readable(b, bufnum)?;
        }
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

    /// `/buffer_getRange bufnum [start count]...` -> `/buffer_getRange.reply bufnum [start blob]...`:
    /// read ranges of samples (flat, interleaved) from the buffer mirror -- how a
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
        let buffer = mirror_buffer(&self.translator.buffers, bufnum);
        if let Some(b) = buffer.as_deref() {
            readable(b, bufnum)?;
        }
        let data = buffer.as_deref();
        let held = data.map_or(0, |b| b.len());
        let mut out = vec![OscType::Int(bufnum)];
        while !args.is_empty() {
            let start = args.int()?.max(0) as usize;
            let count = args.int()?.max(0) as usize;
            let end = start.saturating_add(count).min(held);
            let mut blob = Vec::with_capacity(end.saturating_sub(start) * 4);
            for i in start..end {
                push_sample(&mut blob, data.map_or(0.0, |b| b.at(i)));
            }
            out.push(OscType::Int(start as i32));
            out.push(OscType::Blob(blob));
        }
        self.reply(from, "/buffer_getRange.reply", out);
        Ok(())
    }

    /// `/buffer_export bufnum path` -> `/done /buffer_export bufnum`: write the buffer's raw
    /// samples (flat, interleaved, little-endian `f32`) to `path` as a **local
    /// shared resource**, so a same-machine client (the GUI host) can map and read
    /// a multi-megabyte buffer with no per-sample OSC traffic -- the bulk-data path,
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
        let buffer = allocated(&self.translator.buffers, bufnum)?;
        readable(&buffer, bufnum)?;
        // One snapshot, then the encode: an export is a reading of the buffer
        // at a moment, and taking it in one pass keeps it from straddling a
        // recording UGen's write head more than it has to.
        let bytes = sample_blob(&buffer.to_vec());
        std::fs::write(path, &bytes).map_err(|e| format!("write {path}: {e}"))?;
        self.done_with(from, "/buffer_export", vec![OscType::Int(bufnum)]);
        Ok(())
    }
}
