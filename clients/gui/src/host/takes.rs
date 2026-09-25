//! **The takes the pictures draw**: which samples a widget holds, the one
//! walk over every picture of a buffer, what a recording asks the server to
//! stream, and what a write announces to the other peers.

use super::*;

impl Host {
    /// **Writes down how long each loaded take is**, off the mapped buffers'
    /// directory, where the session did not say. Returns how many learned one;
    /// nothing without an owner or a mapping.
    #[cfg(unix)]
    pub fn learn_take_lengths(&mut self) -> usize {
        let Some(buffers) = self.shared_buffers.as_ref() else {
            return 0;
        };
        let Some(owner) = self.owner.as_mut() else {
            return 0;
        };
        owner.learn_lengths(|bufnum| {
            let index = usize::try_from(bufnum).ok()?;
            buffers.frames(index).map(|f| f as u64)
        })
    }

    /// **A buffer's samples were just made** (`/done /buffer_stitch bufnum`):
    /// every element that asked for that take forgets it, so the next walk
    /// asks again. Returns the windows that had one, for a front to redraw.
    ///
    /// A join's box names its buffer in the same turn the stitch is sent, so
    /// the first ask can find it unallocated, or published and not yet copied
    /// into; this is the second ask, made when the samples are there. Any other
    /// message is not this one and touches nothing.
    pub fn forget_stitched(&mut self, msg: &OscMessage) -> Vec<i32> {
        let [OscType::String(command), OscType::Int(bufnum)] = msg.args.as_slice() else {
            return Vec::new();
        };
        if msg.addr != "/done" || command != "/buffer_stitch" {
            return Vec::new();
        }
        self.forget_take(*bufnum)
    }

    /// **Every element that asked for this take forgets it**, so the next walk
    /// asks again; the windows that had one come back, for a front to redraw.
    ///
    /// The `/done` above is one way to learn the samples are there, and it only
    /// reaches whoever *asked* for the stitch. When a **client** owns the multitrack
    /// it is the client that sends it, and the host hears about that buffer
    /// only as the write the server announces to everyone else
    /// (`/buffer_touched`) -- which is the same news under another name, and is
    /// what makes a join drawn by a client's window fill in rather than stay
    /// the empty box the first ask answered with.
    pub fn forget_take(&mut self, bufnum: i32) -> Vec<i32> {
        let mut touched = Vec::new();
        for (def_id, tree) in &mut self.window_defs {
            if samples_views(tree, &mut |el| el.forget_take(bufnum)) > 0 {
                touched.push(*def_id);
            }
        }
        touched
    }

    /// **How many frames of samples a widget draws**, if it draws any.
    ///
    /// What a gesture clamps against: the right edge of a fully zoomed-out view
    /// maps to *one past* the last sample (a window of 8 samples is 8 wide),
    /// and a stroke that reaches it would carry a frame the buffer does not
    /// have -- which the owner refuses, taking the whole stroke with it.
    pub(crate) fn buffer_frames(&self, def_id: i32, widget_id: i32) -> Option<u64> {
        self.samples_of(def_id, widget_id)?
            .sample_shape()
            .map(|(_, frames)| frames)
    }

    /// **The samples a widget draws**, when it draws any -- the element to ask
    /// a take's shape, buffer and rate of, looked up once.
    pub(crate) fn samples_of(
        &self,
        def_id: i32,
        widget_id: i32,
    ) -> Option<&dyn widget::element::Samples> {
        element_with_samples(self.window_def(def_id)?.find(widget_id)?)
    }

    /// The server buffer a widget's samples are in, when they are in one.
    pub(super) fn buffer_of(&self, def_id: i32, widget_id: i32) -> Option<i32> {
        self.samples_of(def_id, widget_id)?.source_buffer()
    }

    /// **Asks to be told about the recordings the pictures cannot read.**
    ///
    /// A take being recorded grows with nothing announcing it: the writer
    /// publishes only how far it has got, into the shared segment. A host that
    /// **maps** that segment reads the number and re-summarizes the frames it
    /// names, and needs nothing from the wire -- but a page maps nothing, and
    /// its samples are its own copy, so there is no frontier to read and
    /// nothing to re-summarize. For it the server sends the *overview* of what
    /// was written instead (`/buffer_stream`, min/max/energy per bucket, about
    /// a hundredth of the audio's bandwidth), which is the same summary the
    /// mapping path derives for itself.
    ///
    /// So the subscription is exactly the views that asked
    /// (`Samples::stream_want`): the client
    /// said the buffer is being written into (`fills`) and the body is this
    /// element's own copy. A mapped view is deliberately not in it -- it would
    /// be paying twice for one picture.
    ///
    /// One subscription covers all of them, because the server keeps one per
    /// client and replaces it on every call; asking per view would mean the
    /// last view to ask silently cancelled the others. Views that want
    /// different buckets cannot be served at once for the same reason: the
    /// first bucket wins and the rest keep the picture they have, which is the
    /// honest half-answer rather than a subscription that flaps between them.
    ///
    /// Called wherever what is drawn can change -- a def, a set of `fills` or
    /// `buffer`, a free, and a placement that gave an element its body.
    pub(crate) fn sync_buffer_streams(&mut self) {
        let mut buffers: Vec<i32> = Vec::new();
        let mut bucket = 0usize;
        for tree in self.window_defs.values() {
            collect_stream_wants(tree, &mut buffers, &mut bucket);
        }
        // The server takes at most 32 buffers in one subscription.
        buffers.truncate(32);
        buffers.sort_unstable();
        if (&buffers, bucket) == (&self.buffer_stream.0, self.buffer_stream.1) {
            return;
        }
        let Some(server) = self.server.as_ref() else {
            return;
        };
        // The cadence is the server's to keep, and it is the same number the
        // mapped path waits for: `--follow-block` if it was set, and the
        // picture's own rate when it was not (the frame is finer than the wire
        // needs, and 20 Hz is already smoother than a take grows).
        let period_ms = if self.follow_block > 0.0 {
            (self.follow_block * 1000.0).round().max(10.0) as i32
        } else {
            50
        };
        let mut args = vec![
            OscType::Int(if buffers.is_empty() { 0 } else { period_ms }),
            OscType::Int(bucket.max(1) as i32),
        ];
        args.extend(buffers.iter().map(|b| OscType::Int(*b)));
        if let Err(e) = server.send(OscMessage {
            addr: "/buffer_stream".into(),
            args,
        }) {
            return diag::warn!("cannot subscribe to the recording stream: {e}");
        }
        diag::debug!(
            "buffer stream: {} buffer(s) at bucket {bucket}",
            buffers.len()
        );
        self.buffer_stream = (buffers, bucket);
    }

    /// **Says what was written, since the samples said nothing.**
    ///
    /// A stroke into mapped cells reaches no wire -- that is what mapping is
    /// for -- so a second client holding a picture of the same take would never
    /// find out. `/buffer_touch` is the span and not the samples: four
    /// integers, which the servers broadcast to their `/server_notify` clients
    /// as `/buffer_touched` for whoever cares to re-read. A page gets it too,
    /// and a page is exactly who needs it: a browser cannot map a file, so a
    /// message is the only way it can hear about an edit at all.
    ///
    /// Both legs are told when they differ, because each server has its own
    /// clients and neither knows the other's.
    #[cfg(unix)]
    pub(super) fn announce_write(&self, bufnum: i32, channel: usize, start: u64, frames: usize) {
        let msg = OscMessage {
            addr: "/buffer_touch".into(),
            args: vec![
                OscType::Int(bufnum),
                OscType::Int(channel as i32),
                OscType::Int(start as i32),
                OscType::Int(frames as i32),
            ],
        };
        for link in [self.server.as_ref(), self.player.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Err(e) = link.send(msg.clone()) {
                diag::warn!("cannot announce the write of buffer {bufnum}: {e}");
            }
        }
    }
}

/// The samples under `widget` -- its own, or those of one of the bodies a
/// container built from its own props (a clip's take carries no id of its own,
/// so it is only ever reached through the widget that does).
///
/// It answers with the **facet** and not with the element, which is what the
/// callers wanted from it all along: every one of them goes on to ask a
/// question about samples.
pub(super) fn element_with_samples(
    widget: &widget::Widget,
) -> Option<&dyn widget::element::Samples> {
    std::iter::once(widget)
        .chain(widget.children.iter())
        .filter_map(|w| w.kind.as_samples())
        .find(|s| s.sample_shape().is_some())
}

/// **Asks every element of this tree that holds samples to do `apply`**,
/// answering how many did.
///
/// The one walk every pass over the pictures of a take goes through: a write,
/// a re-summary, a span read back, a stream report, a take forgotten. Most of
/// them want the pictures of one buffer, which is [`buffer_views`]; the take
/// forgotten asks every element, because one that draws several takes (a
/// multitrack) names none of them as its own.
pub(crate) fn samples_views(
    widget: &mut widget::Widget,
    apply: &mut dyn FnMut(&mut dyn widget::element::Samples) -> bool,
) -> usize {
    let mut did = 0;
    if let Some(el) = widget.kind.as_samples_mut()
        && apply(el)
    {
        did += 1;
    }
    for child in &mut widget.children {
        did += samples_views(child, apply);
    }
    did
}

/// [`samples_views`] over **the elements drawing server buffer `bufnum`**.
///
/// The buffer is the identity: two widgets are two pictures of one buffer
/// exactly when they name the same buffer, and nothing else in the tree relates
/// them -- a clip and an editor of the same take are not parent and child.
pub(crate) fn buffer_views(
    widget: &mut widget::Widget,
    bufnum: i32,
    apply: &mut dyn FnMut(&mut dyn widget::element::Samples) -> bool,
) -> usize {
    samples_views(widget, &mut |el| {
        el.source_buffer() == Some(bufnum) && apply(el)
    })
}

/// **The shape of the request that reads an announced span back**, as
/// `(channels, summary bucket)`, from the first element of this tree drawing
/// `bufnum`.
///
/// One element is enough because the answer serves them all
/// ([`replies::Front::place_patch`]), and no widget id comes back with it for the same
/// reason: what the walk is for is the shape of the request, which only an
/// element knows.
pub(crate) fn span_to_read_back(widget: &widget::Widget, bufnum: i32) -> Option<(usize, usize)> {
    if let Some(el) = widget.kind.as_samples()
        && el.source_buffer() == Some(bufnum)
        && let Some((channels, _)) = el.sample_shape()
        && let Some(bucket) = el.summary_bucket()
    {
        return Some((channels, bucket));
    }
    widget
        .children
        .iter()
        .find_map(|child| span_to_read_back(child, bufnum))
}

/// Reads a `/buffer_stream.reply bufnum startFrame bucket blob` into
/// `(buffer, start_frame, bucket, stats)`, or `None` when it is not one.
///
/// The blob is little-endian `f32`s at whatever offset the packet left them,
/// which is why they are read out rather than viewed in place -- the same
/// reason every other client reads this payload the same way.
pub(crate) fn stream_report(args: &[OscType]) -> Option<(i32, u64, usize, Vec<f32>)> {
    let [
        OscType::Int(bufnum),
        start,
        OscType::Int(bucket),
        OscType::Blob(blob),
    ] = args
    else {
        return None;
    };
    // `startFrame` rides as a **long**: a buffer's sample axis outgrows an
    // `i32` at about twelve hours, and the server says so on the wire. Both
    // spellings are read, because a reader that insisted on `Int` dropped
    // every report and said nothing -- which is exactly what it did.
    let start = match start {
        OscType::Long(frames) => *frames,
        OscType::Int(frames) => *frames as i64,
        _ => return None,
    };
    let stats: Vec<f32> = clausters_core::osc::blob_samples(blob).collect();
    Some((
        *bufnum,
        start.max(0) as u64,
        (*bucket).max(0) as usize,
        stats,
    ))
}

/// Collects the buffers whose recording the elements of this tree want to be
/// told about, and the bucket the first of them named.
pub(super) fn collect_stream_wants(
    widget: &widget::Widget,
    buffers: &mut Vec<i32>,
    bucket: &mut usize,
) {
    if let Some((bufnum, want)) = widget.kind.as_samples().and_then(|s| s.stream_want()) {
        if *bucket == 0 {
            *bucket = want;
        }
        if want == *bucket && !buffers.contains(&bufnum) {
            buffers.push(bufnum);
        }
    }
    for child in &widget.children {
        collect_stream_wants(child, buffers, bucket);
    }
}
