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

Beyond scsynth, the server also exposes its **sample clock** directly: `/clock` queries the sample counter, and `/sched <int64 target> <blob>` schedules a packet at an **absolute sample** instead of an NTP time — same queue, same sample accuracy, drift-free by construction. See [`sample-clock.md`](sample-clock.md) and `examples/sample_clock.py`.

UDP is not the only transport: local clients can speak the same OSC through **shared memory** (`clausters --shm <path>`) or run the whole server **in-process** through the embed C ABI, with the sample clock and the control buses readable and writable directly in mapped memory. See [`ipc.md`](ipc.md) and `clients/python/clausters/transport.py`.

The server also speaks **TCP** when started with `clausters --tcp [port]` (default port `57110`, alongside UDP — separate namespaces). **UDP is always on** — it is the base transport and cannot be turned off, so `--tcp` *adds* a TCP listener rather than replacing it (there is no TCP-only mode); UDP doubles as internal infrastructure, since the TCP loop wakes itself with a zero-length datagram to the server's own UDP socket. Each OSC packet — message or bundle — is **length-prefixed**: a 4-byte big-endian byte count followed by exactly that many OSC bytes, the same framing scsynth uses; replies come back framed the same way over the one connection. TCP gives a reliable, ordered, connection-oriented channel (no datagram size limit, no silent packet loss) for clients that want it; timing still rides on bundle timetags / `/sched`, so arrival latency does not affect *when* a scheduled command fires. The Python client speaks it through `OscTCPInterface`.

## NRT mode (offline rendering)

The same engine renders scores to WAV without an audio device:

```sh
clausters --nrt score.osc out.wav [--rate 48000] [--channels 2] [--format float|int16|int24]
```

A score is the scsynth binary format: OSC packets back to back, each preceded by its byte count as a big-endian `int32`. Timetags count **seconds from the start of the render** (the immediate tag is time 0); bundles fire sample-accurately exactly like in real time, so an offline render equals a perfectly timed live take. The render ends at the time of the **last** bundle, whose commands produce no sound — close every score with a dummy bundle (a final `/n_free`) to set the duration.

Unlike the live server, a score bundle may also contain the asynchronous commands `/d_recv`, `/d_faust`, `/d_free` and the `/b_*` family: they complete synchronously before time advances (scsynth NRT semantics), and any failure aborts the render with the offending event's time and message. Queries (`/status`, `/b_query`, `/c_get`) are errors in a score.

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
- `controls` (optional) — named parameters addressable from `/s_new` and `/n_set`, with their default values.
- `ugens` — the signal graph in execution order. Each input is one of:

  | input form | meaning |
  |---|---|
  | `{"const": x}` | a constant (finite) float |
  | `{"control": i}` | the i-th entry of `controls` |
  | `{"ugen": i}` | the single output of an **earlier** ugen (`i` < own index) |

  Forward references are rejected, which forces an acyclic, topologically ordered graph. Every ugen has exactly the arity listed below — wrong input counts, unknown kinds, out-of-range references and non-finite constants all come back in `/fail` naming the offending node (e.g. `ugens[2].inputs[0]: control 7 out of range (have 2)`).

### UGen kinds

| kind | inputs | output |
|---|---|---|
| `SinOsc` | freq (Hz) | sine by f64 phase accumulation, starts at phase 0 |
| `Impulse` | freq (Hz) | single-sample `1.0` every `freq` Hz, `0.0` between; the first output sample is always an impulse, so a `/sched`'d `/s_new` places it on an exact frame; `freq` 0 emits one impulse then silence (f64 phase, drift-free) |
| `WhiteNoise` | — | uniform white noise in ±1 |
| `Add`, `Sub`, `Mul`, `Div` | a, b | sample-wise arithmetic |
| `In` | bus | copies an audio bus (read once per block) |
| `InCtl` | bus | a control-bus value, constant over the block |
| `Out` | bus, signal | **sums** the signal into an audio bus |
| `ReplaceOut` | bus, signal | overwrites the bus instead of summing |
| `PlayBuf` | bufnum, chan, rate, loop | buffer player with linear interpolation; `rate` is frames per output sample (1.0 = the server rate — scale by `file_sr / server_sr` for the file's pitch); starts at frame 0, silent at the end unless looping |
| `BufRd` | bufnum, chan, phase, loop | reads the buffer at a `phase` signal in frames (linear interpolation); out-of-range phases wrap when looping, clamp otherwise |
| `LocalIn` | channel | reads synth-private feedback channel `channel` (a constant); see feedback note below |
| `LocalOut` | channel, signal | writes `signal` into synth-private feedback channel `channel` (a constant); also passes `signal` through as its own output |

**Feedback (`LocalIn`/`LocalOut`).** The graph is a DAG — UGens cannot be wired in a cycle. To feed a signal back, write it with `LocalOut` and read it with `LocalIn`: they share a per-synth buffer that persists across blocks, so the value read is what was written **one control block (64 samples) earlier**. `LocalIn` for a channel must appear *before* its `LocalOut` (the compiler enforces this; it is what makes the delay exactly one block), and the channel index must be a constant. Use any number of channels (mono each, like buses). This is **block-rate** feedback — good for feedback delays, block feedback-FM, resonant combs (a one-channel loop resonates at `sampleRate / 64`). Sample-accurate (sub-block) feedback is not possible across composed UGens; fuse the loop into one node — a recursive UGen or a Faust def (`/d_faust` with `~`).

Output happens exclusively through `Out`/`ReplaceOut`; a def without them is silent. Several synths with `Out` on the same bus mix. Bus-index inputs are ordinary signals, sampled at the first frame of each block and clamped to the valid range.

Buffer readers are **mono** (one output per UGen, unlike scsynth's multi-output PlayBuf): the `chan` input picks the channel, and two readers with the same inputs stay sample-locked, so a stereo file is two UGens. Neither has a trigger or done action yet.

## Buffers (`/b_*`)

```text
/b_alloc     bufnum frames [channels=1]                  # zeroed buffer
/b_allocRead bufnum path [fileStart=0] [numFrames=0=all] # shape from the WAV
/b_read      bufnum path [fileStart=0] [numFrames=-1=all] [bufStart=0]
/b_write     bufnum path [header="wav"] [format="int16"|"int24"|"float"] [numFrames=-1] [startFrame=0]
/b_zero      bufnum
/b_free      bufnum
/b_query     bufnum...    →  /b_info  bufnum frames channels sampleRate ...
```

All except `/b_query` are **asynchronous**: the work happens on a dedicated NRT thread (one queue, so commands on the same buffer complete in submission order) and the reply is `/done <cmd> bufnum` or `/fail <cmd> reason`. Buffers keep the file's sample rate (the server never resamples — see `PlayBuf`'s rate above); integer WAVs are scaled to ±1. `/b_read` requires an allocated buffer and keeps its shape; channel-count mismatches fail. WAV is the only file format in v1, and `leaveOpen` (streaming) is not supported.

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

There is deliberately no `soundfile` op: sample data lives in the server's buffers. Play a buffer with `PlayBuf`/`BufRd` in a UGen synth, route it through an audio bus, and let the Faust def read that bus via its reserved `in` control — the signal crosses over without copying anything into the Faust world, and both def families stay composable on the same buses.

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

`/graph_new name id addAction targetID [port value]...` instantiates a GraphDef: it creates an **auto-sorted group** (the member execution order follows the bus connections, M12) at `id` (or `-1` for a server-assigned id), holding the **shared** members, then applies the shared `defaults` and the given `port value` overrides. `/n_set id port value...` on that group id resolves the port names against the surface (never the member ids); a port absent from the surface is ignored. `/n_free id` (or `/g_deepFree`) tears the instance down (with all its voices) and reclaims its private buses. Instantiation is atomic: a missing member def or bus shortfall fails with no partial instance. GraphDefs work in NRT scores too (scored like any def at time 0).

### Shared vs per-voice, and `/graph_voice`

A GraphDef splits into a **shared** part (members without `voice`) and a **per-voice** part (members with `"voice": true`) — the model of a polyphonic instrument: the shared part (the private bus, a mixer, effects) exists once; each note adds a voice. A surface port maps either to shared members or to voice members, never a mix (a `/fail` at `/d_graph` otherwise): shared ports apply to the instance, voice ports to each voice.

`/graph_new` instantiates only the shared members. `/graph_voice instanceID id [port value]...` then spawns the per-voice members as a **sub-group at the head of the instance** (the auto-sort orders it before the shared mixer that reads its bus), wired to the same private buses, applying the voice-port `defaults` and overrides. `/n_set voiceID port value...` resolves against the voice's surface; `/n_free voiceID` frees just that voice. A `/graph_voice` on an instance whose def has no voice members (or on an unknown instance) `/fail`s.

A **GraphDef can be bound to MIDI** exactly like a SynthDef/FaustDef: `/midi_bind channel graphname [target addAction gate]` spawns the shared instance once at bind time and every note becomes a `/graph_voice` into it (note → `freq` port, velocity → `amp` port, note-off frees the voice or, for a gate-aware binding, sets its `gate` port to 0). The GraphDef must have per-voice members. `/midi_unbind` frees the shared instance (and all its voices).

## MIDI control protocol (standard channel-voice actuation)

Besides OSC, the server can be driven by **standard channel-voice MIDI** — note on/off, velocity, aftertouch, pitch-bend, control change, program change. This is the **primary** MIDI path: a note actuates a synthesis node and an expressive message sets a named control, exactly the surface a sequencer or DAW already speaks. (SysEx, when it lands, is reserved for the non-musical control plane — def load, buffers, topology — and is never a tunnel for OSC commands.)

A channel must first be **bound** to an instrument. Binding and mapping are OSC commands (so they ride the same reliable path as the rest of the protocol); the note/control events themselves arrive over the OS's standard MIDI. Start the server with `--midi [name]` (RT only) to open a **virtual ALSA input port** (default name `clausters`) — the same system MIDI any controller or DAW uses; route anything into it (`aconnect`, a keyboard through the kernel, a DAW). Live input is MIDI 1.0 (7-bit, widened internally to the high-resolution form); the full MIDI 2.0/UMP resolution is preserved on the client's persistence path (the `clausters-midi` crate's MIDI 2.0 clip file). Network MIDI is a separate, deliberately out-of-scope idea.

- `/midi_bind channel instrument [target] [addAction] [gate]`: bind a MIDI channel (`0`–`255`: the classic 16 plus the extended UMP group×channel space) to an **instrument def** — a SynthDef, a FaustDef, **or a GraphDef** (M18), actuated identically. Default target is the root group (`0`), default add-action `0` (head). `gate` non-zero marks the def gate-aware (see note off below). The default control map is `freq`/`amp`, matching the client `Event` convention. A **GraphDef** instrument spawns its shared instance at bind time and turns each note into a `/graph_voice` (see the GraphDef section); it must have per-voice members.
- `/midi_unbind channel`: remove the binding and free every voice still sounding on that channel.
- `/midi_map channel selector name`: route a message type to a control. Selectors: `note` (→ frequency control, default `freq`), `vel` (→ amplitude, default `amp`), `gate` (gate control name), `bend` (pitch-bend → control), `pressure` (channel aftertouch → control), `poly` (per-note aftertouch → the note's voice), `ccN` (control change number `N` → control), `progN` (program `N` → an instrument def `name` to switch to).

Actuation semantics, per message type (each with its named conversion):

- **Note on** → `/s_new instrument <voiceID> addAction target freq <midi2freq(note)> amp <velocity2amp(vel)>`. Voice IDs come from a reserved server-side range (≥ `3_000_000`), disjoint from client IDs and the `/s_new -1` auto range. A note-on for an already-sounding `(channel, note)` frees the old voice first. Velocity `0` is a note-off.
- **Note off** → `/n_free <voiceID>`, or `/n_set <voiceID> <gateControl> 0` when the binding is gate-aware.
- **Poly aftertouch** → `/n_set` on that note's voice; **channel aftertouch**, **control change**, **pitch-bend** → `/n_set` on every live voice of the channel, for the mapped control. Unmapped expressive messages and unbound channels are silently ignored (a running MIDI stream never errors).
- **Program change** → re-selects the channel's instrument def from its `prog`-mapped table (no-op if unset).

Conversions (`note → midi2freq`, `velocity → velocity2amp`, `aftertouch`, `bend`, `cc`, `program`) take **MIDI 2.0 / UMP resolution** (16-bit velocity, 32-bit controllers/pressure/bend) and produce the `f32` a control zone wants — no 7-bit quantization. **MIDI 1.0 is backward-compatible**: classic 7/14-bit input is accepted and widened up to those, so the same controls are driven either way. Because a MIDI voice is realized as the *same* `/s_new`/`/n_set`/`/n_free` an OSC client would send, it is byte-identical to the OSC equivalent (`tests/midi.rs` guards this).

## Server logging and verbosity

The server logs to **stderr** through `tracing`, at five levels (`error`, `warn`, `info`, `debug`, `trace`). The startup banner and the NRT render summary go to **stdout** (they are program output, not logs). The **audio thread never logs**: it reports conditions over the lock-free FIFOs and the network thread emits them, so logging never breaks real-time safety.

The level is set, in increasing precedence, by:

- the CLI flags `-v` (info), `-vv` (debug), `-vvv` (trace), `-q` (errors only); default is `warn`;
- the `RUST_LOG` environment variable, an [`EnvFilter`](https://docs.rs/tracing-subscriber) directive that also filters per module — e.g. `RUST_LOG=clausters::osc=trace` to see only OSC traffic;
- at runtime, **from a client**, with two OSC commands (both reply `/done`):
  - `/verbosity <int|string>` — an int level (`-1` errors … `3` trace) or an `EnvFilter` directive string. Lets a client retune the server's logs without restarting.
  - `/dumpOSC <flag>` — toggles the OSC-traffic dump (the `clausters::osc` trace target). Unlike scsynth, this is **not** an ad-hoc console print: it routes through the same logging system, controllable by `/verbosity`/`RUST_LOG`, on stderr.

Note that these control the **server's own** logs (on the server's stderr); the **node tree** is delivered to clients as structured data, never scraped from logs — query it with `/g_queryTree` (scsynth-compatible reply, controls included with flag 1) and the inferred bus graph with `/g_dumpGraph`.

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
| `<dir>/midi.json` | `/midi_bind`/`/midi_unbind`/`/midi_map` | the MIDI bindings (channel → instrument + target + control map), M19 |
| `<dir>/boot.json` | *authored by the user/client* | the boot preset: standalone GraphDefs to instantiate at startup, M19 |

GraphDefs reload after the synth/faust defs (their members reference those names); validation is structural only, so a member def that is still missing at load is caught later, at `/graph_new`.

### MIDI-standalone: bindings + boot preset (M19)

So the server can be **played from a MIDI controller with no OSC programming at all**, the MIDI bindings persist too. Every `/midi_bind`/`/midi_unbind`/`/midi_map` rewrites `midi.json`; at startup — **after** the defs and GraphDefs are in place, so a binding's instrument name resolves — each binding is re-established (a GraphDef binding re-instantiates its shared instance). The minimal workflow becomes: drop a SynthDef/FaustDef (or a GraphDef) and a binding in the data dir once, then every later `clausters --midi --data-dir <dir>` comes up already bound — connect a controller (`aconnect`) and play. The default control map (note→`freq`, velocity→`amp`, note-off→`/n_free`/gate) makes a restored binding immediately playable.

`boot.json` is an optional, user-authored **boot preset**: a JSON array of standalone GraphDefs to instantiate at boot (an always-on reverb bus, a drone, a mixer), each `{"graph": "<name>", "ports": {"<port>": value, ...}}`. They are instantiated (the equivalent of `/graph_new <name> -1 0 0 ...`) after the bindings, so a fresh boot comes up already wired. The boot order is **defs → graphdefs → bindings → boot preset**. All of it honours `--no-persist` (off → nothing read or written) and the data-dir resolution; it applies to the real-time server only (NRT never persists).

The stored **definition** (the JSON) is always the source of truth: it is transparent, human-readable, and what gets recompiled. The Faust `.bc` is a non-authoritative cache — on reload the server re-creates the factory from bitcode (skipping Faust's front-end) only when the libfaust version still matches and the file is intact; otherwise it silently recompiles from the source and rewrites the cache. A libfaust upgrade therefore invalidates every `.bc` automatically. `/d_free <name>` deletes both files. Re-sending a name overwrites them.

Reloading is **incremental**: the socket starts serving immediately and the library loads in the background on the compiler thread, so a large Faust library does not delay startup — a def simply "does not exist yet" until its reload finishes.

## Generating defs programmatically

`examples/json_client.py` (Python, stdlib only) builds all three formats with a few helper functions and drives the whole lifecycle over OSC — use it as a reference client. The equivalence of the families is pinned down by the golden tests in `tests/faust_parity.rs`: a UGen graph, the box translation and the signal translation render side by side in one engine and must agree (bit-exactly for stateless arithmetic on shared input, within float tolerance for oscillators, since `SinOsc` accumulates phase in f64 and Faust in f32).
