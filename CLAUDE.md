# Clausters

A real-time audio synthesis server in the style of SuperCollider's scsynth,
written in Rust and controlled over OSC (UDP, default port 57110).

- Implementation plan and milestones: `PLAN.md` (Spanish).
- Completion notes per milestone: `NOTAS.md` (Spanish) — update it when a
  milestone is finished.
- Developer documentation (threads, memory lifecycle, invariants, how to add
  a UGen): `docs/architecture.md`. User-facing wire formats and OSC
  reference: `docs/schemas.md`. Both in English; keep them current.
- Project skills live in `.claude/skills/` (realtime-audio, scsynth-osc,
  ugen-dsp, audio-testing, faust-embedding).

## Language conventions

- Everything under `src/`, `tests/` and `examples/` (code, comments, strings,
  test names) is in English.
- `PLAN.md`, `NOTAS.md` and conversation with the user are in Spanish.

## E2E testing rule

The Bash sandbox isolates the network between invocations: a server started in
one invocation is unreachable from the next, and UDP packets to localhost are
silently lost. Always run server and client in the **same** Bash invocation
(server in background with `&`, then the client, then kill), e.g.:

```sh
(./target/debug/clausters & PID=$!; sleep 1.5; \
 ./target/debug/examples/osc_ping status vibrato quit; kill $PID 2>/dev/null)
```

## Known bug: rosc 0.10.1 blob decoding

rosc's decoder over-reads the padding of blobs whose length is a multiple of 4
and returns `Eof` on valid packets; inside a **bundle** the failing element is
silently dropped instead (the content is parsed from its own size-prefixed
slice). Workaround: `osc::decode_packet` splits bundles into elements by hand
(recursively) and decodes only leaf messages with rosc, appending 4 zero bytes
first (harmless for well-formed packets — they remain as unparsed remainder).
Always decode through it; do not go back to `decoder::decode_udp` without
verifying rosc has fixed both behaviors upstream.

## Optional `faust` feature

`cargo test --features faust` needs libfaust built **with the LLVM backend**
— Ubuntu's `libfaust2t64` ships without it and without headers, so it is
built from source and installed under `~/.local` (see the F0 section of
`NOTAS.md` for the reproducible recipe). `build.rs` locates it through
`FAUST_PREFIX`, falling back to `~/.local`, then `/usr/local`. The core must
always build and test without the feature and without libfaust installed.

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
