# Defining instruments: FaustDef and SynthDef

An instrument is a **def** — a named processing graph the server compiles once and then instantiates many times as nodes. The client builds two kinds, both living in `clausters.defs`, and they are **peers**: neither is the "real" one, and a piece routinely uses both.

- **`FaustDef`** — a Faust definition, sent with `/d_faust` and JIT-compiled by the server into a node. Its graph is the full Faust language, so it reaches **below the unit**: per-sample recursion, tables, foreign constants, and the whole Faust library ecosystem. Build it three ways — the **Signal API** (`signals`), the **Box API** (`boxes`), or **Faust source** — all equal citizens (see [Building one](#building-one)). Like the SynthDef family, it works out of the box: the wheel bundles Faust (see [What the server must support](#what-the-server-must-support)).
- **`SynthDef`** — a UGen graph, sent with `/d_recv`. It wires the server's UGens (oscillator, noise, impulse, bus I/O, buffer playback, feedback, FFT chains) and the full unary/binary **maths** — the arithmetic operators plus `%`, `min`/`max`, comparisons, `.sin()`, `.midicps()`, `.distort()` … — which compose the generic operator UGens (see [Maths on a UGen graph](#maths-on-a-ugen-graph)). The graph is an **assembly of ready-made units**: it composes what the server already implements, needs no JIT, and works on **any** server build, since the SynthDef family is on by default.

Rule of thumb, not a hierarchy: reach for a `SynthDef` when the units you need already exist (and always for routing, buses and buffer playback); reach for a `FaustDef` when you want DSP the unit set does not cover, or want to write the DSP itself. Both run as ordinary nodes in the same tree and address each other through buses.

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

Three constructors, one per payload the server's `/d_faust` accepts. They are **three ways of writing Faust, not a main road and two detours** — the server compiles all three into the same kind of node, and the same def can be expressed in any of them:

| Constructor | Payload | Use it for |
| --- | --- | --- |
| `FaustDef.from_signals(name, *outputs)` | a **signal tree** built with `clausters.defs.signals` | graphs assembled one output at a time from arithmetic, feedback and controls (see [the signal API](#the-signal-api)) |
| `FaustDef.from_box(name, box)` | a **box tree** — a `Box` built with `clausters.defs.boxes`, or a raw dict | point-free graphs of composed processors, with the Faust **libraries** available as boxes (see [the box API](#the-box-api-reusing-the-faust-libraries)); the dict form for machine-generated trees |
| `FaustDef.from_source(name, src)` | a Faust **source** string | writing Faust directly — the whole language, `import`s included |

**From signals.** Each argument to `from_signals` is one **output** (a `Signal` or a number): one argument is mono, two is stereo, and so on.

```python
from clausters.defs import signals as S, FaustDef

freq = S.hslider("freq", 220.0, 20.0, 20000.0, 0.01)
phase = S.rec(lambda s: (s + freq / S.sr()) % 1.0)   # one-sample feedback phasor
sine = S.sin(phase * S.TAU) * 0.2
fdef = FaustDef.from_signals("fsine", sine)          # one output -> mono
```

**From boxes.** The same def as composed processors, with the Faust libraries in reach (`box.faust` compiles any Faust expression into a `Box`):

```python
from clausters.defs import boxes as box, FaustDef

freq = box.hslider("freq", 220.0, 20.0, 20000.0, 0.01)
fdef = FaustDef.from_box("bsine", box.faust("os.osc")(freq) * 0.2)
```

**From source.** When you would rather write the Faust itself:

```python
FaustDef.from_source("organ", 'import("stdfaust.lib"); process = os.osc(220) * 0.2;')
```

Which one to pick is a question of *how you are thinking about the graph* — one output at a time, blocks plugged into blocks, or the language — and is discussed under [Choosing a form](#choosing-a-form). Whichever you use, the def is sent, instantiated and controlled identically.

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

### The box API: reusing the Faust libraries

`clausters.defs.boxes` (imported as `box` by convention) is the same lowercase-callable pattern over Faust's **Box API**: the point-free algebra where whole **processors** compose by their input/output arities (`seq` / `par` / `split` / `merge` / `rec`). It is the counterpart of `signals` and a complete def-building API in its own right — where `signals` describes one output at a time referentially (`input(n)`), boxes describe multi-channel blocks that plug into each other, the natural shape for routing, chains, and anything conceived as units with inputs and outputs.

On top of the algebra, one addition opens the Faust libraries to it:

```python
from clausters.defs import boxes as box, FaustDef

lp = box.faust("fi.lowpass", 3)          # any Faust expression, stdlib included
```

`box.faust` compiles a Faust expression into a `Box` that is indistinguishable from a primitive — so the Faust libraries (`os.`, `fi.`, `re.`, `pm.`, ...) join the same algebra **without transcribing them**, composed with sliders and arithmetic built in Python.

**Two stages of application, kept apart in the syntax.** Faust applies in two different stages, and the module never mixes them in one argument list:

- Arguments to `box.faust(src, *eval_args)` are **evaluation-stage**: spliced into the source text as Faust application. `box.faust("fi.lowpass", 3)` compiles `fi.lowpass(3)`. Structural parameters — a filter order, a table size, a list of coefficients — must live here; they cannot travel as signals. Formatting: `int`/`float` as literals, a list/tuple as a Faust list `(a, b, c)`, a string verbatim (to pass an expression or a library function as an argument). `defs=` prepends helper definitions to the generated program.
- Arguments to **calling a `Box`** are **composition-stage**: boxes wired to the box's signal inputs, sugar for `seq(par(args), box)`. The call must cover *all* the inputs — use `box.wire()` for the ones left open:

```python
cutoff = box.hslider("cutoff", 800.0, 20.0, 8000.0, 0.1)
y = box.faust("fi.lowpass", 3)(cutoff, box.wire())    # fi.lowpass(3, cutoff, _)
```

Which inputs survive the partial application (here: `fi.lowpass(3)` expects `(fc, x)`) is the library function's business — read its documentation; a mismatch is reported by the Faust compiler itself, verbatim, in the `/fail` reply.

**The wire rule** — the big difference from `signals`: there is no `input(n)` here; **each `box.wire()` is a distinct input**. Two wires in two positions are two bus channels. Reusing the *same* wire object twice is almost always a mistake, and `from_box` rejects it with an error; route explicitly with `box.split`, or write that stretch inside the fragment (`"_ <: ..."`). Every *other* box value reuses freely — a repeated subexpression (an oscillator used by two branches) is **computed once**; the server shares identical subtrees.

**Channel selection.** The JSON carries no arities; the client computes them from the composition rules, except for fragments — only the Faust compiler knows those, so declare `outs=` (and `ins=` if useful) when you need to split channels:

```python
st = box.faust("re.stereo_freeverb", 0.8, 0.7, 0.5, 23, outs=2)(dry, dry)
left, right = st.outs()                               # or st[0], st[1]
```

The composition surface mirrors the server's box schema one to one: `seq` / `par` / `split` / `merge` (n-ary, folded left), `rec(a, b)` (point-free `~`; for the `rec(lambda s: ...)` style use `signals` or a fragment), `wire` / `cut`, `delay` / `delay1`, `select2` / `select3`, the same controls, groups (`hgroup` / `vgroup`), foreign values (`fconst` / `fvar` / `sr()`) and tables (`waveform` / `rdtable` / `rwtable`) as the signal API, and the same operators on `Box` (with `%` mapping to Faust's `fmod`; the box schema has no shifts). See the [API reference](api.md) for signatures.

### Choosing a form

The three forms are interchangeable on the wire, so the choice is about which one *says what you mean* — and they mix freely inside one piece (and even inside one def, since `box.faust` embeds source into a box tree).

| Write it as | When the graph is |
| --- | --- |
| **source** (`from_source`) | a **fixed chain** read top to bottom (`os.osc(freq) : fi.lowpass(3, 800) <: re.stereo_freeverb(...)`), or a **regular bank** — "N copies with index-dependent parameters", which Faust iterates at compile time (`par(i, N, ...)`, labels with `%i`, `ba.take(i+1, list)`). Parametrize it from Python with an f-string that splices `N` and the lists. |
| **boxes** (`from_box`) | **conceived as composed processors** — point-free routing built in Python; its **structure decided by Python data** (a different shape per element, conditionals over analysis or configuration); **library DSP mixed with Python-built pieces** (the `box.faust` fragments); or **machine-generated** (the raw-dict form). |
| **signals** (`from_signals`) | described **one output at a time**, referentially — arithmetic, comparisons, feedback (`rec`) and tables written as expressions over named values, the shape of a filter or an oscillator you are deriving rather than wiring. |

Two graphs, side by side — a fixed chain (left) belongs in source; a data-driven bank (right) belongs in boxes:

```python
# fixed chain: write Faust            # irregular bank: build with boxes
FaustDef.from_source("soft", """      def voice(f, kind):
import("stdfaust.lib");                   src = ("os.osc" if kind == "sine"
freq = hslider("freq",220,20,2000,.1);           else "os.sawtooth")
process = os.osc(freq) * 0.2              return box.faust(src)(f) * 0.2
  : fi.lowpass(3, 800)                mix = sum(voice(f, k) for f, k in spec)
  <: re.stereo_freeverb(.8,.7,.5,23); fdef = FaustDef.from_box("bank", mix)
""")
```

### Controls and reserved ports

`fdef.control_names()` lists the control names the def declares, in tree order — the UI labels, deduplicated. On top of those, every Faust synth also accepts the two **reserved** bus-selecting controls the server adds (`fdef.reserved == ("out", "in")`): set them at `/s_new` time (`"in" bus "out" bus`) to choose the input and output buses. They are not declared in the graph.

`fdef.dump_def()` returns the wire payload (JSON for a signal/box tree, the string itself for a source).

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

Each **UGen output** also carries a calculation **rate** — `ir` (init), `kr` (control), `ar` (audio), `dr` (demand). It defaults per kind (signal UGens are `ar`, `rand` / `sample_rate` are `ir`, the demand sources are `dr`); set it explicitly with `Ugen.at_rate`, e.g. `sine(5.0).at_rate("kr")` for a control-rate LFO. The full rate model and its coercion rules live in the [Clausters server book](https://clausters.readthedocs.io/) (schemas / OSC reference).

### The UGen set

`clausters.defs.ugens` exposes the UGen callables the client implements today — a subset of the server's registry. The set is small for now:

| Group | Callable | Does |
| --- | --- | --- |
| Sources | `sine(freq=440.0)` | sine by f64 phase accumulation, starting at phase 0 |
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
| Side-effect | `send_trig(trig, id, value)` | on each trigger, sends `/tr nodeID id value`; output is silence |
| | `send_reply(trig, *values, cmd="/reply", reply_id=-1)` | sends a custom OSC message with an arbitrary value list |
| | `poll(trig, signal, label="poll", trig_id=-1)` | posts `signal` to the server console (and a `/tr` when `trig_id >= 0`); passes `signal` through |
| Spectral | `fft(source, active=1.0, *, fft_size=1024, hop=0.5, wintype=0)` | opens a spectral chain (windows and transforms `source` per hop) |
| | `pv_mag_above(chain, threshold)` / `pv_mag_below(chain, threshold)` | pass bins above / below a magnitude threshold |
| | `pv_brick_wall(chain, wipe)` | zero a band (`wipe > 0` low pass, `< 0` high pass) |
| | `ifft(chain)` | closes a spectral chain (resynthesises audio by overlap-add) |

Like Faust synths, a SynthDef also accepts the reserved `in` / `out` bus-selecting controls the server adds at `/s_new` time.

The **side-effect** UGens exist for a reply or a console post rather than audio, so a def may consist of them alone with **no `out`** — pass them as `SynthDef` roots (see [Building one](#building-one)). The **spectral** UGens form a frequency-domain chain — see [The frequency-domain chain](#the-frequency-domain-chain) below.

### Maths on a UGen graph

Beyond the four arithmetic operators, the **full unary/binary maths** works on a SynthDef graph: `%`, `min`/`max`, the comparisons, `.sin()`, `.midicps()`, `.distort()`, `.clip2(x)` and the rest compose the server's generic **`BinaryOpUGen`/`UnaryOpUGen`**, each carrying the operator by name. It is the same operation set the value side (`clausters.base.builtins`) computes — one shared `clausters-core` implementation — so a value you compute ahead of time and the UGen on the audio thread agree **bit-for-bit** for the native ops.

```python
from clausters.defs import SynthDef, control, sine, out

note = control("note", 60.0)
freq = note.midicps()                     # UnaryOpUGen (midicps)
sig = sine(freq).distort() * 0.3       # UnaryOpUGen (distort), then Mul
lfo = (sine(5.0) * 0.5 + 0.5).clip2(0.8)  # % min max > .fold/.clip … all compose
sdef = SynthDef("lead", out(0.0, sig * lfo))
```

The `+ - * /` operators keep their dedicated `Add`/`Sub`/`Mul`/`Div` kinds (so existing defs are byte-identical); every other operator becomes an op UGen. Operators and math methods come from the shared `AbstractObject`, so the *same* expression composes a Faust graph, a UGen graph, or concrete numbers depending on what it is applied to. Only a selector with no server op raises `TypeError`. A `FaustDef` is still the tool for genuinely custom per-sample DSP (recursion, tables, sample-accurate feedback); a SynthDef now covers ordinary maths as well as wiring.

Feedback within a block uses the `LocalIn` / `LocalOut` pair. `LocalIn` must be emitted before its `LocalOut`; the topological walk guarantees that as long as the output graph reaches the `local_in` before the `local_out`. Make the `local_out` one of the def's outputs so its write stays in the graph:

```python
from clausters.defs import SynthDef, sine, local_in, local_out, out

fb = local_in(0.0)                            # private channel 0
sig = sine(440.0) * 0.2 + fb * 0.5
echo = local_out(0.0, sig)                    # writes channel 0, passes sig through
sdef = SynthDef("fb", out(0.0, sig), echo)    # echo as an output keeps the write
```

### The frequency-domain chain

Spectral processing wires an `fft` to an `ifft` with any number of `pv_*` filters between them: `fft` windows an audio signal and transforms it to a spectral frame once per hop, the `pv_*` filters mutate the frame, and `ifft` resynthesises audio by overlap-add. Wire them in order — the output of one is the input of the next:

```python
from clausters.defs import SynthDef, white_noise, fft, pv_brick_wall, ifft, out

chain = fft(white_noise() * 0.25, fft_size=1024, hop=0.5, wintype=0)
chain = pv_brick_wall(chain, 0.75)      # keep the bottom 25% of bins (a low pass)
sdef = SynthDef("spec_lp", out(0.0, ifft(chain)))
```

The FFT frame is **synth-private scratch on the server** (SuperCollider's `LocalBuf` model) — no buffer is allocated and none is required. Only `fft` names the window size, hop and window type (they size the transform); the server propagates them to the rest of the chain. A bare `fft` → `ifft` reconstructs the signal at unity gain, delayed by one window.

The window type is also settable **live** with `Server.u_cmd`, which addresses one UGen instance inside a running synth:

```python
from clausters._native import Window
server.u_cmd(synth, fft_index, "window", Window.BLACKMAN)  # swap the FFT window
```

where `fft_index` is the FFT's position in the serialized graph. The smoothing windows themselves are shared with the server through the native core — `clausters._native.window(Window.HANN, n)` returns the **same** coefficients the server's FFT applies, so a client that pre-windows audio matches the server bit for bit.

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
| Built from | `clausters.defs.signals` / `clausters.defs.boxes` / Faust source | `clausters.defs.ugens` |
| Compiled | JIT on the server (libfaust/LLVM) | assembled from the server's UGen registry |
| Maths | full (trig, `exp`/`log`, comparisons, tables) | full (the same operator set, as generic op UGens) |
| Feedback | `rec` / `self_` (one sample) | `local_in` / `local_out` (one block) |
| Granularity | **per sample** — you write the DSP | **per unit** — you wire DSP the server implements |
| Reuse | the Faust libraries (`os.`, `fi.`, `re.`, `pm.`, …) | the UGen set (see the table above) |
| Server build | the `faust` feature — default, and bundled in the wheel | the `synth` feature — default |

Neither is the fallback of the other. The units a `SynthDef` wires are the fastest way to say "an oscillator into a bus", they need no compiler on the server, and they are the only way to reach the server-side machinery that has no Faust equivalent (buffer playback, bus I/O, `send_reply`/`poll`, the FFT chain). A `FaustDef` is how you write DSP that is *not* in the unit set — a filter you derived, a physical model, an algorithm — either from Python (`signals` / `boxes`) or in Faust itself, with its libraries at hand.

The common pattern is to combine them: a FaustDef for the voice, a SynthDef to route it, play back buffers or analyse the result. Both run as ordinary nodes in the same tree, on the same buses, and are controlled by the same `/n_set`.

## What the server must support

Both def families are **on by default** on the server (`synth` and `faust` are Cargo features, and both are in the default set), and everything the wheel ships is built that way: the standalone `clausters` binary, the in-process embedded server and the offline renderer all take a `SynthDef` *and* a `FaustDef`. The package even bundles libfaust and the libLLVM it JITs with, so Faust works on a machine with neither installed — nothing to enable, nothing to build.

The one case where a `FaustDef` can fail is a server **someone built without the feature** (`--no-default-features`, e.g. a minimal SynthDef-only build). It answers `/d_faust` with a `/fail` — "server built without faust support" — which the client raises as a `CommandError`. If that happens against a server you did not build, that is what to check.

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
