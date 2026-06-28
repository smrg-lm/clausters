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

## G9 - Canvas + shaders - DONE (2026-06-26)

Custom visuals from the script.

**Landed:** a `canvas` widget that runs a **script-supplied WGSL shader** over its area (ShaderToy-style), added by extension (a new `WidgetKind` + a GPU view, `host::canvas::CanvasView`), no protocol change, the audio server untouched. The user writes a `shade` function; the host wraps it with a fixed prelude (the uniform block + a full-screen-triangle vertex shader) and a `fs_main`, compiles a pipeline, and exposes uniforms `resolution`, `time` and a `params` vec4. The four params are driven two ways - the point of the widget: from the **script** (`/gui_set param0...`, an OSC value -> `u.params.x..w`) and from a **control bus read out of shared memory each frame** (`buses[i]` maps a bus onto param `i`, `-1` keeps it script-driven), the zero-message path the meters use. A shader that fails to compile is caught with a wgpu validation error scope (no panic, the canvas stays un-painted with a warning); `set_shader` recompiles in place only when the source changed. A canvas window is animated (~30 fps, time-driven, independent of `--shm`). Python: `clausters.gui.canvas(id, shader, params=, buses=)`; `examples/gui_canvas.py` (a shader whose ring follows an OSC param and whose green channel follows a control bus). Runtime-verified against the real server + a GPU window (shader compiles and animates from the OSC param and the shm bus at once; an invalid shader is caught with the window still opening), no panic. See `LOG.md`.

### Scope

- A `canvas` widget that accepts a WGSL shader (sent as a property) and runs it, driven by OSC params and/or server audio/buses, for arbitrary custom visuals.

### Acceptance

- A scripted shader animates from OSC parameters and from a control bus read out of shared memory.

## G10 - Standalone GuiDef + GraphDef bundles - DONE (2026-06-27)

The saved-application mode: a GUI with no language client.

**Landed:** a *bundle* is a data directory holding a named GuiDef beside the SynthDefs/GraphDefs it needs, and `clausters-gui --standalone <name>` boots it as a self-contained instrument - no separate audio server and no language client. GuiDefs persist the way the server's defs do (`host::store::GuiStore` mirrors `src/server/defstore.rs`: the same data-dir resolution, `sanitize_name`, atomic writes, a `defs/guidefs/<name>.json` record `{id, gui}` beside the sibling `defs/synthdefs`/`defs/graphdefs`), the GUI keeping its own small store so the default build (which does not compile the server) still persists. A live `clausters-gui --data-dir` auto-persists any `/gui_def` whose root tree carries a `name` prop, and `/gui_load <name>` replays a saved one; two GuiDef props make a saved tree self-driving so it needs no script - a root `boot` list of OSC messages the standalone host sends once the defs load (e.g. an `/s_new`) and a widget `bind` prop, the declarative form of `/gui_bind` (`Binding::from_json`). The embedded server is a **direct dependency on the `clausters` crate** (`embed,realtime`) behind the optional **`standalone` feature** - the gui is part of the same ecosystem as the server, so it just links it; `host::embed::EmbedServer` wraps `clausters::embed::Clausters` and drives it through its direct Rust API (`open`/`send`/`poll_into`). The feature is off by default because it pulls the engine + audio backend (the size/packaging reason the gui is a separate crate); to make that link clean, `src/embed.rs` was refactored so the in-process server has a Rust API and the C ABI the Python client uses became a thin wrapper over it. The host's server leg becomes a `ServerLink::{Udp, Embed}` so bound widgets drive the in-process server directly and its replies drain into the event loop. The int/float distinction survives end to end (node ids stay integers, control values floats). Python: nothing new to learn - the existing `guidef` builders pass `name`/`boot`/`bind` through verbatim; `examples/gui_standalone.py` authors a bundle on disk (a drone SynthDef + a one-knob GuiDef bound to its `freq`) and prints the `cargo run --features standalone …` launch command. Runtime-verified end to end against a real GPU window: the feature-linked binary starts the embedded server (no FFI), the bundle's def loads, the `boot` `/s_new` brings the instrument up, the window opens and the bound knob drives the embedded server, no panic. See `LOG.md`.

### Scope

- Persist GuiDefs to the def store the way SynthDefs/GraphDefs persist (the `/gui_load <name>` path).
- `clausters-gui` boots a saved GuiDef paired with GraphDefs against an embedded audio server - a self-contained app, no separate `clausters-python`/JS process.

### Acceptance

- A saved GuiDef + its GraphDefs launch a working instrument from `clausters-gui` alone.

## Browser / WebGPU target (G11-G16)

The earlier single milestone framed the web host as "swap the winit surface for a `<canvas>` surface; the renderers run unchanged" - true, but it describes the small part. The host is ~7150 lines, nearly all native-only behind `#[cfg(not(target_arch = "wasm32"))] pub mod host`, against ~1095 lines of genuinely portable renderers/analysis (`viewport`/`peaks`/`view`/`waveform`/`spectrogram`); inside the host sit 16 `UdpSocket`, 15 `SharedSegment` (shm), 12 `MappedFile` (mmap), 6 `EmbedServer` and 3 `pollster::block_on` couplings. Crucially, much of the host is **pure logic** (the typed widget tree, layout, GuiDef parse, the mesh `Painter`/`font`, the registry, the node-tree/scope models, bindings) that only sits behind the `wasm32` exclusion because it was never separated from the I/O glue. So the browser target is not a rewrite: it is **factor the host along the platform seam, then write one new I/O implementation per native coupling, reusing everything else**. The overriding constraint for every milestone below is **maximum reuse** - the protocol, the decode door, the analysis, the renderers and the whole pure host core are shared verbatim; only the platform shell (transport, bus source, bulk loader, GPU/surface bring-up) gets a second, web impl.

### G11 - Host platform seam (agnostic core + Platform traits, wasm build kept green) - DONE (2026-06-28)

No browser code yet: this milestone only carves the seam, so the later web milestones are trait-fills rather than rewrites, and it turns browser-readiness from a one-shot milestone into an invariant a build gate enforces.

**Landed:** `pub mod host` is now unconditional (`src/lib.rs`), so the host compiles for `wasm32`; only the I/O shell stays `#[cfg(not(target_arch = "wasm32"))]` - `client` (UDP leg), `store` (filesystem persistence), `transport` (UDP server front), `bulk` (the new mmap loader) and `gui` (the winit/wgpu driver), with `shm`/`mapfile` keeping `#[cfg(unix)]` and `embed` its `standalone` feature. The pure modules (`widget`, `layout`, `guidef`, `registry`, `controls`, `paint`, `font`, `nodetree`, `plot`, `meters`, `bind`, `canvas` and the protocol dispatch in `mod`) moved out from behind the wasm exclusion unchanged. The I/O couplings sit behind small traits, the only new surface: `Transport` (send one OSC message to the audio server; `ServerLink` implements it), `DefStore` (named-GuiDef save / `/gui_load`; `GuiStore` implements it), `BulkLoader` (resolve a waveform/plot `path`/`cache` to `WaveformData`/samples; native `host::bulk::MmapLoader` is the desktop fill, the `gui` mmap helpers moved into it verbatim), and `BusSource` kept as-is. `Host` no longer names a native type - `store` is a `Box<dyn DefStore>`, `with_store` is generic, `ClientId` moved into the agnostic core, and `ServerLink`'s `Udp` variant is `#[cfg(not(wasm32))]` so the enum is uninhabited on `wasm32` (the host runs with no audio-server leg until the web carrier lands, G13). `clients/gui/check-wasm.sh` builds the agnostic core for `wasm32-unknown-unknown` as the build gate. gui 81 tests unchanged; `cargo fmt --check`/`clippy -D warnings` clean native and `wasm32`; the standalone build still links the embed; the native host verified byte-identical (the headless `gui_skeleton.py` round-trip). See `LOG.md`.

**Scope:**

- Split `host` into a platform-agnostic core and a thin native shell, with the I/O couplings behind small traits: `Transport` (the script front and the audio-server leg: deliver an inbound OSC packet, send an outbound one), `BusSource` (already a `dyn` trait for meters/scopes - keep it as-is), `BulkLoader` (resolve a waveform/plot `path`/`cache`/`buffer` to samples or a peak pyramid; today inline mmap calls in `gui`), and a GPU/surface + loop driver (today `pollster::block_on(Gpu::new)` plus the winit `App`). The traits are the *only* new surface; the logic behind them is moved, not rewritten.
- Move the pure-logic modules out from behind `#[cfg(not(wasm32))]` so they compile for `wasm32` unchanged (`widget`, `layout`, `guidef`, `registry`, `controls`, `paint`, `font`, `nodetree` model, `bind`, `plot` model, the protocol dispatch in `mod`); leave only the native shell cfg-gated (UDP `Transport`, shm `BusSource`, mmap `BulkLoader`, the winit driver, the `standalone` `EmbedServer`).
- Add a `cargo build --target wasm32-unknown-unknown` of the agnostic core to the build/CI checks, so no later milestone can re-couple it to native I/O unnoticed.

**Acceptance:** the native host behaves byte-identically (the existing examples and tests pass unchanged); the agnostic core compiles for `wasm32` with the native shell excluded; the only `#[cfg(not(wasm32))]` left inside `host` is the I/O shell, not the widget/protocol logic.

**Decision (record it):** for the browser GUI track (G11-G16) the browser host always talks to a *separate* audio server over WebSocket, and there is no in-process engine in the browser (the `standalone` `EmbedServer` stays native-only behind its feature); the browser's data paths are then the "async fallback" the bulk-data decision (G7) already reserved for exactly the client that can map neither shared memory nor files. **This is a scope boundary, not a fundamental constraint.** "No in-process engine in the browser" holds only because porting the engine to the browser (a Web Audio / AudioWorklet backend) is a larger, separate piece of work, deferred to its own future track (see "In-browser audio engine" below). If that lands, the browser gains a second link kind - the wasm analogue of the native `Embed` - and the host<->engine OSC rides an in-process channel instead of WebSocket, exactly as `ServerLink::Embed` does natively today; the `Transport`/`ServerLink` seam this milestone builds is shaped to take that variant without a protocol change. WebSocket stays the carrier for the *remote* leg (a browser cannot reach an external process any other way), so it never goes away - it stops being the only option.

### G12 - Web surface: `<canvas>` WebGPU + async GPU + render loop - DONE (2026-06-28)

The first browser pixels, with no transport yet, so the surface/GPU/loop port is isolated from the protocol. Reuses the layout, `Painter`, `font` and `WaveformView` paths verbatim; the only new code is the wasm entry point and async bring-up.

**Landed:** the per-window render was factored verbatim out of the native `gui::App::render` into a shared, platform-agnostic `host::frame::render`, so both fronts draw a tree through **one** code path - the browser is pixel-faithful by construction, not a parallel renderer. The native windowed front calls it with live inputs (a `FrameInputs` carrying the shared-memory bus, the scope histories, the node trees and the held-button highlight); the browser calls it with the defaults (no bus, no node tree). `Gpu` (the wgpu device/surface bring-up) moved to an agnostic `crate::gpu` (it compiles to the WebGPU backend on wasm), shared by the native harness, the windowed front and the web entry. `host::web` (wasm-only) is a `wasm-bindgen` `start` entry that creates a winit window over an HTML `<canvas>` (`with_append`), brings up wgpu **asynchronously** through `wasm_bindgen_futures::spawn_local` plus an `EventLoopProxy` `GpuReady` event (no `block_on` - the browser main thread never blocks), and renders a compiled-in GuiDef (a panel of controls plus an inline-data `waveform`, parsed through the unchanged `GuiNode::parse`/`Widget::from_node` path) via `frame::render` on each `RedrawRequested`. Packaging for the bundle: `[lib] crate-type = ["cdylib", "rlib"]`, wasm32-target deps (`wasm-bindgen`, `wasm-bindgen-futures`, `console_error_panic_hook`, `web-sys`), `web/build.sh` (cargo wasm build + `wasm-bindgen --target web` into `web/`) and `web/index.html` (the quick-start loader). Native unchanged (gui 81 tests, `clippy -D warnings` clean native + wasm; the windowed host opens a real GPU window through the shared path with no panic); the wasm bundle generates and, in Chrome over WebGPU (Vulkan/ANGLE), the full path runs - the async device comes up and `frame::render` executes (console-confirmed). See `LOG.md`.

**Scope:**

- A wasm entry point (wasm-bindgen) that creates a winit web window over an HTML `<canvas>`, requests a WebGPU adapter/device **asynchronously** (no `block_on`; `Gpu::new` is already `async`), and drives the existing render loop from the browser's animation frame via winit's web backend.
- Render a window-rooted GuiDef built in Rust (a panel of controls plus a `waveform` from inline data) through the unchanged core render path.

**Acceptance:** a compiled-in GuiDef renders in a browser tab over WebGPU - controls, chrome, bitmap text and an inline waveform - pixel-faithful to the native host; no `block_on`, socket or mmap on the wasm path.

### G13 - Web transport: drive the browser host live over WebSocket - DONE (2026-06-28)

The browser host stops being static. Reuses the entire protocol dispatch and the G1 WS wire format; the only new code is a `Transport` impl over the browser `WebSocket` plus the small wasm-bindgen surface that lets in-page code feed the host a GuiDef and pump its events.

**Landed:** the browser front now runs the **real** `Host` (the same protocol dispatch, tree, bindings and `forward`) and reuses the shared render (`frame`) and a new shared interaction module `host::interact` (hit-test + value/toggle/menu mutation, extracted verbatim from the native front so both platforms decide bound-vs-event identically; native delegates to it). The binding surface is a wasm-bindgen `GuiBridge`: `feed(packet)` / `def(id, json)` push an OSC packet (a `/gui_def`, `/gui_set`, `/gui_bind`, …) to the host through the one `decode_packet` door, `poll()` drains the outbound `/gui_event`/`/gui_info` packets (encoded OSC) the page reads, and `connect_server(url)` attaches the audio-server leg. That leg is `host::web::WsServerLink`, a new `ServerLink::Ws` variant (a browser-native `WebSocket` to a `--ws` server, frames buffered until open), so a bound widget forwards straight to the audio server with no script round-trip - the browser bypass path. `ClientId::Web` names the in-page origin; `Host::set_server_link` attaches the leg on demand. A throwaway inline HTML/JS harness (`web/index.html`) drives it by emitting the same GuiDef JSON the Python builders emit. Native unchanged (gui 81 tests, `clippy -D warnings` clean native + wasm); verified in Chrome over WebGPU - the console confirms `start` returning the bridge, the binding surface feeding the GuiDef and the host opening the window from the page (`/gui_def 1: window opened from the page`), no panic. Interaction (knob -> `/gui_event`) reuses the shared `interact` path; the `bind` -> `--ws` server bypass is a manual end-to-end test (needs a running `--ws` server). The full TypeScript client stays a separate, unplanned `clients/web` track (the harness here is only to test the host). See `LOG.md`.

**Scope:**

- A `Transport` web impl over the browser-native `WebSocket`: inbound binary frames decode through `osc::decode_packet` (the G1 format - one OSC packet per frame) into `/gui_def`/`/gui_set`/`/gui_free`/`/gui_bind`; outbound `/gui_event`/`/gui_closed` go back as binary frames; the host's audio-server leg rides the same `WebSocket` to a `--ws` server.
- A small wasm-bindgen binding surface on the host (feed an OSC packet / GuiDef in, drain `/gui_event`/`/gui_closed` out), and a **throwaway** inline HTML/JS harness - a few lines, explicitly not a product client - that drives the examples by emitting the same GuiDef JSON the Python builders emit. The point is that the GuiDef authoring and the protocol are reused verbatim; only the carrier and the page glue are new.

**Acceptance:** the same GuiDef a Python client sends to the native host, sent over WS (or handed to the binding surface) in a browser page, opens and drives a browser window; a turned knob/slider emits `/gui_event` back; a `bind`-ed widget drives a `--ws` audio server with no script round-trip (the bypass path, in the browser).

**Note - the full client is its own track, not part of this milestone.** The throwaway harness here is *only* to test the host. A real browser/JS driver belongs to a **TypeScript client** that does not exist yet and is **not planned**: it should live in `clients/web` as its own package (a `clausters` package with the same client model as `clients/python` - GuiDef builders, a `GuiHost`-equivalent, the audio-server client - plus its own docs, examples and tests), and get its **own plan in `clients/web/PLAN.md`** (a parallel client track, the way the Python client has `clients/PLAN.md`). The wasm GUI host (G11-G16) and that TypeScript client are separate deliverables: the host is driven *through* the binding surface / WS, and the client is one more consumer of the same `/gui_*` protocol. Leave this as a forward dependency; do not fold the client into the GUI track.

### G14 - Browser meters/scopes: control buses over the wire

A browser cannot map the shared segment, so the zero-message meter path needs a message-based `BusSource` - a new trait impl, with the meter/scope drawing and the (already time-based) scope sampling reused unchanged.

**Scope:**

- A web `BusSource` fed from the audio server over WS instead of shared memory: the host subscribes to the buses its `meter`/`scope` widgets read and the server streams their values at an interactive rate (a small server-side bus-snapshot/stream command, the network counterpart of the shared segment - note the server addition).
- Feed the existing meter/scope drawing from that source unchanged.

**Acceptance:** a `meter` and a `scope` in the browser track a live control bus over WS, smoothly, against a `--ws` server - the same widgets that read shared memory natively.

### G15 - Browser bulk data: fetch/blob and the `/b_getn` fallback

The mmap bulk path has no browser equivalent; the network primitives the bulk-data decision (G7) deliberately kept become the browser's path. New code is the `BulkLoader` web impl; the `Pyramid`/`WaveformData` consumers and the analysis are reused as-is.

**Scope:**

- A web `BulkLoader`: a waveform/plot `path`/`cache` resolves to a URL fetched as an `ArrayBuffer` (raw `f32` -> samples, or a peak-pyramid cache mapped to the `Pyramid` the renderer already reads); a server `buffer` reference is pulled over `/b_getn` on the WS leg (the existing chunked path) and de-interleaved as natively.
- When only raw samples are fetched, build the peak pyramid in wasm - the analysis already lives in `clausters-core` (in-crate, FFI-free), so it compiles to wasm unchanged.

**Acceptance:** the bulk example's three waveforms (peak cache, raw file, server-exported buffer) render in the browser, fetched/streamed rather than mmap'd, at the same navigation quality (never resolving finer than the screen).

### G16 - Packaging and native/browser parity

Make the wasm GUI host shippable and prove the reuse held. This packages the **host**, not a client: the full TypeScript client is the separate `clients/web` track (see the G13 note), so this milestone uses the throwaway harness, or that client once it lands, to exercise the bundle.

**Scope:**

- A wasm bundle of the GUI host (wasm-bindgen + a thin JS loader; `wasm-pack`/`trunk`) with the binding surface exposed, plus a documented HTML quick-start that loads it.
- A parity pass: the panel, meters and bulk examples run in the browser against a `--ws` server, cross-checked against the native host; the two books cross-link the browser quick-start.

**Acceptance:** a produced bundle loads in a browser and opens the panel, meters and waveform examples against a `--ws` audio server; the same GuiDef yields the same tree and behaviour native and in-browser (the embed/standalone path is explicitly native-only and out of scope here; the product TypeScript client is the separate `clients/web` track, out of scope here too).

### G17 - WebGL2 fallback: browser reach where WebGPU is disabled - DONE (2026-06-28)

Landed out of sequence (a reach fix on the G12 web surface, ahead of G14-G16). The point of the web target is **accessibility/reach**, but WebGPU is unreliable on Linux browsers (the drivers fail and the browser disables it) and on much of Android; depending on WebGPU alone leaves those users with a blank canvas and an error. WebGL2, by contrast, is supported in ~99% of browsers. So the browser host now renders over **WebGPU where it truly works and WebGL2 otherwise**.

**Landed:** the wasm build enables wgpu's `webgl` backend alongside the default `webgpu` (`Cargo.toml`, only in the `wasm32` target deps, so native is untouched); `crate::gpu::new_instance` builds the instance through wgpu's recommended `util::new_instance_with_webgpu_detection` with `Backends::BROWSER_WEBGPU | Backends::GL`, keeping WebGPU only when the browser can actually create a WebGPU adapter (it probes for one, not just for `navigator.gpu` - the Linux-Chrome case where the property exists but no adapter can be made) and dropping to WebGL2 otherwise, with no branch logic of our own; the web `request_device` asks for `Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())` so the device also comes up on a WebGL2 adapter while keeping the adapter's real texture-size maxima (native keeps wgpu's defaults). The "no adapter" messaging (`gpu.rs`, `host::web`, `web/index.html`) now reads in terms of "neither WebGPU nor WebGL2". See `LOG.md`.

**Why it is cheap (the payoff of earlier design):** no renderer, shader or DSP change. The crate already avoids everything WebGL2 cannot do - **no compute shaders and no storage buffers** (only vertex/fragment pipelines, uniform buffers, one `R8Unorm` texture + a linear sampler), the **heavy numeric work (FFT/peaks) runs on the CPU** in `clausters-core`, and the **WGSL shaders (including the `canvas` shader) are translated to GLSL ES 3.0 by naga** automatically (a non-translatable shader already degrades through the existing `push_error_scope`, unpainted, no panic). `R8Unorm` was chosen in G7 for being filterable everywhere, which is WebGL2-safe too.

**Decision (record it):** the browser target is **WebGPU-preferred with a WebGL2 fallback**, not WebGL2-only - WebGPU is kept where the browser genuinely supports it (e.g. Android Chrome) for the better backend, and WebGL2 carries the rest for universal reach. Both backends are compiled into the wasm bundle and chosen at runtime by `new_instance_with_webgpu_detection`; the single-`<canvas>`-context constraint (a canvas takes one of a `webgpu`/`webgl2` context) is handled by that helper deciding the backend before the surface's context is created. Native is unaffected (`Instance::default()`, full default limits).

**Acceptance:** with the built `web/` opened in a Linux browser whose WebGPU is disabled, the host resolves a WebGL2 adapter and renders the panel, bitmap text and inline waveform (and the `canvas` shader) faithfully; in a WebGPU-capable browser it still resolves WebGPU; native is byte-identical (gui 81 tests, `clippy -D warnings` clean native + `wasm32`).

### In-browser audio engine (Web Audio / AudioWorklet) - future track

Not part of G11-G16 and not yet scheduled; recorded here because the G11 seam was deliberately shaped to accept it, and the G11 decision ("no in-process engine in the browser") is a scope boundary that this track relaxes. Through G16 the browser GUI host drives a *separate* audio server over WebSocket. The parallel, larger piece of work is to compile `clausters-server` itself to `wasm32` with a **Web Audio backend**, so the engine runs **in the browser** - the wasm analogue of the native `standalone`/`embed` mode.

- **A new audio backend behind the existing engine seam, not a DSP rewrite.** The engine core (`Engine::process_block`) is already decoupled from the device: it feeds the real-time cpal callback and the offline `render()`/NRT path from the same block function (FTZ armed in both, NRT sample-identical to RT). A browser backend is a *third* driver - an **AudioWorklet** output whose process callback pulls blocks from the engine. cpal does not target Web Audio, so this is a genuine backend addition, which is exactly why it is its own track rather than a step inside G11-G16.
- **An in-process browser link, the wasm `Embed`.** With the engine in the page, the GUI host reaches it through a new `ServerLink` variant (the wasm counterpart of `Embed`): OSC over an in-process channel, **not** WebSocket. This is the same second link kind the native host already has (`Udp` vs `Embed`); the `Transport` trait and the cfg-gated `ServerLink` from G11 take the variant without a protocol change. WebSocket stays the carrier for a *remote* server, so it never disappears - it stops being the browser's only option.
- **The shared-memory paths return inside the browser.** An AudioWorklet runs on its own thread and shares state with the main thread through `SharedArrayBuffer` (which needs cross-origin isolation, COOP/COEP). That is the browser's shared-memory primitive: it can carry the zero-message `BusSource` (control buses read each frame) and the bulk audio path **inside** the page - the same roles `host::shm`/`mapfile` play natively - instead of the WS/`fetch` fallback G14/G15 build for the remote case. So a browser host paired with an in-page engine looks more like the native host than like the remote-server browser.
- **RT-safety carries to the worklet.** The audio thread's no-alloc/no-lock discipline applies to the AudioWorklet thread, and the lock-free command/garbage FIFOs map onto `SharedArrayBuffer` ring buffers. A standalone-style bundle (a GuiDef + GraphDefs) could then boot entirely in a browser tab with no server process at all - the browser twin of `--standalone`.

This intersects the server track, not only the GUI track, so it becomes a numbered milestone on whichever track owns the engine port once its design converges. The product TypeScript client (`clients/web`, see the G13 note) is still a separate concern from both.

## Done outside the numbered tracks

- **Shared config + config-driven standalone.** Server, GUI host and Python
  client read one TOML schema (`clausters-core::config`): a user file under
  `clausters.toml`/`config.toml` overridden by a project `clausters.toml`, then
  CLI flags. The GUI host reads `[gui]`/`[standalone]` as flag defaults, accepts
  `--config <path>`, and `--standalone` with no name falls back to
  `[standalone].gui`. Standalone now lets the embedded server load the whole
  bundle from the data directory (`Clausters::open_with_data_dir` →
  `attach_store`: SynthDefs, Faust defs, GraphDefs, bindings, `boot.json`),
  instead of replaying specs by hand; a `standalone-faust` feature pulls
  `clausters/faust` for Faust bundles. See `LOG.md` and `docs/configuration.md`.

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
