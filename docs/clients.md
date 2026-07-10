# Clients and language bindings

Clausters is a server; clients drive it. This chapter is the **cross-language
map**: the one native contract every client sits on, the Python client built on
it, and the path to a JavaScript client and to distributable packages. The
client work lives in the `clients/` tree; this is the architectural overview.

## One contract: the C ABI

Everything a client needs that must be *native* lives behind a single,
versioned **C ABI** — the scsynth plugin-ABI lesson: every binary boundary is
versioned and checked. Two cdylibs expose it, and the boundary rule is the same
for both: **only flat data crosses** — `f32`/`f64`/integers, pointer+length
arrays, NUL-terminated error strings. Never a library type (a numpy array can
*view* a returned pointer, but that is the client's choice, not a dependency).

| cdylib | crate | what it is | key entry points |
|---|---|---|---|
| `libclausters_ffi` | `clausters-ffi` over `clausters-core` | the **shared numeric/timing core** | `clausters_core_abi_version`, `clausters_core_unary`/`_binary` (builtins), `clausters_core_whitenoise`, `clausters_core_beats_to_secs`/`_secs_to_samples`/…, `clausters_sched_*` (beat queue), `clausters_clocksync_*` (sample-clock model), `clausters_rng_*` (seeded value stream), NTP timetag packing, `quant_delay`, `degree_to_midinote` |
| `libclausters` | `clausters` (feature `embed`) | the **server as a library** | `clausters_abi_version`, `clausters_render` (offline), `clausters_open`/`_send`/`_poll`/`_clock`/`_ctl_*` (in-process live server) |
| `libclausters_midi` | `clausters-midi` over `midly`/`midi2`/`midir` | **MIDI I/O** | `clausters_midi_write_smf` (`.mid`), `clausters_midi_write_clip` (MIDI 2.0 clip), `clausters_midi_free`; with `--features live`: `clausters_midi_output_open`/`_send`/`_close` (virtual port) |

Beside the in-process embed path, the same OSC reaches the server over **UDP**
or **shared memory** (`--shm`); see [Local transports & embedding](ipc.md). So a
client has three ways to talk to the server (UDP, shm, embed) and one way to
reach the native core (`libclausters_ffi`) — all language-agnostic.

Why a shared core at all: the builtins, the seeded white noise and the
beat/second/sample math are compiled **once** in `clausters-core` and used by
both the server's UGens and every client, so client-side results match the
server **by construction** for the operations the server computes natively.

The rule extends to everything a client computes about **values or time**, so a
port rebinds the core instead of reimplementing behaviour. Concretely, the core
owns: the clock's beat-ordered **scheduler queue** (min time, stable insertion
order for ties), the **sample-clock tracking model** (the least-squares
`sample = a + b·t` fit behind locking a clock to a server over the network),
the **seeded value stream** the random patterns draw from (one `u64` state
word, so a seeded sequence replays identically in every client language — a
host language's own RNG never leaks into sequenced values), **NTP timetag
packing** (one rounding rule, identical bits for identical instants),
**quantization** (`quant` boundary snapping) and the **pitch-space math**
(scale degree → MIDI note). What each language keeps is its control flow (the
coroutine driver that resumes routines) — and, as the one documented
exception, the **OSC byte codec**: structured message arguments cannot cross a
flat C ABI, so encode/decode of the wire bytes stays per-language while every
time value inside them comes from the core.

## The Python client

`clients/python/` is the reference client, a selective port of
SuperCollider's class library (sc3) covering both def formats (FaustDefs and
UGen-graph SynthDefs). It is **pure Python at runtime**: it
reaches the core through `ctypes` over `libclausters_ffi`, and speaks ordinary
OSC bytes to the server (UDP, TCP, or shm/embed via the transport module). It
mirrors the native contract in three layers — `base` (server-agnostic timing
and values), `seq` (events and patterns) and `defs` (the Faust/UGen definitions
and the `Server`, whose swappable interface is the live-RT / offline-NRT seam).

It has its **own documentation** — a guide and the generated API reference:
**[the clausters Python client book](https://clausters-python.readthedocs.io/)**.
<!-- Cross-link to the companion book; update the URL if the Read the Docs slug differs. -->

**This is also the proof the contract is language-agnostic**: Python — a
non-Rust language — already drives the whole system (core math, offline render,
live server) purely through the C ABI and OSC. Nothing in the boundary is
Python-specific.

## The GUI host: a scriptable peer

The GUI (`clients/gui`) is **another peer in the system, not code compiled into
the audio server** — the same decoupling Clausters already uses for audio. Just
as SuperCollider's `sclang` builds Qt widgets by sending messages to a separate
widget engine, a **GUI host** process owns the windows, widgets and the GPU and
speaks a widget protocol; scripts (Python now, JavaScript later) send it widget
commands and receive interaction events, while the audio server is untouched.
The host plays two roles in one process: a GUI server for the languages and a
*client* of the audio server (it reads buffers, control buses and the node tree
and can send control back). A bound widget can forward its value straight to the
audio server, bypassing the script, for low-latency control.

Two design choices carry the rest:

- **Declarative, def-shaped protocol.** A whole widget tree is one document, not
  a stream of per-widget messages — mirroring `SynthDef`/`GraphDef`. `/gui_def
  <id> <json>` builds a tree in one message (JSON as payload, OSC as framing,
  the single `osc::decode_packet` door), `/gui_set` updates a live widget,
  `/gui_free` frees a subtree. The wire form is generic (`{id, type, props,
  children}`) so the protocol never changes when a widget type is added; an
  unknown type is laid out but not painted, so old and new hosts interoperate.
- **A web-capable GPU substrate, one stack for both targets.** The heavy widgets
  (waveform, spectrogram, scopes) are custom GPU rendering written against
  `wgpu`/WGSL, which runs natively today and under WebGPU in a browser unchanged
  — so the native desktop host comes first and the browser target is reached by
  swapping the surface, not forking the renderers. The heavy views follow one
  rule — **never resolve the signal finer than the screen**: work is bounded by
  `samples_per_px`, and the expensive analysis (a peak pyramid, an STFT) is
  treated as a cache that moves through local shared resources (mmap natively,
  fetch in the browser), never chunked over the wire.

## The GUI host in the browser

The GUI host (`clients/gui`) also compiles to **WebAssembly** and runs in a browser tab: the same widget protocol, layout, renderers and interaction as the desktop host, over a `<canvas>`. It renders through **WebGPU where the browser truly supports it and WebGL2 otherwise** (~99% browser reach), and it talks to a **separate audio server over WebSocket** — start the server with `--ws` (default port 57120). There is no in-process engine in the browser; the embed/standalone path is native-only.

The browser fills the host's data paths over the network instead of shared memory and mapped files:

- **Meters/scopes/canvas buses**: the host subscribes the buses its widgets read with `/c_stream` and the server streams `/c_set` snapshots back at ~30 fps over the same WebSocket — the network counterpart of the shared-memory segment (see the control-bus commands in [Def schemas](schemas.md)).
- **Audio-tap views**: the host subscribes the audio taps its audio-rate scopes, phasescopes (a stereo pair) and live spectra read with `/tap_stream` and the server streams `/tap_data` windows back — the network counterpart of the segment's tap rings (see the audio-tap commands in [Def schemas](schemas.md)). The phasescope's correlation and goniometer geometry and the spectrum's FFT all come from `clausters-core`, so the browser computes them in wasm identically to the desktop.
- **Bulk waveform/spectrogram/plot data**: a `path`/`cache` reference is fetched as a URL against the page origin (raw `f32` samples — every interleaved channel kept — a prebuilt peak-pyramid cache, or an STFT cache; the pyramids and STFT lanes for raw fetches are built in wasm — the analysis lives in `clausters-core`), and a server `buffer` reference is pulled over `/b_query` + chunked `/b_getn` on the WebSocket leg. The editor chrome of the two heavy views (multichannel lanes, adaptive time/Hz rulers, the selection overlay, the vertical `y_start`/`y_len` view window) renders through the same shared frame path as the desktop — a `/gui_set` of any of it displays identically in the browser, while the pointer gestures (drag-select, pan, the y-ruler zoom) are native-only today; the playhead is driven by polling `/clock` once per animation tick instead of reading the shared segment's sample clock. The **linked navigation groups** (an explicit `link` prop shares one horizontal view, selection and playhead across timeline views; `/gui_set` of `view_start`/`view_len`/`sel_*`/`playhead_at` on any member applies group-wide) live in the host core's protocol dispatch, so they behave identically in the browser.

**Quick start** (from `clients/gui/`; one-time setup: `rustup target add wasm32-unknown-unknown` and `cargo install wasm-bindgen-cli --version <the wasm-bindgen version in Cargo.lock>`):

```sh
./web/build.sh                       # cargo wasm build + wasm-bindgen into web/
(cd web && python3 -m http.server)   # then open http://localhost:8000/
```

The page loads the bundle and calls the wasm entry point `start()`, which returns the **binding surface** (`GuiBridge`) the page drives: `def(id, json)` feeds a GuiDef (the same JSON the Python builders emit), `feed(packet)` pushes any raw `/gui_*` OSC packet, `poll()` drains the outbound `/gui_event`/`/gui_info`/`/gui_closed` packets, and `connect_server(url)` attaches the audio-server leg to a `--ws` server (a `bind`-ed widget then forwards straight to it, with no script round-trip). `web/index.html` is a documented throwaway harness with a panel, a meters and a bulk demo — it proves the host; the product browser client is the separate TypeScript track below.

## A JavaScript client (planned)

A JS client mirrors the Python one with **no new native work**: it sits on the
same C ABI and the same OSC.

- **Native bridge**: Node/Deno **N-API** (or `Deno.dlopen`) over
  `libclausters_ffi`/`libclausters` for desktop; **WebAssembly** for the
  browser (the core compiled to wasm; the server itself targets wasm only for
  the offline `render` path).
- **Mirror the layers**: `base`/`seq`/`defs` map across directly. The one
  language-specific piece is the coroutine driver — JS **generators / async**
  instead of Python's `yield`; the clock and the rest are the same value/time
  logic.

## Distribution

- **Python (done)**: a platform-tagged **wheel** that bundles the cargo-built
  cdylibs (`libclausters_ffi`, and `libclausters` with `embed,realtime`) inside
  the package (`clausters/_libs/`), so an installed package is self-contained —
  no `target/` directory, no build step at import. The runtime stays
  stdlib-only; the loaders prefer the bundled copy, falling back to the
  workspace `target/` in a source checkout. A `setup.py` build hook runs `cargo
  build` and stages the libraries; `python -m build --wheel clients/python`
  produces the wheel. See the [Python client
  book](https://clausters-python.readthedocs.io/) for the install recipes and
  the env knobs (`CLAUSTERS_WORKSPACE`, `CLAUSTERS_CARGO_FEATURES`, …).
  Cross-platform CI wheels (cibuildwheel / manylinux) and a Faust-enabled build
  are still future work.
- **JavaScript (planned)**: an npm package with prebuilt N-API addons per
  platform and a wasm build for the browser.
- **Reproducible Faust build (planned)**: the `faust` feature needs libfaust
  built with the LLVM backend; for the wasm target and for Faust-enabled
  wheels/npm builds this should be vendored under `third_party/` with documented
  build steps. The current wheel ships the core embed build (no `faust`
  feature).

## Status at a glance

| Piece | State |
|---|---|
| Shared core + C ABI (`clausters-core`/`clausters-ffi`) | done |
| Python client (`base`/`seq`/`defs`, incl. UGen `SynthDef`) | done |
| Cross-language docs + sequencing example | done |
| Python wheels packaging | done |
| MIDI interfaces in the Python client (`MidiServer`, SMF / MIDI 2.0 clip export, live port) | done |
| Client-side OSC/MIDI responders (`OscFunc`/`MidiFunc`) | done |
| Browser GUI host (wasm bundle; meters over `/c_stream`, bulk over fetch/`/b_getn`) | done |
| JavaScript client + npm | planned |
| `third_party` Faust build + Faust-enabled wheels | planned |
