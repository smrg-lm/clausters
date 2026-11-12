# Implementation plan: a real-time scsynth-style audio server: the closed tracks

The tracks of [`crates/clausters-server/PLAN.md`](../../../crates/clausters-server/PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

The entries of the plan's "Found by use" and "Future directions" lists that
were fixed or done are at the end of this file, each moved whole once it was
ticked.

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

- ✅ **M12 — Canonical graph form via bus connections** *(done 2026-06-12)* — infer the read/write dependency DAG from the buses and offer opt-in auto-ordered groups (`/group_sortMode`, recomputed on the network thread; `/group_queryTree`/`/group_dumpGraph`; manual moves `/fail` inside them); feedback cycles keep the explicit order (one block of delay). Intrinsic latency and general server compensation are supplied by M28 and J8 (rationale in `docs/model-vs-daw.md`).

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
- ✅ **M26 — Network-edge hardening: fuzzing the decode door + bounding the stream fronts** *(done 2026-07-16)*: the pre-publication hardening pass over the transports. A **cargo-fuzz harness** (`audit/fuzz/`, nightly-only, not a workspace member) fuzzes `osc::decode_packet` — the single door every transport funnels through, so one target covers the whole inbound parse surface — from a small versioned seed corpus; run it before releases that touch the OSC path (recipe in `docs/contributing.md`). The stream fronts get **edge guards**, all shared between TCP and WebSocket: a `--max-clients` ceiling on concurrent connections (default 64, scsynth's `maxLogins` in spirit — each connection costs a thread, so the count is bounded like every other boot-time pool; a connection past it is dropped at accept, a freed slot is reusable), **bounded inbound queues** (a flooding client blocks on TCP flow control instead of growing server memory — no rate limit: dense control traffic is legitimate, the bound is resources, not message rate), and **slow-consumer eviction** on the reply path (a TCP write timeout so a client that stopped reading cannot stall the single-threaded command loop; a bounded WebSocket reply queue whose overflow drops the connection). UDP is untouched — connectionless, datagram-capped, the kernel sheds overload. Size ceilings were already M25's `--max-frame`; this closes the count/backpressure half.

- ✅ **M27 — The curated PV set: parameterized operations, not a catalog** *(done 2026-07-18)* — grow the `PV_*` vocabulary to cover the musically common spectral operations **without porting scsynth's one-UGen-per-op catalog** (whose sc3-plugins tail shows where that leads: dozens of near-duplicate plugins freezing booleans into names — `PV_MagAbove`/`Below` are one algorithm and a flag, which our `PvMag`/`MagMode` already demonstrates). Four additions, each one *implementation* with modes: (a) extend `PvMag` with a `clip` mode and register `PV_MagClip`; (b) a **two-chain combiner** — one binary PV implementation whose operator is a parameter, registered under the scsynth-compatible names (`PV_Add`/`PV_Mul`/`PV_Min`/`PV_Max`/`PV_MagMul`/`PV_CopyPhase`); needs the compiler to let one spectral UGen take **two** chain slots (result lands in chain A, the wire keeps ordering, both `SpectralChain`s stay synth-private); (c) the **stateful pair** `PV_MagFreeze`/`PV_MagSmear` (per-instance frame memory, allocated at build); (d) one **bin-remap** implementation behind `PV_BinShift` (shift + stretch covers `PV_MagShift`). Anything beyond these waits for M29's general mechanism rather than joining a catalog — record that stance in `docs/decisions.md`. Tests extend `tests/spectral.rs`; a commented example in `examples/`; wire reference in `docs/schemas.md`.

- ✅ **M28 — Partitioned convolution: one UGen, kernel prepared off the RT thread, flat load** *(done 2026-07-18)* — a single well-parameterized convolution UGen instead of scsynth's five variants (`Convolution`/`2`/`2L`/`3`/`StereoConvolution2L`), designed around the two constraints the `bench` spectral section quantifies: the FDL multiply–accumulate is the dominant per-hop cost (~217 µs for a 2 s IR at 48 kHz, 16% of a block budget), and it need not land on one block. Pieces: (a) kernel spectra **precomputed off the audio thread** into an immutable pool buffer by a typed `/buffer_gen` routine (the moral heir of scsynth's `PreparePartConv`, per the S-track `/server_cmd` stance) — the RT side only ever FFTs its input block and MACs against ready-made spectra, and a kernel swap is an `Arc` swap with a parameterized crossfade (subsuming `Convolution2L`; no re-FFT on the audio thread, which is where scsynth's `Convolution2` violates our rules); (b) **load spreading**: the P partition MACs distributed across the hop's blocks so the steady-state cost is flat (~P/blocks-per-hop partitions per block), leaving only the input FFT/IFFT pair on the hop block; (c) convolution runs **outside** the `fr` chain (its discipline — zero-padded rectangular segments, hop fixed by partition size — is incompatible with the windowed COLA analysis chain; same reason scsynth keeps them apart); (d) it is the first UGen with **intrinsic latency**, so add the `latency()` hook on `SynthNode` anticipated by M12 and report it (general server compensation is supplied by J8, per `docs/model-vs-daw.md`); a direct time-domain path for short kernels can come later as a degenerate partition case. Acceptance: a golden test against direct convolution, and a bench row showing the spread MAC flattening the peak-block column.

- ✅ **M29 — A general per-frame spectral mechanism (design spike first)** *(done 2026-07-18: spike + implementation)* — the long-term answer to the PV-catalog problem: make the spectral frame **user-programmable** so new bin operations stop requiring server releases. Two candidate designs, to be decided in a written spike before any implementation: (a) **bin algebra** — expose magnitude/phase as frame-rate values the existing S3 operator vocabulary composes (the Max/MSP `pfft~` model: the spectral domain as a substrate the graph itself processes; touches the compiler and the rate system); (b) a **JIT per-frame kernel** — a compiled callback over the frame via the existing Faust family patterns (compile on the network thread, RT-safe run; no registry growth). The spike weighs compiler surface vs. JIT dependency, NRT determinism, and the client-side authoring story, and lands as a `docs/decisions.md` entry; implementation follows only on real need (every M27 op we decline to add is this milestone's demand signal). **Outcome**: the spike surfaced and chose a third design that dominates both on every axis — a **bin-expression program**: one `PV_Kernel` UGen interpreting a compile-time-validated postfix program over per-bin values, opcodes riding the `clausters-core::builtins` op table, authored client-side with (a)'s operator algebra; no new rate, no JIT, exact NRT, full feature matrix. (b) is repositioned as the escalation path for kernels beyond a per-bin map. Recorded in `docs/decisions.md` — and **implemented the same day**: `clausters_core::pvprog` (program + RT-safe evaluator), the variadic `PV_Kernel` row, `mag_expr`/`phase_expr` wire fields validated at `/def_send synth`, the Python `clausters.defs.pv_expr` symbolic terms + `pv_kernel`, sample-identical equivalence tests against `PV_MagAbove`/`PV_BrickWall` in `tests/spectral.rs`, RT-safety coverage, `examples/spectral/kernel.py`, and the use-cases/restrictions docs in both books.

- ✅ **M30 — The introspection verbs: what a running server holds** *(done 2026-07-19)* — the retrieval surface a client palette needs, three queries in the `/server_query` mold, adding **no** node/def semantics anywhere. **`/def_query [name...]`** → one `/def_query.reply` per def then `/done`, listing the loaded defs with `name, family` (`synth`/`faust`/`graph`) and their control surface (`name, default, rate`); a faust def appends each param's `min, max, step`, a graph def reports its surface **ports** with the inner `member, control, mul, add` targets each drives, and an unknown name comes back with an empty family rather than failing the batch. **`/buffer_query` with no argument** → one `/buffer_query.reply` listing every allocated buffer in the existing four-arg shape. **`/ugen_query [kind...]`** → one `/ugen_query.reply` per UGen then `/done`: arity (`-1` variadic), default/allowed rates, exec/bus/op/spectral roles, and the **named inputs with defaults**. That last field did not exist — `UGenDescriptor` carried only a count — so the descriptor grew `inputs` and all sixty rows were filled by reconciling the `docs/schemas.md` catalog table with the Python callables' signatures; the wire stays positional and no def changes behavior, the names being descriptive metadata a palette labels an inlet with (rationale, and why `ugens.py` stays hand-written behind a contrast test instead of generated, in `docs/decisions.md`). Multi-reply batches close with `/done "<command>"` so an argument-less query has an end marker, and each item is its own message because the payloads are variable-length and a whole catalog would outgrow a UDP datagram. A build without `synth` has no catalog and replies with an empty listing, not a failure — Faust has no UGens, only FaustDefs, and its box vocabulary stays client-side. Python: `Server.query_defs()` / `query_buffers()` / `query_ugens()` returning `DefInfo`/`BufferInfo`/`UgenInfo` (named in the existing `query_*` family — a bare `buffers()` would have shadowed the `BufferAllocator` attribute), the anti-drift contrast test against `ugens.py`, and `examples/introspect_server.py`. Pairs with GUI P2.

- ✅ **M31 — What a client needs to own its data paths: writing a buffer, and an identity on the ring** *(done 2026-08-04)* — two gaps the web client's W10 uncovered while opening the read paths to a script. Neither is a defect in something that shipped: both are capabilities a client turns out to need and the server has never offered, and both bound what a client-drawn view (an audio editor, a scope) can do. They ride together because they are the same sentence from the client's side — *a client should be able to read and write its own data without fighting another client for the channel* — and because doing them at once spends one breaking release instead of two.

  **(a) A buffer-write command.** The `/buffer_*` family has no write: it reads with `/buffer_get`/`/buffer_getRange` and has no command that puts samples back (scsynth's `/b_set`/`/b_setn` were commands as well as replies; ours are replies only, and now spelled as such). So a client can read a buffer's samples but never put samples into one — it can only ask the server to fill it (`/buffer_gen`, `/buffer_allocRead`, `/buffer_read`) or, when it shares memory with the engine, install them through the embed door (`buffer_load`, what the browser's in-page carrier uses). This is what an audio-editor view needs to close its read → edit → write cycle, and why W10's bulk path is read-only and its `Buffer.load` is in-page only. To settle: the install path (the NRT queue `/buffer_gen` already uses, so a write never touches the audio thread), the semantics (scsynth's `bufnum start count values…`, repeated, plus a single-sample form), how a multi-megabyte edit is chunked against the `--max-frame` ceiling, whether the write is asynchronous with a `/done` or synchronous on the mirror like `/buffer_getRange`, and whether an edited buffer needs any notification for other readers.

  **(b) Ring clients get identities.** Every packet arriving through the shared-memory / in-process ring is `ClientId::Ring` — a *single* client (`src/osc/mod.rs`), which `docs/ipc.md` already names as future work ("the transport keeps one ring client per segment … multiple ring clients … are explicitly future work"). The per-client subscriptions therefore collide: `/bus_stream` and `/bus_tapStream` are "one per client, replaced on each call", so two independent readers over one ring silently take the stream from each other. **The evidence**, from a browser page where the script and the GUI host both push through `engine.send`: the script subscribes `/bus_stream(20, bus 0)` and gets its snapshots; the host opens a `meter` and sends `/bus_stream(33, bus 1)`, after which the script receives **nothing**; the script re-subscribes and takes it back. The loss is **permanent in one direction** — the browser host only re-sends when its own wanted set changes (`crates/clausters-gui/src/host/web.rs`, `sync_bus_stream`), so once a script replaces the subscription the host's meters and scopes stay frozen until a widget is added or removed. Over sockets there is no such problem (a native host and a script are different `ClientId`s), so this is specifically the shared ring — and it also means two `BusStream`s in one page collide with each other. To settle: where a sender tag lives (a ring frame-header field, which moves the segment layout, or several rings), how the embed and wasm doors carry it (`clausters_send`, `WebServer::send`), whether replies stay broadcast on the shared reply ring (they can — the identity is needed for subscription bookkeeping, not for reply isolation), and what a peer built against the old layout sees. A page-side arbiter merging the two demands into one subscription was considered and set aside: it is a workaround for a missing server capability, and it would leave the same trap for every other ring embedder.

  **A possible optimization, noted from (a) and deliberately not taken there.** The *read* path still costs one round trip per chunk — 623 ms for 200k samples over the shared-memory carrier, against 121 ms for the write of the same samples once its chunks were batched behind one `/server_sync`. That asymmetry is inherent to request/reply: a write is fire-and-forget with a barrier at the end, while every read chunk has to wait for the reply that carries its data. It could be pipelined — send every `/buffer_getRange` at once and collect the replies as they arrive — but that needs reply-matching machinery neither client has (a request today is one send and one await), so it belongs to whoever opens that layer rather than to a buffer-write milestone.

  **Versioning.** (b) moves the segment layout, so it bumps `ABI_VERSION` (6 → 7) and, by the linkage rule, the SemVer breaking tier (the minor, pre-1.0) — the package version itself moves when the release is cut, not here. (a) is additive on the wire and bumps neither by itself.

  **What shipped.** (a): `/buffer_set` (single samples by flat index) and `/buffer_setRange` (runs), on the NRT queue like the rest of the writing family, laid into a copy that replaces the buffer whole; a range past the end **fails** rather than being clamped, since a short write would lose data the caller believes it stored. Two things turned up while building it and are recorded in `docs/decisions.md`. First, bulk samples ride as a **little-endian `f32` blob**, not as float arguments — 200k samples took 2.7 s as typed arguments against 0.1 ms as one blob — so `/buffer_getRange.reply` changed to match, and the rule (payload scaling with the audio → blob; with the parameters → typed arguments) is written in `docs/schemas.md` rather than re-derived. Second, a job rebuilding a buffer from its contents snapshotted the network-side mirror at *parse* time, which is wrong for a batch: every chunk copied the same pre-batch contents and the last install erased the rest, so the NRT queue now keeps its own view of what it last produced per index. With that safe the chunks close with **one** `/server_sync` instead of a `/done` each. Clients: `Buffer.set_samples`/`set_sample` and `setSamples`/`setSample`, each with one shared pack/unpack (`clausters.base.bulk`, `src/base/bulk.ts`) so the endianness check has one owner. Example `clients/python/examples/buffers/buffer_edit.py`.

  (b): each ring frame carries a `u32` **peer tag** beside its length — who authored the packet inbound, who the reply is for outbound — and `ClientId::Ring` becomes `ClientId::Ring(u32)`. The tag lives in the *frame*, so no header or data-plane offset moved and the hand-written readers (`crates/clausters-gui/src/host/shm.rs`, `clients/python/clausters/ipc.py`) needed only their version constant; several rings were considered and set aside for moving the layout and fixing a client count at boot. SPSC survives because the tag is about the packet, not the ring: a multi-client embedder funnels sends through one producer and demultiplexes replies by tag, which is what the page already did through its one `MessagePort`. Tags are the embedder's to assign (no handshake; peer 0 is the single client a segment always had, so existing embedders are untouched), and the C ABI stays deliberately single-peer since its one consumer is itself one client. Replies being addressed removed the page-wide eavesdrop, so the web client grew an explicit `ANY_PEER` **read** door for observers, which two page tests now use rather than reaching into another client's internals.

  **Acceptance (met).** A client writes samples into a buffer and reads back exactly what it wrote, in chunks, over a socket and over the ring (`tests/buffers.rs`, `clients/python/tests/test_buffer_io.py`, `clients/web/tests/data-ws.test.ts`), and a batch of writes no longer erases itself (`a_batch_of_writes_does_not_erase_itself`, verified to fail without the queue's chain). Two ring peers keep their own `/bus_stream` subscriptions (`tests/ipc.rs`), and on one page over the in-page carrier a GUI host `meter` and a script `BusStream` on different buses **both keep updating**: the probe that used to demonstrate the collision is now the acceptance, `clients/web/tests/ring-peers.html`, in that client's suite — and verified to reproduce the original failure exactly (phase 2 leaves the script at 0 snapshots, and the host never recovers) when both peers share a tag.

- ✅ **M32 — Watching an audio bus is asking for a bus** *(done 2026-07-28)* — `/bus_tap tapIndex bus` made the caller allocate one of the segment's eight sample rings, route the bus into it, and carry that index everywhere a view needed it, so the server's own bookkeeping leaked into the wire and into every client. It becomes **`/bus_tap bus watch`**: the server picks the ring, publishes the bus → ring mapping in a new per-bus region of the segment (**`ABI_VERSION` 3 → 4**, and by the linkage rule the next release takes the breaking tier), and **counts watches**, so two views of one bus share a ring and the last one to stop frees it; running out fails loudly instead of drawing nothing. `/bus_tapStream` lists **buses** and the subscription *is* the watch, which is what lets a browser client never send `/bus_tap` at all. The same region carries a **per-bus level** — the block peak held with a 20 dB/s decay — so a meter reads one number per block instead of holding a ring: a mixer's worth of meters is now possible, where three stereo channels used to exhaust the region. The decay (rather than a max the reader clears) is what keeps the value correct for a reader an order of magnitude slower than the engine *and* for several readers at once; `tests/rt_safety.rs` guards the added per-block work. Pairs with GUI host G33, which is where the widget surface (`bus`/`rate`/`channels`) and both client ports live. Rationale in `docs/decisions.md`.

- ✅ **M33 — A group is called something** *(done 2026-08-01)* — a group was addressable only by the id its client happened to allocate, which is enough for a node tree and not enough for a console: a DAW channel, a console group, a send are all *named* things, and a client building them out of groups had no way to say which was which. **`/group_new` takes an optional name per group** and **`/group_name groupID name`** renames one: a **referenceable label on top of the id** — the id remains the identity every command addresses and every reply reports, the name is a second way to refer to the same group — unique among siblings, cleared by an empty name, changed by the same command. The label makes the tree navigable by **path**: every group contributes a segment (its name, or its **decimal id** when unnamed, so nothing falls out of addressing), the root is `/`, and **`/group_query <path>`** resolves one to its id, replying `-1` when nothing answers (absence is a state, the `/node_query` rule). A path is never accepted where a node id goes — resolve once, then command by id — which keeps the ~25 node commands' parsing and their hot path untouched. Three rules the server enforces: unique among siblings, never all digits (an unnamed group answers to its id), no `/` (the server composes the path). A name carried by `/group_new` is judged **before** the group exists, against the group it would land in, so a refused label refuses the creation rather than leaving an anonymous group behind. Reported everywhere a node is: `/node_query.reply` after `headID, tailID`, `/group_dumpGraph` quoted next to the id, `/group_queryTree.reply` giving every node the uniform shape `id, count, name`, and the **`/node_start`/`/node_end` notifications** as a last argument — so a client watching the tree learns which channel came up or went away, and for a death there is no query left to make (the mirror keeps a departing group's name as a bounded **epitaph**, since it drops the entry at translation while the death notice waits on the engine). The shapes follow the feature rather than scsynth's, which is not a cost: nothing but our own clients speaks this protocol, and the packages move together. **The name lives only in the network-side `TreeMirror`**: `node::Group` grew no field, no `Cmd` reaches the engine, and the audio thread never sees a name, so the feature has no RT cost by construction. Ports in both clients (`Group("mixer")` / `Group.new(server, {name})`, `rename`, `Server.group_at` / `groupAt`, and the tree/node parsers), `examples/mixer_paths.py`, rationale in `docs/decisions.md`.

- ✅ **M34 — Where the server's time goes, and it is not the operating system's answer** *(opened 2026-09-15 by the user, asking how to measure the server's CPU use per thread, and then -- reading the first sketch -- rejecting the name it had: "why did you name the API method threads?". The rejection is the milestone's shape and not a detail, so it is recorded first.)*

  **What was missing.** `/server_status` reports one CPU figure, the audio thread's per-block load (M24). That answers *is the server keeping up* and not *what is it spending the time on*, which is the question a session that has grown heavy actually asks: a FaustDef compiling, a soundfile loading, a burst of commands and the DSP itself all cost real time, and only one of them is in that number. Outside the process the answer exists -- the threads are named (`clausters-dsp-N`, `faust-compiler`, `nrt`, `diskin`), so `top -H`, `pidstat -t` and `perf` read it -- but that is Linux's answer, not the server's, and no client can ask for it.

  **The name, which is the decision.** The first sketch was `/server_threads` / `Server.threads()`, taken from those tools. It is wrong twice over, and both are standing rules here rather than taste: a thread is a **language and OS construct**, so it never names an API surface; and the brackets do not measure threads at all -- they measure a block, a stage, a compilation, a job. So the unit is the **role**, the command is **`/server_load`**, and the client verb is **`load`**, which also reads as the breakdown of the one figure `status` already carries. A server that ran one of these roles somewhere else -- an engine in a page, where there is one thread -- reports the same rows, which is exactly what a name built on threads could not have done.

  **The server times itself; it does not ask the platform.** Each piece of work brackets itself with a monotonic clock and adds the elapsed nanos to its role's slot, so the reading is the same on Linux, macOS and Windows with no per-OS thread-inspection API: `Instant::now` is `clock_gettime(CLOCK_MONOTONIC)` through the vDSO, `mach_absolute_time`, `QueryPerformanceCounter` -- no allocation, no lock, no kernel trap, which is what makes the bracket legal on the audio thread and is the same reasoning M24's own meter rests on. The OS route was considered and is not the fallback either: `clock_gettime(CLOCK_THREAD_CPUTIME_ID)` is a real syscall (not in the vDSO), macOS has no such clock and answers through `thread_info`, Windows through `GetThreadTimes` -- three implementations of one number, and the one place it would be most wanted is the one place it may not be called.

  **Cumulative, and the window belongs to the reader.** Slots only grow; a client differences two replies into an interval of its own. That is a correction as much as a design: `Counters::take_peak_cpu` resets on read, so two clients polling one server steal each other's window today, and a second reset-on-read meter would have doubled the defect. Both clients compute the `share` field client-side from their own previous call.

  **What `busy` is not.** It is time the work was **in progress**, not per cent of a core: a DSP worker spinning for its next stage is burning a core and is idle by this reading. That is the honest answer to how much of the block budget a stage took -- which is the question a musician has -- and the documentation says plainly that a system profiler answers the other one.

  **What shipped.** `server::meters`: a fixed slot table built at boot (one `audio`, one `dsp` per worker, `net`, `nrt`, and `faust` when the feature is in), each slot two relaxed atomics -- cumulative nanoseconds and a call count -- plus a `stamp()`/`elapsed_nanos()` pair that is inert on `wasm32`, where there is no monotonic clock and every row reads zero, exactly as `/server_status`'s CPU fields already do there. The audio row is fed by the measurement the CPU meter **already takes**, so the block costs no second clock read; a worker brackets the stage rather than the subtree; the serving turn is bracketed in pieces so the blocking recv is never counted as work. On the wire `/server_load` → `/server_load.reply [uptime, n, n x (role, index, busy, calls)]`, in `docs/schemas.md` beside `/server_status`; `Server.load()` / `Load` / `format_load` in Python and `Server.load()` / `Load` / `formatLoad` / `loadName` in TypeScript, each differencing its own window; `docs/architecture.md` under real-time health; `clients/python/examples/basics/server_load.py`.

  **What it costs**, because a meter that changes what it measures is worthless: two clock reads and two relaxed read-modify-writes per bracket, **none of them new on the audio thread** (the block reuses M24's stamp) and one pair per stage per worker. `examples/bench.rs` measured before and after, interleaved on one machine (`base, head, base, head`, 80 throughput rows): the second `base` and the second `head` agree row for row -- `default/512` 3.82x against 3.86x, `default/1` 321.5x against 317.6x, `wavetable/sine/32` 76.9x against 77.3x -- and `scripts/bench-gate.py` passes on that pair. The *first* head run read ~10% low on **every** row including `default/1`, which is a machine event and not a cost that scales with the graph; run-to-run noise between two identical builds reached +9.5% in the same session. So the cost is under the measurement floor, which is what two atomics per block should be.

  **What is deliberately not metered, and it is a real gap.** The `diskin`/`diskout` threads (`dsp::disk`), which are spawned per `DiskIn`/`DiskOut` node from the UGen registry and so have no reach to the table without threading it through the node build path; they are dynamic and numerous, and would want an aggregated role rather than one slot each. Recorded in "Found by use" rather than folded in here.

  **Acceptance (met).** A poll lists one row per role and one per worker, never a row for a worker the server does not have; the audio row counts one call per block and the net row one per turn; two polls never report less than the first (`tests/osc.rs::server_load_reports_every_role_cumulatively`); each worker accounts the stages it took and a sequential engine reports no `dsp` row at all (`tests/parallel.rs::every_worker_accounts_the_stages_it_took`); the audio thread stays allocation-free (`tests/rt_safety.rs`); and the example reads a live server's roles while it plays, with the NRT row moving for a `/buffer_gen` neither CPU figure would have shown.

- ✅ **M35 — MPE: the expression belongs to the note, and the shape it takes is MIDI 2.0's** *(opened 2026-09-19; done 2026-09-29, with `crates/clausters-apps/PLAN.md` `X3.11` next on it)*

  **What is missing.** `/midi_bind` binds **one channel** to an instrument, and every expressive message on it reaches **every** voice of that channel (`midi_set_channel`). MPE is the opposite arrangement: a *zone* of member channels, one note per channel at a time, and bend/pressure/timbre addressed to that one note through the channel it sits on. Nothing in the server knows what a zone is, and the control-change path never looks at **RPN** — which is how a zone is declared (RPN 6, over CC 101/100/6/38). So an MPE controller routed into the virtual port today plays the notes and loses all three expression dimensions: they either go nowhere or, once mapped, go to every voice at once.

  **The decision: the internal model is per-note, and MPE 1.0 is a decoder in front of it.** `ChannelVoiceMessage` already normalizes every value to MIDI 2.0 / UMP resolution while carrying only MIDI 1.0's *message set*. MIDI 2.0 has per-note pitch bend and per-note controllers natively — which is precisely what MPE emulates by spending a channel per note. So the model grows `PerNotePitchBend` and `PerNoteController` (with a per-note identity), the actuation path learns to address one voice, and **MPE becomes a stateful front-end that turns channel-per-note into those messages**. One actuation path instead of an MPE mode beside the plain one, and a later UMP transport then costs a parser rather than a second design.

  **MIDI 1.0 compatibility is not a fallback here, it is the wire.** MPE *is*
  MIDI 1.0 — a convention over its channel-voice messages, which is the whole
  reason it spends a channel per note — and the transport decodes nothing else
  today (`live.rs` reads three bytes through `parse_midi1`). So the per-note
  internal model is a *representation*, never a requirement on the wire: a
  plain keyboard on one channel keeps arriving as it does now. What that costs
  is three rules, each of which breaks an existing setup if it is left implicit:

  - **The RPN parse only listens on a zone's channels.** Declaring a zone means
    watching CC 101, 100, 6 and 38 — and `/midi_map ccN` takes *any* number
    today, those four included, so a decoder that swallows them globally
    silently steals a control from a binding that already maps CC 6. A channel
    with no zone over it passes every control change through raw, as it does
    now.
  - **A zone of zero members is how a device says "plain MIDI again"**, per the
    specification, and it has to be honored: the zone dissolves and its channels
    go back to being ordinary bound channels rather than leaving the server in a
    mode nothing can leave.
  - **Bend is pitch only inside a zone.** For a per-channel binding, pitch bend
    keeps doing exactly what it does today — `bend2control` into whatever
    `/midi_map bend` named, or nothing at all when it named nothing. The
    note + member bend + master bend sum is the *zone's* rule, not a new global
    one, and `midi_set_channel` stays the per-channel path untouched.

  A channel held by a per-channel binding and covered by a zone at once is the
  one case where the two models meet, and it is **refused by whichever command
  creates the overlap** — a zone whose members reach a bound channel, or a
  `/midi_bind` on a channel a zone already covers — rather than resolved by
  precedence: two owners of one channel is a configuration mistake, and saying
  so is cheaper than picking a winner the user cannot see. The same holds for
  the layout a device declares: an RPN that would widen a zone over a bound
  channel is refused as the command would be, and the binding stands.

  **Where it lives, which is not where it looks like it lives.** There are two MIDI stacks today and they do not touch: `src/midi/` (input transport + bindings + actuation, midir directly) and `crates/clausters-midi` (file I/O and live ports for the clients, behind a flat-data C ABI); the root crate does not depend on the crate. The **zone layout, the RPN parse and the per-note state machine are language-agnostic and wanted at both ends** — the server decodes them on input, a client assigns channels on output — so they belong in `clausters-midi`, which its own module doc already anticipates ("and, later, the server"), and the root crate takes its first dependency on it. What stays server-side is the binding and the actuation (`osc/translate/midi.rs`), which are about defs and nodes and have no MIDI logic to share. **The ABI shape is settled** (`docs/decisions.md`): the decoder crosses as an **opaque handle** — `clausters_mpe_decoder_new`/`feed`/`poll`/`free`, feeding and polling separate because a master bend turns one message into several — and **not** as a caller-held state block, whose layout would have become the ABI and made every change to what the decoder remembers a `MIDI_ABI_VERSION` bump. It is the shape `clausters_midi_input_open`/`poll`/`close` already has, and it is **not** behind the `live` feature: this piece is pure computation, it has to build for wasm, and the server calls the safe Rust face (`Decoder::feed`/`poll`) directly. `MIDI_ABI_VERSION` goes to 3.

  **The pieces.**
  - **Zones, and the verb is its own.** Parse RPN 6 into a layout (lower zone: master channel 1 plus N members ascending; upper zone: master 16 plus N descending). The binding is **`/midi_bindZone master members instrument [target addAction gate]`** rather than extra arguments on `/midi_bind`, whose first argument would otherwise mean a channel in one call and a master in another; it joins the second-tier full-word namespace the protocol already uses, and leaves the existing command's parsing untouched. The `members` argument is the layout for a controller that **sends no RPN** — `0` means "wait for one" — and **RPN 6 wins at runtime**, a device being authoritative about its own layout. Persisted in `midi.json` with serde defaults, so the files a running setup already has keep loading.
  - **A binding is readable back, which today none is.** The dispatch carries `/midi_bind`, `/midi_unbind` and `/midi_map` and no query at all, so a client cannot ask what a channel is bound to — let alone what zone covers it. **`/midi_query [channel...]`** in the introspection mold: one `/midi_query.reply` per binding and then `/done "/midi_query"`, reporting the channel *or* the zone (master plus its members), the instrument, `target`/`addAction`/`gate` and the resolved control map. Argument-less it lists every binding, and an unbound channel comes back empty rather than failing the batch.
  - **Routing.** A bend, a channel pressure or a CC 74 arriving on a **member** channel reaches that channel's note; on the **master** it reaches every note of the zone. `midi_set_channel` stays as it is for the plain per-channel binding.
  - **Bend is pitch, and it has a range.** MPE's defaults are +/-48 semitones on a member and +/-2 on the master, and the two **sum**: a voice's frequency is `midi2freq(note + bend_member * range_member + bend_master * range_master)`. Today `bend2control` yields a bipolar -1..1 into a named control, which cannot express any of that; the range is per zone (RPN 0) and still mappable to a control for defs that want the raw value.
  - **The third dimension is a CC number, not a selector wired to one.** The specification assigns Y to **CC 74 by default** and lets a device use another, so a `timbre` selector hardwired to 74 would be wrong. The **zone carries the number** (74 unless configured) and `timbre` names the control it drives; `/midi_map cc74` goes on meaning what it means today, for whoever thinks in messages rather than in dimensions. The two open-source schools split exactly there — JUCE and Surge name the dimension (`pressure`/`pitchbend`/`timbre`), VCV Rack and MIDI 2.0 itself name the message or its number, CC 74 staying 74 — and separating the number from the name is what serves both.
  - **A zone is playable unmapped.** A plain binding defaults `freq`/`amp` and nothing else; a zone also defaults `pressure` and `timbre` (the table below has the whole set). It costs nothing — an unknown control name is already a no-op on the actuation path — and it is what lets a controller play its three dimensions into a def that declares them without a single `/midi_map`. **The names are ours**: the specification names no parameters, only dimensions and the messages that carry them, so these come from JUCE's vocabulary and from the spellings the selector table already uses — `pressure` rather than `press`, which is the host's word for a gesture, and `lift` a full word like the rest. The only names the specification would have endorsed are `y` and `z`, which say nothing to whoever writes the def.
  - **Note identity, and the three tables it takes.** `(channel, note)` is not a key under MPE: a controller reuses member channels and repeats a note number across them. A note-on allocates a **note id** (what UMP hands over anyway) and the state becomes three lookups — `note id -> voice` (the voices table, rekeyed), `(channel, note) -> note id` for the note-off to resolve, **newest first** when a channel repeats a note, and `channel -> note ids` for a member channel's expression to find what is sounding on it. The plain per-channel path keeps its `(channel, note)` key and is not rekeyed.
  - **Voice budget.** A zone is up to 15 simultaneous voices, all drawn from the reserved MIDI range of the node-id partition; check that `NodeIdPartition::from_max_nodes` sizes it for a zone held down with the sustain pedal, and fail loudly rather than silently dropping notes.

  **The whole selector table afterwards**, since the two bullets above add to a
  set that is spread across three paragraphs otherwise. A default applies when
  nothing maps the selector; "zone" marks a default a zone gives and a plain
  per-channel binding does not.

  | Selector | Message | Default control | Notes |
  |---|---|---|---|
  | `note` | note on/off number | `freq` | |
  | `vel` / `velocity` | note-on velocity | `amp` | |
  | `gate` | (note off, for a gate-aware binding) | `gate` | |
  | `bend` | pitch bend | none | inside a zone it also folds into `freq`, mapped or not |
  | `pressure` | channel pressure | `pressure` (zone) | |
  | `poly` | per-note aftertouch | none | to that note's voice |
  | `timbre` | the zone's third-dimension CC (74 unless configured) | `timbre` (zone) | **new** |
  | `lift` | note-off velocity | none | **new**, discarded today |
  | `ccN` | control change `N` | none | |
  | `progN` | program `N` | (selects an instrument def) | |

  **Four ends, not two, because the packages move together.** The **GUI host** is one of them and it is easy to miss: `crates/clausters-gui` has the `midi` feature **on by default**, depends on `clausters-midi` with `live`, and paints live input into a `midi_in` roll through `parse_note` — which reads note-on and note-off and nothing else, so an MPE note would be painted flat, without the bend that is the point of playing it. The host binds the same decoder (it already depends on the crate) and the roll paints the per-note pitch, so a glide reads as a glide. Host-managed voices (`host/voices.rs`) sending expression *out* is a later thing and not this milestone's.

  Python's `MidiServer` emits on a fixed channel: output needs a **channel assigner** — round robin over the members, preferring a channel with nothing sounding, and when every member is busy reusing the **least recently used** one (the note that has been held longest), which is the rule JUCE's `MPEChannelAssigner` settles on — the zone RPN written at the head of a file or a port, and `Event` fields for the per-note dimensions. The **web client has no MIDI surface at all** — an existing divergence this milestone does not deepen: it is written into `clients/web/PLAN.md` as the gap it is, naming the shape the port follows.

  **The reference implementation.** JUCE's `juce_audio_basics/mpe`. What is worth taking is the *separation*, not the code: `MPEZoneLayout` (zones and the RPN that declares them), `MPEInstrument` (the note state machine holding the five dimensions of every sounding note), `MPEValue` (7- and 14-bit unified), `MPEChannelAssigner` (the output side). The first three map almost one to one onto what `clausters-midi` grows here.

  **Acceptance.** A zone declared over RPN is recognized and readable back; a per-channel binding that maps CC 6 still receives it, and a plain MIDI 1.0 keyboard with a bend wheel behaves exactly as it does before this milestone (a regression test over the existing path, not an inspection); a member bend moves only its own voice while a master bend moves the whole zone; a full controller stream (three dimensions x 15 channels) drives one def with no allocation on the audio thread (`tests/rt_safety.rs`); a take recorded through the Python client writes out with its expression and reads back on another host; `docs/schemas.md` gains the zone commands, `docs/architecture.md` the decoder's place, and an example plays an MPE controller into a gate-aware def.

  **Shipped** *(2026-09-29)*. `clausters_midi::mpe`: `Decoder` (zones on
  either side, RPN 0 and 6 read on a configured master only, a device's RPN 6
  winning, a zone of zero members playing its master, `set_allowed` refusing an
  RPN that would widen a zone over a bound channel), `Assigner`,
  `zone_messages`, `bend_message` and `expression_messages`, with the C ABI
  shell as handles (`MIDI_ABI_VERSION` 4; the version record moved to
  `docs/ipc.md`). **One change from the shape above**: the per-note model is the
  decoder's own message, `MpeEvent` (a note id minted at the note-on; a bend
  already summed in semitones), which the translator actuates directly --
  `ChannelVoiceMessage` did not grow per-note variants, since nothing else
  would produce them until a UMP transport does. The server holds one decoder
  in its bindings and the live input reaches it as bytes
  (`translate_midi_bytes`); `/midi_bindZone`, `/midi_query`, `/midi_map`'s
  `timbre` (with its controller number) and `lift`, zones persisted in
  `midi.json` with serde defaults. An event lane's `midi` list decodes through
  the zones' layout when it is set, so an MPE stream on a lane plays per note
  (`T9`'s path). Python's `MidiServer(zone=..., upper=...)` declares the zone
  and assigns each note a member with its `bend`/`pressure`/`timbre`; the GUI
  host's live input runs through the same decoder (both zones waiting) and the
  roll paints a zone note at its bend and retunes it as the bend moves. The
  voice budget needed nothing: the MIDI id range is twice `--max-nodes`. Tests:
  `crates/clausters-midi` (the decoder and the assigner), `tests/mpe.rs`,
  `tests/lanes.rs` (a lane's MPE bend on its sample), `tests/rt_safety.rs` (a
  full stream), `test_midi.py`; `examples/midi_mpe.py` is the manual test.
  **Moved to `X3.11`**: a take read back *with* its expression -- reading a
  `.mid`'s MPE into a sequence's per-note curves is the notes editor's
  conversion, and writing one from those curves is too.

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
  *Revised 2026-10-13:* Rust UGen plugins stay out; third-party CLAP and VST3
  plugins use defined binary interfaces — CLAP's C ABI and VST3's COM-style
  interfaces — and are planned out of process by the J track. Rust types never
  cross that boundary.

## B track — the engine in the browser (wasm)

Section added 2026-07-18. Compile the engine to `wasm32` and run it **in the
page** behind an AudioWorklet, so browser GUI components stop needing a native
server process: first the engine itself (headless, then live), then the GuiDef
standalone equivalence (a bundle boots entirely in a tab — the browser twin of
`--standalone`), and only then the web-component packaging as a thin capstone.
This is the track `crates/clausters-gui/PLAN.md`'s "In-browser audio engine" section
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
  acceptance `web/smoke.html` + `scripts/smoke-web.sh worklet` under headless Chrome:
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
  `crates/clausters-gui/web/bundle.js` (+ `bundle.json` manifest, the one addition to
  the persisted formats — HTTP cannot list directories;
  `web/bundle-manifest.py` generates it) and the page
  `web/standalone.html`; samples fetch + `decodeAudioData` → the engine's
  `bLoad` over the worklet port. Acceptance `scripts/smoke-web.sh standalone`:
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
  node/bus/buffer namespace. Acceptance `scripts/smoke-web.sh components` +
  `demo.html?smoke=1`: element up with the canvas in its shadow root, raw
  `server()` sees the element's synth (`/server_status`), meter bus streaming.

- ✅ **B5 — Faust in the page: the compiler in the Worker, the DSP in the engine's memory** *(done 2026-08-26)*.
  The one capability the browser engine lacked and the window's had: `/def_send
  faust` used to answer `/fail` in a tab. It is the engine half of the web
  client's **W7** and the two ship together — W7's in-page acceptance is this
  milestone's too, the way G25 pairs with M25.

  The design is recorded in `docs/decisions.md` ("The page's Faust is a second
  wasm module linked into the engine's own memory"). In outline: the def is
  compiled by `libfaust-wasm` in the **NRT/compiler Worker** B6 adds (the page's
  compiler thread, the worklet being its audio thread — it was written here as
  the main thread until B6 priced that at the GUI host's frames) and the module
  bytes plus the JSON come back over the same port the OSC does, so `/def_send
  faust` keeps its existing asynchronous shape and **nothing on the wire
  changes**. The emitted module is `-lang wasm-e`, instantiated against the
  engine's own `WebAssembly.Memory` with its `compute` in the engine's
  `__indirect_function_table` and its math imports bound to the engine's
  exports; the DSP struct is allocated by us and the parameter zones are plain
  stores, so `FaustSynth` keeps its shape and `/node_set`, `/node_map`, the bus
  summing and the done actions are the same code in both builds.

  The pieces, in the order they were checked:

  - ✅ **The backend seam in `src/faust`** *(2026-08-25)*. What varies by target
    is how a factory is created and an instance computed, not what a `FaustDef`
    *is*. The `faust` feature keeps meaning "the FaustDef family exists" — it
    stopped implying libfaust — so the browser build is `synth,faust,embed` and
    the two families stay peers on both targets; `scripts/check-wasm.sh` grew
    the set. `compiler_web` is a queue the host drains, `synth_web` the node.
  - ✅ **The link constraints**, checked by the build rather than by reading
    *(2026-08-25)*: `--export-table` and `--growable-table`, plus
    `--keep-lld-exports` on the engine's bundle, since wasm-bindgen drops the
    exports the linker synthesized and the table is one of them. **The third
    constraint this entry named was wrong**: `--global-base` cannot be moved,
    because rustc links with `--stack-first` and the flag must then be at least
    the stack size — so the wasm backend's data segment at absolute offset 0 is
    **stripped** instead (`faust::wasm_module::strip_data_section`, which
    refuses rather than guesses). The correction is in `docs/decisions.md`.
  - ✅ **The compile leg in the page** *(2026-08-25)*: `libfaust-wasm` built
    from `third_party/faust` at the existing pin by
    `third_party/build-faust-wasm.sh`, staged by `build.sh` as a static asset
    **off** `dist/runtime.js` and imported on the first `/def_send faust`. That
    build carries a patch of its own — upstream binds the Box and Signal APIs
    for the native library and not for the wasm one, so a page could otherwise
    compile Faust source alone.
  - ✅ **One interpreter, not two** *(2026-08-25)*, which was not in this list
    and should have been. A box or signal tree is read by `faust::boxes` and
    `faust::signals` compiled to wasm, driving the page's compiler through its C
    API; the two modules do not share an address space, so a small shim copies
    the strings and handle arrays across. Reading the schema again in TypeScript
    would have been a second implementation of one thing, which is the failure
    the whole milestone exists to avoid.
  - ✅ **The engine's memory is reserved, not grown** *(2026-08-26)*.
    `WebAssembly.Memory.grow` **detaches** the `ArrayBuffer` and every JS view
    over it, and in a page it runs on the audio thread: the OSC pump is inside
    the worklet's `process`, so the allocation a command asks for happens where
    nothing may allocate. The engine was taking rustc's default — 24 pages,
    1.5 MB, whatever the data segments and the stack needed — and booting the
    server alone landed at 4.31 MB, so it grew dozens of times before playing a
    sample. It now reserves 16 MB and declares a 256 MB ceiling
    (`--initial-memory` / `--max-memory` in `crates/clausters-web/build.rs`,
    beside the table flags), which puts the boot inside the reservation and
    makes a page that outgrows it fail at a named limit rather than take iOS
    Safari's tab down near 350 MB.

    Two things this entry expected turned out not to be work. The **glue
    already re-derives** its views — wasm-bindgen guards every cached array
    with `byteLength === 0`, which is exactly how a detached buffer reads — and
    nothing hand-written caches a view over the engine's memory across calls
    (`worklet.ts` holds its own `interleaved`, not a view; `faust-shim.js`
    derives inside the call that uses it). So what was left was the
    reservation, and only that.

    `clients/web/tests/memory.test.ts` asserts both numbers off the binary's
    own memory section — the flags pass through wasm-bindgen, which rewrites
    the module, and an instance can only report the size it currently has, so a
    ceiling that stopped being emitted would be invisible from JS — and that a
    booted engine processing a thousand blocks does not move off the
    reservation.

  **Acceptance:** a FaustDef built from signals, from boxes and from source
  compiles and sounds in a tab with no server process, addressed by `/node_set`
  through the node tree like any other synth; the wasm render of a Faust score
  matches the native NRT render on `scripts/parity-web.sh`'s existing tolerance;
  a page mounting a SynthDef-only bundle loads no compiler asset; and the
  transcendental imports resolve to the engine's own exports, asserted rather
  than assumed.

  **How it came out.** All four clauses hold, and **one of them holds
  differently than it was written**: the Faust score does *not* meet the native
  render on the existing tolerance. It could not — a UGen graph is the same Rust
  compiled twice, while a Faust def is generated by two different backends, and
  a recursive filter turns their coefficient rounding into phase drift. The
  measured number is 7.2e-5 against the graph scene's 1.5e-8, so the Faust scene
  has a bar of its own; the reduction is on
  `clients/web/docs/src/platform.md` and the reasoning is with the parity item
  below.

  The other three: all three def forms compile in the Worker, link into the
  engine's memory and sound at the pitch they declare, with `/node_set "freq"`
  reaching the zone the module reads (`clients/web/tests/faust.html`); a bundle
  with no FaustDef fetches no compiler, asserted out of the HTTP access log
  because the import happens where no window can see it; and every import the
  module declares is resolved from the engine's own exports, an unknown one
  named rather than left to fail.

  **And the five things the milestone still owed**, every one of them closed on
  2026-08-26 and two of them turning up a defect on the way:

  - ✅ **A page's offline render compiles a Faust def** *(2026-08-26)*.
    `server::render` loads a def **where it stands** — time does not advance
    until it has — and a page's compiler is another scope that answers later, so
    there was no turn in which a compiled def could arrive and the path refused
    with a message. The fix is not an `await` inside the render, as this entry
    guessed, but the same work in **the other order**: the host reads the
    score's Faust defs before the render starts (`Score::faust_jobs`, exposed as
    `faustJobs` — the engine's *own* reader, so a score the render understands
    and the pre-pass does not cannot happen), compiles each in the Worker that
    already compiles for the live engine, links the module into the renderer's
    own memory and table (`faust-link.ts`, now shared with the worklet rather
    than copied), and deposits it through `linkFaust`. The renderer's
    `/def_send faust` is then a lookup in `faust::compiler_web`'s prelinked
    store, and everything after it is the code a native render runs. The
    compiler Worker is the renderer's own and is started on the first Faust def
    a score sends: a page can render with nothing booted, and a def linked into
    the worklet's instance could not be used by the renderer anyway.

    `tests/faust.html` renders the same three defs it just played: peak 1.20 —
    three sines of 0.4 summed in phase — at 299 Hz.
  - ✅ **The parity vector** *(2026-08-26)*. `gen_parity` emits a second scene
    — `score-faust.bin` + `native-faust.f32` — and `tests/parity.html` renders
    it through the client's own `renderScoreBytes`, so the pre-pass the item
    above added is itself under test. The def is Faust **source** on purpose:
    a box or signal tree is read by our own interpreters, which are the same
    program on both targets, so a tree would compare the interpreter against
    itself; source is where the two Faust backends are all that differ.

    **And they differ by more than a libm does**, which is the reduction this
    step found: the UGen scene lands at 1.5e-8, and the Faust scene at
    **7.2e-5**, growing through the take (fifths 2.3e-6, 5.6e-6, 2.4e-5,
    4.8e-5, 7.2e-5). LLVM and `-lang wasm-e` round a resonant filter's
    coefficients differently and the recursion turns that into phase drift. So
    the Faust bar is 1e-4, measured rather than chosen, and the page prints the
    per-fifth profile beside the maximum — smooth growth is drift, a step is a
    wrong constant or a mis-read control, and a bar alone could not tell them
    apart. Written on `clients/web/docs/src/platform.md`.

    The harness moved with it: the verdict is **beaconed and read out of the
    access log** rather than dumped from the DOM, because the Faust scene
    compiles in a Worker and Chrome's virtual time does not wait for one. That
    is the posture every other page in the web suite already reports with.
  - ✅ **A page mounting a SynthDef-only bundle loads no compiler asset**
    *(2026-08-26)*. It was true by construction (the glue is imported on the
    first `/def_send faust`) and is now asserted where the fetch actually
    happens: the import is in the NRT worker, whose requests are in no window's
    resource timeline, so `clients/web/test.sh` reads them out of the HTTP
    access log instead — `components.html` mounts the two SynthDef-only bundles
    and must **not** fetch `vendor/faust/libfaust`, and `faust.html` must fetch
    it. The pair is the point: a negative alone would pass just as well if the
    asset were renamed.
  - ✅ **`soundfile`** in a Faust def *(2026-08-26)*. The wasm backend does
    support soundfiles — it moves each `Soundfile*` to the front of the DSP
    struct and forces external memory, which is the `wasm-e` the page already
    compiles with — and what was missing was our half: `FaustSynth::new`
    ignored the buffer pool there, so the field stayed null and a def that
    declared one was silent.

    The struct is now filled the same way on both backends, from one piece of
    code: `SoundfileData` moved out of `faust::synth` into `faust::soundfile`,
    since a soundfile that read a buffer differently in a tab would be the same
    file sounding two ways. What differs is only how the pointer is delivered —
    libfaust asks for it through the UI glue's `addSoundfile` natively, and in a
    page the module's JSON declares the field's byte offset and the host stores
    the address itself (`json_ui`'s `DefLayout`, which also keeps a soundfile
    *out* of the control order, so `/node_set` by name is unaffected).

    Two things it corrected along the way. `ffi::Soundfile`'s `fIsDouble` was a
    `bool`, and Faust declares it `genInt32Typed()` on **every** backend: a
    one-byte field read as a word left three bytes of the read past the end of
    the struct we allocate, natively included. And the soundfile fields are
    filled **before** `init`, because `instanceInit` reads a file's sample rate
    out of them.

    `tests/faust.html` runs the def `tests/golden.rs` uses natively — its first
    output is the soundfile's length — and reads back a 300-frame buffer at
    0.50: 1024 would be the empty placeholder, and 0 would be an unfilled
    field.
  - ✅ **The page keeps one lib context for its life and never destroys it**
    *(2026-08-26)* — measured, and the measurement found a defect and closed
    it. `clients/web/tests/faust-arena.html` compiles distinct defs in a loop
    and reports the compiler's own linear memory; it is a **diagnostic, not an
    acceptance** (absent from the page list the way `tests/wheel.html` is), and
    the Worker's `faust-heap` request is its one caller.

    **The arena was never the cost**: 24 distinct defs leave it exactly where
    the first one left it, at 26 MiB. **The call stack was.** A wasm frame sits
    on the JavaScript engine's stack — about a megabyte, against a native
    thread's eight, and nothing to do with the `STACK_SIZE=8MB` the compiler is
    linked with — and libfaust recurses over the term graph of everything
    compiled so far, so a page compiling distinct **recursive signal** defs ran
    out at roughly every other def and answered `stack overflow` on a def that
    was perfectly well formed. Eight of the same defs compile in one context
    natively, which is what said the interpreter was not at fault.

    The fix is a **fresh compiler**: a def that exhausts the stack is compiled
    again in a new Emscripten instance and succeeds. It does not contradict the
    rule above — what poisons a page is a context that was *destroyed*, not a
    second one that exists — and it costs one instantiation, with the fetch in
    cache and the module already compiled: twelve such defs in a row, six
    replacements between them, average 18 ms each against the 9 ms a def that
    never overflows costs. Written up in `docs/decisions.md` ("The page's
    Faust is a second wasm module linked into the engine's own memory", its
    paragraph "A third silence") and on `clients/web/docs/src/platform.md`, which no longer
    carries a reduction here because there is none left to carry.

- ✅ **B6 — the second scope: a Worker for the work that is neither audio nor UI** *(done 2026-08-25)*.
  The browser engine collapsed four kinds of native thread onto one, and the bill
  is coming due: a buffer install is a memcpy on the thread that owes a block
  every 2.67 ms. The design is decided and recorded in `docs/decisions.md` ("The
  browser engine gets a second scope: a Worker for the work that is neither audio
  nor UI"); this is the build. **B5 depends on it** for where the Faust compiler
  runs, and `DiskIn` in a page depends on it entirely.

  Four steps, each shippable on its own and in this order — the first is the one
  that pays immediately and the rest are capability:

  1. ✅ **`step()` gets a budget** *(done 2026-08-25)* — `ServeBudget`
     (`ring_packets`, `nrt_jobs`), **counted rather than timed** because
     `Instant` panics on `wasm32-unknown-unknown`, which also makes it
     assertable. `NrtRunner::Inline` stopped running a job inside `submit` and
     now queues it for a budgeted `pump`, which is both what makes a ceiling
     possible and the seam step 2 replaces. The bound server keeps
     `ServeBudget::UNLIMITED` — it has a thread of its own. Four tests in
     `tests/headless.rs`, and `examples/measure_turn.rs` is the measurement.

     **What it measured** (native release, 2.67 ms quantum at 48 kHz):
     a burst of 64 allocations unbudgeted is **2.2 ms — 83% of the quantum, in
     one turn**; at four jobs a turn it is 0.20 ms over sixteen turns. That is
     the step paying for itself. But the same run says the budget is **not
     enough on its own**: one `buffer_load` of a one-minute stereo take is
     7.1 ms (2.7x the quantum) and a five-minute take is **36.7 ms, 13.8x** —
     one job, indivisible, and no count bounds it. So step 2 does not merely
     move that copy off the thread, it **must chunk it**, and the reduction to
     record if chunking still misses is a ceiling on how much one install
     copies per turn.

     The example also documents the trap it fell into: `/buffer_alloc` alone
     measures as free (microseconds for 110 MB) because Linux hands out lazily
     zeroed pages and the price resurfaces as page faults on the audio thread.
     A native number bounds the browser's from below — it can say an operation
     cannot fit, never that one does.
  2. The install, then the Worker — the measurement split this in two, and the
     first half turned out to be the one that mattered.

     a. ✅ **A long take is loaded in chunks** *(done 2026-08-25)* — a staged
        load on the pulled server: `buffer_load_begin` allocates and returns a
        ticket, `buffer_load_chunk` copies one run, `buffer_load_end` installs
        with one pointer swap, `buffer_load_cancel` discards. **Nothing is
        visible until `end`**, so the engine never reads a half-written take —
        the same "a job replaces the buffer wholesale" rule the async path
        follows. The host paces it and `ServeBudget::install_frames` (4800, a
        tenth of a second of stereo) is the number to size the run from.
        Four tests in `tests/headless.rs`, one of them asserting the staged
        buffer is sample-for-sample the one-shot buffer.

        **What it measured**: a one-minute stereo take went from 7.4 ms (2.8x
        the quantum) to a **worst chunk of 0.040 ms**, and a five-minute take
        from 34.4 ms (12.9x) to **0.127 ms**; `end` itself is a microsecond,
        which is what a pointer swap should cost. Both numbers the step existed
        to close are closed.

        **And the reduction it found.** A thirty-minute stereo take (659 MB) has
        a worst chunk of **45 ms** — the destination being faulted in during the
        copy, not the copy itself — and it is already past iOS Safari's ~350 MB
        ceiling. So the browser gets a **buffer-size ceiling** rather than a
        best effort, written on `clients/web/docs/src/platform.md` when the
        in-page number replaces this native one. Material that long streams; it
        does not pool.

     b. ✅ **The Worker, and the NRT runner in it** *(done 2026-08-25)* — and
        with it **step 3**, because the two turned out to be one thing: a
        Worker with no filesystem to read has nothing to do, and OPFS is
        reachable from nowhere else.

        `NrtRunner::Delegating` is the mode: a job the host does better leaves
        through `take_delegated` and comes back through `finish_delegated`, and
        **it blocks the queue behind it**, exactly as it would on the single NRT
        thread — one queue means submission order, and a `/buffer_free` that
        overtook the read it follows would be a different program.
        `crates/clausters-nrt-web` binds the decoder alone (no engine, no def
        families, no ring), `src/engine/nrt-worker.ts` is the thread, and
        `src/engine/opfs.ts` is the page's filesystem. The channel is a
        `MessagePort` transferred into the worklet, **feature-detected with a
        handshake** and falling back to a main-thread relay: a slower tab is a
        limit, a tab whose reads never complete is a defect. Results land
        through (a)'s staged load, so the copy is already paced.

        **`Buffer.read(path)` works in a tab**, out of the page's own storage,
        decoded by *our* decoder — `tests/nrt.html` asserts the samples are
        bit-for-bit the file's, that a missing file fails the command rather
        than hanging it, and that `readChannels([1])` comes back mono at the
        right channel's value. `read_audio_bytes` is the door, the same answer
        as the path door over WAV float, WAV int16 and a probed container.

        **Three things it broke and one it predicted.** The worklet's shim
        carried a `TextDecoder` and a note saying no string ever went *into*
        wasm; the delegation door made that false, and the failure is not a
        message about an encoder — the glue's top level throws, the module never
        registers, and the browser says the processor name is undefined. Both
        shims now stay whether or not today's surface uses them. `i64` crosses
        to JS as `BigInt`, so the decoder's frame count takes a double. And a
        guard written for the delegating runner reached the inline one and
        **stranded every `/buffer_allocRead`** — caught by an existing test, not
        a new one, so the missing case (a read with no host to take it) is a
        test now.

        What did not leave, and is written where a reader meets it
        (`clients/web/docs/src/platform.md`): `/buffer_read` overlays the
        buffer's current contents, so delegating it would ship megabytes out and
        back; `/buffer_write` has nowhere to write yet.

     c. ✅ **`/buffer_write` leaves too** *(done 2026-08-26)* — one payload out
        and none back, handed over a run per serving turn and written whole at
        the last one. What it took, and the two-lists bug it turned up, are with
        the entry under "Found by use" ("`/buffer_write` fails in a page, and
        not for the reason the plan gives"). `Alloc` deliberately stays: the
        buffer lives in the engine's linear memory, so a Worker allocating in
        *its* memory would only add a copy back.
  3. ✅ **OPFS: the page gets a filesystem** *(done 2026-08-25, with step 2b —
     the two turned out to be one thing)*. `createSyncAccessHandle` is
     dedicated-worker-only by standard, so it lives exactly where step 2 put the
     runner (`src/engine/opfs.ts`). `/buffer_allocRead` stopped being a path
     with nothing behind it. Decoding is **symphonia in the Worker, never
     `decodeAudioData`** — a different decoder would make one file two sets of
     samples across the two clients.
  4. ✅ **`DiskIn`/`DiskOut` in a tab** *(done 2026-08-25)* — `src/dsp/disk_web.rs`
     is the same two UGens with the thread taken out and the **host** put in its
     place: the ring stays, `process` is unchanged, and an underrun is silence
     exactly as a slow disk gives. `disk_poll`/`disk_push`/`disk_pull` are the
     whole interface between the graph and whatever reads files, and the worklet
     walks it after each block.

     **A stream is born shapeless.** Natively `DiskIn::open` opens the file and
     learns its channel count on the spot; here reading is asynchronous and
     belongs to another thread, so the stream reports `channels: 0`, plays
     silence, and is told once the Worker has looked. That shape was chosen over
     a "declare this file first" call **because the other client has no
     counterpart for one** — the surface a script writes against stays the same
     in both.

     The WAV framing (`wav_header`, `encode_wav_frames`) is in the server crate
     and bound to the Worker rather than written in TypeScript, for the reason
     the decoder was: a second conversion is a second answer, and int16 rounded
     a hair differently in a tab is exactly the kind of divergence nothing
     names. `src/engine/wav.ts` parses chunk offsets and re-emits a canonical
     header — pure framing, no sample math.

     Acceptance is `tests/disk.html`: a tone recorded into the page's own
     filesystem while it plays (1.42 s, 272428 bytes of RIFF) and streamed back
     out of it at **peak 0.500**, the recorded amplitude exactly.

     **Two reductions, both on `clients/web/docs/src/platform.md`.** A tab
     streams **WAV only** — a span of a compressed file is not a file, and
     decoding one whole is what a buffer is for. And **a stream starts after a
     longer lead**: without shared memory a span is *moved* across a port, so
     how far ahead the Worker reads is the design rather than a tuning
     constant.

  **Acceptance, and what it came to.** A long take fills without a dropout
  (worst chunk 0.127 ms against a 2.67 ms quantum, from 34.4 ms); a file in the
  page's storage reaches a buffer through `/buffer_allocRead` with its samples
  bit-for-bit the native read's (`tests/nrt.html`, worst delta 0); `DiskIn`
  streams a take out of that same storage (`tests/disk.html`). **The wire never
  changed** — `/buffer_*` and `/def_send` already reply late, which is why none
  of this was a protocol question.

  **Every step is gated on a measurement, and the measurement may cut it back.**
  The budget in step 1 is a number to be *measured against the quantum*, not
  chosen, and the three steps after it each have one: how much of a chunked
  install fits in a turn, what an OPFS read costs a Worker, how deep a prefetch
  has to be before an underrun stops showing. If a number does not hold, **the
  browser gets the reduced capability, not the full one with a glitch in it** —
  a ceiling on buffer size, a floor under the prefetch, a `DiskIn` that only
  plays forward, a `/buffer_write` that does not exist. A tab doing less than a
  window is a limit, and this project has a place for limits; a tab dropping
  samples is a defect, and it has none. So a step that measures badly is closed
  by **writing the reduction into `clients/web/docs/src/platform.md`** with the
  number that caused it, not by leaving the step open.

  **What it does not do**, stated because the shape invites the assumption: it
  does not bring parallel groups. Those need shared memory, not a second thread.

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

## J track — third-party plugins: a `plugin` def family, out of process

Section added 2026-10-13, at the user's request. The earlier "dynamic plugins not for now" decision was about **Rust UGen plugins**, whose ABI is not stable. Third-party plugins use defined binary interfaces: CLAP's C ABI and VST3's COM-style interfaces, with compatibility checked at load and no Rust types crossing the boundary. CLAP is MIT-licensed and the VST3 SDK has been MIT since 3.8. The revised decision is in `docs/decisions.md`, "Third-party plugins run out of process". **The milestone checkboxes below describe the shipped hosting capabilities and the remaining work.**

**The scope decided with the user:** instruments and effects, CLAP then VST3, Linux first, native plugin editors and server-side delay compensation. Both clients bind the same Rust implementation and each public capability lands in both clients with its documentation and tests.

- **One host process per instance.** `clausters-plugin-host` loads one plugin instance; its main thread owns lifecycle and windows, its audio thread processes samples. A crash silences only that instance. A `PluginDef` holds a reference and initial configuration, not a shared live process. A polyphonic instrument is one persistent instance receiving notes, not one instance per note.
- **A measured pipeline over shared memory.** The server's callback never runs third-party code, allocates, frees, locks, waits or makes syscalls. In particular it does not call `FUTEX_WAKE`; futex waits and wakes may belong to helper or non-RT threads only. J1 measures helper polling and non-RT notification designs before selecting the wake mechanism and the configurable buffering depth. Neither one block nor four blocks is promised. `SynthNode::latency()` reports the chosen bridge delay plus the plugin's declared intrinsic latency. `assert_no_alloc` checks allocation and freeing only; syscall/lock inspection and deadline measurements are separate acceptance checks.
- **Slices become one indexed job.** `PluginSynth` stages every `ProcessCtx.offset`/`frames` slice locally and publishes only once its engine block is complete. The job carries its valid spans, events and transport snapshots at their original offsets, so a split, pause, locate or loop never turns into several submissions of one block. There are bounded SPSC queues per instance, with one producer per queue even when nodes run on parallel workers.
- **An underrun is not a new timeline.** Missing output is silence at its scheduled sample, with a reported deadline counter; late output is discarded, never shifted to a later sample. Jobs and output carry a sequence and generation. J1 must settle the bounded backlog and overflow rules, including how input jobs, note-offs, automation and transport advance after a miss. An overflow or unrecoverable backlog must enter an explicit failed/silent state rather than lose a release invisibly or race a slot the helper still owns.
- **Recovery names a running node.** An explicit asynchronous restart operation creates a fresh helper and segment generation outside RT, restores the last saved state and current writable parameters, clears outstanding note ownership and swaps the prepared bridge at a block boundary. Old generations cannot deliver samples or events. A new `/def_send` changes the recipe for future instances; it does not reconnect existing nodes. Restart does not promise the recovery of unsaved DSP history or held notes.
- **Offline uses the same protocol.** NRT may wait outside the RT path and retains the configured bridge delay. RT/NRT sample equality is an acceptance claim for the deterministic fixture with identical rate, state, event order and transport, no GUI edits and no missed deadlines, not a promise about every plugin.
- **The web client and the wasm engine are different endpoints.** A browser client connected to a native server controls its plugins, including opening an editor on the server's machine. An in-page wasm engine cannot load native plugins and replies with a capability error. WAM2 is an open design question, not an adopted replacement or a second implementation of the same plugin.

- ✅ **J0 — The decision and the track** *(done 2026-10-13)*. Record the isolated hosting decision in the developer docs and this plan, with measured bridge feasibility first and instruments, transport and compensation inside the agreed scope. Documents only; `ROADMAP.md` names pending work rather than recording this completed step.
- ✅ **J1 — Bridge feasibility and the processing contract** *(done 2026-10-13; local feasibility experiment; processing contract and measurement summary in `docs/architecture.md`)*. Build a reproducible prototype and in-repo CLAP fixtures for gain, known delay, instrument events and failure. Compare helper polling with notification from non-RT threads while the callback only accesses preallocated memory and atomics. Measure deadlines, callback cost, helper CPU usage and buffering depths under a declared workload, CPU contention, multiple instances and parallel workers; include split blocks, stops, jumps, queue overflow, crash and hang. Record the tested operating envelope, select the wake mechanism and default depth, and define capacities, slot ownership, publication, memory ordering, generations and backlog/recovery semantics before J2. There is no universal minimum depth assumed for polling and no RT syscall exception in this track. A failed measurement leaves feasibility open rather than weakening the callback rule.
- ✅ **J2 — The host process and the segment.** *(done 2026-10-13)* Implement the J1 contract in a versioned `clausters_core::plugin_shm` layout and `crates/clausters-plugin-host`, one helper and queues per instance. Use a common format interface from the first CLAP adapter (`clack-host`): scan, load, lifecycle, ports, process, events, parameters, state, latency and editor capabilities. Separate administrative control from the audio plane; discovery inspects plugins in disposable helpers with timeout/crash reporting, not in the server. `/plugin_scan [dirs]` returns stable identities, ports and parameters and caches metadata in `<data_dir>/plugins.json`. Test layout/handshake mismatches, capacities and helper cleanup against repository fixtures; scanners and instance loading both validate compatibility.
- ✅ **J3 — The `plugin` family and recovery.** *(done 2026-10-13)* Extend the opt-in `plugin` infrastructure and independently selectable native format adapters to the def family; it combines freely with `synth`/`faust` and remains opt-in for this track. `/def_send plugin <name> <reference-json>` is asynchronous and joins `/server_sync`; extend `DefKind`, name claims, translator lookup, GraphDef members, queries, freeing and persistence. The reference identifies format, binary path and native plugin id (a binary may expose several plugins), port configuration and optional initial state. `PluginSynth: SynthNode` exposes stable native parameter identities and `ControlInfo`, mappings and main/sidechain/additional buses through the shared bus analysis; display names and discovery order are not parameter identities. Finalize parameter value units and port addressing in the protocol reference before implementing the builders. Persist references/state, never binaries, and preserve unavailable records unless `--prune-defs` is explicitly requested. Add per-node failure notifications and the explicit restart operation described above; live instances survive a def being freed or replaced. Both clients, the reference and RT/capacity tests land with each public verb.
- ✅ **J4 — Persistent instruments and the governing transport.** *(done 2026-10-13)* Deliver notes, MIDI and per-note expression to persistent plugin nodes from live bindings, timed bundles and event lanes, using the existing shared MIDI decoder. Preserve note identities across sources and define release on stop, jump, lane replacement and free; never route the existing synth-per-note constructor into a whole plugin instance per note. Carry device time and the node's governing transport, position, playing state, tempo and discontinuities with sample offsets, respecting both governed freezes and live ungoverned processing. Unsupported event dialects are reported rather than silently dropping expression. Both clients and paired examples use the same API and event sequence. Acceptance covers overlapping notes, controllers, expression, releases, slices and transport gestures without new helpers per note.
- ✅ **J5 — Offline and conditional determinism.** *(done 2026-10-14)* Drive the same helper synchronously from `render()` outside RT with the configured bridge latency, all ports and the same event/transport contract. A deterministic fixture with identical inputs and no RT deadline loss must yield identical samples in RT and NRT, including split blocks. NRT failure aborts with the responsible node and event time; an arbitrary third-party plugin is not claimed deterministic.
- ✅ **J6 — State.** *(done 2026-10-14)* `/plugin_saveState <node>` returns the opaque state and `/plugin_loadState <node> <blob>` restores it asynchronously outside RT, coordinated with the helper's lifecycle and processing. State operations participate in the async barrier and refresh changed parameter/port/latency metadata safely. A def may carry initial state, while a live node's state is its own; capture/load and recovery are tested on two independent instances. Both clients and the reference land together.
  **The live contract:** a load quiesces at a block boundary and installs a fresh generation with the image loaded before activation; a capture asks the running helper and swaps nothing *(revised 2026-10-14 by the J1-J9 audit; it first quiesced and swapped too)*. Preparation is silent and clears held notes/events; routing/maps survive, capture/restart keep current controls, and load takes the image's unmapped parameter values. State and compatible metadata belong to the node, never the shared def. Live audio port IDs/order/channel counts and control IDs remain stable; incompatible images fail and explicit restart uses the last successful image. Opaque captures, including empty images, use bounded 6144-byte reply fragments; clients reject missing/duplicate fragments. The trade-off is recorded in `docs/decisions.md`, "Loading a plugin state uses a fresh processing generation; saving does not".
- ✅ **J7 — VST3.** *(done 2026-10-14)* Implement the existing format interface over `vst3`'s COM-style bindings without changing the family or inventing a second client surface. Cover lifecycle, parameter conversion, audio/event ports, state, transport, latency changes and editor callbacks against a repository VST3 fixture. Validate feature combinations with either adapter alone or both, with each existing def-family combination and without `embed`.
  **Parameter domain:** VST3 controls, queries and maps use its processor-native normalized `[0, 1]` values and `1 / stepCount` quantization; CLAP keeps plain units. Controller presentation conversion stays on the helper main thread and never enters audio. Component/controller state share one opaque image; callback edits use the existing parameter/gesture result path. Linux modules stay mapped through helper exit for registered TLS destructors. Native editor attachment belongs to J9.

- ✅ **J8 — Server delay compensation, anticipated by M12 and M28.** *(done 2026-10-14)* Implement the server's compensation work anticipated by **M12 — Canonical graph form via bus connections** and deferred by **M28 — Partitioned convolution**, which added `SynthNode::latency()`. This is shared engine infrastructure, not a plugin-only delay: use bridge plus intrinsic latency to align statically analyzable paths admitted by automatic ordering, across nested groups and their boundaries. Build the global dependency/latency plan and delay storage off RT and publish at block boundaries. Delay distinct writer contributions before bus summation and align main/sidechain inputs before processing; preserve `Out`/`ReplaceOut` semantics and parallel execution. Recompute safely for topology and declared latency changes; report uncompensated manual, dynamic and feedback boundaries, never compensate authored musical delays. Acceptance: impulses through dry/processed paths, multiple writers, chains, sidechains, nested groups, parallel workers, changing latency and the same graph offline. This answers the server-side latency part of **"`mt.fx` is a declared empty slot, and what an effect *is* as a document is unspecified"** under Future directions; its document/preset/bypass/tail questions remain there.
- ✅ **J9 — The plugin's own window and packaging.** *(done 2026-10-14: the paired Python examples and pages heard and seen with a CLAP effect and a VST3 instrument after the J1-J9 audit fixes; feature matrix clean)* The helper's main thread opens/closes/resizes a floating native editor, initially X11/XWayland, and runs the required GUI, timer and fd callbacks; plugin edits update the same parameter queries and notifications. The window is on the native server's machine, including when a web client requests it. Cover reopen, free, crash and processing while the editor is open. Package the matching helper with server, embed and Python native artifacts; validate the version handshake. Run paired Python/TypeScript examples by ear and eye with CLAP and VST3, refresh Python's bundled binaries first, and close with the feature matrix, client parity, pyright, web build/tests and all three doc builds.

- ✅ **J10 — The plugin catalog: a plugin is an object, not a scan row.** *(added 2026-10-14 at the user's request; done 2026-10-15: the paired examples list and find their plugin by name, heard and seen with a CLAP effect and a VST3 instrument from Python and from the pages)* Today every program filters raw scan rows, copies format, binary path and native id by hand (the id as a constant), and assigns a bus to every audio port; the port rule is only checked when the server rejects the def. The information is organized in three layers: the **catalog** (what is installed: server-owned and cached) → a **plugin** (one read-only entry: identity, display, ports, parameters, capabilities, categories; its identity travels inside it and is never typed) → a **`PluginDef`** (the user's decision: routing and initial state, the only thing stored, and what a `Synth` instantiates).
  - **One implementation, in Rust.** The types (`PluginInfo` with its ports and parameters, `PluginReference`) move to `clausters-core` as plain data, bound through FFI and wasm; the plugin host re-exports them. The core holds, once: parsing the catalog from the reply rows (what each client reassembles today), search by name, vendor, format, kind and tags (none or several matches is an error naming the candidates), the port rule the server's probe checks (one function, used by the server and by the client before sending), and the default routing (consecutive buses from a first bus per side, main output at bus 0).
  - **Everything the formats declare is read in discovery**: CLAP's descriptor features and description, audio-port flags and types, a note port's preferred dialect; VST3's class subcategories and SDK version, main/auxiliary bus types and default activation, event-bus channel counts. The raw lists are kept, and Rust derives one vocabulary: `kind` (instrument, effect, note effect, analyzer, other — from the declared category, else from the ports), `tags` (synth, sampler, piano, drum, reverb, delay, eq, dynamics, distortion, modulation, filter, pitch, spatial, utility, mastering, restoration, ...) and `channels` (mono, stereo, surround, ambisonic). External data a plugin loads (an impulse response, samples) has no standard declaration in either format and stays inside the opaque state.
  - **Controls read as the ports do**: the main audio input and output are `in` and `out`, other ports a sanitized display name, parameters their name; the native-id spellings (`in:<id>`, `param:<id>`) stay as the unambiguous form. The collision rule is decided in the milestone.
  - **`/plugin_scan` is incremental**: the cache (`plugins.json`, today written and never read) is read, each binary compared by canonical path, modification time and size, and only changed ones inspected; an explicit flag forces a full rescan. The reply protocol does not change.
  - **Client surface, the same in both**: `server.plugins(*dirs)` returns a `PluginCatalog` (`find`, `filter`, `instruments()`, `effects()`, `errors`) of `Plugin` entries; `PluginDef(name, plugin, inputs=, outputs=, state=)` builds the reference through the core. The dict form stays for stored defs and `scan_plugins` as the low-level call; `plugin_event` defaults its port to the def's main note input.
  - **Acceptance**: the fixtures' catalog parsed identically by both clients (parity vectors); a port mismatch refused before sending; an unchanged binary not inspected again (counted); both formats' categories read from the fixtures. The paired examples are rewritten on the catalog — no native id and no bus arithmetic in any example — and heard and seen again.
- ✅ **J11 — Notes are routed like audio: event buses, one note destination and the `Voices` node.** *(added 2026-10-14 at the user's request; done 2026-10-15: `basics/voices` heard with the built-in default from MIDI, a pattern, a held `pvoice` line and the client, and the persistent plugin pair heard with a VST3 instrument played by its lane, live notes and a pattern on one instance)* Notes reach a plugin by three paths — live MIDI bindings, event lanes and the client's `plugin_event` — and each decides on its own what kind of instrument receives them (a `NodeDef::Plugin` match per entry point in `src/osc/translate/midi.rs`). A live binding can only create an instance of its own, never play an existing node, and the client's `Event` builds `/synth_new` itself, so a pattern cannot play a plugin and an example has to step outside the event infrastructure. The target is scsynth's simplicity: three bus kinds, and every node reads and writes the buses its controls name.
  - **Event buses**, numbered like audio and control buses. One carries normalized notes and channel messages (MIDI 2 resolution, 16 channels; each event with its source and sample offset) in a fixed-capacity per-block buffer on the audio thread. Writers: the system MIDI input (to a configurable bus), event lanes, and client events (a new OSC verb). Readers: nodes with a `notes` control, set at `/synth_new` and by `/node_set` as `in` is. Sources stay distinct, so per-source release (transport stop, lane replacement, free) keeps working, and MPE zones and MIDI 2 per-note messages work over a bus's channels. The existing decoder and mapping tables are reused, not duplicated.
  - **Every instrument is a persistent node: a note never creates a node.** A plugin instance reads its bus and forwards each event to its note port in the port's dialect (native, MIDI, MPE expression); today's per-dialect paths become that reader. A new **`Voices`** node holds N voices of one SynthDef or FaustDef, allocated when the node is built, and assigns them on the audio thread: a note takes a free voice, resets and starts it; a voice that reaches its freeing done action returns to the pool instead of freeing the node; a silent voice costs nothing; a fixed steal policy applies when all are busy. Shared controls (`set` on the node, its `in`/`out`) reach every voice; per-note controls come from the note through the mapping table that is today `MidiBinding`'s (velocity, bend, pressure, timbre, CCs), which becomes the node's configuration. In the tree it is one node like any other. The def formats and their compilation do not change; what is added is resetting a voice's state on its own memory — Faust's `instanceClear`, and for SynthDefs a reset in every UGen, of which some exist (demand rate, `src/synthdef/instance.rs`).
  - **`/midi_bind` becomes routing**: an input writes a bus and whatever reads it plays, an existing node included. `/synth_new` per note stays for whoever wants it, as in scsynth; the event path does not use it, and `Synth` is unchanged.
  - **The client's `Event` addresses a destination**: `pbind(instrument=node, ...)` with a plugin or `Voices` node writes its notes there, with note ids, ports and dialects resolved by the core under the rule the lanes use; a def name keeps today's synth per note.
  - **"Voice" means one thing.** Today the word names a monophonic line (`pvoice`: one node continued by its joined notes), a notation key (`voice`, the staff's voice), a lane note's destination (`EventLaneVoice`: def, graph slot or plugin), a graph's per-note slot (`slot="voice"`) and the nodes live MIDI spawns. Settle the vocabulary on the field's — a *voice* of an instrument is one sounding allocation; a line of a staff stays the notation's voice — and rename where it means something else (`EventLaneVoice` names a destination). A `pvoice` line on a `Voices` node or a plugin is one held voice (one note id, its joined notes setting its controls, or overlapping on MIDI) rather than one node. A sequence rendered from a `pvoice` keeps its joins only as ties and slurs, so a lane plays each joined note as a synth of its own and the legato is lost in the server's timeline: the lane note gains the join, so a line sounds the same from a pattern and from a sequence.
  - **Acceptance**: no node created per note on the event path (counted), under `assert_no_alloc`; a keyboard, a lane and a pattern playing one `Voices` node and one plugin at once, with per-source release; RT/NRT equality for `Voices`; voice reset tested over every UGen and both families; the `pvoice` legato equal from a pattern and from a lane. Examples: a pattern playing a plugin through `Event`, and a `Voices` instrument played from MIDI and from a pattern, heard. Both clients and the references land together.
- ✅ **J12 — A node's generic editor: `edit(node)`, drawn by the host.** *(added 2026-10-15 at the user's request)* A plugin's own editor opens a native window on the server's machine, from a helper thread, and cannot reach a page. With the catalog's data the host can draw an editor of its own for any node, as other hosts' "generic editor" does: it opens in the native host and in a page alike, and starts no GUI thread on the server. **The two are named apart**: the plugin's own window keeps `open_plugin_editor()`/`close_plugin_editor()` ("plugin editor" names the plugin's window), and the generic one is `edit(node)`, the existing verb that opens "an editor of its own kind" for a buffer, a curve, a sequence, a timeline, a multitrack or a symbolic score — a node joins that list. No `node.gui()`: `Session.gui()` already launches the host and a bundle's `gui(tree)` sets its GuiDef.
  - **One generator, for every node.** A node's controls become a GuiDef, by one rule in Rust shared by both clients and the host: a continuous control a slider or knob, a stepped one a list or knob with detents, a two-valued one a toggle, a read-only one a display; grouped by module; ranges, steps and defaults from the control's description. Plugins are the richest case (modules, flags, value text), not a separate design: a `Synth` of a SynthDef or FaustDef gets the same editor from its own controls.
  - **A control's description, per family.** Plugins already report `min`, `max` and `step` in `/def_query` and carry module and flags in the catalog, and a Faust control reports its UI range. A SynthDef control has none (`ControlSpec`: name, default, rate, lag): it gains optional `min`, `max`, `step`, warp and unit, and without them the range comes from conventional names (frequency, amplitude, pan, ...), else from one generic rule around the default. Both clients' def builders can write them.
  - **Writing** uses what exists: each widget is bound with `/gui_bind id "server" "/node_set" <node> <control>`, straight to the server, never through the script.
  - **Reading back is missing and is added.** `/gui_bind` runs one way. A control that changes outside the widget — the plugin's own editor, a preset or a state load (`/plugin_edit`), another client's `/node_set`, a map — must move the widget without the script relaying it: a server-to-widget direction of the binding, general to any node control, not a plugin special case. Gestures (`begin`/`end` from `/plugin_edit`) are honored so a dragged widget is not overwritten mid-drag.
  - **Value text.** A plugin converts values to display text and back (CLAP's parameter text conversion, VST3's string-by-value); a stepped parameter's labels ("Hall", "Room") are read at discovery and kept in the catalog, while continuous values are converted on demand by an administrative request on the control worker, never from audio. SynthDef and Faust controls show the value with their unit.
  - **Scale.** A plugin may declare thousands of parameters (one tested instrument declares 2295): the editor groups by module, leaves out hidden and read-only parameters where the format marks them (both do), and offers search and paging rather than one column of sliders.
  - **Closed** *(2026-10-16)*: checked by eye on a SynthDef (`editors/edit_node.py`: the sliders, the switch, the search) and on an installed VST3 instrument of 2295 parameters, its own window and the generic one following each other both ways; the page and the CLAP list fixture were captured headless (`tests/node-editor.html`, a capture of the list read back as "Plate").
  - **As built** *(2026-10-16)*: the description is `/def_query`'s per-control record, grown for every family (range, step, warp, unit, label, group, flags, a list's labels) and built by one walk per def family; the generator is `clausters_core::node_editor`, which both clients call through the core's door and open as an ordinary GuiDef, so the window is identical from both by construction rather than by vectors; reading back is the host asking with each binding's read form (`/synth_get`, and `/synth_getText` for the value text, a label bound to `/node_setText`) rather than a stream, and the widget a hand holds is left alone, which is what a drag needs whether the other edit comes from the plugin's window or anywhere else. `docs/decisions.md`, "A node's generic editor is a GuiDef, and a binding is read back by asking".
  - **Acceptance**: the generated GuiDef identical from both clients for the fixtures and for a SynthDef and a FaustDef (parity vectors); an edit in the plugin's own editor moving the generic widget, and a drag not overwritten; value text and list labels for both formats' fixtures; a large-parameter plugin opened usably. Examples: the paired plugin examples open `edit(node)` beside the native editor, and a SynthDef instrument opens its own, checked by eye in the native host and in a page. It follows J10 (catalog, control names) and does not depend on J11.

Further formats/platforms are checkbox entries under Future directions. Document inserts remain the existing `mt.fx` entry; this track provides hosting, state and engine compensation, not a separate application routing implementation.

## R track — The second review: the shape of the workspace (2026-11-11)

A review of the workspace as a whole, the GUI host and the editor application
included, fourteen weeks after the first. What it found is of another kind: the
files are in order and the *crates* are not. The dependency graph already has
levels and nothing states or checks them; the GUI host is a workspace of its
own that compiles the server anyway; `clausters-core` holds four things with
four audiences; a panic behind a binding takes the caller's process with it;
and the build directories take 371 GB.

The track's invariant holds for all of it: behaviour is preserved, no assertion
is edited (a test moved to another file is not an edit), and what touches the
audio thread closes with the bench.

**The work is done on a branch, `restructure`, and so is its plan.** These
entries live on the branch alone until it is accepted, so that `main` keeps no
trace of a restructuring that is not taken. The branch merges `main` at the
start of every sitting (a merge rather than a rebase, so a directory rename is
resolved once), and it is left green and mergeable at the end of each. CI runs
on a push to `main` and on a pull request only, so the branch is checked
through a draft pull request, and the two workflows that can be run by hand
(the weekly `stable` job and the release's `verify`) are run on it.

**When each one is done is part of the plan**, because `main` moves fast
exactly where these land (the GUI host and `clausters-apps` are the two most
edited directories of the tree):

- What stops drift goes first and collides with nothing: `R13`, `R14`.
- What moves files goes at a quiet point, in a sitting that does nothing else,
  and never right before a tag, since it changes paths the release workflow
  reads: `R15`, `R16`.
- `R16` goes before the `Q` track opens and before `X11` continues. The first
  adds a third notation crate and the second adds references to the module
  that moves, so every week makes it dearer.
- What orders a crate from inside is done one cycle per visit, with the
  milestone that already has that module open: `R19`.
- What rewrites records goes last, once the branch is accepted, since it
  edits every plan and would conflict with each merge: `R23`.
- `R18`, `R21` and `R22` depend on nothing and are taken between the others.

**What the review first left out was taken up when the rest was done**
*(2026-11-12, asked by the user)*. The review weighed a virtual workspace by
what it does for the build, which is nothing, and not by what the tree says
to whoever reads it: with the server as the root package the repository
reads as a server with things around it, and the server is one component
among several, which the applications are not even entered through. So the
workspace becomes virtual (`R24`) and the transport leaves
`src/server/engine.rs` with a bench of its own rather than waiting for `T3`
(`R25`). The other two (the applications' crate by application, and the
host's files past four thousand lines) are refactorings to evaluate once
these are in, each in its plan's Future directions: `crates/clausters-apps/PLAN.md`,
"The applications as modules that read apart", and
`crates/clausters-gui/PLAN.md`, "The host's longest files, cut where they
have a seam".

- ✅ **R13: The levels are a rule** *(done 2026-11-11)*. The crates already sit in levels and a
  dependency only ever points down, which is cargo's doing for ordinary
  dependencies and nobody's for dev-dependencies:

  | Level | Crates |
  |---|---|
  | 0 | `clausters-core`, `clausters-net` |
  | 1 | `clausters-document`, `clausters-midi`, `clausters-plugin-host`, `clausters-notation` |
  | 2 | `clausters-editing`, `clausters` (the server) |
  | 3 | `clausters-apps` |
  | 4 | `clausters-app-engine` |
  | 5 | `clausters-ffi`, `clausters-core-web`, `clausters-web`, `clausters-nrt-web`, `clausters-gui` |

  A test in the shape of `tests/bindings.rs` reads the manifests as text and
  fails on an edge that points up, dev-dependencies included, and on a crate
  the table does not name. Today's violations go in a list of exceptions that
  may only shrink: the server's dev-dependencies on `clausters-apps` (above
  it) and `clausters-editing` (beside it). The table is written once, in
  `docs/architecture.md`. **Acceptance:** adding an upward edge to any
  manifest turns the test red.

  Shipped as `tests/levels.rs`, which reads the table out of
  `docs/architecture.md` ("The crates and their levels") rather than keeping
  a copy, and takes an edge between two crates of one level as a violation
  too. Proved by mutation: a dev-dependency of the core on the document
  turned it red, and so did an exception that matched no manifest.

- ✅ **R14: Build profiles, and a pinned toolchain** *(done 2026-11-12)*. No manifest carries a
  `[profile]`. The dev profile takes line tables only, which is most of what
  shrinks a debug build; the release profile takes link-time optimization and
  a single codegen unit only if `crates/clausters-server/examples/bench.rs` justifies them, with both
  numbers in the commit. The toolchain is pinned in a `rust-toolchain.toml` at
  the root, which the editor application and the fuzzer inherit and the
  nightly wasm build (it names its own) does not; CI installs what the file
  says, and a weekly job runs fmt, clippy and the tests against `stable`, so
  that a new lint arrives as a commit that raises the pin rather than as a red
  tree nobody touched. The rule in `AGENTS.md` that CI pins nothing is
  rewritten with it. **Acceptance:** the weekly job exists and is seen to run;
  the full feature matrix is clean under the pin.

  *Written 2026-11-11, and open on what only the workflows can show.* The pin is
  `rust-toolchain.toml` (1.99.0, the version the audit harness already
  pinned), every workflow installs it through the `.github/actions/rust`
  composite, and the weekly job is `stable` in `audit.yml`. The dev profile
  takes line tables only (the tests of two crates: 605 MB to 370 MB). The
  release profile stays cargo's own: thin link-time optimization with one
  codegen unit moved the median throughput by +0.8% and 0.0% over two pairs
  of runs, inside the +2.9% two builds of the unchanged profile differed by,
  and made the build a third slower (`docs/decisions.md`, "The toolchain is
  pinned, and a new compiler arrives as a commit"). **What is left:** the
  workflows have not run since the change, so the composite, the weekly job
  and the matrix under the pin are unobserved until they run on the branch.

  *Observed 2026-11-12.* The release workflow was rehearsed against the
  branch at the end of the review and came back green on the commit that
  went into `main`: its gate (the tests and the whole feature matrix under
  the pin), the wheel and the server binary, the editor's wheel and the web
  package. Two earlier rehearsals were red, each on a path still written
  from the repository root after the workspace became virtual (the VST3
  fixture's builder, and the recipe that reads the Faust C API's export
  list for the browser's compiler), which nothing but the release workflow
  and CI's native plugin rows runs. The weekly job against the newest
  stable was run by hand on `main` and passed.

- ✅ **R15: The GUI host is a member of the workspace** *(done 2026-11-12)*. It was kept apart so
  that it could never entangle the server's build, and that stopped being
  true: `clausters-app-engine` is an unconditional native dependency of the
  host and links the server, so every host build compiles it. What the
  separation still costs is measured: two lockfiles that disagree on 26 of
  the 255 dependencies they share, every shared crate compiled twice (146 GB
  of the host's own `target/`), and a version written by a script and watched
  by a test. Two commits, in this order:

  - **The move.** `crates/clausters-gui` becomes `crates/clausters-gui`: it is a crate,
    and a special client that talks to the two language clients and can act
    alone, where `clients/` holds the Python and web packages. Some 356
    references move with it (CI, hooks, the build scripts of both clients,
    the Python launcher's path resolution, the audit scenarios, the rules and
    the skills); the frozen build log is left as it is. A guard beside `R13`'s
    test fails on any tracked file outside `docs/history/` that still names
    the old path, which is what catches a reference arriving from `main`.
  - **The membership.** The host's own `[workspace]` and lockfile go, it
    inherits the version, and its binary is built into the root `target/`.
    The two libfaust-free CI rows exclude it as they already exclude the wasm
    crates, since the host turns on `realtime` and `notation` and a
    `--workspace` build would unify them onto the server those rows test.
    `[workspace.dependencies]` takes what thirteen manifests repeat.

  The root package stays the server and the default member, so a plain
  `cargo build` or `cargo test` at the root compiles no window system.
  **Acceptance:** one lockfile; the Python launcher and the web build find
  the host where it is now built; the release workflow's rehearsal is green.

  Shipped as one commit rather than two: a crate under `crates/` must inherit
  the version (`tests/versions.rs`), which it can only do as a member. The
  merged lockfile keeps every version the server resolved; of the host's, the
  twelve general-purpose packages the two lockfiles disagreed on took the
  server's and the rest stay where the host's lockfile had them.
  `[workspace.dependencies]` holds the eleven dependencies more than one crate
  names. Checked: clippy over the workspace and over the host by itself, the
  host's 1,481 tests, both wasm gates, the isolated feature check of the
  applications, the Python suite (1,594) with the binaries rebuilt, the web
  package built and tested (1,431 and the pages), the audit harness's own
  tests and the three books. The release rehearsal is left for the end of the
  whole review, with `R14`'s.

- ✅ **R16: Notation is a crate of its own** *(done 2026-11-12)*. `clausters_core::notation` is
  16,752 of the core's 51 thousand lines, the server does not use it, and
  inside the core it reads only `ratio` and `event`. It also shares a name
  with its neighbours: the model is a module of the core, `clausters-notation`
  is the binding of the engraving library, and `clausters-notate` is a
  directory holding a plan. The binding becomes `clausters-verovio`; the
  model becomes the crate `clausters-notation`, at level 1, and takes the XML
  and pattern-matching dependencies out of the core; the `notation` feature
  of the five crates above it becomes an optional dependency on that crate.
  The paths are rewritten rather than re-exported from the core, which would
  be a second door onto one module. No C or wasm export changes, so
  `CORE_ABI_VERSION` does not move. **Acceptance:** the isolated feature
  check of `clausters-apps`, the feature matrix and both clients' suites
  pass; the core builds with no notation in it.

  Shipped as written. The document reads the notation model, so the levels
  moved by one: notation at 1, the document, the engraver's binding and the
  server at 2, and the rest one higher (the table in `docs/architecture.md`
  is the current one; `R13`'s above is as it was written). That put the
  server's dev-dependency on the document beside it rather than under it, so
  it joined the exceptions until `R17` removes all three. Checked: clippy
  over the workspace, the host by itself and both engraver features; the
  tests of the six crates the cut touches and of the host; both wasm gates;
  the isolated feature check; the Python suite with the binaries rebuilt,
  the web package built and tested, and the three books. The whole feature
  matrix is run once, at the end of the review.

- ✅ **R17: A test lives at its level, and `tests/` links few binaries** *(done 2026-11-12)*.
  Three of the server's integration tests exercise the applications
  (`multitrack_playback`, `audio_editor_graph`, `long_takes`) and belong to
  `clausters-app-engine`, the crate whose job is joining the two; with them
  go the server's upward dev-dependencies and the last of `R13`'s exceptions.
  Three more, and `R13`'s own, test no crate at all (`ascii`, `bindings`,
  `versions`) and move to a crate of repository checks, which no longer needs
  the server compiled to run. What remains of the 78 binaries `tests/` builds
  today, each linking the whole server, is grouped by def family into a
  handful, one module per present file. What depends on the state of its process
  stays a binary of its own: the tests that install a global allocator, the
  signal guard, the denormal mode, the named shared-memory segments.
  **Acceptance:** the same number of tests runs before and after under each
  feature set; link time and the size of `target/debug/deps` are quoted in
  the commit.

  Shipped narrower than written, on a reading of the files: of the 72 left
  after the six moves, 42 render offline and share nothing with their
  process, and those are two binaries now (`crates/clausters-server/tests/offline`, 38 modules, and
  `crates/clausters-server/tests/faust_offline`, four). The other 30 stay as they were, and the list
  is longer than the entry foresaw: beside the allocator, signal, denormal
  and shared-memory tests, every test that boots a live server on a socket,
  sleeps on the wall clock or runs a plugin's process. Cargo runs test
  binaries one after another and a binary's tests in parallel, so grouping
  those would have run servers side by side that never ran together, which is
  a change of behaviour. The shared helpers are declared once per binary. The
  server's tests count the same before and after under the three feature
  sets (958, 141 and 872), the levels' exceptions are empty, and the
  repository's checks are `crates/clausters-checks`, with no dependency at
  all. Where a new test goes is in the `audio-testing` skill.

- ✅ **R18: A panic at a binding's door is an answer** *(done 2026-11-12)*. None of the 248
  exports of the C ABI catches an unwind, and a panic crossing `extern "C"`
  aborts: one `unwrap` in any verb of any application takes the Python
  interpreter, and the unsaved session, with it. The doors that run verbs
  answer `{"error"}` instead, through one helper, which also recovers the
  handle's lock (poisoned today, after which every call answers nothing for
  good). A page cannot recover, since wasm aborts on a panic, so the wasm
  core installs the reporting hook the browser host already has, and the
  difference is written in `docs/bindings.md`. The native host logs through
  its diagnostic channel before it exits. In the same sitting, the
  application engine's playback tests stop waiting on the wall clock and
  advance the engine by blocks, which is what the intermittent failures filed
  under load point at. **Acceptance:** a verb that panics yields an error in
  both clients and the next call on the same handle answers; the playback
  tests pass fifty runs in a row under load.

  *The barrier shipped 2026-11-12, and the tests' pacing the same day.* The
  verb doors of the applications, the document, the runner and the
  notes playback answer `{"error": "internal: ..."}` on a panic, the calls
  through a handle answer their null-handle value, and every handle's lock is
  taken poisoned or not (`crates/clausters-ffi/src/out.rs`, with its tests);
  the wasm core reports a panic to the console and the native host logs one;
  `docs/bindings.md` ("What a panic becomes") says why the two differ. Not
  observed: a real verb panicking through a client, only the helper's tests.
  The sleep in the playback tests was not a wait for a result but the pacing
  of a race between the engine the test steps and the take's reader thread.
  The reader now counts the turns it ends with nothing to do, and
  `AppEngine::fill_readers` waits for one that began after the call, so a
  test gives the readers their turn after each block (or each burst, where a
  device's pace is staged) instead of sleeping; the readers stay threads, as
  they run in an application. The playback, multitrack and reading suites
  passed fifty runs in a row under eight busy threads. Two sleeps remain on
  purpose: the long take's acceptance, which measures the reader's lead in
  real time, and the polls of a render or a save landing, bounded by a
  deadline.

- ✅ **R19: The host's modules have levels too** *(done 2026-11-12)*. The host is 149 thousand
  lines in one crate, and measured by reference it is one strongly connected
  block: its five largest modules use one another, the drawing base is cyclic
  (`font`, `paint`, `metrics`, `theme`, `layout`), and the renderers at the
  crate's root reach back into the host. No crate can be cut from it as it
  stands. `R13`'s test is extended to the host's modules, in five layers
  (drawing base, views, tree and protocol, elements and gestures, fronts),
  with today's cycles as exceptions that may only shrink. Two parts are
  planned and the rest is opportunistic:

  - ✅ **R19a** breaks the base's cycles (the rectangle leaves `layout` for the
    base; `theme` and `metrics` stop reading `paint` and `font`) and groups
    the native shell, today spread among 73 flat modules, under one module.
  - ✅ **R19b** cuts the base into a crate once its list of cycles is empty.

  The layers above are ordered one cycle per visit, by whichever of the `P`
  and `D` tracks has the module open. **Acceptance:** the host's headless
  captures are identical before and after each part.

  *R19a done 2026-11-12.* The base is seven modules with no reference out
  of it and none among them that turns back: `rect` (the rectangle, out of
  `layout`), `theme` (which now holds the color type `paint` used to),
  `path` (an outline's SVG path, scanned, filled and flattened, out of the
  score's pictures, since the face draws outlines too), `paint` (the batch
  alone), `font`, `metrics` and `draw` (the context of batch and role
  tables, out of `paint`). One part of the entry did not hold as written:
  `metrics` still reads `font`, because the size roles that hold text are
  measured by the face, so the cycle was broken from the other side, by
  taking out of `paint` what read the size roles (the context) and what
  read the face (the GPU half, now `painter` at the crate's root, beside
  the other pipelines). Two tests of the text caret that reached the
  pictures moved to them. The native shell is `host::native`. The test is
  `crates/clausters-checks/tests/host_layers.rs` over the table in
  `docs/architecture.md` ("The host's modules and their layers"), and it
  starts with 55 exceptions above the base: most are references to the
  host's root (`Host`, `ClientId`, the addresses) from the layers below it.
  Seven examples captured headless by the host built before and after were
  the same bytes.

  *R19b done 2026-11-12.* The base is `crates/clausters-gui-draw`, level 0
  (it names no crate of the project), with the face's assets beside it. Its
  features are the host's, forwarded: `font-atlas`, and `path` under the
  host's `notation`. What the host's tests asked of a batch under its own
  `cfg(test)` (an extent, the colors, the alphas, how often a kept part was
  drawn again) is behind a `probe` feature the host turns on as a
  dev-dependency, since a crate's `cfg(test)` does not reach another's
  tests. The host's paths were rewritten to the crate, with no re-export,
  as notation's were. The layers table lost its base row, and the four left
  are numbered from 0. The same seven captures were the same bytes again.

- ✅ **R20: The two bindings have one shape, and a C type is checked** *(done 2026-11-12)*.
  `clausters-ffi` is 24 modules by subject and `clausters-core-web` exposes
  the same in one file of 3,718 lines; it takes the same modules, so a row of
  `docs/bindings.md` is read file against file (and `tests/bindings.rs` reads
  a directory). `R1` settled on checking arity alone for the ctypes leg,
  since mapping types textually would have stated them a third time. A
  header generated from the C ABI by the toolchain is not a third statement,
  it is the first one read back, so each parameter's type is compared with
  the declared `argtypes` through a fixed table of equivalents. The decision
  is revised in `docs/decisions.md`. **Acceptance:** changing the width of
  one declared integer turns the test red.

  *Done 2026-11-12.* `clausters-core-web` is 22 modules named as
  `clausters-ffi`'s are (each wasm export lands in the module of its C pair,
  read off `docs/bindings.md`; `osc` and `midi` have no C pair), the code
  moved unchanged, and the built module exports the same 1889 symbols as
  before. One part of the entry did not hold as written: `_native.py`
  declares its types through variables, list products and loops, so a Rust
  test cannot read them as text. The work is split where each side can be
  read: `crates/clausters-checks/tests/c_header.rs` generates the header with
  cbindgen and fails when the committed
  `crates/clausters-ffi/include/clausters_ffi.h` differs, and
  `test_native_parity.py` holds the declared types of the loaded library
  against that header. Declaring `clausters_core_calc`'s first parameter
  `c_uint64` fails it; the header compiles in C with every warning an
  error. The decision is "The ctypes declarations are checked against a
  header the crate generates".

- ✅ **R21: The clients are linted, and typed a module at a time** *(done 2026-11-12)*. The
  Python client is 52 thousand lines with its signatures already annotated,
  a type checker switched off but for four call-site rules, and no linter.
  It takes a linter with a minimal rule set (unused and undefined names, the
  likely bugs) and the checker's basic mode on the modules whose API is
  settled, starting with the document's (`multitrack`, `document`,
  `history`); the eleven handlers that swallow every exception are each
  narrowed or given their reason. The web client takes a linter. Both run in
  CI. **Acceptance:** zero findings on what is switched on.

  *Done 2026-11-12.* ruff runs over every Python file of the repository
  (`ruff.toml` at the root, the editor application left to its own
  workflow) with F and B less `zip()` without `strict=`; its 93 findings
  were fixed first, in their own commit, among them an `__all__` naming
  three builders that no longer exist (so a star import of `guidef`
  raised) and public names missing from two others. The eleven handlers:
  the nine in `__del__` say why they swallow, one catches `OSError` only,
  one lost its `try`. pyright's basic mode is on in the three modules'
  first line and found a closed editing context reached through `None`,
  which now raises. The web client takes Biome (`biome.json`; the choice
  and why is in `docs/decisions.md`, "The web client lints with Biome, a
  linter that is one package"): correctness and suspicious, less the rules
  about a style of typing; its 174 findings came to zero, which removed
  dead state in both clients' transport player and in the web `Server`, and
  dead helpers in the tests. CI runs ruff and pyright in the `python` job
  and the linter in the `web` job, `test.sh` runs it too.

- ✅ **R22: The editor application, reviewed** *(done 2026-11-12)*. `clausters-editor` stays a
  workspace of its own, which its version, its dependencies and its workflow
  justify. Found reading it: a restart releases the session's lock between
  ending one interpreter and keeping the next, so a request arriving then
  starts a second one that nothing ends; an interpreter replaced after a
  broken pipe is never waited on; the environments' busy flag is cleared by
  hand; the window that can read and write any file and run code has no
  content security policy; and nothing tests the driver (the whole
  protocol), the session, the environments or the page's pure modules. The
  fixes, a test of the driver as a subprocess, tests of the pure modules,
  and the 2024 edition. **Acceptance:** its workflow is green with the new
  tests in it.

  *Done 2026-11-12.* A restart keeps the session's slot locked from the old
  interpreter's end to the new one's start, a process replaced after a
  broken pipe is killed and waited on, and every lock of the application is
  taken poisoned or not; the busy flag is a guard that goes with the task
  and is released when it ends, whatever ends it. The window has a content
  security policy (scripts and fonts from the app, styles inline as the
  editor writes them, nothing framed or loaded from elsewhere; the
  development server keeps none), checked against the built page, which
  has no inline script and no `eval`. The driver has eleven tests that run
  it as a subprocess (an evaluation, an error at the file's line, a syntax
  error, `input()`, an interrupt, help answered while code runs, a query
  with or without Jedi, a bad request, the end of input), and the page's
  pure modules sixteen under `node --test` from the sources (the evaluation
  regions moved to `regions.ts` to be one of them). The crate is on the
  2024 edition, whose lints collapsed six `if`s, and builds `rlib` only.
  Not done here: a session driven by hand in the running window (evaluate,
  interrupt, restart twice), which is the user's eye pass.

- ✅ **R23: The plans hold what is open.** The plans are 37 thousand lines
  against a rule that says a roadmap and a checkbox, `docs/decisions.md` is
  one file of 277 entries, and `docs/bindings.md` carries 600 lines of what
  each ABI version changed. Closed tracks move whole to `docs/history/`, one
  file per plan they came from, and each plan keeps its standing design, its
  open tracks, its Future directions and its Found by use (whose closed
  entries stay, being the record the rules ask for). The decisions are split
  by area, and `docs/decisions.md` remains as an index carrying every title
  verbatim, since the code and the plans cite a decision by its title. The
  ABI history gets a file of its own. Done last, once the branch is accepted.
  **Acceptance:** every citation of a decision by title still finds it, and
  every pointer of `ROADMAP.md` finds its entry.
  *Done 2026-11-12, on the branch rather than after it, since `main` had not
  moved since it was cut.* Sixty-four closed tracks and arcs, 9,320 lines,
  moved to `docs/history/plans/`, one file per plan, and each plan lists their
  headings under "Closed tracks" where the first one stood; the plans went
  from 37 to 28 thousand lines, and no line was lost (checked as a multiset
  of lines against the tree before). The decisions are twelve pages by area
  under `docs/decisions/`, and the index carries the same 280 titles as the
  file did, so every citation that found its entry before finds it now; the
  283 anchors were checked against the built book, and the two citations
  that had found nothing before either (a paragraph cited as an entry, a
  title cited as it was before a rename) now name their entries. The ABI
  history is `docs/core-abi-versions.md`. Three `ROADMAP.md` entries named
  milestones that moved and now point at their history file.

- ✅ **R24: The workspace is virtual, and the server is a crate among the
  others** *(done 2026-11-12)*. The root manifest was the workspace and the
  server's package at once, so the server's `src/`, `tests/`, `examples/`
  and `build.rs` sat at the root beside the directories of every other
  component. The root keeps the workspace alone, and the server moves whole
  to `crates/clausters-server`; its package stays `clausters`, so no `use`
  and no binary name changes. The server is the workspace's default member,
  so `cargo build`, `cargo test` and `cargo run` with no `-p` act on it
  alone, as they did, and no feature of another member is unified onto it.
  **Acceptance:** the same tests run in every configuration as before the
  move; the feature matrix, the three books, the audit harness and both
  clients' suites pass; nothing outside `docs/history/` names a path that
  moved.
  *Done 2026-11-12.* `src/`, `tests/`, `examples/` and `build.rs` moved by
  `git mv`, so the history follows them, and the root manifest was split in
  two with no line of the package changed but its paths. 280 references in
  59 files were repointed by a script that replaced a path only where it
  exists under the server and not under the file's own package, and the
  prose that called the server the root was rewritten by hand (`AGENTS.md`,
  `BUILD.md`, the READMEs, the skills, `docs/contributing.md`). What read
  the root as a package learned the new shape: the levels and the versions
  checks, the audit harness's feature check and its instrumenter, pyright's
  include, and the server's examples and plugin tests, which found the
  repository by a relative path. No CI command changed, which is what the
  default member is for. The feature matrix came back clean over its 37
  configurations; CI's test rows (the default set, `embed`, `synth` alone,
  no features, the host, the native plugins) pass from the root, with the
  same 34 test binaries of the server; the three books, the wasm gate, the
  audit harness (41 tests), pytest (1595), ruff and pyright pass. Not run:
  the web client's suite, whose package changed in comments only, and the
  release workflow, which `R14`'s rehearsal covers.
  (`docs/decisions.md`, "The workspace is virtual, and the server is its
  default member (2026-11-12)".)

- ✅ **R25: The transport leaves `engine.rs`** *(done 2026-11-12)*. `crates/clausters-server/src/server/engine.rs`
  is 2,791 lines and holds the transports beside the block loop. The
  transport moves to a module of its own; nothing it does changes. It touches
  the audio thread, so it closes with `examples/bench.rs` before and after on
  the same machine, both numbers in the commit (the pull request's gate runs
  on a shared runner and is the backstop). **Acceptance:** the suites pass
  with no assertion edited, `tests/rt_safety.rs` included, and the bench
  moves inside the machine's noise.

  Shipped as `crates/clausters-server/src/server/engine/transport.rs`, a
  child module of the engine's, so it reads the engine's fields with no
  field made public: a transport's state and its edges, the cues, the lane
  entries and the loop changes, 674 lines, leaving `engine.rs` at 2,136.
  What stayed is what decides *when*: `next_due` and the block loop, and the
  transport arms of `apply`, which are one `match` with the rest. Six runs a
  side of the bench, alternated: the worst gated row moved 4.9% against the
  base, where two halves of the same binary's runs differ by 4.4% in
  throughput and 11.8% in a peak block.

  *The seam, closed after it.* `next_due` still spelled two transport
  computations inline (a rolling transport's next bundle, and its next lane
  entry) beside the one that was already the transport's (`edge_due`). They
  are `TransportState::bundle_due` and `lane_due`, so `next_due` only
  compares: it is the arbiter between the device queue and the transports,
  with the tie rules, and stays in `engine.rs`. The transport arms of
  `apply` are entered under Future directions.

## Future directions: the entries that were done

- ✅ **A list reader plays backwards** *(named 2026-10-22 by the user, deciding how the audio editor reverses: as an edit it is an NRT render, and reading backwards is the reader's)*. The transport-positioned list reader of `S21` reads forward; reading backwards gives a playhead that runs backwards (scrubbing, a reverse play) for every application that plays a list, and lets an editor reverse a long span as a part read backwards instead of rendering it. What it takes: the disk thread fills the ring ahead in the direction of play, each part's fades and seams read the other way, and a channel map and a resampled part stay correct. After `S21`. *Done 2026-11-08, found already built when it was taken:* a take's reading thread renders a list from its last frame to its first (`TakeRead::Backwards`, `crates/clausters-app-engine/src/take.rs`): each chunk is read forwards and turned round, so the ring is filled ahead in the order it is played, and a part's fades, its seams, its channel map and its resampling are the forward reading's, mirrored. The reverse render streams through it, and `a_take_read_backwards_plays_down_through_its_seams` (`tests/reading.rs`) plays it live on an engine: a cued play down through both seams frame for frame, and a loop over it with no silent frame. Nothing in `TransportDiskIn` or the transport changed, which is the decision (`docs/decisions.md`, "Backwards is a reading, not a transport"). What it gives the applications is open below, as "A play that runs backwards".

- ✅ **A measured bound on what one block drains** *(named 2026-10-15, by the
  first audit; measured and closed the same day, no policy)*.
  `Engine::drain_commands` applies every command in the FIFO at the start of a
  block, and the network thread can refill it while it does, so the FIFO's
  capacity bounds what is queued at once and not the work one block takes.
  Capping the commands per block would delay immediate commands to later
  blocks, so it was measured first: a drain-instrumented release server on the
  real device, a background load of silent nodes, and ten-second phases of
  single `/node_set` messages, immediate bundles of 32 and 128 of them, and
  `/synth_new` + `/node_free` pairs, from paced rates up to four unpaced
  senders.

  - **The producer is the bound, not the drain.** The network thread applied
    at most about 21 000 commands a second at quantum 128 and 66 000 at
    quantum 1024; everything sent beyond that was lost before decoding.
  - **The busiest block drained 1306 commands in 74 µs** (quantum 1024,
    where a callback's gap accumulates them), and 57 in 54 µs at quantum 128;
    the costliest kind, `/synth_new` with its free, peaked at 165 µs. A
    64-frame block lasts 1333 µs.
  - **No late block in any phase** at either quantum.

  The FIFO holds 8192 commands (its capacity is raised to `--max-nodes`); full
  of the costliest kind it would take about 1.8 ms to drain, but at the
  measured production that takes an audio thread stalled for over 100 ms, a
  stall that has already broken the audio on its own. So no bound is added. If
  one is ever needed, the first candidate changes no timing: drain only what
  the FIFO held when the block started, so a refill during the drain waits one
  block, as if it had arrived a moment later.

- ✅ **`/graph_moveSlot`: a slot instance changes parent without being made again** *(named 2026-09-11, with `crates/clausters-document/PLAN.md`'s `O28`)*. A clip is a slot inside its track's graph instance, and a slot's wiring is **baked into its `out` controls when it is instantiated**, so moving the node with `/node_move` would leave it writing the track it came from. Today the only answer is to free it and add it again, and the cost of that is not the node: it is everything hanging off it -- the control buses a curve had mapped to its ports (the map names a node, so it goes with the old one), the readers under it, and the ports the hand does not send because a curve owns them, which is how a box dragged to another track came back at the def's own default. The verb is one operation and not two: re-parent the node **and** re-bake the wiring of that one slot against its new parent's private buses. `out0`/`out1` are ordinary controls and `MoveNode` already exists on the audio thread, so what is missing is the bookkeeping that knows which buses the new parent hands it. It is what lets the instance projection do what a monolithic DAW does when a region changes playlist: move a pointer. **Where it lands is now one place**: `clausters_editing::instance` reproduces the rebuild deliberately (`a_clip_that_changed_track_is_made_again`), so the verb arriving is that one arm of that one match, and the test that states today's behaviour is where tomorrow's is stated instead.

  **Done 2026-09-14.** `/graph_moveSlot slotID instanceID` moves the slot group
  to the head of the new instance and re-wires every bus reference in it --
  its members' bus controls and maps, and the buses handed down to the graphs
  nested in it and to their slots -- against the new instance's buses; nothing
  is freed. The new instance must declare the slot with the same members. The
  slot's record now keeps its nodes by member (`GraphVoice::nodes`), which is
  what the re-wiring walks. `clausters_editing::instance` answers a clip that
  changed track with `Op::Move` instead of a free and a slot, so its readers
  and the maps on its ports stay (`a_clip_that_changed_track_is_moved_there`);
  a source of another width is still made again. Heard in
  `tests/mixer_graph.rs` (`a_moved_box_sounds_through_its_new_track_and_keeps_its_map`),
  and both clients have the verb (`Group.move_slot` / `Group.moveSlot`).

- ✅ **The transport's edges are square, so a stop and a play are clicks**
  *(found by ear 2026-09-11 while the multitrack example played, and then
  measured: `tests/mixer_graph.rs`, `where_the_transport_clicks`)*. A stop
  freezes the governed subtree and a play thaws it, both at a block boundary
  and both instantly, so the output steps by whatever was sounding -- 0.566 of
  full scale on a piece playing a constant, which is the worst case and the
  clearest one. Every mixer in the field ramps those edges over a few
  milliseconds, and this cannot be done where it would naturally go: a frozen
  node gets no time, so the fade has to have happened *before* the freeze.

  **What it needs is a declick window the engine owns.** A stop becomes "ramp
  the governed subtree's output down over N ms, then freeze", and a play "thaw,
  then ramp up" -- which asks two things this does not have yet: a place to
  apply the gain (the piece's master strip is the obvious one, but the
  transport governs subtrees that are not pieces, so the engine wants something
  that is not the mixer's), and a rule for what the transport position does
  during the window (it must not advance, or a stop would play material past
  where it stopped). Neither is hard; both are decisions. Until then a pause
  mid-note clicks, and the example says so.

  **Done 2026-09-24, and ticked 2026-10-07 — the fix shipped as the server
  half of the audio editor's declick and this entry was never closed.** Both
  decisions were taken there, and neither as the entry guessed. **Where the
  gain is applied**: nowhere in the engine. A transport publishes a declick
  level and the `TransportFade` UGen reads it, from the transport of the
  nearest bound group, so whatever sits *outside* the governed group
  multiplies by it — the multitrack's way out (`mixer::out_def`) and the
  audio editor's — and a subtree that is not a multitrack gets the same level
  by naming the same UGen. **What the position does during the window**: it
  advances. A stop with a ramp is a stopping phase in which the transport
  rolls in full while the level falls, and it freezes on the sample the level
  reaches zero, so the position rests where the readers stopped reading; a
  stop then plays the ramp's length past where it was asked, which is what
  every mixer's does and is the opposite of what this entry required.
  `/transport_fade <transport> <samples>` sets the ramp, 0 by default, and the
  multitrack's player and the audio editor's both send it
  (`clausters-editing`, `FADE_SECS`). The rationale is in `docs/decisions.md`.

  What a client that sets no ramp hears is unchanged, and
  `where_the_transport_clicks` still measures it; with a ramp,
  `a_stop_fades_the_master_and_leaves_its_meter_to_fall` asserts the edge at
  the hardware and `tests/transport.rs` the level on a stop and on a play.

- ✅ **A UGen's trailing inputs could be declared optional, so a def survives one growing** *(named 2026-08-16, after S17 moved `PlayBuf` from four inputs to seven and every stored def that used it stopped compiling — seven of them on the author's machine, warning at every boot)*. Arity is exact: `synthdef::compile` rejects a UGen whose input count is not the descriptor's, and `UGenInput.default` is advisory metadata the server never applies. So a UGen that grows breaks every def written against it, including the ones already persisted on disk and the ones inside a saved bundle.

  **The cheap version is the dangerous one and is refused**: "fill whatever is missing from the defaults" makes a short def legal in all 189 input slots of the catalog, and `BinaryOpUGen` is `a=0.0, b=0.0` — a `Mul` truncated to one input would compile, fill `b=0`, and silence the chain with no `/fail` and no name. That is precisely the failure S17 chose an unusual input order to avoid.

  **The shape worth having is a declared optional tail per UGen**: the descriptor says from which index the tail begins, and only there does the compiler fill from `UGenInput.default`. A short `Mul` still fails; a four-input `PlayBuf` still plays. **The user's conditions, stated when the idea was raised** *(2026-08-16)*: no runtime penalty, and resilience to signature changes. The first is met by construction — the fill happens in `compile`, on the network thread, once per def, and the compiled def is byte-identical to one a complete client sent, so the audio thread never learns of it. The second is what the feature *is*, with one limit to state plainly: it makes growth **by the tail** non-breaking and does nothing for an input inserted in the middle, which stays breaking and stays silent.

  **The cost is not the implementation** (interning a constant and pointing an `InputRef` at it is ten lines beside the arity check). It is auditing the 189 slots once to decide where each UGen's tail begins, and accepting that a declared default becomes **wire contract** — changing it afterwards is a breaking change for every def leaning on the fill, where today all 189 are free metadata.

  **What it does not solve, so it is not proposed as a fix for it**: the three copies of the default table (server registry, Python builder, TypeScript builder). A client still materialises every default before the last input the caller *did* give, since the wire is positional and has no sparse form — `play_buf(buf, loop=1.0)` must emit `chan` and `rate` itself. Keeping those copies honest is the contrast tests' job, one per client.

  **Built 2026-08-18, in the shape above and with the audit done.** The declaration sits on the **slot** rather than on the descriptor (`UGenInput::optional`, written `inp_opt(...)` in the catalog), so the fact reads next to the input it is about and a shared signature constant carries it to every kind that shares it; a test enforces that the optional slots are a *suffix* and that a variadic kind's fixed head has none, which is what keeps "how many inputs are required" one number. `compile` accepts a def between `required_inputs()` and the arity and fills the rest from the declared defaults, so a four-input `PlayBuf` compiles into exactly the seven-input def a complete client would have sent.

  **The audit, which was the cost, came out as a rule rather than as a list**: a slot joins the tail only when its default is **inert** — the value that makes the UGen behave as if the slot were not there (0 for a trigger, an offset, a phase, a channel, a done action; 1 for a level or a rate scale) — and never when its default is a *choice* (`freq=440`, `delaytime=0.2`, `width=0.5`, `max=7`), because omitting that is not leaving it alone, and never when the UGen reads its **signal, source, position or chain** from it, whatever the default, because silence and frame 0 are legal values a wrong def would run with. Forty kinds have a tail under that rule and the operators have none, which is the refusal this entry opens with. The reasoning is in `docs/decisions.md`, the rule in `docs/schemas.md`, and the acceptance in `tests/synthdef.rs` (a short `PlayBuf` equals the long one; a truncated `Mul` still fails; too many inputs still fails).

  **What it does not do**, unchanged from what is written above: an input inserted in the middle stays breaking, the clients still materialise every default (the wire has no sparse form), and `/ugen_query` still reports `name, default` per input — publishing optionality would change the reply's shape in three packages and nothing needs it yet.

- ✅ **`LinLin`/`LinExp` as UGens, over the warp family the core already
  carries** *(named 2026-08-23, adding the warp family to `clausters_core`;
  done 2026-08-24)*.
  The eight range maps — `linlin`, `linexp`, `explin`, `expexp`, `lincurve`,
  `curvelin`, `range`, `exprange` — now exist as **value** functions in
  `clausters_core::warp`, reached by both clients, so a script maps a fader
  position to a frequency without writing the arithmetic. A **signal** cannot:
  there is no `LinExp` UGen (scsynth has one) and no `LinLin` pseudo-UGen, so
  `saw(...).linexp(-1, 1, 200, 8000)` is unwritable and a def spells the
  multiply-and-add by hand — which is exactly the drift the shared core exists
  to prevent, since a def's hand arithmetic and a client's `linexp` are then two
  implementations of one map.

  **The formulas are not the work; the plumbing is.** `warp::apply_map` is the
  body of every one of them and a UGen would call it per sample, so what this
  milestone actually decides is the *shape*: they are five- and six-ary, so
  they cannot join the `BinaryOpUGen`/`UnaryOpUGen` tables the way `midicps`
  did, and each needs a descriptor in `dsp::registry`, an entry in
  `docs/schemas.md`, a builder in `clients/python/clausters/defs/ugens` and its
  TypeScript mirror, plus a row in the contrast tests that already caught eleven
  drifted builders. Two questions come with it: whether the bounds are UGen
  inputs (modulatable, which scsynth's `LinExp` allows) or init-rate like a
  ramp's geometry, and whether `range`/`exprange` are UGens at all or the
  method sclang makes them — a composition of `madd` that reads the signal's
  own polarity, which our graph does not track.

  **Done, and the shape is one kind rather than eight.** `RangeMapUGen` carries
  the map by **name** in its `op` field over `in, in_lo, in_hi, out_lo, out_hi[,
  curve]`, which is the argument `BinaryOpUGen` already makes: a math operation
  is an entry in a shared table, not a kind. `clip` rides beside it as static
  config (`minmax` by default, as on the value side), `curve` is the declared
  optional tail — 0 is *no bend*, the inert value the tail rule asks for, while
  a client's `lincurve` passes sclang's −4 — and the four bounds are ordinary
  inputs, so **a range may be modulated**. Both clients spell it as methods on
  a graph node (`lfo.linexp(-1, 1, 200, 8000)`), the sixth of which is where
  the TypeScript side grew `GraphExpr`: `PvExpr` borrows `SynthExpr` for the
  operators and must not inherit a map that composes a UGen.

  **The two questions the entry left are answered by what they cost.** The
  bounds are **inputs**, not init-rate: they are what a modulated map is made
  of, and the price of moving them is paid by the def that moves them, not by
  every def. And `range`/`exprange` are **not** UGens — the value functions keep
  them — because they read the input's own polarity, which sclang knows per
  UGen and this graph does not track: a def that means −1..1 writes −1..1 and
  says what it means, which is one word longer and never wrong.

  **The efficiency half is in the core, and it is what kept one implementation
  of the curves.** A map's bounds settle a good part of its arithmetic —
  the zero-straddling test, `ln(hi/lo)`, `exp(curve)`, the bend's two terms, two
  of them transcendentals — so `warp::Map` is the prepared form and the eight
  named functions are each one of those built and applied once. The UGen builds
  one per **block** when every bound is a constant (the ordinary def) and one
  per sample when a bound moves; `map_slice` builds one per array. Nothing was
  restated to get that: the primitives now *are* `Read`/`Write`, and
  `each_named_map_is_the_pair_of_primitives_it_documents` asserts bit equality
  over a sweep, because a def and the client control driving it read the same
  map and a last-ulp difference between them is drift.

- ✅ **A release bumps one version in five places and four of them are checked
  by nothing** *(named 2026-08-23, fixing the versioning rule and finding the
  check only covered the rule's own half; done 2026-08-24)*. The version lives in the root
  `Cargo.toml`, every crate under `crates/`, `clients/python/pyproject.toml`,
  `clients/web/package.json`, and `crates/clausters-gui/Cargo.toml` with its own lockfile
  (an independent workspace, so it needs `cargo update -p clausters-gui
  --offline` after the edit). **Only one pair is verified** — `npm run
  check-package` refuses a `package.json` that disagrees with the crate — and
  `crates/clausters-gui` has drifted before precisely because no root build reads its
  manifest. `versions.sh` does not close this: it answers the *rule* question
  (did a counter move since the last tag, did the tier move once) and reads only
  the root manifest.

  The fix is one of two shapes and they are not the same work. A **check** — the
  script grows a pass that reads all five and fails on disagreement, cheap and
  it only catches the mistake after it is made. Or a **single source** — the
  crates take `version.workspace = true` (they do not today), and the two
  non-Cargo manifests are written from it by a small script, so there is one
  number to edit rather than five to keep equal. The second is what makes the
  drift impossible rather than reported, and it is the one worth taking, with
  the caveat that `crates/clausters-gui` is its own workspace and inherits nothing from
  the root.

  **Both shapes, because neither closes it alone.** The eight Cargo crates take
  `version.workspace = true`, so the number is *decided* once, in the root
  `[workspace.package]`. The five files that cannot inherit anything —
  `crates/clausters-gui/Cargo.toml` with its lockfile, `pyproject.toml`,
  `package.json` with its lockfile — are **written** by
  `scripts/set-version.sh <x.y.z>`, which regenerates both lockfiles and, given
  no argument, prints what every file says. And because a script is a thing to
  remember, `tests/versions.rs` contrasts all of them and fails when one
  disagrees — including when a Cargo crate writes its own number instead of
  inheriting, which is how the *next* crate would drift.

  **It found one on the way**, which is the entry's own argument arriving on
  time: `clients/web/package-lock.json` was a sixth place and had been sitting
  at `0.8.1` since two minors ago. It was not in the entry's list of five and
  nothing had ever looked at it.

  The one seam this moved is the web package's own checker
  (`tools/check-package.mjs`), which read the root `[package] version` and had
  to learn `[workspace.package]`; `versions.sh` in the skill matched
  `^version` and would have read `version.workspace = true` as "true", so it
  matches `^version = ` now. The release procedure is one line and one command
  in `BUILD.md` and in the skill.

- ✅ **A take's overview could be a file beside its region** *(named 2026-08-18, dropped from S20 the day it was staged — see that milestone for why it stopped being the mechanism)*. Every multitrack editor keeps a summary beside the audio and writes it as the audio is written: Audacity's block files carry min, max and RMS per 256 samples with a second level over 256 of those, which is what lets a project open without reading its audio. Clausters has both halves already — the CLPK format the core writes (`peaks::MultiPyramid::write_cache`, mmap-ready, the same three statistics) and a per-buffer region file the segment names — so the overview would be `<segment>.buf<N>.<gen>.peaks`, created with the region and unlinked with it, and a peer would **map** it instead of building one. What it buys is exactly one thing: the pass a take costs when a view opens it (a ten-minute stereo take is 57.6 million samples read once), which after H7 is a streamed read that allocates only the summary — real, but bounded and paid once. What it costs is a **second writer of derived state**: who refreshes it after an edit (the peer that wrote the samples, the owner, or both), what a reader does with one that disagrees with the material, and whether a stale one is detectable at all. The frontier (S20) deliberately answers the *live* question without any of that, which is why this is an optimization to take on its own terms rather than a milestone's implementation detail. It becomes worth it when a session opens many long takes at once and the opening pass is measured to matter.

  **Done 2026-08-20, and the two costs it named were paid rather than dodged.** The file is `<segment>.buf<N>.<gen>.peaks`, the CLPK the core already writes, built when a buffer is published to its region and unlinked with it. **Who refreshes it after an edit**: the server that owns the samples, and nobody else — a peer maps it read-only, and a peer that edits announces the span exactly as it already did (`/buffer_touch`), which is one more reader of a contract that existed. **What a reader does with one that disagrees**: it cannot get one, because the refresh is by span and not by rebuild — `NrtAction::Wrote` carries the frames every in-place write covered, `Overviews::wrote` recomputes the buckets it touched, and the file is written from the run loop at a bounded rate so a stroke does not rewrite a multi-megabyte file per announcement. **Whether a stale one is detectable**: the generation is in the name, so the one drift left is a peer that stores into the mapping and never announces it — and that is the same thing every *picture* of that take already cannot follow.

  The arithmetic is one function, `peaks::overview` in the shared core, and it is the same one the wire uses: `/buffer_stream` reports with it as a take is written, `/buffer_peaks` answers with it for a take at rest, and the refresh above patches with it. So the summary a peer maps, the summary a page is sent and the summary the server keeps are the same three numbers computed once.

  **And something waited on it**, which it did not when this was written: the GUI host has two ways of getting a view's material — the whole buffer downloaded, or shape plus a summary plus the spans a zoom asks for (H9) — and what chooses between them is the `fills` prop, a question about *recording* standing in for a question about *cost*. The right condition is whether a summary can be had without reading the material, and today it can only arrive free from `/buffer_stream` or from a cache file somebody already wrote. Whoever takes this entry unblocks that one (`crates/clausters-gui/PLAN.md`, Future directions, "A view of a server buffer has two ways of getting its material, and what chooses between them is the wrong question").

- ✅ **A long take is played out of the pool, and `DiskIn` cannot be
  positioned** *(taken whole by `S21` on 2026-10-22, and closed with it)*
  *(named 2026-08-15 alongside S12, and narrowed the same day once
  S12 settled where an* edit *happens: what is left here is where a long take is*
  read *from, which S12 does not answer)*. The arrangement sounds a take as a
  **pool buffer** played by an instrument def (`clausters.form.element`: a
  `Buffer` element renders as an event whose `buf` control carries the bufnum),
  so a five-minute stereo take is 110 MB of RAM and a thirty-minute one 660 MB —
  per take, on top of the client's working copy and the host's mapped view of
  the same material. A multitrack of takes is the case that makes that
  untenable, and it is also the case the streaming pair was built for:
  `DiskIn`/`DiskOut` (`crates/clausters-server/src/dsp/disk.rs`) run their own I/O thread and never
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

  **And a join has to be able to name a file, which is the half this entry was
  missing** *(the user, 2026-09-12, reading the document's `O31`: "multitrack
  editors read from files; here we also read from buffers on the server")*.
  `/buffer_stitch` takes `srcBufnum` for every part, so a join is a join over
  **pool buffers** and nothing else -- which is the same 110 MB per take this
  entry opens with, now arriving through the one feature built to avoid copying
  anything. A multitrack's ordinary source is a file on disk; a buffer is the
  case *this* server adds. So the part needs the other spelling: a path, a start
  frame and a count, read by a stream rather than out of the pool, with the same
  fades and the same channel map. The document's half is already written that
  way and does not move -- a part names a **source and a frame range**, and
  which of the two a source turns out to be is the session table's answer
  (`crates/clausters-document/PLAN.md`, `O31`).

  **Prebuffering is what makes it playable, and it is the reason this is one
  entry and not two.** A positioned `DiskIn` still fills its ring from a
  background thread, so a seam between two file parts is a refill at a known
  frame, not an underrun to be survived: the reader knows where every part
  begins before it gets there, so the next part's head can be read **ahead** of
  the seam rather than requested at it. That is the whole difference between a
  join over files that plays and one that clicks, and it is also what makes a
  start offset worth having on its own -- a transport that starts at bar 40
  seeks once, into a ring that is already full, instead of beginning to read
  when the playhead is already moving. The ring's size and how far ahead of a
  seam a refill starts are the two numbers the work has to pick and state.

  **What has to be measured before it is trusted** *(from the node system's
  spec, 2026-09-10)*: how far ahead the disk thread has to read so a seam never
  arrives late, stated against the hard case -- a comping pass chopped into
  quarter-second parts, each from another file -- and what the reader does when
  the thread runs out of lead anyway, which is declared silence and never a
  click. The shape is the stitched buffer's one layer down: the ring `DiskIn`
  fills becomes a stitch of segments, and the audio thread goes on reading a
  buffer and nothing more.

  **The start frame, done 2026-10-08** -- the smallest step, as this entry
  said. `DiskIn` takes a static `start` beside `path` and `loop` (the file
  frame it begins at, `0` by default; a loop goes back to the file's top), in
  both clients' builders (`disk_in(start=)`, `diskIn({ start })`) and in a
  page, where the host's reader asks for its first span from there. The reader
  thread reaches it by decoding up to it and throwing the frames away before
  the ring sees them: exact in every format, since a compressed one has no
  exact frame seek, and the audio thread's first frame is `start`. Still open
  here: a seek under a moving playhead, a join's part that names a file, and
  the prebuffering that makes either playable.

- ✅ **Several transports on one server, to play concurrently** *(moved to the
  T track as T6, 2026-09-23, when the audio editor's nodes gave it a case.
  Named 2026-09-17 with the user, designing how a timeline plays on a server's
  transport; `clients/python/PLAN.md`, the tempo-map entry)*. A server has
  one transport today -- one governed group, one transport clock, one position
  anchor, one loop -- so two things that each want their own play, pause and
  locate (two timelines, or a timeline beside a multitrack) share one playhead.
  The shape: a transport becomes a resource addressed by id and sized at boot
  (like `--taps`), so the audio thread never allocates, each with its own
  governed group, clock (frozen total), position anchor, loop and
  transport-axis queue. The id reaches every surface that names the
  transport: the `/transport_*` commands and `/sched_atTransport`, the
  `TransportPos` UGen, the per-transport clocks and positions in the
  shared-memory segment, the change notifications, and `/gui_headClock`, which
  says which transport a view draws from. A protocol change across the server,
  both clients, the GUI host and the books at once; `/transport_query` and the
  conductor examples change shape.

  **The timeline's transport property was built first, and named to survive
  this** *(2026-09-17)*: `timeline.transport = server` holds the server, and a
  server with several transports makes it hold one of them
  (`server.transport(id)`) with nothing else in the surface moving.

- ✅ **Two node trees kept in step by hand** *(the server audit,
  2026-09-25; the test written 2026-10-07)*. The network's `TreeMirror` and the engine's `NodeTree` implement the
  same semantics (insert by add action, move, free all, deep free, the ancestor
  walk), and `dispatch.rs` says one rule is "kept in step with
  `Engine::bundle_is_governed` by hand". Two threads, two sets of rules, so two
  implementations are defensible; nothing checks them against each other. A
  test driving both with one sequence of operations is the cheap guard.
  *(Written: `node::tests::the_mirror_and_the_engine_s_tree_take_the_same_walk`
  puts one fixed walk of four thousand steps to both — every add action, every
  place of a move, the three frees, over ids that are live, gone and never
  there — and after each step both took it or both refused it, the two trees
  read the same row for row, and the ancestor walk agrees over every pair of
  ids. **It found one difference.** The network's walk was a method of the
  dispatcher, which the test could not reach, so it moved onto the mirror
  (`TreeMirror::is_descendant_of`) — and there an id the tree does not hold
  was at or under itself, where the engine answers `false` for any unknown
  id. So a `/sched_atTransport` whose packet names the transport's group
  after that group was freed passed the network's check, though the engine's
  tree holds no such node to govern; the mirror now answers as the engine
  does.)*

- ✅ **A pitch spelled by a key, and moved by a step of it** *(2026-11-10,
  with the user, from `crates/clausters-apps/PLAN.md`, `X37.2`)*. What the
  core has spells by an accidental world alone:
  `clausters_core::event::notation::transposed` moves written `pitches` by
  semitones and spells them again in sharps or in flats, and
  `clausters_core::notation::ops::transpose_pitch` moves one by steps and
  semitones as the score editor does. What is missing is the spelling a
  key gives (a pitch of the scale written as the scale writes it, one out
  of it by the nearest name on the line of fifths) and a transposition by a
  step of the key, which moves each pitch to the next degree of its scale.
  Both are general and belong beside the spelling the core already has:
  the roll's transposition calls them, the Spelling step of
  `crates/clausters-notate/PLAN.md` uses them as its baseline, and both
  clients reach them through the core's doors.
  *Done 2026-11-10*: `spell_in`, `transposed_in` and `stepped_in`, with
  `fifths_of`, `key_of` and the table of tonics, in
  `clausters_core::event::notation`; the document's reading spells with
  `spell_in` instead of a copy of its own, and the score's MEI writer names
  the same table. No FFI export yet: the clients reach them through the
  roll's verbs, and an export waits for a client that calls one.

## Found by use: the entries that were fixed

- ✅ **The embedded engine runs without the SIGXCPU guard** *(found
  2026-10-15, by the first audit: the Python suite's embed tests were killed by
  SIGXCPU in the PipeWire callback, on a debug build sharing ten CPUs with
  other compilations, and not on an ordinary machine; fixed 2026-10-15)*.
  RTKit grants the audio thread real-time scheduling under `RLIMIT_RTTIME`, and
  the binary arms `server::rt::install_sigxcpu_guard` so a thread past that
  budget is demoted instead of killed (M24). The library never armed it, since
  a signal disposition is process-wide, so an embedded engine died with a core
  dump under the same overload. The embed now arms it through
  `server::rt::adopt_sigxcpu_guard`, which replaces only the default
  disposition and leaves one the host process set. Confirmed by lowering the
  soft `RLIMIT_RTTIME` of a Python process running the embed: without the guard
  it died of SIGXCPU at once; with it the audio thread was demoted and the
  process kept running past 800 nodes at 117% average load.

- ✅ **The audio callback costs half again as much on the efficiency cores**
  *(found 2026-10-15, by the second audit campaign's device runs; record
  `audit/records/2026-10-15_2558210e_02.zip`)*. On a hybrid CPU (performance
  cores 0-11, efficiency cores 12-19) the same load at quantum 128 ran at 32 %
  average CPU and ~860 us per callback on a performance core, 53 % and ~1420
  us on an efficiency core. Nothing chooses the core (`--pin` is opt-in and
  experimental) and the thread migrates between the two kinds within a run, so
  a server's headroom depends on where the scheduler puts it.

  Forty minutes at that load (900 000 callbacks, with a capture of every
  callback over 90 % of its deadline) gave no overrun: three near misses, all
  on an efficiency core, each two DSP blocks of ~1.25 ms with no page fault,
  no context switch and no command applied, so the margin is the core's speed
  and nothing else. The two overruns seen earlier (3.03 ms and 2.69 ms, in
  ~340 000 callbacks) were not reproduced, and one PipeWire error came with no
  slow callback of the server's. The boot diagnostic now names the CPU the
  audio thread is on and, on a hybrid CPU, whether it is one of the fastest;
  `--pin fast` restricts the audio thread and the workers to those cores
  (`server::rt::fastest_cpus`), which held the same load at 32 % in three of
  three runs. It stays opt-in, decided with the maintainer: it changes how the
  server shares the machine, and the default keeps the kernel's choice.

- ✅ **Every stored plugin def is probed again at boot, before the server
  answers** *(found 2026-10-14, fixed 2026-10-15)*. Restoring
  `defs/plugindefs/` probed each record on the network thread during startup,
  so a data directory that had seen a heavy instrument loaded it on every boot
  and a missing or slow binary delayed the server by its timeout. A record now
  keeps the surface the probe found when the def was sent; restoring rebuilds
  the def from it without loading the plugin, and the first node's instance is
  what checks the binary still matches. A record written before is probed once
  and rewritten.

- ✅ **Clamped audio buses in dependency analysis** *(fixed 2026-10-14)* —
  audio I/O clamps an out-of-range index to the last configured bus, while
  the mirror had analyzed its unbounded index. Ordering, worker masks and
  compensation now analyze the same physical bus, including control retargets
  and mapped controls. A reversed source/reader graph with a reduced bus count
  tests the dependency and allocation-free parallel execution.

- ✅ **Offline plugin unit fixture header path** *(fixed 2026-10-14)* — the
  unit test compiled the bridge fixture against the retired
  `tools/plugin-bridge/vendor` include directory. It now uses the same
  `third_party/clap/include` headers as the production helper and integration
  fixtures, so a clean checkout does not need an ignored legacy vendor tree.

- ✅ **Large CLAP catalogues hid compatible LSP plugins.** Discovery rejected binaries with more than 128 identities, and one over-capacity metadata row failed every sibling. Raise the bounded catalogue to 512, page administrative requests in groups of four and retain per-plugin compatibility diagnostics; the installed LSP catalogue reports 182 compatible plugins and 12 parameter-capacity diagnostics.

- ✅ **A lane's notes froze with the transport, and a stop cut their releases**
  *(the user, 2026-09-29, playing `edit_notes`: "The transport cuts the sound
  before the last note finishes playing", and "Placing the cursor and pressing
  play plays a fragment of the last note of the previous playback")*. `T8`
  made a lane's notes in the governed group, so a stop froze them: the end
  mark, on the last note's end, froze the voice as its release began, and the
  release was never heard; and a voice frozen mid-release thawed on the next
  play and finished its release there, wherever the position now was. Freezing
  is right for a reader, which continues from the sample it read; a voice is
  not one. **Fixed 2026-09-29**: a stop -- `/transport_stop` and the end mark
  alike -- releases what the lanes sound, as a DAW's stop sends its note-offs,
  and the notes are made in a group the transport does not govern (the group
  that follows it, for the notes editor and the multitrack), so their releases
  ring out. A play does not bring a released note back. `/lane_new` says to
  give it such a group.

- ✅ **A group's sort mode is in no structured reply** *(found 2026-09-18 while
  giving both clients the manual ordering verbs, whose docstrings have to say
  "refused inside an auto-ordered group" and leave the reader no way to ask
  which kind a group is; fixed the same day)*. `/group_sortMode` sets it and nothing reads it back:
  `/group_queryTree.reply` carries `ID, count, name` per node and no flag, and
  `/node_query.reply` carries none either. The one place the answer exists is
  `/group_dumpGraph.reply`, which prints `group 1000 (auto)` inside a debug
  **string** -- the one thing both books tell a client never to scrape. So a
  client that now has both ways of ordering cannot say which one it is looking
  at, and a script that moves a node by hand finds out from a `/fail`.

  The shape is a reply change, and that is why it is here rather than in a
  client's plan: a flag per group in `/group_queryTree.reply` is the obvious
  place (it is already the structured tree), and it moves the reply's shape, so
  both clients' parsers and `docs/schemas.md` move with it. Whether `parallel`
  rides along is the same question asked twice, and the two are one decision.

  **What shipped.** Both flags, both replies, and they are one pair: sort mode
  and parallel are the two modes a group *runs under*, and reporting one without
  the other would have left the same hole one question over. A group's record in
  `/node_query.reply` ends with them, after the name -- which was already past
  scsynth's record, so nothing there had to move. In `/group_queryTree.reply`
  they ride at **`detail = 2`**, after each group's name and after the queried
  group's own, which is what keeps 0 and 1 exactly the shape scsynth replies
  with; the level-2 reply's first entry therefore starts at argument 6 rather
  than 4. Both clients carry them on the node record (`auto_order`/`autoOrder`,
  `parallel`) and **print them on a group's line** -- `group 1000 "mixer" (auto,
  parallel)` -- which is how `basics/group_order.py` and its page now show the
  mode changing under the same tree they already printed.

  **Every walker of that reply had to move, including the one that does not read
  the field.** The GUI host's node-tree view asks for detail 1 and would not have
  broken today, but its parser handles detail 2, so it now steps over the two
  ints there: a group read as a synth is what a missed field looks like, and it
  would have surfaced as a corrupt drawing long after this change.

- ✅ **The disk threads are the one worker the load table cannot see** *(found
  2026-09-15 while wiring M34: every other long-lived thread reaches the table,
  these do not)*. `DiskIn::open`/`DiskOut::open` (`crates/clausters-server/src/dsp/disk.rs`) spawn their
  reader and writer from the **UGen registry**, during a synth's build, which is
  the one construction path with no route to the engine's `Arc<Meters>` -- and
  there is one thread per node rather than a fixed count, so the fix is not a
  slot each but an **aggregated role** (`disk-in`, `disk-out`, index 0, several
  threads adding to it), which the table already supports since a slot is only a
  pair of atomics. What it needs is a way for a UGen to reach the table at build
  time: `UGenConfig` is what the registry hands a UGen, so carrying the meters
  there is the obvious shape, and it is the part worth thinking about rather
  than typing. Until then a session bottlenecked on soundfile streaming reads as
  idle in `/server_load` and honest in `top -H`, which is the one direction a
  meter must not be wrong in -- so it is named in `docs/schemas.md` as well as
  here.

  **Fixed 2026-10-07.** Two roles, `disk-in` and `disk-out`, each one slot that
  every thread of its kind adds to. The table reaches a UGen through
  **`BuildCtx`** rather than `UGenConfig`: the config is the def's — static,
  compiled once, shared by every instance and by every server that loads the
  def — while the build context is "what the engine knows at build time",
  which is exactly what a load table is, and it already existed for the sample
  rate. `CmdTranslator::set_meters` hands the server's table over at boot and
  `make_synth` builds with it (`UGenSynth::metered`); an offline render and a
  test keep a table nobody reads.

  A thread accounts **the stretches it worked, never the parks between them**
  (`dsp::disk::Turn`): a stretch runs from one wait on the ring to the next and
  counts only if something was done in it, so a reader waking to a ring still
  full is not a call. Nothing of this is the audio thread's — `process` is the
  same push and pop it was — and that is now held rather than assumed:
  `crates/clausters-server/tests/rt_safety.rs` had no case for the disk UGens at all, and gained
  `disk_streams_do_not_allocate_on_the_audio_thread`, which runs both under
  `assert_no_alloc` with a live table. The wiring from `/synth_new` to the
  thread is held by `server_load_counts_the_disk_threads` in `crates/clausters-server/tests/osc.rs`.

- ✅ **A stop froze the piece's state, and it came back holding it** *(found
  2026-09-11 by ear in the multitrack example; fixed the same day)*. The
  transport governs a group by **pausing** it, so while stopped nothing in the
  piece gets a block -- while everything that drives it goes on moving: the
  position, and the curve nodes outside the group writing the buses its ports
  are mapped to. So every smoother inside kept the value the music ended on. On
  play a strip glided from there to where the automation now was, over the
  fader's ten milliseconds, which is a burst of whatever the box holds at a
  level nothing asked for -- measured at 0.56 where the envelope said zero, and
  fainter the further into the timeline the box sat, since by then more of the
  glide was over. The same freeze made a meter report the *last* pass's peak
  before the new one had played a sample. Both are one thing: **a transport
  stop is not a node pause.** A paused node should resume exactly as it was; a
  stopped transport is a piece that is not there, so `SynthNode::resume` --
  called on the governed subtree, and only when it thaws -- lets every smoother
  forget where it was. Only smoothing: a filter's memory, a delay line and an
  oscillator's phase are the signal itself and stay. Held by
  `a_thawed_strip_does_not_glide_down_from_where_it_stopped` and
  `a_thawed_meter_does_not_report_the_pass_before_it`.

- ✅ **A track's width is a document field the piece cannot honour yet** *(found
  2026-09-11, reading the meter's channel count against the strip it draws)*.
  `Track.channels` is a field of the document and the plan reports it per
  track, but a piece instantiates its tracks from one slot -- `mt.piece.<n>`
  declares `tracks` as `mt.track.<n>` with the **piece's** width -- so a track
  of another width gets the piece's strip and only its *meter* is drawn at the
  width it asked for. Nothing today writes a track of another width, which is
  why it has not been heard. The shape of the fix is the clip slot's, which
  already solved the same problem one level down: a slot per width
  (`clips.1`, `clips.2`), so the piece would declare `tracks.1` and `tracks.2`
  and the plan would name which one each track fills.

  **Fixed 2026-10-07**, in that shape. The tracks' group declares a slot per
  track width (`mixer::track_slot`), the plan names the one each track fills
  (`PlannedTrack::slot`), and the track graph is named by both widths
  (`track.<track>x<master>`), since two multitracks of different masters may be
  loaded at once.

  What the entry did not say is **where the width changes**, and the clip's
  answer does not carry over: a clip has no meter, so its strip can go straight
  from the source's width to the track's. A track has one, after its fader, and
  a mono track read on a stereo meter is the thing the entry was about. So a
  track is its own width through its mix bus, its strip and its `post` bus —
  which is what the meter reads — and the node that carries it into the master
  is where the width changes: the send when the two agree, the 1→2 strip (a
  pan, with the law) for a mono track in a stereo multitrack, the 2→1 strip
  (the sum) the other way. `pan` stays one port on every track and lands on
  the stage that has an image to move: the way out for a mono track, the
  track's own balance otherwise.

  `defs_for` sends every width a strip is written for rather than the ones in
  use, because a graph must exist before the one that names it and the tracks'
  group names both track graphs. A track whose width changes is freed and made
  again with its clips (`Instance::tracks`) — another graph in another slot is
  not something a `set` can say.

  Heard rather than inspected, in `tests/mixer_graph.rs`:
  `a_mono_track_is_one_channel_until_it_meets_the_master` (centred at −3 dB a
  side, its meter one channel at the fader's level and unmoved by the pan) and
  `a_stereo_track_is_summed_into_a_mono_master`. The plan and the instance
  hold their halves in `the_track_width_picks_its_slot`,
  `a_track_fills_the_slot_of_its_own_width` and
  `a_track_of_another_width_is_made_again_with_its_clips`.

  And through the whole playback, in `tests/multitrack_playback.rs`:
  `a_track_s_width_is_the_document_s_and_may_change_while_it_plays` widens a
  mono track while it sounds — which is what `track.channels = 2` from either
  client does — and hears it made again.

- ✅ **A meter's ballistics were nobody's, so a level had no readable shape**
  *(2026-09-10, with the node system)*. A picture of the raw block peak is
  unreadable: it flickers, and a transient shows for one frame of the screen or
  for none. Every meter anybody has ever read answers three rules instead --
  instantaneous attack, a fall of a fixed number of decibels per second, and a
  peak held long enough to be seen -- and those rules are arithmetic, so they
  are `clausters_core::measure::Ballistics` and there is one of them. Two
  clients drawing two different falls off one signal would be two answers to a
  question that has one.

  `Meter(signal, decay, hold)` runs them on the server: one number a block,
  which is exactly what a control bus carries, so a metered strip costs one node
  and one bus per channel and a host reads a *range* of buses rather than a
  message per value. `hold` of 0 is the level and a second or two is the mark
  that waits to be read, so two instances give both halves of a meter.
  `mt.meter` is the def that pairs it with `OutCtl`.

- ✅ **A track's meter has nowhere post-fader to read from** *(found
  2026-09-10 wiring the meter, fixed the same day)*. A meter is post-fader
  because that is what a hand expects, and a track's strip wrote straight into
  the **piece's** mix bus, which every other track writes into too -- so a meter
  on it read the sum and called it the track. A strip now writes a `post` bus
  private to its own instance, and `mt.send.<n>` carries `post` onward at a
  gain: one more node per strip, and the same shape a pre/post-fader send wants,
  so an extra send is another instance of a def that already exists rather than
  a mechanism. The meters are a **slot** (`mt.meter.<n>`, one instance covering
  the strip's width), so a piece nobody is looking at holds none and the level
  and the mark that waits are two instances rather than two defs; where each
  channel lands is a port (`meter/out0`, `meter/out1`), because the host reads
  the bus and so the host says which one. `tests/mixer_graph.rs` renders it: a
  silent track beside a loud one reads nothing, the master reads the sum, and
  the send's gain shuts the track off without touching the fader an automation
  writes. The other half is `G35.14` in `crates/clausters-gui/PLAN.md`: the vertical
  strip of *n* channels in a track header, which reads that range of buses --
  until it lands, nothing instantiates a meter, because nothing draws one.

- ✅ **A curve was drawn, edited and saved, and heard by nothing** *(the oldest
  half of `G35.3`/`G35.4`; closed 2026-09-10)*. An automation's target said
  `gain` and the header knob said `gain` and they were two words spelled alike.
  Now they are one port, and the curve reaches it.

  **A curve is a table read at the transport's own position**, not a stream of
  messages, and the difference is the whole design: with messages a locate would
  leave the curve wherever the last one put it, so every seek would have to
  re-send it, and the resolution of an automation would be the resolution of a
  socket. `mt.curve` reads a buffer at `TransportPos` and writes a control bus,
  so a curve is simply *at* wherever the piece is -- a locate costs nothing, and
  it works the same offline. `BufRd` clamps past either end when it is not
  looping, which is exactly a curve holding its first and last value.

  **`/graph_map instanceID port bus` is what connects the two.** It is the other
  half of `/node_set` against a surface: instead of writing a value it maps
  every control the port resolves to onto a bus. Without it a client would have
  to be told the node behind a port, which is the encapsulation the surface
  exists to keep -- and a curve drives a port of a track, of a clip, or of an
  effect three levels down. A negative bus unmaps, and unmapping is not optional:
  a port left on a bus nobody writes holds whatever was last in it, so a deleted
  curve would go on driving the control at the last value it happened to say.

  The sampling is the crate's: `nodes::plan` hands back the table itself, on the
  **frame** axis, so a tempo change bends a curve the way it bends everything
  else. Each segment is sampled with the shape its first point states in
  `Point::data` (`shape`, `curve`), through `clausters_core::envshape` -- the
  function the host draws it with. *(It was linear between break-points, on the
  grounds that `data` is opaque; a bend drawn on screen was then heard straight.
  Changed 2026-09-13, `crates/clausters-gui/PLAN.md`, "Found by use", "A bent envelope
  was drawn bent and heard straight".)* `tests/mixer_graph.rs` hears it: a table opens a track's fader over
  eight blocks, and unmapping gives the fader back to the hand.

- ✅ **The clients each drove the piece by hand, so the mixer existed twice**
  *(2026-09-10, the other half of the entry below)*. `playback` in both clients
  built reader nodes itself and worked out the mixing as it went. Now it asks:
  `clausters_multitrack_plan` for what to instantiate and `clausters_mixer_defs`
  for what to send, and what it does with the answer is **compare it against
  what is already sounding and send the difference**. Both clients run the same
  two calls, which is what makes one piece sound the same in both of them.

  A diff rather than a rebuild, and that is not an optimization: a piece plays
  itself from the transport, which only holds if the nodes stay. Rebuilding on
  every edit would restart everything sounding, and a hand dragging a box would
  hear its own gesture as a stutter. The one change a `set` cannot express -- a
  clip whose source changed *width*, which is a different wiring -- is torn down
  and made again, and nothing else is.

  Three things it turned up:

  - **`buf` did not have to be initial-rate.** `BufRd` reads its bufnum once a
    block, so it is an ordinary control and changing which buffer a box reads is
    a `/node_set`. That matters because re-cutting a join *is* a new buffer -- a
    stitched one is replaced rather than edited -- and a box rebuilt for it
    would stop and restart every time a hand moved a seam.
  - **`SegmentRef::start`'s unit was documented backwards.** It said frames
    while `picture::Box::start` said seconds, and every reader of it -- the
    picture, the host, both clients -- meant seconds. A unit stated one way and
    meant the other is the cheapest bug to write and among the more expensive to
    find: nothing about it shows until a box plays from somewhere nobody put it.
    Corrected, with the contradiction written into the comment so it is not
    re-introduced.
  - **A track's fader is read out of `config["level"]`**, which is the one key
    the crate looks inside an opaque blob for, and it is there because the
    host's own row reader already writes it there -- a second place would be a
    second answer. See the entry below.

- ✅ **A track's level should be a field, beside its width** *(named 2026-09-10
  writing the instance plan; done the same day)*. `Track::channels` became a
  field of the document because it decides the mix and both clients must write
  it identically; `level` is the same kind of thing and was a key inside
  `Track::config`, which the crate had to reach into -- the one place it read an
  opaque blob, against its own rule that configuration is carried and never
  interpreted. It is `Track::level` now, `f64`, unity by default and left out of
  the file when it is there. `picture::level_of` is the one reader and still
  answers the old key when the field is at unity, so a piece written before this
  plays at the level it was saved with; both row readers -- the crate's
  `SetTracks` path and the host's -- write the field and drop the key as they
  pass, so nothing ends up with two answers. The same field in both clients,
  since a document read in one and written in the other has to come back the
  same.

- ✅ **A piece had no node system, so `gain` on a curve and `gain` on a knob
  were two words spelled alike** *(named 2026-09-10 by the user; the defs and
  the plan landed the same day)*. `clausters_core::mixer` says what a track and
  a clip **are** on the server -- `mt.reader`, `mt.strip.<in>x<out>`,
  `mt.clip.<in>x<out>`, `mt.track.<n>`, `mt.piece.<n>` -- and what their
  surfaces answer to; `clausters_document::multitrack::nodes` says which of them
  a given piece needs, wired to which buffer, at which frame, with which level.
  Between the two nothing is left for a client to work out, which is why a piece
  will sound the same in both of them. `tests/mixer_graph.rs` hears it rather
  than reading it back: a whole piece is one `/graph_new` and everything after
  it is a slot added to what is already sounding.

  **Three things the work turned up, and all three changed the design.**

  - **The master has to contain the tracks.** A track's output is a bus, the
    master's mix bus is private to the master's instance, and a graph
    instantiated on its own could never name it. So a piece is `mt.piece` with a
    `tracks` slot, a track holds a `clips` slot, a clip holds a `source` slot of
    readers -- four levels, one `/graph_new`, and containment is what hands each
    child the bus it writes to.
  - **A GraphDef could not say which *channel* of a bus a member gets.** A UGen
    has one output, so a stereo writer is two `Out` rows and each needs its own
    bus index, which a member cannot compute (a bus input must be a control or a
    constant). Without it a multichannel private bus could be allocated and only
    its first channel ever reached -- which is to say a mixer could not be
    written. `"mix:1"` now names one, and `"OUT:1"` the hardware's second.
  - **`Balance2` is the pan law over a pair, so it is 3 dB down at centre.**
    Three strips in series -- clip, track, master -- would take 9 dB off a piece
    for nothing. A balance *attenuates one side and leaves the centre alone*, so
    the law is written in the strip as `min(1, 1 -+ pan)`. The UGen is not
    wrong; it is not a balance, and the name is scsynth's.

  Two fields joined the document with it, both additive and defaulted so every
  file written until today reads unchanged: `Track::channels` and
  `Multitrack::channels`. They are fields rather than lines in `config` because
  the width decides the mix, so reopening a piece has to give back the mix it
  was left with and both clients have to write it identically -- which is the
  definition of a field the format owns. Ported to both clients with the
  parity vectors that cover them.

  Deliberately not in it: **effects** (the slot is declared and empty -- a send
  to nowhere cannot be heard, so it cannot be checked) and **the meter** (it
  wants an instant attack, a decay in dB/s and a peak hold, and no UGen does
  that yet).

- ✅ **A GraphDef was one level of wiring, and a piece is not one level**
  *(named 2026-09-10 specifying the multitrack's node system; landed the same
  day)*. A track holds clips, a clip holds an effect chain, and each of those is
  itself a wiring with a surface of its own -- so a GraphDef whose members can
  only be single nodes describes the leaves and nothing above them. Two things
  were missing and they are the same thing twice: a member could not be another
  GraphDef, and the one kind of repetition that existed (`voice`) had no name,
  so "there are three clips right now" was not sayable at all.

  Both are in. A member takes `kind: "graph"` and is instantiated as a subgroup
  with private buses of its own, freed with its parent; a bus takes `external`
  and is provided by whoever instantiates rather than allocated, so a child does
  not decide where it goes and one def works standalone and nested; a surface
  target takes `port` instead of `control` and re-exports a child's interface,
  the two scalings composing outer-first. And `voice: bool` generalizes to
  `slot: Option<String>` with `/graph_addSlot instanceID slot id [port value]`,
  of which `/graph_newVoice` is now the `"voice"` spelling -- unchanged on the
  wire, unchanged for MIDI.

  **What this forced, and it is the better shape anyway.** Instancing is a tree,
  and a tree cannot be built the way one level was: a child that fails halfway
  leaves its siblings standing. So `/graph_new` now **plans and then realizes** --
  the whole walk happens first, resolving every def, building every synth and
  taking every bus and node id, holding enough to hand all of it back if any
  part fails; realizing the plan emits commands and cannot fail. The
  all-or-nothing rule the one-level version had is the same rule over a tree.
  Freeing is recursive for the same reason: a nested graph is a registered
  instance with buses of its own, and only external buses are never reclaimed,
  because they were never the child's.

  The nesting is of authoring and not of execution: a port resolves once, at
  instantiation, to the controls of actual nodes however many levels down they
  were written, so `/node_set` costs what it always did and the audio thread
  learns nothing. Depth is capped at 8, which is also what refuses a graph that
  contains itself -- a cycle is not otherwise detectable, since a member names a
  def and a def may name its own name. Both clients gained the same surface
  (`kind`, `slot`, `external`, a port target, `add_slot`/`addSlot`), and a
  member handle answers a control or a port according to what the member is,
  which is one spelling for both.

- ✅ **A cut assembled from several takes could not be played by one reader**
  *(named 2026-09-10 specifying the multitrack's node system; the stitched
  buffer landed the same day)*. A clip was a window onto **one** buffer, so a
  reader was a `bufnum`, a `start` and a `span`. The moment a clip is a join --
  fragments of different takes, or one take in another order, which
  `clausters_document`'s `Body::Segments` has always been able to *write* -- the
  reader would have to change which buffer it reads with sample accuracy, and
  `bufnum` is an initial-rate control: changing it is a new node. A new node per
  seam is a control message in the middle of playback, which is exactly what a
  piece that plays itself from the transport must not need.

  So the join moved out of the reader and into the buffer. `dsp::stitch`: a
  `Stitch` is a list of parts -- a span of a source, its own channel map, its
  own crossfades -- and `Storage::Stitched` makes it a buffer like any other,
  answering `Buffer::sample` and nothing else. `/buffer_stitch` builds one, the
  sources are `Arc`s cloned on the network thread (so freeing a take something
  is stitched over does not silence it), and resolving which part a frame
  belongs to allocates nothing: a shared cursor is checked first, the part after
  it second, and only a real jump pays a binary search. `tests/buffer_stitch.rs`
  and `crates/clausters-server/tests/rt_safety.rs` cover the seam, the ordering, the channel map, the
  refusal and the no-allocation rule.

  **Two things it changed on the way, both deliberate.** `Buffer::cells` now
  answers an `Option`, because a join owns no cells: that made the five callers
  who need a contiguous span (the wavetable oscillators, the waveshaper's
  transfer table, the convolution kernel, the IPC region copy) say so, rather
  than reading a silently empty slice. And a join is **read, never written** --
  `/buffer_set*`, `/buffer_fill`, `/buffer_gain`, `/buffer_reverse` and
  `/buffer_read` refuse it by name, because writing through would turn one edit
  into an edit of several takes. It takes nothing away: a join is *replaced*,
  which is what `/buffer_alloc` and `/buffer_gen` already do, and re-cutting one
  costs the list of parts rather than the samples.

  **The clients speak it** (2026-09-10): `Buffer.stitch` with a `Part` per
  span, and `parts()` over `/buffer_parts` -- the same two calls in both, since
  a command no client can send is a command that exists only in a test.

- ✅ **What a join costs, and the number that reads as alarming is the wrong
  one** *(measured 2026-09-10; the per-run read landed 2026-09-11)*. Two
  measurements, and they were taken in the wrong order the first time round.

  **The microbenchmark** (`cargo test --release --test buffer_stitch --
  --ignored --nocapture`) reads 64-frame blocks interpolated. Resolving the
  part **on every sample** measured +63% over a plain buffer, against an
  acceptance of 10% written before anything was measured. That criterion was
  the wrong measure: a microbenchmark of nothing but the load makes three added
  operations look enormous, and against the block budget the same numbers were
  0.016% and 0.026% — a hundredth of a percent of a block more, per reader.

  **The engine-level measurement is the one that decides anything** (`cargo
  test --release --test stitch_load -- --ignored --nocapture`: N `PlayBuf`
  readers in one engine, timed by `process_block`). At 128 simultaneous readers
  — more clips sounding at one instant than a piece usually has — a block cost
  **5.0% of its budget over plain buffers and 7.5% over joins**.

  **The per-run read**, taken 2026-09-11 rather than postponed further, because
  it turned out to be the shape of the thing and not an optimization bolted on
  it. `Buffer::run_at(frame)` answers the contiguous run a frame falls in — one
  part of a join, the whole of a plain buffer — and `dsp::buf`'s `Reader` holds
  it across a block, asking again only when a frame leaves it. The lookup it
  removes was **two** questions, and a run answers both once: which part holds
  the frame, and where that part's source keeps its samples (`PartRun`), so a
  read inside a run is the indexed load a plain buffer costs. The per-sample
  path stays as the fallback for the seam, a jump, a modulated rate and a phase
  running backwards — and the same run makes a **plain** buffer's read an
  indexed load too, so the fast path is uniform rather than a stitched-only
  branch.

  **What it measured.** Per read, holding the run instead of resolving per
  sample is **-50%** on a join (one part or 256, the same). In an engine at 128
  readers: plain **5.0% → 4.2%** of a block (every buffer reader in the server
  got faster), joins **7.5% → 5.3%**, so the join's overhead went from **+50%
  to +29%**. With the crossfades off it is **+16%**, which says what the rest
  is: the fade is the remaining difference, and a fade is audio somebody asked
  for rather than overhead. How many parts a join has still barely matters (the
  cursor does its job) — what a 256-part join pays is that far more of its
  samples fall inside a fade.

  **And the free rule stands, which is where the real saving is**: only stitch
  what is actually a join. A box that is one window over one take is a plain
  buffer and costs exactly that; the join is built when a hand cuts one, and
  then it pays for a capability it is using.

- ✅ **A client cannot ask whether a buffer is a join** *(found 2026-09-10
  writing `/buffer_stitch`, answered the same day)*. `/buffer_query.reply` is a
  **4-wide repeating group** per buffer, so appending a fifth field is not the
  harmless append `/server_query.reply` takes -- every parser that chunks it by
  four would read the flag as the next buffer's index. A client therefore
  learned a buffer was a join only by being refused when it wrote, which is
  honest but late: a view that would draw an editable waveform wants to know
  before it draws one. So it is a reply of its own: **`/buffer_parts bufnum`**
  -> `/buffer_parts.reply bufnum channels sampleRate [srcBufnum srcStart frames
  fadeIn fadeOut chan...]...`, the parts in the terms `/buffer_stitch` takes
  them, which a join editor wants anyway and which can be edited and sent back.
  A buffer that owns its samples answers with no parts -- that is the answer to
  the question -- and an empty slot answers `channels = 0`, on the rule
  `/buffer_query` follows. `Part` carries the number its source was stitched
  under for this and reads it nowhere else: a join is `Arc`s once it is built,
  and an index is what a client speaks.

- ✅ **A page's engine died on any ephemeral Faust def** *(found 2026-09-05 by
  driving the web examples in a browser; fixed the same day)*. `play(expr)`
  builds a def named `tmp_…`, and `/def_send faust` sends an ephemeral def's
  bitcode speed-cache to the OS temp directory rather than to the store
  (`handle_def_send_faust`, `defstore::ephemeral_dir`). On wasm
  `std::env::temp_dir()` does not return an empty answer — it **panics**, and a
  panic in a release wasm build is `unreachable`, so the whole in-tab engine
  stopped answering: the page that played one Faust box expression lost its
  server, and every later command timed out with nothing said about why. The
  cache is a speed optimization and a page has no filesystem to keep one in, so
  that branch is now `#[cfg(not(target_arch = "wasm32"))]` and a page recompiles
  the expression each time. The rule this belongs to is the standing one: the
  host and the server are the same Rust compiled twice, so a behavior that
  differs between the two builds is a defect, never the platform's quirk.

- ✅ **Three quarters of the wheel was an LLVM nobody called**
  *(found and fixed 2026-08-27, from a question about artifact weight)*. The
  wheel bundled libfaust plus the distro's monolithic `libLLVM.so` — 137 MB of
  it, every target backend and the whole toolchain support surface, where the
  JIT compiles for the machine it runs on and calls one. Linking LLVM into
  libfaust statically, from a component list rather than `--libs`, took the pair
  from 146 MiB to 43 and the wheel from 194 MiB installed to 90 (35 packed); five
  vendored dependencies (libxml2, libedit, libffi, libbsd, libmd) left the
  package with it. Those are LLVM 21's numbers, measured here; the pinned 18 --
  what ships -- is smaller still, 39 MiB of library and a wheel of 87 installed
  / 33 packed, and it keeps `libtinfo.so.6` (0.2 MiB; LLVM 19 is where the
  terminfo dependency went away).

  **Why it had not been done**: the recipe carried `LINK_LLVM_STATIC=off` with a
  comment saying the static path needs Polly, and `docs/decisions.md` argued
  static LLVM "is no lighter" because the binary and the cdylib would each embed
  a copy. Both were wrong in the same way — they were about a *different* link.
  Polly comes from Faust's `libfaustwithllvm.a` target and from asking
  llvm-config for `--libs` (everything); a component list needs neither. And
  linking LLVM into `libfaust.so` duplicates nothing: it is still one shared
  library, loaded once by both consumers. Neither claim had a measurement behind
  it, which is how a wrong one survives three years.

  **The one that was actually hidden**: upstream defines `LLVM_BUILD_UNIVERSAL`
  on every platform (`build/CMakeLists.txt:141`, outside the `if (UNIVERSAL)`
  meant to gate it), so `initJIT` called `InitializeAllTargets()` and *required*
  all twenty backends. The monolithic libLLVM was not a packaging habit, it was
  a consequence. `docs/decisions.md` ("The LLVM in the wheel is the one the JIT
  reaches") carries the four trims and their measurements;
  `third_party/build-faust.sh` explains each where it is applied.

- ✅ **`/buffer_write` fails in a page, and not for the reason the plan gives**
  *(named and fixed 2026-08-26, closing B6's step 2c)*. Both clients
  expose `Buffer.write()` and against the in-page engine it answers `/fail`.
  Two separate things are wrong.

  **The behaviour**: `NrtRunner::take_delegated` matches only
  `NrtJob::AllocRead`, so a `Write` never leaves the worklet, and there
  `write_wav` creates its file through `hound` → `std::fs`, which on
  `wasm32-unknown-unknown` is the unsupported backend. It fails cleanly rather
  than hanging, and it fails on the thread that owes the next block, which is
  the second reason it should leave: a write is one payload out and none back,
  the easiest job in the set to delegate.

  **The reason on record**: B6 said `/buffer_write` "has nowhere to write", and
  since step 3 that is false — the page has OPFS, and `diskOut` records into it
  (`tests/disk.html`, 272428 bytes of RIFF). What is missing is the delegation,
  not the destination. `clients/web/docs/src/platform.md` carried the same stale
  reason and now carries the real one.

  **Why it was invisible**: it sat as a `⬜` inside B6, which is `✅`. That is
  the exact case this section exists for, and it is worth noting that the rule
  caught a real item rather than a hypothetical one.

  **The fix, and the third thing it found.** `Write` now leaves, and its payload
  leaves **in runs** — `ServeBudget::install_frames` a serving turn, the same
  ceiling a staged load copies under and for the same measurement, since a
  one-minute take is 7 ms of copying on the thread that owes the next block. The
  Worker holds the runs and writes the file **once**, at the last one, so a
  half-written file never exists; that is what separates it from `diskOut`'s
  `record`, which rewrites at every flush because a recording has to survive the
  tab closing mid-take. Only the last run is answered, because an early
  acknowledgement arriving after the last run was posted would answer the
  command before the file existed.

  And the reason it did not work on the first try: **there were two lists of
  what is delegable**, `take_delegated`'s match and the pump's guard. A kind
  added to one and not the other does not fail — it runs in the wrong place,
  quietly, which is precisely how this command spent a release writing through a
  filesystem a page does not have. They are one function now
  (`server::nrt::delegated_kind`), read by both.

  `tests/nrt.html` is the round trip: a buffer written to the page's storage and
  read back through the same decoder at worst delta 0, and a 12000-frame take
  crossing three runs — the seam between runs being where an interleave offset
  goes wrong — also at 0.

- ✅ **Three wire selectors carry a spelling the table meant to leave behind**
  *(named 2026-08-23, once the clients' own names were settled; done
  2026-08-24)*. The operator
  vocabulary is lowercase and joined — `bitand`, `bitor`, `bitxor`, `lshift`,
  `rshift`, `midicps`, `softclip`, seventy of them — with **three exceptions**
  that carry an underscore: `as_int`, `as_float` and `hypot_apx`. It is not a
  second convention; it is a leak, and the plan says so in its own words at the
  entry that added the last of them, which describes `hypot_apx` as *"lowercased
  like every other name in that table, as `as_int` already is"*. The intent was
  stated and the opposite shipped.

  **The client half is already done**, which is what leaves this visible and
  small: both clients now spell every operator SuperCollider's way
  (`asinteger`, `asfloat`, `hypotapx`), and each maps its name to the selector
  in one line. So only the wire lags, and the rename is `asint`/`asfloat`/
  `hypotapx` — or `asinteger`, if the selector should read as the method does.

  **What it costs is a format change, which is why it was deferred rather than
  taken with the client rename.** A stored SynthDef carries the selector, so
  every persisted def using one of the three stops loading; `docs/schemas.md`
  lists them; the frozen parity vectors freeze them; and the two Faust `_UNARY`
  tables key on them. The compatible shape is to **accept both and emit the
  new** for a release — `UnaryOp::from_name` already resolves by a table, so an
  alias arm is small — and the incompatible one is to take the break while the
  major is `0`. Neither is decided here.

  **Decided and done: `asint`, `asfloat`, `hypotapx`, with the old spellings
  accepted for good.** The short forms rather than the methods' names, because
  the wire is not the client's vocabulary and never was — `recip` and `lshift`
  cross as they are while a user types `reciprocal` and `leftshift`, so
  "lowercase and joined" is the whole rule the three were breaking, and
  `asinteger` would have been a second decision smuggled in behind a fix.

  The compatible shape, and **without a schedule attached**: `from_name`
  resolves `as_int`, `as_float` and `hypot_apx`, `name()` never emits them, and
  that tolerance is permanent. It is three lines on the input side of one table
  — it is not a deprecation with a removal date, so nothing is postponed by it,
  and it is what makes renaming a name that lives *inside stored documents*
  cost nothing: a def written by an older release goes on loading.

  **No version number moved**, and that was checked rather than assumed. The
  two ABI counters read discriminants and not names, so neither boundary
  changed; and the SemVer tier had already moved twice since `v0.8.1` (the tree
  said `0.10.0` then), so under the release rule a further breaking change rode
  the bump that was already there. `docs/decisions.md` carries the amendment, since
  the paragraph that said renaming these "would be a format change for a
  cosmetic gain" is the sentence the three underscores had been hiding behind.

- ✅ **Every carrier takes an address, and all three default to loopback**
  *(named 2026-08-23, deciding whether a GUI host may be remote; done
  2026-08-24)*. The two OSC
  fronts are siblings and bind differently, and neither is quite right:

  | leg | audio server | GUI host |
  | --- | --- | --- |
  | UDP | `127.0.0.1`, always on | `127.0.0.1`, always on |
  | TCP | `--tcp <port>` → **`0.0.0.0`** | on by default → `127.0.0.1` |
  | WS | `--ws <port>` → **`0.0.0.0`** | `--ws <port>` → **`0.0.0.0`** |

  Choosing a carrier is not consenting to the network, and today it is: a
  `--tcp` or a `--ws` for a client on the same machine opens the port to the
  LAN. The fix is one shape for all three — `--tcp [addr:]port`,
  `--ws [addr:]port`, **loopback by default**, and `--ws 0.0.0.0:57120` for
  whoever means the LAN, which makes the widening a typed decision instead of a
  side effect. The WebSocket leg is not an exception: a page served locally
  reaches `ws://127.0.0.1` either way, so the common case pays nothing.

  The GUI host's TCP is the one that must *stay* loopback whatever else changes
  — it is on by default, so it is the leg a default-configured host exposes.

  Remote is then something you ask for, which is where it belongs: two things
  degrade under it and neither is a bind — a `Source`'s `path` carrier (the
  client writes a temp file the host mmaps, so one filesystem is assumed, and
  remote means inline-only under the 2 KB ceiling) and `--data-dir` (the GuiDef
  store is the host's). `GuiHost.attach()` is unaffected: it is a verb of
  *ownership*, not of distance, and its real case is local — a host left
  running, one launched from a terminal.

  **Done, and it was six legs rather than the four the table named.** The audio
  server's TCP is *also* on by default, so a plain `clausters` was already
  listening on `0.0.0.0:57110` with nobody having asked for anything; that is
  the one this fixes that the entry had not noticed. All six now take the same
  spelling — `--udp [addr:]port`, `--tcp [[addr:]port]`, `--ws [[addr:]port]` on
  both programs — read by one parser in the shared core
  (`config::PortChoice::parse`): a bare port, an address alone (the port still
  follows the base), or both, IPv6 included. **Loopback is the default in all
  six**, and `--ws 0.0.0.0:57120` is what says the network out loud. The config
  keys take the same string (`tcp = "0.0.0.0:57110"`), beside the
  `true`/`false`/port they already took, and the Python client's
  `ServerOptions(tcp=…, ws=…)` passes one through.

  Two things came out of doing it. The bind used to be written **inside each
  leg's own module** — `bind_tcp` said `"127.0.0.1"`, `bind_ws` said
  `"0.0.0.0"`, the windowed front repeated both — which is exactly how one front
  came to differ from the other with nobody choosing it; they take a
  `SocketAddr` now and the address is settled once, where the command line is
  read. And a token after `--ws` that is not a bind used to fall through as "no
  argument" and resurface as `unknown argument: 0.0.0.0:57121`, so a typo
  silently left the leg on its default; it is now an error against the flag that
  owns it. The rationale is in `docs/decisions.md` ("Choosing a carrier is not
  consenting to the network"); watched by hand with `ss` over all six legs,
  default and widened.

- ✅ **A transport addressed in samples still needs a beat grid** *(noticed 2026-08-16 while closing T5, which left it open on purpose; fixed the same day, on the user's "look at the smell and find a solution")*. `/transport_play`, `/transport_stop`, `/transport_locateSample` and `/transport_loop` all refused until `/transport_set` had defined a grid, because every `/transport_*` command but that one always had. For an audio editor that meant declaring a tempo it never reads — `crates/clausters-server/examples/transport_seek.py` set one and said in a comment that it was arbitrary, which is exactly what a smell is: a comment apologising for a call.

  **The fix is that the transport stops being optional and the grid starts being.** `OscServer.transport` was an `Option<Transport>` doing two jobs — "is there a transport" and "is there a grid" — and those are different questions the moment a position exists in samples. It is now a plain `Transport` that exists from boot, with `defined` saying whether `origin_sample`/`tempo` mean anything; `defined` is the wire field of the same name, so the reply did not change shape and no client had to learn a new one. Only the two commands that name a **beat** refuse without a grid: `/transport_locate`, and `/transport_play` *given a position*. Everything else — a bare play, a stop, `/transport_group`, `/transport_locateSample`, `/transport_loop`, and `/sched_atTransport`, which needed a bound group and not a tempo — is in samples and now needs nothing.

  **What it moved on the client side, which is the part worth checking twice.** `transport_state()`/`transportState()` used to answer `None` when no grid was defined — hiding the rolling state and the position, which is precisely the state this makes reachable. They now always answer, with the grid fields `None`/`null`. That reaches one caller in each client, `Playhead.follow_transport`, and the guard there moves from *the state exists* to **the grid exists**: a playhead runs on beats, and applying a `position` of 0 to a transport being driven in samples would locate it to the start. `transport()`/`transport()` — the grid getters — still answer `None` and did not change.

  **The example is the proof**: it no longer calls `set_transport` at all, and it binds a group, seeks by frame, loops a span, plays and stops with no tempo anywhere in the file.

- ✅ **`--shm` only ever creates a segment, so two servers cannot share one** *(found 2026-08-17, answering the user's question about how the processes are arranged; fixed the same day)*. `clausters --shm <path>` calls `Segment::create_full`, which **truncates** the file and re-initializes the header. That was right while a segment was a transport — one server, its own ring — and it is wrong now that the segment indexes the **material** (S19): the process that may be killed and restarted is the one that would wipe what everybody else is editing.

  What is missing is the other half of the door: `--shm` that **opens** an existing segment, validating magic and ABI exactly as a peer's `Segment::open` already does, and creating one only when there is none.

  **Done.** `--shm` opens what is there and creates only what is not, and the segment carries a **claim** — the pid serving its command plane, in the word that already kept `transport_clock` aligned, so no offset moved. The first server on a segment takes it, serves the rings and owns the material; any later one attaches to the data plane, maps what the owner published, publishes nothing of its own, and serves its clients over its own sockets. A claim whose pid no longer answers is stale and is taken over, which is what makes killing a server recoverable. The rule and its reasoning are in `docs/decisions.md` ("One segment has one owner"); the acceptance is `crates/clausters-server/examples/editor_processes.sh`, which kills the player and starts another against the same segment.

  **And the routing half, which no shared memory could fix, shipped with it.** The server names what it holds and what it is called: `--host`, `--device`, `--input-device`, `--list-devices` and `--client-name`. That last one is the one that matters here — cpal names its PipeWire node `cpal-playback-<pid>`, a different name every run, so every connection a person made was lost on restart; PipeWire reads its properties from the environment, which is the only door cpal leaves open, and under JACK the client name comes from the device instead. Verified by hand: a server started with `--client-name clausters-rt` shows up in `pw-dump` as `clausters-rt`.

  **What attaching does *not* restore, and it is the reason this is not merely plumbing** *(the user's, 2026-08-17)*: an audio editor keeps its input and output devices, and under JACK or PipeWire the ports it publishes and the connections a person made to them live with the **process**. A server that attaches to a segment gets the material back and gets no port, no name and no patch — so restarting one is *recovery*, not a routine, and a design that treats an RT server as disposable is a design that drops the user's routing every time. Two consequences worth naming before they are discovered by ear: a recovery is only cheap if the ports return **with the same names** (today the device is `cpal`'s default and nothing names anything, which is fine for one process and not for one expected to come back), and **capture** belongs to whoever holds the input device — so an on-demand session cannot record, and the split is not "RT plays, on-demand works" but *RT owns the devices*. With it the arrangement H6 wants becomes expressible — the editor (or the on-demand session it opens) creates the segment and owns the material, and every server *attaches* — and its acceptance stops being a diagram: killing the RT server and starting another against the same segment leaves the material where it was.

- ✅ **The editor's server is supposed to be the on-demand one, and the host boots a full RT server instead** *(found 2026-08-17 by the user, reading S19's wiring against what the plans already say: "I thought the documentation and the plans had settled that this file editing was done with the NRT server in on-demand mode")*. It had. **S12** states it as a settled decision — *"the performer is the NRT server (S13): a client drives that mode, it does not call the core functions itself"*, which is what keeps *one place performs audio processing and it is the server* true — and S13 built the mode. What nobody checked since is the practice: the `standalone` host boots `clausters::embed::Clausters::open`, which starts a **full server: audio device, engine and ring**. So every destructive edit H3/H4 make is performed by an RT server that also happens to be playing, and the on-demand session — the thing designed for exactly this — is reachable only from Rust that constructs an `NrtSession` directly.

  **It is not a mistake anyone made; it is a door nobody built.** S13 said so on the way out: *"a client still cannot reach a session, so the builders and the example wait for whatever gives one a door"*. The host used the only embed there was.

  **What it costs, stated so the fix is not sold as tidiness.** An editor that owns its material through an RT server holds an audio device it does not need, cannot be restarted without taking the material with it, and pays the RT server's whole surface to run three verbs. And the two servers a finished editor wants — one that renders and edits on demand, one that plays and can be killed — are the same process, which is precisely what H6 exists to separate.

  **The fix is the embed's door, and it is H6's**: `Clausters` gains a way to open a **session** (no device, `enable_offline_renders`, a segment on a path) beside the way it opens a server. `SessionConfig::shm` landed with S19 and is the half that already exists.

  **Done 2026-08-17 with H6.** `embed::ClaustersSession` is that door — engine, network loop and material, no device, driven with the same `send`/`poll` a caller already knows — and `--session` opens one instead of a full server. One thing had to move with it: a session no longer publishes **time** into the segment (the clocks, the taps, the per-bus levels), because those say where a *device* is and a session has none. With both engines on one segment, which is the arrangement, a session that published would jog the playhead every time somebody applied a fade.

- ✅ **A persisted def that no longer compiles warns on every boot, forever** *(found 2026-08-16 closing S17: changing `PlayBuf`'s arity from 4 to 7 made every stored def that used it unloadable, and the server has printed seven `persisted SynthDef failed to load` warnings at every start since)*. `attach_store` recompiles each stored spec and, on failure, warns and skips it (`crates/clausters-server/src/osc/server/lifecycle.rs`) — the right call at load time, since a def that fails today may be a build missing a feature rather than a def that is wrong. But nothing ever prunes it, so a UGen signature change leaves permanent noise in a place where warnings are supposed to mean something, and the person who sees it has no idea which def or how to clear it. The fix is not to delete on failure (a `--no-default-features` boot would eat the library); it is to say **which def**, name the store path, and offer one way to drop the dead ones — a flag, or a `/def_*` command that prunes what will not load.

  **Fixed 2026-08-18, and the shape is the entry's own.** A failed reload now names the def (its file stem, which is the name it was stored under) and, once per boot, how many did not load, which directory they are in and the one command that clears them. That command is `--prune-defs`: a server flag rather than a `/def_*` command, because the person who has to act on the warning is reading a terminal at boot and not driving a client. Deleting on failure stays refused for the reason the entry gives — a def whose family this build lacks fails for the build's reason and not its own — so pruning only ever removes what the *running* build tried and could not compile: the synchronous families as they fail (`retire_dead_defs`), a Faust def when its reload result comes back. `load_synthdef_specs`/`load_graphdef_specs` hand back the path with the bytes, which is what makes a def nameable at all. The acceptance is `crates/clausters-server/tests/persistence.rs::a_def_that_no_longer_loads_is_named_and_dropped_only_on_request` (a plain boot keeps the dead def, a pruning boot takes it and leaves the live one), and it was watched by hand over a store with one stale def.

- ✅ **`transport_group` takes an id where the rest of the Python client takes a node** *(found 2026-08-16 writing T5's example: `server.transport_group(monitor)` raises `TypeError: int() argument must be ... not 'Group'`, and the call has to be spelled `monitor.id`)*. Everywhere else a group is passed as the object — `Synth(..., target=monitor)`, `Group(target=src)` — so the one place that wants the bare integer is the one a reader gets wrong, and it fails at the client rather than at the server. The TypeScript client already has `nodeId(...)` for exactly this coercion, so the fix is the same shape on the Python side and is a couple of lines; it is recorded rather than folded into T5 because a client-API coercion is not a transport decision and should not ride in on one.

  **Fixed 2026-08-18.** `transport_group` coerces with `defs.node._target_id`, the helper `target=` already used, so the object and the bare id are the same call and the coercion lives in one place rather than two. The two examples and the book's transport chapter drop the `.id` they had to spell; the acceptance is a unit test over the fake interface (`test_defs.py::test_the_governed_group_is_a_node_or_its_id`). The web client needed nothing: it has coerced with `nodeId(...)` since it was written.

- ✅ **A sample write costs the whole buffer, not the samples written**
  *(noticed 2026-08-14, costing the message-passing design against a monolithic
  audio editor before opening the document crate's O4)*. `/buffer_setRange`
  lays its runs into a **copy that replaces the buffer whole**
  (`crates/clausters-server/src/osc/server/commands/buffer.rs`), which is what lets the engine read
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

  **Reopened 2026-08-17 by the user, and this time the entry's original title is
  the right one after all.** *"A buffer stays immutable"* was true when S12 wrote
  it and false the next day: **S14** made every sample an atomic cell any thread
  may write, and `BufWr`/`RecordBuf` write in place from the audio thread. So the
  answer above rests on a premise that no longer holds, and what was a cost
  argument is now also a correctness one — a copy-and-swap write silently
  discards whatever an in-place writer put there since the snapshot. Measured
  again (the table is in **S18**, which is where the fix lives): a 1 ms run costs
  0.0001 ms in place against 65.7 ms by copy on a ten-minute take, and even the
  whole-buffer case is 3.1x faster in place. The lasting finding recorded above —
  *editing never goes through the pool* — is a statement about the **editor's**
  architecture and survives untouched; what does not survive is the reason the
  write path copies. **S19** is the other half the user named the same day: with
  the samples in the shared segment a local peer's edit costs no message at all.

  **Fixed 2026-08-17 by S18**, which is where the whole record of the fix lives:
  `Set`, `Fill` and `Edit` write through S14's cells and install nothing, so the
  cost is flat in the material (3.3-3.5 ms through the wire on takes from ten
  seconds to ten minutes, and that is the `/server_sync` round trip, not the
  write). The correctness half went with it — a copy-and-swap no longer discards
  what a `RecordBuf` wrote since the snapshot. The workaround this entry named,
  a client-side working copy pushed on confirmation, is therefore no longer
  forced by the write path, and the document crate has since settled that it is
  not wanted either (`crates/clausters-document/PLAN.md`, Open decisions, "Does
  the working copy still lead, now that a write costs the span?" - taken: a
  loaded take is edited where it lies, and a copy stays mandatory only for
  material reached by reference to the user's own file).

- ✅ **A finished async command waits up to 100 ms to be reported** *(found
  2026-08-15, measuring the write cost above: every single write round-tripped
  in ~104 ms whatever its size, which is not a cost any buffer work explains)*.
  The network loop blocks in `recv_from` under a 100 ms read timeout
  (`GC_INTERVAL`, `crates/clausters-server/src/osc/server/lifecycle.rs`) and collects NRT and Faust
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

  **Fixed 2026-08-18 with the wakeup, and the tick left where it was.** The NRT
  runner and the Faust compiler take a `Waker` (`crates/clausters-server/src/osc/wake.rs`) — the same
  zero-length datagram to the server's own address that the TCP, WebSocket and
  MIDI readers already sent, packaged so a worker thread carries the socket
  rather than the address — and poke it *after* queueing a result, so the woken
  loop finds it. One thing had to move with it, and it is the part that would
  have made the fix look like it did nothing: the loop's zero-length branch
  `continue`d straight back to the top, which drains the carriers but collects
  nothing, so the wake arrived and the result still waited. The three collectors
  are now one `collect_async()` called from every path out of the recv — a
  packet, an idle tick, a wake. `GC_INTERVAL` is untouched and stays what it was
  written to be: housekeeping without traffic. The acceptance is a timed round
  trip with nothing else talking to the server
  (`crates/clausters-server/tests/buffers.rs::a_finished_job_is_reported_without_waiting_for_the_idle_tick`),
  which measured 104 ms before and under 1 ms after.

- ✅ **A node notification waits up to 100 ms on a quiet connection, and the
  audio thread cannot wake anybody** *(found 2026-08-20, chasing what looked
  like a flaky web test: `seq-ws.test.ts`'s "two clocks join one grid and land
  on the same bar" failed once with the two notes 102 ms apart and passed in
  the twenty-five runs after it)*. It is the entry above, in the one place the
  wakeup did not reach. `/node_start`/`/node_end` are posted by the **audio
  thread** into the event queue and drained in `collect_garbage`, which the
  network loop calls on every path out of `recv_from` — so with no other
  traffic a notification waits for the 100 ms `GC_INTERVAL` tick, exactly as an
  NRT result used to.

  **Measured, not inferred**: over a `--ws` connection with nothing else
  talking, `/node_start` arrives **103–112 ms** after the `/synth_new` that
  caused it, eight times out of eight. It is not a flake and it is not load —
  it is the tick, every time. What made it *look* like a flake is that the test
  compares two notifications with each other: both are late by the same tick,
  so the difference is 0 ms until the two happen to straddle a tick boundary,
  and then it is one whole tick. The 0.05 s tolerance is fine; the test was
  reporting a real 100 ms.

  **The fix is not the same fix**, which is why this is written down instead of
  folded into the entry above. That one added a `Waker` the worker thread pokes
  after queueing its result — and the producer here is the **audio thread**,
  which may not send a datagram, take a lock or do anything else. So promptness
  has to be bought with a poll rate rather than a wake: `retune_timeout`
  already narrows the loop's timeout to the fastest subscribed stream period,
  and the natural shape is another term — while any client has `/server_notify`
  on, the tick is the notification bound (10 ms, say) rather than
  `GC_INTERVAL`. That is a cost paid only by connections that asked for
  notifications, and it wants a number chosen deliberately: what a client
  watching the node tree is entitled to expect. The alternative — a non-RT
  helper that watches the queue — is the same poll one thread further away.

  Worth knowing before picking the number: **everything that waits on a
  notification pays this**, including a client reconciling node ids off
  `/node_end`, a GUI host's node-tree view, and any test that times a note by
  when it was told about it.

  **Fixed the same day, with the poll rate and the number already in the
  file.** `NOTIFY_INTERVAL` is `MIN_STREAM_PERIOD` — 10 ms — and
  `retune_timeout` now takes it as one more term beside the stream periods,
  armed while `self.clients` is non-empty and handed back when the last
  registration goes (`/server_notify 0`, or a dropped TCP/WS connection, which
  `prune_disconnected` already retunes for). Choosing rather than inventing the
  number is the whole of the design here: a client that asked to be told about
  the **node tree** is entitled to the same cadence as one that asked to be
  told about a **bus**, and that cadence was decided once already. A server
  nobody is listening to still falls back to `GC_INTERVAL`, which stays what it
  was written to be.

  **Measured before and after, end to end**: `/node_start` over a `--ws`
  connection with nothing else talking went from **103–115 ms** to **12–27 ms**
  (the tick, a block, and the socket hop). The acceptance is
  `crates/clausters-server/tests/osc.rs::a_node_notification_is_reported_without_waiting_for_the_idle_tick`,
  which reads 104 ms with the term removed and a few ms with it — **verified by
  mutation, not by passing**.

  Two things the test had to get right, both of them the reason the defect
  survived this long. It **sends the server nothing** while a notification is
  outstanding: the other notify tests call `tick_until`, which nudges with
  `/server_status` between blocks, and every nudge is a packet that wakes the
  loop — the bug was hiding behind the harness. And it **times the second
  event, not the first**: the trip from the socket to the engine is not what is
  being measured and is not fixed under a parallel harness, so the
  `/synth_new`/`/node_start` pass is a warm-up and the clock runs over
  `/node_free`/`/node_end`. The first version of it timed the first event and
  failed about one run in six on a loaded machine — a flaky test written while
  fixing a defect that presented as a flaky test.

- ✅ **A wall-clock timetag is placed a block late or early, so a routine's
  spacing jitters by one device block** *(found 2026-09-16 while checking a
  report that a routine's first note sounded shorter -- it did not: the output
  was recorded with `RecordBuf` and every note, the first included, is 329.6 ms
  long)*. The same recording showed the onsets. A `TempoClock(2)` routine
  yielding one beat, on the default monotonic timebase, lands its notes
  **490 to 513 ms** apart; the same routine after `lock_to(server)` lands them
  **500.0 ms** apart, every one. The spread is one device block (1024 frames,
  21.3 ms at 48 kHz).

  The cause is in `OscServer::schedule_bundle` (`crates/clausters-server/src/osc/server/dispatch.rs`):
  the target sample is `current_samples() + delta * sample_rate`, where `delta`
  is the timetag minus the wall clock **now** and `current_samples()` is the
  counter **as of the last completed block**. The two terms are read at
  different instants, and the gap between them -- how far into the current
  block the packet arrived -- is anywhere from zero to a block, differently for
  every bundle. A note-on and its release usually arrive together and share the
  error, which is why lengths stay exact while onsets do not.

  The Python book says the opposite: `timing-models.md` promises wall-clock OSC
  time "jitter-free *relative* timing". That holds for the timetags the client
  sends, and this server throws it away on arrival. The fix belongs in the
  server, not in the promise: place a timetag against an estimate of the wall
  instant the counter was read at (the `/clock_query` anchor already models
  exactly that pair), rather than against the moment the packet was handled.
  Acceptance is an E2E that schedules bundles at a fixed spacing over UDP and
  reads the onsets back sample-exact.

  **Fixed the same day, in the server.** The audio callback stamps the frame it
  is about to hand the device with the system clock and publishes the lower
  edge of `stamp - frame / rate` -- the **device epoch**, the Unix instant sample
  0 falls on (`server::device_epoch`). The lower edge because a stamp is only
  ever late (the callback thread's wake-up), tracked with a 1000 ppm rise so a
  crystal slower than the system clock is followed, and restarted from a stamp
  more than three callbacks late (an xrun, a restarted stream, a stepped clock).
  `schedule_bundle` places a timetag at `(T - epoch) * rate`, never before the
  counter; `/clock_query.reply` anchors on the same line, so a wall-clock
  client joining a transport maps the grid through the same numbers the bundles
  land by. A host that drives the engine itself publishes no epoch and keeps
  the counter-plus-delta placement, which is exact there: its time is the
  counter. Nothing changed in either client -- the timing is the server's.

  **Measured on the device**, the same recording: the monotonic-timebase
  routine now lands its notes **499.9 to 500.3 ms** apart (was 490 to 513); the
  sample-locked one still reads 500.0. The acceptance is
  `crates/clausters-server/tests/scheduling.rs::wall_clock_timetags_land_on_the_device_line_whenever_they_arrive`:
  two notes a second apart sent 7 ms apart with the engine not moving between
  them, onsets read back 48 000 samples apart. With the placement reverted it
  reads 47 661 -- **verified by mutation**.

- ✅ **`boot()` returns about 100 ms before the device's first callback**
  *(found 2026-09-16, fixing the entry above)*. A server launched from Python
  answers `/clock_query` for roughly a tenth of a second with its counter at 0
  and no device epoch published: the port is up, the stream is not playing yet.
  A bundle handled in that window can only be placed against a counter that is
  not moving, so the first note of `Server().boot()` followed at once by a
  routine landed about 80 ms off the line the later notes sit on. It may also
  be what a report heard as a first note "shorter" than the rest -- at the
  engine's output that note is identical, so whatever cuts it happens after the
  engine, and a device that has only just started is the likeliest place. Not
  verified by ear. The question is whether readiness should mean the stream is
  running (the first callback published an epoch) rather than the socket
  answering, which is a launcher contract both clients read.

  **Decided and fixed the same day: ready means sounding.** `boot` blocks by
  default, and the case it exists for is a note played on the next line.
  `backend::start` now waits, after `stream.play()`, until the device has called
  back twice -- the first callback is late on the stream's own start, and the
  epoch it seeds settles on the next -- and warns and serves anyway after two
  seconds of a device that never calls back. The OSC front is bound after
  `start` returns, so every launcher that waits on the port (the Python one, the
  web client's node scripts, a terminal, `embed`) reads readiness as sounding
  with no change of its own. Measured: the first `/clock_query` after `boot()`
  already reads the settled epoch, and the routine's first note now lands
  **500.0 ms** before the second (was about 420).

- ✅ **The npm half of a release has never run before the tag that publishes
  it** *(found 2026-09-21, reviewing the tree)*. The
  dry run (`gh workflow run release.yml`) proves `verify` and the wheel's
  `build`, and skips every job guarded by `github.event_name == 'push'` — and
  `publish-npm` is where the whole web package is **built**, not only where it
  is published: `.github/actions/wasm-vendor` (the emsdk builds of the Faust
  compiler and of verovio, added after `v0.8.1`), `build.sh` and `npm run
  check-package`. CI's `web` job runs the smokes without the vendor step. So
  that path has never run on Actions, and its first run is the one-way one. A
  failure there publishes nothing — `publish-pypi` needs `publish-npm` — but
  leaves a tag to delete and re-cut, which is what the dry run exists to
  prevent. Locally the package is fine today (`build.sh`, `check-package`
  "publishable", `test.sh` 1158/1158 and the 24 pages), so what is unproved is
  the runner's path, not the package.
  **To do:** split the build out of `publish-npm` into a `build-web` job with
  no event guard (vendor, `build.sh`, `check-package`, `npm pack`, the tarball
  uploaded as an artifact, as `build` does for the wheel), and let
  `publish-npm` publish that tarball. The dry run then packages both halves.

  **Done in the tree 2026-09-21, and open until a dry run is watched.**
  `build-web` has no event guard and uploads `npm-package` (the `npm pack`
  tarball, whose file list lands in the log); `publish-npm` checks nothing out,
  downloads it and runs `npm publish ./clausters-*.tgz --provenance`; the
  release page downloads the wheel and the server binary by name, so the
  tarball is not attached to it. `docs/contributing.md`, `clients/web/BUILD.md`
  and the `release-versioning` skill say what the dry run now runs — two of the
  three still said it ran `verify` alone, which had been wrong since the wheel
  joined it. What is left is the proof: a `gh workflow run release.yml` on a
  pushed `main`, watched through `build-web`, with the wasm-vendor composite
  building on a cold cache for the first time.

  **Proved 2026-09-26, and the first dry run found what this entry feared.**
  The wasm-vendor composite failed on its first cold run:
  `build-faust-wasm.sh` configured cmake without the backend and target lists
  upstream's `make wasmlib` passes on a fresh tree, so `WASM_BACKEND` was
  empty and the bindings stopped at `wasm_dynamic_dsp_aux.hh` not found. The
  maintainer's clone had passed for weeks because its `CMakeCache.txt` still
  held the lists from a configure through Faust's Makefile. The recipe now
  passes `build/backends/regular.cmake` and `build/targets/regular.cmake`; a
  fresh clone of the pin builds locally and the web suite passes with its
  artifact, and the next dry run went green through `build-web` (verovio built
  cold too, `check-package` "publishable", an 11.1 MB tarball of 674 files).

- ✅ **Milestone labels and a retired verb are still in published code and
  docs** *(found 2026-09-21, the same review)*. Labels, which mean nothing to a
  reader: `clients/web/src/defs/server/index.ts:1` (W0),
  `clients/web/src/gui/page.ts:195` (W4), `docs/architecture.md:1225`
  (W26/W27), `docs/bindings.md:273` (O31), and in the GUI host
  `host/elements/multitrack/mod.rs:10`, `host/graphics/multitrack.rs:14`,
  `host/widget/build.rs:82` (G34) and `host/widget/reconcile/tests.rs:65`
  (O23). "Realize" for making a join's buffer:
  `clients/python/clausters/_native.py:1426`,
  `clients/python/clausters/gui/editing/multitrack.py:199,230`,
  `clients/web/src/document.ts:572`,
  `clients/web/src/gui/editing/multitrack.ts:263,324` and the
  `clausters_editing_stitch` row of `docs/bindings.md` — the buffer is
  **built**. (The "realizes" in `docs/architecture.md`'s MIDI lines and in
  `docs/schemas.md`'s filter note are neither of these and can go with the
  same pass.)

  **Fixed 2026-09-21.** The labels are gone: a comment that pointed at a
  milestone for its argument points at the plan section by its title, and the
  rest lost a coordinate that said nothing. The join's buffer is **built**
  in all seven places, and the other four "realizes" (the MIDI actuation twice
  in `docs/architecture.md`, a MIDI voice and scsynth's filters in
  `docs/schemas.md`) say **translates** and **implements**.

The entries below are one audit of the server and the shared crates
*(2026-09-25, asked by the user after the GUI host's)*: `src/` and
`crates/*`, read for defects, redundancy and fixes narrower than their rule,
with a near-duplicate scan over every function and a scan for doc comments
fused onto the wrong item. Defects first, then what is duplicated, then
structure. What is marked *to check* is a reading of the code that a test
should confirm before the fix.

- ✅ **A page and a native server write different WAV bytes** *(audit
  2026-09-25)*. Four WAV writers quantize an integer sample: `nrt::write_wav`,
  `render`'s file writer and `DiskOut`'s thread scale and **round**; the page's
  `/buffer_write` (`nrt::encode_wav_frames`, through `clausters-nrt-web`)
  **truncates** (`as i16`, and 24-bit by `8_388_607.0` without a round). The
  same write differs by up to one LSB in a browser. One quantizer and one frame
  encoder for all four, and a test pinning the bytes of an int16 and an int24
  write.
  **Fixed**: `nrt::quantize` is the one quantizer (clamp, scale, round) and
  `nrt::write_wav_sample` the one sample writer; `write_wav`, `render_to_wav`
  and `DiskOut` write through the second and `encode_wav_frames` through the
  first. `a_page_encodes_the_int_bytes_a_native_write_writes` pins both
  widths at the half-step values where the two quantizers parted.

- ✅ **An NRT score refuses two buffer commands the live server takes**
  *(audit 2026-09-25, to check)*. `server::render` keeps its own list of the
  `/buffer_*` job addresses and lacks `/buffer_setChannel` and
  `/buffer_setRangeChannel`, which `parse_buffer_msg` and the live dispatch
  accept; offline they fall to the translator and fail as unschedulable. The
  set of job addresses is one list the dispatch table and the renderer both
  read.
  **Fixed**, confirmed first by a score that failed with `/buffer_setChannel
  cannot be scheduled in a timed bundle`: `translate::BUFFER_JOBS` is the one
  list, the renderer matches on it, and two tests hold the command table to it
  in both directions (a job with no row, a row the parse takes that the list
  lacks).

- ✅ **`clausters --nrt` takes any unknown argument for a path** *(audit
  2026-09-25)*. `nrt_main` pushes every argument it does not know into the
  paths, so `--help` after a score renders a WAV named `--help` — the stray
  file that once reached `main` — and a misspelled flag becomes a file. An
  unknown `-`-prefixed argument refuses with the usage, and `--help` prints it.
  **Fixed**: `nrt_main` refuses an unknown `-`-prefixed argument before it
  reads the score, and `--help`/`-h` anywhere prints the usage and renders
  nothing; two tests in `main.rs` hold both.

- ✅ **A transport command can answer `/done` for a change the engine never
  got** *(audit 2026-09-25)*. `/transport_play`, `_stop`, `_loop`, `_end`,
  `_fade` and the locates send their engine command with `.ok()`: a full
  command FIFO leaves the network mirror saying one thing, the engine another,
  and the client told it worked. `_follow` and `_group` fail instead. One rule:
  a command whose engine half did not go out fails.
  **Fixed**: every transport handler sends through `send_engine`, which fails
  the command on a full FIFO, and sends **before** it changes its mirror, so a
  failure leaves the mirror, the engine and the client's `/fail` agreeing that
  nothing changed (`_follow` and `_group` used to fail with the mirror already
  changed). `a_transport_command_the_engine_never_got_fails` fills the FIFO
  and checks both the `/fail` and the query. The two unbinds
  `collect_garbage` sends when a governed or followed group is freed stay
  best-effort: no client asked for them, so there is nobody to fail to.

- ✅ **`/def_load` does not claim the def's name** *(audit 2026-09-25, to
  check)*. `/def_send synth` frees the name in the other def kinds
  (`claim_def_name`); `/def_load` and `/def_loadDir` do not, so a file loaded
  under a GraphDef's or a FaustDef's name leaves both — what `claim_def_name`
  exists to prevent — and they persist an ephemeral name without asking.
  **Fixed**, confirmed first by a listing that named the def twice:
  `install_synthdef` is the one install (compile, claim, persist unless
  ephemeral) that `/def_send synth` and the file loads share.
  `d_load_claims_the_name_from_a_graphdef` counts the name in the listing — a
  query by name answers the first kind found and could not see the second.

- ✅ **`/node_before` and `/node_after` move by a rule of their own** *(audit
  2026-09-25)*. They inline the move instead of `move_one`, and refuse when
  the node's or the target's *current* parent is auto-sorted, where
  `/node_order` and `/group_head` refuse only an auto-sorted *destination*.
  One move, one rule.
  **Fixed**: both go through `move_one`, so every move verb refuses an
  auto-sorted destination and allows taking a node out of one.
  `a_node_leaves_an_auto_group_by_any_move` holds it; `docs/auto-order.md`
  and `docs/schemas.md` now state the destination rule.

- ✅ **`install_buffer` says it is the install path and is not** *(audit
  2026-09-25)*. Its doc and `ClaustersHeadless::buffer_load`'s say "the same
  install path as the async `/buffer_*` commands", but it skips
  `share_buffer`: on a server that owns a segment the buffer would get no
  region, row or overview. No caller reaches that today; the doc is what is
  false, and the two install paths should be one with a flag for the mapped
  peer's case.
  **Fixed, and it was not latent**: `NrtSession::perform_render` — the body
  of `/buffer_render` — installs through it, so on a session given a `shm`
  path a peer kept mapping the allocation the render had replaced (a test saw
  16 frames of one channel where 256 of two were rendered). `install_buffer`
  now calls `share_buffer` and the finished-job arm installs through it, so
  there is one path; no flag was needed, since `share_buffer` already hands a
  buffer back untouched on a server that does not own the samples, which is
  the mapped peer's case. `a_render_on_a_session_given_a_path_is_what_a_peer_maps`
  holds it. The doc of `collect_nrt_results`, fused onto `install_buffer`,
  went back to its function.

- ✅ **Twenty-odd doc comments sit on the wrong item, and labels sit in
  comments** *(audit 2026-09-25)*. Fused docs: `osc/server/mod.rs`
  (`synthdef_spec_bytes`' on `ugen_infos`, two field docs on `translator`),
  `async_pipes.rs` (`collect_nrt_results`' on `install_buffer`),
  `osc/translate/graph.rs` (`apply_surface`'s on `graph_map`),
  `server/engine.rs` (on `silence_time_publication`), `clausters-editing`
  `multitrack.rs` (the boxes' on `segments`), `clausters-document`
  `multitrack/picture.rs` and `clausters-core` `warp.rs` (test docs). Roadmap
  labels in comments, which `CLAUDE.md` forbids: about fifty lines —
  `clausters-document`'s tests ("O9's acceptance"), `clausters-core-web`'s
  sections ("W3"), `clausters-web`/`-nrt-web` ("the B track"), `T2`, `S19`,
  `F0`, `A1`, `K6` — beside note names a scan also matches.
  **Fixed**: each fused doc went back to its item (two had lost theirs —
  `synthdef_spec_bytes` and the boxes' flat form were already gone — and were
  dropped), and the labels were rewritten out of about 180 comment lines in
  the server, the crates, the host, the Python client and the tests, naming
  what the label stood for where the sentence needed it. What stays is
  `tests/subjects.rs` and `crates/clausters-server/tests/common/bench.rs`, whose `U1`…`U8` are the
  keys of their subject table and not prose.

- ✅ **Two argument readers, two policies** *(audit 2026-09-25)*.
  `osc::server::Args` exists so a handler stops destructuring `msg.args` with
  its own wording, and about fifteen still do (the three streams, `/bus_tap`,
  `/buffer_gen`, `/def_free`, the `/sched_*`, `/server_sync`, `_notify`,
  `_dumpOsc`, `_verbosity`) — the same set whose table rows return `()` and
  fail on their own. `osc::translate` has a second reader set (`int_arg`,
  `string_arg`, `float_value`, `int_value`, and `buffers.rs`' `float_arg` and
  `mirror_buffer`) whose optional arguments turn a wrong type into the default,
  where `Args` refuses it, and whose wording drifts ("no buffer allocated at
  N" beside "buffer N not allocated"). One reader for the wire, in `osc`.
  *Step 1 done*: `translate::mirror_buffer` is the one lookup and
  `translate::allocated` the one refusal ("no buffer allocated at N"); the
  server's copy and its five "buffer N not allocated" sites go through them.
  *Step 2 done*: `Args` moved to `osc::args` for both sides, with
  `Args::after` for a tail past a destructured prefix (numbering from the
  message's first argument) and `opt_float`/`opt_str`; `int_arg`,
  `string_arg` and `float_arg` are gone, so a wrong-typed optional argument of
  a `/buffer_*` job or `/midi_bind` is refused instead of read as its default
  (`a_wrong_optional_argument_is_refused_by_its_number`).
  *Step 3 done, closing it*: the handlers that destructured `msg.args` and
  failed on their own — the three subscriptions, `/bus_tap`, `/buffer_gen`,
  `/def_free`, the three `/sched_*`, `/server_sync`, `_notify`, `_dumpOsc`,
  `_verbosity` and the three `/def_send` families — read through `Args` and
  return `Answer`, so the dispatcher is the one place that sends `/fail`.

- ✅ **The command table repeats one closure sixty times** *(audit
  2026-09-25)*. 17 rows are `handle_buffer_cmd(addr, m, f); Ok(())` and 30 are
  `handle_via_translate(m, f); Ok(())`; two named functions make each row one
  line. Beside it, 34 `/done` replies are built by hand (10 in `transport.rs`,
  each followed by the same broadcast), "every notify client" is looped by hand
  seven times (two of them "but the writer"), and "send every command, stop on
  a full FIFO" three times.
  **Fixed**: `handle_buffer_cmd` and `handle_via_translate` have a row's
  signature and return `Answer`, so 44 rows name them directly (the three
  MIDI rows now persist their bindings only when the translate succeeded);
  `done`/`done_with` build every `/done`, transport's `changed` is the
  acknowledgement plus the broadcast, `notify`/`notify_but` are the one loop
  over the notify clients, and `send_all` the one "every command, or a full
  FIFO".

- ✅ **The server's lifecycle is written twice** *(audit 2026-09-25)*.
  `OscServer::bind` and `::headless` spell the same forty-five-field struct;
  `run` and `step` repeat the subscription pump; `drain_tcp` and `drain_ws` are
  one loop over two hubs and both inline `collect_async`; `bind` recomputes the
  loopback wake target `wake_target()` already is. The three subscriptions
  (`/bus_stream`, `/bus_tapStream`, `/buffer_stream`) repeat their parse,
  replace-per-client, pacing and pump, and a comment in `handle_bus_tap_stream`
  restates `tap_window_cap`'s doc.
  **Fixed**: `OscServer::build` is the one struct literal, `bind` and
  `headless` differ only in the socket, runner, clock and budget they hand it;
  `loopback` is the one wake-address rule; `pump_subscriptions` ends both a
  `run` turn and a `step`; `drain_stream` is the one loop over a stream hub,
  collecting through `collect_async`; the three subscriptions pace through one
  `Pace`; and the comment in `handle_bus_tap_stream` points at
  `tap_window_cap` instead of restating it. The subscriptions' parse and
  replace-per-client stay per command — they read different arguments and
  hold different watches.

- ✅ **Four stream-transport implementations** *(audit 2026-09-25)*. TCP and
  WebSocket in `crates/clausters-server/src/osc` and again in `crates/clausters-gui/src/host`: `next_frame` 0.97
  alike, `local_addr` identical, `write_frame`, `reply`, the connection loop
  0.8, and the server's own TCP and WS `bind` 0.90 alike; two `ClientId` enums
  with the same carriers. The framing and the hub (slots, wake, a thread per
  connection, a frame queue) are one thing both ends could link from the core.
  **Fixed**: `crates/clausters-net` holds both carriers once -- the framing,
  the acceptor and connection threads, `ClientSlots`, the `Waker`, one generic
  `Hub` over a `Conn` reply handle, and a sink seam the GUI's windowed front
  feeds its event-loop proxy through. The host gains what only the server had:
  the client ceiling, the TCP reply write timeout, the bounded headless queue,
  and a dropped connection on a failed TCP reply. Not a network dependency of
  `clausters-core`. Found on the way: a TCP connection closed for a bad frame
  prefix stayed open to its client until the loop drained the disconnect,
  because the write half kept the socket alive; the reader now shuts it down.
  The two `ClientId` enums stay two -- beside TCP and WS, one has the ring and
  the other the page, which is what each front actually serves.

- ✅ **Writing one control is written six times** *(audit 2026-09-25)*. In
  `osc::translate`, `/node_set`, `_setRange`, `_fill`, `_map`, `_mapRange` share
  the walk (id, unknown node, control targets, the def, the hit flag,
  re-analysis), and the three setters and the GraphDef surface
  (`apply_surface`) share the write (`SetControl`, mirror, clear the map).
  Also here: "blob or string as JSON" three times (`d_recv`, `d_graph`,
  `synthdef_spec_bytes`), `/def_send synth` and `graph` one function over two
  families, a dead `let _ = def` in `graph_new`, and the instancing step named
  `realize`.
  **Fixed**: `for_each_target` is the walk the five control commands share,
  `write_control` the write the three setters and `apply_surface` share, and
  `map_control` the map the two mappers and `graph_map` share;
  `osc::args::json_payload` is the one "blob or string"; `install_json_def`
  installs a SynthDef or a GraphDef and `handle_def_send_json` answers both
  families; the dead binding is gone and the step is `build_plan`.

- ✅ **The wire's sample blob is spelled by hand** *(audit 2026-09-25)*. Raw
  little-endian `f32` is written in about twelve places (`/buffer_getRange`,
  `_peaks`, `_export`, the streams, `clausters-editing`'s apply and samples,
  the audio editor, the clipboard) and read in about eleven (the translator,
  editing, the clipboard, eight in the GUI host). `clausters_core::bytes` is the
  cache format, native-endian, and not this. One pair in the core.
  **Fixed**: `clausters_core::osc::{push_sample, sample_blob, blob_samples}`
  write and read it, and the server, the translator, `clausters-editing`, the
  audio editor, the clipboard (whose `encode_samples`/`decode_samples` keep
  their names and delegate) and the host's readers and writers all go through
  them. The WAV float body in `nrt` stays its own: it is a file format that
  happens to share the bytes.

- ✅ **Smaller duplicates** *(audit 2026-09-25)*. The sample FIFO of `Ifft` and
  of the convolution (one type in `dsp`); `Fft`/`Ifft`'s `window` command; the
  Faust node body in `faust/synth.rs` and `synth_web.rs` (five methods
  identical, `process` 0.92 — the web module says it *is* the native code); the
  MIDI 7-to-16-bit widening in `crates/clausters-server/src/midi` and `clausters-midi`, and
  `midi2freq` beside the core's `midi_to_hz`; `clausters-ffi`'s `text` and
  `fill` copied into `notation.rs` and `fill` re-inlined in eight files, with
  two ways of holding a sized answer until it is fetched, while
  `clausters-midi` hands bytes out by leaking a `Vec` (a third convention);
  `audio_editor.rs` and `mixer.rs` repeating `control` and `reader_name`; the
  two playbacks in `clausters-editing` building transport commands each their
  own way; the engine building its `GarbageSink` by hand three times and
  handling `AddSynth`'s and `AddGroup`'s insert result with the same lines.
  *Done so far*: `dsp::fifo::SampleFifo` is the one sample FIFO, and
  `window_command` the one window command (which is how the next entry was
  found). `faust::node::FaustBody` is the Faust node body both backends'
  `FaustSynth` delegate to. `clausters_core::midi` is the one 7-bit and 14-bit widening the
  server's live input and `clausters-midi`'s clip reader share, and
  `midi2freq` is the core's `midicps`. `clausters-ffi`'s `out` module is the one `text`,
  `fill` and `fill_then` every file calls (the ones with another contract —
  a nul-terminated path, an error code — keep theirs). The audio editor's defs take `control`
  from the mixer's; both playbacks send through `apply::transport_command`;
  the engine builds its `GarbageSink` with one `garbage_sink!` and reports an
  insert through `started`/`rejected`. (`reader_name` is not a duplicate: the
  two prefixes name two different defs.) *Left*: the three ways a C ABI call
  holds a sized answer until it is fetched — the document's `pending`, the
  apps' `handed`, and `clausters-midi`'s leaked buffer the caller frees —
  which is a convention of the bindings in both clients, to decide before it
  is changed.
  **Decided**: no ABI change. The document and the apps share the protocol
  (size-then-fill in one function), and what each keeps between the passes
  differs for a reason — a pure read's bytes, a verb's answer keyed by its
  request so it never runs twice. `snapshot` copied by hand and now goes
  through `out::fill_then` like the apps; `clausters-midi` keeps the ordinary
  allocate-and-free shape of a separate library. `docs/bindings.md` ("How a
  C call hands bytes back") names both conventions.

- ✅ **An `IFFT` window swap left the overlap-add at the old window's level**
  *(found 2026-09-25, folding `Fft`'s and `Ifft`'s window command into one)*.
  `Ifft` refilled its window on `/node_ugenCmd … window` but not `norm`, the
  COLA denominator computed from the window at construction, so a live swap
  from Hann to rectangular at a 50% hop reconstructed at about +9 dB (RMS 2.05
  for a unit sine). `fill_cola_norm` is now the one computation, run at
  construction and again, in place, on a swap;
  `a_live_window_swap_keeps_the_round_trip_at_unity` holds it.

- ✅ **The engine finds a node by scanning 8192 slots** *(audit 2026-09-25,
  measure first)*. `NodeTree::find` is linear over `MAX_NODES`, on the audio
  thread, for every control set, map, run, free, move, done action and bundle
  classification, and `insert` scans again for a free slot. A block carrying
  many `/node_set` pays that per command. An id-to-slot table, preallocated, is
  O(1); `crates/clausters-server/examples/bench.rs` says whether it matters before anything changes.
  **Measured, then fixed**: the bench's new "per-command cost" rows read
  0.03 / 0.2 / 0.8 us per `/node_set` over 32 / 1000 / 4000 nodes — 256 sets
  in a block over 4000 nodes took 15% of it, and an unknown id always scanned
  the whole slab. `node::index::IdIndex` (open addressing, sized once to twice
  the slab, backward-shift deletion) is kept by the one insert and the one
  `take_slot`, and `find` reads it: 0.03 / 0.05 / 0.06 us. The free-slot scan
  on insert stays linear — it runs once per new node, not per command.

- ✅ **Long functions that hold several steps** *(audit 2026-09-25)*.
  `Engine::process_block` (about 340 lines: the next event, a transport edge,
  the frozen runs, time publication, counters and done actions), with "the
  transport freezes here" written in the edge branch and again in
  `TransportRun`'s stop; `synthdef::compile` (about 510); `realtime_main`
  (about 500, thirty flags parsed with the same two lines each, `nrt_main`
  holding a helper the other does not use); `ipc::Segment`'s eight
  telescoping constructors. Named steps, and no behaviour changes.
  *Done so far*: `flag_value`/`flag_parse` read a flag's value for both
  `realtime_main` and `nrt_main`, which is twenty-odd pairs of lines each
  one call. `ipc::Regions` names the four region sizes a segment is
  created with, so `Segment`'s eight constructors are six (`in_memory`,
  `in_memory_sized`, `create`, `create_sized`, `open_or_create`, `open`) and
  no caller passes four bare counts in a row; a session's in-memory segment
  now takes its audio-bus count from the session like the mapped one did. `Engine::process_block` is 72 lines of named steps —
  `next_due`, `cross_edge`, `apply_due_bundle`, `publish_block`,
  `store_counters`, `apply_done_actions`, `drain_replies`, `meter_block` —
  with the bench's default rows within noise of before. `synthdef::compile`
  is 236 lines, its controls, arity, static configuration, spectral chain and
  lag insertion each a function (`parse_controls`, `check_arity`,
  `ugen_config`, `spectral_chain`, `insert_lags`); what stays inline is the
  walk over the inputs and the rates, which shares the whole graph built so
  far. The "transport freezes here" twin turned out to be one line each
  (`set_paused` on the governed group) with different surroundings, and was
  left.

- ✅ **Minor** *(audit 2026-09-25)*. `/server_notify` answers a client id that
  is its position in the list, so an earlier client leaving renumbers the
  others (no client reads it); a typo "where the the transport is";
  `/buffer_peaks` defaults its bucket to a literal 256 beside `BASE_BUCKET`;
  `MAX_NODES` and `Limits::default().max_nodes` kept equal by a comment; a
  few docs starting lowercase and one "realized" in prose.
  **Fixed**: a notify client keeps the id it was answered with
  (`a_notify_id_survives_an_earlier_client_leaving`); the typo; `/buffer_peaks`
  defaults to `BASE_BUCKET`; `Limits::default()` reads `MAX_NODES`,
  `MAX_GROUP_CHILDREN` and `NUM_BUFFERS` instead of copying them; the
  lowercase sentence starts; "realize" for a join is "build" in the four
  places that said it, the quoted document comment included. One doc in the
  host's gesture tests had lost its first four lines in the host split and
  has them back.

- ✅ **A render inside the client's own process loses what the server
  logged** *(found 2026-09-26 while making a UGen's faults and a `Poll` reach
  whoever they are for; fixed the same day)*. A render in a separate process now reaches the
  client: `clausters --nrt` logs to stderr and Python forwards each line to its
  `clausters.render` logger. A render inside the client's own process does
  not: Python's `render()` without a `path` goes through the embedded library,
  and the web client renders with the wasm engine in the page, and neither has
  a logger installed, so a warning such as a synth the engine rejected is lost.
  A fault is not lost, since it fails the render in both, but a `Poll` and
  every warning are. Both clients are the same here, so it is a gap and not a
  divergence.

  **The decision** is the carrier: a list of warnings on the render's result
  (`RenderStats`), which moves the C ABI (`clausters_render`) and the wasm
  entry point together, and so `CORE_ABI_VERSION`; or a log callback the host
  installs, which moves the same two surfaces differently. The warnings on the
  result are the smaller change and the one both clients can read the same way.

  **What shipped.** The warnings on the result, carried the way the render's
  seed already was. While a render runs, `server::render_log::during` puts a
  subscriber in front of whatever was there that keeps each event at `info`
  and above and passes every event on, so the CLI prints exactly as before;
  the lines ride out on `RenderStats::log` (at most a thousand, then a count of
  what was dropped). Both in-process carriers hand them over the same way, as
  a second call after the render rather than more out pointers on it:
  `clausters_render_log(buf, cap)` on the embed ABI and `last_render_log()` on
  the wasm entry point, one `LEVEL<TAB>message` line each. The render's own
  signature did not move; the counter did (`ABI_VERSION` 13, the embed ABI,
  not `CORE_ABI_VERSION` as guessed above), because a binding that reads the
  log cannot load a library without it and a stale copy should be refused by
  name. Python logs each line on `clausters.render` at its level through one
  `render_line`, which the `--nrt` stderr path now uses too; the web client
  sends `ERROR`/`WARN` to `console.error`/`console.warn` and `INFO` to
  `console.debug`, shown only when asked, as Python's `info` is.

- ✅ **The render's score is named `Score`, and the clients call it
  `OscScore`** *(found 2026-09-27, listing every class named after a score)*.
  `crates/clausters-server/src/server/render.rs` reads the binary an offline render runs into `Score`,
  a list of `ScoreEvent`s (the messages of one bundle at a time in seconds).
  The Python client names the same thing `OscScore`, and `Score` with no prefix
  is notation's across the project (the engraver's editable score in
  `clausters_core::notation` and both clients). So the server's type becomes
  **`OscScore`**; `ScoreEvent` stays, since inside the render nothing else
  it could be confused with lives. **Related:** the web client's copy of the
  same divergence (`clients/web/PLAN.md`, Found by use, "The offline score and
  its carrier are named apart from Python's"). **Fixed 2026-10-01** with the
  web client's, in the same commit.

- ✅ **A lane's data is written twice and typed nowhere** *(found 2026-09-29,
  with the user, reading what `/lane_set` carries)*. The JSON is right -- a
  lane is a sequencing structure, and `/lane_set` is the one command besides
  the defs that takes a JSON object. What is wrong is that no Rust type
  stands for it: the crate writes it with `json!` in
  `clausters_editing::notes_playback::data`, and the server reads it by hand
  into its private `LaneEvent` in `crates/clausters-server/src/osc/server/lanes.rs::parse_events`.
  A field that moves on one side is found by no compiler on the other. The
  fix is one type (a lane's notes and messages, in samples) that both sides
  serialize, where the crate and the server can both depend on it. `T9`'s
  MIDI event is written into that type, not into both halves. **Fixed
  2026-09-29 with `T9`**: `clausters_core::lane::LaneData`.

- ✅ **A group's order does not see control maps** *(found 2026-09-30,
  writing the notes editor's curves, `crates/clausters-apps/PLAN.md` `X3.11c`)*.
  An auto-sorted group orders its children by the audio buses they read and
  write (`TreeMirror::sorted_children`); a control bus written by `OutCtl` and
  read through a `/node_map` counts for nothing, so a curve's reader and the
  node that reads its bus run in whatever order they were added. The note
  graph lists its readers before its voice for that reason, and a note, a slot
  added at the head of its channel's instance, runs before the channel's
  readers and hears their curves one block late. Folding control writes and
  control maps into the usage the sort reads would make the order follow the
  wiring, as it does for audio. **Fixed 2026-09-30**: `BusUsage` keeps the
  control buses read and written beside the audio ones -- `InCtl` and a
  control map read, `OutCtl` writes, the new `BusRole::ReadControl` and
  `WriteControl` -- and `feeds` is either kind; the stage masks fold them into
  their lanes, a map made or cleared re-sorts, and a GraphDef member's maps
  count from the moment it is built. A note now runs after its channel's
  curves, and inside its graph after its own.

- ✅ **A MIDI graph voice released by its gate may leave its slot behind**
  *(found 2026-09-30, reading `/midi_bind` onto a GraphDef while adding
  `ends`; not reproduced)*. A gate-aware binding releases a graph voice by
  setting its `gate` port to 0; the member whose envelope then frees itself
  goes, and nothing frees the slot group around it. If that holds, every note
  played leaves an empty group. A GraphDef member marked `ends` now frees its
  graph and the slot around it; a voice graph's generator member is where the
  mark belongs, or the binding could free the slot itself. **Reproduced and
  fixed 2026-10-01**: three gated notes left three empty slot groups under the
  instance. The server now frees a graph voice whose members have all ended on
  their own (`CmdTranslator::take_emptied_slots`), so a def need not mark its
  generator `ends` for the voice to go with its note; a slot that holds a
  nested graph is left to its `ends`, as before.

- ✅ **A control mapped to a bus reads back what was last set on it** *(found
  2026-10-16, building `J12`)*. `/synth_get` reads the network thread's mirror
  of a node, which holds the value a `/node_set` wrote and the map, not what
  the bus carries; so a generic editor's knob over a mapped control stands still
  while the control follows its bus. A control bus is an atomic the network
  thread can read, so `/synth_get` answering the bus for a control mapped to
  one is a small change; an audio-mapped control has no such value to read and
  stays as it is. **Fixed 2026-10-30:** `/synth_get`, `/synth_getRange` and
  `/synth_getText` read a control the mirror records as mapped to a control
  bus from that bus's atomic.

- ✅ **A FaustDef's `[unit:...]` and `[scale:log]` metadata are not read**
  *(found 2026-10-16, building `J12`)*. A Faust slider brings its range and
  step to `/def_query`, and its unit and its scale stay in the DSP's metadata,
  so a generic editor draws a frequency slider linear and with no unit. Both
  walks that read a def's parameters (the libfaust UI glue and the compiler's
  JSON) would read the two declarations into the control's display.
  **Fixed 2026-10-30:** both walks read a widget's declarations through
  `ParamSpec::declare`, and `/def_query` reports the unit and, for a log
  scale over a range above zero, the `exp` warp.

- ✅ **A write in place behind an undrained replacement can miss a shared
  buffer** *(found 2026-10-16, fixing the audio editor's gain preview)*. While
  a `/buffer_alloc`, read, gen or stitch on an index is still in the NRT queue,
  a `/buffer_set`, fill, gain, reverse or mix sent behind it builds on the
  queue's own buffer (`NrtChain`). When the server shares buffers, the drain
  copies that buffer into a region and the mirror holds the copy, so a write the
  NRT thread runs after the copy lands in memory nobody reads. Every client
  awaits an allocation's `/done` before writing, which is why nothing has hit
  it; closing it means the chain learning the shared buffer, or the share
  happening on the NRT thread before the result is returned.
  *Fixed 2026-11-02, by neither.* A write in place that built on the queue's
  own buffer answers that buffer as a replacement (`NrtChain::run`): the
  drain installs and shares it again, with every write behind it, and the
  index owes a replacement until that result is drained (`nrt::owes`), so a
  write sent meanwhile chains too rather than landing in a region the next
  install leaves behind. A region a new install replaces is unlinked by name,
  as a freed one is. `crates/clausters-server/tests/ipc.rs` reproduces the race with one job a turn
  (two fills behind a reallocation, both lost before) and checks a write
  after the drain.

- ✅ **Idle shared-memory level publication scans every allocated audio bus**
  (2026-10-19, audio-editor window review). Each block cleared all bus samples
  and scanned them again to publish peak levels, even for unused capacity.
  Preallocated atomic write markers now identify buses that need clearing on
  the next block and peak scans in the current block. Silent buses still decay
  their published levels. Directed IPC, parallel/compensation and no-allocation
  regressions preserve samples, bus clearing and meter decay; the native
  multichannel review also checks the live endpoint.

- ✅ **A long take distorts more the further it plays** (2026-10-21, the
  user, in the audio editor: a long file copied and pasted at its own end
  until it reached about 30 minutes). The sound distorts progressively as the
  position advances. Suspected, not verified: a position held in 32-bit
  float, which stops addressing single frames past 2^24 frames (about six
  minutes at 48 kHz); the reader's phase and the transport's position are
  where to check.
  *(Fixed 2026-10-21: the suspicion was right. The phase crossed from
  `TransportPos` to `BufRd` as an `f32` signal; `TransportBufRd` keeps it in
  `f64` and the audio editor's and multitrack's readers use it. `Phasor`
  into `BufRd` or `BufWr` keeps the limit; removing it for every reader is
  "A buffer position wider than a signal", Future directions.)*

- ✅ **An automatic profiler for the server** (2026-10-21, the user, after
  the audio editor's review). A run that measures, without someone watching,
  where the server's time goes: blocks and their cost per node and per
  command, the NRT queue, transfers and replies. It measures the server's
  general efficiency and gives the host's profiler (`crates/clausters-gui/PLAN.md`,
  Found by use, "An automatic profiler for the host") the server's side of
  the same run, so a slow window is told from a slow engine. **Related:**
  "An automatic profiler for the host" (`crates/clausters-gui/PLAN.md`, Found by use).
  *(Done 2026-10-21: `/server_profile` and `Server.profile(seconds)` in both
  clients; the phases of the block and of the serving turn, every synth and
  def, every command and reply with its bytes, every NRT job's run and wait.
  The per-synth timing runs only while a client asks.)*

- ✅ **A stopped audio editor costs the server CPU for every file it holds**
  (2026-10-21, measured in `X23` of `crates/clausters-apps/PLAN.md`). The
  bare server idles at about 4.2% of a core; with an audio editor open and
  stopped it idles at 6.5% holding one stereo file and 9.2% holding four,
  although the files not in focus have their readers paused and the one in
  focus is in a governed group whose transport stands. Where the blocks go
  is unmeasured: the host's side of the same editor does nothing while
  stopped. Measured with the server's profiler before a cause is assumed.
  **Related:** "An automatic profiler for the server", above.
  *(Fixed 2026-10-21: the profiler put it in the serving turn, which
  rebuilt the latency plan from a walk of the whole tree on every turn; it
  is rebuilt now only when the tree changed, and the server idles at 4.7%
  under the editor whatever it holds.)*

- ✅ **An effect applied from its preview is heard twice for a moment**
  *(found 2026-10-23, closing the audio editor's move onto files)*. A gain
  envelope previewed live and then applied renders the take through the
  curve, and the join is stitched over the render while the preview's
  insert still applies the curve; the client puts the insert to unity when
  the edit's answer comes back, so the curve is heard twice between the
  reader's fade into the new list and that answer. Putting the insert to
  unity with the stitch moves the fault to the other side (the dry take for
  as long as the new list takes to read). A signal from the reader to the
  insert does not reach it either: the insert gets the old list and the new
  one already mixed by the reader's fade. What does is **the curve riding
  the list**: a join carries the gain automation it is previewed with, the
  list reader applies each list's own curve, and its fade then plays the old
  list through its curve into the new one rendered through it -- which is
  the X28 acceptance's "nothing heard but the effect", and also keeps a
  curve aligned when a cut moves it under a preview that plays.
  *Closed 2026-10-23 by the user's decision to remove the gain envelope's
  preview* (`crates/clausters-apps/PLAN.md`, the turn to disk's eighth
  decision): nothing hears the curve before it is applied, so nothing is
  heard twice. The design above stays as one way to hand an effect heard
  live over to its render, should one need it (the apps plan's Future
  directions, "An effect heard live before it is applied").

- ✅ **A disk reader starts with a click, and a loop clicks at every
  wrap** *(found 2026-10-23, by ear on the audio lifecycle example; design
  decided with the user the same day)*. `TransportDiskIn` lives in the
  transport's governed group, which is frozen while the transport stands,
  so the reader never fills its ring at the standing position: the first
  block after a play finds it empty, plays silence until the refill lands,
  and the take comes in as a step after the declick's ramp has ended. A
  loop does the same at each wrap, since the reader does not know the
  transport's loop and reads past its end. **The decided design**, the
  field's pre-roll, which keeps every transport's timing as it is:
  - **`/transport_cue <transport>`**, a new command: the transport stays
    stopped where it is, its governed group runs with `rolling` false so
    the disk readers fill their rings at the standing position, and the
    reply is `/done` once every reader holds it (or after a bounded wait,
    reported). Nothing else moves: the transport's clock stays frozen, so
    its bundles and lanes are where they were.
  - **`/transport_play` is unchanged**: it rolls on the sample it lands on,
    alone or in a timetagged bundle, so transports started together stay
    together. A play with no cue before it behaves as today.
  - **The audio editor plays by cue, `/done`, then play**: the playback's
    play and a play after the cursor moved go through the cue in both
    clients, through the coordinator's steps.
  - **A loop is read straight through**: the reader is told the transport's
    loop span (`TransportCtx`), the disk thread renders from the loop's end
    back to its start, and the audio thread's head wraps with it, so a wrap
    costs no refill. A span changed while it plays is a list replaced while
    it plays, which already fades.
  - Native and page readers alike (`dsp::stream`, `dsp::disk_web`), the
    reference in `docs/schemas.md`, both clients' builders, and a test that
    plays from a stand and across a wrap with no silent frame.
  Shipped as designed (2026-10-23): the cue waits at most 0.2 s, and a
  reader is ready holding 4096 frames past where it stands. The page's
  reader has no automated test of its own; it is checked by ear in the
  page's example.
  The recording side of the same timing is a Future direction ("Recording,
  monitoring and playback aligned as one model of the transport's
  latencies").

- ✅ **A loop switched at its end** *(found 2026-10-23, by ear on the audio
  editor the GUI host owns; the rest of the case fixed the same day, and
  this the same day too, by cueing a loop change as the user decided)*.
  Turning the loop switch, or moving the loop, while a take plays is a
  reading replaced while it plays: the reader keeps the old reading's next
  frames aside up to where the two readings part, asks for the new one from
  there and splices it on, so a switch turned mid-loop is not heard at all.
  What is left is a switch turned within one device callback of the loop's
  end (on or off): the frames where the readings part are played before
  the reader's I/O can read the new ones, and the take comes in again from
  a short silence. The field's answer would be to cue a loop change as a
  play is cued (`/transport_cue`): the transport keeps the loop it has
  until its readers hold the new reading, so a switch turned that late
  takes effect one pass later instead of being heard. **Done so**: a
  `/transport_loop` while the transport rolls over disk readers waits until
  every reader holds the new reading (at most `CUE_WAIT_SECS`), and a
  splice reached first plays the old reading on and reads it again.

- ✅ **Two loop switches close together at its end** *(found 2026-10-24,
  measuring the cued loop change)*. A loop change cued while the frames
  kept from the change before it still play keeps reading the loop asked
  for, not the one in force: where the second switch is turned just before
  the loop's end, the engine may put it in place after that end has passed,
  and the reader then holds no reading of the loop that was in force past
  it, so the take comes in again from a short silence. One switch at the
  end is not heard (the change lands a pass later), nor are switches at a
  hand's pace. The design that closes it: the ring always follows the loop
  in force, and a second, small ring of each reader is read ahead with the
  continuation of the loop asked for from where the two part; the engine
  puts the change in place only while that continuation is held before the
  parting point, and a reader switches rings there. Natively, through the
  fed stream's protocol (`dsp::fed`, `TakeFeed`) and in the page's reader
  and the host that serves its spans. *Fixed 2026-10-24 without a second
  ring:* the kept frames were the place to look. A reader was not ready
  while its ring refilled, though the frames it kept from the first change
  already read the loop asked again (the switch turned back on); they now
  count. Where the second change asks for another loop, the reader first
  keeps what the loop in force plays (the kept frames to their splice and
  what the ring read after it), so a change the engine defers past the
  loop's end finds the reader holding the loop the transport wraps by.
