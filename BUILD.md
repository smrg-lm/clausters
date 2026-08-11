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

# only to stage the Python package's native artifacts (build_native.py, and so
# scripts/refresh-bin.sh and `pip install ./clients/python`):
sudo apt install patchelf
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
| `synth` | yes | the **SynthDef family**: the UGen library, the def compiler (`/def_send synth`) and `UGenSynth` |
| `realtime` | yes | the cpal audio backend (the live server) |
| `midi` | yes | live MIDI input via midir (ALSA seq on Linux) |
| `pipewire` | yes | native PipeWire audio backend on Linux/BSD (cpal's pipewire host, ALSA fallback at runtime) — needs `libpipewire-0.3-dev` and `clang` |
| `rtprio` | yes | real-time scheduling for the audio callback thread (SCHED_FIFO/RR via RTKit over DBus — the standard path for Linux audio clients; needs `libdbus-1-dev`), plus the `--pin` CPU-affinity flag and a SIGXCPU guard: if RTKit's `RLIMIT_RTTIME` watchdog fires under sustained overload, the audio thread is demoted back to SCHED_OTHER (the audio degrades, the server survives). Without the feature the callback runs as SCHED_OTHER and scheduling jitter breaks the audio at roughly half capacity — drop it only for minimal builds (no DBus dep; Linux-specific code isolated in `server::rt`) |
| `faust` | yes | the **FaustDef family**: libfaust embedding (Signal/Box API + LLVM JIT, `/def_send faust`) — needs libfaust built with the LLVM backend (below) |
| `midi-jack` | no | route live MIDI through midir's JACK backend instead of ALSA (for PipeWire-native MIDI routing) — needs `libjack-jackd2-dev`, run under `pw-jack` |
| `embed` | no | the C ABI (`clausters_*`) for embedding the server in-process |

`synth` and `faust` are the two **def families**, and **both are on by default**:
they are peers, and a server that can compile only one of them is a partial
product. They are independent and combinable, so a custom build can still ship a
single family. With neither, the engine core (groups, buses, buffers,
transports) still builds and runs, but there are no defs to instantiate: every
`/synth_new` fails, `/def_send synth` and `/def_send faust` reply `/fail` naming the missing
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

# the browser engine (the B track): SynthDef family + the in-memory embed seam
# on wasm32 — no cpal, no libfaust, no sockets. `scripts/check-wasm.sh` gates
# the lean feature sets on this target; `scripts/parity-web.sh` renders a score
# through the wasm build (crates/clausters-web) and compares it to the native
# NRT render; `scripts/smoke-web.sh` runs the live engine in an AudioWorklet
# under headless Chrome (/server_status round trip, clock advance, audible sine).
# One-time setup: rustup target add wasm32-unknown-unknown, and
# cargo install wasm-bindgen-cli at Cargo.lock's wasm-bindgen version.
cargo check --lib --target wasm32-unknown-unknown --no-default-features --features synth,embed
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

libLLVM does not stop at itself, though: it links libxml2, libzstd, libedit,
libz, libffi and libtinfo (and *their* deps), none of which are ours and none
guaranteed on the target — and their sonames drift between distro generations
(a wheel built where LLVM linked `libxml2.so.2` won't load on a host that only
ships `libxml2.so.16`). So `build_native.py` vendors the **whole transitive
closure** of libfaust/libLLVM into `_libs/` (minus the glibc/`libgcc_s`/`libstdc++`
baseline) and rewrites each vendored library's run path to `$ORIGIN` with
**patchelf** — a build-time requirement on Linux. The rewrite is essential
because libLLVM uses `DT_RUNPATH`, which is *not* inherited down the chain, so
the binary's `$ORIGIN/../_libs` never reaches libLLVM's own dependencies; giving
each vendored lib its own `$ORIGIN` (they all sit together in `_libs/`) makes the
graph resolve locally. The release wheel is a plain `python -m build`, with no
auditwheel/repair step, so this staging *is* the relocation.

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

### libverovio (the notation engraver, behind the `verovio` feature)

The `score` widget engraves through **libverovio**, which the Python package
bundles the same way it bundles libfaust: built from a pinned source into a
prefix, then staged into `clausters/_libs/` by `build_native.py`.

```sh
third_party/build-verovio.sh          # into ~/.local; needs only cmake, make, a C++20 compiler
python clients/python/build_native.py # stages it (and everything else) into the package
```

`clausters-notation` links it, behind the **`verovio` feature, off by default**
— a plain `cargo build --workspace` links no libverovio and that crate is empty,
exactly as a SynthDef-only server carries no libfaust. `clausters-ffi` re-exports
the C surface behind the same feature (which pulls `notation`, the pure half in
`clausters-core`), and the wheel turns it on; `build.rs` finds the library
through `VEROVIO_PREFIX`, falling back to `~/.local` then `/usr/local`. The GUI
host still links nothing: it draws a display list and knows nothing about MEI.

Two things separate it from libfaust. It needs no LLVM and no submodules —
verovio vendors its dependencies in-tree. And it is **built from a `develop`
commit rather than a release**, because the score editor is dead in 6.2.1 (a
guard typo upstream fixed after it) — which is also why the published PyPI
module is not an option.

The full recipe, the importer trim and the upstream bug are in
[`third_party/BUILD-VEROVIO.md`](third_party/BUILD-VEROVIO.md).

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

## Release

### Cutting a release (CI → PyPI + RTD)

A release is **driven entirely by pushing a `v*` tag** — that fires
`.github/workflows/release.yml`; nothing is done by hand on PyPI or Read the
Docs. Steps:

1. **Pick the tier** — major / minor / patch — per *Versioning* below. (In our
   Spanish workflow: "hacé una release" plus the tier — mayor / menor / bugfix.)
2. **Bump `version` in lockstep** across every package — the root `Cargo.toml`,
   `crates/clausters-{core,ffi,midi,notation,core-web,web}`, `clients/gui`,
   `clients/python/pyproject.toml` and `clients/web/package.json` (`npm version
   X.Y.Z --no-git-tag-version`, which also moves `package-lock.json`) — and
   refresh both lockfiles (the root `Cargo.lock` and `clients/gui/Cargo.lock`,
   e.g. `cargo update -w --offline`). The web package's checker refuses a
   version that disagrees with the crate's, so a miss there fails the release
   rather than shipping a mismatch.
3. If a binary boundary changed this cycle, bump the matching ABI counter (the
   linkage rule below).
4. **Commit** (`release: vX.Y.Z`) and **push to `main`**.
5. **Tag and push**: `git tag vX.Y.Z && git push origin vX.Y.Z`. The tag triggers
   `release.yml`, whose four jobs:
   - **build** — the self-contained wheel (client + embedded server + standalone
     binary + bundled libfaust/libLLVM + libverovio) and a server-binary tarball
     (Linux x86_64); the tarball version comes from the tag, the wheel version
     from `pyproject.toml` (hence step 2). Both third-party libraries are
     restored or built by their composite actions
     (`.github/actions/{libfaust,verovio}`), and the job sets
     `CLAUSTERS_REQUIRE_COMPLETE=1` — refuse every `CLAUSTERS_SKIP_*`, so no
     release can publish a wheel missing a piece that raises on the user's
     machine.
   - **publish-npm** — builds the web package (the wasm bundles with the
     lockfile-pinned `wasm-bindgen` CLI, then the emit), passes its checker and
     publishes `clausters` to npm with provenance, authenticated by the
     `NPM_TOKEN` of the `npm` environment.
   - **publish-pypi** — publishes the wheel to PyPI via Trusted Publishing (OIDC,
     the `pypi` environment, no stored token). It **waits for the npm job**:
     publishing is one-way, so the release goes out whole or not at all, and
     the npm leg carries the long build that can still fail.
   - **github-release** — creates the GitHub release with generated notes and
     attaches the wheel and the tarball. It waits for both registries, so the
     announcement never precedes what it announces.
6. **Read the Docs** rebuilds the three books from the push/tag webhook (each
   project selects its own `.readthedocs.yaml`); activate the new version in
   each RTD project if it is not set to build tags automatically. A book whose
   config broke in the tagged commit builds `latest` fine and fails `stable`
   until the next tag — CI builds all three the same way for that reason.

### Versioning and the ABI counters

Three version numbers answer different questions; keep them distinct:

| number | where | contract | checked |
|---|---|---|---|
| package **SemVer** | `version` in `Cargo.toml` (+ the Python wheel) | source/package — what `cargo`/`pip` resolves | at resolve/install time |
| embed / IPC **`ABI_VERSION`** | `src/server/ipc.rs`, via `clausters_abi_version()` | shm segment layout + embed C ABI | at runtime, on attach |
| core FFI **`CORE_ABI_VERSION`** | `crates/clausters-ffi`, via `clausters_core_abi_version()` | the language-agnostic C surface | at runtime, on load |

The two integer counters — not SemVer — are the **source of truth for binary
compatibility**: monotonic, bumped only when their own boundary changes
incompatibly, and verified at runtime by an already-compiled peer, which refuses
to connect on a mismatch. SemVer governs the package, never the wire.

**Release rules:**

1. **Pre-1.0 (major `0`)** the **minor** is the breaking tier (standard SemVer —
   the minor acts as the major). *Any* incompatible change (source API **or**
   binary boundary) bumps the minor; additive/corrective changes bump the patch.
2. Bump `ABI_VERSION` / `CORE_ABI_VERSION` **only** when that boundary changes
   incompatibly, independently of SemVer.
3. **Linkage (one-way):** a release that bumps either counter **must** bump
   SemVer's breaking tier (minor pre-1.0, major post-1.0). The reverse does not
   hold — a minor can ship purely additive work without touching a counter.
4. At **`1.0.0`** the standard post-1.0 semantics take over (major breaks, minor
   adds, patch fixes); the counters keep their role.

The current tree is `0.1.0`; the next tag is `0.2.0`. Rationale for the decouple
is in `docs/decisions.md`.

### The Python wheel is feature-complete

The wheel deliberately ships **every documented usage mode** — size is traded
for completeness so anyone can `pip install` it and exercise the whole surface
with nothing else on the machine. `clients/python/build_native.py` stages four
artifacts, each with the features its mode needs:

| usage mode | artifact | features |
|---|---|---|
| standalone server (networked / shm), the `clausters` command | `clausters` binary | crate defaults |
| in-process embedded server (the embed C ABI) | `libclausters` cdylib | defaults **+ `embed,realtime`** |
| the language-agnostic core FFI (ctypes/N-API/wasm) | `libclausters_ffi` cdylib | its crate defaults |
| the visual / GUI server | `clausters-gui` binary | its crate defaults |

The crate defaults (`synth, faust, realtime, midi, pipewire, rtprio`) carry
**both def families**, live audio, ALSA-seq MIDI and RT scheduling into every
artifact, and `libfaust` + `libLLVM` are bundled alongside (the ~50 MB noted
above) so a FaustDef JIT-compiles on a clean install. `libverovio` and its SMuFL
resource data ride along for the same reason — the `score` widget engraves and
edits notation on that clean install, and the client keeps `dependencies = []`. `--features embed,realtime`
*adds* to the defaults (only `--no-default-features` replaces them), so the embed
cdylib keeps `faust`, `synth`, `midi`, … too.

The GUI host's **element families** are features too (`notation`, `patcher`),
both on by default, so the wheel's host draws every widget the protocol has; a
build that drops one is an opt-out for a program embedding the crate, not a
packaging choice here (`clients/gui/README.md`).

One feature is deliberately **not** in the wheel: the GUI host's `font-atlas`
(an outline typeface behind a glyph rasterizer, `clients/gui/README.md`). It is
weight for something the embedded bitmap face already does, and it ships no
typeface of its own — a build that wants it points at a face on the machine, so
it is a local build rather than a packaging default. The **web** package makes
the opposite call and compiles it in (`clients/web/build.sh`): a page cannot
rebuild its own wasm, the cost with no face bundled is the rasterizer alone
(+46 KB gzipped), and the page fetches whatever typeface it wants.

### Building a package with a piece left out

`build_native.py` requires both vendored libraries by default and stops on one
line naming the recipe when either is missing — the same answer for both,
because they are built the same way and absent for the same reason. Three
environment variables take a piece out on purpose, for development:

| variable | what the package loses |
|---|---|
| `CLAUSTERS_SKIP_FAUST` | the FaustDef family: every `/def_send faust` fails. Set it to work without building libfaust first |
| `CLAUSTERS_SKIP_SYNTH` | the SynthDef family: no `/def_send synth`, no UGen graphs — a deliberately Faust-only package |
| `CLAUSTERS_SKIP_VEROVIO` | the notation layer: the `score` widget does not engrave |

The two def-family knobs are peers, like the features themselves: either family
can go, both can, and what survives is the engine core (groups, buses, buffers)
where every `/synth_new` fails. They are the only way to drop a *default* feature
from a packaged build — cargo features only add, so this is what emits
`--no-default-features` and names the survivors. `CLAUSTERS_SKIP_SYNTH` has no
library to miss and therefore nothing to probe: it is a preference, not a
fallback.

Three things to hold when cutting a wheel:

- **Do not set `CLAUSTERS_CARGO_FEATURES`** for a release build. It *replaces*
  the embed cdylib's features, so an incomplete value (e.g. omitting `faust`)
  silently ships a trimmed wheel. Leaving it unset keeps the full defaults.
- **Do not set any `CLAUSTERS_SKIP_*`** — each ships a wheel missing a piece
  that fails at the *user's* run time. You do not have to remember: the release
  sets `CLAUSTERS_REQUIRE_COMPLETE=1`, which refuses all three.
- **Do not set `CLAUSTERS_SKIP_GUI_BUILD`** — that yields a light, server-only
  wheel missing the GUI mode.

The only non-default feature left out is `midi-jack`, and by design: it is not a
distinct mode but an alternative MIDI *backend* (the PipeWire-native routing
above) — the "live MIDI" mode is already covered by the default `midi` (ALSA
seq).

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
- **Scheduling** — `--workers <n>` (DSP threads for `/group_parallel` groups).
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

The Python client has its own suite. **pytest** is its one development
dependency — the package itself declares none — and it is declared as the `dev`
group in `clients/python/pyproject.toml`:

```sh
pip install --group ./clients/python/pyproject.toml:dev   # once: pytest
cargo build -p clausters-ffi && cargo build --features embed,realtime
cd clients/python && python -m pytest
```

## Building the documentation

Two mdBooks, one per platform, plus the rustdoc:

```sh
cargo install mdbook --version 0.4.40   # the version CI and Read the Docs use

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
