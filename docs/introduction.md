# Clausters

Clausters is a **port of SuperCollider's `scsynth` audio server to Rust**: a real-time audio synthesis server controlled over **OSC** (UDP, default port `127.0.0.1:57110`). A single process opens the audio device, keeps a tree of nodes (synths and groups), and receives OSC commands to create and free synths, set parameters, manage buses and buffers — all with sample-accurate scheduling.

It is **conceptually compatible** with scsynth (same node-tree model, the same `/s_new`, `/n_set`, `/c_set`, `/b_*` commands) but uses its own def formats instead of the binary `.scsyndef`. Its main addition over scsynth is the **FaustDef** — a synth definition written in [Faust](https://faust.grame.fr/) and JIT-compiled by the server — as an alternative to SuperCollider's UGen graphs, which Clausters also supports through its own JSON SynthDef format.

## Highlights

- **Hard real-time audio thread**: no allocation, locks or I/O in the audio callback. Commands arrive pre-built over lock-free FIFOs; freed memory leaves through a garbage FIFO and is dropped off the audio thread. Guarded by `assert_no_alloc` tests.
- **Two def formats, both loaded hot over OSC**: a flat **SynthDef JSON** list of UGens (`/d_recv`, default `synth` feature), and **Faust** defs — Faust source or a JSON box tree — JIT-compiled with the LLVM backend (`/d_faust`, `faust` feature). The two families are independent build features: enable both or ship a single-family server.
- **Sample-accurate scheduling**: NTP-timetagged bundles split the audio block at the event's exact frame, plus a direct **sample clock** (`/clock`, `/sched`) as a drift-free client timebase.
- **Offline (NRT) rendering**: the same engine renders scores to WAV without an audio device, bit-identically to a live take.
- **Auto-sorted groups** (`/g_sortMode`): execution order inferred from the buses each def reads and writes — no manual `/n_before` bookkeeping.
- **Parallel groups** (`/g_parallel` + `--workers N`): independent children run on several cores, **bit-identical** to the sequential result.
- **Control/bus mapping** (`/n_map`, `/n_mapa`): any control or Faust parameter can track a control or audio bus, live, every block.
- **Several transports**: OSC over UDP and **TCP** (both on by default) and **WebSocket** (`--ws`), plus local shared memory (`--shm`) and an in-process **C ABI** for embedding, with the sample clock and control buses readable in mapped memory.
- **Configuration & persistence**: a shared **TOML config file** (user and per-project layers) behind every boot flag, and defs persisted and reloaded across runs (`--data-dir`).

## How to read this book

- **New here?** Start with [Getting started](getting-started.md): build it, run the server, play a sound.
- **Driving the server over OSC?** See [Defs, UGens & the OSC protocol](schemas.md) and the feature chapters (timed bundles & the [sample clock](sample-clock.md), [auto-sorted](auto-order.md) and [parallel](parallel.md) groups).
- **Embedding it in your own program?** See [Using Clausters as a library](using-as-a-library.md) and [Local transports & embedding](ipc.md).
- **Working on Clausters itself?** See [Architecture](architecture.md) and [Contributing](contributing.md).

The API reference for the library crate is the **rustdoc**: `cargo doc --open`.

## License

**GPL-3.0-or-later** (the embedded libfaust is GPLv2+).
