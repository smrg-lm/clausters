---
name: clausters-gui
description: How to work on the Clausters GUI track (clients/gui) — the scriptable widget protocol (a GuiDef is JSON-in-OSC, the /gui_* vocabulary, bindings, standalone), the host's structure (platform-agnostic core + native/web fronts behind small traits), the reusable wgpu/WGSL rendering machinery (viewport/peaks/Stft and the "never resolve finer than the screen" rule), the data paths (shm buses, /c_stream, mmap/fetch bulk, buffer fetch), and the clausters.gui Python submodule that drives it all. Consult before touching anything under clients/gui or building any GUI widget/view.
---

# Clausters GUI track (clients/gui)

A system of graphical elements **built and driven from a dynamic language at runtime** - the way sclang builds Qt widgets - not a GUI compiled into the audio server. The design rationale and the staged milestones (`Gx`) both live in `clients/gui/PLAN.md` (a settled summary of the rationale is published in `docs/clients.md`, "The GUI host: a scriptable peer"). This skill is the working map; that file wins on any detail, and `Gx` labels live only in that roadmap (never in published docs or this skill's prose). Per-widget domain knowledge (scopes, editor-grade views, BPF, edit-back-to-data, the widget extension recipe) is in [[gui-widgets]].

The crate is an **independent workspace** - build and test from inside `clients/gui/` (`cargo test`, `cargo run --bin clausters-gui`), never from the repo root, so it can never entangle the core server build. It links `clausters-core` (a path dependency pulling only `rosc`, never the server crate) for the shared OSC seam; the optional `standalone` feature additionally links the `clausters` crate (`embed,realtime`) for the embedded server, off by default for size.

## Architecture in one screen

`clausters-gui` is **two roles in one process**:

- a **GUI server (host) for the languages** - owns windows, widgets and the GPU, exposes a widget protocol;
- a **client of the audio server** - reads buffers/buses/node tree and sends control, exactly as [[clausters-python]] does.

Three legs: script <-> audio server (existing OSC control), script <-> gui host (the widget protocol), gui host <-> audio server (gui as a client). A **bound** widget bypasses the script: its value flows straight from the host to the audio server (low-latency), the same idea as MIDI bindings in the server.

The protocol shape:

- **OSC is the single encoding everywhere**, through the one `clausters_core::osc::decode_packet` door. The native host front is UDP (default port 57210); the browser front rides the wasm binding surface and WebSocket - a browser can only do WebSocket, which is why the server's `WsHub` exists.
- **A GuiDef is JSON inside one OSC argument**, exactly as a `SynthDef`/`GraphDef` rides `/d_recv`. JSON is the payload, OSC is the framing. serde's number handling keeps ids `i32` and control values `f32` across the wire - preserve that int/float distinction on the client side too.
- **Construction is declarative, updates incremental.** A GUI is a *def*: build the whole tree in one message, then mutate live widgets one at a time. No per-widget construction chatter.

### The `/gui_*` vocabulary (canonical tables in PLAN.md)

| Command (script -> host) | Args | Analogue |
|---|---|---|
| `/gui_def` | `id, json` | `/d_recv` - build a whole window/widget tree from a JSON GuiDef |
| `/gui_set` | `id, k, v, ...` | `/n_set` - update one live widget |
| `/gui_bind` | `id, "server", addr, prefix...` | forward a widget's value straight to the audio server; no target unbinds |
| `/gui_free` | `id` | group-free - destroy a widget and its subtree |
| `/gui_query` | `id` | request a `/gui_info` reply |
| `/gui_load` | `name` | load a persisted GuiDef from the def store |

Events back (host -> script): `/gui_event id value...` (also `"view" start len` from the heavy views), `/gui_info id type k v...`, `/gui_closed id`.

A GuiDef node is `{ "id": int, "type": str, <props...>, "children": [...] }`; the widget catalog is in PLAN.md and is **extensible by adding a `WidgetKind` + renderer, never by changing the protocol** (an unknown type is laid out but not painted). The address family is the generic `/gui_*`, not `/win_*`, because a root may be an embeddable panel, not only a window. **Standalone** mode: a saved GuiDef paired with GraphDefs (a *bundle* in the data dir, plus root `name`/`boot` and widget `bind` props that make a tree self-driving) runs against an embedded server with no language client at all (`--standalone <name>`, the `standalone` feature). Server, host and Python client share one TOML config schema (`clausters-core::config`; the host reads `[gui]`/`[standalone]` as flag defaults).

## The host (`src/host/`): agnostic core + platform shells

The host is a platform-agnostic core with two thin fronts; `clients/gui/check-wasm.sh` build-gates the core for `wasm32-unknown-unknown` so it can never re-couple to native I/O unnoticed.

- **Pure core (compiles to wasm unchanged):** `guidef` (generic node parse), `widget` (the typed `WidgetKind` tree, the single source of truth), `registry` (client ids, parent/children, subtree free, redefine-replaces - the node-tree shape), `layout` (`row`/`col`/`grid`/`free` -> pixel rects), `paint` (triangle `Mesh` + `Painter`: rect/quad/line/disc) + `font` (embedded 5x7 bitmap), `controls`, `meters`, `nodetree` (model + `/g_queryTree.reply` parser), `plot`, `bind`, `canvas` (script-supplied WGSL, error-scoped compile), `frame` (**the one per-window render both fronts call** - the browser is pixel-faithful by construction), `interact` (hit-test + value mutation, shared, so bound-vs-event is decided identically), `gestures` (the one press→drag→release→wheel state machine both fronts drive: it mutates the Host through the `interact` doors and returns `GestureEffect`s for the front's sinks; agnostic by design, cfg-gated native until the browser rewire leg), `fetch` (the `/b_query` -> chunked `/b_getn` buffer state machine), `live` (`StreamedBuses`, the `/c_stream`-fed `BusSource`; `collect_live_buses`), the protocol dispatch in `mod.rs`.
- **I/O behind small traits** (the platform seam): `Transport` (send OSC to the audio server; `ServerLink::{Udp, Embed, Ws}` implements it), `DefStore` (named-GuiDef persistence; `store::GuiStore` mirrors the server's defstore), `BulkLoader` (resolve a `path`/`cache` to samples/pyramid; native `bulk::MmapLoader`), `BusSource` (per-frame bus reads, plus `read_tap`/`sample_rate`/`sample_clock` for the tap windows and the timeline playhead).
- **Native shell:** `transport` (UDP front), `client` (the UDP server leg), `shm` (read-only mmap of the server's `--shm` segment - control buses, `sample_clock()`, `sample_rate()`; validates `MAGIC`/`ABI_VERSION`), `mapfile`/`bulk` (mmap), `store`, `embed` (the `standalone` `EmbedServer`), `gui/` (winit + wgpu windowed front, multi-window by def id, ~30 fps animation for live views; split by role: `app` state + event loop, `windows` lifecycle, `serverleg` audio-server replies, `input` thin adapters onto the shared `gestures` machine, `midi` live MIDI note painting).
- **Web shell:** `web.rs` - a wasm-bindgen `GuiBridge` (`feed`/`def` push OSC packets in, `poll` drains events out, `connect_server(url)` attaches a `WsServerLink` to a `--ws` server); bulk `path`/`cache` resolve as URLs via `fetch`; buses arrive over the server's `/c_stream` (one subscription per client, derived from the tree). `crate::gpu` picks WebGPU where a real adapter exists and **falls back to WebGL2** otherwise (the crate deliberately uses no compute/storage buffers, so everything translates); packaging is `web/build.sh` + `web/index.html`.

**Bulk data rule:** large payloads move through **local shared resources** (mapped files, the shm segment), never re-encoded over OSC; the network primitives (`/b_getn`, inline blobs) are the async fallback - and the browser's path. A client builds a peaks cache and hands the host the file; a server buffer is exported to a mapped file or pulled over the leg. Shared analysis (forward FFT, the peak pyramid, windows) lives **once** in `clausters-core` (`fft`/`peaks`/`window`), reused by host (native and wasm), server and Python alike - never reimplemented per client.

## The reusable rendering machinery (crate root)

The heavy views are written **once against `wgpu`/WGSL** and run natively and in the browser (WebGPU or WebGL2) unchanged. The modules are windowing-agnostic; `native.rs` is only the standalone demo harness (`--bin waveform`/`spectrogram`), the real windowed front is `host/gui/`.

| Module | Public surface | Role |
|---|---|---|
| `viewport::View` | `full(total)`, `samples_per_px(w)`, `zoom(factor, anchor, total)`, `pan(dx, total)`, `set_start`, clamp | the visible window in `f64` sample units; pure, unit-tested, renderer-agnostic |
| `peaks` (re-export of `clausters_core::peaks`: `Pyramid` + the multichannel `MultiPyramid`) | `build(samples, base_bucket)`, `level_for(spp)`, `column(level, s0, s1)`, `MultiPyramid::build_interleaved`, `to_bytes`/`from_bytes` (CLPK v1 mono / v2 multichannel — one cache file for all channels), `write_cache`/`read_cache` | resolution-matched min/max peak LOD + cache |
| `spectrogram::Stft` | `compute(samples, window, hop, sr)`, `magnitudes()`, `n_frames`/`n_bins`/`nyquist`, same cache shape; `FreqScale`, `SpectrogramRenderer`, `SpectrogramView` | windowed-FFT time-frequency analysis (FFT from `clausters_core::fft`) + GPU view |
| `waveform` | `WaveformData::new/from_interleaved/with_pyramid/with_multi_pyramid`, `WaveformRenderer` (per-vertex color, one vertex range per channel; `draw`/`draw_channel`), `upload_geometry`, `column` (LOD-crossfaded) | two GPU pipelines (min/max columns + raw polyline); the *three regimes* and the level crossfade are a per-frame data choice, not new pipelines |
| `gpu` | `Gpu::new` (async), `new_instance` | device/surface bring-up shared by native, windowed and web fronts; WebGPU-with-WebGL2-fallback on wasm |
| `bytes` (crate-private) | `push_*` / `Reader` little-endian | flat `f32`-array cache layout, mmap-ready |

## Graphics / GPU knowledge that applies here

- **The one rule: never resolve the signal finer than the screen.** All work is driven by `samples_per_px = visible_len / render_width_px`, never by buffer length. The waveform expresses it by **picking a resolution-matched LOD per frame**; the spectrogram expresses it **structurally** - one texture sample per pixel, so GPU cost is constant regardless of zoom.
- **Two rendering shapes, both used here.** (a) *Geometry pipelines* - the waveform rebuilds a vertex buffer per frame whose size is bounded by `render_width_px` (a line-strip in the zoomed-in regime, min/max column quads otherwise); the flat widget chrome is the same idea through `paint::Mesh`. (b) *Full-screen quad + texture sample* - the spectrogram uploads the STFT once as a 2D texture (x=frame, y=bin) and the fragment shader samples it; the GPU's linear filtering gives free resolution-matched downsampling on zoom-out.
- **Uniforms are free, recompute is expensive - put every display control in the shader.** Colormap (a branch on a uniform index), dB window/contrast (remap within a fixed normalized range), linear-vs-log frequency axis (the shader maps screen-y to a normalized bin geometrically vs linearly), and frequency zoom/pan (a *second* `viewport::View`) all change live with zero CPU work. Only window size / hop / sample rate force a `Stft::compute`.
- **Anchor-preserving zoom is the core navigation math** (`View::zoom`): `pivot = start + len*anchor`, scale `len`, keep `pivot` fixed, clamp. The frequency axis keeps its window in **display coordinates**, not in bins, so the cursor anchor stays fixed in *both* linear and log modes - the log nonlinearity lives entirely in the shader's display->bin mapping.
- **Device pixels, not logical pixels.** `render_width_px` is in physical/device pixels; multiply by the HiDPI scale factor, or peaks come out blurry/over-detailed. winit gives the scale factor on resize.
- **WebGL2 is the compatibility floor.** The wasm bundle compiles both backends and picks at runtime; keep new GPU work inside what WebGL2 can do (vertex/fragment pipelines, uniform buffers, filterable textures - **no compute shaders, no storage buffers**; heavy numeric work runs on the CPU in `clausters-core`). naga translates the WGSL (including user `canvas` shaders) to GLSL ES automatically; a non-translatable shader degrades through the existing error scope, unpainted, no panic.

## Audio-analysis algorithms behind the views

- **Peak pyramid (waveform LOD) = mip-mapping for 1-D audio.** Level 0 stores `(min, max)` per `base_bucket` samples; each higher level halves resolution; `level_for(spp)` selects the level whose bucket ~matches one pixel column. Three render regimes by `samples_per_px`: **Line** (`<= ~2`: polyline through raw samples), **Raw columns** (`< base_bucket`: exact min/max per pixel), **Pyramid columns** (`>= base_bucket`: read the matching level). The pyramid is ~2x its level-0 size and is a **cache** (memory or file, mmap-ready) the way editors keep an overview file.
- **STFT (spectrogram) = the time-domain analogue.** A windowed FFT (Hann) every `hop` samples yields `n_frames x n_bins` magnitudes, normalized over a fixed dB reference (-120..0) and stored as the cache. Log frequency is the audibly useful default. FFT/window DSP details and correctness tests (impulse -> flat spectrum, cosine -> single bin) belong with [[ugen-dsp]] and [[audio-testing]].
- **Live views:** `meter` and the control-rate `scope` read a control bus per frame - from the shm segment natively (zero messages; see [[realtime-audio]] for the shared bus and the embed ABI) or from `/c_stream` in the browser; `canvas` params can ride the same bus path. Audio-rate data comes from the server's **audio taps** (segment ABI v3: `/tap` routes a bus into a pre-allocated ring; `/tap_stream` -> `/tap_data` is the browser/headless sibling): the `scope` widget's `tap` prop makes it a triggered oscilloscope (`host/oscil.rs` + `live::update_tap_windows`, shared by both fronts); the other tap consumers are `phasescope` and the live `spectrum` - algorithms in [[gui-widgets]]. The two timeline views (`waveform`/`spectrogram`) are host widgets at editor grade - multichannel lanes, LOD crossfade, rulers, selection, playhead (native: the shm header's `sample_clock`, zero messages; browser: `/clock` per tick), all drawn as an overlay `Painter` pass in the shared `frame::render` - details in [[gui-widgets]]. Heavy views must **never reimplement DSP the server owns** - the server already carries the spectral chain; host-side analysis is only what plotting needs, from `clausters-core`.
- The widget tree reuses the **scsynth node-tree shape verbatim** - client-allocated ids, subtree free, `/g_query`-style introspection; the live `nodetree` view reads the server's real tree over `/g_queryTree` + `/notify` (see [[scsynth-osc]]).

## Driving it from Python: the `clausters.gui` submodule

Every host feature is exercised (and lands implemented) through `clients/python/clausters/gui/` - each widget gets a builder, each protocol verb a client call:

- **`guidef.py`** builds GuiDef trees the way defs are built: one function per widget type (`window`/`panel`/`label`/`slider`/`knob`/`button`/`toggle`/`number`/`text`/`menu`/`waveform`/`spectrogram`/`meter`/`scope`/`phasescope`/`spectrum`/`nodetree`/`plot`/`bpf`/`canvas`, plus the generic `node`), the envelope round trip `env_to_points`/`points_to_env`, `to_json`, and the bulk helpers `samples_to_blob`/`samples_to_file`/`peaks_cache_file`. It reuses the existing OSC encoder (`clausters/base/_osclib.py` `message(addr, *args)` preserves the int/float distinction) - never a parallel encoder.
- **`host.py`** is the `GuiHost` client object over `OscUdpInterface`, pointed at the host's port, not the audio server's: `define(id, tree, *blobs)`, `set`, `free`, `query`, `bind(id, address, *prefix)`/`unbind(id)`, and `poll`/`listen` for `/gui_event`/`/gui_info`/`/gui_closed`. Building the tree is host-agnostic; only `GuiHost` knows how to talk to the host. Events can also dispatch through the `OscFunc` responder model (`clausters/responders.py`).
- **Examples are the acceptance surface:** `examples/gui_*.py` (skeleton, window, panel, meters, bind, nodetree, plot, canvas, standalone) each demonstrate one protocol capability end to end.
- **E2E sandbox rule (non-negotiable):** the Bash sandbox isolates the network between invocations - run the gui host and the Python client in the **same** invocation (host in background with `&`, then the client, then kill). GPU-free assertions (navigation math, peaks vs brute force, FFT/STFT localization, cache round-trips, parse/layout/fetch/nodetree models) stay in the gui crate's `cargo test`; window binaries need a display and a Vulkan/Metal/DX12/GL adapter; `--headless` runs the protocol with no display.

## Conventions (carry over from the rest of the project)

- Build/test the crate from `clients/gui/`; keep it `cargo fmt --check`-clean and clippy-clean, native **and** `wasm32` (`check-wasm.sh`). Code, comments, strings and tests are English.
- `Gx` milestone labels stay in `clients/gui/PLAN.md` only.
- Markdown in this repo is **single-line paragraphs/list items, no hard-wrap** (tables and code blocks exempt).
- **Closing a GUI milestone** means more than code: a clear commit message and the `clients/gui/PLAN.md` checkbox, refresh that file's design sections if the rationale moved (promoting anything settled to `docs/clients.md`), developer/user docs where applicable (`docs/clients.md` carries the browser quick-start), a commented example, a `docs/decisions.md` note only for a non-obvious choice, and a `GUIA.md` smoke step only for new human-audible/visual behavior ([[documentation]] for placement).
