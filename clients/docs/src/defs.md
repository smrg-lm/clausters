# Defining instruments: FaustDef and SynthDef

An instrument is a **def** — a named processing graph the server compiles once and then instantiates many times as nodes. The client builds two kinds, both living in {{alt `clausters.defs` || `clausters/defs`}}, and they are **peers**: neither is the "real" one, and a script routinely uses both.

- **`FaustDef`** — a Faust definition, sent with `/def_send faust` and JIT-compiled by the server into a node. Its graph is the full Faust language, so it reaches **below the unit**: per-sample recursion, tables, foreign constants, and the whole Faust library ecosystem. Build it three ways — the **Signal API** (`signals`), the **Box API** (`boxes`), or **Faust source** — all equal citizens (see [Building one](#building-one)). Like the SynthDef family, it works out of the box: {{alt the wheel bundles Faust || the package bundles `libfaust-wasm`, which compiles in a tab}} (see [What the server must support](#what-the-server-must-support)).
- **`SynthDef`** — a UGen graph, sent with `/def_send synth`. It wires the server's UGens (oscillator, noise, impulse, bus I/O, buffer playback, feedback, FFT chains) and the full unary/binary **maths** — {{alt the arithmetic operators plus `%`, `min`/`max`, comparisons, `.sin()`, `.midicps()`, `.distort()` … || the operator methods (`.add()`, `.mul()`, `.mod()`, `.min()`/`.max()`, comparisons, `.sin()`, `.midicps()`, `.distort()` …)}} — which compose the generic operator UGens (see [Maths on a UGen graph](#maths-on-a-ugen-graph)). The graph is an **assembly of ready-made units**: it composes what the server already implements, needs no JIT, and works on **any** server build, since the SynthDef family is on by default.

Rule of thumb, not a hierarchy: reach for a `SynthDef` when the units you need already exist (and always for routing, buses and buffer playback); reach for a `FaustDef` when you want DSP the unit set does not cover, or want to write the DSP itself. Both run as ordinary nodes in the same tree and address each other through buses.

Both are built the same way: **lowercase callables** that compose {{alt with ordinary Python operators || by method (TypeScript has no operator overloading)}} into a JSON tree. Both are **instance-based** — there is no global "current graph" as in sclang, so the tree *is* the composed objects and several defs build concurrently. And both are sent **asynchronously**, behind the `/server_sync` barrier (see [Sending a def](#sending-a-def)).

This page is the conceptual map and the catalog of what each callable does. The exact wire format — the JSON node shapes, the UGen registry, the `/def_send faust` / `/def_send synth` signatures and their `/done` / `/fail` replies — is specified in the **[Clausters server book](https://clausters.readthedocs.io/)** (the schemas / OSC reference chapter); this client is one consumer of it. The generated {{alt [API reference](api/clausters.defs.md) || [API reference](api/index.md)}} carries the per-symbol signatures.

## The shared shape

Every graph node is an `AbstractObject`, the {{alt operator-overloading || operator}} base shared with the value layer. Composing two nodes — or a node and a plain number — returns a new node rather than a computed value:

```python
from clausters.defs import signals as sig

freq = sig.hslider("freq", 220.0, 20.0, 20000.0, 0.01)   # a Signal (a control)
detuned = freq * 1.5                                    # another Signal, not a float
```

```ts
import { signals as sig } from "clausters/defs";

const freq = sig.hslider("freq", 220.0, 20.0, 20000.0, 0.01);   // a Signal (a control)
const detuned = freq.mul(1.5);                                 // another Signal, not a number
```

A plain number that meets a node becomes a **constant** in the graph. The operators map on both def kinds: a `FaustDef` accepts the full set below, and a `SynthDef` accepts the same unary/binary maths ({{alt `+ - * /` || `add sub mul div`}} map to dedicated kinds, everything else to the generic operator UGens — see [Maths on a UGen graph](#maths-on-a-ugen-graph)). Only a selector with no server op {{alt raises || throws}} `TypeError`.<!-- web --> Parity between the clients is asserted on the **emitted spec**, never on the source text, which is why the method spelling costs nothing: the tree that goes out is the same tree.<!-- /web -->

## FaustDef

### Building one

Three constructors, one per payload the server's `/def_send faust` accepts. They are **three ways of writing Faust, not a main road and two detours** — the server compiles all three into the same kind of node, and the same def can be expressed in any of them:

| Constructor | Payload | Use it for |
| --- | --- | --- |
| {{alt `FaustDef.from_signals(name, *outputs)` || `FaustDef.fromSignals(name, ...outputs)`}} | a **signal tree** built with `signals` | graphs assembled one output at a time from arithmetic, feedback and controls (see [the signal API](#the-signal-api)) |
| {{alt `FaustDef.from_box(name, box)` || `FaustDef.fromBox(name, box)`}} | a **box tree** — a `Box` built with `boxes`, or a raw {{alt dict || object}} | point-free graphs of composed processors, with the Faust **libraries** available as boxes (see [the box API](#the-box-api-reusing-the-faust-libraries)); the raw form for machine-generated trees |
| {{alt `FaustDef.from_source(name, src)` || `FaustDef.fromSource(name, src)`}} | a Faust **source** string | writing Faust directly — the whole language, `import`s included |

**From signals.** Each argument to `{{api from_signals}}` is one **output** (a `Signal` or a number): one argument is mono, two is stereo, and so on.

```python
from clausters.defs import signals as sig, FaustDef

freq = sig.hslider("freq", 220.0, 20.0, 20000.0, 0.01)
phase = sig.rec(lambda s: (s + freq / sig.sr()) % 1.0)   # one-sample feedback phasor
sine = sig.sin(phase * sig.TAU) * 0.2
fdef = FaustDef.from_signals("fsine", sine)          # one output -> mono
```

```ts
import { FaustDef, signals as sig } from "clausters/defs";

const freq = sig.hslider("freq", 220.0, 20.0, 20000.0, 0.01);
const phase = sig.rec((s) => s.add(freq.div(sig.sr())).mod(1.0));   // one-sample feedback phasor
const sine = sig.sin(phase.mul(sig.TAU)).mul(0.2);
const fdef = FaustDef.fromSignals("fsine", sine);                // one output -> mono
```

**From boxes.** The same def as composed processors, with the Faust libraries in reach (`box.faust` compiles any Faust expression into a `Box`):

```python
from clausters.defs import boxes as box, FaustDef

freq = box.hslider("freq", 220.0, 20.0, 20000.0, 0.01)
fdef = FaustDef.from_box("bsine", box.faust("os.osc")(freq) * 0.2)
```

```ts
import { FaustDef, boxes as box } from "clausters/defs";

const freq = box.hslider("freq", 220.0, 20.0, 20000.0, 0.01);
const fdef = FaustDef.fromBox("bsine", box.faust("os.osc").call(freq).mul(0.2));
```

**From source.** When you would rather write the Faust itself:

```python
FaustDef.from_source("organ", 'import("stdfaust.lib"); process = os.osc(220) * 0.2;')
```

```ts
FaustDef.fromSource("organ", 'import("stdfaust.lib"); process = os.osc(220) * 0.2;');
```

Which one to pick is a question of *how you are thinking about the graph* — one output at a time, blocks plugged into blocks, or the language — and is discussed under [Choosing a form](#choosing-a-form). Whichever you use, the def is sent, instantiated and controlled identically.

### The signal API

`signals` is the Signal API as lowercase callables. {{alt Arithmetic, comparison and bitwise **operators** compose nodes directly; the rest are functions or methods. || Arithmetic, comparison and bitwise operations are **methods** of the operator's name; the rest are functions or methods.}}

**Maths via operators and methods.** These compose a `Signal`:

<!-- python -->
| Group | How |
| --- | --- |
| Arithmetic | `+ - * /`, `%` (modulo), `**` (power), unary `-` |
| Comparison | `< <= > >=` — return a 0/1 signal, for gating with `select2` / `select3` |
| Bitwise | `& \| ^ << >>` |
| Unary methods | `.abs() .floor() .ceil() .sin() .cos() .tan() .asin() .acos() .atan() .exp() .log() .log10() .sqrt() .asinteger() .asfloat()` |
| Binary methods | `.min(b) .max(b) .atan2(b) .pow(b) .mod(b)` |
<!-- /python -->
<!-- web -->
| Group | How |
| --- | --- |
| Arithmetic | `.add(b) .sub(b) .mul(b) .div(b)`, `.mod(b)` (modulo), `.pow(b)`, `.neg()` |
| Comparison | `.lt(b) .le(b) .gt(b) .ge(b)` — return a 0/1 signal, for gating with `select2` / `select3` |
| Bitwise | `.bitand(b) .bitor(b) .bitxor(b) .lshift(b) .rshift(b)` |
| Unary methods | `.abs() .floor() .ceil() .sin() .cos() .tan() .asin() .acos() .atan() .exp() .log() .log10() .sqrt() .asinteger() .asfloat()` |
| Binary methods | `.min(b) .max(b) .atan2(b) .pow(b) .mod(b)` |
<!-- /web -->

The same unary/binary maths is also available as **module functions** (so you can write `sig.sin(x)` as well as `x.sin()`), plus a few that have no method form:

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
| `rec(fn)` | feedback sugar: `fn(s)` builds the body from its own delayed output `s` |
| `select2(sel, a, b)` | picks `a` or `b` by a 0/1 selector |
| `select3(sel, a, b, c)` | picks `a`, `b` or `c` by a 0/1/2 selector |
| `signal(x)` | coerces a number (or `Signal`) into a `Signal` |

`rec` is the everyday way to write feedback; `recursion`/`self_` are the explicit form it desugars to. The phasor above is the canonical example — one sample of delay, exactly as in Faust's `~`. Filters are built the same way (a biquad is `rec` plus a `delay1` per tap).

**Sample rate and constants.** These two look alike but resolve differently, and the distinction matters:

| Callable | What it is |
| --- | --- |
| `sr()` | the engine's sample rate, a **foreign constant** resolved at def-compile time (Faust's `ma.SR`, with its `[1, 192000]` clamp) |
| `PI`, `TAU` | plain number **literals** (`TAU = 2*PI`) |
| `fconst(ctype, name, file="")` | a foreign scalar resolved once at compile time — the building block of `sr` |
| `fvar(ctype, name, file="")` | like `fconst` but re-read each block |

Use `sr()` — never a baked-in `SR` constant — wherever the maths depends on the rate ({{alt `freq / sig.sr()` || `freq.div(sig.sr())`}}, filter coefficients): the def then stays in tune at whatever rate the live engine or the NRT renderer runs. `PI` / `TAU` are literals because they are literals in Faust too, so they involve no server round-trip and become constant signals as soon as they meet a `Signal`.

<!-- python -->
**Integer vs real constants — `2` is not `2.0`.** Faust distinguishes an *integer* constant from a *real* one, and so does this graph. JSON has only one "number" type, so it is tempting to assume `2` and `2.0` are interchangeable, but they are **not**: the distinction rides on the literal's form, which survives the whole way through. The client takes the constant straight from your Python value (it does not coerce it), so a Python `int` serializes as `2` and a `float` as `2.0`; the server then reads an integral token as an **integer** constant and a token with a decimal point as a **real** one — a `2.0` is stored as a float and is *not* folded back to an integer. This matters wherever the operation is integer-typed — the bitwise and shift ops (`& | ^ << >>`), `rem`, `asinteger()`, table indices — while ordinary `+ - * /` promote an int constant to real and so are unaffected. When it matters, write the literal the way you want it read: `2` for an integer, `2.0` for a real. (A `SynthDef` offers no such choice — it coerces every constant to `float`, since UGens compute only in f32.)
<!-- /python -->
<!-- web -->
**Integer vs real constants.** Faust distinguishes an *integer* constant from a *real* one, and so does this graph: the server reads an integral token as an **integer** constant and a token with a decimal point as a **real** one. A JavaScript number cannot say whether it was written `2` or `2.0`, so an integer-typed operation — the bitwise and shift ops, `rem`, `asinteger()`, table indices — takes an integral number as an integer constant, and `asfloat()` says a real where one is meant. Ordinary arithmetic promotes an integer constant to real and is unaffected. (A `SynthDef` offers no such choice — it coerces every constant to `float`, since UGens compute only in f32.)
<!-- /web -->

**Controls.** A control callable's **label becomes the control name** — the parameter you later set with `/synth_new` / `/node_set`:

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
| `waveform(values)` | a constant table from a list of numbers |
| `rdtable(size, init, ridx)` | a read-only table |
| `rwtable(size, init, widx, wsig, ridx)` | a read/write table |

### The box API: reusing the Faust libraries

`boxes` is the same lowercase-callable pattern over Faust's **Box API**: the point-free algebra where whole **processors** compose by their input/output arities (`seq` / `par` / `split` / `merge` / `rec`). It is the counterpart of `signals` and a complete def-building API in its own right — where `signals` describes one output at a time referentially (`input(n)`), boxes describe multi-channel blocks that plug into each other, the natural shape for routing, chains, and anything conceived as units with inputs and outputs.

On top of the algebra, one addition opens the Faust libraries to it:

```python
from clausters.defs import boxes as box, FaustDef

lp = box.faust("fi.lowpass", 3)          # any Faust expression, stdlib included
```

```ts
import { FaustDef, boxes as box } from "clausters/defs";

const lp = box.faust("fi.lowpass", 3);   // any Faust expression, stdlib included
```

`box.faust` compiles a Faust expression into a `Box` that is indistinguishable from a primitive — so the Faust libraries (`os.`, `fi.`, `re.`, `pm.`, ...) join the same algebra **without transcribing them**, composed with sliders and arithmetic built in {{alt Python || TypeScript}}.

**Two stages of application, kept apart in the syntax.** Faust applies in two different stages, and the module never mixes them in one argument list:

- Arguments to {{alt `box.faust(src, *eval_args)` || `box.faust(src, ...evalArgs)`}} are **evaluation-stage**: spliced into the source text as Faust application. `box.faust("fi.lowpass", 3)` compiles `fi.lowpass(3)`. Structural parameters — a filter order, a table size, a list of coefficients — must live here; they cannot travel as signals. Formatting: numbers as literals, a list as a Faust list `(a, b, c)`, a string verbatim (to pass an expression or a library function as an argument). {{alt `defs=` || A `defs` option}} prepends helper definitions to the generated program.
- Arguments to **{{alt calling a `Box` || a box's `.call(...)`}}** are **composition-stage**: boxes wired to the box's signal inputs, sugar for `seq(par(args), box)`. The call must cover *all* the inputs — use `box.wire()` for the ones left open:

```python
cutoff = box.hslider("cutoff", 800.0, 20.0, 8000.0, 0.1)
y = box.faust("fi.lowpass", 3)(cutoff, box.wire())    # fi.lowpass(3, cutoff, _)
```

```ts
const cutoff = box.hslider("cutoff", 800.0, 20.0, 8000.0, 0.1);
const y = box.faust("fi.lowpass", 3).call(cutoff, box.wire());   // fi.lowpass(3, cutoff, _)
```

<!-- web -->
Applying a box is `.call(a, b)` where Python calls the box itself, and selecting one output channel is `st.get(0)` where Python writes `st[0]` — a class instance is neither callable nor indexable in TypeScript. The emitted tree is the same tree, which is what the parity vectors compare.
<!-- /web -->

Which inputs survive the partial application (here: `fi.lowpass(3)` expects `(fc, x)`) is the library function's business — read its documentation; a mismatch is reported by the Faust compiler itself, verbatim, in the `/fail` reply.

**The wire rule** — the big difference from `signals`: there is no `input(n)` here; **each `box.wire()` is a distinct input**. Two wires in two positions are two bus channels. Reusing the *same* wire object twice is almost always a mistake, and `{{api from_box}}` rejects it with an error; route explicitly with `box.split`, or write that stretch inside the fragment (`"_ <: ..."`). Every *other* box value reuses freely — a repeated subexpression (an oscillator used by two branches) is **computed once**; the server shares identical subtrees.

**Channel selection.** The JSON carries no arities; the client computes them from the composition rules, except for fragments — only the Faust compiler knows those, so declare {{alt `outs=` (and `ins=` if useful) || `outs` (and `ins` if useful) in the options}} when you need to split channels:

```python
st = box.faust("re.stereo_freeverb", 0.8, 0.7, 0.5, 23, outs=2)(dry, dry)
left, right = st.outs()                               # or st[0], st[1]
```

```ts
const st = box.faust("re.stereo_freeverb", 0.8, 0.7, 0.5, 23, { outs: 2 }).call(dry, dry);
const [left, right] = st.outs();                      // or st.get(0), st.get(1)
```

The composition surface mirrors the server's box schema one to one: `seq` / `par` / `split` / `merge` (n-ary, folded left), `rec(a, b)` (point-free `~`; for the `rec(fn)` style use `signals` or a fragment), `wire` / `cut`, `delay` / `delay1`, `select2` / `select3`, the same controls, groups (`hgroup` / `vgroup`), foreign values (`fconst` / `fvar` / `sr()`) and tables (`waveform` / `rdtable` / `rwtable`) as the signal API, and the same operators on `Box` (with {{alt `%` || `mod`}} mapping to Faust's `fmod`; the box schema has no shifts). See the {{alt [API reference](api/clausters.defs.boxes.md) || [API reference](api/defs.Namespace.boxes.md)}} for signatures.

### Choosing a form

The three forms are interchangeable on the wire, so the choice is about which one *says what you mean* — and they mix freely inside one script (and even inside one def, since `box.faust` embeds source into a box tree).

| Write it as | When the graph is |
| --- | --- |
| **source** (`{{api from_source}}`) | a **fixed chain** read top to bottom (`os.osc(freq) : fi.lowpass(3, 800) <: re.stereo_freeverb(...)`), or a **regular bank** — "N copies with index-dependent parameters", which Faust iterates at compile time (`par(i, N, ...)`, labels with `%i`, `ba.take(i+1, list)`). Parametrize it from {{alt Python with an f-string || TypeScript with a template string}} that splices `N` and the lists. |
| **boxes** (`{{api from_box}}`) | **conceived as composed processors** — point-free routing built in {{alt Python || code}}; its **structure decided by data** (a different shape per element, conditionals over analysis or configuration); **library DSP mixed with pieces built in code** (the `box.faust` fragments); or **machine-generated** (the raw form). |
| **signals** (`{{api from_signals}}`) | described **one output at a time**, referentially — arithmetic, comparisons, feedback (`rec`) and tables written as expressions over named values, the shape of a filter or an oscillator you are deriving rather than wiring. |

Two graphs — a fixed chain belongs in source; a data-driven bank belongs in boxes:

```python
FaustDef.from_source("soft", """
import("stdfaust.lib");
freq = hslider("freq",220,20,2000,.1);
process = os.osc(freq) * 0.2
  : fi.lowpass(3, 800)
  <: re.stereo_freeverb(.8,.7,.5,23);
""")

def voice(f, kind):
    src = "os.osc" if kind == "sine" else "os.sawtooth"
    return box.faust(src)(f) * 0.2

mix = sum(voice(f, k) for f, k in spec)
fdef = FaustDef.from_box("bank", mix)
```

```ts
FaustDef.fromSource("soft", `
import("stdfaust.lib");
freq = hslider("freq",220,20,2000,.1);
process = os.osc(freq) * 0.2
  : fi.lowpass(3, 800)
  <: re.stereo_freeverb(.8,.7,.5,23);
`);

function voice(f: Box, kind: string): Box {
    const src = kind === "sine" ? "os.osc" : "os.sawtooth";
    return box.faust(src).call(f).mul(0.2);
}

const mix = spec.map(([f, k]) => voice(f, k)).reduce((a, b) => a.add(b));
const fdef = FaustDef.fromBox("bank", mix);
```

### Controls and reserved ports

`fdef.{{api control_names}}()` lists the control names the def declares, in tree order — the UI labels, deduplicated. On top of those, every Faust synth also accepts the two **reserved** bus-selecting controls the server adds ({{alt `fdef.reserved == ("out", "in")` || `FaustDef.reserved`, `["out", "in"]`}}): set them at `/synth_new` time (`"in" bus "out" bus`) to choose the input and output buses. They are not declared in the graph.

`fdef.{{api dump_def}}()` returns the wire payload (JSON for a signal/box tree, the string itself for a source).

## SynthDef

### Building one

A `SynthDef` takes a name and one or more **output UGens** — the things that actually write to a bus. A def with no output UGen is silent on the server.

```python
from clausters.defs import SynthDef, control, sine, out

freq = control("freq", 440.0)
amp = control("amp", 0.2)
sig = sine(freq) * amp
sdef = SynthDef("beep", out(0.0, sig), out(1.0, sig))   # two outputs -> stereo
```

```ts
import { SynthDef, control, out, sine } from "clausters/defs";

const freq = control("freq", 440.0);
const amp = control("amp", 0.2);
const sig = sine(freq).mul(amp);
const sdef = new SynthDef("beep", out(0.0, sig), out(1.0, sig));   // two outputs -> stereo
```

`control(name, default)` declares a parameter; the def gathers every control its graph references, in first-seen order. Reusing the same name with a **conflicting definition** — a different default, type or lag — is an error, caught when the spec is built.

### Control types and rates

A control can carry a **type** (and, for a smoothed control, a lag time), mirroring the server's control types. `control` takes them as {{alt keyword arguments || options}}:

| Argument | Meaning |
| --- | --- |
| `control(name, default)` | a plain `kr` control: one value per block, read every block (the default) |
| {{alt `control(name, default, rate="tr")` || `control(name, default, { rate: "tr" })`}} | a **trigger**: a `/node_set` holds for one block, then the server resets it to `0` — each set re-fires an `{{api env_gen}}` gate, a sample-and-hold, a `Trig` |
| {{alt `control(name, default, rate="ir")` || `control(name, default, { rate: "ir" })`}} | a **scalar**: read once at synth init and frozen; a later `/node_set` is ignored. As an `ir` value it may feed an `ir` input (`rand`, buffer-info UGens) |
| {{alt `control(name, default, lag=t)` || `control(name, default, { lag: t })`}} | a lagged `kr` control: changes are smoothed by an implicit one-pole `Lag` over `t` seconds (the server inserts the UGen) |
| {{alt `control(name, default, lag=up, lag_down=down)` || `control(name, default, { lag: up, lagDown: down })`}} | separate rise (`up`) and fall (`down`) smoothing times (`VarLag`) |

An unknown `rate`, or a `{{api lag_down}}` without a `lag`, {{alt raises a `ValueError` || throws}} at build. Audio-rate controls are not a control *type* here — read an audio signal off a bus with `in_` / `{{api in_ctl}}` and map it with `/node_mapAudio`.

Each **UGen output** also carries a calculation **rate** — `ir` (init), `kr` (control), `ar` (audio), `dr` (demand). It defaults per kind (signal UGens are `ar`, `rand` / `{{api sample_rate}}` are `ir`, the demand sources are `dr`); set it explicitly with `{{api Ugen.at_rate}}`, e.g. {{alt `sine(5.0).at_rate("kr")` || `sine(5.0).atRate("kr")`}} for a control-rate LFO. The full rate model and its coercion rules live in the [Clausters server book](https://clausters.readthedocs.io/) (schemas / OSC reference).

### A widget is built from the control it drives

A def knows two things about a control: its **name** — what `/node_set`
addresses — and its **default**. A widget built from one reads both, so the
widget and the graph cannot disagree about what `"freq"` is, and the handle
addresses the widget by the control's name.

```python
freq = control("freq", 220.0)
sd = SynthDef("voice", out(0.0, sine(freq=freq) * 0.2))

knob(freq, min=110.0, max=880.0)     # name and default from the control
```

```ts
const freq = control("freq", 220.0);
const sd = new SynthDef("voice", out(0.0, sine(freq).mul(0.2)));

knob(freq, { min: 110.0, max: 880.0 });   // name and default from the control
```

**How a control is drawn** is the def's to say, or the widget's. A SynthDef
control may declare it beside its default — a range (`min` and `max`, both or
neither), a `step`, a `warp` (`"exp"` travels by ratios, what a frequency or a
time wants) and a `unit`:

```python
freq = control("freq", 440.0, min=20.0, max=20000.0, warp="exp", unit="Hz")
```

```ts
const freq = control("freq", 440.0, { min: 20.0, max: 20000.0, warp: "exp", unit: "Hz" });
```

The def carries it to the server, which reports it back in `/def_query`, and
both a widget built from the control and a node's generic editor
(`edit(node)`) draw it so. It never limits the control — nothing on the server
holds a `/node_set` to it — and a control that declares none still takes any
number. A **FaustDef**'s parameters always arrive with a range, because
`hslider(label, init, min, max, step)` cannot be written without one, and a
plugin's parameters with the range, the steps and the names the plugin reports.
`min`/`max` are not attributes of the control, since on a signal they are the
binary operators: what a control says lives in its `display`.

All three families answer with the same shape, so a GUI reads any of them alike:

```python
sd["freq"]                  # a ControlInfo: name, default, rate, and what it declared
fd["cutoff"]                # min/max/step, from Faust's own hslider
gd["mix"]                   # a surface port, plus the targets it drives inside
[knob(c, min=0.0, max=1.0) for c in sd.controls]
```

```ts
sd.control("freq");         // a ControlInfo: name, default, rate, and what it declared
fd.control("cutoff");       // min/max/step, from Faust's own hslider
gd.control("mix");          // a surface port, plus the targets it drives inside
sd.controlNames().map((name) => knob(sd.control(name), { min: 0.0, max: 1.0 }));
```

### The UGen set

{{alt `clausters.defs.ugens` exposes || `clausters/defs` exports}} a callable per UGen kind in the server's registry (the arithmetic kinds come from operators rather than callables — see [Maths on a UGen graph](#maths-on-a-ugen-graph)):

<!-- python -->
| Group | Callable | Does |
| --- | --- | --- |
| Sources | `sine(freq=440.0, phase=0.0)` | sine by f64 phase accumulation, starting at phase 0; `phase` is a running offset in cycles |
| | `fsin_osc(freq, iphase)`, `sin_osc_fb(freq, feedback)`, `psin_grain(freq, dur, amp)` | a recurrence sine for fixed tones; a self-modulated sine; one grain that frees its synth |
| | `blip(freq, numharm)`, `formant(fundfreq, formfreq, bwfreq)` | a band-limited impulse train; a formant over a fundamental |
| | `klang(freqs, amps=None, phases=None, freqscale=1, freqoffset=0)`, `klank(signal, freqs, amps=None, times=None, ...)` | banks of fixed sines and of fixed resonators |
| | `impulse(freq=1.0, phase=0.0)` | one-sample `1.0` every `freq` Hz (`freq` 0 = one impulse then silence); `phase` offsets the train in cycles |
| | `white_noise()` | uniform white noise in ±1 |
| | `saw(freq=440.0)` | **band-limited** sawtooth: the harmonics that belong there, not aliases folded down from above Nyquist |
| | `pulse(freq=440.0, width=0.5)` | band-limited pulse; `width` is the duty cycle |
| Modulation | `lf_saw(freq, iphase=0.0)` / `lf_tri(freq, iphase=0.0)` | saw and triangle in ±1, **not** band-limited — cheap shapes to modulate with rather than listen to; `iphase` in cycles |
| | `lf_pulse(freq, iphase=0.0, width=0.5)` | square in `[0, 1]` — a gate, not a bipolar waveform like `pulse` |
| | `var_saw(freq, iphase=0.0, width=0.5)` | triangle whose peak sits at `width` of the cycle: falling ramp → triangle → rising ramp |
| | `lf_cub`, `lf_par(freq, iphase=0.0)` | sine- and cosine-like shapes of cubic and parabolic arcs; not band-limited |
| | `sync_saw(sync_freq, saw_freq)` | a sawtooth hard-synced to a master |
| | `vibrato(freq, rate=6, depth=0.02, delay=0, onset=0, ratevariation=0.04, depthvariation=0.1, iphase=0, trig=0)` | `freq` wobbled by a randomized parabolic sine |
| | `phasor(trig, rate, start, end, reset_pos)` | a ramp in units **per sample**, wrapping at `end`; a rising `trig` jumps to `reset_pos` — an index generator, deliberately not band-limited |
| Filters | `lpf(signal, freq)` / `hpf(signal, freq)` | second-order Butterworth low / highpass: −3 dB at `freq`, −12 dB/octave |
| | `rlpf(signal, freq, rq=None, *, q=None)` / `rhpf(...)` | the resonant pair; give the resonance as `rq` (1/Q, 0 = infinite) **or** as `q` |
| | `bpf(...)` / `resonz(...)` | bandpass with unity gain at the centre — one implementation under scsynth's two names |
| | `brf(...)` | band reject: unity in both passbands, a true null at `freq` |
| | `svf(signal, freq, rq=None, low=1.0, band=0.0, high=0.0)` | the state-variable filter with its three taps as **signal inputs**, so the response itself can be modulated; `svf_morph(pos)` gives the one-knob version |
| | `one_pole(signal, coef)` / `one_zero(signal, coef)` | the one-pole / one-zero primitives; the parameter is the **pole**, not a cutoff |
| | `leak_dc(signal, coef=0.995)` / `integrator(signal, coef=0.999)` | DC blocker / leaky accumulator |
| Delays | `delay_n/l/c(signal, delaytime, *, max_delay=None)` | pure delay — no interpolation, linear, cubic |
| | `comb_n/l/c(signal, delaytime, decaytime, *, max_delay=None)` | a comb: `decaytime` is −60 dB counted from the first echo |
| | `allpass_n/l/c(signal, delaytime, decaytime, *, max_delay=None)` | an allpass: scatters phase and leaves the magnitude alone |
| | `pluck(signal, trig=1.0, maxdelaytime=0.2, delaytime=0.2, decaytime=1.0, coef=0.5)` | a plucked string: a period of `signal` let into a lowpassed feedback line |
| | `pitch_shift(signal, window_size=0.2, pitch_ratio=1.0, pitch_dispersion=0.0, time_dispersion=0.0)` | the pitch moved by four overlapping grains |
| | `del_tap_wr(bufnum, signal)` / `del_tap_rd(bufnum, phase, deltime, interp=1.0)` | a line round a buffer: one writer, any number of taps |
| Reverbs | `free_verb(signal, mix=0.33, room=0.5, damp=0.5)` / `free_verb2(signal, signal2, ...)` | Freeverb, mono and stereo |
| | `gverb(signal, roomsize=10.0, revtime=3.0, ...)` | a room of a size and a decay: a feedback delay network, stereo |
| Physical models | `spring(signal, spring=1.0, damp=0.0)` / `ball(signal, g=1.0, damp=0.0, friction=0.01)` / `tball(...)` | a damped spring; a bouncing ball's height, and its impacts |
| Envelopes | `env_gen(env, gate=1.0, …, done_action=DoneAction.NONE)` | plays an `Env` breakpoint envelope; the `done_action` can free the node |
| | `line(start, end, dur, done_action)` / `x_line(...)` | a single ramp, in equal steps / equal ratios; the same `DoneAction` set |
| | `ienv_gen(env, index, offset=0.0)` | an `Env` read at a position rather than played in time |
| | `linen(gate, attacktime, suslevel, releasetime, done_action)` | the trapezoid of a gate, at control rate |
| | `lf_gauss(duration, width, iphase, loop, done_action)` | a Gaussian bell, repeated or once |
| Noise | `pink_noise()` / `brown_noise()` | −3 dB/octave (quiet by construction, ~0.13 RMS) / −6, a reflecting random walk |
| | `gray_noise()` / `clip_noise()` | one random bit flipped per sample, grainy and low-leaning / ±1 only |
| | `lf_noise0(freq)` / `lf_noise1(freq)` / `lf_noise2(freq)` | a new random value every `1/freq` s: held / ramped / curved (overshoots to ±1.7) |
| | `lf_clip_noise(freq)` | the held one, restricted to ±1 |
| | `lfd_noise0`, `lfd_noise1`, `lfd_noise3`, `lfd_clip_noise(freq=500.0)` | the dynamic forms: a change of rate takes effect at once |
| | `logistic(chaos_param=3, freq=1000, init=0.5)` | the logistic map, iterated and held |
| | `cusp_n`, `henon_c`, `latoocarfian_l`, `lin_cong_n`, `lorenz_l`, `quad_c`, `standard_n`, `fb_sine_l` … | scsynth's chaotic maps, held (`n`), linear (`l`) or cubic (`c`) |
| | `gendy1`, `gendy2`, `gendy3` | Xenakis's dynamic stochastic synthesis |
| | `dust(density)` / `dust2(density)` | random impulses at a **mean** density — exponential gaps, not a clock |
| | `crackle(chaos)` | a deterministic chaotic map; one-sided, so it carries DC |
| Panning | `pan2(signal, pos=0.0, level=1.0)` | places a mono signal between two channels at **equal power**; returns two channels |
| | `lin_pan2(signal, pos, level)` | the same at constant **amplitude** — the two gains sum to `level` |
| | `balance2(left, right, pos, level)` | shifts an already stereo pair to one side; centred, it costs 3 dB |
| | `pan_az(numchans, signal, pos, level=1.0, width=2.0, orientation=0.5)` | places a signal on a **ring** of `numchans`; returns that many channels |
| | `pan4(signal, xpos=0.0, ypos=0.0, level=1.0)` | a signal on a square of four channels |
| | `pan_b`, `pan_b2`, `bipan_b2(a, b, azimuth, level)`, `decode_b2(numchans, w, x, y, orientation=0.5)` | first-order Ambisonics: encode into B-format, decode onto a ring |
| | `splay(signals, spread=1.0, level=1.0, center=0.0)` | spreads a list across the field and folds it to a pair (client-side, no UGen of its own) |
| Stereo field | `rotate2(x, y, pos)` | rotates the plane by `pos` **half turns** — turns an image without resizing it |
| | `mid_side(a, b)` | the mid/side matrix, **its own inverse**: encodes and decodes with one call |
| | `stereo_width(left, right, width)` | widens or narrows: `0` mono, `1` the identity, `2` wide, negative swaps |
| Crossfades | `xfade2(a, b, pan, level)` / `lin_xfade2(...)` | equal-power / constant-amplitude fade between two signals |
| | `select(which, *sources)` / `select_x(which, *sources)` | picks one source by index / crossfades between neighbours; every source still runs |
| Triggers | `trig(signal, dur)` / `trig1(signal, dur)` | holds the level that triggered it / holds 1, for `dur` seconds |
| | `t_delay(signal, dur)` | one sample of 1, `dur` later; a trigger arriving while one is in flight is dropped |
| | `latch(signal, trig)` / `gate(signal, trig)` | one sample per rising edge / transparent while the gate is open |
| | `schmidt(signal, lo, hi)` | comparator with hysteresis — it takes the whole band to change its mind |
| | `changed(signal, threshold)` | 1 where the input moved; compares the **halved** difference (sclang's `HPZ1` gain) |
| Counters | `toggle_ff(trig)` / `set_reset_ff(trig, reset)` | one bit of state: flipped by each trigger / set and reset |
| | `pulse_count(trig, reset)` / `pulse_divider(trig, div, start)` | counts triggers / one out per `div` in, `start` phases it |
| | `stepper(trig, reset, min, max, step, resetval)` | walks `[min, max]`, both ends included, wrapping |
| | `timer(trig)` / `sweep(trig, rate)` | seconds between the last two triggers / a ramp restarted at each |
| Followers | `decay(signal, decaytime)` / `decay2(signal, attacktime, decaytime)` | an impulse into an exponential / the same with a rounded attack |
| | `detect_silence(signal, amp, time, done_action)` | 1 once the input has been quiet for `time`; raises a done flag |
| Node control | `free_self(signal)` / `pause_self(signal)` | frees / pauses the synth while `signal > 0`, passing it through |
| | `done(source)` | 1 once `source` (an envelope or a ramp) has finished — the flag, not its value |
| | `free_self_when_done(source)` | passes `source` through and frees the synth once it finishes |
| | `pause_self_when_done(source)`, `free(trig, id)`, `pause(gate, id)` | pauses the synth once its source finishes; frees, runs or pauses another node by id |
| | `peak`, `running_max`, `running_min(signal, trig)`, `peak_follower`, `running_sum`, `last_value`, `most_change`, `least_change`, `zero_crossing` | running statistics of a signal |
| Bus input | `in_(bus=0.0)` | reads an audio bus (sampled per block) |
| | `in_ctl(bus=0.0)` | reads a control bus (constant over the block) |
| Bus output | `out(bus, signal)` | **sums** `signal` into an audio bus |
| | `replace_out(bus, signal)` | **overwrites** an audio bus instead of summing |
| | `offset_out`, `x_out(bus, xfade, signal)`, `shared_out` | `out` by scsynth's name; a crossfade into a bus; a control bus |
| | `in_feedback(bus)`, `in_trig(bus)`, `lag_in(bus, lag)`, `shared_in(bus)` | a bus a block late when written later; a control bus as a trigger, lagged, shared |
| Buffers | `play_buf(bufnum, chan=0.0, rate=1.0, loop=0.0, trigger=0.0, start_pos=0.0, done_action=0)` | mono buffer player, linear interpolation; `rate` in frames per output sample. A pass begins at `start_pos` and a rising `trigger` re-cues there; without `loop`, the end fires `done_action` (2 frees the synth) |
| | `buf_rd(bufnum, chan, phase, loop=0.0, interpolation=2.0)` | reads a buffer at a `phase` signal in frames; `interpolation` 1 none, 2 linear, 4 cubic |
| Buffer info | `buf_sample_rate(bufnum)` / `buf_frames(bufnum)` | the buffer's own sample rate (Hz) / frame count, block-constant (`kr`) |
| | `buf_rate_scale(bufnum)` | `file_sr / server_sr` — feed `play_buf`'s `rate` to play at the file's true pitch |
| | `buf_channels(bufnum)` / `buf_dur(bufnum)` | channel count / duration in seconds, block-constant (`kr`) |
| Tables | `osc(bufnum, freq=440.0, phase=0.0)` | interpolating wavetable oscillator over a **wavetable-format** buffer (`/buffer_gen`); `phase` in radians |
| | `oscn(bufnum, freq=440.0, phase=0.0)` | non-interpolating oscillator over a **plain** buffer; rawer/cheaper |
| | `vosc(bufpos, freq=440.0, phase=0.0)` | `osc` with the buffer number as a signal: crossfades tables `bufpos`/`bufpos+1`, so sweeping it morphs a contiguous bank |
| | `vosc3(bufpos, freq1, freq2, freq3)`, `cosc(bufnum, freq, beats)` | three voices of one morphing table; two detuned voices of one table |
| | `index`, `index_l`, `wrap_index`, `fold_index(bufnum, signal)` | a buffer as a table, read at an index |
| | `index_in_between`, `detect_index(bufnum, signal)`, `degree_to_key(bufnum, signal, octave=12)` | a table searched for a value; a scale degree to a key |
| | `shaper(bufnum, signal)` | waveshaper: maps `signal` (±1) through a `cheby` transfer table |
| Synth buffers | `local_buf(frames=1, channels=1)` | a buffer of the synth's own, numbered after the pool, freed with the synth |
| | `set_buf(bufnum, values, offset=0.0)` / `clear_buf(bufnum)` / `max_local_bufs(count)` | fill or zero a buffer at the synth's start; scsynth's count, which does nothing |
| | `scope_out(signal, bufnum=0.0)` | writes `signal` round a buffer, a channel each |
| Disk | `disk_in(chan=0.0, *, path, loop=False)` | streams a file from disk, one channel per UGen, no resampling |
| | `vdisk_in(chan=0.0, rate=1.0, *, path, loop=False)` | `disk_in` at a variable rate, cubic interpolation |
| | `disk_out(signal, *, path, format="int16")` | streams `signal` to a mono WAV on disk; passes `signal` through |
| Feedback | `local_in(channel=0.0)` | reads a synth-private feedback channel |
| | `local_out(channel, signal)` | writes a feedback channel, and passes `signal` through |
| Smoothers | `lag(signal, time=0.1)` | one-pole smoother (symmetric); the same UGen a lagged control inserts, usable on any signal |
| | `lag_ud(signal, up=0.1, down=0.1)` | one-pole smoother with separate rise / fall times |
| | `lag2`, `lag3`, `lag2_ud`, `lag3_ud`, `ramp(signal, time)`, `slew(signal, up, down)` | cascaded smoothers, a segmented line, a rate limiter |
| | `lpz1`, `hpz1`, `lpz2`, `hpz2`, `bpz2`, `brz2`, `slope`, `flip`, `delay1`, `delay2` | scsynth's fixed FIRs and one- and two-sample delays |
| | `two_pole`, `two_zero`, `apf`, `ringz`, `formlet`, `mid_eq`, `fos`, `sos` | pole/zero sections and resonators |
| | `b_low_pass`, `b_hi_pass`, `b_all_pass`, `b_band_pass`, `b_band_stop`, `b_peak_eq`, `b_low_shelf`, `b_hi_shelf` | the cookbook biquads (BEQSuite) |
| | `median(length, signal)`, `moog_ff(signal, freq, gain)` | a running median; the Moog ladder |
| | `hilbert(signal)`, `freq_shift(signal, freq, phase)` | the analytic pair; a single-sideband shift |
| | `amplitude`, `compander`, `normalizer`, `limiter` | an envelope follower, a compressor/expander, look-ahead gains |
| | `var_lag(signal, time=0.1, start=None)` | a straight line to each new value, taking `time` seconds, from `start` (the signal itself unless given) |
| Scalar (`ir`) | `sample_rate()` | the engine sample rate in Hz, computed once at init |
| | `block_size()`, `control_rate()`, `control_dur()`, `sample_dur()`, `radians_per_sample()`, `subsample_offset()` | the engine's timing, and where in its first sample the synth's bundle fell |
| | `num_audio_buses()`, `num_control_buses()`, `num_input_buses()`, `num_output_buses()`, `num_buffers()`, `node_id()`, `num_running_synths()`, `buf_samples(bufnum)` | the server's configuration, the node, the synths, a buffer's samples |
| | `rand(lo=0.0, hi=1.0)` | one uniform random value in `[lo, hi)`, drawn once at init and held for the node's life |
| | `irand(lo=0.0, hi=127.0)`, `exp_rand(lo=0.01, hi=1.0)`, `lin_rand(lo, hi, minmax=0.0)`, `n_rand(lo=0.0, hi=1.0, n=0.0)` | one integer, log-uniform, linear or mean-of-`n` value, drawn once and held |
| | `t_rand(lo, hi, trig)`, `ti_rand(...)`, `t_exp_rand(...)` | a random value drawn at the start and again at each trigger |
| | `coin_gate(prob=0.5, trig=0.0)` | lets each trigger through with probability `prob` |
| | `t_windex(trig, weights, normalize=0)` | a weighted choice of an index at each trigger |
| | `rand_id(id=0.0)`, `rand_seed(trig=0.0, seed=56789.0)` | choose the server generator the synth draws from; reseed it on a trigger (a root) |
| Demand (`dr`) | `dseq(values, repeats=0.0)` | yields `values` in order, `repeats` passes (**0 is endless**); a value that is itself a stream is drained, not taken once |
| | `drand(values, repeats=0.0)` / `dxrand(...)` | `repeats` **items** drawn at random from the list; `dxrand` never picks the value it just used |
| | `dshuf(values, repeats=0.0)` | the list shuffled **once** and replayed in that order, `repeats` passes |
| | `dseries(repeats=0.0, start=0.0, step=1.0)` | arithmetic sequence; `step` is read per item, so it may be a stream |
| | `dgeom(repeats=0.0, start=1.0, grow=2.0)` | geometric sequence `start`, `start * grow`, … |
| | `dwhite(repeats=0.0, lo=0.0, hi=1.0)` / `diwhite(...)` | independent uniform draws on `[lo, hi]`; `diwhite` over the integers, both ends included |
| | `dbrown(repeats=0.0, lo=0.0, hi=1.0, step=0.01)` / `dibrown(..., step=1.0)` | a random walk of at most `step` per item, **folded** into `[lo, hi]`; `dibrown` over the integers |
| | `dstutter(repeats, value)` | repeats each item of `value` `repeats` times; the count is pulled per item |
| | `dswitch1(which, *sources)` | takes **one** item from the stream `which` picks, then picks again; the unselected ones do not advance |
| | `dbufrd(bufnum, phase, loop=1.0, channel=0.0)` | reads the buffer frame the `phase` stream names — a `dseries` phase walks a buffer as a step sequence |
| | `demand(trig, reset, source)` | driver: pulls one value per rising edge of `trig`, holding it in between; holds the last value once the stream ends |
| | `duty(dur, reset=0.0, level=1.0, done_action=0)` | driver with a clock of its own: pulls one `level` every `dur` seconds and holds it; `done_action` fires when either stream ends |
| | `tduty(dur, reset=0.0, level=1.0, done_action=0, gap_first=0.0)` | `duty` emitting each level on its own sample and silence in between — a trigger stream with amplitudes |
| Fused | `madd(a, b, c)` | `a*b + c` in one UGen (the multiply-accumulate the server fuses) |
| | `sum3(a, b, c)` / `sum4(a, b, c, d)` | three / four-operand sums in one UGen |
| Maps | `lin_exp(signal, srclo, srchi, dstlo, dsthi)` | linear in, exponential out, unclipped |
| | `in_range(signal, lo, hi)`, `in_rect(x, y, left, top, right, bottom)` | 1 inside, 0 outside |
| | `mod_dif(x, y, mod)`, `unwrap(signal, lo, hi)` | a distance on a circle; a wrap undone |
| | `amp_comp(freq, root, exp)`, `amp_comp_a(freq, root, min_amp, root_amp)` | loudness compensation by a power, or along the A-weighting |
| Rates | `k2a`, `a2k`, `t2a(signal, offset)`, `t2k`, `dc(signal)` | control to audio and back, for signals and triggers; a constant |
| | `hasher(signal)`, `mantissa_mask(signal, bits)` | noise that is a function of its input; precision reduced |
| Measure | `meter(signal, decay=20.0, hold=0.0)` | a level with a **meter's ballistics**: instantaneous attack, a fall of `decay` decibels per second, a new peak held `hold` seconds before it starts falling. One number a block, which is what a control bus carries — pair it with `out_ctl` for a meter a host reads as a bus |
| | `clip_count(signal, ceiling=1.0, run=3)` | **how many times the signal was flattened**, since the pass began: a run of `run` consecutive samples at or over `ceiling` is one over, however long it lasts. A lone sample at full scale is not clipping — the engine is floating point — so the run is what a red lamp is actually reporting; `run=1` is the pessimistic meter. It walks samples rather than blocks, which is what a block peak has already lost. Pair it with `out_ctl` and hand the bus to a `meter` widget's `clip` |
| | `true_peak(signal, decay=20.0, hold=0.0)` | a meter's level over the **reconstructed** signal: `meter`'s ballistics, but the block's true peak (the ITU-R BS.1770-4 filter) rather than its largest sample, so the reading is in dBTP. Pair it with `out_ctl` and a `meter` widget at `rate="control"` with `peak="true"` |
| Side-effect | `send_trig(trig, id, value)` | on each trigger, sends `/node_trigger nodeID id value`; output is silence |
| | `send_reply(trig, *values, cmd="/reply", reply_id=-1)` | sends a custom OSC message with an arbitrary value list |
| | `send_peak_rms(*signals, reply_rate=20, peak_lag=3, cmd, reply_id)` | replies each signal's peak and RMS, a meter's ballistics |
| | `poll(trig, signal, trig_id=-1, *, label="poll")` | posts `signal` to the server console (and a `/node_trigger` when `trig_id >= 0`); passes `signal` through |
| Spectral | `fft(source, active=1.0, *, fft_size=1024, hop=0.5, wintype=0)` | opens a spectral chain (windows and transforms `source` per hop) |
| | `pv_mag_above(chain, threshold)` / `pv_mag_below(chain, threshold)` | pass bins above / below a magnitude threshold |
| | `pv_mag_clip(chain, threshold)` | limit each bin's magnitude to the threshold (phases kept) |
| | `pv_brick_wall(chain, wipe)` | zero a band (`wipe > 0` low pass, `< 0` high pass) |
| | `pv_add(a, b)` / `pv_mul(a, b)` / `pv_min(a, b)` / `pv_max(a, b)` | two-chain combiners: per-bin complex sum / product / smaller / larger magnitude |
| | `pv_mag_mul(a, b)` / `pv_copy_phase(a, b)` | cross-synthesis: A's bins scaled by B's magnitudes / A's magnitudes with B's phases |
| | `pv_mag_freeze(chain, freeze=0)` | hold the stored magnitude envelope while `freeze > 0` (phases keep running) |
| | `pv_mag_smear(chain, bins=0)` | average each bin's magnitude over `bins` neighbors per side (spectral blur) |
| | `pv_bin_shift(chain, stretch=1, shift=0)` / `pv_mag_shift(...)` | remap bin positions (complex bins / magnitude envelope only) |
| | `pv_kernel(chain, *, mag=None, phase=None, params=())` | apply **user-written bin expressions** to every bin of each fresh frame — see [Writing your own spectral operation](#writing-your-own-spectral-operation) |
| | `ifft(chain)` | closes a spectral chain (resynthesises audio by overlap-add) |
| Convolution | `conv(source, kernel, *, fft_size=1024, partitions=16)` | partitioned convolution against a kernel prepared with `dest.gen("prepare_partconv", fft_size, ir_bufnum)` (size `dest` with `partconv_frames`); latency `fft_size / 2` samples |
<!-- /python -->
<!-- web -->
| Group | Callable | Does |
| --- | --- | --- |
| Sources | `sine(freq = 440.0, phase = 0.0)` | sine by f64 phase accumulation, starting at phase 0; `phase` is a running offset in cycles |
| | `fsinOsc(freq, iphase)`, `sinOscFb(freq, feedback)`, `psinGrain(freq, dur, amp)` | a recurrence sine for fixed tones; a self-modulated sine; one grain that frees its synth |
| | `blip(freq, numharm)`, `formant(fundfreq, formfreq, bwfreq)` | a band-limited impulse train; a formant over a fundamental |
| | `klang(freqs, amps = null, phases = null, freqscale = 1, freqoffset = 0)`, `klank(signal, freqs, amps = null, times = null, ...)` | banks of fixed sines and of fixed resonators |
| | `impulse(freq = 1.0, phase = 0.0)` | one-sample `1.0` every `freq` Hz (`freq` 0 = one impulse then silence); `phase` offsets the train in cycles |
| | `whiteNoise()` | uniform white noise in ±1 |
| | `saw(freq = 440.0)` | **band-limited** sawtooth: the harmonics that belong there, not aliases folded down from above Nyquist |
| | `pulse(freq = 440.0, width = 0.5)` | band-limited pulse; `width` is the duty cycle |
| Modulation | `lfSaw(freq = 440.0, iphase = 0.0)` / `lfTri(freq = 440.0, iphase = 0.0)` | saw and triangle in ±1, **not** band-limited — cheap shapes to modulate with rather than listen to; `iphase` in cycles |
| | `lfPulse(freq = 440.0, iphase = 0.0, width = 0.5)` | square in `[0, 1]` — a gate, not a bipolar waveform like `pulse` |
| | `varSaw(freq = 440.0, iphase = 0.0, width = 0.5)` | triangle whose peak sits at `width` of the cycle: falling ramp → triangle → rising ramp |
| | `lfCub`, `lfPar(freq = 440.0, iphase = 0.0)` | sine- and cosine-like shapes of cubic and parabolic arcs; not band-limited |
| | `syncSaw(syncFreq, sawFreq)` | a sawtooth hard-synced to a master |
| | `vibrato(freq, rate = 6, depth = 0.02, delay = 0, onset = 0, ratevariation = 0.04, depthvariation = 0.1, iphase = 0, trig = 0)` | `freq` wobbled by a randomized parabolic sine |
| | `phasor(trig = 0.0, rate = 1.0, start = 0.0, end = 1.0, resetPos = 0.0)` | a ramp in units **per sample**, wrapping at `end`; a rising `trig` jumps to `resetPos` — an index generator, deliberately not band-limited |
| Filters | `lpf(signal, freq = 440.0)` / `hpf(signal, freq = 440.0)` | second-order Butterworth low / highpass: −3 dB at `freq`, −12 dB/octave |
| | `rlpf(signal, freq = 440.0, { rq, q })` / `rhpf(signal, freq = 440.0, { rq, q })` | the resonant pair; give the resonance as `rq` (1/Q, 0 = infinite) **or** as `q` |
| | `bpf(signal, freq = 440.0, { rq, q })` / `resonz(signal, freq = 440.0, { rq, q })` | bandpass with unity gain at the centre — one implementation under scsynth's two names |
| | `brf(signal, freq = 440.0, { rq, q })` | band reject: unity in both passbands, a true null at `freq` |
| | `svf(signal, freq = 440.0, { rq, q }, low = 1.0, band = 0.0, high = 0.0)` | the state-variable filter with its three taps as **signal inputs**, so the response itself can be modulated; `svfMorph(pos)` gives the one-knob version |
| | `onePole(signal, coef = 0.5)` / `oneZero(signal, coef = 0.5)` | the one-pole / one-zero primitives; the parameter is the **pole**, not a cutoff |
| | `leakDc(signal, coef = 0.995)` / `integrator(signal, coef = 0.999)` | DC blocker / leaky accumulator |
| Delays | `delayN/L/C(signal, delaytime = 0.2, maxDelay)` | pure delay — no interpolation, linear, cubic |
| | `combN/L/C(signal, delaytime = 0.2, decaytime = 1.0, maxDelay)` | a comb: `decaytime` is −60 dB counted from the first echo |
| | `allpassN/L/C(signal, delaytime = 0.2, decaytime = 1.0, maxDelay)` | an allpass: scatters phase and leaves the magnitude alone |
| | `pluck(signal, trig = 1.0, maxdelaytime = 0.2, delaytime = 0.2, decaytime = 1.0, coef = 0.5)` | a plucked string: a period of `signal` let into a lowpassed feedback line |
| | `pitchShift(signal, windowSize = 0.2, pitchRatio = 1.0, pitchDispersion = 0.0, timeDispersion = 0.0)` | the pitch moved by four overlapping grains |
| | `delTapWr(bufnum, signal)` / `delTapRd(bufnum, phase, deltime, interp = 1.0)` | a line round a buffer: one writer, any number of taps |
| Reverbs | `freeVerb(signal, mix = 0.33, room = 0.5, damp = 0.5)` / `freeVerb2(signal, signal2, ...)` | Freeverb, mono and stereo |
| | `gverb(signal, roomsize = 10.0, revtime = 3.0, ...)` | a room of a size and a decay: a feedback delay network, stereo |
| Physical models | `spring(signal, spring = 1.0, damp = 0.0)` / `ball(signal, g = 1.0, damp = 0.0, friction = 0.01)` / `tball(...)` | a damped spring; a bouncing ball's height, and its impacts |
| Envelopes | `envGen(env, { gate = 1.0, levelScale = 1.0, levelBias = 0.0, timeScale = 1.0, doneAction = DoneAction.NONE })` | plays an `Env` breakpoint envelope; the `doneAction` can free the node |
| | `line(start = 0.0, end = 1.0, dur = 1.0, doneAction = DoneAction.NONE)` / `xLine(start = 0.01, end = 1.0, dur = 1.0, doneAction = DoneAction.NONE)` | a single ramp, in equal steps / equal ratios; the same `DoneAction` set |
| | `ienvGen(env, index, offset = 0.0)` | an `Env` read at a position rather than played in time |
| | `linen(gate, attacktime, suslevel, releasetime, doneAction)` | the trapezoid of a gate, at control rate |
| | `lfGauss(duration, width, iphase, loop, doneAction)` | a Gaussian bell, repeated or once |
| Noise | `pinkNoise()` / `brownNoise()` | −3 dB/octave (quiet by construction, ~0.13 RMS) / −6, a reflecting random walk |
| | `grayNoise()` / `clipNoise()` | one random bit flipped per sample, grainy and low-leaning / ±1 only |
| | `lfNoise0(freq = 500.0)` / `lfNoise1(freq = 500.0)` / `lfNoise2(freq = 500.0)` | a new random value every `1/freq` s: held / ramped / curved (overshoots to ±1.7) |
| | `lfClipNoise(freq = 500.0)` | the held one, restricted to ±1 |
| | `lfdNoise0`, `lfdNoise1`, `lfdNoise3`, `lfdClipNoise(freq = 500.0)` | the dynamic forms: a change of rate takes effect at once |
| | `logistic(chaosParam = 3, freq = 1000, init = 0.5)` | the logistic map, iterated and held |
| | `cuspN`, `henonC`, `latoocarfianL`, `linCongN`, `lorenzL`, `quadC`, `standardN`, `fbSineL` … | scsynth's chaotic maps, held (`N`), linear (`L`) or cubic (`C`) |
| | `gendy1`, `gendy2`, `gendy3` | Xenakis's dynamic stochastic synthesis |
| | `dust(density = 1.0)` / `dust2(density = 1.0)` | random impulses at a **mean** density — exponential gaps, not a clock |
| | `crackle(chaos = 1.5)` | a deterministic chaotic map; one-sided, so it carries DC |
| Panning | `pan2(signal, pos = 0.0, level = 1.0)` | places a mono signal between two channels at **equal power**; returns two channels |
| | `linPan2(signal, pos = 0.0, level = 1.0)` | the same at constant **amplitude** — the two gains sum to `level` |
| | `balance2(left, right, pos = 0.0, level = 1.0)` | shifts an already stereo pair to one side; centred, it costs 3 dB |
| | `panAz(numchans, signal, pos = 0.0, level = 1.0, width = 2.0, orientation = 0.5)` | places a signal on a **ring** of `numchans`; returns that many channels |
| | `pan4(signal, xpos = 0.0, ypos = 0.0, level = 1.0)` | a signal on a square of four channels |
| | `panB`, `panB2`, `bipanB2(a, b, azimuth, level)`, `decodeB2(numchans, w, x, y, orientation = 0.5)` | first-order Ambisonics: encode into B-format, decode onto a ring |
| | `splay(signals, spread = 1.0, level = 1.0, center = 0.0)` | spreads a list across the field and folds it to a pair (client-side, no UGen of its own) |
| Stereo field | `rotate2(x, y, pos = 0.0)` | rotates the plane by `pos` **half turns** — turns an image without resizing it |
| | `midSide(a, b)` | the mid/side matrix, **its own inverse**: encodes and decodes with one call |
| | `stereoWidth(left, right, width = 1.0)` | widens or narrows: `0` mono, `1` the identity, `2` wide, negative swaps |
| Crossfades | `xfade2(a, b, pan = 0.0, level = 1.0)` / `linXfade2(a, b, pan = 0.0, level = 1.0)` | equal-power / constant-amplitude fade between two signals |
| | `select(which, ...items)` / `selectX(which, ...items)` | picks one source by index / crossfades between neighbours; every source still runs |
| Triggers | `trig(signal, dur = 0.1)` / `trig1(signal, dur = 0.1)` | holds the level that triggered it / holds 1, for `dur` seconds |
| | `tDelay(signal, dur = 0.1)` | one sample of 1, `dur` later; a trigger arriving while one is in flight is dropped |
| | `latch(signal, trig = 0.0)` / `gate(signal, trig = 0.0)` | one sample per rising edge / transparent while the gate is open |
| | `schmidt(signal, lo = 0.0, hi = 1.0)` | comparator with hysteresis — it takes the whole band to change its mind |
| | `changed(signal, threshold = 0.0)` | 1 where the input moved; compares the **halved** difference (sclang's `HPZ1` gain) |
| Counters | `toggleFf(trig = 0.0)` / `setResetFf(trig = 0.0, reset = 0.0)` | one bit of state: flipped by each trigger / set and reset |
| | `pulseCount(trig = 0.0, reset = 0.0)` / `pulseDivider(trig = 0.0, div = 2.0, start = 0.0)` | counts triggers / one out per `div` in, `start` phases it |
| | `stepper(trig = 0.0, reset = 0.0, min = 0.0, max = 7.0, step = 1.0, resetval = 0.0)` | walks `[min, max]`, both ends included, wrapping |
| | `timer(trig = 0.0)` / `sweep(trig = 0.0, rate = 1.0)` | seconds between the last two triggers / a ramp restarted at each |
| Followers | `decay(signal, decaytime = 1.0)` / `decay2(signal, attacktime = 0.01, decaytime = 1.0)` | an impulse into an exponential / the same with a rounded attack |
| | `detectSilence(signal, amp = 0.0001, time = 0.1, doneAction = DoneAction.NONE)` | 1 once the input has been quiet for `time`; raises a done flag |
| Node control | `freeSelf(signal)` / `pauseSelf(signal)` | frees / pauses the synth while `signal > 0`, passing it through |
| | `done(source)` | 1 once `source` (an envelope or a ramp) has finished — the flag, not its value |
| | `freeSelfWhenDone(source)` | passes `source` through and frees the synth once it finishes |
| | `pauseSelfWhenDone(source)`, `free(trig, id)`, `pause(gate, id)` | pauses the synth once its source finishes; frees, runs or pauses another node by id |
| | `peak`, `runningMax`, `runningMin(signal, trig)`, `peakFollower`, `runningSum`, `lastValue`, `mostChange`, `leastChange`, `zeroCrossing` | running statistics of a signal |
| Bus input | `in_(bus = 0.0)` | reads an audio bus (sampled per block) |
| | `inCtl(bus = 0.0)` | reads a control bus (constant over the block) |
| Bus output | `out(bus, signal)` | **sums** `signal` into an audio bus |
| | `replaceOut(bus, signal)` | **overwrites** an audio bus instead of summing |
| | `offsetOut`, `xOut(bus, xfade, signal)`, `sharedOut` | `out` by scsynth's name; a crossfade into a bus; a control bus |
| | `inFeedback(bus)`, `inTrig(bus)`, `lagIn(bus, lag)`, `sharedIn(bus)` | a bus a block late when written later; a control bus as a trigger, lagged, shared |
| Buffers | `playBuf(bufnum, chan = 0.0, rate = 1.0, loop = 0.0, trigger = 0.0, startPos = 0.0, doneAction = 0)` | mono buffer player, linear interpolation; `rate` in frames per output sample. A pass begins at `startPos` and a rising `trigger` re-cues there; without `loop`, the end fires `doneAction` (2 frees the synth) |
| | `bufRd(bufnum, chan, phase, loop = 0.0, interpolation = 2.0)` | reads a buffer at a `phase` signal in frames; `interpolation` 1 none, 2 linear, 4 cubic |
| Buffer info | `bufSampleRate(bufnum)` / `bufFrames(bufnum)` | the buffer's own sample rate (Hz) / frame count, block-constant (`kr`) |
| | `bufRateScale(bufnum)` | `file_sr / server_sr` — feed `playBuf`'s `rate` to play at the file's true pitch |
| | `bufChannels(bufnum)` / `bufDur(bufnum)` | channel count / duration in seconds, block-constant (`kr`) |
| Tables | `osc(bufnum, freq = 440.0, phase = 0.0)` | interpolating wavetable oscillator over a **wavetable-format** buffer (`/buffer_gen`); `phase` in radians |
| | `oscN(bufnum, freq = 440.0, phase = 0.0)` | non-interpolating oscillator over a **plain** buffer; rawer/cheaper |
| | `vosc(bufpos, freq = 440.0, phase = 0.0)` | `osc` with the buffer number as a signal: crossfades tables `bufpos`/`bufpos+1`, so sweeping it morphs a contiguous bank |
| | `vosc3(bufpos, freq1, freq2, freq3)`, `cosc(bufnum, freq, beats)` | three voices of one morphing table; two detuned voices of one table |
| | `index`, `indexL`, `wrapIndex`, `foldIndex(bufnum, signal)` | a buffer as a table, read at an index |
| | `indexInBetween`, `detectIndex(bufnum, signal)`, `degreeToKey(bufnum, signal, octave = 12)` | a table searched for a value; a scale degree to a key |
| | `shaper(bufnum, signal)` | waveshaper: maps `signal` (±1) through a `cheby` transfer table |
| Synth buffers | `localBuf(frames = 1, channels = 1)` | a buffer of the synth's own, numbered after the pool, freed with the synth |
| | `setBuf(bufnum, values, offset = 0.0)` / `clearBuf(bufnum)` / `maxLocalBufs(count)` | fill or zero a buffer at the synth's start; scsynth's count, which does nothing |
| | `scopeOut(signal, bufnum = 0.0)` | writes `signal` round a buffer, a channel each |
| Disk | `diskIn(chan = 0.0, { path, loop = false, start = 0 })` | streams a file from disk, one channel per UGen, no resampling |
| | `vdiskIn(chan = 0.0, rate = 1.0, { path, loop = false, start = 0 })` | `diskIn` at a variable rate, cubic interpolation |
| | `diskOut(signal, { path, format = "int16" })` | streams `signal` to a mono WAV on disk; passes `signal` through |
| Feedback | `localIn(channel = 0.0)` | reads a synth-private feedback channel |
| | `localOut(channel, signal)` | writes a feedback channel, and passes `signal` through |
| Smoothers | `lag(signal, time = 0.1)` | one-pole smoother (symmetric); the same UGen a lagged control inserts, usable on any signal |
| | `lagUd(signal, up = 0.1, down = 0.1)` | one-pole smoother with separate rise / fall times |
| | `lag2`, `lag3`, `lag2Ud`, `lag3Ud`, `ramp(signal, time)`, `slew(signal, up, down)` | cascaded smoothers, a segmented line, a rate limiter |
| | `lpz1`, `hpz1`, `lpz2`, `hpz2`, `bpz2`, `brz2`, `slope`, `flip`, `delay1`, `delay2` | scsynth's fixed FIRs and one- and two-sample delays |
| | `twoPole`, `twoZero`, `apf`, `ringz`, `formlet`, `midEq`, `fos`, `sos` | pole/zero sections and resonators |
| | `bLowPass`, `bHiPass`, `bAllPass`, `bBandPass`, `bBandStop`, `bPeakEq`, `bLowShelf`, `bHiShelf` | the cookbook biquads (BEQSuite) |
| | `median(length, signal)`, `moogFf(signal, freq, gain)` | a running median; the Moog ladder |
| | `hilbert(signal)`, `freqShift(signal, freq, phase)` | the analytic pair; a single-sideband shift |
| | `amplitude`, `compander`, `normalizer`, `limiter` | an envelope follower, a compressor/expander, look-ahead gains |
| | `varLag(signal, time = 0.1, start?)` | a straight line to each new value, taking `time` seconds, from `start` (the signal itself unless given) |
| Scalar (`ir`) | `sampleRate()` | the engine sample rate in Hz, computed once at init |
| | `blockSize()`, `controlRate()`, `controlDur()`, `sampleDur()`, `radiansPerSample()`, `subsampleOffset()` | the engine's timing, and where in its first sample the synth's bundle fell |
| | `numAudioBuses()`, `numControlBuses()`, `numInputBuses()`, `numOutputBuses()`, `numBuffers()`, `nodeId()`, `numRunningSynths()`, `bufSamples(bufnum)` | the server's configuration, the node, the synths, a buffer's samples |
| | `rand(lo = 0.0, hi = 1.0)` | one uniform random value in `[lo, hi)`, drawn once at init and held for the node's life |
| | `irand(lo = 0.0, hi = 127.0)`, `expRand(lo = 0.01, hi = 1.0)`, `linRand(lo, hi, minmax = 0.0)`, `nRand(lo = 0.0, hi = 1.0, n = 0.0)` | one integer, log-uniform, linear or mean-of-`n` value, drawn once and held |
| | `tRand(lo, hi, trig)`, `tiRand(...)`, `tExpRand(...)` | a random value drawn at the start and again at each trigger |
| | `coinGate(prob = 0.5, trig = 0.0)` | lets each trigger through with probability `prob` |
| | `tWindex(trig, weights, normalize = 0)` | a weighted choice of an index at each trigger |
| | `randId(id = 0.0)`, `randSeed(trig = 0.0, seed = 56789.0)` | choose the server generator the synth draws from; reseed it on a trigger (a root) |
| Demand (`dr`) | `dseq(values, repeats = 0.0)` | yields `values` in order, `repeats` passes (**0 is endless**); a value that is itself a stream is drained, not taken once |
| | `drand(values, repeats = 0.0)` / `dxrand(values, repeats = 0.0)` | `repeats` **items** drawn at random from the list; `dxrand` never picks the value it just used |
| | `dshuf(values, repeats = 0.0)` | the list shuffled **once** and replayed in that order, `repeats` passes |
| | `dseries(repeats = 0.0, start = 0.0, step = 1.0)` | arithmetic sequence; `step` is read per item, so it may be a stream |
| | `dgeom(repeats = 0.0, start = 1.0, grow = 2.0)` | geometric sequence `start`, `start * grow`, … |
| | `dwhite(repeats = 0.0, lo = 0.0, hi = 1.0)` / `diwhite(repeats = 0.0, lo = 0.0, hi = 1.0)` | independent uniform draws on `[lo, hi]`; `diwhite` over the integers, both ends included |
| | `dbrown(repeats = 0.0, lo = 0.0, hi = 1.0, step = 0.01)` / `dibrown(repeats = 0.0, lo = 0.0, hi = 1.0, step = 1.0)` | a random walk of at most `step` per item, **folded** into `[lo, hi]`; `dibrown` over the integers |
| | `dstutter(repeats, value)` | repeats each item of `value` `repeats` times; the count is pulled per item |
| | `dswitch1(which, ...items)` | takes **one** item from the stream `which` picks, then picks again; the unselected ones do not advance |
| | `dbufrd(bufnum, phase, loop = 1.0, channel = 0.0)` | reads the buffer frame the `phase` stream names — a `dseries` phase walks a buffer as a step sequence |
| | `demand(trig, reset, source)` | driver: pulls one value per rising edge of `trig`, holding it in between; holds the last value once the stream ends |
| | `duty(dur, reset = 0.0, level = 1.0, doneAction = 0)` | driver with a clock of its own: pulls one `level` every `dur` seconds and holds it; `doneAction` fires when either stream ends |
| | `tduty(dur, reset = 0.0, level = 1.0, doneAction = 0, gapFirst = 0.0)` | `duty` emitting each level on its own sample and silence in between — a trigger stream with amplitudes |
| Fused | `madd(a, b, c)` | `a*b + c` in one UGen (the multiply-accumulate the server fuses) |
| | `sum3(a, b, c)` / `sum4(a, b, c, d)` | three / four-operand sums in one UGen |
| Maps | `linExp(signal, srclo, srchi, dstlo, dsthi)` | linear in, exponential out, unclipped |
| | `inRange(signal, lo, hi)`, `inRect(x, y, left, top, right, bottom)` | 1 inside, 0 outside |
| | `modDif(x, y, mod)`, `unwrap(signal, lo, hi)` | a distance on a circle; a wrap undone |
| | `ampComp(freq, root, exp)`, `ampCompA(freq, root, minAmp, rootAmp)` | loudness compensation by a power, or along the A-weighting |
| Rates | `k2a`, `a2k`, `t2a(signal, offset)`, `t2k`, `dc(signal)` | control to audio and back, for signals and triggers; a constant |
| | `hasher(signal)`, `mantissaMask(signal, bits)` | noise that is a function of its input; precision reduced |
| Measure | `meter(signal, decay = 20.0, hold = 0.0)` | a level with a **meter's ballistics**: instantaneous attack, a fall of `decay` decibels per second, a new peak held `hold` seconds before it starts falling. One number a block, which is what a control bus carries — pair it with `outCtl` for a meter a host reads as a bus |
| | `clipCount(signal, ceiling = 1.0, run = 3)` | **how many times the signal was flattened**, since the pass began: a run of `run` consecutive samples at or over `ceiling` is one over, however long it lasts. A lone sample at full scale is not clipping — the engine is floating point — so the run is what a red lamp is actually reporting; `run=1` is the pessimistic meter. It walks samples rather than blocks, which is what a block peak has already lost. Pair it with `outCtl` and hand the bus to a `meter` widget's `clip` |
| | `truePeak(signal, decay = 20.0, hold = 0.0)` | a meter's level over the **reconstructed** signal: `meter`'s ballistics, but the block's true peak (the ITU-R BS.1770-4 filter) rather than its largest sample, so the reading is in dBTP. Pair it with `outCtl` and a `meter` widget at `rate: "control"` with `peak: "true"` |
| Side-effect | `sendTrig(trig, id = 0, value = 0.0)` | on each trigger, sends `/node_trigger nodeID id value`; output is silence |
| | `sendReply(trig, values = [], { cmd = "/reply", replyId = -1 })` | sends a custom OSC message with an arbitrary value list |
| | `sendPeakRms(signals, { replyRate = 20, peakLag = 3, cmd, replyId })` | replies each signal's peak and RMS, a meter's ballistics |
| | `poll(trig, signal, trigId = -1, { label = "poll" })` | posts `signal` to the server console (and a `/node_trigger` when `trig_id >= 0`); passes `signal` through |
| Spectral | `fft(source, active = 1.0, { fftSize = 1024, hop = 0.5, wintype = 0 })` | opens a spectral chain (windows and transforms `source` per hop) |
| | `pvMagAbove(chain, threshold)` / `pvMagBelow(chain, threshold)` | pass bins above / below a magnitude threshold |
| | `pvMagClip(chain, threshold)` | limit each bin's magnitude to the threshold (phases kept) |
| | `pvBrickWall(chain, wipe)` | zero a band (`wipe > 0` low pass, `< 0` high pass) |
| | `pvAdd(chainA, chainB)` / `pvMul(chainA, chainB)` / `pvMin(chainA, chainB)` / `pvMax(chainA, chainB)` | two-chain combiners: per-bin complex sum / product / smaller / larger magnitude |
| | `pvMagMul(chainA, chainB)` / `pvCopyPhase(chainA, chainB)` | cross-synthesis: A's bins scaled by B's magnitudes / A's magnitudes with B's phases |
| | `pvMagFreeze(chain, freeze = 0.0)` | hold the stored magnitude envelope while `freeze > 0` (phases keep running) |
| | `pvMagSmear(chain, bins = 0.0)` | average each bin's magnitude over `bins` neighbors per side (spectral blur) |
| | `pvBinShift(chain, stretch = 1.0, shift = 0.0)` / `pvMagShift(chain, stretch = 1.0, shift = 0.0)` | remap bin positions (complex bins / magnitude envelope only) |
| | `pvKernel(chain, { mag, phase, params = [] })` | apply **user-written bin expressions** to every bin of each fresh frame — see [Writing your own spectral operation](#writing-your-own-spectral-operation) |
| | `ifft(chain)` | closes a spectral chain (resynthesises audio by overlap-add) |
| Convolution | `conv(source, kernel, { fftSize = 1024, partitions = 16 })` | partitioned convolution against a kernel prepared with `dest.gen("prepare_partconv", fft_size, ir_bufnum)` (size `dest` with `partconvFrames`); latency `fft_size / 2` samples |
<!-- /web -->

Like Faust synths, a SynthDef also accepts the reserved `in` / `out` bus-selecting controls the server adds at `/synth_new` time.

The **side-effect** UGens exist for a reply or a console post rather than audio, so a def may consist of them alone with **no `out`** — pass them as `SynthDef` roots (see [Building one](#building-one-1)). What makes one a root is that it **delivers data out of the graph** — audio or control to a bus, audio to a file (`{{api disk_out}}`), OSC or a console line to a client — not that it has a side effect. `{{api free_self}}`, `{{api pause_self}}`, `{{api free_self_when_done}}` and `done` act on the enclosing synth and pass their input *through*, so they still want an `out` around them ({{alt `out(0, free_self_when_done(env * sig))` || `out(0, freeSelfWhenDone(env.mul(sig)))`}} is the idiom); `{{api detect_silence}}` writes 0/1 for the rest of the graph and takes its done action, which is inward too. `{{api disk_out}}` is the one that surprises: it delivers audio to a file, so {{alt `play(disk_out(sig, path=path))` || `play(diskOut(sig, { path }))`}} records **without sounding** — route it yourself to do both.<!-- web --> In a tab the file is one of the page's storage, as every path is there.<!-- /web --> The **spectral** UGens form a frequency-domain chain — see [The frequency-domain chain](#the-frequency-domain-chain) below.

The **panning** callables return a `ChannelList`, so they compose with everything the container already does — {{alt `out(0, pan2(sig, pos))` || `out(0, pan2(sig, pos))`}} lays the pair on consecutive buses, and {{alt `pan2(...) * 0.5` || `pan2(...).mul(0.5)`}} scales both channels. (A UGen has one output, so each is really one row per channel with a trailing channel index; the builder fills it and you never pass it.) Two things about the family are worth knowing before picking one. **Equal power** — `pan2`, `balance2`, `xfade2`, `{{api select_x}}` — holds one *loudness* across the move, which is what you want for a source crossing the field, but it puts 0.707 in each channel at the centre and adds 3 dB when the two sides are the same signal; **constant amplitude** — `{{api lin_pan2}}`, `{{api lin_xfade2}}` — holds the *sum* instead, which is what correlated samples and mono fold-downs want. And `balance2` is not a pass-through when centred: it applies the pan law to a pair that is already stereo, so it costs 3 dB there (scsynth's behaviour, kept).

`rotate2`, `{{api mid_side}}` and `{{api stereo_width}}` are the same matrix said three ways, and the names are worth reading closely because scsynth only has the first. A **rotation** moves an image without resizing it; **width** resizes it without moving it. `rotate2` at a quarter turn *is* the left/right ↔ mid/side basis change, which is what `{{api mid_side}}` names outright — normalized so one call both encodes and decodes:

```python
m, s = mid_side(left, right)          # encode
out(0, mid_side(lpf(m, 400), s * 1.5))  # process each axis, decode, widened
```

```ts
const [m, s] = midSide(left, right);        // encode
out(0, midSide(lpf(m, 400), s.mul(1.5)));   // process each axis, decode, widened
```

Use `{{api stereo_width}}` when a width knob is all you need, and `{{api mid_side}}` when something has to happen *between* the two — which is the case the knob cannot express.

The **demand** callables are a sequencer inside the def. A `d*` builder is not a signal: it is a **stream**, with no samples and only a next value, and it yields one each time something pulls it. Three things pull. `demand` is told when, by a trigger — the classic "one note per `impulse`". `duty` and `tduty` bring their own clock, pulling one `level` every `dur` seconds, and both `dur` and `level` are pulled, which is what makes them sequencers rather than samplers: a stream of durations against a stream of pitches, the two free to be different lengths. `duty` holds each level; `tduty` emits it on one sample and is silent in between, so it is a trigger train whose amplitudes are the levels — feed it to an envelope's gate.

```python
from clausters.defs import SynthDef, dseq, drand, duty, sine, out

pitch = dseq([60, 62, 67, drand([70, 72, 74], repeats=2)])  # a stream inside a stream
dur = dseq([0.25, 0.25, 0.5])
sdef = SynthDef("seq", out(0.0, sine(duty(dur, level=pitch).midicps()) * 0.2))
```

```ts
import { SynthDef, drand, dseq, duty, out, sine } from "clausters/defs";

const pitch = dseq([60, 62, 67, drand([70, 72, 74], 2)]);   // a stream inside a stream
const dur = dseq([0.25, 0.25, 0.5]);
const sdef = new SynthDef("seq", out(0.0, sine(duty(dur, 0.0, pitch).midicps()).mul(0.2)));
```

Two conventions carry the family. **`repeats` of 0 is endless** — sclang writes `inf`, which a def cannot carry — and it counts *passes over the list* for `dseq`/`dshuf` but *items* for `drand`/`dxrand`, which have no pass to complete. And **a stream may go anywhere a number may**, as above: `dseq` **drains** a nested stream before moving on, and restarts it when it comes round to it again, so that pitch stream is five items per pass (three notes, then two drawn) against three durations, and the two only realign after fifteen. That is the whole reason the family exists — a sequence of phrases rather than of numbers. When a stream ends, `demand` holds its last value while `duty`/`tduty` fire their `{{api done_action}}`, so a finite sequence can free its own node. Because a stream is *pulled*, it cannot be shared: two drivers reading one `dseq` take alternate items from it, so give each its own.

### Maths on a UGen graph

Beyond the four arithmetic operators, the **full unary/binary maths** works on a SynthDef graph: {{alt `%`, `min`/`max`, the comparisons || `.mod()`, `.min()`/`.max()`, the comparisons}}, `.sin()`, `.midicps()`, `.distort()`, `.clip2(x)` and the rest compose the server's generic **`BinaryOpUGen`/`UnaryOpUGen`**, each carrying the operator by name, and `.clip(lo, hi)`, `.wrap(lo, hi)` and `.fold(lo, hi)` compose the `Clip`/`Wrap`/`Fold` UGens. The random operators sclang has on a signal — `.rand()`, `.rand2()`, `.linrand()`, `.bilinrand()`, `.sum3rand()`, `.coin()`, `.rrand(hi)`, `.exprand(hi)` — draw a new value every sample from the node's own seeded stream (a `RandOpUGen`), with the same distributions `rrand` and its siblings draw a value from. It is the same operation set the value side (`builtins`) computes — one shared `clausters-core` implementation — so a value you compute ahead of time and the UGen on the audio thread agree **bit-for-bit** for the native ops.

```python
from clausters.defs import SynthDef, control, sine, out

note = control("note", 60.0)
freq = note.midicps()                     # UnaryOpUGen (midicps)
sig = sine(freq).distort() * 0.3       # UnaryOpUGen (distort), then Mul
lfo = (sine(5.0) * 0.5 + 0.5).clip2(0.8)  # % min max > .fold/.clip … all compose
sdef = SynthDef("lead", out(0.0, sig * lfo))
```

```ts
import { SynthDef, control, out, sine } from "clausters/defs";

const note = control("note", 60.0);
const freq = note.midicps();                          // UnaryOpUGen (midicps)
const sig = sine(freq).distort().mul(0.3);            // UnaryOpUGen (distort), then Mul
const lfo = sine(5.0).mul(0.5).add(0.5).clip2(0.8);   // mod min max gt fold/clip … all compose
const sdef = new SynthDef("lead", out(0.0, sig.mul(lfo)));
```

The {{alt `+ - * /` operators keep || `add sub mul div` methods keep}} their dedicated `Add`/`Sub`/`Mul`/`Div` kinds (so existing defs are byte-identical); every other operator becomes an op UGen. Operators and math methods come from the shared `AbstractObject`, so the *same* expression composes a Faust graph, a UGen graph, or concrete numbers depending on what it is applied to. Only a selector with no server op {{alt raises || throws}} `TypeError`. A `FaustDef` is still the tool for genuinely custom per-sample DSP (recursion, tables, sample-accurate feedback); a SynthDef covers ordinary maths as well as wiring.

**Mapping a range is part of that maths**, and it is the one place the two
sides used to be written twice. The six range maps are methods on any graph
node — the same names, the same arguments and the same `clip` as the value
functions in `builtins`, over the server's `RangeMapUGen`:

```python
from clausters.defs import SynthDef, out, sine

lfo = sine(0.2).at_rate("kr")
sweep = sine(lfo.linexp(-1.0, 1.0, 200.0, 8000.0))   # -1..1 onto 200..8000 Hz
amp = sine(0.7).lincurve(-1.0, 1.0, 0.0, 0.5, curve=-4.0)
sdef = SynthDef("sweep", out(0.0, sweep * amp))
```

```ts
import { SynthDef, out, sine } from "clausters/defs";

const lfo = sine(0.2).atRate("kr");
const sweep = sine(lfo.linexp(-1.0, 1.0, 200.0, 8000.0));   // -1..1 onto 200..8000 Hz
const amp = sine(0.7).lincurve(-1.0, 1.0, 0.0, 0.5, -4.0);
const sdef = new SynthDef("sweep", out(0.0, sweep.mul(amp)));
```

`linlin`, `linexp`, `explin`, `expexp`, `lincurve` and `curvelin` all read the
same `clausters-core` function the script's own `linexp` does, so an LFO mapped
in the def and a fader position mapped in the {{alt script || page}} land in the same place. `clip`
says what an out-of-range input is trimmed to first — `"minmax"` (the default),
`"min"`, `"max"` or `"none"` — and every bound may itself be a signal, so a
range can be modulated. sclang's bipolar `range`/`exprange` are deliberately not
methods here: they read a UGen's declared polarity, which this graph does not
track, so a def writes the input range out and says what it means.

Feedback within a block uses the `LocalIn` / `LocalOut` pair. `LocalIn` must be emitted before its `LocalOut`; the topological walk guarantees that as long as the output graph reaches the `{{api local_in}}` before the `{{api local_out}}`. Make the `{{api local_out}}` one of the def's outputs so its write stays in the graph:

```python
from clausters.defs import SynthDef, sine, local_in, local_out, out

fb = local_in(0.0)                            # private channel 0
sig = sine(440.0) * 0.2 + fb * 0.5
echo = local_out(0.0, sig)                    # writes channel 0, passes sig through
sdef = SynthDef("fb", out(0.0, sig), echo)    # echo as an output keeps the write
```

```ts
import { SynthDef, localIn, localOut, out, sine } from "clausters/defs";

const fb = localIn(0.0);                          // private channel 0
const sig = sine(440.0).mul(0.2).add(fb.mul(0.5));
const echo = localOut(0.0, sig);                  // writes channel 0, passes sig through
const sdef = new SynthDef("fb", out(0.0, sig), echo);   // echo as an output keeps the write
```

### Multichannel: dup, channel lists and mix

Multichannel on a UGen graph is an **explicit container**, the `ChannelList` — there is no sclang-style implicit expansion (`sine([440, 443])` does not fan out; a channel list reaching a single-channel input {{alt raises || throws}} `TypeError` at serialization, pointing here). The container never crosses the wire: outputs unroll it onto consecutive buses and the serialization flattens it, so the server only ever sees single-channel UGens. The bus they start from is a number or a control: `out(control("out", 0.0), pan2(sig, pos))` writes the pair on whatever bus a note sets `out` to and the one after it, which is what an instrument that plays into a track needs.

```python
from clausters.defs import SynthDef, control, dup, mix, out, rand, sine, white_noise

stereo = dup(sine(440.0))                  # 2 channels, ONE Sine (by reference)
bank = dup(lambda: sine(rand(438.0, 442.0)), 8)   # 8 DISTINCT detuned sines
sig = mix(bank) * 0.1                      # folded via Sum4/Sum4/Add, one channel
sdef = SynthDef("bank", out(0.0, dup(sig)))       # channels -> buses 0, 1
```

```ts
import { control, dup, mix, out, rand, sine, SynthDef, whiteNoise } from "clausters/defs";

const stereo = dup(sine(440.0));                      // 2 channels, ONE Sine (by reference)
const bank = dup(() => sine(rand(438.0, 442.0)), 8);  // 8 DISTINCT detuned sines
const sig = mix(bank).mul(0.1);                       // folded via Sum4/Sum4/Add, one channel
const sdef = new SynthDef("bank", out(0.0, dup(sig)));   // channels -> buses 0, 1
```

The rules, in order of appearance:

- **`dup(x, n=2)`** fans out. A node (or number) is repeated **by reference** — identity dedup serializes it *once*, so `dup(sine(440))` is a cheap mono→stereo of identical channels. A **{{alt callable || function}}** is evaluated `n` times, building *distinct* UGens — what a decorrelated or detuned bank needs ({{alt `dup(white_noise, 8)` || `dup(whiteNoise, 8)`}}; duplicating one noise by reference would give eight copies of the *same* noise). The method form `sine(440).dup(8)` is always by reference; the {{alt callable || function}} form is the reference↔fresh contrast, mirroring sclang's `ugen.dup` vs `{ }.dup`.
- **Operators broadcast or zip.** A scalar operand (number, `Ugen`, `Control`) applies to every channel; a list operand pairs channel-wise, and unequal lengths wrap the shorter side modulo — the same rule the value side applies to plain lists, so graph and values agree. <!-- python -->Plain Python lists coerce in operator positions (`bank * [0.1, 0.2]`), but only `ChannelList` *is* an operator object — `[a, b] * 2` is list repetition.<!-- /python --><!-- web -->A plain array coerces in operand positions (`bank.mul([0.1, 0.2])`).<!-- /web -->
- **`out(bus, chans)` turns channels into buses**: one writer per channel on `bus`, `bus+1`, … (`{{api replace_out}}`/`{{api out_ctl}}` likewise).
- **A channel list is an expression**, so the verbs take one directly, with no def of its own: `play(sine(440).dup())` sounds in stereo, `plot(pan2(sig, pos))` shows two lanes, {{alt `render(chans(a, b), channels=2)` || `render(chans(a, b), { channels: 2 })`}} bounces the pair. The coercion lays the members that are *not* already sinks on buses 0, 1, … in order — a member that is one (an `out`, a `{{api send_trig}}`) already knows where its data goes, passes through as a root, and consumes no channel, so a reporter in the list does not push the audio off bus 0.
- **`mix(chans)`** folds back to one channel through the fused sums (`sum4`/`sum3` chunks, not an `Add` chain — an 8-channel mix is 3 UGens). A list of plain numbers folds to a number; a scalar passes through.
- **Not implemented (yet):** per-argument expansion and nested channel lists. Both error early with pointed messages.

### The frequency-domain chain

Spectral processing wires an `fft` to an `ifft` with any number of `pv_*` filters between them: `fft` windows an audio signal and transforms it to a spectral frame once per hop, the `pv_*` filters mutate the frame, and `ifft` resynthesises audio by overlap-add. Wire them in order — the output of one is the input of the next:

```python
from clausters.defs import SynthDef, white_noise, fft, pv_brick_wall, ifft, out

chain = fft(white_noise() * 0.25, fft_size=1024, hop=0.5, wintype=0)
chain = pv_brick_wall(chain, 0.75)      # keep the bottom 25% of bins (a low pass)
sdef = SynthDef("spec_lp", out(0.0, ifft(chain)))
```

```ts
import { SynthDef, fft, ifft, out, pvBrickWall, whiteNoise } from "clausters/defs";

let chain = fft(whiteNoise().mul(0.25), 1.0, { fftSize: 1024, hop: 0.5, wintype: 0 });
chain = pvBrickWall(chain, 0.75);       // keep the bottom 25% of bins (a low pass)
const sdef = new SynthDef("spec_lp", out(0.0, ifft(chain)));
```

The FFT frame is **synth-private scratch on the server** (SuperCollider's `LocalBuf` model) — no buffer is allocated and none is required. Only `fft` names the window size, hop and window type (they size the transform); the server propagates them to the rest of the chain. A bare `fft` → `ifft` reconstructs the signal at unity gain, delayed by one window.

A **two-chain combiner** (`{{api pv_add}}`, `{{api pv_mul}}`, `{{api pv_min}}`, `{{api pv_max}}`, `{{api pv_mag_mul}}`, `{{api pv_copy_phase}}`) takes two chains — both from an `fft` of the **same {{alt `fft_size` || `fftSize`}}**, and distinct — and writes the result into chain A (its first argument), which the combiner's output carries onward. `{{api pv_mag_mul}}` is the classic cross-synthesis: a noise carrier in chain A comes out wearing the spectral envelope of the modulator in chain B (see {{alt `examples/spectral/cross.py` || `examples/spectral/cross.html`}}, which also freezes the ending with `{{api pv_mag_freeze}}`).

The window type is also settable **live** with `{{api synth.u_cmd}}`, which addresses one UGen instance inside a running synth:

```python
from clausters.defs import Window

synth.u_cmd(fft_index, "window", Window.BLACKMAN)  # swap the FFT window
```

```ts
import { Window } from "clausters/defs";

synth.uCmd(fftIndex, "window", Window.BLACKMAN);  // swap the FFT window
```

where {{alt `fft_index` || `fftIndex`}} is the FFT's position in the serialized graph and `Window` names the server's smoothing windows by the numbers it reads.

### Writing your own spectral operation

The `pv_*` set above is deliberately **curated** — it grows as parameterized implementations, never as a catalog of near-duplicates. When the operation you want is not there, you usually do not need a new server UGen at all: `{{api pv_kernel}}` applies **bin expressions you write yourself** to every bin of each fresh frame. The symbolic terms live in {{alt `clausters.defs.pv_expr` || the `pvExpr` namespace of `clausters/defs`}} — `mag`, `phase`, `{{api bin_index}}`, `nbins`, `binfreq` and `param(i)` — and compose with {{alt ordinary Python operators || the same operator methods}} and math methods, the same vocabulary UGen graphs and off-RT values use:

```python
from clausters.defs import SynthDef, control, fft, ifft, out, pv_kernel, white_noise
from clausters.defs.pv_expr import mag, bin_index, nbins, param

src = fft(white_noise() * 0.25)
# A tilted spectral gate: the threshold rises with frequency, so the gate
# bites harder on the highs. Not in any catalog -- it is just an expression.
gated = pv_kernel(
    src,
    mag=mag * (mag >= param(0) * (1 + 4 * bin_index / nbins)),
    params=[control("thresh", 2.0)],
)
sdef = SynthDef("tiltgate", out(0.0, ifft(gated)))
```

```ts
import { control, fft, ifft, out, pvKernel, SynthDef, whiteNoise } from "clausters/defs";
import { binIndex, mag, nbins, param } from "clausters/defs/pv_expr";

const src = fft(whiteNoise().mul(0.25));
// A tilted spectral gate: the threshold rises with frequency, so the gate
// bites harder on the highs. Not in any catalog -- it is just an expression.
const gated = pvKernel(src, {
    mag: mag.mul(mag.ge(param(0).mul(binIndex.div(nbins).mul(4).add(1)))),
    params: [control("thresh", 2.0)],
});
const sdef = new SynthDef("tiltgate", out(0.0, ifft(gated)));
```

**Calibrate thresholds to the FFT's magnitude scale.** A bin's `mag` is a raw transform magnitude, **not** a 0..1 value: it scales with the input level, the window and the FFT size (for the source above — noise at amplitude 0.25 through a 1024-point Hann — magnitudes spread over roughly 0.5..5, median ≈ 2.3). A threshold far below that spread gates almost nothing, and the chain's one-window latency alone will make the output *sound* vaguely different (decorrelated) while the spectrum is essentially untouched — a classic miscalibration trap. Probe the scale first (render and inspect, or `poll` a reference), or express the threshold relative to a known reference level.

Each expression maps **one bin's values** — its magnitude, its phase (radians), its index, the bin count, its center frequency, and the `params` signals sampled once per hop — to that bin's new magnitude (`mag`) or phase (`phase`). An omitted expression is the identity, and an identity phase keeps each bin's phase *exactly*: a pure magnitude map takes the same cheap scaling path as the built-in `pv_*` filters (a kernel reimplementing `{{api pv_mag_above}}` renders sample-identically to it — that equivalence is a server test). The client serializes the expression to a small postfix program; the server validates it at `/def_send synth` (unknown terms, malformed expressions and out-of-range `param` indices fail the def with `/fail`, never at render time) and interprets it per bin, per frame — pure `f32`, allocation-free, bit-identical between real-time and offline rendering.

**What fits, and what does not.** An expression is a **pure per-bin map**, and that boundary is the design:

- **Fits** — anything that decides each bin from that bin alone: gates and thresholds (fixed, tilted, frequency-dependent via `binfreq`), spectral tilts and shelves, magnitude algebra (compression via `.pow()`, saturation via `.tanh()`), masks over bin ranges (`{{api bin_index}}`/`nbins` comparisons), phase manipulation (offsets, scrambles deterministic in `{{api bin_index}}`).
- **Does not fit** — state across frames (freeze, smear, running averages: use `{{api pv_mag_freeze}}` / `{{api pv_mag_smear}}`), moving energy *between* bins (shift, stretch: use `{{api pv_bin_shift}}` / `{{api pv_mag_shift}}`), reading another chain (use the two-chain combiners), and whole algorithms with their own structure (convolution is `conv`, with its off-thread kernel preparation — some operations genuinely deserve a dedicated UGen).

Expressions are capped at 256 tokens, and `params` accepts any signal — a control, an LFO, another UGen — so a kernel stays fully modulatable at run time. A commented walk-through is {{alt `examples/spectral/kernel.py` || `examples/spectral/kernel.html`}}.

### Spec, dump and controls

The graph is serialized by a plain post-order walk of the output UGens, so a UGen is always emitted after its inputs (the `ugens` list is topologically ordered) and a shared sub-graph is emitted once (deduplicated by object identity).

- `sdef.spec()` — the `{"name", "controls", "ugens"}` {{alt dict || object}} the server compiles.
- `sdef.{{api dump_def}}()` — that spec as JSON text, the `/def_send synth` payload.
- `sdef.{{api control_names}}()` — the control names in spec order (parallels `FaustDef.{{api control_names}}()`).

## Inspecting the built graph

Before sending a def you can look at exactly what you composed: both `FaustDef` and `SynthDef` expose `{{api dump_def}}()`, the JSON string that goes on the wire (the `/def_send faust` / `/def_send synth` argument). Printing it shows the resulting graph — handy to confirm a tree built the way you intended.

```python
import json

print(sdef.dump_def())                                   # the raw wire string
print(json.dumps(sdef.spec(), indent=2))                 # SynthDef: spec() is already a dict — pretty-print it
print(json.dumps(json.loads(fdef.dump_def()), indent=2)) # FaustDef signal/box tree, pretty-printed
```

```ts
console.log(sdef.dumpDef());                                      // the raw wire string
console.log(JSON.stringify(sdef.spec(), null, 2));                // SynthDef: spec() is already an object
console.log(JSON.stringify(JSON.parse(fdef.dumpDef()), null, 2)); // FaustDef signal/box tree, pretty-printed
```

`SynthDef.spec()` returns the `{"name", "controls", "ugens"}` {{alt dict || object}} directly, so it is the most convenient handle for a SynthDef. For a `FaustDef` built from signals or boxes, `{{api dump_def}}()` is JSON you can parse; one built from source returns the Faust source string verbatim, not JSON. Either way, `{{api control_names}}()` gives just the declared control names when that is all you need.

## Choosing between the two

| | FaustDef | SynthDef |
| --- | --- | --- |
| Sent with | `/def_send faust` | `/def_send synth` |
| Built from | `signals` / `boxes` / Faust source | the UGen callables |
| Compiled | JIT on the server (libfaust/LLVM{{alt  || ; libfaust-wasm in a tab}}) | assembled from the server's UGen registry |
| Maths | full (trig, `exp`/`log`, comparisons, tables) | full (the same operator set, as generic op UGens) |
| Feedback | `rec` / `self_` (one sample) | `{{api local_in}}` / `{{api local_out}}` (one block) |
| Granularity | **per sample** — you write the DSP | **per unit** — you wire DSP the server implements |
| Reuse | the Faust libraries (`os.`, `fi.`, `re.`, `pm.`, …) | the UGen set (see the table above) |
| Server build | the `faust` feature — default, and bundled in the {{alt wheel || package}} | the `synth` feature — default |

Neither is the fallback of the other. The units a `SynthDef` wires are the fastest way to say "an oscillator into a bus", they need no compiler on the server, and they are the only way to reach the server-side machinery that has no Faust equivalent (buffer playback, bus I/O, `{{api send_reply}}`/`poll`, the FFT chain). A `FaustDef` is how you write DSP that is *not* in the unit set — a filter you derived, a physical model, an algorithm — either from {{alt Python || code}} (`signals` / `boxes`) or in Faust itself, with its libraries at hand.

The common pattern is to combine them: a FaustDef for the voice, a SynthDef to route it, play back buffers or analyse the result. Both run as ordinary nodes in the same tree, on the same buses, and are controlled by the same `/node_set`.

## GraphDef: several defs as one instantiable thing

A `SynthDef` or a `FaustDef` persists **one node**. A `GraphDef` persists a
whole **configuration**: member nodes, the private buses that wire them, and a
named **port surface** the running instance is driven through — never the
private member ids. Instantiating one is a group; setting a port is a
`/node_set` against that group.

```python
g = GraphDef("chain")
mix = g.bus("mix")                          # a private audio bus
src = g.add("gsrc", out=mix, level=1.0)     # a member; its `out` -> the bus
g.add("gsink", {"in": mix, "out": "OUT"})   # "OUT" is hardware bus 0
g.port("gain", src["level"], default=0.5)   # a port -> the source's level

g.send(server)
inst = Group.graph("chain", {"gain": 0.3}, server=server)
```

```ts
const g = new GraphDef("chain");
const mix = g.bus("mix");                           // a private audio bus
const src = g.add("gsrc", { out: mix, level: 1.0 }); // a member; its `out` -> the bus
g.add("gsink", { in: mix, out: "OUT" });            // "OUT" is hardware bus 0
g.port("gain", [src.control("level")], 0.5);        // a port -> the source's level

await g.send(server);
const inst = Group.graph("chain", { gain: 0.3 }, { server });
```

**A member can be another `GraphDef`**, and that is what keeps the layers from
each needing their own machinery. {{alt `kind="graph"` || `kind: "graph"`}} builds the member as a subgroup
with private buses of its own, freed with its parent — so an effect chain, a
compound effect and a nested graph are one thing rather than three. Two rules
make such a child reusable rather than wired in place:

- **It does not decide where it goes.** A bus declared {{alt `external=True` || `external: true`}} is one
  the parent provides, and the member's controls name which of the *parent's*
  buses each of them is. A graph instantiated on its own is handed nothing and
  allocates everything, so the same def works standalone and nested.
- **Its interface is re-exported.** A target on a graph member names one of the
  child's **ports** instead of a control — and it is written exactly the same
  way, because what {{alt `child["gain"]` || `child.control("gain")`}} means follows from what the member is.

**A member can be one there is a changing number of.** {{alt `slot="clips"` || `slot: "clips"`}} marks a
member built on demand rather than at instantiation, and
`instance.{{api add_slot}}("clips")` builds one more, wired to the instance's own
buses. A voice of a polyphonic instrument, a clip on a track, an effect in a
chain: one question — how many of these are there right now — whose answer
changes while the graph is sounding, which is why it cannot be a member list.
{{alt `voice=True` || `voice: true`}} is the slot named `"voice"`, spelled the way it was before slots
had names, and it is what a MIDI note spawns (`instance.voice()`).
A slot that was built can also change instance: `clip.{{api move_slot}}(otherTrack)`
moves it into another instance of the same def — a clip dragged to another
track — and the server re-wires it to that instance's buses instead of building
it again, so its ports and whatever is mapped onto them stay.

```python
track = GraphDef("track")
mix = track.bus("mix")
track.bus("out", external=True)                     # the parent hands this over
track.add("fader", {"in": mix, "out": "OUT"})
clip = track.add("clip", {"out": mix}, kind="graph", slot="clips")
track.port("clip/gain", clip["gain"])               # the child's port, re-exported

live = Group.graph("track", server=server)
first = live.add_slot("clips", {"clip/gain": 0.8})  # and another, and another
```

```ts
const track = new GraphDef("track");
const mix = track.bus("mix");
track.bus("out", { external: true });                // the parent hands this over
track.add("fader", { in: mix, out: "OUT" });
const clip = track.add("clip", { out: mix }, { kind: "graph", slot: "clips" });
track.port("clip/gain", [clip.control("gain")]);     // the child's port, re-exported

const live = Group.graph("track", {}, { server });
const first = live.addSlot("clips", { "clip/gain": 0.8 });   // and another, and another
```

The nesting is **of authoring, not of execution**. Every port resolves once, at
instantiation, to the controls of actual nodes however many levels down they
were written, so a `/node_set` costs what it always did and the server's node
tree is groups and synths as it always was.

## What the server must support

Both def families are **on by default** on the server (`synth` and `faust` are Cargo features, and both are in the default set), and everything the {{alt wheel || package}} ships is built that way: <!-- python -->the standalone `clausters` binary, the in-process embedded server and the offline renderer all take a `SynthDef` *and* a `FaustDef`. The package even bundles libfaust, with its LLVM JIT linked in, so Faust works on a machine without it installed — nothing to enable, nothing to build.<!-- /python --><!-- web -->the engine in a tab and the offline renderer both take a `SynthDef` *and* a `FaustDef`. The package bundles `libfaust-wasm`, which runs in the NRT worker, and the module it emits is linked into the engine's own memory — nothing to enable, nothing to build.<!-- /web -->

The one case where a `FaustDef` can fail is a server **someone built without the feature** (`--no-default-features`, e.g. a minimal SynthDef-only build). It answers `/def_send faust` with a `/fail` — "server built without faust support" — which the client {{alt raises || throws}} as a `CommandError`. If that happens against a server you did not build, that is what to check.

## Sending a def

A def sends itself: `sdef.send(server)`, `fdef.send(server)`, `gdef.send(server)` — one method on each family, no dispatch to pick. Sending is **asynchronous**: `/def_send faust` JIT-compiles on the server's network thread, answered later by `/done` or `/fail`. The barrier discipline mirrors scsynth:

```python
fdef.send(server)                 # RT: BLOCKS until /done (raises CommandError on /fail, ReplyTimeout on silence)
sdef.send(server, wait=False)     # fire-and-forget: only sends
server.sync()                     # barrier: /server_sync -> /server_sync.reply, waits for ALL earlier async work
Synth("fsine", {"freq": 330.0}, server=server)    # safe now — the def is installed
```

```ts
await fdef.send(server);            // RT: resolves on /done (rejects with CommandError on /fail, ReplyTimeout on silence)
sdef.send(server, { wait: false }); // fire-and-forget: only sends
await server.sync();                // barrier: /server_sync -> /server_sync.reply, waits for ALL earlier async work
new Synth("fsine", { freq: 330.0 }, { server });   // safe now — the def is installed
```

- {{alt `wait=True` (the default) blocks on `/done`; `wait=False` || Waiting (the default) resolves on `/done`; `{ wait: false }`}} only sends, after which `sync()` is the barrier before the `/synth_new` that needs the def.
- In **NRT** (a score {{alt interface || connection}}) `send` always *scores* the def at time 0 — the renderer compiles it before time advances — so `wait` does not apply.
- The server owns the def **table**: {{alt `server.free_def(*names)` || `server.freeDef(...names)`}} removes defs by name (`/def_free`). A def is not freed by itself — in use it is overwritten by sending another under the same name.
- `server` is optional: without it the def goes to the ambient session's server, like every other resource constructor.

There is one rule that overrides the convenience of the waiting default: **inside a routine, never {{alt block the clock thread || await}}.** Send the def {{alt `wait=False` || `{ wait: false }`}} and `yield` enough beats before the dependent `/synth_new`, rather than {{alt calling a blocking `sync()` || awaiting a `sync()`}}. See [Routines and clocks](routines-and-clocks.md) for why, and [Getting started](getting-started.md) and the [Examples](examples.md) for end-to-end defs that play.
