//! `/buffer_*`: the buffer pool, and the reads that answer immediately.
//!
//! Anything that allocates, reads a file or writes one is submitted to the NRT
//! thread ([`super::super::async_pipes`]) and answers `/done` later; the
//! queries here answer from the mirror in the same turn.

use super::super::*;
use crate::dsp::summary::SUMMARY_BUCKET;
use crate::osc::translate::{allocated, mirror_buffer};
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

    /// `/buffer_render bufnum frames`: run the graph for `frames` frames and
    /// install what came out of the output buses into `bufnum` -- `/buffer_gen`'s
    /// sibling, generating into a buffer by *playing* rather than by formula,
    /// and the operation an editor means by "apply this def to this selection".
    ///
    /// **Only an offline server answers it.** Running the graph means driving
    /// `Engine::process_block`, and in a real-time server the audio device
    /// drives it against a wall clock nobody else may advance; there the
    /// command fails rather than pretending. Offline, the driver owns the clock
    /// and performs the request between commands (`server::nrtsession`), which
    /// is why this only queues one -- see [`OfflineRender`](crate::osc::server::OfflineRender).
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
        allocated(&self.translator.buffers, index as i32)?;
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

    /// `/buffer_parts bufnum` -> `/buffer_parts.reply bufnum channels sampleRate
    /// [srcBufnum srcStart frames fadeIn fadeOut fadeInShape fadeInCurve
    /// fadeOutShape fadeOutCurve chan...]...`: what a join is
    /// made of, in the terms `/buffer_stitch` takes, so a reply can be edited
    /// and sent back.
    ///
    /// **A buffer that owns its samples answers with no parts**, which is the
    /// answer to "is this a join": a client that would draw an editable
    /// waveform asks before it draws one rather than finding out by being
    /// refused when it writes. An unallocated slot answers with `channels = 0`
    /// and no parts, on the rule `/buffer_query` follows -- absence is a state
    /// and not a protocol error.
    ///
    /// Its own reply rather than a field on `/buffer_query.reply`, whose
    /// repeating group is four wide: a fifth field would be read as the next
    /// buffer's index by every parser that chunks it. Synchronous, answered
    /// from the mirror like the rest of the queries.
    pub(in crate::osc::server) fn handle_buffer_parts(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let bufnum = args.int()?;
        let buffer = mirror_buffer(&self.translator.buffers, bufnum);
        let mut out = vec![OscType::Int(bufnum)];
        out.push(OscType::Int(
            buffer.as_ref().map_or(0, |b| b.channels() as i32),
        ));
        out.push(OscType::Float(
            buffer.as_ref().map_or(0.0, |b| b.sample_rate() as f32),
        ));
        if let Some(stitch) = buffer.as_ref().and_then(|b| b.stitch()) {
            for part in stitch.parts() {
                out.push(OscType::Int(part.src_index));
                out.push(OscType::Int(part.src_start as i32));
                out.push(OscType::Int(part.frames as i32));
                out.push(OscType::Int(part.fade_in as i32));
                out.push(OscType::Int(part.fade_out as i32));
                out.push(OscType::Int(part.fade_in_shape.shape));
                out.push(OscType::Float(part.fade_in_shape.curve));
                out.push(OscType::Int(part.fade_out_shape.shape));
                out.push(OscType::Float(part.fade_out_shape.curve));
                for &to in part.map.iter() {
                    out.push(OscType::Int(to));
                }
            }
        }
        self.reply(from, "/buffer_parts.reply", out);
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

    /// `/buffer_peaks bufnum [bucket=256] [start=0] [frames=-1]` ->
    /// `/buffer_peaks.reply bufnum startFrame bucket blob` (one or more):
    /// **the overview of a buffer that is standing still.**
    ///
    /// `/buffer_stream`'s sibling, and the pair is a distinction in the
    /// *material* and not in the client: a recording's overview is pushed as
    /// it is written, and a buffer nothing is writing has one that can simply
    /// be asked for. Same blob either way -- bucket-major, channel-minor, min,
    /// max and mean square as little-endian `f32` -- so the receiving half is
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
    /// `bucket` is any size, and a view asks for the coarsest that still
    /// resolves its pixel columns: one of 256 * 2^k on its own grid is folded
    /// out of the summary's levels with no sample read, and any other is
    /// answered exactly too. `start` is rounded **down** to a whole bucket, so
    /// the grids agree; `frames < 0` runs to the end. A span that runs to the
    /// end answers its last bucket over the frames it holds -- the pyramid's
    /// own short entry -- and one that stops short answers whole buckets only.
    ///
    /// **One request, one reply, and the reply's own length says how much
    /// came** -- the chunk conversation `/buffer_getRange` already has, for the
    /// same reason: one reply is bounded by [`MAX_STREAM_BYTES`], so no message
    /// is bounded by how long the take is, and a client walking a long take
    /// asks again from where the blob ended. Nothing is remembered between
    /// requests.
    ///
    /// Synchronous on the network thread, like `/buffer_get`, `/buffer_getRange`
    /// and `/buffer_export`, and answered from the buffer's kept summary
    /// ([`crate::dsp::summary`]), so what it reads from samples is only what
    /// nobody asked for before, bounded per reply.
    pub(in crate::osc::server) fn handle_buffer_peaks(
        &mut self,
        mut args: Args,
        from: ClientId,
    ) -> Answer {
        let bufnum = args.int()?;
        let bucket = args.opt_int()?.unwrap_or(SUMMARY_BUCKET as i32).max(1) as usize;
        let start = args.opt_int()?.unwrap_or(0).max(0) as usize;
        let asked = args.opt_int()?.unwrap_or(-1);
        let buffer = allocated(&self.translator.buffers, bufnum)?;
        let frames = buffer.frames();
        // Rounded to the grid the answer is folded into: a bucket summarized
        // from part of itself would report a peak the samples do not have.
        let first = (start / bucket) * bucket;
        let end = match asked {
            n if n < 0 => frames,
            n => (start + n as usize).min(frames),
        };
        // A span that runs to the take's end answers its last bucket too, over
        // the frames it holds: that is the pyramid's own ragged entry, and at
        // a coarse bucket it is most of a second nobody could otherwise ask.
        let held = end.saturating_sub(first);
        let buckets = if end == frames {
            held.div_ceil(bucket)
        } else {
            held / bucket
        };
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
        let buckets = buckets.min(max_stream_buckets(buffer.channels()));
        // **Out of the buffer's summary**, which is kept for every buffer this
        // server owns and composed for a join from its sources' -- so a span
        // asked twice is measured once, an edit's new join is answered from
        // what its sources already had, and a coarse bucket reads levels
        // rather than samples. A server that maps somebody else's samples
        // hears of none of the writes into them, so it reads the samples.
        let bytes = if self.keeps_summaries() {
            sample_blob(&summary_overview(&buffer, first, bucket, buckets))
        } else {
            super::super::streams::overview_blob(&buffer, first, bucket, buckets)
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

    /// Whether this server's buffers keep their summaries: every server but one
    /// mapping another's segment, whose samples change under it unannounced.
    pub(in crate::osc::server) fn keeps_summaries(&self) -> bool {
        self.owns_samples || self.shm_path.is_none()
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
        // One snapshot, then the encode: an export is a reading of the buffer
        // at a moment, and taking it in one pass keeps it from straddling a
        // recording UGen's write head more than it has to.
        let bytes = sample_blob(&buffer.to_vec());
        std::fs::write(path, &bytes).map_err(|e| format!("write {path}: {e}"))?;
        self.done_with(from, "/buffer_export", vec![OscType::Int(bufnum)]);
        Ok(())
    }
}

/// **The overview of `buckets` buckets from `first`, out of the buffer's
/// summary**: bucket-major, channel-minor, min, max and mean square, the
/// layout `/buffer_stream` reports in.
///
/// The work is bounded by what it reads from samples, not by the span: a
/// question over what the summary has already measured reads levels, and one
/// over a take nobody has looked at yet measures it here,
/// [`SUMMARY_READ_SAMPLES`] at most per reply. The reply's own length says how far it got, so the asker
/// goes on from there as it does when the bytes run out -- and the opening pass
/// over a long take is spread over replies rather than paid in one turn of an
/// engine that has a block to deliver. At least one bucket always comes back.
fn summary_overview(
    buffer: &crate::dsp::buffer::Buffer,
    first: usize,
    bucket: usize,
    buckets: usize,
) -> Vec<f32> {
    let channels = buffer.channels().max(1);
    let mut out = Vec::with_capacity(buckets * channels * 3);
    let mut read = 0;
    for b in 0..buckets {
        let at = first + b * bucket;
        for ch in 0..channels {
            let stats = buffer.summary_span(ch, at, at + bucket, &mut read);
            out.extend_from_slice(&stats.bucket());
        }
        if read >= SUMMARY_READ_SAMPLES {
            break;
        }
    }
    out
}
