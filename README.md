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
- **Standard MIDI control**: `--midi` opens a virtual ALSA input port; bind a channel to an instrument def (`/midi_bind`, `/midi_map`) so note on/off, velocity, aftertouch, pitch-bend and CC drive nodes and their `f32` controls — the same system MIDI any controller or DAW speaks. The Python client also **exports** an event pattern to a Standard MIDI File (`.mid`) or a 16-bit-velocity **MIDI 2.0 clip**, and plays it **live** out a virtual OS MIDI port — all via the `clausters-midi` crate.
- **Local transports**: shared memory (`--shm`) and an in-process **C ABI**.

> **Status — proof of concept.** All code was generated with Claude. The automated test suite passes and covers a fair amount, but only a small portion has been manually reviewed or exercised in real use, and the implementation has **not** been independently audited. Treat it as unaudited: review and verify it before relying on it for anything that matters.

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
| `midi` | yes | live MIDI input via midir (ALSA seq on Linux) |
| `pipewire` | yes | native PipeWire audio backend on Linux/BSD (cpal's pipewire host, ALSA fallback at runtime) — needs `libpipewire-0.3-dev` and `clang` |
| `midi-jack` | no | route live MIDI through midir's JACK backend instead of ALSA (for PipeWire-native MIDI routing; avoids the ALSA-seq timestamp panic) — needs `libjack-jackd2-dev`, run under `pw-jack` |
| `faust` | no | libfaust embedding (Box API + LLVM JIT) — needs libfaust built with the LLVM backend |
| `embed` | no | the C ABI (`clausters_*`) for embedding the server in-process |

The target systems always ship PipeWire, so `pipewire` is on by default and the
default binary hard-links `libpipewire`. For a build that runs without PipeWire,
drop it: `cargo build --no-default-features --features realtime,midi` (plain
ALSA). The engine core still builds and tests with no feature at all.

### Build dependencies (Ubuntu 26.04)

```sh
# default build (PipeWire audio + ALSA-seq MIDI)
sudo apt install build-essential libasound2-dev libpipewire-0.3-dev clang
# optional features:
sudo apt install libjack-jackd2-dev          # --features midi-jack
# plain-ALSA build (no PipeWire libs): cargo build --no-default-features --features realtime,midi
```

Audio uses the native PipeWire host directly. For PipeWire-native MIDI routing,
the `midi-jack` build links against jackd2's `libjack`; run the server under
`pw-jack` so `libjack` resolves to PipeWire, which registers a JACK MIDI input
port (`clausters:input_0`) you can wire in qpwgraph:

```sh
cargo build --features midi-jack
pw-jack ./target/debug/clausters --midi
```

(Or activate PipeWire's JACK system-wide via its `ld.so.conf.d` drop-in — see
the `pipewire-jack` package docs — and drop the `pw-jack` prefix.) The default
`--midi` build keeps the ALSA-seq port, routable with `aconnect`.

## Documentation

Two mdBooks, one per platform (both Markdown, ReadTheDocs-deployable). To build
the **server / workspace book** — full guide, OSC reference and architecture,
the mdBook in [`docs/`](docs/):

```sh
cargo install mdbook        # once (or use a distro / prebuilt mdbook)
mdbook build                # render to book/ (git-ignored)
mdbook serve --open         # live-reload preview at http://localhost:3000
```

Start reading at [`docs/introduction.md`](docs/introduction.md).

- **Crate API reference** — `cargo doc --open` (the crate is usable as a library: see [`docs/using-as-a-library.md`](docs/using-as-a-library.md)).
- **Python client book** — the client has its own mdBook (guide + an API reference generated from docstrings); build steps in [`clients/python/README.md`](clients/python/README.md#documentation).
- **Contributing / dev setup** — [`docs/contributing.md`](docs/contributing.md).

## License

**GPL-3.0-or-later** — see [COPYING](COPYING). The embedded libfaust is GPLv2+.
