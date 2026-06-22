# Implementation plan: a real-time scsynth-style audio server

A synthesis server in Rust controlled over OSC, inspired by the architecture of
scsynth (SuperCollider): a process that opens the audio device, keeps a node tree
(synths and groups), and receives OSC commands over UDP to create/destroy synths,
set parameters, manage buses and buffers, all with sample-accurate scheduling.

## Design principles (non-negotiable)

1. **The audio thread never blocks**: no `malloc`/`free`, no locks, no syscalls,
   no I/O inside the audio callback. See the `realtime-audio` skill.
2. **All communication with the audio thread is lock-free**: SPSC ring buffers for
   incoming commands and for returning "garbage" (memory to free) to the non-RT thread.
3. **Block processing**: blocks of 64 samples (like scsynth), not sample-by-sample,
   to amortize UGen dispatch.
4. **Conceptual, not binary, scsynth compatibility**: same model (node tree,
   buses, buffers, SynthDefs, commands `/s_new`, `/n_set`, etc.) but our own
   SynthDef format (not the binary `.scsyndef` format — at least not in v1).

## Thread architecture

```
┌─────────────┐  OSC/UDP   ┌───────────────┐  SPSC cmd FIFO  ┌──────────────┐
│ OSC client  │ ─────────> │ Network thread│ ──────────────> │ Audio thread │
│             │ <───────── │ (parse OSC,   │ <────────────── │ (cpal        │
└─────────────┘  replies   │  alloc        │  SPSC garbage/  │  callback,   │
                           │  pre-built)   │  reply FIFO     │  DSP)        │
                           └──────┬────────┘                 └──────────────┘
                                  │ slow tasks (disk, decode)
                           ┌──────▼───────┐
                           │ NRT thread   │  (loading files into buffers, etc.)
                           └──────────────┘
```

- **Network thread**: UDP socket, parses OSC (`rosc`), builds commands *already
  fully allocated* (e.g. the Synth node already instantiated) and pushes them to the FIFO.
  The audio thread just "plugs them in" — O(1), no allocation.
- **Audio thread**: cpal callback. Each block: (1) drains the command FIFO,
  (2) runs scheduled bundles whose timestamp falls in this block, (3) walks the
  node tree in order and runs the DSP, (4) pushes dead memory to the garbage FIFO.
- **NRT thread**: reading/writing audio files for buffers (`/b_read`,
  `/b_write`), like scsynth's "NRT thread".

## Crates

| Crate | Use |
|---|---|
| `cpal` | Cross-platform audio I/O (ALSA/JACK on Linux) |
| `rosc` | OSC 1.0 encode/decode (messages and bundles with timetag) |
| `rtrb` | Lock-free, realtime-safe SPSC ring buffer |
| `basedrop` | Shared pointers with deferred deallocation off the RT thread |
| `hound` | WAV read/write for buffers |
| `assert_no_alloc` | In tests/debug: panic if the audio thread allocates |

## Core data structures

- **`NodeTree`**: a tree of `Group` and `Synth` with integer IDs (ID→node map,
  pre-allocated or slab). Execution order = depth-first traversal, like scsynth.
- **`Synth`**: an instance of a `SynthDef`: a vector of built UGens + wiring buffers
  ("wires") + control values.
- **`SynthDef`**: a topologically-ordered UGen graph, with constants, named controls
  and wire assignment. Defined in our own format (see M3).
- **`Bus`**: global arrays of audio buses (per block) and control buses (one value);
  the first N audio buses map to the hardware outputs/inputs.
- **`Buffer`**: a pre-allocated pool of sample buffers with channel/frames/samplerate,
  filled by the NRT thread.
- **UGen**: a trait with `fn process(&mut self, ctx: &ProcessCtx, inputs, outputs)`
  over blocks of 64 samples; dynamic dispatch (`Box<dyn UGen>`) is fine in v1
  (construction happens off the RT thread; the per-block virtual call is cheap).

## OSC protocol (a subset of scsynth)

Implement in this order: `/status`, `/quit`, `/notify`, `/dumpOSC` — `/s_new`,
`/n_free`, `/n_set`, `/n_run` — `/g_new`, `/g_freeAll`, `/g_deepFree`, `/n_before`,
`/n_after` — `/b_alloc`, `/b_free`, `/b_read`, `/b_write`, `/b_zero` — `/d_recv`
(with our format), `/d_free` — `/c_set`, `/c_get`. Bundles with an NTP timetag →
sample-accurate scheduling within the block. See the `scsynth-osc` skill.

## Initial UGens

Oscillators: `SinOsc`, `Saw` (PolyBLEP), `Pulse`, `WhiteNoise`, `Phasor`.
Filters: `LPF`/`HPF` (biquad), `OnePole`, `Lag`.
Envelopes/control: `EnvGen` (with done actions: free self, like scsynth), `Line`.
I/O: `Out`, `In`, `ReplaceOut`. Buffers: `PlayBuf`, `BufRd`. Math: binary/unary
operators between signals. See the `ugen-dsp` skill for the algorithms.

## Milestones

- ✅ **M0 — Skeleton**: `cargo init`, cpal opens the device and a hardcoded sine
  plays. Module structure: `server/`, `dsp/`, `osc/`, `node/`.
  *(Completed 2026-06-10 — see LOG.md.)*
- ✅ **M1 — OSC server**: UDP socket (port 57110 by default), `rosc`, reply to
  `/status.reply`, `/quit`, `/notify`. Logging with `/dumpOSC`.
  *(Completed 2026-06-10 — see LOG.md.)*
- ✅ **M2 — RT-safe FIFO + node tree**: command and garbage ring buffers, `NodeTree`
  with groups, a hardcoded synth instantiable via `/s_new` and freeable with
  `/n_free`. Test with `assert_no_alloc` active in the callback.
  *(Completed 2026-06-10 — see LOG.md. Bonus: `/n_set` brought forward from M3.)*
- ✅ **M3 — SynthDefs**: definition format (suggested: a structure serialized with
  `serde` — JSON/our own binary), an interpreter that builds the UGen vector and
  assigns wires, `/d_recv`, `/n_set` over named and indexed controls.
  *(Completed 2026-06-10 — see LOG.md. Includes the `SynthNode` trait,
  a prerequisite of the F fork.)*
- ✅ **M4 — Buses and order**: audio/control buses, `In`/`Out` UGens, `/n_before`,
  `/n_after`, nested groups, `/s_new` add actions (head/tail/before/after/replace).
  *(Completed 2026-06-10 — see LOG.md. Includes `/g_new`, `/g_freeAll`,
  `/g_deepFree`, `/c_set`/`/c_get` and `/n_go`/`/n_end` notifications. Format change:
  defs no longer carry an `out` field; output is via `Out` UGens.)*
- ✅ **M5 — Buffers**: buffer pool, NRT thread, `/b_alloc`, `/b_read` (hound),
  `PlayBuf`/`BufRd`, async `/done` replies.
  *(Completed 2026-06-10 — see LOG.md. Includes `/b_allocRead`, `/b_write`,
  `/b_zero`, `/b_free` and `/b_query`. Immutable buffers shared by `Arc`:
  the NRT thread builds, the engine swaps, the replaced one leaves via the
  garbage FIFO.)*
- ✅ **M6 — Sample-accurate scheduling**: a bundle queue ordered by timetag on the
  audio thread (pre-allocated), NTP→samples conversion, execution with an
  intra-block offset (splitting the block at the event's sample, as scsynth does).
  *(Completed 2026-06-10 — see LOG.md. `ProcessCtx` processes by `offset`+`frames`
  slices; the engine publishes its sample clock and the NTP→samples conversion
  lives in the network thread. Note: real scsynth quantizes to the block
  — we split the block for real, with no need for `OffsetOut`.)*
- ✅ **M7 — NRT mode + golden tests**: offline render to WAV (same engine, no cpal),
  regression tests comparing against golden files, graph benchmarks.
  *(Completed 2026-06-11 — see LOG.md. `clausters --nrt score.osc out.wav`
  with scores in scsynth's binary format; async commands run synchronously like
  scsynth NRT; goldens in `tests/golden/` regenerable with
  `cargo run --example render_golden`; benchmark `cargo run --release
  --example bench`. Bonus: the rosc blob bug also affected bundle elements
  — fixed for both modes in `osc::decode_packet`.)*
- ✅ **M8 — The sample clock as the client's timebase**: the OS clock and the DAC
  crystal drift relative to each other (tens of ppm ≈ ms per minute), so the
  current NTP→samples conversion re-anchors every bundle against two clocks
  that don't agree. A protocol extension so the client uses the sample
  clock as master: (1) expose `current_samples()` over OSC (in
  `/status.reply` or a new `/clock`); (2) accept bundles with a target
  **in samples** (64-bit integer — `Cmd::Schedule` already works this way, the
  NTP conversion is only the front-end); (3) on the client, model
  `sample(t_local) = a + b·t` from (local monotonic clock, queried sample)
  pairs with forgetting regression — JACK DLL / Ableton Link style —
  and schedule ahead directly in samples. The query latency
  doesn't matter (it only needs bounded uncertainty + scheduling ahead): the
  anchor error shifts the whole grid by a constant, and the *relative* timing
  between events is sample-exact by construction.
  Demo/reference in `examples/json_client.py`; document in
  `docs/schemas.md` the difference from scsynth (which doesn't have this). Note: the
  counter counts samples processed, not heard (add device latency to align
  with the outside world) and pauses on xruns (periodic re-anchoring absorbs it).
  **The two clocks coexist, nothing is discarded**: the NTP path stays intact
  (scsynth compatibility) and the samples target is opt-in **per bundle** — NTP
  clients and sample-clock clients can talk to the same server at once, because
  both front-ends feed the same queue (`Cmd::Schedule`). Signaling:
  since the OSC timetag is NTP format by spec, don't reinterpret it;
  the way is a new container message (e.g. `/sched` with the i64 target +
  the bundle as a blob), which moreover nests/schedules just like an ordinary bundle.
  *(Completed 2026-06-12 — see LOG.md. `/clock` → `/clock.reply h d` and
  `/sched <h target> <blob>` (atomic, internal timetags ignored, the passed
  target = next block); reference client `examples/sample_clock.py`
  with the regression model; documented in `docs/sample-clock.md` +
  schemas.md. The test schedules via `/sched` and asserts the **exact** sample,
  without the neighborhood the NTP path needs.)*

## F fork — SynthDefs via Faust (Box/Signal API + JIT)

An alternative path (does not replace M3–M7: they coexist) for building synthesis
nodes: instead of interpreting a graph of our own UGens, the server receives **JSON
that maps to calls of libfaust's Box API (or Signal API)**, compiles to native code
with the LLVM backend (like FaustLive) and hangs the result on the same node tree.
The advantage: the client's "instruction set" is Faust's full Box API —
clients in any language only generate JSON, without depending on our UGen set.

### Changes to the base design this requires

- **Prerequisite in M3**: the tree's synth node must be `Box<dyn SynthNode>`
  (a trait with `process`, `set_control`, `done`), not a concrete type — so
  `UGenSynth` (M3) and `FaustSynth` (F3) are interchangeable in the same tree.
  M3 must be implemented with this trait from the start.
- **Compiler thread** (new, on top of the NRT one): receives compilation requests,
  serializes access to libfaust (its global context is not thread-safe) and publishes
  factories in a shared table (`basedrop::Shared`). JIT compilation takes
  tens-to-hundreds of ms: always asynchronous, never blocks the network or the audio.
- **RT boundary intact**: `compute()` of an already-initialized Faust dsp is RT-safe
  (no allocations); creating/initializing/destroying instances and factories is NOT —
  instantiation on the network/compiler thread, destruction via the garbage FIFO,
  like the current synths.

### F milestones (after M4 recommended; F0 can be done earlier as a spike)

- ✅ **F0 — Toolchain and minimal FFI**: install libfaust with the LLVM backend;
  evaluate existing crates vs. our own binding with bindgen over the C API
  (`libfaust-box-c.h`, `llvm-dsp-c.h`); feature flag `faust` (all optional, the
  core still compiles without libfaust). Smoke test: compile a hardcoded box
  (sine by recursion/phasor) and render offline comparing against our
  `SinOsc`. Here the real risk is measured: link size with LLVM, libfaust
  version, compilation latency.
  *(Completed 2026-06-10 — see LOG.md. Measurements: JIT ≈ 10 ms per def,
  libfaust.so 11 MB with the system's dynamic libLLVM.so; hand-written binding,
  no bindgen for now.)*
- ✅ **F1 — Compiler thread**: a dedicated thread with a `CompileRequest { name,
  json, client }` queue; a factory table with refcount; async
  `/done /d_faust <name>` replies or `/fail` with the readable compilation error.
  *(Completed 2026-06-10 — see LOG.md. F1 compiles Faust source via
  `/d_faust name source`; the JSON→Box mapping comes in F2. Finding: libfaust
  does not tolerate concurrent compilations in a process — global lock on top
  of the dedicated thread.)*
  New OSC command: `/d_faust` (JSON blob) — `/d_recv` stays reserved for the
  M3 UGen format.
- ✅ **F2 — JSON → Box API schema**: define the schema (primitives, composition
  `par`/`seq`/`split`/`merge`/`rec`, math, delays, and UI `hslider`/`button`
  as named controls); a JSON→C-API-calls interpreter with validation and
  errors carrying the path of the offending JSON node. Access to Faust's stdlib (`os.osc`,
  `fi.` filters) via `DSPToBoxes` embedding Faust source fragments inside
  the JSON — the best of both worlds.
  *(Completed 2026-06-10 — see LOG.md. Schema documented in
  `src/faust/boxes.rs`; `/d_faust` accepts JSON or raw Faust source.
  Finding: an upstream bug in `boxFmod()`, worked around via a fragment.)*
- ✅ **F3 — FaustSynth in the tree**: `FaustSynth: SynthNode` wrapping the
  JIT instance; `/s_new` with a Faust def name instantiates on the network thread
  (`createDSPInstance` + `init(sr)` allocate) and plugs in via the cmd FIFO; mapping
  of buses↔Faust's non-interleaved `inputs`/`outputs`; `/n_set` over parameters
  by name (`FAUSTFLOAT*` zones collected with UIGlue at instantiation);
  freeing via the garbage FIFO with factory refcount (destroying a factory with
  live instances is UB).
  *(Completed 2026-06-10 — see LOG.md. Reserved controls `out`/`in`
  for bus mapping; params are probed once on the compiler thread
  and live in `FaustDef`.)*
- ✅ **F4 — Parity and interop**: Faust and UGen synths coexist in groups/buses;
  golden tests of equivalent graphs (UGen `SinOsc` vs box `sin(phasor)`);
  an example Python client that generates JSON; schema documentation.
  *(Completed 2026-06-10 — see LOG.md. `tests/faust_parity.rs` (sine with
  float tolerance + bit-exact gain + a shared group),
  `examples/json_client.py` (stdlib only), `docs/schemas.md`.)*
- ✅ **F5 — Extensions (optional; revised 2026-06-12, see "Future
  milestones")**: the original list was revised against what the server already
  solves. **Kept**: `waveform` (small tables embedded in the def itself
  — wavetables, transfer functions for waveshaping; self-contained and not
  competing with buffers), Faust's interpreter backend (no LLVM) for platforms
  without JIT — it makes real sense with the M14 wasm target — and the Signal API
  as a low-level variant (low priority: the Box API has covered every case so
  far). **Dropped**: Faust's native polyphony — the node tree is already
  the voice allocator (one voice = one `/s_new`, instances share a factory) and
  the polyphonic mode imposes MIDI conventions (`freq`/`gain`/`gate`) alien to
  the scsynth model; the only real use case would be porting existing
  polyphonic Faust DSP untouched, marginal here. **`soundfile` — initially
  dropped, decision reversed (2026-06-20)**: it was first deemed a duplicate of
  the buffer system (a `PlayBuf`/`BufRd` writing to a bus already feeds any Faust
  node via its `in` control), but the direct bridge proved worth having — Faust's
  `soundfile("<bufnum>", n)` now reads the server buffer named by its integer
  label as a snapshot at `/s_new`, alongside the bus-routing pattern (documented
  in `docs/schemas.md`).
  *(Completed 2026-06-12 — see LOG.md. Ops `waveform`/`rdtable`/`rwtable`
  in the schema, `wavetable` demo in the Python client, the
  buffers-as-signal pattern documented in `docs/schemas.md`; interpreter backend
  and Signal API stay part of the M14 wasm target. The `soundfile` bridge landed
  later, 2026-06-20, reversing the original drop — see LOG.md.)*

### Implementation foresight

- **libfaust dependency (not LLVM directly)**: we link against libfaust; LLVM
  comes embedded inside when the build brings the JIT backend. The cost is paid
  one of two ways depending on the consumption mode: *system* libfaust
  (dynamic) keeps the binary light but inherits version fragility —
  the C API (`libfaust-box-c.h`) changed between Faust versions, the bindgen
  headers must match the installed libfaust, which in turn is tied
  to a specific `libLLVM-XX.so`; *vendored/static* libfaust gives a
  self-contained binary in exchange for tens of MB of LLVM inside. F0 measures
  which is preferable; the `faust` feature flag isolates it all from the core.
- **Sample rate baked into each instance's init**: the compiled factory
  is sample-rate independent, but `instanceInit(dsp, sr)` precomputes the
  rate-dependent constants (coefficients, phase increments) once —
  unlike our UGens, which read `ctx.sample_rate` per block. With
  the SR fixed per `engine_pair` run this doesn't affect anything today; it
  becomes relevant only with a hot device change or an NRT render (M7) at
  another SR. Cheap mitigation: re-`instanceInit` (resets state) or
  re-instantiate.
- **Float width**: the JIT picks `FAUSTFLOAT` by flag when creating the factory
  (`-single`/`-double`, default single). Rule: create factories with `-single`
  and assert the factory's float size before using it, to match
  the `f32` buses. If one day f64 were wanted (e.g. NRT mastering),
  the cheap thing is a conversion buffer at the Faust node's boundary — not
  global f64 buses; the option of a parameterizable `Sample` alias
  in the style of the FAUSTFLOAT typedef stays open.

License: this project is GPLv3-or-later, compatible with libfaust
(GPLv2-or-later); the combination is distributed as GPLv3+. Still missing the
`COPYING` file with the verbatim GPLv3 text.

See the `faust-embedding` skill for the C API details and its pitfalls.

## Future milestones (M9+) — additional features

Section added on 2026-06-12 from a list of ideas to review (M8 came out of that
same list). The order reflects dependencies and cost/value, not urgency: M9–M11
are small and independent of each other, M12 enables M13, M14 is independent of
all. At the end, which ideas were dropped and why is noted. Minor directions that
don't reach milestone status (more UGens — `Saw`/`Pulse`/filters/`EnvGen` with
done actions, already listed above —, `/g_queryTree`, buffer streaming) are taken
as loose items when needed.

- ✅ **M9 — Developer documentation**: today `docs/` only has user documentation
  (`schemas.md`). Add `docs/architecture.md` (in English, like all of `docs/`):
  thread map (network / audio / NRT / Faust compiler), module map
  (what lives in `src/server`, `src/node`, `src/dsp`, `src/osc`,
  `src/faust`, `src/synthdef`), memory lifecycle (pre-built commands
  on the network thread, garbage FIFO, `Arc` pools) and invariants
  no change may break (RT-safety, sample-exact RT/NRT identity,
  always decode via `osc::decode_packet`). And a "how to add
  a UGen in Rust" guide: the trait, arity, `ProcessCtx` by slices,
  where the `kind` is registered, and what tests it requires (signal unit
  + no-alloc + golden if the sound changes). Two decisions get written down here:
  (a) Faust's UI mapping to controls — using the labels as control
  names is deliberate: the author of the def picks the names, just like
  in `controls` of the UGen JSON, with `out`/`in` reserved for buses; the
  *what* is already in `schemas.md`, the *why* was missing from the developer doc;
  (b) UGen plugins: Rust has no stable ABI, so there are no dynamic
  plugins in v1 — extending = compiling within the crate and the documented
  internal API is the contract; if dynamic plugins are ever needed, the
  way is a **versioned** C ABI or wasm (scsynth's historical lesson: its
  plugin ABI broke with every struct or feature change). It closes
  with a rustdoc pass over the public items.
  *(Completed 2026-06-12 — see LOG.md. `docs/architecture.md` with a
  thread/module map, memory lifecycle, a table of pre-allocated capacities,
  invariants, the "how to add a UGen" guide and the two decisions; pointers
  from CLAUDE.md and schemas.md; rustdoc with no warnings in both configs. The
  capacities table front-runs half of M10's audit.)*

- ✅ **M10 — Bounded memory and alignment**: the "denormals" half of the original
  idea is already done (post-M7: `dsp::denormals`, `-ftz 2`, tests); the
  memory half remains. (1) Audit and document in a single table (in
  `docs/architecture.md`) every pre-allocated capacity — command/garbage/event
  FIFOs, schedule queue (1024), node slab, buffer pool (1024), buses (128 audio /
  1024 control) — and the failure mode of each when full: the command FIFO
  already replies `/fail … command FIFO full` on all live-server paths;
  verify the rest (what does the audio thread do if the garbage FIFO is full?,
  and the event one?) and align behaviors. (2) Alignment: wires and bus blocks
  are `[f32; 64]` with a natural 4-byte alignment; wrap them in a
  `#[repr(align(64))]` type (one block = 256 bytes = 4 whole cache
  lines, not split) for stable autovectorization — measure with
  `examples/bench` before and after and keep it only if it doesn't get worse.
  (3) Update the `realtime-audio` skill with the three things: bounded
  memory with its table, an alignment note, and a reference to the denormal
  protection already implemented.
  *(Completed 2026-06-12 — see LOG.md. The M9 table now pinned by
  `tests/capacity.rs` (garbage/event/slab/group overflows + a new row
  for the M14 rings); `Block` `#[repr(C, align(64))]` for wires, buses and Faust
  staging — interleaved A/B bench: neutral within the noise (±4%), kept
  for the stability argument; `realtime-audio` skill updated
  (failure modes, alignment, real denormals instead of the deprecated
  `_mm_setcsr`).)*

- ✅ **M11 — `/n_map`/`/n_mapa`: buses as a parameter source** (derived
  from the review of Faust's UI): the conception "UI elements are
  signals arriving over control buses" today is only true for UGen
  defs that include `InCtl` in their graph; Faust params only move via
  discrete `/n_set`. `/n_map nodeID ctl bus` (scsynth) unifies it for both
  worlds: the node reads the control bus at the start of each block and
  writes it into its control/zone until `/n_map ctl -1` or a later `/n_set`
  disables it. RT-safe implementation: a per-node mapping table (control
  index → bus) resolved on the audio thread by reading the control-bus
  atomics that already exist — no allocation. Schedulable in bundles like
  `/n_set`.
  *(Completed 2026-06-13 — see LOG.md. `/n_mapa` was also implemented
  with audio buses: since a control is a scalar per block (and Faust zones
  too), it samples one sample of the bus per block (control-rate, faithful to
  scsynth for `kr` controls; there are no audio-rate controls — for audio there's
  `In`/`in`). The mirror sums an audio mapping's bus into the node's reads
  and marks `dynamic` if the mapped control is a bus index, so M12/M13
  stay correct. `tests/mapping.rs`, +tests in rt_safety/auto_order/
  faust_synth; example `osc_ping map`. The multi variants
  `/n_mapn`/`/n_mapan` remain optional.)*

- ✅ **M12 — Canonical graph form via bus connections**: infer the dependency
  DAG between nodes from the buses: which audio buses each def reads
  (`In`, Faust's `in`) and writes (`Out`/`ReplaceOut`, Faust's `out`).
  The analysis is static only when the bus indices are constants
  or controls — not computed signals: an analyzable def contributes edges, and
  a node with a dynamic bus index acts as a conservative barrier (it depends
  on everything before and everything after depends on it). Over the DAG,
  **opt-in auto-ordered groups** (a new flag on `/g_new` or a command
  `/g_sortMode`): within that group the execution order is recomputed on
  the network thread on each topology or def change and applied
  reusing the existing move machinery (equivalent to `/n_before`) —
  zero changes on the audio thread. Cycles (legitimate read-before-write
  feedback) are not "resolved": they keep the explicit order
  in force = one block of delay, like the return sends of a multitrack
  editor; document it. The loss of flexibility is contained by the
  opt-in: in an auto-ordered group, manual `/n_before`/`/n_after`
  reply `/fail`. So the client can inspect what was inferred:
  `/g_queryTree` (pending from the scsynth set) plus a debug `/g_dumpGraph`.
  Benefit: groups become "multitrack channels" and the
  client stops micro-managing the execution order.
  *(Completed 2026-06-12 — see LOG.md. `osc/graph.rs`: per-def bus
  analysis + `TreeMirror` on the network thread + stable topological sort;
  `/g_sortMode` (schedulable and valid in NRT scores), `/g_queryTree`
  scsynth-compatible and `/g_dumpGraph`; the server's immediate handlers
  were unified via `CmdTranslator::translate`. Example
  `examples/auto_order.py`, doc `docs/auto-order.md`. Zero changes on the
  audio thread, as planned.)*

- ✅ **M13 — Parallel tree processing** (requires M12): the M12 DAG
  is exactly the structure that enables parallelism — stages =
  sets of nodes with no dependencies among them — analogous to supernova's
  `ParGroup` but inferred instead of declared. RT workers (N−1 threads with
  audio priority) synchronized per stage with bounded spin + backoff;
  no locks or syscalls on the hot path. The central risk is the write
  hazard: two nodes in the same stage summing to the same bus.
  Since the analysis already knows the writes, the initial rule is "same
  stage ⇒ disjoint write buses; otherwise, serialize within the
  stage" (the alternative — per-worker accumulators + a reduction pass —
  costs memory and an extra traversal; kept as plan B). `assert_no_alloc`
  on all workers; NRT mode benefits equally (faster
  renders). Tackle it only once a real graph exists that doesn't fit on one
  core: today `examples/bench` gives ~1800 sine voices on one core, and this is
  the most expensive milestone in complexity of the whole section.
  *(Completed 2026-06-12 — see LOG.md. Stage partitioning in the
  engine itself from `BusUsage` masks sent in `Cmd::AddSynth`
  — safety never depends on the network mirror —; `server/workers.rs`
  (fork-join with atomic work stealing, bounded spin, park on idle);
  `/g_parallel` + `--workers` in RT and NRT; **bit-identical to sequential**
  by construction and by test; measured speedup ~3.3x with 3 workers on the
  8-chain × 125-sine bench. Doc in `docs/parallel.md`.)*

- ✅ **M14 — Pluggable transports, embedded mode and synchronous calls**
  (redefined 2026-06-12; before it was only "shm data plane"): the
  goal is that a local client uses the server as if the application
  were monolithic — no network protocol in sight and no mandatory
  asynchrony — without losing remote control over UDP. Three layers:

  1. **Separate encoding from transport.** OSC stays as the single
     encoding (messages, bundles, M8 timetags, replies: one single
     parse/validate path with `decode_packet`); the transport becomes
     a trait with three implementations. **UDP**: the current one, for
     remote clients — modularity is not lost. **OSC-bytes ring in
     shared memory** (two processes, same machine): a pair of round-trip
     rings per client, the commit index published at the end of each
     write (a client dying mid-write corrupts nothing),
     content treated as untrusted bytes (OSC validation already
     exists), wakeup via a named semaphore/eventfd — blocking there is
     legal because the one draining is the **network** thread, not the audio
     one; in exchange, over local UDP: real backpressure instead of silent
     packet loss and no open port. **In-process**: the truly monolithic
     case — the server as a library, the client hands the OSC bytes
     via a function call to the network thread, in the style of `World_SendPacket`
     from libscsynth. The browser has no UDP, so this abstraction is
     also a prerequisite of the wasm target (there the "ring" is a
     `SharedArrayBuffer`; it depends on the F5 interpreter backend, no JIT
     LLVM in wasm).

  2. **Shared data plane** (the original M14): a cross-platform
     segment (`shm_open` on Unix, `CreateFileMapping` on Windows;
     `memmap2` or similar) with a magic header + **layout version**, the sample
     clock (the `AtomicU64` the engine already publishes — M8 anchors without
     UDP jitter) and the control-bus array (read/write, the
     same atomics). In in-process mode it's direct access, no segment.

  3. **Synchronous execution mode.** scsynth-style asynchrony is tedious
     in clients (Routines in sclang, callbacks/promises in JS); for
     interactive/scientific use in Python (query a datum and plot it) the
     binding offers a blocking facade: a call that waits for data =
     send the request + block with a timeout until the correlated reply.
     It requires no server changes (it works even over UDP today), but
     it does require solving two things. **Correlation**: replies identify by
     command + bufnum/nodeID — enough if the binding serializes its requests;
     for real concurrency, a minimal protocol extension: an optional token
     on the queries that the reply echoes. **Large data**: reading a whole
     buffer over UDP requires chunking `/b_getn` style (datagram limit); in
     in-process mode it's **zero-copy** — buffers are already immutable
     `Arc<Buffer>`, the binding clones the `Arc` on the network thread and exposes
     a pointer + length of flat `f32`. **Boundary principle**: only basic
     structures (contiguous `f32` arrays, integers, error strings), never types
     from a library — scientific ones were a usage example, not a dependency:
     numpy can *view* that pointer without copying (buffer protocol), but that
     is the client's choice, not the binding's. A bonus that closes the
     loop: the NRT render as a synchronous call
     (`render(score) → frames f32`; `render_to_vec` already exists). The
     synchronous part is always the **client** waiting: the audio thread never
     finds out and the server never blocks. Per-language note: Python blocks
     without issue; in JS the synchronous mode only exists in workers
     (`Atomics.wait` over `SharedArrayBuffer`) — on the main thread there's
     `await`, which is already tolerable.

  Deliverables: transport trait + shm ring + embedded mode (feature
  `embed` or a separate crate). The **versioned-C-ABI cdylib** (here the
  ABI lesson from the plugins idea applies) is mandatory — it's what allows
  connecting any language: JavaScript via Node/Deno FFI, and whatever
  comes next. The bindings are thin wrappers that respect the boundary
  principle; **how each binding is built is orthogonal to that principle**:
  for Python (the main target) the two ways are stdlib `ctypes`
  over the C ABI (pure Python, zero build of its own, but signatures declared
  by hand — fragile) or a **PyO3** module (a native extension: idiomatic classes,
  errors → exceptions, trivial zero-copy buffer protocol; distributed
  as a wheel). PyO3 imposes no dependencies on the client — it exposes
  native types and a `memoryview` over flat `f32`, without numpy — and for the
  embedded mode it's the most natural path: the module *is* the server linking
  the engine directly, without going through the C ABI. Options to define when
  tackling it: Python client via ctypes or PyO3? (PyO3 favorite for the embedded
  mode; ctypes is enough for the two-process case); a correlation token in the
  protocol? (start without it, serializing requests in the binding);
  large buffers via the shm segment in the two-process case? (start
  without that: copying into the segment doubles memory and complicates the
  layout — large data stays for the embedded mode, which is the real
  scientific use case).
  *(Completed 2026-06-12 — see LOG.md. Versioned segment in
  `server/ipc.rs` (ABI header v1 + per-block-mirrored sample clock +
  genuinely shared control buses + a pair of SPSC OSC-byte rings),
  file-mapped backing (`--shm`, stdlib `mmap` Python client) or heap
  (in-process); `ClientId` replaces `SocketAddr` in the server (replies
  routed by transport); C ABI `embed` (cdylib): synchronous `clausters_render`
  + a live in-process server; binding `clients/python/clausters.py`
  with the synchronous facade. Explicit deferrals: wakeup semaphore (v1
  2 ms polling), multiple ring clients, correlation token, shm
  buffers, JS/wasm.)*

- ✅ **M15 — Comprehensive English documentation (README + mdBook + rustdoc)**:
  today the English documentation is good but scattered in `docs/`
  (`architecture.md` development; `schemas.md` OSC/user reference;
  `auto-order.md`, `parallel.md`, `sample-clock.md`, `ipc.md` per feature) and
  it lacks a front door and a navigable structure that unifies it. Three
  audiences to cover: the OSC user, the **library**/embedded user
  (`rlib`+`cdylib`: `engine_pair`, `render_to_wav`, the C ABI), and the developer.
  Plan:
  - **README.md** at the root, in English (mandatory): overview, quickstart
    (build → run server → an OSC command; and an NRT render), feature matrix
    (`realtime`/`faust`/`embed`), links to the book and rustdoc, license
    GPL-3.0. It doesn't duplicate the book, it links.
  - **mdBook** as the navigable body, the **Rust community standard**
    (the source lives in the repo, the generated HTML is git-ignored). `book.toml`
    at the root with `src = "docs"` to **reuse the `docs/*.md` in place**
    (zero churn in the incoming references to `docs/x.md` that exist in rustdoc,
    tests and this file). `docs/SUMMARY.md` builds the index; new chapters in
    `docs/`: `introduction.md`, `getting-started.md` (English version of the
    runnable parts), `using-as-a-library.md`, `examples.md` (catalog of
    `examples/` and `clients/python/`), `contributing.md` (development setup,
    libfaust from source, the single-Bash-invocation E2E rule). The
    existing chapters (`architecture.md`, `schemas.md`, the feature ones) are
    reused as is.
  - **rustdoc** as the API reference: expand the crate doc-comment
    (`src/lib.rs`) to orient (engine/network split, feature flags, entry
    points), linked to and from the book.
  - The Spanish files (`PLAN.md`, `NOTAS.md`, `GUIA.md`) **stay in
    Spanish and in place** — they're the author's and keep being updated; the
    English user doc is new/separate (`GUIA.md` is still the per-milestone QA
    checklist). *(Historical note: this decision was later revised — `PLAN.md`,
    `clients/PLAN.md` and `LOG.md` were translated to English; only `GUIA.md`
    and the conversation with the user remain Spanish.)*
  - Optional (out of the first pass): a CI workflow for `mdbook build` +
    deploy to GitHub Pages and `mdbook test`; split `schemas.md` if it gets long.

  Close-out criterion: `mdbook build` and `cargo doc` clean and with no broken
  links; README and book with a clear path from the front page for each of the three
  audiences.
  *(Completed in an earlier session — the work is in commit `5424855`
  "Documentation" (unconventional message, predating this log):
  `README.md`, `book.toml` (`src = "docs"`), `docs/SUMMARY.md` and new
  chapters `introduction.md`/`getting-started.md`/`using-as-a-library.md`/
  `examples.md`/`contributing.md`, expanded crate doc-comment in
  `src/lib.rs`, `book/` git-ignored. `mdbook build` (v0.5.3) and `cargo doc`
  clean. The formal close-out —this ✅ and the LOG.md entry— was pending
  and is recorded now.)*

- ✅ **M16 — On-disk def persistence + bitcode cache**: today defs
  (`/d_recv` and `/d_faust`) are volatile, living only in memory; a client that
  builds a library (even importing pieces of faustlib as faustdefs)
  has to resend it each session. Save the defs in a data directory and
  reload them at startup, in two layers: **B** — the original definition (JSON
  of the `SynthDefSpec` for UGens, source/JSON of the Faust def) as a transparent
  source of truth, recompiled on reload; **A** — for Faust, an LLVM bitcode
  cache (`writeCDSPFactoryToBitcodeFile`) **non-authoritative**, keyed by
  libfaust version + payload sha, that skips Faust's front-end at
  startup and falls back to recompiling on any miss/corruption/upgrade.
  Two subdirs `synthdefs/` and `faustdefs/`; dir resolved by
  `--data-dir`/`$CLAUSTERS_DATA_DIR`/XDG, `--no-persist` to turn it off, only on
  the RT server (NRT doesn't persist). Incremental reload on the compiler thread so as
  not to block startup with large libraries. The `FaustDef` itself isn't
  serialized (opaque JIT factory): the definition is persisted, not the artifact.
  *(Completed 2026-06-16 — see LOG.md. Bitcode FFI + `getCLibFaustVersion`;
  modules `faust::cache` and `server::defstore`; `CacheJob`/`client: Option` in the
  compiler thread; wiring in `osc::server` + flags in `main`; dep `sha2`.
  `tests/persistence.rs` (3 core + 6 faust): sample-identical bitcode
  round-trip, end-to-end reload between two servers, version mismatch,
  fallback on corruption, deletion via `/d_free`. Docs in `schemas.md`,
  `architecture.md`, `examples.md`, `GUIA.md` and `examples/persistence.sh`.)*

- ✅ **M17 — MIDI: server protocol and client output (reusable Rust core)** *(**DONE** — sub-part 3 server protocol + live ALSA transport, 2026-06-18; sub-part 1 client offline `.mid`/clip + the `clausters-midi` crate, 2026-06-18; sub-part 2 client live MIDI out + the MIDI 2.0 clip writer, 2026-06-19. Note: the planned `midi2-clip` crate was a v0.1.0 stub, so the clip container is built from `midi2`'s UMP messages)*: today the server only speaks OSC and the Python client doesn't export MIDI (`clients/python/clausters/base/_midiinterface.py` accumulates events but `MidiNrtInterface` writes nothing and `MidiRtInterface` is a stub). MIDI appears in two places — as a **control protocol alternative to OSC in the server** (a user backlog idea) and as **client output** (`.mid` offline + live ports) — and both share the same message layer, so MIDI lives in a **reusable native crate** (`crates/clausters-midi`, cdylib+rlib, versioned C ABI with the same pattern as `clausters-ffi` and `src/embed.rs`), language- and side-agnostic: used by the Python client via ctypes, the future JS client and the server itself. No Python MIDI library (python-rtmidi): the core is Rust, like `clausters-core`.

  **Message layer: MIDI 2.0 (UMP) via `midi2`.** `midi2` was analyzed (bl-midi2-rs, github.com/midi2-dev/bl-midi2-rs; crates.io `midi2`, v0.11 Aug-2025, MIT/Apache-2.0): strongly-typed wrappers for **all** messages of the MIDI 2.0 spec (rev 1.1) over **UMP (Universal MIDI Packet)** — Channel Voice 2 (default feature), Sysex 7/8-bit, Flex Data, System Common/Real-Time, UMP Stream and MIDI-CI (WIP), all opt-in by features. It is **`no_std` and non-allocating**, generic over the backing buffer (`Vec`, `[u32; N]` on the stack, or a borrowed `&[u32]`) → it fits the server's RT boundary (messages built on the stack, zero allocations on the audio thread). The decisive thing is the **higher resolution**: 16-bit velocity and 32-bit controllers versus classic MIDI 1.0's 7 bits — relevant here because a MIDI control ends up mapped to an `f32` parameter of a Faust/UGen node, where 7 bits of quantization are noticeable. **Key limitation**: `midi2` **does not read or write Standard MIDI Files (`.mid`/SMF)** — it's only the message layer. **Standard channel-voice messages are the primary actuation path** (see sub-part 3); **SysEx is reserved only for the non-musical control plane** that has no channel-voice equivalent (`/d_recv` SynthDef/FaustDef load, buffer ops, graph topology) — never the default "tunnel every OSC command" carrier. **Backward compatibility**: MIDI 2.0 is backward-compatible with MIDI 1.0, so the design leverages v2's higher resolution (16-bit velocity, 32-bit/per-note controllers → direct `f32`) **without losing MIDI 1.0**: classic 7-bit channel-voice input is accepted and widened to the same `f32` zones, and the SMF export degrades back to 7 bits — one internal UMP representation, both wire versions in and out.

  **Persistence at full resolution: MIDI 2.0 Clip File via `midi2-clip`.** It's worth preserving the higher resolution on disk: the classic `.mid` (SMF) is MIDI 1.0 by definition of the format and quantizes velocity/controllers to 7 bits on write, so the primary format for scores becomes the **MIDI 2.0 Clip File**, written with `midi2-clip` (crates.io `midi2-clip` v0.1, same author as `midi2`: reads/writes MIDI 2.0 clip files) — it preserves UMP's 16/32 bits end to end. Since **MIDI 2.0 is backward-compatible with MIDI 1.0**, a `.mid`/SMF (MIDI 1.0) writer is kept with `midly` (pure Rust, no system dependency) as an **interop path** for DAWs/tools that only understand MIDI 1 (deliberately degrading to 7 bits). The `clausters-midi` crate hides both formats behind its C ABI, respecting the **only-flat-data** boundary (integer POD in, malloc'd file bytes out — the same shape as `clausters_render`/`clausters_free_samples`).

  **Sub-parts** (cost/value order; tackleable separately):
  1. ✅ **Client — offline file (NRT)** *(DONE 2026-06-18: SMF `.mid` via `midly`; MIDI 2.0 clip added 2026-06-19 — assembled from `midi2` UMP messages since `midi2-clip` was a stub)*: sequence a `Pbind` and write a **MIDI 2.0 clip** (full resolution) — or an interop `.mid`/SMF — with exact timing (ticks ← logical beat, reusing the client's RT/NRT seam). Minimal double-dispatch refactor: move the OSC realization out of `Event.play` to `destination.play_event(event)` (the `/s_new`+`/n_free` logic moves to `Server.play_event`, with no behavior change — the golden in `clients/python/tests/test_golden.py` guards it), so a pattern points at OSC **or** at a MIDI destination without touching clock or routine. `MidiScore` switches to storing `(beat, message)`, `MidiNrtInterface.write(path, ppq)` converts beats→ticks and delegates the file write to the crate; binding `clients/python/clausters/_midi.py` via ctypes (same lazy/versioned pattern as `_native.py`). **Implemented as**: `Event.play(destination)` → `destination.play_event(event)` (OSC logic moved verbatim to `Server.play_event`, golden byte-identical); a `MidiServer` destination (`base/_midiinterface.py`) records `(beat, note on/off)` into `MidiScore`; `MidiScore.to_smf(ppq)` → `_midi.py` ctypes → `clausters_midi_write_smf`. Crate writes SMF via `midly`; `midi2-clip` (full-resolution clip) pending. `tests/test_midi.py` + `examples/midi_file.py`.
  2. ✅ **Client — live MIDI** *(DONE 2026-06-19)*: output to OS ports (the crate's `live` feature, `midir`/ALSA; best-effort, no timetags — MIDI carries none), a real `MidiRtInterface` replacing the stub. **Implemented as**: `clausters_midi_output_open`/`_send`/`_close` (opaque handle) in the crate; `MidiRtInterface` (`base/_midiinterface.py`) opens the port via `_midi.py` and `emit`s each message at its beat (note-on now, note-off via `clock.sched_abs`); `MidiServer(interface=...)` is the RT/NRT seam. Full-loop E2E verified (client out → `aconnect` → server `--midi` in → synths). `examples/midi_live.py`.
  3. ✅ **Server — MIDI protocol alternative to OSC (standard channel-voice actuation)** *(DONE 2026-06-18, commit `585856e`)*: accept standard channel-voice MIDI as a control path parallel to OSC, with **standard channel-voice messages driving synthesis nodes and their named `f32` input controls directly** — the interoperable path any DAW/controller speaks. The pieces, all implemented:
     - **Binding command** `/midi_bind`: maps a MIDI channel — UMP group(4b)+channel(4b), the full 256-channel space — to an **instrument def name (SynthDef *or* FaustDef, treated identically)** + target group + add-action + a **control map**. Unbind via the paired form (empty instrument / `/midi_unbind`). A normal OSC command on the network-thread path; no audio-thread state. **SynthDef and FaustDef are MIDI-actuated on equal footing**: the path is purely `/s_new` + named `f32` control zones + `/n_free`/`gate`, the surface both def kinds already share — nothing in it is SynthDef-specific.
     - **Control map** (defaults matching the client `Event`'s `freq`/`amp` + extras, overridable per binding): note number → `freq` (microtonal fraction via the MIDI 2.0 note-on attribute / per-note pitch); velocity → `amp`/`gain` (16-bit → `f32`, no 7-bit loss); note off → `/n_free` or `gate 0` for gate-aware defs; poly aftertouch + MIDI 2.0 per-note controllers → **per-voice** `f32` controls (e.g. `pressure`); channel pressure / CC / pitch-bend → `/n_set` on the channel's live voices or its group (CC#→control-name sub-map with range→`f32` scaling).
     - **Named conversion helpers** (a shared module, not inline math): `midi2freq` (note + fractional pitch → Hz, the `f32` server counterpart of the client's `midicps`), `velocity2amp`, `cc2control`, `bend2freq`, `pressure2control`, … so the control map references them by name and client/server agree on the curves.
     - **Voice tracking** `(channel, note) → node_id` plus a **server-side node-id allocator** for MIDI-spawned voices (today the client allocates ids; MIDI notes originate in the server, so it needs its own reserved id range — documented in `schemas.md`). All of this lives on the **network thread**: MIDI bytes are parsed allocation-free and translated into the same pre-built engine commands as OSC, pushed over the existing FIFO — `process_block` and the RT-safety invariants are untouched.
     - **Transport — decided and implemented (2026-06-18)**: standard OS MIDI via **`midir`** (the plan's live crate; ALSA sequencer on Linux), a **virtual input port** opened with `--midi [name]`. Network MIDI is explicitly a separate, out-of-scope idea, not this. (UDP-MIDI-2.0 / raw-ALSA-seq were the discarded alternatives.)
     - *Implemented (2026-06-18)*: the **actuation core** — `src/midi/` (the `ChannelVoiceMessage` taxonomy + the named conversions `midi2freq`/`velocity2amp`/`aftertouch2control`/`bend2control`/`cc2control`/`program2control`, a MIDI 1.0→2.0 widening parser, and the `MidiBindings`/voice/id state), plus `CmdTranslator::translate_midi` and the `/midi_bind`, `/midi_unbind`, `/midi_map` commands; a MIDI voice is byte-identical to the OSC path (`tests/midi.rs`). The **transport** — `src/midi/live.rs` (`midir`/ALSA virtual port, its input thread decoding → the network thread via mpsc + UDP wake, the TCP molde), `OscServer::listen_midi`/`drain_midi`, the `--midi` flag, feature `midi` in default. E2E with real ALSA verified (note-on via `aplaymidi` → `synths 0→1`; recipe in `GUIA.md`). (The client/persistence pieces this once listed as pending — the `crates/clausters-midi` crate and client sub-parts 1–2 — **also landed**: sub-part 1 on 2026-06-18, sub-part 2 + the MIDI 2.0 clip writer on 2026-06-19; see sub-parts 1–2 above. That is exactly **C11** of `clients/PLAN.md`, now closed.) Live input here is MIDI 1.0 (7-bit widened); the full 16/32-bit resolution is available on the client's MIDI 2.0 clip file.

  Decisions already taken with the user: native MIDI in Rust (not python-rtmidi); a single reusable client+server crate; persistence/NRT first and live as a follow-up; **high resolution preserved via MIDI 2.0** — UMP (`midi2`) for messages/protocol and Clip File (`midi2-clip`) for disk, with `.mid`/SMF (`midly`) as MIDI 1.0 interop; **standard channel-voice messages are the primary actuation path** (SysEx reserved for the non-musical control plane); **server-side `/midi_bind`** maps channel → instrument def + group + control map with `(channel, note)` voice tracking; **SynthDef and FaustDef are MIDI-actuated identically**; **per-note expression** (poly aftertouch / MIDI 2.0 per-note controllers → per-voice `f32`); **named conversion helpers** (`midi2freq`/`velocity2amp`/`cc2control`/…); **MIDI 1.0 backward compatibility** (accept 7-bit in, SMF degrade out). **Transport decided (2026-06-18)**: live input uses **`midir`** over the OS's standard MIDI (ALSA seq on Linux), a virtual port via `--midi`; the server protocol is implemented (see sub-part 3). **Crate evaluation resolved (2026-06-19)**: `midly` (SMF) and `midi2` (UMP message layer) are solid and used in `clausters-midi`; **`midi2-clip` v0.1.0 is a non-functional stub** (`write_clip_file`/`read_clip_file` are `todo!()`), so the SMF2CLIP container is assembled directly from `midi2`'s UMP messages. `midir` (live) used by both server and client. No date: tackled after closing the client loose ends (track C) and depending on the priority of the server protocol. (Milestone **C11** of `clients/PLAN.md` —in its "Future milestones" section, moved from the old C7— redirects to this M17.)

- ✅ **M18 — GraphDef: persistent node-graph definitions ("programs")** *(**DONE** — server core + client + scsynth group `/n_set` propagation 2026-06-19; per-voice partition `/graph_voice` + MIDI-bind a GraphDef 2026-06-20. See LOG.md. `src/osc/graphdef.rs`, `/d_graph`/`/graph_new`/`/graph_voice` with private-bus pool + named surface + shared/per-voice split, `/midi_bind` → GraphDef, `defs/graphdef.py` builder (`voice=True`, `graph_voice`), `tests/group_nset.rs`/`graphdef.rs`/`test_graphdef.py`, `examples/group_set.py`/`graphdef.py`/`graphdef_poly.py`)*: a **third persistent def kind** alongside SynthDef and FaustDef. Where those persist a *single* synthesis node, a GraphDef persists a whole *configuration of nodes wired by buses* — a loadable "program" — so the server can hold patches, not just instruments. Motivation: to be driven without a programming environment (see M19), the server needs more than per-note instruments; it needs the wiring (an FX chain, a mixer, a layered instrument) saved as a unit. **Content of a GraphDef** (a JSON spec, like `SynthDefSpec`): (a) **member nodes**, each referencing an existing SynthDef/FaustDef *by name* with initial controls and a target slot; (b) **internal bus references** — *symbolic* audio/control buses private to each instance (resolved to concrete indices at instantiation, so two instances never collide), allocated from the existing pools on the network thread; (c) **connections**, expressed with the same reserved `in`/`out` bus controls and `/n_map` the rest of the system already uses (M11/M12); (d) a **shared/per-voice partition** — members tagged *shared* are instantiated once per GraphDef instance, members tagged *per-voice* are instantiated per note (the general-container model chosen with the user: a GraphDef is both a fixed patch *and* a polyphonic voice template, or either); (e) a **named parameter surface** — ports that map to inner member controls or internal buses (a control map), so *all external actuation targets the GraphDef surface, never inner node ids* (the chosen design: a GraphDef behaves like a composite SynthDef with named controls). **Commands** (parallel to the def/node families): `/d_graph` loads/validates a GraphDef (async with `/done`/`/fail`, but cheap — no JIT, it only validates that member defs exist and the wiring is consistent), `/d_free` removes it; `/graph_new defname id target action [port values...]` instantiates one — creates a **group** holding the shared sub-graph, allocates the instance's private buses, wires them via the existing move/`/n_map` machinery, and applies initial port values; `/n_free` on that group tears the instance down (and frees its private buses); per-voice spawning is `/graph_voice` (or, more usually, MIDI notes — see below) instantiating the per-voice sub-graph inside the instance group, wired to its shared internal buses. **Execution order** inside an instance group reuses M12's bus-analysis + opt-in auto-ordered groups (a GraphDef instance can be auto-ordered, so the client doesn't micro-manage `/n_before`). **Persistence**: reuse M16's defstore — a `graphdefs/` subdir, the JSON spec as the transparent source of truth, reloaded and re-validated at startup; no bitcode layer (GraphDef references other defs, which carry their own Faust cache). `--no-persist` and the data-dir resolution as in M16. **MIDI-bindable on equal footing**: because a GraphDef exposes the same actuation surface as a def — named `f32` ports + a voice model — `/midi_bind channel graphname …` works unchanged (M17), which is exactly the bridge to M19. **Client side**: a `GraphDef` builder (`defs/graphdef.py`) composing member synth/faust defs + bus wiring + the parameter surface into the JSON, sent via `Server.add_graphdef` (RT `/done`, NRT scored at t=0) — a thin JSON builder like `FaustDef`/`SynthDef`, landing with this milestone (no separate client milestone needed). **RT-safety**: all of GraphDef lives on the network thread / at boot, building the same pre-built engine commands as `/s_new`+`/n_map`+group moves; `process_block` is untouched. Decisions taken with the user (2026-06-19): general container (shared + per-voice in one def); a **named parameter surface** mapped to inner controls/buses so external actuation never addresses inner node ids. Closing includes `schemas.md` (`/d_graph`/`/graph_new`/`/graph_voice`, the reserved id range for graph-spawned nodes), `architecture.md` (the instantiation/teardown + private-bus lifecycle), `GUIA.md`, and a commented `examples/`.

- ✅ **M19 — MIDI-standalone operation: a playable server with no programming environment** *(**DONE 2026-06-20** — see LOG.md. Persisted MIDI bindings (`midi.json`, rewritten on every `/midi_bind`/`/midi_unbind`/`/midi_map`, restored at boot via `CmdTranslator::restore_binding`); boot order defs→graphdefs→bindings→boot preset in `attach_store`; optional `boot.json` preset of standalone GraphDefs (`BootInstance`); playable-by-default confirmed. `tests/midi_standalone.rs`/`persistence.rs`/`midi.rs`, `examples/midi_standalone.sh`)*: the payoff of M16 + M17 + M18 — boot the server and play it from a MIDI controller or DAW with **zero OSC programming**. Today `/midi_bind` needs the def already loaded *and* the binding issued over OSC each session; M19 makes both survive a restart and come up wired at boot. Pieces: (1) **Persist MIDI bindings** — store the `/midi_bind` config (channel → def/graph name + target group + control map) in the data dir (a `midi.json` or `bindings/` next to `synthdefs/`/`faustdefs/`/`graphdefs/`), written on every `/midi_bind`/`/midi_unbind`/`/midi_map` mutation and reloaded at startup *after* the defs and GraphDefs are in place (so a binding's referenced name resolves), under the same `--no-persist`/data-dir rules as M16. (2) **Boot wiring / preset** — an optional startup preset naming which GraphDef instances to instantiate and the default group layout, so a fresh `--midi` server comes up already wired *and* bound (e.g. `--load <preset>` or a `boot.json`); without a preset it just restores the bindings and waits for notes. (3) **Playable-by-default maps** — confirm the M17 named conversion helpers + default note→`freq` / velocity→`amp` / note-off→`gate`/`/n_free` maps make a freshly-restored binding immediately playable, so the minimal workflow is "drop a def + a GraphDef + a binding in the data dir, start `clausters --midi`, play" with no client at all. No new audio-thread state: persistence and boot wiring are network-thread/boot-time, emitting the same pre-built commands. This is the server-side counterpart of the client's OSCFunc/MIDIFunc (clients/PLAN.md C13): the server can be played *directly* by MIDI (M17/M19) **or** by a client that listens to MIDI and emits OSC — both coexist. Closing includes `schemas.md` (the persisted-binding format + boot preset), `architecture.md` (boot order: defs → graphdefs → bindings → preset), `GUIA.md` and an `examples/*.sh` recipe (data dir + a controller + play).

- ✅ **M20 — Documentation split: two mdBooks (server + Python client) + generated API** *(**DONE 2026-06-21** — see LOG.md. New Python client mdBook `clients/python/docs/` with its API reference generated from docstrings by pydoc-markdown (`pydoc-markdown.yml`, static AST parse — no cdylib); ~200 RST roles converted to plain Markdown across all 32 modules; milestone labels (Mx/Cx/Fx) removed from every published doc and docstring (kept only in PLAN.md/LOG.md); `docs/clients.md` rewritten as the cross-language map that links the Python book; two `.readthedocs.yaml` (`build.commands`, slugs `clausters`/`clausters-python`); doc-build guides in both READMEs)*: extends M15 — the English docs become **two platform books, unified by content but published separately**. The server/workspace mdBook (`docs/`) keeps the OSC/architecture reference; the Python client gets its own mdBook (guide + docstring-generated API) — the documentation deliverable of the C12 Python client — cross-linked, both Markdown and ReadTheDocs-deployable. Conventions set here: no Sphinx/RST directives in docstrings, no milestone labels in any published doc, `GUIA.md` stays a personal file out of the docs. **Not published yet** (still in development); the two ReadTheDocs projects remain to be created.
- **M21 — Master clock anchor over OSC (shared, drift-free time reference)**: let a Clausters server act as the **master clock** for several OSC clients so their timing is mutually coherent and drift-free. The server publishes its OSC time anchored to its sample counter: extend `/clock.reply` to carry, captured in the same read as the counter, the server's OSC/NTP time — `/clock.reply (sample:int64, rate:double, osc_time:timetag)`, the anchor `(T0=osc_time, S0=sample, rate)` (appended, backward-compatible with clients that read just `sample, rate`). A client converts an event's logical OSC time to a master sample with `S0 + (T − T0)·rate` and schedules it via `/sched`, so every client shares **one** drift-free sample axis — the SuperCollider/MIDI guarantee (jitter-free *relative* timing) made robust against the wall-vs-audio drift the per-bundle NTP path can reintroduce; routine starts stay arbitrary. Local embed/shm clients read the counter directly and skip the anchor. Rate: publish the **actual** rate; clients may re-anchor periodically (the existing `UdpSampleClock` least-squares model) to track ppm drift on long sessions — exact for short LAN runs, honest for long ones. No audio-thread change: `/clock` is network-thread and the anchor is one atomic read of (counter, system OSC time). Pairs with client **C14**; the phase-alignment layer is **M22**. Closing: `schemas.md` (extended `/clock.reply`), `sample-clock.md` (master-clock model + anchor), `GUIA.md`, cross-link to the Python book.
- **M22 — Shared transport: a queryable master beat grid (phase alignment)**: a small server-hosted **transport** so independent clients align on the *same* beats, not just share a drift-free axis (M21). M21 gives a common sample axis, but each routine still starts at an arbitrary point on it; phase alignment needs a shared **beat grid** (an origin sample = beat 0, plus a tempo) any client can join. Add a queryable/settable transport — `/transport.reply (origin_sample:int64, tempo:double, …)` plus a setter to define/reset it (who may set it is a policy decision: first client, or an explicit owner). A client reads the transport and quantizes its routine start to the next beat boundary on that shared grid, so several clients hit beat 1 together; because the grid lives on the master sample axis, the alignment is **sample-exact** when `lock_to` a master and beat-accurate (drift-bounded) in plain OSC mode. Deliberately a **separate, optional layer** over M21 — a client can use the shared reference without joining a transport. Pairs with client **C15** (`quant` + `clock.join_transport(server)`). Open when tackled: whether the transport also carries play/stop + a running/free distinction or stays a pure origin+tempo grid. Closing: `schemas.md` (`/transport`), a `sample-clock.md`/feature page (beat-grid model + how quant aligns clients), `GUIA.md`, example.

### Reviewed ideas: what was dropped and why

- **Denormals** (from the memory/efficiency idea): already implemented post-M7
  (`dsp::denormals::flush_to_zero()` + `-ftz 2` + `tests/denormals.rs`);
  only the skill/documentation part was missing, absorbed by M10.
- **Original F5**: Faust's native polyphony dropped with the rationale in F5
  itself (above); `waveform`, interpreter backend and Signal API are kept, with
  the interpreter tied to the M14 wasm target. (`soundfile` was also dropped
  originally, but the decision was reversed on 2026-06-20 — it now reads server
  buffers directly; see F5.)
- **Faust UI**: the implementation is considered correct — using Faust's labels
  as control names is deliberate (the author of the def picks the names, as in
  the UGen JSON) —; what was pending was documenting the rationale (M9) and the
  generalization "params fed by control buses" (M11).
- **Plugin API**: documenting the internal API yes (M9); dynamic plugins
  not for now — Rust has no stable ABI and scsynth's historical problem
  confirms the cost of maintaining that boundary. The mitigation
  (versioning the binary boundary) is applied where the boundary truly exists:
  the embedded mode's C ABI and the M14 segment layout.

## Testing strategy

- **Per-UGen unit tests**: offline render of N blocks, asserts on the signal
  (frequency via zero crossings, RMS, impulse response for filters).
- **Golden files**: NRT mode (M7) renders scenes to WAV and compares with
  tolerance against versioned reference files.
- **RT-safety**: `assert_no_alloc` wraps the callback in test builds; CI runs
  the heaviest graph under that condition.
- **OSC integration**: tests that bring up the server on an ephemeral port and
  talk to it with `rosc` from the test; verifiable by hand with `oscsend` or with sclang
  pointing a `Server` at our port. See the `audio-testing` skill.

## Project skills

- `.claude/skills/realtime-audio` — RT thread rules, lock-free patterns, cpal.
- `.claude/skills/scsynth-osc` — reference for scsynth's OSC protocol and node-tree
  semantics.
- `.claude/skills/ugen-dsp` — the UGens' DSP algorithms (oscillators, filters,
  envelopes) with their formulas.
- `.claude/skills/audio-testing` — how to test audio without ears: NRT, golden files,
  signal asserts, no-alloc.
- `.claude/skills/faust-embedding` — embedding libfaust: C API (box/signal/LLVM),
  factory/instance lifecycle, RT boundaries, JSON→Box API mapping.

## Project notes

- The progress made in each milestone is added to the project notes.
- Closing a milestone always includes, where applicable: the developer
  documentation (`docs/architecture.md`, module docs), the user
  documentation in `docs/` for new features, the manual-test steps and
  counts in `GUIA.md`, and an explained example in `examples/` if the feature
  is user-facing — not just the code and LOG.md.
