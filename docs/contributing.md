# Contributing

Working on Clausters itself. The internals — threads, memory lifecycle, invariants, how to add a UGen — are in [Architecture](architecture.md); this chapter is the practical setup around it.

## Build and test

```sh
cargo build                 # core, no audio device needed to compile
cargo test                  # the full core suite
cargo clippy --all-targets  # lints (kept clean)
cargo doc --no-deps         # the API reference
```

The core **must always build and test without any feature and without libfaust installed**, and with any combination of the def-family features `synth`/`faust` (both, either alone, or neither — most integration suites are gated on `synth`, so the featureless run is thin by design).

## System build dependencies (Ubuntu 26.04)

```sh
# default build (PipeWire audio + ALSA-seq MIDI)
sudo apt install build-essential pkg-config libasound2-dev libpipewire-0.3-dev clang
# only for the matching optional feature:
sudo apt install libjack-jackd2-dev          # --features midi-jack
# plain-ALSA build (no PipeWire libs):
#   cargo build --no-default-features --features synth,realtime,midi
```

`pipewire` is a default feature (the target systems always ship PipeWire), so
the default binary hard-links `libpipewire` and expects it at runtime. `clang`
is only used at build time by bindgen (it parses the PipeWire C headers to
generate bindings); the toolchain itself stays `rustc` + `gcc`. The `midi-jack`
build links against jackd2's `libjack` but resolves to PipeWire's `libjack`
under `pw-jack` at runtime.

## The `faust` feature

`cargo test --features faust` needs **libfaust built with the LLVM backend**. Distro packages (e.g. Ubuntu's `libfaust2t64`) ship without it and without headers, so it is built from source and installed under `~/.local`. The reproducible recipe is in `BUILD.md` (the "Building libfaust from source" section). `build.rs` locates the library through `FAUST_PREFIX`, falling back to `~/.local`, then `/usr/local`.

```sh
FAUST_PREFIX=~/.local cargo test --features faust
```

libfaust **cannot compile concurrently in one process** (it SIGSEGVs), so every compilation FFI call goes through `faust::compiler::ffi_lock()`; instantiating from an already-compiled factory is concurrency-safe.

## Real-time safety (non-negotiable)

`Engine::process_block` and everything it calls must **never allocate, free, lock or do I/O**. Commands arrive fully pre-built over a lock-free FIFO; freed memory leaves through the garbage FIFO and is dropped on the network thread. `tests/rt_safety.rs` guards this with `assert_no_alloc` — run it after touching anything on the audio path. Denormals are flushed to zero on every processing thread (`dsp::denormals::flush_to_zero()` plus `-ftz 2` for Faust); see the RT-safety notes in [Architecture](architecture.md).

## End-to-end testing in a sandboxed shell

Some CI/sandbox environments isolate the network **between** shell invocations: a server started in one invocation is unreachable from the next, and UDP packets to localhost are silently lost. Always run the server and client in the **same** invocation — server in the background with `&`, then the client, then kill it:

```sh
(./target/debug/clausters & PID=$!; sleep 1.5; \
 ./target/debug/examples/osc_ping status quit; kill $PID 2>/dev/null)
```

## OSC decoding

All incoming OSC bytes decode through `osc::decode_packet`, the single entry point (every transport funnels through it). It is a thin wrapper over `rosc::decoder::decode_udp` — keep that one door so decoding and any future hardening stay in one place.

## Continuous integration

GitHub Actions runs the same checks this page asks for by hand
(`.github/workflows/ci.yml`); every job maps to a local command, so a red job
is reproducible with the same line:

- **lint** — `cargo fmt --check` and `cargo clippy --workspace --all-targets
  -- -D warnings`, for the root workspace and for `clients/gui` (its own
  workspace). The tree is clippy-clean; keep it that way.
- **test** — `cargo test --workspace` across the def-family feature matrix:
  default, `--no-default-features`, `--no-default-features --features synth`,
  and default plus `embed`.
- **gui** — `cargo test` in `clients/gui` plus the wasm build gate
  (`clients/gui/check-wasm.sh`).
- **python** — `python clients/python/build_native.py --debug`, then `pytest`
  in `clients/python`.
- **docs** — both mdBooks with the same mdBook version Read the Docs uses,
  and the pydoc-markdown API page for the client book.
- **faust** — `cargo test --features faust -- --test-threads=1`, with
  libfaust built from source at the commit pinned in the workflow
  (the recipe in `third_party/BUILD-FAUST.md`) and cached; a cache hit makes
  the job cheap. Upgrading libfaust = bumping `FAUST_SHA` there after
  verifying locally.

## Releases and publishing

- **Tagged releases** (`.github/workflows/release.yml`): pushing a `v*` tag
  builds the self-contained Python wheel (client + embedded server +
  standalone binary; Linux x86_64 for now) and a server-binary tarball,
  publishes the wheel to PyPI and attaches both to a GitHub release. PyPI
  auth is [Trusted Publishing](https://docs.pypi.org/trusted-publishers/)
  (OIDC — no stored token): the PyPI project must list this repository with
  workflow `release.yml` and environment `pypi` as a trusted publisher, and
  the repository needs a `pypi` environment. There is deliberately **no
  sdist**: the package compiles cdylibs from the Rust workspace, which an
  sdist of `clients/python` would not contain.
- **Read the Docs** hosts the two books as two projects pointing at the same
  repository, each selecting its config under *Settings → Advanced → Path to
  configuration file*: the server/workspace book uses the repo-root
  `.readthedocs.yaml`, the Python client book uses
  `clients/python/.readthedocs.yaml`.

## Conventions

- **Language**: everything under `src/`, `tests/` and `examples/` (code, comments, strings, test names) is in **English**, as are the roadmap files `PLAN.md` / `clients/PLAN.md` and the design record `docs/decisions.md`. `GUIA.md` (root + `clients/python/GUIA.md`) stays in **Spanish** (maintainer-facing manual smoke checklists); this book and the rustdoc are the English documentation.
- **Closing a milestone** means, where applicable: code plus tests, a clear **commit message** (that is the record of *what* shipped — there is no separate per-milestone log), the `PLAN.md` roadmap checkbox updated, developer/user docs where the feature touches them (`docs/architecture.md`, `docs/schemas.md`, module docs), and a commented `examples/` entry for user-facing features. Add a short entry to `docs/decisions.md` **only** when a choice has non-obvious context, and a `GUIA.md` smoke step **only** when a new human-audible/visual behavior appears — neither is a per-milestone obligation.

## Project skills

Domain knowledge lives in `.claude/skills/`: `realtime-audio` (RT thread rules, lock-free patterns, cpal), `scsynth-osc` (the OSC protocol and node-tree model), `ugen-dsp` (UGen DSP algorithms), `audio-testing` (testing audio without ears: NRT, golden files, signal asserts, no-alloc), `faust-embedding` (the libfaust C API and lifecycles), and `faust-language` (writing Faust and transposing it to the Signal/Box APIs — sample-level feedback, physical modeling). Process skills: `clausters-python` (idiomatic client use), `clausters-gui` (the GUI host and GuiDefs) and `documentation` (how to write and place docs — the Diataxis split, the generated API references, and the dev/decision docs).

## Editing this book

The book sources are the Markdown files in `docs/`; `docs/SUMMARY.md` is the table of contents and `book.toml` the config. Build and preview with [mdBook](https://rust-lang.github.io/mdBook/):

```sh
cargo install mdbook
mdbook serve     # live preview at http://localhost:3000
mdbook build     # generates ./book (git-ignored)
mdbook test      # type-checks Rust code snippets
```
