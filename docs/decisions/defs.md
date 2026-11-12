# Defs and UGens

Part of the [design record](../decisions.md), whose index lists every
entry's title.

## `SynthNode`: one trait, symmetric def families

The node tree and FIFOs handle `Box<dyn SynthNode>`, never a concrete synth
type. This was introduced before it was strictly needed so that a second def
family (Faust) could join the tree without touching the engine or the tree.
Consequence: the `synth` (SynthDef/UGen) and `faust` (FaustDef) families are
independent Cargo features that combine freely — new work feature-gates against
`dyn SynthNode` and stays symmetric.

## Def persistence: transparent JSON + a non-authoritative bitcode cache

Two layers, decided with the user:

- **B — JSON is the transparent source of truth.** `synthdefs/<name>.json` is the
  `SynthDefSpec` verbatim; `faustdefs/<name>.json` is a `FaustRecord`
  (source/JSON + libfaust version + payload sha256). Reload = recompile from
  there, by the same path as a fresh `/def_send`. The `FaustDef` itself
  is never serialized — its factory is opaque LLVM JIT state.
- **A — the LLVM bitcode cache is non-authoritative.** `faustdefs/<name>.<sha16>.bc`
  is restored only if the libfaust version matches and the file reads cleanly; any
  miss recompiles from source and rewrites it. A libfaust upgrade invalidates
  every `.bc` automatically, and a corrupt cache never serves a wrong def
  (named by payload sha, so a stale `.bc` never pairs with a newer record).

## `SinOsc` is renamed `Sine`: the name follows the implementation

The first oscillator kept scsynth's name, `SinOsc`, from the M0 walking
skeleton onward. But scsynth's `SinOsc` is a wavetable oscillator (8192-point
table, linear interpolation) — ours is direct phase accumulation calling
`sin()` per sample with the phase held in `f64`, which is *more* precise
(no interpolation noise, tuning stable over long sessions) on hardware where
a `sin()` per sample stopped being the bottleneck decades ago. Once the real
wavetable family landed (`Osc`/`OscN`/`VOsc` + `Shaper`, reading user-visible
buffers), the borrowed name claimed an implementation the UGen does not have.

**Decision:** the kind is `Sine` (wire), `Sine` (Rust), `sine` (Python) —
named for the signal it produces, not for a lookup strategy it does not use.
The bare `Sin`/`sin` was rejected: `sin` is already the unary math operator
(`UnaryOpUGen`'s selector, `.sin()` on `Ugen`, `signals.sin`/`boxes.sin`),
and an oscillator and a waveshaper must not share a name. Pre-1.0 wire
rename, no compatibility alias.

**Consequence:** the wire vocabulary reserves the `*Osc` spelling for the
table-reading family, and a future table-based sine (if massive-additive
profiling ever justifies one) can take a new name instead of redefining this
one.

## Multichannel is an explicit container, not implicit expansion

sclang expands multichannel implicitly: an array argument anywhere fans the
whole UGen call out, invisibly. The Python client takes the explicit half of
that design (following the sc3 client, which reduced expansion to an array of
UGens — here `ChannelList`): `dup` fans out, operators broadcast/zip over the
container, `out` lays channels on consecutive buses, `mix` folds back. A
channel list reaching a single-channel input is a serialization `TypeError`,
and `sine([440, 443])` does not expand.

**Decision details with non-obvious context:**

- **dup duplicates by reference; a callable is evaluated.** Reference dup +
  identity dedup serializes the shared node once — mono→N for free — while
  `dup(white_noise, 8)` by reference would be eight copies of the *same*
  noise (correlated, almost never wanted). The callable form builds distinct
  nodes, mirroring sclang's `ugen.dup` vs `{ }.dup` without needing an
  AbstractFunction port: `dup(callable, n)` covers the one use it would have.
- **Zip wraps the shorter side modulo** — not an error — because that is
  already the client's rule for plain lists on the value side
  (`clausters.base.builtins._extend`), and graph maths and value maths must
  agree; it is also the sclang behavior users' habits assume.
- **The container never crosses the wire.** The server's spec stays
  single-channel-per-UGen; a future multi-output UGen (a panner, a stereo
  buffer reader) will need a wire extension, and its client-side return type
  is then naturally a `ChannelList` — the container is forward-compatible
  with that without re-design.
- **The container carries two roles, told apart by what its members are.**
  Before an output it is a *multichannel signal*; after one it is a *bundle of
  def roots* (`out(bus, chans)` returns a `ChannelList` of `Out`s, which
  `SynthDef` flattens into its varargs roots). The coercion tells them apart
  per member rather than by type, which is the wart the forward-compatibility
  note above did not anticipate: it addressed the multi-output UGen and not
  this. It is contained rather than removed — a member that is a sink passes
  through as a root, the rest are the channels, so the two pure cases and the
  mixed one all have one rule — and separating the roles into distinct types
  stays open.
- Per-argument expansion is deferred, not rejected: every constructor funnels
  through `Ugen(kind, inputs)`, so one hook there can add full expansion
  later, desugaring to the same container. The rules above are written down
  in the composition docs as the spec a later client ports.

## A def root is what delivers data out of the graph

The client wraps a bare expression in `out(0, …)` to make it a def, and skips
the wrapping when the expression already is where it is going. Naming that set
after *side effects* — the obvious reading — is wrong, and the counter-example
is load-bearing: `FreeSelf`, `PauseSelf`, `FreeSelfWhenDone` and `Done` all have
side effects and all **pass their input through**, so `out(0,
free_self_when_done(env * sig))` is the idiom and treating them as roots would
make the def silent. `DetectSilence` is the same shape from the other side: its
done action is a side effect, but its output is a 0/1 signal for the rest of the
graph.

**Decision:** the criterion is **delivery** — a root is a UGen that puts data
*outside* the graph: audio or control on a bus (`Out`, `ReplaceOut`, `OutCtl`,
`LocalOut`), audio in a file (`DiskOut`), OSC or a console line at a client
(`SendTrig`, `SendReply`, `Poll`). Managing the enclosing synth is not
delivering, and neither is feeding the graph.

**Consequence:** `DiskOut` joins the set even though it passes its input
through, so `play(disk_out(path, sig))` records without sounding; recording
*and* hearing is the explicit `out(0, disk_out(path, sig))`. The exception was
declined deliberately — one sentence that always holds beats a shorter set with
a footnote.

## A def name identifies one def, and a generated name says so

Def names are a **single namespace across the three kinds**, and a generated
name carries a unique id plus a `tmp_` prefix that keeps it off disk. The bug
that forced the question: the Python client named ephemeral defs `_expr_<n>`
from a per-process counter, shared by both def families, and the server
persists every def it receives. A second session reused `_expr_2`, the store
still held a *mono* SynthDef under that name from the first, the server
resolved the name against the SynthDef table before the Faust one — and a
stereo FaustDef reported writing one bus. Nothing was corrupt; three
individually reasonable decisions composed into a silent wrong answer.

**Decision:**

- **One namespace.** Sending a def under a name another kind holds replaces
  it, last one wins, deleting the loser's persisted files. Rejecting the
  send was considered and dropped: replacement is already the rule *within* a
  kind, and a client re-sending a name means it, whatever kind it used before.
- **Generated names are `tmp_<kind>_<id>`** with a random id — unique across
  processes and runs (a counter restarts at 0, and the store outlives the
  process), and distinct per kind.
- **`tmp_` means never persisted.** A name generated for one expression means
  nothing to a later session. Whatever such a def must still write — only the
  Faust record and its bitcode — goes under the OS temp directory.

**Why a name prefix rather than a wire flag.** Persistence is a property of
the *def*, and the name already travels with it everywhere: no per-command
argument, no ABI move, and a log line or a `/def_query` listing says which defs
are throwaway without consulting anything. The cost is that a user def named
`tmp_...` is ephemeral too — which is the documented meaning of the prefix
rather than a leak, the same way a leading underscore means private in Python.

**What the failure taught.** A generated name is an identifier with a lifetime,
not a formatting detail: the counter's flaw was invisible until it met a store
that outlives the process, and then presented as a DSP bug. Uniqueness in a
namespace that persists must be global, not per-process.

## The PV set is curated and parameterized, never a per-op catalog

scsynth grew one UGen per spectral operation — `PV_MagAbove` and `PV_MagBelow`
are one algorithm and a boolean, the sc3-plugins tail is dozens of
near-duplicate C plugins with inconsistent parameter conventions and no
maintenance — and the vocabulary is *closed*: a user who wants a bin operation
the catalog lacks must write a server plugin (sclang's `pvcalc`/`pvcollect`
escape hatch is the admission). Clausters takes the opposite stance (M27): the
`PV_*` vocabulary grows only as **parameterized implementations** under
scsynth-compatible registered names — `PvMag` is one filter behind
`PV_MagAbove`/`Below`/`Clip`, `PvCombine` is one two-chain combiner behind
`PV_Add`/`Mul`/`Min`/`Max`/`MagMul`/`CopyPhase`, `PvBinShift` one remap behind
`PV_BinShift`/`PV_MagShift` — and an operation outside the curated set is
declined in favor of the long-term mechanism (a user-programmable per-frame
kernel, the M29 design spike), not added as another name.

**Decision details with non-obvious context:**

- **The structural fact behind the stance**: all the machinery lives in the
  bookends (windowing, transform, hop, COLA overlap-add — `FFT`/`IFFT`); a
  PV op is a trivial per-frame loop. Its real cost is registry, wire name,
  client binding, docs and tests — which is why near-duplicates are pure debt.
- **The combiner needed the one piece of engine work** (`SpectralRole::
  Filter2`): a spectral UGen reading two chain slots, result in chain A. The
  compiler enforces equal window sizes and distinct chains; the instance
  split-borrows the two synth-private chains. Everything else in M27 is loose
  rows over the S8 substrate.
- **B's latest frame, not a barrier**: a combiner acts when chain A has a
  fresh frame and reads whatever frame B holds. Two same-config `FFT`s in one
  synth hop on the same blocks anyway (the S11 stagger is per *node*, not per
  UGen), so the frames align in practice without any cross-chain
  synchronization machinery.

## Convolution: one UGen, kernels prepared off-RT, load spread across the hop

scsynth ships five convolution UGens (`Convolution`/`2`/`2L`/`3`/
`StereoConvolution2L`) whose names encode parameters (kernel source, swap
interpolation, time vs spectral domain, channel count), and its `Convolution2`
re-transforms the kernel *inside the audio callback* on a swap trigger — a
cost spike that violates every RT rule Clausters has. M28 takes the opposite
shape: **one** `Conv` UGen (uniformly partitioned overlap-save with a
frequency-domain delay line), with the kernel spectra computed **once, off the
audio thread**, by the typed `/buffer_gen prepare_partconv` routine into an
ordinary immutable pool buffer — the scsynth `PartConv`+`PreparePartConv`
lineage, which was always the one design compatible with the immutable-buffer
pool and the no-allocation callback.

**Decision details with non-obvious context:**

- **Load spreading is the design constraint, not an optimization.** A uniform
  FDL does all its partition MACs on the hop block (~217 us for a 2 s IR —
  measured in the bench before the implementation existed), a sawtooth the
  block budget cannot absorb at low latency. The `p >= 1` terms of hop `n+1`
  depend only on spectra present after hop `n`, so `Conv` accumulates them
  across the intervening blocks and the hop block adds only the input
  FFT/IFFT pair and the `p = 0` term (~36 us vs ~6 us steady, bench-verified
  with per-phase minima). This pairs with the S11 stagger as the two halves
  of "spectral load must be flat".
- **The FDL is input history, so kernel swaps are cheap**: swapping never
  rebuilds state; the swap hop computes the outgoing kernel's tail and a full
  fresh sum, crossfading over one partition (the `Convolution2L` behavior).
  Regenerating the *same* buffer index is a hard switch — the old spectra are
  gone (the pool swapped the Arc), so there is nothing to fade from; the docs
  say to use a fresh buffer when the transition matters.
- **Static `fft_size`/`partitions` instead of buffer-driven sizing**: the
  FDL and scratch must pre-allocate at build (network thread), so the def
  declares the partition size and the maximum kernel length; a mismatched
  kernel plays silence rather than resizing. scsynth's `PartConv` sizes from
  the spec buffer at construction — equivalent in effect, but our form keeps
  the kernel freely swappable at runtime within the declared capacity.
- **Latency is reported, not compensated**: `Conv` is the first UGen with
  intrinsic latency (one partition), so it lands the `latency()` hook on
  `UGen`/`SynthNode` that the auto-ordering work anticipated. Parallel-path
  compensation was deferred at introduction; the general server pass now
  consumes that hook (see `docs/model-vs-daw.md`).

## The spectral frame is user-programmable through a bin-expression program, not a new rate or a JIT kernel

The M29 spike weighed how to make the spectral frame user-programmable — the
long-term answer to the PV-catalog problem, so that a bin operation outside
the curated set stops requiring a server release. The plan named two
candidates: **(a) bin algebra** — magnitude/phase as frame-rate values the
existing operator vocabulary composes in the graph (the Max/MSP `pfft~`
model), and **(b) a JIT per-frame kernel** — a Faust callback over the frame
via the existing factory/instance patterns. The spike concludes on a third
design that dominates both on every axis the plan fixed: **a bin-expression
program** — one `PV_Kernel` UGen holding a compile-time-validated postfix
program over per-bin values (`mag`, `phase`, `bin`, `nbins`, parameters),
whose opcodes are the discriminants of the operator vocabulary that
`clausters-core::builtins` already single-sources between server and client,
evaluated per fresh frame by a tiny stack machine with a pre-allocated stack.
The user-visible surface is exactly (a)'s algebra — the client composes
symbolic `mag`/`phase` expressions with the operator overloads it already has
— but the evaluation strategy is an interpreter inside one UGen, not a new
compiler rate and not a JIT. (b) is not rejected but repositioned as the
escalation path for kernels that outgrow a per-bin map. The design is
implemented: `clausters_core::pvprog` (the program + evaluator), the
`PV_Kernel` row, and the client's `clausters.defs.pv_expr` symbolic terms.

**Decision details with non-obvious context:**

- **Why not the bin algebra as a rate (a)**: a true frame rate means signal
  vectors of length `winsize/2 + 1` — per *chain*, not global — advancing on
  a per-hop clock, not per block. That touches rate inference, output-buffer
  sizing (fixed at the block size today), a spectral exec mode on every
  operator UGen, and chain-slot threading through the whole expression — the
  largest compiler surface of the three for no expressiveness the program
  form lacks. The cheap sub-variant (fixed unpack/pack UGens over the demand
  substrate, scsynth's `Unpack1FFT`/`PackFFT`/`pvcollect` lineage) is the
  honest prior art, and it is the cautionary tale: either the graph unrolls
  per bin (unusable at 513 bins) or one expression is re-evaluated per bin
  through demand-UGen indirection — an expression evaluator with extra
  steps. `pfft~` does not transfer: Max's whole graph is already
  vector-per-buffer and its subpatch runs natively on the frame clock; a
  block-rate engine has no such substrate.
- **Why not the Faust kernel (b) as the general mechanism**: the spectral
  chain lives in the `synth` family, so a Faust kernel couples `synth` to
  `faust` — a new cell in the feature matrix, and the mechanism vanishes on
  builds without libfaust (the wrong shape for "the long-term answer").
  Mapping bins to samples (`compute(nbins)` with mag/phase as two input
  channels once per hop) fits Faust's stream model well, but the instance
  carries state across `compute` calls, so bin 0 of frame *N* silently sees
  state from the last bin of frame *N − 1* — cross-bin access falls out for
  free with a semantic trap at the seam. And NRT determinism weakens from
  "pure f32, bit-exact by construction" to "deterministic per
  libfaust/LLVM build" — golden files held hostage to a JIT version.
- **Why the program form wins**: the opcode table already exists as shared
  data — `BinaryOp`/`UnaryOp` in `clausters-core::builtins` (`from_name`,
  `apply_binary`/`apply_unary`), single-sourced between the server's op
  UGens and the client's value maths — so the VM adds load/store opcodes and
  a stack loop, nothing else. The program is validated on the network thread
  at def time (opcode validity, stack depth, parameter arity); the RT thread
  runs a fixed loop over pre-allocated scratch — zero allocation, pure f32,
  so RT ≡ NRT bit-exactly, on every feature combination. The authoring story
  is (a)'s: the client compiles overloaded-operator expressions over
  symbolic `mag`/`phase`/`bin` terms into the postfix program; only the
  execution strategy differs.
- **The honest limit, and the escalation path**: the program form is a
  per-bin map (plus cheap neighbor reads from the input frame if wanted).
  Cross-frame state (freeze, smear) and bin permutation (shift) stay curated
  parameterized implementations per the M27 stance — and a kernel that
  outgrows the map escalates to (b), implemented behind `synth`+`faust` only
  on demonstrated need. Every declined op remains the demand signal; this
  entry records the design so implementation can start the day one arrives.
- **The implemented shape**: the evaluator lives in `clausters-core` as a
  peer of `builtins` (`pvprog`); the def carries each program as a postfix
  token list in static fields (`mag_expr`/`phase_expr` — wire surface,
  operator *names* crossing the wire like `BinaryOpUGen`'s `op`); one
  `desc_spectral` row (`SpectralRole::Filter`, the first **variadic**
  spectral kind — inputs past the chain are the `p0…` parameters); the
  Python client's `clausters.defs.pv_expr` composes the expressions with the
  shared `AbstractObject` operators. The acceptance tests are the mechanism
  reproducing curated ops (`PV_BrickWall`, `PV_MagAbove`)
  **sample-identically** in `crates/clausters-server/tests/offline/spectral.rs`.
- **The conversion-cost risks resolved themselves in the identity path**: an
  identity phase program (the common, pure-magnitude case) keeps each bin's
  phase by scaling the complex pair — the exact arithmetic of the curated
  filters, no `atan2`/`cos`/`sin` at all (which is what makes the
  equivalence tests exact rather than approximate); the polar phase is
  computed only when a program reads it. The packed layout's real-only
  DC/Nyquist slots go through the same `get_bin`/`set_bin` accessors as the
  curated ops (imaginary part dropped there), and a zero-magnitude bin's
  phase is `atan2(0, 0) = 0` — no NaN can enter the frame.

## EnvGen: a gate already closed at the first sample is a release, not a wait

Context (found through the piano widget, but a server-wide property): a live
client's note-on (`/synth_new … gate 1`) and its note-off (`/node_set … gate 0`) can
land in the **same command drain** — the engine drains the whole FIFO at block
start, and both messages may have accumulated during one audio callback
interval (a PipeWire quantum is ~20 ms; any note shorter than that can lose
the race). The gate control is then already `0` before the node's first block,
so the envelope never sees a rising *or* falling edge. The old behavior played
the attack/decay anyway and **sustained forever on a closed gate** — a stuck,
audible node whose tree entry even shows `gate: 0`.

Decision: `EnvGen` treats a gate found already closed **on its very first
sample** as a release edge from `initLevel` — the envelope plays the release
segment out (silently, from the initial level) and finishes, so the
`doneAction` still frees the node. scsynth instead holds such an envelope at
`initLevel` waiting for the gate — silent, but the node leaks the same way;
live clients there avoid the race with timetagged bundles, which an
interactive keyboard cannot use. The one pattern this forgoes — spawning a
voice with `gate 0` to open later — still works while the release tail lasts
(the rising gate retriggers from `initLevel`), and beyond that was never
reliable against `FREE_SELF` anyway. Guarded by
`crates/clausters-server/tests/offline/envgen.rs::gate_already_closed_at_the_first_block_releases_and_frees`.

## The UGen registry owns its input names; the client tables become mirrors

`/ugen_query` needed the catalog to report each UGen's inputs, and the descriptors
in `src/dsp/registry.rs` carried only an *arity* — a count. The names existed,
but in two places that no client can consult: the catalog table in
`docs/schemas.md` (prose) and the parameter names of the lowercase callables in
`clients/python/clausters/defs/ugens.py` (one language's source). A second
client — the planned JS/TS one, the wasm host — would have had to copy them a
third time, which is exactly the drift the introspection verbs exist to stop.

So the descriptor grew `inputs: &'static [UGenInput]` and the sixty rows were
filled by reconciling the two existing sources. **The wire is unchanged**: a def
still lists input *values* positionally and no input is ever addressed by name,
the compiler still validates by arity, and no existing def behaves differently.
The names are descriptive metadata — what a palette labels an inlet with, what
an error message can say instead of "input 2" — published through a typed verb
rather than transcribed.

Two consequences worth recording:

**The Python signatures are not the wire order, and could not simply be
harvested.** Nine kinds order their parameters for ergonomics instead: what is
required comes first and the static (non-signal) fields are keyword-only, so
`send_reply(trig, *values, cmd, reply_id)` fronts a wire order of
`[trig, reply_id, *values]`, and `disk_in(path, chan, loop)` has two parameters
that are not inputs at all. The registry names the **wire** order, and the
contrast test (`clients/python/tests/test_session.py`) carries those nine in an
explicit exception list with a reason each, asserting the list is exact — so a
tenth divergence has to be declared deliberately rather than quietly weakening
the check.

**`ugens.py` stays hand-written.** Generating it from the catalog would end the
drift by construction, but the callables carry hand-written docstrings that are
the client's real teaching surface, and generation would add a build step to a
pure-Python package. The contrast test gives the same guarantee and costs one
test: for every kind whose signature maps 1:1 onto the wire, names and defaults
must agree with what the server reports (compared at `f32` precision,
since the server's defaults arrive widened to `f64`).

## Band-limited oscillators are PolyBLEP over an `f64` phase, and the residual is measured rather than claimed

scsynth builds `Saw`, `Pulse` and `Blip` from a **discrete-summation impulse
train**: a sine table divided by a cosecant table, the quotient run through a
leaky integrator with a `0.999` pole, over a 32-bit fixed-point phase. It is
genuinely band-limited, and it costs a division and two table lookups per
sample, carries a settling transient, leaves a residual DC droop from the
integrator, and quantizes tuning to the fixed-point step.

**Decision:** accumulate phase in `f64` and correct each discontinuity with a
**fourth-order PolyBLEP** — the residual obtained by integrating a cubic
B-spline, spanning two samples on each side of the jump. No division, no table,
no integrator state, no DC error, and the polynomial only runs on four samples
per cycle; everywhere else the inner loop is the naive expression plus one
comparison. The `f64` phase is the same reasoning that named `Sine` for what it
produces: an `f32` accumulator drifts audibly over a long note and a fixed-point
one quantizes the pitch.

**What it costs, in numbers.** PolyBLEP is *quasi*-band-limited: a polynomial
cannot be a sinc, so a residual remains and grows with the fundamental. That is
not hedged in prose, it is measured, and the measurement is regenerated on every
test run against a naive waveform built in the same test (alias SNR, 48 kHz):

| fundamental | `Saw` | naive ramp | `Pulse` | naive square |
|---|---|---|---|---|
| 105 Hz | 96.7 dB | 30.9 dB | 98.4 dB | 32.7 dB |
| 996 Hz | 42.6 dB | 16.0 dB | 43.5 dB | 17.7 dB |
| 3996 Hz | 39.2 dB | 9.9 dB | 38.9 dB | 11.4 dB |

At 105 Hz that is within about 2.5 dB of what the analysis itself resolves (a
pure tone reads 99.2 dB through it), so the low end is effectively transparent.

**Why fourth order and not second.** The two-sample residual was implemented,
measured, and rejected on its numbers: 67.6 / 32.3 / 27.7 dB at the same three
fundamentals. Doubling the corrected span adds 29 dB at the bottom and 10 to
12 dB over the rest, for two extra polynomial evaluations per cycle — a trade
worth taking, and one that could only be judged after measuring both.

**Two consequences worth writing down.** Above `sr/4` the two correction regions
would overlap, so the increment is tested once per sample and the calculation
falls back to the second-order residual, which stays disjoint to `sr/2`; a
waveform with a fundamental that high has at most one harmonic left, so the
switch is inaudible. And **direction cancels out**: running the phase backwards
reverses both which side of the discontinuity a sample sits on and the sign of
the jump, and the residual is antisymmetric, so a negative frequency needs the
same expression with `|dt|`. Mirroring the phase instead (`1 - t`) is
algebraically identical but evaluates the polynomial on a difference of nearly
equal numbers, which measurably costs precision at fourth order — it read 25 dB
where the direct form reads 42.

**Not band-limited, on purpose:** the `LF*` family and `VarSaw`, exactly as in
scsynth. They are modulation sources; their corners should be exact, and
softening them would be a defect rather than a feature. Their initial phase is
in **cycles**, `[0, 1)`, where sclang measures the same argument in `[0, 2)`
because its accumulator happens to run over `[-1, 1]` — an implementation
detail exposed as a unit, which every phase in this project declines to inherit.

## The two-pole filters are one trapezoidal state-variable core, and the response can be an input

scsynth realizes `LPF`, `HPF`, `BPF`, `BRF`, `RLPF`, `RHPF` and `Resonz` as
seven direct-form two-pole sections, each with its own coefficient formula, its
own `next` variants, and its own copy of the same algebra.

**Decision:** one *topology-preserving* (trapezoidal-integrator) state-variable
filter behind all of them. It implements the **same** prototype — the bilinear
transform of the analog two-pole — so the transfer function is not an
approximation of scsynth's, it is the same function, and the tests assert it
against the closed form rather than against a golden file: within **0.1 dB**
across nine octaves at every cutoff and resonance tried, with the allpass mix
flat to 0.02 dB and the notch nulling below −136 dB.

What changes is behaviour, not response:

1. **It does not leave its stable region under audio-rate cutoff modulation.**
   A direct-form section's state has no meaning between two coefficient sets; an
   integrator's state is the signal it has integrated, whatever happens next.
   The acceptance test sweeps a resonant cutoff from 20 Hz to 18 kHz at 40 Hz
   under full-scale noise and asserts the output stays bounded.
2. **It stays well conditioned at low cutoff**, where the poles crowd `z = 1`.
   The acceptance test runs `LPF` at 20 Hz for ten seconds and requires the
   passband gain to still match the analytic value within 0.1 dB.
3. **Every response falls out of the same pair of integrator updates**, as a
   linear mix of three taps. That is what lets one implementation carry seven
   scsynth names — and it is the whole reason for the one row scsynth has no
   name for.

**`Svf`: the response as a signal.** Because the taps are already computed,
exposing their gains as **inputs** costs the mix and nothing else, where a
direct-form section would have to recompute coefficients. So `Svf` takes `low`,
`band` and `high` as ordinary signals, and every classic response is a triple:
lowpass `1,0,0`; bandpass `0,rq,0`; highpass `0,0,1`; notch `1,0,1`; peak
`-1,0,1`; allpass `1,-rq,1`. A one-knob morph is a **client-side** helper over
those three inputs (`svf_morph` in the Python client), deliberately not a wire
parameter: committing the protocol to one arbitrary ordering of responses would
exclude every other, and the ordering is a user-interface choice, not a DSP one.

**Precision:** state and coefficients are `f64`. This is not caution and not a
deviation — scsynth's `FilterUGens.cpp` declares `double y1, y2, a0, b1, b2` for
exactly these filters, because near DC the coefficient quantization and the
state truncation of `f32` dominate the output.

**Coefficient rate:** the `tan` and the reciprocal run **once per block** when
the parameters arrive as scalar wires, and twice — at the block's first and last
sample, with the three integrator gains interpolated linearly between — when
either is audio-rate. Interpolating the *gains* rather than the cutoff is what
keeps a modulated filter at three multiply-adds per sample instead of a
transcendental and a division. This is scsynth's `CALCSLOPE` idea applied one
level later.

**`rq`, not `Q`, on the wire.** Keeping scsynth's reciprocal is not a
performance choice: it saves one division per *block*, next to a `tan` that
costs several times more. It is a domain choice — `rq = 0` is infinite Q and is
exactly representable, where `Q = 0` divides by zero and `Q → ∞` is not a
number. The awkwardness is real, so the client builders accept `q=` and convert
(a constant folds at graph-build time; a signal composes one reciprocal).

**`BPF` and `Resonz` are the same row twice.** scsynth ships two historically
distinct two-pole resonators that promise the same parameterization and the same
unity peak gain; reproducing the distinction would mean reproducing an accident.
A test asserts the two are sample-identical, so a reader finds the answer
instead of wondering.

**The one-pole family keeps its coefficient parameterization** (`OnePole`,
`OneZero`, `LeakDC`, `Integrator` take a pole, not a cutoff), as in scsynth: a
one-pole has no `-3 dB` point in the sense a two-pole does, and naming the
parameter a frequency would promise one. `Lag` is the UGen for a time constant.
The coefficient is clamped just inside the unit circle, so a mistyped control
degrades instead of producing NaN forever — which does mean `Integrator` always
leaks, deliberately: a true integrator fed any DC reaches infinity, and it would
do so on the audio thread.

## The delay family is one line in synth-private memory, and its length is configuration

scsynth ships nine delay UGens — `DelayN/L/C`, `CombN/L/C`, `AllpassN/L/C` —
as nine plugins. They are two independent choices: how a fractional tap is
interpolated, and what the line feeds back.

**Decision:** one circular line parameterized by those two, registered under all
nine scsynth names (the `PvMag` pattern). Measured through a half-sample delay at
9 kHz, linear interpolation loses about 1.6 dB and cubic about 0.36 dB — neither
is transparent three quarters of the way to Nyquist, and that gap is what
justifies paying for `C` on a modulated delay.

**The line is synth-private memory, not a pool buffer.** A pool buffer is
immutable once built — the invariant that already put the spectral frame in
private scratch — and a delay line is written every sample. So it is allocated
in `build`, on the network thread, from the static `max_delay` and the sample
rate. **This is the reason `UGenDescriptor::build` receives a sample rate at
all** (U0): the length in samples cannot be known without it, and it cannot be
computed later, because "later" is the audio thread. A consequence worth
stating: there is no `BufDelay*` family here, since a delay over a pool buffer
would have to mutate one.

**`max_delay` is static configuration, not an input.** scsynth passes
`maxdelaytime` as an initial-rate *input* because its `ir` inputs double as
build-time constants. Here the field that sizes an allocation lives where
`fft_size` and `partitions` already live, and the signal inputs are only the
things that vary. It defaults to 0.2 s when a def omits it, like `fft_size`'s
default; a `delaytime` past it is clamped, never wrapped. The Python builders
fill it in from a *constant* delay time and **raise** on a modulated one that
does not state its reach — a silently truncated modulation is worse than an
error at graph-build time.

**These UGens do not report an intrinsic latency.** The `latency()` hook exists
for a UGen whose processing happens to lag (the partitioned convolver) and feeds
general server delay compensation. A delay's delay is what the user asked for;
compensating it would silently undo the instrument.

**One convention worth writing down because it caught a test.** `decaytime` is
the time for the echo train to fall 60 dB counted from the **first** echo, which
is the direct path and always returns at full level: `y[D] = 1`, `y[2D] = g`,
`y[3D] = g²`. The envelope is therefore `10^(-3(t - delay)/decay)`, not
`10^(-3t/decay)`. A negative decay time negates the gain, so alternate echoes
invert — scsynth allows this and it is musically useful; a zero decay leaves a
single echo rather than dividing by zero.

## A calculation rate is a time base: every UGen runs at its own sample rate

Found while starting U4, and older than the U track: `Impulse.kr(10)` fired
**once** per second instead of ten times. Every UGen read `ProcessCtx::
sample_rate`, which was the engine's rate for all of them — but a `kr` UGen
emits one sample per block, so dividing a frequency by 48 000 and then stepping
once per 64 samples made it 64 times too slow. The same factor was in `Lag.kr`'s
convergence time, `Saw.kr`'s pitch, every filter's cutoff at `kr`, and would
have been in `Line.kr`'s duration — which is what made it surface now, since a
one-segment ramp is the canonical control-rate UGen.

**The fix is the one scsynth already makes:** `sample_rate` is the rate of the
**UGen being run**, not the engine's — scsynth's `unit->mRate->mSampleRate`,
which for a control-rate unit is the control rate. A `kr` sample lasts a whole
slice, so its rate is `full_sample_rate / frames`. Everything that turns seconds
into samples then divides by the same field and is correct at either rate with
no branch of its own; the alternative — a `rate_scale` factor each UGen must
remember to apply — is a bug waiting in every kind added after it, and the bug
is silent, because a wrong time base still produces a plausible signal.

**Why `frames` and not `BLOCK_SIZE`.** A scheduled bundle splits a block at the
event's sample and runs the whole tree per slice, so a `kr` UGen ticks once per
*slice*, not once per block. Deriving its rate from the slice length makes a
shorter tick cover proportionally less time, and the two cancel exactly: control
time advances at the same speed whether or not the block was cut. Had the rate
come from `BLOCK_SIZE`, a busy score would have run its control-rate UGens fast
in proportion to how many events it scheduled — the sort of error that shows up
as a mix that drifts and no failing test.

**The engine's rate stays reachable** as `ProcessCtx::full_sample_rate`, for the
two quantities that are hardware facts rather than time bases: the `SampleRate`
UGen (`SampleRate.kr` reports 48 kHz, not the 750 Hz it runs at) and a spectral
chain's Hz-per-bin. `FFT` and the `PV_*` family are `kr` for an unrelated reason
— a frame is not a block — and consume their audio input frame by frame, so they
are untouched by the rate change and only needed the bin spacing corrected.

The consequence for users is worth stating plainly, because it is what makes the
rates comparable: **choosing `kr` changes a UGen's cost, not its meaning.** A
time is still in seconds, a frequency still in hertz; what you give up is
resolution, since nothing above half the control rate (375 Hz at 48 kHz / 64)
can be represented.

## `Line` is its own ramp, not `EnvGen`, and the done flag is not the done action

Two decisions from U4, one of them since reversed.

**One exponential curve, and the clients get the whole warp family.** Reading a
value out of one range and writing it into another is the same act at four
scales — an envelope segment between two levels, an `XLine`'s ramp, a control's
position under a knob, a client mapping a fader to a frequency — and it was
written three times before it was written once. `envshape`'s exponential shape,
`envshape`'s bent one and `dsp::line`'s `XLine` each carried their own formula
and their own answer to the levels an exponential has none for.

`clausters_core::warp` is that act, once. Its shape is what keeps it that way:
a map **reads** a position out of one range and **writes** it into another, each
half in the same three flavours (linear, exponential, bent), so the eight names
SuperCollider gives the family — `linlin`, `linexp`, `explin`, `expexp`,
`lincurve`, `curvelin`, `range`, `exprange` — are pairs of six primitives and no
curve is computed twice. The bent pair shares its two coefficients between both
directions, so a curve and its inverse cannot drift.

**Zero has no ratio, and that is now one function.** `warp::exp_ends` nudges an
endpoint within `1e-5` of zero to that epsilon with the sign it had, and reports
a pair straddling zero as having no exponential at all, which every caller falls
back to the linear map on. SuperCollider's own answer is a `NaN` (or an author
who was supposed to know better), and this crate answers instead for a reason
that is not audio at all: the same curve is what an editor **draws**, and a
`NaN` there is not a wrong sound but a vanished pixel.

**Against sclang the agreement is a tolerance, not bit equality**, and
deliberately so: the formulas are reproduced shape for shape and asserted
against sclang's own values, but sclang computes in `f64` and associates left to
right where this computes in `f32`, the precision the server computes in. `f32`
is the one that matters — it is what makes a value a client maps and the same
map on the audio thread agree. `Clip` carries sclang's `prune` including its
sequential comparisons, so a reversed range prunes the way sclang prunes one;
`range`/`exprange` prune nothing, because a UGen knows its own `signalRange` and
a bare value does not.

**What is not here yet is the signal side.** There are no `LinLin`/`LinExp`
UGens, so a def still writes the arithmetic by hand; when they land they bind
these same functions rather than restating them, which is the whole point of the
module existing before they do.

**`Line`/`XLine` are a ramp of their own, not the segment engine.** U4 first
built them as `EnvGen` with its header filled in — a gate held open, one
segment, no release node — assembled in the wrapper's stack frame. The appeal
was reuse: the whole `doneAction` set, the exact landing, and the shared
`envshape` arithmetic a client draws a curve with, for one indirection per
block and no second ramp to maintain.

The cost turned out to be in the wrong place. `EnvGen` is built for a
breakpoint envelope, so its inner loop re-reads thirteen inputs, runs the gate
edge detection and the segment-advance loop, and evaluates a shape function —
per sample. A `Line` needs one addition and a counter, and an `XLine` one
multiplication; going through the segment engine made the exponential one cost
a `powf` per sample, some fifty times scsynth's. Cheap in absolute terms, and
exactly the sort of thing that stops being cheap at one instance per voice.

So they are now scsynth's ramps (`dsp::line`): the step is derived once and the
inner loop is one arithmetic operation. What the reuse was for is kept
without it — the done actions are the same enum, and the landing is committed
by *assigning* `end` when the counter runs out rather than by arriving at it.
Two things did change, both deliberate:

- **The ramps are init-rate**, as scsynth's are. `end` and `dur` are read on
  the first sample and never again, because the step is derived from them once.
  Modulating them mid-flight used to warp the ramp; now it does nothing.
  `done_action` stays per-block — it addresses the node, not the geometry.
- **The curve is no longer the shared `envshape`.** A line and an exponential
  are the two shapes simple enough to restate exactly (`a + t·(b − a)` and
  `a·(b/a)^t`, here in their accumulating form), so an editor drawing one still
  cannot drift from what the server plays. That argument does not extend to a
  third shape: anything with real curvature belongs in `envshape` and reaches
  the wire through `EnvGen`, not by growing this module.

The zero-endpoint handling `envshape` does for the exponential shape is
restated here too — a zero endpoint nudged to a tiny same-signed level, a sign
change falling back to a linear step — so `XLine(0, 1, …)` is a very steep rise
rather than the `NaN` scsynth produces. That is the one place these ramps are
deliberately *not* scsynth.

**The done flag is a separate hook from the done action.** `Done(src)` and
`FreeSelfWhenDone(src)` ask "has that finished?", and the obvious implementation
— read `src`'s `DoneAction` — is wrong: an envelope with `doneAction` 0 is
exactly the case these two exist for, and it reports `None` forever. So
`UGen::is_done` says only that the UGen ran out, and `UGen::done` keeps saying
what should happen to the node. Two questions, two answers.

**Why watching needs an execution mode.** The flag is not on a wire. An envelope
that has played out sits at its final level, which may be any number and is
routinely the number it started at, so no signal-level test can recover it. The
watcher therefore needs its source's **identity**, not its value — the same need
the demand driver already has for its source slot — and gets it the same way:
`ExecMode::DoneQuery` resolves input 0's wire index in the synth, reads that
UGen's flag and hands it over before `process`. Topological order guarantees the
source ran first in the same slice.

The alternative was to let a watcher silently read zero when pointed at
something that never finishes. Instead the descriptor carries `has_done_flag`
and the compiler rejects the def by name (`Sine has no done flag`). The field
defaults to false in the descriptor constructor and is set by a one-line wrapper
on the three rows that have one, so the hundred other rows did not have to be
touched to add it — the same reason `desc`/`desc_full` were split in the first
place.

**`FreeSelf`/`PauseSelf` do not latch.** They report their action for the block
just processed. For `FreeSelf` the difference is unobservable (the node is gone
either way), but a latched `PauseSelf` would re-pause the instant `/node_run 1`
resumed the node — making the command useless and turning a gate into a one-way
door. So the action is recomputed per block rather than remembered.

## One definition of a trigger, and the boundary cases that follow from it

U5's rows are state machines, so almost every decision in them is about a
boundary: which sample, which of two simultaneous events, what happens before
the first one. Recording them because each was a choice, and because a test had
to be corrected against three of them.

**A trigger is a rising edge through zero, defined once.** The definition was
already copied into three places (`SendTrig`, `SendReply`/`Poll`, the `Demand`
driver) before this milestone; `dsp::trig::Edge` is now the only one, and those
three moved onto it. Nothing about the behaviour changed — the point is that a
kind added later inherits the definition rather than restating it, and that
"trigger" cannot come to mean two things in one server.

**`Timer` and `Sweep` interpolate the crossing.** Both measure time, so they
compute where *between* two samples the input actually crossed zero
(`frac = -prev / (cur - prev)`) instead of rounding to the sample. For a trigger
built from an impulse this is exactly zero and costs nothing. For one built from
an oscillator it is the difference between a period measured to ±0.5 samples and
one measured about twenty times finer: at 997 Hz — deliberately not a whole
number of samples at 48 kHz — the interpolated reading beats sample rounding by
an order of magnitude, which is what the test asserts. scsynth does this too.

**A `TDelay` of `n` samples fires at `t + n`, and re-arms on that sample.** The
countdown therefore advances *before* the trigger is examined, not after. Both
halves matter. Counting the trigger's own sample would put the pulse at
`t + n - 1`, off by one at every duration. And examining the trigger first would
swallow one landing on the very sample the pending pulse fires — which turns a
regular stream of triggers into a limping one (the first version divided a
100 Hz train into intervals of 961, 1440, 1440 samples instead of a steady 960).

**A held pulse includes its trigger's sample; a delay does not.** `Trig`/`Trig1`
of `n` samples cover `t ..= t+n-1`. That is the asymmetry the previous point
turns on, and it is right: a pulse *starts* at the trigger, a delay *ends* `n`
later.

**`Changed` reproduces sclang's halved difference.** It is a pseudo-UGen there,
`HPZ1(in).abs > threshold`, and `HPZ1`'s gain is 0.5 — so a step of 1.0 registers
as 0.5. Reproduced rather than corrected, on the same rule as `hypotapx` in U0:
a def ported from sclang must not change value. Documented everywhere it is
reachable, because it will otherwise be found by someone whose threshold
mysteriously does nothing.

**The done flag has block resolution.** `DetectSilence` raises one, so
`Done`/`FreeSelfWhenDone` can watch it. A watcher reports it for the whole block
in which it was raised, even at `ar`, because the flag is one bool read once
when the watcher runs — not a signal. This is inherent, not a limitation to fix:
a bool has no position within the block. At `kr`, where these two default, it is
exactly the resolution on offer anyway.

**Two small ones.** A `SetResetFF` seeing both edges on one sample ends at 0 —
reset is applied second, so the quiet outcome wins. And a `Stepper` sits at
`resetval` until its first trigger, which therefore lands on `resetval + step`:
a stepper is defined by its transitions, and the alternative makes the first
step invisible.

**Everything defaults to `ar`, counters included — and that reverses the
obvious choice.** A flip-flop or a counter can only move when a trigger does,
so `kr` looks like the free win: one evaluation a block instead of 64, for an
output that cannot change in between. It is a trap. A `kr` UGen reads **one
sample per block** from an `ar` input (`at(input, 0)`), so a `kr` counter fed
an `ar` impulse train sees 1 trigger in 64 and silently drops the rest. The
first draft of this milestone defaulted the counters to `kr` and would have
shipped `pulse_count(impulse(4))` as a builder pair that quietly loses almost
everything.

`kr` is still right *when the trigger is also `kr`* — the saving is then real,
and since the calculation-rate fix that preceded U4 the arithmetic is unchanged
(a duration means seconds at either rate). The rule that came out of it is
general enough to belong in the rate documentation rather than in this family:
**a slower consumer samples a faster input, it does not summarize it.** The
same applies to the U4 node-control rows, which moved to `ar` for the same
reason — a `kr` `FreeSelf` watching a one-sample trigger would miss it and the
node would never leave.

## Pink noise takes the schedule with the worse average and the bounded worst case

Voss–McCartney sums a set of white generators updated at halving rates, and
re-rolls the one picked by the number of trailing zeros in a counter: **exactly
one row changes per sample**, forever. Trammell's stochastic variant decides at
random which rows to update — cheaper on average, and unbounded in the worst
case.

An audio callback is not paid on average. It has one block's budget, and a run
of expensive samples inside one block is a dropout however good the mean was.
Every other choice in this track has gone the same way (the delay line is
allocated at build, the filter coefficients are recomputed per block rather than
per sample): the cost is made **flat and knowable** even when that is not the
cheapest thing on a spreadsheet.

Measured slope, over 40 Hz – 10 kHz: **−3.26 dB/octave** against the ideal
−3.01. That gap is Voss–McCartney's own — the octave-spaced staircase only
approximates 1/f — and it is published here rather than smoothed over, on the
same rule as the oscillators' alias figures.

## Three noise findings that contradict the obvious reading

All three were assumptions written into a test or a doc comment first, and
corrected by measuring.

**`GrayNoise`'s spectrum is not flat.** The obvious reading is that flipping a
random bit per sample gives white noise with a strange amplitude distribution.
It does not: the high bits flip rarely — one sample in 32 for the top one — and
the low bits carry almost no weight, so the energy leans low. Measured
**−2.9 dB/octave**, near enough pink. sclang's help says as much in words; the
number is ours. What *is* distinctive is the distribution: the mean step is some
four thousand times the median, against 1.14 for white noise, and that is the
graininess the kind exists for.

**That bit-level property cannot be tested from the output.** The flip is exact
— it is integer arithmetic on a signed 32-bit word, exactly as scsynth's is
(`int32 mCounter`, `counter ^= 1L << (trand() & 31)`), and bit 31 is the sign
bit, which is what makes the output bipolar. What is lossy is the *conversion*:
the output is `word / 2^31` in `f32`, whose significand is 24 bits against the
word's 31. So the rounding depends on the word's magnitude — and the flip
changes that magnitude. Flipping bit 28 of `0x0001F3A5` reads as a step of
268435451 rather than 2^28; flipping bit 0 of a word near 2^29 reads as no step
at all. The first test asserted "every step is a power of two" and failed on the
third sample. The test now asserts what is observable — the step-size
distribution and the spectrum — and the docs state the limit.

**`Crackle` does not settle below a chaos of 1.** The obvious reading of a
chaos parameter is that low values are periodic and high ones are not. Measured
across 0.3 to 1.9 there is **no period up to 512 samples anywhere**, and the
spread runs 0.56, 0.20, 0.08, 0.05, 0.19, 0.05, 0.06 — not monotonic in either
direction. It is a map, not a level control, and the honest documentation says
to reach for it by ear. (A longer or quasi-period would not be caught by that
test, which is the limit of what a test can say about a chaotic map, and the
test says so.)

## Noise is reproducible on request, and two instances are never the same stream

Every generator draws from `clausters_core::rng` — the xorshift the sequencing
layer and the client's `Pwhite` already use — and each is built from an explicit
seed, so a render replays exactly when it is given one. That is what lets a patch
with noise in it have a golden file at all. Where the seed *comes from* when
nobody says is its own decision, below ("A random process is unpredictable
first").

Each *instance* takes the next seed from the render's own counter, so two
`WhiteNoise` UGens in one def are two streams. This is invisible until someone
writes one: correlated noise summed with itself is a comb filter, not more noise,
and subtracted from itself is silence. There is a test that puts two in a graph
and subtracts them.

What is **not** in the core is the shaping — the dice table, the random walk, the
interpolation. So the claim "a client can reproduce the stream" holds today only
for `WhiteNoise`, whose generator is mirrored there. Saying so is cheaper than
moving ten algorithms across a boundary for a use nobody has yet.

## The pan law is a polynomial, not a table, and it is evaluated per sample

scsynth answers every equal-power question — `Pan2`, `Balance2`, `XFade2`,
`PanAz`'s window — with a **rounded** lookup into a 2049-entry sine table. That
costs a table, an initialization and a worst-case gain error near `3.8e-4`,
which is inaudible on its own but is the kind of floor that ends up under
everything.

Here it is a polynomial in the position: the Taylor series of `sin(t·pi/2)` to
five odd terms, about ten flops, worst-case error `2.6e-7` — three orders of
magnitude under the table's. Three properties are worth stating because they
are design, not luck.

**The endpoints are exact.** The fifth coefficient is not the Taylor one; it is
*defined* as whatever makes the five sum to `1`, which lands within `3.5e-6` of
Taylor's and makes `quarter_sin(1) == 1` exact. It also carries most of the
accuracy: forcing the sum cancels the bulk of the truncation error across the
whole range, which is why the worst case is `2.6e-7` and not the `3.5e-6` the
truncated series alone would give. That number is the gain of a
hard-panned source in the channel it is panned *to*, and the other end —
exact for free, since the polynomial's bare factor is `t` — is the gain in the
channel it is panned *away* from. A hard pan is therefore digital silence on the
far side rather than −110 dB of it.

**The pair is symmetric by construction.** A gain pair is
`(quarter_sin(1 - t), quarter_sin(t))` — one function read from both ends — so
panning to `-pos` is exactly the mirror of panning to `pos`. That is a property
of the expression, not a tolerance a test has to keep honest, and the test
asserts equality rather than closeness.

**The quadrant reduction is exact.** `Rotate2` needs sine and cosine of any
angle, so the same polynomial is read per quadrant. A half turn is therefore
exactly a sign flip and a quarter turn is exactly the mid/side basis — which is
what makes `Rotate2(l, r, 0.25)` and `MidSide(l, r)` agree, and what keeps a
full turn from drifting.

**And it runs per sample when the position is audio rate.** This is the one
place the U track's block-rate stance is deliberately not applied. Interpolating
the two gains across a block — what a filter coefficient does here, and what
scsynth's `CALCSLOPE` does for its own amplitudes — reads `0.5` where the law
wants `0.707` if the position sweeps a whole block: a 3 dB hole in the middle of
every block, on exactly the fast pan sweeps someone reaches for audio rate to
get. A scalar position still computes its gains once per block; the polynomial
is what makes the other path affordable, and is most of the reason it exists.
`crates/clausters-server/examples/bench.rs` measures the gap on the same graph
(`Sine → Pan2 → 2× Out`, position constant against position from an `LFTri`):
**1.30×** the whole graph at every voice count from 32 up — and that figure
includes the `LFTri` the moving version has to run, so it is an upper bound on
the 64 evaluations a block.

## Rotation and width are different operations, so width gets a name

scsynth's `Rotate2` rotates the plane its two inputs span. On a stereo pair that
turns the image without resizing it, and at a quarter turn the rotation *is* the
change of basis between left/right and mid/side — which is how the mid/side
trick is done in sclang, as a side effect of a 45° rotation.

What that cannot express is **width**, which scales the side axis: it resizes
the image without moving it, and no angle produces it (a rotation is orthogonal;
a width is a squeeze). So the family had a hole where its most-wanted member
should be, hidden by a name that describes a different operation. Two rows fill
it, both the same two-by-two matrix `Rotate2` already is:

- **`StereoWidth`** — the knob. `0` mono, `1` exactly the identity (the
  coefficients are `1` and `0`, not `0.99999` and `1e-8`), `2` widened,
  negative swaps the sides.
- **`MidSide`** — the matrix itself, normalized to `1/sqrt(2)` rather than the
  `1/2` a DAW meter shows. That normalization makes it an **involution**: one
  kind encodes and decodes, and the round trip is exact rather than nearly so.
  It costs a 3 dB offset on the mid against the convention, which is a plain
  gain, and it leaves the catalog with one name instead of an encoder and a
  decoder that must be kept inverse to each other by hand.

`StereoWidth` is `MidSide`, a multiply, `MidSide` — collapsed into the matrix it
amounts to, and tested against the two-step route. Keeping both is not
redundancy: the knob is what most defs want, and the pair is the only one of the
two that lets something happen *between* the encode and the decode (the centre
of a mix filtered apart from its sides), which is the reason mid/side processing
exists at all.

Naming them for what they do rather than for scsynth's vocabulary is the same
call the U track's design stance already makes for a capability scsynth has no
name for. The cost is two rows that a def ported from sclang will never mention;
the alternative was documenting a rotation as the way to get a width.

## `SelectX` is one row, not two `Select`s and a crossfade

sclang's `SelectX` is a pseudo-UGen: it builds `Select(which.round(2), array)`,
`Select(which.trunc(2) + 1, array)` and an `XFade2` across
`(which * 2 - 1).fold2(1)` — a ping-pong that alternates which of the two
selectors holds the even neighbour as the index sweeps, with the fold undoing
the direction reversal. It is ingenious and it is three UGens where one will do.

Here it is a mode of the selector: clamp the index, split it into floor and
fraction, and crossfade the two neighbours with the same equal-power law the
rest of the family uses. Across the whole index range the two agree, and that
equivalence is asserted point by point rather than assumed from having copied
the construction.

**Off the ends they part company, and the divergence is deliberate.** sclang
*folds* the crossfade position while *clipping* the two picks, so an index of
`-0.5` comes out as a 50/50 mix of the first two sources rather than the first
one, and an index past the end comes out as the last source crossfaded with
itself — 1.414 times its value, 3 dB of gain from nowhere. (The class even
accepts a `wrap` argument and drops it on the floor before `new1` sees it.)
Reproducing that would honour the letter of "a ported def must not change
value" while shipping a gain bug; the index clamps instead, exactly as
`Select`'s does, and the tests pin both halves — the agreement inside the range
and the clamp outside it.

What does *not* change is the property that surprises people about both: every
source runs whether or not it is selected, because they are UGens in a graph and
not branches.

## `repeats ≤ 0` is the endless stream, because a def cannot say `inf`

Every demand source takes a `repeats` count, and sclang's answer for "keep
going" is `inf` — a float the language has and the wire does not. `compile`
rejects a non-finite constant outright (`crates/clausters-server/src/synthdef/mod.rs`), JSON has no
spelling for one, and neither is worth changing for this: a format that accepts
infinities has to decide what they mean everywhere, not just here.

So the count of **none** is the endless one. A client that writes `repeats=0`
gets what it would have guessed from a count anyway, a positive count behaves
exactly as scsynth's, and an `inf` still works if some client manages to send
one. The one case that needed thinking is a `repeats` that is *itself* an
exhausted stream, which pulls as `NaN`: there the number is a value rather than
a request, so it means **zero** — the stream feeding the count ran out, and a
source with nothing to repeat yields nothing. Reading it as endless would turn
every drained counter into an infinite loop, which is the opposite of what the
exhaustion is reporting.

The asymmetry in *what* the count counts is scsynth's, and kept: passes over the
list for `Dseq` and `Dshuf`, items for `Drand` and `Dxrand`. A shuffle that
stopped mid-list would not be one, and a random pick has no pass to complete.

## A nested pull borrows the prefix, and the depth is a compile-time refusal

The demand family is only interesting because streams nest: `Dseq`'s value list
can hold another `Dseq`, which is what makes it a sequencer of phrases rather
than of numbers. That turns a pull into a recursion, and a recursion into a
question the audio thread has to be able to answer safely — with no allocation,
no aliasing, and no way to run off the stack.

Both halves fall out of one observation: **a UGen's inputs are always earlier
UGens**. So `Pull` (`crates/clausters-server/src/synthdef/instance.rs`) borrows the *prefix* of the UGen
vector before the UGen it is serving, and each nested pull splits a strictly
shorter prefix. The borrows up the call stack form a strictly decreasing chain
of indices — no UGen can be reached twice, no `&mut` can alias, and the compiler
checks it rather than a comment claiming it. Nothing on the path allocates,
because there is nothing to allocate: the whole view is a stack value.

Depth is capped at **16** and the cap lives in `compile`, not in `demand`. Each
level costs a real stack frame *inside the audio callback*, so the honest place
to say no is where a human is still watching — a def that nests too deep is a
def the server refuses, with a message naming the depth. Checking it per pull
would spend the callback's time discovering, every block, something that was
knowable once. Sixteen is far past any musical use (sclang's own patterns rarely
reach three) and far short of anything a callback stack would notice.

The same observation generalized the rate rule. S1 had one: a `dr` wire may feed
a `Demand`'s source slot, and that slot must be `dr`. Neither half survives —
`Duty` pulls two of its four inputs, so "the source slot" stopped naming
anything, and a nesting source pulls inputs that may or may not be streams. What
replaced both is a single rule with no slot in it: **a `dr` wire may feed only
something that pulls it** — a driver, or another demand UGen. A stream anywhere
else would cross into block order, where it has no samples to offer. The
converse rule is gone entirely: a driver handed a plain number pulls a stream of
one value that never ends, which is well defined and occasionally what you meant.

## Resetting a demand stream is per kind, and it happens late

A parent that comes back to a child it has already drained resets it, so the
child replays — that is what makes `Dseq(2, Dseries(3, 0, 1))` give `0 1 2 0 1 2`
rather than `0 1 2` and silence. Two things about that are decisions rather than
mechanics.

**It is lazy.** The reset is *marked* when a slot is left and *performed* just
before that slot is read again. Doing it eagerly is the obvious implementation
and it is wrong at the edge: a parent that moves on and then ends, or is itself
reset, would have restarted a child it never returns to — and a restarted
`Dshuf` has drawn a new order, which is audible. Marking costs one bool and
makes the eager case unreachable.

**It does not propagate by default.** `reset_demand` receives the same
`DemandInputs` a pull does, so each kind decides which of its inputs to restart:
the list sources restart the slot they move to, `Dstutter` and `Dswitch1` restart
their inputs outright (neither has a position of its own to rewind), and the
scalar sources — `Dseries`, `Dwhite`, the walks — propagate nothing at all,
because they re-read their bounds on every pull and have nothing to gain from
restarting whatever produces them. A blanket "reset resets everything below"
would be simpler to describe and would quietly rewind streams shared between two
consumers. scsynth draws the same line, visible in which of its `_next`
functions call `RESETINPUT`; this is that asymmetry made explicit rather than
inherited.

## `f64` in the phase family is for the position, not the phase

The phase family's accumulator is `f64`, and both the module doc and `Sine`'s
used to give the same reason: an `f32` phase drifts over a long note. Measured,
it does not, and the reason it does not is the wrap.

A phase wrapped into `[0, 1)` never grows. Its rounding error per step is about
one ulp of 1.0 — 6e-8 — and it random-walks rather than accumulating, so after
ten seconds (480 000 steps) the expected error is some 4e-5 of a cycle. Stepped
side by side in each precision, a 55 Hz saw reads **55.0003 Hz in its tenth
second either way**. For every wrapped row — `Saw`, `Pulse`, the `LF*` shapes,
`Sine` — `f32` would have done.

`Phasor` is the row that needs the precision, and it needs it badly, because its
position is deliberately *not* wrapped into a small range: it is a buffer index,
one frame per sample, over whatever the file is long. Above 2^24 consecutive
`f32` values are 2 apart, so `pos += 1.0` rounds back to where it started and the
index **stops dead** — measured, an `f32` position eight minutes into a 48 kHz
file advances **0 frames in ten seconds** where the `f64` one advances 479 999.
Not a drift, a stall, and silent: the output is a plausible constant.

So the type is chosen by the widest-ranging row and shared by the family, which
is the right outcome reached by the wrong argument. Both doc comments now say
which row is paying for it, and `crates/clausters-server/tests/offline/oscillators.rs` carries the measurement
next to the assert (`f64_is_for_the_position_not_the_phase`) so the claim cannot
quietly become folklore again. The general lesson is worth keeping: for a
floating-point accumulator, what matters is the **magnitude it reaches**, not the
number of steps it takes to get there.

## SIMD is left to the autovectorizer, and the browser caps it at 128 bits

The question comes back periodically — are the UGens vectorizable with AVX/SSE/
NEON, and what does that mean for the wasm engine — so here is the standing
answer and the reasoning behind it.

**Nothing in the tree asks for SIMD.** There are no intrinsics, no `std::simd`,
no `wide`, and no `RUSTFLAGS`/`target-cpu` anywhere — so a stock build gets the
baseline of its target: `sse`/`sse2` on x86-64 (128 bits, four `f32`) and NEON on
aarch64, which has it in the baseline. The one piece of groundwork is the
cache-line alignment of `Block` (`crates/clausters-server/src/dsp/mod.rs`): a block is `#[repr(C,
align(64))]` over `[f32; 64]`, exactly four 64-byte lines, so a vector load or
store never straddles a line. That was kept **for the stability argument, not for
a measured gain** — the interleaved A/B benchmark that justified it came back
identical within the machine's noise.

**The decision is to stay on autovectorization and write loop bodies it can
take.** Two reasons:

- **It is enough.** Measured below: the arithmetic rows went from scalar to
  4-wide SSE2 without a single intrinsic, purely by handing LLVM a loop it could
  take. An explicit-vector rewrite would be starting from a body that is already
  vectorized.
- **Intrinsics would be three code paths.** `core::arch::x86_64`,
  `core::arch::aarch64` and `core::arch::wasm32`, each hand-written and each
  needing its own correctness argument. One autovectorizable loop body serves all
  three. If explicit vectors are ever genuinely needed, the portable 128-bit
  wrappers are the entry point, not the intrinsics — they are the width every
  target has.

And the vectorized path already exists, under another name: **the Faust family
is the one that gets it**, free and host-tuned. Its factories JIT for the host
CPU, which is exactly why CI has to pin a baseline target — the SIGILL
investigation above found AVX-512 in the emitted pages. Two def families as peers
means a graph that needs the ALU can be written in the family whose compiler
already vectorizes it.

**Which rows could vectorize at all.** They split cleanly. The element-wise ones
— the binary and unary operators, the fused `MulAdd`/`Sum3`/`Sum4`, panning, bus
mixing — have no loop-carried dependence and are candidates. Everything with
state is a recurrence and is not: the filters, the phase accumulators, `Sine`,
`Lag`, `EnvGen`, delay feedback, `LocalIn`/`LocalOut`, the demand rows, the noise
generators. Those can only be widened by rewriting the recurrence itself (the
`y[n] = a^k·y[n-k] + …` unrolling, a matrix step for the biquad, `phase[i] =
phase0 + i·inc`), and **every one of those transforms changes the result in the
last bits** — against golden WAVs and the sample-by-sample Faust parity suite,
that is a real cost, not a rounding detail.

**Our own loop bodies were the obstacle, and they were the whole story.** The
shared operators ran `apply_binary(op, …)` with `op` a runtime field, so a match
over 40 variants sat *inside* the per-sample loop, and `builtins::at` — the
length-1 broadcast — put a branch there too. Both are loop-invariant, so the
first draft of this entry supposed LLVM might hoist them and called the cost
small. **Measured, it does not and it was not.** `binary_slice`/`unary_slice` now
match the operator once and hand a monomorphic closure to a helper that resolves
the broadcast shape before the loop, leaving a flat body over slices of equal
length. Nothing about the arithmetic changed — the closure still calls the same
scalar `apply_*`, which is what keeps the two paths one definition, and a test
asserts the slice ops equal the scalar ones **bit for bit** (`to_bits`, so a NaN
must be the same NaN) across every operator, every broadcast shape and the edge
operands.

The disassembly is the direct evidence, read from two probes that take the
operator as a runtime integer so neither can fold it away. Before: 506
instructions, **no packed arithmetic at all** — every operator scalar (`mulss`,
`addss`), the only `*ps` present being the `xorps`/`andps` sign and absolute-value
bit tricks on a single lane. After: 3744 instructions, of which **985 packed** —
288 `mulps`, 127 `addps`, 112 `subps`. That is the 4-wide SSE2 baseline actually
being used.

The timings, interleaved A/B in one process (best of nine rounds, 400 000 calls
each, 64-frame block), ns per block:

| case | before | after | |
|---|---:|---:|---:|
| `Mul` signal × constant | 71.7 | 4.7 | **15.3×** |
| `Mul` signal × signal | 71.8 | 5.5 | 13.1× |
| `Add` signal × signal | 72.1 | 5.1 | 14.1× |
| `Gt` signal × constant | 72.2 | 5.7 | 12.6× |
| `Neg` (unary) | 39.6 | 4.3 | 9.3× |
| `Sqrt` (unary) | 41.7 | 10.2 | 4.1× |
| `Pow` signal × constant | 253.6 | 231.7 | 1.09× |
| `Sine` (unary) | 161.9 | 134.7 | 1.20× |

More than the 4× vectorization alone, because the per-sample jump through a
40-way match cost more than the multiply did. The transcendental rows are the
control: `Pow` and `Sine` call a scalar libm and barely move, which is what says
the rest of the table is the loop body and not the harness.

At the engine, `crates/clausters-server/examples/bench.rs` over three interleaved rounds: the default def
(`Sine · amp → 2× Out`, one multiply among four UGens) gains **~20%** at every
voice count (1000 synths: 1205 → 1435 blocks/s). The bit-exact `gain` graph —
the bench built precisely to isolate engine overhead — gains **~70%** (128
synths: 87.3 → 150.5 × real time), and Faust's lead on it, the number this entry
originally cited as proof the arithmetic was not worth touching, **fell from 2.9×
to 1.6×**. So the second reason in the first draft ("widening the arithmetic does
not touch that") was wrong twice over: the dispatch was not the only cost, and
removing it closed half the gap.

The price is code size — 506 to 3744 instructions for the binary dispatch,
because 77 operators are each monomorphized. For a synthesis server's hot path
that is the right side of the trade, but it is the reason the technique belongs
here and not reflexively everywhere.

**The fused rows then separated the two obstacles, and the answer is not what
the paragraph above assumed.** `MulAdd`, `Sum3` and `Sum4` always held their
operator as a *constant* — `apply_binary(BinaryOp::Mul, …)` written literally —
so the only thing left in their loops was `at`'s branch. If that branch were
itself a barrier, they would have been as scalar as `binary_slice` was. They were
not: `MulAdd` disassembles to **26 packed arithmetic instructions against 15
scalar**, so LLVM had already unswitched part of it on its own. Hoisting the rest
by hand was written and measured — it takes the same probe to 199 packed against
3, and is worth **1.19–1.23× on the all-signal shapes, 1.03–1.09× on the
constant-heavy ones** — real, consistent, and an order of magnitude less than the
operator match was worth.

So the two are not peers, and it is worth stating the rule the measurement
actually supports: **a runtime `match` in a loop body stops vectorization dead; a
loop-invariant branch on a slice length usually does not.** The first is a jump
through a table the vectorizer cannot see past. The second is something LLVM
often unswitches by itself — often, not always. The fused rows are where "not
always" showed up, which is what made hoisting them worth trying.

**At the engine it does not show up at all.** `crates/clausters-server/examples/bench.rs` grew a fused
section for exactly this question, and the same A/B through it — `MulAdd` and
`Sum4` graphs over a shared `Sine`, three interleaved rounds — moves **inside the
±1% noise** at every voice count. That is the correct reading rather than a
disappointment: an arithmetic row is 5–10 ns of a block that spends ~135 ns in
`Sine`'s per-sample `sin()`, so a 20% cut to the cheap part is invisible next to
the expensive one. It is also the difference from the operator hoist, which *did*
move the engine ~20%: there the loop being fixed was 70 ns, not 10.

What the section is for going forward is the question a user actually asks —
whether to reach for `MulAdd` over `a*b` then `+c` at all. Read at the voice
counts where the bench is steady (32 to 512): **fusing is worth some 3–4% for
`MulAdd` over `Mul`+`Add` and 5–8% for `Sum4` over three `Add`s**, and what it
saves is mostly the dropped `dyn` dispatch and wire buffer rather than the
arithmetic. The 1000-voice row of that section is **not** readable — repeated
rounds swing it from 0.90× to 1.09×, since by then the graph is near real time
and the measurement is competing with the scheduler. Read the ratio in the middle
of the sweep, not at its end.

**So the fused hoist was reverted and the bodies are the naive ones again.** It
worked and it was bit-exact; it just did not earn what it cost. Hoisting three
and four inputs is not `map2`'s four arms but eight and sixteen, one `const bool`
per input and a cartesian product of shapes — around 150 lines of dispatch, in
the shared core, for a gain no graph can observe. Reverting is the cheap decision
here precisely *because* it was measured: the number is written down, so the next
person to have the idea starts from evidence instead of from scratch.

What was kept is the part that stands on its own: the three now live in
`clausters_core::builtins` beside `binary_slice` (`mul_add_slice`, `sum3_slice`,
`sum4_slice`), which is where `dsp::fused`'s module doc already claimed the math
lived — a client folding `a*b + c` off the RT path can now actually call it. They
get no C ABI export yet: additive as that would be, it waits until a client asks
for it, the same deferral the note spelling and the tap reader took. The
sixteen-shape bit-exactness test was kept too, now pinning the operand order as a
contract rather than guarding a refactor — it is the harness the next attempt
would be measured against.

**Where things stand, by family.** Read off the shipped release binary rather
than assumed — every `UGen::process` is a vtable entry, so it survives as a real
symbol and its instruction mix can be counted (`objdump -dC target/release/clausters`,
then count `*ps`/`*pd` against `*ss`/`*sd` per symbol; regenerate it that way
rather than trusting the summary below to stay current):

- **Vectorized**: the operator rows through `binary_slice` (88 `mulps`, 45
  `addps`, 42 `subps`, 24 `minps`), the fused rows, `Out`'s bus accumulation
  (`addps`), `Rotate`.
- **Partly, for a real reason**: `unary_slice` is 27 packed against 116 scalar
  because most unary operators are transcendental — `sin`, `exp`, `log`, `pow`
  are libm calls, and vectorizing those needs a vector libm, not a better loop.
  `Pan`, `Select`, `PanAz` and `Svf` mix for the same kind of reason.
- **Scalar, and correctly so**: everything with state — the filters, the phase
  family, `Sine`, the table readers, `Lag`, `EnvGen`, `Delay`, the noise family,
  the trigger family. These are recurrences; see the split described earlier.
- **One reading trap**: several rows disassemble to a dozen instructions and no
  arithmetic at all (`binop::BinaryOp`, the `spectral` rows, the `demand` rows).
  Those are *thunks* — the vtable entry tail-calls into the core or another
  module. Counting them as "scalar" would say the binary operator does no
  arithmetic.
- **One thing worth a look someday**: `Rotate` vectorizes as `mulpd`/`addpd`,
  not `*ps` — it works in `f64`, so it runs two lanes where the rest run four.
  Whether that precision is needed there was never examined.

**What this means in the browser.** wasm's SIMD is `simd128`: **fixed at 128
bits, four `f32`, and there is nothing wider** — no AVX equivalent, by design of
the instruction set. Rust does not enable it by default on
`wasm32-unknown-unknown` (the default features are `bulk-memory`, `multivalue`,
`mutable-globals`, `nontrapping-fptoint`, `reference-types`, `sign-ext`), so the
browser engine today is entirely scalar. Turning it on is one
`-C target-feature=+simd128` in the web build, and the cost of that switch is a
distribution question rather than a code one: wasm has no cheap runtime feature
detection, so it means either assuming the capability — universal in current
browsers — or shipping a second bundle. That is left for when a browser workload
is actually measured to need it. Worth remembering alongside it: the browser also
gives no denormal control at all, which is why `flush_to_zero` is a no-op
outside x86-64 and aarch64.

The general shape of the decision: vectorization here is a property of the loop
body and of which def family the DSP is written in, not a build flag we forgot to
set.

## The UGen graph's overhead is the wires, and only a compiler can collect it

The entry above ends by naming what was left after the operator loops were fixed:
fusing chains so the intermediate wire buffers disappear, the shape of the gap
the `gain` bench still shows against Faust. Measured, it closes the question
rather than opening it.

A chain of N binary operators, all holding their operator at run time as
`dsp::binop` does, run five ways over one block (ns per block, best of nine
interleaved rounds):

| N | `dyn` + wires | direct call + wires | fused, runtime ops | fused, tiled | fused, static ops |
|---:|---:|---:|---:|---:|---:|
| 1 | 6.4 | 4.7 | 65.0 | 30.3 | 6.8 |
| 2 | 13.5 | 13.2 | 100.6 | 44.1 | 8.0 |
| 4 | 28.4 | 28.3 | 154.9 | 73.7 | 9.0 |
| 8 | 54.5 | 55.1 | 257.7 | 148.0 | 13.5 |

Three things fall out, and none of them was the expected one.

**The `dyn` dispatch costs nothing.** Devirtualizing the whole chain — the second
column, a direct call that still cannot inline — moves the total by −0.7 to
1.8 ns, which is noise. The indirect call through the vtable is one
well-predicted branch per link. Any plan that starts "first get rid of the
virtual dispatch" is optimizing a rounding error.

**The wires are the entire overhead.** At N = 8, 41 of the 54.5 ns — three
quarters — is the round trip through intermediate buffers. Each extra link costs
~6.9 ns wired against ~1 ns fused, so the ceiling for eliminating them is about
**4× on a long chain**, and that ceiling is exactly the shape of the gap the
`gain` bench still shows against Faust: a JIT emits the last column.

**But a generic engine cannot collect it.** The two fusion strategies that do not
need the operator sequence at compile time are both *far worse than the wires
they remove*: interpreting the ops per sample is 4.7× slower than doing nothing,
and tiling the block so the operator is matched once per 16 frames instead of
once per frame — the obvious middle path — is still 2.7× slower, because the tile
copy and the per-call overhead cost more than the wire saved. The last column is
only reachable when the sequence is known before the loop is compiled. There are
exactly two ways to have that: **hard-code the shape** (which is what `MulAdd`
and `Sum4` already are, and why they exist) or **JIT it** — and the JIT is not
missing from this project, it is the Faust family, sitting right there as a peer.

So the conclusion is a decision not to build a chain fuser: the win is real and
it is in the wires, but collecting it generically is a compiler, and we have one.
What remains open is cheap and narrow — adding fused *shapes* when a common
expression justifies one, on the `MulAdd`/`Sum4` precedent and against the bench
section that measures them.


---

## A UGen's trailing inputs may be declared optional, and only where the default is inert

**Context.** Inputs are positional and arity is exact, so a UGen that grows an
input breaks every def ever written against it: `PlayBuf` going from four inputs
to seven made every persisted def that used it fail to compile, and the server
warned about them at every boot. The same break reaches a saved bundle, which is
samples a person made.

**The cheap version is refused.** "Fill whatever is missing from the catalog's
defaults" would make a short def legal in every input slot of the catalog, and
`BinaryOpUGen` is `a=0, b=0` — a `Mul` truncated to one input would compile,
fill `b=0`, and silence the chain with no `/fail` and no name. That is precisely
the failure the unusual input order of `PlayBuf` was chosen to avoid.

**Decision.** A kind declares an **optional tail**: the trailing slots a def may
stop before, marked on the slot itself (`UGenInput::optional`), which
`synthdef::compile` fills from the declared defaults. The optional slots are
always a suffix, enforced by a test, so "how many inputs are required" is one
number and a short def is never ambiguous. A kind with no tail — which is most
of them, and every operator — still needs its inputs exactly.

**What earns a slot the tail**, which is the whole audit and the part worth
writing down: its default must be **inert**, the value that makes the UGen
behave as if the slot were not there — 0 for a trigger, an offset, a phase, a
channel, a done action; 1 for a level or a rate scale. A default that is a
*choice* (`freq=440`, `delaytime=0.2`, `width=0.5`, `max=7`) keeps its slot
required, because omitting it would not be "leave it alone" but "pick a number
for me". So does any slot the UGen reads its **signal, source, position or
chain** from, whatever its default, since silence and frame 0 are legal values
that a def missing one would run with, wrongly and quietly.

**Consequences.** Growth **by the tail** is non-breaking; an input inserted in
the middle stays breaking and stays silent, and nothing here pretends otherwise.
The fill happens in `compile`, on the network thread, once per def, and the
compiled def is byte-identical to one a complete client sent — so there is no
runtime cost and the audio thread never learns of it. And a declared default
becomes **wire contract**: changing one afterwards changes what every def
leaning on the fill sounds like, where before all of them were free metadata.
Clients are unaffected and keep sending every default before the last
input the caller gave: the wire is positional and has no sparse form.

## The built-in `default` is a subtractive voice with the MPE controls

*Decided 2026-10-01.* The `default` was a gated sine with `freq`, `amp` and
`gate`, and a pattern that passed it `pan` — the render-then-load examples do
— sounded centred with nothing to say why: the key reached no control. A sine
is also the one tone where an MPE zone's dimensions have nothing to move. So
the built-in became what a default instrument usually is: two sawtooths
detuned by ±0.4 % and a sine on the fundamental, through a resonant lowpass
whose cutoff follows the key and opens with the timbre, the pressure and a
velocity-scaled bloom, placed at equal power by `pan`. The envelope and the
gate release are the earlier decision's, unchanged.

*Revised the same day:* the sine went. It added nothing the saws do not
already put on the fundamental, and every oscillator here starts each note
at the same phase — `Saw` and `Sine` take none — so the sine summed in
phase with the saws at every onset, and across the voices of a chord, and
pushed the voice out of range.

- **The controls are named `press` and `slide`**, not `pressure` and `timbre`:
  short words beside `freq`, `amp`, `gate` and `pan`, and of a piece with the
  `lift` selector the server already had for note-off velocity. A zone's
  voice is started with those two names (the `/midi_map` *selectors* stay
  `pressure` and `timbre`: they name the MIDI message, not the control), a
  note curve over `{"pressure": ...}` or `{"timbre": ...}` drives them, and
  the Python client's MPE destination reads them from an event — so one
  pattern plays a server's zone and a MIDI port alike.
- **A channel's timbre rests at the centre.** The MPE decoder started a
  channel's timbre at 0 while the encoder sent 64 for a note that stated none:
  a device that never sends CC 74 would have played every note at the
  darkest. Both ends now agree on 64.
- **The graph is `crates/clausters-server/src/synthdef/default.json`**, the wire format verbatim,
  rather than a hand-indexed `UGenSpec` list: at 35 UGens the indices were
  unreadable either way, and JSON is the form a client-sent def has. The
  Python twins (`crates/clausters-server/examples/synthdef.py`, `tests/test_synthdef.py`) render it
  byte-identically, which is what keeps the file and the client builder in
  step.
- **Tests that listen for a sine have their own.** The engine, mapping and
  golden tests measured the old default's frequency and RMS; they now send or
  compile `crates/clausters-server/tests/common/sine.json`, the old graph, so a change of the default's
  tone never moves what the engine is checked against — the arpeggio golden
  stayed byte-identical.

## `scaleneg` is the UGen kernels' form, not the header's

*Upstream finding, 2026-10-09.* scsynth defines `scaleneg` twice. Every
`BinaryOpUGen` kernel computes `a >= 0 ? a : a * b` — scale the negative half,
which is what the operator is documented to do. The `float` specialization of
`sc_scaleneg` in `SC_InlineBinaryOp.h` is a branch-free rewrite,
`b = 0.5*b + 0.5; return (|a| - a)*b + a`, and it is not the same function:
below zero it gives `-a*b`, so `scaleneg(-1, 0.5)` is `0.5` rather than `-0.5`.
No kernel calls it, so scsynth's UGens are right and a plugin that reached for
the header function would not be.

The core takes the kernels' form, measured against them; the other new
operators from scsynth's table (`roundup`, `amclip`, `bitnot`, the four phase
windows, `ramp`, `scurve`) agree with scsynth's definitions on every argument
of a 24 001-point sweep, including the windows' mixed precision — Hann takes
the phase times `2π` in `f32` and the cosine in `f64`, Welch multiplies by `π`
in `f64` — which a port in plain `f32` misses on up to a third of arguments.
