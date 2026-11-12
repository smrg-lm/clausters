# Clausters GUI track - implementation plan

Milestones for the graphical-element system: a scriptable set of GUI widgets driven from a dynamic language (Python now, JavaScript later), covering an audio editor, navigable waveform/spectrogram views, custom instrument panels, a live node-tree view, shader canvases, editable music notation, and - later - an editable sequencer. This file is both the staged plan and the design rationale behind it; a settled, published summary of that rationale lives in the server book's `docs/clients.md` ("The GUI host: a scriptable peer"). Like the `PLAN.md` roadmaps, milestone labels (`Gx`) live only here, never in published docs or docstrings.

## What `clausters-gui` is (the naming)

`clausters-gui` is **two roles in one process**, and the plan only makes sense once both are explicit:

- **A GUI client of the audio server.** It talks to `clausters-server` over OSC exactly the way `clausters-python` does - same encoding, same transports (UDP/TCP/shared-memory ring/WebSocket), same `osc::decode_packet` door. It reads buffers, control buses, the node tree, and sends control messages. Anything `clausters-python` can do against the server, `clausters-gui` can do too.
- **A GUI server (host) for the languages.** It owns the windows, the widgets and the GPU, and exposes a widget protocol that the language clients drive. `clausters-python` and the web client send it a serialized JSON document - a **GuiDef**, built the same way a `SynthDef`/`GraphDef` is - to construct a window/widget tree, then drive live updates and receive interaction events.

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

- **OSC is the single encoding everywhere.** A whole window/widget tree rides **as JSON inside an OSC argument**, exactly as a `SynthDef`/`GraphDef` does today through `/def_send synth` (`serde_json::from_slice` over an `OscType::String` or `OscType::Blob`; the server already accepts both - `crates/clausters-server/src/osc/translate`). "JSON vs OSC" is a false dichotomy: JSON is the payload, OSC is the framing. serde's number handling distinguishes integers from floats, so ids stay `i32` and control values stay `f32` across the wire, the same rule the audio server relies on.
- **Construction is declarative; updates are incremental.** A GUI is a *def*: `/gui_def <id> <json tree>` builds the whole tree in one message (like a `SynthDef`), `/gui_set <id> ...` updates one live widget (like `/node_set`), `/gui_free <id>` frees a subtree. No per-widget construction chatter.
- **The GUI host is a sibling OSC front, not code in the audio server.** It reuses the existing transport layer (UDP/TCP/shared-memory ring/WebSocket, the `osc::decode_packet` door, `ClientId`) with a widget/GPU command interpreter in place of the audio engine. The audio server stays headless and RT-safe and is untouched by any of this.
- **The GUI host is also a client of the audio server.** It reads buffers, sends bound-widget values, mirrors the node tree, and - the payoff of the shared-memory IPC - reads the control buses **directly from the shared segment every frame**, so meters/scopes update with zero messages.
- **WebSocket is one more transport, primarily for the browser.** A browser cannot map shared memory or open raw UDP; it can only do WebSocket. The desktop host reuses the existing transports; the browser host uses `WsHub` (already in the core, see G1) - the same OSC encoding through the same decode door, not a new protocol.
- **Heavy widgets are the existing `wgpu` renderers.** The waveform and spectrogram are written once against `wgpu`/WGSL (the `TimelineView` trait) and run natively today and under WebGPU in a browser unchanged.

The widget tree is a hierarchy with client-allocated integer ids and subtree freeing - the shape of a DOM / scene graph (and, incidentally, the shape of the server's group/node tree), reached as a declarative document rather than mutated node by node.

### Crate / packaging boundary

`clausters-gui` is its **own crate**, kept out of the root `clausters` workspace so it can never break the core server build (the `crates/clausters-gui` crate already validates this). For end users it ships as a **separate package/binary** from the audio server and the Python client, because the GPU/windowing stack (`wgpu`, `winit`, font/shader assets) is large and should not bloat a headless server install or a `pip` wheel. The two move and version independently.

### Why a web-capable rendering substrate

The varied uses (editor, notation, instrument panels) pull toward a web-capable rendering substrate, for three concrete reasons, not fashion:

1. **Scriptability from JS is free** if the host *is* a web surface; Python drives the same surface over the same protocol (WebSocket).
2. **Music notation = Verovio.** Verovio is a C++ engraving library that renders MEI/MusicXML to **SVG**, and ships a **WebAssembly/JS** build. MuseScore (a whole Qt app) is not embeddable, so what we actually want - its notation capability - is Verovio's engraving. *(This originally read as an argument for a web surface: drop the JS build in and let the browser make the SVG interactive. G31 found the better seam - the SVG is parsed into a display list on the **client** side and drawn as ordinary mesh geometry, so notation costs the host no dependency and reaches the browser like every other widget. The argument for the substrate stands on 1 and 3; this point no longer carries it.)*
3. **One GPU stack, two targets.** `wgpu` *is* the WebGPU implementation. The heavy widgets (waveform, spectrogram) are custom GPU rendering either way; written against `wgpu`/WGSL they run natively today and under WebGPU in a browser unchanged. The prototype in this crate exists to prove that seam.

That substrate gives **one frontend and one GPU stack across all the targets**, which is why the browser staging (G11-G17) is incremental rather than a fork: the native desktop host comes first (fastest to iterate), the browser/WebGPU target is reached by swapping the native surface for a `<canvas>` while the renderers run unchanged, and an optional Tauri wrapper repackages the same web frontend as a native app.

What we explicitly reject: building the host on a Rust GUI toolkit (egui/iced/Vizia/Makepad), whether as the scriptable layer or as the drawing backend under the protocol. *(Corrected 2026-10-08: this paragraph used to give three reasons, and two of them do not hold. Such a toolkit does not give up the web -- egui, for one, runs in a page through wasm over wgpu or WebGL; and notation never needed Verovio over FFI, since G31 engraves on the client and the host draws a display list.)* The reason that stands is the shape of the host. A toolkit gives the drawing and the input of widgets compiled into one program; what this host is made of is a widget tree that arrives over a protocol and stays the single source of truth, heavy views that are our own `wgpu` pipelines, and sizing rules of its own (the declared cell, integer text scales, pixel snapping). Under a toolkit the protocol, the tree, the heavy views and the gestures would still be ours to write, and the toolkit's layout model would be a second one to keep in agreement with the host's. What the decision costs is the commodity layers a toolkit ships -- a real typeface (K10), focus and the Tab ring (K6), menus and popups (G37), scrolling (G40), the clipboard (G42), composed input (IME, closed 2026-08-27) -- which the host has had to build itself; G43 is the one of them still open.

### Generic on the wire, typed in the renderer

The wire form is deliberately *generic* - any `{id, type, props, children}` - so the protocol never changes when a widget type is added. The renderer is the other half of that rule: it turns the generic tree into a *typed* model it knows how to lay out and draw, where adding a type is a new variant plus a handler, not a wire change. An unrecognized type is not an error - it is laid out (reserving its space) but not painted, so a host built today renders the parts of a newer GuiDef it understands and ignores the rest. That keeps the catalog extensible from both ends: the script may send types this host predates, and this host may render types a future script does not yet use.

### The host's two fronts and the windowing thread

The host has a headless protocol front (for tests, automation and no-display machines) and a windowed front; both run the same protocol logic, which is transport- and GPU-agnostic and *returns* its effects (replies, window open/close, repaint) rather than performing I/O, so it is unit-testable without a socket or a GPU. Windowing forces a threading shape: the GPU toolkit (winit) must own the main thread, so the OSC transport runs on a background thread and hands each datagram to the main thread through the event loop's proxy. All host state then lives on one thread, lock-free, mirroring the audio server's single-threaded command loop; replies go back out the shared socket. The per-window *render* is itself one shared, platform-agnostic function (`host::frame::render`): both fronts feed it a tree plus the window's GPU resources, so the browser front (a `<canvas>` WebGPU surface) is pixel-faithful by construction, not a parallel renderer - it differs from the desktop only in the surface, the loop driver and where the live inputs come from (the shared-memory bus natively; the server's `/bus_stream` bus snapshots over WebSocket in the browser, filling the same `BusSource` seam). The *interaction* is shared the same way (`host::interact`: hit-test, value/toggle/menu mutation), so a turned knob decides bound-vs-event identically on both.

The typed widget tree is the **single source of truth**, held by the host: the windowed front renders and hit-tests from it and writes interaction results straight back into it, so a script's `/gui_set` and a user's drag are the same edit to one tree and the next frame shows both. A control's value flows out as `/gui_event <id> <value>` to the script that built the window (whose address is captured at `/gui_def`), and a user-closed window as `/gui_closed <id>`; the heavy views send their navigation out the same way.

That "transport- and GPU-agnostic logic" claim is made structural by a **platform seam** (G11): the host is split into a platform-agnostic core (the widget tree, layout, protocol dispatch, the light-widget drawing) that compiles for the browser (`wasm32`) unchanged, and a native I/O shell behind four small traits - `Transport` (send OSC to the audio server), `BusSource` (a control bus, read from shared memory natively), `BulkLoader` (resolve a waveform/plot's local file to samples or a peak pyramid), and `DefStore` (persist named GuiDefs). The browser fills the same seams over its only carrier (WebSocket) and over `fetch`, reusing every line of the agnostic core; `check-wasm.sh` compiles the core for `wasm32` so it can never silently re-couple to native I/O. The in-process embedded server is, *for now*, the one explicitly native-only path - a browser host drives a *separate* audio server over WebSocket. That scope boundary is not a law: the "in-browser audio engine" track below relaxes it through the very same `Transport`/`ServerLink` seam.

### One drawing primitive for the light widgets

The heavy views (`waveform`, the spectrogram/scopes) own custom GPU pipelines; everything else - panel chrome, the control widgets, and the text of labels and values - is built from a single primitive: a batch of flat-colored triangles (rect/quad/line/disc) drawn by one pipeline. A knob is a disc plus a swept pointer; text was first a compact embedded bitmap font emitted as one small quad per lit pixel into that same batch - no glyph texture, no second pipeline. So adding a control is composition, not new GPU code, and the heavy-view machinery stays the only place with bespoke shaders. The bitmap face is still the floor every build draws with; a real typeface came later, as planned, behind the `font-atlas` feature (K10, on by default), and it adds one atlas texture and a textured pipeline drawn inside the same batch rather than a second batch.

## Guiding principle: serve the server and its clients, and reuse what exists

Two constraints sit above every milestone:

- **A GUI must never pull focus from the audio server and its clients.** The point of the project is a headless, RT-safe server driven by OSC clients; `clausters-gui` is one more client and one more front, *in service of* that model, not a parallel universe. So the GUI adopts the server's semantics rather than inventing its own (OSC framing, the def model, the node-tree/id-allocation/subtree-free shape, the flat-primitives-at-the-boundary rule, the single `decode_packet` door), the audio server stays untouched and RT-safe, and no GUI convenience is allowed to fork the wire protocol or the client model. When a feature could live in the server/clients or in the GUI, it lives where the rest of the system already keeps it.
- **Stand on the existing crates; do not reimplement.** A large amount of what the GUI needs already exists in the server and the complementary crates and is meant to be reused. The default answer to "where does this behaviour come from?" is an existing module, behind the FFI when it is DSP.

Concrete reuse map (the things already implemented that the GUI builds on):

| Source | What it already provides | How the GUI reuses it |
|---|---|---|
| `clausters-core::osc` | UDP/TCP/WS/shared-ring transports, `decode_packet`, `ClientId`, OSC encode/bundle assembly, `WsHub` | Both legs: the host's server front (script -> gui) and its client leg (gui -> audio server) ride the *same* transports and decode door; no new protocol. |
| `clausters-core` def model (`SynthDef`/`GraphDef`/`d_recv`/defstore) | JSON-in-OSC defs, serde int/float distinction, on-disk persistence | The GuiDef *is* this pattern; `/gui_def` mirrors `/def_send synth`, and GuiDef persistence reuses the def-store machinery (the standalone mode, G10). |
| `clausters::embed` + shared-memory ring (the embed ABI) | In-process server, control-bus shared segment, garbage/command FIFOs | Zero-message meters/scopes read the shared buses each frame (G5); the standalone app embeds the server (G10). |
| `clausters-ffi` (C ABI, flat data) | `clausters_ws_*` client, timing/sample conversion, seeded RNG, numeric builtins | The host's audio-server-client leg and any DSP it needs go through the FFI, exactly as `clausters-python` does - one implementation, not two. |
| `clausters-midi` | the MIDI path | Feeds the future `timeline`/DAW sequencing view. |
| Python client (`session.Server`, `responders` `OscFunc`/`MidiFunc`, `ipc`, `_native`) | the reference client model and the encoder over the FFI | The script drives `clausters-gui` the same way it drives the server; the GUI's client leg reuses the client encoder rather than a parallel one; events mirror the responder model. |
| node-tree / group-node semantics (scsynth model) | client-allocated ids, add-actions, subtree free, `/g_query`-style introspection | The widget tree reuses this shape verbatim; the `nodetree` view (G8) reads the server's real tree through its existing query/notification path. |
| `crates/clausters-gui` crate renderers (`viewport`/`peaks`/`Stft`/`TimelineView`/`bytes`) | the heavy GPU views and resolution-matched analysis with a cache | The heavy widgets; the analysis is a candidate to migrate behind the FFI so signal code lives once (G7). |

## The OSC vocabulary (generic terminology)

The protocol is deliberately generic - not "windows". The root of a GuiDef may be a window, but it may equally be an embeddable panel, so the address family is `/gui_*`, not `/win_*`. (`/ui_*` is the recorded alternative if `/gui_*` reads worse in practice; the point is *not* a window-specific verb.)

### Commands (script -> gui host)

| Address | Args | Meaning |
|---|---|---|
| `/gui_def` | `id, json` | Build a whole GUI tree (window/panel + widgets) from a JSON GuiDef, in one message. The `SynthDef`/`/def_send synth` analogue. |
| `/gui_set` | `id, k, v, ...` | Update live properties of one widget (value, range, label, color, data ref, ...). The `/node_set` analogue. |
| `/gui_bind` | `id, target...` | Bind this widget's value to forward straight to the audio server, bypassing the script (see below). |
| `/gui_free` | `id` | Destroy a widget and its subtree (or a whole def by its root id). The group-free analogue. |
| `/gui_query` | `id` | Request a `/gui_info` reply describing a widget's current state. |
| `/gui_load` | `name` | Load a persisted GuiDef from the def store by name (standalone / saved-app path). |
| `/gui_font` | `blob` | Draw text with this typeface from now on (a TrueType/OpenType file). No id: a face is the host's, not a window's. |

### Events and replies (gui host -> script)

| Address | Args | Meaning |
|---|---|---|
| `/gui_event` | `id, value...` | A widget was interacted with (knob turned, button pressed, region selected, point dragged). |
| `/gui_info` | `id, type, k, v...` | Reply to `/gui_query`. |
| `/gui_closed` | `id` | A window was closed by the user. |

Property values are OSC primitives (int/float/string/blob) - the binding technology never leaks across the wire, in line with the project's "flat primitives at the boundary" rule.

### Bindings: the value can bypass the script

`/gui_bind` lets a widget's value flow **straight to the audio server** without a round-trip through the script - the same idea already used for MIDI in this project, where a control source is bound to a server-side destination instead of being polled. A knob bound to a synth control sends an OSC `/node_set` (or equivalent) to the audio server itself on every change; an unbound knob just emits `/gui_event` back to the script. This keeps interactive control low-latency while leaving scripted/computed widgets fully in the script's hands.

### Example session

```
# script -> gui host
/gui_def  1  { "type":"window", "title":"Filter", "w":480, "h":240, "layout":"col",
               "children":[
                 {"id":10,"type":"knob",  "label":"cutoff","min":20.0,"max":20000.0,"value":800.0},
                 {"id":11,"type":"slider","label":"res",   "min":0.0, "max":1.0,    "value":0.2},
                 {"id":12,"type":"waveform","buffer":0}
               ] }
/gui_bind 10 "server" "/node_set" 1000 "cutoff"   # knob 10 drives node 1000's cutoff directly

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
| `choice` | control | One of several, in six presentations over the same options and index: combo (a field and the list it opens), radio, segmented, tabs, pager, list (G37.1, G39). It was `menu`. |
| `separator`, `progress` | element | A line between groups of a strip (a spring with a `weight`); a bar over a value in `0..1`, or indeterminate (G39). |
| `table`, `files` | element | Rows of data -- a list, a table and a tree as one element -- and a file chooser over them whose directory the host lists, the disk natively and OPFS in a page (G41). |
| `waveform` | heavy GPU view | Editor-grade min/max peak waveform of a buffer/file/blob: multichannel lanes (stacked or overlaid), LOD-crossfaded zoom, adaptive rulers, selection, playhead, amplitude zoom/pan, linked navigation groups via `link` (G20-G20d); the measured layers — RMS, true peak, loudness — planned in the A track. |
| `spectrogram` | heavy GPU view | Editor-grade STFT time-frequency view, same sources and chrome as `waveform` plus a Hz ruler and frequency zoom/pan, linked groups via `link` (G20-G20d); a constant-Q analysis mode experimental (G20f). |
| `scope` | heavy GPU view | Time-domain scope over `channels` adjacent buses from `bus`, at `rate` (audio by default, control for a bus history): the audio-rate form is a triggered oscilloscope — lanes or `overlay` traces, ms/value rulers, a trigger-level mark and a lock/free read-out (G28, G33). |
| `phasescope` | view | Phase/goniometer (Lissajous) view of the audio bus pair `bus`/`bus + 1`, with a correlation readout (G19, G33). |
| `meter` | element | A level as a meter is read: the column in decibels, the held peak, the latched clip lamp and the ladder beside them, over buses read straight from shared memory (G21b). |
| `spectrum` | view | Live FFT magnitude curves (spectroscope), one per channel over `channels` adjacent audio buses from `bus`: adjustable dB window, linear/log/mel/bark frequency axis with Hz/dB rulers, averaging + peak-hold (G19, G28, G33). |
| `plot` | view | Static plot of an NRT-generated signal/file or a numeric sequence: multichannel lanes, x/y rulers, hover readout, signal/spectrum views (extensible). |
| `nodetree` | view | Live text/graphic view of the audio server's node tree and parameters, updated in real time. |
| `canvas` | view | A surface that runs a supplied WGSL shader, driven by OSC params or server audio - custom visuals. |
| `score` | view | Engraved music-notation page: the client engraves (Verovio) into a **display list** of glyph outlines and primitives in page units, which the host tessellates into the same mesh as the rest of the chrome. Scrolls/zooms in a `scroll`, carries a playback cursor over the score's own timemap, and reports the element under the cursor - a click as `"element"`, a drag as a `"transpose"` in diatonic steps (G31). |
| `timeline` | view | DAW-style tracks + MIDI/OSC sequencing - **future**. |
| `bpf` | view | Drawable break-point-function envelope: breakpoints + per-segment shapes using the server's own `EnvGen` shape numbers (evaluated through `clausters-core`), any `[min, max]` range (bipolar/unipolar/on-off via the hold shape), optional exponential display scale; edits flow back per the edit-back pattern (G21). |

Heavy views never reimplement DSP the server already owns: when a widget needs analysis/processing not already provided by `clausters-server` (peaks, STFT, FFT, resampling), `clausters-gui` reaches for `clausters-ffi`/`libclausters` rather than duplicating signal code. The `crates/clausters-gui` crate's own `peaks`/`spectrogram` modules are the prototype of that shared machinery and are a candidate to migrate behind the FFI.

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

Two decisions follow once the data gets large. **First, where the analysis lives.** The peak pyramid and the forward FFT are not GUI-private: a pyramid is general client functionality (any client with a waveform view), and the FFT is shared with the server's `FFT`/`IFFT` UGens. So they live **once** in `clausters-core` (`peaks`, `fft` over `microfft`) and are reached from non-Rust clients through the FFI; this crate's `peaks` is a re-export and the spectrogram calls the core FFT. **Second, how the bytes move.** A multi-megabyte buffer cannot ride an OSC datagram (the ~64 KB cap), and chunking it over `/buffer_getRange` re-traverses the network asynchronously for data already in local RAM. So large payloads move as **memory-mapped files**: a `waveform` names a `path` (raw `f32`) or a `cache` (a prebuilt pyramid) the host maps read-only and reads zero-copy, and a server buffer reaches the same path via `/buffer_export` (a raw dump to a local file) rather than `/buffer_getRange`. The network reads stay the async fallback, and the browser - which can map neither shared memory nor files - is exactly that fallback's client: `path`/`cache` become URLs fetched against the page origin (the pyramid for raw fetches built in wasm from the same core), and a server buffer rides chunked `/buffer_getRange` over WebSocket through the shared fetch machine (`host::fetch`); the synchronous `BulkLoader` trait stays the native (mmap) fill, since a fetch cannot block. This is the same move the control buses made in the audio path (read from the shared segment, zero messages), generalized to bulk audio.

### Three render regimes (no wasted work, and never "by samples" naively)

Per frame the waveform renderer picks one by `samples_per_px`, so it is always bounded by the screen:

- **Line** (`samples_per_px <= 2`): few enough samples are visible that individual ones matter - draw a polyline through the raw samples in range (vertex count bounded by window width). This is the only regime that touches raw samples directly, and only when it is cheap to.
- **Raw columns** (`2 < samples_per_px < base_bucket`): one min/max column per pixel computed directly from raw samples - exact, and bounded because we are below `base_bucket` samples/px.
- **Pyramid columns** (`samples_per_px >= base_bucket`): one min/max column per pixel read from the peak level matching the zoom.

### The spectrogram: same navigation, constant render cost

The spectrogram is the time-frequency analogue and deliberately reuses the navigation machinery. The STFT magnitudes are uploaded once as a 2D texture (x = time/frame, y = frequency bin). Rendering is a single full-screen quad whose fragment shader samples that texture; `viewport::View` only reshapes the sampled time slice, so the GPU cost is **constant regardless of zoom** (it is bounded by screen pixels, and the one-time analysis is the cache). The GPU's linear filtering gives resolution-matched down-sampling on zoom-out. So the waveform bounds work by *picking a resolution-matched LOD per frame*, while the spectrogram bounds it *structurally* (one texture sample per pixel) - two expressions of the same "never resolve finer than the screen" rule. Display controls (linear/log/mel/bark frequency axis, dB window/contrast, colormap) live entirely in the fragment shader as cheap uniforms, so they change live without re-analyzing; the analysis parameters that *do* require recomputation - window size, hop, sample rate - are arguments to `Stft::compute`.

Notation (`"score"` widget) is a third shape, and the cheapest: **all geometry, no analysis**. An engraved page is a fixed drawing - glyph outlines and strokes in page units - so there is nothing to resolve per frame and no LOD to pick; the page is tessellated into the flat `paint::Mesh` the light widgets use, and zoom is the containing `scroll`'s affine. What makes it fit here at all is that the *engraving* (the expensive, format-reading part) happens on the client, once per edit, exactly as the peak pyramid and the STFT are computed once per source - the same "the host draws, the client analyzes" split, with the display list as its cache (G31).

## Closed tracks

The tracks whose every milestone shipped are in [`docs/history/plans/gui.md`](../../docs/history/plans/gui.md),
moved there whole; the plan keeps what is open. The ticked entries of "Found by
use" and "Future directions" are at the end of the same file. The tracks are,
by their headings:

- Status: foundation in place
- Completed milestones (foundation + desktop host, G1-G10)
- Browser / WebGPU target (G11-G17)
- Widget deepening (G18-G21b): scopes, editor-grade views, edit-back
- Multitrack timeline view (G22): the DAW-style track editor
- G23: the patcher view (the logical group)
- G24: the editor-grade piano-roll view
- Done outside the numbered tracks
- G25: the host's TCP front (default command transport)
- G29: the virtual piano keyboard (`piano`)
- G30: the editable text field (`text`)
- G31: music notation (the `score` widget)
- G32: host gaps the editor example turned up
- G33: every live view names a bus and a rate
- G34 — The multitrack editor reuses the widget set that grew past it
- G35 — The multitrack editor, refined by using it
- G36 — Key bindings are configuration, not code
- G37 — Menus: the popup layer, the chooser put right, and a tree of entries
- G38 — What every widget carries: `enabled`, focus, a tip and an icon
- G39 — One choice, several presentations, and the small elements beside it
- G40 — Container chrome: groups, split panes, scroll bars and a dialog
- G41 — An option: rows of data, and choosing a file
- G42 — The clipboard is the platform's, and a field edits like every field
- G43 — Text is shaped: every script a field accepts is drawn right
- G44 — A take's picture is read at the level it is drawn, out of summaries kept once
- G45 — A take is drawn from its files
- G46 — A complex element is described again only where it changed
- L track — the look: layout, sizing and themes for the light widgets
- E track — the element library: coordinate-system containers and composed elements
- A track — what a signal measures, and the layers that show it
- H track — the gesture is undoable, and the host is the one that saw it
- ✅ In-browser audio engine (Web Audio / AudioWorklet) — shipped as the server's B track
- AP track — the application scope: the window set, the named id, the reconcile, one undo order
- V track — development: the host seen and driven by a script
- P track — the patcher: from view to Max-style editing (graphs and defs)
- K track, Part A — the widget API
- K track, Part B — the general capabilities

## K track — the widget API: a widget becomes an object

*Section opened 2026-08-09 from an audit of the crate read as a general UI library; **rewritten 2026-08-10** after the seam landed, one leaf ported and the next one stopped against the surface. The rewrite is not a change of goal — it is the same goal with the right diagnosis behind it, and the earlier entries `K1a`/`K1b` (added and superseded the same day) are folded into K3–K5 below.*

The track's purpose, in one line: **adding a widget should be writing one file, not editing eight** — and the way to get there is not to shorten the eight, it is to give a widget a self.

### The diagnosis: a widget has no self

The audit's first reading was that the crate is general everywhere around the element and closed in the element itself: no `trait Widget`, so a leaf is an arm in each of eight passes. That was true and it was the shallow half. Porting one leaf showed the deep half, and it is the finding the whole track now rests on.

**A widget's state does not live in the widget.** It cannot: a `WidgetKind` arm is data the host matches on, not an object that owns anything. So every piece of a widget that is not a plain prop has been rehoused somewhere in the host, and each rehousing needed its own routing:

| Where a widget's self ended up | How much | What it is |
|---|---|---|
| `WidgetKind::` matches across the crate | **389 mentions** | the eight passes: build, apply, size, the frame's draw, the query, the hit-test, the press, the tree collectors |
| `gestures::Drag` variants | **25** | the *geometry and snapshot* of a drag in flight — a slider's body, a knob's last y, a note's press-time span — held by the machine because the widget cannot hold it |
| `interact::edit` doors + `interact::read` readers | **27 + 12** | one mutation door and one payload reader per widget, routed by kind from outside |
| `FrameInputs` fields | **5 of 13** | `active_button`, `focused_text`, `menu_popup`, `marquee`, `wiring` — one widget's *interaction state*, fed back into the frame so that widget can draw itself mid-gesture |

Those four rows are one bug with four symptoms. A button is drawn pressed by passing `active_button` down the frame because the button does not know it is held. A slider's groove is remembered in `Drag::Slider` because the slider does not know it is being dragged. A menu's open list is a `FrameInputs` field and a `MenuOpen` in the gesture machine because the menu does not know it is open. Give the widget an object and all of it comes home — which is why this track is a *simplification* with an extension point falling out of it, and not an extension point bolted onto the existing shape.

The published claim "a widget is added by extension, never by protocol change" stays true **of the protocol** and is the opposite of true of the code. That gap is what closes here.

### What the API is: three things in, two things out

The temptation, once a leaf is an object, is one god-context carrying everything the host knows — which is the 389 matches with an indirection added. The boundary is deliberately narrow, and it is narrow because of what the diagnosis says: **most of what looks like "what a widget needs from the host" is the widget's own state coming back to it.** Once that state stays home, what genuinely crosses the boundary is small and it does not grow per widget.

**Three things in.**

1. **The roles** — `Draw`: the one mesh, the theme's colour roles and the placement's size table. Exists today, unchanged. No element names a literal number or an RGBA.
2. **The world** — read-only per-frame facts *no widget owns*: the bus source, the queried node trees, the sample rate, the sample clock, whether a server is attached, the pointer. One `&World`, identical for every element and every frame. This is K3.
3. **The container's coordinate system** — a leaf placed on a lane's time axis needs that lane's window; a leaf in a plane needs the plane's zoom. Already modelled as `interact::Coords` / `TimeAxis` on the hit chain and `Placed::time` on the placement; the trait takes it, it does not invent it.

**Two things out.**

4. **Effects** — `GestureEffect` (emit, redraw): what the element asks the front to do. Exists today.
5. **Declarations** — what the element *is* and what it *needs*: its value, its `/gui_query` fields, the buses it reads, the bulk it wants loaded, the GPU slot it claims. One `Needs` + `info` + `value`. This is what the tree collectors read instead of matching on a kind.

Everything else — the drag's geometry, the caret, the open list, the held key, the selection, the marquee — is **state, and state lives in the element**.

### One trait, not a family of them

`Element` stays a single object-safe trait with defaults on every method but two. Splitting it into `Element` + `Interactive` + `Fed` would force the schema to ask, at every pass, whether this element also implements that one — which `dyn` cannot answer without upcasting gymnastics, and which would put the branching back exactly where the track is removing it. **The smallest element is two methods** (`set`, `draw`); every capability past that is a method you may override, and not overriding it is the same as not having the arm today.

The surface, with the milestone each part lands in:

| The pass a built-in spells as an arm | The element spells as | Lands in |
|---|---|---|
| a `build.rs` arm | a registered `Constructor` keyed by the wire's `type` | ✅ K1 |
| an `apply.rs` arm | `set(&mut self, key, v) -> bool` | ✅ K1 |
| a `size.rs` arm | `natural(&self, m, scale) -> Natural` | ✅ K1 |
| `frame/items.rs` + `frame/draw.rs` | `draw(&self, d, ctx)`, or a declared GPU slot | ✅ K1 / K3 |
| the `is_bare_surface` query | `is_bare_surface(&self)` | ✅ K1 |
| `FrameInputs`' live reads | `ctx.world` — one read-only context | ✅ K3 |
| a `gestures/press.rs` arm + a `Drag` variant + a `drag.rs` arm + a `release` arm + the `interact` sub-hit | `press` / `drag` / `release` / `wheel`, with the drag's state **inside the element** | ✅ K4 |
| the `interact::read` payload readers | the gesture's own `Events` (K4); the non-gesture read has no consumer | ✅ K5 |
| the `gestures/keys.rs` arms + `focused_text` | `accepts_focus` / `key` | ✅ K6 |
| the tree collectors (live buses, taps, node groups, GPU slots) | `needs(&self) -> Needs` — the whole declaration | ✅ K3 (bulk with `signal`, ✅ K7) |
| a widget that draws **outside its rect** (a menu's open list) | `overlay_rect` + `overlay(&self, d, ctx)` | ✅ K4 |
| a container recognizing one of its **bodies** by matching the leaf's variant | `body_role(&self) -> Option<BodyRole>`, with the container's axis arriving in `Ctx`/`Input` | ✅ K5 |
| a workspace measuring a child's content by matching its variant | `content_size(&self) -> Option<(f32, f32)>` | ✅ K7 |

### What stays in the enum, and why that is not a compromise

`WidgetKind` keeps the **containers**: `window`, `layout` (with its stack flow), `plane`, `field` (the lane, the ruler strip and the clip). A container is not a drawing, it is a **coordinate system** — the layout pass has to know how it places into it, the hit chain has to know what system it gives its contents, and a gesture's plan (pan / select / locate) is a property of that system rather than of anything inside it. Promising a third-party coordinate system is a different and much larger piece of work, and pretending otherwise would make the trait carry a container's obligations for every leaf that has none.

So the boundary is: **a container arranges and defines a space; an element draws in one.** K8 makes that a test rather than a convention.

### Five constraints on every milestone

Unchanged from the track's opening, and none of them is relitigated per milestone.

- **wasm parity is not negotiable.** `check-wasm.sh` gates each milestone. Anything that cannot compile for `wasm32` goes behind the platform seam's traits or behind a Cargo feature the browser build turns off; a capability that exists natively and not in a page is a **degradation of one build**, never a fork of the core. The core stays single-threaded, so nothing here needs `Send`/`Sync` — which is what makes `Box<dyn Element>` cheap in the first place.
- **The GuiDef → HTML element path keeps working.** `<clausters-bundle>` mounting a GuiDef into a page (`clients/web/src/elements.ts`) is a shipped product surface, not a demo. Every milestone that reshapes the wasm-bindgen surface ships its web half **in the same commit**.
- **The cost rule is unchanged**: one mesh upload and one draw call per window, plus the heavy views' own pipelines. A capability that needs a second pipeline earns it **once per window** and only in the builds that compiled it in.
- **Weight is a feature, not a default.** The reason the GUI is a separate crate is that the GPU/windowing stack should not bloat a headless install; the same argument applies inside the crate, to a font rasterizer and to element families.
- **The packages move together.** A wire change ports `docs/gui-protocol.md`, both builders and the three books in its own commit; a Python signature change re-runs `pyright`; a web change re-runs `./build.sh && ./test.sh`.

### Build order, and the one lesson that reordered it

The original order was "seam, then port the leaves easiest-first, then the capabilities". Porting `label` — the easiest leaf there is — validated nothing, because the easiest leaf exercises the smallest possible surface: it draws from its own props and its press is a click. `meter` stopped on the world seam and `slider` on the drag seam, both immediately.

**So the surface is designed against the hardest leaves and landed against the easiest ones**, and those are two different orders that were previously conflated:

- **Designed against** `slider` (a drag with geometry), `meter` (a live world read), `text` (focus, a caret, a keyboard) and `patch` (a canvas, a multi-selection, two drags and a marquee). If the trait expresses those four it expresses everything; if it expresses `label`, it expresses `label`.
- **Landed against** the catalog in increasing order of what it touches, one leaf per commit, so a mistake in the surface is found on leaf three rather than leaf fifteen.

K3–K6 complete the surface, one seam each, each with the leaf that proves it. K7 is the port, against a finished surface. K8 locks the boundary. K9–K12 are the general capabilities, unchanged in substance from the original track and now cheaper: they land on one file per widget instead of eight passes. K13 and K16 make the whole thing someone else's; K14–K15 are the pass over where the code sits that Part A's port left owed.

One ordering detail worth stating because it was previously a known cost: **the keyboard (K6) comes before the port (K7)**, so `text` — 479 lines of real editing, today a parallel path through `Host::focused_text` — is ported once, into `key`, instead of being ported and then rewritten.

*Part A (the widget API) and Part B (the general capabilities) shipped whole
and are in [`docs/history/plans/gui.md`](../../docs/history/plans/gui.md), as "K track, Part A" and "K track,
Part B".*

---

### Part C — making it someone else's

- ✅ **K13 — The crate's features: what a build pays for.** It had `midi`, `standalone`, `standalone-faust` and — since K10 — `font-atlas`, and no axis at all through the *catalogue*: a program linking this crate for its controls compiled the engraver, `lyon` and the patcher whether or not any def ever names one. It now has `notation` (the `score` element and `lyon`) and `patcher` (the `plane`-with-boxes graph view), both **on by default**, because a feature exists so a build can *drop* a family and never so it has to ask for one — which is also why `font-atlas` and `standalone` default the other way.

  **The protocol invariant is what made this three files rather than a refactor**, exactly as the entry predicted: a family compiled out drops its rows from `elements::builtin`, a lookup that misses both the table and the registry is `Unknown`, and `Unknown` is laid out and not painted. A slim build therefore degrades the way a host older than the def already does, with no new rule and no new failure mode. `patcher` has the one exception worth naming, and it is a property of the wire rather than of the feature: `plane` is the patcher's name *and* the workspace's, told apart by the presence of `boxes`/`cords`, so without the family that fork is not compiled and a patcher node reads as the workspace it also is — degrading to something real rather than to nothing.

  **What the milestone found is why it exists.** `--no-default-features` did not compile, and had not for some time (its own commit, and its entry under "Found by use"): three `use` lines carried a `midi` gate that belonged to none of them. Then the first matrix run failed twice more — six *tests* that exercise a dropped family, which clippy cannot catch because it type-checks test code and never runs it, and one rustdoc link into a module a configuration compiles out. That third one is the reason the doc build walks the matrix at all, and it is the same lesson the server crate already wrote down: a doc comment naming an item across a feature seam names it in **backticks**, never as a link.

  **`crates/clausters-gui/check-features.sh`** is the runner, the sibling of `.agents/skills/feature-matrix/check.sh`: seventeen configurations — clippy over the floor, each family alone, `font-atlas` both ways and both `standalone`s; the wasm gate over the floor, the default set and the profile `clients/web/build.sh` ships; rustdoc over the default set and the floor; and **the suite** over both ends, which is the row the server's matrix does not have and the one that caught the six tests. The server's matrix now runs it as a single row (and dropped the two it used to spend on this crate, which are two of these seventeen), so one command still answers "is the tree committable?". Named in `docs/contributing.md` and in the `clausters-gui` skill beside `check-wasm.sh`.

  **What is not here, and it is not a Cargo.toml line.** The entry also named `timeline` and `signal` as families; they are not, yet, and the measurement is worth keeping rather than re-deriving. `notation` is named outside its own modules in **two** places (a `mod` line and a `builtin` row) and `patcher` in four; `signal` is named in twenty-nine files and the timeline family in nineteen — and not at the edges: `frame/mod.rs` is *built around* `WaveformSlot`/`SpectrogramSlot`, `view::Renderers` holds both heavy pipelines, `bulk.rs` and the server leg resolve into them, and the timeline's navigation groups live on the `Host` while its time containers are placements in `layout.rs`. Gating those means the frame's slot machinery, the bulk path and the navigation model must each compile with no heavy view at all — a structural pass of K7's size, with its own milestone's worth of decisions (what a `clip` is without a take, what `Renderers` is when it holds nothing). Landing it half-way would leave a matrix that does not build, which is worse than an axis that does not exist. So the two families stay compiled in, the seam that would carry them (`elements::builtin` → `Unknown`) is already proven by the two that landed, and the work is recorded under "Future directions" instead of pretended here.

- ✅ **K14 — Where a leaf's two halves live.** The port gave every leaf an `impl Element`, and left the other half of it where it already was: a flat module in `host/`. So `host/` is thirty-nine loose files that are four different things — the protocol and its state, the drawing toolkit every widget reads, the passes, and the **models** an element draws through — with the platform seam (agnostic core / native shell / wasm) as the only grouping `mod.rs` states. Nothing is wrong; nothing says where a new file goes either, which is how `elements/patch.rs` and `host/patch.rs` come to sit two levels apart with no line naming the difference between them.

  **The line exists and the code already says it**, in `elements/score.rs`: *"the page itself and all of that reading stay in `host::score`, which is the model; this file is only how the passes reach it."* An **element** is the surface the passes see — one method per question the frame, the gesture machine or a `/gui_query` puts to it. A **model** is what that element knows: the data shape, the geometry, the drawing over a `Draw`, the hit-test primitives that invert it. The boundary is stated negatively, and that is what makes it worth having: a model names no `Host`, no trait, no props map, no OSC and no device, which is why every one of them unit-tests without a window.

  **Why a parallel tree and not a package per element.** The obvious move — a leaf's two halves in one directory, the shape `signal/` already had — does not generalize, and the counting is the argument rather than a preference: `controls` draws eight elements, `pianoroll` draws the `notes` leaf *and* a clip's roll body, `bpf` the `curve` leaf *and* a clip's automation body, `textedit` serves the `text` leaf and `controls`, and `track`/`timeline` serve containers and no element at all. A model with several owners has no directory to live in. So elements and models are two trees, the dependency runs one way (an element reads its model, never the reverse), and an element is a **file, or a directory when it needs one** — which is what `signal/` turns out to be rather than an inconsistency.

  The name is `graphics/`: `views/` collides with a live type (`timeline::View`, `ScrollView`, `crate::view`), and three of these modules already open on the word — *"the piano keyboard's graphic primitives"*, *"the piano-roll graphic primitives"*, *"the multitrack graphic unit"*.

  - ✅ **K14a — The signal element, split by the rule it prompted.** `host/signal/` was ten files and read as the one leaf that had already been packaged; measured against the rule it is the opposite — eight of the ten answer trait questions and are the *element*, and only the drawing is the model. So `elements/signal/` keeps `apply`/`bulk`/`slot`/`live`/`freq`/`chrome`/`body`/`element` and the taxonomy the wire is read into, and `graphics/signal/` takes the pictures: `trace` (the one min/max-per-column answer every signal drawing reads), and `plot`, `spectrum`, `phasescope` and `waterfall`, which were four top-level modules in `host/` and are consumed by nothing but this element — 1847 lines that had no owner named anywhere. The seventeen-configuration matrix is clean and the 686 tests pass; `docs/architecture.md`, both GUI skills and `test_gui_props.py`'s element-directory table follow the move in the same commit.
  - ✅ **K14b — The remaining models into `graphics/`.** `controls`, `meters`, `textedit`, `nodetree`, `piano`, `pianoroll`, `bpf`, `patch`, `track` and `score/`, all ten by the same rule. Mechanical but wide — thirty-odd files change an import — and the payoff is what `host/mod.rs` can now say: the agnostic core is fifteen modules instead of thirty, and they group into four families the file names for the first time (the protocol and its tree, the models, the drawing vocabulary, the passes), with the placement rule written beside them — a module stays flat when the *whole host* reads it and goes in a directory when only its own consumers do, which is why `paint` and `metrics` are files while the models are a tree.
  - ✅ **K14c — `host/canvas.rs` is not a model.** It is the WGSL pipeline a script's shader runs on, consumed by `frame`, `gui/` and `web/` — the third of the crate's heavy renderers, and the other two (`crate::waveform`, `crate::spectrogram`) already lived at the crate root. It moved beside them, so the three pipelines are one row of the crate and `host/` holds none.

- ✅ **K15 — What still reaches a model without going through the element.** The entry opened on a wrong premise, and correcting it is the first thing the milestone found: **`gestures/` is clean.** The calls it was accused of — `patch::obj_rect`, the keyboard's geometry — are in `gestures/tests.rs`, and a test asserting a model's geometry directly is the model doing its job, not a leak. The audit that opened this had grepped the directory without separating the test file from the machine.

  What the measurement did turn up is four things, and only one of them is the shape the entry guessed:

  - **A model that names the wire.** `graphics/piano.rs` carried `voice_on_msg`/`voice_off_msg` — it *built OSC*, which is the one thing the models' own negative test forbids. And they were never the keyboard's: a voice is declared by any element (`Element::voice`) and allocated, sent, remembered and released by the `Host`, which already held the map, the counter and the id window. They are `host/voices.rs` now, with those two constants, and the keyboard is geometry again.
  - **Four parse helpers with one caller each.** `parse_notes`/`parse_osc` (only `elements/notes.rs`) and `parse_patch`/`parse_ports` (only `elements/patch.rs`) sat in the schema's shared `widget/parse.rs` and returned `graphics::` types, so the schema declared a dependency on three models it does not use. They moved into their elements. `widget/parse.rs` also stops holding the crate's one `#[cfg(feature = "patcher")]` island.
  - **A control's size lived apart from its drawing.** `slider_thick`, `slider_across`, `knob_h` and `field_h` were in `widget/size.rs` and read `controls::readout_h` from there — which is why a *pass* imported a model at all. How tall a slider wants to be and how a slider is drawn are one fact (the read-out strip is the same number twice), so they are one section of `graphics/controls.rs` now and an element's `natural` calls straight into it. `widget/size.rs` keeps the vocabulary every widget composes from (`control_box`, `label_strip`, `text_box`, `body_inset`), which is what it is for.
  - **A corner shared by four callers, filed under whoever wrote it first.** `meters::value_text` draws a short string in a body's top-right — the scope's `lock`/`free`, the spectral views' scale tag, the frame's overlay. It is `graphics::corner_text`, at the models' root, so the frame's draw pass stops naming the meter's module for a generic helper.

  **And the rest is a rule, not a debt.** Every edge left runs into `graphics::track` (from `layout`, `interact`, `frame`, `timeline`, `widget`) or into `graphics::nodetree::NodeTree` (from `world`), and both are the boundary working as designed: `Track` and `Clip` are **containers**, kept in `WidgetKind` precisely because the layout pass has to know every coordinate system it places into, so there is no element to route through; and the world holds a document the client leg queried, whose type has to live with its model. `Header` stays in `graphics/track.rs` for the same reason and one more — it carries `width`, its own natural size, and splitting the data from the geometry to satisfy a tree would cost more than it states.

  Seventeen configurations clean, 686 tests pass. `docs/architecture.md` gains the `voices.rs` row and the `keys` row says whose the voices are.

- ⬜ **K16 — The host's own documentation.** With Part A done the extension recipe stops being lore in the `gui-widgets` skill and `docs/architecture.md` and becomes a **public API** — which is more than the chapter this milestone originally named. Today the host's documentation does not live anywhere as a whole: the wire is a page of the *server*'s book (`docs/gui-protocol.md`, `docs/gui-props.md`), how a script drives one is a chapter of the *Python* book, the component is a chapter of the *web* book, and how the host itself is built is `docs/architecture.md`, a development doc. A reader who wants to *write an element* has no book to open, and after K13 a program that merely links the crate is a supported user with no entry point at all.
  So the milestone is split, and **the first part is the planning**, because the rest of the split follows from its answer and cannot be decided before it.

  - ⬜ **K16a — Decide whether the GUI host earns a book of its own, and what is in it.** The project rule today is **three mdBooks, one per platform** (server/workspace, Python client, web client), stated in `CLAUDE.md` and in the `documentation` skill; a fourth is a structural change to both, not a file. Four questions it has to answer, and none of them is obvious:
    - **Is the host a platform?** It is not a language client, which is what the three-book split is organized by — but it *is* a crate a third party links, with an API of its own (the trait, the registry, the Rust tree builder) and rustdoc as its reference generator, which is the same shape the other three have.
    - **What moves, and what must not.** The wire reference is read by *client* authors and sits with the rest of the OSC reference; moving it would split one protocol across two books, and leaving it means the new book cross-references rather than contains it. The Python and web chapters are those clients' usage and stay where they are. What is genuinely homeless is the **widget author's** material and the host's own operation (transports, standalone bundles, the theme and metrics files) — the question is whether that is a book or a section.
    - **What it costs.** A fourth `book.toml`, a fourth ReadTheDocs deploy, a fourth generated API reference, and "keep all three current" becomes four. A thin book is worse than a good chapter.
    - **What it is called and who it is for.** The three books are named by platform; this one would be named by *role* (the host, its authors, its embedders), which is the first place the existing scheme stops fitting.
    - **The editor carries it, if it is a book.** clausters-editor's documentation viewer shows every book the session's environment registers, and the client's wheel registers two: its own and the user's parts of the server's (`clients/python/build_docs.py` stages them, `clausters._book` names them under the `clausters_editor.docs` entry points). The host's pages ride inside those two today. A host book of its own is staged and registered the same way: a third entry point, its `SUMMARY.md` headed by its title, only its user-facing parts, and the build and install pages cut as `build_docs.py` cuts the others'. The pages that move out of the server's and the client's books leave their staged copies with them *(noted 2026-10-15)*.
    Closing K16a means the decision is written in `docs/decisions.md` (it is exactly a non-obvious structural call), `CLAUDE.md` and the `documentation` skill say the new number, and **this entry is rewritten with the parts the answer implies** — K16b/K16c below are what is already clear whichever way it goes.
  - ⬜ **K16b — The widget author's guide.** What an element is; the trait's methods and which pass each one replaces; what the world context carries and what it deliberately does not; how a drag is owned by the element and sequenced by the machine; how an element fills a container's body role and is handed that container's axis; how an edit travels back in the owner's terms; how a registration is named on the wire; and where the line between an element and a container is drawn and why. Its home is K16a's answer; its content is not.
  - ✅ **K16c — The smallest complete proof that a third party can do it.** *(Done 2026-10-07: `crates/clausters-gui/examples/custom_element.rs` registers a `step_pad` — a bar of cells a press lights one at a time — builds a window with `tree` and opens it with `host::gui::run`. It compiled against the public surface as it stands, with nothing made public for it: the trait, `Ctx`, `Input` and `Claim` through `clausters_gui::element`, `Draw`, `Metrics` and `Rect` through `host`. One thing an outside implementer meets: `Element::natural` answers a `Natural`, an alias in a `pub(crate)` module, so the example writes the tuple it stands for. Launched, the window opens under its title and the process runs until closed; what it draws and what a press does are still to be looked at by eye.)* `examples/custom_element.rs` in the crate: registers one element, builds a window with K2's builder and opens it — no Python, no OSC, no JSON. It is also the manual test of the public door, which nothing else exercises end to end.

### What stays out of the track

Recorded so no milestone reopens it: a styling language, selectors or a cascade (the role overlay is the only cascade, and it cascades role tables, never per-part rules); a constraint solver; an accessibility tree (K6); a third-party **container** (a coordinate system is a much larger promise than a drawing — K8 draws that line and tests it); and the catalog growing a new name where a product of the existing ones would do — the `signal` rule, which is the only thing that has kept the enum from doubling and which Part A makes structural rather than habitual.

## D track — the hand that edits data: samples, spectra, and the shape of a selection

*Section added 2026-08-12, from the user's brief: selection and editing per sample (zoom in, grab one sample, drag it up or down; a **draw mode** distinct from the selection mode for a run of them), spectral selection by bin and column with the overlap factor understood, a spectral draw mode whose inverse FFT reconstructs a signal, and — the sentence that ties them — selections whose data can be **saved**, to copy and paste and to hand ranges to editing algorithms, with a lasso for regions that are not rectangles.* It folds in the "A sample as a grabbable point" future direction — *named 2026-08-12 by the user on the `gui_window` pass that checked the round hit shapes ("the samples, the round points, cannot be selected individually and dragged — that could be implemented"), and removed from that list here.*

**The track was split on 2026-08-14, and what is left here is its view half.** The original track carried both the gestures *and* the model they edit — what a selection **is**, what a clipboard holds, what an edit means and who applies it — and trying to hold both is what made its premise fight itself: it kept giving the host the functions of an owner because the owner existed nowhere a host could reach. That owner is now a crate, and its roadmap is `crates/clausters-document/PLAN.md` (the `Ox` milestones), which took the typed selection, the typed clipboard, the intent vocabulary and the acknowledgement. **What stays here is what only a window can do**: the gesture, the drawing, the hit-test, and the honest refusal. **The order between the two files matters and is one-way**: O1-O3 (the document, the intent vocabulary, the acknowledgement) come before D1, and each milestone below names the `Ox` it needs — nothing here can land against a host with no owner to answer it, which is the same sentence as the track's premise seen from the schedule.

**The premise is unchanged and is now structural rather than a rule to remember: the host owns no data.** A sample belongs to a source (a server buffer, a file), a spectral frame to an analysis of one, and the arrangement they sit in to the document. So every milestone here ends the same way — a gesture becomes an **intent** the owner applies, and the host draws what came back.

### Decisions taken before opening the track

*(The decisions about the document itself — a selection is a value, the clipboard is one typed document, an edit is an absolute intent, the acknowledgement is a stamped state push — moved with their milestones to `crates/clausters-document/PLAN.md` and are not restated here. What follows is what remains a decision about the view.)*

- **A mode is a gesture plan, not a new state machine.** E5/E10 gave every container an ordered plan of steps per modifier, declared on the wire and live via `/gui_set`. **Draw is a new step in that table**, so "selection mode" and "draw mode" are two plans, and which one is bound is a script's or a toolbar's decision rather than a mode enum inside the host. That is what keeps the feature general: the same step gives a curve a pencil and a spectral plane a brush, and a script that wants pencil-on-Ctrl gets it without asking anyone.
- **The host draws the edit before it is confirmed, as a pending overlay, marked.** Between the gesture and the acknowledgement the host draws what the hand did, in a state that is visibly *pending*, and it drops that overlay when the stamp for it returns (`crates/clausters-document/PLAN.md`, O3 — the rule is *drop every pending at or below the stamp, and adopt what arrived*). The alternative — draw nothing until the round trip closes — makes a pencil unusable over a network leg, and the alternative to *marking* it is a host that lies about what the data is. **What made this safe is the absolute-intent rule**: a pending needs no rebasing against the state that arrives, so the host never has to recompute an edit and therefore never needs an executable copy of the document.
- **A redraw is a span, not a file.** Re-summarizing a minutes-long take because eight samples moved is the one thing that would make this unusable, so `clausters_core::peaks` grows an in-place `update_range` (rebuild the buckets a sample span touches, up the levels) — in the core because the owner rebuilds the identical cache. **Who runs it, and on what copy, is settled by the layering**: the owner updates the resource, the host re-reads the span it was told changed, and the host's mapped resources stay read-only as they always were.
- **Spectral editing needs phase, which the display analysis does not keep.** The `Stft` behind the spectrogram stores magnitudes; an edit that must resynthesize needs complex frames, which is twice the memory for the same picture. So an `editable` spectral element analyzes complex — **opt-in, per element, stated on the wire** — and a read-only spectrogram is unchanged.
- **Bin × frame is not a sample range, and the view must not pretend it is.** The hop sets the column grid; the **window** sets each column's support, so a one-column edit touches `window_size` samples and neighbouring columns share them — which is exactly the overlap factor the brief points at. A spectral selection therefore reports **both**: the frames × bins rectangle it is, and the sample span it actually touches. The cell is drawn hop-wide (that is where the data sits) and the footprint is shown when a selection is made, because a user who edits a column and hears a change a window wide has been told nothing.
- **An edit that only a source can express is refused where it cannot be shown.** Two refusals, and they are the same rule at two ends: a stroke is refused where a pixel is more than one sample (it would write values the reader cannot see), and an edit is refused on an element whose source has no owner to send it to — a live tap or a streamed bus. Refusing visibly is the honest answer; the `editable` opt-in for spectra is the same idea already applied once.

### The milestones

- ✅ **D1 — A sample is a grabbable point** *(folds the "A sample as a grabbable point" future direction)*. Past the polyline threshold the trace already marks each sample with a disc sized as a curve's break-point, and `Element::hit_area` already answers on discs (E22) — so D1 is the route, not the hit-test: press a sample, drag it vertically, draw the pending value, and emit the absolute intent with its previous value. **What the owner does with it is settled and not the host's to know**: a destructive edit runs against a temporary working copy of the element's material and becomes part of the composition only when it is confirmed (`crates/clausters-document/PLAN.md`), so the host draws, emits and adopts exactly as it does for a clip, and it does **not** remap the resource per stroke - the working buffer leads while the session is open, the file is written on confirmation, and the acknowledgement is what corrects the picture. The **reconciliation machinery is O3's** (the pending set, the stamp, the drop-and-adopt rule); what D1 owns is what a pending edit *looks like* on a signal element — the marked drawing the rest of the track reuses, and the rule that a pending value never enters the peak summary. **It also lands `clausters_core::peaks::update_range`**, which does not exist yet and which nothing else claims: rebuilding the buckets a sample span touches and the levels above them, so a redraw costs the span rather than the take. **Requires O1-O3.** Example: `bulk.py` zoomed to samples, edited by hand and re-read.

  **The cost this track was told it could not design away turned out to be the wrong cost**, and the correction is worth carrying because it changes what this milestone has to build. The entry it referred to (`crates/clausters-server/PLAN.md`, "Found by use": *a sample write costs the whole buffer*) read `/buffer_setRange`'s whole-buffer replacement as an asymptotic defect an editor would pay per gesture — measured 2026-08-15 at 33.8 ms a write on a five-minute take, against the ~100 a second a stroke emits. **Both halves of that dissolved under measurement** (`crates/clausters-server/PLAN.md`, **S12**). Hearing a stroke does not go through the take at all: what a stroke has to sound is the **span it just drew**, and a scratch buffer that length sustains 443 writes a second — the route D4 already takes when it plays a copied block out of a buffer of its own. And the edit itself does not go through the pool: it is an NRT job over the working copy, and a take's buffer is replaced whole exactly once, on confirmation, which is the correct operation at that moment rather than a cost. **So what D1 emits is an intent applied against a buffer of the interactive NRT mode** (`crates/clausters-server/PLAN.md`, S13), not a write to the RT pool, and the RT buffer a clip plays from is downstream of the confirmation — which is also why S12 and S13 come before this milestone: they decide what applies the intent and where it runs.
  **What shipped** *(2026-08-16)*. `sample` joins the gesture plan table, so the mode is a plan and not a state the host decides, and it **declines where a sample is not a thing on screen** — read from the drawing's own rule (`trace::dots_fit`) rather than restated, so what can be grabbed is exactly what is drawn, with the plan falling through to its next step where it cannot. The value follows the pointer and the sample does not: which one is held is fixed at the press, or a view scrolling under the drag would hand the hand a different one. One intent leaves on release — `"sample" channel frame value previous`, the frame an OSC **long**, since a float runs out of integers at six minutes of audio and a sample index is exact or it is the wrong sample.

  **What a pending edit looks like**, which is the half `ack.rs` said was missing in as many words: a ring rather than a filled disc, tethered by a line to the value it replaces, so it reads as *this value, moved from there*. It never enters the summary — the material has not changed, and re-summarizing per motion event would cost the span every frame *and* make the overview disagree with the samples until the edit landed. It is dropped when the acknowledgement retires it, which is the other half of O3's drop-and-adopt.

  **`peaks::update_range` landed with it**, in both `Pyramid` and `MultiPyramid`, and is asserted **identical to a full rebuild at every level** over seven span shapes and again after four edits in a row — the multichannel one reading each channel with a *stride* rather than de-interleaving, since copying the channels out to update one span would cost the buffer this exists to avoid. FFI-exported (`clausters_core_peaks_multi_update`, `CORE_ABI_VERSION` 19 → 20) with its wasm sibling and its row in `docs/bindings.md`, because the owner of the material is the one that keeps the cache true.

  **One thing the example forced, and it is a real hole the milestone had not named**: an owner had no way to say *the material is now this*. Without it the pending could never be let go of — dropping it would take the edit with it — so `data` became settable on an **inline** source (refused on a mapped one, which is re-read by resolving the resource again). That the two are different doors is now stated on the wire reference rather than left to be discovered. Example: `bulk.py` grew an editable lane, and the handler in it is the whole round trip — apply, push, *then* acknowledge, in that order, because the other one blinks the old value back.

  **What is still missing, named rather than left**: a **mapped** resource cannot be re-read in place, so an edit to a take that reaches the host as a file or a cache has no way to correct the picture — the pending drawing carries it and then lets go. That is the door D2 will need before a stroke over real material is usable, and it is not this milestone's.

- ✅ **D2 — The draw mode**. `draw` joins the gesture plan table: a press-drag over a signal element writes the value under the pointer for every sample the pointer passes, interpolating between motion events so a fast stroke leaves no holes, and emitting **one intent per stroke** rather than one per sample - the general rule, decided in the crate's plan: one gesture is one intent, and the pending overlay is what carries the hand until it lets go. Refused, visibly, where a pixel is more than one sample. Python/web builders take the plan; the example shows both modes on one element with the plan switched live. One intent per stroke is also what keeps the whole-buffer write cost (see D1) at one per stroke instead of one per sample — the rule was chosen for the log's sake and pays here too.
  **What shipped** *(2026-08-16)*. `draw` joins the plan table beside `sample`, and the two are the same drawing at two lengths — which is what the pending became: a **run** (`PendingEdit`) rather than a sample, so the marked overlay, the emit and the drop-on-acknowledgement are written once. A stroke fills the samples *between* two motion events by interpolation, because a pointer reports where it is and not where it went, and writing only the reported positions would leave the material combed with holes. One intent per stroke, the two runs as **blobs** — the bulk convention `/buffer_setRange` and the clipboard already follow, since a stroke over a few thousand samples as typed arguments is the encode that rule exists to avoid.

  **The refusal is visible and it consumes the press.** Where a pixel is more than one sample a stroke would write values the reader cannot see, so the host says `"refused" "draw" <reason>` — and does *not* fall through to the next step, or a plan naming a sweep behind it would turn a refused stroke into a selection. Both halves are tested.

  **What D1 named as missing landed here, because this is what needed it**: a **mapped** source can now be told to read itself again (`/gui_set reload 1`), which makes the element forget what it resolved so the loader asks for it once more. With `data` for an inline source, an owner that applied an edit can always correct the picture — and that is the precondition for letting go of a pending edit at all, since dropping it otherwise takes the edit with it. A source with nothing behind it ignores a reload rather than erasing itself.

  Example: `bulk.py`'s editable lane now switches between the two steps live from a button — which is the D-track's own rule about modes made visible, a `/gui_set` of the plan rather than a mode inside the host — and reports a refused stroke where it cannot draw.

- ✅ **D3 — The selection gesture grows a second axis** *(done 2026-08-14)*. The marquee over a container with two measuring axes (the roll's already is one, so this is a generalization rather than a new gesture), feeding the crate's typed `Selection` (O6). The sample-axis snapping rule stays exactly as the Found-by-use entry fixed it, and the value axis takes the same treatment on its own domain.

  **Two decisions the milestone had to take before any of it could be written**, and both are answers to *what an axis is*, not to what a gesture does.

  **Where the range lives: on the widget, not on the group.** The span is the navigation group's because every linked view shows the same time — that is what a linked view *is* — but the views sharing a time axis measure different things vertically, so a range held in common would restrict a spectrogram in hertz by a waveform's amplitudes. It goes on `EditorProps` beside `y_start`/`y_len`, which is the line that file already drew for itself ("Only the y axis stays per-widget"), and it is the same split G24d made when the roll's marquee sent its span to the group and kept its note set. The wire says which widget, since the event has always carried the interacted member's id.

  **What "the same treatment" means on a value axis: ordering and clamping, and nothing else.** Time snaps because its data is discrete — the sweep takes the samples it passed over, and one it has not reached is not in — and an amplitude has nothing to pass over, so the range is what the hand drew, clamped to the element's domain, which is the only way it could be wrong. The rule generalizes as *snap to the axis' quantum where it has one, clamp to its domain always*, and the roll is where the first half is not vacuous: a pitch axis **is** discrete, so its band is the whole semitones the sweep covered, both ends included — the passed-over rule in its own unit, `timeline::value_span` and the roll's own `ceil`/`floor` being the two arms of one sentence.

  What shipped. `Element::value_axis` is the sibling of `freq_axis` — the element answers, because where the picture sits inside its rectangle is its own region split and the number under the pointer must be the one the cursor readout names, so the marquee and the readout share `waveform::display_to_value` and cannot disagree. Only a navigable trace answers: a spectral view's second axis is a band of bins (D5's, and its own field in the crate's `Selection`), and a live view has nothing behind it to hand a range of. `Drag::Select` carries the axis and the value it started at, **snapshotted** unlike the time window a linked view can scroll under a held drag. The band is then drawn as the rectangle it is, in every lane — one vertical window serves them all and a value says the same thing in each, which is `centres_y_zoom`'s reasoning applied to a picture. On the wire the event grows by exactly two arguments and only when there are two, so a reader of `start len` keeps working; `sel_min`/`sel_max` join the `y` axis' props with the empty-pair convention `sel_len <= 0` already uses. Python: the builder kwargs, and `Editor.selection` — the payload kept as the crate's typed `Selection`, its time in beats and its range in the element's own domain, of the element where the widget names one and of the shared axis where it does not — plus `Editor.resolve_selection` over O9, which is what makes the milestone's "feeding the crate's `Selection`" a thing that runs rather than a shape that matches. The web builder ports in the same commit, with the parity vectors regenerated. Tests: the two rounding doors side by side, the sweep with height (and the plain one after it, which clears the range rather than leaving a restriction from a finished gesture), the spectral view reporting no range, the roll's semitone band, the band drawn per lane, and the Python route landing a resolvable selection. `editor.py` is the eye pass — the two heavy views **unlinked**, so each one's selection is its own; it was written on `linked.py` first and moved, because that example's whole subject is the selection the two views *share* and a second axis competes with it. **The pass changed the milestone**: the band shipped on the plain drag, which put a rectangle on a waveform, and it is now the `select_box` step a plan asks for (see the Found-by-use entry below, and `docs/decisions.md`).
- ✅ **D4 — Copy, cut and paste as gestures** *(done 2026-08-14)*. The three verbs over a selection, against the typed clipboard (O7): copy reads what the host has **mapped** — which is what it may honestly copy, and a source it cannot read declines and says so — while cut and paste leave as intents. The length change a cut implies is the owner's to make; the host's part is to emit it and to redraw what came back.

  **The host's clipboard stopped being a `String`**, which is the K6 replacement O7 named and left for this track. `crates/clausters-gui` links `clausters-document` for it — serde and serde_json, both already there — because building the same JSON by hand would be a second definition of a format whose whole point is that one clipboard crosses a window, a def and a process unchanged. `host::clipboard::Clip` is that document plus the payloads it *names* rather than holds, and the text case is not a special case: a field still cuts and pastes a string, through the kind that is text, which is also what keeps the browser front's swap of the page's clipboard a matter of a string crossing.

  **The three verbs split exactly where the host's authority does**, and that turned out to be the whole design rather than three features. A **copy is a read**, so the host makes it alone: the selected span leaves the material it has mapped through `Element::sample_block` — the element's own answer, since only it knows what it holds — and lands on the clipboard typed, with the rate it was taken at and nothing resampled. A source it cannot read **declines out loud** (`"refused" "copy" <reason>`): a mapped pyramid is an overview and a live view has no addressable past, and a block of silence on the clipboard is the one answer worse than no. A **cut** and a **paste** change data the host does not own, so they leave as `"cut" start len` and `"paste" position kind json [blob…]` and the owner answers with what the composition now is.

  **A paste carries the clipboard with it**, which is the one call worth recording: the clipboard is the *host's* precisely so that a block copied in one window pastes against an owner that never saw the copy, so the owner cannot be expected to keep one. It rides as the typed document plus one blob per bulk payload — the `/gui_def` convention, JSON as the payload and OSC as the framing — rather than through a mapped file, because that is also the browser's only possible path and one path both fronts share beats two that drift.

  **What the Python leg can honestly do, and what it says instead.** `Editor` answers both payloads: a cut whose selection covers a clip **removes that placement**, through the crate, undoably — an arrangement is elements placed in time, and a placement is what this editor owns. A cut running *across* a clip implies a new length for the material under it, and a paste of a `samples` block is material with neither a source nor a source's owner; both are refused **with their reason**, because a key that silently does nothing teaches that it sometimes does not work. Writing material against a working copy is D1's, and this is the seam it will arrive at rather than a gap. `_set_placements` is the projection that makes an undo of a cut restore the *same* element object rather than a rebuilt one, which is what keeps a widget registry and a pending edit addressing something that exists.

  `editor.py` is the ear check and it needs no destructive edit at all: Ctrl+C over a selection, Ctrl+V, and the block plays out of a buffer of its own — copy a range, hear that range, with nothing written over the take.
- ⬜ **D5 — Spectral selection** *(was D6)*. **The gesture is already in place and waiting for it**: D3's `select_box` step declines over a spectral view and the plan falls through to the plain span, so what D5 adds is the *answer* — the step returns a `BinRange` instead of declining, and a binding written today (`"select_box select"`, as `editor.py` carries) starts drawing a rectangle there with nothing rewired. Until then a spectrogram sweeps time only, which is the one thing a reader notices about it. A drag over a spectral presentation selects frames × bins, drawn as the cells it holds; the read-out gives the time span, the frequency band and the **window-wide footprint** the overlap implies, plus the region's energy through the A-track descriptors. The one gui-side geometry piece is the display→(frame, bin) inversion, which the frequency ruler already owns.
- ⬜ **D6 — The lasso** *(was D7)*. A free-hand region over a spectral plane, kept as a **mask** rather than as a polygon; the same gesture is available wherever a container has two measuring axes, so a value plane gets it for free. The drawn edge is display-only; the mask is the crate's (O6).
- ⬜ **D7 — Spectral drawing and resynthesis** *(was D8)* — **experimental**, in the G20f sense: it ships behind the `editable` opt-in, is evaluated on what it sounds like, and is promoted or dropped on that evidence. The complex analysis for the picture, and a brush over the magnitude plane (attenuate, boost, erase). **What leaves the host is the edit as the parameters of a spectral operation, and the server performs it** — the placement was settled in `crates/clausters-document/PLAN.md`: one place does audio processing and it is the server, so where the operation does not exist yet a UGen is written for it rather than the host growing an inverse transform. The host keeps what it always had, the analysis a picture needs. Material with no original phase is where phase reconstruction (Griffin–Lim, or a documented refusal) is decided **with the evidence recorded**, not before. **Acceptance (experimental bar):** erasing a partial from a two-partial tone leaves the other partial audibly clean, the round trip with no edit is transparent to within a stated error, and the cost on an editor-length file is measured and written down.

- ✅ **D8 — The editor opens an element as a signal view** *(added 2026-08-14, out of the roadmap pass that took A7 apart; **ordered before D1**, the number being the next free one rather than a position)*. `Editor.open_signal(host, element, layers=…)` is `open_pianoroll`'s sibling (G24c): a render mode that draws **one** element as the editor-grade signal view — its material resolved the way a clip's body already resolves it (a mapped source or a pyramid), on the window's navigation group, so the ruler, the selection, the cursor read-out and the playhead are the ones that already exist. It is what the roadmap's Phase 2 carried as "A7's client half, narrowed", and it comes here rather than there because it is a view of the **arrangement** before it is a view of a measurement: what it opens is an element of the composition, and the layers are a parameter of the opening.

  **Why it is a milestone of its own and why it comes before D1.** D1 and D2 edit the samples of an element, and today the editor cannot open one — so their example would be a free-standing waveform beside the arrangement (`bulk.py`) rather than a view *of* it, and the seam that connects the two would be written afterwards, against gestures already shipped. It also decides one thing neither D1 nor A2 can decide alone: **what the editor considers openable**, which is the generated/generator distinction again — a rendered element has material to draw and a generator has to be rendered first, exactly as `open_pianoroll` shows a bounced generator read-only.

  **Requires A2** for the `layers=` half only (a stack of one plain trace is a stack), and nothing else. Example: the editor of `editor.py` opening a take from the arrangement.

  **What shipped, and the correction it forced.** `Editor.open_signal(host, element, layers=…)` draws one rendered element as the editor-grade waveform, registers it as both a lane (the playhead, the cursor read-out and `locate` address it) and a signal (a selection swept there is a selection *of that element*), and refuses a generator by name before a window exists. `layers` is a **property**: assigning it on an open view sends one `/gui_set measure` — no redefine, since a redefine rebuilds every widget (leaving a handler bound by name holding a dead id) and the host reopens the window on one. The example is `take.py`.

  It began as a *lane of layers* — one non-navigable signal element per measure, sharing the lane's axis — which is what A2's "a stack is a placement" implied, and it cost two host additions before the picture was still wrong. **Every view of a signal paints its own field before it draws**, so two of them on one rectangle are not layers: the second is a lid. The stack moved inside the element (`measure: "peak rms"`), the two additions were reverted, and A2's decision entry now records what was believed, why it fails and which tests let it through. Three defects the example turned up on the way: `Editor.poll` drained the host without routing to the handles' `on_event` (so **no** example driven by it had working buttons), a take played 20 dB down because a note's default `amp` overrode the def's (`Buffer.to_event` now states `amp` beside the `legato` it already stated), and the level body was measured over whatever a pixel covered — see the `⬜` entry in "Found by use" for what is still open there.

- ✅ **H9 — A picture reads the span it is looking at, wherever it is running.** *(opened 2026-08-19 by the user, on the rule the whole platform seam is judged by: "the behavior should be identical between the native window and the browser's canvas. I suppose it was because of implementation details that differ." It was — and a detail of implementation is exactly what may differ, so this is the milestone that stops it showing.)*

  **What is different today, and why it is not acceptable.** A native host **maps** the server's memory, so a view reads the cells themselves and zooms to the sample with nothing asked of anybody. A page maps nothing: after H8's stream it holds the *overview* of a take (min/max/energy per bucket) and its own copy of the samples, which for a take recorded in the server is silence. So the two draw the same picture down to one bucket and then diverge — the native one keeps resolving, the page stops. That is a **route** difference (mapping versus a wire) surfacing as a **behaviour** difference, which is the one thing the platform seam is supposed to prevent.

  **The fix is the fetch the plan already owes**, named in Found by use ("A page cannot follow another peer's edit"): `/buffer_getRange` over a **span** rather than a whole buffer. One machine serves both consumers — a zoom that went past the summary, and a peer's announced edit — which is why they are one milestone and not two.

  **The shape.** A view drawn finer than its base bucket over material it cannot answer for **says so** (the draw pass knows the zoom and the span; it is the only place that does), the leg fetches that span, and the samples arrive as a **window** under the overview: `Samples::Window { start, data }` beside the owned and shared forms, with `covers(a, b)` deciding per column which regime can answer. Nothing else in the drawing changes — `measure` already picks the samples when they are finer than a bucket and the summary otherwise; what it gains is that "the samples" may be a run rather than the whole take.

  **What it must not become.** Not a prefetch: a window is fetched because a person is looking at it, and one window at a time per view is what a zoom actually needs. Not a cache with a policy — the window is replaced when the view moves off it, and the pyramid is always there to draw from meanwhile, so a slow reply degrades to the picture that is already correct. And not a second answer: where a window and the summary overlap they measure the same material, which is the property the tests assert.

  **Acceptance**: a page and a native host open the same take and are zoomed to the sample, and the columns agree; a take recorded in a page zooms to the sample after it is finished; a peer's `/buffer_touched` span is re-read and redrawn in a page, which is what closes that Found-by-use entry; and the fetch is bounded — one span in flight per view, none while the zoom stays above the bucket.

  **Done 2026-08-19, and the shape held.** `Samples::Window { start, data, total }` is the third form beside owned and shared, and the regime question stopped being *are there samples* and became **`covers(a, b)`** — one predicate, answered by a whole buffer, a mapping and a run alike, which is what made `measure` take the window with no branch of its own. `WaveformData::set_window` installs one and replaces the last (one run per view, no policy), and refuses where the material already answers for itself. The draw pass leaves the note (`frame::missing_span` → `WaveformSlot::wanted_span`, a `Cell` because the draw borrows the slots immutably) and each leg turns it into `BufferFetches::want_span` — the same chunk conversation as a whole buffer with an origin, landing as `FetchStep::Window` for the one view that asked.

  **Two rules the first version was missing, both found by running it.** A view whose material is **being written** asks for nothing: while a take was recording, the opening view was already finer than a bucket, so it fetched the take's *silence* and then reported itself covered — the picture would have been a flat line under the summary for the rest of the session. And a report's `startFrame` rides as a **long**, which the reply reader had as an `Int`; every report was dropped in silence until that was read both ways (`a_stream_report_reads_the_start_frame_in_either_width`).

  **What this leaves unresolved, and it is named in Future directions** ("A view of a server buffer has two ways of getting its material..."): the *fork itself*. A view still either downloads the whole buffer or takes shape-summary-spans, and what chooses is `fills` — a question about recording standing in for a question about cost. H9 built the second path; deciding when each is right waits on a summary a client can ask for.

  **What is accepted rather than fixed here**: `/buffer_touched` in a page still says the copy is behind rather than re-reading the announced span. The machine it needs now exists — a span fetch keyed by a view — so the Found-by-use entry stays open with that named as its route, and closes with whoever takes it.

  **The acceptance is `clients/web/tests/recording-host.html`**, which asserts what the host *did*, off its own console: a take being recorded is asked its shape and never downloaded; zoomed out nothing is read (the summary is the answer and is on screen); and a zoom to 2000 frames reads exactly 2000 frames back. The by-eye twin is `clients/web/examples/views/recording.html`.

**What stays out of the track**: the typed selection, the typed clipboard, the intent vocabulary, the acknowledgement and the edit log — all of them the document's, and all of them in `crates/clausters-document/PLAN.md`; the client-side editing operations over a resolved range (O9 plus whoever owns the algorithm); and destructive editing verbs in the host, which is the track's premise again.

## N track — notation: the score model, and what is written on it

G31 built the widget and G31h moved the whole notation layer into the workspace
(`clausters-notation` + `clausters_core::notation`), where every client rebinds
it. What neither of them did is give notation a **model**. v1 writes a
monophonic run of plain note values out of a client's `Event`s, and the four
things it cannot say — performance nuance, tuplets, real polyphony, tonal
spelling — were carried as one line (`G31g`) until they were sized. Sizing them
first split them into four encoder refinements; reading the layer against
itself split them differently, and the track below is the second reading.

### What the layer turned out to be missing

Four findings, taken in the order they were found. Each one moves the work
somewhere the encoder refinements could not reach.

**The inverse path already exists, and it is written by hand.**
`Score.display_list()` returns a `notes` layer — `{"t", "dur", "pitch", "id"}`,
ms and MIDI pitch — which is notation to events, and
`clients/python/examples/notation/score_from_data.py` turns it into a `Timeline`
with `Event(midinote=…, dur=…, amp=0.11)`. That hardcoded amplitude is the
finding: the page has no dynamics to give, so the example invents one. By this
project's own rule an example that cannot be written with the supported surface
is reporting a missing feature, and this one has been reporting it since it was
written.

**Each direction goes through a different intermediate.** Forward is `Event` →
`Slot` (the client's half) → MEI (`voice_to_mei`, the shared half). Backward is
MEI → verovio → `NoteEvent` (`t`/`dur`/`pitch`/`id`, already flattened to ms) →
whatever the driver assembles. They are not one seam walked in two directions:
the `Slot` — the deliberate, total, ABI-crossing form — takes no part in the
return. That is why the forward path can be enriched and the return cannot;
there is nowhere to put what is enriched.

**`Score` is an action API, not a data structure.** There *is* an editable score
today (`clausters_notation::Score`: MEI document, `edit`/`transpose`,
undo/redo), but it is an opaque handle to verovio's document — it cannot be
walked, inspected or composed, only sent actions keyed by `xml:id`. Structural
editing needs a structure, so "add score editing" and "build the model" are the
same work.

**Owning the model makes verovio a layout engine again, and that is already
half-paid.** The MEI *writer* is ours (`voice_to_mei`); the undo stack is ours
(MEI snapshots, because verovio's cannot survive the reload its own stale caches
force); the edit cycle is action → commit → reload for the same reason. We
already pay the price of not trusting its editor without taking the benefit.
**The benefit is not the pin** — that was freed by upstream, not by us: 6.3.0
shipped with the editor working and `third_party/verovio.pin` took the release
tag on 2026-08-21, so `Toolkit::Edit()` being dead in 6.2.1 (`docs/decisions.md`,
"Score editing: verovio's editor is dead in the released wheel") is history and
not a reason for anything. The benefit is that a model rewriting its own MEI
never enters that cycle: no commit-then-reload to refresh caches an edit does
not invalidate, no snapshot stack shadowing an editor's own — and, the half that
matters, **the structural edits its action vocabulary cannot express** (insert a
measure, split a voice, change the meter) become possible rather than refused.

### Where the model lives, and why it is not a client module

**In `clausters_core::notation`, whole.** The axis is not "general versus
notation-specific" — it is **pure versus platform**. `clausters-notation` is the
native libverovio binding: it has a `build.rs`, depends on C++, sits behind a
default-off `verovio` feature and does not compile to wasm, and verovio's own
wasm build only emits SVG, so in a page that crate does not exist at all. The
precedent is already built and is exactly this case: the `Engraver` **trait** is
in core, and only the layout engine is per-platform — `impl Engraver for
Toolkit` in `clausters-notation` (native), `impl Engraver for JsEngraver` in
`clausters-core-web` (the page). `Score`, `NoteEvent`, `Page`,
`svg_to_display_list` and `voice_to_mei` are already core's.

So the model and its operators go beside them, and `clausters-notation` gains
nothing: it stays the native `Engraver`. **A Python/TypeScript module holding
the model would reopen G31h**, which moved this layer down for exactly this
reason ("it pushes all the notation logic onto the client, so a second client
would reimplement every line"), and would break the standing rule that the logic
is in Rust and there is one of it. MEI names no language's types, so nothing
about the model is client-native; what stays in each client is only the
`Event`/`Timeline` reduction, which is where the line already falls.

### What the model is

**Two durational structures that coexist, neither containing the other.**

- The **metric layout** — measures and meter changes. It does not sound. It is
  a structure of its own, addressable and editable, carrying meter changes, an
  anacrusis and irregular bars; today it is a `"4/4"` string the encoder uses to
  split and then discards.
- The **content** — voices of notes, chords and rests. It sounds. It is flat:
  notes are *not* nested inside measures, because containment breaks under every
  operation over a whole score (augment a phrase and the containment is wrong).

The two are independent, and that independence is what makes ordinary editing
expressible at all. Three cases that cannot even be stated today: *transpose
measures 3 to 10* (the grid resolves a span of musical time and the operation
applies to the content in it — the measure is an **addressing system**, a role
it cannot have while it is a by-product of emission); *augment a phrase and
reorganize it against the current measures* (the content changes length, the
grid stands, the barring is recomputed); *change the meter* as an operation on
the grid itself rather than an argument to the encoder.

It also makes **metric position** computable, which is meaning and not layout:
accenting beats 1 and 3 of a 4/4 is a function of position against the grid, and
only a grid you can query can answer it. Note that this is style, not notation —
whoever interprets it must be replaceable.

MEI nests (`<measure><staff><layer>`), so emission **projects** flat content
onto the grid: the split-and-tie `voice_to_mei` already does. A flat model and a
nested document are not in conflict.

**Durations are exact rationals, not ticks.** `TPW = 32` is declared today as
the encoder's foundation — every duration an integer count of 32nds, *so that*
barline splitting and tie decomposition are exact integer arithmetic. Exactness
comes from the rational instead, and a triplet eighth is `1/12`: exact as a
rational, unrepresentable on that grid. Ticks are then **one edge conversion
among several** — ms for MIDI, seconds or beats for OSC, `@dur`/`@dots` for MEI
— each done at its own boundary, exactly as a duration is converted today for
the two protocols. The `Slot` and its `ticks` stay as the v1 wire form; `TPW`
stops being a foundation and becomes an encoder detail.

**What is well written comes before what is heard.** The first thing asked of a
score editor is that the score be *right on the page*; playing it matters, but
only if the player honours what the symbols mean. That orders the track (the
emission milestone precedes the interpreter) and it also says what the return
path is: not a converter but an **interpreter** — staccato shortens, a dynamic
sets amplitude, a slur binds, a tuplet divides the time, a repeat expands, the
metric position accents. It can wait; what cannot wait is that the model
*store* the symbols, because an interpreter can be written later against symbols
that were kept and never against symbols that were dropped.

### Eight constraints on the work

**The operators must not become a second vocabulary.** Concatenating, stacking,
repeating, transposing, augmenting is composition, and this project already has
composition in two places: the arrangement's `Aggregate` with its **concrete**
and **logical** kinds, and the patterns. A score's operators do not replace the
patterns — a pattern is forward-only and may be endless, a score is finite,
random-access and carries spelling, voices and beaming, which is precisely the
**generator**/**generated** distinction the arrangement already draws. Where an
operation is the same operation, it takes the same name it has there.

**The scope is closed by what is emitted, not by the specification.** MEI is
enormous and this is not an MEI library. The model covers what the encoder
writes plus what each milestone below adds — a list that grows per milestone and
is written down each time.

**This track now reaches the host.** The earlier reading of it claimed nothing
here changes the host, and that stops being true once a page can be *edited* by
hand: note entry, changing a value and adding a mark are new gestures, and
`is_element_class` (`note`/`rest`/`mRest`) — which governs hit-test identity —
will have to widen. It stays true for everything that is only *drawn*: the
display list dispatches by tag, so a tuplet bracket, a hairpin or a dynamic
draws with no host line touched and is deliberately not separately clickable.

**Everything is modularized, in the core and in the clients.** The track adds a
model, its operators, an emitter and an interpreter, and they are four things, not
one file that grows. In the core they are submodules of `notation` beside the
ones already there (`mei`, `svg`, `score`, `cursors`); in each client they are
modules of the package `E15` already split by what each part knows
(`engraver`/`mei`/`view`), so score editing never lands as more weight on
`mei.py`. **And what is general does not live under `notation` at all.** Exact
rational arithmetic is the clearest case: it is duration and position
arithmetic, useful anywhere the client counts musical time, so it belongs beside
`measure.rs`, `scale.rs` and `tempoclock.rs` in the core's own namespace, not
inside a notation submodule that happens to have needed it first. The rule is
the placement rule the GUI work already runs on, one level up: **a function that
outlives its first caller goes where its subject is, not where it was wanted.**

**Both clients, in one piece of work: Python first, TypeScript after, neither
postponed.** The order is fixed because the Python client is the reference and
the port is a transliteration of it — but the *deferral* is what is ruled out
here. No milestone in this track closes with one client done and the other
named in a plan entry: a vocabulary whose two surfaces are written months apart
is two vocabularies, and unlike a widget the divergence is invisible, since two
different verb sets both work. This is the standing "the packages move
together" rule, stated again because this track is the one place where honouring
it late costs the most.

**The examples get their own family.** Notation has two examples today, and they
sat in `editors/` next to the multitrack ones and **moved to `notation/` with
N1** (`notation/score.py`, `notation/score_from_data.py`, and
`notation/score.html`, `notation/score-from-data.html` in the web package).
This track multiplies them —
composing a score algorithmically, editing one by hand, hearing one interpreted —
and they are not the arrangement's editors, so they have a family of their own,
mirrored path for path as the `examples` skill requires, and the milestone that
grows an example grows **both** copies of it.

**Every edit action is in Rust; a client only binds it — this is what a
standalone needs.** Not a preference and not only the non-divergence rule: a
host opened on a saved session has **no client language attached**, and the
document crate already decided what that host can and cannot do (a frozen
generator draws, contributes its extent and emits nothing). A score it opens has
to be *editable* there, so the edit vocabulary must be complete on the Rust
side — every verb, its validation and its refusals — and a client contributes
nothing but the name it calls it by. Any edit that only works because a client
computed something first is an edit a standalone cannot perform, which is how
the surface silently splits in two. This is the strongest of the reasons the
model crosses as data and the operations cross as data with it: the standalone
holds the model and applies the same operations, through the same one symbol,
with no client in the process at all.

**And it is not a port debt.** The encoding and the model are the shared half,
and `clients/web/PLAN.md` already records that richer encoding extends it so
both clients gain it at once — it now also names the model and the catalog as
the shape the port follows. **How much of an ABI round this is depends on the
shape N1 settles**, which is why the framing does not decide it: a symbol per
operation is a `docs/bindings.md` row and a `CORE_ABI_VERSION` move for every
verb, while operations crossing as data cost neither and pay in parity instead.
Whatever symbols the model itself needs owe their rows either way
(`tests/bindings.rs` and `test_native_parity.py` fail without them).

- ✅ **N1 — The model: two durational structures, in rationals** *(done 2026-08-29)*. The metric
  layout and the content, each addressable, neither containing the other, in
  `clausters_core::notation`. Durations and positions are exact rationals; the
  grid carries meter changes, an anacrusis and irregular bars; the content is
  flat voices of notes, chords and rests carrying pitch **with its spelling**
  (`F#` and `Gb` are one `midinote` and two notes on the page), ties, and the
  optional per-note marks the milestones below fill in. The document header —
  clef, key, staff grouping — is part of it, since it is what `voice_to_mei`
  takes as arguments today and what a second staff needs to be more than a
  string. `Slot` stays the v1 wire and the model is the thing above it.

  **What crosses the ABI, and what shape the client surface takes, is settled
  here** — not in N2, because every operation N2 writes lands on whatever is
  decided. Four questions, none of them answered today, and each one a place
  where two clients diverge by choosing separately: whether an operation
  **returns a new score or mutates one** (an operator wants values, so
  `concat(transpose(a, 2), b)` reads; editing wants mutation with an undo stack,
  and the two can be one only if it is said so); whether there is **one C symbol
  per operation or the operation crosses as data**; **who owns the intermediates**
  a functional form creates; and **how much sugar each language may add**. The
  shape is already in the tree twice and does not have to be invented:
  `Score.edit(action, **param)` sends an action name and a JSON payload through
  one symbol (`clausters_score_edit`), and `voice_to_mei` is data in, data out.
  Taking that form means **a new operation costs no new symbol** — no
  `docs/bindings.md` row, no `CORE_ABI_VERSION` round — and leaves each client
  what `clausters.gui.notation` already declares itself to be, "a shell of names,
  dicts and one handle whose lifetime Python owns".

  **The cost of that form is parity, and this project has paid it twice.** A
  data-driven surface is invisible to the binding tests: `tests/bindings.rs`
  would see one symbol and no operations, so the two clients could expose
  different sets of verbs with nothing going red. That is structurally the same
  blind spot recorded in this plan's "Found by use" for the props manifest,
  which cannot see a divergence between two builders of one wire type, and the
  same class the UGen contrast tests caught eleven of. So the form comes with an
  obligation attached: **a catalog of the operations, in the core, contrasted
  against what each client exposes**, exactly as the UGen catalog is contrasted
  against the builders.

  **The line the sugar may not cross**, and it is the one rule that keeps the
  vocabulary single: *what computes is Rust, what names is the shell*. Chaining,
  `a + b` for concatenation and `score[3:10]` for a slice are idiom and belong to
  each language — but only while they **assemble the operation** and compute
  nothing. Resolving "measures 3 to 10" to a span of rational time is arithmetic
  against the grid, and two clients doing it separately round differently across
  a meter change, so it is Rust. The one reduction that legitimately stays in
  each client is the one that already does: client-native types
  (`Event`/`Timeline`) into content.

  **Acceptance:** the three operations that cannot be stated today can each be *written down* against the
  model (transposing a span addressed by measure, augmenting a phrase, changing
  the meter), even where they are not implemented until N2; a voice with no
  meter change and no marks round-trips to the same MEI `voice_to_mei` writes
  today, byte for byte; the model compiles to wasm with `clausters-notation`
  absent; the four questions above are answered in writing, with what each
  answer beat; the operation catalog and the contrast that reads it against both
  clients **exist**, even holding only the handful of verbs N1 itself needs, so
  that N2 fills a mechanism rather than building one late; `docs/decisions.md` carries why the model is in core
  rather than in the verovio crate, why the grid is not derived, and — if the
  operation crosses as data — why a surface the binding table cannot see is
  worth its own catalog.

  **What shipped.** `clausters_core::notation::model` — `Sheet` (the name is not
  `Score`, which is already the engraver-driven handle: one is data with no
  engraver near it, the other a handle to a layout engine), `Grid` with its
  meters and its irregular bars, flat `Staff`/`Voice`/`Item`, and `Pitch`
  carrying step, alter and octave so `F#` and `Gb` are two notes. `Marks` is
  declared and unemitted, waiting for N3. The exact rational is **not** under
  `notation`: `clausters_core::ratio::Ratio` sits beside `measure`, `scale` and
  `tempoclock`, because duration and position arithmetic outlives its first
  caller. Emission is `sheet_to_mei`, and **`voice_to_mei` is now a front door
  on it** — the v1 slots become a sheet and the sheet is written — which is what
  makes the byte-for-byte acceptance structural rather than a spot check: the
  six-case golden in `testdata/voice_to_mei.txt` was captured from the previous
  commit and not one byte moved. Two duplications went with it: the encoder's
  own spelling tables (now `Pitch::from_midi`, one rule) and the silent
  `_ => ""` that would have written a triple sharp as a natural (now a refusal).
  The four questions were answered as **data both ways** — the model crosses by
  value, the operation crosses as a payload through one symbol — which dissolved
  the third (there are no intermediates to own) and left the fourth as the line
  *what computes is Rust, what names is the shell*. Symbols:
  `clausters_core_voice_to_sheet`, `_sheet_apply`, `_sheet_to_mei`,
  `_sheet_ops`, all four mirrored in wasm, `CORE_ABI_VERSION` 22 → 23. The
  catalog and its contrast landed with the one verb N1 needed (`transpose`,
  which exercises the span addressing that is the Rust-only half), read against
  both clients by `test_gui_notation.py` and `sheet.test.ts` — the same
  assertions in the same order. Client shells: `clausters/gui/notation/sheet.py`
  and `src/gui/notation/sheet.ts`, both of which `from_notes` now travels
  through, so there is one road to MEI rather than two. Docs: the composition
  chapter of both books, `docs/bindings.md`, and two `docs/decisions.md`
  entries — the surface shape, and the model's own. The examples moved to their
  `notation/` family in the same pass.

- ✅ **N2 — The operators, and the edit verbs** *(done 2026-08-29)*. Three families over the two
  structures, and the whole point of the model. **On the content**: concatenate,
  stack voices, repeat, transpose, retrograde, invert, augment and diminish.
  **On the grid**: insert or change a meter, add or remove measures. **Addressed
  by the grid**: any content operation applied to a span named in measures. The
  editing verbs (insert a note, delete one, change a value, tie, split a voice)
  are the same family — they rewrite the model, and the page is re-engraved from
  it, so **verovio is not asked to edit**. Names come from the arrangement and
  the patterns wherever the operation is theirs, and the whole family lands on
  the surface N1 settled — this milestone writes operations, it does not
  re-decide their shape. Each one goes into the catalog as it is written, and
  into **both** clients in the same commit: a vocabulary is the worst possible
  thing to port later, since two surfaces that offer different verbs still both
  work. **Acceptance:** the composition
  of two operations is the operation on the composed score (concatenating two
  scores then transposing equals transposing each then concatenating), pinned by
  tests; an operation on the content leaves the grid untouched and vice versa; a
  phrase augmented against a fixed grid re-bars correctly, ties across the new
  barlines and sounds the augmented durations; `Score`'s edit path stops calling
  `Toolkit::Edit`, and the undo stack — already ours — steps over model
  snapshots rather than MEI text; and the two clients' surfaces are **read side
  by side, verb by verb**, before either is believed — the rule this project
  already has for a port, applied where what is ported is a vocabulary rather than
  an example. Each client's sugar is checked against the line N1 drew: it
  assembles operations and computes nothing, and a measure range is resolved by
  the core in both.

  **What shipped.** Seventeen verbs, in `clausters_core::notation::operators` (the
  structural ones) and `::edit` (the ones that name a single item), with `ops`
  keeping the vocabulary and the dispatch. Content: `concat`, `stack`,
  `repeat`, `retrograde`, `invert`, `stretch`, `transpose`. Grid: `set_meter`,
  `insert_measures`, `remove_measures`. Items: `insert`, `delete`, `silence`,
  `set_dur`, `set_pitches`, `tie`, `to_voice`. **No new symbol and no ABI
  round** — which is the whole point of the shape N1 settled, and the first
  time it paid.

  The model grew the one thing the verbs needed: **an item carries an id**,
  minted on the sheet, travelling through every operation and re-minted only
  where two sheets meet. Not an index (it moves under every other edit) and not
  a time (it is ambiguous the moment two voices sound). A sheet that never had
  ids gets them on the first operation, so a hand-written payload behaves like
  one this layer minted.

  Each verb had a musical question in it, and the six that a plausible
  implementation gets wrong are in `docs/decisions.md`: concatenation refusing
  to invent a barline rather than silently re-barring; stacking refusing two
  grids; `delete` and `silence` as two verbs rather than one with a flag;
  addressing by id; an interval carrying both its sizes; and time-scaling
  leaving the grid alone, which is what makes it augmentation rather than a
  no-op that looks identical.

  **What is deferred, and why it is not a shrunken scope.** This milestone's
  acceptance also asked that `Score`'s edit path stop calling `Toolkit::Edit`
  and that its undo stack step over model snapshots. The model-side half is
  done — every structural edit is here, including the ones verovio's action
  vocabulary cannot express at all (open a measure, split a voice, change the
  meter) — but wiring the *engraved page* to it needs something this track does
  not have yet: a score opened from typed text (ABC, MusicXML, a hand-written
  MEI) has no model behind it, and giving it one is an **MEI reader**, which is
  N4's inverse path. Doing it now would fork `Score` into a model-backed path
  and a verovio-backed one, which is precisely the divergence this track exists
  to avoid. Recorded here rather than quietly dropped; it is the first thing
  after the reader lands.

  Tests: 22 in the core (the operators' composition law, the two structures'
  independence, the augmentation that re-bars and ties, every refusal), plus the
  same assertions in the same order in both clients. Clients:
  `notation/sheet.py` and `sheet.ts` grew a helper per verb — `delete` is `del`
  in TypeScript, a reserved word being the only place the two spellings differ,
  and the contrast test carries the mapping so a *missing* verb still fails.
  Example: `notation/compose.py` and `compose.html`, a piece built entirely by
  operating on a motif, whose last cell stacks it against itself and is
  **refused by name** — the operators say counterpoint long before the emitter
  can write it, and saying so is better than dropping a voice.

- ✅ **N3 — That it is well written** *(done 2026-08-30; the host's own note-entry gestures deliberately left out — see below)*. The emission of everything the model can
  now say, judged **by eye** on the page, which is the first thing a score
  editor owes. This is where the four old refinements land as *emission* rather
  than as encoder branches: **tuplets** (rationals make them representable in
  N1; here the encoder detects the grouping and emits `<tuplet num numbase>`,
  reconciled with the barline split, verovio drawing the bracket, the number and
  the rebeaming); **polyphony** (several voices on a staff, several staves under
  a brace — in the model this is stacking, not a refactor of the encoder);
  **the markup verovio already knows** (articulations, dynamics and hairpins,
  grace notes, ornaments, stem direction) and **tonal spelling**, including the
  difference between the *written* accidental (`<accid>`, forced) and the
  *sounding* one (`@accid.ges`, which hands the decision to the engraver holding
  the key signature); and **written duration** shorter than the slot. Two
  cautions the surface work found: reading `legato` as staccato would restyle
  every existing score, since `0.8` is an audio default and not an intention,
  and the same holds for `amp` at `0.1` as a dynamic — notation intent is
  declared, never inferred from a performance default. The **hand that edits**
  belongs here too and may be taken on its own: the host's note-entry gestures
  and the widening of `is_element_class`. **One found-by-use it has to fix, or
  the cursor lies**: `staff_systems` (`clausters_core::notation::cursors`)
  clusters systems on "the gap between systems is far wider than one between
  staff lines", which reads a two-staff grand staff as *two* systems and gives
  the playback cursor the wrong span. **Acceptance:** three eighths in the time
  of two engrave as a bracketed triplet; a tuplet that would cross a barline is
  kept whole or refused with a message, never silently snapped; a piano texture
  engraves on two staves under one brace and the cursor spans both; a scale in a
  key with a signature prints no accidental its armature implies while a
  chromatic note prints its own; a phrase with staccato, a hairpin and a grace
  note engraves with each mark placed by verovio; a score with none of this is
  byte-identical to what the encoder writes today.

  **What shipped.** The projection was rebuilt: every voice of every staff is
  laid over the same measures and the measures are assembled from the
  projections, so polyphony is `<layer>`s and a grand staff is two `<staffDef>`s
  under a brace with one barline through both. A voice that runs short is filled
  with rests so the staves stay in step. **Tuplets need nothing declared**: the
  ratio is *in the rational* — a denominator carrying an odd factor is inside a
  tuplet, and that factor is how many notes are in the time of the nearest power
  of two below it — so consecutive items sharing one ratio become
  `<tuplet num numbase>`, and a group that would cross a barline is refused by
  name rather than split, since splitting it would stop it being a tuplet.
  **Every element now carries the model's own id** (`n7`, `n7-2` for a split
  piece, `n7-p1` for a chord's pitch), which is what will let a gesture on the
  page name a note in the model. Marks reach the page — articulations, a forced
  stem, grace, and `dur.ges` for a note that sounds shorter than it is written —
  and what MEI hangs off the *measure* rather than the note is gathered
  separately: `<dynam>`, ornaments, and the two-ended things, which the sheet
  now carries as `spanners` because a slur has two ends and cannot live on an
  item. Three verbs went with them (`set_marks`, `add_spanner`,
  `remove_spanner`), taking the catalog to twenty.

  **Two findings, both of which reversed something.** The accidental rule was a
  hypothesis this plan carried and told whoever implemented it to verify;
  verifying reversed it. Verovio **infers nothing** — `<accid>` is always drawn,
  even where the armature implies it and even twice in a bar, and `@accid.ges`
  is never drawn — so the suppression is the encoder's, and it needs both halves
  the hypothesis denied: the key signature *and* a per-measure memory. It also
  turned up a silent wrong the old encoder had been writing for as long as it
  existed: a C in a key that sharpens C came out with no sign at all, which
  reads as C sharp. And the cursor's grand staff was fixed by reading a
  **barline through the brace** instead of a gap threshold, since no distance
  can tell a braced pair from two systems. Both are in `docs/decisions.md` with
  the measurements.

  **What the by-eye pass caught, which nothing else would have.** Reading the
  engraved page turned up a defect the tests could not see: `concat` and `stack`
  carried the notes of the score they joined and **dropped its spanners**, so a
  slur written over a phrase vanished the moment the phrase was joined to
  another — the music still there, the mark not, and nothing said so. Renumbering
  now carries what points at the renumbered ids, and what *removes* items prunes
  the spanners left dangling, since a caller who deleted a note did not ask for a
  score that can no longer be engraved. Two things the same pass showed were
  **not** defects and were the example teaching badly instead: two voices an
  octave apart on one treble staff are drawn correctly and read as a mess of
  ledger lines (the coda moved to a staff of its own in the bass clef, which is
  also what shows the brace), and a note written a quarter that *sounds* an
  eighth is `sounding` working — the example now says you will hear it.

  **And two more the by-eye pass caught, both reported by ear and by reading the
  page.** A **sounding length written as `@dur.ges`** does not shade a note: an
  engraver reads it as the real duration and advances its own clock by it, so a
  staccato quarter written that way pulled every attack after it a quarter-beat
  earlier and the measure came out short. Measured three ways and recorded; the
  emitter now writes the articulation and not the length, the model keeps the
  fact, and honouring it is `N4`'s — which is the track's own division between
  what is written and what is heard. And a **rest covering a measure is
  `<mRest/>`**, drawn centred, where a decomposed whole rest hangs at the start
  of the bar and reads as a rest on the downbeat with something after it — the
  case that matters being a rest *longer* than a measure, which is what an empty
  staff under a written one actually is, and which a rule written only for a
  one-bar rest leaves untouched.

  The golden was **re-recorded twice**, both times deliberately, and its
  docstring says which four things moved and why. Tests: 86 in the core; the same assertions
  in both clients, taking them to 34 and 10. The example's coda is now the
  two-voice ending it always described, with its marks and its slur, and it
  grew the tuplet that is refused for crossing a barline — in both copies.

  **The hand that edits is not here**, which this milestone's own text allows
  ("may be taken on its own"). Note entry is a gesture in the host and needs
  `is_element_class` widened, and it lands on the same seam `N2` deferred: the
  page's edits reach the model through ids, and a score opened from typed text
  has no model until the reader `N4` brings. Taking them together is what makes
  both cheap.

- ✅ **N4 — The interpreter: notation to events** *(done 2026-08-29; the MEI reader and the two pieces waiting on it — `Score`'s edit path onto the model and the host's note-entry gestures — are not here, see below)*. The return path, and the
  reason it is a milestone rather than a conversion: the player must **honour
  what the symbols mean**. Staccato shortens, a dynamic sets amplitude, a slur
  binds, a tuplet divides the time, a repeat expands, and the metric position
  accents. Numeric and timing rules, so it lives in the core and there is one of
  it — with the default interpretation **replaceable**, since accenting 1 and 3
  of a 4/4 belongs to a style and not to the notation. It needs the one thing
  notation cannot carry, a **staff-to-instrument binding**, and the project has
  the precedent and the wording for it (`docs/decisions.md`, "a buffer sounds
  through an instrument"): explicit at render time, never inferred from the
  page. What it replaces is named: `notation.to_notes`/`to_timeline` instead of
  the hand-written converter in the `score_from_data` example, whose `amp=0.11`
  is the missing feature this closes. The flat `notes` layer does not
  go away — it is what the cursor and the timemap need — it stops being the only
  way back. **Acceptance:** the example's converter is deleted and the example
  reads the same or better; a staccato note sounds shorter than a plain one of
  the same value and a hairpin is heard across its span; a score composed
  algorithmically in N2 plays; an interpretation is overridden without editing
  the core; the round trip is honest — what a trip through notation loses
  (exact timing, microtones, continuous amplitude, the instrument) and what a
  trip through events loses (spelling, stems, beaming, which voice) is written
  down rather than found.

  **What shipped.** `clausters_core::notation::interp`, and it is a layer rather
  than a function because **no symbol can be read off the note it sits on**: a
  dynamic governs the notes after it, a hairpin is a shape over a stretch of
  them and lives on neither end, a tie makes two items one sound. A performed
  note carries **two lengths** — `dur` written, `sustain` heard — which is the
  same distinction `@dur.ges` violated from the other direction, and it maps
  onto the client `Event`'s own `dur`/`sustain` with nothing invented: the model
  and the event had agreed all along and only the encoder had not. Beside them
  it carries the `staff` and `voice` it came from, which is where the
  **staff-to-instrument binding** attaches — explicitly, at render time, as
  `docs/decisions.md` already required of a buffer sounding through an
  instrument.

  **The reading is a value the caller holds** (`Interpretation`), and it comes
  *back* from the core (`clausters_core_interpretation` / `interpretation`) so
  an override starts from the defaults rather than restating them — two ABI
  symbols rather than one, because a client that wrote the dynamics table down
  for itself would play the same score at a different amplitude and nothing
  structural compares two tables. That second call is the **parity surface for
  the reading**, exactly as `sheet_ops` is for the verbs. `CORE_ABI_VERSION` is
  24.

  **The defaults claim as little as a player can and still be playing**: the
  marks mean roughly what a dictionary says, `detach` and `slur` are both `1.0`
  so an unmarked page is read literally, and the only metric stress is the
  **downbeat**. One and three of a 4/4 is a style, and a style passes its own
  accents — which may name the meter they apply in, since half a bar is a
  different place in a 4/4 and in a 3/4.

  **Two clauses of this milestone needed no code, and that is a finding rather
  than a gap.** A *tuplet* divides the time exactly in the rational the item
  already holds, so onsets land on it without the interpreter knowing tuplets
  exist; a *repeat* is not a symbol this model carries at all, because
  repetition is written out by `repeat` (`N2`), so by the time a sheet exists
  there is nothing to expand. Both are consequences of decisions taken two
  milestones earlier, and both are written into `docs/decisions.md` because
  "the player must expand a repeat" is the kind of requirement that gets built
  before anybody checks whether the model can state one.

  The examples' hand-written converters are **deleted**, in both clients: what
  plays is now the interpretation of the sheet, and `score_from_data` stops at
  the model on the way out so both directions leave from one place. Their
  `TEMPO` moved to `2.0`, which is the quarter = 120 the engraver times a page
  at — measured, not assumed (an engraved quarter is 500 ms), and it is what
  keeps the cursor locked to the sound now that the sound is timed from the
  model rather than from the timemap. Tests: 15 in the core, and the same
  assertions in the same order in both clients, taking them to 41 and 20.

  **The MEI reader is not here**, and with it the two pieces that were deferred
  onto this milestone because they need one: `Score`'s edit path moving onto the
  model (`N2`) and the host's note-entry gestures (`N3`). This milestone's own
  acceptance is the interpreter and is met; the reader is the *other* return
  path — text back into the model, rather than the model into sound — and
  putting them in one milestone conflated two directions that share only a
  preposition. It is the next thing, and `ROADMAP.md` says so.

- ✅ **N5 — Events to notation, enriched** *(done 2026-08-30)*. The forward path catches up with the
  model: the client-side reduction reads more than `midinote` and `dur`, and the
  slot's optional fields carry it. This is last on purpose — it works at v1
  level today, and every field it adds is a field the model already holds, so
  it is filling a structure rather than growing one. The **note-entry** surface
  question the earlier reading made its first milestone — how a client *says*
  notation intent, which is not the client surface N1 settles — is
  answered here, and is smaller than it looked, because the three candidates are not competitors but carriers for
  three different classes of data: **per-note intent** (articulation, dynamic,
  grace, spelling) rides the slot, which is symmetric — the same field is
  written going out and read coming back; **spans and document-level material**
  (slurs, hairpins, tuplet groups, meter, repeats, the brace) cannot live on an
  event at all and travel beside the voice, as the model's grid and header;
  **bindings** (staff to instrument, voice to channel) are neither and are
  N4's. Whatever a client reads from an `Event`, the rule the fields obey is
  that they are **musical facts, not encoder directives** — `articulations:
  ["staccato"]` and not `shorten_drawn_value`, `accidental: written | sounding`
  and not `print_accid` — because a fact can be read back by N4 and a directive
  can only be written. Growing `_RESERVED` is part of the correctness of any
  reserved-key spelling, not a tidy-up: `_control_args` forwards every numeric
  non-reserved key to the synth, and `bool` is an `int` in Python, so a stray
  `grace=True` is sent as a control `1.0` and ignored in silence.
  **Acceptance:** a phrase carrying no notation intent produces exactly the
  slots it produces today; a phrase carrying it engraves the marks N3 emits; a
  score written from events, interpreted back by N4 and written again is stable
  in what both directions agree on, and what they do not is the documented loss.
  **All three met**, and three things the reading did not foresee:
  - **A `sustain` is not a `sounding`.** The client's `legato` shortens every
    note ever sequenced, so reading it would put a sounding length on all of
    them — and a note that is both staccato and short is *one* fact, so writing
    both would shorten it again on every round trip. The rule that came out is
    the one that makes the trip stable: `sounding` is what the sustain says
    that **no symbol said**, and nothing where the note is held for its written
    value.
  - **The slot stopped being an untagged enum.** Totality stays (a slot with no
    pitches *is* a rest) but as a struct with a defaulted pitch list: with nine
    optional fields the enum had a trap, since a note whose `articulations` was
    mistyped failed the note variant, matched the rest variant, and became a
    **silent rest**. The struct refuses unknown keys instead.
  - **A page's selection could not reach a verb.** `Score.transpose_to` takes
    an element id and every other verb takes an item id, so a click could
    select a note and edit nothing but its pitch. `clausters_core_item_id` is
    the missing step (`CORE_ABI_VERSION` 27), answered by the emitter that
    spelled the element rather than by each client spelling it out again.

  Its example is `score_editor.py` / `score-editor.html`, which is also the
  example `N6` shipped without: a document opened, given a title, a double bar
  and a system break, edited by hand through the model's verbs, written on with
  note entry, and played back from the model.

- ✅ **N6 — The reader: a document back into the model** *(done 2026-08-29)*. The direction `N4`
  turned out not to be. A `Score` opened from typed text — ABC, MusicXML, a
  hand-written MEI, anything the engraver auto-detects — holds a document and no
  model, so every verb the model has is unavailable on exactly the scores a user
  is most likely to open. What it unblocks is already written down and waiting
  on it by name: **`Score`'s edit path onto the model** (deferred in `N2` —
  today an edit is a verovio editor action, which is a second implementation of
  what an edit means and the divergence this track exists to avoid), and **the
  host's note-entry gestures** (deferred in `N3` — a gesture on the page names a
  note by id, and an id means nothing without a model behind it). Reading is
  narrower than it looks: the engraver normalizes whatever it loaded to MEI
  (`Engraver::mei`), so there is one input format rather than four, and the
  ids are already there. What it must not do is lose what it cannot represent —
  a document holds more than the model does, and a reader that silently drops it
  turns "open, edit one note, save" into a lossy pass over somebody's score.
  **Acceptance:** a score typed as ABC opens, is transposed by `transpose` and
  re-engraves; a note dragged on the page moves the model's item and not
  verovio's document; what the reader cannot represent is named in its refusal
  or preserved, never dropped in silence; and a sheet written out, read back and
  written again is byte-identical.

  **What shipped so far** *(2026-08-29)*: the reader itself
  (`clausters_core::notation::read`, `clausters_core_mei_to_sheet` /
  `meiToSheet`, `CORE_ABI_VERSION` 25), which meets three of the four
  acceptance clauses — an ABC score opens, is transposed and re-engraves; what
  is not a score says so; and the round trip is byte-identical.

  Measuring first is what shaped it. **One input format, not four**: the
  engraver normalizes whatever it loaded to MEI, so there is one parser.
  **An element with no id was invented by the emitter** — the padding that
  completes a bar and keeps voices in step is not content, and reading it back
  made a score gain a bar of silence for having been saved. **Split pieces
  rejoin** by their shared id. **Verovio respells a tie** as a `<tie>` element
  hanging off the measure, so reading only our own `@tie` would lose every tie a
  score picked up by being saved once.

  The model grew where a document is musical, which is what "never dropped in
  silence" cost: a **header** (title, subtitle, composer, lyricist — named
  fields, not a map), a measure's **right barline**, a **break** before a
  measure, and **beams** as spanners. The line is the one `Marks::stem` already
  drew — the engraver's default is the engraver's, a writer's choice is the
  model's — and it was drawn there after the first reading of it ("layout is
  recomputable, so it is not loss") turned out to be true only of the beams and
  breaks nobody chose. Three verbs went with it (`set_header`, `set_barline`,
  `set_break`), taking the catalog to twenty-three. Tests: 14 in the core and
  the same assertions in both clients (47 and 26).

  **Then the edit path** *(2026-08-29)*, which is the second acceptance clause
  and the piece `N2` deferred. `Score::apply(&Op)` takes the same payload the
  sheet verbs build, applies it to the model, re-emits and reloads — so an edit
  to an open score and an edit to a sheet in hand are one operation through one
  implementation, and a standalone host holding a sheet performs it with no
  engraver anywhere. The engraver's editor stays as the escape hatch it now
  honestly is: a document that could not be read into a model still draws and
  still plays, and `transpose` falls back to `keyDown` for it. `Score::sheet()`
  says which of the two a caller has.

  Dragging a note is `move_steps`, a **new verb** and not transposition: moving
  along the staff takes the key signature's alteration for the letter it lands
  on, because that is what reading in a key means. `Op::Transpose` is the other
  act — a named interval, keeping the alteration the arithmetic implies — and
  conflating them is how a drag in a flat key produces a natural nobody asked
  for. Catalog at twenty-four; `clausters_score_apply` and
  `clausters_score_sheet` take `CORE_ABI_VERSION` to 26.

  Writing it caught a defect in the reader that the round-trip tests could not
  see, because both halves of the round trip were wrong together: the emitter
  writes no accidental where the armature already gives one, so reading a bare
  `pname="b"` as B natural turned **every B flat in E flat into a B natural on
  the first save**. The reader now runs the emitter's own rule backwards — each
  measure starts from the armature, a printed accidental holds to the end of it
  — and reads an accidental from all four places it can be written, since our
  emitter states the sounding one as a child `<accid>` while verovio hands back
  attributes.

  **And the hand that writes** *(2026-08-29)*, the piece `N3` deferred and the
  last of this milestone. A press on blank paper inside a staff reports
  `"insert" <after> <position> <staff>` on a page that took the new `entry`
  prop — its own opt-in and not a second meaning for `editable`, because the
  gesture takes over one that already does something: on every other page a
  press on blank paper clears the selection, and a page that had not asked for
  note entry would report an insertion every time a user dismissed one.

  **It names a place and not a note.** A staff position becomes a pitch only
  once something knows the clef and the key, and the host knows neither; a
  duration is not implied by a click at all. Reading the place is
  `notation::pitch_at` in the core, reaching the wire as an optional `position`
  on the `insert` verb — no ABI, since the operation was already data — and
  clicking the middle line in E flat writes a B flat because the armature is
  part of what the place means.

  **`is_element_class` was widened, though not in the direction the earlier note
  expected.** The problem was not which classes are elements but that the host
  could not tell: to a renderer an id is an id, a staff's lines carry the
  staff's, so "the nearest element to the left" was always a staff line and an
  insertion "after the staff" names nothing a model can resolve. The walk is the
  one place that knows, so the display list grew an `elements` list — a sixth
  page key — and the host filters by it. The notation parity vectors moved with
  it, and their normalizer learned the new field: a list of *ids* rather than
  objects carrying one would otherwise have compared two engravings' minted ids
  directly, which is the one thing that normalization exists to prevent.

  Tests: 334 in the core, 40 in the host (the measurement, the furniture that is
  not an element, the fit, and the three shapes the press can take), 54 in the
  Python client and 33 in the web one.

- ⬜ **N7 — What opening a foreign score preserves**. The reader takes every
  layout decision it finds as a decision **somebody made**, and most of them
  were made by an engraver. A two-bar ABC phrase that says nothing whatever
  about its page comes into the model carrying a **page break at measure 1**,
  because verovio's importer wrote a `<pb>` and the reader stored it; every
  `<beam>` in a document arrives the same way, as a beam a writer asked for.
  Nothing looks wrong today — the engraving is identical, since what is stored
  is what the engraver would have done anyway — and it stays invisible until
  the music changes underneath it: a rhythm edited afterwards keeps beams that
  no longer suit it, and a bar inserted before the break keeps a page turn
  nobody chose.

  This is the other half of a line `N6` did draw and drew well. It already
  refuses to read the emitter's *own* invented rests, on the test that they
  carry no id — "an element with no id was invented by the emitter" — and it
  already declines to store the double bar that ends any piece, because that is
  what the page says with or without it. What it does not do is ask the same
  question of the decisions that *do* have somewhere to be stored.

  **The decision, and it is one:** what should opening somebody else's score
  preserve? The plainest answer is the one the padding rule already uses —
  read layout only from a document **this layer wrote** (it has our ids) and
  let the engraver work the rest out again — and it has a cost worth naming: a
  MusicXML file exported from a notation program *does* carry breaks and beams
  its author chose, and this would drop them. The alternative is a per-kind
  rule, or a flag on the read. None of the three is obviously right, which is
  why this is a milestone rather than the fix it looked like.
  **Acceptance:** a document this layer wrote round-trips its beams, breaks and
  barlines unchanged; one it did not is read for its **music**, with whatever
  the rule decides about its layout stated where a reader meets it — the book's
  composition chapter and `sheet_from_mei`'s own docstring — and a test for
  each direction. *(Found 2026-08-29 reading an ABC phrase into the model, as
  the beam half; the break half fell out of the `N5` editor example on
  2026-08-30, which is what made it a milestone rather than an entry.)*
  *(Re-read 2026-10-06, with the score editor built (`X5`,
  `crates/clausters-apps/PLAN.md`): still so, and no longer invisible. The
  same ABC phrase still reads with a page break at its first measure and
  eight beams the engraver chose. The score editor opens a foreign file,
  writes and removes breaks (the Measures palette), lays a score out in runs
  at its page breaks, and changes rhythms by hand -- so a stored page break
  is now laid out as a page turn whoever chose it, and the beams that stop suiting an edited
  rhythm are what a hand meets first. The decision is the same.)*

- ✅ **N8 — Which element admits which edit**. *(Closed 2026-10-05 as
  `X5.0`, the score editor's first step, in `crates/clausters-apps/PLAN.md`:
  the display list's `kinds` and the core's `notation::admits`.)* A
  page is editable or it is not:
  `editable` gates the whole of it, and `entry` the whole of it again. That is
  the coarse cut and it was the right one to ship — the host cannot fulfil an
  edit the driver will not apply, so the flag is the driver saying it will —
  but it leaves a page where a barline drags like a notehead and a rest offers
  a transposition that means nothing.

  The finer cut is a **per-element property**, and what makes it a milestone
  rather than a flag is that the host has no idea what an element *is*: to a
  renderer an id is an id, which is exactly the finding `N6` answered once
  already with the display list's `elements` list. So this is that answer
  generalized — the engraving walk knows a notehead from a barline from a rest,
  and what it knows has to reach the host as something narrower than "this id
  is a sounding element". What shape that takes is the decision: a class per
  element, a set of admitted gestures per element, or a rule the host applies
  to a class it is told.
  **Acceptance:** a drag on something that cannot be transposed does nothing
  and says nothing (rather than reporting an edit the model refuses); note
  entry declines where a note cannot go; and whatever the page allows is
  readable from the display list rather than guessed from a shape.

- ✅ **N9 — A score as a box of the multitrack.** *(Rewritten 2026-10-04 with
  the user. It was written as "the score in the arrangement", and the
  arrangement it meant was `clausters.form`, frozen since; what it asks now is
  a box on the multitrack drawn as a score, the way a box of notes is drawn as
  a piano roll -- the roll and the score show the same notes.)*

  **What is there.** A box's base view is what its contents are: a box over
  samples draws a trace or a spectrogram, a box of notes draws a roll fitted to
  the box -- the `pianoroll`'s own element, handed the box's axis through the
  body door -- and a double click on a box of notes opens the roll over the
  same sequence. A score is the third presentation: the `score` element drawn
  into the box the way the roll is, and a double click opening the score
  editor (`X5`, `crates/clausters-apps/PLAN.md`) over what the box holds.

  **Open, none of it decided:**

  - **What the box holds** when it is drawn as a score: the notes a roll draws,
    engraved -- one sequence, two presentations, chosen like the multitrack's
    `view` -- or a sheet of its own.
  - **The time axis.** A box sits on a linear axis and an engraving spaces by
    duration, not proportionally: proportional notation inside the box, or the
    box's time mapped onto the engraving's measures.
  - **Size.** What a staff draws when the row is a few pixels tall, the same
    question the roll answered by fitting its pitch range.
  - **Editing.** A box's contents draw read-only on the multitrack, as a roll
    body does, and are edited in the editor the double click opens.

  **Acceptance:** a box of notes on a multitrack can be drawn as a score,
  scrolls and zooms with the time axis, and opens the score editor; shown in
  an example and in both clients' composition chapter.

  *(Re-read 2026-10-06: the score editor its double click opens is built
  (`X5`), so what this waits on is elsewhere. If the box holds the notes a
  roll draws, engraving them is reading a sequence into a `Sheet` --
  `crates/clausters-apps/PLAN.md`, Future directions, "From the roll to the
  score", which the user left for later; only a sheet of the box's own skips
  it. And it is a box of the multitrack, whose development is postponed
  (`X9`).)*

  **Decided 2026-10-06, with the user**, and taken after "From the roll to the
  score", which it reads its notes with:

  - **The box holds the sequence**, and the score is its reading: one
    sequence, two presentations. Which one is the widget's, as `view` is for
    a box of samples.
  - **The time axis is the box's: proportional notation.** Each note stands
    at its time on the ruler, in line with the other tracks; beams and slurs
    stretch between them. **What has no duration takes no room on the
    axis**: the meter and its changes are written above the staff, and the
    first clef and key signature use the room before the first note. It is
    the editor's continuous view under those rules. The other answer --
    bars in line with the ruler and the engraver's spacing inside them -- was
    weighed and left: the notes would not stand under the cursor.
  - **Size.** The staff is fitted to the row's height, and under a floor it
    can be read at the box is drawn as a roll.
  - **Editing.** Read-only on the multitrack; a double click opens the score
    editor over the sequence.
  - The engraver is the port the score editor is handed; with none, the box
    is drawn as a roll.
  - **How a note stands at its time without being stretched** *(settled
    reading the host, 2026-10-06)*: around each note is a stretch of the
    engraving drawn as it is -- the head, its stem, its accidental, its dot,
    its ledger lines -- moved whole to the note's time; between two of them
    what joins notes (a beam, a slur, a hairpin, the staff's lines) is
    stretched. A glyph is never stretched: it is placed where its own place
    falls.
  - **A box of notes is a window onto a time line that begins** *(the user,
    2026-10-06)*: a roll and a page have a start, so the box's front edge
    stops at the sequence's start and is never pulled into the past, while
    its back edge runs on into the future with nothing in it. The host bounds
    a box's front edge by its contents' beginning (`Contents::begins`), known
    length or not, and leaves the back edge free where no length is known.
  - **A box's page has no end, as a roll has none** *(the user,
    2026-10-07)*: the staff starts where the sequence does and runs the
    whole box. **No bar line is drawn, and no rest after the last note**
    -- the rests between notes are -- a start the user means to revisit.
    How a score editor draws the end of what is written, or keeps to the
    continuous system, is for later.
  - **A ledger line is its notehead's** *(the user, 2026-10-07)*: drawn as
    long as engraved, centred where its head stands, and never stretched or
    squeezed with the space between notes nor drawn over a glyph that is no
    note.
  - **A page is inked as a take is in a box** *(the user, 2026-10-06)*:
    white fought the selection's color, and only part of the page took it.
  - **The first clef and key signature stay in view**: before the box's
    first note while its start is on screen, and held at the left edge of
    what is visible once the start has scrolled off -- as a roll's keyboard
    is -- so the staff can always be read *(the user, 2026-10-06)*.

  **Built 2026-10-06, closed 2026-10-28 with the eye pass** over
  `editors/edit_multitrack` (View, Notes as scores): the user switched the
  keys box to its page and back, zoomed it, grew its row and saw the staff
  grow with it, and opened the score editor from it.
  - **The host** draws it: `notes_view` and `scores` on the `multitrack`
    widget, and `graphics::score::proportional` -- a `Warp` of the page's
    `x` with a zone around each note drawn as engraved, a glyph never
    stretched, a meter at half its size over its staff, the first clef and
    key before the first note and held at the row's left edge.
  - **The application** states the pages: the multitrack editor reads each
    box's sequence, engraves it with a score its holder hands it
    (`bind_engraver`, since the engraver is a port a client has and the crate
    does not), in the continuous view, and answers each box's page with the
    time of every note of it in the box's own frames. A page is engraved
    again when its sequence is no longer the one it was engraved from, and
    not once a correction. The View menu's *Notes as rolls* and *Notes as
    scores* are the switch, `notes_view` the option and the attribute in both
    clients, and a double click on a box drawn as its page opens the score
    editor over the sequence.
  - **Found on the way**: a note's `dur` is the time to the next of a line
    and not what it holds, and a client's event states one by default -- so
    what decides that a note goes to another voice is what **sounds**, and
    four notes written a beat long, half a beat apart and let go early are
    one line of eighths, where they were two voices and their rests.
  - **Left open**, each for when it is met: "What a score box of the
    multitrack left open" (Found by use).

- ✅ **N10 — A page's notes are the model's, and a written page break is
  drawn in one document.** *(Added 2026-11-09. It began as engraving
  only what changed, for the score editor; measuring and reading upstream
  turned it into these two parts, under "Verovio is followed, not adapted" in
  `docs/decisions.md`: what is verovio's to do is left to verovio, and what is
  ours is made right.)*

  **Part one: the notes from the model.** Every page a score produces
  (`Score::pages`, `Score::display_list`) carries the notes that sound, each
  with its onset and length in milliseconds, its pitch and the id it is drawn
  under. Today they are asked of the engraver: its timemap, then
  `getMIDIValuesForElement` once per element, and each call searches the
  whole document, so the cost grows with the square of the score (41 ms at a
  thousand notes, 1.25 s at five thousand). The cursors are already read from
  the model's performance, so the page and its sound come from two readings
  that can disagree (a staccato's length, a repeat played out, a tempo mark).
  Where the document was written from the model, the notes are read from that
  same performance: its beats taken to milliseconds by the tempo map the
  cursors use, the length the one held, and the item named as the page names
  it (`n7`, a chord's pitch `n7-p2`). The performance is computed once for
  both. A document from elsewhere, which has no model, is still read from the
  engraver. Acceptance: on the fixtures the model's notes are the engraver's
  in pitch, onset and id, and where they differ on purpose (a staccato, a
  repeat) the model's stand and a test says so; the cost measured before and
  after at a thousand and five thousand notes; both clients, since the reading
  is in the core.
  *Done 2026-11-09* (`Score::timing_locked`; the model's `Note.written` names
  a chord's pitch). The test that compares the two readings
  (`a_page_s_notes_are_the_model_s_and_agree_with_the_engraver_s`) found
  three places where the engraver's notes were not what the page writes: it
  attacks a tie's continuation again, plays an octave line as written, and
  holds a staccato for its whole value. `Score::pages` after an edit went
  from 119 ms to 46 ms at a thousand notes and from 1.78 s to 0.37 s at five
  thousand.

  **Part two: the page breaks.** A written page break laid the score out in
  runs, each a document of its own, and across two documents the engraver
  has no other end to work from: a tie across the break was dropped on both
  pages, a glissando was not drawn, a slur or a hairpin was drawn **closed**
  at the run's barline, an octave line ended in a hook and restarted as `8`
  without parentheses, a bracket got hooks on both sides of the turn. Inside
  one document the engraver draws every one of them by the convention: a
  slur or a phrase mark as two curves, each whole and open at the edge; a
  tie as two halves; a hairpin left open and coming back partly open; an
  octave line without its hook, restated as `(8)`; a bracket without hooks;
  a glissando at half its slope.

  *Done 2026-11-09, the second way.* A selection of the one document was
  built first and taken out: it draws the lines right, but a selected page
  has no running elements (its score definition is a copy without
  `pgHead`/`pgFoot`), so the title and the page numbers were gone. What
  shipped lays the score out twice, every decision the engraver's: each run
  as today, only to read where the engraver breaks it into systems and pages
  (`svg::system_starts`), then the one document with all those breaks
  written (`sheet_to_mei_paged`, numbered by `pagetext::numbered_xml`) in
  `encoded` mode, which is the drawing (`Score::paged_locked`). The layout
  options are put back after it (`Score::relayout` now keeps them). On the
  way, the walk learned the engraver's name for the later parts of a line
  across a turn (`class="... id-<id> spanning"`), so every part of a line,
  across a system or a page, is pressed as the line. The cost, as recorded:
  a score with page breaks is laid out once per run and once whole, and the
  last system before a page break is justified to the full width. The
  decision record is rewritten ("A page break is honoured by laying the
  score out twice"). Acceptance met by
  `a_line_across_a_page_break_is_drawn_on_both_pages` (slur, phrase mark,
  both hairpins, octave line, bracket, glissando, pedal and tie, each on
  both pages under its one id, and nothing said by the engraver) and by
  `a_written_page_break_turns_the_page` (the title on the first page alone,
  the page numbers on the others).

  **Not taken: engraving only what changed.** Measured at five thousand notes
  (release, one staff, a slur every four bars), `Score::pages` costs 1.78 s
  after an edit: 1.25 s reading the notes back from the engraver one element
  at a time, 0.28 s its timemap and the model's performance, 0.23 s loading,
  about 0.18 s drawing and walking every page. A system laid out by parts is
  primitive for primitive the whole layout's at about 8 ms, but it rests on
  behaviour verovio does not promise, and verovio's own editor already lays
  out again only the pages an edit reaches. So the project follows verovio
  rather than engraving by parts of its own (`docs/decisions.md`, "Verovio is
  followed, not adapted").

**What became of the earlier numbering.** `G31g` was one line; the first sizing
made it `N1`–`N4` (surface, markup, polyphony, tuplets). The four are all still
here and none of them is a milestone any more: the **surface** is N5's, being a
question about carriers rather than the decision everything waited on; the
**markup**, **polyphony** and **tuplets** are all N3, as emission over a model
that can represent them, with tuplets additionally answered in N1 by rationals —
which is why "raise the tick base" is no longer a decision anyone has to take.

**The examples are behind the model, and the milestone that lands the feature
lands the update.** `notation/compose.py` and its page build a piece by
operating on a score, and their last cell **stacks the motif against itself and
is refused**, because the operators say counterpoint long before the emitter can
write it. That refusal is deliberate — it is how the example says which
milestone owes the work rather than quietly dropping a voice — and it is also a
standing note: **N3 turns that cell into a real two-voice coda**, in both
copies, and the same pass re-reads the rest of the file for anything else the
emitter has since learned to say. The same holds each time this track lands
something a score can now be: the example that shows it is part of the
milestone, not a follow-up.

**Closing any of these** includes the docstrings that currently promise the work
as future (`clients/python/clausters/gui/notation/mei.py` says "tuplets are the
engraving-refinements milestone" and "mixed-duration polyphony is the
engraving-refinements milestone"), the notation pages of the client's book, and
the score examples in their `notation/` family — this feature's manual test
surface — which grow the case the milestone adds, in both packages.

## Definition of done (per milestone)

Following the project rule: code + tests, a clear commit message (the record of *what* shipped) and this file's checkbox updated; developer/user docs where the feature touches them; a commented example when the feature is user-facing — the example is also how new human-visible behavior gets checked by eye; and a `docs/decisions.md` note only when a choice has non-obvious context.

## Future directions (to fold into milestones as they firm up)

Captured here so the depth the editor-grade vision needs is not lost; each becomes a milestone — or a track — when its design converges. **The convention, stated because the list only works if it holds:** every entry carries a `⬜`, and one that converges into numbered milestones **leaves this list** — it is not marked `✅` here, because converging is not shipping and a `✅` beside an unstarted milestone says the opposite of what is true. The milestone that absorbed it is the record, and it says where it came from, so nothing needs a pointer that would go stale. Gone that way so far: scopes, the editor-grade views, edit-back-to-data and the BPF view (G18-G21), and — 2026-08-13 — the **DAW / timeline view**, which became the whole of G22 with its dedicated note view following as G24 — while the free arrangement plane it named alongside the lane stack **stays here**, since it is a second kind of multitrack whose model differs structurally from the one that shipped and has still to be defined, not an increment on it; and **a sample as a grabbable point**, folded into the D track (D1, D2), which is where the questions it raised are answered.

**Ordering — what is left of it.** The original decision staged the widget-deepening arc first, the timeline and notation views after it, and packaging plus the in-browser audio engine deliberately last. Three quarters of that is spent: the deepening arc landed (G18-G21), the timeline converged as G22 and notation as G31, and the engine shipped as the server's B track — which also settled the `Transport`/`ServerLink` seam the ordering was keeping open, since `ServerLink::Page` is that variant. What still binds is the other half of the same sentence: **packaging stays last**, because it changes how the system ships and not what the system can show, and it keeps constraining design in the meantime — the web frontend must stay Tauri-wrappable.

An entry that was done without becoming a milestone is ticked and moves whole
to [`docs/history/plans/gui.md`](../../docs/history/plans/gui.md) ("Future directions: the entries that were done"), so this list
holds what is open.

- ⬜ **A paste that needs tracks could make them** *(named 2026-09-12, with the fix above; **the refusal is the decided behaviour**, confirmed by the user the same day -- "if there are no more tracks it is a refuse" -- so this entry is an alternative somebody may want and not a defect waiting)*. A block taller than what is under the selected track is refused today, which is Pro Tools' and Cubase's answer and the honest one for a widget that owns no piece. **REAPER's** is the other: it creates the tracks the block needs and pastes whole. This widget already has the verb — a double click on a header adds a track — so what stands between it and this is not the capability but the **report**: adding tracks is a `lanes` payload and pasting is a `clips` one, and a paste that did both would be the first gesture here to leave two. That is a decision about the conversation and not about the multitrack: whether an owner may be handed a run of payloads from one gesture, in order, as one entry in the history. Worth taking with the audio editor's own paste, which will ask the same question about panes.

- ⬜ **The free arrangement plane (the blueprint view)** — *a second kind of multitrack, not a milestone of the one that shipped: it shares characteristics with the lane stack and its model differs from it structurally, so it is named here and still to be defined and planned.* A second projection of the same arrangement: the L3 workspace with its x `View` joined to the window's navigation group, so the plane's x axis *is* the composition's time axis (ruler, selection, the playhead sweeping the whole plane as a vertical line — all existing group machinery), while y is **free spatial organization** — clips and visual elements placed like an architectural plan, not locked to lanes. The model (`clausters.form`) is untouched: `offset`/`dur` stay the temporal truth, and y is presentation geometry owned by the driver and persisted by the `Editor` beside the tree, exactly the P1 `"move"` pattern. A concrete `Aggregate` draws as a bounding **zone** (visual containment as the aggregate's projection) instead of a lane; the classic lane-stack multitrack becomes the *constrained* case of the free plane, the same general-to-constrained pattern L3 fixes for scrolling — which is the direction of the relation and the reason this cannot be an increment on G22: the general case is the one that is missing. One boundary to hold when this opens: **y means nothing to the render** — not pitch, not bus, not bounce order; presentation only, stated so the view never implies semantics the engine does not have (any later mapped-y is an explicit decision, not drift). The honest precedent is the graphic score — **a UPIC whose material is the multitrack's editing structures**: not freehand curves but clips, groups, snap, selection and the edit-back payloads, placed on a free plane (IanniX is the other kin), and the whole of what "free" means there is what a design has to settle. **What it was waiting on has landed** — G22's clip bodies, L2's placement props and L3's workspace — so what stands between it and a track of its own is that definition, not a dependency.

- ⬜ **A steady goniometer, and what a phase view should read** *(the design half left standing by the ✅ "The goniometer's figure jumped, and the signal was the staircase" fix below — smoothing the source removed the staircase and left the view's own question exposed)*. With the signal smoothed the figure sweeps, and what is left is the view's own: it draws the **newest** window of two taps with no alignment, so consecutive ticks read overlapping stretches and the trace's ends need not meet. Three shapes, and the one that is chosen decides what a phase view *is*. **Align it** — reuse the oscilloscope's trigger on the left channel, which costs nothing new and stands a periodic pair still, at the price of no longer showing where in the signal it is. **Persist it** — the classic hardware look: several windows drawn with a decay, which the mesh's per-vertex alpha already allows and which turns instability into the glow it is on a scope. Or **take a positioned window** (`read_bus_at` reports where a window ends, which is what the retained history already appends by), so consecutive ticks read *consecutive* samples and the phase advances continuously instead of overlapping. The third is the only one that changes what the view reads rather than how it draws, and it is the one that would also survive a slower frame rate.

  **Where it shows is the evidence**: with the signal smoothed, what remains is visible at the *ends* of a sweep, where the figure degenerates into a straight line (mono is one diagonal, anti-phase the other). A line is exactly where a phase error is most legible — any of it opens the line into a thin ellipse — so the residue is the unaligned window and not the drawing's resolution, which is what points at the positioned read rather than at drawing more of it.

- ⬜ **Packaging: an optional Tauri desktop wrapper** reusing the web frontend. The rest of what this entry once listed has shipped — the GUI chapter of the client's book (`gui.md` and its pages) and the worked `gui_*` examples — so the wrapper alone is what remains, and it stays **last** by the ordering rule above: it changes how the system ships, not what it can show. **Only a possibility** *(the user, 2026-10-04)*: `clausters-editor` is already a Tauri application, so a wrapper could come from there later if it is wanted; nothing is planned.

- ⬜ **A selected staff is edited by its line count, not by its position**
  *(the user, 2026-09-07, during the visual review's second sitting)*. *(Partly
  built 2026-10-05, the score editor's ○ entries: the model holds a staff's
  line count (`Staff::lines`, `SetStaff`) and the Staves palette sets one or
  five, and the editor offers no drag of a staff. **Still open**: what the
  count means for pitch -- a percussion clef, and the mapping from a staff
  position to what an unpitched line plays -- so a one-line staff is still
  read through the clef it had.)* Selecting
  a staff is legitimate and a score editor should offer it. What it must **not**
  offer is dragging one up or down: no notation program moves a staff that way,
  because where a staff sits on the page is the engraver's and a system's
  spacing is not a statement anybody made -- it is exactly the class of thing
  the N track already refuses to store.

  What a staff genuinely admits is **how many lines it has**. A five-line staff
  is the default and not the rule: percussion is written on one line, on two, on
  five without pitch meaning, and a change of line count changes what a staff
  position *means* rather than where anything is drawn. So the verb is a
  property of the staff in the model, the clef and the mapping from staff
  position to pitch follow from it, and a percussion staff stops being a thing
  the reader has to fake with a treble clef.

  **Related:** `N8` asks which element admits which edit, and this is its answer
  for one of them; the hit-test entry in "Found by use" is what made the
  question concrete, since it is a press on a staff that raised it.

- ⬜ **The page's clipboard reaches the browser's** *(left by `G42`, 2026-10-04)*. A page keeps the host's own clipboard, so text copied in a canvas does not paste in another tab or program. Reaching it takes code on the page's side: a copy writes `navigator.clipboard` in the gesture that made it, Ctrl+V takes the text the browser's `paste` event carries before it pastes, and the menu's Paste reads it asynchronously (a browser may ask the reader's permission). A page has no primary selection at all, so the middle button stays the host's own there.

- ⬜ **A frame of what moves presents only what moved** *(named
  2026-10-27, measuring the audio editor playing in a window of 3840 by
  2071 pixels)*. With the picture kept on the card, a frame of what moves is
  a copy of the window's pixels and the boxes of its live layers: the copy
  alone measured 13 % of an integrated GPU at thirty frames a second in
  that window, and the
  compositor's own copy beside it. Both are the cost of presenting a whole
  image, since a swapchain keeps no picture to draw over. Presenting with
  the rectangles that changed (incremental present, the compositor's damage
  regions) would cut both to the boxes. Take it when the graphics layer
  offers it on every backend the host draws with, the page included.

- ⬜ **A window opens at the size it was last left at** *(asked 2026-10-29 by
  the user, looking at the multitrack opening at its minimum)*. A window
  opens at the size its def declares, and a redefinition keeps the size the
  person gave it, but nothing survives the window: closed and opened again,
  an editor is back at its declared size. Where that memory lives is the open
  question. It is the host's by what it is about -- the geometry of a window
  on a screen, which no application sees -- and keyed by what the window is,
  which only the application can name: an application's kind (the audio
  editor, the multitrack, a roll), a def's `name`, a client's own key. A
  `window` prop naming that key, read and written by the host in its own
  settings beside the key table, is the shape to weigh first.

- ⬜ **An element says which pointer and which tip the point under it
  means** *(2026-11-10, from `crates/clausters-apps/PLAN.md`, `X37` and
  `X40`: the roll's tools and what is under its pointer)*. The host cannot
  set the pointer's shape: a tool that draws, erases or stretches is told
  apart only by what it does, and the multitrack's grips stand where that
  capability was missing. And a `tip` (`G38.2`) is a widget's, one text for
  all of it, where a roll would name the note under the pointer. Both are
  one answer of an element about a point, read where the hover is already
  resolved: a pointer's kind (a standard set the native and the browser
  fronts both name) and an optional text the tip layer shows with the
  timer it already has. Each front sets the pointer from it.

## Found by use: the running list of fixes

These are not milestones and they are not future directions. They are what
**using the thing** turns up — an eye pass over an example, a path read while
doing something else, a behavior that is correct and unclear — recorded the day
it is found so it is not rediscovered, and kept here rather than inside whichever
milestone happened to be open at the time.

Every entry is a **checkbox**, so what is open reads as open at a glance and
nothing has to be inferred from where it sits. A fixed one is ticked, with the
record of what was wrong and why the fix is the shape it is, and moves whole to
[`docs/history/plans/gui.md`](../../docs/history/plans/gui.md) ("Found by use: the entries that were fixed"): this list holds what is
open, and that file is where a search for a fixed one ends.

Anything unresolved lives here or under "Future directions", both **after** the
tracks: never inside the milestone that happened to be open, and never among
finished work, where a pending item reads as done.

- ⬜ **After Save as and undo back to the original, the page's spectrogram
  keeps an edited picture** (2026-10-21, the user, browser). Playback was
  right and switching the view away and back redrew it; until then the
  spectrogram drew an earlier edited state of the take. A stale spectral
  cache that a restored join does not invalidate is the place to look, once
  the profiler can show what was redrawn and when. *(2026-10-21: the
  sequence -- spectrogram, apply an envelope, save as, undo -- did not
  reproduce in the headless page, whose software renderer draws no
  spectrogram texture at all; it needs a browser with a GPU. Later the same
  day, headless Chrome on Vulkan draws it, and five sequences -- an
  envelope, a cut, a copy and paste, several edits with several undos, each
  with a save as -- left the picture after the undo equal to the original's
  and to a redrawn one. Not reproduced; with the summary now carried across a
  redefine, a view switch no longer redraws from a fresh download, so a stale
  picture, if it happens again, stays until the owner's `reload` and is
  worth a capture of the exact steps.)*
