---
name: faust-embedding
description: How to embed libfaust in the server for JIT-compiling synthesis nodes at runtime — the C API (box/signal/LLVM factory), factory/instance lifecycle and its RT-safety boundaries, parameter zones via UIGlue, JSON→Box API mapping guidance, build/link and licensing. Consult for anything in the F milestones (Faust fork of the plan).
---

# Embedding libfaust (Box/Signal API + LLVM JIT)

The model, as used by FaustLive/faustgen: build a **box expression** through API
calls (no .dsp source needed), compile it to a **factory** (LLVM JIT → native
code), create **instances** from the factory, call `compute()` per block.

## The C API surface (bind these with bindgen)

Headers shipped with libfaust:

- `faust/dsp/libfaust-c.h` — context, misc.
- `faust/dsp/libfaust-box-c.h` — the Box API (`Cbox*` functions).
- `faust/dsp/libfaust-signal-c.h` — the lower-level Signal API (`Csig*`).
- `faust/dsp/llvm-dsp-c.h` — LLVM factory/instance (`llvm_dsp_factory`, `llvm_dsp`).
- `faust/gui/CUI.h` — `UIGlue` for parameter discovery.

## Lifecycle (and which thread owns each step)

```text
createLibContext()                      compiler thread, per compilation
  box = CboxPar(CboxWire(), ...)        compiler thread (cheap, but inside context)
  factory = createCDSPFactoryFromBoxes( compiler thread (SLOW: 10s-100s of ms)
              name, box, argc, argv, "", err, -1)
destroyLibContext()                     compiler thread

dsp = createCDSPInstance(factory)       network/compiler thread (allocates!)
initCDSPInstance(dsp, sample_rate)      network/compiler thread (allocates, fixes SR)
buildUserInterfaceCDSPInstance(dsp,&ui) network/compiler thread (collect param zones)

computeCDSPInstance(dsp, 64, ins, outs) audio thread (RT-safe after init)

deleteCDSPInstance(dsp)                 non-RT thread, via garbage FIFO
deleteCDSPFactory(factory)              non-RT thread, ONLY when no instances remain
```

Hard rules:

- **The lib context is global and not thread-safe**: one compilation at a time.
  A single dedicated compiler thread serializes everything naturally.
- **`compute()` is the only call allowed on the audio thread.** Everything else
  allocates or locks.
- **Destroying a factory while its instances live is UB.** Refcount factories
  (e.g. `Arc`/`basedrop::Shared` wrapper that holds the factory until the last
  instance's garbage is collected).
- `initCDSPInstance` freezes the sample rate. Device SR change → re-instantiate.
- The error buffer (`char err[4096]`) must be passed in; on failure the factory
  is NULL and `err` holds a human-readable message — forward it to the client
  in the `/fail` reply.

## Box API cheat sheet

Primitives and composition (all return `Box`, valid only inside the context):

```text
CboxInt(n)  CboxReal(x)  CboxWire()  CboxCut()
CboxSeq(a,b)   a : b      CboxPar(a,b)    a , b
CboxSplit(a,b) a <: b     CboxMerge(a,b)  a :> b
CboxRec(a,b)   a ~ b      (feedback — this is how phasors/filters are built)
CboxAdd(a,b) CboxSub CboxMul CboxDiv CboxFmod CboxPow ...
CboxSin(x) CboxCos CboxTan CboxExp CboxLog CboxSqrt CboxAbs CboxFloor ...
CboxDelay(x, d)            x@d
CboxIntCast(x) CboxFloatCast(x)
CboxSelect2(sel,a,b) CboxSelect3(...)
CboxWaveform(...)  CboxSoundfile(...)
```

UI elements = named parameters (these become our `/n_set` controls):

```text
CboxHSlider(label, init, min, max, step)
CboxVSlider / CboxNumEntry  (same signature)
CboxButton(label)  CboxCheckbox(label)
CboxHGroup/CboxVGroup(label, box)   (grouping → parameter paths)
```

Example — a 440 Hz sine with controllable freq, no stdlib:

```text
phasor(f) = (f / SR) ~ wrap        where wrap = x - floor(x)  via CboxRec
sine      = CboxSin(CboxMul(twopi, phasor(CboxHSlider("freq",440,20,20000,0.01))))
out       = CboxMul(sine, CboxHSlider("amp", 0.2, 0, 1, 0.001))
```

Accessing the Faust stdlib (`os.osc`, `fi.lowpass`, …) from the box world: use
`CDSPToBoxes(name, "import(\"stdfaust.lib\"); process = os.osc(440);", argc,
argv, &inputs, &outputs, err)` — it compiles Faust *source* into a Box you can
compose with other boxes. This lets the JSON schema embed Faust source
fragments where primitives would be painful.

## Parameters: UIGlue zones

`buildUserInterfaceCDSPInstance` walks the UI tree calling your `UIGlue`
callbacks with `(label, FAUSTFLOAT* zone, init, min, max, step)`. Collect
`label → zone` into the synth's control map at instantiation time (non-RT).
At `/n_set` time the audio thread just writes the `f32` through the pointer —
a plain aligned store, RT-safe. (Faust reads zones at block boundaries; no
torn-read concern for f32.)

`getNumInputsCDSPInstance` / `getNumOutputsCDSPInstance` give the I/O arity for
bus mapping. Faust buffers are **non-interleaved** `float**` — matches our
per-bus layout, no conversion needed.

## Build and link

- Compile-time: `FAUSTFLOAT` must be `float` (f32) — it is the default, but
  assert `sizeof` via a static check in the binding.
- Link: `-lfaust` plus LLVM (`llvm-config --libs` or the distro's monolithic
  `libLLVM.so`). Debian/Ubuntu: `libfaust-dev` (check version ≥ 2.50 for a
  stable box C API) or build from source with `-DINCLUDE_LLVM=ON`.
- Keep everything behind the `faust` cargo feature: optional dependency, the
  core server must build and test without libfaust installed.
- Factory caching: `writeCDSPFactoryToMachine*` / `readCDSPFactoryFromMachine*`
  serialize JIT-ed code — useful later to skip recompiles of known defs.

## JSON → Box mapping guidance

- Mirror the API one-to-one: `{"op": "par", "in": [a, b]}`,
  `{"op": "hslider", "label": "freq", "init": 440, "min": 20, "max": 20000,
  "step": 0.01}`, `{"op": "faust", "src": "process = os.osc(440);"}` (the
  DSPToBoxes escape hatch). Don't invent a higher-level schema in v1 — the
  whole point is that the instruction set is Faust's, not ours.
- Validate before touching the C API and report the JSON path of the offending
  node (`"in[1].op: unknown 'mul3'"`); after construction, compilation errors
  from `err` go back verbatim in `/fail`.
- Box construction calls are cheap but must happen inside the lib context on
  the compiler thread — build the whole tree there, not incrementally over OSC.

## Licensing

Not an issue for this project: Clausters is **GPLv3-or-later**, libfaust is
GPLv2-or-later — the combination distributes as GPLv3+. (Code *generated* by
Faust carries no license restriction either.)

## Testing

- Offline parity goldens: same graph as UGen (`SinOsc`) and as box
  (`sin(2π·phasor)`), rendered through the same engine — outputs must match
  within tolerance (see [[audio-testing]]).
- The compiler thread is async: tests drive it with explicit completion waits
  (reply or condvar), never sleeps.
- RT guardian: `assert_no_alloc` covers `compute()` too once FaustSynth is in
  the tree — the existing rt_safety test should grow a Faust scene (behind the
  feature flag).
