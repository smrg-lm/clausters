---
name: faust-language
description: How to write DSP in the Faust language and transpose it between Faust source and the clausters Signal API (Python `signals`) / Box API — the composition operators, sample-level feedback with `~`/`letrec` (filters, counters, phasors), physical modeling with pm.lib, and the mapping table between the three forms. Complements `clausters-python` (idiomatic client use) and `faust-embedding` (the server-side libfaust C API). Consult when writing or translating any Faust DSP for a FaustDef.
---

# Writing Faust and transposing it to the clausters APIs

One def, three source forms, all landing on the same `/d_faust` + LLVM JIT:

| form | payload | when |
|---|---|---|
| **Faust source** | the `.dsp` text verbatim (`FaustDef.from_source`) | full language: `import("stdfaust.lib")` works (the JIT ships the stdlib), `letrec`, pattern matching, iterations, **pm.lib** — anything the compiler accepts |
| **signal tree** | `{"signals":[…]}` JSON (`FaustDef.from_signals`, built with `clausters.defs.signals`) | building graphs *programmatically from Python* — composable `Signal` objects, no string templating |
| **box tree** | JSON box algebra (`FaustDef.from_box`) | rare; mostly the GUI host / machine-generated graphs. `seq`/`par`/`split`/`merge`/`rec` ops, `wire` (`_`), `cut` (`!`) |

Rule of thumb: **prototype in Faust source** (fast to read, libraries available),
**transpose to the signal API** when the graph must be assembled by Python code
(parametric voices, generated banks). Semantics are identical — the signal tree
is literally Faust's own signal stage.

## Faust language essentials

Five composition operators, by priority: `~` (recursion, highest), `,`
(parallel), `:` (sequential), `<:` (split), `:>` (merge, sums). Delays: `x'`
(one sample, alias `mem`), `x@n` (n samples, bounded, may be dynamic).
Local definitions with `with { … }`; difference equations with
`letrec { 'y = …; }` (the `'` on the *defined* name marks the implicit
z⁻¹). Iterations `par(i,N,e)` / `seq(i,N,e)` / `sum(i,N,e)` / `prod(i,N,e)`
unroll at compile time; pattern matching rewrites (`dup(1,x) = x; dup(n,x) =
x, dup(n-1,x);`). UI primitives are the **controls** in clausters: `hslider`,
`vslider`, `nentry`, `button`, `checkbox` — the label becomes the `/n_set`
control name (`in`/`out` are reserved: the server adds them for bus routing).

## Sample-level feedback (`~`) — the core idiom

`a ~ b`: the output of `a` goes through `b` back into `a`'s first input(s),
with an **implicit one-sample delay** in the loop. This is what UGen graphs
cannot do (their `LocalIn`/`LocalOut` feedback is one *block* = 64 samples;
see `docs/schemas.md` "Feedback"): **sub-block feedback must be fused into
one Faust def**. Three equivalent styles:

```faust
// 1. point-free: a one-pole lowpass in one line
lp = + ~ *(0.999);
// 2. named loop function (clearest for multi-state / mutual recursion)
lp(x) = loop ~ _ with { loop(fb) = x + 0.999 * fb; };
// 3. letrec (difference-equation notation)
lp(x) = y letrec { 'y = x + 0.999 * y; };
```

**Feedback is also the language's iteration/state primitive**, not just DSP:

```faust
counter   = (+(1) ~ _) - 1;                // 0,1,2,… (this is ba.time)
phasor(f) = (+(f/ma.SR) : ma.frac) ~ _;    // 0..1 ramp, the oscillator core
resettable(t) = (+(1) : *(t == 0)) ~ _;    // held at 0 while t>0, counts after
```

Mutual (multi-channel) recursion feeds tuples: `loop ~ (_,_)` (e.g. the
coupled-form quadrature oscillator). See Sanfilippo's *Three ways to implement
recursive circuits* (resources below) for the full comparison.

## Transposing: Faust ↔ Signal API

The Python `signals` module (`clients/python/clausters/defs/signals.py`) is a
1:1 surface over Faust's Signal API; the server maps it in
`src/faust/signals.rs`. `Signal` overloads Python operators, plain numbers are
constants (**Python `int` → Faust int, `float` → real** — write `1.0` for real
math), and each entry of `from_signals(name, *outputs)` is one output channel.

| Faust | signals (Python) |
|---|---|
| `x'` / `x@n` | `delay1(x)` / `delay(x, n)` |
| `expr ~ f` (single loop) | `rec(lambda s: …)` — `s` is the delayed output; or `recursion(body)` + `self_()` |
| `_` (nth audio input) | `input(n)` |
| `hslider("f",…)` etc. | `hslider("f", init, lo, hi, step)` — same five widgets, label = control name |
| `ma.SR` | `sr()` (reproduces the stdlib clamp) |
| `select2/3` | `select2(sel,a,b)` / `select3(…)` |
| `waveform{…}`, `rdtable`, `rwtable` | `waveform([…])`, `rdtable(size,init,ridx)`, `rwtable(…)` |
| `%`, comparison, bit ops | Python `%`, `>`, `&`, … (comparisons emit 0/1 signals, as in Faust) |
| foreign `fconstant`/`fvariable` | `fconst(ctype,name,file)` / `fvar(…)` |

```python
from clausters.defs import signals as sig
# counter = (+(1) ~ _) - 1      phasor = (+(f/SR) : frac) ~ _
count  = sig.rec(lambda s: s + 1) - 1      # ba.time: 0,1,2,…
freq   = sig.hslider("freq", 220.0, 20.0, 2000.0, 0.1)
phase  = sig.rec(lambda s: (s + freq / sig.sr()) % 1.0)
```

**Not exposed in the JSON trees** (use a source def instead): mutual/N-channel
recursion (`CsigRecursionN` — like box `~`, only *single* recursion is the
surface; restructure into nested single loops or write source), `lrsh`
(crashes Faust 2.81.10's typing), `soundfile`, and of course the macro layer
(`with`, pattern matching, `par/seq/sum/prod` — in Python, *loops build the
tree*, which is the whole point of the signal API).

Semantic errors (a `self_()` outside `recursion`, dangling inputs) are
reported by Faust itself through `/fail` — the server validates JSON shape
only. There is no `neg` primitive: the client emits `0 - x` (so does Faust).

## Physical modeling (source defs + pm.lib)

`import("stdfaust.lib")` gives `pm` — the physical modeling library. Its
paradigm: **bidirectional chains** — every element has left-going, right-going
and mixed I/O; `pm.chain(A : B : …)` composes them and generalizes the
one-sample delay across both directions; `pm.terminations(l, chain, r)` adds
reflective ends; `pm.in/inLeftWave/inRightWave` inject the excitation,
`pm.out*` tap; `pm.endChain` seals it. Building blocks: `pm.waveguide`,
`pm.stringSegment`, `pm.bridgeFilter`, `pm.modeFilter` (modal synthesis);
excitations `pm.impulseExcitation(gate)`, `pm.pluckString`, `pm.strikeModel`,
`pm.blower`; whole instruments `pm.ks` (Karplus-Strong), `pm.elecGuitar`,
`pm.violinModel`, `pm.clarinetModel`, `pm.djembeModel`, `pm.marimbaBarModel`.

A gate/trigger control drives the excitation — in clausters, a `button("gate")`
set via `/s_new … "gate" 1` or `/n_set` (and released back to 0), exactly like
a UGen-def `tr` control. Hand-rolled Karplus-Strong, to see the feedback
skeleton (a waveguide *is* `~` around a delay + damping filter):

```faust
import("stdfaust.lib");
freq = hslider("freq", 220, 50, 1000, 0.1);
gate = button("gate");
exc  = no.noise * en.ar(0.001, 0.01, gate);
process = exc : (+ : de.fdelay(4096, ma.SR/freq - 0.5)) ~ (fi.lowpass(1, 6000) * 0.995);
```

Faust's `soundfile("<bufnum>", n)` binds a **server buffer** by numeric label
(snapshot at `/s_new`; outputs `[length, sampleRate, ch0…]`) — the bridge for
sampled excitations and modal IRs; see `docs/schemas.md` "Soundfiles read
server buffers".

## Resources

- Faust syntax manual (operators, letrec, primitives): https://faustdoc.grame.fr/manual/syntax/
- D. Sanfilippo, *Three ways to implement recursive circuits in Faust*: https://www.dariosanfilippo.com/posts/2020/11/28/faust_recursive_circuits.html
- `pm.lib` reference: https://faustlibraries.grame.fr/libs/physmodels/ (source: https://github.com/grame-cncm/faustlibraries/blob/master/physmodels.lib)
- J. O. Smith, *Physical Audio Signal Processing* (waveguide theory behind pm.lib): https://www.dsprelated.com/freebooks/pasp/Virtual_Musical_Instruments.html
- Faust-STK (nonlinear physical models in Faust, all source-compatible with `/d_faust`): https://ccrma.stanford.edu/~rmichon/publications/doc/DAFx11-Faust-STK.pdf
- Worked reverb/feedback algorithms: https://github.com/LucaSpanedda/Digital_Reverberation_in_Faust
