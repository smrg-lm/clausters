# Contributing

Working on Clausters itself. The internals — threads, memory lifecycle, invariants, how to add a UGen — are in [Architecture](architecture.md); this chapter is the practical setup around it.

## Build and test

```sh
cargo build                 # core, no audio device needed to compile
cargo test                  # the full core suite
cargo clippy --all-targets  # lints (kept clean)
cargo doc --no-deps         # the API reference
```

The core **must always build and test without any feature and without libfaust installed**.

## System build dependencies (Ubuntu 26.04)

```sh
# default build (PipeWire audio + ALSA-seq MIDI)
sudo apt install build-essential pkg-config libasound2-dev libpipewire-0.3-dev clang
# only for the matching optional feature:
sudo apt install libjack-jackd2-dev          # --features midi-jack
# plain-ALSA build (no PipeWire libs):
#   cargo build --no-default-features --features realtime,midi
```

`pipewire` is a default feature (the target systems always ship PipeWire), so
the default binary hard-links `libpipewire` and expects it at runtime. `clang`
is only used at build time by bindgen (it parses the PipeWire C headers to
generate bindings); the toolchain itself stays `rustc` + `gcc`. The `midi-jack`
build links against jackd2's `libjack` but resolves to PipeWire's `libjack`
under `pw-jack` at runtime.

## The `faust` feature

`cargo test --features faust` needs **libfaust built with the LLVM backend**. Distro packages (e.g. Ubuntu's `libfaust2t64`) ship without it and without headers, so it is built from source and installed under `~/.local`. The reproducible recipe is in `LOG.md` (the libfaust build section). `build.rs` locates the library through `FAUST_PREFIX`, falling back to `~/.local`, then `/usr/local`.

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

## Conventions

- **Language**: everything under `src/`, `tests/` and `examples/` (code, comments, strings, test names) is in **English**, as are the dev-history files `PLAN.md`, `clients/PLAN.md` and `LOG.md`. `GUIA.md` (root + `clients/python/GUIA.md`) stays in **Spanish** (maintainer-facing QA checklists); this book and the rustdoc are the English documentation.
- **Closing a milestone** means, where applicable: developer docs (`docs/architecture.md`, module docs), user docs in `docs/` for new features, manual-test steps and counts in `GUIA.md`, and a commented `examples/` entry for user-facing features — not just code and `LOG.md`.

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
