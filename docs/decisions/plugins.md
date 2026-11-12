# Third-party plugins

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## Third-party plugins run out of process

*Decided with the user, 2026-10-13.* The restriction on **UGen plugins written in Rust** remains: Rust has no stable ABI and scsynth's binary boundary showed the maintenance cost. Hosting third-party DAW plugins is a separate capability. CLAP defines a C ABI; VST3 defines COM-style binary interfaces. Compatibility is checked at load and no Rust types cross the boundary. CLAP is MIT-licensed, and the VST3 SDK has been MIT since 3.8; that SDK license is not a claim about third-party plugin licenses.

**Plugin code runs outside the server, in one helper process per instance.** The helper's main thread owns lifecycle and native windows; its processing thread runs the plugin. A def is a recipe, not a shared live process. The per-instance boundary isolates crashes and preserves one producer per shared-memory queue when the server processes nodes on parallel workers. A polyphonic instrument remains one persistent plugin instance receiving events; notes do not each create another helper.

The server's callback only accesses preallocated buffers and atomics. It never allocates, frees, locks, waits or makes syscalls, including a futex wake. `crates/clausters-server/tests/rt_safety.rs` guards allocation/freeing, not the whole real-time contract: syscall/lock inspection and deadline measurements are additional checks. The wake mechanism and buffering depth remain **feasibility questions to measure** under a declared workload before implementing the bridge; no one-block or four-block latency is promised. Polling and non-RT notification may be compared without weakening the callback rule.

The bridge carries indexed jobs with generations, sample-offset events and transport snapshots. Engine slices are staged into complete jobs rather than published as separate blocks. An output that misses its scheduled interval becomes silence and is counted; late output never moves to a later interval. Bounded backlog/overflow rules must preserve explicit event ownership or fail the instance visibly, including note releases. Restart names the running node, prepares a fresh helper outside RT, restores saved state/current parameters and swaps generations at a block boundary; it clears outstanding notes and does not recover unsaved DSP history. Sending a def again affects future instances, not the live nodes holding the failed bridge.

The trade-off is measured buffering latency, reported along with intrinsic plugin latency through `SynthNode::latency()`, and per-instance process/CPU overhead. **Server delay compensation is part of the agreed delivery**, including statically analyzable paths across nested automatic groups, mixed writers and sidechains; manual/dynamic/feedback boundaries remain explicit. Native editors open on the server's machine. A web client controlling that native server has the same surface as Python; only the in-page wasm engine cannot load a native binary. A WAM-based implementation is an open question, not an adopted replacement.

Offline rendering uses the same protocol and declared bridge delay, but may wait outside RT. Equality with RT is tested only for a deterministic fixture with identical rate, state, events and transport, no GUI edits and no missed deadlines. Hosting is still planned work; its milestones and the feasibility acceptance live in the server plan.

*Bridge choice measured 2026-10-13.* A local isolated CLAP fixture experiment compared 50 us helper polling with a non-RT polling
notifier/futex design, at depths 1/2/4/8/16, up to eight instances and four
producer workers. Select **50 us helper polling and a configurable four-block
default** for the initial production bridge. Both designs missed some
one/two-block deadlines; depth four had no bridge misses without full CPU
saturation. Notification saved some CPU at multiple instances but added another
scheduler-dependent thread and wake protocol, and cost more at one instance.
The first reference favors the simpler mechanism. This is not a universal
minimum or a guarantee for a device callback with a different job cadence.

*Corrected 2026-10-14.* The measurement drove one block per callback, and so
did every test. A real device callback is many blocks run back to back
(PipeWire's default quantum was 2048 frames, 32 blocks), and a result due
inside one callback cannot come from a job published microseconds earlier in
it. Depth four starved on an idle machine. The bridge depth is therefore at
least one device callback, read from the longest callback the engine has run;
the measured four stays the floor for small callbacks. The bridge latency is
then the device period, which the device already imposes on everything else.

The consequence is 256 bridge samples at 48 kHz/64 frames, plus intrinsic
latency, with a fixed ownership protocol: late output is discarded, accepted
input and releases are still consumed in order, and overflow or eight consecutive
misses latch a failed/silent instance. Full CPU saturation produced controller
lateness and failures even at larger depths, so it is outside the validated
envelope; no RT syscall exception or invisible event loss is adopted to make
that case pass. The capacities, memory ordering, generations, restart boundary
and measurement summary are in
[the developer architecture](../architecture.md#isolated-plugin-processing-contract).

*Native host boundary, 2026-10-13.* The plugin audio segment has its own
`PLUGIN_ABI_VERSION`, and the dedicated administrative protocol its own version.
The helper checks both plus the negotiated layout capacities before accepting a
load or scan. Reusing the embed IPC or core C ABI counter would couple a helper's
private segment to unrelated client binaries, even though neither existing
boundary changed. New instances use fresh generations/mappings, and a helper
mismatch is rejected and reaped. The opt-in `plugin`/`plugin-clap` Cargo features
start with infrastructure and discovery; the plugin def family still belongs to
the engine integration that follows.

## Loading a plugin state uses a fresh processing generation; saving does not (2026-10-14)

A state load can change parameter metadata and intrinsic latency, and a plugin
may require reactivation. Pausing just its helper while the server continues
publishing fills the bounded bridge; stopping the server producer and later
resuming its old sequence shifts queued audio past its due time. Neither is a
valid live timeline.

A load therefore quiesces the node at a block boundary and installs a fresh
helper generation, loading the supplied image before activation. Preparation
is silent, held notes and pending events are cleared, and routing/maps
survive. A load takes the loaded parameter values. Compatible metadata is
refreshed per node. A port/control identity change is rejected rather than
changing the running graph's ownership. The def remains a recipe for other
instances, and a node keeps its last successful image for recovery without
changing that recipe on disk. An accepted failed load stays silent until
explicit restart.

*Revised the same day.* A **capture** first followed the same path: quiesce,
save, then swap in a fresh helper restored from the image. That made every
save an audible gap that dropped held notes and reverb tails, and it started
three helpers. Neither format needs it: CLAP's `state.save` and VST3's
`getState` are main-thread calls a host may make while the processor runs, and
a save changes no surface. A capture now asks the running helper, and the image
becomes the node's recovery point by replacing only its private recipe. Only a
load is disruptive.

## VST3 parameters keep the processor's native domain (2026-10-14)

VST3 processes normalized values, while `IEditController` converts presentation
units only on the UI thread. Calling that conversion for sample-offset
parameter changes or audio-bus mappings would break the thread contract. A
sampled conversion table would make arbitrary nonlinear parameters approximate.
The adapter therefore reports `[0, 1]` values, defaults and normalized steps;
CLAP keeps its native plain units. Both clients use the same `param:<id>`
controls and the reported range. Per-note volume/tuning use the SDK's defined
conversion and remain in the shared expression vocabulary. See the
[VST3 controller contract](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IEditController.html).

The Linux module is opened with `RTLD_NODELETE`. An allocation-guarded local
fixture passed processing and teardown, then crashed in a Rust TLS destructor
after `dlclose` had unmapped its code. Releasing COM references and balancing
ModuleExit does not unregister a library's thread-local destructors. Keeping the
code mapped until process exit is bounded by the disposable or per-node helper
lifetime and preserves those callbacks without retaining live plugin instances.

## A plugin's processing thread is real-time only where the plugin's threads can be (2026-10-14)

The server's callback is real-time through RTKit, which promotes the threads
that ask it and nothing else. The helper's processing thread can ask the same
way, but many instruments process by handing work to threads of their own and
waiting for them, and those threads promote themselves with
`pthread_setschedparam`, which needs a nonzero `RLIMIT_RTPRIO`. On a desktop
with RTKit and no such limit -- the default -- that request fails, and a
real-time thread then waits on ordinary threads it can keep off the CPU. An
instrument doing exactly that ran its processing thread past RTKit's watchdog
on every load: a gap until the SIGXCPU guard demoted it.

So the helper promotes its processing thread only when `RLIMIT_RTPRIO` is
nonzero, which is how a system set up for real-time audio runs and where a
plugin's threads can follow. Elsewhere it stays as an ordinary thread, which
is what every plugin was written to survive. A single-threaded plugin loses
priority it could have had on such a desktop; the alternative is a gap for
every plugin that is not.

## Delay compensation belongs to the server bus graph

The intrinsic-latency hook introduced with partitioned convolution now feeds
one engine planner, independently of the def family. Native plugin bridge
latency is one declaration among others. The planner flattens the automatic
execution domains across nested groups; calculating a separate plan inside
each group would lose the accumulated arrival time at its boundaries.

The engine delays **writer contributions before summation**, using a private
bus view during each node visit, rather than delaying an already mixed bus.
The latter would preserve the phase error inside a dry/processed sum. It also
aligns main and sidechain reads before an opaque processor runs. An overwrite
keeps its mode in the output history, so ReplaceOut and later additive writers
retain their execution semantics.

Plans and histories are allocated on the producer thread and retired through
the existing garbage FIFO. Stable specifications retain histories; a new node
identity cannot retain another instance's samples. Split-block topology edits
suspend compensation until a block boundary. Checking the planned execution
order, object identities, routing controls/maps, usage and declarations against the live tree prevents an ahead-of-time
command mirror from compensating a different graph. Manual, dynamic and
feedback frontiers remain explicit rather than receiving guessed timing;
authored musical delays remain outside the intrinsic-latency contract.
