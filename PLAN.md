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
- ✅ **M1 — OSC server** *(done 2026-06-10)* — UDP on port 57110 over rosc; `/status`, `/quit`, `/notify`, `/dumpOSC`.
- ✅ **M2 — RT-safe FIFO + node tree** *(done 2026-06-10)* — command/garbage ring buffers and a `NodeTree` with groups; `/s_new`/`/n_free`/`/n_set`, guarded by `assert_no_alloc` in the callback.
- ✅ **M3 — SynthDefs** *(done 2026-06-10)* — the serde def format, the UGen-graph interpreter, `/d_recv`, named/indexed controls, and the `Box<dyn SynthNode>` trait (the F-fork prerequisite).
- ✅ **M4 — Buses and order** *(done 2026-06-10)* — audio/control buses, `In`/`Out`, nested groups, `/s_new` add actions, `/n_before`/`/n_after`, the `/g_*` family, `/c_set`/`/c_get`, `/n_go`/`/n_end`; output is purely `Out`-driven.
- ✅ **M5 — Buffers** *(done 2026-06-10)* — the buffer pool + NRT thread, `/b_alloc`/`/b_read`/…, `PlayBuf`/`BufRd`, async `/done`; buffers are immutable `Arc`s swapped by the engine and freed via the garbage FIFO.
- ✅ **M6 — Sample-accurate scheduling** *(done 2026-06-10)* — a timetag-ordered bundle queue on the audio thread, NTP→samples on the network thread, and intra-block splitting at the event's sample (no `OffsetOut` needed).
- ✅ **M7 — NRT mode + golden tests** *(done 2026-06-11)* — offline WAV render on the same engine (`--nrt`, scsynth score format), golden regression tests and graph benchmarks.
- ✅ **M8 — The sample clock as the client's timebase** *(done 2026-06-12)* — `/clock` exposes the sample counter and `/sched <target> <blob>` schedules directly in samples, so a client models the OS-vs-DAC drift for exact relative timing; NTP and sample-clock clients coexist on the same queue.

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
- ✅ **F1 — Compiler thread** *(done 2026-06-10)* — a dedicated compile thread with a refcounted factory table and async `/done`/`/fail` for `/d_faust` (JSON blob; `/d_recv` stays the UGen format).
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
with done actions, **done** 2026-07-01 —, `/g_queryTree`, buffer
streaming) are taken as loose items when needed. (Historical detail for the
completed items lives in the git history.)

- ✅ **M9 — Developer documentation** *(done 2026-06-12)* — `docs/architecture.md` (threads, modules, memory lifecycle, invariants, "how to add a UGen") plus a rustdoc pass; records the Faust-UI-as-controls and the no-dynamic-plugins (no stable Rust ABI) decisions.

- ✅ **M10 — Bounded memory and alignment** *(done 2026-06-12)* — audited and documented every pre-allocated capacity and its full-behavior, cache-line-aligned wire/bus blocks, and updated the `realtime-audio` skill.

- ✅ **M11 — `/n_map`/`/n_mapa`: buses as a parameter source** *(done 2026-06-13)* — a node reads a control (or audio) bus into a control each block until `/n_map … -1` or `/n_set`; an RT-safe per-node mapping table over the existing bus atomics, schedulable like `/n_set`.

- ✅ **M12 — Canonical graph form via bus connections** *(done 2026-06-12)* — infer the read/write dependency DAG from the buses and offer opt-in auto-ordered groups (`/g_sortMode`, recomputed on the network thread; `/g_queryTree`/`/g_dumpGraph`; manual moves `/fail` inside them); feedback cycles keep the explicit order (one block of delay). No plugin delay compensation yet — a natural later addition via a `latency()` on the trait when an intrinsic-latency UGen lands (rationale in `docs/model-vs-daw.md`).

- ✅ **M13 — Parallel tree processing** *(done 2026-06-12; requires M12)* — the M12 DAG drives stage-based parallelism (N−1 RT workers, bounded spin+backoff, no locks); same-stage nodes write disjoint buses so it stays bit-identical to sequential, and NRT renders benefit too.

- ✅ **M14 — Pluggable transports, embedded mode and synchronous calls** *(done 2026-06-12)* — OSC decoupled from transport (UDP / shared-memory ring / in-process), a shared data plane (sample clock + control buses), a client-side synchronous facade, and the versioned-C-ABI cdylib that lets any language embed the server (the "only flat data crosses" boundary).

- ✅ **M15 — Comprehensive English documentation (README + mdBook + rustdoc)** *(done)* — a root README, the mdBook over `docs/` in place (`book.toml src="docs"`), and an oriented crate doc-comment, covering the OSC user, the library/embed user and the developer.

- ✅ **M16 — On-disk def persistence + bitcode cache** *(done 2026-06-16)* — defs persist to a data dir as transparent JSON (layer B) recompiled on reload, plus a non-authoritative libfaust bitcode cache (layer A); `--data-dir`/XDG resolution, `--no-persist`, incremental reload on the compiler thread.

- ✅ **M17 — MIDI: server protocol and client output (reusable Rust core)** *(done 2026-06-18)* — the `clausters-midi` crate (versioned C ABI, MIDI 2.0/UMP via `midi2`, SMF via `midly`, live ports via `midir`): server-side `/midi_bind` maps a channel to an instrument def + control map (SynthDef and FaustDef actuated identically, a MIDI voice byte-identical to the OSC path), and the client exports live ports and `.mid`/MIDI-2.0-clip files. This is client `C11`. Rationale (MIDI-2.0 resolution, native-crate decision, `midi2-clip` stub) in `docs/decisions.md`.

- ✅ **M18 — GraphDef: persistent node-graph definitions ("programs")** *(done 2026-06-19)* — a third persistent def kind saving a whole configuration of bus-wired nodes with a named parameter surface and a shared/per-voice split (`/d_graph`/`/graph_new`/`/graph_voice`); execution auto-ordered via M12, persisted like M16, MIDI-bindable like a def, with a `defs/graphdef.py` client builder.

- ✅ **M19 — MIDI-standalone operation: a playable server with no programming environment** *(done 2026-06-20)* — persisted MIDI bindings and an optional boot preset restored at startup (order: defs → graphdefs → bindings → preset), so a `--midi` server boots already wired and playable with no client; the server-side counterpart of the client's responders (C13).

- ✅ **M20 — Documentation split: two mdBooks (server + Python client) + generated API** *(done 2026-06-21)* — the English docs became two platform books (server `docs/`; the Python client's own book with a pydoc-markdown API page), cross-linked; no RST directives, no milestone labels in published docs.
- ✅ **M21 — Master clock anchor over OSC (shared, drift-free time reference)** *(done 2026-06-22)* — `/clock.reply` carries the server's OSC time anchored to its sample counter, so several OSC clients convert logical time to one common drift-free sample axis and schedule via `/sched` (embed/shm read the counter directly). Pairs with client C14.
- ✅ **M22 — Shared transport: a queryable master beat grid (phase alignment)** *(done 2026-06-22)* — a server-hosted `/transport` (origin sample + tempo) clients join to quantize starts onto one shared grid: sample-exact when locked to a master, beat-accurate otherwise. A separate optional layer over M21; pairs with client C15.

- ✅ **M23 — Continuous integration + publishing (CI, Read the Docs, PyPI)** *(done 2026-07-05)* — a CI workflow (fmt/clippy, `cargo test` over the def-family feature matrix, the gui + wasm gate, the Python suite, both mdBooks, and a from-source libfaust job) and a `v*`-tag release workflow (self-contained wheel + server tarball, PyPI Trusted Publishing). Account-side activation steps recorded in `docs/contributing.md`.
- ✅ **M24 — Real-time health: RT scheduling, CPU metering, affinity, stress harness** *(done 2026-07-08)* — the `rtprio` default feature (RT-scheduled callback with a ground-truth policy diagnostic), an RT-safe CPU meter (avg/peak/late-blocks in `/status`), experimental `--pin`, and `examples/stress.rs`; a SIGXCPU guard degrades instead of dying under overload. The opt-in→default reversal is in `docs/decisions.md`.
- ✅ **M25 — TCP as the default command transport + a configurable frame ceiling** *(done 2026-07-12)*: the server accepts TCP alongside UDP **by default** (same port 57110, scsynth-style; `--no-tcp` opts out, `--tcp [port]` still moves it), and the fixed 64 KiB `MAX_FRAME` becomes a boot option (`--max-frame <bytes>`, default 16 MiB) shared by the TCP and WebSocket fronts — with length-prefixed framing the ceiling is a DoS guard, not a protocol limit, so it is configuration, sized for the target deployments (loopback + controlled networks, not a public service). Replies become **transport-aware**: a stream client (TCP/WS/ring) may receive frames up to the ceiling (`/b_getn` chunks, `/g_queryTree`, the `/tap_stream` window clamp), while a UDP client keeps the datagram cap; `/server_info` advertises the ceiling so clients size their requests instead of hardcoding it. UDP itself is untouched — it remains the discovery/boot protocol and carries small real-time control fine; the IPC command rings keep their 64 KiB (big payloads ride TCP even locally; a ring-size option would bump the versioned segment layout and is deferred until a real need). Every network front stays **individually optional** (a packaged desktop/mobile standalone runs the embedded server over the in-process link and needs no sockets at all), and no limit is hard-wired: whatever bounds a payload is a boot option with a sensible default, never a constant — the project must stay usable as a desktop or mobile application without arbitrary ceilings. Rationale — the transport roles: UDP = discovery + small control, TCP = the command plane, shm = the data plane, WS = the browser's TCP — goes to `docs/decisions.md`. Pairs with client C34 and GUI host G25.
- ✅ **M26 — Network-edge hardening: fuzzing the decode door + bounding the stream fronts** *(done 2026-07-16)*: the pre-publication hardening pass over the transports. A **cargo-fuzz harness** (`fuzz/`, nightly-only, not a workspace member) fuzzes `osc::decode_packet` — the single door every transport funnels through, so one target covers the whole inbound parse surface — from a small versioned seed corpus; run it before releases that touch the OSC path (recipe in `docs/contributing.md`). The stream fronts get **edge guards**, all shared between TCP and WebSocket: a `--max-clients` ceiling on concurrent connections (default 64, scsynth's `maxLogins` in spirit — each connection costs a thread, so the count is bounded like every other boot-time pool; a connection past it is dropped at accept, a freed slot is reusable), **bounded inbound queues** (a flooding client blocks on TCP flow control instead of growing server memory — no rate limit: dense control traffic is legitimate, the bound is resources, not message rate), and **slow-consumer eviction** on the reply path (a TCP write timeout so a client that stopped reading cannot stall the single-threaded command loop; a bounded WebSocket reply queue whose overflow drops the connection). UDP is untouched — connectionless, datagram-capped, the kernel sheds overload. Size ceilings were already M25's `--max-frame`; this closes the count/backpressure half.

- ✅ **M27 — The curated PV set: parameterized operations, not a catalog** *(done 2026-07-18)* — grow the `PV_*` vocabulary to cover the musically common spectral operations **without porting scsynth's one-UGen-per-op catalog** (whose sc3-plugins tail shows where that leads: dozens of near-duplicate plugins freezing booleans into names — `PV_MagAbove`/`Below` are one algorithm and a flag, which our `PvMag`/`MagMode` already demonstrates). Four additions, each one *implementation* with modes: (a) extend `PvMag` with a `clip` mode and register `PV_MagClip`; (b) a **two-chain combiner** — one binary PV implementation whose operator is a parameter, registered under the scsynth-compatible names (`PV_Add`/`PV_Mul`/`PV_Min`/`PV_Max`/`PV_MagMul`/`PV_CopyPhase`); needs the compiler to let one spectral UGen take **two** chain slots (result lands in chain A, the wire keeps ordering, both `SpectralChain`s stay synth-private); (c) the **stateful pair** `PV_MagFreeze`/`PV_MagSmear` (per-instance frame memory, allocated at build); (d) one **bin-remap** implementation behind `PV_BinShift` (shift + stretch covers `PV_MagShift`). Anything beyond these waits for M29's general mechanism rather than joining a catalog — record that stance in `docs/decisions.md`. Tests extend `tests/spectral.rs`; a commented example in `examples/`; wire reference in `docs/schemas.md`.

- ✅ **M28 — Partitioned convolution: one UGen, kernel prepared off the RT thread, flat load** *(done 2026-07-18)* — a single well-parameterized convolution UGen instead of scsynth's five variants (`Convolution`/`2`/`2L`/`3`/`StereoConvolution2L`), designed around the two constraints the `bench` spectral section quantifies: the FDL multiply–accumulate is the dominant per-hop cost (~217 µs for a 2 s IR at 48 kHz, 16% of a block budget), and it need not land on one block. Pieces: (a) kernel spectra **precomputed off the audio thread** into an immutable pool buffer by a typed `/b_gen` routine (the moral heir of scsynth's `PreparePartConv`, per the S-track `/cmd` stance) — the RT side only ever FFTs its input block and MACs against ready-made spectra, and a kernel swap is an `Arc` swap with a parameterized crossfade (subsuming `Convolution2L`; no re-FFT on the audio thread, which is where scsynth's `Convolution2` violates our rules); (b) **load spreading**: the P partition MACs distributed across the hop's blocks so the steady-state cost is flat (~P/blocks-per-hop partitions per block), leaving only the input FFT/IFFT pair on the hop block; (c) convolution runs **outside** the `fr` chain (its discipline — zero-padded rectangular segments, hop fixed by partition size — is incompatible with the windowed COLA analysis chain; same reason scsynth keeps them apart); (d) it is the first UGen with **intrinsic latency**, so add the `latency()` hook on `SynthNode` anticipated by M12 and report it (full PDC stays deferred per `docs/model-vs-daw.md`); a direct time-domain path for short kernels can come later as a degenerate partition case. Acceptance: a golden test against direct convolution, and a bench row showing the spread MAC flattening the peak-block column.

- ✅ **M29 — A general per-frame spectral mechanism (design spike first)** *(done 2026-07-18: spike + implementation)* — the long-term answer to the PV-catalog problem: make the spectral frame **user-programmable** so new bin operations stop requiring server releases. Two candidate designs, to be decided in a written spike before any implementation: (a) **bin algebra** — expose magnitude/phase as frame-rate values the existing S3 operator vocabulary composes (the Max/MSP `pfft~` model: the spectral domain as a substrate the graph itself processes; touches the compiler and the rate system); (b) a **JIT per-frame kernel** — a compiled callback over the frame via the existing Faust family patterns (compile on the network thread, RT-safe run; no registry growth). The spike weighs compiler surface vs. JIT dependency, NRT determinism, and the client-side authoring story, and lands as a `docs/decisions.md` entry; implementation follows only on real need (every M27 op we decline to add is this milestone's demand signal). **Outcome**: the spike surfaced and chose a third design that dominates both on every axis — a **bin-expression program**: one `PV_Kernel` UGen interpreting a compile-time-validated postfix program over per-bin values, opcodes riding the `clausters-core::builtins` op table, authored client-side with (a)'s operator algebra; no new rate, no JIT, exact NRT, full feature matrix. (b) is repositioned as the escalation path for kernels beyond a per-bin map. Recorded in `docs/decisions.md` — and **implemented the same day**: `clausters_core::pvprog` (program + RT-safe evaluator), the variadic `PV_Kernel` row, `mag_expr`/`phase_expr` wire fields validated at `/d_recv`, the Python `clausters.defs.pv_expr` symbolic terms + `pv_kernel`, sample-identical equivalence tests against `PV_MagAbove`/`PV_BrickWall` in `tests/spectral.rs`, RT-safety coverage, `examples/spectral_kernel.py`, and the use-cases/restrictions docs in both books.

- ✅ **M30 — The introspection verbs: what a running server holds** *(done 2026-07-19)* — the retrieval surface a client palette needs, three queries in the `/server_info` mold, adding **no** node/def semantics anywhere. **`/d_query [name...]`** → one `/d_info` per def then `/done`, listing the loaded defs with `name, family` (`synth`/`faust`/`graph`) and their control surface (`name, default, rate`); a faust def appends each param's `min, max, step`, a graph def reports its surface **ports** with the inner `member, control, mul, add` targets each drives, and an unknown name comes back with an empty family rather than failing the batch. **`/b_query` with no argument** → one `/b_info` listing every allocated buffer in the existing four-arg shape. **`/u_query [kind...]`** → one `/u_info` per UGen then `/done`: arity (`-1` variadic), default/allowed rates, exec/bus/op/spectral roles, and the **named inputs with defaults**. That last field did not exist — `UGenDescriptor` carried only a count — so the descriptor grew `inputs` and all sixty rows were filled by reconciling the `docs/schemas.md` catalog table with the Python callables' signatures; the wire stays positional and no def changes behavior, the names being descriptive metadata a palette labels an inlet with (rationale, and why `ugens.py` stays hand-written behind a contrast test instead of generated, in `docs/decisions.md`). Multi-reply batches close with `/done "<command>"` so an argument-less query has an end marker, and each item is its own message because the payloads are variable-length and a whole catalog would outgrow a UDP datagram. A build without `synth` has no catalog and replies with an empty listing, not a failure — Faust has no UGens, only FaustDefs, and its box vocabulary stays client-side. Python: `Server.query_defs()` / `query_buffers()` / `query_ugens()` returning `DefInfo`/`BufferInfo`/`UgenInfo` (named in the existing `query_*` family — a bare `buffers()` would have shadowed the `BufferAllocator` attribute), the anti-drift contrast test against `ugens.py`, and `examples/introspect_server.py`. Pairs with GUI P2.

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

## S track — Synthesis-engine infrastructure completion (the substrate for future UGens)

Section added 2026-07-01. The base UGen set and the node/bus/def machinery are in place, but before *growing the UGen library* we finish the **substrate** every future UGen leans on, so that adding a UGen later is a self-contained job (a `process`, a registry entry, a test) with no engine surgery. Everything here is deliberately **infrastructure, not a UGen catalog**: the concrete DSP UGens it enables — the demand family (`Dseq`/`Dseries`/`Dwhite`), `FFT`/`IFFT` and PV_* processing, the table oscillators (`Osc`/`VOsc`/`Shaper`), more filters — land afterwards as loose items (per "Future milestones (M9+)"), each cheap once the substrate exists. Like the F fork, S coexists with the M line and does not replace anything; the pieces are largely independent (S1 enables S2's `ir` controls and the future demand/FFT UGens; S5 depends on S6's `/b_gen` slot but is written together; S4 pulls `/n_run` in from S6 because it is tied to pause semantics). Canonical scsynth lists below were **verified against the SuperCollider source** (`server/plugins/UnaryOpUGens.cpp`, `BinaryOpUGens.cpp`, `HelpSource/Classes/Done.schelp`) on 2026-07-01.

**Design stance — compatibility of *model*, not literal copy.** These milestones take scsynth's set as the reference for *completeness* (so no capability is missing), but each is free to adopt a **better implementation consistent with what Clausters already built** where scsynth's is weak — the same "conceptual, not binary, compatibility" principle stated at the top of this plan. scsynth carries historical warts we need not inherit: the clearest example is the plugin-command pair **`/cmd`/`/u_cmd`** (S6), which were never cleanly designed (untyped, ad-hoc argument blobs, per-plugin conventions with no schema) — where we need that mechanism we should design a **typed, discoverable** command surface fitting our JSON/OSC conventions rather than reproduce the loose original. Likewise favor our existing machinery (auto-ordered groups, the bus analysis, the network-thread pre-build, the versioned ABIs) over scsynth idioms that predate it. Each S milestone should call out, when it deviates, *why* and *what* it does instead.

- ✅ **S1 — Calculation rates (`ir`/`kr`/`ar`/`dr`) as a first-class property** *(done)* — the output/control rate becomes an explicit, compiler-validated property (with an `ir` init pass and a minimal `dr` demand-pull driver), the substrate for the demand and FFT families.

- ✅ **S2 — Typed controls: `tr`, `lag`/`varlag`, and scalar (`ir`) controls** *(done)* — control types the def author chooses: trigger controls reset after one block, lagged controls insert a shared `Lag` at compile time, `ir` controls freeze at init.

- ✅ **S3 — Special-index operator UGens (`UnaryOpUGen`/`BinaryOpUGen`) + `MulAdd`/`Sum3`/`Sum4`** *(done 2026-07-02)* — two generic op UGens whose `op` is the operator's name over the full scsynth opcode set, computed by the shared `clausters-core` functions for bit-identical client/server parity; `Add`/`Sub`/`Mul`/`Div` stay as aliases.

- ✅ **S4 — Complete the done-action set + `/n_run` (resume) + non-terminal pause** *(done 2026-07-03)* — the full 0–15 `DoneAction` enum (the relative-node actions resolved on the audio thread) and `/n_run`, which makes `PauseSelf` non-terminal.

- ✅ **S5 — Wavetable & table-generation infrastructure (`/b_gen`) + the table oscillators** *(done 2026-07-03)* — `/b_gen` (`sine1`/`sine2`/`sine3`/`cheby`/`copy`) fills buffers through the immutable-`Arc` NRT path, the scsynth interleaved wavetable format, and `Osc`/`OscN`/`VOsc`/`Shaper` as the first consumers.

- ✅ **S6 — Complete the scsynth OSC command set** *(done 2026-07-03)* — the missing node/group/bus/synth/buffer/def/scheduling vocabulary (`/n_setn`/`/n_fill`/`/n_mapn`, `/g_head`/`/g_tail`/`/n_order`, `/c_setn`/`/c_getn`/`/c_fill`, `/s_get`/`/s_getn`, `/b_close`, `/d_load`, `/clearSched`) plus a typed `/cmd`/`/u_cmd` surface and `/error`.

- ✅ **S7 — Boot-time server configuration (audio I/O channels + every pre-allocated pool)** *(done 2026-07-03)* — `--inputs`/`--outputs` (with a real audio-input path via a second cpal stream) and `--max-nodes`/`--max-buffers`/`--max-graph-children`/`--max-ugen-inputs`, chosen at boot and reported in `/server_info`.

- ✅ **S8 — FFT/IFFT and the spectral (`fr`) chain** *(done 2026-07-03)* — `FFT`/`IFFT` + `PV_MagAbove`/`PV_MagBelow`/`PV_BrickWall`, one frame per hop; the transform planned at init (allocation-free per-hop), synth-private spectral scratch, and `/u_cmd` as the live window-swap channel (S6's first consumer).

- ✅ **S9 — Side-effect UGens (no `Out` required)** *(done 2026-07-03)* — `SendTrig`/`SendReply`/`Poll` sending replies out the RT-safe reply FIFO; a valid def may contain only side-effecting UGens (the client relaxation is C19). Write-only UGens (`Out.kr`/`DiskOut`/`RecordBuf`) deferred with the buffer/streaming work.

- ✅ **S10 — Finite-resource registries: every id allocator recycles** *(done 2026-07-16)* — one shared `clausters_core::registry::Registry` (occupancy map, FFI-exposed) behind the server's `/s_new -1` auto range, the MIDI voice range and the GraphDef private buses, and behind the Python client's node/bus/buffer allocators; the node-id space partitioned by `NodeIdPartition::from_max_nodes` (replacing the 2M/3M counters); client ids recycled via `/n_end`, engine rejections broadcast `/fail` with the id appended so nothing is lost; NRT node ids unbounded by design. Rationale in `docs/decisions.md`.

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
  async `/b_*` installs — the browser's `/b_allocRead` replacement),
  documented in `docs/using-as-a-library.md` as a supported native embed
  mode. `tests/headless.rs` drives it end to end (tone + `/done`s, `/c_stream`
  pacing deterministic on sample time, a timed bundle landing on its exact
  mid-block sample, inline `/b_alloc`, `b_load` + `/b_query`, `/quit`
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
  `/status` round trip over the MessagePort, engine clock advance, and the
  `/s_new` sine measured at an AnalyserNode (the verdict beaconed through the
  HTTP access log — real-time audio vs. Chrome's virtual time, see
  `docs/decisions.md`).

- ✅ **B3 — GuiDef standalone equivalence: a bundle boots in a tab**
  *(done 2026-07-18)* — `ServerLink::Page` in the GUI host (wasm-only:
  outbound OSC to a page-registered callback via `GuiBridge.connect_page`;
  inbound via `GuiBridge.server_reply`), the streamed data paths
  (`/c_stream`, `/tap_stream`, `/b_getn`, `/clock`) unchanged over it; the
  bundle boot's ordering/encoding in the platform-agnostic `host::bundle`
  (natively unit-tested, mirroring the server's own data-dir boot order,
  bracketed by two `/sync`s — the second is the page's "bundle up" signal),
  exposed to JS as `bundle_boot_packets`; the fetch half in
  `clients/gui/web/bundle.js` (+ `bundle.json` manifest, the one addition to
  the persisted formats — HTTP cannot list directories;
  `web/bundle-manifest.py` generates it) and the page
  `web/standalone.html`; samples fetch + `decodeAudioData` → the engine's
  `bLoad` over the worklet port. Acceptance `scripts/smoke-web-standalone.sh`:
  a native-format bundle (SynthDef spec + GuiDef with `boot`/`bind`) boots
  entirely in a headless-Chrome tab, `/synced` confirms, and the meter's
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
  `server()` sees the element's synth (`/status`), meter bus streaming.

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
  duty cycle declares two inputs rather than three so `/u_query` never reports an
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
  `Line` and `XLine` assemble `EnvGen`'s input layout in their stack frame
  rather than growing a second ramp, so they inherit the whole done-action set,
  the exact landing on the target and the shared `envshape` arithmetic a client
  draws with. Plus S9's deferred `FreeSelf`, `PauseSelf`, `FreeSelfWhenDone`,
  `Done`, in `src/dsp/nodectl.rs`. Two findings shaped the result. The **done
  flag is not the done action**: `Done` exists precisely for an envelope whose
  `doneAction` is 0, so reading the action would leave it blind, and the flag is
  not on a wire either (a finished envelope sits at its final level, which is
  just a number) — hence `UGen::is_done` and an `ExecMode::DoneQuery` that
  resolves input 0's *identity* the way the demand driver already does, with the
  compiler rejecting a source that can never finish. And **`PauseSelf` must not
  latch**, or `/n_run 1` would be useless: the action is recomputed per block.
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
  for white). That bit-level property is **not observable from the output** at
  all, since the word is 31 bits and an `f32` mantissa 24. And `Crackle` does
  **not** settle below a chaos of 1: there is no period up to 512 samples
  anywhere in 0.3–1.9, and its spread is not monotonic in the parameter.
  `LFNoise2` overshoots to ±1.7 by construction (it aims at midpoints and
  carries its slope), which is stable — the peak is the same over one second
  and over ten, at 5 Hz, 100 Hz and 2 kHz.

- **U7 — Panning and selection** — `src/dsp/pan.rs`. The engine gives a UGen
  **one output** (an input reference names a UGen, not an output of one), a
  deviation `docs/schemas.md` already states for the buffer readers. So a
  two-output panner is a row carrying its channel index and sharing the pan-law
  helper, and the Python `pan2()` returns a `ChannelList` of two — exactly what
  `out()` already does. `Pan2`, `LinPan2`, `Balance2`, `Rotate2`, `PanAz` that
  way; `XFade2`, `LinXFade2`, `Select`, `SelectX` as ordinary single-output rows.

- **U8 — The demand family** — extending `src/dsp/demand.rs` on the `dr`
  substrate S1 built and the shared RNG: `Dseries`, `Dgeom`, `Dwhite`, `Diwhite`,
  `Dbrown`, `Dibrown`, `Drand`, `Dxrand`, `Dshuf`, `Dswitch1`, `Dbufrd`,
  `Dstutter`, plus the drivers `Duty` and `TDuty`.

Every U milestone ships its rows **with** their Python builders in
`clients/python/clausters/defs/ugens.py` (the contrast test keeps the input names
identical to the registry's; `/u_query` picks the rows up with no further work),
the catalog table in `docs/schemas.md`, and the usual milestone checklist.

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
  is user-facing — not just the code and the git history.
