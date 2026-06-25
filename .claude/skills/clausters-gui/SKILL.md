---
name: clausters-gui
description: How to work on the Clausters GUI track (clients/gui) — the scriptable widget protocol (a GuiDef is JSON-in-OSC, the /gui_* vocabulary, bindings, standalone), the reusable wgpu/WGSL rendering machinery (viewport/peaks/Stft/TimelineView and the "never resolve finer than the screen" rule), the audio-analysis algorithms behind the heavy views, and the strategy of growing functionality in a clausters.gui Python submodule to test it. Consult before touching anything under clients/gui or building any GUI widget/view.
---

# Clausters GUI track (clients/gui)

A system of graphical elements **built and driven from a dynamic language at runtime** - the way sclang builds Qt widgets - not a GUI compiled into the audio server. The canonical design rationale is `clients/gui/DESIGN.md`; the staged milestones (`Gx`) are `clients/gui/PLAN.md`. This skill is the working map; those two files win on any detail, and `Gx` labels live only there and in `LOG.md` (never in published docs or this skill's prose).

The crate is an **independent workspace** - build and test from inside `clients/gui/` (`cargo test`, `cargo run --bin waveform`), never from the repo root, so it can never entangle the core server build.

## Architecture in one screen

`clausters-gui` is **two roles in one process**:

- a **GUI server (host) for the languages** - owns windows, widgets and the GPU, exposes a widget protocol;
- a **client of the audio server** - reads buffers/buses/node tree and sends control, exactly as [[clausters-python]] does.

Three legs: script <-> audio server (existing OSC control), script <-> gui host (the new widget protocol), gui host <-> audio server (gui as a client). A **bound** widget bypasses the script: its value flows straight from the host to the audio server (low-latency), the same idea as MIDI bindings in the server.

The protocol shape, decided in PLAN.md and corrected from an earlier per-widget sketch:

- **OSC is the single encoding everywhere**, through the one `osc::decode_packet` door, over the server's existing transports (shared-memory ring / TCP / WebSocket / UDP). The browser can only do WebSocket - that is the primary reason `WsHub` exists.
- **A GuiDef is JSON inside one OSC argument**, exactly as a `SynthDef`/`GraphDef` rides `/d_recv`. JSON is the payload, OSC is the framing. serde's number handling keeps ids `i32` and control values `f32` across the wire - preserve that int/float distinction on the client side too.
- **Construction is declarative, updates incremental.** A GUI is a *def*: build the whole tree in one message, then mutate live widgets one at a time. No per-widget construction chatter.

### The `/gui_*` vocabulary (canonical tables in PLAN.md)

| Command (script -> host) | Args | Analogue |
|---|---|---|
| `/gui_def` | `id, json` | `/d_recv` - build a whole window/widget tree from a JSON GuiDef |
| `/gui_set` | `id, k, v, ...` | `/n_set` - update one live widget |
| `/gui_bind` | `id, target...` | bind a widget's value straight to the audio server |
| `/gui_free` | `id` | group-free - destroy a widget and its subtree |
| `/gui_query` | `id` | request a `/gui_info` reply |
| `/gui_load` | `name` | load a persisted GuiDef (standalone path) |

Events back (host -> script): `/gui_event id value...`, `/gui_info id type k v...`, `/gui_closed id`.

A GuiDef node is `{ "id": int, "type": str, <props...>, "children": [...] }`; the widget catalog (containers, controls, the heavy GPU views) is in PLAN.md and is **extensible by adding a renderer/handler, never by changing the protocol**. The address family is the generic `/gui_*`, not `/win_*`, because a root may be an embeddable panel, not only a window. **Standalone** mode: a persisted GuiDef paired with GraphDefs runs against an embedded server with no language client at all.

## The reusable rendering machinery (the prototype, already in `src/`)

The heavy views are written **once against `wgpu`/WGSL** and run natively today and under WebGPU in a browser unchanged - that is the load-bearing claim the prototype proves. The modules are windowing-agnostic and web-portable; the native winit harness is a thin, swappable driver.

| Module | Public surface | Role |
|---|---|---|
| `viewport::View` | `full(total)`, `samples_per_px(w)`, `zoom(factor, anchor, total)`, `pan(dx, total)`, `set_start`, clamp | the visible window in `f64` sample units; pure, unit-tested, renderer-agnostic |
| `peaks::Pyramid` | `build(samples, base_bucket)`, `level_for(spp)`, `column(level, s0, s1)`, `to_bytes`/`from_bytes`, `write_cache`/`read_cache` | resolution-matched min/max peak LOD + cache |
| `spectrogram::Stft` | `compute(samples, window, hop, sr)`, `magnitudes()`, `n_frames`/`n_bins`/`nyquist`, same cache shape; `FreqScale`, `SpectrogramRenderer`, `SpectrogramView` | windowed-FFT time-frequency analysis + GPU view |
| `waveform` | `WaveformData::new/with_pyramid`, `WaveformRenderer`, `Mode::{Columns,Line}`, `upload_geometry`, `column` | two GPU pipelines (min/max columns + raw polyline); the *three regimes* are a per-frame data choice in `upload_geometry`/`column`, not three pipelines |
| `view::TimelineView` | `total_samples`, `upload(device,queue,view,width_px)`, `draw(pass)` + optional `on_char`/`on_vertical_{zoom,drag_begin,drag}` | the trait both views implement; the extension seam |
| `native::run(title, factory)` | winit + wgpu event loop generic over any `TimelineView` | native-only driver, swappable for a `<canvas>` WebGPU surface |
| `bytes` (crate-private) | `push_*` / `Reader` little-endian | flat `f32`-array cache layout, mmap-ready |

**To add a view (a level meter, an FFT curve, a scope):** implement `TimelineView` in a new module plus a one-screen `src/bin/` binary. No new windowing or input code - view-specific input rides the optional `on_char`/`on_vertical_*` hooks, so `native` stays generic.

## Graphics / GPU knowledge that applies here

- **The one rule: never resolve the signal finer than the screen.** All work is driven by `samples_per_px = visible_len / render_width_px`, never by buffer length. The waveform expresses it by **picking a resolution-matched LOD per frame** (three regimes below); the spectrogram expresses it **structurally** - one texture sample per pixel, so GPU cost is constant regardless of zoom.
- **Two rendering shapes, both used here.** (a) *Geometry pipelines* - the waveform rebuilds a vertex buffer per frame whose size is bounded by `render_width_px` (a line-strip in the zoomed-in regime, min/max column quads otherwise). (b) *Full-screen quad + texture sample* - the spectrogram uploads the STFT once as a 2D texture (x=frame, y=bin) and the fragment shader samples it; the GPU's linear filtering gives free resolution-matched downsampling on zoom-out.
- **Uniforms are free, recompute is expensive - put every display control in the shader.** Colormap (viridis/magma/grayscale = a branch on a uniform index), dB window/contrast (remap within a fixed normalized range), linear-vs-log frequency axis (the shader maps screen-y to a normalized bin geometrically vs linearly), and frequency zoom/pan (a *second* `viewport::View`) all change live with zero CPU work. Only window size / hop / sample rate force a `Stft::compute`.
- **Anchor-preserving zoom is the core navigation math** (`View::zoom`): `pivot = start + len*anchor`, scale `len`, keep `pivot` fixed, clamp. The frequency axis keeps its window in **display coordinates** (the screen's vertical axis), not in bins, so the cursor anchor stays fixed in *both* linear and log modes - the log nonlinearity lives entirely in the shader's display->bin mapping.
- **Device pixels, not logical pixels.** `render_width_px` is in physical/device pixels; multiply by the HiDPI scale factor, or peaks come out blurry/over-detailed. winit gives the scale factor on resize.
- **The winit/wgpu seam is the only non-portable part.** `native` translates OS events into the trait's windowing-agnostic hooks; a web build swaps it for a `<canvas>` surface and the JS client builds OSC over WS, renderers unchanged. Keep new view logic out of `native` and behind `TimelineView`.

## Audio-analysis algorithms behind the views

- **Peak pyramid (waveform LOD) = mip-mapping for 1-D audio.** Level 0 stores `(min, max)` per `base_bucket` samples; each higher level halves resolution; `level_for(spp)` selects the level whose bucket ~matches one pixel column. Three render regimes by `samples_per_px`: **Line** (`<= ~2`: polyline through raw samples), **Raw columns** (`< base_bucket`: exact min/max per pixel from raw samples), **Pyramid columns** (`>= base_bucket`: read the matching level). The pyramid is ~2x its level-0 size and is treated as a **cache** (memory or temp file, mmap-ready) the way editors keep an overview file.
- **STFT (spectrogram) = the time-domain analogue.** A windowed FFT (Hann) every `hop` samples yields `n_frames x n_bins` magnitudes, normalized over a fixed dB reference (-120..0) and stored as the cache. Log frequency is the audibly useful default. FFT/window DSP details and correctness tests (impulse -> flat spectrum, cosine -> single bin) belong with [[ugen-dsp]] and [[audio-testing]].
- **Future views, same machinery:** a time-domain `scope` (ring of recent samples), a `phasescope`/goniometer (L vs R as a Lissajous x/y), a live FFT `spectrum` (a one-frame STFT per frame), and a `meter` reading a control bus **directly from the shared-memory segment each frame with zero messages** (see [[realtime-audio]] for the shared bus and FTZ, and the embed ABI). Heavy views must **never reimplement DSP the server owns** - reach for `clausters-ffi`/`libclausters`, and the `peaks`/`Stft` machinery is itself a candidate to migrate behind the FFI ([[faust-embedding]] for the FFI/DSP boundary).
- The widget tree reuses the **scsynth node-tree shape verbatim** - client-allocated ids, subtree free, `/g_query`-style introspection for the live `nodetree` view (see [[scsynth-osc]]).

## Testing strategy: grow a `clausters.gui` Python submodule

The practical way to exercise and **incrementally implement** the protocol is to extend [[clausters-python]] with a new `clients/python/clausters/gui/` submodule, so each host feature gets a client to drive it (and lands implemented):

- **Build GuiDefs the way defs are built.** Mirror `clausters/defs` (`SynthDef`/`GraphDef`): compose a tree of `{id, type, ...props, children}` dicts and serialize to JSON. Send it with `/gui_def <id> <json>` using the existing OSC encoder `clausters/base/_osclib.py` (`message(addr, *args)` already preserves the int/float distinction) - do not write a parallel encoder.
- **Reuse the transport, point it at the host.** `OscWsInterface` already exists in `clausters/base/_oscinterface.py` (and the ffi WS client over the C ABI); a `GuiHost` client object targets the gui host's port, not the audio server's. Keep the client/server split: building the GuiDef tree is host-agnostic; only the host object knows how to talk to the host.
- **Receive events with the responder model.** `/gui_event`/`/gui_info`/`/gui_closed` dispatch through the same `OscFunc` pattern in `clausters/responders.py` the server replies already use.
- **E2E sandbox rule (non-negotiable):** the Bash sandbox isolates the network between invocations - run the gui host and the Python client in the **same** invocation (host in background with `&`, then the client, then kill). GPU-free assertions on the analysis (navigation math, peak vs brute force, FFT/STFT localization, cache round-trips) stay in the gui crate's `cargo test`; the window binaries (`cargo run --bin waveform`/`spectrogram`) need a display and a Vulkan/Metal/DX12/GL adapter.

## Conventions (carry over from the rest of the project)

- Build/test the crate from `clients/gui/`; keep it `cargo fmt --check`-clean and clippy-clean. Code, comments, strings and tests are English.
- `Gx` milestone labels stay in `clients/gui/PLAN.md` and `LOG.md` only.
- Markdown in this repo is **single-line paragraphs/list items, no hard-wrap** (tables and code blocks exempt).
- **Closing a GUI milestone** means more than code: update `LOG.md` and `PLAN.md`, refresh `DESIGN.md` if the rationale moved, developer/user docs where applicable, `GUIA.md` manual-test steps, and a commented example - the full milestone checklist ([[documentation]] for placement).
