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
  from `/synth_new`, cleaned on `Garbage::Freed`) stay off the audio thread, so
  `Cmd::SetControl` is plain-old-data and the audio thread never compares
  strings. `/def_free` only removes the map entry — live synths keep their `Arc`
  (exact scsynth semantics).
- **UGen wiring allocates nothing.** `UGenSynth::process` builds each UGen's
  inputs in a fixed stack array (`MAX_UGEN_INPUTS`) via `split_at_mut` over the
  wires; the topological order guarantees inputs only read earlier wires.
  Guarded by `assert_no_alloc`.
- **Asynchronous command semantics are deliberate.** A `/server_status` immediately
  after a command may report the old count: commands apply at the start of the
  next block. That is scsynth's model, not a race.
- **A queue the audio thread fills is served by a poll, not by a wake.** The
  network loop parks in `recv_from`, and a worker that finishes something ends
  the park with a zero-length datagram (`osc::wake`) — which is why an NRT job
  that took 2 ms is reported in 2 ms. The audio thread cannot do that: sending
  is I/O. So the one queue it produces into — node lifecycle events — buys its
  promptness with the **tick** instead: while any client holds a
  `/server_notify` registration the loop's timeout is `NOTIFY_INTERVAL`, and
  that number is deliberately `MIN_STREAM_PERIOD` rather than a new one. A
  client that asked to be told about the node tree is entitled to the cadence a
  client that asked to be told about a bus already gets, and picking the same
  number keeps "how often may a subscribed client be served" a single answer.
  Nobody listening, nothing to be prompt about: the tick falls back to
  `GC_INTERVAL`, which is housekeeping and was never a latency.

## `SynthNode`: one trait, symmetric def families

The node tree and FIFOs handle `Box<dyn SynthNode>`, never a concrete synth
type. This was introduced before it was strictly needed so that a second def
family (Faust) could join the tree without touching the engine or the tree.
Consequence: the `synth` (SynthDef/UGen) and `faust` (FaustDef) families are
independent Cargo features that combine freely — new work feature-gates against
`dyn SynthNode` and stays symmetric.

## Buses, execution order, and the network-thread mirror

- **Control buses bypass the command FIFO.** `/bus_set`/`/bus_get` operate directly
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
are re-sent by `Cmd::SetUsage` when a `/node_set` touches a control used as a bus
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
installed wheel could not compile a `FaustDef` at all — `/def_send faust` replied
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
  there, by the same path as a fresh `/def_send`. The `FaustDef` itself
  is never serialized — its factory is opaque LLVM JIT state.
- **A — the LLVM bitcode cache is non-authoritative.** `faustdefs/<name>.<sha16>.bc`
  is restored only if the libfaust version matches and the file reads cleanly; any
  miss recompiles from source and rewrites it. A libfaust upgrade invalidates
  every `.bc` automatically, and a corrupt cache never serves a wrong def
  (named by payload sha, so a stale `.bc` never pairs with a newer record).

## MIDI: standard channel-voice, byte-identical to OSC, in a shared crate

- **A MIDI voice realizes the *same* OSC commands an OSC client would send.**
  `CmdTranslator::translate_midi` maps note-on → `/synth_new` (with `freq`/`amp`
  from named conversions), note-off → `/node_free` or `/node_set gate 0`,
  aftertouch/CC/bend → `/node_set` on live voices. Reusing the OSC path makes a MIDI
  voice byte-identical to its OSC equivalent, and the reserved voice-ID range
  (`MIDI_NODE_ID_BASE`) stays disjoint from client and `/synth_new -1` IDs.
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
`/server_notify` clients) and **never schedules audio** — each client rolls its own
playhead on the shared grid. The `/transport_query.reply` carries `playing` and
`position` beside the three tempo fields it started with.

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
  wheel over the strip zooms the vertical axis and dragging the strip pans it;
  the wheel over the
  body stays horizontal zoom and plain/Shift drag keep selecting/panning time.
  Spatial separation needs no modifier, leaves every existing body
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

**The zoom anchor follows what the axis measures**, because that one window is
shared by every *channel* lane the view stacks — a multichannel file draws N
lanes against a single `y_start`/`y_len`:

- **Frequency (the spectrogram): the cursor's height**, which is the frequency
  under it. A shared window says the same thing in every lane — all of them
  show that band — so anchoring at the cursor is both meaningful and what the
  reader wants.
- **Amplitude (the waveform): the window's own centre**, whatever lane the
  cursor is over, so zero stays at the centre of every lane and the trace grows
  and shrinks inside it. This is the audio-editor convention (a vertical zoom
  is a symmetric change of amplitude scale), and it is the only anchor that
  survives multiple lanes: an anchor taken from the cursor's height is
  meaningless for the *other* lanes, and any off-centre window pushes the wave
  out of its lane and clips it — a wheel near the top of channel 2 used to
  shove every channel's wave against the bottom of its lane. Panning the strip
  still reaches an off-centre region when one is wanted.

## Dragging a clip to the edge scrolls the view

A clip drag mapped the cursor through the group window captured at the *press*,
and nothing moved that window while the drag was in flight. So a clip could
never travel further than one visible window's worth per gesture, and holding
the cursor against the lane's edge did nothing at all — a standing pointer
sends no events. Zoomed in far enough to place something precisely, that was a
sliver: the only way to move a clip across the piece was to zoom out (losing
the precision), drag, and zoom back in.

The fix is the audio-editor convention: **held against the edge, the drag pulls
the view along**. Two decisions inside it are worth recording.

**The cursor maps through the current window, not the press-time one.** That is
the whole mechanism — panning the view under a standing cursor moves the sample
beneath it, and the clip follows. The press snapshot stays for what it was for
(`press_sample` is a *timeline* coordinate, so it is unaffected by the pan, and
a clamped edge still cannot drift).

**The scroll rate is a fraction of the visible window per second, not a pixel
rate.** Zoomed in, a fixed pixel rate would crawl — the window is a sliver of
the piece, so a pixel is worth almost nothing; zoomed out it would fly off the
composition. A fraction of the window behaves the same at every zoom, which is
what a gesture has to do. The pan clamps against the group's span, and that
span already grows as the dragged clip extends the content, so the view keeps
making room instead of stopping at the composition's old end.

The tick is the front's frame timer — the one already driving meters, scopes
and the playhead — so both fronts get the behaviour from one shared
`Gestures::tick`, and a window that is otherwise still runs the timer only
while such a drag is in flight.

## The looping playhead: an explicit loop region, not a flag over the selection

The swept playhead is one subtraction in `host::frame`: a view with
`playhead_at >= 0` draws at `sample_clock - playhead_at`. That is the whole
reason following a transport costs **no messages** — the line is redrawn from
Rust every frame at the display's rate, never sent by a client — and it is
exactly right for a straight pass. It had no notion of a loop, so anything
repeating a region (an editor's "play selection", a looping clip) could not be
followed at all: the line ran past the region and off the view. The two
workarounds a client could reach for are both worse than the gap — re-anchoring
per pass puts a message back in the loop and drifts against the audio, and
sending a position per frame gives up the property the anchor exists for.

Two shapes were weighed. The one chosen is an **explicit loop region** —
`playhead_loop_start` / `playhead_loop_len`, with the swept position becoming
`start + ((sample_clock - playhead_at - start) mod len)` and `len <= 0` keeping
the straight sweep. The alternative was a `playhead_loop` **flag** wrapping
within the existing selection, which reads better in the one case that motivated
this and costs a single boolean. The explicit region won because it does not tie
playback to a selection a gesture can change under it: a drag on the view while
a loop plays would silently move what is heard, and a client that loops
something *other* than the selection (a clip, a timeline range) would have no
way to say so. The cost is one extra prop.

It lives on the shared editor chrome, so `waveform`, `spectrogram`, `track`,
`clip` and `pianoroll` get it at once, and it is **group-wide** like the anchor
— linked views must wrap at the same place, or one file's waveform and
spectrogram would draw the line in different spots. The navigation group is
where it is *kept*, and every view reads it from there; a widget's own props
only seed the group when it is first built. The `score` carries the
same pair in **ms**, its own unit, since its cursor rides an engraved timemap
rather than a sample axis. The static `playhead` keeps its meaning, and a client
still sends **one** message per transport state change.

One consequence worth stating, because it is the easy mistake: a looped pass
anchors at `clock - start`, not `clock`. The reading begins at `start`, so
timeline position 0 sits that far before the clock value the client has;
anchoring there makes the swept position `start` on the first frame and the wrap
returns it there on every lap.

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
document, so every client shares it. The Python arrangement model (`Aggregate`/
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
  note to a parameter — which is the granularity a type list cannot reach.

**Consequence.** The layer is pure and transport-agnostic: no DSP, no protocol, no
GUI — the piece a future client factors into the shared core. It carries the
temptations too: a `Vector` is data and sounds only through the *instrument* named
to play it, and a logical aggregate emits the bus-wired configuration the server
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
evaluates it client-side (the `envshape`/bus_tap-reader precedent). Everything else is
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
16 MiB, advertised in `/server_query.reply` for clients to size bulk requests
from) rather than a constant — no limit is hard-wired, per the rule that the
project must stay usable as a desktop/mobile application without arbitrary
ceilings. Timing is unaffected by the switch: it rides on bundle timetags and
`/sched_at`, never on arrival time. Replies became transport-aware in the same
move (a `/bus_tapStream` window may fill a whole frame for a stream client; UDP
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
at v14 while the embed/IPC ABI is at v5). A monotonic integer per
boundary is exactly the scsynth plugin-ABI lesson: every binary seam is
versioned and verified where it is crossed. SemVer is left to govern the
*package* — what `cargo`/`pip` resolves — where it belongs.

**Where the package's number is written** is a separate question, and it had the
same shape as the drift this section exists to prevent: one number, ten files,
one checked pair. The crates inherit `[workspace.package].version` now, the five
manifests that cannot inherit anything are written by `scripts/set-version.sh`,
and `tests/versions.rs` fails when any of them disagrees — including when a
crate writes its own number instead of inheriting, which is how the next one
would drift. The two ABI counters are deliberately *not* in that machinery: they
are not the package's number and do not move with it.

The one **linkage** rule keeps them from drifting into contradiction: a release
that bumps either counter must also bump SemVer's breaking tier (the minor while
the major is `0`, per standard pre-1.0 SemVer where the minor acts as the major;
the major once at `1.0`). The reverse is not required — a minor can ship purely
additive source-API work without touching either counter.

That a counter measures **distance, not history** follows from the same
reasoning, and decides a case that comes up during development: when a boundary
changes twice before the first change has shipped, the second amends the first
bump rather than adding another. The number exists for a compiled peer asking
"can I attach to this?", and that peer only ever knew the last published value;
counting the intermediate states would tell it about releases that never
existed, and burn a version on each. It is the same argument that keeps the
counter off SemVer — the seam is described by where it *is*, not by how it got
there.

The mechanical release rules live in `CLAUDE.md` ("Versioning"); this entry is
the *why*.

## Client defaults for the wheel: sample clock (live only) and an enveloped default synth

Two out-of-the-box defaults, chosen for the common case of a **local** session:

- **The built-in `default` synth carries a gated envelope.** It was a bare
  `Sine(freq) * amp`, which clicked at note-on (level jumps from 0) and at
  note-off (the node is freed mid-cycle). It is now `Sine * EnvGen * amp` with
  a gated ASR — equal-power sine ramps (0.01 s attack, 0.3 s release),
  `doneAction = FREE_SELF` — the same shape as SuperCollider's `\default`. Because
  a click-free note-off *requires* a release ramp, and a direct `/node_free` cuts
  the ramp, the player must release this instrument by **closing its gate**. The
  global event default stays `has_gate = False` (a gate-less custom def is still
  freed directly, so it can never leak); the player special-cases `instrument ==
  "default"` to gate-release. So the safety of the free-by-default choice is kept
  while the built-in sounds clean.

- **`Session.live()` and `Session.embed()` anchor to the server's sample clock
  by default** (config `[client].clock`, default `"sample"`), rather than
  wall-clock OSC timetags. For a local session the sample clock is strictly
  better — drift-free and sample-exact — and making it the default also
  exercises the `/sched_at` path on every run. It falls back to wall-clock
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
the server's own `/synth_new -1` and MIDI-voice counters wrapped `i32` in release
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
  events drain, a client releases its ids on `/node_end` (never at `/node_free`-send
  time, which could re-hand an id whose node is still alive). The registry is
  passive (events are fed in; it never calls out), which keeps it identical
  across bindings and wasm-compatible. The corollary: an engine rejection
  produces no `/node_end`, so the server broadcasts `/fail` **with the id
  appended** — otherwise the client's in-flight id would be lost, violating
  invariant three.
- **Every capacity is bounded at boot, including node ids**, even though ids
  are "dynamic": concurrent nodes are bounded by the node table anyway, so
  the id space only needs table capacity plus in-flight margin
  (`NodeIdPartition::from_max_nodes` — client 4×, auto 2×, MIDI 2× — replacing
  the magic 2M/3M bases). A bounded registry turns a leak into a visible
  fail-fast error, preallocates once (no growth, no `i32` overflow), and lets
  clients size themselves from `/server_query`'s `max_nodes` by the shared
  formula — by query, not convention. The sanctioned exception is NRT/score:
  no real-time bound and no live `/node_end` stream, so a score client's node-id
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
registries recycle off `/node_end` — a dropped end event is a client id that
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

## `channels` means something different to each verb, and that is the design

`play`, `plot` and `render` share one expression coercion, which invites making
them share one reading of "how many channels". They must not. `play` and `plot`
are **conveniences** — the interactive front door — and may infer: `plot` sizes
its render from the expression, so a stereo pair shows two lanes unasked.
`render` is part of the **NRT interface**, where `channels` is how many outputs
the offline server *has* — a fact about the machine being configured, not a
property of the graph handed to it.

**Decision:** `render` never derives `channels` from what it renders. What it
does instead is **check**: an expression the coercion laid on more buses than
the render has outputs is writing its surplus onto internal buses that reach no
file, so it raises and names the fix. Only the buses the coercion itself
assigned are checked — an explicit `out(8, sig)` is the caller's own routing and
is left alone.

**Why not derive.** Deriving would make `render` guess the shape of its own
output from its input, which is exactly the coupling an NRT interface should not
have: the same expression rendered for a stereo file and for an 8-channel one is
the *same expression*, and only the render differs. Guessing also hides the real
mistake — asking for four channels of a two-channel render is a mistake worth a
message, not a number to silently adjust.

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
  natively would change the native sample stream and change nothing users can
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
  optimization, not a redesign — and the optimization has since been priced,
  which is what settles it. On a page holding forty canvases, each drawing a
  control-rate meter and a control-rate scope, one `/bus_stream` frame is 824
  bytes and taking it in costs about 29 microseconds of the main thread per 33
  ms frame: **a tenth of a percent of the frame**, some 25 kB/s, with the
  page's frame rate and event-loop lag unable to tell a streaming phase from
  the same page with the subscription cancelled. The cost is per call rather
  than per byte (sixty-four canvases: 1304 bytes, the same 28 microseconds).
  So the zero-message path would save a tenth of a percent of a frame, and
  cost isolation headers an embedded component cannot ask for plus a second
  backend for one seam; the MessagePort carrier is not a compromise at
  this scale, it is the right size. The measurement is reproducible —
  `clients/web/tools/profile-bus-stream.sh` — and reopening the question means
  a profile that shows the messages costing something, not an argument.
- **The one relaxation vs. the native RT rules**: OSC→Cmd translation
  allocates on the worklet (audio) thread. wasm malloc is a bump over linear
  memory — no page faults, no locks, no priority inversion — and the DSP
  itself stays allocation-free, so the native no-alloc discipline keeps its
  value without being extended to a heap that cannot misbehave the same way.
  That holds only while the bump stays inside the memory the module already
  has: a bump past the end is a `memory.grow`, which is a host call that may
  copy the whole heap and **detaches** the `ArrayBuffer` and every JS view over
  it. So the engine reserves its linear memory at link time — 16 MB, against a
  boot that lands at 4.3 MB and a rustc default of 1.5 MB, with a 256 MB
  ceiling well under the ~350 MB where iOS Safari kills a tab
  (`--initial-memory` / `--max-memory` in `crates/clausters-web/build.rs`,
  asserted by `clients/web/tests/memory.test.ts`). The claim above is about a
  heap that does not grow; the reservation is what makes it one.
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
  `fetch` of `/smoke-verdict-…`, read from the HTTP server's access log. Every
  web acceptance since is written that way, and one runner walks them all.

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
JS (`clients/gui/web/bundle.js`). The replay is bracketed by two `/server_sync`s: the
engine serves strictly in order, so the trailing `/server_sync.reply` is the page's
"bundle is up" signal — no per-command acking.

- **The one addition is `bundle.json`**, a manifest at the bundle's root
  naming the def files, because HTTP cannot list a directory the way the
  native store lists it. It is generated (`web/bundle-manifest.py`), never
  hand-maintained, and also carries the one genuinely browser-side mapping:
  which audio URL feeds which server buffer (fetch + `decodeAudioData` →
  the engine's `buffer_load` — the browser's `/buffer_allocRead`, decoded by the host
  page because the wasm engine has no sndfile).
- **The in-page leg is one more `ServerLink` variant, not a new protocol.**
  `ServerLink::Page` hands outbound packets to a page-registered callback and
  takes replies through `GuiBridge.server_reply`; the host's streamed data
  paths (`/bus_stream`, `/bus_tapStream`, `/buffer_getRange`, `/clock_query`) run over it
  unchanged — the acceptance smoke watches the meter's `/bus_stream.reply` stream arrive
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
  coexist; per-boot `/server_sync` ids keep concurrent bundle boots from mistaking
  each other's `/server_sync.reply`.
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
voice the key *release* can later reach — `/synth_new` on press, `/node_set <id> gate
0` on release. The server-assigned id form (`/synth_new … -1`) was rejected for
exactly that reason: the host would never learn the id it needs to gate, and
adding a reply round-trip for it would put a network latency inside a played
note. So the host sends **explicit positive ids**, which the server accepts
from any client, and allocates them from a dedicated window:

- **Base `0x1000_0000`, wrapping over a `1 << 16` span.** Far above the Python
  client's counter (`1000..`) and the server's own auto-assign range
  (`1000 + 4·max_nodes ..`), so a host voice can never collide with a node a
  script created — the three allocators partition the id space by
  construction, with no coordination protocol.
- **No `/node_end` tracking.** A voice def is required to free itself on release
  (`FREE_SELF` on the gate envelope), so the host's bookkeeping is just the
  live `(pitch, node)` pairs per widget: the release (or a glissando, a
  re-press, a widget free/redefine — all of which gate the old voice) removes
  the entry. A 65536-id window wraps long before any voice from the previous
  lap can still be sounding.

The same reasoning will apply to any future host-managed spawner (an XY pad
playing voices, a drum grid): reuse this window, not a new one per widget.

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
The **heavy views keep a real scissor** (`host/frame/mod.rs::apply_scissor`) —
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
composition holds a logical aggregate, the way it embeds the piano-roll. The already
shipped level-1 edit-back (P3d) is reclassified into phase B, ahead of its
representation work, and its `Aggregate → patch` mapping is lifted out of `editor.py`
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
require the server to report each def's internal UGen graph over `/def_query` —
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
same callables the `/ugen_query` contrast test pins to the server registry — no new
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
of the renderer. It gives three things the SVG walk cannot: no parsing round-trip,
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
producer — which unblocks the editor *now* and additionally brings the in-process
relayout and edit-back the previous entry describes. (3) Mutate the MEI in
Python and re-engrave: `getMEI()` out, edit the XML (we own the `xml:id`s),
`loadData` back. It needs no editor toolkit and no C++ at all, at the price of a
full relayout per edit and of implementing the semantics ourselves.

Route 3's price is smaller than it sounds and should be measured before it is
assumed away: a full engrave of a six-bar page — load, lay out, render, walk the
SVG into the display list — is **~17 ms**, from MEI or from Plaine & Easie alike.
That is inside a frame budget for a page of this size, so "incremental relayout"
is worth nothing yet; it starts to matter at a page count a rehearsal score reaches,
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
script writes `Synth("beep", {"freq": 440})`, and the client's
`NodeIdAllocator` names the node). The GUI's id handling grew crudely in the
opposite direction: two disjoint monotonic counters (the host client from 1000,
the multitrack editor from 10 000, partitioned only by convention), neither
recycling, and examples that hand-pick ids (`knob(10, …)`) purely so they can
match the `/gui_event` back. Two questions had to be settled: **where** ids are
allocated, and **how** a script refers to a widget without naming an integer.

**Allocation stays client-side, mirroring `NodeIdAllocator`.** The tempting
alternative — the host assigns ids the way `/synth_new … -1` lets the audio server
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
3, and is worth **1.19–1.23× on the all-signal shapes, 1.03–1.09× on the
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

## The TypeScript graph composes by method, and the wire is what parity means

The Python client builds a UGen graph with operators — `sine(freq) * amp`,
`1 - env`, `sig % 2` — because Python lets a class define them. TypeScript does
not: there is no operator overloading, and no proposal close enough to plan
against. So the web client's graph composes by **method**: `sine(freq).mul(amp)`,
and every other operator or math function is a method carrying the same operator
**name** the wire uses (`.midicps()`, `.max(x)`, `.distort()`). Where the
constant is on the left, which a method cannot express, there are free functions
(`sub(1, sig)`) and, on the Faust side, the explicit `rsub`/`rdiv`.

Two consequences worth stating, because they look like drift and are not.

**A method name is not always the wire name — and the method's is
SuperCollider's, not the language's.** The selector *is* what crosses (`asint`,
`hypotapx`, `recip`, `lshift`), and it stays as the def format spells it: those
names are in stored SynthDefs, in `docs/schemas.md` and in the frozen parity
vectors, so renaming one is a format change and not a spelling preference. (This
paragraph said "renaming them would be a format change for a cosmetic gain" and
used `as_int`/`hypot_apx` as its examples, which is the sentence three
underscores hid behind: the wire's own convention is lowercase and **joined**,
those three broke it, and a convention with three exceptions is not one. They
were renamed on 2026-08-24, at the price the paragraph names and no more —
`from_name` resolves the underscored spellings for good, so a def stored before
the rename loads and nothing emits them again.) What a
*user* types is a different question, and it was answered wrong at first — the
web client took the idiomatic TypeScript spelling (`asInt()`, `hypotApx()`)
while the Python client took a third one (`abso`, `as_int`, `recip`), so the two
clients disagreed with each other **and** with the language they are a port of.

The rule now: **a client's name is SuperCollider's, all lowercase, joined** —
`asinteger`, `asfloat`, `reciprocal`, `leftshift`, `rightshift`, `hypotapx`,
`abs`. Lowercasing sclang's camelCase is not new; it is what the operator table
already did for `bitand`/`bitor`/`bitxor`, now applied to the six that had
drifted. So a reader who knows SuperCollider types the same call in both
clients, and neither client's idiom overrides the vocabulary. The mapping from
that name to the wire selector lives in the class, in one line each.

**Parity is asserted on the emitted spec, not on the source.** The two clients
cannot be compared expression for expression, so `clients/web/tests/def-parity.
test.ts` builds each reference graph independently in TypeScript and asserts the
`SynthDefSpec`/signal-tree/`GraphDefSpec` JSON is identical to what the Python
builders emit for the same graph (frozen by `gen-def-vectors.py`). That is the
right seam anyway: the def format is the shared contract, and two clients
agreeing on it is exactly what "numerically equivalent by construction" is for.
The vectors cover what the source alone cannot show — control dedup and
first-seen order, the topological walk, the fused `Sum4`/`Sum3` mix fold, the
`ir`/`tr` control types and lags, generic op naming, and the `sr()` clamp inside
a Faust recursion.

The same reasoning settles a smaller one: a JS number is a double, so the client
cannot infer int-vs-float from a value the way Python does. The `Server` therefore
tags **by position** — node ids, bus indices and add actions as int32, control
values as float32 — and only the free-form `sendMsg`/`genBuffer` guess (an
integral number is an int32), with an explicit `[tag, value]` pair as the escape
hatch where the guess is wrong (`/buffer_gen`'s flag word).

## A GuiDef from TypeScript: the options are the language's, the document is the wire's

The Python GuiDef builders take keyword arguments spelled exactly like the props
they emit (`text_size=3.0`, `base_bucket=512`, `sel_start=…`), because in Python
those are the same identifier. TypeScript's are not, and a client whose surface
reads as snake_case would be the odd one in its own language — the def builders
already took the other road (`control("cutoff", 800.0, { lag: 0.1, lagDown: 0.5 })`).
So the web GuiDef builders take **camelCase options** and write the host's
**snake_case props**: `waveform({ baseBucket: 512, selStart: 0.0 })` emits
`{"base_bucket": 512, "sel_start": 0.0}`. A prop this client does not name yet —
a newer host's — passes straight through under its wire name, the way Python's
`**props` does, so the vocabulary can grow without a client release.

**Parity is asserted on the emitted document, not on the source**, for the same
reason the def specs are (`clients/web/tests/gui-parity.test.ts` against vectors
frozen by `gen-gui-vectors.py`) — and there is a second reason here. A JSON
number from JavaScript carries no int/float distinction: `JSON.stringify(480.0)`
is `480`, where Python writes `480.0`. That looked like a parity problem and is
not, because the host reads every continuous prop through `as_f64` and every id,
index and count as an integer — an integer literal is a perfectly good float on
the way in. What the vectors compare is therefore the **parsed** document, which
is exactly the level at which the two clients have to agree. The int/float
distinction stays load-bearing where it actually rides OSC (`/gui_set`'s values,
tagged by the same inference rule the audio client uses).

The same milestone settles a smaller one: the browser client has **no pump**.
`GuiHost` subscribes to its connection once and routes `/gui_event`/`/gui_closed`
to the handles as they arrive (`win.widget("cutoff").onEvent(fn)`), with `query`
a promise — where the Python client drains the host from the script's own loop
because it must not block the clock thread. Same discipline, but in a language
where it is the only shape available.

## The web clock: the routine driver stays on the page, only the wake-up moves off it, and the server anchors the timebase

The Python client runs a `TempoClock`'s real-time drive on a **background
thread**, which shares objects with the rest of the program: the routine, the
`Server`, the session. A browser Worker shares nothing but structured-cloned
messages, and a routine is a closure over the script's own objects — it cannot
cross. So there is no direct port of that thread, and the coroutine driver stays
on the page's thread, which is what `clients/python/PLAN.md` already asks for ("the
coroutine driver stays in the language").

What *does* port is the property the background thread has: a wake-up the rest
of the program cannot starve. On the page, `setTimeout` is clamped once nested
and throttled to about a second in a background tab — longer than any usable
scheduling headroom, so a sequence would stutter the moment the user changed
tabs. A worker whose only job is `setTimeout` and `postMessage` is not throttled
that way. Hence the split: **queue and routines on the page, the wake-up behind
a `Ticker`** (a shared tick worker in the browser, `setTimeout` where there is
no `Worker`). The exactness never depended on the wake-up anyway — it rides on
the bundle's timetag, and the wake only has to arrive within `Server.latency`.

The same seam pays a second time: with `Ticker` and `Timebase` both injectable,
`node --test` drives the *real* driver by hand, so "a late wake-up does not
shift the music" is a deterministic assertion rather than a timing-dependent
one. That is also why there is no NRT/score drive in this client: a second drive
would be a second thing to keep correct, and the seams already give what it was
for (`Timeline.fromPattern` bounces a pattern by advancing the ordinary clock as
fast as the loop can go).

**Anchoring is the Server's job, not the clock's.** The Python client has
`clock.lock_to(server)`, which puts the clock in conversation with a server and
contradicts that client's own rule — the one its C5 corrected — that the clock
must not talk to the server. The web client inverts the relation:
`server.sampleTimebase()` returns a timebase, and the clock merely reads it. The
Server is the right owner because it is the object that knows the carrier, and
the two carriers need different work:

- **in-page** — the engine runs in this page's `AudioContext`, so one worklet
  round trip pairs the engine's counter with the context's `currentFrame` at the
  same instant. Their difference is a fixed integer (the engine advances one
  quantum per render quantum of that context), so from then on the counter is
  `currentTime` read synchronously: exact, and drift is not a thing between a
  clock and itself.
- **over a socket** — `/clock_query` round trips feed the core's `SampleClockModel`,
  which regresses local time against the server's counter. The warmup must
  **spread its anchors over time**: five back-to-back round trips all land
  inside a couple of milliseconds, and a regression over that span is noise, not
  a rate. (Measured: it read a slope 2× off, which showed up as a sample clock
  running at half speed.)

A server that does not answer leaves the clock on wall-clock time rather than
failing, so a page whose master is unreachable keeps working.

## The document places, the host draws: no window management in the browser

On the desktop `clausters-gui` opens one window per `window`-rooted GuiDef and
the system's window manager places, sizes and stacks them. In a browser tab the
drawing surface is a `<canvas>` in an HTML document, and the document does the
placing — CSS, the order of the markup, the flow of the page. W4 makes that one
substitution and nothing else: the browser host went from `window`/`render`/
`current_def`, all singular, to a map keyed by def id, and the model is the
native front's, ported rather than invented.

Two things had to change direction with it, and both are the same point. The
**element supplies the canvas** (`GuiBridge::attach(def_id, canvas)`, winit's
`with_canvas`) instead of the page hunting for whichever canvas winit appended
to `<body>`: that is the correct ownership, and it is the only way N of them can
exist. And the **size comes from the element** (a `ResizeObserver` times
`devicePixelRatio`, reported in device pixels), so the host never reads the DOM;
it is told "this def draws into this canvas, at this size, and right now it is
(not) visible", and knows nothing else about HTML.

What is deliberately absent is the *management* layer. Components are placed in
the markup by whoever writes the page, in the order they want; nothing opens,
moves or stacks them. Mounting happens in `connectedCallback`, so an element
inserted later works, and unmounting in `disconnectedCallback` — see below for
what that gives back.

One consequence is worth writing down because it was found the hard way: winit
focuses a canvas it creates, and a browser scrolls a freshly focused element
into view. In a document with several components the last one mounted yanked the
reader to the bottom of the page. Canvases are created with `with_active(false)`;
a click focuses them, which is when keyboard input is wanted anyway.

**Not rendering what is not seen.** A document can hold fifty canvases with
three in the viewport. The browser already skips compositing what is off screen,
but that does not stop *our* host from computing the frame — the spectrum
analysis, the scope advance, the FFT — nor, more expensively, from keeping its
`/bus_stream` and `/bus_tapStream` subscriptions alive, which is server CPU and wire
traffic for something nobody is looking at. So each component carries an
`IntersectionObserver`, and every per-frame and per-packet cost is derived from
the **visible** set (`host::live::demand`, platform-agnostic and natively
tested). The same waste exists on the desktop behind an occluded window; only
the browser front acts on it so far.

## An unmounted component gives back what it took, not what the page shares

The mount is two phases because the host does not need audio and the engine
does. The unmount is deliberately **one**: an element removed from the DOM is
removed whole, so its window and widgets (`/gui_free`), the nodes its boot
instantiated (`/node_free`), its canvas (`detach`, which also drops it from the
frame tick and the `/bus_stream` set) and every id it drew from the page's
pools go together. What it does *not* touch is anything shared: the
`AudioContext`, the host, and — the one that looks like a leak and is not — the
def payloads and the sample buffers. Both are keyed by URL and identical for
every instance of a bundle, so freeing them would be freeing a sibling's; they
stay, and a component mounted again finds them loaded.

Which settles the other asymmetry: a re-connected element **mounts afresh**
rather than resuming. The resolver runs again over a new allocation, so the
attributes and the preset are read again — there is no half-alive state to
resume, which is precisely what lets the teardown be complete.

Two things found the hard way. A custom element's constructor **may not touch
its attributes**: `defineComponent` filled in `src` there, which works when the
parser upgrades an element and throws `NotSupportedError` on
`document.createElement` — that is, on exactly the path a page adding
components from script takes. The default bundle is carried as a field and
reflected into the attribute on connect. And a component's canvas must follow
its element's box on **every** `ResizeObserver` firing, not only at mount: an
element appended and mounted inside one task can be measured before the browser
has laid it out, and the first firing is what corrects the 1x1 canvas that
leaves.

The way back — `/gui_closed`, a window closed by the host rather than by the
page — reaches the element that mounted the def, which unmounts and emits
`clausters-closed`. The wasm host in the tab never sends one (nothing there
closes a canvas but the page); a native host over a socket does, and its event
stream joins the page's through `ClaustersGui.deliver`, so an element hears a
window closing wherever the window was.

## Holes live only in the GuiDef record, so a def is sent once

A bundle mounted twice on one page must not collide: two instances need two
node ids, two buses, two blocks of widget ids. The persisted GuiDef record is
therefore a **template** with two kinds of placeholder — `@symbol` for an id the
caller allocates, `$param` for a value the tag supplies — and the resolver fills
them at mount.

The load-bearing decision is *where placeholders are allowed*: *only* in the
GuiDef record and its `boot` list, never in a def payload. That is what makes a
second instance cheap — the `/def_send synth` and `/def_send graph` payloads are byte-identical
between instances, so they are sent to the server once and shared. It also
forces one authoring rule, which is the right rule anyway:

> A bus, a node or a buffer reaches a def **as a control**, never as a baked
> constant.

`piano_voice` used to do `out_ctl(0.0, env)` — the bus number compiled into the
def — which is exactly why that bundle could not be mounted twice. Written as
`out_ctl(control("env_bus"), env)` it can, and the mount passes each instance
the bus it was allocated. The writer enforces the rule (`check_def_payload`)
rather than documenting it, so the wrong form fails at write time.

Widget ids are deliberately **not** symbols. The template numbers its widgets
locally and the resolver offsets them by an allocated base: twelve widgets would
otherwise mean twelve placeholders for no gain. So `"widgets"` in the manifest
is the *width of the id block* — the highest local id, the root's included —
not a count, because a template may number sparsely (`1`, `10`, `20`) and what
is allocated is a contiguous run wide enough for the numbering. A bundle written
before the contract declares no width at all, and the block is measured from the
template instead, which is what lets yesterday's bundles mount twice.

The resolver is **pure**: the caller allocates between `requirements` and
`resolve`, so nothing about a page's or a host's id spaces leaks into the shared
core, nothing is added to the `/gui_*` protocol, and no state is added to the
host. `validate` is the same pass over the declared defaults, run by the writers
— so an unmountable bundle is unwritable.

## The run-time entry excludes the authoring builders

Running a component is the browser equivalent of `clausters-gui --standalone`:
the host is the server's client, and there is no scripting client in between.
The builders ran earlier, in the authoring script; what the page fetches is
data. So the package has **two entry points**, and the difference is what a page
is made to download.

`dist/runtime.js` carries the engine, the GUI host, the OSC codec, the mount and
the element. `dist/index.js` — the whole facade — carries those plus the def
builders (`defs/`), the GuiDef builders (`gui/guidef.ts`) and the sequencing
layer (`seq/`). A page that embeds an instrument in an article needs the first;
a page that sequences, responds or edits live imports the second on top. Both
target the same element.

Making the split real needed one move in the sources: the page's own host — the
singleton, its canvases, the in-page carrier — came out of `gui/host.ts` into
`gui/page.ts`, because `GuiHost` is the transport-agnostic *client* object (it
also drives a native `--ws` host) and pulls the GuiDef builders with it, while
the run time needs only the host itself.

The exclusion is **asserted, not hoped for**: `tests/runtime-graph.test.ts`
walks the emitted module graph of `dist/runtime.js` and fails if it ever reaches
any of the three, with the facade's own graph as the negative control so the
walker cannot pass by finding nothing. An import added anywhere along the chain
— the entry, the element, the mount, the page host — shows up there.

## The web client's API reference: TypeDoc, and a second TypeScript to run it

The repository's documentation rule is that an API reference is *generated* from
the source's own doc comments — the rustdoc for the crates, pydoc-markdown for
the Python client. The web client's book needed the same for TypeScript, under
the toolchain posture the package was built with: minimal, user-space, no
bundler, one dependency.

**TypeDoc, installed as a tool rather than as a dependency.** It is the only
generator that reads the language's own doc format, and with the markdown plugin
it emits the pages an mdBook can host. Its weight — about forty megabytes,
mostly the TypeScript it parses with — does nothing at run time and nothing at
build time, so it goes where pydoc-markdown goes: a **user-space global
install**, never in the package's `node_modules` and never in `package.json`.
Read the Docs installs it the same way, into a directory of its own.

That placement also settles a version conflict rather than fighting it. The
package compiles with **TypeScript 7**, the native compiler, a single dependency
with no transitive ones; TypeDoc is built against the **5.x** compiler API and
resolves its own copy from beside itself. Two TypeScripts exist on the machine
and never meet: one compiles, one reads. Pinning the package back to 5.x to
share it would have been the wrong trade — the emit is the product, the docs
build is a tool.

**The consequence for the sources.** TypeDoc reads TSDoc (`/** */`). The client
had been written with Rust-style `///` doc comments, which are invisible to
*every* TypeScript tool — an editor shows nothing on hover, a generator sees an
undocumented symbol. The tree was converted wholesale; new code writes `/** */`.

**Warnings are errors** (`treatWarningsAsErrors`), the rustdoc posture: a
comment referring to a symbol that moved, was renamed or became private fails
the build instead of producing a dangling page. Getting there also widened the
public surface a little — the types that appeared in signatures without being
exported (`ParamSpec`, `SpecInput`, `TimedMessage`, `Props`, …) are exported
now, which is the honest reading of a warning that says a documented signature
mentions something the reader cannot name.

## Publishing is a step, and a checker stands in for rehearsing it

The npm package is built, tested and installable from a checkout long before it
is published, and the publication itself is deliberately a later step. The risk
that creates is that publishing is the one operation nobody rehearses: its
mistakes — a tarball carrying the emitted modules but not the wasm bundles
`build.sh` stages beside them, a `version` that drifted from the crate's, an
`exports` entry pointing at a file the `files` list leaves out — are invisible
here and only surface in somebody else's install.

So the check runs now, twice: `tools/check-package.mjs` is what `prepublishOnly`
runs, and `tests/package.test.ts` runs it in the ordinary suite along with a
read of what `npm pack --dry-run` would actually ship. The version rule it
enforces is the repository's — package, crate and wheel are one release, one
SemVer — while the binary ABI counters stay separate, as they are everywhere
else.

## Signal logic moves into the core when a second process draws it

The GUI host was, for a long time, the only thing that read the server's live
data: a `meter` names a control bus, a `scope` an audio tap, and the host
subscribes, aligns, transforms and paints on its own. So the algorithms behind
those views lived inside the host crate — the oscilloscope's trigger alignment
in `host/oscil.rs`, the spectrum's decibel scaling inside `host/spectrum.rs` —
and that was the right place while it lasted. Both are pure and both were
already shared by the host's two fronts, native and browser, so nothing about
wasm forced a move: the browser front computes them fine.

What forced it is the web client's data paths (W10), which let a **script** read
the same bus, the same tap and the same buffer and draw its own canvas. That
makes a page a second drawer of the same trace, and a second drawer is exactly
the condition the shared core exists for. The alternative was worse in two ways:
a re-implementation of the trigger in TypeScript would drift from the host's
silently (the failure mode is not a crash but two subtly different pictures of
one signal), and exporting the host's internals from its own wasm bundle would
make a script that draws its own canvas download the whole GPU host — 5.3 MB
against the core's 438 KB — and turn the host crate into a host *and* a function
library.

So `oscil` and `spectrum` are now `clausters-core` modules, the host consumes
them from there, and the browser reaches them through `clausters-core-web`
alongside the peak pyramid and the stereo-field measurements. The precedent was
already in the file next door: the spectrum's FFT and Hann window had made the
same trip earlier, for the same reason (the spectrogram and the spectrum view
had to agree bin for bin).

The rule this settles, stated as a test rather than a taste: **signal logic
belongs to the host only while the host is the only one computing it.** The
count of consumers is what decides placement — not whether the code is "display
math", and not which targets it compiles to. What stays in the host is what has
no second consumer by nature: geometry, hit-testing, chrome, and the state that
is a *look* rather than a measurement — a spectrum's averaging across frames, a
scope's history depth. Those are decisions about how a picture should feel, and
two views are entitled to disagree about them.

**The test was wrong, and the move it justified was right** *(2026-08-20)*. The
second drawer is gone: a client draws nothing at all now, and the trigger, the
decibel curve and the pixel row left the web client with the doors onto them
(`clients/web/PLAN.md`, W26). By the rule above, `oscil` and `spectrum` would go
back into the host — and they are staying where they are, which says the count
of consumers was never the reason. They are functions of a **signal**: a
headless client measuring one, a test asserting one, a future analysis path
would each want the same numbers, and the core is where a thing that has an
answer independent of any screen belongs. What the host keeps is what only a
renderer does. The rule as it now stands is the placement rule in
`architecture.md`: **everything drawn is drawn by the host**, and the arithmetic
of a picture goes into the host or into the core for the host to read — never
into a client, where a second implementation would be reachable from one
language only.

## A bus is a bus: the sample ring is the server's, never an API's

The live data views grew one at a time, and each took whatever number was
nearest the implementation. A `meter` took a control bus, because control buses
sit in the shared segment where anyone can read them. A `scope` took either a
control bus *or* a "tap" — an index into the eight sample rings the segment
carries — because that is the only way an audio bus's samples leave the audio
thread. The goniometer and the spectroscope took only taps, in pairs and runs.

So the surface said there were two kinds of signal, `bus` and `tap`, when the
server has exactly one kind of thing at two rates. Worse, the "tap" number was
not even a bus at a different rate: it is *which of eight canaletas is currently
carrying that bus*, which the caller had to allocate, route (`/bus_tap tapIndex
bus`), thread into the widget, and release. Every layer above the segment —
the wire, the host, both clients, the examples, the books — repeated that
bookkeeping.

**Every view now names `bus`, `rate` and, where it reads several, `channels`
adjacent buses from there.** This is SuperCollider's model (`Stethoscope` takes
an `index`, a `numChannels` and a `rate`, and its `rate` defaults to `\audio`
with a keystroke to flip it), and it makes bus 0 — the first hardware output —
the default a bare `scope()` or `meter()` shows.

The ring does not disappear; it stops being anyone's business but the server's.
`/bus_tap bus watch` replaces `/bus_tap tapIndex bus`: the client asks for a bus, the
**server** picks the ring and publishes the choice in a per-bus directory in the
segment, so every reader resolves bus → samples by lookup. Watches are counted,
so two views of one bus share one ring and the last one to stop frees it. The
GUI host is what turns a widget into that command — it diffs the audio buses its
open documents read whenever a def, a free or a set changes them — and a
`/bus_tapStream` subscription *is* a watch, so a browser client never issues the
command at all.

Why the host and not each client: the alternative was to keep the ring on the
wire and have every client allocate and route before building a tree. That
duplicates the same plumbing in Python, in TypeScript and in wasm, and it breaks
the property that the GuiDef builders are host-agnostic — they would need a live
server handle to construct a widget. Doing it in the host does it once, in the
language the host is already written in, for every client at once.

### The meter needed a data path of its own

Making `meter` an audio-bus view exposed that it could not *be* one. A meter is
the one view in a mixer that exists per channel, and there are eight rings for
the whole system: a stereo master plus four channels would exhaust them, and a
meter never wanted samples anyway — it wants one number per block.

So the segment grew a second per-bus array beside the directory: the **level**,
published for every audio bus every block. Two decisions inside that:

- **The value is a peak held with a decay, not the raw block peak.** A block is
  64 samples; a display frame is a dozen blocks. A reader of the raw value sees
  the last block before it looked and misses the other eleven — which is to say
  it misses exactly the transients a peak meter exists to catch. The engine
  holds `max(block_peak, published * release)` with a 20 dB/s release, the usual
  peak-meter ballistic.
- **A decay rather than a max the reader clears.** The alternative — the engine
  accumulates, the reader `swap`s in a zero — is exact and needs no constant,
  but it is single-reader by construction: the second reader of a bus steals the
  first one's peak. Several readers of one bus is the normal case here (two
  meters in one window, a host and a headless capture), so the value has to be
  readable without consuming it.

The cost is one pass over the block per audio bus per block, one relaxed store —
no allocation, no lock, guarded like the rest by `tests/rt_safety.rs`.

## A widget's own size is a function of the table, and the layout still measures nothing

Until now a GuiDef declared *every* size that was not an even split: a control
in a column got half the window unless the script wrote `h=28`, and the numbers
in the examples were guesses that happened to look right. Giving each widget
kind a **natural size** — how big it wants to be — removes the guessing, and the
question is what that size is allowed to depend on.

It depends on two things: the host's sizing table and the widget's own
presentation props (`text_size`, whether it carries a label, whether it wraps).
It deliberately does **not** depend on the widget's data. A label's width does
not follow its string, a scope's height does not follow its sample count, a
menu's does not follow its options. The reason is not purity: a size that reads
the data turns `/gui_set` — the incremental update the whole protocol is built
on, arriving many times a second — into a relayout of the window, which is both
a per-message cost and a visible jump under a live value. A widget with more
content than room clips or scrolls; it does not grow. That is also what keeps
the layout **one pass with no measurement**: a container never asks its children
how big they are, it asks each kind what it wants, which is a table lookup.

Two consequences worth stating, because both look like omissions:

- **An explicit `weight` beats the natural size**, and comes before it in the
  resolution order (fixed `w`/`h` → `weight` → natural → a share of the
  leftover). Without that, "stretch this button over the pane" would have become
  inexpressible the day buttons learned their own height — and it is the *only*
  thing a single-control window (a bound knob, a standalone bundle) wants.
- **A container does not hug its content.** A row of menus does not shrink its
  panel to their height, because that is the measurement pass this layout does
  not have. Chrome that must be thin still says `h`. What changed is that the
  number inside the strip is no longer a guess: the strip's children size
  themselves, and the strip states its own extent.

  *(Superseded, and by composition rather than by measurement: a container that
  asks for it with `hug` is now the composition of its children's own natural
  sizes, and a window that asks for it is the composition of its content. Both
  bullets above still hold everywhere the prop is absent, which is everywhere it
  is not written — see "A size may read a prop, never a value" below.)*

Which kinds have one follows the content/surface split: content whose extent the
widget itself knows (a label's line, a button, a toggle, a number, a menu, a
single-line field, a slider's thickness across its track, a knob's height, a
ruler strip's) versus a surface whose extent is the caller's (a panel, a scroll,
a patch canvas, a track, the heavy views, a plot, a node tree, a canvas). Mixed
is ordinary rather than exceptional — a slider is intrinsic across its track and
elastic along it — and a knob is intrinsic only in height, because its disc
sizes itself to the shorter side of its body and centres there: extra width is
slack it absorbs, so a row of knobs still spreads instead of packing left.

This redrew every existing GuiDef, which was accepted deliberately: the windows
that improved are the ones where a caption or a control had been taking half the
pane from the view beside it.

## The wire is logical, the plane is physical, and the shell is the one that knows the scale

The sizes a GuiDef declares — `w`/`h`/`x`/`y`, a container's `margin`/`gap`, a
widget's `text_size` — were physical pixels, which meant a window was as small
as the display was dense: on a doubled HiDPI screen the tree drew at half its
apparent size while the window itself (winit's `LogicalSize`) came out at the
size the script asked for. The wire is now **logical** and the host resolves it.

**One scale per window, resolved once.** The size table (`host/metrics.rs`) that
L6 centralized is logical too, and each window holds its own resolution of it —
`Metrics::resolved`, every role scaled and re-quantized by its family (extents
onto the 2-px grid, hairlines onto whole pixels, glyph scales onto half-steps).
That runs on a **scale change**, not on a frame: layout and painting stayed the
code they already were, reading one table through `Host::metrics_for(def_id)`, so
the per-frame cost of HiDPI is zero — which is what matters on a page holding
forty canvases. Snapping is part of resolving rather than a pass afterwards,
because the chrome *is* hairlines: a divider, a track edge and a font pixel are
one unit each, and a fractional position turns a crisp line into a two-pixel
grey smear.

**The scale is an input the shell writes, never something the core detects.** The
platform-agnostic host may not ask a window manager or a DOM what density it is
on — that is the seam `check-wasm.sh` enforces — so `Host::set_ui_scale` is a
door the shells push through: natively from winit's `scale_factor`, re-armed on
`ScaleFactorChanged` (whose `inner_size_writer` is *answered*, with the same
logical extent the window had, so a 800x600 shell stays a 800x600 shell across
the move); in the browser from the page's `devicePixelRatio`.

The browser needs its own *re-arm* for the same reason, and it is not the one a
page would reach for first: a `ResizeObserver` watches the **CSS** box, so
browser zoom or a drag onto a monitor of another density moves the ratio while
the box stays exactly as it was — no callback, and the host goes on resolving
against a scale that is no longer true. So the page watches a media query on the
current ratio (`(resolution: 2dppx)`) and re-arms on the new one each time it
fires (`onScaleChange`). Two triggers, because the box and the density are two
facts.

That last one reshaped the browser binding rather than porting anything. The host
was told its canvas size in device pixels only — the page multiplied its element
box by `devicePixelRatio` and handed over the product — so **the ratio was
destroyed at the boundary** and no arithmetic recovers it. `resize` now carries
it (`resize(defId, width, height, scale)`), and `canvasBox` reports the box and
the ratio side by side instead of collapsing them. The pair does not collapse in
the other direction either, which is why the binding takes device pixels and the
ratio rather than the logical box: the `<canvas>` backing store is an integer the
page has already fixed, and a host that recomputed it from logical times scale
could land a pixel off it — surface and element misaligned, exactly the blur this
milestone exists to remove.

**A zoomable view's default zoom belongs to whatever its content unit is.** The
plane's own units left one thing unanswered: on a doubled display a patcher's
boxes came up half the apparent size of the chrome around them. The fix is not to
scale the plane's geometry (that would put a second multiplier inside a space
that already has one) but to start the plane at the density it is drawn on: a
`scroll`'s `view_zoom` — physical pixels per content unit — **defaults to the
window's UI scale**, so one content unit is one *logical* pixel, and a named zoom
(from the tree or from the wheel) is literal from then on.

A default nobody can name needs a way back, which is why `/gui_set view_zoom 0`
(or any non-number) **clears** the zoom rather than setting one: a script that
wants "the plane as it opened" has no number to ask for — naming `1.0` pins one
physical pixel per content unit, which on a doubled display is half the size the
plane came up at. The shape is borrowed from `theme`, where an empty value drops
the overlay.

Fitting the zoom to the content was rejected as the default: a plane's content
unit is a *display* unit (a box is 96 units wide because that is how wide a box
should look), so fitting would make a box's apparent size follow **how many boxes
there are** and re-zoom the plane on every edit — the "size follows data" failure
the natural sizes were built to avoid. Zoom-to-fit is a command.

The rule sorts the other navigable views without a second thought, because their
content unit is *data*: a `waveform`'s and a `spectrogram`'s is the sample, a
`pianoroll`'s vertical is the semitone, a `score`'s is the engraved staff step.
None of them wants the display scale anywhere near its window — a denser screen
means the same span at finer resolution, which is exactly "never resolve the
signal finer than the screen" read from the other side. Their defaults (the whole
buffer, the pitch window fitted to the lane, the page fitted to the rect) are
already fits to their content, and stay untouched.

**A zoom is an enlargement, so a placement's scale picks its size table.** The
default exposed a second thing the plane had never really done: only *text*
scaled with the zoom, while every metric role (padding, a slider's track, a
knob's diameter, the gap between rows) stayed at the window's table — the
"patcher posture" that was tolerable when the zoom was 1 by default and reads as
broken proportions the moment a box is drawn at another scale. So a placement
carries the table it is drawn with (`Metrics::at`, resolved at the placement's
scale), and the layout measures with the same one: inside a scrolled box the
declared lengths, the roles and the text all carry one factor. The layout
distinguishes three spaces to make that exact — the window, a plane's *content
units*, and the inside of a scrolled child at the accumulated zoom — which is
also what stopped a plane's default margin from being counted twice.

Two more bugs surfaced with that default and were older than it, both from mixing
two spaces inside one expression: a plane's content extent was compared against
the pane's *pixels* (identical at zoom 1, off by the zoom at any other, which
pushed a self-centring graph into the corner), and a widget's natural size was
measured at its `text_size` while its drawing uses `text_size * scale` — so
inside a zoomed plane a box promised room for a 14-pixel line and drew a 28-pixel
one, and a knob, whose height is label strip + disc + read-out strip, gave the
disc away to the text and rendered as a dot. Both are the same lesson: **an
expression may not mix a content unit with a pixel, or a declared text size with
the size it draws at.**

A third had the same shape and cost the wheel its anchor: the content extent of a
graph-sized plane is "the graph, but never smaller than the viewport", and once
the viewport was converted with the *live* zoom the content shrank as the zoom
grew — so the plane the pivot math holds a point in was itself moving, and the
graph slid out from under the cursor. The conversion uses the plane's **natural**
scale, which makes the extent constant under zooming.

**Two pixel spaces coexist, and that is the invariant to state.** Chrome is
logical; a *navigable plane* is physical. A `scroll` workspace's content plane
keeps its own units — its `content_w`/`content_h`, its `view_x`/`view_y` and its
children's placements — because it carries a zoom of its own and its pan is
written in the pixels the pointer moved, so folding a second multiplier into it
would make a drag and a declared position disagree. The heavy views say the same
thing for the same reason: `render_width_px` is physical and "never resolve the
signal finer than the screen" is untouched. What follows from this, and is
deliberate: a widget's own interlocking structural geometry (the patcher's
box/port series, the roll's key gutter and lanes, the score's staff step) is
module-local by L6's rule and lives in the physical space too, so a plane's
interior does not grow with the display's density — it grows with its own zoom.

## A render's noise belongs to the render, and its file to the server

Two decisions about the offline path, both taken by asking who owns a thing
rather than what is convenient to reach.

**The seed.** Stochastic UGens drew their per-instance seed from a
process-global counter that nothing reset. A fresh process was reproducible, so
the golden-file tests passed and nobody noticed; but the *second* render in one
process drew different seeds than the first, so the same score gave different
noise. That is a strange property for an offline renderer — a bounce is supposed
to be a function of its score — and the noise module already claimed the
opposite in its own header. The counter now lives in `BuildCtx`, which every
UGen factory already receives, and `CmdTranslator` reserves one contiguous run of
seeds per synth, so the sequence is a function of the score and of
`RenderConfig::seed`. Two synths still never share a stream, which was the
original reason for a counter at all; what changed is only *whose* counter it is.
The seedless constructors went with it: `WhiteNoise::new()` had no correct
meaning once seeding was per-instance, and a default that silently correlates two
generators is worse than no default.

**Where the sequence starts: a random process is unpredictable first.** Moving
the counter into the render left a second question, and the first answer to it
was wrong: the sequence started from a fixed constant, so an unconfigured render
was reproducible and `--seed` gave you a *different* take. That is the testing
answer, not the musical one. When a piece has a random process in it, the point
is that playing it again is another performance; reproducibility is the
exception you ask for, by fixing the seed, and it is worth having precisely
because it is not the default. The client had this right all along —
`RandomContext.seed(None)` draws from `os.urandom` and every pattern is
unpredictable until `main.seed(n)` says otherwise — so the server was the odd one
out, and the asymmetry showed up as soon as it was named.

So a render with no seed draws one (`clausters_core::rng::entropy_seed`), and a
booted server starts its sequence there too. What makes that usable rather than
merely honest is the other half: **whoever draws a seed reports it**.
`RenderStats::seed`, the `--nrt` summary line, `--stats`'s JSON, `stats.seed` in
Python — you play a score, you like the take, and the seed is how you get it
back. An unpredictable default without a reported seed would just be a take you
can never have again.

The cost is that anything comparing two renders for bit-identity must now pin a
seed and say so: the golden scenes, the parallel-vs-sequential test, the
native↔wasm parity fixture. That is an improvement in what those tests *say* —
each now declares that it wants the same take twice, instead of inheriting it.

On `wasm32` there is no entropy this crate can reach (`SystemTime` is not
implemented there), so `entropy_seed` returns the constant and the JS door takes
a seed explicitly: the browser has `crypto.getRandomValues`, and the shell
forwards entropy from the edge that has it rather than inventing any.

**The file.** The Python client used to render into memory and write the WAV
itself, which meant a duplicate WAV writer, a duplicate decision about sample
format, and — because the stdlib `wave` module cannot read a float32 WAV — no way
to read back what it had just written. Meanwhile the server had both halves
already: `render_to_wav` streams a score to disk through hound, and `read_audio`
decodes WAV/FLAC/OGG/MP3/AAC/ALAC/AIFF for `/buffer_allocRead`. Neither was reachable
from a client.

The reading half became an FFI export, because a client genuinely needs the
samples in its own process. The writing half did **not**: the client hands the
score to `clausters --nrt`, the same renderer the CLI drives, and the server
writes the file. That avoids widening the embed ABI for something a subprocess
already does, and it is *faster* — a sixty-second stereo render is 5.7 million
floats, and not marshalling them is the difference between 430 ms and 60 ms. The
cost is that `--nrt` needed a machine-readable mode (`--stats`) so the client
learns the frames, events, peak and RMS without reopening the file, and that the
`clausters` binary must be findable. Both are acceptable for a client that
already requires the server.

What this fixes in the API is the shape of the return. `path` now means **where
the audio goes, not whether there is a result**: every render returns one
`RenderStats`, and `samples` is populated only when the audio did not go to a
file. The alternative — samples in one case, a bare frame count in the other —
makes every caller branch on an argument it passed itself.

**Interleaved stays the currency.** It is the server's own layout (`/buffer_getRange`
indexes `frame * channels + channel`; `/buffer_export` writes the same order), so
audio *going to* the server needs no conversion, and the one place the server
goes planar — Faust's `soundfile` — it converts internally. Deinterleaving is a
client-side convenience for analysis, and it stays in Python rather than the
core precisely because `array` extended slicing is already a C-level strided
copy: crossing the FFI to do it would copy more, not less.

## A moment is a value, and the destination is what stamps it

Sending OSC to an application that is not our server used to be impossible in
any timed sense. The client could *receive* from anywhere (`OscReceiver` +
`OscFunc`), but every path that turned a routine's logical beat into a timetag
was written inside `Server.send_bundle`, wired to that server's own target and
interface. A second destination would have had to copy the arithmetic.

sc3 solves this in the *interface*: `NetAddr.send_bundle(time, ...)` passes a
delta, and each interface resolves it against `main.current_tt._seconds`, with
an NRT subclass overriding `_get_timetag`. That placement gives something real —
nested sub-bundles all resolve against a single captured "now" — and it is why
any `NetAddr` in sclang is timed without knowing what a clock is.

We did not copy it, for two reasons. It rests on `main` being a process-wide
singleton that owns the interface, the main time thread and one physical time
axis in seconds; here `Session` owns the server and the clock, `current_tt` is
thread-local scope rather than configuration, and beats are per clock with
per-clock origins — so an interface cannot know which axis a send belongs to
without asking an ambient global we deliberately do not have. And the argument
that justifies the placement, nested bundles, is not something we encode: the
codec takes a flat list.

So the concepts were split by owner instead. `Moment` — a clock and an exact
beat on it — is the value that answers "what time is it *for this event*", and
it is the routine that carries one, because the routine is what the clock
stamped. A `Destination` owns the target, the carrier and the policy that turns
a moment into wire time. The interfaces keep receiving an absolute instant, so
none of their signatures moved, in either client.

What that leaves in `Server` is exactly what belongs to a server *we control*:
its `latency`, `/sched_at` at an absolute sample, `/server_sync`, and the offline score.
None of those is standard OSC, and none of them is an external application's
business — least of all latency, which is the headroom our audio pipeline needs,
not a fact about someone else's program. An external destination therefore adds
nothing to the timetag; a program that needs to run ahead asks for it as an
explicit delay.

Two things fell out that were not the goal. The clockless case stopped being a
second code path: with no clock a moment is wall-clock now and beats read as
seconds, so `Event().play()` outside a routine goes through the same call as one
inside it, and `send_bundle` with no clock is defined rather than an error.
And `NetAddr` lost its `send_*` methods: it is right as an address — which is
what the name means everywhere outside sclang — and wrong as an emitter, since
emitting needs an interface and a timing policy that an address has no business
holding.

## Who is asked decides the shape: a catalog, a record, and absence as a state

Introspection had grown one method at a time and ended up with three different
shapes for the same idea. `Server.query_defs`/`query_buffers`/`query_ugens`
returned records; `Server.query_tree` returned a nested dict of ad-hoc keys;
`Server.node_query` — a `<noun>_query` among `query_<noun>`s — returned another
dict; and `Buffer.query` returned neither, filling the *handle* in place (the
web client, whose fields are readonly, returned a filled copy instead: one
call, two semantics). A `BufferInfo` existed but only the plural produced one.

Two questions were being conflated. *What do you hold?* is asked of the
**server**, is about every instance of a type, and answers with a structure of
records. *What are you?* is asked of the **instance**, and answers with one
record — the same one. That maps onto the rule the rest of the API already
follows: a command addressed to a resource is that resource's method. So
`node.info()` and `buffer.info()` replaced the server-side singulars, the
plurals stayed exactly where they were (they are a different question, not a
convenience), and every record became an `Info` dataclass carrying no server
and no commands — data that can be serialized, sent to a view, or kept.

*Kept* is where the resources stop being alike, and the difference is the
server's, not the API's. A buffer's shape changes only when a command of yours
changes it, so the handle keeps its `BufferInfo` and reads `frames`/`channels`/
`sample_rate` off it. A node's controls move on their own — an envelope runs, a
mapped control follows its bus, a `done_action` frees it — so `NodeInfo` is a
photograph and nothing caches it. Same interface, opposite lifetime, stated
rather than implied.

The tree then had no reason to invent a shape: it is the node catalog, so its
entries are `NodeInfo`s and what it adds is the nesting. That needed the wire
to carry as much per tree entry as `/node_query.reply` does, which is why scsynth's
`/group_queryTree` *flag* widened into a **detail level** — 0 and 1 remain what
scsynth sends, 2 appends the maps and inferred bus lists. The client asks for 2
and derives the rest from the structure it already received: a node's parent,
its siblings, a group's head and tail. One reply, no follow-up query per node.
The tree is data first and drawing second, which in Python is exactly the
`repr`/`str` split (the web client's equivalent is the object versus its
`toString()`).

Finally, absence. Three commands had three answers for "it is not there":
`/def_query` reported an empty family, `/buffer_query` zeros (indistinguishable from a
real buffer), `/node_query` a `/fail`. A resource that is gone is a **state of the
resource**, not a protocol error — and a batch query must survive one dead id,
which is why `/def_query` never failed in the first place. So every singular query
now answers with a record whose `exists` is false, marked on the wire by a
sentinel in a field that has no valid negative value: `isGroup = -1`,
`frames = -1`, the empty family. `/fail` is left for what it should always have
meant — a malformed request.

## One naming rule beats compatibility with a name nobody types twice

Clausters spoke scsynth's command names because it speaks scsynth's *model*, and
inheriting the vocabulary looked free. It was not. scsynth's rule is
`/<letter>_<verb>`, where the letter is a resource class (`n`, `s`, `g`, `b`,
`c`, `d`, `u`); everything Clausters added had no letter to take, so it grew a
second tier of full-word namespaces — `/graph_new`, `/midi_bind`,
`/transport_play`, `/tap_stream`, `/server_info` — and the protocol carried two
conventions at once. Worse, the seams between them had gone quietly wrong:
`_info` was the reply suffix everywhere (`/n_info`, `/b_info`, `/d_info`) except
in `/server_info`, which was a command; watching a control bus was named after
the resource (`/c_stream`) and watching an audio bus after the mechanism
(`/tap`); `u_` meant a UGen *instance* in `/u_cmd` and the UGen *catalog* in
`/u_query`; and sending a def was `/d_recv`, `/d_faust` or `/d_graph` — two of
them naming a family where the third named an action.

The decision is to **break the command names completely** and spell every one of
them by a single rule, `/<resource>_<action>` with the resource as a full word,
replies as `<command>.reply`, and ranges as `Range`. The reference states the
rule at the top ([`schemas.md`](schemas.md)), because a convention a reader can
apply is worth more than a table they have to consult.

What made this affordable is that the compatibility being given up was never
real. Clausters had already diverged in the places that decide whether sclang
can drive it — its own def formats instead of `.scsyndef`, a typed `/cmd`
surface, a persistent `/error` toggle — so it was close enough to look like a
drop-in and far enough to fail as one, which is the worst of both. And the model
is what a SuperCollider reader actually carries over: the node tree, the add
actions, the bus and buffer pools, the async barrier. Those are unchanged. Only
the spelling moved, and it moved mechanically (`/s_new` → `/synth_new`,
`/c_getn` → `/bus_getRange`).

Two commands changed shape rather than just spelling, and both for the same
reason — the old name was carrying a datum:

- **`/def_send <family> …`** replaces `/d_recv`/`/d_faust`/`/d_graph`. A def has
  a family whatever command sent it; `/def_query` already reports it, in the
  same three spellings. So the family is an argument, and one command sends a
  def.
- **`/transport_query`** splits off `/transport_set`. One command that queried
  when called with no arguments and set when called with two was two commands
  wearing one name — and only the query has a reply to name.

Pre-1.0, nothing here is frozen; the counterpart obligation is that the four
packages move together, which this change did in one commit.

## `new` is the constructor, and the alternates get names

The Python client's node handles were built with class methods —
`Synth.new("beep", {"freq": 440})`, `Group.new()` — while `__init__` was left
holding the *other* meaning, wrapping an id that already exists:
`Synth(1003, "beep")`. That split was never decided. It is a literal
transliteration of sclang, where `*new` **is** the constructor because the
language has no distinguished initializer, so every constructor is a class
method by that name. Python has `__init__`, and the port kept a shape that only
made sense in the language it came from.

It did not even earn the `__init__` it displaced. Nothing outside the classes
themselves ever built a handle from an id that way: the code that names a node
reported by a responder, a query or the GUI uses the base class,
`Node(id, server)`. So the positional wrapping constructor was serving only the
factories that had taken its place.

So: **building a resource creates it.** `Synth("beep", {"freq": 440},
target=group)` allocates an id and sends `/synth_new`; `Group()` sends
`/group_new`. Naming one that already exists is the alternate, and alternates
get names — `Synth.from_id` / `Group.from_id`, which send nothing. The rest of
the named constructors were always alternates and stay: `Group.graph` (a
different command), `Bus.audio` / `Bus.control` (they pick the rate),
`Buffer.alloc` / `Buffer.read` (they block on `/done`), `Server.boot`,
`FaustDef.from_signals` / `from_source` / `from_box`.

`Bus` and `Buffer` keep a constructor that *names* rather than creates —
`Bus(4, channels=2)`, `Buffer(bufnum)` — and that is not an inconsistency. A bus
and a buffer are slots in a pool of fixed size, so an index is a meaningful
thing to write down: bus 4 is a hardware output on every server. A node id is
not; it comes from the client's `NodeIdAllocator` and means nothing to a reader.
`Bus(4)` says something, `Synth(1003, "beep")` never did.

The web client keeps `Synth.new(server, …)` for now, and the divergence is
deliberate: it has no ambient session, so the server is that method's first
positional argument, and as a constructor it would read `new Synth(server,
"beep", …)` — the server first there, the def name first in Python. The two
signatures converge only once a session resolves the server, so the port is
recorded against the milestone that adds one (`clients/web/PLAN.md`, W18)
rather than shipped as a second, differently-shaped constructor.

## A group's name is a label, and the id is still the identity

*(2026-08-01, M33)*

A group needed to be callable something. The node tree gives a group everything
a DAW channel has — a handle for many nodes, a place in the order, a lifetime —
except a way of saying which one it is: `1002` is the number the client's
allocator happened to hand out, and a console built out of groups reads as a
list of integers. So groups got names.

The shape of that decision is what matters, and it is the same one in three
places: **the name never replaces the id**.

- **On the wire**, no command takes a path where a node id goes. `/group_new`
  takes the label with the creation — a group is born knowing what it is, which
  is when the client knows too — `/group_name` renames one by id,
  `/group_query` resolves a path *to* an id, and every other command is
  untouched. A label carried by `/group_new` is judged **before** the group
  exists — a refused name refuses the creation, because a half-applied command
  that silently downgrades "a group called `mixer`" to "some group" leaves the
  client's model of the tree wrong in a way only a query would reveal. The alternative — accepting a string wherever a
  target int is accepted — would have rewritten the parsing of some twenty-five
  commands, paid a resolution cost on every message, and given a scheduled
  bundle two defensible answers about *when* the path resolves (at translation,
  or when the bundle fires). Resolving once and commanding by id has none of
  those problems, and it is what a client does anyway: it caches the handle.
- **In a path**, a group answers to its name *and* to its id, and an unnamed
  group contributes its id as the segment. That is what keeps every group
  reachable — naming is opt-in, and a subtree with an unnamed group in the
  middle stays addressable — and it is the reason a name may not be **all
  digits**: a numeric name would speak for another group's id segment. (A
  leading digit is fine; `8bit` is a name.)
- **In the reports**, an unnamed group reports an *empty* name, never its id.
  The id-as-segment is a path-composition rule, not a default name.

**A death has to be able to name what died**, and that is the one place the
"names live in the mirror" rule needed care: the mirror drops a node when its
command is *translated*, while `/node_end` only goes out once the engine
confirms the death. So a departing group's label is kept as an **epitaph** — a
bounded queue in the mirror, claimed by the notification — for exactly that gap.
Bounded rather than exact because a mirror removal does not always produce an
event (the node may already have been gone engine-side), and an unclaimed
epitaph is worth less than an unbounded map.

**Where it lives is the whole performance story.** The name is a field of
`MirrorBody::Group` in the network-side `TreeMirror` and exists nowhere else:
`node::Group` grew no field, `/group_name` queues no `Cmd`, and the engine has
no notion of a name. So the audio thread cannot pay for this feature — not a
byte per group slot, not a branch in `process_block` — and that is a structural
guarantee rather than a measurement. It follows the same reasoning as the
auto-sort analysis: anything a client needs to *say* about the tree belongs to
the mirror, and only what the engine must *do* crosses the FIFO. Paths are
composed on the walk and never stored, which is why renaming a group re-paths
its whole subtree in one command.

**The reply shapes follow the feature, not scsynth.** `/group_queryTree.reply`
was scsynth-compatible field for field, and a group name has to go somewhere: it
goes where a synth's def name goes, after the child count, so every node reads
`id, count, name` uniformly. `/node_query.reply` puts it after `headID, tailID`,
and the node notifications carry it as a last argument for *every* node event,
empty where there is no name. That uniformity was chosen over the two
alternatives on purpose — a separate names-only query would make a client cross
two replies by id to draw one tree, and hiding the name behind a `detail` level
would leave a shape nobody can parse without knowing which level they asked for.

Reading these as a compatibility cost would be a mistake, and the reasoning is
the one already recorded under *One naming rule beats compatibility with a name
nobody types twice*: the compatibility being given up was never real. Nothing
but our own clients speaks this protocol — sclang cannot drive Clausters and the
command names are not scsynth's — so a shape is chosen for consistency, and the
four packages move together in the same commit. A back-compatibility patch for
a reader that does not exist is a wart that outlives the reason for it: the
clients parse the field as always present, because it always is.

## A def is a template, so only an instance can be named

A group carries a label now (the entry above), which raises the next question
on its own: a GraphDef instantiates as a group, so should the def declare what
that group is called? It should not. A def is a **template**; a name is a
property of a **place in the tree**, and the two do not travel together.

Sibling names are unique, and a name that comes from the def repeats by
construction: instantiate the same graph twice under one parent and the second
label is already taken. For the per-voice sub-groups of `/graph_newVoice` it
collides *always* — every voice of one instance is the same def. That leaves
the server three ways out, each worse than an unnamed group. Refusing the
instantiation answers a client that asked for two voices with an error about a
label it never wrote. Creating it anonymous is the silent downgrade the naming
rules exist to prevent. Inventing a suffix (`reverb-2`) keeps the tree navigable
under a name the client cannot predict and has to read back out of it. An
unnamed group is not a hole in the addressing anyway: it answers to its id, and
a path reaches it through its decimal segment.

**Naming the instance is a different feature, and it already composes.** Both
clients allocate the instance's id themselves — no client sends `/graph_new`
with `-1` — so the label is a second message on an id the caller already holds,
`/graph_new` then `/group_name`, in order on the same connection. A `name=`
argument in the clients is the whole of it, with nothing new on the wire.

Carrying the label in `/graph_new` itself would give exactly one thing the
composition cannot: the name riding in the instance's own `/node_start`. That
is recorded here rather than built, because the gap it names is not the graph's
— **a rename notifies nobody**. `/group_name` pushes nothing to the
`/server_notify` clients, so an observer that builds its model from the
notifications misses every rename until it queries. If that ever bites, the fix
is for renaming to notify, which closes the case for every group at once; an
optional string in `/graph_new` would close it for graphs alone, and would add
the protocol's only label sitting in the middle of an argument list instead of
at the end of a fixed group of arguments.


## A transport can pause a piece it cannot seek

Every transport protocol worth surveying — JACK's `jack_position_t` with its BBT
and its slow-sync `Starting` state, Ableton Link, MIDI clock's Song Position
Pointer, VST3's `ProcessContext`, CLAP's `clap_event_transport` — shares one
assumption it never states: **position is an index into samples that already
exists**. `song_pos_beats` means something because something is addressable at
that beat.

A clausters piece generates sound. In the extreme it is one def running
stochastic processes and demand-rate sequences on the server, with nothing to
read and no messages arriving. Such a subtree has no index. Its position *is*
its internal state, and no number summarises it.

So the transport splits what those protocols keep together:

- **Pause and resume are symmetric.** They work identically for samples and for
  a generator, because they are only a freeze of the subtree, the clock and the
  queue. Universal, and cheap — the freeze itself is `NodeTree::set_paused`,
  which already keeps a node in the tree with its state intact.
- **Locate is asymmetric.** Over generated samples it is an index; over a
  generator it does not exist as an operation. `/transport_locate` therefore
  moves the position and never the state of a node, and says so.

This is not a limitation worked around; it is the same law the arrangement layer
already states, arriving at the server. `clausters.form` distinguishes a
*generated* element (random-access: it can be read backwards, sliced, edited)
from a *generator* (forward-only: it can only be evaluated), with **render** as
the change of state between them. A generator becomes locatable by being
rendered — the same answer, one layer down.

It follows that the **locatability flag lives in the client, not the protocol**.
The server cannot know which of its nodes are generators; `form` can, because it
holds the samples. Putting a capability bit on the wire would be the server
promising something it cannot check.

## The two transports were absorbed, not renamed

`/transport_*` already existed as a shared beat grid plus an advisory rolling
state: fields a client read to phase-align, which the server stored and
broadcast but never scheduled audio from. The governing transport needs exactly
those fields plus a group — it is, near enough, a `jack_position_t` without BBT.

The choice was to rename the old one (`/grid_*`, freeing `/transport_*` for the
real transport) or to absorb both into one command family. Absorbing won: one
vocabulary across the three books, `Playhead.follow_transport` unbroken, and no
protocol break for a distinction most clients never make.

The price is one command with two intensities — with no group bound
`/transport_stop` is an advisory, with one bound it freezes the engine — and
that has to be said plainly in the reference rather than left to be discovered.
It is a smaller cost than two parallel families that clients can put into
disagreement with each other.

## The wasm binding is not held to the C ABI's shape, only to a written decision

`clausters-core` reaches the world through three bindings, and cargo checks only
that each agrees with *core* — never that they agree with each other. The C ABI
is compiler-checked; `clausters/_native.py` restates it by hand in ctypes; the
wasm surface is a third statement. So a function could be added to one and never
reach the others with every build green, which is how a "shared" core stops
being shared.

The two legs are held to different standards, on purpose.

**Python owes the C ABI total coverage**, so its check is a comparison and needs
no list to maintain: `ctypes` caches each symbol on the `CDLL` instance as it is
reached, so after `_configure` runs the instance dictionary *is* the record of
what the binding declared, and the crate's own source is the other side.

**The wasm surface is legitimately partial**, and the first attempt to test it
as "the same set" was the wrong instrument. A browser already has WebSocket;
libverovio is not built for wasm; JavaScript has no `u64`; wasm frees by `Drop`
where C needs an explicit `_free`; the C peak cache is a byte blob its consumer
parses while the wasm one is a live object. Measured, the two surfaces differ in
*both* directions and most differences are correct. A test asserting equality
would have needed some sixty exemptions — and those exemptions are the only
interesting content, which is the signal that the exemption list *was* the
artifact.

So `docs/bindings.md` is a manifest of the shared surface: one row per symbol on
either side, with a verdict when a cell is empty — `idiom` (same capability,
different shape), `n/a` (deliberately absent, with the reason), or `gap`
(missing, nobody has decided). `tests/bindings.rs` fails when a symbol appears
in neither column, and when the manifest names one that no longer exists.

The point is what it does *not* enforce. Divergence stays allowed, because
forbidding it would only teach people to lie to the table; what becomes
impossible is divergence nobody wrote down. `gap` is a first-class verdict for
exactly that reason: the honest answer is often "I have not thought about the
other side", and a manifest that had no way to say so would be filled with
invented rationales instead.

## There is no widget registry to generate the builders from

The GUI layer has the same disease the core's bindings had, one level up: a
widget prop is declared in the host (`clients/gui/src/host/widget`), in the
Python builder and in the TypeScript one, and **nothing ties the three
together**. The wire is untyped JSON on purpose — that is what lets a widget be
added without touching the protocol — so an unknown prop is ignored and a prop
added on one side and forgotten on another is silent in every build.

The plan for it was to generate both builders from a declarative table taken
from the host's widget registry. Measured, that table does not exist and the
name misleads: `host::registry::Registry` is bookkeeping over
`Map<String, Value>` — it stores whatever props arrive and knows nothing about
which ones a `knob` accepts. The vocabulary lives in the *schema's* two wire
passes, `widget::build` (construction) and `widget::apply` (`/gui_set`), spread
over one arm per kind plus the shared bundles (`Flow`, `EditorProps`, `Range`)
those arms embed. There is a source of truth; there is no table.

Generation was also the wrong goal for a second reason: the explicit keyword
signatures **are** the documentation. The Python builder's docstring is the
widget's user reference (the API page is generated from it) and the TS option
type is what an editor completes. A generated `**props` passthrough would trade
the thing a reader uses for the thing a machine finds convenient.

So the artifact is the one `docs/bindings.md` already established for the core:
`docs/gui-props.md` records every prop that does *not* reach all three surfaces,
with the same three verdicts (`idiom`, `n/a`, `gap`), and
`clients/python/tests/test_gui_props.py` reads the three and fails on a
divergence the table does not name — or on a row that names one no longer there.
The readers differ because the sources do: Python is read by **calling it**
(`inspect.signature`, exact), the TS builders and the host's two passes are read
statically.

Its first act was to surface twenty-eight rows, twenty-six of them pointing the
same way: the host implements the timeline chrome (the playhead, its loop
region, the selection, the vertical window), `docs/gui-protocol.md` documents
it, the TS `TimelineOptions` declares it once for every timeline widget — and
the Python builders name it widget by widget, so `track` and `timeruler` name
almost none of it. The other two point the other way and are the more urgent
kind: the TS `plot` offers `buffer` and `cache` that the host's plot never
reads, so a page passing them gets no error and no effect.

## A cost gate compares two runs, not a number, and its threshold is measured

The R track's invariant is that a refactor closes with a before/after
`examples/bench.rs`. That is a step a human remembers, and a check that depends
on someone remembering is one that eventually is not run — so it became a CI
job. What the job is *not* is the obvious design.

**Committing a baseline number does not work.** `ubuntu-latest` runners are
shared and virtualized; 10–20% swings between runs of identical code are
ordinary. An absolute threshold against a stored number fires on the weather,
and a gate that cries wolf is ignored, which is worse than no gate. So the job
builds and benches the **merge base** and then the head *in the same job on the
same runner, back to back*, and compares those two. Both measurements share the
machine, the thermal state and the noise; the ratio means what the absolute
values do not. The cost is one extra release build per pull request.

**The thresholds are measured, not chosen, and they differ per metric** —
because the metrics do. Three runs of an identical build, compared pairwise on
this machine:

| metric | median | worst |
|---|---|---|
| `x_real_time` (210 comparisons) | 0.5% | 5.6% |
| `peak_block`, aligned, ≥32 voices | | 21.0% |
| `peak_block`, staggered, ≥32 voices | | 34.6% |
| `peak_block`, one voice | | **251.8%** |

So throughput is gated at 10% and the peak block at 50%, each roughly twice its
observed worst case: loose enough never to fire on noise, tight enough to catch
the cliff the gate exists for — a lost inline, an allocation that crept into the
block path. A 3% drift is invisible to it on purpose.

The last two rows are why some measurements are **reported and never gated**.
The spectral peak at one voice is a single hop's spike, and two runs of the same
build differed by 250%: it is not a measurement. The *staggered* peak is worse
in a more interesting way — it measures whether two chains' hops happened to
collide this run, which is exactly what the stagger exists to scatter, so its
variance is the feature working. The **aligned** peak is the deliberate worst
arrangement (every chain hops on the same block), which makes it a property of
the code, and it is the one that is gated. The Faust rows carry whatever the
LLVM JIT decided that run, and the worker sweep reads the machine's core count;
both are printed and neither can fail a build.

**A gate nobody has seen trip is a gate nobody knows is wired up**, so it was
verified in both directions before landing: three identical runs pass in all
three pairings, and `#[inline(never)]` on two hot bus accessors turns eleven
gated rows red at once (`default/1` −10.8%, `sine/ugen/1` −16.2%, the aligned
512-voice peak −49.5%).

## A page holds one host because it wants one, not because it can hold one

`start()` used to be a page-wide singleton: a second call reached winit's
`EventLoop::build`, which answers `RecreationAttempt` — a panic inside the wasm,
not an error a caller could catch — so an embedder wanting a second client had
to refuse it rather than hand back a stack trace.

The refusal was reasoning from the wrong constraint. **The event loop is what a
page can hold one of; the host is not.** That loop already drives any number of
windows — it is how one host serves a document's canvases — so the fix is not to
partition anything, it is to stop conflating the two: build the loop once,
memoize it in the proxy the instances already shared, and let `start()` add an
instance to the app already running.

What made this worth getting right rather than working around is what the
workaround would have cost. Sharing one host between two clients means their
widget ids land in one namespace, so the ranges have to be partitioned — and the
allocating clients are *separate processes with no channel between them*. The
only arbiter that can see who shares a tab is the page, which appears late, after
the first `plot()` has already assigned ids. Every escape from that is bad in its
own way: a slot derived from a session key collides silently, a claim file in a
runtime directory puts protocol state on a disk the client may not share, and
rebasing an already-built tree onto a slot learned at mount is correct but
fiddly. A second host instance makes the question not arise: independent id
spaces by construction.

The same reasoning settles the audio side, and settles it the same way. Nothing
in the engine was page-global either — `bootClausters` has always built its own
`AudioContext` and worklet per call — so `server()`'s memo *was* the singleton.
It stays, because sharing is what a page wants (its components play into one
mix), and `engine()` sits beside it for the caller that must not share a node,
bus and buffer space with the rest of the document.

So the rule for both, and the reason the default reads as a default rather than a
limit: **one host and one engine per page unless a caller asks for another.** Two
of them in one tab are as independent as two tabs. `guiHost()` in `page.ts` keeps
its memo — its contract is *the page's host with the page's default canvas*, and
a second one of those is not a thing anyone wants — and `newGuiHost()` sits
beside it for the caller who needs an instance.

That second function was not there at first, on the reading that such a caller
could go one layer down to the binding surface. That reading was wrong, and the
evidence was in the tree: the front end that needed it went down there, importing
`clausters_gui.js` and calling `start()` itself, because the client offered
nothing else. A capability the client has and does not expose is a capability
every consumer re-implements. `newPools()` came with it for the same reason —
page-global id pools are right for components that share the page's engine and
wrong for a client that does not, and the page having exactly one client had
stopped being true.

The cost of an instance is worth naming, because it is what makes the default
defensible: the wasm module and its memory are shared, and the GPU was already
per canvas, so a second host downloads nothing and adds no device. A second
*engine*, on the other hand, is a second `AudioContext` — they mix in the browser
rather than in the engine, and browsers cap how many a page may have (Chrome at
six), which bounds how many independent clients a tab is worth holding.

One thing had to be added rather than merely unlocked. Nothing ever closed a
host, because a page that holds one until it unloads has nothing to close; a
caller that opens them over time does, and an abandoned instance keeps its
WebSocket open, its `setInterval` running and its GPU surfaces alive. Hence
`GuiBridge::close`.

## A decoded timetag is Unix seconds in every client, and only a reader found out

The three bindings of the shared core agree on what they *export*; nothing makes
them agree on what a value *means* once it has crossed. A timetag argument was
the case where they did not. The Python decoder subtracts the NTP epoch and
hands back Unix seconds, as does the server's own `osc::ntp_to_unix`; the wasm
door built the float straight from `seconds + fractional/2^32` and handed back
NTP seconds — seventy years off, silently, in the one shape a caller cannot
sanity-check by looking at it.

It survived because **no consumer read one**. `/clock_query.reply`'s third
field is the only timetag the server sends as an *argument*, and the sample-clock
tracker models the counter from the first two: the anchor it needs is a local
timestamp, not the server's. The divergence sat in the codec from the day the
codec landed and cost nothing until a wall-clock clock joined a transport, where
mapping the grid's sample origin to Unix time is exactly what the field is for.

Two things follow, and the second is the one worth keeping. The fix is trivial —
the wasm door converts through the core's own function, so there is one
conversion and not two. The lesson is that the parity vectors would not have
caught it: they were **encode** vectors, frozen from the Python encoder and
compared byte for byte, and a decode divergence is invisible to them. Byte parity
on the way out is not value parity on the way in.

So `gen-osc-vectors.py` now writes a second file, `osc-decode-vectors.json`:
packets the Python *encoder* cannot even build — it has no timetag tag — paired
with what the Python *decoder* reads out of them. The generator hand-assembles
the bytes precisely because the reference being frozen is the decoder, not the
encoder. It is a small file and will stay small; what earns its place is that
it covers the direction where a value's *meaning* crosses, which is where two
implementations of one wire drift without any test going red.

## The third carrier writes time, and a page's render has no file at the end

The web client's offline drive had two things to decide, and the second is the
one that does not follow from the first.

**Where the score lives.** The Python client puts it in the *interface* — an
`OscNrtInterface` that accumulates bundles instead of sending them — and the
seam this client has at that spot is `Connection`, which carries bytes and
nothing else. A score cannot be assembled from bytes: a bundle's timetag has to
say *when*, and by the time the packet is encoded that number is already inside
it. So `Connection` grew two optional members — a `timeMode` (`"unix"` or
`"score"`) and a structured `addBundle(secs, messages)` — and the `Server`
branches on the first exactly where the Python client branches on
`interface.time_mode`. The alternative was a second seam above the carrier, and
that would have made "which carrier" a question the layers above `Server` could
ask, which is the one property the seam exists to prevent.

The consequence is worth stating plainly: **only the connection changes**. The
same patterns, defs and routines write a score that a live session would have
sent, and the score comes out byte-identical to the Python client's for the
same piece.

**Where the audio goes.** Here the two clients genuinely part. Python's
`render(path=...)` hands the score to a `clausters --nrt` process that streams
straight to disk — the samples never cross into Python, which is what lets a
long bounce not build millions of floats, and what makes `int16`/`int24`
output meaningful. A page has no process to hand a score to and no filesystem
to write to: its renderer *is* the wasm engine already in the tab, and what it
produces is a `Float32Array` in this page's memory. There is no honest `path`
to offer, so the client offers none, and the intent is served twice over
instead — `wavBytes(stats)` for a download, `Buffer.fromSamples(...)` to put
the take straight back into the engine (the reference client's render-then-load
with the file taken out of the middle, since the carrier shares memory with the
server).

One consequence had to be repaired rather than documented. `render` with no
seed is supposed to draw a fresh one — a take of a noisy piece is a
performance, unpredictable first — but the engine's entropy source is
`SystemTime`, which wasm does not have, so a seedless render there takes a
**fixed** seed and every take is the same take. The platform that has entropy
is the caller's, so the client draws the word from `crypto.getRandomValues` and
forwards it. The wasm shell inventing one would have been the wrong place: it
owns no logic, and this is a capability of the edge, not of the renderer.

## A responder listens to the connection it has, since a page can bind no port

`OscFunc` is the client's input path in both clients, and the port keeps its
whole surface — the constructor, the `(msg, time, src)` callback, the argument
template, `enable`/`disable`/`free`/`oneShot`, the builder form. What could not
be ported is what sits *underneath* it, and the difference is not a design
preference: in the reference client a responder registers with a **receiver**
that binds a UDP socket of its own, so any application on the machine can target
it, and a browser tab can bind nothing and be addressed by nobody.

So a receiver wraps the **`Connection` the client already has**. Everything a
page can hear arrives on the carrier it opened — the in-page engine or a socket
to a server — and that is where the door goes. Three consequences, each visible
in the API rather than hidden:

- **`src` names a carrier**: a socket's URL, or `page` for the in-page engine.
  It answers the same question a `(host, port)` answers — who sent this — with
  what a browser actually knows, and it is still what a responder narrowed by
  `src` compares.
- **The default receiver is the ambient session's server**, resolved per call
  rather than cached as the reference client caches its module default. A page
  can hold two sessions on two engines, and each server owns one receiver, so
  resolving late is what makes a bare `new OscFunc(fn, "/done")` mean "this
  session's server" in both of them.
- **`time` had to be brought across the wasm boundary.** The decoding door
  flattened bundles and dropped their timetags, which is all a reply reader
  needs and not what a responder's callback is defined to receive, so the shell
  grew a second door carrying each message's containing-bundle time in Unix
  seconds (`null` for an immediate bundle or a bare message) — the rule the
  Python client's own decoder applies, now asserted on both sides.

The fold that comes with it is worth stating too: the client's *own* reply
handling — the node ids that recycle as `/node_end` arrives, the streams that
decode `/bus_stream.reply` and `/bus_tapStream.reply`, the playhead that follows
`/transport_query.reply` — are ordinary `OscFunc`s now, on the receiver a page
would use. They had each grown their own address test inside a raw subscription,
which is the shape a responder exists to remove. `Server.onReply` stays under
them as the unmatched seam, because a decoder that wants everything in arrival
order is a real caller and not a responder.

## A buffer write is a copy that replaces the buffer, and it refuses what a read clamps

Context: the `/buffer_*` family could read samples (`/buffer_get`,
`/buffer_getRange`) and never put any back, so a client could show a buffer but
not edit one — the read → edit → write cycle an editor view is made of had no
last step, and the only way to install samples was the embed door
(`buffer_load`), which needs to share the process.

Decision: `/buffer_set` and `/buffer_setRange`, named and shaped as the mirror
image of the two reads (flat interleaved indices, single samples and runs), on
the same NRT queue as the rest of the writing family.

- **Copy-and-swap, not mutation, and it costs nothing to choose.** Buffers are
  immutable and the engine holds a clone of the network side's `Arc`, so every
  job here already builds a replacement and installs it whole — `/buffer_read`
  and `/buffer_zero` do exactly this. A write is one more of those: the samples
  land in a copy that swaps in, the old `Arc` leaves through the garbage FIFO,
  and the audio thread cannot observe a half-written buffer. The alternative —
  writing into the live buffer under a lock — would have been the first lock on
  that path and the first way to tear a buffer mid-block, to save a memcpy on a
  command that is already asynchronous.
- **A write past the end fails; a read past the end clamps.** The asymmetry is
  deliberate and is about what the caller believes afterwards. A short read
  hands back fewer samples than were asked for and says so in the reply's
  `count`, so nothing is lost. A short write would leave the caller believing it
  stored samples the server dropped, which is data loss reported as success.
- **No chunking in the protocol and no change notification.** A multi-megabyte
  edit is several messages the client sizes against the `--max-frame` ceiling
  `/server_query` advertises, symmetrically with how `/buffer_getRange` is
  chunked coming back; adding a second, command-specific chunking mechanism
  would duplicate the one the transport already has. And nothing is told that a
  buffer changed: it would be the family's first push notification, the mirror
  is authoritative, and no reader has asked for one.

**Bulk samples ride as a blob, on both sides, and that is a protocol rule.**
`/buffer_setRange` first shipped taking its samples as float arguments, which is
what the rest of the family looks like — and it was wrong by three orders of
magnitude. Encoding 200k samples as 200k typed arguments took 2.7 s in the
Python client's encoder against 0.1 ms for the same samples as one blob, because
N arguments is N type tags and N encode steps at each end; it is also wider on
the wire (5 bytes per sample against 4). So `/buffer_setRange` carries each run
as one little-endian `f32` blob and `/buffer_getRange.reply` answers with one,
which also removes the declared `count` that could disagree with what arrived.

The protocol already worked this way and the new command was the outlier:
`/bus_tapStream.reply` sends its windows as blobs and `/buffer_export` writes
the same bytes to a file. The rule is therefore written down rather than
re-derived: **a payload whose length scales with the audio is a blob; a payload
whose length scales with the parameters is typed arguments** — which is why
`/buffer_set`'s scattered indices and `/bus_getRange.reply`'s control values
stay as they are. Each client gets one function for the pack and one for the
unpack (`clausters.base.bulk`, `src/base/bulk.ts`), so the endianness check —
a `Float32Array` and an `array('f')` are host-endian, and silently wrong on a
big-endian host — has one owner instead of one per call site.

**The queue is what "the current contents" means, not the mirror.** A job that
rebuilds a buffer from what it holds (`/buffer_read`, `/buffer_setRange`)
snapshots it at *parse* time, from the network-side mirror. That is correct for
one command and wrong for a batch: a chunked write submits every chunk before
any of them completes, so each would copy the same pre-batch contents and the
last one installed would erase the rest — which is exactly what a client's
`set_samples` sends. The NRT queue is the serialization point for buffer
mutation, so it keeps its own view of what it last produced per index and a job
builds on that. It is consulted only while the submitter still has work in
flight for that index, since with nothing in flight the mirror has caught up and
is the authority — which is also what keeps a buffer installed *outside* the
queue (the embed door's `install_buffer`) from being undone by a stale entry.

That in turn is what makes the batch worth sending: the chunks go out together
and close with **one** `/server_sync`, so a long write costs one round trip
instead of one per chunk. Both clients grew a barrier that watches for `/fail`
as well as the sync reply, because a batched send otherwise drops the error
that a per-command `/done` would have raised.

What this does **not** solve is the other half of why a client cannot own its
data paths: over the shared-memory ring every peer is one client, so two of them
still fight over a `/bus_stream` subscription. That is a segment-layout change
and is recorded with the ring's own identity work.

## A ring frame says who wrote it, so one segment carries several clients

Context: everything reaching the in-process/shared-memory engine went through
one ring pair, and the server saw all of it as a single `ClientId::Ring`. But
`/bus_stream` and `/bus_tapStream` are "one subscription per client, replaced on
each call", so two peers over one segment silently took the stream from each
other. The case is not hypothetical and not rare: it is every browser page,
where the script and the GUI host both push through the in-page carrier. The
loss was also **permanent in one direction** — the host only re-subscribes when
its own widget set changes, so once a script replaced the subscription the
meters stayed frozen until a widget was added or removed.

Decision: each ring frame carries a `u32` **peer tag** beside its length —
who authored the packet on the inbound ring, who the reply is for on the
outbound one — and `ClientId::Ring` becomes `ClientId::Ring(u32)`.

- **The tag lives in the frame, not in the segment header.** That is what makes
  this cheap: no field of the header or the data plane moves, so the readers
  that pin offsets by hand (`clients/gui/src/host/shm.rs`,
  `clients/python/clausters/ipc.py`) need only their version constant bumped —
  exactly the case `shm.rs`'s own comment anticipates. The alternative
  considered, **several rings** (one pair per client), moves the layout, fixes a
  client count at boot, and gives nothing the tag does not.
- **SPSC survives, because the tag is about the packet and not the ring.** One
  producer still writes each ring; an embedder holding several clients funnels
  their sends through it and demultiplexes the replies by tag. The page already
  had that shape — every send crosses into the worklet through one `MessagePort`.
- **The embedder assigns the tags.** There is no handshake and none is needed:
  the server has to tell its clients apart, never name them. A sender that picks
  nothing is peer 0, the single client a segment always had, so every existing
  embedder keeps working unchanged.
- **Replies are addressed, and that removed a door.** A listener now hears its
  own client's replies rather than everything on the wire. Observers — a test
  asserting the host's meters stream, a debug tap — get an explicit `ANY_PEER`
  read door in the web client instead of reaching into another client's
  internals. Two page tests were reading the host's traffic by eavesdropping and
  now say so.
- **The C ABI stays single-peer.** Its one consumer is the Python client, which
  *is* one client; a second tag there would be surface nobody calls.
  `clausters_send_as` can be added additively if a C embedder ever grows a
  second client.

Versioning: the framing changed, so `ABI_VERSION` moves 6 → 7 and, by the
linkage rule, drags the SemVer breaking tier with it. Nothing else about the
segment moved.

## How many buses one subscription may list is configuration, and it is per carrier

Context: `/bus_stream` was born with a constant — at most 128 bus indices,
chosen when the only consumer was one browser canvas' meters over WebSocket and
sized by the sentence "128 pairs fit comfortably in a single frame on every
transport". The command exists for clients that cannot map the shared-memory
segment, so **only a page ever meets it**: a native GUI host reads the buses
straight out of the segment and subscribes nothing. What changed underneath is
what the page became. A browser host subscribes the union over every visible
canvas, so the set grows with the *document*: at a bus per meter, a page of
sixty-four canvases walked into the ceiling by opening widgets, and past it
every further canvas read a bus nobody streamed. The refusal was a `/fail` the
host logged and otherwise ignored, so the page went on drawing stale values.

Decision, in three parts.

- **The ceiling is boot-time configuration, like every other pool the server
  sizes** — `--max-stream-buses` / `[server] max_stream_buses`, default 4096 —
  and the in-page engine carries the same knob, because a page is a deployment
  and not a client: `maxStreamBuses` at boot reaches
  `WebServer::set_max_stream_buses`. A limit only one of the two deployments can
  set is a limit the browser is stuck with, which is exactly the position this
  started from.
- **The effective limit is per carrier, and the server reports it.** A snapshot
  is one message per period and is never split across replies, so a
  subscription its carrier cannot deliver would have every reply dropped —
  silently, on the ring. The request is therefore clamped by what the asking
  client's transport carries in one packet (its frame ceiling on TCP/WebSocket,
  the overview reply budget on UDP and the ring) and the result is appended to
  `/server_query.reply`. One number for the whole server would be a fiction:
  the same server answers a page over its ring and a native client over TCP
  with two different figures.
- **A refusal is not a subscription.** The over-large request is still refused
  whole rather than quietly truncated — a client that believes it subscribed a
  set it did not is the disease, not the cure — and the browser host now acts
  on the `/fail` instead of logging it: it forgets the subscription it thought
  it had (its own belief is what stopped it asking again), takes the ceiling
  the refusal names, and re-subscribes what fits. Before that, it asks
  `/server_query` when its leg attaches and clamps the union to the answer,
  naming in the log how many buses were left out and which knob raises them.

What was **not** done, and why: splitting the union across several
subscriptions. The protocol's rule is one per client, so splitting means
several peer tags or a second connection — carrier-dependent plumbing for
headroom a configurable ceiling gives outright.


## The port is a parameter, and that is what makes a handle worth attaching to

The server binary bound 57110 and nothing else: `--tcp [port]` and `--ws [port]`
moved their own listeners, but the UDP front — the one a client probes to find
a server at all — was a constant. So one machine ran one server, and the client
had to say so: `Server.boot` refused any handle whose address was not the
launcher's fixed one, because booting could only ever produce a server at 57110
and moving the handle to meet it would hand back a server nobody asked for.

`--port` makes it a parameter. It is the **base**: UDP binds it, TCP follows it,
WebSocket sits ten above, so one number moves the whole server and `--udp`/
`--tcp`/`--ws` only exist to pull a transport off that base deliberately. UDP is
the one that cannot be turned off, being the discovery door. In the client, the
handle's own address is passed through to the process, and the refusal shrinks
to the half that is not a limitation but a fact: booting starts a process *here*,
so a handle aimed at another machine still raises.

- **Ownership is the axis the API turns on**, and it was already implicit in
  `boot`/`close`. Making it explicit gave `attach` its shape: a verb for a server
  this handle did not start, which therefore does not stop it either — `close`
  lets go, `quit` stops it over the wire, `free_all` cuts only its sound. And
  `boot` refuses a port that already answers rather than adopting it, so
  ownership is never acquired by accident.
- **`attach` verifies and reconciles**, which a bare `Server(...)` cannot. A
  handle pointing where nobody answers used to drop every message into a UDP
  void that reports nothing; now that is an error at the verb. And since a
  server this client did not launch may have been booted with other flags, the
  handle re-reads the real capacities (`/server_query`) and sizes its allocators
  from the answer instead of from its own `options`, which for an attached server
  are a guess.
- **The stray server needed a terminal, not an API.** A client that crashes
  leaves a process holding the audio device, quite possibly still sounding, and
  `kill` was the only way out. `clausters stop|panic|status` are client-side
  words on the same console script; every server flag starts with a dash, so a
  leading word is unambiguously ours and everything else still reaches the
  binary untouched.
- **Two servers sharing a def store raced on one temp name.** Both recompile the
  persisted defs at startup and both wrote `<name>.tmp` before renaming, so the
  loser's rename hit ENOENT. The temp name now carries the pid and a counter,
  which is what makes a shared `--data-dir` a feature (a def sent to one server
  is on disk for the next) rather than a hazard. The MIDI port's default name
  follows the port for the same reason: two `clausters` ports are indistinguishable
  to whoever is patching them.

What is *not* solved here is the audio device: two servers open two streams, so
this works wherever the host mixes (PipeWire, CoreAudio) and fails on an
exclusive ALSA device, where the second boot reports it and stops.

## A binding fires an apply, never another binding

A widget's value could already leave for the audio server without the script
(`/gui_bind … "server" …`). Pointing it at **another widget** — a menu flipping a
`stack`'s page, a slider driving a plot's `max` — is what makes a persisted
GuiDef an application rather than a picture: the pages flip inside the host,
with no client attached at all. But two widgets bound to each other is a cycle
the moment an apply can trigger a delivery, and a GUI that live-locks on a click
is worse than one that cannot switch tabs.

The rule is **one hop, by construction**: what a widget binding fires is the
mutation `/gui_set` performs (`Host::set_props`, extracted for exactly this) and
nothing else. That method applies the prop, mirrors it into the registry and
asks for a repaint; it never enters the delivery path, so the target's own
binding does not fire from it. There is no cycle detector and no depth counter,
because there is no chain to walk — and the alternative, letting a binding
cascade and then policing it, would add a feature nobody asked for and a class
of hangs with it.

- **The destination keyword was already on the wire**, which is the whole reason
  this was additive: `/gui_bind id "server" …` spelled out a destination when
  there was only one. So the binding became an enum of destinations rather than
  an address with a keyword in front of it, and a widget binding carries no
  address, no prefix and no server to be missing.
- **An apply can ask for a repaint where a forward could only ask for a
  datagram**, so the delivery seam had to take a `&mut Host` and an effect sink.
  The gesture layer translates the host's effects into its own rather than
  sharing a vector: what a binding's apply may legitimately produce is a
  redraw, and anything else (a window opening behind a knob turn) is a bug worth
  seeing in the log.
- **A multi-value payload rides as its JSON string** — the scalar carrier the
  array-valued props (`points`, `notes`) already accept from `/gui_set` — so one
  curve drives another curve without a second encoding being invented for the
  binding path.

## A widget type names a container, an element or a control — nothing narrower

*(decided while landing the vocabulary migration)*

The `/gui_*` catalog had grown to twenty-nine type names over a model of three
things, and the same idea was spelled several ways: `waveform`, `plot`,
`scope`, `spectrum`, `spectrogram` and `phasescope` were six points of one
product (a presentation × a source kind × the capabilities over it); `panel`,
`box` and `stack` were one container with three arrangements; `scroll` and
`patch` one plane with and without boxes; `track`, `clip` and `timeruler` one
time/value container told apart by what is placed on it. So the wire now names
the **model**: a container owning 0, 1 or 2 axes (`window`, `layout`, `plane`,
`field`), an element drawn against them (`signal`, `notes`, `curve`, `score`,
`keys`, `nodes`, `meter`, `canvas`, `label`), and a control, which is an element
with a value and no axis (unchanged — a `knob` names what it is).

**Why the wire and not a set of presets over it.** Keeping the old names as
wire names would have cost no migration at all, and was rejected: the reference
would document twenty-nine types over a model of six concepts, and a reader
would never discover that a plot is a trace that does not navigate, because
"plot" reads as a different thing. The model's whole value is that the general
form is what a script learns; hiding it behind the names defeats the refactor.
That argument is about the **wire**. It does not reach a client's own builder
names, which is why `waveform()` and `track()` survive as shortcuts onto the
model — a shortcut re-documents nothing, and every existing call site kept
working through the migration because of them.

**The chrome belongs to the axes, not to each element.** A ruler, a navigation
window, a selection, a playhead and a value range describe the container's axes;
they were props of every view that drew against them. They now ride nested,
under `axes: {x, y}` — nested rather than bare `x`/`y` because those are already
the free-placement props, and a container that is placed *and* owns axes would
have no way to say which it meant. Inside the host they stay flat, and a def's
pair is flattened at the door: an OSC reply is flat arguments, so a structural
prop cannot be answered at all, and nesting them would have made `/gui_query`
stop reporting the ruler, the window and the range a script reads back.

**What the discriminations cost.** Collapsing names means the props have to say
which construction is meant, and three of those rules were only settled by
writing them:

- `navigable: 0` over addressable samples is the whole static-plot
  construction, not a capability switched off: a view that does not navigate
  also holds the sequence itself rather than a take (no peak pyramid) and
  auto-fits a value axis nobody named.
- A `field` with nothing on it is a **lane**, not a ruler. The first rule said
  "children, or nothing at all" — but a multitrack opens empty lanes constantly,
  so the ruler is the case that must declare itself: a bare strip of a given
  thickness `h`, nothing placed on it, no lane chrome.
- The arrangement is `flow`, on every container that has one, because the model
  spends the word `layout` on the container type itself.

**What it does not reach, on purpose.** `box` is the one name the migration
could not take: the catalog spent it on a synonym of `panel`, the model wants it
for a patcher's box, and turning a plane's `boxes` prop into child elements
changes behavior (ids, layout, per-box hit-testing and edit-back), not spelling.
The name is now unclaimed and the prop unchanged, waiting for the element rather
than for a rename.

## A frequency axis is navigated by the element, not by a group

*(decided while making `navigable` mean something over a spectrum)*

Retention had just made a live *time* axis navigable, and the acceptance line it
came with also asked for a zoomable live **spectrum** — which turned out not to
be the same mechanism at all. Everything navigable in the host navigates time: a
navigation group is keyed by `link` over a shared time axis, the group's window
is measured in samples, and joining one is what `is_timeline` answers. A
spectrum's x is **frequency**, a domain that is addressable with no retention
whatsoever — every bin is there every frame — so the capability parsed and had
nowhere to go, and the code said so honestly by dropping it.

Two shapes were open. A **second axis kind** beside the time one (a coordinate
system carrying its unit, so frequency axes group with frequency axes and never
with a lane) generalizes to a pair of spectra comparing two buses, and to an
arbitrary-domain plot later. Or the **element's own window**, no group at all,
in the shape the vertical axis already has: a normalized `[start, len]` the
element carries alone.

**It is the element's own window**, and the argument that settled it is not the
size of the diff. *Frequency is already navigated this way in this host.* The
spectrogram's frequency axis is its vertical one, and that axis is a normalized
per-element window with no group behind it — because a display axis over
`[0, Nyquist]` on a log/mel/bark scale is a display coordinate, inverted through
one shared mapping, and nothing else in a window measures in hertz to share it
with. Making the curve's frequency axis a *grouped* axis would have given one
domain two mechanisms depending on which way it happened to be drawn, and left
the reader unable to guess which. So the horizontal window is the vertical one's
sibling in every respect: same normalized units, same clamp, same read-at-use
validation, and a `"view_x"` event beside `"view_y"`.

**What it costs, if the pair of spectra ever wants a group.** Nothing that is
lost: the per-element window remains where the answer is stored, and a group
would be a layer that writes it, exactly as a timeline group writes its members'
view today. That is the reason this order is safe — the general case can still
arrive, and it will not have to undo this.

Two smaller things fell out of it, both worth stating because they are the kind
that surprise the next reader:

- **The wire keys are the x axis' own `view_start`/`view_len`** (`axes.x.start`
  / `len`) rather than a new pair. It is the same question a timeline member's
  window answers — what slice of the x axis is visible — and only the owner of
  the answer differs. Exactly one reading is live for a given widget: on a
  member of a navigation group those keys never reach the element at all, the
  group model takes them, in samples.
- **`navigable` is off by default here, and only here.** Everywhere else the
  prop defaults to on, because the views the catalog grew as editors navigate.
  A spectrum is a meter-like view that has never moved under a drag, and every
  existing `spectrum` on the wire says nothing about the capability — so
  defaulting it on would have changed what shipped defs do, to no one's
  request.


## A navigable axis stops at the resolution of what it measures

*2026-08-10, from a by-eye pass: zooming a live spectrum far enough left the
widget showing a flat line that no longer answered to the sound.*

The display axes have always been floored by a constant fraction of their own
extent (`viewport::MIN_SPAN`), which is a number about the **screen**: it says
how small a window may get before a view stops being drawable. That is the right
question for a plane you pan, and the wrong one for an axis over a **measured**
domain, which has a resolution of its own. A spectrum's x is frequency, and its
frequency is an FFT of a given size: at `fft_size` 2048 and 48 kHz a bin is
23.4 Hz, and the constant floor let a window shrink to a *fifth* of one — the
whole widget then drawing the interpolation between two neighbouring bins, a
straight line that no zoom, and no signal, will ever change.

**The floor is derived from the analysis**, not from the display: the window may
not go below the display width of a handful of bins. It cannot be a constant,
because a bin is not one — on a log axis a bin is a twentieth of the visible
axis at 500 Hz and a thousandth of it near Nyquist, so any fixed fraction is far
too coarse at the bottom and far too fine at the top. It is measured through the
very display↔hertz mapping the curve and the ruler are drawn with, which is also
what keeps the three from disagreeing.

Two consequences worth stating:

- **The floor belongs to the axis, not to the gesture.** No path can put the
  axis somewhere another path forbids: the wheel, the drag, the `R` reset and a
  script's `/gui_set` all land inside the same bound. It is applied where the
  vertical window's own clamping already lives — at the **read**, not in
  `apply` — which is also why one `/gui_set` carrying both keys cannot depend on
  their order. The *zoom* is the one place that needs it as a number beforehand:
  clamping afterwards would keep the anchor a narrower window computed, so each
  further step at the bottom would slide the picture sideways instead of
  standing still.
- **What is stored is the request; what is shown is the request opened up.** The
  floor is a function of where the window sits — at 12 kHz four bins are a
  thousandth of a log axis, at 20 Hz they are a quarter of it — so a window that
  exists up the axis cannot exist down it. Writing the opening back would make a
  *pan* spend the zoom: the way down would widen the window, and the way back up
  would arrive somewhere the reader never asked to be, one gesture no longer
  undoing itself. So the pair on the element is what was last asked for, from a
  gesture or from the wire alike, and everything that looks at the axis — the
  frame, the gesture's anchor, the `"view_x"` event — asks it for the window it
  can actually show. The two agree everywhere but at the bottom of the axis,
  which is the whole point. A corollary: a request that would show exactly what
  is already on the screen is not written down, so the wheel at the floor does
  not quietly discard the zoom the reader is still going to want back.
- **The gesture needs the sample rate**, which the fronts knew and the gesture
  context did not. It is one field on `GestureCtx`, filled from the same source
  the frame draws with — because a gesture that resolved a different hertz than
  the frame drew would anchor a zoom where the reader is not pointing.
- **A floor read off the axis is not a floor.** Because it is measured forward
  from the window's left edge, the floor is a function of where the window is —
  and a pan hands over an edge that is *off* the axis, which is exactly what
  dragging past the end means, the write clamping it a step later. Measured from
  there, the overshoot is charged to the floor: the window comes back widened by
  how far the drag went, the next step of the same drag reads that wider window
  and goes further still, and a gesture asking to move sideways rushes the
  picture out to the whole axis. The edge is clamped onto the axis before the
  bins are counted from it.

**And a gesture that moves nothing says nothing.** The same pass found the other
half: an axis pressed against a bound goes on receiving wheel notches, and every
one of them re-emitted the window it already had. The view events (`"view"`,
`"view_x"`, `"view_y"`) now report movement rather than input. The comparison
carries a small epsilon, and that is not a fudge: a bound that is itself a
function of the window's position converges to it by last bits rather than
landing on it, and a billionth of a normalized axis is a millionth of a pixel.

## The Rust builder is generic, so it never becomes a fourth widget catalog

*2026-08-10, opening the Rust door into the host (`clausters_gui::tree`).*

A program that links the crate needed a way to build a widget tree without
serializing a JSON document against itself and parsing it straight back, and the
obvious shape for it — one typed constructor per widget, `knob(min, max)`,
`meter(bus)` — is the shape the Python builder already has and the one to
refuse here.

Every widget's props are declared in **three** surfaces today (the host, the
Python builder, the web builder), and `docs/gui-props.md` plus
`clients/python/tests/test_gui_props.py` exist precisely because three surfaces
drift: the test reads all of them and fails on a divergence nobody declared. A
typed Rust mirror would be a fourth, with no test holding it and no client
asking for it — the Python client is the reference surface for the catalog by
the project's own rule, and the reader who wants a knob's props reads its page,
not a Rust signature.

The second reason is the one that makes it structural rather than a matter of
taste: a **registered element** (the `Element` trait) has props no catalog
inside this crate can know, and instantiating one is a first-class use of the
builder. So the open door — a node named by its wire type, props set by key —
has to exist whatever else is built on top of it. Once it does, a typed twin of
the catalog checks spelling rather than safety, at the cost of a surface that
rots.

What *is* typed is what a hand-written document actually loses: a prop is a Rust
value, so an integer stays an integer and a continuous value stays continuous —
the int/float distinction the host reads props by, kept by construction instead
of by remembering to type `2.0`.

## The keyboard focus can leave the tree, because a GuiDef is not always the whole screen

*2026-08-10, generalizing the host's focus from "the focused text field" to a
tab ring (`host/gestures/focus.rs`).*

Every toolkit's tab ring **wraps**: Tab off the last control returns to the
first, because the window is the world. That is true of a desktop window and
false of the host's other target, where a GuiDef is mounted **inside a
document** — a `<canvas>` among the page's own headings, links and forms. A
canvas must carry a `tabindex` to receive keys at all, and the host's front
prevents the browser's default on every key it handles, so a wrapping ring there
would be a **keyboard trap**: once Tab entered the canvas, nothing else on the
page could ever be reached again. That is a worse regression than shipping no
keyboard at all, and it would land on the page author rather than on us.

So the ring **runs out** instead of wrapping. Tab past its last stop clears the
focus and reports it to the front (`GestureEffect::FocusOut`), which is the one
thing the two platforms answer differently: the browser shell blurs its canvas
and the document's own tab order carries on from there, while a desktop window
— having nothing outside to hand the keyboard to — simply has nothing focused,
and the next Tab enters the ring again. The core decides *that* focus left; only
the shell knows what that means locally, which is the same seam every other
platform difference in this crate goes through.

**The desktop does not wrap**, and that was asked and answered rather than
overlooked: the shell could perfectly well re-enter the ring on `FocusOut`, and
every desktop toolkit does. It does not, because the platform that *cannot*
wrap sets the rule for both — a window and a canvas behave the same way, so
there is one behaviour to learn, one to document and one to test, rather than a
divergence whose only reward is saving a keystroke on one of the two targets.
The empty step is not a hole either: "nothing is focused" is a state the user
reaches anyway by clicking on empty space, and seeing the ring go is what says
the keyboard left.

Two smaller calls follow it and are recorded here so they are not reopened.
**`focus` is not a prop.** It rides on `/gui_set` because that is the wire's one
mutation verb, but it says where the keyboard is pointing — host state, not a
widget's — so it is taken out before the document is written: a `/gui_query`
does not report it, and reloading a persisted def does not restore a focus
nobody asked for. It is also not echoed: the `"focus"` event reports what the
*user* did, the way every other event on that stream does, and a `/gui_set` has
never announced itself back to the script that sent it. And **composition (IME) stays the page's**: a canvas cannot
host an input method, so the host reads the keys it is handed and never pretends
to compose them. Text that needs composing is not entered through a host field,
which is a stated limit rather than a bug to find.

## The crate embeds no typeface, because a face is licensed, heavy and already installed

A GUI host built with a rasterizer draws with a face, and the obvious way to
ship one is to embed it: `include_bytes!`, one file, nothing to configure. That
is not what happened, and the reason is worth keeping so the question is not
reopened as an oversight.

A face is **three costs at once**. It is hundreds of kilobytes in a repository
that carries no other binary asset, in a wasm bundle whose whole argument is
that a page may hold forty canvases, and in a wheel that already carries a
JIT compiler. It carries a **license of its own** — its own attribution and
redistribution terms — inside a GPL crate, which is a bookkeeping obligation for
every downstream that copies the artifact. And it answers a question the
platform has already answered: every desktop has faces installed, and every page
can fetch one from the origin it was served from.

So the face arrives through a **seam**, the fifth platform trait
(`FontSource`): a file natively (`--font <path>`, `[gui] font`, or one of the
system's own when neither names one) and fetched bytes in the page
(`GuiBridge::font`). What the feature compiles in is the *rasterizer*, not a
typeface.

**And no face is an ordinary state, not a failure.** The embedded 5x7 bitmap is
the floor this crate always draws on, so a host with the feature and nothing
loaded renders exactly what a host built without it renders — which is the same
degradation rule the protocol already has for a widget an older host does not
know. That is what makes the runtime half of the seam safe to leave optional:
there is no error path to design, no boot to block on a font, and a machine with
no fonts at all still draws every label.

## The cell is declared and the face is fitted to it, in both faces

The bitmap face landed under one rule — the layout declares a cell
(`metrics::CELL`) and the glyphs are drawn to fit it, never the reverse — and
the atlas keeps it rather than inheriting a typeface's own metrics. A scale
rasterizes at the pixel size whose **cap height** is the body box, so a capital
through the atlas is exactly as tall as the capital the bitmap drew.

The property it delivers is the one the sizing model paid for: the metrics table
is constant data resolved once per scale change, so **changing the typeface
never relayouts a window**. Loading a face mid-session is a redraw, not a
re-measure; a document laid out by a host with a face and one without is the
same document.

What legitimately *does* follow the face is the width of a **string**, and it is
asked for where a string changes — a label's text, a field's line, a wrapped
paragraph — never in a layout pass. That is the seam the bitmap face already
left when it separated its own nominal advance (a property of the face, which
the size roles that reserve room for N characters ask for) from the measurement
of a run of characters. Making that measurement proportional was the whole of
the port: the caret, the selection band, the click-to-column hit-test, the
ellipsis cut and the word wrap were all counting cells, and each of them now
measures the glyphs it is actually drawing — which is the same arithmetic under
a fixed-pitch face, and correct under either.

One consequence is deliberately visible and is the **only** place two builds of
this host differ: `text_size` quantizes to half-steps of the cell without the
feature, because a bitmap glyph is scaled by repeating its own pixels and an
uneven scale makes them ragged, and is continuous with it, because an outline is
rasterized at whatever size is asked for. A user who wants larger type still
asks for it where density lives — the `scale` key of `[gui.metrics]`, resolved
once — never by choosing a different face.

## A size may read a prop, never a value — and only where a container asked

`hug` lets a container want what it holds instead of the share the layout would give it, which reopens the question L7 closed by fiat: *may a size read the widget's content?* Its answer was "never" — a scope's height must not follow its sample count, a label's width must not follow its string — and it was right about the failure it was avoiding and too coarse about the reason.

The line that actually holds is **where the value is resolved**, not what it is. A prop settles at a *mutation point* — a `/gui_def`, a `/gui_set` — which is exactly where the theme overlay already resolves and where a relayout costs one message. A **value** streams: a bound knob writes one per pointer move, a meter one per frame, a field one per keystroke. So a menu's `options` may size it and the option it is *on* may not; a label's `text` may and a `number`'s reading may not. The failure L7 was avoiding is entirely on the value side: a window that jumps while a control is being turned, and a per-message relayout cost. Sizing to a caption has neither.

Two mechanisms keep that honest rather than a convention. The question is asked by a **different function** (`Element::hug`, beside `natural`), so the ordinary layout pass cannot read content even by accident, and every element that does not implement it answers its data-free natural size. And it is only ever called **under a container carrying `hug`** — a container that has asked for its size to follow what it holds. A def that does not use the prop lays out byte-for-byte as it did, which is why this could land without the one-time break L7 needed.

The composition itself stays inside the boundary L2 fixed and L7 restated: one bottom-up walk over functions that were already pure, no measurement pass, no constraint solver, no negotiation between a parent and a child. `None` (elastic) propagates, so a container holding a plane or a heavy view still hands the axis back rather than inventing one — a hugging container is not a promise that every subtree can be measured.

The one asymmetry between the two builds is the **window** case, and it is a platform truth rather than a fork: `hug` on a root sizes the OS window to its content, and in a page there is no window to size — the element owns its box and reports its pixels, which is the rule that keeps the host from ever reading the DOM. The composition inside is identical on both; only the outermost rectangle has a different owner.

## The paint capabilities are the ones that keep the batch — and antialiasing is not a widget's

The host's chrome is one batch of flat triangles: everything a window draws goes into one vertex list and one draw call, and that is the crate's whole performance argument. It is also why the chrome had no rounded corners, no fades and hard edges, which is right for an oscilloscope and an absolute ceiling for application chrome. Widening it therefore had one criterion, and only one: **what can we add that does not split the batch?**

Three things could. Rounded corners are *geometry* — the arcs tessellate into the same mesh, the way the score's outlines already do. Opacity is *already there* — the vertices carry RGBA, so a fade needs a prop and a resolution rule, not a pipeline. Antialiasing is *the attachment* — MSAA is a property of the render pass, so it changes no drawing code at all. What stayed out is what would split it, and is recorded so it is not re-argued: textures beyond the glyph atlas, gradients, drop shadows, and per-layer compositing.

**Both props ride the mesh, not the draw functions.** They apply to a *run* of triangles — everything one widget contributes — exactly as the clip rectangle does, so the frame sets one `Ink` per placement and no draw function grows a parameter, no element is told it is being faded, and every element written before this milestone honours both without knowing they exist. The corner radius reaches only the **box** primitive (`Mesh::rect` and the border built from it), never a line, a disc or a glyph, and each box clamps it to half its shorter side — which is what lets one number ride a whole widget: a divider, a tick and a track edge have no room for an arc and come out unchanged, while the widget's own frame rounds.

**The two inherit differently, and that is the design.** Opacity composes down the subtree like a theme group — a control at 0.5 inside a panel at 0.5 draws at 0.25 — because a fade is a statement about a *group* of widgets ("this pane is inactive"). A radius does not inherit at all: a rounded panel says nothing about the controls in it, since rounding is a statement about one box's shape. Both settle at a mutation point, never on a value, so the rule "a size may read a prop, never a value" has a sibling here: a *stream* of values can no more fade a window than it can resize one.

The fade's bound is worth stating rather than discovering: it is **per-primitive alpha, not layer compositing**. Two overlapping shapes inside a faded widget show through each other, because compositing a subtree means rendering it to a second target — which is exactly the batch we refused to split. And it fades what the mesh carries: the chrome, the controls and the text. A heavy view's picture is drawn by its own pipeline against its own uniforms and keeps its opacity; fading it would be a uniform on the waveform, the spectrogram and every user `canvas` shader, i.e. three pipelines paying for one prop.

**Antialiasing is a host setting and not a prop**, and that follows from the same fact rather than from taste: it is the pass's attachment that is multisampled, not a widget. Offering it per widget would promise something the hardware does not do at that granularity — a widget cannot have its own sample count without its own pass — so it is one number per window (`--msaa`, `[gui] msaa`, `GuiBridge.msaa`), clamped at bring-up to what the adapter reports for the surface format, falling back to 1 with a warning rather than refusing to open. The default is 1: a signal trace is drawn at the resolution of the screen on purpose, and smoothing is what a chrome-heavy document opts into.

## A waveform column is its own envelope: there is no fill, and no zoom at which one belongs

Three renderers drew a signal against time — the GPU pipeline of a navigable view, the shared mesh (a plot, a clip's take) and the live oscilloscope's own loop — and they agreed on the columns and on nothing else. The most visible disagreement: the GPU path clamped every column to include zero (`lo.min(0.0)`, `hi.max(0.0)`) and the two mesh paths did not, so a signal offset from zero was a body filled from the baseline in one view and a floating band in another of the same samples.

The first attempt closed it the other way — one shared rule, "fill when the value domain straddles zero" — and an eye pass killed it in one screenshot. At a zoom where cycles are legible a column covers two or three samples that all sit near +0.6, and filling to zero **inks a band the signal was never in**. So the question became *at which zoom does a fill belong*, and that question has no answer: a **subsonic** signal — a 1 Hz LFO, a control curve, a long envelope — has far more samples than the screen has pixels at any zoom showing a whole cycle, so every "fill once the samples stop fitting" rule fills it, and a cycle a second is precisely a curve that a fill destroys. `samples_per_px` is not the criterion, and neither is anything else about the zoom.

What separates a body from a curve is whether the signal **crosses the span inside one column** — and the min/max already answers that, measured, per column, at no cost. So the rule is that there is no rule: **a column is drawn as it is measured, everywhere.** The solid body of a zoomed-out waveform is the data filling it (audio crosses zero many times per column at overview zoom), the curve of a slow signal is the data not filling it, and the drawing never changes its mind about which it is looking at. That also removes a threshold nobody could have named and a prop nobody has to learn.

Two floors keep it legible, and they are the mesh renderer's rules applied to the pipeline that had neither. A column is inked **at least one pixel in each direction**, so a flat stretch — the sustain of an envelope, the tail of a decay — stays visible instead of collapsing to nothing. And once the zoom is deep enough that consecutive samples stand **three `point_radius` apart**, each sample is **marked with a dot**: the polyline between samples is an interpolation the drawing invents, and the dot is what says which points of it are data. The dot is sized as a curve's break-point deliberately — it is the affordance sample-level editing will take hold of, so the two read as the same kind of target the day the second one becomes draggable.

## `min`/`max` are one value domain, and the amplitude axis is its default

The same audit found the navigable trace ignoring `min`/`max` outright: its geometry was pinned to ±1 through `AMP_MARGIN` while the take in a clip, the plot and the live scope all mapped through the declared pair. One prop meaning something in four of an element's presentations and nothing in the fifth is the per-arm divergence the E track exists to remove, so the pipeline takes the domain now, `[-1, 1]` by default — which is what every view that names none draws at, byte for byte.

The consequence is on the **ruler**, and it is the one policy call here. `db`, `bits` and `percent` are units of *full scale* — a rung at -6 dB or at 2^15 says nothing over `[20, 20000]` — so the amplitude ladders belong to the default domain, and an axis whose element named a domain of its own is ruled as a plain **value** axis over the slice its window shows. Stated the other way round: **the amplitude axis *is* the full-scale domain**, not a separate mechanism, which is why the switch is a comparison against the default and needs no flag on the wire.

## A signal against time is drawn in one place, and the waveform's pipeline was the second

The navigable waveform used to build its own vertex buffer through a dedicated
`wgpu` pipeline (a triangle list for min/max columns, a line strip for the raw
polyline, one WGSL module), while every other drawing of a signal — a clip's
take, a plot's series, a meter's history — went through the shared triangle mesh
in `host::graphics::signal::trace`. Two implementations of one picture, and the
measurement that had once justified keeping them (`tests/gpu_slot_cost.rs`) said
the opposite of what it was read to say: the per-column *computation* — the
peak-pyramid reads — dominates so completely that where the vertices land is
noise. The pipeline was not paying for itself; it was only a second place to
write the same thing.

So it drifted, in the ways duplicated arithmetic drifts. The pipeline's polyline
took the sample before the window's left edge but not the one past its right
edge, so at the deepest zoom a trace arrived at a sample and **stopped dead**
where the data continued — visible the moment someone zoomed a bulk file in far
enough. It marked samples with squares where the mesh marks discs. It split the
two regimes on the opposite side of the threshold (`<=` against `<`), and floored
a column's ink at one physical pixel where the mesh floors it at the trace's
weight. Each of those is a one-line fix and none of them stays fixed.

The pipeline is gone. `WaveformData` is data (raw samples plus a peak pyramid per
channel) and `WaveformView` is the vertical navigation state of a view over it;
the picture is `trace::draw_channel` into the window's mesh, exactly like every
widget's. What was lost with it is worth naming: a waveform no longer draws
through `set_viewport`, so the `Framing` trick that cuts an off-screen lane at a
fixed size is the spectrogram's alone — the mesh's clip rectangle does that job
for triangles, and does it per widget rather than per pass.

The spectrogram keeps its pipeline, and the difference is the load-bearing one:
its picture is a **texture sampled once per pixel**, so the GPU's own filtering
is what resolves it at any zoom. That is work triangles cannot do. A waveform's
columns were folded on the CPU either way.


## A drawing that overshoots its rect is the one that bounds it

A clip masks what it holds, and that is right: a clip is a coordinate system,
and one that does not bound its contents is a rectangle they happen to start in.
But the mask was put there to stop a specific overshoot — the trace reads the
sample before its left edge and the one after its right, or the line would start
and end inside the box — and that overshoot belongs to the *trace*, not to the
clip. So it came back the moment the same drawing stood on its own: a
free-standing `waveform` is nobody's content, no container mask is in force, and
the sample discs landed on the vertical ruler beside the view.

Bounding it per container is a rule every future holder of a trace has to
re-learn. So `trace::draw_channel` narrows the mesh's clip to the lane it was
handed and restores what it found — narrowing only, so a clip's mask and a
scroll's still hold — and every destination is bounded at once, on both axes:
the vertical overflow of a value outside the amplitude window was relying on the
same absent mask.

The cost is small and worth naming: a column at either end of a rect is widened
to the trace's weight like every other, and now the lane shaves that widening
instead of letting it hang over the edge. That is the same treatment a value
just outside the vertical window gets, which is what makes it consistent rather
than a special case.

## A hit-test is the shape that was drawn, and it compares squared distances

The GUI host answered every pointer question with `Rect::contains`, because
nearly everything it draws is a rectangle or a stroke. The exceptions read as
their bounding box, which is not a small error: the corner of a square around a
circle is a quarter of the box that never belonged to the shape, and at the
radius a knob's dial or a notehead is actually drawn at, that quarter is most of
where the pointer tends to be. A knob's cell holds a label strip over the dial
over a read-out, so a press on the name or the number turned the value; a
notehead's box, on a dense page, overlaps the stem, the beam and the note a
line away.

`host::graphics::shape` holds the round shapes — the disc, the ellipse
inscribed in a box, and the squared distance both are tested with. It is one
module rather than a helper per model because the round things are spread
across the catalog and each had either reinvented the arithmetic or skipped it.

**Squared, never rooted.** A distance in a hit-test is compared and never
reported: against a radius, and against the best candidate so far. Squaring
preserves the order of non-negative numbers, so both comparisons answer off
`dx² + dy²` and the square root is work with no reader. The ellipse is the same
test with each axis divided by its own radius before squaring.

The rule is wider than the circles that exposed it: **a hit-test is the shape
that was drawn**. A slider's track area is its cell minus the label and the
read-out, and what is drawn in it is a groove a few pixels thick with a short
grip riding it; a toggle stretched across a row is a small box with a word
beside it and a great deal of air after that. Both were acting on presses that
landed on blank space the layout had left around them.

So the filter belongs to the **trait**, not to each leaf. `Element::hit_area`
returns the shape the element answers on — the placement rectangle by default,
a disc or an ellipse otherwise — and `gestures::element::press` is the one place
it is applied, adding the metrics' hit slop so no element decides for itself how
much air a small target deserves. A point off the shape reads exactly as a
decline, so the press falls back to the chain like any other. Declared rather
than tested is what keeps it general: adding a leaf drawn smaller than its cell
is one method, not three lines every leaf must remember, and a leaf that says
nothing keeps the rectangle it always had.

What that leaves is a pass rather than a design: each remaining light widget
states its drawn shape, and each is checked by eye on the example that shows it.
Two questions belong to that pass and not to this record — whether hover and the
wheel should read the same shape as the press (they still read the placement
rectangle), and what a hugging container should do when its child answers on
less than it was given.

The one route that skips the filter is the **overlay**: a modal is offered the
press because it is outside as often as because it is inside — clicking the
window is how a menu's list closes — so "is this widget's drawing under the
pointer" is the tree's question, not a modal's.

Two consequences worth stating, because they are what makes the pattern
generalize. The shape is carried **per hit entry**, not decided at the test: the
score's index measures extents, so everything arrives as a box, and only the
indexer knows that this one was measured around a notehead (a codepoint in
SMuFL's Noteheads range) rather than around a beam. And an element whose press
lands outside its own shape **declines**, which hands the press back to the
chain — the pixels beside a dial go on meaning whatever the window means by
them, rather than becoming dead space around every round control.

## One document, and sources nothing overwrites

Three parties touch a composition's data and each is good at a different thing.
The **GUI host** has the hand: it draws, it hit-tests, it knows where the
pointer is. The **client** has the algorithms: it generates, it iterates, it
scripts. The **server** has the samples: buffers, and the audio thread that
reads them. Giving any two of them a copy of the model is how the three drift,
and the drift is silent — a note springs back, a clip lands half a grid step
from where it was dropped, an undo writes a state nobody was ever in.

MVC does not settle this, and saying so is the useful part: the pattern assumes
the view *displays*, and every one of these views **edits**. Once a view edits,
"the model" stops being one thing and starts being whichever copy the last
gesture happened to touch.

What settles it is splitting four layers that were being called one:

- **Sources** — the samples. Never overwritten, ever: not the user's file, not
  the session's own. A destructive edit is not an exception but the clearest
  case, because it writes a *temporary* source of its own and the composition
  takes the result only when the edit is confirmed.
- **The document** — the description of what plays when. This is the only thing
  that is "the model", and calling anything else that is what disfigures the
  pattern. Undo lives here, beside the data it inverts.
- **Presentation** — waveforms, peak pyramids, engraved pages, spectrograms. All
  derived, all **invalidated rather than synchronized**: a derived thing that is
  kept in step is a second model with extra steps.
- **Screen state** — zoom, scroll, a selection in flight, a pending overlay.
  Never persisted, never logged.

This is what every audio editor already does and none of them writes down as
such: a DAW's regions reference source files nobody rewrites, and a destructive
sample editor makes the source cheap to replace rather than making the edit
destructive in place. Our own wire had already stated half of it —
`/buffer_setRange` keeps a buffer's shape and fails rather than clamping past
the end — without anyone noticing it was a rule about ownership.

The document is a **Rust crate** (`crates/clausters-document`) rather than a
module of the Python client, and the forcing argument is not parity but the
`standalone` host: it edits with no language attached, so either the model
exists somewhere it can reach or there are two implementations of it in two
languages. The clients then **round-trip the format** rather than holding
handles into a Rust object graph — one function across the ABI instead of an
accessor per field of a tree — and what makes that safe is the crate's central
discipline: it is the only thing that applies an edit. A client does not apply
and then report; it hands over the document and the intent and receives the new
document plus the outcome.

## An intent states the whole value, so absence is a value

The rule the document is built on has an edge nothing had written down, and
three independent implementations got it wrong in the same way. An intent is
**absolute**: it states the value the edit results in. A `place` therefore
states the *whole* placement, and a placement that carries no `dur` is a
placement with **no length** — the member takes the element's own. That is not
a shorthand for "leave the length as it is", and the difference only becomes
visible in an inverse: the undo of the *first* resize of a clip has no `dur` to
carry, because before that resize there was none.

Every projection of an intent onto instantiated data read that absence as
"unchanged", because that is what the convenience method underneath does —
`Aggregate.move(member, offset, dur=None)` leaves the length alone, and so did
the host's own adoption (`if let Some(dur) = dur`). The result was a document
that stepped back correctly while the picture and the objects the script holds
kept the size the hand had given them: `undo` answers *true*, the log is right,
and the clip does not move. It reads as a dead button, and it was reported as
one.

So the rule is stated as a rule, and it holds wherever an intent is written
onto something instantiated — the arrangement objects a client holds, the
widgets a host draws, a leaf's configuration:

- **A field the intent does not carry is that field's absence, and absence is
  written.** A `place` with no `dur` writes *no length*; a member whose node
  carries no `config` is a leaf configured as it was made, so the empty table is
  written rather than skipped (an undone trim that skipped it left the window
  over the samples where the trim had put it, under a clip that had gone back to
  its old size).
- **What a picture shows is one rule, in one place.** The length a clip is drawn
  at — the placement's, else the element's own, else the samples', else a beat —
  is asked by the draw *and* by every path that puts a placement back. Two
  implementations of it is how a picture and a model come to disagree without
  anything failing.
- **An intent over an aggregate moves every member it states.** A trim, a split
  and a join are one `setmembers` over a lane, and answering with only the lane's
  own widget left each clip drawn as the hand had left it.

The samples are the same rule seen from further down, and they already followed
it: a destructive edit's inverse carries the values it painted over, and the
host replays them onto the buffer it drew from. That is why an undone stroke has
always come back while an undone resize did not — not because samples are
special, but because a payload of values has no way to express "unchanged" and a
placement did.

## A placement is a prop; a widget that was not there is a redefine

The acknowledgement can carry any *value* a widget already has — a placement, a
length, a curve, a note list, the window over a take's samples — and that is the
cheap channel: the host adopts it without rebuilding anything and without losing
what it has in flight. It cannot carry a widget. So a **structural** edit — a
split, a join, a cut, a paste, a cord drawn in a patcher: anything that changes
*which members exist* — has exactly one channel, and it is a whole-tree
`/gui_def`.

Who owes it is the editor that drew the window. The host emits the verb and
holds nothing ("it answers with the tree that now stands, exactly as it answers
a drag"), so an owner that only applied the intent left the document and the
objects the script holds with two clips while the picture had one, until
something happened to redraw. Nothing did: no example calls `update()`, and
none should have to.

The rule is therefore *both* halves, and the second is what keeps it honest:

- **A structural edit redefines**, forward and backward alike. An undo of a
  split takes a clip away, which is as unsayable in props as adding one.
- **A placement edit does not.** A redefine rebuilds every widget and drops what
  the host had in flight, which is exactly wrong for a drag and would turn every
  frame of one into a new tree.

Two things this pulled in with it, both of which had been quietly wrong:

**A redefine moves the version, and the document has to move with it.** The
crate refuses an edit whose `against` version is not the document's — ahead of
it as loudly as behind, since the two would then not be talking about the same
piece — so a version bumped on the client's side alone answered every gesture
after a redefine with a refusal nobody asked for, and with no reason to show:
the clip simply stopped moving. The redefine now re-derives the document at the
version it is drawing, which is what `refresh` already did for the case where a
script edits the tree.

**The node index is added to, not replaced.** An element that has left the tree
is still named by the inverses in the log, and putting it back means placing
*that object* again — a rebuilt one is a different identity to every widget and
every pending edit. The Python client kept stale entries and worked; the port
cleared the map on each re-derivation and lost the restore, so an undo of a cut
and a redo of a split quietly did nothing. One of those was a rule and the other
was a habit, and finding out which is the whole of what a port is for.

## The acknowledgement is a stamped state push, not a reply code

A GUI host holds no data, so every edit it produces is a **proposal**. Between
the gesture and the answer there is a gap the host has to draw across, and the
shape of the answer is what decides how much machinery that costs.

The answer is: the owner pushes the state that now holds, stamped with the
sequence number of the last edit it processed. That collapses three outcomes
into one message — *applied verbatim* (the value equals what the host drew),
*applied transformed* (the value is the effective one, post-snap or post-clamp)
and *refused* (the value is the previous one, unchanged). The host needs no
branch for them: its whole rule is **drop every pending edit at or below the
stamp, and adopt what arrived**. A refusal needs no error path either, because
"the state after your gesture is the state you already had" is a state push like
any other. It is Ardour's `rdiff()` seen over a wire: what reports the change is
the model, never the hand.

Three things follow, and each was a decision.

**The acknowledgement is sent always**, including when nothing changed. Silence
is not a refusal; it is a hang.

**It is a verb (`/gui_ack`) rather than a widget property.** The project's rule
against new `/gui_*` addresses exists to stop a widget or a prop from becoming
an address, and its escape hatch is a new payload on `/gui_event` — which is the
*host-to-client* direction. This is the other one, and it is the reply
`/gui_event` never had. It cannot be a property because it is scoped to the
conversation rather than to the tree: the sequence is per client, so two clients
driving one window would collide on a single prop, and it does not round-trip,
which a property has to.

**Intents are absolute, which is what makes replay unnecessary.** An edit states
the resulting value (`"note" id pitch`), never the increment. A pending edit
simply stays drawn over whatever authoritative state arrives, with nothing to
recompute, and a resend over a lossy leg is harmless. The rejected alternative
is worse than it looks: relative intents must be rebased against a corrected
state, and rebasing is exactly the netcode replay that would require the host to
hold an executable copy of the document.

**The answers lag, and staleness is measured against a floor rather than
against the lag.** The host stamps every event with the version it was last
*told*, and it is told only when an acknowledgement arrives — so on any carrier
an event naming a version the owner has already moved past is the ordinary case,
not a collision. The page's is the extreme: the host's outbox is drained on a
33 ms interval and the answer goes back through the event loop's proxy, so a
round trip is two queues wide and a hand crosses it easily. Making the ack path
same-turn is not available on either platform (native is a socket, the page is a
queue in each direction), and the alternative — telling a host that is behind to
stop emitting — trades a refused edit for a stalled hand, which is the wrong way
round for a gesture that has to track the pointer. So the mechanism is built to
be correct with the answers arbitrarily late: the owner keeps a **floor**, the
version at which the document last moved by a route no event produced (a script,
a second editor, a re-derivation, a history step), and refuses only an edit
naming something below it. Every other older version is one of the owner's own
answers in flight. The earlier rule — *a run of edits from one widget is one
gesture* — was this same insight scoped too narrowly: it saved the drag it was
written for and still refused the next gesture whenever two began inside one
round trip.

What this replaces is worth recording, because it is the honest reason the
acknowledgement never seemed necessary. Consistency was being maintained by
**duplicating the owner's rule in the view**: the lane's snap grid travels in
the GuiDef and the same snap is implemented on both sides, so the round trip
closed because both sides did the same arithmetic. That works for a grid and
generalizes to nothing — a bus allocation, a normalize, a user-written function
cannot be shipped down — which is why the places it already failed were so
quiet.

## A selection's second axis belongs to the view that measures it

A marquee swept with height over a waveform selects a **band of amplitudes** as
well as a span of time. The span goes to the navigation group, where selections
have always lived; the band does not, and where it goes is the decision.

**The group is one axis, and it is the time one.** What makes a linked view
linked is that a waveform lane and a spectrogram lane show the same stretch of
the same sound — one window, one selection, one playhead. Vertically they show
nothing in common: one measures amplitude over a domain the element declared,
the other frequency over the Nyquist, a piano roll semitones. A range held in
the group would therefore have to mean four things at once, and the first
concrete consequence is absurd — a sweep between −0.5 and 0.25 on a waveform
would restrict the spectrogram beside it to a quarter of a hertz.

So the range lives on the **widget**, beside its vertical window, and the event
that reports it already names which widget it came from. That is the same split
the piano roll made when its marquee was built: the time span left as the
group's, the notes it enclosed stayed the roll's own state.

**Rounding follows what the axis measures, not what the gesture did.** The time
axis snaps to whole samples because a selection between two samples holds no
data — it can be neither played nor cut — and the snap takes the samples the
sweep *passed over* rather than the nearest, so a sample joins when the cursor
reaches it. A value axis has no such grid: every height inside a lane is a value
the signal can take, so the range is what the hand drew, ordered and clamped to
the element's domain. The rule that covers both is *snap to the axis' quantum
where it has one, clamp to its domain always* — and the quantum is not
hypothetical, because a roll's pitch axis has one: its band is the whole
semitones the sweep covered, both ends included.

**A spectral view's second axis is not this one.** Frequency bins are a separate
field of the selection, deliberately, because an operation that understands a
band of values need not understand a region of frames × bins — and a band drawn
in hertz that some reader took for amplitudes is exactly the confusion the
separate fields prevent. A time-frequency view therefore reports no value range
at all rather than reporting its frequencies as values.

The wire consequence is small and worth stating: the two numbers are **appended**
to the `"selection"` event and sent only when there are two, so a script reading
the two-number form it has always had keeps working, and an empty or inverted
pair means *no restriction* — the same convention a non-positive length already
uses on the time axis.

**And it is a gesture step, not what a plain drag does.** This was shipped the
other way round for exactly one afternoon, and an eye pass killed it: with the
band on the plain drag, sweeping a waveform came back as a rectangle while the
spectrogram below it — the picture that actually has two measured axes — came
back as a stripe, which reads as the two views having been swapped. Nothing was
swapped. A drag over a waveform means *this stretch of time* in every editor
there has ever been, and what a band of amplitudes is *for* — gate this range,
copy only these peaks — is the script's business rather than the host's guess.

So `select` sweeps the span and `select_box` sweeps the rectangle, and the
second **declines** where the picture measures only time: one binding
(`"select_box select"`) draws a rectangle where there is one to draw and the
plain span where there is not, and it keeps working unchanged the day a
spectrogram answers it with a range of bins. That a spectrogram wants *both*
selections — the temporal sweep and the spectral rectangle — is the case a
two-step plan per modifier already expresses, which is what makes this the
right shape rather than a workaround for the missing half.

The general rule it leaves behind is worth more than the fix: **a capability
whose sibling does not exist yet must not be the default gesture.** Until the
pair is complete, the half that shipped reads as belonging to the other view.

## A paste carries the clipboard, because the clipboard is the host's

The host owns no data, so a cut and a paste leave it as requests for whoever
does. That much follows from the premise. What does not follow, and had to be
decided, is **where the thing being pasted comes from**.

The alternative is that the owner keeps the clipboard: the host reports that a
copy happened and the owner remembers what was copied. It is tempting because it
keeps the wire small, and it is wrong for the reason the clipboard exists. A
clipboard is host-wide so that a block copied in one window pastes into another
— against a different owner, a different def, or a process that never saw the
copy. An owner-side clipboard makes that case impossible while looking like it
works in the single-window one, which is the worst shape a design can have.

So the paste event carries the whole typed document plus one blob per bulk
payload it names. The framing is the one `/gui_def` already uses — JSON as the
payload, OSC as the framing, bulk beside it rather than base64 inside it — and
it is also the **only** path a browser has, which settles the choice between
this and writing the block to a mapped file: one path both fronts share beats
two that drift. A mapped file remains available the day a clipboard is large
enough to want one; nothing about the event's shape would change.

**Copy stays on the host's side of the line**, and that is not an exception to
the premise but the premise read carefully: a copy is a *read*, and the host
already holds what it draws. What it may honestly copy is exactly what it has
mapped, which is why a source it cannot read — a peak overview with no samples
behind it, a live view with no addressable past — refuses out loud instead of
handing over a block of silence. The refusal is a payload of its own
(`"refused" verb reason`) for the reason every other refusal here carries one: a
key that silently does nothing teaches that it sometimes does not work.

**The rate travels and nothing resamples it.** That is the crate's rule for the
clipboard and it holds at both ends of the wire: resampling is an edit, an edit
is something an owner performs and logs, and a paste that quietly converted
would change data nobody asked it to change in a step nothing records.

## The pyramid stores energy, and a cache that never measured it says so

The peak cache grew a third statistic beside each bucket's min and max, and two
of the three decisions it forced are about honesty rather than about DSP.

**It is stored as mean square, never as RMS.** The reason is that a level of the
pyramid is *built from the level below it*, and mean squares combine while roots
do not: the energy of two buckets is the mean of their energies weighted by the
samples each holds, so a bucket at any level equals the direct mean square of
its samples exactly, and a renderer cross-fading between two adjacent levels
blends two numbers that mean the same thing. Storing the root instead would make
every level above the first an approximation of the one below it, and the error
would be largest exactly where a view is zoomed out and reading the top. The
square root costs one operation at the moment of display, which is where it
belongs — and it is also what lets the same array answer a question that is not
an amplitude at all.

**The weights are derived, not stored.** A bucket holds `bucket` samples except
at the ragged tail, where it holds the remainder — and that count is a function
of the buffer's length, the bucket size and the index, so combining exactly
needs no count array and the cache does not grow a fourth plane. The one place
it bites is the odd sibling: min and max may read the last entry twice (both are
idempotent), while an energy averaged with itself would weigh the tail as if it
held twice its samples, so the sibling is an option there rather than a clamped
index.

**A cache that predates the measure reports its absence instead of zeros.** The
format went to CLPK v3 and the two older layouts still parse, which leaves a
pyramid that has min/max and no energy. Filling that gap with zeros would be a
measurement — silence — and a measured layer drawn from it would be a flat line
across samples that is not flat. So the measure is an option that rides with
the data, `Pyramid::has_mean_square` reports it, and a view's honest answer to an
old cache is to draw no measured layer at all rather than a wrong one.

**And a mono cache is now one channel rather than a second format.** v1 was
mono, v2 added the channel count, and v3 could have been "v2 plus energy" with
the mono layout carried alongside — two writers, two sizers and two readers for
one picture. Instead both writers emit v3 and a mono cache is the one-channel
case of it, so `cache_size` is `multi_cache_size` at one channel and the layouts
cannot drift apart. Reading is where the leniency stays: v1 and v2 still load,
and the mono reader takes a one-channel v3 cache while **refusing** a wider one
rather than silently narrowing it to its first channel.

## A measure is a factor of the signal element, and a picture may carry several

The RMS body an editor draws inside a waveform's peaks could have been a widget
of its own. It is not: `measure` says what the element's columns measure —
`peak`, the min/max envelope, or `rms`, the symmetric body at the level the
signal held — and the classic editor picture is one view naming **both**
(`measure: "peak rms"`), drawn by the one renderer placed once per measure.

**That generality is what the signal element was factored for.** The
element is already a presentation × a source × its capabilities; a measure is
one more factor of that product, so it costs no name in the catalog and it
means the same thing everywhere the element goes — over a file it is an offline
reading, inside a clip a second body, over a bus a live one.

**The stack is inside the element because a picture owns its field.** This
first shipped as a *placement*: two signal elements handed one rectangle by a
lane, neither knowing the other was there, which is composition at its
cheapest and reads beautifully. It does not work. Every view of a signal paints
its field before it draws anything — a heavy view's `view_field`, a plot's
`track`, both opaque — so of two pictures on one rectangle the second is not a
layer over the first, it is a lid on it. The tests that let it through were
looking at the wrong things: one asserted the two placements had equal rects
(true, and irrelevant), another that the mesh grew (true of a covered drawing).
Making the fields translucent would have traded one picture's contrast for
another's, and each layer would still have brought its own ruler, gutter,
selection and upload of the same samples. So the layering is a **set on the
element** — one body, one axis, one ruler, one selection, one playhead, one
upload — and the order is the type's rather than the composition's: the
envelope is the outer shape, so it goes under, whatever order the names arrive
in.

**The body is drawn where the envelope is and nowhere else.** The measure stays
exact at any span; what a short span stops being is *informative*. Below a
cycle, root-mean-square and peak converge — measured on the example's own bounce
at 88 samples a column, the ratio runs 0.6 to 0.9, so the body simply retraces
the envelope — and on the way there it reads the wave's **phase**, beating
against the period in a lattice nobody can interpret. Editors answer this the
same way: Audacity's RMS "will disappear" as you zoom in, "because there are not
enough samples to provide a meaningful average in the region being displayed".

**The window is fixed, and what ends the body is the envelope** (settled
2026-08-19, by eye, after three answers that were not). Two rules, one picture:

- **A level is averaged over a fixed 50 ms of the source** (`BODY_WINDOW_SECS`;
  2400 samples at 48 kHz), reaching out around a pixel column's centre wherever
  the column is narrower than that. A root-mean-square is an average over a
  *duration*, so a window that followed the column would make the body's own
  values follow the **zoom** — the level moving over samples that did not
  change, which is the defect that decided this. 50 ms is WaveLab's default RMS
  window (adjustable to 999 ms); it sits below the ear's own integration —
  energy integrates over something like 200 ms, a VU meter's window is 300 ms —
  so it is the floor of what still reads as a level rather than the window a
  meter would pick. A caller with no rate (a live bus window, already measured
  in milliseconds) averages each column's own span, having no zoom of its own to
  be wrong about.
- **The body goes when the envelope has come down onto it** (`BODY_MERGE_RATIO`
  = 0.8, over the span on screen, weighted by the peak so the loud part decides).
  The envelope *does* narrow with the zoom, since a column covers less of the
  wave; once it is within a fifth of the level there are no longer two readings.
  This is also what keeps a fixed-window level from ever poking **outside** the
  envelope, which is the one thing a body must never do: one weight throughout
  and then a cut, the manual's own word again.

**Three answers were tried first, and each is recorded because each was wrong in
its own way.** The original ramped **opacity linearly in samples-per-pixel** down
to `RMS_FLOOR = 256` samples a column: the floor was the pyramid's summarizing
bucket and the order of a cycle of the lowest musical pitches — reasoning, not
evidence — and the ramp was measured against nothing, so the body's weight
tracked the *zoom* and a body at a third of its weight could be read as a quiet
passage. The second cut it where the level **met** the envelope but kept
averaging the column, which is measured and does track the signal, yet still let
the level's values move under a zoom — and watching them climb toward the peaks
is itself the artefact. The third made the threshold a **duration of the column**
(50 ms, then 10 ms): right about what the quantity is made of, and an order of
magnitude coarser than a working zoom — six seconds across a window is under
seven milliseconds a column, so any floor that reads as perceptual removes the
body from the place it is looked at. Only the fourth separates the two roles the
duration was being asked to play at once: it is the **averaging window**, and the
thing that ends the layer is the envelope.

**A source that cannot measure draws nothing.** A peak cache written before the
format carried the mean square has an envelope and no energy, and the layer is
absent rather than flat: zeros are a measurement — silence — and drawing them
over samples that is not silent is the one picture worse than no picture.

## A buffer's samples are one thing, and every picture of it takes the same write

A destructive edit in a standalone editor has to land in two places at once: the
server's buffer — the copy that sounds, and the one a save writes — and whatever
the window is drawing, because refetching a take to show a stroke that is
already on screen would be a round trip per gesture.

**The picture and the samples are one thing on purpose.** A session's take is
read into a **server buffer** rather than mapped as a file (see the session
sources note): the editing verbs address a buffer, so a host that drew a file
and edited a buffer would show one copy and write another. The same argument
carries one level down, inside the host.

**A take is drawn more than once, and the views hold it in different forms.** A
session window shows the clip in its lane *and* the take's editor under the
tracks — two widgets, one samples. Worse, they do not hold it alike: a
navigable view keeps a **peak pyramid** (a take is minutes of audio and reaches
the host summarized), while a clip's take body keeps the **samples** it was
handed. A host that patched the pyramid of the widget under the pointer
therefore left the clip drawing samples that no longer existed anywhere, and the
undo — which addresses the node, not the widget the hand happened to be over —
reported that it could not restore samples it was looking straight at.

So the write is split the way the knowledge is:

- **The element writes its own samples** (`Element::write_samples`), because
  only it knows which form it is in — it patches the pyramid, the samples, or
  both, recomputing only the columns over the span
  (`peaks::update_range`).
- **The host decides which elements**, and the answer is the **buffer number**:
  two widgets are two pictures of one samples exactly when they name the same
  buffer. Nothing else in the tree relates them — a clip and an editor of the
  same take are not parent and child, and they are not even in the same lane.

Both are replaced rather than mutated: the pyramid is shared with whatever slot
is drawing it (one `Arc`, so keeping it costs a pointer), and patching it in
place would rewrite a picture under a renderer that never asked. The element
marks its slot dirty instead, and the next frame refills it — keeping the view's
navigation, since where the eye is is not part of what was edited.

**Amended for the refresh, once the sharing was measured: the holder lets go
first.** Replacing means the element clones the pyramid before it patches it,
and that clone is proportional to the **take** while the patch is proportional
to the **span**. A stroke can afford it — one gesture, one copy — but a picture
that *follows* a recording re-summarizes on every step, so it paid for the whole
take each time and only a coarse cadence made that affordable. What the sharing
defends against is a **borrow**, not a race: the front is single-threaded and
the only other holder is the slot. So the leg that follows the frontier has the
view **give the samples back before the write** (`WaveformView::release_data`;
`WaveformData::nothing` is what a released view holds) and take it again on the
next fill, which a repaint runs first. In between the element is the sole owner
and `Arc::make_mut` hands it `&mut` with no copy at all — and where a slot *is*
still holding it, the same call copies first, which is the old rule unchanged.
The invariant above is what makes this safe rather than a shortcut around it:
the write leaves the slot dirty, and nothing draws from a released view, so no
renderer is ever patched under.

**One channel of a multichannel take needed a command that did not exist.**
`/buffer_setRange` writes a contiguous run of *flat, interleaved* samples, so
one channel of a stereo buffer is a **strided** write. Sending it as N
single-sample messages is not the answer; the command is — so the host refused
with that sentence rather than half-doing it, and the command arrived:
`/buffer_setRangeChannel` (and `/buffer_setChannel`), whose positions are frames
of one channel. Inside the server it is not a second write path but a **stride**
on the one that was already there. The edit vocabulary took the same coordinate:
`Intent::WriteSamples` carries a `channel`, which is what makes an undo put back
the channel it took rather than assuming the first.

**The lesson generalizes past the write.** Once a stereo take could be edited,
three other places turned out to have assumed one channel — the clip's body drew
only the first, the monitor played only the first, and a drag that crossed into
another lane read *that* lane's value. None of them was the write path, and all
three were invisible while the samples were mono. A channel count is not a
detail of the data: it is a fact every view, every gesture and every voice has
to carry, and the way to find out who forgot it is to open something wider than
two.

## A stroke writes what the reader can see

The pencil is already refused where a pixel is more than one sample — a stroke
there would write values nobody can check, and the refusal is visible
(`"refused" "draw" …`) because a pencil that sometimes silently does nothing
teaches that it sometimes does not work.

The same rule decides what happens when the hand keeps going: a drag holds the
pointer, so it goes on reporting past the edge of the view and past the window
itself. Following it would rewrite samples nobody is looking at, and the damage
is discovered only by scrolling there afterwards. So the stroke is **clamped to
the body it started on** — the last visible column still follows the hand, and
the stroke ends where the eye does — and clamped again to the buffer's last
frame, since the right edge of a fully zoomed-out view maps to *one past* the
last sample and a stroke carrying that frame is refused whole by the owner.

## A clock is not a position, and a reader follows the transport rather than carrying one

The transport could say whether a piece was rolling and not where it was. `/transport_locate` moved a number the network thread kept and sent the audio side nothing; the counter the shared segment published — the "transport clock" — counts samples **elapsed** under the transport, so it is monotonic by construction and a locate cannot move it, while the reference called it the time of the piece. Nothing in a graph could read where the piece was at all.

**The two are different quantities and both are needed.** A clock counts what has happened and only goes forward; a position says where you are and moves wherever a locate puts it. The transport scheduler's queue needs the first — "due" only means anything on an axis that cannot jump — and a playhead needs the second. So the position was added *beside* the clock rather than replacing its meaning: `PiecePosition` joins `DeviceSample` and `TransportSample` in `server::clock_axis`, a type for the same reason those are (three `u64`s that mean different things get swapped eventually, and a swapped axis does not fail, it plays audio in the wrong place), and the segment publishes both. `/sched_atTransport` is therefore untouched.

**The position is anchored, not accumulated.** `PositionAnchor` holds the position a locate put the transport at and the transport sample that locate landed on, so a read is one add and a seek is one store — the per-sample path costs nothing, and there is no second counter to keep in step with the first. A loop is the same store: the engine cuts its block at the wrap exactly as it already cuts at a scheduled bundle, so the position advances by exactly one per sample inside every slice.

**That is what lets a reader *follow* the transport.** `TransportPos` is a UGen whose output is the position, and a `BufRd` driven by it seeks when the transport seeks, loops when it loops and holds when it stops — with nothing sent per pass and no position of its own to reconcile. Two consequences worth stating. It is why locating never has to reach into a node, which had read as a limitation ("a locate moves the position, never the state of a node") and is really the other half of the design: a follower has no state to move, and a *generator* has no position because its position is its state. And it is the shape a multitrack needs — many readers, one time — where a reader carrying its own position needs every one of them kept in step by whoever owns the clip list.

The other shape stays available and is not deprecated: `PlayBuf` from its own frame 0, `BufRd` over any phase at all. A one-shot fired from a pattern has no business consulting a transport.

**Two costs, recorded rather than discovered.** The position spends the last of the segment header's reserved space, so the next counter added there moves offsets that out-of-process readers pin by hand. And a signal is `f32`, which counts single frames exactly only to 2²⁴ — about 5.8 minutes at 48 kHz — so `TransportPos` takes an `offset` and subtracts it in `f64` internally: a clip reads its own samples from frame 0 however deep into a long piece it sits, where a graph subtracting afterwards with a `Sub` has already lost what it was trying to keep.

## Every buffer is writable, because the alternative promised something the compiler could not keep

A pool buffer used to be immutable: built once, handed to the audio thread through an `Arc`, replaced whole by whichever `/buffer_*` command changed it. That is what made the read path free — a flat `&[f32]` the optimizer can vectorise and hoist out of a loop — and it is why the write-side UGens did not exist. `RecordBuf`, `BufWr` and the whole `BufDelay*` family need one thing the design refused: a buffer two nodes can hold at once, one of them writing.

**The shape first planned was a writable *kind*.** A buffer would declare at allocation whether it could be written, keeping flat storage and today's speed when it could not, and the four per-block readers that take a whole slice (`Osc`, `VOsc`, `Shaper`, `Conv`) would **refuse a writable buffer by name at build**, since a recording is not a wavetable.

**That refusal was not available.** A `bufnum` is a **runtime control**, not a static field — `Osc`'s buffer arrives as an input and can be changed with a `/node_set` between two blocks — so there is no build at which a compiler could see which buffer a reader will read. What a kind could actually have done is fail at instantiation, or read silence, and reading silence from a buffer that has samples in it is a worse answer than reading the samples. The promise was the load-bearing half of the design, and it could not be kept.

**So the kind was dropped and the guarantee moved into the storage.** Samples are `AtomicU32` holding `f32` bits, read with relaxed loads through one door and written through `set_sample` on `&self`. Shape — frames, channels, sample rate — is settled at allocation and never changes, which is the invariant that actually matters: a reader's bounds cannot move under it. Contents can change under anything, at any time, from a UGen or a command. What that guarantees is **per-element atomicity and no ordering between elements**: a reader crossing a writer sees some old samples and some new, never half of one. That is scsynth's own semantics for exactly this case, and it is what a looper crossing its own write head has always sounded like.

**The cost was measured before the choice, not defended after it.** An interpolated random read of 64 frames (`PlayBuf`, `BufRd`) goes 145 → 150 ns, **+5%** — 12 ns per block per reader, about a thousandth of a percent of a 64-frame block's budget at 48 kHz — and **+0%** on the other three shapes that matter (sequential reads, a wavetable hot in cache, the convolver's kernel), where the loads still vectorise. The atomics are what make it **legal to write while another thread reads** — without them a concurrent write is a data race and therefore undefined behaviour, whatever the generated code happens to do — and what they cost is optimizer freedom, not an instruction.

**The consequence is a smaller surface, not a bigger one.** No kind, no flag on `/buffer_alloc`, no branch in any reader, no second storage layout to keep working, and no refusal to explain in the reference. One sentence replaces all of it: a buffer's contents are mutable and only its shape is fixed. The one thing genuinely given up is the flat `data() -> &[f32]` accessor, which is why every reader in the tree had to be visited once — and the golden renders say they came back bit-identical.

## An id names one node, and a repeated one is refused only when the two differ

A node id is minted by whoever writes a document and every intent addresses a
node by it. Two nodes carrying one id is therefore not a cosmetic defect: the
crate's lookup applies the intent to the first match while the client that sent
it keeps the last in its own index, so one gesture writes two places and the
thing the hand moved springs back. It was reachable by ordinary authoring, from
a direction nobody had looked at — ids are stamped on the element object and
numbering starts at 1 for every root, so two arrangements built in one script
both hold 1, 2, 3, and samples authored in one and used in the other arrives
carrying a number a different element there already holds.

**The fix is in two places because the failure has two sources.** The Python
bridge cannot produce it any more: the conversion **claims** each id for the
object it first meets carrying it, and an object that turns up with an id
already claimed by another is renumbered past everything in the tree. The crate
refuses it at the door — checked on **deserialization**, which is the one point
every writer passes through, so a client, a host and a file all get the same
answer without any of them remembering to ask.

**What is deliberately not refused is one element placed twice.** That produces
a repeated id too, and refusing it would have settled a question that is open
and is not this one: `Group([(0, take), (4, take)])` is the arrangement's most
natural sentence for *this take, twice*, and what an id identifies in that case
has three candidate answers — forbid it, copy it, or have the intent name the
**placement** rather than the node. One of those three is "forbid", which is
exactly what a validation that refused every repeated id would have chosen, from
inside a check about a different failure.

So the line is drawn by **what the two nodes are**, not by the fact that they
repeat: identical nodes are *ambiguous but consistent* — the document says the
same thing twice, and which placement an intent means is the open question —
while different nodes are *incoherent*, since no answer to that question makes a
document well-formed in which one id names two different things. The message
says which two it found (`node id 2 names two different nodes, an aggregate
and a clang`), because the id alone does not tell an author where to look.

**One asymmetry, recorded rather than smoothed over.** The C ABI's
`clausters_document_open` answers with a null handle and has no channel for the
crate's message, while the wasm face raises it as an ordinary error. So the
Python client looks for the collision itself once the handle comes back null,
and only then — nothing is validated twice on the path where the document is
fine.

## A node carries a name, and an aggregate carries the restrictions its writer put on it

A session did not round-trip. A piece authored with named lanes and a track in
each reopened anonymous, with a level of nesting nobody wrote and the rolls gone
— and both halves were the format's rather than any client's, because what was
missing was somewhere to put them.

**The name is the server's rule, taken rather than invented.** A node gets an
optional `name`, and it is a *referenceable label, never a second identity* —
which is exactly what `/group_new`'s name already is on the wire: the id remains
what every intent addresses and every outcome reports, and the name is a second
way to refer to the same thing. A node is born named or stays anonymous, nothing
addresses by name, so an anonymous one is reachable exactly as before. The
alternative — keeping the label in each client — is what had been happening, and
it means a piece labelled in one writer opens unlabelled in the next.

**The track is the harder half, and the answer is that there is still one
aggregate kind.** A multitrack's track is *an aggregate with the restrictions of
a view*, and the
layer's own rule says the tree stays general — no lane, no vertical position, no
type per container — because a view is a projection and may decline what its
shape does not admit. But a writer that has such an aggregate has to get it back, or
the format silently promotes every track to a plain aggregate. So the **restriction
travels as opaque configuration**, through the same door a generator's code goes
through: `Body::Aggregate` gains a `config` the document carries and never reads, the
Python client writes `{"form": "track"}` into it, and the crate is not one line
wiser about what a track is.

That line is worth stating because it looks like a loophole and is not. Carrying
a payload is what this format already does with everything it cannot own; what
the rule forbids is the tree *acting* on view-ness — sorting by it, validating
against it, giving it a variant of its own. A reader that does not know the key
lays the set out as a set, which is precisely the behaviour an unknown widget
gets in the GUI protocol.

**What it does not fix**, said so the next reader does not assume it: the two
clients now have to agree on a string that no schema checks. That is the cost of
keeping it out of the model, and it is the same cost the GUI protocol pays for
`kind`. The alternative — a typed `view` field — would be checked, and would put a
view in the tree, which is the trade this layer decided in the other
direction on the day it was designed.

## The working copy leads, and it is the buffer the samples were loaded into

O8 says the working buffer leads while a session is open and that a take's pool
buffer is replaced whole once, on confirmation. That was not a preference: it
was derived from a measurement — a write cost the whole buffer (33.8 ms on a
five-minute take), so an editor that wrote through per gesture was unusable, and
a client-side working copy was the honest workaround. S18 removed the
measurement: a write is now flat in the samples.

**The rule survives the measurement that motivated it, and what dies is a
reading of it.** The server buffer *is* the working copy. Loading samples into
a pool buffer copies it — `/buffer_allocRead` reads a file and never touches it
again — so an edit that writes that buffer has already not written the source.
What the four-layer table protects is the user's own file, and the copy that
loading made is what protects it. There is therefore **no second take** while
one is edited, and **no confirmation step per edit**: what confirms a stroke is
the acknowledgement and the log entry, which is what the editor already does.

**Undo never needed the copy either**, and the crate had already said so.
`inverse_of` returns the *empty* write for `WriteSamples` — the samples are not
in the document — "which is why a destructive caller reads its own span before
writing", and the host does exactly that: the previous samples ride in the log
entry, span by span. A second take would have held the whole samples to
recover what a stroke covers.

**Where a temporary copy stays mandatory** is a property of how the samples are
held, not of what a write costs: samples reached **by reference to the user's
file** — mapped rather than loaded, which is the path shared-memory editing
opens. An edit there would write the user's file, which the four-layer rule
forbids outright, so it must write a `Temporary` copy first, and
`confirm`/`promote` are what settle it. That is the whole remaining job of the
`editing` field in the session format, and why it stays: a session that dies
mid-edit over mapped samples must reopen knowing what was undecided.

Stated as one line, because the distinction is easy to lose: **an edit writes
the copy the system already made, and makes a copy only when it has not made one
yet.**

## A redo reports what it applied, so it is the same shape as an undo

An undo hands back the intents it applied and the caller projects them onto
whatever it draws. A redo did not: it applied its steps to the document inside
the crate and answered only with what it *could not* perform, so a client had to
take the whole document and walk it against its own objects to find what moved.

That asymmetry cost three things, and the third is why it is worth a record.
It was **O(document) per step**, in a design whose whole point is that an edit
costs the edit. It was a **second implementation of what an edit means** —
reconciliation, on the one path that had no reason to need it. And it was the
cause of a defect that read as a dead button: only one of the two routes kept
the *drawn record* in step, so a redo moved the model and told the host to go on
drawing the clip where it had been, and the picture caught up one step late, on
the next undo.

So the log's redo now returns `redone` beside `remaining`: the intents it
applied, in order. A client projects them exactly as it projects an undo, and
the adopt-the-whole-document path is **deleted** rather than extended — which is
the opposite of what an earlier attempt at undo-inside-a-clip had to do, and the
reason that attempt was reverted.

`remaining` keeps its own job unchanged: the steps from the first one the crate
cannot perform onward, for the owner to re-run.

## A node id names the placement, and what may be placed twice is what a node references

`Group([(0, take), (4, take)])` is the arrangement's most natural way to write
*this take, twice*, and it silently mis-addressed every edit: the conversion
stamped the node id on the **element object**, so both placements wrote members
carrying one number, the crate applied an intent to the first match and the
editor's index kept the last. Two writes, two destinations, and the clip the
hand moved came back to where it was.

Three answers were on the table for months — forbid it, copy it, or have the
intent name the **placement** — with the third recorded as the faithful one and
the most expensive, "because a member has no stable identity in the document".
Read against what a multitrack *is*, the first two are not options: a clip is a
**window onto samples** and the identity is the samples, so forbidding the
sentence outlaws non-destructive editing's basic move, and copying quietly forks
what the author wrote as one thing.

**The third turned out to cost nothing, and that is the finding.** A `Place`
already names a node, and a node is already what a `Member` holds — the format
has been able to express two windows all along. What collapsed them was the
bridge. Moving the stamp to the **member handle**, which `Group.add` has always
returned and whose docstring already called it "the stable identity `remove` and
`move` take", makes each placement its own node with no change to the crate, the
wire, or the intent vocabulary. It was expensive against a model where a member
has no identity; that stopped being true the day the handle existed.

**What that leaves is the real question, and it is about samples rather than
addressing.** Two windows share samples only when the node *references* it. So
a **buffer** (two nodes, one source), a **generator** and a pattern-backed
**sequence** may be placed twice — two views of one take, two evaluations of one
function, which is the instance/function distinction the user's own exposition
named. An **event**, a **track** or a **group** carries its samples *inside* the
node, so a second placement is a second copy that diverges on the first edit;
that is refused with the distinction rather than made in silence.

What stays open is the **alias** — a node that says "my samples are that node's" —
which is what would let a container be placed twice and edited in one place, and
a placement's own **arguments**, for a function evaluated twice with different
parameters. Both need a placement to be a thing with an identity, which is what
this makes it.

## One segment has one owner, and every other server on it is a guest

**Context.** A segment used to belong to one server for its whole life: `--shm
<path>` created the file, truncated it and re-initialised the header. That was
right while the segment was that server's own transport — one process, one
ring. It stopped being right the moment the segment indexed the **samples**,
because the process most likely to be restarted is the one holding the audio
device, and it was also the one wiping what everybody else was editing.

**Decision.** `--shm` opens what is there and creates only what is not, and the
segment carries a **claim**: the pid serving its command plane, in the word that
already existed to keep `transport_clock` aligned. The first server on a segment
takes it, serves the rings and owns the samples — a directory row and a region
per buffer it installs. Any later one attaches to the data plane, maps what the
owner published, publishes nothing of its own, and serves its clients over its
own sockets. A claim whose pid no longer answers is stale and is taken over.

**Why a claim rather than a convention.** The rings are SPSC and there is one
pair. Two servers draining the inbound one would each get half the commands, and
nothing would say so — the failure is silent and intermittent, which is the kind
a rule in a document does not prevent.

**Consequence.** The arrangement an editor wants becomes expressible: the
on-demand session creates the segment and owns the takes, and the player — a
separate process, holding the devices — attaches. Killing the player takes no
samples with it, and the next one adopts what is there. Two things do **not**
follow from it and are stated where they bite: the clocks belong to whoever runs
a device (a session publishes none), and attaching restores the samples and not
the routing, since ports and patches live with the process.

**And the claim is also what collects the dead.** The property above has a cost
nobody paid at first: samples *outliving* its process is the design, so a
segment left by an editor that was killed rather than closed is indistinguishable
from one being kept, and a region is a whole take — a few crashes fill a memory
filesystem with files nothing can tell live from dead. The claim answers that
question too, so **creating a segment sweeps its directory** of segments whose
owner no longer exists. Three things make a sweep that removes a file this
process never created the right shape rather than a dangerous one. Unlinking a
name never invalidates a mapping somebody holds — the same property freeing one
buffer relies on — so the sweep ends a name, not a reader. A claim of *nobody*
is never swept: `0` is a segment created a moment ago as much as one released on
a clean exit, and neither is dead. And the path being opened is never swept, so
recovery keeps its exact meaning — start a server against the same path and it
adopts what is there, which is the case where a dead owner's samples are wanted.
What is *not* offered is recovering it under a different name: an editor names
its segment for its pid, so its next run is a new path, and the previous run's
take is a file the sweep is right to collect.

## The write frontier is the buffer's, and the segment's row is a mirror of it

A take being recorded is the one samples that changes with nothing announcing
it: a `RecordBuf` fills a buffer block by block from the audio thread, which is
the one place that must never send a message. What a writer publishes instead is
a single number — how far it has filled — and everything that draws a recording
reads that number.

It was first published **only into the shared segment's buffer directory**,
which is where a peer that maps the region reads it, and that made it a fact
about *sharing* rather than about the samples. The consequence was invisible
and total: `/buffer_stream` — the command that exists for clients which cannot
map anything — derived its report from that row, so on a server with no segment
it reported nothing, silently, forever. That is an engine inside a page, and a
`clausters` booted without `--shm`: the command's whole audience, missing
exactly where it was needed.

So the counter lives with the samples. `Buffer` keeps its own `written`, every
writing UGen raises it, and a buffer that has a segment sink mirrors the same
number into the row. A stream reads **the higher of the two**, and that is not
belt and braces: a *peer* writing into the shared region raises the row in its
own process, and this server's `Buffer` never hears about it.

The rule generalizes past this field: a fact about the samples belongs to the
samples, and a shared-memory layout is a way to *publish* facts, never the
place they are kept. What a picture needs is the same number whether it maps the
memory, subscribes to a summary, or is the process doing the writing.

## A peer writes samples, announces the span, and asks for every operation

**Context.** With a take mapped, an editor stores a stroke into the very cells
the engine reads: no message, no blob, no reply, no reconciliation. That is the
point of mapping it, and it takes two things away — the boundary that said what
a client may do to samples, and the only signal any other client had that they
changed.

**Decision.** Two rules, both narrow.

*What may be written is **samples**, samples the writer already holds*: a drawn
stroke, a pasted block, a take it loaded. Every **operation** over samples — a
gain, a fade, a reverse, a render, a resynthesis — stays the server's verb and
is asked for over the wire, however easy the mapped memory makes the other
thing. Without that line the mapping quietly re-opens the question S12 refused,
since a client with write access can compute a fade and store the result, and
nothing in the memory would stop it. One place performs audio processing, and it
is the server.

*What was written is **announced**, not carried*: `/buffer_touch bufnum channel
start frames`, which the server broadcasts as `/buffer_touched` to every
`/server_notify` client but the one that wrote. Four integers, and whoever holds
a picture of that take re-reads the span.

**Consequence.** The saving stays the round trip rather than the copy, and the
clients that cannot map anything keep working: a page is exactly who needs the
announcement, because a browser cannot map a file and a message is the only way
it can hear about an edit at all. What a peer may *compute* is unchanged and is
what it always was — what drawing needs: a peak pyramid, an analysis for a
picture.

## The segment's layout lives in the shared core, not in each reader

**Context.** Four processes read the shared-memory segment — the server that
writes it, the GUI host, the Python client, and any later peer — and three of
them mirrored its `#[repr(C)]` layout by hand: the same offsets, the same
constants, a second copy of the buffer directory's seqlock, a second tap-window
reader, a second implementation of the ring's framing. The only thing tying
them together was the ABI counter, checked on attach.

That is not a check. A version number says "we agree about which layout this
is", never "we agree about what that layout is", and the difference showed up
twice in one week. The GUI host was moved to ABI v9 by its *number* and kept a
size check that demanded a segment with no buffer directory, so it refused
every valid segment while compiling perfectly. The Python client declared 1024
control buses against a server that had had 16 384 for months — wrong, unused
and invisible, because a client maps the *file's* length and only a
documentation constant was derived from the number.

**Decision.** `clausters_core::shm` is the definition: the header, the rings,
the directory row, every offset derived from the header, and a `View` carrying
the whole data plane — the clocks, the control-plane claim, the buses and
levels, the tap rings with their tear-free read, the directory with its
seqlock, the ring framing. It is pure atomics over an address, so it compiles
for wasm and is unit-tested without a mapping.

What each process keeps is **getting the memory**: `mmap` of a file, a heap
allocation, Python's `mmap`. That is the genuinely platform-shaped part, and it
is the only part. A non-Rust peer reaches the rest through
`clausters_core_shm_*` (the C ABI): the *shape* — every count and offset in one
call — plus the things that are logic rather than arithmetic.

**Consequence.** `src/server/ipc.rs` lost 700 lines, `host/shm.rs` half of
itself, and `ipc.py` its whole layout section; what is left in each is that
process's own concern. Three `const` assertions tie the constants the engine
also declares (the audio-bus cap, the block size, the pool size) to the ones
the layout is sized for, so a divergence is a build error. And the tests that
used to build a segment by hand from a reader's own idea of the layout — a
mirror checked against itself — build one through the core or through the
server instead.

**What this does not do.** It does not make the layout versionless: `ABI_VERSION`
still gates attaching, because two *builds* can still disagree. What it removes
is the second kind of disagreement, the one a version cannot see.

## One layer is edited at a time, and it is the only one that acts

**Context.** A container that layers editable things — a clip today — draws
several of them on one rectangle: the placement (where it sits, how long it
is), the samples under it, the events over that, an automation over both. Four
claimants over the same pixels, with no rule saying which one a press belongs
to. Three attempts at something as small as a read-only body were written and
reverted in one day, each of them changing what the clip itself does, because
every attempt was really a fourth ad-hoc precedence added to three that already
disagreed.

**Decision.** One sentence, in `host::layers`: **one layer is active at a time,
and it is the only one that acts or offers an affordance.** A press resolves the
layer from what is drawn under it — the active layer first, then the topmost
layer whose *own samples* is there, then the container's placement — and
selecting a layer is an operation of its own, with the pointer rule as one
caller, `/gui_set layer` as another, and a key binding or a menu as the next.

Three things make it a rule rather than a table:

- **A layer's data is not its rectangle.** An element answers
  `Element::layer_hit` for the things it holds — a break-point, the line
  between two of them, a note — and never for the rectangle it shares with its
  container. That is the whole of why the clip's background, and the grips drawn
  on it, stay the clip's.
- **The stack is the container's contents**, read off the children that fill a
  body role, in the order they are drawn. Nothing in the module names a widget
  type, so a container that grows a fourth kind of content grows a fourth layer.
- **The active layer is a field of the node**, not of the `Clip` variant. A clip
  is the first container here that layers editable things and deliberately not
  the last: an audio editor's view is the same picture — samples, a selection
  over it, an automation over both, later a spectral layer.

**Consequence.** Two standing defects closed without being touched directly. A
clip whose contents are locked **moves again**: a layer that cannot be edited is
never selected by pointing at it, so the press falls through to the placement
instead of being consumed by a refusal — where a body sits is the
composition's, what it holds is the body's. And a curve's **lit segment** is
lit exactly when the curve is the layer in hand, where the bend is the gesture;
inside a clip it used to be lit whether or not the curve was in hand, and the
press there moved the clip.

The grips followed from the same rule rather than from a second one: they are
the placement layer's affordance, so they are drawn while it is active, and the
pixels that light up are the pixels that act — by construction now, instead of
by a precedence written down in two places.

**What is drawn is a second question**, and it is answered the same way:
`visible` on a node, `hidden` on the container, naming which layers are drawn.
Several are drawn at once and one is edited. The two meet in one rule: what is
not drawn is not edited either, since a window taking presses for a picture it
is not showing is the one combination a reader cannot see.

## A clip is a window onto a segment of data, so its edges trim it

**Context.** A clip's take was fitted to whatever rectangle the clip had, so
shortening a clip squeezed its picture — while playback kept reading the buffer
from its first frame for as long as the event lasted. The picture said one
thing and the sound another, and what the picture said was *time stretch*, which
is a rendering nobody had asked for and which this project does not implement.

**Decision.** The multitrack model: a clip is a **window onto a segment of its
samples**, the memory-view idea. `SourceWindow` says where a placement's own
time zero reads (`start`), whether the window wraps (`loop`), and — as an
explicit opt-in — whether the picture is fitted instead (`fit`, the prop a time
stretch will set when there is one). **One timeline sample is one source
frame**: trimming hides frames rather than compressing them, and opening the
window again brings them back.

Everything the definition of a clip carries falls out of that one property:

- **The start grip is a trim**: the offset, the duration and the window's head
  advance together, so the samples stand still while the clip shows less of
  it.
- **The edges stop where the samples end**, unless the clip loops — where past
  the end is the beginning again and before frame zero is the tail of the
  iteration before, which is what stretching an edge means on a loop.
- **A split is two windows over one source**, and a **join** is the inverse; the
  frames neither half shows are still there, which is why a join can put back
  exactly what a split cut.

**Consequence.** A take is drawn a **run at a time**, one run per pass over the
samples: the wrap lives in the run list rather than in the coordinate maps, so
one affine renderer draws a looping clip, a plain one, and the part of a clip
that reaches past samples it does not loop — which stops there instead of
clamping into a flat line nobody recorded.

The window travels with the axis (`TimeSpace`) because it is the other half of
the same mapping: the container decides which part of the data its time covers,
and the element is told rather than working it out.

**On the client side a trim is one edit, not two.** Where a clip sits is its
placement's and what it reads is its element's, so a trim touches both — and a
gesture recorded as two log entries takes two undos to reverse, the first of
which leaves a clip showing frames it does not play. A parent's members carry
both (a member is a placement *and* the node it holds), so one `SetMembers`
states the result of the whole gesture.

**What the arrangement cannot express**, stated because the join meets it: an
element reading **several segments** — two different files, or two windows with
a gap between them, read as one thing. An element wraps one thing. Such a join
is refused by name rather than approximated, because approximating it silently
drops samples.

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


## The arrangement's primitives are named for what they are, not for what they wrap

**Problem.** Three of the arrangement's primitives carried names another part of
the project had already taken, and two of them had been taken by the very object
the element wraps. `clausters.form.Buffer` wrapped a `clausters.defs.Buffer`, so
the sentence describing it was circular and the two could not both be imported
into one script. `clausters.form.Event` wrapped a `clausters.seq.Event`, and the
editor had to alias one of them at every call site (`Event as FormEvent` beside
`Event as SeqEvent`) to keep them apart. `clausters.form.Group` was not the
server's `Group` at all — that one is scsynth's node-tree group, named by every
`/group_*` command and every reply — and prose about "a group" had to say which
one it meant every time.

Shadowing is the visible cost, but it is the smaller one. The larger is that a
name repeated across two layers stops carrying information: a reader who sees
`Buffer` learns nothing about whether they are holding samples or a placement
of it.

**Decision.** The three are renamed for what they are, and the rename reaches
every end of every wire in one move — the Python classes, the `Body` variants of
`clausters-document`, and the `kind` strings of the saved format:

| was | is | what it is |
|---|---|---|
| `Event` / `Body::Event` / `"event"` | `Clang` / `Body::Clang` / `"clang"` | parameters or actions that happen together, internally simultaneous |
| `Buffer` / `Body::Buffer` / `"buffer"` | `Vector` / `Body::Vector` / `"vector"` | a succession of data at constant rate |
| `Group` / `Body::Set` / `"set"` | `Aggregate` / `Body::Aggregate` / `"aggregate"` | the recursive container of placed members |

**Why these words.** `Vector` was already the crate's own gloss for the body
(*"a succession of data at constant rate: a vector"*), so it is adopted rather
than invented. `Aggregate` names the container without `Set`'s collision with
Python's builtin, and the arrangement's is the one that moves because the
server's `Group` is scsynth's and is what the protocol already calls that thing.
`Clang` is James Tenney's term, from *Meta+Hodos*: a **gestalt unit**, a
sound-configuration perceived as a single thing, which he derived from the
German *Klang*. That is precisely what this element is, and it is the one name
here that names the object rather than describing it.

**Two words it is not, and neither is an objection.** SuperCollider has
`Klang`/`Klank` UGens (banks of sines and resonators) — a different spelling, a
different layer, and Clausters ships neither, having declined to port scsynth's
UGen catalog wholesale. And `clang` is the C compiler this repo's build
instructions install for bindgen — a different domain entirely, never a type in
any namespace of ours. A word can belong to more than one domain; what it may
not do is name two things in the same one, which is the whole reason these three
moved.

**Consequences.** The saved format changed, and there is no compatibility
shim: a document written before this reads its three renamed bodies as
`Body::Unknown` and round-trips intact but is not understood. That is acceptable
pre-1.0 and would not have been after. The `grouping` field keeps its name, as
do `Sequence`, `Segments`, `Track`, `Generator`, and the `CONCRETE`/`LOGICAL`
kinds. `Vector.to_event()` and `Segments.to_events()` keep theirs too, because
what they return really is a `clausters.seq.Event`.

## A column reaches the column before it: the trace is a curve, and the columns are not

A digital square wave drew as a row of dashes along the top and another along
the bottom, with the vertical edge between them missing — and it came back at
some magnifications and vanished at others, which is what said the picture had
lost its grip on the samples rather than that one zoom was wrong.

The two regimes are one picture: below `LINE_THRESHOLD` the trace is the
polyline through the samples, where every consecutive pair is joined by a
segment; above it a column is that same polyline's envelope over the samples one
pixel covers. But a column measures a **group of samples**, and groups partition
the samples where the curve does not. Between the last sample of one column and
the first of the next there is a segment, and it was drawn nowhere. On audio it
never showed — a column holding a hundred samples of a wave already spans nearly
the excursion its neighbour does, so consecutive columns overlap by themselves —
and on a one-sample jump it is the whole of the feature. Whether the jump lands
inside a column or on its boundary is a fact about the zoom and the scroll, so
the edge appeared and disappeared as the view moved.

**A column is inked over what it measured, extended to reach the column before
it** (`trace::join`): where the previous column sat wholly below, this one
reaches down to its top; wholly above, up to its bottom; already overlapping,
nothing changes. What is remembered for the next column is the **measurement**
and never the extension, so a run of them cannot walk the trace outwards.

This is not the fill that was refused next door ("A waveform column is its own
envelope"), and the distinction is the whole of it. Filling to the baseline inks
values **the signal never took** — three samples at +0.6 drawn from 0 to 0.6 —
and needs a zoom threshold nobody can name. The join inks exactly the values the
curve takes **while it crosses the boundary**: the same segment the polyline
draws a zoom later, no more, measured from the two columns themselves. Both
rules say the same thing from two sides — the drawing shows the signal and
nothing else, at every zoom, and never changes its mind about what it is looking
at.

It holds for every source (raw samples, a peak pyramid, a cache-only view) and
every picture (the navigable waveform, a clip's take, a plot's series) at once,
because the host's `draw_channel` is the one renderer. The measurements stay
untouched: the pyramid's buckets keep tiling, the wire's overview keeps its
meaning, and nothing about the summary format moves.

**The rule itself is the core's** (`clausters_core::peaks::join`, and
`join_columns` over a whole measured row), which is the correction the web
client forced. "One renderer" is true of the *host* and of nothing else: a page
that reads `Peaks.columns` gets measurements and strokes them itself, so the
first version of this left the rule in Rust and copied it by hand into one
example — and the square wave came apart in a browser exactly as it had in a
window, while every widget-drawn picture in the same page was right. Two
implementations of one rule is how the same buffer comes to look different in a
window and in a browser, so the rule went where the measurement already is,
beside it and one level below both drawings: the host calls it per column,
keeping only the walk (which column the trace is on, and where a run starts
again because a column held nothing), and the web client's `data.joinColumns`
calls it over the row a page just measured. The C ABI has no row of its own —
nothing on that side strokes pixels — which `docs/bindings.md` records.

**The web half of that is gone, and the rule is better for it** *(W26)*. A page
strokes no columns any more: `Peaks.columns` and `data.joinColumns` were a
surface no other client had — a second drawing, reachable from one language —
and they were removed rather than kept in step. What is left is the sentence
this record was always making: `peaks::join` is the core's, the host is the one
thing that calls it, and a client names what to look at. The measurement side
is untouched, and so is everything above about *why* the join exists.

## A zoom past the summary asks for a finer summary, and only then for the samples

A view that cannot map the samples used to have two states and one crossing: the
summary it holds, and — past the base bucket — the **samples** over the span on
screen, read back with `/buffer_getRange`. That is right for the deepest zoom
and it is the wrong request everywhere else. What a picture needs is **one
min/max pair per pixel column**: eight hundred columns is eight hundred pairs,
and at a hundred samples a pixel the span behind them is eighty thousand samples
— a few hundred kilobytes through a 64 KiB carrier, a chunk a frame, to compute
a few kilobytes' worth. `/buffer_peaks` measures exactly that row, at any
bucket, in one reply.

What stood in the way is that **a view holds one pyramid at one base bucket**,
and a report at another bucket cannot be folded into it: the grids do not line
up, and `write_buckets` refuses it, correctly. So the finer answer lives *beside*
the summary rather than inside it — `waveform::Detail`, a pyramid over a span,
the way `Samples::Window` is a run of samples beside the whole. The regime picker
gains a rung: the samples where they answer, then a detail grid where it covers
at this zoom, then the summary.

**The crossing between the two requests is a question about bytes**, and it is
the wire's own arithmetic: a bucket is three floats where a sample is one, so a
grid only pays where a bucket holds well more than three samples. At four it
carries three quarters of what it describes, which is no saving for a second
grid to keep; at sixteen it carries a fifth. With a column holding two buckets
or more — so the fold's position error stays under half a pixel — that puts the
crossing at about **thirty-two samples a pixel**, which is also roughly where
the trace stops being columns and becomes the polyline through the samples. Below
it the samples are asked for and kept: they are exact, they answer every deeper
zoom too, and a grid answers only down to its own bucket.

The grid's bucket is a **power of two**, so a zoom holds still: a grid answers
every column from its bucket upwards (the levels above it are the same pyramid),
so zooming out is free and zooming in asks again only after a factor of two. And
it is coarsened until the whole span fits **one reply** (4096 buckets), because a
detail grid is *replaced* rather than extended — one grid at a time per view, for
the same reason there is one window at a time: what it is for is where the eye
is, and a cache with a policy is a different design.

Nothing about this is on the wire. Both requests are ordinary commands the
server already answered; which one a host sends is its own business, and the
draw pass is where it is decided because that is the only place that knows the
zoom and the span at once (`frame::owed`).

## The score model is the core's, and the engraver is a port shaped like verovio's toolkit

Engraving reaches a page and a window through completely different plumbing —
libverovio linked into a process, or the same library compiled to wasm and
reached through JS glue — and the temptation with a wire like that is to let
each side own "its" score. It is a trap, and a familiar one: what looks like
plumbing is mostly *logic*. An edit is not one call, it is an action, then a
`commit` that re-runs the layout, then a reload of the edited MEI (the
MIDI/timemap cache survives an edit, so without it a transposed note keeps
sounding at its old pitch); a document must be drawn before an edit reaches it,
or the editor reaches through drawing state the load never built and the process
dies; and undo is a stack of MEI snapshots of our own, because reloading resets
verovio's stack, its `canUndo` lies, and its `undo` on an empty stack takes the
process down. Written twice, those are two chances to get each of them wrong,
and no compiler is watching.

**Decision.** `clausters_core::notation::Score` owns all of it, generic over an
`Engraver` port; `clausters-notation` implements that port over libverovio for a
native caller, and a page implements it over the wasm build's exports. Building
the engraver — a resource path, an options JSON, whatever the binding needs —
stays with the binding, because that is the part that genuinely differs.

**The port is verovio's toolkit surface, deliberately, and not an abstract
notion of engraving.** Both implementations drive the *same* C wrapper: natively
through `tools/c_wrapper.h`, in a page through the Emscripten build's exports,
which are that same wrapper (`_vrvToolkit_edit`, `_getMEI`, `_renderToSVG`, …).
An abstraction over it would be a third vocabulary nobody speaks, and it would
have to be invented from one implementation anyway. What the port buys is not
independence from verovio — it is that the *order of the calls* has one
definition.

**Consequence.** The core's own tests drive the state machine over a fake
engraver, which is the first time any of those rules has been testable without a
C++ library present; the binding's tests keep the cases that need a real
engraver to be right about. And the two clients must engrave with **one verovio
version**: `third_party/verovio.pin` tracks the release the npm package
publishes, so a display list from a window and one from a page are comparable to
each other rather than to two different engines.

## The browser's verovio is our build, not the published one — and the SDK that makes it so

The engraver reaches a page as a wasm module, and there are two ways to have
one: take the artifact upstream publishes on npm, or compile the pinned sources
ourselves with the **Emscripten SDK**. The published one is tempting — no
toolchain, no build leg, no CI question — and it is what was chosen first,
before the two builds were actually compared.

**They are not the same build.** `third_party/build-verovio.sh` trims verovio on
purpose: `NO_HUMDRUM_SUPPORT` (which vendors humlib, 148k lines, and is worth
3 MB in the wheel), `NO_GABC_SUPPORT`, `NO_DARMS_SUPPORT` — so the library
carries MEI, MusicXML and its compressed form, Plaine & Easie and ABC, which is
the list of formats the client actually offers. Upstream's npm build makes its
own choices. Taking it would have meant one *version* at both ends and two
different builds, and what a build decides here is **which input formats
exist** — so a page would read a GABC file a window refuses. That is a
capability in one client and not the other, which this project calls a defect
rather than packaging (`CLAUDE.md`, "Non-divergence").

**Decision.** `third_party/build-verovio-wasm.sh` sits beside the native script,
reads the same `verovio.pin` and passes **the same three options**; the web
client's `build.sh` stages the pair it produces, off the slim `dist/runtime.js`
so a page that never engraves downloads nothing. The Emscripten SDK is installed
user-space under `~/.local`, the same pattern as node and libfaust; nothing in
`src/` or the test loop touches it, which is what keeps the "typescript is the
only dependency" rule intact. The fonts are deliberately *not* trimmed, by the
same argument that trims the importers: the native prefix installs the whole
SMuFL set, so cutting it in the browser would make a font a window engraves in
one a page cannot.

**Size turned out not to be an argument at all, and the estimate that said
otherwise was wrong.** Dropping `-s SINGLE_FILE=1` — upstream's release flag,
which base64s the engraver into the glue — was expected to halve the bytes.
Measured, it does not: the published module is 7.0 MB raw and 2.2 MB gzipped,
this build is 6.6 MB and 2.2 MB. Base64 costs a third on disk and gzip gives
almost all of it back, so the two are the same download. What a separate
`.wasm` still gives is a file compiled from bytes rather than decoded from text
first, and cached and served as what it is — worth having, and not worth a
decision on its own. The decision rests on the build identity above, alone.

**This is reversible, and the conditions are worth naming.** The producer of the
artifact is not visible to any TypeScript: the shell drives the six toolkit
calls through the `Engraver` port whatever compiled them, so going back to the
published packages — npm `verovio`, and `@grame/faustwasm` for the Faust
compiler — costs a change to `build.sh` and an asset path, and nothing else. Do
it if the vendored build stops being convenient **for both** artifacts: if
keeping the SDK current turns into a maintenance tax, if CI's build leg costs
more than the divergence it prevents, or if upstream's builds converge on the
options we pass. The reverse trade is the one recorded above, and it is not
free: a published artifact brings its own build's surface with it.

**Faust is the other half of this and is not settled by it.** `libfaust-wasm` is
published too (`@grame/faustwasm`), so nothing forces a vendored build there
either — but the Faust wasm has to be integrated with the Rust engine rather
than merely loaded beside it, and that integration is what will decide the
shape. It is also blocked on something a compiler artifact does not solve: the
in-page engine is the `synth,embed` build with no LLVM JIT, so a def compiled in
the page has no way to be instantiated until the engine grows one (a wasm DSP
module behind the worklet, or the Faust interpreter backend executed in our own
engine). The SDK installed here is what W7 would have had to install; the
decision it still owes is its own.


## A knob is measured from its press, so no front captures the pointer

*2026-08-22.*

A knob has no groove on screen to point at, so what turns it is a distance
rather than a position — and there are two ways to read that distance. The
first: accumulate the step since the last event, re-anchoring every frame. The
second: measure the travel since the **press**, against the value the press
found. They agree while the pointer stays on the widget and part company the
moment it does not.

The host had the first one, and paid for it twice. A drag whose pointer left the
element kept accumulating whatever motion still arrived, so the value fell out of
phase with the hand; and the clamp at an end **ate** the motion spent past it, so
coming back left the value short by however far the hand had gone. The same pair
of defects had already been found and fixed on a curve's bend, which is now
anchored at the press (`bpf::bend_curve` sets rather than adds). A knob was left
incremental on the argument that it is different — a knob has nothing on screen
to stay level with, so nothing to be out of phase *with*, and a pinned control
that reverses immediately reads better than one that has to unwind.

The reason to accumulate was that the cursor was expected to be **captured**: the
desktop front locked the pointer at the press (`CursorGrabMode::Locked`) and drove
the element with raw device deltas, so there were no positions to measure from.
The browser front had no such path, so the very same widget was one gesture in a
window and another in a page — which is a defect under the standing rule, not a
platform's prerogative.

**Making the page capture the pointer was tried and does not work**, and the
reasons are winit's web backend rather than ours:

- `set_cursor_grab(Locked)` calls `requestPointerLock` and returns `Ok(())`
  whatever the browser decides afterwards, so the front's answer — the very thing
  the machine routes on — is a guess. Nothing synchronous can do better: the lock
  resolves asynchronously, and the press has to decide now.
- The deltas it would be paid in are emitted only from a `pointermove` that
  carries **no button** (`PointerEvent.button == -1`). Chrome reports `-1` on a
  move; Firefox reports `0` while the button is down and takes the chorded-button
  path instead, where no motion event is emitted at all. The knob would be dead
  in one of the two browsers — the same Chrome/Firefox split that once delivered
  a press per frame.
- Those deltas come from `getCoalescedEvents()`, whose list is **empty for a
  synthesized event**, so no page test could drive a captured knob even where it
  worked.

So the capture is gone and the measurement changed instead: `Dial` records where
the press landed, the fraction the value stood at there and the body's height,
and every step is `t_press + drag_fraction_delta(cy - y_press, body_h)`, clamped.
A cursor position has **one** answer, so the pointer may leave the disc, cross
the window and come back with the value exactly where it says, and the motion
spent past an end is kept rather than eaten. `number` follows `knob`; the
absolute controls (`slider`) never had the question.

**What this costs** is the pinned-reversal the incremental form gave: past an
end, the hand has to travel back to the anchor before the value moves. That is
not a trade-off so much as the same rule applied honestly — the value is where
the cursor says, at an end as everywhere else — and it is what the bend already
does.

**What it removes** is a whole seam: `Take::grab`, `Claim::grabbing()`,
`Element::drag_relative`, `Gestures::locked`/`relative_motion`,
`GestureEffect::ReleasePointer`, the grab callback `Gestures::press` took, and
the native front's `grab_pointer`/`release_pointer`/`device_event` path. No
element asks a front for the pointer any more, and a gesture is made of cursor
positions on both fronts — which is what makes the two the same program.


## Choosing a carrier is not consenting to the network

Every carrier's flag was a port and nothing else, and the two fronts had drifted
into three different answers about where they listened: the audio server bound
UDP on loopback and TCP and WebSocket on `0.0.0.0`; the GUI host bound UDP and
TCP on loopback and WebSocket on `0.0.0.0`. Nothing said any of it — a `--tcp`
or a `--ws` asked for on behalf of a client on the same machine opened the port
to the LAN, and the one leg that is on by default (the host's TCP) was loopback
only by luck of which file it was written in.

So the flags take the address: `--udp [addr:]port`, `--tcp [[addr:]port]`,
`--ws [[addr:]port]`, on both programs, in one shared parser
(`config::PortChoice::parse`) — a bare port, an address alone (the port still
follows the base), or both. **Loopback is the default in all six legs**, and the
widening is a decision written on the command line: `--ws 0.0.0.0:57120` for
whoever means the network. The WebSocket leg is not the exception it looks
like — a page served from this machine reaches `ws://127.0.0.1` either way, so
the common case pays nothing.

- **The bind belongs to the flag, not to the leg's own module.** `bind_tcp`,
  `bind_ws` and the windowed front's `run` used to name `"127.0.0.1"` or
  `"0.0.0.0"` in their bodies, which is how one front came to differ from the
  other without anybody choosing it. They take a `SocketAddr` now and where it
  points is settled once, where the line is read.
- **A config value says it the same way.** `tcp = "0.0.0.0:57110"` is the same
  string the flag takes, beside the `true`/`false`/port the key already
  accepted, so nothing has two spellings.
- **A typo is an error, not a bare flag.** A token after `--ws` that is not a
  bind used to fall through as "no argument" and resurface as `unknown
  argument: 0.0.0.0:57121`; it is now reported against the flag that owns it.
  Addresses are literals — a hostname would resolve to several and the flag
  binds one.

**Remote is then something asked for**, which is where it belongs, and two
things degrade under it — neither of them a bind, both worth naming before they
are found by ear. A GUI source's `path` carrier writes a temp file the host
maps, so client and host are assumed to share a filesystem; over a distance only
the inline ceiling is left. And `--data-dir` names the *host's* GuiDef store,
not the script's.


## The page's Faust is a second wasm module linked into the engine's own memory

*2026-08-25.*

The in-page engine is the `synth,embed` build: no libfaust, no LLVM JIT, so
`/def_send faust` answers `/fail` in a tab and `/done` in a window. That is a
capability one client has and the other does not, which this project treats as a
defect rather than as a platform's quirk, and closing it is the half of W7 that
a compiler artifact does not solve on its own. `libfaust-wasm` gives us a
*compiler* in the page; what it does not give us is a way to **instantiate** what
it compiled inside a node of our tree.

**The shape of the answer is fixed by what a Faust node already is here.**
`FaustSynth` (`src/faust/synth.rs`) holds an opaque instance pointer, a `Vec` of
`f32` parameter zones reaching into it, and private non-interleaved block
buffers it stages I/O through; per block it does one FFI call,
`computeCDSPInstance(dsp, frames, in**, out**)`, and `/node_set` is a plain
aligned store into a zone. Faust's **wasm backend emits exactly that ABI**:
`compute(dsp: i32, count: i32, inputs: i32, outputs: i32)`, pointers being byte
offsets into linear memory, with each parameter's offset inside the DSP struct
published in the JSON the compiler returns beside the binary (`"size"` for the
struct, `"index"` per parameter). On `wasm32` a `*mut f32` *is* a `u32` offset,
so `in_ptrs`/`out_ptrs` and `zones` are already the right types. The node's
`process` needs no restructuring at all — three `ffi::` calls change hands.

**So the Faust module is linked into the engine, not run beside it.** Compiled
with `-lang wasm-e` (external memory: the compiler documents it for polyphony,
and what it means is that the module allocates nothing), the emitted module is
instantiated with `env.memory` bound to **the engine's own
`WebAssembly.Memory`** and its `compute` export appended to the engine's
`__indirect_function_table`; Rust calls it through that table by transmuting the
slot index to an `extern "C" fn`, which on `wasm32` is what a function pointer
already is. The DSP struct is allocated by our allocator, at an offset we choose,
and the I/O buffers are the `Block`s the synth already owns. **No copies, no
JavaScript frame on the audio path, and the node stays a node** — `/node_set`,
`/node_map`, the bus summing and the done actions are the same code in both
builds.

**The transcendentals are ours too.** A Faust wasm module imports what the
instruction set lacks — `env._sinf`, `env._powf`, `env._fmodf` and their
neighbours. A wasm function exported by one instance is a legal import of
another, so these are bound to the engine's own exports rather than to
`Math.sin` closures: no JS on the audio path, and Faust and our UGens go through
one libm, which is what keeps a parity vector meaningful.

**The compiler gets a scope of its own, and the protocol does not learn about
it.** Natively, `/def_send faust` compiles on the compiler thread and replies
`/done` later; the audio thread never sees a compiler. In a page that thread is
the **NRT worker** — this first said the main thread, and the second decision
below moved it, for the reason that decision gives: the main thread owes the GUI
host its frames. So the worklet, on `/def_send faust`, posts the payload out, the
Worker compiles with `libfaust-wasm`, posts back the module bytes and the JSON,
and the worklet instantiates and replies `/done`.
This is the same asynchronous shape the command already has, which is why
**nothing on the wire changes**: no precompiled-payload form, no second command,
no client-side special case. `docs/schemas.md` is untouched, and the one
observable difference is that the reply stops being `/fail`.

It also settles the packaging rule W7 states without needing a rule: the
compiler's assets are fetched by the engine's glue **on the first `/def_send
faust`**, so a page that mounts a prebuilt bundle of SynthDefs downloads none of
them, and one whose bundle carries a FaustDef downloads them because it needs
them.

**The one hazard, named because it is invisible until it corrupts something.**
The wasm backend writes the DSP's JSON into a data segment at **absolute offset
0**, unconditionally — external memory included. Instantiating such a module
against the engine's memory therefore writes over the engine's own first bytes.

*This paragraph first said to move the engine's data out of the way with
`--global-base`, and that is not available.* Building the thing found out why,
and the correction is worth keeping because the reasoning that produced it was
sound and still wrong: rustc links wasm with `--stack-first`, so the low
megabyte is the **stack**, not `.rodata` — and `--global-base` must be at least
the stack size when `--stack-first` is used, so it cannot open a gap *below* the
stack, which is exactly where the segment lands. A reserved page there is not
something the linker will give.

So the segment goes instead of being worked around. Nothing reads it — the JSON
we use is the one `createDSPFactory` returns beside the binary — so the copy
inside the module is dead weight, and `faust::wasm_module::strip_data_section`
removes it before instantiation. It walks the binary's own top-level framing (an
id byte and a LEB128 length, stable since the MVP) and **refuses rather than
guesses**: more than one segment, or one that does not start at zero, means the
backend grew a use for that memory this has not accounted for, and dropping it
blind would be the kind of corruption nobody traces back here.

**What was rejected, and why it is not a near miss.**

- **A Faust wasm module as a WebAudio node beside the engine** — the shape the
  phrase "run behind our AudioWorklet" first suggested. It puts the Faust DSP
  *outside* the node tree: no `/node_set` by control index, no group ordering, no
  bus summing, no done action. The browser would gain a Faust that is not the
  same thing the window's Faust is, which is the divergence this milestone
  exists to remove, not a cheaper way to remove it.
- **A separate memory per instance, with JS copying blocks across** — the
  conservative version of the choice above, and the fallback if the linker
  constraints below ever stop holding. It keeps the node in the tree, but it puts
  a JavaScript frame and two copies in the audio callback, per Faust node, per
  block. The B track's own topology note already draws the line in exactly this
  place: OSC translation may allocate on the worklet thread, **the DSP may not**.
- **Faust's interpreter backend, executed inside `libfaust-wasm`** — the other
  path the plan named. It moves the audio compute into the Emscripten module
  entirely, which means the whole compiler resident in the worklet, an
  interpreted inner loop, and the same cross-module boundary as the option above
  with worse numbers on the other side of it. Its only advantage is one artifact
  instead of two.
- **A wasm interpreter in Rust inside the engine** (`wasmi` and its family) —
  everything stays in Rust and nothing else does: interpreting wasm inside wasm,
  on the audio thread, allocating.

**Two things the build found, both invisible until something is silent.**

*A `WebAssembly.Module` posted into an AudioWorklet is dropped on arrival.*
Compiling in the Worker and cloning the finished module across is the obvious
shape — a Module is serializable, and it keeps the last piece of compilation off
the thread that owes a block. It appears to work: the `postMessage` succeeds,
nothing throws on either side, and the message simply never arrives, because a
Module can only be cloned inside one agent cluster and a worklet is not in the
Worker's. So the **bytes** travel and the worklet compiles them itself, which is
microseconds once per def against a silence with no error anywhere. The port now
carries an `onmessageerror` for the same reason: that is where a dropped message
does show up, and without it the engine just never answers.

*wasm-bindgen drops the exports the linker synthesized*, `__indirect_function_table`
among them, so `--export-table` on the Rust side is not enough — the whole design
rests on that table and it was gone from the staged bundle. `--keep-lld-exports`
on the engine's bundle alone keeps it; the other three bundles link no second
module and keep the smaller surface.

**The interpreters are the server's, and they run in the page.** A def sent as a
box or a signal tree is read by `faust::boxes` and `faust::signals` — the same
Rust a native server runs, compiled to wasm and living in the NRT worker — and
what they issue is Faust's own C API. The alternative was to read the schema
again in TypeScript, or to print it back to Faust source; both are a second
implementation of one schema, and the failure they produce is a def that means
something slightly different in a tab than in a window, with nothing to report
it. Two things had to be built for the interpreters to reach the compiler:

- **Faust's Emscripten bindings expose only `createDSPFactory`.** The wasm
  backend has `createWasmDSPFactoryFromBoxes` and `...FromSignals` already; what
  was missing was the JS-facing surface over them and the C API being exported at
  all. `third_party/faust-wasm-bindings.patch` adds four embind methods, and the
  link exports the C API — the list read out of `src/faust/ffi.rs`, so a call the
  interpreter grows and the artifact does not export fails at load, by name,
  rather than later.
- **The two modules do not share an address space.** A box handle is an integer
  and crosses as it is; a `const char*` is a pointer into whichever module made
  it. `faust-shim.js` is that boundary and nothing else: it reads a C string out
  of the interpreter's memory and writes it into the compiler's, copies a
  NULL-terminated handle array the same way, and gives `CDSPToBoxes` its `argv`
  and its three out-pointers. Which arguments are strings is a table of
  nineteen entries; everything else passes through.

**Two more silences, both found by building it and both worth the words.** They
have the same shape as the two above — the thing works, then a *later* def fails
for no reason anyone would connect to it.

- **A label freed when its call returns breaks a def compiled afterwards.**
  Faust keeps what it was handed, so the native path holds every `CString` until
  the factory exists and says so. Freeing per call passes every small test and
  then, once the heap has churned enough to hand the block out again, breaks the
  term merging hash-consing does: a graph that shares a subterm stops sharing it,
  and a recursion over it never terminates. It is reported as a stack overflow
  inside the compiler. So the shim holds one arena per def and frees it when the
  factory exists.
- **A destroyed lib context poisons the next one**, the same way and with the
  same report. The native path brackets every def with
  `createLibContext`..`destroyLibContext`; here that bracket is what breaks the
  *following* def — reproduced down to two, a box with a `rec` and then anything
  recursive. So a page opens one arena and keeps it, which also costs less than
  one per def. That in turn decides how **source** is compiled: through the box
  schema's own escape hatch (`{"op": "faust", "src": …}`, which is
  `CDSPToBoxes`) rather than through `createDSPFactory`, because that entry
  point allocates and destroys a context of its own and would take the page's
  arena down with it. One consequence worth naming: all three formats now reach
  the compiler by one path, which is a better shape than the one this started
  from.

**A third silence, found by measuring the first two.** Keeping one arena for the
tab's life has a bill nobody had asked for: the *call stack*. A wasm frame sits
on the JavaScript engine's stack — about a megabyte, against the eight a native
thread gets, and unrelated to the `STACK_SIZE=8MB` the compiler is linked with,
which is only its shadow stack — and libfaust recurses over the term graph of
everything compiled so far. So a page compiling **distinct recursive signal**
defs runs the stack out at roughly every other def, reported as
`stack overflow (Maximum call stack size exceeded)` on a def that is perfectly
well formed. It is browser-only: eight of the same defs compile in one context
natively, which is what says the interpreter is not at fault.

The way out is not a smaller def and not a destroyed context. It is a **fresh
compiler**: a def that exhausts the stack is compiled again in a new Emscripten
instance — new memory, new arena, new context — and succeeds. This does not
contradict the entry above, and the distinction is the whole point: what poisons
a page is a context that was *destroyed*, not a second one that exists. The cost
is one instantiation, and the fetch is in cache and the module already compiled,
so twelve such defs in a row (six replacements between them) average 18 ms each
against the 9 ms a def that never overflows costs. `tests/faust-arena.html` is
the measurement and the numbers move with the compiler's pin, so it is a page to
re-run when `third_party/faust.pin` does.

Defs written as **box** trees or as **source** never hit it, which is worth
knowing but is not a reason to steer anyone: the retry makes the signal API work
without a caveat, and a client surface that is fine in one language and
conditional in another is the divergence this project does not keep.

**Three assumptions carry this, and all three were checked before anything was
built on them** — a probe that instantiates a hand-emitted second module against
a Rust one. They hold: `-C link-arg=--export-table -C link-arg=--growable-table`
makes rustc export `memory` and `__indirect_function_table`; a second module
instantiates with `env.memory` bound to the first's memory and a Rust export
bound as its `_sinf`; and Rust calls that module's export **through the shared
table by index**, transmuting the index to an `extern "C" fn` — which on
`wasm32` is what a function pointer already is.

Each also fails loudly rather than subtly, which is what makes them safe to
depend on. A wrong index or a changed pointer representation traps on
`call_indirect`'s type check, and the fallback is a JS shim doing
`table.get(i)(…)`, one frame per block. The link flags are checked by the wasm
build gate. And a Faust release where `-lang wasm-e` stopped meaning "allocates
nothing" would be caught by the def-send path failing to find its parameters
where the JSON says they are. None of the three is a reason to prefer a design
that is slower on purpose.


## The browser engine gets a second scope: a Worker for the work that is neither audio nor UI

*2026-08-25.*

The native server has four kinds of thread and the browser engine has had one.
Network, audio, and the auxiliaries that may not block either — the NRT runner
(every `/buffer_*`: allocation, decoding, file I/O, zeroing), the Faust
compiler, the disk streams — are four roles that the page collapsed onto the
AudioWorklet, because `OscServer::headless` serves a pulled turn (`step()`)
before each block and everything else was compiled out or made inline
(`NrtRunner::Inline`, `workers = 0`, `DiskIn`/`DiskOut` target-gated off).

That was right for B0–B4, whose question was whether the engine runs at all. It
is wrong now, and the cost is measurable rather than theoretical: **the render
quantum's budget is about 3 ms** at 44.1 kHz (2.67 at 48), and a buffer install
is a memcpy into wasm linear memory — a five-minute stereo take is 110 MB, tens
of milliseconds, on the thread that owes a block every 2.67 ms. Nothing has
glitched yet only because no example is that big.

**Decision: one dedicated Worker, beside the worklet.** It is the browser's
version of "the threads that are neither audio nor UI", and it takes exactly the
roles that are those threads natively:

- **The NRT runner.** `server::nrt::run_job` is already a `pub fn` whose own
  docstring names two callers — the NRT thread and the offline renderer, which
  calls it synchronously. The Worker is a third caller of the same door, not a
  new architecture: a slim wasm shell beside `clausters-web` exporting `run_job`,
  fed jobs over a port and answering with the built buffer's frames.
- **The Faust compiler** (revising `B5`, which had put it on the main thread).
  Compiling a def is tens of milliseconds; on the main thread that is the GUI
  host's frames. It belongs where the native compiler thread is — off both.

**Why the decoder is ours and not the browser's.** The obvious shortcut is
`decodeAudioData`. It is the wrong one: it is a different decoder from
symphonia, so the same file would become different samples in a tab and in a
window — a divergence in values, which is worse than one in surface because
nothing names it. The Worker runs our decoder.

**What travels, and what it costs.** No `SharedArrayBuffer`: that question is
settled and priced elsewhere in this file, and reopening it means a profile
rather than an argument. Samples cross as **transferred** `ArrayBuffer`s (a
move, not a copy), and the Worker holds a `MessagePort` transferred into the
worklet, so a chunk does not queue behind the main thread's event loop at 60 fps.
What remains on the audio thread is one bump allocation in linear memory and one
memcpy — which is the next decision.

**`step()` gets a budget, and that is the general form of the fix.** Everything
the worklet does outside `process_block` is bounded per turn, and work that does
not fit resumes on the next one. A buffer install is **chunked** across serving
turns; the swap-in stays one pointer store, which the "every job replaces the
buffer" rule already gives us, and `/buffer_*` already replies `/done` late, so
the protocol learns nothing. The same budget bounds an OSC burst or an oversized
bundle, which today have no ceiling either. This is the one invariant that
changes: the worklet's serving turn stops being "drain everything" and becomes
"drain what fits".

**OPFS is what makes the file-shaped commands mean something.** A page has a
private filesystem, and `createSyncAccessHandle()` — synchronous read and write,
the same shape `reader_thread`/`writer_thread` have natively — is available in
**dedicated Workers only**, by standard, precisely so nobody blocks the main
thread with it. So the Worker is also where files live: `/buffer_allocRead`
stops being a path with nothing behind it, `DiskIn`/`DiskOut` become possible,
and a persisted data dir — the browser twin of `--data-dir` — becomes possible
with them.

**`DiskIn`'s honest price.** Without SAB the stream cannot be a sample-accurate
SPSC ring; it is a chunk queue the Worker keeps ahead and transfers. Chrome's
own guidance is worth quoting against ourselves here: a ring buffer "only
reconciles the buffer size mismatch and does not give more time to run the
code", and Worker↔worklet synchronization stays loose enough that an
underrun shows up as a drop. So **the prefetch depth is the design**, not a
tuning constant, and a browser `DiskIn` is a deeper-latency instrument than the
native one. That is a difference to write on the page, not to hide.

**Browser compatibility, checked rather than assumed.** Every piece is
Baseline: AudioWorklet (Chrome 66, Firefox 76, Safari 14.1); OPFS sync access
handles, widely available since March 2023, dedicated-worker-only in all three;
`MessagePort` exposed to Window, Worker **and** AudioWorklet and transferable,
per the HTML standard. Two WebKit findings are worth carrying:

- **Passing a `WebAssembly.Module` to a worklet was broken in Safari** and is
  the mechanism B2 already relies on (`processorOptions`). Fixed in November
  2022 (WebKit 220038), so the floor for the engine in Safari is that release,
  not 14.1 — worth knowing before a bug report blames our code.
- **iOS Safari crashes a tab somewhere near 350 MB of wasm memory**,
  undocumented. It is the reason the buffer pool must stay modest on mobile and
  a long take must stream rather than pool — the same conclusion `DiskIn`
  reaches from the other side.

The one thing not verified from documentation is transferring a `MessagePort`
*into* the worklet on Safari specifically; WebKit's serialization fix above
covers the mechanism, but no test here can drive Safari. So the direct
Worker↔worklet channel is **feature-detected with a handshake at boot**, falling
back to relaying through the main thread. Same semantics either way — it is a
transport detail, like choosing a carrier — and the fallback costs latency, not
correctness.

**A number that does not hold cuts the feature, not the budget.** Each step of
this is gated on a measurement rather than on being finished, and the outcome of
a bad measurement is decided in advance: the browser keeps the **reduced**
capability — a ceiling on a buffer, a floor under a prefetch, a `DiskIn` that
only plays forward — and the reduction is written on the page of limits with the
number that caused it. A tab that does less than a window is a limit, and there
is now a place for limits; a tab that drops samples is a defect, and there is no
place for those. The temptation this forecloses is the other order: keeping the
full capability and widening the budget until the glitch is rare enough to be
somebody else's bug report.

**And what this does not fix, ever.** Parallel groups. DSP threads mean wasm
threads mean `SharedArrayBuffer` mean cross-origin isolation, and a component
embedding on a page we do not control cannot demand headers of it. The Worker
does not help: it is a second thread, not shared memory. `/group_parallel` is
accepted and serializes — bit-identically, so the samples are the ones the flag
promises — and `Session.embed` takes no `workers` argument in the browser. This
is now written where a reader meets it, in the web client's book ("What a tab
cannot do"), beside every other limit that is the platform's rather than the
port's.

## A page has one number type, and Faust's promotion makes that cost nothing

*2026-08-27.*

Porting the box API found that a page cannot write a Faust constant that is
*real* and integral. A def builder's numeric constant travels as a bare JSON
number and the server reads it back with `serde_json`: an integral one becomes a
Faust **int**, anything else a **real** (`src/faust/boxes.rs`, `number_box`; the
signal schema does the same). Python separates them by the literal's type —
`box(2)` is `int 2`, `box(2.0)` is `real 2.0` — and JavaScript has one number
type, so `2.0` *is* `2` by the time `boxes.box`/`signals.signal` sees it. It is
older than the box API (`signals.ts` has had it since W1) and the frozen parity
vectors cannot see it, because they are compared as parsed JSON where `2.0` and
`2` are the same value — worth knowing about what those vectors prove.

The obvious repair is a spelling: both schemas already accept an explicit
`{"op": "real", "value": x}` node. It was not taken, and the reason is a
measurement rather than a preference. Compiling both spellings of every operator
that could care (`faust -lang c`, the pinned compiler) says the constant's type
never decides a **value**:

- **`/` is real whatever its operands are.** `7 / 2` folds to `3.5f`; there is no
  integer division to fall into.
- **Any real operand promotes the whole expression.** An audio signal is real, so
  `sig % 3` and `sig % 3.0` are the same `fmodf`, and so on through the arithmetic.
- **Where int is *required*** — a table index, `@`, `select2`, the bitwise family
  — a real is cast down silently, so the int a page emits is what those wanted.
- **What is left is an operator pair chosen by type against an *int* signal**:
  `%`, `min`, `max` and the comparisons compile to `%`/`min_i`/`>` against an int
  and `fmodf`/`fminf`/`>` on floats. Both compute the same number for an integral
  constant; they diverge only past 2²⁴, where the exact one is the int.

So the gap is a spelling, not a behaviour — and the spelling already exists, in
a verb both clients have: `asfloat`. `box(2).asfloat()` is `float(2)` in Faust,
which the compiler folds to `2.0f`, and `int(x) % float(4)` is byte-identical to
`int(x) % 4.0`. Adding a `real()` builder would put a name in one client that the
other has no use for — Python has the float literal — which is the divergence the
non-divergence rule exists to prevent, paid for a distinction nothing can hear.
Recorded in the web client's book, under the platform's own limits.

## The two vendored wasm artifacts are built by the release, from three pins

*2026-08-27.*

`libfaust-wasm` and `verovio.wasm` — the Faust compiler a page JITs a def with,
the engraver it lays a score out with — are the only things a page loads that are
not compiled from this repository's sources. Both were built on a maintainer's
machine and staged by `clients/web/build.sh`, which left the packaging half open:
CI grows an Emscripten leg, or the artifact is fetched from somewhere and pinned
by digest.

**Fetching was never actually available**, and that is what settles it. There is
a published build of each, and neither is the one we need: npm's `verovio` is a
different build of the same version, and `@grame/faustwasm` binds the Faust
*source* API only — not the Box and Signal APIs, which `third_party/faust-wasm-bindings.patch`
adds so that the one JSON interpreter (`faust::boxes`, `faust::signals`, in Rust)
drives the same compiler in a tab as in a window. A def built with the box API
would compile in a window and fail in a tab. So the artifact is ours, somebody
has to build it, and "pinned by digest" would only ever pin *our own* build —
which is the pin file, not a digest.

**The cost of leaving it on a laptop was measured and it was not hypothetical.**
`package.json`'s `files` lists `dist/`, and what is not in `dist/` is not
published: `publish-npm` built the package in a runner with no SDK, `build.sh`
printed its two `note: vendor/… missing` lines, and the published package shipped
**with no compiler and no engraver**. It installs, it loads, it plays — and then a
FaustDef will not compile and a score will not engrave, on the user's machine.
That is the quietest failure in the package.

So: **the release builds them**, through `.github/actions/wasm-vendor`, the same
restore-or-build composite already proven for the two native libraries, running
the same vendored recipes a maintainer runs. And **`tools/check-package.mjs`
requires them**, so the silence cannot come back: a tree without them is not
publishable, while a tree without them is still perfectly developable — nothing in
`src/`, the suites or the smokes reaches either artifact.

Two consequences worth naming. **The SDK is pinned** (`third_party/emsdk.pin`,
read by both recipes and by the composite's cache key): these artifacts are almost
entirely toolchain output, so an unpinned `emsdk install latest` meant the
published package was built by whatever resolved that morning — and two of the
five departures `build-faust-wasm.sh` documents exist precisely because a current
emcc changed under the pinned Faust's build files. **CI does not build them**: the
two builds are ~20 minutes cold, `ci.yml`'s smokes reach neither, and a page that
wants the compiler is a manual test either way. The cache key is the three pins
plus the recipes' hashes, so a repin or a flag change moves it with nothing to
keep in sync in the callers.
