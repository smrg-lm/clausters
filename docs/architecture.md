# Architecture (developer documentation)

How Clausters is built, where everything lives, and the invariants a change
must not break. User-facing documentation (wire formats, OSC commands) is in
[`schemas.md`](schemas.md); the plan and per-milestone notes are `PLAN.md`
and `NOTAS.md` (Spanish) at the repository root.

## Threads

```
            OSC/UDP            cmd FIFO (SPSC, pre-built commands)
 client ◄────────► network ─────────────────────────► audio (cpal callback)
                   thread  ◄───────────────────────── Engine::process_block
                     │   garbage FIFO + event FIFO
          ┌──────────┴──────────┐
          ▼                     ▼
      NRT thread          Faust compiler thread   (feature `faust`)
   (disk I/O, buffers)    (libfaust JIT, ~10 ms/def)
```

- **Network thread** (`osc::server::OscServer::run`): owns the UDP socket
  (100 ms read timeout — each timeout tick collects garbage and async
  results; 2 ms when an M14 ring is attached, which it also drains every
  iteration), parses every packet — datagram or ring — through
  `osc::decode_packet`, builds
  commands *fully allocated* — boxed synths, pre-reserved group child lists —
  and pushes them into the command FIFO. It also owns all lookup tables: the
  def tables, the node-ID→def mirror, the buffer mirror, and the M12
  **tree mirror** (`osc::graph::TreeMirror` inside
  `osc::translate::CmdTranslator`) — topology, per-node control values and
  bus usage, fed by the same `Cmd` stream the engine gets and rolled back by
  rejection garbage; it answers `/g_queryTree` and drives the auto-sorted
  groups without touching the audio thread. All replies (`/done`, `/fail`,
  `/n_go`/`/n_end`, queries) are sent from here.
- **Audio thread** (the cpal callback, `server::backend`): runs
  `Engine::process_block` on 64-frame blocks. Per block: drain the command
  FIFO, fire scheduled bundles whose time falls inside the block (splitting
  it at the exact sample), walk the node tree in depth-first order, push dead
  memory to the garbage FIFO. It never allocates, locks or does I/O, and
  re-arms `dsp::denormals::flush_to_zero()` on every callback.
- **NRT thread** (`server::nrt`): all `/b_*` work — allocation, WAV
  reading/writing via hound, zeroing. One queue, so commands on the same
  buffer complete in submission order. Produces immutable buffers the network
  thread installs with `Cmd::SetBuffer`.
- **DSP workers** (`server::workers`, M13, opt-in via `--workers N`): a
  fork-join pool the audio thread conducts to process the stages of
  `/g_parallel` groups. Atomic work stealing, bounded spinning, park/unpark
  only across idle gaps; each worker arms flush-to-zero at spawn. With 0
  workers (the default and the whole test suite) the pool is inert and
  everything is sequential.
- **Faust compiler thread** (`faust::compiler`, feature `faust`): JIT
  compilation of `/d_faust` defs. libfaust does not tolerate concurrent
  compilation in one process (SIGSEGV), so every compiling FFI call holds the
  process-wide `ffi_lock()`; instantiating from a finished factory is
  concurrency-safe and happens on the network thread.

**Offline rendering** (`server::render`, the `--nrt` CLI mode) uses no
threads at all: one thread drives both halves of `engine_pair`, runs NRT
jobs and Faust compilations synchronously between blocks (scsynth NRT
semantics), and arms flush-to-zero once at the start. Because scheduled
commands go through the same engine queue as in real time, an offline render
is sample-identical to a perfectly timed live take.

The realtime backend (cpal) sits behind the `realtime` feature (on by
default); the engine itself knows nothing about cpal, which is what makes
the offline mode and the integration tests possible.

## Module map

| Path | Contents |
|---|---|
| `src/server/engine.rs` | The core: `Engine` (audio half), `EngineHandle` (network half), `Cmd`, `Garbage`, the FIFOs, the schedule queue, the sample clock |
| `src/server/backend.rs` | cpal glue: `BlockAdapter` slices arbitrary callback sizes into 64-frame engine blocks (feature `realtime`) |
| `src/server/nrt.rs` | NRT thread, `NrtJob`/`run_job` (also called synchronously by the renderer), WAV format helpers |
| `src/server/workers.rs` | M13 worker pool: stage publish/steal/wait protocol for parallel groups |
| `src/server/ipc.rs` | M14: the versioned shared segment — data plane (clock, control buses) + OSC byte rings (`--shm` and embed transports) |
| `src/embed.rs` | M14: the embed C ABI (feature `embed`, exported by the cdylib) |
| `src/server/render.rs` | Offline mode: `Score` (binary scsynth score format), `render`/`render_to_vec`/`render_to_wav` |
| `src/node/mod.rs` | `NodeTree` (fixed slab), `SynthNode` trait, groups, add actions, moves |
| `src/dsp/mod.rs` | `UGen` trait, `ProcessCtx`, buses, the cache-line-aligned `Block` (M10), block/bus-count constants |
| `src/dsp/<ugen>.rs` | One file per UGen family (`sinosc`, `binop`, `io`, `noise`, `buf`) |
| `src/dsp/registry.rs` | `UGenKind`: name parsing, input arity, construction |
| `src/dsp/buffer.rs` | Immutable sample buffers and the engine-side pool |
| `src/dsp/denormals.rs` | Per-thread flush-to-zero (x86-64 MXCSR, aarch64 FPCR) |
| `src/synthdef/` | SynthDef JSON wire format, validation/compilation, `UGenSynth` instance |
| `src/osc/mod.rs` | `decode_packet` — the only entry point for incoming OSC bytes |
| `src/osc/server.rs` | The network thread: socket loop, immediate handlers, replies |
| `src/osc/translate.rs` | `CmdTranslator`: OSC message → `Cmd`, shared by the live server and the renderer; owns the M12 tree mirror |
| `src/osc/graph.rs` | M12: bus-usage analysis, the network-side `TreeMirror`, the stable topological sort behind `/g_sortMode` |
| `src/faust/` | libfaust embedding: hand-written FFI, compiler thread, JSON→Box interpreter (`boxes.rs`), `FaustDef`/`FaustSynth` |
| `src/main.rs` | CLI: realtime server (default) or `--nrt` renderer |

## Memory lifecycle

The rule behind everything: **memory is allocated on the network (or NRT /
compiler) thread, used on the audio thread, and freed back on the network
thread.** The audio thread only moves pointers.

1. A command arrives over OSC. The network thread builds the complete object
   — e.g. `/s_new` boxes a `UGenSynth` with its UGens and wires, `/g_new`
   pre-reserves the child list — and pushes a `Cmd` into the command FIFO.
2. `process_block` drains the FIFO and plugs the object into the tree: O(1),
   no allocation.
3. When a node dies (`/n_free`, replace actions, done semantics later), the
   boxed synth leaves through the **garbage FIFO** as a `Garbage` variant;
   the network thread drops it (`collect_garbage`), updating its mirrors.
4. Rejected commands (duplicate ID, unknown target, full slab/group) come
   back as `Garbage::RejectedSynth`/`RejectedGroup` so the memory still dies
   off the audio thread.

Two shared structures cross threads without the FIFOs:

- **Control buses**: 1024 atomics (`dsp::ControlBuses`). Immediate `/c_set`
  and `/c_get` are served directly on the network thread; the audio thread
  reads them through `InCtl`. A *scheduled* `/c_set` must land on its exact
  sample, so it travels as `Cmd::SetControlBus` instead. With an M14
  segment the backing array lives in shared memory: other processes write
  the same atomics.
- **Buffers**: `Arc<Buffer>`, **immutable once installed**. The NRT thread
  builds them, `Cmd::SetBuffer` swaps them into the engine pool, the
  replaced `Arc` returns as `Garbage::FreedBuffer`. "Mutating" commands
  (`/b_zero`, `/b_read` into an existing buffer) build a replacement instead
  of touching shared memory. The network thread keeps a mirror for
  `/b_query`/`/b_write` and for validation.

### Control/bus mapping (`/n_map`, `/n_mapa`, M11)

`/n_set` writes a control once; `/n_map`/`/n_mapa` make a control **follow a
bus**, re-read at the start of every block. Each synth carries a `ControlMap`
table parallel to its controls (`node::ControlMap`, pre-allocated at build —
`map_control` only flips an entry, never grows it). At the top of `process`,
before any UGen runs, the synth pulls each live mapping into its control/zone:
a control bus value (`/n_map`), or one frame of an audio bus sampled at
control rate (`/n_mapa` — controls are one value per block, and Faust zones
are scalar, so there is no audio-rate control). Writing straight to the
control storage, never through `set_control`, keeps the mapping intact; a
`/n_set` *does* go through `set_control`, which clears the mapping first, so
an explicit set always wins (scsynth semantics). `Cmd::MapControl` carries it
to the engine and is schedulable in bundles like `/n_set`.

This feeds the M12/M13 bus analysis: the network-side mirror records each
node's live maps, and `fold_maps_into_usage` adds an audio map's bus to the
node's `reads` and marks the node a dynamic barrier when a mapped control is
used as a bus index — so auto/parallel groups stay correct under mappings.

### Preallocated capacities and what happens when they fill

Audited in M10: `tests/capacity.rs` overflows each structure on purpose and
pins the behavior below.

| Structure | Capacity | When full |
|---|---|---|
| Command FIFO | 1024 | reply `/fail … command FIFO full` (render mode: abort with the event time) |
| Garbage FIFO | 1024 | spills into a 64-slot holding list retried next block; if that also fills, the memory is **leaked** (`mem::forget`) — leaking is the only RT-safe option left |
| Event FIFO (`/n_go`/`/n_end`) | 2048 | events are POD and best-effort: dropped silently |
| Schedule queue (timed bundles) | 1024 | the bundle is rejected and returned whole as a non-empty `Garbage::SpentBundle` (render mode: abort) |
| Node slab | 1024 (`node::MAX_NODES`) | command rejected → `Garbage::Rejected*` |
| Children per non-root group | 256 | command rejected → `Garbage::Rejected*` |
| Buffer pool | 1024 | `/b_*` validates the index up front and replies `/fail` |
| Audio buses | 128 (`dsp::NUM_AUDIO_BUSES`) | bus-index inputs are clamped per block |
| Control buses | 1024 | out-of-range reads return 0.0, writes are ignored |
| IPC rings (M14) | 64 KiB each | backpressure: `push` fails, the producer retries; nothing is dropped (a full *reply* ring drops the reply with a log — the client stopped draining) |

## Clocks and scheduling

The engine publishes its **sample clock** — samples processed since start —
as an `AtomicU64` (`EngineHandle::current_samples`). The conversion from an
OSC NTP timetag to an absolute sample position happens on the **network
thread** (`timetag_delta_secs` against the system clock, then delta ×
sample rate against the stream clock); the engine itself never looks at wall
time. A timed bundle becomes `Cmd::Schedule { time, cmds }` in a
pre-allocated, stably-sorted queue; when its sample falls inside a block,
the block is genuinely **split** at that frame — `ProcessCtx` carries
`offset` + `frames`, and every UGen/synth processes the sub-range — unlike
scsynth, which quantizes to block boundaries and needs `OffsetOut`. The
spent `Vec` shell of an executed bundle returns through the garbage FIFO
(`Garbage::SpentBundle`) to be freed on the network side.

The NTP conversion is one of **two front-ends** to the same queue: `/sched`
(M8) carries an absolute sample target directly — no wall clock involved —
and `/clock` exposes the counter so clients can model the sample clock as
their master timebase (see `docs/sample-clock.md`). In offline rendering,
score timetags are seconds from render start, and the renderer pushes the
same `Cmd::Schedule` commands — that single shared code path is the
sample-identity guarantee, and it is why scheduling fixes must never fork
between the two modes.

## Invariants — do not break these

1. **The audio thread never allocates, frees, locks or does I/O.**
   `Engine::process_block` and everything it calls — including the M13
   parallel dispatch (atomics, bounded spins, at worst an `unpark`).
   Guarded by `tests/rt_safety.rs` (`assert_no_alloc`); new processing code
   must stay under that umbrella.
2. **Commands arrive fully built.** If a handler needs the audio thread to
   "finish" constructing something, the design is wrong.
3. **All incoming OSC bytes decode through `osc::decode_packet`.**
   Whatever the transport — UDP datagrams and IPC ring contents are equally
   untrusted. rosc
   0.10.1 over-reads the padding of blobs whose length is a multiple of 4
   (top-level: `Err(Eof)`; inside a bundle: the element is **silently
   dropped**). `decode_packet` splits bundles by hand, recursively. Do not
   reintroduce `rosc::decoder::decode_udp` without verifying both fixes
   upstream (see `CLAUDE.md`).
4. **RT and NRT render sample-identically.** Same engine, same schedule
   queue, same FPU mode — flush-to-zero is armed in the cpal callback, in
   `render()` *and* in every DSP worker at spawn, and Faust factories get
   `-ftz 2`. Keep all the call sites (`tests/denormals.rs`, Faust tail test
   in `tests/golden.rs`).
   Corollary (M13): **parallel execution is bit-identical to sequential** —
   a stage only batches children with pairwise disjoint bus usage, verified
   by the engine against its own masks (`tests/parallel.rs`). Never weaken
   the stage partition rule: concurrent same-bus access is not just wrong
   ordering, it is the unsafe contract of `Buses::audio_mut` and of the
   per-slot `UnsafeCell`s in `NodeTree`.
5. **Buffers are immutable once installed.** Replace, never mutate. A
   recording UGen would need a new scheme — design it, don't poke holes.
6. **Synth output goes only through `Out`/`ReplaceOut`** (and the Faust
   reserved `out` mapping). There is no implicit output.
7. **The core builds and tests without the `faust` feature and without
   libfaust installed** (and without `realtime`/cpal: the renderer and the
   whole test suite run deviceless). Everything Faust hides behind
   `#[cfg(feature = "faust")]`.
8. **Binary boundaries are versioned.** The IPC segment layout and the
   embed C ABI share one version constant (`ipc::ABI_VERSION`), checked on
   attach/load; `tests/ipc.rs` pins the layout size. Any layout or C-ABI
   change bumps it — never ship an unversioned boundary (the scsynth
   plugin-ABI lesson).
9. **Determinism in tests.** Golden scenes must be reproducible: no
   wall-clock, no global seeds shared across parallel tests (`WhiteNoise`
   seeds from a global counter — keep it out of golden scenes), tolerances
   per `tests/golden.rs` (1e-4: libm differs across platforms, same machine
   is bit-exact).

## How to add a UGen

Using a hypothetical `Lag` (one-pole smoother, inputs `in` and `time`):

1. **DSP** — new file `src/dsp/lag.rs` (or extend a family file):

   ```rust
   use crate::dsp::{ProcessCtx, UGen, at};

   pub struct Lag {
       y: f32, // state lives in the struct, initialized in new()
   }

   impl Lag {
       pub fn new() -> Self { Self { y: 0.0 } }
   }

   impl UGen for Lag {
       fn process(&mut self, ctx: &mut ProcessCtx, inputs: &[&[f32]], output: &mut [f32]) {
           for i in 0..output.len() {
               let x = at(inputs[0], i);          // block or single-sample input
               let t = at(inputs[1], i).max(0.0);
               let coeff = coeff_for(t, ctx.sample_rate);
               self.y += coeff * (x - self.y);
               output[i] = self.y;
           }
       }
   }
   ```

   Rules: no allocation/locks/I/O in `process` (the struct is built on the
   network thread — allocate there, in `new()`); read inputs with `at()` so
   constants, controls and wires all work; `output.len()` is the slice
   length, **not** `BLOCK_SIZE` — scheduled bundles split blocks. Only
   bus-touching UGens need `ctx.offset` (see `src/dsp/io.rs`): bus slices
   must be indexed at `offset..offset+frames`.
2. **Register** — `src/dsp/registry.rs`: add the `UGenKind` variant and its
   arms in `parse_kind` (wire name), `arity` (input count; must be ≤
   `MAX_UGEN_INPUTS`) and `build`. Declare the module in `src/dsp/mod.rs`.
   That's the whole registration: the synthdef compiler validates arity
   against the registry, so a def with the wrong input count already fails
   in `/d_recv` with a pointed error.
3. **Tests** — a signal-level unit test (render offline, assert on the
   numbers: frequency by zero crossings, RMS, impulse/step response — see
   `tests/synthdef.rs` and the `audio-testing` skill). If the UGen has
   state, also assert it behaves across block splits (see
   `tests/scheduling.rs` patterns). RT-safety is covered as long as
   `process` follows rule 1 — `tests/rt_safety.rs` exercises the tree with
   `assert_no_alloc`, extend its scene if the UGen does something novel
   (first buffer reader, first bus feedback, …). Add a golden scene only if
   the UGen is meant to be regression-pinned (then regenerate with
   `cargo run --example render_golden` and **listen** before committing).
4. **Document** — the UGen kinds table in `docs/schemas.md` (name, inputs,
   output semantics), and manual steps in `GUIA.md` if it is user-visible.

## Faust integration notes

The lifecycle is factory (compiler thread, `ffi_lock`, `-single -ftz 2`,
stdlib include) → `FaustDef` (factory + params probed once with `UIGlue`) →
`FaustSynth` instance (created and `init`ed on the network thread, only
`compute` runs on the audio thread, dies through the garbage FIFO holding
its `Arc<FaustDef>` so the factory outlives every instance). Details and
traps: the `faust-embedding` skill and `src/faust/*` module docs.

**Why Faust UI labels are the control names** (a deliberate decision, not a
leftover): a def's parameters are named by whoever writes the def — exactly
like the `controls` array in the UGen JSON format. Inventing a second naming
layer (renaming, indices-only, a Clausters-side mapping table) would add a
translation step for zero expressiveness: the label *is* the parameter's
name in both def families, and `/s_new`/`/n_set` address both identically.
Group paths (`hgroup`/`vgroup`) are ignored on purpose — bare labels, first
declaration wins on collision. The two reserved names `out`/`in` (first
output/input bus) come *after* the def's own params, and a def that declares
its own `out`/`in` control wins over the reserved meaning. UI elements are
plain values written by `/n_set` (zone stores, RT-safe) and, since M11, can
also be **bound to a bus** with `/n_map`/`/n_mapa` — the same mechanism
unifies UGen controls and Faust zones (see *Control/bus mapping* above). The
two reserved `out`/`in` routing controls are not mappable.

## Extending the server: the plugin question

scsynth grew a binary plugin API (`UnitCmd`/interface tables) whose ABI
broke with struct or feature changes, and keeping out-of-tree UGens alive
became a maintenance tax. Rust removes the temptation: **there is no stable
Rust ABI**, so dynamically loaded Rust plugins are off the table in v1, by
decision and not omission:

- Extending Clausters = adding code to the crate (the section above). The
  documented internal surface — `UGen` + `ProcessCtx` + the registry, and
  `SynthNode` for whole-synth implementations — is the contract, and it can
  evolve freely because everything compiles together.
- The *runtime*-extensible path for users is deliberately not Rust: send a
  **Faust def** (`/d_faust`, JIT-compiled, sandboxed by the same `SynthNode`
  boundary). Most "I need a custom UGen" cases are a Faust one-liner.
- If dynamic plugins ever become necessary, the lesson from scsynth applies:
  the boundary must be a **versioned C ABI** (or a wasm interface), checked
  at load time — the same policy already planned for the shared-memory
  layout and embed cdylib of M14. Do not expose Rust types across it.
