# Def schemas

User-facing reference: wire formats and OSC commands. For the server's internals — threads, memory lifecycle, invariants, adding UGens — see [`architecture.md`](architecture.md).

Clausters accepts instrument definitions in two wire formats, both loaded hot over OSC (UDP, default port `127.0.0.1:57110`):

| format | OSC command | availability |
|---|---|---|
| **SynthDef JSON** — a flat list of UGens | `/d_recv <blob>` | `synth` feature (default) |
| **Faust def** — Faust source *or* a JSON box tree | `/d_faust <name> <string>` | `faust` feature |

The two families are independent build features and combine freely; a server built without one replies `/fail` (naming the missing feature) to that family's def command. See the feature matrix in [Using clausters as a library](using-as-a-library.md) and `BUILD.md`.

Both reply asynchronously: `/done` with the command (and the def name for `/d_faust`), or `/fail` with a human-readable error. Once loaded, defs of either kind are instantiated, controlled and freed the same way:

### The async barrier (`/sync`)

`/sync <int id>` is the general way to wait for asynchronous work: the server replies `/synced <id>` once **every async command received before this `/sync`** has completed — Faust compiles (`/d_faust`), SynthDef sends (`/d_recv`) and buffer jobs (`/b_*`), each of which runs on its own FIFO worker thread. Use it instead of matching individual `/done`s when you fire several async commands and just want to know they have all landed (e.g. after sending a def without waiting, before the `/s_new` that needs it). It is a real barrier, not a round-trip of `/status`: a `/synced` guarantees the prior compiles/jobs are installed.

```text
/s_new  name id addAction targetID [ctlName value]...   # ctl args: s f pairs
/n_set  id ctlName value...                             # one control per pair
/n_setn id ctl numControls value...                    # a consecutive range
/n_fill id ctl numControls value                       # fill a range with one value
/n_map  id ctlName busIndex...                          # control bus -> control
/n_mapa id ctlName busIndex...                          # audio bus  -> control
/n_mapn id ctl busIndex numControls...                 # map a range to consecutive buses
/n_mapan id ctl busIndex numControls...                # audio-bus range variant
/s_get  id ctl...                    -> /n_set id (ctl value)...        # read controls
/s_getn id ctl numControls...        -> /n_set id (ctl numControls value...)...
/n_free id...
/n_trace id...                                          # log a node's state (debug)
/s_noid id...                                           # acknowledged (see note)
/d_free name...                                         # SynthDef JSON only
```

Engine facts that apply to every def: blocks of 64 samples; **by default** 128 audio buses (`0..outputs` are the hardware outputs, bus 0 = left; `outputs..outputs+inputs` are the hardware inputs) and 16384 control buses; a pool of 4096 sample buffers filled by the `/b_*` commands; all signals are `f32` at the configured sample rate. Everything sized at boot, never at compile time: the bus counts by `--audio-buses` (≤128) and `--control-buses`, the sample rate by `--sample-rate` (default 48000; PipeWire honors it per-application), the hardware channel counts by `--outputs` (default: the device's) and `--inputs` (default 0), the pre-allocated pools by `--max-nodes`, `--max-buffers`, `--max-graph-children` and `--max-ugen-inputs` (≤32), and the audio-tap region by `--taps` and `--tap-frames` (defaults 8 × 16384; see "Audio taps" below). Each flag also has a `[server]` config key of the same name (`max_nodes`, `outputs`, …), with the CLI winning. A client reads the live configuration with **`/server_info`** → `/server_info.reply [audio_buses, control_buses, output_channels, block_size, nominal_sr, actual_sr, input_channels, max_nodes, max_buffers, max_graph_children, max_ugen_inputs, taps, tap_frames, max_frame]`, so it can size its own bus/node allocators from the server instead of assuming the defaults. The first six fields are the stable original set; the S7 capacities, the tap region shape and the stream-transport frame ceiling (`--max-frame` — what a client should size bulk requests like `/b_getn` chunks from, see "Transports" below) are appended, so a client that reads only the first six keeps working.

**Node ids are a finite, boot-partitioned resource.** Like every capacity above, the node-id space is fixed at boot and **every range of it recycles** — an id allocator (server- or client-side) is a *registry* of ids in use, never a counter that only climbs. The space is partitioned by one shared formula (`clausters_core::registry::NodeIdPartition`, scaled from `max_nodes`, the resource that actually bounds concurrent nodes): a **client range** starting at 1000 (width `4 × max_nodes` — table capacity plus in-flight margin), then the server's **auto range** (`/s_new` with id `-1`, GraphDef groups and members) and its **MIDI voice range**, each `2 × max_nodes` wide. At the default `--max-nodes 8192` that is client `1000..33768`, auto `33768..50152`, MIDI `50152..66536`. A client derives the partition from the `max_nodes` field of `/server_info.reply` with the same formula (exposed by the core FFI), so client and server agree by query, not convention. Ids return to their registry when the node dies: the server releases its auto/MIDI ids on each node's end (and on an engine rejection), and a client releases its ids from the same **`/n_end`** notifications — register with `/notify 1` and free on `/n_end` (releasing at `/n_free`-send time would re-hand an id whose node is still alive). When the engine **rejects** a node asynchronously (duplicate id, bad target, full table), no `/n_end` will ever come, so the server broadcasts **`/fail "/s_new" <message> <int id>`** to every `/notify` client — the appended id (not part of scsynth's two-argument `/fail`) is what lets a client registry reconcile the in-flight id instead of losing it. Exhausting a range is an explicit error (`/fail … out of auto node ids`, or the client's own exception), never a silent wrap; in NRT/score mode the client id space is deliberately unbounded — an offline score has no live `/n_end` stream and no real-time bound on ids over its length.

**Server health** is one poll away: **`/status`** → `/status.reply [1, ugens, synths, groups, defs, avgCPU, peakCPU, nominal_sr, actual_sr, late_blocks]` — the scsynth field set plus one appended int. `avgCPU` and `peakCPU` are the audio thread's per-block processing time as a **percentage of the block budget** (`block_size / sample_rate` of wall time — at 48 kHz a 64-frame block must be computed in 1.33 ms): the average is an exponential moving average with a ~1 s time constant, the peak is the worst single block **since the previous `/status` poll** (reading it resets the window, so every poll reports the peak of its own interval; expect it to sit well above the average — the callback must fit its *worst* block, not its mean). `late_blocks` counts, cumulatively since boot, the blocks whose processing exceeded their budget — the engine-side xrun proxy. It is conservative: when the device quantum is larger than one block (PipeWire typically runs 256–1024 frames), a single late block can be absorbed by the rest of the cycle, so an occasional increment is a warning, a steady climb is audible trouble (cross-check with `pw-top`'s ERR column). Both meters read the real callback only: in an offline render they just measure render speed. `examples/stress.rs` builds a capacity ramp on top of this reply — see [Examples](examples.md).

**Live audio input.** With `--inputs N` the server also opens the default input device and exposes its `N` channels on audio buses `outputs..outputs+inputs`, so `In` (or `In.ar`) reading those bus indices returns live device input — a microphone, a loopback, another app's output routed in via PipeWire/JACK. Without `--inputs`, those buses read as silence (no input device is opened). The count is negotiated with the host and may be clamped; the value actually opened is the `input_channels` field of `/server_info.reply` (0 if the input device was unavailable).

### Mapping controls to buses (`/n_map`, `/n_mapa`)

`/n_set` writes a control once. `/n_map id ctl bus` instead **binds** the control to a **control bus**: the node re-reads that bus at the start of every block, so the control tracks whatever any client (`/c_set`) or synth (`OutCtl`) writes there — no further `/n_set`. `/n_mapa` is the same against an **audio bus**. Both take any number of `ctl bus` pairs, by control name or index, and work for UGen controls and Faust parameters alike.

A `busIndex` of `-1` removes the mapping (the control keeps its last value); a later `/n_set` on the same control also clears it and fixes the value.

### Ranges of controls (`/n_setn`, `/n_fill`, `/n_mapn`, `/n_mapan`)

The `-n` variants address **consecutive** controls in one message, for defs with array-like control blocks. `/n_setn id ctl numControls value...` sets `numControls` controls starting at `ctl` (by name or index) from the value list; several `(ctl, numControls, values...)` groups may follow. `/n_fill id ctl numControls value` fills such a range with a single value (repeatable in `(ctl, numControls, value)` triples). `/n_mapn id ctl busIndex numControls` maps `numControls` consecutive controls to `numControls` **consecutive** buses starting at `busIndex` (`busIndex = -1` unbinds the whole range); `/n_mapan` is the audio-bus form. Like `/n_set`/`/n_map`, all four accept a **group** id and propagate down its subtree, and clearing/setting a control that is used as a bus index re-sorts auto/parallel groups.

### Reading control values (`/s_get`, `/s_getn`)

`/s_get id ctl...` is the read counterpart of `/n_set`: it replies `/n_set id (ctl value)...` with each requested control's current value (by name or index). `/s_getn id ctl numControls...` reads a **range**, replying `/n_set id (ctl numControls value...)...`. Values come from the server-side node mirror, so they reflect the latest `/n_set`/`/n_setn`/`/s_new`. An unknown node or out-of-range control replies `/fail`.

### `/s_noid` and `/n_trace`

`/s_noid id...` exists for scsynth compatibility. In scsynth it releases integer node IDs back to the pool; Clausters assigns IDs per client (auto IDs come from a reserved server range) and never reuses a live or freed ID under a new node, so there is nothing to release — the command validates the IDs name live synths and replies `/done`. `/n_trace id...` is a debug aid: it logs each node's current control values (or a group's children) to the server console through the `clausters::osc` trace target, no OSC reply (matching scsynth's console trace).

### Pausing and resuming nodes (`/n_run`)

`/n_run id flag …` (pairs of `nodeID, flag`) pauses (`flag = 0`) or resumes (`flag = 1`) a node — a synth or a whole **group** (a paused group skips its entire subtree). A paused node stays in the tree and keeps its state; it is just skipped during processing, so it is silent and consumes no CPU, and resumes exactly where it left off. This is what resumes a synth parked by `doneAction 1` (`pauseSelf`) — pause is not terminal. An unknown id replies `/fail`.

### Addressing a group (scsynth group semantics)

`/n_set`, `/n_map` and `/n_mapa` accept a **group** id as well as a synth id. Addressed to a group, the command transfers each named control **down the group's subtree** to every synth (and Faust node) that has a control of that name, recursing through subgroups and stopping at each synth — the standard scsynth behaviour, so one message moves a parameter across a whole bank of nodes. A node without a matching control name is simply skipped; an empty group is a no-op; an unknown id replies `/fail`. Addressed to a single synth, the command sets only that synth, as before.

Because a control is one value per block, `/n_mapa` **samples** one frame of the audio bus per block (control rate) — this matches scsynth for a control-rate control; there are no audio-rate controls here (feed an audio signal through `In`/an input bus instead). Mapping a control that is used as a bus index makes the node a dynamic barrier for auto/parallel groups, and an audio map adds that bus to the node's reads so the dependency analysis stays correct.

### Moving nodes in the tree (`/n_before`, `/n_after`, `/g_head`, `/g_tail`, `/n_order`)

Execution order within a group is the child order, and these commands rewrite it. `/n_before id target` / `/n_after id target` move a node just before/after a sibling (any number of `id target` pairs). `/g_head group id` / `/g_tail group id` move a node to the **head/tail** of a group (pairs of `group id`). `/n_order addAction target id...` moves **several** nodes to one place at once, keeping their listed order: `addAction` selects `0` head of the target group, `1` tail, `2` before the target node, `3` after it.

All of these are disabled inside an **auto-sorted group** (`/g_sortMode … 1`): there the execution order is recomputed from the bus-connection DAG, so a manual move replies `/fail` — use auto-sort, or a manually-ordered group, but not both. (This is why `/n_order` earns its place only in manual groups: it is a batch `/n_before`/`/n_after`/`/g_head`/`/g_tail`.)

### Control buses (`/c_set`, `/c_setn`, `/c_fill`, `/c_get`, `/c_getn`, `/c_stream`)

Control buses are shared `f32` slots any client or synth reads and writes — the glue between `/c_set` and `/n_map`. `/c_set bus value...` writes single buses (pairs); `/c_setn bus numBuses value...` writes a **consecutive range** from a value list (repeatable groups); `/c_fill bus numBuses value` fills a range with one value (repeatable triples). `/c_get bus...` reads single buses, replying `/c_set (bus value)...`; `/c_getn bus numBuses...` reads ranges, replying `/c_setn (bus numBuses value...)...`. Unset buses read `0.0`. The immediate forms write the shared atomics on the network thread; inside a **timed bundle** the writes travel to the audio thread so they land on the exact scheduled sample.

Beyond scsynth, **`/c_stream periodMs bus...`** subscribes the sending client to a **periodic snapshot** of the listed buses: the server acks `/done /c_stream`, sends one `/c_set (bus value)...` immediately, and keeps sending one every `periodMs` (clamped to a 10 ms floor; at most 128 buses per subscription) without further requests. It is the network counterpart of reading the control buses from the shared-memory segment, for clients that cannot map it — a browser GUI's meters and scopes over WebSocket. One subscription per client, **replaced** by each new `/c_stream`; `periodMs <= 0` (or an empty bus list) cancels. A subscription dies with its TCP/WebSocket connection; over UDP or the shared-memory ring it lasts until an explicit cancel or `/quit` (the `/notify` posture). Not schedulable in a timed bundle.

### Audio taps (`/tap`, `/tap_stream`)

Audio taps are the **audio-rate** counterpart of the control buses' data plane: pre-allocated single-channel sample rings inside the shared-memory segment (`--taps` of them, `--tap-frames` samples each — defaults 8 × 16384; `--taps 0` disables the region), sized at boot and reported in `/server_info.reply`. They exist so a scope can see the *samples* of a live signal: SuperCollider answers this with `ScopeOut2` writing into scope buffers; here the ring is a first-class segment region and routing into it is a command, not a UGen.

**`/tap tapIndex bus`** routes audio bus `bus` into tap ring `tapIndex`: from the next block on, the audio thread appends that bus's samples to the ring (RT-safe — one `memcpy` and one atomic store per block), where a local peer (the GUI host's oscilloscope) reads the newest window straight out of shared memory, with zero per-frame messages. `bus = -1` stops the tap. No ack, like `/n_map` (it only flips routing state; failures reply `/fail`) — sequence with `/sync` when needed.

**`/tap_stream periodMs frames tapIndex...`** is the network counterpart, for clients that cannot map the segment — a browser oscilloscope over WebSocket, or headless capture. The server acks `/done /tap_stream`, then sends, immediately and every `periodMs` (10 ms floor), one **`/tap_data tapIndex endPosition blob`** per listed tap: the newest `frames` samples of its ring as raw little-endian `f32`, with `endPosition` (int64) the tap's stream position — total samples ever written — at the window's end, so consecutive snapshots can be placed on the tap's own sample axis (they overlap or gap by exactly the position delta). `frames` is clamped to the subscriber's transport bound — 8192 for a datagram-bounded client (UDP, the shared-memory ring), up to the `--max-frame` ceiling for a TCP/WebSocket client — and always to half the ring; at most 8 taps per subscription; a tap that has not yet filled a window sends nothing. Same subscription posture as `/c_stream`: one per client, replaced on each call, `periodMs <= 0` (or no taps) cancels, dies with its TCP/WebSocket connection, not schedulable in a timed bundle.

## Timed bundles

OSC bundles carry an NTP timetag. The immediate tag (`1`) executes on arrival; a **future** timetag is converted to a position on the server's sample clock and the whole bundle fires **sample-accurately**: the engine splits the audio block at the event's exact sample, so a `/s_new` scheduled mid-block starts on that very frame. Bundles with equal times run in arrival order; late bundles run immediately (and are logged). Nested bundles are scheduled independently by their own timetags.

Schedulable inside a timed bundle: `/s_new`, `/n_set`, `/n_setn`, `/n_fill`, `/n_map`, `/n_mapa`, `/n_mapn`, `/n_mapan`, `/n_free`, `/n_before`, `/n_after`, `/n_order`, `/g_head`, `/g_tail`, `/g_new`, `/g_freeAll`, `/g_deepFree`, `/c_set`, `/c_setn`, `/c_fill`, `/u_cmd`, `/g_sortMode`, `/g_parallel`, `/graph_new`, `/graph_voice`. Anything else (defs, buffers, queries, server commands) replies `/fail … cannot be scheduled in a timed bundle` — load defs and buffers first, then schedule the notes.

**`/clearSched`** flushes the whole timed-bundle queue: every bundle waiting on the sample clock is dropped (their heap freed off the audio thread), and the command replies `/done /clearSched`. Use it to abort a scheduled score — the scsynth panic button.

Also beyond scsynth: **auto-sorted groups**. `/g_sortMode groupID 1` makes a group keep its children in dependency order inferred from the buses each def reads and writes — no more manual `/n_before` bookkeeping; query what the server inferred with `/g_queryTree` (scsynth-compatible reply) and `/g_dumpGraph`. See [`auto-order.md`](auto-order.md) and `examples/auto_order.py`. The same analysis powers **parallel groups**: `/g_parallel groupID 1` (with the server started as `--workers N`) runs a group's independent children on several cores, bit-identically to the sequential result — see [`parallel.md`](parallel.md).

Beyond scsynth, the server also exposes its **sample clock** directly: `/clock` queries the sample counter, the actual sample rate and the server's OSC time captured with the counter (the master-clock anchor), and `/sched <int64 target> <blob>` schedules a packet at an **absolute sample** instead of an NTP time — same queue, same sample accuracy, drift-free by construction. See [`sample-clock.md`](sample-clock.md) and `examples/sample_clock.py`.

It also keeps a **shared transport** — a beat grid for phase-aligning several clients on that sample clock, plus a DAW-style rolling state. `/transport` with no args **queries** it (replies `/transport.reply <int64 originSample> <double tempo> <int32 defined> <int32 playing> <double position>`, `defined` 0 until one is set; the last two fields are appended, so older clients reading the first three still work); `/transport <int64 originSample> <double tempo>` **sets** the grid (last writer wins, stopped at position 0, replies `/done`). Beat `b` is sample `originSample + b·rate/tempo`. The **rolling state** rides on top: `/transport_play [<double position>]` starts playing (from `position`, or where it last stopped), `/transport_stop` stops, `/transport_locate <double position>` sets the song position; each replies `/done` and needs a grid defined first. Any change is **pushed** to every `/notify` client as a `/transport.reply`, so a responder re-aligns or rolls its playhead live. It is an in-memory registry the server stores, serves and broadcasts but **never schedules audio from** — clients read it to start their routines/playheads on the same beat. See [`sample-clock.md`](sample-clock.md).

UDP is not the only transport: local clients can speak the same OSC through **shared memory** (`clausters --shm <path>`) or run the whole server **in-process** through the embed C ABI, with the sample clock and the control buses readable and writable directly in mapped memory. See [`ipc.md`](ipc.md) and `clients/python/clausters/ipc.py`.

The server also speaks **TCP**, **on by default** on the same port `57110` (alongside UDP — separate namespaces; `--no-tcp` disables it, `--tcp [port]` moves it). **UDP is always on** — it is the base transport and cannot be turned off (there is no TCP-only mode); it is the boot/discovery protocol a client probes first, and it doubles as internal infrastructure, since the TCP loop wakes itself with a zero-length datagram to the server's own UDP socket. Each OSC packet — message or bundle — is **length-prefixed**: a 4-byte big-endian byte count followed by exactly that many OSC bytes, the same framing scsynth uses; replies come back framed the same way over the one connection. TCP gives a reliable, ordered, connection-oriented channel — no silent packet loss, and no datagram size limit: a frame may be as large as the **`--max-frame` ceiling** (default 16 MiB, advertised in `/server_info.reply`), which is a DoS guard on the untrusted length prefix, not a protocol limit. That makes TCP the natural **command plane** for large payloads — whole defs, big `/b_getn` chunks — while timing still rides on bundle timetags / `/sched`, so arrival latency does not affect *when* a scheduled command fires. The Python client speaks it through `OscTcpInterface`.

The server also speaks **WebSocket** when started with `clausters --ws [port]` (default port `57120`) — always available, like TCP and shared memory, not behind a build feature. This is the transport a **browser** can reach — a browser cannot open a raw UDP socket or map shared memory, but speaks WebSocket natively — so it is what lets a web-hosted client drive the server (and the server "run in the browser"). It carries the same OSC the same way the other transports do, multiplexed into the same loop and waking it with the same zero-length-UDP trick; the one framing difference from TCP is that each OSC packet is one WebSocket **binary** message, so the frame boundary *is* the packet boundary and there is no length prefix (replies come back as binary messages). Inbound bytes validate through the same single decode door as every transport, and messages are bounded by the same `--max-frame` ceiling TCP applies to its length prefix. The Python client speaks it through `OscWsInterface`; a browser uses the native `WebSocket` API (see `examples/ws_ping.py` and `examples/ws_ping.html`). `ws://` only — TLS (`wss://`) is out of scope, terminate it at a reverse proxy if needed.

Both stream transports share a few **edge guards**, sized like everything else for the target deployments (loopback and controlled networks, not a public service). Concurrent connections, TCP + WebSocket combined, are capped by **`--max-clients`** (default 64, in the spirit of scsynth's `maxLogins`) — a connection past the ceiling is dropped at accept, and closing any connection frees its slot. A client that *floods* commands faster than the server consumes them is held back by TCP flow control (the inbound queues are bounded, so server memory is too) — nothing is dropped, the sender just blocks. A client that *stops reading* its replies is disconnected rather than allowed to stall the server or grow its reply queue without bound. None of this affects a well-behaved client, dense control streams included; UDP is connectionless and keeps its own natural bounds (the datagram cap, kernel buffering).

## NRT mode (offline rendering)

The same engine renders scores to WAV without an audio device:

```sh
clausters --nrt score.osc out.wav [--rate 48000] [--channels 2] [--format float|int16|int24]
```

A score is the scsynth binary format: OSC packets back to back, each preceded by its byte count as a big-endian `int32`. Timetags count **seconds from the start of the render** (the immediate tag is time 0); bundles fire sample-accurately exactly like in real time, so an offline render equals a perfectly timed live take. The render ends at the time of the **last** bundle, whose commands produce no sound — close every score with a dummy bundle (a final `/n_free`) to set the duration.

Unlike the live server, a score bundle may also contain the asynchronous commands `/d_recv`, `/d_faust`, `/d_free` and the `/b_*` family: they complete synchronously before time advances (scsynth NRT semantics), and any failure aborts the render with the offending event's time and message. Queries (`/status`, `/b_query`, `/b_get`, `/b_getn`, `/c_get`) are errors in a score.

`python3 examples/json_client.py score` writes an example score; `cargo run --release --example bench` measures graph throughput offline.

## SynthDef JSON (`/d_recv`)

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

- `name` — the key used by `/s_new` and `/d_free`. Re-sending a name replaces the def (existing synths keep playing the old one).
- `controls` (optional) — named parameters addressable from `/s_new` and `/n_set`, with their default values and an optional **type** and **lag** (see *Control types* below).
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
| `"rate": "kr"` | *(default)* a plain control: one value per block, settable any time with `/n_set`, mappable to a bus with `/n_map`/`/n_mapa` |
| `"rate": "tr"` | a **trigger**: a `/n_set` value holds for exactly one block, then the engine resets it to `0` — so a rising edge fires once (drives an `EnvGen` gate, a sample-and-hold, …) |
| `"rate": "ir"` | a **scalar**: read once when the synth starts and then frozen; a later `/n_set` is ignored (it pairs with the `ir` UGen rate, so it may feed an `ir` input like `Rand`/`BufFrames.ir`) |
| `"lag": t` | on a `kr` control, smooth its changes with an implicit one-pole `Lag` over `t` seconds — the server inserts a real `Lag` UGen at compile time, so a stepped `/n_set` glides instead of jumping |
| `"lag_down": t` | with `"lag"`, use separate up (`lag`) and down (`lag_down`) times (an inserted `VarLag`) |

Example: `{"name": "freq", "default": 440.0, "lag": 0.1}` glides over 100 ms; `{"name": "gate", "default": 0.0, "rate": "tr"}` is a one-shot trigger. Audio-rate controls are not a control type — feed an audio signal through `In`/an input bus and `/n_mapa` instead.

### UGen catalog (built-in kinds)

The `kind` field is an **opaque string** as far as the protocol is concerned: the schema above is the whole contract (name + inputs + optional rate + static fields). What follows is a separate thing — the **catalog of UGens the server currently ships**, which the server resolves at runtime and grows independently of the wire format (each is one descriptor entry in `src/dsp/registry.rs`; a client never enumerates them, it just names one and the server validates). So this table is reference, not part of the protocol: an unknown name simply fails with `/fail … unknown kind`.

| kind | inputs | output |
|---|---|---|
| `Sine` | freq (Hz) | sine by f64 phase accumulation, starts at phase 0 |
| `Impulse` | freq (Hz) | single-sample `1.0` every `freq` Hz, `0.0` between; the first output sample is always an impulse, so a `/sched`'d `/s_new` places it on an exact frame; `freq` 0 emits one impulse then silence (f64 phase, drift-free) |
| `WhiteNoise` | — | uniform white noise in ±1 |
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
| `PlayBuf` | bufnum, chan, rate, loop | buffer player with linear interpolation; `rate` is frames per output sample (1.0 = the server rate — multiply by `BufRateScale(bufnum)`, i.e. `file_sr / server_sr`, for the file's pitch); starts at frame 0, silent at the end unless looping |
| `BufRd` | bufnum, chan, phase, loop | reads the buffer at a `phase` signal in frames (linear interpolation); out-of-range phases wrap when looping, clamp otherwise |
| `BufSampleRate` | bufnum | the buffer's own sample rate (Hz), block-constant |
| `BufRateScale` | bufnum | `file_sr / server_sr`; feed `PlayBuf`'s `rate` (`rate: BufRateScale(buf) * pitch`) to play at the file's true pitch without the client knowing either rate |
| `BufFrames` | bufnum | frame count, block-constant |
| `BufChannels` | bufnum | channel count, block-constant |
| `BufDur` | bufnum | duration in seconds (`frames / file_sr`), block-constant |
| `Osc` | bufnum, freq (Hz), phase (rad) | interpolating wavetable oscillator; `bufnum` must hold a **wavetable-format** buffer (see `/b_gen` below); `phase` is an offset in radians |
| `OscN` | bufnum, freq (Hz), phase (rad) | non-interpolating oscillator over a **plain** (non-wavetable) buffer; rawer/cheaper than `Osc` |
| `VOsc` | bufpos, freq (Hz), phase (rad) | like `Osc` but the buffer number is a signal: reads wavetables `bufpos` and `bufpos+1` and crossfades by its fractional part, so sweeping `bufpos` morphs a bank of adjacent tables (allocate them contiguously, same size) |
| `Shaper` | bufnum, in | waveshaper: maps `in` (in ±1, clamped) through a transfer table in wavetable format (typically `/b_gen cheby`); the table's first point is `in = −1`, its last `in = +1` |
| `DiskIn` | chan | streams a file from disk (mono per UGen — `chan` picks the channel); needs a `path` field; `loop` restarts at end of stream; see streaming note below |
| `DiskOut` | signal | streams `signal` to a mono WAV on disk; needs a `path` field; `format` is the WAV sample format; passes `signal` through as its output |
| `LocalIn` | channel | reads synth-private feedback channel `channel` (a constant); see feedback note below |
| `LocalOut` | channel, signal | writes `signal` into synth-private feedback channel `channel` (a constant); also passes `signal` through as its own output |
| `EnvGen` | gate, levelScale, levelBias, timeScale, doneAction, *envelope array* | breakpoint envelope; gate-driven, with a `doneAction` that can free the node — see the envelope note below |
| `Lag` | in, time | one-pole smoother: `in` lagged over `time` seconds (symmetric); `time` 0 passes through; primed to the first input (no glide up from 0) |
| `VarLag` | in, lagUp, lagDown | one-pole smoother with separate rise (`lagUp`) and fall (`lagDown`) times |
| `SampleRate` | — | the engine sample rate in Hz; init-rate (`ir`) by default |
| `Rand` | lo, hi | one uniform random value in `[lo, hi)`, drawn once at synth init and held; init-rate only (its inputs must be constants/`ir`) |
| `Demand` | trig, reset, source | demand driver: pulls the next value from its demand `source` on each rising edge of `trig`, holds it between triggers; a rising `reset` restarts the source; 0 before the first trigger — see the demand note below |
| `Dseq` | repeats, v0, v1, … | demand source (`dr` only): yields the value list in order, `repeats` times (`≤ 0` loops forever), then signals end-of-stream; only valid as a `Demand` source |
| `SendTrig` | in, id, value | on each trigger of `in`, sends `/tr nodeID id value` to `/notify` clients; output is silence — see the side-effect note below |
| `SendReply` | trig, replyID, value0, value1, … | on each trigger of `trig`, sends a custom OSC message `cmdName nodeID replyID value…` (the `cmdName` is a static `label` field, default `/reply`); output is silence |
| `Poll` | trig, in, trigid | on each trigger of `trig`, posts `label: value` (the `in` value) to the server console (a static `label` field) and, when `trigid ≥ 0`, also sends `/tr nodeID trigid value`; passes `in` through as its output |
| `FFT` | in, active | opens a spectral chain: windows `in` and transforms it to a spectral frame once per hop (`active > 0` runs, `≤ 0` holds); static fields `fft_size` (default 1024), `hop` (fraction, default 0.5), `wintype` (default 0 = Hann); the window is also settable live via `/u_cmd` — see the FFT-chain note below |
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
| `IFFT` | chain | closes a spectral chain: inverse-transforms each fresh frame and overlap-adds it back to audio; `fft_size`/`wintype` are inherited from the chain's `FFT` (given only on the `FFT`) |

**Envelopes (`EnvGen`).** `EnvGen` plays a breakpoint envelope, modelled on SuperCollider's. Its inputs are five fixed signals followed by a flat **envelope array**: `gate, levelScale, levelBias, timeScale, doneAction`, then `initLevel, numSegments, releaseNode, loopNode`, then four values **per segment** — `target, duration, shape, curve`. The output is `envelope · levelScale + levelBias`; `timeScale` stretches every segment's duration. A rising `gate` (re)triggers from `initLevel`; while the gate is held the envelope **sustains** at `releaseNode` (an index into the levels — hold that level until release); when the gate falls it plays the segments from `releaseNode` on. `releaseNode < 0` disables the sustain, so the envelope plays straight through (a one-shot). With a `loopNode` (an index `< releaseNode`), the held phase **cycles** the segments in `[loopNode, releaseNode)` instead of holding a single level, carrying the level at the release node back as the loop's start; the release still plays out from `releaseNode` when the gate falls. `loopNode < 0` disables looping. The **shape** numbers are `0` step, `1` linear, `2` exponential (needs same-sign, non-zero levels), `3` sine, `4` welch, `5` custom-curvature (bent by the `curve` value: 0 linear, positive starts slow, negative starts fast), `6` squared, `7` cubed, `8` hold. When the last segment finishes the UGen applies its `doneAction` — scsynth's full set (0–15), with the freeing done on the audio thread through the garbage FIFO, never a blocking free:

| # | action | # | action |
|---|---|---|---|
| 0 | do nothing | 8 | free this synth and every following node in its group |
| 1 | **pause** this synth (kept in the tree; resume with `/n_run`) | 9 | free this synth and pause the preceding node |
| 2 | **free** this synth | 10 | free this synth and pause the following node |
| 3 | free this synth and the preceding node | 11 | free this synth; if the preceding node is a group, deep-free it, else free it |
| 4 | free this synth and the following node | 12 | free this synth; if the following node is a group, deep-free it |
| 5 | free this synth; if the preceding node is a group, free all its children, else free it | 13 | free this synth and every other node in its group |
| 6 | free this synth; if the following node is a group, free all its children | 14 | free the enclosing **group** (this synth included; if it is the un-freeable root, frees just the synth) |
| 7 | free this synth and every preceding node in its group | 15 | free this synth and **resume** the following node |

The relative actions (3–13, 15) resolve the node's previous/next sibling and head/tail-of-group from the tree's execution order. Any input can be a signal, so a control can drive the gate or scale the levels/times live. The Python client builds the array with the `Env` breakpoint helper (`Env.adsr`, `Env.perc`, `Env.asr` …, plus `release_node`/`loop_node`) and the `env_gen` callable.

**Operator UGens (`BinaryOpUGen`/`UnaryOpUGen`).** Rather than a distinct kind per math operation, one generic UGen per arity carries the operator as a static **`op`** field (alongside `kind`/`inputs`) — the operator's **name**. So `{"kind": "BinaryOpUGen", "op": "mul", "inputs": [a, b]}` multiplies. Every operator is one entry in `clausters_core::builtins` — the **same code** the client's value FFI runs off the audio thread — so a value a client computes ahead of time and the UGen on the audio thread are **bit-identical** for the native ops. A missing or unknown `op` fails the def with `/fail` naming the node. The `Add`/`Sub`/`Mul`/`Div` kinds remain as aliases for the `add`/`sub`/`mul`/`div` operators. (Internally each operator also has a stable integer id for the C ABI, but that is an implementation detail — defs and clients only ever use the name.)

Binary operators (`BinaryOpUGen` `op`): `add` (`a+b`), `sub` (`a−b`), `mul` (`a·b`), `div` (`a/b`), `mod` (`a%b`), `pow` (`aᵇ`), `min`, `max`, `atan2`, `gt` (`a>b`), `lt` (`a<b`), `ge` (`a>=b`), `le` (`a<=b`), `eq`, `ne`, `bitand`, `bitor`, `bitxor`, `lshift`, `rshift`, `hypot` (`√(a²+b²)`), `ring1` (`ab+a`), `ring2` (`ab+a+b`), `ring3` (`a²b`), `ring4` (`a²b−ab²`), `sumsqr` (`a²+b²`), `difsqr` (`a²−b²`), `sqrsum` (`(a+b)²`), `sqrdif` (`(a−b)²`), `absdif` (`|a−b|`), `thresh` (`a<b?0:a`), `clip2` (clip to ±b), `excess` (`a−clip2`), `round` (to step b), `trunc` (to step b). Comparisons and bitwise ops follow Faust's convention (a boolean is `0.0`/`1.0`; bitwise acts on the `i32` casts).

Unary operators (`UnaryOpUGen` `op`): `neg`, `abs`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `exp`, `exp10`, `log`, `log10`, `log2`, `sqrt`, `floor`, `ceil`, `rint` (ties to even), `as_int`, `as_float`, `squared` (`x²`), `cubed` (`x³`), `recip` (`1/x`), `frac`, `sign`, `sinh`, `cosh`, `tanh`, `midicps`, `cpsmidi`, `midiratio`, `ratiomidi`, `dbamp`, `ampdb`, `octcps`, `cpsoct`, `distort` (`x/(1+|x|)`), `softclip`.

The tables are open — a new operator is one more `clausters_core::builtins` entry, no renumbering. The Python client maps its operators and math methods (`%`, `min`, `>`, `.midicps()`, `.distort()`, …) to these names automatically, and exposes `madd`/`sum3`/`sum4` for the fused kinds.

**Calculation rates (`ir`/`kr`/`ar`/`dr`).** Every ugen output has a rate, chosen with the optional `"rate"` field or defaulted per kind. `ar` (audio rate) is one value per sample — the normal signal wire, and the default. `kr` (control rate) is one value per block, recomputed each block; downstream it reads as a constant over the block. `ir` (initial rate) is computed **once, when the synth starts**, then held for the node's life — a fixed `SampleRate`, a `Rand` seed, a `BufFrames.ir` snapshot; its inputs must themselves be constant/`ir` (nothing that varies can be frozen). `dr` (demand rate) is pulled, not run — see the demand note. Coercion is one-way: a slower rate feeds a faster input for free (a constant broadcast over a block), but an `ar` signal cannot feed an `ir` input, and a `dr` output may only feed a `Demand` source. Audio-rate controls are not a thing here — feed an audio signal through `In`/an input bus (as noted under `/n_mapa`).

**Demand-rate sequences (`Demand`/`Dseq`).** A demand ugen produces values only when *pulled*. `Dseq` is a source — a stream of numbers (`repeats` passes over a value list, then exhausted); it never runs in the normal per-block order. `Demand` is the driver that pulls it: on each rising edge of its `trig` it demands the next value and holds it until the next trigger (a rising `reset` restarts the stream; the output is 0 before the first trigger, and holds the last value once the source is exhausted). Wire a trigger source (an `Impulse`, a `/n_set` trigger later) into `Demand`'s `trig`, and a `Dseq` into its `source`, to step through a sequence sample-accurately — the foundation the rest of the demand family (`Dseries`/`Dwhite`/`Duty`, later) builds on. This is a **minimal** driver today: end-of-stream is a held value, and one demand source per driver.

**Side-effect UGens (`SendTrig`/`SendReply`/`Poll`).** Some UGens exist for a **side effect** — an OSC reply or a console post — not for audio on a bus, and a def may contain *only* these with **no `Out` at all** (the server requires at least one UGen, never an `Out`). All three fire on a **trigger**: a signal crossing from `≤ 0` up to `> 0`. `SendTrig` replies `/tr nodeID id value`; `SendReply` replies at a custom address (its static `label` field, default `/reply`) carrying `nodeID replyID value…` for an arbitrary value list; `Poll` posts `label: value` to the server console and, with a non-negative `trigid`, also emits a `/tr`. The replies reach every client registered with `/notify` (`SendReply`'s custom address is delivered verbatim). Mechanically the reply leaves the audio thread through a lock-free FIFO — the same discipline as the `/n_go`/`/n_end` node events — so triggering never allocates or blocks; a burst of triggers beyond the per-block buffer is dropped (best-effort, like the node events). The `id`/`replyID`/`trigid` and `value`s can be any signal (sampled at the trigger). The Python client builds them with `send_trig`/`send_reply`/`poll` and passes them as `SynthDef` roots.

**Frequency-domain chain (`FFT`/`PV_*`/`IFFT`).** Spectral processing bookends a
chain of `PV_*` (phase-vocoder) UGens between an `FFT` and an `IFFT`: `FFT`
windows an audio input and transforms it to a complex frame once per **hop**;
each `PV_*` mutates that frame (only on the blocks a fresh one is ready); `IFFT`
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
frame. It acts on the blocks chain A has a fresh frame, reading chain B's
latest frame. The operator is a property of the *name*; all six are one
server-side implementation.

Analysis and resynthesis use the same window (Hann by default), and the
overlap-add is **window-normalized** (divided by the steady-state window-overlap
denominator, COLA), so a plain `FFT`→`IFFT` reconstructs the signal at unity
gain, delayed by the transform latency (one window). The window type is settable live per instance
with `/u_cmd <nodeID> <ugenIndex> window <wintype>` (`-1` rectangular, `0` Hann,
`1` sine, `2` Welch, `3` Hamming, `4` Blackman) — the first consumer of the typed
per-UGen command surface.

**Where the frame lives (deviation from scsynth).** scsynth threads the frame
through a client-allocated buffer whose bins the audio thread mutates in place,
which would break Clausters' rule that a pool buffer is immutable once built. So
the frame lives in **synth-private scratch** allocated when the synth is
instantiated (like the `LocalIn`/`LocalOut` feedback buffer, and the moral
equivalent of SuperCollider's `LocalBuf`), freed with the synth — **no `/b_alloc`
is required** and the sample-buffer pool stays fully immutable. A future
extension may add copying the frame into a buffer for inspection/sharing.

**Feedback (`LocalIn`/`LocalOut`).** The graph is a DAG — UGens cannot be wired in a cycle. To feed a signal back, write it with `LocalOut` and read it with `LocalIn`: they share a per-synth buffer that persists across blocks, so the value read is what was written **one control block (64 samples) earlier**. `LocalIn` for a channel must appear *before* its `LocalOut` (the compiler enforces this; it is what makes the delay exactly one block), and the channel index must be a constant. Use any number of channels (mono each, like buses). This is **block-rate** feedback — good for feedback delays, block feedback-FM, resonant combs (a one-channel loop resonates at `sampleRate / 64`). Sample-accurate (sub-block) feedback is not possible across composed UGens; fuse the loop into one node — a recursive UGen or a Faust def (`/d_faust` with `~`).

**Streaming disk I/O (`DiskIn`/`DiskOut`).** These stream to/from disk in real time, so arbitrarily long files never touch the buffer pool (`PlayBuf`/`BufRd` load the whole file first). Each is **self-contained**: one background I/O thread plus a lock-free ring shared with the audio thread, opened at `/s_new` and closed when the synth is freed. The audio thread only pushes/pops the ring — never blocks. A ring underrun (disk too slow) plays silence; a `DiskOut` overrun drops samples; both are rare with the ~1 s ring. They carry extra **static fields** in the UGen spec (alongside `kind`/`inputs`): `path` (required), `loop` (`DiskIn`, default false), and `format` (`DiskOut`, `int16`|`int24`|`float`, default `int16`). Both are **mono per UGen** like the buffer readers: `DiskIn` extracts one channel (`chan`) — a stereo file is two `DiskIn`s; `DiskOut` writes a mono WAV — record stereo with two `DiskOut`s. `DiskIn` streams one file frame per server sample with **no resampling** (pitch follows the sample-rate ratio, as in scsynth's `DiskIn`); these UGens spawn a thread each, so they are for a handful of streams, not per-voice. Example DiskOut UGen: `{"kind": "DiskOut", "inputs": [{"ugen": 0}], "path": "/tmp/rec.wav", "format": "float"}`.

Output happens exclusively through `Out`/`ReplaceOut`; a def without them is silent. Several synths with `Out` on the same bus mix. Bus-index inputs are ordinary signals, sampled at the first frame of each block and clamped to the valid range.

Buffer readers are **mono** (one output per UGen, unlike scsynth's multi-output PlayBuf): the `chan` input picks the channel, and two readers with the same inputs stay sample-locked, so a stereo file is two UGens. Neither has a trigger or done action yet.

## Buffers (`/b_*`)

```text
/b_alloc     bufnum frames [channels=1]                  # zeroed buffer
/b_allocRead bufnum path [fileStart=0] [numFrames=0=all] # shape from the file
/b_read      bufnum path [fileStart=0] [numFrames=-1=all] [bufStart=0]
/b_write     bufnum path [header="wav"] [format="int16"|"int24"|"float"] [numFrames=-1] [startFrame=0]
/b_zero      bufnum
/b_gen       bufnum cmd flags args...  # fill/generate — see the wavetable section
/b_free      bufnum
/b_close     bufnum                    →  /done /b_close bufnum   # see note
/b_query     bufnum...                 →  /b_info  bufnum frames channels sampleRate ...
/b_get       bufnum index...           →  /b_set   bufnum index value ...
/b_getn      bufnum [start count]...   →  /b_setn  bufnum start count value ...
/b_export    bufnum path               →  /done /b_export bufnum
```

`/b_alloc`, `/b_allocRead`, `/b_read`, `/b_write`, `/b_zero`, `/b_gen` and `/b_free` are **asynchronous**: the work happens on a dedicated NRT thread (one queue, so commands on the same buffer complete in submission order) and the reply is `/done <cmd> bufnum` or `/fail <cmd> reason`. Buffers keep the file's sample rate (the server never resamples — see `PlayBuf`'s rate above); integer WAVs are scaled to ±1. `/b_read` requires an allocated buffer and keeps its shape; channel-count mismatches fail. Reading decodes by **content**, not extension: WAV goes through hound (exact, int24-aware), and FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and CAF decode through [symphonia](https://github.com/pdeljanov/Symphonia) (whole-file decode, then slice — compressed formats have no cheap exact frame seek). `/b_write` still emits WAV only, and `leaveOpen` (streaming) is not supported. `/b_close bufnum` closes the soundfile a streaming buffer left open — scsynth pairs it with `DiskIn`/`DiskOut`; since Clausters has no streaming buffers yet (every `/b_read`/`/b_write` reads or writes the whole file and closes it), it validates the buffer is live and replies `/done /b_close bufnum`, forward-compatible with the streaming UGens.

### Table generation and the wavetable format (`/b_gen`)

`/b_gen bufnum cmd flags args…` fills an **already-allocated** buffer with a computed signal — additive spectra, a waveshaping curve, or a copy from another buffer. Like `/b_read` it reads the target's shape from the current contents, so a `/b_gen` right after a `/b_alloc` must be separated by a `/sync` (the alloc has to complete first). It runs on the same NRT queue and replies `/done /b_gen bufnum`.

The `flags` int packs three bits, `normalize`(1) + `wavetable`(2) + `clear`(4) — the usual value is `7` (all three). `normalize` scales the result to a peak magnitude of 1; `clear` starts from silence (without it the new signal is **added** on top of the buffer's current contents); `wavetable` stores the result in the interleaved wavetable format below (an `N`-sample buffer then holds `N/2` period points). `copy` takes no flags.

| cmd | args | fills with |
|---|---|---|
| `sine1` | flags, amp… | additive sine partials; `amp[k]` is the amplitude of harmonic `k+1` |
| `sine2` | flags, (freq amp)… | partials at arbitrary (possibly fractional) harmonic numbers |
| `sine3` | flags, (freq amp phase)… | as `sine2` with a per-partial phase in radians |
| `cheby` | flags, amp… | a waveshaping transfer function `Σ amp[k]·T_{k+1}(x)` of Chebyshev polynomials over `x∈[−1,1]` (`amp[0]` weights `T₁`, the linear/passthrough term); read by `Shaper` |
| `copy` | dstStart srcBufnum srcStart numSamples | overlays `numSamples` of another buffer onto this one (`numSamples < 0` = to the end of the shorter side) |
| `env` | level0, (level time shape curve)… | discretizes a break-point envelope across the whole buffer (see below); no flags |

`env` fills the buffer with a **break-point curve** — the buffer-world form of an automation curve. The arguments are the same decomposition an `EnvGen` carries: an initial `level0`, then one `(level, time, shape, curve)` quad per segment, where `shape` is the envelope-shape number (0 step, 1 linear, 2 exponential, 3 sine, 4 welch, 5 custom-`curve`, 6 squared, 7 cubed, 8 hold) and `curve` is read only by the custom shape. Each output sample evaluates the segment it falls in through the **same shared math** (`clausters-core`'s `envshape`) the `EnvGen` UGen plays, so a curve drawn or edited on a client (the `bpf` editor) and this buffer read identically. Segment `time`s are **relative** — only their proportions matter, since the buffer holds the curve *shape*; playback rate maps it onto real time (e.g. a `PlayBuf` whose rate spans the buffer over the desired duration). The mono curve is written to every channel. A client reads it back onto a control bus to drive `/n_map`-ed controls.

**The wavetable format.** An interpolating oscillator (`Osc`/`VOsc`) reads a period stored not as raw samples but as scsynth's interleaved offset/slope pairs: for each point `i` the buffer holds `[2·a[i] − a[i+1], a[i+1] − a[i]]`. With the fractional phase `frac∈[0,1)`, a sample is then one fused multiply-add — `x0 + (1+frac)·x1 = a[i] + frac·(a[i+1] − a[i])` — with no branch. `sine1/2/3` build periodic (wrapping) tables; `cheby` builds a non-wrapping one (it holds its endpoint, since a transfer curve is not periodic). A `wavetable`-format buffer is meant for `Osc`/`VOsc`/`Shaper`, not `PlayBuf`; a plain (non-`wavetable`) `/b_gen` buffer is a normal signal (read it with `OscN` or `BufRd`). This is the buffer-world counterpart of a Faust def's small embedded `waveform` table (see *Tables and waveforms* under Faust defs): the same idea — precompute a period or a transfer curve numerically — for the UGen graph instead of a JIT def.

`/b_query`, `/b_get` and `/b_getn` are **synchronous** reads, answered from the network-side buffer mirror (state as of the last completed command). `/b_get` reads single samples by flat (interleaved) index; `/b_getn` reads ranges, with `count` clamped to what the buffer holds from `start` — a request past the end returns only the available samples, and an unallocated buffer returns count 0. Sample indices are flat across channels (`frame * channels + channel`), so a stereo buffer reads as interleaved `L R L R ...`. Large buffers are read in client-chosen chunks sized to the client's transport: over TCP/WebSocket a chunk may be as large as the `/server_info` frame ceiling (megabytes per round-trip), over UDP each reply must fit one datagram.

`/b_export bufnum path` writes the buffer's raw samples — flat, interleaved, little-endian `f32` — to a local file and replies `/done /b_export bufnum` (or `/fail` on a missing buffer or write error). It is the **bulk-data path** for a same-machine consumer: where `/b_getn` chunks a buffer over the network, `/b_export` puts a multi-megabyte buffer in a file the consumer memory-maps and reads zero-copy, with no per-sample OSC. It is synchronous on the network thread (not the audio thread), reading the same mirror as `/b_query`. The reader pairs the file with the buffer's channel count (from `/b_query`) to de-interleave. (This is what the GUI host's mapped-`path` waveform reads.)

## Faust defs (`/d_faust`)

`/d_faust name payload` — the payload is **Faust source** unless its first non-whitespace byte is `{`, in which case it is JSON: a **box tree** (root `{"op": …}`, below) or a **signal tree** (root `{"signals": […]}`, see [JSON signal tree](#json-signal-tree-the-signal-api)). All three are JIT-compiled (LLVM) on a dedicated compiler thread; expect the `/done`/`/fail` reply a few milliseconds later.

### Controls

A Faust synth exposes, in this order:

1. every UI element of the def (`hslider`, `vslider`, `nentry`, `button`, `checkbox`) addressed by its **label**;
2. two reserved names: `out` — first output bus (default 0, hardware left) — and `in` — first input bus, for defs that process signal. A def with N outputs writes (sums) to buses `out .. out+N-1`; same for inputs.

```text
/s_new fsine 2000 1 0 freq 330 out 1
/n_set 2000 freq 660
```

If a def declares its own `out`/`in` control, the def's wins.

### Faust source payloads

Any complete Faust program; `import("stdfaust.lib")` and friends resolve against the stdlib installed with libfaust (`<prefix>/share/faust`). Compilation is single precision (`FAUSTFLOAT` = `f32`) with `-ftz 2` (recursive state below the normal float range flushes to zero — decaying tails cannot stall the audio thread in subnormal math), and the sample rate is fixed per instance at `/s_new` time.

```text
/d_faust fsine 'import("stdfaust.lib"); freq = hslider("freq", 440, 20, 20000, 0.01); process = os.osc(freq) * 0.2;'
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

This `waveform` box is the Faust-side counterpart of `/b_gen`'s wavetables (see *Table generation and the wavetable format* under Buffers): `waveform` inlines a small table **inside one def** for a self-contained JIT program, whereas `/b_gen` fills a **server buffer** shared by the whole UGen graph and read by `Osc`/`Shaper`. Same idea — precompute a period or a transfer curve numerically — at two different scales.

**Soundfiles read server buffers.** Faust's `soundfile("<bufnum>", n)` primitive binds to the server buffer whose index is its label (a plain integer string), e.g. `soundfile("0", 1)` reads buffer 0. At `/s_new` the instance's soundfile is filled from that buffer's current contents (deinterleaved to Faust's planar layout); a non-numeric label or an empty/missing slot yields a silent placeholder, so a def always instantiates. The primitive's outputs are `[length, sampleRate, channel0 … channel_{n-1}]` and the read index saturates at the part length, exactly as in stock Faust. The bind is a **snapshot** taken at instantiation — re-`/s_new` to pick up a buffer that changed; loading is mono-or-more by the buffer's own channel count (Faust reads up to `n`). For *streaming* a bus instead of a static buffer, the older path still works too: route a `PlayBuf`/`BufRd` through an audio bus and read it via the def's reserved `in` control, so both def families stay composable on the same buses.

### Errors

Structural problems (unknown ops, missing fields, wrong arities in `"in"`) fail during interpretation and the `/fail` message carries the path of the offending JSON node from the root `$`, e.g. `at $.in[1].op: unknown op "mul3"`. Semantic errors — composition arity mismatches, dangling inputs — are reported by the Faust compiler verbatim, prefixed with the path of the fragment for `faust` ops.

## JSON signal tree (the Signal API)

A third `/d_faust` format maps Faust's lower-level **Signal API** (`Csig*`) instead of the box algebra. It is selected by the **shape of the JSON**: a root object keyed by `"signals"` (`{"signals": [ … ]}`) is a signal tree, anything else starting with `{` is a box tree, and a non-`{` payload is raw source. The `"signals"` array lists one node per DSP **output** (this is how a signal def declares more than one output).

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

## GraphDef (`/d_graph`, `/graph_new`) — node-graph programs

A **GraphDef** is a third kind of persistent def. Where a SynthDef/FaustDef stores one synthesis node, a GraphDef stores a whole **configuration of member nodes wired by buses** — an effect chain, a mixer, a layered instrument — instantiated as one unit. It exposes a **named parameter surface**: ports that map to inner member controls (with optional scaling), so the running instance is driven through the port names, never the private member node ids. A GraphDef instantiates entirely into primitives the server already has (a group, member `/s_new`s, `/n_map` wiring), so nothing new touches the audio thread.

`/d_graph <blob|string>` loads a GraphDef from a JSON spec: it validates the structure (cheap — no JIT) and stores it, replying `/done`/`/fail` like the other def commands and persisting it when a data directory is configured. `/d_free name...` removes it (and SynthDefs/FaustDefs of the same name).

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
- `maps` (per member, optional) — binds a member control to an internal **control** bus via `/n_map`.
- `surface` — the named ports. Each maps to a list of `{member, control}` targets, with optional `mul`/`add` linear scaling of the incoming value. One port may drive several inner controls, each scaled differently (e.g. a `freq` port playing a detuned pair). This is the difference from a bare group `/n_set`, which can only broadcast one value to controls that happen to share a name.
- `defaults` — surface-port values applied at instantiation, overridable per instance.

`/graph_new name id addAction targetID [port value]...` instantiates a GraphDef: it creates an **auto-sorted group** (the member execution order follows the bus connections) at `id` (or `-1` for a server-assigned id), holding the **shared** members, then applies the shared `defaults` and the given `port value` overrides. `/n_set id port value...` on that group id resolves the port names against the surface (never the member ids); a port absent from the surface is ignored. `/n_free id` (or `/g_deepFree`) tears the instance down (with all its voices) and reclaims its private buses. Instantiation is atomic: a missing member def or bus shortfall fails with no partial instance. GraphDefs work in NRT scores too (scored like any def at time 0).

### Shared vs per-voice, and `/graph_voice`

A GraphDef splits into a **shared** part (members without `voice`) and a **per-voice** part (members with `"voice": true`) — the model of a polyphonic instrument: the shared part (the private bus, a mixer, effects) exists once; each note adds a voice. A surface port maps either to shared members or to voice members, never a mix (a `/fail` at `/d_graph` otherwise): shared ports apply to the instance, voice ports to each voice.

`/graph_new` instantiates only the shared members. `/graph_voice instanceID id [port value]...` then spawns the per-voice members as a **sub-group at the head of the instance** (the auto-sort orders it before the shared mixer that reads its bus), wired to the same private buses, applying the voice-port `defaults` and overrides. `/n_set voiceID port value...` resolves against the voice's surface; `/n_free voiceID` frees just that voice. A `/graph_voice` on an instance whose def has no voice members (or on an unknown instance) `/fail`s.

A **GraphDef can be bound to MIDI** exactly like a SynthDef/FaustDef: `/midi_bind channel graphname [target addAction gate]` spawns the shared instance once at bind time and every note becomes a `/graph_voice` into it (note → `freq` port, velocity → `amp` port, note-off frees the voice or, for a gate-aware binding, sets its `gate` port to 0). The GraphDef must have per-voice members. `/midi_unbind` frees the shared instance (and all its voices).

## MIDI control protocol (standard channel-voice actuation)

Besides OSC, the server can be driven by **standard channel-voice MIDI** — note on/off, velocity, aftertouch, pitch-bend, control change, program change. This is the **primary** MIDI path: a note actuates a synthesis node and an expressive message sets a named control, exactly the surface a sequencer or DAW already speaks. (SysEx, when it lands, is reserved for the non-musical control plane — def load, buffers, topology — and is never a tunnel for OSC commands.)

A channel must first be **bound** to an instrument. Binding and mapping are OSC commands (so they ride the same reliable path as the rest of the protocol); the note/control events themselves arrive over the OS's standard MIDI. Start the server with `--midi [name]` (RT only) to open a **virtual ALSA input port** (default name `clausters`) — the same system MIDI any controller or DAW uses; route anything into it (`aconnect`, a keyboard through the kernel, a DAW). Live input is MIDI 1.0 (7-bit, widened internally to the high-resolution form); the full MIDI 2.0/UMP resolution is preserved on the client's persistence path (the `clausters-midi` crate's MIDI 2.0 clip file). Network MIDI is a separate, deliberately out-of-scope idea.

- `/midi_bind channel instrument [target] [addAction] [gate]`: bind a MIDI channel (`0`–`255`: the classic 16 plus the extended UMP group×channel space) to an **instrument def** — a SynthDef, a FaustDef, **or a GraphDef**, actuated identically. Default target is the root group (`0`), default add-action `0` (head). `gate` non-zero marks the def gate-aware (see note off below). The default control map is `freq`/`amp`, matching the client `Event` convention. A **GraphDef** instrument spawns its shared instance at bind time and turns each note into a `/graph_voice` (see the GraphDef section); it must have per-voice members.
- `/midi_unbind channel`: remove the binding and free every voice still sounding on that channel.
- `/midi_map channel selector name`: route a message type to a control. Selectors: `note` (→ frequency control, default `freq`), `vel` (→ amplitude, default `amp`), `gate` (gate control name), `bend` (pitch-bend → control), `pressure` (channel aftertouch → control), `poly` (per-note aftertouch → the note's voice), `ccN` (control change number `N` → control), `progN` (program `N` → an instrument def `name` to switch to).

Actuation semantics, per message type (each with its named conversion):

- **Note on** → `/s_new instrument <voiceID> addAction target freq <midi2freq(note)> amp <velocity2amp(vel)>`. Voice IDs come from the reserved MIDI range of the node-id partition (see "Node ids are a finite, boot-partitioned resource" above), disjoint from client IDs and the `/s_new -1` auto range, and recycled as the voices die. A note-on for an already-sounding `(channel, note)` frees the old voice first. Velocity `0` is a note-off.
- **Note off** → `/n_free <voiceID>`, or `/n_set <voiceID> <gateControl> 0` when the binding is gate-aware.
- **Poly aftertouch** → `/n_set` on that note's voice; **channel aftertouch**, **control change**, **pitch-bend** → `/n_set` on every live voice of the channel, for the mapped control. Unmapped expressive messages and unbound channels are silently ignored (a running MIDI stream never errors).
- **Program change** → re-selects the channel's instrument def from its `prog`-mapped table (no-op if unset).

Conversions (`note → midi2freq`, `velocity → velocity2amp`, `aftertouch`, `bend`, `cc`, `program`) take **MIDI 2.0 / UMP resolution** (16-bit velocity, 32-bit controllers/pressure/bend) and produce the `f32` a control zone wants — no 7-bit quantization. **MIDI 1.0 is backward-compatible**: classic 7/14-bit input is accepted and widened up to those, so the same controls are driven either way. Because a MIDI voice is realized as the *same* `/s_new`/`/n_set`/`/n_free` an OSC client would send, it is byte-identical to the OSC equivalent (`tests/midi.rs` guards this).

## Node tree introspection

The node tree is delivered to clients as **structured replies** — never scraped from the server's logs. Three queries (all answered by the network-side tree mirror, so they never touch the audio thread):

- **`/g_queryTree <groupID> [flag]`** → `/g_queryTree.reply` — the whole subtree from `groupID` (use `0` for the root), scsynth-compatible. Args: `flag`, the group and its child count, then depth-first per node: ID and child count (`-1` marks a synth), the def name for synths, and — when `flag` is 1 — the control count followed by (name|index, value) pairs.
- **`/n_query <nodeID>...`** → one `/n_info` per node — per-node detail beyond the tree shape. Layout: `nodeID, parentID, prevID, nextID, isGroup`; then for a **group** `headID, tailID` (`-1` if empty); for a **synth** `defName`, control count + (name|index, value) pairs, map count + (controlIndex, bus, audio) triples (the `/n_map`/`/n_mapa` bindings), and the inferred `reads`/`writes` bus lists as two strings (`"0,16"`, or `"-"` when none). Siblings are `-1` when absent.
- **`/g_dumpGraph <groupID>`** → `/g_dumpGraph.reply [groupID, text]` — a human-readable rendering of the inferred bus graph (what each child reads/writes and the current order). A debugging aid; for machine use prefer `/g_queryTree`/`/n_query`.

The Python client wraps these as `Server.query_tree()` (nested dict), `Server.node_query()` (per-node dict) and `Server.dump_graph()` (string).

## Server logging and verbosity

The server logs to **stderr** through `tracing`, at five levels (`error`, `warn`, `info`, `debug`, `trace`). The startup banner and the NRT render summary go to **stdout** (they are program output, not logs). The **audio thread never logs**: it reports conditions over the lock-free FIFOs and the network thread emits them, so logging never breaks real-time safety.

The level is set, in increasing precedence, by:

- the CLI flags `-v` (info), `-vv` (debug), `-vvv` (trace), `-q` (errors only); default is `warn`;
- the `RUST_LOG` environment variable, an [`EnvFilter`](https://docs.rs/tracing-subscriber) directive that also filters per module — e.g. `RUST_LOG=clausters::osc=trace` to see only OSC traffic;
- at runtime, **from a client**, with two OSC commands (both reply `/done`):
  - `/verbosity <int|string>` — an int level (`-1` errors … `3` trace) or an `EnvFilter` directive string. Lets a client retune the server's logs without restarting.
  - `/dumpOSC <flag>` — toggles the OSC-traffic dump (the `clausters::osc` trace target). Unlike scsynth, this is **not** an ad-hoc console print: it routes through the same logging system, controllable by `/verbosity`/`RUST_LOG`, on stderr.
  - `/error <mode>` — `1` posts command failures to the server console (default), `0` silences them. The `/fail` **OSC reply is always sent** regardless (clients rely on it); `/error` only gates the server-side console logging. scsynth's bundle-local `-1`/`-2` forms are not separately supported — the persistent `0`/`1` toggle is the model that fits our logging (a deliberate deviation).

Note that these control the **server's own** logs (on the server's stderr); the **node tree** is delivered to clients as structured data, never scraped from logs — see [Node tree introspection](#node-tree-introspection) above (`/g_queryTree`, `/n_query`, `/g_dumpGraph`).

## Server and UGen commands (`/cmd`, `/u_cmd`)

Two extension commands carry out-of-band instructions that are neither node nor bus state. Both are **typed and discoverable** — the deliberate replacement for scsynth's untyped `/cmd`/`/u_cmd` argument blobs (a command *name* plus validated typed args, errors naming the offending field, like `compile`).

- `/cmd <name> args...` — a **server-wide** command. `name` selects a handler; the built-in `ping` replies `/done /cmd ping` (it proves the surface). An unknown name replies `/fail /cmd "unknown server command …"`. New server commands register here.
- `/u_cmd <nodeID> <ugenIndex> <name> args...` — a command addressed to **one UGen instance** inside a synth. It validates the node is a UGen synth and `ugenIndex` is in range (a Faust synth is one opaque block, not a UGen graph, so it is rejected), packs the numeric args inline (up to 8, so nothing heap-allocated crosses to the audio thread), and routes them to that UGen on the audio thread; the command name is hashed to a stable selector both sides agree on. The first real consumer is the FFT chain: `/u_cmd <nodeID> <ugenIndex> window <wintype>` swaps an `FFT`/`IFFT`'s analysis/synthesis window live (see the FFT-chain note above). A UGen that does not recognize the command name ignores it, so a `/u_cmd` to a valid target is otherwise accepted silently.

## Persisting defs across restarts

The real-time server can persist loaded defs to a data directory and reload them automatically on the next start, so a client need not re-send its instrument library every session. It is **on by default**; control it with two flags:

```text
clausters --data-dir <dir>   # where defs are stored/reloaded
clausters --no-persist       # disable for this run
```

With no `--data-dir`, the directory is `$CLAUSTERS_DATA_DIR` if set, else `$XDG_DATA_HOME/clausters`, else `~/.local/share/clausters`. Persistence applies to the real-time server only; offline `--nrt` renders never read or write it.

A client can also load defs **on demand** from an arbitrary path, complementing the boot-time reload: `/d_load <path>` loads one SynthDef spec file (the Clausters def format — the same `SynthDefSpec` JSON `/d_recv` carries), and `/d_loadDir <dir>` loads every `*.json` SynthDef in a directory (a single unreadable/invalid file fails the whole command, naming it). Both compile through the `/d_recv` path (so the def is also persisted under its name) and reply `/done`. GraphDefs load through `/d_graph`, Faust defs through `/d_faust`.

The def kinds live in subdirectories of a `defs/` directory (so the data
directory itself is free for other persistent aspects); `midi.json` and
`boot.json` sit at the top level:

| path | written on | content |
|---|---|---|
| `<dir>/defs/synthdefs/<name>.json` | `/d_recv` | the `SynthDefSpec` JSON, verbatim |
| `<dir>/defs/faustdefs/<name>.json` | `/d_faust` | a record: the original Faust source/JSON, the libfaust version, and the payload's SHA-256 |
| `<dir>/defs/faustdefs/<name>.<sha>.bc` | `/d_faust` | the compiled LLVM **bitcode** (a speed cache) |
| `<dir>/defs/graphdefs/<name>.json` | `/d_graph` | the `GraphDefSpec` JSON, verbatim |
| `<dir>/midi.json` | `/midi_bind`/`/midi_unbind`/`/midi_map` | the MIDI bindings (channel → instrument + target + control map) |
| `<dir>/boot.json` | *authored by the user/client* | the boot preset: standalone GraphDefs to instantiate at startup |

GraphDefs reload after the synth/faust defs (their members reference those names); validation is structural only, so a member def that is still missing at load is caught later, at `/graph_new`.

### MIDI-standalone: bindings + boot preset

So the server can be **played from a MIDI controller with no OSC programming at all**, the MIDI bindings persist too. Every `/midi_bind`/`/midi_unbind`/`/midi_map` rewrites `midi.json`; at startup — **after** the defs and GraphDefs are in place, so a binding's instrument name resolves — each binding is re-established (a GraphDef binding re-instantiates its shared instance). The minimal workflow becomes: drop a SynthDef/FaustDef (or a GraphDef) and a binding in the data dir once, then every later `clausters --midi --data-dir <dir>` comes up already bound — connect a controller (`aconnect`) and play. The default control map (note→`freq`, velocity→`amp`, note-off→`/n_free`/gate) makes a restored binding immediately playable.

`boot.json` is an optional, user-authored **boot preset**: a JSON array of standalone GraphDefs to instantiate at boot (an always-on reverb bus, a drone, a mixer), each `{"graph": "<name>", "ports": {"<port>": value, ...}}`. They are instantiated (the equivalent of `/graph_new <name> -1 0 0 ...`) after the bindings, so a fresh boot comes up already wired. The boot order is **defs → graphdefs → bindings → boot preset**. All of it honours `--no-persist` (off → nothing read or written) and the data-dir resolution; it applies to the real-time server only (NRT never persists).

The stored **definition** (the JSON) is always the source of truth: it is transparent, human-readable, and what gets recompiled. The Faust `.bc` is a non-authoritative cache — on reload the server re-creates the factory from bitcode (skipping Faust's front-end) only when the libfaust version still matches and the file is intact; otherwise it silently recompiles from the source and rewrites the cache. A libfaust upgrade therefore invalidates every `.bc` automatically. `/d_free <name>` deletes both files. Re-sending a name overwrites them.

Reloading is **incremental**: the socket starts serving immediately and the library loads in the background on the compiler thread, so a large Faust library does not delay startup — a def simply "does not exist yet" until its reload finishes.

## Generating defs programmatically

`examples/json_client.py` (Python, stdlib only) builds all three formats with a few helper functions and drives the whole lifecycle over OSC — use it as a reference client. The equivalence of the families is pinned down by the golden tests in `tests/faust_parity.rs`: a UGen graph, the box translation and the signal translation render side by side in one engine and must agree (bit-exactly for stateless arithmetic on shared input, within float tolerance for oscillators, since `Sine` accumulates phase in f64 and Faust in f32).
