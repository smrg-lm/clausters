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
  x86-64 target. It must be a **full triple** (`x86_64-unknown-linux-gnu:x86-64`):
  faust sets `mcpu` from the string, but with an *empty* triple newer LLVM still
  host-detects the CPU *features* and re-introduces the crash — only a concrete
  triple pins it down. The override is a plain env var read by
  `faust::compiler::host_target`, so only CI opts in.
  - *Provisional note (2026-07-16, revised 2026-07-17):* one SIGILL recurred
    **with the override active**
    (`auto_order::faust_synths_sort_by_their_reserved_buses`, run 29492474371
    on an unrelated docs-only commit; the rerun passed on another runner). A
    single occurrence since the pin — nothing changed, and it has not recurred
    since. Two suspects were investigated and one survives:
    - **Ruled out: host-tuned code in the cached libfaust build itself.**
      Faust's CMake does not pass `-march=native`, the bundled libLLVM is the
      distro's baseline build, and nothing in our Rust uses `target-cpu` or
      feature detection — the shared cache cannot ship instructions a
      restoring runner lacks.
    - **Current suspect: LLVM's own feature autodetection in
      `EngineBuilder::selectTarget()`.** In the vendored Faust source
      (`llvm_dynamic_dsp_aux.cpp`), our target string pins the *triple* and
      *mcpu*, but the target machine's **feature attributes** are left to
      `selectTarget()` — and when the passed triple equals the host process's
      triple, LLVM may still autodetect the virtualized CPU's features (the
      same mechanism already documented above for the empty triple). A
      hypervisor that misreports features would then make the JIT emit
      instructions the VM traps.
    If it recurs: diagnose with a manual workflow that dumps `/proc/cpuinfo`
    flags and loops the test to capture the bad SKU (plus the trapping
    instruction from the core); fix by keeping `selectTarget()` from
    inheriting host features — e.g. a triple spelled differently from the
    process's (`x86_64-pc-linux-gnu` vs `-unknown-`) to break LLVM's
    host-triple comparison, after checking in LLVM 18's source whether a
    non-empty MCPU already inhibits the autodetection (which would rule this
    suspect out too).

## Def persistence: transparent JSON + a non-authoritative bitcode cache

Two layers, decided with the user:

- **B — JSON is the transparent source of truth.** `synthdefs/<name>.json` is the
  `SynthDefSpec` verbatim; `faustdefs/<name>.json` is a `FaustRecord`
  (source/JSON + libfaust version + payload sha256). Reload = recompile from
  there, by the same path as a fresh `/d_recv`/`/d_faust`. The `FaustDef` itself
  is never serialized — its factory is opaque LLVM JIT state.
- **A — the LLVM bitcode cache is non-authoritative.** `faustdefs/<name>.<sha16>.bc`
  is restored only if the libfaust version matches and the file reads well; any
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

## The patcher shows a connection, not a direction

The logical side of a composition — members wired to each other through buses —
is drawn by the `graph` widget. The obvious design is a directed patch: outputs on
one side, inputs on the other, arrows between them. It cannot be built honestly.

**Context:** a GraphDef says only that a member's *control* names a bus
(`{"out": "mix"}`, `{"in": "mix"}`). Which end of that bus writes and which reads
is not in the data — it is the **server's** analysis, which sorts the graph
topologically before it runs. A view could guess the direction from a control's
name ("out" writes, "in" reads), but that is a convention, not a contract: a def
is free to name its controls anything, and the renderer would then draw arrows
that are simply wrong.

**Decision:** the patch is **bipartite** — member boxes on one side, bus nodes on
the other, and a wire per `(member, control) ↔ bus` pair. It shows the
*connection*, which is what the data knows, and leaves the *direction* to the
engine, which is what decides it.

**Consequence:** the view can never lie about signal flow, and the edit stays
well-defined: rewiring means pointing a control at another bus (or at none), one
wire per control, which is exactly the mutation the group and the GraphDef both
accept. A directed rendering could be layered on later — but only from
information the server surfaces, not from a name.

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

**Why.** A binary boundary needs a check that a *already-compiled* peer can make
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
compiler rate and not a JIT. Implementation is deferred until a real declined
op demands it; (b) is not rejected but repositioned as the escalation path
for kernels that outgrow a per-bin map.

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
- **Sketch kept with the decision** (to fix the shape, not to start work):
  the evaluator lands in `clausters-core` as a peer of `builtins`; the def
  carries the program as a static field (it becomes wire surface — version
  it like the other static fields); one `desc_spectral` registry row
  (`SpectralRole::Filter`); the Python client grows a symbolic-expression
  module beside the `pv_*` builders. The acceptance test is the mechanism
  reproducing curated ops (`PV_BrickWall`, `PV_MagAbove`) sample-identically.
  Named risks: mag/phase↔re/im conversion cost per hop (consider exposing
  re/im operands too), the packed layout's real-only dc/nyquist slots, and
  phase of zero-magnitude bins (NaN discipline).
