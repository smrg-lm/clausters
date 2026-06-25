# The client, layer by layer

The package is three layers plus a thin convenience wrapper. The split is deliberate: the value/timing work knows nothing about the server, and the server side is one swappable seam.

## `clausters.base` — server-agnostic timing and values

- `builtins` — scalar and list math, computed as `f32` through the native core, so a value the client computes equals the one the server's UGens would compute.
- `absobject` — operator overloading, the base for composing values and signals.
- `stream` — `Routine`/`Stream`, the `yield` coroutine layer. A routine must never block the clock thread.
- `clock` — `TempoClock`, **timing only**: it schedules and paces, it does not talk to the server.
- `timebase` — monotonic, or anchored to the server's sample clock (`/sched`) for drift-free timing.
- `netaddr`, `main` — addressing and a thread-local execution context. No global state that would block running RT and NRT in one script.

See [Routines and clocks](routines-and-clocks.md) for driving these directly — writing a routine by hand — and [Timing models](timing-models.md) for the clock's timing modes (wall-clock, sample-locked, shared transport) and how to observe each.

## `clausters.seq` — sequencing

- `Event` — a note plays a synth and frees it after its sustain.
- The value patterns (`Pseq`, `Pwhite`, `Pseries`, ...) and `Pbind`.
- `EventStreamPlayer` — `Pbind(...).play(clock, server)` runs live or builds an NRT score depending on which interface the `Server` holds, with yield-exact timing (monotonic pacing, wall-clock timetags).

## `clausters.defs` — the server side

- `signals` — lowercase callables mapping Faust's Signal API, composed into the JSON graph — and `FaustDef` (sent with `/d_faust`).
- `ugens` — the UGen-graph counterpart, lowercase callables producing `Ugen`/`Control`, and `SynthDef` (sent with `/d_recv`). Both are built **instance-based, with no global build context**.
- `Node`/`Bus`/`Buffer` handles and their allocators.
- `clocksync` — models the server's sample clock over UDP (`Server.sample_clock()`) for drift-free `/sched` timing without shared memory.
- `Server` — **owns the communication interface and emits.** Swapping its interface retargets a routine from a live RT server to an NRT score without touching the clock or the routine. Interfaces include `OscUdpInterface`, `OscTcpInterface` (length-prefixed OSC; start the server with `--tcp`), and `OscWsInterface` (OSC over WebSocket, the browser-reachable transport; start the server with `--ws`), all drop-in.

See [Defining instruments: FaustDef and SynthDef](defs.md) for the full def-building vocabulary — every `signals` / `ugens` callable, how the two def kinds differ, and how a def is sent behind the `/sync` barrier.

## `clausters.Session` — ergonomic defaults, no globals

`Session` bundles a `Server` and a clock, with `Session.nrt()` / `Session.live()` factories and `.play(pattern)` / `.render()` / `.run(s)`. Several sessions coexist — an offline NRT one for plots next to a live RT one — in the same script. See [Sessions](sessions.md) for the full picture.

## The seam, restated

Everything above the `Server` (clock, routines, patterns, defs) is written once. The `Server`'s interface is the only thing that decides whether the OSC reaches a live device, a shared-memory transport, or an in-process renderer producing a WAV. That is what lets a single script do a live take and an offline render side by side.

## Where the server contract lives

The wire formats this client emits — the OSC command set, the SynthDef JSON and Faust def formats, the node-tree and bus/buffer semantics, and the C ABI behind the native core — are specified in the **[Clausters server book](https://clausters.readthedocs.io/)**, not here. When in doubt about what a byte means on the wire, that is the source of truth; this client is one consumer of it.
