# Implementation plan: a real-time scsynth-style audio server: the closed tracks

The tracks of [`PLAN.md`](../../../PLAN.md) whose every milestone shipped, moved here
whole on 2026-11-12 so that the plan holds what is open. They are the record
of what each track decided and built; a track is not reopened here, and
work that changes what one built is written in the plan.

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
