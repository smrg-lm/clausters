# The client, layer by layer

The package is three layers plus a thin convenience wrapper. The split is deliberate: the value/timing work knows nothing about the server, and the server side is one swappable seam.

## `clausters.base` — server-agnostic timing and values

- `builtins` — scalar and sequence math, computed as `f32` through the native core (named at the top level too: `clausters.builtins`, which shadows nothing), so a value the client computes equals the one the server's UGens would compute. Three families: the operator/conversion set every SuperCollider user knows (`midicps`, `dbamp`, `clip2`, …); the **range maps** (`linlin`, `linexp`, `explin`, `expexp`, `lincurve`, `curvelin`, `range`, `exprange`), which read a value out of one range and write it into another; and the two **perceptual frequency scales** a spectrogram axis is labeled in (`cpsmel`/`melcps`, `cpsbark`/`barkcps`), named the same `<from><to>` way and the one family computed in `f64` — no UGen computes a mel, so there is no f32 result to match. All of them take a sequence as readily as a number — `midicps(range(0, 120))` maps a hundred and twenty notes in one crossing — and a sequence is anything iterable, not one blessed type.
- `absobject` — operator overloading, the base for composing values and signals.
- `stream` — `Routine`/`Stream`, the `yield` coroutine layer. A routine must never block the clock thread.
- `clock` — `TempoClock`, **timing only**: it schedules and paces, it does not talk to the server.
- `timebase` — monotonic, or anchored to the server's sample clock (`/sched_at`) for drift-free timing.
- `moment` — `Moment`, when something is happening: a clock and an exact beat on it. The one answer to "what time is it *for this event*", which is what a destination stamps onto the wire.
- `destination` — where OSC goes: the `Server` for our own, `OscDestination` for any other application.
- `netaddr`, `main` — addressing and a thread-local execution context. No global state that would block running RT and NRT in one script.

See [Routines and clocks](routines-and-clocks.md) for driving these directly — writing a routine by hand — and [Timing models](timing-models.md) for the clock's timing modes (wall-clock, sample-locked, shared transport) and how to observe each.

## `clausters.seq` — sequencing

- `Event` — a note plays a synth and frees it after its sustain.
- The value patterns (`Pseq`, `Pwhite`, `Pseries`, ...) and `Pbind`. The random
  patterns (`Pwhite`, `Prand`) draw from the **random context** — the running
  routine's generator, derived at creation from the context that created it,
  seeded per session (`session.seed(n)`, or `clausters.seed(n)` for the
  default session; sclang's model, no per-pattern seeds). One seed reproduces that
  session end to end, sessions reproduce independently, and the generator lives
  in the shared native core, so the same seed replays the same music in every
  Clausters client language. The context is also exposed directly as
  `clausters.uniform(lo, hi)` / `choice(items)`, with `current_rng()` and
  `spawn_rng()` for the stream itself (a `clausters.Rng`) and the raw draws
  (`next_f64()`, `next_below(n)`) under `clausters.base.rand`.
- `EventStreamPlayer` — `Pbind(...).play(clock, server)` runs live or builds an NRT score depending on which interface the `Server` holds, with yield-exact timing (monotonic pacing, wall-clock timetags).

## `clausters.defs` — the server side

- `ugens` — lowercase callables producing `Ugen`/`Control`, and `SynthDef` (sent with `/def_send synth`): the server's UGens wired into a graph.
- `FaustDef` (sent with `/def_send faust`), its peer: DSP the server JIT-compiles, built from `signals` (Faust's Signal API as lowercase callables), from `boxes` (its Box API — point-free, with `boxes.faust` opening the Faust libraries), or from Faust source. The three forms are equals; so are the two def families.
- Both families are built **instance-based, with no global build context**, and both instantiate as ordinary nodes in the same tree.
- `Node`/`Bus`/`Buffer` handles and their allocators. A `Group` is **born named** —
  `Group("mixer")`, and `group.rename(...)` afterwards — a referenceable label
  on top of the node id: the id stays the identity every command uses, and the
  name is how you *refer* to the group instead of to a number, comes back in
  every node record, and makes the tree navigable by path
  (`Server.group_at("/mixer/drums")`). That is what lets a mixer's channels, its
  sends and its master be built out of groups and still be sayable.
- `clocksync` — models the server's sample clock over UDP (`Server.sample_clock()`) for drift-free `/sched_at` timing without shared memory.
- **Introspection** — `Server.query_tree()` and `node.info()` read what is *playing* (the server is asked about every node it holds, a node about itself; every entry of the tree is the same record, and `print(tree)` draws it); `Server.query_defs()`, `query_buffers()` and `query_ugens()` read what the server **holds**: the loaded defs with their control surface, the allocated buffers, and the UGen catalog with named inputs and defaults. Worth asking rather than assuming — the def store persists across restarts, so a server can hold defs this client never sent. All blocking, so never from a routine.
- `Server` — **owns the communication interface and emits through it.** Swapping its interface retargets a routine from a live RT server to an NRT score without touching the clock or the routine. Interfaces include `OscUdpInterface`, `OscTcpInterface` (length-prefixed OSC; start the server with `--tcp`), and `OscWsInterface` (OSC over WebSocket, the browser-reachable transport; start the server with `--ws`), all drop-in.

The names that make up this layer's core — `Server`, `ServerOptions`, `Synth`, `Group`, `AddAction`, `SynthDef`, `FaustDef`, `GraphDef`, `Bus`, `Buffer` — are re-exported at the top level, so `from clausters import Server, Group, SynthDef, Bus` reads like the sequencing and timing imports beside it. The UGen and signal callables are not: they are a vocabulary of hundreds of names, and they stay under `clausters.defs`.

That is the whole rule for the top level, and it holds for every layer: **what you name while writing a piece is flat, what is enumerative is named through its module.** So `seq.Pbind` and `gui.knob`, like `defs.sine` — and, for the same reason from the other side, the transports and process launchers (`ipc.Clausters`, `ipc.ShmClient`, `launch.ServerProcess`, `launch.GuiProcess`) stay under theirs: you reach those as a property or an argument of the layer above, never by instantiating them. The layer modules themselves *are* re-exported, so `from clausters import *` gives you `defs`, `seq`, `gui`, `form`, `base`, `ipc`, `launch` and `errors` to reach through.

See [Defining instruments: FaustDef and SynthDef](defs.md) for the full def-building vocabulary — every `signals` / `ugens` callable, how the two def kinds differ, and how a def is sent behind the `/server_sync` barrier.

## `clausters.Session` — ergonomic defaults, no globals

`Session` bundles a `Server` and a clock, with `Session.nrt()` / `Session.live()` factories and `.play(pattern)` / `.render()` / `.run(s)`. Several sessions coexist — an offline NRT one for plots next to a live RT one — in the same script. See [Sessions](sessions.md) for the full picture.

## The ambient verbs — `play`, `plot`, `render`

On top of the sessions sit three free-standing verbs that resolve the ambient context (the running session, else the default one), each a single entry for many kinds: `play` sounds what already sounds directly (an event or dict, a pattern, a routine or generator, a def or bare expression, a timeline, a buffer, an automation), `render` performs the change of state to audio (defs and expressions, arrangement elements, timelines, patterns — offline to samples or a WAV, or delegated to a live destination), and `plot` (with its live sibling `scope`) is the visual counterpart. See [The ambient verbs](verbs.md) for the full dispatch tables and the play/render split.

## The seam, restated

Everything above the `Server` (clock, routines, patterns, defs) is written once. The `Server`'s interface is the only thing that decides whether the OSC reaches a live device, a shared-memory transport, or an in-process renderer producing a WAV. That is what lets a single script do a live take and an offline render side by side.

## Where the server contract lives

The wire formats this client emits — the OSC command set, the SynthDef JSON and Faust def formats, the node-tree and bus/buffer semantics, and the C ABI behind the native core — are specified in the **[Clausters server book](https://clausters.readthedocs.io/)**, not here. When in doubt about what a byte means on the wire, that is the source of truth; this client is one consumer of it.

Every command in that set is spelled by one rule — **`/<resource>_<action>`**,
the resource in full (`node`, `synth`, `group`, `bus`, `buffer`, `def`, `ugen`,
`server`, …), the action in camelCase, a reply as `<command>.reply`, a range as
`Range` — so the addresses this client builds can be read without a lookup. The
rule and its three corollaries open the server's OSC reference.

