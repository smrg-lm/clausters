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
| `realtime` | yes | the cpal backend (the live server). Disable it for offline/embedded use with no audio device. |
| `faust` | no | libfaust embedding (Box API + LLVM JIT). Needs libfaust built with the LLVM backend — see [Contributing](contributing.md). |
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
// (it resolves def names, allocates synths, keeps the M12 tree mirror):
let mut translator = CmdTranslator::new(48_000.0);
let mut cmds = Vec::new();
let msg = OscMessage { addr: "/s_new".into(), args: vec![/* … */] };
translator.translate(&msg, &mut cmds)?;
for cmd in cmds { let _ = handle.send(cmd); }

// Audio thread: pull blocks.
let mut out = vec![0.0f32; 64 * 2];
engine.process_block(&mut out);
handle.collect_garbage();
# Ok::<(), String>(())
```

You can also construct `Cmd` values directly (see the rustdoc for the enum) if you don't need OSC. `engine_pair_with_workers` adds a DSP worker pool for `/g_parallel` groups; `engine_pair_full` wires an IPC segment so external processes share the control buses and sample clock.

## Embedding in another process (C ABI)

With the `embed` feature the `cdylib` exposes a versioned C ABI (`clausters_render` for synchronous offline renders, plus a live in-process server: `clausters_open` / `send` / `poll` / `clock` / `ctl_*`). The boundary passes only plain `f32`/integers. Full reference, the shared-memory transport, and the stdlib-only Python binding are in [Local transports & embedding](ipc.md).
