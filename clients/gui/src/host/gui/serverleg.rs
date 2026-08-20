//! The audio-server client leg, main-thread side: routing the server's replies
//! (`/buffer_query.reply`/`/buffer_getRange.reply` into the buffer-fetch machine, `/group_queryTree.reply`
//! into the node-tree store), the `/server_notify` registration and the node-tree
//! re-query, and placing a finished buffer download into its waiting views.

use std::sync::Arc;
use std::time::Instant;

use clausters_core::osc::{OscMessage, OscPacket, OscType};
use tracing::{debug, info, warn};

#[cfg(feature = "standalone")]
use crate::host::ServerLink;
use crate::host::fetch::{FetchStep, SpanUse, WaveWant, align_span};
use crate::host::frame;
use crate::host::graphics::nodetree::NodeTree;
use crate::host::widget::Widget;
use crate::host::widget::element::{Bulk, Loaded, SlotKind};
use crate::waveform::WaveformData;

use super::app::App;

impl App {
    /// Registers timeline widgets (waveform/spectrogram) that reference a
    /// server buffer and queries the audio server for each distinct buffer's
    /// shape (the fetch proceeds on the `/buffer_query.reply` reply). `refs` is
    /// `(widget_id, bufnum, shape_only)`, the last saying the widget draws a
    /// take being recorded into and wants its length rather than its silence.
    pub(super) fn start_buffer_fetches(&mut self, def_id: i32, refs: Vec<(i32, i32, bool)>) {
        for (widget_id, bufnum, shape_only) in refs {
            // **Mapped samples need no conversation.** When the take is in a
            // region this host can open, its samples are read straight out of
            // it — no `/buffer_query`, no chunked `/buffer_getRange`, no
            // waiting. The fetch machine below stays exactly as it is for
            // everybody else: a remote server, a page, a host with no segment.
            #[cfg(unix)]
            if self.place_mapped_buffer(def_id, widget_id, bufnum) {
                continue;
            }
            debug!("gui_def {def_id}: widget {widget_id} waits on server buffer {bufnum}");
            let query = if shape_only {
                self.fetches.want_shape(def_id, widget_id, bufnum)
            } else {
                self.fetches.want(def_id, widget_id, bufnum)
            };
            if let Some(query) = query {
                self.send_to_server(query);
            }
        }
    }

    /// Places a take out of the mapped samples, returning whether it was
    /// there. The zero-message half of [`Self::start_buffer_fetches`].
    ///
    /// **Nothing is read out.** The picture is built over the mapping itself —
    /// one [`crate::host::mapped::MappedChannel`] per channel, summarized
    /// where it lies — so opening a ten-minute take allocates its pyramid and
    /// no copy of the samples. What the analysis path needs is the one
    /// exception, and it says so where it takes it.
    #[cfg(unix)]
    fn place_mapped_buffer(&mut self, def_id: i32, widget_id: i32, bufnum: i32) -> bool {
        let Ok(index) = usize::try_from(bufnum) else {
            return false;
        };
        let Some(take) = self.host.shared_buffers().and_then(|m| m.map(index)) else {
            return false;
        };
        // **And the summary beside it, when the server wrote one.** Opening a
        // take costs one pass over every sample to build its pyramid; the
        // overview file is that pass already paid, so this reads a few
        // megabytes instead of a few hundred. Absent, the pass happens as it
        // always did.
        let summary = self.host.shared_buffers().and_then(|m| m.overview(index));
        let (channels, _, sample_rate) = take.shape();
        debug!("gui_def {def_id}: widget {widget_id} maps buffer {bufnum}, nothing sent");
        self.place_mapped_buffer_data(
            Arc::new(take),
            channels,
            sample_rate,
            summary,
            vec![WaveWant {
                def_id,
                widget_id,
                shape_only: false,
            }],
        );
        true
    }

    /// [`Self::finalize_buffer`] for samples that is **mapped** rather than
    /// downloaded: the views share one `WaveformData` reading the region, and
    /// the pyramid over it is built once for every want.
    #[cfg(unix)]
    fn place_mapped_buffer_data(
        &mut self,
        take: Arc<crate::host::mapped::MappedBuffer>,
        channels: usize,
        sample_rate: f64,
        summary: Option<clausters_core::peaks::MultiPyramid>,
        wants: Vec<WaveWant>,
    ) {
        let channels = channels.max(1);
        let (_, frames, _) = take.shape();
        let mut shared: Option<Arc<WaveformData>> = None;
        for want in wants {
            let Some(slot) = self
                .host
                .window_def(want.def_id)
                .and_then(|t| t.find(want.widget_id))
                .map(|w| w.bulk_target().kind.needs().slot)
            else {
                continue;
            };
            let Some(ws) = self.windows.get_mut(&want.def_id) else {
                continue;
            };
            // The one form that cannot read the samples where it lies: an
            // analysis consumes every sample by definition, so the transform
            // reads the take once and the picture it makes is its own.
            if let Some(SlotKind::Texture {
                window_size,
                hop,
                sample_rate: declared,
            }) = slot
            {
                let rate = if declared > 0.0 {
                    declared
                } else {
                    sample_rate
                };
                let stfts = frame::stft_lanes(
                    frame::deinterleave(&take.read_all(), channels),
                    window_size,
                    hop,
                    rate,
                );
                if let Some(slot) = frame::spectrogram_slot(stfts, &ws.gpu, &ws.renderers) {
                    ws.spectrograms.insert(want.widget_id, slot);
                }
                ws.gpu.window.request_redraw();
                self.finish_placement(want, frames, sample_rate);
                continue;
            }
            // The bucket is the element's own, as it is on every other path:
            // a navigable trace declares it with its slot, and anything else
            // takes the default the signal element uses.
            let base_bucket = match slot {
                Some(SlotKind::Geometry { base_bucket }) => base_bucket,
                _ => crate::host::elements::signal::DEFAULT_BASE_BUCKET,
            };
            let data = shared
                .get_or_insert_with(|| {
                    let sources =
                        crate::host::mapped::MappedChannel::channels_of(Arc::clone(&take));
                    // The file's own bucket is what it was written at, so a
                    // view asking for another one summarizes rather than
                    // drawing a grid the file does not describe.
                    let read = summary
                        .clone()
                        .filter(|s| s.base_bucket() == base_bucket)
                        .and_then(|s| WaveformData::from_sources_summarized(sources.clone(), s));
                    Arc::new(
                        read.unwrap_or_else(|| WaveformData::from_sources(sources, base_bucket)),
                    )
                })
                .clone();
            if matches!(slot, Some(SlotKind::Geometry { .. })) {
                ws.waveforms
                    .insert(want.widget_id, frame::waveform_slot(data.clone()));
            }
            ws.gpu.window.request_redraw();
            if let Some(w) = self
                .host
                .window_def_mut(want.def_id)
                .and_then(|t| t.find_mut(want.widget_id))
            {
                w.take_bulk(|| Loaded::Shared(data.clone()));
            }
            if matches!(slot, Some(SlotKind::Geometry { .. })) {
                self.finish_placement(want, frames, sample_rate);
            }
        }
    }

    /// Sends one fetch-machine message over the client leg (`/buffer_query`,
    /// `/buffer_getRange`), warning instead of failing when no server is attached.
    fn send_to_server(&self, msg: OscMessage) {
        let Some(server) = self.host.server() else {
            return warn!(
                "waveform references a server buffer but no audio server is attached (--server)"
            );
        };
        let addr = msg.addr.clone();
        if let Err(e) = server.send(msg) {
            warn!("failed to send {addr} to the audio server: {e}");
        }
    }

    /// Carries out one fetch-machine step: send the next request, or place a
    /// finished buffer into every window that was waiting on it.
    fn apply_fetch_step(&mut self, step: FetchStep) {
        match step {
            FetchStep::Request(msg) => self.send_to_server(msg),
            FetchStep::Done {
                bufnum,
                samples,
                channels,
                sample_rate,
                wants,
            } => self.finalize_buffer(bufnum, samples, channels, sample_rate, wants),
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

    /// Pops every pending reply from an embedded server and routes it, the
    /// embed counterpart of the UDP reply thread. Only built with the
    /// `standalone` feature (the only way to get an embed link); otherwise a
    /// no-op (see the stub below).
    #[cfg(feature = "standalone")]
    pub(super) fn drain_embed_replies(&mut self) {
        let mut packets: Vec<Vec<u8>> = Vec::new();
        let mut buf = vec![0u8; 65536];
        // Both in-process links are polled here: the embedded server's ring and
        // the session's. An editor has the second and not the first, and the
        // replies it waits on -- a `/done` per edit, per render -- come back
        // exactly the same way.
        if let Some(embed) = self.host.server().and_then(ServerLink::embed) {
            while let Some(n) = embed.poll_into(&mut buf) {
                packets.push(buf[..n].to_vec());
            }
        }
        if let Some(session) = self.host.server().and_then(ServerLink::session) {
            while let Some(n) = session.poll_into(&mut buf) {
                packets.push(buf[..n].to_vec());
            }
        }
        for bytes in packets {
            match clausters_core::osc::decode_packet(&bytes) {
                Ok(packet) => self.handle_server_packet(packet),
                Err(e) => warn!("malformed OSC reply from the embedded server: {e}"),
            }
        }
    }

    /// Without the `standalone` feature there is no embed link, so draining its
    /// replies is nothing — kept so the event loop calls it unconditionally.
    #[cfg(not(feature = "standalone"))]
    pub(super) fn drain_embed_replies(&mut self) {}

    /// Routes one decoded reply from the audio server (the client leg).
    pub(super) fn handle_server_packet(&mut self, packet: OscPacket) {
        let OscPacket::Message(msg) = packet else {
            return; // bundles are not used on the reply path yet
        };
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
                        let step = self.fetches.on_info(
                            *bufnum,
                            (*frames).max(0) as usize,
                            (*channels).max(0) as usize,
                            float_arg(rate),
                        );
                        self.apply_fetch_step(step);
                    }
                }
            }
            "/buffer_getRange.reply" => {
                let step = self.fetches.on_data(&msg.args);
                self.apply_fetch_step(step);
            }
            // **Somebody else wrote these samples.** A peer editing a shared
            // buffer stores into the cells and announces the span; the server
            // broadcasts it to everyone but the writer. A picture reading the
            // mapping is already the new one — what it needs is to be told
            // which columns to re-summarize.
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
            // own copy of the samples — the wire's answer to the frontier a
            // mapping reads for free.
            "/buffer_stream.reply" => {
                if let Some((bufnum, start, bucket, stats)) = crate::host::stream_report(&msg.args)
                {
                    self.on_stream_report(bufnum, start, bucket, &stats);
                }
            }
            // **The overview of a take that is standing still**, asked for
            // rather than pushed. Identical payload, so it folds through the
            // same door -- and then the walk continues, because one reply
            // carries only so many buckets.
            "/buffer_peaks.reply" => {
                if let Some((bufnum, start, bucket, stats)) = crate::host::stream_report(&msg.args)
                {
                    self.on_stream_report(bufnum, start, bucket, &stats);
                    let shape = self.host.window_def_ids().into_iter().find_map(|id| {
                        self.host
                            .window_def(id)
                            .and_then(|tree| crate::host::buffer_shape_in(tree, bufnum))
                    });
                    if let Some(next) =
                        crate::host::next_peaks_frame(shape, start, bucket, stats.len())
                    {
                        self.ask_peaks(bufnum, next);
                    }
                }
            }
            "/group_queryTree.reply" => self.on_query_tree_reply(&msg.args),
            // A node was created or freed (on any client): refresh the tree
            // promptly instead of waiting for the next poll.
            "/node_start" | "/node_end" => self.next_query = Instant::now(),
            "/fail" => warn!("audio server replied /fail: {:?}", msg.args),
            _ => {}
        }
    }

    /// `/group_queryTree.reply`: parse the server's node tree, store it by group and
    /// repaint the windows showing it (only when it actually changed, so an
    /// idle tree polled at a few Hz does not repaint needlessly).
    fn on_query_tree_reply(&mut self, args: &[OscType]) {
        let Some(tree) = NodeTree::parse(args) else {
            return warn!("malformed /group_queryTree.reply ({} args)", args.len());
        };
        let group = tree.group;
        if self.node_trees.get(&group) == Some(&tree) {
            return;
        }
        debug!(
            "node tree for group {group} updated ({} top-level node(s))",
            tree.root.len()
        );
        self.node_trees.insert(group, tree);
        let ids: Vec<i32> = self
            .windows
            .keys()
            .copied()
            .filter(|id| self.window_shows_group(*id, group))
            .collect();
        for id in ids {
            self.redraw(id);
        }
    }

    /// The distinct server groups any open window's `nodetree` widgets mirror.
    pub(super) fn node_tree_groups(&self) -> Vec<i32> {
        let mut groups = Vec::new();
        for id in self.windows.keys() {
            if let Some(tree) = self.host.window_def(*id) {
                collect_node_tree_groups(tree, &mut groups);
            }
        }
        groups
    }

    /// Whether window `def_id` has a `nodetree` mirroring `group`.
    fn window_shows_group(&self, def_id: i32, group: i32) -> bool {
        let mut groups = Vec::new();
        if let Some(tree) = self.host.window_def(def_id) {
            collect_node_tree_groups(tree, &mut groups);
        }
        groups.contains(&group)
    }

    /// Registers for node lifecycle notifications (`/server_notify 1`) once, so a
    /// `nodetree` refreshes as soon as nodes appear or disappear.
    pub(super) fn ensure_notify(&mut self) {
        if self.notified {
            return;
        }
        if let Some(server) = self.host.server() {
            if let Err(e) = server.send(OscMessage {
                addr: "/server_notify".into(),
                args: vec![OscType::Int(1)],
            }) {
                return warn!("failed to register for node notifications: {e}");
            }
            self.notified = true;
        }
    }

    /// Sends a `/group_queryTree <group> 1` for every group an open `nodetree` shows.
    pub(super) fn requery_node_trees(&self) {
        let Some(server) = self.host.server() else {
            return;
        };
        for group in self.node_tree_groups() {
            if let Err(e) = server.send(OscMessage {
                addr: "/group_queryTree".into(),
                args: vec![OscType::Int(group), OscType::Int(1)],
            }) {
                warn!("failed to query node tree for group {group}: {e}");
            }
        }
    }

    /// A buffer finished downloading (interleaved, every channel kept): look
    /// up each waiting widget and build its view — a multichannel waveform, or
    /// one STFT lane per channel for a spectrogram. The buffer's `/buffer_query.reply`
    /// sample rate also fills a widget's unknown `sample_rate`, so its ruler
    /// and readout label real time.
    fn finalize_buffer(
        &mut self,
        bufnum: i32,
        samples: Arc<[f32]>,
        channels: usize,
        sample_rate: f64,
        wants: Vec<WaveWant>,
    ) {
        let channels = channels.max(1);
        info!(
            "buffer {bufnum}: {} frames x {channels} channel(s) loaded into {} view(s)",
            samples.len() / channels,
            wants.len()
        );
        for want in wants {
            // The fetch was keyed by a widget id, and for a clip that is the
            // *clip's* — a body carries none — so the reply resolves to the
            // element that wanted the samples rather than to the container.
            // What is copied out is the **declaration**, never the widget: a
            // slot says where the data goes, and its parameters say what has to
            // be made of the samples before a pipeline can take them.
            let Some(slot) = self
                .host
                .window_def(want.def_id)
                .and_then(|t| t.find(want.widget_id))
                .map(|w| w.bulk_target().kind.needs().slot)
            else {
                continue;
            };
            let Some(ws) = self.windows.get_mut(&want.def_id) else {
                continue;
            };
            match slot {
                Some(SlotKind::Geometry { base_bucket }) => {
                    let data: Arc<WaveformData> = Arc::new(WaveformData::from_interleaved(
                        &samples,
                        channels,
                        base_bucket,
                    ));
                    ws.waveforms
                        .insert(want.widget_id, frame::waveform_slot(data.clone()));
                    // ...and the element keeps the same pyramid, so a copy over
                    // a fetched buffer reads the samples it is drawing.
                    if let Some(w) = self
                        .host
                        .window_def_mut(want.def_id)
                        .and_then(|t| t.find_mut(want.widget_id))
                    {
                        frame::keep_data(w, &Loaded::Peaks(data));
                    }
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
                    let stfts = frame::stft_lanes(
                        frame::deinterleave(&samples, channels),
                        window_size,
                        hop,
                        rate,
                    );
                    if let Some(slot) = frame::spectrogram_slot(stfts, &ws.gpu, &ws.renderers) {
                        ws.spectrograms.insert(want.widget_id, slot);
                    }
                }
                // Mesh-drawn (a clip's take, a plot): the samples go home to
                // the element, which makes of them whatever it draws from.
                _ => {
                    ws.gpu.window.request_redraw();
                    if let Some(w) = self
                        .host
                        .window_def_mut(want.def_id)
                        .and_then(|t| t.find_mut(want.widget_id))
                    {
                        let raw = || Loaded::Raw {
                            samples: samples.to_vec(),
                            channels,
                        };
                        // A clip addressed the fetch for its body, so the
                        // door looks one level in for itself.
                        w.take_bulk(raw);
                    }
                    continue; // no navigation group, no ruler rate: a lane owns those
                }
            }
            ws.gpu.window.request_redraw();
            self.finish_placement(want, samples.len() / channels, sample_rate);
        }
    }

    /// **The span a view had zoomed past its summary into**, landed: the
    /// samples go under that view's overview as a window, and it draws them.
    ///
    /// The slot lets the samples go first, as every other write here does —
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
        debug!(
            "gui: buffer {bufnum} window of {} frame(s) at {start_frame} for widget {}",
            samples.len() / channels.max(1),
            want.widget_id
        );
        if let Some(slot) = self
            .windows
            .get_mut(&want.def_id)
            .and_then(|ws| ws.waveforms.get_mut(&want.widget_id))
        {
            slot.view.release_data();
        }
        let took = self
            .host
            .window_def_mut(want.def_id)
            .and_then(|t| t.find_mut(want.widget_id))
            .is_some_and(|w| {
                w.bulk_target_mut()
                    .kind
                    .as_element_mut()
                    .is_some_and(|el| el.set_window(start_frame as u64, channels, samples))
            });
        if took && let Some(ws) = self.windows.get(&want.def_id) {
            ws.gpu.window.request_redraw();
        }
    }

    /// **Asks for the spans the last frame could not draw.** A view zoomed
    /// finer than its summary leaves the span it was asked for on its slot;
    /// this is where that note becomes a `/buffer_getRange`.
    ///
    /// Called after drawing, once per pass: the note is this frame's, and a
    /// span already in flight is not asked for again (the fetch machine keeps
    /// one download per buffer, which is what bounds this).
    pub(super) fn fetch_wanted_spans(&mut self) {
        let mut asked: Vec<(i32, i32, i32, usize, usize, usize)> = Vec::new();
        for (def_id, ws) in &self.windows {
            for (widget_id, slot) in &ws.waveforms {
                let Some((a, b)) = slot.wanted_span.take() else {
                    continue;
                };
                let Some((channels, _)) = self
                    .host
                    .window_def(*def_id)
                    .and_then(|t| t.find(*widget_id))
                    .and_then(|w| w.bulk_target().kind.as_element())
                    .and_then(|el| el.sample_shape())
                else {
                    continue;
                };
                let Some(bufnum) = self
                    .host
                    .window_def(*def_id)
                    .and_then(|t| t.find(*widget_id))
                    .and_then(|w| w.bulk_target().kind.as_element())
                    .and_then(|el| el.source_buffer())
                else {
                    continue;
                };
                asked.push((*def_id, *widget_id, bufnum, a, b - a, channels));
            }
        }
        for (def_id, widget_id, bufnum, start, frames, channels) in asked {
            if let Some(msg) = self.fetches.want_span(
                bufnum,
                start,
                frames,
                channels,
                SpanUse::Window { def_id, widget_id },
            ) {
                self.send_to_server(msg);
            }
        }
    }

    /// A buffer answered with its **shape**: every waiting view gets an empty
    /// summary of that length, and the summary is filled rather than the
    /// samples downloaded.
    ///
    /// The picture is the whole of the box the take fills — so the axis does
    /// not move while it fills — and what fills it comes from one of two
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
        debug!("gui: buffer {bufnum} ({frames} frames) is drawn from its summary");
        for want in wants {
            let Some(base_bucket) = self.summary_bucket_of(want.def_id, want.widget_id) else {
                continue;
            };
            let data: Arc<WaveformData> = Arc::new(WaveformData::with_multi_pyramid(
                clausters_core::peaks::MultiPyramid::empty(frames, channels, base_bucket),
            ));
            if let Some(ws) = self.windows.get_mut(&want.def_id) {
                if matches!(
                    self.host
                        .window_def(want.def_id)
                        .and_then(|t| t.find(want.widget_id))
                        .and_then(|w| w.bulk_target().kind.needs().slot),
                    Some(SlotKind::Geometry { .. })
                ) {
                    ws.waveforms
                        .insert(want.widget_id, frame::waveform_slot(data.clone()));
                }
                ws.gpu.window.request_redraw();
            }
            if let Some(w) = self
                .host
                .window_def_mut(want.def_id)
                .and_then(|t| t.find_mut(want.widget_id))
            {
                frame::keep_data(w, &Loaded::Peaks(data));
            }
            self.finish_placement(want, frames, sample_rate);
        }
        if ask_summary {
            self.ask_peaks(bufnum, 0);
        }
    }

    /// **The bucket a view's summary is built at**, which is what a request for
    /// one has to be phrased in. The element declares it with the resource it
    /// wants; a view that declared none takes the default every signal element
    /// summarizes at.
    fn summary_bucket_of(&self, def_id: i32, widget_id: i32) -> Option<usize> {
        let needs = self
            .host
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

    /// **Asks for a buffer's overview from `first_frame` on** (`/buffer_peaks`).
    ///
    /// One request answers at most a few thousand buckets, so a long take takes
    /// several — walked by the replies themselves rather than by a state
    /// machine here: each one says where it ended, and the tree says how long
    /// the take is.
    fn ask_peaks(&mut self, bufnum: i32, first_frame: usize) {
        let Some(bucket) = self.host.window_def_ids().into_iter().find_map(|def_id| {
            self.host
                .window_def(def_id)
                .and_then(|tree| crate::host::summary_bucket_for(tree, bufnum))
        }) else {
            return;
        };
        self.send_to_server(crate::host::fetch::peaks_request(
            bufnum,
            bucket,
            first_frame,
        ));
    }

    /// Re-summarizes the span another writer announced, in every view of that
    /// buffer, and redraws the windows that hold one.
    ///
    /// **Only a view that reads the samples can follow this**, and that is
    /// the honest half: its samples are the ones that changed, so the summary
    /// is all that is stale. A view holding its own copy (a fetched buffer,
    /// a page) would have to fetch the span back, which is the fetch machine's
    /// work and not this one's.
    fn resummarize(&mut self, bufnum: i32, channel: i32, start: i32, frames: i32) -> usize {
        let (Ok(channel), Ok(start), Ok(frames)) = (
            usize::try_from(channel),
            u64::try_from(start),
            usize::try_from(frames),
        ) else {
            return 0;
        };
        let mut touched = Vec::new();
        let open = self.host.window_def_ids();
        for def_id in open {
            let Some(tree) = self.host.window_def_mut(def_id) else {
                continue;
            };
            if crate::host::refresh_buffer_views(tree, bufnum, channel, start, frames) > 0 {
                touched.push(def_id);
            }
        }
        for def_id in &touched {
            if let Some(ws) = self.windows.get(def_id) {
                ws.gpu.window.request_redraw();
            }
        }
        touched.len()
    }

    /// **Reads an announced span back off the wire**, for a host whose views
    /// hold their own copy of the samples.
    ///
    /// The other half of [`Self::resummarize`], and the one a mapped host
    /// never reaches: with no segment to open — a remote server, a page —
    /// the samples this host draws are a download, so an edit somebody else
    /// made is not in them and no summary over them can find it. What the
    /// announcement gives is where to look, and this asks for exactly that
    /// span, widened to the summary's buckets so what comes back can replace
    /// what the summary says over it.
    fn read_span_back(&mut self, bufnum: i32, start: i32, frames: i32) {
        let (Ok(start), Ok(frames)) = (usize::try_from(start), usize::try_from(frames)) else {
            return;
        };
        let Some((channels, bucket)) = self.host.window_def_ids().into_iter().find_map(|def_id| {
            self.host
                .window_def(def_id)
                .and_then(|tree| crate::host::span_to_read_back(tree, bufnum))
        }) else {
            return;
        };
        let (start, frames) = align_span(start, frames, bucket);
        if let Some(msg) = self
            .fetches
            .want_span(bufnum, start, frames, channels, SpanUse::Patch)
        {
            debug!("buffer {bufnum}: reading {frames} frame(s) at {start} back after an edit");
            self.send_to_server(msg);
        }
    }

    /// Puts a span read back after an edit into every view of that buffer, and
    /// redraws the windows that took it. Then asks for whatever else was
    /// announced while this was in flight.
    fn place_patch(&mut self, bufnum: i32, start_frame: usize, channels: usize, samples: &[f32]) {
        let mut redraw = Vec::new();
        for def_id in self.host.window_def_ids() {
            // A pyramid a slot is holding cannot be written in place, so the
            // samples go first — the same order a streamed report takes, and
            // for the same reason.
            if let Some(ws) = self.windows.get_mut(&def_id) {
                for slot in ws.waveforms.values_mut() {
                    slot.view.release_data();
                }
            }
            let Some(tree) = self.host.window_def_mut(def_id) else {
                continue;
            };
            if crate::host::patch_buffer_views(tree, bufnum, start_frame as u64, channels, samples)
                > 0
            {
                redraw.push(def_id);
            }
        }
        for def_id in redraw {
            if let Some(ws) = self.windows.get(&def_id) {
                ws.gpu.window.request_redraw();
            }
        }
        if let Some(msg) = self.fetches.queued_span(bufnum) {
            self.send_to_server(msg);
        }
    }

    /// Folds one `/buffer_stream.reply` into every view of that buffer and
    /// repaints the windows that took it.
    ///
    /// The slots let the samples go first, for the reason the mapped path
    /// gives: a pyramid a slot is holding cannot be written in place, so the
    /// element would copy the whole take before patching the buckets that
    /// arrived. Released, the element is the sole owner and the write costs
    /// the report.
    fn on_stream_report(&mut self, bufnum: i32, start: u64, bucket: usize, stats: &[f32]) {
        let mut redraw = Vec::new();
        for def_id in self.host.window_def_ids() {
            let holding: Vec<i32> = self
                .host
                .window_def(def_id)
                .map(|tree| {
                    tree.descendants()
                        .filter(|w| {
                            w.kind
                                .as_element()
                                .and_then(|el| el.source_buffer())
                                .is_some_and(|b| b == bufnum)
                        })
                        .filter_map(|w| w.id)
                        .collect()
                })
                .unwrap_or_default();
            for widget_id in holding {
                if let Some(slot) = self
                    .windows
                    .get_mut(&def_id)
                    .and_then(|ws| ws.waveforms.get_mut(&widget_id))
                {
                    slot.view.release_data();
                }
            }
            let Some(tree) = self.host.window_def_mut(def_id) else {
                continue;
            };
            if crate::host::stream_buffer_views(tree, bufnum, start, bucket, stats) > 0 {
                redraw.push(def_id);
            }
        }
        for def_id in redraw {
            if let Some(ws) = self.windows.get(&def_id) {
                ws.gpu.window.request_redraw();
            }
        }
    }

    /// Whether this window draws samples that is **still being written** —
    /// a frontier that has moved and has not reached the end of the buffer.
    ///
    /// It is the wake condition for a recording, and it is deliberately narrow
    /// so an ordinary session window still sleeps: a take read from a file has
    /// no frontier at all (nothing wrote it here), and a finished recording
    /// has one that stopped moving at the buffer's end. What is left is a
    /// take being filled right now.
    #[cfg(unix)]
    pub(super) fn window_follows_a_recording(&self, def_id: i32) -> bool {
        let Some(samples) = self.host.shared_buffers() else {
            return false;
        };
        let Some(tree) = self.host.window_def(def_id) else {
            return false;
        };
        tree.descendants().any(|w| {
            let Some(el) = w.kind.as_element() else {
                return false;
            };
            let (Some(bufnum), Some((_, frames))) = (el.source_buffer(), el.sample_shape()) else {
                return false;
            };
            usize::try_from(bufnum)
                .ok()
                .and_then(|index| samples.frontier(index))
                .is_some_and(|frontier| frontier > 0 && frontier < frames)
        })
    }

    /// Off Unix nothing here is mapped, so nothing fills under the window.
    #[cfg(not(unix))]
    pub(super) fn window_follows_a_recording(&self, _def_id: i32) -> bool {
        false
    }

    /// **Follows the recordings**: for every view of mapped samples whose
    /// write frontier has moved, re-summarizes what was added and redraws.
    ///
    /// This is the half of a live picture that a mapping cannot give by
    /// itself. The samples are already the engine's own cells, so a zoomed-in
    /// view is current with nothing done at all — but the *overview* is a
    /// summary of what was there when it was taken, and nothing announces an
    /// engine write (a `RecordBuf` filling a take says nothing on the wire,
    /// correctly). What the writer does publish is how far it has got, and
    /// that is exactly the span to re-read.
    ///
    /// Called on the frame tick. Costs one relaxed load per drawn buffer while
    /// nothing is recording, and the summary of the new frames when something
    /// is — never the take.
    #[cfg(unix)]
    pub(super) fn follow_recordings(&mut self) -> Vec<i32> {
        let Some(samples) = self.host.shared_buffers() else {
            return Vec::new();
        };
        // Read every frontier first: the borrow of the samples ends before
        // the trees are touched, and the answer is a handful of relaxed loads.
        let mut moved: Vec<(i32, i32, u64, u64)> = Vec::new();
        for def_id in self.host.window_def_ids() {
            let Some(tree) = self.host.window_def(def_id) else {
                continue;
            };
            for w in tree.descendants() {
                // A body carries no id of its own, so what is followed is the
                // widget that does — the same addressing every other samples
                // path here uses.
                let (Some(id), Some(el)) = (w.id, w.kind.as_element()) else {
                    continue;
                };
                let Some(bufnum) = el.source_buffer() else {
                    continue;
                };
                let Ok(index) = usize::try_from(bufnum) else {
                    continue;
                };
                let Some(frontier) = samples.frontier(index) else {
                    continue;
                };
                let drawn = self.frontiers.get(&(def_id, id)).copied().unwrap_or(0);
                if frontier > drawn {
                    moved.push((def_id, id, drawn, frontier));
                }
            }
        }
        let mut redraw = Vec::new();
        for (def_id, widget_id, drawn, frontier) in moved {
            self.frontiers.insert((def_id, widget_id), frontier);
            // **The slot gives the samples back before the element writes to
            // it.** A pyramid a slot is holding cannot be written in place —
            // the element would be patching a picture under a renderer that
            // never asked — so the refresh below would have to copy it first,
            // and that copy is the size of the whole take rather than of the
            // block that just arrived. Letting go is a refcount, the write is
            // then the block's own cost, and the slot is refilled before the
            // next draw: a repaint runs `refresh_slots_for` first, and the
            // write leaves the element dirty, which is what a fill answers to.
            if let Some(slot) = self
                .windows
                .get_mut(&def_id)
                .and_then(|ws| ws.waveforms.get_mut(&widget_id))
            {
                slot.view.release_data();
            }
            let Some(tree) = self.host.window_def_mut(def_id) else {
                continue;
            };
            let Some(w) = tree.find_mut(widget_id) else {
                continue;
            };
            let Some(el) = w.kind.as_element_mut() else {
                continue;
            };
            // **Every channel in one refresh.** The frontier is the buffer's,
            // not a channel's, so they all advance together — and a refresh
            // per channel would copy the whole view's summary once per
            // channel, which is the quadratic shape this had first.
            // **How far it is written is a fact the element is told**, beside
            // the summary being refreshed: whether it draws only that far is
            // its own answer to its own props, since a frontier alone cannot
            // tell a take being recorded from a loaded one a single write
            // touched.
            let told = el.set_written(frontier);
            if (el.resummarize(None, drawn, (frontier - drawn) as usize) || told)
                && !redraw.contains(&def_id)
            {
                redraw.push(def_id);
            }
        }
        redraw
    }

    /// Off Unix there is no mapped samples to follow — the picture arrives by
    /// message there, and so does the news that it changed.
    #[cfg(not(unix))]
    pub(super) fn follow_recordings(&mut self) -> Vec<i32> {
        Vec::new()
    }

    /// What a placed buffer leaves behind whichever way it arrived: its extent
    /// joins the widget's navigation group, and a widget that knew no sample
    /// rate takes the samples' so its ruler can label real time.
    fn finish_placement(&mut self, want: WaveWant, frames: usize, sample_rate: f64) {
        self.host.set_timeline_total(want.widget_id, frames);
        // The samples just arrived, so whether this view can follow its own
        // recording is only answerable now: a mapped body reads the frontier,
        // an owned one has to be told.
        self.host.sync_buffer_streams();
        if sample_rate > 0.0
            && let Some(w) = self
                .host
                .window_def_mut(want.def_id)
                .and_then(|t| t.find_mut(want.widget_id))
            && let Some(editor) = w.kind.editor_mut()
            && editor.sample_rate <= 0.0
        {
            editor.sample_rate = sample_rate;
        }
    }
}

/// A numeric OSC argument as `f64` (0.0 when it is neither float nor int).
fn float_arg(arg: &OscType) -> f64 {
    match arg {
        OscType::Float(x) => *x as f64,
        OscType::Double(x) => *x,
        OscType::Int(n) => *n as f64,
        OscType::Long(n) => *n as f64,
        _ => 0.0,
    }
}

/// Whether a widget tree contains a `nodetree` view (so the window drives the
/// node-tree query/notify path).
pub(super) fn tree_has_node_tree(widget: &Widget) -> bool {
    widget
        .descendants()
        .any(|w| !w.kind.needs().node_groups.is_empty())
}

/// Appends the distinct server groups every `nodetree` in `tree` mirrors.
fn collect_node_tree_groups(tree: &Widget, out: &mut Vec<i32>) {
    for group in tree.descendants().flat_map(|w| w.kind.needs().node_groups) {
        if !out.contains(&group) {
            out.push(group);
        }
    }
}
