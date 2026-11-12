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

- **OSC is the single encoding everywhere.** A whole window/widget tree rides **as JSON inside an OSC argument**, exactly as a `SynthDef`/`GraphDef` does today through `/def_send synth` (`serde_json::from_slice` over an `OscType::String` or `OscType::Blob`; the server already accepts both - `src/osc/translate`). "JSON vs OSC" is a false dichotomy: JSON is the payload, OSC is the framing. serde's number handling distinguishes integers from floats, so ids stay `i32` and control values stay `f32` across the wire, the same rule the audio server relies on.
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
moved there whole; the plan keeps what is open. They are, by their headings:

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

## P track — the patcher: from view to Max-style editing (graphs and defs)

Section added 2026-07-19; **reconceived 2026-07-22.** G23 gave the logical group a *view*; this track grows it into an **editing and creation surface** in the spirit of a Max/MSP patcher. Said crisply, the two abstraction levels are one idea at two scales: **level 1 is visual programming of a `GraphDef`; level 2 is visual programming of a `SynthDef`/`FaustDef`.** Both are **one directed, typed cord patcher**. The earlier *bus-as-node*, undirected view (built, then set aside on the `patcher-bus-as-node` branch) proved unreadable as a signal-flow diagram — a patch's one job — and the cause was drawing the connection **without direction**. Direction is not a guess: it is **structural** — a def control that feeds an `In` is an **inlet**, one that feeds an `Out` is an **outlet**, one that feeds neither is a **value**, not a port. So the patch is directed and typed, and the rationale is recorded in `docs/decisions.md` ("The patcher is a directed, typed graph").

**Re-sequenced 2026-07-22 (representation before editing).** The track splits into two phases. **Phase A builds the read-only *representation* of a def as an autonomous view**; **phase B adds editing on top of the closed view.** The reason: a patcher's editing affordances are far easier to get right when they sit on a faithful drawing of a structure that already exists — the def — than when view and editing grow together. So level 1 (a `GraphDef` drawn as a patch) and level 2 (a `SynthDef`/`FaustDef` drawn as its internal UGen graph) are both delivered **as views first, decoded from the def's own structure**, before a box can be created or a cord rewired. The shipped P3a–P3d stand as the record of what was built; P3d's edit-back *code* (a driver rewriting the arrangement — shipped, but not a proven editing capability yet; see the current-state note below) is the **seed of the editing surface**, not part of the representation. **Build order.** Phase A — representation, the forward path: **P3** (level 1, the signal-flow patch — done through P3h) → **P4** (level 2, the read-only Def-view). Phase B is deliberately **consolidated into one milestone still to plan**, not sprinkled through the middle where it bred confusion: **P5 — the editing & authoring surface** (structural creation, rewiring, parameter/value/buffer boxes, the live loop, level-2 compile). Then **P6** — the patch as a persisted document. The rationale is recorded in `docs/decisions.md` ("Representation before editing: the patcher's Def-views are autonomous, decoded client-side").

- **A cord is a bus the user never numbers.** A box has **inlets** (top edge) and **outlets** (bottom), each **typed** — audio (`ar`) or control (`kr`). A cord runs outlet → inlet and *is* a bus of the inlet's type; **the inlet defines the bus** (Max's rule) and several cords into one inlet **sum** on it. The user draws cords, never bus indices: the **client names one bus per net of cords** and writes it into the def/GraphDef, and **the server does the rest it already does** — it allocates the bus numbers and **auto-orders** the members topologically, considering the buses. No client-side ordering, no new server logic.
- **Level 1 — visual programming of a `GraphDef`.** Boxes are **whole synthesis nodes** — defs of all three families, synths, groups — and buffers; a cord is a **server bus** between them. The graphic is deliberately **explicit and restricted**: a signal goes exactly where a cord connects it. The power to route arbitrarily lives in the *server's* node/bus architecture, not in this canvas — arbitrary, cyclic routing is something you build in code (nodes and groups inside a control cycle), never drawn (see the scope boundary).
- **Level 2 — visual programming of a `SynthDef`/`FaustDef`.** Boxes are **UGens**, with the def's controls and `In`/`Out`/hardware as edge boxes; a cord is an **internal wire** of the single def it compiles to. This is Max proper: patch objects, get a DSP program. It adds `ir` (a third cord type) over level 1's `ar`/`kr`.
- **One grammar, different objects.** Both levels are directed typed patchers over the *same* client cord→bus pass; they differ only in what a box **is** (a whole node vs a UGen) and what the patch **compiles to** (a `GraphDef` the server wires and orders vs a single def). Build the engine once and it serves both — the old "which level first" question dissolves into one path.

**Scope boundary — the graphic is a DAG.** Feedback (a node reading a bus a later node writes) is a **code** construction — nodes and groups inside a control cycle, a one-block delay across the loop — not something the acyclic patch drawing expresses. The patcher draws the directed *acyclic* routing; the server's own ordering resolves it. A genuine cycle stays code, not a cord.

**Ownership stance (the edit-back pattern, unmoved).** The host never owns patch semantics: it draws boxes and directed cords, runs the gestures, and emits flat `/gui_event` edits (the directed `"wire"`, plus `"move"`/`"make"`/`"free"`), exactly as the clips and the bpf do. The **driver** — Python first, per the reference-client rule — owns the patch model and its one small pass (cords → a bus named per net, summing fan-in), translates it (level 1 → a `GraphDef` spec with those bus names, which the server allocates and orders; level 2 → the existing SynthDef builder / Faust box API, never a second compiler), and re-renders. **The server and the Python client are barely touched — this is a GUI track:** the server already allocates buses and auto-orders a `GraphDef`, and the Python arrangement model (`Aggregate`/`Generator` → `GraphDef`) already exists; what is new lives in the GUI (the directed `patch` widget) and the shared cord→bus pass. The **patch document schema *and* that pass** are language-agnostic and land in `clausters-core`, so later clients rebind them rather than reimplement them.

**Current state: no editing is realized yet — a state of the development, not a premise.** As the track stands, no surface actually edits: the level-1 edit-back (P3d) shipped as *code* but is unproven in the window, and nothing else is built — so every surface the track has today is, in practice, a representation. This is **not** a claim that views cannot edit; editing is precisely what phase B builds (driver-side, over the edit-back seam the ownership stance describes — the widget emits gestures, the driver applies them). Representation-first simply matches where the track already is: make the views complete and faithful first (decoded from the defs), then add the editing on top.

**Autonomy stance (a Def-view is a peer of the heavy views; the editor only orchestrates).** The graphic of a def is an *autonomous visualization*, exactly like the waveform, spectrogram, oscilloscope, timeline, bpf and env views: a host widget pure over its `Mesh`/data, fed by a **headless decomposition of the def's structure** (`defs`/`clausters-core`, no window needed), and openable **on its own** — one window per def through the free-standing opener, the `clausters.plot` posture (G26). The **`Editor` does not own it.** The editor is the **gui-side representation of the arrangement model** (`clausters.form`): `form` is pure and transport-agnostic and never imports the GUI (its own docstring names `clausters.gui.editor` as its view), so the editor depends on `form`, never the reverse, and belongs in `gui`, **not** in `form` — the model must not know its view. Its job here is to **orchestrate**: when a composition holds a logical aggregate it embeds the autonomous Def-view as a lane, the same way it embeds the piano-roll — it does not carry the `Aggregate → patch` decomposition monolithically. That is the concrete correction the re-sequencing makes to P3d, which put that mapping inside `editor.py`; P3h lifts it into `defs` as `GraphDef → GraphPatch`, leaving the editor a thin consumer.

**Retrieval — client-side, from the def in memory (the server is not extended).** A box's directed typed ports come from the def itself: the client that **built** the def (or the box API) already knows which controls feed an `In` (inlets), which feed an `Out` (outlets), and which are values; for UGens the rates come from P2's `/ugen_query`. Level 2 goes one step deeper into the *same* structure: a `DefPatch` reads a `SynthDef`/`FaustDef`'s **internal UGen graph from the def object in memory** — the DFS `synthdef_ports` already walks (every UGen a box, every input a cord), not a new query. So the common case needs **no new server verb**. This covers the authoring case: a def you built or loaded through the client. Inspecting a def the server holds but no client in this process built would need the server to report each def's internal UGen graph over `/def_query`; that is **deliberately not done** (recorded 2026-07-22) — it would add per-def storage and processing to the server for something the arrangement model does not do for its other abstractions either, and it stays an explicit later decision, never a prerequisite. (`/def_query` already lists a def's control surface; it *could* later also report each control's **role** — input-bus / output-bus / value — but that too is an optional refinement.) No new node/def semantics are added anywhere in this track. "Tables" need no new object class: a table **is** a buffer (filled by `/buffer_gen`), so the patcher shows buffers, optionally with their generator recipe as a label.

**Scope boundary (what this is not).** Not a Max clone: no message scheduling, no `bang`s, no UI objects inside the patch (control surfaces are what the GuiDef already is — a patch and a panel compose side by side), and never a third def family: a def patched visually is byte-for-byte a def the code API could have built, auditioned and freed like any other.

- ✅ **P1 — the patch canvas: free placement, navigation, selection** *(done 2026-07-19; as anticipated, the canvas grew no navigation of its own — a `patch` inside a `scroll` pans/zooms through the container, the boxes/wires/text scaling by the placement's workspace zoom (the L4 `Placed::scale`), and the wire/box/marquee gestures ride the shared machine)*: the `patch` widget sheds its fixed two-column layout without losing it — members and buses gain optional `x`/`y` props (absent, today's stacked layout auto-places them, and stays the default so every existing GuiDef renders unchanged); boxes are **draggable** (a `Drag::Box` in the shared `gestures` machine, so the browser gets it by rewiring), each move emitted as `/gui_event <id> "move" <kind> <index> <x> <y>` and settable back via `/gui_set` (the round trip that lets the driver own geometry); the canvas gains **pan/zoom** (the L track's 2D workspace container (L3) — a `viewport::View` per axis, the anchor-preserving math reused — wires and text scale with it; if P1 opens before L3, the canvas grows the viewport pair directly and L3 later hoists it) and **click/box selection** with the selected set as view state (the `selected` posture). Pure over the `Mesh`, window-free tests for placement, hit under transform, and the move round trip. Python: `x`/`y` on the `patch` builder's members/buses; the `Editor` persists geometry beside the logical group so a patch reopens as it was left.

- ✅ **P2 — the introspection verbs** *(done 2026-07-19; numbered on the server track as **M30**)*: the retrieval surface the palettes need, three queries in the `/server_query` mold. **`/def_query`** → `/def_query.reply` replies listing the loaded defs: name, family (`synth`/`faust`/`graph`), and per def its control surface (names, defaults, rates; a GraphDef reports its **ports**, scaling included — the named surface is what a patcher wires); with a name argument it details one def. **`/buffer_query` with no argument** (or `/b_list`) → the allocated buffer numbers, each with the `/buffer_query.reply` shape (frames, channels, sample rate) so the patcher can show real buffers as objects. **`/ugen_query`** → the UGen catalog straight from `src/dsp/registry.rs` descriptors (kind, inputs with defaults, static fields, rate rules) — the level-2 palette derives from the server's truth instead of a client-side copy that would drift (the schemas.md stance "a client never enumerates them" is deliberately relaxed into a typed verb; the catalog stays reference, the verb makes it discoverable). Faust primitives stay client-side knowledge (the box vocabulary is Faust's own, already in the client builders). Python: `server.defs()`, `server.buffers()`, `server.ugens()` (async, `/server_sync`-barrier friendly); docs in `schemas.md` beside the other queries. Shipped as M30 (the G25/M25 pairing pattern); the Python methods landed as `query_defs()`/`query_buffers()`/`query_ugens()`, joining the existing `query_*` family — a bare `buffers()` would have shadowed the server's `BufferAllocator` attribute. The UGen descriptors had no input names to report, so the registry grew them and the client tables became mirrors held in line by a contrast test (see `docs/decisions.md`).

- **P3 — the directed patch at level 1: typed inlets/outlets, cords, object creation** *(P3a–P3d and P3h shipped 2026-07-22; P3 is the level-1 signal-flow patch and its **read-only representation is complete**. The editing that used to sit here as sub-items P3e–P3g — structural creation, parameter/value boxes, buffers as value boxes — is **consolidated into the editing surface, P5**, so it no longer interleaves with the representation)*: the `patch` widget is **directed and typed** — a box draws inlet cells (top) / def name / outlet cells (bottom), each port typed (`ar` heavy, `kr` thin) and **derived from the def**; a **cord** is a drag outlet → inlet (a rate mismatch refused at the gesture, fan-in **sums**) and *is* a bus the user never numbers — the **client cord→bus pass** names one per net and the server allocates and orders them. The vision and its rationale are in the P-track intro above and `docs/decisions.md`; the milestone is broken into sub-items, each with its status:

  - ✅ **P3a — the cord→bus pass (`clausters_core::patch`) + FFI** *(2026-07-22)*: connected-components over the cord graph, one private bus per net (`b0`/`b1`/…), writers summing, no drawn hardware net (hardware is a terminal def). FFI `clausters_core_patch_compile` (ABI 11, JSON in/out, a bad cord on an error channel). Window-free tests.
  - ✅ **P3b — the directed `patch` widget + gestures + edit-back** *(2026-07-22)*: `host::patch` draws a box as three color-coded bands — inlet cells (top strip), the def name (middle), outlet cells (bottom strip) — cords typed by weight; `cord_between` normalizes and rate-checks a pair; the cord-drag / box-move / marquee gestures ride the shared machine; the four-place `"wire"` edit-back. The bus-as-node view is fully replaced.
  - ✅ **P3c — the programmatic model `clausters.defs.GraphPatch`** *(2026-07-22)*: build in code, `compile()` via the pass, `to_graphdef(name)` to send, `to_widget()` for the view; `synthdef_ports` derives a box's typed ports from its `SynthDef` graph (a control feeding an `In`/`InCtl` is an inlet, one feeding an `Out`/`OutCtl`/`ReplaceOut` an outlet) and `add(a_synthdef)` reads them off the def; `guidef.patch` emits the schema. *The GUI is a view of this*, usable headless. Example `examples/editors/patch1.py`.
  - ✅ **P3d — the directed Editor driver** *(2026-07-22; **the shipped seed of the editing surface, P5** — this is editing, and it shipped ahead of the representation work; its `Aggregate → patch` mapping lives in `editor.py` and is superseded by P3h's headless decode, which leaves the editor a thin consumer)*: `Editor._logical_patch` maps a `Aggregate{logical}`'s members + shared-bus controls to a `GraphPatch`, drawn as a directed `patch` in a `scroll`; the edit-back rewrites the arrangement — a `"wire"` points both members' controls at one bus (reusing one, or a fresh `w{n}` via `Aggregate.declare_bus`, so the next render's GraphDef wires the way the cord is drawn), a `"move"` persists a box's canvas position (kept by group identity across redraws). A member wrapping a bare def *name* draws port-less; an `"OUT"` control is not a cord (that member reaches hardware itself).
  - ✅ **P3h — the level-1 Def-view: headless decode + autonomy** *(done 2026-07-22; phase A. Window-free tests throughout; the on-screen render is a manual smoke step, not asserted here)*: the level-1 representation stopped being the editor's and became an **autonomous view of a `GraphDef`**, the peer of the heavy views (the autonomy stance above). The three parts: **(1) the inverse decode `GraphDef → GraphPatch`** in `defs` — `GraphPatch.from_graphdef(gdef, defs)`, the reverse of `to_graphdef`: it reads each member's bus-valued controls back into cords, typing each box's ports from the `SynthDef` it was built from (`defs` maps a member's def name to that `SynthDef`; direction is never guessed, so a def not in `defs` draws port-less and grows no cords), plus a `GraphDef.members()` read accessor it walks. **(2) the `plot_def()` method** — `GraphDef.plot_def(defs=None)` decodes the def and opens its **structure** as a `patch` view in its own window on the ambient GUI host (one window per call, the `clausters.plot` posture, G26; the shared opener `clausters.plot._open_patch_view` + a `PatchWindow` handle), **scoped to the def as a method, not a global verb**: kept distinct from `clausters.plot(def)`, which renders the def's *sound* (its rendered waveform) where `plot_def` shows its *structure*. Lazy imports keep `defs` from coupling to the GUI. (`SynthDef`/`FaustDef` `plot_def`, the level-2 structure view, is P4.) **(3) the `Editor` reduced to a consumer** — `_logical_patch` is now a thin adapter: it renders the `Aggregate{logical}` to a `GraphDef` (`Aggregate.to_graphdef`), collects the members' `SynthDef`s, and calls `from_graphdef`; the mapping itself lives in `defs`. (The `synthdef_ports` use that remains in `editor.py` is the edit-back path `_outlet_rate` — the P3d seed of P5, not the representation decode.) Read-only: no cord drawn, no box created — that is the editing surface, P5.
  **Naming (recorded):** the level-2 counterpart is **`DefPatch`** (UGens → a `SynthDef`/`FaustDef`), beside this level-1 `GraphPatch` (defs → a `GraphDef`). The representation half is `DefPatch.from_synthdef` (P4), the same inverse-decode shape as `GraphPatch.from_graphdef` (P3h).

- ✅ **P4 — level 2, the read-only Def-view: a `SynthDef`/`FaustDef` as its internal UGen graph** *(done 2026-07-22; phase A — representation. Window-free tests throughout; the on-screen render is a manual smoke step, `examples/editors/patch2.py`)*: the **same directed widget and cord grammar P3 built**, now **representing** a def — **UGens as the boxes**, the def's controls and `In`/`Out`/hardware as edge boxes (the parameter surface as sources, the output as a sink), as in a real def. This is the level-2 half of phase A: a **decode `SynthDef → DefPatch`** (`DefPatch.from_synthdef`, the counterpart to P3h's `GraphDef → GraphPatch`) that reads the def's **in-memory UGen graph** (the DFS `synthdef_ports` already walks, one step deeper — every UGen a box, every input a cord), a widget that draws it, and the same `plot_def()` method (`synthdef.plot_def()` / `faustdef.plot_def()` open the level-2 structure view, as `graphdef.plot_def()` opens level 1). Faust boxes carry a `signature` from the client's own box vocabulary; UGen boxes from P2's `/ugen_query`. What level 2 adds over level 1 is `ir` (a third cord weight, dashed alongside `ar` heavy / `kr` thin) and the fact that a **cord is an internal wire, never an allocated server bus**. **Read-only** — no box created, no cord drawn (that is the editing surface, P5). The **patch document** is the one P3 defined in `clausters-core`, extended here with the UGen/Faust signatures — the schema is the deliverable as much as the widget. Window-free tests: the decode of a known `SynthDef` into its UGen boxes/cords, typed hit-tests over UGen signatures, the `DefPatch → SynthDef → DefPatch` round trip where it holds.
    **As shipped (the concrete shape):** the level-2 model is `clausters.defs.DefPatch`, a Python peer of `GraphPatch` (the pattern from level 1: the *model* is client-side; only a shared *pass* would live in `clausters-core`, and level 2 has none — a read-only decode has nothing to compile). So `clausters-core` gained exactly the shared vocabulary the widget needs — `Rate::Init` (the third cord type, `ar`/`kr`/`ir`) — and nothing more; the `DefPatch` document (boxes carrying each UGen's `kind`/`rate`/`op`/`label`/`static`, control boxes their name/default/rate, constants recorded per inlet) stays in Python, and its wire form is the same generic `{boxes, cords}` widget schema P3 defined, now with `rate:"init"` ports. A UGen box's inlets are **named from the client's own builder signatures** (`ugen_input_names` in `clausters.defs.ugens`, the same callables the `/ugen_query` contrast test keeps honest — no new server verb), falling back to positional for the generic op UGens and the wire-order-misaligned kinds (`EnvGen`, `Poll`, …). An unset UGen output rate defaults to **audio** (the honest headless heuristic — the exact per-kind default is the server's, not the client's). The `ir` cord draws **dashed** (`host::patch::draw_cord`). `to_synthdef` reconstructs the def from the boxes (dependency-ordered, one forward pass) and the round trip **reproduces the spec exactly**. Faust: the **signal-tree** form decodes node for node (every op a box, every operand a cord, a slider a source box); a **box-tree or source** def is opaque and draws as a single box (its internals are the compiler's, not reconstructable client-side). Examples consolidated to one per level — `examples/editors/patch1.py` (level 1) and `examples/editors/patch2.py` (level 2).
    **Refined after the on-screen reviews (also 2026-07-22):** the **node-positioning moved to the host** (`host::patch::auto_layout`) — the decode ships no coordinates, so every client rebinding the widget gets the layout for free (`GraphDef.plot_def` benefits too). Because a def is a **DAG** (not a single-root tree), the layout is a small **layered (Sugiyama-style)** graph drawing — layer by longest path to a sink (inputs land above their use, not piled in one top row — the mistake a first attempt made by pinning inputs to the top; Reingold–Tilford was rejected as it needs a single-root tree), insert a **dummy node** on every rank a long edge skips (so the wire bends through the gap instead of cutting across the boxes between its rows), **order** each layer by alternating up/down **barycenter** sweeps (crossings cut at every level, not just near the sinks — the earlier single simultaneous relaxation only settled the bottom row), **place** on the x axis by iterated barycenter separated with an **isotonic (pool-adjacent-violators)** fit so overlapping boxes spread around their shared centre (children align under parents, no left-margin pile-up). The scroll workspace **sizes its content to the graph's natural extent, never below the viewport** (`scroll_content` reads `patch::natural_size`) and the layout **centres** the block in that area — a small graph centres in the window, a large one fills the content and pans (the earlier Python guess over-sized the content and pushed the graph off-screen). The panel **frame hugs its boxes and their bent wires** instead of clipping a fixed rect; **constants became `const` value boxes** (a number box corded into the inlet, distinct `value_fill`) rather than inline defaults; every cord is **one width**, the **rate read by colour** — **contrasting primaries, audio red / control blue / init yellow+dashed** (a fat-vs-thin pair and the earlier green/blue both read badly); the box's **central band is white with black text**, framed by **dark port strips**; the per-box **role** is now only a drawing tag; and the panel **caption is the def kind** (`synthdef`/`faustdef`/`graphdef`), not the def name. The level-1 example was expanded to a four-box effect chain with the full canvas (placement, marquee, navigation, render/hear). See `docs/decisions.md`.

- **P5 — the editing & authoring surface (to plan)** *(phase B — one milestone, deliberately not designed yet; it earns editing on top of the closed level-1 and level-2 representations)*: **all** the patcher's editing lives here, consolidated so it stops interleaving with the representation. The shipped **P3d** edit-back (a `"wire"`/`"move"` rewriting the arrangement, still wired into `editor.py`'s `apply()` but unproven in the window) is its **seed**; this milestone makes it a real, verified authoring surface across both levels.

  **What the E track changed under this milestone** *(recorded 2026-08-09, after E16 closed; P5 was written before E9 moved the wire and the bullets below still spoke the old vocabulary)*. Three facts the design must start from rather than rediscover:

  - **There is no `patch` type any more.** E9 moved the wire onto the model: the patcher is a **`plane`** — a two-axis container in content units — **that has boxes**, and their presence is what makes it a patcher rather than a second type name. So every edit-back below is a `/gui_event` from a `plane`, and the cords are still a prop of the plane (a wire is between two children and belongs to neither). The Python and TS builders keep the name `patch` as a *shortcut* onto that node, which is why no example changed and why this entry's old prose read as if nothing had.
  - **The gestures ride the shared machine, and the container decides first.** E10 gave every container a **gesture plan** — an ordered list of steps per modifier, declared on the wire (`gestures`) and defaulted from what the container is — where `GestureStep::Element` is the point at which the thing under the cursor gets its turn, and a step that declines passes the press **outward**. A patcher's canvas is `Coords::Canvas`, so a marquee sweeps canvas units and a press that no box claims falls through to the plane under it. P5's new gestures are therefore arms of `gestures/press.rs` and `interact/edit.rs`, not a private handler; the box-drag and cord-drag already there (`Drag::Box`, `Drag::Wire`) are the pattern the rest follow.
  - **The eye pass is part of closing it.** E12 established what the whole E track had skipped: nothing in CI runs an example, so `gui_patch1` and `gui_patch2` are the manual test surface and a milestone is not closed until they have been opened and watched. P4's "the on-screen render is a manual smoke step, not asserted here" is the honest note; P5 inherits it as an obligation rather than a caveat.

  **And the one milestone the E track deliberately handed here** — `box` **as an element**. The catalog spent the name on a synonym for `panel`, E9's model wants it for the patcher's box, and E9's expand stage could not take it while both spellings parsed, so a plane's boxes are **still its `boxes` prop**. Turning them into child elements is behavior, not spelling: ids (a box becomes addressable on its own), layout (the plane places them instead of drawing them), per-box hit-testing (the chain reaches a box the way it reaches any child) and edit-back (a `"move"` becomes a child's `place`, not an indexed prop of the parent). It belongs here because P5 is what needs it — structural creation, per-box error state and a control panel per box all want a box to *be* something addressable — and it should be sequenced **first** within the milestone, since every other bullet reads differently depending on whether a box has an id. Note the ordering against the **K track**: K1 moves every *leaf* element behind `trait Element` and keeps containers in the enum, so a `box` that lands before K1 is an enum arm and one that lands after is a registered element. Neither is wrong; deciding which is cheaper is part of opening P5.

  What P5 must cover, each a design question to converge before opening:
  - **Structural creation** (`"make"`/`"free"`) — the drop-down/object-box gestures to add and remove boxes (`/gui_event <id> "make" <kind> <name> <x> <y>`, `"free" <kind> <index>`, cords dropping first). Graphically heavy (in-canvas text entry + filterable lists over P2's `/def_query`/`/ugen_query`) and the create round-trip is not yet defined — until it lands the patcher is the **view of the def-generators** a composition already holds, not an authoring-from-empty surface.
  - **Parameter/value boxes** — a box representing a **parameter as a value** (a number for `freq`, an envelope, a buffer) feeding a def box's parameter inlet; today the patcher wires def-to-def signal buses only and a def's parameters-as-values are undrawn.
  - **Buffers as a value box** — a buffer is **data, not a def-box** (one kind of value box). The arrangement model keeps a `Vector` object (parity — the patch is a view of a part of the model), a cord carrying a buffer is *not* a signal bus; *visualizing* it (fetch its samples, draw like the audio editor's waveform) and *playing* it (a temporary synthdef) are facilities off the server buffer. See `docs/decisions.md`.
  - **The live patch** — the patch meets the running tree, closing the **drawn-vs-sounding gap**: rendered members light up (the driver correlates its instance with `/group_queryTree` + `/server_notify`, sets a per-member `live` flag via `/gui_set` — the host draws state, never queries), a box edited since its render reads **stale**, double-click opens a member's **control panel** (sliders over its controls from `/def_query.reply`, bindable via `/gui_bind` — Max's live number boxes with zero new widget code), and a `"render"`/`"stop"` affordance closes the create → wire → sound loop.
  - **Level-2 authoring → def** — the same directed edit-backs over UGen/Faust boxes, compiled to a def through the builders that already exist (a UGen patch through the SynthDef graph builder, a Faust patch through the box API — **never a third compiler**): a tree walk from the `Out` boxes; a patch the walk cannot serialize (a cycle without a delay, a dangling required inlet) fails client-side naming the offending box. The def is sent (`/def_send` + `/server_sync`) and auditioned live; a server `/fail` routes back as an `error` prop on the box. The translation lives beside the `clausters-core` document schema so it is portable. Examples when it lands: `examples/editors/patch1.py` (a level-1 effect chain from an empty canvas), `examples/editors/patch2.py` (a synth patched from UGen boxes).

- **P6 — the patch as a document: persistence and round trip** *(design to converge before opening)*: where a patch lives when the window closes. The original two options: the patch document persists **beside the def it produced** — a sidecar in the server's def store (the store already persists defs across sessions; a def compiled from a patch would reload *with* its patch, so any client can reopen the boxes behind a sound) — against a purely client-side document beside the arrangement. Needs a decision on identity (what happens when the def is re-sent from code and the patch goes stale) and lands with a `docs/decisions.md` entry either way; not opened until that converges.

  **A third option exists now, and it did not when this was written** *(recorded 2026-08-09)*. The GUI host grew its own persistence twice over while the P track was paused, and both are a better fit than a def-store sidecar for the thing a patch actually is:

  - **The host's named-GuiDef store** — `store::GuiStore` behind the `DefStore` seam, serving `/gui_load`: the host already saves a GuiDef by name and hands it back. A patcher window *is* a GuiDef, so the boxes and their canvas positions persist with no new format at all.
  - **The persisted bundle** — `host::bundle`, the boot the native standalone mode and the browser both replay: defs → graphdefs → boot preset → the GuiDef's `boot` messages, and, when the manifest declares a component contract, a **template** whose symbols are allocated and holes resolved at mount. That is precisely a patch's two halves travelling together — *the def it produced* and *the boxes behind it* — which is what the def-store sidecar was reaching for, in a container that already exists on all three legs.

  So the decision to converge is no longer "sidecar or client-side" but **which existing container owns it**, and the identity question sharpens rather than goes away: a bundle mounted twice is two instances of one patch, so a patch document has to say whether the boxes are the template's or the instance's. Two constraints to carry in. The bundle format is **version-bearing** — E9 moved it and that is what dragged the SemVer tier to 0.9.0 — so a patch inside one is a release decision under the `release-versioning` skill, not only a schema one. And a browser host **runs without a store** by design (`DefStore` is a seam the page simply does not fill), so anything built on `/gui_load` alone persists on one leg and not the other, which is a degradation of one build the K track's first constraint would otherwise forbid.

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

---

### Part A — the widget API

- ✅ **K1 — `trait Element`: the seam and the registry.** `host/widget/element.rs` carries the object-safe trait, `Needs`, `Claim` and a thread-local registry; `WidgetKind::Custom(Box<dyn Element>)` sits *beside* the enum and is wired through build, apply, size, the frame's flat draw, the query, the bus collector and the element step of the press; `clausters_gui::{register, unregister}` is the public door; `host/elements/` is where a ported leaf lives with its tests beside it, and `elements::builtin` is the table the schema consults **before** the registry.

  Three invariants, each now covered by a test. A built-in resolves first, so **a registration can never shadow one** or change what a shipped def means. A miss on both is `Unknown` — laid out, not painted — which is what an unrecognized type has always been, so **an element family compiled out of a build degrades exactly the way a host older than the def does**, with no new failure mode. And a container is not extensible here.

  `label` ported, and porting it corrected two things the paper design had wrong: `draw` takes the placement's `scale` (a widget's own `text_size` is a number no table resolves, so a label in a zoomed plane was measured at one size and drawn at another), and `is_bare_surface` is a trait method (whether the wheel falls through is a real per-element answer, `false` by default so a registered element never swallows the axis behind it by accident). **The port itself moved out to K7**, because the surface has to be finished before sixteen leaves are written against it.

- ✅ **K2 — A Rust API for building a tree.** `clausters_gui::tree` (`node`/`window`/`layout`/`plane`/`field`, chained `id`/`prop`/`child`/`children`) and `Host::define`/`define_with_blobs`, factored out of `on_def` so the wire and Rust share one definition path from the parsed node down: a tree built in Rust is recorded, rendered, bound and persisted by the same steps, and its JSON is *derived* rather than skipped, since that is what persistence, reload and `/gui_query` read. Parity is the suite — four builder-vs-parser document tests and two host-level ones.

  **The builder deliberately does not mirror the widget catalog** — a leaf is `node("knob").prop(…)`. The props already live in three surfaces `docs/gui-props.md` holds together, a fourth would drift, and a registered element's props no catalog in this crate can know; the reasoning is in `docs/decisions.md`.

- ✅ **K3 — The world: what a leaf reads that no widget owns.** `Element::draw` saw the mesh, the theme and the size table, which is everything a `label` needs and not enough for most of the catalog. The frame already assembled the answer — `FrameInputs` — and the milestone was to pass the *right part* of it and no more.

  The split is the whole design, and it came straight from the diagnosis. Of `FrameInputs`' thirteen fields, **five are one widget's own interaction state** (`active_button`, `focused_text`, `menu_popup`, `marquee`, `wiring`) and those are **not** in the world: they go home to the element in K4/K6 and the fields are deleted there. What was left is genuinely nobody's: the bus source, the queried node trees, `server_attached`, the sample rate, the sample clock, the pointer, and the timeline groups' shared window. That is `host/world.rs`'s `World`, read-only and identical for every element — and it carries the two bus-read rules (`control`, `level`) so no reader restates them.

  It reaches an element through a **`Ctx`**, not as a fourth argument, which was the one shape the paper design left open. `Ctx` carries the world *and the placement* (rect, scale, the resolved metrics), so K4's coordinate system lands as a field rather than as a signature change in every element ever written — and a method that gets no `Draw` (the slot's) still has the size table.

  **`Needs` became the whole declaration** a tree collector reads — control buses, the audio buses whose level is read, the audio buses whose samples must be tapped, the server groups whose node tree is drawn, whether the picture follows the clock, and the GPU slot a heavy view claims. Four walks that each matched on a kind now ask every widget the same question. **The bulk resources are the one part deliberately left out**: a bulk *declaration* is only half a seam — the other half is how the loaded data gets back into the element, which is shaped by `WaveformData`/pyramid types only `signal` uses — so it lands with `signal` in K7 rather than being invented against no consumer.

  The declaration also caught a bug the old walks had. An audio-rate `meter` reads a published level, which is neither a streamed bus nor a tap, so the liveness walk did not count it: a window holding only level meters never animated and the column sat still. It declares a level now, and the window follows the declaration.

  And the **GPU slot** got its shape, which settles what would otherwise be an open question about `canvas`. An element that cannot draw into the shared mesh — because it needs a texture, a vertex buffer or a shader of its own — *claims a `SlotKind`* in its `Needs` and is fed by `Element::slot(ctx)` each frame, and the frame maintains it keyed by widget id, exactly as it does for the built-in heavy views. The static claim is what a window allocates from; the per-frame call is where a param resolves from a bus, with the world in hand. **The set of slot kinds is closed and belongs to the frame** (which owns the device, the pipelines and the one-batch rule), so an element chooses among them and cannot invent one: a `canvas` takes the shader slot with its user WGSL — a *parameter* of that slot, not a new pipeline — so `canvas` is an ordinary element, not an exemption. The set ships with that one kind; `signal`'s geometry and texture kinds join it when `signal` ports, for the same reason bulk waits. Widening it is adding a pipeline to the frame, which the cost rule already prices at once per window and only in the builds that compiled it in; that is the same boundary as "a container is not extensible", drawn where the hardware actually is.

  **The three proving leaves ported with the seam**, one commit each after it: `meter` (the smallest thing that reads the world — one bus, and the rate picks the table), `nodes` (a queried document plus the `server_attached` placeholder) and `canvas` (the slot declaration carrying a shader). Each was an arm in build, apply, the frame's collect and the frame's draw, plus — for the first two — an item struct copied out of the tree for a draw that never needed the tree; each is now one file in `host/elements/` with its tests beside it, and `WidgetKind` is three variants shorter. The prop parsers they share with the schema widened to `pub(crate)`, which is what an element outside the `widget` module needs to read the same props the same way.

- ✅ **K4 — The element owns its interaction.** The largest single simplification in the track and the one that makes the API consistent: a widget's own geometry and press-time snapshot were parked in the gesture machine as `Drag` variants only because the widget could not hold them. What the machine keeps is the **sequence** — press → drag → release → wheel — and the containers' navigation plans, which move a coordinate system rather than a widget. What the element gains is the **meaning**: `press` returns a `Claim` that holds the press, `drag` is handed the cursor and mutates *itself*, `release` delivers. Beside the plans sits `Drag::Element`, which carries no geometry at all.

  **The `Claim` says one thing, not two.** The design named two flags — grab the pointer, tick the edge scroll — and only the first landed; the grab was later removed outright, since a knob measured from its press needs no capture (see "A knob is a locked pointer on the desktop and a travelling one in a page", Found by use). The edge scroll's only drag today is a `clip`'s, and a clip is a **container** that keeps its own drag; the flag lands with the first *element* that runs off an edge, which is also what will settle how the axis under it is panned. Adding it against no consumer would have been guesswork, the same call `Needs`' bulk half got in K3.

  **What an element reports is `Events`** — a list of messages, not a value — because the case that is not one value is a patcher's release reporting a `"move"` per box. The bound-vs-event decision stays where it was, on one restated rule: a single argument is a value and takes the value road, anything longer is a tagged edit-back payload.

  **An overlay is declared, not flagged.** A `menu`'s open list is the state that is not a value, and it used to exist twice — a `MenuOpen` in the machine so the next press could be tested against the rows, and a `FrameInputs` field so the renderer knew what to paint over the window. `overlay_rect` replaces both: the frame draws `overlay()` last into the overlay mesh, and a press routes to whoever declares one before the tree is consulted. Nothing is kept in step, and a def that replaces the tree takes its open lists with it.

  **The leaves that landed it**, one commit each after the seam: `slider` (absolute — a position in the groove *is* the value, and the groove is snapshotted because it may not move under the drag), `knob` and `number` (incremental — a delta re-anchored every step, so a control pinned at an end has no dead zone; the knob is what proves the grab), `button` and `toggle` (the click, and the held state that used to travel a full circle through the machine and back down through the frame), and `menu` (the overlay). The third family — snapshotted, a press-time origin plus a container's axis and a snap — belongs to the leaves placed on a time axis and lands with them in K7.

  What went with them: six `Drag` variants (`Slider`, `Vertical`, `Button`, twenty-five down to nineteen), `FrameInputs`' `active_button` and `menu_popup`, `Gestures::active_button`/`menu_open`/`close_menu`/`MenuOpen`, the frame's `MenuPopupItem` and its collect arm, `controls::draw`'s `active` parameter, and four `interact` doors that existed only to route a mutation in by kind (`set_fraction`, `fraction_of`, `flip_toggle`, `set_menu_index`) — for an element there is no outside. `WidgetKind::` is mentioned 305 times, from the 389 the track opened at.

  One behaviour is now uniform where it was special: **every taken press is held** until the button comes up, a toggle's included. It changes nothing a user can see and it removes the question of which claims are "clicks", which the machine has no way to answer and no longer has to. The release of an element repaints even when it reports nothing — an element that drew itself held has to be drawn let go.

  **A sub-hit is not on the trait.** The host's hit-test answers *which widget* — rect containment over the placements, generic — and *which part of itself* was hit is the element's own business, inside `press`, where it already is for a caret, a note, a break-point and a patch port. Putting a `Part` on the trait would make the host route by a type it cannot interpret, which is the routing this track is removing.

  A gesture takes an `Input`, not the `Ctx` a draw takes, and the reason is structural rather than stylistic: a gesture **mutates** the host tree while the `World` borrows out of it (the timeline groups, the queried node trees), so nothing can hold both. An element reads the outside when it draws and moves itself when it is dragged. `Input` carries the placement, the modifiers and the window size, and grows the container's coordinate system the same way `Ctx` does.

- ✅ **K5 — The body-role seam, and the `curve` port.** *(Rewritten on landing: the milestone opened as "the element owns its payload" and the payload half turned out to be already done — see the last paragraph.)*

  **The seam.** A `clip` layers its contents — the material, the events over it, the automation over both — and recognized each of them by matching the leaf's *variant*, in seven places: `empty_clip_body`'s candidate array and the `fn(&WidgetKind) -> bool` predicates threaded through it, `ensure_body`'s layering rank, `track::draw_body_widget`, `interact::clip_point_edit`/`clip_point_move`, the clip's hit-test, the frame's body collect and the point drag. A leaf behind the trait has no variant, so a curve could not move until the container had another way to know one of its bodies.

  `BodyRole` is that way, and it is shaped after the GPU slot because the boundary is the same one: **a closed set belonging to the container**, which an element *chooses* among and cannot invent. `Element::body_role` declares it, `WidgetKind::body_role` is the single door (a built-in answers from its variant, an element for itself), and the role's own declaration order **is** the layering order, back to front — so `ensure_body`'s rank stops being a hand-written table. The container keeps the layering, the axis and the edit doors, which are a coordinate system's business.

  It landed as a **method rather than a field of `Needs`**, which is the one place the proposal moved. `Needs` is what an element reads from *outside* itself, gathered by the tree collectors once per collect; a body role is what the element *is* inside a container, asked by that container on a hit and on a `/gui_set`. Same closed-set discipline, different question and a different rhythm — and asking it would have meant allocating four vectors per hit.

  **What the trait gained with it** is the third of the three things in: `Ctx`/`Input` carry the **container's coordinate system** (`TimeSpace` — the visible window and the span), which the design named from the start and nothing had yet filled. That is what makes one element two placements rather than two drawings.

  **The port.** `WidgetKind::Bpf` is gone, and with it a build arm, an apply arm, the frame's `BpfItem` and its collect arm, `interact::bpf_edit`, the whole clip half (`clip_curve`, `clip_point_edit`, `clip_point_move`, `ClipHit`'s `point`/`has_curve`), `read::bpf_event_args` with `emit_points`, three `Drag` variants (`BpfPoint`, `BpfCurve`, `ClipPoint`) and `track`'s four curve twins. `host/bpf.rs` stays the model, and the two display mappings it and `track` had written twice — the widget-local one and the clip-placed one — became one `bpf::Axes`: a field, the window of the domain it spans, the value axis. The element picks the window; everything else is shared.

  **The machine grew the address a body needs.** `Drag::Element` carries an `element::At` — which widget, or which *body of which container*, plus the placement and the axis it was placed on — and a body's address is its container's id plus its role, which is the routing a `/gui_set` of a body prop already took (a body carries no id; the script addresses the clip). The clip offers a press to its bodies, topmost first, before its own drag.

  **One rule the port had to state rather than inherit**, and it is the finding worth carrying to `notes` and the take: **a body claims its own parts and declines everywhere else.** It shares its rectangle with the container, whose own drag is what the rest of that rectangle means — so bending a *segment* stays the standalone view's gesture, or an automation clip stops being movable. Standing alone the whole field is the element's. A gesture test covers both halves, because the port nearly shipped without the second. (The segment drag itself is confirmed working and confirmed **unclear** — see "the editing gestures want affordances" under "Found by use".)

  Two things the shared mapping fixed on the way: a clip's curve now draws the vertical connector at a discontinuity (a hold-shape automation used to render its jumps as one-pixel slants), and a body's points and text follow the placement's own size table. `WidgetKind` is sixteen variants and `WidgetKind::` is mentioned **299** times, from the 305 K4 left and the 389 the track opened at.

  **The payload half was already spent, and that is the honest report.** K4 made a gesture return `Events`, so every reader whose payload the gesture *produces* went home with its leaf; the curve's `"points"` is produced by `press`/`drag`/`release` and needs no trait method. The non-gesture read the milestone was written around had **no consumer** as this landed: `/gui_query` answered from the registry's wire props rather than from the live tree, and the one binding that forwards on a `/gui_set` forwards a scalar. So `event()` was **not** added — it would have been a declaration with nothing calling it, the call `Needs`' bulk half and the `Claim`'s edge-scroll flag already got. `Element::value` stays the scalar door. (`Element::info` was in the same position and predates this — declared in K1, unread — and it is what the `/gui_query` fix recorded under "Found by use" then gave a consumer, which is also the answer to why `event()` is still not needed: a query wants the props' own vocabulary, keyed, not one anonymous payload.)

  Also fixed here, in its own commit because it predates the work: `clients/python/tests/test_gui_props.py` read the host's vocabulary out of the widget schema alone, so every leaf that moved into `host/elements/` had been reading as a "python and web only" divergence since K1 — eleven widgets, which is the shape of a real finding and therefore what made the instrument useless. It reads the element files too now.

- ✅ **K6 — Focus and the keyboard, generically** *(the original K4, moved ahead of the port; landed with the `text` port, which is why K7 no longer lists it)*.

  **The seam.** `Host::focused_text` was focus *of text* — one field, one parallel path, no tab order and no way for a script to ask where the keyboard is. It is now `focused: Option<(def_id, widget_id)>`, one focus per host because there is one keyboard, and the trait carries the two methods that make a leaf a keyboard reader: `accepts_focus(&self)` (whether it is a stop on the ring, and whether a press moves the focus there) and `key(&mut self, &Key, &mut KeyInput)` — `Some` consumes the key and its `Events` are delivered exactly as a drag's are, `None` lets it fall through to the front's own shortcuts, which are addressed to what is under the *cursor* rather than to what holds the focus. `KeyInput` carries the modifiers and the **host-wide clipboard**, which is not the element's: one string serves every field of every window, so a selection cut in one pastes into another.

  **The ring is asked, not remembered.** It is the layout pass' own order over the widgets that accept focus, recomputed per step (`host/gestures/focus.rs`), so there is nothing to keep in step with a freed widget, a redefined tree or a `stack` that turned the page. The wire gained `/gui_set <id> focus 1` and a `"focus"` event on both ends of every move **the user makes** — the widget that lost it as well as the one that gained it, since a script mirroring the focus needs both, while a `/gui_set focus` is not echoed, exactly as no other set is — and it is emitted as a **notification** even from a bound widget: a binding says where a widget's *value* goes, and this is not it. `focus` is deliberately **not a prop**: it is taken out before the document is written, so a `/gui_query` does not report it and a reloaded def does not restore a focus nobody asked for. The ring is drawn by the host, one `focus` colour role and one `focus_ring` metrics role, so being focused looks the same everywhere; what being focused means *inside* an element (a caret, a selection) arrives as `Ctx::focused`.

  **Tab leaves the tree, and that is the whole browser half** — which turned out to be smaller than the entry expected and in one place it did not. winit already gives its canvas a `tabindex` and forwards the keys, so no page-side plumbing was needed; what *was* needed is the exit, because winit also prevents the browser's default on every key it sees, so a wrapping ring would have made a mounted GuiDef an absolute keyboard trap. `GestureEffect::FocusOut` is the report — the browser shell blurs its canvas and the document's tab order carries on; a desktop window has nothing outside to hand the keyboard to, so nothing is focused and the next Tab enters the ring again. **The desktop deliberately does not wrap** — the shell could, and every desktop toolkit does, but the platform that *cannot* wrap sets the rule for both: one behaviour to learn, document and test beats saving a keystroke on one target. The rationale is in `docs/decisions.md`; IME and the system clipboard stay the page's, stated as a limit rather than left to be found.

  **The port.** `WidgetKind::Text` is gone, and with it a build arm, an apply arm, a size arm, two query arms, `controls::draw` entirely (its last two kinds had already moved), `interact::text_edit`, `interact::text_caret_at`, the `Drag::TextSelect` variant and its drag arm, and the 140-line `gestures::keys::text_key`. `host/elements/text.rs` holds the field, its caret and its `key`; `host/textedit.rs` stays the pure model and `controls::field` the drawing, which is the rule every ported leaf follows — **the element owns its state, not its geometry**. The drag's anchor is a field of the struct rather than a variant of the machine's `Drag`, which is what let the variant go. `WidgetKind` is fourteen variants and `WidgetKind::` is mentioned **281** times, from the 299 K5 left and the 389 the track opened at.

  **What the generalization bought immediately**, beyond `text`: a press now moves the focus by *asking the widget* instead of matching on `Text`, so a registered element is a stop on the ring like any other — the seam's suite covers exactly that, a third-party element that reads the keyboard in two methods. And a key an element declines still reaches the view under the cursor, which is what lets a field swallow `q` while a piano-roll behind it keeps quantizing on the same key when nothing is focused.

  - **Out of scope, recorded**: an accessibility tree (AccessKit). It is native-only with no canvas equivalent, and shipping half of it in one of two targets is worse than shipping none. Revisit if the desktop host becomes the product.

- ✅ **K7 — The port: the rest of the leaves onto a finished surface.** One leaf per commit, each with its tests moving beside it into `host/elements/`, each green on its own — `Custom` sits beside the enum, so the host is never half-migrated. The order is increasing surface, so a mistake in the trait is found early and cheap. **Twelve leaves have already landed with the seams that needed them** — `label` (K1), `meter`, `nodes` and `canvas` (K3), `slider`, `knob`, `number`, `button`, `toggle` and `menu` (K4), `curve` (K5), `text` (K6) — which leaves:

  1. ✅ **A payload** — `score`. The page (`host/score/`) was always the model; what the leaf owed the machine was routing, and that is what went: three `interact` doors (`score_select`, `score_drag`, `score_drag_end`), two payload readers, a `nav::score_steps` lookup and the `Drag::ScoreStep` variant, which existed only to carry the press-time origin the machine had nowhere else to put. It is a field of the element now. Two things landed with it. **`Needs` grew `clock`** — whether an element reads the engine sample clock — because a score's sweeping cursor is the one playhead that is the *widget's* rather than a navigation group's, and `live.rs` was asking that by matching on the variant; a front with the shared segment mapped reads the clock for free while a page has to poll for it, which is why it is a need of its own and not a flavour of `animated`. And **`Ctx` grew `clip`**, the placement's clip rectangle, for the element that narrows the clip itself: the frame holds that datum, so an element must not have to ask the mesh what state it was left in. A score also answers `/gui_query` with its `selected` now, which the built-in arm never did.
  2. **A GPU slot** — `signal` (one arm already, and it ports as one), which is also where `Needs`' bulk half and the geometry/texture slot kinds land — the two pieces K3 deliberately left for the consumer that shapes them. It is **four seams and then the port**, because a signal element is the only leaf that is fed from outside on that many different rhythms, and each of them was a walk of the tree matching on the kind:

     - ✅ **The tick** (`Element::tick`, a `Live` context). What a live view accumulates from a forward-only source — a rolling history, a triggered window, a per-channel analysis, a waterfall's transform — lived in **four front-side maps keyed by widget id**, filled by four collectors and read by a frame that looked each one up again. It is one `LiveState` on the element and one walk now, and the frame's live items borrow it (which is why the `id` came off all four: it existed only to find the data). The context is separate from `Ctx` on purpose — a tick mutates and runs at the front's steady rate, while a draw borrows the world out of the same tree — and the retained history of a **bus** stays outside it, since one history serves every view watching that bus. The browser gained `StreamedSource`, one `BusSource` over its two subscriptions, so both platforms now read through one door.
     - ✅ **The bulk half of `Needs`, and where a load lands.** An element says which resource it wants and in which form (`Bulk::{PeakCache, Peaks, Samples, StftCache, Stft, Buffer}`) and takes the answer home (`Element::bulk`). Four walks were deriving that from outside — `collect_slots` and `load_element_bulk` natively, `collect_bulk` in the page, and a third fork in each `serverleg`'s buffer reply — and the browser's `BulkRequest` was already exactly this taxonomy, so it is gone. **Where the answer goes is a declaration too**, which is what makes the fork disappear: a claimed slot is fed through the slot, everything else takes the data home. `Loaded::Raw` is the form that needed no fork at all — a server buffer's samples go home as they are and the element makes of them what it draws from, at its own bucket.
     - ✅ **The geometry and texture slot kinds** — `SlotKind::{Geometry { base_bucket }, Texture { window_size, hop, sample_rate }}`, their parameters riding with them because whoever fills a slot has to make the data fit it and only the element knows how. Both loaders route on them now, and the filling is the bullet below.
     - ✅ **The filling** (`Element::fill`, a `SlotFill`). A slot had two rhythms and only one door: `Element::slot` said what a claimed slot *draws* this frame, and what it is *fed* was still derived from outside, twice per front. The inline-data path matched on the presentation to decide whether an element's own samples became a pyramid or a set of analyses (`collect_timelines` natively, `build_inline_timelines` in the page), and the waterfall pass reached into `el.live.roll` for the columns of the tick. Both are one walk now — `frame::fill_slots`, which asks every widget the same question and uploads what comes back, keyed by the addressing rule (a clip's body is its container's id) it also states. `SlotFill` is the three shapes an element can hand over: `Geometry` (a pyramid), `Texture` (the analyses) and `Columns`, the rolling case, whose upload is what landed since the last fill rather than the whole span. **A fill is a taking**: the element marks itself clean because the frame kept what it gave, which is what keeps a still window at zero uploads — and the one thing it cannot know for itself, that a window's device was rebuilt and took the contents with it, arrives as `Element::slot_dropped` (a fresh native window, a re-attached page canvas). A **load** stays the loader's: what a mapped file, a fetch or a server buffer resolved is routed into the slot on the declared kind, which is why an element whose data is still out there hands back nothing rather than an empty picture that would be replaced the moment it lands.
     - ✅ **The tail the port had to answer.** With the four seams above, what was left was not data at all — each is a small door on the trait or on `Needs`, and none of them is a design question the way the four above were:
       - ✅ **The tap subscription** (`Element::tap_frames`, `Needs::retention`). Three collectors walked the tree building a per-kind read spec — a scope's window and trigger, a goniometer's pair, a spectrum's FFT size — only for `tap_stream_frames` to take the largest of the three and throw the specs away; they are gone, and each widget answers how wide a read of its taps has to be. The span is the other half and is data, so it is a `Needs` field: the history is the **bus's**, and a view now retains exactly the buses it declared it taps (which is also a small fix — a goniometer's span retains the pair it reads, where the walk had gone by `channels`).
       - ✅ **The lane count and the vertical anchor** (`Element::lanes`, `Element::centres_y_zoom`). Two halves of one answer met in the gesture machine: the front knows how many channels reached the card (only its GPU slots do), the widget knows what it *does* with them — so `GestureCtx` carries one `slot_channels` map where it carried a `wave_lanes` and a `spect_lanes` (the two existed only to pick which one to match against), and an overlaid trace answers one lane out of four channels. The anchor is the same shape: whether a y zoom holds the centre or the value under the pointer is a property of what the axis *measures* (amplitude is centred on zero in every lane; frequency and pitch say the same thing in each), and the widget states it instead of the wheel matching on a presentation.
       - ✅ **The axis chrome, the body draw and the default drag** (`Element::{gutter, measured_gutter, draw_body, texture_body, gesture_map}`). What a leaf placed on *somebody else's* axis owes it, in one pass: the band it reserves left of the shared body — asked twice, from the props before anything is placed (the layout needs it to place a lane's clips) and from the placement after, because an amplitude ruler's width is a property of the data and not of the kind; what it draws as a **clip's body**, chromeless and against the clip's own span, with `texture_body` for the one picture the frame has to route itself (a time-frequency take samples an uploaded texture, keyed by the clip's id since a body carries none); and the drag table a navigable view wants when the wire declares none, which is the axis' own selection and pan rather than the picture's.
       - ✅ **The frequency-axis reads** (`Element::{navigates_freq, freq_axis, freq_window_of, freq_min_span}`, and `FreqAxis` moved to the seam). The one axis in the host that is neither the window's shared time nor a container's coordinate system, and the machine held all of it: where the picture lies inside the rectangle (the element's own region split — it must be the *same* one the renderer drew through, or a zoom anchors at a hertz the reader is not pointing at), the window it shows, what it would show if asked for another, and the floor a zoom may not pass. All four are facts about the **analysis** — its resolution, its scale, its rate — and the machine only ever wanted the numbers.
       - ✅ **The frame's item fan-out**, as far as it goes without the port: the clip-body branch asks `texture_body` instead of matching the arm and reading `is_texture_view`. The rest of `frame::items::signal_item` — six item lists, one per renderer a presentation picks — is not a door and does not become one: those *are* the built-in's draw, and they go when the arm does.
     - ✅ **The arm itself.** `WidgetKind::Signal` is gone: every view of a signal is a registered element, built by a constructor the schema looks up and drawn through the trait. The draw split along the line that was already there — the four mesh presentations and the stored plot draw in `Element::draw` (the plot's hover readout in `Element::overlay`), and the two heavy ones describe their frame in `Element::slot`, which is what `SlotFrame::{Waveform, Spectrogram}` are: the body, the vertical window and the look, executed by the frame on the pipelines it owns. The horizontal window is deliberately *not* in them (it is the navigation group's, addressed by an id the element does not carry) and neither is the lane count (it is the slot's, being what reached the card). Three more doors fell out of it and are the ones a **later** leaf on a shared axis will want too: `Ctx::indent` (where the group's axis begins, the layout's answer and not a measurement), `editor`/`editor_mut` (what a group member is made of, read *and written* from outside), and `as_any`, defaulting to `None` — not for the passes, each of which has a door, but for an element's own tests. `frame::items::signal_item` and its five item lists are gone with it; the chrome around a heavy view stays the frame's, because rulers, playhead, selection and readout belong to the **axis** and a lane, a roll and a free-standing ruler draw the same ones.
     With that, `WidgetKind` is the containers plus `Custom` and `Unknown` — and the three leaves of item 3.
  3. ✅ **Group-aware** — `keys`, `notes`, `patch`: the three that read a container's axis or hold a selection, last because they exercise every seam at once. **What the port needs, surveyed once item 2 was done** (56 match sites over 19 files; the order below is by how much of each is already a door):
     - ✅ **`keys`** (`host/elements/keys.rs`). Both drags moved into the element as the **snapshotted** family K4 named and left for the leaves: the press records what it needs (the strip and the range, or the key layout and the pitch under the cursor) and every motion is measured against that, so a drag pinned at an end never drifts. `Drag::PianoKey` and `Drag::PianoView` are gone from the machine, and with them the four `interact` doors that existed only to reach into the variant (`piano_press_key`, `piano_release_key`, `piano_set_range`, `piano_key_active`) and the two effects that wrapped them.
       **Voice mode became the seam it always was**: an element cannot reach the audio server, so it *asks* — a note carries a `Voice` beside what it reports and the host performs it, reading the def from `Element::voice`. That splits the two questions the variant had conflated: *what to play* is a prop a script sets (and can change mid-hold), *play it now* is a gesture's. `Host::piano_voice_on/off` are `voice_on/off` now and know nothing about keyboards.
       One rule the port had to state rather than inherit: **an element's wheel wins over the container it sits in**. The old `Piano` arm ran before the timeline and plane checks, and the generic element arm ran after them — so a keyboard inside a `plane` would have started scrolling the plane. The element is asked first now and `None` back hands the wheel on, which is what the fall-through already meant.
     - ✅ **`notes`** (`host/elements/notes.rs`). The two asks landed as named, and each of them turned out to be the *same* answer in a different direction. **The axis** is not snapshotted but **resolved per call** (`gestures::element::time_of`): a widget addressed directly reads its navigation group's window now, a body still takes the container's, and both arrive as `Ctx::time`/`Input::time` — which is what makes a drag survive an axis moving under it, and therefore what makes the edge-scroll flag work at all rather than freeze the note where the axis used to be. `TimeSpace` grew the other two facts of that same axis, its **shared selection** and where its **playhead** stands, because the roll draws them: its ruler sits under a grid with two strips between them, its selection band covers the grid alone and its readout names a pitch, so its chrome was never the heavy views' and stays with the element rather than going to the frame. **The request** is `Events::and_select`, beside `Voice`. And `Take::edge_scroll` landed on the same tick the clip's edge scroll already ran on, which is one arm rather than a second mechanism.
       Three more doors fell out, each replacing a match the machine was doing on its behalf: `Element::axis_body` (where the shared axis lies inside the rect — a roll's is its **grid**, not the rect minus its chrome, since the velocity and event strips are stacked under it and read the same time), `Element::content_span` (how far an **authored** surface reaches, which is what lets a roll being written into lengthen the group's timeline — it replaced a match in `sync_track_totals`, one in the ungrouped-view fallback, one in the roll-grid lookup and the `"notes"`/`"osc"` key list in `on_set`), and `Needs::midi` + `Element::midi` with the platform-neutral `MidiNote` (the `Key` posture: the front owns the device and the translation, the element owns what a note *does*) — which brought the held keys and the step cursor home from two maps on the native front.
       **The keyboard's second addressee is now a door rather than four methods.** `q`, Delete and Ctrl+C/X/V were front shortcuts matching on the kind; they are arms of `Notes::key`, reached by `Gestures::key_at_cursor` — the same call the focus makes, at a different address, which is exactly what K6 said out loud ("a key an element declines still reaches the view under the cursor"). The note clipboard went with them: there is **one** host-wide clipboard now, and a block of notes rides it as the JSON a `/gui_set notes` already accepts, which is the carrier every non-scalar uses.
       What went: `WidgetKind::PianoRoll`, six `Drag` variants (`Note`, `NoteBlock`, `Velocity`, `VelocityBlock`, `OscMark`, and `Select`'s whole `value` half), five `interact` doors (`pianoroll_notes_edit`, `pianoroll_state_edit`, `pianoroll_osc_edit`, `clear_element_selection`, `select_elements_in_rect`), the `pianoroll_hit`/`PianoRollHit`/`PrRegion` block with its two pickers, two payload readers, `YAxis::window` and `interact::value_at`, `frame`'s `PianoRollItem` + `draw_pianoroll_item` + `pitch_window`, `track::draw_piano_roll`, `nav::note_at`, four `gestures::keys` methods and their four front dispatches on both fronts. `WidgetKind::` is mentioned **198** times, from the 281 K6 left and the 389 the track opened at.
     - ✅ **`patch`** (`host/elements/patch.rs`). It needed nothing of its own, exactly as the survey said, and that is the finding: the last and least control-shaped leaf in the catalog — a cord that means something only where it is *let go*, a whole selection moved by a delta, a marquee that selects the element's own boxes — is three ordinary `press`/`drag`/`release` arms over three fields of one struct. Nothing was added to the trait for the drags, which is what "the drag shape is general" turns out to mean.
       **The marquee came home, and took a coordinate system with it.** `Coords::Canvas` existed for one reason: the machine had to know it was over a canvas so a plain drag could run the `Select` plan and sweep a rectangle nothing outside the widget could interpret. So it is gone, with `CanvasAt` and `Frame::scale` beside it — a patcher is a *leaf*, and the plane it is seen through is the workspace's, which the chain already carried. What the container keeps is what a container is for: Shift on the bare canvas is declined by the element and pans the workspace around it, which is the same rule `notes` stated for its own strips.
       **One door, and it is the opposite of an existing one.** `Element::content_size` is what a workspace sizes its scrollable content to, and it is the deliberate inverse of `natural`: a natural size is pure over the metrics and must **never** follow the data (it resolves in the layout's main axis, so data-driven would make a `/gui_set` relayout a window), while this *is* the data — the graph the host laid out — and it sizes a plane that scrolls rather than a slot that divides. Same shape as `content_span`, one axis more.
       One consequence stated rather than found: a `gestures` prop on a patcher node is now **inert**, because a table belongs to a container and a patcher is a leaf. That is what `docs/gui-protocol.md` already says ("any *container* may carry a `gestures` prop"), and the leaf's own answer — `Element::gesture_map` — is `None`, so it takes the default every element takes.
       **And the instrument came back green**, which is the **third** time this reader has gone blind on a restructuring (E6, K1, here) and therefore the last time it is left to infer where a leaf lives: `clients/python/tests/test_gui_props.py` read `elements::builtin` for a module *file*, so the two leaves that are not one — `signal`, written across `host/signal/`, and the patcher, built by the **schema** because its wire name `plane` means two constructions — had silently dropped out, reporting eleven props as a "python and web only" divergence. Both resolutions are now named in the reader rather than inferred, and two blind spots in its Rust scanner went with them (a helper reached by its module path, `widget::signal_element(props, blobs)`; a key passed after the thing it is read over, `spectral_props(props, el.spectral, "window_size")`). The failure predates this commit — it landed with item 2 — so it ships as its own.
       What went: `WidgetKind::Patch`, three `Drag` variants (`Wire`, `Box`, `Marquee`), `Gestures::marquee`/`wiring` and the four `interact` doors behind them (`graph_cord`, `graph_select`, `graph_move`, `graph_marquee`), `Coords::Canvas` with the `Select`-on-canvas press arm, `CanvasAt`, `Frame::scale`, the frame's patch draw arm and — the K3 diagnosis closed — the **last two `FrameInputs` fields that were one widget's interaction state**, `marquee` and `wiring`, with the cursor plumbing each front fed them. `FrameInputs` is three fields (metrics, world, focused), and `Gestures::release` is one arm. `WidgetKind::` is mentioned **187** times, from the 198 `notes` left and the 389 the track opened at.
     What item 2 already bought them: `Ctx::indent`, `editor`/`editor_mut`, `gutter`/`measured_gutter`, `body_role`, `draw_body`, `gesture_map`, `lanes` and `needs` are doors, so none of the three has to invent one — what is left is each leaf's **own interaction state**, which is where the remaining match sites are. `notes` and the take are the other two **body roles**, so they land on K5's seam — declaring a role and taking the container's axis, and following its rule that a body claims its own parts and declines everywhere else. `notes` is where the `Claim`'s edge-scroll flag lands, being the first element that runs off a lane's edge; `patch` is the one that proves the drag shape is general rather than control-shaped, with its two drags, its marquee and its multi-selection.

  What each commit deletes is the measure of the milestone: an arm from each of the eight passes, a `Drag` variant or three, an `interact` door and a `read` reader. The prize is not the eight edits — it is that **arm 26 can no longer quietly duplicate arm 12**, which is the pressure the `signal` collapse had to apply after the fact and which nothing structural prevented from recurring.

- ✅ **K8 — The enum is the containers, and a test says so.** `WidgetKind` holds `Window`, `Panel`, `Stack`, `Scroll`, `Track`, `TimeRuler`, `Clip`, `Custom` and `Unknown` — nine, from twenty-five.

  **The guard is two tests, because the rule has two halves.** One reads the enum out of its own source and pins the nine names, so adding a variant fails until whoever added it has read *why* — the failure message points at the test's own documentation, which states the boundary (a container arranges and defines a coordinate system, which is why they are a closed set; an element draws in one, which nothing outside it has to know) and where a leaf actually goes. The other asks the same rule of the widgets rather than of the source: a document builds one of each of the seven containers and every one of them answers the neutral value through every door — no element, no `Needs`, no focus, no body role, no overlay, no value, no axis of its own, no taps, no authored span. **A convention nobody checks is how the eight passes grew in the first place**, and a name list alone would pass while a container quietly grew an answer.

  **`query.rs` collapsed to one match.** The milestone expected "leftover per-kind helpers to fold into trait methods", and what the port had actually left was the opposite shape: twenty-odd methods each spelling `match self { Custom(el) => el.x(), _ => default }`, which is a pass over the enum written out to ask a question the enum does not answer. They are one line each now, through `as_element`/`as_element_mut` — so the *only* match left in a leaf's question is that one door, and the method beside it carries nothing but the **container's** answer. The five that stayed a match are the five where a container genuinely answers (`is_bare_surface`, `info`, `gutter`, `editor`, `editor_mut`). `WidgetKind::` is mentioned **165** times, from the 187 the port left and the 389 the track opened at.

  One fork closed on the way, found by the fold: `Widget::take_bulk` promised in its documentation to route a load to a clip's body and did not — **both fronts were walking that level themselves**, the same walk written twice. The routing is the door's now (`serverleg` calls it once on each side), and it takes a *maker* rather than a `Loaded`, since the payload is built only for the widget that takes it and never copied past one that declined.

  `interact` needed nothing: what is left there is the hit chain, the coordinate arithmetic and the **containers'** own doors — a scroll's view, a lane's header and thickness, a clip's placement, their three payloads — which is exactly the line the milestone drew. The per-widget doors left with the leaves that owned them, one milestone at a time.

  What is still lore rather than API is the widget author's checklist, which K16b documents: one file in `host/elements/`, one row in `elements::builtin`, and — unchanged, cross-package, and by design — a builder in Python, a builder in TypeScript and a row in `docs/gui-props.md`, which `clients/python/tests/test_gui_props.py` holds together.

### Part B — the general capabilities

*Unchanged in substance from the original track; each is cheaper after Part A, landing on one file per widget instead of on eight passes.*

- ✅ **K9 — The embedded font grows lowercase, accents and a real advance.** The 5×7 cell rendered lowercase as uppercase and had no accents, so `text` — an editable field of real editing — could not write "canción", and every user-visible string (a track name, a file path, a label a script wrote in Spanish) goes through it. The one limit no user of the library could work around, and fixable without a dependency.

  **The entry's own plan was the thing that had to move**, and the reason is worth keeping. It said the glyph box grows from 7 rows to 9 and `CELL` is pinned at 14 so the layout does not follow it. Pinning `CELL` is right and landed — but it only saves the *table*: with a 9-row box and the default scale, text would draw 18 px tall inside controls sized for 14 and clip. Growing the box moves the ink whatever the table says.

  So the split is one row lower down: **`GLYPH_H` is the body box** — the seven rows a capital fills, what `font::height` returns and every layout reserves, unchanged — and a glyph may put ink **outside** it, `ASCENT` rows above for a diacritic and `DESCENT` below for a tail, the way a real typeface overshoots its cap height. The drawing origin is the body box, so every capital lands on the exact pixel it landed on before (a test asserts it), and **nothing that was on screen moved**. What did change is `line_advance`, now the whole glyph rather than the box plus a row: two lines of wrapped text have to clear each other's tails and marks, and that is the one deliberate visual change — in the direction the milestone exists for.

  **Latin-1 is composed, not enumerated.** A mark is placed two rows above the base's *own* topmost ink, which resolves the space problem by itself: a lowercase letter's x-height leaves the box's top two rows free, so `á`/`ñ`/`ü` sit **inside** the body and cost no room at all, while only a capital's mark lifts into the ascent. `í` decomposes to the dotless base so the accent replaces the dot rather than stacking on it, the cedilla hangs in the descent row and the slash crosses the body. Ninety-six bitmaps became one table of fifty-eight rows plus seven marks, and a test walks every alphabetic Latin-1 code point asserting none of them falls back to the box.

  **`CELL` is declared, and that is the whole of the dependency direction.** It was `font::GLYPH_H * font::DEFAULT_SIZE`, which made the layout of every window a consequence of how the face was drawn; it is `14.0` with a test fixing it, and the cell is what the face is drawn to fit. `font::advance(scale)` is the face's own answer for the roles sized to hold text (`ruler_w`, `header_w`) and for the field's caret column, replacing both the metrics module's private restatement of the cell and the `font::width(" ", size)` the text field measured a column with. That is the seam K10 needs: a proportional face is asked for a string's width at the mutation point where the string changes, never in a layout pass.

  By eye through `text.py`, which grew the panel the milestone is about: the alphabet in both cases, an accented line, and the same text in a field you can type into.

  **The direction of the dependency is the decision, and it is inverted here**: the cell is declared and the *font is rasterized to fit it*, never the other way round. That is what K10 then builds on — a `FontSource` is asked for a face at the pixel size whose line height lands in the declared cell — and it is what keeps the property the sizing model paid for: the metrics table is constant data resolved once per scale change, so **changing the typeface must never relayout a window**. What legitimately follows the font is the measurement of a *string* (`advance_of`), which happens at a mutation point, not in a layout pass: the cell says how tall a line is, the font says how wide this text is. It follows, and is recorded so it is not rediscovered while implementing: a user who wants larger type asks for it where density already lives — the `scale` multiplier of `[gui.metrics]`, resolved once — and never by choosing a different face.

- ✅ **K10 — A real font behind `font-atlas`.** A rasterizer (fontdue — pure Rust, wasm-safe, no system dependency), one atlas texture per window and a second, textured pipeline that exists only when the feature is compiled in. `text_size` is continuous with the feature on and quantized without it, which is the one place two builds of this host legitimately differ.

  **The entry planned an embedded face, and that is what moved.** A face is hundreds of kilobytes with a license of its own, in a repository that carries no other binary asset — and the platform has already answered the question: a desktop has faces installed, a page can fetch one. So what the feature compiles in is the **rasterizer**, and the face arrives through the seam the entry already named: `FontSource`, the fifth platform trait, filled by `fontfile::FontFile` natively (`--font <path>`, `[gui] font`, or one of the system's own when neither names one) and by `web::FetchedFace` in the page (`GuiBridge::font(bytes)`, what a `fetch` came back with). `docs/decisions.md` records the three costs.

  **Which makes the feature safe twice over**, and that is the shape worth keeping: the bitmap face is the **floor**, not a fallback to reason about. Every entry point in `host::font` asks `atlas::has_face()` first, so a host built with the feature and pointed at no face draws exactly what a host built without it draws — same measurements, same quads, same tests. There is no boot to block on a font and no error path to design; a machine with no fonts still draws every label.

  **The cell still declares and the face is still fitted to it.** A scale rasterizes at the pixel size whose *cap height* is the body box, so a capital through the atlas is exactly as tall as the one the bitmap drew and **no layout follows the typeface** — loading a face is a redraw, never a re-measure. What legitimately follows it is the width of a **string**, and the port was making that measurement real everywhere it had been a count of cells: the ellipsis cut, the word wrap (`wrap` takes a width now, not a column count), the field's caret, its selection band and its click-to-column hit-test. Each of them measures the glyphs it is actually drawing, which is the same arithmetic under a fixed-pitch face and correct under either. The field keeps its scroll window in *characters* — a caret moves by characters — and binds its visible run by width, since a field's text is clipped by nothing.

  **One batch survived, and one rule had to be stated to keep it.** The glyphs are a second vertex list *inside* the same `Mesh`, sharing its clip rectangle, uploaded with it and drawn immediately after it — so the atlas is one texture and one bind group per window, and text draws over the flat geometry of **its own** batch, which is where it always sat (the overlay batch still paints over both). A clipped glyph cuts its texture window with its quad, so half a letter at a `scroll`'s edge is half a letter rather than a squeezed whole one. The sheet is 1 MiB of coverage shelf-packed by `(character, pixel size)`; a document that overruns it repacks, which costs one frame of stale texels and no growing texture on every window.

  **What is on trust, and it is the tests' own limit**: the crate ships no face, so the atlas tests read one from the system and *skip* where there is none. They state that rather than hide it — the alternative was committing a megabyte of test data to assert what a machine with fonts already proves.

  By eye through `text.py` against a host built with the feature (`cargo build --release --features font-atlas --bin clausters-gui`, then `CLAUSTERS_GUI_BIN=…`): the same window, the same layout, in a real typeface.

- ✅ **K11 — A container may hug its content.** The layout stays one pass, no measurement, no constraint solver — this widens what a natural size *is* without crossing that boundary. `hug` on a `window`, a `panel` or a `stack` makes its natural size the composition of its children's (`Widget::hug_size`), which is one bottom-up walk over functions that were already pure and already computed. Both workarounds are retired: `rulers.py`'s row of menus is `hug=True` instead of the `h=60.0` a comment already called compact, and the single control alone in a window (`bind.py`, `standalone.py`) drops its `weight=1.0` because the **window** hugs — which is what that entry turned out to mean.

  **The rule the milestone was written around moved from "never the data" to "where the value is resolved"**, and that is the whole of its design. A prop settles at a *mutation point* — a `/gui_def`, a `/gui_set` — the same place the theme overlay already resolves; a **value** streams, one per pointer move, one per frame, one per keystroke. The failure L7 avoided is entirely on the value side (a window jumping while a control is turned, a relayout per message), so a menu's `options` may size it and the option it is *on* may not, a label's `text` may and a `number`'s reading may not.

  **Two mechanisms keep that structural rather than conventional**, and they are what let this land with no one-time break at all: the question is a **different method** (`Element::hug`, beside `natural`, defaulting to it), so the ordinary pass cannot read content by accident and every element that says nothing keeps its data-free answer; and it is only ever called **under a container carrying `hug`**. Four leaves override it — `label` (its line), `button` and `toggle` (their captions), `menu` (its widest option, never the one it is on) — and each measures what its own drawing lays out, so a hugged control never draws itself into an ellipsis. Every other element, every value and every signal answers the natural size unchanged, and the existing suite passed untouched.

  **What composes, and what propagates.** A `row` sums along its axis and takes the largest across it, a `col` the other way round, a `grid` counts its cells, a `free` container reaches its children's placements, and a `stack` takes the largest of **every** page rather than the shown one — a pager that resized when it flipped would be a worse answer than no hug. It is asked of the whole **subtree** (a plain panel nested in a hugging one is measured too, so a toolbar of grouped buttons needs the prop once), and `None` propagates: a container holding a plane, a lane or a heavy view hands that axis back to the layout, because a hug is not a promise that everything under it can be measured. `scroll` is deliberately not in the set — a workspace is a viewport onto a content area it already names with `content_w`/`content_h`.

  **The window half is the one asymmetry, and it is a platform truth rather than a fork**: `hug` on a root sizes the OS window to its content on the axes that settle, keeping the declared `w`/`h` on the others (`Host::window_size`, the one door both the shell and its tests read). A page has no window to size — the element owns its box and reports its pixels, which is what keeps the host from ever reading the DOM — so a mounted GuiDef lays out in the box it is given and only the containers inside it hug. The composition is identical in both builds.

  **What the eye pass added, and it is the milestone's own lesson.** Fitting `gui_bind`/`gui_standalone` to one knob exposed the two things a paper design does not: a **composite** control has to be fitted to *all* of itself, and a size measured in one table cannot be drawn in another. The knob is a label strip over a disc over a value read-out — one element, three drawings, one cell — so a hug that answered the disc's diameter ellipsized the name to `fr…` and pushed the number against the edge, which is parts of a widget being cut to fit the widget. Its `hug` is the widest of the three now, and the read-out's term is measured over `min`/`max` (`controls::readout_w`), never over the value, so the width is stable while the knob is being turned — the mutation-point rule again, on a control rather than on a menu. The knob's *natural* size stays elastic in width, which is not a contradiction and is the clearest statement of why the two functions are separate: elastic is the right answer to "how much of the row do you want" and the wrong one to "how big are you".

  The second was a **units** bug the rule this crate already carries would have predicted: a window is created before anything knows what display it landed on, so its size is measured in the logical table, while the resolved table snaps every role to a whole pixel — at 1.25 the two disagree by a pixel or two, which is exactly enough for a label measured one way and drawn the other to ellipsize inside the box that was supposed to fit it. So a hugging window is asked **twice**, the same shape the shared gutter already had: `Host::window_size` is the logical estimate the window is created with, and `Host::window_size_px` is the exact answer in physical pixels, asked by the shell as soon as it has written the scale and measured with the very table the layout will use. A window that declares its size answers `None` there, so nothing that did not ask to be fitted is ever resized under it.

  Ported in the same commit: `docs/gui-protocol.md` (the prop, and the three widget tables), both builders (`hug=` in Python, `hug?` in TypeScript — on `window`/`layout`/`panel`/`stack` and deliberately *not* on the `ContainerOptions` every plane shares), the Python book's layout section, `docs/architecture.md`, and a `docs/decisions.md` entry for the mutation-point rule. `clients/python/tests/test_gui_props.py` reads the prop on all three surfaces and stays at zero divergences.

- ✅ **K12 — Paint: the chrome capabilities, bounded.** Flat triangles with no antialiasing is the right decision for an oscilloscope and an absolute ceiling for application chrome, so this milestone takes the three that do not cost a batch — rounded corners (tessellated into the same mesh, the way the score's outlines already are), per-widget opacity (the mesh is already per-vertex RGBA; it needs a prop and a resolution rule, not a pipeline), and antialiasing as **MSAA on the window's pass**, one flag with a per-window cost and nothing per widget. Deliberately still out, and recorded so it is not re-argued: textures other than K10's atlas, gradients, drop shadows and per-layer compositing — each splits the batch, and the batch is the crate's whole performance argument.

  **The two props ride the mesh, which is what kept them off every draw function.** They apply to a *run* of triangles — everything one widget contributes — exactly as the clip rectangle already does, so `Mesh` carries an `Ink` (the resolved opacity and the corner radius), the frame sets one per placement beside the clip, and **no draw function grew a parameter and no element learned it is being faded**: every element written before this milestone honours both without knowing they exist. The radius reaches only the **box** primitive (`rect`, and the border built from it), never a line, a disc or a glyph, and each box clamps it to half its shorter side — which is what lets one number ride a whole widget, since a divider, a tick and a track edge have no room for an arc and come out byte-for-byte unchanged while the widget's own frame and its focus ring round.

  **The resolution rule the entry asked for turned out to be two, because the props inherit differently**, and that is the design rather than an inconsistency. `opacity` composes down the subtree like a theme group (a control at 0.5 inside a panel at 0.5 draws at 0.25) — a fade is a statement about a *group* of widgets — and it resolves at the same mutation point, in the same walk, which is why `resolve_themes` is `resolve_style` now. `radius` does not inherit at all: rounding is a statement about one box's shape, and a rounded panel says nothing about the controls in it. Both are style props on the K11 door, both are live, and a **negative** number clears either — the escape an empty `color` already is, since no number in either range means "say nothing".

  **What the fade does not do is stated rather than left to be discovered**: it is per-primitive alpha, not layer compositing, so two overlapping shapes inside a faded widget show through each other — compositing a subtree means a second target, which is the batch we refused to split. And it fades what the mesh carries; a heavy view's picture keeps its own opacity, because reaching it would be a uniform on the waveform, the spectrogram and every user `canvas` shader, three pipelines paying for one prop.

  **MSAA landed as one fact about the pass, and made the pass's contract explicit.** `crate::view::Target` (the format *and* the sample count) replaced the bare format every pipeline constructor took — a pipeline that disagrees with the attachment on either is rejected at draw time, so the two belong together — and `Gpu` owns the multisampled attachment, clamping the asked-for count to what the adapter reports for the surface format and falling back to 1 with a warning rather than refusing to open. It is a **host** setting on all three legs (`--msaa`/`[gui] msaa`, `GuiBridge::msaa`), never a prop: a widget cannot have its own sample count without its own pass, so offering it per widget would promise something the hardware does not do.

  Ported in the same commit: `docs/gui-protocol.md` (the two props and the host flag), `docs/gui-props.md`'s generic list, `docs/cli.md` and the Python book's `[gui]` table (`--msaa`), the Python book's style section, `docs/architecture.md` (a `paint.rs` row it never had), a `docs/decisions.md` entry for the batch criterion and the two bounds, and both builders (`opacity`/`radius` in Python's generic props, in TypeScript's `WidgetOptions`). By eye through `style.py`, which grew the row the milestone is about — a rounded panel whose controls stay square, a faded one whose controls fade with it — and boots its host with `--msaa 4`, since the arcs are what shows it.

### Part C — making it someone else's

- ✅ **K13 — The crate's features: what a build pays for.** It had `midi`, `standalone`, `standalone-faust` and — since K10 — `font-atlas`, and no axis at all through the *catalogue*: a program linking this crate for its controls compiled the engraver, `lyon` and the patcher whether or not any def ever names one. It now has `notation` (the `score` element and `lyon`) and `patcher` (the `plane`-with-boxes graph view), both **on by default**, because a feature exists so a build can *drop* a family and never so it has to ask for one — which is also why `font-atlas` and `standalone` default the other way.

  **The protocol invariant is what made this three files rather than a refactor**, exactly as the entry predicted: a family compiled out drops its rows from `elements::builtin`, a lookup that misses both the table and the registry is `Unknown`, and `Unknown` is laid out and not painted. A slim build therefore degrades the way a host older than the def already does, with no new rule and no new failure mode. `patcher` has the one exception worth naming, and it is a property of the wire rather than of the feature: `plane` is the patcher's name *and* the workspace's, told apart by the presence of `boxes`/`cords`, so without the family that fork is not compiled and a patcher node reads as the workspace it also is — degrading to something real rather than to nothing.

  **What the milestone found is why it exists.** `--no-default-features` did not compile, and had not for some time (its own commit, and its entry under "Found by use"): three `use` lines carried a `midi` gate that belonged to none of them. Then the first matrix run failed twice more — six *tests* that exercise a dropped family, which clippy cannot catch because it type-checks test code and never runs it, and one rustdoc link into a module a configuration compiles out. That third one is the reason the doc build walks the matrix at all, and it is the same lesson the root crate already wrote down: a doc comment naming an item across a feature seam names it in **backticks**, never as a link.

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

  **The cost this track was told it could not design away turned out to be the wrong cost**, and the correction is worth carrying because it changes what this milestone has to build. The entry it referred to (root `PLAN.md`, "Found by use": *a sample write costs the whole buffer*) read `/buffer_setRange`'s whole-buffer replacement as an asymptotic defect an editor would pay per gesture — measured 2026-08-15 at 33.8 ms a write on a five-minute take, against the ~100 a second a stroke emits. **Both halves of that dissolved under measurement** (root `PLAN.md`, **S12**). Hearing a stroke does not go through the take at all: what a stroke has to sound is the **span it just drew**, and a scratch buffer that length sustains 443 writes a second — the route D4 already takes when it plays a copied block out of a buffer of its own. And the edit itself does not go through the pool: it is an NRT job over the working copy, and a take's buffer is replaced whole exactly once, on confirmation, which is the correct operation at that moment rather than a cost. **So what D1 emits is an intent applied against a buffer of the interactive NRT mode** (root `PLAN.md`, S13), not a write to the RT pool, and the RT buffer a clip plays from is downstream of the confirmation — which is also why S12 and S13 come before this milestone: they decide what applies the intent and where it runs.
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

- ✅ **A list's spectrogram kept beside its peaks** *(named 2026-10-22,
  deciding how a list's spectrogram is drawn)*. A list's spectrogram is
  analyzed from its samples over the span shown, which costs the view's width
  times the window on every move past what it holds. A stored analysis beside
  each file's peaks (columns at a fixed hop, with levels that keep each bin's
  largest, as the time pyramid does) would draw a zoomed-out view with no
  sample read, composed where a part keeps its file's hop and analyzed from
  the samples only across a seam, a fade or a resampled part. Take it only if
  the measured cost of the analysis asks for it: over an hour today it is
  87 ms natively. *Half of it since 2026-10-25, in memory:* what asked for it
  was not the cost but an edit's picture, which waited for the span to be
  analyzed again and so did not follow a cut or a paste. The answerer keeps
  each file's columns at a grid of the view's hop over the file's frames and
  composes a list's from them, analyzing only the windows over a seam, a fade
  or a resampled part (`disk::answer`, `docs/decisions.md`). What is left is
  this entry's other half: keeping them on disk beside the peaks, with the
  levels that keep each bin's largest, so a file opened again, or zoomed out
  past what was analyzed, reads no samples. *Closed 2026-10-25:* the summary beside the
  peaks is `clausters_core::spectra`, written by `PeaksOut` in the render
  that writes the peaks, natively and in a page; a column of the view at
  least a summary column wide is folded from it and reads no sample, and every
  column is the largest of the windows it stands for (`docs/decisions.md`).

- ✅ **The multitrack application is written twice: once in the crate and once in the host** *(named 2026-09-12 by the user; closed 2026-09-13)*. The standalone host is now an endpoint of the one application rather than a second copy of it: it plays the piece through `nodes::plan` + `Instance::reconcile` (`host/instance.rs`, which replaced `document/sound.rs`), pushes the projection's whole picture back, asks the projection which tags are the piece's, and realizes a minted join through `clausters_editing::sources::stitch` -- the one reading of a segmented source, bound by both clients as `editing_stitch`.

- ⬜ **A paste that needs tracks could make them** *(named 2026-09-12, with the fix above; **the refusal is the decided behaviour**, confirmed by the user the same day -- "if there are no more tracks it is a refuse" -- so this entry is an alternative somebody may want and not a defect waiting)*. A block taller than what is under the selected track is refused today, which is Pro Tools' and Cubase's answer and the honest one for a widget that owns no piece. **REAPER's** is the other: it creates the tracks the block needs and pastes whole. This widget already has the verb — a double click on a header adds a track — so what stands between it and this is not the capability but the **report**: adding tracks is a `lanes` payload and pasting is a `clips` one, and a paste that did both would be the first gesture here to leave two. That is a decision about the conversation and not about the multitrack: whether an owner may be handed a run of payloads from one gesture, in order, as one entry in the history. Worth taking with the audio editor's own paste, which will ask the same question about panes.

- ✅ **The structures come out from under the drawing** *(named 2026-09-12, in the consistency pass over the host)*. `elements::multitrack` imports `graphics::multitrack as model`, and the verbs over notes -- split, join, paste, copy, quantize, remove -- live in `graphics::pianoroll`. The module name says drawing and the contents are a structure, its edits and its picture, in that order of importance and in the wrong module. Nobody looking for where a note is edited looks under `graphics`.

  It matters more than a rename usually does because of what is coming: three applications over the **same structures**, and the structures are the part they share. A layer that names them -- boxes on a time axis, notes, points, samples, segments -- with the verbs beside them and `graphics` left holding only what puts pixels down, is what makes the audio editor's stack of layers a composition of things that already exist rather than a fourth copy of them. The precedents are already in the crate and are the argument: `placement` is one geometry for a note and a clip, and `graphics::selection` draws one sweep for five views because the difference between them turned out to be a value and not a type.

  **Done the same day**, and it is four modules. `host::structures` holds `boxes` (`placement`, moved: it was already outside `graphics` and it is the same layer), `notes` (the note and a roll's list verbs), `clips` (a lane, a box on it, a curve over either, and the payload each list reports as) and `points` (a break-point, its value axis and the edits that move one). `graphics::{pianoroll, multitrack, bpf}` keep exactly what they are named for: where a pitch is drawn, which note a pixel is on, how tall a row is, where a value lands on a body.

  **The boundary is a rule and not a taste, so it is stated negatively and can be checked by reading the imports**: nothing under `structures` names a `Rect`, a `View`, a `Draw` or a `Metrics` -- a module that took one would be a drawing again. The one place it nearly broke is worth recording: a curve's value axis needed the meter's `fraction`, which lives in `graphics::meters`, and what it actually needed was the `viewport::Axis` underneath -- so the structure asks the axis and there is still one implementation of the rule.

  The tests moved with what they test, which is where the split was read off in the first place: a test that drives an edit through an `Axes` is the drawing's, and one that calls the verb is the structure's.

- ✅ **`Element` is one trait where it wants to be a few facets** *(named 2026-09-12, same pass; the shape was put to the user as a choice and this is the one they took)*. The trait is **75 methods** over 19 implementations. Most are defaulted and most elements take the defaults -- the median element overrides about ten -- but the signal element overrides 49, and the shape of the rest is a long tail of narrow capability questions: does this navigate time, does it navigate frequency, does it draw a readout under the pointer, is it a bare surface, can it hand over a block of samples, what is its summary bucket.

  That is already better than what it replaced (a leaf used to be an arm in each of eight passes), and the doors are honest -- each one exists because a pass needed an answer and asking *which element is this* was the wrong question. What it has grown into, though, is a single interface that every element must read to implement any of it, and a place where the audio editor's layers would add another dozen methods that mean nothing to a knob.

  The shape to take is **facets**: the trait keeps what every element answers -- set, draw, size, value -- and the rest moves behind `fn as_samples(&self) -> Option<&dyn Samples>` and its kind, one per question a pass actually asks (samples, a time axis, a measured axis, gestures, bulk, voices). A pass then asks for the facet it needs and gets `None` from everything else, which is what it does today method by method; what changes is that an element implements the facets it *is*, and a new family of layers adds a facet instead of adding to everyone's interface.

  **Two of them landed**, and they are the two the list above is mostly made of: `Samples` (nineteen methods -- the shape, the buffer, a sample's value, a block of them, the writes, the summary, and what the view is asking for next) and `Measured` (four -- the axis in hertz an element carries alone). `Element` is **53 methods where it was 75**, and the twenty-three that left were being inherited and declined by every leaf in the catalog. The doors are `Element::samples`/`samples_mut`/`measured`/`measured_mut`, with `WidgetKind::as_samples` and its kin beside `as_element` because that is how every pass reaches one: it has a node, asks it for the capability it needs, and gets nothing from everything else -- which is exactly what it got method by method before. Held by `an_element_states_the_facets_it_is_and_nothing_else`: a label answers `None` once and a signal view answers both.

  **What it cost was small and that is the finding**: two implementors (the signal element, and the multitrack for the one bulk method it answers), and about forty call sites, most of them already going through `as_element()` and now going through `as_samples()`. The refactor being cheap is what says the seam was in the right place; what was wrong was only how much of it every element had to carry.

  **And the other two landed the same day**, once the application split below made it clear which questions were a *view's* rather than a widget's: `OnAxis` (eleven -- how far the content reaches, the band it reserves beside the axis, the rows it stacks, where the axis lies in its rectangle, and the navigation chrome a gesture writes back through) and `Slotted` (five -- what the frame's GPU pass has to upload, hand over and drop). Four implementors and three, against a catalog of nineteen. **`Element` is 41 methods where it was 75**, and the thirty-nine that left were being declined by every leaf that is not a view.

  **The gestures are not a facet, and that is the finding worth keeping.** They read like the obvious next one -- fourteen methods -- until you count the implementors: thirteen of nineteen elements have a hand, because the median element in this catalog *is* a control. A facet earns its accessor where the capability is the **exception**; interaction is the rule, so the hand stays on the trait. `voice` is one method and stays for the same reason from the other end.

- ✅ **A multitrack is an application, and it is written as a widget** *(named 2026-09-12 by the user, closing the consistency pass: the file grew this large because it **is** a small application, it may well grow further, and other applications -- the audio editor -- are coming)*. It is 4650 lines in one file -- the second largest in the crate -- holding a model, its props, its hit-testing, its gestures, its verbs, its clipboard and its tests. Every one of those is there for a good reason and the file is not badly written; what it is, is an **application** that entered the crate through the one door the crate had for a picture.

  The audio editor is the second, the score editor the third, and they share more with this than they differ from it: a stack of things on a shared time axis, a selection over them, verbs over that selection, a hand that drags and trims and splits, an owner that answers. So the refactor when it comes is not "split the file" but **name the general part**: what an application over the document *is* in this host, in a module of its own, with each application left holding only its own -- a multitrack's lanes and headers, an audio editor's panes and layers, a score's staves. `interact` and `gestures` already took this pass and their module docs say what the boundary was; this one is larger and lands after the facets, which is what gives it something to be general *in*.

  Deliberately not scheduled yet: the interaction vocabulary it would generalize is itself provisional ("The whole interaction vocabulary is provisional" below), and generalizing an unsettled vocabulary fixes it in place. What is decided is the direction and that this file is not where it ends.

  **Taken 2026-09-12, and it is the half that does not guess.** The file is a directory split by the **question a reader arrives with** -- the same pass `interact` and `gestures` took, and for the same reason: a reader has one question at a time. `mod.rs` (622) is the type, what it holds and the *table* of what it answers, where a trait method longer than a few lines delegates to an inherent one beside its question; `props` (497) is the wire's lists becoming the widget's own and every payload a gesture leaves, one module because it is one contract read from either end; `rows` (267) is the vertical model and the time axis beside it; `draw` (301) is the composition; `hand` (819) is every phase of a gesture and the state a press claims; `verbs` (293) is what the piece can be asked and what it refuses with; `tests` (2099) stays whole, as `gestures`' does.

  **What was not done, and it is not shyness.** Nothing was hoisted into a module *above* the multitrack, because there is **one application**: a general module with one user is a guess, and the rule this project keeps everywhere else is to extract on the second occurrence. The pieces that already had two -- the box arithmetic and the verbs over a selection (`structures::boxes`), the sweep's drawing (`graphics::selection`), the box geometry -- were extracted when the second user appeared, and that is exactly what will happen here: the audio editor's panes are the second stack of rows, its layers the second selection over a stack. The split above is what makes that cheap, because the seams are now named and a reader can see which of the six the second application would share.

  **And one thing the split found by itself**: the `impl Element` block was 857 lines, so *what this element is* could not be read without reading everything it does. As a table it is thirty.

- ✅ **The node system, specified: which groups a track is, and what a curve
  drives** *(the user, 2026-09-10)*. A specification of the synthesis nodes and
  groups that carry a piece -- **source, pre, post, fx** and the rest -- and of
  the **names** the multitrack's own elements answer to, so that the two ends
  meet: an automation whose target says `gain` must drive the *same* `gain` the
  track's knob shows, and today they are two words that happen to be spelled
  alike. It is the same subject `G35.3 + G35.4` opens (a clip's gain and a
  track's gain are two controls in one chain) seen from the other side: that
  entry builds the chain, this one says what everything in it is called and who
  may address it. Design first, then either absorbs the other.

  *Closed 2026-09-10: specified and built.* A clip, a track and the piece are
  nested `GraphDef`s (`mt.clip`, `mt.track`, `mt.piece` in `clausters_core::mixer`,
  which also holds the port vocabulary), and an automation's target is
  `{"port": "gain"}` resolved against the instance it hangs on by the plan in
  `clausters_editing` — so the header's knob and the curve write the same port,
  which is how `G35.3 + G35.4` closed. What the spec left open is in root
  `PLAN.md`'s Future directions: what an effect is, free routing, sends, a track
  wider than stereo, a silent track's cost, and a join over file spans.

- ✅ **Join over fragments: one box, several sources, sample by sample** *(the
  user, 2026-09-10)*. `G35.11` left join at what touches over one source. What a
  join **is**, though, is a box made of fragments of many files, each played
  over the range its own box was showing -- and that is not something a reader
  over one buffer can do: a `DiskIn`/`PlayBuf` cannot seek between different
  buffers from one sample to the next. So it needs a server-side answer (a
  reader that walks a list of windows, or a render that assembles them into one
  take) before the document can be asked for a vocabulary. Two more cases go
  with it and are not answerable without it either: a **gap** between two boxes
  (silence in the joined one) and an **overlap** (a mix of the two signals).
  Possibly a milestone of its own; it needs thinking through before it is one.

  **What the gap cost before it was closed off** *(found by use 2026-09-11: the
  user cut a take into pieces, pulled some edges to show more, shuffled the
  order and joined them all)*. Join did not ask whether the pieces read one run
  -- it stated one window over the whole span from where the *first* piece read,
  which for anything but the halves of a cut is a different recording than the
  one that was there: it played straight through material the pieces skipped,
  and where the first piece read late in its take it ran off the end and the
  rest was **silence**. So `j` now leaves a run alone unless each box reads on
  from where the one before it stops, over the same source
  (`placement::continues`). That is honest and it is not the feature: what the
  user was asking for is exactly this entry, and the piece of it that already
  exists is `/buffer_stitch` -- a join over fragments **is** a stitched buffer,
  and the server reads one as fast as a plain one. What is missing is the
  document's half: a box that reads several runs needs a content the format can
  state, or the cut is gone the moment the piece is saved and reopened.

  **Asked for a second time, in the smallest case there is** *(2026-09-12, the
  user: "I did exactly the steps I described to you, it should be possible;
  zooming in I see there is no overlap at all")*. Not a shuffled take this time
  but **one cut, two halves, swapped** -- which is the same feature at its
  minimum and settles that this is not an edge case somebody wandered into. It
  also shows what the refusal costs while the feature is missing: the two boxes
  touch exactly, read one source, and show a picture with nothing wrong in it,
  so "these are not consecutive pieces of their source" reads as a quibble
  about the picture rather than a statement about what a box *is*. A region is
  **one window onto one source**; two fragments in an order the source does not
  have cannot be said that way, however they sit on the lane. That is why this
  is a source question and not a placement one.

  The document's half may be less missing than the entry assumed: `SegmentSource`
  and `SegmentRef` already exist and are what the example's `comp` take is
  (`Buffer.stitch`, read as one reader). So the shape to think through is
  whether a join over fragments **mints a source** -- a new `SegmentSource` over
  the spans the boxes show, in the order they show them, the region then being
  one plain window onto it -- which would make the joined box save, reopen and
  play with nothing new in the format. The gap and the overlap cases follow from
  it (a span of silence; a span the server mixes), and so does the crossfade at
  every seam.

  **Taken as a milestone** *(2026-09-12, the user: "do it and record it as a
  milestone done, because it is what had to be done when we made the
  segments")*: `crates/clausters-document/PLAN.md`, `O31` -- a join over
  fragments mints a source, and the source is segments, recorded in the
  session's source table as a `Location`. The server's half, which the same
  message named and this entry had lost, is the root `PLAN.md` entry "A long
  take is played out of the pool, and `DiskIn` cannot be positioned": a part of
  a join must be able to name a **file** span and not only a pool buffer, read
  by a prebuffered stream, because a multitrack's ordinary source is a file and
  the buffer is what *this* server adds.

  *Closed 2026-09-12 as `O31`* (`crates/clausters-document/PLAN.md`): a join mints
  a source made of segments, saved as a `Location` in the session's source table
  and played as one stitched buffer. What is left is elsewhere: a part naming a
  file span rather than a pool buffer (root `PLAN.md`, "A long take is played out
  of the pool"), and what a join does with a gap and an overlap
  (`crates/clausters-document/PLAN.md`, "A join takes its clips' fades, and
  invents none").

- ✅ **The whole interaction vocabulary is provisional, and it is decided after the other two applications exist, not before** *(stated by the user 2026-09-12, while asking for the multitrack example's last three gestures: "these rules I am asking you for may change later. Then, once we have made the other applications, we will have to refactor and define many things about the interaction, things that are not decided")*.

  **This is the frame every entry below sits in**, and it is written here once so no individual gesture has to argue for itself. The multitrack is the first of the three applications over one document; the audio editor and the score editor are not built. What a hand does — which key, which modifier, which button, what a double click means, what a verb makes rather than asks about — is being settled **by use, in one application**, and a vocabulary settled in one application is a vocabulary that has been tested against one third of the problem. So these rules are the current answer and not the design, and the refactor that replaces them is expected work rather than a failure of this one.

  What that means concretely while it lasts:

  - **A gesture is written where it can be changed, and written down where it can be found.** In the host, in the element that performs it, with the reason in its doc comment and the rule in `docs/gui-protocol.md` — never spread across a client, never a convention two clients each implement.
  - **A facility says it is one.** A verb that exists because the example needed something to try (the `A` toggle making a gain curve, the wheel's two modifiers, the double click that adds a track) carries that sentence in its own documentation, so the next reader does not mistake a scaffold for a norm.
  - **What is deliberately not decided yet**, and what the other two applications are expected to move: which keys the verbs are bound to and who owns the binding (`G36`); what a modifier addresses — a place under the pointer, or a thing — which this year already produced one defect in each direction; what a track may automate, and how one is chosen, where today one button makes the one curve every track has a port for; how a fade *inside* a box is drawn, which the join's seam and the gap and overlap cases all wait on (`crates/clausters-document/PLAN.md`, "A join's crossfade"); and how much of any of it is the **application's** rather than the widget's, which is the question the other two applications exist to answer.

  It does not open as a milestone: there is nothing to build until there is a second application to disagree with the first. It opens the day the audio editor or the score editor wants a gesture this one already spends.

  **Decided 2026-10-06, with the audio, multitrack and notes editors all built** (the user asked for the common and the specific vocabulary to be defined now that the three exist, and for their menus to use it). What came out, and where each part lives:

  - **A verb names one act in every window**, whatever the window holds. The common words are the host's `Verb`s plus the history's and the window's: `undo`, `redo`, `cut`, `copy`, `paste`, `delete`, `select_all` (new: Ctrl+A, every box, every note, the whole take), `view_all`, `play`, `to_start`, `to_end`, `loop`, `close`, `keys`. Each application adds its own: the audio editor `save` and `mix`; the multitrack and the roll `split`, `join`, `quantize`; the multitrack `add_track`, `reset_heights`, `compact_tracks`, a transport that pauses (`pause`, `stop`) and `save` where the window is the work's only holder. The table is `clausters_apps::chrome`, once, for all three.
  - **A menu entry, its key and its tool are one command**: a bar entry holding no state is performed as its key would be, and a tool is a `button` with a `verb` (it takes no focus, and a press on it leaves the focus where it was). Both address the focused element, then the window's `main` view, then the window, and the owner hears what nobody performed -- one dispatch (`Gestures::command`).
  - **What has a gesture and no meaning without one stays the hand's**: moving a box, trimming it, a fade, a break-point -- in no menu. **What is about one thing under the pointer is the element's own context menu** (`Element::context_menu`, asked at the point): the multitrack offers, over a clip, its curves, over a track, the track's -- each shown or hidden alone (`ShowAutomation`, recorded), as the user asked for both -- and the heights of the rows as a whole (reset, compact).
  - **Which keys** stay `G36`'s table; **what a track may automate** and how a curve is chosen are still open, now in their own entries rather than under this one.

Captured here so the depth the editor-grade vision needs is not lost; each becomes a milestone — or a track — when its design converges. **The convention, stated because the list only works if it holds:** every entry carries a `⬜`, and one that converges into numbered milestones **leaves this list** — it is not marked `✅` here, because converging is not shipping and a `✅` beside an unstarted milestone says the opposite of what is true. The milestone that absorbed it is the record, and it says where it came from, so nothing needs a pointer that would go stale. Gone that way so far: scopes, the editor-grade views, edit-back-to-data and the BPF view (G18-G21), and — 2026-08-13 — the **DAW / timeline view**, which became the whole of G22 with its dedicated note view following as G24 — while the free arrangement plane it named alongside the lane stack **stays here**, since it is a second kind of multitrack whose model differs structurally from the one that shipped and has still to be defined, not an increment on it; and **a sample as a grabbable point**, folded into the D track (D1, D2), which is where the questions it raised are answered.

**Ordering — what is left of it.** The original decision staged the widget-deepening arc first, the timeline and notation views after it, and packaging plus the in-browser audio engine deliberately last. Three quarters of that is spent: the deepening arc landed (G18-G21), the timeline converged as G22 and notation as G31, and the engine shipped as the server's B track — which also settled the `Transport`/`ServerLink` seam the ordering was keeping open, since `ServerLink::Page` is that variant. What still binds is the other half of the same sentence: **packaging stays last**, because it changes how the system ships and not what the system can show, and it keeps constraining design in the meantime — the web frontend must stay Tauri-wrappable.

- ✅ **A clip carries one automation, and the definition asks for several** *(named 2026-08-18 closing the clip's interaction rules — the user's statement of what a clip holds says "several automations can be superimposed in the same space, as lines of different colors", and only one is editable at a time; closed the same day with the wire change the join needed)*. The interaction half was already general — a layer is addressed by its position in the stack, `points:1` is the name a second curve answers to, and one-edited-at-a-time is what makes several of them legible. What was missing was that a clip's contents were **props** (`points`, singular), so a second curve could not be *said*.

  **A clip's contents are children now**, one node per body, and its flat props are the shorthand for one of each. So two automations are two `curve` children, each with its own `color`, drawn in declaration order and named `points` and `points:1` — and a body being a node is also what gives it a colour to carry, which the prop form had nowhere to put. The same change is what lets a take be several segments, which is the other half of the question: it was one missing thing, not two.

- ✅ **A view of a server buffer has two ways of getting its material, and what chooses between them is the wrong question** *(named 2026-08-19 by the user, on reading H9: "why are there two different kinds of view to do the same thing?" — and it was not marked here before they asked, which is the part worth recording)*.

  **What is true.** There is one view and one `WaveformData`: the drawing has never had two forms. What has two forms is how the material arrives.

  - **The whole buffer** (`Bulk::Buffer`): `/buffer_query` then `/buffer_getRange` in chunks from 0 to the end. It gives the summary *and* every sample at once, so the view zooms to the sample from the first frame and asks nothing afterwards.
  - **Shape, summary, spans** (`Bulk::Recording` + `/buffer_stream` + H9's `want_span`): the length up front, the overview as it is written, and the run under the eye read back when a zoom goes past the summary.

  **Both are defensible, and the split is a cost threshold**: a short buffer is one round trip and then no latency at all, while a long take cannot be downloaded (a ten-minute stereo take is 230 MB, and the page's own bulk read already fails above one chunk — `clients/web/PLAN.md`). What is **not** defensible is the criterion. Today the fork is `fills` — *is this being recorded?* — which has nothing to do with either cost. Two views of the same finished take, one opened before it was recorded and one after, take different paths and behave differently under the same hand.

  **What it should be**: the second path for everything, and the first kept only where it is cheaper. That is one condition — *can this material be summarized without reading it all?* — and it is exactly what the server cannot answer yet: an overview arrives free only from `/buffer_stream` (a recording) or from a cache file a client wrote. **So this is blocked on the root plan's "A take's overview could be a file beside its region"** (or on whatever else lets a client ask for a summary), and it is written down here rather than half-built, because changing the fork before there is an answer would only move the arbitrariness.

  **What it would leave**: `fills` back to being what it says — *this material is being written, draw only that far* — and not a chooser of transports; the whole-buffer download as a size decision the host makes; and one behaviour for a view of a buffer, whatever it was doing when it opened.

  **Done 2026-08-20, and the entry's own framing was half wrong** — the user said so on reading it back: the fork is not a duplication to be removed, it is two costs that are cheap in opposite cases, and the data path is better for having both. So both routes stayed and only the **criterion** moved. A short buffer is still downloaded whole: one conversation, and then every zoom is answered out of the samples in hand with no round trip ever. A long one is drawn from its summary, which is now possible for a take that stands still and not only for one being recorded into — `/buffer_peaks` answers what `/buffer_stream` pushes, the same blob through the same `write_buckets` — with the span under the eye read back as the eye moves. The line is the **size**, drawn once where the shape is first known (`fetch::BufferFetches::whole`, on `/buffer_query.reply`, at about ten seconds of stereo), and `fills` now says only what it means: these samples are being written, so they are not there to fetch. That is what fixes the thing that was actually wrong — two views of one finished take, one opened while it recorded and one after, no longer take different routes.

  A long take's summary arrives over several replies, and the walk keeps **no state**: each answer says where it began, its own length says how many buckets it held, and the tree says how long the take is (`host::next_peaks_frame`). An unanswered request stops it, leaving the picture with the buckets it already has.

  It is written down in the two places a reader would look — `docs/architecture.md` (the host's bulk path) and `docs/gui-protocol.md` (under `fills`, saying what it does *not* decide) — because the question this entry records was asked of the code and the code did not answer it.

- ✅ **A column is its own quad, and a signal that steps shows what that costs** *(named 2026-08-20 by the user, on a lane drawing a square wave: "it does not draw vertical lines and the horizontal ones are not straight" — deferred the same day, to be looked at another way rather than patched)*. The trace is **one quad per pixel column, from that column's lowest sample to its highest**, and nothing joins one column to the next. Measured on the lane that raised it, at a minute across 1250 columns (2304 samples a column, nine buckets): of eleven transitions, **ten** draw as a tall column — the column holds both levels, which is what a vertical *is* here — and the eleventh draws none, because it falls on a column boundary and each side is a separate quad. And 1239 columns come out flat to within a hundredth of full scale: a constant run has no height at all and is inked at the trace's minimum weight, so a value that drifts by less than a pixel per column reads as a line that steps.

  Both are the rule working as written — **the picture is a measurement of the column, and the minimum ink keeps a flat stretch visible** — and both are only ever seen on a signal that is exactly a step, which is why they surfaced on a synthetic square wave and not on audio. What is open is whether that is the right rule at a *discontinuity*: the signal does pass through the values between two columns, so a trace that stretched each column to meet its neighbour would be drawing something true and would close both gaps at once. That is a change to what a column claims, so it is a design and not a fix, and it is the one the user asked to come back to.

  **Answered 2026-08-20, and the answer is yes — the stretch is what a column claims now.** The design this entry describes is exactly the one that shipped as the join (`clausters_core::peaks::join`, reached by every drawing): a column is inked over what it measured, **extended to reach the column before it**, which inks the values the curve takes while it crosses the boundary and nothing else — the same segment the polyline draws a zoom later. Overlapping columns are untouched, and the extension is drawn and never remembered, so a run cannot walk the trace outwards.

  **Re-measured on the lane that raised it**, as a test rather than a claim (`every_transition_of_a_minute_long_square_wave_is_drawn`): a minute across 1250 columns at 2304 samples a column, eleven transitions, of which one falls on a column boundary. **All eleven draw a vertical now** where ten did, and the picture no longer changes with where the zoom puts a boundary. The 1239 constant columns still have no height and are still inked at the trace's weight, and that half stays as it is: it is what keeps a flat stretch visible, and it is a measurement rather than an artefact. What the join *did* change there is that a run of them is connected — a value drifting by less than a pixel a column is one line rather than a row of bars with rounding gaps between them (`a_drift_of_less_than_a_pixel_a_column_still_draws_one_line`). What it does not give is slope, and cannot: below a pixel a column there is none to draw, which is the "never resolve finer than the screen" rule and not a defect of this one.

- ✅ **A zoom asks for samples where what it wants is a finer summary** *(named 2026-08-20, out of the third eye pass over a page drawing server buffers: the fixes that day made the traffic correct and left the shape of it wrong)*. A view zoomed past its summary fetches the **samples** over the span it shows and computes each column's min and max from them. That is right for a host that maps the take — the samples are already under its pointer — and it is the wrong request for one that cannot: the span is a few hundred kilobytes, the carrier is a 64 KiB ring, and what the drawing actually needs is one number pair per *pixel column*, which is a few kilobytes.

  `/buffer_peaks` already answers exactly that shape — an overview at any bucket — so a zoom could ask for the summary **at the column's own resolution** and have its picture in one reply instead of a few dozen. What stands in the way is that a view holds one pyramid at one `base_bucket`, and a report at another bucket cannot be folded into it (`write_buckets` refuses, correctly). So the view would need a *detail* summary beside its own, over a span, the way `Samples::Window` is a run of samples beside the whole: a second grid the fine regime prefers where it covers, with the samples themselves left for the zoom below which no summary is worth asking for.

  It is a design rather than a fix because it changes what a view *is*: `waveform::Channel` would carry a span-limited pyramid, `covers` would answer for two grids, and the regime picker would gain a rung between "the summary" and "the samples". What it gives is the thing the carrier cannot: a zoom that resolves in one round trip on any client that cannot map the take.

  **Done 2026-08-20, and it is the three changes named above.** `waveform::Detail` is a pyramid over a span beside the channel's own (`set_detail`, `detail_covers`, one grid at a time per view — replaced when the eye moves, like the window beside it); `measure` gained the rung, reading whole buckets out of the detail grid on its own axis through the same fold the summary uses; and the draw pass now leaves an `Owed` rather than a span, saying which *shape* would settle it. On the wire it is `/buffer_peaks` with the span named, which the command already answered — nothing new was needed there, and a reply is told from the summary walk's by its **bucket**.

  **The crossing between the two requests turned out to be arithmetic rather than taste**: a bucket is three floats where a sample is one, so a grid only pays where a bucket holds well more than three samples — at four it carries three quarters of what it describes, at sixteen a fifth. With a column holding two buckets or more (so the fold's position error stays under half a pixel) that puts it at about **thirty-two samples a pixel**, which is also roughly where the trace stops being columns and becomes the polyline. Below it the samples are asked for and kept, because they are exact and answer every deeper zoom too. The grid's bucket is a power of two so a zoom holds still — it answers every column from its bucket upwards, so zooming out is free and zooming in asks again only after a factor of two — and it is coarsened until the whole span fits one reply, since a grid is replaced and not extended.

  Both legs took it together, and `recording-host.html` asserts the fork by what the host says it did: a zoom between the bucket and the sample logs a **grid** and no `read back`, and the deeper zoom beside it still reads the samples. The record of why is in `docs/decisions.md` ("A zoom past the summary asks for a finer summary, and only then for the samples"), with the shape in `docs/architecture.md` and the two commands under `fills` in `docs/gui-protocol.md`.

- ✅ **Time-stretch: an edge that changes the material rather than the window** *(named 2026-08-18 with the clip's window, which is what made it nameable; moved 2026-10-04 by the user to the multitrack application, `crates/clausters-apps/PLAN.md`, Future directions, where it stays open)*. An edge drag is a **trim** and the material stands still; a multitrack's other answer is to change the material's *length* — resampling it, or stretching it at pitch — which is a **rendering on demand** (an NRT pass over the source, its result a new take) and not a placement edit. Everything it needs is now stated: `fit` is the prop that draws a placement's span over the whole of its material, which is exactly the picture such a stretch produces, and the window says which segment is being stretched. What is undecided is the rest of it: which gesture asks for it (a modifier on the edge, a verb like the split's), what the client does with the result (a new source, or a source that remembers it was stretched), and whether the drawing shows the stretch before the render lands. Deliberately not part of the interaction rules: they had to say what an edge drag *is*, and they do.

- ✅ **The audio editor's layers: the view is the next container, and it is the richer one** *(named 2026-08-18 by the user while the clip's rules closed, in two notes: "the concept also holds for audio editing", and then what that means in detail — this is a design to think through and build, not a gap to patch)*.

  **The mechanism is done and general**: one layer edited at a time, the stack read off whatever children fill a role, the active layer a field of the `Widget` node, `hidden` for what is drawn — none of it names a widget type, and a `clip` proved it. What the audio editor needs is the **contents**, and the user's description says they are of two kinds that the design already separates and must keep separating:

  - **Visualization layers** — what the *same* material is drawn as. Sonic Visualiser is the reference: one file seen in several widgets with different presentations, or in **one** widget alternating between them, or **superimposed translucently**. Time domain and spectral domain are the first pair (and spectral *editing* needs the spectrogram layer to be the one on top); after them, a MIDI-note layer over a take, markers, and whatever a later analysis draws. Several drawn at once, in an order, with opacity — which is what `hidden` and the stack's order already are, plus a per-layer alpha the node's `opacity` may already answer for.
  - **Edit layers** — what a hand is doing, and one at a time. The one the user names first is **non-destructive processing**: a dynamic envelope over a take, and envelopes of other parameters beside it, edited as curves over the waveform — *see what the wave becomes, hear it, and apply it*. That is three states of one layer (drawn over the material, previewed against it, rendered into it), and the third is where it meets the render-on-demand question time-stretch also lands on.

  **The two combine and are not the same axis**: certain edits are only meaningful on certain visualizations (a spectral edit on the spectrogram), so an edit layer may *require* a visualization layer to be shown — which is a rule between the two stacks, and the reason they stay two.

  What has to be decided before any of it is built: whether a view's contents become bodies of the container the way a clip's are (which is what would let the whole rule apply unchanged, and what `at`/`dur`/`start` on a body already allow); what a **preview** is on the wire (a layer drawn from material the host does not hold, since the processed signal is the owner's); and how a `BodyRole` set that is closed and belongs to the container grows to carry a spectrogram, a marker list and a parameter envelope. The visualization half shipped as **X12** in `crates/clausters-apps/PLAN.md`; the pane, shared axis and body routing shipped as **X13**. The gain edit body and its Apply state ship as **X14**. Preview processing remains the separate Effects-in-preview entry in `crates/clausters-apps/PLAN.md`; other application framing remains under **O24** in `crates/clausters-document/PLAN.md`.

- ✅ **The layer stack is one container's, and an audio editor's view has one too** *(named 2026-08-18, closing the clip's interaction rules — the user said the same day that the principle is not the clip's: "the concept also holds for audio editing", and for the **visualization** layers with it)*. `host::layers` was written general on purpose — the stack is whatever children fill a body role, the active layer is a field of the `Widget` node, nothing in the module names a widget type — but the only container that layers editable things today is the `clip`. A dedicated audio editor's view is the next: material, a selection over it, an automation over both, and later a spectral layer, all on one rectangle with one of them in hand. What it needs is not the rule, which is settled and shipped, but the **contents**: a signal view whose automation is a body rather than a widget beside it, and a `BodyRole` set that stops being three (the set is closed and belongs to the container, which is the right shape and the thing that has to grow). **Closed 2026-10-16 as X13**, after the visual stack in **X12** (`crates/clausters-apps/PLAN.md`). The audio editor now composes a `pane`, which owns the axis and layers signal and curve bodies through `host::layers`; `layer` selects the body in hand and `hidden` excludes bodies from drawing and new presses. Existing body roles suffice; the first gain envelope is application data under X14 in the entry above. **O24** in `crates/clausters-document/PLAN.md` records the pane decision and remains open for the other applications.

- ✅ **Many channels are drawn and not yet readable, and a take cannot be created empty** *(both named by the user, 2026-08-16, once a four-channel session opened: the two halves of "a view is made from what the file has")*. **Drawing** scales already — a clip and an editor both stack a lane per channel, whatever the count, and an ambisonic file draws all sixteen. **Reading** them does not: the editor pane is built as tall as its channel count with a cap, so past three or four lanes they thin out, and nothing scrolls, folds or selects among them. That is the design: a pane with its own vertical scroll, lanes that collapse to a summary, or a channel picker — and whichever it is, the *clip* has the same problem in a lane a third of the height. **Creating** is the other half and does not exist at all: a session draws material that came from a file, so there is no way to make an **empty take of N channels** and start drawing into it. The pieces are all there — `/buffer_alloc` makes the buffer, the document already spells a source with no file (`Location::Volatile`), and the write path is channel-addressed since the server's S16 — so what is missing is the gesture and what a session does with a source that has nowhere to be saved yet.

  **The type the reading half needs has landed** *(2026-09-03)*. `host/bands.rs`
  is the vertical axis as one structure -- a roll's semitone row, a
  multitrack's lane and a stacked view's channel row are the same band of the
  same stack -- with a `Uniform` arm that holds nothing and a `Table` arm that
  holds `n + 1` edges, so **rows of unequal height cost 4 bytes each and a
  binary search** (measured: `band` 1.4-2.2 ns in both arms, `index_at` 2.3 ns
  uniform and 6-15 ns tabulated, growing logarithmically). Every place a
  container splits its height into rows now asks it. What is still missing is
  therefore not a mechanism but the two things a mechanism cannot decide: the
  **wire surface** that says which rows are folded (a prop on the stacked view,
  which is both clients and `docs/gui-protocol.md`), and the **affordance** that
  folds one by hand -- and `YAxis` carries a lane count and a lane height today,
  so a stack that is not uniform is where it grows a `Bands` instead.

  **Closed 2026-10-16** (the audio editor's phase 5, user-approved prop).
  Reading: a stacked `signal` takes `folded`, channel indices drawn as strips
  one control tall with their name in the ruler's place; the open rows share
  the rest and are never thinned past three controls, a stack that does not
  fit scrolling under the wheel over its ruler (Ctrl zooms the amplitude
  there). One layout, `host::bands::Lanes`, is what every row-relative read
  asks -- drawing, the cursor readout, a value-restricted sweep, a held
  sample, a y zoom's anchor, the spectrogram's viewports -- and the even split
  is its default. A press on a strip's name reports `"unfold"`; the audio
  editor owns the folded set (`fold_channel`, `solo_channel`,
  `show_all_channels`, View menu entries per channel). Creating: `SoundFile`
  in both clients, `edit(SoundFile.empty(...))`, a Save as form the first
  save opens, and File -> New file.... The clip's lane is not folded: see
  Found by use.

- ⬜ **The free arrangement plane (the blueprint view)** — *a second kind of multitrack, not a milestone of the one that shipped: it shares characteristics with the lane stack and its model differs from it structurally, so it is named here and still to be defined and planned.* A second projection of the same arrangement: the L3 workspace with its x `View` joined to the window's navigation group, so the plane's x axis *is* the composition's time axis (ruler, selection, the playhead sweeping the whole plane as a vertical line — all existing group machinery), while y is **free spatial organization** — clips and visual elements placed like an architectural plan, not locked to lanes. The model (`clausters.form`) is untouched: `offset`/`dur` stay the temporal truth, and y is presentation geometry owned by the driver and persisted by the `Editor` beside the tree, exactly the P1 `"move"` pattern. A concrete `Aggregate` draws as a bounding **zone** (visual containment as the aggregate's projection) instead of a lane; the classic lane-stack multitrack becomes the *constrained* case of the free plane, the same general-to-constrained pattern L3 fixes for scrolling — which is the direction of the relation and the reason this cannot be an increment on G22: the general case is the one that is missing. One boundary to hold when this opens: **y means nothing to the render** — not pitch, not bus, not bounce order; presentation only, stated so the view never implies semantics the engine does not have (any later mapped-y is an explicit decision, not drift). The honest precedent is the graphic score — **a UPIC whose material is the multitrack's editing structures**: not freehand curves but clips, groups, snap, selection and the edit-back payloads, placed on a free plane (IanniX is the other kin), and the whole of what "free" means there is what a design has to settle. **What it was waiting on has landed** — G22's clip bodies, L2's placement props and L3's workspace — so what stands between it and a track of its own is that definition, not a dependency.
- ⬜ **A steady goniometer, and what a phase view should read** *(the design half left standing by the ✅ "The goniometer's figure jumped, and the signal was the staircase" fix below — smoothing the source removed the staircase and left the view's own question exposed)*. With the signal smoothed the figure sweeps, and what is left is the view's own: it draws the **newest** window of two taps with no alignment, so consecutive ticks read overlapping stretches and the trace's ends need not meet. Three shapes, and the one that is chosen decides what a phase view *is*. **Align it** — reuse the oscilloscope's trigger on the left channel, which costs nothing new and stands a periodic pair still, at the price of no longer showing where in the signal it is. **Persist it** — the classic hardware look: several windows drawn with a decay, which the mesh's per-vertex alpha already allows and which turns instability into the glow it is on a scope. Or **take a positioned window** (`read_bus_at` reports where a window ends, which is what the retained history already appends by), so consecutive ticks read *consecutive* samples and the phase advances continuously instead of overlapping. The third is the only one that changes what the view reads rather than how it draws, and it is the one that would also survive a slower frame rate.

  **Where it shows is the evidence**: with the signal smoothed, what remains is visible at the *ends* of a sweep, where the figure degenerates into a straight line (mono is one diagonal, anti-phase the other). A line is exactly where a phase error is most legible — any of it opens the line into a thin ellipse — so the residue is the unaligned window and not the drawing's resolution, which is what points at the positioned read rather than at drawing more of it.
- ✅ **The heavy families as features** *(what K13 measured and did not take; moved 2026-10-04 by the user to the applications' plan, where it is an open question of `X6` -- `crates/clausters-apps/PLAN.md` -- since which heavy widgets become applications decides what a build can drop)*. `timeline` and `signal` were named as element families beside `notation` and `patcher`; they are not families in the same sense yet, and the difference is structural rather than a matter of effort. The two that landed are named outside their own modules in two and four places; `signal` is named in **twenty-nine** files and the timeline family in **nineteen**, and not at the edges — `frame/mod.rs` is built around `WaveformSlot`/`SpectrogramSlot`, `view::Renderers` holds both heavy pipelines, `bulk.rs` and the server leg resolve into them, the navigation groups live on the `Host` and the time containers are placements in `layout.rs`. So gating them is not a `Cargo.toml` line: it means the frame's slot machinery, the bulk path and the navigation model each compile with **no heavy view at all**, which needs its own decisions (what a `clip` is with no take; what `Renderers` is when it holds nothing; whether a host with no timeline still has navigation groups) and a milestone of K7's size to carry them. The seam itself is not in doubt — `elements::builtin` → `Unknown` is proven by the two families that did land — and `check-features.sh` is already the runner such a milestone would report through. What it buys is the browser bundle above all, which is where a dropped family is worth real kilobytes. The A and D tracks make it heavier, not lighter: both grow the signal element, so this is measured again when they close.
- ✅ **The in-page shared-memory path — measured, and it does not open.** The browser engine shipped (the server's B track) and it talks over a **MessagePort**: OSC bytes both ways, commands crossing into the engine through the same in-memory ring the native embed uses. What did *not* ship is the half that would make a browser host look like the native one — a **`SharedArrayBuffer`** carrying the zero-message `BusSource` (control buses read each frame) and the bulk audio path *inside* the page, the roles `host::shm`/`mapfile` play natively. In a tab those still take the WS/`fetch` fallback built for a *remote* server, even when the engine is in the same document. It was left out **by requirement, not by omission**: SAB needs COOP/COEP isolation headers, and an embeddable component cannot demand those of a page it does not control (`docs/decisions.md`). That is also what makes it a **track rather than a milestone** — it is an optimization behind a build: a page that can isolate itself opts in, every other page keeps the MessagePort path, and both have to stay correct, which is a second backend for one seam and not a redesign (the ring seam was shaped to accept it).

  **The number is in, and it argues the other way** *(measured 2026-08-20; the profile is `clients/web/tools/bus-stream-profile.html`, run by `clients/web/tools/profile-bus-stream.sh`)*. The page builds N canvases, each a window with a control-rate meter and a control-rate scope on two buses of its own and an LFO per bus, and measures the same page three times over: streaming, with the subscription cancelled under the host's own peer tag, and streaming again — the same canvases ticking and drawing throughout, so what the phases differ by is the stream and nothing else. At **forty canvases (eighty buses)** one `/bus_stream.reply` frame is **824 bytes**, and taking it in — decode plus apply, timed around `bridge.server_reply`, which is the whole of what a shared segment would replace with a load — costs **~29 µs of the main thread per 33 ms frame: 0.09 % of it**, about 25 kB/s. At sixty-four canvases the frame is 1304 bytes and the cost is the same 28 µs: it is per-call overhead, not payload. The page's own frame rate, its event-loop lag and Chrome's blocked-time total do not separate the streaming phases from the cancelled one at all — the lag moves by a few tenths of a millisecond between runs, in both directions. (Headless Chrome on software WebGL, so the *drawing* is a floor rather than a forecast; the stream's own cost is CPU work either way and carries over.)

  So the trade-off has a price now and it is not close: SAB would give back a tenth of a percent of a frame, for COOP/COEP that an embeddable component cannot ask of a page it does not control, plus a second backend for one seam that both halves have to keep correct. **It stays shut**, and this entry is answered rather than pending — it reopens only on a page profile that shows the messages costing something, which is a measurement and not an argument. What the profile *did* find is a wall, and it is a different one: past 128 buses the subscription is refused and the page streams a stale subset (see "Found by use" below).
- ✅ **Composed text (IME) cannot be typed into a field.** K6 reads keys and nothing else: the native front takes winit's resolved characters, and in a page a `<canvas>` cannot host an input method at all — so a field takes Latin text and refuses everything a reader of Japanese, Chinese, Korean or a Latin script with dead keys actually types. It is recorded as a limit in `docs/decisions.md` rather than left to be found, and it is a *design* rather than a fix because the answer is not in the host: the page owns composition (an off-screen `<input>` or `contenteditable` fed through `beforeinput`, forwarded to the host as finished text), and the native front owns winit's own IME events — two different mechanisms filling one hole, which is exactly the shape the platform seam exists for. It becomes a milestone the day a field has to hold a name that is not ASCII. **The other half of the sentence has since closed**: text the host cannot *draw* is no better than text it cannot receive, and K9 gave the embedded face lowercase and accents while K10 put any real typeface behind `font-atlas` — so the drawing half is a matter of loading a face that carries the script, and the input half is the whole of what is left here.

  **Closed 2026-08-27**, found by the user typing Spanish into `oscsend` in the
  manual review — which is the day the entry said it would become a milestone,
  and it arrived as "accents cannot be typed" rather than as a decision to
  schedule. Both mechanisms are in, each on its own platform's terms, and the
  entry's guess about which was which was right:

  - **Native** was not a mechanism at all, it was a bug. winit composes through
    xkb and hands the result over in the key event's `text`; the front was
    reading `logical_key`, which on the press a dead key composes onto is the
    *key* (`Space`) and not the character (`` ` ``). It was also asking
    `key_without_modifiers` on every key, which unshifts, so the field held no
    capital either. Two lines, in the commits.
  - **The browser** is the mechanism the entry described: `host::web::compose`,
    a hidden `<input>` per canvas that takes the keyboard **only while a widget
    that takes text is focused** (`Element::takes_text`, the one thing a shell
    now asks about the focus) and gives it back to the canvas otherwise, so no
    gesture, shortcut or ring walk moved. It forwards `beforeinput` for a typed
    or pasted character and `compositionend` for what an input method settled
    on — never the steps in between, since the host has no notion of text that
    is still being decided.

  **What it does not do, and is the honest limit of the browser half**: a
  composition in progress is invisible. A real IME shows its preedit — the
  half-spelled phrase, the candidate list — and this shows nothing until the
  composition ends, at which point the finished text appears at once. For a
  dead key that is imperceptible (one press). For Japanese or Chinese it is the
  difference between an input method and a keyboard that occasionally emits a
  word, so a field is usable rather than good. Showing a preedit means the host
  drawing text it does not own, under a caret it does not own, and that is a
  widget design rather than a shell fix — it is what would open this again.
- ⬜ **Packaging: an optional Tauri desktop wrapper** reusing the web frontend. The rest of what this entry once listed has shipped — the GUI chapter of the client's book (`gui.md` and its pages) and the worked `gui_*` examples — so the wrapper alone is what remains, and it stays **last** by the ordering rule above: it changes how the system ships, not what it can show. **Only a possibility** *(the user, 2026-10-04)*: `clausters-editor` is already a Tauri application, so a wrapper could come from there later if it is wanted; nothing is planned.

- ✅ **Cache lifecycle**: a cache key (source path + mtime + analysis params) and memory-mapping the cache file instead of reading it into RAM. *(Closed 2026-10-04.)* The one cache whose key was short was the summary the host writes beside a `path` (`<path>.<base_bucket>.peaks`): the path and the bucket are in its name, but it was checked only for its shape, so samples rewritten at the same length drew the old picture. It is now reused only when written after the samples, and written whole through a rename. The server's overviews already carry the buffer's generation in their name, and a `cache` the client names is the client's to keep current. The rest of the entry was decided against: a server buffer's region is already read where it lies, a summary is a small fraction of its samples, and a client's `path` is copied on purpose -- a file rewritten in place under a held mapping faults on the next read, in a draw.

- ✅ **Spectrogram scaling**: time-axis mipmaps or tiling for buffers wider than the max texture size; a smoother (interpolating) log resample.

  **Built 2026-10-06**, the three of it:
  - **A transform is no longer as wide as a texture.** A long buffer's hop was raised until its frames fit 8192, so five minutes at 48 kHz asked for at a hop of 512 came out at 1758. What bounds a transform now is the memory it takes -- 64 MiB of magnitudes (`stft::MAX_STORED`), some six minutes at the defaults before the hop rises -- and the card is shown a **window** of it: the run of columns on screen and as much again either side (`spectrogram::hold_for`), uploaded again only when a pan leaves it or a zoom changes the level. Not tiles: one texture a channel, never wider than a card takes, whatever the take.
  - **Mipmaps in time, as a pyramid of peaks.** A stored transform has levels above it, each column the **largest** of the two under it (`Stft::build_pyramid`), and a view draws the level that is between one and two columns to the pixel. Zoomed out, a pixel column used to be whichever two of ten frames the sampler landed between; it is now what happened in all of them, a click included. A card's own mip chain would have halved the frequency rows with the frames, which is why the levels are the transform's.
  - **The frequency rows are interpolated with a curve** where a bin is taller than a pixel (a Catmull-Rom through four rows, in the shader): a log axis spreads its lowest bins over tens of pixels each, and the sampler's straight line between rows drew each as a ramp with a crease at its centre.
  - **Not taken**: an analysis made on demand at the screen's resolution from the samples, which is what would take the memory bound away. A cache holds no samples, and a host does not always hold them.

- ✅ **Migrating the rest of the `Stft` machinery behind `clausters-ffi`/`libclausters`** so the signal code lives once: done for `peaks` and the forward FFT (both now in `clausters-core`, the pyramid reachable over the FFI); the `Stft` windowing/normalization and the inverse FFT (for resynthesis UGens) remain, the latter waiting on the server's `FFT`/`IFFT` UGens.

  **Done 2026-10-06.** The inverse FFT had already moved (`clausters_core::fft::irfft_into`, which the server's `IFFT` calls). What was left was the transform itself, and it is `clausters_core::stft` now: the window and its gain, the normalization over the reference range the spectrum curve uses, the whole-buffer and the rolling transform, the time pyramid and the `CLSG` cache -- the host's `spectrogram.rs` keeps the display (the frequency scale, the decibel window, the colors, what is on the card). **The export had a consumer waiting**: the host has always read a spectrogram `cache`, and nothing but the host's own crate could write one. `clausters_core_stft_cache_size` and `clausters_core_stft_build` (core ABI v84), `stft_cache` in wasm, `clausters.gui.spectrogram_cache_file` and `data.stftCache` in the two clients, checked against each other by what the analysis says rather than by its bytes -- the window is a cosine and the magnitudes a logarithm, and the two builds take those from two math libraries.

  *(The three above are design-level questions the heavy views still leave open. They
  lived mid-plan, under "Open questions in the rendering strategy" and without
  checkboxes, which is where a pending item stops reading as pending; moved here
  verbatim 2026-08-20. Two of the questions that stood beside them have since landed —
  the LOD crossfade in G20 and the `bpf` editor's edit-back in G21.)*

- ✅ **A roll that sounds shows no cursor, and what can drive the line is a
  `Playhead`** *(named 2026-08-30 by the user, from `examples/editors/pianoroll`
  in both clients; rewritten the same day, because the first version of this
  entry said the props are moved by nothing outside the `Editor`, and that is
  not true)*.

  **What already exists.** `Transport` (`clients/python/clausters/gui/transport.py`,
  public and exported) is exactly the widget-agnostic driver this needs: it takes
  a host, the widget ids showing the line, a `source(at)` that starts a pass, a
  tempo, a sample rate and a `to_units`, and it drives `playhead_at` the cheap
  way — one anchor per pass, the host reading the engine's clock every frame. Its
  own documentation says it is the play/pause/stop/locate logic "once, independent
  of which widget it drives". So a widget *can* follow a player with no `Editor` in
  between, and the piano-roll example simply does not do it.

  **What actually blocks it is what the source has to be.** `Transport`'s
  `source(at)` returns a `clausters.seq.Playhead` — `update` reads its `position`
  and `playing` — and the example plays a `Pbind`, whose `EventStreamPlayer` has
  `play` and `stop` and nothing else: no position, no seek, forward-only. **A
  piece played as a pattern cannot drive the line at all**, and no prop on the
  host can fix that. To sweep, the example would have to play the drawn notes as
  a `Timeline` under a `Playhead`, which is the verb the client does not expose —
  "The mapping exists and is private to the `Editor`" (`clients/python/PLAN.md`,
  Future directions), where that decision sits.

  **The question this side owns**, and it survives that decision: whether a
  forward-only player should be able to drive a line at all — a pattern knows the
  beat it is on, so an anchor is computable — or whether the honest answer stays
  "only a seekable transport gets a cursor", and the roll, the waveform and the
  engraved page all say so the same way.

  *Re-read 2026-09-18, after the tempo entry's phases*
  (`clients/python/PLAN.md`, "Nothing fundamental holds the tempo map").
  `Transport` is `PlayheadSync` and holds no tempo, `Playhead` is gone, and a
  `Timeline` plays itself and reports its position -- so a roll whose notes are
  played as a timeline can drive the line today, with no verb missing. The
  question this side owns is unchanged: a pattern player is still forward-only
  and has no position.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

- ✅ **Copying an *element* needs a door the protocol does not have.** The typed
  clipboard has an `elements` kind — a piece of the tree, placed members — and
  nothing builds one, because neither side can. The **host** cannot: it owns no
  composition, so it has no member to serialize, and writing one would mean
  inventing a client's vocabulary for a node's configuration. The **owner**
  cannot either: the clipboard is the host's (that is the whole point — a block
  copied in one window pastes against an owner that never saw the copy) and
  there is no command that puts something on it. So `Ctrl+C` over a clip copies
  the *samples* under the selection, which is honest and is not what the gesture
  usually means.

  What closes it is one door, not a feature: a **copy request** the host emits
  when what is under the pointer is a placement rather than a trace, and a way
  for the owner to answer it by putting a typed document on the host's
  clipboard. Then copying a clip and pasting it — the arrangement's own
  cut/copy/paste, undoable like every other `setmembers` — falls out, and the
  `elements` kind stops being a constructor with only tests behind it. **The
  system clipboard is a separate question and stays out**: what crosses to the
  desktop or to a page is a string, which is why a note block already travels in
  the `text` kind, and bridging it is its own work with its own platform matrix.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

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

- ✅ **An insert does not carry the ties the document was read with** *(found
  2026-09-07, reading `notation/score_editor`'s log during the visual review's
  second sitting; moved here from "Found by use" the same day, at the user's
  call: "it makes no sense for us to be fixing bugs that are things not
  implemented")*. The page engraves clean when it opens. After the first
  inserted note the engraver starts reporting

      [Warning] Unable to match @tie of note 'n24-2', skipping it
      [Warning] Expected @tie median or terminal in note 'n39', skipping it

  and never stops -- 16 of them for one note across the sitting, on seven notes
  in all. It begins on the `-2` ids, which are the bass staff made by
  transposing a copy, and later reaches the typed staff's own.

  **It is filed here rather than as a defect because nothing was broken: note
  entry simply does not maintain ties.** A tie is a reference between two notes,
  an insert changes what follows what, and nothing in the entry path was ever
  written to answer that -- so the warnings are the engraver reporting a
  document it was handed, not a regression in one that worked. Two things it
  needs, and they are the same shape as everything else on this list: an insert
  that knows what a tie means, and a transposed copy that carries references
  naming its own notes rather than the original's.

  A skipped tie is a duration that changes on the page and in what is played,
  and the whole of the evidence today is a warning on stderr that no example
  reads and no test asserts -- so whatever is done here starts by making the
  engraver's complaint something a run can be checked against.

  **Fixed 2026-10-06, and the diagnosis above was wrong in its first half.**
  The `-2` ids are not a transposed copy's: they are the second part of an
  item a barline splits, which the emitter ties to the first and never stores.
  The engraver hands every tie back as an element between two ids, and the
  reader took the one between two parts of *one* item for the item's own --
  so a note split by a barline came back tied into whatever followed it, and
  the next writing put out a tie no two notes answered. That is why it began
  with the first edit, an edit being what reads the engraver's document back,
  and why it never stopped. The reader now leaves the barline's tie to the
  emitter.

  The second half was right, and is settled as a rule rather than per verb:
  **a tie that has nothing to tie into is no tie**
  (`notation::edit::settle_ties`), kept after every operation and after a
  reading -- a note inserted between two tied ones, the second deleted or
  silenced, either moved off the other's pitch, each leaves no tie. And the
  engraver's complaint is something a run is checked against
  (`clausters_notation::complaints`, its log for one document): the tests
  that pin what this layer writes ask that the engraver has nothing to say.

- ✅ **One cursor, it is the transport's, and the content never moves it**
  *(the user, 2026-09-07, dictated after the paste and playhead entries in
  "Found by use" turned out to be two halves of one question; "this may change
  the behavior of the examples, but it has to be done right on the host's
  side". **The host's half done 2026-09-08**; what sounds under the cursor is
  the client's and stays open -- see the end of this entry)*.

  **Superseded in part by `G35.1`, 2026-09-09**: the half that is still true is
  *the content never moves it*. The "one" turned out to be two — a **position
  cursor** a hand places and a **playhead** that is never placed — so where this
  entry says a click puts the cursor and playback carries on from there, read:
  the click places the position cursor, and the music is left alone. The rest of
  the rule below (a time on the clock, monotonic, what sounds under it) stands
  as written.

  **The rule.** A window has one cursor. Its position is a **time on the
  clock**, independent of what is drawn: the content does not move it, does not
  push it and does not define it. It is placed by a click and it advances
  monotonically. What *sounds* is whatever lies under it at each moment,
  entered at the offset its time implies.

  **Placing it**, and the phrase that matters is *regardless of content*:

  - **Stopped** — a click on the time ruler or on a lane puts the cursor at
    that time.
  - **Playing** — the same click puts the cursor there **and playback carries
    on from there**.

  **What sounds under it**, which is where continuous and discrete part company
  (the distinction to write against, not "audio" and "MIDI" — a clip of events
  is discrete however it is voiced):

  - A clip **picked up** from under the cursor stops sounding for as long as
    the drag lasts: it is no longer under the cursor.
  - **Dropped ahead** of the cursor, it sounds at exactly the sample position
    it was placed at, when the cursor arrives.
  - **Dropped under** the cursor — the line is inside its body — its samples
    play **from the sample that time corresponds to**, from the release.
  - A **roll** is the same, except an event sounds only if the cursor passes
    through its **onset**: one already gone past is not recovered, because the
    note-on it needed already happened. A sampled signal can be entered
    anywhere and an event cannot be half-triggered.

  **Ctrl+V**, kept on the same one cursor: the start of what is pasted lands at
  the cursor, playing or not, and while playing it lands at the cursor's
  position *at the moment the paste was made*. Two alternatives were named and
  set aside — disabling paste during playback, and offsetting the items against
  what was copied — because both introduce a second rule for a second state.
  Where a **clip** is pasted onto is the lane question beside it: a lane can be
  selected to paste onto, and a block spanning several lanes lands from the
  first source lane (or the selected one) onto the lanes that follow, clamping
  at the end of the stack or growing it.

  **What the host did before this, so the work was the difference and not a
  rewrite** -- kept as the record of what was wrong, each line answered below:

  - A **lane** already locates on a click — a marquee that never left the slop
    is where the hand pointed (`Gestures::release`) — and a **ruler** does too.
    What is missing is *regardless of content*: `Element` comes first in a
    track's plan, so a click on a **clip** is the clip's and locates nothing.
  - A **roll** locates on nothing. It has no `Locate` in its plan and its grid
    hands an unclaimed press back to the container, so a click on the grid
    background places no cursor. This is the half `Ctrl+V` is waiting on: the
    roll pastes at `self.step`, which only step-recording advances.
  - `locate_at` sets a **drawn** cursor and emits `"locate"`; whether the
    transport seeks is the client's, and `"locate"` is in `NOT_AN_EDIT` — screen
    state. One cursor means the drawn one and the playing one are the same
    thing, which is a decision about where the transport's position lives, not
    a new gesture.
  - Nothing can start a take **from an offset** on a live edit, and
    `Playhead._feed` is a scan: it walks items in order and sleeps to the next
    onset, so a pass's position is an index plus accumulated waits. The rule
    above needs it to be a time, with *what is live at this beat, and how far
    into each* as a question that can be asked at any moment. (See "A pass
    re-cued from the playhead drops the clip the playhead is inside",
    `clients/python/PLAN.md`, which is the same rule from the client's side and
    carries the defect that made it visible.)

  **Done, 2026-09-08 -- the host's half.** The click is now the **machine's**
  and not an arm's: `press` reads where the press landed on the innermost time
  axis (once, where the axis is known, with the ruler strip beside it) and
  `release` places the cursor there when the pointer never left the hit slop --
  after the drag's own arm and whatever it delivered. So a clip, a note, a
  roll's grid, empty lane space and a ruler all locate by one rule, and the
  three copies that each answered a click for one view (the ruler's pan, the
  marquee, the range sweep) are gone. The four steps that already answered for
  the pressed pixel -- `locate`, `marker`, `sample`, `draw` -- cancel the click,
  since none of them is a hand looking for a place.

  The roll turned out to locate already, and not by its own plan: the marquee
  unification had made its grid press a `Marquee` with a lane of its own, whose
  release located. The entry above was written before that landed and read the
  old code. What was missing there was the other half -- **`Ctrl+V` pastes at
  the cursor** (and `e` cuts there), through `KeyInput::cursor`, which is the
  window's cursor asked once in the machine rather than a position each element
  keeps. `Notes::step` survives as what it always was, step entry's own
  position, and stands in on a roll that is on no axis.

  And **the transport's position lives in the group**: `locate_timeline_cursor`
  sets the static cursor *and*, while the transport is running, re-anchors
  `playhead_at` against the front's sample clock (now on `GestureCtx`), so a
  click while playing carries the sweep on from where the hand pointed. The
  `"locate"` event still goes out -- the host owns where the cursor is, the
  script owns what sounds under it.

  **Still open, and it is the client's half**: what *sounds* under the cursor --
  a clip picked up going quiet, one dropped ahead sounding at the sample it was
  placed at, one dropped under it entering at the offset its time implies, a
  roll's event triggering only if the cursor passes its onset. That is `C54` in
  `clients/python/PLAN.md` ("A timeline plays what is under the cursor, and an
  edit reaches the pass that is running"), opened 2026-09-08 when the by-ear
  check of this work found the other half of it: a clip dragged while the line
  is about to reach it is left sounding where it no longer is. *(`C54` closed
  2026-10-03: the multitrack's transport and the event lane do this, and no
  client re-cues.)*

  **Related:** `O24`. This is what a multitrack's transport *is*, and doing it
  before the application is taking it off the application's first day.

- ✅ **Messages are not the roll's to edit, and nothing else edits them** *(the
  user, 2026-09-07, after the markers lane was made read-only: "OSC messages
  (not events) could have another widget with a special timeline, but it is not
  part of the roll")*. A `Timeline` holds three kinds of item and only one of
  them has a pitch: an `Event` is a note, an `OscItem` is `(addr, *args)` and a
  `MidiItem` is raw bytes. The roll is the editor of the first -- its grid *is*
  pitch × time -- and the other two are drawn under it as markers, read-only,
  because the flag is a lossy view: the address as a label, the arguments not
  drawn. So a hand can place a message in time by programming
  (`timeline.add(beat, OscItem(addr, ...))`) and by no other means.

  **What a real editor of messages would be is not a roll, and was never
  designed.** It is multidimensional in a way the grid cannot hold: an address,
  arguments of several types, and a *destination* that differs per item --
  `seq/timeline.ts` already says an `OscItem` on a MIDI port and a `MidiItem`
  on an OSC server are both an error. A widget for it has its own time line and
  its own idea of a row, and the roll is not where it goes.

  Three things worth knowing before it is designed, all found by reading the
  model back rather than by using it:

  - **The two items are not peers in practice.** `OscItem` has a second, sound
    use the roll never touched: `render(Timeline)` bounces through
    `Session.nrt()`, where its `play` puts a timestamped message into the score
    with its arguments intact -- which is what an NRT score is. `MidiItem` has
    no such path (a bounce's destination is a server, so it would fail) and
    **nothing in either client constructs one** outside a single test, though
    it is exported, serialized, dispatched and drawn.
  - **The lane's round trip is by label.** A marker moved or removed is matched
    back to its item by the string it draws, so two messages to one address are
    indistinguishable to it. That is a property of *any* view that shows a
    message by its address, and it is what a real one has to answer.
  - **An item that is not one of the three cannot be written down.** A custom
    object with `play(destination)` plays but `item_data` answers `None` for
    it, so it reaches neither the document nor the editing domain. The
    "timeline-item protocol" is a capability to *sound*, not a way to be *in* a
    timeline, and nothing says so yet.

  **Related:** the design question under it, which the user raised and which is
  not settled -- whether a message is a kind of `Event` (one item type that may
  address) or a sibling of it (three types told apart by a key), which is what
  `item_data`/`item_from_data` implement today. Nothing about `Timeline` or the
  items was changed for the read-only fix, deliberately: the defect was the
  widget's.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

- ✅ **A press on a score means what its context says, not what is under it**
  *(the user, 2026-09-07, after the staff-line fix and the regression that
  followed it: "for the score editor to work correctly we will have to add
  editing rules according to the context a click happens in, and not only the
  specific place or element")*. The host answers a press by asking **what is
  under the pointer** and nothing else: a sounding element if there is one,
  otherwise the tightest box, with one exception now carved out for the staff's
  own lines on a page that takes entry. That is a rule about *place*, and a
  score editor is not driven by place alone.

  The day this was written is the argument for it. Three passes over one press
  — the noteheads' fix, the staff lines', and the regression between them that
  made every slur and dynamic unselectable — were three attempts to say in
  geometry what a musician says in a sentence: *I am writing notes*, *I am
  adjusting dynamics*, *I am looking at what is here*. Each attempt got one
  case right by making another wrong, because the question being answered was
  never the one being asked.

  What a context is, is the open part, and the candidates are not equivalent:
  a **mode** the editor is in (write / select / a tool per family, which every
  notation program has and which is a state the client owns); the **kind of
  thing last touched** (a press after selecting a dynamic means dynamics); or
  the **modifier**, which is the cheapest and the weakest, since it puts the
  context in the hand every time instead of once. Whichever it is, it is the
  *client's* — the host holds no score — so what the protocol needs is a way
  for the client to say what the page is currently for, and for the host to
  resolve a press against it. That is a prop and a rule, not a new gesture.

  **Related:** `N8` again, from the other side — that entry asks which element
  admits which edit, and this asks which edit a press is *asking for* before an
  element is even chosen. And the "Found by use" pair on the hit test is the
  evidence: both are closed, both were correct, and the editor is still not
  what a hand expects, which is what says the missing piece is not another
  geometric tie-break.

  **Resolved 2026-10-06, against what the field's editors do** *(the user:
  "see whether they can be settled by the standards of score editors")*. Of
  the three candidates, the field uses each for one thing and none of them for
  the others:

  - **The mode** is the context of a plain press on a staff, and there are two:
    selecting and note entry. It is the client's state, said to the page as the
    `entry` prop the score editor's `X5` built -- so that half was already
    answered when this was read again. No editor has a tool per family.
  - **The modifier** extends a selection and does nothing else: Ctrl takes an
    element in or out, Shift reaches to it. Also built.
  - **The kind of thing touched** is the context of a *verb*, not of a press:
    everything drawn can be picked, and a verb means what the selection makes
    it mean. This was the missing piece, and it was not in the host at all. A
    slur, a hairpin, a dynamic or a tempo mark selected was "not one of this
    model's items" to the editor, and no verb reached it.

  **Built 2026-10-06**: what is written beside the notes is named after the
  model on the page (`a-slur-2-11`, `a-dynamic-n3`, `a-tempo-n5`;
  `notation::attachment_id`), so a press on it names it and a line drawn in
  two parts is one thing; the editor reads a selection as what each element is
  (`score::selection` in `crates/clausters-apps`), Delete takes away what is
  selected and not the notes under it, and a verb that asks for notes reads the
  ones the selection is attached to -- a dynamic from the palette replaces the
  one selected. The keys follow the same rule: outside note entry Delete, the
  arrows and Escape act on the selection, and in it they are the cursor's.
  What the field's editors do and this one does not yet is in
  `crates/clausters-apps/PLAN.md`, "Future directions": the signs the engraver
  draws from the grid, a sweep that selects, and a menu on what is pressed.

- ✅ **Key bindings per window** *(left open by `G36`, 2026-10-04)*. The key table is the host's, like its theme, so two windows of one host cannot give one chord two verbs. A `keys` prop on a `window` -- or on a subtree, as `theme` scopes a look -- is the next level, and the question it waits on is the one an application inside another asks: which of two applications that both claim a chord answers it. **Built 2026-10-05, with the score editor's note entry**: a window names the key table's **scopes** in its `keys` prop, sub-tables (`[gui.keys.<scope>]`, a nested object on `/gui_keys`) read before the table's own rows, the last named first -- so one chord has two verbs in two windows, and in one window from one mode to the next (`docs/gui-protocol.md`, "The keys are a table"). A subtree naming scopes, and which of two nested applications answers a chord they both claim, are the open part of `crates/clausters-document/PLAN.md`'s "An application inside another", where nesting is decided.

- ✅ **A key in a tip** *(left open by `G36`; done 2026-10-07)*. A menu entry shows its chord because it names a verb; a tip belongs to a widget, whose report is its value. A tool button that *is* a verb (`verb` beside its `tip`) would let the tip show the key and the key press the button. *(Done: the tool button has existed since the editors' chrome — a button with a `verb` performs it as the key does — so what was left was the tip. `Host::tip_at` appends the first chord the key table binds to a tool's verb, in parentheses, read as the tip comes up, the way an open menu reads its entries'; a verb no key performs shows the text alone. The editors' tools had written their keys into the tips by hand (`Loop (L)`, `Play or stop (Space)`), which a rebinding made false; they state the text only now.)*

- ✅ **A tool that latches states its key by hand** *(left 2026-10-07 by the entry above; done the same day)*. The score editor's note entry, play and loop tools are a `toggle` and plain buttons whose owner reads their value, not buttons with a `verb` — a latch shows a state, which a tool has none of — so the host adds no key and their tips still write one (`(N)`, `(the space bar)`, `(L)` in `crates/clausters-apps/src/score/tools.rs`), which a rebinding makes false. What it waits on is the half the entry above set aside: a widget that reports a value and is also pressed by a key, which is a `verb` on a `toggle` or a tip that names the verb whose key it shows. *(Done, the second way: a widget names the verb in a `shortcut` prop, which the host performs nothing for and reads the key of, exactly as it reads a tool's `verb`. A `verb` on a toggle would have made the latch a tool, and a tool reports nothing — the score editor reads those three by their value. The three tools name `entry`, `play` and `loop`, and their tips state the text only.)*

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

- ✅ **A capture of the multitrack example's window never answers, headless**
  *(found 2026-11-10, in a pass over the notes editor beside the
  multitrack)*. `editor.capture(path)` on the window
  `examples/editors/edit_multitrack.py` opens, run under a headless host,
  gets its `/gui_info` and then no `/gui_captured` within 60 seconds, and
  `scripts/look.py` on that example times out at its first picture. The host
  logs nothing. The same happens with the host built at the start of that
  day and with the release bundled on 2026-11-09, so it is older than the
  day's work; a notes or score window captures at once on the same host.
  What the multitrack's window waits for before it writes the picture is
  to be found out, and until then a scripted pass over the box's roll in the
  multitrack cannot be made. *(Fixed 2026-11-10: the multitrack is a plural
  asker, owed its boxes' summaries until it has been drawn and its width is
  known, and a capture waited for what was owed before drawing the window
  that would ask for it -- so it waited forever; and its ten-second limit
  never came, since a headless window has no frame to wake the loop with. A
  capture now waits only on reads already asked, draws otherwise, and wakes
  the loop by its deadline.)*

- ✅ **A script's verb that reads what the verb before it asked an
  application for runs before the answer lands** *(found 2026-11-10,
  writing the notes editor example's selection by pitch)*. A `/gui_verb` is
  performed when it comes; a verb whose answer is the application's (a
  roll's `select_pitch`, answered as a correction of its `selected`) leaves
  the host's state as it was until that correction comes back from the
  application's thread, so a second verb sent at once (`octave_up` over
  what was selected) reads the selection from before. A question is ordered
  already: a `/gui_query` of a window an application holds, and the
  `selected` of the application door, go round the application's thread
  (`ToApp::After`) and are answered after the turns before them, which is
  also what makes a capture after a verb show the verb done. A verb cannot
  simply take the same road, since it is performed by the front after the
  answer is back and the next question would overtake it; what is open is a
  per-window queue of a script's verbs and questions, each released when the
  application has answered the one before. Until then a script reads the
  window (`selected`, a capture) between two such verbs, as the example
  does.
  **Fixed 2026-11-10** (`host::scripted`): on a window an application holds,
  a script's verbs, hands, keys, sets and questions wait in the window's
  queue and each is released once the application has sent word back
  behind what the one before asked of it. The same pass found the opener of
  an application's window answered before the window was published, so a
  capture right after `open` met no window; the answer now goes round the
  application's thread behind the publishing.

- ✅ **A focused multitrack's meters at rest cover its focus ring** *(found 2026-10-08 by the user in `editors/edit_multitrack` with tall tracks scrolled: the meters drew over the blue line round the multitrack, above and below, only while they showed no signal; fixed the same day)*. The ring is drawn with the picture and the live layer over it, since a window keeps its picture and a tick draws what moves; a meter column at rest is the height of its track, so where a track met the multitrack's top or bottom edge the column covered the ring. The live layer of a focused element now ends with the ring (`frame::draw_live_places`), so it is the last thing of the element in a tick as it is in a whole frame. Pinned by `the_focus_ring_is_drawn_over_the_live_layer`; `a_scrolled_stacks_meters_paint_inside_its_own_rect` and `a_tall_multitracks_meters_stay_inside_it` were written while looking for it and pin that the meters stay inside the multitrack.

- ✅ **A press in a multitrack lands off its mark again at a density of 0.8 — seen only in the automated browser** *(found 2026-10-08, driving a multitrack in a page from the automated browser, whose `devicePixelRatio` is 0.8)*. The same numbers as the entry below, which is closed: a 900x240 CSS canvas with a 960x256 backing store (1.0667) at a ratio of 0.8. A press on a fade envelope's corner trimmed the box's left edge instead, and the wheel zoomed about a time that was not under the pointer — while a host test pressing the same corner through the element (`a_fade_corner_is_taken_and_the_flat_top_is_the_boxs`) takes it. **Probably that browser and not the host**: the user saw, the same day, that the automated browser's window restricts the page's area, does not scroll and does not pass the mouse's coordinates right. To confirm or drop: the same gestures in an ordinary browser zoomed out to 80 %. The native window is not affected. *Closed 2026-11-07 as the automated browser's, measured with real input:* headless Chrome at `--force-device-scale-factor=0.8` (a 900x400 CSS canvas over a 720x320 backing store) and at 1.0, driven over CDP with `Input.dispatchMouseEvent`, a short drag at every point of a 15 x 12 CSS-pixel grid over a multitrack with one box and its two fades, each classified by the undo label it left. At 0.8 every class falls where the box is drawn in the same frame's screenshot -- the left edge's grip trims, the fade's slope edits the fade, the body moves -- so the pointer is read at the density the canvas is drawn at. What the two grids differ by is the drawing, not the pointer (the next entry).

- ✅ **A multitrack's track heights do not follow the window's scale** *(found 2026-11-07, closing the entry above)*. A track's `height` on the wire is a length like `h`, so it is logical and the window's `ui_scale` should resolve it; the multitrack lays its rows out in physical pixels instead (`Stack::shown` reads the heights as they are), so at a density of 0.8 a track of 100 is 100 device pixels tall (125 CSS pixels) while the header's controls and the ruler shrink, and on a display at 2 a track is drawn half as tall as declared. The hand's zoom of a row, its floor and ceiling (`MIN_LANE_H`, `MAX_LANE_H`) and the curve rows' heights are in the same space, so the fix resolves them together and keeps a drag's delta in the same units as the height it changes. *Fixed 2026-11-08*: the stack resolves the heights, the gap and the scroll at the window's `ui_scale` (`Multitrack::stack`, `Stack::shown`), a hand's edge drag and the wheel's scroll are made logical before they change one, and the roll's automation rows, which compared logical heights against device pixels the same way, are resolved there too (`a_track_s_height_is_resolved_at_the_window_s_scale`).

- ✅ **A press lands off its mark in a page whose density changed after it
  opened** *(found 2026-10-06, driving the score editor's page from an
  automated browser, and seen the same day by the user in that window: "the
  mouse coordinates inside the canvas are failing")*. Measured: a canvas 1216
  CSS pixels wide held a backing store 1297 wide -- a ratio of 1.0667 --
  while the page reported a `devicePixelRatio` of 0.8, and a press reached
  the element a third further from the canvas' corner than the pointer was
  (1.0667 / 0.8). The drawing was right; only the pointer was off. The
  pointer is read at the density of the moment, and the surface is sized by
  the one it was last fitted at: the fit follows a layout change and a media
  query on the ratio, and the automation's resize changed the ratio without
  the query firing.
  Measured again with the density changed on purpose: neither that query nor
  an observer of the element's box in device pixels is told, so there is no
  moment to fit at.
  **Fixed 2026-10-06**: a pointer's place is read back into CSS pixels by the
  density it came in and out by the surface's own (`host/web/input.rs`,
  `at_fit`), so the two cannot disagree, told or not. Checked in a page with
  the density set to 1.6, 0.6 and 1.2 under a surface fitted at 1.0667: the
  same point names the same note each time.

- ✅ **The same theme is two sets of colours, one per build** *(found 2026-09-26, asked how the meter's gradient is computed)*. No shader corrects gamma, so a vertex colour reaches the screen through whatever the surface format does with it, and `Gpu::new` takes `get_default_config`'s first format. Measured: natively (Vulkan) that is `Bgra8UnormSrgb`, so the theme's numbers are read as **linear** and encoded to sRGB on write; in a page (WebGPU) it is `Rgba8Unorm`, so they reach the screen **as they are**. `meter_low` `[0.24, 0.78, 0.44]` is `(61, 199, 112)` in the browser and `(134, 229, 177)` in a window, and every colour of the host shifts the same way -- and so does what a `canvas` shader writes, and every blend and gradient, which are done in linear light on one build and on the encoded values on the other. A behaviour that differs between the two builds is a defect. Not measured: the WebGL2 fallback's format. **The decision:** which picture is the intended one -- ask for a non-sRGB surface natively (it is offered, `Bgra8Unorm`), so both read the theme as sRGB values as a page does; or keep sRGB surfaces in both (a page reaches one through an sRGB view format) and state the theme in linear terms.

  **Correction and fix, 2026-09-26.** The page measured above ran on WebGPU, which the user's browsers do not use: both run the WebGL2 fallback, whose surface offers `[Rgba8UnormSrgb, Rgba8Unorm, Rgba16Float]` in Chrome and in Firefox alike, and `get_default_config` took the sRGB one, which each browser then resolved its own way -- Chrome's picture matched the page above and Firefox's matched the window. Decided (the user, preferring Chrome's): the theme's numbers reach the screen as written, in every build. `Gpu::new` now asks for the plain twin of the default format (`plain_format`), measured to be `Bgra8Unorm` in a Vulkan window and `Rgba8Unorm` on WebGL2 in both browsers; a surface offering no plain format warns.

- ✅ **A box can be stretched past the samples it has** *(found 2026-09-17 by the user, in the multitrack editor, after the multitrack moved to seconds; reported as a regression)*. An edge drag on a box that does not loop is meant to stop at the last frame of its source (`boxes::drag` bounded by `Contents::total`, the frames the widget knows for that box), and it no longer does: the box's right edge goes on past its duration in samples. Not diagnosed yet. Where to look first: what `contents_of` answers for a box now -- the source's length as the widget learns it, and the box's window start against it, since the start and the lengths now cross through the seconds rule (`tempoclock::secs_to_samples_over`) -- and whether the `clips` props the editor sends still carry what the bound is computed from.

  **Fixed 2026-09-21: it was a join, and the seconds were not involved.** The
  bound held for every box over a take and for none over a join. Since "A join
  is drawn from the takes it reads" (`7ed858b1`, 2026-09-15) a join's own buffer
  is never fetched, so `contents_of` found no take under the join's `source`,
  answered no length, and an edge was bounded by nothing -- the case the
  contract reserves for samples nobody loaded. A join's length is the sum of its
  spans, which the `segments` prop carries before any sample arrives, and
  `contents_of` now answers that for a box named there. Held by
  `a_join_is_trimmed_no_further_than_its_spans`, which pulls a join of two
  300-frame spans out to 900 and fails without the fix; a box over a loaded
  take already stopped at its last frame, raw or kept whole. **Seen by eye
  2026-09-22** in `editors/edit_multitrack`, where it was reported: a joined
  box's edge stops where its spans end, the picture and the rectangle agree
  through a trim from either edge, and nothing about a box at the session's own
  rate changed -- which is the whole of what the ratio work underneath it owed
  (`crates/clausters-document/PLAN.md`, "A box is measured in the view's
  samples and filled with the source's frames").

- ✅ **The position cursor stood between two samples, beside the playhead that started from it** *(found 2026-09-17 by the user, zoomed to the sample in the multitrack editor)*. A click placed the group's cursor at the fractional sample under the pixel, while the transport it cued located on a whole one, so the two lines stood apart. `set_timeline_cursor` puts the cursor on a whole sample, as a selection already was, and the `"locate"` report carries that sample as a double. Held by `the_ruler_of_the_focused_view_does_not_take_its_focus_away`.

- ✅ **A box cut in two dotted the same sample at the end of one half and the start of the other** *(found 2026-09-17 by the user, same session)*. The sample regime reaches the sample just past each edge so the line crosses it, and dotted it too: the sample on a view's right edge is the first one after the view, so the first half drew the second half's first sample. A dot now marks only the samples of a half-open view. Held by `the_sample_on_the_right_edge_is_the_next_views`.

- ✅ **A box's edges on the multitrack are drawn on a sample, where a selection's are drawn between two** *(asked 2026-09-17 by the user: "the selection of samples visually places the margins between the spaces before and after in the audio editor. Should that be possible here too?")*. A selection's band runs from half a sample before its first sample to half a sample after its last (`selection::span_x`), so a one-sample selection is one sample wide. A box `[k0, k1)` is drawn from `x(k0)` to `x(k1)`, so zoomed to the sample its edges sit on the dots of its first sample and of the next box's first. Drawing the box by the same half-sample rule would put a cut between the two samples it separates. It reaches the box rectangle, its hit testing and edge drags, the local view its contents and layer are drawn in, and its fades; the position cursor and the playhead stay on the sample.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

- ✅ **Try cubic instead of straight segments where the samples are joined** *(asked 2026-09-15 by the user, after `A3`)*. Straight segments remain in two places: above the dots' zoom, where a column under two samples wide measures the line between samples (`line_column`), and at the dots' zoom when a view draws `measure: "peak"` without the `signal` layer. The user wants to try a cubic (Catmull-Rom or Hermite) there so the joined samples do not read as polylines. Two constraints go with it: a cubic is smooth but is still not the signal (its overshoots are its own, not the inter-sample peaks), so it belongs only to the sample layer and never replaces `signal`; and `line_column` and the joined segments must use the same curve, or the picture changes character again at the dots' crossing.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

- ✅ **The reconstruction draws nothing zoomed out** *(found 2026-09-15, closing `A3`)*. The `signal` layer is drawn only where the samples are separate points; in the column regime it is absent, because a picture of it there is the envelope of the *reconstructed* signal and that differs from the samples' envelope by a fixed amount at every zoom, so no bounded reconstruction per frame reaches it. The fix is a plane in the peak pyramid — the reconstructed min/max per bucket, computed once when the cache is built, as the mean square was in `A1` — which makes it a cache-format change (CLPK v3 → v4, `CORE_ABI_VERSION`, the Python builder, the parity vectors). iZotope RX draws exactly this ("Show analog waveform … when the waveform is zoomed out"), so the shape is settled and the cost is the format.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

- ✅ **A join blinked: its box drew empty until its own buffer had been downloaded** *(found 2026-09-15 by the user, on the web multitrack example: a join of shuffled fragments drew its waveform, but the box flashed empty first)*. Measured frame by frame in the page: the joined box was empty from the moment the join was drawn until about 290 ms later, the time it took to download the join's whole buffer -- 96 000 samples, one chunk a frame -- although every one of those samples is a span of a take already on screen. A join owns no samples, so the download was of nothing new, and its picture could only arrive after the server had built the buffer and the host had read it back.

  **A join is now drawn from the takes it reads.** The shared projection emits a `segments` prop beside `clips` -- the flat `box source start frames` quadruples of every join a caller knows, resolved through `Buffers::parts` -- and the multitrack draws such a box span by span, each span handed to its take's body over the part of the box it covers. Its own buffer is not asked for at all. The clip row keeps its seven fields, so nothing that reads a report changes shape. Both clients get it without a line of their own, since the applications crate's editor already kept the segments of every join it minted; the standalone host answers the same question out of the session's source table, where `Instance::mint_sources` writes the join. The fades at a join's seams are not drawn, which is stated in `docs/gui-protocol.md`.

  **And the standalone host now knows how long its takes are when it reads a report.** Answering the join's spans meant giving its look the source table, and with it the take lengths the table already held -- which the clients' look has answered since a window's duration became how much of its source it reaches (2026-09-13). The standalone never did, so a box its hand made was minted with the duration the box showed, and `a_box_the_piece_does_not_know_becomes_a_region_on_its_lane`, written four days before that rule, still expected it. It expects the reach now: the same box reads the same way in every endpoint.

  Held by `the_editor_joins_cuts_of_its_own_joins_flat` (the window is told the joined box's two spans), `a_join_asks_for_the_takes_its_spans_read_and_not_for_its_own_buffer` and `a_span_that_reads_nothing_is_dropped`.

- ✅ **A buffer the server did not have yet was drawn as an empty take, for good** *(found 2026-09-15 by the user, on the web multitrack example: four cuts in `glide`, the fragments mixed onto the track below, `j` -- and the joined box drew nothing)*. The protocol answers `/buffer_query` for an absent buffer with `frames = -1`, apart from a buffer of no frames, and both fronts clamped it to zero at the door. So the fetch machine finished the want with no samples, the multitrack stored that as the take (`bulk_of` replaces whatever a take held), and the take was never asked for again. A join a client mints is exactly this case: its box names the buffer in the turn the stitch is sent, the stitch completes on the NRT thread, and a query in between is answered from the mirror, which has no buffer there yet. A join of two boxes usually won that race; a join of five lost it every time.

  `/buffer_query.reply` with `frames < 0` now reaches `BufferFetches::on_absent`: the want stays, nothing is handed over, and `tick` asks again every `STALLED_ASKS` frames, letting go after `ABSENT_ASKS` so a buffer that was freed does not keep a front awake. The rule is the shared machine's, so the native front and a page read absence the same way.

  **What the previous fix claimed and did not do.** `91b15349` says a join a client mints reaches the host as `/buffer_touched`. It does not: the server announces `NrtAction::Wrote` and nothing else, and an install (a stitch, an allocation, a read) tells no other peer. Its `Host::forget_take` branch stays, because a peer writing samples into a take a multitrack draws *is* announced and used to be ignored, but it never covered the join, and the join it was tested on drew only because its query happened to arrive after the stitch.

  Held by `an_absent_buffer_is_asked_for_again_and_never_handed_over_empty`.

- ✅ **The web client wrote a minted join into its table a microtask late** *(found 2026-09-15, reading the two clients' `mint` side by side)*. Python's `Buffer.stitch(wait=False)` answers on the line that sends; the page awaited a promise, so the picture pushed in the same turn named no buffer for the joined box, and the old comment called that the platform's line. `stitchSent` is the synchronous half the public promise and `mint` both use now. Closed in `91b15349`.

- ✅ **A client's piece never told its window where the meters were** *(found 2026-09-15 by the user: no meters in Chrome, meters in Firefox)*. The window is composed before the playback makes the tracks, so the def carried an empty `meters` and nothing pushed them afterwards: the strips filled on whatever unrelated turn pushed the picture next, which one browser happened to produce and the other did not. `MultitrackEditor.open` pushes `meters` once the playback is prepared, in both clients, the way `Instance::tell_meters` already did for the standalone host. Closed in `382433f9`.

- ✅ **A piece drew three of its six takes, and which three was a race** *(found 2026-09-15 by the user, on the web multitrack example)*. `MAX_IN_FLIGHT` bounded a span and not a whole download, so six takes started six downloads into a 64 KiB reply ring and the replies that did not fit were dropped; a whole download had no stall clock either, so one lost reply left its box empty for the session. Whole downloads wait for a slot now, and `BufferFetches::tick` asks again for any conversation that stops answering. Closed in `e3061a6f`; held by `more_takes_than_the_carrier_holds_download_one_after_another` and `a_conversation_that_stops_answering_is_asked_again`.

- ✅ **A join in a standalone host makes a source nobody builds** *(found 2026-09-12 by the user)*. A join mints a source and the host built one only when a session was opened, so a join made during one drew empty, sounded through nothing and had no length to stop an edge at. Closed 2026-09-13 by the convergence entry in "Future directions": the reading of a segmented source is `clausters_editing::sources::stitch`, and the host realizes a minted one at edit time (`Host::mint_sources`).

- ✅ **A host with no client answered its own edits and drew half of what it applied** *(found 2026-09-12 by the user, on a standalone host opened on a session: the `A` toggle made no automation row, `j` over two boxes with a gap between them logged `join 22 23` and nothing else, and `j` over two that touch did nothing either -- while **split worked**, which is the observation that splits the causes)*. Three symptoms, two defects, and split working is what tells them apart: a split reports `clips`, which was routed and whose answer is a prop the host pushed back.

  **The vocabulary was written twice** (the entry in `crates/clausters-document/PLAN.md`, closed the same day): the dispatch knew `clips` and `lanes`, the projection answers for four, so `join` reached no reader at all -- the event went out on the wire, to nobody, which is exactly the line the user saw and the silence after it.

  **And the answer back was written short.** `clausters_editing::multitrack::props` produces **seven** props -- the rows, the boxes, the curves, the layers, the break-points, which curves are hidden and which boxes loop -- and both the def that opened the window and the adopt that follows an edit named **two** of them. So a standalone host never drew a curve at all, and the `A` toggle, whose entire job is to *mint* one, applied the edit, kept it in the document and pushed back nothing that draws it. A client answers with all seven, which is why this was invisible until somebody opened a session with nobody attached.

  `tree::Piece` carries the projection's map now instead of two named fields, and both sites push what is in it. The fix is the same shape as the other half's: **ask the projection, do not restate it** -- a key the picture grows arrives without anyone remembering to pass it along.

  Held by `the_toggle_that_makes_a_curve_puts_the_row_on_the_widget`, which fails on the two-prop version at the assertion that the widget was told.

  **The lesson worth keeping** *(and it cost this session a bad manual test)*: the standalone host is the configuration where every shortcut of this kind shows, and it is the one nobody runs. Both halves of this had been shipped for weeks behind a client that filled them in.

- ✅ **A pasted block chose its track by which box was earliest in time** *(found 2026-09-12 by the user, on the multitrack example, and stated there as the rule it should follow: a paste runs from the selected track **downward**, always, and what it did instead looked like some other rule -- one that changed with which tracks the boxes had come from and which one they were pasted onto)*. A paste has two coordinates and the widget anchored the block at **one** box for both of them: the earliest onset. In time that is right — an axis runs one way, and "it starts here" means the first thing on it. In rows it is meaningless, and it produced exactly the report: which track a block landed on depended on the order its boxes happened to have been recorded in, so the same block pasted onto the same track went up or down according to that, and part of it landed **above** the track the hand had pointed at.

  The rows anchor at the **topmost** box instead. The selected track is where the block *begins*: everything lands on it or below it, keeping whatever gaps the block had, which is the user's own case — boxes from tracks 2, 4 and 1 pasted onto track 3 are tracks 4, 6 and 3. It is also what every editor with tracks does; where they differ is the overflow, and that is the second half of this.

  **A block that does not fit is now refused out loud** rather than flattened. It used to clamp every row past the last track onto that track, which turns a block four tracks tall into a pile on one -- the one thing a paste promises not to do, and a failure the topmost anchor makes *more* reachable, since pasting low on the stack is now the ordinary thing to do. The refusal names what it needs (`this block is N track(s) tall and needs M here; the piece has K`), which is the difference between a verb that did nothing and a verb that says why.

  Held by `a_paste_starts_at_the_selected_track_and_never_above_it` -- which walks the user's own three boxes, and checks the *time* anchor in the same test, since the point is that the two anchors are different boxes -- and by `a_paste_that_runs_past_the_last_track_says_so`.

- ✅ **The host's diagnostics are native-only, and a dead key still says nothing** *(found 2026-09-12, in the consistency pass the user asked for over the host, whose first subject was this one: the status bar as a general facility for debugging, compiled only into a debug build)*. The status bar has exactly two doors -- `gestures::effects::emit`, which reads every event the host produces, and `Host::settle`, which reads the owner's reason -- and between them they cover *what the hand asked for and what came back*. Everything the host says to **itself** takes a different road and three of them are broken.

  **102 `tracing::` call sites in shared host code** (`src/host`, excluding the two fronts) print natively, through the subscriber `bin/clausters-gui.rs` installs, and are **no-ops in the page**: the browser front has its own `web::log` and no `tracing` subscriber at all, because a wasm shim was out of scope when the front was written. That is one diagnostic surface in one of the two builds of a host whose whole rule is that it is one host compiled twice -- so a defect diagnosed by reading the log in a window cannot be diagnosed at all in a tab.

  **A key nothing consumed is silent.** It reaches `gui::app`'s last arm, which says `tracing::debug!("key: reached the window's own shortcuts")` and stops. That is the exact shape of the defect reported twice on 2026-09-12 (the two entries below, on `j` over a pair that could not be joined): the key was refused correctly and the refusal reached nobody. The two fixes that day taught the three multitrack verbs to answer with a reason; the *general* case -- a letter no element claimed -- was never taught anything.

  **And a bound widget says nothing at all.** `effects::deliver` returns as soon as `Host::forward` takes the value, so of the two roads a value leaves by, one feeds the bar and the other does not. A knob bound to the server is the one control whose whole history is invisible.

  **Two channels, not one, and that is the design decision.** The bar is *product*: its reason for existing is that a person at the window learns what was refused, and that is as true in release as in debug. What wants compiling out is the **diagnostic** channel the 102 sites are reaching for -- so the fix is a third `status::Kind`, a `note!` door that writes it, and a `#[cfg]` that drops the door in release, with the 102 shared sites migrated onto it so both builds say the same things. Native keeps `tracing` underneath (the file log is what a session's transcript is); the page gains the lines it never had.

  **The refusal has two spellings, which is the smaller half.** `effects::refuse` is the general door and only `gestures::press` uses it; six other sites build `"refused" <verb> <why>` by hand (`elements/notes` twice, `elements/curve`, `gestures/keys` twice, and a private helper in `elements/multitrack`). One `Events::refused` beside `Events::message` closes them.

  **Closed the same day.** `host::diag` is the door: `emit` is `tracing` natively and the console in a page, the four levels are macros over it, and the 102 sites now say `diag::warn!` -- nothing in `host` names `tracing` any more except the one function that *is* the native half. `debug!` and `note!` carry a `#[cfg(debug_assertions)]` **inside** the expansion, so a release build has neither the call nor the text of the line; `warn!` and `info!` stay, because a warning about the world is not instrumentation. The consequence is written in the macro's own doc rather than left to be found: a binding whose only reader is a `debug!` is unused in a release build, and the spelling that reads in both is `_name` at the binding and `{_name}` in the format string.

  `note!` is the level with a second destination -- `status::Kind::Note`, colored apart (the accent's quiet form) because it reports on the machine and not on the work. Two call sites to start: a key no element claimed, said at the one place both fronts learn it (`gestures::keys::key_at_cursor`), and nothing else invented, since the point is the door and not a census of what to instrument. The bound value is **not** a note but an ordinary `Did` line (`Line::of_bound`): it is something the hand did, and the bar was quiet for it only because a binding is the one road that never passes through `emit`. `refusal()` is now one function three readers share, and the six hand-built sites are gone.

  Held by `a_key_no_element_took_leaves_a_note_and_not_a_line`, `a_bound_widget_says_what_it_did` and `a_note_is_the_machines_line_and_collapses_by_its_verb`; `docs/gui-protocol.md` says what a reader of the bar now sees and `docs/architecture.md` carries the module row.

  **And one consequence found by trying it**: the binaries the Python launcher stages are **release** ones, so a channel compiled out of a release is invisible in exactly the path a manual test takes. Hence the `diagnostics` feature -- `#[cfg(any(debug_assertions, feature = "diagnostics"))]` -- which turns the two levels back on in a release build that is being instrumented, off by default because a shipped host carries no instrumentation. The recipe is in `crates/clausters-gui/README.md` beside the `font-atlas` one it reads like.

- ✅ **The clip's verbs and the roll's verbs are one table written twice** *(found 2026-09-12, same pass)*. `Multitrack::key` and `Notes::key` implement the same letters over the same reading: `q` quantizes the held boxes to the snap grid, `e` cuts them at the window's cursor, `j` joins what is adjacent, `Delete` removes them, and `Ctrl`+`c`/`x`/`v` carry a block through the host's one clipboard string as the same JSON a `/gui_set` of the prop accepts, pasted at the cursor keeping its own shape. The *arithmetic* is already shared and has been since the placement module was written -- `split_at`, `merge`, `adjacent`, `quantize`, `move_block` -- and what is duplicated is the layer above it: the verb over a **selection**.

  It has already drifted, which is what makes this a fix and not a preference: a multitrack that cannot quantize says so (`"these boxes are already on the grid"`) and a roll that cannot quantize returns `None`, which is the silence the entries above spent two reports learning to call a defect. The score will be the third copy, and the audio editor's segments the fourth.

  What is missing for one implementation is small, because `Placements` already abstracts the storage: it answers *where the box is* and not *how to make or drop one*. Adding that -- insert, remove, and the identity a new box gets -- makes split, join, delete, copy, cut and paste generic over anything with boxes on a time axis, and leaves each element only what is genuinely its own: a clip mints a fresh name, a note is identified by its pitch, a join over fragments is the document's to refuse.

  **Closed the same day, and it is two methods.** `placement::Boxes` is `Placements` plus `duplicate` and `discard` -- the two questions a verb asks and a drag never does -- and `split`, `discard` and `rebased` are written over it once. A roll's `e` and a lane's `e` are now the same call, and so are their Delete, their cut and where a pasted block lands. What stayed with the holder is exactly what nothing general could state: a clip is minted with a fresh name (`fresh_name`, because a name is a clip's identity and a correction finds it again by it), and a note's second half keeps the pitch, the velocity and the channel of the one it was cut out of.

  **Two conventions became one on the way.** A roll normalized its clipboard block to a first onset of zero and a lane carried the offsets as they stood, so the same gesture meant two things in two views; both now carry the block as it is and `rebased` decides where it lands, which is the paste's question and not the copy's.

  **And the drift itself is gone**: the roll's `q`, `e` and `j` say why they did nothing, in the words a lane's already used. Held by `a_verb_that_acts_on_nothing_says_why_rather_than_nothing` in the roll (the twin of the multitrack's), `a_cut_note_leaves_two_halves_that_are_still_the_same_note`, and three tests in `placement` written against a holder that is **neither** a roll nor a multitrack -- which is what says the verbs reach for nothing either of them happens to have.

  **What is not general yet, and why.** A **join** is local in a roll and asked of the owner in a multitrack, because a join over material is the document's to refuse (a gap it cannot state as silence, an overlap it cannot state as a mix). And `copy`/`paste` still carry the holder's own JSON, since the carrier *is* the prop each one accepts. Both wait on the same thing the third copy will need: the structures layer below.

- ✅ **A join did nothing, and said nothing, because the correction renamed the half the hand was holding** *(found 2026-09-12 by the user: "j for join does nothing, logs nothing in the status bar, nothing")*. The third time this seam broke, and the first time from *this* side. The host mints the tail of a split (`a 2`) and the document mints its id, so the correction that answers a minted name calls the same box something else -- which is the whole point of the correction. The `clips` arm re-found the hand **by name**, so exactly that box was let go; `join_held` then had one box where the hand held two, and a join that joins nothing emits nothing, so the key looked dead.

  Fixed by a second pass: name first, then **where it is** -- same lane, same offset within a frame, each box claimed once. The piece placed the half exactly where this widget said, so the place is an identity the rename cannot touch. `a_split_survives_the_correction_that_renames_its_half_and_joins` walks the whole round trip (cut, correction, join) rather than the halves separately, which is why the two earlier fixes each passed and the path still failed.

- ✅ **And a verb that acted on nothing still said nothing** *(found 2026-09-12 by the user, reporting the same key a second time: "I press ctrl-j but it does nothing. They are still apart")*. The report that closed the entry above named this as still open, and the second report is what settles that a correct refusal nobody is told about cannot be left as one: it is indistinguishable from a key that does not work, and the person at the window has no way to tell the two apart -- so the same key gets reported again, correctly.

  The gesture in the report is the **refused case by design**: two halves of a cut, the tail moved in front of the head and snapped there. They touch and they read one source, but they do not read on from each other, and one window over both would play the source straight through material the boxes skip ("Join over fragments" is the other half of this). The verb was right to refuse; the silence was the defect.

  There was never a protocol question here, which is what the earlier note got wrong: the host already refuses out loud, by emitting `"refused" <verb> <why>` as an ordinary event -- `status::Line::of_event` reads that shape, and an unwritable body and a Ctrl press on the markers lane both already answer with it. What was missing was that the three key verbs returned `bool`. So `join_held` answers with the **reason** instead, `why_not_joined` names which of a join's four conditions the pair failed, and `q` and `e` say theirs. A client sees a tag its intake does not know and answers `Intake::nothing`, which is the same path every other host refusal already takes. Held by `a_verb_that_acts_on_nothing_says_why_rather_than_nothing`; the two tests that asserted the old silence now assert the sentence, which is the better assertion.

  **Still open, and smaller than it looked**: `Ctrl`+`J` is not a binding -- the three verbs all guard on `!ctrl`, so the clipboard family keeps `Ctrl`+letter -- and a modifier that means nothing is swallowed by nothing and says nothing either. Which keys these are is the unsettled question already filed as "A shortcut is the application's, not the widget's".

- ✅ **The automation toggle flipped a flag nothing draws from** *(found 2026-09-12 by the user, reported three times: "the A button does not work", "the A still neither hides nor shows", "on creating a track and activating A nothing happens; on creating another track the previous one's automation appears")*.

  The stack is built from `hidden`, which is the **owner's** answer about which curves are not shown. The press flipped the lane's own `curves` flag and emitted the `lanes` report, and the owner wrote `visible` onto the automations -- but the owner answers with the whole picture only when a **name** changes, and hiding a curve changes none. So a press that only hid or showed stated nothing anybody draws from, and the row stayed exactly where it was.

  It read as three different bugs because of the one case that *does* change a name: the first press on a bare track **mints** a gain curve, which is answered, so that press appeared to work and every press after it appeared not to. And a press whose answer was withheld was carried back by the next gesture that did add a row, which is the "on creating another track the previous one's automation appears" report -- the same silence, one gesture later.

  Fixed by saying it in both places the press already touches: the flag and this lane's entries in `hidden` move together, so the picture changes under the hand the way a dragged clip does and the owner's next correction confirms it. `the_toggle_hides_the_row_and_shows_it_again` presses twice and reads the stack, which is the assertion the earlier tests were missing: they each proved one leg of the path -- the gesture emits, the crate creates, the element accepts the correction -- and the path they proved was never the one that failed.

  **What this cost, and the lesson worth keeping**: it was diagnosed twice from reading and both readings were wrong, because every part read correctly on its own. What settled it was logging the actual wire of the running example -- the events out and the corrections back -- which showed in one pass that the press *was* emitting, the owner *was* applying it, and nothing was coming back. Instrument the seam before theorising about it; a seam whose halves are each correct is exactly the one that cannot be read.

- ✅ **The `meters` prop was read once and never again** *(found 2026-09-11 by
  eye, a track made by double click showing no strip; fixed the same day)*.
  `Multitrack::set` had an arm for every list it draws except this one, so the
  buses reached the element at construction and a later correction was dropped.
  A track's meter buses are allocated when the track reaches the server, which
  for a track made in the host is *after* the window opened -- so exactly the
  tracks that need the prop set were the ones that never got it. It also
  explained the layout report that came with it: the header lays the name out
  against what the strip takes off the right edge, so a lane with no meters
  gave the name the whole band and the name spread over the space the strip
  should have had. Held by `a_metered_track_declares_its_buses_and_reads_them`.

- ✅ **A correction emptied the hand, so a split could not be joined** *(found
  2026-09-11 by use; fixed the same day)*. Setting `clips` cleared the
  selection, which was right while the payload only ever came from the client's
  own first draw. Since the client answers a name the host minted with the
  whole picture, that payload now arrives **between** the cut and the `j` that
  would put it back: the two halves the hand was still holding were let go on
  the way, and a join had nothing to join. The list is still replaced whole --
  the indices it holds are meaningless afterwards -- but the *names* are not,
  and the boxes are the same boxes, so the selection is re-read through them.
  Held by `a_clips_correction_leaves_the_selection_where_it_was`.

- ✅ **A curve kept pointing at the node its clip used to be** *(found
  2026-09-11 by ear, a box dragged to another track playing flat out; fixed the
  same day)*. A clip that changed track is made again rather than set -- a clip
  is a slot inside its track's group -- and `/graph_map` names a **node**, so
  the map went away with the old one while the curve went on writing a bus
  nobody reads. The hand does not send that port either, since a curve owns the
  port it names, so the box came back at the def's own default. Both clients
  re-map when the owner they hold is not the owner they are given.

- ✅ **The example's first box makes no sound, and the reason is that a curve is
  not a control yet** *(found by use 2026-09-10; closed 2026-09-11 by
  `G35.3`/`G35.4` and the example's pass onto them)*. `multitrack_audio.py` read
  a clip envelope **once**, at the box's first break-point, and wrote it into
  the reader's `amp`: the `white` box's envelope starts at zero, so the box was
  silent for as long as it was on screen. Nothing was wrong with the document,
  the picture or the report -- what was missing was the curve becoming a control
  that *plays*, which is what `mt.curve` plus `/graph_map` now do. It was left
  alone on purpose rather than patched in the example, since sampling the curve
  per block is exactly what the milestone existed to stop.

- ✅ **A take reloads into the web host's views over and over** *(found
  2026-09-11, driving `edit-multitrack.html` from a browser to check the port
  against its Python twin; **not diagnosed**)*. The page's console prints
  `buffer N: 96000 frames x 1 channel(s) loaded into K view(s)` for the visible
  takes about four times a second, with nothing on screen changing and no
  gesture in flight, and `K` wanders (9 to 12) between lines. It predates the
  work that found it -- the same lines come out of the previous commit's page --
  so it is recorded rather than fixed here. Two things to settle before it is
  worth touching: whether a reload copies samples or only re-binds a view (the
  cost is nothing like the same), and why the count of views moves when the
  window does not. The native host is where to compare, since the host is one
  program compiled twice and a loop in only one of them names its own cause.

  **Fixed 2026-09-13** (`7ecbbf84`). The pass before every repaint pushed every
  take the multitrack declares, so a finished download started again on the
  next frame. An element asks for its takes through `Samples::ask_takes` now:
  all of them when a window is built, and a stitched one again when its
  `/done` arrives.

- ✅ **Buffer 0 is a buffer, and `x or -1` said it was not** *(found by use
  2026-09-10, on the box the example loads first)*. `Sources.bufnum` answered
  `-1` -- "nobody loaded this" -- for a take whose buffer number is **zero**,
  which is the first one an allocator hands out. So the first take a script
  loaded was the one take its boxes could not draw, in both clients, under a
  docstring that says in as many words that negative and not zero is the
  sentinel. Asked for explicitly now, and pinned by a test in each client.

- ✅ **A time axis is labelled by one tempo, and a piece can have several**
  *(found 2026-09-05, reading `editors/tempo_map` after its clip was put right)*.
  A lane's `x` axis and a `timeruler` take `tempo` (beats per second) and
  `beat_at`, which is one segment and an offset — so a page whose tempo changes
  is ruled by whatever tempo held at its start. The clips are placed correctly,
  because a client converts each onset through the piece's `TempoMap` before it
  sends it; only the **labels** are wrong, and they are wrong from the first bar
  after the change: `tempo_map`'s piece is sixteen beats and its ruler counts to
  eight.

  It is not a client's to work around: the ticks are chosen by the host, at the
  spacing the zoom leaves room for, so the client has nothing to convert. What
  is missing is a way to hand the axis the **map** — the segments a
  `TempoMap` already is, which both clients hold and the shared crate already
  converts with.

  **Fixed 2026-09-05.** `axes.x.tempo_map` carries the breakpoint list a
  `TempoMap` serializes to, and `TimeUnit::Beats` holds it. The tick math stopped
  assuming a straight line: `fit_step`, `labels_fit` and `emit_time_ticks` take a
  placement function, so every tick -- majors and minors alike -- is placed by
  asking the map what second its beat falls on. Two things fell out rather than
  needing rules of their own: the rung that fits is decided by the closest pair
  (because `labels_fit` already walked every pair measuring labels instead of
  dividing the axis by a count), and the minors appear or not by `min_gap_px`,
  the narrowest gap on the axis. The cursor read-out goes through the map too,
  and takes seconds per pixel instead of beats per pixel, since under a map that
  is a local rate. Without a map nothing changed. `views/tempo_ruler` is the
  example: a tempo curve with the seconds ruler under it and the beat ruler it
  makes above.

- ✅ **A curve over a parameter span has no vertical ruler**
  *(found 2026-09-05, writing `views/tempo_ruler`, whose curve is beats per
  minute)*. `RulerY` is `Off`, `Norm`, `Db`, `Bits`, `Percent`, `Hz` -- every
  unit an *amplitude* or a *frequency*. But the `bpf` body is general on
  purpose, and says so: "values live in `[min, max]` -- unipolar, bipolar, or
  any parameter span". So a curve over a cutoff in hertz can be ruled and a
  curve over beats per minute, a pan position, a mix or a MIDI controller cannot
  -- the numbers are on the widget (`min`/`max`, `points_min`/`points_max`) and
  there is no way to draw them beside it.

  The arithmetic exists and is already used: `ruler::value_ticks(lo, hi, ...)`
  is the generic value axis a `plot` draws with, on the same 1-2-5 ladder and
  the same measured-label fit. What is missing is a `ruler_y` that names it --
  a unit that says *these are the numbers `min`/`max` are in*, optionally with a
  suffix a script supplies, since "BPM" and "Hz" are the widget's to know and not
  the host's to guess. `views/tempo_ruler` states its 30-90 BPM range in prose
  because it cannot state it on screen.

  **Fixed** (2026-09-05). `RulerY::Value` names that ladder -- `y.unit:
  "value"`, straight onto `ruler::value_ticks` over the element's own
  `min`/`max`, with no suffix: the number is the widget's and a unit string
  would be the host guessing at it. The unit was the smaller half. The `curve`
  element had **no editor chrome at all**, so there was nowhere to hang a value
  strip: it now carries an `EditorProps`, draws a time strip under its field and
  a value strip left of it (both defaulting off, so every window that already
  draws a bare envelope keeps its picture), reports a `content_span` and a
  `gutter`/`measured_gutter`, and joins the navigation group `link` names --
  which is what lets a `timeruler` above a curve rule the curve.

  **And the trap that came with it**, which is the part worth keeping: giving
  the element an `EditorProps` made the frame hand it a `TimeSpace`, and
  `Curve::draw` had been reading exactly that to decide "I am a clip's body" --
  so the first build drew the envelope bare, over the whole rect and against the
  group's window instead of its own field, with neither strip and with its
  edit-back reporting times that were off. Body-or-view is a fact about **which
  door built the element** (`body()`/`empty_body()` against `from_props`), not
  about whether an axis was handed in: sharing an axis with a ruler is not being
  layered under anything. It is now a field, and `a_body_draws_without_the_view_s_chrome`
  drives a real body against a real view rather than one object through two
  branches.

- ✅ **The time range is the ruler's, and the two selections coexist**
  *(stated 2026-09-03 by the user closing the marquee's unification, decided and
  done 2026-09-04 — the gesture table is the user's)*. Landed for the
  **free-standing `timeruler`**; the ruler *strips* an element reserves out of
  its own height are the entry under this one.

  **There are two selections, they are of different things, and they live at
  the same time.** Confusing them is what made a marquee leave a band behind
  it:

  - a **data selection** — what is going to be *edited*: the clips a rectangle
    covered, the boxes of a patcher, the notes of a roll, the frames of a take.
    The rectangle is the *gesture's* picture and goes with the hand; what stays
    is the things, drawn as selected.
  - a **time-range selection** — what is going to be *played or rendered*: a
    span of the axis, which is the selection itself and so outlives the
    gesture. The group keeps it, every linked view draws it as a band, the
    transport loops inside it.

  **Why a signal needs only one and a roll needs both.** In a signal the datum
  *is* the axis — the unit is the frame, and a take's several channels are read
  frame-wise — so a selection of samples **is** a span and one gesture answers
  both questions. In a roll or a multitrack the data are **objects placed in
  time**: which notes or clips are in the hand does not follow from a stretch of
  the axis (a half-covered clip, a note outside the swept band of pitches), so
  the two questions come apart and each needs its own gesture.

  **They are told apart by where the gesture starts, not by a mode.** The data
  selection is swept on the **body** — the grid, the lane stack, the trace — and
  the time range on the **ruler**, which is the element that is nothing but the
  axis. Nothing has to decide what a drag means; the place it began already
  said.

  **The ruler's table** (the chords are the ones already in use elsewhere, so
  they are the ones to write; they can be changed later without changing any of
  the above):

  | Gesture on a ruler | What it does |
  |---|---|
  | drag | **scrolls** — pans the axis, on every view (signal, roll, lane), which is what the bare wheel and Shift+drag already do elsewhere |
  | Alt+drag | sweeps the **time range** |
  | Ctrl+click | adds or removes a **marker** (below) |
  | click | puts the transport's cursor where it pointed |

  On a **signal** the ruler's range is not a second thing: it writes the same
  selection the body's sweep does, because there the frames and the span are one
  selection. So nothing is disabled there and nothing doubles — the ruler is
  simply another hand onto the selection the view already has.

  **What it took.** `TimeRuler`'s plan was `(&[Locate], &[Pan], &[Locate],
  &[Locate])` — the plain drag scrubbed. It is now plain, Shift and Ctrl `Pan`
  (Ctrl left on the pan deliberately: the markers below want that chord and
  nothing should be holding it), Alt `Select`, and `locate` moved to the
  **click** — the precedent the lane already set ("locating did not move to a
  modifier, it moved to the click": a sweep that never left the slop is where
  the hand pointed). `Drag::Pan` grew the axis' `body` for it, which is what a
  locate reads the sample against; it kept only the width before.

  No new state, which is the half that made this cheap: a free-standing
  `timeruler` already joins the lanes' navigation group, and the span already
  lives on the group (`sel_start`/`sel_len`), where every linked view draws it
  and the transport reads it. Nothing about the range itself was written — only
  a hand onto it.

  The pass: the host, `docs/gui-protocol.md`'s `timeruler` row, and both
  clients' `timeruler` builder docs (all three said a press locates and that you
  scrub on the ruler).

- ✅ **A ruler *strip* is not the ruler widget, and it carried none of its
  table** *(found and fixed 2026-09-04, implementing the entry above)*. The free-standing
  `timeruler` is a widget of its own, so its gesture table is its kind's. The
  ruler an element reserves out of its own height — a lane's `ruler` prop, a
  roll's, a signal's — is **not**: the press lands on the lane or on the
  element, whose plan is the body's, so on a strip a drag sweeps whatever the
  body sweeps and Alt does not reach the range. A roll's own strip is where
  `edit_notes.py` would look for it.

  What stood in the way is that **a plan is per-modifier and a strip needs a
  per-region answer**: `GestureMap::of_kind` and `Element::gesture_map` are
  asked without coordinates, so neither can say "over the strip, this table;
  over the grid, that one". Three shapes were weighed and none taken — a
  region-aware gesture map (a coordinate on the call, touching every element),
  a strip that claims the press itself (the `Claim::take().marquee()` door,
  which serves an element and not a lane, since a lane is a container), or
  making the strip a child widget so it simply *is* a `timeruler` (the honest
  model, and the most disruptive: ids, layout, and every def that sets `ruler`
  as a prop).

  **What it took was a fourth, and smaller: choose the table at the press, by
  where it landed.** One site reads it — `interact::hit`'s `chain_of`, which is
  the hit test and so already has the coordinates — and one predicate decides:
  *a press in the bottom `ruler_h` of a widget whose editor has a ruler on is a
  press on a ruler*. It is one band because every view reserves it the same way
  (`lane_body`, `pianoroll::regions` and `timeline_body` all take `ruler_h` off
  the bottom), so the gesture is read from the numbers the drawing reserves it
  with and the two cannot drift.

  The answer is kept on the `Frame` (`ruler: bool`) and carried into
  `Drag::Pan` (`on_ruler`) rather than re-derived at the release: a pan may
  travel off the strip it began on, and it is the *press* that says what the
  gesture is. No element signature moved, no ids moved, and a def that sets
  `ruler` as a prop lays out exactly as it did.

  Written as a rule rather than as an exception, in the reference and in both
  clients' `timeruler` docs: **a ruler strip answers as a ruler, whoever drew
  it** — and over the body of the same widget nothing changes, which is still
  the data selection.

- ✅ **Labelled markers on the time ruler** *(decided and done 2026-09-04 with
  the two entries above; the ruler is what made it possible)*. A marker is
  **three fields** — an exact `time`, a `label` and a `color` — and it is the
  *axis'*, not a view's: a marker names a moment in the piece, so it rides on
  the editor chrome (`EditorProps.markers`) and every ruler that shows that axis
  draws the same ones.

  **It draws an arrow into the ruler's ticks and no line down the picture.** A
  playhead and a selection band are the two things that draw a vertical line,
  and a third would make three lines mean three things at a glance. What a
  marker is *for* is the click: the transport goes to the moment it was placed
  at, not to the pixel the hand landed on — which is why the arrow is
  hit-tested against the same geometry it was drawn with (`ruler_strip` /
  `marker_at`, one derivation read by the drawing and by the gesture).

  **Numbered, because a blank label says nothing.** Ctrl+click adds one — the
  chord the ruler's plan was left holding for exactly this — labelled with the
  count, and Ctrl+click on one takes it away; renaming and recolouring are the
  owner's, through the prop. `GestureStep::Marker` declines where there is no
  ruler to put one on, so a plan naming it over a plain body falls through.

  The edit-back is the whole list every time (`"markers" time label color …`),
  as `notes`/`osc`/`points` are, so an owner replaces what it holds rather than
  reconciling; `/gui_query` gives it back as the JSON string a `/gui_set` takes.
  Both clients take `markers` on the five builders that carry time chrome —
  `timeruler`, `track`, `pianoroll`, `waveform`, `spectrogram` — through the one
  place each has for it (`_X_AXIS`/`_axes`, `TimelineOptions`/`timelineProps`),
  so the two lists cannot drift apart one builder at a time. `multitrack.py` and
  its page twin seed three and print what comes back.

  **It is sized against the numbers it stands among** *(found 2026-09-04 by the
  user, looking at the example: "they look fine, very small")*. It was drawn at
  `micro_scale` -- the size a roll's OSC flags use, in a lane of their own -- and
  its arrow was a fixed nine pixels: a full step under the tick labels it sits
  beside, and a number that stopped meaning anything at another density. Both
  now come off the ruler's own caption: the label at `caption_scale`, on the
  tick labels' row, and the arrow a cell and a half of that font wide
  (`marker_w`). The tick row's own two numbers are named (`TICK_H`,
  `TICK_LABEL_TOP`) so the marker stands on that line by construction rather
  than by a literal repeated in a second file. The test asserts the *proportion*
  at every density the metrics are generated at -- wider than one character,
  narrower than the strip is tall -- since the rule is the proportion and not a
  pixel count.

  **What is not here, and is not missing**: dragging a marker to move it. A
  marker is a moment and the press has already said which; moving one is a
  second gesture on a surface whose plain drag is the scroll, and it is worth
  deciding once there is a hand that wants it rather than now.

- ✅ **A selection has two axes wherever the picture has two, and one where it
  has one** *(stated 2026-09-03 by the user, closing the selection unification;
  done the same day)*.
  The rule the configured sweep has to satisfy, written as a rule rather than as
  four behaviours: **the patcher, the multitrack and the notes select
  horizontally *and* vertically** -- a rectangle over boxes on a plane, a
  rectangle down the lane stack that takes clips of **different tracks**, a
  rectangle over the roll that takes the notes of the pitch rows it crossed --
  while **a signal selects on time alone**, because its second axis is amplitude
  and a band of it answers no question a hand asked. (The spectrogram is the
  exception the `select_box` step already expresses: bins are a real second axis,
  which is why the restriction is a *step* and not a mode.)

  Where the tree stands against it: the three vertical selections all take the
  right things now, and it is the **picture** that is still one-axis on the lane
  stack -- the swept band is the navigation group's, so it paints every lane
  while the clips taken are the crossed ones. That is the entry below ("The swept
  time band is the whole group's"), and this one is what it has to be measured
  against: not "draw fewer lanes" but "the vertical half of a rectangle is a
  value the hand chose, in every view that has a second axis". Signal views are
  correct as they are and must stay that way -- one axis is the answer there, not
  a missing feature.

  **Closed by the entry below**, which was the whole of what was left: the lanes
  a sweep crossed are now recorded beside the group's span and the band is
  painted on those and no others, so the four pictures answer the rule the four
  takes already did.

- ✅ **A press on empty staff answers with the engraver's own drawing about one
  time in seven, and it is the staff lines** *(found 2026-09-07 by the user, by
  eye, in `notation/score_editor`)*. Measured over the sitting: **64** presses
  wrote a quarter and **10** came back as `"element"` carrying an id the model
  does not own -- the engraver's own drawing -- so no note was written and the
  page answered a selection instead. It looked random from the window until the
  user placed it: it is the presses that land **on a line** rather than in a
  space, which is not a coincidence but the whole mechanism.

  It is the open half of the hit test the noteheads' fix closed on 2026-09-03.
  That one settled the case where something *sounds*: a sounding element wins
  over anything drawn across it, and the tightest box only decides between
  those. On empty staff nothing sounds, so the tightest box decides alone --
  and a staff line is a hairline the width of the system, the tightest box on
  the page. It is the same geometry that made a notehead answer with the staff
  in the first place; only the arm that resolves it is different, because there
  is no sounding element to prefer.

  **Selecting the staff is not the wrong answer -- pre-empting the write is**
  *(the user, the same day)*. A staff is a thing a score editor selects, and
  what a selected one admits is a question of its own (see "A selected staff is
  edited by its line count, not by its position" in Future directions). What
  cannot stand is that a press meant to write a note is spent on a selection
  because it landed a pixel onto a line: writing is what a press on the staff
  *is for*, and selecting the staff needs its own way to be asked for. The rule needs its second
  half: what a press means when it lands on nothing that sounds, where a staff
  is a **place** and not a thing to select.

  **Fixed 2026-09-07, and the second half is exactly that.** On a page that
  takes note **entry**, a pick that is not a sounding element is blank paper:
  the press writes. Selecting a staff is untouched where it is the only thing a
  press can be -- a page without `entry` still answers with the furniture under
  the pointer, because there the press has nothing else to be -- so what the fix
  refuses is a *write* being spent on a selection, which is what the user asked
  for and no more.

  Two guards, and the second was nearly missed. The rule applies only where the
  page **declared which ids sound**: a client that named no `elements` leaves
  the host unable to tell a note from a staff line, and there every pick is the
  tightest box, so treating them all as paper would answer a press on a notehead
  with an insert. And the rule reads the pick rather than the *kind* of
  primitive, so it needs no list of what counts as furniture -- the engraving
  walk already said which ids sound and a renderer still cannot re-derive it.

  **Nothing tested this, and the fixture is why.** The score fixture's staff
  lines carried **no id**, so in every test the hit test had nothing to fall
  back to and the defect could not appear. The real page labels them with the
  staff's own id. There is a second fixture now that engraves them as the
  engraver does, and the two tests hang off it: a press on a line writes, and a
  read-only page still selects. It is the same lesson the pencil's threshold
  taught this morning -- the gap was not in the code that was wrong but in the
  test data that could not express it.

  **The first cut of the rule was too wide, shipped, and was caught by eye
  within the hour**: *"it is not possible to select slurs, p, mp and other
  elements, now it always adds notes"*. It asked whether the pick **sounds**,
  and `elements` names notes and rests -- so a slur, a hairpin, a dynamic and a
  beam, which sound nothing and are elements of the score all the same, became
  blank paper and every press on one wrote a note. *Sounding* and *being the
  staff's own drawing* are different questions, and only the second was this
  fix's business.

  The rule is now the narrow one: **the staff's lines and nothing else**,
  through `ScoreData::staff_ids`, filled by the same pass that clusters those
  lines into staves. Derived rather than declared, because it is the same
  geometry the staves themselves come from and a second source for one fact is
  a second answer to it. And the test data was the gap again, for the third
  time in one day: the fixture held noteheads and staff lines and nothing that
  is neither, so no test could have caught it. There is one with a dynamic on
  it now.

  **What this did not fix, and the user named it** *(see "A press on a score
  means what its context says" in Future directions)*: three passes over one
  press were three attempts to say in geometry what a musician says in a
  sentence, and each got one case right by making another wrong. The editor
  needs to know what a press is *for* before it decides what it landed on.

- ✅ **A paste lands on a cursor no hand can place** *(found 2026-09-07 by the
  user, by eye, in `editors/pianoroll`: "pasting should do it from the cursor,
  otherwise there is no reference for where to paste")*. `Ctrl`+`V` over a roll
  pastes the block at `self.step`, and so does the cut that shares the comment:
  *"a paste has no pointer, and the cursor is where the roll is being written"*.
  The diagnosis is right -- a key gesture has no pointer to read -- and the
  cursor chosen is written in exactly one place in the whole element: it is
  **advanced** when a live note is entered by step, and no gesture ever sets it.
  So for anyone who is not step-recording it is 0, every paste lands at the
  beginning whatever the eye was on, and there is no way to aim one.

  What it wants is a cursor a hand can put somewhere and see -- the roll already
  answers `locate`, and a window already has a position -- rather than a second
  invisible one. The same anchor is what a cut would report and what a `q`
  would quantize toward, so it is one decision for the three of them.

  **Decided 2026-09-07 (the user): it is the transport's cursor, and there is
  only one.** The start of what is pasted lands where the cursor is, playing or
  not; while playing it lands at the cursor's position at the moment the paste
  was made. So this stops needing a cursor of its own and starts needing the
  roll to *have* the one every other view has -- a click on its grid background
  places it, which is what the roll cannot do today. The whole rule, its
  playing/stopped halves and the lane question a clip paste raises are in "One
  cursor, it is the transport's, and the content never moves it" (Future
  directions); this entry is the defect that made it concrete.

  **Closed with "One cursor, it is the transport's, and the content never moves
  it"**: `Ctrl`+`V` pastes at the window's cursor through `KeyInput::cursor`, and
  a press on the roll's grid locates it.

- ✅ **A marker added on the OSC lane names nothing, and the two clients
  disagree about it** *(found 2026-09-07 by the user, by eye, comparing
  `editors/pianoroll` with `editors/edit_notes`)*. `Ctrl`+click on the marker
  lane pushes `OscMark { time, label: None }` -- a marker with **no address**.
  A marker is the message it sends, so one with no address is a message that
  names nothing, and the wire carries it as an empty label.

  What the two clients do with it is the visible half. `edit_notes` routes the
  lane through the timeline's domain, which cannot build an `OscItem` without an
  address and refuses with a sentence naming the verb to call instead;
  `pianoroll` keeps its own list and counts what arrives, so the marker is
  accepted and will never send anything. Neither is wrong about the value it was
  handed: the host offered a thing the model cannot hold, and each client
  answered honestly.

  So the question is the host's, and it is the same one the wire settled for
  everything else -- a view emits what a structure can take. Either the lane
  refuses an add it cannot address (and says so, which is the entry below), or
  the address is part of the gesture, or a labelless marker is given a meaning
  of its own. It cannot stay a value only one client notices.

  **Fixed 2026-09-07 by removing the gesture, which is what the widget was
  built with** *(the user, reading the model back with me: "the piano roll is
  there to edit events that always have freq/degree/midinote ... the markers
  are not editable, they are read-only, I think that was the original
  decision")*. It was. `G24a` recorded it -- *"OSC events (which have no pitch)
  draw as flags in a separate lane below it"* -- and `G24c` recorded it again
  -- *"display-only for now: the `(time, label)` marker is a lossy view of the
  message, so writing it back would drop the args"*. The add/remove/move press
  was added against both and **nothing tested this lane's editing at all**,
  which is how a gesture contradicts a decision and stays.

  So the lane shows and does not write. A `Ctrl` press -- the one that meant to
  edit -- is refused out loud and consumed; every other press there goes back
  to the container, the way the axis strip beside it does, so a sweep still
  crosses the lane. The `"osc"` edit-back, the marker drag and the marker index
  in the hit go with it, and the compiler named each one as it became dead.

  **What reading the model together settled, and it is worth keeping**: the
  loss is the *view's*, not the model's. `OscItem` earns its place elsewhere --
  `render(Timeline)` with no destination bounces through `Session.nrt()`, where
  `OscItem.play` puts a timestamped message into the score, arguments intact,
  which is what an NRT score *is*. Nothing about that needed changing; what was
  wrong was a roll claiming to edit it through a label.

  **And "the OSC lane" is a name only this repository uses** *(the user, asking
  what it was, 2026-09-07: "Which is the OSC lane? ... in a piano roll there
  are only note lanes, a velocity lane, a marker lane")*. A roll has a notes
  lane, a velocity lane and a **markers lane**; the host calls the third one
  after what a marker *is* (`osc_lane` in the theme, `OscMark` in the model,
  "the roll's OSC lane" in the docstring) and the reader calls it after what it
  shows. Both are right and only one is on screen, so the name to use in prose,
  in a theme role and in an example is the reader's -- the same rule that makes
  a buffer a buffer and its contents samples. The internal type may keep saying
  what a marker sends; a lane is called the markers lane. Worth doing with
  whatever answers the question above, since both touch the same lane in both
  clients.

- ✅ **An undo leaves the pencil dead, and a dead pencil draws a selection**
  *(found 2026-09-07 by the user, by eye, in `editors/edit_samples`: "I did
  undo/redo and it did not let me draw any more")*. Reproduced with a trace.
  One stroke lands, `Ctrl+Z` and `Ctrl+Shift+Z` walk the history, and from
  then on **`Alt`+drag and `Ctrl`+drag both arrive at the client as
  `selection`** -- 54 of them in the sitting's log, and not one `draw`,
  `sample` or `refused`. It lasts for the rest of the session.

  The chain is four calls and each one is doing what it says:

  - a history step corrects the widget with `reload`, which is the mapped
    sibling of a `/gui_set data`: *it is where it always was, and it moved*;
  - `reread` answers by forgetting what it resolved -- `data.body = None` --
    so the loader picks the source up on the next pass;
  - with no body, `trace()` falls back to the inline samples, and a take backed
    by a server buffer has none, so `has_raw()` is false and `sample_value`
    returns `None`;
  - the press arm for the pencil needs that value (it is what the stroke
    replaces, and the inverse rides in the event), so it **declines**.

  And declining is `false`, which in this machine means *this press was not
  mine*: the press walks on down the chain and the plain plan sweeps a
  selection. So a widget whose bulk has been forgotten does not refuse a
  stroke -- it silently turns into a selection tool.

  **Two things to fix, and they are not the same size.** The first is the bug:
  either the reload must not drop what the gestures read, or the loader must
  refill it and the arm must wait rather than decline for good. The second is
  the rule the bug rode in on, and it is the host's own standard turned on
  itself: the zoom gate one screen above refuses out loud and comments
  `return true; // consumed: the plan must not fall through to a sweep` --
  every *other* way the same arm gives up returns `false` and does fall
  through. A plan that resolved to a gesture should consume the press whatever
  happens next, and say why when it cannot act. **Related:** "A refused edit
  springs back and says nothing" is where the saying-why half is decided.

  **Fixed 2026-09-07: there was no next pass** (`gui/windows.rs`,
  `web/bulk.rs`). The first of the two, and the cause was one line further on
  than the chain above reaches. `reread`'s own comment says the element forgets
  what it resolved *"and the loader picks it up on the next pass"* -- and there
  was no next pass. `load_bulk` is the only walk that reads `Needs::bulk` and
  the only thing that starts a buffer fetch, and it runs **once, when the
  window opens**. So a take told to re-read forgot its body and nothing ever
  asked the server for it again.

  Which also explains the half that made it hard to see: the window looked
  fine. A fill with no data returns `None`, meaning *nothing new for the slot*,
  so the GPU kept the picture it already had. The picture on screen stopped
  being the buffer's while the element behind it held nothing -- the user's own
  reading, *"undo/redo corrupts the state"*, and a better description than the
  one this entry was filed under.

  The pass now exists on both fronts, and what makes it affordable is that the
  ask is **one-shot**: `Element::wants_reload` is the mutable twin of
  `Needs::bulk`, and asking clears the ask, so a per-repaint walk answers once
  per `reload`. Deriving it from *"it has no body"* would have been a query per
  frame for as long as a fetch took -- an element waiting for an answer and one
  that has not asked look identical from outside.

  **What is left is the second half**, and it is now its own entry below: an
  arm that resolved to a gesture still returns `false` on every path but the
  zoom gate's, so the next thing that goes wrong inside one will fall through
  to a sweep exactly as this did.

- ✅ **A press arm that hit a fault let the press through to a sweep**
  *(split out 2026-09-07 from the entry above, whose bug it carried in; fixed
  the same day)*. `GestureMap::plan` resolves `ctrl -> alt -> shift -> plain`
  and a step that returns `false` means *this press was not mine*, so the press
  walks on down the chain. The pencil's zoom gate was the only place that
  consumed instead, and it said so:
  `return true; // consumed: the plan must not fall through to a sweep`. Every
  other way the same arm gave up returned `false` -- including the one an undo
  produced -- so a pencil that could not read its samples became a selection
  tool, silently.

  **This entry was filed with the wrong shape and the reading is worth keeping,
  because it was wrong twice in the same direction.** It said the host was
  breaking one published rule in most of its arms, and that the work was
  therefore mechanical. Reading the arms says otherwise: `docs/gui-protocol.md`
  specifies the two cases *separately and oppositely*, one line apart -- `sample`
  *"declines where a sample is not a thing on screen ... so `"sample select"`
  edits where the samples are visible and sweeps where they are not"*, and
  `draw` *"refused ... and consuming the press"*. Both were implemented as
  written, and there is a test pinning the decline. The general rule this entry
  asserted does not exist, and asserting it would have deleted a composition the
  protocol offers on purpose.

  **What is true is narrower and is the actual bug.** A step has two kinds of
  dead end and only one of them is the plan's business:

  - **nothing to act on, over a good picture** -- the samples are not drawn one
    by one, the view measures no second axis, the view starts before the take
    does. Not this gesture's press: it declines, the plan tries its next step,
    and that is what a composed plan is *for*.
  - **a fault** -- a view holding no samples at all, one that cannot hold an
    edit in flight. The picture is wrong, not empty. It is said out loud and the
    press is consumed, because falling through there is a pencil turning into a
    selection tool with nothing said.

  The distinction is now written where the reasons live (`gestures/press.rs`),
  stated in the protocol's gesture-plan preamble, and carried by one door
  (`effects::refuse`) so a new arm gets it by using it. The `sample` arm keeps
  its documented decline at the zoom gate and refuses its faults; the `draw` arm
  refuses both, as its own row always said.

  **And the second defect it claimed to close is not the same shape at all.**
  A press on an empty staff space answering with the engraver's drawing was
  filed here as an arm that meant to write and fell through. It is not: the
  score's press reaches the element, `data.hit` **resolves** -- it returns the
  staff -- and the note-entry branch is guarded by `picked.is_none()`, so it is
  never evaluated. That is hit-test precedence, a big box winning over blank
  paper, and it is its own work. Kept as the record of how a defect gets
  mis-shelved: two symptoms that look alike from the window ("the press became a
  selection") had nothing in common underneath, and the resemblance was mine
  rather than the code's.

- ✅ **The pencil drew before the samples were drawn, and wrote hundreds at a
  time** *(found and fixed 2026-09-07, by the user, by eye: "it is letting me
  edit the waveform before the samples' disks show, and a very large amount can
  be edited at once")*. Two numbers for one rule. The gate refused where a
  pixel was more than one sample; the trace puts its sample dots on at three
  radii of spacing (`graphics::signal::trace:: dots_fit`), which is twelve
  pixels a sample at the default density. Between the two a stroke was allowed
  over a picture with no dots in it.

  The two halves of the report are one gap: the samples were not being shown
  one by one, **and** a drag across the body wrote every sample it crossed --
  at a pixel a sample, nine hundred of them from one gesture, none of which the
  hand could aim at.

  The gesture now asks the drawing (`trace::samples_are_drawn`) rather than
  carrying a threshold beside it, so the pencil is allowed exactly where the
  reader can see which sample they are writing, and how much a stroke can write
  is bounded by that rather than by a number invented for it.

  **Everything needed to catch this was already written down.** The example's
  own instructions say *zoom in until the samples are discs, then draw*, and
  `docs/decisions.md` has a section titled *"A stroke writes what the reader
  can see"*. The principle was decided and documented; only the number
  disagreed. What let it drift is that **nothing tested the gate** -- the one
  gesture test over a pencil draws at a hundred pixels a sample, far inside
  both thresholds, so either number passed it. The general lesson is in the
  decisions entry: a rule stated in two places is a rule that will disagree
  with itself.

- ✅ **A refused edit springs back and says nothing** *(found 2026-09-03, while
  chasing "the clip does not respond": pressing `e` over a piano-roll clip is
  answered by a clear refusal nobody can see)*. The acknowledgement already
  carries the reason -- `/gui_ack`'s optional trailing string -- and the host
  parses it into `ack::Acked::reason`, where it stops: nothing reads it and
  nothing draws it. So an edit the owner declined, with a sentence saying
  exactly why, reaches the person at the window as a clip that did not move.
  `host/ack.rs`'s own module doc states the standard this fails: *"an edit that
  springs back with no explanation teaches 'sometimes it does not work' rather
  than 'not here'"*.

  What is undecided is **where a reason shows**, and it is a real question
  rather than a detail: a window has no status line of its own (the composer
  example has one, but that is the script's widget and a refusal is not the
  script's business), the corner slot is taken by the cursor read-out, and a
  notice that lingers is noise while one that vanishes is missed. The three
  candidates: a transient line in the window's own chrome, a flash on the widget
  that refused, or an event a client may draw where it likes -- the last being
  what the `"refused"` emissions already are for the gestures the *host* itself
  declines, which is an inconsistency of its own (the host refuses out loud and
  the owner refuses silently).

  **What this is *not* an excuse for** *(added 2026-09-03; the fix landed the
  same day)*: the refusal that raised it should not have existed. A clip is a
  view and what it holds decides what it admits, so a piano-roll clip refusing
  to split was a defect of the client, not a message with nowhere to go -- see
  "Which edits a clip admits is asked of `form`'s classes, not of the data"
  (`clients/python/PLAN.md`, Found by use), now closed: a roll clip splits,
  joins and trims. What is left for this entry to display is the honest
  remainder -- a generator that has not been rendered, and a join across
  timelines the document cannot store yet -- which is what the decision should
  be sized against, and it is a much smaller thing than the day it was
  written.

  **Measured on the pencil, 2026-09-07** *(the visual review's second sitting,
  `editors/edit_samples`)*. The third candidate is no longer a hypothesis: the
  host refuses a stroke below one pixel per sample and **says so on the wire**
  -- `refused draw "zoom in to draw: one pixel is N samples"` -- and the event
  arrives at the client, which drops it. The debug trace of a sitting reads
  `event 1000 'refused' -> no payload` and then nothing, because a tag no domain
  claims is nothing rather than an error, which is the right rule for an edit
  and the wrong one for a refusal. So the person at the window gets a pencil
  that draws sometimes and explains never, and the reasonable conclusion -- the
  one the user drew -- is that it broke and will not come back. That is the cost
  the entry was written against, now with a number on it: the sentence existed,
  was correct, was sent, and was thrown away one process later.

  **And the owner's half, the same afternoon** *(`editors/edit_notes`)*. This
  entry was written about an owner's refusal, and that is now measured too, in
  the case the example's own docstring promises: a `Ctrl`+click on empty space
  in the marker lane cannot add a marker, because a marker is the message it
  sends and a roll has no way to type an address. The owner declines, corrects
  the widget back, and the acknowledgement carries

      reason='a marker is the message it sends, and a roll cannot say which:
              add it to the timeline with its address, then drag it here'

  -- a sentence that names what to do instead. *(It named the Python call when
  this was written; the sentence became the crate's with `O27` and had to stop
  naming a language, since one string now reaches a page and a script alike.)* What reaches the person at
  the window is the correction springing the picture back: **a line that
  flickers on each click and nothing else**, which is how the user described it
  before being told what the wire had said. Both halves of the entry now have a
  witness, and they fail identically from the outside.

  It also says where the smallest fix is. `"refused"` is a tag every view can
  emit and no view can mean as an edit, so it belongs with the tags an editor
  answers itself rather than routes -- beside `NOT_AN_EDIT`, not inside a
  domain -- and what an editor does with one is the same open question as
  before. **Related:** "The routing table's tag list is written twice"
  (this file, Found by use) is the list it would join, and since
  2026-09-09 that list is the crate's (`clausters_document::view::NOT_AN_EDIT`),
  so joining it is one line in one place rather than one per client.

  **Fixed 2026-09-07: the status bar** (`host/status.rs`). The decision the
  entry was waiting on -- *where a reason shows* -- is answered, and by the
  candidate none of the three named: a band the **host** draws along the bottom
  of every window, saying what it last did and what it last refused. It is not
  a widget, it is not in the tree, and **no line of it crosses the wire**, which
  is the whole of why it settles the question the client-side candidates could
  not: the host already knows what it just refused, so routing that fact out to
  a client and back would be telling the host something the host said first.
  A window that wants the pixels back says `status: false`; it is on by
  default, because a bar nobody turns on is a bar nobody hears from.

  Both measured halves are now visible from the same two lines of code. The
  host's own refusals are read at `gestures::effects::emit`, the one place a
  `/gui_event` is produced -- so `refused draw "zoom in to draw: one pixel is N
  samples"` lands on the bar as it goes out, and every other verb with it. The
  owner's reason is read at `Host::settle`, which makes the status bar **the
  first reader `ack::Acked::reason` has ever had**: the field the protocol
  always carried and the mechanism deliberately does not need. Closed the bar
  shows the newest line, in the warning role when it reports a refusal; a click
  opens it into the window's log area (128 lines kept, the wheel scrolls back
  through them) and another closes it. Consecutive lines from one widget with
  one verb replace rather than stack, because a drag emits per motion.

  Two rules made it geometry rather than an overlay, and both are one function
  each so they cannot drift: `Host::content_area` carves the band for **both**
  the renderer and the hit test, so a widget is hit on the pixels it was drawn
  on; and the band is chrome, so the press and the wheel are consumed there and
  never reach the tree.

  **What is left is a crumb, not the entry.** The client still routes a
  `"refused"` tag into a domain that cannot claim it (`Editor._route`), which
  prints `event N 'refused' -> no payload` in a trace and does nothing -- noise
  now rather than a lost message, since the host displayed it before the client
  ever saw it. Putting it beside `NOT_AN_EDIT` is still the tidy answer and is
  no longer urgent -- and it is now cheaper than it was, since that list lives
  in the document crate and a tag added there reaches both clients at once.

- ✅ **Four views drew one gesture four ways, and one of them drew nothing**
  *(found 2026-09-03, on the report that a lane's marquee draws no rectangle;
  fixed the same day)*. Every view that lets a hand sweep a selection had
  written the drawing itself, and the four had drifted apart: the signal views
  drew the band with the half-sample rule and with the value restriction the
  sweep had set, the roll drew a full-height stripe from the raw sample and
  **threw away the pitch band it had just swept**, the multitrack lane drew
  nothing at all -- so a marquee over it looked like a gesture the window had
  ignored -- and the patcher drew its own rectangle in its own alpha.

  One routine now (`graphics::selection`), and the difference between the views
  is a value rather than a copy: `Vertical` says what the second axis measures,
  which is the only question a sweep asks that a view answers differently, and
  it decides whether the sweep is a stripe or a rectangle cut out of one. The
  edges follow from the same answer instead of a flag -- a full-height band owns
  its two vertical edges, since its top and bottom are the lane's own, and a
  restricted one owns all four because every one of them is a value the hand
  chose.

  **And the rectangle has a second axis on a lane stack too**: a sweep down the
  lanes takes the clips of every lane it crossed, through `Bands::window` and
  the stack read at the press -- the same call the roll makes for the semitone
  rows a rectangle crosses, which is what one vertical axis is for. The drop
  happens once for all of them; cleared per lane, the selection would keep only
  what the lane the sweep ended on had.

- ✅ **The swept time band is the whole group's, so a sweep over two lanes of
  five paints all five** *(found 2026-09-03, writing the above; fixed the same
  day)*. The time
  selection belongs to the navigation group -- it is the loop span, and every
  linked view draws it -- so the stripe correctly appears on every lane, while
  the *clips* the sweep took are only those of the lanes it crossed. The
  picture and the hand therefore disagree about the vertical half of the
  rectangle. Making them agree means the group holding **which lanes** the
  sweep crossed beside the span it swept, as an index range into the stack;
  what has to be decided first is whether that range is host-internal (set by
  the sweep, dropped by any `/gui_set` of the selection) or a property of the
  axis on the wire, which is a protocol change and reaches both clients.

  **Decided host-internal**, and the reason is the one `host/bands.rs` already
  states for the vertical in general: *the vertical is the container's, never
  the group's*. A span on the wire is a fact two clients can both mean -- it is
  the loop region, and a script sets it -- while "how far down this window's
  stack a pointer travelled" is not something a client that never swept has
  anything to say about, and a prop nobody can honestly write is surface both
  clients would have to carry. So `TimelineGroups` holds `sel_lanes` beside the
  states, `Host::select_lanes` is written by the sweep immediately after the
  span, and `set_timeline_selection` -- the one door every selection goes
  through -- drops it, which makes a `/gui_set` the whole stack again without a
  rule of its own. The lane asks `lane_shows_selection` before it draws, so a
  view that is not a lane of the swept stack (a waveform linked to it) is
  untouched: it asks with its own id and no range names it. A redefine
  reallocating ids is handled where the members are already being walked
  (`sync_timeline_groups` prunes a range to the lanes still in the group and
  drops one left with none), because a range naming lanes that are gone would
  hide the band everywhere -- a selection plainly there and drawn nowhere,
  which is worse than the disagreement this fixes.

- ✅ **The auto-fit was taught door by door, and the doors kept disagreeing**
  *(found 2026-09-03, on the report that a view still re-framed itself with
  `autofit` off; fixed the same day)*. `autofit` first landed as a guard inside
  `set_timeline_total_inner`, and every other way the content reaches a window
  kept the rule it had written for itself: `set_timeline_offset` refitted by a
  test of its own (and clamped against the raw total instead of the navigable
  span), the redefine's reseed in `sync_timeline_groups` re-clamped the window
  it had just decided to keep, `set_timeline_link` re-clamped and never brought
  `GroupState::total` in step, and `follow_timeline_end` paged the axis forward
  because paging is not spelled "refit". The visible defect was the smallest of
  them: with the switch off, a piece that got *shorter* still pulled a window
  back onto its content.

  Now there is one `Host::content_moved` and every door goes through it: it asks
  `group_autofit` first, and with the switch off it registers the extent and
  touches nothing -- not the refit, and not the clamp either, since the clamp
  belongs to the hand that navigates. The "was it showing everything" test is
  one predicate (`Host::showing_everything`), asked in the same words by the
  extent door and the redefine. Tested: a pinned window survives a shrink
  through each door, one member that does not follow its content pins the group
  (a waveform linked to a lane), and a roll with the switch off does not page
  forward under a take being written into it.

- ✅ **The score's element hit is a linear scan over the whole page** *(found
  2026-09-03, measuring the vertical axis for a different design and checking a
  premise that the score's hit was the scalable case)*. `ScoreData::hit`
  (`graphics/score/cursor.rs:242`) walks every box the page engraved, keeps the
  ones that hold the point and takes the smallest by area; `entry_at` (`:271`)
  is a second walk of the same shape. The `partition_point` in
  `graphics/score/tess.rs:171` is not this at all — it is the playhead's search
  over the timemap's cursors, which is why the scan is easy to miss.

  Measured with that exact shape, per click:

  ```
        200 boxes       158 ns      <- a real page
      2 000 boxes     1 045 ns      <- a dense page
     20 000 boxes    10 313 ns
    200 000 boxes    75 907 ns
  2 000 000 boxes 1 318 750 ns      <- 1.3 ms
  ```

  **This is correct today and is written down for the day it stops being.** The
  widget holds one engraved page (`elements/score.rs:62-82`) — the client
  re-engraves and sends it — so the view never holds the work, and at page
  scale the scan costs less than a microsecond. It becomes the thing to look at
  first the day a page stops being a page: a very dense orchestral system, or a
  continuous unrolled view. The fix, if it is ever needed, is a coarse grid or
  an interval tree over `hits`, built in `index()` where the boxes are already
  being walked — not a change to the hit itself.

  The reason it is filed rather than fixed: the answer to "a score can have
  millions of elements" is already in the design, and it is not a faster index
  — it is that the view holds a page and the work stays in the client.

  **Fixed 2026-10-06, at the user's call**, since a page stopped being a page:
  the page view stacks every page of the score in one widget, and the
  continuous view is one system as long as the score. The hit itself had
  already gone through a grid (`HitGrid`, `graphics/score/hit.rs`); what
  still walked the page was everything else a press or a frame asks.
  `entry_at` -- the press of note entry -- walked every box and measured each
  one's nearest staff by walking the staves, and the selection's highlight,
  a drag's ledger lines and the edit cursor each found an id by a walk, the
  highlight once per selected id per frame. Now `index()` builds, beside the
  grid, each id's boxes (`by_id`) and each staff's boxes in order across
  (`rows`); `entry_at` searches its staff's row outward from the press and
  stops where nothing nearer can be, and the nearest staff is a binary search
  over the staves, which are top to bottom and apart. Per press, note entry
  and the hit together, measured on pages of two-staff systems:

  ```
                   before       after
        410 boxes   7.0 us      0.97 us
     40 100 boxes   1.4 ms      1.6 us
    800 400 boxes  98.8 ms      7.9 us
  ```

  A test reads the indexes against the walks they replaced over a dense page
  (`the_indexes_answer_what_a_walk_over_the_page_answers`).

- ✅ **A session host draws no lane mixing, and its header controls reach
  nothing** *(found 2026-09-02, closing the Python client's `C48`; fixed
  2026-09-02)*. The host emitted the three tags from a lane header and carried
  the configuration across a save, and did neither of the two things in
  between: it did not **read** the mixing (a lane was a label and the time
  chrome, so a piece muted in a client opened audible) and the lane widget was
  bound to no node (only its clips were), so a press on a header named nothing.

  Both pieces landed as the entry described. `tree.rs` draws `mute`/`solo`/`level`
  from the node's configuration — **only the keys the document holds**, since a
  header is presence-driven and a `false` this driver invented would grow a
  button the piece never asked for — and `read_event` turns the tag into a
  `Configure`, the same intent a client emits, so the undo comes out of the
  document identically whoever made the edit. A configuration is replaced whole,
  so it starts from what the node carries: a fader cannot erase a mute.

  **Two things the work forced, and both are the same shape as a bug already
  fixed for clips.** A node can be drawn *twice* — a lane holding one element is
  that element's header and its clip — so "which widget is this node" has two
  answers and the bindings are two maps (`Drawn::bindings`, `Drawn::headers`),
  rather than one where a `HashMap`'s iteration order decides which picture an
  undo moves. And `adopt` had to learn `Configure`: without it an undo of a mute
  moved the document and left the button down, which is exactly what the clip's
  own "an undo that moves the model and not the picture" comment is about.

  A lane's `height` stayed what it was: the view's, carried by nothing.

- ✅ **A tie between two different pitches is accepted, written, and dropped by
  the engraver with a warning nobody reads** *(found 2026-08-29, round-tripping
  an emitted document through verovio while building the reader; fixed
  2026-08-30, walked into again by `N5`'s editor example, where a **tie** button
  does nothing on any two notes that differ — which is most of them)*. `tie` sets
  the flag on any item; the emitter writes `@tie="i"`/`"t"`; verovio answers
  `Unable to match @tie of note 'n3', skipping it` on stderr and hands back a
  document with no tie in it. A tie joins two notes of the **same** pitch — that
  is what a tie is, as against a slur — so the model should refuse one that does
  not, at the verb rather than at emission, with the sentence saying which two
  pitches it was asked to join. Until then a caller can write a tie that is
  silently not there, and the only sign is a warning on a stream no client
  shows. Found while chasing a different suspicion (that ties were being lost),
  which turned out to be verovio respelling them as `<tie>` elements — a real
  finding, and this one fell out beside it.
  **Fixed as written**: `edit::tie` refuses it, naming both pitches and saying
  that a line between different ones is a slur. What the entry did not say is
  the one case that has to stay legal — a **chord** holding one of its pitches
  over — so the requirement is that the two items share *a* pitch, not all of
  them.

- ✅ **The props manifest compares wire types, so a divergence between two
  builders of one type is invisible to it** *(found 2026-08-28, reading
  `gui/guidef.py` against `gui/guidef.ts` by hand and finding five differences
  the green test had never seen)*. `test_gui_props.py` folds every builder of
  one wire type into one union — "`waveform`, `plot` and `scope` all build a
  `signal`", and the comment says so on purpose — and `generic_props()` drops
  the place props, `theme`, `flow` and `axes` out of the comparison entirely.
  Both choices are right for what the test is *for*: it guards the three
  **surfaces** against each other, and a union is the surface. But the clients
  are compared builder by builder by a reader, and there the folding hides
  exactly the class that drifts: `layout` could not say `hug` while `panel`
  could (same type, `layout`), `scroll` took only the old name for `flow` while
  `plane` took both (same type, `plane`), the spectrogram had no
  `sel_min`/`sel_max` while the waveform did (same type, `signal`), the ruler
  took no `theme` (a generic prop, excluded) and only two builders in the whole
  web client took `axes` (structural, flattened at the host's door and
  excluded). All five shipped; none of them ever turned the test red, and
  nothing else would have caught them.

  **Two readings now sit beside it, and they answer different questions.** The
  first is static and keyed by builder name, over the two clients only (the
  host has no builders): `docs/gui-props.md` grew a second table for it, and
  `test_gui_props.py` holds it the way it holds the first — undeclared,
  stale and wrong-surfaces. It came out at eighteen rows, all `idiom` and all
  one story: TypeScript declares a bundle once in an interface (`ContainerOptions`'s
  `layout`, `TimelineOptions`'s selection and playhead) and every builder that
  extends it inherits members it does nothing with, while Python names props
  per builder and names only the ones each acts on.

  The second is **executed**, which neither manifest reading is: `gen-gui-vectors.py`
  now also freezes a sweep — every builder crossed with every option its Python
  signature declares, one at a time — and `gui-parity.test.ts` rebuilds all 469
  of them with the web builders and compares the documents. That is what
  catches a prop landing under the wrong wire key, or as `true` where the other
  writes `1`: names that match on both sides and mean different things, which
  no static reading can see. It is Python-driven, so it covers what the web
  client is missing and the static table covers the other direction.

  Checked by breaking each one on purpose: putting the pre-fix `scroll` back
  fails `GuiDef parity: scroll(flow)`, and taking `sel_min`/`sel_max` off the
  Python spectrogram fails the builder table. What neither guards is a
  **generic** prop a builder does not name — `view` was the one container
  whose signature omitted `color` — because both readings exclude the generics
  by construction and the sweep is generated from the signature that would be
  missing it. That is naming rather than capability (Python reaches every
  generic prop through `**props`), so it is left as it is rather than guarded.

- ✅ **Nothing resizes a window, so nothing tests a squeeze** *(named 2026-08-26,
  after a drag on a corner panicked the host in `ruler::draw_ticks_v` — see the
  commit; what is left here is the class, not that bug)*. Every suite draws into
  a mesh at a size it chose, and the examples open at the size their GuiDef
  declares. So the whole family of states where a widget is **smaller than its
  own contents** — a lane under a line of text, a body under its rulers, a strip
  under its widest label — is reachable only by a hand on a corner, and only on
  the native front. Each of those is arithmetic that has a lower bound nobody
  states, which is how one of them became a panic between four pixels and ten
  and a half.

  What would cover it is not a window: the drawing entry points are pure
  functions over a `Mesh`, so a test can walk a view's rect down through the
  sizes that matter and assert only that it draws and does not panic — the
  shape the regression test for that crash already has, generalized to the
  widgets that have a floor. Worth deciding whether that is one shared test
  over the element registry (every registered element, a shrinking rect) or a
  case per widget, and whether the browser front needs its own, since the
  compositor there gives sizes the native one does not.

  **Closed 2026-08-26 as one table**, `frame::tests::every_widget_survives_being_squeezed_to_nothing`.
  Both open questions answered, and neither the way the entry guessed:

  - **One table, not a case per widget.** A case per widget could assert more
    — *what* a squeezed widget drops, and in what order — but nothing would
    oblige anyone to write one, and a widget added next year would not have it.
    A row is one line, which is the property that keeps the coverage from
    rotting; what it asserts is only the floor.
  - **The browser front needs nothing of its own**, and that is not a
    concession: the frame's mesh pass *is* the same Rust compiled twice, so a
    squeeze is the same arithmetic on both fronts. A separate browser test
    would be testing wgpu.
  - **Not the element registry either.** The walk goes through the **frame**,
    because that is where the crash was: a heavy view's chrome is drawn by the
    frame's own pass, and an `Element::draw` test would have missed the very
    panic that prompted this.

  **Two things keep it from being vacuously green**, which is the failure mode
  of a table: a row that does not build as the type it names is a failure (a
  typo would otherwise build as `Unknown`, draw nothing and pass), and every row
  must draw *something* at a comfortable size before the walk begins. And the
  test was verified by reverting the ruler fix and watching it fail.

- ✅ **The host's own theme is a wasm export the protocol has no verb for**
  *(found 2026-08-25, porting `panels/style.py`; fixed the same day)*. Exactly the typeface's case,
  one entry below, and found the same way — by an example that could not be
  written. The colors a host draws its chrome from overlay at three levels: the
  host's own table, a `theme` group on a container, a widget's `color`. The last
  two are props and both clients have them; the **first** is a launch flag
  (`--theme file.toml`, `[gui.theme]`) on the native front and
  `GuiBridge.theme(json)` on the browser one, with **no `/gui_*` verb** — so a
  page can reach under its client to the bridge and a script cannot do it at all
  after launch.

  Three ends, as the font's had:

  - **The host**: `/gui_theme <json>` — the same partial table the prop takes,
    scoped to the host rather than to a subtree, on either front. The machinery
    is there on both sides already (the browser export *is* this, and the native
    launch path parses the same table).
  - **The reference client**: `GuiHost.theme(table)` over that verb, beside
    `GuiHost.font(face)`.
  - **The web client**: the same `GuiHost.theme(table)`, so the bridge stops
    being where a capability is reached.

  **Done, and it was two verbs rather than one.** `metrics` sat right beside
  `theme` in exactly the same shape — a `[gui.metrics]` config table, a
  `GuiBridge.metrics(json)` export, no verb — so `/gui_theme <json>` and
  `/gui_metrics <json>` landed together, in `host/mod.rs` where every other
  command lives. Both overlay the host's table, re-resolve what depends on it
  (a theme group is its own table over the *inherited* one, so the groups in
  every open window are resolved again) and return a `Redraw` per window; both
  report an unknown role and skip it rather than refusing, as the launch-time
  tables do. The two wasm exports are **gone**, which is what happened to
  `font` when `/gui_font` landed: one door, both fronts, both clients
  (`GuiHost.theme(table)` / `.metrics(table)`).

  `msaa` is the one left in that shape and is not the same case: every pipeline
  in a render pass agrees on the sample count, so it is read when a device comes
  up rather than being a live property of the host. It stays a launch flag on
  both fronts, and `panels/style`'s two examples say so where their pictures
  differ — the script's rounded corners are smooth and the page's are stepped.

  `clients/web/examples/panels/style.html` is written now, and `style.py` hands
  its host table over with the verb instead of writing a TOML file to boot with.

- ✅ **A typeface is a wasm binding, so only the browser can change one**
  *(found 2026-08-23, reading the web book's "What the browser changes" against
  the reference client)*. The browser front takes a face at **runtime** —
  `bridge.font(bytes)`, `src/host/web/bridge.rs` — and the native front takes
  one only at **launch**, from `--font <path>` or `[gui] font`
  (`src/host/fontfile.rs`). There is **no `/gui_*` verb** for it at all, so the
  difference is not a client's to close: it is a wasm export that grew surface
  the protocol never had, which is the case the standing rule names outright
  ("a binding, a door, a wasm export or a convenience wrapper may not grow
  surface the other client lacks"). The web book documents it as a prestation,
  with example code, which is how it went unnoticed.

  Three ends, and the first decides the rest:

  - **The host**: a wire verb (`/gui_font <blob>`, the bytes beside it as a
    `/gui_def`'s do) so a face can be handed over after launch on either front.
    The native side already has the machinery — `fontfile.rs` parses into the
    rasterizer's tables and drops the bytes, so the source is a `&[u8]` either
    way. Loading one relayouts nothing (the sizing table never followed the
    typeface), which is what makes a late hand-over safe.
  - **The reference client**: `GuiHost.font(bytes)` over that verb, and
    `GuiProcess(font=...)` for the launch flag it does not surface today (a
    Python script passes `extra_args=["--font", path]`, which is the launcher
    admitting it has no opinion).
  - **The web client**: the same `GuiHost.font(bytes)`, so a page stops reaching
    under the client to the bridge for it — and the book's section stops being
    the place a capability is announced.

  Until then it is a **known divergence** rather than an oversight, and it is
  the one thing in that section that is not platform: a browser has no
  filesystem and a desktop has no `fetch`, but *when* a face may be handed over
  is a decision, not a platform.

  **Done 2026-08-24, and the first end is what settled the other two.**
  `/gui_font <blob>` is the verb: raw TrueType/OpenType bytes, **no id** — a
  face is a property of the host and not of a window, which is also what makes
  a late hand-over safe (nothing relayouts; every open window redraws, and the
  effect is the `Redraw` the protocol already had). A build without the
  rasterizer, and a face it cannot read, both log and keep drawing with the
  embedded bitmap one — the floor every build draws on, so the verb never fails
  a client. Both clients got the same call in the same place, `host.font(bytes)`
  beside `load`, and `GuiProcess(font=…)` surfaces the launch flag the launcher
  had no opinion about.

  **The wasm export is gone rather than kept beside it**: `GuiBridge::font`,
  `WebEvent::Face` and `FetchedFace` were the door under the client, and leaving
  them would have been two ways to do one thing on one front only. What survives
  the change is the platform's genuine half — a page fetches the bytes, a native
  host maps a file — and the web book's "What the browser changes" is one entry
  shorter for it. Watched by hand: a windowed host given DejaVu Sans Mono over
  the wire redrew with it, and fourteen junk bytes warned and left it drawing.
- ✅ **A blob has no live door, so a page cannot change the samples it drew**
  *(found 2026-08-23, porting the `Source` to TypeScript; fixed 2026-08-25,
  porting `views/window.py`, which is the example that could not be written)*. `/gui_set` takes
  `data` (inline samples) or `reload` (re-read the `path` a widget was built
  with), and that pair covers a native client exactly: short samples ride
  inline and change inline, long ones spill to a file the client rewrites in
  place. A **page has no file** — its bulk carrier is a blob beside the
  `/gui_def` — so a source past the inline ceiling has no way to change what a
  live widget draws, and the client refuses rather than pretending. The fix is
  the host's: either a `/gui_set` that carries a blob argument, or a browser
  `path` the wasm front can re-fetch (`host/fetch.rs` already fetches one at
  define time; `reload` is what it does not do). **Done, and it was the first of the two**: `/gui_set` takes a
  **blob** in a value slot now, expanded to exactly the array the inline `data`
  prop would have held, so nothing downstream learns a second shape — a live
  view's samples are replaced by the path that already replaced them. A ragged
  length (not whole `f32`s) is dropped rather than read short.

  With it, the two clients say the same thing about a source: the carriers that
  **hold** samples take a `set` and the ones that **name** them do not, which is
  `data`/`path` there and `data`/`blob` here — the platform pair the reform
  already recorded. The carrier stays **fixed** for a source's life on both
  sides: an inline source handed more than it can hold refuses rather than
  promoting itself, because a widget on screen was built around it.
- ✅ **The host's binds follow the server's rule: every carrier takes an
  address, loopback by default** *(named 2026-08-23, done 2026-08-24 with the
  server's half)*. The rule and the table live in the root `PLAN.md`'s "Found by
  use", which owns it — both fronts changed together. The host's half: `--ws`
  bound `0.0.0.0` and now takes `--ws [[addr:]port]` defaulting to loopback, the
  TCP leg **stayed** loopback, and the UDP leg grew the flag it never had
  (`--udp [addr:]port`) so the three read the same. What the host contributed to
  the diagnosis is that its `bind_ws` and the windowed front's `run` each wrote
  an address into their own body, one of them disagreeing with the headless
  path's neighbour two functions above; they take a `SocketAddr` now and decide
  nothing.
- ✅ **A clip's curve body drew nothing, in both fronts.** `Element::draw_body`
  paints nothing by default and `Curve` never overrode it, so an automation
  clip — the multitrack editor's whole envelope lane — was an empty rectangle
  everywhere. The body was built, placed and collected; the pass called a
  no-op. Every test drove `Curve::draw` with a `TimeSpace` instead (the
  standalone door taking its body-shaped branch), which is exactly why it
  survived: two doors that look alike and only one of them wired.
  `a_clip_body_draws_the_line` now goes through the one a clip uses.
- ✅ **One `editable` for two bodies, so a read-only roll locked the curve over
  it** *(found 2026-08-22 by the user, looking at the composer example: "the
  bass and sweep lanes cannot be edited")*. A simultaneous aggregate draws as
  one clip with its members' bodies **layered**, and both bodies read **one
  props map** — so the roll's `editable: false`, which is correct (a `Clang`'s
  notes are a rendering), reached `curve::body` as well. The envelope drew and
  refused every press. The clip stayed draggable throughout, which is exactly
  why it read as "the lane does not edit" rather than as a dead widget: a
  read-only curve answers `false` from `layer_hit` and the press falls through.
  The `/gui_set` route had already decided what `editable` *means* — "it reaches
  every body the clip carries, because it is a statement about the clip and not
  about one of them" — so the build path reading it per body was the half that
  never got the memo, and the client saying a body's business with a clip-wide
  key was the other half. Fixed with the split `min`/`max` already had from
  `points_min`/`points_max`: `notes_editable` and `points_editable`, each
  overriding `editable` where given, on both paths, in both builders and in
  `docs/gui-protocol.md`.

- ✅ **A curve jumped under the hand editing it** *(found 2026-08-22 by the
  user, dragging the composer's envelope: "on moving the envelope's point or
  line, the drawing jumps position")*. A break-point's place on screen is its
  value **against the clip's value axis**, and both clients derived that axis
  from the break-points on every redraw (`_curve_range`: their own range plus a
  tenth of headroom). So an edit rescaled the picture: dragging one point moved
  *every other one*, and the point under the cursor was the only one that
  appeared to stay put. Nothing in the host was wrong — it drew what it was
  given, twice, correctly. The axis is now **remembered** per `Automation`,
  screen state exactly as the edit layer is, and widened only on the side that
  stopped holding the data (a script's replacement, an undo of a taller curve);
  never narrowed, so a point dragged down and back up leaves the drawing where
  it was. In both clients, with the test in both.

- ✅ **A gesture reported a value per frame, so a drag was a hundred edits and
  all but the first were refused** *(found 2026-08-22 by the user, dragging the
  composer's envelope: the point teleported, the segment "it trembles in place",
  and it started as soon as the pointer left the window and came back)*. A clip,
  a roll and a curve each emitted their whole edit-back on **every drag step**.
  Two things follow, and the second is the one that showed: an undo history of a
  hundred entries for one bend, and — since every event names the last version an
  acknowledgement reported — a hundred round trips that a hand outruns, so every
  frame after the first named a version the owner had moved past, came back
  **refused**, and was answered with a resync that snapped the picture to the
  first frame. Measured in a real browser: a burst of 24 moves left the host at a
  bend of ~-3 and the model at -0.55, with `apply` answering `false` down the
  line. Native survived it only because its round trip happens to keep up with a
  hand; the page does not, which is what made it look like a browser-only defect
  and is why the shell was searched first (winit's coalesced `offsetX` was
  measured in Chrome and is *not* wrong — that hypothesis was tested and
  dropped).

  The host already had the rule, written at two other gestures: *"one gesture is
  one edit, and what the hand did on the way is the pending drawing's business
  rather than the owner's"* (`Drag::Draw`, `Drag::Sample`). The clip, the roll
  and the curve now follow it — the picture follows the hand, the edit leaves
  whole **on the release** — and `docs/gui-protocol.md` states it for every
  payload, along with what is still reported as it goes (a view pan, a
  selection, a layer change, a bound control) because none of those is a
  document edit.

  The clients gained the second half: a run of edits from **one widget** naming
  versions the editor has already moved past is that widget's own gesture, not a
  stale edit, so a host that does report as it goes still edits correctly. What
  ends a run is anything else moving the document — another widget, a script, a
  history step, a `refresh()`. *(The user's own note, kept as an open question:
  an acknowledgement should not be lost to saturation in the first place. That
  is the carrier's, and is not what this entry fixed.)*

- ✅ **A bent segment drifted out of phase with the hand** *(found 2026-08-22 by
  the user, right after the release-only rule landed: "when the cursor leaves
  the area it keeps adding up, the original offset is lost and the cursor drifts
  from the movement -- this already happened with other graphic elements")*. The
  bend was **incremental**: each step measured from the last, re-anchored every
  frame, which the comment defended as "like a knob". A curve is not a knob — a
  knob is dragged under a locked pointer with nothing on screen to stay level
  with, while a segment is a shape the hand is pointing at — and the relative
  form drifts twice over. The clamp **eats motion**: drag past the limit and the
  steps spent beyond it are swallowed, so coming back leaves the bend short by
  however far it went. And a pointer that has left the element keeps
  accumulating whatever motion still arrives, so the shape is out of phase from
  then on. Now anchored at the press (`bpf::bend_curve` sets rather than adds,
  from the curvature the press found), so a given cursor position has one
  answer: leave the area, come back, and the shape is where the pointer says it
  is. The user's note that this has been seen on other graphical elements is
  worth acting on — **any drag whose value is accumulated per frame rather than
  measured from the press has the same two defects**, and the knobs and numbers
  are the ones to read next (they may be right: a locked pointer has no absolute
  position to measure from).

- ✅ **An undo of a curve edit moved the model and told the host nothing**
  *(found the same day, and only visible once a curve could be edited at all)*.
  A **layered** clip draws an aggregate, and since the curve edit was fixed to
  name the member that carries the automation, the widget an undo has to correct
  is no longer the one the edited element is registered against: `_widget_of`
  searched the clips by their own element and the rolls, found neither, and
  answered `None`. So the inverse was applied, the `Automation` stepped back,
  and the picture went on drawing the shape the hand had left — a dead button
  with the sound and the drawing disagreeing. It now falls back to the clip
  whose aggregate **holds** that member. In both clients, with the test in both.

- ✅ **A browser delivered a press per frame, so every gesture re-anchored
  itself** *(found 2026-08-22 by the user, after the bend was made absolute:
  fixed on the desktop, unchanged in the page — "I find it odd that it happens
  on only one of the two platforms, because the implementation MUST BE THE
  SAME", which is exactly right and is what found it)*. Winit's web backend
  turns any `pointermove` **carrying a button** into a synthesized `MouseInput`
  whose state is *pressed* while that button is down (`window_target.rs`: `let
  state = if buttons.contains(button.into()) { Pressed } else { Released }`),
  and `PointerEvent.button` is `-1` on a plain move only where a browser follows
  the spec. So a drag arrived as a **fresh press on every frame**, the front
  took each one, and every press-time decision was re-run: a bend's origin, a
  note's press time, a clip's grab sample, all re-anchored to wherever the
  pointer had got to. That is the incremental drift the absolute forms were
  written to end, coming back through the door beside them — and it is why the
  page kept behaving like the code before the fix while the desktop, whose
  stream cannot repeat a press, behaved like the code after it.

  Fixed in the **machine** rather than in the browser front (`Gestures::press`
  returns early while a drag is in flight), because the single-pointer rule --
  one press, one drag, one release, already stated for fingers in the touch slot
  -- is the machine's and not a platform's: a press arriving mid-drag is never a
  new gesture, whether a stream repeated it or a second button was chorded onto
  the first. `a_press_repeated_mid_drag_does_not_re_anchor_the_gesture` drives
  the browser's stream against the desktop's and asserts one answer; without the
  guard the repeats swallow a third of the bend. Five older tests modelled a
  *click* as a bare press and now release first, which is what a click is.

- ✅ **An acknowledgement should not be lost to saturation** *(the user's own
  note, 2026-08-22, taken while the release-only rule was landing: "anyway the acks should not fail from saturation. That has to be looked at later too")*. **No acknowledgement was ever lost.** Both legs are reliable and
  neither drops: natively the host speaks TCP by default and a page's carrier is
  two in-process queues with no bound. What the note saw was an edit **refused**,
  and the refusal was the staleness check turning the round trip's own latency
  into a conflict.

  The two sub-questions the entry left open, answered rather than fixed. *Could
  the ack path be delivered in the same turn?* No, on either platform, and not
  by plumbing: the page drains the host's outbox on a **33 ms interval**
  (`page.ts`) and feeds the answer back through winit's event-loop proxy, so a
  round trip is two queues wide; native is a socket. *Should a host that is
  behind be told to stop emitting?* No — back-pressure trades a refused edit for
  a stalled hand, which is the wrong way round for a gesture that tracks the
  pointer, and it is unnecessary because an edit-back payload is absolute and
  whole, so a late report simply wins. The answers lag by construction, and the
  mechanism has to be correct with them arbitrarily late.

  So the fix is in the check. The host stamps every event with the version it
  was last *told*, so an event naming a version the owner has moved past is the
  ordinary case, not a collision. The owner now keeps a **floor** — the version
  at which the document last moved by a route no event produced (a script, a
  second editor, a re-derivation, a history step) — and refuses only an edit
  naming something below it; every other older version is one of its own answers
  still in flight. The rule it replaces (*a run of edits from one widget is one
  gesture*) was the same insight scoped too narrowly: it saved the drag it was
  written for and still refused **the next gesture whenever two began inside one
  round trip**, which is well inside 33 ms of hand. In both clients, with
  `two_gestures_inside_one_round_trip_are_both_applied` in both, and the wire
  rule rewritten in `docs/gui-protocol.md`. The trade-off, stated because it is
  the reason the narrow rule existed: a whole payload is now applied over a
  version another *widget's* gesture moved, which is safe only because every
  edit-back resolves to its own widget's placement, roll or curve — a foreign
  route, the case that can touch anything, still raises the floor.

- ✅ **Undo is not one behaviour: it answers some gestures and not others**
  *(found 2026-08-22 by the user, in `gui_composer`: "shrinking a piano-roll
  clip by the clip's grip does not respond to undo", and "the clips' contents
  (envelope) and the change of duration by the clip's grip do not come back";
  then, on the shape of the fix: "undo/redo happens in the host that shows the
  arrangement and has to be reflected in the document and in the client's
  instantiated data structures ... editing samples also responds to undo/redo.
  This is something low-level that can affect different kinds of components and
  data structures in different contexts")*. Every route already recorded an
  intent through the crate's log, and the crate was right in every case measured
  — the document stepped back correctly each time. What was wrong is the
  **projection**: writing an intent back onto the instantiated data, three
  implementations read the same thing wrong in the same way.

  **The rule, now stated as one** (`docs/decisions.md`, "An intent states the
  whole value, so absence is a value"): an intent states the value the edit
  results in, *whole*, so a field it does not carry is that field's **absence**,
  and absence is written. A `place` with no `dur` is a placement with **no
  length** — the element's own is what plays — not "leave the length alone".

  What that fixed, each of which was a live defect:

  - **The first resize of a clip could not be undone.** Its inverse carries no
    `dur` at all, because before it the placement stated none; every projection
    passed that to a convenience method whose `None` means *unchanged*
    (`Aggregate.move(member, offset, dur=None)`, the same in TypeScript, and
    `if let Some(dur) = dur` in the host's own `adopt`). So the log stepped
    back, `undo()` answered `True`, and the clip kept the size the hand had
    given it. A *second* resize undid correctly, which is why it read as
    intermittent. This is the case the user reported.
  - **An undone trim left the window over the samples where the trim put it.**
    A trim states the placement *and* the window in one `setmembers`, so its
    inverse states the member as it was — and a take nobody has configured has
    no `config` at all, which the projection skipped as "nothing to write". The
    clip went back to its old size still reading the wrong frames. The existing
    trim test passed only because its take carried an `instrument`.
  - **An intent over an aggregate moved one widget.** A trim, a split, a join
    and a cut are one `setmembers` over a *lane*, and the projection answered
    with the lane's own widget, so every clip the edit moved stayed drawn where
    the hand had left it. It now answers with every member it states, and a
    resync carries a take's window (`start`/`loop`) as well as its placement,
    since that is as much of "what this widget should be drawing".
  - **The drawn length had two rules.** The draw resolved it as *placement →
    element's own → the element's extent → the samples*; the path that puts a
    placement back knew only the first two. Both now ask one function
    (`_drawn_beats`/`_drawn_dur`, `drawnBeats`/`drawnDur`, `tree::clip_dur`),
    which is what makes the picture unable to disagree with the model.

  **The samples were already right, and are the same rule seen from lower
  down**: a destructive edit's inverse carries the values it painted over and
  the host replays them onto the buffer it drew from
  (`a_stroke_reaches_the_buffer_and_the_picture_and_undo_puts_both_back`). That
  is why an undone stroke always came back while an undone resize did not — not
  because samples are special, but because a payload of values has no way to
  express *unchanged* and a placement did.

  Three implementations, three tests, each of which fails without its fix:
  `an_undone_first_resize_gives_the_element_its_own_length_back` and
  `an_undone_trim_puts_the_window_back_on_a_take_that_configures_nothing` in
  both clients, and `an_undone_first_resize_puts_the_clips_width_back` in the
  host (whose `Owner` now holds the session's takes, since the length rule ends
  at the samples). The **envelope half of the report was already closed** by the
  entry above it — the undo that moved the model and told the host nothing —
  and re-reading it here confirmed as much: a layered clip's curve moves,
  undoes and redoes model and picture together, in the shape `gui_composer`
  builds and across a mixed run of gestures undone to the beginning, with a
  test in both clients. What was still broken was the length beside it, which
  is why one report named the two together.

- ✅ **A structural edit is never redrawn, so an undo of one could not be
  either.** Found while reading every editable body against the rule above. A
  split, a join, a cut and a cord change *which members exist*, and neither
  client redefined the window: the host emits the verb and holds nothing
  (`gestures::keys::clip_verb` — "it answers with the tree that now stands,
  exactly as it answers a drag"), the editor applied it to the arrangement, and
  nothing called `update()`. No example calls it either. So the second half of a
  split existed in the document and in the objects the script holds while the
  picture had one clip, until something happened to redraw — and an undo of it
  had the same gap in reverse.

  **The channel decides, and it is both halves of one rule.** An acknowledgement
  can carry any *value* a widget already has — a placement, a length, a curve, a
  note list, a take's window — and cannot carry a widget. So a structural edit
  redefines, forward and backward alike (an undo of a split takes a clip away,
  which is as unsayable in props as adding one), and a placement edit does not: a
  redefine rebuilds every widget and drops what the host has in flight, which is
  exactly wrong for a drag. The editor that drew the window is what owes the
  redefine, since it is the only party that can build the tree.

  Two things this pulled in with it, both live and neither visible from the
  outside:

  - **A redefine left the editor unable to edit.** `update()` bumped the
    client's version and the document's stayed where it was; the crate refuses
    an edit whose `against` version is not the document's — *ahead* of it as
    loudly as behind, since the two would not be talking about the same piece —
    so every gesture after a redefine came back refused, with no reason to show
    and the clip simply not moving. It now re-derives the document at the version
    it is drawing, which is what `refresh` already did for a script's edit.
    `a_redefine_leaves_the_editor_able_to_edit` in both clients; it only bites
    once a document exists, which is why nothing had caught it.
  - **The two clients disagreed about the node index, and one of them was
    right by accident.** Python added to it and kept elements that had left the
    tree; the port cleared the map on every re-derivation. An element that is
    gone is still named by the inverses in the log, and putting it back means
    placing *that object* again — a rebuilt one is a different identity to every
    widget and every pending edit — so the port's undo of a cut, and its redo of
    a split, quietly did nothing: the node came back in the document with
    nothing left to place. Now both keep it, deliberately and with the reason
    written down. This is the non-divergence rule doing exactly what it is for:
    one of the two was a rule and the other a habit, and only reading them
    against each other says which.

  Three tests in each client, each failing without its fix:
  `a_structural_edit_redefines_the_window_and_so_does_its_undo` (split, undo,
  redo — the model *and* the tree the host is handed),
  `a_redefine_leaves_the_editor_able_to_edit`, and
  `a_placement_edit_does_not_redefine_the_window`, which is the half that keeps
  a drag cheap.

- ✅ **The browser's lost release is guarded and the guard is unverified.** A
  page can lose a button-up — it comes up outside the window, over another
  application, after an alt-tab — where a desktop window cannot, and winit
  synthesizes a button event only from a move that *reports* a change. So
  `web::input` ends the drag itself when the DOM says no button is down
  (`CanvasSlot::buttons`, a capture-phase listener). It is small and its
  reasoning is sound, and **nothing has exercised it**: the case did not
  reproduce under Chrome DevTools, and the two hypotheses tried alongside it
  were measured and dropped. Worth either a way to drive it or a decision that
  it stays on trust, rather than sitting as code nobody has seen work.

  **Driven, and seen to work, 2026-08-26.** Not under DevTools, where it never
  reproduced, but as a case in `tests/gui.html` — the acceptance page already
  synthesizes gestures, and a synthesized `PointerEvent` carries its own
  `buttons`, which the capture-phase listener on `window` reads exactly as a
  real one's. So the case is staged rather than provoked: press on the bound
  knob, one real drag step, then a `pointermove` at a **far** position with
  `buttons: 0` — the pointer coming back in with the button already up — and
  then a second such move, since the drag has to be *over* rather than skipped
  once.

  **What proves it is the run with the guard removed**, which is the half the
  entry was actually asking for: the knob teleports from 1558 Hz to 50 — the
  bottom of its range, which is where the pointer re-entered — and the page
  fails with `a lost release dragged the knob`. With the guard the value stands
  where the hand left it. That is the described failure, observed, rather than
  reasoning about it.

- ✅ **The other drags that accumulate per frame** *(named 2026-08-22 by the
  user while the curve's bend was fixed: "this already happened with other
  graphic elements")*. The entry asked for a reading rather than a change —
  "read each one and say which it is, rather than assuming the shape from the
  file it sits in" — and the reading is the answer. **No drag was accumulating
  wrongly.** Every continuous drag in the host is one of the two correct forms:

  - **Absolute, the groove snapshotted at the press** — `control.rs`'s `Track`,
    which is the slider. A position inside the rectangle *is* the value, and the
    rectangle is taken at the press because the groove may move under the hand.
  - **Press-anchored, measured from a snapshot** — the roll's velocity drags and
    its note block (`notes.rs`): a single bar follows the cursor's height
    (absolute, like a slider), and a block nudges `orig` by
    `velocity_at(cursor) - press_velocity`, which is the bend's fixed form
    already. The note block is the same shape (`press_time`, `press_pitch`,
    `orig`).
  - **Incremental, and correctly so** — `control.rs`'s `Dial`, the knob and the
    number. It re-anchors every step *on purpose*: a knob has no groove on
    screen to stay level with, so there is nothing to be out of phase with, and
    the clamp eating motion past an end is the wanted behaviour rather than the
    bend's defect — pinned at the top, reversing moves at once instead of
    unwinding a snapshot. Both were already written down in the module and are
    held by tests (`positions_and_deltas_turn_it_the_same_way`,
    `a_pinned_knob_reverses_immediately`). This is exactly the "they may well be
    right" the entry allowed for, and it is why it asked to look rather than to
    fix.

  So the answer to the user's note is that the bend was the only one of its
  kind: it was relative *and* pointed at a shape on screen, which is the
  combination that drifts. What the reading did turn up is below, and it is a
  different defect.

- ✅ **A knob is a locked pointer on the desktop and a travelling one in a
  page.** Found by the reading above, and it is the standing rule's own case
  rather than a drag's. `Dial::press` asked for the pointer grab and the machine
  honoured what the front answered: the desktop front locked the cursor
  (`CursorGrabMode::Locked`, falling back to `Confined`) and drove the element
  with raw `DeviceEvent::MouseMotion` deltas, while the browser front passed
  `&mut || false` and had no device-event path at all. The element differenced
  the positions itself and landed on the same value — but the *gesture* was not
  the same one: in a page the cursor walked away from the knob and the drag
  ended at the edge of the window, where the desktop hid the pointer and turned
  as far as the hand went.

  **The fix is the opposite of the one this entry sketched, and the sketch was
  tried first.** Locking the pointer in a page cannot be made to work, and the
  reasons are worth keeping because they are winit's rather than ours: web's
  `set_cursor_grab(Locked)` calls `requestPointerLock` and returns `Ok(())`
  whatever the browser then decides, so `grab()` answers "locked" for a lock
  that may never happen; the delta it is supposed to be paid in is emitted only
  from a `pointermove` **carrying no button**, which is Chrome's `-1` and not
  Firefox's `0`, so the same drag would be dead in one of the two browsers; and
  the deltas come from `getCoalescedEvents()`, whose list is empty for a
  synthesized event, so no page test could ever drive one. With `grab` true the
  machine stops reading positions — and the knob moved on the press and never
  again.

  **A knob does not need the cursor captured.** What it needs is the thing the
  bend already had two entries above: a value measured from **one fixed anchor**
  — the press — rather than accumulated per step. `Dial` now records where the
  press landed, the fraction the value stood at there and the body's height, and
  every step is `t_press + drag_fraction_delta(cy - y_press, body_h)`. A given
  cursor position has one answer, so the pointer may leave the disc, cross the
  window and come back with the value exactly where it says; the motion spent
  past an end is kept rather than eaten, which is what makes coming back
  faithful. One algorithm, one gesture, both fronts, and nothing for a platform
  to answer differently.

  So the capture machinery had no user left and is **gone**: `Take::grab`,
  `Claim::grabbing()`, `Element::drag_relative`, `Drag::Element`'s `grab` field,
  `Gestures::locked`/`relative_motion`, `GestureEffect::ReleasePointer`, the
  grab callback the `press` signature took on both fronts, and the native
  front's `grab_pointer`/`release_pointer`/`device_event` path. `number` follows
  `knob`, as it always has.

- ✅ **A page burns the main thread while nothing is happening.** Measured on
  `clients/web/examples/editors/composer.html` through the DevTools protocol: **67% of
  the main thread, idle** — no transport running, no pointer moving, no streams
  subscribed. The guesses in this entry were all wrong: the tick's own stages
  are cheap, the `/clock_query` poll is gated correctly, and nothing in the
  browser front asks for a frame it should not. What asked was a **widget**.

  **A `notes` roll declared `Needs::clock` for its kind rather than for its
  anchor** — "a roll follows the transport, so the window has to keep
  repainting", which is true *while one is running* and was written as always.
  `clock` is the element's way of saying its own playhead is sweeping, so it
  makes `has_playhead` true, which makes the tree `animated`, which runs the ~30
  fps tick and repaints the whole window — for as long as the page is open, with
  nothing moving in it. The composer holds three roll clips. `score`, the other
  element with an anchor of its own, had the rule right all along
  (`clock: self.data.playhead_at >= 0.0`), and the roll now reads the same way.

  **Measured before and after**, same page, same browser, three seconds idle:
  **58.8% of the main thread and 30.7 paint requests per second → 3.2% and
  zero.** A sweep over the other examples says the defect was this one widget's:
  `editor`, `typed-controls`, `verbs`, `score`, `automation-lane`, `recording`,
  `multichannel` and `spectral` were already still (0 paints/s, ~1–3%), and the
  two that do animate — `scope` and `piano` — hold a scope and a meter, which is
  what animating is *for*.

  **The stages, since the entry asked for them.** Per frame on the renderer's
  main thread: the host's own work (the tick's tree walk plus the frame build
  and submit) is ~1.4 ms, and the rest — around 18 ms in a headless software-GPU
  browser — is the browser committing and presenting the canvas. So the cost is
  never in making a frame cheaper; it is in **not asking for one**. Both fronts
  share the rule, so the native window was repainting at 30 fps beside a still
  roll too, where a real GPU made it invisible.

  Guarded in both places it can regress: `notes` (the element declares it from
  its anchor) and `live::demand` (a tree with a still roll is not animated).

  *What is left of this entry, and it is a different question:* whether a page
  that **is** animating costs too much per frame — the jitter the playhead was
  reported with was measured with this burn underneath it, so it needs looking
  at again by eye, on a real GPU, now that a still page is still.

- ✅ **A drag beside the graph swept a marquee over nothing** *(found 2026-08-22
  by the user, on the patcher's Def view: "beside (outside) the frame that
  holds the boxes, dragging selects")*. The patcher draws a **labelled panel
  that hugs its laid-out boxes** — deliberately, so every box is inside it
  whatever rect the widget got — while its press claimed the whole widget rect,
  which a `scroll` workspace never sizes below the window. So the paper at the
  sides *looked* like the workspace and *acted* like the canvas: a drag there
  swept a selection instead of panning. It is the defect a knob's cell corners
  and a checkbox's air had, and it has the same fix — the element answers with
  a `hit_area`, and what can be grabbed is exactly what is drawn. A press
  outside the panel now falls through to the workspace, which pans it. Found in
  a page and fixed in the host, so both fronts get it: the rule was never the
  browser's.

  **And the edge is taken exactly**, which the first fix did not do: the machine
  adds a few pixels of hit slop to every shape an element declares, so the
  selection still started just outside the drawn frame. Slop grows a *target* —
  a dial, a note, a break-point, where nobody should hunt for a pixel — and this
  is a *region*, whose edge is where one owner stops and another begins.
  `HitArea::Region` is that distinction, named so the next boundary can ask for
  it: the same rectangle, taken at its edge.

- ✅ **A page lost the modifier keys until its canvas had been clicked**
  *(found the same day, reading the one above: Shift over a patcher is what
  makes it decline and the workspace pan, and Shift did nothing)*. The browser
  front tracked shift/ctrl/alt from winit's `ModifiersChanged`, which its web
  backend emits **only while the canvas has DOM focus**
  (`window_target.rs`: `has_focus.get() && …`). A reader who had not clicked the
  canvas yet held Shift and panned nothing; one who released it after clicking
  away left the front believing it was still down, so a plain drag panned. Both
  halves of "sometimes with, sometimes without". Every pointer and wheel event
  carries `shiftKey`/`ctrlKey`/`altKey` whatever has focus, so the front reads
  them there — the same listener, and the same reason, as the button mask it
  already reads that way. `ModifiersChanged` stays as the keyboard's path.

These are not milestones and they are not future directions. They are what
**using the thing** turns up — an eye pass over an example, a path read while
doing something else, a behaviour that is correct and unclear — recorded the day
they are found so they are not rediscovered, and kept here rather than inside
whichever milestone happened to be open at the time.

Two conventions, because the section only works if both hold. Every entry is a
**checkbox**, so what is open reads as open at a glance and nothing has to be
inferred from where it sits. And a fixed one **stays**, with the record of what
was wrong and why the fix is the shape it is — that is what makes the list worth
reading rather than a queue that empties.

Anything unresolved lives here or under "Future directions", both **after** the
tracks: never inside the milestone that happened to be open, and never among
finished work, where a pending item reads as done.

- ✅ **The wheel zooms faster in a page than in a window, and the conversion is written three times** *(found 2026-08-20 by the user, comparing the two twins side by side: "the wheel zooms faster than in native")*. The host's own arithmetic is not what differs — `native.rs`, `host/gui/app.rs` and `host/web/input.rs` each carry the identical two lines, `LineDelta(_, y) => y` and `PixelDelta(p) => p.y / 50.0`. What differs is what winit hands each shell: a wheel click on X11/Wayland arrives as `LineDelta(0, ±1)`, one step, while a browser sends a `WheelEvent` at `DOM_DELTA_PIXEL` with a `deltaY` of about a hundred per click, which the same divisor turns into **two**. So the input is not normalized per shell, and the `/50` is calibrated for the native trackpad.

  **Fixed 2026-08-21, and the measurement turned up a second axis the entry did
  not have.** Reading winit's web backend rather than a browser: at
  `DOM_DELTA_PIXEL` it takes the `WheelEvent`'s `deltaY` in **CSS** pixels and
  converts it **to physical** by the window's scale factor before handing over a
  `PixelDelta` (`platform_impl/web/web_sys/event.rs`, `mouse_scroll_delta`). So
  the divisor was not only calibrated for the wrong shell, it made the zoom rate
  a property of the **display**: the same notch on a 2x screen reported twice
  the pixels and zoomed twice as far, in a page and on a native trackpad alike.
  That is the same thing `CLIP_EDGE_PX` was doing before the grip shipped — an
  interaction site naming device pixels — and it is why the fix is arithmetic
  rather than a number.

  `gestures::Wheel` is that arithmetic, in the module where the wheel's meaning
  already lives: a `WheelDelta` (lines or physical pixels — the two shapes any
  source has), a per-shell calibration (`Wheel::NATIVE`, `Wheel::BROWSER`), and
  `steps()`, which takes the scale off a pixel report before dividing it. The
  core stays platform-agnostic by rule, so each shell still translates its own
  winit event into `WheelDelta` — that is the shell's job — but the conversion
  to steps, and the numbers, are written once. `Wheel::BROWSER` is 100 logical
  pixels per notch against the native trackpad's 50, which is exactly the factor
  of two that was felt. The test asserts the claim rather than either number: a
  notch is one step, on either shell and on any display.

  **Measured 2026-08-21 in a browser, and the number was not the suspected
  one.** Firefox 153 on X11 reports `DOM_DELTA_LINE` with `deltaY = 6` per
  notch, identical across every event — so at the placeholder `lines_per_step`
  of 1.0 a wheel click was **six** zoom steps, worse than the two that were
  reported and in a mode the entry had not considered. The instrument is
  `clients/web/tests/wheel.html`: `deltaMode`, `deltaY` and `devicePixelRatio`,
  needing none of the web package built. It is the one page there that is
  **not** in `test.sh`'s `run_page` list and says so — it needs a hand turning a
  wheel and has no verdict to beacon — and it is in the tree rather than thrown
  away because the number it reads is per browser and per machine, so this is
  asked again and not once.

  **What the measurement showed is that the number should not be used.** A
  pixel report is a browser constant; a *line* report is the viewer's own
  scroll preference — how far a document moves per notch — and 6 is a setting,
  so calibrating to it would repeat, smaller, exactly the mistake the `/50` was.
  Nothing about that preference is about zoom: no browser or OS carries one for
  how far a notch should zoom, the browsers' own `ctrl`+wheel moves one step per
  notch whatever the scroll setting says, and **this host had already decided
  not to honour it anywhere** — a wheel pan is `steps * WHEEL_PAN_PX`, the
  host's own quantum, and a lane resize `1.1^steps`. The preference was never
  being respected; it was leaking into a place it means nothing.

  So a line report is **counted, not divided**: `Lines::Notch` takes the
  direction and reads the event as the one notch it is, which agrees with
  `Lines::Per(1.0)` natively without either being tuned to the other, and holds
  for a machine set to 3 lines or to 16. The pixel half keeps its divisor,
  because a trackpad's stream is continuous and there the magnitude *is* the
  information — the two shapes `WheelDelta` already had turn out to be the two
  rules. The test asserts a notch is a step at four different scroll
  preferences.

  **The pixel half was measured the same day, in Chrome 151 on X11: 120 CSS
  pixels a notch**, identical across every event — so `pixels_per_step` is 120
  and not the 100 the original report's factor of two had suggested. It goes in
  **as read**, and the first version of the measuring page got that wrong: it
  divided the figure by `devicePixelRatio` and reported 80. `deltaY` at
  `DOM_DELTA_PIXEL` is already in CSS pixels; winit multiplies it by the
  window's scale factor and `Wheel::steps` divides that back out, so the ratio
  **cancels** and dividing again on the way in is a double correction. Read
  properly it also reverses which way the old constant was wrong: at 100 a
  Chrome notch was 1.2 steps — twenty per cent fast — not the 0.8 the mistaken
  row showed. The test now walks four device-pixel ratios so the cancellation is
  asserted rather than reasoned about.

  **Verified end to end 2026-08-21 in the page** (`clients/web/tests/editor.html`,
  the host drawing a `waveform` and a `spectrogram` over the in-page engine):
  the two browsers move the same as each other and as the native window, with
  Chrome felt as *imperceptibly* faster. That residual is expected and is a
  consequence of the two rules rather than a mistuned constant: a line report is
  **counted**, so one event is one step however hard the wheel is spun, while a
  pixel report is **divided**, so a browser applying scroll acceleration to a
  fast flick sends more than 120 and gets more than one step. Per notch at an
  ordinary speed they agree exactly, which is what was measured and what is
  felt. It cannot be closed by counting the pixel half too: a trackpad's stream
  is continuous and there the magnitude is the whole of the information. Worth
  reopening only if a fast spin turns out to overshoot by enough to notice.

  **The number has to be measured before it is chosen**: `deltaY` per click depends on the browser, the OS and the scale factor, so the fix starts by logging `steps` in both shells and turning one click on each. Then it is **one function** — "a wheel event becomes this many zoom steps", with the per-shell adjustment inside it — that all three call sites use, rather than three copies that happen to agree today. Same family as the drawing divergence and on the input side of it: the host is one, so what reaches it has to be one too.

- ✅ **A page's opening pass re-subscribes once per canvas** *(found 2026-08-20, watching the refusals while fixing the entry below — with the page engine's ceiling lowered to 50, a forty-canvas page produced fifteen refused subscriptions before the first reply landed)*. `on_tree_changed` re-derives and re-sends the `/bus_stream` set on every tree change, and a document's canvases open in one synchronous pass — so a page of forty sends forty subscriptions, each replacing the last, and the engine cannot answer any of them until its next serving turn. It is correct and it is bounded, which is why it survived: the end state is always the right set. What it costs is a round trip and a server-side re-subscribe per canvas at load, and it is what turns one wrong ceiling into a burst of refusals instead of one. The fix is to coalesce: mark the subscription dirty and re-send it on the tick that follows, which is where the host already does its per-frame work — and the same would hold for `/bus_tapStream` beside it. Left open because nothing is wrong with what the page ends up drawing.

  **Fixed the same day, and it is the coalescing the entry named.** `on_tree_changed` now settles what is local (the animation tick) and *queues* what is a message: `schedule_stream_sync` posts a zero-delay timeout unless one is already queued, and the handler re-derives the demand and sends `/bus_stream` and `/bus_tapStream` once. A zero-delay timeout is exactly the window a synchronous document pass needs — everything built in that pass lands in one request, and a change that arrives in its own turn still goes out immediately. The two other places that re-derived a subscription by hand (the rate arriving with `/clock_query.reply`, the ceiling with `/server_query.reply`) go through the same queue, so there is one door that talks to the server about subscriptions rather than three.

  Measured on the same profile, which counts the requests now: **41 `/bus_stream` requests to open forty canvases, and 1 after**. With the page engine's ceiling lowered to 50 against 80 wanted, the refusals during the opening pass go from fifteen to one — the single refusal that teaches the host the ceiling, which is what a refusal is for.

- ✅ **Past 128 buses a page's stream stops growing, and it stops silently** *(found 2026-08-20, profiling what a frame of `/bus_stream` costs on a page of forty canvases — the question the in-page shared-memory path waits on, in "Future directions" above)*. `/bus_stream` is one subscription per client, and the server caps one at 128 bus indices (`MAX_STREAM_BUSES`, `src/osc/server/mod.rs`): a larger request is refused with `/fail` **and the previous subscription is kept**. A browser host subscribes the union of every visible canvas's live widgets, so a page crosses that ceiling by opening canvases — about sixty-four of them at two buses each — and from there every new canvas reads a bus nobody streams while the ones already in the set keep moving. The refusal reaches the console log and nowhere else; the widgets draw their last value, and the host's `streamed` goes on recording a subscription that does not exist. Measured on the profile at seventy canvases: eight refusals, and once the subscription is cancelled and re-derived the page gets **no stream at all** — the refusal leaves it with nothing rather than with the subset it had.

  What a fix has to settle spans both packages, and the smallest half is the honest one: a `/fail` on the host's *own* subscription cannot be ignored, since the host's belief about what it subscribed is what stops it asking again. Above that is the real question — whether the union is split across several subscriptions (one per client is the server's rule, so that means several peer tags or a protocol change), whether the ceiling rises, or whether the host drops canvases from the set by some order and says which. Nothing on the path to the complete example reaches sixty-four live canvases, which is why this is here and not scheduled.

  **Fixed 2026-08-20, and the constant was the smallest part of it.** Why it surfaced here at all: the command exists for clients that *cannot map the segment*, so only a page ever meets the ceiling — a native host reads the buses out of shared memory and subscribes nothing — and the 128 was chosen the day the command was written (2026-07-05, "a browser GUI host's meters/scopes over WebSocket"), sized by the sentence "128 pairs fit comfortably in a single frame on every transport". It was a guess at a *frame*, made before the in-page engine, before the ring's peer tags, and before a page could hold a document's worth of canvases; the sibling written later, `/bus_tapStream`, already clamps its window against the asking client's real transport bound.

  So the ceiling is **boot-time configuration now, on both deployments**: `--max-stream-buses` / `[server] max_stream_buses` (default 4096, generous like the rest of the server's pools) and, for the page, `maxStreamBuses` at engine boot → `WebServer::set_max_stream_buses` → `ClaustersHeadless::set_max_stream_buses`. A limit only a server operator can move is one the browser is stuck with, which is where this started. What a **given** client may ask for is that clamped by what one reply carries over its own carrier — a snapshot is one message and is never split — so it is per client, and it is the last field of `/server_query.reply`, read by both clients' `ServerInfo` and by the host itself.

  The host's half is the part the entry called the honest one. It asks `/server_query` when its leg attaches, clamps the union to the answer (`live::clamp_stream_set`, keeping the head — def-id order, so the canvases that opened first hold their stream) and says in the log how many buses were left out and which knob raises them, once per *change* rather than once per canvas. A `/fail` is no longer only logged: it drops the belief in the subscription — which is what had stopped the host ever asking again — takes the ceiling the refusal names (`live::refused_stream_cap`) and re-subscribes what fits. Both rules live on the agnostic side of the seam and are unit-tested there with no browser. The leg starts out believing the historical 128 rather than the configured default, because a document's canvases open in one synchronous pass and the engine cannot answer `/server_query` until its next serving turn.

  **Measured with the profile that found it** (`clients/web/tools/profile-bus-stream.sh`, whose third argument lowers the page engine's ceiling on purpose): seventy canvases — the case that used to end with no stream at all — now stream all 140 buses, and at a ceiling of 50 against 80 wanted the page streams 50, draws, and says what it left out instead of freezing.

  What it did **not** do is split the union across subscriptions: the protocol's rule is one per client, so splitting means several peer tags or a second connection — carrier-dependent plumbing for headroom a configurable ceiling gives outright (`docs/decisions.md`).

- ✅ **An overview reply was bounded in buckets, and a bucket is not a size** *(found 2026-08-20 by the user, on the first eye pass over the page that draws a minute of stereo from its summary: the lane never resolved, and two others froze at their summary beside it)*. `/buffer_peaks` and `/buffer_stream` both capped a reply at 4096 **buckets**, which is 96 kB of a stereo take and 400 kB of an eight-channel one — while the smallest carrier a reply crosses is the shared ring at **64 KiB**, whose `push` refuses what does not fit and says nothing. So the first answer for that take was dropped in silence, and the picture stayed the empty pyramid it had been given.

  The cap is bytes now (`MAX_STREAM_BYTES`, a sixteenth of the ring, turned into a bucket count by the channel count), which fixes the streamed path for a multichannel recording as well — it had the same defect and had simply never been asked for enough at once to hit it.

- ✅ **A reply lost on the way back held its buffer for the rest of the session** *(found 2026-08-20 alongside the entry above: with the ring full, three of four lanes zoomed past their summary and stayed there, and nothing in the log said why)*. `BufferFetches` allows one download per buffer, which is the bound that keeps a session's traffic sane — but a conversation that never completes is never removed either, so every later span for that buffer was refused by a download that would never finish. No error, no retry, a view frozen at its summary with no way back.

  **A download that is working lands something on every reply**, and that is the difference the fix keys on: a span refused for the same buffer without anything having landed in between counts, and after two seconds' worth of frames the stale fetch is dropped so the next ask starts a new one. A slow conversation is never restarted, because progress resets the count.

- ✅ **A page cannot zoom to the sample while a take records, and behind the frontier it could** *(found 2026-08-20 by the user, comparing the two twins: "it works the same, with the difference that in native you can zoom to the sample while recording")*. Natively the fine regime asks for nothing — the samples are the mapped cells, so any zoom is current with nothing told to it, and `fills` only decides how far the lane draws. In a page `fills` does a second thing: while it is set the view asks for **no span at all** (`frame::missing_span` returns `None` the moment there is a frontier), so a recording can only be drawn at the bucket the stream reports.

  **The reason written down is stronger than it needs to be.** `docs/gui-protocol.md` says a take still being written asks for nothing because "there is nothing past the frontier to read, and behind it the next block would make a run stale" — the first half holds always, the second only for the run *at* the frontier. A span entirely behind it is final: a recorder writes forward and does not come back. So the condition wants to be the span's position rather than the take's state — everything below `written`, less a margin for the block being written — and the machinery it would use is the one that is already there (`want_span`, `set_window`, and now a bounded number of them in flight).

  Two things to settle when it is taken: how much margin behind the frontier is honest (a block? the report period?), and whether a view that is following a recording should be asking for spans at all while the stream is also arriving — the traffic is bounded now, but it is worth measuring rather than assuming.

  **Done 2026-08-20, and both questions came back smaller than they were asked.** The condition is the span's position, as the entry said: `missing_span` no longer refuses on `written` at all, it **clamps** to it — everything below the frontier is asked for exactly as a finished take's span is, and nothing crosses it. The **margin is none**, and that is an answer rather than a shortcut: the frames a report counts were written before it was measured, and a recorder writes forward and does not come back, so the last frame behind the frontier is as settled as the first. What needs guarding is the other side — past the frontier the buffer still holds the zeros it was allocated with, and a run over them would claim measured silence over audio that has not arrived — and that is the clamp itself, not a margin. On the second question the traffic answered on its own: a view asks only where it is zoomed past its own bucket, which is rare, and what it asks for is bounded by `MAX_IN_FLIGHT` and re-asked when lost, so a view following a recording competes with the stream on the same terms every other reader does. Nothing was needed for it.

  It is one condition in the one place that knows the zoom, the span and the frontier at once, so both legs took it together. `recording-host.html` asserts the read **while the take records** as well as after it is finished (two different spans, because a run already held is a view that can answer for itself), and `missing_span`'s own test pins the three cases: wholly behind the frontier, straddling it, wholly past it. `docs/gui-protocol.md` (under `fills`) and `docs/architecture.md` (the bulk path) now say the frontier is a **ceiling on the span** rather than a refusal of it, and `examples/views/recording.html` says it to the hand that will try it.

- ✅ **The carrier is allowed to lose a reply, and nothing that reads over it knew** *(found 2026-08-20 by the user across three eye passes, each showing a different face of it: a lane that never resolved, three that froze at their summary, a picture with silence written into it, and finally "only the first lane zooms, the rest take many seconds")*. The root of all of them is one line in the server: a reply to a ring client whose ring is full is **dropped**, deliberately — "backpressure, not loss ... dropping the reply is all we can do without blocking the server". The ring is 64 KiB. Every reader over it, including this host's fetch machine, was written as if a request always gets an answer.

  **Three things follow, and all three are now the host's own business.** What it asks for is sized by the *carrier* rather than by throughput: a `/buffer_getRange` chunk is 16 kB, so four fit the ring, where the old 32 kB let two lanes fill it between them and every other lane's answer went missing. How much it asks for **at once** is bounded (`MAX_IN_FLIGHT`), because its own traffic is what fills the ring and a session that zooms four lanes should queue rather than lose three of them. And what is lost is asked for again: a download that lands nothing for half a second is started over, and a summary walk that hears nothing repeats its request.

  **The summary walk moved into the fetch machine** for exactly that reason — it was the one conversation with no recovery, so a lost piece left a hole in the picture for the rest of the session and nothing anywhere would notice.

  What is *not* fixed here, and is the interesting part: a page zoomed past its summary still has to move a few hundred kilobytes of samples through a 64 KiB carrier, a chunk per frame. It converges in a fraction of a second and it is still the wrong shape of answer — see Future directions, "A zoom asks for samples where what it wants is a finer summary".

- ✅ **A straggler ended the fetch that had replaced it, and the window it left claimed samples nobody read** *(found 2026-08-20 by the user on the pass after the one above — the lines had a dashed hairline over most of their length and the waveform only in a stretch of it, in both browsers)*. Two halves of one failure, and the first was **introduced the same day** by the fix that stops a lost reply from wedging a buffer: a restarted fetch has a new span, and the abandoned one's reply can still arrive. `on_data` read "no range landed anything" as *the server declined the read* and ended the download with what it had — so a late range from the old conversation, which is not this fetch's to be ended by, finished the new one on the spot. Whatever it had not received was still the zeros the buffer was sized with, and a window says it **covers** its whole length, so the picture drew measured silence over a stretch of audio it had simply not read.

  A range outside the fetch's own span is ignored now, and ignoring it is not the same as ending on it. And a span is handed over **as far as it actually arrived**: truncated, the run answers where it reaches and the summary answers everywhere else, which is what a partial read honestly is — and the frame asks again for what is still not covered, so it fills in rather than staying wrong.

  The other thing the same picture showed is **not** a defect and is worth the note: a sine drawn at about two pixels per cycle alternates between the halves of its own waveform, because that is what a column's min and max are there. The comb is the signal.

- ✅ **A patched buffer released every slot in the window, not the ones drawing it** *(found 2026-08-20 by the user on the same pass: one peer's edit landed on the take it named and blanked every other lane on the canvas)*. A GPU slot holds the pyramid it draws, so an element about to rewrite that pyramid has to release the slot first — but the release walked *all* the waveform slots rather than the ones whose element names that buffer, and a slot released and never refilled draws nothing. The streamed-report path next to it had always narrowed the walk to the widgets holding the buffer; the patch path was written from it and lost that line.

- ✅ **The embedded-segment test slept for the engine instead of waiting for it** *(found 2026-08-18 running the standalone suite repeatedly while building something else: `the_embedded_segment_reads_the_pieces_position` failed about one run in five, always the same assertion, always reading a clock of 0)*. It sent a locate and slept 120 ms. The engine publishes the position once a block — a fraction of a millisecond of work — but the first block arrives when the **device** starts, and on a loaded machine that is not within any fixed number of milliseconds.

  Fixed by waiting rather than sleeping: a short poll against a two-second deadline, which costs one poll interval on an idle machine and does not go red on a busy one. Worth writing down because a flaky test is the worst kind of red — it says nothing about the code, and it teaches the reader to re-run rather than to look.

- ✅ **A session killed rather than closed leaves its segment and its takes in `/dev/shm`** *(found 2026-08-17 closing H6, by running the editor under a timeout)*. A session that created its segment removes it on the way out, regions and all — but `Drop` does not run on a signal, so a killed editor (a `kill`, a crash, a `timeout` in a script) leaves the segment file and one region per take behind, and a region is the take's whole size: 230 MB for ten minutes of stereo. Nothing ever collects them, so a machine that has crashed an editor a few times has a full `/dev/shm` and no way to tell which files are live.

  What it is not: a leak in the ordinary sense — the material is *supposed* to outlive the session's threads (a player still mapping it reads valid memory, which is the property freeing one buffer relies on). What is missing is a way to know a segment is **dead**: the control-plane claim already records a pid and already knows how to tell a stale one, so the same test could sweep a directory of segments at startup. Deliberately not written blind: which process does the sweeping, and whether an editor may remove a segment it did not create, is a decision rather than a patch.

  **Decided and fixed 2026-08-18, in the server rather than here.** The sweeper is **whoever creates a segment**, at the moment it creates one: that is the only point where a process is provably starting fresh, and it needs no daemon, no timer and nobody to remember. It **may** remove a segment it did not create, and three things make that the right shape rather than a dangerous one — unlinking a name leaves every mapping somebody holds valid until it drops it (the property freeing one buffer already relies on), a claim of *nobody* is never swept (`0` is a segment created a moment ago as much as one released on a clean exit), and the path being opened is never swept, so recovering a killed owner's material by opening the same path works exactly as before. `server::ipc::sweep_dead_segments` is the sweep and `remove_segment` the removal both it and a clean exit use; `Segment::open_or_create_full` calls it, which covers both doors (`clausters --shm` and a session, hence the standalone editor). The acceptance is `tests/ipc.rs::a_dead_owners_segment_is_swept_and_a_live_one_is_left_alone` plus the last section of `examples/editor_processes.sh`, which SIGKILLs the owner, shows the files it left, and starts the next one to watch them go. The rule is written down in `docs/ipc.md` and the reasoning in `docs/decisions.md`.

- ✅ **A recording's unwritten remainder draws as silence, because the host cannot tell it from silence** *(found 2026-08-18 closing H8, which needed the distinction and could not make it; the prop shipped 2026-08-19)*. A take being recorded is material up to its write frontier and *nothing* past it; a take read from a file is material everywhere and has no frontier at all. Those are two different pictures, and the host has one number that does not separate them — a `BufWr` writing one sample into the middle of a loaded take raises a frontier too, and a view that stopped drawing past it would erase the rest of a file. **The client is what knows**: it allocated the empty buffer to record into. So the answer is a prop of the signal element (`fills`, or whatever it ends up called) that says *this material is being written; draw only up to its frontier* — the wire, both builders, `docs/gui-props.md` and its parity test, which is a small milestone rather than a fix. Until then an unwritten remainder draws as the silence it is in memory, with the minimum-ink rule putting a flat line across it.

  **Shipped as `fills`, and it is a prop for the reason the entry gave.** The element takes `fills` (`/gui_def` and `/gui_set` alike, live both ways), the host pushes the frontier it already reads into it (`Element::set_written`, from the same walk that re-summarizes what appeared), and the one renderer of a signal against time cuts there: `TraceStyle::with_written`, which stops the column loop and clamps the polyline's last sample, so both regimes and the level body take it from one place. Nothing written yet is nothing drawn — not a flat line across the buffer, which is what the minimum-ink rule was doing to a take before anything reached it. Cleared, the whole of the material is drawn again, which is the right answer for a finished take: what was written is now all there is.
  **Two things fell out of the cross-package pass, and both were real.** A `bool` handed to `/gui_set` raised `TypeError: OSC has no bool tag here` in the Python client and rode through untouched in the TypeScript one — `set(fills=False)` is what a reader of `fills=True` in a builder will write, so both ends coerce it to `1`/`0` now, which is what a flag prop has always been on the wire. And the example is what found it: nothing runs them, and `recording.py` is where this prop is checked by eye — the lanes open empty, each trace grows into an empty axis, and the last cell clears the prop when the takes are finished.

- ✅ **A refresh copied the take's summary because the slot that draws it was holding it** *(found 2026-08-19 by the user, watching `recording.py`: a take grew a second at a time and looked stuck)*. The second was exact — `--follow-block`, one second — and H8 chose that cadence for a reason it did not look past: re-reading a summary costs a copy of it. It did, because the pyramid a view draws is shared with its GPU slot, so a refresh cloned the whole thing before patching the span it had actually measured. Measured on that example: **13 µs of clone against 33 µs of summarizing**, per take per step, and the clone is proportional to the take — 780 µs a channel at ten minutes. That is a cost the block cannot shrink, which is exactly why the block had to grow.

  **Fixed by having the holder let go.** `WaveformView::release_data` gives the material back before the element writes and the next fill hands it over again — a repaint runs `refresh_slots_for` first, and the write leaves the element dirty, which is what a fill answers to. In between the element is the sole owner and `Arc::make_mut` gives it `&mut` with no copy at all. The sharing was defending against a **borrow** and never a race: the front is single-threaded. Measured again, 88 of 88 refreshes copied before and 0 of 88 after; the example then runs 1976 refreshes of 5 µs over spans of 768 frames, about a millisecond a second with eight takes recording and flat in their length. `--follow-block` stays, defaulting to `0`, for the case where even that is too much.

  **Two things fell out.** Every source of a wake-up now combines by the **nearest** deadline: two of the five assigned instead, so whichever ran last decided and the other's was dropped — a window that both follows a recording and animates kept only one. And `WaveformData::nothing` is what a released view holds, so giving the material back needs no `Option` at every site that reads it.

  **And the first answer was the wrong one, which is worth keeping.** The late last block was read as a **wake condition** — `window_follows_a_recording` asks whether something is still writing (a frontier short of the buffer's end), evaluated after the tick has already followed, so a writer finishing in between left it false with material outstanding. It was rewritten to ask whether there is anything not yet drawn, and then **reverted**: the dropped deadline above is what was actually losing the wake, and the narrow condition is the one that lets an ordinary window sleep. The symptom was one bug's and the diagnosis was another's.

- ✅ **A page cannot fold a streamed overview into the picture it holds** *(found 2026-08-18 closing H8; the wire it needs shipped with the server's S20)*. `/buffer_stream` carries the summary of what a recording added — min, max and mean square per bucket, the pyramid's own three statistics — and both clients can subscribe to it (`stream_buffers`, `streamBuffers`). What no page does with the reply is the last step: **writing those buckets into a pyramid it already holds**, which `clausters_core::peaks` has no door for (it can summarize a source and update a span *from* one, and a page has no source — it has the buckets themselves). That door is the work, and it is the core's: `Pyramid` taking level-0 buckets at an offset and recombining upward, which is `update_range_from`'s own second half with the measuring skipped. Then a page follows a recording exactly as the native host does, at two kilobytes a second.

  **The door shipped first, 2026-08-19.** `peaks::Bucket` is one level-0 bucket somebody else measured and `Pyramid::write_buckets` puts a run of them at an offset, recombining upward the way the builder combines — `update_range_from` with the measuring skipped, as the entry said. `MultiPyramid::write_buckets` is the one a client calls: it takes the reply's payload *as it arrives* (bucket-major, channel-minor, `start_frame` on the buffer's own axis), refuses a report on another grid — another bucket size, a start off a boundary, a run that does not fit — and refuses it **whole**, since a half-applied report would leave the channels describing different material. Both legs bind it (`clausters_core_peaks_multi_write_buckets`, `Peaks.writeBuckets`, `peaks_cache_stream_file`), and the parity vectors assert what makes it worth having: a cache filled *only* from reports is byte-identical to one built from the samples, in Python and in the browser alike.

  **And the native host is a consumer of the same wire, which was not the plan when this was written** *(named 2026-08-19 by the user, watching `recording.py` after the `fills` prop shipped)*. A recording's last block lands up to a **second** late, and the second is `--follow-block` (default `1.0`, `[gui] follow_block`): the host does not wait for a block of audio, it **polls** each buffer's frontier on that cadence, summarizes what appeared and repaints once for every view that grew. The cadence is deliberate — `app.rs` says why, and it is that following at frame rate would repaint thirty times a second with thirty-two takes recording — so the fix is not a shorter poll.

  **What the lateness turned out to be, 2026-08-19: not the wire.** The second was buying back a copy of the summary, and once the copy was gone the poll could run at the frame — `--follow-block` defaults to `0` now and a take grows with the sound. So the native leg is **no longer late**, and what is left of this paragraph is the weaker half of the same argument, which still holds: the poll re-derives, from mapped samples, the summary `/buffer_stream` already carries, and a host that subscribed would be **told** at the writer's own cadence rather than asking at its own. That is a saving and a simplification, not a fix for anything visible.
  Knowing *which sample* the writer stopped at adds nothing either: the frontier already carries exactly that. What is missing is not the datum but **when we look**, and the answer the user chose is that the server should say. `/buffer_stream` already carries the summary — min, max and mean square per bucket — which is the same summary the poll is re-deriving from mapped samples, so a host that subscribed would be told rather than asking, at the writer's own cadence and with no re-summarizing at all. That makes the core door below serve **both** legs rather than the page's alone, and it is why this entry is the one the phase waits on.

  **Closed 2026-08-19: the consumers shipped, and the first of them is the host itself.** What sits on the door here is the rule that decides *who* is told: a view whose material is its **own copy** and whose client set `fills` asks to be told (`Element::stream_want`), and the host keeps **one** subscription covering every such view of every window (`Host::sync_buffer_streams`) — one, because the server keeps one per client and a second call would cancel the first. Each report is folded by the element (`Element::write_buckets` → `WaveformData::write_buckets`), with the slot letting the material go first so the write costs the report and not the take, exactly as the mapped path does. Both legs route the reply, so this is the browser host *and* a native host on a server that shares no segment.

  **And it turned out the wire had never worked where it was needed.** `/buffer_stream` read the write frontier out of the **segment's** directory row, so a server with no segment — an engine in a page, a `clausters` without `--shm`, which is most of the clients that cannot map anything — reported nothing at all, silently, forever. The frontier is the material's fact, so it is the **buffer's** now and the row is its mirror (the row still counts, since a peer writing into the shared region raises it there and this server's buffer never hears about it). `tests/osc.rs::buffer_stream_reports_a_recording_with_no_segment_behind_it` is the guard.

  **What did not change is the mapped path, deliberately.** A host that maps the segment goes on reading the frontier and re-summarizing out of the material: it is told the same thing either way, and the poll costs one relaxed load per drawn buffer against a subscription's traffic. So the saving named in the paragraph above stays untaken, and what the wire bought is the case that had no answer at all rather than a cheaper one for the case that did.

  **The client half is `data.RecordingStream`** (`clients/web`), for a page drawing the take on its own canvas rather than through the host — and it cannot be used *beside* a host view following the same way, for the one-subscription-per-client reason above. `clients/web/tests/recording.html` is the acceptance: a take recorded by the in-page engine, followed as it fills, and then compared column for column against a pyramid built from the samples read back. `clients/web/examples/views/recording.html` is the by-eye twin of `recording.py`.

  **What a told picture is not**: the samples. The wire carries the summary, so *while it is filling* a view fed this way resolves to the report's bucket. That was going to be the one lasting difference from a host that maps the material, and the user refused it on the rule the whole seam is judged by — the behaviour is identical across platforms, only the route differs — which is what opened **H9**: a finished take reads back the span it is zoomed into, and the two draw the same picture at every zoom.

- ✅ **A page cannot follow another peer's edit, and now says so instead of showing it** *(found 2026-08-18 closing H7, which took the native half of the same wire)*. `/buffer_touched` carries a span and no samples — deliberately, since the peer that wrote it wrote into memory the receiver may already share. A host that **maps** the material therefore follows it for free by re-summarizing that span, and a **page** cannot: its samples are its own copy, so what it needs is the span *back*, and the fetch machine only knows how to download a whole buffer. The page logs that its copy is behind rather than redrawing it, which is the honest failure and not the fix. What it takes is a partial fetch — `/buffer_getRange` over the announced span, placed into the view's existing samples and pyramid — which is one more state in `host::fetch` and the same `resummarize` at the end. Worth taking with the next thing that touches that machine rather than alone.

  **The machine arrived 2026-08-19 and this entry did not close with it.** H9 built the span fetch for the other consumer — a zoom past the summary — so `BufferFetches::want_span` and `Samples::Window` are here, keyed by the view that asked. What is left for a peer's edit is the trigger and the summary: re-read the announced span into the window when it overlaps one, and re-summarize those buckets either way (the pyramid is what a zoomed-out view draws, and it is what goes stale). Whoever takes it has the machine and owes the two lines that use it.

  **Done 2026-08-20, and it was three things rather than the two the note promised.** The span fetch now says what it is *for*: `SpanUse::Window { def_id, widget_id }` is a zoom, which one view asked for and which is dropped when the buffer is busy (the next frame asks again), and `SpanUse::Patch` is an announced edit, which names no view at all — the samples that come back are the buffer's own, so every picture of it takes them (`host::patch_buffer_views`). An edit announced while a download is in flight is **queued and merged** into the one span covering it instead of being dropped, because an announcement happens once: a lost zoom costs a frame, a lost edit stays wrong until something else touches that buffer.

  **What lands depends on what the copy is, and the element decides** (`SignalElement::patch_span`). Samples that are here are written and re-summarized over the span — the same door a stroke goes through, which already behaves correctly whether the copy is owned or mapped. A summary with *no* samples under it — a take recorded into, whose picture is buckets and a window — takes the span as the buckets it covers, so the request is widened to whole buckets first (`fetch::align_span`): a bucket patched from part of itself would report a peak the samples do not have.

  **And the third thing, which is why the trigger could not have worked alone: a page was never registered to hear the announcement.** `/buffer_touched` is broadcast to `/server_notify` clients, the native front registers when a window draws a buffer, and the browser front registered for nothing — so the arm handling it was unreachable. It registers now, on the same condition, the first time a tree names a server buffer.

  The eye pass is `clients/web/examples/views/recording.html`: a second connection over the same in-page engine is a second *client*, it silences the middle third of the first take with the server's own verb and announces the span, and the lane reads it back. Zoomed out is the half worth watching — the gap is in the summary, not only in the run under the cursor.

- ✅ **The crossfade covers the levels and not the regime, so a transient jumps into a column it is not in** *(found 2026-08-18, looking for the crossfade algorithm H7 was said to touch — and it does not; this is what reading it turned up instead, measured rather than argued)*. `WaveformData::column` reads **raw samples** while a pixel is finer than the pyramid's base bucket and the **pyramid** at or above it, and the crossfade blends level N with level N+1 — never the raw regime with level 0. But the two do not measure the same span: raw reads exactly the column `[s0, s1)`, while `Pyramid::column` takes every **whole bucket overlapping** it, so an unaligned column of 256 samples reads 512. Measured on a signal with a single spike at frame 900 and a column at `[1004, 1260)`, which does not contain it: at 255.9 samples per pixel the column draws **0.0**, and at 256.1 it draws **1.0** — the same column, one zoom step apart, inking a transient that is a hundred samples outside it.

  **The fix is not a second crossfade, it is exactness at the edges.** Blending the two regimes over a zoom band would hide the step and keep the error; reading the **partial head and tail buckets from the samples** and whole buckets in between makes the pyramid's answer exact for any column, at a cost of at most two buckets' worth of sample reads — which the caller is already paying for in the regime just below, and which after H7 comes off a mapping. Then there is nothing to fade: the switch stops being a switch, `column` is one function of the zoom, and the crossfade above it goes on doing the job it is actually for. The alternative worth weighing only if that measures badly is to align the *columns* to buckets at the pyramid's zooms, which costs sub-pixel jitter in the drawing instead.

  It is small, it is not H7's, and H7 is what makes the edge reads cheap — so it is taken after it, and it wants the eye as much as the test: a spike appearing and disappearing across one zoom step is exactly the artifact a unit test can confirm and only a person can judge the size of.

  **Fixed 2026-08-18, and it deleted more than it added.** `peaks::aligned_stats` folds the whole buckets inside a span — the largest aligned block at each position, a segment tree's own walk, so it costs the logarithm of the span rather than its buckets and the answer no longer depends on which level it came from. The buckets are **tiled**: the span is rounded down at both ends, so consecutive columns cover every bucket exactly once — a bucket that straddles a boundary lands in one of them rather than in both (which duplicated and widened, the defect) or in neither (which would lose a transient outright). **The LOD crossfade went with the approximation it was hiding**, since there is no level to switch between any more.

  **What is left is a position error under one bucket** — one pixel at the zoom where a column *is* a bucket, shrinking from there — and that is the resolution the summary has. Two tests pin it: a transient is drawn in exactly one column and within a bucket of itself, and no column drops a bucket at a coarse zoom.

  **The first version of this fix read the two partial edges from the samples, which made it exact and cost forty times more** *(measured 2026-08-18, after the user asked where the CPU was going)*. A column of two buckets is ~500 samples of edge, so a 900-column frame over a ten-second take cost **703 µs** against **17 µs** for the fold — per frame, per channel, at every zoom between one bucket and eight. Exactness bought by reading the material on every frame is not exactness a picture can afford, and the regime that *does* read every sample is the one below, where a column is finer than a bucket and there is nothing else to read. The edges are gone; what they were buying is now a sub-pixel displacement.

  ```
  900 columns of a 10 s take, per frame      owned      mapped
  spp 200 (raw samples, exact)              153 us      528 us
  spp 533 (was: fold + sample edges)        210 us      703 us
  spp 533 (now: fold, tiled)                 18 us       18 us
  spp 8000                                    7 us        6 us
  ```

  The two tests that named the crossfade were rewritten around what is now true: a column measures its own span at every zoom, and the envelope is continuous where the regimes meet — trivially, because they do not switch.

- ✅ **A mapped take is still copied into the widget before it is drawn** *(named 2026-08-17 while closing H6, which says so in its own record rather than leaving it implied)*. `host::material` removed the conversation — no `/buffer_query`, no chunked fetch — but the samples are then read into the widget's own array and the peak pyramid is built over *that*. So opening a take is one local read rather than zero, and a second writer's edit does not appear in the picture until something refetches. The per-gesture cost, which is what the milestone was about, is gone either way.

  What it would take is a `WaveformData` that reads the mapped region rather than owning a `Vec` — which touches the peak pyramid's source, the LOD crossfade and the browser's own path (a page has no mapping and must keep the owned copy), so it is a design over `waveform`/`peaks` and not a change to the material module.

  **Taken as a milestone, 2026-08-18: it is H7**, on the user's "it is important, it has to be done". Reading the write path while staging it found the other half of the same defect — a stroke copies the material twice more, over samples already written — and the placement question the design was waiting on has an answer with three callers behind it. **Fixed by H7 the same day**, which is where the record of the fix is: the copy is gone in both directions, and the one that remains per stroke is the pyramid's, which is the sharing invariant rather than the material.

- ✅ **A reopened session draws less on every redefine, while the model is intact** *(found 2026-08-17 by the user, pressing save then open in `composer.py`: a clip's contents go, then the clip, and on the second cycle the takes' waveforms too — "but they keep sounding with play")*. The last clause is the diagnosis: what is played is the arrangement, and it is right. The **model round-trips** — measured over three save/open generations of that same four-lane piece, and again in-process against a live server: the tree the editor hands the host after `Editor.load` carries the take clips' `buffer`, the curve's `points` and the surviving rolls' `notes`. So the tree sent is correct and what is drawn from it is not, which puts this in the host's **redefine** path rather than in the client.

  Where to look, and why the shape is familiar: `/gui_def` on an already-defined root recycles ids and takes fresh ones, and this host has been bitten there twice already — a redraw dropping the handlers bound to the recycled ids (fixed in the client), and a buffer fetch that never completed because nothing woke the window (fixed here). A picture that degrades *per redefine* rather than per session smells like state kept against the old ids — a fetch keyed by a widget id that no longer draws that material, or a body's cached samples surviving into a widget that is now something else. **Not diagnosed further**, deliberately: it needs the window and a host-side trace, and guessing at it from the client is what the two earlier fixes already show costs a day.

  **Diagnosed 2026-08-17, and it was not the host.** The trace this entry asked for was taken — three save/open generations driven without a hand, `RUST_LOG=clausters_gui=debug`, a tree with two takes and a roll — and the host's data path is healthy in every generation: each redefine registers its wants (`gui_def N: widget W waits on server buffer B`), each fetch completes and reports `loaded into 1 view(s)`. Nothing is kept against a recycled id and nothing is dropped.

  **Both observed symptoms were the client's**, and each has its own entry with its own fix. The lane that emptied first was the **pattern** lane: a generator's reference was `repr(obj)`, so a reopened piece could never resolve it and drew a frozen leaf (`crates/clausters-document/PLAN.md`). The takes' waveforms went on the **second** cycle because the example wrote a source table built from the buffer read at startup, while reopening resolves each take into a *new* buffer — so the second save wrote a document naming sources the table did not contain, and the third open resolved nothing (`clients/python/PLAN.md`; `to_session` now refuses such a file).

  **What would reopen this**: a save/open pass on the fixed example that still loses drawing. The evidence above says the host redraws what it is handed; it says nothing about a tree that is handed less. **Run by hand 2026-08-17, three save/open cycles over the four-lane piece: nothing was lost** — every lane keeps its label, its body and its material through all three, which is the acceptance this diagnosis was owed.

- ✅ **A clip has two levels of editing and no interaction rules between them** *(opened 2026-08-17 by the user, after three attempts at the read-only body kept altering what the clip itself does: "you are tangling the code ... we need to set interaction rules we have not defined". The attempts were reverted; what they were missing is written here.)*

  **The two levels, in the user's own terms.** A clip as a **rectangle** — where it sits, how long it is — and its **internal elements** — the notes, the break-points, the samples it holds. They are different things to edit and they are edited by hands that mean different things, and the host has no rule saying which one a press belongs to. Every attempt to add one (a read-only body, a pinned placement, a contested pixel) landed in the same place: the rectangle's own behaviour changed, because the detection of the *internal* objects and the rectangle's parts are racing each other.

  **What is racing, precisely**: the clip's rectangle (its move), the **grip** at each end (its resize), the piano-roll's **notes** (editable, and a note is a wide region), and the **BPF's parts** (its break-points, which sit at `t = 0` and `t = dur` by construction, and its segments, whose bend is a gesture the body does not offer at all). Four claimants over one set of pixels, with the order decided ad hoc in three different places.

  **The context this has to be designed against, because the rules follow from what a clip *is*** (the user's, recorded so the next reader does not re-derive it wrong):

  - A clip is a **container of data — audio or MIDI**. In multitrack editing a clip is a *window onto a segment of a file*: it can be lengthened and shortened against the content it holds, and it can be **cut into pieces** so fragments sit in different places.
  - **Time-stretch is a separate story and is not implemented.** Today an audio clip can be stretched and the *drawing* of the waveform stretches with it, while playback still lasts the original buffer. Any rule about what an edge drag means has to say which of the two it is.
  - A clip may carry **automations laid over it** (the example's sweep BPF), acting on **that clip's content only** — clip-level dynamic envelopes, pans, and for generated audio the synthesis or MIDI parameters. So a curve inside a clip is not a lane of the piece; it is part of the clip.
  - A **MIDI clip is usually not edited in the multitrack at all** — a dedicated window is what a DAW opens. That window is not implemented, and editing notes directly in the multitrack is a good option to keep; but it is an option, not the given, and the rules should say which one is in force.

  **Why it went wrong three times, and it is not the reasoning:** the host's behaviour here is **only visible by looking at it**, and the changes were made without that. A rule that reads correctly and picks the wrong claimant is indistinguishable, on paper, from one that works.

  **What has to be defined, explicitly, before any more of it is built**: what a press means on each part of a clip, in which mode; whether "not editable" is one statement or two (the contents, and the rectangle); what an edge drag is (a trim of the window, or a stretch); and how a dedicated editor for a clip's contents changes all of the above. It is a design pass with the drawing in front of it, not a defect to patch.

  **Closed 2026-08-18, and the rule turned out to be one sentence**: *one layer is edited at a time, and it is the only one that acts or offers an affordance* (`host::layers`, and `docs/decisions.md` for the reasoning). What made it small is where it is stated — not a precedence between the four claimants, which is what every reverted attempt was, but a question each of them answers for itself: **is the pointer on my own material?** A break-point, the line between two of them, a note — never the rectangle the layer shares with its container, which is what leaves the background and the grips to the clip.

  The four questions the entry said had to be defined, answered:

  - **What a press means on each part of a clip**: it selects the layer whose material is under it and edits that layer; the background is on no layer's material, so it is the clip's move and the placement becomes the layer in hand again. The grips are the placement layer's affordance and are drawn only while it is active, so what is lit is what the press takes — with no exception written anywhere, which the old rule needed.
  - **Whether "not editable" is one statement or two**: two, and they are now separate props. `editable` is about the *contents* (a layer that cannot be edited is never selected by pointing at it, so the press falls through and the clip moves — closing the defect this entry was opened over). The *rectangle* is the placement layer, which a script addresses with `layer`/`hidden` like any other.
  - **What an edge drag is**: a **trim**. A clip is a window onto a segment of data (`SourceWindow`: `start`, `loop`, and `fit` for the picture a time stretch would make) and one timeline sample is one source frame, so an edge hides frames rather than compressing them and the ones it hides come back. Time-stretch stays unimplemented and now has a prop to land on.
  - **How a dedicated editor for a clip's contents changes it**: it does not change the rule, which is the point of stating the rule this way. A dedicated window is another container with its own layer stack; editing notes in the multitrack stays an *option*, and it is the `notes` layer being selectable that says so — a build that wanted the DAW convention would leave that layer unselectable and open a window instead.

  What shipped with it: `layer` and `hidden` on the wire (and in both clients' builders), the `"layer"` payload, the split (`e`) and join (`j`) verbs as intents, and the client half — the arrangement's `Vector` carrying its window, a trim recorded as **one** edit so one undo reverses it, and a join refusing by name the case the model cannot express (an element reading several segments).

- ✅ **A read-only clip body has no way to say so, so every drag on one flickers** *(found 2026-08-17 by the user, dragging notes on a pattern lane's clip in `composer.py`: "the notes cannot be edited visually, they flicker, jump and go back to their original place")*. The refusal itself is correct and is the acknowledgement working as designed — a `Sequence` over a pattern is forward-only, its notes are a *rendering* of an algorithm, and what comes back is the notes as they still are, which the host adopts by the ordinary drop-and-adopt rule (the client now sends a **reason** with it, `clients/python/PLAN.md`). What is missing is earlier than the refusal: **nothing in the protocol tells a widget its body is read-only**, so the roll offers the drag, draws it for its whole duration and then unwinds it. The user reads that as a broken editor rather than as material that cannot be edited here.

  **The user's verdict on seeing it, 2026-08-17: "if the refusal behavior is correct, the rectangles that represent notes should not be movable."** That is the acceptance, stated better than the entry had it: the picture must not follow a hand that cannot edit. It covers a second body the same way — a clip holding one placed **event** has no editable timeline either (its position is the placement's, so the clip is what moves), and today its note offers the same drag and unwinds it.

  The pieces to build it with already exist and the shape is **not** decided here. `signal` carries `editable` as one of its capabilities, so a body-level capability is the established vocabulary rather than a new idea; the draw mode landed the other half — a refusal that is **visible** *and* consumes the press — so a host has somewhere to put the answer. What has to be settled with it: whether the capability is a prop of `clip` (and then of the `notes`/`points` bodies it can carry), what a refused press *looks like* so it reads as "not here" rather than as a dead widget, and whether the same prop covers the case of an editable body inside a locked lane. It touches the host, `docs/gui-protocol.md`, `docs/gui-props.md` and both clients' builders, so it is a milestone rather than a fix — recorded here because the eye pass found it and the roadmap has no entry for it.

  **Done 2026-08-17, and the three questions it left open answered as they were asked.** The capability is a prop of **`clip`** (`editable`, default true), because a clip is what a script addresses and a body carries no id of its own — so one statement reaches every body the clip has, and a `/gui_set` moves it live (a lane locked while it plays, a generator's clip that becomes editable the moment it is rendered to a track). A refused press **looks like the draw mode's refusal**, which is the vocabulary already chosen: `"refused" "notes"|"points" <reason>`, visible, and it **consumes** the press so nothing behind it turns a refused edit into a selection. And it says nothing about the clip's own drag — a read-only body still moves and resizes as a placement, because *where* material sits is the composition's and *what* it holds is the material's, which is the same seam the whole layer is built on.

  The editor sets it where the material is a rendering: a clip whose notes have no editable timeline behind them (a `Sequence` over a `Pbind`, a `Generator`). The refusal the client already sent on the way back is unchanged — what is new is that it is no longer the *first* thing the user learns.

  **What it got wrong, left standing rather than patched** *(2026-08-17)*: the refusal consumes **every** press over a locked body, so a clip whose contents are locked cannot be **moved** either — and where material sits is the composition's while what it holds is the material's, which is precisely the distinction this does not yet make. Three attempts to separate them were written and reverted the same day, because each one changed what the clip itself does. The entry above this one is what they were missing.

- ✅ **Everything about a take was the first channel** *(found 2026-08-16 by the user, once a stereo take could be edited: it drew mono in the clip, sounded only on the left, and a drag that left its lane wrote values it was never aimed at; all three fixed the same day)*. Three places had quietly assumed one channel, each for its own reason, and they only became visible together. **The clip's take body** drew channel 0 and stopped — a clip is the picture of the material, so a right channel drawn nowhere is a picture of half of it, and it is also where an edit on that channel lands invisibly; it now stacks a lane per channel exactly as the standalone view does, sharing the same `overlay` choice so the two cannot disagree about what a channel is. **The monitor** played one `PlayBuf`, which in this server is a mono reader whose `chan` picks the channel — so the fix is the server's own convention rather than a wider def: one reader per channel, each out on the bus of the same number, freed together. And **the value under the pointer** was read in the lane the pointer was *over*: correct for a hover readout, wrong for an edit, because a drag belongs to the channel it started on — leaving that lane must clamp to its own end and not take the neighbour's mid-scale reading.

- ✅ **A stroke went on writing samples that had scrolled off the view** *(found 2026-08-16 by the user, drawing on a take in the session mode: drag out of the window and the pencil keeps writing where nobody is looking; fixed the same day)*. A drag holds the pointer, so it goes on reporting past the edge of the picture and past the window itself, and the drawing step turned every reported x into a frame. What that writes is only discovered by scrolling there afterwards, which is the worst shape a destructive edit can have. The pencil already had the rule it needed — it is refused where a pixel is more than one sample, *because a stroke there would write values the reader cannot see* — so the fix is that rule applied to the other axis: the stroke is clamped to the body it started on, the last visible column still following the hand. Clamped a second time to the material's last frame, which turned out to be a separate bug of its own: the right edge of a fully zoomed-out view maps to **one past** the last sample, so a stroke that reached it carried a frame the material does not have and the owner refused the whole stroke — a pencil that silently did nothing at the one place a user is most likely to drag to.

- ✅ **A take's clip and its editor disagreed after an edit, and the undo said so** *(found 2026-08-16 by the user while testing H4: the stroke landed in the editor, the clip above kept the old picture, and `Ctrl+Z` restored nothing; fixed the same day)*. Two widgets draw one material and they hold it in **different forms** — the navigable view keeps a peak pyramid, a clip's take body keeps the samples it was handed — so a host that patched "the pyramid of the widget under the pointer" reached exactly one of them. The undo made it visible rather than causing it: an inverse addresses a *node*, and the node's first widget is the clip, which had no pyramid at all — `cannot restore N sample(s): this widget is not drawing any material`, about material it was drawing. The fix moves the write to the element (which knows its own form) and picks the elements by **buffer number**, and it is the milestone's one real design finding, so it is written up in `docs/decisions.md`.

- ✅ **A buffer fetch finished only if something else woke the window** *(found 2026-08-16 while wiring H4's takes: the session opened, the query went out, the reply sat in the ring and the clip stayed empty; fixed the same day)*. An embedded server's replies are polled on the **main thread**, in `about_to_wait` — and an event-driven window sets `ControlFlow::Wait` when it has no animation, so after the last startup event there was nothing to wake it and nothing read the ring. The take appeared when the pointer happened to cross the window, which reads as a picture that does not load rather than one that loads late. The fix is the rule every other periodic reader here already follows: **a download in flight is a reason to keep waking**, so `BufferFetches::pending` joins the meter animation and the node-tree poll in choosing the next wake. It was never H4's alone — any window whose only server work is a buffer fetch had it, standalone bundles included, and it is invisible under a mouse that moves.

- ✅ **`R` reset nothing in a window with no signal view** *(found 2026-08-16 by the user, testing the session mode: zoom out with the wheel, press `r`, and the axis stays where the wheel left it; fixed the same day)*. The reset asked the tree for widgets that **navigate a signal** — a waveform, a spectrogram — and a multitrack has none of those: its lanes hold clips, and a clip carries notes, a curve, or nothing at all. So the key found nothing to reset and ran in silence, while the *wheel* zoomed the very axis it could not undo. The two have to agree about what the axis is, and the broader question was already written down beside the narrow one (`is_timeline`, which counts a lane and a time ruler as being on the shared axis). It matters past this milestone: any window of clips without signal bodies had the same dead key, which is most of what a DAW view is.

- ✅ **No `Ctrl` shortcut worked, and every one of them had been dead** *(found 2026-08-16 by the user, testing H3's undo by hand: `Ctrl+Z` did nothing, and neither did any other chord, while a bare key on the same path worked; fixed the same day)*. The window's shortcuts matched on winit's `logical_key`, which is the key **with modifiers applied** — so `Ctrl+Z` arrived as the control character `\u{1a}` and never equalled `"z"`. Nothing matched, nothing was logged, and the key was simply gone. It was not H3's: it took down **undo and redo** (H2), the **clipboard verbs** (D4's Ctrl+C/X/V) and the chords a `text` field and a `notes` roll handle themselves — all of which check `mods.ctrl` and were therefore written expecting a letter they never received. So none of them had ever run outside a test, which is what a shortcut nobody presses in a test looks like.

  The fix is winit's own answer to exactly this, `key_without_modifiers`, and it restores **only the letter**: a named key (Escape, Tab, the arrows) still comes off `logical_key`, so the text path's dead keys and IME behaviour are untouched. `Shift` is read where the rest of the host already reads it, from the tracked modifier state, which is what keeps `Ctrl+Shift+Z` a redo. The bare `z`/`a`/`s` that stood in while this was open are gone with it.

  **What it says about the tests, which is the part worth keeping:** every one of these verbs *is* tested, and every test drives the host's own `key`/`history` entry points — past the layer that turns a physical press into a letter. So the suite could not see it and no amount of it would have, which is exactly the seam the examples exist to cover.

  **What it got wrong** *(found 2026-08-27 by the user, typing into `oscsend.py` in the manual review: "accents cannot be typed, only lowercase ASCII"; fixed the same day)*. The paragraph above says the text path's dead keys and IME behaviour are untouched, and they were not: `key_without_modifiers` was asked on **every** key, and it drops `Shift` as readily as `Ctrl`. So a field on the native front could hold no capital and no composed letter — `A` arrived as `a` and `A` with an accent as `a` — and the widget looked like it only spoke lowercase ASCII. It is asked only when Ctrl or Alt is down now, which is the only case it was ever for; everything a person types comes off `logical_key`, shifted and composed. The seam this sat in is the same one the entry names: the layer that turns a press into a letter has no test, because a `winit::KeyEvent` cannot be built outside winit, and the suite enters below it.

- ✅ **A sweep to the first or last sample left the pointer over nothing, and the copy answered nothing — silently** *(found 2026-08-15 by the user, in the same `editor.py` pass, right after the copy started working: "it works except for the selections that include the beginning and the end of the buffer"; fixed the same day)*. A block operation is addressed to what is under the pointer, which is right whenever there *is* something under it — and a drag out to either end of the material parks the pointer in the window's margin, or off the window entirely, where `CursorLeft` has already left it at `(-1, -1)`. `hit` found nothing, `clipboard_key` returned `None`, and nothing was emitted: not even a refusal, over a selection drawn on screen. Moving the pointer back into the middle made the same Ctrl+C work, which is how the addressee was identified rather than the extremes.

  **The pointer still names the addressee whenever it is over a view** — the D4 decision is unchanged — and the *selection* names it when the pointer does not (`Host::selection_addressee`). Between several views carrying one, **the last selection made wins**: it is the only ordering a window has over its views, and every selection goes through one door (`set_timeline_selection`, whether a sweep, a `/gui_set` or an edit-back wrote it), so recording it is one line and it is never inferred. A selection cleared gives the title up, so nothing is addressed by accident.

- ✅ **A copy over a mapped take refused, because the picture was routed to the slot and the element was left holding nothing** *(found 2026-08-15 by the user, running `editor.py` and reporting that the keys did not work; fixed the same day)*. Ctrl+C on the waveform answered `refused: this source has no samples the host can read` — over a source whose raw file the host had mapped and was drawing from. The keys were never the problem: the probe showed winit delivering `Character("c")` with `ctrl=true`, `r` resetting the axes, and the verb reaching `clipboard_key`. What was empty was the **element**: `Loaded::Peaks` went into the front's GPU slot and only there, so `Data::body` stayed `None` and `sample_block` had nothing to read. The clipboard therefore could not work for exactly the views it was written for — a navigable heavy view is the one kind whose data a loader routes into a slot — while a `data`-inline element, which no editor uses, answered fine. That asymmetry is why the D4 tests passed.

  **A slot is where a picture goes, not where the material lives.** The pyramid is both, so it is now one shared `Arc<WaveformData>`: the loader hands it to the slot *and* to the element that named the resource (`frame::keep_material`, called at all four routing sites — the native walk, the page's fetch, and each front's buffer reply), and `WaveformView` holds an `Arc` rather than a copy, so keeping it costs a pointer. Nothing else is kept: an analysis is a reading of a signal and not the signal, so a spectrogram over a path still declines, and a **cache-only** source still declines for the right reason — an overview has no samples, which is the refusal the message was written for.

  It also closed a latent one in the same seam: `fill()` now offers a resolved body to the slot, so a window opened against a tree that already loaded its take fills from what the element holds instead of from a resource nothing asks for twice.

  The example was wrong on its own account, and by ear rather than by build: it wrote the audition buffer with a raw `send_msg("/buffer_set", bufnum, 0, *values)` — the wrong shape for that command (`FAILURE ... expected: bufnum, then (index, value) pairs`) and, at 210 kB for half a second of stereo, past any datagram. It uses `Buffer.read`/`Buffer.alloc`/`set_samples` now, which chunk it as blobs, and the clipboard voice takes its buffer as a control since each paste allocates one sized to the block.

- ✅ **The level body's fade is a guess, and it makes weight mean the zoom** *(found 2026-08-15 by the user, looking at the first `take.py` picture and asking how editors draw this; settled 2026-08-19, by eye, on the fourth answer)*. The body is measured over the pixel column's own samples, as an audio editor measures it, and it fades out where a column stops holding a meaningful average — both settled and recorded in `docs/decisions.md`. What is **not** settled is the fade itself: Audacity's manual says the RMS *disappears*, a cut, and the gradual ramp is ours so the picture does not pop at a zoom step. The ramp is opacity, linear in samples-per-pixel, measured against nothing — so a body at a third of its weight can be misread as a quiet passage, and its weight tracks the zoom instead of the signal. Alternatives to weigh: the cut the editors use, a ramp in a perceptual parameter rather than a linear one, or leaving the colour alone and narrowing the body. The threshold it ramps to (`RMS_FLOOR`, 256 samples a column) wants the same scrutiny: it is the pyramid's summarizing bucket and the order of a cycle of the lowest pitches, which is reasoning, not evidence.

  **Settled 2026-08-19, and the fade was the smaller half of what was wrong.** The rule is two, and they are one picture seen from either side. **A level is averaged over a fixed 50 ms of the source** (`BODY_WINDOW_SECS`, 2400 samples at 48 kHz), reaching out around a pixel column's centre wherever the column is narrower — because a root-mean-square is an average over a *duration*, and averaging whatever a column happens to cover makes the body's own **values** follow the view, moving over material that did not change. That is the defect the entry never named: the fade made *weight* mean the zoom, and the per-column window made the *reading* mean it too. The window is the editors' (WaveLab's default RMS window, adjustable to 999 ms), below the ear's own integration — ~200 ms for energy, 300 ms for a VU meter — so it is the floor of what reads as a level rather than the window a meter would pick. **And the layer ends where the envelope has come down onto it** (`BODY_MERGE_RATIO` = 0.8 over the span on screen, weighted by the peak): the envelope narrows with the zoom since a column covers less of the wave, and once it is within a fifth of the level there are no longer two readings — which is also what keeps a fixed window from ever drawing the body *outside* the envelope. One weight throughout, then a cut, which is the manual's own word.
  **Three answers were tried by eye first and each is in `docs/decisions.md` with what it got wrong**, because the sequence is the finding: a cut at the polyline crossing (right that the crossing is the picture's own, but far too late); a cut where the level *met* the envelope while still averaging the column (measured, and still moving under the zoom — and watching the level climb onto the peaks is itself the artefact); and a **duration floor on the column** at 50 ms and then 10 ms (right about what the quantity is made of, and an order of magnitude coarser than a working zoom — six seconds across a window is under seven milliseconds a column, so any perceptual floor removes the body from the place it is looked at). Only the fourth separates the two roles the duration was being asked to play at once: it is the **averaging window**, and the thing that ends the layer is the envelope. The `RMS_FLOOR` this entry was written about is gone.

- ✅ **A staff line was recognized by the page's width, so a short system on a wide sheet had no staff** *(found 2026-08-14 by H1's own acceptance test, running the reading against real verovio output rather than a fixture)*. `index_staves` took a horizontal stroke to be a staff line when it ran more than `0.3 * vb_w`, which is true of a page engraved to fit its music and false of one that is not: a two-note phrase comes out **21000 units wide with a 2838-unit system**, so the filter rejected every staff line and the page had no staves at all. Nothing had noticed because the only consumer was the ledger-line preview, which draws nothing when it finds no staff — a silent absence rather than a wrong picture, on pages nobody had engraved that narrow. It became visible the moment a *pitch position* had to be measured against the staff, since there was no staff to measure against and the edit was refused.

  The rule is now structural rather than proportional to the sheet: a staff line is one of the page's **longest horizontal strokes** (at least half the longest), which is what actually distinguishes it from the two things it can be confused with — a ledger line is a notehead wide and a beam a few noteheads, both an order of magnitude shorter. It lives in `clausters_core::notation::DisplayList::staves` and the host's `index_staves` is the same rule, because H1 made the two readings have to agree: `crates/clausters-notation/tests/staff_position_vector.json` is asserted by a test on each side, so a change to one that is not a change to both fails.

- ✅ **A clip could be resized out of existence** *(found 2026-08-13 by the user, right after the grip fix below; fixed the same day)*. Drag an edge past the other one and `dur` reached zero — and a clip of no duration draws no rectangle, so there was nothing left to press, at any zoom. The clip stayed in the tree, still addressable by `/gui_set`, invisible and unreachable: a state the user could enter with one gesture and could not leave with any.

  **The answer is a floor in three places rather than one**, which is why the fix is not a single `max`. (1) The **data** floor: a drag stops **one sample** short of the other edge — the smallest length the axis addresses, and a **length in the axis' own units, never a count of pixels**, so the same drag stops at the same place at every zoom (the rule the time selection already follows). A clip a *script* made shorter than a sample is not grown to one: a drag moves the edge it was given hold of, and stretching the far end out to a minimum nobody asked for is an edit of its own. (2) The **drawing** floor: a clip on screen is drawn no thinner than the **hairline** every divider in the host uses — a line marking where it is. The data floor alone does not answer the report, since one sample is far under a pixel at any musical zoom. (3) The **affordance** floor: a clip too narrow for two grips used to get none — "all body", movable and never growable — and now keeps one, its end (the start when the end is the one off screen), as wide as the clip and no wider.

  The three are one property stated at three layers: **every state a gesture can reach, it can leave by the same gesture.** The regression test is that round trip end to end — shrink a clip against its floor, check it is drawn as a line, zoom in, then grab it and grow it back.

  **The first version of this got two of the three wrong, and the user's correction is the entry worth keeping.** The data floor was one `snap` *step*, which reads plausibly and is wrong in both directions: zoomed in it refuses to trim below a grid step the axis plainly resolves, and it makes the shortest possible clip a different length on every lane. The grid says where an edge *lands*, not how short a clip may be. Worse, the drawing floor was `grip_w` — chosen so the collapsed clip would still be *grabbable*, which sounds like the point and is exactly the trap: a floor wide enough to aim at **freezes the clip's apparent length**, so it stops narrowing as the reader zooms out and stops widening as they zoom in, and length is the one thing a timeline exists to show. A floor may say *"a clip is here"*; the moment it also says *"and it is about this long"*, it is lying at every zoom but one. So the floor is the hairline, which claims nothing, and **the way back is the zoom** rather than the affordance: the line marks where to zoom, the sample widens with the window, and the expand grip rides on the line — never a plate hanging over pixels the press cannot be given, which is the rule the grip entry below had just established.

- ✅ **The grip was lit half a clip away from where it acts, and the press on it went to the notes** *(found 2026-08-13 by the user, on the roll clips of `gui_composer` and `gui_multitrack`, immediately after the note-clamp fix below; fixed the same day)*. Two halves of one rule, which is why they are one entry. The **press** on a clip's grip fell through to the clip's bodies — they were offered every press first — so those dozen pixels went to whatever the body found under them: a note at the end of a roll clip moved instead of the clip's edge, with the cursor sitting on the arrow that promised the opposite. And the **drawing** lit the grip of whichever *half* of the clip the pointer was in, so a strip `grip_w` wide announced itself from the middle of the material, where there is no resize at all.

  The two are the same defect seen from either end: **an affordance is drawn where it acts, and the thing that is drawn is the thing that acts.** Lit by half, the mark was a promise the press could not keep, and both possible fixes on their own make it worse — widening the grab zone to the half would swallow half the roll, and leaving the press with the bodies while lighting only the strip would mean the arrow never does anything. So the strip is the unit on both sides: `clip_grip_at` lights the grip the pointer is **on** (`clip_grips`, the same function `interact::clip_part` hit-tests through, so the pixels cannot disagree), and `gestures::press` offers the bodies everything *except* a grip. A pointer over the material lights nothing, because nothing there resizes; the body keeps the whole rectangle whenever the clip's ends are off screen or it is too narrow to carry a grip, which is the case that already existed and is unchanged.

  One behaviour changed on the way and it is deliberate: a clip being **moved** now lights no grip. The held-clip rule exists because a clip moves in `snap` steps and the pointer does not, so a resize keeps the edge it holds wherever the pointer drifts — but a *move* holds no edge, and the grip it used to light under the drifting pointer offered a gesture that was not the one happening. The frame test that pins the topmost-clip rule was rebuilt around the only place two clips can now both claim the pointer — where one's end strip and the other's start strip land on the same pixels — which is a sharper statement of the same invariant than the half-and-half version it replaces.

- ✅ **A note could be dragged out of the clip that draws it** *(found 2026-08-13 by the user, on the roll clips of `gui_composer` and `gui_multitrack`; fixed the same day)*. Drag a note rightwards past the clip's end and it kept going: still in the list, still played by whatever reads the edit-back, and drawn on no pixel the clip owns — a body is clipped to its clip's rectangle. It could be found again only by resizing the clip by hand, which is the report's own wording for it and the reason this is a bug rather than a quirk: the editor's whole claim is that what you drag *is* the music, and a note nobody can see is a note nobody can drag back.

  **The choice was between two habits, and the DAWs settle it in a third place neither option had.** Auto-growing the clip to take the note was the other candidate; it is what a DAW does *not* do, because a clip's length is set by its own edge, and content that lengthened its container would make one gesture — nudge a note — also move the end of the piece and the scroll under it. What real editors actually do is escape the dilemma: notes are edited in a separate piano-roll window that **draws the space past the clip's end**, shaded, so a note out there stays visible and simply does not sound. That third way is not available here — the roll *is* the clip's body on the lane, so "outside the clip" has nowhere to be drawn — and once it is off the table, stopping the note at the edge is the only option that keeps what is edited visible. It also matches what the arrangement already assumed: `gui_multitrack`'s lead cuts its notes to the clip's length when it builds the pass, so the clip was already the authority on what plays.

  **The rule is the placement's, not the roll's**, which is what makes it one fix for both examples and for the raw `clip(notes=...)` alike. `Notes::edit_limit` reads the seam that already exists — a clip's body is handed its container's axis with no gutter (`indent == 0`), the roll's own view has a keyboard gutter and navigates — and hands a `pianoroll::Limit` to the pure edit functions. A body clamps into `[0, dur]`; the roll standing alone gets `None` and is **deliberately** left unbounded, because its view spans its own content: the note lands where it was dropped, the span grows, and the axis scrolls to it. Bounding that one would take away the one thing the standalone view can do (the user named this in the same pass, on `gui_pianoroll`).

  Two details the clamp turns on. A note is clamped **whole** — its tail parks on the far edge, not its onset — since the part that would vanish is exactly the part being dragged; a note longer than the clip pins to zero instead, the near edge being the one that can be honoured. And a **block** move stops on its last tail with the spread intact, the same way it already stopped on time zero and on the pitch window. The same edge holds a resize's end and an `osc` marker. Shortening a clip over notes still deletes nothing: they stay in the list and come back when it is stretched again. Covered at both levels — the pure functions in `pianoroll`, the seam in `notes` (both placements in one test, since the bug was one of them behaving like the other), and a real press-and-drag on a clip through the gesture machine.

  **Open, and it is the client's half** *(named by the user in the same message)*: the host clamps a gesture, and the arrangement model behind it (`clausters.form`) holds a structure that is only partially evaluated — a generator element is bounced to be drawn — so what a clamped note means for an element that has not been rendered, and for one whose notes came from a pattern rather than from a list, is a question this fix does not answer. It is filed on the Python roadmap.

- ✅ **A round thing was hit-tested as its bounding box** *(found 2026-08-12, named by the user — the bounds of a circular figure, computed with Pythagoras squared, and generalized across the elements; fixed the same day)*. Everything the host draws answered the pointer through `Rect::contains`, which is right for the rectangles and the strokes and wrong for the round ones: a knob's dial is a disc inside a cell that also holds a label strip and a read-out (and whatever width the row spread it to), so pressing the name, the number or the paper in a corner grabbed the value and turned it; a notehead is an oval whose box has a quarter more area than the glyph, and on a dense page that quarter is where the stem, the beam and the note on the next line live.

  The same audit, by eye on `gui_panel`, found two more and they are the same bug without a circle in them: a **slider**'s track area is the cell minus the label and the read-out, and what is drawn in it is a groove a few pixels thick with a short grip riding it, so a press anywhere in a band several times that thick jumped the value; a **toggle** stretched across a row is a small box with a word beside it and a great deal of air after that, and the air flipped the value. So the entry is not about circles: **a hit-test is the shape that was drawn**, whatever that shape is. The knob, the slider and the toggle are verified by eye — the toggle in two passes, since the first bounded its width and left the cell's full height, and in a row of mixed controls that height is the tallest sibling's: a shape is two axes, and bounding one of them reads exactly like bounding neither. The rest of the chrome has not been done at all, which is E23, and why this entry closes on the seam rather than on the catalog.

  Which is why it ends at the trait rather than as a guard per leaf — the user asked the question directly, and it is the right one. `Element::hit_area` returns a `HitArea` (`Rect` by default, `Disc`, `Ellipse`), and `gestures::element::press` is the **one** place it is applied, adding the metrics' `hit_slop`: an element states its shape once, never writes the filter, and a point off it reads exactly as a decline, so the press falls back to the chain the way any declined press does. A leaf that says nothing keeps the whole rectangle it always had, so nothing else in the catalog moved. Each shape is read off the same function the drawing places it with (`knob_disc`, `slider_groove`, `toggle_hit`), so the two cannot disagree. The one deliberate exception is the **overlay** route: a modal is offered the press precisely *because* it is outside as often as because it is inside (a click on the window closes a menu's list), so it goes through the unfiltered door.

  `host::graphics::shape` is the shared answer — `in_disc`, `in_ellipse`, `dist2` — and it is **squared throughout**: a distance in a hit-test is compared (against a radius, against the best so far) and never reported, and squaring preserves the order, so the square root is work with no reader. The ellipse is the same test normalized per axis. Three call sites so far: the knob presses only on the disc the drawing places (`controls::knob_disc` is now one function for both, so they cannot disagree) plus the `hit_slop` every small target gets, declining otherwise so those pixels keep meaning whatever the window means by them; the break-point grab drops its `sqrt`; and the score's hit index carries a `HitShape` per entry, `Ellipse` for a glyph in the SMuFL notehead range and `Rect` for everything else — per entry because only the indexer knows what it measured. What stays rectangular is what *is* rectangular: patcher boxes and ports, roll notes, keys, lanes, clips.

- ✅ **A selection was a band of pixels, so it could select where there are no samples** *(found 2026-08-12, by eye on `gui_bulk` at sample-level zoom, and stated as a rule by the user; fixed the same day)*. Zoomed in until consecutive samples stand apart with a dot on each, a sweep drew its band starting and ending **between** two dots: a region holding no data, which can be neither played nor cut nor read back as anything. The sweep was in pixel coordinates all the way down — `sample_at` gives a fractional position and nothing rounded it — so the number the `"selection"` event reported was a length in fractions of a sample.

  What the selection *means* is a count of samples, so the axis it is measured on is the sample: every write goes through `timeline::snap_selection` (the sweep) or `.round()` (the `/gui_set` keys, the def-time seed). Two calls inside that, both the user's, and both are about the selection agreeing with the hand rather than with the pixels. **Which samples**: the ones the sweep *passed over* — `ceil`/`floor` — not the ones it came nearest. Rounding to the nearest takes a sample in half a sample-width before the cursor arrives and drops one it has already passed, so at sample zoom the band leads and lags the hand; a sweep that has reached no sample selects none, which is the click it started as. **Where the band's edges are**: halfway between the last selected sample and the first one left out (`[start - 0.5, start + len - 0.5]`), because each sample owns the half-width on either side of it. Drawn edge-to-edge the band ends *on* the last selected sample and reads as excluding it — the ambiguity only a sample-level zoom makes visible.

  The rule is the **navigation group's**, not the waveform's, which is what makes it hold for the spectrogram the user named in the same breath: an editor lays the two views over one timeline and they read one selection, so a rule living in either view would have to be written twice and would disagree at the edges. `docs/gui-protocol.md` and both clients' builders say it now.

- ✅ **The trace drew outside its lane, again — so the drawing bounds itself now** *(found 2026-08-12, by eye on `gui_bulk` right after the two renderers became one; named by the user as a recurrence of the clip fix, and as a behavior that should be general; fixed the same day)*. Zoomed into the polyline regime, the sample discs and the stroke around them sat **on the vertical ruler beside the view** and over the labels under it. It is the same overshoot a clip was taught to mask a commit earlier: a trace resolved per visible pixel reads the sample before the left edge and the one after the right — it must, or the line would start and end inside the box — and a value outside the vertical window lands outside the lane the same way.

  What the recurrence says is that the mask was in the wrong place. A clip masks its bodies because a clip is a coordinate system, and that is right; but a free-standing `waveform` is **nobody's content**, so no container mask is in force and the overshoot goes wherever it goes. Fixing it per placement means every future container that holds a trace re-learns this. So the answer is the drawing's: `trace::draw_channel` narrows the mesh's clip to the lane it was handed — never widens it, and puts back whatever it found — which bounds *every* destination at once, on both axes, including the vertical one whose doc comment already claimed a clip rectangle was cutting it. Two consequences worth stating: a column widened to the trace weight at either end of the rect is shaved by the lane rather than hanging over it, and the tests that counted vertices through the edges now count them inside. The regression test draws a signal that leaves the window on both axes and asserts no vertex outside the lane.

- ✅ **Two renderers drew a navigable waveform, and only one of them was right** *(found 2026-08-12, by eye on `gui_bulk` closing the visual pass, and named by the user: “why are there different ways of drawing a waveform with zoom that do the same thing? there should be only one”; fixed the same day)*. Zoomed past the polyline threshold, the trace **arrived at a sample and stopped**: a line came in from the left edge, reached the one visible sample, and nothing left it. The signal continues on both sides, so the segment to the sample past the right edge was simply never built — the pipeline took `floor(start)` and `ceil(start + len)`, exclusive, which is one sample short on the right.

  The off-by-one is not the entry. The entry is that there were **two implementations of one picture**: the mesh renderer (`signal::trace::draw_channel` — a clip's take, a plot, a meter's history) and the navigable view's own `wgpu` pipeline, which built its own vertex buffer through its own WGSL. The previous entry below folded the *live* renderers into the mesh one and left the pipeline standing, on the strength of `tests/gpu_slot_cost.rs` — which, read again, says the opposite of what it was taken to say: the per-column **computation** dominates so completely that where the vertices land is noise. The pipeline bought nothing and cost a second place to write the same drawing, so it drifted in four ways at once: the missing right-hand neighbour, sample dots as **squares** where the mesh draws discs, the regime split on the other side of the threshold (`<=` against `<`), and a minimum ink of one physical pixel against the mesh's trace weight.

  So the pipeline is gone — `WaveformRenderer`, `WaveformGeometry`, `Mode`, `waveform.wgsl`, the vertex buffer, the per-lane `Framing` and the whole upload/draw walk in `frame::render`. `waveform.rs` is now the **data** (raw samples plus a peak pyramid per channel) and a navigable view's **vertical state**; `waveform::draw_lane` places it on its two axes and hands it to `draw_channel`, which every signal against time now reaches the screen through. `Renderers` holds one pipeline, the spectrogram's, and that asymmetry is the point: a spectrogram's picture is a texture sampled once per pixel, which is work triangles cannot do, while a waveform's columns were folded on the CPU either way. The regression tests are the two behaviours that differed: a window narrower than one sample **enters and leaves** the rect, and a cache-only source (a pyramid with no samples) never enters the line regime at all — which is also a real fix, since asking it for a sample reads an empty buffer and the wave vanishes exactly where the reader zoomed in to look at it. `docs/decisions.md` carries the rationale and what was given up with the pipeline (`Framing` is the spectrogram's alone now); `docs/architecture.md`, the skill and the slot-cost test's own narrative are updated.

- ✅ **A take drew nothing in its clip: an element drawn as a body reached its own drawing** *(found 2026-08-12, by eye on `gui_multitrack` opening the visual pass; fixed the same day)*. The three lanes of files came up as empty frames with a line across the middle. The line is what named the cause: it is the baseline `draw_take` puts down before the trace, so the data was there and the trace was not — the element was drawing, and drawing the wrong picture. A clip's body is an element like any other, and the frame hands it its container's axis on `Ctx::time`; the roll and the curve read it, and the signal element did not — so it fell through to `plot::draw`, which reads the **inline samples** and rules its own axes. A bulk take has no inline samples (it has its pyramid), so the plot had nothing to read, and `SignalElement::draw_body` — the body drawing the model documents, with the trace against the clip's span and no chrome of its own — was reachable from nobody. The fix is the missing fork, ahead of the presentations: placed on somebody else's axis, a stored non-navigable signal draws as a body. What let it survive is that the two shapes agree wherever a take is a sketch — an inline `data` clip drew *something* through the plot, which is what the tests and the older examples carry — so only a mapped take, the case the wire exists for, showed it. The regression test is exactly that case: a pyramid, zero samples in the tree, and one column per pixel of the clip demanded (36 vertices without the fix — the baseline alone — over 704 pixels).

- ✅ **A rectangle over a waveform is not what a drag means** *(found 2026-08-14 by the user, by eye on the D3 pass: "the kind of selection is inverted... over the waveform a spectral selection (an arbitrary rectangle) can be made, and below is the spectrogram, where only a time selection can be made"; fixed the same day)*. D3 shipped the value band on the **plain drag**, so a sweep over a waveform came back as a rectangle while the spectrogram beside it — the picture that actually has two measured axes — came back with a stripe. Nothing was swapped in the code, and that is what made it worth fixing rather than explaining: the reading is right even though the wiring is. A drag over a waveform means *this stretch of time* in every editor there has ever been, and a marquee that also cuts a band of amplitudes out of it answers a question nobody asked.

  **Fixed by making it a step**, which is the track's own first decision applied to itself: a mode is a gesture plan, not a state the host decides. `select` sweeps the span; **`select_box`** sweeps the span restricted on the second axis, and it **declines** where the picture measures only time — so `"select_box select"` is one binding that draws a rectangle where there is one to draw and the plain span where there is not, and the day D5 gives a spectrogram its bin range that same binding answers there with nothing rewired. The user's own note settles what D5 has to preserve: *both* selections belong on a spectrogram, the temporal sweep and the bins rectangle, which is exactly what a two-step plan per modifier already expresses. `editor.py` binds it on Ctrl and says in its header what each view does with it.

  Worth keeping as the general shape rather than as this one fix: **a capability whose sibling does not exist yet must not be the default gesture**, because until the pair is complete the half that shipped reads as belonging to the other view.

- ✅ **An element's look does not answer for the space it is given** *(found 2026-08-12, by eye on `gui_multitrack`; the rule stated by the user in the same pass; the three concrete halves paid as they were found, the general one closed 2026-08-19)*. Three things the multitrack shows, and they are one entry because they are one rule: **the visual properties of an element belong to the element, and each one answers what it does when its size and its visible space change — including when part of it leaves the screen.** Today that answer is the container's or nobody's, so each of the three is decided at a different place and none of them by the thing being drawn.

  - **A clip's name follows the clamped rectangle instead of the clip.** `clip_rect` is the *visible* part of the clip (`clip_x_range` clamps the span to the lane body), and `draw_clip_label` writes at `cr.x + m.pad` — so a clip scrolled half off the left has its name at the screen edge rather than at its own start, and every clip that runs off the left stacks its name in the same place (two names overlap in the screenshot). It is also written with plain `font::text`: no width, so it spills out of a box narrower than the string, where the lane's *header* right beside it truncates with an ellipsis (`font::text_ellipsis`, the light widgets' rule). The roll's cursor readout does the same thing in the same clip.
  - **The clip's border reads as a grip, at whichever pixel the clamp landed on.** The border is drawn on the clamped rectangle too, so a clip half off-screen gets a vertical edge *in the middle of its material* that looks exactly like the resize affordance the open entry below wants to draw — and where two clips overlap, one's fill buries the other's, so the edge disappears. Whatever shape the grip takes, it must be drawn where the clip's **end** is and be absent when that end is not on screen; and its vertical extent has to mean something, which today it does not (a lane's height is the track's, not the clip's).
  - **The transport's read-out is lost as the window narrows.** The counter beside the buttons is cut and then gone, with no rule about what goes first — a strip that cannot fit its contents has to say what it drops (a shorter unit, fewer forms, an ellipsis), the way a lane header already drops its controls before its name.

  It is an L-track question (what the look says a part is) and it overlaps the affordances entry below: answering either alone would give the two a different vocabulary, so they converge into one milestone. What this entry adds is the general half — the rule is not "clamp the label too", it is that an element carries its own answer to *less room than I want* and the container asks it, rather than each drawing site improvising a clamp.

  **What shipped the same day, and what deliberately did not.** The two texts now stay in the box that holds them, each dropping the end that matters least: a clip's name truncates with the ellipsis to its own visible rectangle (`font::text_ellipsis`, the rule the lane header beside it already followed) and draws nothing at all when there is no room for a glyph, so a sliver of a clip no longer writes over the neighbour that starts there; the roll's cursor read-out is right-aligned **inside** the grid and gives up its tail — the time — keeping the note name, which is the half a pointer on a pitch is asking for (unclamped it started 18 px left of a 40 px body, which is what the regression test pins). The transport read-out was the **example's**, not the host's: the buttons were elastic and split the row four ways, so the counter — the one thing there whose width is its content — got a quarter of the row and came back ellipsized. The buttons take a fixed width now (chrome is a fixed strip; a caption's budget is its box divided by the character cell), and the host's ellipsis stands as the honest answer to a window narrower than the text. **Left open, and it is the milestone**: the two fixes are each a clamp at a drawing site, which is exactly what the rule above says not to settle for — an element still cannot be *asked* what it does with less room than it wants. And the grip stays undrawn (below). *(The grip closed with the entry below on 2026-08-17, ends-aware and no wider than the clip; the general half is the paragraph after this one.)*

  **Closed 2026-08-19 with the general half, and it turned out to be one number a widget was never asked for.** The rule wanted a widget to carry its own answer to *less room than I want*; what the layout actually lacked was the **question**. `strip()` resolved a wanted size and stopped there — `leftover = (main - gaps - fixed).max(0.0)` — so a strip shorter than its children placed them at full size anyway and the last of them was drawn past the edge, with the elastic ones collapsed to nothing beside it. A container had no way to ask for room back and no way to know what asking would cost, which is why every answer so far had to be a clamp improvised where the drawing happened.
  So a widget now answers at **both ends**: `Element::floor` (through `WidgetKind::floor_size` and `Widget::floor_size`) is the smallest size it still says what it is at, the layout takes the deficit out of the difference between the two **in proportion to what each child offered**, and three rules keep it honest. A widget that offers nothing keeps what it asked for and the strip **overflows visibly** — being squeezed is a thing a widget consents to, not something done to it, and a picture running off the edge is a better report than a control silently cut. An **explicit** `w`/`h` is never squeezed, or the one way to pin a size would stop pinning it. And an **elastic** child gets no floor at all, because sharing the leftover already takes it to zero — which is what keeps this the same single pass with no solver in it.
  **What a control gives up is its label strip**, and the reason it is the right first thing to lose was already written at the drawing site: `controls::label_height` had been dropping the caption once a cell could not hold both — "a value with no caption is merely terser". The floor now names that same term (`controls::label_give`), so the room the layout reserves and the room the drawing takes cannot disagree: offer a strip the drawing keeps and the body gets cut instead; keep one the drawing drops and the control floats in dead space. `knob`, `number`, `slider`, `menu` and `text` answer with it; `button`, `label` and `toggle` have no part to drop and say so by not overriding the door. A hugging container composes its children's floors on the walk `hug` already makes, so a hugged panel offers exactly what the controls inside it offered.
  **What this does not do, stated so it is not looked for**: it does not touch what a widget *draws* at a given size — the shared vocabulary for that is `font::text_ellipsis` and `plate_text`, and `label_height` is the one shape rule. This is the door beside them: the size arrives having been negotiated rather than assumed, which is what the entry's three findings each lacked.
  **A fourth thing the same pass asked for, and it is the general half arriving early: the text plate.** With the two texts inside their boxes they were still *illegible*, because a caption over a picture is written in one color and the picture draws in another — wherever the two meet the line dissolves into the material, and the denser the take the less of the name survives. So a line drawn over a picture now sits on a ground of its own: `graphics::plate_text` — a translucent, rounded box under the glyphs, truncated to the room it was given, returning the plate it laid down so a caller stacking two knows where the first ended. It is **one piece in the models' shared vocabulary** (beside `corner_text`, the same reach) rather than a clip feature: its signature names no clip, no lane and no axis, so the roll's read-out took it unchanged, and the two properties that make it a *ground* are what the test pins — it is as wide as the line it grounds (a truncated caption takes a truncated plate, never the plate of the string it would have drawn) and where not even the ellipsis fits there is no plate at all. Both of its numbers are **roles**, not literals at the drawing site: `plate` (the color, whose alpha *is* the role — an opaque plate would be a label with a background, a different thing) and `plate_radius` (the corner), so the density scale and a window's `ui_scale` carry them like everything else. Documented in the client book's color and size tables and in `docs/gui-protocol.md`'s text paragraph.

- ✅ **A take is drawn in amplitude and heard in decibels** *(found 2026-08-12, by ear on `gui_multitrack` after the envelope fix below; deferred by decision — it belongs with the revision of multitrack playback and the arrangement model)*. With the picture and the voice finally built from one description, the sound of an audio clip still stops about a **quarter of the clip** before the playhead reaches its end. What is left is not a timing error: it is that the trace is drawn **linearly in amplitude** and heard **logarithmically**. The example's fastest take (`exp(-5t)`) is at 0.024 of full scale three quarters through — **−33 dB**, silence to the ear — while the drawing still puts a line there, and the minimum-ink floor *guarantees* it does: a column is inked at least one pixel, which is exactly the rule that keeps a flat stretch visible. The two rules are right on their own and disagree here.

  The measurements, so the next pass does not re-derive them: at three quarters of its span the example's three takes sit at −32.6, −7.8 and −13.0 dB (decays 5.0, 1.2, 2.0), and at the very end at −43.4, −10.4 and −17.4. The slow ones are plainly audible to the end, so what the ear generalizes from is the fast one — which is also the one whose picture is emptiest.

  Two answers, and the choice is a design rather than a fix. **Draw the body on a dB scale** — the amplitude axis already *labels* in `db`/`bits`/`percent` (the ruler's units of full scale), but the geometry is linear, so this means a display mapping a take body can carry, and a decision about its floor. Or **accept the divergence and say so**: an editor's waveform is amplitude, and a reader learns to read a decay. Whichever wins, it wants the same call the trace renderers took — one rule for every view of a signal, not one for a clip body.

  **Answered 2026-08-21 by the user: the second one. Editing is in linear
  amplitude and only there, and the geometry stays as it is.** What settled it
  was a fact neither answer had: the `db` ruler already places its rungs **by
  amplitude** — −6 dB sits at amplitude 0.5, not halfway
  (`ruler::amp_ticks`, and its own test says so). So there is no lie on screen
  today. The axis is linear in amplitude and the decibel labels are drawn where
  those values actually fall, which is a coherent answer already, and the ink at
  −33 dB is the minimum-ink floor doing exactly what it is for. A dB *body*
  would also have to choose a floor, which is a parameter with no natural value:
  a wrong floor is a different lie, not one less. A per-view dB body remains a
  legitimate **feature** — the editors that offer it offer it as a view option —
  and it is not this fix.

  **The safeguard is an assertion and not a check, because there is nothing to
  check.** Grepping every use of `ruler_y` outside `ruler.rs`: they are the prop
  itself (parse, presets, the axis vocabulary), whether the strip exists and how
  wide it is (`!= Off`, `amp_strip_w`, the strip's hit-test), and labels and
  read-out strings. Not one of them reaches the value. The value under a height
  comes from `ValueAxis::value_in` → `waveform::display_to_value(display,
  domain.0, domain.1)`, and `ValueAxis` carries **no unit at all** — body,
  window, domain, lanes. A runtime guard would defend an input nobody can write.
  So what holds the decision is
  `signal::tests::the_amplitude_unit_labels_the_axis_and_never_maps_it`: the
  same value at every height under `norm` and under `db`, the two bodies
  vertically identical (the unit may move the body *horizontally*, since its
  strip is as wide as its own labels), and the mapping affine. It fails the day
  the body's geometry becomes logarithmic, which is exactly when somebody should
  come back and read this. The rule is written where a reader would otherwise
  "fix" it: the `RulerY` enum, both clients' `waveform` builders, and
  `docs/gui-protocol.md`'s axis table.

- ✅ **The take sounded a different envelope than the one it drew** *(found 2026-08-12, by ear on `gui_multitrack`: the sound stopped a little before the playhead reached the end of a clip; fixed the same day)*. The first suspicion was the playhead, and it was wrong — worth recording, because the measurements are what turned it around. `Transport.anchor` already compensates the latency (the line marks where the audio is *heard*, not where it was scheduled), the query it takes after starting the pass costs **1.9 ms**, and the engine's sample clock runs ~1 % behind wall time, which over a three-second clip is 30 ms: none of it is a quarter of a beat, and none of it explains a gap that shows on a clean play.

  What the reader was seeing is that **the picture and the sound were two different envelopes**. The take drawn on the lane is a file written with `exp(-decay·t/secs)`; the instrument playing it was a ready-made percussive envelope (`Env.perc`, curve −4). Rendered offline and measured at the same instants, a four-beat take at decay 1.2 is at **0.301** in the picture and **0.001** in the sound at the clip's end — inaudible from about three-quarters through, where the drawing still shows material. Hence *a little before*, hence every audio clip and not the roll (whose notes are drawn as bars, so nothing contradicts them), and hence the spectral clip being fine: the sweep's file and its synth already share one envelope, which the same measurement confirms end to end.

  The fix is in the example, and it is the example's own headline claim — *what sounds is what is drawn* — finally holding for the takes: `multi_tone` takes `decay` as a control beside `secs` and plays the file's curve (a ramp over the clip's span, exponentiated, the ramp being what frees the synth at the end). The three takes now match their files to within 0.008 at every position. **The general lesson is worth more than the fix**: an example that draws one signal and sounds another reads as a *timing* defect, and sends the reader after the clock. When a view and a voice are meant to be the same material, they have to be built from the same description — the arrangement's rule, one level down.

- ✅ **A clip did not bound what it drew** *(found 2026-08-12, by eye on `gui_multitrack` zooming into a take; fixed the same day)*. Zoomed past the polyline threshold, the sample dots at both ends of a clip were drawn **outside** it, on the lane beside it. The drawing is right to read them: a trace resolved per visible pixel takes the sample before the left edge and the one after the right, or the line would start and end inside the box — but once each sample is also *marked*, that overshoot stops being a line running off the edge and becomes a pair of discs sitting where no clip is.

  Fixed as containment rather than as a trim, and that is the whole of it: a clip hands its bodies a **mask of its own box** (`layout::place_on_time`, intersected with whatever mask it inherited), so every drawing placed inside one is bounded by it and no renderer has to know where the edges are. A clip is a coordinate system; one that does not bound its contents is a rectangle they happen to start in. The regression test draws twelve samples over a 400-pixel clip — far enough apart that the polyline and every dot are drawn — and asserts not one vertex of the bodies lands outside the clip's rectangle.

- ✅ **The other text over pictures has no plate yet** *(found 2026-08-12, answering where `plate_text` belongs after it shipped)*. The plate is in the models' shared vocabulary and two callers use it — a clip's name and the roll's cursor read-out. Five more sites draw text over a *picture* with a bare `font::text` and want the same ground, and they are one change rather than five, since each is one call: the heavy views' cursor read-out (`frame/draw.rs`, time + value over the waveform or the spectrogram), the `plot`'s (`signal/plot.rs`, `440 HZ  -12.3 DB`), the goniometer's and the spectrum's and the scope's labels (`signal/phasescope.rs`, `meters::label_strip`), and `corner_text` itself, which is the same problem in the opposite corner. What deliberately stays **unplated** is text on **chrome** — ruler labels, a lane's header, every control — because there the ground is already the widget's own; the plate is for where the background is *material*. Not done with the multitrack pass because adopting them is an eye pass over the live views (four of the five are in `gui_analyzer`), and that is a session of its own rather than a change to wave through.

  **Done 2026-08-21, and the eye pass found three sites rather than five** —
  which is what the pass was for, since the list above was written from a grep
  and the rule is about what a line *sits on*. Plated: the heavy views' cursor
  read-out (`frame/draw.rs`, time + value over the take or the spectrogram), the
  `plot`'s (`signal/plot.rs`, `440 HZ  -12.3 DB`) and `corner_text` itself,
  whose whole point is that the slot is *inside the body* — a scope's
  `lock`/`free` tag sits on the trace, and the corner is where a signal is least
  often quiet rather than most.

  **The other four are one thing, and it is chrome.** The goniometer's label,
  the spectrum's, the plot's and `meters::label_strip` are the same eight lines
  copied four times: a widget's label drawn in the strip *above* its body, and
  the strip is **reserved** — every one of them goes through
  `controls::body_rect(rect, label.is_some(), m)`, which takes the label's
  height off the body before anything is drawn. So no label overlaps a picture,
  the ground under them is already the widget's own, and plating them would have
  put a translucent box on flat chrome for no reason. The entry's own rule said
  so; the list did not. The phasescope's `r +0.52` is the near miss: it is not
  over a picture either, but it *is* centred on the correlation bar's own fill,
  which is the rule the controls follow the other way round (a control's
  read-out gets a reserved strip at the bottom "so it never lands on the thing
  it is reading"). Left alone, and named here rather than fixed, because it is a
  layout question and not a legibility one.

  **What the pass also found and did not take**: those four label blocks are one
  helper written four times, and `meters::label_strip` is already that helper.
  It is written down in "Found by use" below rather than folded in here, since a
  cleanup riding in a legibility fix is how a diff stops being readable.

- ✅ **The patcher's model is not behind the `patcher` feature** *(found 2026-08-12, auditing where the host's files sit for K14)*. K13 gave the two element families a feature each, and `notation` gates both halves — `host::score` (the model) and `elements/score.rs` (the leaf). `patcher` gates only the leaf: `elements/patch.rs` and `widget::parse::parse_patch`/`parse_ports` carry the attribute, and `host/graphics/patch.rs` — 1570 lines of boxes, cords, hit-testing and geometry — does not, so a build that dropped the family still compiles all of it and nothing can reach it. It is not a break (the matrix is clean either way, and a `pub mod` warns about nothing), which is exactly why it survived: the asymmetry is invisible to every check that exists, and only shows up reading the two families side by side. *(fixed 2026-08-12, in its own commit)*. One `#[cfg]` line in `host/graphics/mod.rs`, and the floor build reported nothing: every caller of the model was already gated, which is the confirmation that only the module declaration had been missed. The seventeen configurations stay clean and a build without the family now drops the whole 1570 lines instead of compiling them for nobody.

- ✅ **One signal, three inkings: the trace renderers agree on the columns and on nothing else** *(found 2026-08-11, fixed the same day, reading the crate's drawing paths after an eye pass noticed a waveform filled at one zoom and outlined at another)*. E6 says the three min/max implementations collapsed into `signal::trace`, and they did — **the stored ones**. The live pair was never folded in, so a signal against time is drawn by three renderers today: the GPU one (`waveform.rs`'s `Mode::{Columns, Line}`), the mesh one (`signal::trace::draw_channel`, the plot and a clip's take) and `meters::trace_lane` (the audio-rate oscilloscope), with `meters::draw_scope` a fourth polyline for a control bus's history. They share the *column source* (`Trace`/`WaveformData::column`) and nothing about how a column is inked.

  **What the reader sees, and it is four disagreements rather than one.** *(1)* Only the GPU path extends a column to **zero** (`lo.min(0.0)`/`hi.max(0.0)`): for bipolar audio the two agree by accident, and anything offset from zero — an envelope, an automation, a unipolar take, a DC-bearing buffer — is a body filled from the baseline in a navigable view and a floating band in the clip of the same samples. *(2)* Only the GPU path ignores `min`/`max`: `amp_to_clip` is pinned to ±1 through `AMP_MARGIN`, and the slot is handed a `WaveformData` and never the element's value bounds, so a navigable view of data outside ±1 leaves clip space while the mesh paths map it. *(3)* Only the mesh path guarantees a **minimum ink** (`max(cw, style.width)`, with a test), which is the fix a flat stretch of an envelope already needed once — the GPU path and `trace_lane` can still collapse one to sub-pixel. *(4)* The optical weight is three numbers: a hardware `LineStrip`, `trace_w`, and `divider_w` for `trace_lane`'s columns. On top of it `LINE_THRESHOLD` is written twice (`waveform.rs`, `signal/trace.rs`) with a comment asking the two not to drift, and `trace_lane` decides the regime by a third rule entirely (`frames > columns * 2`).

  **The fill itself is not the bug** and should be said plainly, since that is what an eye pass reports first: a filled band above the threshold and a polyline below it is the three-regime design, and all three renderers do it. What is a defect is that they disagree about *what* they fill.

  Two decisions to make before touching anything, because either one alone would leave the divergence. **Does a column extend to zero?** It is a property of the *element* — what the signal means — not of the destination that happens to draw it, so whichever way it goes it goes for all three. And **does the GPU path take the value bounds?** It is the one that would move a public surface (the slot is filled with a `WaveformData`; the bounds would ride with it, the way the texture slot's analysis parameters already do). Then `trace_lane` and `draw_scope` fold into `draw_channel`, which is the copy E6 left outside and the only one with a regime rule of its own, and the two `LINE_THRESHOLD`s become one.

  **What shipped, and the first decision was answered by an eye pass rather than by the argument above.** It landed as written — one shared fill rule, "a column reaches the baseline when the domain straddles zero" — and a screenshot at a zoom where cycles are legible killed it on sight: there a column covers two or three samples that all sit near +0.6, and filling to zero **inks a band the signal was never in**. The question that survives is *at which zoom a fill belongs*, and it has no answer, which the **subsonic** case proves outright: a 1 Hz curve has far more samples than the screen has pixels at any zoom showing a cycle, so every "fill once the samples stop fitting" rule fills it, and a cycle a second is exactly the curve a fill destroys. So the rule is that there is no rule — **a column is drawn as it is measured, everywhere** — and the solid body of a zoomed-out waveform is the data filling it (audio crosses zero many times per column), not something the drawing adds. The mesh renderers were right all along; the pipeline's clamp was the anomaly, and closing the divergence the other way would have propagated it. `docs/decisions.md` carries both this and the domain decision.

  The other three closed as planned. **The domain** (`min`/`max`, `[-1, 1]` by default) reaches the pipeline through `SlotFrame::Waveform`, and the **ruler and the cursor readout follow the same pair** — where an element names a domain of its own the strip rules a plain value axis, since `db`/`bits`/`percent` are units of full scale; `Vertical` replaced the `Option<(f64, FreqScale, f64)>` whose `None` silently meant "a waveform, in amplitude". **The minimum ink** is now the pipeline's too (`lane_h_px` reaches `upload_geometry` for it). And **`meters::trace_lane` and `draw_scope` fold into `draw_channel`**, so the live views summarize instead of drawing a segment per sample — the copy E6 left outside is gone, with its third regime rule, and `LINE_THRESHOLD` is one `pub const` the mesh path re-exports.

  **One capability fell out of the same pass, and it is an affordance more than a drawing.** Zoomed past the polyline threshold, each sample is now **marked with a dot** once consecutive samples stand three `point_radius` apart (`trace::dots_fit`, the collision rule the ruler's labels already use): the line between samples is an interpolation the drawing invents, and the dot says which points of it are data. It is sized as a **curve's break-point** on purpose — that is what sample-level editing will grab, so the two read as the same kind of target the day the second becomes draggable, and it connects to the open "the editing gestures want affordances" entry below. Both renderers draw them; on the GPU they are their own vertex range through the column pipeline, since a `LineStrip` cannot draw a quad.

  Ported in the same change: `docs/gui-protocol.md`, both `waveform` builders (which grew `min`/`max`, until now reachable only through the escape hatch), and `docs/decisions.md`. Green on 686 tests, the 17-configuration matrix, `pyright` at zero and the web suite; checked by eye on `gui_rulers`.

- ✅ **The crate did not build without its default feature** *(found 2026-08-11 running the def-family matrix while closing K12; fixed the same day, in its own commit)*. `cargo build --no-default-features` failed with 19 errors, and had for some time: three `use` lines in `host/gui/app.rs` carried a `#[cfg(feature = "midi")]` that belongs to none of them — the live-tick path (`live::retention_window`, `tree_animates`, `tree_has_live_widget`), the key vocabulary (`element::Key`) and the live element context (`element::Live`) are what a *window* reads every frame, MIDI or no MIDI. The attribute was almost certainly added to silence an unused-import warning in a no-MIDI build; only `timeline::group_key` was genuinely MIDI-only, and it keeps its cfg. Nothing but the imports moved.

  What made it survive is what K13 is about: **nothing ever built this crate any other way.** CI lints the default set, `check-wasm.sh` builds the default set for another target, and no habit reaches a configuration where a feature is *off*. The root crate learned this with its def families and answered it with a matrix; this crate had a feature matrix and no matrix runner, which is precisely the gap `crates/clausters-gui/check-features.sh` closes — and the fix is a good measure of the cost of not having one, since the break is four lines and had been unbuildable indefinitely.

- ✅ **A `standalone` build could not load a single persisted SynthDef** *(found 2026-08-11 running `gui_standalone`'s own launch line while closing K11; fixed the same day, in its own commit)*. The bundle booted, the window opened and the knob turned — and nothing sounded, with three warnings the example never mentions: `persisted SynthDefs found but this build lacks the 'synth' feature`, then the boot's `/synth_new: SynthDef not found`, then one `/node_set: node 1000 not found` per turn of the knob. The cause is one line of `Cargo.toml`: the server is linked `default-features = false` (so a standalone build does not also pull MIDI and PipeWire, which is right) and `standalone` never added a def family back, so **the feature the example documents could not run the example**. `synth` is in the feature now; `faust` stays the separate `standalone-faust` because it needs libfaust at build time, and `synth` needs nothing. What made it survive this long is exactly what `CLAUDE.md` says about the examples: nothing runs them, and this one's real test is a `cargo run` line printed for a human to paste.

- ✅ **Ctrl+Backspace was missing from the field's editing alphabet** *(found 2026-08-11 while reading K10's text pass; fixed the same day)*. The `text` field walked words with Ctrl+arrow and selected them with Shift, but **deleted only one character at a time** — so removing a typed word meant holding Backspace, in a field whose whole point is that a typed string is delivered live. It is the plainest gap a keyboard has: every editor on either platform has it, and it is the one an unfamiliar user reaches for first.

  The unit is **one run per press**, and that was the second reading rather than the first: `"a, b"` loses `b`, then `", "`, then `a` — where the *motion* crosses separators and the word behind them in a single step. They differ on purpose. A caret that overshoots costs a keystroke to come back; a delete that overshoots costs text, so the destructive one takes the smaller step, which is what every editor a user arrives from does.

  Both are built on the same scan (`run_start_left`/`run_end_right`, one run; `word_start_left`/`word_end_right`, the same scan applied twice when the first run is separators), so the two can never drift into disagreeing about where a word begins. Ctrl+Delete lands with Ctrl+Backspace because it is the same arm mirrored, and a selection still wins over the run in both, the rule the plain Backspace/Delete already follow. No wire change and no front change: the modifiers already ride beside the key.

- ✅ **A by-eye example must not time out** *(found 2026-08-11 while closing K9; fixed the same day)*. `text.py` script-run replayed its timed changes and then **closed itself at thirty seconds**, which is exactly long enough to miss what you opened it for: the example is the manual test surface, and a window that ends on a clock ends the test before the reader has finished reading. Script-run it now holds until the window is closed (the `patch1.py` shape); the `seconds` bound stays for a cell run, where a notebook wants the loop to give the prompt back. Worth stating as a rule rather than a fix: **an example that exists to be looked at ends when the person looking at it says so.**

- ✅ **A query answers what a widget is, not what it was defined as** *(found 2026-08-10 while closing K5; fixed the same day)*. Two surfaces say what a widget holds and a gesture writes only one of them: the **registry** keeps the document (what the script sent, kept current by every `/gui_set`), the **render tree** keeps the widget as the user has since left it — so `/gui_query` reported a dragged control's def-time value forever, a moved clip's original placement, an edited curve's original points. It is the one answer a script cannot get any other way, since the `/gui_event` that announced the edit is a stream you had to be listening to.

  `Widget::info()` is the overlay, and it is what finally gives `Element::info` the consumer it has lacked since K1: an element answers for itself, a built-in is an arm in the query pass whose row disappears as the leaf ports (the shape `needs()` already has), and a **clip answers for its bodies too**, since a body carries no id and the routing is the one a `/gui_set` of a body prop already takes. Two rules keep it from becoming a second source of truth: only what a **gesture** can change belongs there, named by the prop that sets it; and a non-scalar rides as the JSON string its own `/gui_set` accepts (`points`, `notes`, `osc`), so **what a query gives back is what a set would take** — `pianoroll::notes_json`/`osc_json` and `ScrollView::info` were the missing inverses.

- ✅ **A held drag that reports nothing did not repaint** *(found 2026-08-10 while porting `score` in K7; fixed the same day, and it is a K6 regression)*. The machine repainted an element's drag **only when it reported** — the redraw rode along with the delivery — which is right for every control (a value moved, so it is both drawn and sent) and wrong for the two drags whose whole effect is a picture: extending a `text` selection, and a `score`'s element crossing a diatonic step. K6 introduced it by retiring `Drag::TextSelect`, whose arm pushed the repaint by hand; the port would have shipped a score whose displacement only appeared on windows that animate for some other reason. The fix states the rule where the phases already agree on it — **a held drag repaints, whether or not it reported** — and does it in the machine rather than asking every element to remember, which is the same reason `Claim::Take` redraws on the press.

- ✅ **An example wrote captions longer than the boxes it put them in** *(found 2026-08-10, by eye on `analyzer.py` while checking the tick seam; fixed the same day)*. The spectrum's label asked for 480 px of the ~374 its half of the row had, so it came back cut. The **host is right**: a single line that does not fit is truncated with an ellipsis rather than bleeding into the neighbour, which is the rule the light widgets have always followed. What was wrong is the example — a caption written for a full-width view and then placed in a half-width one — and the fix is the labels plus the comment saying why, since the next reader will otherwise read the ellipsis as a bug in the widget. Worth one line here because it is the second time an example's *text* has been the finding of an eye pass, and the measurement is cheap: at `text_scale` 2.0 a character is 12 physical pixels wide, so a caption's budget is the box's width divided by twelve.

- ✅ **The goniometer's figure jumped, and the signal was the staircase** *(found 2026-08-10, by eye on `analyzer.py` while checking the tick seam; confirmed against the pre-tick build — it predates the seam — and fixed the same day)*. The image snapped rather than swept while the script moved it, and the first guess was the drawing: the phase view has no trigger, so its window holds a non-integer number of periods and the trace's ends need not meet. The cause was upstream and simpler. A control is applied **in steps at block boundaries**, and the sweep wrote `wm`/`ws`/`freq` about thirty times a second — so the stereo weights moved in a staircase, and a stepped `freq` breaks the sine's phase, which a goniometer shows as the figure jumping. The example lags the three controls (`control(..., lag=0.05)`, the server's own smoother), so what the views analyze is the continuous signal the script means rather than the staircase the messages arrive as. What the phase view *itself* still has no answer for is stated in "Future directions" below, since it is a design and not this fix: a window with no alignment.

  Worth keeping because of the reflex it corrects: **a live view is a measurement, so an unsteady picture is a claim about the signal before it is a complaint about the drawing.** The A/B against the previous host is what settled it, and the same question is worth asking of the next one.

- ✅ **Persistence saves the document, not what the user did to it** *(found 2026-08-10, reading the query path while fixing the entry above)*. `def_json` is the bytes the `/gui_def` carried, and neither a `/gui_set` nor a gesture rewrites it — so a named def reloads exactly as it was sent, and a standalone bundle a user has spent an afternoon arranging reopens on the arrangement the script wrote. It is the sibling of the query gap and it is **not** the same fix: making a query honest is a read, and this is a decision about what a persisted def *is*. Two defensible answers, and the entry exists so the choice is made rather than defaulted into — a def is a **document** (it reloads as authored; a session is a separate thing the host does not have) or a def is a **state** (it reloads as left, which is what a user of a standalone bundle will expect and what makes the bundle an instrument rather than a demo). If the second wins, the mechanism is already there: `Widget::info()` says what diverged, so persisting is overlaying it on the document before the store writes — the same overlay the query does, at a different moment.

  **Answered 2026-08-21 by the user, and the entry was asking the wrong
  question.** The two answers were not a choice: "a def is a state" is not an
  alternative to "a def is a document", it is a **confusion of domains**. A
  GuiDef defines an *interface* — it is the same interface every time it is
  instantiated and it holds no data. What a session of work produced is the
  **session's**, and this project already has that thing: the document plus its
  source table (`form.to_session`, `session::FORMAT`), owned by
  `clausters-document`, which lists "the session document and its provenance"
  among what it owns. In any desktop application a widget is not what saves the
  configuration; the file being worked on is. Handing persistent configuration
  to a GuiDef puts the state back inside the view, which is the same mistake
  `clausters-document` exists to undo — the entry above it, "a query answers what
  a widget *is*", is a **read** and stays right for exactly that reason.

  **So nothing changes here, and there was never a defect in the work path.**
  A dragged clip is not lost: `Editor.apply` turns the gesture into a change on
  the arrangement and `to_session` saves *that*, so the composition round-trips
  through the document and the def is not on the route. `def_json` persisting
  the bytes the script sent is correct and stays.

  **What is left is not this plan's question and is reformulated where it
  belongs** *(2026-08-21)*: a standalone bundle is a combination of GuiDefs with
  **no application document behind it**, so a control a user left somewhere has
  no owner to be written to — and that absence is a question about what the
  second kind of document *is*, not about what a def stores. It is open and
  undefined, and it is written where documents live:
  `crates/clausters-document/PLAN.md`, Open decisions, "What is the second
  document: the application, and not the arrangement?". Nothing here waits on
  it, and nothing here changes when it is answered — that is what makes the
  half settled above worth having settled on its own.

- ✅ **The editing gestures want affordances** *(found 2026-08-10, by eye on `bpf.py` and `multitrack.py` after the K5 port)*. Two gestures that work correctly and that nothing on screen announces. They are one entry because they are one decision — what a *grabbable part* looks like in this host — and answering either alone would give the two a different vocabulary.

  - **Bending a segment of a curve.** Press the drawn line between two break-points and drag vertically. The curve is one trace of one weight end to end, so nothing distinguishes "a place you can bend" from "a place Ctrl adds a point to" or from the container underneath. Three shapes to choose between: a **hover state** (the segment under the pointer lit, the way a handle lights), a **midpoint handle** (a small mark at the segment's middle that is what you actually grab, which makes the target explicit and leaves plain-drag-anywhere free), or **leaving the gesture and teaching it** in the widget's documentation. The midpoint handle is the one that also answers what the current gesture cannot: on a clip body the segment drag is deliberately *off* (a body claims its points and leaves the rest to the clip — K5), so an automation clip cannot bend anything at all, and a handle is a point-like target a body could legitimately claim.
  - **Resizing a clip.** `interact::coords::clip_part` gives each end of a clip a six-pixel zone: a press there drags `dur` instead of `offset`. `track::draw_clip` paints a fill and a border, so the zone is invisible and a user finds it by accident — and below twelve pixels wide the clip is all body and there is no resize at all, which is also unannounced. What the ruler and the lane fader already do is the precedent to reuse rather than invent against: a *drawn* grip, a metrics role wide, at each end. **Decided 2026-08-12, from the multitrack pass:** a grip **at each side of the clip, revealed on hover** — so a lane of clips is not a row of permanent handles, and the affordance appears where the pointer already is. Two things it inherits from the entry above: it is drawn where the clip's **end** is (never at the pixel a clamped rectangle landed on, which is what read as a grip before there was one), and it is absent when that end is off screen — the element answering for its own visible space.

  **One real bug to fix with whichever shape wins**, found while reading this: `CLIP_EDGE_PX` is a literal `6.0` in **device** pixels, which is exactly what L6 forbids (no hit site names a number; `hit_slop` is the role) and what L8 makes visible — on a 2x display the grab zone is half the physical width it means, so a clip is hardest to resize on the screen where its edge is thinnest. Drawing the grip forces the number into the metrics table on the way, which is the right order: the affordance and the role are the same change.

    **What shipped (2026-08-12).** The grip is drawn, and the two rules it inherited from the entry above are what make it more than a rectangle: it stands at each end **of the clip**, never at the pixel the window's clamp landed on, and it is **absent when that end is off screen** — `track::clip_ends_on_screen` reads the clip's own axis (the slice of `[0, dur]` its rectangle shows) and answers for each side. It appears **on hover**, so a lane of clips is material rather than a row of handles, and it goes into the overlay mesh with the name (a take's trace would bury it). `track::clip_grips` is the one geometry the renderer and the hit-test both read, so the strip that lights up is the strip that resizes — including the narrow case, where a clip too small for two grips is all body in both. And `CLIP_EDGE_PX` is gone: the width is the **`grip_w` role**, so it is logical and resolved per window, where the literal was device pixels and halved the grab zone on a HiDPI screen. What is still open in this bullet is the **curve's** affordance — a segment you can bend still looks like a line.

    **A second pass the same day, by eye, and four of its five points were the look rather than the rule.** The grip is a **plate** now — the translucent rounded ground the captions sit on, with an **arrow** centred on its height — because an opaque strip cuts a hole in the take instead of marking an edge of it; it is `grip_w` **12**, since 6 was a target you aim at rather than one you land on; and **one** is drawn, the one on the side the pointer is on, because both ends lit is a claim about where the pointer is going that the pointer has not made. The fourth is a z-order rule and the only defect among them: clips overlap, the overlay is painted after every clip's box, so a **covered** clip lighting its edge drew its affordance *over* the clip covering it — announcing a grab the press, which takes the topmost, would never give. Only the topmost clip under the pointer carries a grip now, the rule the hit-test already followed.

    The fifth is not a defect and is worth writing down as the answer it is: **a grip does not appear when the zoom has pushed the clip's end off screen**, which reads as the gesture breaking on zoom-in. It is the rule working — there is no edge on screen to grab, and the alternative (resize from whatever pixel the window cut) is precisely the lie the grip exists to stop telling. What a zoomed-in editor owes the reader instead is a way to *reach* the end: scroll to it, or an edge-scroll while dragging, which is a separate gesture and not this affordance's job.

    **A third pass, and the one rule the first two were missing: an affordance belongs to what is *held*, not to where the pointer is.** Mid-drag the two part company, because a clip moves in `snap` steps and the pointer does not — so the held clip's grip blinked out between steps, and whatever the pointer drifted over lit up instead, including the clip being dragged *across* (whose grab was not on the table at all). The frame now takes a `Grab` beside the world: nothing held (the pointer's rules), something else held (no clip lights up), or *this* clip held by *this* side. Both fronts fill it from the same gesture machine, so the page draws what the desktop draws.

    Found with it, and **not** a grip defect: dragging a clip felt broken under zoom because the lane's `snap` is a fixed length in samples, so a zoomed-in drag moves the pointer far and the clip not at all. The example sets `snap=0` now (a clip moves by whole samples, which is what a pass over the *drawing* wants); a piece being assembled wants its grid back. What the multitrack still owes is the DAW answer — **a grid that subdivides with the zoom** rather than a constant the reader has to change by hand.

    **And what the arrow will have to say.** An edge drag means **trim** on one arrangement and **stretch** on another, and neither is implemented for audio — the current behaviour is a resize of the clip's span with the material unchanged. The symbol is where that difference becomes visible, so it is a parameter of the *gesture*: `draw_clip_grip` takes the side today and takes the meaning the day a lane declares one.
  **Closed 2026-08-17 with the second half.** The vocabulary was chosen by the clip grip and did not need choosing again: an affordance belongs to what is **held**, it is drawn where the thing is rather than where the pointer got to, and it is absent when there is nothing to grab. Applied to the curve, that is one lit segment — the accent at a handle's weight, over the part a vertical drag would bend — taken from the **grab** when a bend is in flight and from the pointer only when nothing is held, and only where the curve is editable at all.

  **The one defect left standing is closed** *(2026-08-18)*, and not by the guard it was waiting for. The segment is lit exactly when the curve is the **active edit layer**, where the bend *is* the gesture — so inside a clip it lights when the curve is in hand and not otherwise, and the press there bends rather than moving the clip. The condition that was written and reverted three times was a precedence between claimants; what it needed was the layer rule above, and then the affordance and the press agree without either of them knowing about the other.

- ✅ **A widget's label strip is written four times** *(found 2026-08-21 doing
  the plate pass above, which had to read all four to decide none of them wanted
  a plate)*. `graphics::meters::label_strip` is the helper — "draws the label
  strip above a view body, if it has a label" — and three views do not call it:
  `signal/plot.rs`, `signal/spectrum.rs` and `signal/phasescope.rs` each open
  with the same `if let Some(text) = label { font::text(mesh, text, rect.x +
  m.pad, rect.y + m.pad, m.text_scale, theme.text) }`, byte for byte. Nothing is
  wrong on screen: all four agree today, and they agree because they were copied
  rather than because anything holds them together — which is the whole of the
  entry. The strip's *height* is already shared (`controls::body_rect` takes it
  off the body), so the drawing is the one half that was left loose, and the fix
  is to make `label_strip` `pub(crate)` and call it. Not folded into the plate
  pass because a cleanup riding in a legibility fix is how a diff stops being
  readable.

  **Done as written** *(2026-08-26)*, and it shrank rather than moved:
  `meters::label_strip` is `pub(crate)`, the three views call it as their first
  line, and each lost the four lines — `spectrum` its `font` import with them.
  The call goes *before* `d.parts()`, which is what the two views that already
  called it do, so the three now read like the fourth. The helper's doc comment
  carries the reason it exists at all: the strip's height is
  `controls::body_rect`'s and its drawing is this, and the two are one fact
  stated in two places.

  **Closed on re-reading (2026-09-14)**: the note above says it was done; only
  the box was left open.

- ✅ **A control has a range and no curve, and no step** *(noted 2026-08-23,
  with the Python client's control-widget work)*. `props::Range` is
  `{value, min, max, label, text_size}`: a knob, a slider and a number are
  **linear over min..max**, and that is the whole of it. Two things the clients
  can now say and the host cannot draw:

  - **A step.** A widget can be given one (and a Faust parameter arrives with
    one, from its `hslider`), it reaches the host as a `step` prop, and nothing
    reads it — the drag is continuous whatever it says. A quantized dial is the
    obvious use, and a `number` over integers the commonest.
  - **A curve.** There is none at all, which is what makes a *named spec*
    (sclang's `\freq`: 20..20000 **exponential**) something the client
    deliberately did not ship — a `spec="freq"` that silently drew linear
    would be worse than not having it. A warp belongs here, in the shared
    geometry the display already has for frequency axes
    (`display_to_hz`/`freq_scale`), not as a mapping each client re-derives:
    two implementations of one curve is how the same control comes to feel
    different in two places.

  Both are one prop each on `Range` plus their use in the drag math. Do them
  together: a spec is the pair, and shipping the curve without the step leaves
  `\midinote` (integers over a linear range) still wrong.

  **Closed 2026-08-24, both at once.** `props::Range` carries `curve` and
  `step`, parsed with the rest and live over `/gui_set`, and both drags read
  them: `fraction`/`set_fraction` are `warp::curve_unit`/`warp::curve_value`
  now, so the bend is the core's one function and not a second copy — a `curve`
  of 0 falls through to the linear map *inside* the core, which is why an
  unbent control computes exactly what it always did. The value a drag produces
  then lands on the step's grid, counted from `min` and clamped as a step
  **count** rather than as a value, so a grid that does not divide the range
  (`0..10` by `3`) stops on the last whole step instead of on an off-grid end,
  and a reversed range steps from its own `min` downward. The step is a rule
  about the **hand**: a value the script sends is drawn as sent, because a
  control shows what it was told. Both clients grew the two keywords on
  `knob`/`slider`/`number` — a Faust parameter's own step already arrived
  through the control it was built from — and `docs/gui-protocol.md` states the
  pair. The named spec is still deliberately absent, and it is now client-side
  sugar over these two rather than a capability nothing has.

  **It turned up a defect of the same math on the way.** `fraction` read the
  value off a *normalized* axis while `set_fraction` wrote it against the
  declared ends, so a **reversed** range (`min > max`, a legitimate control)
  drew its handle mirrored: the drag put the value where the hand was and the
  picture showed the other end. Reading both directions out of one primitive
  is what fixes it, and `Range::axis` — which had no other caller — is gone.

- ✅ **The gesture machine knows no click.** *(found 2026-08-24, answering
  "what kind of thing is a button?"; the click closed the same day, the other
  two are named below)*. Press and release were the only primitives the machine
  reported, and every composition over them was absent: a **click** (a press and
  a release that landed inside the widget — cancellable by sliding off before
  letting go, which is every desktop convention for a command button), a
  **double click**, a **long press**. Nothing under `host/gestures/` mentioned
  any of them.

  **The click landed, and it landed in the machine.** `Element::release` grew an
  `inside` argument, answered by `gestures::element::inside` — the same declared
  shape and hit slop `press` is filtered through, asked a second time where the
  button came up, and written beside `press` for exactly that reason. An element
  that re-implemented "landed inside" would be a second answer to a question the
  machine owns; the ten implementors that have nothing composed over their
  release ignore the argument.

  What an element *sees* was the open question, and the answer is: the fact, not
  a composed event. The machine says whether the pointer was still there and the
  element says what that means, which is what keeps a click from being sprayed
  by every knob whose drag happened to end inside its groove. `button` reports
  it as an interface event (`Events::and_interface`), on a road no binding
  touches — the client half is `clients/python/PLAN.md`, Found by use.

  The button's `mode` deliberately did **not** grow a click: it says which
  primitive reaches the *server*, and a click is an interface gesture composed
  from both. That separation is what let the two halves ship a day apart without
  either one reaching into the other.

- ✅ **The double click and the long press, and whichever gestures a pass over
  the usual ones turns up** *(named 2026-08-24, when the click closed; the user
  asked for the review rather than for the two)*. Neither exists, and unlike the
  click neither is a hit test: a double click is a press-time **window** the
  machine would have to keep, and a long press a **timer** it would have to run,
  so both grow state the machine does not have today and a threshold somebody
  has to choose. Nothing is waiting on them — no widget wants one — which is why
  this is a review of what a GUI's gesture vocabulary should be rather than two
  items to implement. Worth doing before a widget invents one for itself.
  *(2026-10-04: the long press now has something waiting on it — `G37.2` takes
  it, with the secondary button, as the request that opens a context menu, and
  `G38.2` reuses its timer for a tip. The review of the rest stands.)*
  *(Closed 2026-10-04: both exist, and the pass over the vocabulary is done.
  The machine counts a run of presses from the front's clock (`count_press`,
  400 ms and 6 px apart, handed to an element as `Input::clicks`): a double
  click opens a box of notes and activates a table row, a double and a triple
  click select a word and a line in a text field. The long press is `G37.2`'s
  timer, a held finger asking for the context menu, and `G38.2`'s rest timer
  shows a tip. With them the vocabulary is the usual one -- click, double and
  triple click, drag, Shift/Ctrl/Alt with either, the secondary button, the
  middle button pasting the primary selection, the wheel and the long press --
  and no widget invents its own.)*

- ✅ **A paste was refused everywhere, and the reason blamed samples for it**
  *(found 2026-09-02, reading the three verbs against each other after a
  question about where paste sits in the edit stack; fixed the same day)*. `G24h`
  gave the roll its own trio inside the element (`elements/notes.rs`): all three
  keys handled there, cut and paste emitting `notes_event()`, so a block pasted
  in a roll was already one ordinary `setmembers` and one undo. `D4` then gave
  the *window* a second trio (`gestures/keys.rs::clipboard_key`) against the
  typed clipboard: copy is the host's read, cut and paste leave as intents. The
  two never met at the far end — `Editor._apply_paste` refused **every** paste
  it saw, with a sentence about samples, so a note block copied in a roll and
  pasted over a clip on a lane was answered by a reason that described a
  different payload.

  **The fix is that a paste is the same edit its own copy was.** A `text` block
  holding the flat quintuple array is read back and handed to `_apply_notes` /
  `applyNotes`, the very call a drag on a note goes through: one `setmembers`,
  one entry on the pile, one undo for the block. It also settles an offset the
  two sides measured differently — the position the host sweeps is on the
  timeline's axis and a roll's notes are in its clip's own time — and the
  refusals now name what is missing (a view with no roll to write onto) instead
  of naming samples for everything. The note block stays in the `text` kind
  deliberately: a string is the one thing a *system* clipboard carries on every
  platform, so the block stays pasteable the day the host's own clipboard is
  bridged to the desktop's or the browser's.

  What remains is `Clipboard::elements`, still a constructor nobody calls —
  and it is not an oversight but a missing door, written up under "Future
  directions" above.

- ✅ **A sweep on a roll's grid left a time band behind it** *(found 2026-09-04
  by the user, running `clients/python/examples/editors/edit_notes.py`)*. A selection of notes is a selection of the **rectangles
  the notes are** — the patcher's marquee over its boxes, the multitrack's over
  its clips, one gesture — and a *time range* over the same grid is the **other**
  selection, the one the entry above already holds open for a lane. The roll
  plans `select`, which does both: it sweeps the notes *and* writes the shared
  span, so every drag leaves a band nobody asked for.

  **Its own doc comment already says `marquee`** ("a note first, then the
  container's marquee over the empty grid -- the same plan a lane carries"), and
  the code under it says `Select`. What keeps that code from being a simple
  typo is a gap one step down: the machine's `Marquee` arm asks only the
  **lanes** what the rectangle caught, never the element it was begun on, so a
  bare roll planning `marquee` would select nothing at all — `select` is the
  only step that reaches a note, span and all.

  **Both halves, since neither is right alone.** `Marquee` now passes the
  element under the press to `marquee_caught` — which is what its two parameters
  and `Drag::Marquee`'s own `at` field were always for, the patcher's
  element-claim path filling that field already — and the roll plans
  `[Element, Marquee]`, a lane's plan. The time range stays reachable by name
  (`gestures={"drag": "select"}`) in all three views. The one test that asserted
  the span is two: the grid's sweep leaves `sel_len == 0` and catches the note,
  and a roll asked for the range by name still sweeps it.

  **It is the same shape as the entry above** — a plain drag that should catch
  objects and a range nothing reaches by hand — and the half that is still open
  is that one: reaching a lane's time range by hand. What this settled is the
  roll's default and the machine underneath both.

- ✅ **A clip drag released outside its lane keeps the new position and emits
  nothing** *(found 2026-09-07 by the user, by eye, in `editors/multitrack`)*.
  Drag a clip along its lane and let the button go with the pointer over the
  ruler, over the transport row or outside the window: the clip stays drawn
  **where the hand left it**, and no `"clip"` edit-back reaches the client. The
  driver keeps the position it had, so the piece sounds where the clip no longer
  is — the same clip, in two places, one on screen and one in the arrangement.

  It is the branch's own subject seen from the host's side. A `/gui_def` means
  *make it look like this* and the client is the picture's single owner, which
  holds only as long as **every** change the host makes to the picture is
  announced. A drag that ends off the lane is a change the host made and did not
  announce, so the client's copy is stale in the one way the reconcile cannot
  repair: nothing will redraw until the client sends a def, and the def it would
  send is the old position.

  Two ends are worth reading together, and neither has been: whether the drag's
  release is bound to the widget that took the press or to whatever is under the
  pointer, and whether the release path that runs off-target reaches the code
  that emits the edit at all. Either the drag should **cancel** and spring back
  — a legal answer, and then nothing is announced because nothing changed — or
  it should commit and say so. What it does today is neither.

  **Measured 2026-09-07, and it does not happen** *(the status bar's first real
  use: the window says what it emitted, so this took four gestures and no
  instrumentation)*. Every case reports, and reports the right verb — released
  inside the lane, `"clip"`; onto another lane, `"lane"`; changed lane and let
  go over the ruler, `"lane"`; unchanged and let go over the ruler, `"clip"`.
  A test now pins the three off-lane releases. Nothing about the release path
  was wrong.

  **What was actually seen is the entry below**, and the difference matters
  because the diagnosis here was *inferred from a symptom* and the inference
  went somewhere the code never did. The clip did move on its own, and it moved
  because the pointer crossed the window manager's chrome — not because it was
  released anywhere in particular. "No edit-back reaches the client" was never
  observed; it was deduced from "the driver kept the old position", which had
  another cause entirely. Kept as the record of that: **a symptom names where to
  look, not what to look at**, and an entry that reasons past the last thing it
  measured will send the next reader down the branch it invented.

- ✅ **A pointer that left the window was written down as a place, and a held
  clip jumped to the start of the timeline** *(found 2026-09-07 by the user, by
  eye, `editors/multitrack`; fixed the same day)*. Drag a clip and cross the
  **window manager's own chrome** — a resize border, the title bar — and the
  clip jumps to the beginning of the axis, at the exact pixel the pointer
  leaves the content. The user had seen the same thing on a knob and said what
  it was: *"the wrong coordinates over the window's chrome had already happened
  with the knobs. This is a problem that has to be solved in a general way."*

  The desktop front kept the pointer as a pair and wrote `(-1.0, -1.0)` into it
  on `CursorLeft` — a **sentinel for *not here*, stored in the field that means
  *where***. The edge auto-scroll reads that field: `-1` is left of every lane's
  body, so it pulled the view leftwards at full tilt every frame and carried the
  held clip to the start with it. Everything downstream was working correctly on
  a coordinate no hand could have been at.

  Two changes, both general rather than a platform's:

  - **The type stopped admitting it.** The cursor is `Option<(f64, f64)>` in
    *both* fronts now: absent and *at minus one* are different facts and only
    one of them is a coordinate. The compiler named all eleven readers, which is
    the whole reason to fix it in the type rather than at the site that
    happened to be caught.
  - **A drag owns the pointer until the button comes up.** A press captures it,
    so crossing onto the chrome is the platform saying it left the *content*,
    not that it stopped belonging to this gesture — motion keeps arriving and
    the drag keeps following. `CursorLeft` no longer forgets the position while
    a drag is in flight.

  The page had the same shape of lie without the sentinel: a canvas nobody had
  pointed at reported a cursor at its own top-left corner, which the readout
  drew as a hand that was never there. Same `Option`, same invariant, one rule.

  **A first attempt is worth recording because the tests killed it.** Clamping
  a drag's coordinates into the window, in the shared machine, looked like the
  general answer and turned two tests red: a plane's pan *must* keep going when
  the hand runs past the edge, and so must a block of clips. The coordinate
  being outside was never the problem — the front **inventing** one was. The
  clamp would have fixed the symptom by breaking the feature underneath it.

  **What is not covered, and should be said**: nothing tests either front. The
  fix is the type (compiler-enforced) plus one behaviour in winit-driven code
  no harness reaches, so the check was a hand on the window. The machine-level
  test beside it pins the other half — that a coordinate left of the body
  *does* pull the view, which is correct and is exactly what made the sentinel
  fatal rather than merely wrong.

- ✅ **A click on a clip reports an edit that nothing edited**
  *(found 2026-09-08, giving the click its cursor: the release arm beside it
  emits the placement whatever the drag did)*. `Gestures::release`'s clip arm
  ends in `emit_clip`/`emit_clips` unconditionally, so a press and a release
  with nothing in between sends the clip's own offset and duration back to the
  owner. The owner applies it and the document takes an entry: **a click on a
  clip is an undo step**, and a hand that clicks four clips to look at them has
  to undo four times to get past them.

  It predates the cursor work and is untouched by it -- the click now also
  locates, which is right, and the intent beside it is still noise. The fix is
  to say what the arm means: one gesture is one edit *when the gesture changed
  something*, so the release compares the placement against the press-time
  `orig` it already carries and emits nothing when they agree. The lane-change
  branch above it is already the honest shape (it reports because something
  moved), and a trim that came back to its own edge is the same case.

  **Fixed the same day**, and that is what it does: the arm reads the clip's
  placement back and returns before the emit when it equals the `orig` the drag
  was carrying, unless the clip changed lane (which is a move whatever the
  offset says). The block needs no separate answer -- it moves rigidly with the
  grabbed clip, so the grabbed one standing still is the whole block standing
  still. Three cases in one test: the click reports no edit and still locates, a
  real drag still reports, and a drag that came back to where it began reports
  nothing either.

- ✅ **Step entry keeps a position of its own, and a click does not move it**
  *(found 2026-09-08, closing "One cursor" -- the paste and the cut read the
  window's cursor now, and this is the one anchor that still does not)*.
  `Notes::step` is where live MIDI writes when the transport is stopped, and it
  advances one grid per chord. Nothing else moves it: locating the window puts
  the cursor somewhere and step entry goes on writing where it left off, which
  is the same "a position stored beside the one cursor" the paste was fixed of.

  It is not the same shape as the paste, which is why it was left: a paste
  *reads* an anchor and step entry **writes** one, so making it the window's
  cursor means the element asking the host to move that cursor on each advance
  -- a request beside `Events::and_select`, and the same door a step-recording
  roll would need to scroll the view it is writing past the end of. Worth doing
  with whatever else needs that door rather than as a special case for `midi`.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

- ✅ **A `multitrack` clip draws one waveform body and nothing else**
  *(found 2026-09-08, porting `--session`'s tree onto the widget)*. A box is a
  window onto **one server buffer** (`Clip::source`), so the three bodies the
  old `clip` widget could carry no longer draw: a **roll** (an aggregate of
  clangs, which is what a track of notes is), a **spectrogram** (which wants a
  GPU slot per clip — the "N slots in one element" question again), and
  **assembled samples**, where several windows sit back to back inside one box.
  The session host lost all three when its tree became one widget: a piece of
  notes draws as named rectangles, and a joined clip draws empty rather than as
  a third of itself, which is at least honest.

  The multitrack **places**, so none of these is an editing surface — what is
  missing is the read-only picture inside the box, which is what tells a reader
  which clip is which without entering it. The roll is the cheap one (the notes
  are mesh, like the waveform); the spectrogram is the one that needs a design.

  **Re-read 2026-09-14: two of the three draw now.** A box of notes draws the
  roll's body on the box's own axis (`multitrack/draw.rs`, since the multitrack
  became an application, 2026-09-13), and a join is stitched into one take, so
  it draws as one waveform. The spectrogram is what is left, with its design.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

- ✅ **A left trim does not travel with the clip**
  *(found 2026-09-08, same port)*. The `"clips"` payload carries `start` — the
  source frame the box's own zero reads — and the document's tree vocabulary
  has nowhere to put it: `Intent::Place` states an offset and a length, so a
  trim of the **left** edge reaches the document as a move plus a resize and the
  window into the samples stays where it was. The picture is right (the widget
  holds it) and the saved session is not, which is the worst of the two.

  It is the shape `MultitrackIntent::TrimRegion` already has in the crate, and
  the honest fix is the owner speaking that vocabulary rather than a fourth
  field on `Place` — so it waits on the document's own multitrack (`O21`-`O24`)
  rather than growing a stopgap here.

  **Closed with the piece bound in the host (2026-09-08, `O24`)**. A box whose
  width changed is read as `MultitrackIntent::TrimRegion`, with its position and,
  when the left edge rewound the window, the region's content with it -- one edit
  and not a move beside a resize (`host/document/piece.rs`, its test).

- ✅ **`--session` opens a session written today as an empty window**
  *(found 2026-09-08, by eye, right after the tree became one `multitrack`)*.
  Nothing to do with the widget: a `Session` written by the current Python
  client carries only `multitrack` (the crate's own tracks/lanes/regions) and
  its general `document` is `Document::empty`, which the format allows on
  purpose — "absent means empty, not invalid", and the general tree is the leg
  being walked off. The standalone host still reads only that leg, so it draws
  a piece with no lanes and no clips and says so in one log line
  (`0 clip(s) on 0 lane(s)`) that nobody is reading while looking at a window.

  **Fixed 2026-09-08**, and it is `O24`'s first leg rather than a patch:
  `host/document/piece.rs` draws a `Multitrack` into the same widget the tree
  draws into — a row is a **track** showing the lane it plays, a box is a
  **region**, both named by their ids — and reads the edit-back in the piece's
  own vocabulary. The two descriptions are two structures in **one** history, so
  an undo walks them in the order the hand made them. `sources::plan` walks the
  piece too, which is why the takes now read in at all.

- ✅ **The space bar never worked, on any keyboard**
  *(found 2026-09-08 by the user, pressing it at a session that had just been
  given readers)*. `key_pressed` hands back what a key *produced* when no chord
  is held, and a space produces `" "` — so the window's arm, which matched only
  `Key::Named(NamedKey::Space)`, could fire only with a modifier held, which is
  to say never. The take monitor's own key had the same hole and nobody had
  pressed it: the key was declined in silence, with nothing in the log to say a
  key had arrived at all. **Fixed the same day** (`is_space` matches both
  spellings, with a test); found only because a piece with six readers gave
  space something audible to do.

- ✅ **`e` and `j` over a `multitrack` fall through to a verb written for the
  old widgets** *(found 2026-09-08, same sitting)*. Split and join reach the
  element first, which declines when nothing is selected — correct — and then
  the window's own `clip_verb`, which resolves a `WidgetKind::Clip` under the
  cursor and finds none, because a clip is not a widget any more. So the keys do
  nothing and say nothing, and with a selection they take the element's path
  instead. The window-level pair is dead code for a multitrack and goes when
  `track`/`clip` leave the tree (`G34`).

  **The half that was not dead code is fixed (2026-09-09).** Over a piece a
  split names its halves after the box they came from (`"12 2"`), which is no
  node id, so the reader dropped the tail and kept the trim that had shortened
  the original — a cut that silently truncated a region and lost the rest of
  it. The rule now is general and infers nothing: **a box the piece has no
  region for is a new region on the lane it landed on**, built from what the
  payload already carries — the buffer it is a window onto, where in that buffer
  it opens, where it sits and how long it is — and the lane is stated whole
  (`SetLane`), which is also what a removal and a paste are. So the reader never
  asks which gesture a payload was.

  What that leaves, small and written down rather than discovered: a box over a
  buffer this session never read is **not** invented (the document would name a
  source nobody can resolve, and a piece that cannot be reopened is worse than a
  box that did not stick); and a *join* of two windows that are not contiguous
  in their source becomes one window claiming the stretch between them, because
  a box is one window and `JoinRegions`' composite is not reachable from a flat
  payload.

  **Closed 2026-09-09** (`b75a4298`): `track` and `clip` left the tree, and the
  window's `clip_verb` with them. A join of windows that are not contiguous is
  stitched into a take of its own since G35.11.

- ✅ **The piece is drawn and sounded through one tempo, and it carries a map**
  *(found 2026-09-09, auditing what the standalone host reimplements)*.
  `document/piece.rs` and `document/sound.rs` both convert a region's beats to
  frames with a single ratio (`units_per_beat`), and `Multitrack::tempo` is a
  **map** with steps and ramps. So a piece with a ritardando draws its boxes and
  places its readers in the wrong place, together — which at least keeps the
  picture and the sound agreeing about being wrong.

  It is the defect this plan already recorded for the rulers ("A time axis is
  labelled by one tempo, and a piece can have several"), now on the other side
  of the seam, and it is the worse shape of a repeated rule: not a second copy
  of the relation but a **simpler** one, standing in for
  `clausters_core::tempomap::TempoMap`, which models exactly this and is already
  bound by both clients.

  Where the bridge goes was decided by a boundary rather than by taste:
  `clausters-document` depends on **serde and nothing else**, deliberately, so
  the conversion cannot live beside `Tempo`.

  **Fixed 2026-09-09.** `TempoMap::from_changes` takes a run of authored
  entries (`TempoChange`: a beat, a tempo, and whether it *ramps*) plus the
  default a piece that never said a tempo leaves to its reader, and it holds the
  three decisions that would otherwise be written per reader: a ramp reaches the
  next entry, the default is prepended when the first entry is past beat 0, and
  no entries is the default alone. The host maps its own `Tempo` list into that
  shape, which is one line and a binding. `piece::Look` then places a position
  as `secs_at(beat) × rate` and a **length as the difference of two of them**,
  since four beats are longer later than earlier under a ritardando; the ruler
  is handed the same map so its labels and the boxes agree.

- ✅ **A session snaps to nothing, and its grid is musical while the prop is in
  frames** *(found 2026-09-09, reading why a dragged region landed at beat
  8.0558)*. `--session` never sets `Look::quant`, so the `multitrack` gets no
  `snap` and a drag lands wherever the pixel was; `q` over a selection
  quantizes to that same nothing.

  Setting it is not a one-liner, and that is the entry: `snap` is a number of
  **frames**, and under a tempo map a beat is not a fixed number of frames — so
  a grid of one beat cannot be stated as a frame count at all, and the obvious
  fix reintroduces exactly the single-ratio defect the tempo map just removed.
  The piece already carries what the grid should come from (its meter map), so
  what is missing is a **musical** snap: the widget's own axis knows the map
  (`tempo_map`), which is what makes the ruler right, and the snap has to be
  read off the same one instead of off a number the wire converted once.
  *(Closed 2026-10-04 by the user: resolved when the editors moved into the applications, `crates/clausters-apps/PLAN.md`.)*

- ✅ **The browser host's space bar never reaches `play_key`** *(fixed 2026-09-14: the page's key path answers the space bar among the window's keys, after a focused field and the widget under the cursor, as the native one does -- the take under the pointer, or the window's `play` over nothing, which is also what it does before a pointer has been over the canvas at all, since a piece is the window's. Reading the two key paths side by side found `Ctrl`+`S` missing from the page too, and it tells the window `save` now, as a desktop window does. Unchecked in a tab until the page is run by hand.)* *(found 2026-09-13, moving a piece's transport into the applications crate)*. The native front routes a space to `play_key`, which plays the take under the pointer or, over nothing a take answers for, tells the window `play` — what a multitrack editor reads as play/pause. The browser front maps the key to `HostKey::Char(' ')` for a focused field and stops (`host/web/input.rs`), so in a page the space bar neither plays a take nor reaches a script's editor. The host is one host compiled twice, and a key that does something in a window and nothing in a tab is the divergence that rule forbids: the page's key path should reach the same `play_key`.

- ✅ **A join made in a standalone host stitched an empty buffer** *(found 2026-09-13 by the user, by ear: "Join creates an empty clip that fails when stretched to the right"; the log said `buffer 4: 0 frames`)*. The entry that closed a join's source "at edit time" made it on the wrong server: `mint_sources` sent `/buffer_stitch` to the **player**, while the picture reads the **session**, which owns the takes. The player could sound it; the session had no such buffer, so the box drew empty and its edge had no length to stop at. A host with two servers now stitches in the session and attaches the join on the player once that stitch is done, which is the order a session's open already followed. Held by `a_stitch_done_in_the_session_is_attached_once`; the two sends themselves have no test, because nothing here fakes a server link.

- ✅ **A standalone host stopped hearing its player 200 ms after it booted, so a piece never sounded** *(found 2026-09-13, diagnosing the by-ear check `O32` was left open for: the space bar and the transport row logged `the piece is rolling` and nothing sounded)*. `await_player` waits for a spawned player with a 200 ms read timeout on the leg's socket, and that socket is an `Arc` the reply thread shares; the timeout stayed on, and `server_reply_loop` returned on any error but a refused connection. So the first 200 ms of quiet ended the thread, the piece's step queue was left held behind a `/done` nobody would deliver, and `/transport_play` -- which waits for its own `/done` -- sat behind it. The player had the whole graph, in the governed group, stopped at 0; `/proc` showed no `clausters-gui-server` thread. Now the probe takes its timeout off when it is done, and the loop reads a timeout as a quiet socket rather than a closed one. It was not a stale binary, and it is not what `crates/clausters-document/PLAN.md` `O33` is about, though it is what that milestone's log line showed: a host's reply path that can die unseen is exactly what one step runner fed by the same delivery everywhere would not have.

- ✅ **A standalone host's head never swept, and its meters never moved** *(found 2026-09-13 by the user, by eye, once the piece sounded: "There is no play cursor yet, only the positioning. The meters are not there yet")*. Two things a client does on every turn that the host's window never got.
  - **The anchor.** A linked timeline group is seeded by its **first** member's def-time props, and the multitrack editor's window puts the ruler first and stated `playhead_at` only on the piece -- so the group opened parked, and only the clients' `Transport`, which sets the anchor with a `/gui_set` after the window is open, ever swept it. The window (`clausters_apps::multitrack`) now states the anchor on the ruler too, so every endpoint opens it anchored and the client's later set is a restatement. Held by `a_piece_window_opens_with_its_head_anchored`.
  - **The meters.** The window is composed before anything sounds, and a client hands its editor the meters' buses in every `sync`; the host never did, so the strips had no bus to read. `sound_piece` now tells the editor and sets the piece's `meters` when they change. Held by `a_sounding_piece_tells_its_window_where_the_meters_are`. It is one more thing the host hands its editor by a path of its own, which is `crates/clausters-document/PLAN.md` `O33`'s third point.

- ✅ **A bent envelope was drawn bent and heard straight** *(found 2026-09-13 by the user, in `clients/python/examples/editors/edit_multitrack.py`: "It can be edited but it has no effect")*. `nodes::plan` sampled every curve's table linearly between break-points and never read a point's `data`, a choice written down on the grounds that `data` is opaque to the document. But the multitrack editor had already settled what `shape` and `curve` mean there -- its projection draws them and its reading writes them -- so the only thing left not reading them was the sound. The table now takes each segment's shape from its first point through `clausters_core::envshape::shape_value`, which is what the host draws the curve with; a point with no shape stays linear. Held by `a_curve_is_sampled_with_the_shape_its_points_state`.

- ✅ **In a standalone host a bent envelope straightened itself** *(found 2026-09-13 by the user: in the standalone the curve would not bend, while in the Python example it did)*. The multitrack hands each curve its points as one `f64` run (`parse_points` in `elements/multitrack/props.rs`), so a shape reaches the curve as `5.0`, and the curve parsed a shape with `as_i64`, which a float fails: it read linear. A client never noticed, because its host keeps the bend it drew and nothing sends the points again after an acknowledged edit; the standalone host redraws the whole piece after every applied edit (`adopt`), so each bend came back straight -- drawn straight and, since the document had the shape, heard bent. The curve now reads a float shape as the shape it names. *(The first attempt made the projection write an integer, which changed nothing: the float was made inside the host.)* Held by `a_shape_written_as_a_float_is_that_shape`.

- ✅ **A piece's undo was acknowledged with the tree's version** *(found 2026-09-13, moving the standalone host's editor onto the event a client receives)*. There is **one** history: the owner registers the piece as a structure of the same log, and a step walks back whichever structure each leg is over, reporting the version *that* structure was left at. The window's `undo` and `redo` arms threw that report away and settled with `document.version` -- the tree's counter, which a piece session never moves -- so after an undo the host went on drawing a version the piece did not have. Harmless while the host's own events carried no version; once they carry the one a client's do, the next edit would have gone out against a number below the floor and been refused as overtaken. Both arms, and the tree's `clips` arm, now settle with the version their own step left.

- ✅ **A standalone host never corrected a name it minted, and took the piece's counter for the conversation's version** *(found 2026-09-13, replaying the multitrack editor's recorded exchange against the host)*. After a split the host mints the new box's name and the document mints its id; a client's editor answers with the piece whole when the two differ (`settle`, after every changed turn), and the host never asked, so its window went on naming the box by the word it made up. And the version the host stated back was `piece.version`, which moves once per document edit, where a client's editor states the history's counter: a split is two edits and one turn, so from the first split on the two disagreed, and a gesture would have been refused as overtaken. The host now settles after each changed turn and keeps the conversation's version apart (`Owner::conversed`). Held by `the_recorded_exchange_is_answered_the_same_by_the_host`.

- ✅ **A joined box sounded and drew empty until it was moved to another track** *(found 2026-09-13 by the user, right after the join's parts were fixed: "it did not show the waveform in the resulting clip but played the contents correctly. On moving the clip to another track it appeared")*. Measured, not guessed: a temporary log of what each clip is drawn from read `source=-1 take=false` for the joined box after `j`, and `source=8 take=true` after the move. Following both turns through `answer_piece` gave the cause. Each turn hands the editor the host's buffer table first; the join's source enters that table only afterwards, in `mint_sources`; `adopt` projects the piece from the owner and draws the box right, and then the editor's `settle` -- which answers the join because the box names changed -- pushed the whole piece again, projected with the editor's table from **before** the source was minted, so the box went back to no buffer. The playback reads the owner's table, which is why it sounded. A move to another track starts its turn by handing the table over again, now with the source in it, which is why that fixed the picture. It was introduced the same day, when the host's settle was wired; a client's editor is handed its table before every call (`_sync_core`). The host now hands the editor the current table before `settle` and before a step's `resync_all`. Held by `a_joined_box_is_drawn_over_the_buffer_its_source_was_made_in`.

- ✅ **Joining cuts of joins nested sources until the server refused one** *(found 2026-09-13 by the user, "everything was going well, it kept joining cuts from several clips... until the last join I made failed"; the log said `/buffer_stitch: part 0: sources are stitched more than 4 deep`)*. A join minted a source whose parts named the sources of the boxes it joined, and a box over a join names that join -- so joining cuts of joins built a join of joins, one level per join, until the fifth. The user stated the model this breaks: *"segments should not nest as a data structure, they are two-dimensional pointers into a buffer or a file, like a memory view { sample, length }; join joins segments creating a new segments object [ { buf, sample, length }, ... ]. There can be no recursion, the objects must be read-only."* Now `read_join` reads a box over a join **through** to the takes that join is made of, cut to the span the box shows, so every source a join mints is one flat list of spans of takes however many joins it was made over. The multitrack editor keeps the segments of each join it mints (and a standalone host hands it the session's), which is all it needs: a join is read-only and never edited, so what it was made of is what it is. Held by `a_join_over_a_joined_box_names_only_takes`. The refusal it produced also left a hollow box, which is the entry below.

- ✅ **A stitch the server refuses still leaves a minted source behind** *(found 2026-09-13, diagnosing the join below; fixed the same day, see the note at its end)*. Both endpoints commit the source a join minted before the server has made it: the standalone host writes it into the session and the take table in `mint_sources` and pushes the stitch, and the Python client sends `Buffer.stitch(..., wait=False)`. When the server refuses (`/fail`), the piece already holds a box that is a window onto a source with no buffer -- drawn empty, silent, and fetched forever by the picture -- and nothing says why. A join whose parts cannot be made should be refused as an edit, with its reason, rather than applied and left hollow: either `sources::stitch` answers a part past its take as unmakeable before anything is recorded, or the endpoints undo the join when the stitch fails.

  **Fixed 2026-09-13 the first way, in the shared crate rather than in `sources::stitch`**, since by the time a stitch is built the join is already recorded. A trim is not bounded by its take, so a box can play past the end of its samples, and `read_join` is the first place the box and the take's length meet: it now refuses such a join as an edit, "one of these boxes reads past the end of its take" (one frame over is the rounding of seconds to frames, and is cut). The length comes through the table every caller already hands the editor: `Buffers::frames`, read by `projection::lengths` from an entry's `frames` (a zero means unknown, and an unknown length checks nothing, as before). Both clients' `sync` now send `Sources.held()`, which already carried it, and the standalone host hands its takes' frames with `set_lengths`. Python's `held()` reads `frames` only where it is a number, since a timeline's is not. Held by `a_join_past_the_end_of_a_take_is_refused` and `a_table_states_the_lengths_it_knows` in `clausters-editing`; the host's own join test had stated a take shorter than its boxes read, and now states the four seconds they do. A stitch the server refuses for any other reason still leaves the source unmade; none is reachable from an editor's own join now that joins are flat and bounded.

- ✅ **A join over more than two segments at once fails** *(the count was not the cause; diagnosed and fixed the same day -- see the note at its end)* *(found 2026-09-13 by the user, in `clients/python/examples/editors/edit_multitrack.py`, checking the step runner: "join fails when more than two segments are joined at once")*. A join of two fragments works, reordered or not; selecting three or more and pressing `j` does not. Not diagnosed yet: whether the refusal is the hand's (`why_not_joined`), the document's (`edit::join_regions` over a run whose parts are not all consecutive), or the stitch the minted source becomes, is the first question.

  *Reported again the same day, in the standalone host after `O33` closed: "there are still problems with joins without contents". Joins made by a hand can still come out as a box with no contents. Whether that is this defect or another one is part of the diagnosis; the join a session opens with now plays what it joined.*

  **Diagnosed, 2026-09-13, from the host's own log and the code, not by trying.** The count was never the cause; a join over three boxes is simply likelier to include a trimmed one. The log said it: `/buffer_stitch: part 0: buffer 1 has 96000 frames and the part asks for 134434`. A trim reported by a hand slides a box's window start and **leaves its duration as the piece had it** (`picture::read`, on purpose: the report says how long a box is, not how much of the source is behind it), so a left-hand trim of 0.8 s on a box over a two-second take left a window claiming 0.8 s to 2.8 s -- and a split leaves its left half the same way. Nothing heard or drawn reads `window.duration`, so none of that showed, until `read_join` built each part out of it: the part asked for samples the take does not have, the server refused the whole stitch, and the joined box was a window onto a source nobody made (the entry above). `read_join` now takes each span from what the box **shows** -- its length from its start, through the piece's tempo map -- and the joined box is as long as those spans. Held by `a_trimmed_box_joins_as_what_it_shows` in `clausters-editing`. What is left of the chain is filed on its own: a refused stitch still leaves a minted source (above), the picture fetches every take on every repaint (below), and what `window.duration` means after a trim (`crates/clausters-document/PLAN.md`, "Found by use").

- ✅ **A buffer that failed to be made is fetched a hundred times a second** *(found 2026-09-13, in the same log)*. When a session's join failed to stitch, the multitrack asked the server for buffer 5 on every frame -- `waits on server buffer 5`, then `buffer 5: 0 frames ... loaded`, then again, about ten ms apart, for as long as the window was open. A reply of zero frames is an answer, and the fetch machine has to hold it as one (drawn empty, asked again only when something says the buffer changed) rather than as a miss to retry.
  **Fixed 2026-09-13, and it was every take, not only the missing one.** The pass each front runs before every repaint is meant to serve only the elements told their resource moved, but it also pushed every `Needs::takes` of the multitrack: natively each take was mapped again and its summary rebuilt per frame (a full pass over the samples where the server wrote no overview), and on a leg that downloads, a finished download started again on the next frame. The repetition was also hiding a race: a join's box names its buffer in the turn `mint_sources` queues the stitch, so the first ask can find the buffer unallocated, or published and not yet copied into, and the next frame repaired it. Now an element answers its takes through `Samples::ask_takes` -- all of them when a window is built, only the ones not yet asked for on a repaint -- and `/done /buffer_stitch n` makes every element that asked for `n` forget it (`Host::forget_stitched`, called by both fronts), so a join is asked for once more when its samples are there. Held by `a_take_is_asked_for_once_and_again_when_forgotten` and `a_stitched_take_is_asked_for_again`.

- ✅ **A change of shape redefined the whole window, so an edit in one lane cost
  every other lane its screen state** *(found 2026-09-06 by the user, by eye:
  splitting a clip reset the vertical zoom of every lane)*. First narrowed to a
  redefine of the smallest subtree whose shape moved, which needed the host to
  splice a non-window def into the drawn tree at all — the client measured two
  defs and zero window rebuilds while the host drew nothing new; an eye on the
  window is what caught it. Then answered at the root by the reconcile (`AP5`).

- ✅ **A set was addressed to a widget the redefine beside it had just removed**
  *(found 2026-09-06 in the log)*. Two generated tests exonerated the walk and the
  wire, because the defect was in the premise both were written under: the
  picture the difference compared was the client's, and the widget was freed in
  the host's. Closed 2026-09-07 by removing the sentence: a publish is one
  `/gui_def`, with no set beside it.

- ✅ **A clip dragged past the first or last lane oscillated back to the start**
  *(found 2026-09-06 by the user, by eye)*. The drag is the host's
  `elements::multitrack` now, with the row index clamped
  (`a_drag_past_the_last_lane_stops_at_it`).

- ✅ **The playhead drew behind the clips** *(found 2026-09-06 by the user)*. It
  stopped being a lane prop: the navigation group's line is drawn in the overlay
  pass, over every row, so no redefine can put it behind anything.

- ✅ **A clip could not change lanes once a split had happened** *(found
  2026-09-06 by the user)*. Closed with a test rather than by the subject going:
  `a_half_left_by_a_split_still_changes_lanes`.

- ✅ **Dropping a clip onto another drew the lane as one layered clip** *(found
  2026-09-06 by the user)*. The rule that collapsed simultaneous members went
  with `FormEditor`; two regions at one offset draw as two overlapping boxes,
  because that is what they are.

- ✅ **Screen state was keyed by an address** *(found 2026-09-06, fixed the same
  day)*. Four tables under `id(object)` handed a freed object's state to the next
  one at that address; keyed by the object, weakly. Every one of them read
  correctly and failed only after a free.

- ✅ **An `Application` built with no version callable read its editors before it
  had any** *(found and fixed 2026-09-06)*. The list is made first; every editor
  passed a callable, so only a test constructing one alone reached it.

- ✅ **The routing table's tag list was written twice** *(found 2026-09-06,
  lowered 2026-09-09)*. It is `clausters_document::view::NOT_AN_EDIT`, read once
  by each client.

- ✅ **`GuiHost.redefine` and `Application` existed in Python and not in the web
  client** *(found 2026-09-06 and 2026-09-07, closed 2026-09-09 by `W30`)*. Found
  only because somebody writing a wire word needed the door: a gap that costs
  nothing to have is a gap nothing will find, which is why the two clients are
  read against each other verb by verb.

- ✅ **The undo walk was each client's** *(found 2026-09-06, settled 2026-09-09,
  taken to the crate by `AP7`)*. Its data half became `History::walk`; the rest
  became the editing context's turn and step.

- ✅ **`SamplesDomain` held the inverse between two calls that did not mention
  it** *(found 2026-09-06, closed 2026-09-14 by `AP7` step 2)*. The gesture is
  read once in the application's editor, and the inverse comes off that reading.

- ✅ **A publish that redefined widgets did not always tell the host its
  version** *(moot 2026-09-06 with `FormEditor`)*. Kept for the observation: the
  pairing was a convention at five call sites rather than a mechanism where the
  redefinition is decided.

- ✅ **`Echo.raise_floor` was called by nothing** *(fixed 2026-09-07)*. The verb
  stayed and the assignable floor went, in both clients: a floor that can be
  assigned is not a floor, and the protocol test moved onto the path an editor
  takes.

- ✅ **A `signal`'s docstring describes it "inside a `clip`", and there is no
  clip container any more** *(found 2026-09-21, fixing the books' references to
  the builders `b75a4298` removed)*. `signal` in both clients
  (`clausters/gui/guidef.py`, `src/gui/guidef.ts`) documents `at`/`dur` as a
  body's stretch of "the clip's own time" and `start`/`loop` as its window onto
  its samples, "only" inside a `clip` — the container a `field` with a
  placement used to be. The host still reads the four (`widget::parse::span`,
  the body window in `widget/props.rs`), so the props are not simply dead: what
  is open is **which container gives them a meaning now** — a multitrack box
  draws its contents through the body door from its own `clips` row, not from
  a child `signal` — and so whether the docstrings are to name that container,
  or the props are surface nothing can reach from a GuiDef and go in both
  clients and the host. The `pianoroll`'s docstrings name a "compact `clip`
  roll" beside it in the same way.

  **Answered and fixed 2026-09-22: the props are live and the container is the
  one that measures time.** `widget::mod` parses `at`/`dur` off any node and
  `host::layers::layer_input` places the child by them, inside whatever
  container holds a time axis -- which is the layer mechanism `b75a4298` kept
  whole on purpose ("the container it was written for is exactly what an
  entered box brings back"). So nothing was dead; what was wrong was the
  *name*: the four props say what a **layer of a placement** is, and the clip
  that used to be that placement cannot be written any more. Both clients'
  `signal` docstrings say that, and the two `pianoroll` ones -- plus the two
  examples that echoed them -- name the compact roll a `multitrack` draws
  inside a box instead of a "`clip` roll".

- ✅ **The audio editor showed no play cursor, and space paused instead of
  stopping** *(found 2026-09-24 by the user, in `edit_audio.py`: "I cannot
  tell where the cursor is"; fixed the same day)*. The take monitor rolled the
  transport and drew nothing of it, so where the sound was could only be
  guessed; and a second press paused where the pass stood, so the next press
  continued from there rather than from the mark. Now the monitor anchors the
  view's play cursor to the transport's position (the host draws it from the
  engine's counter, as a multitrack's), space plays from the position cursor —
  a selection loops — and stops back at it, and **Home**/**End** put the
  position cursor at the take's start and end. Pausing is still the monitor's
  verb (`Host::pause_playback`); no key reaches it.

  **Neither cursor was there when the window opened** *(the same day)*. The
  audio editor's window placed no position cursor and anchored no play cursor,
  and the host drew heads from the device clock until the first play. The
  catalogue's waveform now takes a `cursor` beside `playhead_at`, the clients'
  window and the standalone host's take panes both open with the two at 0, and
  both clients' audio editors ask for the transport clock when they open, as
  the multitrack's do. A stop leaves the play cursor anchored: the stop puts
  the position back on the mark, so the line stands there.

- ✅ **Space did nothing in a freshly opened audio editor until the mouse
  moved** *(found 2026-09-24 by the user: "on opening the window the cursor
  is there, but pressing the space bar does nothing until the position is
  changed with the mouse"; fixed the same day)*. The monitor's keys were
  addressed to the view under the pointer, and a window that has just opened
  has no pointer: it is unknown until it moves, so the native front skipped
  the take gesture and sent the multitrack's window verb, which an audio
  editor does not answer. Now space, Home and End with no pointer — or over
  nothing — reach the window's **one** take, when the window has a single
  timeline view and it draws samples; a window of several views (a
  multitrack, its ruler, its take panes) is still addressed by pointing. Both
  fronts.

- ✅ **A mono take was heard in the left ear only** *(found 2026-09-24 by the
  user in `edit_audio.py`, who took it for a regression; it was not — the
  monitor had played a mono take on bus 0 alone since it existed, and the
  day's measurements show the right channel at zero throughout; fixed the same
  day)*. The monitor starts one reader per channel, each to the bus of the
  same number, which is right for a stereo take and puts a mono one on the
  left only. A mono take now gets two readers of its one channel, to the left
  bus and the right, as a mono file is heard in any editor.

- ✅ **The head clock is one per host, and a host can hold applications on
  two transports** *(found 2026-09-24, closing the root plan's `T6`; fixed
  the same day)*.
  `/gui_headClock "transport" <id>` names which transport's position every
  playhead in the host is drawn from, and that was enough while a server had
  one transport. With several, a standalone host running a multitrack on
  transport 0 and an audio editor on another would draw one of the two lines
  from the wrong transport. What it wants is the transport per window, or per
  view, beside the anchor a window already carries -- decided when the audio
  editor takes a transport of its own (`crates/clausters-apps/PLAN.md`, `X7`).

  **Per view, and the id is required** (the user). `/gui_headClock <id>
  <which> [transport]` names the counter on a window or a widget; a widget
  draws from its own or its nearest ancestor's, then its window's, then the
  host's `--clock`, so one window can hold several complex widgets that each
  play a timeline. The host reads every counter a window needs once per frame
  (`Host::head_clocks`), each element and gesture asks for its widget's, a
  freed widget takes what was named on it along, and a page polls
  `/transport_query` for each transport a visible playhead reads. The
  monitor names its take's view, and the clients name the window.

- ✅ **A window repaints whole for a line that moves in one view** *(the
  user, 2026-09-24, with the head clock per view)*. A frame is the window's:
  every view is redrawn while any playhead in it moves. Now that the host
  knows which views draw a moving line and from which counter, a frame could
  repaint only those -- a change to the drawing pipeline rather than to the
  protocol, and a design of its own. *(Checked 2026-10-04: still so -- every
  pass begins with `LoadOp::Clear` and nothing tracks a changed region; the
  user marked it important.)* *(Measured 2026-10-04, a release host, the
  edited multitrack example at 1000x560 on an integrated GPU: a whole frame
  is about 1 ms of CPU -- 0.3-0.5 building ~26 000 vertices, 0.2-0.3
  uploading, the rest encoding and presenting -- at 50 fps while it plays
  and 30 fps stopped, which its meters keep drawing. The process stays at
  5-7% of one core. A cached base picture would save about half of that
  millisecond, so the entry waits until a heavier window measures worse. The
  multitrack's playhead is in the base mesh, not the overlay, so splitting it
  out is part of that design.)*

  **Built 2026-10-06** *(the user called it one of the important optimizations)*: **a
  window keeps its picture, and a tick draws what moves.** A frame is four
  batches -- base, over, **live**, top -- and an element draws everything a
  clock or a fed value moves on its live layer (`Element::draw_live`,
  `Needs::live`): the playhead of a timeline view, of a roll and of a
  multitrack, a multitrack's meters, a page's cursor. A front says what each
  redraw was asked for (`Batches::want_whole`, `want_live`), and a frame asked
  for what moves alone builds the live mesh from the placements the last
  whole frame kept and uploads only that; when the live mesh is where it was
  to a quarter of a pixel it draws **nothing**, so the rate follows how far a
  line moved. A reading set many times a second is a live prop: a `label`
  with `live` draws its text there, and a `/gui_set` of it asks for the live
  layers alone (`HostEffect::RedrawLive`) -- the multitrack's clock is one,
  which is what lets its window keep its picture while it plays.
  - **Measured** (release, the same machine, CPU time of the process). The
    standalone multitrack session, host with its server and audio: **9.5 % ->
    2.8 %** of a core at rest and **14.8 % -> 8.8 %** playing; at rest every
    tick finds nothing moved (32 a second at 0.045 ms, where there were 30
    whole frames), and playing, a frame is the live layers alone. The score
    editor opened from a client, the host process alone: **11.2 % -> 1.1 %**
    playing, 0.8 % -> 0.7 % at rest.
  - **What is kept is the describing, not the drawing.** The card still draws
    the whole window, from buffers it already holds, into a cleared pass:
    nothing tracks a region, and a swapchain keeps no picture to draw over.
    An offscreen copy of the picture, blitted under the live layer, is what
    would save the card's own work; nothing measured asks for it yet (the
    live frame's remaining 0.8 ms is the pass and the present).
    *(Revised 2026-10-27: measured on the audio editor in a large window,
    the card's work was what a playing window cost, and the picture under
    the live layers is kept on the card, the live layers multisampled in
    their boxes alone: `host::frame::underlay`, and `docs/decisions.md`,
    "A window keeps its picture, and a tick draws what moves".)*
  - **Whole wins, and silence is whole.** A frame the platform asked for, a
    window with a dialog up (its lines are under the scrim) and a picture
    drawn before the glyph atlas was packed again are drawn whole. An element
    that reads a bus, a level or a tap and does not say `live` -- a `meter`, a
    scope, a shader `canvas` -- makes its window repaint every tick, as it
    did; moving their live parts over is one element at a time.
  - **The playhead is the top of its view now**, over the selection and the
    readout it used to be drawn among: it is on a layer of its own.

The entries below are one audit of the host's orchestration layer
*(2026-09-25, asked by the user once `host/mod.rs` reached 5748 lines)*:
`mod.rs`, `play.rs`, `instance.rs`, `document.rs`, `gestures/` and the two
fronts, read for redundancy and for fixes narrower than the rule they fixed.
The renderers (`elements/`, `graphics/`, `widget/`) were searched, not read.
The order is the one to take them in: what is wrong first, then what is
duplicated, and the file is split last, so no duplicate is carried into a
module of its own.

- ✅ **The monitor's loop is sent in the take's frames** *(audit
  2026-09-25)*. `Host::set_loop` (`play.rs`) writes a selection straight to
  `/transport_loop` on the monitor's transport, where the positions are the
  engine's samples; every other position the monitor sends goes through
  `AudioEditorPlayback`, which converts. A take at another rate than the
  engine's loops somewhere other than the selection drawn -- the fault fixed
  for the play cursor the same day, on the path that fix did not reach. The
  loop belongs to the playback, as the locate does.
  **Fixed 2026-09-25**: `AudioEditorPlayback::set_loop` converts the span as
  `locate` does, and the host's `set_loop` is that call; the host's
  `send_sound`, whose last caller it was, went with it.
- ✅ **Nineteen doc comments sit on the wrong item** *(audit 2026-09-25)*. An
  item was moved or removed and its doc stayed, fused onto the next one: in
  `mod.rs` the `/gui_query` and `/gui_ack` docs are on `say`, `voice_on`'s is
  on `element_midi`, `send_to_player`'s is on `sync_bus_watches`,
  `blob_args`' is on `watch_msg` and `json_arg`'s is on `json_table`; the
  rest are in `fetch.rs`, `document.rs`, `gestures/nav.rs` (three),
  `elements/notes.rs`, `elements/signal/bulk.rs` (two), `elements/signal/mod.rs`,
  `elements/multitrack/{mod,draw}.rs`, `widget/element.rs` (two),
  `widget/props.rs` and `gui/app.rs`. rustdoc renders each as one paragraph
  that describes two things, and nothing lints it.
  **Fixed 2026-09-25**: each doc moved onto the item it describes where that
  item is still there (`on_query`, `on_ack`, `voice_on`, `blob_args`,
  `json_arg`, a fetch test, `long_at`, `want`, `sample_shape`, `live`,
  `boxes_on_screen`, `needs`, `bulk`, `write_samples`, `key_pressed`,
  `to_key`, `set_selection`, `locate_timeline`, `locate_at`, `EditorProps`),
  and dropped where it is not (a marquee the container owns now, a stray line
  of `send_to_player`'s). One link the same pass found broken,
  `Host::playhead_clock`, names `head_clock_of` and `head_clocks`.
- ✅ **The two fronts each dispatch the buffer replies** *(audit 2026-09-25)*.
  `gui/serverleg.rs` and `web/serverleg.rs` both match `/buffer_query.reply`,
  `/buffer_getRange.reply`, `/buffer_touched`, `/buffer_stream.reply` and
  `/buffer_peaks.reply`, with bodies copied and already apart: the rate of a
  `/buffer_query.reply` is read by two different functions (only one takes an
  `Int`), and `/buffer_touched` is a method in one front and inline in the
  other. Beside them, ten functions exist in both fronts at a similarity of
  0.7 or more -- `apply_extents`, `widgets_drawing`, `is_space` (identical),
  `summary_bucket_of`, `to_key`, `on_stream_report`, `fetch_wanted_spans`,
  `place_detail`, `read_span_back`, `apply_gesture_effects`. The host is one:
  this is the shared host's, behind the one thing a front differs in (how a
  message leaves).
  **The replies half, 2026-09-25**: `host/replies.rs` reads the five buffer
  replies once, as the provided methods of a `Front` trait whose required
  half is what a front differs in -- its slots, its redraw, its way out.
  Where the copies had drifted the stricter one won: a summary fills a slot
  only for a view that claimed a geometry one, a rate is read in any numeric
  kind, a mesh-drawn take resyncs the recording streams, and what a
  placement says is `info` in both builds (a page's tests read it).
  **The rest, the same day**: `apply_extents` is a `Host` method, and the
  winit key mapping (`to_key`, `is_space`) is `host/winit_keys.rs`, read by
  both shells. `apply_gesture_effects`, `on_press`, `on_wheel`,
  `advance_edge_scroll` and `play_key` stay per front: they share a shape,
  but each arm is the platform's (a socket or a page's queue, a blur or
  nothing) and what they share is already the host's (`event_message`,
  `deliver`, the gesture machine).
- ✅ **Forgetting a widget is six functions, and two call sites disagree**
  *(audit 2026-09-25)*. Bindings, voices, focus, head clocks, timeline groups
  and the monitor's files each keep state by widget id and each prune it their
  own way (`prune_*`). `/gui_free` prunes five; a redefine that replaced
  widgets prunes three, so a clock named on a widget a redefine removed stays;
  the monitor prunes only when it next plays. One "these ids are gone" pass,
  called from both.
  **Fixed 2026-09-25**: `Host::forget_gone` is that pass, run by a free and
  by a redefine that replaced widgets. The one-line prunes are inside it;
  the timeline's stays its own (a group sync runs it too), the voices' says
  so to the server, and the monitor's now also forgets a take it was
  playing that is gone.
- ✅ **Five walks find the views of a buffer** *(audit 2026-09-25)*.
  `refresh_buffer_views`, `patch_buffer_views`, `stream_buffer_views`,
  `write_buffer_views` and `forget_take_views` are one recursion -- every
  samples element drawing buffer N, do one thing, count -- written five times;
  `collect_stream_wants` and `span_to_read_back` are the same walk read-only.
  And `buffer_of`, `buffer_frames`, `buffer_channels` and `buffer_rate` look
  the same element up four times to read four facts of it.
  **Fixed 2026-09-25**: one recursion, `samples_views`, and `buffer_views`
  over it for the pictures of one buffer; each pass is a closure at its call
  site. `Host::samples_of` is the one lookup, `play_buffer` reads the buffer,
  shape and rate off it once, and `buffer_channels` and `buffer_rate` are
  gone (`buffer_of` and `buffer_frames` stay, for the gestures that ask one
  fact).
- ✅ **A definition installs a tree by two copies of the same steps**
  *(audit 2026-09-25)*. `define_node`'s window branch and its in-window
  branch each build, reconcile, resolve the style, resync the bus taps, the
  buffer stream and the timeline groups, and reopen. The bus-tap and stream
  resyncs are paired at four call sites, and the theme is cloned into a new
  `Arc` at every one of four style resolutions.
  **Fixed 2026-09-25**: `build_tree` builds, reconciles and styles for both
  branches, `tree_changed` resyncs and reopens, and `sync_subscriptions` is
  the pair. The theme's copy stays: `Host::theme` is a public field a
  launcher writes, so an `Arc` kept beside it would go stale, and a method
  that made the copy cannot be called where `set_props` holds the tree.
- ✅ **The owner's answer repeats itself** *(audit 2026-09-25)*.
  `answer_tree`'s undo and redo branches are the same fifteen lines; six
  settles build the same `Acked` from a version. `answer_multitrack` hands the
  editor its multitrack, source table and lengths twice in one turn, and
  `document.rs` does it a third way that also hands the segments -- which the
  two in `mod.rs` do not, so a turn may run on stale segments (to be checked,
  not assumed). One "bring the editor up to the owner" method.
  **Fixed 2026-09-25**: `Owner::sync_editor` is that method, `settle_at` the
  self-answered settle, and undo and redo one branch. The segments were
  checked and are not a defect: the editor learns every join a turn mints
  (`MultitrackEditor::learn`), and `set_segments` only adds the session's
  own when the window opens.
- ✅ **`host/mod.rs` holds every concern of the host** *(audit 2026-09-25)*.
  3760 lines of code and 1990 of tests in one `impl Host`: the protocol
  dispatch, the definition path, the owner's answer, the buffer views, the
  head clocks, focus, status, bindings, voices, subscriptions and the OSC
  argument helpers. Split by concern after the entries above, each module
  with its tests; the OSC argument helpers join the ones in `status.rs` and
  `gui/serverleg.rs` rather than becoming a fourth set.
  **Fixed 2026-09-25**: `mod.rs` is 1313 lines -- the `Host`, its links, its
  windows' sizes and the `/gui_*` vocabulary -- and the rest is one file per
  concern: `wire.rs`, `define.rs`, `set.rs`, `answer.rs`, `takes.rs`,
  `clocks.rs`, with bindings, voices and the status bar's methods beside
  their own modules (`bind.rs`, `voices.rs`, `status.rs`). The tests follow
  the crate's idiom (`widget/tests.rs`): `tests.rs` for the protocol and
  `write_tests.rs` for strokes and the monitor. The argument helpers are
  `wire.rs`'s: `status.rs`'s `arg_str` is its `string_arg` now, and
  `gui/serverleg.rs`'s `float_arg` went with the replies.

- ✅ **A roll's curves do not say what they are or where they stand**
  *(the user, 2026-09-30, trying the notes editor's curves: hovering over an
  envelope, its points, the lane below and the notes' boxes could show a
  status saying which control it is and what value it has)*. A lane's row
  carries its label in the keyboard's column, and a note's layer carries
  nothing: over a bend, a pressure or a point there is no way to read which
  control it drives or the value under the pointer, and over a note's box
  none of its keys. The multitrack's curves have the same gap. What is
  wanted is a hover readout -- the control's name and the value at the
  pointer, a point's exact value, a note's pitch and level -- drawn by the
  host in the element's own space, since the value's units are the curve's
  (`min`/`max`) and the element already knows them. The row labels need it
  too: a lane's label is drawn in the keyboard's column, which is narrow, so
  `editors/edit_midi_file`'s "volume" lane shows only its first letters (the
  user, 2026-09-30) -- the hover is where the whole name is read. **Done
  2026-10-01**, in the readout the roll already drew in its grid's corner: over
  a lane its whole label and the curve's value at that time, over its label
  cell the label, over a layer's line the control's (a bend in signed
  semitones), a break-point's own value on a point, over a note its pitch,
  velocity and channel. The multitrack draws the same readout over its rows
  and its clip envelopes. Both read a value through the curve element's own
  `hover_value`, and format it by one rule (`graphics::bpf::readout_value`).

- ✅ **The points editor carries a range and a selection it has no use for**
  *(found 2026-10-02 closing `X4` in `crates/clausters-apps/PLAN.md`, as "A
  curve standing on its own cannot be swept"; rewritten 2026-10-03 with the
  user, who placed the defect in the rule rather than in the gesture)*.
  `C61`'s two words (`clients/python/PLAN.md`) were decided for the three
  applications being made at the time -- the multitrack, the roll and the
  audio editor -- where what the hand marks is what their operations act on:
  held regions, events moved, deleted, quantized and copied together, a
  range of samples. `X4` then gave the points editor the same `span` and
  `selected`, reading the rule as one for every editor, outside the context
  it was decided in. In the points editor no operation acts on a group of
  points -- what a hand takes there is one segment, for its shape -- so the
  range and the points in it are surface no hand makes and nothing reads,
  and the sweep this entry first asked for would have been a gesture with
  no consequence.

  **It is also the class hierarchy failing.** The three applications each
  write `selected`, `select` and `unselect` on their own, over the host's
  `selected` prop, and the points editor has a `span` and a `selected` of
  another shape; the base `Editor` says nothing about either. Marking is a
  capability some editors have, not a level of the hierarchy, so the fix is
  a **mixin** -- a trait, in Rust's terms -- that the editors whose
  operations act on what is marked take, and that the points editor does
  not until it has such operations. Its `span` and `selected` go, in both
  clients and in the crate's points editor.

  **If it ever has them** (moving or deleting several points, copying a
  stretch of an envelope, scaling the values in a band), the sweep is what
  it takes: a standalone `curve` navigates no time axis
  (`OnAxis::navigates_time` is `false`), so it joins no navigation group,
  every modifier hands the press to its points, and a `sel_start` /
  `sel_len` it is set with is drawn nowhere. The curve would join a group,
  opt-in as a signal view's `navigable` is, so the pictures that draw a bare
  envelope do not move; Alt would sweep the range there, as on a roll, with
  the value band through `select_in`; and its axis would be in **samples**
  at its `sample_rate` rather than in its own seconds, since the group
  rounds a selection to whole samples and a three-second curve would be
  swept in whole seconds.

  *(Fixed 2026-10-03.)* **The points editor marks nothing.** Its `span` and
  `selected` are gone from both clients, and from the crate's points editor
  the range and the value band it kept, the `selection` gesture it read, the
  `span` and `selected` verbs of its door, the `span` of its outcome and the
  `sel_*` props it corrected the curve with. **Marking is a mixin**:
  `Marking` (`clients/python/clausters/gui/editing/marking.py`,
  `clients/web/src/gui/editing/marking.ts`) holds `selected`, `select` and
  `unselect` once, and `NotesEditor`, `MultitrackEditor` and `AudioEditor`
  take it — in Python as a base class beside `Editor`, in the web client
  by the prototype copy `Server` composes its own mixins with. An editor
  answers two hooks, what is marked now and marking in its place, and where
  the host keeps the marks (the roll and the multitrack, a widget's
  `selected` prop) the read and the write of that prop are the mixin's too.
  **One shape in the web client, which moved the audio editor's**: its
  `selected` was a getter, the roll's and the multitrack's a method that
  awaits the host, and one mixin states one member — so it is
  `await editor.selected()` in the three, and `select` and `unselect` answer
  a promise in the three (`docs/bindings.md`, the asynchronous idiom). The
  Python surface did not move. The crate has no trait: its three
  applications keep no marks — the host's widget does — so there was
  nothing there to share.

- ✅ **A bundle mounted by the standalone host is given ids by an allocator
  of its own** *(audit 2026-10-03, asked for by the user while transports
  became a space of the ids: "check whether there are several
  implementations for assigning ids")*. Every endpoint allocates server ids
  through one policy, `clausters_core::ids::IdSpaces` over the core's
  `Registry`, and widget ids through `clausters_core::widgetids`; the
  clients' allocator classes are views of those and state no range. Two
  places in the host still count by hand.
  - **`bundle::MountAllocator`** (`src/host/bundle.rs`), which
    `--standalone` mounts a bundle's manifest with: a bump from fixed bases
    -- widgets and nodes from 1000, buses from 64, buffers from 32 -- that
    releases nothing. Its comment says the standalone front is the only
    client of its embedded server, which was true before the host allocated
    for itself. It does now (`Host::ids`, an `IdSpaces` with the whole
    share): its voices, its take monitor and a multitrack's nodes come from
    the client range, which also starts at 1000, and its buffers from 0. So
    a bundle that declares a node and a host that then plays a voice or a
    take are handed the same node id by two allocators. The fix is the
    mount allocating from the host's `IdSpaces` -- the browser leg already
    does, from the page's (`base/pool.ts`).
  - **The rolls' windows** (`open_roll`, `src/host/answer.rs`): a def id of
    `1000 + 10 * source`, ten ids apiece by formula, in the widget-id window
    a client's `WidgetIds` leases from at the same base. A session owned by
    the host has no client defining widgets beside it, so nothing collides
    today; the ids would come from the host's own widget-id table.
  *(Fixed 2026-10-03, with a third case the fix turned up.)*
  - **The mount allocates from the host's tables.** `bundle::MountAllocator`
    is gone: `bundle::allocate` takes the node ids, buses and buffers from
    the host's `IdSpaces` and the widget block from the host's widget ids,
    so the host exists before the mount and nothing it plays later is handed
    a bundle's id. The fixed bases went with it: a bundle's control buses and
    buffers now start where any client's do, as on the browser leg, rather
    than at 64 and 32.
  - **The host has widget ids of its own.** A `WidgetIds` over a window past
    the one its clients allocate from (`clausters_core::widgetids`:
    `CLIENT_BASE` and `CLIENT_CAPACITY`, then `HOST_BASE`), so what a host
    opens for itself and what a client attached to it defines never answer
    to one number. A roll's window and its notes widget are named there by
    the source they draw and forgotten when the roll closes; a mounted
    bundle's widgets are a block of it (`WidgetIds::alloc_block`, new).
  - **A page had two maps over one window.** A component mounted on the
    page's host took its widget block from a registry of the page's pools,
    and a `GuiHost` attached to that same host named its windows from a
    table of its own; both started at 1000, where the comment in
    `base/pool.ts` said they were one. They are now: `pageWidgetIds` is the
    page host's one table, the pools' widgets are a view of it and an
    attached `GuiHost` allocates from it. A host of a handle's own, and a
    share of one, keep their own table.
  - **Left:** the session window a standalone host opens is still def 1 with
    its chrome at 2 to 8, in the hand-picked range below every allocated
    id, which is what that range is for.
- ✅ **A texture view inside a dialog is drawn under it** *(found 2026-10-04,
  closing `G40`; fixed the same day)*. A dialog's flat drawing goes into the overlay batch, over
  the window; the heavy views' texture passes run between the two batches, so a
  spectrogram or a shader `canvas` placed in a dialog is drawn under the dialog
  that holds it. A dialog of controls is unaffected. The fix is a pass order
  for what stands over the window, not a dialog's own. *(A window now draws
  three flat batches, with the window's textures after the first and the
  dialog's after the second; the host's bands and lists are in the third.)*
- ✅ **Rows past what a def should carry** *(left open by `G41.1`)*. A
  `table`'s rows ride the def and every `set` of them; a list of tens of
  thousands of rows wants the bulk path, and which size is the line is not
  decided.

  **Decided 2026-10-06, by measuring** (`tests/table_rows_cost.rs`, a release
  host, rows of three cells): a thousand rows are 35 kB and a third of a
  millisecond to replace, twenty thousand 700 kB and 7 ms, fifty thousand
  1.8 MB and 19 ms, a hundred thousand 3.5 MB and 38 ms -- past the 33 a frame
  is. **The line is fifty thousand** (`graphics::table::WIRE_ROWS`), where a
  set is still well inside a frame, and it is a line rather than a wall: a
  longer table is shown whole and the host warns once a set. **No bulk path
  for rows**: what bounds a table sooner is its carrier (a datagram holds
  some eighteen hundred rows, a stream frame nine times the line), and a
  list past it is one its owner pages -- it holds the rows and sends the
  stretch a reader is in -- rather than one that needs a second format. The
  measurement found a cost that was not the wire's: a draw, a press and a key
  each walked every row to find the ones a fold leaves showing, so a frame
  cost the length of the list; the list is kept now and walked only when the
  rows or a fold change.
- ✅ **A context menu and a tip per part of a heavy view** *(left open by
  `G37.2` and `G38.2`; closed 2026-10-04 by the user: it is being done in the
  applications, `crates/clausters-apps/PLAN.md`)*. Today one menu and one tip answer for the whole
  widget; a clip, a note or a box answering with its own is a question for the
  editors that hold them.

- ✅ **The host reads no Command key** *(found 2026-10-04, writing `G36`)*. Both fronts read Ctrl, Shift and Alt, and the native one reads a Mac's Command as nothing, so `Ctrl+Z` is Ctrl+Z on a Mac too where every other program there undoes on Cmd+Z. The key table can spell only what the fronts read. **The decision:** whether a chord's `Ctrl` means the platform's command key (Cmd on a Mac, Ctrl elsewhere -- the usual reading of a portable binding) or a fourth modifier `Cmd`/`Meta` is added and the defaults name it per platform.

  **Fixed 2026-10-04, decided by the user: `Ctrl` is the platform's command key.** The two fronts read one rule (`winit_keys::command`): on a Mac the host's Ctrl is Command -- natively when built for macOS, in a page when the browser says it runs on one -- and Control is the modifier the host has no name for, as the logo key is elsewhere; a key pressed with that one reaches nothing, so Control+E is not a bare `e`. It holds for every gesture qualified by Ctrl, not only the keys, which is how a desktop toolkit maps it there. A file may spell it `Cmd`, and a menu on a Mac shows `Cmd+Z`.

- ✅ **The host's text was half again the size of a desktop's, and every symbol looked small beside it** *(found 2026-10-05, the user looking at the score editor: "even at 130% the text is too big ... the proportions have to be found", and then pointing at the text of the page the window was mounted in)*. The default `text_size` was 2: capitals 14 logical pixels high, the whole cell, where a browser's default text has capitals of 11 to 12 and an interface's 9 to 10 -- and with no air in its line, since the line *was* the capitals. **Fixed 2026-10-05: the default is 1.5**, capitals three quarters of the cell, and **a line is the cell**: `size::line_box` no longer shrinks with the text, so a label is as high as a control and a table's rows keep their pitch. Nothing else in the table moved but the two roles measured in characters (`header_w`, eight characters, 96 -> 72). It is the size of every window, not the score editor's; a script that wants the old one says `text_size: 2`. `docs/decisions.md`, "Text is three quarters of its line".

- ✅ **An outline glyph was a character-wide cell, and a thin stroke was lost in it** *(the same day; the user: "the sharp in the toolbar is deformed, it is missing a line")*. A window's outlines (`font::outline`) were centred in the bitmap's cell and stepped by its nominal advance, and a music font's uprights -- 17 units of 1000 -- are half a pixel at an icon's size: with no antialiasing a fill that thin lands between pixel centres and is not drawn. **Fixed**: an outline steps by its own width (never under the cell), and under the size where such a stroke is a pixel its edges are drawn over the fill, as wide as what it is short of one (`outline::HINT`); a single character wider than the box it is centred in is drawn across it instead of cut to an ellipsis (`font::text_centered`).

- ✅ **A page's text was drawn at 1.4 times the size it was laid out for** *(the same day)*. The `score` element took the font size the engraver names -- the em -- for the height of the host's line of capitals. Fixed (`graphics::score::CAP_PER_EM`); the record is in `crates/clausters-apps/PLAN.md`, "Two lines of the page's head run into each other".

- ✅ **A plane that flows its children could not scroll them** *(the same day)*. A `plane` with `flow: "col"` and no `content_h` was its viewport: a column of folding sections taller than the window had its last ones out of reach, and no number could be declared for it, since how long it is depends on what is folded. **Fixed**: a flowed plane is as long as its children are along the flow (`layout::scroll_content`, `Widget::flowed_content`), never under the pane.

- ✅ **An element could not be told the focus had left it** *(the same day, building the score's in-place text edit)*. A field delivers on every key, so nothing ever needed to end an edit; an element that holds one until it is done had no way to learn that a press went to a tool. **Fixed**: `Element::blur`, called from the one door that moves the focus (`gestures::focus::set`), and whatever it answers is reported like a key's answer.

- ✅ **A widget's text is not on whole pixels at a fractional display factor** *(found 2026-10-05)*. The table's text roles are quantized when a window resolves them, and a widget's own `text_size` deliberately was not ("the number it sent, scaled"): at a factor of 1.25 the default 1.5 is 1.875 font pixels to the cell pixel, and the bitmap face's own pixels came out unequal. **Fixed 2026-10-05** (the user: "fix the scaling problem"): the bitmap face is drawn at the whole step nearest the size asked (`font::bitmap_pixel`; 1.5 the one half step), centred in the cell that size measures, so every measurement is what it was and no layout moves. A real face never had the grid.

- ✅ **A window draws with the built-in face unless a script hands one over** *(found 2026-10-05; the user: "the built-in font is very bad")*. The rasterizer was a feature (`font-atlas`), compiled into the web build and not into the host the Python package stages, and it shipped no face. **Decided and done 2026-10-05, reversing the earlier decision** (the user: "I reverted all my decisions ... they were made for a specific moment of the development. Add rasterization to it"): `font-atlas` is a default feature, and the crate carries the face -- a subset of DejaVu Sans, 238 KB, in `assets/fonts/` with its license and the script that makes it -- which each shell loads as it starts (`Host::load_default_face`). The same text in a native window and in a page, with nothing to find and nothing to fetch; the implicit search of the system's faces is removed, and `--font` and `/gui_font` still name another. The bitmap face is the floor: a build without the feature, a host with no face loaded (every test's). `docs/decisions.md`, "A host draws with a face it carries, and antialiased".

- ✅ **Nothing is antialiased by default** *(found 2026-10-05)*. `--msaa` existed and defaulted to 1. **Done 2026-10-05, reversing the earlier default** (the user: "Agregale ... antialiasing"): `host::DEFAULT_MSAA` is 4, for the native host and the page alike; `--msaa 1` / `[gui] msaa` / `GuiBridge.msaa(1)` turn it off. The stroke hint of an outline glyph stays: four samples make a half-pixel stroke a half-grey one, and the hint makes it a pixel.

- ✅ **An editor's window was repainted thirty times a second for as long as it was open** *(found 2026-10-05, measuring what the three above cost; the user: "check the performance, optimize whatever can be optimized without losing image quality")*. Measured on the score editor's page in a browser, main-thread time as a share of one core: **17.0 % at rest and 16.5 % playing**, the same, because a playhead anchored at 0 on a transport's position declares a clock and the tick repainted every window that declared one -- whether or not the transport rolled. Three things, none of which changes a pixel:
  - **A window that shows a playhead and nothing fed live is repainted when its counter moves** (`Host::playheads_moved`, `live::tree_is_fed`), in both shells. At rest: 17.0 % -> **3.0 %**, part of it the measuring loop's own.
  - **A page keeps the fills it tessellated** (`graphics::score::Fills`): one fill to a glyph per octave of zoom, where every placement of every glyph was read from its path and tessellated on every frame. One render of the example's page, native release: 0.446 ms -> **0.094 ms**.
  - **A triangle wholly inside its clip is not walked round it**, and one wholly outside is dropped by its corners' sides (`Mesh::tri`): the profile's largest single function while playing.
  Playing, with the face and four samples on: **14.0 %** (13.0 % with one sample -- the antialiasing is within the noise; the browser's GPU process spends 11.7 % with four samples and 12.2 % with one). What is left of a playing frame, by the profile: the browser's own work for a frame of WebGL (about a quarter), pushing the triangles, uploading the whole mesh, and drawing the symbols.

- ✅ **What a playing frame still costs, and where it could go** *(2026-10-05, the analysis the measurements above end in; three of its six taken 2026-10-06, the rest under "What a whole frame still costs" below)*. In the order of what each would give:
  - **The chrome is rebuilt and uploaded on every frame though only the cursor moved.** The frame is one immediate mesh: the toolbar, the palettes and their seventy symbols, the page, the cursor. A window's mesh kept in layers -- what a `/gui_set` or a gesture changes, and what the clock changes -- would rebuild and upload the cursor's alone while a score plays. It is the largest saving and the largest change: every element draws into one `Mesh` today.
  - **A page that did not change is tessellated into the mesh again.** With the fills kept, what is left is mapping and pushing some eight thousand vertices; keeping the page's own triangles in screen space while neither the page nor its fit moved makes it a copy.
  - **A symbol's triangles and its hint are computed where they are drawn**, per frame (`font::outline::draw`): its final triangles could be kept by size, as a page's fills are.
  - **A string's width is measured character by character on every layout** (`outline::advance` and the atlas's `advance_of` each look their table up per character): a layout's hugging containers ask on every frame, and a width kept by string and size would answer it.
  - **Thirty frames a second is one number for every window**: a cursor crossing a page at a quarter note every half second does not need them all, and a rate that follows how far the line moved would draw fewer.
  - **One megabyte of atlas texture per window** is allocated whole: a page of forty small canvases holds forty. A sheet that starts small and grows, or one shared by a page's canvases, is what that profile wants.
  - Not measured: the GPU's own time. The browser's GPU process is the only proxy this pass had, and it does not see shading; `--msaa 1` is the knob if a machine shows it.

  **Taken 2026-10-06**, with "A window repaints whole for a line that moves in one view" above, which is the layers the first of these asks for:
  - **The chrome and the page are not rebuilt while only the cursor moves**: the window's picture is kept and a tick builds the live layers alone. A whole frame -- a set, a gesture, a fed element that is not on its live layer -- still builds all of it.
  - **The rate follows the line.** A tick whose live layers are where they are on screen draws nothing; the 33 ms stay as how often the question is asked.
  - **The atlas is a sheet that grows, and a window has one.** The entry asked for a megabyte allocated whole, and there was more to find: **every batch of a window held its own** megabyte and its own textured pipeline -- three to a window, four with the live batch -- and each copied the whole sheet again when the cache rasterized one glyph. The sheet now starts at 64 rows and doubles as it fills (`font::atlas`), a glyph's coordinates are texels rather than fractions so the text already uploaded outlives a growth, a window has one copy for all its batches (`paint::GlyphSheet`), and a sync copies the rows written since the last. A page of forty small canvases holds forty sheets of a few shelves; one shared by a page's canvases is not built, since each canvas is a device of its own.

- ✅ **What a whole frame still costs** *(left by the entry above, 2026-10-06)*. A frame drawn whole builds everything, and three of the costs named there are still in it:
  - **A symbol's triangles and its hint are computed where they are drawn**, per frame (`font::outline::draw`): its final triangles could be kept by size, as a page's fills are.
  - **A string's width is measured character by character on every layout** (`outline::advance` and the atlas's `advance_of` each look their table up per character): a width kept by string and size would answer it.
  - **A page that did not change is tessellated into the mesh again** on a whole frame: keeping its triangles in screen space while neither the page nor its fit moved makes it a copy.
  - **The card draws the whole window on every frame**, live ones included, and its time is not measured. An offscreen copy of the kept picture is the design if a measurement asks for it.
  - **The fed elements that are not on their live layer** -- `meter`, the live `signal` views, `progress` -- repaint their window every tick.

  **Done 2026-10-06**, measured on the heaviest window the host draws -- the score editor over two staves of 128 eighths, engraved by the real engraver, a whole frame's picture without the card (`frame::tests::what_a_whole_frame_costs`, release): **0.46 ms -> 0.28 ms**. Taking pieces out first said where it went: the page 0.18 ms (33 000 vertices), the toolbar's symbols 0.14 ms (35 000 -- the hint strokes on every edge of a small symbol), the rest of the chrome 0.15.
  - **A page keeps its triangles** while nothing it is drawn from changes (`elements::score::KeptPage`): the placement, the clip, the colors, the ink and the text's generation, and the element's own state, which every way it changes -- a set, a key, a press, a drag, a release, the focus leaving -- lets go of them. Drawing it again is a copy.
  - **A symbol keeps its triangles by size** (`font::outline::sized`), the fill and the hint strokes together, and is drawn by moving them (`Mesh::tris_at`), which checks the clip once for the shape rather than once a triangle.
  - **A character's advance is kept by size** (`font::advance_of`), emptied when a face is loaded or outlines arrive (`font::generation`). Measured, it was not where the time went -- the face's metrics are cheap -- and it is kept because a hugging layout asks it of every character of every label on every whole frame.
  - **The fed elements are on their live layer**: a `meter` is all level and draws nothing else, a forward-only `signal` view of a bus (a scope, a spectrum, a phase view) draws on it whole, and a `progress` bar does while it sweeps. A shader `canvas` that follows the clock still repaints its window: its picture is a texture its frame uploads, and a live frame uploads none. So does a live view whose picture is a texture (the waterfall).
  - **The card drawing the whole window is left as it is.** An offscreen copy of the kept picture would replace drawing its triangles with a full-window copy of a multisampled texture, which is of the order of what the triangles cost a card; and nothing measured asks for it -- the host's process is at 1 % of a core while a score plays.

- ✅ **A selection is reported in single precision** *(found 2026-10-07,
  making a loop follow a redrawn selection; fixed the same day)*. `"selection" start len` leaves
  the host as two `f32` (`gestures/nav.rs::set_selection`), and a float holds
  a whole frame only up to 2^24 of them: 5.8 minutes at 48 kHz. Past that the
  two ends a sweep reports land on every second frame, on every sixteenth an
  hour in, while the `"locate"` the same sweep ends with is a double and
  exact -- so the audio editor matches the two within that error
  (`on_start` in `clausters-apps/src/audio/editor.rs`), and a cut over a
  selection that far into a take is off by as much. The two numbers are
  doubles in the host's own state; sending them as such is the fix, with
  the readers of the event in both clients and `docs/gui-protocol.md`.
  *(Fixed: `"selection" start len` leaves as two doubles, and so do the
  other frames a hand reports from that state — the span of `"cut"` and of
  `"delete"`, and the position of `"paste"` and `"mix"`, which were floats
  for the same reason and are what the cut reads. The `min max` of a boxed
  sweep stay floats: they are values on the view's second axis and not
  frames. Neither client changes — both decode a double and a float to the
  same number — and the audio editor matches a sweep's start to its
  `"locate"` exactly, `on_start` and its tolerance gone.)*

- ✅ **The host in a page opens no MIDI input** *(found 2026-10-07, giving the notes editor its `midi_in` option; fixed the same day)*. A roll with `midi_in` paints what is played on the desktop: the native shell opens a port, feeds what arrives through the MPE reader and hands each note to every element that reads live MIDI, at the play line or on the step cursor (`host/gui/midi.rs`). All of that is in the **native shell**, so the same roll in a page is carried the prop and paints nothing — a behaviour that differs between the two hosts, which is a defect by this project's own rule and not a property of the browser: a page has Web MIDI. What it takes is the walk moved into the agnostic core (the reader, `note_of` and the delivery to the readers, which name no port), a door on `GuiBridge` for the bytes, and the page asking for MIDI access when a def holds a roll that listens — a permission the browser asks the person for, which is the one part with no desktop twin. Until then `editors/edit_notes` says so in its page.

  **Fixed as the entry said, and the walk is one.** `host/midi.rs` is the agnostic half: a `Reader` (the MPE decoder and `note_of`, bytes in and `MidiNote`s out), `Host::midi_readers`, `Host::playhead_sample` and `Host::deliver_midi`, which answers with `GestureEffect`s — so both fronts carry out a played note through the door they carry out a gesture through. `host/gui/midi.rs` is down to the port, and `host/web/midi.rs` is the page's device: `navigator.requestMIDIAccess()` asked for when the first element that declared `Needs::midi` is defined, every input listened to (the ones plugged in later too), let go when the last such element is freed, a refusal logged once. `clausters-midi` became a dependency of every target, with its `live` feature — the port — kept to the native one.

  **Two things the native front did differently by having its own copy, and no longer does.** A note painted from MIDI was emitted straight to the window's origin and never offered to a host that owns the window (`Host::deliver`), so in a host that opens an editor by itself the note went to nobody; and it was not said in the status bar, where every other edit is. Both followed from `emit_element` being a second implementation of the delivery rule, and both are gone with it.

  **A door, because a page cannot make a port.** The desktop's input is a *virtual* port, which is what lets another program route into the host; Web MIDI enumerates devices and makes nothing. So `GuiBridge::midi(bytes)` takes a message from wherever the page has one — the same event the host's own listeners post — and it is also what `tests/gui.html` plays its key through, since a headless browser has no input and refuses the permission. It is the host's binding surface and no client's: neither client has a verb for it.

  **Not verified: a real keyboard in a real browser.** Headless Chrome answers the request with "Permission to use Web MIDI API was not granted", so what ran here is the request, the refusal path and the walk from the bytes on; the listeners on a granted access have run on no device. `editors/edit-notes.html` with a keyboard plugged in is that pass.

- ✅ **The host's wasm exports the MIDI crate's C ABI, which no page calls** *(found 2026-10-07, measuring the entry above)*. `clausters-midi` declares its binding surface as `#[unsafe(no_mangle)] extern "C"` functions in the crate itself, and a `cdylib` exports every such function of every crate it links — so the host's wasm now carries `clausters_midi_write_smf`, `clausters_midi_read_clip`, the `clausters_mpe_*` handles and the rest (`clients/web/src/gui-host/clausters_gui.d.ts` lists them), with the file readers and writers they keep alive. The release wasm went from 17 563 528 to 17 819 402 bytes, 256 KB or 1.5 %, and that figure is the ceiling: part of it is the MPE decoder and the Web MIDI glue the fix needed. `clausters-core-web` links the same crate and carries the same exports for the same reason. What it takes is the C ABI behind a feature of `clausters-midi` that the two wasm crates leave off, which touches the wheel's build (`build_native.py`), the binding tests and the feature matrix — a packaging change, not a behaviour, so it is not folded into a fix. *Fixed 2026-11-02.* `no_mangle` is behind the crate's `cabi` feature, a default, so `libclausters_midi` (the wheel's, built with `live`) exports all 26 symbols as before, and the server, the GUI host and `clausters-core-web` link it with `default-features = false`. Every wasm the web package builds now exports none of them, the server's engine and offline builds included, which carried them through the server crate: the host's release wasm went from 9 342 229 to 9 173 967 bytes, the applications' build from 24 274 636 to 24 052 883, and the five generated `.d.ts` lost their twenty lines each. The feature matrix lints the crate without the feature.

- ✅ **A `nodetree` in a page never refreshes** *(found 2026-10-16, while
  building the read-back of a binding)*. The native front re-queries the tree
  of every open `nodetree` with `/group_queryTree` every 200 ms and on each
  `/node_start`/`/node_end`; the web front sends none of it, so the same
  element drawn in a page shows the tree as it was when it opened. The query
  belongs beside the read-back, which both fronts already schedule from the
  host (`host::readback`), rather than in one front. **Fixed 2026-10-30:**
  the trees, what is asked and what an answer redraws are the host's
  (`host::node_trees`), read in the shared reply path; a page asks on the
  read-back's beat and at once on a `/node_start` or `/node_end`, as a
  window does.
- ✅ **A clip's lane stacks every channel evenly, however short the lane**
  *(found 2026-10-16, closing the audio editor's folded channels)*. The
  stacked view of a take folds and scrolls (`host::bands::Lanes`), but a
  clip's body in a multitrack lane still splits its height evenly
  (`graphics::track`, `frame::spectral_bodies`), so a sixteen-channel take in
  a lane a third of the editor's height draws sixteen lines. The layout is
  there to reuse; what is undecided is who holds a clip's folded set -- the
  region, the track or the view -- and whether a lane scrolls at all.
  *Fixed 2026-11-08, both decided*: the **view** holds the folded set -- a
  box of samples is the audio editor's view placed, and its body already
  carries that view's folds -- and **a box does not scroll**, since the stack
  it sits on does and a second scroll under the same wheel is a gesture
  conflict. The waveform drew the even split while a box's spectrogram read
  the view's `Lanes` and overflowed the box; both now ask `Lanes::placed`:
  the view's folds and strips at a floor of one control and no scroll, and
  where the channels do not fit at that floor the waveform is drawn
  overlaid on one row (every channel still in it, and the track's edge
  gives the rows back) while a spectrogram shares the box evenly
  (`a_box_s_rows_are_its_view_s_lanes_without_a_scroll`).
- ✅ **A short channel row's ruler labels collide at its edges** *(found
  2026-10-16, by eye on eight channels at the row floor)*. Each row is ruled
  on its own, so at three controls tall the bottom label of one row (`-1`)
  and the top label of the next (`1`) are drawn a few pixels apart. The ruler
  could drop the end labels where a row is short, or rule shared edges once.
  *Fixed 2026-11-02, by ruling shared edges once.* A row of a stack knows
  which of its edges another row meets (`ruler::Shared`), and on such an
  edge a label that would be pushed by it, or come closer than half the clear
  space between two labels, is dropped and its tick stays; the stack's own
  edges keep theirs. The signal rows of the audio editor and the multitrack,
  and the plot's and the spectrum's channel rows, all draw through it. A
  test rules three rows three lines tall in two units and measures the
  space across each shared edge.

- ✅ **The audio editor's meter is cut at the window's right edge** *(found
  2026-10-17, in the captures of the edit-audio example)*. The level reading
  at its foot shows as `-...` and the column has no room beside its ladder,
  with or without the file list. It predates the file list: the window's
  width does not leave the meter its natural size. The meter should keep
  the width it asks for and the take's pane yield it. **Fixed 2026-10-30:**
  the row already gave the meter its natural width; the width was what fell
  short. It counted one pad of the two its body is inset by and the
  metric's ruler width rather than the strip its ladder draws, and left the
  columns, which the reading is written over, too thin for it. It now asks
  for both pads, the ladder's own strip and columns as wide as its widest
  reading.

- ✅ **Eight tracks bring the GUI's frame rate down, and the play cursor
  jitters** (2026-10-21, the user, native, in the audio editor's review).
  With eight tracks on screen the window redraws visibly slower and the play
  cursor moves in uneven steps instead of following the sound smoothly.
  Measured first, with the host's profiler ("An automatic profiler for
  the host", below), before any cause is assumed. *(Measured 2026-10-21:
  eight channels draw as two do, thirty frames a second of the live layers
  at about 0.9 ms each, so the cost of drawing is not it; the frames were
  the tick's, a 30 Hz timer the 60 Hz display does not follow, which holds
  each position for two refreshes and then three. A moving playhead is now
  drawn at the display's rate (`Host::paced_by_display`): 60 live frames a
  second while the transport rolls, none once it stands, and the host's CPU
  while playing from 4.5% to 7.6%. Native eye pass 2026-10-21: the cursor
  follows the sound evenly; browser pass the same day, the same.)*
- ✅ **The page draws much slower than the native host** (2026-10-21, the
  user, browser). Switching the audio editor between waveform, spectrogram
  and both, and switching files, is visibly slower in the page than natively.
  The host is one program compiled twice, so the difference is a defect of
  the browser front or of what it is fed, to be measured with the same
  profiler on both fronts. *(Measured 2026-10-21, nine view switches, both
  clients sending the same six `/gui_def` and eighteen `/gui_set`: a frame
  costs about a millisecond on both fronts, but the page draws 52 whole
  frames to the native host's 18. Each switch redefines the window, and the
  reconcile carries no buffer-backed summary across a redefine
  (`SignalElement::keep_bulk` refuses a `buffer` source), so the page asks
  the engine again for every drawn buffer's shape and peaks -- 9
  `/buffer_query` and 54 `/buffer_peaks` replies -- and each reply redraws;
  the native host reads the same samples from shared memory at once. The fix
  is carrying a buffer's summary across a redefine where the buffer and its
  contents are unchanged, which is exactly where a stale picture comes from
  (the spectrogram entry below), so it is decided with that one. *Fixed
  2026-10-21*, in three causes found one under the other: a layer change
  (texture on) re-read a resolved buffer although both pictures are the
  same resource; the reconcile stopped at a container the client numbers
  afresh on every def, so nothing inside it was carried; and nothing
  carried a buffer's summary at all. A widget naming the same buffer now
  keeps what the host loaded, until a write is announced or the owner says
  `reload`. Nine switches draw 27 whole frames in the page instead of 57 --
  the rest are the spectrogram lane of the both view, a widget that is new
  each time -- and 18 natively, as before; an edit, a save and an undo leave
  the picture equal to the original's.)*
- ✅ **Two of the page's dialogs draw wrong** (2026-10-21, the user's
  captures of `edit-audio.html`). A dialog that reaches the meter is drawn
  under it: the meter's ladder and column cover the dialog's right edge. In
  the Save as dialog the OK button's label is drawn over another label (it
  reads as overlapping glyphs), and the path field shows a stray green bar
  inside the text where the caret is. Both are host drawing, so the native
  window is checked for the same before either is fixed in one front.
  *(Fixed 2026-10-21 for the meter, reproduced in the page at a narrow stage:
  a batch draws all its flat geometry and then all its glyphs, and the scrim
  and the dialog shared the batch that holds the window's overlays, so the
  window's text read through the dialog. The two now take the batch after
  it, in both fronts. The OK label and the green bar did not reproduce; text
  of the window through the dialog fits both. Native eye pass 2026-10-21: a
  Save as dialog over the meter in a narrow window is whole, the field
  focused shows no stray bar; browser pass the same day, where it was
  seen, the same: nothing reads through, OK is clean and the field has no
  bar.)*
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
- ✅ **A hidden native window on Wayland is still ticked** (2026-10-21, the
  `X23` native pass in `crates/clausters-apps/PLAN.md`). The host stops
  ticking, reading back and querying for a window the platform reports
  occluded, but the windowing library (winit 0.30) reports occlusion on X11
  and never on Wayland. A minimized playing editor there keeps its thirty
  ticks a second; the compositor already withholds the frames, so the
  draws stop while the ticks do not. A window whose frame requests go
  unanswered is the signal Wayland does give. **Related:** "An automatic
  profiler for the host", below. *(2026-10-21: a window whose oldest
  frame request has waited past 500 ms now counts as hidden -- no tick, read
  back or query -- and is drawn whole when the compositor answers again. A
  visible window on Wayland is unchanged. Eye pass 2026-10-21: a playing
  editor minimized and restored keeps sounding and draws right on its
  return.)*
- ✅ **An automatic profiler for the host** (2026-10-21, the user, after the
  audio editor's review). A run that measures, without someone watching,
  where the host's time goes, natively and in a page: frames and what each
  redrew, layout and upload cost, data paths (taps, bulk, streams) and the
  queries the fronts schedule. The eight-track window, the page slower than
  native and the stale spectrogram above are measured with it before any
  cause is assumed; it runs beside the server's. **Related:** "An automatic
  profiler for the server" (root `PLAN.md`, Found by use). *(Done
  2026-10-21: every drawn frame's cost by phase -- layout, build, upload,
  encode, present -- with its uploaded bytes and placements, counted in
  `/gui_activity`, and `host.profile(id, seconds)` / `editor.profile` with
  `format_window_profile` in both clients.)*
- ✅ **The Open file chooser gives its label too much of the width**
  (2026-10-21, the user, in the browser pass over `edit-audio.html`). The
  label beside the file browser in File -> Open file... takes more room than
  it needs; it should narrow and give the width to the browser. The room it
  frees is where a preview of the selected file could go later, which is a
  design of its own and not part of this fix. **Fixed 2026-10-30:** a
  caption beside a chooser is as wide as its text, and the chooser takes
  the rest of a packed row as wide as the caption column and the chooser
  were (`clausters_apps::forms`), so the dialog keeps its width.

- ✅ **A playhead looping over a selection moves at the tick, not the
  display** *(found 2026-10-22, the eye pass over the audio editor's
  lifecycle example)*. Looping the whole take, the editor's window draws its
  playhead about 48 times a second, paced by the display; looping a
  selection, 30, one frame per tick, and the frames the display asks for in
  between find the playhead where it was (`/gui_activity`: `live` 60 and
  `still` 61 over two seconds). The position a frame draws from seems to
  advance only when the tick reads the transport while a loop span is set.
  Measured with the fix for the core it spun (`Host::asks_next_frame`); the
  cause is not established.
  *Fixed 2026-10-22:* the selection was not the cause. The engine publishes
  a transport's position once per audio block (47 a second at a 1024-frame
  period), so with the whole take or a selection alike two frames in five
  found the cursor unmoved and asked for no next frame. A rolling counter is
  now carried between blocks (`clocks::Glide`): 120 of 120 frames move over
  two seconds of either loop, and a stopped window draws none.

- ✅ **A page shows an edit of a short take a second and a half late**
  *(found 2026-10-22, the browser pass over `editors/audio-lifecycle.html`;
  the user: a stereo paste takes two seconds to appear)*. The edit itself
  lands in the page's engine in about 70 ms, as natively. What a window then
  does is read the new join: a take under `WHOLE_DOWNLOAD_SAMPLES` is
  downloaded whole, one `BUFFER_CHUNK` (4096 samples) per request, one
  request in flight per buffer and each gated by a frame (`host/fetch.rs`).
  Measured after pasting two seconds of stereo onto two: 94 requests, 1.5 MB,
  landing at about 75 a second, so the picture settles about 1.6 s after the
  paste; a native window maps the samples and shows it at once. The chunk and
  the bound are the shared ring's (64 KiB, lossy under pressure), so the fix
  is a decision about the page's carrier -- more requests in flight within
  that bound, a wider reply for a page's own in-tab engine, or reading the
  span under the edit rather than the whole take -- not a constant to raise.
  *Narrowed 2026-10-22:* a wider reply is not open to a page either -- its
  in-tab engine is the same embedded server with the same 64 KiB reply ring.
  Three chunk replies are now kept in the air over every download and span
  (`CHUNKS_IN_FLIGHT`, the fewest-out conversation asking first), which in
  headless Chrome took the same paste from 3.1 s to 1.9 s. What is left is
  the page engine's pace: it answers once per audio callback, about 40 ms in
  headless, and its ring holds three chunks per answer, so the next step is
  that ring's capacity in a page, or reading the span under the edit. The
  browser pass the same day saw the stereo paste land somewhat sooner, and
  still not at once.
  *Fixed 2026-10-22:* every take is drawn from its summary, whatever its
  size, and samples are read only under a zoom (`docs/decisions.md`, "A host
  that cannot map draws every take from its summary"). The same paste is
  eight requests, 59 kB and about 120 ms in headless Chrome.

- ✅ **The GUI host's library suite stopped once on a SIGSEGV** *(found
  2026-10-22, closing the entries of the audio editor's acceptance)*. One
  `cargo test --lib` in `crates/clausters-gui` ended with signal 11 and no failing
  test named; the next two runs and fifteen runs of the same test binary
  passed. The output named no test, so neither the case nor the cause is
  known. What is open is the next failure's output, run with
  `--test-threads=1` to name the test.
  *Closed 2026-11-02, with the entry below:* the same suite, the same signal
  and no test named is the crash found there, whose cause is known now.

- ✅ **An edit of a long take costs a native server with a segment a pass
  over it** *(found 2026-10-22, measuring the summaries of G44a)*. A server
  that shares its buffers copies every join an edit makes into a region a
  peer maps, and builds the region's overview: 183 ms for ten minutes of
  stereo, from 545 ms before the copy went part by part
  (`tests/summary_cost.rs`). The summaries no longer need it, but two peers
  do: a host maps the region to draw the take, and a player server attached
  to the segment plays the join out of it. Both read a join flattened where
  its sources are already mapped. The direction: publish a join as its parts
  over the mapped sources, and have each peer stitch or compose it, so that an
  edit costs its parts on every server.
  *Taken by the turn to takes on disk (2026-10-22): a take stops being a
  pool buffer, so there is no join to copy; it closes with `S23`, which
  removes the copy.*
  **Closed 2026-10-30, with nothing left to fix:** `S23` took the server's
  joins away whole (`/buffer_stitch`, `/buffer_parts`, the stitched
  storage, the overview file beside a shared region and the cost test
  that measured it), so no edit makes a join on a server and there is
  nothing to copy into a region a peer maps. An application reads, joins
  and measures its takes itself, on its own engine.

- ✅ **A list of segments over files draws no spectrogram** *(found
  2026-10-22, building `G45`)*. A `signal` naming `segments` is drawn from
  its files' peaks and, under a zoom, their samples; its `spectrogram` layer
  and the `spectrogram` view over it draw nothing, since an analysis is made
  from samples and a list is never read whole, and a stored transform of a
  file does not compose across a seam or a fade the way peaks do. The audio
  editor draws a spectrogram of its take, so `X28` decides it: an analysis
  file kept beside each file's peaks and composed where the parts keep their
  file's hop, the span under the eye analyzed from the samples, or both.
  *Decided and done 2026-10-22, opening `X28`:* the span under the eye,
  analyzed from the samples, as the field does for long files (a fast preview
  above a visible length, accurate zoomed in). A view of a list asks
  `/buffer_spectrum` of the host's own answerer for one window per column of
  the span it shows, read through the parts, and asks again when it moves
  past it or zooms to another hop (`host::columns`, `frame::columns_owed`);
  the transform of a span carries its origin (`Stft::of_span`). Over an hour
  of stereo, 760 columns of 2048 frames read 37 MB in 87 ms natively
  (`tests/long_list.rs`); a page draws one (`segments-host.html`). A stored
  analysis is this plan's Future directions, "A list's spectrogram kept beside
  its peaks".

- ✅ **A spectrogram of a server buffer may draw nothing in a host that
  cannot map it** *(found 2026-10-22 reading the path, closing the entry
  above; not yet seen on screen)*. Since every take is drawn from its summary
  in such a host (a page, a remote server), a `spectrogram` over a `buffer`
  is owed the take's summary like a waveform, and a summary is peaks: the
  texture slot is never filled, where it used to analyze the samples it
  downloaded whole. `tests/editor.html` draws one and asserts only that its
  capture is not blank, which the waveform beside it satisfies. What is open:
  see it in a page, and if it is blank, give a buffer's spectrogram the
  columns a list's has, which needs the server to answer `/buffer_spectrum`,
  or read the span it shows back as samples and analyze it in the host.
  *Seen 2026-10-28 by the user in `views/layers.html`:* the texture is
  blank once the take is drawn, only the peaks and the RMS show; zoomed in
  with the wheel a texture appears, but over the wrong times (a sweep
  starting at zero drawn from about a quarter of the take to past
  three quarters), so the window read back for the zoom is analyzed and
  placed as if it were another span. Both halves are the fix.
  *Fixed 2026-10-28, checked in a page headless.* The zoom was not
  misplaced: the run read back was analyzed with silence around it, so only
  that run showed. A view that draws a texture over a summary now asks the
  run around what it shows at any zoom (`frame::texture_owed`, at most
  `TEXTURE_SPAN` frames about the view's middle), the run alone is analyzed
  and drawn where it lies (`Stft::placed_at`), and a server buffer's texture
  asks no columns, which only a list answers. `views/layers.html` draws the
  whole sweep as it opens and keeps it aligned under a zoom.
- ✅ **The audio editor's view of a take glitches under zoom and pan**
  *(found 2026-10-24, by eye on the audio editor the host owns,
  `clausters-gui --audio`)*. Zooming in makes the picture flicker; at the
  level of samples the sample circles are drawn or not depending on where
  the zoom lands; panning draws what comes in at the edges erratically, as
  if from frames further on. After a paste, an undo and another paste, zoom
  and pan stopped answering: the view stayed one fixed length while the
  take played on past it. Suspects to check first, before guessing (an A/B
  build against the commits before them): the view a paste opens out
  (`AudioEditor::show_span`, carried by the commit's `viewport`), the
  extent every list now registers as it is named (`sync_lists` calling
  `Host::set_take_total`), a view kept longer than a take an undo shortened,
  and the `"view"` event carrying its start and length as `f32`. Whether the
  flicker and the edges predate the host owning the editor is part of the
  finding. *Fixed 2026-10-24, reproduced with captures over a minute pasted
  to seventeen hours; none of it was new with the host owning the editor.*
  The fetch machine asked a summary and a run of samples with frames cast to
  `int32`, so past about twelve hours (six, a stereo take's flat index) the
  summary walk wrapped and never finished; and the axis took the length of
  the picture still drawn from before an edit, which then stayed when the
  new picture never came. A pan revealed the take's coarsest summary at the
  edges (a grid was asked for the view alone, and replaced on every frame the
  hand moved), a zoom in fell back to it at every step, and samples were
  drawn as zeros past the run held. Separately, an undo left the cursor past
  the end of the shorter take, and the next paste opened the view from there.
- ✅ **The play cursor overshoots a short loop on screen** *(found
  2026-10-24, by eye on the audio editor the host owns)*. Looping a span as
  short as one cycle, the drawn play cursor runs past the selection's end
  before it wraps. The sound is right; the sweep is extrapolated from the
  engine clock between readings and not wrapped at the loop's end in the
  drawing. To check: the wrap the timeline chrome applies
  (`playhead_loop_*`) against the engine's loop for a span shorter than a
  frame of the screen.

  **Fixed (2026-10-24).** The host carried a transport's position on by up
  to a block between readings and knew nothing of its loop. Each transport
  now publishes the pass it is on beside its position (rolling, the loop,
  the end mark), the host carries its clock, which only advances, and
  places the position on the pass.
- ✅ **Time is a type of its own, 64 bits wide** *(asked 2026-10-24 by the
  user)*. Frames, positions and spans in the host and the applications are
  bare `f64`/`usize`/`u64`, and a conversion to 32 bits slipped in three
  times in one day (an `i32` frame on the fetch wire, the `"view"` event
  and the sample dots' spacing in `f32`), each found only by eye on a take
  of hours. As the server does for its clocks (`server::clock_axis`), a
  time type in `clausters-core` that converts to a pixel only relative to
  a view's origin, and to the wire only as 64 bits, makes the mistake
  unwritable.

  **Decided with the user (2026-10-24): the type counts NTP ticks.** A
  `Time` is an `i64` of 2^-32 s, the fraction of an OSC timetag: 233 ps
  and 68 years either way of an axis' origin. A time on an axis plus the
  timetag of the origin is a timetag, with nothing lost, and takes at two
  rates share one axis without a scale. Samples stay whole frames, a
  sample index being a different quantity: a time becomes a frame only
  where samples are read, with the rate of what is read. A beat axis is
  musical time and stays `f64` beats until a tempo map turns it into
  `Time`. The record is `docs/decisions.md`, "Time is NTP ticks on every
  axis".

  The pass, each step landing green and committed:

  - ✅ **T1, the type.** `clausters_core::time::Time`: ticks, seconds,
    frames at a rate (rounded, floored, ceiled, exact in integers for an
    integral rate), a timetag since an origin and back, sums and
    differences in integers, and a pixel only as a difference from an
    origin over ticks per pixel. Rust only: the host is its first reader,
    and the bindings land with T3, where both clients first build a time.
  - ✅ **T2, the host's time axis.** A timeline group's window, selection,
    cursor, playhead, sweep anchor and loop are held as `Time`, at the rate
    the group's members speak (a pane's or a roll's `sample_rate`, else a
    take's own). A member reads them in frames of that rate, since what it
    draws is samples, and a whole frame crosses in and out unchanged. The
    wire is unchanged at this step and converts at the group's door with
    the same rate.
  - ✅ **T3, the wire.** Decided with the user the same day: the clients'
    surface stays in frames, and a time crosses the wire in frames, **never
    narrowed**: a whole frame as an `int64`, a fraction (a window's edge) as
    a double. Ticks would have needed every view's rate on both sides, and a
    client does not know a file's rate before the host reads it. The host
    reports a selection, a cut, a copy and a locate as longs; both clients
    send a fractional prop as a double and an integer past an `int32` as a
    long, where Python sent a float as `f` and refused an int past 2^31 and
    the page sent both narrowed; the host reads a long past an `int32` as
    no `int32` instead of wrapping it, and answers a query's fractions as
    doubles.
  - ✅ **T4, what is left.** An audit of the host and the applications for
    a time cast narrower than 64 bits before it is a difference from an
    origin, each fixed or named. `docs/app-behavior.md` gains the rule: a
    time crosses the wire whole and becomes a pixel only as a difference
    from the view's origin.

    **Done (2026-10-24).** The pixel maps already took the difference in
    `f64` before narrowing. What the audit found was on the wire: a
    breakpoint's time went as a float, a stitch of buffer parts sent its
    start and length as `int32`s, which wrap at twelve hours, and
    `/buffer_touch` and `/buffer_setRangeChannel`, whose server side counts
    in `int32`, were sent a span past it wrapped; the time is a double now,
    a stitch's counts go as longs past an `int32` through one helper
    (`clausters_editing::apply::count_arg`, which had been written three
    times), and a span past an `int32` is refused with a warning instead of
    landing elsewhere. A sample rate as a float, a fade's length and a
    tap's frame count are not times of a take and stay as they are.

- ✅ **A long list's spectrogram does not follow an edit, and a zoom
  distorts it until it settles** *(found 2026-10-25 by the user, editing a
  stereo take of five minutes in the audio editor's spectrogram view, after
  the file's columns were kept by the answerer)*. On a short take the
  edited spectrogram is there within a frame or two; on a long one, after
  a cut, the part of the take past about two minutes and a half is drawn
  as one column stretched to the end, every bin a horizontal line, and it
  stays so. Zooming in or out draws the picture distorted (columns of the
  previous level stretched over the new axis) until the new columns land.
  *Closed 2026-10-25.* The first was the length: the view kept the
  columns of the list before the edit and asked for the new one's over the
  take as long as it had been, so a paste's gain was never asked for. The
  second was what a view drew past its columns: the span it held, its edge
  columns stretched. A list's view now holds the whole take, about a column
  to the pixel, under the span shown, draws each only where its columns
  reach, and after an edit asks for the whole take again first, at its new
  length (`frame::columns_owed`, `docs/app-behavior.md`).

- ✅ **The note axis is a log axis, not one over the notes' range**
  *(found 2026-10-26 by the user, looking at the audio editor's
  spectrogram on its note scale)*. The note scale maps like the log scale,
  with its floor at C0 of the tuning, so it spans the log axis's range and
  not the notes'. The user's rule: the vertical in notes is linear in
  pitch over the MIDI notes 0 to 127 -- each octave the same height --
  from about 8 Hz (note 0, infrasonic) to about 12.5 kHz (note 127, above
  the fundamentals music uses), so the bins of the analysis are mapped
  onto that range. What is open is the mapping a bin takes onto it (how a
  bin wider than a note at the bottom, and many bins to a note at the top,
  are drawn), chosen among mathematical options the user asked for,
  before the shader and the ruler (`spectrogram.wgsl`, `host::ruler`
  `note_ticks`) are changed.
  *(Closed 2026-10-27: the axis spans MIDI 0 to 127 at an even step
  (`ruler::axis_floor`, the shader's fifth scale), and a bin is drawn as
  the block it measured, the rule every scale now follows.)*

- ✅ **A number field's drag crosses its whole range in a few dozen
  pixels** *(found 2026-10-26, dragging the spectrogram's tuning field)*.
  A `number`'s drag is a knob's (`control::Dial`), which spreads the range
  over a few heights of the body, and a field's body is one line tall: 40
  pixels took the tuning from 440 to 1000 Hz. A field is read to a step,
  so its drag wants a rate a hand can stop on (the step per pixel, or a
  fixed run of pixels for the range, with a modifier for fine), looked up
  in how scrubbable fields behave before it is chosen.
  *(Closed 2026-10-27: a step a pixel, Shift ten and Alt a tenth, the
  scrubbing fields' convention (`control::Scrub`).)*

- ✅ **A spectrogram's first look at a span of 20 s to a few minutes
  analyzes every window under it** *(found 2026-10-27, measuring a zoom
  over a four-minute stereo take)*. Under the file's spectral summary,
  whose finest column stands for 8192 frames, a view's detail is pooled
  from the analysis's own windows, every one of them in the span shown:
  a view of 20 s to about 170 s at a thousand pixels is up to some 16 000
  windows a channel, which comes as a burst of about 250 ms of one core
  the first time a span is seen (the answerer's column cache makes every
  later look free). Finer levels in the summary would read those views
  with no sample at all: columns of 2048 frames start the analysis only
  under views of about 40 s, at four times the summary's size (some
  170 MB for an hour of stereo against 43). It is a change to a format
  the server writes beside its peaks (`clausters_core::spectra`, its
  version), so the size is weighed against the burst before it is made.
  *Fixed 2026-10-28, without changing the format:* the work is spread
  rather than shrunk. An answer analyzes at most 128 windows
  (`SPECTRUM_WINDOWS`, 4096 before), so the longest is about 6 ms natively
  where it was 47, over the same four-minute stereo take and about the
  same 300 ms in all; and a page, which answers on its own thread, gives
  itself a turn before each spectrum answer (`web::disk`), so the analysis
  never holds a frame back. The view keeps the picture it has until the
  columns are whole. A summary with finer levels stays the way to make the
  work itself smaller, if a first look at minutes ever needs to be faster.

- ✅ **A window the compositor does not draw asks for no picture** *(found
  2026-10-27, probing the audio editor from a script with its window
  covered)*. What a view owes (`SpectrogramSlot::owed`, the waveform's too)
  is noted by the draw pass, so a window that receives no frame asks for
  nothing: a capture of it drew the take's one dark column for seconds, and
  a change of analysis ran only when the next gesture drew a frame. Whether
  a window nobody sees should ask at all is the question (asking on
  placement would spend a read on a picture nobody looks at), and a capture,
  which is the one reader of a window nobody sees, may be what should ask.
  *Fixed 2026-10-28:* a capture asks. A window nobody sees still asks for
  nothing, and a capture's own draw notes what its views owe, asks it at
  once and writes the file once nothing more is owed
  (`Front::awaits_picture`, the same rule natively and in a page), or after
  ten seconds as it stands (`a_picture_is_awaited_until_its_summary_is_whole`).

- ✅ **A paste blinks the audio editor's picture, in a short take as in a
  long one** *(found 2026-10-28 by the user, in the native eye pass of
  `G44c`)*. The pasted span shows at once and nothing is redrawn from the
  left, but the picture blinks as it is replaced, the same over a
  two-second and a ten-minute stereo take. The other window, over another
  file, does not move. What blinks (the summary placed whole, a frame drawn
  with no picture between the old and the new) is the question.
  *Fixed 2026-10-28, measured in a page frame by frame.* The axis spans
  the new take as soon as its list is named, and the picture stayed the
  old take's until the new summary landed, so a frame or two drew the old
  picture squeezed onto the new axis. A window whose picture is of another
  length than its axis keeps its last frame on screen, up to 250 ms, while
  what it waits on is asked as a drawn frame would ask it
  (`replies::PictureHold`, both fronts); the page went from the old picture
  to the new one with nothing between. Found with it: the status line
  printed the paste's clipboard document whole; a long string is counted
  now.

- ✅ **A stopped audio editor window ticks thirty times a second for its
  playhead** *(found 2026-10-28, measuring the fix above natively)*.
  `activity()` over two seconds of a window that never played reads 61
  ticks with `demand: ['playhead']`; the lifecycle example says a stopped
  window ticks for nothing. Whether the demand is the play cursor's
  standing mark or a transport the editor reads is the first thing to see.
  *Fixed 2026-10-28.* A transport the host has heard nothing of counts as
  one that may move, and nothing reports an application's transport the
  way a server reports its own, so the editor's playhead asked for the tick
  for as long as the window was open. A window over an engine of its own
  now reads whether each transport rolls where it reads the position (the
  engine's `TransportPass::rolling`); a server's still go by what the
  server reported.

- ✅ **What a score box of the multitrack left open** *(moved here
  2026-10-28 when `N9` closed; each for when it is met)*.
  - ✅ the standalone host binds no engraver to its multitrack, so there a
    box of notes is a roll whatever the menu says;
    *fixed 2026-11-07*: the host binds one wherever its build links an
    engraver (since `a186abd6`), and a build that links none tells the
    multitrack so (`refuse_pages`): the View menu's *Notes as scores* is
    refused with why, and the view stays the roll
    (`a_host_with_no_engraver_offers_no_page_and_says_why`);
  - ✅ the box is read with the transcription's defaults, and nothing on the
    multitrack states another meter, key or smallest value for it;
    *fixed 2026-11-08* (`crates/clausters-document/PLAN.md`, `O35`): a box
    is read with the reading its sequence stores (`notation.reading`,
    `Transcription::of`), as a score editor over the sequence is;
  - ✅ a page is engraved whole when its sequence changes, and what that
    costs for a sequence of thousands of notes is not measured;
    *measured 2026-11-07* (`what_a_box_page_costs`, release, one box):
    a thousand notes engrave in about 190 ms and five thousand in about
    2.3 s, on the application's thread, after every edit of the sequence;
    a picture with nothing changed costs 1.8 ms and 12.7 ms against 0.6 ms
    and 2.4 ms as rolls, almost all of it copying the page (some 2 MB of
    display list at five thousand notes), which every picture hands over
    and the host parses again. The comparison that decides whether a page
    is stale no longer copies the sequence, and the anchors are kept with
    the page. **What is open**: a page engraved and stated only where a
    sequence changed -- by system or by a span of bars, which is the score
    editor's kept parts in `G46` seen from the engraving side -- and the
    `scores` prop restated only when a page moved.
    *Closed 2026-11-09 without the change*: engraving by parts was measured
    and left (`N10`, "Not taken"), since the project follows verovio rather
    than replacing part of its layout (`docs/decisions.md`, "Verovio is
    followed, not adapted"); the user then took the boxes out of the
    multitrack, for now (`crates/clausters-apps/PLAN.md`, Future
    directions, "Whether the multitrack keeps its score boxes");
  - ✅ a rest has no id on the page, so it stands where the stretch between
    its neighbours puts it rather than at its own time;
    *fixed 2026-11-07*: it had an id (`n<id>`, as a note's) and no anchor;
    the multitrack anchors every rest before a voice's last note, and none
    after it, which fills a bar nobody wrote
    (`a_box_of_notes_is_stated_as_its_page_once_an_engraver_is_bound`);
  - ✅ the box's label and the meter over the first note share the box's top
    left corner.
    *fixed 2026-11-07*: the page starts under the name's plate, which
    `track::clip_label_reach` measures as the plate is drawn
    (`a_box_s_name_reaches_as_far_down_as_its_plate`).

- ✅ **The audio editor in a page: the pointer lands off the picture, and the
  body draws a dark column and stray ticks** *(found 2026-10-28 by the user,
  the page half of `G44c`'s eye pass, `editors/audio-lifecycle.html?isolated`
  in Chrome)*. The two windows open below the page's prose, and a gesture
  does not act where it is drawn. The picture shows a dark column across
  the take at about 0.42 s with a dashed line of short ticks beside it, and
  the second window, zoomed in, a column of ticks at its left edge. Neither
  is in the native windows. To find first: whether the offset is the page's
  scroll (the windows sit below the fold, and the suite's pages never
  scroll), and whether the column is a cursor, a selection or a stale
  strip of the picture -- and how this relates to "A press in a multitrack
  lands off its mark again at a density of 0.8".
  *Fixed 2026-10-28, reproduced in headless Chrome over CDP.* Not the
  scroll: an application's window gets a canvas the host appends, which
  nobody fitted, so it stayed a fixed 480 x 420 laid out at a scale of 1
  over the page's device pixels while a press was read at the page's
  density -- at 1.25 a press at 1.0 s located 0.477 s. The host now opens
  that canvas at the window's declared size and reads its density, as a
  native window does. The dark column was the playhead's box: WebGL
  resolves and presents by a blit the scissor clips, and the live layers'
  passes ended on the box's scissor, so the box came out transparent; each
  pass now ends over its whole target. `tests/app-window.html` checks the
  size and a press; Chrome skews a synthetic event under a forced display
  scale, so the press at 1.25 was checked with real input.

- ✅ **A reopened multitrack waits behind summaries of freed lists**
  *(found 2026-10-30 during the session-lifecycle review)*. Closing a window
  frees its disk lists, but pending summary requests still occupy the shared
  fetch queue. A second window draws clip frames and curves without its
  waveforms. Retire the requests when the list loses its last holder; verify
  immediate close/reopen with several clips and a stalled previous summary.
  **Fixed 2026-10-30.** List release now tells the existing fetch machine to
  retire those requests. A regression fills the summary slots, frees their
  lists and opens another window; the native load example draws every clip's
  waveform after immediate close/reopen.

- ✅ **The parallel default host suite can exit with SIGSEGV**
  *(found 2026-10-30 during session-lifecycle verification)*. A permitted
  `cargo test --offline --lib` ended with signal 11 and no Rust failure
  report. The same suite with `-- --test-threads=1` passed 1400 tests (one
  ignored). Identify the concurrent path and add a deterministic regression;
  no cause is established by the serial pass. Caught under a debugger
  2026-10-31: the fault is in `libvulkan` under wgpu's
  `set_debug_utils_object_name`, creating a render pipeline in
  `host::frame::tests::a_window_is_its_reference_picture` while other tests
  run, so a test that holds a real device is where to look first.
  *Fixed 2026-11-02.* Reproduced by running the frame tests on eight threads:
  13 runs in 40 crashed. Under the debugger the faulting thread was creating
  a device and another was in `dlclose`: the Vulkan loader unloads its
  drivers when the last instance goes, and every device was asked of an
  instance of its own, so a test dropping its device on one thread unloaded
  the driver under another's creation. A first instance is now kept for the
  life of the process (`gpu::new_instance`), so the drivers are never
  unloaded; each caller still gets an instance of its own, since sharing one
  made the GL backend's context current on two threads and it refused the
  second (tried, and 56 runs in 60 failed so). After it, 0 in 60 runs of the
  frame tests and 0 in 10 of the whole suite in parallel. A test opens and
  drops 48 devices on eight threads: 8 runs in 10 crashed without the kept
  instance, none in 20 with it.

- ✅ **A control's size does not follow what it says, in a row or across one**
  *(found 2026-11-01 by the user, by eye on the curve window, as something the
  examples keep showing)*. The layout has no rule for the two cases left once
  a box covers its text. **Along a row**, a button or a chooser with no `w`
  takes an even share unless the row carries `pack`, so `play` and `stop` are
  each half a window wide. **Across a row**, the cross axis fills without
  reading the child's size: a chooser or a button in a tall cell is a block as
  tall as the cell, and a chooser's body grows with it, while a row given an
  `h` below what it holds squeezes its children: a knob becomes a dot and a
  check box loses its frame. Measured on the windows of `panels/panel`,
  `panels/style`, `views/layers`, `views/bulk`, `notation/score_from_data` and
  `editors/patch1`. A row's height written as a number does not know its own
  margins either: a row in a row at 40 leaves a cell of 16. What is wrong is
  the rule and not each example, so the decision to take is whether a control
  with content keeps its content's size on both axes unless a `weight` asks it
  to stretch, what that does to every window, and whether a container's `h`
  may stay under what its children's floors add up to. Pinned today by
  `a_natural_size_only_binds_its_own_axis`, so this changes a written rule.
  **Decided 2026-11-09 by the user**, as principles: a control with content
  keeps its content's size on both axes unless it is asked to change it
  (`w`, `h`, `weight`), and a container's `h` never goes under what its
  children's floors add up to. What is wrong is the **defaults**, so what
  is open is a rule per **standard** element type (not a custom one): on
  each axis, whether it keeps its content's size, may grow when asked, or
  takes what the layout gives -- the kind of per-type default a toolkit
  declares for each of its widgets. The rules are chosen by looking at
  every standard element at its defaults in every layout (`row`, `col`,
  `grid`, a tall cell, a short one), which no example shows today.
  `panels/palette` shows them now (2026-11-09).
  **The rules, decided with the user on 2026-11-09** after looking at the
  palette and at what a browser does with unstyled form controls (measured
  in headless Chrome: a flex item keeps its `min-content` width along the
  main axis, so a range input does not go under 129 px and overflows a
  narrower box; every control stretches across the cross axis and fills a
  grid cell; and an explicit `height` wins over the content, so a row of
  8 px squeezes its controls to 8 px and their text spills out -- the one
  thing this host must not do, which a toolkit whose layouts honour each
  widget's minimum does not do either). Three layers are kept apart: the
  **defaults** must be correct (nothing overflows, everything reads), not
  pleasing; the **capacity** is the props an author has to change them;
  the **design** is what an author chooses with those props.
  - **A floor is a size every container keeps.** Each element declares the
    smallest size it still reads at (its `floor`, which a knob already has:
    it gives up its label strip and keeps its disc and read-out). A
    container's floor is derived from its children: across its axis the
    largest child floor, along it the sum of the child floors and the gaps,
    plus its margin on both sides and its title strip. No container places a
    child under its floor, and a declared `h` or `w` resolves to the larger
    of itself and the floor: a row declared 28 high is as high as its
    tallest child's floor. A declared size is the outer size (title strip
    and margin included), so a row of 40 inside a row no longer leaves a
    cell of 16.
  - **The root's floor is the window's minimum size**, natively and in a
    page, which also ends a window made one pixel wide.
  - **Filling a cell is a right default.** A button that takes all the
    room its row or its grid cell gives it is the step sequencer's grid of
    push buttons; that it looks too large in a panel is a design question,
    answered by the capacity below and never by a default that hugs.
  - **The capacity grows by a maximum and an alignment.** `max_w`/`max_h`
    beside `w`/`h`/`weight` (and `min_w`/`min_h` if a floor larger than the
    element's own is wanted), and an alignment inside the cell for an
    element its maximum keeps smaller than the cell.
  - **Text sets no minimum width.** A line that does not fit is cut with an
    ellipsis, as the host's single lines already are, and its whole text is
    shown on hover. The convention is a tooltip (a page's `title`, a
    toolkit's tooltip); a marquee is the other option and is a design
    choice still to make.
  - **A control with geometry does set one**: a knob the larger of its disc
    and its read-out, a slider its grip and a least useful travel, a number
    the width of its widest value (an ellipsis on a number would show a
    wrong number). A text field may be narrow, since it scrolls its
    contents.
  - **The defaults per standard type**, as a starting table to check on the
    palette: label (one line high; ellipsis wide; fills), button (a line and
    its padding high; padding and an ellipsis wide; fills), toggle (its box;
    its label in ellipsis), choice (its body, and its label strip when it
    has one; its arrow and an ellipsis wide; fills), number (its field; its
    widest value), knob (disc and read-out, the label strip given up first;
    the larger of disc and read-out; the disc centred in what it is given),
    slider (its thickness, fixed; its grip and least travel; along its
    axis it fills), text (its field; a few characters), progress and
    separator (their thickness; along their axis they fill).

  `a_natural_size_only_binds_its_own_axis` pins the old rule and goes with
  it; the palette is the manual test, and each container shown there is
  checked against the rules above.
  *Done 2026-11-09.* `Widget::min_size` is every widget's floor on both
  axes, composed for containers; the strip, the grid and `free` never place
  a child under it, a share is held at it, and a declared size or a maximum
  under it resolves to it. The root's floor is the window's minimum, told to
  the platform by both fronts, and a window declared smaller opens at it.
  `max_w`/`max_h`, `min_w`/`min_h` and `anchor` are the new place props. Two
  things the eye pass over the applications corrected on the way: a caption's
  floor is its text without its padding (with it, the score editor's symbol
  palettes and its toolbar were wider than the window they had always fit in),
  and a knob gives up its caption before its disc, whose floor grows with the
  knob's own text. `a_natural_size_only_binds_its_own_axis` still holds --
  filling a cell is the default -- and the palette shows the floors and, in
  its sixth container, a maximum and an anchor. The multitrack and the audio
  editor draw as before; the score editor's toolbar now fits its window at
  960 (its transport used to fall off the right edge).

- ✅ **A scroll's plane is no wider than its pane, whatever its children's
  floors** *(found 2026-11-09, closing the entry above, on the score editor's
  palette column)*. A `scroll` that flows its children in a column takes its
  pane's width as its content's, so a child whose floor is wider -- the
  ornaments grid, whose widest symbol floors a few pixels past the column --
  is drawn past the pane and cut by it, where the rest of the layout now
  never cuts a floor. The plane's extent across its flow should be at least
  its children's floors, which makes such a column scroll sideways; whether
  that is the fix, or the palette's cells are, is to decide.
  *Decided and done 2026-11-09, by the user: the grid is fixed, so the
  symbol shrinks to fit it.* As a rule rather than for one symbol: a caption
  that is one outline character shrinks where a word is cut, and the lone
  symbols of a container share **one** fit, the one at which the widest fits
  its box (`layout::symbol_fit`, `Ctx::text_fit`), never under the host's text
  size, which is also their floor. So the palette's symbols keep their sizes
  against each other however many are added, and the score editor's toolbar
  shrinks its symbols together rather than running past the window. The
  scroll's plane is unchanged.

- ✅ **A pass can play with no play cursor until a frame happens to come**
  *(found 2026-11-01 by the user, by ear and eye on `notation/score_editor`)*.
  After many actions in no particular order a play sounds and the page shows no
  cursor; when it does appear it stands on the note sounding, so the position
  is right and what is missing is the frame. A log of every frame showed it:
  `draw_live` was not called for 5.8 s while the transport's clock went from
  144000 to 423040, and for shorter stretches (0.4 to 1.4 s) at other plays. So
  the window is not asked for live frames while it plays, and the place to look
  is what asks: the window's demand (`Host::playhead_ticks`, which reads the
  source's `pass.rolling`), the tick in `gui/app.rs` and `asks_next_frame`. Not
  reproduced by a script, and a capture cannot show it: it draws the window
  whole. Find the cause before changing any of them.
  *Fixed 2026-11-02.* The cause was the order of two things: a run that
  plays ends, and its window is told, before the engine's next block takes
  the command, so the transport still read stopped when the front asked what
  the window needed (`playhead_ticks` reads the engine's `pass.rolling` raw).
  Finding nothing, the front slept, and only another event woke it; when the
  application happened to say something else a block later the cursor came,
  which is why it was intermittent and why a script, which asks again at
  once, never showed it. A window over an application's engine now keeps its
  playheads' tick for a settling after each answer of its application
  (`Host::settle_window`, `SETTLE_APP`), as one over a server's transport
  does after a push, and gives it back when no play lands. A host test plays
  a roll on an engine nothing processes: the demand was empty after the run
  and is `Playhead` now, until the settling ends.

- ✅ **A score that writes its meter, key and clefs on each staff is read
  in 4/4 and C** *(found 2026-11-09 by the user, opening an MEI exported by
  a notation program in the score editor: the opening repeat barline was
  missing)*. The reader takes the meter, the key and the clef from the
  `scoreDef` alone; MEI 5 writes them on each `staffDef` (`meter.count`,
  `keysig`, a `<clef>` child), and the engraver keeps them there in the MEI
  it hands back. So a zamba of 32 bars in 6/8 with one flat came back as 24
  bars of 4/4 in C, and every edit and save wrote that. The same pass lost:
  - **a left barline** (`left="rptstart"` on the measure a repeat opens): the
    model keeps only each measure's right barline, so a repeat that opens
    the score has nowhere to be, and one written as the right barline of the
    measure before is drawn at the end of a system where the engraver's
    convention draws it at the start of the next (it splits only `rptboth`
    at a system break);
  - **the instrument's name** written on the staff group (`<label>`,
    `<labelAbbr>` on a `staffGrp`), which the reader looks for on a staff;
  - **the running head the engraver writes** (`<rend type="subtitle">`,
    `type="poet"`, `type="composer"` inside `pgHead`): the title stays, the
    subtitle, the poet and the composer go.
  To check with them: an empty `<verse>`, which carries nothing and may
  stay dropped, and the mid-score `<clef>` changes, which may be the same
  gap. The repeat barline in the model is a design: where a left barline
  lives, and whether a start repeat is written as a measure's `left`
  everywhere, which is how the engraver encodes and draws it.
  *Fixed 2026-11-09.* The meter and the key are read where a score
  definition states them or on its first staff's (`read::stated`); the
  clef was already read from a `<clef>` child. The staff group carries its
  instrument's name (`Group.label`, `Group.abbr`, written as the group's
  `<label>`); the document head's people are read by role (`persName
  role=...`), and the engraver's typed running head fills what the head
  leaves empty. **A start repeat is a measure's left barline**: the grid's
  `(measure, "rptstart")` is the measure the repeat opens at, written
  `left="rptstart"` as MEI and the engraver write it -- so one can open the
  score, and one at the start of a system is drawn there -- and read back
  from either side (a `right="rptstart"` opens the next measure); the
  decision record says why ("A start repeat is the left barline of the
  measure it opens"). The score editor's Repeat start opens at the first
  selected measure; the other barlines still end the last. The empty
  `<verse>` stays dropped, and the file holds no clef change. The zamba
  reads as 32 bars of 6/8 in F with its repeat, its piano's name and its
  head, writes back the same, and the engraver says nothing about it
  (`a_score_written_staff_by_staff_reads_whole`).
