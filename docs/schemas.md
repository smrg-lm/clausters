# Def schemas

Clausters accepts instrument definitions in two wire formats, both loaded
hot over OSC (UDP, default port `127.0.0.1:57110`):

| format | OSC command | availability |
|---|---|---|
| **SynthDef JSON** — a flat list of UGens | `/d_recv <blob>` | always |
| **Faust def** — Faust source *or* a JSON box tree | `/d_faust <name> <string>` | `faust` feature |

Both reply asynchronously: `/done` with the command (and the def name for
`/d_faust`), or `/fail` with a human-readable error. Once loaded, defs of
either kind are instantiated, controlled and freed the same way:

```text
/s_new  name id addAction targetID [ctlName value]...   # ctl args: s f pairs
/n_set  id ctlName value
/n_free id
/d_free name...                                          # SynthDef JSON only
```

Engine facts that apply to every def: blocks of 64 samples; 128 audio buses
(`0..channels` are the hardware outputs, bus 0 = left) and 1024 control
buses; all signals are `f32` at the device sample rate.

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

- `name` — the key used by `/s_new` and `/d_free`. Re-sending a name
  replaces the def (existing synths keep playing the old one).
- `controls` (optional) — named parameters addressable from `/s_new` and
  `/n_set`, with their default values.
- `ugens` — the signal graph in execution order. Each input is one of:

  | input form | meaning |
  |---|---|
  | `{"const": x}` | a constant (finite) float |
  | `{"control": i}` | the i-th entry of `controls` |
  | `{"ugen": i}` | the single output of an **earlier** ugen (`i` < own index) |

  Forward references are rejected, which forces an acyclic, topologically
  ordered graph. Every ugen has exactly the arity listed below — wrong input
  counts, unknown kinds, out-of-range references and non-finite constants
  all come back in `/fail` naming the offending node
  (e.g. `ugens[2].inputs[0]: control 7 out of range (have 2)`).

### UGen kinds

| kind | inputs | output |
|---|---|---|
| `SinOsc` | freq (Hz) | sine by f64 phase accumulation, starts at phase 0 |
| `WhiteNoise` | — | uniform white noise in ±1 |
| `Add`, `Sub`, `Mul`, `Div` | a, b | sample-wise arithmetic |
| `In` | bus | copies an audio bus (read once per block) |
| `InCtl` | bus | a control-bus value, constant over the block |
| `Out` | bus, signal | **sums** the signal into an audio bus |
| `ReplaceOut` | bus, signal | overwrites the bus instead of summing |

Output happens exclusively through `Out`/`ReplaceOut`; a def without them is
silent. Several synths with `Out` on the same bus mix. Bus-index inputs are
ordinary signals, sampled at the first frame of each block and clamped to
the valid range.

## Faust defs (`/d_faust`)

`/d_faust name payload` — the payload is **Faust source** unless its first
non-whitespace byte is `{`, in which case it is parsed as a **JSON box
tree**. Either way it is JIT-compiled (LLVM) on a dedicated compiler thread;
expect the `/done`/`/fail` reply a few milliseconds later.

### Controls

A Faust synth exposes, in this order:

1. every UI element of the def (`hslider`, `vslider`, `nentry`, `button`,
   `checkbox`) addressed by its **label**;
2. two reserved names: `out` — first output bus (default 0, hardware left) —
   and `in` — first input bus, for defs that process signal. A def with N
   outputs writes (sums) to buses `out .. out+N-1`; same for inputs.

```text
/s_new fsine 2000 1 0 freq 330 out 1
/n_set 2000 freq 660
```

If a def declares its own `out`/`in` control, the def's wins.

### Faust source payloads

Any complete Faust program; `import("stdfaust.lib")` and friends resolve
against the stdlib installed with libfaust (`<prefix>/share/faust`).
Compilation is single precision (`FAUSTFLOAT` = `f32`) and the sample rate
is fixed per instance at `/s_new` time.

```text
/d_faust fsine 'import("stdfaust.lib"); freq = hslider("freq", 440, 20, 20000, 0.01); process = os.osc(freq) * 0.2;'
```

### JSON box tree payloads

The JSON mirrors Faust's Box API one-to-one: every node denotes a box
expression, and the tree is the `process` definition. Two shorthands: a JSON
**number** is a constant box (`int` if integral, `real` otherwise), and the
**strings** `"_"` / `"!"` are the wire and cut primitives. Everything else
is an object with an `"op"` field:

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
| `faust` | `src` | escape hatch: a complete Faust program compiled to a composable box, with stdlib access |

Example — `sin(2π·phasor(freq)) * 0.2` with `freq` as a named control
(`wrap(x) = x - floor(x)`, `phasor = (+(freq/SR) : wrap) ~ _`):

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

The `faust` op is the bridge to the stdlib — an embedded program becomes a
box you can compose with primitives:

```json
{"op": "seq", "in": [
  {"op": "faust", "src": "import(\"stdfaust.lib\"); process = os.osc(330);"},
  {"op": "mul", "in": ["_", 0.2]}
]}
```

### Errors

Structural problems (unknown ops, missing fields, wrong arities in `"in"`)
fail during interpretation and the `/fail` message carries the path of the
offending JSON node from the root `$`, e.g.
`at $.in[1].op: unknown op "mul3"`. Semantic errors — composition arity
mismatches, dangling inputs — are reported by the Faust compiler verbatim,
prefixed with the path of the fragment for `faust` ops.

## Generating defs programmatically

`examples/json_client.py` (Python, stdlib only) builds both formats with a
few helper functions and drives the whole lifecycle over OSC — use it as a
reference client. The equivalence of the two def families is pinned down by
the golden tests in `tests/faust_parity.rs`: a UGen graph and its Faust
translation render side by side in one engine and must agree (bit-exactly
for stateless arithmetic on shared input, within float tolerance for
oscillators, since `SinOsc` accumulates phase in f64 and Faust in f32).
