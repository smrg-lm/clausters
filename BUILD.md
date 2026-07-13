# Building, testing and running Clausters

Everything needed to build the server from source, run the test suites, run
the server with its different options, and build the documentation. For the
project internals and contribution conventions see
[`docs/contributing.md`](docs/contributing.md).

## System build dependencies

A stable Rust toolchain (`rustup` or a distro package) plus, on Ubuntu
(24.04/26.04 — package names are similar on other distros):

```sh
# default build (PipeWire audio + ALSA-seq MIDI + RT scheduling) — also enough for `cargo test`
sudo apt install build-essential pkg-config libasound2-dev libpipewire-0.3-dev clang libdbus-1-dev

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
| `rtprio` | yes | real-time scheduling for the audio callback thread (SCHED_FIFO/RR via RTKit over DBus — the standard path for Linux audio clients; needs `libdbus-1-dev`), plus the `--pin` CPU-affinity flag and a SIGXCPU guard: if RTKit's `RLIMIT_RTTIME` watchdog fires under sustained overload, the audio thread is demoted back to SCHED_OTHER (the audio degrades, the server survives). Without the feature the callback runs as SCHED_OTHER and scheduling jitter breaks the audio at roughly half capacity — drop it only for minimal builds (no DBus dep; Linux-specific code isolated in `server::rt`) |
| `faust` | yes | the **FaustDef family**: libfaust embedding (Signal/Box API + LLVM JIT, `/d_faust`) — needs libfaust built with the LLVM backend (below) |
| `midi-jack` | no | route live MIDI through midir's JACK backend instead of ALSA (for PipeWire-native MIDI routing) — needs `libjack-jackd2-dev`, run under `pw-jack` |
| `embed` | no | the C ABI (`clausters_*`) for embedding the server in-process |

`synth` and `faust` are the two **def families**, and **both are on by default**:
they are peers, and a server that can compile only one of them is a partial
product. They are independent and combinable, so a custom build can still ship a
single family. With neither, the engine core (groups, buses, buffers,
transports) still builds and runs, but there are no defs to instantiate: every
`/s_new` fails, `/d_recv` and `/d_faust` reply `/fail` naming the missing
feature, and persisted defs of the absent family are skipped with one warning at
boot.

The one cost of `faust` being default is a **build dependency**: a plain `cargo
build` now needs libfaust with the LLVM backend on the machine (the next section
builds it, once). Drop the feature to build with nothing installed.

Common variants:

```sh
# plain ALSA, no PipeWire libs linked
cargo build --no-default-features --features synth,faust,realtime,midi

# SynthDef-only server — builds with no libfaust on the system
cargo build --no-default-features --features synth,realtime,midi,pipewire,rtprio

# Faust-only custom build (no UGen library, smaller binary)
FAUST_PREFIX=~/.local cargo build --no-default-features --features faust,realtime,midi,pipewire

# engine core only (no audio device, no def family; NRT and core tests still run)
cargo build --no-default-features

# the embeddable cdylib used by the Python client
cargo build --release --features embed,realtime
```

### The `faust` feature (default)

The FaustDef family needs **libfaust built with the LLVM backend**. Since the
feature is on by default, so does a plain `cargo build`. Distro packages (e.g.
Ubuntu's `libfaust2t64`) ship without the LLVM backend and without headers, so
build it from source and install it under `~/.local` (once — the recipe below).
`build.rs` locates it through `FAUST_PREFIX`, falling back to `~/.local`, then
`/usr/local`:

```sh
FAUST_PREFIX=~/.local cargo build          # the prefix is only needed if it is not ~/.local
```

If you would rather not have the dependency, build the SynthDef-only server:
`cargo build --no-default-features --features synth,realtime,midi,pipewire,rtprio`.

**Relocatable artifacts.** With the feature on, `build.rs` writes a `DT_RPATH`
of `$ORIGIN`, `$ORIGIN/../_libs` and the build-time prefix, in that order. The
`$ORIGIN` entries are what let a distribution (the Python wheel) ship
`libfaust.so` and the `libLLVM.so` it JITs with *beside* the binary and the
cdylibs, so Faust works on a machine with neither installed. It must be
`DT_RPATH`, not `DT_RUNPATH`: only the former is inherited by transitive
dependencies, and libfaust — which carries no rpath of its own — is the one that
needs to find libLLVM. This is also why the Python wheel weighs ~50 MB: libLLVM
*is* the Faust JIT, and it is ~130 MB unpacked (see
`clients/python/build_native.py`).

#### Building libfaust from source (reproducible, no sudo)

Pin the same version the tree is tested against and build the dynamic library
with the LLVM backend:

```sh
# system deps: cmake, an LLVM dev package, libzstd-dev, zlib1g-dev
sudo apt install cmake llvm-20-dev libzstd-dev zlib1g-dev

git clone --depth 1 -b 2.81.10 https://github.com/grame-cncm/faust
cd faust
make most                       # builds the compiler; note: NOT the .so yet

# two cmake cache tweaks in the build dir, then rebuild + install to ~/.local:
#   -DINCLUDE_DYNAMIC=ON        the `most` target skips the shared lib
#   -DLINK_LLVM_STATIC=off      link the monolithic libLLVM.so (no libpolly-*-dev)
#   -DLLVM_CONFIG=llvm-config-20
cmake -DINCLUDE_DYNAMIC=ON -DLINK_LLVM_STATIC=off -DLLVM_CONFIG=llvm-config-20 \
      -S build -B build/faustdir
cmake --build build/faustdir
make -C build/faustdir install PREFIX=$HOME/.local
```

Notes: static LLVM linking (`LINK_LLVM_STATIC=on`) additionally needs
`libpolly-20-dev` and is not used here. The dynamic `libfaust.so` is ~11 MB
against the system `libLLVM.so`; a full build from source takes ~10 min on 8
cores. See [the design record](docs/decisions.md#faust-embedding-decisions-and-upstream-bugs)
for why the distro package cannot be used.

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
cargo test                       # the full suite, both def families (no audio device)
cargo clippy --all-targets       # lints (kept clean)
cargo fmt --check                # formatting (rustfmt is the source of truth)
```

The core **must always build and test without any feature and without libfaust
installed** (`cargo test --no-default-features` is part of keeping the build
green). Most integration suites exercise the engine through SynthDefs, so they
are gated on the `synth` feature: the featureless run is thin by design, and
`cargo test --no-default-features --features synth` runs the same suite as the
default build.

The Faust suites run in the default parallel harness like any other: every
compilation FFI call goes through `faust::compiler::ffi_lock()`, which
serializes libfaust within the process. (Historically they had to run with
`--test-threads=1` because concurrent compilation SIGSEGVed; that is no longer
needed. If a segfault ever reappears in a `faust_*` suite, reach for
`-- --test-threads=1` first — it points at an unlocked FFI path.)

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
