# Clausters GUI track - design notes

Exploratory branch (`gui`). This crate is an **independent workspace**, deliberately not a member of the root `clausters` workspace, so it can never break the core server build. It holds two things: this design note for a scriptable widget protocol, and a working GPU waveform prototype (`src/`) that validates the heavy-rendering path.

Nothing here is committed to. It exists to make the architecture concrete enough to argue about before any of it lands in `PLAN.md`.

## The one decision that drives everything

The goal is not "a GUI for Clausters". It is a system of graphical elements that can be **built and driven from a dynamic language (Python, JavaScript)** at runtime - the way SuperCollider's `sclang` builds Qt widgets - covering varied uses: an audio editor, a navigable waveform/spectrogram, editable music notation, custom instrument panels.

SuperCollider does not embed a GUI library into a language. `sclang` sends messages to a separate widget engine (Qt) over a protocol; the engine owns the windows and the pixels. That decoupling is exactly the Clausters philosophy already in place for audio: a server owns the real-time work, and any number of clients drive it over OSC.

So the GUI is **another client peer**, not code compiled into the audio server, and not a fixed Rust API that scripts cannot reach. Concretely there is a **GUI host** process that owns windows, widgets and the GPU, and speaks a widget protocol. Scripts (Python now, JS later) send it widget commands and receive interaction events. The audio server is untouched by any of this.

```
  Python / JS script  ──widget protocol──▶  GUI host (windows, widgets, GPU)
        │                                        │
        └──────────── OSC ───────────────▶  Audio server (scsynth-style)
                                                 ▲
                 (a widget may be bound to forward its value straight here)
```

### Why web / hybrid for the pixels

The varied uses pull toward a web-capable rendering substrate, for three concrete reasons, not fashion:

1. **Scriptability from JS is free** if the host *is* a web surface; Python drives the same surface over the same protocol (WebSocket).
2. **Music notation = Verovio.** Verovio is a C++ engraving library that renders MEI/MusicXML to **SVG**, and ships a **WebAssembly/JS** build. In a web surface it drops in directly; MuseScore (a whole Qt app) is not embeddable, so what we actually want - its notation capability - is Verovio rendering SVG. Editable notation then means making that SVG interactive, which the browser does natively.
3. **One GPU stack, two targets.** `wgpu` *is* the WebGPU implementation. The heavy widgets (waveform, spectrogram) are custom GPU rendering either way; written against `wgpu`/WGSL they run natively today and under WebGPU in a browser unchanged. The prototype in this crate exists to prove that seam.

Two ways to ship the host, sharing ~90% of the frontend:

- **A - Web frontend over WebSocket.** Host the widget surface in a browser; the audio server (or a thin bridge) speaks OSC-over-WebSocket. Maximum scriptability and reach (the browser is the ultimate cross-platform target), Verovio for free.
- **B - Tauri desktop app.** Rust core + system-webview frontend (same web stack), packaged as a native app, with FFI available to Rust/C++ when a widget needs a native library directly.

Start with A for iteration speed; B reuses the frontend when a packaged desktop app is wanted.

What we explicitly reject: betting the whole thing on a single native Rust toolkit (egui/iced/Vizia/Makepad) as the scriptable layer. That would force us to invent a widget protocol *and* solve Verovio over FFI *and* give up the web - paying all three costs. Those toolkits are excellent for a monolithic Rust app, which is not what this is.

## The widget protocol (mini-design)

The protocol is between a **script (client)** and the **GUI host**, carried over the same OSC encoding Clausters already uses (`osc::decode_packet` is the single decode door), transported over WebSocket for the web host. It deliberately mirrors the scsynth node-tree model the project already implements, so the mental model is reused rather than reinvented.

### Addressing model

- Widgets form a **tree**. Every widget has a client-allocated integer id, exactly like scsynth node ids (the client owns an id allocator; no server round-trip to create one).
- A **window** is a root container. Containers hold children; layout is a property of the container.
- Destroying a widget destroys its subtree, like freeing a group frees its nodes.

This 1:1 reuse of the node-tree semantics is intentional: id allocation, add-actions, and subtree freeing already exist conceptually in the codebase.

### Commands (client -> host)

| Address | Args | Meaning |
|---|---|---|
| `/win_new` | `id, title, w, h` | Create a top-level window (root container). |
| `/w_new` | `parent_id, id, type, [k, v]...` | Create a widget of `type` under `parent_id` with initial properties. |
| `/w_set` | `id, [k, v]...` | Update properties (value, range, label, color, ...). |
| `/w_layout` | `container_id, type, [params]...` | Set a container's layout (`row`/`col`/`grid`/`free`). |
| `/w_bind` | `id, target...` | Bind this widget's value to a destination (see below). |
| `/w_free` | `id` | Destroy a widget and its subtree. |
| `/w_query` | `id` | Request a `/w_info` reply. |

`type` is a short string (`"knob"`, `"slider"`, `"button"`, `"label"`, `"waveform"`, `"spectrogram"`, `"score"`, ...). Property values are OSC primitives (int/float/string/blob) - the binding technology never leaks across the wire, in line with the project's "flat primitives at the boundary" rule. A `"waveform"`/`"spectrogram"` widget is fed a buffer reference (a server buffer number) or a blob, and owns its own GPU rendering (the prototype here is exactly that widget's renderer).

### Events and replies (host -> client)

| Address | Args | Meaning |
|---|---|---|
| `/w_event` | `id, value...` | A widget was interacted with (knob turned, button pressed, region selected). |
| `/w_info` | `id, type, [k, v]...` | Reply to `/w_query`. |
| `/win_closed` | `id` | A window was closed by the user. |

### Bindings: the value can bypass the script

`/w_bind` lets a widget's value flow **straight to the audio server** without a round-trip through the script - the same idea already used for MIDI in this project, where a control source is bound to a server-side destination instead of being polled. A knob bound to a synth control sends an OSC `/n_set` (or equivalent) to the audio server itself on every change; an unbound knob just emits `/w_event` back to the script. This keeps interactive control low-latency while leaving scripted/computed widgets fully in the script's hands.

### Example session

```
# script -> host
/win_new      1 "Filter" 480 240
/w_new        1 10 "knob"   "label" "cutoff" "min" 20.0 "max" 20000.0 "value" 800.0
/w_new        1 11 "slider" "label" "res"    "min" 0.0  "max" 1.0     "value" 0.2
/w_new        1 12 "waveform" "buffer" 0          # renders server buffer 0 (prototype renderer)
/w_bind       10 "server" "/n_set" 1000 "cutoff"  # knob 10 drives synth node 1000's cutoff directly

# host -> script  (only for the unbound slider; the knob talks to the server itself)
/w_event      11 0.35
```

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

### Three render regimes (no wasted work, and never "by samples" naively)

Per frame the renderer picks one by `samples_per_px`, so it is always bounded by the screen:

- **Line** (`samples_per_px <= 2`): few enough samples are visible that individual ones matter - draw a polyline through the raw samples in range (vertex count bounded by window width). This is the only regime that touches raw samples directly, and only when it is cheap to.
- **Raw columns** (`2 < samples_per_px < base_bucket`): one min/max column per pixel computed directly from raw samples - exact, and bounded because we are below `base_bucket` samples/px.
- **Pyramid columns** (`samples_per_px >= base_bucket`): one min/max column per pixel read from the peak level matching the zoom.

### The spectrogram: same navigation, constant render cost

The spectrogram is the time-frequency analogue and deliberately reuses the navigation machinery. The STFT magnitudes are uploaded once as a 2D texture (x = time/frame, y = frequency bin). Rendering is a single full-screen quad whose fragment shader samples that texture; `viewport::View` only reshapes the sampled time slice, so the GPU cost is **constant regardless of zoom** (it is bounded by screen pixels, and the one-time analysis is the cache). The GPU's linear filtering gives resolution-matched down-sampling on zoom-out. So the waveform bounds work by *picking a resolution-matched LOD per frame*, while the spectrogram bounds it *structurally* (one texture sample per pixel) - two expressions of the same "never resolve finer than the screen" rule.

Display controls live entirely in the fragment shader as cheap uniforms, so they change live without re-analyzing:

- **Frequency axis, linear or log** (`L`): the shader maps screen-y to a normalized bin either linearly or geometrically - log is the audibly useful default.
- **Frequency zoom/pan** (Shift+wheel / Shift+drag): a *second* `viewport::View` supplies the visible window, the clearest payoff of factoring `View` out. The window is kept in **display coordinates** (the screen's vertical axis), not in bins, so the linear screen anchor of a zoom holds the point under the cursor fixed *in both linear and log modes* - the log nonlinearity lives entirely in the shader's display->bin mapping over the full axis.
- **dB window / contrast** (`[` / `]`): magnitudes are stored normalized over a fixed reference range (-120..0 dB), and the shader remaps a movable display window within it. So contrast is a uniform, not a recompute.
- **Colormap** (`/`): cycles viridis / magma / grayscale, a shader branch on a uniform index.

The analysis parameters that *do* require recomputation - window size, hop, sample rate - are arguments to `Stft::compute`.

Notation (`"score"` widget) is out of the GPU path entirely: Verovio-rendered SVG hosted by the web surface, made interactive there.

## The prototype in this crate

```
src/lib.rs           module index
src/viewport.rs      View + zoom/pan (unit-tested)
src/peaks.rs         Pyramid peak analysis + cache (unit-tested)
src/spectrogram.rs   Stft analysis + FFT + cache + renderer (unit-tested)
src/spectrogram.wgsl full-screen quad, texture sample, viridis colormap
src/waveform.rs      WaveformData + WaveformRenderer (3 regimes)
src/waveform.wgsl    passthrough shader (columns + line pipelines)
src/view.rs          TimelineView trait (incl. optional char/vertical hooks)
src/native.rs        winit + wgpu harness driving any TimelineView
src/demo.rs          synthetic test signal
src/bin/waveform.rs      waveform binary
src/bin/spectrogram.rs   spectrogram binary
```

`cargo test` exercises the reusable machinery without a GPU: navigation math, peak correctness vs brute force, FFT correctness (impulse -> flat spectrum, cosine -> peak at its bin), STFT frequency localization, and cache round-trips (memory and temp file). `cargo run --bin waveform` and `cargo run --bin spectrogram` open the two windows (need a display and a Vulkan/Metal/DX12/GL adapter). Shared time navigation: wheel zooms toward the pointer, left-drag pans, `R` resets, `Esc` quits. The spectrogram adds: Shift+wheel / Shift+drag for frequency zoom/pan, `L` to toggle linear/log frequency, and `[` / `]` for the dB floor. Both render the same ~4 M-sample sweep, so the waveform shows cycles/bursts and the spectrogram shows a rising frequency ridge.

The split is the point:

- The renderers take a `wgpu::Device`/`Queue` and a target format and own nothing windowing-specific; `native` is just a driver, swappable for a `<canvas>` WebGPU surface in a browser build.
- Adding a view (e.g. a level meter, an FFT curve) is a new module implementing `TimelineView` plus a one-screen binary - no new windowing or input code. View-specific input rides the trait's optional `on_char`/`on_vertical_*` hooks, so the harness stays generic.

This validates the load-bearing claim: the heavy, custom, GPU-bound widgets can be written once against `wgpu`/WGSL and run both natively and on the web, while the *composition* of widgets is a scripted protocol, not compiled Rust.

## Open questions / next steps

- Transport: OSC-over-WebSocket bridge vs. a dedicated host protocol; reuse `osc::decode_packet` regardless.
- Where the GUI host lives: standalone process, or embedded next to the embed-server surface.
- Cache lifecycle: a cache key (source path + mtime + analysis params) and memory-mapping the cache file instead of reading it into RAM.
- Spectrogram: time-axis mipmaps or tiling for buffers wider than the max texture size; frequency-axis labels/ruler in Hz; a smoother (interpolating) log resample. (Log axis, frequency zoom via a second `View`, and a live dB window are done.)
- Multi-channel waveforms (stacked or overlaid), plus interpolating between adjacent pyramid levels for smoother zoom-out.
- Verovio integration spike (wasm/JS build) behind a `"score"` widget.
- Selection/playhead overlays and time-axis rulers as shared widget chrome.
```
