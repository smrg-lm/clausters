# The client, layer by layer

The package is three layers plus a thin convenience wrapper. The split is deliberate: the value/timing work knows nothing about the server, and the server side is one swappable seam. The two clients have the same layers under the same names — {{alt `clausters.base`, `clausters.seq`, `clausters.defs`, `clausters.gui` and `clausters.session` || `base/`, `seq/`, `defs/`, `gui/` and `session.ts`}} — so a reader who knows one client finds the other at the same relative path.

## The core: what is not written in {{alt Python || TypeScript}}

Everything that is a *value* or a *time* comes from `clausters-core`, the same crate the server itself runs, reached {{alt through its C ABI || compiled to wasm}}. The OSC bytes, the beat/second/sample conversions, the bar grid, the NTP timetag, the beat-ordered scheduler queue, the seeded random stream, the builtins below and the node/bus/buffer allocators are all calls into it.

This is not an optimization. It is what makes the client numerically **equal** to the server and to the other client rather than merely close: a seeded pattern replays identically in three languages, and a beat resolves to the same instant on both sides of a socket, because there is one implementation.

<!-- web -->
It is fetched **on demand**, which is the one thing a script never has to
think about: the wasm is a separate artifact and a page pays for it when it is
going to use it, not on import. `Session`'s verbs and `Server`'s `boot()` /
`attach()` await it for you, so a program that opens a server never names it.
Everything else that reaches the core does:

```ts
await loadCore();      // the wasm, once; later calls reuse it
```

That is a real list, not a footnote — encoding a packet by hand, authoring a
bundle, building a `TempoMap` or drawing a curve, and **booting a `GuiHost`
alone**, which is the one that surprises: a host with no server still names its
widgets out of the core's id registry, so `new GuiHost().boot()` in a page that
opened no server comes after a `loadCore()`. Forgetting it is not silent — the
call throws saying which verb needed the core and that this line is missing.
<!-- /web -->

## {{alt `clausters.base` || `base`}} — server-agnostic timing and values

- `builtins` — scalar and sequence math, computed as `f32` through the native core (named at the top level too: {{alt `clausters.builtins`, which shadows nothing || `builtins`}}), so a value the client computes equals the one the server's UGens would compute. Three families: the operator/conversion set every SuperCollider user knows (`midicps`, `dbamp`, `clip2`, `scaleneg`, the windows read at a phase such as `hanwindow`, …) with `clip`, `wrap` and `fold` between two bounds; the **range maps** (`linlin`, `linexp`, `explin`, `expexp`, `lincurve`, `curvelin`, `range`, `exprange`), which read a value out of one range and write it into another; the two **perceptual frequency scales** a spectrogram axis is labeled in (`cpsmel`/`melcps`, `cpsbark`/`barkcps`), named the same `<from><to>` way; and sclang's **functions on a number that are not operators** — `degrad`, `snap`, `softround`, `moddif`, `bilin`/`biexp`, `lcurve`, `gausscurve`, `zapgremlins`, `nextpoweroftwo`, the special functions `erf`, `tgamma`, `besselj` …, over a scale `degkey`/`keydeg`, `nearestinscale`, `nearestinlist`, and over a whole list `blend` (point by point), `blendat`, `normalize`, `normalizesum`, `integrate`, `differentiate`, `resamp0`/`resamp1` and `asrandomtable`, whose table `tablerand` draws from. Those last two families are computed in `f64`, as sclang computes them: no UGen computes them, so there is no f32 result to match. All of them take a sequence as readily as a number — {{alt `midicps(range(0, 120))` || `midicps([...Array(120).keys()])`}} maps a hundred and twenty notes in one crossing.
- `absobject` — {{alt operator overloading || the operators as methods (`add`, `mul`, `midicps` …), since TypeScript has no operator overloading}}, the base for composing values and signals.
- `stream` — `Routine`/`Stream`, the `yield` coroutine layer. A routine must never block {{alt the clock thread || the page: it never awaits}}.
- `clock` — `TempoClock`, **timing only**: it schedules and paces, it does not talk to the server.
- `timebase` — monotonic, or anchored to the server's sample clock (`/sched_at`) for drift-free timing.
- `moment` — `Moment`, when something is happening: a clock and an exact beat on it. The one answer to "what time is it *for this event*", which is what a destination stamps onto the wire.
- `destination` — where OSC goes: the `Server` for our own, `OscDestination` for any other application.
- {{alt `netaddr`, `main` — addressing and a thread-local execution context || `main` — the execution context}}. No global state that would block running RT and NRT side by side.

See [Routines and clocks](routines-and-clocks.md) for driving these directly — writing a routine by hand — and [Timing models](timing-models.md) for the clock's timing modes (wall-clock, sample-locked, shared transport) and how to observe each.

## {{alt `clausters.seq` || `seq`}} — sequencing

- `Event` — a note plays a synth and frees it after its sustain.
- The value patterns (`pseq`, `pwhite`, `pseries`, ...) and `pbind`, an
  `EventPattern` — a pattern is the definition of a generator, and only one
  whose values are events plays. The random
  patterns (`pwhite`, `prand`) draw from the **random context** — the running
  routine's generator, derived at creation from the context that created it,
  seeded per session (`session.seed(n)`, or {{alt `clausters.seed(n)` || `seed(n)`}} for the
  default session; sclang's model, no per-pattern seeds). One seed reproduces that
  session end to end, sessions reproduce independently, and the generator lives
  in the shared native core, so the same seed replays the same music in every
  Clausters client language. The context is also exposed directly as
  the draws sclang names ({{alt `clausters.rand` || `rand`}}, `rrand`, `exprand`, `coin`,
  `choose`, `wchoose`, `gauss`, …, with the whole numbers as `irand`, `irand2`,
  `irrand`), and `{{api current_rng}}()` and `{{api spawn_rng}}()` for the stream itself (a
  `Rng`, which draws the same by method).
- `EventStreamPlayer` — what playing an event pattern gives back; it runs live or builds an NRT score depending on which {{alt interface || connection}} the `Server` holds, with yield-exact timing.
- `Timeline`, a beat-sorted plan that plays itself, and `EventSequence`, events as data — see [Timelines](timelines.md).

## {{alt `clausters.defs` || `defs`}} — the server side

- `ugens` — lowercase callables producing `Ugen`/`Control`, and `SynthDef` (sent with `/def_send synth`): the server's UGens wired into a graph.
- `FaustDef` (sent with `/def_send faust`), its peer: DSP the server JIT-compiles, built from `signals` (Faust's Signal API as lowercase callables), from `boxes` (its Box API — point-free, with `boxes.faust` opening the Faust libraries), or from Faust source. The three forms are equals; so are the two def families.<!-- web --> All three compile in a tab: `libfaust-wasm` runs in the NRT worker, and the module it emits is linked into the engine's own memory.<!-- /web -->
- Both families are built **instance-based, with no global build context**, and both instantiate as ordinary nodes in the same tree.
- `Node`/`Bus`/`Buffer` handles and their allocators. **The constructor creates the thing**: {{alt `Synth("beep", {"freq": 440})` || `new Synth("beep", { freq: 440 })`}} allocates an id and sends the `/synth_new`, so the synth is sounding by the time it returns; `{{api from_id}}` is a handle on a node that already exists, which sends nothing. A `Group` is **born named** —
  {{alt `Group("mixer")` || `new Group({ name: "mixer" })`}}, and `group.rename(...)` afterwards — a referenceable label
  on top of the node id: the id stays the identity every command uses, and the
  name is how you *refer* to the group instead of to a number, comes back in
  every node record, and makes the tree navigable by path
  ({{alt `Server.group_at("/mixer/drums")` || `server.groupAt("/mixer/drums")`}}). That is what lets a mixer's channels, its
  sends and its master be built out of groups and still be sayable. A group also
  says **how** its members run: `{{api group.auto_order}}()` has the server order them
  by the buses each one reads and writes rather than by the order they were
  added in, and `group.parallel()` has it run the independent ones on the DSP
  worker threads, bit-identically<!-- web --> (in a tab they do not run in parallel: `/group_parallel` is accepted and serializes, which produces the same samples — see [What a tab cannot do](platform.md))<!-- /web -->. Both are the same analysis, and
  either can be handed back with {{alt `False` || `false`}}. Where the order is kept by hand,
  `AddAction` places a node when it is made and five verbs move one afterwards:
  `node.before(other)`, `node.after(other)`, {{alt `node.order(a, b, …)` || `node.order([a, b, …])`}} — a whole
  chain in one message, so it arrives in the order it was written — and
  `group.head(…)` / `group.tail(…)`, which move nodes into the group. They are
  refused inside an auto-ordered group, which computes the order itself — and
  which mode a group is in is on its record ({{alt `info.auto_order` || `info.autoOrder`}},
  `info.parallel`) and on its printed line, so a script can ask rather than
  find out from a refusal.
- `clocksync` — models the server's sample clock over a socket for drift-free `/sched_at` timing without shared memory (what `{{api Server.sample_timebase}}()` builds).
- **Introspection** — {{alt `Server.query_tree()` || `server.queryTree()`}} and `node.info()` read what is *playing* (the server is asked about every node it holds, a node about itself; every entry of the tree is the same record, and printing it draws it); {{alt `Server.query_defs()`, `query_buffers()` and `query_ugens()` || `server.queryDefs()`, `queryBuffers()` and `queryUgens()`}} read what the server **holds**: the loaded defs with their control surface, the allocated buffers, and the UGen catalog with named inputs and defaults; `status` and `load` are the live counters, the latter broken down by role. Worth asking rather than assuming — the def store persists across restarts, so a server can hold defs this client never sent. {{alt All blocking, so never from a routine. || All asynchronous, so awaited before a routine starts, never inside one.}}
- `Server` — **owns the {{alt communication interface || connection}} and emits through it.** Swapping it retargets a routine from a live RT server to an NRT score without touching the clock or the routine. A command addressed to a resource is that resource's method — `def.send()`, `node.set`, `buffer.{{api get_samples}}` — so the receiver is never an argument. <!-- python -->Interfaces include `OscUdpInterface`, `OscTcpInterface` (length-prefixed OSC; start the server with `--tcp`), and `OscWsInterface` (OSC over WebSocket, the browser-reachable transport; start the server with `--ws`), all drop-in.<!-- /python --><!-- web -->Connections are `pageConnection()` over this tab's engine, `WsConnection.open(url)` over a socket to a `--ws` server, a worker's link onto the page's engine, and `OscNrtInterface` for a score: a handle built with an explicit `connection` uses that one, which is how an offline handle is made (`new Server({ connection: new OscNrtInterface() })`).<!-- /web -->

<!-- python -->
The names that make up this layer's core — `Server`, `ServerOptions`, `Synth`, `Group`, `AddAction`, `SynthDef`, `FaustDef`, `GraphDef`, `Bus`, `Buffer` — are re-exported at the top level, so `from clausters import Server, Group, SynthDef, Bus` reads like the sequencing and timing imports beside it. The UGen and signal callables are not: they are a vocabulary of hundreds of names, and they stay under `clausters.defs`.

That is the whole rule for the top level, and it holds for every layer: **what you name while playing is flat, what is enumerative is named through its module.** So `seq.pbind` and `gui.knob`, like `defs.sine` — and, for the same reason from the other side, the transports and process launchers (`ipc.Clausters`, `ipc.ShmClient`, `launch.ServerProcess`, `launch.GuiProcess`) stay under theirs: you reach those as a property or an argument of the layer above, never by instantiating them. The layer modules themselves *are* re-exported, so `from clausters import *` gives you `defs`, `seq`, `gui`, `form`, `base`, `ipc`, `launch` and `errors` to reach through.
<!-- /python -->
<!-- web -->
The names that make up this layer's core — `Server`, `Synth`, `Group`, `AddAction`, `SynthDef`, `FaustDef`, `GraphDef`, `Bus`, `Buffer` — are exported from `clausters` itself, so `import { Server, Group, SynthDef, Bus } from "clausters"` reads like the sequencing and timing imports beside it. The UGen and signal callables are a vocabulary of hundreds of names and are imported from `clausters/defs`; the patterns from `clausters/seq`, the builders from `clausters/gui`, the streams from `clausters/data`.
<!-- /web -->

See [Defining instruments: FaustDef and SynthDef](defs.md) for the full def-building vocabulary — every `signals` / `ugens` callable, how the two def kinds differ, and how a def is sent behind the `/server_sync` barrier.

### Native plugin discovery

{{alt `server.plugins()` || `await server.plugins()`}} returns a `PluginCatalog` of the CLAP and VST3 plugins
installed on the native server's machine: the server's standard search roots,
or the directories and binaries given ({{alt `server.plugins("/opt/plugins")` || `server.plugins(["/opt/plugins"])`}}). Each
entry is a `Plugin`, found by what it is called rather than by its native id:

```python
catalog = server.plugins(timeout=60.0)
eq = catalog.find("Parametric Equalizer x8 Stereo")
catalog.find("Keys", format="vst3")          # the name is installed in both
catalog.filter(kind="effect", tag="reverb")  # every reverb
```

```ts
const catalog = await server.plugins([], { timeout: 60.0 });
const eq = catalog.find("Parametric Equalizer x8 Stereo");
catalog.find("Keys", { format: "vst3" });          // the name is installed in both
catalog.filter(null, { kind: "effect", tag: "reverb" });   // every reverb
```

A `Plugin` carries its identity (`format`, `path`, `id`, `key`), what it says
about itself (`name`, `vendor`, `version`, `description`, `url`), its audio and
note ports, its parameters and what it supports (`{{api has_state}}`, `{{api has_editor}}`).
**What it is** comes from what it declares in either format's vocabulary
(`features`, as declared): `kind` (`"instrument"`, `"effect"`,
`"note_effect"`, `"analyzer"`, `"other"`), `tags` (`"synth"`, `"reverb"`,
`"eq"`, ...) and `channels` (`"mono"`, `"stereo"`, ...). `find` {{alt raises || throws}} when
nothing matches or when several still do, naming the candidates. Data a
plugin loads from outside, such as an impulse response, has no standard
declaration in either format; it lives in the plugin's state.

The server keeps what it found: a binary unchanged since it was inspected is
not inspected again, so asking twice is cheap, and {{alt `rescan=True` || `{ rescan: true }`}} inspects
every binary. A Linux server built with `plugin-clap` or `plugin-vst3` and its
matching helper are required; ordinary builds reply with a capability error<!-- web -->, and so does an in-page engine, which cannot load native binaries or create helper processes: a tab reaches a native server's plugins over its WebSocket<!-- /web -->.
Discovery {{alt is blocking, so keep it outside clock routines; use TCP for a large catalogue || is awaited, so keep it outside clock routines}} and choose a timeout for all binaries. `{{api server.scan_plugins}}()`
returns the rows underneath. Stable identities, plain parameter units,
capacities and the cache are specified in the
[server reference](https://clausters.readthedocs.io/en/latest/schemas.html#native-plugin-discovery).

### Native plugin definitions

{{alt `PluginDef(name, plugin)` || `new PluginDef(name, plugin, { inputs, outputs, state })`}} is the decision about one `Plugin`: which buses its
audio ports use and the state it starts from. `outputs` defaults to bus 0 and
`inputs` to unconnected (an input reads silence); a bus number puts the main
port there and leaves auxiliary ports (a sidechain, separate outputs)
unconnected, and `{port: bus}` routes the ports it names. A port the plugin
does not have is refused before anything is sent:

```python
PluginDef("eq", eq, inputs=4).send(server)                  # main in on 4, main out on 0
PluginDef("duck", comp, inputs={"in": 4, "in_sidechain": 8})
```

```ts
await new PluginDef("eq", eq, { inputs: 4 }).send(server);  // main in on 4, main out on 0
new PluginDef("duck", comp, { inputs: { in: 4, in_sidechain: 8 } });
```

A plugin def builds a `Synth` like any other def. Its controls answer to the
names in `plugin.controls` — `in` and `out` for the main audio ports,
`in_<name>`/`out_<name>` for the others, a parameter's name — and always to
their native-id names, `param:<id>`, `in:<id>` and `out:<id>`; a negative bus
leaves a port unconnected. Query the loaded def to obtain its `ControlInfo`
entries. CLAP parameter controls use native plain units; VST3 uses normalized
`[0, 1]` values. The def's `reference` is its stored form, and
{{alt `PluginDef(name, reference)` || `new PluginDef(name, reference)`}} rebuilds one from it. A native Linux
server and matching helper process CLAP and VST3 plugins outside the audio callback.
Latency includes the bounded block pipeline. General server delay compensation
aligns statically admitted automatic bus paths across all def families, including
convolution; use the group's `{{api auto_order}}()`.
Nested automatic groups share the plan; manual/dynamic/feedback boundaries are
reported by `/group_dumpGraph`. Musical delays are unchanged. Native X11/XWayland editors open on the server's machine<!-- web -->, a browser's included<!-- /web -->. See the
[protocol reference](https://clausters.readthedocs.io/en/latest/schemas.html#native-plugin-definitions-and-recovery).

`{{api node.restart_plugin}}()` prepares recovery asynchronously and {{alt waits for || resolves with}} its acknowledgement.

`{{api node.open_plugin_editor}}()` opens or raises the plugin's native editor;
`{{api node.close_plugin_editor}}()` closes it without stopping audio. A resizable
editor accepts `{{api node.resize_plugin_editor}}(width, height)` in physical pixels.
`{{api node.query_plugin_editor}}()` returns `open`, `width` and `height`. These calls
wait for the helper's main-thread acknowledgement. Editor parameter changes
update `/synth_get` queries and subscribed `/plugin_edit` notifications through the
same audio bridge. A mapped bus remains authoritative. State capture/load
and restart replace the helper, so they close its editor; reopen it afterward.<!-- python -->
The installed Linux wheel includes the matching helper beside its server.<!-- /python -->

{{alt `state = node.save_plugin_state()` || `const state = await node.savePluginState()`}} captures that live node's opaque {{alt bytes || `Uint8Array`}};
`{{api node.load_plugin_state}}(state)` loads them into a live node. These calls wait
for completion and {{alt raise || throw}} on failure. The image is independent of its def and
other instances, and becomes that node's recovery image for
`{{api node.restart_plugin}}()`. Keep the bytes yourself for use after a server boot.
A capture does not interrupt the node: it keeps playing, held notes
included. A load is silent during preparation and clears held notes; routing and
maps survive, and it takes the loaded values for unmapped parameters.
Metadata is refreshed safely; incompatible port/control identities fail.
Use a stream carrier ({{alt TCP or IPC || a WebSocket}}) for state images too large for one datagram, and issue one
capture at a time per connection. Offline renders carry initial bytes in the
`PluginDef` reference instead.

Offline OSC scores can also carry plugin definitions and persistent events
when rendered by a native Linux build with `plugin-clap` or `plugin-vst3` and its matching
helper<!-- web --> (an in-page render cannot load native plugins)<!-- /web -->. The bridge latency stays in the samples; processing waits outside RT,
and a failed or hung helper aborts with the node id and processing time.
Plugins are deterministic only when their own implementation and inputs are;
the engine's random seed does not control a plugin's randomness.
See the [offline reference](https://clausters.readthedocs.io/en/latest/schemas.html#nrt-mode-offline-rendering).

### Persistent native instruments

`{{api node.plugin_event}}(event)` sends notes, MIDI or native per-note expression to the
existing instance. Use distinct `source` origins for independent players and
note IDs for overlapping keys; an event without a `port` goes to the main
note input. A lane voice `{"plugin": node_id}` (with an optional `"port"`)
holds notes on that same instance; replacing or freeing the
lane releases only its notes. MIDI bindings of a plugin def likewise create
one persistent instance, using the shared MIDI decoder. MPE requires a CLAP
note-expression port; unsupported dialects report errors. Bind a governed or
followed transport for its playing state and position; an ungoverned live
instance continues processing while stopped. See the
[persistent-event reference](https://clausters.readthedocs.io/en/latest/schemas.html#persistent-plugin-events).

## `Session` — ergonomic defaults, no globals

`Session` bundles a `Server` and a clock, with `Session.nrt()` / `Session.embed()` / `Session.live()` factories and `.play(pattern)` / `.render()` / `.run(s)`. Several sessions coexist — an offline NRT one for plots next to a live RT one — in the same {{alt script || page}}. See [Sessions](sessions.md) for the full picture.

## The ambient verbs — `play`, `plot`, `scope`, `render`

On top of the sessions sit free-standing verbs that resolve the ambient context (the running session, else the default one), each a single entry for many kinds: `play` sounds what already sounds directly (an event or {{alt dict || plain object}}, a pattern, a routine or generator, a def or bare expression, a timeline, a sequence, a buffer), `render` performs the change of state to audio (defs and expressions, form elements, timelines, patterns — offline to samples or a WAV, or delegated to a live destination), and `plot` (with its live sibling `scope`) is the visual counterpart. See [The ambient verbs](verbs.md) for the full dispatch tables and the play/render split.

## The seam, restated

Everything above the `Server` (clock, routines, patterns, defs) is written once. The `Server`'s {{alt interface || connection}} is the only thing that decides whether the OSC reaches a live device, {{alt a shared-memory transport || the engine in this tab}}, or an in-process renderer producing a WAV. That is what lets a single {{alt script || page}} do a live take and an offline render side by side.<!-- web --> Swapping carriers is a one-line edit in a program of any size, which is exactly the property the examples demonstrate by offering a radio button.<!-- /web -->

<!-- web -->
## What the browser changes

**Everything that waits is a promise.** Where the Python client blocks a thread on a reply, this one `await`s: `await server.sync()`, `await bus.get()`, `await server.queryTree()`. The page has a single thread and has to keep running, so "never block the clock thread" — a discipline in the Python client — is here simply the language.

**The graph composes by method.** `sine(freq).mul(amp).add(bias)` where Python writes `sine(freq) * amp + bias`, TypeScript having no operator overloading. Because of that, parity between the clients is asserted on the **emitted spec**, never on the source text.

**A path is the page's own storage.** Wherever a verb takes a path — `render`'s `path`, a `SoundFile`, `Buffer.read`, a saved session — a tab means its origin private file system (`opfs`) and node means the disk, so a take a render writes is a file the engine reads back. A render to a `path` is written as it is rendered, a run at a time, in a Worker of its own in a tab (so two renders run at once): only the run is held, and the files the score reads — a list of takes a `TransportDiskIn` plays, a `DiskIn`'s file — are read the moment the render needs them, so what is rendered is what a native render renders. `workers` has no counterpart: the wasm renderer runs on the calling thread, and wasm threads need a cross-origin isolation the embedding page has to grant.

**Nothing pumps.** There is no drain call and no event loop of ours: a page subscribes once (`onEvent`, `onClosed`, an [`OscFunc`](responders.md) or the raw `onReply` under it) and the host's or server's messages arrive as calls, while a query resolves a promise.
<!-- /web -->

## Errors

`ClaustersError` is the root: `CommandError` for a `/fail` from the server, `ReplyTimeout` for a reply that never came, `AllocationError` for an exhausted or double-released id pool. They are ordinary exceptions from ordinary {{alt calls || async calls}}, {{alt in `clausters.errors` || exported from `clausters/errors`}}.

## Where the server contract lives

The wire formats this client emits — the OSC command set, the SynthDef JSON and Faust def formats, the node-tree and bus/buffer semantics, and the C ABI behind the native core — are specified in the **[Clausters server book](https://clausters.readthedocs.io/)**, not here. When in doubt about what a byte means on the wire, that is the source of truth; this client is one consumer of it.

Every command in that set is spelled by one rule — **`/<resource>_<action>`**,
the resource in full (`node`, `synth`, `group`, `bus`, `buffer`, `def`, `ugen`,
`server`, …), the action in camelCase, a reply as `<command>.reply`, a range as
`Range` — so the addresses this client builds can be read without a lookup. The
rule and its three corollaries open the server's OSC reference.
