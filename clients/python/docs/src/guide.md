# The client, layer by layer

The package is three layers plus a thin convenience wrapper. The split is deliberate: the value/timing work knows nothing about the server, and the server side is one swappable seam.

## `clausters.base` — server-agnostic timing and values

- `builtins` — scalar and list math, computed as `f32` through the native core, so a value the client computes equals the one the server's UGens would compute.
- `absobject` — operator overloading, the base for composing values and signals.
- `stream` — `Routine`/`Stream`, the `yield` coroutine layer. A routine must never block the clock thread.
- `clock` — `TempoClock`, **timing only**: it schedules and paces, it does not talk to the server.
- `timebase` — monotonic, or anchored to the server's sample clock (`/sched`) for drift-free timing.
- `moment` — `Moment`, when something is happening: a clock and an exact beat on it. The one answer to "what time is it *for this event*", which is what a destination stamps onto the wire.
- `destination` — where OSC goes: the `Server` for our own, `OscDestination` for any other application.
- `netaddr`, `main` — addressing and a thread-local execution context. No global state that would block running RT and NRT in one script.

See [Routines and clocks](routines-and-clocks.md) for driving these directly — writing a routine by hand — and [Timing models](timing-models.md) for the clock's timing modes (wall-clock, sample-locked, shared transport) and how to observe each.

## `clausters.seq` — sequencing

- `Event` — a note plays a synth and frees it after its sustain.
- The value patterns (`Pseq`, `Pwhite`, `Pseries`, ...) and `Pbind`. The random
  patterns (`Pwhite`, `Prand`) draw from the **random context** — the running
  routine's generator, derived at creation from the context that created it,
  seeded per session (`session.seed(n)`, or `main.seed(n)` for the default
  session; sclang's model, no per-pattern seeds). One seed reproduces that
  session end to end, sessions reproduce independently, and the generator lives
  in the shared native core, so the same seed replays the same music in every
  Clausters client language. The context is also exposed directly as
  `clausters.next_f64()` / `uniform(lo, hi)` / `next_below(n)` /
  `choice(items)`.
- `EventStreamPlayer` — `Pbind(...).play(clock, server)` runs live or builds an NRT score depending on which interface the `Server` holds, with yield-exact timing (monotonic pacing, wall-clock timetags).

## `clausters.defs` — the server side

- `ugens` — lowercase callables producing `Ugen`/`Control`, and `SynthDef` (sent with `/d_recv`): the server's UGens wired into a graph.
- `FaustDef` (sent with `/d_faust`), its peer: DSP the server JIT-compiles, built from `signals` (Faust's Signal API as lowercase callables), from `boxes` (its Box API — point-free, with `boxes.faust` opening the Faust libraries), or from Faust source. The three forms are equals; so are the two def families.
- Both families are built **instance-based, with no global build context**, and both instantiate as ordinary nodes in the same tree.
- `Node`/`Bus`/`Buffer` handles and their allocators.
- `clocksync` — models the server's sample clock over UDP (`Server.sample_clock()`) for drift-free `/sched` timing without shared memory.
- **Introspection** — `Server.query_tree()` / `node_query()` read what is *playing*; `Server.query_defs()`, `query_buffers()` and `query_ugens()` read what the server **holds**: the loaded defs with their control surface, the allocated buffers, and the UGen catalog with named inputs and defaults. Worth asking rather than assuming — the def store persists across restarts, so a server can hold defs this client never sent. All blocking, so never from a routine.
- `Server` — **owns the communication interface and emits through it.** Swapping its interface retargets a routine from a live RT server to an NRT score without touching the clock or the routine. Interfaces include `OscUdpInterface`, `OscTcpInterface` (length-prefixed OSC; start the server with `--tcp`), and `OscWsInterface` (OSC over WebSocket, the browser-reachable transport; start the server with `--ws`), all drop-in.

See [Defining instruments: FaustDef and SynthDef](defs.md) for the full def-building vocabulary — every `signals` / `ugens` callable, how the two def kinds differ, and how a def is sent behind the `/sync` barrier.

## `clausters.Session` — ergonomic defaults, no globals

`Session` bundles a `Server` and a clock, with `Session.nrt()` / `Session.live()` factories and `.play(pattern)` / `.render()` / `.run(s)`. Several sessions coexist — an offline NRT one for plots next to a live RT one — in the same script. See [Sessions](sessions.md) for the full picture.

## The ambient verbs — `play`, `plot`, `render`

On top of the sessions sit three free-standing verbs that resolve the ambient context (the running session, else the default one), each a single entry for many kinds: `play` sounds what already sounds directly (an event or dict, a pattern, a routine or generator, a def or bare expression, a timeline, a buffer, an automation), `render` performs the change of state to audio (defs and expressions, arrangement elements, timelines, patterns — offline to samples or a WAV, or delegated to a live destination), and `plot` (with its live sibling `scope`) is the visual counterpart. See [The ambient verbs](verbs.md) for the full dispatch tables and the play/render split.

## The seam, restated

Everything above the `Server` (clock, routines, patterns, defs) is written once. The `Server`'s interface is the only thing that decides whether the OSC reaches a live device, a shared-memory transport, or an in-process renderer producing a WAV. That is what lets a single script do a live take and an offline render side by side.

## Where the server contract lives

The wire formats this client emits — the OSC command set, the SynthDef JSON and Faust def formats, the node-tree and bus/buffer semantics, and the C ABI behind the native core — are specified in the **[Clausters server book](https://clausters.readthedocs.io/)**, not here. When in doubt about what a byte means on the wire, that is the source of truth; this client is one consumer of it.
