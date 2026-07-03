# Defining instruments: FaustDef and SynthDef

An instrument is a **def** — a named processing graph the server compiles once and then instantiates many times as nodes. The client builds two kinds, both living in `clausters.defs`:

- **`FaustDef`** — a Faust definition, sent with `/d_faust`. Its graph is the full Faust **Signal API**, so it has the complete maths vocabulary (trigonometry, `exp`/`log`, comparisons, tables, sample-accurate feedback). Reach for it for any actual DSP.
- **`SynthDef`** — a UGen graph, sent with `/d_recv`. It wires the server's **structural** UGens (oscillator, noise, impulse, bus I/O, buffer playback, feedback) and the full unary/binary **maths** — the arithmetic operators plus `%`, `min`/`max`, comparisons, `.sin()`, `.midicps()`, `.distort()` … — which compose the generic operator UGens (see [Maths on a UGen graph](#maths-on-a-ugen-graph)). Genuinely custom per-sample DSP (recursion, tables, sample-accurate feedback) still goes in a `FaustDef`.

Both are built the same way: **lowercase callables** that compose with ordinary Python operators into a JSON tree. Both are **instance-based** — there is no thread-global "current graph" as in sclang, so the tree *is* the composed objects and several defs build concurrently. And both are sent **asynchronously**, behind the `/sync` barrier (see [Sending a def](#sending-a-def)).

This page is the conceptual map and the catalog of what each callable does. The exact wire format — the JSON node shapes, the UGen registry, the `/d_faust` / `/d_recv` signatures and their `/done` / `/fail` replies — is specified in the **[Clausters server book](https://clausters.readthedocs.io/)** (the schemas / OSC reference chapter); this client is one consumer of it. The generated [API reference](api.md) carries the per-symbol signatures.

## The shared shape

Every graph node is an `AbstractObject`, the operator-overloading base shared with the value layer. Composing two nodes — or a node and a plain number — returns a new node rather than a computed value:

```python
from clausters.defs import signals as S

freq = S.hslider("freq", 220.0, 20.0, 20000.0, 0.01)   # a Signal (a control)
detuned = freq * 1.5                                    # another Signal, not a float
```

A plain number that meets a node becomes a **constant** in the graph. The operators map on both def kinds: a `FaustDef` accepts the full set below, and a `SynthDef` accepts the same unary/binary maths (`+ - * /` map to dedicated kinds, everything else to the generic operator UGens — see [Maths on a UGen graph](#maths-on-a-ugen-graph)). Only a selector with no server op raises `TypeError`.

## FaustDef

### Building one

Three constructors, one per payload the server's `/d_faust` accepts:

| Constructor | Payload | Use it for |
| --- | --- | --- |
| `FaustDef.from_signals(name, *outputs)` | a **signal tree** built with `clausters.defs.signals` | graphs assembled in Python from the primitives below |
| `FaustDef.from_source(name, src)` | a Faust **source** string | hand-written Faust (`process = ...;`) |
| `FaustDef.from_box(name, box)` | a raw **box tree** dict | a pre-built Box-API tree |

Each argument to `from_signals` is one **output** (a `Signal` or a number): one argument is mono, two is stereo, and so on.

```python
from clausters.defs import signals as S, FaustDef

freq = S.hslider("freq", 220.0, 20.0, 20000.0, 0.01)
phase = S.rec(lambda s: (s + freq / S.sr()) % 1.0)   # one-sample feedback phasor
sine = S.sin(phase * S.TAU) * 0.2
fdef = FaustDef.from_signals("fsine", sine)          # one output -> mono
```

A source def is the escape hatch when you would rather write Faust directly:

```python
FaustDef.from_source("organ", "import(\"stdfaust.lib\"); process = os.osc(220) * 0.2;")
```

### The signal API

`clausters.defs.signals` (imported as `S` by convention) is the Signal API as lowercase callables. Arithmetic, comparison and bitwise **operators** compose nodes directly; the rest are functions or methods.

**Maths via operators and methods.** These compose a `Signal`:

| Group | How |
| --- | --- |
| Arithmetic | `+ - * /`, `%` (modulo), `**` (power), unary `-` |
| Comparison | `< <= > >=` — return a 0/1 signal, for gating with `select2` / `select3` |
| Bitwise | `& \| ^ << >>` |
| Unary methods | `.abs() .floor() .ceil() .sin() .cos() .tan() .asin() .acos() .atan() .exp() .log() .log10() .sqrt() .as_int() .as_float()` |
| Binary methods | `.min(b) .max(b) .atan2(b) .pow(b) .mod(b)` |

The same unary/binary maths is also available as **module functions** (so you can write `S.sin(x)` as well as `x.sin()`), plus a few that have no method form:

| Kind | Functions |
| --- | --- |
| Unary | `sin cos tan asin acos atan exp exp10 log log10 sqrt abs floor ceil rint` |
| Binary | `min max pow atan2 fmod rem` |

**Sources and structure:**

| Callable | Builds |
| --- | --- |
| `input(index=0)` | audio input channel `index` |
| `delay(x, n)` | `x` delayed by `n` samples |
| `delay1(x)` | `x` delayed by one sample (Faust `'`) |
| `recursion(body)` | single-sample feedback; `body` may reference `self_()` |
| `self_()` | the one-sample-delayed output of the enclosing `recursion` |
| `rec(fn)` | Pythonic feedback sugar: `fn(s)` builds the body from its own delayed output `s` |
| `select2(sel, a, b)` | picks `a` or `b` by a 0/1 selector |
| `select3(sel, a, b, c)` | picks `a`, `b` or `c` by a 0/1/2 selector |
| `signal(x)` | coerces a number (or `Signal`) into a `Signal` |

`rec` is the everyday way to write feedback; `recursion`/`self_` are the explicit form it desugars to. The phasor above (`S.rec(lambda s: (s + freq / S.sr()) % 1.0)`) is the canonical example — one sample of delay, exactly as in Faust's `~`. Filters are built the same way (a biquad is `rec` plus a `delay1` per tap).

**Sample rate and constants.** These two look alike but resolve differently, and the distinction matters:

| Callable | What it is |
| --- | --- |
| `sr()` | the engine's sample rate, a **foreign constant** resolved at def-compile time (Faust's `ma.SR`, with its `[1, 192000]` clamp) |
| `PI`, `TAU` | plain Python float **literals** (`TAU = 2*PI`) |
| `fconst(ctype, name, file="")` | a foreign scalar resolved once at compile time — the building block of `sr` |
| `fvar(ctype, name, file="")` | like `fconst` but re-read each block |

Use `sr()` — never a baked-in `SR` constant — wherever the maths depends on the rate (`freq / S.sr()`, filter coefficients): the def then stays in tune at whatever rate the live engine or the NRT renderer runs. `PI` / `TAU` are literals because they are literals in Faust too, so they involve no server round-trip and become constant signals as soon as they meet a `Signal`.

**Integer vs real constants — `2` is not `2.0`.** Faust distinguishes an *integer* constant from a *real* one, and so does this graph. JSON has only one "number" type, so it is tempting to assume `2` and `2.0` are interchangeable, but they are **not**: the distinction rides on the literal's form, which survives the whole way through. The client takes the constant straight from your Python value (it does not coerce it), so a Python `int` serializes as `2` and a `float` as `2.0`; the server then reads an integral token as an **integer** constant and a token with a decimal point as a **real** one — a `2.0` is stored as a float and is *not* folded back to an integer. This matters wherever the operation is integer-typed — the bitwise and shift ops (`& | ^ << >>`), `rem`, `as_int()`, table indices — while ordinary `+ - * /` promote an int constant to real and so are unaffected. When it matters, write the literal the way you want it read: `2` for an integer, `2.0` for a real. (A `SynthDef` offers no such choice — it coerces every constant to `float`, since UGens compute only in f32.)

**Controls.** A control callable's **label becomes the control name** — the parameter you later set with `/s_new` / `/n_set`:

| Callable | Control kind |
| --- | --- |
| `hslider(label, init, lo, hi, step)` | horizontal slider |
| `vslider(label, init, lo, hi, step)` | vertical slider |
| `nentry(label, init, lo, hi, step)` | number entry |
| `button(label)` | momentary button |
| `checkbox(label)` | toggle |

**Tables:**

| Callable | Builds |
| --- | --- |
| `waveform(values)` | a constant table from a list of floats |
| `rdtable(size, init, ridx)` | a read-only table |
| `rwtable(size, init, widx, wsig, ridx)` | a read/write table |

### Controls and reserved ports

`fdef.control_names()` lists the control names the def declares, in tree order — the UI labels, deduplicated. On top of those, every Faust synth also accepts the two **reserved** bus-selecting controls the server adds (`fdef.reserved == ("out", "in")`): set them at `/s_new` time (`"in" bus "out" bus`) to choose the input and output buses. They are not declared in the graph.

`fdef.dump_def()` returns the wire payload (JSON for a signal/box tree, the string itself for a source).

## SynthDef

### Building one

A `SynthDef` takes a name and one or more **output UGens** — the things that actually write to a bus. A def with no output UGen is silent on the server.

```python
from clausters.defs import SynthDef, control, sin_osc, out

freq = control("freq", 440.0)
amp = control("amp", 0.2)
sig = sin_osc(freq) * amp
sdef = SynthDef("beep", out(0.0, sig), out(1.0, sig))   # two outputs -> stereo
```

`control(name, default)` declares a parameter; the def gathers every control its graph references, in first-seen order. Reusing the same name with a **conflicting definition** — a different default, type or lag — is an error, caught when the spec is built.

### Control types and rates

A control can carry a **type** (and, for a smoothed control, a lag time), mirroring the server's control types. `control` takes them as keyword arguments:

| Argument | Meaning |
| --- | --- |
| `control(name, default)` | a plain `kr` control: one value per block, read every block (the default) |
| `control(name, default, rate="tr")` | a **trigger**: a `/n_set` holds for one block, then the server resets it to `0` — each set re-fires an `env_gen` gate, a sample-and-hold, a `Trig` |
| `control(name, default, rate="ir")` | a **scalar**: read once at synth init and frozen; a later `/n_set` is ignored. As an `ir` value it may feed an `ir` input (`rand`, buffer-info UGens) |
| `control(name, default, lag=t)` | a lagged `kr` control: changes are smoothed by an implicit one-pole `Lag` over `t` seconds (the server inserts the UGen) |
| `control(name, default, lag=up, lag_down=down)` | separate rise (`up`) and fall (`down`) smoothing times (`VarLag`) |

An unknown `rate`, or a `lag_down` without a `lag`, raises a `ValueError` at build. Audio-rate controls are not a control *type* here — read an audio signal off a bus with `in_` / `in_ctl` and map it with `/n_mapa`.

Each **UGen output** also carries a calculation **rate** — `ir` (init), `kr` (control), `ar` (audio), `dr` (demand). It defaults per kind (signal UGens are `ar`, `rand` / `sample_rate` are `ir`, the demand sources are `dr`); set it explicitly with `Ugen.at_rate`, e.g. `sin_osc(5.0).at_rate("kr")` for a control-rate LFO. The full rate model and its coercion rules live in the [Clausters server book](https://clausters.readthedocs.io/) (schemas / OSC reference).

### The UGen set

`clausters.defs.ugens` exposes the UGen callables the client implements today — a subset of the server's registry. The set is small for now:

| Group | Callable | Does |
| --- | --- | --- |
| Sources | `sin_osc(freq=440.0)` | sine by f64 phase accumulation, starting at phase 0 |
| | `impulse(freq=1.0)` | one-sample `1.0` every `freq` Hz (`freq` 0 = one impulse then silence) |
| | `white_noise()` | uniform white noise in ±1 |
| Bus input | `in_(bus=0.0)` | reads an audio bus (sampled per block) |
| | `in_ctl(bus=0.0)` | reads a control bus (constant over the block) |
| Bus output | `out(bus, signal)` | **sums** `signal` into an audio bus |
| | `replace_out(bus, signal)` | **overwrites** an audio bus instead of summing |
| Buffers | `play_buf(bufnum, chan=0.0, rate=1.0, loop=0.0)` | mono buffer player, linear interpolation; `rate` in frames per output sample |
| | `buf_rd(bufnum, chan, phase, loop=0.0)` | reads a buffer at a `phase` signal in frames |
| Feedback | `local_in(channel=0.0)` | reads a synth-private feedback channel |
| | `local_out(channel, signal)` | writes a feedback channel, and passes `signal` through |
| Smoothers | `lag(signal, time=0.1)` | one-pole smoother (symmetric); the same UGen a lagged control inserts, usable on any signal |
| | `var_lag(signal, up=0.1, down=0.1)` | one-pole smoother with separate rise / fall times |
| Scalar (`ir`) | `sample_rate()` | the engine sample rate in Hz, computed once at init |
| | `rand(lo=0.0, hi=1.0)` | one uniform random value in `[lo, hi)`, drawn once at init and held for the node's life |
| Demand (`dr`) | `dseq(values, repeats=0.0)` | a demand-rate sequence source (`repeats` 0 loops forever); only valid as a `demand` source |
| | `demand(trig, reset, source)` | pulls the next value from a demand `source` on each rising edge of `trig`, holding it between triggers |
| Fused | `mul_add(a, b, c)` | `a*b + c` in one UGen (the multiply-accumulate the server fuses) |
| | `sum3(a, b, c)` / `sum4(a, b, c, d)` | three / four-operand sums in one UGen |

Like Faust synths, a SynthDef also accepts the reserved `in` / `out` bus-selecting controls the server adds at `/s_new` time.

### Maths on a UGen graph

Beyond the four arithmetic operators, the **full unary/binary maths** works on a SynthDef graph: `%`, `min`/`max`, the comparisons, `.sin()`, `.midicps()`, `.distort()`, `.clip2(x)` and the rest compose the server's generic **`BinaryOpUGen`/`UnaryOpUGen`**, each carrying the operator by name. It is the same operation set the value side (`clausters.base.builtins`) computes — one shared `clausters-core` implementation — so a value you compute ahead of time and the UGen on the audio thread agree **bit-for-bit** for the native ops.

```python
from clausters.defs import SynthDef, control, sin_osc, out

note = control("note", 60.0)
freq = note.midicps()                     # UnaryOpUGen (midicps)
sig = sin_osc(freq).distort() * 0.3       # UnaryOpUGen (distort), then Mul
lfo = (sin_osc(5.0) * 0.5 + 0.5).clip2(0.8)  # % min max > .fold/.clip … all compose
sdef = SynthDef("lead", out(0.0, sig * lfo))
```

The `+ - * /` operators keep their dedicated `Add`/`Sub`/`Mul`/`Div` kinds (so existing defs are byte-identical); every other operator becomes an op UGen. Operators and math methods come from the shared `AbstractObject`, so the *same* expression composes a Faust graph, a UGen graph, or concrete numbers depending on what it is applied to. Only a selector with no server op raises `TypeError`. A `FaustDef` is still the tool for genuinely custom per-sample DSP (recursion, tables, sample-accurate feedback); a SynthDef now covers ordinary maths as well as wiring.

Feedback within a block uses the `LocalIn` / `LocalOut` pair. `LocalIn` must be emitted before its `LocalOut`; the topological walk guarantees that as long as the output graph reaches the `local_in` before the `local_out`. Make the `local_out` one of the def's outputs so its write stays in the graph:

```python
from clausters.defs import SynthDef, sin_osc, local_in, local_out, out

fb = local_in(0.0)                            # private channel 0
sig = sin_osc(440.0) * 0.2 + fb * 0.5
echo = local_out(0.0, sig)                    # writes channel 0, passes sig through
sdef = SynthDef("fb", out(0.0, sig), echo)    # echo as an output keeps the write
```

### Spec, dump and controls

The graph is serialized by a plain post-order walk of the output UGens, so a UGen is always emitted after its inputs (the `ugens` list is topologically ordered) and a shared sub-graph is emitted once (deduplicated by object identity).

- `sdef.spec()` — the `{"name", "controls", "ugens"}` dict the server compiles.
- `sdef.dump_def()` — that spec as JSON text, the `/d_recv` payload.
- `sdef.control_names()` — the control names in spec order (parallels `FaustDef.control_names()`).

## Inspecting the built graph

Before sending a def you can look at exactly what you composed: both `FaustDef` and `SynthDef` expose `dump_def()`, the JSON string that goes on the wire (the `/d_faust` / `/d_recv` argument). Printing it shows the resulting graph — handy to confirm a tree built the way you intended.

```python
import json

print(sdef.dump_def())                                   # the raw wire string
print(json.dumps(sdef.spec(), indent=2))                 # SynthDef: spec() is already a dict — pretty-print it
print(json.dumps(json.loads(fdef.dump_def()), indent=2)) # FaustDef signal/box tree, pretty-printed
```

`SynthDef.spec()` returns the `{"name", "controls", "ugens"}` dict directly, so it is the most convenient handle for a SynthDef. For a `FaustDef` built `from_signals` / `from_box`, `dump_def()` is JSON you can `json.loads`; one built `from_source` returns the Faust source string verbatim, not JSON. Either way, `control_names()` gives just the declared control names when that is all you need.

## Choosing between the two

| | FaustDef | SynthDef |
| --- | --- | --- |
| Sent with | `/d_faust` | `/d_recv` |
| Built from | `clausters.defs.signals` | `clausters.defs.ugens` |
| Maths | full (trig, `exp`/`log`, comparisons, tables) | `+ - * /` for now (the rest not yet implemented) |
| Feedback | `rec` / `self_` (one sample) | `local_in` / `local_out` |
| Best for | oscillators, filters, any DSP | routing, mixing, buffer playback, bus I/O |

A common pattern is to combine them: a FaustDef for the voice, a SynthDef to route or play back buffers. Both run as ordinary nodes in the same tree.

## Sending a def

Sending a def is **asynchronous**: `/d_faust` JIT-compiles on the server's network thread, answered later by `/done` or `/fail`. The `Server` mirrors scsynth:

```python
server.add_faustdef(fdef)                 # RT: BLOCKS until /done (raises CommandError on /fail, ReplyTimeout on silence)
server.add_synthdef(sdef, wait=False)     # fire-and-forget: only sends
server.sync()                             # barrier: /sync -> /synced, waits for ALL earlier async work
server.synth("fsine", {"freq": 330.0})    # safe now — the def is installed
```

- `wait=True` (the default) blocks on `/done`; `wait=False` only sends, after which `sync()` is the barrier before the `/s_new` that needs the def.
- In **NRT** (a score interface) `add_*` always *scores* the def at time 0 — the renderer compiles it before time advances — so `wait` does not apply.
- `server.free_def(*names)` removes defs (`/d_free`).

The same shape applies to `add_synthdef`. There is one rule that overrides the convenience of the blocking default: **inside a routine, never block the clock thread.** Send the def `wait=False` and `yield` enough beats before the dependent `/s_new`, rather than calling a blocking `add_*` or `sync()`. See [Routines and clocks](routines-and-clocks.md) for why, and [Getting started](getting-started.md) and the [Examples](examples.md) for end-to-end defs that play.
