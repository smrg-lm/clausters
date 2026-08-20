//! The host's leg to the **audio server**, in the browser: the two carriers and
//! what comes back over them.
//!
//! A bound widget bypasses the script here exactly as it does natively -- its
//! value goes straight to the audio server -- and the two carriers are the
//! deployment's, not the protocol's: [`WsServerLink`] to a separate `--ws`
//! server, [`PageServerLink`] to an in-page worklet engine. Both encode through
//! the one door.
//!
//! What the browser does *not* have is the shared-memory segment, and that is
//! the whole difference from the native leg: a live bus arrives as a
//! `/bus_stream` subscription and a tap window as `/bus_tapStream`, both
//! re-diffed whenever the trees change, so the page asks for exactly what it
//! draws.

use super::*;

/// The host's audio-server leg over a browser `WebSocket` to a `--ws` server.
/// Bidirectional: outbound frames carry bound-widget values and the host's own
/// requests (`/bus_stream`, `/buffer_query`, `/buffer_getRange`); inbound frames (the server's
/// replies and streamed `/bus_stream.reply` snapshots) are forwarded into the event loop
/// as [`WebEvent::ServerInbound`] and decode through the one `decode_packet`
/// door. Frames sent before the socket opens are buffered and flushed on open,
/// so a `connect` immediately followed by interaction does not drop values.
pub struct WsServerLink {
    socket: web_sys::WebSocket,
    open: Rc<Cell<bool>>,
    pending: Rc<RefCell<Vec<Vec<u8>>>>,
}

impl WsServerLink {
    /// Opens a WebSocket to `url` (e.g. `ws://127.0.0.1:57120`) for `host` —
    /// the instance whose leg this is, since a page may hold several and each
    /// reaches its own server.
    pub(crate) fn connect(url: &str, host: HostId) -> Result<Self, String> {
        let socket = web_sys::WebSocket::new(url).map_err(|e| format!("{e:?}"))?;
        socket.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let open = Rc::new(Cell::new(false));
        let pending: Rc<RefCell<Vec<Vec<u8>>>> = Rc::new(RefCell::new(Vec::new()));
        // On open: mark open and flush whatever was buffered before the handshake.
        let socket2 = socket.clone();
        let open2 = open.clone();
        let pending2 = pending.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            open2.set(true);
            for frame in pending2.borrow_mut().drain(..) {
                let _ = socket2.send_with_u8_array(&frame);
            }
            log("audio-server WebSocket open");
        });
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget();
        // Inbound: each binary frame is one OSC packet from the audio server
        // (a streamed `/bus_stream.reply`, a `/buffer_query.reply`/`/buffer_getRange.reply` reply, a `/fail`),
        // forwarded to the app through the event-loop proxy.
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |event: web_sys::MessageEvent| {
                let Ok(buffer) = event.data().dyn_into::<js_sys::ArrayBuffer>() else {
                    return; // non-binary frames carry nothing of ours
                };
                let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                if let Some(proxy) = web_proxy() {
                    let _ = proxy.send_event(HostEvent::To(host, WebEvent::ServerInbound(bytes)));
                }
            },
        );
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();
        Ok(Self {
            socket,
            open,
            pending,
        })
    }

    /// Sends one OSC message as a binary frame (one packet per frame, the
    /// WebSocket wire format the audio server speaks), buffering until the
    /// socket is open.
    pub fn send(&self, msg: OscMessage) -> std::io::Result<()> {
        let bytes = encode(&OscPacket::Message(msg))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        if self.open.get() {
            self.socket
                .send_with_u8_array(&bytes)
                .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        } else {
            self.pending.borrow_mut().push(bytes);
        }
        Ok(())
    }
}

/// The host's audio-server leg to the **in-page engine** (the AudioWorklet
/// backend): outbound OSC packets are handed to a page-registered JS callback
/// (which forwards them to the worklet's MessagePort); inbound replies arrive
/// through [`GuiBridge::server_reply`]. The whole audio server lives in the
/// same tab — no process, no socket, no headers.
pub struct PageServerLink {
    pub(super) callback: js_sys::Function,
}

impl PageServerLink {
    /// Sends one OSC message by invoking the page's callback with the encoded
    /// bytes (a fresh `Uint8Array` per call; the page may transfer it onward).
    pub fn send(&self, msg: OscMessage) -> std::io::Result<()> {
        let bytes = encode(&OscPacket::Message(msg))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let array = js_sys::Uint8Array::from(bytes.as_slice());
        self.callback
            .call1(&JsValue::NULL, &array)
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        Ok(())
    }
}

impl WebApp {
    /// Subscribes the audio server to exactly the control buses the drawing
    /// canvases read live (`/bus_stream`, replacing this client's previous
    /// subscription), or cancels when none are left. Skipped without a server
    /// leg; `ConnectServer` re-runs it once the leg exists.
    pub(super) fn sync_bus_stream(&mut self, wanted: Vec<i32>) {
        if wanted == self.streamed {
            return;
        }
        let Some(server) = self.host.server() else {
            return;
        };
        let mut args = Vec::with_capacity(wanted.len() + 1);
        if wanted.is_empty() {
            args.push(OscType::Int(0)); // periodMs 0: cancel
        } else {
            args.push(OscType::Int(live::STREAM_PERIOD_MS));
            args.extend(wanted.iter().map(|&bus| OscType::Int(bus)));
        }
        match server.send(OscMessage {
            addr: "/bus_stream".into(),
            args,
        }) {
            Ok(()) => {
                log(&format!("/bus_stream subscription: {wanted:?}"));
                self.streamed = wanted;
            }
            Err(e) => log(&format!("failed to (re)subscribe /bus_stream: {e}")),
        }
    }

    /// Subscribes the audio server to exactly the audio taps the drawing
    /// canvases' oscilloscopes read (`/bus_tapStream`, replacing this client's
    /// previous subscription), sized to the largest raw window any of them
    /// needs; cancels when none are left. Skipped without a server leg.
    pub(super) fn sync_tap_stream(&mut self, wanted: Vec<i32>, frames: usize) {
        if (wanted.clone(), frames) == self.tap_streamed {
            return;
        }
        let Some(server) = self.host.server() else {
            return;
        };
        let mut args = Vec::with_capacity(wanted.len() + 2);
        if wanted.is_empty() {
            args.push(OscType::Int(0)); // periodMs 0: cancel
            args.push(OscType::Int(0));
        } else {
            args.push(OscType::Int(live::STREAM_PERIOD_MS));
            args.push(OscType::Int(frames as i32));
            args.extend(wanted.iter().map(|&tap| OscType::Int(tap)));
        }
        match server.send(OscMessage {
            addr: "/bus_tapStream".into(),
            args,
        }) {
            Ok(()) => {
                log(&format!(
                    "/bus_tapStream subscription: {wanted:?} x{frames}"
                ));
                self.tap_streamed = (wanted, frames);
            }
            Err(e) => log(&format!("failed to (re)subscribe /bus_tapStream: {e}")),
        }
    }

    /// Routes one decoded OSC packet from the audio server (the WS leg): the
    /// streamed `/bus_stream.reply` snapshots into [`StreamedBuses`], the buffer replies
    /// into the shared fetch machine. The browser twin of the native
    /// `handle_server_packet`.
    pub(super) fn on_server_inbound(&mut self, bytes: &[u8]) {
        let packet = match decode_packet(bytes) {
            Ok(p) => p,
            Err(e) => return log(&format!("malformed OSC packet from the server: {e}")),
        };
        let OscPacket::Message(msg) = packet else {
            return; // bundles are not used on the reply path
        };
        match msg.addr.as_str() {
            "/bus_stream.reply" => {
                if !self.stream_seen {
                    self.stream_seen = true;
                    log(&format!("bus stream flowing: {:?}", msg.args));
                }
                for pair in msg.args.chunks(2) {
                    if let [OscType::Int(index), OscType::Float(value)] = pair
                        && *index >= 0
                    {
                        self.buses.set(*index as usize, *value);
                    }
                }
            }
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
                        let rate = match rate {
                            OscType::Float(x) => *x as f64,
                            OscType::Double(x) => *x,
                            _ => 0.0,
                        };
                        let step = self.fetches.on_info(
                            *bufnum,
                            (*frames).max(0) as usize,
                            (*channels).max(0) as usize,
                            rate,
                        );
                        self.apply_fetch_step(step);
                    }
                }
            }
            "/buffer_getRange.reply" => {
                let step = self.fetches.on_data(&msg.args);
                self.apply_fetch_step(step);
            }
            // **Another peer wrote a span of samples this page is drawing.**
            // A page holds its **own copy** — it cannot map anything — so the
            // announcement names a span whose samples are not in it, and no
            // summary over what it holds can find the edit. So the span is
            // read back off the wire and put where the picture keeps it, which
            // is what a mapped host gets for the price of re-summarizing.
            "/buffer_touched" => {
                if let [
                    OscType::Int(bufnum),
                    OscType::Int(channel),
                    OscType::Int(start),
                    OscType::Int(frames),
                ] = msg.args.as_slice()
                {
                    let mut refreshed = 0;
                    let ids = self.host.window_def_ids();
                    for def_id in ids {
                        if let Some(tree) = self.host.window_def_mut(def_id) {
                            refreshed += crate::host::refresh_buffer_views(
                                tree,
                                *bufnum,
                                (*channel).max(0) as usize,
                                (*start).max(0) as u64,
                                (*frames).max(0) as usize,
                            );
                        }
                    }
                    if refreshed == 0 {
                        self.read_span_back(*bufnum, (*start).max(0), (*frames).max(0));
                    }
                }
            }
            // **The recording this page cannot read, reported by the server.**
            // A page maps nothing and holds its own copy of the samples, so a
            // take filling in the server's memory reaches it only as the
            // overview of what was written -- min, max and energy per bucket,
            // folded into the pyramid the picture already holds. This is the
            // page's half of what a mapped host gets from a frontier.
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
            "/bus_tapStream.reply" => {
                // (tap, stream position, raw LE f32 blob): the newest window
                // of one tap; store it for the tick to align and draw.
                if let (Some(OscType::Int(tap)), Some(OscType::Blob(bytes))) =
                    (msg.args.first(), msg.args.get(2))
                {
                    let samples: Vec<f32> = bytes
                        .chunks_exact(4)
                        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect();
                    // The position the window ends at: what retention appends
                    // by, so a slow tick never stretches the history.
                    let at = match msg.args.get(1) {
                        Some(OscType::Long(p)) => *p as u64,
                        Some(OscType::Int(p)) => *p as u64,
                        _ => 0,
                    };
                    self.taps.set(*tap, samples, at);
                }
            }
            "/clock_query.reply" => {
                // (samples, rate, ...): keep the rate for window sizing and
                // the sample counter for the timeline playhead.
                if let Some(OscType::Long(samples)) = msg.args.first() {
                    self.server_clock = *samples as f64;
                }
                if let Some(OscType::Double(rate)) = msg.args.get(1) {
                    self.server_rate = *rate;
                    // Window sizes may change with the real rate known.
                    let demand = self.demand();
                    self.sync_tap_stream(demand.taps, demand.tap_frames);
                }
            }
            "/fail" => log(&format!("audio server replied /fail: {:?}", msg.args)),
            // `/done` acks (e.g. for `/bus_stream`) need no action.
            "/done" => {}
            _ => {}
        }
    }

    /// Carries out one fetch-machine step: send the next request over the WS
    /// leg, or turn a finished buffer into view data for its widgets —
    /// looking each widget up in the tree, like the native front, to decide
    /// between a multichannel waveform and per-channel STFT lanes.
    fn apply_fetch_step(&mut self, step: FetchStep) {
        match step {
            FetchStep::Request(msg) => self.send_to_server(msg),
            FetchStep::Done {
                bufnum,
                samples,
                channels,
                sample_rate,
                wants,
            } => {
                let channels = channels.max(1);
                log(&format!(
                    "buffer {bufnum}: {} frames x {channels} channel(s) loaded into {} view(s)",
                    samples.len() / channels,
                    wants.len()
                ));
                for want in wants {
                    if !self.canvases.contains_key(&want.def_id) {
                        continue;
                    }
                    // The fetch was keyed by a widget id, and for a clip that
                    // is the *clip's* — a body carries none — so the reply
                    // resolves to the element that wanted the samples. What is
                    // read out is the **declaration**: a slot says where the
                    // data goes and what has to be made of it first.
                    let Some(slot) = self
                        .host
                        .window_def(want.def_id)
                        .and_then(|t| t.find(want.widget_id))
                        .map(|w| w.bulk_target().kind.needs().slot)
                    else {
                        continue;
                    };
                    match slot {
                        Some(SlotKind::Geometry { base_bucket }) => {
                            let data = std::sync::Arc::new(WaveformData::from_interleaved(
                                &samples,
                                channels,
                                base_bucket,
                            ));
                            // The same pyramid to the slot and to the element,
                            // as everywhere else (`frame::keep_data`).
                            if let Some(w) = self
                                .host
                                .window_def_mut(want.def_id)
                                .and_then(|t| t.find_mut(want.widget_id))
                            {
                                frame::keep_data(w, &Loaded::Peaks(data.clone()));
                            }
                            self.place_bulk(want.def_id, want.widget_id, Loaded::Peaks(data));
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
                            self.place_bulk(want.def_id, want.widget_id, Loaded::Stfts(stfts));
                        }
                        // Mesh-drawn: the samples go home to the element, which
                        // makes of them whatever it draws from. It falls
                        // through to the shared tail — a body carries no editor
                        // props, so the sample-rate fill is a no-op for it, but
                        // the **repaint** is not.
                        _ => {
                            if let Some(w) = self
                                .host
                                .window_def_mut(want.def_id)
                                .and_then(|t| t.find_mut(want.widget_id))
                            {
                                let raw = || Loaded::Raw {
                                    samples: samples.to_vec(),
                                    channels,
                                };
                                w.take_bulk(raw);
                            }
                        }
                    }
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
                    // The samples just arrived, so whether this view has to
                    // be told about its own recording is only answerable now.
                    self.host.sync_buffer_streams();
                    self.request_redraw(want.def_id);
                }
            }
            // **A take drawn from its summary**: the shape is the answer and
            // no samples are downloaded. Where the summary comes from is the
            // one difference between the two cases this covers -- a take being
            // recorded into is streamed one (the samples are silence until
            // something writes them), and a take standing still is asked for
            // one. The run under the eye is read back on a zoom, either way.
            FetchStep::Empty {
                bufnum,
                frames,
                channels,
                sample_rate,
                ask_summary,
                wants,
            } => {
                let channels = channels.max(1);
                log(&format!(
                    "buffer {bufnum}: drawn from its summary ({frames} frames x {channels} \
                     channel(s)); {} view(s), {}",
                    wants.len(),
                    if ask_summary { "asked for" } else { "streamed" }
                ));
                for want in wants {
                    let Some(base_bucket) = self.summary_bucket_of(want.def_id, want.widget_id)
                    else {
                        continue;
                    };
                    let data = Arc::new(crate::waveform::WaveformData::with_multi_pyramid(
                        MultiPyramid::empty(frames, channels, base_bucket),
                    ));
                    if let Some(w) = self
                        .host
                        .window_def_mut(want.def_id)
                        .and_then(|t| t.find_mut(want.widget_id))
                    {
                        crate::host::frame::keep_data(w, &Loaded::Peaks(data.clone()));
                    }
                    self.place_bulk(want.def_id, want.widget_id, Loaded::Peaks(data));
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
                    self.host.sync_buffer_streams();
                    self.request_redraw(want.def_id);
                }
                if ask_summary {
                    self.ask_peaks(bufnum, 0);
                }
            }
            FetchStep::Window {
                bufnum,
                want,
                start_frame,
                channels,
                samples,
            } => {
                log(&format!(
                    "buffer {bufnum}: {} frame(s) at {start_frame} read back for widget {}",
                    samples.len() / channels.max(1),
                    want.widget_id
                ));
                if let Some(slot) = self
                    .canvases
                    .get_mut(&want.def_id)
                    .and_then(|c| c.render.as_mut())
                    .and_then(|r| r.waveforms.get_mut(&want.widget_id))
                {
                    slot.view.release_data();
                }
                let took =
                    self.host
                        .window_def_mut(want.def_id)
                        .and_then(|t| t.find_mut(want.widget_id))
                        .is_some_and(|w| {
                            w.bulk_target_mut().kind.as_element_mut().is_some_and(|el| {
                                el.set_window(start_frame as u64, channels, &samples)
                            })
                        });
                if took {
                    self.request_redraw(want.def_id);
                }
            }
            // **A span another peer wrote**, read back. It goes to every view
            // of that buffer, not only to whoever asked: the samples are the
            // buffer's own, so any picture of it is entitled to them.
            FetchStep::Patch {
                bufnum,
                start_frame,
                channels,
                samples,
            } => {
                log(&format!(
                    "buffer {bufnum}: {} frame(s) at {start_frame} read back after an edit",
                    samples.len() / channels.max(1),
                ));
                let mut redraw = Vec::new();
                for def_id in self.host.window_def_ids() {
                    // A pyramid a slot is holding cannot be written in place,
                    // so the samples go first -- the same order a streamed
                    // report takes, and for the same reason. **Only the slots
                    // drawing this buffer**: a slot released and not refilled
                    // draws nothing, so releasing every one of them blanks
                    // every other take on the canvas.
                    for widget_id in self.widgets_drawing(def_id, bufnum) {
                        if let Some(slot) = self
                            .canvases
                            .get_mut(&def_id)
                            .and_then(|c| c.render.as_mut())
                            .and_then(|r| r.waveforms.get_mut(&widget_id))
                        {
                            slot.view.release_data();
                        }
                    }
                    let Some(tree) = self.host.window_def_mut(def_id) else {
                        continue;
                    };
                    if crate::host::patch_buffer_views(
                        tree,
                        bufnum,
                        start_frame as u64,
                        channels,
                        &samples,
                    ) > 0
                    {
                        redraw.push(def_id);
                    }
                }
                for def_id in redraw {
                    self.request_redraw(def_id);
                }
                if let Some(msg) = self.fetches.queued_span(bufnum) {
                    self.send_to_server(msg);
                }
            }
            FetchStep::None => {}
        }
    }

    /// The widgets of `def_id` drawing server buffer `bufnum` — whose GPU slots
    /// a write to that buffer has to release before the element rewrites the
    /// pyramid they share.
    fn widgets_drawing(&self, def_id: i32, bufnum: i32) -> Vec<i32> {
        self.host
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
            .unwrap_or_default()
    }

    /// **The bucket a view's summary is built at**, which is what a request for
    /// one has to be phrased in. The element declares it with the resource it
    /// wants; a view that declared none takes the default every signal element
    /// summarizes at.
    fn summary_bucket_of(&self, def_id: i32, widget_id: i32) -> Option<usize> {
        use crate::host::widget::element::{Bulk, SlotKind};
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
    /// several -- walked by the replies themselves rather than by a state
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

    /// **Reads an announced span back off the wire.** A page maps nothing, so
    /// the samples it draws are a download and an edit somebody else made is
    /// not in them -- no summary over what it holds can find it. The
    /// announcement says where to look, and this asks for exactly that span,
    /// widened to the summary's buckets so what comes back can replace what
    /// the summary says over it.
    fn read_span_back(&mut self, bufnum: i32, start: i32, frames: i32) {
        let (Ok(start), Ok(frames)) = (usize::try_from(start), usize::try_from(frames)) else {
            return;
        };
        let Some((channels, bucket)) = self.host.window_def_ids().into_iter().find_map(|def_id| {
            self.host
                .window_def(def_id)
                .and_then(|tree| crate::host::span_to_read_back(tree, bufnum))
        }) else {
            return log(&format!(
                "buffer {bufnum} was edited by another peer; nothing here draws it"
            ));
        };
        let (start, frames) = align_span(start, frames, bucket);
        if let Some(msg) = self
            .fetches
            .want_span(bufnum, start, frames, channels, SpanUse::Patch)
        {
            self.send_to_server(msg);
        }
    }

    /// **Asks for the spans the last frame could not draw** on this canvas: a
    /// view zoomed finer than its summary left the span it was asked for on
    /// its slot, and this turns that note into a `/buffer_getRange`.
    ///
    /// One download per buffer is in flight at a time (the fetch machine's own
    /// bound), and nothing is asked while the zoom stays above the bucket —
    /// where the summary is the right answer and already on screen.
    pub(super) fn fetch_wanted_spans(&mut self, def_id: i32) {
        let mut asked: Vec<(i32, i32, usize, usize, usize)> = Vec::new();
        if let Some(render) = self.canvases.get(&def_id).and_then(|c| c.render.as_ref()) {
            for (widget_id, slot) in &render.waveforms {
                let Some((a, b)) = slot.wanted_span.take() else {
                    continue;
                };
                let Some(el) = self
                    .host
                    .window_def(def_id)
                    .and_then(|t| t.find(*widget_id))
                    .and_then(|w| w.bulk_target().kind.as_element())
                else {
                    continue;
                };
                let (Some(bufnum), Some((channels, _))) = (el.source_buffer(), el.sample_shape())
                else {
                    continue;
                };
                asked.push((*widget_id, bufnum, a, b - a, channels));
            }
        }
        for (widget_id, bufnum, start, frames, channels) in asked {
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

    /// Folds one `/buffer_stream.reply` into every view of that buffer and
    /// repaints the canvases that took it.
    ///
    /// The slots let the samples go first, the way the mapped path does: a
    /// pyramid a slot is holding cannot be written in place, so the element
    /// would copy the whole take before patching the buckets that arrived.
    pub(super) fn on_stream_report(
        &mut self,
        bufnum: i32,
        start: u64,
        bucket: usize,
        stats: &[f32],
    ) {
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
                    .canvases
                    .get_mut(&def_id)
                    .and_then(|c| c.render.as_mut())
                    .and_then(|r| r.waveforms.get_mut(&widget_id))
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
            self.request_redraw(def_id);
        }
    }

    /// Sends one fetch-machine message over the WS leg (`/buffer_query`, `/buffer_getRange`),
    /// logging instead of failing when no server is attached.
    pub(super) fn send_to_server(&self, msg: OscMessage) {
        let Some(server) = self.host.server() else {
            return log("waveform references a server buffer but no --ws server is connected");
        };
        let addr = msg.addr.clone();
        if let Err(e) = server.send(msg) {
            log(&format!("failed to send {addr} to the audio server: {e}"));
        }
    }

    /// Encodes `msg` and pushes it to the outbox for the page to drain; also logs
    /// a short summary so events are visible without a JS OSC decoder.
    pub(super) fn queue(&self, msg: OscMessage) {
        log(&format!("-> {} {:?}", msg.addr, msg.args));
        match encode(&OscPacket::Message(msg)) {
            Ok(bytes) => self.outbox.borrow_mut().push_back(bytes),
            Err(e) => log(&format!("failed to encode an outbound packet: {e}")),
        }
    }
}
