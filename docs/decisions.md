# Design decisions & findings

The non-obvious choices behind Clausters, and the upstream bugs we had to work
around — the "why it is this way" that the code alone cannot explain. Each entry
is written in ADR spirit: **context → decision → consequence**.

This is a curated record, not a changelog. *What* shipped and when lives in the
git history; the full historical build journal (kept for reference, frozen) is
[`history/build-log.md`](history/build-log.md). When a milestone makes a choice
with non-obvious context, add a short entry here — not a per-milestone diary.

## Threading and the RT-safe boundary

The OSC network thread may allocate, lock and do I/O freely; the audio thread
(the cpal callback, or `render()` in NRT) may do none of those. Everything the
audio thread needs arrives **fully pre-built over lock-free FIFOs**, and freed
memory leaves through a garbage FIFO to be dropped on the network thread.

- **Defs and their control-name tables live only on the network thread.** The
  `HashMap<String, Arc<SynthDef>>` and a mirror `node_id → Arc<SynthDef>` (kept
  from `/s_new`, cleaned on `Garbage::Freed`) stay off the audio thread, so
  `Cmd::SetControl` is plain-old-data and the audio thread never compares
  strings. `/d_free` only removes the map entry — live synths keep their `Arc`
  (exact scsynth semantics).
- **UGen wiring allocates nothing.** `UGenSynth::process` builds each UGen's
  inputs in a fixed stack array (`MAX_UGEN_INPUTS`) via `split_at_mut` over the
  wires; the topological order guarantees inputs only read earlier wires.
  Guarded by `assert_no_alloc`.
- **Asynchronous command semantics are deliberate.** A `/status` immediately
  after a command may report the old count: commands apply at the start of the
  next block. That is scsynth's model, not a race.

## `SynthNode`: one trait, symmetric def families

The node tree and FIFOs handle `Box<dyn SynthNode>`, never a concrete synth
type. This was introduced before it was strictly needed so that a second def
family (Faust) could join the tree without touching the engine or the tree.
Consequence: the `synth` (SynthDef/UGen) and `faust` (FaustDef) families are
independent Cargo features that combine freely — new work feature-gates against
`dyn SynthNode` and stays symmetric.

## Buses, execution order, and the network-thread mirror

- **Control buses bypass the command FIFO.** `/c_set`/`/c_get` operate directly
  on shared atomics from the network thread; a synth sees the change on its next
  block — the same effect as routing through the FIFO, without the traffic.
- **Execution order is audible and testable.** `Out` sums, `ReplaceOut`
  overwrites; the order tests use a `ReplaceOut(0.0)` "silencer" that wins or
  loses a bus depending on whether it runs after or before the source.
- **The mirror can run ahead of the engine.** The auto-order mirror reflects
  commands *when sent*, so a scheduled future bundle is already mirrored;
  `queryTree` may briefly show that future state and a re-sort converges on the
  next change. Correctness never depends on the mirror (see parallel groups).

## Parallel groups are bit-identical to sequential

The safety of parallelism must not depend on the network mirror (which can run
ahead). So `BusUsage` masks travel *to the engine* inside `Cmd::AddSynth` (and
are re-sent by `Cmd::SetUsage` when a `/n_set` touches a control used as a bus
index), and stage partitioning happens on the audio thread with its own data —
pure bitops, no allocation. A greedy per-block rule in child order closes a
stage on the first bus conflict, so writers to the same bus serialize in order.

Consequence: a stage's members touch pairwise-disjoint buses and never read what
the stage writes, so results are independent of interleaving and the stages
preserve order — **`--workers` changes only wall-clock time**, never the samples.
Golden and RT/NRT-identity tests guard this.

## Denormal protection (three independent pieces)

Subnormals appear in recursive states decaying to zero (filter tails, envelopes,
Faust recursions) and resolve 10–100× slower in microcode on many CPUs — exactly
as a sound fades out. Three guards, kept in lockstep:

1. **`dsp::denormals::flush_to_zero()`** puts the calling thread in
   flush-to-zero mode (MXCSR FTZ+DAZ on x86-64, FPCR.FZ on aarch64, via inline
   asm — the `_mm_setcsr` intrinsics are deprecated). Re-armed in every cpal
   callback **and** armed at the start of `render()`: FTZ changes results, so
   NRT must arm it too to stay sample-identical to live.
2. **`-ftz 2` in every Faust factory** flushes recursive variables below the
   normal range, independent of architecture and thread FPU mode.
3. The one exception: the Faust *compiler* path runs inside
   `dsp::denormals::normal_precision` — libfaust's interval typing aborts under
   FTZ/DAZ, and the guard restores the armed mode on exit.

Guarded by `tests/denormals.rs` and the Faust-tail test in `tests/golden.rs`.

## Faust embedding: decisions and upstream bugs

- **Ubuntu's `libfaust` is unusable for embedding** — built without the LLVM
  backend and shipped without headers. libfaust must be built from source with
  the LLVM backend; the reproducible recipe lives in `BUILD.md`.
- **A hand-written FFI, not a crate.** `faust-build`/`faust-types` do
  Faust→Rust codegen at build time (they need the `faust` compiler and static
  DSP source) and do not embed the JIT; there is no maintained libfaust binding.
  We bind ~30 functions by hand against the real headers, avoiding a libclang
  build dependency. Because dynamic linking is lazy, a mistyped FFI symbol only
  fails when called — so a "kitchen sink" test exercises every schema op once.
- **libfaust does not tolerate concurrent compilation.** Two compiler threads in
  one process SIGSEGV — Faust's global compiler state is not thread-safe, even
  for `createCDSPFactoryFromString`. Fix: a process-global `compiler::ffi_lock()`
  around every compilation call. A server has a single compiler thread, but the
  test harness (and any multi-server embedder) needs the lock — which is why the
  faust suites must run `--test-threads=1`.
- **Instantiation does *not* take the lock.** Creating instances from an
  already-compiled factory is independent of the compiler's global state (JIT
  code + malloc; FaustLive does it concurrently with compilations). The lock
  stays only for compiling. (Open caveat: `deleteDSPFactory` may touch the global
  factory table and can flake under parallel test load — non-blocking for a real
  single-compiler server.)
- **Upstream bug — `boxFmod()` returns `abs`.** `boxFmod()` in
  `compiler/box_signal_api.cpp` returns the abs primitive (a copy-paste bug,
  present through master-dev). Workaround: build `fmod` from a
  `CDSPToBoxes("process = fmod;")` fragment instead of the binding.
- **Upstream bug — the `cos` box returned abs** likewise; fixed the same way.

## Def persistence: transparent JSON + a non-authoritative bitcode cache

Two layers, decided with the user:

- **B — JSON is the transparent source of truth.** `synthdefs/<name>.json` is the
  `SynthDefSpec` verbatim; `faustdefs/<name>.json` is a `FaustRecord`
  (source/JSON + libfaust version + payload sha256). Reload = recompile from
  there, by the same path as a fresh `/d_recv`/`/d_faust`. The `FaustDef` itself
  is never serialized — its factory is opaque LLVM JIT state.
- **A — the LLVM bitcode cache is non-authoritative.** `faustdefs/<name>.<sha16>.bc`
  is restored only if the libfaust version matches and the file reads well; any
  miss recompiles from source and rewrites it. A libfaust upgrade invalidates
  every `.bc` automatically, and a corrupt cache never serves a wrong def
  (named by payload sha, so a stale `.bc` never pairs with a newer record).

## MIDI: standard channel-voice, byte-identical to OSC, in a shared crate

- **A MIDI voice realizes the *same* OSC commands an OSC client would send.**
  `CmdTranslator::translate_midi` maps note-on → `/s_new` (with `freq`/`amp`
  from named conversions), note-off → `/n_free` or `/n_set gate 0`,
  aftertouch/CC/bend → `/n_set` on live voices. Reusing the OSC path makes a MIDI
  voice byte-identical to its OSC equivalent, and the reserved voice-ID range
  (`MIDI_NODE_ID_BASE`) stays disjoint from client and `/s_new -1` IDs.
- **MIDI lives in a reusable native crate, not the Python client.** The original
  plan (MIDI 1.0 in a Python library) was scrapped: MIDI belongs in
  `crates/clausters-midi` (a versioned C ABI, shared by client and server), with
  the message layer in **MIDI 2.0/UMP** for high resolution (16-bit velocity,
  32-bit controllers, no 7-bit loss), persistence to a MIDI 2.0 clip file and to
  `.mid` (SMF) for interop.

## One global transport; the server broadcasts *control*, not audio

Decided with the user: a single global transport, and a conductor's
play/stop/locate drives every client's playhead in lockstep. The server
broadcasts transport *control* (`/transport_play|stop|locate`, pushed to
`/notify` clients) and **never schedules audio** — each client rolls its own
playhead on the shared grid. The `/transport.reply` grew `playing` and
`position` fields, kept backward-compatible (older clients read the first three).

## One seeded random context per script

Per-pattern seeds were a design error (corrected with the user): a piece is only
reproducible end-to-end if everything random shares **one** seedable context —
the sclang model. `main.seed(n)` seeds the root; every `Routine`/`Stream` derives
its own generator at creation from the creating context; a draw uses the running
routine's generator (thread-local), falling back to the root outside a routine.
One root seed reproduces a whole script in creation order, and concurrent
routines stay reproducible *per routine* regardless of wake interleaving. The
generator itself is the core's seeded `Rng` over the FFI, so a seeded script
replays identically in every client language.

## Real-time scheduling: opt-in, then restored as default

A two-step decision worth recording because it reversed:

- **First made opt-in (M24b):** the default `rtprio` promotion tripped RTKit's
  `RLIMIT_RTTIME` watchdog under sustained overload and the kernel killed the
  process with SIGXCPU — a silent death that hung clients. Overload must break
  the audio, never the process.
- **Then restored as default (M24c):** with a SIGXCPU guard in place (sustained
  overload demotes the audio thread to SCHED_OTHER — audio degrades, server
  survives), only the performance cost of *not* scheduling RT remained.
  Measured on one machine: ~300 stable 1-sine nodes as SCHED_OTHER (124% peak at
  36% avg) vs ~500+ as SCHED_RR (92% peak at 34% avg) — the average misleads,
  the callback must fit its **worst** block. An RT-scheduled callback is the
  standard operating mode of every production Linux audio client, so the default
  came back, kept safe by the guard.

## Editor y-axis navigation: gestures on the ruler strip, props in display units

The editor-grade views (waveform, spectrogram) zoom and pan vertically. Two
choices were on the table and both are worth recording:

- **Gesture surface: the y-ruler strip, not a modifier over the body.** The
  wheel over the strip zooms the vertical axis (anchored at the cursor's
  height within its lane) and dragging the strip pans it; the wheel over the
  body stays horizontal zoom and plain/Shift drag keep selecting/panning time.
  Spatial separation needs no modifier chord, leaves every existing body
  gesture untouched, and matches the audio-editor convention (Audacity, most
  DAWs pan/zoom an axis by grabbing its ruler). The strip only exists when the
  ruler is on (`ruler_y != "off"`), which is also the only time the axis has
  visual feedback to navigate against.
- **Prop shape: `y_start`/`y_len` in normalized display units** (0 = axis
  bottom, 1 = top; `0, 1` = no zoom; a non-positive `y_len` resets), on the
  shared `EditorProps` — not amplitude/frequency values. Display coordinates
  make the anchor math linear and unit-independent: the spectrogram's window
  survives switching `freq_scale` between linear/log/mel/bark without moving
  what is on screen (the nonlinearity lives entirely in the shader's
  display→bin mapping, as with the shader uniforms), and the waveform's
  amplitude window composes with `AMP_MARGIN` without leaking it into the
  protocol. Living in the widget tree means `/gui_set` drives it and the
  browser renders it through the same shared frame path with zero extra wiring
  (display + `/gui_set` parity; drag/wheel gestures stay native for now).
  Changes emit `/gui_event id "view_y" y_start y_len`, the `"view"` posture.

The y state is deliberately **per widget** while the horizontal view is slated
to move into shared navigation groups (linked views): two lanes of one file
scroll in lockstep in time, but each keeps its own vertical slice.
