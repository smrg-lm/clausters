# Architecture (developer documentation)

How Clausters is built, where everything lives, and the invariants a change must not break. User-facing documentation (wire formats, OSC commands) is in [`schemas.md`](schemas.md); the plan and per-milestone completion log are `PLAN.md` and `LOG.md` at the repository root.

## Threads

```text
         OSC/UDP+TCP           cmd FIFO (SPSC, pre-built commands)
 client ◄────────► network ─────────────────────────► audio (cpal callback)
                   thread  ◄───────────────────────── Engine::process_block
                     │   garbage FIFO + event FIFO
          ┌──────────┴──────────┐
          ▼                     ▼
      NRT thread          Faust compiler thread   (feature `faust`)
   (disk I/O, buffers)    (libfaust JIT, ~10 ms/def)
```

- **Network thread** (`osc::server::OscServer::run`): owns the UDP socket (100 ms read timeout — each timeout tick collects garbage and async results; 2 ms when an IPC ring is attached, which it also drains every iteration), parses every packet — datagram or ring — through `osc::decode_packet`, builds commands *fully allocated* — boxed synths, pre-reserved group child lists — and pushes them into the command FIFO. It also owns all lookup tables: the def tables, the node-ID→def mirror, the buffer mirror, and the **tree mirror** (`osc::graph::TreeMirror` inside `osc::translate::CmdTranslator`) — topology, per-node control values and bus usage, fed by the same `Cmd` stream the engine gets and rolled back by rejection garbage; it answers `/g_queryTree`, `/n_query` (per-node detail) and `/g_dumpGraph`, and drives the auto-sorted groups without touching the audio thread. All replies (`/done`, `/fail`, `/n_go`/`/n_end`, queries) are sent from here.
- **Audio thread** (the cpal callback, `server::backend`): runs `Engine::process_block` on 64-frame blocks. Per block: drain the command FIFO, fire scheduled bundles whose time falls inside the block (splitting it at the exact sample), walk the node tree in depth-first order, push dead memory to the garbage FIFO. It never allocates, locks or does I/O, and re-arms `dsp::denormals::flush_to_zero()` on every callback.
- **NRT thread** (`server::nrt`): all `/b_*` work — allocation, file reading (WAV via hound, compressed/other formats via symphonia), WAV writing via hound, zeroing. One queue, so commands on the same buffer complete in submission order. Produces immutable buffers the network thread installs with `Cmd::SetBuffer`.
- **Disk I/O threads** (`dsp::disk`, `DiskIn`/`DiskOut`): one background thread per streaming UGen instance, spawned at `/s_new` (build) and joined when the synth's `Box` is dropped on the network thread (via the garbage FIFO). It decodes (symphonia) or encodes (hound) between disk and a lock-free `rtrb` ring the audio thread pops/pushes; the audio thread never does disk I/O. Self-contained: no engine, OSC or `ProcessCtx` involvement.
- **DSP workers** (`server::workers`, opt-in via `--workers N`): a fork-join pool the audio thread conducts to process the stages of `/g_parallel` groups. Atomic work stealing, bounded spinning, park/unpark only across idle gaps; each worker arms flush-to-zero at spawn. With 0 workers (the default and the whole test suite) the pool is inert and everything is sequential.
- **Faust compiler thread** (`faust::compiler`, feature `faust`): JIT compilation of `/d_faust` defs. libfaust does not tolerate concurrent compilation in one process (SIGSEGV), so every compiling FFI call holds the process-wide `ffi_lock()`; instantiating from a finished factory is concurrency-safe and happens on the network thread.
- **TCP I/O threads** (`osc::tcp`, opt-in via `--tcp [port]`): an acceptor thread plus one reader thread per connection turn each TCP byte stream into whole, length-prefixed OSC frames and hand them to the network thread over an `mpsc` channel — drained every loop iteration like the IPC ring. The command processing stays single-threaded; these threads only do I/O. A reader pings the network thread's own UDP socket with a **zero-length datagram** after queuing a frame, so the loop wakes immediately instead of waiting for the GC tick. Replies write back through the connection's write half (the network thread owns it; `&TcpStream` is `Write`, so a reply needs only a shared borrow). No async runtime, no new dependency.
- **MIDI input thread** (`midi::live`, feature `midi`, opt-in via `--midi [name]`): `midir` opens a virtual ALSA-seq input port and runs the input callback on **its own thread**, which decodes each MIDI 1.0 message (`midi::parse_midi1`, widening to the internal high-resolution form) and hands the `ChannelVoiceMessage` to the network thread over an `mpsc` channel — drained every loop iteration, exactly like the TCP frames, with the same zero-length-datagram wake. The network thread does the actuation (`CmdTranslator::translate_midi` realizes each message as the equivalent `/s_new`/`/n_set`/`/n_free`); the MIDI thread only decodes. The audio thread is never involved.

**Offline rendering** (`server::render`, the `--nrt` CLI mode) uses no threads at all: one thread drives both halves of `engine_pair`, runs NRT jobs and Faust compilations synchronously between blocks (scsynth NRT semantics), and arms flush-to-zero once at the start. Because scheduled commands go through the same engine queue as in real time, an offline render is sample-identical to a perfectly timed live take.

The realtime backend (cpal) sits behind the `realtime` feature (on by default); the engine itself knows nothing about cpal, which is what makes the offline mode and the integration tests possible.

**Logging** (`src/logging.rs`) uses `tracing`. The **binary** installs the subscriber (`logging::init`, stderr, level from `-v`/`-vv`/`-q` or `RUST_LOG`); the library and embed users do not, so the macros are no-ops for them. The filter is a runtime-reloadable `EnvFilter`, so the OSC commands `/verbosity` (set the level/directive) and `/dumpOSC` (overlay `clausters::osc=trace`) let a *client* retune the server's logs live. The audio thread never calls a `tracing` macro: every condition it detects (a rejected command, a dropped bundle) leaves through the garbage FIFO and is logged by the network thread, so logging stays clear of the real-time path.

## Module map

| Path | Contents |
|---|---|
| `src/server/engine.rs` | The core: `Engine` (audio half), `EngineHandle` (network half), `Cmd`, `Garbage`, the FIFOs, the schedule queue, the sample clock |
| `src/server/backend.rs` | cpal glue: `BlockAdapter` slices arbitrary callback sizes into 64-frame engine blocks (feature `realtime`) |
| `src/server/nrt.rs` | NRT thread, `NrtJob`/`run_job` (also called synchronously by the renderer), audio reading (`read_audio`: WAV via hound, other formats via symphonia) and WAV write helpers |
| `src/server/workers.rs` | worker pool: stage publish/steal/wait protocol for parallel groups |
| `src/server/ipc.rs` | the versioned shared segment — data plane (clock, control buses) + OSC byte rings (`--shm` and embed transports) |
| `src/embed.rs` | the embed C ABI (feature `embed`, exported by the cdylib) |
| `src/logging.rs` | `tracing` setup: `init` (binary-only subscriber, stderr), runtime-reloadable filter behind `/verbosity` and `/dumpOSC` |
| `src/server/render.rs` | Offline mode: `Score` (binary scsynth score format), `render`/`render_to_vec`/`render_to_wav` |
| `src/node/mod.rs` | `NodeTree` (fixed slab), `SynthNode` trait, groups, add actions, moves |
| `src/dsp/mod.rs` | `UGen` trait, `ProcessCtx`, buses, the cache-line-aligned `Block`, block/bus-count constants |
| `src/dsp/<ugen>.rs` | One file per UGen family (`sinosc`, `binop`, `io`, `noise`, `buf`) |
| `src/dsp/registry.rs` | `UGenKind`: name parsing, input arity, construction |
| `src/dsp/buffer.rs` | Immutable sample buffers and the engine-side pool |
| `src/dsp/denormals.rs` | Per-thread flush-to-zero (x86-64 MXCSR, aarch64 FPCR) |
| `src/synthdef/` | SynthDef JSON wire format, validation/compilation, `UGenSynth` instance |
| `src/osc/mod.rs` | `decode_packet` — the only entry point for incoming OSC bytes |
| `src/osc/server.rs` | The network thread: socket loop, immediate handlers, replies |
| `src/osc/tcp.rs` | TCP transport (`--tcp`): acceptor/reader threads, length-prefixed framing, `TcpHub` |
| `src/osc/translate.rs` | `CmdTranslator`: OSC message → `Cmd`, shared by the live server and the renderer; owns the tree mirror |
| `src/osc/graph.rs` | bus-usage analysis, the network-side `TreeMirror`, the stable topological sort behind `/g_sortMode` |
| `src/osc/graphdef.rs` | GraphDef spec/instance types + the private-bus `RangeAllocator`; instantiation lives in `translate.rs` |
| `src/midi/` | standard channel-voice MIDI actuation — message-type conversions (`convert.rs`), bindings/voice state, MIDI 1.0→2.0 widening; live input via `midir`/ALSA (`live.rs`, feature `midi`). `CmdTranslator::translate_midi` realizes a message as the equivalent `/s_new`/`/n_set`/`/n_free` on the **network thread** (the audio thread is untouched) |
| `src/faust/` | libfaust embedding: hand-written FFI, compiler thread, JSON→Box interpreter (`boxes.rs`), `FaustDef`/`FaustSynth`; `soundfile("<bufnum>", n)` is filled from a server buffer at instantiation (`SoundfileData` in `synth.rs`) |
| `src/main.rs` | CLI: realtime server (default) or `--nrt` renderer |

## Memory lifecycle

The rule behind everything: **memory is allocated on the network (or NRT / compiler) thread, used on the audio thread, and freed back on the network thread.** The audio thread only moves pointers.

1. A command arrives over OSC. The network thread builds the complete object — e.g. `/s_new` boxes a `UGenSynth` with its UGens and wires, `/g_new` pre-reserves the child list — and pushes a `Cmd` into the command FIFO.
2. `process_block` drains the FIFO and plugs the object into the tree: O(1), no allocation.
3. When a node dies (`/n_free`, replace actions, or an `EnvGen` done action — see below), the boxed synth leaves through the **garbage FIFO** as a `Garbage` variant; the network thread drops it (`collect_garbage`), updating its mirrors.
4. Rejected commands (duplicate ID, unknown target, full slab/group) come back as `Garbage::RejectedSynth`/`RejectedGroup` so the memory still dies off the audio thread.

Two shared structures cross threads without the FIFOs:

- **Control buses**: 1024 atomics (`dsp::ControlBuses`). Immediate `/c_set` and `/c_get` are served directly on the network thread; the audio thread reads them through `InCtl`. A *scheduled* `/c_set` must land on its exact sample, so it travels as `Cmd::SetControlBus` instead. With an IPC segment the backing array lives in shared memory: other processes write the same atomics.
- **Buffers**: `Arc<Buffer>`, **immutable once installed**. The NRT thread builds them, `Cmd::SetBuffer` swaps them into the engine pool, the replaced `Arc` returns as `Garbage::FreedBuffer`. "Mutating" commands (`/b_zero`, `/b_read` into an existing buffer, `/b_gen` filling a wavetable) build a replacement instead of touching shared memory. The network thread keeps a mirror for `/b_query`/`/b_write` and for validation.

### Done actions (self-freeing nodes)

A UGen can ask for its node to be freed, paused or resumed once it finishes: the `UGen` trait has a `done(&self) -> DoneAction` hook, checked right after `process`. `EnvGen` is the first user — it returns its `doneAction` when its last segment ends, one of scsynth's full **0–15** set (`DoneAction::from_i32` maps the float; see the table in `schemas.md`). The synth aggregates its UGens' actions (`UGenSynth::done_action` takes the strongest). The tree walk handles them by kind: **`None`** does nothing; **`PauseSelf`** is applied inline — the node's `paused` flag is set and it is skipped (silent, state kept) from the next block on. A paused node (synth **or** group — a paused group skips its whole subtree) is resumed by clearing that flag: the **`/n_run`** command (`Cmd::RunNode` → `NodeTree::set_paused`), which makes `PauseSelf` non-terminal, and the `freeSelfResumeNext` action. **Every other action** — `FreeSelf`, `FreeGroup`, and the relative ones — is recorded (id plus action code) into a **lock-free finished-node queue** (`NodeTree`'s `done_nodes`/`done_actions`/`done_count`, index reservation by `fetch_add` so the parallel workers can push concurrently), drained **once per block, after the whole tree has run** (`Engine::process_block` → `NodeTree::apply_done_action`). Freeing after the full walk (not mid-traversal) keeps the tree stable while it is being read, and makes a re-queue from a mid-block schedule split — or two synths both freeing the same group — harmless (freeing an already-gone id is a no-op). The queue's atomics only need to be race-free for the concurrent reservation; visibility to the drain is provided by the worker pool's join.

The **relative actions** (`freeSelfAndPrev`/`Next`, `freeSelfToHead`/`Tail`, `freeSelfPause`/`Resume` a neighbour, `freeSelfAndFreeAllIn`/`DeepFree` a neighbour group, `freeAllInGroup`) act on the finishing node's siblings. `apply_done_action` resolves the previous/next sibling (and, for the head/tail runs, iterates them) from the parent group's ordered child list — **before** freeing self, since freeing shifts positions — then reuses the ordinary `free`/`free_all`/`deep_free`/`set_paused` paths. All of that runs in the drain on the audio thread, so it stays allocation-free: sibling resolution is index arithmetic and the frees reuse the tree's pre-allocated `free_stack`/`dfs_stack`. Everything leaves on the garbage FIFO exactly like an `/n_free`, never dropped on the audio thread.

### Control/bus mapping (`/n_map`, `/n_mapa`)

`/n_set` writes a control once; `/n_map`/`/n_mapa` make a control **follow a bus**, re-read at the start of every block. Each synth carries a `ControlMap` table parallel to its controls (`node::ControlMap`, pre-allocated at build — `map_control` only flips an entry, never grows it). At the top of `process`, before any UGen runs, the synth pulls each live mapping into its control/zone: a control bus value (`/n_map`), or one frame of an audio bus sampled at control rate (`/n_mapa` — controls are one value per block, and Faust zones are scalar, so there is no audio-rate control). Writing straight to the control storage, never through `set_control`, keeps the mapping intact; a `/n_set` *does* go through `set_control`, which clears the mapping first, so an explicit set always wins (scsynth semantics). `Cmd::MapControl` carries it to the engine and is schedulable in bundles like `/n_set`.

This feeds the bus analysis: the network-side mirror records each node's live maps, and `fold_maps_into_usage` adds an audio map's bus to the node's `reads` and marks the node a dynamic barrier when a mapped control is used as a bus index — so auto/parallel groups stay correct under mappings.

### Typed controls (`tr`/`ir`/lag)

A control carries a **type** (`SynthDef::control_types`, from the def's `"rate"`) that shapes how the engine treats it. Two are handled inside `UGenSynth`, RT-safe:

- **Trigger (`tr`).** After the UGen loop, `process` resets every trigger control to `0`. A `/n_set` therefore holds for exactly the one block it landed in, so a UGen watching for a rising edge (an `EnvGen` gate) fires once. No extra state — the reset is unconditional and cheap.
- **Scalar (`ir`).** `set_control` ignores a write to a scalar control once `initialized` is set (the same S1 flag). The initial `/s_new` values are applied on the network thread before the first block, so they take; a later `/n_set` is dropped, matching scsynth. In the compiler an `ir` control counts as `Rate::Ir`, so it may feed an `ir` UGen input.

**Lag is not a control type — it is an inserted UGen.** A control with a `"lag"` compiles to a real `Lag` (or `VarLag` with `lag_down`) prepended to the graph, reading the raw control; every reference to that control is rewritten to the smoother's wire (`synthdef::compile`, the `lagged` pass, which shifts the original UGens down by the number of inserted smoothers and remaps their wire indices). This keeps **one** lag implementation shared with the client-facing `Lag` UGen — no bespoke control-smoothing path. The inserted smoothers run at audio rate, so a stepped control glides per sample toward its block-constant target.

### Group `/n_set`/`/n_map` and GraphDef

Both live entirely on the network thread, in `CmdTranslator`, and lower into the same `Cmd`s as a hand-written `/s_new`/`/n_set`/`/n_map` would — the audio thread learns nothing new.

- **Group propagation.** `/n_set`/`/n_map`/`/n_mapa` addressed to a group walk the `TreeMirror` subtree (`control_targets` → `collect_subtree_synths`, recursing through subgroups, stopping at synths) and emit one `SetControl`/`MapControl` per descendant that has a control of that name (resolved through `node_defs`). Engine `SetControl`/`MapControl` on an unknown id are no-ops, so the fan-out is safe even against a node freed concurrently.

- **GraphDef.** `osc::graphdef` holds the `GraphDefSpec`/`GraphInstance`/`GraphVoice` types and a `RangeAllocator` (a contiguous-run busy map) over the reserved private-bus ranges (audio `96..128`, control `896..1024`). `/d_graph` parses, validates structurally and stores the spec (persisted by `defstore` as `defs/graphdefs/<name>.json`, reloaded after the synth/faust defs). `/graph_new` instantiates the **shared** members: a **fallible phase** first (pre-build the member synths via `make_synth`, allocate the private buses — freeing them back on a shortfall) so the **infallible phase** — `AddGroup` (auto-sorted, so the sort orders members by their bus wiring), member `AddSynth`s with their bus-selecting controls set to the allocated buses, `/n_map` wiring, then the resolved surface — never leaves a partial instance. The shared steps factor into `alloc_graph_buses`/`build_members`/`resolve_ports`, reused by `/graph_voice`, which spawns the **per-voice** members as a sub-group at the head of the instance (the sort then orders it before the shared mixer via the sub-group's aggregate `usage_of`). A resolved surface (`port → [(node_id, control_index, mul, add)]`) lives per instance and per voice; `/n_set` on an instance or a voice id is intercepted (`graph_set`) and routed through that surface, never the member ids. `/n_free` of a voice forgets it (`free_graph_node`); of an instance reclaims its private buses and drops its voices. A GraphDef bound to MIDI (`/midi_bind`) spawns the shared instance at bind time and each note becomes a `/graph_voice`. All of it also runs in NRT scores (the renderer shares `translate`).

### Preallocated capacities and what happens when they fill

Audited: `tests/capacity.rs` overflows each structure on purpose and pins the behavior below.

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
| IPC rings | 64 KiB each | backpressure: `push` fails, the producer retries; nothing is dropped (a full *reply* ring drops the reply with a log — the client stopped draining) |

**No wire-buffer or RT-memory pool (vs scsynth).** scsynth builds the synth graph *on the audio thread*, so it cannot allocate there and instead pre-sizes global pools at boot: wire buffers (`-w`), real-time memory for UGen-internal buffers like delay lines (`-m`), plus max-nodes/-buffers/-buses (`-n`/`-b`/`-a`/`-c`). Clausters builds each synth on the **network thread** (off the audio thread) and ships the finished node over the command FIFO, so the per-graph memory is owned per synth and freed per synth through the garbage FIFO: the inter-UGen **wires** are a `Vec<Block>` sized to the UGen count in `synthdef::instance` (cache-line-aligned via `Block`'s `#[repr(C, align(64))]`), and any future buffer-owning UGen (a delay line, say) allocates its ring in `new()` the same way. There is therefore **no `-w` and no `-m`** to size — that memory scales with the synths actually running, is contiguous and aligned, never fragments a pool, and never touches the audio thread. Only the counts in the table above are fixed at boot; of those, the **buses** are the meaningful synthesis capacity (the rest are throughput/headroom limits).

### Def persistence (`server::defstore`, `faust::cache`)

Loaded defs optionally outlive the process: when a data directory is configured (default on, `--data-dir`/`--no-persist`/`$CLAUSTERS_DATA_DIR`, resolved by `defstore::resolve_data_dir`), `/d_recv` and `/d_faust` write the def to disk and the server reloads everything on startup. The wire format and layout are in [`schemas.md`](schemas.md#persisting-defs-across-restarts); the mechanics:

- **Source of truth is the definition, never a compiled artifact.** A `FaustDef`'s factory is opaque LLVM JIT state and is *not* serialized. `defs/synthdefs/<name>.json` stores the `SynthDefSpec` verbatim; `defs/faustdefs/<name>.json` stores a `FaustRecord` (original source/JSON + libfaust version + payload SHA-256). Reloading recompiles from that — the same path a fresh `/d_recv`/`/d_faust` takes.
- **The Faust bitcode (`<name>.<sha>.bc`) is a non-authoritative speed cache** (layer "A"). On reload `faust::cache::try_restore` re-creates the factory from bitcode — skipping Faust's front-end — only if the libfaust version matches and the file reads cleanly; any miss falls back to a full compile and rewrites the cache. So a libfaust upgrade invalidates every `.bc`, and a corrupt cache can never serve a wrong def. The `.bc` is named by the payload's SHA so a stale file (interrupted overwrite) is never paired with a fresher record.
- **Threading.** Writes happen on the network thread (`/d_recv`) or the compiler thread (`/d_faust`, which holds the factory); both are non-RT. Reloads are queued on the compiler thread with `client = None` (no reply) and drained by `collect_faust_results`, so the socket serves immediately and a large library loads incrementally. All disk writes are atomic (temp file + rename).
- **MIDI-standalone.** `midi.json` persists the MIDI bindings (rewritten on every `/midi_bind`/`/midi_unbind`/`/midi_map`); `boot.json` is a user-authored preset of standalone GraphDefs. `attach_store` reloads in a fixed **boot order — defs → graphdefs → bindings → boot preset** — so a binding's instrument and a boot graph's name already resolve; restoring a GraphDef binding re-instantiates its shared instance (`CmdTranslator::restore_binding`). All of it runs at boot on the network thread, emitting the same pre-built commands as the live path, so the server comes up playable from a controller with no client.

## Clocks and scheduling

The engine publishes its **sample clock** — samples processed since start — as an `AtomicU64` (`EngineHandle::current_samples`). The conversion from an OSC NTP timetag to an absolute sample position happens on the **network thread** (`timetag_delta_secs` against the system clock, then delta × sample rate against the stream clock); the engine itself never looks at wall time. A timed bundle becomes `Cmd::Schedule { time, cmds }` in a pre-allocated, stably-sorted queue; when its sample falls inside a block, the block is genuinely **split** at that frame — `ProcessCtx` carries `offset` + `frames`, and every UGen/synth processes the sub-range — unlike scsynth, which quantizes to block boundaries and needs `OffsetOut`. The spent `Vec` shell of an executed bundle returns through the garbage FIFO (`Garbage::SpentBundle`) to be freed on the network side.

The NTP conversion is one of **two front-ends** to the same queue: `/sched` carries an absolute sample target directly — no wall clock involved — and `/clock` exposes the counter so clients can model the sample clock as their master timebase (see `docs/sample-clock.md`). In offline rendering, score timetags are seconds from render start, and the renderer pushes the same `Cmd::Schedule` commands — that single shared code path is the sample-identity guarantee, and it is why scheduling fixes must never fork between the two modes.

## Feedback (`LocalIn`/`LocalOut`)

A SynthDef graph is a **DAG by construction**: the compiler rejects any input that references a UGen at index `>= i` (`src/synthdef/mod.rs`), and UGens exchange whole blocks through wires processed in topological order. So a feedback loop cannot be wired directly — and intra-block, multi-UGen feedback is impossible in any block-processing engine, because the wire between two UGens only carries a *finished* block.

`LocalIn`/`LocalOut` give the SuperCollider answer: a synth-private feedback bus with **one control block (64 samples) of delay**. `LocalOut` writes a signal into `UGenSynth::locals[channel]` — a `Vec<Block>` that, unlike the per-UGen `wires`, **persists across `process_block` calls**. `LocalIn` reads that buffer. Because the compiler requires `LocalIn` to precede `LocalOut` for a channel, within a block `LocalIn` reads the buffer *before* `LocalOut` overwrites it — so it sees the previous block's value. That read-before-write order is the entire delay mechanism; no double buffering. It holds under mid-block schedule splits too: each slice reads then writes its own `[offset..offset+frames]` sub-range.

These two are the one place a UGen needs **synth-private** state that `ProcessCtx` (global, shared by the parallel scheduler) cannot carry, so `UGenSynth::process` handles them inline (matching on `def.ugens[i].kind`) instead of through the `UGen` trait; `src/dsp/local.rs` holds only placeholder structs. They touch no global bus, so `osc::graph::ugen_usage` gives them empty `BusUsage` and feedback synths still parallelize safely (private state, nothing shared). The channel index must be a constant (so the buffer is sized at compile time, `SynthDef::num_locals`).

The delay is **block-rate**, not sample-accurate: a one-channel loop is a comb resonating at `sampleRate / 64`. For a sample-accurate recursive filter (one-pole, biquad) or sub-block feedback, the loop must be fused into a single node — a recursive UGen written as one `process` with internal state (the `SinOsc` pattern), or a Faust def, whose `~` operator (`CboxRec`) compiles the whole loop into one `compute` with the state inside the instance. That fusion is exactly why `FaustSynth` exists.

## Invariants — do not break these

1. **The audio thread never allocates, frees, locks or does I/O.** `Engine::process_block` and everything it calls — including the parallel dispatch (atomics, bounded spins, at worst an `unpark`). Guarded by `tests/rt_safety.rs` (`assert_no_alloc`); new processing code must stay under that umbrella.
2. **Commands arrive fully built.** If a handler needs the audio thread to "finish" constructing something, the design is wrong.
3. **All incoming OSC bytes decode through `osc::decode_packet`.** Whatever the transport — UDP datagrams and IPC ring contents are equally untrusted, and one entry point keeps decoding (and any future hardening) in one place. It is a thin wrapper over `rosc::decoder::decode_udp`.
4. **RT and NRT render sample-identically.** Same engine, same schedule queue, same FPU mode — flush-to-zero is armed in the cpal callback, in `render()` *and* in every DSP worker at spawn, and Faust factories get `-ftz 2`. Keep all the call sites (`tests/denormals.rs`, Faust tail test in `tests/golden.rs`). Corollary: **parallel execution is bit-identical to sequential** — a stage only batches children with pairwise disjoint bus usage, verified by the engine against its own masks (`tests/parallel.rs`). Never weaken the stage partition rule: concurrent same-bus access is not just wrong ordering, it is the unsafe contract of `Buses::audio_mut` and of the per-slot `UnsafeCell`s in `NodeTree`.
5. **Buffers are immutable once installed.** Replace, never mutate. A recording UGen would need a new scheme — design it, don't poke holes.
6. **Synth output goes only through `Out`/`ReplaceOut`** (and the Faust reserved `out` mapping). There is no implicit output.
7. **The core builds and tests without the `faust` feature and without libfaust installed** (and without `realtime`/cpal: the renderer and the whole test suite run deviceless). Everything Faust hides behind `#[cfg(feature = "faust")]`.
8. **Binary boundaries are versioned.** The IPC segment layout and the embed C ABI share one version constant (`ipc::ABI_VERSION`), checked on attach/load; `tests/ipc.rs` pins the layout size. Any layout or C-ABI change bumps it — never ship an unversioned boundary (the scsynth plugin-ABI lesson).
9. **Determinism in tests.** Golden scenes must be reproducible: no wall-clock, no global seeds shared across parallel tests (`WhiteNoise` seeds from a global counter — keep it out of golden scenes), tolerances per `tests/golden.rs` (1e-4: libm differs across platforms, same machine is bit-exact).

## How to add a UGen

Using a hypothetical `Lag` (one-pole smoother, inputs `in` and `time`):

1. **DSP** — new file `src/dsp/lag.rs` (or extend a family file):

   ```rust,ignore
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

   Rules: no allocation/locks/I/O in `process` (the struct is built on the network thread — allocate there, in `new()`); read inputs with `at()` so constants, controls and wires all work; `output.len()` is the slice length, **not** `BLOCK_SIZE` — scheduled bundles split blocks. Only bus-touching UGens need `ctx.offset` (see `src/dsp/io.rs`): bus slices must be indexed at `offset..offset+frames`.
2. **Register** — `src/dsp/registry.rs`: add **one row to the `UGENS` table**, a `UGenDescriptor` giving the wire `name`, `arity`, `default_rate`, the allowed `rates`, its `exec` mode, `bus` role, whether it `needs_path`, and a `build` closure. Declare the module in `src/dsp/mod.rs`. That is the whole registration — there is no central `match kind`: the compiler and the bus analysis read the descriptor fields, so a def with the wrong input count, an unknown kind or a disallowed rate already fails in `/d_recv` with a pointed error, generically. A **variable-arity** UGen (like `EnvGen`, whose envelope array grows with the segment count) uses `Arity::Variadic`; the compiler then skips the exact-count check but still rejects a def whose inputs exceed `MAX_UGEN_INPUTS` (the per-synth input array is stack-sized to that bound). For a plain signal processor you copy a `SinOsc`-style row and change the name/arity/build; the rate fields default to audio-or-control rate (see the next section).
   A UGen that **frees its own node** (an envelope reaching its end) also implements the `done(&self) -> DoneAction` trait method; it is polled after every `process`, and `FreeSelf` routes the node to the tree's finished-node queue and out through the garbage FIFO (see "Done actions" above). Default is `DoneAction::None`.
3. **Tests** — a signal-level unit test (render offline, assert on the numbers: frequency by zero crossings, RMS, impulse/step response — see `tests/synthdef.rs` and the `audio-testing` skill). If the UGen has state, also assert it behaves across block splits (see `tests/scheduling.rs` patterns). RT-safety is covered as long as `process` follows rule 1 — `tests/rt_safety.rs` exercises the tree with `assert_no_alloc`, extend its scene if the UGen does something novel (first buffer reader, first bus feedback, …). Add a golden scene only if the UGen is meant to be regression-pinned (then regenerate with `cargo run --example render_golden` and **listen** before committing).
4. **Document** — the UGen kinds table in `docs/schemas.md` (name, inputs, output semantics), and manual steps in `GUIA.md` if it is user-visible.

**Adding an arithmetic operator, not a UGen.** Math operations (`min`, `clip2`, `midicps`, …) are *not* new kinds — they are entries in the two generic op UGens, `BinaryOpUGen`/`UnaryOpUGen` (`src/dsp/binop.rs`, `src/dsp/unop.rs`), which carry the operator by **name** in their `op` field. To add one you add a variant (its scalar formula, and a `name()`/`from_name` spelling) to the `BinaryOp`/`UnaryOp` enums in **`clausters_core::builtins`** — the shared crate — and nothing else: the compiler resolves the wire name to the op, the server's op UGens compute it, and the client's value FFI computes the same op off the RT path, so the two agree bit-for-bit (guarded by `tests/core_parity.rs`, which drives the whole table through the real `UGen::process`). This is the C0 "single source of truth for native ops" discipline; the registry's `op_family` field marks the two op-UGen rows so the compiler validates the operator name. Each op also has a stable integer id for the C ABI (`from_u32`), but that never crosses the wire — defs and clients use names. The fused `MulAdd`/`Sum3`/`Sum4` (`src/dsp/fused.rs`) are ordinary fixed kinds that compose the core operators. Prefer an operator over a new kind for anything expressible as a pure per-sample function of its inputs.

### Calculation rates (`ir`/`kr`/`ar`/`dr`)

Every UGen output carries an explicit **rate**, the same four scsynth uses, decided per instance in the def (`"rate": "kr"`) or by the kind's default. The rate is what makes the output wire's shape and schedule explicit instead of implied:

| rate | wire | when it runs | example |
|---|---|---|---|
| `ar` | full [`Block`] (one value/sample) | every block | `SinOsc`, `Out` |
| `kr` | length-1 (one value/block) | every block, once | a control-rate `Mul` |
| `ir` | length-1 (one value, then held) | **once, at synth init** | `SampleRate`, `Rand`, `BufFrames.ir` |
| `dr` | none — pulled | on demand by a driver | `Dseq` under `Demand` |

Downstream this is invisible: a `kr`/`ir` wire is a length-1 slice, which `at()` broadcasts as a constant, so an `ar` consumer reads it the same as a constant. The synth (`synthdef::instance`) sizes each output slice by the producer's rate, and reads `ir` wires straight back on later blocks.

Each UGen's rate metadata is data on its descriptor: **`default_rate`** (the rate when the def omits one) and **`rates`** (which rates the UGen implements). There is no central rate switch — a **new oscillator/filter/math UGen just sets `default_rate: Rate::Ar` and `rates: R_KR_AR`** (audio-or-control rate), the same as `SinOsc`. Only the exceptions differ in their row: the scalar `ir` UGens (`SampleRate`, `Rand`) and the `dr` source (`Dseq`) *widen or move* the set, and the whole-block I/O UGens *narrow* it to `ar` only (a length-1 wire would drop the block they read/write).

The compiler infers each output rate, checks it against the descriptor's `rates`, and validates coercion generically: lower rates widen into higher-rate inputs for free, so the only rejections are an **`ir` UGen fed anything non-`ir`** (it is frozen at init — a varying source can't be), and `dr` crossing the block boundary (below).

**The `ir` init pass.** `ir` UGens are computed **once**, on the first `process` block; their wire then holds the value for the node's life (wires persist across blocks) and the UGen is skipped from then on (an `initialized` flag). This runs on the audio thread — not in `UGenSynth::new` — because the value often needs `ctx` (the sample rate, the buffer pool), which only exists there; it stays RT-safe because an `ir` `process` only reads. `Rand.ir` is the sharp test: recomputing it would give a new number every block, so it only stays constant because the pass runs it exactly once.

**The `dr` sub-list contract.** A demand UGen is **not** in block-execution order. It is a sub-list its *driver* owns: the driver (`Demand`) pulls one value at a time through `UGen::demand`, the source (`Dseq`) yields the next value of its stream per pull (`NaN` = exhausted). The synth wires the two in `UGenSynth::process` exactly like `LocalIn`/`LocalOut` are special-cased: the source is skipped in block order and reached **only** through the driver's `step` callback, so there is a single mutable path to it (no aliasing) and no allocation. A `dr` wire may therefore feed **only** a demand driver's source slot — the compiler rejects a `dr` output anywhere else, and a driver whose source slot is not `dr`. This is the substrate the wider demand family (`Dseries`/`Dwhite`/`Duty`) builds on, and it shares its "not evaluated every block" idea with FFT's frame-rate chains.

## Faust integration notes

The lifecycle is factory (compiler thread, `ffi_lock`, `-single -ftz 2`, stdlib include) → `FaustDef` (factory + params probed once with `UIGlue`) → `FaustSynth` instance (created and `init`ed on the network thread, only `compute` runs on the audio thread, dies through the garbage FIFO holding its `Arc<FaustDef>` so the factory outlives every instance). Details and traps: the `faust-embedding` skill and `src/faust/*` module docs.

**Why Faust UI labels are the control names** (a deliberate decision, not a leftover): a def's parameters are named by whoever writes the def — exactly like the `controls` array in the UGen JSON format. Inventing a second naming layer (renaming, indices-only, a Clausters-side mapping table) would add a translation step for zero expressiveness: the label *is* the parameter's name in both def families, and `/s_new`/`/n_set` address both identically. Group paths (`hgroup`/`vgroup`) are ignored on purpose — bare labels, first declaration wins on collision. The two reserved names `out`/`in` (first output/input bus) come *after* the def's own params, and a def that declares its own `out`/`in` control wins over the reserved meaning. UI elements are plain values written by `/n_set` (zone stores, RT-safe) and can also be **bound to a bus** with `/n_map`/`/n_mapa` — the same mechanism unifies UGen controls and Faust zones (see *Control/bus mapping* above). The two reserved `out`/`in` routing controls are not mappable.

## Extending the server: the plugin question

scsynth grew a binary plugin API (`UnitCmd`/interface tables) whose ABI broke with struct or feature changes, and keeping out-of-tree UGens alive became a maintenance tax. Rust removes the temptation: **there is no stable Rust ABI**, so dynamically loaded Rust plugins are off the table in v1, by decision and not omission:

- Extending Clausters = adding code to the crate (the section above). The documented internal surface — `UGen` + `ProcessCtx` + the registry, and `SynthNode` for whole-synth implementations — is the contract, and it can evolve freely because everything compiles together.
- The *runtime*-extensible path for users is deliberately not Rust: send a **Faust def** (`/d_faust`, JIT-compiled, sandboxed by the same `SynthNode` boundary). Most "I need a custom UGen" cases are a Faust one-liner.
- If dynamic plugins ever become necessary, the lesson from scsynth applies: the boundary must be a **versioned C ABI** (or a wasm interface), checked at load time — the same policy already planned for the shared-memory layout and embed cdylib. Do not expose Rust types across it.
