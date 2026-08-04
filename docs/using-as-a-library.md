# Using Clausters as a library

The crate is both a binary (the server) and a library (`rlib` + `cdylib`). This chapter is the conceptual map; the precise API reference is the **rustdoc** — run `cargo doc --open`.

```toml
[dependencies]
clausters = { path = "…" }   # or a git/version dependency
```

The engine (`server::engine`) knows nothing about the audio backend: it processes blocks of `BLOCK_SIZE` frames against in-memory slices, so you can drive it from your own code exactly like the cpal callback or the offline renderer do.

## Feature flags

| feature | default | what it adds |
|---|---|---|
| `synth` | yes | the SynthDef family: the UGen library, the def compiler (`/def_send synth`) and `UGenSynth`. Independent from `faust` — the two def families combine freely, and a single-family build works (with neither, the engine core still builds but has no defs to instantiate). |
| `realtime` | yes | the cpal backend (the live server). Disable it for offline/embedded use with no audio device. |
| `pipewire` | yes | native PipeWire audio backend on Linux/BSD via cpal's pipewire host. `cpal::default_host()` prefers PipeWire, falling back to ALSA at runtime, so no code changes. Default (the binary hard-links `libpipewire`); drop it with `--no-default-features` for a plain-ALSA build. Building it needs `libpipewire-0.3-dev` and `clang`. |
| `midi-jack` | no | routes live MIDI through midir's JACK backend instead of ALSA seq (needed on PipeWire systems — the ALSA-seq backend panics on the timestamp of events bridged from PipeWire). Building it needs `libjack-jackd2-dev`. |
| `faust` | yes | the FaustDef family: libfaust embedding (Box API + LLVM JIT, `/def_send faust`). Needs libfaust built with the LLVM backend — see [Contributing](contributing.md). |
| `embed` | no | the C ABI (`clausters_*` exports) for embedding the server in another process — see [Local transports & embedding](ipc.md). |

## Offline rendering (the simplest entry point)

`server::render` turns a score into samples. A `Score` is the binary score format (or built in memory from `(time, messages)` pairs); `RenderConfig` picks the rate, channels and worker count.

```rust,ignore
use clausters::server::render::{render_to_wav, render_to_vec, RenderConfig, Score};

let score = Score::load("score.osc")?;          // or Score::from_bytes / Score::new
let cfg = RenderConfig { sample_rate: 48_000.0, channels: 2, workers: 0 };

// Straight to a WAV file ("int16" | "int24" | "float"):
let stats = render_to_wav(&score, &cfg, "out.wav", "float")?;

// …or to an interleaved Vec<f32> for further processing / tests:
let (samples, stats) = render_to_vec(&score, &cfg)?;
# Ok::<(), String>(())
```

Rendering is deterministic and bit-identical to a live take of the same score (and to the parallel path with `workers > 0`).

## Driving the live engine directly

`engine_pair(sample_rate, channels)` returns the two halves of the design:

- **`Engine`** — the audio side. Call `process_block(&mut [f32])` to fill an interleaved output buffer; this is what the cpal callback calls. It must not allocate, so build everything before handing it over.
- **`EngineHandle`** — the control side. `send(Cmd)` pushes a fully-built command over the lock-free FIFO; `collect_garbage()` drains freed memory (drop it here, off the audio thread); `counters()` exposes node/ugen counts.

```rust,ignore
use clausters::server::engine::{engine_pair, Cmd};
use clausters::osc::translate::CmdTranslator;
use clausters::rosc::OscMessage;

let (mut engine, mut handle) = engine_pair(48_000.0, 2);

// Build commands from OSC messages with the same translator the server uses
// (it resolves def names, allocates synths, keeps the tree mirror):
let mut translator = CmdTranslator::new(48_000.0);
let mut cmds = Vec::new();
let msg = OscMessage { addr: "/synth_new".into(), args: vec![/* … */] };
translator.translate(&msg, &mut cmds)?;
for cmd in cmds { let _ = handle.send(cmd); }

// Audio thread: pull blocks.
let mut out = vec![0.0f32; 64 * 2];
engine.process_block(&mut out);
handle.collect_garbage();
# Ok::<(), String>(())
```

You can also construct `Cmd` values directly (see the rustdoc for the enum) if you don't need OSC. `engine_pair_with_workers` adds a DSP worker pool for `/group_parallel` groups; `engine_pair_full` wires an IPC segment so external processes share the control buses and sample clock.

## The pulled server: the whole server inside your audio callback

Between the raw engine pair (above) and the threaded in-process server (below)
sits `clausters::embed::ClaustersHeadless` (feature `embed`, no `realtime`
needed): the **complete server** — the OSC translator, def machinery, buffer
commands, `/bus_stream`/`/bus_tapStream`, `/server_sync`, everything — with **no device,
no sockets and no threads**. The host owns the audio thread and pulls,
callback-style:

```rust,ignore
use clausters::embed::ClaustersHeadless;

let mut server = ClaustersHeadless::new(48_000.0, 2, unix_now_secs)?;
server.send(&osc_packet_bytes);        // complete OSC packets, any command
let mut out = vec![0.0f32; 64 * 2];
server.process_block(&mut out)?;       // a serving turn + the engine blocks
let mut buf = vec![0u8; 64 * 1024];
while let Some(len) = server.poll_into(&mut buf) { /* replies */ }
# Ok::<(), String>(())
```

Three properties define the mode:

- **Time follows the sample clock.** Stream periods and bundle timetags are
  measured on the engine's sample axis (anchored at `unix_epoch`, the third
  constructor argument), not the wall clock — so an offline or faster-than-
  real-time drive stays deterministic, and a paused callback pauses time.
- **NRT work runs inline.** `/buffer_alloc` and friends execute on the calling
  thread in submission order; `buffer_load(index, channels, sample_rate, samples)`
  installs host-decoded samples directly — the `/buffer_allocRead` replacement for
  hosts without a filesystem (a browser page decodes with `decodeAudioData`
  and hands the engine the floats).
- **Not RT-strict.** The serving turn allocates on the calling thread (the
  translate/NRT work the threaded server does elsewhere). A host that needs
  the native no-alloc callback keeps using `Clausters` or the full server;
  the pulled mode trades that guarantee for having no thread of its own.

This is the exact shape the browser build runs (the AudioWorklet is the
caller; `crates/clausters-web` wraps this type 1:1 as `WebServer`, and the
web package's `src/engine/worklet.ts` + `src/engine/loader.ts` are the
browser host: the wasm module is compiled on the main thread, instantiated
synchronously in the worklet, OSC bytes travel over the MessagePort — see
`clients/web/examples/engine.html` for a complete page), and it is just as
usable natively — a plugin process or a test that wants the whole protocol
in one thread.

## Embedding in another process (C ABI)

With the `embed` feature the `cdylib` exposes a versioned C ABI (`clausters_render` for synchronous offline renders, plus a live in-process server: `clausters_open` / `send` / `poll` / `clock` / `ctl_*`). The boundary passes only plain `f32`/integers. Full reference, the shared-memory transport, and the stdlib-only Python binding are in [Local transports & embedding](ipc.md).
