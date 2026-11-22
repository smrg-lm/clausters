# Def schemas

User-facing reference: wire formats and OSC commands. For the server's internals — threads, memory lifecycle, invariants, adding UGens — see [`architecture.md`](architecture.md).

## How a command is named

Every command in this reference is spelled by one rule, and knowing it means most of the surface can be guessed rather than looked up:

**`/<resource>_<action>`** — the resource is the *kind of thing the command addresses*, spelled in full (`node`, `synth`, `group`, `bus`, `buffer`, `def`, `ugen`, `graph`, `server`, `midi`, `transport`, `sched`, `clock`, `plugin`); the action is a verb in camelCase (`set`, `free`, `query`, `allocRead`, `tapStream`).

Three corollaries carry most of the vocabulary:

- **A reply is `<command>.reply`.** Every answer the server sends back is the command's own address plus that suffix — `/node_query` → `/node_query.reply`, `/bus_get` → `/bus_get.reply`, `/server_status` → `/server_status.reply`. A stream pushes under the same address as its subscription's reply (`/bus_stream` → `/bus_stream.reply`). The exceptions are the three answers that belong to no single command: `/done` and `/fail` (which name the command in their first argument) and the node notifications (`/node_start`, `/node_end`, `/node_trigger`, `/node_fault`), which nothing asked for. One command answers under another's address on purpose: `/synth_get`/`/synth_getRange` reply `/node_set`, the very message that writes those controls back (see below).
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

### Native plugin discovery

`/plugin_scan [int rescan] [string dir ...]` asynchronously discovers CLAP and VST3 plugins on a Linux
server built with `plugin-clap`, `plugin-vst3` or both. The server uses the matching
`clausters-plugin-host` executable (adjacent to the server, or selected by
`CLAUSTERS_PLUGIN_HOST`). Without directories it searches `CLAP_PATH`, `~/.clap`,
`/usr/lib/clap` and `/usr/local/lib/clap`, plus `VST3_PATH`, `~/.vst3`,
`/usr/lib/vst3` and `/usr/local/lib/vst3`, ignoring absent default roots. Explicit
missing paths yield diagnostics. Paths name the server's filesystem.

Discovery never loads a plugin into the server: each binary is inspected in a
disposable helper, with a two-second timeout per administrative request. With
persistence enabled, a binary whose canonical path, modification time and size
(for a bundle directory, its newest file and total size) match the cache
written by the same helper version is **not inspected again**: its cached rows,
diagnostics included, are reported. A leading nonzero `rescan` int inspects
every binary.
Compatibility is checked again when an instance is loaded. A failed binary
returns an error row and other binaries continue. Traversal/catalogue capacity
errors, an unavailable helper, a full scan queue or a cache-write failure reply
`/fail "/plugin_scan" <reason>`. Builds without native plugin support and an in-page
engine return a capability error. Scanning does not create nodes or send defs.

Each `/plugin_scan.reply` has five arguments:
`<int entry_index> <int entry_count> <int chunk_index> <int chunk_count> <blob utf8_json>`.
One manifest has `entry_index = -1`, the total entry count, zero chunk indices
and an empty blob. Each subsequent entry is serialized as JSON, split into
chunks of at most 6144 bytes. Entry and chunk indices start at zero; clients
reassemble by indices and reject inconsistent, duplicated or missing fragments,
including a wholly missing entry. `/done "/plugin_scan"` ends the batch. TCP or
WebSocket is recommended for large catalogues; UDP/shared-ring delivery can
lose a burst, which clients report as incomplete rather than return a partial
catalogue. `/server_sync` waits for the scan and its cache write.

The reconstructed rows use these schemas:

```json
{"kind":"plugin","plugin":{"format":"clap","path":"/plugins/gain.clap","id":"org.vendor.gain","name":"Gain","vendor":"Vendor","version":"1","description":"A gain","url":"https://vendor.example","manual_url":"","support_url":"","features":["audio-effect","utility","stereo"],"sdk_version":"1.2.2","inputs":[{"id":10,"name":"Input","channels":2,"flags":1,"main":true,"layout":"stereo","active":true}],"outputs":[{"id":20,"name":"Output","channels":2,"flags":1,"main":true,"layout":"stereo","active":true}],"note_inputs":[],"note_outputs":[],"parameters":[{"id":19,"name":"Gain","module":"","flags":32,"min":0.0,"max":1.0,"step":0.0,"default":0.5}],"latency":0,"state":true,"editor":false}}
{"kind":"error","path":"/plugins/broken.clap","error":"plugin helper timeout/protocol error"}
```

**Everything the formats declare is reported.** `description`, `url`,
`manual_url` and `support_url` come from a CLAP descriptor; a VST3 class has
none of the first and third, reports its factory's address as `url` and its
e-mail as `support_url`. `features` is what the plugin declares itself to be,
as declared: CLAP's descriptor features, or VST3's class subcategories split at
`|` (`["Instrument","Piano"]`). `sdk_version` is the interface version it was
built against. An audio port's `main` marks the main port of its direction
(CLAP's main flag, VST3's main bus type) against auxiliary ones; `layout` is
its declared channel layout (`mono`, `stereo`, `surround`, `ambisonic`, a CLAP
port type) or empty; `active` is whether the plugin wants it active by default
(VST3's flag; always true for CLAP). `flags` stays in the format's own
vocabulary. A note port also carries `preferred`, the dialect bit it prefers
(zero when none), and `channels` (a VST3 event bus's count; 16 for CLAP). A
parameter's `module` is CLAP's module path or the VST3 unit path, `/`-joined.
Fields added later default when absent, so a reader takes older rows. Data a
plugin loads from outside (an impulse response, samples) has no standard
declaration in either format; it lives in the opaque state.

**The clients read rows into one vocabulary**, through the shared core
(`clausters_core::plugin_catalog`): a `kind` (`instrument`, `effect`,
`note_effect`, `analyzer`, `other`; from the declared category, else from the
ports), `tags` (the common words both vocabularies map to: `synth`, `sampler`,
`piano`, `drum`, `reverb`, `delay`, `eq`, `dynamics`, `distortion`,
`modulation`, `filter`, `pitch`, `spatial`, `utility`, ...), `channels` and the
control aliases below. That reading is the clients' and the core's; the wire
carries the declarations.

VST3 candidates can be standalone `.vst3` binaries or Linux bundles with a
native module under `Contents/<architecture>-linux/`. Discovery reports only
`Audio Module Class` entries, with a 32-digit hexadecimal class ID. Use the
returned path and ID in a `PluginDef`; the format is `vst3`. Build the matching
helper with `cargo build -p clausters-plugin-host --no-default-features
--features vst3` (or `--features clap,vst3` for both). A missing adapter returns
a per-candidate diagnostic. VST3 component and controller state are stored
together in an opaque host image; preserve those bytes without interpreting them.

Identity is `(format, canonical binary path, native plugin id)`; a binary may
hold several plugins. Native port and parameter ids survive discovery order;
names are display strings. Port arrays follow processing-port order. Note ports
carry `id`, `name` and native `dialects` flags. Parameter values and bounds use
adapter-native units: CLAP plain values, VST3 normalized values in `[0, 1]`.
`step` is the quantization interval (zero for continuous parameters); `flags`
use CLAP-compatible stepped/read-only/automatable bits for either adapter. `latency` is the intrinsic
frame count queried after activation at 48 kHz, independent of the bridge delay.
`state` reports opaque save/load support; `editor` reports native X11 editor
support, without opening a window.

Limits are 32 roots, 128 binaries, 512 plugins per binary, 16 ports and 32 audio
channels per direction, 8192 parameters per plugin and a 16 MiB catalogue.
With persistence enabled the server atomically writes
`<data_dir>/plugins.json` as `{"version":2,"helper":<package version>,
"binaries":[{"path","modified","size","entries":[...]}]}`: every binary it has
inspected, diagnostics included, merged across scans and pruned of binaries no
longer on disk. A file of another version or helper is read as empty. With
persistence disabled there is no cache and every binary is inspected.
`Server.plugins(*dirs, rescan=..., timeout=...)` and `await
server.plugins(dirs, { rescan, timeout })` return a catalog of these rows, and
`scan_plugins`/`scanPlugins` the rows themselves; choose a timeout for the
whole catalogue rather than one binary.

### Native plugin definitions and recovery

`/def_send "plugin" <name:string> <reference:JSON-string-or-blob>` probes and
validates a native recipe asynchronously. Success is `/done "/def_send"
"plugin" <name>`; failure is `/fail "/def_send" <reason>`. `/server_sync`
waits for earlier plugin submissions, scans and restart preparation. A
superseded submission fails rather than replacing a newer name claim.

```json
{"format":"clap","path":"/plugins/gain.clap","id":"org.vendor.gain","inputs":[{"id":10,"channels":2,"bus":4}],"outputs":[{"id":20,"channels":2,"bus":0}],"state":[],"depth":4}
```

The path names the binary on the server filesystem; `id` selects one native
plugin in that binary. Every audio port must occur exactly once in its
respective direction, with its native `id` and matching `channels`. Rows may
be in any order. `bus` defaults to zero; the port reads or sums its consecutive
channels there. A **negative** `bus` leaves the port unconnected: an input
reads silence and an output writes nowhere. The clients build references
through the shared core, where a bus number routes the main port and leaves
auxiliary ones unconnected. This includes sidechains and additional outputs, without a
separate addressing rule. Unknown fields and incompatible layouts fail. The
optional `state` is an opaque snapshot as JSON byte integers (at most 1 MiB).
An omitted or empty initial `state` uses the plugin defaults; live state commands
can still round-trip a valid empty image.
`depth` is the minimum pipeline delay, 1–96 engine blocks, default four. A live instance raises it to cover the longest device callback the server has run (a 2048-frame callback is 32 blocks), because the blocks of one callback run back to back; `/synth_new` and `/plugin_restart` read it when they build the instance. Offline rendering uses `depth` as written.

Instantiate with `/synth_new` and control with `/node_set`, `/node_map` and
`/node_mapAudio`. Parameter names are `param:<native-id>`, in **adapter-native
units** (CLAP plain values, VST3 normalized `[0, 1]` values), represented at the server's ordinary float control precision. Bus
names are `in:<native-port-id>` and `out:<native-port-id>`. Native IDs, never
display labels, identify controls. **Each control also answers to an alias**
read from its port or parameter: the main audio input and output are `in` and
`out` (the one flagged main, else the first), another port
`in_<name>`/`out_<name>`, a parameter its name; a name is lowercased, every
other run of characters one `_`, a leading digit prefixed by `p`. An alias two
controls would share is nobody's: a port's wins over a parameter's, and one
claimed twice within ports or within parameters is dropped for all claimants.
Queries report the native-id names. Numeric control indices refer to the
reported control order: parameters sorted by native ID, then input ports,
then output ports. `/def_query.reply` uses family `plugin`; each control is
the record every family reports, with the parameter's own name as its label,
its module as its group, its flags and, for a parameter the format marks as a
list, the names of its steps; a port's group is `Buses` and its minimum -1,
unconnected. A stepped CLAP parameter's step is one; a stepped VST3 parameter's step is
`1 / stepCount`. Continuous parameters have step zero. Bus controls have integer steps.
Read-only parameters cannot be written. Out-of-range parameters or exceeded
block capacities produce silence and a per-node fault, rather than clipping
or losing automation without a report.

Records in `defs/plugindefs` contain the original `name` and `reference`,
including initial state, and the `surface` the probe found: the plugin's
metadata and its control defaults. Binaries do not. Restoring a record at boot
loads no plugin; the first node of the def loads it and fails if the binary no
longer has the stored ports and parameters. A record without a `surface` is
probed once at boot and rewritten with one.
Unavailable records remain on disk unless `--prune-defs` is requested. A live
node retains its recipe after `/def_free` or replacement. GraphDef members
can name plugin defs and wire their bus controls like other single-node defs.

Native editors are controlled asynchronously on the helper's main thread:

| Command | Arguments | Effect |
|---|---|---|
| `/plugin_openEditor` | `<node:int>` | Open or raise the native editor. |
| `/plugin_closeEditor` | `<node:int>` | Close it and release its resources. Repeated close succeeds. |
| `/plugin_resizeEditor` | `<node:int> <width:int> <height:int>` | Resize the client area in physical pixels, subject to the plugin's constraints. Dimensions must be 1–16384. |
| `/plugin_queryEditor` | `<node:int>` | Query its current state. |

Each success is `/done <command:string> <node:int> <state:JSON-string>`, where
state is `{"open":true,"width":480,"height":280}` (closed: false and zero
sizes). Failure is `/fail <command> <reason>`. These operations participate in
`/server_sync`, use the bounded four-job plugin worker, and do not suspend
processing. They are live-native operations, unavailable in an OSC score or
in-page engine. A browser connected to a native server opens the editor on the
**server's machine**; it does not embed a plugin in the page.

The Linux helper provides a floating X11/XWayland parent for an embedded editor;
a CLAP plugin may instead manage its own floating window. Fixed-size editors
and plugin-managed floating windows reject host resizing. Missing displays,
missing native editors and plugins rejecting attachment report errors. Closing
the window releases the editor without freeing the node. Free, helper failure,
restart and state-generation replacement close it; a replacement starts closed.

Editor/output changes produce `/plugin_edit <node:int> <native-param-id:int>
<action:string> <value:float>` for `/server_notify` subscribers. Actions are
`parameter`, `modulation`, `begin` and `end`; gesture values are zero. Native
parameter IDs use their 32-bit bit pattern in the OSC int. Parameter values use
the adapter-native domain and the server's ordinary float precision. Global
parameter edits update `/synth_get` and `/synth_getRange` queries and current
controls used by recovery. Per-note changes are not global control edits. A
bus map stays authoritative and is reasserted after an editor edit. Feedback
from a retired helper generation cannot update a reused node ID.

The helper must match both the server package version and its administrative
and audio protocols. The Linux server archive and Python wheel include it.
Standalone lookup checks beside the executable; an embedded library checks
beside itself, the Python `_libs`/`_bin` pair and a prefix's `lib`/`bin` pair.
`CLAUSTERS_PLUGIN_HOST` is the explicit development override. A missing or
incompatible helper reports an error before loading third-party code.

A plugin def keeps one **spare instance** running: `/def_send` starts it, and
each `/synth_new` takes it and starts the next, so creating a node hands over a
helper that has already loaded the plugin rather than loading it on the
server's network thread. A burst of `/synth_new` faster than spares are built
loads the rest synchronously. Plugin requests run on three workers -- discovery,
loading (def, spare, restart, state load) and control (editor, state capture)
-- so an editor never waits behind a scan or a slow load. A plugin may take up
to 30 s to load; an administrative request answers within 2 s.

`/plugin_restart <node-id:int>` prepares a fresh helper asynchronously; `/done
"/plugin_restart" <node-id>` acknowledges preparation and queuing the swap,
or `/fail` reports a non-plugin/missing node, full queue or load error. The
swap occurs at a block boundary, keeps tree identity, restores the node's last successfully captured or loaded state
(or the recipe's initial state before its first state operation) plus the actual current controls and maps, and clears held
notes and unsaved DSP history. A generation check prevents a freed/reused ID
from receiving an old restart. Faults use `/node_fault <id> "Plugin" <code>
<reason>` for notification subscribers: 1 capacity (a bounded queue or
event list overflowed), 2 starved (eight results in a row missed their block:
the helper died or hung, or the device callback grew past the pipeline), 3
processing, 4 protocol (an invalid event, a changed surface, or a restart the
plugin asks for because its buses, latency, MIDI controller map or -- in CLAP --
whole parameter list changed; new parameter values, titles or a VST3 parameter
ID remap table are absorbed live). A burst of parameter changes waits for room in later
blocks, and a burst of releases is deferred, rather than either failing. Recovery is explicit; nodes never restart by themselves.

`/plugin_saveState <node-id:int>` asynchronously captures opaque state from a
live node, **while it keeps playing**: the running helper saves on its main
thread and nothing is swapped, so held notes and DSP history survive. Replies are `/plugin_saveState.reply <node:int> <chunk:int>
<chunks:int> <blob>` followed by `/done "/plugin_saveState" <node>`. Fragments
are zero-based, at most 6144 bytes each; an empty image has one empty fragment.
The image limit is 1 MiB. Clients must reject missing, repeated, inconsistent
or out-of-range fragments. Use one outstanding state capture per connection,
and TCP or IPC for loading images larger than a UDP datagram.

`/plugin_loadState <node-id:int> <blob>` asynchronously installs an opaque image;
`/done "/plugin_loadState" <node>` acknowledges preparation and queuing the
swap. Both state commands participate in `/server_sync`. They are live
operations; offline renders use the reference's initial `state` bytes instead.
A live node's image is independent of its def and every other instance; it is
kept for explicit recovery, without modifying the persisted def. Save the
returned bytes yourself to retain a live image across server boots.

A load suspends the node at a block boundary and prepares a fresh helper with
the state loaded **before activation**. The node is silent during preparation;
the new bridge clears held notes, queued events and unsaved DSP history.
Routing, maps and tree identity survive. A load replaces unmapped parameter
values; existing maps continue to drive the loaded parameters. A capture
changes no control. On a load, parameter ranges, flags, display names, port labels and intrinsic latency are
refreshed per node. Audio port IDs/order/channel counts and control IDs must
remain compatible with the live routing; an incompatible image fails rather
than changing a running graph's bus ownership. The shared def stays unchanged.

`/fail` names the state command and reports a missing/non-plugin node, absent
state extension, invalid arguments, the state size limit, a full worker queue,
an overlapping operation, timeout, plugin rejection or incompatible surface.
An accepted transaction that fails leaves the old generation silent until
`/plugin_restart`; recovery uses the last successful image and current
controls. A freed/reused node ID cannot receive an old operation's result.

Latency is `depth * 64 + native latency` samples. The general server
compensation pass aligns admitted automatic bus paths across def families
(see [automatic ordering](auto-order.md#server-delay-compensation)). Native hosting requires a Linux server built with `plugin-clap` or `plugin-vst3`
and its matching `clausters-plugin-host` binary. The in-page engine reports a
capability error.

## The async barrier (`/server_sync`)

`/server_sync <int id>` is the general way to wait for asynchronous work: the server replies `/server_sync.reply <id>` once **every async command received before this `/server_sync`** has completed — Faust compiles (`/def_send faust`), SynthDef sends (`/def_send synth`), buffer jobs (`/buffer_*`) and plugin discovery (`/plugin_scan`), plugin defs, recovery, native editor and live state operations, each of which runs on its own FIFO worker thread. Use it instead of matching individual `/done`s when you fire several async commands and just want to know they have all landed (e.g. after sending a def without waiting, before the `/synth_new` that needs it). It is a real barrier, not a round-trip of `/server_status`: a `/server_sync.reply` guarantees the prior compiles/jobs are installed.

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
/node_setText id ctl text...                               # set controls from text
/synth_getText id ctl...                 -> /node_setText id (ctl text)...     # read them as text
/node_free id...
/node_trace id...                                          # log a node's state (debug)
/synth_forgetId id...                                           # acknowledged (see note)
/def_free name...                                         # SynthDef JSON only
```

Engine facts that apply to every def: blocks of 64 samples; **by default** 1024 audio buses (`0..outputs` are the hardware outputs, bus 0 = left; `outputs..outputs+inputs` are the hardware inputs) and 16384 control buses; a pool of 4096 sample buffers filled by the `/buffer_*` commands; all signals are `f32` at the configured sample rate. Everything sized at boot, never at compile time: the bus counts by `--audio-buses` and `--control-buses` (each a power of two, with no ceiling in the code: what a bus costs is memory plus the per-block clear, measured in `crates/clausters-server/tests/bus_scale.rs`), the sample rate by `--sample-rate` (default 48000; PipeWire honors it per-application), the hardware channel counts by `--outputs` (default: the device's) and `--inputs` (default 0), the pre-allocated pools by `--max-nodes`, `--max-buffers`, `--max-graph-children` and `--max-ugen-inputs` (≤256), the transports by `--transports` (default 8, at most 64; see the transport below), the event buses by `--event-buses` (default 64; see "Event buses" below), and the audio-tap region by `--taps` and `--tap-frames` (defaults 8 × 16384; see "Audio taps" below). Each flag also has a `[server]` config key of the same name (`max_nodes`, `outputs`, …), with the CLI winning. A client reads the live configuration with **`/server_query`** → `/server_query.reply [audio_buses, control_buses, output_channels, block_size, nominal_sr, actual_sr, input_channels, max_nodes, max_buffers, max_graph_children, max_ugen_inputs, taps, tap_frames, max_frame, max_stream_buses, transports, event_buses]`, so it can size its own bus/node allocators from the server instead of assuming the defaults. The first six fields are the stable original set; the S7 capacities, the tap region shape, the stream-transport frame ceiling (`--max-frame` — what a client should size bulk requests like `/buffer_getRange` chunks from, see "Transports" below) the `/bus_stream` bus ceiling **as it applies to the asking client's carrier**, the transport count and the event-bus count are appended, so a client that reads only the first six keeps working.

**Node ids are a finite, boot-partitioned resource.** Like every capacity above, the node-id space is fixed at boot and **every range of it recycles** — an id allocator (server- or client-side) is a *registry* of ids in use, never a counter that only climbs. The space is partitioned by one shared formula (`clausters_core::registry::NodeIdPartition`, scaled from `max_nodes`, the resource that actually bounds concurrent nodes): a **client range** starting at 1000 (width `4 × max_nodes` — table capacity plus in-flight margin), then the server's **auto range** (`/synth_new` with id `-1`, GraphDef groups and members) and its **MIDI voice range**, each `2 × max_nodes` wide. At the default `--max-nodes 8192` that is client `1000..33768`, auto `33768..50152`, MIDI `50152..66536`. A client derives the partition from the `max_nodes` field of `/server_query.reply` with the same formula (exposed by the core FFI), so client and server agree by query, not convention. Ids return to their registry when the node dies: the server releases its auto/MIDI ids on each node's end (and on an engine rejection), and a client releases its ids from the same **`/node_end`** notifications — register with `/server_notify 1` and free on `/node_end` (releasing at `/node_free`-send time would re-hand an id whose node is still alive). When the engine **rejects** a node asynchronously — the id is already a node, the target is gone, the node table or the target group is full — no `/node_end` will ever come, so the server broadcasts **`/fail "/synth_new" <message> <int id>`** to every `/server_notify` client, the message naming which of those it was — the appended id (not part of scsynth's two-argument `/fail`) is what lets a client registry reconcile the in-flight id instead of losing it. Exhausting a range is an explicit error (`/fail … out of auto node ids`, or the client's own exception), never a silent wrap; in NRT/score mode the client id space is deliberately unbounded — an offline score has no live `/node_end` stream and no real-time bound on ids over its length.

**The client range is one range, and several clients may be on it.** The partition tells a client where its ids begin; it does not tell two clients of the same server how to stay out of each other's way, and nothing on the wire does — both registries start at the same base and hand out the same first id, so the second `/synth_new` of the pair is refused as a duplicate, or worse, lands on the other client's node. Clients that share a server therefore split the range **client-side**, by taking equal slices of it in a fixed order: with `n` clients, client `i` allocates from slice `i` of `n`, the last slice taking the remainder so the range is tiled exactly. There is nothing to negotiate — the slices are disjoint by arithmetic — so two clients with no channel between them agree as long as whoever arranged them handed each its index. The same split applies to every client-side space (audio buses, control buses, buffers, and the GUI host's widget ids), taken *after* the server's own reservations, so a slice never opens a hole below itself. It costs range, not capability: a split of two halves what either client may hold live at once, so a server's only client takes the whole space and pays nothing. Both clients implement it (`clausters.base.IdShare` / `IdShare` in the web client).

**Server health** is one poll away: **`/server_status`** → `/server_status.reply [ugens, synths, groups, defs, avgCPU, peakCPU, nominal_sr, actual_sr, late_blocks, starved]` — the scsynth field set, minus its leading unused int and plus two appended ones, so the reply opens on its first real field like every other one here. `avgCPU` and `peakCPU` are the audio thread's per-block processing time as a **percentage of the block budget** (`block_size / sample_rate` of wall time — at 48 kHz a 64-frame block must be computed in 1.33 ms): the average is an exponential moving average with a ~1 s time constant, the peak is the worst single block **since the previous `/server_status` poll** (reading it resets the window, so every poll reports the peak of its own interval; expect it to sit well above the average — the callback must fit its *worst* block, not its mean). `late_blocks` counts, cumulatively since boot, the blocks whose processing exceeded their budget — the engine-side xrun proxy. It is conservative: when the device quantum is larger than one block (PipeWire typically runs 256–1024 frames), a single late block can be absorbed by the rest of the cycle, so an occasional increment is a warning, a steady climb is audible trouble (cross-check with `pw-top`'s ERR column). Both meters read the real callback only: in an offline render they just measure render speed. **A server with no clock reports none of the three**: `avgCPU`, `peakCPU` and `late_blocks` are then OSC **nil** (`N`), not zero — zero would read as an idle engine, and it cannot say what a block cost. An engine inside a page is that server (see `/server_load` below). `starved` (a long) is counted and not timed, so every server reports it: the samples a disk stream (`TransportDiskIn`) played as silence since boot because the frame it needed had not been read yet — a session outrunning its disk, said in a number rather than heard as a click. `crates/clausters-server/examples/stress.rs` builds a capacity ramp on top of this reply — see [Examples](examples.md).

**The seed** is the server's whole random state: every random UGen draws from one of a table of 64 generators (scsynth's generator, `taus88`, and scsynth's model — `RandID` chooses which one a synth draws from, `RandSeed` reseeds it), and one seed seeds the table. **`/server_seed [seed]`** reseeds it (an int, a 64-bit int or a decimal string; in a timed bundle or a score, at that sample) and answers `/server_seed.reply seed`, the seed in force as a **decimal string** since an OSC int is 32 bits; with no argument it only answers. A server boots from `--seed` or the `[server]` key `seed`, or from entropy, and prints the seed it booted with either way, as an offline render reports its own. **The same seed and the same commands at the same samples give the same samples**, in real time as offline: the values depend on the order the UGens draw in, which is the node tree's, and a `/group_parallel` group keeps it — nodes that draw from one generator never run in one stage, so a parallel render draws what a sequential one does, and nodes meant to run side by side draw from different generators (`RandID`). A seed below 2^32 seeds the first generator exactly as `RandSeed` with that number does, so a def run in SuperCollider from a seed draws the same numbers here. What is outside the table: a FaustDef's noise is its own code's, started the same way in every instance and seeded by nothing the server controls, and a plugin's is the plugin's.

**Where the time goes** is a second poll: **`/server_load`** → `/server_load.reply [uptime, n, n x (role, index, busy, calls)]`. `/server_status` answers whether the server is keeping up; this answers *on what*, which is the question a heavy session actually asks — a def compiling, a soundfile loading, a burst of commands and the DSP itself all cost, and only one of them shows in `avgCPU`. The roles are `audio` (the callback's whole block), `dsp` *i* (one worker thread, by index — the stages it took off the conductor; the conductor's own share stays in `audio`, since it is the same thread doing it), `net` (the serving turn: decoding, translating, pumping the subscriptions, collecting results — never the blocking wait for the next packet), `nrt` (the job queue: soundfiles, `/buffer_gen`, the editing verbs), `faust` (compiling a FaustDef, and absent from a build without the feature), and `disk-in` and `disk-out` (streaming: what the reader threads of every `DiskIn` node decode, and what the writer threads of every `DiskOut` node write). `index` is `0` for every role but `dsp`.

`uptime` and `busy` are in **seconds since boot** and `calls` counts the runs — blocks, stages, turns, jobs, compilations, and for the disk roles the stretches a thread worked between two waits on its ring. **Cumulative on purpose**: a window would have to be reset by whoever read it, which is exactly what makes `peakCPU` wrong when two clients poll at once, so here a client differences two replies and owns its own interval. Both clients do that for you (`Server.load()` / `Server.load()`, whose `share` field is the fraction of wall time busy since *your* previous call).

The server times **itself** rather than asking the operating system, which is what makes the reading identical on Linux, macOS and Windows: each piece of work brackets itself with a monotonic clock — `clock_gettime(CLOCK_MONOTONIC)` through the vDSO, `mach_absolute_time`, `QueryPerformanceCounter` — with no allocation, no lock and no kernel trap, so the bracket is legal on the audio thread. Two consequences to read the numbers by. **A role is not a thread**: what is measured is the work, so a build that runs one of these somewhere else reports the same roles. And **`busy` is time in progress, not per cent of a core**: a DSP worker spinning for its next stage is burning a core and is idle here, which is the honest answer to how much of the block budget a stage took — a system profiler (`top -H`, `pidstat -t`, `perf`) answers the other question. The disk roles are the one case of **several threads in one row**: a `DiskIn` or `DiskOut` node owns an I/O thread, there are as many as there are such nodes, and each direction sums into its own row — so a session bottlenecked on soundfile streaming shows it here, as one figure for all its streams rather than one per node.

**What inside a role** is a third poll: **`/server_profile`** → `/server_profile.reply [uptime, profiling, n, n x (role, phase, busy, calls), n, n x (node, def, busy, runs), n, n x (def, busy, runs, nodes), n, n x (address, busy, count, bytes), n, n x (address, count, bytes), n, n x (command, busy, count, waited)]`. Six sections, each a row count and its rows, all cumulative since boot like `/server_load`, so a client differences two replies:

| Section | A row | What it says |
|---|---|---|
| phases | `role, phase, busy, calls` | where the block went (`audio`: `commands`, the drained FIFO and the timed bundles, lanes and transport edges; `nodes`, running the tree; `publish`, the clocks, taps and bus levels) and where the serving turn went (`net`: `lanes`, `streams`, `tap-streams`, `buffer-streams`, `collect`). What a role spent outside its phases is the difference from its `/server_load` row. |
| synths | `node, def, busy, runs` | each live synth that ran while profiled, the costliest first, at most 256. |
| defs | `def, busy, runs, nodes` | every def whose synths ran while profiled, live or since freed, and how many synths that was. `?` for a synth whose def the server could no longer name. |
| commands | `address, busy, count, bytes` | every address the server received: the serving turn's time on it, how many, and how many bytes they carried. The messages a timed bundle or a `/sched_at` carries are listed under their own addresses, and a `/sched_at` row's time includes theirs. |
| replies | `address, count, bytes` | every address the server sent, pushes and streams included. |
| NRT jobs | `command, busy, count, waited` | every command that ran on the NRT queue: how long its jobs ran and how long they waited behind others. |

**The synth timing is the one part that is switched**: it costs the audio thread two clock reads per synth per block, which is a real share of the block in a session of thousands, so it runs only while some client has sent **`/server_profile 1`** (each `1` and `0` answers `/done /server_profile`). It stops when the last of them sends `0` or disconnects; a UDP client cannot be seen to leave, so it sends the `0` itself. `profiling` in the reply says whether it is on. Everything else costs a clock read or a table update per message, reply or job, off the audio thread, and is always counted. A server with no clock sends nil for every second, as `/server_load` does, and keeps every count. Both clients run a whole profile in one call (`Server.profile(seconds)`): ask for the timing, read, wait, read again, give it back, and return the difference.

**A server that cannot time itself says so, field by field.** `uptime` and every `busy` are then OSC **nil** (`N`) rather than a number, and `calls` is still there: a run is counted, not timed. Zero seconds would read as *idle*, which is the one direction a meter must not be wrong in. An engine inside a page is the case, and not for want of a clock in `wasm32` alone: the engine runs in an AudioWorklet, whose scope has no `performance.now()`, whose `currentTime` does not move inside a block, and whose `Date.now()` steps by a millisecond against a block budget of 1.33 ms. Both clients hand the nils on as no value (`None`, `null`) and print the reading as not available.

**Live audio input.** With `--inputs N` the server also opens the default input device and exposes its `N` channels on audio buses `outputs..outputs+inputs`, so `In` (or `In.ar`) reading those bus indices returns live device input — a microphone, a loopback, another app's output routed in via PipeWire/JACK. Without `--inputs`, those buses read as silence (no input device is opened). The count is negotiated with the host and may be clamped; the value actually opened is the `input_channels` field of `/server_query.reply` (0 if the input device was unavailable).

### Mapping controls to buses (`/node_map`, `/node_mapAudio`)

`/node_set` writes a control once. `/node_map id ctl bus` instead **binds** the control to a **control bus**: the node re-reads that bus at the start of every block, so the control tracks whatever any client (`/bus_set`) or synth (`OutCtl`) writes there — no further `/node_set`. `/node_mapAudio` is the same against an **audio bus**. Both take any number of `ctl bus` pairs, by control name or index, and work for UGen controls and Faust parameters alike.

A `busIndex` of `-1` removes the mapping (the control keeps its last value); a later `/node_set` on the same control also clears it and fixes the value.

### Ranges of controls (`/node_setRange`, `/node_fill`, `/node_mapRange`, `/node_mapAudioRange`)

The `-n` variants address **consecutive** controls in one message, for defs with array-like control blocks. `/node_setRange id ctl numControls value...` sets `numControls` controls starting at `ctl` (by name or index) from the value list; several `(ctl, numControls, values...)` groups may follow. `/node_fill id ctl numControls value` fills such a range with a single value (repeatable in `(ctl, numControls, value)` triples). `/node_mapRange id ctl busIndex numControls` maps `numControls` consecutive controls to `numControls` **consecutive** buses starting at `busIndex` (`busIndex = -1` unbinds the whole range); `/node_mapAudioRange` is the audio-bus form. Like `/node_set`/`/node_map`, all four accept a **group** id and propagate down its subtree, and clearing/setting a control that is used as a bus index re-sorts auto/parallel groups.

### Reading control values (`/synth_get`, `/synth_getRange`)

`/synth_get id ctl...` is the read counterpart of `/node_set`: it replies `/node_set id (ctl value)...` with each requested control's current value, each control **as it was asked for** — by name or by index — so the reply is the very message a GUI widget bound to that control would send. On a **GraphDef** instance (or a voice of one) the controls are its surface **ports**, named, as `/node_set` takes them, and each reads back as the port was set: from the first member control it drives, with that port's scaling undone. `/synth_getRange id ctl numControls...` reads a **range**, replying `/node_set id (ctl numControls value...)...`. Values come from the server-side node mirror, so they reflect the latest `/node_set`/`/node_setRange`/`/synth_new`, a plugin's own edits and a loaded state — except a control mapped to a **control bus**, which reads what the bus carries now, since that is what it follows. A control mapped to an audio bus has no single value to read and answers what was last set on it. An unknown node or out-of-range control replies `/fail`.

### A control as text (`/node_setText`, `/synth_getText`)

`/synth_getText id ctl...` answers `/node_setText id (ctl text)...`: each control's current value as text, the pair `/synth_get` makes with `/node_set`. A **plugin** parameter's text is the plugin's own (CLAP's value-to-text, VST3's string-by-value: `"-6.0 dB"`, `"Hall"`), asked on the plugin's control worker and never from audio, so its answer comes after the command returns; while one read for a node and a client is on its way, another from the same client is answered by that one. Any other control's text is its number — four significant figures at most, trailing zeros dropped — with its declared unit after a space (`"440 Hz"`).

`/node_setText id (ctl text)...` sets each control from text read the same way: by the plugin, in its own words, or as a number with its unit or without (`"880"`, `"880 hz"`). Text that does not read as a value replies `/fail "/node_setText"` and changes nothing.

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

All of these are disabled into an **auto-sorted group** (`/group_sortMode … 1`): there the execution order is recomputed from the bus-connection DAG, so a manual move whose destination is one replies `/fail` (moving a node out of one is allowed) — use auto-sort, or a manually-ordered group, but not both. (This is why `/node_order` earns its place only in manual groups: it is a batch `/node_before`/`/node_after`/`/group_head`/`/group_tail`.)

### Control buses (`/bus_set`, `/bus_setRange`, `/bus_fill`, `/bus_get`, `/bus_getRange`, `/bus_stream`)

Control buses are shared `f32` slots any client or synth reads and writes — the glue between `/bus_set` and `/node_map`. `/bus_set bus value...` writes single buses (pairs); `/bus_setRange bus numBuses value...` writes a **consecutive range** from a value list (repeatable groups); `/bus_fill bus numBuses value` fills a range with one value (repeatable triples). `/bus_get bus...` reads single buses, replying `/bus_get.reply (bus value)...`; `/bus_getRange bus numBuses...` reads ranges, replying `/bus_getRange.reply (bus numBuses value...)...`. Unset buses read `0.0`. The immediate forms write the shared atomics on the network thread; inside a **timed bundle** the writes travel to the audio thread so they land on the exact scheduled sample.

Beyond scsynth, **`/bus_stream periodMs bus...`** subscribes the sending client to a **periodic snapshot** of the listed buses: the server acks `/done /bus_stream`, sends one `/bus_stream.reply (bus value)...` immediately, and keeps sending one every `periodMs` (clamped to a 10 ms floor) without further requests. It is the network counterpart of reading the control buses from the shared-memory segment, for clients that cannot map it — a browser GUI's meters and scopes over WebSocket. One subscription per client, **replaced** by each new `/bus_stream`; `periodMs <= 0` (or an empty bus list) cancels.

**How many buses one subscription may list is boot-time configuration** (`--max-stream-buses`, `[server] max_stream_buses`, default 4096; the in-page engine has the same knob at boot), because a subscription is one client's whole live picture: a page of many canvases asks for a bus per meter, so the number scales with a document rather than with a widget. What a **given** client may ask for is that ceiling clamped by what one reply can carry over its own carrier — a snapshot is one message and is never split across replies, so a stream client (TCP/WebSocket) is bounded by its frame ceiling and a datagram-bounded one (UDP, the shared ring) by the same reply budget the overview commands keep. That per-client number is the last field of `/server_query.reply`, and a client that draws a great deal should read it rather than assume one. A request over it is refused whole with `/fail` — the previous subscription is left running, so the refusal must be acted on rather than logged: a client that believes its own request goes on drawing stale values. A subscription dies with its TCP/WebSocket connection; over UDP or the shared-memory ring it lasts until an explicit cancel or `/server_quit` (the `/server_notify` posture). Not schedulable in a timed bundle.

### Watching audio buses (`/bus_tap`, `/bus_tapStream`)

A control bus lives permanently in the shared segment, so anyone can read its value with a load. An audio bus does not: it is engine memory, one block at a time, and nothing outside the audio thread can see it. So the segment carries a fixed set of single-channel **sample rings** (`--taps` of them, `--tap-frames` samples each — defaults 8 × 16384; `--taps 0` disables the region), sized at boot and reported in `/server_query.reply`, and the server copies a bus into one on request. That is what lets a scope see the *samples* of a live signal: SuperCollider answers this with `ScopeOut2` writing into scope buffers; here the ring is a first-class segment region and asking for a bus is a command, not a UGen. **The rings are the server's own bookkeeping** — a client asks for a *bus*, never for a ring, and the segment publishes which ring ended up carrying it.

**`/bus_tap bus watch`** asks the server to record audio bus `bus` (`watch = 1`) or to stop (`0`). From the next block on, the audio thread appends that bus's samples to one of the rings (RT-safe — one `memcpy` and one atomic store per block), where a local peer (the GUI host's oscilloscope) reads the newest window straight out of shared memory, with zero per-frame messages. **The bus is the only number a client names**: the server picks the ring, publishes the choice in the segment's bus directory for whoever reads the samples, and counts watches — two views of one bus share a ring, and the last one to stop frees it. No ack, like `/node_map` (it only flips routing state; failures reply `/fail` — an unknown bus, no tap region, or every ring already in use) — sequence with `/server_sync` when needed. A **meter** needs none of this: the engine publishes a per-bus **level** every block — the block peak held with a 20 dB/s decay, so a reader far slower than the engine still catches a transient — and the same segment carries one for every audio bus, so metering a whole mixer costs no rings at all.

**`/bus_tapStream periodMs frames bus...`** is the network counterpart, for clients that cannot map the segment — a browser oscilloscope over WebSocket, or headless capture. The server acks `/done /bus_tapStream`, then sends, immediately and every `periodMs` (10 ms floor), one **`/bus_tapStream.reply bus endPosition blob`** per listed bus: the newest `frames` samples of that bus as raw little-endian `f32`, with `endPosition` (int64) the tap's stream position — total samples ever written — at the window's end, so consecutive snapshots can be placed on the tap's own sample axis (they overlap or gap by exactly the position delta). `frames` is clamped to the subscriber's transport bound — 8192 for a UDP client, up to the `--max-frame` ceiling for a TCP/WebSocket client, and for the **shared-memory ring** that same 8192 divided by the buses the subscription lists, because one serving turn pushes a snapshot per bus into the one ring and a subscription that asked for the whole of it would have all but the first dropped, silently, every turn — and always to half the tap ring; at most 8 buses per subscription; a bus that has not yet filled a window sends nothing. Same subscription posture as `/bus_stream`: one per client, replaced on each call, `periodMs <= 0` (or no taps) cancels, dies with its TCP/WebSocket connection, not schedulable in a timed bundle. The subscription **is** the watch: it starts recording every bus it lists and stops when it is replaced, cancelled or its connection dies, so a streaming client never sends `/bus_tap` itself.

## Timed bundles

OSC bundles carry an NTP timetag. The immediate tag (`1`) executes on arrival; a **future** timetag is converted to a position on the server's sample clock and the whole bundle fires **sample-accurately**: the engine splits the audio block at the sample nearest the event, as scsynth rounds it, so a `/synth_new` scheduled mid-block starts on that very frame, and what is left of the sample is the synth's `SubsampleOffset`, scsynth's `[0, 1)` value. Bundles with equal times run in arrival order; late bundles run immediately (and are logged). Nested bundles are scheduled independently by their own timetags.

Schedulable inside a timed bundle: `/synth_new`, `/node_set`, `/node_setRange`, `/node_fill`, `/node_map`, `/node_mapAudio`, `/node_mapRange`, `/node_mapAudioRange`, `/node_free`, `/node_before`, `/node_after`, `/node_order`, `/group_head`, `/group_tail`, `/group_new`, `/group_name`, `/group_freeAll`, `/group_deepFree`, `/bus_set`, `/bus_setRange`, `/bus_fill`, `/node_ugenCmd`, `/group_sortMode`, `/group_parallel`, `/graph_new`, `/graph_addSlot`, `/graph_moveSlot`, `/graph_newVoice`. Anything else (defs, buffers, queries, server commands) replies `/fail … cannot be scheduled in a timed bundle` — load defs and buffers first, then schedule the notes.

**`/sched_clear [axis]`** flushes timed bundles: with no argument the whole queue, device axis and transport axis alike — every bundle waiting on the sample clock is dropped (their heap freed off the audio thread) — and the command replies `/done /sched_clear`. That is the scsynth panic button, and it is every client's score at once.

**`/sched_clear "transport" <int32 transport>`** drops **that transport's queue alone** and leaves the device one, and every other transport's, standing. It is what a client re-cueing a plan after a locate needs: the transport clock is monotonic and does not jump, so bundles queued for the position that was left would sound at it, while the bare form would take the rest of the server's scheduled work with them. Anything else in that argument fails.

Also beyond scsynth: **auto-sorted groups**. `/group_sortMode groupID 1` makes a group keep its children in dependency order inferred from the buses each def reads and writes — no more manual `/node_before` bookkeeping; query what the server inferred with `/group_queryTree` and `/group_dumpGraph`. See [`auto-order.md`](auto-order.md) and `crates/clausters-server/examples/auto_order.py`. The same analysis powers **parallel groups**: `/group_parallel groupID 1` (with the server started as `--workers N`) runs a group's independent children on several cores, bit-identically to the sequential result — see [`parallel.md`](parallel.md).

Beyond scsynth, the server also exposes its **sample clock** directly: `/clock_query` queries the sample counter, the actual sample rate and the server's OSC time captured with the counter (the master-clock anchor), and `/sched_at <int64 target> <blob>` schedules a packet at an **absolute sample** instead of an NTP time — same queue, same sample accuracy, drift-free by construction. See [`sample-clock.md`](sample-clock.md) and `crates/clausters-server/examples/sample_clock.py`.

It also keeps **shared transports** — each a beat grid for phase-aligning several clients on that sample clock, plus a DAW-style rolling state.

**A server has several transports, and every command names one.** They are sized at boot (`--transports`, 16 by default, at most 64) and addressed by id, `0` to that count minus one, which **every** `/transport_*` command and `/sched_atTransport` takes as its **first argument**, an `int32`; an id the server does not have `/fail`s. Each transport is independent — its own grid, rolling state, position, loop, end mark, governed group, clock and scheduling queue — so two applications on one server each play, pause and locate their own nodes without moving the other's. Transport 0 is the one a server has always had. The id is never optional: a sample position may arrive as an `int32`, so a leading optional integer could not be told from the argument after it (`/transport_end 5` would be a mark at 5, or transport 5's mark cleared). Everything below describes one transport.

`/transport_query <int32 transport>` **reads** it (replies `/transport_query.reply <int64 originSample> <double tempo> <int32 defined> <int32 playing> <double position> <int32 group> <int64 transportSample> <int64 positionSample> <int64 loopStart> <int64 loopEnd> <int64 endSample> <int64 returnSample> <int32 transport> <int32 followGroup> <int64 fade>`, `defined` 0 until one is set, `group` `-1` when no group is bound, `loopStart`/`loopEnd` both 0 when nothing loops, `endSample`/`returnSample` `-1` when there is no end mark or it has no return, `transport` the id the reply is about — the pushes of every transport share the address, so a listener tells them apart by it — `followGroup` the group following it, `-1` for none, and `fade` the length of its ramp in samples, `0` for none); `/transport_set <int32 transport> <int64 originSample> <double tempo>` **sets** the grid (last writer wins, stopped at position 0, replies `/done`); the governed group and the loop survive it, since both are bindings to the tree and to the samples rather than parts of the grid. Beat `b` is sample `originSample + b·rate/tempo` of the **device** clock — the grid runs on through a stopped transport, and where the music is in a pause is its `position` — where `rate` is the rate the engine runs at (`actual_sr`, which differs from the requested `nominal_sr` only when the device could not honour it). The **rolling state** rides on top: `/transport_play <int32 transport> [<double position>]` starts playing (from `position`, or where it last stopped), `/transport_stop <int32 transport>` stops, `/transport_locate <int32 transport> <double position>` sets the song position; each replies `/done`. Any change is **pushed** to every `/server_notify` client as a `/transport_query.reply`, so a responder re-aligns or rolls its playhead live.

**The grid is optional, and only the commands that speak beats need it.** The transport exists on every server from boot: rolling, stopping, saying where the transport is and looping a span of it are all in **samples**, and an audio editor has no tempo to declare. So `/transport_play` (bare), `/transport_stop`, `/transport_group`, `/transport_locateSample` and `/transport_loop` work with no grid at all, while the two that name a **beat** — `/transport_locate`, and `/transport_play` *given a position* — `/fail` until `/transport_set` has said what a beat is. `defined` in the reply says whether the grid is there, and `position` (the beat spelling) stays 0 while it is not; `positionSample` is live either way.

**Where the transport is, and the two commands that address it in samples.** `position` is the song position in *beats*, which is what a sequencer speaks; an audio editor addresses **frames**, so the same position is also reported as `positionSample` — the sample of the transport's axis the engine is playing. **`/transport_locateSample <int32 transport> <int64 sample>`** is `/transport_locate`'s sibling for that axis: a sequencer locates by beat and an editor by frame, and converting either into the other on the client is how a rounding error gets into a seek. Both spellings are kept in step, so the two fields never disagree. The transport's axis starts at its own 0 — `originSample` anchors the beat grid on the *device* axis, for aligning several clients on one running server, and is not consulted here.

**`/transport_loop <int32 transport> [<int64 start> <int64 end>]`** is the span the position wraps inside, in samples; **with no arguments it turns looping off**. The span is **half-open**: a loop of `0 n` over an `n`-sample take plays every frame exactly once and joins its own start with no repeated frame. Setting one does not move the transport — it keeps playing from where it is and wraps when it first reaches the end — and the wrap happens *in the engine*, on its exact sample, so a client sends nothing when a pass completes and a reader following the transport hears no seam. An empty or inverted span `/fail`s rather than being ignored. There is no `enabled` argument: what a loop toggle remembers is the span it last used, and that is the client's to keep. **A change while the transport rolls over disk readers is cued**: a `TransportDiskIn` under its governed group reads the new loop's frames first, splicing them on where the old and the new readings part, and the engine puts the loop in place once every such reader holds them — at most 0.2 s later, as the engine's cue waits. Until then the transport wraps by the loop it had, so a change asked for just before the old loop's end takes effect a pass later rather than being heard. A change asked while the readers still play the frames they kept from the change before it is the same: frames that already read the loop asked for (a switch turned off and back on) make a reader ready at once, and otherwise a reader keeps what the loop in force plays, so whichever loop the transport wraps by, the reader holds it. A transport with no disk reader under it, or one that is stopped, takes the change at once.

**`/transport_end <int32 transport> [<int64 end> [<int64 return>]]`** is the **end mark**: the sample of the position a rolling transport stops on, and where it is located once it has; **with no arguments it clears the mark**. The stop is the engine's, on the mark's exact sample, and it is what a `/transport_stop` landing there would do — the governed group and the transport clock freeze — so a take played without a loop stops where its samples do rather than rolling on forever, and nothing depends on a client watching the position and sending a stop late by its own latency. With no `return` the position rests on the mark. The stop is pushed to every `/server_notify` client as a `/transport_query.reply` with `playing` 0, so whoever holds readers learns the pass is over. The mark stays set, so the next play from `return` ends at the same place, and the transport **never rolls past it**: a play from at or past the mark stops at once. **A loop wins** — while one is set the position wraps and never reaches the mark. Like the loop, it only has teeth with a group bound, since an ungoverned transport does not roll in the engine at all.

**Two quantities, and confusing them is the easy mistake.** `transportSample` is the transport **clock** — samples *elapsed* under the transport, held while it is stopped and **monotonic**, which is what `/sched_atTransport`'s queue needs, since "due" only means anything on an axis that cannot jump. `positionSample` is where the transport **stands**: it jumps wherever a locate puts it and wraps at a loop's end. A playhead reads the position; anything scheduling reads the clock. Both are published in the shared-memory segment ([`ipc.md`](ipc.md)), and `positionSample` is answered from the engine as of its **last completed block** — the same rule `/buffer_query` states for the buffer mirror — **except right after a locate**: until a block has applied it, the server answers with the place it located to, so a reply in the same breath as a locate (its own broadcast above all) never reports the place the transport is leaving. A client re-cueing from a broadcast depends on that.

**The transport has two intensities, and `/transport_group` is the switch.** With **neither a governed nor a followed group bound** it is what the paragraph above describes: an in-memory registry the server stores, serves and broadcasts but never schedules audio from — clients read it to start their routines and playheads on the same beat, and obey it by choice. **`/transport_group <int32 transport> <int32 group>`** binds the group the transport *governs* (a negative id unbinds, replying `/done`; an unknown id, a non-group, or a group another transport governs `/fail`s — a group has one transport), and from then on the **engine enforces it**: `/transport_stop` freezes that subtree and the server's own transport clock at the exact sample it lands on, and `/transport_play` thaws them. Freeing the governed group unbinds the transport, and unbinding thaws whatever it governed, so no frozen subtree is ever left with nobody to resume it. **A node reads its own transport**: the one bound to the nearest bound group above it, or transport 0 when none is — so a `TransportPos` inside a group bound to transport 1 follows transport 1 with nothing in the def saying so.

**`/transport_follow <int32 transport> <int32 group>`** binds a group that **follows** the transport without being governed by it (a negative id unbinds; replies `/done`, fails like `/transport_group`). Its nodes read the transport — its position, whether it rolls — and nothing freezes them, and their timed bundles stay on the device queue. It is for the part of an application that must go on running while its transport is stopped and still has to know that transport: an output with its meter and its declick, beside the governed group or around it. A transport has one following group as it has one governed group, and a group is bound to one transport either way.

**`/transport_fade <int32 transport> <int64 samples>`** sets how long a stop and a play **ramp** (replies `/done`; a negative length `/fail`s). `0`, the default, is no ramp, and a stop freezes on its own sample as described above. With a ramp, a stop is a **stopping phase**: the governed group goes on running and the position and the transport clock go on advancing while the level falls to zero, and only then do they freeze — so the position rests where the readers stopped reading, a ramp's length after the stop was asked. A play thaws and the level rises from zero; a play during a stopping phase calls it off, rising from wherever the level had fallen to. The end mark starts its ramp that long before the mark, so the pass still ends on it. A loop's wrap and a locate are not stops and are not ramped. What reads the level is the `TransportFade` UGen, in a group that follows the transport: an output multiplies what the readers wrote by it, and neither edge clicks.

**A thaw is not the other half of a pause.** A frozen subtree gets no time while everything that drives it keeps moving -- the position it reads, and any control bus written from outside it -- so the *smoothing* inside it holds a value from a moment that has passed. On `/transport_play` the engine therefore lets every smoother under the governed group forget where it was, and it comes back at the value its driver has now: a fader mapped to an automation is where the automation says, from the first sample, and a meter reports the pass it is playing rather than the one before it. Only smoothing resets -- a filter's memory, a delay line and an oscillator's phase are the signal itself and survive the stop, which is what makes playing again a *continuation*. `/node_run` is untouched: a node paused by hand resumes exactly as it was.

A frozen node stays in the tree with its internal state untouched — the same thing `/node_run 0` does — so a resume **continues** the sound rather than restarting it. That is the only thing a pause can mean for samples the server generates itself: a def running a stochastic process or a demand-rate sequence has no samples to index into, so its position *is* its state. Consequently **`/transport_locate` moves the position, never the state of a node**: locating changes `position` and re-anchors the beat→sample mapping, and resident nodes stay exactly where they are. A client that wants a generator to become seekable renders it first.

**A reader can follow the position instead of carrying one.** That is what the `TransportPos` UGen is for: a buffer reader whose phase is the transport's position seeks when the transport seeks, loops when it loops and holds when it stops, with nothing sent per pass and no position of its own to keep in step. It is the shape a multitrack needs — many readers, one time — and it is why locating never has to reach into a node: a follower has no state to move. A reader that carries its own position is the other shape and stays available (`PlayBuf` from its own `startPos`, `BufRd` over any phase you like); the two coexist, and a one-shot fired from a pattern has no business consulting a transport.

Two clocks follow from this, and every sample count on the wire belongs to one of them. The **device** clock never stops (`/clock_query`, the taps, the bus streams, the meters); the **transport** clock, reported as `transportSample`, advances only while the transport rolls and is the time of the governed *subtree*. They are one physical clock — `transportSample` is the device clock minus the total time spent stopped — so they cannot drift apart, and they differ only while a governed transport is stopped. The shared-memory segment publishes both.

What rides which clock is decided by **where a scheduled bundle's messages point**: a bundle any of whose messages targets a node at or under a governed group waits out that transport's pause — the transport governing the first such target — and everything else fires on the device clock as always. A bundle is atomic, so a **mixed** bundle — one message inside the governed subtree, one outside — goes entirely to the transport clock, which is the one way a message aimed at a live node can end up waiting. Classification happens **once**, when the bundle is queued: a bundle scheduled *before* `/transport_group` binds stays on the device clock even if its target becomes governed later.

**`/sched_atTransport <int32 transport> <int64 sampleTarget> <blob packet>`** is `/sched_at` on that transport's axis (see below), for a client naming an absolute sample of that axis. Declaring the axis is not about disambiguation — classification is deterministic, and a client that bound the group knows which of its nodes are governed — it is about **verification**: the server compares the declaration against its own classification and replies `/fail` when they disagree, instead of playing the bundle in the wrong place. It needs a group bound.

**Event lanes: notes and messages the transport plays by its position.** A take follows a transport because its reader reads the position, so a locate, a loop and a stop are the transport's and nothing is sent per pass. A **lane** is the same for events: it holds notes, OSC messages and MIDI messages as data, at positions of one transport, and the server plays them there — a locate moves them, a loop plays them again on every pass, a stop releases them, with no client in the loop. Three commands, each replying `/done`:

- **`/lane_new <int32 transport> <int32 lane> <int32 target>`** makes lane `lane` (an id the client picks, like a buffer's) on that transport, its notes made at the tail of group `target`. **Make it a group the transport does not govern** — one that follows it, or any other: a note is a voice, not a reader, and a voice frozen by a stop would sound again, mid-release, wherever the next play starts. The lane releases what it sounds on every stop, so its releases ring out there. An existing lane of that id is freed first.
- **`/lane_set <int32 lane> <json>`** replaces the lane's data whole, the JSON as a string or a blob (as a def is sent): `{"notes": [[start, end, destination, {"control": value, ...}, "gate"|"free", joins], ...], "messages": [[position, "/address", args...], ...], "midi": [[position, byte, byte, ...], ...], "ump": [[position, word, word, ...], ...]}`, every position a sample of the transport's position and every list optional. A note's `destination` is a def's name — a synth of that def with those controls — or `{"graph": id, "slot": name}`, one more of that slot of the running GraphDef instance `id` (`/graph_addSlot`), the controls its ports: how a note carries the curves that shape it, read beside it in its own graph — or `{"node": id}`, a note of node `id`, one that reads notes (a `Voices` node or a plugin's): its `key`, `channel`, `velocity` and `tuning` say the note, given to the node as `/node_event` gives one and identified by the lane, and every other control is set on that note's voice. A synth or a slot is released at `end` by `gate 0` (the default; a slot's through its `gate` port) or by a free, a node's note by its note-off. The optional `joins` is the index of an earlier note in the list that this one goes on from — a voice's legato: on a node the note-on joins that note (a `Voices` node moves its voice to the new key, a plugin gets the new note overlapping the old one); on a def the line keeps the synth the first note made and each joined note sets its controls, and the line's last note releases it. A locate into the middle of a line still ends it; a message is a command the server takes in a timed bundle (the list above), run as written — one it cannot is logged and skipped; a MIDI message plays as though it had reached the server's MIDI input at that position — written on the event bus the channel's `/midi_bind` routes it to, from the lane's own source, so a note-on and the note-off after it are one note (identified by the lane) that the lane releases on a jump or a stop, and its controllers, bends and pressure never reach the live input's notes, nor theirs the lane's. A note-on the lane never turns off is not played, a message on an unbound channel sounds nothing, and a program change is written before the notes after it. On a channel bound to a GraphDef a note is one more of its voice slots, as live. A `ump` entry is one MIDI 2.0 packet (a Universal MIDI Packet's words), read among the MIDI bytes in position order as `/midi_ump` reads it: its notes and channel messages play as their MIDI 1.0 twins do, at their own resolution, and a per-note message reaches the lane's note sounding on its channel and key. What sounds keeps its release, and the new data is heard from where the position is.
- **`/lane_free <int32 lane>`** frees the lane and releases the notes it is sounding.

How it plays: the server keeps the stretch of each lane just ahead of the position (half a second) **built** on the transport's lane queue, and an entry fires on the exact sample the position reaches it — a note's release then goes onto the transport's clock queue, its length later. Keyed by position, nothing is re-stamped when the transport jumps. **A jump or a stop releases what the lanes are sounding** — a locate, a `/transport_stop`, and a pass that stops on its end mark — as a DAW's stop sends its note-offs; a play does not bring a released note back, and a wrap releases nothing, so a note rings across the seam. Node ids are the server's, minted per pass from the auto range, since a lane plays again and again. A lane's releases are left standing by a client's `/sched_clear`, which would otherwise leave its notes sounding for good; a lane is emptied by a new `/lane_set` or by `/lane_free`. An NRT score has no lanes.

With no group bound, `group` reports `-1` and the transport is the advisory registry it has always been: `/transport_play` and `/transport_stop` are broadcast for the clients to obey by choice, and nothing in the tree freezes. A followed group also drives the engine clock, without being frozen by it. See [`sample-clock.md`](sample-clock.md).

UDP is not the only transport: local clients can speak the same OSC through **shared memory** (`clausters --shm <path>`) or run the whole server **in-process** through the embed C ABI, with the sample clock and the control buses readable and writable directly in mapped memory. See [`ipc.md`](ipc.md) and `clients/python/clausters/ipc.py`.

The server also speaks **TCP**, **on by default** on the same port `57110` (alongside UDP — separate namespaces; `--no-tcp` disables it, `--tcp [[addr:]port]` moves it). **UDP is always on** — it is the base transport and cannot be turned off (there is no TCP-only mode); it is the boot/discovery protocol a client probes first, and it doubles as internal infrastructure, since the TCP loop wakes itself with a zero-length datagram to the server's own UDP socket. Each OSC packet — message or bundle — is **length-prefixed**: a 4-byte big-endian byte count followed by exactly that many OSC bytes, the same framing scsynth uses; replies come back framed the same way over the one connection. TCP gives a reliable, ordered, connection-oriented channel — no silent packet loss, and no datagram size limit: a frame may be as large as the **`--max-frame` ceiling** (default 16 MiB, advertised in `/server_query.reply`), which is a DoS guard on the untrusted length prefix, not a protocol limit. That makes TCP the natural **command plane** for large payloads — whole defs, big `/buffer_getRange` chunks — while timing still rides on bundle timetags / `/sched_at`, so arrival latency does not affect *when* a scheduled command fires. The Python client speaks it through `OscTcpInterface`.

The server also speaks **WebSocket** when started with `clausters --ws [[addr:]port]` (default port `57120`, on loopback — a page served from another machine needs the address said out loud, `--ws 0.0.0.0:57120`) — always available, like TCP and shared memory, not behind a build feature. This is the transport a **browser** can reach — a browser cannot open a raw UDP socket or map shared memory, but speaks WebSocket natively — so it is what lets a web-hosted client drive the server (and the server "run in the browser"). It carries the same OSC the same way the other transports do, multiplexed into the same loop and waking it with the same zero-length-UDP trick; the one framing difference from TCP is that each OSC packet is one WebSocket **binary** message, so the frame boundary *is* the packet boundary and there is no length prefix (replies come back as binary messages). Inbound bytes validate through the same single decode door as every transport, and messages are bounded by the same `--max-frame` ceiling TCP applies to its length prefix. The Python client speaks it through `OscWsInterface`; a browser uses the native `WebSocket` API (see `crates/clausters-server/examples/ws_ping.py` and `crates/clausters-server/examples/ws_ping.html`). `ws://` only — TLS (`wss://`) is out of scope, terminate it at a reverse proxy if needed.

Both stream transports share a few **edge guards**, sized like everything else for the target deployments (loopback and controlled networks, not a public service). Concurrent connections, TCP + WebSocket combined, are capped by **`--max-clients`** (default 64, in the spirit of scsynth's `maxLogins`) — a connection past the ceiling is dropped at accept, and closing any connection frees its slot. A client that *floods* commands faster than the server consumes them is held back by TCP flow control (the inbound queues are bounded, so server memory is too) — nothing is dropped, the sender just blocks. A client that *stops reading* its replies is disconnected rather than allowed to stall the server or grow its reply queue without bound. None of this affects a well-behaved client, dense control streams included; UDP is connectionless and keeps its own natural bounds (the datagram cap, kernel buffering).

## NRT mode (offline rendering)

The same engine renders OSC scores to WAV without an audio device.
Native Linux builds with `plugin-clap` or `plugin-vst3` and their matching helper also accept
`/def_send plugin`, `/synth_new` and `/plugin_event` in an OSC score. References
are loaded synchronously before time advances. All audio ports and the same
sample-offset event/transport spans are processed by the isolated helper.
The configured bridge depth and intrinsic latency remain in the output; the
renderer waits outside RT rather than dropping results against a deadline.
An error, helper exit or processing timeout aborts with the node id and
processing time. Arbitrary plugins need not be deterministic, even with a
fixed engine seed. An in-page engine cannot load native libraries.

The command is:

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

Unlike the live server, a score bundle may also contain the asynchronous commands `/def_send synth`, `/def_send faust`, `/def_free` and the `/buffer_*` family: they complete synchronously before time advances (scsynth NRT semantics), and any failure aborts the render with the offending event's time and message. A score's `/server_seed` reseeds at its time. Queries (`/server_status`, `/server_load`, `/buffer_query`, `/buffer_get`, `/buffer_getRange`, `/bus_get`, `/def_query`, `/ugen_query`) are errors in a score.

`python3 crates/clausters-server/examples/json_client.py score` writes an example score; `cargo run --release --example bench` measures graph throughput offline.

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
| `"lag_down": t` | with `"lag"`, use separate up (`lag`) and down (`lag_down`) times (an inserted `LagUD`) |

Example: `{"name": "freq", "default": 440.0, "lag": 0.1}` glides over 100 ms; `{"name": "gate", "default": 0.0, "rate": "tr"}` is a one-shot trigger. Audio-rate controls are not a control type — feed an audio signal through `In`/an input bus and `/node_mapAudio` instead.

A control entry may also say **how it is drawn**, which `/def_query` reports back and a node's generic editor and a widget built from the control draw it with. It never limits the control: a `/node_set` outside the range still sets it.

| field | meaning |
|---|---|
| `"min"`, `"max"` | the range it is drawn over — both or neither, `min` below `max` |
| `"step"` | the grid a gesture lands on; `0` or absent is continuous |
| `"warp"` | `"linear"` (the default) or `"exp"`, equal ratios over equal travel — over a positive range |
| `"unit"` | what the value is measured in (`"Hz"`, `"dB"`), written after it as text |

Example: `{"name": "freq", "default": 440.0, "min": 20.0, "max": 20000.0, "warp": "exp", "unit": "Hz"}`. A description no view can draw — a reversed range, half a range, a negative step, an unknown warp, an exponential range touching zero — fails the `/def_send`.

### UGen catalog (built-in kinds)

The `kind` field is an **opaque string** as far as the protocol is concerned: the schema above is the whole contract (name + inputs + optional rate + static fields). What follows is a separate thing — the **catalog of UGens the server currently ships**, which the server resolves at runtime and grows independently of the wire format (each is one descriptor entry under `crates/clausters-server/src/dsp/registry/`). So this table is reference, not part of the protocol: an unknown name simply fails with `/fail … unknown kind`.

A client does not *have* to enumerate the catalog — naming a kind and letting the server validate remains the normal path — but it **can**, with [`/ugen_query`](#def-buffer-and-ugen-introspection): the same descriptors as a typed reply, so a palette or completion list derives from the server's own truth (and from the build it is actually talking to) instead of a copy of this table that would drift. Input names below are given in **wire order**, which is what `/ugen_query` reports.

**A def is short only where the kind says it may be.** Inputs are positional and arity is exact, so a UGen that gains an input would break every def ever written against it — including the ones persisted on disk and the ones inside a saved bundle (`PlayBuf` going from four inputs to seven did exactly that). Some kinds therefore declare an **optional tail**: a run of trailing inputs a def may stop before, which the server fills from the catalog's own defaults while compiling. The compiled def is identical to one a complete client sent, so nothing about playing it changes.

Which slots earn it is a property of the *kind*, not a general permission: a slot is optional only when its default is **inert** — the value that makes the UGen behave as if the slot were not there (0 for a trigger, an offset, a phase, a channel, a done action; 1 for a level or a rate scale). A slot whose default is a *choice* (`freq`, `delaytime`, `width`, `max`) stays required, because omitting it would not be leaving it alone; so does any slot the UGen reads its signal, source, position or chain from, since silence and frame 0 are legal values a wrong def would run with. `BinaryOpUGen` is `a=0, b=0` and has **no** optional tail at all — filling a truncated `Mul` would silence the chain with no `/fail` and no name. A def that is short of the tail, or longer than the kind, still fails with `expected … inputs`.

This makes growth **by the tail** non-breaking and does nothing for an input inserted in the middle, which stays breaking. And the declared default of an optional slot is **wire contract**: changing it later changes what every def leaning on the fill sounds like.

| kind | inputs | output |
|---|---|---|
| `Sine` | freq (Hz), phase (cycles, optional) | sine by f64 phase accumulation, starts at phase 0 — scsynth's `SinOsc`, computed rather than read from a table. `phase` is a **running** offset (an audio-rate one is phase modulation; a control-rate one moves across the block rather than stepping), in cycles where `SinOsc` takes radians |
| `FSinOsc` | freq (Hz), iphase (cycles) | a sine by the two-pole recurrence `y = 2cos(w)·y₁ − y₂`; the frequency read once a block, and a change moves the amplitude with it (the recurrence's price), so it is for a fixed tone. scsynth's `iphase` is in radians |
| `SinOscFB` | freq (Hz), feedback (rad) | `y = sin(phase + feedback·y₁)`, computed where scsynth reads a table; a control-rate `feedback` is a line across the block |
| `PSinGrain` | freq (Hz), dur (s), amp | one sine grain under a parabola peaking at `amp`, its inputs read at the start; frees its synth at the end |
| `Blip` | freq (Hz), numharm | `numharm` equal cosine harmonics, peak 1, kept below Nyquist: the Dirichlet kernel computed where scsynth reads sine and cosecant tables. A change of the count crossfades over the block; both inputs read once a block. scsynth advances a clipped block at the larger of the old and new frequency; here at the frequency |
| `Formant` | fundfreq, formfreq, bwfreq (Hz) | at every cycle of `fundfreq`, a burst of a sine at `formfreq` under a raised cosine `bwfreq` wide (never longer than the period). Each burst restarts within the sample; scsynth's restart overflows a 32-bit product, so its carrier starts each burst at a phase that is noise |
| `Klang` | freqscale, freqoffset, *(freq, amp, phase) per partial* | a bank of fixed sines by the recurrence, read at the start; `phase` in cycles (scsynth's in radians) |
| `Impulse` | freq (Hz), phase (cycles, optional) | single-sample `1.0` every `freq` Hz, `0.0` between; with `phase` 0 the first output sample is an impulse, so a `/sched_at`'d `/synth_new` places it on an exact frame; `freq` 0 emits one impulse then silence (f64 phase, drift-free). `phase` offsets the train and is a running offset: moving it moves the train, firing where that carries it across a cycle. A negative `freq` runs the train backwards. A control-rate `freq` or `phase` moves across the block, as scsynth's does |
| `WhiteNoise` | — | uniform white noise in ±1 |
| `Saw` | freq (Hz) | band-limited rising sawtooth in ±1, starting at 0 (no onset step, no DC). Anti-aliased with PolyBLEP — see the note below |
| `Pulse` | freq (Hz), width | band-limited pulse in ±1; `width` is the duty cycle (0.5 = square), clamped just inside (0, 1) where the two edges would coincide |
| `LFSaw` | freq (Hz), iphase | rising sawtooth in ±1, **not** band-limited — a modulation shape. `iphase` is the initial phase in **cycles**, [0, 1), read once at the first sample |
| `LFPulse` | freq (Hz), iphase, width | square in **[0, 1]** (a gate, unlike the bipolar `Pulse`) with `width` as its duty cycle, taken at the start of each cycle, so modulating it never cuts one short; the first sample of a cycle is always the polarity a narrow duty would otherwise lose, so a pulse narrower than a sample still pulses (both scsynth's rules); not band-limited |
| `LFTri` | freq (Hz), iphase | triangle in ±1 starting at 0 and rising; not band-limited |
| `VarSaw` | freq (Hz), iphase, width | triangle whose peak sits at `width` of the cycle — sweeps from a falling ramp through a triangle to a rising one. `width` is taken at the start of each cycle and kept within [0.001, 0.999]; not band-limited |
| `LFCub` | freq (Hz), iphase | a sine-like shape of two cubic arcs, `z²(6 − 4z) − 1` over a triangle `z`: slope continuous, within 0.020 of a sine; starts at 0 rising; not band-limited |
| `LFPar` | freq (Hz), iphase | a cosine-like shape of two parabolas, within 0.056 of a cosine; starts at its peak; not band-limited |
| `SyncSaw` | sync_freq (Hz), saw_freq (Hz) | a sawtooth at `saw_freq` restarted at every cycle of a master at `sync_freq` (hard sync), the restart placed within the sample as scsynth places it; not band-limited |
| `Vibrato` | freq, rate (Hz), depth, delay (s), onset (s), ratevariation, depthvariation, iphase, trig | `freq · (1 + depth · w)`, `w` a parabolic sine; each cycle draws its rate and the depth of each half from the synth's generator, scsynth's draws in its order. Starts after `delay`, fades in over `onset`; a rising `trig` restarts it from `iphase` (cycles). scsynth reads `freq` as a block buffer, so a constant there is read past its value; here any rate is itself |
| `LFGauss` | duration (s), width, iphase, loop, done_action | the bell `exp(−x²/(2·width²))`, `x` from −1 to 1 over `duration`; `iphase` shifts it along `x` and is a running offset; without `loop` it plays once and takes `done_action` |
| `LPF`, `HPF` | signal, freq (Hz) | second-order Butterworth low/highpass: −3 dB at `freq`, −12 dB/octave. Every named two-pole row is scsynth's own section — see the filter note below |
| `RLPF`, `RHPF` | signal, freq (Hz), rq | resonant low/highpass, scsynth's: unity gain at DC / at Nyquist |
| `BPF` | signal, freq (Hz), rq | bandpass with **unity gain at the centre**, `rq` its bandwidth ratio |
| `Resonz` | signal, freq (Hz), rq | scsynth's two-pole resonator, its zeros at DC and Nyquist and its poles placed by radius: near unity at the centre for a narrow bandwidth, and not the same response as `BPF` at a wide one |
| `BRF` | signal, freq (Hz), rq | band reject: unity in both passbands, a true null at `freq` |
| `Svf` | signal, freq (Hz), rq, low, band, high | the state-variable filter with its three tap gains as **signal inputs**, so the response is modulable. Lowpass = `1,0,0`; bandpass (unity peak) = `0,rq,0`; highpass = `0,0,1`; notch = `1,0,1`; peak = `-1,0,1`; allpass = `1,-rq,1` |
| `Klank` | signal, freqscale, freqoffset, decayscale, *(freq, amp, time) per partial* | a bank of fixed resonators, each ringing `time` to −60 dB with its response's peak at `freq` (`cos θ = 2R·cos w / (1 + R²)`), the sum through zeros at DC and Nyquist; read at the start |
| `Median` | length, signal | the median of the last `length` samples (1..31), scsynth's aged sorted window; `length` read at the start |
| `MoogFF` | signal, freq (Hz), gain, reset | Fontana's zero-delay-feedback Moog ladder: four bilinear one-poles, −12 dB at `freq` without feedback, resonance `gain` 0..4 (`|H| = 1/4 / |1 − gain/4|` at `freq`); a positive `reset` clears it; a control-rate `gain` is a line across the block |
| `Hilbert` | signal, chan | the analytic pair, two chains of six allpasses 90° apart across the band (scsynth's poles), `chan` 0 the real part and 1 the imaginary |
| `FreqShift` | signal, freq (Hz), phase (cycles) | single-sideband shift of every frequency by `freq`: the analytic pair times a quadrature oscillator, computed where scsynth reads a table. scsynth's `phase` is in radians and its oscillator runs a sample ahead |
| `Amplitude` | signal, attacktime, releasetime (s) | envelope follower; times to within a tenth (20 dB), scsynth's code (its documentation says 60 dB). At control rate over audio it follows every sample, where scsynth's times come out a block's samples longer |
| `Compander` | signal, control, thresh, slopebelow, slopeabove, clamptime, relaxtime | `control`'s envelope sets a gain `(env/thresh)^(slope − 1)` once a block, the gain a line across it; scsynth's first block fades in from 0 |
| `Normalizer`, `Limiter` | signal, level, dur (s) | look-ahead gain: the input `2·dur` late under a gain that moves in lines to what each window's peak and the next one's call for, so the output never passes `level` (the `Normalizer` brings it to `level`). `dur` sizes the windows at build; the gain in `f64`, where scsynth's `f32` line drifts by its rounding |
| `LPZ1`, `HPZ1`, `LPZ2`, `HPZ2`, `BPZ2`, `BRZ2` | signal | scsynth's fixed FIRs: two-point average and difference, and the two-zero low, high, band and band-reject sections |
| `Slope` | signal | the rate of change per second, `rate·(x0 − x1)` |
| `Flip` | signal | every other sample negated, the first among them: the spectrum mirrored about a quarter of the rate |
| `Delay1`, `Delay2` | signal, x1 [, x2] (optional) | the input one or two samples late, starting from `x1` (and `x2`) |
| `TwoPole`, `TwoZero`, `APF` | signal, freq (Hz), radius | two poles, two zeros, or the two-pole allpass, at `freq` with `radius`. scsynth's `APF` is the identity in exact arithmetic (only its unrolled loop's misread state lets anything else through); this is the allpass its documentation describes |
| `Ringz` | signal, freq (Hz), decaytime (s) | a resonator ringing `decaytime` to −60 dB |
| `Formlet` | signal, freq (Hz), attacktime (s), decaytime (s) | a resonance rising in `attacktime`, falling in `decaytime`: two `Ringz`, the slower minus the faster |
| `MidEQ` | signal, freq (Hz), rq, db | the input plus a bandpass: `db` of boost or cut around `freq` |
| `BLowPass`, `BHiPass`, `BAllPass` | signal, freq (Hz), rq | the cookbook biquads of scsynth's BEQSuite |
| `BBandPass`, `BBandStop` | signal, freq (Hz), bw (octaves) | the cookbook band biquads |
| `BPeakEQ` | signal, freq (Hz), rq, db | cookbook peaking EQ |
| `BLowShelf`, `BHiShelf` | signal, freq (Hz), rs, db | cookbook shelves; `rs` the slope, 1 the steepest |
| `FOS` | signal, a0, a1, b1 | first-order section with its coefficients as inputs, direct form |
| `SOS` | signal, a0, a1, a2, b1, b2 | second-order section with its coefficients as inputs, direct form |
| `Ramp` | signal, time (s) | the input sampled every `time` and joined by straight lines |
| `Slew` | signal, up, down | the input followed at no more than `up` units per second rising and `down` falling |
| `Lag2`, `Lag3` | signal, time | two or three `Lag`s in a row |
| `Lag2UD`, `Lag3UD` | signal, up, down | two or three `LagUD`s in a row (scsynth's `Lag3UD` reads the second stage's comparison for its third; here each stage reads its own) |
| `OnePole` | signal, coef | `y[n] = (1−\|c\|)·x[n] + c·y[n−1]` — lowpass for a positive coefficient, highpass for a negative one. The parameter is the **pole**, not a cutoff |
| `OneZero` | signal, coef | `y[n] = (1−\|c\|)·x[n] + c·x[n−1]` |
| `LeakDC` | signal, coef | DC blocker: a zero exactly at 0 Hz with a pole just inside it |
| `Integrator` | signal, coef | leaky accumulator, `y[n] = x[n] + c·y[n−1]`; `coef` is clamped just inside 1 so it always forgets eventually |
| `DelayN`, `DelayL`, `DelayC` | signal, delaytime (s) | pure delay: no interpolation (rounded to whole samples), linear, or four-point cubic. Plus the static `max_delay` field |
| `CombN`, `CombL`, `CombC` | signal, delaytime (s), decaytime (s) | feedback comb. `decaytime` is the time for the echo train to fall 60 dB **counting from the first echo**, which is the direct path and returns at full level; negative inverts alternate echoes, zero leaves a single one |
| `AllpassN`, `AllpassL`, `AllpassC` | signal, delaytime (s), decaytime (s) | Schroeder allpass: exactly flat magnitude, phase only. Decay as for the comb |
| `BufDelayN/L/C`, `BufCombN/L/C`, `BufAllpassN/L/C` | bufnum, chan, signal, delaytime (s) [, decaytime (s)] | the same nine, over **a channel of a pool buffer** instead of private memory: same interpolation, same feedback, same arithmetic, and the line's contents are **addressable** — inspectable, resamplable, savable, playable by another node while it runs. No `max_delay` field: the buffer's own frame count is the bound. It is **not zeroed** (allocating and clearing are the client's, as in scsynth), a missing buffer or a channel it does not have plays silence, and `bufnum`/`chan` being inputs means a line can be moved between buffers with a `/node_set` |
| `Pluck` | signal, trig, maxdelaytime, delaytime, decaytime, coef | the Karplus-Strong string: a rising `trig` lets one period of `signal` into a line that feeds back through a one-pole lowpass `(1−\|coef\|)·x + coef·y`, falling 60 dB in `decaytime`; the delay is read cubically and held to `maxdelaytime`, a constant that sizes the line (scsynth's is held only to its line rounded up to a power of two). A control-rate delay, decay or `coef` moves in a line across the block |
| `PitchShift` | signal, windowSize, pitchRatio, pitchDispersion, timeDispersion | four read heads a quarter window apart, each reading the line at `pitchRatio` (0 to 4) times the speed it is written, under a triangle a window long; each head's ratio and start moved at random by up to the dispersions. `windowSize` is read at the start; the line holds the longest time dispersion too, which scsynth's does not |
| `DelTapWr` | bufnum, signal | writes `signal` round a mono buffer, cleared at its start, and outputs the frame each sample went to — as a number, exact to 2²⁴ frames, where scsynth's carries the integer's bits |
| `DelTapRd` | bufnum, phase, deltime, interp | reads that buffer `deltime` seconds behind the phase, `interp` 1 none, 2 linear, 4 cubic; every read wraps round the buffer (scsynth's linear read at a steady control-rate delay reads past it) |
| `FreeVerb` | signal, mix, room, damp | Jezar's Freeverb: eight lowpassed combs in parallel into four allpasses in series; `mix` the wet part, `room` the combs' feedback (0.7 to 0.98), `damp` their lowpass. Its lines are its tunings for 44.1 kHz **scaled to the server's rate**, where scsynth's are the same samples at every rate (its room halves at 88.2 kHz) |
| `FreeVerb2` | signal, signal2, mix, room, damp, chan | Freeverb on a pair, both channels' tanks fed the sum, the right's lines 23 samples longer (at 44.1 kHz) |
| `GVerb` | signal, roomsize, revtime, damping, inputbw, spread, drylevel, earlyreflevel, taillevel, maxroomsize, chan | a room `roomsize` metres across: four lines in a Hadamard feedback network falling 60 dB in `revtime`, early reflections tapped before it, each output through allpasses of its own; `maxroomsize` is a constant that sizes the lines. scsynth's first line is −1 samples long unless the room's longest line is already prime (its nearest-prime search truncates its range to 0), a change of room never moves it, and its levels rise from 0 across the first block; here every line follows the room and the levels start at theirs |
| `Spring` | signal, spring, damp | a mass on a spring driven by the force `signal`, its velocity damped by `damp` a sample; outputs the spring's force. scsynth's units: it rings at `√(spring·sampleRate)/2π` |
| `Ball` | signal, g, damp, friction | the height of a ball falling under `g` onto a floor at the height `signal`, bouncing with its velocity damped by `damp`, held by `friction` once close; a bounce adds a dither from the synth's generator |
| `TBall` | signal, g, damp, friction | `Ball`'s velocity at each impact, 0 between |
| `GrainSin` | trigger, dur, freq, pan, envbufnum, maxGrains, numchans, chan | at each rising `trigger`, a grain `dur` seconds long of a sine at `freq`, under the Hann (a `sin²` by a sine recurrence) or the window in buffer `envbufnum` read across it; panned at equal power between the two channels either side of `pan` (−1 to 1 left to right on a pair, a turn round a ring). The parameters are read at the trigger; a parameter scsynth would pull from a demand stream is a signal here (a `Demand` on the same trigger gives it). One row a channel: each keeps the grains in its channel and counts the rest, so `maxGrains` (a constant) is the whole UGen's limit — scsynth's holds one fewer |
| `GrainFM` | trigger, dur, carfreq, modfreq, index, pan, envbufnum, maxGrains, numchans, chan | `GrainSin` of a carrier at `carfreq` deviated by `index·modfreq` at `modfreq` |
| `GrainIn` | trigger, dur, signal, pan, envbufnum, maxGrains, numchans, chan | `GrainSin` of the input while each grain sounds |
| `GrainBuf` | trigger, dur, sndbuf, rate, pos, interp, pan, envbufnum, maxGrains, numchans, chan | `GrainSin` of a mono buffer read from `pos` (0 to 1 of it) at `rate` times its own rate, wrapping round it; `interp` 1 none, 2 linear, 4 cubic |
| `TGrains` | trigger, bufnum, rate, centerPos, dur, pan, amp, interp, numchans, chan | a grain of a mono buffer centred on `centerPos` seconds, under the Hann, at `amp`; at most 64 at once |
| `Warp1` | bufnum, pointer, freqScale, windowSize, envbufnum, overlaps, windowRandRatio, interp, chan | channel `chan` of a buffer stretched in time: grains `windowSize` long (moved at random by up to `windowRandRatio` of it), `overlaps` at once, each read from `pointer` (0 to 1) at `freqScale`; at most 64 at once. scsynth's, once it reaches that limit or misses a window buffer, never starts another grain; here the next is scheduled |
| `GrainTap` | bufnum, grainDur, pchRatio, pchDispersion, timeDispersion, overlap | grains of a delay line in a buffer whose length is a power of two, which a delay writes as this reads: `overlap` grains of `grainDur` at once under a parabola, read at `pchRatio` (moved at random by up to `pchDispersion`) from up to `timeDispersion` seconds back. scsynth's plays a grain started late in a block to the block's end, past its parabola; here it ends where it lasts |
| `Phasor` | trig, rate, start, end, reset_pos | ramp from `start` to `end` advancing by `rate` **per sample** (not Hz), wrapping at `end`; a rising `trig` jumps to `reset_pos`. The index source for a buffer reader: a rate of 1 advances one frame per sample |
| `TransportPos` | offset | the **position of the node's transport** (the one governing its group, or transport 0), in frames, minus `offset` — what a buffer reader follows so that seeking, looping and pausing belong to the transport and not to each reader (`/transport_locate`, `/transport_loop`). Ramps one frame per sample while the transport rolls and holds while it is stopped. `offset` is where these samples start on that axis, so a clip reads its own frame 0 when the transport reaches it — and the subtraction happens in `f64` inside the UGen, which keeps a clip's own position small. The output is still an `f32` signal, which counts single frames only up to 2^24 (about 5.8 minutes at 48 kHz): a buffer reader past that point is `TransportBufRd`, not this into `BufRd` |
| `TransportFade` | — | the **declick level of the node's transport**: `1` while it rolls, `0` while it is stopped, and a straight ramp between the two across a stop and a play, as long as `/transport_fade` says — with no ramp set, simply whether it rolls. An output outside the governed group, in a group that follows the transport, multiplies what the readers wrote by it |
| `BinaryOpUGen` | a, b | one of a table of binary operators, chosen by the `op` **name** — see the operator note below |
| `UnaryOpUGen` | a | one of a table of unary operators, chosen by the `op` **name** — see the operator note below |
| `Add`, `Sub`, `Mul`, `Div` | a, b | sample-wise arithmetic; thin aliases for the `add`/`sub`/`mul`/`div` operators (kept so existing defs are unchanged) |
| `MulAdd` | a, b, c | `a·b + c` in one UGen (the multiply-accumulate the server fuses) |
| `LinExp` | signal, srclo, srchi, dstlo, dsthi | linear in, exponential out, unclipped: `clausters_core::warp`'s `linexp`, the map `RangeMapUGen` and the clients compute |
| `InRange` | signal, lo, hi | 1 where `lo ≤ signal ≤ hi`, else 0 |
| `InRect` | x, y, left, top, right, bottom | 1 where `(x, y)` lies in the rectangle, edges included, else 0 |
| `ModDif` | x, y, mod | the distance from `x` to `y` on a circle of circumference `mod`, the shorter way: in `[0, mod/2]` |
| `Unwrap` | signal, lo, hi | undoes a wrap: a jump of more than half the range adds or takes away a whole range. The range is read at the start and the output starts inside it (scsynth's initial offset takes a start outside further out) |
| `AmpComp` | freq, root, exp | `(root / freq)^exp`, sign-preserving: the gain that evens a higher partial's loudness with a lower one's |
| `AmpCompA` | freq, root, min_amp, root_amp | loudness compensation along the A-weighting (IEC 61672): `root_amp` at `root`, `min_amp` at the weighting's peak, linear in `1 − A(freq)` |
| `K2A` | signal | a control-rate signal at audio rate, a line across each block; an audio-rate input passes |
| `A2K` | signal | an audio-rate signal at control rate: the block's first sample |
| `T2A` | signal, offset | a control-rate trigger at audio rate: at a rising edge, the sample `offset` into the block takes its value |
| `T2K` | signal | an audio-rate trigger at control rate: the block's largest sample, 0 when none is positive |
| `DC` | signal | the constant `signal`, taken at the start and held |
| `Hasher` | signal | the input's bits through scsynth's integer hash, as a value in ±1: noise that is a function of its input |
| `MantissaMask` | signal, bits | the input with only the top `bits` (0..23) of its mantissa kept |
| `Sum3` | a, b, c | `a + b + c` |
| `Sum4` | a, b, c, d | `a + b + c + d` |
| `In` | bus | copies an audio bus (read once per block) |
| `InCtl` | bus | a control-bus value, constant over the block |
| `OutCtl` | bus, signal | writes the signal's per-block value to a **control** bus — the value the block **starts** at, since a reader holds one number for the whole block and the block's last sample would reach a reader running later in that same block a block early (the write side of `InCtl`); passes the signal through as its output |
| `Out` | bus, signal | **sums** the signal into an audio bus |
| `ReplaceOut` | bus, signal | overwrites the bus instead of summing |
| `OffsetOut` | bus, signal | `Out` under scsynth's name: scsynth starts a synth at a block boundary and delays this writer to its sample; here a synth starts at its sample, so the two are one |
| `XOut` | bus, xfade, signal | crossfades into the bus, `bus + xfade·(signal − bus)`: 1 replaces, 0 leaves it; a control-rate `xfade` is a line across the block |
| `InFeedback` | bus | an audio bus read whatever the node order: this block's samples if it has been written this block, else the last block's (the buses keep each written bus's last block) — feedback between synths |
| `InTrig` | bus | a control bus as a trigger: its value in the block the server set it (`/bus_set`, an `OutCtl`), the same value included, or a write through the shared segment changed it; 0 otherwise |
| `LagIn` | bus, lag (s) | a control bus through a one-pole lag to −60 dB |
| `SharedIn`, `SharedOut` | bus [, signal] | read and write a control bus: the server's control buses are the memory another process maps, scsynth's shared controls |
| `PlayBuf` | bufnum, chan, rate, loop, trigger, startPos, doneAction | buffer player with **cubic** interpolation, as scsynth's (at a rate of exactly 1 every frame plays verbatim); `rate` is frames per output sample (1.0 = the server rate — multiply by `BufRateScale(bufnum)`, i.e. `file_sr / server_sr`, for the file's pitch). A pass begins at `startPos` and a rising `trigger` re-cues there mid-play, so one player is a re-usable voice; without `loop`, reaching the last frame (or the first, playing backwards) stops it there, holding that frame, and fires `doneAction` (`2` frees the synth, so a one-shot leaves the tree by itself), and a looping player never finishes, so its action never fires. The last three come **after** `loop` rather than in scsynth's order (`rate, trigger, startPos, loop`): inputs are positional, so putting `startPos` before `loop` would silently re-read every existing `loop` argument as a cue frame |
| `BufRd` | bufnum, chan, phase, loop, interpolation | reads the buffer at a `phase` signal in frames; `interpolation` is scsynth's 1 (none), 2 (linear, the default) or 4 (cubic). Out-of-range phases wrap when looping, clamp otherwise, and the frames past an end that interpolation reads are the other end when looping and the end frame when not |
| `TransportBufRd` | bufnum, chan, offset, rate, start, loop | reads the buffer at the **transport's position**: `BufRd` at `(position - offset) * rate + start` frames, worked out in `f64` inside the UGen. `offset` is as `TransportPos` takes it, `rate` the buffer's frames per transport frame (`BufRateScale`, times a playrate). One UGen rather than `TransportPos` into `BufRd` because a phase passed between UGens is an `f32` signal, which past 2^24 frames repeats and drops frames, so a long take read that way distorts more the further it plays. Seeks, loops and holds with the transport. The audio editor and the multitrack read their takes with it |
| `TransportDiskIn` | bufnum, chan, offset, start, loop | plays a **fed buffer** at the transport's position: the reader of a take an application that holds its engine reads itself and hands over through a ring (the engine knows no file, list or part). Frame `(position - offset) + start` of the stream plays at each sample, as `TransportBufRd` works it out. A locate asks the owner for a refill and is silence until it lands — at most one block measured — and never a stale frame; a forward jump the ring already holds is no silence at all, and a play after the engine's cue has nothing to wait for. **The transport's loop is read straight through**: the owner is told the span, renders from the loop's end on at its start, and the wrap is in the ring before the position gets there, so a loop clicks at no wrap. A loop set, moved or cleared while it plays is the stream read another way, and fades as new contents do. **New contents while it plays are no silence**: the old reading plays on while the new one is read, and once the new one holds the frame the transport is at, the old fades out and the new fades in over 128 frames each. A ring that runs dry plays silence too, counted in `/server_status.reply`'s `starved`. Silent outside the stream, while the transport is stopped and over a buffer that is not fed. A `Voices` node refuses a def that holds one |
| `BufWr` | bufnum, chan, phase, loop, signal | **writes** `signal` into the buffer at `phase` (frames, truncated — no interpolation, which on write would store a value the signal never had); wraps when looping, writes nothing out of range otherwise. Passes `signal` through as its output, like `OutCtl` |
| `RecordBuf` | bufnum, chan, signal, offset, recLevel, preLevel, run, loop, trigger, doneAction | **records** `signal`, advancing one frame per sample — the self-advancing writer, as `PlayBuf` is the self-advancing reader. Each frame becomes `signal·recLevel + old·preLevel`, so `(1, 0)` overwrites, `(1, 1)` overdubs and `(1, 0.5)` overdubs with the older layers fading: that pair is what makes it a looper rather than a tape head. `run` at 0 holds the position and writes nothing; a rising `trigger` re-cues to `offset`; without `loop`, reaching the end stops it and fires `doneAction`. Passes `signal` through |
| `BufSampleRate` | bufnum | the buffer's own sample rate (Hz), block-constant |
| `BufRateScale` | bufnum | `file_sr / server_sr`; feed `PlayBuf`'s `rate` (`rate: BufRateScale(buf) * pitch`) to play at the file's true pitch without the client knowing either rate |
| `BufFrames` | bufnum | frame count, block-constant |
| `BufChannels` | bufnum | channel count, block-constant |
| `BufDur` | bufnum | duration in seconds (`frames / file_sr`), block-constant |
| `BufSamples` | bufnum | the buffer's samples, frames times channels |
| `LocalBuf` | channels, frames | the number of a buffer of the synth's own, allocated with it (both inputs constants), zeroed at each note's start and freed with it; numbered after the pool (`NumBuffers` + its place among the synth's), so every buffer UGen reaches it as it does one of the pool |
| `MaxLocalBufs` | count | scsynth's declaration of the count, which the compiler makes itself: accepted, does nothing, outputs 0 |
| `SetBuf` | bufnum, offset, count, values… | `ir`: writes the values into the buffer from sample `offset` on at the synth's start, as many as fit |
| `ClearBuf` | bufnum | `ir`: zeroes the buffer at the synth's start |
| `ScopeOut` | bufnum, signals… | writes the signals round the buffer, one a channel and a frame a sample, wrapping at its end; output silence. scsynth's waits for its GUI to read each full buffer; here a reader takes what is there |
| `Osc` | bufnum, freq (Hz), phase (rad) | interpolating wavetable oscillator; `bufnum` must hold a **wavetable-format** buffer (see `/buffer_gen` below); `phase` is an offset in radians |
| `OscN` | bufnum, freq (Hz), phase (rad) | non-interpolating oscillator over a **plain** (non-wavetable) buffer; rawer/cheaper than `Osc` |
| `VOsc` | bufpos, freq (Hz), phase (rad) | like `Osc` but the buffer number is a signal: reads wavetables `bufpos` and `bufpos+1` and crossfades by its fractional part, so sweeping `bufpos` morphs a bank of adjacent tables (allocate them contiguously, same size). A control-rate `bufpos` is a line across the block, as scsynth's is |
| `VOsc3` | bufpos, freq1, freq2, freq3 (Hz) | `VOsc`'s morphing table read at three frequencies and summed |
| `COsc` | bufnum, freq (Hz), beats (Hz) | `Osc` twice, at `freq ± beats/2`, summed: two voices beating `beats` times a second; peak twice the table's |
| `Shaper` | bufnum, signal | waveshaper: maps `signal` (in ±1, clamped) through a transfer table in wavetable format (typically `/buffer_gen cheby`); the table's first point is `signal = −1` and the end of its last pair `signal = +1`, as scsynth's |
| `Index`, `WrapIndex`, `FoldIndex` | bufnum, signal | the buffer as a table (its samples in order, channels interleaved): the value at `signal` truncated, the index clipped, wrapped or folded into it |
| `IndexL` | bufnum, signal | the table at a fractional index, linear between neighbours, the index clipped. scsynth interpolates a negative index by its fraction; here below the table is its first value |
| `IndexInBetween` | bufnum, signal | the inverse of `IndexL` over an ascending table: the fractional index where the table reaches `signal`, by scsynth's search from the start |
| `DetectIndex` | bufnum, signal | the index of the first table value equal to `signal`, or −1 |
| `DegreeToKey` | bufnum, signal, octave | a scale degree to a key: the table is one octave of steps, degree `⌊signal⌋` its step plus `octave` per whole octave, below included. scsynth reads past the table at a negative whole octave and truncates `octave`; here the degree divides as a scale does and `octave` is any size |
| `DiskIn` | chan | streams a file from disk (mono per UGen — `chan` picks the channel); needs a `path` field; `start` is the file frame it begins at (default 0); `loop` restarts at the file's top at end of stream; a W64 or an RF64 is read by arithmetic, its start a seek; see streaming note below |
| `VDiskIn` | chan, rate | `DiskIn` at a variable `rate` (frames of the file a sample, none backwards), read with cubic interpolation; the same `path`, `start` and `loop` fields. scsynth's caps the rate by its cue buffer, takes a `sendID` reporting the cue buffers read, and plays the first frame twice; none of that here |
| `DiskOut` | signal | streams `signal` to a mono sound file on disk — a WAV, closed as an RF64 if the take outgrew 4 GB, or a W64 or an RF64 when the `path` ends in `.w64` or `.rf64`; needs a `path` field; `format` is the sample format; passes `signal` through as its output |
| `LocalIn` | channel | reads synth-private feedback channel `channel` (a constant); see feedback note below |
| `LocalOut` | channel, signal | writes `signal` into synth-private feedback channel `channel` (a constant); also passes `signal` through as its own output |
| `EnvGen` | gate, level_scale, level_bias, time_scale, done_action, *envelope array* | breakpoint envelope, at audio or control rate; gate-driven, with a `done_action` that can free the node — see the envelope note below |
| `Line` | start, end, dur, done_action | ramps from `start` to `end` over `dur` seconds, then holds; the same `done_action` set as `EnvGen`, which it is built on |
| `EnvShape` | phase, start, end, shape, curve | **one envelope segment read at a position**: `phase` (clamped to 0..1) through the segment from `start` to `end`, in an `EnvGen` segment's shape number and curvature, by the very function `EnvGen` plays. Where `EnvGen` runs on time from a gate, this follows whatever drives `phase` — a box's fade reads the transport's position through it, so a locate jumps the fade and a stop holds it |
| `IEnvGen` | index, offset, *envelope array* | an envelope **read at a position**: `index − offset`, in the envelope's seconds. The array is scsynth's interpolation layout: level0, segment count, total duration, then per segment duration, shape, curve, level. Before the start the first level, past the end the last. scsynth truncates a curvature to an integer and makes `hold` depend on the index's history; here the curvature is the one given and a held segment is its start level |
| `Linen` | gate, attacktime (s), suslevel, releasetime (s), done_action | control rate only: a straight rise to `suslevel` when `gate` opens, held, a straight fall when it closes, then `done_action`; a gate ≤ −1 releases over `−gate − 1` s. A gate closing during the rise releases from there, where scsynth finishes the rise first |
| `XLine` | start, end, dur, done_action | `Line` in equal ratios instead of equal steps; `start`/`end` must be non-zero and share a sign |
| `FreeSelf` | signal | passes `signal` through; frees the enclosing synth while it is greater than zero |
| `PauseSelf` | signal | passes `signal` through; pauses the enclosing synth while it is greater than zero (resume with `/node_run 1`) |
| `Done` | source | 1 once the ugen wired into `source` has **finished**, 0 before — see the done-flag note below |
| `FreeSelfWhenDone` | source | passes `source` through and frees the synth once it has finished |
| `PauseSelfWhenDone` | source | passes `source` through and pauses the synth once, when `source` finishes; resumed, it is not paused again (scsynth's outputs 0 from then on) |
| `Free` | trig, id | at each rising edge of `trig`, frees node `id` (any node, this one included); passes `trig` through. Applied with the done actions at the block's end |
| `Pause` | gate, id | runs node `id` while `gate` ≠ 0 and pauses it while 0, acting when the gate changes; passes `gate` through |
| `PinkNoise` | — | equal energy per octave, −3 dB/octave (Voss–McCartney); a **quiet** signal, ~0.13 RMS |
| `BrownNoise` | — | a random walk, −6 dB/octave; reflects at ±1 rather than clamping |
| `GrayNoise` | — | one random bit of a 31-bit word flipped per sample; leans low (~−2.9 dB/octave) and steps by wildly unequal amounts |
| `ClipNoise` | — | −1 or 1 only, a coin flip per sample |
| `LFNoise0` | freq | a new random value in ±1 every `1/freq` seconds, held (steps) |
| `LFNoise1` | freq | the same, ramped linearly between values |
| `LFNoise2` | freq | the same, quadratic between values — no corners, and it **overshoots** to about ±1.7 |
| `LFClipNoise` | freq | `LFNoise0` restricted to ±1 |
| `LFDNoise0`, `LFDNoise1`, `LFDNoise3`, `LFDClipNoise` | freq | the dynamic noises: held, linear, cubic (values within ±0.8, so within ±1) or ±1, their position a phase per sample, so a change of `freq` takes effect at once. scsynth's, its phase in `f32`; a negative `freq` is its magnitude, where scsynth's freezes |
| `Logistic` | chaos_param, freq, init | the logistic map `y = r·y·(1 − y)` iterated `freq` times a second from `init`, held between; scsynth's constructor iterates once before the first block, so its output starts an iterate later |
| `CuspN`/`L`, `GbmanN`/`L`, `HenonN`/`L`/`C`, `LatoocarfianN`/`L`/`C`, `LinCongN`/`L`/`C`, `LorenzL`, `QuadN`/`L`/`C`, `StandardN`/`L`, `FBSineN`/`L`/`C` | freq, *the map's parameters*, *its initial conditions* (scsynth's order) | scsynth's chaotic maps, one engine: the map stepped `freq` times a second (every sample at the rate and above), its value held (`N`), joined by lines (`L`) or by a cubic two steps late (`C`); a new initial condition restarts the map. Where scsynth's variants of one map disagree the theory decides: its `HenonN`/`L` iterate with the initial values exchanged (here every Henon starts at `x1`, `x0` the value before), its cubic forms and `LinCongL` start their first period off the map, and a diverged Henon holds its last value |
| `Gendy1`, `Gendy2` | ampdist, durdist, adparam, ddparam, minfreq, maxfreq, ampscale, durscale, init_cps, knum [, a, c] | Xenakis's dynamic stochastic synthesis: `knum` control points a cycle (`init_cps` in memory, a constant at build), each level and duration walking by a distribution (0 linear … 6 the parameter), reflected into ±1 and [0, 1]; `Gendy2` walks its steps too. scsynth's `Gendy2` reflects a level below −1 out of range; here every value stays in range |
| `Gendy3` | ampdist, durdist, adparam, ddparam, freq, ampscale, durscale, init_cps, knum | `Gendy1`'s walks at a fixed pitch: the durations are proportions of a `1/freq` cycle, every cycle starting at 0. scsynth truncates each segment's interpolation factor to an integer and crosses one point a sample; here the lines reach their points |
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
| `Pan4` | signal, xpos, ypos, level, chan | a source on a square of four channels (`chan` 0 left front, 1 right front, 2 left back, 3 right back), equal power across and front to back; a position off the square is carried to its edge along the line from the centre |
| `PanB` | signal, azimuth, elevation, level, chan | first-order B-format (`chan` 0 W, 1 X, 2 Y, 3 Z), Furse-Malham's weights: W `1/√2`, X `cos·cos`, Y `−sin(az)·cos(el)`, Z `sin(el)`. `azimuth` is in half turns (±1, clockwise), `elevation` in quarter turns (±1) — scsynth's units, which its help calls radians. The trigonometry is computed where scsynth reads a table at a truncated index |
| `PanB2` | signal, azimuth, level, chan | `PanB` in two dimensions: W, X, Y |
| `BiPanB2` | a, b, azimuth, level, chan | `a` at the azimuth and `b` opposite it, in two-dimensional B-format |
| `DecodeB2` | w, x, y, orientation, numchans, chan | speaker `chan` of a ring of `numchans`, clockwise from the front: `W/√2 + (X·cos t + Y·sin t)/2` at its angle `t = 2π (orientation − chan)/numchans`, a cardioid towards it; read at the start |
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
| `Peak`, `RunningMax`, `RunningMin` | signal, trig | the largest `|x|`, the largest `x`, the smallest `x` since the last rising edge of `trig`, starting over from the sample it rises on (that sample's output still the old extreme). A control-rate trigger rises at the block's last sample, scsynth's. At control rate over audio the whole block is measured — scsynth's `Peak` (its SIMD form, the one it runs), which here `RunningMax`/`RunningMin` share where scsynth's read one sample |
| `PeakFollower` | signal, decay | `|x|` held at each peak, falling back by `y = x + decay·(y − x)` a sample; a control-rate `decay` is a line across the block. scsynth uses three formulas by the decay's sign and moves its line only on falling samples |
| `RunningSum` | signal, numsamp | the sum of the last `numsamp` samples (sized at build), restarted from the exact window sum at each window's end |
| `LastValue` | signal, diff | the value the input had before it last moved by at least `diff` |
| `MostChange`, `LeastChange` | a, b | whichever input moved more (less) since the last sample; on a tie the one chosen last |
| `ZeroCrossing` | signal | the frequency of the rising zero crossings, each placed within its sample, held between; crossings under five samples apart are not counted |
| `Lag` | signal, time | one-pole smoother: `signal` lagged over `time` seconds (symmetric); `time` 0 passes through; primed to the first input (no glide up from 0) |
| `LagUD` | signal, up, down | one-pole smoother with separate rise (`up`) and fall (`down`) times; what a control with a `lag_down` inserts |
| `VarLag` | signal, time, start | a **straight line** to each new value of `signal`, taking `time` seconds, from `start` at the first sample; a new `time` rescales what is left of the line. Arrives exactly, where the one-poles approach |
| `SampleRate` | — | the engine sample rate in Hz; init-rate (`ir`) by default |
| `BlockSize`, `ControlRate`, `ControlDur`, `SampleDur`, `RadiansPerSample` | — | the engine's timing: 64, the blocks a second, a block's and a sample's seconds, `2π/sr`; initial rate by default |
| `SubsampleOffset` | — | where within its first sample the timed bundle that started the synth fell, scsynth's `time + ½ − ⌊time + ½⌋` in samples; 0 for an untimed start |
| `NumAudioBuses`, `NumControlBuses`, `NumInputBuses`, `NumOutputBuses`, `NumBuffers` | — | the server's configuration: its buses, its hardware inputs and outputs, its buffer slots |
| `NodeID`, `NumRunningSynths` | — | the node the synth is, and the synths in the tree (at control rate, following it) |
| `Rand` | lo, hi | one uniform random value in `[lo, hi)`, drawn once at synth init and held; init-rate only (its inputs must be constants/`ir`) |
| `IRand` | lo, hi | one integer in `[lo, hi]` (bounds truncated), drawn once and held; `ir` |
| `ExpRand` | lo, hi | one log-uniform value between `lo` and `hi` (one sign, nonzero), drawn once and held; `ir` |
| `LinRand` | lo, hi, minmax (optional) | one value in `[lo, hi)`, the lower of two draws (the higher when `minmax > 0`), drawn once and held; `ir` |
| `NRand` | lo, hi, n | one value in `[lo, hi)`, the mean of `n` draws, drawn once and held; `ir` |
| `TRand`, `TIRand`, `TExpRand` | lo, hi, trig | a uniform, integer or log-uniform value drawn at the start and again at each rising edge of `trig`, held between |
| `CoinGate` | prob, trig | each rising edge of `trig` passes (its own value, for that sample) with probability `prob`; 0 otherwise |
| `TWindex` | trig, normalize, *weights* | at each rising edge of `trig` (and at the start), an index drawn with the weights' probabilities, divided by their sum with `normalize` 1. What unnormalized weights leave goes to the last index, where scsynth answers one past the weights |
| `RandID` | id | chooses which of the server's 64 generators the synth's random UGens draw from, from its place in the graph on (0 until it runs); outputs 0 |
| `RandSeed` | trig, seed | at each rising edge of `trig`, reseeds the generator the synth draws from with `seed` (at `ir`, once at the start), so every synth drawing from it restarts the same sequence; outputs 0 |
| `Demand` | trig, reset, source | demand driver: pulls the next value from `source` on each rising edge of `trig`, holds it between triggers; a rising `reset` restarts the stream; 0 before the first trigger, the last value once the stream ends — see the demand note below |
| `Duty` | dur, reset, level, done_action | demand driver with a clock of its own: pulls one `level` every `dur` seconds and holds it. Both `dur` and `level` are pulled, so a stream of durations against a stream of values is a sequencer; when either ends, `done_action` fires |
| `TDuty` | dur, reset, level, done_action, gap_first | `Duty` emitting each level on its own sample and nothing in between — a trigger stream whose amplitudes are the levels. `gap_first` spends the first duration before the first level, so it opens with a gap |
| `DemandEnvGen` | level, dur, shape=1, curve=0, gate=1, reset=1, level_scale=1, level_bias=0, time_scale=1, done_action=0 | an envelope whose breakpoints are streams: each segment pulls its duration, shape, curvature and end level (`EnvGen`'s shapes) and runs from wherever the output is. A `gate` ≥ 1 runs, between 0 and 1 releases at the segment's end (holding there, `done_action` firing, until the gate is back at 1), ≤ 0 holds; a rising `reset` restarts the streams, gliding on from the current level up to 1 and jumping to the first level above it; when the duration or the level stream ends it holds and `done_action` fires |
| `Dseq` | repeats, v0, v1, … | demand source: yields the value list in order, `repeats` passes (`≤ 0` endlessly). A value that is itself a stream is **drained**, not taken once, and restarted when the sequence comes round to it |
| `Drand` | repeats, v0, v1, … | `repeats` **items** drawn at random from the list, each pick independent |
| `Dxrand` | repeats, v0, v1, … | `Drand` that never picks the value it just used |
| `Dshuf` | repeats, v0, v1, … | the list shuffled **once** and replayed in that order, `repeats` passes; the shuffle is redrawn on a reset, not per pass |
| `Dser` | repeats, v0, v1, … | the list in order, like `Dseq`, but `repeats` counts **items**: `Dser(5, 1, 2, 3)` is `1 2 3 1 2` |
| `Dwrand` | repeats, size, w0 … w(size-1), v0 … v(size-1) | `repeats` items drawn at random, each value as likely as its share of the weights (which need not sum to 1) |
| `Dseries` | repeats, start, step | arithmetic sequence `start`, `start + step`, …; `step` is read per item, so it may be a stream |
| `Dgeom` | repeats, start, grow | geometric sequence `start`, `start * grow`, … |
| `Dwhite` | repeats, lo, hi | independent uniform draws on `[lo, hi]` |
| `Diwhite` | repeats, lo, hi | `Dwhite` over the integers in `[lo, hi]`, both ends included |
| `Dbrown` | repeats, lo, hi, step | random walk of at most `step` per item, **folded** into `[lo, hi]` (it turns around at a bound rather than piling up against it) |
| `Dibrown` | repeats, lo, hi, step | `Dbrown` over the integers |
| `Dstutter` | repeats, value | repeats each item of the `value` stream `repeats` times; the count is pulled per item, so it may vary |
| `Dswitch1` | which, v0, v1, … | takes **one** item from the stream `which` picks, then picks again; an unselected stream is not advanced and the selected one is not drained. The index wraps |
| `Dbufrd` | bufnum, phase, loop, channel | reads the buffer frame the `phase` stream names — a `Dseries` phase walks a buffer as a step sequence; out of range it wraps when `loop` is set and clamps when it is not |
| `Dbufwr` | bufnum, phase, signal, loop=1, channel=0 | writes each item of the `signal` stream into the buffer frame the `phase` stream names, and yields it; either stream ending ends it; out of range it wraps when `loop` is set and clamps when it is not |
| `Ddup` | repeats, value | `Dstutter`'s newer name, the same stream |
| `Dswitch` | which, v0, v1, … | plays the **whole** stream `which` picks, then picks again; a plain number is a stream of one item, and a stream picked again plays again from its start. The index wraps |
| `Dconst` | sum, signal, tolerance=0.001 | yields `signal` until its items add up to `sum`, the last cut to land on it; within `tolerance` of the sum counts as there |
| `Dreset` | signal, reset=0 | passes the `signal` stream through and restarts it when the `reset` stream rises; the restart shows on the next item |
| `Dpoll` | signal, trig_id=-1, run=1 | passes the `signal` stream through and, while `run` > 0, posts `label: value` for every item (a static `label` field) and, when `trig_id ≥ 0`, sends `/node_trigger nodeID trig_id value` |
| `Meter` | signal, decay=20, hold=0 | a level with a **meter's ballistics**: instantaneous attack, a fall of `decay` decibels per second, and a new peak held `hold` seconds before it begins to fall. One number a block (the block's own peak goes in, the whole block comes out at what the meter reads), which is exactly what a control bus carries — pair it with `OutCtl` for a meter a host reads as a bus. `hold` of 0 is the ordinary level and a second or two is the mark that waits to be read, so two instances give both halves of a meter. The arithmetic is `clausters_core::measure::Ballistics`, shared so two clients cannot draw two different falls off one signal |
| `ClipCount` | signal, ceiling=1, run=3 | **how many times the signal was flattened** since the pass began: a run of `run` consecutive samples at or over `ceiling` is one over, however long it goes on. A single sample at full scale is not clipping — the engine is floating point, so nothing is destroyed until a conversion, and what a red lamp reports is a flattened waveform, whose signature is a run; 3 is the field's number and `run=1` the pessimistic meter. It walks **samples**, not blocks, which is the whole of what a block peak has already lost. The count and not a flag, because the flag is a reader's state (a `meter` widget's lamp, cleared by hand) while the count is the signal's: pair it with `OutCtl` and hand the bus to a `meter`'s `clip`, which differences it. `resume` restarts the count, so a new transport pass puts the lamps out. The arithmetic is `clausters_core::measure::ClipCount` |
| `TruePeak` | signal, decay=20, hold=0 | a **meter's level over the reconstructed signal**: the same ballistics as `Meter`, but what goes in is the block's **true peak** rather than its largest sample — the ITU-R BS.1770-4 Annex 2 filter at 4×, with its context kept across blocks, so the reading is in **dBTP** and never below `Meter`'s on the same signal (a tone whose samples all sit at full scale can be three decibels over it between them). It walks the input's samples at either rate, and `resume` forgets both the held peak and the filter's context. Pair it with `OutCtl` and a `meter` widget at `rate: "control"` with `peak: "true"`, which reads it against -1 dBTP. The arithmetic is `clausters_core::resample::TruePeakMeter` |
| `SendTrig` | trig, id, value | on each trigger of `trig`, sends `/node_trigger nodeID id value` to `/server_notify` clients; output is silence — see the side-effect note below |
| `SendReply` | trig, reply_id, value0, value1, … | on each trigger of `trig`, sends a custom OSC message `cmdName nodeID reply_id value…` (the `cmdName` is a static `label` field, default `/reply`); output is silence |
| `SendPeakRMS` | reply_rate, peak_lag, reply_id, *signals* (label: cmd) | `reply_rate` times a second, `cmd nodeID replyID` and each signal's peak and RMS over the interval, the peak held and falling back by `peak_lag` s to −60 dB; up to eight signals, audio and control rate mixed. The command is the static `label`, as `SendReply`'s |
| `Poll` | trig, signal, trig_id | on each trigger of `trig`, posts `label: value` (the `signal` value) to the server console (a static `label` field) and, when `trig_id ≥ 0`, also sends `/node_trigger nodeID trig_id value`; passes `signal` through as its output |
| `CheckBadValues` | signal, id=0, post=2 | the class of every sample: 0 an ordinary number (zero included), 1 a NaN, 2 an infinity, 3 a denormal. `post` 0 posts nothing, 1 a line at every bad sample, 2 a line whenever the class changes, with how long the previous one lasted (logged at `warn`) |
| `Sanitize` | signal, replace=0 | `signal` with every NaN, infinity and denormal replaced by `replace` |
| `FFT` | source, active | opens a spectral chain: windows `source` and transforms it to a spectral frame once per hop (`active`, read per sample, gates the input: `≤ 0` is analysed as silence, so the round trip is the gated input exactly, and a frame whose whole window was gated is not taken — the chain does no work while it stays off, and reads as silence to a combiner); static fields `fft_size` (a power of two from 64 to 32768, default 1024), `winsize` (the frame of signal windowed, at least one 64-sample block and at most `fft_size`, zero-padded to it; default `fft_size`), `hop` (fraction of the frame in (0, 1], at least one 64-sample block; default 0.5), `wintype` (default 0 = Hann; scsynth's numbers differ, see the FFT-chain note); the chain's window is also settable live via `/node_ugenCmd` to the `FFT` — see the FFT-chain note below |
| `FFTTrigger` | — | opens a chain whose frames are **written**, not analysed: marks the frame fresh once per hop and leaves its bins to what follows (`PackFFT`, a `PV_Kernel` program); the frame starts silent and keeps what is written into it; the static fields are `FFT`'s, and its `IFFT` normalizes by the synthesis window alone, so a bin of magnitude `m` plays a partial of `2m/fft_size` |
| `PV_MagAbove` | chain, threshold | passes only bins whose magnitude is **above** `threshold`, zeroing the rest; `chain` is the wire from an earlier `FFT`/`PV_*` |
| `PV_MagBelow` | chain, threshold | passes only bins whose magnitude is **below** `threshold` |
| `PV_BrickWall` | chain, wipe | brick-wall band limit: `wipe > 0` zeroes that fraction of the bins between DC and Nyquist from the bottom up, DC with them (a high pass), `wipe < 0` from the top down, Nyquist with them (a low pass), `0` passes everything (`wipe` in −1..1; scsynth's direction) |
| `PV_MagClip` | chain, threshold | limits each bin's magnitude **to** `threshold` (louder bins are scaled down, phases kept) |
| `PV_Add` | chainA, chainB | two-chain combiner: per-bin complex sum, result in chain A — see the combiner note below |
| `PV_Mul` | chainA, chainB | two-chain combiner: per-bin complex product |
| `PV_Min` | chainA, chainB | two-chain combiner: per bin, whichever input has the **smaller** magnitude |
| `PV_Max` | chainA, chainB | two-chain combiner: per bin, whichever input has the **larger** magnitude |
| `PV_MagMul` | chainA, chainB | two-chain combiner: A's bins scaled by B's magnitudes (A's phases kept) |
| `PV_CopyPhase` | chainA, chainB | two-chain combiner: A's magnitudes with B's phases |
| `PV_MagFreeze` | chain, freeze | while `freeze ≤ 0` stores each frame's magnitudes and passes through; while `> 0` rescales every bin to the stored magnitude (phases keep running); the first frame is always stored |
| `PV_MagSmear` | chain, bins | averages each bin's magnitude over `bins` neighbors on each side (`0` is transparent), phases untouched |
| `PV_BinShift` | chain, stretch, shift, interp | remaps bin `b` to `round(b·stretch + shift)`: colliding bins sum, out-of-range bins are dropped; with `interp > 0` a bin is split linearly between the two bins around `b·stretch + shift` |
| `PV_MagShift` | chain, stretch, shift | the same remap applied to the magnitude envelope only, over the frame's original phases |
| `PV_Kernel` | chain, p0, p1, … | applies user-written **bin expressions** to every bin of each fresh frame: static fields `mag_expr`/`phase_expr` are postfix token lists mapping one bin's values to its new magnitude / phase; the variadic `p0…` inputs are parameters the expressions read, sampled at the hop — see the bin-expression note below |
| `PV_MagSquared` | chain | squares every bin's magnitude, phases (DC's sign included) kept |
| `PV_MagNoise` | chain | multiplies every bin by its own random factor in [−1, 1), drawn each frame (the inner bins, then DC, then Nyquist: scsynth's order) |
| `PV_Conj` | chain | the complex conjugate of every bin between DC and Nyquist |
| `PV_PhaseShift90` / `PV_PhaseShift270` | chain | every bin between DC and Nyquist turned a quarter turn forward / back |
| `PV_PhaseShift` | chain, shift, integrate | every bin between DC and Nyquist turned by `shift` radians; with `integrate > 0` the shift sums frame to frame (kept in [0, 2π)) |
| `PV_Div` | chainA, chainB | two-chain combiner: per-bin complex quotient A / B, as A times the conjugate of B over B's squared magnitude plus 1e-20, so a silent bin of B gives 0 |
| `PV_MagDiv` | chainA, chainB, zeroed | two-chain combiner: A's magnitudes divided by B's, each floored at `zeroed` (default 0.0001), A's phases kept |
| `PV_BinWipe` | chainA, chainB, wipe | two-chain combiner: A with a run of B's bins, chosen as `PV_BrickWall` chooses the bins it zeroes |
| `PV_RectComb` | chain, num_teeth, phase, width | zeroes the bins between the teeth of a comb over the bins: `num_teeth` teeth from DC to Nyquist, starting at `phase` and each `width` of a period wide (both fractions of a period) |
| `PV_RectComb2` | chainA, chainB, num_teeth, phase, width | two-chain combiner: the same comb, the bins between the teeth taken from B |
| `PV_RandComb` | chain, wipe, trig | zeroes a random `wipe` fraction of the bins between DC and Nyquist (all of them, DC and Nyquist too, at 1), in an order drawn at the first frame and again at each `trig` |
| `PV_RandWipe` | chainA, chainB, wipe, trig | two-chain combiner: a random `wipe` fraction of the bins taken from B, in the same kind of order; at 1 it is B |
| `PV_BinScramble` | chain, wipe, width, trig | moves a random `wipe` fraction of the bins between DC and Nyquist to random places, each from up to `width` (a fraction of the bins) away; the arrangement is drawn at the first frame and again at each `trig` |
| `PV_Diffuser` | chain, trig | turns every bin between DC and Nyquist by its own random phase, drawn at the first frame and again at each `trig` |
| `PV_LocalMax` | chain, threshold | keeps the bins no neighbor of which is louder and at least `threshold`, zeroing the rest |
| `PV_ConformalMap` | chain, areal, aimag | maps each bin `z` between DC and Nyquist to `(z − a) / (1 − conj(a)·z)`, `a = areal + i·aimag` (a denominator's squared modulus held at 0.001 at least) |
| `PV_Copy` | chain | a chain of its own, a copy of `chain` with its shape: one analysis feeding two branches. It copies at its place in the graph, each fresh frame |
| `PackFFT` | chain, frombin, tobin, zeroothers, mag0, phase0, … | writes bins `frombin` to `tobin` (and no further than the pairs given) from the pairs that follow, read at each frame; DC and Nyquist take `mag·cos(phase)`; with `zeroothers > 0` every other bin is zeroed |
| `Unpack1FFT` | chain, binindex, whichmeasure | a control-rate value: the magnitude (`whichmeasure` 0) or the phase (1) of bin `binindex`, updated at each fresh frame; DC's and Nyquist's magnitude is their absolute value and their phase 0 or π. It reads the chain and its output is not one |
| `IFFT` | chain | closes a spectral chain: inverse-transforms each fresh frame and overlap-adds it back to audio; `fft_size`/`winsize`/`hop`/`wintype` are inherited from the chain's first UGen (given only there) |
| `Conv` | source, kernel | partitioned convolution against a **prepared** kernel buffer (`/buffer_gen prepare_partconv`); static fields `fft_size` (transform size; the partition — and intrinsic latency — is `fft_size/2`) and `partitions` (the longest kernel accepted, in partitions, 1–256, default 16 — a longer kernel is refused, not cut); see the convolution note below |
| `PartConv` | source, fftsize, irbufnum | `Conv` with scsynth's inputs: `fftsize` is a constant (the transform size), `irbufnum` the prepared kernel; static field `partitions`, as on `Conv` |
| `Convolution2` | source, kernel, trigger, framesize | convolves `source` with the samples of buffer `kernel` (channel 0, at most `framesize` of them, a longer one refused with a fault), read at the start and again at each `trigger`, switching at the next frame; `framesize` is a constant power of two from 32 to 16384, and the latency |
| `Convolution2L` | source, kernel, trigger, framesize, crossfade | `Convolution2`, fading to a reloaded kernel over `crossfade` frames (read at the trigger) |
| `StereoConvolution2L` | source, kernelL, kernelR, trigger, framesize, crossfade, chan | one channel of a stereo `Convolution2L`: channel `chan` (0 or 1) convolves with `kernelL` or `kernelR`; a builder makes both rows |
| `Convolution3` | source, kernel, trigger, framesize | direct convolution in time with the samples of buffer `kernel` (channel 0, at most `framesize` of them, a constant up to 65536), read at the start and at each `trigger`: no latency, a cost per sample of the kernel's length. `ar` or `kr` |
| `Convolution` | source, kernel, framesize | convolves two signals frame by frame: each `framesize` samples of `source` with the same samples of `kernel`, overlap-added (a cross-synthesis); `framesize` is a constant power of two from 32 to 16384, and the latency |
| `SpecCentroid` | chain | the spectrum's centre of mass in hertz, every bin (DC included) weighted by its magnitude; held between frames, 0 for silence. It reads the chain, and its output is a value, as every row down to `BeatTrack`'s is |
| `SpecFlatness` | chain | the geometric mean of the magnitudes over their arithmetic mean (a bin at 0 is left out of the geometric mean): 1 for white noise, near 0 for a few partials, 0.8 for silence |
| `SpecPcile` | chain, fraction, interpolate, binout | the frequency below which `fraction` of the magnitude lies, in hertz or (`binout > 0`) as a bin number, between two bins when `interpolate > 0` |
| `Loudness` | chain, smask, tmask | loudness in sones: power in 42 bands of about an ERB, masked across bins (`smask`) and in time (`tmask` phons a frame), through equal-loudness contours; the bands at their frequencies and the power at one calibration whatever the chain's size, window and the rate; silence 0 |
| `MFCC` | chain, numcoeff, chan | coefficient `chan` (from the first) of `numcoeff` (1–42, constants) mel-frequency cepstral coefficients, over 42 mel bands from 80 Hz to 18 kHz, scaled into about 0–1; a builder makes the `numcoeff` rows |
| `KeyTrack` | chain, keydecay, chromaleak | the key: 0–11 major from C, 12–23 minor; a chroma from six partials of 60 notes (A1–G#6) matched against diatonic profiles, its memory falling to 1% over `keydecay` seconds at the chain's own hop. A 4096-point chain resolves the low notes |
| `Onsets` | chain, threshold, odftype, relaxtime, floor, mingap, medianspan, whtype, rawodf | 1 on the frames an onset starts in (held to the next frame), by OnsetsDS: adaptive whitening, a detection function (`odftype` 0 power, 1 magnitude sum, 2 complex, 3 rectified complex, 4 phase, 5 weighted phase, 6 modified Kullback-Leibler) less its running median, past `threshold` and `mingap` frames after the last; `rawodf > 0` gives the function. `odftype` and `medianspan` are constants |
| `PV_HainsworthFoote` | chain, proph, propf, threshold, waittime | `ar`: 1 over the slice of a frame whose weighted Hainsworth measure (the mean `log2` rise between 30 Hz and 5 kHz) and Foote measure (one minus the cosine similarity with the last frame) pass `threshold`, then none for `waittime` s |
| `PV_JensenAndersen` | chain, propsc, prophfe, prophfc, propsf, threshold, waittime | `ar`: the same over the frame-to-frame changes of the spectral centroid, the energy above 4 kHz, the high-frequency content and the flux |
| `BeatTrack` | chain, lock, chan | the beat, by Davies and Plumbley's context-dependent tracker: `chan` 0 a trigger each beat, 1 each half beat, 2 each quarter, 3 the tempo in beats a second; `lock > 0.5` holds the outputs. Tuned to frames of about 11.6 ms (1024 points at a half hop); its seconds are the chain's own hop. A builder makes the four rows |
| `BeatTrack2` | busindex, numfeatures, windowsize, phaseaccuracy, lock, weightingscheme, chan | the beat of `numfeatures` features on the control buses from `busindex`, by template matching twice a second over the last `windowsize` s, 60–179 beats a minute: `chan` 0–2 the triggers, 3 the tempo, 4 the phase, 5 the groove. `weightingscheme` -2.5 flat, -1.5 compensated, or a buffer of 120 weights; `numfeatures`, `windowsize`, `phaseaccuracy` and `weightingscheme` are constants. Its bus role is `read_controls` |
| `Pitch` | signal, init_freq, min_freq, max_freq, exec_freq, max_bins_per_octave, median, amp_threshold, peak_threshold, down_sample, clar, chan | `kr`: `chan` 0 the frequency of `signal` by autocorrelation (the first peak above `peak_threshold` of the zero lag, refined by a parabola, through a median of `median`), 1 whether it has one (or its clarity with `clar > 0`); every input but `signal` is a constant, and a control-rate `signal` is taken once a block |

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

**Noise.** Every random source is scsynth's algorithm drawing from scsynth's generator — the one its synth selects from the server's table (see `/server_seed`): two `WhiteNoise` ugens in one def are two runs of one stream drawn in turn, not one summed with itself (which would be a comb filter, not more noise), and a seed replays exactly, which is what lets a noisy patch have a golden file. A source here starts one sample's draws ahead of scsynth's, whose constructor computes a sample before the first block; otherwise the numbers are scsynth's. Three of these are worth reading twice before use: `PinkNoise` is **quiet** by construction (~0.13 RMS against white's 0.58) and, as scsynth's, sits at a DC of 1/16 — the level and offset a def ported from sclang expects; `Dust` is **not a clock** (each sample is an independent trial, so its `density` is a mean and its gaps are exponential — use `Impulse` for even spacing); and `Crackle` is a chaotic map from a random start, deterministic after it, whose parameter changes the sound drastically and not monotonically, and whose output is one-sided and therefore carries DC.

**Panning and the stereo field.** A ugen has **one output** — an input reference names a ugen, not an output of one, the same deviation the buffer readers already carry. So a two-channel panner is **two rows** sharing their inputs and differing only in a trailing `chan` index (0 left, 1 right), and `PanAz` is `numchans` of them, each told the ring size and its own place on it. A client builds the set and hands back a channel list: `pan2(sig, pos)` in the Python client returns two channels, exactly as `out(0, …)` already accepts.

Two laws run through the family. **Equal power** (`Pan2`, `Balance2`, `XFade2`, `SelectX`, and `PanAz`'s lobe) holds `l² + r² = 1`, which keeps one loudness while a source crosses the field — at the price of 0.707 in each channel at the centre, and of a 3 dB lift when the two sides are the *same* signal. **Constant amplitude** (`LinPan2`, `LinXFade2`) holds `l + r = 1` instead: the right choice when the two sides are correlated or when a mono fold-down has to stay put. `Balance2` is the row that surprises people: it applies the *pan* law to a pair that is already stereo, so passing a signal through a centred `Balance2` costs 3 dB. That is scsynth's behaviour and is kept.

The law itself is **computed, not looked up**: a fifth-term polynomial in the position, worst-case error 2.6·10⁻⁷ against scsynth's rounded 2049-entry sine table's 3.8·10⁻⁴, and exact at both ends — a hard pan is digital silence on the far side, not −110 dB of it. It is evaluated **per sample** whenever the position is audio rate, rather than interpolated across the block like a filter coefficient: a position that sweeps a whole block would otherwise read 0.5 where the law wants 0.707, a 3 dB hole in the middle of the block.

`Rotate2`, `MidSide` and `StereoWidth` are the same two-by-two matrix under three parameterizations, and only the first is scsynth's name. A **rotation** moves the image without resizing it; **width** scales the side axis, resizing it without moving it, and no angle produces that. `Rotate2` at a quarter turn *is* the change of basis between left/right and mid/side, which is what `MidSide` names outright — normalized so one kind both encodes and decodes, and so the round trip is exact. Reach for `StereoWidth` for a width knob; reach for `MidSide` when something has to happen *between* the encode and the decode (the centre of a mix filtered apart from its sides), which is the case a single knob cannot express.

`Select`/`SelectX` choose what is **heard**, never what is computed: every source is a ugen in the graph and runs whether or not it is selected. An index off either end holds the source at that end rather than wrapping — including `SelectX`, where sclang's pseudo-ugen instead folds the crossfade and returns a mix of the first two sources for a negative index, or the last one at 1.414 past the end.

**Triggers.** A **trigger** is a signal crossing from `<= 0` up to `> 0` — one definition, the same for every ugen that takes one (`Demand`, `SendTrig`, `EnvGen`'s gate, and the whole family above). Two things about that family are worth knowing before you pick a threshold or a rate. `Changed` compares the **halved** difference, `|(x[n] - x[n-1]) / 2|`, because sclang builds it out of `HPZ1` whose gain is 0.5 and a ported def must not change value: a step of 0.2 registers against a threshold of 0.09, not of 0.19. And they all default to **`ar`**, counters included, even though a counter can only move when a trigger does: a `kr` ugen reads **one sample per block** from an `ar` input, so a `kr` counter fed an `ar` trigger train sees one trigger in 64 and silently drops the rest. `kr` is the right choice when the trigger is `kr` too — the saving is real then, and a duration means seconds at either rate.

**Freeing on a trigger (`FreeSelf`/`PauseSelf`).** The counterpart for when what ends a note is not an envelope: both pass their input through and act while it is greater than zero. Neither latches — the action is reported for the block just processed — which is what makes `PauseSelf` a gate rather than a one-way door: `/node_run 1` really resumes the node, and it re-pauses only if its input is still up.

**Operator UGens (`BinaryOpUGen`/`UnaryOpUGen`).** Rather than a distinct kind per math operation, one generic UGen per arity carries the operator as a static **`op`** field (alongside `kind`/`inputs`) — the operator's **name**. So `{"kind": "BinaryOpUGen", "op": "mul", "inputs": [a, b]}` multiplies. Every operator is one entry in `clausters_core::builtins` — the **same code** the client's value FFI runs off the audio thread — so a value a client computes ahead of time and the UGen on the audio thread are **bit-identical** for the native ops. A missing or unknown `op` fails the def with `/fail` naming the node. The `Add`/`Sub`/`Mul`/`Div` kinds remain as aliases for the `add`/`sub`/`mul`/`div` operators. (Internally each operator also has a stable integer id for the C ABI, but that is an implementation detail — defs and clients only ever use the name.)

Binary operators (`BinaryOpUGen` `op`): `add` (`a+b`), `sub` (`a−b`), `mul` (`a·b`), `div` (`a/b`), `mod` (floored: the result takes `b`'s side, so `mod(-1, 3)` is 2; a zero `b` gives 0), `pow` (`aᵇ`, **signed**: `−((−a)ᵇ)` for a negative `a`, so a bipolar signal stays bipolar), `min`, `max`, `atan2`, `gt` (`a>b`), `lt` (`a<b`), `ge` (`a>=b`), `le` (`a<=b`), `eq`, `ne`, `bitand`, `bitor`, `bitxor`, `lshift`, `rshift`, `hypot` (`√(a²+b²)`, without overflowing on the squares), `ring1` (`ab+a`), `ring2` (`ab+a+b`), `ring3` (`a²b`), `ring4` (`a²b−ab²`), `sumsqr` (`a²+b²`), `difsqr` (`a²−b²`), `sqrsum` (`(a+b)²`), `sqrdif` (`(a−b)²`), `absdif` (`|a−b|`), `thresh` (`a<b?0:a`), `clip2` (clip to ±b), `excess` (`a−clip2`), `round` (to step b), `trunc` (to step b), `fold2` (fold into ±b, reflecting), `wrap2` (wrap into ±b, modulo), `gcd`, `lcm` (both on the integer truncations of `a` and `b`; the result is negative only when both operands are), `hypotapx` (`|a|+|b|−(√2−1)·min(|a|,|b|)` — the cheap approximation, never below the true hypotenuse and up to 15.9 % above it), `idiv` (`⌊a/b⌋`), `roundup` (up to step b), `urshift` (unsigned right shift of the `i32` cast), `amclip` (`a·b` where `b > 0`, else 0), `scaleneg` (`a` where `a ≥ 0`, `a·b` below zero), `firstarg` (`a`; keeps `b` in the graph without letting it reach the output). Comparisons and bitwise ops follow Faust's convention (a boolean is `0.0`/`1.0`; bitwise acts on the `i32` casts, which saturate, and a shift count wraps modulo 32). Where C and scsynth disagree on an operator's meaning, the operator is scsynth's.

Unary operators (`UnaryOpUGen` `op`): `neg`, `abs`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `exp`, `exp10`, `log`, `log10` (of `|x|`), `log2`, `sqrt` (**signed**: `−√(−x)` below zero), `floor`, `ceil`, `rint` (ties to even), `asint`, `asfloat`, `squared` (`x²`), `cubed` (`x³`), `recip` (`1/x`), `frac`, `sign`, `sinh`, `cosh`, `tanh`, `midicps`, `cpsmidi`, `midiratio`, `ratiomidi`, `dbamp`, `ampdb`, `octcps`, `cpsoct`, `distort` (`x/(1+|x|)`), `softclip`, `bitnot` (complement of the `i32` cast), `ramp` (`x` clamped into 0..1), `scurve` (`x²(3−2x)` over 0..1, clamped outside), and the four **windows read at a phase** `x` in 0..1, zero outside it — `rectwindow`, `hanwindow` (`½−½cos 2πx`), `welwindow` (`sin πx`), `triwindow` — which are shapes, not FFT frames (an `FFT` chain's window is its `wintype`).

Every name is **lowercase and joined**, sclang's selector with its camel hump flattened (`hypotApx` → `hypotapx`, `asInteger` → `asint`). Three of them carried an underscore until 0.11 (`as_int`, `as_float`, `hypot_apx`); those spellings are still **accepted** — a def stored before the rename loads — and nothing emits them.

**Random draws (`RandOpUGen`).** A new random value every sample, the distribution carried **by name** in `op` like a range map's: `rand` (`[0, a)`), `rand2` (`[−a, a)`), `linrand`, `bilinrand`, `sum3rand`, `coin` (1 with probability `a`), `rrand` and `exprand` (between `a` and `b`), and the whole-number `irand`, `irand2`, `irrand` and `gauss` (mean `a`, deviation `b`) — the table `clausters_core::rng::Draw` a client draws a value from. Inputs `a[, b]`; `b` is a declared optional tail (0) that only the two-bound draws read. Unlike the other operator kinds it has state: its own generator, seeded off the graph's seed walk like every stochastic UGen, so a render seeded the same replays it.

**Two bounds (`Clip`, `Wrap`, `Fold`).** The general form of `clip2`/`wrap2`/`fold2`, whose range is the symmetric `−b..b`: `{"kind": "Fold", "inputs": [in, lo, hi]}` keeps `in` inside `lo..hi` by holding it at the nearer bound, taking it modulo the range, or reflecting it at the bounds as many times as it takes. Three kinds rather than one generic because that is how sclang names them; the arithmetic is one core function (`clausters_core::builtins::apply_bound`) that a client clips, wraps or folds a value with too. Both bounds are required inputs and may be signals.

**Range maps (`RangeMapUGen`).** The same argument one arity out: rather than a kind per map, one generic UGen carries the **map by name** in its `op` field, over the inputs `in, in_lo, in_hi, out_lo, out_hi[, curve]` — so `{"kind": "RangeMapUGen", "op": "linexp", "inputs": [lfo, -1, 1, 200, 8000]}` sweeps a frequency the way an LFO sweeps. The maps are `linlin`, `linexp`, `explin`, `expexp`, `lincurve` and `curvelin`, the family in `clausters_core::warp` — **the same function a client computes a value with**, so a signal mapped in a def and a fader position mapped in the script land in the same place. `curve` is a declared optional tail (0, no bend) and is read only by the bent pair; the four that ignore it take whatever arrives. A static **`clip`** field says what an out-of-range input is trimmed to before it is mapped — `"minmax"` (the default, both ends), `"min"`, `"max"` or `"none"` (extrapolate) — exactly as on the value side. Every bound is an ordinary input, so a range may itself be a signal; the UGen then prepares the map per sample instead of once per block, which is what modulating a range costs.

sclang's bipolar `range`/`exprange` are **not** here, though the value functions have them: they read the input's own polarity (a UGen's `signalRange`), which this graph does not track, so a def that means "−1..1 onto lo..hi" writes that input range out and says what it means.

The tables are open — a new operator is one more `clausters_core::builtins` entry, no renumbering. The Python client maps its operators and math methods (`%`, `min`, `>`, `.midicps()`, `.distort()`, …) to these names automatically, and exposes `madd`/`sum3`/`sum4` for the fused kinds.

**Calculation rates (`ir`/`kr`/`ar`/`dr`).** Every ugen output has a rate, chosen with the optional `"rate"` field or defaulted per kind. `ar` (audio rate) is one value per sample — the normal signal wire, and the default. `kr` (control rate) is one value per block, recomputed each block; downstream it reads as a constant over the block. `ir` (initial rate) is computed **once, when the synth starts**, then held for the node's life — a fixed `SampleRate`, a `Rand` seed, a `BufFrames.ir` snapshot; its inputs must themselves be constant/`ir` (nothing that varies can be frozen). `dr` (demand rate) is pulled, not run — see the demand note. Coercion is one-way: a slower rate feeds a faster input for free (a constant broadcast over a block), but an `ar` signal cannot feed an `ir` input, and a `dr` output may only feed something that *pulls* it — a demand driver, or another demand ugen nesting it. Audio-rate controls are not a thing here — feed an audio signal through `In`/an input bus (as noted under `/node_mapAudio`).

A slower consumer **samples** a faster input, it does not summarize it: a `kr` ugen reading an `ar` wire takes the block's first sample and ignores the other 63. That is invisible for a smooth signal and destructive for a trigger train, so a counter and its clock belong at the same rate. Otherwise, choosing `kr` changes a ugen's **cost**, not its meaning: a time still counts in seconds and a frequency in hertz, because each ugen runs at its own sample rate (one `kr` sample lasts a whole block). `Impulse.kr(10)` and `Impulse.ar(10)` both fire ten times a second; `LPF.kr(sig, 300)` cuts off at 300 Hz. What you give up is resolution — a `kr` ugen cannot represent anything above half the control rate (375 Hz at the default 48 kHz / 64), which is why oscillators and filters in the audible band belong at `ar` and envelopes, sweeps and modulation are comfortable at `kr`. The one exception is `SampleRate`, which reports the engine's rate at any rate, because that is a fact about the hardware rather than about the ugen.

**Demand-rate streams (the `D*` family).** A demand ugen produces values only when *pulled*: it has no samples, only a next value, and between two pulls it does nothing at all. It never runs in the normal per-block order. What turns a stream into signal is a **driver**. `Demand` is told when to pull — one value per rising edge of its `trig`, held until the next, a rising `reset` restarting the stream. `Duty` and `TDuty` carry their own clock instead: every `dur` seconds they pull one `level`, so a stream of durations against a stream of values *is* a sequencer, the two free to be different lengths. `Duty` holds each level until the next is due; `TDuty` emits it on that one sample and is silent in between — a trigger train whose amplitudes are the levels. Where a driver's own inputs are ordinary signals (`trig`, `reset`, `done_action`) they are read at the sample the pull happens on, so a modulated duration lands where it was modulated.

The sources are the rest of the table above: the ramps (`Dseries`, `Dgeom`), the stochastic ones (`Dwhite`/`Diwhite` drawing independently, `Dbrown`/`Dibrown` walking), the list sources (`Dseq`, `Dser`, `Drand`, `Dwrand`, `Dxrand`, `Dshuf`), and the one-offs (`Dstutter`/`Ddup`, `Dswitch1`, `Dswitch`, `Dconst`, `Dreset`, `Dpoll`, `Dbufrd`, `Dbufwr`). Every one of them takes **`repeats`** first, and **`repeats ≤ 0` is the endless stream** — sclang writes `inf` there, which a def cannot carry (the wire rejects a non-finite constant, and JSON has no spelling for one), so the count of none is the endless one. For `Dseq` and `Dshuf` the count is of **passes over the list**; for `Dser`, `Drand`, `Dwrand` and `Dxrand` it is of **items**, since a random pick has no pass to complete and `Dser` is the in-order list that does count items.

**Streams nest, and that is the point of the family.** Any input of a source may itself be a `dr` wire — `Dseq(1, Dwhite(2, 0, 1), 100)` yields two random numbers and then 100 — which is what makes a sequence of *phrases* expressible rather than a sequence of numbers. A list source **drains** a nested slot (reading it until it answers end-of-stream) before moving on, and **restarts** the slot it comes round to again, so `Dseq(2, Dseries(3, 0, 1))` gives `0 1 2 0 1 2`. Whether a reset travels inward is per kind, not a blanket rule: the list sources restart the slot they move to, `Dstutter` and `Dswitch1` restart their inputs outright, and the scalar sources propagate nothing, since they re-read their bounds on every pull anyway. Nesting is capped at **16 levels** — a pull descends one stack frame per level on the audio thread, so a def past the limit is refused at compile time rather than guarded at run time.

End-of-stream reaches the drivers differently. `Demand` holds the last value it got (and outputs 0 before the first trigger); `Duty`/`TDuty` stop pulling and fire their `done_action` (the U4 set: `Done`, `FreeSelf`, …), which is how a finite sequence frees its own node.

**Side-effect UGens (`SendTrig`/`SendReply`/`Poll`).** Some UGens exist for a **side effect** — an OSC reply or a console post — not for audio on a bus, and a def may contain *only* these with **no `Out` at all** (the server requires at least one UGen, never an `Out`). All three fire on a **trigger**: a signal crossing from `≤ 0` up to `> 0`. `SendTrig` replies `/node_trigger nodeID id value`; `SendReply` replies at a custom address (its static `label` field, default `/reply`) carrying `nodeID reply_id value…` for an arbitrary value list; `Poll` posts `label: value` to the server console and, with a non-negative `trig_id`, also emits a `/node_trigger`. The replies reach every client registered with `/server_notify` (`SendReply`'s custom address is delivered verbatim). A `Poll`'s line is logged at `info`, so the server prints it with `-v`; an offline render posts it the same way. `Dpoll` posts the same line for each item of a demand stream, and `CheckBadValues` posts what kind of number it found, at `warn`.

**Faults (`/node_fault`).** A UGen that cannot honour what it was given exactly — a `Conv` kernel it cannot use whole, a combiner whose two chains' windows differ — plays **silence rather than a wrong result**, and says so once: the live server logs a warning and sends **`/node_fault <int32 nodeID> <string kind> <int32 code> <string sentence>`** to every `/server_notify` client, where `kind` is the UGen's (`Conv`, `PV_Add`, …), `code` its fault (the convolution codes are `1` partition size, `2` too long, `3` a raw kernel longer than its `framesize`; a combiner's `1` is windows that differ) and `sentence` the explanation the server logged. Both clients warn it (Python on the `clausters.server` logger, the web client on the console). An **offline render fails** with the sentence instead, since a file of silence and no error is a wrong result nobody is told about. Mechanically the reply leaves the audio thread through a lock-free FIFO — the same discipline as the `/node_start`/`/node_end` node events — so triggering never allocates or blocks; a burst of triggers beyond the per-block buffer is dropped (best-effort, like the node events). The `id`/`reply_id`/`trig_id` and `value`s can be any signal (sampled at the trigger). The Python client builds them with `send_trig`/`send_reply`/`poll` and passes them as `SynthDef` roots.

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
rate. The window size is a **static** field (`fft_size`, a power of two from 64
to 32768, scsynth's range) given **only on the `FFT`** — the compiler propagates
it, the frame (`winsize`), the hop and the window type to the rest of the
chain, so `PV_*`/`IFFT` need none of them. A `winsize` below `fft_size` windows
that many samples and pads the transform with zeros, a finer grid of bins over
the same stretch of time; the `IFFT` keeps the frame's own samples of the
inverse. The resynthesis is exact only at the analysis hop with the analysis
window, so a `PV_*`/`IFFT` that repeats a `hop` or `wintype` other than its
chain's fails the def. An unsupported size, or a `PV_*`/`IFFT` whose first
input is not a spectral chain, fails the def with `/fail`.

A **two-chain combiner** (`PV_Add`/`PV_Mul`/`PV_Min`/`PV_Max`/`PV_MagMul`/
`PV_CopyPhase`) takes **two** chain inputs — both must be spectral chains, of
the **same `fft_size`, hop and window**, and **distinct** (the same chain on
both sides fails the def). The combined frame is resynthesized as chain A's, so
a chain B analysed at another hop or with another window would come back at the
wrong level. The result lands in **chain A** (input 0): the combiner's output wire
carries chain A onward, and whatever `PV_*`/`IFFT` follows reads the combined
frame. It acts on the blocks where chain A has a fresh frame, reading chain B's
latest frame. The operator is a property of the *name*; all six are one
server-side implementation.

**Copies, written frames and readers.** A chain wire feeding two `PV_*`
branches is one frame both change in turn, as in scsynth; `PV_Copy` makes the
second branch a chain of its own, of its source's shape (and combinable with
it). `FFTTrigger` opens a chain no signal is analysed into: `PackFFT` or a
`PV_Kernel` program writes its bins, and since its frames carry no analysis
window the `IFFT` divides their overlap by the synthesis window alone.
`Unpack1FFT` reads a bin out as a control-rate value; its output is a value, so
the compiler refuses it (and an `IFFT`'s) where a chain is expected.

**Analysis.** The measures of a frame, the onset detectors and `BeatTrack`
are chain sinks: they read each fresh frame, leave it as it was, and hold
their value between frames, so one chain feeds any number of them beside its
`IFFT`. scsynth carries their tables for one transform at 44.1 or 48 kHz;
here each table is the formula that generated it, built for the chain's
transform and the engine's rate, so a measure follows the chain (and is
scsynth's at its own). The trackers step their search once a control block,
on the slice that starts the block, so a timed event that cuts a block moves
no step; a UGen with several outputs is a row an output (`chan`).

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
`2 + P·fftSize` frames). The whole response is prepared or none of it: a
target smaller than that, an empty source, or a response over 256 partitions
(use a larger `fftSize`) fails the command with `/fail`. So does an impulse
response at another sample rate than the server's: a partition is a span of
samples, and the server converts no rates, so resample the IR first. The audio thread only ever multiplies against the
ready spectra, and its per-block cost is **flat**: the partition products are
spread across the hop's blocks, so a long reverb tail does not spike the hop
block. Intrinsic latency is one partition (`fftSize/2` samples), reported by
the node, consumed by general server delay compensation in automatic bus paths.
Moving the `kernel` input to a *different* prepared buffer crossfades over one
partition (scsynth's `Convolution2L` behavior); regenerating the same buffer
index switches hard — use a fresh buffer when the transition matters. An
unprepared or missing kernel plays silence; the input history keeps running,
so a valid kernel resumes cleanly. A kernel the `Conv` cannot use **whole** —
prepared at another `fftSize`, or with more partitions than its `partitions`
holds — also plays silence rather than a response cut short, and says so once:
the live server logs a warning naming the buffer and both numbers and sends it
to its clients as a `/node_fault` (see the side-effect note), and an
offline render fails with it.

Analysis and resynthesis use the same window (Hann by default), and the
overlap-add is **window-normalized** (divided by the steady-state window-overlap
denominator, COLA), so a plain `FFT`→`IFFT` reconstructs the signal at unity
gain, delayed by exactly one frame. A frame is taken at exactly every hop of
the synth's own input, whatever slices a timed event cuts the block into, so
any hop in range reconstructs exactly, not only whole blocks. The first frame
ends one hop in and holds the silence before the synth started, so the first
samples come back whole too. scsynth plays a frame one block sooner, in the
block it closes, and numbers its windows `0` sine and `1` Hann where `wintype`
here is `0` Hann and `1` sine (the core's `Window`, which the GUI and the
clients share).

The window is settable live with `/node_ugenCmd <nodeID> <ugenIndex> window
<wintype>` (`-1` rectangular, `0` Hann, `1` sine, `2` Welch, `3` Hamming, `4`
Blackman) addressed to the chain's **`FFT`** — the first consumer of the typed
per-UGen command surface. The window is the chain's: the `IFFT` follows it frame
by frame (a `window` command sent to the `IFFT` does nothing), and the change
takes effect at a frame, at most once per window length — a second one within a
window waits. The samples covered by frames on both sides of the change are
normalized by the windows those frames actually used, so a round trip stays
exact through it. A **combiner**'s two chains must change together (both
`FFT`s in one bundle): while their windows differ it writes silence into chain A
and reports the mismatch once, as a `Conv` does a kernel it cannot use.

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

**Streaming disk I/O (`DiskIn`/`DiskOut`).** These stream to/from disk in real time, so arbitrarily long files never touch the buffer pool (`PlayBuf`/`BufRd` load the whole file first). Each is **self-contained**: one background I/O thread plus a lock-free ring shared with the audio thread, opened at `/synth_new` and closed when the synth is freed. The audio thread only pushes/pops the ring — never blocks. A ring underrun (disk too slow) plays silence; a `DiskOut` overrun drops samples; both are rare with the ~1 s ring. They carry extra **static fields** in the UGen spec (alongside `kind`/`inputs`): `path` (required), `loop` (`DiskIn`, default false; a loop goes back to the file's top), `start` (`DiskIn`, the file frame the stream begins at, default 0 — reached by decoding up to it on the reader's side, so it is exact in every format and the audio thread's first frame is that one), and `format` (`DiskOut`, `int16`|`int24`|`float`, default `int16`). Both are **mono per UGen** like the buffer readers: `DiskIn` extracts one channel (`chan`) — a stereo file is two `DiskIn`s; `DiskOut` writes a mono WAV — record stereo with two `DiskOut`s. `DiskIn` streams one file frame per server sample with **no resampling** (pitch follows the sample-rate ratio, as in scsynth's `DiskIn`); these UGens spawn a thread each, so they are for a handful of streams, not per-voice. Example DiskOut UGen: `{"kind": "DiskOut", "inputs": [{"ugen": 0}], "path": "/tmp/rec.wav", "format": "float"}`. **In an offline render the streams wait for their threads**, as scsynth's NRT reads a file: nothing there plays against a clock and a render runs as fast as it can, so a `DiskIn` (and a `TransportDiskIn`) waits for each frame and a `DiskOut` for room, rather than playing silence or dropping samples. Two renders at once each wait on their own threads, and write what each writes alone.

Output happens exclusively through `Out`/`ReplaceOut`; a def without them is silent. Several synths with `Out` on the same bus mix. Bus-index inputs are ordinary signals, sampled at the first frame of each block and clamped to the valid range.

**Band limiting in `Saw`/`Pulse` (deviation from scsynth).** scsynth builds these from a discrete-summation impulse train — a sine table divided by a cosecant table — smoothed by a leaky integrator, over a 32-bit fixed-point phase. Ours accumulate phase in `f64` and correct each discontinuity with a fourth-order PolyBLEP, which costs no division, no table and no integrator: there is no settling transient, no residual DC droop, and no fixed-point tuning error. The trade is that PolyBLEP stays *quasi*-band-limited — a residual remains and grows with the fundamental. Measured alias SNR at 48 kHz, against the same waveform generated naively:

| fundamental | `Saw` | naive ramp | `Pulse` | naive square |
|---|---|---|---|---|
| 105 Hz | 96.7 dB | 30.9 dB | 98.4 dB | 32.7 dB |
| 996 Hz | 42.6 dB | 16.0 dB | 43.5 dB | 17.7 dB |
| 3996 Hz | 39.2 dB | 9.9 dB | 38.9 dB | 11.4 dB |

The 105 Hz figure is within about 2.5 dB of what the measurement itself can resolve, so the low end is effectively transparent. The `LF*` shapes are deliberately not band-limited at all, as in scsynth — they are modulation sources and their corners should be exact.

**The two-pole rows are scsynth's sections, realized as one filter.** scsynth implements `LPF`, `HPF`, `BPF`, `BRF`, `RLPF`, `RHPF` and `Resonz` as separate direct-form sections, each with its own coefficient formula, and those formulas define the rows: `RLPF` and `RHPF` are not the textbook bilinear resonant pair, and `Resonz` is a different resonator from `BPF`. Each row here computes scsynth's coefficients and realizes that transfer function through a *topology-preserving* (trapezoidal-integrator) state-variable filter. Every digital two-pole is the bilinear transform of exactly one analog two-pole, which a state-variable filter realizes exactly, so the response is scsynth's, not an approximation of it — `tests/offline/sc_reference.rs` holds each row against scsynth's own code sample by sample. What changes is behaviour, not response: it does not leave its stable region under audio-rate cutoff modulation, it stays well conditioned at low cutoff, and every row is a mix of the same three taps. That last point is why `Svf` exists at all: exposing the tap mix as inputs costs the mix and nothing else, so a filter whose response is itself a signal is free here and would mean recomputing coefficients in a direct-form section.

State and coefficients are `f64` throughout, matching scsynth's own choice for the same reason: near DC the poles crowd the unit circle and `f32` truncation dominates the output. The coefficients are worked out once per block, from the parameters at its last sample, and the integrator gains and the tap mix move in a straight line across the block from where the last one left them — scsynth's coefficient slope, so a control-rate sweep does not step — which keeps a modulated filter at three multiply-adds and the mix per sample rather than a transcendental.

Resonance travels as **`rq`**, the reciprocal of Q, as in scsynth. That is not a performance choice (one division per block, next to a `tan` that costs several times more) but a domain one: `rq = 0` is infinite Q and is exactly representable, where `Q = 0` would divide by zero and `Q → ∞` is not a number. Clients are free to offer `q` and convert.

**The delay family is one line (deviation from scsynth).** `DelayN/L/C`, `CombN/L/C` and `AllpassN/L/C` are the same circular buffer with two independent parameters: how a fractional tap is interpolated, and what is fed back. Measured through a half-sample delay at 9 kHz, `L` loses about 1.6 dB and `C` about 0.36 dB — four-point interpolation is not a brick wall, but that is the gap that justifies paying for `C` on a modulated delay.

The line is **synth-private memory**, allocated when the synth is built and sized from the static `max_delay` field (in seconds) and the server's sample rate — memory no other node can reach, which is what a per-voice delay wants. Unlike scsynth, `max_delay` is **static configuration and not an input**: it sizes an allocation, so it belongs with `fft_size` and `partitions` rather than among the signals, and it defaults to 0.2 s if the def omits it. A `delaytime` past it is clamped, never wrapped.

**The `BufDelay*` family is the same nine over a pool buffer** (the rows above), for the case the private line cannot serve: a line whose contents are addressable — recorded into, read by another node, resampled, saved. One implementation serves both, parameterised by where the samples live, so the two families cannot drift apart in their arithmetic. Which to reach for is a question about the *line*, not about the sound: private if nothing else needs to see it, a buffer if something does.

These UGens do **not** report an intrinsic latency, on purpose. Their delay is what the user asked for, not an artifact of processing; the latency hook exists for something like the partitioned convolver, and compensating a musical delay would silently undo it.

Buffer readers **and writers** are **mono** (one output per UGen, unlike scsynth's multi-output PlayBuf): the `chan` input picks the channel, and two of them with the same inputs stay sample-locked, so a stereo file is two UGens.

**A cue point belongs to whoever carries a position.** `PlayBuf` and `RecordBuf` advance themselves, so each takes a `trigger`, a frame to cue to (`startPos`/`offset`) and a `doneAction`. `BufRd` and `BufWr` are driven by a phase signal and have no position of their own: re-cueing one means changing the signal that drives it, and a done action would have nothing to be done with.

**A buffer's contents are mutable, and only its shape is fixed.** Frames, channels and sample rate are settled at allocation and never change; every sample can be written at any time, by a UGen or by a `/buffer_*` command, while anything else is reading it. That is scsynth's model, and it is why `RecordBuf`, `BufWr` and the `BufDelay*` family exist at all: recording into a buffer another node is playing is the ordinary case. What a reader crossing a writer sees is some old samples and some new, never half of one — per-sample atomicity, no ordering between samples, which is what a looper crossing its own write head has always sounded like. Nothing has to be declared: there is no writable *kind* of buffer, because they all are.

**A write costs the samples it writes, and it lands where the reader is looking.** `/buffer_set`, `/buffer_setRange`, their `*Channel` forms and `/buffer_fill` write **in place**: the samples go into the cells the engine is already reading, so the cost is the span's rather than the buffer's (a 1 ms run costs the same on a ten-minute take as on a ten-second one) and nothing a recorder wrote meanwhile is lost. The same rule as above is what a client sees: a reader crossing the write sees some old samples and some new, never half of one, and the write becomes audible on the next block with no further command. The commands that *replace* a buffer rather than write into it — `/buffer_alloc`, `/buffer_allocRead`, `/buffer_read`, `/buffer_gen` — are the ones that change what a buffer *is* (its shape, its sample rate, its whole contents), and those still install a new buffer whole; a recording crossing one of those is replaced along with everything else, which is what replacing the samples means.

## Buffers (`/buffer_*`)

```text
/buffer_alloc     bufnum frames [channels=1] [sampleRate=0]   # zeroed buffer; 0 = the server's rate
/buffer_allocRead bufnum path [fileStart=0] [numFrames=0=all] # shape from the file
/buffer_read      bufnum path [fileStart=0] [numFrames=-1=all] [bufStart=0]
/buffer_allocReadChannel bufnum path fileStart numFrames channel...  # shape from the file, selected channels
/buffer_readChannel      bufnum path fileStart numFrames bufStart channel...
/buffer_write     bufnum path [header="wav"] [format="int16"|"int24"|"float"] [numFrames=-1] [startFrame=0]
/buffer_zero      bufnum
/buffer_gen       bufnum cmd flags args...  # fill/generate — see the wavetable section
/buffer_fill      bufnum [start count value]...  # runs of one value (flat, like /buffer_set)
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
/buffer_stream    periodMs bucket bufnum...  →  /buffer_stream.reply  bufnum startFrame bucket blob   # the overview as it is written
```

`/buffer_alloc`, `/buffer_allocRead`, `/buffer_read`, `/buffer_write`, `/buffer_zero`, `/buffer_gen`, `/buffer_set`, `/buffer_setRange`, `/buffer_setChannel`, `/buffer_setRangeChannel`, `/buffer_fill`, `/buffer_readChannel`, `/buffer_allocReadChannel` and `/buffer_free` are **asynchronous**: the work happens on a dedicated NRT thread (one queue, so commands on the same buffer complete in submission order) and the reply is `/done <cmd> bufnum` or `/fail <cmd> reason`. Buffers keep the file's sample rate (the server never resamples — see `PlayBuf`'s rate above); integer WAVs are scaled to ±1. `/buffer_read` requires an allocated buffer and keeps its shape; channel-count mismatches fail. Reading decodes by **content**, not extension: WAV goes through hound (exact, int24-aware), **W64 and RF64** — the two containers past WAV's 4 GB, which keep a WAV's samples and change only the header (Sony Wave64's GUID chunks with 64-bit sizes; EBU RF64's `ds64` chunk) — are read by the server's own PCM reader, which a WAV that hound refuses falls back to (an RF64 is often named `.wav`), and FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and CAF decode through [symphonia](https://github.com/pdeljanov/Symphonia) (whole-file decode, then slice — compressed formats have no cheap exact frame seek). `/buffer_write` writes a WAV, a W64 or an RF64: the `header` argument names `w64` or `rf64`, and the default `wav` leaves it to the path, so a path ending in `.w64` or `.rf64` is written in that container. Every WAV written keeps an RF64's room (a `JUNK` chunk the size of a `ds64`, as EBU Tech 3306 says to write one), so a WAV whose body outgrew 4 GB is closed as an RF64 under its own name rather than refused. `leaveOpen` (streaming) is not supported. `/buffer_close bufnum` closes the soundfile a streaming buffer left open — scsynth pairs it with `DiskIn`/`DiskOut`; since Clausters has no streaming buffers yet (every `/buffer_read`/`/buffer_write` reads or writes the whole file and closes it), it validates the buffer is live and replies `/done /buffer_close bufnum`, forward-compatible with the streaming UGens.

`/buffer_attach bufnum` is the one command that exists only for a server sharing a segment with another one. A server that **attached** to somebody else's segment (see [`ipc.md`](ipc.md)) maps every buffer the owner had published when it started; this points it at one published since. It is synchronous, replies `/done /buffer_attach bufnum`, and fails when this server owns the samples, when it has no shared segment, or when the directory has no live buffer under that number. Nothing about the samples travels: the reply means this server's engine now plays the very cells the owner writes.

`/buffer_touch bufnum channel start frames` is the announcement a **local peer** makes after editing a shared buffer in place. A peer that maps the samples writes into the very cells the engine reads, and nothing about that reaches the wire — which is the point, and also why a second client holding a picture of the same take would never find out. This carries the span and not the samples: the server broadcasts `/buffer_touched bufnum channel start frames` to every client registered with `/server_notify` **except the one that wrote**, and what a receiver does with it depends on how it holds the samples: a peer that **maps** it re-summarizes that span (its samples are already the new ones — this is what the GUI host does), and one holding its own copy re-reads the span with `/buffer_getRange`. It is a notification rather than a command, so nothing is answered to the sender. A page is exactly who needs it: a browser cannot map a file, so a message is the only way it can hear about an edit at all.

**`/buffer_stream periodMs bucket bufnum...`** subscribes the sending client to the **overview of samples as it is written** — a recording filling a picture, for a client that cannot map the region and watch it fill. The server acks `/done /buffer_stream`, then sends every `periodMs` (10 ms floor) one **`/buffer_stream.reply bufnum startFrame bucket blob`** per watched buffer whose write frontier has moved by at least one whole bucket since that client's last report — and nothing at all for one that has not moved, so a still buffer costs no traffic. The blob is **bucket-major, channel-minor**: for each bucket in order, for each channel, `min`, `max` and **mean square** as raw little-endian `f32` — the peak pyramid's own three statistics in its own energy form, so a client folds them into its summary without converting anything. `startFrame` is where the report begins, on the buffer's own sample axis. **The receiving half is in the shared core**, so no client folds this by hand: `peaks::MultiPyramid::write_buckets` takes the blob as it arrives — `peaks_cache_stream_file` in the Python client, `Peaks.writeBuckets` in the web one — and puts the buckets into the pyramid the picture already holds, rebuilding the levels above them. Subscribe with the bucket that pyramid was built at (256 unless it says otherwise) and the two grids agree by construction.

What it is for is the round trip it replaces. A view of a server buffer that cannot map it had two ways to get a picture: download every sample (230 MB for a ten-minute stereo take), or have nothing until something records into it. This is the third — about a hundredth of the bandwidth, enough to draw the whole take at once, with the spans under a zoom read back by `/buffer_getRange` as they are needed.

The unit is the **summary and not the samples**, which is the whole point of the command: one channel at 48 kHz is 187 buckets a second at the default 256, about 2.2 kB/s, against 192 kB/s for the audio it describes. At most 32 buffers per subscription and 8 KiB of statistics per report (the rest arrives in the next one). Same posture as `/bus_stream`: one subscription per client, **replaced** by each new call, `periodMs <= 0` (or an empty buffer list) cancels, it dies with a TCP/WebSocket connection, and it is not schedulable in a timed bundle. A subscription is a watch on what happens **next** — it reports from the frontier as it stands when you subscribe, not the overview of what is already there, which is a fetch and is spelled `/buffer_getRange`.

What moves the frontier is a **writing UGen** (`RecordBuf`, `BufWr`): each publishes, once per block, the highest frame it has filled. The frontier is the **buffer's own**, so this command works on any server — an engine inside a page, a `clausters` booted without `--shm` — and when there *is* a segment the same number is mirrored into the buffer directory's row (see [`ipc.md`](ipc.md)), which is where a peer that maps the region reads it. Such a peer needs no subscription at all: it re-summarizes the new span out of the memory it already has. This command is that reading, for whoever cannot map.

### Filling and channel-selective reads (`/buffer_fill`, `/buffer_readChannel`, `/buffer_allocReadChannel`)

`/buffer_fill bufnum [start count value]...` writes runs of one repeated value — scsynth's `/b_fill`, and the sibling of the `/bus_fill` beside it. Its indices are **flat and interleaved**, like `/buffer_set`'s. Several runs ride in one message, and a run past the end **fails** rather than clamping, like every other write here.

`/buffer_readChannel` and `/buffer_allocReadChannel` are `/buffer_read` and `/buffer_allocRead` with a **channel selection**: the trailing integers name which of the file's channels to keep, and the positions before them are required rather than optional, since with a variadic tail there is no telling a `fileStart` from a channel index. An empty selection is every channel, which is exactly what the two commands without `Channel` in their name are. The order is honoured and repeats are allowed — `1 0` swaps a stereo pair, `0 0` makes a mono file two-channel — and a channel the file does not have **fails** rather than reading as silence, since asking for the right channel of a mono file is a mistake worth hearing about. `/buffer_readChannel` keeps the target's shape like `/buffer_read`, so the selection must match its channel count; `/buffer_allocReadChannel` takes its shape from the file *and the selection*, which is how one channel of a stereo file lands in a mono buffer — the thing that could not be done at all before.

### Table generation and the wavetable format (`/buffer_gen`)

`/buffer_gen bufnum cmd flags args…` fills an **already-allocated** buffer with a computed signal — additive spectra, a waveshaping curve, a copy from another buffer, or a prepared convolution kernel. Like `/buffer_read` it reads the target's shape from the current contents, so a `/buffer_gen` right after a `/buffer_alloc` must be separated by a `/server_sync` (the alloc has to complete first). It runs on the same NRT queue and replies `/done /buffer_gen bufnum`.

The `flags` int packs three bits, `normalize`(1) + `wavetable`(2) + `clear`(4) — the usual value is `7` (all three). `normalize` scales the result to a peak magnitude of 1; `clear` starts from silence (without it the new signal is **added** on top of the buffer's current contents); `wavetable` stores the result in the interleaved wavetable format below (an `N`-sample buffer then holds `N/2` period points). `copy` takes no flags.

| cmd | args | fills with |
|---|---|---|
| `sine1` | flags, amp… | additive sine partials; `amp[k]` is the amplitude of harmonic `k+1` |
| `sine2` | flags, (freq amp)… | partials at arbitrary (possibly fractional) harmonic numbers |
| `sine3` | flags, (freq amp phase)… | as `sine2` with a per-partial phase in radians |
| `cheby` | flags, amp… | a waveshaping transfer function `Σ amp[k]·(T_{k+1}(x) − T_{k+1}(0))` of Chebyshev polynomials over `x∈[−1,1]`, each shifted to pass through 0 at `x = 0` so a silent input stays silent (`amp[0]` weights `T₁`, the linear/passthrough term), sampled as scsynth's: in wavetable form its pairs span `[−1, 1]` inclusive; read by `Shaper` |
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

Three consumers build this format today: the Python client's `clausters.defs.boxes` module (the box algebra as composable Python values; see the [client book's defs chapter](https://clausters-python.readthedocs.io/en/latest/defs.html)), the web client's `defs/boxes.ts` (the same algebra, the same emitted tree) and machine-generated graphs (the GUI host). `faust` fragments are **memoized by `src` within one compilation**: the same source text yields the same box, so a client that reuses one fragment value many times (duplicating the subtree in the JSON) gets one computation and one compile — every `CDSPToBoxes` evaluation would otherwise mint fresh recursion symbols and defeat the sharing (the CSE suite in `crates/clausters-server/tests/faust_offline/faust_box.rs` pins this).

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

The **sample rate** enters the graph through `fconst`, not as a baked number: `{"op": "fconst", "ctype": "int", "name": "fSamplingFreq", "file": "<math.h>"}` is the runtime constant behind Faust's `ma.SR`, resolved when the def is instantiated, so a def stays in tune at whatever rate the engine (or NRT renderer) runs. `ma.SR` itself is that value clamped to `[1, 192000]`; both clients wrap the whole thing as `signals.sr()` (and `boxes.sr()` in the box algebra). (`ma.PI`, by contrast, is a plain numeric literal — no `fconst` needed.)

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
- `buses` — internal buses, **private to each instance** (`rate` `"audio"` or `"control"`, `channels` default 1). They are allocated per instantiation from a reserved range at the **top half of the bus space** (so `512..1024` and `8192..16384` at the default counts, shifting with `--audio-buses`/`--control-buses`) — so they never collide with client-allocated buses (the same idea as the reserved MIDI/auto node-id ranges). The reservation is a **share** and not a fixed count (`clausters_core::registry::graph_audio_reserved`/`graph_control_reserved`, exposed over the core FFI), because it partitions a configured resource: a server booted with more buses has to hand the extra ones to both sides, or raising the count would not buy a multitrack one extra track. A client asks with the counts that server reported over `/server_query` rather than assuming the defaults. Two instances of one GraphDef get disjoint buses, and a freed instance's buses return to the pool.
- `maps` (per member, optional) — binds a member control to an internal **control** bus via `/node_map`. A control the member's def does not have is skipped.
- `ends` (per member, optional) — **when this member ends, the graph it is in ends with it**, and the slot around that graph when the graph is all that fills one. A voice that frees itself when its envelope closes takes the rest of its graph along — the readers of the curves that shaped it — rather than leaving them running beside nothing. The graph is freed when the server hears of the end, a few milliseconds after it.
- `surface` — the named ports. Each maps to a list of `{member, control}` targets, with optional `mul`/`add` linear scaling of the incoming value. One port may drive several inner controls, each scaled differently (e.g. a `freq` port playing a detuned pair). This is the difference from a bare group `/node_set`, which can only broadcast one value to controls that happen to share a name.
- `defaults` — surface-port values applied at instantiation, overridable per instance.

`/graph_new name id addAction targetID [port value]...` instantiates a GraphDef: it creates an **auto-sorted group** (the member execution order follows the bus connections, audio and control) at `id` (or `-1` for a server-assigned id), holding the **shared** members and any graphs nested among them, then applies the shared `defaults` and the given `port value` overrides. `/node_set id port value...` on that group id resolves the port names against the surface (never the member ids); a port absent from the surface is ignored. `/node_free id` (or `/group_deepFree`) tears the instance down (with all its voices) and reclaims its private buses. Instantiation is atomic **over the whole nested tree**: everything fallible — resolving every def, building every synth, taking every bus and node id, at every level — happens before a single command or mirror change, so a missing member def or a bus shortfall anywhere fails with no partial instance and hands back everything it took. GraphDefs work in NRT scores too (scored like any def at time 0).

### Shared members and slots (`/graph_addSlot`, `/graph_newVoice`)

A GraphDef splits into a **shared** part (members with no slot) and its **slots** (members with `"slot": "<name>"`). A shared member is instantiated once at `/graph_new` — the always-on part: the private buses, a mixer, effects. A slot member is instantiated on demand, once per thing there is one of: a voice of a polyphonic instrument, a clip on a track, an effect in a chain. They are one mechanism because they are one question — how many of these are there right now — and the answer changes while the graph is sounding, which is why it cannot be a member list. A surface port drives either the shared members or the members of **one** slot, never a mix (a `/fail` at `/def_send graph` otherwise).

`/graph_addSlot instanceID slot id [port value]...` builds one more of a slot as a **sub-group at the head of the instance** (the auto-sort orders it before the shared mixer that reads its bus), wired to the instance's own private buses, applying that slot's `defaults` and the given overrides. `/node_set slotID port value...` resolves against the sub-graph's surface; `/node_free slotID` frees just that one. Adding a slot the def does not have (or naming an unknown instance) `/fail`s.

`"voice": true` is the slot named `voice`, spelled the way it was before slots had names, and `/graph_newVoice instanceID id [port value]...` is `/graph_addSlot` on it. Both keep working unchanged.

### Moving a slot to another instance: `/graph_moveSlot`

`/graph_moveSlot slotID instanceID` moves a slot built by `/graph_addSlot` into another running instance — a clip dragged to another track — **without building it again**. A slot's wiring is baked into its members' bus controls when it is built, so moving the group alone (`/group_head`) would leave it writing the instance it came from; this moves the sub-group to the head of the new instance and resolves every bus reference in it again against that instance's private buses, including the buses handed to the graphs nested in it and to their own slots. Nothing is freed, so what hangs off the nodes stays: the slot's id, the values set on its ports, a `/graph_map` onto one of them, the slots inside it.

`instanceID` may be an instance or a slot group that is one nested graph, as for `/graph_addSlot`. The new instance must declare the same slot with the same members — two instances of one GraphDef always do — and a slot cannot move into anything nested inside itself; either is a `/fail`, and so is a `slotID` that is not a slot. Moving a slot to the instance it is already in does nothing.

### Driving a port from a bus: `/graph_map`

`/graph_map instanceID port bus [audio=0]` is the other half of `/node_set` against a surface: instead of writing a value it **maps** every control the port resolves to onto `bus`, so whatever writes that bus drives the port. A negative `bus` unmaps, which is how a port goes back to whoever sets it by hand.

It exists because an automation needs it. A curve is a node writing a control bus, and what it drives is a port — of a track, of a clip, of an effect three levels down — whose member ids are private and are meant to stay that way; without this a client would have to be told the node behind a port, which is the encapsulation the surface exists to keep. The port's own scaling (`mul`/`add`) is **not** applied: a bus carries a signal and scaling one would need a node, so the bus is mapped straight onto the control and the scaling belongs to whatever wrote it. A port the instance does not have is a `/fail`.

### A member can be another GraphDef

`{"def": "<graphname>", "kind": "graph"}` makes a member a whole nested graph rather than one node: it is instantiated as a **subgroup** with private buses of its own, and it is freed with its parent. That is what lets a track hold clips and a clip hold an effect chain without either being a second mechanism — and it means an effect can itself be a GraphDef, so a compound effect needs nothing new.

Two things make a nested graph reusable rather than wired in place:

- **It does not decide where it goes.** A bus marked `"external": true` is one the parent provides: the member's `controls` name which of the *parent's* buses each of the child's buses is (`{"def": "sub", "kind": "graph", "controls": {"out": "mix"}}` hands the child the parent's `mix`). A bus the instantiation was not handed is allocated normally, so the same def works standalone and nested. An external bus is never reclaimed by the child — it was never the child's.
- **Its interface is re-exported.** A surface target on a graph member names one of the child's **ports** instead of a control: `{"member": 1, "port": "gain", "mul": 2.0}`. The two scalings compose, outer first.

The nesting is **of authoring, not of execution**. A port resolves, once at instantiation, to the controls of actual nodes however many levels down they were written, so `/node_set` costs what it always did and the audio thread sees groups and synths as it always has. Depth is capped at 8, which is also what refuses a graph that contains itself.

A **GraphDef can be bound to MIDI**, its notes its voice slots rather than a `Voices` node's voices: `/midi_bind channel graphname [target addAction gate]` spawns the shared instance once at bind time and every note becomes a `/graph_newVoice` into it (note → `freq` port, velocity → `amp` port, note-off frees the voice or, for a gate-aware binding, sets its `gate` port to 0). The GraphDef must have per-voice members. `/midi_unbind` frees the shared instance (and all its voices).

## Event buses and the `Voices` node

The server has three kinds of bus, numbered from 0. An **audio bus** carries a block of samples and a **control bus** one value; an **event bus** carries the notes and channel messages written to it during a block, each at its sample. A node reads the event bus its `notes` control names, as it reads the audio bus its `in` names, and whatever reads a bus plays what is written there: a plugin forwards it to its note input, a `Voices` node gives each note a voice. What writes a bus is never a node — the system MIDI input (through `/midi_bind`), an event lane, and a client's `/bus_event` — so a reader always finds a slice's events written before the slice runs. How many there are is boot configuration (`--event-buses`, `[server] event_buses`, default 64); every bus shares one list of 1024 events a block, allocated at boot, and a block that writes more than that drops the rest.

An event is normalized: MIDI 2.0 resolution, values in their natural units, sixteen channels. A note carries an identity (`note_id`, `0` or more, or `-1` for a note named by its channel and key, as MIDI names one) and every event carries its **source** — the writer it came from — so what one writer started only that writer ends: a lane's notes are released when the lane is cleared and a transport's when it stops, while the keyboard's go on. A key's note-off its writer still owes a note a release cut is that note's, and ends no new note on the same key.

- **`/bus_event <int32 bus> <json>`** writes events on event bus `bus` at the sample its bundle names (immediately, outside one). The JSON (a string or a blob) is one event or a list of them:

  ```json
  {"kind": "note_on", "channel": 0, "key": 60, "note_id": 7, "velocity": 0.8}
  {"kind": "note_on", "key": 62, "note_id": 8, "joins": 7}
  {"kind": "note_off", "note_id": 7, "velocity": 0}
  {"kind": "choke", "channel": 0}
  {"kind": "expression", "expression": "tuning", "note_id": 7, "value": 0.5}
  {"kind": "controller", "channel": 0, "controller": 74, "value": 0.5}
  {"kind": "bend", "channel": 0, "value": -0.25}
  {"kind": "pressure", "channel": 0, "value": 0.4}
  {"kind": "program", "channel": 0, "program": 3}
  ```

  A `note_on` names its key (0–127) and channel (0–15, default 0), its velocity 0–1 (default 1); with `joins` it **continues** the note of that id — a legato line — and names its own `note_id`: a `Voices` node moves the voice it already holds to the new key, and a plugin gets the new note before the old one ends, played as it plays overlapping notes (on the same key, the held note goes on as the new one). A `note_off` ends a note by `note_id`, or by `channel` and `key` (the source's oldest such note). A `choke` stops at once every note whose given fields match. An `expression` is one note's (by `note_id` or `key`) or the channel's (both `-1`), one of `volume` (0–4), `pan` (0–1, 0.5 the center), `tuning` (semitones from the key), `vibrato`, `expression`, `brightness` (MPE's timbre) or `pressure` (each 0–1). A `controller`'s value is 0–1, a `bend` −1 to 1, a `pressure` 0–1. Every address defaults to `-1` and `source` (a client's own number, to keep two writers apart) to 0. A field out of its range replies `/fail` and writes nothing.
- **`/node_event <int32 node> <json>`** gives the same events to one node that reads notes — a plugin's or a `Voices` node — as though they came from its bus, and adds what only an event for one node can say: `{"kind": "control", "control": "cutoff", "value": 900, "note_id": 7}` sets one of the node's controls for that note's voice alone (or, without a `note_id`, for the node). A plugin's controls are the instance's, set with `/node_set`, so a plugin refuses a control event.

**A plugin reads notes from a bus.** Each note input of a plugin node is a control: `notes` for the main one, `notes_<name>` for the others and `notes:<id>` by native id (see the catalog's `controls`), unconnected (`-1`) until one is set. What the bus carries is sent in the port's dialect: native notes and expressions where the port takes them, MIDI where it takes only that (a note's identity is then its channel and key), and what the port has no spelling for is dropped rather than faulting the instance. A note that collides with a held one ends it first, as a retrigger; a joined note retunes the held note on a native port and overlaps it on a MIDI one. A burst larger than one job is spread over the next blocks, and a note past the 256 an instance holds is not played.

**A `Voices` node is N voices of one def.** **`/voices_new <string def> <int32 id> <int32 addAction> <int32 target> <int32 voices> [control value ...]`** makes one node holding `voices` (1–256) instances of a SynthDef or a FaustDef, all built with the node. On the audio thread a note takes a free voice, resets it on its own memory (every UGen back to its first block, a Faust instance cleared) and starts it with the note's controls; a voice whose def frees itself (a done action) is free again; a free voice is not processed; and when every voice is busy the note takes the oldest released voice, else the oldest held. In the tree it is one node like any other, so a note on the event path never makes one. Its controls are its def's — a set on the node reaches every voice and is kept for the voices to come, as do its `in`, `out` and bus maps — plus `notes`, the event bus it reads. A plugin def (which plays its own voices), a GraphDef (whose notes are its voice slots) and a def holding a `DiskIn`/`DiskOut` (whose stream a voice cannot restart) are refused.

What a note sets on its voice is the node's **voice map**: the key and the note's tuning as the frequency (`freq` unless mapped otherwise), the velocity as the amplitude (`amp`), the gate opened at the note's start and closed at its end (`gate`; a def without one has its voice freed by the note's end), and a note's tuning, pressure, brightness and release velocity, a channel's bend and pressure and each controller, to the controls the map names. A channel's controller, bend and pressure are the node's, set on every voice and kept for the next. A joined note keeps its voice and moves it to the new key and velocity. **`/voices_map <int32 node> <string selector> <string control>`** sets an entry: `note`, `vel`, `gate`, `bend`, `pressure`, `poly` (a note's own pressure), `timbre`, `lift` or `ccN`, an empty control clearing it; a control the def does not have replies `/fail`. The map starts with `freq`, `amp` and `gate`, and `press` and `slide` (the built-in `default`'s) as pressure and timbre where the def declares them.

## MIDI control protocol (standard channel-voice actuation)

Besides OSC, the server can be driven by **standard channel-voice MIDI** — note on/off, velocity, aftertouch, pitch-bend, control change, program change — the surface a sequencer or DAW already speaks. (SysEx, when it lands, is reserved for the non-musical control plane — def load, buffers, topology — and is never a tunnel for OSC commands.)

**A binding is routing.** A channel writes an event bus, and whatever reads the bus plays it (see "Event buses and the `Voices` node" above): a note makes no node. Binding and mapping are OSC commands (so they ride the same reliable path as the rest of the protocol); the note and control events themselves arrive over the OS's standard MIDI. Start the server with `--midi [name]` (RT only) to open a **virtual ALSA input port** (default name `clausters`) — the same system MIDI any controller or DAW uses; route anything into it (`aconnect`, a keyboard through the kernel, a DAW). Live input is MIDI 1.0 (7-bit, widened internally to the high-resolution form); the full MIDI 2.0/UMP resolution is preserved on the client's persistence path (the `clausters-midi` crate's MIDI 2.0 clip file). Network MIDI is a separate, deliberately out-of-scope idea.

- `/midi_bind channel bus`: route a MIDI channel (`0`–`255`: the classic 16 plus the extended UMP group×channel space) to event bus `bus`. Whatever reads that bus plays the channel — an existing `Voices` node or plugin node whose `notes` names it, or several. The channel keeps its number on the bus.
- `/midi_bind channel instrument [target] [addAction] [gate]`: the same routing with a node of its own, made on an event bus of the binding's (the top half of the event buses is theirs) and freed with it: a **`Voices` node** of 16 voices of a SynthDef or a FaustDef, its voice map the binding's control map; or a **plugin**'s node, one instance whose main note input reads the bus. Default target is the root group (`0`), default add-action `0` (head); `gate` is accepted and ignored (a voice closes its gate when the def has one). A **GraphDef** is the exception: its shared instance is spawned at bind time and each note becomes a `/graph_newVoice` into it (see the GraphDef section); it must have per-voice members.
- `/midi_unbind channel`: remove the binding and free what it made (its node and bus, or the GraphDef instance and its voices).
- `/midi_bindZone master members bus` and `/midi_bindZone master members instrument [target] [addAction] [gate]`: route or bind an **MPE zone** (see "MPE zones" below). `master` is channel `0` (the lower zone, members ascending from 1) or `15` (the upper zone, members descending from 14); `members` is `0`–`15`, `0` meaning "wait for the device's own layout". Refused when the zone's channels would reach one a `/midi_bind` holds, as `/midi_bind` is refused on a channel a zone covers. A plugin zone needs a native note-expression port.
- `/midi_map channel selector name [cc]`: a control an instrument binding's node takes from a message type — on a `Voices` node, an entry of its voice map (`/voices_map`). Selectors: `note` (the frequency control, default `freq`), `vel` (the amplitude, default `amp`), `gate`, `bend`, `pressure` (channel aftertouch), `poly` (a note's own pressure), `lift` (note-off velocity), `ccN` (control change `N`), and on a zone's master `timbre` (the zone's third dimension; the optional fourth argument is the controller that carries it, 74 unless given). A control the def does not have replies `/fail`. On a plugin's node a controller, pressure or bend mapped to one of its controls sets that control instead of reaching the plugin; the note selectors are refused there. A channel routed to a bus has nothing to map — map on the node that reads it — and a program change is the reading node's to interpret, so `progN` is refused.
- `/midi_ump word...`: **MIDI 2.0 in**, each int a Universal MIDI Packet's 32-bit word, played as the live input plays its messages. A Channel Voice 2 note, controller, pressure, bend or program change is written as its MIDI 1.0 twin is, at its own resolution (a 16-bit velocity, 32-bit values; a velocity of 0 is the softest note, never an off); a MIDI 1.0 message carried in a packet is its bytes; and a **per-note** pitch bend (48 semitones either way) is that note's tuning, a per-note timbre controller (74) its brightness. Any other packet plays nothing. The MIDI port reads MIDI 1.0 bytes; a port that carries packets is not open yet.
- `/midi_query [channel...]`: read the bindings back. One `/midi_query.reply kind channel members instrument target addAction gate selector name ...` per binding, then `/done "/midi_query"`. `kind` is `"channel"` or `"zone"` (`channel` its master, `members` as the device last set them, and a `timbreCc <int>` pair); the pairs are every selector that names a control, then `bus <int>` (the event bus the channel writes) and, for an instrument binding, `node <int>` (the node it made). A routed binding's instrument is empty. With no argument every binding is listed; a channel asked for and bound to nothing replies `"none" channel`.

What a message writes on the bus, from the MIDI input's source for its channel:

- **Note on** → a note-on of that channel and key, named by them (`note_id` `-1`), the velocity scaled to 0–1. Velocity `0` is a note-off.
- **Note off** → a note-off of that channel and key: the oldest such note of the channel.
- **Poly aftertouch** → that key's `pressure` expression; **channel aftertouch**, **control change**, **pitch-bend** and **program change** → the channel's pressure, controller, bend and program. A reader takes what it plays: a `Voices` node the ones its map names, a plugin what its port has a spelling for.

### MPE zones

MIDI Polyphonic Expression gives each note a channel of its own, so a bend, a pressure and a third dimension move **that note**. A **zone** is a master channel and a run of member channels: the lower zone's master is channel `0` with members from `1` up, the upper zone's master `15` with members from `14` down, the two sharing fifteen. The model is per note — each note-on gets an identity, and a member's expression reaches the note sounding on it — and MPE is a decoder in front of it (the `clausters-midi` crate's, shared with the GUI host and the clients).

- **A zone is heard only where it is bound.** `/midi_bindZone` says where; the device's RPN 6 (the MPE Configuration Message, over CC 101/100/6/38 on the master) then sizes it and **wins**, since a device is authoritative about its own layout. The RPN is read on a bound zone's master and nowhere else, so a channel no zone covers passes every control change through raw. An RPN that would widen a zone over a bound channel is refused and the layout stands. A zone of zero members still plays notes on its master, the master's bend as pitch: a device's "plain MIDI again".
- **Each note by its identity.** A note-on writes the note (its member channel and key, `note_id` the decoder's) and the bend, pressure and timbre it starts with as its `tuning`, `pressure` and `brightness` expressions; after it, a bend, a pressure and the timbre controller on its **member** are that note's expression, and on the **master** every note's of the zone. The tuning is `member_bend * member_range + master_bend * master_range` semitones, the ranges 48 and 2 unless the device sets them (RPN 0: on a member for every member, on the master for the master). The sustain pedal and another controller are the zone's, from its own source.
- **Playable unmapped.** A `Voices` node's map starts with `press` and `slide` (the built-in `default`'s) as the pressure and the timbre, so a def that declares those controls plays the three dimensions with no `/midi_map`, and the `default` plays them with nothing sent.
- **A lane's MIDI decodes the same way.** An event lane's `midi` list is read through the zones' layout when it is set, so an MPE stream on a lane plays per note on its samples.

Conversions take **MIDI 2.0 / UMP resolution** (16-bit velocity, 32-bit controllers, pressure and bend) to the values an event carries — no 7-bit quantization. **MIDI 1.0 is backward-compatible**: classic 7/14-bit input is accepted and widened up to those. A MIDI note is the same event a client's `/bus_event` writes, only from another source (`crates/clausters-server/tests/offline/midi.rs` guards this).

## Node tree introspection

The node tree is delivered to clients as **structured replies** — never scraped from the server's logs. Three queries (all answered by the network-side tree mirror, so they never touch the audio thread):

- **`/group_queryTree <groupID> [detail]`** → `/group_queryTree.reply` — the whole subtree from `groupID` (use `0` for the root). Args: `detail`, the group, its child count and its name, then depth-first per node: ID and child count (`-1` marks a synth) and then a name — the group's own (empty when unnamed) or the synth's def name — and per level the same payload `/node_query.reply` carries. Every node reads `ID, count, name` — one shape for both kinds, rather than a name only where it is new. `detail` is scsynth's `flag` widened into a level: **0** the shape alone, **1** the control count followed by (name|index, value) pairs (both scsynth-compatible), **2** also the maps and the inferred bus lists — which is what makes every entry a **full node record**, so a client reading a tree needs no follow-up `/node_query`. Values above 2 clamp to 2. At **2** a *group* also carries the two modes it runs under, as two ints right after its name: `/group_sortMode` (1 = the server orders its children by the buses they touch) and `/group_parallel` (1 = the independent ones run on the DSP workers). The queried group carries them too, right after its own name and before the first child — so the first entry starts at argument 6 rather than 4 at that level. They ride at 2 because 0 and 1 are scsynth's shape.
- **`/node_query <nodeID>...`** → one `/node_query.reply` per node — the same record for one node, plus where it sits. Layout: `nodeID, parentID, prevID, nextID, isGroup`; then for a **group** `headID, tailID` (`-1` if empty), its `/group_name` (empty when unnamed) and the two modes it runs under as two ints, `/group_sortMode` and `/group_parallel`; for a **synth** `defName`, control count + (name|index, value) pairs, map count + (controlIndex, bus, audio) triples (the `/node_map`/`/node_mapAudio` bindings), and the inferred `reads`/`writes` bus lists as two strings (`"0,16"`, or `"-"` when none). Siblings are `-1` when absent.
  - A node the server does **not** hold answers `nodeID, -1, -1, -1, -1` — **`isGroup` = -1 means the node is gone** — rather than `/fail`. Absence is a *state* of the resource, reported in the record like `/def_query`'s empty family and `/buffer_query`'s `frames = -1`, so one dead id never aborts a query about the others. Only a malformed request (a non-int id) is a `/fail`.
- **`/group_dumpGraph <groupID>`** → `/group_dumpGraph.reply [groupID, text]` — a human-readable rendering of the inferred bus graph (what each child reads/writes and the current order), with each group's name quoted next to its ID, followed by the global planned intrinsic latencies, input/writer compensation delays in samples and uncompensated boundary reasons. A debugging aid; for machine use prefer `/group_queryTree`/`/node_query`, which is also where a group's `auto`/`manual` and `parallel` modes are read as flags rather than out of this string.

The tree also **pushes**: a `/server_notify` client gets `/node_start` and `/node_end` as `nodeID, parentID, prevID, nextID, isGroup, name` (siblings are `-1` — this side does not track them; `name` is the group's `/group_name`, empty for a synth or an unnamed group). One shape for both kinds, so a client reads the record the same way whichever node it is about. These leave the audio thread through a lock-free FIFO and are sent by the network thread, so a registration also **buys promptness**: while any client is registered the server's idle tick shortens to the cadence a subscribed stream gets (10 ms), because the audio thread cannot wake the network loop the way a worker thread does. Without that the notification would wait for the housekeeping tick — measured at ~104 ms on an otherwise quiet connection.

The clients ask at whichever end the question belongs to: the **server** is asked about every node it holds — `Server.query_tree()` / `queryTree()`, which returns a tree of node records and prints itself indented — and a **node** is asked about itself, `node.info()` in both clients. `Server.dump_graph()` / `dumpGraph()` returns the debug string.

## Def, buffer and UGen introspection

Where the queries above read what is *playing*, these three read what the server **holds** — the samples a client can build with. All are retrieval only: they add no semantics and change nothing.

They exist because the answers are not derivable client-side. The def store **persists across restarts**, so a running server may hold defs no client in this process ever sent; the buffer pool likewise outlives any one client; and the UGen catalog depends on how the server was built. A tool that offers the user a palette of what can be instantiated has to ask.

Each of the two multi-reply queries sends **one reply message per item** — the payloads are variable-length, so an aggregate would nest badly and a large catalog would outgrow a UDP datagram — and closes the batch with `/done "<command>"`, which is how a client with no argument knows the listing ended.

- **`/def_query [name...]`** → one `/def_query.reply` per def, then `/done "/def_query"`. With no argument, every loaded def of every family. Layout: `name, family, numControls`, then per control the same record in every family: `name, default, rate, min, max, step, warp, unit, label, group, flags, numLabels, label...`.
  - `family` is `"synth"`, `"faust"` or `"graph"`; a def the server does not hold comes back with an **empty** family and no controls, rather than failing the batch (the `/buffer_query` convention for an unallocated slot).
  - `rate` is the control type the def declared — `kr`, `tr` or `ir`, the same set the [control-type table](#control-types) defines, reported under the same field name `/def_send synth` accepts it under. Note this is a **different vocabulary** from the calculation rates `/ugen_query` reports below: a control is a value pushed in from outside, so `ar` is not among them (map an audio bus with `In` + `/node_mapAudio` instead) and neither is `dr` (a demand value is *pulled* by a driver — there is nowhere for a `/node_set` to land). `tr` in turn is a behavior rather than a rate, which is why the server's own type is named `ControlType`.
  - `min, max, step, warp, unit` are how the control is drawn: what a SynthDef control [declared](#control-types), a **faust** slider's range and step with the unit its `[unit:...]` metadata declares and an `exp` warp for its `[scale:log]` (over a range above zero), a plugin parameter's. `min` equal to `max` (both zero) is no declared range, `step` zero is continuous, and an empty `warp` or `unit` is none. `label` is a plugin parameter's own name (empty is the control's name), `group` its module, `flags` its bits — stepped `1`, hidden `4`, read-only `8`, automatable `32`, a list `65536` — and the labels name a list's steps, lowest first; all of them empty or zero in the other families. A faust def's reserved `out`/`in` bus controls are engine plumbing and are not reported.
  - A **graph** def reports its surface **ports** instead of controls: after a port's record, `numTargets` and then per target `member, control, mul, add` — the inner controls it drives and the linear scaling it applies, so a patcher can draw the port's real connections.
- **`/buffer_query`** with **no argument** → one `/buffer_query.reply` listing every **allocated** buffer, four args each (`bufnum, frames, channels, sampleRate`) — the same shape the per-index form replies with, so one parser reads both. (With arguments it is the per-index query documented under [Buffers](#buffers-buffer_).) The per-index form answers **`frames = -1`** for a slot with nothing in it, the same "absent is a state" rule `/node_query` and `/def_query` follow; the listing form simply omits it.
- **`/ugen_query [kind...]`** → one `/ugen_query.reply` per UGen, then `/done "/ugen_query"`. With no argument, the whole catalog. Layout: `name, arity, defaultRate, rates, exec, bus, needsPath, opFamily, spectral, numInputs`, then per input `name, default`.
  - `arity` is the input count, or `-1` for a **variadic** kind — whose named inputs are then its **fixed head** only (`EnvGen`'s five before the envelope array, `Dseq`'s `repeats`).
  - `rates` is a comma list (`"kr,ar"`) of the rates the kind may be instantiated at; `defaultRate` is the one a def gets by omitting `rate`.
  - `exec`, `bus`, `opFamily` and `spectral` expose the compiler's own classification (`normal`/`local_in`/`local_out`/`demand_driver`/`spectral`; `read`/`write`/`read_write` for an audio bus, `read_control`/`write_control` for a control bus, `read_controls` for a run of them; `unary`/`binary`; `source`/`trigger`/`copy`/`filter`/`filter2`/`sink`), each an empty string when it does not apply.
  - The input **names are descriptive**: the wire stays positional and no input is ever addressed by name. They are what a palette labels an inlet with, and `default` what to offer when the user leaves a slot alone — the server applies no default of its own, a def that omits an input is simply short and fails on arity.
  - An unknown kind comes back with an empty rate set and no inputs. A server built **without the `synth` feature** has no UGen catalog at all and replies with an empty listing (not a failure) — its defs would all be FaustDefs, and the Faust box vocabulary is Faust's own, held client-side rather than published here.

The Python client wraps these as `Server.query_defs()`, `Server.query_buffers()` and `Server.query_ugens()`, returning `DefInfo`/`BufferInfo`/`UgenInfo` records — the web client as `queryDefs()`, `queryBuffers()` and `queryUgens()`, returning the same records in camelCase. `crates/clausters-server/examples/introspect_server.py` walks all three.

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
- `/node_ugenCmd <nodeID> <ugenIndex> <name> args...` — a command addressed to **one UGen instance** inside a synth. It validates the node is a UGen synth and `ugenIndex` is in range (a Faust synth is one opaque block, not a UGen graph, so it is rejected), packs the numeric args inline (up to 8, so nothing heap-allocated crosses to the audio thread), and routes them to that UGen on the audio thread; the command name is hashed to a stable selector both sides agree on. The first real consumer is the FFT chain: `/node_ugenCmd <nodeID> <ugenIndex> window <wintype>` sent to an `FFT` changes its chain's window live, analysis and synthesis both (see the FFT-chain note above). A UGen that does not recognize the command name ignores it, so a `/node_ugenCmd` to a valid target is otherwise accepted silently.

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
| `<dir>/midi.json` | `/midi_bind`/`/midi_bindZone`/`/midi_unbind`/`/midi_map` | the MIDI bindings (channel → event bus, or instrument + target + control map; a zone's entry is its master with its `zone` configuration) |
| `<dir>/boot.json` | *authored by the user/client* | the boot preset: standalone GraphDefs to instantiate at startup |

GraphDefs reload after the synth/faust defs (their members reference those names); validation is structural only, so a member def that is still missing at load is caught later, at `/graph_new`.

**A def that no longer loads is named, and dropped only when asked.** A UGen that gains an input makes every stored def written against the old shape uncompilable, and the reload warns and skips each one — the right call, since a def may also fail because *this build* lacks its family (a `--no-default-features` server would otherwise eat the library on sight). So the warning names the def and, once, the directory it is in, and `--prune-defs` is the one door that removes them: it drops exactly the defs that failed to load in the families the running build has.

### MIDI-standalone: bindings + boot preset

So the server can be **played from a MIDI controller with no OSC programming at all**, the MIDI bindings persist too. Every `/midi_bind`/`/midi_bindZone`/`/midi_unbind`/`/midi_map` rewrites `midi.json`; at startup — **after** the defs and GraphDefs are in place, so a binding's instrument name resolves — each binding is re-established (an instrument binding makes its node again, a GraphDef binding its shared instance). The minimal workflow becomes: drop a SynthDef/FaustDef (or a GraphDef) and a binding in the data dir once, then every later `clausters --midi --data-dir <dir>` comes up already bound — connect a controller (`aconnect`) and play. The default voice map (key→`freq`, velocity→`amp`, the note's end closing `gate`) makes a restored binding immediately playable.

`boot.json` is an optional, user-authored **boot preset**: a JSON array of standalone GraphDefs to instantiate at boot (an always-on reverb bus, a drone, a mixer), each `{"graph": "<name>", "ports": {"<port>": value, ...}}`. They are instantiated (the equivalent of `/graph_new <name> -1 0 0 ...`) after the bindings, so a fresh boot comes up already wired. The boot order is **defs → graphdefs → bindings → boot preset**. All of it honours `--no-persist` (off → nothing read or written) and the data-dir resolution; it applies to the real-time server only (NRT never persists).

### A name identifies one def, and `tmp_` names are never stored

A def **name is unique across all three kinds**, not per kind. Sending a def under a name another kind holds *replaces* it — last one wins, and the loser's persisted files are deleted with it. Without that rule the two records both survive a restart and the reload order silently decides which one a `/synth_new` (or a `/def_query`, or the bus-usage inference the parallel scheduler reads) resolves to.

A name beginning with **`tmp_`** marks the def as **ephemeral**: the server compiles and keeps it in memory for the session but never writes it to the data directory. It is for defs a client builds to carry an expression that has no name of its own — the Python client's `as_def` names them `tmp_synthdef_<id>` / `tmp_faustdef_<id>` — and for the ones the shared crates **generate** from what is playing — the multitrack's mixer (`tmp_mt.…`), the readers and graphs a sequence's curves play through (`tmp_ev.…`, a note or channel graph named for the lane it plays from, `tmp_ev.<lane>.note.<hash>`, and freed with `/def_free` by whoever sent it once nothing plays it). None has a meaning to a later session, and they would otherwise accumulate forever. An ephemeral Faust def is instead **cached by its content** — the libfaust version, the front-end, the compiler's arguments and the payload, hashed, and never the name, which is new on every send — in a `clausters-tmpdefs` directory: under the OS temp directory natively (its bitcode), and at the root of the page's own storage in a tab (the compiled module and the compiler's JSON, keyed by the commit the vendored compiler was built from and bounded to the newest few hundred). So the same expression evaluated again skips the compile — in a tab, the compiler's whole download — while the store stays clean. The prefix is the whole signal: a def *you* name `tmp_...` is ephemeral too, by that definition.

Two properties are what make a generated name safe, and a client that generates its own should have both: **globally unique** (a per-process counter restarts at 0 next run, and the store outlives the process, so the names collide with the previous session's), and **distinct per kind** (one counter feeding two kinds lets a SynthDef and a FaustDef claim one name).

The stored **definition** (the JSON) is always the source of truth: it is transparent, human-readable, and what gets recompiled. The Faust `.bc` is a non-authoritative cache — on reload the server re-creates the factory from bitcode (skipping Faust's front-end) only when the libfaust version still matches and the file is intact; otherwise it silently recompiles from the source and rewrites the cache. A libfaust upgrade therefore invalidates every `.bc` automatically. `/def_free <name>` deletes both files. Re-sending a name overwrites them.

Reloading is **incremental**: the socket starts serving immediately and the library loads in the background on the compiler thread, so a large Faust library does not delay startup — a def simply "does not exist yet" until its reload finishes.

## Generating defs programmatically

`crates/clausters-server/examples/json_client.py` (Python, stdlib only) builds all three formats with a few helper functions and drives the whole lifecycle over OSC — use it as a reference client. The equivalence of the families is pinned down by the golden tests in `crates/clausters-server/tests/faust_offline/faust_parity.rs`: a UGen graph, the box translation and the signal translation render side by side in one engine and must agree (bit-exactly for stateless arithmetic on shared input, within float tolerance for oscillators, since `Sine` accumulates phase in f64 and Faust in f32).

## Persistent plugin events

`/plugin_event <node:int> <event:JSON-string-or-blob>` is schedulable in OSC
bundles and on both sample axes. It targets the existing instance, never
creates a synth per note. The same events can be entries of an event lane's
`messages` list. Unsupported ports, kinds, expression IDs or dialects reply
`/fail`; runtime capacity failures latch a per-node fault and silence.

```json
{"kind":"note_on","source":0,"note_id":7,"port":0,"channel":0,"key":60,"value":0.8}
{"kind":"expression","source":0,"note_id":7,"port":0,"channel":0,"key":60,"id":6,"value":0.5}
{"kind":"note_off","source":0,"note_id":7,"port":0,"channel":0,"key":60,"value":0.0}
{"kind":"midi","source":0,"port":0,"bytes":[176,64,127]}
```

`port` is a stable native note-port ID; omitting it selects the first input
note port. `source` is a client-chosen unsigned 32-bit origin, default zero;
clients sharing an instance allocate distinct origins. `note_id` is a
nonnegative identity within that origin and port, default -1. Without an
identity, note-offs release the oldest matching attack by channel/key.
Explicit IDs may overlap on the same key, but an ID cannot be attacked twice
before release; retain distinct IDs while any older commands for those notes
remain queued. The server assigns unique native note IDs across origins.
`channel` is 0–15 for attacks, default zero; `key` is 0–127. `value` is velocity
0–1 for attacks/releases, or a native expression value. An expression's `id`
is CLAP's 0 volume, 1 pan, 2 tuning (semitones), 3 vibrato, 4 expression,
5 brightness or 6 pressure. Expressions and `choke` can use -1 for note ID,
channel or key, applying only to matching notes of their own origin.
`midi` carries exactly one three-byte MIDI 1 channel-voice message; its running
status, SysEx and MIDI 2 native dialect are not supported by this command.

Native expression requires a native note-expression port (CLAP or VST3). MIDI-only ports receive converted
note attacks/releases but cannot distinguish overlapping attacks of the same
channel/key: the node reports a protocol fault rather than merging ownership.
MIDI notes, sustain, pressure, pitch bend and standard volume/pan/expression/
brightness controllers can be translated to CLAP note events when no MIDI
dialect is offered. Other unsupported controllers report an error.

For VST3 the adapter converts volume to its normalized gain range (amplitude
`0..4` becomes `0..1`), tuning in semitones to its centered normalized range
(`-120..120` becomes `0..1`), and pressure to poly pressure. `choke` becomes an
immediate note-off; VST3 has no distinct choke or note-end event. MIDI notes
become native notes and CC/pressure/bend use the controller's `IMidiMapping`
assignments, read before processing; an unassigned controller has no effect.
Editor callbacks and output parameters use the same parameter/gesture replies.
A plugin request to change ports, parameters or latency fails the current
generation safely; `/plugin_restart` prepares a new one with compatible IDs
and ports, including refreshed latency. The general server compensation plan
is rebuilt for the replacement declaration.

`/midi_bind` on a plugin def builds **one persistent instance at binding time**,
its main note input reading the binding's event bus. MIDI/UMP input goes
through the shared decoder onto that bus, so MPE (which needs a native
expression port) keeps the decoder's note identities. `/midi_unbind` frees the
instance. Mapped parameter controls still use `/midi_map` on plain-channel
bindings (CC, pressure, poly pressure or bend); otherwise native controllers
reach the plugin. Note/velocity/gate selector maps and MPE parameter remapping
report unsupported routing. Note velocities and native expressions preserve
the decoded precision; MIDI-only controllers use the plugin's MIDI 1
resolution.

A lane note's voice may be `{"plugin": <node-id>}`, a node that reads notes (a
plugin's or a `Voices` node), with controls `key`, `velocity` and `channel`:
its note-on and note-off go to that node (`/node_event`), the note identified
by the lane. Both `gate` and `free` release that **note**, leaving the node
alive. Lane MIDI/UMP and plugin messages carry separate lane/generation
origins; clearing, replacing or freeing a lane releases its notes without
releasing live notes on that node. A transport stop, locate or loop wrap releases plugin notes in its
governed subtree. Freeing a plugin node ends its helper and all owned notes.
A frozen subtree emits silence and freezes plugin DSP; releases are delivered
before new attacks when processing resumes. Ungoverned live nodes keep
processing while transport is stopped. Each helper sees monotonic device time
relative to its instance's first block, including frozen blocks, and the
nearest bound transport's position, playing state and BPM at each slice
(default 120 BPM before a grid is declared).
Position/playing discontinuities are carried at their original sample offsets;
seconds and beat positions agree at the supplied tempo.

One instance owns at most 256 notes and 64 sustain-origin/channel records.
Every block has at most 64 input events and eight spans. Overflow from
`/plugin_event` is explicit failure, followed by silence and explicit restart;
a release burst, and what an event bus delivers, is spread over the next
blocks instead. Held notes are cleared on restart; old-generation
events cannot actuate the replacement.
