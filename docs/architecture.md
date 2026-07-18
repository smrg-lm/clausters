# Architecture (developer documentation)

How Clausters is built, where everything lives, and the invariants a change must not break. User-facing documentation (wire formats, OSC commands) is in [`schemas.md`](schemas.md); the design decisions and upstream-bug findings are in [`decisions.md`](decisions.md), and the forward roadmap is `PLAN.md` at the repository root.

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
- **Audio thread** (the cpal callback, `server::backend`): runs `Engine::process_block` on 64-frame blocks. Per block: drain the command FIFO, fire scheduled bundles whose time falls inside the block (splitting it at the exact sample), walk the node tree in depth-first order, push dead memory to the garbage FIFO. It never allocates, locks or does I/O, and re-arms `dsp::denormals::flush_to_zero()` on every callback. Each block is **timed** for the CPU meter, and with the default `rtprio` feature it runs under **real-time scheduling** (SCHED_FIFO/RR) — see [Real-time health](#real-time-health-scheduling-cpu-meter-affinity).
- **Audio input thread** (the cpal *input* callback, `server::backend`, opt-in via `--inputs N`): a second cpal stream on the default input device. Its callback runs on **its own real-time thread** and only pushes the decoded interleaved f32 frames into a lock-free ring; at the top of each block `Engine::process_block` pops one block's worth into audio buses `outputs..outputs+inputs` (before any node runs), where `In` reads them like any bus. The two streams are decoupled by the ring: an overrun drops input samples on the callback side, an underrun reads as silence on the engine side — neither ever blocks. The output-channel count (`--outputs`) is negotiated with the host the same way as the sample rate (requested first, device default as fallback).
- **NRT thread** (`server::nrt`): all `/b_*` work — allocation, file reading (WAV via hound, compressed/other formats via symphonia), WAV writing via hound, zeroing. One queue, so commands on the same buffer complete in submission order. Produces immutable buffers the network thread installs with `Cmd::SetBuffer`.
- **Disk I/O threads** (`dsp::disk`, `DiskIn`/`DiskOut`): one background thread per streaming UGen instance, spawned at `/s_new` (build) and joined when the synth's `Box` is dropped on the network thread (via the garbage FIFO). It decodes (symphonia) or encodes (hound) between disk and a lock-free `rtrb` ring the audio thread pops/pushes; the audio thread never does disk I/O. Self-contained: no engine, OSC or `ProcessCtx` involvement.
- **DSP workers** (`server::workers`, opt-in via `--workers N`): a fork-join pool the audio thread conducts to process the stages of `/g_parallel` groups. Atomic work stealing, bounded spinning, park/unpark only across idle gaps; each worker arms flush-to-zero at spawn. With 0 workers (the default and the whole test suite) the pool is inert and everything is sequential.
- **Faust compiler thread** (`faust::compiler`, feature `faust`): JIT compilation of `/d_faust` defs. libfaust does not tolerate concurrent compilation in one process (SIGSEGV), so every compiling FFI call holds the process-wide `ffi_lock()`; instantiating from a finished factory is concurrency-safe and happens on the network thread.
- **TCP I/O threads** (`osc::tcp`, on by default at the OSC port; `--no-tcp` disables, `--tcp [port]` moves it): an acceptor thread plus one reader thread per connection turn each TCP byte stream into whole, length-prefixed OSC frames and hand them to the network thread over an `mpsc` channel — drained every loop iteration like the IPC ring. The command processing stays single-threaded; these threads only do I/O. A reader pings the network thread's own UDP socket with a **zero-length datagram** after queuing a frame, so the loop wakes immediately instead of waiting for the GC tick. Replies write back through the connection's write half (the network thread owns it; `&TcpStream` is `Write`, so a reply needs only a shared borrow). No async runtime, no new dependency.

  The TCP and WebSocket fronts share the **edge guards** (`osc::ClientSlots` and per-file constants): the `--max-clients` ceiling on live connections (one shared slot pool, a `SlotGuard` per connection thread, refusal at accept), **bounded** inbound channels (`sync_channel` — a flooding client blocks its own reader and TCP flow control pushes back to the sender, so the queue bounds server memory instead of growing), and slow-consumer eviction on the reply path (TCP: a socket write timeout so a client that stopped reading cannot stall the network thread; WebSocket: a bounded per-connection reply queue whose overflow drops the connection). The single decode door these fronts share, `osc::decode_packet`, has a fuzz harness in `fuzz/` (see the contributing chapter).
- **MIDI input thread** (`midi::live`, feature `midi`, opt-in via `--midi [name]`): `midir` opens a virtual ALSA-seq input port and runs the input callback on **its own thread**, which decodes each MIDI 1.0 message (`midi::parse_midi1`, widening to the internal high-resolution form) and hands the `ChannelVoiceMessage` to the network thread over an `mpsc` channel — drained every loop iteration, exactly like the TCP frames, with the same zero-length-datagram wake. The network thread does the actuation (`CmdTranslator::translate_midi` realizes each message as the equivalent `/s_new`/`/n_set`/`/n_free`); the MIDI thread only decodes. The audio thread is never involved.

**Offline rendering** (`server::render`, the `--nrt` CLI mode) uses no threads at all: one thread drives both halves of `engine_pair`, runs NRT jobs and Faust compilations synchronously between blocks (scsynth NRT semantics), and arms flush-to-zero once at the start. Because scheduled commands go through the same engine queue as in real time, an offline render is sample-identical to a perfectly timed live take.

The realtime backend (cpal) sits behind the `realtime` feature (on by default); the engine itself knows nothing about cpal, which is what makes the offline mode and the integration tests possible.

## Real-time health (scheduling, CPU meter, affinity)

The portable, always-on piece is the **CPU meter**; the default `rtprio` feature adds the Linux-specific scheduling around it. Together they answer "how many voices fit before the audio breaks", reliably (see `examples/stress.rs` for the harness that asks it).

- **CPU meter** (`Engine::process_block`, always on, portable): every block is timed with `Instant::now` (`clock_gettime(CLOCK_MONOTONIC)` through the vDSO — no allocation, no lock, no kernel trap, so it is RT-safe) and published through the `Counters` atomics as a fraction of the block budget: an average (EMA, ~1 s time constant), a peak (bitwise `fetch_max`; non-negative floats order like their bits) that resets on every read so each `/status` poll sees its own window, and a cumulative **late-block** counter (block wall time > budget — the engine-side xrun proxy; conservative when the device quantum spans several blocks). `/status.reply` reports all three (avg/peak as percentages in the scsynth fields that were previously zero, the late count appended). Offline renders run the same code and just measure render speed.
- **Real-time scheduling** (`rtprio` feature, default): cpal promotes the callback thread to SCHED_FIFO/RR through `audio_thread_priority` — **RTKit over DBus**, the same unprivileged path every PulseAudio/PipeWire client uses, so no `RLIMIT_RTPRIO` configuration is needed on a desktop. It is the default because it is the standard operating mode of every production audio client on Linux: without the promotion the DSP thread runs as SCHED_OTHER and competes with every normal process, and scheduling jitter — not throughput — breaks the audio well below 100% CPU (typically around half the theoretical single-core capacity; the callback must fit its *worst* block, and unscheduled peaks run at 2–3× the average). Because a promotion can fail silently, the callback publishes the policy the kernel **actually** gave it (one-shot syscalls on its 64th callback — a cold path) and the binary logs it shortly after boot: `audio thread is real-time: SCHED_RR priority 10`, or a warning naming the likely fix. RTKit promotes with `SCHED_RESET_ON_FORK` OR'd into the policy; the log masks it. Everything the feature compiles is isolated in `server::rt` (no-op stubs off Linux), so portability is unaffected and dropping the feature yields a build with no DBus dep and no RT watchdog — at the SCHED_OTHER capacity cost above.
- **CPU affinity** (`--pin cpu[,cpu...]`, part of `rtprio`): the first CPU pins the audio callback thread (it pins itself on its first callback — the thread is spawned deep inside cpal/PipeWire, so nobody else can reach it earlier); the remaining CPUs are assigned round-robin to the `clausters-dsp-N` workers by a `/proc/self/task` scan at boot. Pinning trades load-balancing for cache residency and steadier per-block times; measure it with `examples/stress.rs` rather than assuming it helps.

Real-time privileges cut both ways: RTKit imposes `RLIMIT_RTTIME` (~200 ms of *continuous* real-time CPU without blocking) on the thread it promotes, so a server driven past 100% sustained load — where the callback no longer sleeps between cycles — gets **SIGXCPU from the kernel**, whose default disposition kills the process with a core dump. That is the watchdog working as designed (a runaway SCHED_FIFO thread would otherwise freeze the machine); small device quanta (`PIPEWIRE_QUANTUM=64/48000`) reach that regime sooner because there is less slack per cycle. The `rtprio` binary therefore arms a **SIGXCPU guard** at boot, before the stream exists (`server::rt::install_sigxcpu_guard`): the handler demotes the audio thread back to SCHED_OTHER with async-signal-safe syscalls and writes one line to stderr — the audio degrades, **the server survives** (a restart re-promotes it). The overload rule is thus the same in every build: **overload breaks the sound, never the process**. Keep stress ramps below 100% anyway (the harness's default `--limit 90` exists for this). Note also that the real deadline is the **device quantum**, not the 64-frame engine block: the engine's late-block counter measures against the block budget, so with a large quantum it is a conservative early warning, and with quantum = 64 it is nearly the ground truth.

Two measured facts to keep in mind when reading the meter (numbers from a desktop at 48 kHz, release build): applying a **burst of node insertions** inside one block costs real budget (~5–10 µs per `/s_new` apply, plus the new synth's first-touch page faults — the wire buffers are allocated on the network thread but first *written* on the audio thread), so a client that adds hundreds of nodes at once can make that one block late without any sustained overload; and the **peak sits naturally at 2–3× the average** at low load (per-cycle wake-up cost, cache refill, frequency scaling), converging toward the average as the core saturates — capacity planning must budget for the peak.

**Logging** (`src/logging.rs`) uses `tracing`. The **binary** installs the subscriber (`logging::init`, stderr, level from `-v`/`-vv`/`-q` or `RUST_LOG`); the library and embed users do not, so the macros are no-ops for them. The filter is a runtime-reloadable `EnvFilter`, so the OSC commands `/verbosity` (set the level/directive) and `/dumpOSC` (overlay `clausters::osc=trace`) let a *client* retune the server's logs live. The audio thread never calls a `tracing` macro: every condition it detects (a rejected command, a dropped bundle) leaves through the garbage FIFO and is logged by the network thread, so logging stays clear of the real-time path.

## Module map

| Path | Contents |
|---|---|
| `src/server/engine.rs` | The core: `Engine` (audio half), `EngineHandle` (network half), `Cmd`, `Garbage`, the FIFOs, the schedule queue, the sample clock |
| `src/server/backend.rs` | cpal glue: `BlockAdapter` slices arbitrary callback sizes into 64-frame engine blocks (feature `realtime`) |
| `src/server/nrt.rs` | NRT thread, `NrtJob`/`run_job` (also called synchronously by the renderer), audio reading (`read_audio`: WAV via hound, other formats via symphonia) and WAV write helpers |
| `src/server/workers.rs` | worker pool: stage publish/steal/wait protocol for parallel groups |
| `src/server/ipc.rs` | the versioned shared segment — data plane (clock, control buses, audio-tap rings) + OSC byte rings (`--shm` and embed transports) |
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
| `src/osc/tcp.rs` | TCP transport (on by default): acceptor/reader threads, length-prefixed framing, `TcpHub` |
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
- **Audio taps**: single-channel sample rings in the IPC segment (`--taps` × `--tap-frames`; segment ABI v3). `/tap` sends `Cmd::SetTap`, which flips an entry in the engine's pre-allocated tap table; at the end of every block the audio thread appends each routed bus's block to its ring — one `memcpy` plus one Release store per tap, then the clock store, so a reader that sees clock N sees block N. Readers (the GUI host's oscilloscope over shared memory, `/tap_stream` on the network thread) copy the newest window lock-free with a cursor double-check; `tests/rt_safety.rs` guards the write.
- **Buffers**: `Arc<Buffer>`, **immutable once installed**. The NRT thread builds them, `Cmd::SetBuffer` swaps them into the engine pool, the replaced `Arc` returns as `Garbage::FreedBuffer`. "Mutating" commands (`/b_zero`, `/b_read` into an existing buffer, `/b_gen` filling a wavetable) build a replacement instead of touching shared memory. The network thread keeps a mirror for `/b_query`/`/b_write` and for validation.

### Done actions (self-freeing nodes)

A UGen can ask for its node to be freed, paused or resumed once it finishes: the `UGen` trait has a `done(&self) -> DoneAction` hook, checked right after `process`. `EnvGen` is the first user — it returns its `doneAction` when its last segment ends, one of scsynth's full **0–15** set (`DoneAction::from_i32` maps the float; see the table in `schemas.md`). The synth aggregates its UGens' actions (`UGenSynth::done_action` takes the strongest). The tree walk handles them by kind: **`None`** does nothing; **`PauseSelf`** is applied inline — the node's `paused` flag is set and it is skipped (silent, state kept) from the next block on. A paused node (synth **or** group — a paused group skips its whole subtree) is resumed by clearing that flag: the **`/n_run`** command (`Cmd::RunNode` → `NodeTree::set_paused`), which makes `PauseSelf` non-terminal, and the `freeSelfResumeNext` action. **Every other action** — `FreeSelf`, `FreeGroup`, and the relative ones — is recorded (id plus action code) into a **lock-free finished-node queue** (`NodeTree`'s `done_nodes`/`done_actions`/`done_count`, index reservation by `fetch_add` so the parallel workers can push concurrently), drained **once per block, after the whole tree has run** (`Engine::process_block` → `NodeTree::apply_done_action`). Freeing after the full walk (not mid-traversal) keeps the tree stable while it is being read, and makes a re-queue from a mid-block schedule split — or two synths both freeing the same group — harmless (freeing an already-gone id is a no-op). The queue's atomics only need to be race-free for the concurrent reservation; visibility to the drain is provided by the worker pool's join.

The **relative actions** (`freeSelfAndPrev`/`Next`, `freeSelfToHead`/`Tail`, `freeSelfPause`/`Resume` a neighbour, `freeSelfAndFreeAllIn`/`DeepFree` a neighbour group, `freeAllInGroup`) act on the finishing node's siblings. `apply_done_action` resolves the previous/next sibling (and, for the head/tail runs, iterates them) from the parent group's ordered child list — **before** freeing self, since freeing shifts positions — then reuses the ordinary `free`/`free_all`/`deep_free`/`set_paused` paths. All of that runs in the drain on the audio thread, so it stays allocation-free: sibling resolution is index arithmetic and the frees reuse the tree's pre-allocated `free_stack`/`dfs_stack`. Everything leaves on the garbage FIFO exactly like an `/n_free`, never dropped on the audio thread.

### Side-effect replies (`SendReply`/`SendTrig`/`Poll`)

A UGen can emit an **OSC reply or a console post** from the audio thread — `SendTrig` (`/tr`), `SendReply` (a custom address) and `Poll` (a console line, plus `/tr`). These carry a payload out (a node id, a reply id, a value list, a name), so unlike the POD node events they need a message type: `dsp::ReplyMsg`, a fully **inline, `Copy`** struct (a fixed name buffer + a fixed value array, no heap) so buffering and shipping one never allocates. Two hops keep it RT-safe and off the parallel-scheduler hot path: (1) during `process` each reply UGen detects triggers (a crossing from `≤ 0` to `> 0`) and buffers up to 8 messages in a fixed inline array of its own; (2) the tree marks any synth that ran a reply UGen into a **lock-free slot queue** (`NodeTree`'s `reply_slots`/`reply_count`, the same `fetch_add` index-reservation as the done-action queue, so the M13 workers can push while holding their slot), and `Engine::process_block` drains it **once after the whole block** — `NodeTree::drain_replies` walks the marked synths, stamps each message with the node id, and pushes it into the **reply FIFO** (`ReplyMsg`, SPSC, drained on the network thread). This mirrors the done-action discipline exactly: buffer on the audio thread, publish through the worker join, drain after the walk (a synth re-marked by a mid-block schedule split is harmless — the first drain empties its buffer, later ones find nothing). On the network thread `collect_garbage` turns each `ReplyMsg` into OSC: `Trig` → `/tr nodeID id value` to every `/notify` client, `Reply` → the custom address with `nodeID replyID value…`, `Poll` → a `tracing` console line (and a `/tr` when its trigid is set). The console post lands on the network thread, never the audio thread — the same reason the audio thread never calls a `tracing` macro. A def with only these UGens and no `Out` is valid (the compiler requires ≥1 UGen, never an `Out`); `UGenSynth` precomputes a `has_reply_ugens` flag so a synth without them is never marked or drained.

### Control/bus mapping (`/n_map`, `/n_mapa`)

`/n_set` writes a control once; `/n_map`/`/n_mapa` make a control **follow a bus**, re-read at the start of every block. Each synth carries a `ControlMap` table parallel to its controls (`node::ControlMap`, pre-allocated at build — `map_control` only flips an entry, never grows it). At the top of `process`, before any UGen runs, the synth pulls each live mapping into its control/zone: a control bus value (`/n_map`), or one frame of an audio bus sampled at control rate (`/n_mapa` — controls are one value per block, and Faust zones are scalar, so there is no audio-rate control). Writing straight to the control storage, never through `set_control`, keeps the mapping intact; a `/n_set` *does* go through `set_control`, which clears the mapping first, so an explicit set always wins (scsynth semantics). `Cmd::MapControl` carries it to the engine and is schedulable in bundles like `/n_set`.

This feeds the bus analysis: the network-side mirror records each node's live maps, and `fold_maps_into_usage` adds an audio map's bus to the node's `reads` and marks the node a dynamic barrier when a mapped control is used as a bus index — so auto/parallel groups stay correct under mappings.

### Typed controls (`tr`/`ir`/lag)

A control carries a **type** (`SynthDef::control_types`, from the def's `"rate"`) that shapes how the engine treats it. Two are handled inside `UGenSynth`, RT-safe:

- **Trigger (`tr`).** After the UGen loop, `process` resets every trigger control to `0`. A `/n_set` therefore holds for exactly the one block it landed in, so a UGen watching for a rising edge (an `EnvGen` gate) fires once. No extra state — the reset is unconditional and cheap.
- **Scalar (`ir`).** `set_control` ignores a write to a scalar control once `initialized` is set (the same S1 flag). The initial `/s_new` values are applied on the network thread before the first block, so they take; a later `/n_set` is dropped, matching scsynth. In the compiler an `ir` control counts as `Rate::Ir`, so it may feed an `ir` UGen input.

**Lag is not a control type — it is an inserted UGen.** A control with a `"lag"` compiles to a real `Lag` (or `VarLag` with `lag_down`) prepended to the graph, reading the raw control; every reference to that control is rewritten to the smoother's wire (`synthdef::compile`, the `lagged` pass, which shifts the original UGens down by the number of inserted smoothers and remaps their wire indices). This keeps **one** lag implementation shared with the client-facing `Lag` UGen — no bespoke control-smoothing path. The inserted smoothers run at audio rate, so a stepped control glides per sample toward its block-constant target.

### Node moves and the auto-order guard

`/n_before`/`/n_after`/`/g_head`/`/g_tail`/`/n_order` all lower to `Cmd::MoveNode { id, target, place }`, and `Place` (`node::Place`) is the four-way position: `Before`/`After` place next to a sibling (`target` is that sibling), `Head`/`Tail` place first/last in a group (`target` **is** that group). Both `NodeTree::move_node` (audio thread) and its `TreeMirror` twin resolve the destination group from `place` — the target's parent for sibling moves, the target itself for head/tail — then do the same reject checks (root, cycle, full group). `/n_order addAction target id...` places its first node relative to `target` and chains each following node `After` the previous one, so the listed order is preserved. The network thread rejects any manual move whose destination is an **auto-sorted group** (`TreeMirror::is_auto_group`) with `/fail`, since there the order is recomputed from the bus DAG — `/n_order` is therefore only meaningful in manual groups, where it is a batch of the pairwise moves.

### Range and query commands (`/n_setn`, `/c_getn`, `/s_get`, …)

The `-n`/range commands add no new engine `Cmd`s: `/n_setn`/`/n_fill` expand to `SetControl` per index, `/n_mapn`/`/n_mapan` to `MapControl` per index (buses advancing with the control), `/c_setn`/`/c_fill` to atomic writes (or `SetControlBus` in a bundle) per bus — all reusing the group-propagation and re-sort machinery of their scalar forms. The **query** commands are pure network-thread reads of the mirrors: `/s_get`/`/s_getn` read the node mirror's control values and reply `/n_set`; `/c_getn` reads the control-bus atomics and replies `/c_setn`; `/n_trace` logs a node's mirror state. `/s_noid` and `/b_close` are compatibility acknowledgements (Clausters does not reuse node IDs, and has no open streaming soundfiles yet).

### Group `/n_set`/`/n_map` and GraphDef

Both live entirely on the network thread, in `CmdTranslator`, and lower into the same `Cmd`s as a hand-written `/s_new`/`/n_set`/`/n_map` would — the audio thread learns nothing new.

- **Group propagation.** `/n_set`/`/n_map`/`/n_mapa` addressed to a group walk the `TreeMirror` subtree (`control_targets` → `collect_subtree_synths`, recursing through subgroups, stopping at synths) and emit one `SetControl`/`MapControl` per descendant that has a control of that name (resolved through `node_defs`). Engine `SetControl`/`MapControl` on an unknown id are no-ops, so the fan-out is safe even against a node freed concurrently.

- **GraphDef.** `osc::graphdef` holds the `GraphDefSpec`/`GraphInstance`/`GraphVoice` types; the reserved private-bus ranges (audio `96..128`, control `896..1024`) are `clausters_core::registry::Registry` occupancy maps (see "Finite-resource registries" below). `/d_graph` parses, validates structurally and stores the spec (persisted by `defstore` as `defs/graphdefs/<name>.json`, reloaded after the synth/faust defs). `/graph_new` instantiates the **shared** members: a **fallible phase** first (pre-build the member synths via `make_synth`, allocate the private buses, then the node ids — each step handing everything back on a shortfall) so the **infallible phase** — `AddGroup` (auto-sorted, so the sort orders members by their bus wiring), member `AddSynth`s with their bus-selecting controls set to the allocated buses, `/n_map` wiring, then the resolved surface — never leaves a partial instance. The shared steps factor into `alloc_graph_buses`/`build_members`/`resolve_ports`, reused by `/graph_voice`, which spawns the **per-voice** members as a sub-group at the head of the instance (the sort then orders it before the shared mixer via the sub-group's aggregate `usage_of`). A resolved surface (`port → [(node_id, control_index, mul, add)]`) lives per instance and per voice; `/n_set` on an instance or a voice id is intercepted (`graph_set`) and routed through that surface, never the member ids. `/n_free` of a voice forgets it (`free_graph_node`); of an instance reclaims its private buses and drops its voices. A GraphDef bound to MIDI (`/midi_bind`) spawns the shared instance at bind time and each note becomes a `/graph_voice`. All of it also runs in NRT scores (the renderer shares `translate`).

### Preallocated capacities and what happens when they fill

Audited: `tests/capacity.rs` overflows each structure on purpose and pins the behavior below.

| Structure | Capacity (default) | Boot flag | When full |
|---|---|---|---|
| Command FIFO | 1024 | — | reply `/fail … command FIFO full` (render mode: abort with the event time) |
| Garbage FIFO | 1024, scaled to `2 × max_nodes` at boot | (follows `--max-nodes`) | spills into a 64-slot holding list retried next block; if that also fills, the memory is **leaked** (`mem::forget`) — leaking is the only RT-safe option left |
| Event FIFO (`/n_go`/`/n_end`) | 2048, scaled to `2 × max_nodes` at boot | (follows `--max-nodes`) | events are POD and best-effort: dropped silently — the scaling keeps a full-tree mass-free deliverable, since the id registries recycle off `/n_end` |
| Reply FIFO (`SendReply`/`SendTrig`/`Poll`) | 2048 | — | POD reply messages, best-effort: dropped silently (plus a per-block per-UGen buffer of 8) |
| Schedule queue (timed bundles) | 1024 | — | the bundle is rejected and returned whole as a non-empty `Garbage::SpentBundle` (render mode: abort) |
| Node slab | 8192 (`node::MAX_NODES`) | `--max-nodes` | command rejected → `Garbage::Rejected*` |
| Node-id ranges (client / auto / MIDI) | 4× / 2× / 2× `max_nodes` (`registry::NodeIdPartition`) | `--max-nodes` | allocation refused with an explicit error; ids recycle on node end — see "Finite-resource registries" below |
| Children per non-root group | 512 (`node::MAX_GROUP_CHILDREN`) | `--max-graph-children` | command rejected → `Garbage::Rejected*` |
| Buffer pool | 4096 (`buffer::NUM_BUFFERS`) | `--max-buffers` | `/b_*` validates the index up front and replies `/fail` |
| Inputs per UGen | 32 (`dsp::MAX_UGEN_INPUTS`, hard ceiling) | `--max-ugen-inputs` | `/d_recv` rejects the def with `/fail` |
| Audio buses | 128 (`dsp::NUM_AUDIO_BUSES`, hard ceiling) | `--audio-buses` | bus-index inputs are clamped per block |
| Control buses | 16384 | `--control-buses` | out-of-range reads return 0.0, writes are ignored |
| IPC rings | 64 KiB each | `--shm` | backpressure: `push` fails, the producer retries; nothing is dropped (a full *reply* ring drops the reply with a log — the client stopped draining) |

The **boot-flag** column marks what is configurable at server start (S7): the four pool sizes plus the buses and the hardware I/O channels (`--outputs`/`--inputs`). Every one sizes a slab or `Vec` built **once at startup** — fixed at runtime, never at compile time — fed by the same config-file → flag precedence as the other options (`[server]` keys `max_nodes`, `max_buffers`, `max_graph_children`, `max_ugen_inputs`, `outputs`, `inputs`). Two carry a compile-time hard ceiling the flag clamps to, because they size a fixed-width structure the audio thread relies on: audio buses at 128 (the `BusUsage` mask is a `u128`) and UGen inputs at 32 (the per-UGen input list is a stack array in `synthdef::instance`). A client discovers the live values with `/server_info` (see `schemas.md`). `tests/capacity.rs` overflows the configurable slabs at both the default and a small custom size.

### Finite-resource registries (node ids, buses, buffers)

Ids over the pools above are themselves a finite resource, and every allocator of them — server- or client-side — is a **registry**: an occupancy map over a bounded range, in `clausters_core::registry` (one implementation, shared through the core FFI). The invariants: every released id is allocatable again; no monotonically increasing counter anywhere; no operation loses track of an id (a bad release is refused and reported, exhaustion is an explicit error, never a wrap). The node-id space is partitioned by `NodeIdPartition::from_max_nodes` — client range, `/s_new -1`/GraphDef auto range, MIDI voice range, all scaled from the node table's capacity — and the wire contract is in `schemas.md` ("Node ids are a finite, boot-partitioned resource").

Who holds which registry, and when ids come back:

- **`CmdTranslator.auto_ids`** (network thread) — the auto range. Released when the node's `End` event (or `Rejected*` garbage) drains in `OscServer::collect_garbage` / the NRT renderer's `collect`, both via `CmdTranslator::release_node_id`, which routes the id to the auto or MIDI registry by range.
- **`MidiBindings.ids`** (network thread) — the MIDI voice range, released through the same path as voices die.
- **GraphDef private buses** (`graph_audio_buses`/`graph_control_buses`) — reclaimed when the instance is freed; a refused release there is logged, because it means an instance lost track of a bus.
- **Clients** hold the client range plus their bus/buffer registries (the Python allocators wrap the same core `Registry` over ctypes) and recycle node ids from the `/n_end` notifications — which is why an engine rejection also broadcasts a `/fail` carrying the id: no `/n_end` will ever come for it.

NRT is the sanctioned exception: an offline score has no real-time bound on ids over its length and no live `/n_end` stream, so a score client's node-id registry is unbounded (`Registry::unbounded`); the server-side ranges recycle in NRT exactly as live (the renderer drains the same events). `tests/registry.rs` pins the recycling, the explicit exhaustion and the no-leak rollback of failed instantiations; the rationale for the model is in `docs/decisions.md`.

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

**`/clearSched`** (`Cmd::ClearSched`) flushes the whole queue: the engine `drain`s the schedule `Vec` (keeping its capacity — no dealloc on the audio thread) and ships each bundle's `Vec<Cmd>`, with its boxed synths, back as `Garbage::SpentBundle` to be freed on the network side. So even the panic button stays RT-safe.

## Feedback (`LocalIn`/`LocalOut`)

A SynthDef graph is a **DAG by construction**: the compiler rejects any input that references a UGen at index `>= i` (`src/synthdef/mod.rs`), and UGens exchange whole blocks through wires processed in topological order. So a feedback loop cannot be wired directly — and intra-block, multi-UGen feedback is impossible in any block-processing engine, because the wire between two UGens only carries a *finished* block.

`LocalIn`/`LocalOut` give the SuperCollider answer: a synth-private feedback bus with **one control block (64 samples) of delay**. `LocalOut` writes a signal into `UGenSynth::locals[channel]` — a `Vec<Block>` that, unlike the per-UGen `wires`, **persists across `process_block` calls**. `LocalIn` reads that buffer. Because the compiler requires `LocalIn` to precede `LocalOut` for a channel, within a block `LocalIn` reads the buffer *before* `LocalOut` overwrites it — so it sees the previous block's value. That read-before-write order is the entire delay mechanism; no double buffering. It holds under mid-block schedule splits too: each slice reads then writes its own `[offset..offset+frames]` sub-range.

These two are the one place a UGen needs **synth-private** state that `ProcessCtx` (global, shared by the parallel scheduler) cannot carry, so `UGenSynth::process` handles them inline (matching on `def.ugens[i].kind`) instead of through the `UGen` trait; `src/dsp/local.rs` holds only placeholder structs. They touch no global bus, so `osc::graph::ugen_usage` gives them empty `BusUsage` and feedback synths still parallelize safely (private state, nothing shared). The channel index must be a constant (so the buffer is sized at compile time, `SynthDef::num_locals`).

The delay is **block-rate**, not sample-accurate: a one-channel loop is a comb resonating at `sampleRate / 64`. For a sample-accurate recursive filter (one-pole, biquad) or sub-block feedback, the loop must be fused into a single node — a recursive UGen written as one `process` with internal state (the `Sine` pattern), or a Faust def, whose `~` operator (`CboxRec`) compiles the whole loop into one `compute` with the state inside the instance. That fusion is exactly why `FaustSynth` exists.

## The frequency-domain chain (`FFT`/`PV_*`/`IFFT`)

Spectral processing (`src/dsp/spectral.rs`) is the second consumer of synth-private state, and a second kind of "not every block": the **frame rate (`fr`)**. An `FFT` opens a chain, `PV_*` UGens transform the frame, an `IFFT` closes it back to audio. Like the demand (`dr`) sub-list, this is not block-execution as usual — but where `dr` is a *pull*, `fr` is a **hop gate**: `FFT` emits one spectral frame every hop's worth of input samples, and the `PV_*`/`IFFT` only do work on the slices a fresh frame is ready.

The chain UGens share one **`SpectralChain`** — the packed complex frame (`[dc, nyquist, re₁, im₁, …]`, the `clausters_core::fft::rfft_into` layout), a `ready` flag, and the hop `advance`. It is synth-private scratch in `UGenSynth::chains`, allocated at instantiation and persistent across blocks, exactly like the feedback `locals`. Each spectral UGen carries a compile-assigned **chain slot**: the `FFT` (`SpectralRole::Source`) gets a fresh slot, recorded in `SynthDef::spectral_sizes`; each `PV_*`/`IFFT` (`Filter`/`Sink`) follows its input-0 wire back to the upstream spectral UGen and **inherits** that slot (and window size, so the size is given only on the `FFT`). `UGenSynth::process` special-cases `ExecMode::Spectral` — it resolves the slot and calls `UGen::process_spectral(ctx, inputs, output, &mut self.chains[slot])`; the wire between the UGens only enforces topological order (`chains` is a distinct field from `ugens`, so both borrow mutably at once). This is the same "synth-coordinated execution" mechanism as `LocalIn`/`LocalOut` and the demand driver, generalized: the small closed set of `ExecMode`s the engine special-cases.

The transform stays RT-safe by construction: `FFT` keeps a sliding input buffer, `IFFT` an overlap-add tail and an output FIFO, all sized at build (network thread); the per-hop `rfft_into`/`irfft_into` run in pre-allocated scratch (`microfft` is zero-allocation). The `advance` carried on the chain keeps analysis and resynthesis in lockstep no matter how the hop relates to the block size (the hop is effectively quantized up to the processing slice, as scsynth transforms at block granularity). The `IFFT` overlap-add is **window-normalized**: it divides each output sample by a **precomputed steady-state overlap (COLA) denominator** — `Σ window[phase + i·hop]²`, one value per hop phase — so a bare `FFT`→`IFFT` reconstructs at unity gain (one window of latency), and a spectrally *modified* frame or the under-overlapped startup fades cleanly instead of over-amplifying where the window is small (dividing by a *running* partial window sum would blow up those edges). `FFT`/`IFFT` accept a live `/u_cmd window <wintype>` (the first consumer of the S6 per-UGen command surface) through `UGen::command`, rebuilding the window off any hop.

**Hop-phase stagger (S11).** A chain concentrates its whole per-hop cost on the block where the hop closes — a sawtooth load that averages (and the EMA CPU meter) underreport, and chains instantiated on the same block would all hop on the same block, stacking their spikes. So each `FFT` defers its *first* frame by a deterministic sub-hop offset — the node id modulo the hop's block count, in whole blocks — delivered through `SynthNode::set_node_id`/`UGen::set_node_id` when the engine inserts the node (the one point every add path funnels through: OSC, NRT scores, graphdef and MIDI voices, direct embedding; the call is arithmetic only, so it is RT-safe). Only the first fire shifts: the per-hop cadence, the analysis discipline and the chain's latency-to-content are untouched (`tests/spectral.rs::hop_stagger_shifts_only_the_first_frame` asserts the exact onset shift and steady-state sample equality), and the same score yields the same node ids, so RT and NRT renders stay sample-identical — the stagger is part of the deterministic output, not a scheduling heuristic. The `examples/bench.rs` spectral section measures both scenarios (ids congruent modulo blocks-per-hop reproduce the aligned worst case).

**Bin-expression programs (`PV_Kernel`, M29).** The general per-frame mechanism: instead of one registered row per spectral operation, `PV_Kernel` interprets a pair of **user-authored postfix programs** (magnitude and phase) over every bin of each fresh frame — `clausters_core::pvprog`, whose opcodes are the shared `builtins` operator discriminants plus per-bin loads (`mag`, `phase`, `bin`, `nbins`, `binfreq`, `p0`…). The RT discipline is split exactly like the rest of the engine: the def carries the programs as wire token lists (`mag_expr`/`phase_expr`), the SynthDef compiler resolves and validates them on the network thread (`pvprog::compile` — stack discipline, parameter arity against the UGen's variadic inputs, the 256-op length cap), and the audio thread runs `PvProgram::eval` as a fixed loop over a build-time stack — no allocation, pure `f32`, so RT ≡ NRT bit-exactly on every feature combination. Two cost-shaping details: an identity phase program keeps each bin's phase by *scaling* the complex pair (the same arithmetic as the curated magnitude filters — a kernel reimplementing one renders sample-identically, which `tests/spectral.rs` asserts), and the polar `atan2` runs only when some program actually reads `phase`. A program is a pure per-bin map by design; cross-frame state, bin remaps and two-chain reads stay curated rows (the ADR: "The spectral frame is user-programmable through a bin-expression program").

**Partitioned convolution (`Conv`, M28).** Convolution lives **outside** the `fr` chain (`src/dsp/conv.rs`): fast convolution's zero-padded rectangular segments, with a hop fixed by the partition size, are incompatible with the chain's windowed COLA discipline — a spectral multiply inside the chain would be *circular* convolution. The design is uniformly partitioned overlap-save with a frequency-domain delay line, shaped by two RT constraints. First, **the audio thread never transforms a kernel**: `/b_gen prepare_partconv` computes every partition's spectrum on the NRT queue into an ordinary immutable pool buffer (`dsp::conv::layout`), and a kernel swap is just the `kernel` input moving to another prepared buffer (one-partition crossfade; the FDL holds *input* history, which is kernel-agnostic, so nothing rebuilds). Second, **the load is flat**: the `p ≥ 1` partition MACs of the upcoming hop only involve spectra that already exist, so they are spread across the blocks between hops (a fair share per `process` call, sample-exact under block splits), leaving the hop block only the input FFT/IFFT pair and the fresh-spectrum MAC — the bench's conv section measures the resulting steady-vs-hop phase profile. `Conv` is also the first UGen with **intrinsic latency** (one partition), reported through the new `UGen::latency`/`SynthNode::latency` hook (`UGenSynth` sums its UGens') that M12 anticipated — informational until a delay-compensation pass exists (`docs/model-vs-daw.md`).

**Why not scsynth's buffer.** scsynth's chain threads the frame through a client-allocated buffer the audio thread mutates in place. That would violate the immutability invariant of the sample-buffer pool (below), so Clausters keeps the frame in synth-private scratch instead — no `/b_alloc`, the pool stays immutable, and the chain is freed with the synth (the `LocalBuf` model). Copying a frame into a pool buffer for inspection is a possible future extension.

## Invariants — do not break these

1. **The audio thread never allocates, frees, locks or does I/O.** `Engine::process_block` and everything it calls — including the parallel dispatch (atomics, bounded spins, at worst an `unpark`) and the CPU meter (`Instant::now` is a vDSO clock read, not a syscall). Guarded by `tests/rt_safety.rs` (`assert_no_alloc`); new processing code must stay under that umbrella. The only exception (`rtprio` builds only) is deliberate, one-shot and *before* steady state: the first callbacks run `server::rt::RtSetup` (optional self-pinning, one scheduling read), a handful of syscalls that never recur.
2. **Commands arrive fully built.** If a handler needs the audio thread to "finish" constructing something, the design is wrong.
3. **All incoming OSC bytes decode through `osc::decode_packet`.** Whatever the transport — UDP datagrams and IPC ring contents are equally untrusted, and one entry point keeps decoding (and any future hardening) in one place. It is a thin wrapper over `rosc::decoder::decode_udp`.
4. **RT and NRT render sample-identically.** Same engine, same schedule queue, same FPU mode — flush-to-zero is armed in the cpal callback, in `render()` *and* in every DSP worker at spawn, and Faust factories get `-ftz 2`. Keep all the call sites (`tests/denormals.rs`, Faust tail test in `tests/golden.rs`). One deliberate exception: the Faust **compiler** runs inside `dsp::denormals::normal_precision` — libfaust's front-end does real double math (interval typing, LLVM folding) that must see IEEE semantics even when the NRT renderer compiles scored defs on its armed render thread (its interval assertions abort the process on a flushed bound); the guard restores the armed mode on exit, so the processing invariant is untouched. Corollary: **parallel execution is bit-identical to sequential** — a stage only batches children with pairwise disjoint bus usage, verified by the engine against its own masks (`tests/parallel.rs`). Never weaken the stage partition rule: concurrent same-bus access is not just wrong ordering, it is the unsafe contract of `Buses::audio_mut` and of the per-slot `UnsafeCell`s in `NodeTree`.
5. **Buffers are immutable once installed.** Replace, never mutate. A recording UGen would need a new scheme — design it, don't poke holes.
6. **Synth output goes only through `Out`/`ReplaceOut`** (and the Faust reserved `out` mapping). There is no implicit output.
7. **The core builds and tests with any combination of the def-family features** — `synth` (SynthDefs/UGens, default) and `faust` (FaustDefs) are independent and combinable, and the engine core builds with neither (and without libfaust installed, and without `realtime`/cpal: the renderer and the whole test suite run deviceless). Everything Faust hides behind `#[cfg(feature = "faust")]`; the UGen library, the def compiler and `UGenSynth` hide behind `#[cfg(feature = "synth")]`. The node tree only knows `dyn SynthNode`, which is what keeps the families symmetrical.
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
2. **Register** — `src/dsp/registry.rs`: add **one row to the `UGENS` table**, a `UGenDescriptor` giving the wire `name`, `arity`, `default_rate`, the allowed `rates`, its `exec` mode, `bus` role, whether it `needs_path`, and a `build` closure. Declare the module in `src/dsp/mod.rs`. That is the whole registration — there is no central `match kind`: the compiler and the bus analysis read the descriptor fields, so a def with the wrong input count, an unknown kind or a disallowed rate already fails in `/d_recv` with a pointed error, generically. A **variable-arity** UGen (like `EnvGen`, whose envelope array grows with the segment count) uses `Arity::Variadic`; the compiler then skips the exact-count check but still rejects a def whose inputs exceed `MAX_UGEN_INPUTS` (the per-synth input array is stack-sized to that bound). For a plain signal processor you copy a `Sine`-style row and change the name/arity/build; the rate fields default to audio-or-control rate (see the next section).
   A UGen that **frees its own node** (an envelope reaching its end) also implements the `done(&self) -> DoneAction` trait method; it is polled after every `process`, and `FreeSelf` routes the node to the tree's finished-node queue and out through the garbage FIFO (see "Done actions" above). Default is `DoneAction::None`.
   A UGen that takes **out-of-band commands** (an FFT setting a buffer, a streaming reader seeking) implements the `command(&mut self, cmd: &UGenCmd)` trait method: `/u_cmd nodeID ugenIndex name args...` validates the target on the network thread, hashes `name` to a stable `selector` (`dsp::ugen_cmd_selector`), packs the numeric args inline (no heap crosses to the audio thread), and `Cmd::UGenCommand` routes them to `SynthNode::ugen_command` → `ugens[index].command`. Match `self`'s selectors against `ugen_cmd_selector("myCommand")`; unknown selectors are a no-op. Default ignores every command. It runs on the audio thread — stay allocation-free.
   A UGen that emits an **OSC reply or console post** (like `SendReply`/`SendTrig`/`Poll`) returns `true` from `is_reply()` and buffers `ReplyMsg`s during `process` into a fixed inline array, draining them in `drain_replies(node_id, sink)`; the tree marks such synths and the engine ships the messages out through the reply FIFO after the block (see "Side-effect replies" above). Both defaults are no-ops, so a normal UGen ignores this. Stay allocation-free — the `ReplyMsg` is `Copy` and inline for exactly that reason.
   A UGen in a **spectral chain** (`FFT`/`PV_*`/`IFFT`) sets `exec: ExecMode::Spectral` and a `spectral` role (`Source`/`Filter`/`Filter2`/`Sink`), and implements `process_spectral(ctx, inputs, output, chain)` instead of `process`: it reads/writes the synth-private `SpectralChain` the synth resolves by its compile-assigned slot (see "The frequency-domain chain" above). Only `FFT` (a `Source`) declares a window size (`config.fft_size`, sized at `build`); the compiler propagates it and the slot to the downstream filters/sink. A **two-chain combiner** (`Filter2`, M27) takes chain wires as inputs 0 and 1 and implements `process_spectral_pair(…, a, b)` instead: the compiler validates equal window sizes and distinct slots (recording the second as `chain_slot_b`), and the synth split-borrows the two chains; the result lands in chain A, whose slot the combiner passes downstream. Before adding a new `PV_*` name at all, read the curated-set stance in `docs/decisions.md` ("The PV set is curated and parameterized") — an operation is a *parameter* of an existing implementation wherever possible, and one outside the curated set that is a pure per-bin map is a **`PV_Kernel` bin expression** (M29, the general mechanism — no new row at all); only an operation that genuinely needs new state or access patterns joins the catalog.
3. **Tests** — a signal-level unit test (render offline, assert on the numbers: frequency by zero crossings, RMS, impulse/step response — see `tests/synthdef.rs` and the `audio-testing` skill). If the UGen has state, also assert it behaves across block splits (see `tests/scheduling.rs` patterns). RT-safety is covered as long as `process` follows rule 1 — `tests/rt_safety.rs` exercises the tree with `assert_no_alloc`, extend its scene if the UGen does something novel (first buffer reader, first bus feedback, …). Add a golden scene only if the UGen is meant to be regression-pinned (then regenerate with `cargo run --example render_golden` and **listen** before committing).
4. **Document** — the UGen kinds table in `docs/schemas.md` (name, inputs, output semantics), and a smoke step in `GUIA.md` if it adds audible behavior worth checking by ear.

**Adding an arithmetic operator, not a UGen.** Math operations (`min`, `clip2`, `midicps`, …) are *not* new kinds — they are entries in the two generic op UGens, `BinaryOpUGen`/`UnaryOpUGen` (`src/dsp/binop.rs`, `src/dsp/unop.rs`), which carry the operator by **name** in their `op` field. To add one you add a variant (its scalar formula, and a `name()`/`from_name` spelling) to the `BinaryOp`/`UnaryOp` enums in **`clausters_core::builtins`** — the shared crate — and nothing else: the compiler resolves the wire name to the op, the server's op UGens compute it, and the client's value FFI computes the same op off the RT path, so the two agree bit-for-bit (guarded by `tests/core_parity.rs`, which drives the whole table through the real `UGen::process`). This is the C0 "single source of truth for native ops" discipline; the registry's `op_family` field marks the two op-UGen rows so the compiler validates the operator name. Each op also has a stable integer id for the C ABI (`from_u32`), but that never crosses the wire — defs and clients use names. The fused `MulAdd`/`Sum3`/`Sum4` (`src/dsp/fused.rs`) are ordinary fixed kinds that compose the core operators. Prefer an operator over a new kind for anything expressible as a pure per-sample function of its inputs.

### Calculation rates (`ir`/`kr`/`ar`/`dr`)

Every UGen output carries an explicit **rate**, the same four scsynth uses, decided per instance in the def (`"rate": "kr"`) or by the kind's default. The rate is what makes the output wire's shape and schedule explicit instead of implied:

| rate | wire | when it runs | example |
|---|---|---|---|
| `ar` | full [`Block`] (one value/sample) | every block | `Sine`, `Out` |
| `kr` | length-1 (one value/block) | every block, once | a control-rate `Mul` |
| `ir` | length-1 (one value, then held) | **once, at synth init** | `SampleRate`, `Rand`, `BufFrames.ir` |
| `dr` | none — pulled | on demand by a driver | `Dseq` under `Demand` |

Downstream this is invisible: a `kr`/`ir` wire is a length-1 slice, which `at()` broadcasts as a constant, so an `ar` consumer reads it the same as a constant. The synth (`synthdef::instance`) sizes each output slice by the producer's rate, and reads `ir` wires straight back on later blocks.

Each UGen's rate metadata is data on its descriptor: **`default_rate`** (the rate when the def omits one) and **`rates`** (which rates the UGen implements). There is no central rate switch — a **new oscillator/filter/math UGen just sets `default_rate: Rate::Ar` and `rates: R_KR_AR`** (audio-or-control rate), the same as `Sine`. Only the exceptions differ in their row: the scalar `ir` UGens (`SampleRate`, `Rand`) and the `dr` source (`Dseq`) *widen or move* the set, and the whole-block I/O UGens *narrow* it to `ar` only (a length-1 wire would drop the block they read/write).

The compiler infers each output rate, checks it against the descriptor's `rates`, and validates coercion generically: lower rates widen into higher-rate inputs for free, so the only rejections are an **`ir` UGen fed anything non-`ir`** (it is frozen at init — a varying source can't be), and `dr` crossing the block boundary (below).

**The `ir` init pass.** `ir` UGens are computed **once**, on the first `process` block; their wire then holds the value for the node's life (wires persist across blocks) and the UGen is skipped from then on (an `initialized` flag). This runs on the audio thread — not in `UGenSynth::new` — because the value often needs `ctx` (the sample rate, the buffer pool), which only exists there; it stays RT-safe because an `ir` `process` only reads. `Rand.ir` is the sharp test: recomputing it would give a new number every block, so it only stays constant because the pass runs it exactly once.

**The `dr` sub-list contract.** A demand UGen is **not** in block-execution order. It is a sub-list its *driver* owns: the driver (`Demand`) pulls one value at a time through `UGen::demand`, the source (`Dseq`) yields the next value of its stream per pull (`NaN` = exhausted). The synth wires the two in `UGenSynth::process` exactly like `LocalIn`/`LocalOut` are special-cased: the source is skipped in block order and reached **only** through the driver's `step` callback, so there is a single mutable path to it (no aliasing) and no allocation. A `dr` wire may therefore feed **only** a demand driver's source slot — the compiler rejects a `dr` output anywhere else, and a driver whose source slot is not `dr`. This is the substrate the wider demand family (`Dseries`/`Dwhite`/`Duty`) builds on, and it shares its "not evaluated every block" idea with FFT's frame-rate chains.

## Faust integration notes

The lifecycle is factory (compiler thread, `ffi_lock`, `-single -ftz 2`, stdlib include) → `FaustDef` (factory + params probed once with `UIGlue`) → `FaustSynth` instance (created and `init`ed on the network thread, only `compute` runs on the audio thread, dies through the garbage FIFO holding its `Arc<FaustDef>` so the factory outlives every instance). Details and traps: the `faust-embedding` skill and `src/faust/*` module docs.

**Why Faust UI labels are the control names** (a deliberate decision, not a leftover): a def's parameters are named by whoever writes the def — exactly like the `controls` array in the UGen JSON format. Inventing a second naming layer (renaming, indices-only, a Clausters-side mapping table) would add a translation step for zero expressiveness: the label *is* the parameter's name in both def families, and `/s_new`/`/n_set` address both identically. Group paths (`hgroup`/`vgroup`) are ignored on purpose — bare labels, first declaration wins on collision. The two reserved names `out`/`in` (first output/input bus) come *after* the def's own params, and a def that declares its own `out`/`in` control wins over the reserved meaning. UI elements are plain values written by `/n_set` (zone stores, RT-safe) and can also be **bound to a bus** with `/n_map`/`/n_mapa` — the same mechanism unifies UGen controls and Faust zones (see *Control/bus mapping* above). The two reserved `out`/`in` routing controls are not mappable.

## The arrangement layer: where it lives

The arrangement model and its multitrack editor are **client-side** — the server
knows nothing of them, and nothing in this section runs on the audio thread. What
makes the layer thin is that every one of its concepts is a name for something the
system already had; this is the map, for when you have to change one and want to
know what else it touches. (The user-facing explanation is the composition chapter
of the Python client's book; the reasoning behind it is in
[Design decisions](decisions.md).)

| Concept | What it already was | Where |
|---|---|---|
| Element (onset, duration) | a placed item on a timeline | `clients/python/clausters/form/element.py`, `seq/timeline.py` |
| Group, concrete | a `Timeline` (client) projected onto server groups and timetagged bundles | `form/group.py`, `seq/timeline.py`, `src/node/mod.rs` |
| Group, logical | a `GraphDef` — the bus-wired configuration the server already expresses | `form/group.py` (`to_graphdef`), `defs/graphdef.py`, `src/osc/graphdef.rs` |
| Event | `seq.Event` (parameters in one action) | `seq/event.py` |
| List (order, no concrete time) | a Python list, or a `Pattern` | `seq/pattern.py` |
| Buffer (a list at constant time) | `defs.Buffer` over the server's immutable buffers | `defs/buffer.py`, `src/dsp/buffer.rs` |
| Set (mixed placement — a track) | `seq.Timeline` | `seq/timeline.py` |
| Function (a process) | a def (`SynthDef`/`FaustDef`/`GraphDef`) **or** a `Pbind`/`Routine` | `defs/`, `seq/pattern.py`, `base/stream.py` |
| Automation (a curve) | an `Env` discretized into a control buffer, read onto a bus | `seq/automation.py`, `/b_gen "env"`, `src/dsp/io.rs` (`OutCtl`) |
| Change of state (generator → generated) | evaluating a def or bouncing a pattern | `Timeline.from_pattern`, `session.py`, `src/server/render.rs` |
| Rendering (in time) | timetagged bundles (RT) or a `Score` (NRT) — one flattening, two destinations | `form/render.py`, `seq/timeline.py` (`Playhead`), `src/server/render.rs` |
| The editor driver (data ↔ view) | — the one piece that is new, and the only one that knows both | `clients/python/clausters/gui/editor.py` |
| Graphic unit (a clip: length = duration) | the placed rectangle and its bodies | `clients/gui/src/host/track.rs` |
| Base level (coarser or finer) | the LOD rule, and a group collapsed to a summary or resolved into lanes | `clients/gui/src/{waveform,spectrogram}.rs`, `gui/editor.py` |
| Shared time axis, playhead, cursor | the navigation groups (linked views), grown to hold lanes | `clients/gui/src/host/timeline.rs` |

Two boundaries hold this together, and both are worth defending: `clausters.form`
imports **nothing** from the GUI (it is pure and transport-agnostic — the piece a
future client factors into the shared core), and the driver is the **only**
converter between the arrangement's beats and the view's timeline samples.

## The GUI host: structure, and how to add a widget

The GUI (`clients/gui`) is a **separate process**, not code linked into the audio
server — it is a peer that speaks the `/gui_*` protocol to the language clients
and, with its own client leg, speaks the audio protocol to the server (see
[Clients and language bindings](clients.md) for the why). Its internals follow one
split, and every rule below falls out of it:

| Path | Contents |
|---|---|
| `src/host/mod.rs` | The protocol core: the `Host`, the command dispatch (`/gui_def`/`/gui_set`/`/gui_free`/`/gui_query`/`/gui_bind`), the `Registry`, `HostEffect` |
| `src/host/widget.rs` | `WidgetKind` — the typed tree the renderer reads — plus its JSON parse and its `/gui_set` `apply` |
| `src/host/frame.rs` | The one frame renderer: places the tree (`layout`), builds the flat-geometry mesh and uploads the heavy GPU views. **Both fronts call it**, so the browser is pixel-faithful by construction |
| `src/host/{gui,web}.rs` | The two fronts: native (winit/wgpu, sockets, mmap) and browser (canvas/WebGPU, WebSocket, fetch). Event *sources* and *sinks* only |
| `src/host/interact.rs` | Pointer logic over the typed tree — hit-test, value writes, the edit-back payloads — shared by both fronts |
| `src/host/gestures.rs` | The one press → drag → release → wheel state machine **both fronts drive**: it mutates the `Host` through the `interact` doors and returns `GestureEffect`s (emit/redraw/release-pointer) for the front's own sinks, so every editing gesture behaves identically on either platform by construction |
| `src/host/{track,pianoroll,bpf,plot,graph,nodetree,meters,…}.rs` | One module per flat view: pure over a `Mesh`, unit-tested without a window. `pianoroll` is the note core (the notes, their mapping, drawing, hit-test, editing) **shared** by the dedicated `pianoroll` widget and the multitrack `clip`'s roll body, so the two never disagree — the `bpf::place_point` reuse move again |
| `src/{waveform,spectrogram,viewport}.rs` | The heavy GPU views and the navigation window (`View`) |
| `src/host/timeline.rs` | The navigation **groups**: the shared window/selection/playhead of linked views and of the multitrack's aligned lanes |
| `src/host/{bulk,fetch,shm,mapfile}.rs` | The data seams: a local resource is mapped (native) or fetched (browser), a server buffer is pulled over the client leg, control buses are read from the shared segment |

Four rules hold the design together, and breaking any of them is what a review
looks for:

- **Generic on the wire, typed in the renderer.** A GuiDef is `{id, type, props,
  children}` and nothing more. A new widget is a new `WidgetKind` variant plus a
  handler — the `/gui_*` vocabulary never grows for it, and an unknown type is
  laid out but not painted, so an old host and a new script still interoperate.
- **Never resolve the signal finer than the screen.** Work is bounded by
  `samples_per_px`; the expensive analysis (a peak pyramid, an STFT) is a *cache*
  reached through a local shared resource, never chunked over the wire.
- **Value math belongs to the core, display math to the host.** Anything a
  headless client could also want — envelope shapes, peak pyramids, FFT windows,
  scales, tempo/sample arithmetic — lives in `clausters-core` and is *shared*;
  only geometry, hit-testing and chrome are gui-side. This is why the `bpf`
  editor and the server's `EnvGen` cannot drift: they evaluate the same function.
- **Edit-back is a payload, not an address.** A view that writes data back emits
  `/gui_event <id> <tag> <flat values…>` — `"points"` for a curve, `"clip"` for a
  placement, `"wire"` for a connection. The widget tree stays the source of truth
  on the host side; the script's model stays it on the other.

### Adding a widget

Take a hypothetical `meterbar`. The steps are always the same:

1. **The typed kind** — a `WidgetKind::MeterBar { … }` variant in
   `host/widget.rs`, its arm in the JSON parse (`Widget::build`) and, for every
   live-updatable prop, an arm in `WidgetKind::apply` (that is `/gui_set`).
2. **The view** — a module `host/meterbar.rs`, pure over a `Mesh`: layout, draw,
   and (if it is interactive) a hit-test. No GPU, no platform: that is what makes
   it unit-testable, and the tests go beside it. A *heavy* view (one that needs a
   texture or a vertex buffer) instead gets a GPU slot in `frame.rs` and follows
   the LOD rule above.
3. **The frame** — collect the placed widget in `frame::render` and draw it. Base
   mesh for the view, overlay mesh for chrome that must read on top (selection,
   playhead, wires in flight).
4. **Interaction, if any** — the hit-test and the mutation go in
   `host/interact.rs` (shared, so both fronts behave the same); the *gesture*
   (which button, which drag state) belongs to the front. If the widget writes
   data back, add its flat event payload here — never a new OSC address.
5. **The Python builder** — a function in `clients/python/clausters/gui/guidef.py`
   returning the node dict, with the docstring that *is* the widget's user
   reference (the API page is generated from it).
6. **Tests and an example** — pure tests for the layout/hit-test/edit, and a
   `clients/python/examples/gui_*.py` when the widget is user-facing.

A **new element kind** (a new primitive of the arrangement) is the client's
business, not the host's: it lands in `clients/python/clausters/form/`, and it
reaches the screen through the editor driver (`clausters/gui/editor.py`), which is
the only module that knows both the arrangement and the widget tree. The Python
book's composition chapter is its user documentation.

## Extending the server: the plugin question

scsynth grew a binary plugin API (`UnitCmd`/interface tables) whose ABI broke with struct or feature changes, and keeping out-of-tree UGens alive became a maintenance tax. Rust removes the temptation: **there is no stable Rust ABI**, so dynamically loaded Rust plugins are off the table in v1, by decision and not omission:

- Extending Clausters = adding code to the crate (the section above). The documented internal surface — `UGen` + `ProcessCtx` + the registry, and `SynthNode` for whole-synth implementations — is the contract, and it can evolve freely because everything compiles together.
- The *runtime*-extensible path for users is deliberately not Rust: send a **Faust def** (`/d_faust`, JIT-compiled, sandboxed by the same `SynthNode` boundary). Most "I need a custom UGen" cases are a Faust one-liner.
- If dynamic plugins ever become necessary, the lesson from scsynth applies: the boundary must be a **versioned C ABI** (or a wasm interface), checked at load time — the same policy already planned for the shared-memory layout and embed cdylib. Do not expose Rust types across it.
