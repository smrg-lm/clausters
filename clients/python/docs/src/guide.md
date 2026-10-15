# The client, layer by layer

The package is three layers plus a thin convenience wrapper. The split is deliberate: the value/timing work knows nothing about the server, and the server side is one swappable seam.

## `clausters.base` — server-agnostic timing and values

- `builtins` — scalar and sequence math, computed as `f32` through the native core (named at the top level too: `clausters.builtins`, which shadows nothing), so a value the client computes equals the one the server's UGens would compute. Three families: the operator/conversion set every SuperCollider user knows (`midicps`, `dbamp`, `clip2`, `scaleneg`, the windows read at a phase such as `hanwindow`, …) with `clip`, `wrap` and `fold` between two bounds; the **range maps** (`linlin`, `linexp`, `explin`, `expexp`, `lincurve`, `curvelin`, `range`, `exprange`), which read a value out of one range and write it into another; the two **perceptual frequency scales** a spectrogram axis is labeled in (`cpsmel`/`melcps`, `cpsbark`/`barkcps`), named the same `<from><to>` way; and sclang's **functions on a number that are not operators** — `degrad`, `snap`, `softround`, `moddif`, `bilin`/`biexp`, `lcurve`, `gausscurve`, `zapgremlins`, `nextpoweroftwo`, the special functions `erf`, `tgamma`, `besselj` …, over a scale `degkey`/`keydeg`, `nearestinscale`, `nearestinlist`, and over a whole list `blend` (point by point), `blendat`, `normalize`, `normalizesum`, `integrate`, `differentiate`, `resamp0`/`resamp1` and `asrandomtable`, whose table `clausters.tablerand` draws from. Those last two families are computed in `f64`, as sclang computes them: no UGen computes them, so there is no f32 result to match. All of them take a sequence as readily as a number — `midicps(range(0, 120))` maps a hundred and twenty notes in one crossing — and a sequence is anything iterable, not one blessed type.
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
- The value patterns (`pseq`, `pwhite`, `pseries`, ...) and `pbind`, an
  `EventPattern` — a pattern is the definition of a generator, and only one
  whose values are events plays. The random
  patterns (`pwhite`, `prand`) draw from the **random context** — the running
  routine's generator, derived at creation from the context that created it,
  seeded per session (`session.seed(n)`, or `clausters.seed(n)` for the
  default session; sclang's model, no per-pattern seeds). One seed reproduces that
  session end to end, sessions reproduce independently, and the generator lives
  in the shared native core, so the same seed replays the same music in every
  Clausters client language. The context is also exposed directly as
  the draws sclang names (`clausters.rand`, `rrand`, `exprand`, `coin`,
  `choose`, `wchoose`, `gauss`, …, with the whole numbers as `irand`, `irand2`,
  `irrand`), and `current_rng()` and `spawn_rng()` for the stream itself (a
  `clausters.Rng`, which draws the same by method).
- `EventStreamPlayer` — `pbind(...).play(clock, server)` runs live or builds an NRT score depending on which interface the `Server` holds, with yield-exact timing (monotonic pacing, wall-clock timetags).

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
  sends and its master be built out of groups and still be sayable. A group also
  says **how** its members run: `group.auto_order()` has the server order them
  by the buses each one reads and writes rather than by the order they were
  added in, and `group.parallel()` has it run the independent ones on the DSP
  worker threads (`workers`), bit-identically. Both are the same analysis, and
  either can be handed back with `False`. Where the order is kept by hand,
  `AddAction` places a node when it is made and five verbs move one afterwards:
  `node.before(other)`, `node.after(other)`, `node.order(a, b, …)` — a whole
  chain in one message, so it arrives in the order it was written — and
  `group.head(…)` / `group.tail(…)`, which move nodes into the group. They are
  refused inside an auto-ordered group, which computes the order itself — and
  which mode a group is in is on its record (`info.auto_order`,
  `info.parallel`) and on its printed line, so a script can ask rather than
  find out from a refusal.
- `clocksync` — models the server's sample clock over UDP (`Server.sample_clock()`) for drift-free `/sched_at` timing without shared memory.
- **Introspection** — `Server.query_tree()` and `node.info()` read what is *playing* (the server is asked about every node it holds, a node about itself; every entry of the tree is the same record, and `print(tree)` draws it); `Server.query_defs()`, `query_buffers()` and `query_ugens()` read what the server **holds**: the loaded defs with their control surface, the allocated buffers, and the UGen catalog with named inputs and defaults. Worth asking rather than assuming — the def store persists across restarts, so a server can hold defs this client never sent. All blocking, so never from a routine.
- `Server` — **owns the communication interface and emits through it.** Swapping its interface retargets a routine from a live RT server to an NRT score without touching the clock or the routine. Interfaces include `OscUdpInterface`, `OscTcpInterface` (length-prefixed OSC; start the server with `--tcp`), and `OscWsInterface` (OSC over WebSocket, the browser-reachable transport; start the server with `--ws`), all drop-in.

The names that make up this layer's core — `Server`, `ServerOptions`, `Synth`, `Group`, `AddAction`, `SynthDef`, `FaustDef`, `GraphDef`, `Bus`, `Buffer` — are re-exported at the top level, so `from clausters import Server, Group, SynthDef, Bus` reads like the sequencing and timing imports beside it. The UGen and signal callables are not: they are a vocabulary of hundreds of names, and they stay under `clausters.defs`.

That is the whole rule for the top level, and it holds for every layer: **what you name while playing is flat, what is enumerative is named through its module.** So `seq.pbind` and `gui.knob`, like `defs.sine` — and, for the same reason from the other side, the transports and process launchers (`ipc.Clausters`, `ipc.ShmClient`, `launch.ServerProcess`, `launch.GuiProcess`) stay under theirs: you reach those as a property or an argument of the layer above, never by instantiating them. The layer modules themselves *are* re-exported, so `from clausters import *` gives you `defs`, `seq`, `gui`, `form`, `base`, `ipc`, `launch` and `errors` to reach through.

See [Defining instruments: FaustDef and SynthDef](defs.md) for the full def-building vocabulary — every `signals` / `ugens` callable, how the two def kinds differ, and how a def is sent behind the `/server_sync` barrier.

### Native plugin discovery

`server.scan_plugins("/plugins", timeout=30.0)` scans directories on the native
server and returns JSON metadata and per-binary error rows. With no directories,
it uses the server's CLAP and VST3 search roots. A Linux server built with `plugin-clap` or `plugin-vst3`
and its matching helper are required; ordinary builds reply with a capability
error. Scanning is blocking, so keep it outside clock routines. Use TCP for a
large catalogue and choose a timeout for all binaries. Stable identities, plain
parameter units, capacities and the persistence cache are specified in the
[server reference](https://clausters.readthedocs.io/en/latest/schemas.html#native-plugin-discovery).

## `clausters.Session` — ergonomic defaults, no globals

`Session` bundles a `Server` and a clock, with `Session.nrt()` / `Session.live()` factories and `.play(pattern)` / `.render()` / `.run(s)`. Several sessions coexist — an offline NRT one for plots next to a live RT one — in the same script. See [Sessions](sessions.md) for the full picture.

## The ambient verbs — `play`, `plot`, `render`

On top of the sessions sit three free-standing verbs that resolve the ambient context (the running session, else the default one), each a single entry for many kinds: `play` sounds what already sounds directly (an event or dict, a pattern, a routine or generator, a def or bare expression, a timeline, a sequence, a buffer), `render` performs the change of state to audio (defs and expressions, arrangement elements, timelines, patterns — offline to samples or a WAV, or delegated to a live destination), and `plot` (with its live sibling `scope`) is the visual counterpart. See [The ambient verbs](verbs.md) for the full dispatch tables and the play/render split.

## The seam, restated

Everything above the `Server` (clock, routines, patterns, defs) is written once. The `Server`'s interface is the only thing that decides whether the OSC reaches a live device, a shared-memory transport, or an in-process renderer producing a WAV. That is what lets a single script do a live take and an offline render side by side.

## Where the server contract lives

The wire formats this client emits — the OSC command set, the SynthDef JSON and Faust def formats, the node-tree and bus/buffer semantics, and the C ABI behind the native core — are specified in the **[Clausters server book](https://clausters.readthedocs.io/)**, not here. When in doubt about what a byte means on the wire, that is the source of truth; this client is one consumer of it.

Every command in that set is spelled by one rule — **`/<resource>_<action>`**,
the resource in full (`node`, `synth`, `group`, `bus`, `buffer`, `def`, `ugen`,
`server`, …), the action in camelCase, a reply as `<command>.reply`, a range as
`Range` — so the addresses this client builds can be read without a lookup. The
rule and its three corollaries open the server's OSC reference.


### Native plugin definitions

`PluginDef(name, reference)` sends a native binary reference with native IDs,
port routing and optional initial state. A plugin def builds a `Synth` like
any other def; its controls use `param:<id>`, `in:<id>` and `out:<id>`.
Query the loaded def to obtain its `ControlInfo` entries. CLAP parameter
controls use native plain units; VST3 uses normalized `[0, 1]` values. VST3
references use `format: "vst3"` and the scanned hexadecimal class ID. A native Linux
server and matching helper process CLAP and VST3 plugins outside the audio callback.
Latency includes the bounded block pipeline. General server delay compensation
aligns statically admitted automatic bus paths across all def families, including
convolution; use the group's `auto_order()`.
Nested automatic groups share the plan; manual/dynamic/feedback boundaries are
reported by `/group_dumpGraph`. Musical delays are unchanged. Native X11/XWayland editors open on the server's machine. See the
[protocol reference](https://clausters.readthedocs.io/en/latest/schemas.html#native-plugin-definitions-and-recovery).

`node.restart_plugin()` prepares recovery asynchronously and waits for its acknowledgement.

`node.open_plugin_editor()` opens or raises the plugin's native editor;
`node.close_plugin_editor()` closes it without stopping audio. A resizable
editor accepts `node.resize_plugin_editor(width, height)` in physical pixels.
`node.query_plugin_editor()` returns `open`, `width` and `height`. These calls
wait for the helper's main-thread acknowledgement. Editor parameter changes
update `/synth_get` queries and subscribed `/plugin_edit` notifications through the
same audio bridge. A mapped bus remains authoritative. State capture/load
and restart replace the helper, so they close its editor; reopen it afterward.
The installed Linux wheel includes the matching helper beside its server.

`state = node.save_plugin_state()` captures that live node's opaque bytes;
`node.load_plugin_state(state)` loads them into a live node. These calls wait
for completion and raise on failure. The image is independent of its def and
other instances, and becomes that node's recovery image for
`node.restart_plugin()`. Keep the bytes yourself for use after a server boot.
A capture does not interrupt the node: it keeps playing, held notes
included. A load is silent during preparation and clears held notes; routing and
maps survive, and it takes the loaded values for unmapped parameters.
Metadata is refreshed safely; incompatible port/control identities fail.
Use TCP or IPC for state images too large for one UDP datagram, and issue one
capture at a time per connection. Offline renders carry initial bytes in the
`PluginDef` reference instead.

Offline OSC scores can also carry plugin definitions and persistent events
when rendered by a native Linux build with `plugin-clap` or `plugin-vst3` and its matching
helper. The bridge latency stays in the samples; processing waits outside RT,
and a failed or hung helper aborts with the node id and processing time.
Plugins are deterministic only when their own implementation and inputs are;
the engine's random seed does not control a plugin's randomness.
See the [offline reference](https://clausters.readthedocs.io/en/latest/schemas.html#nrt-mode-offline-rendering).

### Persistent native instruments

`node.plugin_event(event)` sends notes, MIDI or native per-note expression to the
existing instance. Use distinct `source` origins for independent players and
note IDs for overlapping keys. A lane voice `{"plugin": node_id, "port":
native_port_id}` holds notes on that same instance; replacing or freeing the
lane releases only its notes. MIDI bindings of a plugin def likewise create
one persistent instance, using the shared MIDI decoder. MPE requires a CLAP
note-expression port; unsupported dialects report errors. Bind a governed or
followed transport for its playing state and position; an ungoverned live
instance continues processing while stopped. See the
[persistent-event reference](https://clausters.readthedocs.io/en/latest/schemas.html#persistent-plugin-events).
