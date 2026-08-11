//! The **binding surface** the page holds: [`GuiBridge`].
//!
//! The instance's whole JS-facing API, and the only `wasm_bindgen` surface in
//! the host: OSC packets and GuiDefs in, `/gui_event`/`/gui_closed`/`/gui_info`
//! out, plus the page-side facts the agnostic core may never ask a platform for
//! -- a canvas, its size and its `devicePixelRatio`, the theme and metrics
//! tables. Everything crossing it is either raw OSC bytes or JSON, so the
//! surface stays the same shape as the wire.
//!
//! [`start`] is the entry: it builds the page's one event loop on first call
//! and adds an instance on every later one.

use super::*;

/// The binding surface JS holds: feed OSC packets / GuiDefs in, drain events out,
/// and connect the audio-server WebSocket. It reaches the running app through the
/// event-loop proxy and shares the outbox queue.
///
/// One bridge is one host instance. A page that calls [`start`] once — every
/// served page — never sees the distinction; one that calls it again gets a
/// second host that shares nothing with the first.
#[wasm_bindgen]
pub struct GuiBridge {
    /// Which instance this drives. Every event carries it, since the page's
    /// instances share one event loop and one proxy.
    id: HostId,
    proxy: EventLoopProxy<HostEvent>,
    outbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

impl GuiBridge {
    /// Addresses one event to this bridge's instance and posts it.
    ///
    /// A failed send means the loop is gone (the page is going away), which is
    /// nothing a caller can act on — the same posture the discarded results
    /// here always had.
    pub(super) fn send(&self, event: WebEvent) {
        let _ = self.proxy.send_event(HostEvent::To(self.id, event));
    }
}

#[wasm_bindgen]
impl GuiBridge {
    /// Feeds one raw OSC packet (e.g. a `/gui_def`/`/gui_set`/`/gui_bind`) to the
    /// host, exactly as the WS wire format delivers it (one packet per call).
    pub fn feed(&self, packet: &[u8]) {
        self.send(WebEvent::Inbound(packet.to_vec()));
    }

    /// Gives one `window`-rooted def its own `<canvas>`, which the caller
    /// created and the document places.
    ///
    /// This is the browser's answer to the desktop's window manager: on the
    /// desktop `clausters-gui` opens a window per def and the system places it;
    /// in a tab the canvas is an element and **the document places it** — CSS,
    /// the order of the markup. Attach before feeding the def's `/gui_def`, so
    /// the first frame draws into the right surface. Attaching a def that
    /// already has a canvas replaces it.
    ///
    /// A page that never calls this still works: a `/gui_def` with no canvas
    /// gets one appended to `<body>`, the older single-canvas posture.
    pub fn attach(&self, def_id: i32, canvas: web_sys::HtmlCanvasElement) {
        self.send(WebEvent::Attach {
            def_id,
            canvas: Some(canvas),
        });
    }

    /// Frees a def's canvas: its GPU surface and every derived resource go. The
    /// `<canvas>` element itself is the page's, to remove or reuse.
    pub fn detach(&self, def_id: i32) {
        self.send(WebEvent::Detach(def_id));
    }

    /// Sizes a canvas in **device pixels**, with the **scale** those pixels were
    /// measured at — a component's `ResizeObserver` box times
    /// `devicePixelRatio`, and that ratio. The host never reads the DOM: the
    /// element owns its box and reports the pixels.
    ///
    /// Both halves are needed and neither substitutes for the other. The
    /// backing store is device pixels, so the surface takes the product; the
    /// widget sizes a GuiDef declares are **logical**, so resolving them takes
    /// the ratio — and a product cannot be un-multiplied. A page that already
    /// scales its box by `devicePixelRatio` passes the same ratio here.
    pub fn resize(&self, def_id: i32, width: u32, height: u32, scale: f32) {
        self.send(WebEvent::Resize {
            def_id,
            width,
            height,
            scale,
        });
    }

    /// Tells the host whether a canvas is in the viewport (a component's
    /// `IntersectionObserver`).
    ///
    /// A hidden canvas is skipped on the tick and its buses leave the
    /// `/bus_stream`/`/bus_tapStream` sets — a document can hold fifty canvases with
    /// three in view, and neither this host nor the server should be working
    /// for the other forty-seven.
    pub fn set_visible(&self, def_id: i32, visible: bool) {
        self.send(WebEvent::SetVisible { def_id, visible });
    }

    /// Convenience: build and feed a `/gui_def <id> <json>` from a GuiDef JSON
    /// string — the same JSON the Python builders emit, so a page needs no OSC
    /// encoder of its own.
    pub fn def(&self, id: i32, json: &str) {
        let msg = OscMessage {
            addr: crate::host::GUI_DEF.into(),
            args: vec![OscType::Int(id), OscType::String(json.to_string())],
        };
        match encode(&OscPacket::Message(msg)) {
            Ok(bytes) => self.feed(&bytes),
            Err(e) => log(&format!("cannot encode /gui_def: {e}")),
        }
    }

    /// Pops the next outbound OSC packet (`/gui_event`/`/gui_closed`/`/gui_info`)
    /// for the page to decode, or `undefined` when the queue is empty.
    pub fn poll(&self) -> Option<Vec<u8>> {
        self.outbox.borrow_mut().pop_front()
    }

    /// Attaches the host's audio-server leg to a `--ws` server `url`, so a bound
    /// widget forwards straight to it (the bypass path, in the browser).
    pub fn connect_server(&self, url: &str) {
        self.send(WebEvent::ConnectServer(url.to_string()));
    }

    /// Attaches the host's audio-server leg to the **in-page engine**: every
    /// outbound OSC packet (bound-widget values, `/bus_stream`/`/bus_tapStream`
    /// subscriptions, buffer fetches, `/clock_query`) is handed to `send` as a
    /// `Uint8Array`; the page forwards it to the engine and feeds the engine's
    /// replies back through [`server_reply`](Self::server_reply).
    pub fn connect_page(&self, send: js_sys::Function) {
        self.send(WebEvent::ConnectPage(send));
    }

    /// Overlays the host's color theme from a JSON object of
    /// `{"role": "#rrggbb[aa]"}` entries — the browser form of the native
    /// `[gui.theme]` config table. A partial object is fine; unknown roles or
    /// bad colors are logged and skipped.
    pub fn theme(&self, json: &str) {
        match serde_json::from_str::<std::collections::BTreeMap<String, String>>(json) {
            Ok(table) => {
                self.send(WebEvent::Theme(table.into_iter().collect()));
            }
            Err(e) => log(&format!("cannot parse theme JSON: {e}")),
        }
    }

    /// Overlays the host's size metrics from a JSON object of
    /// `{"role": number}` entries — the browser form of the native
    /// `[gui.metrics]` config table, the reserved `scale` density key included.
    /// A partial object is fine; unknown roles or unusable numbers are logged
    /// and skipped.
    pub fn metrics(&self, json: &str) {
        match serde_json::from_str::<std::collections::BTreeMap<String, f64>>(json) {
            Ok(table) => {
                self.send(WebEvent::Metrics(table.into_iter().collect()));
            }
            Err(e) => log(&format!("cannot parse metrics JSON: {e}")),
        }
    }

    /// Draws the host's windows with `samples`x multisampling — the browser
    /// form of the native `[gui] msaa` / `--msaa`, and the same bounded
    /// capability: `1` (the default) draws the flat picture, a higher count
    /// smooths every edge in the pass at the cost of one multisampled
    /// attachment per canvas. A count the GPU does not offer for the surface
    /// format falls back to `1` with a message.
    ///
    /// It applies to canvases attached **after** it, since every pipeline in a
    /// pass agrees on the count: call it before mounting, and re-attach a
    /// canvas to change it.
    pub fn msaa(&self, samples: u32) {
        self.send(WebEvent::Msaa(samples));
    }

    /// Draws text with the typeface in `bytes` — a TrueType/OpenType face the
    /// page fetched, which is the browser's half of the host's font seam (a
    /// native host maps a file instead).
    ///
    /// Only a bundle built with the `font-atlas` feature carries a rasterizer;
    /// any other logs and keeps drawing with the embedded bitmap face, which is
    /// also what happens if the bytes are not a readable face. Loading one
    /// relayouts nothing — a size table never followed the typeface — so it may
    /// be called at any point, before or after the first `/gui_def`.
    pub fn font(&self, bytes: &[u8]) {
        #[cfg(feature = "font-atlas")]
        self.send(WebEvent::Face(bytes.to_vec()));
        #[cfg(not(feature = "font-atlas"))]
        {
            let _ = bytes;
            log(
                "this host was built without a rasterizer (the `font-atlas` feature); \
                 drawing with the embedded bitmap face",
            );
        }
    }

    /// Feeds one reply packet from the in-page engine (a streamed `/bus_stream.reply`, a
    /// `/bus_tapStream.reply`, a `/buffer_query.reply`/`/buffer_getRange.reply`, a `/clock_query.reply`) into the host —
    /// the inbound half of [`connect_page`](Self::connect_page), the same
    /// dispatch the WS leg's `onmessage` uses.
    pub fn server_reply(&self, packet: &[u8]) {
        self.send(WebEvent::ServerInbound(packet.to_vec()));
    }

    /// Closes this host: its canvases, GPU slots, tick and audio-server leg go,
    /// and the page's other instances carry on.
    ///
    /// A page that holds one host for as long as it lives never needs this —
    /// which is why nothing called it while a page could hold only one. A
    /// caller that opens hosts over time does: an abandoned instance keeps its
    /// WebSocket open, its `setInterval` running and its GPU surfaces alive,
    /// none of which the loop will collect on its own.
    ///
    /// Sending through the bridge afterwards is harmless and does nothing.
    pub fn close(&self) {
        let _ = self.proxy.send_event(HostEvent::Remove(self.id));
    }
}

/// The ordered boot packets of a persisted bundle, for the page to send to the
/// in-page engine: `synthdefs`/`graphdefs` are arrays of `Uint8Array` (each
/// file's bytes verbatim), `boot_json` the optional `boot.json` text,
/// `guidef_tree` the GuiDef tree JSON (its root `boot` messages run last).
/// Returns an array of `Uint8Array` packets ending in `/server_sync sync_id+1` — the
/// page knows the bundle is up when `/server_sync.reply sync_id+1` comes back. The
/// ordering/encoding logic lives in the platform-agnostic `host::bundle`
/// module, natively unit-tested.
#[wasm_bindgen]
pub fn bundle_boot_packets(
    synthdefs: js_sys::Array,
    graphdefs: js_sys::Array,
    boot_json: Option<String>,
    guidef_tree: &str,
    sync_id: i32,
) -> js_sys::Array {
    let to_bytes = |array: js_sys::Array| -> Vec<Vec<u8>> {
        array
            .iter()
            .map(|v| js_sys::Uint8Array::new(&v).to_vec())
            .collect()
    };
    let packets = crate::host::bundle::boot_packets(
        &to_bytes(synthdefs),
        &to_bytes(graphdefs),
        boot_json.as_ref().map(|s| s.as_bytes()),
        guidef_tree.as_bytes(),
        sync_id,
    );
    packets
        .into_iter()
        .map(|bytes| js_sys::Uint8Array::from(bytes.as_slice()))
        .collect()
}

/// The wasm entry point: **one host instance**, and the page's event loop under
/// the first of them.
///
/// The first call builds the loop and spawns the app on the browser's
/// animation-frame loop (returning immediately, nothing blocks the main
/// thread); every later call adds an instance to the app already running. A
/// page that calls this once — which is every served page — behaves exactly as
/// before and needs to know none of it.
///
/// **Instances share nothing.** Each has its own widget-id space, its own
/// audio-server leg, its own canvases and its own streamed data, so two hosts
/// in one document are as independent as two documents — no id range has to be
/// partitioned between them. What they do share is the event loop, because
/// winit allows a page exactly one (a second `EventLoop` is
/// `RecreationAttempt`, a panic inside the wasm), and the wasm module itself,
/// so the second instance costs neither a download nor a GPU device.
///
/// Close one with [`GuiBridge::close`] when it outlives its purpose; a page
/// that keeps its host until it unloads need not.
#[wasm_bindgen]
pub fn start() -> GuiBridge {
    console_error_panic_hook::set_once();
    let id = HostId(NEXT_HOST.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    }));
    let outbox = Rc::new(RefCell::new(VecDeque::new()));

    // The loop already runs: this is an additional instance, and it joins the
    // set the way every other message travels, through the proxy.
    if let Some(proxy) = web_proxy() {
        let _ = proxy.send_event(HostEvent::Add {
            id,
            outbox: outbox.clone(),
        });
        return GuiBridge { id, proxy, outbox };
    }

    let event_loop = EventLoop::<HostEvent>::with_user_event()
        .build()
        .expect("build the web event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    WEB_PROXY.with(|p| *p.borrow_mut() = Some(proxy.clone()));
    let bridge = GuiBridge {
        id,
        proxy,
        outbox: outbox.clone(),
    };
    log("clausters-gui web host starting");
    event_loop.spawn_app(WebHosts::new(id, outbox));
    bridge
}
