//! **What the audio server answers about buffers**, read once for both fronts.
//!
//! A take reaches a picture over the wire in five replies -- its shape
//! (`/buffer_query.reply`), a run of its samples (`/buffer_getRange.reply`), a
//! span another peer wrote (`/buffer_touched`), the overview of a recording
//! (`/buffer_stream.reply`) and the overview of a take standing still
//! (`/buffer_peaks.reply`) -- and what each one does to the widget trees is the
//! same in a window and in a page. It was written twice, once per front, and
//! the two copies had already drifted apart: a rate one of them could not read,
//! a summary one of them put into a slot the element had not claimed.
//!
//! What a front differs in is small, and it is the [`Front`] trait: where a
//! widget's GPU slot lives and how it lets go of its samples, how a window is
//! asked to draw again, and how a message leaves for the server. Everything
//! else is the provided methods here, so a fix to one reply is a fix in both
//! builds.
//!
//! What a placement says is `info`, which a release build keeps: which route a
//! take took -- downloaded, drawn from its summary, a span read back -- is what
//! a page's own tests read to know the picture came the right way.

use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscType};

use super::fetch::{BufferFetches, FetchStep, SpanUse, WaveWant, align_span};
use super::frame::{self, Owed};
use super::instance::Leg;
use super::widget::element::{Bulk, Loaded, SlotKind};
use super::{Host, diag};
use crate::waveform::WaveformData;

/// **What a front supplies** so the replies can be read once: the host, the
/// fetch machine, its windows' GPU slots, its redraw and its way out.
pub(crate) trait Front {
    /// The host whose trees the replies land in.
    fn host(&self) -> &Host;
    /// [`Front::host`], mutably.
    fn host_mut(&mut self) -> &mut Host;
    /// The fetch machine the downloads walk through.
    fn fetches(&mut self) -> &mut BufferFetches;
    /// Sends one message to the server that holds the samples.
    fn to_server(&self, msg: OscMessage);
    /// Asks window `def_id` for another frame.
    fn redraw_window(&self, def_id: i32);
    /// Whether window `def_id` is open in this front -- a reply for one that
    /// closed while it was in flight is dropped.
    fn window_open(&self, def_id: i32) -> bool;
    /// Puts `data` into the GPU slot widget `widget_id` of window `def_id`
    /// claimed, answering the extent it loaded ([`frame::place_in_slot`]).
    fn place_slot(&mut self, def_id: i32, widget_id: i32, data: Loaded) -> Option<usize>;
    /// Lets go of the samples the geometry slot of widget `widget_id` holds,
    /// so the element is their sole owner and rewrites them in place rather
    /// than copying the whole take first.
    fn release_slot(&mut self, def_id: i32, widget_id: i32);
    /// The spans the last frame of window `def_id` could not draw, by widget,
    /// taken so each is asked for once.
    fn take_owed(&self, def_id: i32) -> Vec<(i32, Owed)>;

    /// **Reads one reply from the server**: the ids and the multitrack's
    /// waiting steps hear it first, a join whose samples are there is asked
    /// for again, and a reply about a buffer lands in the trees. Answers
    /// whether it was one of the buffer replies, so a front reads the rest.
    fn on_server_reply(&mut self, from: Leg, msg: &OscMessage) -> bool {
        self.host_mut().on_server_reply(from, msg);
        for def_id in self.host_mut().forget_stitched(msg) {
            self.redraw_window(def_id);
        }
        match msg.addr.as_str() {
            "/buffer_query.reply" => {
                // (bufnum, frames, channels, sampleRate) per buffer.
                for group in msg.args.chunks(4) {
                    if let [
                        OscType::Int(bufnum),
                        OscType::Int(frames),
                        OscType::Int(channels),
                        rate,
                    ] = group
                    {
                        // **-1 is absence, not emptiness**: the buffer is not
                        // there yet, and is asked for again rather than drawn
                        // as a take of no frames (`BufferFetches::on_absent`).
                        if *frames < 0 {
                            self.fetches().on_absent(*bufnum);
                            continue;
                        }
                        let step = self.fetches().on_info(
                            *bufnum,
                            *frames as usize,
                            (*channels).max(0) as usize,
                            number(rate),
                        );
                        self.apply_fetch_step(step);
                    }
                }
            }
            "/buffer_getRange.reply" => {
                let step = self.fetches().on_data(&msg.args);
                self.apply_fetch_step(step);
            }
            // **Somebody else wrote these samples.** A peer editing a shared
            // buffer stores into the cells and announces the span; the server
            // broadcasts it to everyone but the writer. A picture reading the
            // mapping is already the new one and only its summary is stale; one
            // holding its own copy -- a remote server, a page -- reads the span
            // back.
            "/buffer_touched" => {
                if let [
                    OscType::Int(bufnum),
                    OscType::Int(channel),
                    OscType::Int(start),
                    OscType::Int(frames),
                ] = msg.args.as_slice()
                    && self.resummarize(*bufnum, *channel, *start, *frames) == 0
                {
                    self.read_span_back(*bufnum, *start, *frames);
                }
            }
            // **A recording this host cannot read, reported by the server.**
            // The overview of the frames that appeared, for a view holding its
            // own copy of the samples -- the wire's answer to the frontier a
            // mapping reads for free.
            "/buffer_stream.reply" => {
                if let Some((bufnum, start, bucket, stats)) = super::stream_report(&msg.args) {
                    self.on_stream_report(bufnum, start, bucket, &stats);
                }
            }
            // **The overview of a take that is standing still**, asked for
            // rather than pushed. Identical payload, so it folds through the
            // same door -- and then the walk continues, because one reply
            // carries only so many buckets.
            "/buffer_peaks.reply" => {
                if let Some((bufnum, start, bucket, stats)) = super::stream_report(&msg.args) {
                    // **Which of the two summaries this is, told by its
                    // bucket**: a detail grid is asked for finer than the view
                    // it is for, and the walk asks at the view's own -- so a
                    // reply some view asked for at that bucket and start is
                    // that view's, and everything else is the walk's.
                    if let Some((def_id, widget_id)) =
                        self.fetches().detail_reply(bufnum, start, bucket)
                    {
                        self.place_detail(bufnum, def_id, widget_id, start, bucket, &stats);
                    } else {
                        self.on_stream_report(bufnum, start, bucket, &stats);
                        if let Some(next) = self.fetches().on_peaks(bufnum, start, stats.len()) {
                            self.to_server(next);
                        }
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// Carries out one fetch-machine step: send the next request, or place
    /// what arrived into every view that was waiting on it.
    fn apply_fetch_step(&mut self, step: FetchStep) {
        match step {
            FetchStep::Request(msg) => self.to_server(msg),
            FetchStep::Done {
                bufnum,
                samples,
                channels,
                sample_rate,
                wants,
            } => self.place_samples(bufnum, samples, channels, sample_rate, wants),
            FetchStep::Empty {
                bufnum,
                frames,
                channels,
                sample_rate,
                ask_summary,
                wants,
            } => self.place_summary(bufnum, frames, channels, sample_rate, ask_summary, wants),
            FetchStep::Window {
                bufnum,
                want,
                start_frame,
                channels,
                samples,
            } => self.place_window(bufnum, want, start_frame, channels, &samples),
            FetchStep::Patch {
                bufnum,
                start_frame,
                channels,
                samples,
            } => self.place_patch(bufnum, start_frame, channels, &samples),
            FetchStep::None => {}
        }
    }

    /// **A whole take downloaded**, placed in every view that asked for it.
    ///
    /// The fetch was keyed by a widget id, and for a clip that is the
    /// *clip's* -- a body carries none -- so the reply resolves to the element
    /// that wanted the samples rather than to the container. What is read out
    /// is the **declaration**, never the widget: a slot says where the data
    /// goes, and its parameters say what has to be made of the samples before
    /// a pipeline can take them.
    fn place_samples(
        &mut self,
        bufnum: i32,
        samples: Arc<[f32]>,
        channels: usize,
        sample_rate: f64,
        wants: Vec<WaveWant>,
    ) {
        let channels = channels.max(1);
        diag::info!(
            "buffer {bufnum}: {} frames x {channels} channel(s) loaded into {} view(s)",
            samples.len() / channels,
            wants.len()
        );
        for want in wants {
            if !self.window_open(want.def_id) {
                continue;
            }
            let Some(slot) = self
                .host()
                .window_def(want.def_id)
                .and_then(|t| t.find(want.widget_id))
                .map(|w| w.bulk_target().kind.needs().slot)
            else {
                continue;
            };
            match slot {
                Some(SlotKind::Geometry { base_bucket }) => {
                    let data = Arc::new(WaveformData::from_interleaved(
                        &samples,
                        channels,
                        base_bucket,
                    ));
                    // ...and the element keeps the same pyramid, so a copy over
                    // a fetched buffer reads the samples it is drawing.
                    self.keep(want, &Loaded::Peaks(data.clone()));
                    self.place_slot(want.def_id, want.widget_id, Loaded::Peaks(data));
                }
                Some(SlotKind::Texture {
                    window_size,
                    hop,
                    sample_rate: declared,
                }) => {
                    let rate = if declared > 0.0 {
                        declared
                    } else {
                        sample_rate
                    };
                    let stfts = frame::stft_channels(
                        frame::deinterleave(&samples, channels),
                        window_size,
                        hop,
                        rate,
                    );
                    self.place_slot(want.def_id, want.widget_id, Loaded::Stfts(stfts));
                }
                // Mesh-drawn (a clip's take, a plot): the samples go home to
                // the element, which makes of them whatever it draws from. A
                // clip addressed the fetch for its body, so the door looks one
                // level in for itself -- and the buffer number goes with it,
                // for the element that asked for several and has to put each
                // where it belongs. No navigation group and no ruler rate: a
                // lane owns those.
                _ => {
                    if let Some(w) = self
                        .host_mut()
                        .window_def_mut(want.def_id)
                        .and_then(|t| t.find_mut(want.widget_id))
                    {
                        let raw = || Loaded::Raw {
                            samples: samples.to_vec(),
                            channels,
                        };
                        w.take_bulk_of(bufnum, raw);
                    }
                    self.host_mut().sync_buffer_streams();
                    self.redraw_window(want.def_id);
                    continue;
                }
            }
            self.finish_placement(want, samples.len() / channels, sample_rate);
            self.redraw_window(want.def_id);
        }
    }

    /// A buffer answered with its **shape**: every waiting view gets an empty
    /// summary of that length, and the summary is filled rather than the
    /// samples downloaded.
    ///
    /// The picture is the whole of the box the take fills -- so the axis does
    /// not move while it fills -- and what fills it comes from one of two
    /// places, which is what `ask_summary` says: a take being **written** has
    /// its overview pushed as it appears (`/buffer_stream`), and one standing
    /// still is asked for it (`/buffer_peaks`). Either way the samples stay
    /// where they are, and the run under the eye is read back when a zoom goes
    /// past what the summary can answer.
    fn place_summary(
        &mut self,
        bufnum: i32,
        frames: usize,
        channels: usize,
        sample_rate: f64,
        ask_summary: bool,
        wants: Vec<WaveWant>,
    ) {
        let channels = channels.max(1);
        diag::info!(
            "buffer {bufnum}: drawn from its summary ({frames} frames x {channels} channel(s)); \
             {} view(s), {}",
            wants.len(),
            if ask_summary { "asked for" } else { "streamed" }
        );
        let mut bucket = None;
        for want in wants {
            let Some(base_bucket) = self.summary_bucket_of(want.def_id, want.widget_id) else {
                continue;
            };
            let data = Arc::new(WaveformData::with_multi_pyramid(
                clausters_core::peaks::MultiPyramid::empty(frames, channels, base_bucket),
            ));
            // Only a view that claimed a geometry slot draws the summary from
            // one; every other keeps it on the element.
            let geometry = matches!(
                self.host()
                    .window_def(want.def_id)
                    .and_then(|t| t.find(want.widget_id))
                    .and_then(|w| w.bulk_target().kind.needs().slot),
                Some(SlotKind::Geometry { .. })
            );
            if self.window_open(want.def_id) {
                if geometry {
                    self.place_slot(want.def_id, want.widget_id, Loaded::Peaks(data.clone()));
                }
                self.redraw_window(want.def_id);
            }
            self.keep(want, &Loaded::Peaks(data));
            self.finish_placement(want, frames, sample_rate);
            bucket = bucket.or(Some(base_bucket));
        }
        // **And then the summary itself**, when it is not being pushed: one
        // walk per buffer, however many views drew the empty picture, at the
        // bucket their pyramids are built on.
        if let (true, Some(bucket)) = (ask_summary, bucket)
            && let Some(msg) = self.fetches().want_peaks(bufnum, bucket, channels, frames)
        {
            self.to_server(msg);
        }
    }

    /// **The span a view had zoomed past its summary into**, landed: the
    /// samples go under that view's overview as a window, and it draws them.
    ///
    /// The slot lets the samples go first, as every other write here does --
    /// the element is then the sole owner and the window costs the run rather
    /// than a copy of the summary.
    fn place_window(
        &mut self,
        bufnum: i32,
        want: WaveWant,
        start_frame: usize,
        channels: usize,
        samples: &[f32],
    ) {
        diag::info!(
            "buffer {bufnum}: {} frame(s) at {start_frame} read back for widget {}",
            samples.len() / channels.max(1),
            want.widget_id
        );
        self.release_slot(want.def_id, want.widget_id);
        let took = self
            .host_mut()
            .window_def_mut(want.def_id)
            .and_then(|t| t.find_mut(want.widget_id))
            .is_some_and(|w| {
                w.bulk_target_mut()
                    .kind
                    .as_samples_mut()
                    .is_some_and(|s| s.set_window(start_frame as u64, channels, samples))
            });
        if took {
            self.redraw_window(want.def_id);
        }
    }

    /// **A span another peer wrote**, read back, put into every view of that
    /// buffer -- not only into whoever asked: the samples are the buffer's
    /// own, so any picture of it is entitled to them. Then whatever else was
    /// announced while this was in flight is asked for.
    fn place_patch(&mut self, bufnum: i32, start_frame: usize, channels: usize, samples: &[f32]) {
        diag::info!(
            "buffer {bufnum}: {} frame(s) at {start_frame} read back after an edit",
            samples.len() / channels.max(1),
        );
        let mut redraw = Vec::new();
        for def_id in self.host().window_def_ids() {
            // A pyramid a slot is holding cannot be written in place, so the
            // samples go first -- the same order a streamed report takes, and
            // for the same reason. **Only the slots drawing this buffer**: a
            // slot released and not refilled draws nothing, so releasing every
            // one of them blanks every other take in the window.
            for widget_id in self.widgets_drawing(def_id, bufnum) {
                self.release_slot(def_id, widget_id);
            }
            let Some(tree) = self.host_mut().window_def_mut(def_id) else {
                continue;
            };
            if super::patch_buffer_views(tree, bufnum, start_frame as u64, channels, samples) > 0 {
                redraw.push(def_id);
            }
        }
        for def_id in redraw {
            self.redraw_window(def_id);
        }
        if let Some(msg) = self.fetches().queued_span(bufnum) {
            self.to_server(msg);
        }
    }

    /// **Puts a finer grid under one view**, the summary counterpart of
    /// [`Front::place_window`]: the same `/buffer_peaks` blob every other
    /// overview arrives in, folded beside the view's own summary rather than
    /// into it, because it is measured at a different bucket.
    fn place_detail(
        &mut self,
        bufnum: i32,
        def_id: i32,
        widget_id: i32,
        start: u64,
        bucket: usize,
        stats: &[f32],
    ) {
        diag::info!(
            "buffer {bufnum}: detail of {} bucket(s) of {bucket} at {start} for widget {widget_id}",
            stats.len() / 3
        );
        self.release_slot(def_id, widget_id);
        let took = self
            .host_mut()
            .window_def_mut(def_id)
            .and_then(|t| t.find_mut(widget_id))
            .is_some_and(|w| {
                w.bulk_target_mut()
                    .kind
                    .as_samples_mut()
                    .is_some_and(|s| s.set_detail(start, bucket, stats))
            });
        if took {
            self.redraw_window(def_id);
        }
    }

    /// Folds one `/buffer_stream.reply` into every view of that buffer and
    /// redraws the windows that took it.
    ///
    /// The slots let the samples go first, for the reason the mapped path
    /// gives: a pyramid a slot is holding cannot be written in place, so the
    /// element would copy the whole take before patching the buckets that
    /// arrived. Released, the element is the sole owner and the write costs
    /// the report.
    fn on_stream_report(&mut self, bufnum: i32, start: u64, bucket: usize, stats: &[f32]) {
        let mut redraw = Vec::new();
        for def_id in self.host().window_def_ids() {
            for widget_id in self.widgets_drawing(def_id, bufnum) {
                self.release_slot(def_id, widget_id);
            }
            let Some(tree) = self.host_mut().window_def_mut(def_id) else {
                continue;
            };
            if super::stream_buffer_views(tree, bufnum, start, bucket, stats) > 0 {
                redraw.push(def_id);
            }
        }
        for def_id in redraw {
            self.redraw_window(def_id);
        }
    }

    /// Re-summarizes the span another writer announced, in every view of that
    /// buffer, and redraws the windows that hold one. Answers how many did.
    ///
    /// **Only a view that reads the samples can follow this**, and that is
    /// the honest half: its samples are the ones that changed, so the summary
    /// is all that is stale. A view holding its own copy (a fetched buffer, a
    /// page) has to read the span back ([`Front::read_span_back`]).
    fn resummarize(&mut self, bufnum: i32, channel: i32, start: i32, frames: i32) -> usize {
        // A channel of -1 is every channel of the span: what a write that
        // arrived as samples announces, since it was resolved to frames before
        // it landed. A peer that mapped the buffer names the channel it stored
        // into, and that one is refreshed alone.
        let channel = usize::try_from(channel).ok();
        let (Ok(start), Ok(frames)) = (u64::try_from(start), usize::try_from(frames)) else {
            return 0;
        };
        let mut touched = Vec::new();
        for def_id in self.host().window_def_ids() {
            let Some(tree) = self.host_mut().window_def_mut(def_id) else {
                continue;
            };
            if super::refresh_buffer_views(tree, bufnum, channel, start, frames) > 0 {
                touched.push(def_id);
            }
        }
        for def_id in &touched {
            self.redraw_window(*def_id);
        }
        touched.len()
    }

    /// **Reads an announced span back off the wire**, for a host whose views
    /// hold their own copy of the samples.
    ///
    /// The other half of [`Front::resummarize`], and the one a mapped host
    /// never reaches: with no segment to open -- a remote server, a page --
    /// the samples this host draws are a download, so an edit somebody else
    /// made is not in them and no summary over them can find it. What the
    /// announcement gives is where to look, and this asks for exactly that
    /// span, widened to the summary's buckets so what comes back can replace
    /// what the summary says over it.
    fn read_span_back(&mut self, bufnum: i32, start: i32, frames: i32) {
        let (Ok(start), Ok(frames)) = (usize::try_from(start), usize::try_from(frames)) else {
            return;
        };
        let host = self.host();
        let Some((channels, bucket)) = host.window_def_ids().into_iter().find_map(|def_id| {
            host.window_def(def_id)
                .and_then(|tree| super::span_to_read_back(tree, bufnum))
        }) else {
            // **The announcement is the second ask.** Nothing here has a
            // picture of this buffer with a shape to put a span into -- which is
            // what a join looks like a moment after the edit that minted it:
            // its box named the buffer in the turn the stitch was sent, the
            // first ask answered with no frames at all, and a take remembered
            // as asked is never asked again. The write the server has just
            // announced is what says the samples are there now.
            let asked = self.host_mut().forget_take(bufnum);
            if asked.is_empty() {
                return diag::info!(
                    "buffer {bufnum} was edited by another peer; nothing here draws it"
                );
            }
            diag::info!("buffer {bufnum} was written by another peer; the views of it ask again");
            for def_id in asked {
                self.redraw_window(def_id);
            }
            return;
        };
        let (start, frames) = align_span(start, frames, bucket);
        if let Some(msg) = self
            .fetches()
            .want_span(bufnum, start, frames, channels, SpanUse::Patch)
        {
            diag::debug!(
                "buffer {bufnum}: reading {frames} frame(s) at {start} back after an edit"
            );
            self.to_server(msg);
        }
    }

    /// **Asks for the spans the last frame could not draw** in windows
    /// `defs`. A view zoomed finer than its summary leaves the span it was
    /// asked for on its slot; this is where that note becomes a
    /// `/buffer_getRange` (or a finer `/buffer_peaks`).
    ///
    /// Called after drawing, once per pass: the note is this frame's, and a
    /// span already in flight is not asked for again (the fetch machine keeps
    /// one download per buffer, which is what bounds this). The summary walks
    /// are ticked here too, for the same reason and against the same clock: a
    /// summary that never came back is asked for again rather than leaving a
    /// hole in the picture.
    fn ask_owed_spans(&mut self, defs: &[i32]) {
        for msg in self.fetches().tick() {
            self.to_server(msg);
        }
        let mut asked: Vec<(i32, i32, i32, usize, Owed)> = Vec::new();
        for &def_id in defs {
            for (widget_id, owed) in self.take_owed(def_id) {
                let Some(el) = self
                    .host()
                    .window_def(def_id)
                    .and_then(|t| t.find(widget_id))
                    .and_then(|w| w.bulk_target().kind.as_samples())
                else {
                    continue;
                };
                let (Some((channels, _)), Some(bufnum)) = (el.sample_shape(), el.source_buffer())
                else {
                    continue;
                };
                asked.push((def_id, widget_id, bufnum, channels, owed));
            }
        }
        for (def_id, widget_id, bufnum, channels, owed) in asked {
            let msg = match owed {
                Owed::Summary { a, b, bucket } => {
                    self.fetches()
                        .want_detail(bufnum, def_id, widget_id, a, b - a, bucket)
                }
                Owed::Samples { a, b } => self.fetches().want_span(
                    bufnum,
                    a,
                    b - a,
                    channels,
                    SpanUse::Window { def_id, widget_id },
                ),
            };
            if let Some(msg) = msg {
                self.to_server(msg);
            }
        }
    }

    /// The widgets of `def_id` drawing server buffer `bufnum` -- whose GPU
    /// slots a write to that buffer has to release before the element rewrites
    /// the pyramid they share.
    fn widgets_drawing(&self, def_id: i32, bufnum: i32) -> Vec<i32> {
        self.host()
            .window_def(def_id)
            .map(|tree| {
                tree.descendants()
                    .filter(|w| {
                        w.kind
                            .as_samples()
                            .and_then(|s| s.source_buffer())
                            .is_some_and(|b| b == bufnum)
                    })
                    .filter_map(|w| w.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// **The bucket a view's summary is built at**, which is what a request for
    /// one has to be phrased in. The element declares it with the resource it
    /// wants; a view that declared none takes the default every signal element
    /// summarizes at.
    fn summary_bucket_of(&self, def_id: i32, widget_id: i32) -> Option<usize> {
        let needs = self
            .host()
            .window_def(def_id)
            .and_then(|t| t.find(widget_id))
            .map(|w| w.bulk_target().kind.needs())?;
        Some(match needs.bulk {
            Some(Bulk::Recording { base_bucket, .. }) => base_bucket,
            _ => match needs.slot {
                Some(SlotKind::Geometry { base_bucket }) => base_bucket,
                _ => crate::host::elements::signal::DEFAULT_BASE_BUCKET,
            },
        })
    }

    /// Gives the element that wanted the samples the pyramid its slot draws,
    /// so a copy over a fetched buffer reads the samples it is drawing.
    fn keep(&mut self, want: WaveWant, data: &Loaded) {
        if let Some(w) = self
            .host_mut()
            .window_def_mut(want.def_id)
            .and_then(|t| t.find_mut(want.widget_id))
        {
            frame::keep_data(w, data);
        }
    }

    /// What a placed buffer leaves behind whichever way it arrived: its extent
    /// joins the widget's navigation group, and a widget that knew no sample
    /// rate takes the samples' so its ruler can label real time.
    fn finish_placement(&mut self, want: WaveWant, frames: usize, sample_rate: f64) {
        let host = self.host_mut();
        host.set_timeline_total(want.widget_id, frames);
        // The samples just arrived, so whether this view can follow its own
        // recording is only answerable now: a mapped body reads the frontier,
        // an owned one has to be told.
        host.sync_buffer_streams();
        if sample_rate > 0.0
            && let Some(w) = host
                .window_def_mut(want.def_id)
                .and_then(|t| t.find_mut(want.widget_id))
            && let Some(editor) = w.kind.editor_mut()
            && editor.sample_rate <= 0.0
        {
            editor.sample_rate = sample_rate;
        }
    }
}

/// A numeric OSC argument as `f64`, whichever width or kind the server wrote
/// it in (0.0 when it is not a number).
fn number(arg: &OscType) -> f64 {
    match arg {
        OscType::Float(x) => *x as f64,
        OscType::Double(x) => *x,
        OscType::Int(n) => *n as f64,
        OscType::Long(n) => *n as f64,
        _ => 0.0,
    }
}
