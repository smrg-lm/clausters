//! The audio-server client leg, main-thread side: routing the server's replies
//! (`/buffer_query.reply`/`/buffer_getRange.reply` into the buffer-fetch machine, `/group_queryTree.reply`
//! into the node-tree store), the `/server_notify` registration and the node-tree
//! re-query, and placing a finished buffer download into its waiting views.

use std::sync::Arc;
use std::time::Instant;

use clausters_core::osc::{OscMessage, OscPacket, OscType};
use tracing::{debug, warn};

use crate::host::Host;
#[cfg(feature = "standalone")]
use crate::host::ServerLink;
use crate::host::fetch::{BufferFetches, WaveWant};
use crate::host::frame::{self, Owed};
use crate::host::graphics::nodetree::NodeTree;
use crate::host::instance::Leg;
use crate::host::replies::Front;
use crate::host::widget::Widget;
use crate::host::widget::element::{Loaded, SlotKey, SlotKind};
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
            // it -- no `/buffer_query`, no chunked `/buffer_getRange`, no
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
    /// **Nothing is read out.** The picture is built over the mapping itself --
    /// one [`crate::host::mapped::MappedChannel`] per channel, summarized
    /// where it lies -- so opening a ten-minute take allocates its pyramid and
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
            bufnum,
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

    /// [`Front::place_samples`] for samples that is **mapped** rather than
    /// downloaded: the views share one `WaveformData` reading the region, and
    /// the pyramid over it is built once for every want.
    #[cfg(unix)]
    fn place_mapped_buffer_data(
        &mut self,
        bufnum: i32,
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
                let stfts = frame::stft_channels(
                    frame::deinterleave(&take.read_all(), channels),
                    window_size,
                    hop,
                    rate,
                );
                if let Some(slot) = frame::spectrogram_slot(stfts, &ws.gpu, &ws.renderers) {
                    ws.spectrograms
                        .insert((want.widget_id, SlotKey::SELF), slot);
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
                ws.waveforms.insert(
                    (want.widget_id, SlotKey::SELF),
                    frame::waveform_slot(data.clone()),
                );
            }
            ws.gpu.window.request_redraw();
            if let Some(w) = self
                .host
                .window_def_mut(want.def_id)
                .and_then(|t| t.find_mut(want.widget_id))
            {
                w.take_bulk_of(bufnum, || Loaded::Shared(data.clone()));
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
                Ok(packet) => self.handle_server_packet(packet, Leg::Server),
                Err(e) => warn!("malformed OSC reply from the embedded server: {e}"),
            }
        }
    }

    /// Without the `standalone` feature there is no embed link, so draining its
    /// replies is nothing -- kept so the event loop calls it unconditionally.
    #[cfg(not(feature = "standalone"))]
    pub(super) fn drain_embed_replies(&mut self) {}

    /// Routes one decoded reply from the audio server (the client leg).
    pub(super) fn handle_server_packet(&mut self, packet: OscPacket, from: Leg) {
        let OscPacket::Message(msg) = packet else {
            return; // bundles are not used on the reply path yet
        };
        // The ids, the multitrack's waiting steps and the buffer replies are
        // read once for both fronts ([`Front::on_server_reply`]); what is left
        // is this front's own.
        if self.on_server_reply(from, &msg) {
            return;
        }
        match msg.addr.as_str() {
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

    /// **Asks for the spans the last frame could not draw.** A view zoomed
    /// finer than its summary leaves the span it was asked for on its slot;
    /// this is where that note becomes a `/buffer_getRange`.
    ///
    /// Called after drawing, once per pass: the note is this frame's, and a
    /// span already in flight is not asked for again (the fetch machine keeps
    /// one download per buffer, which is what bounds this).
    ///
    /// The summary walks are ticked here too, for the same reason and against
    /// the same clock: a multitrack of a summary that never came back is asked for
    /// again rather than leaving a hole in the picture.
    pub(super) fn fetch_wanted_spans(&mut self) {
        let open: Vec<i32> = self.windows.keys().copied().collect();
        self.ask_owed_spans(&open);
    }

    /// Whether this window draws samples that is **still being written** --
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
            let Some(el) = w.kind.as_samples() else {
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
    /// view is current with nothing done at all -- but the *overview* is a
    /// summary of what was there when it was taken, and nothing announces an
    /// engine write (a `RecordBuf` filling a take says nothing on the wire,
    /// correctly). What the writer does publish is how far it has got, and
    /// that is exactly the span to re-read.
    ///
    /// Called on the frame tick. Costs one relaxed load per drawn buffer while
    /// nothing is recording, and the summary of the new frames when something
    /// is -- never the take.
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
                // widget that does -- the same addressing every other samples
                // path here uses.
                let (Some(id), Some(el)) = (w.id, w.kind.as_samples()) else {
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
            // it.** A pyramid a slot is holding cannot be written in place --
            // the element would be patching a picture under a renderer that
            // never asked -- so the refresh below would have to copy it first,
            // and that copy is the size of the whole take rather than of the
            // block that just arrived. Letting go is a refcount, the write is
            // then the block's own cost, and the slot is refilled before the
            // next draw: a repaint runs `refresh_slots_for` first, and the
            // write leaves the element dirty, which is what a fill answers to.
            if let Some(slot) = self
                .windows
                .get_mut(&def_id)
                .and_then(|ws| ws.waveforms.get_mut(&(widget_id, SlotKey::SELF)))
            {
                slot.view.release_data();
            }
            let Some(tree) = self.host.window_def_mut(def_id) else {
                continue;
            };
            let Some(w) = tree.find_mut(widget_id) else {
                continue;
            };
            let Some(el) = w.kind.as_samples_mut() else {
                continue;
            };
            // **Every channel in one refresh.** The frontier is the buffer's,
            // not a channel's, so they all advance together -- and a refresh
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

    /// Off Unix there is no mapped samples to follow -- the picture arrives by
    /// message there, and so does the news that it changed.
    #[cfg(not(unix))]
    pub(super) fn follow_recordings(&mut self) -> Vec<i32> {
        Vec::new()
    }
}

/// The native front's side of the replies read once for both fronts: a
/// window's slots live in its `WindowState`, and a message leaves over the
/// client leg.
impl Front for App {
    fn host(&self) -> &Host {
        &self.host
    }

    fn host_mut(&mut self) -> &mut Host {
        &mut self.host
    }

    fn fetches(&mut self) -> &mut BufferFetches {
        &mut self.fetches
    }

    fn to_server(&self, msg: OscMessage) {
        self.send_to_server(msg);
    }

    fn redraw_window(&self, def_id: i32) {
        if let Some(ws) = self.windows.get(&def_id) {
            ws.gpu.window.request_redraw();
        }
    }

    fn window_open(&self, def_id: i32) -> bool {
        self.windows.contains_key(&def_id)
    }

    fn place_slot(&mut self, def_id: i32, widget_id: i32, data: Loaded) -> Option<usize> {
        let ws = self.windows.get_mut(&def_id)?;
        frame::place_in_slot(
            data,
            (widget_id, SlotKey::SELF),
            &ws.gpu,
            &ws.renderers,
            &mut ws.waveforms,
            &mut ws.spectrograms,
        )
    }

    fn release_slot(&mut self, def_id: i32, widget_id: i32) {
        if let Some(slot) = self
            .windows
            .get_mut(&def_id)
            .and_then(|ws| ws.waveforms.get_mut(&(widget_id, SlotKey::SELF)))
        {
            slot.view.release_data();
        }
    }

    fn take_owed(&self, def_id: i32) -> Vec<(i32, Owed)> {
        let Some(ws) = self.windows.get(&def_id) else {
            return Vec::new();
        };
        ws.waveforms
            .iter()
            .filter_map(|((widget_id, _key), slot)| slot.owed.take().map(|o| (*widget_id, o)))
            .collect()
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
