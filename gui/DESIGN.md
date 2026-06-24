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

An editor-grade waveform or spectrogram cannot draw every sample. The strategy, validated by the prototype:

- Precompute a **min/max envelope pyramid** over the buffer once (a small constant fraction of its size).
- Each frame, pick the pyramid level matching the current zoom and emit **one quad per pixel column** spanning that column's [min, max]. Per-frame cost is proportional to window width in pixels, not buffer length, so zoom/pan over millions of samples stay smooth.
- A spectrogram is the same navigation logic over a precomputed STFT, sampled into a texture instead of min/max columns - a natural next step reusing the same `View`/zoom/pan machinery.

Notation (`"score"` widget) is out of the GPU path entirely: it is Verovio-rendered SVG hosted by the web surface, made interactive there.

## The prototype in this crate

`src/waveform.rs` + `src/waveform.wgsl` + `src/main.rs` implement the waveform widget's renderer and a native `winit` driver for it. Build/run with a display and a GPU:

```
cd gui && cargo run
```

Controls: mouse wheel zooms toward the pointer, left-drag pans, `R` resets the view, `Esc` quits. It generates ~4 million samples (a frequency sweep with a tremolo envelope plus light noise) so that zooming in shows individual cycles and zooming out shows the amplitude bursts.

The split is the point:

- `WaveformRenderer` takes a `wgpu::Device`/`Queue` and a target texture format and owns nothing windowing-specific. The native window in `main.rs` is just a driver.
- The identical `WaveformRenderer` + WGSL would be driven by a `<canvas>` WebGPU surface in a browser build - which is how this prototype maps onto host option A/B above.

This validates the load-bearing claim: the heavy, custom, GPU-bound widgets can be written once against `wgpu`/WGSL and run both natively and on the web, while the *composition* of widgets is a scripted protocol, not compiled Rust.

## Open questions / next steps

- Transport: OSC-over-WebSocket bridge vs. a dedicated host protocol; reuse `osc::decode_packet` regardless.
- Where the GUI host lives: standalone process, or embedded next to the embed-server surface.
- Spectrogram widget: STFT precompute + texture sampling reusing `View`.
- Multi-LOD for extreme zoom-out accuracy (the prototype uses a single pyramid walk; production may interpolate between adjacent levels) and a raw-sample polyline path for extreme zoom-in.
- Verovio integration spike (wasm/JS build) behind a `"score"` widget.
- Selection/playhead overlays and time-axis rulers as shared widget chrome.
```
