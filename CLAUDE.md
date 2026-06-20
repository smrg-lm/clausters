# Clausters

A real-time audio synthesis server in the style of SuperCollider's scsynth,
written in Rust and controlled over OSC (UDP, default port 57110).

- Implementation plan and milestones: `PLAN.md` (server) and `clients/PLAN.md`
  (client track), both in English.
- Completion log per milestone: `LOG.md` (English) — update it when a
  milestone is finished.
- English documentation is an **mdBook** in `docs/` (`docs/SUMMARY.md` is the
  table of contents, `book.toml` the config, `README.md` the front door; build
  with `mdbook build`, output `book/` is git-ignored). It reuses the existing
  `docs/*.md` in place. Key chapters: developer docs (threads, memory
  lifecycle, invariants, how to add a UGen) in `docs/architecture.md`;
  user-facing wire formats and OSC reference in `docs/schemas.md`; library/
  embedding use in `docs/using-as-a-library.md` and `docs/ipc.md`. The crate
  API reference is the rustdoc (`cargo doc`). Keep all of it current.
- **Closing a milestone always includes, whenever applicable**: the
  developer documentation (`docs/architecture.md`, module docs), the user
  documentation in `docs/` for new features, manual testing steps and
  counts in `GUIA.md`, and a commented/explained example in `examples/`
  when the feature is user-facing — not just code and LOG.md.
- Project skills live in `.claude/skills/` (realtime-audio, scsynth-osc,
  ugen-dsp, audio-testing, faust-embedding, clausters-python).

## Language conventions

- Everything under `src/`, `tests/` and `examples/` (code, comments, strings,
  test names) is in English.
- **Git commit messages are in English** (subject and body), ASCII-only.
- `PLAN.md`, `clients/PLAN.md` and `LOG.md` (the dev-history files) are in
  English. `GUIA.md` (root + `clients/python/GUIA.md`) and the conversation
  with the user are in Spanish.

## Commit workflow

Before generating any commit that touches Rust, run `cargo fmt` (or at least
`cargo fmt --check`) and include the formatting fixes — the tree must be
`cargo fmt --check`-clean. Do not hand-format Rust against rustfmt; rustfmt is
the source of truth. (Likewise keep the build green: the core must compile and
test without the `faust`/`embed` features.)

## E2E testing rule

The Bash sandbox isolates the network between invocations: a server started in
one invocation is unreachable from the next, and UDP packets to localhost are
silently lost. Always run server and client in the **same** Bash invocation
(server in background with `&`, then the client, then kill), e.g.:

```sh
(./target/debug/clausters & PID=$!; sleep 1.5; \
 ./target/debug/examples/osc_ping status vibrato quit; kill $PID 2>/dev/null)
```

## OSC decoding

All incoming OSC bytes — UDP datagrams and IPC ring contents alike — decode
through `osc::decode_packet`, the single entry point (a thin wrapper over
rosc's `decoder::decode_udp`). Keep that one door so decoding and any future
hardening stay in one place.

## Optional `faust` feature

`cargo test --features faust` needs libfaust built **with the LLVM backend**
— Ubuntu's `libfaust2t64` ships without it and without headers, so it is
built from source and installed under `~/.local` (see the F0 section of
`LOG.md` for the reproducible recipe). `build.rs` locates it through
`FAUST_PREFIX`, falling back to `~/.local`, then `/usr/local`. The core must
always build and test without the feature and without libfaust installed.

**Run the faust tests single-threaded:** `cargo test --features faust --
--test-threads=1`. libfaust/LLVM is not safe for **concurrent** compilation in
one process, so the default parallel test harness SIGSEGVs in the faust suites
(`faust_compiler` and friends) — a known libfaust limitation, not a bug in our
code. This only affects the test harness creating factories in parallel: the
server itself compiles on a single thread holding `faust::ffi_lock()`, so
production is unaffected.

## RT-safety (non-negotiable)

The audio thread (`Engine::process_block` and everything it calls) must never
allocate, free, lock or do I/O. Commands arrive fully pre-built over a
lock-free FIFO; freed memory leaves through the garbage FIFO and is dropped on
the network thread. `tests/rt_safety.rs` guards this with `assert_no_alloc`.

Denormals: every processing thread runs in flush-to-zero mode —
`dsp::denormals::flush_to_zero()` is re-armed in the cpal callback and armed
in `render()` (both, so NRT stays sample-identical to RT) — and Faust
factories are compiled with `-ftz 2`. Keep all three call sites if you touch
them; `tests/denormals.rs` and the Faust tail test in `tests/golden.rs` guard
this.
