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
use crate::host::fetch::{FetchStep, WaveWant};
use crate::host::frame;
use crate::host::graphics::nodetree::NodeTree;
use crate::host::widget::Widget;
use crate::host::widget::element::{Loaded, SlotKind};
use crate::waveform::WaveformData;

use super::app::App;

impl App {
    /// Registers timeline widgets (waveform/spectrogram) that reference a
    /// server buffer and queries the audio server for each distinct buffer's
    /// shape (the fetch proceeds on the `/buffer_query.reply` reply). `refs` is
    /// `(widget_id, bufnum)`.
    pub(super) fn start_buffer_fetches(&mut self, def_id: i32, refs: Vec<(i32, i32)>) {
        for (widget_id, bufnum) in refs {
            // **Mapped material needs no conversation.** When the take is in a
            // region this host can open, its samples are read straight out of
            // it — no `/buffer_query`, no chunked `/buffer_getRange`, no
            // waiting. The fetch machine below stays exactly as it is for
            // everybody else: a remote server, a page, a host with no segment.
            #[cfg(unix)]
            if self.place_mapped_buffer(def_id, widget_id, bufnum) {
                continue;
            }
            debug!("gui_def {def_id}: widget {widget_id} waits on server buffer {bufnum}");
            if let Some(query) = self.fetches.want(def_id, widget_id, bufnum) {
                self.send_to_server(query);
            }
        }
    }

    /// Places a take out of the mapped material, returning whether it was
    /// there. The zero-message half of [`Self::start_buffer_fetches`].
    ///
    /// **Nothing is read out.** The picture is built over the mapping itself —
    /// one [`crate::host::material::MappedChannel`] per channel, summarized
    /// where it lies — so opening a ten-minute take allocates its pyramid and
    /// no copy of the material. What the analysis path needs is the one
    /// exception, and it says so where it takes it.
    #[cfg(unix)]
    fn place_mapped_buffer(&mut self, def_id: i32, widget_id: i32, bufnum: i32) -> bool {
        let Ok(index) = usize::try_from(bufnum) else {
            return false;
        };
        let Some(take) = self.host.material().and_then(|m| m.map(index)) else {
            return false;
        };
        let (channels, _, sample_rate) = take.shape();
        debug!("gui_def {def_id}: widget {widget_id} maps buffer {bufnum}, nothing sent");
        self.place_mapped_take(
            Arc::new(take),
            channels,
            sample_rate,
            vec![WaveWant { def_id, widget_id }],
        );
        true
    }

    /// [`Self::finalize_buffer`] for material that is **mapped** rather than
    /// downloaded: the views share one `WaveformData` reading the region, and
    /// the pyramid over it is built once for every want.
    #[cfg(unix)]
    fn place_mapped_take(
        &mut self,
        take: Arc<crate::host::material::MappedTake>,
        channels: usize,
        sample_rate: f64,
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
            // The one form that cannot read the material where it lies: an
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
                    Arc::new(WaveformData::from_sources(
                        crate::host::material::MappedChannel::channels_of(Arc::clone(&take)),
                        base_bucket,
                    ))
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
            // **Somebody else wrote this material.** A peer editing a shared
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
                {
                    self.refresh_material(*bufnum, *channel, *start, *frames);
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
                    // a fetched buffer reads the material it is drawing.
                    if let Some(w) = self
                        .host
                        .window_def_mut(want.def_id)
                        .and_then(|t| t.find_mut(want.widget_id))
                    {
                        frame::keep_material(w, &Loaded::Peaks(data));
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

    /// Re-summarizes the span another writer announced, in every view of that
    /// buffer, and redraws the windows that hold one.
    ///
    /// **Only a view that reads the material can follow this**, and that is
    /// the honest half: its samples are the ones that changed, so the summary
    /// is all that is stale. A view holding its own copy (a fetched buffer,
    /// a page) would have to fetch the span back, which is the fetch machine's
    /// work and not this one's.
    fn refresh_material(&mut self, bufnum: i32, channel: i32, start: i32, frames: i32) {
        let (Ok(channel), Ok(start), Ok(frames)) = (
            usize::try_from(channel),
            u64::try_from(start),
            usize::try_from(frames),
        ) else {
            return;
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
        for def_id in touched {
            if let Some(ws) = self.windows.get(&def_id) {
                ws.gpu.window.request_redraw();
            }
        }
    }

    /// Whether this window draws material that is **still being written** —
    /// a frontier that has moved and has not reached the end of the buffer.
    ///
    /// It is the wake condition for a recording, and it is deliberately narrow
    /// so an ordinary session window still sleeps: a take read from a file has
    /// no frontier at all (nothing wrote it here), and a finished recording
    /// has one that stopped moving at the buffer's end. What is left is a
    /// take being filled right now.
    #[cfg(unix)]
    pub(super) fn window_follows_a_recording(&self, def_id: i32) -> bool {
        let Some(material) = self.host.material() else {
            return false;
        };
        let Some(tree) = self.host.window_def(def_id) else {
            return false;
        };
        tree.descendants().any(|w| {
            let Some(el) = w.kind.as_element() else {
                return false;
            };
            let (Some(bufnum), Some((_, frames))) = (el.material_buffer(), el.material_shape())
            else {
                return false;
            };
            usize::try_from(bufnum)
                .ok()
                .and_then(|index| material.frontier(index))
                .is_some_and(|frontier| frontier > 0 && frontier < frames)
        })
    }

    /// Off Unix nothing here is mapped, so nothing fills under the window.
    #[cfg(not(unix))]
    pub(super) fn window_follows_a_recording(&self, _def_id: i32) -> bool {
        false
    }

    /// **Follows the recordings**: for every view of mapped material whose
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
        let Some(material) = self.host.material() else {
            return Vec::new();
        };
        // Read every frontier first: the borrow of the material ends before
        // the trees are touched, and the answer is a handful of loads.
        let mut moved: Vec<(i32, i32, usize, u64, u64)> = Vec::new();
        for def_id in self.host.window_def_ids() {
            let Some(tree) = self.host.window_def(def_id) else {
                continue;
            };
            for w in tree.descendants() {
                // A body carries no id of its own, so what is followed is the
                // widget that does — the same addressing every other material
                // path here uses.
                let (Some(id), Some(el)) = (w.id, w.kind.as_element()) else {
                    continue;
                };
                let (Some(bufnum), Some((channels, _))) =
                    (el.material_buffer(), el.material_shape())
                else {
                    continue;
                };
                let Ok(index) = usize::try_from(bufnum) else {
                    continue;
                };
                let Some(frontier) = material.frontier(index) else {
                    continue;
                };
                let seen = self.frontiers.get(&(def_id, id)).copied().unwrap_or(0);
                if frontier > seen {
                    moved.push((def_id, id, channels, seen, frontier));
                }
            }
        }
        let mut redraw = Vec::new();
        for (def_id, widget_id, channels, seen, frontier) in moved {
            self.frontiers.insert((def_id, widget_id), frontier);
            let Some(tree) = self.host.window_def_mut(def_id) else {
                continue;
            };
            let Some(w) = tree.find_mut(widget_id) else {
                continue;
            };
            let Some(el) = w.kind.as_element_mut() else {
                continue;
            };
            let mut followed = false;
            for ch in 0..channels {
                followed |= el.refresh_material(ch, seen, (frontier - seen) as usize);
            }
            if followed && !redraw.contains(&def_id) {
                redraw.push(def_id);
            }
        }
        redraw
    }

    /// Off Unix there is no mapped material to follow — the picture arrives by
    /// message there, and so does the news that it changed.
    #[cfg(not(unix))]
    pub(super) fn follow_recordings(&mut self) -> Vec<i32> {
        Vec::new()
    }

    /// What a placed buffer leaves behind whichever way it arrived: its extent
    /// joins the widget's navigation group, and a widget that knew no sample
    /// rate takes the material's so its ruler can label real time.
    fn finish_placement(&mut self, want: WaveWant, frames: usize, sample_rate: f64) {
        self.host.set_timeline_total(want.widget_id, frames);
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
