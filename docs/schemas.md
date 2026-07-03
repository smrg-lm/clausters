# Def schemas

User-facing reference: wire formats and OSC commands. For the server's internals — threads, memory lifecycle, invariants, adding UGens — see [`architecture.md`](architecture.md).

Clausters accepts instrument definitions in two wire formats, both loaded hot over OSC (UDP, default port `127.0.0.1:57110`):

| format | OSC command | availability |
|---|---|---|
| **SynthDef JSON** — a flat list of UGens | `/d_recv <blob>` | always |
| **Faust def** — Faust source *or* a JSON box tree | `/d_faust <name> <string>` | `faust` feature |

Both reply asynchronously: `/done` with the command (and the def name for `/d_faust`), or `/fail` with a human-readable error. Once loaded, defs of either kind are instantiated, controlled and freed the same way:

### The async barrier (`/sync`)

`/sync <int id>` is the general way to wait for asynchronous work: the server replies `/synced <id>` once **every async command received before this `/sync`** has completed — Faust compiles (`/d_faust`), SynthDef sends (`/d_recv`) and buffer jobs (`/b_*`), each of which runs on its own FIFO worker thread. Use it instead of matching individual `/done`s when you fire several async commands and just want to know they have all landed (e.g. after sending a def without waiting, before the `/s_new` that needs it). It is a real barrier, not a round-trip of `/status`: a `/synced` guarantees the prior compiles/jobs are installed.

```text
/s_new  name id addAction targetID [ctlName value]...   # ctl args: s f pairs
/n_set  id ctlName value
/n_map  id ctlName busIndex...                           # control bus -> control
/n_mapa id ctlName busIndex...                           # audio bus  -> control
/n_free id
/d_free name...                                          # SynthDef JSON only
```

Engine facts that apply to every def: blocks of 64 samples; **by default** 128 audio buses (`0..channels` are the hardware outputs, bus 0 = left) and 1024 control buses; a pool of 1024 sample buffers filled by the `/b_*` commands; all signals are `f32` at the configured sample rate. The bus counts are set at boot by `--audio-buses` (≤128) and `--control-buses`, and the sample rate by `--sample-rate` (default 48000; PipeWire honors it per-application). A client reads the live configuration with **`/server_info`** → `/server_info.reply [audio_buses, control_buses, output_channels, block_size, nominal_sr, actual_sr]`, so it can size its own bus allocators from the server instead of assuming the defaults.

### Mapping controls to buses (`/n_map`, `/n_mapa`)

`/n_set` writes a control once. `/n_map id ctl bus` instead **binds** the control to a **control bus**: the node re-reads that bus at the start of every block, so the control tracks whatever any client (`/c_set`) or synth (`Out` to a control bus) writes there — no further `/n_set`. `/n_mapa` is the same against an **audio bus**. Both take any number of `ctl bus` pairs, by control name or index, and work for UGen controls and Faust parameters alike.

A `busIndex` of `-1` removes the mapping (the control keeps its last value); a later `/n_set` on the same control also clears it and fixes the value.

### Addressing a group (scsynth group semantics)

`/n_set`, `/n_map` and `/n_mapa` accept a **group** id as well as a synth id. Addressed to a group, the command transfers each named control **down the group's subtree** to every synth (and Faust node) that has a control of that name, recursing through subgroups and stopping at each synth — the standard scsynth behaviour, so one message moves a parameter across a whole bank of nodes. A node without a matching control name is simply skipped; an empty group is a no-op; an unknown id replies `/fail`. Addressed to a single synth, the command sets only that synth, as before.

Because a control is one value per block, `/n_mapa` **samples** one frame of the audio bus per block (control rate) — this matches scsynth for a control-rate control; there are no audio-rate controls here (feed an audio signal through `In`/an input bus instead). Mapping a control that is used as a bus index makes the node a dynamic barrier for auto/parallel groups, and an audio map adds that bus to the node's reads so the dependency analysis stays correct.

## Timed bundles

OSC bundles carry an NTP timetag. The immediate tag (`1`) executes on arrival; a **future** timetag is converted to a position on the server's sample clock and the whole bundle fires **sample-accurately**: the engine splits the audio block at the event's exact sample, so a `/s_new` scheduled mid-block starts on that very frame. Bundles with equal times run in arrival order; late bundles run immediately (and are logged). Nested bundles are scheduled independently by their own timetags.

Schedulable inside a timed bundle: `/s_new`, `/n_set`, `/n_map`, `/n_mapa`, `/n_free`, `/n_before`, `/n_after`, `/g_new`, `/g_freeAll`, `/g_deepFree`, `/c_set`, `/g_sortMode`, `/g_parallel`, `/graph_new`, `/graph_voice`. Anything else (defs, buffers, server commands) replies `/fail … cannot be scheduled in a timed bundle` — load defs and buffers first, then schedule the notes.

Also beyond scsynth: **auto-sorted groups**. `/g_sortMode groupID 1` makes a group keep its children in dependency order inferred from the buses each def reads and writes — no more manual `/n_before` bookkeeping; query what the server inferred with `/g_queryTree` (scsynth-compatible reply) and `/g_dumpGraph`. See [`auto-order.md`](auto-order.md) and `examples/auto_order.py`. The same analysis powers **parallel groups**: `/g_parallel groupID 1` (with the server started as `--workers N`) runs a group's independent children on several cores, bit-identically to the sequential result — see [`parallel.md`](parallel.md).

Beyond scsynth, the server also exposes its **sample clock** directly: `/clock` queries the sample counter, the actual sample rate and the server's OSC time captured with the counter (the master-clock anchor), and `/sched <int64 target> <blob>` schedules a packet at an **absolute sample** instead of an NTP time — same queue, same sample accuracy, drift-free by construction. See [`sample-clock.md`](sample-clock.md) and `examples/sample_clock.py`.

It also keeps a **shared transport** — a beat grid for phase-aligning several clients on that sample clock, plus a DAW-style rolling state. `/transport` with no args **queries** it (replies `/transport.reply <int64 originSample> <double tempo> <int32 defined> <int32 playing> <double position>`, `defined` 0 until one is set; the last two fields are appended, so older clients reading the first three still work); `/transport <int64 originSample> <double tempo>` **sets** the grid (last writer wins, stopped at position 0, replies `/done`). Beat `b` is sample `originSample + b·rate/tempo`. The **rolling state** rides on top: `/transport_play [<double position>]` starts playing (from `position`, or where it last stopped), `/transport_stop` stops, `/transport_locate <double position>` sets the song position; each replies `/done` and needs a grid defined first. Any change is **pushed** to every `/notify` client as a `/transport.reply`, so a responder re-aligns or rolls its playhead live. It is an in-memory registry the server stores, serves and broadcasts but **never schedules audio from** — clients read it to start their routines/playheads on the same beat. See [`sample-clock.md`](sample-clock.md).

UDP is not the only transport: local clients can speak the same OSC through **shared memory** (`clausters --shm <path>`) or run the whole server **in-process** through the embed C ABI, with the sample clock and the control buses readable and writable directly in mapped memory. See [`ipc.md`](ipc.md) and `clients/python/clausters/ipc.py`.

The server also speaks **TCP** when started with `clausters --tcp [port]` (default port `57110`, alongside UDP — separate namespaces). **UDP is always on** — it is the base transport and cannot be turned off, so `--tcp` *adds* a TCP listener rather than replacing it (there is no TCP-only mode); UDP doubles as internal infrastructure, since the TCP loop wakes itself with a zero-length datagram to the server's own UDP socket. Each OSC packet — message or bundle — is **length-prefixed**: a 4-byte big-endian byte count followed by exactly that many OSC bytes, the same framing scsynth uses; replies come back framed the same way over the one connection. TCP gives a reliable, ordered, connection-oriented channel (no datagram size limit, no silent packet loss) for clients that want it; timing still rides on bundle timetags / `/sched`, so arrival latency does not affect *when* a scheduled command fires. The Python client speaks it through `OscTcpInterface`.

The server also speaks **WebSocket** when started with `clausters --ws [port]` (default port `57120`) — always available, like TCP and shared memory, not behind a build feature. This is the transport a **browser** can reach — a browser cannot open a raw UDP socket or map shared memory, but speaks WebSocket natively — so it is what lets a web-hosted client drive the server (and the server "run in the browser"). It carries the same OSC the same way the other transports do, multiplexed into the same loop and waking it with the same zero-length-UDP trick; the one framing difference from TCP is that each OSC packet is one WebSocket **binary** message, so the frame boundary *is* the packet boundary and there is no length prefix (replies come back as binary messages). Inbound bytes validate through the same single decode door as every transport. The Python client speaks it through `OscWsInterface`; a browser uses the native `WebSocket` API (see `examples/ws_ping.py` and `examples/ws_ping.html`). `ws://` only — TLS (`wss://`) is out of scope, terminate it at a reverse proxy if needed.

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
    {"kind": "SinOsc", "inputs": [{"control": 0}]},
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
| `SinOsc` | freq (Hz) | sine by f64 phase accumulation, starts at phase 0 |
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
| `Out` | bus, signal | **sums** the signal into an audio bus |
| `ReplaceOut` | bus, signal | overwrites the bus instead of summing |
| `PlayBuf` | bufnum, chan, rate, loop | buffer player with linear interpolation; `rate` is frames per output sample (1.0 = the server rate — multiply by `BufRateScale(bufnum)`, i.e. `file_sr / server_sr`, for the file's pitch); starts at frame 0, silent at the end unless looping |
| `BufRd` | bufnum, chan, phase, loop | reads the buffer at a `phase` signal in frames (linear interpolation); out-of-range phases wrap when looping, clamp otherwise |
| `BufSampleRate` | bufnum | the buffer's own sample rate (Hz), block-constant |
| `BufRateScale` | bufnum | `file_sr / server_sr`; feed `PlayBuf`'s `rate` (`rate: BufRateScale(buf) * pitch`) to play at the file's true pitch without the client knowing either rate |
| `BufFrames` | bufnum | frame count, block-constant |
| `BufChannels` | bufnum | channel count, block-constant |
| `BufDur` | bufnum | duration in seconds (`frames / file_sr`), block-constant |
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

**Envelopes (`EnvGen`).** `EnvGen` plays a breakpoint envelope, modelled on SuperCollider's. Its inputs are five fixed signals followed by a flat **envelope array**: `gate, levelScale, levelBias, timeScale, doneAction`, then `initLevel, numSegments, releaseNode, loopNode`, then four values **per segment** — `target, duration, shape, curve`. The output is `envelope · levelScale + levelBias`; `timeScale` stretches every segment's duration. A rising `gate` (re)triggers from `initLevel`; while the gate is held the envelope **sustains** at `releaseNode` (an index into the levels — hold that level until release); when the gate falls it plays the segments from `releaseNode` on. `releaseNode < 0` disables the sustain, so the envelope plays straight through (a one-shot). With a `loopNode` (an index `< releaseNode`), the held phase **cycles** the segments in `[loopNode, releaseNode)` instead of holding a single level, carrying the level at the release node back as the loop's start; the release still plays out from `releaseNode` when the gate falls. `loopNode < 0` disables looping. The **shape** numbers are `0` step, `1` linear, `2` exponential (needs same-sign, non-zero levels), `3` sine, `4` welch, `5` custom-curvature (bent by the `curve` value: 0 linear, positive starts slow, negative starts fast), `6` squared, `7` cubed, `8` hold. When the last segment finishes the UGen applies its `doneAction`: `0` none, `1` pause the synth (skipped from then on but kept in the tree; there is no `/n_run` to resume it yet), `2` free the synth, `14` free its enclosing group (the synth included; if that group is the un-freeable root it frees just the synth) — freeing happens on the audio thread through the garbage FIFO, never a blocking free. Any input can be a signal, so a control can drive the gate or scale the levels/times live. The Python client builds the array with the `Env` breakpoint helper (`Env.adsr`, `Env.perc`, `Env.asr` …, plus `release_node`/`loop_node`) and the `env_gen` callable.

**Operator UGens (`BinaryOpUGen`/`UnaryOpUGen`).** Rather than a distinct kind per math operation, one generic UGen per arity carries the operator as a static **`op`** field (alongside `kind`/`inputs`) — the operator's **name**. So `{"kind": "BinaryOpUGen", "op": "mul", "inputs": [a, b]}` multiplies. Every operator is one entry in `clausters_core::builtins` — the **same code** the client's value FFI runs off the audio thread — so a value a client computes ahead of time and the UGen on the audio thread are **bit-identical** for the native ops. A missing or unknown `op` fails the def with `/fail` naming the node. The `Add`/`Sub`/`Mul`/`Div` kinds remain as aliases for the `add`/`sub`/`mul`/`div` operators. (Internally each operator also has a stable integer id for the C ABI, but that is an implementation detail — defs and clients only ever use the name.)

Binary operators (`BinaryOpUGen` `op`): `add` (`a+b`), `sub` (`a−b`), `mul` (`a·b`), `div` (`a/b`), `mod` (`a%b`), `pow` (`aᵇ`), `min`, `max`, `atan2`, `gt` (`a>b`), `lt` (`a<b`), `ge` (`a>=b`), `le` (`a<=b`), `eq`, `ne`, `bitand`, `bitor`, `bitxor`, `lshift`, `rshift`, `hypot` (`√(a²+b²)`), `ring1` (`ab+a`), `ring2` (`ab+a+b`), `ring3` (`a²b`), `ring4` (`a²b−ab²`), `sumsqr` (`a²+b²`), `difsqr` (`a²−b²`), `sqrsum` (`(a+b)²`), `sqrdif` (`(a−b)²`), `absdif` (`|a−b|`), `thresh` (`a<b?0:a`), `clip2` (clip to ±b), `excess` (`a−clip2`), `round` (to step b), `trunc` (to step b). Comparisons and bitwise ops follow Faust's convention (a boolean is `0.0`/`1.0`; bitwise acts on the `i32` casts).

Unary operators (`UnaryOpUGen` `op`): `neg`, `abs`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `exp`, `exp10`, `log`, `log10`, `log2`, `sqrt`, `floor`, `ceil`, `rint` (ties to even), `as_int`, `as_float`, `squared` (`x²`), `cubed` (`x³`), `recip` (`1/x`), `frac`, `sign`, `sinh`, `cosh`, `tanh`, `midicps`, `cpsmidi`, `midiratio`, `ratiomidi`, `dbamp`, `ampdb`, `octcps`, `cpsoct`, `distort` (`x/(1+|x|)`), `softclip`.

The tables are open — a new operator is one more `clausters_core::builtins` entry, no renumbering. The Python client maps its operators and math methods (`%`, `min`, `>`, `.midicps()`, `.distort()`, …) to these names automatically, and exposes `mul_add`/`sum3`/`sum4` for the fused kinds.

**Calculation rates (`ir`/`kr`/`ar`/`dr`).** Every ugen output has a rate, chosen with the optional `"rate"` field or defaulted per kind. `ar` (audio rate) is one value per sample — the normal signal wire, and the default. `kr` (control rate) is one value per block, recomputed each block; downstream it reads as a constant over the block. `ir` (initial rate) is computed **once, when the synth starts**, then held for the node's life — a fixed `SampleRate`, a `Rand` seed, a `BufFrames.ir` snapshot; its inputs must themselves be constant/`ir` (nothing that varies can be frozen). `dr` (demand rate) is pulled, not run — see the demand note. Coercion is one-way: a slower rate feeds a faster input for free (a constant broadcast over a block), but an `ar` signal cannot feed an `ir` input, and a `dr` output may only feed a `Demand` source. Audio-rate controls are not a thing here — feed an audio signal through `In`/an input bus (as noted under `/n_mapa`).

**Demand-rate sequences (`Demand`/`Dseq`).** A demand ugen produces values only when *pulled*. `Dseq` is a source — a stream of numbers (`repeats` passes over a value list, then exhausted); it never runs in the normal per-block order. `Demand` is the driver that pulls it: on each rising edge of its `trig` it demands the next value and holds it until the next trigger (a rising `reset` restarts the stream; the output is 0 before the first trigger, and holds the last value once the source is exhausted). Wire a trigger source (an `Impulse`, a `/n_set` trigger later) into `Demand`'s `trig`, and a `Dseq` into its `source`, to step through a sequence sample-accurately — the foundation the rest of the demand family (`Dseries`/`Dwhite`/`Duty`, later) builds on. This is a **minimal** driver today: end-of-stream is a held value, and one demand source per driver.

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
/b_free      bufnum
/b_query     bufnum...                 →  /b_info  bufnum frames channels sampleRate ...
/b_get       bufnum index...           →  /b_set   bufnum index value ...
/b_getn      bufnum [start count]...   →  /b_setn  bufnum start count value ...
/b_export    bufnum path               →  /done /b_export bufnum
```

`/b_alloc`, `/b_allocRead`, `/b_read`, `/b_write`, `/b_zero` and `/b_free` are **asynchronous**: the work happens on a dedicated NRT thread (one queue, so commands on the same buffer complete in submission order) and the reply is `/done <cmd> bufnum` or `/fail <cmd> reason`. Buffers keep the file's sample rate (the server never resamples — see `PlayBuf`'s rate above); integer WAVs are scaled to ±1. `/b_read` requires an allocated buffer and keeps its shape; channel-count mismatches fail. Reading decodes by **content**, not extension: WAV goes through hound (exact, int24-aware), and FLAC, OGG/Vorbis, MP3, MP4/AAC, ALAC, AIFF and CAF decode through [symphonia](https://github.com/pdeljanov/Symphonia) (whole-file decode, then slice — compressed formats have no cheap exact frame seek). `/b_write` still emits WAV only, and `leaveOpen` (streaming) is not supported.

`/b_query`, `/b_get` and `/b_getn` are **synchronous** reads, answered from the network-side buffer mirror (state as of the last completed command). `/b_get` reads single samples by flat (interleaved) index; `/b_getn` reads ranges, with `count` clamped to what the buffer holds from `start` — a request past the end returns only the available samples, and an unallocated buffer returns count 0. Sample indices are flat across channels (`frame * channels + channel`), so a stereo buffer reads as interleaved `L R L R ...`. Each reply must fit one datagram, so large buffers are read in client-chosen chunks.

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
- `buses` — internal buses, **private to each instance** (`rate` `"audio"` or `"control"`, `channels` default 1). They are allocated per instantiation from a reserved range at the **top of the bus space** — the top 32 audio buses and top 128 control buses (so `96..128` and `896..1024` at the default counts, shifting with `--audio-buses`/`--control-buses`) — so they never collide with client-allocated buses (the same idea as the reserved MIDI/auto node-id ranges). Two instances of one GraphDef get disjoint buses.
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

- **Note on** → `/s_new instrument <voiceID> addAction target freq <midi2freq(note)> amp <velocity2amp(vel)>`. Voice IDs come from a reserved server-side range (≥ `3_000_000`), disjoint from client IDs and the `/s_new -1` auto range. A note-on for an already-sounding `(channel, note)` frees the old voice first. Velocity `0` is a note-off.
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

Note that these control the **server's own** logs (on the server's stderr); the **node tree** is delivered to clients as structured data, never scraped from logs — see [Node tree introspection](#node-tree-introspection) above (`/g_queryTree`, `/n_query`, `/g_dumpGraph`).

## Persisting defs across restarts

The real-time server can persist loaded defs to a data directory and reload them automatically on the next start, so a client need not re-send its instrument library every session. It is **on by default**; control it with two flags:

```text
clausters --data-dir <dir>   # where defs are stored/reloaded
clausters --no-persist       # disable for this run
```

With no `--data-dir`, the directory is `$CLAUSTERS_DATA_DIR` if set, else `$XDG_DATA_HOME/clausters`, else `~/.local/share/clausters`. Persistence applies to the real-time server only; offline `--nrt` renders never read or write it.

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

`examples/json_client.py` (Python, stdlib only) builds all three formats with a few helper functions and drives the whole lifecycle over OSC — use it as a reference client. The equivalence of the families is pinned down by the golden tests in `tests/faust_parity.rs`: a UGen graph, the box translation and the signal translation render side by side in one engine and must agree (bit-exactly for stateless arithmetic on shared input, within float tolerance for oscillators, since `SinOsc` accumulates phase in f64 and Faust in f32).
