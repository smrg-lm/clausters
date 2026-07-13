# Implementation plan: a real-time scsynth-style audio server

A synthesis server in Rust controlled over OSC, inspired by the architecture of
scsynth (SuperCollider): a process that opens the audio device, keeps a node tree
(synths and groups), and receives OSC commands over UDP to create/destroy synths,
set parameters, manage buses and buffers, all with sample-accurate scheduling.

## Design principles (non-negotiable)

1. **The audio thread never blocks**: no `malloc`/`free`, no locks, no syscalls,
   no I/O inside the audio callback. See the `realtime-audio` skill.
2. **All communication with the audio thread is lock-free**: SPSC ring buffers for
   incoming commands and for returning "garbage" (memory to free) to the non-RT thread.
3. **Block processing**: blocks of 64 samples (like scsynth), not sample-by-sample,
   to amortize UGen dispatch.
4. **Conceptual, not binary, scsynth compatibility**: same model (node tree,
   buses, buffers, SynthDefs, commands `/s_new`, `/n_set`, etc.) but our own
   SynthDef format (not the binary `.scsyndef` format — at least not in v1).

## Thread architecture

```
┌─────────────┐  OSC/UDP   ┌───────────────┐  SPSC cmd FIFO  ┌──────────────┐
│ OSC client  │ ─────────> │ Network thread│ ──────────────> │ Audio thread │
│             │ <───────── │ (parse OSC,   │ <────────────── │ (cpal        │
└─────────────┘  replies   │  alloc        │  SPSC garbage/  │  callback,   │
                           │  pre-built)   │  reply FIFO     │  DSP)        │
                           └──────┬────────┘                 └──────────────┘
                                  │ slow tasks (disk, decode)
                           ┌──────▼───────┐
                           │ NRT thread   │  (loading files into buffers, etc.)
                           └──────────────┘
```

- **Network thread**: UDP socket, parses OSC (`rosc`), builds commands *already
  fully allocated* (e.g. the Synth node already instantiated) and pushes them to the FIFO.
  The audio thread just "plugs them in" — O(1), no allocation.
- **Audio thread**: cpal callback. Each block: (1) drains the command FIFO,
  (2) runs scheduled bundles whose timestamp falls in this block, (3) walks the
  node tree in order and runs the DSP, (4) pushes dead memory to the garbage FIFO.
- **NRT thread**: reading/writing audio files for buffers (`/b_read`,
  `/b_write`), like scsynth's "NRT thread".

## Crates

| Crate | Use |
|---|---|
| `cpal` | Cross-platform audio I/O (ALSA/JACK on Linux) |
| `rosc` | OSC 1.0 encode/decode (messages and bundles with timetag) |
| `rtrb` | Lock-free, realtime-safe SPSC ring buffer |
| `basedrop` | Shared pointers with deferred deallocation off the RT thread |
| `hound` | WAV read/write for buffers |
| `assert_no_alloc` | In tests/debug: panic if the audio thread allocates |

## Core data structures

- **`NodeTree`**: a tree of `Group` and `Synth` with integer IDs (ID→node map,
  pre-allocated or slab). Execution order = depth-first traversal, like scsynth.
- **`Synth`**: an instance of a `SynthDef`: a vector of built UGens + wiring buffers
  ("wires") + control values.
- **`SynthDef`**: a topologically-ordered UGen graph, with constants, named controls
  and wire assignment. Defined in our own format (see M3).
- **`Bus`**: global arrays of audio buses (per block) and control buses (one value);
  the first N audio buses map to the hardware outputs/inputs.
- **`Buffer`**: a pre-allocated pool of sample buffers with channel/frames/samplerate,
  filled by the NRT thread.
- **UGen**: a trait with `fn process(&mut self, ctx: &ProcessCtx, inputs, outputs)`
  over blocks of 64 samples; dynamic dispatch (`Box<dyn UGen>`) is fine in v1
  (construction happens off the RT thread; the per-block virtual call is cheap).

## OSC protocol (a subset of scsynth)

The full, current command set is the canonical wire reference in
`docs/schemas.md` (the single source of truth; the `scsynth-osc` skill is the
working map). It is no longer restated here — this plan tracks *what to build
next*, not the shipped protocol.

## Initial UGens

Oscillators: `SinOsc`, `Saw` (PolyBLEP), `Pulse`, `WhiteNoise`, `Phasor`.
Filters: `LPF`/`HPF` (biquad), `OnePole`, `Lag`.
Envelopes/control: `EnvGen` (with done actions: free self, like scsynth), `Line`.
I/O: `Out`, `In`, `ReplaceOut`. Buffers: `PlayBuf`, `BufRd`. Math: binary/unary
operators between signals. See the `ugen-dsp` skill for the algorithms.

## Milestones

- ✅ **M0 — Skeleton** *(done 2026-06-10)* — cpal opens the device and a hardcoded sine plays; the `server`/`dsp`/`osc`/`node` module layout.
- ✅ **M1 — OSC server** *(done 2026-06-10)* — UDP on port 57110 over rosc; `/status`, `/quit`, `/notify`, `/dumpOSC`.
- ✅ **M2 — RT-safe FIFO + node tree** *(done 2026-06-10)* — command/garbage ring buffers and a `NodeTree` with groups; `/s_new`/`/n_free`/`/n_set`, guarded by `assert_no_alloc` in the callback.
- ✅ **M3 — SynthDefs** *(done 2026-06-10)* — the serde def format, the UGen-graph interpreter, `/d_recv`, named/indexed controls, and the `Box<dyn SynthNode>` trait (the F-fork prerequisite).
- ✅ **M4 — Buses and order** *(done 2026-06-10)* — audio/control buses, `In`/`Out`, nested groups, `/s_new` add actions, `/n_before`/`/n_after`, the `/g_*` family, `/c_set`/`/c_get`, `/n_go`/`/n_end`; output is purely `Out`-driven.
- ✅ **M5 — Buffers** *(done 2026-06-10)* — the buffer pool + NRT thread, `/b_alloc`/`/b_read`/…, `PlayBuf`/`BufRd`, async `/done`; buffers are immutable `Arc`s swapped by the engine and freed via the garbage FIFO.
- ✅ **M6 — Sample-accurate scheduling** *(done 2026-06-10)* — a timetag-ordered bundle queue on the audio thread, NTP→samples on the network thread, and intra-block splitting at the event's sample (no `OffsetOut` needed).
- ✅ **M7 — NRT mode + golden tests** *(done 2026-06-11)* — offline WAV render on the same engine (`--nrt`, scsynth score format), golden regression tests and graph benchmarks.
- ✅ **M8 — The sample clock as the client's timebase** *(done 2026-06-12)* — `/clock` exposes the sample counter and `/sched <target> <blob>` schedules directly in samples, so a client models the OS-vs-DAC drift for exact relative timing; NTP and sample-clock clients coexist on the same queue.

## F fork — SynthDefs via Faust (Box/Signal API + JIT)

An alternative path (does not replace M3–M7: they coexist) for building synthesis
nodes: instead of interpreting a graph of our own UGens, the server receives **JSON
that maps to calls of libfaust's Box API (or Signal API)**, compiles to native code
with the LLVM backend (like FaustLive) and hangs the result on the same node tree.
The advantage: the client's "instruction set" is Faust's full Box API —
clients in any language only generate JSON, without depending on our UGen set.

### Changes to the base design this requires

- **Prerequisite in M3**: the tree's synth node must be `Box<dyn SynthNode>`
  (a trait with `process`, `set_control`, `done`), not a concrete type — so
  `UGenSynth` (M3) and `FaustSynth` (F3) are interchangeable in the same tree.
  M3 must be implemented with this trait from the start.
- **Compiler thread** (new, on top of the NRT one): receives compilation requests,
  serializes access to libfaust (its global context is not thread-safe) and publishes
  factories in a shared table (`basedrop::Shared`). JIT compilation takes
  tens-to-hundreds of ms: always asynchronous, never blocks the network or the audio.
- **RT boundary intact**: `compute()` of an already-initialized Faust dsp is RT-safe
  (no allocations); creating/initializing/destroying instances and factories is NOT —
  instantiation on the network/compiler thread, destruction via the garbage FIFO,
  like the current synths.

### F milestones (after M4 recommended; F0 can be done earlier as a spike)

- ✅ **F0 — Toolchain and minimal FFI** *(done 2026-06-10)* — libfaust with the LLVM backend and a hand-written FFI over the C API behind the `faust` feature; smoke-tested against `SinOsc`.
- ✅ **F1 — Compiler thread** *(done 2026-06-10)* — a dedicated compile thread with a refcounted factory table and async `/done`/`/fail` for `/d_faust` (JSON blob; `/d_recv` stays the UGen format).
- ✅ **F2 — JSON → Box API schema** *(done 2026-06-10)* — the schema (composition/math/delays/UI-as-controls), a validating JSON→C-API interpreter with path-carrying errors, and stdlib access via `DSPToBoxes` source fragments.
- ✅ **F3 — FaustSynth in the tree** *(done 2026-06-10)* — `FaustSynth: SynthNode` wrapping the JIT instance, instantiated on the network thread, params by name via UIGlue zones, freed under factory refcount.
- ✅ **F4 — Parity and interop** *(done 2026-06-10)* — Faust and UGen synths coexist in groups/buses, golden-tested equivalent graphs, a Python JSON example, schema docs.
- ✅ **F5 — Extensions** *(done 2026-06-12)* — kept `waveform` embedded tables, the interpreter backend (for the wasm target) and the Signal API; native Faust polyphony dropped (the node tree is the voice allocator); `soundfile` initially dropped then reversed (reads server buffers directly). Rationale in "Reviewed ideas" below.

### Implementation foresight

- **libfaust dependency (not LLVM directly)**: we link against libfaust; LLVM
  comes embedded inside when the build brings the JIT backend. The cost is paid
  one of two ways depending on the consumption mode: *system* libfaust
  (dynamic) keeps the binary light but inherits version fragility —
  the C API (`libfaust-box-c.h`) changed between Faust versions, the bindgen
  headers must match the installed libfaust, which in turn is tied
  to a specific `libLLVM-XX.so`; *vendored/static* libfaust gives a
  self-contained binary in exchange for tens of MB of LLVM inside. F0 measures
  which is preferable; the `faust` feature flag isolates it all from the core.
- **Sample rate baked into each instance's init**: the compiled factory
  is sample-rate independent, but `instanceInit(dsp, sr)` precomputes the
  rate-dependent constants (coefficients, phase increments) once —
  unlike our UGens, which read `ctx.sample_rate` per block. With
  the SR fixed per `engine_pair` run this doesn't affect anything today; it
  becomes relevant only with a hot device change or an NRT render (M7) at
  another SR. Cheap mitigation: re-`instanceInit` (resets state) or
  re-instantiate.
- **Float width**: the JIT picks `FAUSTFLOAT` by flag when creating the factory
  (`-single`/`-double`, default single). Rule: create factories with `-single`
  and assert the factory's float size before using it, to match
  the `f32` buses. If one day f64 were wanted (e.g. NRT mastering),
  the cheap thing is a conversion buffer at the Faust node's boundary — not
  global f64 buses; the option of a parameterizable `Sample` alias
  in the style of the FAUSTFLOAT typedef stays open.

License: this project is GPLv3-or-later, compatible with libfaust
(GPLv2-or-later); the combination is distributed as GPLv3+. Still missing the
`COPYING` file with the verbatim GPLv3 text.

See the `faust-embedding` skill for the C API details and its pitfalls.

## Future milestones (M9+) — additional features

Section added on 2026-06-12 from a list of ideas to review (M8 came out of that
same list). The order reflects dependencies and cost/value, not urgency: M9–M11
are small and independent of each other, M12 enables M13, M14 is independent of
all. At the end, which ideas were dropped and why is noted. Minor directions that
don't reach milestone status (more UGens — `Saw`/`Pulse`/filters, and `EnvGen`
with done actions, **done** 2026-07-01 —, `/g_queryTree`, buffer
streaming) are taken as loose items when needed. (Historical detail for the
completed items lives in the git history.)

- ✅ **M9 — Developer documentation** *(done 2026-06-12)* — `docs/architecture.md` (threads, modules, memory lifecycle, invariants, "how to add a UGen") plus a rustdoc pass; records the Faust-UI-as-controls and the no-dynamic-plugins (no stable Rust ABI) decisions.

- ✅ **M10 — Bounded memory and alignment** *(done 2026-06-12)* — audited and documented every pre-allocated capacity and its full-behavior, cache-line-aligned wire/bus blocks, and updated the `realtime-audio` skill.

- ✅ **M11 — `/n_map`/`/n_mapa`: buses as a parameter source** *(done 2026-06-13)* — a node reads a control (or audio) bus into a control each block until `/n_map … -1` or `/n_set`; an RT-safe per-node mapping table over the existing bus atomics, schedulable like `/n_set`.

- ✅ **M12 — Canonical graph form via bus connections** *(done 2026-06-12)* — infer the read/write dependency DAG from the buses and offer opt-in auto-ordered groups (`/g_sortMode`, recomputed on the network thread; `/g_queryTree`/`/g_dumpGraph`; manual moves `/fail` inside them); feedback cycles keep the explicit order (one block of delay). No plugin delay compensation yet — a natural later addition via a `latency()` on the trait when an intrinsic-latency UGen lands (rationale in `docs/model-vs-daw.md`).

- ✅ **M13 — Parallel tree processing** *(done 2026-06-12; requires M12)* — the M12 DAG drives stage-based parallelism (N−1 RT workers, bounded spin+backoff, no locks); same-stage nodes write disjoint buses so it stays bit-identical to sequential, and NRT renders benefit too.

- ✅ **M14 — Pluggable transports, embedded mode and synchronous calls** *(done 2026-06-12)* — OSC decoupled from transport (UDP / shared-memory ring / in-process), a shared data plane (sample clock + control buses), a client-side synchronous facade, and the versioned-C-ABI cdylib that lets any language embed the server (the "only flat data crosses" boundary).

- ✅ **M15 — Comprehensive English documentation (README + mdBook + rustdoc)** *(done)* — a root README, the mdBook over `docs/` in place (`book.toml src="docs"`), and an oriented crate doc-comment, covering the OSC user, the library/embed user and the developer.

- ✅ **M16 — On-disk def persistence + bitcode cache** *(done 2026-06-16)* — defs persist to a data dir as transparent JSON (layer B) recompiled on reload, plus a non-authoritative libfaust bitcode cache (layer A); `--data-dir`/XDG resolution, `--no-persist`, incremental reload on the compiler thread.

- ✅ **M17 — MIDI: server protocol and client output (reusable Rust core)** *(done 2026-06-18)* — the `clausters-midi` crate (versioned C ABI, MIDI 2.0/UMP via `midi2`, SMF via `midly`, live ports via `midir`): server-side `/midi_bind` maps a channel to an instrument def + control map (SynthDef and FaustDef actuated identically, a MIDI voice byte-identical to the OSC path), and the client exports live ports and `.mid`/MIDI-2.0-clip files. This is client `C11`. Rationale (MIDI-2.0 resolution, native-crate decision, `midi2-clip` stub) in `docs/decisions.md`.

- ✅ **M18 — GraphDef: persistent node-graph definitions ("programs")** *(done 2026-06-19)* — a third persistent def kind saving a whole configuration of bus-wired nodes with a named parameter surface and a shared/per-voice split (`/d_graph`/`/graph_new`/`/graph_voice`); execution auto-ordered via M12, persisted like M16, MIDI-bindable like a def, with a `defs/graphdef.py` client builder.

- ✅ **M19 — MIDI-standalone operation: a playable server with no programming environment** *(done 2026-06-20)* — persisted MIDI bindings and an optional boot preset restored at startup (order: defs → graphdefs → bindings → preset), so a `--midi` server boots already wired and playable with no client; the server-side counterpart of the client's responders (C13).

- ✅ **M20 — Documentation split: two mdBooks (server + Python client) + generated API** *(done 2026-06-21)* — the English docs became two platform books (server `docs/`; the Python client's own book with a pydoc-markdown API page), cross-linked; no RST directives, no milestone labels in published docs.
- ✅ **M21 — Master clock anchor over OSC (shared, drift-free time reference)** *(done 2026-06-22)* — `/clock.reply` carries the server's OSC time anchored to its sample counter, so several OSC clients convert logical time to one common drift-free sample axis and schedule via `/sched` (embed/shm read the counter directly). Pairs with client C14.
- ✅ **M22 — Shared transport: a queryable master beat grid (phase alignment)** *(done 2026-06-22)* — a server-hosted `/transport` (origin sample + tempo) clients join to quantize starts onto one shared grid: sample-exact when locked to a master, beat-accurate otherwise. A separate optional layer over M21; pairs with client C15.

- ✅ **M23 — Continuous integration + publishing (CI, Read the Docs, PyPI)** *(done 2026-07-05)* — a CI workflow (fmt/clippy, `cargo test` over the def-family feature matrix, the gui + wasm gate, the Python suite, both mdBooks, and a from-source libfaust job) and a `v*`-tag release workflow (self-contained wheel + server tarball, PyPI Trusted Publishing). Account-side activation steps recorded in `docs/contributing.md`.
- ✅ **M24 — Real-time health: RT scheduling, CPU metering, affinity, stress harness** *(done 2026-07-08)* — the `rtprio` default feature (RT-scheduled callback with a ground-truth policy diagnostic), an RT-safe CPU meter (avg/peak/late-blocks in `/status`), experimental `--pin`, and `examples/stress.rs`; a SIGXCPU guard degrades instead of dying under overload. The opt-in→default reversal is in `docs/decisions.md`.
- **M25 — TCP as the default command transport + a configurable frame ceiling**: the server accepts TCP alongside UDP **by default** (same port 57110, scsynth-style; `--no-tcp` opts out, `--tcp [port]` still moves it), and the fixed 64 KiB `MAX_FRAME` becomes a boot option (`--max-frame <bytes>`, default 16 MiB) shared by the TCP and WebSocket fronts — with length-prefixed framing the ceiling is a DoS guard, not a protocol limit, so it is configuration, sized for the target deployments (loopback + controlled networks, not a public service). Replies become **transport-aware**: a stream client (TCP/WS/ring) may receive frames up to the ceiling (`/b_getn` chunks, `/g_queryTree`, the `/tap_stream` window clamp), while a UDP client keeps the datagram cap; `/server_info` advertises the ceiling so clients size their requests instead of hardcoding it. UDP itself is untouched — it remains the discovery/boot protocol and carries small real-time control fine; the IPC command rings keep their 64 KiB (big payloads ride TCP even locally; a ring-size option would bump the versioned segment layout and is deferred until a real need). Every network front stays **individually optional** (a packaged desktop/mobile standalone runs the embedded server over the in-process link and needs no sockets at all), and no limit is hard-wired: whatever bounds a payload is a boot option with a sensible default, never a constant — the project must stay usable as a desktop or mobile application without arbitrary ceilings. Rationale — the transport roles: UDP = discovery + small control, TCP = the command plane, shm = the data plane, WS = the browser's TCP — goes to `docs/decisions.md`. Pairs with client C34 and GUI host G25.

### Reviewed ideas: what was dropped and why

- **Denormals** (from the memory/efficiency idea): already implemented post-M7
  (`dsp::denormals::flush_to_zero()` + `-ftz 2` + `tests/denormals.rs`);
  only the skill/documentation part was missing, absorbed by M10.
- **Original F5**: Faust's native polyphony dropped with the rationale in F5
  itself (above); `waveform`, interpreter backend and Signal API are kept, with
  the interpreter tied to the M14 wasm target. (`soundfile` was also dropped
  originally, but the decision was reversed on 2026-06-20 — it now reads server
  buffers directly; see F5.)
- **Faust UI**: the implementation is considered correct — using Faust's labels
  as control names is deliberate (the author of the def picks the names, as in
  the UGen JSON) —; what was pending was documenting the rationale (M9) and the
  generalization "params fed by control buses" (M11).
- **Plugin API**: documenting the internal API yes (M9); dynamic plugins
  not for now — Rust has no stable ABI and scsynth's historical problem
  confirms the cost of maintaining that boundary. The mitigation
  (versioning the binary boundary) is applied where the boundary truly exists:
  the embedded mode's C ABI and the M14 segment layout.

## S track — Synthesis-engine infrastructure completion (the substrate for future UGens)

Section added 2026-07-01. The base UGen set and the node/bus/def machinery are in place, but before *growing the UGen library* we finish the **substrate** every future UGen leans on, so that adding a UGen later is a self-contained job (a `process`, a registry entry, a test) with no engine surgery. Everything here is deliberately **infrastructure, not a UGen catalog**: the concrete DSP UGens it enables — the demand family (`Dseq`/`Dseries`/`Dwhite`), `FFT`/`IFFT` and PV_* processing, the table oscillators (`Osc`/`VOsc`/`Shaper`), more filters — land afterwards as loose items (per "Future milestones (M9+)"), each cheap once the substrate exists. Like the F fork, S coexists with the M line and does not replace anything; the pieces are largely independent (S1 enables S2's `ir` controls and the future demand/FFT UGens; S5 depends on S6's `/b_gen` slot but is written together; S4 pulls `/n_run` in from S6 because it is tied to pause semantics). Canonical scsynth lists below were **verified against the SuperCollider source** (`server/plugins/UnaryOpUGens.cpp`, `BinaryOpUGens.cpp`, `HelpSource/Classes/Done.schelp`) on 2026-07-01.

**Design stance — compatibility of *model*, not literal copy.** These milestones take scsynth's set as the reference for *completeness* (so no capability is missing), but each is free to adopt a **better implementation consistent with what Clausters already built** where scsynth's is weak — the same "conceptual, not binary, compatibility" principle stated at the top of this plan. scsynth carries historical warts we need not inherit: the clearest example is the plugin-command pair **`/cmd`/`/u_cmd`** (S6), which were never cleanly designed (untyped, ad-hoc argument blobs, per-plugin conventions with no schema) — where we need that mechanism we should design a **typed, discoverable** command surface fitting our JSON/OSC conventions rather than reproduce the loose original. Likewise favor our existing machinery (auto-ordered groups, the bus analysis, the network-thread pre-build, the versioned ABIs) over scsynth idioms that predate it. Each S milestone should call out, when it deviates, *why* and *what* it does instead.

- ✅ **S1 — Calculation rates (`ir`/`kr`/`ar`/`dr`) as a first-class property** *(done)* — the output/control rate becomes an explicit, compiler-validated property (with an `ir` init pass and a minimal `dr` demand-pull driver), the substrate for the demand and FFT families.

- ✅ **S2 — Typed controls: `tr`, `lag`/`varlag`, and scalar (`ir`) controls** *(done)* — control types the def author chooses: trigger controls reset after one block, lagged controls insert a shared `Lag` at compile time, `ir` controls freeze at init.

- ✅ **S3 — Special-index operator UGens (`UnaryOpUGen`/`BinaryOpUGen`) + `MulAdd`/`Sum3`/`Sum4`** *(done 2026-07-02)* — two generic op UGens whose `op` is the operator's name over the full scsynth opcode set, computed by the shared `clausters-core` functions for bit-identical client/server parity; `Add`/`Sub`/`Mul`/`Div` stay as aliases.

- ✅ **S4 — Complete the done-action set + `/n_run` (resume) + non-terminal pause** *(done 2026-07-03)* — the full 0–15 `DoneAction` enum (the relative-node actions resolved on the audio thread) and `/n_run`, which makes `PauseSelf` non-terminal.

- ✅ **S5 — Wavetable & table-generation infrastructure (`/b_gen`) + the table oscillators** *(done 2026-07-03)* — `/b_gen` (`sine1`/`sine2`/`sine3`/`cheby`/`copy`) fills buffers through the immutable-`Arc` NRT path, the scsynth interleaved wavetable format, and `Osc`/`OscN`/`VOsc`/`Shaper` as the first consumers.

- ✅ **S6 — Complete the scsynth OSC command set** *(done 2026-07-03)* — the missing node/group/bus/synth/buffer/def/scheduling vocabulary (`/n_setn`/`/n_fill`/`/n_mapn`, `/g_head`/`/g_tail`/`/n_order`, `/c_setn`/`/c_getn`/`/c_fill`, `/s_get`/`/s_getn`, `/b_close`, `/d_load`, `/clearSched`) plus a typed `/cmd`/`/u_cmd` surface and `/error`.

- ✅ **S7 — Boot-time server configuration (audio I/O channels + every pre-allocated pool)** *(done 2026-07-03)* — `--inputs`/`--outputs` (with a real audio-input path via a second cpal stream) and `--max-nodes`/`--max-buffers`/`--max-graph-children`/`--max-ugen-inputs`, chosen at boot and reported in `/server_info`.

- ✅ **S8 — FFT/IFFT and the spectral (`fr`) chain** *(done 2026-07-03)* — `FFT`/`IFFT` + `PV_MagAbove`/`PV_MagBelow`/`PV_BrickWall`, one frame per hop; the transform planned at init (allocation-free per-hop), synth-private spectral scratch, and `/u_cmd` as the live window-swap channel (S6's first consumer).

- ✅ **S9 — Side-effect UGens (no `Out` required)** *(done 2026-07-03)* — `SendTrig`/`SendReply`/`Poll` sending replies out the RT-safe reply FIFO; a valid def may contain only side-effecting UGens (the client relaxation is C19). Write-only UGens (`Out.kr`/`DiskOut`/`RecordBuf`) deferred with the buffer/streaming work.

## Testing strategy

- **Per-UGen unit tests**: offline render of N blocks, asserts on the signal
  (frequency via zero crossings, RMS, impulse response for filters).
- **Golden files**: NRT mode (M7) renders scenes to WAV and compares with
  tolerance against versioned reference files.
- **RT-safety**: `assert_no_alloc` wraps the callback in test builds; CI runs
  the heaviest graph under that condition.
- **OSC integration**: tests that bring up the server on an ephemeral port and
  talk to it with `rosc` from the test; verifiable by hand with `oscsend` or with sclang
  pointing a `Server` at our port. See the `audio-testing` skill.

## Project skills

- `.claude/skills/realtime-audio` — RT thread rules, lock-free patterns, cpal.
- `.claude/skills/scsynth-osc` — reference for scsynth's OSC protocol and node-tree
  semantics.
- `.claude/skills/ugen-dsp` — the UGens' DSP algorithms (oscillators, filters,
  envelopes) with their formulas.
- `.claude/skills/audio-testing` — how to test audio without ears: NRT, golden files,
  signal asserts, no-alloc.
- `.claude/skills/faust-embedding` — embedding libfaust: C API (box/signal/LLVM),
  factory/instance lifecycle, RT boundaries, JSON→Box API mapping.

## Project notes

- The progress made in each milestone is added to the project notes.
- Closing a milestone always includes, where applicable: the developer
  documentation (`docs/architecture.md`, module docs), the user
  documentation in `docs/` for new features, the manual-test steps and
  counts in `GUIA.md`, and an explained example in `examples/` if the feature
  is user-facing — not just the code and the git history.
