# Clausters

[![CI](https://github.com/smrg-lm/clausters/actions/workflows/ci.yml/badge.svg)](https://github.com/smrg-lm/clausters/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/clausters)](https://pypi.org/project/clausters/)
[![Server book](https://readthedocs.org/projects/clausters/badge/?version=latest)](https://clausters.readthedocs.io/)
[![Python client book](https://readthedocs.org/projects/clausters-python/badge/?version=latest)](https://clausters-python.readthedocs.io/)

📖 **Documentation:** [server book](https://clausters.readthedocs.io/) ·
[Python client book](https://clausters-python.readthedocs.io/)

Clausters is a **port of SuperCollider's `scsynth` audio server to Rust**: a
real-time audio synthesis server controlled over OSC, with the same node-tree
model and command set. Its main addition over scsynth is the **FaustDef** — a
synth definition written in [Faust](https://faust.grame.fr/) and JIT-compiled
by the server with LLVM — as an alternative to SuperCollider's UGen graphs,
which Clausters also supports through its own JSON SynthDef format.

The project is a workspace with several coordinated parts:

- **`clausters-server`** — the Rust server (this crate), also usable as a
  library and embeddable in-process through a C ABI.
- **`crates/`** — the shared native core (`clausters-core`/`clausters-ffi`:
  numeric builtins, seeded noise and clock math compiled once for server and
  clients) and `clausters-midi` (Standard MIDI File / MIDI 2.0 clip writing and
  live virtual-port output).
- **[`clients/python`](clients/python/README.md)** — the reference client, a
  selective port of SuperCollider's class library covering both def formats:
  patterns and events, routines and clocks, responders, live and offline
  rendering, pip wheels.
- **`clients/gui`** — a GUI host driven by the same OSC protocol (`GuiDef`
  windows, meters and scopes over shared memory, node-tree views, canvas and
  shaders), native and in the browser (WebGPU with a WebGL2 fallback).

## Features

**Engine**

- **Hard real-time audio thread** — no allocation, locks or I/O in the audio
  callback; commands arrive pre-built over lock-free FIFOs and freed memory
  leaves through a garbage FIFO. Guarded by `assert_no_alloc` tests.
- **Sample-accurate scheduling** — NTP-timetagged bundles split the audio block
  at the event's exact frame, plus a direct **sample clock** (`/clock`,
  `/sched`) as a drift-free client timebase.
- **Offline (NRT) rendering** to WAV, bit-identical to a live take, no audio
  device needed.
- **Auto-sorted groups** (`/g_sortMode`) — execution order inferred from the
  buses each def reads and writes — and **parallel groups** (`/g_parallel` +
  `--workers`), bit-identical to the sequential result.
- Flush-to-zero denormal handling on every processing thread.

**Synthesis**

- **Two def formats, loaded hot over OSC**: **FaustDefs** (Faust source, or a
  JSON signal/box tree, JIT-compiled with the LLVM backend; `/d_faust`) and a
  flat **SynthDef JSON** UGen graph (`/d_recv`). They are peers, and both
  families are **on by default** (the `faust` and `synth` Cargo features — a
  default build therefore needs libfaust with the LLVM backend). They are
  independent, so a custom deployment can still build a single-family server
  (see [`BUILD.md`](BUILD.md)).
- A growing UGen library with **first-class calculation rates** (`ar`/`kr`/
  `ir`), typed controls (triggers, lag/varlag, scalars), operator UGens,
  **envelopes with the full scsynth done-action set**, **wavetable oscillators**
  with server-side table generation (`/b_gen`), and a **spectral chain**
  (FFT/IFFT and `PV_*` phase-vocoder filters).
- **Buffers**: multi-format sound-file reading, buffer-info UGens, and
  streaming disk I/O (`DiskIn`/`DiskOut`).
- **Reply UGens** (`SendTrig`/`SendReply`/`Poll`) for data flowing back to
  clients, RT-safely.

**Control**

- The **complete scsynth OSC command set** (node tree, buses, buffers,
  notifications), conceptually compatible with existing scsynth clients except
  for the def formats.
- **Control/bus mapping** (`/n_map`, `/n_mapa`) so any control or Faust
  parameter tracks a bus, live, every block.
- **Standard MIDI**: `--midi` opens a virtual input port; bind channels to defs
  (`/midi_bind`, `/midi_map`) so notes, velocity, aftertouch, pitch-bend and CC
  drive nodes — including a **standalone mode** where bindings persist and
  reload at boot, no client process required.
- **Configuration**: boot-time sizing of audio I/O and every pre-allocated pool
  (buses, nodes, buffers) via flags or a shared **TOML config file** (user and
  per-project layers), and **def persistence** across runs (`--data-dir`).

**Transports**

- OSC over **UDP** (scsynth-compatible) and **TCP** (both on by default, one
  port), and **WebSocket** (`--ws`, reachable from a browser page).
- Local transports: **shared memory** (`--shm`, with the sample clock and
  control buses readable in mapped memory) and an in-process **C ABI** for
  embedding the server in another program.

## Quickstart

```sh
# Build and run the server (silent until you create a synth)
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

Or drive everything from Python — the package is
[on PyPI](https://pypi.org/project/clausters/) as a self-contained wheel
(client + server + libfaust; Linux x86_64 for now), or installs from this
checkout:

```sh
pip install clausters             # from PyPI, or:
pip install ./clients/python      # from this checkout (builds the Rust side)
python clients/python/examples/live_udp.py
```

## Building, testing and documentation

**[`BUILD.md`](BUILD.md)** collects the full development setup: system build
dependencies, the feature-flag matrix (the `synth`/`faust` def families,
PipeWire/ALSA/JACK audio, MIDI, embedding), how to run the test suites, and
how to build both documentation books.

The short version (Ubuntu):

```sh
sudo apt install build-essential pkg-config libasound2-dev libpipewire-0.3-dev clang
cargo build --release
cargo test
```

## Documentation

Two books, one per platform, both published on Read the Docs:

- **[Server / workspace book](https://clausters.readthedocs.io/)** — full
  guide, OSC reference and architecture.
- **[Python client book](https://clausters-python.readthedocs.io/)** — the
  client's guide plus an API reference generated from its docstrings.

Both are mdBooks kept in this repository (Markdown, ReadTheDocs-deployable), so
they also build and read offline:

- **Server / workspace book** — the mdBook in [`docs/`](docs/), starting at
  [`docs/introduction.md`](docs/introduction.md). Build with `mdbook build`.
- **Python client book** — [`clients/python/docs/`](clients/python/docs/); see
  [`clients/python/README.md`](clients/python/README.md#documentation) for the
  build (its API page is generated from the docstrings).
- **Crate API reference** — `cargo doc --open` (the crate is usable as a
  library: see [`docs/using-as-a-library.md`](docs/using-as-a-library.md)).
- **Contributing / dev setup** — [`docs/contributing.md`](docs/contributing.md).

## License

**GPL-3.0-or-later** — see [COPYING](COPYING). The embedded libfaust is GPLv2+.
