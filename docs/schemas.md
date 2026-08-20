# Def schemas

User-facing reference: wire formats and OSC commands. For the server's internals — threads, memory lifecycle, invariants, adding UGens — see [`architecture.md`](architecture.md).

## How a command is named

Every command in this reference is spelled by one rule, and knowing it means most of the surface can be guessed rather than looked up:

**`/<resource>_<action>`** — the resource is the *kind of thing the command addresses*, spelled in full (`node`, `synth`, `group`, `bus`, `buffer`, `def`, `ugen`, `graph`, `server`, `midi`, `transport`, `sched`, `clock`); the action is a verb in camelCase (`set`, `free`, `query`, `allocRead`, `tapStream`).

Three corollaries carry most of the vocabulary:

- **A reply is `<command>.reply`.** Every answer the server sends back is the command's own address plus that suffix — `/node_query` → `/node_query.reply`, `/bus_get` → `/bus_get.reply`, `/server_status` → `/server_status.reply`. A stream pushes under the same address as its subscription's reply (`/bus_stream` → `/bus_stream.reply`). The exceptions are the three answers that belong to no single command: `/done` and `/fail` (which name the command in their first argument) and the node notifications (`/node_start`, `/node_end`, `/node_trigger`), which nothing asked for. One command answers under another's address on purpose: `/synth_get`/`/synth_getRange` reply `/node_set`, the very message that writes those controls back (see below).
- **The resource is what is addressed, not what it holds.** `/node_ugenCmd` reaches a UGen *inside a node*, so it is a `node` command; `/ugen_query` reads the catalog of UGen kinds, so it is a `ugen` command. Both control buses and audio buses are `bus` — one index space each, one namespace.
- **A range is `Range`.** Where scsynth appends an `n` for "n consecutive", the action ends in `Range`: `/node_setRange`, `/bus_getRange`, `/buffer_getRange`.

Clausters deliberately **does not** reproduce scsynth's command names. It keeps scsynth's *model* — the node tree, the add actions, the bus and buffer pools, the async barrier — while spelling the commands consistently, because the one-letter prefixes (`/s_new`, `/c_get`, `/b_allocRead`) had grown a second tier of full-word namespaces for everything scsynth never had, and two conventions in one protocol is one too many. A reader coming from SuperCollider will recognize every concept and no address; the mapping is mechanical (`/s_new` → `/synth_new`, `/c_getn` → `/bus_getRange`, `/d_recv` → `/def_send synth`).

Clausters accepts instrument definitions in two wire formats, both loaded hot over OSC (UDP, default port `127.0.0.1:57110`):

| format | OSC command | availability |
|---|---|---|
| **SynthDef JSON** — a flat list of UGens | `/def_send synth <blob>` | `synth` feature (default) |
| **Faust def** — Faust source *or* a JSON box tree | `/def_send faust <name> <string>` | `faust` feature |

The two families are independent build features and combine freely; a server built without one replies `/fail` (naming the missing feature) to that family's def command. See the feature matrix in [Using clausters as a library](using-as-a-library.md) and `BUILD.md`.

**One command, the family as its first argument.** `/def_send <family> <payload…>` sends a def of any family — `"synth"` (one `SynthDefSpec` JSON blob), `"faust"` (a name and a payload) or `"graph"` (one `GraphDefSpec` JSON blob, see [GraphDef](#graphdef-def_send-graph-graph_new--node-graph-programs)). The family is a wire argument rather than three commands because a def *has* a family either way: it is what `/def_query` reports back, in the same three spellings. An unknown family is a `/fail`.

Both reply asynchronously: `/done "/def_send" <family>` — a Faust compile, which finishes later on its own thread, appends the def name — or `/fail "/def_send" <reason>` with a human-readable error. Once loaded, defs of either kind are instantiated, controlled and freed the same way:

### The async barrier (`/server_sync`)

`/server_sync <int id>` is the general way to wait for asynchronous work: the server replies `/server_sync.reply <id>` once **every async command received before this `/server_sync`** has completed — Faust compiles (`/def_send faust`), SynthDef sends (`/def_send synth`) and buffer jobs (`/buffer_*`), each of which runs on its own FIFO worker thread. Use it instead of matching individual `/done`s when you fire several async commands and just want to know they have all landed (e.g. after sending a def without waiting, before the `/synth_new` that needs it). It is a real barrier, not a round-trip of `/server_status`: a `/server_sync.reply` guarantees the prior compiles/jobs are installed.

```text
/synth_new  name id addAction targetID [ctlName value]...   # ctl args: s f pairs
/node_set  id ctlName value...                             # one control per pair
/node_setRange id ctl numControls value...                    # a consecutive range
/node_fill id ctl numControls value                       # fill a range with one value
/node_map  id ctlName busIndex...                          # control bus -> control
/node_mapAudio id ctlName busIndex...                          # audio bus  -> control
/node_mapRange id ctl busIndex numControls...                 # map a range to consecutive buses
/node_mapAudioRange id ctl busIndex numControls...                # audio-bus range variant
/synth_get  id ctl...                    -> /node_set id (ctl value)...        # read controls
/synth_getRange id ctl numControls...        -> /node_set id (ctl numControls value...)...
/node_free id...
/node_trace id...                                          # log a node's state (debug)
/synth_forgetId id...                                           # acknowledged (see note)
/def_free name...                                         # SynthDef JSON only
```

Engine facts that apply to every def: blocks of 64 samples; **by default** 128 audio buses (`0..outputs` are the hardware outputs, bus 0 = left; `outputs..outputs+inputs` are the hardware inputs) and 16384 control buses; a pool of 4096 sample buffers filled by the `/buffer_*` commands; all signals are `f32` at the configured sample rate. Everything sized at boot, never at compile time: the bus counts by `--audio-buses` (≤128) and `--control-buses`, the sample rate by `--sample-rate` (default 48000; PipeWire honors it per-application), the hardware channel counts by `--outputs` (default: the device's) and `--inputs` (default 0), the pre-allocated pools by `--max-nodes`, `--max-buffers`, `--max-graph-children` and `--max-ugen-inputs` (≤32), and the audio-tap region by `--taps` and `--tap-frames` (defaults 8 × 16384; see "Audio taps" below). Each flag also has a `[server]` config key of the same name (`max_nodes`, `outputs`, …), with the CLI winning. A client reads the live configuration with **`/server_query`** → `/server_query.reply [audio_buses, control_buses, output_channels, block_size, nominal_sr, actual_sr, input_channels, max_nodes, max_buffers, max_graph_children, max_ugen_inputs, taps, tap_frames, max_frame]`, so it can size its own bus/node allocators from the server instead of assuming the defaults. The first six fields are the stable original set; the S7 capacities, the tap region shape and the stream-transport frame ceiling (`--max-frame` — what a client should size bulk requests like `/buffer_getRange` chunks from, see "Transports" below) are appended, so a client that reads only the first six keeps working.

**Node ids are a finite, boot-partitioned resource.** Like every capacity above, the node-id space is fixed at boot and **every range of it recycles** — an id allocator (server- or client-side) is a *registry* of ids in use, never a counter that only climbs. The space is partitioned by one shared formula (`clausters_core::registry::NodeIdPartition`, scaled from `max_nodes`, the resource that actually bounds concurrent nodes): a **client range** starting at 1000 (width `4 × max_nodes` — table capacity plus in-flight margin), then the server's **auto range** (`/synth_new` with id `-1`, GraphDef groups and members) and its **MIDI voice range**, each `2 × max_nodes` wide. At the default `--max-nodes 8192` that is client `1000..33768`, auto `33768..50152`, MIDI `50152..66536`. A client derives the partition from the `max_nodes` field of `/server_query.reply` with the same formula (exposed by the core FFI), so client and server agree by query, not convention. Ids return to their registry when the node dies: the server releases its auto/MIDI ids on each node's end (and on an engine rejection), and a client releases its ids from the same **`/node_end`** notifications — register with `/server_notify 1` and free on `/node_end` (releasing at `/node_free`-send time would re-hand an id whose node is still alive). When the engine **rejects** a node asynchronously (duplicate id, bad target, full table), no `/node_end` will ever come, so the server broadcasts **`/fail "/synth_new" <message> <int id>`** to every `/server_notify` client — the appended id (not part of scsynth's two-argument `/fail`) is what lets a client registry reconcile the in-flight id instead of losing it. Exhausting a range is an explicit error (`/fail … out of auto node ids`, or the client's own exception), never a silent wrap; in NRT/score mode the client id space is deliberately unbounded — an offline score has no live `/node_end` stream and no real-time bound on ids over its length.

**The client range is one range, and several clients may be on it.** The partition tells a client where its ids begin; it does not tell two clients of the same server how to stay out of each other's way, and nothing on the wire does — both registries start at the same base and hand out the same first id, so the second `/synth_new` of the pair is refused as a duplicate, or worse, lands on the other client's node. Clients that share a server therefore split the range **client-side**, by taking equal slices of it in a fixed order: with `n` clients, client `i` allocates from slice `i` of `n`, the last slice taking the remainder so the range is tiled exactly. There is nothing to negotiate — the slices are disjoint by arithmetic — so two clients with no channel between them agree as long as whoever arranged them handed each its index. The same split applies to every client-side space (audio buses, control buses, buffers, and the GUI host's widget ids), taken *after* the server's own reservations, so a slice never opens a hole below itself. It costs range, not capability: a split of two halves what either client may hold live at once, so a server's only client takes the whole space and pays nothing. Both clients implement it (`clausters.base.IdShare` / `IdShare` in the web client).

**Server health** is one poll away: **`/server_status`** → `/server_status.reply [1, ugens, synths, groups, defs, avgCPU, peakCPU, nominal_sr, actual_sr, late_blocks]` — the scsynth field set plus one appended int. `avgCPU` and `peakCPU` are the audio thread's per-block processing time as a **percentage of the block budget** (`block_size / sample_rate` of wall time — at 48 kHz a 64-frame block must be computed in 1.33 ms): the average is an exponential moving average with a ~1 s time constant, the peak is the worst single block **since the previous `/server_status` poll** (reading it resets the window, so every poll reports the peak of its own interval; expect it to sit well above the average — the callback must fit its *worst* block, not its mean). `late_blocks` counts, cumulatively since boot, the blocks whose processing exceeded their budget — the engine-side xrun proxy. It is conservative: when the device quantum is larger than one block (PipeWire typically runs 256–1024 frames), a single late block can be absorbed by the rest of the cycle, so an occasional increment is a warning, a steady climb is audible trouble (cross-check with `pw-top`'s ERR column). Both meters read the real callback only: in an offline render they just measure render speed. `examples/stress.rs` builds a capacity ramp on top of this reply — see [Examples](examples.md).

**Live audio input.** With `--inputs N` the server also opens the default input device and exposes its `N` channels on audio buses `outputs..outputs+inputs`, so `In` (or `In.ar`) reading those bus indices returns live device input — a microphone, a loopback, another app's output routed in via PipeWire/JACK. Without `--inputs`, those buses read as silence (no input device is opened). The count is negotiated with the host and may be clamped; the value actually opened is the `input_channels` field of `/server_query.reply` (0 if the input device was unavailable).

### Mapping controls to buses (`/node_map`, `/node_mapAudio`)

`/node_set` writes a control once. `/node_map id ctl bus` instead **binds** the control to a **control bus**: the node re-reads that bus at the start of every block, so the control tracks whatever any client (`/bus_set`) or synth (`OutCtl`) writes there — no further `/node_set`. `/node_mapAudio` is the same against an **audio bus**. Both take any number of `ctl bus` pairs, by control name or index, and work for UGen controls and Faust parameters alike.

A `busIndex` of `-1` removes the mapping (the control keeps its last value); a later `/node_set` on the same control also clears it and fixes the value.

### Ranges of controls (`/node_setRange`, `/node_fill`, `/node_mapRange`, `/node_mapAudioRange`)

The `-n` variants address **consecutive** controls in one message, for defs with array-like control blocks. `/node_setRange id ctl numControls value...` sets `numControls` controls starting at `ctl` (by name or index) from the value list; several `(ctl, numControls, values...)` groups may follow. `/node_fill id ctl numControls value` fills such a range with a single value (repeatable in `(ctl, numControls, value)` triples). `/node_mapRange id ctl busIndex numControls` maps `numControls` consecutive controls to `numControls` **consecutive** buses starting at `busIndex` (`busIndex = -1` unbinds the whole range); `/node_mapAudioRange` is the audio-bus form. Like `/node_set`/`/node_map`, all four accept a **group** id and propagate down its subtree, and clearing/setting a control that is used as a bus index re-sorts auto/parallel groups.

### Reading control values (`/synth_get`, `/synth_getRange`)

`/synth_get id ctl...` is the read counterpart of `/node_set`: it replies `/node_set id (ctl value)...` with each requested control's current value (by name or index). `/synth_getRange id ctl numControls...` reads a **range**, replying `/node_set id (ctl numControls value...)...`. Values come from the server-side node mirror, so they reflect the latest `/node_set`/`/node_setRange`/`/synth_new`. An unknown node or out-of-range control replies `/fail`.

### `/synth_forgetId` and `/node_trace`

`/synth_forgetId id...` exists for scsynth compatibility. In scsynth it releases integer node IDs back to the pool; Clausters assigns IDs per client (auto IDs come from a reserved server range) and never reuses a live or freed ID under a new node, so there is nothing to release — the command validates that the IDs name live synths and replies `/done`. `/node_trace id...` is a debug aid: it logs each node's current control values (or a group's children) to the server console through the `clausters::osc` trace target, no OSC reply (matching scsynth's console trace).

### Pausing and resuming nodes (`/node_run`)

`/node_run id flag …` (pairs of `nodeID, flag`) pauses (`flag = 0`) or resumes (`flag = 1`) a node — a synth or a whole **group** (a paused group skips its entire subtree). A paused node stays in the tree and keeps its state; it is just skipped during processing, so it is silent and consumes no CPU, and resumes exactly where it left off. This is what resumes a synth parked by `doneAction 1` (`pauseSelf`) — pause is not terminal. An unknown id replies `/fail`.

### Addressing a group (scsynth group semantics)

`/node_set`, `/node_map` and `/node_mapAudio` accept a **group** id as well as a synth id. Addressed to a group, the command transfers each named control **down the group's subtree** to every synth (and Faust node) that has a control of that name, recursing through subgroups and stopping at each synth — the standard scsynth behaviour, so one message moves a parameter across a whole bank of nodes. A node without a matching control name is simply skipped; an empty group is a no-op; an unknown id replies `/fail`. Addressed to a single synth, the command sets only that synth, as before.

Because a control is one value per block, `/node_mapAudio` **samples** one frame of the audio bus per block (control rate) — this matches scsynth for a control-rate control; there are no audio-rate controls here (feed an audio signal through `In`/an input bus instead). Mapping a control that is used as a bus index makes the node a dynamic barrier for auto/parallel groups, and an audio map adds that bus to the node's reads so the dependency analysis stays correct.

### Naming a group (`/group_name`, `/group_query`)

A group can carry a **name**: a referenceable label, not a new identity. The node ID remains what every command addresses and every reply reports; the name is a second way to *refer* to the same group — one you choose, that survives being read back and that says what the group is.

A group is **born named**: `/group_new` takes an optional string after each `(id, addAction, targetID)` triple, so the label travels with the creation, in one message, at the moment the client knows what it is building. `/group_name <groupID> <name>` is for **renaming** afterwards (repeatable `groupID name` pairs), and an **empty name clears it**. A group's label is free to change; its ID is not.

The label is what makes the tree navigable by **path**. Every group contributes one path segment — its name if it has one, its **decimal ID** if it does not — so `/mixer/drums` and `/1000/drums` are both valid paths and *every* group is reachable, named or not. The root group is `/`. **`/group_query <path>...`** resolves one, replying `/group_query.reply <path> <nodeID>` per path, with **`nodeID = -1`** when nothing answers to it (absence is a state, the same rule `/node_query` follows). A path is never accepted where a node ID goes: resolve it once, then command the group by the ID you got back.

Paths are composed on the walk and never stored, so renaming a group re-paths its whole subtree at once. Three rules the server enforces, each replying `/fail` and changing nothing:

- **Unique among siblings** — two children of one group cannot answer to the same segment. The same name under *different* parents is exactly the point: `/g1/mixer` and `/g2/mixer` are two channels' mixers.

- **Never all digits** — a numeric name would speak for another group's ID segment. (Digits *inside* a name are fine: `8bit`.)
- **No `/`** — the server composes the path from the tree; the client names one group at a time.

A group with a name still answers to its ID in a path, because the name never replaced it. And since names live only in the network-side tree mirror, naming a group costs the audio thread exactly nothing.

A name a group is created with is validated the same way and **before the group exists**, judged against the group it would land in: a refused label replies `/fail` and the group **is not created**. A client that asked for a named group is never left holding an anonymous one it did not ask for.

The label also rides the **node notifications**: `/node_start` and `/node_end` carry it as their last argument (empty for a synth or an unnamed group), so a client watching the tree learns *which* channel came up or went away without a follow-up query — and for a death there is no query left to make.

The clients build this as a label you pass at creation: `Group("mixer")` / `Group.new(server, { name: "mixer" })`, `group.rename(...)`, and `Server.group_at("/mixer/drums")` / `groupAt(...)` to resolve one.

### Moving nodes in the tree (`/node_before`, `/node_after`, `/group_head`, `/group_tail`, `/node_order`)

Execution order within a group is the child order, and these commands rewrite it. `/node_before id target` / `/node_after id target` move a node just before/after a sibling (any number of `id target` pairs). `/group_head group id` / `/group_tail group id` move a node to the **head/tail** of a group (pairs of `group id`). `/node_order addAction target id...` moves **several** nodes to one place at once, keeping their listed order: `addAction` selects `0` head of the target group, `1` tail, `2` before the target node, `3` after it.

All of these are disabled inside an **auto-sorted group** (`/group_sortMode … 1`): there the execution order is recomputed from the bus-connection DAG, so a manual move replies `/fail` — use auto-sort, or a manually-ordered group, but not both. (This is why `/node_order` earns its place only in manual groups: it is a batch `/node_before`/`/node_after`/`/group_head`/`/group_tail`.)

### Control buses (`/bus_set`, `/bus_setRange`, `/bus_fill`, `/bus_get`, `/bus_getRange`, `/bus_stream`)

Control buses are shared `f32` slots any client or synth reads and writes — the glue between `/bus_set` and `/node_map`. `/bus_set bus value...` writes single buses (pairs); `/bus_setRange bus numBuses value...` writes a **consecutive range** from a value list (repeatable groups); `/bus_fill bus numBuses value` fills a range with one value (repeatable triples). `/bus_get bus...` reads single buses, replying `/bus_get.reply (bus value)...`; `/bus_getRange bus numBuses...` reads ranges, replying `/bus_getRange.reply (bus numBuses value...)...`. Unset buses read `0.0`. The immediate forms write the shared atomics on the network thread; inside a **timed bundle** the writes travel to the audio thread so they land on the exact scheduled sample.

Beyond scsynth, **`/bus_stream periodMs bus...`** subscribes the sending client to a **periodic snapshot** of the listed buses: the server acks `/done /bus_stream`, sends one `/bus_stream.reply (bus value)...` immediately, and keeps sending one every `periodMs` (clamped to a 10 ms floor; at most 128 buses per subscription) without further requests. It is the network counterpart of reading the control buses from the shared-memory segment, for clients that cannot map it — a browser GUI's meters and scopes over WebSocket. One subscription per client, **replaced** by each new `/bus_stream`; `periodMs <= 0` (or an empty bus list) cancels. A subscription dies with its TCP/WebSocket connection; over UDP or the shared-memory ring it lasts until an explicit cancel or `/server_quit` (the `/server_notify` posture). Not schedulable in a timed bundle.

### Watching audio buses (`/bus_tap`, `/bus_tapStream`)

A control bus lives permanently in the shared segment, so anyone can read its value with a load. An audio bus does not: it is engine memory, one block at a time, and nothing outside the audio thread can see it. So the segment carries a fixed set of single-channel **sample rings** (`--taps` of them, `--tap-frames` samples each — defaults 8 × 16384; `--taps 0` disables the region), sized at boot and reported in `/server_query.reply`, and the server copies a bus into one on request. That is what lets a scope see the *samples* of a live signal: SuperCollider answers this with `ScopeOut2` writing into scope buffers; here the ring is a first-class segment region and asking for a bus is a command, not a UGen. **The rings are the server's own bookkeeping** — a client asks for a *bus*, never for a ring, and the segment publishes which ring ended up carrying it.

**`/bus_tap bus watch`** asks the server to record audio bus `bus` (`watch = 1`) or to stop (`0`). From the next block on, the audio thread appends that bus's samples to one of the rings (RT-safe — one `memcpy` and one atomic store per block), where a local peer (the GUI host's oscilloscope) reads the newest window straight out of shared memory, with zero per-frame messages. **The bus is the only number a client names**: the server picks the ring, publishes the choice in the segment's bus directory for whoever reads the samples, and counts watches — two views of one bus share a ring, and the last one to stop frees it. No ack, like `/node_map` (it only flips routing state; failures reply `/fail` — an unknown bus, no tap region, or every ring already in use) — sequence with `/server_sync` when needed. A **meter** needs none of this: the engine publishes a per-bus **level** every block — the block peak held with a 20 dB/s decay, so a reader far slower than the engine still catches a transient — and the same segment carries one for every audio bus, so metering a whole mixer costs no rings at all.

**`/bus_tapStream periodMs frames bus...`** is the network counterpart, for clients that cannot map the segment — a browser oscilloscope over WebSocket, or headless capture. The server acks `/done /bus_tapStream`, then sends, immediately and every `periodMs` (10 ms floor), one **`/bus_tapStream.reply bus endPosition blob`** per listed bus: the newest `frames` samples of that bus as raw little-endian `f32`, with `endPosition` (int64) the tap's stream position — total samples ever written — at the window's end, so consecutive snapshots can be placed on the tap's own sample axis (they overlap or gap by exactly the position delta). `frames` is clamped to the subscriber's transport bound — 8192 for a datagram-bounded client (UDP, the shared-memory ring), up to the `--max-frame` ceiling for a TCP/WebSocket client — and always to half the ring; at most 8 buses per subscription; a bus that has not yet filled a window sends nothing. Same subscription posture as `/bus_stream`: one per client, replaced on each call, `periodMs <= 0` (or no taps) cancels, dies with its TCP/WebSocket connection, not schedulable in a timed bundle. The subscription **is** the watch: it starts recording every bus it lists and stops when it is replaced, cancelled or its connection dies, so a streaming client never sends `/bus_tap` itself.

## Timed bundles

OSC bundles carry an NTP timetag. The immediate tag (`1`) executes on arrival; a **future** timetag is converted to a position on the server's sample clock and the whole bundle fires **sample-accurately**: the engine splits the audio block at the event's exact sample, so a `/synth_new` scheduled mid-block starts on that very frame. Bundles with equal times run in arrival order; late bundles run immediately (and are logged). Nested bundles are scheduled independently by their own timetags.

Schedulable inside a timed bundle: `/synth_new`, `/node_set`, `/node_setRange`, `/node_fill`, `/node_map`, `/node_mapAudio`, `/node_mapRange`, `/node_mapAudioRange`, `/node_free`, `/node_before`, `/node_after`, `/node_order`, `/group_head`, `/group_tail`, `/group_new`, `/group_name`, `/group_freeAll`, `/group_deepFree`, `/bus_set`, `/bus_setRange`, `/bus_fill`, `/node_ugenCmd`, `/group_sortMode`, `/group_parallel`, `/graph_new`, `/graph_newVoice`. Anything else (defs, buffers, queries, server commands) replies `/fail … cannot be scheduled in a timed bundle` — load defs and buffers first, then schedule the notes.

**`/sched_clear`** flushes the whole timed-bundle queue: every bundle waiting on the sample clock is dropped (their heap freed off the audio thread), and the command replies `/done /sched_clear`. Use it to abort a scheduled score — the scsynth panic button.

Also beyond scsynth: **auto-sorted groups**. `/group_sortMode groupID 1` makes a group keep its children in dependency order inferred from the buses each def reads and writes — no more manual `/node_before` bookkeeping; query what the server inferred with `/group_queryTree` and `/group_dumpGraph`. See [`auto-order.md`](auto-order.md) and `examples/auto_order.py`. The same analysis powers **parallel groups**: `/group_parallel groupID 1` (with the server started as `--workers N`) runs a group's independent children on several cores, bit-identically to the sequential result — see [`parallel.md`](parallel.md).

Beyond scsynth, the server also exposes its **sample clock** directly: `/clock_query` queries the sample counter, the actual sample rate and the server's OSC time captured with the counter (the master-clock anchor), and `/sched_at <int64 target> <blob>` schedules a packet at an **absolute sample** instead of an NTP time — same queue, same sample accuracy, drift-free by construction. See [`sample-clock.md`](sample-clock.md) and `examples/sample_clock.py`.

It also keeps a **shared transport** — a beat grid for phase-aligning several clients on that sample clock, plus a DAW-style rolling state. `/transport_query` **reads** it (replies `/transport_query.reply <int64 originSample> <double tempo> <int32 defined> <int32 playing> <double position> <int32 group> <int64 transportSample> <int64 positionSample> <int64 loopStart> <int64 loopEnd>`, `defined` 0 until one is set, `group` `-1` when no group is bound, `loopStart`/`loopEnd` both 0 when nothing loops); `/transport_set <int64 originSample> <double tempo>` **sets** the grid (last writer wins, stopped at position 0, replies `/done`); the governed group and the loop survive it, since both are bindings to the tree and to the samples rather than parts of the grid. Beat `b` is sample `originSample + b·rate/tempo`. The **rolling state** rides on top: `/transport_play [<double position>]` starts playing (from `position`, or where it last stopped), `/transport_stop` stops, `/transport_locate <double position>` sets the song position; each replies `/done`. Any change is **pushed** to every `/server_notify` client as a `/transport_query.reply`, so a responder re-aligns or rolls its playhead live.

**The grid is optional, and only the commands that speak beats need it.** The transport exists on every server from boot: rolling, stopping, saying where the piece is and looping a span of it are all in **samples**, and an audio editor has no tempo to declare. So `/transport_play` (bare), `/transport_stop`, `/transport_group`, `/transport_locateSample` and `/transport_loop` work with no grid at all, while the two that name a **beat** — `/transport_locate`, and `/transport_play` *given a position* — `/fail` until `/transport_set` has said what a beat is. `defined` in the reply says whether the grid is there, and `position` (the beat spelling) stays 0 while it is not; `positionSample` is live either way.

**Where the piece is, and the two commands that address it in samples.** `position` is the song position in *beats*, which is what a sequencer speaks; an audio editor addresses **frames**, so the same position is also reported as `positionSample` — the sample of the piece the engine is playing. **`/transport_locateSample <int64 sample>`** is `/transport_locate`'s sibling for that axis: a sequencer locates by beat and an editor by frame, and converting either into the other on the client is how a rounding error gets into a seek. Both spellings are kept in step, so the two fields never disagree. The piece's axis starts at its own 0 — `originSample` anchors the beat grid on the *device* axis, for aligning several clients on one running server, and is not consulted here.

**`/transport_loop [<int64 start> <int64 end>]`** is the span of the piece the position wraps inside, in samples; **with no arguments it turns looping off**. The span is **half-open**: a loop of `0 n` over an `n`-sample take plays every frame exactly once and joins its own start with no repeated frame. Setting one does not move the piece — it keeps playing from where it is and wraps when it first reaches the end — and the wrap happens *in the engine*, on its exact sample, so a client sends nothing when a pass completes and a reader following the transport hears no seam. An empty or inverted span `/fail`s rather than being ignored. There is no `enabled` argument: what a loop toggle remembers is the span it last used, and that is the client's to keep.

**Two quantities, and confusing them is the easy mistake.** `transportSample` is the transport **clock** — samples *elapsed* under the transport, held while it is stopped and **monotonic**, which is what `/sched_atTransport`'s queue needs, since "due" only means anything on an axis that cannot jump. `positionSample` is where the piece **is**: it jumps wherever a locate puts it and wraps at a loop's end. A playhead reads the position; anything scheduling reads the clock. Both are published in the shared-memory segment ([`ipc.md`](ipc.md)), and `positionSample` is answered from the engine as of its **last completed block** — the same rule `/buffer_query` states for the buffer mirror — so a query issued in the same breath as a locate may still answer the previous place.

**The transport has two intensities, and `/transport_group` is the switch.** With **no group bound** it is what the paragraph above describes: an in-memory registry the server stores, serves and broadcasts but never schedules audio from — clients read it to start their routines and playheads on the same beat, and obey it by choice. **`/transport_group <int32 group>`** binds the group the transport *governs* (a negative id unbinds, replying `/done`; an unknown id or a non-group `/fail`s), and from then on the **engine enforces it**: `/transport_stop` freezes that subtree and the server's own transport clock at the exact sample it lands on, and `/transport_play` thaws them. Freeing the governed group unbinds the transport, and unbinding thaws whatever it governed, so no frozen subtree is ever left with nobody to resume it.

A frozen node stays in the tree with its internal state untouched — the same thing `/node_run 0` does — so a resume **continues** the sound rather than restarting it. That is the only thing a pause can mean for samples the server generates itself: a def running a stochastic process or a demand-rate sequence has no samples to index into, so its position *is* its state. Consequently **`/transport_locate` moves the position, never the state of a node**: locating changes `position` and re-anchors the beat→sample mapping, and resident nodes stay exactly where they are. A client that wants a generator to become seekable renders it first.

**A reader can follow the position instead of carrying one.** That is what the `TransportPos` UGen is for: a buffer reader whose phase is the transport's position seeks when the transport seeks, loops when it loops and holds when it stops, with nothing sent per pass and no position of its own to keep in step. It is the shape a multitrack needs — many readers, one time — and it is why locating never has to reach into a node: a follower has no state to move. A reader that carries its own position is the other shape and stays available (`PlayBuf` from its own `startPos`, `BufRd` over any phase you like); the two coexist, and a one-shot fired from a pattern has no business consulting a transport.

Two clocks follow from this, and every sample count on the wire belongs to one of them. The **device** clock never stops (`/clock_query`, the taps, the bus streams, the meters); the **transport** clock, reported as `transportSample`, advances only while the transport rolls and is the time of the *piece*. They are one physical clock — `transportSample` is the device clock minus the total time spent stopped — so they cannot drift apart, and they differ only while a governed transport is stopped. The shared-memory segment publishes both.

What rides which clock is decided by **where a scheduled bundle's messages point**: a bundle any of whose messages targets a node at or under the governed group waits out a pause, and everything else fires on the device clock as always. A bundle is atomic, so a **mixed** bundle — one message inside the governed subtree, one outside — goes entirely to the transport clock, which is the one way a message aimed at a live node can end up waiting. Classification happens **once**, when the bundle is queued: a bundle scheduled *before* `/transport_group` binds stays on the device clock even if its target becomes governed later.

**`/sched_atTransport <int64 sampleTarget> <blob packet>`** is `/sched_at` on the transport axis (see below), for a client naming an absolute sample of the piece. Declaring the axis is not about disambiguation — classification is deterministic, and a client that bound the group knows which of its nodes are governed — it is about **verification**: the server compares the declaration against its own classification and replies `/fail` when they disagree, instead of playing the bundle in the wrong place. It needs a group bound.

With no group bound, `group` reports `-1` and the transport is the advisory registry it has always been: `/transport_play` and `/transport_stop` are broadcast for the clients to obey by choice, and nothing in the tree freezes. See [`sample-clock.md`](sample-clock.md).

UDP is not the only transport: local clients can speak the same OSC through **shared memory** (`clausters --shm <path>`) or run the whole server **in-process** through the embed C ABI, with the sample clock and the control buses readable and writable directly in mapped memory. See [`ipc.md`](ipc.md) and `clients/python/clausters/ipc.py`.

The server also speaks **TCP**, **on by default** on the same port `57110` (alongside UDP — separate namespaces; `--no-tcp` disables it, `--tcp [port]` moves it). **UDP is always on** — it is the base transport and cannot be turned off (there is no TCP-only mode); it is the boot/discovery protocol a client probes first, and it doubles as internal infrastructure, since the TCP loop wakes itself with a zero-length datagram to the server's own UDP socket. Each OSC packet — message or bundle — is **length-prefixed**: a 4-byte big-endian byte count followed by exactly that many OSC bytes, the same framing scsynth uses; replies come back framed the same way over the one connection. TCP gives a reliable, ordered, connection-oriented channel — no silent packet loss, and no datagram size limit: a frame may be as large as the **`--max-frame` ceiling** (default 16 MiB, advertised in `/server_query.reply`), which is a DoS guard on the untrusted length prefix, not a protocol limit. That makes TCP the natural **command plane** for large payloads — whole defs, big `/buffer_getRange` chunks — while timing still rides on bundle timetags / `/sched_at`, so arrival latency does not affect *when* a scheduled command fires. The Python client speaks it through `OscTcpInterface`.

The server also speaks **WebSocket** when started with `clausters --ws [port]` (default port `57120`) — always available, like TCP and shared memory, not behind a build feature. This is the transport a **browser** can reach — a browser cannot open a raw UDP socket or map shared memory, but speaks WebSocket natively — so it is what lets a web-hosted client drive the server (and the server "run in the browser"). It carries the same OSC the same way the other transports do, multiplexed into the same loop and waking it with the same zero-length-UDP trick; the one framing difference from TCP is that each OSC packet is one WebSocket **binary** message, so the frame boundary *is* the packet boundary and there is no length prefix (replies come back as binary messages). Inbound bytes validate through the same single decode door as every transport, and messages are bounded by the same `--max-frame` ceiling TCP applies to its length prefix. The Python client speaks it through `OscWsInterface`; a browser uses the native `WebSocket` API (see `examples/ws_ping.py` and `examples/ws_ping.html`). `ws://` only — TLS (`wss://`) is out of scope, terminate it at a reverse proxy if needed.

Both stream transports share a few **edge guards**, sized like everything else for the target deployments (loopback and controlled networks, not a public service). Concurrent connections, TCP + WebSocket combined, are capped by **`--max-clients`** (default 64, in the spirit of scsynth's `maxLogins`) — a connection past the ceiling is dropped at accept, and closing any connection frees its slot. A client that *floods* commands faster than the server consumes them is held back by TCP flow control (the inbound queues are bounded, so server memory is too) — nothing is dropped, the sender just blocks. A client that *stops reading* its replies is disconnected rather than allowed to stall the server or grow its reply queue without bound. None of this affects a well-behaved client, dense control streams included; UDP is connectionless and keeps its own natural bounds (the datagram cap, kernel buffering).

## NRT mode (offline rendering)

The same engine renders scores to WAV without an audio device:

```sh
clausters --nrt score.osc out.wav [--rate 48000] [--channels 2] \
    [--format float|int16|int24] [--seed N] [--stats]
```

`--format` defaults to `float` — f32 is what the engine computes in and what
buffers hold, so a float WAV loses nothing on the way out or back in.

**`--seed N`** starts the render's stochastic UGens. **Without it every run is a
new take** — a random process is unpredictable first, so rendering a score with
noise in it twice gives two performances, the way playing it twice would. The
seed a render used is always reported (on the summary line, and as `seed` in
`--stats`); pass it back here and that exact take returns, sample for sample.
That is what lets a patch with noise in it have a golden file: the test asks for
the seed it pinned.

The seed sequence belongs to the render, not to the process, so a fixed seed
reproduces in any process, at any worker count, in memory or through a file.

**`--stats`** replaces the human summary with one JSON line:

```json
{"frames":24000,"events":3,"channels":2,"sampleRate":48000,
 "seed":12157665459056928801,"peak":[0.5,0.2],"rms":[0.353,0.115]}
```

`peak` and `rms` are per channel, measured while the render streamed; `seed` is
the one the render actually started from, drawn or given. This mode
exists for a client driving `--nrt` as a subprocess — it learns what the render
did without opening the file it just asked for. The Python client's
`render(path=...)` is exactly that: the server writes the audio, the client
keeps the numbers.

A score is the scsynth binary format: OSC packets back to back, each preceded by its byte count as a big-endian `int32`. Timetags count **seconds from the start of the render** (the immediate tag is time 0); bundles fire sample-accurately exactly like in real time, so an offline render equals a perfectly timed live take. The render ends at the time of the **last** bundle, whose commands produce no sound — close every score with a dummy bundle (a final `/node_free`) to set the duration.

Unlike the live server, a score bundle may also contain the asynchronous commands `/def_send synth`, `/def_send faust`, `/def_free` and the `/buffer_*` family: they complete synchronously before time advances (scsynth NRT semantics), and any failure aborts the render with the offending event's time and message. Queries (`/server_status`, `/buffer_query`, `/buffer_get`, `/buffer_getRange`, `/bus_get`, `/def_query`, `/ugen_query`) are errors in a score.

`python3 examples/json_client.py score` writes an example score; `cargo run --release --example bench` measures graph throughput offline.

## SynthDef JSON (`/def_send synth`)

The blob is a JSON object:

```json
{
  "name": "default",
  "controls": [
    {"name": "freq", "default": 440.0},
    {"name": "amp",  "default": 0.2}
  ],
  "ugens": [
    {"kind": "Sine", "inputs": [{"control": 0}]},
    {"kind": "Mul",    "inputs": [{"ugen": 0}, {"control": 1}]},
    {"kind": "Out",    "inputs": [{"const": 0.0}, {"ugen": 1}]}
  ]
}
```

- `name` — the key used by `/synth_new` and `/def_free`. Re-sending a name replaces the def (existing synths keep playing the old one).
- `controls` (optional) — named parameters addressable from `/synth_new` and `/node_set`, with their default values and an optional **type** and **lag** (see *Control types* below).
- `ugens` — the signal graph in execution order. Each input is one of:

  | input form | meaning |
  |---|---|
  | `{"const": x}` | a constant (finite) float |
  | `{"control": i}` | the i-th entry of `controls` |
  | `{"ugen": i}` | the single output of an **earlier** ugen (`i` < own index) |

  Forward references are rejected, which forces an acyclic, topologically ordered graph. Every ugen has exactly the arity listed below — wrong input counts, unknown kinds, out-of-range references and non-finite constants all come back in `/fail` naming the offending node (e.g. `ugens[2].inputs[0]: control 7 out of range (have 2)`).

  Each ugen may also carry an optional **`"rate"`** — its output calculation rate, one of `"ir"`, `"kr"`, `"ar"`, `"dr"` (see *Calculation rates* below). Omitted, it defaults per kind (`ar` for signal UGens); the compiler rejects a rate a kind does not implement, or an illegal coercion, naming the node.

### Control types

A control entry may carry a **`"rate"`** (its type) and a **`"lag"`** time, both optional:

| field | meaning |
|---|---|
| `"rate": "kr"` | *(default)* a plain control: one value per block, settable any time with `/node_set`, mappable to a bus with `/node_map`/`/node_mapAudio` |
| `"rate": "tr"` | a **trigger**: a `/node_set` value holds for exactly one block, then the engine resets it to `0` — so a rising edge fires once (drives an `EnvGen` gate, a sample-and-hold, …) |
| `"rate": "ir"` | a **scalar**: read once when the synth starts and then frozen; a later `/node_set` is ignored (it pairs with the `ir` UGen rate, so it may feed an `ir` input like `Rand`/`BufFrames.ir`) |
| `"lag": t` | on a `kr` control, smooth its changes with an implicit one-pole `Lag` over `t` seconds — the server inserts a real `Lag` UGen at compile time, so a stepped `/node_set` glides instead of jumping |
| `"lag_down": t` | with `"lag"`, use separate up (`lag`) and down (`lag_down`) times (an inserted `VarLag`) |

Example: `{"name": "freq", "default": 440.0, "lag": 0.1}` glides over 100 ms; `{"name": "gate", "default": 0.0, "rate": "tr"}` is a one-shot trigger. Audio-rate controls are not a control type — feed an audio signal through `In`/an input bus and `/node_mapAudio` instead.

### UGen catalog (built-in kinds)

The `kind` field is an **opaque string** as far as the protocol is concerned: the schema above is the whole contract (name + inputs + optional rate + static fields). What follows is a separate thing — the **catalog of UGens the server currently ships**, which the server resolves at runtime and grows independently of the wire format (each is one descriptor entry in `src/dsp/registry.rs`). So this table is reference, not part of the protocol: an unknown name simply fails with `/fail … unknown kind`.

A client does not *have* to enumerate the catalog — naming a kind and letting the server validate remains the normal path — but it **can**, with [`/ugen_query`](#def-buffer-and-ugen-introspection): the same descriptors as a typed reply, so a palette or completion list derives from the server's own truth (and from the build it is actually talking to) instead of a copy of this table that would drift. Input names below are given in **wire order**, which is what `/ugen_query` reports.

**A def is short only where the kind says it may be.** Inputs are positional and arity is exact, so a UGen that gains an input would break every def ever written against it — including the ones persisted on disk and the ones inside a saved bundle (`PlayBuf` going from four inputs to seven did exactly that). Some kinds therefore declare an **optional tail**: a run of trailing inputs a def may stop before, which the server fills from the catalog's own defaults while compiling. The compiled def is identical to one a complete client sent, so nothing about playing it changes.

Which slots earn it is a property of the *kind*, not a general permission: a slot is optional only when its default is **inert** — the value that makes the UGen behave as if the slot were not there (0 for a trigger, an offset, a phase, a channel, a done action; 1 for a level or a rate scale). A slot whose default is a *choice* (`freq`, `delaytime`, `width`, `max`) stays required, because omitting it would not be leaving it alone; so does any slot the UGen reads its signal, source, position or chain from, since silence and frame 0 are legal values a wrong def would run with. `BinaryOpUGen` is `a=0, b=0` and has **no** optional tail at all — filling a truncated `Mul` would silence the chain with no `/fail` and no name. A def that is short of the tail, or longer than the kind, still fails with `expected … inputs`.

This makes growth **by the tail** non-breaking and does nothing for an input inserted in the middle, which stays breaking. And the declared default of an optional slot is **wire contract**: changing it later changes what every def leaning on the fill sounds like.

| kind | inputs | output |
|---|---|---|
| `Sine` | freq (Hz) | sine by f64 phase accumulation, starts at phase 0 |
| `Impulse` | freq (Hz) | single-sample `1.0` every `freq` Hz, `0.0` between; the first output sample is always an impulse, so a `/sched_at`'d `/synth_new` places it on an exact frame; `freq` 0 emits one impulse then silence (f64 phase, drift-free) |
| `WhiteNoise` | — | uniform white noise in ±1 |
| `Saw` | freq (Hz) | band-limited rising sawtooth in ±1, starting at 0 (no onset step, no DC). Anti-aliased with PolyBLEP — see the note below |
| `Pulse` | freq (Hz), width | band-limited pulse in ±1; `width` is the duty cycle (0.5 = square), clamped just inside (0, 1) where the two edges would coincide |
| `LFSaw` | freq (Hz), iphase | rising sawtooth in ±1, **not** band-limited — a modulation shape. `iphase` is the initial phase in **cycles**, [0, 1), read once at the first sample |
| `LFPulse` | freq (Hz), iphase, width | square in **[0, 1]** (a gate, unlike the bipolar `Pulse`) with `width` as its duty cycle; not band-limited |
| `LFTri` | freq (Hz), iphase | triangle in ±1 starting at 0 and rising; not band-limited |
| `VarSaw` | freq (Hz), iphase, width | triangle whose peak sits at `width` of the cycle — sweeps from a falling ramp through a triangle to a rising one; not band-limited |
| `LPF`, `HPF` | signal, freq (Hz) | second-order Butterworth low/highpass: −3 dB at `freq`, −12 dB/octave |
| `RLPF`, `RHPF` | signal, freq (Hz), rq | the same with the resonance as an input; unity gain at DC / at Nyquist |
| `BPF`, `Resonz` | signal, freq (Hz), rq | bandpass with **unity gain at the centre**, `rq` its bandwidth ratio. Two names, one implementation — see the filter note below |
| `BRF` | signal, freq (Hz), rq | band reject: unity in both passbands, a true null at `freq` |
| `Svf` | signal, freq (Hz), rq, low, band, high | the state-variable filter with its three tap gains as **signal inputs**, so the response is modulable. Lowpass = `1,0,0`; bandpass (unity peak) = `0,rq,0`; highpass = `0,0,1`; notch = `1,0,1`; peak = `-1,0,1`; allpass = `1,-rq,1` |
| `OnePole` | signal, coef | `y[n] = (1−\|c\|)·x[n] + c·y[n−1]` — lowpass for a positive coefficient, highpass for a negative one. The parameter is the **pole**, not a cutoff |
| `OneZero` | signal, coef | `y[n] = (1−\|c\|)·x[n] + c·x[n−1]` |
| `LeakDC` | signal, coef | DC blocker: a zero exactly at 0 Hz with a pole just inside it |
| `Integrator` | signal, coef | leaky accumulator, `y[n] = x[n] + c·y[n−1]`; `coef` is clamped just inside 1 so it always forgets eventually |
| `DelayN`, `DelayL`, `DelayC` | signal, delaytime (s) | pure delay: no interpolation (rounded to whole samples), linear, or four-point cubic. Plus the static `max_delay` field |
| `CombN`, `CombL`, `CombC` | signal, delaytime (s), decaytime (s) | feedback comb. `decaytime` is the time for the echo train to fall 60 dB **counting from the first echo**, which is the direct path and returns at full level; negative inverts alternate echoes, zero leaves a single one |
| `AllpassN`, `AllpassL`, `AllpassC` | signal, delaytime (s), decaytime (s) | Schroeder allpass: exactly flat magnitude, phase only. Decay as for the comb |
| `BufDelayN/L/C`, `BufCombN/L/C`, `BufAllpassN/L/C` | bufnum, chan, signal, delaytime (s) [, decaytime (s)] | the same nine, over **a channel of a pool buffer** instead of private memory: same interpolation, same feedback, same arithmetic, and the line's contents are **addressable** — inspectable, resamplable, savable, playable by another node while it runs. No `max_delay` field: the buffer's own frame count is the bound. It is **not zeroed** (allocating and clearing are the client's, as in scsynth), a missing buffer or a channel it does not have plays silence, and `bufnum`/`chan` being inputs means a line can be moved between buffers with a `/node_set` |
| `Phasor` | trig, rate, start, end, reset_pos | ramp from `start` to `end` advancing by `rate` **per sample** (not Hz), wrapping at `end`; a rising `trig` jumps to `reset_pos`. The index source for a buffer reader: a rate of 1 advances one frame per sample |
| `TransportPos` | offset | the **transport's position in the piece**, in frames, minus `offset` — what a buffer reader follows so that seeking, looping and pausing belong to the transport and not to each reader (`/transport_locate`, `/transport_loop`). Ramps one frame per sample while the transport rolls and holds while it is stopped. `offset` is where these samples start in the piece, so a clip reads its own frame 0 when the transport reaches it — and the subtraction happens in `f64` inside the UGen, which is what keeps a long piece's position exact in an `f32` signal |
| `BinaryOpUGen` | a, b | one of a table of binary operators, chosen by the `op` **name** — see the operator note below |
| `UnaryOpUGen` | a | one of a table of unary operators, chosen by the `op` **name** — see the operator note below |
| `Add`, `Sub`, `Mul`, `Div` | a, b | sample-wise arithmetic; thin aliases for the `add`/`sub`/`mul`/`div` operators (kept so existing defs are unchanged) |
| `MulAdd` | a, b, c | `a·b + c` in one UGen (the multiply-accumulate the server fuses) |
| `Sum3` | a, b, c | `a + b + c` |
| `Sum4` | a, b, c, d | `a + b + c + d` |
| `In` | bus | copies an audio bus (read once per block) |
| `InCtl` | bus | a control-bus value, constant over the block |
| `OutCtl` | bus, signal | writes the signal's latest per-block value to a **control** bus (the write side of `InCtl`); passes the signal through as its output |
| `Out` | bus, signal | **sums** the signal into an audio bus |
| `ReplaceOut` | bus, signal | overwrites the bus instead of summing |
| `PlayBuf` | bufnum, chan, rate, loop, trigger, startPos, doneAction | buffer player with linear interpolation; `rate` is frames per output sample (1.0 = the server rate — multiply by `BufRateScale(bufnum)`, i.e. `file_sr / server_sr`, for the file's pitch). A pass begins at `startPos` and a rising `trigger` re-cues there mid-play, so one player is a re-usable voice; without `loop`, reaching the end stops it and fires `doneAction` (`2` frees the synth, so a one-shot leaves the tree by itself), and a looping player never finishes, so its action never fires. The last three come **after** `loop` rather than in scsynth's order (`rate, trigger, startPos, loop`): inputs are positional, so putting `startPos` before `loop` would silently re-read every existing `loop` argument as a cue frame |
| `BufRd` | bufnum, chan, phase, loop | reads the buffer at a `phase` signal in frames (linear interpolation); out-of-range phases wrap when looping, clamp otherwise |
| `BufWr` | bufnum, chan, phase, loop, signal | **writes** `signal` into the buffer at `phase` (frames, truncated — no interpolation, which on write would store a value the signal never had); wraps when looping, writes nothing out of range otherwise. Passes `signal` through as its output, like `OutCtl` |
| `RecordBuf` | bufnum, chan, signal, offset, recLevel, preLevel, run, loop, trigger, doneAction | **records** `signal`, advancing one frame per sample — the self-advancing writer, as `PlayBuf` is the self-advancing reader. Each frame becomes `signal·recLevel + old·preLevel`, so `(1, 0)` overwrites, `(1, 1)` overdubs and `(1, 0.5)` overdubs with the older layers fading: that pair is what makes it a looper rather than a tape head. `run` at 0 holds the position and writes nothing; a rising `trigger` re-cues to `offset`; without `loop`, reaching the end stops it and fires `doneAction`. Passes `signal` through |
| `BufSampleRate` | bufnum | the buffer's own sample rate (Hz), block-constant |
| `BufRateScale` | bufnum | `file_sr / server_sr`; feed `PlayBuf`'s `rate` (`rate: BufRateScale(buf) * pitch`) to play at the file's true pitch without the client knowing either rate |
| `BufFrames` | bufnum | frame count, block-constant |
| `BufChannels` | bufnum | channel count, block-constant |
| `BufDur` | bufnum | duration in seconds (`frames / file_sr`), block-constant |
| `Osc` | bufnum, freq (Hz), phase (rad) | interpolating wavetable oscillator; `bufnum` must hold a **wavetable-format** buffer (see `/buffer_gen` below); `phase` is an offset in radians |
| `OscN` | bufnum, freq (Hz), phase (rad) | non-interpolating oscillator over a **plain** (non-wavetable) buffer; rawer/cheaper than `Osc` |
| `VOsc` | bufpos, freq (Hz), phase (rad) | like `Osc` but the buffer number is a signal: reads wavetables `bufpos` and `bufpos+1` and crossfades by its fractional part, so sweeping `bufpos` morphs a bank of adjacent tables (allocate them contiguously, same size) |
| `Shaper` | bufnum, signal | waveshaper: maps `signal` (in ±1, clamped) through a transfer table in wavetable format (typically `/buffer_gen cheby`); the table's first point is `signal = −1`, its last `signal = +1` |
| `DiskIn` | chan | streams a file from disk (mono per UGen — `chan` picks the channel); needs a `path` field; `loop` restarts at end of stream; see streaming note below |
| `DiskOut` | signal | streams `signal` to a mono WAV on disk; needs a `path` field; `format` is the WAV sample format; passes `signal` through as its output |
| `LocalIn` | channel | reads synth-private feedback channel `channel` (a constant); see feedback note below |
| `LocalOut` | channel, signal | writes `signal` into synth-private feedback channel `channel` (a constant); also passes `signal` through as its own output |
| `EnvGen` | gate, level_scale, level_bias, time_scale, done_action, *envelope array* | breakpoint envelope; gate-driven, with a `done_action` that can free the node — see the envelope note below |
| `Line` | start, end, dur, done_action | ramps from `start` to `end` over `dur` seconds, then holds; the same `done_action` set as `EnvGen`, which it is built on |
| `XLine` | start, end, dur, done_action | `Line` in equal ratios instead of equal steps; `start`/`end` must be non-zero and share a sign |
| `FreeSelf` | signal | passes `signal` through; frees the enclosing synth while it is greater than zero |
| `PauseSelf` | signal | passes `signal` through; pauses the enclosing synth while it is greater than zero (resume with `/node_run 1`) |
| `Done` | source | 1 once the ugen wired into `source` has **finished**, 0 before — see the done-flag note below |
| `FreeSelfWhenDone` | source | passes `source` through and frees the synth once it has finished |
| `PinkNoise` | — | equal energy per octave, −3 dB/octave (Voss–McCartney); a **quiet** signal, ~0.13 RMS |
| `BrownNoise` | — | a random walk, −6 dB/octave; reflects at ±1 rather than clamping |
| `GrayNoise` | — | one random bit of a 31-bit word flipped per sample; leans low (~−2.9 dB/octave) and steps by wildly unequal amounts |
| `ClipNoise` | — | −1 or 1 only, a coin flip per sample |
| `LFNoise0` | freq | a new random value in ±1 every `1/freq` seconds, held (steps) |
| `LFNoise1` | freq | the same, ramped linearly between values |
| `LFNoise2` | freq | the same, quadratic between values — no corners, and it **overshoots** to about ±1.7 |
| `LFClipNoise` | freq | `LFNoise0` restricted to ±1 |
| `Dust` | density | random impulses in [0, 1) at a **mean** `density` per second — exponential gaps, not a clock |
| `Dust2` | density | the same, firing both ways in ±1 |
| `Crackle` | chaos | the chaotic map `y[n] = |chaos·y[n-1] − y[n-2] − 0.05|`; deterministic, one-sided (carries DC) |
| `Pan2` | signal, pos, level, chan | places a mono `signal` between two channels at `pos` (−1 left, 1 right) at **equal power**; one row per output channel — see the panning note below |
| `LinPan2` | signal, pos, level, chan | the same with the **constant-amplitude** law: the two gains sum to `level`, 0.5 each at the centre |
| `Balance2` | left, right, pos, level, chan | shifts an already stereo pair towards one side by attenuating the other, at equal power; a **centred** `Balance2` is 3 dB down, not a pass-through |
| `Rotate2` | x, y, pos, chan | rotates the plane the two signals span by `pos` **half turns** (0.25 is 45°); equal power at every angle, so it turns a stereo image without resizing it |
| `MidSide` | a, b, chan | the mid/side matrix, normalized to `1/√2` so it is **its own inverse** — the same kind encodes and decodes |
| `StereoWidth` | left, right, width, chan | scales the side component: `0` mono, `1` exactly the identity, `2` widened, negative swaps the sides |
| `PanAz` | signal, pos, level, width, orientation, numchans, chan | places a mono `signal` on a **ring** of `numchans` channels; `pos` spans the whole ring over −1..1, `width` is the lobe in channels (2 = equal power between neighbours), `orientation` turns the ring (0.5 puts the origin between two channels) |
| `XFade2` | a, b, pan, level | equal-power crossfade: −1 all `a`, 1 all `b`; keeps *uncorrelated* samples at one loudness and lifts correlated samples 3 dB in the middle |
| `LinXFade2` | a, b, pan, level | the same with the constant-amplitude law — a plain interpolation, the right one for correlated sources |
| `Select` | which, source0, source1, … | outputs the source at the truncated `which`, clamped to the ends; every source still runs |
| `SelectX` | which, source0, source1, … | the same with the index's fraction crossfading to the next source at equal power |
| `Trig` | signal, dur | holds the **level the input had at the trigger** for `dur` seconds, then 0 |
| `Trig1` | signal, dur | holds 1 for `dur` seconds after each trigger |
| `TDelay` | signal, dur | one sample of 1, `dur` seconds after each trigger; a trigger arriving while one is in flight is dropped |
| `Latch` | signal, trig | sample and hold: one sample of `signal` per rising edge |
| `Gate` | signal, trig | passes `signal` while `trig > 0`, freezes at the last value when it is not |
| `Schmidt` | signal, lo, hi | comparator with hysteresis: 1 past `hi`, 0 past `lo`, unchanged between |
| `ToggleFF` | trig | flips between 0 and 1 on each trigger |
| `SetResetFF` | trig, reset | 1 from `trig`, 0 from `reset`; both at once leaves it 0 |
| `PulseCount` | trig, reset | counts triggers from 1; a rising `reset` zeroes it |
| `PulseDivider` | trig, div, start | one trigger out per `div` in; `start` (read once) phases the counter — `div - 1` fires on the first |
| `Stepper` | trig, reset, min, max, step, resetval | walks `[min, max]` (**both ends included**) one `step` per trigger, wrapping; sits at `resetval` until the first |
| `Timer` | trig | seconds between the last two triggers, held; the crossing is interpolated, not rounded to the sample |
| `Sweep` | trig, rate | a ramp rising at `rate` per second, restarted at each trigger; running before the first one |
| `Changed` | signal, threshold | 1 where the input moved by more than `threshold` — see the note on its halved difference below |
| `Decay` | signal, decaytime | each impulse becomes an exponential falling 60 dB in `decaytime`; instantaneous attack |
| `Decay2` | signal, attacktime, decaytime | `Decay` minus a faster decay, which rounds the attack |
| `DetectSilence` | signal, amp, time, done_action | 1 once the input has stayed within `±amp` for `time` seconds, with the done action; raises a done flag |
| `Lag` | signal, time | one-pole smoother: `signal` lagged over `time` seconds (symmetric); `time` 0 passes through; primed to the first input (no glide up from 0) |
| `VarLag` | signal, up, down | one-pole smoother with separate rise (`up`) and fall (`down`) times |
| `SampleRate` | — | the engine sample rate in Hz; init-rate (`ir`) by default |
| `Rand` | lo, hi | one uniform random value in `[lo, hi)`, drawn once at synth init and held; init-rate only (its inputs must be constants/`ir`) |
| `Demand` | trig, reset, source | demand driver: pulls the next value from `source` on each rising edge of `trig`, holds it between triggers; a rising `reset` restarts the stream; 0 before the first trigger, the last value once the stream ends — see the demand note below |
| `Duty` | dur, reset, level, done_action | demand driver with a clock of its own: pulls one `level` every `dur` seconds and holds it. Both `dur` and `level` are pulled, so a stream of durations against a stream of values is a sequencer; when either ends, `done_action` fires |
| `TDuty` | dur, reset, level, done_action, gap_first | `Duty` emitting each level on its own sample and nothing in between — a trigger stream whose amplitudes are the levels. `gap_first` spends the first duration before the first level, so it opens with a gap |
| `Dseq` | repeats, v0, v1, … | demand source: yields the value list in order, `repeats` passes (`≤ 0` endlessly). A value that is itself a stream is **drained**, not taken once, and restarted when the sequence comes round to it |
| `Drand` | repeats, v0, v1, … | `repeats` **items** drawn at random from the list, each pick independent |
| `Dxrand` | repeats, v0, v1, … | `Drand` that never picks the value it just used |
| `Dshuf` | repeats, v0, v1, … | the list shuffled **once** and replayed in that order, `repeats` passes; the shuffle is redrawn on a reset, not per pass |
| `Dseries` | repeats, start, step | arithmetic sequence `start`, `start + step`, …; `step` is read per item, so it may be a stream |
| `Dgeom` | repeats, start, grow | geometric sequence `start`, `start * grow`, … |
| `Dwhite` | repeats, lo, hi | independent uniform draws on `[lo, hi]` |
| `Diwhite` | repeats, lo, hi | `Dwhite` over the integers in `[lo, hi]`, both ends included |
| `Dbrown` | repeats, lo, hi, step | random walk of at most `step` per item, **folded** into `[lo, hi]` (it turns around at a bound rather than piling up against it) |
| `Dibrown` | repeats, lo, hi, step | `Dbrown` over the integers |
| `Dstutter` | repeats, value | repeats each item of the `value` stream `repeats` times; the count is pulled per item, so it may vary |
| `Dswitch1` | which, v0, v1, … | takes **one** item from the stream `which` picks, then picks again; an unselected stream is not advanced and the selected one is not drained. The index wraps |
| `Dbufrd` | bufnum, phase, loop, channel | reads the buffer frame the `phase` stream names — a `Dseries` phase walks a buffer as a step sequence; out of range it wraps when `loop` is set and clamps when it is not |
| `SendTrig` | trig, id, value | on each trigger of `trig`, sends `/node_trigger nodeID id value` to `/server_notify` clients; output is silence — see the side-effect note below |
| `SendReply` | trig, reply_id, value0, value1, … | on each trigger of `trig`, sends a custom OSC message `cmdName nodeID reply_id value…` (the `cmdName` is a static `label` field, default `/reply`); output is silence |
| `Poll` | trig, signal, trig_id | on each trigger of `trig`, posts `label: value` (the `signal` value) to the server console (a static `label` field) and, when `trig_id ≥ 0`, also sends `/node_trigger nodeID trig_id value`; passes `signal` through as its output |
| `FFT` | source, active | opens a spectral chain: windows `source` and transforms it to a spectral frame once per hop (`active > 0` runs, `≤ 0` holds); static fields `fft_size` (default 1024), `hop` (fraction, default 0.5), `wintype` (default 0 = Hann); the window is also settable live via `/node_ugenCmd` — see the FFT-chain note below |
| `PV_MagAbove` | chain, threshold | passes only bins whose magnitude is **above** `threshold`, zeroing the rest; `chain` is the wire from an earlier `FFT`/`PV_*` |
| `PV_MagBelow` | chain, threshold | passes only bins whose magnitude is **below** `threshold` |
| `PV_BrickWall` | chain, wipe | brick-wall band limit: `wipe > 0` zeroes the top fraction of bins (low pass), `wipe < 0` the bottom (high pass), `0` passes everything (`wipe` in −1..1) |
| `PV_MagClip` | chain, threshold | limits each bin's magnitude **to** `threshold` (louder bins are scaled down, phases kept) |
| `PV_Add` | chainA, chainB | two-chain combiner: per-bin complex sum, result in chain A — see the combiner note below |
| `PV_Mul` | chainA, chainB | two-chain combiner: per-bin complex product |
| `PV_Min` | chainA, chainB | two-chain combiner: per bin, whichever input has the **smaller** magnitude |
| `PV_Max` | chainA, chainB | two-chain combiner: per bin, whichever input has the **larger** magnitude |
| `PV_MagMul` | chainA, chainB | two-chain combiner: A's bins scaled by B's magnitudes (A's phases kept) |
| `PV_CopyPhase` | chainA, chainB | two-chain combiner: A's magnitudes with B's phases |
| `PV_MagFreeze` | chain, freeze | while `freeze ≤ 0` stores each frame's magnitudes and passes through; while `> 0` rescales every bin to the stored magnitude (phases keep running) |
| `PV_MagSmear` | chain, bins | averages each bin's magnitude over `bins` neighbors on each side (`0` is transparent), phases untouched |
| `PV_BinShift` | chain, stretch, shift | remaps bin `b` to `round(b·stretch + shift)`: colliding bins sum, out-of-range bins are dropped |
| `PV_MagShift` | chain, stretch, shift | the same remap applied to the magnitude envelope only, over the frame's original phases |
| `PV_Kernel` | chain, p0, p1, … | applies user-written **bin expressions** to every bin of each fresh frame: static fields `mag_expr`/`phase_expr` are postfix token lists mapping one bin's values to its new magnitude / phase; the variadic `p0…` inputs are parameters the expressions read, sampled at the hop — see the bin-expression note below |
| `IFFT` | chain | closes a spectral chain: inverse-transforms each fresh frame and overlap-adds it back to audio; `fft_size`/`wintype` are inherited from the chain's `FFT` (given only on the `FFT`) |
| `Conv` | source, kernel | partitioned convolution against a **prepared** kernel buffer (`/buffer_gen prepare_partconv`); static fields `fft_size` (transform size; the partition — and intrinsic latency — is `fft_size/2`) and `partitions` (the longest kernel accepted, default 16); see the convolution note below |

**Envelopes (`EnvGen`).** `EnvGen` plays a breakpoint envelope, modelled on SuperCollider's. Its inputs are five fixed signals followed by a flat **envelope array**: `gate, level_scale, level_bias, time_scale, done_action`, then `initLevel, numSegments, releaseNode, loopNode`, then four values **per segment** — `target, duration, shape, curve`. The output is `envelope · level_scale + level_bias`; `time_scale` stretches every segment's duration. A rising `gate` (re)triggers from `initLevel`; while the gate is held the envelope **sustains** at `releaseNode` (an index into the levels — hold that level until release); when the gate falls it plays the segments from `releaseNode` on. A gate found **already closed on the node's very first sample** also counts as a release: a live note-on and its note-off can both apply before the node's first block, and the envelope then plays the release from `initLevel` and finishes (so the `doneAction` still frees the node) instead of sustaining forever on a closed gate. `releaseNode < 0` disables the sustain, so the envelope plays straight through (a one-shot). With a `loopNode` (an index `< releaseNode`), the held phase **cycles** the segments in `[loopNode, releaseNode)` instead of holding a single level, carrying the level at the release node back as the loop's start; the release still plays out from `releaseNode` when the gate falls. `loopNode < 0` disables looping. The **shape** numbers are `0` step, `1` linear, `2` exponential (needs same-sign, non-zero levels), `3` sine, `4` welch, `5` custom-curvature (bent by the `curve` value: 0 linear, positive starts slow, negative starts fast), `6` squared, `7` cubed, `8` hold. When the last segment finishes the UGen applies its `done_action` — scsynth's full set (0–15), with the freeing done on the audio thread through the garbage FIFO, never a blocking free:

| # | action | # | action |
|---|---|---|---|
| 0 | do nothing | 8 | free this synth and every following node in its group |
| 1 | **pause** this synth (kept in the tree; resume with `/node_run`) | 9 | free this synth and pause the preceding node |
| 2 | **free** this synth | 10 | free this synth and pause the following node |
| 3 | free this synth and the preceding node | 11 | free this synth; if the preceding node is a group, deep-free it, else free it |
| 4 | free this synth and the following node | 12 | free this synth; if the following node is a group, deep-free it |
| 5 | free this synth; if the preceding node is a group, free all its children, else free it | 13 | free this synth and every other node in its group |
| 6 | free this synth; if the following node is a group, free all its children | 14 | free the enclosing **group** (this synth included; if it is the un-freeable root, frees just the synth) |
| 7 | free this synth and every preceding node in its group | 15 | free this synth and **resume** the following node |

The relative actions (3–13, 15) resolve the node's previous/next sibling and head/tail-of-group from the tree's execution order. Any input can be a signal, so a control can drive the gate or scale the levels/times live. The Python client builds the array with the `Env` breakpoint helper (`Env.adsr`, `Env.perc`, `Env.asr` …, plus `release_node`/`loop_node`) and the `env_gen` callable.

**One-segment ramps (`Line`/`XLine`).** A ramp from `start` to `end` over `dur` seconds, then held. They take the whole `doneAction` set above — `Line(1, 0, 2, 2)` is a two-second fade that frees its synth — and land exactly on `end` rather than near it. `XLine` moves in equal *ratios*, which is the shape that reads as straight when it drives a frequency or a gain; a zero endpoint is undefined there, and is nudged to a tiny same-signed value rather than producing a `NaN`. Both run at `kr` as well as `ar`, and a sweep usually belongs at `kr`.

`start`, `end` and `dur` are **read once, when the ramp starts**. They set the ramp's geometry, which is then fixed: feeding them a control or a signal that moves later changes nothing, so a sweep that has to be re-aimed while it runs wants a fresh synth or an `EnvGen`. `done_action` is the exception — it is read every block, because it says what happens to the node rather than to the ramp.

**The done flag (`Done`/`FreeSelfWhenDone`).** A ugen that plays out — an envelope, a ramp — raises a **done flag** when it finishes. That is not the same as its `doneAction` (a ugen with action 0 still raises the flag), and it is not visible on its wire either: an envelope that has ended sits at its final level, which is just a number. These two read the flag of the ugen wired into their `source`, so `source` must name a **ugen**, and one that can finish; the compiler rejects a constant, a control, or a kind that never finishes, naming it. `Done` reports the flag as a 0/1 signal for the rest of the graph to trigger on; `FreeSelfWhenDone` passes the source through and frees the synth — the idiom for an envelope whose own `doneAction` is 0 because something else in the graph still needs it. The flag has **block resolution**: it is one bit of state, read once when the watcher runs, so a watcher reports it for the whole block in which it was raised even at `ar`. `DetectSilence` raises one too, so it can be watched the same way.

**Noise.** Every stochastic source draws from the same seeded xorshift the sequencing layer uses, and each instance in a graph gets its own seed — two `WhiteNoise` ugens in one def are two streams, not one summed with itself (which would be a comb filter, not more noise). A given seed replays exactly, which is what lets a noisy patch have a golden file. Three of these are worth reading twice before use: `PinkNoise` is **quiet** by construction (~0.13 RMS against white's 0.58 — the level a def ported from sclang expects); `Dust` is **not a clock** (each sample is an independent trial, so its `density` is a mean and its gaps are exponential — use `Impulse` for even spacing); and `Crackle` has **no RNG at all** — it is a deterministic chaotic map whose parameter changes the sound drastically and not monotonically, and whose output is one-sided and therefore carries DC.

**Panning and the stereo field.** A ugen has **one output** — an input reference names a ugen, not an output of one, the same deviation the buffer readers already carry. So a two-channel panner is **two rows** sharing their inputs and differing only in a trailing `chan` index (0 left, 1 right), and `PanAz` is `numchans` of them, each told the ring size and its own place on it. A client builds the set and hands back a channel list: `pan2(sig, pos)` in the Python client returns two channels, exactly as `out(0, …)` already accepts.

Two laws run through the family. **Equal power** (`Pan2`, `Balance2`, `XFade2`, `SelectX`, and `PanAz`'s lobe) holds `l² + r² = 1`, which keeps one loudness while a source crosses the field — at the price of 0.707 in each channel at the centre, and of a 3 dB lift when the two sides are the *same* signal. **Constant amplitude** (`LinPan2`, `LinXFade2`) holds `l + r = 1` instead: the right choice when the two sides are correlated or when a mono fold-down has to stay put. `Balance2` is the row that surprises people: it applies the *pan* law to a pair that is already stereo, so passing a signal through a centred `Balance2` costs 3 dB. That is scsynth's behaviour and is kept.

The law itself is **computed, not looked up**: a fifth-term polynomial in the position, worst-case error 2.6·10⁻⁷ against scsynth's rounded 2049-entry sine table's 3.8·10⁻⁴, and exact at both ends — a hard pan is digital silence on the far side, not −110 dB of it. It is evaluated **per sample** whenever the position is audio rate, rather than interpolated across the block like a filter coefficient: a position that sweeps a whole block would otherwise read 0.5 where the law wants 0.707, a 3 dB hole in the middle of the block.

`Rotate2`, `MidSide` and `StereoWidth` are the same two-by-two matrix under three parameterizations, and only the first is scsynth's name. A **rotation** moves the image without resizing it; **width** scales the side axis, resizing it without moving it, and no angle produces that. `Rotate2` at a quarter turn *is* the change of basis between left/right and mid/side, which is what `MidSide` names outright — normalized so one kind both encodes and decodes, and so the round trip is exact. Reach for `StereoWidth` for a width knob; reach for `MidSide` when something has to happen *between* the encode and the decode (the centre of a mix filtered apart from its sides), which is the case a single knob cannot express.

`Select`/`SelectX` choose what is **heard**, never what is computed: every source is a ugen in the graph and runs whether or not it is selected. An index off either end holds the source at that end rather than wrapping — including `SelectX`, where sclang's pseudo-ugen instead folds the crossfade and returns a mix of the first two sources for a negative index, or the last one at 1.414 past the end.

**Triggers.** A **trigger** is a signal crossing from `<= 0` up to `> 0` — one definition, the same for every ugen that takes one (`Demand`, `SendTrig`, `EnvGen`'s gate, and the whole family above). Two things about that family are worth knowing before you pick a threshold or a rate. `Changed` compares the **halved** difference, `|(x[n] - x[n-1]) / 2|`, because sclang builds it out of `HPZ1` whose gain is 0.5 and a ported def must not change value: a step of 0.2 registers against a threshold of 0.09, not of 0.19. And they all default to **`ar`**, counters included, even though a counter can only move when a trigger does: a `kr` ugen reads **one sample per block** from an `ar` input, so a `kr` counter fed an `ar` trigger train sees one trigger in 64 and silently drops the rest. `kr` is the right choice when the trigger is `kr` too — the saving is real then, and a duration means seconds at either rate.

**Freeing on a trigger (`FreeSelf`/`PauseSelf`).** The counterpart for when what ends a note is not an envelope: both pass their input through and act while it is greater than zero. Neither latches — the action is reported for the block just processed — which is what makes `PauseSelf` a gate rather than a one-way door: `/node_run 1` really resumes the node, and it re-pauses only if its input is still up.

**Operator UGens (`BinaryOpUGen`/`UnaryOpUGen`).** Rather than a distinct kind per math operation, one generic UGen per arity carries the operator as a static **`op`** field (alongside `kind`/`inputs`) — the operator's **name**. So `{"kind": "BinaryOpUGen", "op": "mul", "inputs": [a, b]}` multiplies. Every operator is one entry in `clausters_core::builtins` — the **same code** the client's value FFI runs off the audio thread — so a value a client computes ahead of time and the UGen on the audio thread are **bit-identical** for the native ops. A missing or unknown `op` fails the def with `/fail` naming the node. The `Add`/`Sub`/`Mul`/`Div` kinds remain as aliases for the `add`/`sub`/`mul`/`div` operators. (Internally each operator also has a stable integer id for the C ABI, but that is an implementation detail — defs and clients only ever use the name.)

Binary operators (`BinaryOpUGen` `op`): `add` (`a+b`), `sub` (`a−b`), `mul` (`a·b`), `div` (`a/b`), `mod` (`a%b`), `pow` (`aᵇ`), `min`, `max`, `atan2`, `gt` (`a>b`), `lt` (`a<b`), `ge` (`a>=b`), `le` (`a<=b`), `eq`, `ne`, `bitand`, `bitor`, `bitxor`, `lshift`, `rshift`, `hypot` (`√(a²+b²)`), `ring1` (`ab+a`), `ring2` (`ab+a+b`), `ring3` (`a²b`), `ring4` (`a²b−ab²`), `sumsqr` (`a²+b²`), `difsqr` (`a²−b²`), `sqrsum` (`(a+b)²`), `sqrdif` (`(a−b)²`), `absdif` (`|a−b|`), `thresh` (`a<b?0:a`), `clip2` (clip to ±b), `excess` (`a−clip2`), `round` (to step b), `trunc` (to step b), `fold2` (fold into ±b, reflecting), `wrap2` (wrap into ±b, modulo), `gcd`, `lcm` (both on the integer truncations of `a` and `b`; the result is negative only when both operands are), `hypot_apx` (`|a|+|b|−(√2−1)·min(|a|,|b|)` — the cheap approximation, never below the true hypotenuse and up to 15.9 % above it). Comparisons and bitwise ops follow Faust's convention (a boolean is `0.0`/`1.0`; bitwise acts on the `i32` casts).

Unary operators (`UnaryOpUGen` `op`): `neg`, `abs`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `exp`, `exp10`, `log`, `log10`, `log2`, `sqrt`, `floor`, `ceil`, `rint` (ties to even), `as_int`, `as_float`, `squared` (`x²`), `cubed` (`x³`), `recip` (`1/x`), `frac`, `sign`, `sinh`, `cosh`, `tanh`, `midicps`, `cpsmidi`, `midiratio`, `ratiomidi`, `dbamp`, `ampdb`, `octcps`, `cpsoct`, `distort` (`x/(1+|x|)`), `softclip`.

The tables are open — a new operator is one more `clausters_core::builtins` entry, no renumbering. The Python client maps its operators and math methods (`%`, `min`, `>`, `.midicps()`, `.distort()`, …) to these names automatically, and exposes `madd`/`sum3`/`sum4` for the fused kinds.

**Calculation rates (`ir`/`kr`/`ar`/`dr`).** Every ugen output has a rate, chosen with the optional `"rate"` field or defaulted per kind. `ar` (audio rate) is one value per sample — the normal signal wire, and the default. `kr` (control rate) is one value per block, recomputed each block; downstream it reads as a constant over the block. `ir` (initial rate) is computed **once, when the synth starts**, then held for the node's life — a fixed `SampleRate`, a `Rand` seed, a `BufFrames.ir` snapshot; its inputs must themselves be constant/`ir` (nothing that varies can be frozen). `dr` (demand rate) is pulled, not run — see the demand note. Coercion is one-way: a slower rate feeds a faster input for free (a constant broadcast over a block), but an `ar` signal cannot feed an `ir` input, and a `dr` output may only feed something that *pulls* it — a demand driver, or another demand ugen nesting it. Audio-rate controls are not a thing here — feed an audio signal through `In`/an input bus (as noted under `/node_mapAudio`).

A slower consumer **samples** a faster input, it does not summarize it: a `kr` ugen reading an `ar` wire takes the block's first sample and ignores the other 63. That is invisible for a smooth signal and destructive for a trigger train, so a counter and its clock belong at the same rate. Otherwise, choosing `kr` changes a ugen's **cost**, not its meaning: a time still counts in seconds and a frequency in hertz, because each ugen runs at its own sample rate (one `kr` sample lasts a whole block). `Impulse.kr(10)` and `Impulse.ar(10)` both fire ten times a second; `LPF.kr(sig, 300)` cuts off at 300 Hz. What you give up is resolution — a `kr` ugen cannot represent anything above half the control rate (375 Hz at the default 48 kHz / 64), which is why oscillators and filters in the audible band belong at `ar` and envelopes, sweeps and modulation are comfortable at `kr`. The one exception is `SampleRate`, which reports the engine's rate at any rate, because that is a fact about the hardware rather than about the ugen.

**Demand-rate streams (the `D*` family).** A demand ugen produces values only when *pulled*: it has no samples, only a next value, and between two pulls it does nothing at all. It never runs in the normal per-block order. What turns a stream into signal is a **driver**. `Demand` is told when to pull — one value per rising edge of its `trig`, held until the next, a rising `reset` restarting the stream. `Duty` and `TDuty` carry their own clock instead: every `dur` seconds they pull one `level`, so a stream of durations against a stream of values *is* a sequencer, the two free to be different lengths. `Duty` holds each level until the next is due; `TDuty` emits it on that one sample and is silent in between — a trigger train whose amplitudes are the levels. Where a driver's own inputs are ordinary signals (`trig`, `reset`, `done_action`) they are read at the sample the pull happens on, so a modulated duration lands where it was modulated.

The sources are the rest of the table above: the ramps (`Dseries`, `Dgeom`), the stochastic ones (`Dwhite`/`Diwhite` drawing independently, `Dbrown`/`Dibrown` walking), the list sources (`Dseq`, `Drand`, `Dxrand`, `Dshuf`), and the three one-offs (`Dstutter`, `Dswitch1`, `Dbufrd`). Every one of them takes **`repeats`** first, and **`repeats ≤ 0` is the endless stream** — sclang writes `inf` there, which a def cannot carry (the wire rejects a non-finite constant, and JSON has no spelling for one), so the count of none is the endless one. For a list source and for `Dshuf` the count is of **passes over the list**; for `Drand`/`Dxrand` it is of **items**, since a random pick has no pass to complete.

**Streams nest, and that is the point of the family.** Any input of a source may itself be a `dr` wire — `Dseq(1, Dwhite(2, 0, 1), 100)` yields two random numbers and then 100 — which is what makes a sequence of *phrases* expressible rather than a sequence of numbers. A list source **drains** a nested slot (reading it until it answers end-of-stream) before moving on, and **restarts** the slot it comes round to again, so `Dseq(2, Dseries(3, 0, 1))` gives `0 1 2 0 1 2`. Whether a reset travels inward is per kind, not a blanket rule: the list sources restart the slot they move to, `Dstutter` and `Dswitch1` restart their inputs outright, and the scalar sources propagate nothing, since they re-read their bounds on every pull anyway. Nesting is capped at **16 levels** — a pull descends one stack frame per level on the audio thread, so a def past the limit is refused at compile time rather than guarded at run time.

End-of-stream reaches the drivers differently. `Demand` holds the last value it got (and outputs 0 before the first trigger); `Duty`/`TDuty` stop pulling and fire their `done_action` (the U4 set: `Done`, `FreeSelf`, …), which is how a finite sequence frees its own node.

**Side-effect UGens (`SendTrig`/`SendReply`/`Poll`).** Some UGens exist for a **side effect** — an OSC reply or a console post — not for audio on a bus, and a def may contain *only* these with **no `Out` at all** (the server requires at least one UGen, never an `Out`). All three fire on a **trigger**: a signal crossing from `≤ 0` up to `> 0`. `SendTrig` replies `/node_trigger nodeID id value`; `SendReply` replies at a custom address (its static `label` field, default `/reply`) carrying `nodeID reply_id value…` for an arbitrary value list; `Poll` posts `label: value` to the server console and, with a non-negative `trig_id`, also emits a `/node_trigger`. The replies reach every client registered with `/server_notify` (`SendReply`'s custom address is delivered verbatim). Mechanically the reply leaves the audio thread through a lock-free FIFO — the same discipline as the `/node_start`/`/node_end` node events — so triggering never allocates or blocks; a burst of triggers beyond the per-block buffer is dropped (best-effort, like the node events). The `id`/`reply_id`/`trig_id` and `value`s can be any signal (sampled at the trigger). The Python client builds them with `send_trig`/`send_reply`/`poll` and passes them as `SynthDef` roots.

**Frequency-domain chain (`FFT`/`PV_*`/`IFFT`).** Spectral processing bookends a
chain of `PV_*` (phase-vocoder) UGens between an `FFT` and an `IFFT`: `FFT`
windows an audio input and transforms it to a complex frame once per **hop**;
each `PV_*` mutates that frame (only on the blocks where a fresh one is ready); `IFFT`
inverse-transforms and overlap-adds it back to audio. Wire them in order —
`FFT`'s output feeds the first `PV_*`, whose output feeds the next, and the last
feeds `IFFT` — exactly like scsynth's chain. A def may of course also feed
`IFFT`'s audio into filters, `Out`, etc.

The chain is **not** block-rate: `FFT`/`PV_*` are control rate (`kr`, a per-block
ready marker on the wire) and only do work on hop boundaries; `IFFT` is audio
rate. The window size is a **static** field (`fft_size`, a power of two in
256/512/1024/2048/4096) given **only on the `FFT`** — the compiler propagates it
(and the window type) to the rest of the chain, so `PV_*`/`IFFT` need no size. An
unsupported size, or a `PV_*`/`IFFT` whose first input is not a spectral chain,
fails the def with `/fail`.

A **two-chain combiner** (`PV_Add`/`PV_Mul`/`PV_Min`/`PV_Max`/`PV_MagMul`/
`PV_CopyPhase`) takes **two** chain inputs — both must be spectral chains, of
the **same `fft_size`**, and **distinct** (the same chain on both sides fails
the def). The result lands in **chain A** (input 0): the combiner's output wire
carries chain A onward, and whatever `PV_*`/`IFFT` follows reads the combined
frame. It acts on the blocks where chain A has a fresh frame, reading chain B's
latest frame. The operator is a property of the *name*; all six are one
server-side implementation.

**Bin expressions (`PV_Kernel`).** The general per-frame mechanism: instead of
one registered name per spectral operation, `PV_Kernel` interprets a pair of
**user-written programs** over every bin of each fresh frame. The wire format
is a **postfix token list** per program, in the static fields `mag_expr` (the
bin's new magnitude) and `phase_expr` (its new phase):

- a **number** pushes a constant;
- a **word** is either a per-bin load — `mag`, `phase` (radians), `bin` (the
  index), `nbins` (`fft_size/2 + 1`), `binfreq` (the bin's center Hz),
  `p0`…`p31` (the UGen's parameter inputs, sampled at the hop) — or an
  **operator wire name** from the shared builtins tables (`mul`, `ge`, `max`,
  `tanh`, `pow`, … — the same names `BinaryOpUGen`/`UnaryOpUGen` accept).

Example — a spectral gate whose threshold is input 1:
`{"kind": "PV_Kernel", "inputs": [chain, thresh], "mag_expr": ["mag", "mag",
"p0", "ge", "mul"]}`. An omitted program is the identity; an identity
`phase_expr` keeps each bin's phase exactly (pure magnitude maps skip the
polar conversion). The def is validated at `/def_send synth` and fails with `/fail`
on an unknown word, a program whose stack discipline is broken (an operator
without operands, or a program not netting exactly one value), a `pN` past
the UGen's parameter inputs, a non-finite constant, or more than 256 tokens.
A program is a **pure per-bin map** — it cannot read other bins, keep state
across frames, or move energy between bins; those remain the dedicated
`PV_MagFreeze`/`PV_MagSmear`/`PV_BinShift` rows above.

**Partitioned convolution (`Conv`).** Convolution is **not** a `PV_*`: fast
convolution needs zero-padded rectangular segments whose hop is fixed by the
partition size, which is incompatible with the windowed COLA analysis chain (a
spectral multiply inside the chain computes *circular* convolution). `Conv` is
a self-contained audio UGen — one name covering what scsynth splits into
`Convolution`/`2`/`2L`/`3`/`StereoConvolution2L`. The kernel must be
**prepared off the audio thread**: `/buffer_gen bufnum prepare_partconv fftSize
irBufnum` partitions the IR in `irBufnum` (channel 0) into blocks of
`fftSize/2`, transforms each once on the NRT queue, and writes
`[L, P, P × fftSize packed spectra]` into the target (size it as
`2 + P·fftSize` frames). The audio thread only ever multiplies against the
ready spectra, and its per-block cost is **flat**: the partition products are
spread across the hop's blocks, so a long reverb tail does not spike the hop
block. Intrinsic latency is one partition (`fftSize/2` samples), reported by
the node (the first consumer of the latency hook; no delay compensation yet).
Moving the `kernel` input to a *different* prepared buffer crossfades over one
partition (scsynth's `Convolution2L` behavior); regenerating the same buffer
index switches hard — use a fresh buffer when the transition matters. An
unprepared, missing or mismatched (`L` differs) kernel plays silence; the
input history keeps running, so a valid kernel resumes cleanly.

Analysis and resynthesis use the same window (Hann by default), and the
overlap-add is **window-normalized** (divided by the steady-state window-overlap
denominator, COLA), so a plain `FFT`→`IFFT` reconstructs the signal at unity
gain, delayed by the transform latency (one window). The window type is settable live per instance
with `/node_ugenCmd <nodeID> <ugenIndex> window <wintype>` (`-1` rectangular, `0` Hann,
`1` sine, `2` Welch, `3` Hamming, `4` Blackman) — the first consumer of the typed
per-UGen command surface.

**Where the frame lives (deviation from scsynth).** scsynth threads the frame
through a client-allocated buffer whose bins the audio thread mutates in place.
Here the frame lives in **synth-private scratch** allocated when the synth is
instantiated (like the `LocalIn`/`LocalOut` feedback buffer, and the moral
equivalent of SuperCollider's `LocalBuf`), freed with the synth — **no
`/buffer_alloc` is required**. A pool buffer would now be able to hold it (its
contents are writable), so the reason is no longer the pool's: a frame is
scratch belonging to one synth, and it is a **spectrum in an internal layout**
rather than samples, which is not something to leave where `/buffer_write` and
the readers can reach it. A future extension may add copying the frame into a
buffer for inspection/sharing.

**Feedback (`LocalIn`/`LocalOut`).** The graph is a DAG — UGens cannot be wired in a cycle. To feed a signal back, write it with `LocalOut` and read it with `LocalIn`: they share a per-synth buffer that persists across blocks, so the value read is what was written **one control block (64 samples) earlier**. `LocalIn` for a channel must appear *before* its `LocalOut` (the compiler enforces this; it is what makes the delay exactly one block), and the channel index must be a constant. Use any number of channels (mono each, like buses). This is **block-rate** feedback — good for feedback delays, block feedback-FM, resonant combs (a one-channel loop resonates at `sampleRate / 64`). Sample-accurate (sub-block) feedback is not possible across composed UGens; fuse the loop into one node — a recursive UGen or a Faust def (`/def_send faust` with `~`).

**Streaming disk I/O (`DiskIn`/`DiskOut`).** These stream to/from disk in real time, so arbitrarily long files never touch the buffer pool (`PlayBuf`/`BufRd` load the whole file first). Each is **self-contained**: one background I/O thread plus a lock-free ring shared with the audio thread, opened at `/synth_new` and closed when the synth is freed. The audio thread only pushes/pops the ring — never blocks. A ring underrun (disk too slow) plays silence; a `DiskOut` overrun drops samples; both are rare with the ~1 s ring. They carry extra **static fields** in the UGen spec (alongside `kind`/`inputs`): `path` (required), `loop` (`DiskIn`, default false), and `format` (`DiskOut`, `int16`|`int24`|`float`, default `int16`). Both are **mono per UGen** like the buffer readers: `DiskIn` extracts one channel (`chan`) — a stereo file is two `DiskIn`s; `DiskOut` writes a mono WAV — record stereo with two `DiskOut`s. `DiskIn` streams one file frame per server sample with **no resampling** (pitch follows the sample-rate ratio, as in scsynth's `DiskIn`); these UGens spawn a thread each, so they are for a handful of streams, not per-voice. Example DiskOut UGen: `{"kind": "DiskOut", "inputs": [{"ugen": 0}], "path": "/tmp/rec.wav", "format": "float"}`.

Output happens exclusively through `Out`/`ReplaceOut`; a def without them is silent. Several synths with `Out` on the same bus mix. Bus-index inputs are ordinary signals, sampled at the first frame of each block and clamped to the valid range.

**Band limiting in `Saw`/`Pulse` (deviation from scsynth).** scsynth builds these from a discrete-summation impulse train — a sine table divided by a cosecant table — smoothed by a leaky integrator, over a 32-bit fixed-point phase. Ours accumulate phase in `f64` and correct each discontinuity with a fourth-order PolyBLEP, which costs no division, no table and no integrator: there is no settling transient, no residual DC droop, and no fixed-point tuning error. The trade is that PolyBLEP stays *quasi*-band-limited — a residual remains and grows with the fundamental. Measured alias SNR at 48 kHz, against the same waveform generated naively:

| fundamental | `Saw` | naive ramp | `Pulse` | naive square |
|---|---|---|---|---|
| 105 Hz | 96.7 dB | 30.9 dB | 98.4 dB | 32.7 dB |
| 996 Hz | 42.6 dB | 16.0 dB | 43.5 dB | 17.7 dB |
| 3996 Hz | 39.2 dB | 9.9 dB | 38.9 dB | 11.4 dB |

The 105 Hz figure is within about 2.5 dB of what the measurement itself can resolve, so the low end is effectively transparent. The `LF*` shapes are deliberately not band-limited at all, as in scsynth — they are modulation sources and their corners should be exact.

**The two-pole rows are one filter (deviation from scsynth).** scsynth realizes `LPF`, `HPF`, `BPF`, `BRF`, `RLPF`, `RHPF` and `Resonz` as separate direct-form sections, each with its own coefficient formula. Here they are one *topology-preserving* (trapezoidal-integrator) state-variable filter, which implements the **same** prototype — the bilinear transform of the analog two-pole — and therefore the same transfer function, verified against it to within 0.1 dB across nine octaves. What changes is behaviour, not response: it does not leave its stable region under audio-rate cutoff modulation, it stays well conditioned at low cutoff, and lowpass/bandpass/highpass/notch all fall out of the same pair of integrator updates. That last point is why `Svf` exists at all: exposing the tap mix as inputs costs the mix and nothing else, so a filter whose response is itself a signal is free here and would mean recomputing coefficients in a direct-form section. `BPF` and `Resonz` are the same row twice on purpose — scsynth ships two historically distinct resonators that promise the same parameterization and the same unity peak gain.

State and coefficients are `f64` throughout, matching scsynth's own choice for the same reason: near DC the poles crowd the unit circle and `f32` truncation dominates the output. The `tan` and reciprocal that turn a cutoff into integrator gains run **once per block** when the parameters are scalar, and twice — block start and end, linearly interpolated — when either is audio-rate, so a modulated filter costs three multiply-adds per sample rather than a transcendental.

Resonance travels as **`rq`**, the reciprocal of Q, as in scsynth. That is not a performance choice (one division per block, next to a `tan` that costs several times more) but a domain one: `rq = 0` is infinite Q and is exactly representable, where `Q = 0` would divide by zero and `Q → ∞` is not a number. Clients are free to offer `q` and convert.

**The delay family is one line (deviation from scsynth).** `DelayN/L/C`, `CombN/L/C` and `AllpassN/L/C` are the same circular buffer with two independent parameters: how a fractional tap is interpolated, and what is fed back. Measured through a half-sample delay at 9 kHz, `L` loses about 1.6 dB and `C` about 0.36 dB — four-point interpolation is not a brick wall, but that is the gap that justifies paying for `C` on a modulated delay.

The line is **synth-private memory**, allocated when the synth is built and sized from the static `max_delay` field (in seconds) and the server's sample rate — memory no other node can reach, which is what a per-voice delay wants. Unlike scsynth, `max_delay` is **static configuration and not an input**: it sizes an allocation, so it belongs with `fft_size` and `partitions` rather than among the signals, and it defaults to 0.2 s if the def omits it. A `delaytime` past it is clamped, never wrapped.

**The `BufDelay*` family is the same nine over a pool buffer** (the rows above), for the case the private line cannot serve: a line whose contents are addressable — recorded into, read by another node, resampled, saved. One implementation serves both, parameterised by where the samples live, so the two families cannot drift apart in their arithmetic. Which to reach for is a question about the *line*, not about the sound: private if nothing else needs to see it, a buffer if something does.

These UGens do **not** report an intrinsic latency, on purpose. Their delay is what the user asked for, not an artifact of processing; the latency hook exists for something like the partitioned convolver, and compensating a musical delay would silently undo it.

Buffer readers **and writers** are **mono** (one output per UGen, unlike scsynth's multi-output PlayBuf): the `chan` input picks the channel, and two of them with the same inputs stay sample-locked, so a stereo file is two UGens.

**A cue point belongs to whoever carries a position.** `PlayBuf` and `RecordBuf` advance themselves, so each takes a `trigger`, a frame to cue to (`startPos`/`offset`) and a `doneAction`. `BufRd` and `BufWr` are driven by a phase signal and have no position of their own: re-cueing one means changing the signal that drives it, and a done action would have nothing to be done with.

**A buffer's contents are mutable, and only its shape is fixed.** Frames, channels and sample rate are settled at allocation and never change; every sample can be written at any time, by a UGen or by a `/buffer_*` command, while anything else is reading it. That is scsynth's model, and it is why `RecordBuf`, `BufWr` and the `BufDelay*` family exist at all: recording into a buffer another node is playing is the ordinary case. What a reader crossing a writer sees is some old samples and some new, never half of one — per-sample atomicity, no ordering between samples, which is what a looper crossing its own write head has always sounded like. Nothing has to be declared: there is no writable *kind* of buffer, because they all are.

**A write costs the samples it writes, and it lands where the reader is looking.** `/buffer_set`, `/buffer_setRange`, their `*Channel` forms, `/buffer_fill`, `/buffer_gain` and `/buffer_reverse` write **in place**: the samples go into the cells the engine is already reading, so the cost is the span's rather than the buffer's (a 1 ms run costs the same on a ten-minute take as on a ten-second one) and nothing a recorder wrote meanwhile is lost. The same rule as above is what a client sees: a reader crossing the write sees some old samples and some new, never half of one, and the write becomes audible on the next block with no further command. The commands that *replace* a buffer rather than write into it — `/buffer_alloc`, `/buffer_allocRead`, `/buffer_read`, `/buffer_gen` — are the ones that change what a buffer *is* (its shape, its sample rate, its whole contents), and those still install a new buffer whole; a recording crossing one of those is replaced along with everything else, which is what replacing the samples means.

## Buffers (`/buffer_*`)

```text
/buffer_alloc     bufnum frames [channels=1]                  # zeroed buffer
/buffer_allocRead bufnum path [fileStart=0] [numFrames=0=all] # shape from the file
/buffer_read      bufnum path [fileStart=0] [numFrames=-1=all] [bufStart=0]
/buffer_allocReadChannel bufnum path fileStart numFrames channel...  # shape from the file, selected channels
/buffer_readChannel      bufnum path fileStart numFrames bufStart channel...
/buffer_write     bufnum path [header="wav"] [format="int16"|"int24"|"float"] [numFrames=-1] [startFrame=0]
/buffer_zero      bufnum
/buffer_gen       bufnum cmd flags args...  # fill/generate — see the wavetable section
/buffer_fill      bufnum [start count value]...  # runs of one value (flat, like /buffer_set)
/buffer_gain      bufnum [start=0] [frames=-1] gain [to=gain] [shape=1] [curve=0]  # scale a span
/buffer_reverse   bufnum [start=0] [frames=-1]                # turn a span around
/buffer_set       bufnum [index value]...   # write single samples
/buffer_setRange  bufnum [start blob]...    # write runs of samples (f32 LE blob)
/buffer_setChannel      bufnum channel [frame value]...  # one channel, positions in frames
/buffer_setRangeChannel bufnum channel [frame blob]...   # one channel, runs of frames
/buffer_free      bufnum
/buffer_attach    bufnum                    →  /done /buffer_attach bufnum   # map a shared buffer, see below
/buffer_touch     bufnum channel start frames                  # a peer wrote a span; broadcast as /buffer_touched
/buffer_close     bufnum                    →  /done /buffer_close bufnum   # see note
/buffer_query     bufnum...                 →  /buffer_query.reply  bufnum frames channels sampleRate ...  (frames -1: empty slot)
/buffer_query     (no argument)              →  /buffer_query.reply  every allocated buffer, same shape
/buffer_get       bufnum index...           →  /buffer_get.reply   bufnum index value ...
/buffer_getRange      bufnum [start count]...   →  /buffer_getRange.reply  bufnum start blob ...  (f32 LE blob)
/buffer_export    bufnum path               →  /done /buffer_export bufnum
/buffer_render    bufnum frames             →  /done /buffer_render bufnum   # offline sessions only, see below
/buffer_stream    periodMs bucket bufnum...  →  /buffer_stream.reply  bufnum startFrame bucket blob   # the overview as it is written
/buffer_peaks     bufnum [bucket=256] [start=0] [frames=-1]  →  /buffer_peaks.reply  bufnum startFrame bucket blob   # the overview as it stands
```

`/buffer_alloc`, `/buffer_allocRead`, `/buffer_read`, `/buffer_write`, `/buffer_zero`, `/buffer_gen`, `/buffer_set`, `/buffer_setRange`, `/buffer_setChannel`, `/buffer_setRangeChannel`, `/buffer_fill`, `/buffer_gain`, `/buffer_reverse`, `/buffer_readChannel`, `/buffer_allocReadChannel` and `/buffer_free` are **asynchronous**: the work happens on a dedicated NRT thread (one queue, so commands on the same buffer complete in submission order) and the reply is `/done <cmd> bufnum` or `/fail <cmd> reason`. Buffers keep the file's sample rate (the server never resamples — see `PlayBuf`'s rate above); integer WAVs are scaled to ±1. `/buffer_read` requires an allocated buffer and keeps its shape; channel-count mismatches fail. Reading decodes by **content**, not extension: WAV goes through hound (exact, int24-aware), and FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and CAF decode through [symphonia](https://github.com/pdeljanov/Symphonia) (whole-file decode, then slice — compressed formats have no cheap exact frame seek). `/buffer_write` still emits WAV only, and `leaveOpen` (streaming) is not supported. `/buffer_close bufnum` closes the soundfile a streaming buffer left open — scsynth pairs it with `DiskIn`/`DiskOut`; since Clausters has no streaming buffers yet (every `/buffer_read`/`/buffer_write` reads or writes the whole file and closes it), it validates the buffer is live and replies `/done /buffer_close bufnum`, forward-compatible with the streaming UGens.

`/buffer_attach bufnum` is the one command that exists only for a server sharing a segment with another one. A server that **attached** to somebody else's segment (see [`ipc.md`](ipc.md)) maps every buffer the owner had published when it started; this points it at one published since. It is synchronous, replies `/done /buffer_attach bufnum`, and fails when this server owns the samples, when it has no shared segment, or when the directory has no live buffer under that number. Nothing about the samples travels: the reply means this server's engine now plays the very cells the owner writes.

`/buffer_touch bufnum channel start frames` is the announcement a **local peer** makes after editing a shared buffer in place. A peer that maps the samples writes into the very cells the engine reads, and nothing about that reaches the wire — which is the point, and also why a second client holding a picture of the same take would never find out. This carries the span and not the samples: the server broadcasts `/buffer_touched bufnum channel start frames` to every client registered with `/server_notify` **except the one that wrote**, and what a receiver does with it depends on how it holds the samples: a peer that **maps** it re-summarizes that span (its samples are already the new ones — this is what the GUI host does), and one holding its own copy re-reads the span with `/buffer_getRange`. It is a notification rather than a command, so nothing is answered to the sender. A page is exactly who needs it: a browser cannot map a file, so a message is the only way it can hear about an edit at all.

**`/buffer_stream periodMs bucket bufnum...`** subscribes the sending client to the **overview of samples as it is written** — a recording filling a picture, for a client that cannot map the region and watch it fill. The server acks `/done /buffer_stream`, then sends every `periodMs` (10 ms floor) one **`/buffer_stream.reply bufnum startFrame bucket blob`** per watched buffer whose write frontier has moved by at least one whole bucket since that client's last report — and nothing at all for one that has not moved, so a still buffer costs no traffic. The blob is **bucket-major, channel-minor**: for each bucket in order, for each channel, `min`, `max` and **mean square** as raw little-endian `f32` — the peak pyramid's own three statistics in its own energy form, so a client folds them into its summary without converting anything. `startFrame` is where the report begins, on the buffer's own sample axis. **The receiving half is in the shared core**, so no client folds this by hand: `peaks::MultiPyramid::write_buckets` takes the blob as it arrives — `peaks_cache_stream_file` in the Python client, `Peaks.writeBuckets` in the web one — and puts the buckets into the pyramid the picture already holds, rebuilding the levels above them. Subscribe with the bucket that pyramid was built at (256 unless it says otherwise) and the two grids agree by construction.

**`/buffer_peaks bufnum [bucket] [start] [frames]`** answers the same overview for a buffer that is **standing still**, and the pair is a distinction in the samples rather than in the client: what is being written is pushed as it is written, what is finished is asked for. The reply — **`/buffer_peaks.reply bufnum startFrame bucket blob`** — carries the identical blob, so the receiving half does not fork: the same `write_buckets` folds either one into the pyramid a picture already holds. `start` is rounded **down** to a whole bucket and comes back as `startFrame`, since a bucket summarized from part of itself would report a peak the samples do not have; `frames` of -1 runs to the end. The conversation is `/buffer_getRange`'s — one request, one reply, and the reply's own length says how much came, at most 4096 buckets — so a client walking a long take asks again from where the blob ended, and nothing is remembered between requests. It is synchronous, like `/buffer_get` and `/buffer_export`: the server reads the span once, a bucket at a time, and allocates only the summary.

What it is for is the round trip it replaces. A view of a server buffer that cannot map it had two ways to get a picture: download every sample (230 MB for a ten-minute stereo take), or have nothing until something records into it. This is the third — about a hundredth of the bandwidth, enough to draw the whole take at once, with the spans under a zoom read back by `/buffer_getRange` as they are needed.

The unit is the **summary and not the samples**, which is the whole point of both commands: one channel at 48 kHz is 187 buckets a second at the default 256, about 2.2 kB/s, against 192 kB/s for the audio it describes. At most 32 buffers per subscription and 4096 buckets per report (the rest arrives in the next one). Same posture as `/bus_stream`: one subscription per client, **replaced** by each new call, `periodMs <= 0` (or an empty buffer list) cancels, it dies with a TCP/WebSocket connection, and it is not schedulable in a timed bundle. A subscription is a watch on what happens **next** — it reports from the frontier as it stands when you subscribe, not the overview of what is already there, which is a fetch and is spelled `/buffer_peaks`.

What moves the frontier is a **writing UGen** (`RecordBuf`, `BufWr`): each publishes, once per block, the highest frame it has filled. The frontier is the **buffer's own**, so this command works on any server — an engine inside a page, a `clausters` booted without `--shm` — and when there *is* a segment the same number is mirrored into the buffer directory's row (see [`ipc.md`](ipc.md)), which is where a peer that maps the region reads it. Such a peer needs no subscription at all: it re-summarizes the new span out of the memory it already has. This command is that reading, for whoever cannot map.

### Filling and channel-selective reads (`/buffer_fill`, `/buffer_readChannel`, `/buffer_allocReadChannel`)

`/buffer_fill bufnum [start count value]...` writes runs of one repeated value — scsynth's `/b_fill`, and the sibling of the `/bus_fill` beside it. Its indices are **flat and interleaved**, like `/buffer_set` and unlike the editing verbs below whose spans are frames: this is the writing family's member, not an editor's verb. Several runs ride in one message, and a run past the end **fails** rather than clamping, like every other write here.

`/buffer_readChannel` and `/buffer_allocReadChannel` are `/buffer_read` and `/buffer_allocRead` with a **channel selection**: the trailing integers name which of the file's channels to keep, and the positions before them are required rather than optional, since with a variadic tail there is no telling a `fileStart` from a channel index. An empty selection is every channel, which is exactly what the two commands without `Channel` in their name are. The order is honoured and repeats are allowed — `1 0` swaps a stereo pair, `0 0` makes a mono file two-channel — and a channel the file does not have **fails** rather than reading as silence, since asking for the right channel of a mono file is a mistake worth hearing about. `/buffer_readChannel` keeps the target's shape like `/buffer_read`, so the selection must match its channel count; `/buffer_allocReadChannel` takes its shape from the file *and the selection*, which is how one channel of a stereo file lands in a mono buffer — the thing that could not be done at all before.

### The destructive edits (`/buffer_gain`, `/buffer_reverse`)

The verbs an editor applies to a selection. `/buffer_gain bufnum start frames gain [to] [shape] [curve]` scales a span; `/buffer_reverse bufnum start frames` turns one around, frame by frame, so a stereo pair stays a stereo pair.

**Their span is in frames, not in flat sample indices**, which is the one thing to keep straight against `/buffer_set`/`/buffer_setRange` next to them: a selection is a stretch of time across every channel, and every channel of a frame is scaled alike — a fade cannot tilt a stereo image. `frames` of -1 runs to the end, and a span past the end **fails** rather than being clamped, like the rest of the writing family.

One gain value is a constant factor; a second (`to`) makes it a fade, sweeping along `shape` — the [envelope shape numbers](#envelopes) `EnvGen` and the breakpoint editor already speak, with `curve` read only by the custom-curvature shape (5). So a fade in is `0 1`, a fade out `1 0`, and a silence `0 0`, which lands on exact zeros where a fade only tends to its target (the envelope convention: the last frame has not arrived yet). The arithmetic is `clausters_core::edit`, the same function a client or an offline session calls, so a fade sounds identical wherever it is applied.

Like every write here they are asynchronous and **compose in flight**: a batch of edits on one buffer chains, each building on what the queue last produced rather than each on the contents you started with, so three edits fired back to back give the same result as three awaited one at a time.

### Rendering into a buffer (`/buffer_render`)

`/buffer_render bufnum frames` runs the **graph** for `frames` frames and installs what came out of the output buses into `bufnum` — `/buffer_gen`'s sibling, generating into a buffer by playing rather than by formula, and what an editor means by *apply this def to this selection*. What lands **replaces** the buffer: `frames` frames of as many channels as the server has outputs, at its sample rate. The index must already be allocated, which is how the caller says that slot is the one they mean.

**Only an offline server answers it, and every other one fails it.** Running the graph means advancing the engine, and in a real-time server the audio device advances it against a clock nobody else may touch; there is no correct moment for this command there, so it is refused rather than approximated. An offline session (no audio device, no clock of its own — the mode an editor works in) owns its clock and performs the render between commands, which is also why the reply comes when the operation has finished rather than when it was accepted.

Determinism is the point of it: the samples a `/buffer_render` leaves are the samples the same samples yields as a score rendered offline, which is asserted sample for sample. Pin the server's seed if the graph is stochastic and you want the take back.

### Table generation and the wavetable format (`/buffer_gen`)

`/buffer_gen bufnum cmd flags args…` fills an **already-allocated** buffer with a computed signal — additive spectra, a waveshaping curve, a copy from another buffer, or a prepared convolution kernel. Like `/buffer_read` it reads the target's shape from the current contents, so a `/buffer_gen` right after a `/buffer_alloc` must be separated by a `/server_sync` (the alloc has to complete first). It runs on the same NRT queue and replies `/done /buffer_gen bufnum`.

The `flags` int packs three bits, `normalize`(1) + `wavetable`(2) + `clear`(4) — the usual value is `7` (all three). `normalize` scales the result to a peak magnitude of 1; `clear` starts from silence (without it the new signal is **added** on top of the buffer's current contents); `wavetable` stores the result in the interleaved wavetable format below (an `N`-sample buffer then holds `N/2` period points). `copy` takes no flags.

| cmd | args | fills with |
|---|---|---|
| `sine1` | flags, amp… | additive sine partials; `amp[k]` is the amplitude of harmonic `k+1` |
| `sine2` | flags, (freq amp)… | partials at arbitrary (possibly fractional) harmonic numbers |
| `sine3` | flags, (freq amp phase)… | as `sine2` with a per-partial phase in radians |
| `cheby` | flags, amp… | a waveshaping transfer function `Σ amp[k]·T_{k+1}(x)` of Chebyshev polynomials over `x∈[−1,1]` (`amp[0]` weights `T₁`, the linear/passthrough term); read by `Shaper` |
| `copy` | dstStart srcBufnum srcStart numSamples | overlays `numSamples` of another buffer onto this one (`numSamples < 0` = to the end of the shorter side) |
| `env` | level0, (level time shape curve)… | discretizes a break-point envelope across the whole buffer (see below); no flags |

`env` fills the buffer with a **break-point curve** — the buffer-world form of an automation curve. The arguments are the same decomposition an `EnvGen` carries: an initial `level0`, then one `(level, time, shape, curve)` quad per segment, where `shape` is the envelope-shape number (0 step, 1 linear, 2 exponential, 3 sine, 4 welch, 5 custom-`curve`, 6 squared, 7 cubed, 8 hold) and `curve` is read only by the custom shape. Each output sample evaluates the segment it falls in through the **same shared math** (`clausters-core`'s `envshape`) the `EnvGen` UGen plays, so a curve drawn or edited on a client (the `bpf` editor) and this buffer read identically. Segment `time`s are **relative** — only their proportions matter, since the buffer holds the curve *shape*; playback rate maps it onto real time (e.g. a `PlayBuf` whose rate spans the buffer over the desired duration). The mono curve is written to every channel. A client reads it back onto a control bus to drive `/node_map`-ed controls.

**The wavetable format.** An interpolating oscillator (`Osc`/`VOsc`) reads a period stored not as raw samples but as scsynth's interleaved offset/slope pairs: for each point `i` the buffer holds `[2·a[i] − a[i+1], a[i+1] − a[i]]`. With the fractional phase `frac∈[0,1)`, a sample is then one fused multiply-add — `x0 + (1+frac)·x1 = a[i] + frac·(a[i+1] − a[i])` — with no branch. `sine1/2/3` build periodic (wrapping) tables; `cheby` builds a non-wrapping one (it holds its endpoint, since a transfer curve is not periodic). A `wavetable`-format buffer is meant for `Osc`/`VOsc`/`Shaper`, not `PlayBuf`; a plain (non-`wavetable`) `/buffer_gen` buffer is a normal signal (read it with `OscN` or `BufRd`). This is the buffer-world counterpart of a Faust def's small embedded `waveform` table (see *Tables and waveforms* under Faust defs): the same idea — precompute a period or a transfer curve numerically — for the UGen graph instead of a JIT def.

`/buffer_query`, `/buffer_get` and `/buffer_getRange` are **synchronous** reads, answered from the network-side buffer mirror (state as of the last completed command). `/buffer_query` with **no argument** lists every allocated buffer instead of querying named ones — see [Def, buffer and UGen introspection](#def-buffer-and-ugen-introspection). `/buffer_get` reads single samples by flat (interleaved) index; `/buffer_getRange` reads ranges, asking in samples and answering with one **little-endian `f32` blob** per range, with `count` clamped to what the buffer holds from `start` — a request past the end returns only the available samples, and an unallocated buffer returns an empty blob. Sample indices are flat across channels (`frame * channels + channel`), so a stereo buffer reads as interleaved `L R L R ...`. Large buffers are read in client-chosen chunks sized to the client's transport: over TCP/WebSocket a chunk may be as large as the `/server_query` frame ceiling (megabytes per round-trip), over UDP each reply must fit one datagram.

`/buffer_set` and `/buffer_setRange` are the **write** half of that pair. `/buffer_set bufnum index value ...` writes single samples by flat index; `/buffer_setRange bufnum start blob [start blob ...]` writes runs, several per message, each run's samples carried as one **little-endian `f32` blob** whose length *is* the run length. They are asynchronous like the rest of the writing family, replying `/done /buffer_set bufnum`, and they require an allocated buffer whose shape they keep — a write is laid into a **copy** that replaces the buffer whole, so the engine never reads a partially written one and no write needs a lock. Two rules differ from the reads, both deliberate: a range past the end **fails** instead of being clamped (a short read hands back less than was asked for, a short write would lose data the caller believes it stored), and the shape is read from the mirror, so a write right after a `/buffer_alloc` needs a `/server_sync` between them exactly as `/buffer_gen` does. There is no protocol-level chunking: a multi-megabyte edit is several messages, each sized by the client against the `--max-frame` ceiling that `/server_query` advertises, symmetrically with how `/buffer_getRange` is chunked on the way in. Nothing is notified when a buffer changes — the mirror is authoritative and a reader that cares re-reads.

**One channel at a time: `/buffer_setChannel` and `/buffer_setRangeChannel`.** Storage is interleaved, so a *channel* of a buffer is not a contiguous run — its frames are `channels` apart — and no flat start and length can name one. These two are the same two commands with the span written in the caller's terms: `/buffer_setChannel bufnum channel [frame value]...` and `/buffer_setRangeChannel bufnum channel [frame blob]...`, where every position is a **frame of that channel** and the run's samples are that channel's consecutive frames. The channel comes **before** the runs because the runs are the variadic tail (the same reason `/buffer_readChannel` puts its channel list last, after fixed positions), and one message writes one channel: two channels are two messages, like every other per-channel thing here. Everything else is the writing family's as it stands — asynchronous with a `/done`, laid into a copy that replaces the buffer whole, chained so a batch composes, and a range past the end **fails**, reported in frames since that is the unit that was written in. A channel the buffer does not have fails too, for the same reason asking for the right channel of a mono *file* does. This is what makes an **editor** possible over a multichannel take: drawing on one channel of a stereo buffer is a strided write, and sending it as one message per sample is the encode the blob rule exists to avoid.

**Bulk samples travel as a blob, and that is a rule of this protocol, not of these two commands.** A payload whose length scales with the *audio* — a buffer range, a scope window — rides as raw little-endian `f32`: `/buffer_setRange` and `/buffer_getRange.reply` here, `/bus_tapStream.reply` under Audio taps, and `/buffer_export`'s file. A payload whose length scales with the *parameters* — `/buffer_set`'s scattered indices, `/bus_getRange.reply`'s control values, a UGen's inputs — stays typed OSC arguments. The boundary is worth naming because the cost is not marginal: N samples as N float arguments is N type tags and N encode steps at each end, which is thousands of times slower than one byte copy at the sizes an editor works with, and the arguments are larger on the wire too (5 bytes per sample against 4).

`/buffer_export bufnum path` writes the buffer's raw samples — flat, interleaved, little-endian `f32` — to a local file and replies `/done /buffer_export bufnum` (or `/fail` on a missing buffer or write error). It is the **bulk-data path** for a same-machine consumer: where `/buffer_getRange` chunks a buffer over the network, `/buffer_export` puts a multi-megabyte buffer in a file the consumer memory-maps and reads zero-copy, with no per-sample OSC. It is synchronous on the network thread (not the audio thread), reading the same mirror as `/buffer_query`. The reader pairs the file with the buffer's channel count (from `/buffer_query`) to de-interleave. (This is what the GUI host's mapped-`path` waveform reads.)

## Faust defs (`/def_send faust`)

`/def_send faust name payload` — the payload is **Faust source** unless its first non-whitespace byte is `{`, in which case it is JSON: a **box tree** (root `{"op": …}`, below) or a **signal tree** (root `{"signals": […]}`, see [JSON signal tree](#json-signal-tree-the-signal-api)). All three are JIT-compiled (LLVM) on a dedicated compiler thread; expect the `/done`/`/fail` reply a few milliseconds later.

### Controls

A Faust synth exposes, in this order:

1. every UI element of the def (`hslider`, `vslider`, `nentry`, `button`, `checkbox`) addressed by its **label**;
2. two reserved names: `out` — first output bus (default 0, hardware left) — and `in` — first input bus, for defs that process signal. A def with N outputs writes (sums) to buses `out .. out+N-1`; same for inputs.

```text
/synth_new fsine 2000 1 0 freq 330 out 1
/node_set 2000 freq 660
```

If a def declares its own `out`/`in` control, the def's wins.

### Faust source payloads

Any complete Faust program; `import("stdfaust.lib")` and friends resolve against the stdlib installed with libfaust (`<prefix>/share/faust`). Compilation is single precision (`FAUSTFLOAT` = `f32`) with `-ftz 2` (recursive state below the normal float range flushes to zero — decaying tails cannot stall the audio thread in subnormal math), and the sample rate is fixed per instance at `/synth_new` time.

```text
/def_send faust fsine 'import("stdfaust.lib"); freq = hslider("freq", 440, 20, 20000, 0.01); process = os.osc(freq) * 0.2;'
```

### JSON box tree payloads

The JSON mirrors Faust's Box API one-to-one: every node denotes a box expression, and the tree is the `process` definition. Two shorthands: a JSON **number** is a constant box (`int` if integral, `real` otherwise), and the **strings** `"_"` / `"!"` are the wire and cut primitives. Everything else is an object with an `"op"` field:

| `op` | fields | Faust equivalent |
|---|---|---|
| `int`, `real` | `value` | constant |
| `wire`, `cut` | — | `_`, `!` |
| `seq`, `par`, `split`, `merge` | `in`: array of ≥ 2 boxes, folded left | `:` `,` `<:` `:>` |
| `rec` | `in`: exactly 2 boxes | `~` |
| `add` `sub` `mul` `div` `fmod` `pow` `min` `max` `atan2` `gt` `lt` `ge` `le` `eq` `ne` `and` `or` `xor` | `in`: exactly 2 boxes | binary operators |
| `sin` `cos` `tan` `asin` `acos` `atan` `exp` `exp10` `log` `log10` `sqrt` `abs` `floor` `ceil` `rint` `round` `intcast` `floatcast` | `in`: exactly 1 box | unary functions |
| `delay` | `in`: signal, delay length | `@` |
| `select2` | `in`: selector, then 2 branches | `select2` |
| `select3` | `in`: selector, then 3 branches | `select3` |
| `hslider`, `vslider`, `nentry` | `label`, `init`, `min`, `max`, `step` | named control |
| `button`, `checkbox` | `label` | named control (0/1) |
| `hgroup`, `vgroup` | `label`, `in`: exactly 1 box | control grouping |
| `waveform` | `values`: non-empty array of numbers | `waveform{…}` — outputs the (size, content) pair |
| `rdtable` | `in`: size, init, ridx — or 2 boxes when a `waveform` stands in for (size, init) | `rdtable` |
| `rwtable` | `in`: size, init, widx, wsig, ridx — or 4 boxes starting with a `waveform` | `rwtable` |
| `faust` | `src` | escape hatch: a complete Faust program compiled to a composable box, with stdlib access |

Two consumers build this format today: the Python client's `clausters.defs.boxes` module (the box algebra as composable Python values; see the [client book's defs chapter](https://clausters-python.readthedocs.io/en/latest/defs.html)) and machine-generated graphs (the GUI host). `faust` fragments are **memoized by `src` within one compilation**: the same source text yields the same box, so a client that reuses one fragment value many times (duplicating the subtree in the JSON) gets one computation and one compile — every `CDSPToBoxes` evaluation would otherwise mint fresh recursion symbols and defeat the sharing (the CSE suite in `tests/faust_box.rs` pins this).

Example — `sin(2π·phasor(freq)) * 0.2` with `freq` as a named control (`wrap(x) = x - floor(x)`, `phasor = (+(freq/SR) : wrap) ~ _`):

```json
{"op": "mul", "in": [
  {"op": "sin", "in": [{"op": "mul", "in": [
    6.283185307179586,
    {"op": "rec", "in": [
      {"op": "seq", "in": [
        {"op": "add", "in": ["_", {"op": "div", "in": [
          {"op": "hslider", "label": "freq",
           "init": 440.0, "min": 20.0, "max": 20000.0, "step": 0.01},
          48000.0]}]},
        {"op": "split", "in": ["_",
          {"op": "sub", "in": ["_", {"op": "floor", "in": ["_"]}]}]}]},
      "_"]}]}]},
  0.2]}
```

The `faust` op is the bridge to the stdlib — an embedded program becomes a box you can compose with primitives:

```json
{"op": "seq", "in": [
  {"op": "faust", "src": "import(\"stdfaust.lib\"); process = os.osc(330);"},
  {"op": "mul", "in": ["_", 0.2]}
]}
```

### Tables and waveforms

`waveform` embeds a small lookup table in the def itself: the client computes the values numerically — a wavetable period, a waveshaping transfer function — instead of formatting them into Faust source. The box outputs the (size, content) pair the table primitives expect, so the usual idiom is `rdtable` with just two inputs:

```json
{"op": "rdtable", "in": [
  {"op": "waveform", "values": [0.0, 0.25, 0.5, 0.75]},
  {"op": "intcast", "in": [{"op": "and", "in": [
    {"op": "intcast", "in": [{"op": "sub", "in": [
      {"op": "rec", "in": [{"op": "add", "in": ["_", {"op": "int", "value": 1}]}, "_"]},
      {"op": "int", "value": 1}]}]},
    {"op": "int", "value": 3}]}]}
]}
```

`rdtable` also accepts the explicit (size, init, ridx) form with plain boxes, and `rwtable` (size, init, widx, wsig, ridx) is a table written and read at audio rate. Sizes must be constant expressions and indexes integers (`intcast`), as in Faust.

This `waveform` box is the Faust-side counterpart of `/buffer_gen`'s wavetables (see *Table generation and the wavetable format* under Buffers): `waveform` inlines a small table **inside one def** for a self-contained JIT program, whereas `/buffer_gen` fills a **server buffer** shared by the whole UGen graph and read by `Osc`/`Shaper`. Same idea — precompute a period or a transfer curve numerically — at two different scales.

**Soundfiles read server buffers.** Faust's `soundfile("<bufnum>", n)` primitive binds to the server buffer whose index is its label (a plain integer string), e.g. `soundfile("0", 1)` reads buffer 0. At `/synth_new` the instance's soundfile is filled from that buffer's current contents (deinterleaved to Faust's planar layout); a non-numeric label or an empty/missing slot yields a silent placeholder, so a def always instantiates. The primitive's outputs are `[length, sampleRate, channel0 … channel_{n-1}]` and the read index saturates at the part length, exactly as in stock Faust. The bind is a **snapshot** taken at instantiation — re-`/synth_new` to pick up a buffer that changed; loading is mono-or-more by the buffer's own channel count (Faust reads up to `n`). For *streaming* a bus instead of a static buffer, the older path still works too: route a `PlayBuf`/`BufRd` through an audio bus and read it via the def's reserved `in` control, so both def families stay composable on the same buses.

### Errors

Structural problems (unknown ops, missing fields, wrong arities in `"in"`) fail during interpretation and the `/fail` message carries the path of the offending JSON node from the root `$`, e.g. `at $.in[1].op: unknown op "mul3"`. Semantic errors — composition arity mismatches, dangling inputs — are reported by the Faust compiler verbatim, prefixed with the path of the fragment for `faust` ops.

## JSON signal tree (the Signal API)

A third `/def_send faust` format maps Faust's lower-level **Signal API** (`Csig*`) instead of the box algebra. It is selected by the **shape of the JSON**: a root object keyed by `"signals"` (`{"signals": [ … ]}`) is a signal tree, anything else starting with `{` is a box tree, and a non-`{` payload is raw source. The `"signals"` array lists one node per DSP **output** (this is how a signal def declares more than one output).

Where boxes compose point-free, signals are explicit: there is no implicit wire (`"_"`), inputs are addressed by index (`{"op": "input", "index": n}`), delays are explicit (`delay`/`delay1`), and **feedback** is explicit — `{"op": "recursion", "in": [body]}` with `{"op": "self"}` inside the body (the `CsigRecursion`/`CsigSelf` pair, one implicit sample of delay). That single recursive node is sample-accurate feedback fused into one DSP — the thing the UGen graph's block-rate `LocalIn`/`LocalOut` cannot do.

| `op` | fields | Signal API |
|---|---|---|
| `int`, `real` | `value` | `CsigInt` / `CsigReal` (a bare number works too) |
| `input` | `index` | `CsigInput` |
| `delay` | `in`: signal, delay | `CsigDelay` |
| `delay1` | `in`: 1 signal | `CsigDelay1` |
| `recursion` | `in`: 1 body using `self` | `CsigRecursion` |
| `self` | — | `CsigSelf` (only inside a `recursion` body) |
| `add` `sub` `mul` `div` `rem` `fmod` `remainder` `pow` `min` `max` `atan2` `gt` `lt` `ge` `le` `eq` `ne` `and` `or` `xor` `lsh` `rsh` | `in`: exactly 2 signals | binary ops (bit ops/shifts need integer operands) |
| `sin` `cos` `tan` `asin` `acos` `atan` `exp` `exp10` `log` `log10` `sqrt` `abs` `floor` `ceil` `rint` `intcast` `floatcast` | `in`: exactly 1 signal | unary functions |
| `select2`, `select3` | `in`: selector, then 2 / 3 signals | `CsigSelect2` / `CsigSelect3` |
| `hslider`, `vslider`, `nentry` | `label`, `init`, `min`, `max`, `step` | named control |
| `button`, `checkbox` | `label` | named control (0/1) |
| `fconst`, `fvar` | `ctype`: `"int"`/`"real"`, `name`, `file` (optional) | `CsigFConst` / `CsigFVar` — a runtime scalar resolved at instance init |
| `hbargraph`, `vbargraph` | `label`, `min`, `max`, `in`: 1 signal | passive monitor (passes the signal through) |
| `waveform` | `values`: non-empty array of numbers | `CsigWaveform` (size is `int(len)`) |
| `rdtable` | `in`: size, init, ridx | `CsigReadOnlyTable` |
| `rwtable` | `in`: size, init, widx, wsig, ridx | `CsigWriteReadTable` |

Differences from the box schema: no `seq`/`par`/`split`/`merge`, `hgroup`/`vgroup`, the `"_"`/`"!"` shorthands or the `faust` source escape hatch (those are box/UI-tree concepts); `round` is absent upstream (`rint` rounds); N-ary mutual recursion (`selfN`/`recursionN`) is not exposed — like the box `~`, single recursion is the surface. Errors carry the node path the same way (`at $.signals[0].in[1]: …`).

The **sample rate** enters the graph through `fconst`, not as a baked number: `{"op": "fconst", "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}` is the runtime constant behind Faust's `ma.SR`, resolved when the def is instantiated, so a def stays in tune at whatever rate the engine (or NRT renderer) runs. `ma.SR` itself is that value clamped to `[1, 192000]`; the Python client wraps the whole thing as `signals.sr()`. (`ma.PI`, by contrast, is a plain numeric literal — no `fconst` needed.)

Example — a one-pole lowpass `y = (1-a)·x + a·y'` reading audio input 0, the explicit-feedback idiom:

```json
{"signals": [{"op": "recursion", "in": [
  {"op": "add", "in": [
    {"op": "mul", "in": [
      {"op": "sub", "in": [1.0, {"op": "hslider", "label": "a",
        "init": 0.9, "min": 0.0, "max": 0.999, "step": 0.001}]},
      {"op": "input", "index": 0}]},
    {"op": "mul", "in": [
      {"op": "hslider", "label": "a",
        "init": 0.9, "min": 0.0, "max": 0.999, "step": 0.001},
      {"op": "self"}]}]}]}]}
```

## GraphDef (`/def_send graph`, `/graph_new`) — node-graph programs

A **GraphDef** is a third kind of persistent def. Where a SynthDef/FaustDef stores one synthesis node, a GraphDef stores a whole **configuration of member nodes wired by buses** — an effect chain, a mixer, a layered instrument — instantiated as one unit. It exposes a **named parameter surface**: ports that map to inner member controls (with optional scaling), so the running instance is driven through the port names, never the private member node ids. A GraphDef instantiates entirely into primitives the server already has (a group, member `/synth_new`s, `/node_map` wiring), so nothing new touches the audio thread.

`/def_send graph <blob|string>` loads a GraphDef from a JSON spec: it validates the structure (cheap — no JIT) and stores it, replying `/done`/`/fail` like the other def commands and persisting it when a data directory is configured. `/def_free name...` removes it (and SynthDefs/FaustDefs of the same name).

```json
{
  "name": "chain",
  "buses": [{"name": "mix", "rate": "audio", "channels": 1}],
  "members": [
    {"def": "tone", "controls": {"out": "mix", "freq": 220.0}},
    {"def": "amp",  "controls": {"in": "mix", "out": "OUT"}, "maps": {"level": "lfo"}}
  ],
  "surface": {"gain": [{"member": 1, "control": "level", "mul": 1.0, "add": 0.0}]},
  "defaults": {"gain": 0.5}
}
```

- `members` — each references an existing SynthDef **or** FaustDef by `def` (resolved at instantiation, both kinds identically), with initial `controls`. A control **value** that is a number is a literal; a **string** names an internal bus to wire that control to (its bus-selecting control — `out`/`in` on a Faust def, or whatever control feeds an `Out`/`In` UGen). The reserved string `"OUT"` wires to hardware bus 0. A member with `"voice": true` is a **per-voice** member (see below); the default (`false`) is a **shared** member.
- `buses` — internal buses, **private to each instance** (`rate` `"audio"` or `"control"`, `channels` default 1). They are allocated per instantiation from a reserved range at the **top of the bus space** — the top 32 audio buses and top 128 control buses (so `96..128` and `16256..16384` at the default counts, shifting with `--audio-buses`/`--control-buses`) — so they never collide with client-allocated buses (the same idea as the reserved MIDI/auto node-id ranges). The reserved widths are constants of the shared core (`clausters_core::registry::GRAPH_AUDIO_BUS_RESERVED`/`GRAPH_CONTROL_BUS_RESERVED`, exposed over the core FFI), so a client's bus allocators subtract the same reservation the server applies instead of hardcoding it. Two instances of one GraphDef get disjoint buses, and a freed instance's buses return to the pool.
- `maps` (per member, optional) — binds a member control to an internal **control** bus via `/node_map`.
- `surface` — the named ports. Each maps to a list of `{member, control}` targets, with optional `mul`/`add` linear scaling of the incoming value. One port may drive several inner controls, each scaled differently (e.g. a `freq` port playing a detuned pair). This is the difference from a bare group `/node_set`, which can only broadcast one value to controls that happen to share a name.
- `defaults` — surface-port values applied at instantiation, overridable per instance.

`/graph_new name id addAction targetID [port value]...` instantiates a GraphDef: it creates an **auto-sorted group** (the member execution order follows the bus connections) at `id` (or `-1` for a server-assigned id), holding the **shared** members, then applies the shared `defaults` and the given `port value` overrides. `/node_set id port value...` on that group id resolves the port names against the surface (never the member ids); a port absent from the surface is ignored. `/node_free id` (or `/group_deepFree`) tears the instance down (with all its voices) and reclaims its private buses. Instantiation is atomic: a missing member def or bus shortfall fails with no partial instance. GraphDefs work in NRT scores too (scored like any def at time 0).

### Shared vs per-voice, and `/graph_newVoice`

A GraphDef splits into a **shared** part (members without `voice`) and a **per-voice** part (members with `"voice": true`) — the model of a polyphonic instrument: the shared part (the private bus, a mixer, effects) exists once; each note adds a voice. A surface port maps either to shared members or to voice members, never a mix (a `/fail` at `/def_send graph` otherwise): shared ports apply to the instance, voice ports to each voice.

`/graph_new` instantiates only the shared members. `/graph_newVoice instanceID id [port value]...` then spawns the per-voice members as a **sub-group at the head of the instance** (the auto-sort orders it before the shared mixer that reads its bus), wired to the same private buses, applying the voice-port `defaults` and overrides. `/node_set voiceID port value...` resolves against the voice's surface; `/node_free voiceID` frees just that voice. A `/graph_newVoice` on an instance whose def has no voice members (or on an unknown instance) `/fail`s.

A **GraphDef can be bound to MIDI** exactly like a SynthDef/FaustDef: `/midi_bind channel graphname [target addAction gate]` spawns the shared instance once at bind time and every note becomes a `/graph_newVoice` into it (note → `freq` port, velocity → `amp` port, note-off frees the voice or, for a gate-aware binding, sets its `gate` port to 0). The GraphDef must have per-voice members. `/midi_unbind` frees the shared instance (and all its voices).

## MIDI control protocol (standard channel-voice actuation)

Besides OSC, the server can be driven by **standard channel-voice MIDI** — note on/off, velocity, aftertouch, pitch-bend, control change, program change. This is the **primary** MIDI path: a note actuates a synthesis node and an expressive message sets a named control, exactly the surface a sequencer or DAW already speaks. (SysEx, when it lands, is reserved for the non-musical control plane — def load, buffers, topology — and is never a tunnel for OSC commands.)

A channel must first be **bound** to an instrument. Binding and mapping are OSC commands (so they ride the same reliable path as the rest of the protocol); the note/control events themselves arrive over the OS's standard MIDI. Start the server with `--midi [name]` (RT only) to open a **virtual ALSA input port** (default name `clausters`) — the same system MIDI any controller or DAW uses; route anything into it (`aconnect`, a keyboard through the kernel, a DAW). Live input is MIDI 1.0 (7-bit, widened internally to the high-resolution form); the full MIDI 2.0/UMP resolution is preserved on the client's persistence path (the `clausters-midi` crate's MIDI 2.0 clip file). Network MIDI is a separate, deliberately out-of-scope idea.

- `/midi_bind channel instrument [target] [addAction] [gate]`: bind a MIDI channel (`0`–`255`: the classic 16 plus the extended UMP group×channel space) to an **instrument def** — a SynthDef, a FaustDef, **or a GraphDef**, actuated identically. Default target is the root group (`0`), default add-action `0` (head). `gate` non-zero marks the def gate-aware (see note off below). The default control map is `freq`/`amp`, matching the client `Event` convention. A **GraphDef** instrument spawns its shared instance at bind time and turns each note into a `/graph_newVoice` (see the GraphDef section); it must have per-voice members.
- `/midi_unbind channel`: remove the binding and free every voice still sounding on that channel.
- `/midi_map channel selector name`: route a message type to a control. Selectors: `note` (→ frequency control, default `freq`), `vel` (→ amplitude, default `amp`), `gate` (gate control name), `bend` (pitch-bend → control), `pressure` (channel aftertouch → control), `poly` (per-note aftertouch → the note's voice), `ccN` (control change number `N` → control), `progN` (program `N` → an instrument def `name` to switch to).

Actuation semantics, per message type (each with its named conversion):

- **Note on** → `/synth_new instrument <voiceID> addAction target freq <midi2freq(note)> amp <velocity2amp(vel)>`. Voice IDs come from the reserved MIDI range of the node-id partition (see "Node ids are a finite, boot-partitioned resource" above), disjoint from client IDs and the `/synth_new -1` auto range, and recycled as the voices die. A note-on for an already-sounding `(channel, note)` frees the old voice first. Velocity `0` is a note-off.
- **Note off** → `/node_free <voiceID>`, or `/node_set <voiceID> <gateControl> 0` when the binding is gate-aware.
- **Poly aftertouch** → `/node_set` on that note's voice; **channel aftertouch**, **control change**, **pitch-bend** → `/node_set` on every live voice of the channel, for the mapped control. Unmapped expressive messages and unbound channels are silently ignored (a running MIDI stream never errors).
- **Program change** → re-selects the channel's instrument def from its `prog`-mapped table (no-op if unset).

Conversions (`note → midi2freq`, `velocity → velocity2amp`, `aftertouch`, `bend`, `cc`, `program`) take **MIDI 2.0 / UMP resolution** (16-bit velocity, 32-bit controllers/pressure/bend) and produce the `f32` a control zone wants — no 7-bit quantization. **MIDI 1.0 is backward-compatible**: classic 7/14-bit input is accepted and widened up to those, so the same controls are driven either way. Because a MIDI voice is realized as the *same* `/synth_new`/`/node_set`/`/node_free` an OSC client would send, it is byte-identical to the OSC equivalent (`tests/midi.rs` guards this).

## Node tree introspection

The node tree is delivered to clients as **structured replies** — never scraped from the server's logs. Three queries (all answered by the network-side tree mirror, so they never touch the audio thread):

- **`/group_queryTree <groupID> [detail]`** → `/group_queryTree.reply` — the whole subtree from `groupID` (use `0` for the root). Args: `detail`, the group, its child count and its name, then depth-first per node: ID and child count (`-1` marks a synth) and then a name — the group's own (empty when unnamed) or the synth's def name — and per level the same payload `/node_query.reply` carries. Every node reads `ID, count, name` — one shape for both kinds, rather than a name only where it is new. `detail` is scsynth's `flag` widened into a level: **0** the shape alone, **1** the control count followed by (name|index, value) pairs (both scsynth-compatible), **2** also the maps and the inferred bus lists — which is what makes every entry a **full node record**, so a client reading a tree needs no follow-up `/node_query`. Values above 2 clamp to 2.
- **`/node_query <nodeID>...`** → one `/node_query.reply` per node — the same record for one node, plus where it sits. Layout: `nodeID, parentID, prevID, nextID, isGroup`; then for a **group** `headID, tailID` (`-1` if empty) and its `/group_name` (empty when unnamed); for a **synth** `defName`, control count + (name|index, value) pairs, map count + (controlIndex, bus, audio) triples (the `/node_map`/`/node_mapAudio` bindings), and the inferred `reads`/`writes` bus lists as two strings (`"0,16"`, or `"-"` when none). Siblings are `-1` when absent.
  - A node the server does **not** hold answers `nodeID, -1, -1, -1, -1` — **`isGroup` = -1 means the node is gone** — rather than `/fail`. Absence is a *state* of the resource, reported in the record like `/def_query`'s empty family and `/buffer_query`'s `frames = -1`, so one dead id never aborts a query about the others. Only a malformed request (a non-int id) is a `/fail`.
- **`/group_dumpGraph <groupID>`** → `/group_dumpGraph.reply [groupID, text]` — a human-readable rendering of the inferred bus graph (what each child reads/writes and the current order), with each group's name quoted next to its ID. A debugging aid; for machine use prefer `/group_queryTree`/`/node_query`.

The tree also **pushes**: a `/server_notify` client gets `/node_start` and `/node_end` as `nodeID, parentID, prevID, nextID, isGroup, name` (siblings are `-1` — this side does not track them; `name` is the group's `/group_name`, empty for a synth or an unnamed group). One shape for both kinds, so a client reads the record the same way whichever node it is about.

The clients ask at whichever end the question belongs to: the **server** is asked about every node it holds — `Server.query_tree()` / `queryTree()`, which returns a tree of node records and prints itself indented — and a **node** is asked about itself, `node.info()` in both clients. `Server.dump_graph()` / `dumpGraph()` returns the debug string.

## Def, buffer and UGen introspection

Where the queries above read what is *playing*, these three read what the server **holds** — the samples a client can build with. All are retrieval only: they add no semantics and change nothing.

They exist because the answers are not derivable client-side. The def store **persists across restarts**, so a running server may hold defs no client in this process ever sent; the buffer pool likewise outlives any one client; and the UGen catalog depends on how the server was built. A tool that offers the user a palette of what can be instantiated has to ask.

Each of the two multi-reply queries sends **one reply message per item** — the payloads are variable-length, so an aggregate would nest badly and a large catalog would outgrow a UDP datagram — and closes the batch with `/done "<command>"`, which is how a client with no argument knows the listing ended.

- **`/def_query [name...]`** → one `/def_query.reply` per def, then `/done "/def_query"`. With no argument, every loaded def of every family. Layout: `name, family, numControls`, then per control `name, default, rate`.
  - `family` is `"synth"`, `"faust"` or `"graph"`; a def the server does not hold comes back with an **empty** family and no controls, rather than failing the batch (the `/buffer_query` convention for an unallocated slot).
  - `rate` is the control type the def declared — `kr`, `tr` or `ir`, the same set the [control-type table](#control-types) defines, reported under the same field name `/def_send synth` accepts it under. Note this is a **different vocabulary** from the calculation rates `/ugen_query` reports below: a control is a value pushed in from outside, so `ar` is not among them (map an audio bus with `In` + `/node_mapAudio` instead) and neither is `dr` (a demand value is *pulled* by a driver — there is nowhere for a `/node_set` to land). `tr` in turn is a behavior rather than a rate, which is why the server's own type is named `ControlType`.
  - A **faust** def appends `min, max, step` to each entry (its params carry a declared range). The reserved `out`/`in` bus controls are engine plumbing and are not reported.
  - A **graph** def reports its surface **ports** instead of controls: after the shared triple, each port carries `numTargets` and then per target `member, control, mul, add` — the inner controls it drives and the linear scaling it applies, so a patcher can draw the port's real connections.
- **`/buffer_query`** with **no argument** → one `/buffer_query.reply` listing every **allocated** buffer, four args each (`bufnum, frames, channels, sampleRate`) — the same shape the per-index form replies with, so one parser reads both. (With arguments it is the per-index query documented under [Buffers](#buffers-buffer_).) The per-index form answers **`frames = -1`** for a slot with nothing in it, the same "absent is a state" rule `/node_query` and `/def_query` follow; the listing form simply omits it.
- **`/ugen_query [kind...]`** → one `/ugen_query.reply` per UGen, then `/done "/ugen_query"`. With no argument, the whole catalog. Layout: `name, arity, defaultRate, rates, exec, bus, needsPath, opFamily, spectral, numInputs`, then per input `name, default`.
  - `arity` is the input count, or `-1` for a **variadic** kind — whose named inputs are then its **fixed head** only (`EnvGen`'s five before the envelope array, `Dseq`'s `repeats`).
  - `rates` is a comma list (`"kr,ar"`) of the rates the kind may be instantiated at; `defaultRate` is the one a def gets by omitting `rate`.
  - `exec`, `bus`, `opFamily` and `spectral` expose the compiler's own classification (`normal`/`local_in`/`local_out`/`demand_driver`/`spectral`; `read`/`write`/`read_write`; `unary`/`binary`; `source`/`filter`/`filter2`/`sink`), each an empty string when it does not apply.
  - The input **names are descriptive**: the wire stays positional and no input is ever addressed by name. They are what a palette labels an inlet with, and `default` what to offer when the user leaves a slot alone — the server applies no default of its own, a def that omits an input is simply short and fails on arity.
  - An unknown kind comes back with an empty rate set and no inputs. A server built **without the `synth` feature** has no UGen catalog at all and replies with an empty listing (not a failure) — its defs would all be FaustDefs, and the Faust box vocabulary is Faust's own, held client-side rather than published here.

The Python client wraps these as `Server.query_defs()`, `Server.query_buffers()` and `Server.query_ugens()`, returning `DefInfo`/`BufferInfo`/`UgenInfo` records — the web client as `queryDefs()`, `queryBuffers()` and `queryUgens()`, returning the same records in camelCase. `examples/introspect_server.py` walks all three.

## Server logging and verbosity

The server logs to **stderr** through `tracing`, at five levels (`error`, `warn`, `info`, `debug`, `trace`). The startup banner and the NRT render summary go to **stdout** (they are program output, not logs). The **audio thread never logs**: it reports conditions over the lock-free FIFOs and the network thread emits them, so logging never breaks real-time safety.

The level is set, in increasing precedence, by:

- the CLI flags `-v` (info), `-vv` (debug), `-vvv` (trace), `-q` (errors only); default is `warn`;
- the `RUST_LOG` environment variable, an [`EnvFilter`](https://docs.rs/tracing-subscriber) directive that also filters per module — e.g. `RUST_LOG=clausters::osc=trace` to see only OSC traffic;
- at runtime, **from a client**, with two OSC commands (both reply `/done`):
  - `/server_verbosity <int|string>` — an int level (`-1` errors … `3` trace) or an `EnvFilter` directive string. Lets a client retune the server's logs without restarting.
  - `/server_dumpOsc <flag>` — toggles the OSC-traffic dump (the `clausters::osc` trace target). Unlike scsynth, this is **not** an ad-hoc console print: it routes through the same logging system, controllable by `/server_verbosity`/`RUST_LOG`, on stderr.
  - `/server_errorMode <mode>` — `1` posts command failures to the server console (default), `0` silences them. The `/fail` **OSC reply is always sent** regardless (clients rely on it); `/server_errorMode` only gates the server-side console logging. scsynth's bundle-local `-1`/`-2` forms are not separately supported — the persistent `0`/`1` toggle is the model that fits our logging (a deliberate deviation).

Note that these control the **server's own** logs (on the server's stderr); the **node tree** is delivered to clients as structured data, never scraped from logs — see [Node tree introspection](#node-tree-introspection) above (`/group_queryTree`, `/node_query`, `/group_dumpGraph`).

## Server and UGen commands (`/server_cmd`, `/node_ugenCmd`)

Two extension commands carry out-of-band instructions that are neither node nor bus state. Both are **typed and discoverable** — the deliberate replacement for scsynth's untyped `/server_cmd`/`/node_ugenCmd` argument blobs (a command *name* plus validated typed args, errors naming the offending field, like `compile`).

- `/server_cmd <name> args...` — a **server-wide** command. `name` selects a handler; the built-in `ping` replies `/done /server_cmd ping` (it proves the surface). An unknown name replies `/fail /server_cmd "unknown server command …"`. New server commands register here.
- `/node_ugenCmd <nodeID> <ugenIndex> <name> args...` — a command addressed to **one UGen instance** inside a synth. It validates the node is a UGen synth and `ugenIndex` is in range (a Faust synth is one opaque block, not a UGen graph, so it is rejected), packs the numeric args inline (up to 8, so nothing heap-allocated crosses to the audio thread), and routes them to that UGen on the audio thread; the command name is hashed to a stable selector both sides agree on. The first real consumer is the FFT chain: `/node_ugenCmd <nodeID> <ugenIndex> window <wintype>` swaps an `FFT`/`IFFT`'s analysis/synthesis window live (see the FFT-chain note above). A UGen that does not recognize the command name ignores it, so a `/node_ugenCmd` to a valid target is otherwise accepted silently.

## Persisting defs across restarts

The real-time server can persist loaded defs to a data directory and reload them automatically on the next start, so a client need not re-send its instrument library every session. It is **on by default**; control it with two flags:

```text
clausters --data-dir <dir>   # where defs are stored/reloaded
clausters --no-persist       # disable for this run
clausters --prune-defs       # drop the ones that no longer load
```

With no `--data-dir`, the directory is `$CLAUSTERS_DATA_DIR` if set, else `$XDG_DATA_HOME/clausters`, else `~/.local/share/clausters`. Persistence applies to the real-time server only; offline `--nrt` renders never read or write it.

A client can also load defs **on demand** from an arbitrary path, complementing the boot-time reload: `/def_load <path>` loads one SynthDef spec file (the Clausters def format — the same `SynthDefSpec` JSON `/def_send synth` carries), and `/def_loadDir <dir>` loads every `*.json` SynthDef in a directory (a single unreadable/invalid file fails the whole command, naming it). Both compile through the `/def_send synth` path (so the def is also persisted under its name) and reply `/done`. GraphDefs load through `/def_send graph`, Faust defs through `/def_send faust`.

The def kinds live in subdirectories of a `defs/` directory (so the data
directory itself is free for other persistent aspects); `midi.json` and
`boot.json` sit at the top level:

| path | written on | content |
|---|---|---|
| `<dir>/defs/synthdefs/<name>.json` | `/def_send synth` | the `SynthDefSpec` JSON, verbatim |
| `<dir>/defs/faustdefs/<name>.json` | `/def_send faust` | a record: the original Faust source/JSON, the libfaust version, and the payload's SHA-256 |
| `<dir>/defs/faustdefs/<name>.<sha>.bc` | `/def_send faust` | the compiled LLVM **bitcode** (a speed cache) |
| `<dir>/defs/graphdefs/<name>.json` | `/def_send graph` | the `GraphDefSpec` JSON, verbatim |
| `<dir>/midi.json` | `/midi_bind`/`/midi_unbind`/`/midi_map` | the MIDI bindings (channel → instrument + target + control map) |
| `<dir>/boot.json` | *authored by the user/client* | the boot preset: standalone GraphDefs to instantiate at startup |

GraphDefs reload after the synth/faust defs (their members reference those names); validation is structural only, so a member def that is still missing at load is caught later, at `/graph_new`.

**A def that no longer loads is named, and dropped only when asked.** A UGen that gains an input makes every stored def written against the old shape uncompilable, and the reload warns and skips each one — the right call, since a def may also fail because *this build* lacks its family (a `--no-default-features` server would otherwise eat the library on sight). So the warning names the def and, once, the directory it is in, and `--prune-defs` is the one door that removes them: it drops exactly the defs that failed to load in the families the running build has.

### MIDI-standalone: bindings + boot preset

So the server can be **played from a MIDI controller with no OSC programming at all**, the MIDI bindings persist too. Every `/midi_bind`/`/midi_unbind`/`/midi_map` rewrites `midi.json`; at startup — **after** the defs and GraphDefs are in place, so a binding's instrument name resolves — each binding is re-established (a GraphDef binding re-instantiates its shared instance). The minimal workflow becomes: drop a SynthDef/FaustDef (or a GraphDef) and a binding in the data dir once, then every later `clausters --midi --data-dir <dir>` comes up already bound — connect a controller (`aconnect`) and play. The default control map (note→`freq`, velocity→`amp`, note-off→`/node_free`/gate) makes a restored binding immediately playable.

`boot.json` is an optional, user-authored **boot preset**: a JSON array of standalone GraphDefs to instantiate at boot (an always-on reverb bus, a drone, a mixer), each `{"graph": "<name>", "ports": {"<port>": value, ...}}`. They are instantiated (the equivalent of `/graph_new <name> -1 0 0 ...`) after the bindings, so a fresh boot comes up already wired. The boot order is **defs → graphdefs → bindings → boot preset**. All of it honours `--no-persist` (off → nothing read or written) and the data-dir resolution; it applies to the real-time server only (NRT never persists).

### A name identifies one def, and `tmp_` names are never stored

A def **name is unique across all three kinds**, not per kind. Sending a def under a name another kind holds *replaces* it — last one wins, and the loser's persisted files are deleted with it. Without that rule the two records both survive a restart and the reload order silently decides which one a `/synth_new` (or a `/def_query`, or the bus-usage inference the parallel scheduler reads) resolves to.

A name beginning with **`tmp_`** marks the def as **ephemeral**: the server compiles and keeps it in memory for the session but never writes it to the data directory. It is for defs a client builds to carry an expression that has no name of its own — the Python client's `as_def` names them `tmp_synthdef_<id>` / `tmp_faustdef_<id>` — which have no meaning to a later session and would otherwise accumulate forever. Anything an ephemeral def must nonetheless write (only the Faust record and its bitcode) goes to a `clausters-tmpdefs` directory under the OS temp directory, so a replayed expression still skips the recompile while the store stays clean. The prefix is the whole signal: a def *you* name `tmp_...` is ephemeral too, by that definition.

Two properties are what make a generated name safe, and a client that generates its own should have both: **globally unique** (a per-process counter restarts at 0 next run, and the store outlives the process, so the names collide with the previous session's), and **distinct per kind** (one counter feeding two kinds lets a SynthDef and a FaustDef claim one name).

The stored **definition** (the JSON) is always the source of truth: it is transparent, human-readable, and what gets recompiled. The Faust `.bc` is a non-authoritative cache — on reload the server re-creates the factory from bitcode (skipping Faust's front-end) only when the libfaust version still matches and the file is intact; otherwise it silently recompiles from the source and rewrites the cache. A libfaust upgrade therefore invalidates every `.bc` automatically. `/def_free <name>` deletes both files. Re-sending a name overwrites them.

Reloading is **incremental**: the socket starts serving immediately and the library loads in the background on the compiler thread, so a large Faust library does not delay startup — a def simply "does not exist yet" until its reload finishes.

## Generating defs programmatically

`examples/json_client.py` (Python, stdlib only) builds all three formats with a few helper functions and drives the whole lifecycle over OSC — use it as a reference client. The equivalence of the families is pinned down by the golden tests in `tests/faust_parity.rs`: a UGen graph, the box translation and the signal translation render side by side in one engine and must agree (bit-exactly for stateless arithmetic on shared input, within float tolerance for oscillators, since `Sine` accumulates phase in f64 and Faust in f32).
