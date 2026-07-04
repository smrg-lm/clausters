# Building, testing and running Clausters

Everything needed to build the server from source, run the test suites, run
the server with its different options, and build the documentation. For the
project internals and contribution conventions see
[`docs/contributing.md`](docs/contributing.md).

## System build dependencies

A stable Rust toolchain (`rustup` or a distro package) plus, on Ubuntu
(24.04/26.04 — package names are similar on other distros):

```sh
# default build (PipeWire audio + ALSA-seq MIDI) — also enough for `cargo test`
sudo apt install build-essential pkg-config libasound2-dev libpipewire-0.3-dev clang

# only for the matching optional feature:
sudo apt install libjack-jackd2-dev          # --features midi-jack
```

- `pkg-config` locates the ALSA and PipeWire libraries at build time; without
  it `cargo build` / `cargo test` fail in the `alsa-sys` / `pipewire-sys`
  build scripts.
- `clang` is only used at build time by bindgen (it parses the PipeWire C
  headers); the toolchain itself stays `rustc` + `gcc`.
- The engine core builds and tests **with no system audio library at all** if
  you disable the default features (see below).

## Building the server

```sh
cargo build --release                 # default: PipeWire audio + ALSA-seq MIDI
```

Feature matrix (see `Cargo.toml`):

| feature | default | adds |
|---|---|---|
| `synth` | yes | the **SynthDef family**: the UGen library, the def compiler (`/d_recv`) and `UGenSynth` |
| `realtime` | yes | the cpal audio backend (the live server) |
| `midi` | yes | live MIDI input via midir (ALSA seq on Linux) |
| `pipewire` | yes | native PipeWire audio backend on Linux/BSD (cpal's pipewire host, ALSA fallback at runtime) — needs `libpipewire-0.3-dev` and `clang` |
| `midi-jack` | no | route live MIDI through midir's JACK backend instead of ALSA (for PipeWire-native MIDI routing) — needs `libjack-jackd2-dev`, run under `pw-jack` |
| `faust` | no | the **FaustDef family**: libfaust embedding (Box API + LLVM JIT, `/d_faust`) — needs libfaust built with the LLVM backend |
| `embed` | no | the C ABI (`clausters_*`) for embedding the server in-process |

`synth` and `faust` are the two **def families**. They are independent and
combinable — enable both for a server that mixes UGen and Faust synths on the
same node tree, or ship a single-family binary for a custom build. With
neither, the engine core (groups, buses, buffers, transports) still builds and
runs, but there are no defs to instantiate: every `/s_new` fails, `/d_recv`
and `/d_faust` reply `/fail` naming the missing feature, and persisted defs of
the absent family are skipped with one warning at boot.

Common variants:

```sh
# plain ALSA, no PipeWire libs linked
cargo build --no-default-features --features synth,realtime,midi

# both def families: UGen graphs (/d_recv) + Faust JIT (/d_faust)
FAUST_PREFIX=~/.local cargo build --features faust

# Faust-only custom build (no UGen library, smaller binary)
FAUST_PREFIX=~/.local cargo build --no-default-features --features faust,realtime,midi,pipewire

# engine core only (no audio device, no def family; NRT and core tests still run)
cargo build --no-default-features

# the embeddable cdylib used by the Python client
cargo build --release --features embed,realtime
```

### The `faust` feature

`--features faust` needs **libfaust built with the LLVM backend**. Distro
packages (e.g. Ubuntu's `libfaust2t64`) ship without it and without headers,
so build it from source and install it under `~/.local` — the reproducible
recipe is in `LOG.md` (the libfaust build section). `build.rs` locates it
through `FAUST_PREFIX`, falling back to `~/.local`, then `/usr/local`:

```sh
FAUST_PREFIX=~/.local cargo build --features faust
```

### PipeWire-native MIDI (`midi-jack`)

The default `--midi` build opens an ALSA-seq port (routable with `aconnect`).
For PipeWire-native MIDI routing, the `midi-jack` build links against jackd2's
`libjack`; run the server under `pw-jack` so `libjack` resolves to PipeWire,
which registers a JACK MIDI input port (`clausters:input_0`) you can wire in
qpwgraph:

```sh
cargo build --features midi-jack
pw-jack ./target/debug/clausters --midi
```

(Or activate PipeWire's JACK system-wide via its `ld.so.conf.d` drop-in — see
the `pipewire-jack` package docs — and drop the `pw-jack` prefix.)

## Running the server

```sh
./target/release/clausters              # live server, OSC on UDP 57110
./target/release/clausters --help       # every flag
```

The main groups of options (all detailed in `--help` and in
[`docs/configuration.md`](docs/configuration.md)):

- **Transports** — `--tcp [port]` (length-prefixed OSC over TCP), `--ws [port]`
  (OSC over WebSocket, reachable from a browser), `--shm <path>` (shared-memory
  segment for local clients), `--midi [name]` (virtual MIDI input port).
- **Audio I/O & pools** — `--sample-rate`, `--inputs`/`--outputs`,
  `--audio-buses`/`--control-buses`, `--max-nodes`, `--max-buffers`, and the
  other pre-allocated pool sizes.
- **Scheduling** — `--workers <n>` (DSP threads for `/g_parallel` groups).
- **Persistence** — `--data-dir <dir>` (where defs and MIDI bindings are
  persisted and reloaded at boot), `--no-persist`.
- **Offline render** — `--nrt <score.osc> <out.wav>` with `--rate`,
  `--channels`, `--format` (int16 | int24 | float).

Every flag defaults to the `[server]` section of the shared **TOML config
file** — the user file (`~/.config/clausters/config.toml`) overridden field by
field by the nearest project `clausters.toml`; a command-line flag wins over
both. See [`docs/configuration.md`](docs/configuration.md).

## Running the tests

```sh
cargo test                       # the full core suite (no Faust, no audio device)
cargo clippy --all-targets       # lints (kept clean)
cargo fmt --check                # formatting (rustfmt is the source of truth)
```

The core **must always build and test without any feature and without libfaust
installed** (`cargo test --no-default-features` is part of keeping the build
green). Most integration suites exercise the engine through SynthDefs, so they
are gated on the `synth` feature: the featureless run is thin by design, and
`cargo test --no-default-features --features synth` runs the same suite as the
default build.

With the Faust feature, **run single-threaded** — libfaust/LLVM cannot compile
concurrently in one process, so the default parallel harness SIGSEGVs (a known
libfaust limitation, not a bug in the suites):

```sh
cargo test --features faust -- --test-threads=1
```

Suites worth knowing about: `tests/rt_safety.rs` asserts the audio thread
never allocates (run it after touching anything on the audio path),
`tests/golden.rs` holds bit-exact render references, and `tests/denormals.rs`
guards flush-to-zero.

The Python client has its own suite:

```sh
cargo build -p clausters-ffi && cargo build --features embed,realtime
cd clients/python && python -m pytest
```

## Building the documentation

Two mdBooks, one per platform, plus the rustdoc:

```sh
cargo install mdbook             # once (or a distro / prebuilt mdbook)

# server / workspace book (docs/ -> book/, git-ignored)
mdbook build
mdbook serve --open              # live-reload preview at http://localhost:3000

# crate API reference
cargo doc --open
```

The **Python client book** additionally generates its API page from the
package docstrings with pydoc-markdown:

```sh
uv tool install --python 3.12 pydoc-markdown   # user-space, no sudo (or: uvx / pip install)
clients/python/docs/build.sh                   # writes src/api.md, then mdbook build
```

Details and the reasons for the 3.12 pin are in
[`clients/python/README.md`](clients/python/README.md#documentation).
