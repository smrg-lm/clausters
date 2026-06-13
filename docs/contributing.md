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

## The `faust` feature

`cargo test --features faust` needs **libfaust built with the LLVM backend**. Distro packages (e.g. Ubuntu's `libfaust2t64`) ship without it and without headers, so it is built from source and installed under `~/.local`. The reproducible recipe is in the **F0 section of `NOTAS.md`**. `build.rs` locates the library through `FAUST_PREFIX`, falling back to `~/.local`, then `/usr/local`.

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

## Known bug: rosc 0.10.1 blob decoding

rosc's decoder over-reads the padding of blobs whose length is a multiple of 4 and can drop a valid bundle element. The workaround is `osc::decode_packet`, which splits bundles by hand and decodes only leaf messages. Always decode through it; don't go back to `decoder::decode_udp` without verifying rosc has fixed both behaviors upstream.

## Conventions

- **Language**: everything under `src/`, `tests/` and `examples/` (code, comments, strings, test names) is in **English**. `PLAN.md`, `NOTAS.md` and `GUIA.md` are in **Spanish** (maintainer-facing) — this book and the rustdoc are the English documentation.
- **Closing a milestone** means, where applicable: developer docs (`docs/architecture.md`, module docs), user docs in `docs/` for new features, manual-test steps and counts in `GUIA.md`, and a commented `examples/` entry for user-facing features — not just code and `NOTAS.md`.

## Project skills

Domain knowledge lives in `.claude/skills/`: `realtime-audio` (RT thread rules, lock-free patterns, cpal), `scsynth-osc` (the OSC protocol and node-tree model), `ugen-dsp` (UGen DSP algorithms), `audio-testing` (testing audio without ears: NRT, golden files, signal asserts, no-alloc), and `faust-embedding` (the libfaust C API and lifecycles).

## Editing this book

The book sources are the Markdown files in `docs/`; `docs/SUMMARY.md` is the table of contents and `book.toml` the config. Build and preview with [mdBook](https://rust-lang.github.io/mdBook/):

```sh
cargo install mdbook
mdbook serve     # live preview at http://localhost:3000
mdbook build     # generates ./book (git-ignored)
mdbook test      # type-checks Rust code snippets
```
