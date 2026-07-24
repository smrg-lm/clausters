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
