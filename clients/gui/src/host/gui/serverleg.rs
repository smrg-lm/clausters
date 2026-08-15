//! The audio-server client leg, main-thread side: routing the server's replies
//! (`/buffer_query.reply`/`/buffer_getRange.reply` into the buffer-fetch machine, `/group_queryTree.reply`
//! into the node-tree store), the `/server_notify` registration and the node-tree
//! re-query, and placing a finished buffer download into its waiting views.

use std::sync::Arc;
use std::time::Instant;

use clausters_core::osc::{OscMessage, OscPacket, OscType};
use tracing::{debug, info, warn};

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
            if let Some(query) = self.fetches.want(def_id, widget_id, bufnum) {
                self.send_to_server(query);
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
        if self.host.server().and_then(|s| s.embed()).is_none() {
            return;
        }
        let mut packets: Vec<Vec<u8>> = Vec::new();
        if let Some(embed) = self.host.server().and_then(|s| s.embed()) {
            let mut buf = vec![0u8; 65536];
            while let Some(n) = embed.poll_into(&mut buf) {
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
            // The fetched buffer's extent joins the widget's navigation group.
            self.host
                .set_timeline_total(want.widget_id, samples.len() / channels);
            // Let the ruler label real time when the widget knew no rate.
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
