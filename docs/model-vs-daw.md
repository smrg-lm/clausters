# The node-tree model and DAWs (design rationale)

Why Clausters keeps SuperCollider's node-tree model, what that model buys and costs against the channel-graph model of a digital audio workstation (DAW), and — in detail — how the auto-sorted-group dependency analysis relates to a DAW's plugin delay compensation (PDC), including why Clausters has no PDC today and how one would fit. This is background and rationale; the user-facing behavior of auto-sorted groups is in [`auto-order.md`](auto-order.md), and the implementation invariants are in [`architecture.md`](architecture.md).

## Three models in one sentence

- **DAW (channel graph).** A fixed, hierarchical graph of *channels*. Each track is a linear chain of insert plugins feeding buses/groups and finally the master; sends branch off to return buses. The user draws the topology in a UI and the engine derives the execution order from the routing. The unit is the *channel*, not the *voice*.
- **scsynth (node tree).** An ordered *tree* of nodes (groups and synths). Execution order **is** the depth-first traversal of the tree, and nothing else. Signal routing is separate, through globally numbered *buses*. Making "who runs first" agree with "who writes the bus another reads" is the client's manual job (add actions, `/node_before`, `/node_after`).
- **Clausters.** The same tree and buses as scsynth for the baseline semantics, plus two departures: it can **infer execution order from the bus routing** (auto-sorted groups, `/group_sortMode`) — recovering the DAW's automatic ordering without giving up the tree — and it **builds every synth off the audio thread**, which removes several historical limits of scsynth (RT/wire memory pools, block-boundary scheduling).

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

The DAW industry's winning model is the binary plugin (VST/AU/CLAP-style): a stable C ABI, a large ecosystem, variable sandboxing — its biggest market advantage and its biggest source of fragility (ABI breakage, crashes from third-party code). scsynth copied that pattern (`UnitCmd`/interface tables) and paid the price: the ABI broke on struct/feature changes and out-of-tree UGens became a maintenance tax.

Clausters declines it by decision, not omission: there is no stable Rust ABI, so there are no dynamically loaded Rust plugins in v1. Extending the server means adding code to the crate (the `UGen` + `ProcessCtx` + registry contract). The runtime-extensible path for users is a **Faust def** (`/def_send faust`, JIT-compiled, sandboxed by the same `SynthNode` boundary) — most "I need a custom UGen" cases are a Faust one-liner. If dynamic plugins ever become necessary, the policy is fixed: a versioned C ABI or a wasm interface, checked at load. This avoids the plugin-ABI fragility of both scsynth and DAWs, at the cost of a third-party UGen marketplace.

### State, persistence and remote control

A DAW's state *is* the document (the session): single-user, monolithic, a proprietary project format. The node-tree model is **client/server over OSC**: the server has no notion of a "song"; state lives in the defs and the live tree. Clausters adds def persistence, MIDI bindings and a boot preset, and a standalone mode playable from a controller with no client. The model decouples fully — multiple clients/languages against one engine, network control, embeddable as a library, offline render as a first-class citizen. The flip side is that there is no timeline/arrangement/undo — the very product of a DAW. The scope is genuinely different: the node tree is an engine, not a workstation.

## Where each model wins

The node-tree model (and Clausters on it) wins in: voice granularity and dynamic polyphony; arbitrary routing orthogonal to order; deterministic, sub-block timing with bit-identical offline render; formal, verified RT-safety; the decoupled client/server, embeddable, multi-language, network-controllable design; and extensibility without ABI fragility (Faust JIT as a sandboxed plugin).

DAWs win in: ergonomics (order is visible in the routing — nobody produces silence from a misplaced add action; auto-sorted groups close much of this gap, but only for the statically analysable graph); the binary-plugin ecosystem; the complete workstation product (timeline, arrangement, persistent mix, undo, comping — all outside the node-tree model's scope); and mature, transparent per-channel parallelism.

Clausters' specific contribution is to take scsynth's model and close its three classic weaknesses without leaving the paradigm: the manual-ordering burden (→ auto-sort from buses), the hand-sized RT memory pools and on-audio-thread build (→ off-thread build, per-synth memory), and the block quantization and RT/NRT divergence (→ sub-block split, bit-identity). And it deliberately rejects the one pattern where scsynth imitated DAWs and it went wrong: the binary plugin ABI.

## Dependency analysis versus plugin delay compensation (PDC)

Auto-sorted groups and a DAW's PDC look like the same feature but are not. They are the two **complementary** problems that appear once execution order is no longer wired into the structure:

> The auto-sort solves **ordering** (in what sequence do I run the nodes so a reader runs after its writer, within the same block?). The PDC solves **latency alignment** (given that nodes introduce latency, how much delay do I add to the short paths so the summing points stay in phase?). A DAW gets ordering for free and spends its engineering on PDC. SuperCollider does the inverse.

### Why ordering is trivial in a DAW and the whole problem in SC

In a DAW the topology *is* the routing, so deriving execution order from it is a trivial topological sort the host always does — there was never an "ordering mode" because order was never a user decision. scsynth does the opposite by design: order (the tree) is orthogonal to routing (global numbered buses). You gain expressiveness (any node to any bus, live re-routing) but lose the free derivation: the tree does not *know* that synth A writes bus 16 that synth B reads. Auto-sorted groups are, literally, recovering the DAW's free derivation on top of a model that had dropped it on purpose.

### The Clausters dependency analysis in detail

Everything below runs on the network thread against `TreeMirror` (a mirror of the node tree fed by the same commands the engine gets); re-sorts reach the audio thread as ordinary `Cmd::MoveNode`s, so the engine is untouched. The code is `src/osc/graph.rs`.

- **The atom: `BusUsage` as a bitmask.** Each node reduces to `reads` and `writes` (masks over the audio buses, `1 << bus`) plus a `dynamic` flag. An edge between two nodes is then just `writes_a & reads_b != 0` — a bit `AND`.
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

### The real gap: Clausters has no PDC

Clausters recovers the "easy" half (ordering, which SC had made hard) but does **not** solve the half a DAW considers hard — latency alignment — and the reason is structural and, today, benign. A UGen in Clausters processes sample-for-sample within the block; it does not report latency because, in the normal case, it has none (there is no `latency()` on the `UGen`/`SynthNode` trait). So there is nothing to align in the typical static chain: `source → fx → master` comes out with zero inter-path skew.

Latency *does* exist in two places: (a) the one-block delay each cycle/feedback introduces (`LocalIn`/`LocalOut`, or a cycle the sort preserves), and (b) any dynamic barrier forcing an extra bus hop. If path A is `source → fx → master` (0 extra) and path B passes through a feedback node (+64 samples) before summing into the master, the two are **64 samples out of phase and the auto-sort does not compensate it** — a PDC would. The current model hides the PDC problem because its unit of latency is zero-or-one-block and parallel paths rarely differ in feedback-hop count. But the problem is not solved, it is *absent for now*: the moment a UGen with real intrinsic latency appears (an FFT convolver with lookahead, a pitch-shifter, a linear-phase filter written in Faust), the alignment between parallel paths becomes an audible skew the current infrastructure does not see.

### How a PDC would fit (future extension)

The base is already in place and the design admits it cleanly:

1. Add `latency(&self) -> usize` (in samples) to the trait, default 0. Most UGens stay at 0; a `FaustSynth` with a delay/FFT reports its own.
2. The DAG already exists: `stable_topo_sort` gives the order; one only has to **annotate each `writes_i & reads_j` edge with accumulated latency** and run longest-path over the same mirror, on the network thread. The block-rate feedback delay is a fixed contribution to the count.
3. Compensation is inserted as what the engine already inserts: a delay UGen of N samples on the short paths, or an offset in `ProcessCtx` (which already carries `offset`/`frames` for the sub-block split).
4. Determinism: any PDC-added latency must be identical between real time and offline (an engine invariant). Since the computation is on the mirror and never touches the audio thread, it fits the same pattern as the auto-sort — but it would need pinning in the test suite. The alignment being global (across nested-group boundaries that today collapse in `usage_of`) is the non-trivial part.

In short, the auto-sort is the topological sort of a PDC without the latency annotation. Clausters has already built the ordering half a DAW gets for free (static bus analysis, conservative degradation on the unanalysable, zero audio-thread cost) and left the PDC half for when an intrinsic latency that does not exist today shows up. The mirror infrastructure leaves the door open to close the gap without touching the engine.
