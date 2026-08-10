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
                    // resolves to the element that wanted the samples.
                    let Some(kind) = self
                        .host
                        .window_def(want.def_id)
                        .and_then(|t| t.find(want.widget_id))
                        .map(|w| {
                            w.signal_target()
                                .map(|el| WidgetKind::Signal(Box::new(el.clone())))
                                .unwrap_or_else(|| w.kind.clone())
                        })
                    else {
                        continue;
                    };
                    match kind {
                        WidgetKind::Signal(ref el)
                            if el.presentation == Presentation::Signal && el.is_gpu_view() =>
                        {
                            let bucket = el
                                .source
                                .data()
                                .map_or(signal::DEFAULT_BASE_BUCKET, |d| d.base_bucket);
                            let data = WaveformData::from_interleaved(&samples, channels, bucket);
                            self.place_bulk(want.def_id, want.widget_id, BulkData::Waveform(data));
                        }
                        // A **mesh-drawn take** (a clip's body): its pyramid
                        // lands in the tree, no GPU slot — the same landing the
                        // mapped bulk path uses.
                        WidgetKind::Signal(ref el) if !el.needs_gpu_slot() => {
                            let bucket = el
                                .source
                                .data()
                                .map_or(signal::DEFAULT_BASE_BUCKET, |d| d.base_bucket);
                            let data = WaveformData::from_interleaved(&samples, channels, bucket);
                            self.set_take_body(want.def_id, want.widget_id, data);
                            // Falls through to the shared tail: a body carries
                            // no editor props, so the sample-rate fill is a
                            // no-op for it, but the **repaint** is not — a
                            // `continue` here left the take sitting in the tree
                            // with the canvas still showing the frame before it.
                        }
                        WidgetKind::Signal(ref el)
                            if el.presentation == Presentation::TimeFrequency
                                && el.needs_gpu_slot() =>
                        {
                            let rate = if el.editor.sample_rate > 0.0 {
                                el.editor.sample_rate
                            } else {
                                sample_rate
                            };
                            let stfts = frame::stft_lanes(
                                frame::deinterleave(&samples, channels),
                                el.spectral.fft_size,
                                el.spectral.hop,
                                rate,
                            );
                            self.place_bulk(
                                want.def_id,
                                want.widget_id,
                                BulkData::Spectrogram(stfts),
                            );
                        }
                        _ => continue,
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
                    self.request_redraw(want.def_id);
                }
            }
            FetchStep::None => {}
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
