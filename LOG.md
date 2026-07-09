# Completion log

A record of what Claude implemented in each milestone (see PLAN.md).

## M0 — Skeleton (completed 2026-06-10)

**What's there:** a binary that opens the default audio device with cpal and
plays a 440 Hz sine at amplitude 0.2. Verified on this machine:
44100 Hz, 2 channels, no stream errors.

### Structure

```
src/
├── lib.rs              # lib crate so tests can use the engine
├── main.rs             # starts the backend and waits (Ctrl-C)
├── server/
│   ├── engine.rs       # Engine: 64-frame process_block(), knows nothing of cpal
│   └── backend.rs      # cpal + BlockAdapter (only with the `realtime` feature)
├── dsp/sinosc.rs       # SinOsc by phase accumulation (f64 phase)
├── node/mod.rs         # stub — M2
└── osc/mod.rs          # stub — M1
tests/sine.rs           # offline engine tests (2 tests, pass)
```

### Decisions made

- **Engine decoupled from the backend**: `Engine::process_block(&mut [f32])`
  processes blocks of `BLOCK_SIZE = 64` frames interleaved into memory. cpal lives
  only in `backend.rs`. This enables tests with no device and the future NRT mode (M7).
- **Feature `realtime`** (default on): cpal is an optional dependency;
  `cargo test --no-default-features` runs without ALSA — what CI must use.
- **`BlockAdapter`**: cpal delivers interleaved buffers of variable size, not a
  multiple of 64; the adapter requests blocks from the engine and holds the
  leftover between callbacks (`pos` starts saturated to force the first block).
- **Sample formats**: f32, i16, u16 via `cpal::FromSample`; other formats
  return an explicit error.
- **Oscillator phase in `f64`** so tuning does not degrade in long sessions.
- The callback doesn't allocate (everything pre-allocated when building the adapter);
  no `assert_no_alloc` guard yet — it comes in M2 along with the FIFOs.

### Verification

- `cargo test --no-default-features`: 2 tests pass — frequency 440 Hz ±5 by
  zero crossings, RMS ≈ 0.2/√2, no NaN, coherent channels.
- `cargo run --release` opens the stream and plays (tested 2026-06-10).

### System dependencies

- Linux: requires `libasound2-dev` and `pkg-config` to build with the
  `realtime` feature (alsa-sys needs them).

## M1 — OSC server (completed 2026-06-10)

**What's there:** the binary now brings up, besides audio, an OSC server over UDP
at `127.0.0.1:57110` implementing `/status`, `/quit`, `/notify` and `/dumpOSC`
with scsynth semantics. Verified end to end against the real binary:
`/status` replies `/status.reply` with the device's sample rates and `/quit`
shuts the server down cleanly.

### What was added

```
src/osc/server.rs       # OscServer: UDP socket, command dispatch, replies
src/main.rs             # starts backend + OSC; the OSC loop runs on the main thread
src/lib.rs              # re-exports rosc for tests and clients
examples/osc_ping.rs    # minimal client: /status (+ /quit) for hand testing
tests/osc.rs            # 5 integration tests over real UDP
```

### Implemented behavior

- **`/status`** → `/status.reply` with the 9-argument scsynth format:
  `(1, #UGens, #synths, #groups, #defs, avg_cpu, peak_cpu, sr_nominal, sr_real)`.
  The counters are zero until M2 wires up the node tree; the sample rates
  are the device's real ones (Double).
- **`/notify 1|0`** → registers/unregisters the client address and replies
  `/done /notify clientID` (IDs from 1; registering twice keeps the ID).
  The client list is ready for M2's `/n_go`/`/n_end` notifications.
- **`/quit`** → replies `/done /quit` and the loop returns; main drops the backend.
- **`/dumpOSC 0|1`** → enables/disables logging of parsed messages to stdout.
- **Unknown command / invalid arguments** → `/fail <cmd> <reason>`, without
  killing the server.
- **Bundles**: executed immediately (recursive); timetag scheduling is M6.

### Decisions made

- The OSC server runs **on the main thread** (blocking on `recv_from`); the
  audio lives on the cpal callback thread. The network thread may allocate and do
  I/O freely — the RT-safe boundary (FIFOs) arrives in M2.
- `rosc` is **re-exported from the lib** so integration tests and
  clients use exactly the same version.
- Bind to `127.0.0.1` (not `0.0.0.0`) by default; exposing it will be a CLI option.
- `ECONNREFUSED` on `recv_from` (an ICMP bounce of a reply to an already-closed
  client, Linux behavior) is ignored and serving continues.
- Integration tests with the server on an **ephemeral port** (`127.0.0.1:0`) and real
  UDP, the thread joined after `/quit` — they run in parallel without collisions.

### Verification

- `cargo test`: 7 tests pass (5 OSC + 2 from the M0 engine).
- Manual E2E: `cargo run --release` + `cargo run --example osc_ping -- quit`
  (tested 2026-06-10; the server exited cleanly after `/quit`).

## M2 — RT-safe FIFO + node tree (completed 2026-06-10)

**What's there:** the server now starts silent (like scsynth) and plays only via
commands: `/s_new` instantiates the hardcoded "default" synth (SinOsc with controls
`freq`/`amp`), `/n_set` modifies it live and `/n_free` frees it. All
network→audio communication goes over lock-free ring buffers; the audio thread never
allocates, verified by the guard test with `assert_no_alloc`.

### What was added

```
src/server/engine.rs    # rewritten: Cmd/Garbage FIFOs (rtrb), Counters, engine_pair()
src/node/mod.rs         # NodeTree: pre-allocated slab of 1024 slots, DFS with its own stack
src/node/default_synth.rs # DefaultSynth "default": SinOsc + controls freq(0)/amp(1)
src/osc/server.rs       # + /s_new, /n_free, /n_set; real counters in /status
tests/engine.rs         # 6 engine tests (replaces tests/sine.rs)
tests/rt_safety.rs      # assert_no_alloc guard over process_block
```

### Implemented architecture (the scsynth pattern)

- **`engine_pair()`** splits the server into two halves: `Engine` (audio thread)
  and `EngineHandle` (network thread), connected only by two `rtrb` SPSC FIFOs
  (1024 entries each):
  - **Commands** (network→audio): `Cmd::{AddSynth, FreeNode, SetControl}`. The synth
    travels already built and boxed — the audio thread just plugs it in, O(1).
  - **Garbage** (audio→network): `Garbage::{Freed, Rejected}`. The audio thread never
    drops a `Box`; it returns it whole and the network thread drops it in
    `collect_garbage()`, which runs after each packet and every 100 ms via socket
    timeout (rejected commands also travel there: duplicate ID or full table).
  - If the garbage FIFO fills up: a pre-allocated local list of 64; if that also
    fills, `mem::forget` (a deliberate leak — the only RT-safe option).
- **`NodeTree`**: a slab of 1024 pre-allocated slots (`MAX_NODES`), linear search
  by ID (enough for now), the root group's children in execution order, iterative
  DFS with a pre-allocated stack. `Group` exists structurally; `/g_new`
  arrives in M4.
- **Counters**: the audio thread publishes `synths`/`ugens` with relaxed atomic
  stores; `/status.reply` reads them — the counters are real already.

### Decisions made

- `/n_set` was brought forward from M3 because `Cmd::SetControl` came for free and
  lets you test the engine live (change freq without recreating the synth).
- Automatic IDs (`/s_new` with -1) from 2_000_001, like scsynth's high
  counter. ID 0 (root) and negatives are rejected with `/fail`.
- Add actions 0 (head) and 1 (tail) over the root; 2–4 reply `/fail` until M4.
- Controls by name (`freq`, `amp`) or by index (0, 1); unknown ones are
  silently ignored, like scsynth.
- `/n_free` of a nonexistent ID is ignored (the async `/fail` needs the
  reply FIFO of M4). Engine rejections are logged when collecting garbage.
- A `/status` sent immediately after a command may show the old
  count: commands are applied at the start of the next block (~1.45 ms
  at 44.1 kHz). It's scsynth's asynchronous semantics, not a bug.

### Verification

- `cargo test`: 14 tests pass — 6 engine ones (sine, mixing two synths,
  live pitch change, silence after free, duplicate-ID rejection), 7 OSC
  (including the network→FIFO→audio→/status round-trip with a manual clock) and the
  RT guard: 400 blocks processing, inserting and freeing 32 synths under
  `assert_no_alloc`, with not a single allocation.
- Manual E2E with the real binary (2026-06-10): `osc_ping status beep status quit`
  — audible beep at 440 Hz, retuned to 660 Hz live, freed, server
  shut down cleanly. The `osc_ping` example gained the `beep` mode.

## M3 — SynthDefs (completed 2026-06-10)

**What's there:** clients now define synths live: `/d_recv` receives a SynthDef
in JSON (our own format, not SC's binary `.scsyndef`), the interpreter validates
and compiles it, and `/s_new` instantiates arbitrary UGen graphs. The hardcoded
`DefaultSynth` is gone — "default" is now a SynthDef built by the same
interpreter and registered at startup.

### What was added

```
src/synthdef/mod.rs      # SynthDefSpec (serde/JSON), compile() with validation, default_spec()
src/synthdef/instance.rs # UGenSynth: UGen vector + wires, impl SynthNode
src/dsp/mod.rs           # trait UGen { process(ctx, inputs, output) }, helper at()
src/dsp/{sinosc,binop,noise,registry}.rs  # SinOsc refactored, Add/Sub/Mul/Div, WhiteNoise
src/node/mod.rs          # trait SynthNode; the tree holds Box<dyn SynthNode>
tests/synthdef.rs        # 12 tests: format, validation, signal (FM/vibrato included)
```

### The format (full example in the doc of src/synthdef/mod.rs)

```json
{"name": "beep",
 "controls": [{"name": "freq", "default": 440.0}],
 "ugens": [{"kind": "SinOsc", "inputs": [{"control": 0}]},
           {"kind": "Mul",    "inputs": [{"ugen": 0}, {"const": 0.2}]}],
 "out": 1}
```

Inputs: `{"const": x}`, `{"control": i}`, `{"ugen": j}` (only earlier UGens —
topological order validated). `compile()` rejects with messages naming the
offending node (`ugens[2].inputs[0]: references ugen 3; only earlier...`) that
travel to the client in `/fail`.

### Decisions made

- **`SynthNode` trait** (a prerequisite of the F fork): the tree and the FIFOs
  handle `Box<dyn SynthNode>` — `UGenSynth` today, `FaustSynth` in F3, without touching
  the engine or the tree.
- **Defs live only on the network thread** (`HashMap<String, Arc<SynthDef>>`):
  instances are built there and travel ready; the audio thread never sees
  the table. `/d_free` only removes the def from the map — live synths keep their
  `Arc` (exact scsynth semantics).
- **Control name resolution on the network thread**: a mirror
  `node_id → Arc<SynthDef>` kept from `/s_new` and cleaned when collecting
  `Garbage::Freed` — so `Cmd::SetControl` stays POD and the audio thread
  doesn't compare strings.
- **Wiring without allocation**: `UGenSynth::process` builds each UGen's inputs in
  a fixed stack array (`MAX_UGEN_INPUTS = 8`) with `split_at_mut` over the
  wires — the topological order guarantees inputs only look at earlier wires.
  Verified by the `assert_no_alloc` guard.
- Initial UGens: `SinOsc` (freq modulable by signal — FM/vibrato works),
  `Add`/`Sub`/`Mul`/`Div`, `WhiteNoise` (xorshift with a per-instance seed, no
  `rand`). The rest of the catalog (filters, EnvGen, PolyBLEP) stays for M4+.
- `/d_recv` accepts the JSON as an OSC Blob or String.

### Verification

- `cargo test`: 31 tests pass — 12 synthdef ones (JSON roundtrip, validation of
  the 6 compilation errors, signal: frequency/RMS of interpreted defs,
  mix via `Add`, noise, vibrato via FM), 12 OSC (includes `/d_recv` with
  an invalid def → `/fail` with the compilation error, `/d_free`, `/n_set` by
  name via the mirror), 6 engine and the RT guard (now processing interpreted
  instances).
- Real E2E (2026-06-10): `osc_ping status vibrato status quit` — def "vibrato"
  (5 UGens, FM) sent via `/d_recv` as a JSON blob, `/done` received, it
  played 1.2 s audibly, `/status` during playback: 5 ugens / 1 synth / 2 defs.

## M4 — Buses and order (completed 2026-06-10)

### What got done

- **Buses** (`src/dsp/mod.rs`): `Buses` with 128 audio buses (`[f32; 64]`,
  owned by the audio thread, cleared each block) and 1024 control buses
  shared (`ControlBuses`: `Arc<Vec<AtomicU32>>` with bit-cast of f32,
  relaxed stores/loads — lock-free on both threads). Buses `0..channels`
  are the hardware outputs. `ProcessCtx` now carries `sample_rate` +
  `&mut Buses` and is passed to every UGen.
- **I/O UGens** (`src/dsp/io.rs`): `Out` (sums into the bus — several synths on
  the same bus mix, scsynth semantics), `ReplaceOut` (overwrites),
  `In` (copies an audio bus), `InCtl` (reads a control bus as a block
  constant). **Format change**: SynthDefs no longer carry the
  `out` field; output is exclusively via `Out` UGens (a def without `Out`
  is silent). The "default" def now ends in two `Out` (buses 0 and 1).
- **Node tree** (`src/node/mod.rs`, rewritten): the root group (ID 0) lives
  in slot 0 of the slab and cannot be freed/moved; each node keeps its
  `parent`; nested groups with pre-allocated `children`
  (`MAX_GROUP_CHILDREN=256`, root `MAX_NODES`) and capacity rejection before
  inserting (a Vec never grows on the audio thread). Add actions 0–4
  complete (`Replace` frees the target's subtree). `move_node`
  (`/n_before`/`/n_after`) with cycle check by ancestors and capacity
  check when crossing groups. Recursive `free`, `free_all` (empties the group) and
  `deep_free` (frees only synths, keeps subgroups) — all without allocation,
  with a pre-allocated `free_stack` separate from the `dfs_stack`. Nodes leave
  via a *sink* (`&mut dyn FnMut(FreedNode)`) reporting ID + parent.
- **Engine** (`src/server/engine.rs`): owns the `Buses`; the interleaved
  output is copied from buses 0..channels (no more `mix`/`scratch`).
  `Cmd` extended: `AddSynth`/`AddGroup` with `target` + action, `FreeNode`,
  `FreeAllInGroup`, `DeepFreeGroup`, `MoveNode`, `SetControl`. `Garbage` with 4
  variants (Freed/Rejected × Synth/Group). New event FIFO
  (`NodeEvent { Go|End, id, parent_id, is_group }`, capacity 2048, best-effort
  delivery) for `/n_go`/`/n_end`. An internal `GarbageSink` borrows the engine's
  fields separately to avoid the double borrow with the tree.
  `Counters` adds `groups` (initialized to 1: the root exists before the first
  tick). `BLOCK_SIZE` moved to `dsp` (re-exported in `engine` for
  compatibility).
- **OSC** (`src/osc/server.rs`): `/s_new` with add actions 0–4 and a real target;
  new `/g_new` (id/action/target triples), `/g_freeAll`, `/g_deepFree`,
  `/n_before`/`/n_after` (pairs); `/c_set`/`/c_get` served directly on the
  network thread over the atomics (no engine round-trip; `/c_get` replies
  with a `/c_set` of index/value pairs, like scsynth). `collect_garbage`
  also drains the event FIFO and sends `/n_go`/`/n_end`
  (`[id, parent, -1, -1, isGroup]`) to `/notify` clients. `/status`
  reports the real group count.

### Decisions

- `/c_set`/`/c_get` don't go through the command FIFO: control buses are
  shared atomics and the network thread operates directly. A synth sees them only
  on its next block (same effect as going through the FIFO).
- `Out` sums and `ReplaceOut` overwrites → execution order is audible and
  testable: the order tests use a "silencer" (`ReplaceOut` of 0.0) that
  wins or loses the bus depending on whether it's after or before the source.
- The generic reply FIFO for async `/fail` ended up covered by the event
  FIFO (`/n_go`/`/n_end`); engine rejections still leave as
  `Garbage::Rejected*` with a log to stderr (a real async `/fail`
  would need to store the sender per command — evaluated in M5/M6).

### Verification

- `cargo test`: 47 tests pass — 15 engine ones (bus mixing, audible
  order + `MoveNode`, before/after/replace, nested groups with recursive
  free, `free_all`/`deep_free`, go/end events, control buses from the
  network thread), 16 OSC (new: `/g_new` + group count in `/status`,
  `/g_freeAll`, `/c_set`/`/c_get` roundtrip, `/n_go`/`/n_end` notifications to
  `/notify` clients), 15 synthdef (Out/ReplaceOut/In/InCtl semantics, a def
  without `Out` silent) and the RT guard (now with a group + 32 synths, move and
  recursive free under `assert_no_alloc`).
- Real E2E (2026-06-10): `osc_ping status vibrato status quit` against the
  M4 binary — the "vibrato" def rewritten with `Out` UGens (7 ugens) played
  audibly; `/status` during playback: 7 ugens / 1 synth / 1 group.

## F0 — Faust toolchain and minimal FFI (completed 2026-06-10)

The first milestone of the F fork (SynthDefs via Faust's Box API + LLVM
JIT). The goal was to measure the toolchain's real risk; result: **much
cheaper than expected** (JIT ≈ 10 ms per def).

### Toolchain findings

- **Ubuntu's libfaust is useless for embedding**: `libfaust2t64` (2.81.10)
  is compiled without the LLVM backend (it doesn't depend on libLLVM) and there's no
  `-dev` package with headers. We had to compile from source.
- **Crates evaluated and dropped**: `faust-build`/`faust-types` do
  Faust→Rust codegen at build time (they need the `faust` compiler and the DSP
  as static source) — they don't embed the JIT. There's no maintained binding of
  libfaust. Decision: **our own hand-written binding** against the real
  headers (~30 functions); bindgen stays for F1+ if the surface grows
  (avoids the libclang dependency for now).
- **Build from source** (reproducible recipe, no sudo):
  `git clone --depth 1 -b 2.81.10 github.com/grame-cncm/faust` +
  `make most` + two cmake cache tweaks in `build/faustdir`:
  `-DINCLUDE_DYNAMIC=ON` (the `most` target doesn't build the .so) and
  `-DLINK_LLVM_STATIC=off -DLLVM_CONFIG=llvm-config-20`; then
  `make install PREFIX=$HOME/.local`. System deps: `cmake`,
  `llvm-20-dev`, `libzstd-dev`, `zlib1g-dev`.
- **Static LLVM linking fails on Ubuntu without `libpolly-20-dev`** (Polly is
  in a separate package) — with `LINK_LLVM_STATIC=off` it's not needed: the
  monolithic `libLLVM.so` is linked.
- **Measurements** (Faust 2.81.10 + LLVM 20.1.8): `libfaust.so` = 11 MB
  (dynamic against `libLLVM.so.20.1`, 137 MB already present as a system
  lib); the static alternative (`libfaustwithllvm.a`) = 35 MB.
  JIT latency of the smoke test's def: **~10 ms**; instantiation + init:
  **~0.08 ms**. Full Faust compilation from source: ~10 min on 8
  cores.

### What got done

- **Feature `faust`** in Cargo.toml (off by default; the core compiles and
  tests without libfaust on the system).
- **`build.rs`**: with the feature active, locates libfaust via `FAUST_PREFIX`
  (fallback `~/.local`, then `/usr/local`), links `-lfaust` and adds an rpath
  so tests and binaries run without `LD_LIBRARY_PATH`.
- **`src/faust/ffi.rs`**: minimal FFI verified against the headers of the
  exact build — context (`createLibContext`/`destroyLibContext`), Box API
  (`CboxReal`, `CboxWire`, `CboxSeq/Par/Split/Rec`, the applied `Cbox*Aux`,
  `CboxHSlider`) and JIT (`createCDSPFactoryFromBoxes`, instance, `compute`).
  C API detail: operators exist in two forms — `CboxAdd()`
  (primitive, a 2-input box) and `CboxAddAux(b1, b2)` (applied) — because
  C has no overloading.
- **`tests/faust_smoke.rs`** (gated): builds the equivalent of `SinOsc`
  from primitives — `sin(2π·phasor(freq))`, `phasor = (+(f/SR) : wrap) ~ _`,
  `wrap = _ <: _ - floor(_)`, `freq` as an hslider at its default 440 — compiles it
  by JIT, renders 1 s offline and asserts frequency (±5 Hz) and RMS;
  second test: an invalid box fills the error buffer (4096 bytes) instead
  of crashing.

### Verification

- `cargo test --features faust`: 49 tests (the 47 core ones + 2 smoke).
- `cargo test` without the feature: same as before, without touching libfaust.
- libfaust installed in `~/.local` (lib + headers + Faust's stdlib).

## F1 — Faust compiler thread (completed 2026-06-10)

### What got done

- **`src/faust/compiler.rs`**: a dedicated `CompilerThread` ("faust-compiler")
  with an mpsc queue of `CompileRequest { name, source, client }` and a channel of
  `CompileResult` back. The network thread drains results in its loop
  (after each packet and on the GC tick) and sends the async reply to the
  client that requested it: `/done "/d_faust" <name>` or `/fail` with the Faust
  compiler error verbatim. Clean shutdown on Drop (closes the channel and
  joins).
- **`src/faust/factory.rs`**: `FaustFactory` (a wrapper owning the
  pointer, `Drop` → `deleteCDSPFactory` on a non-RT thread). Refcount via
  `Arc<FaustFactory>` in the OscServer's `faust_defs` table; F3's instances
  will keep clones so the factory never dies before them.
- **OSC**: `/d_faust name source` (String or UTF-8 Blob) enqueues the
  compilation; `/d_free` also cleans the Faust table; `/status` counts
  both def tables. Without the feature, `/d_faust` replies
  `/fail "server built without faust support"`.
- F1 compiles **Faust source** (`createCDSPFactoryFromString`); the JSON→Box
  API mapping enters in F2 replacing only the body of `compile()`.

### Finding: libfaust does not tolerate concurrent compilation

Two `CompilerThread`s compiling at once in the same process → SIGSEGV
(verified: parallel test runs crashed, serial ones passed). Faust's
global compiler state is not thread-safe even for
`createCDSPFactoryFromString` (not just the Box API's lib context). Fix:
a process-global lock (`compiler::ffi_lock()`) around every compilation
FFI call; the F0 smoke test takes it too. A server has
a single compiler thread, but the tests (and any embedder with several
servers) need the lock.

#### Pending flake: factory deletion under parallel load (2026-06-18)

`cargo test --features faust` with the default parallelism sometimes fails in
`faust_compiler` with `WARNING : deleteDSPFactory factory not found!` (non-
deterministic; serially with `--test-threads=1` it passes 4/4 reliably). It's a
symptom of the same non-thread-safe global state, but it points at **factory
deletion** (the drop of a `FaustDef`'s factory), a path that today does NOT go
through `ffi_lock()`: by the F0 decision (see "Decisions" below) instantiating/
deleting from an already-compiled factory was assumed independent of the
compiler's state, so only *compilation* takes the lock. The warning suggests
`deleteDSPFactory` does touch the global factory table and can race with
concurrent compilations/deletions. Distinct from the concurrent-compilation
crash already solved (that one is covered). Pending: either put the factory
delete under `ffi_lock()`, or mark the `faust_compiler` binary as
`--test-threads=1`. Non-blocking (a real server has a single compiler and does
not delete factories concurrently).

### Verification

- `cargo test --features faust`: 55 tests (47 core + 2 F0 smoke + 6 F1:
  direct thread with FIFO order and readable errors, async OSC round-trip
  of `/d_faust` with `/done`/`/fail`, count in `/status`, `/d_free`).
  Stable across 3 consecutive runs (no races).
- `cargo test` without the feature: intact. Clippy clean.

## F2 — JSON → Box API schema (completed 2026-06-10)

### What got done

- **`src/faust/boxes.rs`** (new): a JSON → Box API calls interpreter.
  The schema (documented with a table and example in the module doc) mirrors
  the C API one-to-one: shortcuts (a number = constant, `"_"` = wire, `"!"` =
  cut), `{"op": …}` objects for composition (`seq`/`par`/`split`/`merge`
  n-ary with left fold, binary `rec`), 18 binaries (arithmetic,
  comparisons, bitwise, `delay`), 19 unaries (trig, exp/log, roundings,
  casts), `select2`/`select3`, UI (`hslider`/`vslider`/`nentry`/`button`/
  `checkbox`/`hgroup`/`vgroup`) and the escape hatch `{"op": "faust", "src":
  "…"}` that compiles a complete Faust program to a box via `CDSPToBoxes` —
  access to the whole stdlib (`os.osc`, `fi.`) composable with primitives.
- **Errors with a path**: structural validation is done while building and
  each error carries the path of the offending JSON node from the root `$` (e.g.
  `at $.in[0].in[1]: unknown op "zzz"`); Faust's semantic errors
  (composition arities, dangling inputs) come verbatim from the
  factory step.
- **`src/faust/compiler.rs`**: `CompileRequest` now carries a
  `CompilePayload::Source` (F1) or `::Json` (F2); a `LibContext` guard (lock +
  `createLibContext`/`destroyLibContext` on Drop); `FaustArgs::stdlib()`
  passes `-I $PREFIX/share/faust` (search like build.rs: `FAUST_PREFIX` →
  `~/.local` → `/usr/local`) both to `createCDSPFactoryFromString` — raw-source
  defs can now `import("stdfaust.lib")` — and to the fragments.
- **`src/faust/ffi.rs`**: Box API surface completed (~45 new
  symbols: binary/unary `Aux`, delays, selects, UI, `CDSPToBoxes`),
  verified against `nm -D libfaust.so` besides the header.
- **OSC**: `/d_faust name def` distinguishes by sniffing — if the def starts
  with `{` it's JSON, otherwise it's Faust source (top-level Faust source never
  starts with `{`, the sniff is unambiguous).

### Finding: upstream bug in `boxFmod()`

`CboxFmodAux(a, b)` builds `(a, b) : abs` — `boxFmod()` in
`compiler/box_signal_api.cpp` returns `gGlobal->gAbsPrim->box()` (a copy-paste
bug present in 2.81.10 and still in master-dev). The "kitchen sink"
test that exercises each schema op once caught it (needed because dynamic
linking is lazy: a mistyped symbol in the hand-written FFI only blows up
when called). Workaround: `fmod` doesn't use the binding but a
`CDSPToBoxes("process = fmod;")` fragment returning the real 2-input
primitive; `CboxFmodAux` was left unbound with a note in ffi.rs.

### Verification

- `cargo test --features faust`: 64 tests (47 core + 2 F0 smoke + 8 F1/OSC
  + 7 F2: JSON sine with frequency and RMS parity against the F0 smoke,
  stdlib fragment composing `os.osc` with primitives, stdlib import
  from raw source, validation errors with the node path, fragment
  error with path + compiler message, kitchen sink of all the
  ops). Stable across 3 consecutive runs.
- `cargo test` without the feature: 47 tests, intact. Clippy clean (only the two
  preexisting `Default` ones from dsp).

## F3 — FaustSynth in the tree (completed 2026-06-10)

### What got done

- **`src/faust/synth.rs`** (new): `FaustDef` and `FaustSynth`.
  - **`FaustDef`** is what the def tables now hold: the compiled
    factory plus the parameters (name, init, min, max, step) and the I/O
    arity, discovered **once** by probing a disposable
    instance on the compiler thread (`FaustDef::probe`, called by
    `compile()` after creating the factory). So `/s_new` and `/n_set`
    resolve control names on the network thread without touching libfaust.
  - **`FaustSynth: SynthNode`**: built on the network thread
    (`createCDSPInstance` + `initCDSPInstance(sr)` allocate), collects the
    `FAUSTFLOAT*` zones with `UIGlue` at instantiation, travels already
    assembled via the cmd FIFO, and `process()` only calls
    `computeCDSPInstance` (libfaust's only RT-safe call) plus staging
    copies. `Drop` deletes the instance — it always runs on the network thread because
    freed nodes leave via the garbage FIFO; the synth's
    `Arc<FaustDef>` guarantees instance-dies-before-factory.
- **Controls convention**: indices `0..n` = the def's UI parameters (declaration
  order, bare labels — groups are flattened); then two
  reserved names: `out` (index n) and `in` (n+1), the first audio bus
  outputs/inputs map to. Defaults `out=0`, `in=0`. Clamp so that
  the full channel span stays within the buses.
- **Bus mapping**: Faust's I/O is non-interleaved `float**` like
  our buses, but the synth goes through its own staging buffers: outputs
  **sum** into the bus (`Out` semantics, synths mix) and
  inputs are copied before writing outputs (an in-place chain
  `in == out` stays correct).
- **OSC**: `/s_new` instantiates Faust defs like any other (helper
  `make_synth` that searches both tables); the `node_defs` mirror now
  holds an enum `NodeDef::{UGen, Faust}` to resolve names in
  `/n_set`. `/d_free` with live instances breaks nothing (refcount).
- **FFI**: `UIGlue` (a repr(C) struct of 13 callbacks from CInterface.h) and
  `buildUserInterfaceCDSPInstance`.

### Decisions

- Instantiation on the network thread **without** `ffi_lock()`: creating instances
  from an already-compiled factory is independent of the compiler's global
  state (it's JIT code + malloc; FaustLive/faustgen~ do it
  concurrently with compilations). The lock stays only for compiling.
- `ugen_count() = 1` per Faust instance in `/status.reply`.
- The SR is frozen by `instanceInit` (see the foresight in PLAN.md);
  the probe uses a fixed 48 kHz because params and arity don't depend on the SR.

### Verification

- `cargo test --features faust`: 73 tests (64 from F2 + 8 from `faust_synth`:
  param and reserved-control probe, sine in the tree with
  frequency/RMS, `/n_set` by zone, routing via `out`, chain via input
  bus with `in`, UGen+Faust mix on the same bus (F4 interop
  brought forward), free with the factory surviving the `/d_free`, and the full
  cycle over OSC with the engine ticked by hand + 1 in `rt_safety`: 8
  FaustSynths inserted, processed, recontrolled and freed under
  `assert_no_alloc`). Stable across 3 runs.
- `cargo test` without the feature: 47 tests, intact. Clippy clean.

## F4 — Parity and interop (completed 2026-06-10)

### What got done

- **`tests/faust_parity.rs`** (new): golden tests of equivalent graphs,
  rendered side by side **in the same engine** (UGen to channel 0, Faust to
  channel 1, same blocks):
  - **Sine**: `SinOsc(440)·0.2` against the same graph via JSON→Box
    (`sin(2π·phasor)` with `delay 1` to align the phase: our `SinOsc`
    starts at phase 0, the raw phasor `(+(f/SR) : wrap) ~ _` starts at
    `f/SR`). Sample-by-sample equality with tolerance `4e-3` — `SinOsc`
    accumulates phase in f64 and Faust (-single) in f32, so it can't be
    exact — and a discrimination assert: the same signals shifted
    one sample **must** violate the tolerance (a 1-sample phase offset
    peaks at ≈ 0.0115, well above).
  - **Bit-exact gain**: a UGen sine feeds bus 4; a UGen chain
    `In·0.5` and a Faust one `_ * 0.5` read it in the same block into
    channels 0 and 1. Same f32 multiplication over the same samples:
    **zero** bits of difference (stateless arithmetic is identical between
    the two worlds; only the oscillators diverge by precision).
  - **Shared group**: a UGen synth + a Faust synth as siblings in a
    non-root group, mix into the same bus (RMS of the sum) and a single
    `FreeAllInGroup` frees them together (2 in the garbage FIFO).
- **`examples/json_client.py`** (new): an example client in Python, stdlib
  only (a hand-written OSC encoder/decoder: i, f, s, b, d). It **generates** the two
  def formats programmatically — `SynthDefBuilder` for `/d_recv`
  (noise with AM) and `box()`/`hslider()`/`faust()` helpers for `/d_faust`
  (sine from primitives + a def with stdlib via the escape hatch) — and handles
  the full cycle: `/done`//`/fail`, `/s_new` with controls by name,
  `/n_set`, `/n_free`, `/status`, `/quit`. Demos: `status ugen faust quit`.
- **`docs/schemas.md`** (new): reference documentation of both
  schemas (in English, like the code docs): full SynthDef JSON
  format (UGen table, input forms, `Out`/`ReplaceOut` semantics,
  errors), Faust defs (source vs JSON, op table mirroring the Box API,
  reserved controls `out`/`in`, errors with the `$` path), and the
  common OSC cycle.

### Verification

- `cargo test --features faust`: 76 tests (73 + 3 parity). `cargo test`
  without the feature: 47, intact. Clippy clean (only the 2 preexisting
  `Default` warnings).
- Real E2E of the Python client against the release server with the feature, in a
  single invocation: `status` (reply with doubles — the decoder needed the
  `d` tag), `/d_recv` amnoise `/done`, `/d_faust` jsine and jstdlib `/done`,
  synths sounding and `/quit`.
- One run of the full suite had 1 sporadic failure in
  `faust_synth` that didn't reproduce in 7 later runs (5 isolated
  + 2 full); I suspect the UDP OSC test under load. Watch if it
  reappears.

## M5 — Buffers (completed 2026-06-10)

### What got done

- **`src/dsp/buffer.rs`** (new): `Buffer` — interleaved f32 data +
  frames/channels/sample-rate — **immutable once built**, shared
  as `Arc<Buffer>`. A pool of 1024 slots (`BufferPool`) in the engine;
  a mirror on the network thread for `/b_query` and to give
  `/b_read`/`/b_write`/`/b_zero` the current content/shape. Immutability
  is the central decision: no locks or aliasing between threads (scsynth mutates
  shared memory; we pay a copy per replacement). Recording UGens
  will need another scheme.
- **`src/server/nrt.rs`** (new): an NRT thread with the same pattern as the
  Faust compiler (mpsc requests/results, drained in the OSC server's
  loop). Jobs: `Alloc` (zero), `AllocRead`/`Read`/`Write` (WAV via `hound`:
  int 1–32 bits scaled to ±1 and float32; `Read` overlays the file over
  a copy of the current content keeping the shape), `Free`. **A single
  queue = buffer commands complete in submission order** (that's why even
  `/b_free` goes through it: it can't overtake a pending alloc).
- **Engine**: `Cmd::SetBuffer { index, Option<Arc<Buffer>> }` swaps the
  slot; the replaced one leaves as `Garbage::FreedBuffer` (the last `Arc`
  is never dropped on the audio thread). `ProcessCtx` now carries
  `buffers: &[Option<Arc<Buffer>>]`.
- **UGens** (`src/dsp/buf.rs`): `PlayBuf` (bufnum, channel, rate, loop; rate
  in frames per output sample — 1.0 = the server's sr, the client
  compensates with `file_sr / server_sr`; f64 phase; silence at the end if
  not looping) and `BufRd` (bufnum, channel, phase in frames, loop; out-of-range
  phase wraps with loop and clamps without it). Both **mono** with a `chan`
  input (our UGens have one output): a stereo file is two
  sample-locked readers. Linear interpolation. No trigger or done action
  yet.
- **OSC**: `/b_alloc`, `/b_allocRead`, `/b_read`, `/b_write` (WAV only;
  int16/int24/float), `/b_zero` (replaces with a zeroed one of the same
  shape), `/b_free` — asynchronous, replying `/done cmd bufnum` or `/fail` —
  and `/b_query` → `/b_info` (synchronous from the mirror). `leaveOpen` is
  accepted and ignored (no streaming).
- **Python client**: a `buffer` demo (writes a WAV with the `wave` module,
  `/b_allocRead`, correct rate from `/b_info` + `/status`, `/n_set` of
  rate, `/b_free`). Schema and commands documented in `docs/schemas.md`;
  manual steps in GUIA.md.

### Verification

- `cargo test`: 61 tests (47 + 13 from `tests/buffers.rs` + 1 in
  `rt_safety`); with the feature: 90. The buffer tests include **exact**
  sample-by-sample equality (playback rate 1, loop, channels,
  `BufRd` interpolation with representable values), lossless float WAV
  round-trip, verified int16 quantization grid
  (scale 32767 on write, 1/32768 on read), file slicing,
  `/b_read` overlay, errors (channel mismatch, nonexistent file)
  and the full `/b_*` cycle over OSC with the engine ticked by hand.
- `rt_safety`: install, replace (even shrinking with a `PlayBuf`
  reading), empty the slot and free the synth — zero allocs on the audio
  thread, 3 items via the garbage FIFO.
- Real E2E: release server + `json_client.py buffer` (a 330 Hz sine at
  22050 Hz played at rate 0.5 on a 44100 server, a fifth up with
  `/n_set`, `/b_free` with `/done`). Clippy clean (only the 2 preexisting
  `Default` warnings).

## M6 — Sample-accurate scheduling (completed 2026-06-10)

### What got done

- **Slices in `ProcessCtx`**: `offset` + `frames` — normally the whole
  block, but a scheduled bundle splits the block at the event's sample and
  every node processes only the sub-range. `UGenSynth` trims wires and
  inputs to `frames`; `In`/`Out`/`ReplaceOut` index the buses at `offset`;
  `FaustSynth` copies partial staging and calls `compute(frames)`. This goes
  **beyond real scsynth**, which quantizes bundles to the 64 block and
  needs `OffsetOut` to compensate — here the split is genuine and not
  needed.
- **Queue in the engine**: `Cmd::Schedule { time, cmds }` — absolute time
  in samples, commands already built (boxed synths) on the network thread.
  A pre-allocated `Vec<ScheduledBundle>` (1024), stable ordered insertion
  (FIFO on ties, `partition_point` + `insert` without allocation) and `remove(0)`
  when due; the executed `Vec` shell returns as `Garbage::SpentBundle`
  (heap capacity freed on the network thread); a full queue = the whole bundle
  rejected by the same path. `process_block`: drains immediate commands at the
  start of the block, then a loop of segments running the due bundles at their
  exact offset (late ones at offset 0).
- **Clock and NTP conversion**: the engine publishes `now` (samples processed,
  `AtomicU64`) every block; the network thread converts
  `delta = timetag − SystemTime::now()` and schedules at
  `current_samples() + delta·sr`. An immediate timetag (`{0,1}`) or a past one =
  execution on arrival (past ones log "late", like scsynth). Nested bundles
  are scheduled independently by their own timetag.
- **Message translation** (`schedule_message`): `/s_new` (controls
  applied at boxing, the `node_defs` mirror updated at scheduling, -1 IDs
  resolved), `/n_set` (by name via the mirror), `/n_free`,
  `/n_before`/`/n_after`, `/g_new`, `/g_freeAll`/`/g_deepFree` and `/c_set`
  — the last as a new `Cmd::SetControlBus`: the immediate form writes
  the atomics on the network thread, but the scheduled one must land on its exact
  sample. The unschedulable replies `/fail "… cannot be scheduled in a timed bundle"`.
- **Python client**: `bundle(seconds_ahead, *packets)` (NTP timetag by
  hand) and a `bundle` demo: an arpeggio scheduled entirely ahead.

### Verification

- `cargo test`: 72 (61 + 10 from `tests/scheduling.rs` + 1 in `rt_safety`);
  with the feature: 102 (+1: FaustSynth split mid-block with a constant
  def, exact edge). The scheduling tests are **sample-exact**
  with DC signals: a mid-block trigger (sample 100 = block 1 offset
  36), three events splitting one block (10/30/50), bundle
  atomicity, ties in arrival order, out-of-order times, late ones at
  offset 0, scheduled `/c_set` (step at sample 32), full queue (the 1025th
  rejected and returned whole), and the OSC round-trip with a real NTP
  timetag (a tolerance window over the published clock).
- `rt_safety`: 16 bundles at odd offsets (ordered enqueue, split,
  execute) — zero allocs, 16 shells back. Clippy clean (only the 2
  preexisting warnings).
- Real E2E: release server + `json_client.py bundle` (5 notes scheduled
  ahead, regular rhythm). The banner now says `clausters M6`.

## M7 — NRT mode + golden tests (completed 2026-06-11)

### What got done

- **Prior refactor** (`src/osc/translate.rs`): the message→`Cmd` translation
  moved out of `OscServer` into a shared `CmdTranslator` — def tables,
  the `node_defs` mirror, auto-IDs, `translate()` (the old `schedule_message`),
  `d_recv`/`d_free`, `make_synth` — plus `parse_buffer_msg` (the six async `/b_*`
  → `NrtJob`, before six nearly-identical handlers) and `parse_d_faust`.
  The server delegates; the immediate `/s_new` now also goes through `translate`.
- **Renderer** (`src/server/render.rs`): `Score` (events stably ordered
  by time) + `render`/`render_to_vec`/`render_to_wav`. A `Score` is
  loaded from scsynth's binary format (`[i32 BE size][OSC packet]`…;
  the timetag counts **seconds since the render start**, immediate tag
  = 0). The render is single-threaded with the two halves of `engine_pair`: the
  schedulable commands travel as `Cmd::Schedule` through the M6 queue (the same
  sub-block split as live → the offline render is sample-by-sample
  identical to a perfect live take), and the async ones (`/d_recv`, `/d_faust`,
  `/d_free`, `/b_*`) run **synchronously** before advancing time
  (scsynth NRT semantics): `run_job` and `faust::compiler::compile` are now
  `pub` and called inline; buffers are installed with the rest of the
  bundle (sample-accurate swap). The render ends at the time of the last
  bundle (its commands don't sound): close the score with a dummy
  bundle. Strict errors: an unknown/failed command aborts with the time and
  message; bundles dropped due to a full queue too (better than silently
  missing notes in a golden).
- **CLI**: `clausters --nrt score.osc out.wav [--rate] [--channels]
  [--format float|int16|int24]` — available **without** the `realtime` feature
  (no cpal); `--help`. The Python client gained `score_bundle` (relative
  timetag) and the `score` demo that writes `/tmp/clausters_score.osc`.
- **New rosc bug, fixed for both modes**: the multiple-of-4 blob bug
  also breaks blobs **inside a bundle** — the element is parsed from its own
  slice with a size prefix (the outer padding doesn't reach it)
  and rosc returns the bundle with the content **silently
  empty**. `osc::decode_packet` splits bundles by hand (recursive) and only
  decodes leaf messages with rosc + padding; the UDP server and the
  score loader use it. CLAUDE.md updated.
- **Goldens** (`tests/golden.rs` + `tests/golden/*.wav` float32, scenes in
  `tests/common/scenes.rs` shared with `cargo run --example
  render_golden`): `arpeggio` (the default def, mid-block entries,
  `/n_set`, staggered frees) and `playbuf` (`/d_recv` + `/b_allocRead` at
  44100 with compensated rate, scheduled `/c_set`, `/b_zero` mid-
  playback). Sample comparison with tolerance 1e-4 (libm's sin
  can vary between platforms; on the same machine it's bit-exact) **plus**
  independent signal asserts (frequency via zero crossings, RMS,
  silences) so an old golden doesn't bless a broken render.
  Regenerate only by hand and **listen before committing**.
- **Benchmark** (`cargo run --release --example bench`): offline block
  throughput → real-time factor at 48 kHz, the default def and a Faust
  def (with the feature). Measurement here: ~1790 synth·xRT stable from 32 to 1000
  default synths (≈1800 sine voices in real time); 1 synth alone
  ~1000x (the fixed per-block overhead dominates).

### Verification

- `cargo test`: 80 (72 + 8 from `tests/golden.rs`); `--features faust`: 111
  (+1 synchronous `/d_faust` in NRT). Without default features also green.
  Clippy clean in both configs (only the 2 preexisting warnings).
- E2E: `json_client.py score` → `clausters --nrt` (release): 11 events,
  2.1 s; first non-zero sample at frame 4801 (4800 is sin(0)=0) and
  the last note freed exactly at frame 96000 — sample-accurate end
  to end. Live server verified with the demos
  `status ugen buffer bundle quit` after the refactor.

## Post-M7 — Denormal protection (2026-06-11)

At the user's request (the question came from before; the technique was
documented in the `realtime-audio` skill but not implemented). Subnormals
appear in recursive states that decay to zero (filter tails, envelopes,
Faust recursions) and on many CPUs are resolved in microcode 10–100x slower —
exactly when a sound fades out. Three pieces:

- **`dsp::denormals::flush_to_zero()`**: puts the calling thread in
  flush-to-zero mode — MXCSR FTZ+DAZ (bits 15 and 6) on x86-64, FPCR.FZ (bit 24)
  on aarch64, both via inline asm (the `_mm_setcsr` intrinsics are
  deprecated); a no-op on other architectures. It's re-armed in each cpal
  callback (cheap, a couple of register accesses) and armed at the start of
  `render()` — **in both modes**, because FTZ changes results (flushes
  to zero) and the NRT render must stay sample-identical to live.
- **`-ftz 2` in the Faust factories** (`FaustArgs::defaults()`, formerly
  `stdlib()`): the generated code flushes recursive variables below
  the normal range — independent of the architecture and of the thread's FPU
  mode. It was the real exposure: our current UGens don't
  have decaying recursive state (that comes with LPF/EnvGen), but any
  Faust def does.
- **Tests**: `tests/denormals.rs` (the FPU switch: a subnormal result and
  operand collapse to 0 after arming; idempotency; normal math
  intact — each `#[test]` runs in its own thread, not contaminating) and in
  `tests/golden.rs` the Faust tail `1-1' : fi.pole(0.9)` (y[n]=0.9ⁿ leaves
  the normal range near sample 830): no subnormal sample and
  `out[1000] == 0.0` exactly. 82 core tests / 114 with faust, goldens
  intact (the scenes didn't generate subnormals).

## F5 — Faust extensions: waveforms and tables (completed 2026-06-12)

The scope came from the 2026-06-12 review (see "Future milestones" in
PLAN.md): of the original F5 list, what's useful today was implemented —
`waveform` + table primitives — and what the server already solves
another way was dropped.

### What got done

- **Three new ops in the JSON→Box schema** (`src/faust/boxes.rs`):
  - `waveform` with `values` (a non-empty array of numbers): a table embedded in
    the def, computed numerically by the client (wavetables, transfer
    functions for waveshaping) without formatting Faust source. Emits the pair
    (size, content) as in Faust. FFI: `CboxWaveform` receives an array
    of `CboxInt`/`CboxReal` boxes **NULL-terminated** (verified in
    faust's source, `box_signal_api.cpp`).
  - `rdtable` (2 or 3 boxes in `in`) and `rwtable` (4 or 5): composed as
    `seq(par(...), primitive)` — exactly like upstream's `Aux` helpers,
    which this time do **not** have the `boxFmod` slip (checked in
    the source). The short form is the idiom `wf, idx : rdtable` with a
    `waveform` filling (size, init); Faust validates the total arity at
    compile time.
  - A shared `number_box` helper with the numeric shorthand (int if it fits
    in `c_int`, real otherwise).
- **Documented drops** (in PLAN.md and `docs/schemas.md`): `soundfile`
  — audio data lives in the server's buffers; `PlayBuf`/`BufRd`
  → bus → the Faust def's `in` control crosses the signal without copying anything to the Faust
  world — and Faust's native polyphony — the node tree is the
  voice allocator (one voice = one `/s_new`, instances share a
  factory) and the polyphonic mode imposes MIDI conventions alien to the model.
  The interpreter backend (no LLVM) and the Signal API stay tied to the
  M14 wasm target.
- **Python client**: a `wavetable` demo — a 256-point table (4 sawtooth
  harmonics) computed in Python, normalized and sent as a `waveform`;
  an oscillator with `freq`/`amp` as sliders.
- **Docs**: new rows in the op table and a "Tables and
  waveforms" section in `docs/schemas.md`, with the buffers-as-signal pattern.
  Server banner to F5.

### Verification

- `cargo test --features faust`: 118 (+4: exact cycle through the table with
  a `& 3` counter; a 64-point wavetable oscillator at 440 Hz with RMS
  1/√2; explicit `rdtable` with a constant init; `rwtable` write-and-read;
  plus 4 validation error cases and the ops in the kitchen sink).
  `cargo test` core: 82, intact. Clippy clean in both configs (only the
  2 preexisting warnings).
- E2E: the `wavetable` demo against the release server with faust — `/done
  /d_faust jwavetable`, `/s_new` + `/n_set freq` audible, `/quit` clean.

## M9 — Developer documentation (completed 2026-06-12)

### What got done

- **`docs/architecture.md`** (in English, like all of `docs/`): a thread map
  (network / audio / NRT / Faust compiler, plus the single-thread offline mode and
  where flush-to-zero is armed), a module map (path → content table),
  the memory lifecycle (the rule "allocated on network/NRT/compiler, used
  on audio, freed on network"; the two crossings without a FIFO: atomic control
  buses and immutable `Arc` buffers), **a table of pre-allocated capacities
  with each one's failure mode when full** (verified case by case
  in the code: cmd FIFO → `/fail`, garbage FIFO → a 64-entry retention list
  retried per block and `mem::forget` as a last resort, events
  best-effort, schedule queue → non-empty `SpentBundle`, slab/groups →
  `Rejected*`), clocks and scheduling (sample clock, NTP conversion on the
  network thread, block split), and the **8 invariants** a change cannot
  break (RT-safety, pre-built commands, mandatory `decode_packet`,
  RT/NRT identity, immutable buffers, output only via
  `Out`/`ReplaceOut`, core without features, determinism in tests).
- **"How to add a UGen" guide**: a complete `Lag` example (state in the
  struct, `at()` for inputs, `output.len()` ≠ `BLOCK_SIZE` due to the splits,
  `ctx.offset` only for bus UGens), registration in `registry.rs` (the variant
  + `parse_kind`/`arity`/`build`), required tests (signal + no-alloc +
  golden if applicable, with the `WhiteNoise` determinism note) and what
  documentation to update.
- **Decision (a), Faust UI**: the labels are the control names on
  purpose (the author of the def picks the names, like `controls` in the
  UGen JSON); group paths ignored, first declaration wins,
  `out`/`in` reserved at the end and the def overrides the reserved ones if it declares
  them; the params are NOT tied to control buses today (that's M11/`/n_map`).
- **Decision (b), plugins**: no dynamic plugins in v1 — Rust has no stable
  ABI; extend = compile in the crate (the documented internal API is the
  contract) and the runtime path for users is `/d_faust`; if they're ever
  needed, a **versioned** C ABI or wasm (scsynth's lesson, same policy as
  the M14 shm layout).
- **Pointers**: CLAUDE.md now lists the two `docs/` docs (with "keep
  them current"); schemas.md opens by referring to architecture.md for
  internals.
- **Rustdoc**: the 2 warnings (links to `FaustArgs`, a private item, from
  `denormals.rs` and `compiler.rs`) fixed to plain text; `cargo doc
  --no-deps` clean with and without the feature.

### Verification

- The doc's claims verified against the code before writing them:
  capacities and constants (`engine.rs`, `node/mod.rs`, `dsp/mod.rs`,
  `buffer.rs`), the socket timeout (100 ms), the `pending_garbage` retry
  per block, `WhiteNoise`'s global seed, the
  `current_samples() + delta·sr` conversion.
- `cargo test` 82 / `--features faust` 118 — intact (only doc
  comments in `src/` changed); `cargo doc` with no warnings in both configs.

## M8 — The sample clock as the client's timebase (completed 2026-06-12)

The server exposes its sample clock and accepts scheduling by absolute sample;
the client can use the audio clock as master instead of the OS clock
(which drifts tens of ppm relative to the DAC crystal). The two
paths coexist: NTP (M6) and samples (M8) feed the same
`Cmd::Schedule` queue, so clients of both types coexist against the same
server.

### What got done

- **`/clock`** → `/clock.reply h <samples> d <sampleRate>`: the engine's
  sample counter (the `AtomicU64` it already published since M6) and the device's
  real sample rate.
- **`/sched <h target> <b packet>`**: schedules a full OSC packet at an
  **absolute** sample, atomic and sample-accurate (the same block split as
  M6). Decisions: a container message instead of reinterpreting the timetag
  (which is NTP format by spec — don't break standard clients);
  the blob's internal timetags are **ignored** (a `/sched` = one instant);
  a past target = next block, like late NTP bundles; an
  `i` int32 target tolerated (hand clients) but it overflows in <13 h at 48 kHz;
  `/fail` per bad individual message, the rest of the packet fires anyway
  (same criterion as `schedule_bundle`); not schedulable within an NTP
  bundle nor valid in NRT scores (score timetags are already exact).
- **Reference client `examples/sample_clock.py`** (stdlib, imports the
  OSC helpers from json_client.py, which gained the `h` int64 tag — encode with
  an `Int64` marker, decode — and `reply(quiet=)`): a `SampleClock` class with
  NTP-style anchors (t0/t1 around the query, a (midpoint,
  counter) pair, uncertainty = half-width), least-squares fit over a sliding
  window of 64 anchors (forgetting), `now()`/`local_time_of()`, and an
  8-note pattern with **sample-exact spacing** scheduled
  ahead (0.3 s lead) re-anchoring on each beat. The demo's honesty: the
  slope needs minutes of baseline to show real drift — on a short
  run the quantization by counter buffer jumps dominates (bounded
  noise: it only affects when a /sched is *sent*, never when it
  fires) — and the report says so.
- **Docs**: `docs/sample-clock.md` new (protocol, model recipe, why
  latency doesn't matter, caveats: samples processed vs heard,
  pause on xruns, buffer jumps; difference from scsynth) + a paragraph in
  schemas.md (Timed bundles) + architecture.md (the two front-ends of the
  same queue). Banner to M8.

### Verification

- New tests: `/clock` reports the counter and advances with the blocks
  (tests/osc.rs); validation of `/sched`'s arguments (no args, no blob,
  negative target, garbage blob, unschedulable query → `/fail` naming the
  message); an `Int` target + a bundle blob with a future NTP timetag ignored;
  and the central one in tests/scheduling.rs: `/sched` mid-block and an assert
  of the **exact sample** (5026 = target+1 due to sin(0)=0) — without the neighborhood
  the equivalent NTP test needs. 86 core / 122 with faust, clippy and
  rustdoc clean.
- E2E with the real server: the full `sample_clock.py` (anchors, model,
  8 audible regular beats, reported slope) and GUIA.md's `/clock` one-liner
  (the counter advances ≈22050 over 0.5 s at 44.1 kHz).

## M12 — Bus-connection auto-ordered groups (completed 2026-06-12)

The server infers the dependency DAG between nodes from the
buses each def reads (`In`, Faust's `in`) and writes (`Out`/`ReplaceOut`,
Faust's `out`), and keeps **opt-in auto-ordered groups**: groups become
multitrack channels and the client stops micro-managing order.
Zero changes on the audio thread: the reorderings arrive as
ordinary `Cmd::MoveNode`.

### What got done

- **`src/osc/graph.rs`**: `BusUsage` (`u128` read/write bitmasks
  + a `dynamic` flag), per-def analysis — `ugen_usage` (constant or
  control bus indices = static, recording which controls are bus
  indices; a signal index = `dynamic`) and `faust_usage`
  (`out..out+N` / `in..in+M` from the reserved controls, the same clamps
  as `FaustSynth`) —, `stable_topo_sort` (stable Kahn: among the ready,
  the earliest of the current order wins; a barrier = a dynamic node with edges
  against everything by position; a deadlock = a cycle → the earliest is
  released: cycles keep relative order = one block of delay, like a return
  in multitrack; `ReplaceOut` counts as read+write, so an insert fx
  lands between sources and readers; pure writers to the same bus don't
  generate an edge — mixing commutes), and **`TreeMirror`**: a mirror of the tree
  on the network thread (topology, per-node control values, usage, the auto
  flag per group) fed by the same `Cmd` stream the engine receives,
  with rollback via the rejection garbage (`remove` idempotent).
- **`CmdTranslator` integrates the mirror**: each arm of `translate()`
  updates the mirror and, if the topology or usage changes, reorders the
  ancestor chain that is auto (`resort_from`), appending moves to the same batch
  — that's why it works the same immediately, in timed bundles (the sort
  fires atomic with the bundle) and in **NRT scores** (the renderer shares
  the translator). A `/n_set` over a control used as a bus index
  re-analyzes and reorders. `/n_before`/`/n_after` with a node or target inside
  an auto group → `Err`/`/fail`. Frees don't reorder (removing
  nodes never invalidates a topological order).
- **Protocol**: `/g_sortMode groupID mode` (1 = auto, 0 = manual; accepts
  pairs; root allowed; schedulable), `/g_queryTree [gid] [flag]` →
  `/g_queryTree.reply` in **scsynth format** (flag 1 includes control names and
  values from the mirror) and `/g_dumpGraph [gid]` →
  `/g_dumpGraph.reply` with the inferred graph readable (reads/writes/dynamic
  per child).
- **Refactor**: the server's duplicated immediate handlers (`/s_new`,
  `/n_set`, `/n_free`, `/n_before`, `/n_after`, `/g_new`, `/g_freeAll`,
  `/g_deepFree`) unified into `handle_via_translate` — a single translation
  path for immediate/bundle/score, which was a prerequisite for the
  mirror not to desync. Along the way, `/g_new` via bundle gained the
  `id > 0` validation that only the immediate path had.
- **Docs and example**: `docs/auto-order.md` (the analysis rules, cycles,
  barriers, the mirror-ahead-of-engine caveat), a section in schemas.md
  (+ `/g_sortMode` in the schedulable list), architecture.md (a module
  row + mirror on the network thread), `examples/auto_order.py` (a source→fx→master
  chain built backwards: silent in a manual group, sounds when
  activating `/g_sortMode`, the graph printed before/after, a second voice at head
  that orders itself), a section in GUIA.md.

### Decisions and caveats

- The mirror reflects commands **when sent**: what's scheduled in a future
  bundle is mirrored already (queryTree may briefly show the
  future state); a re-sort that runs against a pending bundle converges on the
  next change. Documented.
- Dynamic barriers: nothing is ordered across them even if the static
  subgraph asks for it (conservative on purpose).
- The mirror's capacity is the network thread's (HashMaps): the real
  limits are set by the engine and rejections roll back via the garbage.

### Verification

- 10 new tests in `tests/auto_order.rs` (+1 with faust): an inverted chain
  ordered and **audible** (RMS exactly 0.1/√2) vs. silence in a manual group;
  `/g_sortMode` over existing children and back to manual; `/fail` of manual
  moves in auto groups and of `/g_sortMode` over nonexistent groups or
  synths; the full `/g_queryTree.reply` format with flag 1; a dynamic
  barrier reported and respected; a feedback cycle keeps insertion
  order with the source ordered first; a `/n_set` of a control index
  reorders (silence → sound); an NRT score with `/g_sortMode` renders the
  inverted chain; a Faust def ordered by its reserved `out` control.
  **96 core / 133 with faust**, clippy and rustdoc clean.
- E2E: `examples/auto_order.py` against the real server — the dump before/after
  shows the reorder (manual: master,fx,src → auto: src,fx,master) and the
  chain sounds.

## M13 — Parallel tree processing (completed 2026-06-12)

The independent children of a group marked with `/g_parallel` run in
parallel over a pool of workers (`--workers N`), by **stages** derived
from the same M12 bus analysis — the analog of supernova's `ParGroup`
but **inferred and verified by the engine** instead of promised by the
user: a wrong declaration doesn't corrupt audio, it just serializes.

### Central design decision

The `BusUsage` masks travel **to the engine** inside `Cmd::AddSynth` (and
are re-sent with `Cmd::SetUsage` when a `/n_set` touches a control used
as a bus index). Stage partitioning happens on the audio thread with its own
data — pure bitops, no allocation — so the *safety* of the
parallelism never depends on the network mirror (which can run ahead due to
scheduled bundles). A greedy rule per block, in child order: a child
enters the stage as long as it doesn't write anything the stage reads or writes, nor
read anything the stage writes; the conflict closes the stage (= the
writers to the same bus serialize themselves, in order); a `dynamic` child
runs isolated; subgroups = units (the subtree's union); nested parallel
groups within a worker run sequential (v1).

**Key consequence: bit-identical to sequential.** A stage's members
touch pairwise-disjoint buses and don't read what the stage writes ⇒
their results don't depend on the interleaving; the stages preserve order
⇒ the same sums in the same order. `--workers` only changes wall
time. Goldens and RT/NRT identity intact.

### What got done

- **Support refactor**: `BusUsage` moved to `dsp` (analysis and the
  engine use it); the audio buses moved to per-bus `UnsafeCell`
  (`Buses::audio()`/`audio_mut()` unsafe with a documented contract) and
  `ProcessCtx.buses` is `&Buses` (the struct is now `Copy` — each worker
  carries its own); the `NodeTree`'s slots moved to
  `UnsafeCell<Option<NodeSlot>>` with `unsafe impl Sync` (disjoint subtrees
  per stage = one visitor per slot); `NodeKind::Synth` now
  carries `{ node, usage }`; the process went from a DFS stack to recursion with
  `process_index` (with a pool) and `process_index_seq` (workers: no nested
  fork-join).
- **`server/workers.rs`**: a fork-join pool. The conductor publishes the stage
  (job + cursor + remaining + Release epoch), wakes only the
  parked ones, participates in work stealing (cursor `fetch_add`), waits for
  `remaining == 0` and then `active == 0` (the `active` counter closes the
  ABA window of stragglers over cursor/job). Workers: bounded spin →
  yield → park (re-check anti-lost-wakeup); FTZ armed at birth (the two
  modes stay sample-identical in parallel too). The conductor's path with no
  allocations or locks; the only syscall is `unpark` on leaving idle.
- **Protocol and CLI**: `/g_parallel groupID mode` (schedulable, NRT scores
  included, mirrored for `/g_dumpGraph` which now shows
  `(auto, parallel)`), `--workers N` in the RT server and in `--nrt`
  (`RenderConfig.workers`); `engine_pair_with_workers` (the usual `engine_pair`
  = 0 workers: the whole previous suite runs identical).
- **Benchmark** (`examples/bench.rs`, new section): 8 subgroups × 125
  sines on disjoint buses — on this machine ~1.76x with 1 worker, ~2x with
  2, **~3.3x with 3** and degradation with 7 (SMT/contention), against the
  ~1790 synth·xRT of one core.
- **Docs**: `docs/parallel.md` (usage, stage formation, determinism,
  when it doesn't help), architecture.md (workers in the thread map, a module
  row, invariants 1 and 4 expanded: the partition rule is the
  unsafe contract of `audio_mut` and of the slots), schemas.md
  (`/g_parallel` schedulable + a paragraph), GUIA.md (the M13 section with the bench
  as a demo + checklist + counts).

### Verification

- `tests/parallel.rs` (4): **bit-identity** sequential vs 3 workers over
  a torture graph (disjoint sources, a nested subgroup as a unit, 2
  insert fx, 2 conflicting masters serialized, a dynamic node, and a
  `/n_set` that re-points a bus mid-test); survival of many
  publish/park/unpark cycles + clean shutdown; `/fail` of `/g_parallel`
  over non-groups; **NRT with workers bit-identical**. `tests/rt_safety.rs`
  gained `parallel_dispatch_does_not_allocate` (16 disjoint sources, 2
  workers, 300 blocks under `assert_no_alloc` in the conductor; the workers
  run the same process code already covered — the per-thread guard does not
  wrap them, noted as a known limit).
- E2E: RT server `--workers 2` with an auto-ordered, parallel chain sounding
  (dump `(auto, parallel)`); `--nrt --workers 2` produces a WAV
  **byte-identical** to sequential (`cmp` clean).
- **101 core tests / 138 with faust**, clippy and rustdoc clean.

## M14 — Local transports, embedded mode and synchronous calls (completed 2026-06-12)

OSC stays as the single encoding; the UDP transport is joined by two locals
built on a **versioned shared-memory segment**, and the
server can be embedded as a library with a C ABI. Asynchrony stops
being mandatory for the client: a synchronous facade (blocks the caller,
never the server) and a 100% synchronous offline render for the
scientific flow.

### What got done

- **`server/ipc.rs` — the segment** (135,360 bytes, ABI v1, pinned by
  test): a header with a magic + **layout version** (mismatch = rejection on
  connect; scsynth's ABI lesson), sample rate, and two planes:
  - **Data plane**: the sample clock **mirrored by the audio thread on
    each block** (one extra Release store in `process_block`; M8 anchors without
    transport jitter) and the **control buses living inside the
    segment** — `ControlBuses` was refactored to a pointer + owner
    (`from_raw`), so the engine's `InCtl` reads the same atomics the client
    process writes: an external write sounds the next block with no
    command at all.
  - **Command plane**: two SPSC byte rings (64 KiB each, OSC packets
    with a length prefix, Release/Acquire head/tail). Unlike
    UDP: **backpressure** instead of silent loss. Content as
    untrusted as a datagram: `decode_packet` validates and garbage
    re-syncs the ring instead of hanging it.
  - Backings: a mapped file (`mmap` MAP_SHARED via libc, already transitive
    from cpal — zero new deps; put it in `/dev/shm`) or aligned heap
    (in-process). Windows deferred.
- **`ClientId` refactor** (`osc::ClientId::{Udp, Ring}`): client
  identity stopped being a `SocketAddr` in server.rs, `NrtRequest` and
  `CompileRequest`; replies are routed by transport. The loop drains the
  ring on each iteration; with a ring connected the socket timeout drops to
  2 ms (v1 with no cross-process semaphore: command latency bounded by the
  tick, the data plane without latency — an explicit deferral).
- **CLI**: `clausters --shm <path>` creates the segment and connects it (coexists
  with UDP and `--workers`).
- **Embedded C ABI** (`src/embed.rs`, feature `embed`, crate-type cdylib):
  `clausters_abi_version` (== the segment's version),
  **`clausters_render`** — the synchronous scientific call: a binary
  score → flat f32 frames (pointer + length, the basic-structures
  boundary) —, and the live in-process server: `clausters_open` (device
  + engine + network loop with the host as a ring client; an ephemeral
  localhost socket only as a debug tick/escape), `send`/`poll`,
  `clock`/`sample_rate`/`ctl_set`/`ctl_get` directly to the data plane,
  `close` (sends `/quit` over the ring and joins).
- **Python binding** (`clients/python/clausters.py`, pure stdlib):
  `ShmClient` (mmap + struct: the layout parsed by hand, the same offsets as
  Rust), `Clausters` (ctypes over the cdylib, checks the ABI on load),
  `render()` → `array('f')` (numpy can wrap it without copying — the
  client's choice, not a dependency), and `request()` = the **synchronous
  facade** over both transports (over UDP it already existed:
  `json_client.Client.reply`). Correlation by serializing requests; a protocol
  token deferred.
- **Demos**: `examples/shm_client.py` (clock read from the segment, `/status`
  via the ring, an audible fade by writing bus 7 in shared memory) and
  `examples/embed_render.py` (synchronous render → WAV).
- **Docs**: `docs/ipc.md` (segment, rings, reference C ABI,
  synchronous facade, pure-Python client caveats), architecture.md (network
  loop, module rows, a new invariant: **every binary boundary is
  versioned**), schemas.md (a transports paragraph), GUIA.md (the M14 section,
  checklist, counts).

### Verification

- `tests/ipc.rs` (5 core + 1 with embed): ring roundtrip and wraparound
  with FIFO order + backpressure without loss; corrupt content
  re-syncs without hanging; file segments validate magic/version/
  size and share memory between mappings; **the whole server speaking
  only via the ring** (status, /s_new audible, /fail routed, /quit) with the
  clock mirrored block-accurate; data plane: an external control-bus write
  read by `InCtl` the next block and visible to `/c_get`;
  `clausters_render` returns exactly 4800 frames and reports errors per
  buffer. The layout size pinned (changing it = bump ABI_VERSION).
- E2E: real server `--shm /dev/shm/clausters` + Python client — the clock
  advancing (+11328 ≈ 0.257 s at 44.1 kHz), `/status` and `/d_recv` via the ring,
  an audible fade via the data plane, `/quit` via the ring shutting down the server; and
  `embed_render.py` → 100800 frames, an audible WAV.
- **106 core tests / 143 faust / 107 embed**, clippy and rustdoc clean.

## M10 — Bounded memory and alignment (completed 2026-06-12)

The "denormals" half of the original idea was already there (post-M7); this is the
memory half. The M9 capacities table stops being just
documentation: it's now **pinned by tests**, and the signal blocks
got cache-line aligned.

### What got done

- **`tests/capacity.rs`** (5 tests): overflows each structure on purpose and
  pins the failure mode —
  - garbage FIFO (1024) + retention list (64): 1500 dead synths without
    collection → a leak bounded by `mem::forget` (the only RT-safe option),
    the engine keeps processing and sounding; the later collection drains
    FIFO + retention (assert 1024..1500 collected);
  - event FIFO (2048): 2400 events undrained → silent drop,
    exact tree state;
  - node slab (1024 with root): 1100 adds → 1023 alive + 77
    `RejectedSynth` that roll back via the garbage (exact count);
  - non-root groups (256 children): 300 adds → 256 + 44 rejections;
  - aligned `Block`: `align_of == 64`, no padding (`size == 256`),
    a `Vec<Block>`'s addresses verified.
- **Alignment**: a `Block` type (`#[repr(C, align(64))]` over
  `[f32; BLOCK_SIZE]`, access via `.0`) for `UGenSynth`'s wires, the
  audio buses (`UnsafeCell<Block>`) and `FaustSynth`'s staging buffers.
  A block = exactly 4 cache lines: no SIMD load splits
  a line. **Measurement** (the plan's condition was "keep only
  if it doesn't get worse"): interleaved A/B bench with `git stash` (1000 synths) —
  WITHOUT {1186, 1283, 1328, 1337} vs WITH {1240, 1281, 1286, 1292, 1315}
  blocks/s: identical means within the machine's noise (±4–8%). Kept
  for the stability argument, not for a measured gain —
  noted as is.
- **Table in architecture.md**: a note that `tests/capacity.rs` pins it +
  a new row for the M14 rings (backpressure; full reply ring = drop with
  log) + a mention of `Block` in the module map.
- **`realtime-audio` skill updated**: failure-mode philosophy
  (reject-and-report / best-effort drop / bounded leak) with a pointer to the
  table and the tests; a new alignment section; the denormals section
  rewritten to refer to the real implementation
  (`dsp::denormals::flush_to_zero()` via asm — the old example used the
  deprecated `_mm_setcsr` intrinsic — plus `-ftz 2` and the requirement to arm
  FTZ in every new processing thread).

### Verification

- **111 core tests / 148 with faust** (+5), clippy and rustdoc clean.
- Goldens intact (alignment changes no value — `size_of` doesn't
  change, only the base address).

## M11 — `/n_map` and `/n_mapa`: buses as a parameter source (completed 2026-06-13)

The plan's last milestone. `/n_set` writes a control once; `/n_map` ties it
to a **control bus** and `/n_mapa` to an **audio bus**, re-read at the
start of each block. It unifies the two worlds that used to diverge: a UGen
def read control buses only if it included `InCtl` in its graph, and Faust
parameters only moved via discrete `/n_set` — now any
control or zone is tied to a bus with the same command.

### What got done

- **Audio thread**: a trait `SynthNode::map_control(index, bus, audio)` and a
  pre-allocated `node::ControlMap { bus, audio }` parallel to the controls in
  `UGenSynth` and `FaustSynth`. At the start of `process`, before running UGens /
  `compute`, the synth pulls each live mapping into its control/zone:
  the control bus's value, or **one sample** of the audio bus (control-rate; a
  control is a scalar per block and Faust zones too — there's no audio-rate
  control, and for an audio signal there's already `In`/the input bus). It's
  written directly to the control, never via `set_control` (which would unmap it);
  a `/n_set` does go through `set_control`, which clears the mapping first, so an
  explicit set always wins (scsynth semantics).
- **Engine**: `Cmd::MapControl { id, index, bus, audio }` dispatched as
  `SetControl`; schedulable in bundles. RT-safe (only changes one table
  entry, never allocates) — pinned by `tests/rt_safety.rs`.
- **OSC**: `/n_map`/`/n_mapa` handlers in `translate` (`ctl bus` pairs like
  `/n_set`, by name or index, `-1` unmaps) and in the server's immediate
  dispatch list. The scheduled path already went through `translate`.
- **Bus analysis (M12/M13)**: the mirror keeps the live mappings per node;
  `fold_maps_into_usage` sums an audio mapping's bus into the node's `reads`
  and marks it a `dynamic` barrier if the mapped control is used as a bus
  index — so auto/parallel groups stay correct under mappings. Fine detail:
  the topo-sort is **stable**, so unmapping doesn't revert the order (there's no
  constraint forcing it), it just stops imposing it.
- **Docs and example**: `docs/schemas.md` (OSC reference + a control-rate
  sampling note), `docs/architecture.md` (a mapping-model subsection),
  `GUIA.md` (the M11 section + checklist), `examples/osc_ping.rs` (the `map`
  subcommand: `/n_map`+`/c_set` live and an LFO→audio bus→`/n_mapa` = vibrato),
  the `scsynth-osc` skill.

### Verification

- **117 core tests / 155 with faust** (+6/+7): `tests/mapping.rs` (4:
  live control-bus tracking, an unmap that keeps the last
  value, a `/n_set` that breaks the mapping, audio-bus sampling),
  `tests/rt_safety.rs` (no-alloc with control and audio mappings per block),
  `tests/auto_order.rs` (`/n_mapa` adds a read edge and reorders),
  `tests/faust_synth.rs` (a Faust zone following a control bus). clippy and
  rustdoc clean; goldens intact.
- E2E against the real server: `osc_ping map` retunes via `/c_set` and arms the
  vibrato via `/n_mapa` without `/fail`.

## Loose item: libfaust upgrade to 2.85.5

- **2.85.5 is the latest release** (2.81.10 → 2.83.1 → 2.85.5; the
  `v2-5-x` tags are old). With the `fmod` and `cos` workarounds already in the tree
  and `lrsh` not exposed, upgrading is safe.
- **Build** (the F0 recipe, reproducible): checkout the `2.85.5` tag in
  `third_party/faust`, `make most` + reconfigure `build/faustdir` with
  `-DINCLUDE_DYNAMIC=ON -DLINK_LLVM_STATIC=off -DLLVM_CONFIG=llvm-config-20
  -DCMAKE_INSTALL_PREFIX=$HOME/.local`, `make -j` and `make install`. It produced
  `libfaust.so.2.85.5` (10.7 MB, dynamic against libLLVM 20). The previous
  install (2.81.10) was backed up in `~/.local/faust-backup-2.81.10`.
- **FFI unchanged**: the C signatures we use (`createCDSPFactoryFromBoxes/
  Signals`, `Csig*`/`Cbox*`, `compute`, `UIGlue`) are identical in 2.85.5;
  the hand-written binding stays valid. Only version mentions were
  updated (`src/faust/ffi.rs`, the recipe in `GUIA.md`).
- **Verification**: the whole faust suite green against 2.85.5 (includes the
  `cos` regression tests and the kitchen-sinks touching each op);
  `ldd` confirms the binaries load `libfaust.so.2 → 2.85.5`. The bugs
  that persist in 2.85.5 (`boxFmod`, `boxCos`, `kLRsh`) stay covered by
  the workarounds / non-exposure.

## Loose item: fix the `cos` box (it returned abs due to an upstream bug)

- Found while checking whether Faust 2.85.5 fixed the known bugs (no:
  `boxFmod` and `kLRsh` are still broken): in `box_signal_api.cpp`, `boxCos()`
  returns `gGlobal->gAbsPrim->box()` (the same copy-paste as `boxFmod`), in
  2.81.10 and 2.85.5. That is, the **box API**'s `cos` op silently computed
  **abs** (verified: box `cos(0.5)` gave 0.5, not 0.8776). The **signal API
  is fine** (`sigCos` uses `gCosPrim`).
- Fix in `src/faust/boxes.rs`: `cos` leaves `unary_op` and is routed through a
  `CDSPToBoxes("process = cos;")` fragment, like `fmod`. A regression
  test `tests/faust_json.rs::box_cos_computes_cosine_not_abs`
  (cos(0.5)≈0.8776, not 0.5). The kitchen-sink didn't catch it because it only
  checks compiles+finite.
- A clone of Faust's source in `third_party/faust` (git-ignored) for work
  with upstream.

## Loose item: upgrade to rosc 0.11

- `Cargo.toml`: `rosc = "0.10"` → `"0.11"`; the API didn't break any call site.
- `src/osc/mod.rs::decode_packet` stayed a **thin wrapper** over
  `decoder::decode_udp`, the single decoding point of all
  transports. Test `osc::tests::multiple_of_four_blob_round_trips` (round-trip
  of a multiple-of-4-length blob, top-level and inside a bundle). The core +
  faust suite green; clippy clean.

## Loose item: Faust **signal API** (`Csig*` / `createCDSPFactoryFromSignals`)

- **A third `/d_faust` format**: besides Faust source (F1) and JSON box tree
  (F2), a JSON with root `{"signals":[...]}` maps Faust's **signal API**
  (the low layer: **explicit** inputs, delays and recursion). The discriminator
  is by JSON shape in `CompilePayload::classify` (shared by
  `osc/server.rs` and `server/render.rs`): root `{"signals":...}` → signal,
  `{"op":...}` → box, text → source.
- **Design**: the signal API only diverges in the **def→factory** step. `Signal`
  is the same opaque `CTree*` as `FaustBox`; `createCDSPFactoryFromSignals`
  has the same shape as the boxes one (a **null-terminated** vector of outputs).
  Everything below (`FaustDef::probe`, `FaustSynth`, controls, the OSC cycle) is
  **reused untouched**. New: `src/faust/signals.rs` (a JSON→Signal interpreter,
  mirroring `boxes.rs`), `src/faust/json_util.rs` (validation helpers
  `err`/`inputs`/`num_field`/`label_field` extracted and shared with
  `boxes.rs`), `Csig*` bindings + factory in `ffi.rs`, a `compile_signal`
  arm in `compiler.rs`.
- **What's distinctive**: **explicit and sample-accurate** feedback —
  `{"op":"recursion","in":[body]}` with `{"op":"self"}` inside
  (`CsigRecursion`/`CsigSelf`, one sample of delay). It's the `self()` the box
  `~` wraps; it fuses the loop into one node, which the graph's `LocalIn`/`LocalOut`
  (1 block) cannot. Inputs `input`, delays `delay`/`delay1`,
  multi-output (one node per output in `signals`).
- **Coverage**: parity with the box API's op set + the signal stuff. Left
  out: `lrsh` (logical right shift): Faust 2.81.10 crashes its own
  `sigtyperules.cpp` with that opcode (`unrecognized opcode : 7`); `round` doesn't
  exist in the upstream signal API (`rint`); N-ary (`selfN`/`recursionN`) is not
  exposed, just like the box only has `~`.
- **Docs/example**: `docs/schemas.md` (a "JSON signal tree" subsection +
  the discriminator), `docs/examples.md`, `GUIA.md` (manual test + checklist),
  `examples/json_client.py` (the `signal` subcommand: a recursion/self sine +
  a one-pole over noise). A note in the `faust-embedding` skill.

### Verification

- **+8 tests** with faust: `tests/faust_signal.rs` (6: a 440 sine by
  recursion/self, a one-pole with a geometric impulse response at the pole,
  multi-output, a def via the synth path, a kitchen-sink touching each op,
  validations with a path); `tests/faust_parity.rs` (+1: box vs signal sine
  match within tolerance); `tests/faust_compiler.rs` (+1: `/d_faust`
  with `{"signals":[...]}` → `/done` over OSC). The faust and core suite green;
  clippy/rustdoc clean. A refactor of `boxes.rs` to use `json_util` without
  behavior change (the arity message is now neutral, no "boxes").
- E2E with real audio OK: `json_client.py signal` against the live server
  (`--features faust`) — both defs load (`/done`), the sine sounds and the
  one-pole over noise filters (a noise tail with high-frequency energy
  ~0.02 of the total, vs ~2 for white noise). Captured from the output node.

## Loose item: intra-synth feedback `LocalIn`/`LocalOut` (1-block delay)

- **UGens `LocalIn`/`LocalOut`** (scsynth-style): **synth-private** feedback
  with 1 control block (64 samples) of delay. The graph is a DAG
  (you can't wire a cycle), so the loop goes through a buffer
  **persistent between blocks** living in `UGenSynth` (`locals: Vec<Block>`,
  unlike `wires` which are recomputed). `LocalIn` (the source, goes first)
  reads it; `LocalOut` (the sink, goes last) writes it. Since `LocalIn` reads **before**
  `LocalOut` writes, it sees the previous block's value → the 1-block delay
  comes from the read-before-write order, with no double buffer. It works the same under
  the M6 block split (the sub-range `[offset..offset+frames]` is operated on).
- **Implementation**: `src/dsp/local.rs` (no-op placeholder structs),
  registered in `registry.rs`/`mod.rs`. The real work is done in
  `UGenSynth::process` (`src/synthdef/instance.rs`), which intercepts by
  `def.ugens[i].kind` — they're the only case that needs **synth-private**
  state that `ProcessCtx` (global, shared by the parallel scheduler)
  cannot carry. `compile` (`src/synthdef/mod.rs`) requires a constant channel
  index, computes `SynthDef::num_locals`, and validates `LocalIn` before
  `LocalOut` per channel (a clear error otherwise). They don't touch global buses → an empty
  `BusUsage` (`osc/graph.rs` already falls in `_ => continue`), so synths with
  feedback stay parallelizable.
- **Limit (documented)**: **block-rate** feedback, not sample-accurate;
  a one-channel loop resonates at `sampleRate/64` (≈750 Hz). For sub-block IIR
  (one-pole/biquad) the loop has to be fused into one node: a recursive UGen or
  a Faust def (`~`/`CboxRec`) — `FaustSynth`'s reason for being.
- **Docs/example**: `docs/schemas.md` (rows + a feedback note),
  `docs/architecture.md` (a "Feedback" section), `GUIA.md` (manual test +
  checklist), `examples/json_client.py` (the `feedback` subcommand: a resonant
  comb).

### Verification

- **+6 tests** in `tests/feedback.rs`: an exact 1-block delay to the sample,
  a per-block accumulator, two independent channels, survival of the block
  split, and two compilation validations (order and constant channel). **+1**
  no-alloc scene in `tests/rt_safety.rs` (a `LocalIn→·0.9→LocalOut` loop). The full
  suite green; clippy with no new warnings (`LocalIn`/`LocalOut` are unit
  structs, they don't trigger `new_without_default`).

## Loose item: UGen vs Faust performance comparison in `bench.rs`

- **Two head-to-head sections** (gated `--features faust`) in
  `examples/bench.rs` run the *same* DSP through both engines (the
  parity pairs from `tests/faust_parity.rs`, sample-by-sample identical),
  measuring **only `process_block`** (instantiation and JIT stay out of the loop):
  a **sine** (`sin(2π·phasor)·0.2`) and a bit-exact **gain** (`·0.5` over
  a shared bus, no transcendental and no f64/f32 asymmetry → pure engine
  overhead). A table with each one's xRT and a `Faust slowdown` column.
- **Finding**: at equal DSP, Faust is **not slower** (the suspicion that
  motivated this), but ~1.3–1.6× **faster** and consistent across all
  voice counts, even in the bit-exact `gain`. Reason: one vectorized
  LLVM `compute` call over the block vs 3 `dyn` dispatches + 2 intermediate wire
  buffers in the UGen graph. The old bench wasn't comparable (default
  `SinOsc·amp → 2×Out` f64 vs Faust `os.osc → 1` by table).
- A minor harness refactor: `measure()` (warmup + measurement loop) and
  `send_cmd()` (send with FIFO drain) shared. Docs: `bench.rs`,
  `docs/examples.md`, `GUIA.md`.

## Loose item: UGen `Impulse` + pristine impulses in `clock_recorder.py`

- **UGen `Impulse`** (`src/dsp/impulse.rs`): an impulse train like
  SuperCollider's — a single-sample `1.0` every `freq` Hz, `0.0` in
  between. The phase starts "due" (`phase = 1.0`) so that the **first**
  output sample is always an impulse: combined with a `/s_new` via
  `/sched` (which splits the block at the target sample), it places a clean
  impulse on an exact frame. `freq = 0` emits that single impulse and silence
  after. f64 phase, no drift. Registered in `src/dsp/registry.rs`
  (enum/`parse_kind`/`arity` 1/`build`) and `src/dsp/mod.rs`; `osc/graph.rs`
  needs no changes (it doesn't touch buses, falls in `_ => continue`).
- **Example**: `clock_recorder.py` replaces the 4 ms tone burst with a
  pristine single-sample impulse (`Impulse(0)·amp`), scheduled at each target
  sample. With no envelope or attack ramp, the marked frame *is* the
  impulse (unlike `SinOsc`, which starts at `sin(0)=0`). Args:
  `--burst-ms`/`--freq` → `--hold-ms` (how long the synth lives before the
  `/n_free`); the onset detector now marks the impulse's edge (a
  single sample in the direct node capture).
- **Docs**: `docs/schemas.md` (an `Impulse` row in the UGen table),
  `docs/examples.md` and `GUIA.md` (the recorded-clock section, now
  "impulses").

### Verification

- **119 core tests** (+2): `tests/scheduling.rs` —
  `scheduled_impulse_lands_on_its_exact_sample` (an `Impulse(0)` via `/sched`
  lands 1.0 on the exact sample and 0.0 on the rest) and
  `impulse_train_is_periodic_to_the_sample` (freq = SR/64 → an impulse every 64
  samples, no drift). The full suite green; rt-safety intact.
- E2E against the real server: 220 impulses in 120 s, exact gaps of 24000
  samples, jitter 0.000 ms (a direct capture of the `alsa_playback.clausters` node,
  which shares the server's PipeWire clock).

## M15 — Comprehensive English documentation (README + mdBook + rustdoc)

A late close-out record: the work was done in an earlier session and ended up
in commit **`5424855` "Documentation"** (an unconventional message, which is why it
wasn't easy to find), but the milestone was never marked closed in
PLAN.md nor noted here. The code/doc was already in `main`'s history; this
entry and PLAN.md's ✅ are the formal close-out.

### What landed (in `5424855`)

- **`README.md`** at the root: overview, quickstart (build → server → an
  OSC command; and an NRT render), feature matrix (`realtime`/`faust`/`embed`),
  links to the book and rustdoc, GPL-3.0 license.
- **mdBook**: `book.toml` with `src = "docs"` (reuses the `docs/*.md` in
  place, zero churn in incoming references), `docs/SUMMARY.md` as the
  index, new chapters `introduction.md`, `getting-started.md`,
  `using-as-a-library.md`, `examples.md`, `contributing.md`. The existing ones
  (`architecture.md`, `schemas.md`, the feature ones) are reused as is. The
  generated HTML (`book/`) is git-ignored.
- **rustdoc**: an expanded crate doc-comment in `src/lib.rs` (engine/network
  split, feature flags, entry points), linked with the book.
- The Spanish files (`PLAN.md`, `NOTAS.md`, `GUIA.md`) stay in
  Spanish and in place. *(Historical note: this was later revised — `PLAN.md`,
  `clients/PLAN.md` and this `LOG.md` were translated to English; only `GUIA.md`
  and the conversation with the user remain Spanish.)*

### Verification

- `mdbook build` (v0.5.3) and `cargo doc` clean, no broken links.
- Explicit deferral (out of the first pass): CI of `mdbook build` + deploy to
  GitHub Pages and `mdbook test`.

## M16 — On-disk def persistence + bitcode cache

The loaded defs (`/d_recv` and `/d_faust`) can now be saved in a
data directory and reloaded by themselves when the server starts, so as not to have
to resend the library each session (intended for importing large
faustlib-style libraries as faustdefs).

### Design (layers B + A, decided with the user)

- **B — JSON definition, transparent source of truth** (both tables).
  `synthdefs/<name>.json` = the `SynthDefSpec` verbatim; `faustdefs/<name>.json`
  = a `FaustRecord` (original source/JSON + libfaust version + payload sha256).
  Reloading = recompiling from there, by the same path as a new
  `/d_recv`/`/d_faust`. The `FaustDef` itself isn't serialized (its factory
  is opaque LLVM JIT state).
- **A — bitcode cache, non-authoritative** (Faust only).
  `faustdefs/<name>.<sha16>.bc` is the LLVM bitcode; on reload,
  `cache::try_restore` re-creates the factory from the `.bc` (skips Faust's
  front-end) only if the libfaust version matches and the file reads well.
  Any miss → recompiles from the source and rewrites the cache. A libfaust
  upgrade invalidates all the `.bc` automatically; a corrupt cache
  never serves a wrong def. The `.bc` is named by the payload sha, so
  an old `.bc` from an interrupted overwrite never pairs with a newer
  record.
- **Startup in parts**: reloads are enqueued on the compiler thread with
  `client = None` (no reply) and drained in `collect_faust_results`, so the
  socket serves from startup and a large library loads incrementally.
- **Data dir**: `--data-dir` > `$CLAUSTERS_DATA_DIR` >
  `$XDG_DATA_HOME/clausters` > `~/.local/share/clausters`. On by default in
  the RT server; `--no-persist` turns it off; NRT never persists. Atomic
  writes (temp + rename). Sanitized names (percent-encoding).

### Implementation

- New FFI in `src/faust/ffi.rs`: `writeCDSPFactoryToBitcodeFile`,
  `readCDSPFactoryFromBitcodeFile`, `getCLibFaustVersion` (from the C-API of
  `llvm-dsp-c.h`). The bitcode is target-independent IR: it's re-JITed to the host on
  read (`target=""`), so a `.bc` is portable between machines of the same
  libfaust.
- `src/faust/cache.rs` (new, faust-gated): bitcode read/write +
  `FaustRecord`/`FaustKind` + `persist`/`try_restore`/`load_records`/`remove`.
- `src/server/defstore.rs` (new, ungated): dir resolution, layout,
  sanitization, atomic IO, synthdef persistence. The Faust part of the wiring is
  gated.
- `src/faust/compiler.rs`: `CacheJob` (boxed in `CompileRequest`),
  `client: Option<ClientId>`, `run_request` (tries the cache and if not, compiles +
  persists). `src/osc/server.rs`: `store: Option<DefStore>`, `attach_store`
  (reload on start), persistence in `/d_recv`/`/d_faust`, deletion in
  `/d_free`. `src/main.rs`: flags `--data-dir`/`--no-persist`. `d_recv` now
  returns the def's name. New dep: `sha2` (pure Rust).

### Verification

- **`tests/persistence.rs`** (3 core + 6 faust): sanitization, on-disk
  synthdef round-trip, `resolve_data_dir`; **sample-identical** bitcode round-trip
  (compile → write → read → render byte-for-byte equal), persist/restore by
  record, rejection on version mismatch, fallback on a corrupt `.bc`, end-to-end
  reload between two `OscServer` instances over one dir, and deletion
  of files via `/d_free`.
- The core and `--features faust` suite green; clippy clean (tests included).
- Docs: `docs/schemas.md` (on-disk format + flags), `docs/architecture.md`
  (lifecycle), `docs/examples.md`, `GUIA.md` (two sessions + a checklist row),
  `examples/persistence.sh`.

## C0 — Workspace + shared native core + C-ABI (client track)

The client's first milestone (plan in `clients/PLAN.md`). It lays the base for
the Python client (and the future JS one) to share native code with the server.

- **Workspace**: the root becomes a workspace (`[workspace]`, `resolver = "3"`)
  staying the server crate; the new crates live in `crates/`.
  All existing paths (build.rs, tests, examples,
  `target/…/libclausters.so`) stay intact.
- **`crates/clausters-core`** (new): the pure, dependency-free core on the
  hot path. Modules:
  - `builtins`: unary/binary ops over a scalar and a slice (with `dsp::at`-style
    broadcast). `Add/Sub/Mul/Div` are the server's; the rest mirror the
    Faust Signal API with the same formula. `#[repr(u32)]` enums as the
    C-ABI contract.
  - `rng`: `splitmix64` + `WhiteNoise` identical to `dsp::noise`.
  - `tempoclock`: an affine beat↔second mapping (with tempo rebase),
    sec↔sample helpers and a `Scheduler` (a min-heap by beat, stable).
  - `osc`: NTP timetag, instant→sample conversion by anchoring, bundle
    assembly (the only dep `rosc`, not fit for the audio thread).
- **`crates/clausters-ffi`** (new, cdylib + rlib): a C-ABI over the core
  (`clausters_core_*`), version `CORE_ABI_VERSION = 1`. It exposes builtins over
  arrays, seeded white noise and the clock/sample scalars. OSC assembly
  via FFI is deferred to C2 (when the Python client needs it). Artifact:
  `libclausters_ffi.so`, distinct from the embed's `libclausters.so`.
- **Server refactored to `clausters-core`** (equivalence by construction):
  `dsp::binop` uses `builtins::binary_slice` and the core's `BinaryOp`; `dsp::noise`
  delegates to `rng::WhiteNoise` (only the per-instance seeding stays in the
  server). RT-safety intact (`#[inline]` functions, no alloc).

### Verification

- `tests/core_parity.rs` (new): the `Add/Sub/Mul/Div` UGens via the real
  `UGen::process` path give bit-identical results to `clausters_core::builtins`
  (full block and constant broadcast); the server's `WhiteNoise` runs by
  delegation.
- Unit tests in `clausters-core` (14) and `clausters-ffi` (4). The server's
  suite, `--features embed` and `--features faust` green (Faust parity
  unchanged); `tests/rt_safety.rs` and `tests/denormals.rs` stay green.
- Commands: `cargo test` (server), `cargo test --workspace` (includes the
  core and FFI crates), `cargo build -p clausters-ffi` (generates the cdylib).
- **Documented equivalence contract**: bit-exact for the server's native
  ops; for the higher math (Faust-only in the server) the core
  uses the same formula, with no bit-for-bit guarantee against Faust's LLVM codegen
  (tolerance, to be set at its consumption).

## C1 — Python package scaffold + accessible core (client track)

Scaffolding for the high-level Python client and its access to the native core. There is no
base/seq/defs layer yet (that's C2–C4); this leaves the package importable and the core
usable from Python.

- **Package** `clients/python/clausters/` with `pyproject.toml` (setuptools,
  stdlib-only at runtime), `README.md` and placeholder subpackages `base/`,
  `seq/`, `defs/` (each documents which milestone fills it).
- **Transport relocated**: `clients/python/clausters.py` → `clausters/transport.py`
  via `git mv` (preserves history). The `__init__.py` re-exports
  `Clausters`/`ShmClient`/`render`/`ABI_VERSION`/`SEGMENT_SIZE`, so code and
  the `examples/*.py` that do `from clausters import ...` keep working.
  The repo-root computation in `_find_library` was adjusted (one level
  deeper).
- **`clausters/_native.py`**: a ctypes binding over `libclausters_ffi` (lazy,
  versioned load against `CORE_ABI_VERSION = 1`, so importing the package
  doesn't fail if the cdylib isn't built). Exposes `BinaryOp`/`UnaryOp`
  (IntEnum with the core's discriminants), `binary`/`unary` (scalar or
  sequence, with broadcast; return a float or `array('f')`), `white_noise`, and
  the clock/sample scalars. Boundary rule: only flat data crosses.
- **`clausters/base/_osclib.py`**: a minimal OSC wire encoder (stdlib) —
  `message`, `bundle`/`score_bundle`, `score` — equivalent to the helpers of
  `examples/json_client.py`, to build scores that render identically. The
  RT/NRT/MIDI interface abstraction goes in C2.

### Verification

- `clients/python/tests/test_smoke.py` (pytest; also runnable with
  `python tests/test_smoke.py`): re-exports, scalar/list/broadcast builtins +
  higher math, deterministic and in-range white noise, TempoClock
  conversions, OSC bundle assembly, and `render()` of a score with the `default`
  synth (14400 frames @ 48k, peak = amp). Skip-aware tests if a
  cdylib is missing.
- Smoke run inline (pytest not installed in the environment): **all
  checks pass**. For `render`, `transport._find_library` prefers
  `target/release/`; if there's an old `libclausters.so` there **without** the
  embed feature, use `CLAUSTERS_LIB` or build the release with
  `--features embed,realtime`.
- Commands: `cargo build -p clausters-ffi` (core) and
  `cargo build --features embed,realtime` (transport `render`); then
  `cd clients/python && python -m pytest`.
- Docs updated (the moved transport path): `docs/examples.md`,
  `docs/ipc.md`, `docs/schemas.md`, `GUIA.md`.

## C2 — Python client base layer (client track)

A selective port of `sc3/base`. The central piece is the **target-interface
seam**: one and the same `Routine`+`TempoClock` produces RT events or an NRT score
just by changing the interface, without touching clock or routine.

- **`base/builtins.py`**: numeric ops over a scalar or list, dispatched to the
  core (`_native`) → computed in **f32**, equivalent to the server (Python's
  `float` is f64 and would diverge). Lists with cyclic extension of the shorter
  operand (sc3 semantics). Music-theory helpers (`midicps`,
  `dbamp`, …) in pure Python with the standard formula. Watch out: the module's
  `min`/`max`/`pow` shadow Python's builtins; internally `_py.max` is used.
- **`base/absobject.py`**: `AbstractObject` with operator overloading
  (arithmetic, comparison, bitwise) and named methods, all dispatched by
  four hooks (`_compose_unop/_binop/_rcompose_binop/_narop`). The selectors
  are the same names as `builtins` (value) and later `defs/signals` (graph).
- **`base/stream.py`**: `Stream`/`Routine`/`FunctionStream` + `StopStream`/
  `YieldAndReset`. `Routine` wraps a **generator function** (0 or 1 arg);
  `next(inval)` resumes it (the first resume with the arg, then `.send`), the
  `yield`ed value is the time to wait (in beats). `yield` stays in Python.
- **`base/clock.py`**: a native-backed `TempoClock`. The beat↔second arithmetic
  goes via `_native` (matches the server's sample-clock); the queue is `heapq` in
  Python (the core's `Scheduler` isn't exposed via FFI yet). Two
  drives: `start/run` (real time, a thread + `Condition`) and `render` (NRT, drains
  the queue in beat order without sleeping). `send_bundle` emits to the interface with the
  correct time according to `time_mode` (absolute unix in RT, seconds-since-start
  in NRT).
- **`base/_oscinterface.py`**: `OscInterface` + `OscUDPInterface` (RT, socket),
  `OscNrtInterface`+`OscScore` (accumulates bundles → score → `render()` via the
  C1 transport), `OscTCPInterface` (stub: TCP not implemented in the
  server). **`base/_midiinterface.py`**: `MidiNrtInterface`+`MidiScore`
  functional, `MidiRtInterface` a stub (no MIDI backend as a dependency).
- **`base/netaddr.py`** (`NetAddr` host/port) and **`base/main.py`**
  (`main`: default clock, current time-thread, seeded RNG).
- **`base/_osclib.py`**: added `bundle_at` (absolute NTP timetag) for the
  RT send.

### Verification

- `clients/python/tests/test_base.py` (pytest or `python tests/test_base.py`):
  scalar/list/f32/music builtins, operator overloading by selector,
  routine (yield + inval + reset + StopStream), clock math, the TCP stub,
  and the **star case**: routine→`OscNrtInterface`→score→`render()`.
- Run inline (pytest not installed): **everything passes**. The NRT seam = 120000 frames
  @ 48k (peak = amp); the RT driver smoke = 4 events, clean stop, no
  deadlock.

## C3 — Faust-first defs and server resources (client track)

A Faust-first port of `sc3/synth`. It's the client's center: building Faust defs
from Python and managing nodes/buses/buffers against the server.

- **`defs/signals.py`**: the user interface for FaustDefs. **Lowercase**
  callables (`sin`, `cos`, `min`, `delay`, `hslider`, `recursion`, `input`,
  …) that return a `Signal` (a subclass of `AbstractObject`), and their composition
  —by operators or by functions— builds the server's **JSON signal tree**
  (`{"signals":[…]}`). Constants = bare numbers; explicit feedback with
  `recursion`/`self_` (or the sugar `rec(lambda s: …)`). The selectors of
  `absobject` → Faust ops (`mod`→`rem`, `neg`→`0-x`, bitwise→`and/or/xor`…).
- **`defs/faustdef.py`**: `FaustDef` with the three forms for `/d_faust`
  (`from_signals`, `from_source`, `from_box`); `.payload()` serializes,
  `.control_names()` extracts the controls' labels, `reserved=("out","in")`
  (the output/input buses the server adds).
- **`defs/node.py` / `bus.py` / `buffer.py`**: flat handles (`Synth`/`Group`,
  `Bus`, `Buffer`) and scsynth-style client-side allocators (ids from 1000;
  audio buses reserving the hardware outputs; control 0..1023; buffers
  0..1023; reuse of freed).
- **`defs/server.py`**: a `Server` facade over a `send`/`recv` connection
  (`UdpConnection` by default; supports adapters over the transport). It builds the
  OSC and handles async replies: `add_def` blocks until `/done` (or raises on
  `/fail`), `synth`/`group`/`set`/`map`/`free`, buses (`/c_set`/`/c_get`),
  buffers (`/b_alloc`), `notify`/`status`/`sync`/`quit`. The controls go via a
  dict or a list of pairs (so `in`/`out`, which are keywords, are expressible).
- **`base/_osclib.py`**: added `decode` (OSC message → `(addr, args)`) to
  read the replies.

### Verification

- `clients/python/tests/test_defs.py`: signals (functions + operators +
  `recursion`/`self`), `FaustDef`'s payload and `control_names`, allocators
  (reuse of freed, reservation of outputs), `Server` over a fake connection (the layout of
  `/s_new`, `/d_faust` done/fail, `/n_set`), and the **E2E vertical slice** over NRT.
- Run inline (pytest not installed): **everything passes**. Offline E2E
  graph→`/d_faust`→`/s_new`→control→`render()` = 48000 frames @ 48k (peak =
  amp), with Faust's JIT running in NRT.
- **Live E2E** validated (server + client in the same Bash invocation, the
  CLAUDE.md rule): `Server` over UDP at 57110 → `/status`, `add_def` (compiles
  Faust), `/s_new`, `/n_set`, `/n_free`, `quit`. Requires the binary with
  `--features …,faust`.
- **`clients/python/GUIA.md`** (new, user request; moved from `clients/python/clausters/GUIA.md` to the package root on 2026-06-17): a manual-test
  guide for the client in the style of the root `GUIA.md`, with runnable
  snippets per milestone (C0–C3), an NRT slice and a live slice, and a checklist.

## C4 — Refactor: client/server separation (client track)

A surgical correction detected after C3: `TempoClock` had ended up owning
communication that belongs to `Server` (the representation of the Clausters
server). It was moved, **without rewriting what already worked** (see memory
`separacion-cliente-servidor-clausters`).

- **`base/clock.TempoClock`**: stripped of `target`/`interface` and the methods
  `send_bundle`/`send_msg`/`_emit`/`_when`. It keeps **only timing** (math via
  the core, queue, RT/NRT drives, resuming routines) and exposes time:
  `beats()`, `beats2secs()` and the new `start_time` property. In `_wake` it now
  sets `routine.clock = self` (the running thread carries its clock, sc3-style).
- **`base/stream`**: `Routine`/`FunctionStream` have a `clock` slot (set by the
  clock when resuming; so the `Server` finds the logical time via `main.current_tt`).
- **`base/_oscinterface`**: the base now declares `recv`/`close`;
  `OscUDPInterface` **binds** the socket and adds `recv(timeout)`, so a single
  interface does sending **and** receiving of replies (reconciles the old
  `UdpConnection`).
- **`defs/server.Server`**: now **owns the communication interface**
  (`OscUDPInterface` by default in RT; `OscNrtInterface` for offline; shm/embed
  would be new interfaces) and **emits**: `send_msg` (immediate) and `send_bundle`
  (timed, computes the timetag by reading the running routine's clock and the
  interface's `time_mode`). `request` uses `interface.recv`. `render()`
  (NRT mode) delegates to the interface. `UdpConnection` removed. The pattern in
  routines went from `clock.send_bundle(...)` to `server.send_bundle(...)`.

### Verification

- Tests updated (`test_base.py` seam, `test_defs.py` Server + E2E): the
  connection fake became an **interface fake** (`send_msg`/`send_bundle`/
  `recv`); the seam and the E2E emit via `Server`. Run inline (pytest not
  installed): **everything passes**. NRT seam = 120000 frames; E2E faustdef = 48000
  frames (peak = amp).
- **Live E2E** revalidated (server+client same Bash invocation): `Server`
  over UDP → `/status`, `add_def` (compiles Faust), `/s_new`, `/n_set`, `/n_free`.
- The RT driver smoke with `Server.send_bundle` (the wall-clock branch of `_when`): 4
  events, clean stop.
- Acceptance criterion met: `TempoClock` references no interface/NetAddr;
  changing transport = adding a communication interface to the `Server`,
  without touching clock/seq. `GUIA.md` updated.

## C5 — Sequencing layer (seq) (client track)

A port of `sc3/seq`. A `Pbind` runs RT or NRT just by changing the `Server`'s
interface (the seam), with **`yield`-exact** timing.

- **`seq/event.py`**: `Event` (a dict with note defaults). `play(server)`
  assigns a node id, emits `/s_new` at the current logical beat and schedules the
  release at `sustain = dur*legato*stretch` (by default `/n_free`; with
  `has_gate` it sends `gate 0`). `delta = dur*stretch`. Pitch from `freq`/
  `midinote`/`degree`+scale. Adapted to Clausters (no doneAction yet).
- **`seq/pattern.py`**: a `Pattern` base + value patterns (`Pseq`, `Pser`,
  `Prand`, `Pwhite`, `Pseries`, `Pgeom`, `Pfunc`, `Pn`, `Pconst`) and `Pbind`
  (combines patterns by key → a stream of `Event`s; cuts when a finite
  stream runs out). Implemented as Python generators; sub-patterns are
  embedded in place.
- **`seq/eventstream.py`**: `EventStreamPlayer` = a `Routine` that for each
  event plays it against the `Server` (emitting at the logical beat) and `yield`s
  its `delta`. `Pbind(...).play(clock, server)` builds it.
- **Exact timing (a core refinement, see memory
  `tempoclock-timebase-clausters`)**: the clock sets `routine._logical_beat` when
  resuming; `Server.send_bundle` emits from that logical beat (accumulated by
  `yield`), not from "now". Pacing with `time.monotonic()` (a pluggable
  `timebase` source); the OSC timetag uses a separate **wall/Unix** clock
  (`start_time`), valid for the server. `Server.latency` for RT lookahead.
- **Pending (follow-up)**: an alternative timebase = the server's sample-clock
  (selectable; the `timebase` hook already allows it) + robust tests with both
  options; a more complete score parity golden.

### Verification

- `clients/python/tests/test_seq.py` (with **pytest**, now a dev dependency):
  value patterns, `Event` (defaults/pitch/delta/sustain), `Pbind`, the **exactness
  test** (`/s_new` at exactly `[0, 0.5, 1.0, 1.5]` in the NRT score) and the
  render of the seam (`Pbind` of `default` → score → audio). Full suite:
  **32 passed**.
- **Live E2E** (server+client same Bash invocation): `Pbind` over UDP with
  `latency`, an RT clock in a thread; the synths free themselves at the end
  (`status` = 0 synths). Monotonic pacing, wall timetags.

### Post-C5 follow-up: a context without globals that clobber across threads

A project principle: avoid global state (see memory
`evitar-estados-globales-clausters`); enable **RT and NRT in the same script**.

- **`base/main.py`**: `main.current_tt` became **thread-local**
  (`threading.local`). So several `TempoClock`s (threads) and a live RT clock
  alongside an NRT render don't clobber the "current thread". `Server`/`clock` stay
  explicit per instance; `default_clock` is only optional sugar (never
  required).
- **`tests/test_concurrency.py`** (new): (1) `current_tt` is thread-local
  (a worker doesn't clobber the main); (2) two concurrent NRT clocks render
  independent scores with exact timing (without mixing frequencies or
  times); (3) **litmus**: an RT clock churning in a background thread
  while the main builds an NRT score — the NRT stays exact. Suite: **35 passed**.
### Post-C5 follow-up: selectable timebase (monotonic vs sample-clock)

- **`base/timebase.py`** (new): `Timebase` with `MonotonicTimebase` (default, the
  OS clock; events via NTP bundle) and `SampleClockTimebase(sample, sr)`
  (`now = sample()/sr`, anchored to the **server's sample clock**; `sample`
  is any callable to the counter, e.g. `Clausters.clock`/`ShmClient.clock`).
- **`base/clock.py`**: `TempoClock(timebase=…)` uses the object (default
  `MonotonicTimebase`); `pacing_origin` exposes the timebase's origin in seconds.
  **`defs/server.py`**: in RT, if the timebase is `SampleClockTimebase` it
  emits via **`/sched <absolute_sample>`** (`origin + secs + latency` × sr,
  sample-accurate, no drift) instead of a wall timetag; otherwise an NTP bundle as
  before. NRT (score, logical) is **independent of the timebase**. Added
  `_osclib.immediate_bundle` (the `/sched` blob).
- **Verification** (`tests/test_timebase.py`, **40 passed** in the suite):
  monotonic emits an NTP bundle (not `/sched`); sample-clock paces against the counter
  and emits `/sched` with the exact sample (+ the effect of `latency`); NRT identical with
  both timebases. `/sched` **validated live** (the server accepts and processes it;
  synths sound and free themselves).
- Pending: a real sample-clock reader over UDP (`/clock` anchoring); a more complete
  parity golden; defaults ergonomics without globals; an instance-based SynthDef
  when porting it.

## C9 (partial) — Cross-language documentation

The close-out milestone brought forward because it's the most useful for planning the
JS client and distribution.

- **`docs/clients.md`** (a new mdBook chapter, in `SUMMARY.md` under
  "Library & Embedding"): the **single C-ABI contract** (`clausters-core` →
  `clausters-ffi` + embed/shm, the flat-data rule, the two cdylibs and their
  entry points), the **Python client** (base/seq/defs layers, C0–C5 state,
  how it consumes the C-ABI via ctypes and speaks OSC), the **path to the JS client**
  (the same C-ABI via N-API/wasm, generators/async instead of `yield`; not
  implemented) and the **distribution plan** (Python wheels, npm/wasm JS, Faust
  reproducible in `third_party`). A state table.
- **C-ABI reuse confirmation**: Python (a non-Rust language) already drives the whole
  system (core math, offline render, live server) only via the
  C-ABI + OSC → the boundary is not Python-specific.
- `mdbook build` green with the new chapter.
- Pending for the close-out (C9 ⏳ / C10): an example in `examples/`, a real JS client,
  wheels/npm packaging + Faust in `third_party`; and keeping docs/examples up
  to date (milestone C10).

## C5 — closing the loose ends (golden + ergonomics without globals)

- **`clausters.session.Session`** (new, exported from the package): an explicit
  context that bundles `Server`+`TempoClock`; factories `Session.nrt(tempo)` and
  `Session.live(host, port, …)`; `play(pattern)`/`render()`/`run(s)`/`start`/
  `stop`/context-manager. It gives back sc3's defaults ergonomics **without
  global state**: several sessions coexist (NRT for plot + live RT in the
  same script). Test `tests/test_session.py` (NRT render; two independent
  sessions).
- **`tests/test_golden.py`** (new): a parity golden — the render of the
  `Pbind` path is **byte-identical** to that of the equivalent hand-rolled OSC (same engine
  via the embed render); `list(hi)==list(lo)`, 91200 frames. An end-to-end test of
  event/pattern/timing.
- Suite: **43 passed**. The only C5 pending: an **instance-based** graph when porting
  SynthDef (closed below, "C5 — instance-based UGen graph").

## C6 — UDP sample-clock anchoring (client track)

Completes the sample-clock timebase for the **UDP** transport (before it could only
be anchored easily via embed/shm with `.clock`).

- **`defs/clocksync.py`** (new): `SampleClockModel` — `sample = a + b·t` by
  **least squares** over a sliding window of `/clock` anchors (the round-trip
  midpoint as local time; latency doesn't accumulate, it just shifts the grid
  by a constant). The same model as the server's `examples/sample_clock.py`.
  `UdpSampleClock` — uses **its own socket** (doesn't compete with the `Server`'s);
  `anchor()`/`warmup()`/`track(interval)` (background re-anchoring) and
  `timebase()` → `SampleClockTimebase(now, rate)`. `Server.sample_clock()` builds
  it.
- With this, a `TempoClock(timebase=sc.timebase())` **paces against the server's
  sample clock** and the `Server` emits via **`/sched <absolute_sample>`**
  (sample-accurate, no drift) live over UDP.
- **Verification**: `tests/test_clocksync.py` (model: recovers a clean line,
  measures drift ppm with a known slope, falls back to the nominal rate with 1 anchor,
  a timebase smoke). Suite **47 passed**. **Live** (server+client same
  invocation): `warmup` via `/clock` (rate 44100, anchor ±0.20 ms) → `Pbind` via
  `/sched`, synths sound and free themselves. (Drift on short runs is noisy due to
  the counter's buffer quantization; it converges with a long baseline and only
  affects how early the `/sched` is sent, not when it fires — documented.)

## C5 — instance-based UGen graph (C5 leftover closed, 2026-06-17)

Closes the only recorded C5 pending: the UGen counterpart of the Faust pair
`signals`/`FaustDef`, built **instance-based** (without a global build
context in the style of sclang's `UGen.buildSynthDef`).

- **`defs/ugens.py`**: graph nodes and lowercase callables (the client's
  "instruction set"). `Ugen(kind, inputs)` and `Control(name, default)` share
  `_Node(AbstractObject)`: the four arithmetic operators compose UGens
  `Add`/`Sub`/`Mul`/`Div` (the server's only math UGens); any other
  operator/unary raises a clear `TypeError` ("use a Faust def"). Callables:
  `sin_osc`, `impulse`, `white_noise`, `in_`/`in_ctl`, `out`/`replace_out`,
  `play_buf`/`buf_rd`, `local_in`/`local_out`, `control`. Input order follows
  the server's registry (see `docs/schemas.md`).
- **`defs/synthdef.py`**: `SynthDef(name, *outputs)` walks the graph in
  post-order: each UGen is emitted after its inputs ⇒ a topologically
  ordered `ugens` list (every `{"ugen": w}` points to an earlier node, as the
  server requires) and shared subgraphs are emitted once (dedup by identity).
  Controls collected in order of first appearance (conflicting defaults = error).
  `payload()` → JSON `SynthDefSpec`; `control_names()` parallel to `FaustDef`.
- **`Server.add_synthdef(sdef)`**: in RT it blocks until `/done` (or `/fail`); in
  NRT it scores `/d_recv` at t=0 (the render compiles it before advancing time —
  scsynth's NRT semantics). The same pattern as `add_def` (Faust).
- **Boundary/no-globals**: the graph is the tree of composed objects; nothing
  thread-global, so several defs are built in parallel (memory
  `evitar-estados-globales-clausters`). The rest of `sc3/synth` (more UGens) is
  server-side and stays independent of this leftover.
- **Verification** (`tests/test_synthdef.py`, **56 passed, 1 skip** in the
  suite): structure (a spec identical to the internal `default`, dedup, operators →
  Add/Mul/…, unsupported-operator errors, a conflicting control, LocalIn
  before LocalOut, outputs must be UGens) **without a server**; and a **parity
  golden**: the `Pbind` over a client def equivalent to the `default` renders
  **byte-identical** to the `Pbind` over the internal `default` (same engine via the
  embed render). **Live E2E** (server+client same Bash invocation): `/d_recv`
  → `/done`, `/s_new` instantiates the same as an internal def (synths/defs via
  `/status`). Example `examples/synthdef.py` (prints the JSON, tests the parity,
  optionally writes a WAV).

## C8 — TCP transport (server track M + client, 2026-06-17)

Both ends of the only milestone with a cross client↔server dependency: the
server learns to speak OSC over TCP and the client debuts `OscTCPInterface`.

- **UDP always + TCP optional**: UDP is always bound (the base transport, there's
  no "TCP-only" mode); `--tcp` *adds* the TCP listener. UDP is also
  the TCP's own infrastructure: the loop's wake sends a zero-length UDP datagram
  to the server's socket. (If a TCP-only mode is ever wanted, that wake mechanism
  would have to change — noted.)
- **Server — `src/osc/tcp.rs`** (`--tcp [port]`, default 57110 alongside UDP,
  separate namespaces): **length-prefixed** OSC (4 bytes BE + bytes,
  scsynth's framing, both ways). Multiplexed in the single-thread loop
  **with no async runtime and no new dependency**, with the M14 ring pattern:
  an **acceptor** thread + one **reader thread per connection** split the
  stream into complete OSC frames and pass them through an `mpsc` channel the loop
  drains each iteration (`drain_tcp`, like `drain_ring`). So as not to wait for the GC tick
  (100 ms), the reader **wakes** the loop with a **zero-length** UDP datagram
  to the server's own socket (`run()` treats `len==0` as a wake and reiterates). The
  replies leave via the connection's write-half, owned by the network
  thread in a `HashMap<u64, TcpStream>`; since `&TcpStream: Write`, writing
  a reply needs only `&self` (dead connections are pruned on receiving
  `Disconnected`). `ClientId::Tcp(id)` (id per connection) routes each reply to its
  origin. Max frame 64 KiB; an invalid/0 prefix closes the connection.
- **Client — `OscTCPInterface`** (`base/_oscinterface.py`): a drop-in for
  `OscUDPInterface` (the `Server` uses it the same; the `target` arg is ignored, the
  connection already knows its peer). `send_msg`/`send_bundle` frame; `recv`
  **reassembles** prefix+payload across TCP segments with an
  internal buffer. `--tcp` takes an optional port.
- **Boundary/decoding**: the TCP bytes go through the single `decode_packet`
  like every transport; validation is not relaxed. Timing still rides on
  timetags/`/sched`, so TCP arrival latency doesn't affect *when*
  a scheduled command fires (it only delays the arrival of immediate commands, and
  the wake makes that ~immediate).
- **Verification**: `tests/osc.rs::tcp_status_and_d_recv_roundtrip` and
  `tcp_replies_route_to_the_originating_connection` (no audio device, the same
  `engine_pair` as the UDP tests); `clients/python/tests/test_tcp.py` (framing and
  reassembly across segments with a fake socket, deterministic, no live
  server); live E2E (server `--tcp` + `OscTCPInterface`: `/status`, `/d_recv`,
  a synth). Example `examples/tcp_client.py`. Suites green (Rust `osc`: 21;
  Python: 61). The historical `OscTCPInterface` stub and its test moved to the
  real implementation. **Deferred**: lower-latency via epoll/mio (today the UDP-0
  wake already makes the round-trip immediate), and cleaning up `/notify` entries of
  dead TCP connections (today a reply to a dead connection is dropped).

## C9 — commented client example (2026-06-17)

Advances C9 by closing its **example** item: `examples/sequencing.py`, the
introductory tour of the Python client's high-level sequencing layer.
It shows `Session` (ergonomics without globals) + `Pbind` combining value patterns
(`Pseq` of `degree`, `Pwhite` of `amp`, fixed `dur`) into a stream of `Event`s, and
above all the **NRT/live seam**: the same pattern renders offline (`Session.nrt`
→ render to samples/WAV) or plays live over UDP (`Session.live` → `run(seconds)`)
changing only the session, never the routine. Commented to serve as an entry
point. Validated offline (46800 frames, peak 0.165) and live (E2E same
Bash invocation). Cataloged in `docs/examples.md`.

With this **C9 is closed** (cross-language doc + example). The two heavy items
that were poorly planned inside C9 were pulled out into separate milestones
(they're not of the "same topic" docs/examples): the **real JS client** moved to a new
**"J" track** (to be planned later together with the npm packaging; it's based on
the Python client already done), and the **Python client's wheel packaging**
(with the reproducible Faust build in `third_party`, a user backlog item) moved to
**C12** in the C track's future milestones. The general maintenance of
docs/examples stays **C10** (e.g. cataloging `synthdef.py`/`tcp_client.py`
and refreshing "C0–C5" refs to "C0–C8" in README/`docs/clients.md`), not touched here.

## C10 — docs/examples maintenance sweep (2026-06-17)

A C10 sweep leaving docs and examples up to date with the real state (C0–C9):

- **`SynthDef` review** (requested): the class is unchanged since its
  C5 commit (`db8557e`); the "corrections that were being made" are the design
  decisions of C5, now **documented as they ended up** in the GUIA (the "Own
  UGen def" section): a **topological post-order** traversal + dedup by identity,
  controls collected by name (a conflicting default = error), **only `+-*/`**
  compose UGens (another operator/function raises a `TypeError` → use a Faust def), and
  outputs must be UGens. README/`docs/clients.md` already described it as the
  instance-based counterpart of `signals`/`FaustDef`; verified it matches the
  code.
- **Refreshed states**: "Milestones C0–C5 are done" → C0–C9 in
  `clients/python/README.md` (+ mentions of `timebase`/TCP C8 and `clocksync` C6);
  "C0–C5" → C0–C9 and the state table updated in `docs/clients.md` (the C9 row
  done, JS → track J, distribution → C12/track J).
- **Example catalog** (`docs/examples.md`): added `synthdef.py` and
  `tcp_client.py` (they were missing) alongside the already-cataloged `sequencing.py`.
- **GUIA**: an intro to C0–C9 and a C9 row in the checklist (run `examples/sequencing.py`).
- `mdbook build` clean; Python suite 61 passed. C10 stays active: re-review as
  new milestones land (track J, C11/C12, etc.).

## Next: new features

The original plan (M0–M7), F0–F5 and M8–M14 are complete (M11 closed
2026-06-13). None of PLAN.md's "Future milestones" remain. Loose ends:
more UGens (filters, EnvGen with done actions, Line), buffer streaming
(`leaveOpen`), `/n_query`, multi-client with per-ID notifications, the multi
variants `/n_mapn`/`/n_mapan` (a trivial loop over the already-done command),
and the M14 deferrals (wakeup semaphore, multiple ring clients, JS/wasm).

## The `/sync` barrier and async def sending (client C-series)

- **Server**: `/sync <id>` → replies `/synced <id>` when *all* the async
  commands received before it have finished (Faust compilations, `/d_recv`,
  `/b_*` jobs). Implemented with submitted/drained counters per pipeline
  (each completes FIFO on its thread), `pending_syncs` and `resolve_syncs()`
  drained after each `collect_*` (also on the idle tick, so `/synced` goes out
  with no more traffic). Tests in `tests/osc.rs`
  (`sync_answers_synced_with_the_same_id`, `sync_waits_for_an_async_buffer_alloc`).
- **Client**: `Server.add_def` → **`add_faustdef`** (symmetric with
  `add_synthdef`; no alias). Both `add_*` accept `wait` (kw-only): `True`
  by default = blocks until `/done`/`/fail`; `False` = fire-and-forget.
  `Server.sync()` now does the real `/sync`→`/synced` barrier (before it was a
  `/status` round-trip hack, which did NOT guarantee the end of the compiles).
- **The routines rule** (documented in `Routine` and `Server.sync`): a
  routine's generator **never** must block the clock thread
  (the user's responsibility). To create defs from a routine, use the async
  mode (`wait=False`) and do **not** call a blocking `sync()` there. The
  non-blocking barrier that can be `yield`ed from a routine is future work
  (`OSCFunc` / notifications), which will also replace the current synchronous
  wait (recv in a loop).

## Graph sample rate via `fconst` / `ma.SR` (Signal API)

- **Problem**: the biquad example baked `SR=48000` as a Python
  constant, so the RBJ coefficients were out of tune if the engine ran
  at another rate. Faust's `ma.SR` is not a literal: it's
  `min(192000, max(1, fconstant(int fSamplingFreq, <math.h>)))` — a
  **foreign constant** the compiler resolves in `initCDSPInstance`. It wasn't
  bound (`CsigFConst`/`CsigFVar` were missing in `ffi.rs`).
- **Bound**: `ffi.rs` adds the enum `SType {Int,Real}` and
  `CsigFConst`/`CsigFVar` (+ the box twins `CboxFConst`/`CboxFVar`). Both
  interpreters (`signals.rs`, `boxes.rs`) gain the ops `fconst`/`fvar`
  (`ctype`: `"int"`/`"real"`, `name`, optional `file`); the shared parsing
  lives in `json_util::foreign_args` (+ the helper `str_field`/`cstr`).
- **Python client** (`defs/signals.py`): `fconst()`/`fvar()` and above all
  `sr()` = an exact replica of `ma.SR` (clamp included). `PI`/`TAU` stay as
  **floats** (like `ma.PI`, which is a literal — it doesn't need the server).
- **Example** `examples/biquad_signal.py`: uses `S.sr()` and `S.TAU` in the graph
  (phasor and coefficients); `RENDER_SR` is only the rate asked of the NRT
  render and the WAV header (the host's choice, now decoupled from the graph).
  Verified in tune at 44100/48000/96000 (frames scale, peak ~0.74).
- **Tests**: `tests/faust_signal.rs` (`fconst_reads_the_engine_sample_rate`,
  `fvar_probe_compiles`, ops in the kitchen-sink and `ctype`/`name` validation),
  `tests/faust_json.rs` (box kitchen-sink), `test_defs.py`
  (`test_foreign_constant_and_sample_rate`, `test_pi_and_tau_are_plain_literals`).

## Decision: examples stay under `examples/` until C12 (2026-06-18)

Briefly tried moving the Python client example `biquad_signal.py` into
`clients/python/examples/` (the idea: package-specific examples ship with the
wheel). Reverted — at this stage examples are a **development review/planning
surface**, not a distribution artifact: only `biquad_signal.py` uses the client
library idiomatically, the rest are scaffolding-era demos that will be rewritten
as missing functionality lands, and C12 (the wheel packaging that would justify
the split) has no date. Keeping a single flat `examples/` is the low-regret
choice: a unified catalog to scan, and `biquad_signal.py`'s `sys.path` shim
(written for the repo-root `examples/`) stays valid. Note: Cargo only discovers
the `.rs` examples there; the `.py` files are inert to Cargo, so `examples/`
mixes Rust examples + raw-protocol demos + client-library demos by convention,
catalogued in `docs/examples.md`. The split into `clients/python/examples/` (the
package-dependent examples, `sys.path` shims dropped) is deferred to **C12**, when
the client examples have stabilized. See the C12 note in `clients/PLAN.md`.

## M17 (partial) — MIDI: standard channel-voice actuation core (2026-06-18)

First slice of M17: the **transport-independent server actuation core**, so
standard channel-voice MIDI (not SysEx) is the primary way to drive synthesis
nodes and their named `f32` input controls. The wire transport, the
`crates/clausters-midi` persistence/live crate and the client sub-parts stay
pending (see `PLAN.md` M17).

- **`src/midi/`** (new module):
  - `convert.rs` — one named conversion per MIDI message type (the user's
    correction: `midi2freq`/`velocity2amp` are the note-on/off conversions):
    `midi2freq` (note number + microtonal fraction → Hz, 12-TET, the `f32`
    server counterpart of the client's `midicps`), `velocity2amp`,
    `aftertouch2control`, `bend2control` (bipolar, center `0x8000_0000`),
    `cc2control`, `program2control`. Inputs are MIDI 2.0 / UMP resolution
    (16-bit velocity, 32-bit controllers/pressure/bend) → no 7-bit loss.
  - `mod.rs` — the `ChannelVoiceMessage` taxonomy (note on/off, poly/channel
    aftertouch, control change, program change, pitch bend), MIDI 1.0→2.0
    widening (`widen_7_to_16`/`widen_7_to_32`/`widen_14_to_32`) and a
    provisional `parse_midi1` (backward compatibility: classic 7/14-bit input
    accepted and widened to the same `f32` zones), plus `MidiBinding` /
    `MidiBindings` (per-channel binding, the `(channel, note) → node` voice
    table, and the reserved voice-ID allocator from `MIDI_NODE_ID_BASE =
    3_000_000`, disjoint from client IDs and the `/s_new -1` auto range).
- **`CmdTranslator` (`src/osc/translate.rs`)**: a `midi: MidiBindings` field and
  `translate_midi(ChannelVoiceMessage)` that realizes each message as the
  **same** `/s_new`/`/n_set`/`/n_free` an OSC client would send (note on →
  `/s_new` with `freq`/`amp` from the conversions; note off → `/n_free` or
  `/n_set gate 0`; aftertouch/CC/bend → `/n_set` on the channel's live voices;
  program change → re-select the instrument). Reusing the OSC path makes a MIDI
  voice **byte-identical** to the OSC equivalent. Config commands, also routed
  through `translate` (so RT and NRT share them) and dispatched immediately in
  `osc/server.rs`: `/midi_bind`, `/midi_unbind`, `/midi_map`. The
  binding addresses an instrument by name and drives **SynthDef and FaustDef
  identically** (both expose named `f32` control zones). All on the network
  thread; the audio thread and its RT-safety invariants are untouched.
- **Tests**: `src/midi/` unit tests (conversions, widening, note-on-velocity-0,
  bend center) and `tests/midi.rs` (note-on spawns a voice with converted
  controls from the reserved ID range; **byte-identical parity** with the
  equivalent hand-written `/s_new`; note off frees the right voice; CC sets the
  mapped control on live voices; gate binding releases instead of freeing;
  unbound channel ignored; unbind frees sounding voices). Full suite green,
  `cargo fmt --check` clean, core builds without `faust`/`embed`.
- **Docs**: `docs/schemas.md` gains the "MIDI control protocol" section
  (commands, control map, per-message semantics, conversions, MIDI 1.0
  compatibility, the SysEx-scope note). Not a milestone close: no runnable
  example / `GUIA` E2E yet — those land with the transport.

## M17 (transport) — live MIDI input over ALSA via midir (2026-06-18)

The wire transport for the M17 actuation core: standard OS MIDI in, using
**`midir`** (the live crate the plan pinned; ALSA sequencer on Linux — the same
system MIDI any controller or DAW uses). Network MIDI stays a separate,
out-of-scope idea.

- **`src/midi/live.rs`** (feature `midi`): `MidiHub::open` creates a **virtual
  ALSA input port** named for the server (`--midi [name]`, default `clausters`).
  `midir` runs the input callback on **its own thread**, which decodes each MIDI
  1.0 message with `parse_midi1` (widening to the internal high-resolution form)
  and hands the `ChannelVoiceMessage` to the command loop over an `mpsc`
  channel, waking it with a zero-length UDP datagram — the exact TCP-transport
  pattern (`src/osc/tcp.rs`). The audio thread is never involved.
- **`src/osc/server.rs`**: `listen_midi(name)` opens the hub; `drain_midi()`
  (called every loop iteration alongside `drain_ring`/`drain_tcp`) translates
  each queued message with `CmdTranslator::translate_midi` and ships the
  commands. Field/method gated on feature `midi`; a no-op `drain_midi` keeps the
  loop uniform when the feature is off.
- **`src/main.rs`**: `--midi [name]` flag (RT only); prints the open port and a
  hint to connect with `aconnect`. Without the feature it errors with a rebuild
  hint.
- **Build/deps**: new optional dep `midir = "0.11"` behind feature `midi`, which
  is in `default` (it reuses the libasound cpal already links — no new system
  dep). `--no-default-features` still builds with neither `midir` nor `cpal`.
- **E2E** (real ALSA, same Bash invocation): start with `--midi clausters`, the
  virtual port shows in `aconnect -l`; `/midi_bind 0 default` then
  `aplaymidi -p clausters note.mid` (a note-on) makes `/status` report
  **synths 0 -> 1** (ugens 0 -> 4) — the MIDI note created the node end to end.
  Recipe in `GUIA.md`. Live input is MIDI 1.0 (7-bit, widened); full MIDI
  2.0/UMP resolution is the persistence-crate path, still pending.
- **Docs**: `docs/schemas.md` (the `--midi` transport), `docs/architecture.md`
  (the MIDI input thread in the Threads section + module map), `README.md`
  (Features bullet), `GUIA.md` (E2E recipe + checklist).

## M17 (client sub-part 1) — Event pattern -> Standard MIDI File (2026-06-18)

The client offline-file half of M17: a `Pbind` realized as standard MIDI and
written to a `.mid`, on the same clock/routine/pattern as the audio path — only
the destination differs (the double dispatch the plan called for).

- **`crates/clausters-midi`** (new workspace crate, cdylib+rlib): a flat-data C
  ABI over an SMF writer. `clausters_midi_write_smf(ticks, msgs, n, ppq,
  out_len) -> *mut u8` (+ `_free`, `_abi_version`) takes parallel `u32` ticks
  and 3-byte channel-voice messages and returns malloc'd `.mid` bytes — the
  same POD-in/bytes-out shape as `clausters_render`. SMF (type 0) via **`midly`**
  (mature, pure Rust); the **MIDI 2.0 Clip File** (`midi2-clip`, full 16/32-bit
  resolution) is the planned follow-up behind the same ABI. Rust tests: SMF
  round-trip, two-byte messages, C-ABI parity + clean free.
- **Double dispatch** (`Event.play(destination)` -> `destination.play_event`):
  the OSC realization moved verbatim from `Event.play` to `Server.play_event`
  (`defs/server.py`) — `test_golden.py` confirms the render stays **byte-
  identical**. `Event.midinote()` factored out (explicit `freq` inverted via
  `cpsmidi`).
- **`MidiServer`** (`base/_midiinterface.py`): the MIDI destination. `play_event`
  records a note on at the routine's logical beat and a note off after the
  sustain into a `MidiScore` (now keyed by **beat**, not seconds); `write(path,
  ppq)` -> `MidiScore.to_smf` -> the `_midi.py` ctypes binding -> the crate.
  Note number from `Event.midinote()`, velocity from `amp`.
- **Tests/example**: `clients/python/tests/test_midi.py` (Pbind -> note on/off in
  beats; explicit-freq -> note 69; writes a valid SMF). `examples/midi_file.py`
  renders a phrase to `out.mid` (cataloged in `docs/examples.md`). Full Python
  suite green, golden included; `cargo fmt --check` clean.
- **Still pending**: client sub-part 2 (a live `MidiRtInterface` out a port) and
  the MIDI 2.0 clip writer in the crate.

## M17 (client sub-part 2 + clip writer) — live MIDI out + MIDI 2.0 clip (2026-06-19)

Closes the M17 client output: live MIDI out a virtual OS port, and the
full-resolution MIDI 2.0 clip file. Both in the `clausters-midi` crate behind
its C ABI, driven by the same `MidiServer` destination through a swappable
interface (the RT/NRT seam, mirroring the OSC `Server`).

- **MIDI 2.0 Clip File (SMF2CLIP)** — `clausters-midi`: the planned `midi2-clip`
  crate turned out to be a **v0.1.0 stub** (`write_clip_file`/`read_clip_file`
  are `todo!()`), so the container is assembled from **`midi2`**'s typed UMP
  messages (the message layer the plan pinned, which *is* functional): the
  8-byte `SMF2CLIP` header, then DCTPQ + Start of Clip + (Delta Clockstamp +
  Channel Voice 2) per event + End of Clip, words big-endian. Note velocities
  widened to **16 bits** (vs SMF's 7). `clausters_midi_write_clip` parallels
  `_write_smf`; Rust test walks the UMP stream back and checks ticks/velocity.
  Enabled `midi2` features `utility` + `ump-stream`; dropped the unused
  `midi2-clip` dep.
- **Live MIDI output** — `clausters-midi` feature `live` (midir/ALSA, unix):
  `clausters_midi_output_open`/`_send`/`_close` (an opaque handle, the embed-ABI
  pattern) open a virtual output port and send raw bytes.
- **RT/NRT seam** (`base/_midiinterface.py`): `MidiServer(interface=...)` now
  holds a swappable interface. `MidiNrtInterface` accumulates the `MidiScore`
  (`write(path, ppq, fmt="smf"|"clip")`); **`MidiRtInterface`** (real backend,
  replacing the stub) opens the port via `_midi.py` and `emit`s each message at
  its beat — note-on now, note-off scheduled with `clock.sched_abs`.
  `MidiRtInterface.close()` sends an **all-notes-off (CC 123) on all 16
  channels** (the standard MIDI panic) before dropping the port: stopping the
  clock leaves note-offs scheduled past the stop unsent, so without it a partial
  run would hang the last note on the destination.
  `_midi.py` gains `write_clip` and `output_open`/`_send`/`_close` (live symbols
  guarded: a clear error if the cdylib lacks `--features live`).
- **Tests/examples**: `tests/test_midi.py` adds the clip file and a live-output
  smoke (drives a Pbind out a real virtual port). `examples/midi_file.py`
  gains `--clip`; new `examples/midi_live.py` plays a phrase live. **Full-loop
  E2E** (real ALSA, one Bash invocation): client `MidiRtInterface` out port ->
  `aconnect` -> server `--midi` in port -> the server makes synths
  (`/status` reports synths 0 -> 2). Full Rust + Python suites green, golden
  included; `cargo fmt --check` clean.

## M18 (server core + client) — GraphDef + scsynth group /n_set propagation (2026-06-19)

Two related control-graph features (PLAN.md M18; the group-propagation half was
a long-standing loose item). Both live entirely on the network thread in
`CmdTranslator` and lower into the same `Cmd`s as hand-written commands, so the
audio thread and RT-safety are untouched.

- **Group `/n_set`/`/n_map`/`/n_mapa` propagation (scsynth semantics)**: a
  command addressed to a **group** now transfers each named control down the
  subtree to every synth/faust that has a matching control, recursing through
  subgroups and stopping at each synth. New `control_targets` /
  `collect_subtree_synths` helpers gather the targets off the `TreeMirror`; a
  synth target is unchanged, an empty group is a no-op, an unknown id `/fail`s.
  Engine `SetControl`/`MapControl` on an unknown id were already no-ops, so the
  fan-out is safe against concurrent frees. `tests/group_nset.rs` (7).
- **GraphDef (M18 server core)** — `src/osc/graphdef.rs`: a third persistent
  def kind storing a wired configuration of member synth/faust nodes + internal
  buses + a **named parameter surface** (ports → member controls, with mul/add
  scaling). `/d_graph` parses + validates structurally (cheap, no JIT) + stores;
  `defstore` persists `graphdefs/<name>.json`, reloaded at startup after the
  synth/faust defs (members reference their names). `/graph_new` instantiates an
  **auto-sorted** group (M12 orders members by their bus wiring) with
  **instance-private buses** from a reserved top-of-range pool (audio 96..128,
  control 896..1024, a contiguous-run `RangeAllocator`), wiring members via the
  existing `/s_new` + `/n_map` primitives. Instantiation is **atomic** — all
  fallible work (member build, bus alloc) precedes any command/mirror change.
  `/n_set` on an instance group resolves names against the surface (`graph_set`),
  never the private member ids; `/n_free`/`/g_deepFree` reclaim the private
  buses. Works in NRT scores (the renderer shares `translate`). `/d_free` and
  the persistence delete also cover graph names. `tests/graphdef.rs` (8).
- **Client** (`clients/python`): `defs/graphdef.py` — a thin `GraphDef` JSON
  builder (`bus`/`add`/`port`, member control values that are bus refs or
  `"OUT"`, surface targets with `.scaled(mul, add)`), `Server.add_graphdef`
  (`/d_graph`, the same async/`/done` shape as `add_synthdef`) and
  `Server.graph(...)` (`/graph_new`). `tests/test_graphdef.py` (4): builder
  structure + an NRT render that sounds.
- **Examples**: `examples/group_set.py` (one `/n_set` on a group ramps three
  voices) and `examples/graphdef.py` (a two-oscillator voice wired through a
  private bus, one `freq` port driving both oscillators — the second scaled to
  a fifth — which a bare group `/n_set` cannot do). Both render offline and were
  run (peaks 0.52 / 0.11).
- **Docs**: `docs/schemas.md` (group addressing semantics; the full GraphDef
  section — spec, private-bus ranges, `/d_graph`/`/graph_new`/surface; the
  persistence table row; `/graph_new` in the schedulable list),
  `docs/architecture.md` (module-map row + a "Group `/n_set` and GraphDef"
  subsection on the two-phase atomic instantiation and private-bus lifecycle),
  `docs/examples.md`, root `GUIA.md` + `clients/python/GUIA.md`.

Core green with and without `faust`; full Python suite green; `cargo fmt`
clean. Deferred within M18 (noted in PLAN.md): MIDI-binding a GraphDef
(`/midi_bind` → graph) and an explicit per-voice `/graph_voice`; both build on
this core.

## M18 (closed) — per-voice partition (/graph_voice) + MIDI-bind a GraphDef (2026-06-20)

The two deferred M18 sub-parts, closing the milestone.

- **Shared/per-voice partition + `/graph_voice`**: a member tagged
  `"voice": true` is per-voice; `/graph_new` now instantiates only the
  **shared** members, and `/graph_voice instanceID id [port value...]` spawns
  the per-voice members as a sub-group at the head of the instance, wired to
  the same private buses. The auto-sort orders the voice before the shared
  mixer via the sub-group's aggregate `usage_of` (M12). A surface port maps to
  shared *or* voice members, never a mix (`/d_graph` `/fail`s otherwise):
  shared ports resolve at `/graph_new`, voice ports per voice; `/n_set` on an
  instance or a voice id routes through the right surface. `/n_free` of a voice
  forgets it; of an instance reclaims buses and drops its voices. The shared
  steps were factored into `alloc_graph_buses`/`build_members`/`resolve_ports`,
  reused by both paths. `GraphInstance` now stores the `Arc<GraphDefSpec>`, the
  resolved `bus_index` and its voice set; new `GraphVoice` per voice.
- **MIDI-bind a GraphDef**: `/midi_bind channel graphname ...` spawns the
  shared instance at bind time (a reserved MIDI id) and each note becomes a
  `/graph_voice` into it (note → `freq` port, velocity → `amp` port; note-off
  frees the voice or, gate-aware, sets its `gate` port). `/midi_unbind` frees
  the instance (and its voices with it). `MidiBinding` gains
  `graph_instance: Option<i32>`; `midi_bind`/`midi_note_on`/`midi_unbind`
  branch on it. A GraphDef with no per-voice members is rejected at bind.
- **Client**: `GraphDef.add(..., voice=True)` and `Server.graph_voice(...)`.
  Example `examples/graphdef_poly.py` (a shared mixer + a per-voice oscillator,
  an arpeggio of overlapping voices; renders offline, peak ~0.12). Tests:
  `tests/graphdef.rs` +7 (voice spawn/surface/free, mixed-port reject,
  instance-frees-voices, `/midi_bind` to a GraphDef plays voices and
  `/midi_unbind` frees the instance); `test_graphdef.py` +2.
- **Docs**: `schemas.md` (the shared/per-voice + `/graph_voice` section,
  `/midi_bind` to a GraphDef, `voice` member field, schedulable list),
  `architecture.md`, `examples.md`, root and client `GUIA.md`.

Core green with and without `faust`; full Python suite green; `cargo fmt`
clean. **M18 fully done.**

## M19 — MIDI-standalone operation: persisted bindings + boot preset (2026-06-20)

The payoff of M16 + M17 + M18: boot the server and play it from a MIDI
controller with **zero OSC programming**. All network-thread / boot-time, no
new audio-thread state.

- **Persisted MIDI bindings** (`midi.json`): `MidiBinding` derives
  serde (the runtime `graph_instance` is `#[serde(skip)]`); `MidiBindings::
  persist()` exports a channel-sorted `Vec<PersistedBinding>`. `defstore` gains
  `save_bindings`/`load_bindings` (`<dir>/midi.json`). `OscServer` rewrites it
  after every `/midi_bind`/`/midi_unbind`/`/midi_map` (`persist_bindings`).
- **Boot reload** (`attach_store`): fixed order **defs -> graphdefs -> bindings
  -> boot preset**, so a binding's instrument and a boot graph's name already
  resolve. `CmdTranslator::restore_binding` re-establishes a binding from its
  stored config, re-instantiating its shared GraphDef instance via the factored
  `bind_graph_instance` (shared with `/midi_bind`). Restore/boot commands ship
  to the engine through `ship_boot_cmds`.
- **Boot preset** (`boot.json`): an optional user-authored
  `[{"graph": name, "ports": {...}}]` of standalone GraphDefs (`BootInstance`),
  instantiated at boot via the `/graph_new` path (`defstore::load_boot`).
- **Playable-by-default**: the M17 default control map (note->freq, vel->amp,
  note-off->/n_free or gate) already makes a restored binding immediately
  playable — confirmed by tests.
- **Tests**: `tests/midi_standalone.rs` (two real `OscServer`s on one data dir:
  a GraphDef MIDI binding and a boot preset both revive at restart, observed via
  `/g_queryTree`); `tests/persistence.rs` (+2: `midi.json` round-trip,
  `boot.json` load); `tests/midi.rs` (+1: persist -> restore -> a note plays
  through the default map).
- **Example + docs**: `examples/midi_standalone.sh` (set up once, restart,
  the binding is back — run end-to-end with `oscsend`). `schemas.md`
  (the persisted-binding + boot-preset section, persistence-table rows),
  `architecture.md` (boot order), `examples.md`, `GUIA.md`.

Server-side counterpart of the client OSCFunc/MIDIFunc (C13): the server can be
played directly by MIDI (M17/M19) or by a client that emits OSC; both coexist.
Core green with and without `faust`; `cargo fmt` clean. **M19 done.**

## Multi-format buffer reading (2026-06-20)

`/b_read` and `/b_allocRead` now accept compressed and other container formats,
not just WAV. Reading dispatches by **content**, not extension:

- WAV stays on hound (`read_wav`): exact, int24-aware, cheap frame seek, and the
  format `/b_write` emits.
- Everything else decodes through **symphonia** 0.6 (`read_symphonia`): FLAC,
  OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF, CAF. Pure-Rust, decode-only. Compressed
  formats have no cheap exact frame seek, so it decodes the whole file into an
  interleaved f32 buffer and then applies the `fileStart`/`numFrames` slice. This
  runs on the NRT thread, where allocation is fine; the buffer keeps the file's
  own sample rate (the engine never resamples).

`read_audio` is the new dispatcher both jobs call. No OSC, engine or RT-thread
changes — buffers are still immutable `Arc<Buffer>`s installed via `SetBuffer`.

- **Tests**: `tests/buffers.rs` (+1: `non_wav_extensions_decode_through_symphonia`
  writes lossless float PCM to a `.dat` path and reads it back through the
  symphonia branch, checking the full-read and the sliced read). Verified
  end-to-end out of band against real `ffmpeg`-generated FLAC/OGG/MP3 (FLAC
  bit-identical to the WAV; OGG/MP3 RMS within tolerance).
- **Docs**: `schemas.md` (the `/b_*` format note), `architecture.md` (NRT
  thread), `GUIA.md` (manual ffmpeg-based step). Dependency: `symphonia` added to
  `Cargo.toml` (default-features off, explicit codec/container feature list).

## BufRateScale + buffer-info UGen family (2026-06-20)

Stage 1 of the buffer/disk follow-up. The server never resamples, so a file at
a different sample rate than the server must be pitch-corrected at the UGen
level via `PlayBuf`'s `rate`. Until now the client had to compute
`file_sr / server_sr` itself; the new `BufInfo` family reads it from the buffer
at run time:

- `BufInfo(BufInfoKind)` in `dsp::buf`, registered under five scsynth names:
  `BufSampleRate`, `BufRateScale` (`file_sr / server_sr`), `BufFrames`,
  `BufChannels`, `BufDur` (`frames / file_sr`). All take the bufnum as their one
  input and emit a block-constant value (control-rate-like); a missing slot
  reports 0. They read `ProcessCtx::{buffers, sample_rate}` only — no bus use,
  no allocation, no audio-thread state.
- Idiomatic use: `PlayBuf(buf, rate: BufRateScale(buf) * pitch)` plays at the
  file's true pitch without the client knowing either sample rate.
- **Tests**: `tests/buffers.rs` (+1: a 24 kHz buffer on a 48 kHz engine returns
  RateScale 0.5, plus SampleRate/Frames/Dur/Channels).
- **Docs**: `schemas.md` (UGen table rows + the `PlayBuf` note now points at
  `BufRateScale`), `GUIA.md`.

## DiskIn / DiskOut: streaming disk I/O UGens (2026-06-20)

Stage 2 of the buffer/disk follow-up. Stream audio to/from disk in real time,
so arbitrarily long files never load into the buffer pool.

- **Self-contained design** (`dsp::disk`): each `DiskIn`/`DiskOut` owns one
  background I/O thread plus an `rtrb` SPSC ring shared with the audio thread.
  Built on the network thread at `/s_new` (open file, spawn thread); the audio
  thread only pops/pushes the ring (RT-safe — underrun plays silence, DiskOut
  overrun drops samples); the synth `Box` dropped on the network thread (via the
  garbage FIFO) signals stop and joins the thread. No engine, OSC, `ProcessCtx`
  or M13-worker changes — the whole feature lives in the UGen.
- **DiskIn**: streams via symphonia (any decodable format), one file frame per
  server sample (no resampling, like scsynth); `loop` restarts from the top of
  the file (re-open, exact). **DiskOut**: encodes a mono WAV via hound; the
  server sample rate reaches the writer thread through an atomic published on the
  first `process`. Both are **mono per UGen** (the `chan` input / one file each).
- **Static UGen params**: `UGenSpec`/`UGenDef` gained `path`/`loop`/`format`
  (serde-default, omitted when empty), compiled into a new
  `registry::UGenConfig` that `build(kind, &config)` consumes. `compile` rejects
  a `DiskIn`/`DiskOut` without a path.
- **Tests**: `tests/buffers.rs` (+1: PlayBuf -> DiskOut writes a float WAV,
  read back exactly, then DiskIn streams the same file and the samples arrive in
  order). **Example**: `examples/json_client.py disk` (record a sine with
  DiskOut, stream it back with DiskIn). **Docs**: `schemas.md` (UGen table +
  streaming note), `architecture.md` (the disk I/O threads), `GUIA.md`.

## Faust soundfile bridge: read server buffers (2026-06-20)

Stage 3 of the buffer/disk follow-up, and a reversal of the earlier "deliberately
no soundfile" decision. Faust's `soundfile("<bufnum>", n)` primitive now binds to
the server buffer named by its (integer) label, so a Faust def can read sample
memory directly, not only through an audio bus.

- **FFI** (`faust::ffi`): the packed `Soundfile` struct (`#[repr(C, packed)]`,
  matching `gui/Soundfile.h`) plus the `MAX_CHAN`/`MAX_SOUNDFILE_PARTS` constants.
- **Fill** (`faust::synth`): `SoundfileData` owns the `Soundfile` and all memory
  it points at (planar f32 channels, the length/SR/offset part arrays, the
  MAX_CHAN pointer table with channel aliasing). The previously-stub
  `add_soundfile` UI callback parses the label/url as a bufnum, deinterleaves the
  server buffer into a one-part Soundfile, and writes it into the zone; a
  non-numeric label or empty slot yields a silent placeholder so `compute` is
  always safe. Channel arrays are padded one sample (the read index is
  inclusive). The instance keeps the `SoundfileData` alive and drops it after
  `deleteCDSPInstance` (network thread, via the garbage FIFO).
- **Plumbing**: the network-side buffer mirror moved from `OscServer` into
  `CmdTranslator` (which already owns every other mirror), so `make_synth` can
  pass it to `FaustSynth::new(def, sr, &buffers)`. The bind is a snapshot at
  `/s_new`.
- **Tests**: `tests/faust_synth.rs` (+1: a def reading `soundfile("0", 1)`
  returns the buffer's length and sample rate, and a self-incrementing index
  streams the channel back, clamping past the end). **Example**:
  `examples/json_client.py soundfile` (load a WAV into buffer 5, loop it from a
  Faust def). **Docs**: `schemas.md` (rewrote the soundfile note), `GUIA.md` (F6
  section, replacing the old "no soundfile" note), `architecture.md`.

## C12 — Python client packaging: pip-installable wheels (2026-06-21)

The `clausters` Python package is now pip-installable as a self-contained,
platform-tagged **wheel** that bundles the two cargo-built cdylibs, so an
installed package needs no `target/` directory and no build step at import. This
also makes the standard "install from the repo into a venv and run self-contained
tests/examples" workflow trivial.

- **Staging** (`clients/python/build_native.py`): finds the cargo workspace
  (upward search or `CLAUSTERS_WORKSPACE`), runs `cargo build -p clausters-ffi`
  and `cargo build -p clausters --features embed,realtime` (features overridable
  via `CLAUSTERS_CARGO_FEATURES`; `--debug`/`CLAUSTERS_CARGO_PROFILE` for the
  profile), and copies the resulting `lib*.{so,dylib,dll}` into
  `clausters/_libs/`. Runnable standalone, and imported by `setup.py`.
- **Build hook** (`clients/python/setup.py`): a `build_py` subclass stages the
  cdylibs before collecting package files (with `allow_skip` so an isolated build
  of a pre-staged tree still works); a `bdist_wheel` subclass forces a
  non-pure, `py3-none-<plat>`-tagged wheel (the code is pure Python + ctypes, so
  it is platform- but not Python-version-specific). `pyproject.toml` adds `wheel`
  to the build requires and `clausters/_libs/*.{so,dylib,dll}` as package data.
- **Loader precedence** (`clients/python/clausters/_libpath.py`, shared by
  `_native.py` and `transport.py`): env override (`CLAUSTERS_FFI_LIB` /
  `CLAUSTERS_LIB`) -> the bundled `clausters/_libs/` (wheel / editable) -> the
  workspace `target/{release,debug}/` (source checkout). The source-checkout
  fallback keeps the historic build-and-run flow working unchanged.
- **`.gitignore`** for `clients/python` (`clausters/_libs/`, `build/`, `dist/`,
  `*.egg-info/`, caches): the staged cdylibs and packaging artifacts stay
  untracked.
- **Examples** (`clients/python/examples/`, the deferred split from the
  2026-06-18 decision): installed-package examples with the `sys.path` shim
  dropped — `offline_render.py` (fully self-contained NRT render to WAV, no
  server/device) and `live_udp.py` (the same `Pbind` live over UDP to a running
  server), plus a `README.md`. The repo-root `examples/` stays the broad catalog.
- **Verified**: `python -m build --wheel` produces
  `clausters-0.1.0-py3-none-linux_x86_64.whl` bundling both `.so`s; installed
  into a fresh venv it imports and renders from an unrelated CWD (no `target/`).
  `pip install -e . --group dev` + `pytest` -> 77 passed, 1 skipped.
- **Docs**: `clients/python/README.md` (the install/wheel section), `GUIA.md`
  (C12 row), `docs/clients.md` and `docs/using-as-a-library.md` (packaging).

## M20 — Documentation: dual mdBook (server + Python client) + pydoc-markdown API (2026-06-21)

Unified the documentation **technically by content** while keeping **two
separate books, one per platform**, both Markdown and ReadTheDocs-deployable.

- **Python client book** (`clients/python/docs/`): a second mdBook with its own
  `book.toml`/`src/` and the same theme as the server book. Hand-written
  chapters (`introduction`, `getting-started`, `guide`, `examples`) distilled
  from `README.md`/`docs/clients.md` (not from `GUIA.md`, which is a personal
  file kept out of the docs). `build.sh` builds it.
- **Python API reference from docstrings, no Sphinx**: `pydoc-markdown.yml`
  generates `src/api.md` from the public modules' docstrings via a **static AST
  parse** (no import, so no cdylib needed). `src/api.md` and `book/` are
  git-ignored.
- **Docstrings cleaned to plain Markdown**: converted ~200 RST cross-reference
  roles (`:mod:`/`:class:`/`:meth:`/`:func:`/`:attr:`/`:data:`, including the
  `~`-last-component form) to backtick code spans across all 32 modules; double
  backtick RST literals left as-is (valid Markdown, render as `<code>`).
- **Milestone labels removed from every published doc and docstring**
  (`Mx`/`Cx`/`Fx`): all of `docs/*.md` (in `architecture.md` the labels were
  subsystem nicknames — "the M12 tree mirror" -> "the tree mirror", etc.),
  `clients/python/README.md`, and the private module docstrings. They remain
  only in `PLAN.md`/`LOG.md`. `docs/clients.md` was rewritten as the
  cross-language **map** that links the Python book instead of duplicating its
  layer-by-layer detail.
- **ReadTheDocs**: two `build.commands`-driven configs — repo-root
  `.readthedocs.yaml` (server book) and `clients/python/.readthedocs.yaml`
  (Python book) — each fetches a prebuilt mdBook and copies HTML into
  `$READTHEDOCS_OUTPUT/html`; the Python one also runs pydoc-markdown. Canonical
  slugs `clausters` / `clausters-python`; cross-links use those RTD URLs. The
  two RTD projects must still be created on readthedocs.org (each pointing at its
  config-file path).
- **Verified**: both books build with no errors/warnings; `python -m compileall`
  of the package is clean; the generated `api.md` has zero RST roles and zero
  milestone labels; the one cross-book anchor (`examples.md` ->
  `schemas.md#midi-standalone-bindings--boot-preset`) still resolves after the
  heading lost its `(M19)` suffix.

## Faust soundfile: offline NRT fix + idiomatic Python example (2026-06-22)

Follow-up to the soundfile bridge (2026-06-20). Two parts.

- **Idiomatic Python example** (`examples/faust_soundfile.py`): a live RT demo
  that loads a generated motif into a server buffer (`/b_allocRead` via the
  client's buffer allocator) and loops it from inside a `FaustDef` reading
  `soundfile("<bufnum>", 1)` -- built with `FaustDef.from_source`, since the
  signal-tree builder (`clausters.defs.signals`) has no `soundfile` op. Sweeps
  `gain`/`speed` with `/n_set` and demonstrates the snapshot-at-`/s_new`
  semantics by reloading the buffer mid-play and spawning a second voice. Docs in
  `clients/python/GUIA.md` (live-examples section) and a cross-ref in the root
  `GUIA.md` F6 note.
- **NRT soundfile fix** (`server::render`): how Faust stores a soundfile is
  host-filled, not compiled in -- the DSP holds a `Soundfile*` and the host fills
  `fBuffers`/`fLength`/... at instantiation (canonically a `SoundfileReader`
  reading files; here, our `add_soundfile` reads a clausters server buffer named
  by the integer label). So both RT and NRT must hand `make_synth` the server
  buffer pool. The offline renderer kept its own `Renderer::buffers` separate
  from `CmdTranslator::buffers` (the pool `make_synth` fills the zone from), so an
  offline soundfile got the empty placeholder (length 1024) and rendered silent,
  while the live server (which updates `translator.buffers`) read it. Fix: drop
  the redundant `Renderer::buffers` field and route every `/b_*` install and
  lookup through `translator.buffers`, the single source of truth -- the NRT path
  now uses the same buffer infrastructure as RT. **Test**: `tests/golden.rs`
  (`soundfile_reads_a_score_buffer_in_nrt`): a score `/b_alloc`s a 300-frame
  buffer and renders a def whose output is the soundfile length -- 300 when wired,
  1024 (the placeholder) when not. Verified end to end: an offline render of the
  example's def went from peak 0.0 to a correct read.

## C13 — Responders (OscFunc/MidiFunc): the client's input path (2026-06-23)

The Python client was output-only — it built OSC/MIDI and sent it. C13 adds the
**receive** path and the client's role as an OSC/MIDI hub, mirroring sclang's
`OSCFunc`/`MIDIFunc`: receive from any app, match/dispatch to a callback, and let
that callback emit onward (to the Clausters server or elsewhere). It splits along
the existing server-agnostic vs server-specific seam.

- **MIDI input transport** (`crates/clausters-midi`, ABI v1 -> **v2**): the
  `live` feature was output-only; added a virtual MIDI **input** port
  (`clausters_midi_input_open`/`_poll`/`_close`, ALSA seq via midir, mirroring
  the server's `src/midi/live.rs`). `midir` runs the input callback on its own
  thread and pushes raw messages into an `mpsc` channel the host **drains by
  polling** — no callback crosses the C boundary, keeping the flat-data
  contract. Python `ctypes` side in `clausters/_midi.py` (`input_open` /
  `input_poll` -> `bytes | None` / `input_close`), `MIDI_ABI_VERSION` bumped.
- **OSC receive** (client-side, stdlib): `clausters/base/_osclib.decode_packet`
  (bundle-aware, the recv counterpart of the server's single decode door) and
  `clausters/base/_oscinterface.OscReceiver` (binds a UDP socket, a demux thread
  decodes each datagram and calls every registered handler `(addr, args, time,
  src)`; a `send` method so it is a bidirectional endpoint — a responder can
  reply, and a client can register `/notify` from this socket so server pushes
  return here). MIDI counterpart `MidiReceiver` + `parse_midi` (raw channel-voice
  bytes -> a `{type, channel, …}` dict, mido/sc3-style) in
  `clausters/base/_midiinterface.py`.
- **Dispatch layer** (`clausters/responders.py`): `OscFunc(func, path, *, src,
  arg_template, recv)` and `MidiFunc(func, midi_msg, *, chan, arg_template,
  recv)`, plus `oscfunc`/`midifunc` decorators. Each registers a self-filtering
  handler with its receiver; `one_shot()`, `enable`/`disable`/`free`. Lazily
  created module-default receivers (`default_osc_receiver`/`_midi_receiver`) are
  the one bit of process-wide state, opt-in like `main.default_clock`; explicit
  receivers always available. **Threading discipline**: callbacks run on the
  receiver thread (or, with a `clock`, via `clock.sched`); the golden rule holds
  (never block) — to *sequence* in response, `clock.play(Routine(...))`.
- **Server-specific convenience**: the responder callbacks turn incoming
  notes/messages into `/s_new` etc. on the `Server`, reusing the existing
  `Server`/`Event` machinery — the client-side mirror of the server's own direct
  MIDI path (M17/M19): the server can be played by MIDI/OSC it receives, or by a
  client that listens and forwards. Both coexist.
- **`/transport` push-on-change** (server, the M22 deferred half, decided with
  the user): setting `/transport` now **pushes** the new grid as a
  `/transport.reply` to every `/notify` client (reusing the existing `clients`
  notify list), so a responder on `/transport.reply` re-`join_transport`s live
  when a conductor changes tempo/origin — no polling. (Transport model kept
  **single global**, M22 as-is, per the user; named/multiple transports were
  considered and deferred.)
- **Tests**: `clients/python/tests/test_responders.py` (12) — OSC end to end over
  a loopback UDP socket (address/arg-template match, bundle unwrap with time,
  one-shot, disable/enable, decorator) and MIDI parsing + `MidiFunc` match
  against injected messages (the real ALSA port is the manual E2E). Server:
  `tests/osc.rs::transport_pushes_on_change_to_notify_clients`.
- **Examples** (`clients/python/examples/`): `osc_responder.py` (the OSC hub —
  relay `/note` to the server, react to a `/transport.reply` push; self-feeds to
  demonstrate, verified live E2E) and `midi_responder.py` (a `MidiFunc` turning a
  MIDI keyboard into server synths; manual, needs the `live` cdylib + a wired
  source).
- **Docs**: new Python-book page `responders.md` (receivers, matching, the golden
  rule, the transport reaction) in `SUMMARY.md`; `examples.md` + the examples
  `README.md` cataloged; `clausters.responders` added to the pydoc-markdown API
  config; `clients/python/GUIA.md` section 9 + checklist row. Also a dedicated
  DAW-style transport guide `transport.md` (conductor/follower, `quant` bars,
  beat-vs-sample alignment, the live tempo-change reaction, what it is/isn't vs a
  DAW), cross-linked from `timing-models.md`.
- **Verified**: full client suite 91 passed / 4 skipped (12 new in
  `test_responders.py`); `cargo test --test osc` green (25, incl. the new push
  test); live E2E of `osc_responder.py` against a running server printed the
  relayed notes and the `transport changed -> re-aligning` reaction. `cargo fmt
  --check` clean, core builds without features.

## C16 — Static timelines + a playhead (random-access sequencing) (2026-06-23)

The generative layer (`Routine`/`Pbind`) is forward-only — its state lives in the
generator's locals, so it cannot be *seeked*. C16 adds the static counterpart: a
materialized, editable, random-access-by-time structure, which is what makes
DAW-style transport controls (play/stop/locate/loop + a song position) possible.
A playhead scans it forward as the clock advances; the random access lives only
at the boundaries (play/seek/loop wrap). Designed with the user (AskUserQuestion,
2026-06-23): **client-only** playhead first (a server-broadcast transport layers
on later), and items are **realizable** (high-level `Event`s plus raw OSC/MIDI).

- **`clausters/seq/timeline.py`**:
  - `Timeline` — a sorted list of `(beat, item)` (stable `bisect.insort`), edited
    with `add` (returns a handle) / `remove` / `move` / `clear` and read by time
    with `index_at` (the seek primitive) / `range` (`[t0, t1)`) / `at` /
    `duration`. `from_pattern(pattern, dur)` captures a `Pbind` offline into a
    timeline ("bounce to a clip"), recording each event at its logical beat.
  - `Playhead` — `play(at, quant)` / `stop` / `locate(beat)` / `loop(start, end)`
    / `unloop` / `position`, over a timeline on a clock + destination. It is a
    cursor walk fed to the clock as a `Routine`: `index_at` re-seeks at the
    boundaries, the body yields the gaps between items. Rides the clock's logical
    time, so it inherits `quant` / `lock_to` / `join_transport` unchanged.
    `position()` interpolates from the clock while playing. An epoch counter +
    `clock.unsched` make `stop`/`locate` cancel the in-flight feeder cleanly.
  - `OscEvent(addr, *args)` / `MidiEvent(message)` — raw OSC/MIDI items (a
    timeline can be a plain editable OSC/MIDI score), realized via
    `Server.send_bundle` / the new `MidiServer.send_message`.
- **Supporting additions**: `TempoClock.unsched(item)` (remove one scheduled
  routine by identity, heapify the rest — cancel without `clear`'s scorched
  earth); `MidiServer.send_message` (emit a raw message at the logical beat, the
  MIDI counterpart of `send_bundle`). Exports from `clausters.seq`.
- **Tests**: `clients/python/tests/test_timeline.py` (10) — Timeline edits and
  random access (pure), and the Playhead driven **offline** into a recording
  destination so play order, `locate` (skip + offset), `loop` wrap and raw-item
  realization are asserted by the logical beats each item lands on; `stop` checked
  at the queue level (it unscheds the feeder); `from_pattern`.
- **Example**: `clients/python/examples/timeline_transport.py` — captures a
  pattern, edits the timeline, then drives it live (`play` -> `locate` -> `loop`
  -> `stop`), printing the song position. Live E2E against a running server: exit
  0, position interpolation read 2.40 beats at ~1.2 s / 2 bps.
- **Docs**: new Python-book page `timelines.md` (the static-vs-generative split,
  the timeline, the playhead, capture, offline render, the relation to the shared
  transport) in `SUMMARY.md`; `transport.md` "what it is not" reconciled (the
  client now has a local playhead; only the *server* transport lacks play/stop);
  `examples.md` + examples `README.md` cataloged; `clausters.seq.timeline` added
  to the pydoc-markdown config; `GUIA.md` section 10 + checklist row.
- **Verified**: full client suite 101 passed / 4 skipped (10 new); live E2E clean.

## C16 follow-up — server-broadcast transport (conductor play/stop/locate) (2026-06-23)

The deferred layer above C16's client-only playhead, decided with the user: a
conductor's play/stop/locate driving every client's playhead in **lockstep**.
This also closes M22's deferred "running/stopped state." The server broadcasts
transport *control* — it never schedules audio; each client rolls its own
playhead on the shared grid.

- **Server** (`src/osc/server.rs`): `Transport` gained `playing: bool` + a
  `position: f64` (the song-position beat). New commands `/transport_play
  [position:double]` (start rolling, from `position` or where it stopped),
  `/transport_stop`, `/transport_locate <position:double>` — each needs a grid
  defined, replies `/done`, and is **broadcast** to `/notify` clients (the C13
  push, factored into `broadcast_transport`). `/transport.reply` extended to
  `(origin_sample:int64, tempo:double, defined:int32, playing:int32,
  position:double)` — backward-compatible (older clients read the first three).
  Setting the grid resets to stopped at 0. Tests: `tests/osc.rs::transport_play_
  stop_locate` (play/stop/locate update the state, fail before a grid exists) and
  the push test extended to assert a play also pushes.
- **Client**: `Server.transport_play` / `transport_stop` / `transport_locate` /
  `transport_state` (`defs/server.py`). `Playhead.follow_transport(server, recv,
  quant)` (+ `unfollow_transport`) in `seq/timeline.py`: registers `/notify` and
  an `OscFunc` on `/transport.reply` (the C13 responder layer) that rolls / halts
  / seeks the local playhead to match the broadcast, then applies the current
  state once. Every follower computes from the *same* broadcast, so they are
  beat-aligned (sample-exact when each clock is also `lock_to` the server). The
  design is symmetric — every client follows; whoever issues the commands is the
  conductor.
- **Tests**: `tests/test_timeline.py::test_playhead_follows_transport_broadcast`
  (feeds a simulated `/transport.reply` over loopback, no live server — the
  playhead rolls on play, halts + locates on stop).
- **Example**: `clients/python/examples/transport_conductor.py` — two independent
  followers (`lock_to` + `join_transport` + `follow_transport`) roll together
  when a conductor `transport_play`s. Live E2E: positions matched
  (1.04/1.04, 2.45/2.43, 3.86/3.83 — the deltas are just the two sequential
  `position()` reads, both sample-locked to one grid), exit 0.
- **Docs**: `transport.md` gained a "Rolling the transport: a conductor" section
  and its "what it is not" / cheat-sheet reconciled (the server now broadcasts a
  rolling state but still never schedules audio); `timelines.md` "Following a
  conductor"; `examples.md` + examples `README.md`; server `docs/schemas.md` and
  `docs/sample-clock.md` (the new commands + extended reply). `clients/PLAN.md`
  C16 deferred-note marked done; root `PLAN.md` M22 note updated.
- **Verified**: `cargo test --test osc` 26 green; full client suite 102 passed /
  4 skipped (1 new); both books build; `cargo fmt --check` clean.

## C17 — Embedded server as a first-class destination + one self-contained wheel (2026-06-23)

Until now the in-process embedded server (`clausters.Clausters`) was reachable
only at the low level — raw OSC bytes through `send`/`poll` — while the ergonomic
layer (`Server`/`Session`/`Pbind`/defs) spoke only UDP, TCP or an NRT score. C17
makes the embedded server **just another destination**: the same routines,
patterns and defs drive it unchanged. The seam is the `Server`'s communication
interface, so the addition is one new interface plus a session factory; no client
behaviour above the interface changed. Decided with the user (2026-06-23): the
embedded server is a first-class variant handled identically to the others, and
the standalone server binary ships **in the same wheel** (one artifact, no
optional extras — pip extras add dependencies, they cannot gate files inside a
wheel).

- **`OscEmbedInterface`** (`clausters/base/_oscinterface.py`): encodes exactly
  like `OscUdpInterface` — same wire bytes, same NTP-timetagged bundles — but
  delivers each packet to an in-process `clausters.ipc.Clausters` by function
  call and reads replies by polling it. The embedded server decodes through the
  same command path as the networked one and (running in this process) shares the
  wall clock the timetags are written against, so the timing semantics match UDP
  exactly. `target` is ignored (like `OscTcpInterface`). Opens and owns a fresh
  `Clausters` by default, or wraps an existing handle (then it does not close
  it). Exported from `clausters.base`.
- **`Session.embed(...)`** (`clausters/session.py`): the real-time factory whose
  server runs in-process, twin of `nrt`/`live` — same `latency`/`timebase`, plus
  `workers` and an optional `server=` to reuse a handle. `session.server.interface.server`
  is the live `Clausters` handle (direct sample-clock / control-bus reads, no OSC
  round trip).
- **One self-contained wheel** (`build_native.py`, `setup.py`, `pyproject.toml`,
  new `clausters/_cli.py`, `_libpath.py`): `build_native.py` now also builds
  (`cargo build --release --bin clausters`, default features) and stages the
  standalone binary into `clausters/_bin/`. It travels as **package data**;
  the `clausters` **console-script** (`clausters._cli:main`) locates and execs
  it, so `pip install` puts the standalone server on the PATH with the executable
  bit set. (The wheel's `scripts=` slot does not work for a native binary:
  setuptools' `build_scripts` parses every script as Python source via
  `tokenize.open` and chokes on the ELF's null bytes — documented in `setup.py`.)
  `_libpath.bundled_bin_candidates` gives `_cli` the same lookup precedence the
  cdylib loaders use (`CLAUSTERS_BIN` env → bundled `_bin/` → workspace
  `target/`). One `pip install clausters-…whl` now yields: the client library,
  the in-process embedded server (`Clausters` / `Session.embed`), and the
  standalone server (`clausters` command).
- **Tests**: `clients/python/tests/test_session.py` (+2) — `Session.embed` drives
  the in-process server with the same API (request/reply over embed, the engine
  advances on play) and coexists with an NRT session with no global state. Both
  skip cleanly when no audio device / no `embed,realtime` build is available.
- **Example**: `clients/python/examples/embedded.py` — the third session flavour
  next to `offline_render.py` and `live_udp.py`; `Session.embed` plays the shared
  phrase from an in-process server. Verified: exit 0, embedded server at 48000 Hz.
- **Docs**: Python book `sessions.md` (now "Three kinds of session" + a
  when-to-use table and an `embed()` section), `getting-started.md` (the wheel
  bundles the standalone binary as the `clausters` command; three play-a-sound
  paths), `examples.md` + examples `README.md` (the new example); `GUIA.md`
  section 11 (embedded session + the all-in-one wheel, with manual E2E steps) and
  section 8 updated.
- **Verified** (from a clean venv, neutral cwd): wheel 4.18 MB carries the binary
  as package data, mode 0o775; installed `clausters --help` runs the bundled
  server; `Session.embed` plays in-process; standalone server + `Session.live`
  E2E over UDP (status / play / query_tree) clean; full client suite 104 passed /
  4 skipped (2 new). No Rust changed (so no `cargo fmt`).

## WebSocket transport for the OSC server (`--ws`) (2026-06-25)

A fourth carrier of the same OSC encoding beside UDP, TCP and the shared-memory
ring — the one a **browser** can reach (a browser cannot open a raw UDP socket
or map shared memory, but speaks WebSocket natively), so it is what lets a
web-hosted client drive the server and the server "run in the browser". Decided
with the user (2026-06-25) while planning a scriptable GUI peer: "JSON vs OSC"
for that peer is a false dichotomy — OSC stays the single encoding (structured
payloads already ride as JSON inside an OSC arg, as `/d_recv` does), and
WebSocket is just one more transport through the same decode door, not a new
protocol — and, like UDP/TCP/shm, a first-class one, so it is **always built**,
not behind a feature (decided with the user 2026-06-25). Landed on `main` as a
generic server feature, independent of the GUI work.

- **`src/osc/ws.rs` (`WsHub`)**: mirrors `osc::tcp` — an acceptor thread plus one
  thread per connection turn the socket into whole OSC packets handed to the
  single-threaded command loop over an `mpsc` channel, and a zero-length UDP
  datagram to the server's own address wakes the loop the instant a frame or a
  disconnect is queued. The one structural difference: a `tungstenite`
  `WebSocket` owns its stream (read/write are not split like a `TcpStream`), so
  instead of the loop owning a write half, each connection thread drains a
  per-connection reply channel and writes the bytes itself, polling with a 5 ms
  read timeout to interleave reads with queued replies (the same bounded-latency
  trade-off the IPC ring documents, here for the reply leg).
- **Framing**: each WebSocket **binary** message carries exactly one OSC packet,
  so — unlike TCP — there is no length prefix; the frame boundary *is* the packet
  boundary, and replies go back as binary messages. Every inbound packet
  validates through the single `osc::decode_packet` door; `tungstenite` enforces
  its own max message size (the DoS ceiling TCP gets from `MAX_FRAME`).
- **Routing**: a new `ClientId::Ws(u64)` variant (kept in the enum
  unconditionally so reply routing stays a total match) carries replies back to
  the originating connection.
- **Always built** (not feature-gated): `tungstenite` 0.21 is a base dependency,
  synchronous, no async runtime, no TLS (we serve `ws://`, not `wss://`) — the
  same first-class status UDP/TCP/shm have. `--ws` always works.
- **CLI**: `clausters --ws [port]` (default `57120`, away from `--tcp`'s `57110`
  since both bind a TCP listener), wired through `OscServer::listen_ws`.
- **Client transport in the shared core** (`crates/clausters-ffi`, ABI bumped to
  v2): a WebSocket **client** C ABI — `clausters_ws_connect`/`_send`/`_recv`/
  `_close`/`_last_error`, an opaque connection handle — reusing the **same**
  `tungstenite` the server uses, so the protocol has one implementation. Decided
  with the user (2026-06-25): rather than a second WebSocket implementation in
  Python, the client takes WS from Rust the way it already takes shm/embed — the
  project's "transport in Rust, thin ctypes binding" pattern. `tungstenite` is a
  non-optional dependency of the ffi cdylib, so any binding can reach a `--ws`
  server. Only flat data crosses (byte buffers, integers, an error string).
- **Python client**: `_native.WsClient` binds those calls; `OscWsInterface`
  (`clients/python/clausters/base/_oscinterface.py`) is now a thin wrapper over
  it — no hand-rolled handshake/framing — a drop-in beside `OscUdpInterface`/
  `OscTcpInterface`, exported from `clausters.base`. The same `Server`/`Session`
  facade runs over WebSocket. (The browser leg is unchanged: it uses the native
  `WebSocket`, so it needs none of this.)
- **Tests**: `src/osc/ws.rs` unit test (one OSC packet per binary message
  round-trips through the hub and `decode_packet`, a reply routes back) and
  `clausters-ffi` `ws::tests` (connect/send/recv/close through a real WebSocket,
  embedded NULs and all, against an inline echo server) — both in-process.
- **Examples**: `examples/ws_ping.py` (the `Server` facade over WebSocket, twin
  of `tcp_client.py`) and `examples/ws_ping.html` (the same `/status` round trip
  from a browser, native `WebSocket`, zero deps).
- **Docs**: `docs/schemas.md` (the WebSocket transport alongside TCP, with the
  browser rationale and the no-length-prefix framing); Python `guide.md`
  (`OscWsInterface` in the interface list).
- **Verified**: `cargo build` green; `cargo test --lib osc::ws` and `cargo test
  -p clausters-ffi` pass; `cargo fmt --check` clean; clippy adds no new warnings;
  E2E (`clausters --ws` + the ffi-backed `examples/ws_ping.py` in one shell)
  round-trips `/status`, `/d_recv`→`/done`, `/s_new`, `/sync`→`/synced`,
  `/n_free`.

## G2 — GUI host skeleton (`clausters-gui`) (2026-06-25)

The first milestone of the GUI track's protocol/host work (the heavy-rendering
prototype was already in place). `clausters-gui` grows a headless **GUI host**:
the dual-role process from the design — a *GUI server* for the language clients
(it speaks the `/gui_*` widget protocol) and a *client of the audio server* — but
with a widget command interpreter where the audio engine would be, and no GPU
yet (G3 brings the first pixels). The whole point is to validate the protocol and
the topology against a real client before any windowing lands.

- **Transport decision (the milestone asked to record one):** the host does
  **not** extract or link the audio server's transport layer
  (`src/osc/{server,tcp,ws}.rs`) — it is tangled with the audio `ServerState`,
  the engine wake and the IPC ring, so lifting it now would drag server concerns
  into the independent gui crate for no gain. Instead the host **links
  `clausters-core`** (a path dependency that pulls only `rosc`, never the server
  crate, so the gui crate stays independent of the core build) for the shared OSC
  seam, and owns a **thin transport front** of its own. G2 ships the **UDP**
  front (the default Clausters carrier, minimal to drive from a Python client);
  TCP/WebSocket/ring follow in later milestones behind the same `ClientId`/reply
  seam, which is shaped to generalize.
- **One decode door for the whole system:** `clausters_core::osc::decode_packet`
  is the new single decode entry point, and the server's `osc::decode_packet`
  now delegates to it — so the audio server and every client (the gui host
  included) validate incoming bytes through one function, honoring the
  project-wide "single door" rule across processes, not just inside the server.
- **The GuiDef is JSON-in-OSC, like a SynthDef.** `host::guidef::GuiNode` is a
  deliberately **generic** node — `{ id, type, <props…>, children }` — parsed
  with serde so integer ids stay `i64` and continuous values `f64` (the int/float
  distinction the wire relies on). The widget *catalog* grows by adding a
  renderer/handler later, never by changing this shape; the host registers and
  introspects any tree without knowing concrete widget types yet.
- **The widget registry reuses the node-tree shape verbatim** (`host::registry`):
  client-allocated integer ids, a parent/children hierarchy, and **subtree
  freeing** (freeing a widget frees its descendants, like freeing a group).
  `/gui_def` flattens a tree into one record per widget (redefining a root
  replaces the old def; duplicate/idless children are skipped with a warning),
  `/gui_set` mutates props, `/gui_free` removes a subtree, `/gui_query` reads one
  back.
- **The command loop** (`host::Host`, transport-agnostic and unit-testable —
  `handle_packet` mutates state and *returns* replies) interprets
  `/gui_def`/`/gui_set`/`/gui_free`/`/gui_query`, **logs the parsed tree**, and
  answers `/gui_query` with `/gui_info <id> <type> <k> <v>…` (an empty type means
  "no such widget" — it still answers, like the server on a miss). `/gui_bind`
  and `/gui_load` are reserved (log "not implemented yet"). Bundles are unwrapped
  immediately (no scheduling yet). The client leg (`host::client::ServerLeg`) is
  scaffolded — a UDP sender through the same encode door — and attached with
  `--server host:port`; bindings (G6) build on it.
- **Binary:** `clausters-gui` (`--port`, default 57210, clear of the audio
  server's 57110/57120 family; `--server`; `-v`/`-q`; `RUST_LOG`).
- **Python driver** (`clients/python/clausters/gui/`): `guidef` composes the
  tree as plain dicts (host-agnostic, the way building a SynthDef is
  server-agnostic) with `node`/`window`/`panel`/`label`/`knob`/`slider`/
  `waveform`; `GuiHost` points the existing `OscUdpInterface` at the host's port
  and exposes `define`/`set`/`free`/`query` — no parallel wire code.
- **Example:** `examples/gui_skeleton.py` — build a small instrument panel, send
  one `/gui_def`, read a widget back over `/gui_info`.
- **Tests:** 13 host unit tests (parse/dump/registry/dispatch, bundle unwrap,
  int/float preservation), all GPU-free; the existing prototype tests still pass
  (27 total in the gui crate).
- **Verified:** gui crate `cargo test` green, `cargo fmt --check` and
  `cargo clippy --all-targets -- -D warnings` clean; the core server still builds
  and tests with `--no-default-features`; E2E in one shell (the host plus the
  Python driver) round-trips `/gui_def`→log, `/gui_query`→`/gui_info`,
  `/gui_set`, `/gui_free`, with the int/float distinction intact end to end.

## G3 — GuiDef schema + the first window (waveform pixels) (2026-06-25)

The GUI track's "first pixels": a `window`-rooted GuiDef now instantiates an
actual winit + wgpu window hosting the existing renderers. The headless protocol
of G2 stays; the host gains a windowed front and a typed schema/layout/render
path around the prototype's `WaveformView`.

- **Typed widget schema** (`host::widget`) — the renderer's *interpretation* of
  the generic `host::guidef::GuiNode`, not a second protocol: `WidgetKind` for
  `window`/`panel`/`label`/`waveform`, plus `Unknown(tag)` for any type this
  build does not paint yet (laid out, ignored — so a host renders the parts of a
  newer GuiDef it understands). Adding a widget type is a new variant + handler,
  never a wire change. serde keeps the int/float distinction; `w`/`h`/`layout`/
  `title`/`text`/`base_bucket` are typed fields.
- **Waveform data: inline or blob.** A `waveform` reads its samples from inline
  `"data": [f32…]` or — for bulk — `"blob": <index>` into the OSC blobs carried
  *beside the JSON in the same `/gui_def` message* (raw little-endian `f32`), so
  `/gui_def` is now `id, json, [blob…]`. A `"buffer"` (server buffer) reference
  is recognized but deferred to the milestone where the host attaches to the
  audio server. Datagram-bounded for now (UDP ~64 KB); a bulk/streamed path is a
  later milestone.
- **Layout engine** (`host::layout`) — pure geometry, unit-tested: a container
  splits its area among children by `row`/`col`/`grid`/`free` into device-pixel
  `Rect`s (top-left origin, what `set_viewport` wants), evenly sized with a small
  margin/gap (editor-grade per-widget sizing is future work).
- **Windowed front** (`host::gui`) — winit owns the main thread; the OSC
  transport runs on a **background thread** and forwards each datagram to it
  through an `EventLoopProxy` (window creation must happen on the main thread),
  so all host state stays single-threaded and lock-free. Multi-window, keyed by
  def id: a `window` root opens an OS window (rebuilt on re-`/gui_def`, closed by
  `/gui_free` or the window's close button/`Esc`); the host keeps running with
  zero windows. Replies go back out the shared `Arc<UdpSocket>` to the requester.
- **Rendering** — each `waveform` renders into its laid-out rectangle's viewport
  via the existing, verified `WaveformView`/`WaveformRenderer` (the three-regime,
  resolution-matched path), navigable with the prototype's bindings (wheel zooms
  toward the cursor, left-drag pans, `R` resets), routed to the waveform under
  the pointer. Panels/labels paint as flat chrome rectangles through a tiny
  `host::rects` pipeline (modeled on the waveform's column pipeline); **glyph
  text for labels is deferred** to the control-widget milestone. `native::Gpu`
  was made `pub(crate)` and reused so the surface/device setup lives once.
- **Effects model.** `Host::handle_packet` now returns `Vec<HostEffect>`
  (`Reply`/`OpenWindow`/`CloseWindow`) instead of bare replies, so the protocol
  logic stays transport- and GPU-agnostic and unit-testable: the windowed front
  opens/closes windows and sends replies; the headless front sends replies and
  logs the window effects.
- **Binary.** `clausters-gui` now opens windows by default; `--headless` runs
  the protocol with no display (tests, automation, no-GPU machines). A missing
  display fails with a clear "use --headless" message.
- **Python.** `clausters.gui.waveform(data=…/blob=…)`, `samples_to_blob` (LE
  `f32` packing), `GuiHost.define(id, tree, *blobs)`; `examples/gui_window.py`
  opens a real window showing a decaying sine fed as a blob (`gui_skeleton.py`
  stays the headless protocol example, now run with `--headless`).
- **Tests:** 38 in the gui crate (widget schema parse incl. blob/unknown/errors,
  layout row/col/grid/nesting, host effects + window def storage + waveform-blob
  through the def message), all GPU-free; plus runtime verification on this
  machine — the windowed host opens a window and renders the waveform with no
  panic or wgpu validation error.
- **Verified:** gui `cargo test` green, `cargo fmt --check` and
  `cargo clippy --all-targets -- -D warnings` clean; the core server is
  untouched and still builds/tests with `--no-default-features`; E2E in one shell
  both ways — `--headless` round-trips `/gui_def`→log, `/gui_query`→`/gui_info`,
  `/gui_set`, `/gui_free`; windowed opens a waveform window from one `/gui_def`.

## G4 — Standard control widgets + live `/gui_set` + events (2026-06-25)

The GUI host gains the essentials of any GUI: the standard control widgets, the
live-update path, and the host->script event path. Built on G3's window/layout/
render foundation; the glyph text deferred in G3 lands here as a small embedded
bitmap font, so labels and values are legible.

- **Control widgets** (`host::widget` typed kinds + `host::controls` rendering/
  hit-math): `slider`, `knob`, `number` (a draggable read-out) over a `Range`
  (value/min/max/label); `button` (momentary), `toggle` (boolean), `menu`
  (click-cycles its options), and `text` (shows its value, script-driven). All
  parse from the generic GuiNode, keep the int/float distinction, and an
  unrecognized type is still `Unknown` (laid out, not painted) — the protocol
  never changed.
- **One drawing primitive.** The G3 rect renderer became `host::paint` — a
  `Mesh` of flat-colored triangles (rect/quad/line/disc) and a one-pipeline
  `Painter` — so knobs (a disc + a swept pointer) and glyphs need no new GPU
  code. `host::font` is a compact embedded **5x7 bitmap font** drawn as one quad
  per lit pixel into that mesh (no texture, no second pipeline); it renders
  labels and numeric values (uppercase, with a box fallback). A control is thus
  composed from the painter's primitives + text, never bespoke pipelines.
- **Single source of truth.** The typed window tree now lives only in the
  `Host` (`window_def`/`window_def_mut`); the windowed front renders and
  hit-tests from it and writes interaction results back into it, so a live
  `/gui_set` and a user drag update the same tree and the next frame reflects
  both. `/gui_set` updates the generic registry (for `/gui_query`) and, via
  `Registry::root_of`, the typed widget in its window, emitting a new
  `HostEffect::Redraw`.
- **Interaction -> events.** The front hit-tests the widget under the cursor and
  routes the gesture: a slider follows the cursor x, a knob/number a vertical
  drag (`host::controls::{slider_fraction, drag_fraction_delta}`, unit-tested),
  a toggle flips, a menu cycles, a button is momentary. Each change writes the
  value back into the host tree and emits `/gui_event <id> <value>` to the script
  that built the window (its address is captured at `/gui_def`); a `button`
  reports `1` on press and `0` on release. Closing a window (button or `Esc`)
  emits `/gui_closed <id>`. The waveform's zoom/pan/reset emit
  `/gui_event <id> "view" start len`, wiring the `TimelineView` interactions out.
- **Binary/Python.** Unchanged CLI (`--headless` still runs the protocol with no
  display). Python `clausters.gui` gains `number`/`button`/`toggle`/`text`/`menu`
  builders and `GuiHost.poll`/`listen` for the event path (the receive side of
  the responder model); `examples/gui_panel.py` is a scripted instrument panel
  that drives a widget with `/gui_set` and prints the `/gui_event`s/`/gui_closed`
  your interactions emit.
- **Tests:** 49 in the gui crate (control parse/clamp, `apply`+`event_value`,
  slider/knob value math, font pixel-quad emission, layout, host effects), all
  GPU-free; runtime-verified on this machine — the panel window opens and renders
  every control with no panic or wgpu validation error, a live `/gui_set` moves a
  knob, and a real knob drag round-trips a `/gui_event` to the script.
- **Verified:** gui `cargo test` green, `cargo fmt --check` and
  `cargo clippy --all-targets -- -D warnings` clean; the core server is untouched
  and still builds/tests with `--no-default-features`.

## G5 — GUI as a client of the audio server + shared-memory meters/scopes (2026-06-26)

The third leg of the topology lands: the GUI host attaches to `clausters-server`
as a client, meters/scopes read a control bus straight from shared memory (zero
messages), and a `waveform` can name a server buffer instead of carrying its own
samples.

- **Standard buffer reads on the server.** `/b_get` (reply `/b_set`) and `/b_getn`
  (reply `/b_setn`) — the scsynth client reads, absent only because no client had
  needed them. Synchronous, answered from the network-side buffer mirror exactly
  like `/b_query`, so RT-safety is untouched; `count` clamps to what the buffer
  holds (a request past the end returns the available samples; an unallocated
  buffer returns count 0). Indices are flat/interleaved. Benefits every client,
  not just the GUI. Doc in `docs/schemas.md`; round-trip test in `tests/osc.rs`.
- **Shared-memory reader (`clients/gui` `host::shm`).** A read-only `mmap` of the
  server's `--shm` segment that mirrors its versioned `#[repr(C)]` ABI and
  rejects a magic/`ABI_VERSION` mismatch on attach. The recorded reuse decision:
  the GUI crate must stay independent of the **server** crate (linking it would
  pull the engine, cpal and Faust), so it plays the same role against this
  versioned binary boundary that any independent peer does (the Python `ctypes`
  client, a future JS one) rather than reimplementing or importing `server::ipc`.
  Reading a control bus is one atomic load of the very word the engine uses.
  Unix-only, as the server's segment is; a small `BusSource` trait keeps the
  windowed front free of platform `cfg`s. Tested against a fabricated segment.
- **Meter and scope widgets (`host::widget` + `host::meters`).** New `WidgetKind`s
  carrying a control-bus index and a range; drawn with the existing flat painter
  (a bar that fills to the bus value; a rolling polyline of the bus's recent
  history kept per window). No new pipeline, no analysis. The windowed front reads
  the bus from `host::shm` each frame and animates any window holding a live
  widget at ~30 fps via `ControlFlow::WaitUntil`; idle windows stay event-driven.
  Pure drawing/`fraction` math is unit-tested without a GPU.
- **Server-buffer waveform.** `WidgetKind::Waveform` gained an optional `buffer`
  number; the windowed front fetches it over the now-bidirectional client leg
  (`ServerLeg` over a shared `Arc<UdpSocket>`): a second thread drains the leg and
  routes `/b_info`/`/b_setn` into a buffer-fetch state machine (`/b_query` then
  chunked `/b_getn`, de-interleaved to channel 0), building a `WaveformView` when
  the samples arrive. The bulk-transfer optimization for very large buffers is
  G7.
- **Binary/Python.** `clausters-gui --shm <path>` maps the segment for meters
  (Unix; no effect headless). Python `clausters.gui` gains `meter`/`scope`
  builders and `waveform(buffer=)`; `examples/gui_meters.py` runs the audio
  server, the host and the script together — a moving meter/scope on a control
  bus and a waveform of a sine buffer pulled from the server.
- **Tests:** 56 in the gui crate (shm reader round-trip + bad magic/ABI, meter/
  scope parse+apply+`live_bus`, `meters::fraction`/draw geometry, waveform buffer
  ref), all GPU-free; `/b_getn`/`/b_get` round trip in `tests/osc.rs`.
- **Verified:** runtime end-to-end against the real server — the host maps the
  live `--shm` segment (1024 buses), opens the window, and loads a 24000-frame
  buffer over the leg with no panic or wgpu validation error. gui `cargo test`
  green, `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`
  clean; the core still builds/tests with `--no-default-features`.

## G6 — Bindings (`/gui_bind`): bypass the script (2026-06-26)

A widget can now be *bound* so its value flows **straight to the audio server**,
with no round-trip through the script — the low-latency interactive path the
topology promised (and the same idea as the server's MIDI bindings, a control
source wired to a server-side destination instead of being polled).

- **The binding (`clients/gui` `host::bind`).** A `Binding` is an OSC `addr` plus
  a fixed `prefix` of arguments, parsed from a `/gui_bind <id> "server" <addr>
  <prefix…>` target. The leading `"server"` destination keyword is deliberate: it
  is kept in the wire form so the message shape can grow later (binding to another
  widget, or back to the script with a transform) without a protocol change, even
  though only the audio server is meaningful now. `Binding::message(value)` builds
  `addr prefix… value`, keeping the int/float distinction of the prefix verbatim.
  Parse/build are unit-tested in isolation.
- **Host state and forwarding (`host::mod`).** The `Host` holds a `widget id ->
  Binding` map. `on_bind` registers a binding (and warns when no `--server` leg is
  attached, since the value then has nowhere to go); `/gui_bind <id>` with **no**
  target removes it, restoring the event path; `forward(widget_id, value)` sends
  the binding's message through the existing client leg (`host::client::ServerLeg`,
  the same `clausters_core::osc` encode door — one encoder, not a parallel one) and
  returns whether it handled the value. A bound widget with no server still returns
  `true` (the value is swallowed, never leaked to the script). Bindings are pruned
  when their widget is freed or redefined away (`/gui_free`, a replacing
  `/gui_def`), so a stale id cannot keep forwarding.
- **One delivery seam in the windowed front (`host::gui`).** Every value-bearing
  interaction (slider/knob/number drag, toggle, menu, button press and release)
  now routes through a single `deliver`, which calls `Host::forward` first and only
  emits a `/gui_event` when the widget is unbound. So the bound vs. unbound choice
  is made in exactly one place, for all controls; the waveform's structural `view`
  event is unaffected. No new GPU or transport code.
- **Python.** `GuiHost.bind(id, address, *prefix)` sends the `"server"` form, and
  `unbind(id)` removes it; `examples/gui_bind.py` runs the audio server, the host
  and the script together — a knob bound to a sine synth's `freq` drives the pitch
  directly (nothing prints in the script while bound), then unbinds and the same
  knob starts emitting `/gui_event` again.
- **Tests:** 62 in the gui crate (6 new): three for `Binding` (parse/message, its
  rejections, an optional prefix) and three on `Host` (a bind that forwards the
  exact `/n_set 1000 cutoff 440.0` over a real loopback leg and stops after
  unbind; a free-drops-the-binding case; a no-server bind that still swallows the
  value). All GPU-free.
- **Verified:** a headless E2E round-trips bind/unbind from the Python client (the
  host logs `/gui_bind 10 -> audio server /n_set [Int(1000), String("freq")]` then
  `unbound (events restored)`, the int/float distinction kept); the windowed host
  opens a window with a bound knob and registers the binding with no panic. gui
  `cargo test` green, `cargo fmt --check` and `cargo clippy --all-targets -- -D
  warnings` clean; the core still builds/tests with `--no-default-features`
  (unchanged this milestone — no server code touched).

## G7 — Bulk data path + shared DSP (2026-06-26)

Two principles the system already implied, made concrete (both implemented, so
the milestone split into two parts): heavy data moves between Clausters
processes through **local shared resources**, not the wire; and an analysis
algorithm used by more than one process lives **once**, in the shared core.

**Part B — shared analysis algorithms in `clausters-core`.** The "DSP" here is
the GUI's analysis-for-plotting; investigation confirmed the server owns none of
it today (the gui crate was the sole owner, with a handrolled radix-2 FFT in
`spectrogram.rs`).

- **FFT (`clausters_core::fft`).** A forward real FFT (`rfft_magnitudes_into`)
  over `microfft` — `no_std`, zero-allocation, compile-time power-of-two sizes
  (256..4096, the STFT's window sizes), so `process` never allocates, the
  property the server's coming `FFT`/`IFFT` UGens need. The gui
  `spectrogram::Stft` dropped its private `fft()` for this; the magnitude
  convention (bin 0 = `|DC|`, the Nyquist `microfft` packs into the DC bin's
  imaginary part not exposed) matches the old output, so the STFT tests still
  localize a sine to the right bin. `microfft` is forward-only; the inverse (for
  resynthesis UGens) is deferred behind the same API, recorded.
- **Peaks (`clausters_core::peaks`) + `bytes`.** The min/max peak pyramid moved
  out of the gui crate into the core (pure, no GPU): general client
  functionality (any waveform view, any client), not RT, with its
  memory-mappable `CLPK` cache unchanged. A new `cache_size(n, base_bucket)`
  predicts the cache length without building, for sizing an FFI buffer. The gui
  `peaks.rs` is now a re-export; the renderer is untouched.
- **FFI (`clausters-ffi`, ABI v2→v3).** `clausters_core_peaks_cache_size` and
  `clausters_core_peaks_build` let a client build the **byte-identical** cache
  the host maps. Python (`_native.peaks_cache`, `clausters.gui.peaks_cache_file`)
  builds a `.peaks` file over the FFI.

**Part A — bulk transfer via local shared resources (the recorded decision).**
Large payloads — sample buffers, peak caches — move as **memory-mapped files**,
never re-encoded over OSC (a datagram caps near 64 KB; chunking a buffer over
`/b_getn` re-traverses the network asynchronously for data already in local
RAM). The network reads stay the **async fallback** (and the browser's path,
G11); this generalizes G5's zero-message control buses to bulk audio.

- **Mapped waveform sources (`host::mapfile` + `host::gui`).** A `waveform` names
  a local resource: `cache` (a prebuilt pyramid file mapped and used directly —
  raw samples never loaded) or `path` (raw little-endian `f32` mapped and
  de-interleaved by `channels`, pyramid built once and cached as a sibling
  `<path>.<base_bucket>.peaks`). `host::mapfile::MappedFile` is the same
  read-only `libc::mmap` as `host::shm`, over an arbitrary file. `WaveformData`
  now takes its length from the pyramid, so a cache-only view (no raw samples)
  renders the overview.
- **Server RT buffers via the same path (`/b_export`).** `/b_export bufnum path`
  dumps a buffer's raw interleaved `f32` to a local file the host maps — so a
  live server buffer is plotted without `/b_getn`. Synchronous on the network
  thread (not the audio thread), from the buffer mirror, like `/b_get`/`/b_getn`.
  Doc in `docs/schemas.md`; round-trip test in `tests/osc.rs`.
- **Python + example.** `clausters.gui` gains `waveform(path=/cache=/channels=)`,
  `samples_to_file`, `peaks_cache_file`; `examples/gui_bulk.py` shows all three
  forms (a multi-megabyte client sweep from its raw file and its peak cache, plus
  a server buffer exported with `/b_export`).
- **Tests:** core 24 (FFT impulse/cosine, peaks moved, `cache_size` vs
  `to_bytes`), ffi 6 (`peaks_build` byte-identical to the in-process build), gui
  57 (mapfile de-interleave + empty-file reject, `path`/`cache` parse),
  `tests/osc.rs` 28 (`/b_export` round trip + missing-buffer fail).
- **Verified:** runtime end-to-end — the windowed host maps a 500k-sample
  (2 MB) raw `f32` file and its 31 KB peak cache from a `/gui_def` and renders
  both with **no OSC for the samples** (host logs `mapped 500000 samples from
  … (no OSC, no re-send)` and `mapped peak cache … (no raw data, no OSC)`), no
  panic. Python FFI builds a `CLPK` cache byte-identical to the Rust build. gui
  `cargo fmt --check`/`clippy -D warnings` clean and `cargo test` green; core/ffi
  green; the core builds/tests with `--no-default-features`. (Pre-existing
  rust-1.95 clippy lints in `translate.rs`/`server.rs:929`/`graphdef.rs` are
  unrelated to G7 and left as-is.)

## G8 — Node-tree view + NRT plots (2026-06-26)

Two read-only views that exercise the *gui is a client of the audio server* leg,
both cheap (the flat-geometry painter + bitmap text, no dedicated GPU pipeline),
both added by extension — a new `WidgetKind` plus a renderer, no protocol change.
The server is untouched: G8 reuses the node-tree query/notification path that
already exists (`/g_queryTree`, `/notify`, `/n_go`/`/n_end`).

- **`nodetree` (`host::nodetree`).** A live text view of the server's node tree.
  The model (`NodeTree`/`NodeEntry`/`NodeBody`) and the parser of scsynth's
  depth-first `/g_queryTree.reply` are pure and unit-tested (nested groups, named
  vs index controls, an empty tree, and a truncated reply returning `None` rather
  than panicking). The widget carries `group` (root group, default 0) and a
  `controls` flag (show each synth's name/value pairs). `draw` renders the
  flattened, indented lines into a framed field, clipped to the body height
  (scrolling is future work), with `no server`/`querying...` placeholders for the
  empty states.
- **`plot` (`host::plot`).** A simple static signal view — the lightweight
  counterpart of the heavy navigable `waveform`. It honors the one graphics rule
  (never resolve finer than the screen) by decimating to the pixel width: a
  connected polyline when the data fits, a per-column min/max envelope when it
  does not, plus a zero baseline when the range straddles 0. Samples arrive
  inline (`data`/`blob`) or — the bulk path for an NRT render's output — from a
  mapped local `path` of raw little-endian `f32` (`channels` de-interleaves
  channel 0), reusing the `host::mapfile` mmap, so the samples never ride OSC.
- **Windowed wiring (`host::gui`).** The front mirrors the tree by group
  (`node_trees`), routing `/g_queryTree.reply` into the model and repainting only
  when it actually changed. A node-tree window registers for notifications once
  (`/notify 1`), re-queries immediately on `/n_go`/`/n_end` (node creation/removal
  is snappy) and otherwise polls every 200 ms (`/n_set` control changes raise no
  notification). `about_to_wait` now schedules both the ~30 fps meter/scope
  animation and the node-tree poll. Plots that name a `path` are mapped into the
  host tree on window open (`load_plot_paths`); rendering both views copies their
  rects out of the host-tree borrow exactly as the meters/scopes do. A small
  `Mesh::border` (factored from `meters`) draws the shared framed chrome.
- **Python + examples.** `clausters.gui` gains `nodetree` and `plot` builders;
  `examples/gui_nodetree.py` (a live tree, with a swept `freq` and a synth coming
  and going) and `examples/gui_plot.py` (a `Session.nrt()` render written to a raw
  file and plotted, no server). `GUIA.md` section 18.
- **Tests:** gui 68 (`nodetree` parse/lines/draw ×6, `plot` regimes ×3, the
  `WidgetKind` parse/apply ×2). `cargo fmt --check`/`clippy -D warnings` clean.
- **Verified:** runtime end-to-end against the real server and a GPU window — the
  node-tree window opens and refreshes ~5 Hz tracking a live `/n_set` freq sweep
  (30 distinct updates, host log `node tree for group 0 updated`), and a `plot`
  window maps a 4000-sample file and renders the envelope (`plot: mapped 4000
  samples … (no OSC)`), both with no panic. Headless E2E round-trips a
  `nodetree`+`plot` `/gui_def` and reads them back via `/gui_query` with the
  int/float distinction kept. The core is untouched and still builds/tests with
  no optional features.

## G9 — Canvas + shaders (2026-06-26)

A `canvas` widget that runs a **script-supplied WGSL shader** over its area
(ShaderToy-style), driven by OSC params and by control buses read from shared
memory. Added by extension — a new `WidgetKind` plus a GPU view — with no
protocol change and the audio server untouched.

- **`host::canvas::CanvasView`.** The GPU piece: it wraps the user's `shade`
  function with a fixed prelude (the uniform block + a full-screen-triangle
  vertex shader) and a `fs_main` that calls it, then compiles a pipeline. The
  uniform block is 8 `f32` — `resolution`, `time`, a pad, and a `params` vec4 —
  written each frame. A shader that fails to compile is **caught with a wgpu
  validation error scope** (`push_error_scope`/`pop`), leaving the canvas
  un-painted with a warning instead of crashing the host. `set_shader`
  recompiles in place only when the source changed (so a `/gui_set shader` is
  cheap and a broken shader is not retried every frame).
- **`WidgetKind::Canvas { shader, params: [f32;4], buses: [i32;4], label }`.**
  The four params are driven two ways, the point of the widget: from the script
  (`/gui_set param0…`, an OSC value → `u.params.x…w`) and from a **control bus
  read out of shared memory each frame** (`buses[i]` ≥ 0 maps a bus onto param
  `i`; `-1` keeps it script-driven) — the same zero-message path the meters use.
  `/gui_set bus0…` remaps a slot live. Generic `f32_array`/`i32_array`/
  `index_suffix` prop helpers added.
- **Windowed wiring (`host::gui`).** A canvas builds its `CanvasView` on window
  open (`collect_canvases`); a canvas window is **animated** (continuous ~30 fps
  redraw — time-driven, independent of `--shm`) so `u.time` advances and the
  buses are re-read. Each frame the param vector is resolved (bus slots from
  `read_bus`), the shader recompiled if it changed, the uniforms uploaded, and
  the shader drawn into the widget's viewport (a `body_rect` below an optional
  label) — the same `set_viewport`+draw path the waveform uses. `Mesh::border`
  and the per-view label strip are shared with the other views.
- **Python + example.** `clausters.gui.canvas(id, shader, params=, buses=)`;
  `examples/gui_canvas.py` — an animated shader whose ring pulse follows an OSC
  `param0` the script sweeps and whose green channel follows a control bus the
  script writes (read by the host from shared memory).
- **Tests:** gui 70 (canvas parse/apply/default-shader ×2). `cargo fmt --check`/
  `clippy -D warnings` clean; core untouched.
- **Verified:** runtime end-to-end against the real server + a GPU window — the
  canvas window opens with the segment mapped, the user shader compiles and
  animates from the swept OSC param and the shared-memory bus at once, no panic;
  and a deliberately invalid shader is caught (`canvas shader failed to compile:
  Validation Error`) with the window still opening and no panic.

## G10 — Standalone GuiDef + GraphDef bundles (2026-06-27)

A *bundle* — a data directory holding a named GuiDef beside the
SynthDefs/GraphDefs it needs — that `clausters-gui --standalone <name>` boots as
a self-contained instrument: an embedded audio server, no separate server
process and no language client. GuiDefs persist the way the server's defs do,
and a saved tree carries enough to drive itself.

- **`host::store::GuiStore`.** The GUI's own def store, mirroring
  `src/server/defstore.rs` (so it works in the default gui build, which does not
  compile the server crate): the same data-dir resolution (CLI override →
  `$CLAUSTERS_DATA_DIR` → `$XDG_DATA_HOME/clausters` → `$HOME/.local/share/
  clausters`), `sanitize_name` and atomic temp-file-rename writes. A GuiDef is
  saved as `defs/guidefs/<name>.json`, a record `{ "id": <i32>, "gui": <tree> }`
  (the tree verbatim, JSON the source of truth), beside the sibling
  `defs/synthdefs`/`defs/graphdefs` the store also reads. `boot_messages` parses
  a GuiDef root `boot` array into OSC messages keeping the int/float distinction.
- **Self-driving GuiDefs.** Two props make a saved tree need no live script: a
  root `boot` list of `[addr, args…]` the standalone host sends once the defs
  load (e.g. `["/s_new","drone",1000,0,0]`), and a widget `bind` prop, the
  declarative form of `/gui_bind` — `Binding::from_json([addr, prefix…])`
  registered at `/gui_def` time, so a knob in the file wires straight to the
  server. A live `clausters-gui --data-dir` **auto-persists** any `/gui_def`
  whose root carries a `name` prop, and `/gui_load <name>` replays a saved one
  (`host::on_load`); `Host` now keeps each def's verbatim JSON (`def_json`) for
  the save and the standalone open (`window_def_ids`).
- **`host::embed::EmbedServer` (feature `standalone`).** The embedded server is a
  **direct dependency on the `clausters` crate** (`embed,realtime`, default
  features off) behind the optional `standalone` feature — the gui is part of the
  same ecosystem as the server, so it just links it. `EmbedServer` is a thin
  wrapper over `clausters::embed::Clausters`, constructed and driven through its
  **direct Rust API** (`Clausters::open`/`send`/`poll_into`, `Drop` shuts it
  down). To support that, `src/embed.rs` was refactored to expose that Rust API
  and the C ABI (`clausters_open`/`_send`/`_poll`/`_close`, used by the Python
  client) became a thin wrapper over it — behavior unchanged. The feature is off
  by default because it pulls the engine + audio backend; keeping it opt-in is the
  size/packaging reason the gui is a separate crate (the default build never
  compiles the server). This is the native-Rust counterpart of how the Python
  client reaches the same server over the C ABI; here it is a crate link, not FFI.
- **`ServerLink::{Udp, Embed}`.** The host's client-of-server leg is now an enum,
  so a bound widget's value and the def/boot messages flow to either a UDP server
  or the in-process one through one `send`. In standalone the embed's replies are
  drained each loop turn (`drain_embed_replies`) and fed through the same
  `handle_server_packet` as UDP, so the node-tree view and friends work embedded.
- **The binary.** `clausters-gui` gains `--data-dir <dir>` (opens the GuiDef
  store; named GuiDefs persist, `/gui_load` reads) and `--standalone <name>`:
  load the GuiDef, `EmbedServer::open`, replay the bundle's SynthDef/GraphDef
  specs (`/d_recv`/`/d_graph`), send the GuiDef's `boot` messages, register the
  GuiDef and open its window — a self-contained app. `--standalone` needs the
  `standalone` feature; without it the flag returns a clear "rebuild with
  `--features standalone`" error (the default binary does not link the server).
- **Python + example.** Nothing new to learn: the `guidef` builders already pass
  `name`/`boot`/`bind` through verbatim. `examples/gui_standalone.py` authors a
  bundle on disk — a drone SynthDef (`SynthDef.dump_def` → `defs/synthdefs`) and a
  one-knob GuiDef bound to its `freq`, with `boot` creating the synth (`{id,gui}`
  → `defs/guidefs`) — and prints the `cargo run --features standalone …`
  launch command.
- **Tests:** gui 76, identical with and without the feature (store round-trip/
  missing/boot-parse/sanitize, `from_json` inline binding, named-def persist +
  `/gui_load` reinstantiation). `cargo fmt --check`/`clippy -D warnings` clean on
  both configs; the core builds and tests without `embed`, and the embed cdylib
  (the Python build) still compiles after the C-ABI refactor.
- **Verified:** runtime end-to-end against a real GPU window — the feature-linked
  binary starts the embedded server (no FFI load), the bundle's def loads, the
  `boot` `/s_new` brings the instrument up, the window "Standalone drone" opens
  and the bound knob drives the embedded server, no panic.

## G11 — Host platform seam (agnostic core + Platform traits) (2026-06-28)

No browser code yet: this milestone carves the platform seam so the later web
milestones are trait-fills, not rewrites, and turns browser-readiness into an
invariant a build gate enforces. The host splits into a **platform-agnostic
core** (the widget/protocol logic) that compiles for `wasm32` unchanged and a
**native I/O shell** behind small traits.

- **`pub mod host` is now unconditional** (`src/lib.rs`): the host compiles for
  `wasm32`. Only the I/O shell modules stay `#[cfg(not(target_arch = "wasm32"))]`
  — `client` (the UDP leg), `store` (filesystem persistence), `transport` (the
  UDP server front), `bulk` (the mmap loader) and `gui` (the winit/wgpu driver);
  `shm`/`mapfile` keep their existing `#[cfg(unix)]` (which already excludes
  wasm) and `embed` its `standalone` feature. The pure modules — `widget`,
  `layout`, `guidef`, `registry`, `controls`, `paint`, `font`, `nodetree`,
  `plot`, `meters`, `bind`, `canvas` and the protocol dispatch in `mod` — moved
  out from behind the wasm exclusion and now build for `wasm32` as they are.
- **The traits are the only new surface; the logic behind them moved, not
  rewritten.** `Transport` (send one OSC message to the audio server — the third
  leg; `ServerLink` implements it, a browser WebSocket carrier plugs in behind
  the same trait later), `DefStore` (named-GuiDef save / `/gui_load`; `GuiStore`
  implements it, a wasm host runs with none), `BulkLoader` (resolve a
  waveform/plot `path`/`cache` to `WaveformData`/samples; the new native
  `host::bulk::MmapLoader` is the desktop fill, returning the platform-agnostic
  data the GPU views build from — the `gui` mmap helpers moved into it verbatim),
  and `BusSource` kept **as-is** (already a `dyn` trait the shared segment fills).
- **`Host` no longer names a native type.** `store` is a `Box<dyn DefStore>`
  (was `GuiStore`), `with_store` is generic over `DefStore`; `ClientId` moved
  from the UDP front into the agnostic core so the dispatch names it on every
  platform. `ServerLink`'s `Udp` variant is `#[cfg(not(wasm32))]`, so on `wasm32`
  the enum is uninhabited and the host simply runs with no audio-server leg until
  the web carrier lands (G13) — the bulk-data decision (G7) already reserved the
  network "async fallback" for the browser, which can map neither shared memory
  nor files.
- **Decision (recorded):** for the browser GUI track (G11-G16) the browser host
  always talks to a *separate* audio server over WebSocket, with no in-process
  engine in the browser (the `standalone` `EmbedServer` stays native-only behind
  its feature). This is a **scope boundary, not a fundamental constraint**:
  porting the engine to the browser (a Web Audio / AudioWorklet backend) is a
  larger, separate future track (recorded in `PLAN.md`, "In-browser audio
  engine"). If it lands, the browser gains a second link kind - the wasm analogue
  of the native `Embed` - and the host<->engine OSC rides an in-process channel,
  not WebSocket, exactly as `ServerLink::Embed` does natively; the
  `Transport`/`ServerLink` seam built here takes that variant without a protocol
  change. WebSocket stays the carrier for the *remote* leg, so it never goes away
  - it stops being the only option. The GPU / surface + loop driver stays the
  native `gui` module for now; the web surface is G12, where `Gpu::new` (already
  `async`) is awaited instead of `block_on`.
- **Build gate.** `clients/gui/check-wasm.sh` runs
  `cargo build --lib --target wasm32-unknown-unknown` (the agnostic core, native
  shell excluded), so no later milestone can re-couple the core to native I/O
  unnoticed.
- **Tests:** gui 81, unchanged. `cargo fmt --check` and `clippy -D warnings`
  clean on native **and** on `wasm32`; the standalone-feature build still links
  the embed. The agnostic core builds for `wasm32` (wgpu compiles to the WebGPU
  backend). The only `#[cfg(not(wasm32))]` left inside `host` is the I/O shell,
  never the widget/protocol logic.
- **Verified:** the native host behaves byte-identically — the headless E2E
  round-trip (`gui_skeleton.py` against the running host) parses the GuiDef and
  answers `/gui_query` with `/gui_info` exactly as before, no panic.

## G12 — Web surface: `<canvas>` WebGPU + async GPU + render loop (2026-06-28)

The first browser pixels. With no transport yet, the surface/GPU/loop port is
isolated from the protocol: a compiled-in GuiDef renders in a browser tab over
WebGPU through the **same** render code the desktop runs.

- **Shared frame path (`host::frame`).** The per-window render moved verbatim out
  of the native `gui::App::render` into a platform-agnostic `frame::render(gpu,
  painter, waveforms, canvases, scopes, tree, &FrameInputs)`. Both fronts now
  draw a tree through one function, so the browser is pixel-faithful by
  construction, not a parallel renderer. `FrameInputs` carries the live values the
  native front has (the shared-memory `BusSource` for meters/canvas, the scope
  histories, the node trees, the held-button highlight); its `Default` is the
  no-transport case the browser uses at G12 (no bus, empty node tree). The native
  `App::render` is now a thin wrapper that gathers those inputs (disjoint field
  borrows) and calls it; `WaveformSlot` and the `CLEAR`/panel/label constants
  moved with it.
- **`Gpu` is agnostic (`crate::gpu`).** The wgpu device/surface bring-up moved out
  of the native-only `native.rs` into a shared module (it compiles to the WebGPU
  backend on wasm), used by the native harness, the windowed front and the web
  entry. `Gpu::new` was already `async`; only *when* it is awaited differs per
  platform.
- **`host::web` (wasm-only).** A `wasm-bindgen` `start` entry that builds a winit
  window over an HTML `<canvas>` (`WindowAttributesExtWebSys::with_append`),
  requests the WebGPU adapter/device **asynchronously** via
  `wasm_bindgen_futures::spawn_local` and hands it back through an
  `EventLoopProxy` `GpuReady` user event (winit's web loop is single-threaded, so
  moving the non-`Send` `Gpu` through the proxy is fine), and drives the render
  from `RedrawRequested` — **no `block_on`, no socket, no mmap on the wasm path**.
  The compiled-in GuiDef (a panel of controls + an inline-data `waveform`) is
  authored as the same JSON a client would send and built through the unchanged
  `GuiNode::parse` + `Widget::from_node` path.
- **Packaging.** `[lib] crate-type = ["cdylib", "rlib"]` (cdylib for wasm-bindgen,
  rlib so the native bins still link); wasm32-target deps (`wasm-bindgen`,
  `wasm-bindgen-futures`, `console_error_panic_hook`, `web-sys` `console`).
  `clients/gui/web/build.sh` runs the wasm build + `wasm-bindgen --target web`
  into `web/`, loaded by `web/index.html` (the generated `.js`/`.wasm` are
  git-ignored). Serve over http (WebGPU needs a secure context; localhost
  counts) and open in a WebGPU browser.
- **Verified:** native unchanged — gui 81 tests, `clippy -D warnings` clean on
  native and `wasm32`, and the windowed host opens a real GPU window through the
  shared `frame::render` with no panic. The wasm bundle generates with
  `wasm-bindgen`; loaded in Chrome over WebGPU (Vulkan/ANGLE) the **full path
  executes** — the console logs `start`, the `<canvas>` window, the async device
  coming up, and `frame::render` running ("WebGPU ready; rendering the GuiDef"),
  no panic. Capturing the rendered pixels in a screenshot needs a non-headless
  WebGPU browser (a headless-Chrome WebGPU readback limitation, not a code issue).

## G13 — Web transport: drive the browser host live over WebSocket (2026-06-28)

The browser host stops being static: it runs the **real** `Host`, fed live
through a binding surface and forwarding bound widgets to a `--ws` audio server.
Reuses the whole protocol dispatch, the G1 WS wire format and the shared render;
the new code is the carrier and the page glue.

- **Shared interaction (`host::interact`).** Hit-testing and the value/toggle/
  menu mutations were extracted verbatim out of the native front into an
  agnostic module (`hit`, `fraction_of`, `set_fraction`, `flip_toggle`,
  `cycle_menu`, `value_of`, `slider_t`) — pure work on the `Host` tree plus the
  `layout`/`controls` math. The native front now delegates to it; the browser
  front calls the same functions, so a turned knob updates the tree and decides
  bound-vs-event identically on both platforms.
- **The binding surface (`GuiBridge`, wasm-bindgen).** The in-page JS holds it:
  `feed(packet)` pushes a raw OSC packet (a `/gui_def`/`/gui_set`/`/gui_bind`, the
  G1 one-packet-per-frame format) to the host through the one `decode_packet`
  door; `def(id, json)` is the convenience that builds the `/gui_def` from a
  GuiDef JSON string (the same JSON the Python builders emit, so a page needs no
  OSC encoder); `poll()` drains the outbound `/gui_event`/`/gui_info` packets
  (encoded OSC) for the page; `connect_server(url)` attaches the audio-server
  leg. The bridge reaches the running app through the winit web `EventLoopProxy`
  and shares an outbox `VecDeque`. `start()` now **returns** the bridge (no
  `#[wasm_bindgen(start)]`), so the page gets a handle.
- **The audio-server leg over WebSocket (`ServerLink::Ws`, `WsServerLink`).** A
  new cfg-gated `ServerLink` variant wrapping a browser-native `web_sys::WebSocket`
  to a `--ws` server: `send` encodes one OSC message per binary frame, buffering
  frames until the socket opens (so a `connect` immediately followed by a turn
  loses nothing). `ServerLink::send` gained the `Ws` arm (replacing the wasm
  `unreachable!`); `impl Transport for ServerLink` is now native-only (the `Ws`
  link wraps a non-`Send` socket but never crosses a thread and is reached
  through the inherent `send`). `Host::set_server_link` attaches it on demand;
  `ClientId::Web` names the in-page origin in the dispatch. A bound widget's value
  thus reaches the `--ws` server with no script round-trip — the bypass path, in
  the browser.
- **The throwaway harness (`web/index.html`).** A few lines, explicitly not a
  product client (the TypeScript client is the separate `clients/web` track): it
  builds the same GuiDef JSON a Python builder emits (a panel of controls + an
  inline `waveform`, with a declaratively `bind`-ed `freq` knob), feeds it with
  `gui.def(...)`, optionally `connect_server(?server=ws://…)`, and drains events.
- **Graceful no-WebGPU.** `Gpu::new` now returns `Result` instead of
  `expect`-ing the adapter: a browser without WebGPU enabled (e.g. Linux Chrome
  without the WebGPU/Vulkan flags) makes `request_adapter` return `NotFound`, so
  the web front logs a clear, actionable message and writes it into the page's
  `#note` (the canvas stays blank, the page survives) rather than aborting; the
  native front warns and skips the window. The adapter request also drops the
  `HighPerformance` preference (the permissive default) for broader
  compatibility.
- **Verified:** native unchanged — gui 81 tests, `clippy -D warnings` clean on
  native and `wasm32`, the windowed host opens a real GPU window with no panic.
  In Chrome over WebGPU the live path runs: the console shows `start` returning
  the bridge, the binding surface feeding the GuiDef and the host **opening the
  window from the page** (`/gui_def 1: window opened from the page`), then the
  async device up, no panic. The knob→`/gui_event` interaction reuses the shared
  `interact` path (the same the native smoke exercises); the `bind`→`--ws`
  server bypass is a manual end-to-end test (a running `--ws` server + a real
  browser). The full TypeScript client remains a separate, unplanned track.

## G17 — WebGL2 fallback: browser reach where WebGPU is disabled (2026-06-28)

Landed out of sequence (a reach fix on the G12 web surface, ahead of G14–G16):
the browser host renders over **WebGPU where it works and WebGL2 otherwise**, so
it keeps working on browsers — notably on Linux, and on older Android — whose
WebGPU is disabled by the drivers. The motivation is accessibility: the point of
the web target is reach, and WebGPU on Linux/Android is unreliable, so a
universal fallback (WebGL2 is supported almost everywhere) is what makes the web
host actually usable there.

- **Cheap by design — no renderer, shader or DSP changes.** The crate already
  avoids everything WebGL2 cannot do, partly from explicit G7 decisions: no
  compute shaders and no storage buffers anywhere (only vertex/fragment render
  pipelines, uniform buffers, one `R8Unorm` texture + a linear sampler); the
  heavy numeric work (FFT via `microfft`, the peak pyramid) runs on the CPU in
  `clausters-core`; the WGSL shaders — including the script-supplied `canvas`
  shader (G9) — are translated to GLSL ES 3.0 by naga automatically (a shader
  that fails to translate already falls into the existing `push_error_scope`,
  staying unpainted with no panic). `R8Unorm` was chosen in G7 precisely for
  being filterable everywhere, which is WebGL2-safe too. The whole change is
  config + a few lines in `gpu.rs`.
- **Both backends compiled, picked at runtime (`gpu.rs`).** The wasm build now
  enables wgpu's `webgl` feature alongside the default `webgpu`
  (`Cargo.toml`, in the `wasm32` target deps so native is untouched). `new`
  builds the instance through wgpu's recommended
  `util::new_instance_with_webgpu_detection` with
  `Backends::BROWSER_WEBGPU | Backends::GL`: it keeps WebGPU only when the
  browser can actually create a WebGPU adapter — it probes for one, not just for
  `navigator.gpu`, which is exactly the Linux-Chrome case (the property exists
  but no adapter can be made) — and otherwise drops to WebGL2. No branch logic of
  our own; native keeps `Instance::default()`.
- **Device limits (`gpu.rs`).** `request_device` with the full default limits
  would fail on a WebGL2 adapter, so the web path requests
  `Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())` —
  the WebGL2 floor, but with the texture-size limits lifted back to what the
  adapter actually reports (a long spectrogram texture needs the real maximum,
  not the 2048 floor). On a WebGPU adapter this stays well within support. Native
  keeps wgpu's defaults. `required_features` stays empty.
- **Messaging.** The "no adapter" error and the page `#note`/harness text now
  read in terms of "neither WebGPU nor WebGL2" (a near-impossible state, since
  WebGL2 is ubiquitous) and per-platform hints (`gpu.rs` `NO_ADAPTER_HINT`,
  `host::web` logs, `web/index.html`), replacing the WebGPU-only wording from
  G13.
- **Verified:** native unchanged — gui 81 tests pass, `cargo fmt --check` and
  `clippy -D warnings` clean on both native and `wasm32`, and a native windowed
  smoke (the `waveform` binary) brings the GPU up and stays alive with no panic.
  The wasm `--lib` build for `wasm32-unknown-unknown` now compiles the GL backend
  in (`glow`/`wgpu-hal`/`wgpu-core`), so `check-wasm.sh` covers it. **End-to-end
  in a real Linux browser whose WebGPU is disabled:** the WebGPU probe fails
  (`No available adapters.`), the host falls back to a WebGL2 adapter and renders
  the panel, controls, bitmap text and the inline waveform — visually confirmed
  (the first actual pixels of the browser host; G12/G13 were console-only). A
  turned unbound control prints its `/gui_event`; a `bind`-ed widget stays silent
  (the bypass path, to a `--ws` server when one is attached).
- **Surface-size fix found in that verification (`host::web`).** The first build
  rendered only the clear color (a gray canvas, no widgets): on the web the
  `<canvas>` is often not laid out when `Gpu::new` reads its size (captured before
  the async adapter/device awaits), so the surface came up configured to a stale
  1x1 — the clear stretched to fill the canvas while every widget laid out into a
  ~0 px area. `GpuReady` now re-reads the laid-out size (falling back to a
  `pending_size` stashed from any `Resized` that arrived while the GPU was still
  coming up) and reconfigures before the first frame; it logs the resolved
  surface size. This was latent on the WebGPU path too, never caught because it
  was only console-verified. Native is unaffected (its size is stable before the
  awaits).

## Config — shared TOML configuration files (completed 2026-06-28)

**What's there:** a single TOML configuration schema, read by the server, the
GUI host and the Python client, plus a no-interpreter standalone launch that
loads a whole bundle from a data directory.

- **Shared model in the core (`clausters-core::config`).** Serde structs with
  all-`Option` fields (`Config { server, client, gui, standalone }`), a
  field-by-field `merge` (higher layer wins), and the native path resolution
  (gated off `wasm32`, like the rest of the platform seam): user file
  (`$CLAUSTERS_CONFIG`, `$XDG_CONFIG_HOME`, `%APPDATA%`, `~/.config`) merged
  under the nearest project `clausters.toml` walking up from the CWD. The structs
  are agnostic and compile to wasm; only the file reading is native, so the
  `toml` dependency is gated to non-wasm. Precedence end to end: **CLI flag >
  project file > user file > compiled default**. The config is read-only to the
  programs; machine-written state (def store, `boot.json`, `midi.json`) is
  untouched.
- **Server (`src/main.rs`).** `realtime_main` seeds every option default from
  `[server]` before parsing flags, so a flag still overrides the file. The
  `tcp`/`ws`/`midi` toggles accept `true`/`false` or a concrete port/name.
- **Embedded server loads the data-dir store (`src/embed.rs`).**
  `Clausters::open_with_data_dir` attaches a `DefStore` and runs the same boot as
  the standalone server binary (SynthDefs, Faust defs, GraphDefs, MIDI bindings,
  `boot.json`). This closed a gap: the previous standalone path replayed only
  SynthDefs/GraphDefs by hand and skipped Faust defs and `boot.json`. A bundle
  with Faust defs warns when the build lacks the `faust` feature.
- **GUI standalone (`clients/gui`).** `clausters-gui` reads `[gui]`/`[standalone]`
  as flag defaults, takes a `--config <path>` override, and accepts
  `--standalone` with no name (falling back to `[standalone].gui`).
  `run_standalone` now lets the embedded server load the bundle itself; a new
  `standalone-faust` feature pulls `clausters/faust` for Faust bundles.
- **Python client.** `clausters.config.load_config()` reads the same files with
  `tomllib` (so `requires-python` rises to 3.11), merging project over user.
  `Server`/`ServerOptions` and `Session.live()` take their `[client]`/`[server]`
  defaults from it; an explicit argument still wins.

**Docs/examples:** `docs/configuration.md` (canonical schema/precedence) and a
Python `configuration.md`, both linked from their SUMMARYs; a commented
`examples/config.toml`; the standalone example notes that re-launching needs no
interpreter; `GUIA.md` gains a Config section and a table row.

**Verified:** `cargo test -p clausters-core config` and the Python
`tests/test_config.py` cover the project-over-user merge in both languages
(parity); `check-wasm.sh` still builds (config structs agnostic); server,
`clausters-gui` (default + `standalone`) and the full Python suite build/pass.

## EnvGen — segment envelopes with done actions (completed 2026-07-01)

**What's there:** the first envelope UGen, `EnvGen`, modelled on SuperCollider's
— all its shape curves, gate-driven sustain and sustain-loop at the release
node, and `doneAction`s (pause/free-self/free-group) applied RT-safely — plus the
client-side `Env`/`env_gen` builders. Started from a first pass (the previous
commit) that implemented only linear/exponential shapes and, crucially, did not
sustain at the release node; this entry is the correctness fix + completion
(the loop node and the pause/free-group actions landed in a follow-up).

- **All shapes (`src/dsp/envgen.rs`).** A pure `shape_value(shape, curve, a, b, t)`
  covers the SC set: 0 step, 1 linear, 2 exponential (same-sign non-zero, zeros
  nudged), 3 sine, 4 welch, 5 custom-curvature (uses the `curve` value), 6
  squared, 7 cubed, 8 hold. The generator interpolates by segment fraction (more
  numerically robust than SC's per-sample recurrences), landing exactly on each
  target when the segment completes.
- **Gate, sustain and loop.** A rising gate (re)triggers from `initLevel`; a
  persistent `released` flag distinguishes "sustaining, waiting for release" from
  "released, playing out". While the gate is open and the envelope reaches
  `releaseNode`, it **holds** that level (the previous pass played straight
  through, breaking ADSR); on release it resumes from that segment.
  `releaseNode < 0` = one-shot. A `loopNode` (`< releaseNode`) turns the held
  phase into a **cycle** of the segments in `[loopNode, releaseNode)`, carrying
  the release-node level back as the loop's start; the release still plays out.
- **Done actions (`node`, `server::engine`).** The `UGen` trait gained a
  `done()` hook, polled after `process`; `UGenSynth::done_action` aggregates. The
  tree handles each action by kind: `PauseSelf` inline (a `NodeSlot::paused` flag
  skips the synth from the next block — silent but kept; no `/n_run` to resume
  yet); `FreeSelf`/`FreeGroup` recorded (id + action) into a lock-free
  finished-node queue (concurrent `fetch_add` reservation), drained once per
  block after the whole walk — `FreeSelf` frees the node, `FreeGroup` its
  enclosing group (`free_enclosing_group`, falling back to the synth when that is
  the root), both via the normal `NodeTree::free` path onto the garbage FIFO,
  never dropped on the audio thread. Re-queue from a split block or two group-mates
  both freeing the group is a harmless no-op. Replaced the earlier 64-per-block cap
  (which could leak frees) with an unbounded drain via `take_done_count`/
  `done_node`/`done_action_at`. `MAX_UGEN_INPUTS` rose to 32 and the compiler now
  allows variable-arity UGens (`arity == usize::MAX`) while still rejecting inputs
  beyond the cap.
- **Client (`clients/python`).** `Env` (breakpoint builder with `perc`/`adsr`/
  `asr`, per-segment shape names or numeric curvatures) and `env_gen`, plus a
  `DoneAction` constant set, serializing to the flat `EnvGen` input list.

**Docs/examples:** the UGen table + an envelope note in `docs/schemas.md`; a
"Done actions" subsection and the variadic/`done()` steps in
`docs/architecture.md`; a `GUIA.md` section and checklist row; a commented
`clients/python/examples/envelope.py` (a self-freeing ADSR pad rendered offline).

**Verified:** `cargo test --test envgen` (linear ramp + hold, constant
exponential ratio, sustain that only advances on gate release, loop cycling with
release exit, `pauseSelf` that stops output but keeps the node, `freeGroup` that
frees the enclosing group, and `doneAction=2` freeing the node) and the no-alloc
`envgen_free_self_...` scene in `tests/rt_safety.rs`; the client `test_env_*` in
`test_synthdef.py` (input-layout, shapes, release/loop nodes, done-action
constants); the example renders end to end through the embedded NRT renderer.

## S1 — Calculation rates as a first-class property (completed 2026-07-02)

**What's there:** the first S-track milestone — the **rate substrate** every
future UGen leans on. Before S1 the wire model was implicit: a UGen output was
either a full `Block` (audio-rate) or a scalar (a constant/control, effectively
`kr`), decided by construction. Now the four scsynth rates are an explicit,
validated property of every UGen output, with the engine plumbing behind each:
`ar` (per sample), `kr` (once per block), `ir` (once at synth init, then held)
and `dr` (pulled on demand). Infrastructure, not a UGen catalog: it ships two
tiny `ir` UGens and a minimal `dr` driver to prove the contracts; the demand
family, table oscillators and FFT chains build on them later.

- **The `Rate` enum + trait hooks (`src/dsp/mod.rs`).** `Rate { Ir, Kr, Ar, Dr }`
  with a coercion `rank` (`ir < kr < ar`; `dr` off-axis) and `parse`/`as_str` for
  the wire names. The `UGen` trait gains three defaulted hooks for the pull
  protocol: `demand(ctx, inputs) -> f32` (a source yields its next value, `NaN`
  = exhausted), `reset_demand()`, and `drive(trig, reset, output, step)` (a
  driver steps a block, pulling via a `step` callback). Non-demand UGens
  implement none of them.
- **Rate is registry data, not a per-UGen match (`src/dsp/registry.rs`).**
  `default_rate(kind)` (the rate when a def omits one) and `rate_allowed(kind,
  rate)` sit next to `arity`, and both are **default-friendly**: the fallthrough
  is the signal-processor case (`ar`/`kr`), so the open-ended family —
  oscillators, filters, arithmetic, every UGen added later — needs no entry.
  Only the bounded exceptions are listed: the `ir` scalars and `dr` source that
  *widen* the set, and the block-I/O kinds (bus/disk/feedback) that *narrow* it
  to `ar`-only. This was a mid-implementation correction after the reviewer
  flagged the original exhaustive enumeration as unscalable.
- **New UGens.** `src/dsp/scalar.rs`: `SampleRate.ir` (the engine rate) and
  `Rand.ir(lo, hi)` (one uniform value drawn once, held — the sharpest test of
  the init pass, since recomputing it would differ). `src/dsp/demand.rs`:
  `Dseq(repeats, values…)` (a demand source cycling a list) and `Demand(trig,
  reset, source)` (the driver). Both `scalar` and `demand` sit in the shared
  `clausters_core::rng` lineage for the RNG.
- **Compiler inference + validation (`src/synthdef/mod.rs`).** `UGenSpec` gains
  an optional `"rate"` (serde-default, so every existing def is unchanged);
  `UGenDef` gains a resolved `Rate`. `compile` picks the explicit-or-default
  rate, checks it against `rate_allowed`, and validates coercion: an `ir` UGen
  must have only `ir` inputs (a varying source can't be frozen), and a `dr` wire
  may feed **only** a `Demand`'s source slot (and that slot must be `dr`). Each
  rejection names the offending node like the rest of `compile`.
- **Rate-aware instance (`src/synthdef/instance.rs`).** Each output slice is
  sized by rate (`ar` → `frames`, `kr`/`ir` → 1), and input wires are sliced by
  their **producer's** rate, so a length-1 `kr`/`ir` wire flows through `at()` as
  a block constant. The **`ir` init pass** runs each `ir` UGen once on the first
  block (an `initialized` flag) and skips it thereafter — its wire persists and
  holds the value. Chosen to run on the audio thread, not in `UGenSynth::new` as
  the plan first suggested, because `ir` values often need `ctx` (sample rate,
  buffer pool) that only exists there; it stays RT-safe because an `ir`
  `process` only reads. `dr` UGens are skipped in block order; `Demand` is
  special-cased like `LocalIn`/`LocalOut`, resolving its source and driving it
  through a stack `step` closure — a single mutable path to the source, no
  allocation.

**Deviations from the plan (per the S-track design stance):** the `ir` init pass
runs on the **first audio block**, not on the network thread in
`UGenSynth::new`, for the `ctx`-availability reason above (still once, still
held). The `dr` driver is deliberately minimal (one source per driver,
end-of-stream = a held value) — enough to prove the pull protocol the demand
family will extend.

**Docs/examples:** a *Calculation rates* section in `docs/architecture.md`'s
"How to add a UGen" (the rate table, the registry-as-data note, the `ir` init
pass and the `dr` sub-list contract); the `"rate"` field, the four new UGen rows
and two notes (rates; demand sequences) in `docs/schemas.md`; a `GUIA.md`
section and checklist row.

**Verified:** `cargo test --test rates` (a test per rate — `ar` varies per
sample, `kr` is block-constant but tracks its input across blocks, `SampleRate`
reports the engine rate, `Rand.ir` stays frozen in range, `Demand`/`Dseq` steps
and loops a sequence and resets/exhausts — plus five compiler-rejection tests);
the no-alloc `rate_substrate_does_not_allocate_on_the_audio_thread` scene in
`tests/rt_safety.rs` (the `ir` init pass + the demand pull path); the full core
suite (`--no-default-features`) stays green and `cargo fmt --check`/clippy clean.

## Registry — the UGen catalog as descriptors, not central switches (2026-07-02)

**What's there:** a follow-up refactor, prompted by review of S1: the UGen
registry no longer enumerates every kind in parallel `match`es. It flagged a
real coupling — the *general logic* (compiler, bus analysis) was reaching into
per-kind switches, and the *wire schema* documentation enumerated the concrete
Rust kinds as if they were part of the protocol. The fix separates the two.

- **One `UGenDescriptor` per kind (`src/dsp/registry.rs`).** A descriptor holds
  `name`, `arity` (`Fixed`/`Variadic`), `default_rate`, allowed `rates`,
  `exec` mode, `bus` role, `needs_path`, and a `build` fn — all the metadata,
  co-located. A single `UGENS` table is the catalog; `lookup(name)` finds a
  descriptor. The old `UGenKind` enum and the parallel `parse_kind`/`arity`/
  `build`/`default_rate`/`rate_allowed` switches are gone.
- **The compiler and bus analysis went generic.** `synthdef::compile` reads
  descriptor fields (arity, `allows(rate)`, `exec` for the LocalIn/LocalOut/
  Demand special cases, `needs_path`); `UGenDef` carries a
  `&'static UGenDescriptor` instead of a kind enum; `osc::graph::ugen_usage`
  reads `desc.bus`. No file matches on a kind anymore.
- **The small closed sets stayed enums** — because they *are* general logic,
  not per-implementation data: `Rate`, plus new `ExecMode` (Normal / LocalIn /
  LocalOut / DemandDriver — the only synth-coordinated behaviors, matched in
  `instance.rs`), `BusRole` and `Arity`. Adding a UGen no longer touches any of
  these; a new signal UGen copies a `SinOsc`-style row.
- **Doc reframing (`docs/schemas.md`).** The kind table is now "UGen catalog
  (built-in kinds)", explicitly *not* part of the wire schema: `kind` is an
  opaque string the server resolves and the catalog grows independently. "How
  to add a UGen" in `architecture.md` now says "add one `UGENS` row".
- **Holes left for the S track:** an `op` field in `UGenConfig` is where S3's
  special-index `BinaryOpUGen`/`UnaryOpUGen` land (a table entry, not a new
  kind); S2's typed controls and its compile-time `Lag` insertion are a new
  descriptor plus control-side work, unaffected by this change.

**Verified:** the full core suite (`--no-default-features`, 207 tests) stays
green — including the disk, buffer, auto-order, graphdef and parallel scenes
that exercise every descriptor field — with `cargo fmt --check`, clippy and
`cargo doc` clean.

## S2 — Typed controls: tr, lag/varlag, and scalar (ir) controls (completed 2026-07-02)

**What's there:** SynthDef controls now carry a **type** the def author chooses,
the way scsynth's do. A control was one mutable `f32` read once per block (a
plain `kr`); S2 adds the three other behaviors, each wired by the compiler and
the engine, RT-safe.

- **The type in the wire format (`src/synthdef/mod.rs`).** `ControlSpec` gains
  an optional `"rate"` (`kr`/`tr`/`ir`, also spellable `control`/`trigger`/
  `scalar`) plus `"lag"`/`"lag_down"` times; all serde-default, so every
  existing def is unchanged. A `ControlType` enum rides on `SynthDef`
  (`control_types`, parallel to the names). `compile` parses and validates them
  (unknown type, lag only on a `kr` control, `lag_down` needs `lag`).
- **Trigger (`tr`) — `src/synthdef/instance.rs`.** After the UGen loop,
  `process` resets every trigger control to `0`, so a `/n_set` value holds for
  exactly one block and a rising edge fires once (an `EnvGen` gate, a
  sample-and-hold). Unconditional and cheap; no per-control "fired" flag needed.
- **Scalar (`ir`).** `set_control` ignores a write to a scalar control once the
  synth's `initialized` flag (reused from S1) is set — the `/s_new` init values,
  applied before the first block, still take; a later `/n_set` is dropped, per
  scsynth. In the compiler an `ir` control counts as `Rate::Ir` for input
  coercion, so it may feed an `ir` UGen input (e.g. `Rand.ir`).
- **Lag / varlag — an inserted UGen, not a bespoke path.** New `Lag(in, time)`
  and `VarLag(in, up, down)` one-pole smoothers (`src/dsp/lag.rs`, scsynth's
  `b1 = exp(ln(0.001)/(time·sr))`, primed to the first input). A control with a
  `"lag"` compiles to a real `Lag`/`VarLag` **prepended** to the graph reading
  the raw control; every reference to that control is rewritten to the
  smoother's wire (the `lagged` pass in `compile` shifts the original UGens down
  and remaps their wire indices). So there is one lag implementation, shared
  with the client-facing UGen. The inserted smoothers run at audio rate, so a
  stepped control glides per sample.

- **Client mirror (`clients/python`).** `defs.control` gained `rate` (`tr`/`ir`,
  also `trigger`/`scalar`), `lag` and `lag_down` keywords; `Control` validates
  them (unknown type, `lag_down` without `lag`) and its `_signature` folds
  type/lag into the conflicting-reuse check, and `SynthDef.spec` emits the
  optional `rate`/`lag`/`lag_down` control fields plus a per-UGen output `rate`.
  New UGen callables mirror the substrate: `lag`/`var_lag` (the smoothers),
  `sample_rate`/`rand` (the `ir` scalars) and the `dseq`/`demand` pair (`dr`),
  each `Ugen` carrying an optional output `rate` set fluently with `at_rate`.

**Deviation:** the inserted lag runs at **audio rate**, not control rate as in
scsynth. A `kr` Lag would need the control-block rate for its coefficient (a
subtlety when a length-1 wire has no block context); an `ar` Lag over a
block-constant target is simpler, correct, and glides without zipper — a
strictly nicer result at a small cost, consistent with the S-track stance.

**Docs/examples:** a *Control types* table in `docs/schemas.md` plus the
`Lag`/`VarLag` catalog rows; a *Typed controls* section in `architecture.md`
(the trigger reset, the `ir` freeze, the compile-time lag insertion + wire
remap); a `GUIA.md` section and checklist row. Client side: a *Control types and
rates* section and the new UGen rows in the Python book's `defs.md`, a
`typed_controls.py` example (offline WAV render — a lagged glide, a `tr`
re-pluck, an `ir` random detune, driven by a `send_bundle` routine) listed in
`examples.md`, and a section in the Python `GUIA.md`.

**Verified:** `cargo test --test controls` (a `tr` fires exactly one block then
resets; an `ir` control freezes under `/n_set` but takes its init value; an `ir`
control may feed `Rand.ir` while a `kr` one is rejected; `lag` glides a step
rather than jumping and converges; `varlag` rises fast and falls slow — plus
three compiler rejections); a no-alloc `typed_controls_...` scene in
`tests/rt_safety.rs` (trigger reset + inserted Lag + scalar reject); the full
core suite (215 tests) green, `cargo fmt --check`/clippy/`cargo doc` clean.
Client: `clients/python` tests green (the typed-control/rate serialization and
the validation rejections in `test_synthdef.py`), and `typed_controls.py`
renders 8 distinct plucks offline (RMS envelope verified) proving the `tr` reset
and lag glide end to end through the embedded renderer.

## S3 — Operator UGens (BinaryOpUGen/UnaryOpUGen) + MulAdd/Sum3/Sum4 (completed 2026-07-02)

**What's there:** math on a UGen graph beyond the four arithmetic kinds. Two
generic op UGens carry the operator by **name** in their `op` field; every
operator is one entry in the shared `clausters_core::builtins`, so the server's
audio-thread op and a client's off-RT value compute with the same code —
bit-identical for the native ops (the C0 discipline applied to the operator
layer).

- **Naming, not numbering (decided with the user).** The wire `op` is the
  operator's **name** (`"mul"`, `"midicps"`, …). scsynth's "special index" is an
  implementation detail — internally each op still has a stable C-ABI integer in
  `clausters_core` (`from_u32`, needed by the FFI), but that number never crosses
  the wire and is not in any doc; the ops act as ordinary UGens addressed by
  name. So scsynth's tables were only a checklist of *which* ops to include.
- **Core (`crates/clausters-core/src/builtins.rs`).** Extended both `BinaryOp`/
  `UnaryOp` enums with a broad, well-defined set — `hypot`, `ring1`–`ring4`,
  `sumsqr`/`difsqr`/`sqrsum`/`sqrdif`, `absdif`, `thresh`, `clip2`, `excess`,
  `round`, `trunc` (binary); `squared`/`cubed`/`recip`, `frac`, `sign`, `log2`,
  `sinh`/`cosh`/`tanh`, the pitch/gain conversions `midicps`…`cpsoct`,
  `distort`, `softclip` (unary) — with their `apply_*` formulas plus `name()`/
  `from_name` (the wire spelling) and unit tests (incl. a name round-trip).
- **Server op UGens.** `dsp::binop::BinaryOp::from_index` and a new
  `dsp::unop::UnaryOp` map the resolved op to the core op and call the shared
  `*_slice` per block; `dsp::fused` adds `MulAdd` (`a*b+c`), `Sum3`, `Sum4` as
  ordinary fixed kinds composing the core operators. `Add`/`Sub`/`Mul`/`Div`
  stay as thin alias kinds (existing defs byte-identical).
- **Registry + compiler.** `UGenDescriptor` gains `op_family: Option<OpFamily>`
  (set only on the two op rows via a `desc_op` helper; every other row unchanged
  through a forwarding `desc`); `UGenSpec.op` is a name string; the compiler
  resolves it to the internal index via `from_name` (missing/unknown → `/fail`
  naming the node) and stores that index in `UGenConfig` for `build`.
- **Client mirror.** `defs.ugens` maps every operator/method selector to the
  operator **name** and emits `BinaryOpUGen`/`UnaryOpUGen` with `op` (the four
  arithmetic keep their alias kinds); `Ugen` carries an `op`, `SynthDef.spec`
  serializes it. `base.builtins` grows the new primitives and now routes the
  pitch/gain **conversions through the core** (f32) too — they were pure-Python
  f64 before, so this is what makes them bit-identical to the server. New
  `AbstractObject` methods expose the ops (`.distort()`, `.clip2()`, …), so the
  same expression composes a value, a Faust graph or a UGen graph; `mul_add`/
  `sum3`/`sum4` callables for the fused kinds.

**Scope note:** the RNG/approximation and fold/wrap opcodes (`randRange`,
`expRandRange`, `hypotApx`, `fold2`, `wrap2`, `gcd`, `lcm`, `roundUp`) are left
as future rows — each is one more `builtins` entry, the mechanism proven.

**Verified:** `cargo test --no-default-features` green — `tests/core_parity.rs`
drives the **whole** opcode table plus `MulAdd`/`Sum3`/`Sum4` through the real
`UGen::process` and asserts bit-identity with the core (bit-pattern compare so
equal NaNs match); `tests/ops.rs` the compile+render path and the `op` index
validation (missing/unknown/arity); a no-alloc `operator_ugens_...` scene in
`tests/rt_safety.rs`; core unit tests; `cargo fmt`/clippy clean (no new
warnings). Client: `clients/python` suite green (graph serialization of the op
UGens and value-side ops in `test_synthdef.py`/`test_base.py`), and a live E2E —
`graph_maths.py` renders a lead built entirely from graph maths (`midicps`,
`distort`, a `clip2` tremolo) offline through the rebuilt embed lib. Both mdBooks
build clean.

## S4 — Complete the done-action set (0-15) + `/n_run` (resume) + non-terminal pause (completed 2026-07-03)

**What's there:** the full scsynth done-action set and a resume path, so a
finished node can act on its neighbours and a paused node is no longer stuck.
`DoneAction` went from 4 values (`None`/`PauseSelf`/`FreeSelf`/`FreeGroup`) to
all **16** (0-15), and **`PauseSelf` is no longer terminal**.

- **The enum (`src/dsp/mod.rs`).** `DoneAction` is now `#[repr(u8)]` over 0-15
  with the relative actions — `FreeSelfAndPrev`(3)/`Next`(4),
  `FreeSelfAndFreeAllInPrev`(5)/`Next`(6), `FreeSelfToHead`(7)/`ToTail`(8),
  `FreeSelfPausePrev`(9)/`Next`(10), `FreeSelfAndDeepFreePrev`(11)/`Next`(12),
  `FreeAllInGroup`(13), `FreeSelfResumeNext`(15). A single `from_u8`/`from_i32`
  maps the wire/UGen integer (out-of-range → `None`); `EnvGen` and
  `NodeTree::done_action_at` both route through it, replacing the two ad-hoc
  `match` ladders that only knew 1/2/14.
- **Tree handlers (`src/node/mod.rs`).** New `apply_done_action(id, action,
  sink)` resolves the previous/next sibling (`sibling_id`, index arithmetic over
  the parent group's ordered child list) and the head/tail runs, **before**
  freeing self (freeing shifts positions), then reuses the existing
  `free`/`free_all`/`deep_free` machinery (`free_or_free_all`/`free_or_deep_free`
  pick the group-vs-node branch). `set_paused(id, paused)` pauses/resumes a synth
  **or a whole group** — the walk now checks `slot.paused` at the top, so a
  paused group skips its entire subtree. All allocation-free (the pre-allocated
  `free_stack`/`dfs_stack`), asserted by a no-alloc scene.
- **`/n_run` command.** `Cmd::RunNode { id, run }` (`src/server/engine.rs` →
  `set_paused(id, !run)`); the translator maps `/n_run` pairs `(nodeID, flag)`
  (`flag 0` pause, non-zero resume). **Wired into both dispatch paths**: the
  immediate/UDP whitelist in `src/osc/server.rs` (this was the one gap — the
  arm was missing, so an immediate `/n_run` fell through to the default `/fail`
  even though the translator handled it) and the scheduled-bundle path (already
  covered by the translator). The done drain in `Engine` collapses to the single
  `apply_done_action` call.
- **Client mirror.** `defs.ugens.DoneAction` carries the full 0-15 enum;
  `Server.run`/`pause`/`resume` emit `/n_run` (accept a node object or a bare id,
  so a whole group works). Docstrings drive the generated `api.md`.

**Docs:** `schemas.md` gains the `/n_run` section and the full 0-15 done-action
table (was 0/1/2/14) and drops the "no `/n_run` to resume it yet" note;
`architecture.md` explains the sibling resolution and the paused-group skip;
`GUIA.md` gets the S4 manual-test section (and the summary-table row) and the
EnvGen note is corrected. A user-facing `pause_resume.py` example renders a
drone paused for a beat and resumed (RMS on/paused/resumed = 0.141/0.000/0.141).

**Scope note:** the relative actions act within a node's **parent group** (the
ordered child list); "previous/next" is sibling order, matching scsynth. Groups
are pausable now, which `/n_run` on a group exercises.

**Verified:** `cargo test --no-default-features` green — 10 `node` unit tests
(each relative action's tree effect, `set_paused` round-trip + unknown-id
reject, sibling resolution at the edges), `tests/envgen.rs` (`pauseSelf` parked
then `/n_run 1` audible again; `freeSelfAndNext` through the real
float→`from_i32`→queue→`apply_done_action` chain), `tests/osc.rs::n_run_...`
(the OSC dispatch: a valid `/n_run` does not `/fail`, an unknown id does — this
guards the whitelist wiring), and a no-alloc `relative_done_actions_and_n_run`
scene in `tests/rt_safety.rs`; `cargo fmt` clean. Client: `clients/python` suite
green (`/n_run` emission + the full enum) and the `pause_resume.py` E2E offline
render through the rebuilt embed lib.

## S5 — Wavetable & table-generation infrastructure (`/b_gen`) + the table oscillators (completed 2026-07-03)

**What's there:** the buffer-generation command `/b_gen`, scsynth's wavetable
format, and the first UGens that consume it — so a client can synthesize a
table (a harmonic spectrum, a waveshaping curve) into a server buffer and read
it with an oscillator, all without a Faust def.

- **The wavetable format + generators (`src/dsp/wavetable.rs`, new).** Pure and
  off the audio thread. `signal_to_wavetable(signal, wrap)` builds scsynth's
  interleaved layout — per point `[2·a[i]−a[i+1], a[i+1]−a[i]]` — so an
  interpolating read is one fused multiply-add (`wt_interp`: `x0 + (1+frac)·x1`,
  no branch). `GenFlags` unpacks `normalize`(1)/`wavetable`(2)/`clear`(4);
  `GenCommand` is one parsed command — `Sine1` (harmonic amps), `Sine2`
  (freq/amp), `Sine3` (freq/amp/phase), `Cheby` (a `Σ ampₖ·T_{k+1}(x)` transfer
  curve, non-wrapping), `Copy` (overlay another buffer). `GenCommand::apply`
  renders one period, optionally normalizes/accumulates, wavetable-encodes, and
  returns a same-shape immutable `Buffer`.
- **Table oscillators (`src/dsp/osc.rs`, new).** `Osc` (interpolating wavetable
  oscillator), `OscN` (non-interpolating, plain buffer), `VOsc` (buffer number
  is a signal, crossfading adjacent tables), `Shaper` (waveshaper over a `cheby`
  transfer table, input in ±1 spanning the table). All mono, single-output, read
  the immutable pool through `ctx.buffers` like `PlayBuf` — no allocation.
  Registered as four rows in `src/dsp/registry.rs`; no other engine change.
- **`/b_gen` on the NRT queue.** New `NrtJob::Gen { current, cmd }` (`run_job`
  calls `cmd.apply`), so generation rides the same one-queue, submission-order,
  build-and-swap path as `/b_read`/`/b_zero` — the audio thread only ever sees a
  finished buffer. `parse_b_gen` (`src/osc/translate.rs`) resolves the command
  (and, for `copy`, the source buffer) from the network-side mirror; like
  `/b_read` it needs the target already allocated (so a `/b_gen` after a
  `/b_alloc` waits on the alloc's `/done`). Wired into the live dispatch
  (`src/osc/server.rs`, `handle_b_gen` + a shared `submit_nrt`) and the NRT
  renderer (`src/server/render.rs`), so offline scores generate tables too.

**Docs:** `schemas.md` gains the `/b_gen` command family, the wavetable-format
explanation, and the `Osc`/`OscN`/`VOsc`/`Shaper` catalog rows, cross-linked
both ways with the Faust `waveform` table (the same precompute-a-table idea at
def scale); `architecture.md`'s buffer note lists `/b_gen` as a build-and-swap
"mutation"; `GUIA.md` gets the S5 manual-test section. The `bgen` demo in
`examples/json_client.py` fills buffers with `sine1` and `cheby` and plays them
through `Osc` and `Shaper`.

**Verified:** `cargo test --no-default-features` green — `tests/wavetable.rs`
(the format round-trip reconstructs a sine; `sine1` wavetable vs a computed sine
within tolerance; the `cheby` `T₂` transfer curve; `copy` overlay; no-clear
accumulation; `Osc` renders a sine and `Shaper` with a linear `cheby [1]` passes
its input through — both through the real engine), `tests/osc.rs::b_gen_...`
(full OSC round-trip: `/b_alloc` → `/b_gen sine1` → `/b_getn` reconstructs the
sine; unknown command and unallocated target `/fail`), and a no-alloc
`table_oscillators` scene in `tests/rt_safety.rs` (`Osc`/`VOsc`/`Shaper` reading
the pool). `cargo fmt` clean. Live smoke: the `bgen` demo against a running
server plays the wavetable and waveshaper voices.

## S6 — Complete the scsynth OSC command set (completed 2026-07-03)

**What's there:** the rest of scsynth's OSC vocabulary, so a client can rely on
the full command surface. Every command is network-thread and RT-safe like its
neighbours, mapping onto the existing tree/bus/def/schedule machinery — the
audio thread learns two new `Cmd`s (`ClearSched`, `UGenCommand`) and one new
`Place` axis (head/tail), nothing more.

- **Node ranges (`src/osc/translate.rs`).** `/n_setn` (a consecutive control
  range from a value list, repeatable groups), `/n_fill` (fill a range with one
  value), `/n_mapn`/`/n_mapan` (map consecutive controls to consecutive buses,
  `-1` unbinds the range). All expand to the existing `SetControl`/`MapControl`
  per index, reuse the group-subtree propagation and the bus-control re-sort,
  and are schedulable in timed bundles.
- **Tree moves.** `Place` (`src/node/mod.rs`) grows `Head`/`Tail` beside
  `Before`/`After`; `NodeTree::move_node` and its `TreeMirror` twin resolve the
  destination group from the variant (the target's parent for sibling moves, the
  target itself for head/tail). `/g_head`/`/g_tail` move a node to a group's
  head/tail; `/n_order addAction target id...` moves several nodes to one place
  keeping their listed order (first relative to the target, the rest chained
  `After` the previous). A shared `move_one` rejects any manual move into an
  auto-sorted group (`/g_sortMode`) with `/fail`.
- **Control-bus ranges (`src/osc/server.rs`, `translate.rs`).** `/c_setn`,
  `/c_fill` (immediate atomic writes, or `SetControlBus` per bus in a bundle),
  `/c_getn` (reply `/c_setn bus numBuses val...`).
- **Synth queries.** `/s_get`/`/s_getn` read the node mirror's control values
  and reply `/n_set` (the read counterpart of `/n_set`); `/s_noid` is a
  compatibility acknowledgement (Clausters assigns node IDs per client and never
  reuses a live/freed one, so there is nothing to release); `/n_trace` logs a
  node's mirror state to the console.
- **Buffers/defs.** `/b_close` validates a live buffer and replies `/done`
  (forward-looking for the future streaming UGens — no soundfile is ever left
  open today); `/d_load path` / `/d_loadDir dir` load SynthDef spec JSON files
  from disk on demand through the `/d_recv` path (persisting under their names).
- **Scheduling.** `Cmd::ClearSched` (`/clearSched`) `drain`s the engine's
  timed-bundle queue (keeping its capacity), shipping each bundle's `Vec<Cmd>`
  and boxed synths back as `Garbage::SpentBundle` — flush the schedule without a
  drop on the audio thread.
- **Server/UGen commands + `/error`.** `/error mode` gates console error posting
  (the `/fail` OSC reply always goes out; scsynth's bundle-local `-1`/`-2` are a
  documented deviation — `0`/`1` is the model that fits our logging). `/cmd name
  args...` is a typed, discoverable server command (built-in `ping`); `/u_cmd
  nodeID ugenIndex name args...` addresses one UGen instance — it validates the
  target, hashes the name to a stable selector (`dsp::ugen_cmd_selector`), packs
  the numeric args inline (`UGenCmd`, up to 8 floats, no heap across the FIFO),
  and `Cmd::UGenCommand` routes them to `SynthNode::ugen_command` →
  `UGen::command` (default no-op — the mechanism for future FFT/streaming UGens,
  the typed replacement for scsynth's untyped `/u_cmd` blob).

**Docs:** `schemas.md` gains the node-range, tree-move, control-bus-range,
synth-query, `/b_close`, `/d_load`/`/d_loadDir`, `/clearSched`, `/error`, and
`/cmd`/`/u_cmd` sections, and updates the schedulable-in-bundle list;
`architecture.md` adds "Node moves and the auto-order guard", "Range and query
commands", the `UGen::command` hook in "how to add a UGen", and the
`/clearSched` garbage path in "Clocks and scheduling". `GUIA.md` gets the S6
manual-test section and a checklist row; the `commands` demo in
`examples/json_client.py` exercises `/n_setn`, `/s_getn`, `/g_head`, `/c_setn`,
`/cmd`, and `/clearSched`.

**Verified:** `cargo test --no-default-features` green — 18 new `tests/osc.rs`
cases (`n_setn`/`s_get`/`s_getn`, `n_fill`, `n_mapn`, `g_head`/`g_tail`/`n_order`
ordering via `/g_queryTree` + auto-sort rejection, `c_setn`/`c_getn`/`c_fill`,
`s_noid`, `b_close`, `d_load` + missing file, `clearSched` proving a flushed
bundle never fires, `error_mode` still replying `/fail`, `cmd` ping/unknown,
`u_cmd` target/index validation) plus a no-alloc `command_set_completion` scene
in `tests/rt_safety.rs` (`MoveNode` head/tail, `UGenCommand`, `ClearSched`).
`cargo fmt`/`clippy` clean. Live smoke: `json_client.py commands` against a
running server round-trips every command (`/s_getn` returns the range,
`/g_head` reorders, `/c_getn` reads back, `/cmd ping` and `/clearSched` reply
`/done`).

## S7 — Boot-time server configuration (audio I/O channels + every pre-allocated pool) (completed 2026-07-03)

**What's there:** every operational size the server used to hard-code is now
chosen **at boot** — at runtime, never at compile time — through the same
config-file → flag precedence as the other options. Before S7 only the buses
were configurable; now so are the four pre-allocated pools and the hardware I/O
channel counts, and the server finally has a real audio-input path.

- **The `Limits` type (`src/dsp/mod.rs`).** A small POD (`max_nodes`,
  `max_buffers`, `max_group_children`, `max_ugen_inputs`) with `Default` (the
  historical 1024/1024/256/32) and `clamped()`. It threads
  `engine_pair_full(…, limits)` → `Engine` (which sizes `NodeTree::with_capacity`
  and `empty_pool_with`) and `EngineHandle.limits`, and into
  `CmdTranslator::with_limits` on the network side. `NodeTree::with_capacity` and
  `Group::with_capacity`/`empty_pool_with` size every slab (the node slab, the
  DFS/free stacks, the done-action queue, group child lists, the buffer pool)
  from it; the old `MAX_NODES`/`MAX_GROUP_CHILDREN`/`NUM_BUFFERS` consts remain as
  the documented defaults. The done-queue clamps now key off `done_nodes.len()`,
  and the buffer-index bound is `mirror.len()`, so the whole thing is one number
  per pool with no scattered constants.
- **UGen-input limit.** `--max-ugen-inputs` is enforced in `d_recv` after
  `compile` (rejecting a def whose UGen exceeds it with `/fail`, the offending
  index in the message). Its ceiling stays the compile-time `MAX_UGEN_INPUTS`
  (32): the per-UGen input list is a stack array in `synthdef::instance`, so like
  audio buses at 128 (the `BusUsage` `u128` mask) the runtime knob can only make
  it *stricter*, a documented deviation from a fully dynamic size — everything
  else (nodes, buffers, group children) is a genuinely resized heap `Vec`.
- **Audio I/O channels + live input (`src/server/backend.rs`).** `start` gains
  `limits`, `outputs: Option<usize>` and `inputs: usize`. Output channels are
  negotiated with the host (requested count first, device default as fallback,
  same shape as the sample-rate fallback). With `inputs > 0` a **second cpal
  stream** on the default input device pushes decoded interleaved f32 frames into
  a lock-free ring; `Engine::process_block` pops one block's worth into audio
  buses `outputs..outputs+inputs` at block start (before any node runs), so `In`
  reads live device input like any bus. The two streams are decoupled by the
  ring — overrun drops on the callback side, underrun reads silence on the engine
  side, neither blocks. An unavailable input device leaves the server
  output-only (logged, not fatal). `Engine::attach_input`/`input_ring` are the
  seam; the input callback re-arms flush-to-zero like the output one.
- **Flags & config.** `src/main.rs` parses `--outputs`, `--inputs`,
  `--max-nodes`, `--max-buffers`, `--max-graph-children`, `--max-ugen-inputs`
  (with `[server]` keys `outputs`/`inputs`/`max_nodes`/… in
  `clausters-core::config`, merged and defaulted the usual way); `--help` and the
  startup line report out/in channels.
- **Discovery.** `/server_info.reply` appends `input_channels`, `max_nodes`,
  `max_buffers`, `max_graph_children`, `max_ugen_inputs` after the original six
  fields (stable prefix, so older clients that read six keep working).

**Docs:** `docs/architecture.md` — the M10 capacity table gains a boot-flag
column and a paragraph on what is boot-sized vs. ceiling-capped, plus an "audio
input thread" bullet in the thread model. `docs/schemas.md` — the engine-facts
paragraph lists every boot flag and the extended `/server_info` reply, and a new
"Live audio input" note. `GUIA.md` — an S7 manual-test section and a checklist
row. `examples/json_client.py` — a `serverinfo` demo that prints the eleven
`/server_info` fields and, if the server was booted with `--inputs`, plays an
`In → Out` passthrough of the live input.

**Verified:** `cargo test --no-default-features` green. New `tests/audio_io.rs`
(3: `In` reads the ring same-block with no latency, no-input → silence, underrun
→ silence without blocking); `tests/capacity.rs` extended to 7 (a small
`--max-nodes` and a small `--max-graph-children` overflow exactly at capacity
alongside the defaults); `tests/osc.rs` +2 (`/server_info` reports the
configured limits; `/d_recv` rejects a def over `--max-ugen-inputs`);
`tests/rt_safety.rs` +1 (the input ring pop inside `process_block` allocates
nothing). `crates/clausters-core` config test covers the new keys. `cargo fmt`
clean; no new clippy warnings. The full-binary path (a second cpal input stream)
can't run in the sandbox — no audio device, and the fixed UDP port collides with
a running instance — so it is exercised at the engine seam instead; the
`serverinfo` demo is the live smoke.

## S8 — FFT/IFFT and the spectral (`fr`) chain (completed 2026-07-03)

**What's there:** scsynth-style frequency-domain processing — an `FFT` windows an
audio input and transforms it to a spectral frame once per **hop**, a chain of
`PV_*` UGens mutates that frame, and an `IFFT` inverse-transforms and overlap-adds
it back to audio. The chain is **frame-rate (`fr`)**: `FFT`/`PV_*` are control
rate and only work on the blocks a fresh frame is ready; `IFFT` is audio rate.

- **Shared transform + windows in `clausters-core`.** The user flagged that
  `microfft` (already a core dep, `no_std`, zero-allocation) surprisingly *does*
  do inverse FFT — its `inverse::ifft_*` (`microfft` 0.6, normalized by `1/N`);
  the old "forward-only" note in `fft.rs` was stale. So no new dependency: `fft.rs`
  gains `rfft_into` (forward, packing the scsynth frame layout `[dc, nyquist, re₁,
  im₁, …]`) and `irfft_into` (rebuild the Hermitian spectrum from the half-frame,
  run `ifft_*`, take the real part) — both zero-allocation (stack scratch). A new
  `window` module holds the smoothing windows (Hann/Sine/Welch/Hamming/Blackman/
  rectangular, periodic) **shared with the clients** for bit-identical analysis, as
  the user asked. Core tests: forward↔inverse round trip, the DC/Nyquist packing,
  window shapes (`sine² == hann`).
- **Where the frame lives — a documented deviation from scsynth.** scsynth mutates
  a client-allocated buffer in place on the audio thread, which would break
  Clausters' immutable-sample-buffer invariant. Decided with the user: the frame
  lives in **synth-private scratch** (`dsp::spectral::SpectralChain`: the packed
  frame + a `ready` flag + the hop `advance` + winsize), allocated when the synth
  is instantiated and freed with it — exactly the `LocalIn`/`LocalOut` `locals`
  pattern, the moral equivalent of SuperCollider's `LocalBuf`. **No `/b_alloc` is
  required** and the sample pool stays fully immutable.
- **The UGens (`src/dsp/spectral.rs`).** `Fft` keeps a sliding input ring + an
  analysis window and emits one packed frame per hop (quantized up to the
  processing slice, as scsynth transforms at block granularity), carrying the hop
  `advance` on the chain. `Ifft` keeps an overlap-add tail (with a parallel
  window-energy accumulator) and an output FIFO; it inverse-transforms each fresh
  frame, overlap-adds it **window-normalized** (÷ a precomputed steady-state COLA
  denominator `Σ window[phase+i·hop]²` per hop phase, so a bare `FFT`→`IFFT`
  reconstructs at unity gain with one window of latency and a modified frame does
  not over-amplify the low-window edges), and drains the FIFO per slice — keeping
  analysis/resynthesis in lockstep via the `advance` regardless of the hop/block
  relationship. `PvMag` (`PV_MagAbove`/
  `PV_MagBelow`) thresholds bin magnitudes; `PvBrickWall` zeroes a band. All reuse
  pre-allocated scratch — nothing allocates on the audio thread.
- **Wiring.** New `ExecMode::Spectral` + a `SpectralRole` (`Source`/`Filter`/
  `Sink`) descriptor field; `UGen::process_spectral(ctx, inputs, output, chain)`
  (default no-op). The compiler assigns a fresh chain **slot** to each `FFT`
  (recorded in `SynthDef::spectral_sizes`) and makes each `PV_*`/`IFFT` inherit its
  upstream's slot, window size and window type by following input 0 — so the size
  is given only on the `FFT`. `UGenConfig`/`UGenSpec` gain `fft_size`/`hop`/
  `wintype`. `UGenSynth` allocates one `SpectralChain` per slot and special-cases
  `ExecMode::Spectral` (borrowing `chains[slot]` and `ugens[i]` — distinct fields —
  mutably at once). Bad sizes and non-chain inputs fail `compile` with a pointed
  error.
- **`/u_cmd` — first real consumer of the S6 surface.** `FFT`/`IFFT` `command()`
  handle the `window` selector, swapping the analysis/synthesis window live off any
  hop (`/u_cmd <node> <ugenIndex> window <wintype>`), validating that S6's typed
  per-UGen command mechanism works end to end.
- **Registry rows:** `FFT`, `IFFT`, `PV_MagAbove`, `PV_MagBelow`, `PV_BrickWall`.
- **Tests:** `tests/spectral.rs` (FFT→IFFT reconstructs a tone within tolerance;
  `PV_BrickWall`/`PV_MagAbove` attenuate a band; compiler validation; a `/u_cmd`
  window swap), `tests/rt_safety.rs` (`spectral_chain_does_not_allocate...`, a full
  `SinOsc→FFT→PV_BrickWall→PV_MagAbove→IFFT→Out` scene crossing hop boundaries),
  the core `fft`/`window` unit tests. Docs: `schemas.md` (catalog rows + the FFT
  chain note + the synth-private-scratch deviation + the `/u_cmd window` surface),
  `architecture.md` (the `fr` chain section + the "how to add a UGen" spectral
  note), a `GUIA.md` S8 section + row, the `fft` demo in `examples/json_client.py`
  (E2E-verified: `/done`, spectral low-passed noise, live window swap).

## S9 — Side-effect UGens (SendReply/SendTrig/Poll), no `Out` required (completed 2026-07-03)

**What's there:** a family of UGens whose purpose is a **side effect** — an OSC
reply or a console post — rather than audio on a bus, plus the client relaxation
(C19) that lets a def consist only of them, with no `Out` at all. The server
already permitted output-less defs (`compile` requires ≥1 UGen, never an `Out`);
S9 adds the UGens that make that useful and the RT-safe path their replies take
out of the audio thread.

- **The reply message type (`src/dsp/mod.rs`).** `ReplyMsg` is a fully inline,
  `Copy` POD — a fixed 31-byte name buffer (`SendReply` command name / `Poll`
  label) plus a 16-slot value array and a `ReplyKind` (`Trig`/`Reply`/`Poll`) —
  so buffering and shipping one never allocates. `REPLY_BUFFER_LEN` (8) caps the
  per-block per-UGen buffer. The `UGen` trait gains `is_reply()` (default false)
  and `drain_replies(node_id, sink)` (default no-op); `SynthNode` gains
  `has_replies()`/`drain_replies()`.
- **The UGens (`src/dsp/reply.rs`).** `SendTrig(in, id, value)`,
  `SendReply(trig, replyID, values…)` and `Poll(trig, in, trigid)`. Each detects
  a trigger (a crossing from `≤ 0` to `> 0`) frame-by-frame and buffers a
  `ReplyMsg` per crossing in a fixed inline `ReplyBuffer`; `Poll` also passes its
  `in` signal through so it can sit mid-chain. The command name / label lives in
  the UGen as a `String` built on the network thread and only *read* while
  processing. Registered as three `UGENS` rows (kr default, kr/ar allowed,
  `BusRole::None`); the name/label rides a new `label` field on `UGenSpec`/
  `UGenConfig`, wired through `compile` like `op`/`path`.
- **The RT-safe reply path (`src/node/mod.rs`, `src/server/engine.rs`).** Same
  discipline as the done-action queue: during the walk the tree marks any synth
  that ran a reply UGen into a **lock-free slot queue** (`reply_slots`/
  `reply_count`, `fetch_add` reservation so the M13 workers can push while
  holding their slot); `Engine::process_block` drains it **once after the whole
  block** (`NodeTree::drain_replies` stamps each message with the node id and
  pushes it into a new SPSC **reply FIFO**, capacity 2048, best-effort drop when
  full). `UGenSynth` precomputes `has_reply_ugens` so a synth without reply UGens
  is never marked or drained. A synth re-marked by a mid-block schedule split is
  harmless (the first drain empties its buffer).
- **OSC emission (`src/osc/server.rs`).** `collect_garbage` drains the reply FIFO
  after the node events: `Trig` → `/tr nodeID id value` to every `/notify`
  client, `Reply` → the custom address with `nodeID replyID value…`, `Poll` → a
  `tracing` console line (network thread, never the audio thread) plus a `/tr`
  when its trigid is non-negative.
- **Client (C19, `clients/python`).** `SynthDef` no longer requires an output
  UGen — it takes graph **roots**, which may be side-effect UGens; the error and
  docstring say so. New `send_trig`/`send_reply`/`poll` builders (with a `label`
  field on `Ugen`, serialized like `op`), exported from `clausters.defs`.

**Docs:** `docs/schemas.md` — three catalog rows and a "Side-effect UGens" note
(trigger semantics, the FIFO discipline, `/notify` delivery). `docs/architecture.md`
— a "Side-effect replies" subsection under the memory-lifecycle section, a
reply-FIFO row in the capacity table, and an `is_reply`/`drain_replies` note in
"How to add a UGen". `GUIA.md` — an S9 manual-test section and a checklist row.
`examples/json_client.py` — a `replies` demo (an output-less def, `/notify`, a
fired trigger control, prints the `/tr` and custom replies).

**Verified:** full `cargo test` green. `tests/osc.rs` +3 (`send_trig_replies...`
asserts `/tr` id+value on an `Out`-less def; `send_reply_replies_at_custom_address`
round-trips the value list at the custom address; `poll_with_trigid_replies_tr`).
`tests/rt_safety.rs` +1 (`reply_ugens_do_not_allocate_on_the_audio_thread`: an
Impulse fires all three every block, the FIFO fills and drops — no allocation).
`clients/python/tests/test_synthdef.py` +1 (`test_side_effect_ugens_need_no_out`).
`cargo fmt` clean; no new clippy warnings. Live E2E: `json_client.py replies`
against a running server prints `/tr [3200, 7, 0.5]` and `/custom [3200, 42, 1.5,
2.5]`.

## Build features — independent def families: `synth` (SynthDef) / `faust` (FaustDef) (completed 2026-07-04)

**Goal:** let custom builds ship either def family alone or both. Previously
the SynthDef/UGen family was unconditional and only Faust was a feature; now
`synth` (default) and `faust` are symmetrical, freely combinable Cargo
features, and the engine core still builds with neither.

**What moved behind `synth`:** the `synthdef` module (`compile`, `UGenSynth`,
the built-in `default` def), the whole UGen library in `dsp` (every UGen
submodule plus the registry; the shared core — `Block`/`Buses`/`ProcessCtx`/
`DoneAction`/`ReplyMsg`/`UGenCmd`, `dsp::buffer`, `dsp::denormals`, and
`dsp::wavetable` for `/b_gen` — stays unconditional), `ugen_usage` in
`osc::graph`, and the translator/server paths: the `synth_defs` table, the
`NodeDef::UGen` variant, `/d_recv` (a `not(synth)` stub replies `/fail
"server built without synthdef support"`, which also covers `/d_load`,
`/d_loadDir`, NRT scores and the persisted-def reload — the latter warns once
at boot, mirroring the missing-`faust` courtesy). `NodeDef` reduces to an
empty enum with neither family; each match keeps a diverging
`_ => match *self {}` arm for that case. The node tree was already generic
over `dyn SynthNode`, so the engine is untouched.

**Tests/examples:** the UGen-driven integration suites are gated
`#![cfg(feature = "synth")]` (the featureless run keeps the lib unit tests,
denormals, and the faust suites stay on `faust`; `faust_parity` needs
`all(faust, synth)`; the mixed test in `faust_synth` gates individually).
`examples/bench.rs` gets `required-features = ["synth"]`.

**Docs:** feature matrix + variants in `BUILD.md` (single-family build lines),
`README.md`, `docs/using-as-a-library.md`, `docs/schemas.md` (availability
column + `/fail` behavior), `docs/introduction.md`, `docs/getting-started.md`,
`docs/contributing.md`, `docs/architecture.md` invariant 7 (build with any
def-family combination), and the def-family section in `CLAUDE.md`.

**Verified:** `cargo check --all-targets` clean on all five combos (none,
`synth`-only, `faust`-only, default, default+`faust`+`embed`; faust combos
type-check only on this machine — libfaust not installed, so no link/run).
`cargo test` 286 passed; `--no-default-features --features synth` identical;
`--no-default-features` 21 passed. `cargo fmt --check` clean; clippy adds no
new warnings. Live E2E: default binary plays `vibrato`; a core-only binary
(`--features realtime`) boots, answers `/status`, `/fail`s `/d_recv` with the
feature message and warns once about skipped persisted SynthDefs.

## M23 — Continuous integration + publishing (CI, Read the Docs, PyPI) (completed 2026-07-05)

**Goal:** automate the checks the project already required by hand and set up
the publishing pipeline for what is finished. Repo side complete; three
account-side activation steps remain manual (listed at the end).

**CI** (`.github/workflows/ci.yml`, six jobs, each mirroring a local command):
`lint` — `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D
warnings` for the root workspace and for `clients/gui` (its own workspace);
`test` — `cargo test --workspace` over the def-family matrix (default /
`--no-default-features` / `synth`-only / default+`embed`); `gui` — the gui
tests plus the G11 wasm build gate (`check-wasm.sh`); `python` —
`build_native.py --debug` staging + pytest; `docs` — both mdBooks with
mdBook 0.4.40 (the Read the Docs version) plus the pydoc-markdown API page;
`faust` — libfaust built from source pinned to the `BUILD-FAUST.md`-verified
commit `56c9e678d` (shallow SHA fetch + submodules, the dynamic-libLLVM
recipe), the `~/.local` install cached by SHA, then `cargo test --features
faust -- --test-threads=1`.

To make the clippy gate strict, the handful of pre-existing warnings was
fixed: `translate.rs` (`iter().copied()` instead of a snapshot `to_vec` —
`collect_subtree_synths` takes `&self`, so the borrow comment was stale; a
`GraphBusAlloc` type alias for the bus-allocation tuple), `server.rs`
(`tempo.is_nan() || tempo <= 0.0` replacing `!(tempo > 0.0)`, same semantics
spelled positively) and `tests/graphdef.rs` (`!contains_key` for
`get().is_none()`). Behavior unchanged; the affected suites re-run green.

**Release** (`.github/workflows/release.yml`): a `v*` tag builds the
self-contained wheel (release profile; Linux x86_64) and a
`clausters-server-<version>-linux-x86_64.tar.gz`, publishes the wheel to PyPI
via Trusted Publishing (OIDC, GitHub environment `pypi`, no stored token) and
attaches both to a GitHub release. Deliberately no sdist: the package
compiles cdylibs from the Rust workspace, which an sdist of `clients/python`
would not contain. macOS/Windows wheels are a later matrix extension.

**Docs:** `docs/contributing.md` — new "Continuous integration" and
"Releases and publishing" sections (job↔command map; the
two-Read-the-Docs-projects setup, each project pointing at its own
`.readthedocs.yaml` via *Path to configuration file*). `GUIA.md` — section
3ter (the local command block, the tag flow, the three activation steps) and
a checklist row.

**Verified locally** (the CI commands themselves): `cargo fmt --check` clean
(root + gui); clippy zero warnings workspace-wide and in gui;
`tests/graphdef.rs` + `tests/osc.rs` re-run green after the lint fixes (49
passed); both books build with the exact CI/RTD mdBook 0.4.40 (server book +
pydoc-markdown regeneration + client book); `build_native.py --debug` stages
the three artifacts; client pytest 128 passed / 4 skipped. Workflow YAML
parses. Not verifiable from here: the GitHub runners themselves (first push
shows; the `faust` job is the one most likely to need a first-run tweak —
Ubuntu 24.04 ships LLVM 18 via `llvm-dev`, vs 20/21 verified locally).

**Manual activation steps (account side, pending):** (1) create the two Read
the Docs projects (root `.readthedocs.yaml` → slug `clausters`;
`clients/python/.readthedocs.yaml` → slug `clausters-python`); (2) PyPI
trusted publisher for `clausters` (owner `smrg-lm`, repo `clausters`,
workflow `release.yml`, environment `pypi`; a *pending publisher* works
before the first upload); (3) the `pypi` environment in the GitHub repo
settings.

**First-run outcome (2026-07-05, activation done):** CI green on all nine
jobs after two predicted first-run fixes in the `faust` job — the pinned
fetch needs the **full 40-char SHA** (fetching an arbitrary commit by
abbreviated SHA fails with `couldn't find remote ref`), and
`-DINCLUDE_STATIC=off` (CMAKEOPT overrides the FORCEd cache) skips the
static `libfaustwithllvm.a`, which embeds LLVM's static component libs and
needs libPolly, absent from Ubuntu 24.04's `llvm-dev`; the dynamic
`libfaust.so` clausters links builds fine on the runner's LLVM 18.1.3 and
the faust suites pass, `~/.local` install cached. The `v0.1.0` release run
then exposed one wheel fix: PyPI rejects the bare `linux_x86_64` platform
tag, so `setup.py` maps it to `manylinux_<glibc>` from the build machine's
own `CS_GNU_LIBC_VERSION` (PEP 600's honest bound: the cdylibs run on the
glibc they were built against; musl/non-Linux tags pass through). Trusted
publishing itself validated on the first attempt (OIDC token exchanged, the
failure was the tag, not auth).

## G14 — Browser meters/scopes: control buses over the wire (2026-07-05)

The browser host's meters, scopes and `canvas` bus parameters now read **live
control buses streamed over WebSocket** — the message-based fill of the
`BusSource` seam for the one client that cannot map the shared segment. The
server side is a new command designed once for two consumers (this host and the
future TS client, W4).

- **Server: `/c_stream periodMs bus...` (src/osc/server.rs).** One subscription
  per `ClientId` (transport-agnostic: UDP/TCP/WS/ring), replaced on every call;
  acks `/done`, sends one `/c_set (bus value)...` snapshot immediately and then
  one per period. Period clamped to a 10 ms floor, ≤128 buses (`/fail` beyond),
  `periodMs <= 0` or an empty list cancels; not schedulable in timed bundles.
  Reading a bus is one relaxed atomic load on the network thread — zero RT
  involvement. Cadence rides the run loop: `pump_streams()` each iteration, and
  `retune_timeout()` shortens the socket timeout (the loop's idle tick) to the
  fastest subscribed period (the 2 ms IPC poll wins unconditionally).
- **Disconnect pruning (src/osc/ws.rs, tcp.rs, server.rs).** The hubs now
  surface closed connections (`take_disconnects`); the loop drops their bus
  streams **and their `/notify` registrations** — fixing a pre-existing leak
  where dead WS/TCP clients kept receiving notifications forever. UDP/ring
  subscriptions last until explicit cancel or `/quit` (the `/notify` posture,
  documented). Reply shape reuses `/c_set` (the query→setter convention), so
  every existing client decodes the stream for free. Documented in
  `docs/schemas.md` (control-bus section, "Beyond scsynth").
- **Shared live-bus logic: `host::live` (clients/gui).** `SCOPE_HISTORY`, the
  scope collectors, `advance_scope_histories`, `collect_live_buses`
  (meter/scope buses + a canvas's non-negative `buses`, deduped/sorted) and
  `StreamedBuses` (a `Mutex<HashMap>` `BusSource` — uncontended on the
  single-threaded wasm runtime) moved out of the native front; native
  `advance_scopes` now delegates, behavior identical.
- **Web front (host::web).** `WsServerLink` gained the inbound leg (an
  `onmessage` closure → `WebEvent::ServerInbound` → the one `decode_packet`
  door) — it was send-only since G13. The host derives the subscription from
  the tree (`sync_bus_stream`, re-sent only when the bus set changes; re-run on
  open/close/`/gui_set` and after `connect_server`) and runs a 33 ms
  `setInterval` animation tick (`std::time::Instant` does not exist on
  wasm32) that advances the scope histories exactly like the native tick.
  `FrameInputs.bus` now carries the streamed source; the meter/scope/canvas
  drawing is reused unchanged.
- **Python:** `Server.stream_buses(period_ms, *buses)` wraps the command
  (unit test in `tests/test_defs.py`).
- **Verified:** 3 new integration tests in `tests/osc.rs` (ack + immediate
  snapshot + periodic frames tracking a write; resubscribe-replaces and
  cancel-stops; argument validation), root suite green across the feature
  matrix; E2E over WebSocket through the Python client (server + client in one
  invocation, streamed `/c_set` frames observed, cancel silences). gui: 88
  tests (81 + live/fetch units), `clippy -D warnings` clean native and wasm32.
  Headless-Chrome parity run shows `/c_stream subscription: [10]` and
  `bus stream flowing: [Int(10), Float(0.5)]` — the value a Python client wrote
  arriving in the browser (see G16).

## G15 — Browser bulk data: fetch/blob and the /b_getn fallback (2026-07-05)

The browser host's bulk paths: a waveform/plot `path`/`cache` resolves as a
**URL fetched against the page origin**, and a server `buffer` reference is
pulled over **`/b_query` + chunked `/b_getn` on the WebSocket leg** — the
"async fallback" the G7 bulk decision reserved for exactly this client. The
`Pyramid`/`WaveformData` consumers and the analysis are reused as-is; the peak
pyramid for raw fetches is built **in wasm** (`clausters_core::peaks`,
FFI-free).

- **Shared fetch machine: `host::fetch` (`BufferFetches`).** The native
  buffer-fetch state machine (G5: `/b_query` → `/b_info` → sequential 8192-
  sample `/b_getn` chunks reassembled by explicit `start` → channel-0
  de-interleave) extracted verbatim from the windowed front into a pure,
  platform-agnostic module returning `FetchStep::{Request, Done, None}`; both
  fronts drive it (native `App` and `WebApp`), and it is unit-tested without a
  GPU or socket (chunk walk, multichannel de-interleave, empty buffer,
  mid-download window close, unsolicited replies).
- **Web fetch path (host::web).** `collect_bulk` mirrors the native
  `collect_waveforms`/`load_plot_paths` resolution: inline data builds slots
  immediately; `cache` fetches a prebuilt peak pyramid (`Pyramid::from_bytes`,
  raw samples never loaded); `path` fetches raw little-endian `f32` and
  de-interleaves channel 0 (`decode_channel0`, the browser twin of
  `MappedFile::channel0_f32`); `buffer` goes through the shared machine over
  WS. Fetches run on `wasm_bindgen_futures::spawn_local` → `WebEvent::BulkReady`;
  waveforms that finish before the GPU is up are stashed and replayed on
  `GpuReady`; plot samples land in the host tree (no GPU needed). The sync
  `BulkLoader` trait stays native-only by design (fetch cannot block).
- **`Cargo.toml`:** web-sys gains `MessageEvent` (the G14 inbound leg) and
  `Response` (fetch); nothing new natively.
- **Verified:** gui 88 tests green (4 are the fetch-machine units), clippy
  `-D warnings` native + wasm32, `check-wasm.sh` green. Headless-Chrome parity
  run (G16) shows all three bulk paths against a real `--ws` server:
  `fetched peak cache sine.peaks (48000 samples, no raw data)`,
  `fetched 48000 samples from sine.f32 (pyramid built in wasm)`, and
  `buffer 0: 48000 frames loaded into 1 waveform(s)` (six `/b_getn` chunks over
  WebSocket).

## G16 — Packaging and native/browser parity (2026-07-05)

The wasm GUI host is shippable and the reuse claim is proven end-to-end. This
packages the **host**, not a client: the product TypeScript client remains the
separate `clients/web` track (its W4 now notes `/c_stream` is already served).

- **Packaging: `web/build.sh` + wasm-bindgen CLI stays the shipping path** (no
  wasm-pack/trunk — they add nothing over a `start()` + `GuiBridge` surface and
  the CLI version is pinned by Cargo.lock). `web/index.html` is now the
  documented harness: a server-URL field + connect, and **panel / meters /
  bulk** demo buttons feeding the same GuiDef JSONs the Python examples build
  (the meters demo is a self-contained loop: a knob bound to `/c_set 10` drives
  the meter/scope through the server's stream — no script needed).
- **Scripted parity pass: `web/parity.html`.** Auto-connects and opens the
  three demos in sequence; the evidence is the host's own console log, so the
  pass runs headless (`google-chrome --headless=new --enable-unsafe-swiftshader
  --enable-logging=stderr`, WebGL2 over SwiftShader). Full pass verified
  against a live `--ws` server with buffer 0 filled by `/b_gen` and the bulk
  files served next to the page: three `window opened from the page`,
  `audio-server WebSocket open`, the G14 stream lines, the three G15 bulk
  lines, `parity: pass complete`. Native reference: the same GuiDefs through
  `gui_panel.py`/`gui_meters.py`/`gui_bulk.py` (GUIA §26); tree and behaviour
  match by construction — parse, layout, `frame::render`, `interact` and
  `host::fetch` are shared code, and the platform shells are the only
  difference (shm vs `/c_stream`, mmap vs fetch, `/b_export` vs `/b_getn`).
- **Docs:** browser quick-start in `docs/clients.md` ("The GUI host in the
  browser": build, serve, the `GuiBridge` surface, the `--ws` requirement, the
  two network data paths, WebGPU/WebGL2, the native-only embed boundary),
  cross-linked from the Python book (`examples.md`) — the two books now link
  the browser quick-start both ways. GUIA gained §24–§26 (manual steps + the
  headless evidence lines). `web/sine.f32`/`web/sine.peaks` (generated demo
  data) git-ignored like the bundle.
- **Verified:** the produced bundle loads in a browser and opens the panel,
  meters and waveform demos against a `--ws` audio server (headless pass
  above); gui 88 tests, `cargo fmt --check`/`clippy -D warnings` clean native
  and wasm32; root crate suite green across the feature matrix. The browser
  track G11–G17 is complete.

## C21 — Seam audit: value/time logic pushed down to the core (completed 2026-07-05)

The audit pass `clients/PLAN.md`'s build strategy calls for before starting the
W (TypeScript) track: sweep the Python reference client for any value- or
time-level computation still implemented in Python and move it into
`clausters-core`/`clausters-ffi`, so a port rebinds the core instead of
reimplementing behaviour. Core ABI **v4 → v5**.

- **Beat queue.** `TempoClock` now drives the core's `Scheduler`
  (`clausters_sched_new/push/peek_time/pop_due/remove/len/clear`) instead of a
  Python `heapq` that duplicated it; `Scheduler::remove(id)` was added for
  `unsched`. Only beats and flat `u64` ids cross; the clock keeps an
  id → routine map (holding the strong reference while queued) and all of the
  control flow (the yield driver, condition-variable pacing) stays in Python.
- **Sample-clock tracker.** The least-squares model (`sample = a + b·t` over a
  sliding anchor window) moved to `clausters_core::clocksync`
  (`clausters_clocksync_*`); the Python `SampleClockModel` is a thin wrapper
  and `UdpSampleClock` keeps only the transport (socket, round-trip midpoint,
  background re-anchoring).
- **Pattern randomness.** `Pwhite`/`Prand` (and the `main.rng` context stream)
  now draw from the core's seeded value generator (`rng::Rng`: splitmix64
  seeding + xorshift64, 53-bit uniforms; `clausters_rng_seed/next_f64/
  next_below` with a single `u64` state word crossing the ABI), never Python's
  Mersenne Twister — a seeded pattern replays the same values in every client
  language. Fixed en route: resuming from a persisted state must not force the
  low bit (only zero is illegal for xorshift; an even word is a normal
  mid-stream state).
- **Timetag packing.** `_osclib` packs NTP timetags through
  `clausters_core_ntp_timetag`/`_unix_to_ntp` (core `osc::pack_timetag`). This
  fixed a real bit divergence: Python truncated the fractional part where the
  core rounds it.
- **Emit-path rounding and grids.** `/sched` sample targets
  (`Server.send_bundle`) and `SampleClockTimebase.sample_at` go through the
  core's `secs_to_samples` (ties to even); `quant` snapping is
  `clausters_core_quant_delay`; `join_transport`'s wall anchor uses
  `samples_to_secs`; `Event`'s degree → midinote resolution is
  `clausters_core_degree_to_midinote` (floored octave wrapping, sclang
  semantics, replicating Python's floor-division behaviour for negative
  degrees).
- **Documented exception.** The OSC byte codec stays per-language: structured
  message arguments cannot cross the flat C ABI ("only flat data crosses"), so
  encode/decode of wire bytes remains in each client while every time value
  inside them comes from the core. Recorded in `docs/clients.md`, which also
  gained the full list of what the core owns after this pass.
- **Verified:** unit tests for every new core/FFI surface (scheduler removal,
  clocksync fit/drift, RNG determinism and state resume, timetag rounding,
  quant, degree wrapping); full workspace suite green and `cargo fmt --check`
  clean; Python suite 129 passed (the suite's behaviour-level assertions —
  yield-exact timing, golden scores, quant snapping — held unchanged); live
  E2E in one Bash invocation (UDP `Pbind` with degrees + `lock_to` driving the
  `/sched` path through the native tracker); `clients/gui/check-wasm.sh` green
  (the new core modules compile for wasm32). Caveat: the gate must run from
  `clients/gui/` — from the repo root `cargo build --lib` resolves to the root
  server crate and fails on `getrandom` for wasm32, which briefly looked like a
  pre-existing breakage; the script now `cd`s to its own directory.

## C21 follow-up — one random context for a whole script (completed 2026-07-05)

Design correction on C21's randomness, at the user's direction: per-pattern
seeds are wrong for music scripts — everything random must share **one
seedable context** or a piece is not reproducible end to end. This adopts the
sclang model:

- **No per-pattern seeds.** `Pwhite`/`Prand` lost their `seed` parameter
  (deliberately breaking: a per-pattern seed now raises `TypeError`); their
  draws go to the random context at draw time.
- **The context** (`clausters/base/rand.py`): `main.seed(n)` seeds the root
  generator; every `Stream`/`Routine` derives its **own** generator at
  creation from the context creating it (`RngStream.spawn` — the child's seed
  is the parent's next word, `clausters_rng_next_u64`, core ABI **v5 → v6**);
  a draw always uses the running routine's generator (thread-local
  `main.current_tt`), falling back to the root outside any routine. One root
  seed reproduces a whole script in creation order, and concurrent routines
  (several clocks, RT beside NRT) are reproducible **per routine** regardless
  of wake interleaving — the thread-local discipline the client already had,
  extended to randomness.
- **Exposed draws**: `clausters.next_f64()` / `uniform(lo, hi)` /
  `next_below(n)` / `choice(items)` (and `main`) re-exported at the top level,
  all answering to the same context. `RngStream` draws are lock-serialized
  (ctypes releases the GIL) so the shared root fallback is safe across
  threads.
- **Tests** (`test_seq.py`): replay of mixed `Pwhite`+`Prand` under
  `main.seed`; the exposed functions replay under the root seed; and the
  per-routine property — two routines' values are unchanged when their
  scheduling order and interleave are flipped. Suite: 132 passed. Docs:
  `routines-and-clocks.md` gained "The random context" (sessions/guide link to
  it); examples migrated from `seed=` to `main.seed(n)`; GUIA section 28.

## M24 — Real-time health: RT scheduling, CPU metering, affinity, stress harness (completed 2026-07-08)

**Goal:** make the audio callback's health observable and controllable —
answer "how many voices fit on one core before the audio breaks" reliably,
verify the server's real-time permissions, and make CPU pinning testable.
Motivated by the field observation that ~1000 one-sine nodes ran but 2000
never did, while `examples/bench` (offline) reported ~1400-1800 sines/core:
the limiter was not throughput but **scheduling jitter** — the callback ran
as SCHED_OTHER, because cpal 0.18 ships the RT promotion only behind its
non-default `realtime`/`realtime-dbus` features.

- **`rtprio` feature (default)**: enables `cpal/realtime-dbus`, so cpal
  promotes the callback thread to real time through `audio_thread_priority`
  — RTKit over DBus, the unprivileged desktop path; works on both the
  PipeWire and ALSA hosts. New build-dep `libdbus-1-dev` (documented in
  `BUILD.md`); droppable like `pipewire` for minimal builds.
- **Ground-truth diagnostic** (`backend::RtDiag` + `RtSetup`): the callback
  thread publishes its **actual** kernel policy/priority (one-shot syscalls
  at callback #64, after cpal's promotion attempt; cold path) and the binary
  logs it shortly after boot — `audio thread is real-time: SCHED_RR priority
  10`, or a warning naming the likely fix. RTKit ORs `SCHED_RESET_ON_FORK`
  into the policy; the reader masks it.
- **CPU meter** (`Engine::process_block`): every block timed with
  `Instant::now` (vDSO `clock_gettime` — no alloc/lock/trap, RT-safe;
  `tests/rt_safety.rs` still green). Published via `Counters` atomics:
  `avg_cpu` (EMA, ~1 s time constant), `peak_cpu` (bitwise `fetch_max` — non-
  negative floats order like their bits — reset on read, so each `/status`
  poll sees its own window), `late_blocks` (cumulative blocks over budget —
  the engine-side xrun proxy, conservative when the device quantum spans
  several blocks). `/status.reply` now reports real avg/peak CPU percentages
  (previously hardcoded 0.0) plus the late count appended as a trailing int
  (positional readers keep working). Test:
  `tests/engine.rs::cpu_meter_publishes_load_and_peak_resets_per_read`.
- **`--pin cpu[,cpu...]`** (Linux, experimental): first CPU pins the audio
  callback thread — it pins itself on its first callback, since the thread
  is spawned deep inside cpal/PipeWire; the rest are assigned round-robin to
  the `clausters-dsp-N` workers via a `/proc/self/task` comm scan at boot.
  Verified live: `pw_out` lands on the requested CPU, workers on theirs.
- **`examples/stress.rs`**: the real-time complement to `bench` — an OSC
  client that `/d_recv`s an n-sine def (`--sines`), ramps m nodes in
  throttled steps against the **running** server, polls `/status` (double
  poll: the first closes the window holding the insertion transient), and
  stops on peak > `--limit` or a late block in the clean window, reporting
  the last stable count. Cross-check real xruns with `pw-top` (ERR).
- **Measured while building it** (release, desktop, 48 kHz): a plain block
  with 1000 default synths ≈ 920 µs of the 1333 µs budget; applying 25
  `AddSynth`s inside one block adds ~150-250 µs (~5-10 µs per apply — the
  linear `NodeTree::find`/free-slot scans — plus first-touch page faults of
  the new synths' wire buffers, allocated on the network thread but first
  written on the audio thread); the peak load sits naturally at 2-3× the
  average at low load. **Follow-ups noted in PLAN.md M24, not taken**: O(1)
  node lookup (id→slot table), RT priority for DSP workers (priority
  inversion under an RT conductor), prewarm/mlock of wire buffers.
- **Docs**: `architecture.md` ("Real-time health" section + threads bullet +
  invariant 1 amended with the deliberate one-shot exception),
  `schemas.md` (`/status` reference), `BUILD.md` (dep + feature row),
  `examples.md` (stress row), `GUIA.md` (M24 manual section + checklist).

## M24b — `rtprio` made opt-in; SIGXCPU guard; Linux tuning isolated in `server::rt` (completed 2026-07-08)

**Why:** M24's default `rtprio` feature surfaced its failure mode immediately
in normal use: driving the server past sustained 100% load tripped RTKit's
`RLIMIT_RTTIME` watchdog and the kernel killed the process with SIGXCPU
("Rebasado el límite de tiempo de CPU (`core' generado`)") — a **silent
death** that left clients hanging. The pre-M24 behavior was preferable:
overload must break the audio, never the process. The RT promotion is a
measurement/tuning aid, not something the release build should pay for with a
process-killing watchdog and a DBus build dependency.

- **`rtprio` demoted from default to opt-in**: `default` no longer includes
  it, so the release build has no `libdbus-1-dev` dep, no RT promotion and no
  watchdog — the callback runs as SCHED_OTHER (the pre-M24 status quo: xruns
  appear earlier under load, the process never dies). `BUILD.md` moved the
  dep to the optional list and rewrote the feature row.
- **All Linux-specific code consolidated in `server::rt`** (new module,
  compiled only with the feature): the callback-thread setup (`RtSetup`:
  tid publication, optional self-pin, one-shot scheduling diagnostic),
  `pin_workers`, `spawn_diag_report` and the SIGXCPU guard, with non-Linux
  no-op stubs. `backend.rs` reverted to its portable pre-M24 shape (one
  `#[cfg(feature = "rtprio")]` field on `BlockAdapter` + one cfg'd statement
  in the callback); `backend::start` lost the `pin_audio` parameter
  (`embed.rs` reverted); the `--pin` CPU reaches the callback thread through
  `rt::request_audio_pin` instead. `--pin` in a build without the feature
  fails with an error naming it.
- **SIGXCPU guard** (`rt::install_sigxcpu_guard`, armed by the binary at boot
  before the stream exists — a signal disposition is process-global, so the
  binary owns it, not the library): the handler demotes the audio thread
  (published tid + calling thread) back to SCHED_OTHER with
  async-signal-safe syscalls and `write(2)`s one line to stderr. The audio
  degrades, the server survives; once demoted the RT clock stops accruing so
  the signal stops firing. Two traps found while verifying it live (server
  up, `kill -XCPU`, `ps -Lo comm,cls,rtprio` before/after, `/status` after):
  (1) the RTKit-promoted thread carries **`SCHED_RESET_ON_FORK`**, and the
  kernel EPERMs an unprivileged `sched_setscheduler` that would *clear* the
  flag — the demotion silently did nothing until the handler OR'd the flag
  back into the new policy; (2) the signal interrupts the network thread's
  `recv_from` with **EINTR** (`SA_RESTART` does not restart a recv under
  `SO_RCVTIMEO`), which the OSC loop treated as fatal — it now `continue`s
  on `ErrorKind::Interrupted` like a timeout tick (`osc/server.rs`), a
  robustness fix that holds for any signal. The overload rule is now
  uniform: **overload breaks the sound, never the process**, in every
  build.
- **Unconditional pieces kept**: the CPU meter and the extended
  `/status.reply` (portable, RT-safe — `Instant::now` via vDSO) and
  `examples/stress.rs` (a pure-std OSC client, no Linux deps; its header now
  points capacity measurement at an `rtprio`-built server).
- **Docs**: `Cargo.toml` feature comment, `BUILD.md`, `architecture.md`
  ("Real-time health" restructured around always-on meter vs opt-in tuning;
  invariant 1 exception scoped to `rtprio` builds), `examples.md`,
  `stress.rs` header, `GUIA.md` (M24 section reworked, new SIGXCPU-guard
  manual test, troubleshooting entry rewritten), `PLAN.md` M24 follow-up
  note. Tests unchanged and green (`cpu_meter_*`, `status_reply_format`,
  `rt_safety`); feature matrix checked with and without `rtprio`.

## M24c — `rtprio` restored as a default feature (completed 2026-07-08)

**Why:** field use of the M24b default (no RT scheduling) immediately hit the
SCHED_OTHER ceiling it had accepted: 500 one-sine nodes glitched at ~46%
*average* CPU — the average misleads, the callback must fit its **worst**
block and unscheduled peaks run at 2–3× the average, blowing the budget long
before 100%. Measured on the same machine and ramp (release, 48 kHz,
`examples/stress.rs`, 1-sine nodes): **~300 stable nodes as SCHED_OTHER
(peak 124% at 36% avg) vs ~500+ as SCHED_RR (peak 92% at 34% avg)** — the
"roughly half the capacity" cost predicted by M24. M24b's real objection was
the silent SIGXCPU death, and its guard removed it; what remained was only
the performance cost. Decision (with the user): a release must ship the best
performance, and an RT-scheduled callback is the standard operating mode of
every production audio client on Linux (scsynth, JACK and PipeWire clients
alike) — so the default came back.

- `rtprio` re-added to `default` (one line in `Cargo.toml`); `libdbus-1-dev`
  back in `BUILD.md`'s default dependency line.
- **Everything that made M24b safe and clean stays** and is what makes the
  default acceptable now: the SIGXCPU guard (sustained overload demotes the
  audio thread to SCHED_OTHER — the audio degrades, the server survives; the
  overload rule "overload breaks the sound, never the process" holds in
  every build), the `SCHED_RESET_ON_FORK`-aware demotion, the EINTR-tolerant
  network loop, and the isolation of all platform-specific code in the
  feature-gated `server::rt` module with non-Linux no-op stubs —
  multiplatform builds are unaffected, and the feature stays droppable for
  minimal no-DBus builds (at the SCHED_OTHER capacity cost).
- **Docs re-framed from "testing aid" to "standard for Linux audio"**:
  `Cargo.toml` feature comment, `BUILD.md` (deps + feature row back to
  default), `architecture.md` ("Real-time health": why the default, with
  the peak-vs-average argument), `examples.md` + `stress.rs` header (the
  default server measures DSP throughput; a build without the feature
  measures jitter), `GUIA.md` (M24 section back to default commands, new
  troubleshooting entry "glitches at low average CPU": check the peak,
  `late_blocks` and the thread's actual scheduling class), `PLAN.md` M24
  follow-up note. Verified: default release boots `SCHED_RR priority 10`,
  survives SIGXCPU (demotion visible in `ps -Lo cls`), full suite + feature
  matrix green.

## C22 — Python box API: Faust's box algebra, libraries included (completed 2026-07-08)

`clausters.defs.boxes` is the box counterpart of `signals` and a complete
def-building API in its own right: Faust's point-free algebra — `seq`/`par`/
`split`/`merge`/`rec`, `wire`/`cut`, the controls, groups, foreign values and
tables, plus the operators on `Box` — as lowercase callables that emit the
server's box-tree JSON (`src/faust/boxes.rs` schema, mirrored one to one).
Where `signals` describes one output at a time referentially (`input(n)`),
boxes describe multi-channel processors that plug into each other. Arities
propagate on the client (`num_inputs`/`num_outputs`, `None` = unknown), which
powers channel selection (`st[k]`/`.outs()`); a real mismatch is Faust's to
report, verbatim, through `/fail`.

On top of the algebra, `faust(src, *eval_args, defs="", ins=None, outs=None)`
compiles any Faust expression into a `Box` indistinguishable from a
primitive, so the libraries (`fi.lowpass`, `os.osc`, `re.`, `pm.`, ...) join
the algebra without transcription. The design keeps Faust's **two application
stages separate in the syntax** — arguments to `faust()` are
evaluation-stage, spliced into the generated source (`faust("fi.lowpass",
3)` compiles `fi.lowpass(3)`; ints/floats as literals, lists as Faust lists,
strings verbatim, `defs=` for helper definitions); arguments to *calling* a
`Box` are composition-stage (`seq(par(args), box)`). No heuristic ever splits
one argument list: only the Faust evaluator knows a function's signature, and
an unapplied pattern-matched function is not a box at all.

The wire rule is enforced, not just documented: each `wire()`/`cut()` builds
a fresh dict, and `FaustDef.from_box` (which now accepts a `Box`; raw dicts
unchanged) rejects by object identity a wire reused in two positions — each
wire is a distinct input, the one silent mistake the algebra allows (the def
would read more bus channels than intended). Every wireless value reuses
freely: duplicated JSON subtrees are shared server-side.

Two server fixes fell out of the milestone's exit tests:

- **Fragment memoization (CSE)** — the blocking exit condition caught it:
  Faust hash-conses everything built from schema primitives (a duplicated
  primitive `rec` loop at depth 2^10 adds 12 bytes of bitcode), but every
  `CDSPToBoxes` evaluation mints fresh recursion symbols, so duplicated
  stateful fragments did **not** share (2^10 copies grew the generated code
  27x). Fix: memoize `dsp_to_boxes` by source text within one compilation
  (`FragMemo`) — same `src`, same box pointer, same subterm, full sharing;
  dup and split now compile to identical code and the redundant front-end
  runs are gone (the CSE suite went from 18 s to 0.24 s). It also covers the
  `cos`/`fmod` workaround fragments.
- **`normal_precision` around the Faust compiler** — the NRT renderer
  compiles scored defs on its flush-to-zero render thread, and libfaust's
  front-end (interval typing, LLVM folding) aborted the whole process
  (`intervalPow.cpp: x.lo() > 0`) on defs the live server compiled fine
  (`fi.lowpass` through box composition was a minimal trigger). The RAII
  bracket in `dsp::denormals` clears FTZ/DAZ around `compile()` and the
  bitcode-restore path and re-arms on exit — documented as the one exception
  to the FTZ invariant in `architecture.md`/`CLAUDE.md`.

Verified: the Rust CSE suite and the mixed-graph parity test (fragments +
ops render bit-identical to the same DSP as pure source) in
`tests/faust_box.rs`; the FTZ regression in `tests/denormals.rs`; 14 Python
unit tests (`tests/test_boxes.py`: schema JSON, splicing, arity, lint,
`__call__`/`outs`); the offline example `examples/boxes_library.py` (osc →
lowpass → stereo freeverb from library fragments, channel selection with
`.outs()`); and a live E2E in one Bash invocation (`from_box` → `/d_faust` →
`/s_new` → `/n_set` on fragment sliders). Docs: the box-API section in the
client book's defs chapter — positioned as the counterpart of `signals`,
with the libraries as the addition, not the module's purpose — plus the
choosing-a-form guidance (fixed chains read better as source; regular banks
as Faust iterations parametrized by splicing; boxes for composed processors,
data-driven structure and mixing library DSP with Python-built pieces).

## G18 — Server audio tap + a real oscilloscope (2026-07-09)

The scopes' shared prerequisite exists: the server can expose the recent
samples of any audio bus, and the GUI's `scope` widget grew an audio-rate,
level-triggered oscilloscope form on top of it — natively with zero per-frame
messages, in the browser over a streamed sibling.

- **The tap region (segment ABI v2 → v3).** The shared-memory segment gains a
  trailing region of `--taps` single-channel sample rings of `--tap-frames`
  samples each (defaults 8 × 16384, ~341 ms at 48 kHz; `--taps 0` removes the
  region). Each slot is a cache-line-aligned monotonic cursor (total samples
  ever written) plus the ring; the write is one `memcpy` + one Release store
  per block, and the read (`tap_read_latest`) copies the newest window with a
  half-ring cap and a cursor double-check, so a torn read is a checked retry,
  never silent garbage. **Decision:** the rings live in the versioned segment,
  not the buffer pool — buffers are freeable and UGen-touched, while the
  segment already owns the "pre-allocated, ABI-checked, host-mapped" role; the
  version bump makes drift fail loudly on attach. A server launched without
  `--shm` but with taps gets an in-memory segment so `/tap_stream` still works.
- **The creation surface is a command, not a UGen.** `/tap tapIndex bus`
  routes an audio bus into a ring by flipping an entry in the engine's
  pre-allocated tap table (`Cmd::SetTap`; `bus = -1` stops; no ack, the
  `/n_map` posture). Any bus becomes tappable live, with no def rebuild —
  where SuperCollider reaches for `ScopeOut2`, here the routing is server
  state. The engine writes active taps at the end of every block, just before
  the mirrored sample clock, so a reader that sees clock N sees block N.
- **The browser sibling: `/tap_stream`.** `periodMs frames tapIndex...`
  subscribes one periodic `/tap_data tap endPosition blob` snapshot per tap —
  the newest `frames` samples as raw LE `f32`, plus the tap's stream position
  so windows sit on the tap's own sample axis. **Decision:** a new command
  pair rather than a `/c_stream` extension (the payload is a windowed blob,
  not bus scalars), same subscription posture (one per client, replaced,
  `periodMs <= 0` cancels, dies with its connection); `frames` clamps to 8192
  and half the ring. `/server_info.reply` appends `taps, tap_frames`.
- **The oscilloscope.** **Decision:** the scope stayed one widget — audio-rate
  is the `tap` prop (or `rate: "audio"`) on the existing `scope` kind, so
  `/gui_set` retunes `tap`/`window_ms`/`trigger`/`hold` live and the catalog
  gains no new type. The signal logic is `host/oscil.rs`, pure and shared by
  both fronts: `window_ms` → display frames (clamped, 48 kHz fallback), a raw
  window of 2× slack, and the trigger — the **latest** rising crossing of the
  level that still leaves a full window, re-armed below a 2%-of-peak-to-peak
  hysteresis, free-running on the newest window when no crossing exists. The
  native front reads the segment's rings per tick (`SharedSegment` ABI v3,
  offsets derived from the header); the web front subscribes exactly the
  tree's taps and reads its `/tap_data` store — both feed the same
  `live::update_tap_windows` and the same `meters::draw_wave` (polyline or
  per-column min/max, never finer than the screen).
- **Placement analysis (the G7b rule):** the trigger search is display-only →
  gui crate (`oscil.rs`). The ring reader stays host-side for now: Python
  headless capture goes over `/tap_stream` (`Server.stream_taps`), which needs
  no mmap, so promoting the ring layout/reader to `clausters-core` + FFI is
  deferred until a client actually needs to map-read taps.
- **Python leg:** `Server.tap` / `Server.stream_taps`; `ServerOptions` grows
  `taps`/`tap_frames` (config-file defaults, emitted as flags) and
  `ServerInfo` reports them; the `scope()` builder grows
  `tap`/`window_ms`/`trigger`/`hold`; `examples/gui_scope.py` shows a
  triggered and a free-running scope on the same tap while the pitch sweeps.

Verified: ring write/wrap/read + every refusal case in `tests/ipc.rs` (and the
pinned v3 segment size); `/tap` + `/tap_stream` E2E against live audio, plus
the no-segment `/fail` paths, in `tests/osc.rs`; the RT guard
`tap_writes_do_not_allocate_on_the_audio_thread` in `tests/rt_safety.rs`; the
host reader against a crafted v3 segment file and the trigger/window math in
the gui crate (93 tests, wasm gate clean); 147 Python tests; and a headless
E2E in one Bash invocation (server + Python client: `query_info` reporting the
region, `/tap`, `/tap_stream`, three non-silent `/tap_data` windows). Docs:
the audio-taps section in `docs/schemas.md`, the v3 layout in `docs/ipc.md`,
GUIA manual steps, and the two GUI skills refreshed.

## G19 — Phasescope + live spectrum (2026-07-09)

The two remaining *future* scopes, both consumers of the G18 audio tap — no new
server work. Both are added by extension (a `WidgetKind` variant + a pure
renderer, never a protocol change) and driven from the Python client over the
unchanged `/gui_*` vocabulary.

- **`phasescope` — the goniometer.** Reads a **stereo pair** of taps (`tap`
  left, `tap2` right defaulting to `tap + 1` — a stereo pair on adjacent rings)
  and draws their recent sample pairs as the 45°-rotated Lissajous figure:
  vertical is the mid `(L+R)/√2`, horizontal the side `(L−R)/√2`, so mono reads
  as a vertical line, anti-phase as horizontal, a wide field fills the lozenge.
  `window_ms` sizes an age-faded persistence trail (oldest faint, newest
  bright; strided down past a segment cap so a long window stays a bounded
  mesh), and a **correlation** bar (Pearson's r over the window, green toward
  mono/+1, red toward anti-phase/−1) sits beneath, with a numeric readout that
  shows a dash on a silent/DC window. `host/phasescope.rs`, pure.
- **`spectrum` — the spectroscope.** One forward FFT per animation tick over the
  newest `fft_size` window of a tap (a supported power of two 256..4096, default
  2048; an unsupported value degrades to 2048), magnitudes to dB with the
  spectrogram's coherent-gain normalization, drawn as a curve over an adjustable
  `[db_floor, db_ceil]` window on a log (default) or linear frequency axis, one
  point per pixel column (never finer than the screen). Raw per-frame FFTs
  flicker, so `averaging` exponentially smooths each bin and an optional
  `peak_hold` overlays a peak trace decaying ~0.6 dB/tick. The analysis is a
  persistent per-widget `SpectrumState` carrying the smoothed and peak-hold
  curves plus reused scratch buffers (so a tick never allocates);
  `host/spectrum.rs`, pure. The FFT and Hann window are the shared
  `clausters_core::fft`/`window`, so the spectrum agrees with the spectrogram
  bin for bin.
- **Shared tick, both fronts.** `live::update_phase_windows` stores each
  phasescope's interleaved `[l, r, …]` window in the same `tap_windows` map the
  oscilloscope uses (ids do not collide); `live::update_spectra` folds each tap
  window into its `SpectrumState`. Both the native front (shm rings) and the
  browser front (`/tap_stream` → `/tap_data` store) call them, so the analysis
  runs once. `WidgetKind::taps_read` reports every tap a widget reads (one for a
  scope/spectrum, two for a phasescope), unifying the animation set,
  `tree_has_live_widget`, and the browser subscription; `live::tap_stream_frames`
  sizes that subscription for the largest window any of the three consumers
  needs. `FrameInputs` grew a `sample_rate` (native from the segment, browser
  from `/clock.reply`) so the spectrum places its frequency axis (48 kHz
  fallback, as the oscilloscope does).
- **Placement analysis (the G7b rule).** The FFT and windows already lived in
  `clausters-core` (reused). The two *new* general audio functions moved to a
  new **`clausters_core::measure`** — the **correlation** (Pearson's r, the
  phase-coherence metric) and the **Lissajous / goniometer** geometry (a stereo
  pair → the mid/side plane) — because both are general measurements, not pixel
  concerns: a future server analysis UGen, a headless Python capture, or an
  electroacoustic-composition sketch plausibly want the same numbers. They gain
  a **`clausters-ffi` export** (`clausters_core_correlation` /
  `clausters_core_lissajous`, CORE_ABI v6 → v7) so a non-Rust client reads the
  identical values the phasescope draws, surfaced as
  `clausters.gui.correlation` / `clausters.gui.lissajous`. Only the display
  (trail, field, curve, correlation bar) stayed gui-side.
- **Python leg.** `clausters.gui.phasescope` / `spectrum` builders; the
  `correlation` / `lissajous` helpers over the FFI; `examples/gui_analyzer.py`
  (a stereo source whose image sweeps mono→wide→anti-phase, beside a live
  spectrum, so the goniometer visibly collapses/opens/falls while its
  correlation swings +1→0→−1 and the spectrum peak drifts along the log axis).

Verified: the correlation/Lissajous math and the spectrum analysis (a sine
peaks at its bin, ~0 dB full-scale; averaging/peak-hold behavior) and the
phasescope drawing in unit tests — gui 101 tests (from 93), `clausters-core`
`measure` and `clausters-ffi` correlation/lissajous tests; `clippy -D warnings`
clean native + `wasm32` and `cargo fmt --check` clean both workspaces; the
Python bindings round-tripped (ABI v7, existing native functions intact); a
headless E2E in one Bash invocation (both widgets parsed over the wire with the
int/float distinction kept, two buses tapped); and a windowed runtime pass
against the real server — the host opened a GPU window, mapped the segment and
ran the phasescope + spectrum tick against a live stereo synth with no panic.
Closes the catalog's four *future* scope entries together with G18. Docs: the
audio-tap-views note in `docs/clients.md`, the Python builder/analysis
docstrings (the generated `api.md`), GUIA manual steps, and the two GUI skills.

## G20 — Editor-grade waveform + spectrogram (2026-07-09)

The two heavy views raised to audio-editor depth — multichannel lanes, pop-free
zoom, rulers, a draggable selection, a playhead and a cursor readout — and the
`spectrogram` finally wired into the host as a widget (it had only existed as
the standalone demo binary). All view-side: the data paths and the analysis
model are unchanged; widgets extend, the protocol does not.

- **Multichannel, one cache resource (decision recorded).** The multichannel
  peak cache is **one file for all channels**, not per-channel siblings — a
  single resource to name in a `cache` prop, atomic, channels can never drift:
  `clausters_core::peaks::MultiPyramid` (format CLPK v2 = a channel count +
  one level sequence per channel; v1 mono caches still parse, and mono
  `Pyramid::to_bytes` still writes v1). The de-interleave lands core-side in
  `MultiPyramid::build_interleaved`, per the placement rule, and the FFI grows
  `clausters_core_peaks_multi_cache_size`/`_build` (CORE_ABI v7 → v8) so
  `clausters.gui.peaks_cache_file(..., channels=N)` builds the byte-identical
  cache the host maps. `WaveformData` holds raw samples + a pyramid per
  channel; the mapped `path` de-interleaves every channel
  (`MappedFile::channels_f32`) and writes/reuses a *multichannel* sibling
  cache; the buffer fetch machine keeps the interleaved download whole
  (channels + `/b_info` sample rate attached) and the fronts decide waveform
  vs. spectrogram by looking the waiting widget up at completion. Lanes are
  **stacked** by default (one viewport per channel, divider lines) or
  **overlaid** (`overlay: 1`) with per-channel trace colors — the waveform
  shader moved from a uniform color to per-vertex color (painter-shaped), one
  vertex range per channel so line strips never connect across channels.
- **LOD crossfade.** `WaveformData::column` blends the two pyramid levels
  adjacent to the zoom, weighted by the fractional position of
  `samples_per_px` between their buckets (`log2(spp/bucket)` in 0..1): pure
  fine at a level's own bucket, converging to pure coarse exactly where
  `level_for` switches, so the min/max envelope is continuous across level
  switches instead of popping. A per-frame data choice inside the existing
  geometry build; unit-tested for continuity at the switch point.
- **The `spectrogram` widget.** `WidgetKind::Spectrogram` with the waveform's
  source surface (`path`/prebuilt single-channel STFT `cache`/server
  `buffer`/inline `data`/`blob`), `channels` lanes analyzed separately
  (`frame::stft_lanes`), `window_size`/`hop` fixed at def time and the display
  (`db_floor`/`db_ceil`/`log_freq`/`colormap`) as live `/gui_set` shader
  uniforms through the new `SpectrogramView::set_display`. A long buffer no
  longer risks GPU validation: `spectrogram::hop_capped` raises the hop so the
  magnitude texture stays within 8192 frames (`MAX_FRAMES`), trading time
  resolution for robustness.
- **Rulers (display-only, `host/ruler.rs`).** An adaptive 1-2-5 time axis in a
  strip under both views — `ruler: "time"` (default; `h:mm:ss.mmm`-style
  labels when a rate is known, from the `sample_rate` prop, the `/b_info`
  reply or the segment), `"samples"`, `"off"` — with unlabeled minor
  subdivisions; the spectrogram adds a Hz ruler along the left edge whose
  decade ticks (1/2/5 labeled) are placed by inverting the shader's exact
  display→bin mapping (`bin_norm = f_lo^(1−d)`), log and linear. Pure tick
  math, unit-tested against the shader geometry.
- **Selection + playhead + readout.** Both kinds share an `EditorProps`
  (ruler mode, `sample_rate`, `sel_start`/`sel_len`, `playhead_at` — `f64`
  fields, sample-accurate past `f32` range): the selection draws as a
  translucent band, drags with the pointer (**plain drag selects** — the
  editor convention — and **Shift+drag pans**; decision recorded), emits
  `/gui_event id "selection" start len` live during the drag (the `"view"`
  event's model) and is settable via `/gui_set`; the playhead draws at
  `sample_clock − playhead_at` (the script anchors it with `/clock` when it
  starts a synth) — natively read from the shm header through the new
  `BusSource::sample_clock` (zero messages), in the browser from `/clock`
  polled once per animation tick; a playhead makes the window animate
  (`tree_has_live_widget`). The cursor readout (time + amplitude, or time +
  frequency by inverting the display mapping) sits in the body's corner. All
  the chrome rides a second **overlay `Painter` pass** drawn after the GPU
  views in the one shared `frame::render`, so the browser front renders it
  identically by construction (browser parity is display + `/gui_set`; the
  drag interactions stay native-only for now — recorded, not a protocol gap).
- **Python leg.** `clausters.gui.waveform` grows the editor props
  (`channels` now keeps every channel, `overlay`, `ruler`, `sample_rate`,
  `sel_start`/`sel_len`, `playhead_at`); a new `clausters.gui.spectrogram`
  builder; `peaks_cache_file(..., channels=N)`;
  `examples/gui_editor.py` — a stereo NRT render shown as a two-lane waveform
  over a two-lane spectrogram from one mapped file, a script-set selection
  replaced live by dragging (events printed), and a looping `PlayBuf` whose
  passes re-anchor the playhead from `/clock`.

Verified: gui 119 unit tests (from 101 — crossfade continuity, ruler 1-2-5 /
`h:mm:ss.mmm` / Hz-vs-shader geometry, multichannel lanes and cache-only
views, widget parse/apply for both kinds, multichannel fetch, `hop_capped`);
core `MultiPyramid` build/round-trip/v1-compat/size tests and the ffi
multichannel build test; `clippy -D warnings` + `cargo fmt --check` clean in
both workspaces, native and `wasm32`; the Python binding smoke-tested against
the rebuilt cdylib (ABI v8, v1/v2 caches produced as expected); a headless E2E
in one Bash invocation (waveform + spectrogram + multichannel cache parsed
over the wire, sel/playhead/display `/gui_set`s applied, `/gui_info`
round-trip); and a windowed runtime pass against the real server — a stereo
file mapped into two waveform lanes, a stereo server buffer fetched into
spectrogram lanes, selection/playhead/display driven live, no panic. Docs: the
bulk-data note in `docs/clients.md`, the Python builder docstrings (the
generated `api.md`), GUIA manual steps, and the two GUI skills.
