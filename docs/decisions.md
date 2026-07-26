# Design decisions & findings

The non-obvious choices behind Clausters, and the upstream bugs we had to work
around — the "why it is this way" that the code alone cannot explain. Each entry
is written in ADR spirit: **context → decision → consequence**.

This is a curated record, not a changelog. *What* shipped and when lives in the
git history; the full historical build journal (kept for reference, frozen) is
[`history/build-log.md`](history/build-log.md). When a milestone makes a choice
with non-obvious context, add a short entry here — not a per-milestone diary.

## Threading and the RT-safe boundary

The OSC network thread may allocate, lock and do I/O freely; the audio thread
(the cpal callback, or `render()` in NRT) may do none of those. Everything the
audio thread needs arrives **fully pre-built over lock-free FIFOs**, and freed
memory leaves through a garbage FIFO to be dropped on the network thread.

- **Defs and their control-name tables live only on the network thread.** The
  `HashMap<String, Arc<SynthDef>>` and a mirror `node_id → Arc<SynthDef>` (kept
  from `/s_new`, cleaned on `Garbage::Freed`) stay off the audio thread, so
  `Cmd::SetControl` is plain-old-data and the audio thread never compares
  strings. `/d_free` only removes the map entry — live synths keep their `Arc`
  (exact scsynth semantics).
- **UGen wiring allocates nothing.** `UGenSynth::process` builds each UGen's
  inputs in a fixed stack array (`MAX_UGEN_INPUTS`) via `split_at_mut` over the
  wires; the topological order guarantees inputs only read earlier wires.
  Guarded by `assert_no_alloc`.
- **Asynchronous command semantics are deliberate.** A `/status` immediately
  after a command may report the old count: commands apply at the start of the
  next block. That is scsynth's model, not a race.

## `SynthNode`: one trait, symmetric def families

The node tree and FIFOs handle `Box<dyn SynthNode>`, never a concrete synth
type. This was introduced before it was strictly needed so that a second def
family (Faust) could join the tree without touching the engine or the tree.
Consequence: the `synth` (SynthDef/UGen) and `faust` (FaustDef) families are
independent Cargo features that combine freely — new work feature-gates against
`dyn SynthNode` and stays symmetric.

## Buses, execution order, and the network-thread mirror

- **Control buses bypass the command FIFO.** `/c_set`/`/c_get` operate directly
  on shared atomics from the network thread; a synth sees the change on its next
  block — the same effect as routing through the FIFO, without the traffic.
- **Execution order is audible and testable.** `Out` sums, `ReplaceOut`
  overwrites; the order tests use a `ReplaceOut(0.0)` "silencer" that wins or
  loses a bus depending on whether it runs after or before the source.
- **The mirror can run ahead of the engine.** The auto-order mirror reflects
  commands *when sent*, so a scheduled future bundle is already mirrored;
  `queryTree` may briefly show that future state and a re-sort converges on the
  next change. Correctness never depends on the mirror (see parallel groups).

## Parallel groups are bit-identical to sequential

The safety of parallelism must not depend on the network mirror (which can run
ahead). So `BusUsage` masks travel *to the engine* inside `Cmd::AddSynth` (and
are re-sent by `Cmd::SetUsage` when a `/n_set` touches a control used as a bus
index), and stage partitioning happens on the audio thread with its own data —
pure bitops, no allocation. A greedy per-block rule in child order closes a
stage on the first bus conflict, so writers to the same bus serialize in order.

Consequence: a stage's members touch pairwise-disjoint buses and never read what
the stage writes, so results are independent of interleaving and the stages
preserve order — **`--workers` changes only wall-clock time**, never the samples.
Golden and RT/NRT-identity tests guard this.

## Denormal protection (three independent pieces)

Subnormals appear in recursive states decaying to zero (filter tails, envelopes,
Faust recursions) and resolve 10–100× slower in microcode on many CPUs — exactly
as a sound fades out. Three guards, kept in lockstep:

1. **`dsp::denormals::flush_to_zero()`** puts the calling thread in
   flush-to-zero mode (MXCSR FTZ+DAZ on x86-64, FPCR.FZ on aarch64, via inline
   asm — the `_mm_setcsr` intrinsics are deprecated). Re-armed in every cpal
   callback **and** armed at the start of `render()`: FTZ changes results, so
   NRT must arm it too to stay sample-identical to live.
2. **`-ftz 2` in every Faust factory** flushes recursive variables below the
   normal range, independent of architecture and thread FPU mode.
3. The one exception: the Faust *compiler* path runs inside
   `dsp::denormals::normal_precision` — libfaust's interval typing aborts under
   FTZ/DAZ, and the guard restores the armed mode on exit.

Guarded by `tests/denormals.rs` and the Faust-tail test in `tests/golden.rs`.

## Faust embedding: decisions and upstream bugs

- **Ubuntu's `libfaust` is unusable for embedding** — built without the LLVM
  backend and shipped without headers. libfaust must be built from source with
  the LLVM backend; the reproducible recipe lives in `BUILD.md`.
- **A hand-written FFI, not a crate.** `faust-build`/`faust-types` do
  Faust→Rust codegen at build time (they need the `faust` compiler and static
  DSP source) and do not embed the JIT; there is no maintained libfaust binding.
  We bind ~30 functions by hand against the real headers, avoiding a libclang
  build dependency. Because dynamic linking is lazy, a mistyped FFI symbol only
  fails when called — so a "kitchen sink" test exercises every schema op once.
- **libfaust does not tolerate concurrent compilation.** Two compiler threads in
  one process SIGSEGV — Faust's global compiler state is not thread-safe, even
  for `createCDSPFactoryFromString`. Fix: a process-global `compiler::ffi_lock()`
  around every compilation call. A server has a single compiler thread, but the
  test harness (and any multi-server embedder) needs the lock. With every FFI
  compilation path behind it, the faust suites now run in the ordinary parallel
  harness; the historical `--test-threads=1` rule is retired (a SIGSEGV there
  would mean a path skipped the lock).
- **Instantiation does *not* take the lock.** Creating instances from an
  already-compiled factory is independent of the compiler's global state (JIT
  code + malloc; FaustLive does it concurrently with compilations). The lock
  stays only for compiling. (Open caveat: `deleteDSPFactory` may touch the global
  factory table and can flake under parallel test load — non-blocking for a real
  single-compiler server.)
- **Upstream bug — `boxFmod()` returns `abs`.** `boxFmod()` in
  `compiler/box_signal_api.cpp` returns the abs primitive (a copy-paste bug,
  present through master-dev). Workaround: build `fmod` from a
  `CDSPToBoxes("process = fmod;")` fragment instead of the binding.
- **Upstream bug — the `cos` box returned abs** likewise; fixed the same way.

## Faust is a default feature, and the wheel ships it

`faust` used to be an opt-in Cargo feature, purely to keep a development build
free of the libfaust dependency. The cost of that convenience landed on the
*user*: the packaged artifacts were built with the default features, so an
installed wheel could not compile a `FaustDef` at all — `/d_faust` replied
`/fail`, and one of the two def families was, in practice, unavailable. Two
families we document as peers cannot have one of them missing from the product.

So `faust` joins the default set, and the packaging bundles it:

- **A default build needs libfaust** (built with the LLVM backend, a one-time
  from-source install; `BUILD.md`). Building without it stays supported and
  tested — `--no-default-features --features synth,realtime,…` is a SynthDef-only
  server with no libfaust on the machine — but it is now the explicit choice,
  not the default one.
- **The wheel carries libfaust *and* libLLVM** in `clausters/_libs/`, staged by
  `clients/python/build_native.py` off the built artifacts (keyed by the exact
  soname the loader asks for). libLLVM is ~130 MB, which takes the wheel from a
  few MB to ~50 MB packed. That is not accidental weight: the Faust JIT *is*
  LLVM, and the alternative (static LLVM inside libfaust) is no lighter — the
  server binary and the embed cdylib would each embed their own copy, where the
  bundled shared library is loaded once by both.
- **`DT_RPATH`, not `DT_RUNPATH`.** `build.rs` emits `-Wl,--disable-new-dtags`
  with an rpath of `$ORIGIN`, `$ORIGIN/../_libs` and the build prefix. The
  `$ORIGIN` entries make the artifacts relocatable (the wheel's `_bin/clausters`
  finds `../_libs/libfaust.so.2`), and `DT_RPATH` is required because only it is
  inherited by *transitive* dependencies: libfaust itself carries no rpath, so
  its libLLVM is resolved through ours. With `RUNPATH` the loader would fall back
  to the system libLLVM — or find none.

## CI with a default `faust`: one shared libfaust, and a baseline JIT CPU

Making `faust` default turned every job that links the default build (clippy,
the default-set tests, the Python client, the release wheel/server) into a
libfaust consumer, where before only one dedicated job needed it. Two choices
keep that cheap and green:

- **One `libfaust` job builds it; everyone else restores a cache.** A single
  job runs the from-source build and, crucially, stages the libLLVM it JITs with
  *into `<prefix>/lib` beside libfaust.so*. Downstream jobs `needs:` it and
  restore the cache through the `.github/actions/libfaust` composite; the same
  `DT_RPATH` that makes the wheel self-contained (`<prefix>/lib`, inherited
  transitively) then resolves both libfaust and its libLLVM, so a consumer needs
  **no LLVM runtime installed** — only the restore. A warm cache makes every
  downstream job free. `release.yml` uses the same composite, so the wheel build
  no longer sets up libfaust by hand.
- **The build is vendored and pinned, so the bundle is reproducible.** The Faust
  source is too heavy to commit (it stays git-ignored under `third_party/faust`),
  so reproducibility lives in two committed files: `third_party/faust.pin` (the
  exact commit — the unpatched base, keeping the boxcos/boxfmod canaries valid —
  and the LLVM version) and `third_party/build-faust.sh` (the one recipe). The
  composite, `release.yml` and a developer all run *that* script and read *that*
  pin — the CI cache key included — so the bundled libfaust/libLLVM are the same
  everywhere, not whatever the build host defaulted to. **LLVM is pinned to 18**:
  it is Ubuntu 24.04's default (CI installs it from the distro repos, no
  apt.llvm.org), and the distro build targets a baseline x86-64, which keeps the
  wheel's ~130 MB libLLVM portable — it runs on any x86-64 CPU. A newer upstream
  build (e.g. apt.llvm.org's 21) is built for a higher baseline and can itself
  hit an illegal instruction on older machines, so it is the wrong thing to
  ship; a developer who happens to have another LLVM builds locally with
  `LLVM_CONFIG=llvm-config-NN` (the FFI surface is version-independent).
- **CI pins a baseline JIT target (`CLAUSTERS_FAUST_TARGET`).** The Faust factory
  JITs for `""` = the host machine, and LLVM tunes the code to the detected CPU —
  correct and fast on a *real* machine, which is why production leaves it unset.
  But virtualized CI runners can report CPU features the hypervisor then traps,
  so host-tuned JIT code hits an illegal instruction at run time on some Azure
  SKUs (seen intermittently, VM-dependent). The override forces a baseline
  x86-64 target. The string is a Faust `triple:mcpu` pair
  (`x86_64-unknown-linux-gnu:x86-64`); what actually pins the CPU is the **mcpu
  half**, since an empty mcpu is what sends Faust to `sys::getHostCPUName()`.
  (An earlier revision credited the *triple* for this, on the theory that an
  empty one let LLVM re-detect host features. That was wrong, and it hid the
  real defect for longer than it should have — see the resolution below.) The
  override is a plain env var read by `faust::compiler::host_target`, so only CI
  opts in — and it only works against a libfaust carrying the `setTarget()`
  ordering fix.
  - *Resolved (2026-07-20): the override never took effect — it is a Faust
    bug.* In `llvm_dynamic_dsp_aux.cpp`, both source-based entry points
    (`createCDSPFactoryFromString`, `...FromSignals`) call
    `factory_aux->setTarget(target)` **after** `factory_aux->initJIT()`. But
    `initJIT()` is what reads `fTarget` to pick the triple and mcpu, and with an
    empty mcpu it falls back to `builder.setMCPU(sys::getHostCPUName())` — so
    every factory was JITed host-tuned no matter what target we passed. The
    bitcode/IR entry points on the same file pass the target through the factory
    *constructor*, i.e. before `initJIT()`, which is the intended order and makes
    the source paths an oversight rather than a design choice.
    Measured on the emitted JIT pages (not the input bitcode — see the caution
    below), same DSP and same LLVM, counting VEX/EVEX instructions:
    unpatched with the override, 139 (including `vfnmsub231ss`, FMA, absent from
    baseline x86-64); patched with the override, 0 — only scalar SSE; patched
    without the override, 139 again, so production keeps its host tuning.
    The fix is to hoist `setTarget()` above `initJIT()` in both paths. It has not
    been sent upstream.
    Two SIGILLs are on record, both with the override nominally active:
    `auto_order::faust_synths_sort_by_their_reserved_buses` (run 29492474371) and
    `test_defs::test_faustdef_renders_through_the_seam` (run 29711088891). The
    core captured for the third (run 29714651299) traps on `kmovd %ecx,%k1` —
    AVX-512 — in anonymous executable memory, i.e. JIT pages, with masked
    `vmovss` and `vroundss` around it. Host-tuned code on a runner that cannot
    run it, exactly as the original entry supposed.
    - **Caution, learned the hard way.** This bug was briefly "ruled out" on
      three separate arguments, all wrong in the same way: they measured a stage
      the bug does not live in. Running the *input bitcode* through `llc` at the
      pinned target shows clean SSE2 — but that tests whether `llc` honours
      `--mcpu`, not whether MCJIT ever received the target. Reading
      `EngineBuilder::selectTarget()` shows no feature autodetection — true, and
      irrelevant, because the host tuning entered one level up through an empty
      `fTarget`. A tell was visible and misread: bitcode written by a supposedly
      pinned factory carried `target triple = "x86_64-pc-linux-gnu"`, LLVM's host
      default, not the triple we passed. When a pin appears not to work, verify
      the stage that actually fails — here, disassemble the JIT pages.
    - **Also found, unrelated and open:** valgrind on `auto_order` reports a
      use-after-free at process exit — `llvm::MCJIT::~MCJIT` reading a block
      already freed by an earlier exit handler, reached from libfaust's global
      `dsp_factory_table` destructor during `_dl_fini` — plus uninitialised reads
      in `global::initDirectories`. Our own `Arc<FaustDef>` refcounting audits
      clean. Neither is the SIGILL, but the first can still crash a run at exit.
    - **Not a bug, do not "fix" it:** `FaustArgs` passes `-ftz` as `argv[0]` and
      does not NULL-terminate. libfaust prepends its own `"faust"` `argv[0]` and
      NULL-terminates its copy (`libcode.cpp`), so the arguments arrive correctly.

## Def persistence: transparent JSON + a non-authoritative bitcode cache

Two layers, decided with the user:

- **B — JSON is the transparent source of truth.** `synthdefs/<name>.json` is the
  `SynthDefSpec` verbatim; `faustdefs/<name>.json` is a `FaustRecord`
  (source/JSON + libfaust version + payload sha256). Reload = recompile from
  there, by the same path as a fresh `/d_recv`/`/d_faust`. The `FaustDef` itself
  is never serialized — its factory is opaque LLVM JIT state.
- **A — the LLVM bitcode cache is non-authoritative.** `faustdefs/<name>.<sha16>.bc`
  is restored only if the libfaust version matches and the file reads cleanly; any
  miss recompiles from source and rewrites it. A libfaust upgrade invalidates
  every `.bc` automatically, and a corrupt cache never serves a wrong def
  (named by payload sha, so a stale `.bc` never pairs with a newer record).

## MIDI: standard channel-voice, byte-identical to OSC, in a shared crate

- **A MIDI voice realizes the *same* OSC commands an OSC client would send.**
  `CmdTranslator::translate_midi` maps note-on → `/s_new` (with `freq`/`amp`
  from named conversions), note-off → `/n_free` or `/n_set gate 0`,
  aftertouch/CC/bend → `/n_set` on live voices. Reusing the OSC path makes a MIDI
  voice byte-identical to its OSC equivalent, and the reserved voice-ID range
  (`MIDI_NODE_ID_BASE`) stays disjoint from client and `/s_new -1` IDs.
- **MIDI lives in a reusable native crate, not the Python client.** The original
  plan (MIDI 1.0 in a Python library) was scrapped: MIDI belongs in
  `crates/clausters-midi` (a versioned C ABI, shared by client and server), with
  the message layer in **MIDI 2.0/UMP** for high resolution (16-bit velocity,
  32-bit controllers, no 7-bit loss), persistence to a MIDI 2.0 clip file and to
  `.mid` (SMF) for interop.

## One global transport; the server broadcasts *control*, not audio

Decided with the user: a single global transport, and a conductor's
play/stop/locate drives every client's playhead in lockstep. The server
broadcasts transport *control* (`/transport_play|stop|locate`, pushed to
`/notify` clients) and **never schedules audio** — each client rolls its own
playhead on the shared grid. The `/transport.reply` grew `playing` and
`position` fields, kept backward-compatible (older clients read the first three).

## One seeded random context per script

Per-pattern seeds were a design error (corrected with the user): a piece is only
reproducible end-to-end if everything random shares **one** seedable context —
the sclang model. `main.seed(n)` seeds the root; every `Routine`/`Stream` derives
its own generator at creation from the creating context; a draw uses the running
routine's generator (thread-local), falling back to the root outside a routine.
One root seed reproduces a whole script in creation order, and concurrent
routines stay reproducible *per routine* regardless of wake interleaving. The
generator itself is the core's seeded `Rng` over the FFI, so a seeded script
replays identically in every client language.

## Real-time scheduling: opt-in, then restored as default

A two-step decision worth recording because it reversed:

- **First made opt-in (M24b):** the default `rtprio` promotion tripped RTKit's
  `RLIMIT_RTTIME` watchdog under sustained overload and the kernel killed the
  process with SIGXCPU — a silent death that hung clients. Overload must break
  the audio, never the process.
- **Then restored as default (M24c):** with a SIGXCPU guard in place (sustained
  overload demotes the audio thread to SCHED_OTHER — audio degrades, server
  survives), only the performance cost of *not* scheduling RT remained.
  Measured on one machine: ~300 stable 1-sine nodes as SCHED_OTHER (124% peak at
  36% avg) vs ~500+ as SCHED_RR (92% peak at 34% avg) — the average misleads,
  the callback must fit its **worst** block. An RT-scheduled callback is the
  standard operating mode of every production Linux audio client, so the default
  came back, kept safe by the guard.

## Editor y-axis navigation: gestures on the ruler strip, props in display units

The editor-grade views (waveform, spectrogram) zoom and pan vertically. Two
choices were on the table and both are worth recording:

- **Gesture surface: the y-ruler strip, not a modifier over the body.** The
  wheel over the strip zooms the vertical axis (anchored at the cursor's
  height within its lane) and dragging the strip pans it; the wheel over the
  body stays horizontal zoom and plain/Shift drag keep selecting/panning time.
  Spatial separation needs no modifier chord, leaves every existing body
  gesture untouched, and matches the audio-editor convention (Audacity, most
  DAWs pan/zoom an axis by grabbing its ruler). The strip only exists when the
  ruler is on (`ruler_y != "off"`), which is also the only time the axis has
  visual feedback to navigate against.
- **Prop shape: `y_start`/`y_len` in normalized display units** (0 = axis
  bottom, 1 = top; `0, 1` = no zoom; a non-positive `y_len` resets), on the
  shared `EditorProps` — not amplitude/frequency values. Display coordinates
  make the anchor math linear and unit-independent: the spectrogram's window
  survives switching `freq_scale` between linear/log/mel/bark without moving
  what is on screen (the nonlinearity lives entirely in the shader's
  display→bin mapping, as with the shader uniforms), and the waveform's
  amplitude window composes with `AMP_MARGIN` without leaking it into the
  protocol. Living in the widget tree means `/gui_set` drives it and the
  browser renders it through the same shared frame path with zero extra wiring
  (display + `/gui_set` parity; the drag/wheel gestures, native-only at
  first, later moved into the shared gesture machine both fronts drive).
  Changes emit `/gui_event id "view_y" y_start y_len`, the `"view"` posture.

The y state is deliberately **per widget** while the horizontal view is slated
to move into shared navigation groups (linked views): two lanes of one file
scroll in lockstep in time, but each keeps its own vertical slice.

## Linked views: explicit groups in the host core, shaped for the multitrack view

The linked editor views (a waveform lane and a spectrogram lane navigating as
one item) settled four decisions:

- **Grouping is explicit — a `link` (int) prop — not implicit by shared
  source.** An editor item legitimately wants *unlinked* views of one file
  (compare two zoom levels), and a link legitimately spans sources (aligned
  takes), so the source is the wrong grouping key on both sides. A negative
  `link` unlinks live, and the widget carries its current view along
  (unlink-to-diverge is the workflow); joining an existing group adopts it.
- **Group events carry the interacted member's id, not a group id.** Every
  `/gui_event` names a widget id; a group id is not a widget and would fork
  the event shape. A gesture emits `"view"`/`"selection"` once — linked
  members repaint but do not re-emit — and a script that cares which lane was
  touched gets that for free.
- **Shared chrome is plain composition, not an automatic strip.** A stack of
  linked lanes that wants one time ruler sets `ruler:"off"` on all but one —
  existing containers plus existing props, no new container kind, no protocol
  change. An automatic shared strip would need cross-widget layout coupling
  for something a prop already expresses.
- **The group state lives in the host core and navigates in session-timeline
  units.** The state (horizontal view + selection + playhead anchor, in
  `host/timeline.rs`) is keyed by group, not by window: membership spans
  windows and fronts, the group's length is the max over its members'
  registered data extents (a shorter take just ends earlier), and both fronts
  drive it through the same `Host` ops — the `/gui_set` interception lives in
  the core dispatch, so the browser gets group semantics with no front code.
  This shape is deliberately the seat of the future multitrack (DAW-style)
  view: a clip item there is a member with a *placement* (a start offset) on
  the same shared timeline — the one designed extension point; today every
  member sits at offset 0. Selection/playhead are mirrored group→members'
  `EditorProps` so everything that reads the widget tree (renderer, playhead
  animation, readouts) keeps working; the group is the single writer, so the
  mirrors cannot drift.

## Edit-back-to-data: flat event payloads, the binding path generalized to lists

The `bpf` envelope editor is the first widget that *writes data back*, and the
pattern it establishes is meant to be reused verbatim by the later drawn-buffer
and automation-lane cases. Four decisions:

- **Edited data flows to the script as new event *payloads*, never new
  addresses.** An edit emits `/gui_event <id> <tag> <flat values...>` — for the
  envelope, `"points"` followed by `t v shape curve` per breakpoint, ints kept
  int and floats float. The `/gui_*` address family does not grow per widget;
  bulk edits (a drawn buffer region) would ride one blob in the existing
  `samples_to_blob` little-endian `f32` layout. The flat-primitives boundary
  rule holds on the way back exactly as on the way in.
- **The server-bound direction is the widget-value binding generalized to a
  list.** A bound editor forwards its whole edited structure straight to the
  audio server — `Binding::message_args`/`Host::forward_args` send
  `addr prefix… values…`, the multi-argument form of the bound knob's
  `addr prefix… value` — bypassing the script exactly as `/gui_bind` promises.
  No new binding vocabulary: the same destination, prefix and prune rules.
- **The host's mapped resources stay read-only.** Edits mutate the host's
  typed tree and flow out over the two channels above; the mmap bulk path is
  never written through. Anything that must land in a shared file or a server
  buffer goes through the server (or the script), so ownership of bulk
  resources stays single-writer.
- **The envelope shape math lives once, in the core.** The editor draws
  segments through `clausters_core::envshape::shape_value` — relocated from
  the server, whose `EnvGen` now delegates to it (the same move the forward
  FFT made) — so what the editor draws is what the server plays by
  construction. Its FFI export is deferred until a client actually evaluates
  envelopes client-side (the tap-reader precedent): the Python leg only maps
  breakpoint lists to/from `Env`, which needs no curve evaluation. The
  breakpoint model, hit-testing and drag clamps are display logic and stay
  gui-side (`host/bpf.rs`).

The widget model itself is deliberately the future automation-lane shape:
values in any `[min, max]` (bipolar, unipolar, on/off lanes via the hold
shape — SC's step jumps to the *target* level at segment start, so a step
segment shows the next point's value), every standard `EnvGen` transition
curve, an optional exponential
display scale for frequency-like ranges, and times over an explicit
`[0, duration]` domain — so a multitrack automation view later composes this
model instead of designing a new one.

## The multitrack editor: beats in the data, samples on the axis, and one converter

The arrangement places elements in **beats**; the multitrack view places
clips in **timeline samples**. Two units, and the temptation is to pick one. Both
are load-bearing:

- **The view must be in samples.** A clip's body *is* audio data, and the
  placement work established that a member's data sample 0 sits at its `offset`.
  A take placed at its own frame count then lands 1:1 on the axis, and its
  waveform draws where it sounds. In beats, every body would need a scale factor
  that only the client knows.
- **The arrangement must be in beats.** Its elements are musical, tempo-relative,
  and they render onto a beat clock. Baking a sample rate into them would tie a
  composition to one engine.

**Decision:** keep both, and make the **editor driver the only converter** — one
beat is `sample_rate / tempo` timeline units. It is also where a *musical* `quant`
becomes the lane's pixel-drag grid, so the grid a clip is dropped on is the grid
the arrangement re-schedules on. The arithmetic itself is the core's
(`beats_to_secs` → `secs_to_samples`), never a second implementation, so a port
inherits it.

**Consequence:** `clausters.form` stays pure and transport-agnostic (it is the
piece a future TypeScript client factors into `clausters-core`), the host stays
unit-free (it only knows "timeline units"), and the whole conversion is a handful
of lines in one client module. Its dependency arrow is one-way — the editor
imports the arrangement, never the reverse — the same call that moved the envelope
point helpers out of the GUI submodule so `seq` would not depend on it.

## A buffer sounds through an instrument, not by itself

Rendering a `Buffer` element was deferred with a note that it "needs an
instrument". The temptation on picking it back up was to give the arrangement a
built-in sampler def.

**Decision:** a buffer is **data**, and it sounds through the def *named to play
it* — `Buffer(buf, instrument="take")`, whose event carries the buffer number in a
`buf` control. A buffer with no instrument is still a perfectly good element: it
draws in the editor and contributes its extent, but emits no event.

**Consequence:** `clausters.form` ships no DSP and no def of its own (it stays the
client-side structure it claims to be), the instrument stays the user's — any def
that reads a buffer works, at any quality — and the "data vs. process" split the
layer is built on holds at its most tempting exception. The one concession is
practical: such an event sets `legato = 1` so a take sounds its whole length,
where a note's default would cut it short.

## The patcher is a directed, typed graph

The logical side of a composition — members wired to each other — is drawn by the
`patch` widget (the P track). The first design drew it **undirected**: a bus as a
first-class node, a wire as an untyped `(control ↔ bus)` touch, on the argument
that a GraphDef says only that a control *names* a bus and the writing/reading
direction is the server's own topological sort — so a directed view would be a
guess. That was built, tested, and rejected; it lives on the `patcher-bus-as-node`
branch.

**Context — the undirected view could not be read.** On real patches a
bus-as-node, single-edge canvas fails at a patcher's one job, showing signal flow:
ports land on one side and zigzag to bus nodes, an inlet and an outlet meet the
same bus with no visible order, and the picture does not match the running
program. And the premise was wrong. Direction is **not** a guess from a control's
*name* — it is **structural**: a control that feeds an `In` UGen is an input (an
inlet), one that feeds an `Out` is an output (an outlet), one that feeds neither is
a value, not a port. The client that built the def (or reads a UGen descriptor)
knows this exactly — it is the same analysis the server does to order the graph,
not a naming convention.

**Decision — one directed, typed cord patcher, at two scales.** A box has
**inlets** (top) and **outlets** (bottom), each **typed** (`ar` audio, `kr`
control; level 2 adds `ir`), and a cord runs outlet → inlet, a rate mismatch
refused at the gesture. A **cord *is* a bus the user never numbers**: the inlet
defines the bus (its type, and the summing point for the cords into it — several
cords into one inlet *sum*), and the **client names one bus per connected net** and
writes it into the def. The two levels are the same grammar over different objects:

- **Level 1 — visual programming of a `GraphDef`.** Boxes are whole synthesis
  nodes (defs of all families, synths, groups) and buffers; a cord is a **server
  bus**. The graphic is **explicit and restricted** — a signal goes exactly where a
  cord connects it. The power to route arbitrarily is a property of the *server's*
  node/bus architecture (nodes and groups inside a control cycle), built in code,
  never drawn on this canvas.
- **Level 2 — visual programming of a `SynthDef`/`FaustDef`.** Boxes are UGens with
  `In`/`Out`/control edge boxes; a cord is an **internal wire** of the single def
  the patch compiles to.

**Who does what — the server and the client barely change.** The **server already**
allocates a GraphDef's bus numbers and **auto-orders** its members topologically,
considering the buses; the directed patcher adds no server ordering logic and needs
no new verb (the port directions come from the def the client already has). The
**client** contributes one small, language-agnostic pass — directed cords → one bus
named per net, summing fan-in — which lives in `clausters-core` beside the patch
document, so every client shares it. The Python arrangement model (`Group`/
`Generator` → `GraphDef`) is unchanged; the new work is GUI-side (the directed
`patch` widget).

**Consequence.** The picture reads as the program: outlet → inlet, typed, flowing
one way, no bus nodes cluttering the canvas. The honesty *inversion* from the first
design is deliberate — direction and type are **shown** because they are
structural; bus numbering is **hidden** because it is bookkeeping. The one thing the
drawing does not express is **feedback** (a reader ordered before its writer, a
block of latency): the patch graphic is a **DAG**, and a genuine cycle stays a code
construction, not a cord.

**The hardware output is not a box.** A cord *is* a bus, and buses are never
drawn or numbered — so the hardware output, being a bus, cannot be a box either.
There is no `OUT`/hardware node on the canvas and no drawn hardware net; every net
is a private bus (`b0`, `b1`, …). A signal reaches the speakers through a
**terminal def** — a `dac` with an inlet and no outlet, its `Out.ar(0, …)` baked
in — a member like any other, distinguished only by having no outlet to cord
onward. (This corrected an earlier design that kept a special `OUT` box wired to a
drawn hardware bus, which contradicted "a bus is never drawn." Level 2 is
different: there a def genuinely contains `In`/`Out` UGens, so they *are* edge
boxes of the patch.)

## A buffer in the patcher is a data value, not a box that is a def

The level-1 patcher draws **generators that are defs**: a box is a whole
SynthDef/FaustDef, a cord is a signal bus between two of them. A **buffer** is not
one of those. It is *data a def reads* — a `Buffer` control on a playback synth —
exactly as a number is data (a `freq`) or an envelope is (an `env`). So a buffer
in the patch is a **value fed to a box's parameter**, of the general family of
**parameter/value boxes** (a constant, an envelope, a buffer) that feed a def's
parameter inlets. That family is **not built yet**: the level-1 patcher so far
wires def-to-def buses only, and a def's own parameters-as-values are the
value-box work (P5, the consolidated editing surface). Buffers-as-boxes therefore wait for the value-box work,
rather than bolting a "buffer rate" onto the cord→bus pass now — the coherent
order is to introduce parameter boxes first, of which a buffer is one kind.

**Context — why not a third cord type now.** The tempting shortcut is a `buffer`
rate beside `ar`/`kr`, a "buffer outlet" feeding a `buf` inlet, compiled by the
cord→bus pass into a scalar the server resolves. But that pass is about **signal
buses** — connected components summing onto a numbered bus — and a buffer
reference is neither a signal nor a bus; overloading it would make the one clear
rule ("a cord is a bus") lie for one case. A buffer is a *value*, and values are a
different box family than signal-carrying defs.

**Decision.** A buffer in the patch is the arrangement's own `Buffer` (a reference
to a server buffer), kept in the model for **parity**: the patch is a visual
representation of a *part of the arrangement model*, so what it shows a buffer to
be is what the model holds, not a patch-only artifact. Two facilities follow, both
**deferrable and both off the server buffer the model already points at** — no new
model state:

- **Visualize** — to *see* a buffer, fetch its samples from the server and draw
  them the way the audio editor's waveform view already does.
- **Play** — to *hear* a buffer, build a **temporary synthdef** that reads it, the
  same move the editor makes to audition a take.

So P3 ships the def-generator patcher and leaves buffers to follow the
parameter/value box work — both parts of the plan, in their natural order;
recording the shape now keeps that later implementation coherent with the model
instead of a patcher-only bolt-on.

## The arrangement model: five primitives, one recursive group

A sequencing layer (a timeline of items, a playhead) is enough to *play* music
and not enough to *compose* it. A composition is an element inside an element — a
phrase inside a section, a take against a melody, a generator that has not been
evaluated yet — and the client had no name for that. The question was what to add.

**Context.** The tempting answer is a bag of editor types: an audio clip, a MIDI
clip, an automation lane, a folder track, each with its own fields, its own view
and its own rendering path. That is how a DAW grows, and it is also why a DAW's
granularity stops where its type list stops: you can edit a clip, but not "the
inside of that clip at the level you happen to care about".

**Decision.** A closed algebra instead of a type list. An **element** is anything
bounded that produces a unit of meaning — in one of two modes, and this is the axis
the layer turns on: **generated** (the rendered thing, editable data) or a
**generator** (the algorithm that renders it). Evaluating the second into the first
is the *change of state* — and it is a compositional act, not an optimization: what
separates the two modes is not data versus process but what can be *done* with
them. A generated element is random-access (an audio file plays backwards, slices,
edits in place); a generator is forward-only (it evaluates, in order, and that is
all). Bouncing is what turns something you can only produce into something you can
manipulate. An element carries only two temporal
properties — an *onset* and a *duration*, either of which may be absent (which
gives it a temporal *character*: a segment, a punctual event, a relative segment,
or a pure abstract context). There are exactly **five** kinds, and each is a thin
adornment over something the client already had, never a reimplementation of it:
an **Event** (parameters in one action), a **List** (strict order, no concrete
time), a **Buffer** (a list at constant time), a **Set** (mixed placement — a
track), and a **Function** (a *process*: server DSP, or a generator of sequences).
The one genuinely new structure is the **Group**: a recursive placement of
elements by offset, in two kinds — *concrete* (a relation in time) and *logical*
(a relation of processing, wired by buses). A group's temporal
relation (successive / simultaneous / mixed) is **derived** from where its members
sit, never declared.

Two consequences of the shape are load-bearing:

- **Rendering is a change of state, not a second engine.** Rendering flattens
  the tree — accumulating nested offsets into absolute beats — into the flat
  timeline the client already plays, bouncing any contained generator in the same
  pass. RT and NRT differ only in the destination, so the offline render stays
  sample-identical to what was heard.
- **The base level is the view, not the data.** How coarse or fine a group is
  shown (a summary rectangle, or its members resolved into lanes) is a property of
  the *look*, so the same structure serves the whole scale from a section to a
  note to a parameter — which is the granularity a type list cannot buy.

**Consequence.** The layer is pure and transport-agnostic: no DSP, no protocol, no
GUI — the piece a future client factors into the shared core. It carries the
temptations too: a `Buffer` is data and sounds only through the *instrument* named
to play it, and a logical group emits the bus-wired configuration the server
already expresses (a `GraphDef`) rather than a wiring language of its own. Both
exceptions were resolved in the algebra's favour, and both are recorded above.

## The piano-roll: OSC events get their own lane, and the notes live once

The editor-grade `pianoroll` draws the two message families a sequence carries —
MIDI notes and OSC events — and it had to place them without lying about what
each is.

**Context.** A note has a pitch, so it maps naturally onto the grid's vertical
axis (pitch × time). An OSC event does not: a `/trig` or a `/cue` is a moment, not
a pitch. Forcing it into the grid means inventing a vertical position for
something that has none — the same kind of lie the patcher refused when it
declined to guess signal direction from a control's name.

**Decision.** MIDI notes draw in the pitch × time grid; OSC events draw as flags
in a **separate lane** below it, on the shared time axis but with no pitch
pretence. Velocity gets its own lane too (the DAW convention), so the grid stays
one clean plane of notes.

**Reuse.** The note primitives — the notes, the pitch/time mapping, the drawing,
the hit-test, the drag clamps — live **once** in `host::pianoroll`, shared by the
dedicated `pianoroll` widget and the multitrack `clip`'s roll body (which now
delegates its drawing there). This is the `bpf::place_point` move again (G22h): a
mapping-free core both the widget-local and the clip-placed editors call, so the
two can never disagree on where a note is drawn or grabbed. The `Note` carries
`velocity`/`channel` (a real MIDI note), and the wire form is the flat quintuple
`start dur pitch velocity channel` — a length that is a multiple of five is read
as quintuples, anything else as a legacy `start dur pitch` triple list, so old
data still parses.

**Placement (the G7b rule).** The one piece of *general* musical knowledge — the
MIDI-note ↔ name / black-key spelling on the keyboard — went to
`clausters_core::scale` (`note_name`/`pitch_class`/`is_black_key`), beside the
perceptual frequency scales; its FFI export is **deferred** until a client
evaluates it client-side (the `envshape`/tap-reader precedent). Everything else is
display-only and stays gui-side. The edit-back stays flat payloads, not new
addresses: `"notes"` (`start dur pitch velocity channel …`) and `"osc"`
(`time label …`) — the fourth use of the one edit-back pattern.

**Multi-note selection: the marquee is the time selection, restricted in
pitch.** The piano-roll needed a multi-note selection without stealing the
grid's one free gesture — a plain drag on empty grid is the heavy views' *time
selection* (the linked group follows it), and a DAW marquee wants that same
drag. The resolution is that there is no second gesture: the marquee **is** the
shared time selection, restricted in pitch. Dragging the empty grid keeps
driving the linked views' time span exactly as before, and the notes inside the
time × pitch rectangle become the selected set (Alt+click toggles one note in
or out; Delete/Backspace removes the set; dragging a selected note moves the
block rigidly, its clamp computed as one so an edge stops the block instead of
folding it; the velocity lane nudges the set relatively, each note saturating
on its own). The set itself is **native view state**, not a wire prop: it lives
in the widget state, clears when the script replaces `notes` (the indices would
dangle), and every block edit reaches the script as the same flat `"notes"`
payload — the wire did not grow, so every client consumes block edits with the
code it already had.

**Edit-back to the data (the Editor's dedicated view).** When the `Editor`
opens an element as a dedicated piano-roll, a per-note edit writes back only to a
**generated element**: a `Track`'s editable `Timeline` is rebuilt from the
`"notes"` payload (times converted to beats, the OSC/MIDI events sharing the
timeline preserved). A generator (`Pbind`/`Routine`) is forward-only — there is
no second note to rewrite until it evaluates again — so its bounced notes are
shown **read-only**, and bouncing it to a `Track` is what makes them editable.
This is not a new rule: it is the generated/generator distinction of the
arrangement model (above) doing its job at the granularity of a single note.
OSC events stay display-only in this view too: the `(time, label)` flag is a
lossy projection of the message, so writing it back would silently drop the
arguments.

## Transport roles: TCP as the default command plane, UDP as the probe

**Decision.** The server accepts TCP on the OSC port **by default** (`--no-tcp`
opts out), and clients connect their command interface over TCP by default while
keeping the UDP *boot-or-attach* probe. Each transport has one role: **UDP** is
discovery and small real-time control (scsynth compatibility included), **TCP**
is the command plane, **shared memory** is the data plane (taps, control buses),
**WebSocket** is the browser's TCP, and the **in-process link** is the packaged
standalone's (no sockets at all).

**Why.** A UDP datagram cannot exceed ~64 KB (the OS rejects the send), and off
loopback anything over one MTU fragments at the IP layer, where a single lost
fragment silently drops the whole packet. That bounds exactly the payloads a
complex application needs to move — whole defs, GuiDef trees, buffer chunks,
query replies — while the deployments this server targets (loopback as the
common case, controlled networks for sound installations) get nothing back from
staying datagram-only. TCP's framing makes size a **configuration**, not a
protocol property: the length-prefix ceiling exists only so an untrusted prefix
cannot drive an allocation, so it is a boot option (`--max-frame`, default
16 MiB, advertised in `/server_info.reply` for clients to size bulk requests
from) rather than a constant — no limit is hard-wired, per the rule that the
project must stay usable as a desktop/mobile application without arbitrary
ceilings. Timing is unaffected by the switch: it rides on bundle timetags and
`/sched`, never on arrival time. Replies became transport-aware in the same
move (a `/tap_stream` window may fill a whole frame for a stream client; UDP
keeps the datagram-safe clamp), and the IPC command rings deliberately stayed
at 64 KiB — large payloads ride TCP even locally, and growing the ring would
bump the versioned segment layout for no demonstrated need.

## Package SemVer is decoupled from the binary ABI counters

Compatibility is tracked by **two monotonic integer counters** —
`ABI_VERSION` (the shm segment layout + embed C ABI) and `CORE_ABI_VERSION`
(the core FFI surface) — not by the package's SemVer. A release bumps a counter
only when *that* boundary changes incompatibly, and both are advertised by
runtime calls (`clausters_abi_version()`, `clausters_core_abi_version()`) that a
peer checks on attach/load, refusing to connect on a mismatch.

**Why.** A binary boundary needs a check that an *already-compiled* peer can make
at runtime, before it trusts a single byte of layout — SemVer strings cannot do
that job, and the two boundaries evolve on independent cadences (the core FFI is
already at v9 while the embed/IPC ABI is at v3). A monotonic integer per
boundary is exactly the scsynth plugin-ABI lesson: every binary seam is
versioned and verified where it is crossed. SemVer is left to govern the
*package* — what `cargo`/`pip` resolves — where it belongs.

The one **linkage** rule keeps them from drifting into contradiction: a release
that bumps either counter must also bump SemVer's breaking tier (the minor while
the major is `0`, per standard pre-1.0 SemVer where the minor acts as the major;
the major once at `1.0`). The reverse is not required — a minor can ship purely
additive source-API work without touching either counter. The mechanical
release rules live in `CLAUDE.md` ("Versioning"); this entry is the *why*.

## Client defaults for the wheel: sample clock (live only) and an enveloped default synth

Two out-of-the-box defaults, chosen for the common case of a **local** session:

- **The built-in `default` synth carries a gated envelope.** It was a bare
  `Sine(freq) * amp`, which clicked at note-on (level jumps from 0) and at
  note-off (the node is freed mid-cycle). It is now `Sine * EnvGen * amp` with
  a gated ASR — equal-power sine ramps (0.01 s attack, 0.3 s release),
  `doneAction = FREE_SELF` — the same shape as SuperCollider's `\default`. Because
  a click-free note-off *requires* a release ramp, and a direct `/n_free` cuts
  the ramp, the player must release this instrument by **closing its gate**. The
  global event default stays `has_gate = False` (a gate-less custom def is still
  freed directly, so it can never leak); the player special-cases `instrument ==
  "default"` to gate-release. So the safety of the free-by-default choice is kept
  while the built-in sounds clean.

- **`Session.live()` and `Session.embed()` anchor to the server's sample clock
  by default** (config `[client].clock`, default `"sample"`), rather than
  wall-clock OSC timetags. For a local session the sample clock is strictly
  better — drift-free and sample-exact — and making it the default also
  exercises the `/sched` path on every run. It falls back to wall-clock
  gracefully if no master answers, so a client with no Clausters server still
  works; `"monotonic"` opts out for driving a remote or non-Clausters peer.
  `Session.embed()` was excluded at first — the sample-clock tracker reaches the
  server over its own UDP socket, which an in-process server has no endpoint
  for, so auto-locking would only have burned the 2 s lock timeout on a
  guaranteed-failing attempt. It joined once the in-process reader existed:
  the embed handle already publishes the sample counter through shared memory
  (`clausters_clock`), so `EmbedSampleClock` reads it directly — no socket, no
  model, no timeout — behind the same tracker surface `lock_to` drives, and
  `Server.sample_clock()` picks the reader by interface. `render`/`nrt` never
  had a live clock.

## Id allocators are registries of finite resources, never counters

Node ids, buses and buffers are finite server resources fixed at boot; an id
allocator — on either side of the wire — is the **registry** of one such
resource's usage. Three invariants, adopted 2026-07 after an audit found every
allocator violating at least one: every released resource becomes allocatable
again; no monotonically increasing counter anywhere; no operation may lose
track of a resource. The audit's headline: the event player allocated one node
id per note and never returned it, so a long-running session marched the id
space monotonically into the server's reserved ranges (silent duplicate-id
rejections at two million notes) and eventually into `struct.pack` overflow;
the server's own `/s_new -1` and MIDI-voice counters wrapped `i32` in release
builds; the client bus allocator reused freed runs only at exact width and
would hand out the GraphDef reserved top of the bus space.

The fix is one shared implementation, `clausters_core::registry::Registry` —
a bounded occupancy map with a next-fit hint (releases are reusable and runs
coalesce by construction; a double release or foreign id is refused, not
absorbed; exhaustion is an explicit `None`) — used by the server's reserved
ranges directly and by every client over the core FFI, per the build-strategy
rule that language-agnostic logic lives in the core. Two design points carry
the weight:

- **Release follows the resource, not the request.** A node id returns to its
  registry when the node *dies* — the server releases auto/MIDI ids as `End`
  events drain, a client releases its ids on `/n_end` (never at `/n_free`-send
  time, which could re-hand an id whose node is still alive). The registry is
  passive (events are fed in; it never calls out), which keeps it identical
  across bindings and wasm-compatible. The corollary: an engine rejection
  produces no `/n_end`, so the server broadcasts `/fail` **with the id
  appended** — otherwise the client's in-flight id would be lost, violating
  invariant three.
- **Every capacity is bounded at boot, including node ids**, even though ids
  are "dynamic": concurrent nodes are bounded by the node table anyway, so
  the id space only needs table capacity plus in-flight margin
  (`NodeIdPartition::from_max_nodes` — client 4×, auto 2×, MIDI 2× — replacing
  the magic 2M/3M bases). A bounded registry turns a leak into a visible
  fail-fast error, preallocates once (no growth, no `i32` overflow), and lets
  clients size themselves from `/server_info`'s `max_nodes` by the shared
  formula — by query, not convention. The sanctioned exception is NRT/score:
  no real-time bound and no live `/n_end` stream, so a score client's node-id
  registry is unbounded by design.

**Generous defaults (2026-07-16 follow-up).** With every id space bounded and
recycling, the boot defaults were raised so a limit is only ever hit when
something is genuinely wrong: `--max-nodes` 1024 → **8192** (a node can be a
note; the id partition scales with it automatically), `--max-buffers` 1024 →
**4096**, `--control-buses` 1024 → **16384** (scsynth's own default),
`--max-graph-children` 256 → **512** (per *direct* children of one group — it
pre-reserves 8 bytes each per created group and never multiplies `max_nodes`).
The memory cost is small: ~2 MB of node slab, 64 KB of control buses (the shm
segment's default instance grows to 721 600 bytes), 4 KB per created group.
One capacity now *scales* rather than being constant: the node-event and
garbage FIFOs grow to `2 × max_nodes` at boot (floor 2048/1024), because the
registries recycle off `/n_end` — a dropped end event is a client id that
never returns, so a full-tree mass-free must fit one turnover per drain.

## The verbs divide by state: an element is rendered, a timeline is played

Extending the ambient verbs (`play`, `plot`, `render`) raised the question of
whether an arrangement element should also be playable — `play(song)` reads
naturally, and every other sounding thing goes through `play`.

**Decision:** the verbs follow the generated/generator split the arrangement is
built on. `play` takes what already sounds directly — events, patterns,
routines, defs and bare expressions, and a flat `Timeline` (already generated:
random-access, a `Playhead` just reads it). `render` is the *change of state*:
an `Element` is rendered, never played, and `play(element)`'s `TypeError`
points there. Without a destination, `render` **bounces** — an ephemeral NRT
session plays the source and returns `(samples, frames)`, optionally writing a
float32 WAV — so forward-only sources (an event pattern, a routine, a bare
generator) are renderable too; with a destination it delegates to the
arrangement's own seam (`form.render`), where RT and NRT already differ only by
the destination.

**Consequence:** each verb states a semantic, not a dispatch convenience — you
can read `play`/`render` in a script as "sounds now" vs. "changes state" —
and the historical byte-score `render` survives unchanged as the `bytes`
branch of the promoted verb. The coercions the verbs share live once:
`defs.as_def` (a bare `Ugen`/`Signal`/`Box` into an ephemeral def, used by
`play`, `plot` and `render`) and `render.bounce_def` (a def's offline samples,
drawn by `plot`, delivered by `render`).

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
- Per-argument expansion is deferred, not rejected: every constructor funnels
  through `Ugen(kind, inputs)`, so one hook there can add full expansion
  later, desugaring to the same container. The rules above are written down
  in the composition docs as the spec a later client ports.

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
audio thread**, by the typed `/b_gen prepare_partconv` routine into an
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
  compensation (PDC) remains open, per `docs/model-vs-daw.md`.

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
  **sample-identically** in `tests/spectral.rs`.
- **The conversion-cost risks resolved themselves in the identity path**: an
  identity phase program (the common, pure-magnitude case) keeps each bin's
  phase by scaling the complex pair — the exact arithmetic of the curated
  filters, no `atan2`/`cos`/`sin` at all (which is what makes the
  equivalence tests exact rather than approximate); the polar phase is
  computed only when a program reads it. The packed layout's real-only
  DC/Nyquist slots go through the same `get_bin`/`set_bin` accessors as the
  curated ops (imaginary part dropped there), and a zero-magnitude bin's
  phase is `atan2(0, 0) = 0` — no NaN can enter the frame.

## Native↔wasm render parity is a tight tolerance, not bit-identity

Context (the B track): the engine compiles to `wasm32` and the B0 acceptance
compares the wasm render of a score against the native NRT render of the same
bytes. Two platform facts make strict bit-identity the wrong bar there — and
only there (same-platform RT ≡ NRT stays bit-exact, unchanged):

- **Different libm implementations.** Native Rust lowers `f64::sin` & co. to
  the system libm; `wasm32-unknown-unknown` uses Rust's own libm port. The two
  legitimately round a few ULP apart — the same fact `tests/golden.rs` already
  records for its cross-platform tolerance. UGens call transcendentals per
  sample (`Sine` is `phase.sin()`), so the difference is unavoidable without
  forcing one libm on every target (a rejected option: pinning the Rust libm
  natively would change the native sample stream and buy nothing users can
  hear).
- **No flush-to-zero on wasm.** wasm has no FTZ/DAZ mode, so a render that
  enters the denormal range diverges from the FTZ-armed native engine. The
  parity scene is therefore required to be denormal-free (the generator
  asserts it), which keeps this factor out of the comparison entirely.

The bar chosen: max |delta| ≤ 1e-6 with the bit-exact count reported —
measured reality is 47990/48000 samples bit-exact and max delta 1.5e-8 (one
f32 ULP at signal scale) on the B0 scene, so the tolerance is three orders
above the legitimate noise and three below any plausible DSP bug. The harness
is `scripts/parity-web.sh` (native fixture generator + headless-Chrome page,
the same scripted-page pattern as the GUI's `web/parity.html`).

## The browser engine is one wasm instance inside the AudioWorklet, spoken to over the MessagePort

Context (the B track): the live browser backend has to host GUI
components that embed on **arbitrary pages** — documentation sites, notebooks,
pages we do not control the headers of.

Decision: one wasm instance — OSC translate + engine together — lives inside
the `AudioWorkletGlobalScope`. OSC bytes travel over the node's MessagePort in
both directions; commands cross into the engine through the same in-memory
ring the native embed mode uses (`Segment::in_memory`), and each 128-frame
render quantum runs one serving turn before its two 64-frame engine blocks
(the pulled `step()` pacing, tested natively in `tests/headless.rs`).

- **No SharedArrayBuffer, by requirement.** SAB needs COOP/COEP isolation
  headers, which an embeddable component cannot demand of its host page. The
  MessagePort path has no header requirement at all. The ring seam keeps a
  later SAB/wasm-threads build (a zero-message in-page `BusSource`) open as an
  optimization, not a redesign.
- **The one relaxation vs. the native RT rules**: OSC→Cmd translation
  allocates on the worklet (audio) thread. wasm malloc is a bump over linear
  memory — no page faults, no locks, no priority inversion — and the DSP
  itself stays allocation-free, so the native no-alloc discipline keeps its
  value without being extended to a heap that cannot misbehave the same way.
- **Synchronous instantiation, async compilation.** The main thread
  `WebAssembly.compileStreaming`s the module and passes it through
  `processorOptions` (a `WebAssembly.Module` is structured-cloneable); the
  processor constructor runs wasm-bindgen's `initSync` — the worklet never
  awaits anything, and is live from its first quantum.

Two findings worth keeping with this record:

- **The worklet scope has no `TextDecoder`**, and the wasm-bindgen glue
  instantiates one at module-evaluation time. The fix is import order:
  `worklet.js` imports `worklet-shim.js` (a minimal UTF-8 `TextDecoder`)
  *before* the glue — ES modules evaluate dependencies in import order, so
  the shim is installed when the glue's top level runs.
- **Chrome's `--virtual-time-budget` races timers ahead of the audio clock**,
  so the B0 parity trick (dump the DOM after a virtual-time budget) cannot
  drive a smoke that waits on real audio progress. `scripts/smoke-web.sh`
  instead runs Chrome in real time and has the page beacon its verdict as a
  `fetch` of `/smoke-verdict-…`, read from the HTTP server's access log.

## The browser bundle boot replays the persisted files over the wire; the only new artifact is a manifest

Context (the B track): a standalone bundle must boot in a tab exactly as
`clausters-gui --standalone` boots it natively. Natively the *embedded server*
loads the data directory itself (defs, `boot.json`) and the host only replays
the GuiDef; a browser has neither a filesystem nor an embedded loader.

Decision: the browser boots the **same persisted files, fetched as URLs and
replayed as ordinary OSC** to the in-page engine — no browser-specific bundle
format. The split of labor is deliberate: the boot's **ordering and encoding**
live in the GUI host's platform-agnostic `host::bundle` module (natively
unit-tested; it mirrors the server's own boot order — defs → graphdefs → boot
preset → the GuiDef's `boot` messages), while the **fetching** stays in page
JS (`clients/gui/web/bundle.js`). The replay is bracketed by two `/sync`s: the
engine serves strictly in order, so the trailing `/synced` is the page's
"bundle is up" signal — no per-command acking.

- **The one addition is `bundle.json`**, a manifest at the bundle's root
  naming the def files, because HTTP cannot list a directory the way the
  native store lists it. It is generated (`web/bundle-manifest.py`), never
  hand-maintained, and also carries the one genuinely browser-side mapping:
  which audio URL feeds which server buffer (fetch + `decodeAudioData` →
  the engine's `b_load` — the browser's `/b_allocRead`, decoded by the host
  page because the wasm engine has no sndfile).
- **The in-page leg is one more `ServerLink` variant, not a new protocol.**
  `ServerLink::Page` hands outbound packets to a page-registered callback and
  takes replies through `GuiBridge.server_reply`; the host's streamed data
  paths (`/c_stream`, `/tap_stream`, `/b_getn`, `/clock`) run over it
  unchanged — the acceptance smoke watches the meter's `/c_set` stream arrive
  with moving values.
- MIDI bindings, the remaining thing the native data-dir boot restores, are
  deliberately not replayed: the browser has no MIDI leg.

## The web package is plain ES modules over per-page singletons; the canvas moves to the last-booted element

Context (the B track's capstone): the web components had a deferred design
("converges when the milestone starts"). What converged:

- **Plain ES modules, no toolchain.** The `clausters` package in
  `clients/web/` ships as ES modules a static server can serve as-is; a
  `build.sh` stages the two wasm bundles (`engine/`, `gui-host/`) next to
  them. No bundler and no node dependency today — the TS track (W0) adds its
  toolchain *around* this package later, rather than this milestone
  front-loading one the repo cannot even run (no node on the dev machine, and
  nothing here needs it).
- **Two lazy per-page singletons, wired once.** `server()` (the engine) and
  `guiHost()` (the GUI host) boot on first use; the in-page server leg —
  engine replies → `GuiBridge.server_reply`, host outbound → `engine.send` —
  is wired inside `guiHost()`'s first boot, exactly once. The engine handle's
  single `onReply` slot is owned by the singleton and fanned out to a
  listener set, so any number of components, watchers and REPL scripts
  coexist; per-boot `/sync` ids keep concurrent bundle boots from mistaking
  each other's `/synced`.
- **One canvas, adopted by the last-booted element.** The browser GUI host
  shows one window-rooted def on one canvas (its long-standing shape), so
  `<clausters-bundle>` does not pretend otherwise: booting an element moves
  the page's host canvas into that element's shadow DOM (re-parenting
  preserves the GPU context and winit's listeners). Multiple elements share
  the engine namespace today and take turns showing; per-element windows are
  a host-side multi-window question, not a packaging one.
- **The gesture is the element's power button.** The autoplay affordance is
  standardized in the components themselves (`<clausters-bundle>`'s boot
  button, `<clausters-power>` alone for raw-singleton pages), not left to
  each embedding page.

## The web client toolchain is tsc + node:test, nothing else; OSC parity is a committed vector file

Context (the TS track's start): the web client needed a JS toolchain, and the
default answer in that ecosystem (vite + vitest, or any bundler) contradicts
both the repo's no-heavy-deps posture and the package's already-settled shape
(plain servable ES modules, wasm bundles and the AudioWorklet module as
static assets — the things bundlers fight).

- **`tsc` is the whole toolchain**: type-checker and emitter (module-per-module,
  `.js`-extension imports, output identical in shape to the hand-written B4
  modules). TypeScript 7 (the native compiler) is a single package with no
  transitive dependencies; `@types/node` rides along as declarations only.
  The dev server stays `python3 -m http.server`; the evaluation of
  vite/esbuild/vitest and the reasons each was declined are recorded in
  `clients/web/PLAN.md` ("Tooling").
- **Tests run from source under `node --test`** — node's native type
  stripping (default since 23.6) runs `.ts` directly, so the pure-logic
  suites need no compile step and no runner package. Browser-only behavior
  keeps the B-track posture (headless-Chrome smokes, access-log beacon);
  `clients/web/test.sh` is the one entry that runs all three layers.
- **Codec parity is a frozen artifact, not a convention.** The TS codec goes
  through `clausters-core` compiled to wasm (`crates/clausters-core-web`), so
  it is the server's codec by construction — and the tie to the *Python*
  client is held by `clients/web/tests/osc-vectors.json`, generated from
  `clausters.base._osclib` and committed. Both clients answer to the same
  frozen bytes; regenerating the file is an explicit act (new cases), never
  part of a test run.

## The web front-end lives in one package that mirrors the Python client

Context: the B track left the browser JS/HTML where each milestone was born —
the worklet/loader runtime and its harnesses beside the engine crate
(`crates/clausters-web/web/`), the standalone page, the bundle fetch module
and the manifest generator in the GUI host's harness directory
(`clients/gui/web/`), and the package itself in `clients/web/` — with a
two-hop staging chain copying the engine bundle between them, the bundle boot
implemented twice (the harness's single-consumer `bundle.js` next to the
package's `bundle.ts`) and the interim page codec still shipped after the
core-backed one replaced it. Unmaintainable as the client track grows.

- **`clients/web` is the only web directory.** All browser JS/HTML — package
  modules, the worklet/loader runtime, examples, test pages, tools — lives in
  the `clausters` package; the crates (`clausters-web`, `clausters-core-web`,
  `clients/gui`) stay Rust-only and their wasm-bindgen glue is staged into
  the package by `clients/web/build.sh`, directly, with no intermediate
  copies. The duplicates died: `bundle.js` was absorbed by `bundle.ts` (the
  standalone page now boots over the package singletons) and the interim
  codec was deleted.
- **The package structurally mirrors `clients/python`.** Sources under
  `src/` at the same relative paths as `clausters/`'s modules (`base/`,
  `gui/`, later `defs/`/`seq/`/`responders`/`session`), with `examples/`,
  `tests/` and (later) `docs/` beside them; `dist/` reproduces the `src/`
  tree 1:1 and the wasm bundles staged inside it (`engine/`, `gui-host/`,
  `core/`) are the browser's `_bin`/`_libs` — binary artifacts inside the
  installable tree, no source beside them.
- **Node-TS conventions, servable output.** Sources import each other with
  `.ts` extensions (`rewriteRelativeImportExtensions`), so node runs them
  directly (tests never need a build); `tsc` emits `dist/` with declarations
  and source/declaration maps — the browser interface is JS with a type map,
  ready for in-page typed consumption (livecoding). Imports of the
  wasm-bindgen glue keep `.js` and resolve against staged copies (full
  bundles in `dist/`; `.d.ts` stubs — plus the core's glue `.js` for
  node-from-source — mirrored into `src/`).
- Consuming the package from node to control local servers (a native
  WebSocket carrier, headless scripting) is a **separate future feature**
  ("Node target" in `clients/web/PLAN.md`), deliberately not folded into the
  consolidation.

## The piano's host voices use explicit node ids from a dedicated high window

Context (the `piano` widget's voice mode): a key press must spawn a server
voice the key *release* can later reach — `/s_new` on press, `/n_set <id> gate
0` on release. The server-assigned id form (`/s_new … -1`) was rejected for
exactly that reason: the host would never learn the id it needs to gate, and
adding a reply round-trip for it would put a network latency inside a played
note. So the host sends **explicit positive ids**, which the server accepts
from any client, and allocates them from a dedicated window:

- **Base `0x1000_0000`, wrapping over a `1 << 16` span.** Far above the Python
  client's counter (`1000..`) and the server's own auto-assign range
  (`1000 + 4·max_nodes ..`), so a host voice can never collide with a node a
  script created — the three allocators partition the id space by
  construction, with no coordination protocol.
- **No `/n_end` tracking.** A voice def is required to free itself on release
  (`FREE_SELF` on the gate envelope), so the host's bookkeeping is just the
  live `(pitch, node)` pairs per widget: the release (or a glissando, a
  re-press, a widget free/redefine — all of which gate the old voice) removes
  the entry. A 65536-id window wraps long before any voice from the previous
  lap can still be sounding.

The same reasoning will apply to any future host-managed spawner (an XY pad
playing voices, a drum grid): reuse this window, not a new one per widget.

## EnvGen: a gate already closed at the first sample is a release, not a wait

Context (found through the piano widget, but a server-wide property): a live
client's note-on (`/s_new … gate 1`) and its note-off (`/n_set … gate 0`) can
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
`tests/envgen.rs::gate_already_closed_at_the_first_block_releases_and_frees`.

## The 2D workspace has one uniform scale, and it clips with geometry rather than a scissor

Context (the `scroll` container): the workspace shows a virtual content area
through a panning, zooming window. Two implementation choices went against the
obvious reading of the roadmap, and both are load-bearing enough to record.

**One scale, two offsets — not a `viewport::View` per axis.** The plan said
"one `viewport::View` per axis, so the anchor-preserving zoom/pan math is
reused rather than rewritten". A `View` carries a *start and a length*, so a
pair of them carries two independent scales, and each axis' clamp adjusts its
own length — which silently de-couples them the first time one axis hits the
content edge, and the plane shears. A workspace showing boxes and wires must
never distort, so the state is instead **one uniform `view_zoom` (device pixels
per content unit) plus a pan offset per axis**. The reuse the plan asked for
still happens, just at the right level: the anchor-preserving *pivot math* of
`View::zoom` is applied to the shared scale factor (`host/scroll.rs::zoom_at`),
so the content point under the cursor stays fixed exactly as it does in the
timeline views. The constrained forms then fall out of configuration rather
than of geometry: `axis` gates which offsets a gesture may move, `zoom: 0`
gates the scale — one gesture path, as intended.

**A geometric clip, not a per-widget scissor.** A scrolled widget must not
paint outside its container. The reflex answer is a GPU scissor rect per
widget, but the host's whole light-widget economy rests on the opposite
property: *a whole window is one mesh upload and one draw call*, which is what
lets an application face cost what a panel of sliders costs (the L-track cost
rule). A scissor is pipeline state, so honoring it per widget would split that
batch into one draw per clipped widget. So the clip lives in `paint::Mesh`
instead: a clip rectangle set around each placed widget's geometry, applied by
a Sutherland-Hodgman polygon clip as triangles are emitted (a triangle against
a rectangle yields at most 7 vertices; fully-outside geometry is dropped
before it reaches the buffer). The batch stays single, and the clipping is
identical on both fronts by construction because it happens before the GPU.
The **heavy views keep a real scissor** (`host/frame.rs::apply_scissor`) —
they own their own pipelines and draw through `set_viewport`, which positions
but does not cut, so there is no batch to protect there and nothing else can
do the job.

**A third, found by using it: the plane and the scroll view want different
bounds.** The first cut clamped every axis to `[0, content - visible]` — the
window stays on the content. That is right for a *document*: a scroll view must
not scroll above its first row. Applied to the free plane it is wrong, and
visibly so: a plane sits at its content's top-left corner by default, so half
the drag directions were clamped dead and the whole gesture read as broken. The
bound now follows what the axis *means* — a constrained axis (`axis: "x"`/`"y"`)
keeps the strict clamp, the free plane overscrolls by half a viewport past each
edge (`host/scroll.rs::SLACK`), enough that every direction always moves it and
little enough that the contents can never be lost off-screen. This is the same
general-first rule the milestone states, applied to the bounds themselves: the
strict clamp is the *constrained* behavior, and letting the general case
inherit it gave the plane a restriction only the special case means.

## The UGen registry owns its input names; the client tables become mirrors

`/u_query` needed the catalog to report each UGen's inputs, and the descriptors
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
pure-Python package. The contrast test buys the same guarantee at the cost of
one test: for every kind whose signature maps 1:1 onto the wire, names and
defaults must agree with what the server reports (compared at `f32` precision,
since the server's defaults arrive widened to `f64`).

## The editable text field carries an internal clipboard, not an OS-clipboard dependency

The editable `text` widget needs cut/copy/paste, and that is the one editing
facility a GUI toolkit normally reaches the operating system for. We do not: the
native front (winit) exposes no clipboard, so real OS-clipboard interop would
mean a new crate (`arboard`/`copypasta`) — and libwgpu, the only heavy
dependency the host already carries, covers none of it. Against that, the host
already had the precedent from the piano-roll (G24h): a **host-wide in-process
clipboard**, a plain buffer that a copied block travels through between widgets
and windows. The text field reuses exactly that shape — a host-wide `String` —
so cut/copy/paste work with **zero new dependency**, at the one cost that the
native clipboard does not interoperate with other applications.

The browser is the asymmetric case: a page reaches the real OS clipboard through
the DOM (`navigator.clipboard`, the `paste` event) with **no Rust dependency at
all**, so the web front is where OS-clipboard interop is actually free. The first
cut ships the in-page clipboard on both fronts (functional parity within the
host); wiring the web front's copy to `writeText` and its paste to a DOM
`paste`-event listener is the recorded follow-up — an enhancement of one front,
not a dependency the whole workspace takes on.

## Representation before editing: the patcher's Def-views are autonomous, decoded client-side

The patcher track (P) first grew view and editing together, per level: P3
delivered the level-1 `GraphDef` view *and* its edit-back in one arc, driven from
the editor, and the level-2 milestone was scoped as "UGen boxes that compile to a
def" — a surface editable from the start. Two frictions surfaced. First, the
edit-back reached into the arrangement model early (`Group.declare_bus`, the
`Group → GraphPatch` mapping living inside `editor.py`), coupling a still-moving
view to the model before the drawing itself was settled. Second, nothing yet drew
a def's *internal* structure at all — level 2 was planned as an authoring surface
without first being a faithful picture of an existing `SynthDef`/`FaustDef`.

**Context — a patcher is easier to edit once it can be read.** A patch's one job
is to show a structure truthfully (the same finding that turned it directed and
typed — see "The patcher is a directed, typed graph"). Editing affordances —
creating boxes, drawing cords, rewiring — are far easier to get right when they
sit on a faithful, closed drawing of a structure that already exists (the def)
than when the view and the editing grow in the same step and destabilize each
other. And the structure is *already in hand*: a def the client built holds its
whole graph in memory — the DFS that `synthdef_ports` already walks to find the
`In`/`Out` controls is, one step deeper, every UGen and every wire — so a view can
be *decoded* from the def rather than authored from scratch.

**Decision — split the track into representation (phase A) then editing (phase
B), and make each Def-view autonomous.** Level 1 (`GraphDef`) and level 2
(`SynthDef`/`FaustDef`) are both delivered **as read-only views first**, decoded
from the def's own structure by an inverse pass (`GraphPatch.from_graphdef`,
`DefPatch.from_synthdef` — the reverse of the compile that renders them), before a
box can be created or a cord rewired. A Def-view is an **autonomous
visualization**, a peer of the heavy views (waveform, spectrogram, oscilloscope,
timeline, bpf, env): a host widget pure over its data, fed by a headless
decomposition (`defs`/`clausters-core`), openable on its own — one window per def,
the `clausters.plot` posture. The **editor does not own it**: the editor is the
gui-side *representation of the arrangement model* (`clausters.form` is pure and
never imports the GUI, so the dependency runs editor → form, never the reverse),
and it merely *orchestrates* — embedding an autonomous Def-view as a lane when a
composition holds a logical group, the way it embeds the piano-roll. The already
shipped level-1 edit-back (P3d) is reclassified into phase B, ahead of its
representation work, and its `Group → patch` mapping is lifted out of `editor.py`
into `defs`.

This is sharpened by a plain fact about the **current state of development** (not
a premise): no editing is realized yet. The level-1 edit-back (P3d) shipped as
code but is unproven in the window, and nothing else is built, so every surface
the track has today is, in practice, a representation. This is not a claim that
views cannot edit — editing is exactly what phase B adds, driver-side over the
edit-back seam (the widget emits gestures, the driver applies them).
Representation-first is therefore not only the better sequence; it is the honest
description of where the track already is.

**Decision — the internal graph is read client-side; the server is not
extended.** Level-2 representation reads the def object **in memory**, which
covers the authoring case (a def you built or loaded through the client).
Inspecting a def the server holds but no client in this process built would
require the server to report each def's internal UGen graph over `/d_query` —
**deliberately not done**: it would add per-def storage and processing to the
server for something the arrangement model does not do for its other abstractions
either. It stays an explicit later decision, never a prerequisite of this track.

**Consequence.** Phase A is `GraphDef → GraphPatch` (level 1, lifting the mapping
out of the editor) then `SynthDef → DefPatch` (level 2), plus a free-standing
opener; phase B is the editing — **consolidated into one milestone to plan**
(structural creation, parameter/value/buffer boxes, the live patch, and the
level-2 compile) that the shipped P3d edit-back seeds — followed by persistence as
its own milestone. The directed/typed grammar,
the cord→bus pass and the `clausters-core` patch document are unchanged — this is
a re-sequencing and a placement discipline, not a redesign of the model. The
free-standing opener is a **`plot_def()` method on the def classes**
(`graphdef.plot_def()`, `synthdef.plot_def()`, `faustdef.plot_def()`),
deliberately not a global verb and kept distinct from `clausters.plot(def)`, which
renders the def's *sound* rather than its structure.

**What level 2 added, concretely (P4), and what it did *not* add to the core.**
Because the level-2 Def-view is a *read-only decode*, it has **no cord→bus pass** —
a cord is an internal UGen wire, never an allocated bus, so there is nothing to
compile and nothing language-agnostic to place in `clausters-core` beyond the
shared drawing vocabulary. So the core gained exactly one thing: **`Rate::Init`**
(`ir`), the third cord weight (drawn dashed) the level-1 audio/control pair lacked;
the `DefPatch` *model* stays client-side Python, the peer of the client-side
`GraphPatch` (level 1's model is Python too — only its *pass* is core). This
refines the P-track's earlier phrasing that "the patch document schema and the
pass land in `clausters-core`": that holds for the level-1 pass; level 2, having no
pass, keeps its model where `GraphPatch` already lives. Two smaller decisions fell
out of decoding a def **headless** (`plot_def` runs no server): a UGen box's inlet
names come from **the client's own builder signatures** (`ugen_input_names`, the
same callables the `/u_query` contrast test pins to the server registry — no new
verb), falling back to positional for the generic op UGens and the wire-misaligned
kinds; and an **unset UGen output rate defaults to audio** — the exact per-kind
default is the server compiler's, not the client's, so the view takes the honest
common-case guess rather than mirroring the rate table. The decode stays faithful
where it must: `DefPatch.to_synthdef` reconstructs the def and reproduces its spec.
Faust decodes the tractable **signal-tree** form node for node; a **box-tree or
source** def is opaque (its internals are the compiler's) and draws as a single
box. Several refinements followed the on-screen reviews. **The node-positioning
is host-side, not client-side:** the decode ships no coordinates — positioning
belongs with the widget that draws, so every client rebinding it gets the layout
for free — and the panel frame hugs whatever boxes it holds rather than clipping a
fixed rect. **The layout is a small layered (Sugiyama-style) graph drawing, not a
tree algorithm:** a def is a **DAG** (fan-in, fan-out, shared sub-graphs, several
`Out` sinks), so a single-root tree layout (Reingold–Tilford) does not fit — it
would have to duplicate shared nodes. `host::patch::auto_layout` instead layers
each box by its **longest path down to a sink** (so an input lands just above
where it is used rather than piling into one top row — the mistake a first
attempt made by pinning all inputs to the top), then orders and places by iterated
**barycenter** relaxation (each box pulled to the mean x of its neighbours, layers
re-sorted and packed) so children line up under their parents; dummy nodes for
long edges are a noted future refinement. **Constants are value boxes, not inline
defaults:** each literal input becomes a small `const` box captioned with the
number and corded into its inlet (so every inlet is wired and the round trip is
uniform), drawn with a distinct `value_fill` so a data box reads as data. **Rate
reads by colour, not weight:** a cord is coloured audio-green / control-blue /
init-amber (init also dashed) — stroke weight alone was too subtle to tell apart.
And the panel **caption is the def *kind*** (`synthdef` / `faustdef` /
`graphdef`), not the def's name — the view names *what* it draws; the window title
carries the name. The per-box **role** the decode still ships (`source` / `const`
/ `object`) is now only a *drawing* tag (the `const` fill), not a layout input.

## Music notation: the client engraves a display list; verovio's DeviceContext is a proven-viable path kept in reserve

Notation is engraved by [verovio](https://verovio.org), which lays out a digital
score (MEI, MusicXML, ABC or Plaine & Easie) into resolution-independent
geometry: SMuFL glyph outlines placed by transform, plus engraving strokes and
fills (staff lines, stems, beams, slurs). Two ways exist to get that geometry
onto the GPU, because verovio renders through an **abstract `DeviceContext`** —
SVG is only one backend (`SvgDeviceContext`), and `Toolkit::RenderToDeviceContext`
is public. Either parse verovio's rendered **SVG** into a display list, or
subclass the `DeviceContext` in-process and receive the draw stream directly.

**Context — no GPU API consumes notation directly.** Neither WebGPU nor WebGL2
draws paths or SVG; they draw triangles and textures. So regardless of source,
notation must become a **display list of primitives** the host tessellates: a
glyph-outline table keyed by SMuFL codepoint plus placed glyph/line/fill
primitives in page units, each carrying the MEI `xml:id` it was engraved from.
That display list is the stable seam, and the host `score` widget is its *only*
renderer (glyph outlines and fills through a fill tessellator, staff/stems as the
painter's thick-line quads — one upload, one draw, WebGL2-safe). The open question
was only which **producer** feeds it.

**Decision — ship the SVG-parse producer, keep the native `DeviceContext`
documented as viable and deferred.** The engraver is driven client-side, its
rendered SVG walked into the display list and sent; verovio stays an **optional
client dependency the host never links**, so any later client (JS, wasm) reuses
the same host renderer by sending the same display list. It began as Python-only
and has since moved down into the workspace (`clausters-notation` for the
binding, `clausters_core::notation` for the pure walk/encoder/cursor fold,
`clausters-ffi` for the C ABI), which changes where the producer *lives*, not
what it is: still SVG in, display list out, still optional and still absent from
the host. The alternative — a native `GpuDeviceContext : DeviceContext`
compiled into the workspace — is real and was **empirically proven viable**, not
assumed: verovio builds from source as a standalone library (C++20, no external
dependencies — pugixml, the tuning library and the rest are embedded; no LLVM, no
Qt), a `DeviceContext` subclass overriding its ~35 primitives compiles against the
public headers, and rendering the sample score through `RenderToDeviceContext`
yields geometry **identical** to the SVG walk (22 glyphs, 29 lines, 1 beam; the
only difference is the page-margin transform the probe did not accumulate). The
licence is compatible either way — the workspace is GPL-3.0-or-later and verovio
is LGPL-3, so linking it in is clean, no separate process required.

**Consequence.** The two producers are interchangeable behind the display list, so
the native path is a drop-in replacement for the *producer alone*, never a rewrite
of the renderer. It buys three things the SVG walk cannot: no parsing round-trip,
in-process incremental relayout, and direct **edit-back** (mutate a note, re-engrave
in memory, no re-serialize). None of those pays off until the score view becomes
interactive — and editing shipped on the SVG walk after all (G31d), which reloads
and re-engraves rather than mutating in place. What actually pins the producer is
**cross-client parity**: verovio's wasm build only emits SVG, so a native
`DeviceContext` would leave a wasm client on a *different* producer that would have
to agree with it bit-for-bit, whereas the SVG walk is one producer every client
shares (native libverovio→SVG and verovio-wasm→SVG both feed the same walk). So
the notation layer's move into `clausters-notation`/`core`/`ffi` kept the SVG
walk, and the native `DeviceContext` is a **deferred, optional producer-swap** —
decoupled from editing, not scheduled with it. The verovio clone under
`third_party/` is kept as the reference for that later build and for the
libverovio binding; nothing in the shipping path depends on it.

## Score editing: verovio's editor is dead in the released wheel (upstream `#define` bug), so the producer decides the route

verovio carries a complete editing surface of its own: `EditorToolkitShared`
implements `drag`, `set`, `insert`, `insertControl`, `delete`, `keyDown`,
`navigate`, `chain`, `commit` and **undo/redo**, reached through
`Toolkit::Edit(json)` and surfaced by the Python binding as `tk.edit(dict)` /
`tk.editInfo()`. On paper that is the whole first editing pass for the `score`
widget, client-side, with no C++ build.

**It does not work in the released wheel.** In 6.2.1 the editor instance is
created by `Toolkit::SetViewAndEditor()` inside `#if defined NO_HUMDRUM_SUPPORT`
— the wrong macro (the editor has nothing to do with Humdrum; the intent was
`#ifndef NO_EDIT_SUPPORT`). The PyPI wheel is built *with* Humdrum support, so
the guard never opens, `m_editorToolkit` stays null, and **every** `edit()`
returns `false` with an empty `editInfo()` — including parameterless actions like
`undo`, which the parser would otherwise accept unconditionally. That last detail
is what makes the diagnosis certain rather than a guess about action payloads.
The symbols are all present in the shipped `.so`, so it is not a
`NO_EDIT_SUPPORT` build; it is the guard. Upstream fixed it on 2026-05-27
(`8100cb396`, "Invert #define fix"), after 6.2.1 — the clone under `third_party/`
carries the corrected `#ifndef NO_EDIT_SUPPORT`.

**Consequence — three routes to editing, and they differ in cost, not in
capability.** (1) Ship verovio ≥ 6.3 once released: editing arrives in the Python
client for free, no build. (2) Build verovio ourselves — the deferred native
producer — which unblocks the editor *now* and additionally buys the in-process
relayout and edit-back the previous entry describes. (3) Mutate the MEI in
Python and re-engrave: `getMEI()` out, edit the XML (we own the `xml:id`s),
`loadData` back. It needs no editor toolkit and no C++ at all, at the price of a
full relayout per edit and of implementing the semantics ourselves.

Route 3's price is smaller than it sounds and should be measured before it is
assumed away: a full engrave of a six-bar page — load, lay out, render, walk the
SVG into the display list — is **~17 ms**, from MEI or from Plaine & Easie alike.
That is inside a frame budget for a page of this size, so "incremental relayout"
buys nothing yet; it starts to matter at a page count a rehearsal score reaches,
not at the sizes the editing work will first be shaped against.

**Resolved: route 2 — vendor and build verovio ourselves.** It is the only route
that does not either wait on an upstream release or reimplement the editing
semantics, and the build turned out cheap: verovio vendors all its dependencies,
has no submodules, and needs nothing but cmake and a C++20 compiler. The
arrangement follows the one already proven for libfaust — the source is *not*
committed (a clone is ~140 MB, git-ignored under `third_party/verovio`) and
reproducibility comes from two committed files, `third_party/verovio.pin` (the
remote and the exact commit) and `third_party/build-verovio.sh` (one recipe).
`third_party/BUILD-VEROVIO.md` documents it.

The pin is a `develop` commit past the `#define` fix rather than a tag, because
no released tag contains it; **repin to 6.3.0 when it ships**, at which point the
published wheel is usable again and building stops being mandatory for the Python
client. It stays required for the native producer.

**One artifact, not one per language.** The build produces the shared
`libverovio.so` with its headers and SMuFL resources, and every consumer goes
through that: the Python client binds the C wrapper (`tools/c_wrapper.h`) with
`ctypes`, `build_native.py` stages the library and its data into
`clausters/_libs/` beside libfaust, and a wasm build would expose the same
functions. This is the libfaust arrangement reused verbatim, and it was reached
the wrong way round first — via verovio's SWIG Python module, which is what the
project's own packaging offers. That module is a trap in this context: a second
compile of the same sources, a second copy of the engine and its 12 MB of glyph
data in `site-packages`, and a distribution literally *named* `verovio` that pip
can replace with the published one, whose editor is dead. It did replace it, in
this checkout, and the editing tests began failing for an upstream reason. A
library we build, bundle and load by path has none of that ambiguity, and it
keeps the client's `dependencies = []` intact.

**Building it ourselves also means choosing what is in it,** and the importers
are independent cmake options. The build keeps MEI (the canonical format the
edit cycle round-trips through), MusicXML and `.mxl`, and the two compact
hand-typed formats, ABC and Plaine & Easie; it drops Humdrum, GABC and DARMS.
Only Humdrum is a size argument, and a real one — it vendors humlib, ~148k
lines, and dropping it takes `libverovio.so` from 21 MB to 13 MB. The other
two are noise (ABC and PAE measure ~10 KB apart) and are out because nothing
reads them. The tempting inference — that the fonts are the weight — is wrong in
the direction that matters: SMuFL *is* 12 of the 30 MB **installed**, across
2656 one-glyph XML files in five music fonts, but those compress to ~600 KB in a
wheel, so they cost disk, not download.

A last twist on the `#define` bug: in 6.2.1 the editor and Humdrum were
entangled, so this very trim would have revived the editor there. Past the pin
they are independent — which is why the build can be both small and editable
without trading one for the other.

**A second upstream bug, found once the editor was alive:** `undo` and `redo` on
an *empty* undo stack dereference it and **SIGSEGV** (reproducible, exit 139).
The irony is operational, not academic — parameterless `undo` is precisely the
probe that proves the released wheel's editor is null, and that same probe
crashes the build where it works. So our editing layer must **gate `undo`/`redo`
on `editInfo()`'s `canUndo`/`canRedo`** and never issue them blind. Note those
flags are looser than they appear: a successful `drag` leaves `canUndo` false,
yet a following `undo` succeeds and sets `canRedo`. They are a crash guard, not a
model of the stack — which means the client, not verovio, will have to track
whether there is anything to undo.

## Engraving sequencing data: target MEI, and a starting-point rhythm policy

`from_notes`/`from_timeline` turn the client's own `Event`/`Timeline` into a
score — the inverse of the score→sound flow. Two choices shape it, and the
second is a **starting point, not a fixed rule**.

**Target MEI, not ABC, even though the examples are typed in ABC.** For a human
typing, ABC's compactness wins; for a *generator*, its context-dependence is
exactly the hazard — an accidental persists through the bar, beaming is driven
by whitespace, octaves are marks. MEI spells every note's pitch (`pname`/`oct`/
`accid`) and value (`dur`/`dots`) explicitly, with no state to track, so the
encoder is a straight map from data. It is also the format the edit cycle
already round-trips through, and it needs **no `xml:id`s** — verovio mints them
on load exactly as on the ABC path, so id stability across editing is unchanged.
The generator is therefore a thin MEI writer feeding the existing `engrave`/
`Score`, not a new path.

**Written duration is `dur`, and off-grid durations tie rather than tuplet.**
The initial version engraves each note as its written `dur` (the delta), not its
sounding `sustain` — notation shows the written value, and legato/stretch are
performance nuances a score does not draw. Durations that are not a single note
value decompose into **tied** notes (a dotted value when exact), and a note
overrunning a barline splits and ties across it; anything finer than the 32nd
grid snaps to it. Both are deliberately the *simple correct* policy, not a
ceiling: reading `sustain`/`legato` for staccato and short-of-slot notes, and
detecting tuplets instead of snapping, are the undated engraving-refinements
milestone (`clients/gui/PLAN.md`, G31g). The implementation keeps the two seams
that milestone needs — the `dur`→value step (`_pieces`) and the pitch spelling
(`_spell`) — each isolated in one helper, so the refinement extends them rather
than rewriting the encoder.

## The notation C ABI has no one-shot engrave: size-then-fill needs a deterministic payload

Every entry point in `clausters-ffi` that hands back text or JSON is
**size-then-fill**: the call returns the byte count the result needs and writes
it only if it fits, so a binding sizes with a null buffer and fills with a
second call. That contract has an unstated premise — *the payload is the same on
both calls* — which every previous user of it satisfied by being a pure
function of its input.

The engraver is not. It mints a fresh `xml:id` per element on every load, and
those ids vary in **length**, so a one-shot `engrave(data) -> page JSON` would
lay out two different documents across the size call and the fill call. The
second could be a byte longer than the buffer measured for the first, at which
point the fill silently does not happen and the caller is handed a size again —
a contract that can only be used in a retry loop, for a function whose result is
supposed to be a single page.

So the ABI exposes no one-shot. A binding's one-shot is
`clausters_score_open` → `clausters_score_display_list` → `clausters_score_free`:
the ids are minted once when the handle opens, and the page is stable for as
long as it lives, so the size and the fill see the same bytes. It costs one
extra call and one extra free, and it makes every size-then-fill entry point in
the crate deterministic without exception. The Rust-side one-shot
(`clausters_notation::engrave_svg`) stays — it hands back an owned `String`, so
determinism never enters into it.

The general rule this leaves behind: **anything nondeterministic gets a handle,
not a size-then-fill pair.**

## A pass ends in the playhead's data, and every view drives one transport

Two facts kept being re-derived by every script that plays into a view.

**When does a pass end?** `Playhead`'s feeder returned when the timeline drained
but left the playhead marked as running, so `playing` kept answering yes forever
after the last item and `position` kept interpolating off the end. Each caller
timed the end itself, differently: the score view compared against the last
note's onset plus its length, the multitrack against the composition's extent,
both polling every frame or two. The fix belongs where the fact already is — the
feeder knows it drained — so the drain records it: `playing` goes False and
`finished` says the end is why, as against a `stop` by hand.

It **records** rather than announces. A callback would fire on the clock thread,
which is the one thread a client must never do work on, and the work a transport
does at the end is exactly the wrong kind (a widget update, a socket write). A
flag read from the script's own loop keeps the clock thread free and costs a
comparison. It is also the scan that ends, not the sound: a loop never finishes,
and the last item keeps sounding for its own length — a playhead schedules
items, it does not wait for them.

**What plays a view?** The transport — play, pause, stop, locate, plus the two
numbers the line is made of — is not about what a view draws. `playhead_at` is
one anchor on the engine's sample clock that the host sweeps from; the static
`playhead` is where a stopped transport sits. Both are the same for a lane, a
piano-roll and an engraved page. So there is **one** `Transport`, and a view
contributes exactly one thing to it: the unit its static cursor is placed in
(timeline samples for a lane, score milliseconds for a page). What each view
still owns is what a pass *is* — a render of the arrangement, a timeline built
from the engraved notes — which enters as a callable, not as a subclass.

The alternative, a transport per view, is how a client ends up with three of them
that disagree about the end of a piece — which is precisely what was found:
one of the two copies corrected the anchor for the server's latency and the
other did not, so its line ran early by exactly that much. A port keeps the
shape: the arithmetic is small, but its being in one place is the point.

## GUI widget ids are allocated client-side and recycled; a name is the stable handle

Context (the client's GUI ergonomics): a high-level client should not make the
user pick and thread integer widget ids — the audio-server side never does (a
script writes `server.synth("beep", freq=440)`, and the client's
`NodeIdAllocator` names the node). The GUI's id handling grew crudely in the
opposite direction: two disjoint monotonic counters (the host client from 1000,
the multitrack editor from 10 000, partitioned only by convention), neither
recycling, and examples that hand-pick ids (`knob(10, …)`) purely so they can
match the `/gui_event` back. Two questions had to be settled: **where** ids are
allocated, and **how** a script refers to a widget without naming an integer.

**Allocation stays client-side, mirroring `NodeIdAllocator`.** The tempting
alternative — the host assigns ids the way `/s_new … -1` lets the audio server
assign a node id — was rejected for the GUI for the same reason the piano's host
voices reject it (see above): the client needs the id *immediately* (to `set`,
`bind`, wire an edit-back), so a host-assigned id would force a reply round-trip
into the build path, and an async id-resolution step into every `open`. The
audio server's own primary path is client-side too; the `-1` convention is a
secondary path for cases where the *server* generates the node (a GraphDef's
members). The GUI has no such server-generated widgets, so it needs no `-1` path
at all. So a single `GuiIdAllocator` — the GUI sibling of `NodeIdAllocator`, over
the same core `Registry` — owns the one namespace, and the editor draws from it
instead of a second counter. It is **bounded and recycling** (unlike a bare
counter): a freed subtree returns its ids to the pool, and re-defining a window
frees the old subtree first, so the editor's redraw churn reuses ids within a
fixed window instead of climbing forever.

**A widget is named, not numbered.** `open`/`define` return a window handle that
is the window id *and* resolves the tree's `name`d widgets, so a script writes
`win["cutoff"].set(value=800.0)` / `.bind(…)` / `.on_event(fn)` and never touches
an integer — the `WidgetHandle` delegates to the host with the resolved id, the
way `Node.free()` delegates to its `Server`. The `name` is a **client-only** key:
it is stripped from the JSON, so the host still sees only ids and the wire is
unchanged. The deeper reason to prefer a name over a hand-picked id is that the
name is the *stable* identity: an assigned id recycles across redraws, so it is
the wrong thing to hold, whereas the name a script drew a widget with is what an
edit-back or a live `set` should address against. This keeps the wire protocol
and the host untouched — the whole change is the client growing an allocator and
a handle layer, no `ABI` counter moves — which is the "when a feature could live
in the client or the wire, keep it where the system already keeps it" rule.

## Docstrings are Google-style Markdown: markup a reader has to skip is a cost

Python docstrings are read twice, and the *first* read is the one that decides
this: in the source, by whoever is editing the function. Sphinx/RST field lists
and roles (`:param x:`, or `:class:` wrapped around a symbol) pay for their
structure in characters
that a human has to look past on every read — colons, role names, nested
backticks around what is otherwise an ordinary sentence. Google-style sections
plus plain backticks say the same thing with markup that reads as prose. So the
convention is **Markdown, Google style, no RST** — the rule that keeps
`clients/python/docs/build.sh` and the rustdoc honest is a consequence, not the
reason.

The toolchain agrees, which is why the choice costs nothing: nothing in the
chain speaks RST. The two books are mdBooks, the client's API page is generated
by pydoc-markdown (a static AST parse emitting Markdown), the Rust reference is
rustdoc. An RST role here is not "less supported" — it is inert text that lands
verbatim in the published page.

**Cross-references fall under the same rule, and cost more than they look.** A
reference only means something after a resolver has run: it needs a symbol table
plus a URL scheme for the output. Move a symbol or change generators and it
breaks — which is why the two books deliberately cross-link by their Read the
Docs URLs rather than by symbol resolution.

The consequence is sharper when the reference sigil is a character that occurs
in ordinary prose. pydoc-markdown's own `#name` syntax (neither Markdown nor
RST, a third thing) is matched by `\B\#`, so the quote in `"#rrggbb[aa]"`
satisfied it; nothing resolved, and the fallback **rewrote the text** — dropping
the `#` and injecting backticks inside an already-open code span — so the
published reference documented the color format as `"rrggbb[aa]"` in all four
places it appeared. The source was fine; only the artifact was wrong, behind one
build warning. `#` is everywhere in this domain (hex colors, shell comments, the
C preprocessor, anchors, issue numbers), so the collision was a matter of time.
The `crossref` processor is therefore **not** in `pydoc-markdown.yml`: removing
it changed exactly those four lines out of a 351 KB page, so nothing was relying
on it. The general form: a docstring should mean the same thing read raw,
through `help()`, or rendered — and a syntax that only resolves in one of those
does not merely fail to link when it misfires, it edits your prose.

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
fundamentals. Doubling the corrected span buys +29 dB at the bottom and +10 to
+12 dB over the rest, for two extra polynomial evaluations per cycle — a trade
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
a future plugin-delay compensation. A delay's delay is what the user asked for;
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

## `Line` is `EnvGen` with its header filled in, and the done flag is not the done action

Two decisions from U4, both about *not* growing a second mechanism.

**`Line`/`XLine` are the segment engine, not a second ramp.** They could have
been forty lines of `start + t·(end − start)` each. Instead they assemble
`EnvGen`'s input layout in their stack frame — a gate held open, one segment,
no release node — and call it. That buys the whole `doneAction` set (including
the relative ones, which scsynth's `Line` also accepts), the same landing exactly
on the target, and the same shared `envshape` arithmetic a client draws a curve
with, so a ramp the editor shows and the ramp the server plays cannot drift.
What it costs is one indirection per block and an input array on the stack;
the RT-safety guard covers the claim that this is not a heap allocation.

The wrapper is the `PvMag` pattern once more: one struct, a shape enum, two
registry rows, and no `Ramp` kind on the wire.

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
either way), but a latched `PauseSelf` would re-pause the instant `/n_run 1`
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
as 0.5. Reproduced rather than corrected, on the same rule as `hypot_apx` in U0:
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

## Noise is reproducible, and two instances are never the same stream

Every generator draws from `clausters_core::rng` — the xorshift the sequencing
layer and the client's `Pwhite` already use — and each can be built from an
explicit seed, so a render replays exactly. That is what lets a patch with noise
in it have a golden file at all.

Each *instance* takes its seed from a shared atomic counter, so two `WhiteNoise`
UGens in one def are two streams. This is invisible until someone writes one:
correlated noise summed with itself is a comb filter, not more noise, and
subtracted from itself is silence. There is a test that puts two in a graph and
subtracts them.

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
Taylor's and buys `quarter_sin(1) == 1` exactly. It also buys most of the
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
`examples/bench.rs` measures the gap on the same graph
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
  gain, and buys a catalog with one name instead of an encoder and a decoder
  that must be kept inverse to each other by hand.

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
rejects a non-finite constant outright (`src/synthdef/mod.rs`), JSON has no
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
UGens**. So `Pull` (`src/synthdef/instance.rs`) borrows the *prefix* of the UGen
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
which row is paying for it, and `tests/oscillators.rs` carries the measurement
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
cache-line alignment of `Block` (`src/dsp/mod.rs`): a block is `#[repr(C,
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

At the engine, `examples/bench.rs` over three interleaved rounds: the default def
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
3, and buys **1.19–1.23× on the all-signal shapes, 1.03–1.09× on the
constant-heavy ones** — real, consistent, and an order of magnitude less than the
operator match was worth.

So the two are not peers, and it is worth stating the rule the measurement
actually supports: **a runtime `match` in a loop body stops vectorization dead; a
loop-invariant branch on a slice length usually does not.** The first is a jump
through a table the vectorizer cannot see past. The second is something LLVM
often unswitches by itself — often, not always. The fused rows are where "not
always" showed up, which is what made hoisting them worth trying.

**At the engine it does not show up at all.** `examples/bench.rs` grew a fused
section for exactly this question, and the same A/B through it — `MulAdd` and
`Sum4` graphs over a shared `Sine`, three interleaved rounds — moves **inside the
±1% noise** at every voice count. That is the correct reading rather than a
disappointment: an arithmetic row is 5–10 ns of a block that spends ~135 ns in
`Sine`'s per-sample `sin()`, so a 20% cut to the cheap part is invisible next to
the expensive one. It is also the difference from the operator hoist, which *did*
move the engine ~20%: there the loop being fixed was 70 ns, not 10.

What the section is for going forward is the question a user actually asks —
whether to reach for `MulAdd` over `a*b` then `+c` at all. Read at the voice
counts where the bench is steady (32 to 512): **fusing buys some 3–4% for
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

