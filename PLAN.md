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

- **M31 — What a client needs to own its data paths: writing a buffer, and an identity on the ring** — two gaps the web client's W10 uncovered while opening the read paths to a script. Neither is a defect in something that shipped: both are capabilities a client turns out to need and the server has never offered, and both bound what a client-drawn view (an audio editor, a scope) can do. They ride together because they are the same sentence from the client's side — *a client should be able to read and write its own data without fighting another client for the channel* — and because doing them at once spends one breaking release instead of two.

  **(a) A buffer-write command.** The `/buffer_*` family has no write: it reads with `/buffer_get`/`/buffer_getRange` and has no command that puts samples back (scsynth's `/b_set`/`/b_setn` were commands as well as replies; ours are replies only, and now spelled as such). So a client can read a buffer's samples but never put samples into one — it can only ask the server to fill it (`/buffer_gen`, `/buffer_allocRead`, `/buffer_read`) or, when it shares memory with the engine, install them through the embed door (`b_load`, what the browser's in-page carrier uses). This is what an audio-editor view needs to close its read → edit → write cycle, and why W10's bulk path is read-only and its `Buffer.load` is in-page only. To settle: the install path (the NRT queue `/buffer_gen` already uses, so a write never touches the audio thread), the semantics (scsynth's `bufnum start count values…`, repeated, plus a single-sample form), how a multi-megabyte edit is chunked against the `--max-frame` ceiling, whether the write is asynchronous with a `/done` or synchronous on the mirror like `/buffer_getRange`, and whether an edited buffer needs any notification for other readers.

  **(b) Ring clients get identities.** Every packet arriving through the shared-memory / in-process ring is `ClientId::Ring` — a *single* client (`src/osc/mod.rs`), which `docs/ipc.md` already names as future work ("the transport keeps one ring client per segment … multiple ring clients … are explicitly future work"). The per-client subscriptions therefore collide: `/bus_stream` and `/bus_tapStream` are "one per client, replaced on each call", so two independent readers over one ring silently take the stream from each other. **The evidence**, from a browser page where the script and the GUI host both push through `engine.send`: the script subscribes `/bus_stream(20, bus 0)` and gets its snapshots; the host opens a `meter` and sends `/bus_stream(33, bus 1)`, after which the script receives **nothing**; the script re-subscribes and takes it back. The loss is **permanent in one direction** — the browser host only re-sends when its own wanted set changes (`clients/gui/src/host/web.rs`, `sync_bus_stream`), so once a script replaces the subscription the host's meters and scopes stay frozen until a widget is added or removed. Over sockets there is no such problem (a native host and a script are different `ClientId`s), so this is specifically the shared ring — and it also means two `BusStream`s in one page collide with each other. To settle: where a sender tag lives (a ring frame-header field, which moves the segment layout, or several rings), how the embed and wasm doors carry it (`clausters_send`, `WebServer::send`), whether replies stay broadcast on the shared reply ring (they can — the identity is needed for subscription bookkeeping, not for reply isolation), and what a peer built against the old layout sees. A page-side arbiter merging the two demands into one subscription was considered and set aside: it is a workaround for a missing server capability, and it would leave the same trap for every other ring embedder.

  **Versioning.** (b) moves the segment layout, so it bumps `ABI_VERSION` and, by the linkage rule, the SemVer breaking tier (the minor, pre-1.0). (a) is additive on the wire and bumps neither by itself.

  **Acceptance:** a client writes samples into a buffer and reads back exactly what it wrote, in chunks, over both a socket and the ring; and, on one page over the in-page carrier, a GUI host `meter` and a script `BusStream` on different buses **both keep updating** — the probe that fails today passes, as a headless acceptance — `clients/web/tests/ring-clash-probe.html` reproduces the collision and is deliberately outside that client's suite until this milestone makes it pass. Pairs with the Python client's `set_samples` and the web client's `setSamples`, in that order: the reference client leads, the port follows.

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
  plus `b_load` installing host-decoded samples through the same path as the
  async `/buffer_*` installs — the browser's `/buffer_allocRead` replacement),
  documented in `docs/using-as-a-library.md` as a supported native embed
  mode. `tests/headless.rs` drives it end to end (tone + `/done`s, `/bus_stream`
  pacing deterministic on sample time, a timed bundle landing on its exact
  mid-block sample, inline `/buffer_alloc`, `b_load` + `/buffer_query`, `/server_quit`
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
average hides. R11 turns this from a discipline into a CI gate; until it lands,
it is a discipline, and a discipline nobody checks is how the sediment got here
in the first place.

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

- ⬜ **R1 — One declaration per shared-core function.** The core's surface is
  hand-written **three times**: `clausters-ffi/src/lib.rs` (the C ABI),
  `clausters-core-web/src/lib.rs` (the wasm bindings, 72 functions) and
  `clients/python/clausters/_native.py` (133 lines of `argtypes`/`restype` over
  155 calls). Only the first is compiler-checked against core; the other two are
  on trust, so a function added to core reaches the clients only if three edits
  happen and match. Cheapest sufficient fix first: a **parity test** that fails
  when `ffi` and `core-web` do not expose the same set, and a generated (or
  test-checked) `_native.py` signature block. The fuller fix is a declarative
  manifest — one row per function, from which both `extern "C"` and the wasm
  export are generated by macro. Either way the acceptance condition is the
  same: adding a core function and forgetting a binding must **fail a build or a
  test**, never ship. Record the choice in `docs/decisions.md` — it governs how
  every later core function is added.

- ⬜ **R2 — Split the OSC front (`src/osc/server.rs`, 3217 lines).**
  `OscServer` carries five separable jobs: binding and draining the transports
  (UDP/TCP/WS/MIDI/IPC), packet and bundle dispatch, ~60 command handlers, the
  streaming subscriptions (`BusStream`/`TapStream`), and the async pipelines
  (NRT/Faust plus the `/server_sync` barrier). Split into
  `osc/server/{mod,transports,dispatch,streams,async_pipes}.rs` with one `impl
  OscServer` block per file — the struct and every signature stay put, so the
  diff is pure movement and `git log --follow` still reads.

- ⬜ **R3 — An argument reader, and one voice for failures.** Every handler
  re-derives the same destructuring by hand: 95 `OscType::Int` patterns feeding
  117 `self.fail(...)` sites whose message strings were each written
  individually and have drifted in wording. Introduce an `Args<'a>` reader
  (`int()`, `float()`, `str()`, `pairs()`, `rest()`, each returning `Result<_,
  String>`) and a `self.command(addr, |args| …)` wrapper mapping `Err` to
  `fail`, so a handler states what it wants and the failure text comes from one
  place. Collapse with it the five near-identical bus-range walkers
  (`/bus_set`, `/bus_get`, `/bus_setRange`, `/bus_getRange`, `/bus_fill`) onto
  one `for_each_bus_range` helper, and the `/buffer_get`↔`/buffer_getRange`,
  `/synth_get`↔`/synth_getRange` pairs likewise. The error *strings* clients
  read may be reworded here (they are prose, not protocol), but the `/fail`
  address and its arguments may not.

- ⬜ **R4 — Dispatch as a table, checked against the schema.** `handle_message`
  is a ~100-arm `match` on the address string. A `&[(&'static str,
  HandlerFn)]` table makes the command set **enumerable at runtime**, which buys
  the thing the `match` cannot: a test asserting that the dispatch table and
  `docs/schemas.md` list the same addresses. Today nothing checks that the
  reference documents the commands the server actually answers. Depends on R2
  (it lives in `dispatch.rs`) and reads best after R3.

- ⬜ **R5 — Split the translator (`src/osc/translate.rs`, 2747 lines).** Three
  concerns share a file and little else: the OSC→`Cmd` translation proper, the
  GraphDef instancing (`graph_new`/`graph_voice`/`alloc_graph_*`/
  `resolve_ports`, ~600 lines) and the MIDI layer (~1000 lines from
  `midi_bind` on: bind/unbind/map, note on/off, binding persistence). Split into
  `translate/{mod,graph,midi}.rs`. The MIDI half touches `clausters-midi` and
  nothing else in the translator beyond the tree mirror, so it separates
  cleanly.

- ⬜ **R6 — Split the client's `Server` facade
  (`clients/python/clausters/defs/server.py`).** The mirror image of R2 on the
  other end of the wire: ~35 public methods spanning boot, sending, queries,
  transport, streams and offline render, with `timeout: float = 5.0` copied into
  23 signatures. Regroup as collaborators or mixins (`ServerQueries`,
  `ServerTransport`, `ServerStreams`) hanging off the same `Server`, so no
  attribute path a script or an example uses changes, and make the timeout an
  instance default the per-call argument overrides. The manual test surface is
  the examples: run the ones that boot a server, query it and drive the
  transport. When it lands, the TS port's shape is the same — say so in
  `clients/web/PLAN.md` rather than letting `defs/server.ts` re-derive a
  different split.

- ⬜ **R7 — The widget's props declared once.** `gui/guidef.py` (1278 lines, 29
  builders) and `gui/guidef.ts` (1302) each enumerate every widget's props as
  explicit keyword arguments filtered through `_drop_none`. It reads well and
  documents itself, which is why it stays *shaped* like that — but a prop
  currently costs three synchronized edits (host, Python builder, TS builder)
  and nothing checks the three agree. Drive the builders from a declarative
  `{type: [props]}` table shared by both clients (generated from the host's
  widget registry, so `docs/gui-protocol.md` and the two builders cannot
  diverge from what the host accepts), keeping the typed signatures where they
  earn their keep. This is R1's problem in the GUI layer and should follow its
  decision.

- ⬜ **R8 — Catalogs split by family.** `clients/python/clausters/defs/ugens.py`
  (1971 lines, 161 functions) becomes `defs/ugens/{osc,filter,buf,demand,
  spectral,io}.py` re-exported from `ugens/__init__.py`; `src/dsp/registry.rs`
  (2443) keeps its descriptor-as-data design untouched — that design is the
  reason adding a UGen is one entry — and only splits the `UGENS` array into
  per-family tables concatenated at the end. Navigability only; no symbol moves
  out of its public path.

- ⬜ **R9 — Group the engine's command application.** `Engine::apply` is a
  ~210-line `match Cmd` and `process_block` ~215 lines. Group `apply` by family
  (`apply_node`, `apply_bus`, `apply_buffer`). **This is audio-thread code**:
  the change must be purely syntactic, and it is not finished until
  `tests/rt_safety.rs` and the golden renders pass untouched **and the bench
  says it costs what it cost before** — grouping a `match` into functions is
  exactly the shape of change that can stop being inlined, and `apply` runs
  per command per block. Lowest value in the track and the highest care — do it
  last, or not at all, and let the measurement decide which.

- ⬜ **R10 — Split the remaining oversized crate files.**
  `clausters-ffi/src/lib.rs` (1543) by domain — `builtins`, `time`, `rng`,
  `registry`, `sched`, `clocksync` — the way `notation.rs` and `ws.rs` already
  sit apart; `clausters-core/src/bundle.rs` (1287) along its assembly/decoding
  seam. Follows R1, since the manifest decides how the ABI functions are
  written before deciding which file they live in.

- ⬜ **R11 — The performance gate stops being a discipline.** Everything the
  track's cost invariant asks for is, until this lands, a step a human
  remembers: CI (`.github/workflows/ci.yml`) runs fmt, clippy, the test matrix
  and the GUI/wasm gate, and **no benchmark at all**, so a regression ships if
  whoever closed the milestone skipped the before/after run. This milestone
  makes the machine do it.

  **The hard part is not running the bench, it is having something to compare
  against.** `ubuntu-latest` runners are shared and noisy — 10–20% swings
  between runs of identical code are ordinary — so an absolute threshold
  against a committed number produces false alarms until the gate is ignored,
  which is worse than no gate. The design that survives that is a
  **same-runner, same-job A/B**: in one workflow step, build and bench the
  merge base, then build and bench the head, and compare *those two* numbers.
  Both measurements share the machine, the thermal state and the noise, so the
  ratio means something the absolute values don't. Cost is one extra release
  build per run, which is why this is its own milestone and not a line in the
  workflow.

  Shape to aim for:
  - `examples/bench.rs` learns a machine-readable mode (a `--json` flag or an
    env var) emitting per-section `{name, x_real_time, peak_block}`. Today it
    prints a human table; the gate needs to parse it, and the human table stays
    the default.
  - A script diffs two such JSONs and fails on a regression past a threshold
    **set generously** (start around 10%, tighten only if the observed run-to-run
    spread on the runner turns out narrower). Deliberately blind to small
    changes: this catches the 30% cliff from a lost inline, not a 3% drift.
  - It runs on the branch, not on every push to `main`, and a label or a
    commit-message opt-out exists — a refactor that knowingly trades speed for
    something else should be able to say so and move on.
  - Sections that are themselves noisy (anything JIT-dependent under `faust`)
    are reported but not gated, at least initially.

  **Acceptance: prove it catches something.** Land it with a deliberately
  pessimized commit (an `#[inline(never)]` on a hot path, reverted after) and
  show the gate going red — a threshold nobody has ever seen trip is a
  threshold nobody knows is wired up. Useful beyond this track: once it exists,
  the U track's new UGens and the S11 stagger work get the same protection for
  free.

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
  - **The eight configurations nobody automates.** Of the thirteen in
    `.claude/skills/feature-matrix/check.sh`, CI covers five (the two `fmt`s
    and three `clippy` runs). The other eight — `clippy` under neither def
    family, `synth` alone, `faust` alone, `clausters-ffi` with `verovio`, and
    all five `rustdoc` builds — are covered by a human remembering, which is
    what the skill exists to remind them of. Running the full `check.sh` in
    the release's `verify` job is the cheapest place to make that automatic:
    once per release rather than once per push, at the moment where being
    wrong is unrecoverable. `check.sh --fast` runs exactly the eight if the
    five are already proven by the same job. The `faust`-linking
    configurations need libfaust, which the release workflow already builds
    and caches through the `.github/actions/libfaust` composite — reuse it,
    don't add a second recipe.

  `check.sh` only ever reads: it is a gate, and it stays one. Nothing about
  wiring it into a workflow introduces a `--fix` pass — clearing a warning is
  deliberate, by hand, on one configuration at a time.

  **Acceptance: prove it blocks.** Tag a commit with a known-bad tree (a
  warning that only appears under one def family is the honest test, since that
  is the class CI cannot see) on a throwaway tag, and show the release stopping
  before any publish step. A gate that has only ever been observed passing is
  indistinguishable from no gate.

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
