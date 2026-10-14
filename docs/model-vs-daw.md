# The node-tree model and DAWs (design rationale)

Why Clausters keeps SuperCollider's node-tree model, the trade-off it makes against the channel-graph model of a digital audio workstation (DAW), and — in detail — how the auto-sorted-group dependency analysis relates to a DAW's plugin delay compensation (PDC), including the general server compensation pass and its analysis boundaries. This is background and rationale; the user-facing behavior of auto-sorted groups is in [`auto-order.md`](auto-order.md), and the implementation invariants are in [`architecture.md`](architecture.md).

## Three models in one sentence

- **DAW (channel graph).** A fixed, hierarchical graph of *channels*. Each track is a linear chain of insert plugins feeding buses/groups and finally the master; sends branch off to return buses. The user draws the topology in a UI and the engine derives the execution order from the routing. The unit is the *channel*, not the *voice*.
- **scsynth (node tree).** An ordered *tree* of nodes (groups and synths). Execution order **is** the depth-first traversal of the tree, and nothing else. Signal routing is separate, through globally numbered *buses*. Making "who runs first" agree with "who writes the bus another reads" is the client's manual job (add actions, `/node_before`, `/node_after`).
- **Clausters.** The same tree and buses as scsynth for the baseline semantics, plus three departures: it can **infer execution order from the bus routing** (auto-sorted groups, `/group_sortMode`) — recovering the DAW's automatic ordering without giving up the tree — it **aligns declared intrinsic latency across automatic bus paths**, and it **builds every synth off the audio thread**, which removes several historical limits of scsynth (RT/wire memory pools, block-boundary scheduling).

## Axis-by-axis comparison

### Topology and expressiveness

| | Node tree (SC / Clausters) | Channel graph (DAW) |
|---|---|---|
| Unit | Voice / synth, ephemeral | Channel, persistent |
| Structure | Order tree + orthogonal buses | Fixed routing graph |
| Arbitrary routing | Total: any synth to any bus | Bounded to inserts + N sends |
| Dynamic voices | Native (spawn/free a synth per note) | Outside the model (handled inside a plugin) |

The tree's strength is that **order is orthogonal to routing**. The signal graph is not wired into the structure; it is emergent from which buses each node reads and writes, and it can be reconfigured live without rebuilding anything. The granularity is the *voice*, not the channel — which is why the model expresses granular synthesis, massive polyphony and structures that appear and vanish, all of which a DAW only does *inside* a plugin (a black box).

That same orthogonality is the model's main cost. In a DAW "this runs after that" is *visible* in the routing. In the node tree, order and routing are two independent things the user must keep consistent by hand: a synth placed at the wrong point of the tree produces silence or a one-block delay, with no error. This is the central ergonomic defect of the model, and the one auto-sorted groups address (see below).

A DAW's channel has one more thing a bare node has not: it is **called
something**. A channel is addressed by what it is — the drum bus, the reverb
send, the master — while a node is addressed by a number the client happened to
allocate. Clausters closes that gap with **group names** (`/group_name`): a
referenceable label on top of the ID, unique among siblings, that makes the tree
navigable by path (`/mixer/drums`, resolved with `/group_query`). The ID stays
the identity — nothing addresses a group by name — but a mixer's channels, its
sends and its master become sayable, which is what a channel strip, a console
group or a DAW send needs to be built out of groups at all. See
[`schemas.md`](schemas.md#naming-a-group-group_name-group_query).

### Execution order, feedback and cycles

A DAW resolves order by analysing the routing graph, and hides cycles behind an implicit buffer delay on return sends. In the node tree, order is the tree, and feedback is made explicit with buses and a read-before-write across nodes, which introduces **one control block (64 samples) of delay** — the same as a return send, but visible.

A SynthDef graph is a **DAG by construction** (the compiler rejects an input referencing a later UGen), so a feedback loop cannot be wired directly; `LocalIn`/`LocalOut` give the synth-private one-block feedback path, and the auto-sort **does not "solve" cycles** — it preserves the members' current relative order, which is exactly one block of delay. This is honest: sample-accurate feedback *within* a block is impossible in any block-processing engine (the wire between two UGens carries only a finished block), DAWs included; they merely hide it. For a sample-accurate recursive filter the loop must be fused into a single node — a recursive UGen, or a Faust def whose `~` operator compiles the whole loop into one `compute`. See the *Feedback* section of [`architecture.md`](architecture.md).

### Timing and scheduling

This is where the synthesis-server model, and Clausters' implementation in particular, is structurally ahead of the typical DAW. A DAW ties scheduling to the transport and to PDC, and in practice quantizes automation and events to the buffer size. scsynth carries timetagged bundles but **quantizes commands to the block boundary** (hence `OffsetOut`). Clausters **splits the block at the exact sample**: `ProcessCtx` carries `offset` + `frames` and every UGen processes the sub-range. It also renders **bit-identical between real time and offline** (same engine, same schedule queue, same FPU mode), so an offline render equals a perfectly timed live take — something a DAW rarely guarantees between playback and bounce. See *Clocks and scheduling* in [`architecture.md`](architecture.md).

### Concurrency and parallelism

A DAW parallelizes per channel automatically and transparently — its historical strength. scsynth's engine is single-threaded (parallel groups exist only in a fork). Clausters has an opt-in fork-join pool (`--workers N`) that processes the stages of parallel groups (`/group_parallel`) with the guarantee that **parallel execution is bit-identical to sequential**: a stage only batches children with pairwise-disjoint bus usage, verified against the engine's own masks. The trade-off is that it stays explicit (you mark groups parallel) rather than the DAW's automatic per-channel parallelism, and it only pays off when the graph actually has disjoint parallelism.

### Memory and RT-safety

This is where the implementation departs most from both scsynth and a plugin host. scsynth builds the synth graph *on the audio thread*, so it cannot allocate there and instead pre-sizes global pools at boot: wire buffers (`-w`), real-time memory for UGen-internal buffers (`-m`), plus max nodes/buffers/buses. A plugin host, in turn, lets each plugin allocate its own memory (often on the audio thread), so RT-safety depends on every third-party plugin.

Clausters allocates on the network (or NRT / compiler) thread, uses on the audio thread, and frees back on the network thread; the audio thread only moves pointers (guarded by `assert_no_alloc`). Consequently there is **no `-w` and no `-m`** to size: the inter-UGen wires are a per-synth `Vec<Block>` (cache-line aligned), memory scales with the synths actually running, is contiguous, never fragments a pool and never touches the audio thread. Fixed boot capacities remain (node slab, audio/control buses, etc. — see the capacity table in [`architecture.md`](architecture.md)), but those are throughput/headroom limits, not per-graph memory pools. The price is that every synth is *fully built before* it reaches the audio thread, on the network thread.

### Extensibility

The DAW industry's winning model is the binary plugin: a defined binary interface (CLAP's C ABI or VST3's COM-style interfaces), a large ecosystem, variable sandboxing — its biggest market advantage and its biggest source of fragility (ABI breakage, crashes from third-party code). scsynth copied that pattern (`UnitCmd`/interface tables) and paid the price: the ABI broke on struct/feature changes and out-of-tree UGens became a maintenance tax.

Clausters declines dynamically loaded Rust plugins by decision, not omission: Rust has no stable ABI. Extending the server means adding code to the crate (the `UGen` + `ProcessCtx` + registry contract). The runtime-extensible path for users is a **Faust def** (`/def_send faust`, JIT-compiled behind the same `SynthNode` interface) — most "I need a custom UGen" cases are a Faust one-liner. The policy for an external binary boundary is compatibility checked at load, with no Rust types crossing it. Hosting *DAW* plugins — CLAP and VST3 — follows that policy with process isolation on Linux: one helper per instance, a bounded shared-memory bridge and no waiting or syscalls in the server's callback. Instruments, effects, transport, events and state are available. The measured bridge delay plus the native processor's intrinsic latency feeds the same server compensation pass as a SynthDef convolver. Native editor windows run on the helper main thread through X11/XWayland, including when requested by a browser connected to the native server. See [Third-party plugins run out of process](decisions.md#third-party-plugins-run-out-of-process).

### State, persistence and remote control

A DAW's state *is* the document (the session): single-user, monolithic, a proprietary project format. The node-tree model is **client/server over OSC**: the server has no notion of a "song"; state lives in the defs and the live tree. Clausters adds def persistence, MIDI bindings and a boot preset, and a standalone mode playable from a controller with no client. The model decouples fully — multiple clients/languages against one engine, network control, embeddable as a library, offline render as a first-class citizen. The node tree is the audio engine, while the project's editable multitrack, sources, regions, take lanes and undo live in the shared document and its applications. Keeping those layers distinct lets several clients edit one document without turning the engine's voices into persistent DAW channels.

## Where each model wins

The node-tree model (and Clausters on it) wins in: voice granularity and dynamic polyphony; arbitrary routing orthogonal to order; deterministic, sub-block timing with bit-identical offline render; formal, verified RT-safety; the decoupled client/server, embeddable, multi-language, network-controllable design; and extensibility without ABI fragility (Faust JIT as a sandboxed plugin).

DAWs win in: ergonomics (order is visible in the routing — nobody produces silence from a misplaced add action; auto-sorted groups close much of this gap, but only for the statically analysable graph); broader commercial plugin/platform integration; the complete workstation product (multitrack, persistent mix, undo, comping — separate from the node-tree engine); and mature, transparent per-channel parallelism.

Clausters' specific contribution is to take scsynth's model and close its three classic weaknesses without leaving the paradigm: the manual-ordering burden (→ auto-sort from buses), the hand-sized RT memory pools and on-audio-thread build (→ off-thread build, per-synth memory), and the block quantization and RT/NRT divergence (→ sub-block split, bit-identity). General server compensation closes the intrinsic-latency gap on automatic bus paths. Dynamically loaded Rust UGen plugins remain rejected; isolated CLAP/VST3 hosting uses their defined binary interfaces without exposing Rust types.

## Dependency analysis versus plugin delay compensation (PDC)

Auto-sorted groups and a DAW's PDC look like the same feature but are not. They are the two **complementary** problems that appear once execution order is no longer wired into the structure:

> The auto-sort solves **ordering** (in what sequence do I run the nodes so a reader runs after its writer, within the same block?). The PDC solves **latency alignment** (given that nodes introduce latency, how much delay do I add to the short paths so the summing points stay in phase?). A DAW gets ordering for free and spends its engineering on PDC. SuperCollider does the inverse.

### Why ordering is trivial in a DAW and the whole problem in SC

In a DAW the topology *is* the routing, so deriving execution order from it is a trivial topological sort the host always does — there was never an "ordering mode" because order was never a user decision. scsynth does the opposite by design: order (the tree) is orthogonal to routing (global numbered buses). You gain expressiveness (any node to any bus, live re-routing) but lose the free derivation: the tree does not *know* that synth A writes bus 16 that synth B reads. Auto-sorted groups are, literally, recovering the DAW's free derivation on top of a model that had dropped it on purpose.

### The Clausters dependency analysis in detail

Everything below runs on the network thread against `TreeMirror` (a mirror of the node tree fed by the same commands the engine gets); re-sorts reach the audio thread as ordinary `Cmd::MoveNode`s. The engine retains the ordering mode to validate the compensation snapshot; the dependency computation stays off RT. The code is `src/osc/graph.rs`.

- **The atom: `BusUsage` as a bit vector.** Each node reduces to `reads` and `writes` (one bit per audio bus, sized to however many the server was configured with) plus a `dynamic` flag. An edge between two nodes is then just an overlap test between two bit vectors.
- **Extraction: what is read, what is written.** `ugen_usage` walks the def's UGens and looks only at three: `In` (reads), `Out` (writes), `ReplaceOut` (**reads and writes** — it consumes what is on the bus). Each one's bus index is classified by where it comes from: a **constant** is a static index; a **control** is static but *tracked* (recorded in `bus_controls`, because an `/node_set` on it changes the routing and forces re-analysis); a **signal-computed index** (a wire) is **not analysable** and sets `dynamic = true`. `faust_usage` is analogous — a Faust synth reads `in..in+inputs` and writes `out..out+outputs`, and its two reserved routing controls sit after the UI params, so they are always static indices.
- **The sort: stable Kahn with explicit degradation on cycles.** `stable_topo_sort` builds the adjacency `before[i][j] = writes_i & reads_j`, injects dynamic barriers as edges against everything by current position, and runs Kahn always picking the *earliest ready* unit. Two properties follow: **stability** (pure summing writers to the same bus have no edge between them — mixing commutes — so insertion order is preserved), and **cycles are not solved** (on a deadlock the earliest remaining unit is released; cycle members keep their current relative order = one block of feedback delay). It is O(n²) per group, where n is one group's child count, not the whole tree; nested groups sort independently, each subgroup collapsed to the union of its subtree's usage (`usage_of`).
- **Incremental re-analysis.** The sort re-runs on every change that can alter the DAG: a node added/removed, a subtree moved, an `/node_set` on a `bus_control`, or an `/node_map`/`/node_mapAudio` touching an audio map or a bus-index control (`set_map`, `fold_maps_into_usage`).

### What a PDC does, and why it is a different computation

A PDC starts from the fact that **each plugin reports its latency in samples** (a lookahead compressor: +N; a linear-phase EQ: +M). The routing graph already gives the order; the problem is that **parallel paths that sum accumulate different latencies** and arrive out of phase at the mix point. The host solves a **longest-path** problem: for each node it takes the maximum accumulated latency over its inputs and **inserts compensating delay lines** on the shorter inputs so all confluent signals align. It is O(V+E) over the *global* routing graph (alignment is an end-to-end property, not local to a group) and it produces a new physical artifact: delay buffers inserted into the graph.

| | Auto-sort (Clausters) | PDC (DAW) |
|---|---|---|
| Question | In what **order** do I run? | How much **delay** do I add? |
| Input | per-node bus reads/writes | per-node reported latency |
| Computation | topological sort, O(n²) per group | longest path, O(V+E) global |
| Output | a permutation of children (`MoveNode`) | delay lines inserted |
| Granularity | block (64 samples) | sample |
| "Unknowable" frontier | signal-computed index → dynamic barrier | plugin with no/wrong report → misalignment |
| Runtime cost | mirror on network thread, zero on audio | recompute on a non-audio thread, re-latch |

### The shared "unknowable frontier"

The deepest parallel is not the mechanics but how each treats what it cannot analyse — the same philosophy applied to the two halves of the problem. The auto-sort, facing a signal-computed bus index, **does not guess**: it marks the node `dynamic` and freezes it as a barrier. Conservative correctness — possibly suboptimal (it will not sort across the barrier) but never a silently wrong order; the user's remedy is to isolate the dynamic node in its own manual group. The PDC, facing a plugin that reports latency wrongly, variably, or not at all, **also cannot**: the result is a misalignment the host does not fix, and the user's remedy is analogous (pin the latency by hand or isolate the plugin). Both are static analyses over node-declared metadata (bus index / reported latency), and both degrade to an observable default at the "signal-dependent / undeclared" boundary.

### General server delay compensation

Clausters now solves latency alignment as shared engine infrastructure. The
contract is `SynthNode::latency()`, in samples: zero by default, intrinsic
processing latency for a convolver or spectral reconstruction, and bridge plus
intrinsic latency for an isolated plugin. The planner does not select a def
family. Faust nodes use the same bus machinery; a delay authored in Faust or
with a delay UGen is musical timing and does not report an intrinsic latency.

The server flattens the tree's admitted execution order and computes accumulated
arrival times globally across nested automatic groups. For each processor it
aligns all audio inputs, including main and sidechain buses, to the latest
input. Each output then arrives at that input time plus the node's declared
latency. At a bus with several writers, every shorter **writer contribution**
is delayed to the latest arrival **before summation**. Delaying the mixed bus
would retain the skew inside its sum. `Out` adds its delayed contribution;
`ReplaceOut` overwrites with its delayed contribution and later writers still
add in execution order. Its ordering-only read is not an audio input; an actual
`In` on the same bus remains a read/write boundary.

The plan and its delay storage are built on the network or offline command
thread. The engine installs a prepared plan at a block boundary and retires its
old storage through the garbage FIFO. Unchanged node/channel specifications
retain their histories; changed paths start with zeroed histories. A new node
identity cannot inherit the samples of a freed node. No allocation, freeing,
lock, syscall or third-party processing is introduced into the server callback.
The existing parallel scheduler still proves exclusive bus ownership; delay
histories belong to individual nodes and remain private to their DSP visitor.

A topology edit at a nonzero sample offset takes effect at that sample, but
compensation is suspended for the remainder of that block and the prepared
plan becomes active at the next boundary. Routing commands and their plan are
published together. The engine checks the planned group ancestry/ordering modes, node order, usage masks, object identities, routing controls/maps and
latency declarations against the live tree before using a plan: a mirror that
includes a future scheduled node cannot silently retime a different live tree.
Live and offline processing use the same activation rule and delay histories.

### Explicit analysis boundaries

Compensation follows automatic ordering. A manual group breaks its automatic
domain; nested automatic groups inside one uninterrupted automatic domain can
share paths, including across their boundaries. Manual, read-before-write and
feedback paths are uncompensated, and the connected audio paths and sums that
depend on their unknown arrival times remain uncompensated too. A signal-driven
bus index may touch any bus, so a dynamic node conservatively suspends the
global plan. Feedback's authored block delay is never treated as latency to
remove. A wrong declaration cannot be inferred from samples.

`/group_dumpGraph` includes the **global planned** intrinsic latencies, input
and writer delays in samples, and the reason for each excluded node. Like the
tree query, it describes the command mirror, including pending timed edits.
Aggregate history is bounded; a plan exceeding the storage capacity is reported
as uncompensated rather than allocating on the callback or truncating a delay.
The exact limit and retirement mechanism are in
[the architecture](architecture.md#server-delay-compensation).

The bus graph treats a synth as an opaque processor with one declared latency.
A serial SynthDef chain reports the sum of its intrinsic UGen latencies. To
align different intrinsic paths, put them in distinct synth nodes connected by
buses: this pass aligns those paths, rather than rewriting the internal UGen
wiring or undoing a musical delay. See [automatic ordering](auto-order.md) for
the commands and [the protocol reference](schemas.md) for the def families.
