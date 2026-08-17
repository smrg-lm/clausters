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
   buses, buffers, SynthDefs, commands `/synth_new`, `/node_set`, etc.) but our own
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
- **NRT thread**: reading/writing audio files for buffers (`/buffer_read`,
  `/buffer_write`), like scsynth's "NRT thread".

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

The full, current command set is the canonical wire reference in
`docs/schemas.md` (the single source of truth; the `scsynth-osc` skill is the
working map). It is no longer restated here — this plan tracks *what to build
next*, not the shipped protocol.

## Initial UGens

Oscillators: `Sine`, `Saw` (PolyBLEP), `Pulse`, `WhiteNoise`, `Phasor`.
Filters: `LPF`/`HPF` (biquad), `OnePole`, `Lag`.
Envelopes/control: `EnvGen` (with done actions: free self, like scsynth), `Line`.
I/O: `Out`, `In`, `ReplaceOut`. Buffers: `PlayBuf`, `BufRd`. Math: binary/unary
operators between signals. See the `ugen-dsp` skill for the algorithms.

## Milestones

- ✅ **M0 — Skeleton** *(done 2026-06-10)* — cpal opens the device and a hardcoded sine plays; the `server`/`dsp`/`osc`/`node` module layout.
- ✅ **M1 — OSC server** *(done 2026-06-10)* — UDP on port 57110 over rosc; `/server_status`, `/server_quit`, `/server_notify`, `/server_dumpOsc`.
- ✅ **M2 — RT-safe FIFO + node tree** *(done 2026-06-10)* — command/garbage ring buffers and a `NodeTree` with groups; `/synth_new`/`/node_free`/`/node_set`, guarded by `assert_no_alloc` in the callback.
- ✅ **M3 — SynthDefs** *(done 2026-06-10)* — the serde def format, the UGen-graph interpreter, `/def_send synth`, named/indexed controls, and the `Box<dyn SynthNode>` trait (the F-fork prerequisite).
- ✅ **M4 — Buses and order** *(done 2026-06-10)* — audio/control buses, `In`/`Out`, nested groups, `/synth_new` add actions, `/node_before`/`/node_after`, the `/group_*` family, `/bus_set`/`/bus_get`, `/node_start`/`/node_end`; output is purely `Out`-driven.
- ✅ **M5 — Buffers** *(done 2026-06-10)* — the buffer pool + NRT thread, `/buffer_alloc`/`/buffer_read`/…, `PlayBuf`/`BufRd`, async `/done`; buffers are immutable `Arc`s swapped by the engine and freed via the garbage FIFO.
- ✅ **M6 — Sample-accurate scheduling** *(done 2026-06-10)* — a timetag-ordered bundle queue on the audio thread, NTP→samples on the network thread, and intra-block splitting at the event's sample (no `OffsetOut` needed).
- ✅ **M7 — NRT mode + golden tests** *(done 2026-06-11)* — offline WAV render on the same engine (`--nrt`, scsynth score format), golden regression tests and graph benchmarks.
- ✅ **M8 — The sample clock as the client's timebase** *(done 2026-06-12)* — `/clock_query` exposes the sample counter and `/sched_at <target> <blob>` schedules directly in samples, so a client models the OS-vs-DAC drift for exact relative timing; NTP and sample-clock clients coexist on the same queue.

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

- ✅ **F0 — Toolchain and minimal FFI** *(done 2026-06-10)* — libfaust with the LLVM backend and a hand-written FFI over the C API behind the `faust` feature; smoke-tested against `SinOsc`.
- ✅ **F1 — Compiler thread** *(done 2026-06-10)* — a dedicated compile thread with a refcounted factory table and async `/done`/`/fail` for `/def_send faust` (JSON blob; `/def_send synth` stays the UGen format).
- ✅ **F2 — JSON → Box API schema** *(done 2026-06-10)* — the schema (composition/math/delays/UI-as-controls), a validating JSON→C-API interpreter with path-carrying errors, and stdlib access via `DSPToBoxes` source fragments.
- ✅ **F3 — FaustSynth in the tree** *(done 2026-06-10)* — `FaustSynth: SynthNode` wrapping the JIT instance, instantiated on the network thread, params by name via UIGlue zones, freed under factory refcount.
- ✅ **F4 — Parity and interop** *(done 2026-06-10)* — Faust and UGen synths coexist in groups/buses, golden-tested equivalent graphs, a Python JSON example, schema docs.
- ✅ **F5 — Extensions** *(done 2026-06-12)* — kept `waveform` embedded tables, the interpreter backend (for the wasm target) and the Signal API; native Faust polyphony dropped (the node tree is the voice allocator); `soundfile` initially dropped then reversed (reads server buffers directly). Rationale in "Reviewed ideas" below.

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
don't reach milestone status (more UGens — `Saw`/`Pulse`/filters, and `EnvGen`
with done actions, **done** 2026-07-01 —, `/group_queryTree`, buffer
streaming) are taken as loose items when needed. (Historical detail for the
completed items lives in the git history.)

- ✅ **M9 — Developer documentation** *(done 2026-06-12)* — `docs/architecture.md` (threads, modules, memory lifecycle, invariants, "how to add a UGen") plus a rustdoc pass; records the Faust-UI-as-controls and the no-dynamic-plugins (no stable Rust ABI) decisions.

- ✅ **M10 — Bounded memory and alignment** *(done 2026-06-12)* — audited and documented every pre-allocated capacity and its full-behavior, cache-line-aligned wire/bus blocks, and updated the `realtime-audio` skill.

- ✅ **M11 — `/node_map`/`/node_mapAudio`: buses as a parameter source** *(done 2026-06-13)* — a node reads a control (or audio) bus into a control each block until `/node_map … -1` or `/node_set`; an RT-safe per-node mapping table over the existing bus atomics, schedulable like `/node_set`.

- ✅ **M12 — Canonical graph form via bus connections** *(done 2026-06-12)* — infer the read/write dependency DAG from the buses and offer opt-in auto-ordered groups (`/group_sortMode`, recomputed on the network thread; `/group_queryTree`/`/group_dumpGraph`; manual moves `/fail` inside them); feedback cycles keep the explicit order (one block of delay). No plugin delay compensation yet — a natural later addition via a `latency()` on the trait when an intrinsic-latency UGen lands (rationale in `docs/model-vs-daw.md`).

- ✅ **M13 — Parallel tree processing** *(done 2026-06-12; requires M12)* — the M12 DAG drives stage-based parallelism (N−1 RT workers, bounded spin+backoff, no locks); same-stage nodes write disjoint buses so it stays bit-identical to sequential, and NRT renders benefit too.

- ✅ **M14 — Pluggable transports, embedded mode and synchronous calls** *(done 2026-06-12)* — OSC decoupled from transport (UDP / shared-memory ring / in-process), a shared data plane (sample clock + control buses), a client-side synchronous facade, and the versioned-C-ABI cdylib that lets any language embed the server (the "only flat data crosses" boundary).

- ✅ **M15 — Comprehensive English documentation (README + mdBook + rustdoc)** *(done)* — a root README, the mdBook over `docs/` in place (`book.toml src="docs"`), and an oriented crate doc-comment, covering the OSC user, the library/embed user and the developer.

- ✅ **M16 — On-disk def persistence + bitcode cache** *(done 2026-06-16)* — defs persist to a data dir as transparent JSON (layer B) recompiled on reload, plus a non-authoritative libfaust bitcode cache (layer A); `--data-dir`/XDG resolution, `--no-persist`, incremental reload on the compiler thread.

- ✅ **M17 — MIDI: server protocol and client output (reusable Rust core)** *(done 2026-06-18)* — the `clausters-midi` crate (versioned C ABI, MIDI 2.0/UMP via `midi2`, SMF via `midly`, live ports via `midir`): server-side `/midi_bind` maps a channel to an instrument def + control map (SynthDef and FaustDef actuated identically, a MIDI voice byte-identical to the OSC path), and the client exports live ports and `.mid`/MIDI-2.0-clip files. This is client `C11`. Rationale (MIDI-2.0 resolution, native-crate decision, `midi2-clip` stub) in `docs/decisions.md`.

- ✅ **M18 — GraphDef: persistent node-graph definitions ("programs")** *(done 2026-06-19)* — a third persistent def kind saving a whole configuration of bus-wired nodes with a named parameter surface and a shared/per-voice split (`/def_send graph`/`/graph_new`/`/graph_newVoice`); execution auto-ordered via M12, persisted like M16, MIDI-bindable like a def, with a `defs/graphdef.py` client builder.

- ✅ **M19 — MIDI-standalone operation: a playable server with no programming environment** *(done 2026-06-20)* — persisted MIDI bindings and an optional boot preset restored at startup (order: defs → graphdefs → bindings → preset), so a `--midi` server boots already wired and playable with no client; the server-side counterpart of the client's responders (C13).

- ✅ **M20 — Documentation split: two mdBooks (server + Python client) + generated API** *(done 2026-06-21)* — the English docs became two platform books (server `docs/`; the Python client's own book with a pydoc-markdown API page), cross-linked; no RST directives, no milestone labels in published docs.
- ✅ **M21 — Master clock anchor over OSC (shared, drift-free time reference)** *(done 2026-06-22)* — `/clock_query.reply` carries the server's OSC time anchored to its sample counter, so several OSC clients convert logical time to one common drift-free sample axis and schedule via `/sched_at` (embed/shm read the counter directly). Pairs with client C14.
- ✅ **M22 — Shared transport: a queryable master beat grid (phase alignment)** *(done 2026-06-22)* — a server-hosted `/transport_set` (origin sample + tempo) clients join to quantize starts onto one shared grid: sample-exact when locked to a master, beat-accurate otherwise. A separate optional layer over M21; pairs with client C15.

- ✅ **M23 — Continuous integration + publishing (CI, Read the Docs, PyPI)** *(done 2026-07-05)* — a CI workflow (fmt/clippy, `cargo test` over the def-family feature matrix, the gui + wasm gate, the Python suite, both mdBooks, and a from-source libfaust job) and a `v*`-tag release workflow (self-contained wheel + server tarball, PyPI Trusted Publishing). Account-side activation steps recorded in `docs/contributing.md`.
- ✅ **M24 — Real-time health: RT scheduling, CPU metering, affinity, stress harness** *(done 2026-07-08)* — the `rtprio` default feature (RT-scheduled callback with a ground-truth policy diagnostic), an RT-safe CPU meter (avg/peak/late-blocks in `/server_status`), experimental `--pin`, and `examples/stress.rs`; a SIGXCPU guard degrades instead of dying under overload. The opt-in→default reversal is in `docs/decisions.md`.
- ✅ **M25 — TCP as the default command transport + a configurable frame ceiling** *(done 2026-07-12)*: the server accepts TCP alongside UDP **by default** (same port 57110, scsynth-style; `--no-tcp` opts out, `--tcp [port]` still moves it), and the fixed 64 KiB `MAX_FRAME` becomes a boot option (`--max-frame <bytes>`, default 16 MiB) shared by the TCP and WebSocket fronts — with length-prefixed framing the ceiling is a DoS guard, not a protocol limit, so it is configuration, sized for the target deployments (loopback + controlled networks, not a public service). Replies become **transport-aware**: a stream client (TCP/WS/ring) may receive frames up to the ceiling (`/buffer_getRange` chunks, `/group_queryTree`, the `/bus_tapStream` window clamp), while a UDP client keeps the datagram cap; `/server_query` advertises the ceiling so clients size their requests instead of hardcoding it. UDP itself is untouched — it remains the discovery/boot protocol and carries small real-time control fine; the IPC command rings keep their 64 KiB (big payloads ride TCP even locally; a ring-size option would bump the versioned segment layout and is deferred until a real need). Every network front stays **individually optional** (a packaged desktop/mobile standalone runs the embedded server over the in-process link and needs no sockets at all), and no limit is hard-wired: whatever bounds a payload is a boot option with a sensible default, never a constant — the project must stay usable as a desktop or mobile application without arbitrary ceilings. Rationale — the transport roles: UDP = discovery + small control, TCP = the command plane, shm = the data plane, WS = the browser's TCP — goes to `docs/decisions.md`. Pairs with client C34 and GUI host G25.
- ✅ **M26 — Network-edge hardening: fuzzing the decode door + bounding the stream fronts** *(done 2026-07-16)*: the pre-publication hardening pass over the transports. A **cargo-fuzz harness** (`fuzz/`, nightly-only, not a workspace member) fuzzes `osc::decode_packet` — the single door every transport funnels through, so one target covers the whole inbound parse surface — from a small versioned seed corpus; run it before releases that touch the OSC path (recipe in `docs/contributing.md`). The stream fronts get **edge guards**, all shared between TCP and WebSocket: a `--max-clients` ceiling on concurrent connections (default 64, scsynth's `maxLogins` in spirit — each connection costs a thread, so the count is bounded like every other boot-time pool; a connection past it is dropped at accept, a freed slot is reusable), **bounded inbound queues** (a flooding client blocks on TCP flow control instead of growing server memory — no rate limit: dense control traffic is legitimate, the bound is resources, not message rate), and **slow-consumer eviction** on the reply path (a TCP write timeout so a client that stopped reading cannot stall the single-threaded command loop; a bounded WebSocket reply queue whose overflow drops the connection). UDP is untouched — connectionless, datagram-capped, the kernel sheds overload. Size ceilings were already M25's `--max-frame`; this closes the count/backpressure half.

- ✅ **M27 — The curated PV set: parameterized operations, not a catalog** *(done 2026-07-18)* — grow the `PV_*` vocabulary to cover the musically common spectral operations **without porting scsynth's one-UGen-per-op catalog** (whose sc3-plugins tail shows where that leads: dozens of near-duplicate plugins freezing booleans into names — `PV_MagAbove`/`Below` are one algorithm and a flag, which our `PvMag`/`MagMode` already demonstrates). Four additions, each one *implementation* with modes: (a) extend `PvMag` with a `clip` mode and register `PV_MagClip`; (b) a **two-chain combiner** — one binary PV implementation whose operator is a parameter, registered under the scsynth-compatible names (`PV_Add`/`PV_Mul`/`PV_Min`/`PV_Max`/`PV_MagMul`/`PV_CopyPhase`); needs the compiler to let one spectral UGen take **two** chain slots (result lands in chain A, the wire keeps ordering, both `SpectralChain`s stay synth-private); (c) the **stateful pair** `PV_MagFreeze`/`PV_MagSmear` (per-instance frame memory, allocated at build); (d) one **bin-remap** implementation behind `PV_BinShift` (shift + stretch covers `PV_MagShift`). Anything beyond these waits for M29's general mechanism rather than joining a catalog — record that stance in `docs/decisions.md`. Tests extend `tests/spectral.rs`; a commented example in `examples/`; wire reference in `docs/schemas.md`.

- ✅ **M28 — Partitioned convolution: one UGen, kernel prepared off the RT thread, flat load** *(done 2026-07-18)* — a single well-parameterized convolution UGen instead of scsynth's five variants (`Convolution`/`2`/`2L`/`3`/`StereoConvolution2L`), designed around the two constraints the `bench` spectral section quantifies: the FDL multiply–accumulate is the dominant per-hop cost (~217 µs for a 2 s IR at 48 kHz, 16% of a block budget), and it need not land on one block. Pieces: (a) kernel spectra **precomputed off the audio thread** into an immutable pool buffer by a typed `/buffer_gen` routine (the moral heir of scsynth's `PreparePartConv`, per the S-track `/server_cmd` stance) — the RT side only ever FFTs its input block and MACs against ready-made spectra, and a kernel swap is an `Arc` swap with a parameterized crossfade (subsuming `Convolution2L`; no re-FFT on the audio thread, which is where scsynth's `Convolution2` violates our rules); (b) **load spreading**: the P partition MACs distributed across the hop's blocks so the steady-state cost is flat (~P/blocks-per-hop partitions per block), leaving only the input FFT/IFFT pair on the hop block; (c) convolution runs **outside** the `fr` chain (its discipline — zero-padded rectangular segments, hop fixed by partition size — is incompatible with the windowed COLA analysis chain; same reason scsynth keeps them apart); (d) it is the first UGen with **intrinsic latency**, so add the `latency()` hook on `SynthNode` anticipated by M12 and report it (full PDC stays deferred per `docs/model-vs-daw.md`); a direct time-domain path for short kernels can come later as a degenerate partition case. Acceptance: a golden test against direct convolution, and a bench row showing the spread MAC flattening the peak-block column.

- ✅ **M29 — A general per-frame spectral mechanism (design spike first)** *(done 2026-07-18: spike + implementation)* — the long-term answer to the PV-catalog problem: make the spectral frame **user-programmable** so new bin operations stop requiring server releases. Two candidate designs, to be decided in a written spike before any implementation: (a) **bin algebra** — expose magnitude/phase as frame-rate values the existing S3 operator vocabulary composes (the Max/MSP `pfft~` model: the spectral domain as a substrate the graph itself processes; touches the compiler and the rate system); (b) a **JIT per-frame kernel** — a compiled callback over the frame via the existing Faust family patterns (compile on the network thread, RT-safe run; no registry growth). The spike weighs compiler surface vs. JIT dependency, NRT determinism, and the client-side authoring story, and lands as a `docs/decisions.md` entry; implementation follows only on real need (every M27 op we decline to add is this milestone's demand signal). **Outcome**: the spike surfaced and chose a third design that dominates both on every axis — a **bin-expression program**: one `PV_Kernel` UGen interpreting a compile-time-validated postfix program over per-bin values, opcodes riding the `clausters-core::builtins` op table, authored client-side with (a)'s operator algebra; no new rate, no JIT, exact NRT, full feature matrix. (b) is repositioned as the escalation path for kernels beyond a per-bin map. Recorded in `docs/decisions.md` — and **implemented the same day**: `clausters_core::pvprog` (program + RT-safe evaluator), the variadic `PV_Kernel` row, `mag_expr`/`phase_expr` wire fields validated at `/def_send synth`, the Python `clausters.defs.pv_expr` symbolic terms + `pv_kernel`, sample-identical equivalence tests against `PV_MagAbove`/`PV_BrickWall` in `tests/spectral.rs`, RT-safety coverage, `examples/spectral_kernel.py`, and the use-cases/restrictions docs in both books.

- ✅ **M30 — The introspection verbs: what a running server holds** *(done 2026-07-19)* — the retrieval surface a client palette needs, three queries in the `/server_query` mold, adding **no** node/def semantics anywhere. **`/def_query [name...]`** → one `/def_query.reply` per def then `/done`, listing the loaded defs with `name, family` (`synth`/`faust`/`graph`) and their control surface (`name, default, rate`); a faust def appends each param's `min, max, step`, a graph def reports its surface **ports** with the inner `member, control, mul, add` targets each drives, and an unknown name comes back with an empty family rather than failing the batch. **`/buffer_query` with no argument** → one `/buffer_query.reply` listing every allocated buffer in the existing four-arg shape. **`/ugen_query [kind...]`** → one `/ugen_query.reply` per UGen then `/done`: arity (`-1` variadic), default/allowed rates, exec/bus/op/spectral roles, and the **named inputs with defaults**. That last field did not exist — `UGenDescriptor` carried only a count — so the descriptor grew `inputs` and all sixty rows were filled by reconciling the `docs/schemas.md` catalog table with the Python callables' signatures; the wire stays positional and no def changes behavior, the names being descriptive metadata a palette labels an inlet with (rationale, and why `ugens.py` stays hand-written behind a contrast test instead of generated, in `docs/decisions.md`). Multi-reply batches close with `/done "<command>"` so an argument-less query has an end marker, and each item is its own message because the payloads are variable-length and a whole catalog would outgrow a UDP datagram. A build without `synth` has no catalog and replies with an empty listing, not a failure — Faust has no UGens, only FaustDefs, and its box vocabulary stays client-side. Python: `Server.query_defs()` / `query_buffers()` / `query_ugens()` returning `DefInfo`/`BufferInfo`/`UgenInfo` (named in the existing `query_*` family — a bare `buffers()` would have shadowed the `BufferAllocator` attribute), the anti-drift contrast test against `ugens.py`, and `examples/introspect_server.py`. Pairs with GUI P2.

- ✅ **M31 — What a client needs to own its data paths: writing a buffer, and an identity on the ring** *(done 2026-08-04)* — two gaps the web client's W10 uncovered while opening the read paths to a script. Neither is a defect in something that shipped: both are capabilities a client turns out to need and the server has never offered, and both bound what a client-drawn view (an audio editor, a scope) can do. They ride together because they are the same sentence from the client's side — *a client should be able to read and write its own data without fighting another client for the channel* — and because doing them at once spends one breaking release instead of two.

  **(a) A buffer-write command.** The `/buffer_*` family has no write: it reads with `/buffer_get`/`/buffer_getRange` and has no command that puts samples back (scsynth's `/b_set`/`/b_setn` were commands as well as replies; ours are replies only, and now spelled as such). So a client can read a buffer's samples but never put samples into one — it can only ask the server to fill it (`/buffer_gen`, `/buffer_allocRead`, `/buffer_read`) or, when it shares memory with the engine, install them through the embed door (`buffer_load`, what the browser's in-page carrier uses). This is what an audio-editor view needs to close its read → edit → write cycle, and why W10's bulk path is read-only and its `Buffer.load` is in-page only. To settle: the install path (the NRT queue `/buffer_gen` already uses, so a write never touches the audio thread), the semantics (scsynth's `bufnum start count values…`, repeated, plus a single-sample form), how a multi-megabyte edit is chunked against the `--max-frame` ceiling, whether the write is asynchronous with a `/done` or synchronous on the mirror like `/buffer_getRange`, and whether an edited buffer needs any notification for other readers.

  **(b) Ring clients get identities.** Every packet arriving through the shared-memory / in-process ring is `ClientId::Ring` — a *single* client (`src/osc/mod.rs`), which `docs/ipc.md` already names as future work ("the transport keeps one ring client per segment … multiple ring clients … are explicitly future work"). The per-client subscriptions therefore collide: `/bus_stream` and `/bus_tapStream` are "one per client, replaced on each call", so two independent readers over one ring silently take the stream from each other. **The evidence**, from a browser page where the script and the GUI host both push through `engine.send`: the script subscribes `/bus_stream(20, bus 0)` and gets its snapshots; the host opens a `meter` and sends `/bus_stream(33, bus 1)`, after which the script receives **nothing**; the script re-subscribes and takes it back. The loss is **permanent in one direction** — the browser host only re-sends when its own wanted set changes (`clients/gui/src/host/web.rs`, `sync_bus_stream`), so once a script replaces the subscription the host's meters and scopes stay frozen until a widget is added or removed. Over sockets there is no such problem (a native host and a script are different `ClientId`s), so this is specifically the shared ring — and it also means two `BusStream`s in one page collide with each other. To settle: where a sender tag lives (a ring frame-header field, which moves the segment layout, or several rings), how the embed and wasm doors carry it (`clausters_send`, `WebServer::send`), whether replies stay broadcast on the shared reply ring (they can — the identity is needed for subscription bookkeeping, not for reply isolation), and what a peer built against the old layout sees. A page-side arbiter merging the two demands into one subscription was considered and set aside: it is a workaround for a missing server capability, and it would leave the same trap for every other ring embedder.

  **A possible optimization, noted from (a) and deliberately not taken there.** The *read* path still costs one round trip per chunk — 623 ms for 200k samples over the shared-memory carrier, against 121 ms for the write of the same samples once its chunks were batched behind one `/server_sync`. That asymmetry is inherent to request/reply: a write is fire-and-forget with a barrier at the end, while every read chunk has to wait for the reply that carries its data. It could be pipelined — send every `/buffer_getRange` at once and collect the replies as they arrive — but that needs reply-matching machinery neither client has (a request today is one send and one await), so it belongs to whoever opens that layer rather than to a buffer-write milestone.

  **Versioning.** (b) moves the segment layout, so it bumps `ABI_VERSION` (6 → 7) and, by the linkage rule, the SemVer breaking tier (the minor, pre-1.0) — the package version itself moves when the release is cut, not here. (a) is additive on the wire and bumps neither by itself.

  **What shipped.** (a): `/buffer_set` (single samples by flat index) and `/buffer_setRange` (runs), on the NRT queue like the rest of the writing family, laid into a copy that replaces the buffer whole; a range past the end **fails** rather than being clamped, since a short write would lose data the caller believes it stored. Two things turned up while building it and are recorded in `docs/decisions.md`. First, bulk samples ride as a **little-endian `f32` blob**, not as float arguments — 200k samples took 2.7 s as typed arguments against 0.1 ms as one blob — so `/buffer_getRange.reply` changed to match, and the rule (payload scaling with the audio → blob; with the parameters → typed arguments) is written in `docs/schemas.md` rather than re-derived. Second, a job rebuilding a buffer from its contents snapshotted the network-side mirror at *parse* time, which is wrong for a batch: every chunk copied the same pre-batch contents and the last install erased the rest, so the NRT queue now keeps its own view of what it last produced per index. With that safe the chunks close with **one** `/server_sync` instead of a `/done` each. Clients: `Buffer.set_samples`/`set_sample` and `setSamples`/`setSample`, each with one shared pack/unpack (`clausters.base.bulk`, `src/base/bulk.ts`) so the endianness check has one owner. Example `clients/python/examples/buffer_edit.py`.

  (b): each ring frame carries a `u32` **peer tag** beside its length — who authored the packet inbound, who the reply is for outbound — and `ClientId::Ring` becomes `ClientId::Ring(u32)`. The tag lives in the *frame*, so no header or data-plane offset moved and the hand-written readers (`clients/gui/src/host/shm.rs`, `clients/python/clausters/ipc.py`) needed only their version constant; several rings were considered and set aside for moving the layout and fixing a client count at boot. SPSC survives because the tag is about the packet, not the ring: a multi-client embedder funnels sends through one producer and demultiplexes replies by tag, which is what the page already did through its one `MessagePort`. Tags are the embedder's to assign (no handshake; peer 0 is the single client a segment always had, so existing embedders are untouched), and the C ABI stays deliberately single-peer since its one consumer is itself one client. Replies being addressed removed the page-wide eavesdrop, so the web client grew an explicit `ANY_PEER` **read** door for observers, which two page tests now use rather than reaching into another client's internals.

  **Acceptance (met).** A client writes samples into a buffer and reads back exactly what it wrote, in chunks, over a socket and over the ring (`tests/buffers.rs`, `clients/python/tests/test_buffer_io.py`, `clients/web/tests/data-ws.test.ts`), and a batch of writes no longer erases itself (`a_batch_of_writes_does_not_erase_itself`, verified to fail without the queue's chain). Two ring peers keep their own `/bus_stream` subscriptions (`tests/ipc.rs`), and on one page over the in-page carrier a GUI host `meter` and a script `BusStream` on different buses **both keep updating**: the probe that used to demonstrate the collision is now the acceptance, `clients/web/tests/ring-peers.html`, in that client's suite — and verified to reproduce the original failure exactly (phase 2 leaves the script at 0 snapshots, and the host never recovers) when both peers share a tag.

- ✅ **M32 — Watching an audio bus is asking for a bus** *(done 2026-07-28)* — `/bus_tap tapIndex bus` made the caller allocate one of the segment's eight sample rings, route the bus into it, and carry that index everywhere a view needed it, so the server's own bookkeeping leaked into the wire and into every client. It becomes **`/bus_tap bus watch`**: the server picks the ring, publishes the bus → ring mapping in a new per-bus region of the segment (**`ABI_VERSION` 3 → 4**, and by the linkage rule the next release takes the breaking tier), and **counts watches**, so two views of one bus share a ring and the last one to stop frees it; running out fails loudly instead of drawing nothing. `/bus_tapStream` lists **buses** and the subscription *is* the watch, which is what lets a browser client never send `/bus_tap` at all. The same region carries a **per-bus level** — the block peak held with a 20 dB/s decay — so a meter reads one number per block instead of holding a ring: a mixer's worth of meters is now possible, where three stereo channels used to exhaust the region. The decay (rather than a max the reader clears) is what keeps the value correct for a reader an order of magnitude slower than the engine *and* for several readers at once; `tests/rt_safety.rs` guards the added per-block work. Pairs with GUI host G33, which is where the widget surface (`bus`/`rate`/`channels`) and both client ports live. Rationale in `docs/decisions.md`.

- ✅ **M33 — A group is called something** *(done 2026-08-01)* — a group was addressable only by the id its client happened to allocate, which is enough for a node tree and not enough for a console: a DAW channel, a console group, a send are all *named* things, and a client building them out of groups had no way to say which was which. **`/group_new` takes an optional name per group** and **`/group_name groupID name`** renames one: a **referenceable label on top of the id** — the id remains the identity every command addresses and every reply reports, the name is a second way to refer to the same group — unique among siblings, cleared by an empty name, changed by the same command. The label makes the tree navigable by **path**: every group contributes a segment (its name, or its **decimal id** when unnamed, so nothing falls out of addressing), the root is `/`, and **`/group_query <path>`** resolves one to its id, replying `-1` when nothing answers (absence is a state, the `/node_query` rule). A path is never accepted where a node id goes — resolve once, then command by id — which keeps the ~25 node commands' parsing and their hot path untouched. Three rules the server enforces: unique among siblings, never all digits (an unnamed group answers to its id), no `/` (the server composes the path). A name carried by `/group_new` is judged **before** the group exists, against the group it would land in, so a refused label refuses the creation rather than leaving an anonymous group behind. Reported everywhere a node is: `/node_query.reply` after `headID, tailID`, `/group_dumpGraph` quoted next to the id, `/group_queryTree.reply` giving every node the uniform shape `id, count, name`, and the **`/node_start`/`/node_end` notifications** as a last argument — so a client watching the tree learns which channel came up or went away, and for a death there is no query left to make (the mirror keeps a departing group's name as a bounded **epitaph**, since it drops the entry at translation while the death notice waits on the engine). The shapes follow the feature rather than scsynth's, which is not a cost: nothing but our own clients speaks this protocol, and the packages move together. **The name lives only in the network-side `TreeMirror`**: `node::Group` grew no field, no `Cmd` reaches the engine, and the audio thread never sees a name, so the feature has no RT cost by construction. Ports in both clients (`Group("mixer")` / `Group.new(server, {name})`, `rename`, `Server.group_at` / `groupAt`, and the tree/node parsers), `examples/mixer_paths.py`, rationale in `docs/decisions.md`.

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

## T track — The governing transport (a pause that stops time)

Section added 2026-08-02. M22's transport is a shared beat grid plus an advisory
rolling state the server never schedules audio from. T makes it able to *stop
time*: a group bound with `/transport_group` is frozen with its internal state
intact, the transport clock stops, and the queue of anything scheduled against
that clock stops falling due — all at the same sample. Part of the server, not a
Cargo feature: every build has both clocks and both queues.

- ✅ **T1 — The governing transport** *(done 2026-08-02)* — two clocks
  (`transport_now = now - frozen_total`, made two Rust types so an axis cannot be
  swapped silently), two scheduler queues with bundles routed by where their
  messages point, `/transport_group` and `/sched_atTransport`, the transport
  clock in the shm header (ABI v6, in reserved space so no offset moved), a
  `TempoClock` that freezes and thaws, `Transport.resume()` distinct from
  `play()` (MIDI's continue against start), and `form`'s `locatable` — a
  resident generator has no position, so a locate over one refuses and names
  the way out rather than faking it. Proven by cutting the frozen span out of a
  paused render and asserting it equals the unpaused one sample for sample, over
  a seeded-noise def, with a deliberately non-block-aligned pause.

- ✅ **T5 — The transport has a position, and the engine owns it** *(done 2026-08-16; opened the same day, out of the GUI's session mode wanting a playhead and a seek — `clients/gui/PLAN.md`, H5, which consumes this and is written against it. Opened here rather than there on the user's rule: **the server is the only thing that manages playback time**, so anything that computes where a piece is belongs in the server even when a window is what asked for it.)*

  **What is actually missing, and it is not what the name suggests.** T1 gave the transport a rolling state the engine enforces, and `/transport_locate` looks like the seek an editor wants. It is not, and the reason is one fact: **the position never reaches the engine.** `Transport { origin_sample, tempo, playing, position, group }` lives on the network thread (`src/osc/server/mod.rs`), and exactly two commands cross to the audio side — `Cmd::TransportRun { rolling }` and `Cmd::TransportGroup { id }`. A locate sends **nothing**; it changes a number and broadcasts it. So today the server owns *whether* the piece is rolling and not *where* it is, and nothing in a graph can read the piece's time.

  The published clock does not fill the gap either, and the reference currently claims it does. `transportSample` is samples **elapsed** under the transport (the device clock minus the total time spent stopped) — monotonic by construction, so **a locate does not move it**, while `docs/schemas.md` calls it "the time of the *piece*". Elapsed and position are two quantities and only one of them is the piece's time.

  **What follows from that, and is the whole reason this is a milestone**: with no position on the engine's side, every reader invents its own — a `PlayBuf` free-running from frame 0 — and anything an editor wants on top (start here, loop that span, show me where it is) has to be assembled by the client out of arithmetic on a clock. That is a client managing playback time, and it is exactly what the rule above forbids. It also does not scale to the thing this is all for: a multitrack is many readers and **one** time.

  **The shape: a reader follows the transport.** Chosen 2026-08-16 over the alternative of giving each reader its own seek (`PlayBuf` growing scsynth's `trigger`/`startPos`/`doneAction` — a real gap, recorded as **S17**, and deliberately *not* what this rests on). Four pieces:

  - **A position on the engine, in samples**, advancing per sample while rolling, holding while stopped, and **jumping on a locate** — which is the first time `/transport_locate` sends the audio thread anything at all. It is a *second* quantity beside the elapsed count, never a redefinition of it: the transport scheduling queue is on the elapsed axis and needs it monotonic (a jumping axis makes "due" ambiguous), so `/sched_atTransport` is untouched — pinned by a test, since that is the half of "nothing else moved" that code can check. What does change is the reference: `transportSample` is documented as elapsed, and the sentence calling it the piece's time now belongs to the position.
  - **Published in the segment**, because that is how a host reads a clock with no messages. It **fits in the header's reserved space** (`_reserved: [u32; 2]` at offset 56, 8-byte aligned — the same room ABI v6 put the transport clock in), so no offset moves and the out-of-process readers that pin offsets by hand take a number change and nothing else: `clients/gui/src/host/shm.rs`, `clients/python/clausters/ipc.py`, and the web reader. `ABI_VERSION` 7 → 8, with what that drags for the package version read off the `release-versioning` skill rather than decided here. **After this the header has no reserved space left** — the next counter costs a real layout change, and that is worth knowing before it is spent.
  - **Readable from the graph**, so a reader can follow it: a UGen reporting the transport position in frames, per sample at `ar` (a reader needs consecutive frames within a block) and block-constant at `kr`. A governed node is frozen while stopped and does not process at all, so it holds with everything else and reads the new position on the next play.
  - **Loop points on the transport** (`/transport_loop`), by the same rule that puts the position here: a loop that wrapped by having a client locate on each pass would be a client managing playback time once a bar. With the position wrapping in the engine, a loop is seamless and costs the graph nothing.

  **What this dissolves, worth recording because it was a whole discussion.** With the reader following the transport there is no "one shot" to arrange: the transport rolls past the end of the material, the reader clamps or goes silent, and the head keeps moving because time keeps moving — which is what a DAW does. No self-freeing node, no timer, and nothing computing a duration.

  **What it does not take, named so it is not read as included.** A locate does **not** re-arm the transport scheduling queue: jumping backwards over `/sched_atTransport` entries does not make them fall due again, and jumping forwards does not skip them, because that queue is on the elapsed axis by the decision above. Re-arming a queue on a jumping axis is genuine DAW behaviour and a milestone of its own. **T2 is likely dragged in**: a position in samples crosses exactly the beats↔samples conversion whose origin T2 says is still read on the device axis, so where those two meet is settled here or T2 is closed first — decided when this is started, not now.

  **What shipped** *(2026-08-16)*. `PiecePosition` and `PositionAnchor` in `server::clock_axis`, beside the two clocks and typed for the same reason; `Cmd::TransportLocate` and `Cmd::TransportLoop` reaching the audio thread, the first applied at the sample it lands on (the engine grew a `cursor` for that, the precision `frozen_total` already had); the block cut at a loop's wrap, with a wrap on the boundary belonging to *that* block because the position published at a block's end is what the next block's first sample plays; `TransportCtx` on `ProcessCtx`; the `TransportPos` UGen with its `offset`; `/transport_locateSample` and `/transport_loop` on the wire with three appended reply fields; ABI 7 → 8 with the position in the last of the reserved header space and both out-of-process readers following; the builders in Python and TypeScript, and `transport_pos` in the Python UGen vocabulary. The rationale is in `docs/decisions.md`, and `examples/transport_seek.py` is the manual test — four one-second tones, so a seek, a pause and a loop are each audible the moment they happen.

  **What it did not need, which is the finding.** T2 was named as likely dragged in and was not: the beats→samples conversion deliberately ignores `originSample`, because that origin anchors the beat grid on the *device* axis for phase-aligning clients while the **piece's own axis starts at its own 0**. Saying that out loud settled the conversion without touching the grid, so T2 stays exactly as open as it was and about exactly what it was about.

  **What it left open, deliberately.** A transport addressed in samples still needs a beat grid defined before it will roll, because `/transport_play` has always required one — which for an audio editor means calling `/transport_set` with a tempo it never reads. Relaxing that means the rolling state existing independently of the grid (`defined` becoming a flag rather than an `Option`), which is a change to a command that already works and did not belong inside this one. Recorded in "Found by use".

  **Acceptance:** a locate while stopped moves the published position, and the graph reads the new one on play; a reader following the position renders the same samples as the same span expressed in a score and rendered in batch (T1's own technique, over a seeded def, deliberately not block-aligned); loop points wrap with no client in the loop and no discontinuity at the seam; the elapsed clock stays monotonic across a locate and `/sched_atTransport` behaves exactly as before; `tests/rt_safety.rs` stays green. **The packages move together**: `/transport_query.reply` gains the position **appended** (so a client reading only the older fields keeps working), `/transport_loop` and whatever the locate grows get their builders in both clients, and `docs/schemas.md` plus `docs/sample-clock.md` carry the two-quantity distinction — which is the part most likely to be misread, since one of them is called "the transport clock" and is not the piece's time.

Open, not blocking:

- ⬜ **T2 — `/transport_set`'s grid origin on the transport axis.** With a group
  bound, `originSample` is still read on the device axis, so the grid slides by
  the frozen total across a pause. Needs the grid semantics re-derived; no test
  pins it today.
- ⬜ **T3 — Classification is once, at drain.** A bundle scheduled before
  `/transport_group` binds stays on the device queue even if its target becomes
  governed. Documented; re-classifying would mean rewriting a queue on the audio
  thread.
- ⬜ **T4 — `bundle_is_governed` re-resolves the group per message.** A linear
  `find` per targeted message, bounded and allocation-free but `O(messages x
  nodes)` worst case inside a block budget. Hoist the group's index.

## S track — Synthesis-engine infrastructure completion (the substrate for future UGens)

Section added 2026-07-01. The base UGen set and the node/bus/def machinery are in place, but before *growing the UGen library* we finish the **substrate** every future UGen leans on, so that adding a UGen later is a self-contained job (a `process`, a registry entry, a test) with no engine surgery. Everything here is deliberately **infrastructure, not a UGen catalog**: the concrete DSP UGens it enables — the demand family (`Dseq`/`Dseries`/`Dwhite`), `FFT`/`IFFT` and PV_* processing, the table oscillators (`Osc`/`VOsc`/`Shaper`), more filters — land afterwards as loose items (per "Future milestones (M9+)"), each cheap once the substrate exists. Like the F fork, S coexists with the M line and does not replace anything; the pieces are largely independent (S1 enables S2's `ir` controls and the future demand/FFT UGens; S5 depends on S6's `/buffer_gen` slot but is written together; S4 pulls `/node_run` in from S6 because it is tied to pause semantics). Canonical scsynth lists below were **verified against the SuperCollider source** (`server/plugins/UnaryOpUGens.cpp`, `BinaryOpUGens.cpp`, `HelpSource/Classes/Done.schelp`) on 2026-07-01.

**Design stance — compatibility of *model*, not literal copy.** These milestones take scsynth's set as the reference for *completeness* (so no capability is missing), but each is free to adopt a **better implementation consistent with what Clausters already built** where scsynth's is weak — the same "conceptual, not binary, compatibility" principle stated at the top of this plan. scsynth carries historical warts we need not inherit: the clearest example is the plugin-command pair **`/server_cmd`/`/node_ugenCmd`** (S6), which were never cleanly designed (untyped, ad-hoc argument blobs, per-plugin conventions with no schema) — where we need that mechanism we should design a **typed, discoverable** command surface fitting our JSON/OSC conventions rather than reproduce the loose original. Likewise favor our existing machinery (auto-ordered groups, the bus analysis, the network-thread pre-build, the versioned ABIs) over scsynth idioms that predate it. Each S milestone should call out, when it deviates, *why* and *what* it does instead.

- ✅ **S1 — Calculation rates (`ir`/`kr`/`ar`/`dr`) as a first-class property** *(done)* — the output/control rate becomes an explicit, compiler-validated property (with an `ir` init pass and a minimal `dr` demand-pull driver), the substrate for the demand and FFT families.

- ✅ **S2 — Typed controls: `tr`, `lag`/`varlag`, and scalar (`ir`) controls** *(done)* — control types the def author chooses: trigger controls reset after one block, lagged controls insert a shared `Lag` at compile time, `ir` controls freeze at init.

- ✅ **S3 — Special-index operator UGens (`UnaryOpUGen`/`BinaryOpUGen`) + `MulAdd`/`Sum3`/`Sum4`** *(done 2026-07-02)* — two generic op UGens whose `op` is the operator's name over the full scsynth opcode set, computed by the shared `clausters-core` functions for bit-identical client/server parity; `Add`/`Sub`/`Mul`/`Div` stay as aliases.

- ✅ **S4 — Complete the done-action set + `/node_run` (resume) + non-terminal pause** *(done 2026-07-03)* — the full 0–15 `DoneAction` enum (the relative-node actions resolved on the audio thread) and `/node_run`, which makes `PauseSelf` non-terminal.

- ✅ **S5 — Wavetable & table-generation infrastructure (`/buffer_gen`) + the table oscillators** *(done 2026-07-03)* — `/buffer_gen` (`sine1`/`sine2`/`sine3`/`cheby`/`copy`) fills buffers through the immutable-`Arc` NRT path, the scsynth interleaved wavetable format, and `Osc`/`OscN`/`VOsc`/`Shaper` as the first consumers.

- ✅ **S6 — Complete the scsynth OSC command set** *(done 2026-07-03)* — the missing node/group/bus/synth/buffer/def/scheduling vocabulary (`/node_setRange`/`/node_fill`/`/node_mapRange`, `/group_head`/`/group_tail`/`/node_order`, `/bus_setRange`/`/bus_getRange`/`/bus_fill`, `/synth_get`/`/synth_getRange`, `/buffer_close`, `/def_load`, `/sched_clear`) plus a typed `/server_cmd`/`/node_ugenCmd` surface and `/server_errorMode`.

- ✅ **S7 — Boot-time server configuration (audio I/O channels + every pre-allocated pool)** *(done 2026-07-03)* — `--inputs`/`--outputs` (with a real audio-input path via a second cpal stream) and `--max-nodes`/`--max-buffers`/`--max-graph-children`/`--max-ugen-inputs`, chosen at boot and reported in `/server_query`.

- ✅ **S8 — FFT/IFFT and the spectral (`fr`) chain** *(done 2026-07-03)* — `FFT`/`IFFT` + `PV_MagAbove`/`PV_MagBelow`/`PV_BrickWall`, one frame per hop; the transform planned at init (allocation-free per-hop), synth-private spectral scratch, and `/node_ugenCmd` as the live window-swap channel (S6's first consumer).

- ✅ **S9 — Side-effect UGens (no `Out` required)** *(done 2026-07-03)* — `SendTrig`/`SendReply`/`Poll` sending replies out the RT-safe reply FIFO; a valid def may contain only side-effecting UGens (the client relaxation is C19). Write-only UGens (`Out.kr`/`DiskOut`/`RecordBuf`) deferred with the buffer/streaming work.

- ✅ **S10 — Finite-resource registries: every id allocator recycles** *(done 2026-07-16)* — one shared `clausters_core::registry::Registry` (occupancy map, FFI-exposed) behind the server's `/synth_new -1` auto range, the MIDI voice range and the GraphDef private buses, and behind the Python client's node/bus/buffer allocators; the node-id space partitioned by `NodeIdPartition::from_max_nodes` (replacing the 2M/3M counters); client ids recycled via `/node_end`, engine rejections broadcast `/fail` with the id appended so nothing is lost; NRT node ids unbounded by design. Rationale in `docs/decisions.md`.

- ✅ **S11 — Hop-phase staggering for spectral chains** *(done 2026-07-18)* — de-align the hop blocks of concurrently running `FFT` chains so their transform spikes spread across blocks instead of stacking on one. Today every chain instantiated on the same block hops on the same block: the `bench` spectral section measures 32 aligned 1024-point chains at ~8% average load but ~65% of the block budget on the hop block — the sawtooth worst case. The fix is an initial `since_hop` offset chosen **deterministically per instance** (derived from the node id modulo the hop, quantized to blocks), so RT and NRT stay sample-identical for the same score and a given chain's own analysis is untouched (the hop was always quantized to the processing slice; only *which* block a chain first fires on shifts). Acceptance metric: the peak-block column of `examples/bench.rs` at 32/128 voices approaches `avg + one chain's pair cost`. Document the stagger in `docs/architecture.md` (spectral section) and note the determinism rule next to the NRT sample-identity invariant.

- ✅ **S12 — Editing does not go through the pool, and its verbs are three** *(opened 2026-08-15 as "a buffer write costs the samples written", rewritten twice the same day as the premise under it was rejected and then narrowed. The measurement is kept because it is what makes the decision safe rather than merely tidy; the earlier shapes are kept named because a milestone that quietly changes its mind teaches nothing.)*

  **The decision.** An editor's write never travels through the real-time server's buffer pool. The RT pool buffer stays immutable and replaceable exactly as `src/dsp/buffer.rs` says, `Osc`/`VOsc`/`Shaper`/`Conv` are untouched, and the question this milestone was opened to answer — what those four do when pointed at a writable buffer — **does not arise here at all**. (It does arise in S14, which is a different subject: recording, not editing.)

  **What was measured** (2026-08-15, release build; writes of 1 ms fired back to back, closed by one `/server_sync`, so the NRT queue's serial rate is what shows):

  ```
  10 s stereo    3.7 MB     2.26 ms per write     443 writes a second
  1 min stereo  22.0 MB     6.49 ms per write     154 writes a second
  5 min stereo 109.9 MB    33.84 ms per write      30 writes a second
  10 min stereo 219.7 MB   62.88 ms per write      16 writes a second
  ```

  Linear in the **buffer** (~0.29 ms per MB) and flat in the span, three quarters of it allocating and faulting in a second take rather than the memcpy. What that establishes is narrower than what it was gathered for: 33.8 ms is a defect **only under the assumption that every stroke writes the take**, and that was never the architecture. The working copy leads while the session is open (`crates/clausters-document/PLAN.md`, O8), a stroke is heard against the span it just drew (a scratch buffer that length sustains 443 writes a second, the route D4 already takes), and a take's pool buffer is replaced whole **once, on confirmation** — where replacing it whole is the correct operation and not a cost, because the material changed.

  **The placement rule this settles, and it is sharper than "does it need a graph":** what separates the two families of edit operation is **whether it has a timeline**. No timeline — arithmetic over samples — is `clausters-core`. A timeline — anything that runs a UGen graph, where an envelope evolves and a delay line fills — is the NRT engine (S13).

  **The three verbs**, all `clausters_core::edit`, pure over `&mut [f32]` plus a channel count, no I/O, and **none of them a new algorithm**:

  | verb | over the span | what it covers | what it reuses |
  |---|---|---|---|
  | `gain` | `from`, `to`, `shape`, `curve` | constant gain (`from == to`), fade in/out, silence, each half of a crossfade | `clausters_core::envshape::shape_value`, the SC shape numbers `EnvGen` and the BPF editor already speak |
  | `replace` | the samples | the pencil stroke (D2), the paste (D4) | a `copy_from_slice` |
  | `reverse` | — | a reversed span | a frame-wise reverse in place |

  `normalize` is not a verb: it is a measurement (the A track's descriptors) followed by `gain`.

  **Where they live is not who runs them, and the distinction is what keeps a settled decision true.** The verbs are `clausters-core` because that is where a shared algorithm lives (the placement rule), but the **performer is the NRT server** (S13): a client drives that mode, it does not call the core functions itself. So *one place performs audio processing and it is the server* (`crates/clausters-document/PLAN.md`) holds as written and needs no amendment — it speaks to who performs, not to which crate the code sits in. An earlier shape of this milestone had the Python client calling the verbs through the FFI with no server involved, which would have made two performers; S13 is what removed the reason to.

  **Two boundaries that follow.** **Length changes stay in the arrangement**: deleting a span from a flat file rewrites everything after it, while a placement already expresses "this part does not sound", undoably, which is what D4 shipped — so cut and paste stay placements and destructive length change is left to consolidate/bounce, which is a whole-file rewrite by nature. And **each verb answers which span it wrote**: the server holds no session state, so it cannot touch a peaks cache, and whoever owns the cache calls `peaks::update_range` over the span it was told about (D1's half of the same sentence).

  **Acceptance:** a drawn edit on a five-minute take is applied, heard and confirmed with no pool buffer written per stroke; the RT read path is untouched byte for byte, with `tests/rt_safety.rs` and the golden renders saying so; each verb is tested against a hand-computed span; and no verb duplicates an algorithm the core already has.

  **What shipped** *(2026-08-15)*. `clausters_core::edit` — `gain` (taking a `Fade`, which is `constant` or `from_to` along an `envshape` shape number), `silence`, `replace` and `reverse`, pure over `&mut [f32]` plus a channel count, spans in **frames**. On the wire, `/buffer_gain` and `/buffer_reverse` join the writing family as ordinary NRT jobs, chained like `/buffer_setRange` so a batch of edits composes in flight rather than each starting from the pre-batch contents; `replace` needed no command of its own, being `/buffer_setRange` already, and `silence` is `gain 0 0`. Python (`Buffer.gain`/`fade`/`silence`/`reverse`) and TypeScript builders in the same commit, `docs/schemas.md`, and `clients/python/examples/buffer_edit.py` grown a second half: the same edits said rather than performed client-side, with no samples crossing the wire.

  **No verb duplicates an algorithm**, which was the acceptance clause worth checking rather than asserting: `gain` rides `envshape::shape_value` (what `EnvGen` plays and the breakpoint editor draws, so a fade is the same curve everywhere), `replace` is a `copy_from_slice` and `reverse` a frame-wise swap. `normalize` stayed out for the same reason it is not a verb.

  **Where the acceptance over-reached.** Its first clause — *a drawn edit on a five-minute take is applied, heard and confirmed* — is D1 and D2's, not this milestone's: there is no pencil yet to draw one. What this can and does claim is the half it owns: no pool buffer is written per stroke because no verb writes a take at all, and the RT read path is untouched, which `rt_safety` and the golden renders pin.

  **Two things the work turned up, both recorded where they belong.** The command table is searched with `binary_search`, so `/buffer_gain` filed in the wrong place made it **unreachable** while reading as wired up — now guarded by a sortedness test that would have caught it. And ten examples play routines on a clock nobody starts (`clients/python/PLAN.md`, "Found by use"), which is how the new example cell came back silent.

- ✅ **S13 — The NRT server takes operations on demand** *(opened 2026-08-15, out of the editing pass; proposed by the user as "add an interactive mode to the NRT server, and do not touch the RT one")*. Today the NRT side is a batch renderer: it takes a score and returns a file, start to end. Editing needs the same engine to execute **operations on demand**, because interaction is not predictable — it answers a document, not a timeline.

  **It is its own mode, not a variant, and that is the point.** Almost every part is reused — the engine, the translator, the OSC surface, and the pulled door `src/embed.rs` already proves (`server.step()` before each `engine.process_block`, the caller owning the clock and the sink) — and the *semantics* are not shared. Treating it as a mode from the start is what keeps the reuse from turning into a mode that is secretly two.

  **The shape, settled 2026-08-15 before writing any of it**, because the sentence this milestone opened with — *it goes on the `Renderer`'s path* — turned out to name the wrong half. Reading the two: the `Renderer` has the synchronous execution and its own `now` and has **no** transport, no clients, no `/done`/`/fail`/`/server_sync`, and only part of the `/buffer_*` family; `OscServer` has all of that, and `src/embed.rs` already drives it with no audio device (`Segment::in_memory()` + `OscServer::headless` + an `IpcPeer` at each end, `step()` before each block, the caller owning the clock and the sink). So the **front is embed's shape**, reused rather than rebuilt, and *batch* is honoured where it belongs — in the **operation**, which is a closed render over the instance's engine. `render.rs` stays the score's path, untouched, and so does the RT server, which is the proposal's own condition.

  **Two mechanics that fall out of having no clock.** Between operations the engine must still *apply* what arrived (a `/buffer_alloc` installs through the engine's command FIFO) without time passing, so the engine grows a `drain` that is `process_block`'s first two steps and none of the rest. And the operation runs on the **instance's** engine rather than a fresh one per operation: a fresh engine would have to be handed the loaded buffers, which is the 110 MB copy this whole line of work exists to avoid.

  **There is no server clock.** Nothing advances between commands and nothing is scheduled against a running `now`. Two tiers follow, and conflating them is the mistake available here: **buffer-editing commands have no timeline in any sense** (S12's verbs — applying a gain to a span is not an event at an instant), while a **render operation does have one internally** (an envelope evolves, a delay line fills) — a self-contained score starting at 0 and lasting the span. What it deliberately does not carry, named so it does not drift in: interactive *generation* against a clock, which is a plausible thing and is not editing.

  **Determinism is of process, not of time.** A batch render is deterministic because it runs start to end; an interactive session cannot be, and does not need to be. The invariant is instead that **the same operation over the same material yields the same samples it would yield expressed in a score and rendered in batch** — which is exactly testable, with T1's own acceptance technique (cut the span out of a batch render and assert it equals the operation's output sample for sample, over a seeded-noise def, deliberately not block-aligned).

  **In this mode a pool buffer is mutable**, because there is no audio thread: the immutability contract is an RT rule, not a property of a buffer. So the loop is load (`/buffer_allocRead`), edit in place, write (`/buffer_write`), and the loaded buffer *is* the session state — which is right for an editing instance and would be wrong for a live server. From the T track it reuses the type discipline (`src/server/clock_axis.rs`: the axis is a type) and not the clock, there being no second axis where there is no first.

  **Open, to design once the mode runs (noted 2026-08-15 by the user): can it move the samples through shared memory instead of over the wire?** Bulk crosses as OSC blobs today (`/buffer_setRange` in, `/buffer_getRange.reply` out), which is the right trade for a client filling a buffer once and the wrong one for an editing session, where the material is a take and the operations are many. Two directions and neither is chosen here: **share the segment** — the machinery exists at both ends (`--shm` for a local RT client, `Segment::in_memory()` for embed), so an out-of-process session could carry samples the way control buses already travel; or **share the file**, which is sharper if it holds — the client owns the working copy and the GUI host already maps it read-only, so if the session maps the same file there is no data to move at all, only a span to name. The second would make the first unnecessary for editing and not for anything else.

  **Acceptance:** an operation applied interactively is sample-identical to the same operation in a batch score; the RT server's sources are untouched; the mode builds and runs **without `realtime`/cpal**; a `/server_quit`-shaped end and the absence of any scheduling surface are both tested.

  **What shipped** *(2026-08-15)*. `server::nrtsession::NrtSession` is the mode, on embed's front exactly as decided above; `Engine::drain` is the one thing the engine had to grow, and `/buffer_render bufnum frames` is the operation on the wire — `/buffer_gen`'s sibling, generating into a buffer by playing rather than by formula. The command's two halves stay apart on purpose: the server owns the message (parse, validate, answer) and the driver owns the engine, so it queues an `OfflineRender` the driver performs between commands, which is the NRT queue's own shape with the driver in the worker's seat. It is legal only where something owns the clock (`enable_offline_renders`), and a real-time server fails it rather than approximating — pinned by a test, since that is the half of *the RT server is untouched* that code can check.

  **Where the acceptance was wrong, and it is the milestone's own sentence.** *"The absence of any scheduling surface"* is not a property this mode has or should have: a **timetag is meaningful here**, because an operation *is* a score — a bundle inside its span lands at its exact sample exactly as the batch renderer places one, and one past the end waits for the next operation instead of firing. What the mode lacks is a clock that moves **on its own**, which is a different claim and the one that is now tested from both sides (an operation split around thirty-two settles joins seamlessly; a bundle beyond the span cannot be made to fire by settling).

  **What is honestly not "untouched", stated rather than glossed:** the RT server's *behaviour* is unchanged and `tests/rt_safety.rs` and the golden renders say so, but four additive things reach shared code — `Engine::drain`, `OscServer::set_seed`, the offline-render request pair, and the `/buffer_render` dispatch row. None of them runs in a real-time server; all of them compile into it.

  **What is left, and it belongs to the client leg rather than here**: no client can reach a session yet, since the front is the in-process ring and nothing exposes it over a socket or through the C ABI. So `/buffer_render` has its reference in `docs/schemas.md` and **no Python or TypeScript builder** — the packages move together, and a builder for a command no client can send would be a shape nobody exercises. Its example is in the same position, which is why this milestone closes without one. Both land with whatever first gives a client a door to a session, which is S12's verbs needing one.

- ✅ **S14 — Every pool buffer is writable, and the write-side UGens exist** *(opened 2026-08-15; the gap S9 deferred and `src/dsp/buffer.rs` and `src/dsp/delay.rs` have both been naming in their module docs since)*. In SuperCollider you allocate an empty buffer, zero it and use it for anything — record into it, delay through it, loop it, save it. Here you cannot, because a pool buffer is immutable once built, and **that is a missing capability rather than a design stance**: it is exactly the completeness the S track exists to reach.

  **What it unblocks, named:** `RecordBuf` and `BufWr` (write-side UGens, deferred in S9 for want of this substrate) and the `BufDelay*` family `src/dsp/delay.rs` says it cannot have (*"a delay over a pool buffer would have to mutate one"*) — the looper, the multi-second delay whose contents you want to inspect, resample or save. Long delays themselves already work through synth-private lines allocated at `build`; what is missing is only the **shared** case: record here, read there.

  **The shape as it was written, and how it changed.** It said: *writable is declared at allocation and is not a promotion — a buffer allocated for recording is not a wavetable and never becomes one, so the four per-block `data()` readers (`Osc`, `VOsc`, `Shaper`, `Conv`) refuse a writable buffer by name at build, while an ordinary buffer keeps flat `[f32]` storage and today's speed.* **That is not what shipped** *(the policy was changed 2026-08-16, by the user, before the substrate was written)*: there is no kind, no declaration and no refusal — every buffer is writable. See "What shipped" for the measurement that made the choice cheap and for the promise the type system could not have kept anyway.

  **The reader cost is measured, not assumed** (2026-08-15): with `[AtomicU32]` storage and relaxed loads, an interpolated stereo read of 64 frames goes 145 → 150 ns (**+3%**), and **+0%** on a wavetable-shaped read of a table hot in cache. Per-element atomicity means no sample is ever read half-written; what a reader can see is a mixture of old and new across a span, which is scsynth's own semantics for exactly this case and is what a looper crossing its own write head has always sounded like. **The concurrency is real and is why the immutable design was chosen** — the DSP workers process in parallel — so the writable kind must state what it guarantees (per-element atomicity, no ordering between elements) rather than inherit an invariant it breaks.

  **Three scsynth commands turned up missing on the same pass and are S15's**, not this one's: they need no writable buffer and this needs none of them. `LocalBuf` is named here deliberately and **not** claimed as missing: its main scsynth use, the spectral chain's own buffer, is already synth-private scratch here, and its other uses are covered by the private delay line — so it needs a decision, not an implementation.

  **Acceptance:** a synth records into an allocated buffer while another plays it, and the loop sounds; a `BufDelay` over a pool buffer runs; `tests/rt_safety.rs` stays green and the read path's cost is reported; `Osc` pointed at a writable buffer refuses by name.

  **Taken in the same pass as S17** *(decided 2026-08-16)*. They are one pass over the buffer-UGen surface and over `src/dsp/buf.rs` in particular: this one changes how a reader **reads** (the `[AtomicU32]` storage whose +3% is measured above, in `read_lin` and `PlayBuf::process`), and S17 changes what a reader **is** (a trigger, a cue point, a done action) in the same two functions. Doing them apart means editing that path twice and measuring it twice.

  **What shipped** *(2026-08-16, over four commits)*. The substrate first: `Buffer`'s storage is `Vec<AtomicU32>` holding `f32` bits, the shape (`frames`, `channels`, `sample_rate`) is settled at allocation and immutable, and every read goes through one relaxed-load door (`load`/`at`/`sample`) while `set_at`/`set_sample` take `&self` — the pool reaches the audio thread through an `Arc`, so the mutability has to live in the cells. `data() -> &[f32]` is **gone**, which is what forced every reader to be looked at: the wavetable path, the oscillators and the convolver now read `&[AtomicU32]` and are bit-identical (the goldens say so). Then `RecordBuf` and `BufWr`, then the nine `BufDelay*`/`BufComb*`/`BufAllpass*` rows over one implementation parameterised by `Storage { Private, Pool }`.

  **The refusal was dropped because it could not have been kept, and dropping it removed more than it cost.** A `bufnum` is a **runtime control** — `Osc`'s buffer is an input, not a static field, so "refuse a writable buffer at build" was never available to the compiler; what it could have done is fail at instantiation, or read silence, which is a worse answer than reading the samples that are there. Gone with it: the kind, the wire flag on `/buffer_alloc`, the branch in every reader, and a second storage layout to keep working. What remains is one sentence in the reference — contents are mutable, the shape is fixed.

  **The cost, measured before the decision and unchanged after** *(the acceptance's "the read path's cost is reported")*: an interpolated random read (`PlayBuf`/`BufRd`, 64 frames) goes 145 → 150 ns, **+5%** — 12 ns a block a reader, about a **thousandth of a percent** of a 64-frame block's budget at 48 kHz — and **+0%** on the three other shapes (sequential, wavetable, the convolver's kernel), where the loads vectorise as before. What is bought is per-element atomicity: a reader crossing a writer sees old samples and new, never half of one.

  **The example is `examples/buffer_writing.py`**, and what it had to solve is worth recording: a looper that does not click. Two rules make it silent — the recorded phrase is windowed to zero at the loop's seam and every change of what is written is made *during* that silence, where a change can store no edge; and the reader is started **after** the writer at the same rate, so it trails it by a fixed distance and never crosses the write head. The stored loop was then measured rather than trusted: the largest sample-to-sample step in the buffer is a fifth of the material's own steepest slope, and the seam is exactly zero.

- ✅ **S15 — `/buffer_fill`, `/buffer_readChannel`, `/buffer_allocReadChannel`** *(opened 2026-08-15, found while writing S14 by reading our `/buffer_*` set against scsynth's `b_*` one; recorded rather than folded in, because none of the three needs a writable buffer and S14 needs none of them)*. Those three names are **the** names — the `/<resource>_<action>` rule applied to scsynth's `/b_fill`, `/b_readChannel` and `/b_allocReadChannel`, following the `/buffer_read`/`/buffer_allocRead` pair that is already there. S6 declared the scsynth command set complete and these are not in it, and are not recorded as deliberate omissions anywhere either — which is the part worth fixing beyond the commands themselves, since an undocumented gap in a set declared complete is what makes the next reader trust the wrong thing.

  - **`/buffer_fill`** — fill ranges with a value (`[start numSamples value]...`), scsynth's `/b_fill`. It is the sibling of the `/bus_fill` that **S6 did ship**, so the asymmetry is inside one milestone. It needs nothing new: an NRT job shaped like `Set` with a value instead of a run, laid into the copy that replaces the buffer whole, exactly as the writing family already works. It is also an editing verb — silencing a span is a fill with 0 — so S12's `gain` and this overlap deliberately: one is the core function an editing mode calls, the other the wire command any client has.
  - **`/buffer_readChannel`** and **`/buffer_allocReadChannel`** — scsynth's `/b_readChannel` and `/b_allocReadChannel`: read *selected channels* of a file, into an existing buffer or into a fresh one. This is how a stereo file's left channel is loaded into a mono buffer, and today it cannot be done at all: `/buffer_read` **fails** on a channel-count mismatch (`"channel count mismatch: buffer has N, path has M"`), so the only route is loading every channel and paying for the ones you discard. `read_audio` returns interleaved frames, so the selection is a de-interleave at the end of the existing path rather than a second reader.

  **The packages move together**, so each lands with its Python and TypeScript builders and its row in `docs/schemas.md`. **Acceptance:** a fill over several ranges in one message, including one past the end, which fails rather than clamping like the rest of the writing family; a stereo file's second channel loaded into a mono buffer and asserted sample for sample against the interleaved read; and `docs/schemas.md` listing the three, so the set is complete in the reference and not only in the server.

  **What shipped** *(2026-08-16)*. `NrtJob::Fill` beside `Set` rather than folded into it — a fill says how *many* samples it writes, and expanding it into a run would allocate the very thing it exists to avoid — chained like the rest so a batch composes. The two channel reads are **one argument on the existing arms**, not a second implementation of reading a file: `select_channels` de-interleaves after `read_audio`, and an empty selection is every channel, which is exactly what `/buffer_read` and `/buffer_allocRead` send. Both clients' builders (`Buffer.fill`, `Buffer.read_channels`, `Buffer.read_channels_into`; `fill`, `readChannels`, `readChannelsInto`), the reference, and a test file that writes a real stereo WAV and reads each channel back out of it.

  **Two calls worth recording.** The channel list is a **variadic tail, so the positions before it are required** rather than optional — with a tail there is no telling a `fileStart` from a channel index, which is the same reason scsynth fixes those positions. And the order is honoured with repeats allowed (`1 0` swaps a pair, `0 0` widens a mono file), because that is what naming channels explicitly is *for* and it costs nothing to permit; a channel the file does not have **fails**, since asking for the right channel of a mono file is a mistake worth hearing about rather than a silent track.

  **No new example, stated rather than omitted:** the three are variants inside families the examples already exercise, and what is genuinely new — a channel-selective read — is covered end to end by a test that goes through a real file on disk rather than a mock.

- ✅ **S16 — A buffer write can address one channel** *(opened 2026-08-16, found by the GUI's standalone editor: the destructive-edit path refuses a multichannel take by name, and this is the command it names)*. Every writing command in the family addresses **flat, interleaved** samples: `/buffer_set` takes indices, `/buffer_setRange` takes a start and a contiguous run. That is the right addressing for filling a buffer and the wrong one for editing a *channel* of one — one channel of a stereo take is a **strided** span, and there is no command for it. The editing verbs S12 shipped are frame-addressed and already take a channel count, so the gap is on the wire and not in the core.

  **What it blocks, concretely.** The GUI host draws a take and writes a stroke back into the very buffer it is drawing; with more than one channel it refuses, with that sentence, rather than writing the wrong samples or sending one message per sample — which is the shape this rule exists to avoid (a stroke over a few thousand samples as N messages is the encode the blob convention was introduced to kill). So a stereo take is drawable and not editable today.

  **The shape, to decide when it opens.** Either a channel argument on the existing writers (`/buffer_setRange bufnum channel start blob`, where the run is that channel's own frames) or a separate `/buffer_setRangeChannel` beside `/buffer_readChannel` — the family already spells a channel-selective variant that way once, which is an argument for the second and against growing an argument on a command whose current shape is in every client. Whichever it is, `/buffer_set` gets the same treatment, and the reply, the past-the-end failure and the NRT chaining are the family's as they stand.

  **Acceptance:** one channel of a stereo buffer is written and the other is asserted unchanged, sample for sample; a span past the end fails like the rest of the family; both clients' builders and `docs/schemas.md` carry it; and the GUI host stops refusing a multichannel take (`clients/gui/PLAN.md`, H4).

  **What shipped** *(2026-08-16)*. Two commands, `/buffer_setChannel bufnum channel [frame value]...` and `/buffer_setRangeChannel bufnum channel [frame blob]...` — the `*Channel` spelling the family already uses once (`/buffer_readChannel`), with the channel **before** the runs because the runs are the variadic tail and a tail cannot be told from a start. Positions are frames of that channel; one message writes one channel, like every other per-channel thing here.

  **Inside, it is not a second job.** `NrtJob::Set` now carries `SampleWrite { at, stride, values }`: the flat forms pass a stride of 1 and the channel forms pass the channel count, so the copy-and-swap, the batch chaining and the bounds check are the ones that were already there — one conversion at the parse (`frame * channels + ch`) and nothing downstream knows. A refusal speaks the unit it was written in: past the end reports **frames** for a channel write and samples for a flat one, and a channel the buffer does not have fails the way a channel a *file* does not have already did.

  **It closed the milestone that asked for it in the same commit.** The GUI host writes through `/buffer_setRangeChannel` and no longer refuses a multichannel take; `Intent::WriteSamples` grew a `channel` (serde-default, so an older document reads as channel 0), which is what makes an undo put back the channel it took. Found by using it: the clip's take body drew channel 0 alone, the monitor played channel 0 alone, and a drag that left its lane read the *neighbouring* lane's value — all three in the "Found by use" list of `clients/gui/PLAN.md`.

- ✅ **S17 — `PlayBuf` completes its set: `trigger`, `startPos`, `doneAction`** *(opened 2026-08-16, while deciding T5's shape; the gap itself is older and both `docs/schemas.md` and `src/dsp/buf.rs` have been recording it in prose — "starts at frame 0", "neither has a trigger or done action yet")*. scsynth's `PlayBuf` takes `trigger` and `startPos` (re-cue to a frame on a rising edge) and a `doneAction` (what happens when a non-looping pass reaches the end). Ours takes none of the three, so a buffer player can be started and freed and nothing else — no cue point, no re-trigger, and a finished one-shot sits in the tree outputting silence until somebody frees it. That is the S track's own subject: a capability missing from a set declared complete.

  **It is not what an editor's playback rests on, and that is deliberate.** T5 puts the piece's position in the engine and has readers *follow* it, which is the DAW shape and the one a multitrack needs — many readers, one time. This is the other shape, where a reader carries its own position, and both are legitimate: a one-shot sample triggered from a pattern has no business consulting a transport. So this lands when the S track's completeness argues for it, not as a dependency of anything in the GUI.

  **Taken in the same pass as S14** *(decided 2026-08-16)*, which is the open S milestone this shares its code with: S14 changes how a reader **reads** (writable storage, and the `read_lin`/`PlayBuf::process` cost it measures) and this changes what a reader **is**, both inside `src/dsp/buf.rs`. Apart, that path is edited twice and measured twice; together it is one pass, one benchmark and one row rewritten in the reference.

  **Acceptance:** a rising trigger re-cues to `startPos` mid-play; `doneAction` 2 frees the synth when a non-looping pass ends, with the whole set behaving as `EnvGen`'s; a looping reader ignores the done action; `docs/schemas.md`'s row and both clients' builders move with it. `BufRd`'s own row is untouched — it is phase-driven and has no position of its own to cue.

  **What shipped** *(2026-08-16)*. The three inputs, read the way `RecordBuf` reads its own: `done_action` block-scalar like `EnvGen`'s, the trigger read per sample and *before* anything else in the loop so a finished player can be re-cued, and `start_pos` taken at the first block and at every rising edge.

  **The order is ours and not scsynth's, deliberately.** They arrive **after** `loop` rather than in scsynth's `rate, trigger, startPos, loop`, because inputs are positional and the arity check counts them: inserting `start_pos` before `loop` would have left every existing call the right length and silently re-read its `loop` argument as a cue frame. Appending costs a divergence documented in the reference and buys a wrong call that fails loudly. It still moved the arity from 4 to 7, so every hand-written def JSON in the tree grew three constants; both clients' builders default them, which is why no example moved.

  **A cue point belongs to whoever carries a position**, and that is the line the milestone draws: `PlayBuf` and `RecordBuf` advance themselves and take the set; `BufRd` and `BufWr` are driven by a phase signal and have nothing to cue — re-cueing one means changing the signal that drives it. Said in `docs/schemas.md` and in `src/dsp/buf.rs`, where the prose used to promise the gap instead.

  **Found while checking it by ear:** an `EnvGen` retriggered mid-envelope restarts from its **initial level** rather than gliding from where it was, so a re-cue is a step in two places at once (the reader's jump and the envelope's). `examples/buffer_writing.py` therefore fires grains shorter than the gap between triggers, which puts both steps in silence — the general rule for anything driven by a trigger, and the reason the example says it out loud.

## B track — the engine in the browser (wasm)

Section added 2026-07-18. Compile the engine to `wasm32` and run it **in the
page** behind an AudioWorklet, so browser GUI components stop needing a native
server process: first the engine itself (headless, then live), then the GuiDef
standalone equivalence (a bundle boots entirely in a tab — the browser twin of
`--standalone`), and only then the web-component packaging as a thin capstone.
This is the track `clients/gui/PLAN.md`'s "In-browser audio engine" section
anticipated; that section now points here. The GUI host's browser build
(G11–G17) is the substrate and stays untouched; the `faust` feature is out of
scope (LLVM JIT — a Faust *interpreter* backend is its own future work), so the
wasm engine is the `synth,embed` build.

**Topology (decided; recorded in `docs/decisions.md` with B2):** one wasm
instance — OSC translate + engine — inside the AudioWorkletGlobalScope, OSC
bytes over MessagePort both ways, commands through the in-memory ring
(`Segment::in_memory`). No COOP/COEP requirement (components must embed on
arbitrary pages), which rules out SharedArrayBuffer initially; the one
relaxation vs. native RT rules is that OSC→Cmd translation allocates on the
worklet thread (wasm malloc is a bump over linear memory — no page faults, no
priority inversion; DSP itself stays allocation-free). The ring seam keeps a
later SAB/wasm-threads build (zero-message in-page `BusSource`) open as an
unnumbered optimization.

**Consolidation note (with W0):** every browser JS/HTML artifact the B
milestones describe below (the worklet/loader runtime, the harness and
standalone pages, the bundle fetch module, the manifest generator) now lives
in the web package — `clients/web/` — with the crates staying Rust-only; the
entries keep their original paths as a record of what shipped where. See
`docs/decisions.md` ("The web front-end lives in one package").

- ✅ **B0 — wasm32 build gate + offline render parity** *(done 2026-07-18)* —
  the engine compiles and renders on `wasm32-unknown-unknown` before any Web
  Audio work: `tungstenite`/`osc::ws` target-gated off wasm (the one compile
  blocker), the `Instant` CPU meter and `DiskIn`/`DiskOut` gated, the new
  workspace member `crates/clausters-web` (the JS door, sibling of
  `clausters-ffi`'s C door: `abi_version`, `render` over `render_to_vec`,
  workers = 0), `scripts/check-wasm.sh` as the build gate and
  `scripts/parity-web.sh` as the acceptance: the wasm render of a
  denormal-free score matches the native NRT render within 1e-6 (measured
  max delta 1.5e-8; strict bit-identity is impossible cross-libm — see
  `docs/decisions.md`).

- ✅ **B1 — the headless live server (pulled mode), natively testable**
  *(done 2026-07-18)* — `OscServer::headless` (no socket, inline NRT via
  `NrtRunner`, streams/timetags on the **engine sample clock** through the
  `TimeSource` seam — wall time on the native `bind` path, unchanged) plus a
  public `step()`, one pulled serving turn run before **each** engine block;
  `ClaustersHeadless` in `src/embed.rs` (feature `embed`, no `realtime`
  needed: `send`/`poll_into`/`process_block`/`clock`/`ctl_*`/`quit_requested`,
  plus `buffer_load` installing host-decoded samples through the same path as the
  async `/buffer_*` installs — the browser's `/buffer_allocRead` replacement),
  documented in `docs/using-as-a-library.md` as a supported native embed
  mode. `tests/headless.rs` drives it end to end (tone + `/done`s, `/bus_stream`
  pacing deterministic on sample time, a timed bundle landing on its exact
  mid-block sample, inline `/buffer_alloc`, `buffer_load` + `/buffer_query`, `/server_quit`
  reported not enacted); the wasm shell wraps it 1:1 as `WebServer` with a
  native smoke pulling 128-frame quanta.

- ✅ **B2 — the AudioWorklet backend: the engine live in a page**
  *(done 2026-07-18)* — `web/worklet.js` (the processor: the wasm module
  compiled on the main thread, passed through `processorOptions` and
  instantiated **synchronously** in the constructor via `initSync`;
  `port.onmessage` → `send` with ordered backpressure retry; each 128-frame
  quantum one `WebServer.process` call, de-interleaved into the output;
  replies drained to `postMessage`), `web/worklet-shim.js` (the worklet scope
  lacks `TextDecoder`; imported before the glue), `web/loader.js`
  (`bootClausters`: compile + `addModule`, `AudioWorkletNode`, raw
  `send`/`onReply`/`clock`, `resume()` as the gesture hook), `web/osc.js` (a
  page-side OSC codec), the audible harness `web/index.html`, and the
  acceptance `web/smoke.html` + `scripts/smoke-web.sh` under headless Chrome:
  `/server_status` round trip over the MessagePort, engine clock advance, and the
  `/synth_new` sine measured at an AnalyserNode (the verdict beaconed through the
  HTTP access log — real-time audio vs. Chrome's virtual time, see
  `docs/decisions.md`).

- ✅ **B3 — GuiDef standalone equivalence: a bundle boots in a tab**
  *(done 2026-07-18)* — `ServerLink::Page` in the GUI host (wasm-only:
  outbound OSC to a page-registered callback via `GuiBridge.connect_page`;
  inbound via `GuiBridge.server_reply`), the streamed data paths
  (`/bus_stream`, `/bus_tapStream`, `/buffer_getRange`, `/clock_query`) unchanged over it; the
  bundle boot's ordering/encoding in the platform-agnostic `host::bundle`
  (natively unit-tested, mirroring the server's own data-dir boot order,
  bracketed by two `/server_sync`s — the second is the page's "bundle up" signal),
  exposed to JS as `bundle_boot_packets`; the fetch half in
  `clients/gui/web/bundle.js` (+ `bundle.json` manifest, the one addition to
  the persisted formats — HTTP cannot list directories;
  `web/bundle-manifest.py` generates it) and the page
  `web/standalone.html`; samples fetch + `decodeAudioData` → the engine's
  `bLoad` over the worklet port. Acceptance `scripts/smoke-web-standalone.sh`:
  a native-format bundle (SynthDef spec + GuiDef with `boot`/`bind`) boots
  entirely in a headless-Chrome tab, `/server_sync.reply` confirms, and the meter's
  control bus streams live values over the in-page leg.

- ✅ **B4 — web components + the per-page singleton (thin capstone)**
  *(done 2026-07-18)* — the `clausters` npm package seeding `clients/web/`
  (plain ES modules, no bundler/node toolchain — W0 adds those; `build.sh`
  stages the two wasm bundles into `engine/`/`gui-host/` so the directory is
  servable as-is): `server()` the lazy per-page engine singleton (raw
  `send`/`addReply` fan-out, `clock`, `bLoad`, resume/suspend — the REPL/TS
  surface), `guiHost()` the per-page host singleton (wires the in-page leg
  once: engine replies → `server_reply`, outbound → `engine.send`; captures
  the winit canvas for adoption), `bootBundle()` over both, and the custom
  elements: `<clausters-bundle src name>` (boots a bundle, adopts the canvas
  into its shadow DOM, its button the standard autoplay-gesture affordance;
  `clausters-ready`/`-error` events) and `<clausters-power>` (the affordance
  alone). Components share one engine/host by construction — the common
  node/bus/buffer namespace. Acceptance `scripts/smoke-web-components.sh` +
  `demo.html?smoke=1`: element up with the canvas in its shadow root, raw
  `server()` sees the element's synth (`/server_status`), meter bus streaming.

## U track — the UGen library

Section added 2026-07-25. The S track finished the substrate *for* this and
closed; the catalog itself never grew past the base set, so a `synth`-only build
(the wasm engine included, which has no LLVM and therefore no Faust) still cannot
produce a band-limited saw, a resonant filter, a delay, a trigger or a pan.
Faust covers all of it today, which is why the gap has not hurt in practice —
the justification for closing it is (a) builds without Faust/LLVM, (b)
conceptual parity with scsynth, (c) teaching material. Like F, S and B, U
coexists with the M line; its milestones are largely independent of each other
and interleavable as loose items, with U0 the one prerequisite.

**Design stance — one implementation per family, scsynth names on the wire.**
Implementations are grouped by **affinity**: one Rust core per family, and the
registry exposes only the **scsynth names**, one row each, the mode chosen in
the row's `build`. This is the `PvMag` pattern (`PV_MagAbove`/`PV_MagBelow`/
`PV_MagClip` → one struct + a `MagMode`), deliberately not the `BinaryOpUGen`
pattern: no `Svf` or `Delay` kind exists on the wire, so the catalog reads as
scsynth's and a mode is never something a def has to spell. Where a
parameterized core makes a *new* capability nearly free that scsynth has no name
for (a filter whose mode is a signal input — continuous LP→BP→HP morphing, which
falls out of the SVF's shared integrator update and would cost a coefficient
recompute in a biquad), that is a kind on its own merits, named for itself, and
out of scope here.

**Design stance — the realization may differ where the transfer function does
not.** Per this plan's "conceptual, not binary, compatibility" principle, each
milestone below is free to adopt a better implementation than scsynth's and must
say why. The three that shape the track, each landing as a `docs/decisions.md`
entry:

1. **Internal precision.** Wires and buses stay `f32` (the ABI), but filter and
   integrator **state and coefficients are `f64`**, as are phase accumulators and
   delay read positions. This is not a deviation but an alignment: scsynth's own
   `FilterUGens.cpp` declares `double y1, y2, a0, b1, b2` for `LPF`/`HPF`/
   `RLPF`/`RHPF`/`BPF`/`BRF`/`Resonz`/`Ringz`/`OnePole`, because at low cutoff
   the poles sit near `z = 1` and in `f32` the coefficient quantization and state
   truncation noise dominate the output. The `Sine` precedent (`f64` phase) is
   the same reasoning already applied once.
2. **Filters: one TPT/ZDF state-variable core, not direct-form-II biquads.** The
   transfer function is the *same* bilinear-transformed two-pole prototype, so
   the magnitude response is verifiable analytically and matches; the
   realization is trapezoidal-integrator state space, which does not blow up
   under audio-rate cutoff modulation, is far better conditioned at low fc, and
   yields LP/BP/HP/notch/peak from one computation — which is what lets one core
   cover eight scsynth names.
3. **Oscillators: PolyBLEP over an `f64` phase, not the DSF impulse train.**
   scsynth's `Saw`/`Pulse`/`Blip` divide a sine table by a cosecant table and run
   the result through a `0.999f` leaky integrator over an `int32` fixed-point
   phase — a division per sample, a table, DC settling and drift, and fixed-point
   tuning error. PolyBLEP has none of those; its honest cost is being *quasi*-
   bandlimited (aliasing rises toward Nyquist), which the tests **measure and
   publish** rather than claim away.

**Design stance — efficiency lives at the block, not the sample.** A parameter
arriving as a length-1 wire (a constant, an `ir`/`kr` value) is distinguishable
at the top of `process`, so coefficients are recomputed **once per block**;
an `ar` parameter gets two evaluations per block and a linear per-sample ramp
(scsynth's `CALCSLOPE` in spirit) rather than a per-sample recompute. That is the
same effect scsynth gets from generating a `next` variant per input-rate
combination, without the combinatorial code. Every family lands with a row in
`examples/bench.rs`, whose existing UGen-vs-Faust comparison is this track's
yardstick.

**Testing stance — the asserts are measurements, not goldens.** U0 builds the
harness and the rules: a filter is asserted against the **analytic transfer
function of the structure actually implemented**, evaluated in `f64` — never
against a golden, never against scsynth's output; an oscillator reports a
measured alias SNR at several fundamentals and asserts a documented floor; a
stochastic source is tested for distribution (mean, variance, spectral slope)
with a fixed seed plus bit-exact reproducibility; every stateful UGen gets a
long-run numerical test (the one that catches an `f64`→`f32` state regression)
and a block-split test. The rules go into the `audio-testing` skill so later work
inherits them.

- ✅ **U0 — Build context, the deferred operators, and the measurement harness**
  *(done 2026-07-25)* — the prerequisite, nothing user-visible. (a) A `BuildCtx`
  (sample rate, block size) reaches `UGenDescriptor::build`, so a UGen may size
  its allocation from the sample rate — `build` and `UGenSynth::new` had none,
  which is what blocked the delay family; plus the `max_delay` static field on
  both the wire spec and `UGenConfig`. (b) The S3 operator deferrals close as
  `clausters_core::builtins` entries and nothing else: `fold2`, `wrap2`, `gcd`,
  `lcm`, `hypot_apx` (lowercased like every other name in that table, as
  `as_int` already is). `randRange`/`expRandRange` are **not** among them and
  never will be — they are not pure functions of their operands, so they cannot
  live in an op table whose entire purpose is a scalar formula both sides
  compute identically; the stochastic need is a UGen with its own RNG state
  (`Rand.ir` today, its exponential sibling with U6). scsynth's `hypot_apx` is
  reproduced rather than corrected — it is deliberately the cheap one, and a
  ported def must not change value — with its real error bound *measured*
  (never below the true hypotenuse, at worst +15.9 % near 30.4 deg, which is
  **not** the diagonal the intuition suggests) and scsynth's own prose/formula
  mismatch recorded. (c) `tests/common/signal.rs`, the shared measurement
  module over `clausters_core::fft`/`window`: single-frequency DFT (gain and
  phase at an *arbitrary* frequency, not the nearest bin), `response_at` for a
  filter's I/O pair, coherent-frequency selection, alias SNR, Welch spectrum,
  spectral slope in dB/octave, and sub-sample group delay — each documenting
  when its estimate is exact, each driven in `tests/signal.rs` by a signal whose
  answer is known in closed form. It establishes the baseline U1 must beat: a
  naive saw measures 30.9 / 16.0 / 9.9 dB of alias SNR at 105 / 996 / 3996 Hz.
  The rules the track tests by moved into the `audio-testing` skill.

- ✅ **U1 — The phase family** *(done 2026-07-25)* — `src/dsp/phase.rs`: one
  `f64` phase accumulator plus a **fourth-order** `poly_blep` (the residual of
  the cubic B-spline, derived in the module rather than tabulated), behind the
  rows `Saw`, `Pulse`, `VarSaw`, `Phasor` (the trigger-resettable ramp, in units
  per sample) and `LFSaw`/`LFPulse`/`LFTri`. Measured alias SNR for `Saw`:
  96.7 / 42.6 / 39.2 dB at 105 / 996 / 3996 Hz, against 30.9 / 16.0 / 9.9 for
  the same waveform generated naively — the second-order residual was
  implemented first and rejected on its numbers (67.6 / 32.3 / 27.7). Two
  findings worth the record: above `sr/4` the fourth-order correction regions
  overlap and it falls back to the second-order one, and a negative frequency
  needs the *same* expression with `|dt|` because reversing direction reverses
  both the sample's side and the jump's sign — the phase-mirroring form is
  algebraically identical but loses 17 dB to cancellation. The `LF*` shapes take
  their initial phase in **cycles**, not sclang's `[0, 2)`, and a shape without a
  duty cycle declares two inputs rather than three so `/ugen_query` never reports an
  inlet the UGen ignores.

- ✅ **U2 — The filter core** *(done 2026-07-25)* — `src/dsp/filter.rs`: one
  trapezoidal-integrator state-variable core behind `LPF`, `HPF`, `BPF`, `BRF`,
  `RLPF`, `RHPF` and `Resonz`, plus the one-pole family (`OnePole`, `OneZero`,
  `LeakDC`, `Integrator`). It implements the *same* bilinear-transformed two-pole
  prototype scsynth does, asserted against the closed form rather than a golden:
  **within 0.1 dB across nine octaves**, allpass flat to 0.02 dB, notch nulling
  below −136 dB. The two properties the realization was chosen for each have an
  acceptance test — a resonant cutoff swept 20 Hz→18 kHz at 40 Hz under
  full-scale noise stays bounded, and `LPF` at 20 Hz for ten seconds still gives
  the analytic passband gain (the test that would catch `f64` state regressing to
  `f32`). Resonance stays `rq` on the wire, for its clean domain rather than for
  cost; the Python builders accept `q=` and fold it. `BPF` and `Resonz` are one
  implementation under two names, with a test that says so. And the row scsynth
  has no name for: **`Svf`**, whose three tap gains (`low`, `band`, `high`) are
  **signal inputs**, so the response itself is modulable — every classic
  response is a triple (notch `1,0,1`, peak `-1,0,1`, allpass `1,-rq,1`) and the
  one-knob morph is a client helper, so no arbitrary ordering of responses enters
  the wire. Two measurement findings recorded in the tests: a digital two-pole is
  steeper than 12 dB/octave near Nyquist (bilinear warping — the filter being
  right, not wrong), and every gain measurement needs a coherent window or it
  reads a tenth of a dB off.

- ✅ **U3 — The delay core** *(done 2026-07-25)* — `src/dsp/delay.rs`: one line
  (`f32` storage, `f64` read position) parameterized by interpolation × feedback,
  behind `DelayN/L/C`, `CombN/L/C` and `AllpassN/L/C`. The line is
  **synth-private memory** allocated at build from `max_delay` and the sample
  rate — which is what U0's `BuildCtx` exists for — not a pool buffer, since a
  pool buffer is immutable; `BufDelay*` is therefore out of this track.
  `max_delay` is static configuration rather than scsynth's `ir` input, and the
  Python builders fill it from a constant delay time but **raise** on a modulated
  one that does not state its reach. These do not report `latency()`: a delay's
  delay is what the user asked for. Acceptance is each family's defining
  property — a pure delay lands on the exact frame with nothing anywhere else, a
  fractional one has the group delay requested to within 0.05 samples, the comb's
  envelope tracks `10^(-3(t-delay)/decay)` to 2 %, and **the allpass is flat to
  0.02 dB** across three interpolations, three decay times and four frequencies.
  Measured interpolation loss at 9 kHz through a half-sample delay: 1.6 dB
  linear, 0.36 dB cubic.

The batch closes with `examples/subtractive.py` — a band-limited saw through an
envelope-swept resonant lowpass, a pulse through a morphing `Svf`, and a
comb-plus-allpass space, rendered offline so it needs no audio hardware.

- ✅ **U4 — `Line`/`XLine` and the self-control set** *(done 2026-07-25)* —
  `Line` and `XLine` are scsynth's ramps in `src/dsp/line.rs` — the step
  derived once, one addition or one multiplication per sample — carrying the
  whole done-action set and landing exactly on the target. They first delegated
  to `EnvGen`; that reuse cost a shape evaluation (a `powf`, for `XLine`) per
  sample for a straight line, and was undone. The trade is scsynth's: the
  geometry is init-rate. Plus S9's deferred `FreeSelf`, `PauseSelf`, `FreeSelfWhenDone`,
  `Done`, in `src/dsp/nodectl.rs`. Two findings shaped the result. The **done
  flag is not the done action**: `Done` exists precisely for an envelope whose
  `doneAction` is 0, so reading the action would leave it blind, and the flag is
  not on a wire either (a finished envelope sits at its final level, which is
  just a number) — hence `UGen::is_done` and an `ExecMode::DoneQuery` that
  resolves input 0's *identity* the way the demand driver already does, with the
  compiler rejecting a source that can never finish. And **`PauseSelf` must not
  latch**, or `/node_run 1` would be useless: the action is recomputed per block.
  `RecordBuf`/`BufWr` remain out — they write into a pool buffer, which the
  immutability invariant forbids, and need their own decision first.

  Its prerequisite turned out to be a bug older than the U track: **every UGen
  now runs at its own sample rate** (scsynth's `unit->mRate->mSampleRate`), not
  the engine's. `Impulse.kr(10)` fired once a second instead of ten times, and
  the same factor sat in `Lag.kr`'s convergence time, `Saw.kr`'s pitch and every
  filter's cutoff at `kr`; `Line.kr` could not have been written correctly on
  top of it. The control rate is derived from the *slice* rather than from
  `BLOCK_SIZE`, so a scheduled bundle splitting a block does not make control
  time run fast. Choosing `kr` now changes a UGen's cost, not its meaning.

- ✅ **U5 — Triggers and control** *(done 2026-07-25)* — `src/dsp/trig.rs`: one
  rising-edge detector (`Edge`) under `Trig`, `Trig1`, `TDelay`, `Latch`,
  `Gate`, `Schmidt`, `ToggleFF`, `SetResetFF`, `PulseCount`, `PulseDivider`,
  `Stepper`, `Timer`, `Sweep`, `Changed`, `Decay`, `Decay2` and
  `DetectSilence`. The definition of a trigger had been copied into three
  places (`SendTrig`, `SendReply`/`Poll`, the `Demand` driver); they now share
  this one, so a kind added later inherits it rather than restating it. Nine
  state machines behind seventeen names, grouped where they are genuinely the
  same machine and left apart where they are not.

  These are state machines, so the milestone is mostly boundary decisions, each
  in `docs/decisions.md`: `Timer` and `Sweep` **interpolate** the zero crossing
  (at 997 Hz, not a whole number of samples at 48 kHz, that beats sample
  rounding by an order of magnitude — the tested claim); a `TDelay` of `n`
  fires at `t + n` and **re-arms on that sample**, without which a regular
  trigger stream came out limping (961, 1440, 1440 instead of a steady 960);
  a held pulse *includes* its trigger's sample while a delay does not; a
  simultaneous set and reset leaves a flip-flop at 0; a `Stepper` sits at
  `resetval` so its first trigger lands on `resetval + step`. `Changed`
  reproduces sclang's **halved** difference (`HPZ1`'s gain is 0.5) rather than
  correcting it, on U0's rule that a ported def must not change value.
  `DetectSilence` raises a done flag, and that flag has **block resolution** by
  nature — a bool has no position within a block.

- ✅ **U6 — Noise** *(done 2026-07-25)* — `src/dsp/noise.rs`: `PinkNoise`,
  `BrownNoise`, `GrayNoise`, `ClipNoise`, `LFNoise0/1/2`, `LFClipNoise`,
  `Dust`, `Dust2`, `Crackle`, all drawing from `clausters_core::rng` and all
  buildable from an explicit seed, so a render replays exactly; each *instance*
  seeds from a shared counter, since correlated noise summed with itself is a
  comb filter and subtracted from itself is silence. Pink noise is
  Voss–McCartney and explicitly not Trammell's stochastic variant: its
  randomized update schedule has an unbounded worst case, and an audio callback
  is not paid on average. Measured slopes over 40 Hz – 10 kHz: white **−0.08**,
  pink **−3.26** (the ideal is −3.01; the gap is Voss–McCartney's own staircase,
  published rather than smoothed over), brown **−5.79**.

  Three assumptions were written into a test or a doc comment first and
  corrected by measuring, all in `docs/decisions.md`. `GrayNoise` is **not**
  flat — it leans low at −2.9 dB/octave, and what distinguishes it is its
  step distribution (a mean step four thousand times the median, against 1.14
  for white). That bit-level property is exact in the **integer** (bit 31 is
  the sign, which is what makes the output bipolar) but **not observable from
  the output**, since `word / 2^31` in `f32` has a 24-bit significand and rounds
  by an amount that depends on the magnitude the flip just changed. And `Crackle` does
  **not** settle below a chaos of 1: there is no period up to 512 samples
  anywhere in 0.3–1.9, and its spread is not monotonic in the parameter.
  `LFNoise2` overshoots to ±1.7 by construction (it aims at midpoints and
  carries its slope), which is stable — the peak is the same over one second
  and over ten, at 5 Hz, 100 Hz and 2 kHz.

- ✅ **U7 — Panning and selection** *(done 2026-07-26)* — `src/dsp/pan.rs`:
  eleven rows over four cores. The engine gives a UGen **one output** (an input
  reference names a UGen, not an output of one), a deviation `docs/schemas.md`
  already states for the buffer readers — so a two-channel panner is two rows
  sharing their inputs and differing in a trailing `chan` index, and the Python
  `pan2()` returns a `ChannelList` of two, exactly what `out()` already accepts.
  `Pan2`, `LinPan2`, `Balance2`, `Rotate2`, `PanAz` that way; `XFade2`,
  `LinXFade2`, `Select`, `SelectX` as single-output rows, plus `splay()` as a
  client-side helper over `pan2`.

  Three decisions, all in `docs/decisions.md`. The pan law is a **polynomial**,
  not scsynth's rounded 2049-entry table: worst-case `2.6e-7` against its
  `3.8e-4`, exact at both ends (a hard pan is digital silence on the far side)
  and
  symmetric by construction, since the pair is one function read from both ends.
  It is evaluated **per sample** when the position is audio rate — the one place
  the track's block-rate stance is deliberately reversed, because ramping the
  gains across a block leaves a 3 dB hole wherever a fast sweep crosses it;
  measured cost, `examples/bench.rs`, **1.30×** the whole graph. And **width got
  a name**: `Rotate2` rotates the plane (moving an image without resizing it)
  and cannot express a width (resizing it without moving it), so the same matrix
  also carries `StereoWidth` — the knob — and `MidSide`, normalized to `1/√2`
  and therefore its own inverse, the only one of the two that lets something
  happen *between* the encode and the decode. Neither name is scsynth's.

  `SelectX` is one row rather than sclang's two `Select`s and an `XFade2`; the
  values agree across the index range and deliberately not outside it, where
  sclang folds the crossfade while clipping the picks and returns a mix of the
  first two sources for a negative index, or the last one at 1.414 past the end.
  `examples/panning.py` measures the family's level claims on its own render:
  equal power holds the stereo level and lifts the mono fold-down 3 dB at the
  centre, constant amplitude holds the mono sum instead, a centred `Balance2`
  costs 3 dB for doing nothing, and width leaves the mono sum **exactly** where
  it was — 0.688 at widths 0, 1 and 2 alike, since it only scales what cancels.

- ✅ **U8 — The demand family** *(done 2026-07-26)* — `src/dsp/demand.rs`:
  fourteen rows over six cores. `Dramp` (`Dseries`, `Dgeom`), `Drandom`
  (`Dwhite`/`Diwhite`, `Dbrown`/`Dibrown`), `Dlist` (`Dseq`, `Drand`, `Dxrand`,
  `Dshuf`), then `Dstutter`, `Dswitch1` and `Dbufrd` one machine each, plus the
  self-clocked drivers `Duty` and `TDuty` beside S1's `Demand`.

  The milestone was not the sources; it was making **streams nest**, which is
  what turns the family into a sequencer of phrases rather than of numbers.
  S1's substrate could not: it wired one driver to one source through a `step`
  closure, and its rate rule named a slot. Both were replaced by
  `dsp::DemandInputs`, the protocol a source sees its own inputs through — a
  plain number and a nested `Dseq` differ only in what `is_demand` answers —
  implemented by `Pull` in `synthdef::instance`, which borrows the **prefix** of
  the UGen vector before the row it serves. That one property (a UGen's inputs
  are earlier UGens) gives the recursion for free: each nested pull splits a
  strictly shorter prefix, so the borrows form a decreasing chain of indices and
  cannot alias, and the whole view is a stack value that allocates nothing.
  `tests/rt_safety.rs` drives all fourteen rows nested three deep under
  `assert_no_alloc`.

  Three decisions, all in `docs/decisions.md`. **`repeats ≤ 0` is the endless
  stream**, because the wire rejects a non-finite constant and JSON cannot spell
  `inf` — with the one wrinkle that a `NaN` count (an exhausted stream feeding
  it) means *zero*, since there the number is a value and not a request. **The
  nesting depth is a compile-time refusal** (16), not a runtime guard: a level
  costs a stack frame inside the callback, so the honest place to say no is
  where a human is still watching. And the rate rule lost its slot — a `dr` wire
  may feed **anything that pulls it**, a driver or another demand UGen, while a
  driver handed a plain number now gets a stream of one value that never ends
  instead of a compile error. **Resetting is per kind and lazy**: marked when a
  slot is left, performed just before it is read again, so a child the parent
  never returns to is never restarted — a restarted `Dshuf` has drawn a new
  order, and that is audible.

  `examples/demand.py` plays six sections and then measures the family's claims
  on its own render: five pitches against three durations realign after fifteen
  notes, a nested slot is drained and restarted (`1 2 3 9 1 2 3 9`), `Dswitch1`
  leaves the branch it did not pick exactly where it was (`1 10 2 20`),
  `Dshuf`'s second pass repeats its first, and `Duty`'s f64 countdown puts the
  600th pull of a 1.429 ms slot within a sample of where it belongs — against
  the 257 samples a naive per-pull counter would have drifted by.

Every U milestone ships its rows **with** their Python builders in
`clients/python/clausters/defs/ugens.py` (the contrast test keeps the input names
identical to the registry's; `/ugen_query` picks the rows up with no further work),
the catalog table in `docs/schemas.md`, and the usual milestone checklist.

## R track — structural refactoring (the shapes that grew past their module)

Section added 2026-08-02, from a structural review of the workspace crates and
the Python client. Nothing here is a feature: the codebase is healthy (no
`TODO`/`FIXME` anywhere in `src/`, `crates/` or the client; three
non-clippy `#[allow]`s, all justified), and what the review found is the
ordinary sediment of a codebase that grew faster than its file boundaries —
god-objects that accreted five responsibilities, one idiom copied a hundred
times, and one surface declared by hand in three languages.

**The track's invariant: behaviour is preserved, and the existing suites are
the proof.** No wire change, no OSC address added or removed, no client API
signature moved, no golden file touched. A milestone here is finished when the
full suite (plus the def-family matrix and the examples the change touches)
passes with **no test edited** — an edited assertion means the refactor changed
behaviour and is out of scope. Anything that wants a behaviour change is a
different milestone in a different track.

**And cost is part of behaviour.** The suites prove RT-*safety*
(`tests/rt_safety.rs`: no allocation on the audio thread) and sample-identity
(the golden renders), which are not the same claim as *speed*: a refactor can be
allocation-free, bit-exact and still spend more CPU per block, and nothing in
the repo would notice — `examples/bench.rs` and `examples/bench_transport.rs`
measure exactly that, but they are run by hand and CI runs neither. So a
milestone here that touches the audio thread, the command path or the UGen
tables **closes with a before/after `cargo run --release --example bench`**,
both numbers quoted in the commit message. The tolerance is the noise floor of
the machine it ran on (re-run the baseline, don't compare against a number from
another day); anything beyond it is a regression to explain or revert, not to
accept because the tests are green. Two columns carry the claim: "x real time"
for throughput, and the spectral section's **peak block**, which is the one an
average hides. **R11 has since turned this into a CI gate** — a pull request is
benched against its merge base on the same runner, and `scripts/bench-gate.py`
fails it on a regression past the measured threshold — so the before/after is
now done by the machine. Run it by hand anyway while working; the gate is the
backstop, not the instrument.

The exposure is wider than the audio thread, and mostly indirect. R9 is the
only milestone that edits `process_block`/`apply` directly. But R2 and R5 move
code across module boundaries (which can change what the compiler chooses to
inline), R8 restructures the `UGENS` table (which prices synth *instantiation*
— network thread, not RT, but on the latency budget a `/synth_new` answers
within), and R3 rewrites the argument parsing every command flows through.
Those are cheap to check and expensive to discover later, so check them.

**Priority is by bleeding, not by size.** R1 is first because it is the only
item where the current shape actively *produces* drift rather than merely
tolerating it; R2–R4 are next because the OSC front is where every future
command lands.

**R11 and R12 are not refactors — they are this track's guardrails**, and they
are here rather than in a track of their own because they exist for the same
reason the rest of it does: a check that depends on someone remembering is a
check that eventually is not run. R11 makes the cost invariant above
enforceable; R12 makes a release verify anything at all. Neither has to wait
for the refactors, and R12 in particular is worth doing first if a release is
near.

- ✅ **R1 — One declaration per shared-core function** *(done 2026-08-02)* — the
  core's surface was hand-written **three times** (the C ABI, the wasm bindings,
  and 133 lines of ctypes `argtypes` over 155 calls), with only the first
  compiler-checked against core, so a function added to core reached the clients
  only if three edits happened and matched. Both legs are now checked, by
  different instruments, because they owe different things.

  Python owes the C ABI **total coverage**, so its check is a comparison with no
  list to maintain: `ctypes` caches each symbol on the `CDLL` instance as it is
  reached, so after `_configure` runs the instance dictionary *is* the record of
  what the binding declared. It also asserts arity against the Rust signature,
  and catches the quiet case — parameters with no `argtypes` at all, where
  ctypes guesses from the Python values.

  The wasm surface is **legitimately partial**, and the "same set" premise this
  milestone was written with turned out to be wrong: measured, the two bindings
  differ in both directions and most differences are correct (a browser has
  WebSocket, libverovio is not built for wasm, JavaScript has no `u64`, wasm
  frees by `Drop`). Equality would have needed some sixty exemptions, and those
  exemptions were the only interesting content — so the artifact is
  `docs/bindings.md`, a manifest with a verdict per empty cell (`idiom`, `n/a`,
  `gap`), enforced by `tests/bindings.rs`. Divergence stays allowed; divergence
  nobody wrote down does not. Rationale in `docs/decisions.md`.

  Both checks were verified by mutation, since a parity test that passes on its
  first run has proven nothing: dropping a declaration, shortening an argtypes
  list, adding an export to either binding, and leaving a stale row in the
  manifest each fail one of them.

  **What the manifest surfaced, as its first act:** thirteen `gap` rows. Missing
  in wasm — `whitenoise`, `window`, `stats`, the four perceptual scale
  conversions (`hz_to_mel` and friends, which a spectrogram axis wants),
  `patch_compile`, and the clock model's raw `slope`/`intercept`. Missing in the
  C ABI — `spectrum_db`, the three `oscil_*` framing calls, `JsRng.uniform` and
  `JsRng.spawn` (the reproducible-substream primitive), and three `JsRegistry`
  reads. None is urgent and none belongs to this track: they are each either
  small work or a decision to write down, and the manifest is where they now sit
  in the open instead of being invisible.

- ✅ **R2 — Split the OSC front** *(done 2026-08-02)* — `src/osc/server.rs` was
  3217 lines and one `impl` block of 109 methods carrying five separable jobs:
  binding and draining the transports (UDP/TCP/WS/MIDI/IPC), packet and bundle
  dispatch, ~60 command handlers, the streaming subscriptions
  (`BusStream`/`TapStream`), and the async pipelines (NRT/Faust plus the
  `/server_sync` barrier). Pure movement: no signature changed and no test was
  touched, and the only edit the move forced was visibility — the compiler
  named all 62 methods a sibling module calls, so none was promoted on a guess.

  The milestone sketched five files and left the handlers unplaced; they are
  the bulk, so they became `commands/`, one module per resource family — the
  same families the wire names. That keeps the largest file at 500 lines
  instead of folding 1700 lines of handlers into `dispatch`.

- ✅ **R3 — An argument reader, and one voice for failures** *(done 2026-08-02)*
  — every handler used to destructure `msg.args` by hand and write its own
  refusal at each step, which is how 117 `fail` sites came to phrase the same
  four complaints in a dozen ways. `Args` is a cursor whose readers
  (`int`, `index`, `float`, `str`, `long`, `double`, the `opt_*` pair) return
  `Err` instead, handlers return `Answer`, and `OscServer::attempt` is the one
  place a refusal becomes `/fail`. Every command handler now reads through it;
  the count is down to 55, and what is left is not argument shape — the
  dispatch layer, the stream subscriptions and the async pipelines fail about
  other things.

  Beyond the count, two properties the old shape could not have. The failure is
  addressed with **the address the client sent**, so a handler and its dispatch
  arm can no longer drift into disagreeing about what the command is called.
  And `index()` is one read where callers used to write two separate refusals —
  not an integer, and negative — which is exactly the pair that had drifted
  most.

  The bus family is where the duplication was worst: five commands walking the
  same `(base, count, values)` shape with five copies of the same three checks,
  now differing only in the reading. `/synth_get`↔`/synth_getRange` collapsed
  into one body over a shared `control` reader, since resolving a control name
  needs the def and is the one read `Args` has no business doing.

  Two error strings did **not** change, because tests read them: rewording
  `"unknown group"` broke `transport_group_binds_and_unbinds`, and the wording
  went back rather than the test being edited. That is the track's invariant
  doing its job — and the reason to check, before starting, which strings the
  suite is holding.

- ✅ **R4 — Dispatch as a table, checked against the schema** *(done 2026-08-02)*
  — `handle_message` was a ~100-arm `match` on the address string. It is now
  `COMMANDS`, a sorted `&[(&str, Command)]` searched in one binary search, with
  one place that answers `/fail`.

  The table is not faster and was never meant to be; what it buys is that the
  command set became **enumerable**, and `tests/schema.rs` walks it in both
  directions against `docs/schemas.md`: a command the server answers and nobody
  documented, and a command the reference promises and nothing answers. Neither
  was catchable before — the two lived in different files with no way to
  compare them, so the reference could be trusted only as far as somebody's
  memory.

  Two things fell out rather than being aimed at. The dispatcher passes each
  row its own `&'static str`, so the handlers that carry their command's name
  into a later reply (the async buffer jobs) take it from the table instead of
  being told it a second time by their caller, and `/synth_get` distinguishes
  itself from `/synth_getRange` by reading its own address. And `attempt` /
  `attempt_for` disappeared: they existed because a `match` arm could not
  express "run this, and fail with the address that matched", which is now what
  the dispatcher does for every row.

  The parser needed real rules rather than an exclusion list, since the
  reference spells three other things as `/name`: group paths
  (`/mixer/drums`), scsynth's original names cited in the mapping table
  (`/s_new`, `/c_getn` — told apart by their one-letter resource prefix, which
  is exactly what clausters replaced), and the addresses the server *sends*
  (`/done`, the node notifications, `SendReply`'s default `/reply`).

  What it cannot check is whether the *arguments* a page describes are the ones
  the handler reads. That is the deeper drift; it needs types the protocol does
  not have, and this catches the coarse one, which is the one that has actually
  happened.

- ✅ **R5 — Split the translator** *(done 2026-08-02)* —
  `src/osc/translate.rs` was 2747 lines. The milestone named three concerns;
  measured, there were five, and the two it did not name are the ones a reader
  scrolls past looking for the translation proper: what the translator
  *reports* (`def_info`, `query_tree`, `node_info`, `dump_graph` — reply
  arguments, never a `Cmd`) and the `/buffer_*` parsing, which produces
  `NrtJob`s and is not translation at all. So `translate/{graph,midi,queries,
  buffers}.rs` beside a `mod.rs` that keeps the struct, the node and control
  commands, and `translate` itself.

  Pure movement, verified as such: the same 85 `fn` declarations before and
  after, and the whole code stream with whitespace removed differs by exactly
  the module scaffolding plus the seven methods the compiler demanded be
  promoted to `pub(in crate::osc::translate)`. No test touched. The bench came
  back inside the noise floor on every column, which is what the boundary move
  was checked against.

- ✅ **R6 — Split the client's `Server` facade** *(done 2026-08-02)* — the
  mirror image of R2 on the other end of the wire: `defs/server.py` was 958
  lines spanning boot, sending, queries, transport, streams and offline render.
  It is now a package whose `Server` composes `ServerQueries`,
  `ServerStreams` and `ServerTransport`, with the configuration types
  (`ServerOptions`/`ServerInfo`) in `options`. Mixins rather than collaborators
  precisely so no attribute path moves: `server.query_tree(...)` is the same
  call it was, and `from clausters.defs.server import ...` still answers to
  every name it used to.

  **The timeout is now the handle's**, `Server.timeout`, resolved in the two
  methods that actually consume it. The milestone counted 23 copies of
  `timeout: float = 5.0` in this file; there were 31 across the client, because
  `Buffer`, `Bus` and `Node` each carried the literal too and passed it down
  explicitly — which would have made the instance default a lie for half the
  API. All 31 became `None`, so an absent timeout means "the handle's" all the
  way down and `server.timeout = 30` is one assignment instead of an argument
  at every call site.

  The generated API page splits along the same lines (the four new modules are
  listed in `pydoc-markdown.yml`, so nothing left the reference), and
  `clients/web/PLAN.md` carries the shape for the TS port — including the
  timeout, which `defs/server.ts` repeats 26 times.

- ✅ **R7 — The widget's props, declared once** *(done 2026-08-02)* — a prop
  costs three synchronized edits (host, Python builder, TS builder) and nothing
  checked the three agree. It follows R1's decision, and for R1's reason: the
  premise it was written with does not hold.

  **There is no widget registry to generate from.** `host::registry::Registry`
  is bookkeeping over `Map<String, Value>` — it stores whatever arrives and
  knows nothing about which props a `knob` takes. The vocabulary lives in the
  *schema's* two wire passes (`widget::build`, `widget::apply`), one arm per
  kind plus the shared bundles those arms embed. And generation was the wrong
  goal anyway: the explicit signatures **are** the documentation — the Python
  docstring is the widget's user reference and the TS option type is what an
  editor completes.

  So `docs/gui-props.md` is the manifest and
  `clients/python/tests/test_gui_props.py` enforces it: Python read by *calling
  it* (`inspect.signature`, exact), the TS option types and the host's two
  passes read statically. A prop that does not reach all three fails the test
  unless a row says why, with R1's three verdicts. Verified by mutation in six
  directions (a prop dropped from a builder, a prop added to one, a row
  deleted, a row naming the wrong surfaces, a stale row, the host no longer
  reading a prop).

  **What it surfaced, as its first act: 28 rows, 26 pointing the same way.**
  The host implements the timeline chrome (`playhead`, `playhead_loop_*`,
  `sel_*`, `y_*`, `link`), `docs/gui-protocol.md` documents it and the TS
  `TimelineOptions` declares it once for every timeline widget — while the
  Python builders name it widget by widget, so `track` and `timeruler` name
  almost none of it. The other two point the other way and are worse: the TS
  `plot` offers `buffer` and `cache` that the host's plot never reads, so a
  page passing them gets no error and no effect. Closing either is an API
  change, so both are recorded as `gap` rather than fixed here.

- ✅ **R8 — Catalogs split by family** *(done 2026-08-02)* — the two catalogs,
  1971 and 2443 lines, both of them a long list with the families marked only
  by a comment. Navigability only: no symbol moved out of its public path.

  `defs/ugens.py` became `defs/ugens/{graph,osc,filter,pan,io,buf,spectral,
  trig,demand,env}.py`, all 156 names re-exported from `__init__` (plus the
  three private ones other modules already imported). `ugen_input_names` stays
  in `__init__` and not in a family module for a reason worth writing down: it
  maps each server kind to the parameter names of the callable that builds it
  by reading `globals()`, and only there are all the builders in one namespace.
  Proved by comparison against the pre-split module: the same 141 kinds mapped,
  the same signatures and docstrings for all 177 names.

  `src/dsp/registry.rs` became `registry/` with one module per family and
  `FAMILIES`, a slice of slices — an array literal cannot be assembled from
  pieces at compile time, so the catalog is the concatenation, still entirely
  static (no allocation, no lazy init). The families are **contiguous groups in
  the original table order**, which is not a detail: `all()` is what
  `/ugen_query` reports, so a regrouping would have reordered a reply. Verified
  by dumping the catalog before and after — the same 147 kinds, same order,
  same arities and input names.

  `all()` returns an iterator rather than a slice now, which its one caller and
  the uniqueness test adapt to (same assertion, same message). `lookup` runs in
  `compile`, once per def sent — not per `/synth_new` — so instantiation
  latency is untouched; the bench's RT columns confirm nothing else moved.

- ✅ **R9 — The engine's command application, grouped** *(done 2026-08-02)* —
  `Engine::apply` was a ~210-line `match Cmd` of nineteen arms. Done last, as
  the milestone asked, and the shape it takes is decided by the borrow the
  milestone did not account for: `sink` holds three of the engine's fields for
  the whole match, so a `&mut self` helper cannot coexist with it. What is
  separable is the family that touches **only the tree** — twelve of the
  nineteen arms — and that is one free function of two parameters,
  `apply_to_tree(tree, sink, cmd)`, returning the command back when it is not
  one of its own. The other seven each read different engine fields
  (`transport_*`, `sched`, `buffers`, `tap_buses`, `ipc`) and stay where they
  are: 210 lines become 110 plus a homogeneous 111.

  The leftover match names the twelve rather than using a `_`, so the compiler
  still refuses a `Cmd` variant neither half handles — a wildcard there would
  turn that omission into a panic on the audio thread.

  **Measured, paired and alternated on the same machine**, because this is
  audio-thread code: `bench.rs` A/B/A/B reads 1506.5x and 1481.9x before
  against 1486.6x and 1514.2x after (1 synth) — the distributions overlap and
  the spread matches four no-change baseline runs earlier the same day.
  `bench_transport` reads ~6% *faster* after, uniformly, **including its
  0-bundles-per-block rows where `apply` never runs** — which is what says that
  number is code layout in that binary and not the command path. `rt_safety`
  and the golden renders pass untouched.

- ✅ **R10 — The remaining oversized crate files** *(done 2026-08-02)* —
  `clausters-ffi/src/lib.rs` (1543) is now ten domain modules beside the
  `notation.rs` and `ws.rs` that already sat apart: `builtins`, `time`, `scale`,
  `rng`, `sched`, `clocksync`, `registry`, `patch`, `bundle`, `measure`. `lib.rs`
  keeps the crate docs, the ABI version and the re-exports — the C symbols do
  not care which file declares them (`no_mangle` names are flat), but a Rust
  caller does, so `pub use` keeps every `clausters_ffi::…` path. Each test
  travelled with what it tests. Proved by `nm` on the built cdylib: the same 69
  exported symbols, and `tests/bindings.rs` (which reads the whole `src`
  directory) passes untouched.

  `clausters-core/src/bundle.rs` (1287) split at its own seam: `format` (the
  serde types and the one `Error`), `resolve` (the substitution machinery,
  private) and `mod` (the passes, the binding envelope, the tests). The only
  edits movement forced were visibility, and the compiler named each one.

- ✅ **R11 — The performance gate stops being a discipline** *(done
  2026-08-02)* — CI ran fmt, clippy, the test matrix and the GUI/wasm gate, and
  no benchmark at all, so the track's cost invariant shipped only if whoever
  closed a milestone remembered it. It is a job now: on a pull request, the
  merge base and the head are built and benched **in the same job on the same
  runner**, and `scripts/bench-gate.py` compares those two. `[no-bench]` in the
  head commit message steps it aside.

  `examples/bench.rs` grew `--json` — one record per measured row, `{name,
  x_real_time, peak_block, gated}` — beside the human table, which is still the
  default and still prints exactly what it did.

  **The thresholds are measured, not chosen**, and the measuring changed the
  design twice. Three runs of an identical build, compared pairwise: throughput
  moves 0.5% median and 5.6% worst over 210 comparisons — so 10% is a safe
  gate. The peak block does not behave like that: 21% worst for the *aligned*
  peak, 34.6% for the *staggered* one, and **251.8% at one voice**. So the peak
  is gated at 50% and only where it is a measurement — the aligned peak from 32
  voices up. The staggered peak measures whether two chains' hops collided this
  run, which is the stagger working, and is reported instead. Same for the
  Faust rows (the JIT is in the number) and the worker sweep (it reads the
  core count). Rationale and the table in `docs/decisions.md`.

  **Proved in both directions before landing**, since a threshold nobody has
  seen trip is one nobody knows is wired up: three identical runs pass in all
  three pairings, and `#[inline(never)]` on two hot bus accessors turns eleven
  gated rows red at once (`default/1` −10.8%, `sine/ugen/1` −16.2%). The
  pessimization was reverted; the gate's own bootstrap case — a merge base that
  predates `--json` — is handled by the compare step, which says so and passes.

- ⬜ **R12 — A release verifies something.** `release.yml` fires on a `v*` tag
  and chains `build` → `publish-npm` → `publish-pypi` → `github-release`, with
  `needs:` pointing only at each other. There is no `needs:` on CI, no
  `workflow_run` gate, and no `cargo fmt`, `clippy`, `rustdoc` or `cargo test`
  anywhere in the file. A tag on a commit whose CI is red — or that CI never
  ran — publishes to PyPI and npm exactly the same. The workflow already treats
  publishing as one-way where the *contents* are concerned
  (`CLAUSTERS_REQUIRE_COMPLETE: "1"` exists so a piece cannot be silently
  skipped); this closes the other half, where the contents are complete and
  broken.

  Two things to wire, and they are separable:

  - **The gate itself.** A `verify` job at the head of the release, with
    `build` and both `publish-*` jobs `needs:` it. What it runs is the cheap
    question: at minimum the same set CI runs, so the published commit is held
    to the bar every merge is. Rejecting the tag when the commit's CI is not
    green is the alternative, and is weaker — it depends on CI having run at
    all on that exact commit.
  - **The nine configurations nobody automates.** Of the fourteen in
    `.claude/skills/feature-matrix/check.sh`, CI covers five (the two `fmt`s
    and the workspace/gui/default `clippy` runs). The other nine — `clippy`
    under neither def family, `synth` alone, `faust` alone, `clausters-ffi`
    with `verovio`, and all five `rustdoc` builds — are covered by a human
    remembering, which is what the skill exists to remind them of. Running the
    full `check.sh` in the release's `verify` job is the cheapest place to make
    that automatic: once per release rather than once per push, at the moment
    where being wrong is unrecoverable. `check.sh --fast` runs exactly the nine
    if the five are already proven by the same job. The `faust`-linking
    configurations need libfaust, which the release workflow already builds
    and caches through the `.github/actions/libfaust` composite — reuse it,
    don't add a second recipe.

  `check.sh` only ever reads: it is a gate, and it stays one. Nothing about
  wiring it into a workflow introduces a `--fix` pass — clearing a warning is
  deliberate, by hand, on one configuration at a time.

  **Both halves shipped** (2026-08-02): the `verify` job runs `check.sh` in
  full and the two `cargo test` runs, `build` and `publish-npm` each `needs:`
  it, and `CLAUSTERS_FAUST_TARGET` is set on that job alone so the published
  artifacts stay host-tuned. Wiring it turned up the two reds it exists to
  catch, both already on `main` and both fixed alongside: a headless test still
  filtering for a reply address the wire rename had moved, and two TypeDoc
  warnings failing the mdBooks job.

  **⚠ The one thing left, and the only reason this is not ✅: nobody has
  watched the gate stop anything.** Everything about it is verified except the
  behaviour that is its whole purpose. What *is* verified: the `needs:` graph
  (nothing reaches a publish step without `verify`), and the job's contents,
  run by hand on this tree — the fourteen matrix configurations clean and the
  four test configurations green. What is **not**: that a failing `verify`
  actually stops the run.

  This is unproven rather than untried, because the obvious test is unsafe.
  Testing it meant starting a real release; and if `verify` passed when it
  should not have — precisely the misconfiguration under test — the run would
  continue into `publish-npm` and `publish-pypi`, which cannot be taken back.
  **The test's failure mode is the disaster it is testing for**, so it does not
  get run casually, and it did not get run to close this milestone.

  **Half of it is now observed** (2026-08-02): a `workflow_dispatch` trigger
  rehearses the workflow without a tag — `verify` runs exactly as a release
  would, and the four build/publish jobs are guarded by
  `github.event_name == 'push'`, so nothing can reach a registry. The first run
  went green in 6m1s with all four skipped, which retires the weaker of the two
  doubts: the job runs on Actions at all, on the real runner, not just as YAML
  that parses. It says nothing about the stronger one — the dispatch path skips
  the publish jobs whatever `verify` does, so it cannot distinguish a gate from
  its absence.

  How to close the rest safely, when it is worth the time:

  - **On a fork or a scratch repository.** The `npm` and `pypi` environments,
    the `NPM_TOKEN` secret and PyPI's trusted-publisher binding (which names
    this repository *and* this workflow) exist only here, so on a fork a
    publish leg cannot succeed even if it is reached. Push a `v*` tag there on
    a deliberately broken tree — a warning under one def family alone is the
    honest break, since that is the class CI cannot see — and watch `verify`
    go red with everything downstream skipped.
  - **Worth having regardless of the test:** GitHub environment protection
    rules on `pypi` and `npm`. A required reviewer turns any run that reaches a
    publish step into a pause instead of a publication — a second, independent
    stop that does not depend on this workflow being correct.
  - **What does not count as proof:** watching a green `verify` — in a real
    release or in a dispatch rehearsal. That shows the job runs, not that its
    failure stops anything, and those are different claims. A gate only ever
    observed passing is indistinguishable from no gate.

**Not in this track (it is new work, not restructuring).** The review also found
a real parity gap: `defs/ugens.py` exposes 151 builders against
`clients/web/src/defs/ugens.ts`'s 116, and the 40 missing ones are five whole
families (demand, spectral, panning/stereo, convolution, disk I/O) plus the two
SVF filters. Per "the packages move together" that belongs to the web client's
own roadmap, and it does: W6 there already owns it, now carrying the measured
inventory. Recorded next to it, so the same diff does not raise them twice: the
four `gui/guidef.py` helpers that look Python-only are not — `correlation` and
`lissajous` are reachable through the wasm core, and the two file writers have
no page equivalent by design.

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
  documentation in `docs/` for new features, and an explained example in
  `examples/` if the feature is user-facing — not just the code and the git
  history. The examples are also the manual-test surface: new
  human-audible/visual behavior is checked by running one.

## Future directions (a design that is not a fix)

Opened 2026-08-15 with its first entry. The counterpart of "Found by use"
below: what belongs here is a **design** that has not converged into a
milestone, carrying a checkbox like everything unresolved, and leaving this
list when a milestone absorbs it (the milestone is then the record, and says
where it came from).

- ⬜ **A long take is played out of the pool, and `DiskIn` cannot be
  positioned** *(named 2026-08-15 alongside S12, and narrowed the same day once
  S12 settled where an* edit *happens: what is left here is where a long take is*
  read *from, which S12 does not answer)*. The arrangement sounds a take as a
  **pool buffer** played by an instrument def (`clausters.form.element`: a
  `Buffer` element renders as an event whose `buf` control carries the bufnum),
  so a five-minute stereo take is 110 MB of RAM and a thirty-minute one 660 MB —
  per take, on top of the client's working copy and the host's mapped view of
  the same material. A multitrack of takes is the case that makes that
  untenable, and it is also the case the streaming pair was built for:
  `DiskIn`/`DiskOut` (`src/dsp/disk.rs`) run their own I/O thread and never
  touch the pool. **The gap is concrete rather than architectural**: `DiskIn`
  takes one input (`chan`) plus a static path and plays front to back at one
  file frame per server sample, with **no start offset, no seek and no rate** —
  so a transport that starts at bar 40, or a take placed at an offset in the
  arrangement, cannot use it at all. Giving it a start frame is the smallest
  step and is probably a U-track item rather than a design; giving it a *seek*
  under a moving playhead is the design, since its ring is filled by a thread
  that would have to be told to refill from elsewhere. The third shape — the
  server **mapping** the file and reading it from the audio thread, which is
  what a monolithic editor does — stays named and unchosen, with the one firm
  finding this entry carries: a mapped read can **page-fault on the audio
  thread**, so it could only ever be an editing *mode*, a session's tolerance
  for a stall not being a live server's.

## Found by use: the running list of fixes

These are not milestones and they are not future directions. They are what
**using the thing** turns up — an eye pass over an example, a path read while
doing something else, a behavior that is correct and unclear — recorded the day
it is found so it is not rediscovered, and kept here rather than inside whichever
milestone happened to be open at the time.

Two conventions, because the section only works if both hold. Every entry is a
**checkbox**, so what is open reads as open at a glance and nothing has to be
inferred from where it sits. And a fixed one **stays**, with the record of what
was wrong and why the fix is the shape it is — that is what makes the list worth
reading rather than a queue that empties.

Anything unresolved lives here or under "Future directions", both **after** the
tracks: never inside the milestone that happened to be open, and never among
finished work, where a pending item reads as done.

- ✅ **A transport addressed in samples still needs a beat grid** *(noticed 2026-08-16 while closing T5, which left it open on purpose; fixed the same day, on the user's "look at the smell and find a solution")*. `/transport_play`, `/transport_stop`, `/transport_locateSample` and `/transport_loop` all refused until `/transport_set` had defined a grid, because every `/transport_*` command but that one always had. For an audio editor that meant declaring a tempo it never reads — `examples/transport_seek.py` set one and said in a comment that it was arbitrary, which is exactly what a smell is: a comment apologising for a call.

  **The fix is that the transport stops being optional and the grid starts being.** `OscServer.transport` was an `Option<Transport>` doing two jobs — "is there a transport" and "is there a grid" — and those are different questions the moment a position exists in samples. It is now a plain `Transport` that exists from boot, with `defined` saying whether `origin_sample`/`tempo` mean anything; `defined` is the wire field of the same name, so the reply did not change shape and no client had to learn a new one. Only the two commands that name a **beat** refuse without a grid: `/transport_locate`, and `/transport_play` *given a position*. Everything else — a bare play, a stop, `/transport_group`, `/transport_locateSample`, `/transport_loop`, and `/sched_atTransport`, which needed a bound group and not a tempo — is in samples and now needs nothing.

  **What it moved on the client side, which is the part worth checking twice.** `transport_state()`/`transportState()` used to answer `None` when no grid was defined — hiding the rolling state and the position, which is precisely the state this makes reachable. They now always answer, with the grid fields `None`/`null`. That reaches one caller in each client, `Playhead.follow_transport`, and the guard there moves from *the state exists* to **the grid exists**: a playhead runs on beats, and applying a `position` of 0 to a transport being driven in samples would locate it to the start. `transport()`/`transport()` — the grid getters — still answer `None` and did not change.

  **The example is the proof**: it no longer calls `set_transport` at all, and it binds a group, seeks by frame, loops a span, plays and stops with no tempo anywhere in the file.

- ⬜ **A persisted def that no longer compiles warns on every boot, forever** *(found 2026-08-16 closing S17: changing `PlayBuf`'s arity from 4 to 7 made every stored def that used it unloadable, and the server has printed seven `persisted SynthDef failed to load` warnings at every start since)*. `attach_store` recompiles each stored spec and, on failure, warns and skips it (`src/osc/server/lifecycle.rs`) — the right call at load time, since a def that fails today may be a build missing a feature rather than a def that is wrong. But nothing ever prunes it, so a UGen signature change leaves permanent noise in a place where warnings are supposed to mean something, and the person who sees it has no idea which def or how to clear it. The fix is not to delete on failure (a `--no-default-features` boot would eat the library); it is to say **which def**, name the store path, and offer one way to drop the dead ones — a flag, or a `/def_*` command that prunes what will not load.

- ⬜ **`transport_group` takes an id where the rest of the Python client takes a node** *(found 2026-08-16 writing T5's example: `server.transport_group(monitor)` raises `TypeError: int() argument must be ... not 'Group'`, and the call has to be spelled `monitor.id`)*. Everywhere else a group is passed as the object — `Synth(..., target=monitor)`, `Group(target=src)` — so the one place that wants the bare integer is the one a reader gets wrong, and it fails at the client rather than at the server. The TypeScript client already has `nodeId(...)` for exactly this coercion, so the fix is the same shape on the Python side and is a couple of lines; it is recorded rather than folded into T5 because a client-API coercion is not a transport decision and should not ride in on one.

- ⬜ **A sample write costs the whole buffer, not the samples written**
  *(noticed 2026-08-14, costing the message-passing design against a monolithic
  audio editor before opening the document crate's O4)*. `/buffer_setRange`
  lays its runs into a **copy that replaces the buffer whole**
  (`src/osc/server/commands/buffer.rs`), which is what lets the engine read
  without a lock and needs no allocation on the audio thread — a good trade for
  what the command was written for: a client filling a buffer it just
  allocated, a wavetable, a one-shot load. It is the wrong trade for an
  *editor*, where the same command is the write half of every destructive
  gesture: nudging one sample of a five-minute stereo take copies ~115 MB to
  change four bytes, and a draw stroke (the GUI track's D2) emits one of those
  per stroke. The cost is O(buffer) where the edit is O(span), so it grows with
  the material rather than with the work — a monolithic editor's
  copy-on-write-per-block copies the block. Worth stating plainly: **this is the
  only place where splitting the host from the data costs real time.** The
  gesture itself never crosses (the host draws the drag and emits one intent on
  release), and the commit round trip is sub-millisecond against a monolith's
  function call — invisible. Bulk in the other direction is already solved by
  mapped files and the shm segment. It is only this one write path that is
  asymptotically wrong. What it needs is a way to write **in place** rather than
  by whole-buffer replacement, which is a real RT-safety question and not a
  patch: an in-place write races the reader unless the range is handed over
  under something the audio thread can honour without locking (a per-buffer
  epoch the engine checks, a swap of just the affected blocks, or a
  copy-on-write block table that makes the copy proportional to the span). The
  choice needs measuring, not guessing. Until it is taken, an editing client's
  honest workaround is to keep the working copy client-side and push the buffer
  only on confirmation — which is exactly what the document crate already
  specifies (`crates/clausters-document/PLAN.md`, O8: the working buffer leads
  while the session is open), so nothing is blocked; it is the *live* audition
  of a destructive edit that pays.

  **Measured 2026-08-15, and answered by S12 — but not in this entry's own
  terms.** The numbers are in the milestone; what belongs here is that
  **everything above from "What it needs is" onward is the wrong question**, and
  the measurement is what showed it. The estimate was conservative (a write on a
  five-minute take costs **33.8 ms** sustained, three quarters of it allocating
  and faulting in a second take, not the memcpy alone) and mutable storage was
  measured and found affordable — and it was refused anyway, because the premise
  under the whole entry is that *every stroke writes the take*, and that was
  never the architecture. A stroke is heard against a **scratch span** (a
  one-second buffer sustains 443 writes a second, and D4 already plays a copied
  block out of a buffer of its own), the edit happens over the **working copy**,
  and a take's pool buffer is replaced whole exactly once, on confirmation. So
  the O(buffer) write this entry calls asymptotically wrong is **correct at the
  one moment it is used**: the material changed, and replacing it whole is what
  that means. The lasting finding is the opposite of the one recorded here — a
  buffer stays immutable and replaceable, and editing never goes through the
  pool at all.

- ⬜ **A finished async command waits up to 100 ms to be reported** *(found
  2026-08-15, measuring the write cost above: every single write round-tripped
  in ~104 ms whatever its size, which is not a cost any buffer work explains)*.
  The network loop blocks in `recv_from` under a 100 ms read timeout
  (`GC_INTERVAL`, `src/osc/server/lifecycle.rs`) and collects NRT and Faust
  results **after** the recv returns — so with no other traffic a job that
  finished in 2 ms is reported at the next wakeup, and the `/server_sync` that
  waits on it answers then. Every async command pays it: a `/buffer_alloc`, a
  `/buffer_read`, a def compile, and any `wait=True` in the Python client. It
  hides whenever traffic is flowing (each packet drains the pipes too) and
  whenever writes are batched behind one barrier, which is why it went unnoticed
  — a batch of fifty divides the floor by fifty. The timeout is there so garbage
  is collected without traffic, which is a different need from *promptness on a
  result*: the fix is a wakeup when a result lands (the NRT thread poking the
  socket, as the TCP leg already does with a zero-length datagram) or a short
  timeout while jobs are in flight, not a smaller `GC_INTERVAL` for everyone.
