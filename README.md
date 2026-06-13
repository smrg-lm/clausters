# Clausters

A **real-time audio synthesis server** in the style of SuperCollider's `scsynth`, written in Rust — designed and directed, a.k.a. prompted, by Lucas Samaruga and implemented by Claude as a proof of concept. One process opens the audio device, keeps a tree of synths and groups, and receives OSC commands to make sound — with a hard real-time audio thread (no allocation, locks or I/O) and sample-accurate scheduling.

It is conceptually compatible with scsynth (same node-tree model and `/s_new`, `/n_set`, `/c_set`, `/b_*` commands) but uses its own def formats instead of the binary `.scsyndef`.

## Features

- **Real-time safe** audio thread; lock-free command/garbage FIFOs.
- **Two def formats** loaded hot over OSC: a flat **SynthDef JSON** UGen graph (`/d_recv`) and **Faust** defs JIT-compiled with LLVM (`/d_faust`, optional `faust` feature).
- **Sample-accurate scheduling**: NTP-timetagged bundles split the block at the exact frame, plus a direct **sample clock** (`/clock`, `/sched`).
- **Offline (NRT) rendering** to WAV, bit-identical to a live take.
- **Auto-sorted groups** (`/g_sortMode`) and **parallel groups** (`/g_parallel` + `--workers`) from bus-connection analysis.
- **Control/bus mapping** (`/n_map`, `/n_mapa`) for live-driven parameters.
- **Local transports**: shared memory (`--shm`) and an in-process **C ABI**.

> **Status — proof of concept.** This was built in about three days and all code was generated with Claude. The automated test suite passes and covers a fair amount, but only a small portion has been manually reviewed or exercised in real use, and the implementation has **not** been independently audited. Treat it as unaudited: review and verify it before relying on it for anything that matters.

## Quickstart

```sh
# Build and run the server (OSC on UDP 57110, silent until you create a synth)
cargo run --release

# In another terminal: play the built-in sine, retune it, free it
cargo run --example osc_ping -- beep
# …or by hand with oscsend (liblo):
oscsend localhost 57110 /s_new siii default 1000 1 0
oscsend localhost 57110 /n_set  isf  1000 freq 330
oscsend localhost 57110 /n_free i    1000
```

Render a score offline, no audio device needed:

```sh
python3 examples/json_client.py score    # writes /tmp/clausters_score.osc
cargo run --release -- --nrt /tmp/clausters_score.osc /tmp/out.wav
```

## Feature flags

| feature | default | adds |
|---|---|---|
| `realtime` | yes | the cpal audio backend (the live server) |
| `faust` | no | libfaust embedding (Box API + LLVM JIT) — needs libfaust built with the LLVM backend |
| `embed` | no | the C ABI (`clausters_*`) for embedding the server in-process |

The core builds and tests without any feature and without libfaust installed.

## Documentation

- **The book** — full guide, OSC reference and architecture: the mdBook in [`docs/`](docs/) (`mdbook build`, start at [`docs/introduction.md`](docs/introduction.md)).
- **API reference** — `cargo doc --open` (the crate is usable as a library: see [`docs/using-as-a-library.md`](docs/using-as-a-library.md)).
- **Contributing / dev setup** — [`docs/contributing.md`](docs/contributing.md).

## License

**GPL-3.0-or-later** — see [COPYING](COPYING). The embedded libfaust is GPLv2+.
