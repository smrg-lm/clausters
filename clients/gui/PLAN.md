# Clausters GUI track - implementation plan

Milestones for the graphical-element system: a scriptable set of GUI widgets driven from a dynamic language (Python now, JavaScript later), covering an audio editor, navigable waveform/spectrogram views, custom instrument panels, a live node-tree view, shader canvases, and - later - editable sequencer and music notation. The design rationale lives in `DESIGN.md`; this file is the staged plan. Like `PLAN.md`/`clients/PLAN.md`, milestone labels (`Gx`) live only here and in `LOG.md`, never in published docs or docstrings.

## What `clausters-gui` is (the naming)

`clausters-gui` is **two roles in one process**, and the plan only makes sense once both are explicit:

- **A GUI client of the audio server.** It talks to `clausters-server` over OSC exactly the way `clausters-python` does - same encoding, same transports (UDP/TCP/shared-memory ring/WebSocket), same `osc::decode_packet` door. It reads buffers, control buses, the node tree, and sends control messages. Anything `clausters-python` can do against the server, `clausters-gui` can do too.
- **A GUI server (host) for the languages.** It owns the windows, the widgets and the GPU, and exposes a widget protocol that the language clients drive. `clausters-python` (and later a JS client) sends it a serialized JSON document - a **GuiDef**, built the same way a `SynthDef`/`GraphDef` is - to construct a window/widget tree, then drives live updates and receives interaction events.

So the topology has three legs:

```
  clausters-python / JS  ──GuiDef + control──▶  clausters-gui  (windows, widgets, GPU)
        │                                            │   ▲
        │                                            │   │ (a widget bound to the server
        └────────────── OSC ─────────────────▶  clausters-server   bypasses the script)
                                                     ▲   │
                                          OSC (gui is also a client)
```

1. **`clausters-python` <-> `clausters-server`** - the existing audio control path, unchanged.
2. **`clausters-python` <-> `clausters-gui`** - the new widget protocol: the script builds windows/widgets with a serialized JSON GuiDef and drives/reads them.
3. **`clausters-gui` <-> `clausters-server`** - the gui acting as an audio-server client (reading buffers/buses/node tree, sending control).

**The script can be bypassed.** When a widget is *bound* to a server destination (`/gui_bind`), its value flows straight from `clausters-gui` to `clausters-server` with no round-trip through `clausters-python` - the low-latency interactive path. Unbound widgets keep emitting events back to the script.

**Standalone is a first-class mode.** Because a GuiDef is a persisted document just like a `GraphDef`, a saved GuiDef can be paired with GraphDefs and run by `clausters-gui` against an embedded audio server - a self-contained application with **no separate language client running at all**.

## Architecture (converged)

The earlier sketch proposed a separate JSON-over-WebSocket protocol with one OSC message per widget. That was wrong on two counts; the corrected model, consistent with the existing server, is:

- **OSC is the single encoding everywhere.** A whole window/widget tree rides **as JSON inside an OSC argument**, exactly as a `SynthDef`/`GraphDef` does today through `/d_recv` (`serde_json::from_slice` over an `OscType::String` or `OscType::Blob`; the server already accepts both - `src/osc/translate.rs`). "JSON vs OSC" is a false dichotomy: JSON is the payload, OSC is the framing. serde's number handling distinguishes integers from floats, so ids stay `i32` and control values stay `f32` across the wire, the same rule the audio server relies on.
- **Construction is declarative; updates are incremental.** A GUI is a *def*: `/gui_def <id> <json tree>` builds the whole tree in one message (like a `SynthDef`), `/gui_set <id> ...` updates one live widget (like `/n_set`), `/gui_free <id>` frees a subtree. No per-widget construction chatter.
- **The GUI host is a sibling OSC front, not code in the audio server.** It reuses the existing transport layer (UDP/TCP/shared-memory ring/WebSocket, the `osc::decode_packet` door, `ClientId`) with a widget/GPU command interpreter in place of the audio engine. The audio server stays headless and RT-safe and is untouched by any of this.
- **The GUI host is also a client of the audio server.** It reads buffers, sends bound-widget values, mirrors the node tree, and - the payoff of the shared-memory IPC - reads the control buses **directly from the shared segment every frame**, so meters/scopes update with zero messages.
- **WebSocket is one more transport, primarily for the browser.** A browser cannot map shared memory or open raw UDP; it can only do WebSocket. The desktop host reuses the existing transports; the browser host uses `WsHub` (already in the core, see G1) - the same OSC encoding through the same decode door, not a new protocol.
- **Heavy widgets are the existing `wgpu` renderers.** The waveform and spectrogram are written once against `wgpu`/WGSL (the `TimelineView` trait) and run natively today and under WebGPU in a browser unchanged.

The widget tree is a hierarchy with client-allocated integer ids and subtree freeing - the shape of a DOM / scene graph (and, incidentally, the shape of the server's group/node tree), reached as a declarative document rather than mutated node by node.

### Crate / packaging boundary

`clausters-gui` is its **own crate**, kept out of the root `clausters` workspace so it can never break the core server build (the `clients/gui` crate already validates this). For end users it ships as a **separate package/binary** from the audio server and the Python client, because the GPU/windowing stack (`wgpu`, `winit`, font/shader assets, later Verovio) is large and should not bloat a headless server install or a `pip` wheel. The two move and version independently.

## Guiding principle: serve the server and its clients, and reuse what exists

Two constraints sit above every milestone:

- **A GUI must never pull focus from the audio server and its clients.** The point of the project is a headless, RT-safe server driven by OSC clients; `clausters-gui` is one more client and one more front, *in service of* that model, not a parallel universe. So the GUI adopts the server's semantics rather than inventing its own (OSC framing, the def model, the node-tree/id-allocation/subtree-free shape, the flat-primitives-at-the-boundary rule, the single `decode_packet` door), the audio server stays untouched and RT-safe, and no GUI convenience is allowed to fork the wire protocol or the client model. When a feature could live in the server/clients or in the GUI, it lives where the rest of the system already keeps it.
- **Stand on the existing crates; do not reimplement.** A large amount of what the GUI needs already exists in the server and the complementary crates and is meant to be reused. The default answer to "where does this behaviour come from?" is an existing module, behind the FFI when it is DSP.

Concrete reuse map (the things already implemented that the GUI builds on):

| Source | What it already provides | How the GUI reuses it |
|---|---|---|
| `clausters-core::osc` | UDP/TCP/WS/shared-ring transports, `decode_packet`, `ClientId`, OSC encode/bundle assembly, `WsHub` | Both legs: the host's server front (script -> gui) and its client leg (gui -> audio server) ride the *same* transports and decode door; no new protocol. |
| `clausters-core` def model (`SynthDef`/`GraphDef`/`d_recv`/defstore) | JSON-in-OSC defs, serde int/float distinction, on-disk persistence | The GuiDef *is* this pattern; `/gui_def` mirrors `/d_recv`, and GuiDef persistence reuses the def-store machinery (the standalone mode, G10). |
| `clausters::embed` + shared-memory ring (the embed ABI) | In-process server, control-bus shared segment, garbage/command FIFOs | Zero-message meters/scopes read the shared buses each frame (G5); the standalone app embeds the server (G10). |
| `clausters-ffi` (C ABI, flat data) | `clausters_ws_*` client, timing/sample conversion, seeded RNG, numeric builtins | The host's audio-server-client leg and any DSP it needs go through the FFI, exactly as `clausters-python` does - one implementation, not two. |
| `clausters-midi` | the MIDI path | Feeds the future `timeline`/DAW sequencing view (G13). |
| Python client (`session.Server`, `responders` `OscFunc`/`MidiFunc`, `ipc`, `_native`) | the reference client model and the encoder over the FFI | The script drives `clausters-gui` the same way it drives the server; the GUI's client leg reuses the client encoder rather than a parallel one; events mirror the responder model. |
| node-tree / group-node semantics (scsynth model) | client-allocated ids, add-actions, subtree free, `/g_query`-style introspection | The widget tree reuses this shape verbatim; the `nodetree` view (G8) reads the server's real tree through its existing query/notification path. |
| `clients/gui` crate renderers (`viewport`/`peaks`/`Stft`/`TimelineView`/`bytes`) | the heavy GPU views and resolution-matched analysis with a cache | The heavy widgets; the analysis is a candidate to migrate behind the FFI so signal code lives once (G7). |

## The OSC vocabulary (generic terminology)

The protocol is deliberately generic - not "windows". The root of a GuiDef may be a window, but it may equally be an embeddable panel, so the address family is `/gui_*`, not `/win_*`. (`/ui_*` is the recorded alternative if `/gui_*` reads worse in practice; the point is *not* a window-specific verb.)

### Commands (script -> gui host)

| Address | Args | Meaning |
|---|---|---|
| `/gui_def` | `id, json` | Build a whole GUI tree (window/panel + widgets) from a JSON GuiDef, in one message. The `SynthDef`/`/d_recv` analogue. |
| `/gui_set` | `id, k, v, ...` | Update live properties of one widget (value, range, label, color, data ref, ...). The `/n_set` analogue. |
| `/gui_bind` | `id, target...` | Bind this widget's value to forward straight to the audio server, bypassing the script (see below). |
| `/gui_free` | `id` | Destroy a widget and its subtree (or a whole def by its root id). The group-free analogue. |
| `/gui_query` | `id` | Request a `/gui_info` reply describing a widget's current state. |
| `/gui_load` | `name` | Load a persisted GuiDef from the def store by name (standalone / saved-app path). |

### Events and replies (gui host -> script)

| Address | Args | Meaning |
|---|---|---|
| `/gui_event` | `id, value...` | A widget was interacted with (knob turned, button pressed, region selected, point dragged). |
| `/gui_info` | `id, type, k, v...` | Reply to `/gui_query`. |
| `/gui_closed` | `id` | A window was closed by the user. |

Property values are OSC primitives (int/float/string/blob) - the binding technology never leaks across the wire, in line with the project's "flat primitives at the boundary" rule.

### Bindings: the value can bypass the script

`/gui_bind` lets a widget's value flow **straight to the audio server** without a round-trip through the script - the same idea already used for MIDI in this project, where a control source is bound to a server-side destination instead of being polled. A knob bound to a synth control sends an OSC `/n_set` (or equivalent) to the audio server itself on every change; an unbound knob just emits `/gui_event` back to the script. This keeps interactive control low-latency while leaving scripted/computed widgets fully in the script's hands.

### Example session

```
# script -> gui host
/gui_def  1  { "type":"window", "title":"Filter", "w":480, "h":240, "layout":"col",
               "children":[
                 {"id":10,"type":"knob",  "label":"cutoff","min":20.0,"max":20000.0,"value":800.0},
                 {"id":11,"type":"slider","label":"res",   "min":0.0, "max":1.0,    "value":0.2},
                 {"id":12,"type":"waveform","buffer":0}
               ] }
/gui_bind 10 "server" "/n_set" 1000 "cutoff"   # knob 10 drives node 1000's cutoff directly

# gui host -> script  (only for the unbound slider; the knob talks to the server itself)
/gui_event 11 0.35
```

## The widget catalog (the JSON elements)

A GuiDef is a tree of nodes; each node is `{ "id": int, "type": str, <props...>, "children": [...] }`. Every widget type combines freely inside that tree. The catalog is meant to be **extensible** - adding a type is a new renderer/handler in the host, not a protocol change.

| Type | Category | Role |
|---|---|---|
| `window` | container | Top-level root container (title, size, layout). |
| `panel` / `box` | container | Nestable container; `layout` = `row` / `col` / `grid` / `free`. |
| `label` | control | Static text. |
| `slider` | control | Continuous value with range. |
| `knob` | control | Rotary continuous value. |
| `button` | control | Momentary action. |
| `toggle` | control | Boolean state. |
| `number` | control | Numeric entry field (int or float). |
| `text` | control | Text entry field. |
| `menu` | control | Dropdown / option list. |
| `waveform` | heavy GPU view | Editor-grade min/max peak waveform of a buffer or blob (existing renderer). |
| `spectrogram` | heavy GPU view | STFT time-frequency view (existing renderer). |
| `scope` | heavy GPU view | Oscilloscope (time-domain) - **future**. |
| `phasescope` | heavy GPU view | Phase/goniometer (Lissajous) view - **future**. |
| `meter` | heavy GPU view | Level meter reading a control bus directly from shared memory. |
| `spectrum` | heavy GPU view | Live FFT magnitude curve (spectroscope) - **future**. |
| `plot` | view | Simple static plot of an NRT-generated signal/file. |
| `nodetree` | view | Live text/graphic view of the audio server's node tree and parameters, updated in real time. |
| `canvas` | view | A surface that runs a supplied WGSL shader, driven by OSC params or server audio - custom visuals. |
| `score` | view | Music notation (Verovio SVG) - **future**, off the GPU path. |
| `timeline` | view | DAW-style tracks + MIDI/OSC sequencing - **future**. |
| `bpf` | view | Drawable break-point-function envelope with curves - **future**. |

Heavy views never reimplement DSP the server already owns: when a widget needs analysis/processing not already provided by `clausters-server` (peaks, STFT, FFT, resampling), `clausters-gui` reaches for `clausters-ffi`/`libclausters` rather than duplicating signal code. The `clients/gui` crate's own `peaks`/`spectrogram` modules are the prototype of that shared machinery and are a candidate to migrate behind the FFI.

## Status: foundation in place

The `clients/gui` crate (an independent workspace, so it can never break the core build) already validates the heavy-rendering path - the `Gx` work below builds the protocol/host around it:

- `viewport::View` - reusable time/secondary-axis navigation (zoom/pan/clamp, `samples_per_px`), unit-tested.
- `peaks::Pyramid` - resolution-matched min/max peak analysis with a memory/temp-file cache (mmap-ready), unit-tested.
- `spectrogram::Stft` - STFT analysis + FFT with the same cache shape, unit-tested.
- `waveform` / `spectrogram` renderers - the GPU pieces (three render regimes for the waveform; constant-cost texture sample for the spectrogram), with log/linear axis, dB window and colormaps.
- `view::TimelineView` + `native` - the trait both views implement and a generic winit + wgpu harness driving either.
- `bytes` - shared little-endian cache (de)serialization.

And on the core/server side, the transport the web direction needs is already done (G1, below).

## G1 - WsHub transport - DONE (in main)

A WebSocket transport carrying the existing OSC encoding, with a minimal client that drives the running audio server over it. Landed on `main`, not yet on this branch.

- `src/osc/ws.rs` - the server-side WebSocket listener, a sibling of `tcp.rs`: an acceptor thread plus one thread per connection turning each binary WebSocket message into a whole OSC packet for the single-threaded command loop, reusing the zero-length-UDP wake. Each connection thread drains a per-connection reply channel (a `tungstenite` `WebSocket` owns its stream) and polls with a short read timeout to interleave reads and replies. Enabled with `--ws`.
- `ClientId::Ws` - replies route back by connection id, mirroring `Tcp`.
- `crates/clausters-ffi/src/ws.rs` - the WebSocket **client** over the C ABI (`clausters_ws_connect/send/recv/close/last_error`), so a non-browser binding (Python `ctypes`, JS N-API) reaches a `--ws` server without re-implementing the handshake/framing. Browsers use their native `WebSocket`.
- `examples/ws_ping.py` and `examples/ws_ping.html` - the smallest round trips over the carrier (Python via the ffi client; browser via native `WebSocket`/`ArrayBuffer`).

**Wire format:** each WebSocket **binary** message carries exactly one OSC packet; the WS frame boundary *is* the OSC packet boundary (no length prefix). Replies go back as binary messages. Everything decodes through `osc::decode_packet`.

**Remaining for this branch:** merge `main` into `gui` so `WsHub` and the ffi client are present here; nothing else in G1 to build.

## G2 - GUI host skeleton (`clausters-gui` binary) - DONE (2026-06-25)

A separate crate/binary that speaks OSC over the reused transport layer with a widget command interpreter instead of the audio engine - the dual-role process from the naming section, starting headless.

**Transport decision (the milestone asked to record one):** the host does *not* extract or link the audio server's transport layer (`src/osc/{server,tcp,ws}.rs`) - it is tangled with the audio `ServerState`, the engine wake and the IPC ring, so lifting it now would drag server concerns into the independent gui crate for no gain. Instead the host **links `clausters-core`** (a path dependency that pulls only `rosc`, never the server crate) for the shared OSC seam - the single decode door (`clausters_core::osc::decode_packet`, which the server now delegates to as well, so the whole system decodes through one function), plus encode/bundle/message - and owns a **thin transport front** of its own (`host::transport`). G2 ships the **UDP** front (the default Clausters carrier, minimal to drive from a Python client); TCP/WebSocket/ring follow in later milestones behind the same `ClientId`/reply seam, which is shaped to generalize.

**Landed:** the `clausters-gui` binary (`--port` default 57210, `--server host:port`, `-v`/`-q`/`RUST_LOG`); a generic GuiDef node (`host::guidef`, `{id,type,...props,children}`, serde-parsed with the int/float distinction kept); the widget `host::registry` reusing the node-tree shape (client ids, parent/children, subtree free, redefine-replaces); the transport-agnostic `host::Host` command loop (interprets def/set/free/query, logs the parsed tree, answers `/gui_query` with `/gui_info`, reserves `/gui_bind`/`/gui_load`); the scaffolded client leg `host::client::ServerLeg`; the Python driver `clients/python/clausters/gui/` (`guidef` builders + `GuiHost` over `OscUdpInterface`); `examples/gui_skeleton.py`; 13 host unit tests. See `LOG.md` for detail.

### Scope

- The `clausters-gui` binary: a server front (script -> host) over the reused transports (ring/TCP/WS/UDP, the `decode_packet` door, `ClientId`), and a client leg (host -> audio server) reusing the same client encoder `clausters-python`/the ffi client use.
- A widget registry: client-allocated integer ids, a window/widget tree with subtree freeing, mirroring the node-tree semantics.
- The command loop interprets `/gui_def`/`/gui_set`/`/gui_free`/`/gui_query` and logs them; emits `/gui_event`/`/gui_info` stubs. **No GPU yet.**
- Decide how much transport code is shared: extract a small transport crate vs. the host linking the core. Record the decision.

### Acceptance

- A script (Python) connects to `clausters-gui`, sends a `/gui_def`, and the host logs the parsed tree and replies to `/gui_query`; run as a single Bash invocation per the E2E rule.
- `cargo fmt --check` clean, clippy clean; the core still builds/tests with no optional features.

## G3 - `/gui_def` + JSON widget tree + a real window - DONE (2026-06-25)

The GuiDef schema and the first pixels.

**Landed:** a typed widget schema (`host::widget`, the *renderer's* interpretation of the generic `GuiNode` - adding a type is a new variant, not a protocol change; an unknown type is laid out but not painted) for `window`/`panel`/`label`/`waveform`, with inline (`data`) or trailing-OSC-blob (`blob`) waveform samples and the int/float distinction kept; a pure layout engine (`host::layout`, `row`/`col`/`grid`/`free` -> pixel rects, unit-tested); the windowed front (`host::gui`) - winit on the main thread, the OSC transport on a background thread feeding it via an `EventLoopProxy`, multi-window by def id, a `window` root opening an OS window and `/gui_free` closing it; rendering the heavy `waveform` (the existing `WaveformView`) into each widget's viewport with wheel-zoom/drag-pan/`R`-reset navigation, and panels/labels as flat chrome rects (`host::rects`; glyph text deferred). The binary gains `--headless` (protocol with no display; the default opens windows). Python: `clausters.gui.waveform(data=/blob=)`, `samples_to_blob`, `GuiHost.define(id, tree, *blobs)`, and `examples/gui_window.py`. See `LOG.md`.

### Scope

- The widget-tree JSON schema (serde), with the int/float distinction preserved and the "flat primitives at the boundary" rule.
- `/gui_def <id> <json tree>` instantiates a winit window with a wgpu surface hosting the existing renderers; `/gui_free <id>` frees a subtree.
- First standardized widgets: `window` + `panel`/layout (`row`/`col`/`grid`/`free`) + `label`, plus the heavy `waveform` view fed a blob or buffer ref.

### Acceptance

- The test client creates an actual window showing the waveform from one declarative `/gui_def` message.

## G4 - Standard control widgets + `/gui_set` + events - DONE (2026-06-25)

The essentials of any GUI, plus the live update and event paths.

**Landed:** the standard controls (`host::widget` typed kinds + `host::controls`) - `slider`/`knob`/`number` over a value range, `button` (momentary), `toggle`, `menu` (click-cycles), `text` (script-driven); the G3 rect renderer generalized into `host::paint` (a triangle `Mesh` + one `Painter`: rect/quad/line/disc) so knobs and a small embedded 5x7 bitmap font (`host::font`, the glyph text deferred from G3) need no new GPU code; the typed tree made the single source of truth in the `Host` so a live `/gui_set` and a user drag update the same tree (`Registry::root_of` + `HostEffect::Redraw`); interaction routed by hit-test to set values and emit `/gui_event <id> <value>` to the def's origin (button press/release 1/0, toggle/menu/slider/knob/number), `/gui_closed <id>` on user close, and the waveform's zoom/pan emitting `/gui_event <id> "view" start len`. Python `clausters.gui` gained `number`/`button`/`toggle`/`text`/`menu` builders and `GuiHost.poll`/`listen`; `examples/gui_panel.py`. Runtime-verified (window opens, controls render, live set + real drags round-trip events, no panic). See `LOG.md`.

### Scope

- Control widgets: `slider`, `knob`, `button`, `toggle`, `number`, `text`, `menu`.
- Live property updates (`/gui_set <id> <k> <v>...`) and host->script events (`/gui_event`, `/gui_closed`, `/gui_info`).
- Wire the `TimelineView` interactions (zoom/selection) back out as `/gui_event`.

### Acceptance

- A scripted instrument panel (knobs/sliders/buttons) round-trips: `/gui_set` updates a live widget, user interaction emits `/gui_event`, closing the window emits `/gui_closed`.

## G5 - GUI as a client of the audio server + shared-memory meters/scopes - DONE (2026-06-26)

The host attaches to the audio server and the zero-message metering path lands.

**Landed:** the host's client leg is now bidirectional (`host::client::ServerLeg` over a shared `Arc<UdpSocket>`: send queries, receive replies); the windowed front spawns a second thread draining the leg and routing `/b_info`/`/b_setn` to a buffer-fetch state machine. **Shared-memory meters/scopes:** a read-only `host::shm::SharedSegment` mmaps the audio server's `--shm` segment, mirroring its versioned `#[repr(C)]` ABI and rejecting a magic/version mismatch (the transport/reuse decision, recorded below); `meter` and `scope` are new `WidgetKind`s drawn from `host::meters` (a bar / a rolling polyline) that read a control bus straight from the segment every frame, with the windowed front animating such windows at ~30 fps (`ControlFlow::WaitUntil`) and reading **zero** OSC. **Server-buffer waveform:** a `waveform` with a `buffer` number is fetched over the leg (`/b_query` then chunked `/b_getn`, de-interleaved to channel 0) and built into a `WaveformView` once it arrives. The server gained the standard scsynth reads `/b_get` (`/b_set`) and `/b_getn` (`/b_setn`), synchronous from the buffer mirror, benefiting every client. Binary: `--shm <path>`. Python: `clausters.gui.meter`/`scope` builders, `waveform(buffer=)`, and `examples/gui_meters.py`. Runtime-verified against the real server: the host maps the live segment (1024 buses), opens the window, and loads a 24000-frame buffer over the leg with no panic. See `LOG.md`.

**Transport / reuse decision (the milestone's recurring "record it"):** the GUI crate stays independent of the **server** crate (it would drag in the engine, cpal and Faust). For the zero-message meter path it therefore **mirrors the shared segment's versioned binary ABI** in a small read-only reader (`host::shm`) rather than linking `server::ipc` - the same role any independent peer (the Python `ctypes` client, a future JS one) plays against this boundary. The safety net against drift is the segment's `MAGIC`/`ABI_VERSION`, checked on attach, so a layout change fails loudly instead of reading stale memory. The command plane (buffer reads, later bound widgets) rides the existing UDP leg through the one `clausters_core::osc` encode/decode door; only what shared memory cannot carry goes over messages.

### Scope

- The host attaches to `clausters-server` as a client (the third leg of the topology).
- `meter` and `scope` widgets read control buses **directly from the shared segment each frame** (zero messages).
- A `waveform`/`spectrogram` widget can reference a **server buffer number** instead of a local blob.

### Acceptance

- A meter widget tracks a live control bus with no per-frame OSC traffic; a waveform widget renders a server buffer by number.

## G6 - Bindings (`/gui_bind`): bypass the script - DONE (2026-06-26)

The low-latency interactive control path.

**Landed:** a `host::bind::Binding` (an OSC `addr` plus a fixed `prefix` of arguments) parsed from a `/gui_bind <id> "server" <addr> <prefix…>` target - the leading `"server"` destination keyword is kept in the wire form so the message shape can grow later (binding to another widget, or to the script with a transform) without a protocol change. The `Host` holds a `widget id -> Binding` map: `on_bind` registers it (warning when no `--server` leg is attached, since the value then has nowhere to go), `/gui_bind <id>` with **no** target removes it (restoring the event path), and `forward(widget_id, value)` sends `addr prefix… value` through the existing client leg (`host::client::ServerLeg`, the same `clausters_core::osc` encode door) and reports whether the binding handled the value. The windowed front routes **every** value-bearing interaction (slider/knob/number drag, toggle, menu, button press/release) through one `deliver` that calls `forward` first and only emits a `/gui_event` when unbound - so a bound widget's value reaches the audio server with no script round-trip. Bindings are pruned when their widget is freed or redefined away (`/gui_free`, a replacing `/gui_def`), so a stale id cannot keep forwarding. Python: `GuiHost.bind(id, address, *prefix)` / `unbind(id)` and `examples/gui_bind.py` (a knob bound to a sine synth's `freq`, then unbound). Verified: a unit test forwards `/n_set 1000 cutoff 440.0` over a real loopback leg and stops after unbind; a headless E2E round-trips bind/unbind from the Python client with the int/float distinction kept; the windowed host opens a window with a bound knob and registers the binding with no panic. See `LOG.md`.

### Scope

- `/gui_bind <id> <target...>` forwards a widget's value straight to the audio server as OSC (`/n_set` and friends); the unbound case keeps emitting `/gui_event`.

### Acceptance

- A bound knob drives a running synth's control with no round-trip through the script; unbinding restores the event path.

## G7 - Bulk data path + shared DSP - DONE (2026-06-26)

Two principles the rest of the system already implies, made concrete - and both are implemented here, so the milestone splits into two subsections. First, heavy data moves between Clausters processes through **local shared resources** rather than the wire. Second, an algorithm used by more than one process lives **once**, in the shared core, never reimplemented per client. The "DSP" here is the GUI's *analysis-for-plotting* - the peak pyramid (waveform level-of-detail) and the FFT/STFT (spectrogram) - which today lives only in this crate (the server owns no peak/FFT analysis: `clausters-core` is builtins/osc/rng/tempoclock, and `spectrogram.rs` carries a handrolled radix-2 FFT).

### G7a - Bulk data transfer: local shared resources, not the network

**Decision** (the milestone asked to record one, against the real constraint rather than in the abstract): large payloads - sample buffers, analysis caches (peaks/STFT), rendered or filtered files - move between processes via **local shared resources** (memory-mapped files, and the shared segment where it already exists), **never re-encoded over OSC**. The reasons are concrete: a UDP datagram caps near 64 KB, so a multi-megabyte buffer cannot ride one `/gui_def` blob, and chunking it over `/b_getn` re-traverses the network *asynchronously* for data that already sits in local RAM; a mapped file is read once, zero-copy, with no re-send. The network buffer primitives stay available - they work, and the browser, which can map neither shared memory nor files, needs them (G11) - but they are the **async fallback**, not the bulk path. This is the same move G5 made for control buses (read from the shared segment with zero messages), generalized to bulk audio.

Concretely:
- A `waveform`/`spectrogram` names a **local resource**: a memory-mapped file of raw little-endian `f32` samples (`path`, with `channels` to de-interleave, default 1) or a prebuilt analysis cache (`cache`, a `peaks` pyramid). The host maps it read-only and reads it zero-copy - reusing the `host::shm` mmap path - instead of receiving an OSC blob. A multi-megabyte buffer renders with no network traffic and no re-send; a built pyramid is cached as a sibling file keyed by `base_bucket`, so re-opening is instant.
- **Server RT buffers are plottable through the same shared-resource path**, not only client files: a server command exports a buffer's raw `f32` samples to a mapped file (mirroring the existing buffer disk I/O), which the host maps like any other resource. So a live server buffer is shown without pulling it over `/b_getn`. (A buffer pool natively backed by shared memory - a live, updating view - is the follow-on once a consumer needs the live case; G7 ships the snapshot export. The `--shm` segment's fixed ABI is untouched.)
- The principle "audio processing happens once, in the server" makes this natural: a client that needs processed audio (an FFT buffer, a filtered file) runs it in a server instance - live, or a parallel NRT instance via `clausters_render`/`DiskOut`, which already yield flat samples / a WAV file - and hands the **result resource** to the host, rather than computing audio client-side.

### G7b - Shared analysis algorithms in the core: FFT (and peaks)

**Principle**: an algorithm used by more than one Clausters process lives **once**, in `clausters-core`, reached by clients through the FFI - never reimplemented per client. Two land here.
- **FFT** is shared between the server and the clients. The server will grow `FFT`/`IFFT` UGens (the SuperCollider spectral chain) and the GUI spectrogram already needs a forward FFT (today a handrolled radix-2 in `spectrogram.rs`, with a standing note to swap in a crate). The forward FFT moves into `clausters-core` on a **lightweight, RT-capable, task-specific** crate - `microfft` (`no_std`, zero-allocation, compile-time power-of-two sizes, which are exactly the STFT's window sizes) so `process` never allocates, the property the future RT UGens require. The gui `spectrogram::Stft` drops its private `fft()` for the core one. (`microfft` is forward-only; if the later `IFFT`/`PV_*` UGens need the inverse, that crate choice is revisited then, behind the same core API - out of scope here.)
- **Peaks** - the min/max pyramid - is not real-time and the server will not use it for processing, but it is **general Clausters client functionality** (any waveform view, in any client). It moves into `clausters-core` (pure, no GPU) and is exposed over the FFI so the Python client (and a future JS one) build the **identical** cache the host reads - which is what feeds G7a's bulk path (the client builds the compact cache; the host maps it). The gui crate's `peaks.rs` delegates to the core implementation; the renderer is unchanged.

### Scope

- Bulk path for large client payloads via mapped files / cache, and for server RT buffers via a shared-resource export; the network buffer reads remain the async fallback.
- The forward FFT and the peak pyramid live once in `clausters-core`; peaks is reachable from clients over the FFI. The inverse FFT, STFT-as-a-server-product, and a live shared buffer pool are deferred with their reasons recorded.

### Acceptance

- A multi-megabyte buffer - client-origin (a mapped file/cache) and server-origin (an exported RT buffer) - renders without re-sending it per frame and without riding OSC; the analysis path is documented, the FFT and peaks live once in the shared core, and peaks is callable from the Python client through the FFI.

## G8 - Node-tree view + NRT plots - DONE (2026-06-26)

Two read-only views that exercise the "gui is a client of the server" leg.

**Landed:** both views are cheap (the flat-geometry painter + bitmap text, no dedicated GPU pipeline) and added by extension - a new `WidgetKind` plus a renderer, no protocol change - and the audio server is untouched (G8 reuses its existing `/g_queryTree`/`/notify`/`/n_go`/`/n_end` path). The `nodetree` view (`host::nodetree`) mirrors the server's node tree: a pure, unit-tested model + parser of scsynth's depth-first `/g_queryTree.reply` (nested groups, named/index controls, empty and truncated replies), drawn as indented lines in a framed field with `no server`/`querying...` placeholders; the windowed front (`host::gui`) registers `/notify 1` once, re-queries on every `/n_go`/`/n_end` and polls every 200 ms for `/n_set` changes, and repaints a group's windows only when the parsed tree actually changed. The `plot` view (`host::plot`) is the lightweight static counterpart of the heavy `waveform`: it decimates to the pixel width (a polyline when the data fits, a per-column min/max envelope otherwise, with a zero baseline), fed inline (`data`/`blob`) or from a mapped local `path` of raw little-endian `f32` (the bulk path, no OSC, reusing `host::mapfile`). Python: `clausters.gui.nodetree`/`plot`; `examples/gui_nodetree.py` (a live tree with a swept control and a synth coming and going) and `examples/gui_plot.py` (a `Session.nrt()` render written to a file and plotted). Runtime-verified against the real server + a GPU window: the tree refreshes ~5 Hz tracking a live `/n_set` sweep, the plot maps and renders a file, no panic. See `LOG.md`.

### Scope

- `nodetree` widget: a live text/graphic view of the audio server's node tree and parameters, updated in real time (driven by the server's query/notification path).
- `plot` widget: a simple static plot of an NRT-generated signal/file.

### Acceptance

- The node-tree widget reflects group/synth creation and `/n_set` changes live; a `plot` renders an NRT render's output.

## G9 - Canvas + shaders

Custom visuals from the script.

### Scope

- A `canvas` widget that accepts a WGSL shader (sent as a property) and runs it, driven by OSC params and/or server audio/buses, for arbitrary custom visuals.

### Acceptance

- A scripted shader animates from OSC parameters and from a control bus read out of shared memory.

## G10 - Standalone GuiDef + GraphDef bundles

The saved-application mode: a GUI with no language client.

### Scope

- Persist GuiDefs to the def store the way SynthDefs/GraphDefs persist (the `/gui_load <name>` path).
- `clausters-gui` boots a saved GuiDef paired with GraphDefs against an embedded audio server - a self-contained app, no separate `clausters-python`/JS process.

### Acceptance

- A saved GuiDef + its GraphDefs launch a working instrument from `clausters-gui` alone.

## G11 - Browser / WebGPU target

The web host becomes real.

### Scope

- Swap the native winit surface for a `<canvas>` WebGPU surface; the JS client builds OSC over WS; the renderers run unchanged.

### Acceptance

- The same GuiDef that opens a native window opens a browser window over WebGPU, driven by a JS client over WS.

## Future directions (to fold into milestones as they firm up)

Captured here so the depth the editor-grade vision needs is not lost; each becomes a `Gx` when its design converges.

- **Editor-grade waveform/spectrogram.** Refine both views with the full visual-parameter surface of an audio editor (multi-channel stacked/overlaid waveforms, interpolating between pyramid levels, frequency-axis Hz rulers, selection/playhead overlays, time-axis chrome), even when first used only to plot signals.
- **Scopes.** A time-domain oscilloscope, a phase/goniometer (Lissajous) scope, and a live FFT spectroscope - each a `TimelineView`-style module sharing the navigation machinery, fed from shared memory.
- **Edit-back-to-data.** Today the heavy views *receive* data to visualize; the design must keep room for them to *modify* it - drawing into a buffer, editing a list/envelope - and write it back to a client or the server.
- **DAW / timeline view.** Tracks with audio and MIDI/OSC sequencing; since the audio lives in the server, the view reads it from there. The reference point is an OSC-controllable DAW transport and control elements.
- **Automation / BPF view.** Drawable break-point-function envelopes whose drawn values become a specification consumed elsewhere - the cleanest case of edit-back-to-data.
- **Notation (`score`).** Verovio (C++ -> wasm/JS) rendering MEI/MusicXML to interactive, editable SVG in the web surface, off the GPU path entirely.
- **Packaging.** An optional Tauri desktop wrapper reusing the web frontend; the GUI chapter in the docs; worked examples and `GUIA.md` steps.

## Definition of done (per milestone)

Following the project rule: code + tests, `LOG.md`/this file updated, developer docs where applicable, user docs in `docs/` for user-facing features, `GUIA.md` manual-test steps, and a commented example when the feature is user-facing - not just code.
