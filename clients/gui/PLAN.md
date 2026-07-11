# Clausters GUI track - implementation plan

Milestones for the graphical-element system: a scriptable set of GUI widgets driven from a dynamic language (Python now, JavaScript later), covering an audio editor, navigable waveform/spectrogram views, custom instrument panels, a live node-tree view, shader canvases, and - later - editable sequencer and music notation. This file is both the staged plan and the design rationale behind it; a settled, published summary of that rationale lives in the server book's `docs/clients.md` ("The GUI host: a scriptable peer"). Like the `PLAN.md` roadmaps, milestone labels (`Gx`) live only here, never in published docs or docstrings.

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

### Why a web-capable rendering substrate

The varied uses (editor, notation, instrument panels) pull toward a web-capable rendering substrate, for three concrete reasons, not fashion:

1. **Scriptability from JS is free** if the host *is* a web surface; Python drives the same surface over the same protocol (WebSocket).
2. **Music notation = Verovio.** Verovio is a C++ engraving library that renders MEI/MusicXML to **SVG**, and ships a **WebAssembly/JS** build. In a web surface it drops in directly; MuseScore (a whole Qt app) is not embeddable, so what we actually want - its notation capability - is Verovio rendering SVG. Editable notation then means making that SVG interactive, which the browser does natively.
3. **One GPU stack, two targets.** `wgpu` *is* the WebGPU implementation. The heavy widgets (waveform, spectrogram) are custom GPU rendering either way; written against `wgpu`/WGSL they run natively today and under WebGPU in a browser unchanged. The prototype in this crate exists to prove that seam.

That substrate gives **one frontend and one GPU stack across all the targets**, which is why the browser staging (G11-G17) is incremental rather than a fork: the native desktop host comes first (fastest to iterate), the browser/WebGPU target is reached by swapping the native surface for a `<canvas>` while the renderers run unchanged, and an optional Tauri wrapper repackages the same web frontend as a native app.

What we explicitly reject: betting the whole thing on a single native Rust toolkit (egui/iced/Vizia/Makepad) as the scriptable layer. That would force us to invent a widget protocol *and* solve Verovio over FFI *and* give up the web - paying all three costs. Those toolkits are excellent for a monolithic Rust app, which is not what this is.

### Generic on the wire, typed in the renderer

The wire form is deliberately *generic* - any `{id, type, props, children}` - so the protocol never changes when a widget type is added. The renderer is the other half of that rule: it turns the generic tree into a *typed* model it knows how to lay out and draw, where adding a type is a new variant plus a handler, not a wire change. An unrecognized type is not an error - it is laid out (reserving its space) but not painted, so a host built today renders the parts of a newer GuiDef it understands and ignores the rest. That keeps the catalog extensible from both ends: the script may send types this host predates, and this host may render types a future script does not yet use.

### The host's two fronts and the windowing thread

The host has a headless protocol front (for tests, automation and no-display machines) and a windowed front; both run the same protocol logic, which is transport- and GPU-agnostic and *returns* its effects (replies, window open/close, repaint) rather than performing I/O, so it is unit-testable without a socket or a GPU. Windowing forces a threading shape: the GPU toolkit (winit) must own the main thread, so the OSC transport runs on a background thread and hands each datagram to the main thread through the event loop's proxy. All host state then lives on one thread, lock-free, mirroring the audio server's single-threaded command loop; replies go back out the shared socket. The per-window *render* is itself one shared, platform-agnostic function (`host::frame::render`): both fronts feed it a tree plus the window's GPU resources, so the browser front (a `<canvas>` WebGPU surface) is pixel-faithful by construction, not a parallel renderer - it differs from the desktop only in the surface, the loop driver and where the live inputs come from (the shared-memory bus natively; the server's `/c_stream` bus snapshots over WebSocket in the browser, filling the same `BusSource` seam). The *interaction* is shared the same way (`host::interact`: hit-test, value/toggle/menu mutation), so a turned knob decides bound-vs-event identically on both.

The typed widget tree is the **single source of truth**, held by the host: the windowed front renders and hit-tests from it and writes interaction results straight back into it, so a script's `/gui_set` and a user's drag are the same edit to one tree and the next frame shows both. A control's value flows out as `/gui_event <id> <value>` to the script that built the window (whose address is captured at `/gui_def`), and a user-closed window as `/gui_closed <id>`; the heavy views send their navigation out the same way.

That "transport- and GPU-agnostic logic" claim is made structural by a **platform seam** (G11): the host is split into a platform-agnostic core (the widget tree, layout, protocol dispatch, the light-widget drawing) that compiles for the browser (`wasm32`) unchanged, and a native I/O shell behind four small traits - `Transport` (send OSC to the audio server), `BusSource` (a control bus, read from shared memory natively), `BulkLoader` (resolve a waveform/plot's local file to samples or a peak pyramid), and `DefStore` (persist named GuiDefs). The browser fills the same seams over its only carrier (WebSocket) and over `fetch`, reusing every line of the agnostic core; `check-wasm.sh` compiles the core for `wasm32` so it can never silently re-couple to native I/O. The in-process embedded server is, *for now*, the one explicitly native-only path - a browser host drives a *separate* audio server over WebSocket. That scope boundary is not a law: the "in-browser audio engine" track below relaxes it through the very same `Transport`/`ServerLink` seam.

### One drawing primitive for the light widgets

The heavy views (`waveform`, the spectrogram/scopes) own custom GPU pipelines; everything else - panel chrome, the control widgets, and the text of labels and values - is built from a single primitive: a batch of flat-colored triangles (rect/quad/line/disc) drawn by one pipeline. A knob is a disc plus a swept pointer; text is a compact embedded bitmap font emitted as one small quad per lit pixel into that same batch - no glyph texture, no second pipeline. So adding a control is composition, not new GPU code, and the heavy-view machinery stays the only place with bespoke shaders. Proportional/large text (a real font rasterizer) is a deliberate later refinement; the fixed-cell font is enough for instrument-panel labels and read-outs.

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
| `clausters-midi` | the MIDI path | Feeds the future `timeline`/DAW sequencing view. |
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
| `waveform` | heavy GPU view | Editor-grade min/max peak waveform of a buffer/file/blob: multichannel lanes (stacked or overlaid), LOD-crossfaded zoom, adaptive rulers, selection, playhead, amplitude zoom/pan, linked navigation groups via `link` (G20-G20d); the RMS body planned (G20e). |
| `spectrogram` | heavy GPU view | Editor-grade STFT time-frequency view, same sources and chrome as `waveform` plus a Hz ruler and frequency zoom/pan, linked groups via `link` (G20-G20d); a constant-Q analysis mode experimental (G20f). |
| `scope` | heavy GPU view | Time-domain scope, both rates: the control-bus history form, and the audio-rate triggered oscilloscope over a server audio tap (a `tap`/`rate: "audio"` prop). |
| `phasescope` | view | Phase/goniometer (Lissajous) view of a stereo tap pair, with a correlation readout (G19). |
| `meter` | heavy GPU view | Level meter reading a control bus directly from shared memory. |
| `spectrum` | view | Live FFT magnitude curve (spectroscope) over an audio tap (G19). |
| `plot` | view | Simple static plot of an NRT-generated signal/file. |
| `nodetree` | view | Live text/graphic view of the audio server's node tree and parameters, updated in real time. |
| `canvas` | view | A surface that runs a supplied WGSL shader, driven by OSC params or server audio - custom visuals. |
| `score` | view | Music notation (Verovio SVG) - **future**, off the GPU path. |
| `timeline` | view | DAW-style tracks + MIDI/OSC sequencing - **future**. |
| `bpf` | view | Drawable break-point-function envelope: breakpoints + per-segment shapes using the server's own `EnvGen` shape numbers (evaluated through `clausters-core`), any `[min, max]` range (bipolar/unipolar/on-off via the hold shape), optional exponential display scale; edits flow back per the edit-back pattern (G21). |

Heavy views never reimplement DSP the server already owns: when a widget needs analysis/processing not already provided by `clausters-server` (peaks, STFT, FFT, resampling), `clausters-gui` reaches for `clausters-ffi`/`libclausters` rather than duplicating signal code. The `clients/gui` crate's own `peaks`/`spectrogram` modules are the prototype of that shared machinery and are a candidate to migrate behind the FFI.

## Heavy widgets: the rendering strategy

An editor-grade waveform or spectrogram cannot draw every sample, and does not need to: it only needs to resolve the signal to the *rendered resolution*. The work is therefore driven by `samples_per_px` (visible length / render width in device pixels), never by buffer length.

### Reusable machinery (the modules)

The navigation and analysis are factored out so the waveform and the spectrogram share them. Core, windowing-agnostic (web-portable) modules:

- `viewport::View` - the visible window in sample units (`f64`), with `zoom`/`pan`/`clamp` and `samples_per_px`. Pure, unit-tested, renderer-agnostic. The spectrogram reuses it verbatim.
- `peaks::Pyramid` - the resolution-matched **min/max peak analysis** (waveform): level 0 summarizes every `base_bucket` samples into a `(min, max)` pair, each higher level halves the resolution, and `level_for(samples_per_px)` selects the level whose bucket matches the zoom so each pixel column reads ~one bucket.
- `spectrogram::Stft` - the **STFT analysis** (spectrogram): `n_frames` x `n_bins` magnitudes from a windowed FFT, the time-domain analogue of the peak pyramid.
- `view::TimelineView` - the trait both views implement (`total_samples`, `upload`, `draw`), so one harness drives either.
- `bytes` - shared little-endian (de)serialization for the caches.
- `waveform`/`spectrogram` renderers - the GPU pieces built on the above.

Native-only helpers (excluded from wasm): `native` (a winit + wgpu windowing harness generic over `TimelineView`) and `demo` (the synthetic test signal). A web build swaps `native` for a `<canvas>` surface and keeps everything else.

### The analysis is a cache (memory or temp file)

Computing the analysis for a long file is the one expensive pass, so both `Pyramid` and `Stft` are treated as caches, the way audio editors keep an overview/peak file beside the audio: each lives in memory and serializes to/from a flat byte buffer (`to_bytes`/`from_bytes`) or a temp/cache file (`write_cache`/`read_cache`) via the shared `bytes` module. The layout is a flat sequence of `f32` arrays so a production build can **memory-map** it instead of reading it into RAM. The peak pyramid is ~2x its level-0 size (a small constant fraction of the source); the STFT is `n_frames * n_bins` floats.

### Bulk data moves through local shared resources, not the wire

Two decisions follow once the data gets large. **First, where the analysis lives.** The peak pyramid and the forward FFT are not GUI-private: a pyramid is general client functionality (any client with a waveform view), and the FFT is shared with the server's `FFT`/`IFFT` UGens. So they live **once** in `clausters-core` (`peaks`, `fft` over `microfft`) and are reached from non-Rust clients through the FFI; this crate's `peaks` is a re-export and the spectrogram calls the core FFT. **Second, how the bytes move.** A multi-megabyte buffer cannot ride an OSC datagram (the ~64 KB cap), and chunking it over `/b_getn` re-traverses the network asynchronously for data already in local RAM. So large payloads move as **memory-mapped files**: a `waveform` names a `path` (raw `f32`) or a `cache` (a prebuilt pyramid) the host maps read-only and reads zero-copy, and a server buffer reaches the same path via `/b_export` (a raw dump to a local file) rather than `/b_getn`. The network reads stay the async fallback, and the browser - which can map neither shared memory nor files - is exactly that fallback's client: `path`/`cache` become URLs fetched against the page origin (the pyramid for raw fetches built in wasm from the same core), and a server buffer rides chunked `/b_getn` over WebSocket through the shared fetch machine (`host::fetch`); the synchronous `BulkLoader` trait stays the native (mmap) fill, since a fetch cannot block. This is the same move the control buses made in the audio path (read from the shared segment, zero messages), generalized to bulk audio.

### Three render regimes (no wasted work, and never "by samples" naively)

Per frame the waveform renderer picks one by `samples_per_px`, so it is always bounded by the screen:

- **Line** (`samples_per_px <= 2`): few enough samples are visible that individual ones matter - draw a polyline through the raw samples in range (vertex count bounded by window width). This is the only regime that touches raw samples directly, and only when it is cheap to.
- **Raw columns** (`2 < samples_per_px < base_bucket`): one min/max column per pixel computed directly from raw samples - exact, and bounded because we are below `base_bucket` samples/px.
- **Pyramid columns** (`samples_per_px >= base_bucket`): one min/max column per pixel read from the peak level matching the zoom.

### The spectrogram: same navigation, constant render cost

The spectrogram is the time-frequency analogue and deliberately reuses the navigation machinery. The STFT magnitudes are uploaded once as a 2D texture (x = time/frame, y = frequency bin). Rendering is a single full-screen quad whose fragment shader samples that texture; `viewport::View` only reshapes the sampled time slice, so the GPU cost is **constant regardless of zoom** (it is bounded by screen pixels, and the one-time analysis is the cache). The GPU's linear filtering gives resolution-matched down-sampling on zoom-out. So the waveform bounds work by *picking a resolution-matched LOD per frame*, while the spectrogram bounds it *structurally* (one texture sample per pixel) - two expressions of the same "never resolve finer than the screen" rule. Display controls (linear/log/mel/bark frequency axis, dB window/contrast, colormap) live entirely in the fragment shader as cheap uniforms, so they change live without re-analyzing; the analysis parameters that *do* require recomputation - window size, hop, sample rate - are arguments to `Stft::compute`.

Notation (`"score"` widget) is out of the GPU path entirely: Verovio-rendered SVG hosted by the web surface, made interactive there.

### Open questions in the rendering strategy

Design-level questions the heavy views still leave open, distinct from the staged milestones above (interpolating between adjacent pyramid levels for smoother zoom-out and the edit-back-to-data pattern have since landed - the LOD crossfade in G20 and the `bpf` editor in G21):

- **Cache lifecycle**: a cache key (source path + mtime + analysis params) and memory-mapping the cache file instead of reading it into RAM.
- **Spectrogram scaling**: time-axis mipmaps or tiling for buffers wider than the max texture size; a smoother (interpolating) log resample.
- **Migrating the rest of the `Stft` machinery behind `clausters-ffi`/`libclausters`** so the signal code lives once: done for `peaks` and the forward FFT (both now in `clausters-core`, the pyramid reachable over the FFI); the `Stft` windowing/normalization and the inverse FFT (for resynthesis UGens) remain, the latter waiting on the server's `FFT`/`IFFT` UGens.

## Status: foundation in place

The `clients/gui` crate (an independent workspace, so it can never break the core build) already validates the heavy-rendering path - the `Gx` work below builds the protocol/host around it. The prototype layout:

```
src/lib.rs           module index
src/viewport.rs      View + zoom/pan (unit-tested)
src/peaks.rs         Pyramid peak analysis + cache (unit-tested)
src/spectrogram.rs   Stft analysis + FFT + cache + renderer (unit-tested)
src/spectrogram.wgsl full-screen quad, texture sample, viridis colormap
src/waveform.rs      WaveformData + WaveformRenderer (3 regimes)
src/waveform.wgsl    passthrough shader (columns + line pipelines)
src/view.rs          TimelineView trait (incl. optional char/vertical hooks)
src/bytes.rs         shared little-endian cache (de)serialization
src/native.rs        winit + wgpu harness driving any TimelineView
src/demo.rs          synthetic test signal
src/bin/waveform.rs      waveform binary
src/bin/spectrogram.rs   spectrogram binary
```

`cargo test` exercises the reusable machinery without a GPU: navigation math, peak correctness vs brute force, FFT correctness (impulse -> flat spectrum, cosine -> peak at its bin), STFT frequency localization, and cache round-trips (memory and temp file). `cargo run --bin waveform` and `cargo run --bin spectrogram` open the two windows (need a display and a Vulkan/Metal/DX12/GL adapter). The split is the point: the renderers take a `wgpu::Device`/`Queue` and a target format and own nothing windowing-specific, so `native` is just a driver, swappable for a `<canvas>` WebGPU surface; adding a view is a new module implementing `TimelineView` plus a one-screen binary, no new windowing or input code. This validates the load-bearing claim: the heavy, custom, GPU-bound widgets can be written once against `wgpu`/WGSL and run both natively and on the web, while the *composition* of widgets is a scripted protocol, not compiled Rust.

And on the core/server side, the transport the web direction needs is already done (G1, below).

## Completed milestones (foundation + desktop host, G1-G10)

Each landed with its detail in the git history; the design rationale that outlived them is in this file's design sections above and `docs/decisions.md`.

- ✅ **G1 - WsHub transport**: a WebSocket transport carrying the existing OSC encoding (`src/osc/ws.rs`, `ClientId::Ws`, the `clausters_ws_*` FFI client), one OSC packet per binary frame through `osc::decode_packet`.
- ✅ **G2 - GUI host skeleton** (`clausters-gui` binary): the dual-role process, headless first — links `clausters-core` (not the server crate) for the shared OSC seam and owns a thin UDP transport front; a generic GuiDef node, the widget registry (node-tree shape), the transport-agnostic command loop, and the Python driver `clausters.gui`.
- ✅ **G3 - `/gui_def` + JSON widget tree + a real window**: the typed widget schema (renderer's interpretation of the generic node), a pure layout engine, and the winit-on-main-thread windowed front rendering the heavy `waveform` with zoom/pan; `--headless` for no-display runs.
- ✅ **G4 - Standard control widgets + `/gui_set` + events**: `slider`/`knob`/`number`/`button`/`toggle`/`menu`/`text` over one mesh `Painter` + a bitmap font (no new GPU code); the typed tree as single source of truth so a live `/gui_set` and a user drag are one edit; interaction emits `/gui_event`/`/gui_closed`.
- ✅ **G5 - GUI as a client of the audio server + shared-memory meters/scopes**: a bidirectional client leg; a read-only `SharedSegment` mmapping the `--shm` segment (versioned ABI checked on attach) so `meter`/`scope` read a control bus each frame with zero OSC; server-buffer waveforms fetched over `/b_getn`. The GUI mirrors the segment's binary ABI rather than linking the server crate.
- ✅ **G6 - Bindings (`/gui_bind`): bypass the script**: a `widget id -> Binding` map so a bound widget forwards its value straight to the audio server; the unbound case keeps emitting `/gui_event`; bindings pruned on free/redefine.
- ✅ **G7 - Bulk data path + shared DSP**: heavy data moves between processes via local shared resources (mmap files, exported server buffers), never re-encoded over OSC (the network reads stay the async fallback); the forward FFT (`microfft`) and the peak pyramid live once in `clausters-core`, peaks reachable over the FFI so a client builds the identical cache the host maps.
- ✅ **G8 - Node-tree view + NRT plots**: `nodetree` (a pure parser of `/g_queryTree.reply`, refreshed off `/notify`) and `plot` (a lightweight decimated static view), both cheap flat-geometry views added by extension, the server untouched.
- ✅ **G9 - Canvas + shaders**: a `canvas` widget running a script-supplied WGSL shader over its area, params driven from OSC and from a control bus read out of shared memory each frame; a failed compile is caught, not fatal.
- ✅ **G10 - Standalone GuiDef + GraphDef bundles**: a bundle (a named GuiDef beside its GraphDefs) boots as a self-contained instrument via `--standalone <name>` against an embedded server, no language client — GuiDefs persist like the server's defs, a root `boot` list and widget `bind` props make a saved tree self-driving. The `standalone` feature links `clausters` (`embed,realtime`); `ServerLink::{Udp,Embed}`.

## Browser / WebGPU target (G11-G17)

The earlier single milestone framed the web host as "swap the winit surface for a `<canvas>` surface; the renderers run unchanged" - true, but it describes the small part. The host is largely native-only I/O glue against a smaller core of genuinely portable renderers/analysis and *pure* host logic (the typed tree, layout, GuiDef parse, the mesh `Painter`/`font`, the registry, the node-tree/scope models, bindings) that only sat behind the `wasm32` exclusion because it was never separated from the I/O glue. So the browser target is not a rewrite: it is **factor the host along the platform seam, then write one new I/O impl per native coupling, reusing everything else** - the protocol, the decode door, the analysis, the renderers and the whole pure core shared verbatim; only the platform shell (transport, bus source, bulk loader, GPU/surface bring-up) gets a second, web impl.

- ✅ **G11 - Host platform seam** (agnostic core + `Platform` traits, wasm build kept green): `pub mod host` made unconditional; only the I/O shell stays `#[cfg(not(wasm32))]`, behind small traits (`Transport`, `DefStore`, `BulkLoader`, `BusSource`). `check-wasm.sh` is the build gate so no later milestone re-couples the core to native I/O. **Decision:** the browser host always talks to a *separate* audio server over WebSocket (no in-process engine in the browser) — a scope boundary the "in-browser audio engine" track below relaxes, and the `Transport`/`ServerLink` seam is shaped to take the wasm-`Embed` variant without a protocol change.
- ✅ **G12 - Web surface**: `<canvas>` WebGPU + async GPU bring-up (no `block_on`) + render loop; the per-window render factored into a shared `host::frame::render` so the browser is pixel-faithful by construction. `web/build.sh` + `web/index.html`.
- ✅ **G13 - Web transport**: drive the browser host live over WebSocket — the browser front runs the real `Host` (shared protocol dispatch + a shared `host::interact`); a wasm-bindgen `GuiBridge` (`feed`/`def`/`poll`/`connect_server`) is the in-page binding surface, and `ServerLink::Ws` gives a bound widget the server bypass. The full TypeScript client stays a separate, unplanned `clients/web` track — the harness here only tests the host.
- ✅ **G14 - Browser meters/scopes**: control buses over the wire — the server grew `/c_stream periodMs bus...` (one subscription per client, transport-agnostic), feeding a message-based `BusSource` (`StreamedBuses`) so the same meter/scope drawing runs from streamed values. Fixed a `/notify`/stream leak on WS/TCP disconnect.
- ✅ **G15 - Browser bulk data**: fetch/blob and the `/b_getn` fallback — the native buffer-fetch state machine extracted into a pure `host::fetch`; on the web `path`/`cache` resolve as URLs (`window.fetch`), the pyramid built in wasm from `clausters_core::peaks`. Fetch fills the `BulkLoader` seam through the event loop (it cannot block).
- ✅ **G16 - Packaging and native/browser parity**: `web/build.sh` + the pinned wasm-bindgen CLI (no wasm-pack/trunk); `web/index.html` the documented harness and `web/parity.html` the scripted headless parity pass; browser quick-start in `docs/clients.md`.
- ✅ **G17 - WebGL2 fallback**: browser reach where WebGPU is disabled — the wasm build enables wgpu's `webgl` backend alongside `webgpu` and picks per-runtime via `new_instance_with_webgpu_detection` (WebGPU where truly supported, WebGL2 otherwise, ~99% reach). Cheap because the crate already avoids compute shaders/storage buffers, runs FFT/peaks on the CPU, and lets naga translate WGSL to GLSL ES.

## Widget deepening (G18-G21): scopes, editor-grade views, edit-back

With the host complete on both platforms, the next arc deepens the **graphical elements themselves** - the catalog entries once marked *future* (`scope` as a real oscilloscope, `phasescope`, `spectrum`, `bpf`) plus the editor-grade refinement of the two heavy views and the edit-back-to-data pattern. The ordering criterion: every one of these is **driven from the Python client as it stands** (new `clausters.gui` builders over the unchanged `/gui_*` protocol - widgets are added by extension, never by protocol change), so each milestone lists its Python leg explicitly. The per-widget domain knowledge (trigger algorithms, goniometer geometry, LOD crossfade, envelope-shape math) lives in the `gui-widgets` skill; this plan stages the work. Packaging (Tauri) and the in-browser audio engine stay deliberately last - see "Future directions".

**Recurring analysis (every milestone in this arc, the G7b rule):** each new compute function gets an explicit placement decision, recorded with the milestone - **general** (useful to another client from Python, or to a future server feature) goes to `clausters-core`, with a `clausters-ffi` export when a non-Rust client consumes it; **display-only** (hit-testing, tick spacing, trigger alignment of a drawn window) stays in the gui crate. Peaks, the forward FFT and the windows already took the core path; of the known candidates below, the correlation metric (G19), the multichannel peak cache (G20) and the envelope shape math (G21, `clausters_core::envshape` with the server's `EnvGen` delegating; its FFI export deferred until a client evaluates envelopes client-side — the tap-reader precedent) have since landed core-side, and the G20c tick/label layout was analyzed display-only (gui-side) — leaving the per-bucket RMS descriptor (G20e) and the constant-Q transform (G20f, experimental).

- ✅ **G18 - Server audio tap + a real oscilloscope**: tap rings in the versioned `--shm` segment (ABI v2 → v3), created by a `/tap tapIndex bus` command (not a UGen), with the browser sibling `/tap_stream` → `/tap_data` (a windowed blob per period). The `scope` gained an audio-rate mode (`tap`/`rate:"audio"`) with a hysteresis level trigger. The trigger search is display-only (gui-side); the ring reader's core move is deferred until a client needs to map-read taps.
- ✅ **G19 - Phasescope + live spectrum**: two tap-consuming views added by extension — `phasescope` (the 45°-rotated Lissajous with an age-faded trail + a Pearson-r bar) and `spectrum` (one forward FFT per frame, dB with adjustable window, log/linear axis, averaging + peak-hold). The new general functions — correlation and Lissajous geometry — landed in `clausters_core::measure` with FFI exports (ABI v6 → v7); display stays gui-side. Closes the catalog's four *future* scope entries with G18.
- ✅ **G20 - Editor-grade waveform + spectrogram**: the two heavy views at editor depth and the `spectrogram` finally host-wired. **Multichannel** as one cache file (`MultiPyramid`, CLPK v2, FFI CORE_ABI v7 → v8) so the client builds the identical cache; lanes stacked or overlaid. **LOD crossfade** (blend the two adjacent pyramid levels). **Rulers** (adaptive 1-2-5 time axis; the spectrogram's Hz ruler by inverting the shader's display→bin mapping). **Selection + playhead + readout** as shared `EditorProps` on both kinds, drawn in a second overlay pass through the one `frame::render` (plain drag selects, Shift+drag pans).
- ✅ **G20b - Configurable rulers: units per axis + side strips**: every axis is a configurable ruler in **its own strip** (bottom for x, left for y, independently optional). Time axis gains `"beats"` (`bar:beat` on the client's grid); the waveform amplitude axis gains `"db"`/`"bits"`/`"percent"`; `log_freq` grew into `freq_scale` = linear/log/mel/bark (new shader mappings). All live `/gui_set`. **Placement:** hertz↔mel/bark → `clausters_core::scale`, the bar/beat reads → `clausters_core::tempoclock`, six exported over FFI (CORE_ABI v8 → v9); tick spacing and the ladders stay gui-side.
- ✅ **G20c - Editor views: adaptive ruler layout + vertical zoom/pan**: the fixed pixel constants in `host/ruler.rs` gave way to a layout that **measures its labels** (`font::width`/`height` at the ruler scale, device pixels): each unit's ladder (1-2-5 decimal, musical, dB rungs, decades) is tried against its *own* formatted labels and the smallest colliding-free step wins (property test: no two labels overlap for any window/zoom/unit/strip size). Every tick generator now lays out over the **visible sub-range** of its axis, and the heavy views gained that vertical window: `y_start`/`y_len` in normalized display units on `EditorProps` (live `/gui_set`, `"view_y"` events; non-positive `y_len` resets) — the waveform maps its geometry through the visible amplitude slice, the host `spectrogram` drives `SpectrogramView`'s display-coordinate frequency window, so the cursor anchor holds across linear/log/mel/bark and the browser keeps display + `/gui_set` parity while gestures stay native (wheel on the y-ruler strip zooms, dragging it pans, `R` resets — see `docs/decisions.md`). **Placement:** tick spacing and label fitting are pixel concerns — display-only, gui-side.

- ✅ **G20d - Linked views: shared navigation groups, composable editor items**: the per-widget navigation/selection/playhead state (formerly a per-slot `View` in each front) extracted into one **group model owned by the host core** (`host/timeline.rs`: horizontal `View` + selection + playhead anchor per `GroupKey`); the GPU slots keep only pixels, per-widget state keeps only the y axis (G20c). Grouping is an **explicit `link` (int) prop** (decision recorded: an item may want *unlinked* views of one file, and a link may span sources), live via `/gui_set` (negative = unlink, carrying the current view along; joining adopts the group). A gesture on any member mutates the group and every member window repaints; `/gui_set` of `view_start`/`view_len`/`sel_*`/`playhead_at` on any member applies group-wide (the interception lives in the core `on_set`, so the browser gets it for free); `"view"`/`"selection"` events emit **once**, carrying the **interacted member's id** (decision: a designated group id is not a widget id and would break the `/gui_event` shape). Selection/playhead are **mirrored** group→members' `EditorProps` (single writer, no drift); shared chrome stays plain composition - a linked lane under a ruled sibling sets `ruler:"off"`, no auto strip, no new container. The group navigates in **session-timeline units** with its length aggregated (max) over member extents and membership spanning windows - deliberately the seat of the future multitrack view, whose clip items become members with a per-member *placement* (offset), the one designed extension point (all members at offset 0 today). Placement: navigation/group state is protocol/display logic - gui-side host core, nothing for `clausters-core`. Python: `link=` on both heavy builders; `examples/gui_linked.py` composes a waveform + spectrogram of one NRT render in lockstep, single event stream, live re-linking. See `docs/decisions.md`.

- ✅ **G21 - BPF envelope editor + the edit-back-to-data pattern**: the first widget that *writes data back*, and the pattern it establishes (folding in the former "Edit-back-to-data" and "Automation / BPF view" future directions). The **`bpf` widget**: breakpoints `(time, value)` plus a per-segment shape/curve **using the server's own envelope shape numbers**, evaluated once per pixel column (painter geometry); interaction hit-tests points before segments — drag to move (times clamped monotonic), drag a segment vertically for curvature, Ctrl+click adds a point / removes the one under it; the model is deliberately the future automation-lane shape — any `[min, max]` range (bipolar/unipolar; an on/off lane is the hold shape — SC's step jumps to the target at segment start) with an optional exponential display scale (`exp`), times over `[0, duration]` (0 = fit). **Placement (recorded):** the shape evaluation relocated to `clausters_core::envshape` with the server's `EnvGen` delegating to it (the FFT move); its FFI export is **deferred** until a client actually evaluates envelopes client-side (the tap-reader precedent — the Python leg maps breakpoints to `Env`, it does not evaluate curves). Breakpoint model/hit-testing/drag stay gui-side (`host/bpf.rs`, pure and unit-tested). **The edit-back pattern (recorded):** edited data flows back **to the script** as `/gui_event <id> "points" <t v shape curve ...>` — flat OSC primitives, ints kept int, new event *payloads* rather than new addresses (bulk data would ride one blob in the `samples_to_blob` layout) — and **to the server** through the binding path, `Host::forward_args`/`Binding::message_args` generalizing the bound widget-value forward to a flat list after the fixed prefix. The host's mapped resources stay **read-only**; editing gestures are native-only today with display + `/gui_set` parity in the browser (the editor-view posture). Python: the `bpf` builder plus `env_to_points`/`points_to_env` mapping to/from `clausters.defs.Env`; `examples/gui_bpf.py` draws an envelope and hears an `EnvGen` play it. See `docs/decisions.md`.

### G20e - RMS waveform layer

The third classic waveform layer: the RMS body drawn inside the min/max envelope, the editor convention that separates dense peaks from perceived level.

**Scope:**

- **Placement analysis (the G7b rule, decided up front):** per-bucket RMS is a **general audio descriptor** - the Python client analyzing arrays/chunks headlessly and a future server analysis UGen/command both plausibly consume it, exactly the user-noted case - so it lands in `clausters-core`: an `rms` measure over slices/chunks (beside `measure::correlation`) and the peak pyramid growing per-bucket energy - CLPK v2 → v3 (v1/v2 caches still parse), the FFI exports bumped so `clausters.gui.peaks_cache_file` keeps building the identical cache the host maps. Only the drawing (the body layer, its color, the regime blend) stays gui-side.
- **Energy-exact aggregation (decision - record it):** store mean-square (or sum-of-squares) per bucket internally and expose RMS, so higher pyramid levels combine exactly (the RMS of a union is the root of the mean of the mean-squares) and the LOD crossfade blends without bias.
- **Rendering:** a second column layer inside the min/max envelope (±rms about zero, the editor convention) with its own color, LOD-crossfaded exactly like min/max; an `rms` prop toggling it live via `/gui_set`. **Decision (record it):** the zoomed-in Line regime (raw samples on screen) - fade the body out with the existing crossfade weight vs. hard-off below `base_bucket`.
- Python: the cache builder and `waveform` builder grow the RMS surface; the descriptor exposed through the `_native` wrappers for headless use; an example (or `gui_editor.py` extended).

**Acceptance:** the waveform draws the RMS body inside the min/max envelope with no pop across LOD switches; a Python-built v3 cache and a host-built pyramid draw identically; core RMS matches brute force on random chunks; v1/v2 caches still load.

### G20f - Constant-Q spectrogram - **experimental**

An exploratory alternative analysis for the `spectrogram` widget: a constant-Q transform (geometrically spaced center frequencies, window length inversely proportional to frequency, so Q = f/Δf is constant) gives uniform per-octave resolution - the musically faithful time-frequency picture the log-scaled STFT only approximates (an STFT's low bins are linearly spaced, so the log display stretches a handful of coarse bins across the bottom octaves). Experimental: it lands behind a prop, is evaluated for quality/cost against the STFT views, and is promoted or dropped on that evidence - the STFT stays the default either way.

**Scope:**

- **The analysis**: a CQT over the same sources, parameterized by `bins_per_octave` (12-48 typical), `fmin` and the source's Nyquist. **Decision (record it):** the algorithm - the Brown/Puckette FFT-based sparse-kernel CQT vs. the multi-rate hybrid (one STFT per octave, window/hop halving per octave, resampled or restacked) - decided by measured compute cost on editor-length files and by wasm friendliness (CPU-only, no threads assumed in the browser). Long-file capping follows the existing posture (`hop_capped`'s role) so the magnitude grid fits the 8192-wide texture.
- **Placement analysis (the G7b rule):** the transform is general audio analysis (Python chunk analysis and any future server spectral feature are plausible consumers, exactly like the forward FFT) - it lands in `clausters-core` beside `fft`/`window`, with the `Stft` pipeline shape (compute → magnitude grid → cache/texture) and an FFI export **deferred until a client actually consumes it** (the tap-reader precedent: record the decision, move on evidence). Display mapping, ruler ticks and props stay gui-side.
- **The widget surface**: a prop on the existing `spectrogram` kind (`analysis: "stft"` default / `"cqt"` plus the CQT params), extension by props - no new widget kind, no protocol change. Rendering reuses the full-screen-quad pipeline unchanged: the magnitude grid uploads as the same 2D texture; the rows are already geometrically spaced, so the shader's display→bin mapping is the identity on the log axis (and `freq_scale` interacts accordingly - **decision:** which scales stay meaningful under CQT rows, likely log/identity only). The frequency ruler inverts the bin→Hz law `f(k) = fmin·2^(k/bpo)`; with bins on the equal-tempered grid a note-name unit (`"note"`: C4, A4...) becomes nearly free - **decision (record it):** whether the note ruler ships with this milestone or waits for promotion.
- Python: the `spectrogram` builder grows the analysis props; an example comparing STFT and CQT views of the same harmonic material.

**Acceptance (experimental bar):** a CQT view of a bass-register arpeggio resolves the individual partials the log-STFT smears, with correct ruler/readout frequencies across zoom (G20c) and linked navigation (G20d); compute cost on an editor-length file is measured and recorded; the promote-or-drop decision is written down with the evidence.

## Multitrack timeline view (G22): the DAW-style track editor

The "DAW / timeline view" future direction, now firming up. It builds on exactly the seat G20d designed and named: the navigation group already navigates in **timeline sample units** with its length aggregated over member extents, and its own doc block declared "a clip item there is a member with a *placement* (offset) on the same shared timeline — G20d ships every member at offset 0; the per-member placement is the one designed extension point." G22 activates that point and draws on top of it, reusing the group model, the `EditorProps` chrome, the second overlay pass of `host::frame::render`, the `host::interact` hit-test, the heavy views as clip bodies, and the G21 edit-back pattern — no protocol change (a widget is added by extension). Staged in incremental, separately-shippable parts, each with its Python leg. **Placement analysis (the G7b rule, decided up front):** clip/track geometry, the offset arithmetic, hit-testing and snapping are protocol/display logic — **gui-side** (host core + `interact`), nothing for `clausters-core`, the same call G20d made for navigation state.

- ✅ **G22a - Per-member placement (the offset)**: the extension point G20d declared, activated as pure host-core plumbing. An `offset` (timeline samples) on `EditorProps` — per-member (unlike the group-wide `link`/`sel_*`/`view_*`), so a view's own data sample 0 sits at timeline position `offset`; a placed member lengthens its group timeline to `offset + extent` (`timeline_total` aggregates the *placed* extents). A change routes through the group model (`set_timeline_offset`: writes the one widget's offset, re-clamps the shared window with the same full-follows-growth rule as `set_timeline_total`, repaints every member) and rides `/gui_set offset` as a timeline key. The GPU body upload draws through the **placed** window (`frame::placed_nav` shifts the group window by `-offset`); the time ruler and the selection/playhead overlay keep the timeline-unit window. At `offset = 0` (the un-placed default) everything is the identity, so no existing view regresses. Tests: the placement lengthens the group and re-clamps a zoomed window; `placed_nav` shifts the body window; the offset parses/clamps.
- ⬜ **G22b - `track`/`clip` widgets + the graphic-unit overlay** (the visible milestone): a new `WidgetKind::Track` lane container (arbitrary vertical = stacked clips via `frame::lane_rect`); a clip is an existing editor view placed by G22a plus a `dur` bounding its data on the timeline. The second overlay pass of `frame::render` draws track headers + clip rectangles (x from `offset` via `sample_to_x`, width from `dur`) reusing the `EditorProps` chrome; a collapsed group clip → a labelled rectangle, expanded → its heavy body inside the clip rect. Scalar vertical (a `track` of events → piano-roll) defers to G22d. Python: `track`/`clip` builders; smoke via `Session.gui()` with two tracks (refresh the bundled binary first).
- ⬜ **G22c - Clip edit-back (drag/resize)**: `host::interact` grows clip hit-testing on the overlay — drag the body → move `offset` (clamped >= 0, snapped to the grid via `editor.quant`); drag an edge → resize `dur` (edges before body, as `bpf` prioritizes points over segments). Emits `/gui_event <id> "clip" <offset> <dur>` — flat OSC primitives, a new event *payload* not a new address, through `Host::forward_args`, exactly the G21 pattern; the widget tree stays the source of truth (the edit writes `offset`/`dur` and repaints). The bound path (forward to the server to re-schedule) and the model re-realize are the Fase 3 driver's job. The host's mapped resources stay read-only (the editor-view posture).
- ⬜ **G22d - Events track → piano-roll** *(later sub-milestone)*: a `track` of events drawn as note lanes (vertical *scalar* by pitch) — genuinely new geometry/heavy view (no events view to reuse), so it follows the reused-body parts, the way G20f stayed experimental. If it needs pitch↔y or note names it reuses `clausters_core::scale`/`tempoclock` (G20b), not a reimplementation.

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
  `clausters/faust` for Faust bundles. See `docs/configuration.md`.

## Future directions (to fold into milestones as they firm up)

Captured here so the depth the editor-grade vision needs is not lost; each becomes a `Gx` when its design converges. (The former entries for scopes, the editor-grade views, edit-back-to-data and the BPF view converged into G18-G21 above.) **Ordering (decided):** the widget-deepening arc comes first because everything in it is immediately usable from the installed Python client over the existing protocol; the timeline and notation views follow as their designs firm up; **packaging and the in-browser audio engine are deliberately last** - they change how the system ships, not what it can show, and both keep constraining design in the meantime (the web frontend must stay Tauri-wrappable; the `Transport`/`ServerLink` seam must keep the wasm-engine variant open).

- **DAW / timeline view.** Now firming up as the **G22** arc above (per-member placement, `track`/`clip` widgets, clip edit-back, an events piano-roll). Tracks with audio and MIDI/OSC sequencing; since the audio lives in the server, the view reads it from there. Builds directly on G20d's group navigation/placement seat and G21's edit-back pattern.
- **Notation (`score`).** Verovio (C++ -> wasm/JS) rendering MEI/MusicXML to interactive, editable SVG in the web surface, off the GPU path entirely.
- **Packaging.** An optional Tauri desktop wrapper reusing the web frontend; the GUI chapter in the docs; worked examples and `GUIA.md` steps.
- **In-browser audio engine.** The Web Audio / AudioWorklet track recorded in its own section below - it intersects the server track and is numbered on whichever track owns the engine port once its design converges.

## In-browser audio engine (Web Audio / AudioWorklet) - future track

Not part of G11-G16 and not yet scheduled; recorded here because the G11 seam was deliberately shaped to accept it, and the G11 decision ("no in-process engine in the browser") is a scope boundary that this track relaxes. Through G16 the browser GUI host drives a *separate* audio server over WebSocket. The parallel, larger piece of work is to compile `clausters-server` itself to `wasm32` with a **Web Audio backend**, so the engine runs **in the browser** - the wasm analogue of the native `standalone`/`embed` mode.

- **A new audio backend behind the existing engine seam, not a DSP rewrite.** The engine core (`Engine::process_block`) is already decoupled from the device: it feeds the real-time cpal callback and the offline `render()`/NRT path from the same block function (FTZ armed in both, NRT sample-identical to RT). A browser backend is a *third* driver - an **AudioWorklet** output whose process callback pulls blocks from the engine. cpal does not target Web Audio, so this is a genuine backend addition, which is exactly why it is its own track rather than a step inside G11-G16.
- **An in-process browser link, the wasm `Embed`.** With the engine in the page, the GUI host reaches it through a new `ServerLink` variant (the wasm counterpart of `Embed`): OSC over an in-process channel, **not** WebSocket. This is the same second link kind the native host already has (`Udp` vs `Embed`); the `Transport` trait and the cfg-gated `ServerLink` from G11 take the variant without a protocol change. WebSocket stays the carrier for a *remote* server, so it never disappears - it stops being the browser's only option.
- **The shared-memory paths return inside the browser.** An AudioWorklet runs on its own thread and shares state with the main thread through `SharedArrayBuffer` (which needs cross-origin isolation, COOP/COEP). That is the browser's shared-memory primitive: it can carry the zero-message `BusSource` (control buses read each frame) and the bulk audio path **inside** the page - the same roles `host::shm`/`mapfile` play natively - instead of the WS/`fetch` fallback G14/G15 build for the remote case. So a browser host paired with an in-page engine looks more like the native host than like the remote-server browser.
- **RT-safety carries to the worklet.** The audio thread's no-alloc/no-lock discipline applies to the AudioWorklet thread, and the lock-free command/garbage FIFOs map onto `SharedArrayBuffer` ring buffers. A standalone-style bundle (a GuiDef + GraphDefs) could then boot entirely in a browser tab with no server process at all - the browser twin of `--standalone`.

This intersects the server track, not only the GUI track, so it becomes a numbered milestone on whichever track owns the engine port once its design converges. The product TypeScript client (`clients/web`, see the G13 note) is still a separate concern from both.

## Definition of done (per milestone)

Following the project rule: code + tests, a clear commit message (the record of *what* shipped) and this file's checkbox updated; developer/user docs where the feature touches them; a commented example when the feature is user-facing; a `docs/decisions.md` note only when a choice has non-obvious context; and a `GUIA.md` smoke step only when a new human-audible/visual behavior appears.
