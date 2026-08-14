//! The browser entry point: a live GUI host driven over the binding surface and
//! WebSocket.
//!
//! This is the wasm twin of the native windowed front (`gui`, which this build
//! does not compile). It runs
//! the **real** [`Host`] (the same protocol dispatch, tree, bindings and
//! `forward`), renders through the shared [`super::frame::render`], and handles
//! pointer interaction through the shared [`super::interact`] primitives — so a
//! browser window opens, updates and emits events exactly as the desktop does.
//! Only the carrier and the page glue are new:
//!
//! - a **binding surface** ([`GuiBridge`]) the in-page JS feeds OSC packets into
//!   (a `/gui_def`, `/gui_set`, `/gui_bind`, …) and drains `/gui_event`/
//!   `/gui_closed`/`/gui_info` out of, all as raw OSC bytes through the one
//!   [`decode_packet`]/encode door;
//! - a [`WsServerLink`]: the host's audio-server leg over the browser-native
//!   `WebSocket` to a `--ws` server, so a bound widget bypasses the script in the
//!   browser too.
//!
//! There is no in-process engine in the browser (that is native-only); the
//! browser always drives a *separate* `--ws` audio server.

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use clausters_core::osc::{OscMessage, OscPacket, OscType, decode_packet, encode};
use wasm_bindgen::prelude::*;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

use crate::gpu::Gpu;
use crate::peaks::MultiPyramid;
use crate::spectrogram::Stft;
use crate::view::Renderers;
use crate::waveform::WaveformData;

use super::fetch::{BufferFetches, FetchStep};
use super::frame::{self, SpectrogramSlot, WaveformSlot};
use super::gestures::{ClipVerb, GestureCtx, GestureEffect, Gestures};
use super::live::{self, StreamedBuses, StreamedTaps};
use super::paint::Painter;
use super::widget::Widget;
use super::widget::element::{Key as HostKey, Live, Loaded, SlotKind};
use super::{BusSource, ClientId, GUI_EVENT, Host, HostEffect, ServerLink};

mod bridge;
mod bulk;
mod canvas;
mod input;
mod serverleg;

pub use bridge::{GuiBridge, bundle_boot_packets, start};
use canvas::{CanvasSlot, WindowRender};
pub use serverleg::{PageServerLink, WsServerLink};

/// The default canvas size; a fed GuiDef lays out into it (the layout uses the
/// framebuffer size, not the def's declared `w`/`h`).
const CANVAS_SIZE: (u32, u32) = (480, 420);

/// Logs a line to the browser console (a full `tracing` wasm shim is out of scope).
fn log(msg: &str) {
    web_sys::console::log_1(&msg.into());
}

/// Writes a status line into the page's `#note` element (if present), so a
/// failure the user must act on (no GPU adapter at all) is visible without the
/// console.
fn set_status(msg: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("note"))
    {
        el.set_text_content(Some(msg));
    }
}

/// Which host instance on this page an event is for.
///
/// A page holds one host by default and one is what a served page ever asks
/// for, but the count is not a property of the page: it is one per caller of
/// [`start`]. See [`WebHosts`] for why the instance — and not the page — is the
/// unit that owns a widget-id space and an audio-server leg.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct HostId(u32);

impl std::fmt::Display for HostId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "host {}", self.0)
    }
}

/// One [`WebEvent`] addressed to its instance, plus the two that manage the set
/// itself.
///
/// The event loop owns the instances, so a new one cannot be inserted by a
/// call: it arrives as `Add`, like everything else. That costs nothing
/// observable — every [`GuiBridge`] method already goes through the proxy, so a
/// packet sent immediately after [`start`] queues behind the `Add` in order.
enum HostEvent {
    /// Take a new instance into the set, with the outbox its bridge drains.
    Add {
        id: HostId,
        outbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
    },
    /// Drop an instance: its canvases, its GPU slots, its tick and its
    /// audio-server leg go with it.
    Remove(HostId),
    /// One event for one instance.
    To(HostId, WebEvent),
}

/// What the binding surface and the async GPU hand the running app, through the
/// winit web event loop's proxy (single-threaded, so the non-`Send` `Gpu` and
/// the byte buffers move freely).
enum WebEvent {
    /// Give this def a canvas of its own. `canvas` is the element the page
    /// created for it; `None` asks winit to append one to `<body>`, which is
    /// what a page that never attaches anything gets.
    Attach {
        def_id: i32,
        canvas: Option<web_sys::HtmlCanvasElement>,
    },
    /// Drop this def's canvas: its surface, its GPU slots and its live state go.
    Detach(i32),
    /// The element's box changed size, in device pixels, with the ratio that
    /// produced them (an `ResizeObserver` box and `devicePixelRatio`): the page
    /// reports both, because the product alone cannot be undone and the host
    /// needs the ratio to resolve its logical sizes.
    Resize {
        def_id: i32,
        width: u32,
        height: u32,
        scale: f32,
    },
    /// The element entered or left the viewport (`IntersectionObserver`).
    SetVisible { def_id: i32, visible: bool },
    /// The async WebGPU device for one canvas is ready.
    GpuReady { def_id: i32, gpu: Gpu },
    /// One inbound OSC packet from the in-page binding surface (a `/gui_*`).
    Inbound(Vec<u8>),
    /// Attach the audio-server leg to this `--ws` URL (for bound widgets).
    ConnectServer(String),
    /// Attach the audio-server leg to the in-page engine: outbound packets go
    /// to this page-registered callback (replies via `server_reply`).
    ConnectPage(js_sys::Function),
    /// One inbound OSC packet from the audio server over the WS leg (a streamed
    /// `/bus_stream.reply`, a `/buffer_query.reply`/`/buffer_getRange.reply` reply, a `/fail`).
    ServerInbound(Vec<u8>),
    /// The animation tick (a `setInterval` at ~30 fps while the window has live
    /// widgets): advance the scope histories and repaint.
    Tick,
    /// A `fetch` of a waveform/plot URL completed and decoded (the browser's
    /// bulk path: `path`/`cache` resolve against the page origin).
    BulkReady {
        def_id: i32,
        widget_id: i32,
        data: Loaded,
    },
    /// A theme overlay from the page: role -> "#rrggbb\[aa\]" pairs (the same
    /// table `[gui.theme]` and a `--theme` file carry natively).
    Theme(Vec<(String, String)>),
    /// A metrics overlay from the page: role -> number pairs (the same table
    /// `[gui.metrics]` carries natively, `scale` included).
    Metrics(Vec<(String, f64)>),
    /// The MSAA sample count canvases attached from here on are drawn with
    /// (the browser form of the native `[gui] msaa`).
    Msaa(u32),
    /// The bytes of a typeface the page fetched — the browser's half of the
    /// [`FontSource`](crate::host::FontSource) seam, which a native host fills
    /// by mapping a file.
    #[cfg(feature = "font-atlas")]
    Face(Vec<u8>),
}

/// A typeface the page fetched, as the platform seam sees it: bytes that came
/// from somewhere this core does not name.
#[cfg(feature = "font-atlas")]
pub struct FetchedFace(pub Vec<u8>);

#[cfg(feature = "font-atlas")]
impl crate::host::FontSource for FetchedFace {
    fn face(&self) -> Option<Vec<u8>> {
        (!self.0.is_empty()).then(|| self.0.clone())
    }
}

/// The browser host application: the live [`Host`], one [`CanvasSlot`] per
/// `window`-rooted def, and the shared outbox the binding surface drains.
struct WebApp {
    /// Which instance this is, for the events its own closures send back
    /// (`Gpu::new`'s hand-off, the tick, a bulk fetch, the WS leg's
    /// `onmessage`) — all four are built inside this struct's methods, so the
    /// id is always at hand where a proxy is taken.
    id: HostId,
    host: Host,
    outbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
    /// One canvas per `window`-rooted def, keyed by its def id.
    canvases: HashMap<i32, CanvasSlot>,
    /// The reverse index winit's per-window events route through.
    by_winit: HashMap<WindowId, i32>,
    /// Whether the event loop resumed — a window can only be created after it,
    /// so an `attach` that arrives first waits here.
    resumed: bool,
    pending_attach: Vec<(i32, Option<web_sys::HtmlCanvasElement>)>,
    /// The host-wide clipboard (Ctrl+C/X/V), page-wide. An in-page clipboard
    /// like the native front's; binding it to the browser's OS clipboard (a
    /// `writeText` out plus a `paste`-event listener in) is a later refinement,
    /// and the typed clipboard is what makes that binding a matter of a string
    /// crossing rather than of a format: text is one of its kinds.
    text_clipboard: crate::host::clipboard::Clip,
    /// Live control-bus values streamed from the audio server (`/bus_stream` →
    /// `/bus_stream.reply`), the browser's [`BusSource`] for meters/scopes/canvases.
    buses: Arc<StreamedBuses>,
    /// The bus set currently subscribed with `/bus_stream` (sorted), so a tree
    /// change only resubscribes when the set actually changed.
    streamed: Vec<i32>,
    /// The newest `/bus_tapStream.reply` window per tap — the browser's source for
    /// audio-rate scopes, read on the tick exactly as the native front reads
    /// the segment's tap rings.
    taps: Arc<StreamedTaps>,
    /// The `(taps, window frames)` currently subscribed with `/bus_tapStream`,
    /// so a tree change only resubscribes when they actually changed.
    tap_streamed: (Vec<i32>, usize),
    /// The server's sample rate (from `/clock_query.reply`, requested when the leg
    /// connects); `0.0` until known — window sizing then assumes 48 kHz.
    server_rate: f64,
    /// The engine's sample clock from the newest `/clock_query.reply` — the browser
    /// playhead source (polled once per tick while a playhead is shown; the
    /// native front reads the shm header instead).
    server_clock: f64,
    /// The animation tick: the `setInterval` id and its closure, kept alive
    /// while the current def has live widgets (meter/scope/canvas).
    tick: Option<(i32, Closure<dyn FnMut()>)>,
    /// Whether the first streamed `/bus_stream.reply` snapshot was logged (one line as
    /// evidence the bus stream is flowing; logging every frame would spam).
    stream_seen: bool,
    /// The server-buffer fetch machine (`/buffer_query` → chunked `/buffer_getRange`),
    /// shared with the native front; requests ride the WS leg.
    fetches: BufferFetches,
}

impl WebApp {
    fn new(id: HostId, outbox: Rc<RefCell<VecDeque<Vec<u8>>>>) -> Self {
        Self {
            id,
            host: Host::new(),
            outbox,
            canvases: HashMap::new(),
            by_winit: HashMap::new(),
            resumed: false,
            pending_attach: Vec::new(),
            text_clipboard: crate::host::clipboard::Clip::default(),
            buses: Arc::new(StreamedBuses::default()),
            streamed: Vec::new(),
            taps: Arc::new(StreamedTaps::default()),
            tap_streamed: (Vec::new(), 0),
            server_rate: 0.0,
            server_clock: 0.0,
            tick: None,
            stream_seen: false,
            fetches: BufferFetches::default(),
        }
    }

    /// Handles one inbound OSC packet (from the binding surface) through the real
    /// protocol dispatch, then carries out the effects: open/redraw the window,
    /// queue replies for the page to drain.
    fn on_inbound(&mut self, event_loop: &ActiveEventLoop, bytes: &[u8]) {
        let packet = match decode_packet(bytes) {
            Ok(p) => p,
            Err(e) => return log(&format!("malformed OSC packet from the page: {e}")),
        };
        for effect in self.host.handle_packet(packet, ClientId::Web) {
            match effect {
                HostEffect::Reply(msg) => self.queue(msg),
                HostEffect::OpenWindow(id) => {
                    log(&format!("/gui_def {id}: opened from the page"));
                    // A page that fed a `/gui_def` without attaching a canvas
                    // gets one appended, as the single-canvas host always did.
                    if !self.canvases.contains_key(&id) {
                        self.attach(event_loop, id, None);
                    }
                    if let Some(slot) = self.canvases.get_mut(&id) {
                        slot.clear_def_state();
                    }
                    self.fetches.drop_def(id); // rebuild semantics on a re-/gui_def
                    self.build_resources(id);
                    self.request_redraw(id);
                    self.start_bulk(id);
                    self.on_tree_changed();
                }
                HostEffect::CloseWindow(id) => {
                    if let Some(slot) = self.canvases.get_mut(&id) {
                        slot.clear_def_state();
                        slot.request_redraw();
                    }
                    self.fetches.drop_def(id);
                    self.on_tree_changed();
                }
                HostEffect::Redraw(id) => {
                    if self.canvases.contains_key(&id) {
                        // A `/gui_set` may have retargeted a meter/scope `bus`:
                        // re-derive the subscription (a no-op when unchanged).
                        self.on_tree_changed();
                        self.request_redraw(id);
                    }
                }
            }
        }
    }

    /// The defs currently drawing: one canvas each, and in the viewport.
    ///
    /// Everything that costs something per frame or per packet — the tick, the
    /// `/bus_stream` and `/bus_tapStream` subscriptions — is derived from exactly
    /// this set, which is what makes a scrolled-away component free.
    fn visible_defs(&self) -> Vec<i32> {
        let mut ids: Vec<i32> = self
            .canvases
            .iter()
            .filter(|(_, slot)| slot.visible)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// A freshly attached server leg (WS or in-page) holds no subscription:
    /// forget the old ones and subscribe the current tree's buses and taps
    /// (WS frames queue until the socket opens, so sending now is safe).
    /// `/clock_query` fetches the rate the oscilloscope windows are sized with.
    fn on_server_attached(&mut self) {
        self.streamed.clear();
        self.tap_streamed = (Vec::new(), 0);
        if let Some(server) = self.host.server() {
            let _ = server.send(OscMessage {
                addr: "/clock_query".into(),
                args: vec![],
            });
        }
        self.on_tree_changed();
    }

    /// Re-derives everything that follows from the current tree's live widgets:
    /// the `/bus_stream` and `/bus_tapStream` subscriptions on the WS leg and the
    /// animation tick. Called on open/close/redraw and after the server leg
    /// attaches; cheap (a tree walk) and idempotent, so calling it eagerly is
    /// fine.
    fn on_tree_changed(&mut self) {
        let demand = self.demand();
        self.sync_bus_stream(demand.buses);
        self.sync_tap_stream(demand.taps, demand.tap_frames);
        self.ensure_tick(demand.animated);
    }

    /// What the drawing canvases ask of the server and of the frame clock —
    /// the union over the **visible** set, so a scrolled-away component drops
    /// out of it. The derivation itself is platform-agnostic
    /// ([`live::demand`]), natively tested.
    fn demand(&self) -> live::LiveDemand {
        let trees: Vec<&Widget> = self
            .visible_defs()
            .into_iter()
            .filter_map(|def| self.host.window_def(def))
            .collect();
        live::demand(trees, self.host.timelines(), self.server_rate)
    }

    /// Starts or stops the ~30 fps animation tick to match the drawing
    /// canvases: running while any has live widgets (meter/scope/canvas),
    /// stopped otherwise. The tick advances the scope histories and repaints —
    /// the browser twin of the native `about_to_wait` frame timer, driven by
    /// `setInterval` because `std::time::Instant` does not exist on wasm.
    fn ensure_tick(&mut self, animated: bool) {
        let Some(window) = web_sys::window() else {
            return;
        };
        match (animated, self.tick.is_some()) {
            (true, false) => {
                let host = self.id;
                let closure = Closure::<dyn FnMut()>::new(move || {
                    if let Some(proxy) = web_proxy() {
                        let _ = proxy.send_event(HostEvent::To(host, WebEvent::Tick));
                    }
                });
                match window.set_interval_with_callback_and_timeout_and_arguments_0(
                    closure.as_ref().unchecked_ref(),
                    live::STREAM_PERIOD_MS,
                ) {
                    Ok(id) => self.tick = Some((id, closure)),
                    Err(e) => log(&format!("cannot start the animation tick: {e:?}")),
                }
            }
            (false, true) => {
                if let Some((id, _closure)) = self.tick.take() {
                    window.clear_interval_with_handle(id);
                }
            }
            _ => {}
        }
    }

    /// Registers what a fill turned out to be worth with the widget's
    /// navigation axis, which has to know how long it is or the visible window
    /// falls back to a span the size of the body in *samples* and the whole
    /// picture draws as one stretched column.
    ///
    /// A **rolling** extent goes through the live setter: a retained axis
    /// slides, so it follows the newest column until someone navigates it and
    /// then holds where they left it.
    pub(super) fn apply_extents(&mut self, extents: Vec<(i32, frame::Extent)>) {
        for (id, extent) in extents {
            match extent {
                frame::Extent::Stored(total) => self.host.set_timeline_total(id, total),
                frame::Extent::Rolling(total) => self.host.set_live_timeline_total(id, total),
            }
        }
    }

    /// One animation tick: push a fresh streamed-bus sample into every scope's
    /// rolling history and refresh the audio-rate scopes' triggered windows
    /// from the `/bus_tapStream.reply` store (time-based, exactly like the native tick),
    /// then repaint.
    fn on_tick(&mut self) {
        let mut wants_clock = false;
        // Applied after the loop: registering an axis' length borrows the
        // host, which the loop holds a tree of.
        let mut extents: Vec<(i32, frame::Extent)> = Vec::new();
        for def in self.visible_defs() {
            let Some(slot) = self.canvases.get_mut(&def) else {
                continue;
            };
            let Some(tree) = self.host.window_def_mut(def) else {
                continue;
            };
            let source = live::StreamedSource {
                buses: self.buses.clone(),
                taps: self.taps.clone(),
            };
            live::update_retention(
                tree,
                self.server_rate,
                live::retention_window(self.server_rate, 0),
                |tap, out| source.read_bus_at(tap, out),
                &mut slot.histories,
            );
            live::tick_tree(
                tree,
                &Live {
                    bus: Some(&source),
                    sample_rate: self.server_rate,
                    histories: &slot.histories,
                },
            );
            extents.extend(refresh_slots(slot, tree));
            slot.request_redraw();
            // Asked after the tick, so the mutable walk above is over: whether
            // this tree draws a moving playhead is what makes the page poll the
            // engine clock at all.
            if let Some(tree) = self.host.window_def(def) {
                wants_clock |= live::tree_has_playhead(tree, self.host.timelines());
            }
        }
        self.apply_extents(extents);
        // A visible playhead needs the engine clock: poll it once per tick (the
        // browser's stand-in for the shm header's sample clock) — once for the
        // page, however many canvases show one.
        if wants_clock && let Some(server) = self.host.server() {
            let _ = server.send(OscMessage {
                addr: "/clock_query".into(),
                args: vec![],
            });
        }
        self.advance_edge_scroll();
    }

    /// The frame step of a clip drag held against a lane's edge: pans the view
    /// and carries the clip with it. The tick's own period is the `dt`, so the
    /// scroll runs at the same speed whatever else the frame is doing.
    fn advance_edge_scroll(&mut self) {
        let dt = f64::from(live::STREAM_PERIOD_MS) / 1000.0;
        let dragging: Vec<i32> = self
            .canvases
            .iter()
            .filter(|(_, slot)| slot.gestures.edge_scrolling(slot.cursor.0))
            .map(|(def, _)| *def)
            .collect();
        for def in dragging {
            let Some((ctx, (cx, cy))) = self.gesture_ctx(def) else {
                continue;
            };
            let Some(slot) = self.canvases.get_mut(&def) else {
                continue;
            };
            let effects = slot.gestures.tick(&mut self.host, &ctx, cx, cy, dt);
            self.apply_gesture_effects(effects);
        }
    }
}

/// The instance's own half of the loop's callbacks. [`WebHosts`] is the
/// [`ApplicationHandler`]: winit takes one, and a page holds several of these.
impl WebApp {
    /// Nothing opens on its own: a canvas exists because the page attached one
    /// (or because a `/gui_def` arrived without one). This only unblocks the
    /// attaches that raced ahead of the loop.
    fn on_resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resumed = true;
        for (def_id, canvas) in std::mem::take(&mut self.pending_attach) {
            self.attach(event_loop, def_id, canvas);
        }
    }

    fn on_user_event(&mut self, event_loop: &ActiveEventLoop, event: WebEvent) {
        match event {
            WebEvent::Attach { def_id, canvas } => self.attach(event_loop, def_id, canvas),
            WebEvent::Detach(def_id) => self.detach(def_id),
            WebEvent::Resize {
                def_id,
                width,
                height,
                scale,
            } => {
                // The page's device-pixel ratio is this canvas' UI scale: the
                // wire's logical sizes resolve against it, once per change.
                self.host.set_ui_scale(def_id, scale);
                let Some(slot) = self.canvases.get_mut(&def_id) else {
                    return;
                };
                // The element owns the box; the host is only told the pixels.
                // winit writes the canvas' backing size, and the surface follows
                // right here rather than waiting for the `Resized` echo.
                let _ = slot
                    .window
                    .request_inner_size(PhysicalSize::new(width, height));
                match slot.render.as_mut() {
                    Some(render) => render.gpu.resize(width, height),
                    None => slot.pending_size = Some((width, height)),
                }
                slot.request_redraw();
            }
            WebEvent::SetVisible { def_id, visible } => {
                let Some(slot) = self.canvases.get_mut(&def_id) else {
                    return;
                };
                if slot.visible == visible {
                    return;
                }
                slot.visible = visible;
                if visible {
                    slot.request_redraw();
                }
                // The subscriptions and the tick follow the visible set: a
                // canvas out of the viewport stops costing server CPU and wire.
                self.on_tree_changed();
            }
            WebEvent::GpuReady { def_id, mut gpu } => {
                // On the web the `<canvas>` is often not laid out yet when
                // `Gpu::new` reads its size (the size is captured before the async
                // adapter/device awaits), so the surface can come up configured to
                // a stale 1x1. Re-read the now-laid-out size and reconfigure before
                // the first frame — otherwise the clear fills the canvas (a gray
                // backdrop) but every widget lays out into a ~0 px area and nothing
                // visible is drawn. A `Resized` that arrived while the GPU was
                // pending was stashed in `pending_size`; prefer the live size and
                // fall back to it.
                let Some(slot) = self.canvases.get_mut(&def_id) else {
                    return; // detached while the device was coming up
                };
                let size = gpu.window.inner_size();
                let (w, h) = if size.width > 0 && size.height > 0 {
                    (size.width, size.height)
                } else {
                    slot.pending_size.unwrap_or((size.width, size.height))
                };
                gpu.resize(w, h);
                let renderers = Renderers::new(&gpu.device, gpu.target());
                let painter = Painter::new(&gpu.device, gpu.target());
                let overlay = Painter::new(&gpu.device, gpu.target());
                log(&format!(
                    "def {def_id}: GPU device ready; surface {}x{}",
                    gpu.config.width, gpu.config.height
                ));
                slot.render = Some(WindowRender {
                    gpu,
                    renderers,
                    painter,
                    overlay,
                    waveforms: HashMap::new(),
                    spectrograms: HashMap::new(),
                });
                let pending = std::mem::take(&mut slot.pending_bulk);
                self.build_resources(def_id);
                // Bulk data that finished downloading while the device was
                // still coming up gets its GPU slots now.
                for (widget_id, data) in pending {
                    self.place_bulk(def_id, widget_id, data);
                }
                self.request_redraw(def_id);
            }
            WebEvent::Inbound(bytes) => self.on_inbound(event_loop, &bytes),
            WebEvent::ConnectServer(url) => match WsServerLink::connect(&url, self.id) {
                Ok(link) => {
                    self.host.set_server_link(ServerLink::Ws(link));
                    log(&format!("audio-server leg connecting to {url}"));
                    self.on_server_attached();
                }
                Err(e) => log(&format!("cannot open audio-server WebSocket {url}: {e}")),
            },
            WebEvent::ConnectPage(callback) => {
                self.host
                    .set_server_link(ServerLink::Page(PageServerLink { callback }));
                log("audio-server leg attached to the in-page engine");
                self.on_server_attached();
            }
            WebEvent::ServerInbound(bytes) => self.on_server_inbound(&bytes),
            WebEvent::Tick => self.on_tick(),
            WebEvent::BulkReady {
                def_id,
                widget_id,
                data,
            } => self.on_bulk_ready(def_id, widget_id, data),
            WebEvent::Theme(entries) => {
                for w in self
                    .host
                    .theme
                    .overlay(entries.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                {
                    log(&w);
                }
                // The base changed under the resolved references: re-resolve
                // every window's theme groups over the new host theme.
                let base = std::sync::Arc::new(self.host.theme.clone());
                for id in self.host.window_def_ids() {
                    if let Some(tree) = self.host.window_def_mut(id) {
                        super::widget::resolve_style(tree, &base);
                    }
                }
                for def in self.canvases.keys().copied().collect::<Vec<_>>() {
                    self.draw(def);
                }
            }
            WebEvent::Msaa(samples) => {
                // A canvas already showing keeps the pass its pipelines were
                // built against, exactly as a native window does: the count is
                // read when a device comes up, and re-attaching applies it.
                self.host.msaa = samples.max(1);
            }
            WebEvent::Metrics(entries) => {
                for w in self
                    .host
                    .metrics
                    .overlay(entries.iter().map(|(k, v)| (k.as_str(), *v)))
                {
                    log(&w);
                }
                // Every canvas re-resolves the new roles at its own scale, and
                // sizes are then read per frame from that one table, so a
                // redraw is the rest of the update.
                self.host.refresh_metrics();
                for def in self.canvases.keys().copied().collect::<Vec<_>>() {
                    self.draw(def);
                }
            }
            #[cfg(feature = "font-atlas")]
            WebEvent::Face(bytes) => {
                if self.host.load_face(&FetchedFace(bytes)) {
                    // Nothing was measured differently until now, so a redraw
                    // is the whole update: no size table moved, and no window
                    // was laid out against the face.
                    for def in self.canvases.keys().copied().collect::<Vec<_>>() {
                        self.draw(def);
                    }
                } else {
                    log("those bytes are not a typeface this host can read");
                }
            }
        }
    }
}

/// The page's host instances behind winit's one [`ApplicationHandler`].
///
/// **The event loop is the only thing a page can hold just one of.** winit
/// refuses a second `EventLoop` — `RecreationAttempt`, a panic inside the wasm
/// rather than an error a caller could catch — but it drives any number of
/// windows, which is already how one instance serves a document's canvases. So
/// the loop is built once and memoized in [`WEB_PROXY`], and [`start`] adds an
/// instance to this set rather than starting anything.
///
/// Everything a host *is* lives in [`WebApp`]: its `Host` (and therefore its
/// widget-id space), its audio-server leg, its canvases, buses, taps, tick and
/// fetches. Nothing here is shared, which is the point — two instances are as
/// independent as two pages, and neither has to partition an id range against
/// the other. The GPU was already per canvas (`Gpu::new` builds one per
/// `CanvasSlot`), so instances add no devices.
struct WebHosts {
    apps: HashMap<HostId, WebApp>,
    /// Whether the loop resumed. A window can only be created after it, and an
    /// instance added later has missed the callback — so it is remembered here
    /// and handed on, or its first canvas would wait for a `resumed` that
    /// already happened.
    resumed: bool,
}

impl WebHosts {
    fn new(id: HostId, outbox: Rc<RefCell<VecDeque<Vec<u8>>>>) -> Self {
        let mut apps = HashMap::new();
        apps.insert(id, WebApp::new(id, outbox));
        Self {
            apps,
            resumed: false,
        }
    }
}

impl ApplicationHandler<HostEvent> for WebHosts {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.resumed = true;
        for app in self.apps.values_mut() {
            app.on_resumed(event_loop);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: HostEvent) {
        match event {
            HostEvent::Add { id, outbox } => {
                let mut app = WebApp::new(id, outbox);
                if self.resumed {
                    app.on_resumed(event_loop);
                }
                self.apps.insert(id, app);
                log(&format!("{id} added ({} on this page)", self.apps.len()));
            }
            HostEvent::Remove(id) => {
                if self.apps.remove(&id).is_some() {
                    log(&format!("{id} closed ({} left)", self.apps.len()));
                }
            }
            HostEvent::To(id, event) => match self.apps.get_mut(&id) {
                Some(app) => app.on_user_event(event_loop, event),
                // A packet for an instance that was closed while it was in
                // flight. Dropping it is the whole handling: the canvases it
                // would have drawn into are gone with it.
                None => log(&format!("event for {id}, which is closed")),
            },
        }
    }

    /// winit addresses a window, not an instance, so the owner is whoever
    /// claims the id. The set is a page's worth of hosts — units, not
    /// thousands — and asking them is what keeps the routing correct across
    /// every attach and detach without a second index to maintain.
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let owner = self
            .apps
            .iter()
            .find(|(_, app)| app.owns(id))
            .map(|(host, _)| *host);
        if let Some(app) = owner.and_then(|host| self.apps.get_mut(&host)) {
            app.on_window_event(id, event);
        }
    }
}

thread_local! {
    /// The page's one event-loop proxy, so an instance's own closures can reach
    /// the loop for the async GPU hand-off, the tick and the bulk fetches
    /// (winit's web loop is single-threaded). Shared by every instance —
    /// each of its events carries the [`HostId`] it is for.
    ///
    /// It doubles as the record that the loop exists: `Some` means [`start`]
    /// already built it, and the next call adds an instance instead.
    static WEB_PROXY: RefCell<Option<EventLoopProxy<HostEvent>>> = const { RefCell::new(None) };
    /// The source of instance ids, page-wide.
    static NEXT_HOST: Cell<u32> = const { Cell::new(0) };
}

/// The proxy every instance-side closure reaches the loop through.
fn web_proxy() -> Option<EventLoopProxy<HostEvent>> {
    WEB_PROXY.with(|p| p.borrow().clone())
}

/// Uploads whatever this canvas' tree has for its GPU slots this tick — the
/// columns a waterfall just analyzed, the picture an element that got its data
/// rebuilt — and only what moved, so a still page costs no upload.
///
/// The browser twin of the desktop front's own pass, kept beside the tick that
/// advances the elements rather than inside the render, since an upload is not
/// a drawing decision.
fn refresh_slots(slot: &mut CanvasSlot, tree: &mut Widget) -> Vec<(i32, frame::Extent)> {
    let mut extents = Vec::new();
    let Some(render) = slot.render.as_mut() else {
        return extents;
    };
    frame::fill_slots(
        tree,
        None,
        &render.gpu,
        &render.renderers,
        &mut render.waveforms,
        &mut render.spectrograms,
        &mut extents,
    );
    extents
}
