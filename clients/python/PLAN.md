# Plan — High-level clients for Clausters, with a shared native Rust core

This plan covers the **Python** client, the reference client. It was written to serve any other language as well, and the JavaScript/TypeScript client — the **web** client, roadmap in `clients/web/PLAN.md` — is built on the shared design recorded here: the same native Rust core and the same C-ABI contract (as wasm in the browser). The language-specific part is only the coroutine driver and the thin binding wrappers.

> **Note — sc3 as the reference model.** For any design or semantics question (module structure, clock/routine behavior, events, patterns, OSC/MIDI interfaces, names, conventions), fall back to [sc3](https://github.com/smrg-lm/sc3) as the model. This client is a clean, pruned rewrite, but sc3 is the source of truth on how these pieces should combine and behave; deviate from it only with an explicit reason (the Clausters-specific parts: FaustDefs, server resources, native Rust core).

## Build strategy — finish one client, then port (keep the seam modular)

"Build all clients at once vs. finish one then port" is a false binary: what makes a port cheap is **where the logic lives**, not the order. The rule for this project, across all clients (Python the reference, the web client in TypeScript, any future language/platform):

- **One reference client at a time.** Finish and polish the Python client first — it is the most mature. Do not grow two full clients in parallel: that duplicates tests and bugs and lets them diverge.
- **Push every language-agnostic piece down into the shared core as you write it** (not "later"). If you find yourself writing protocol/value/time logic inside Python, it belongs in `clausters-core`/`clausters-ffi`. This is what makes the later port mechanical rather than a rewrite. The GUI track proved it: carving the platform seam first (the `host` traits) made the browser port reuse the native code verbatim.
- **Porting reuses the core, never reimplements it.** A new client = rebind the same core (ctypes/N-API natively, wasm in the browser, mirroring `clients/gui`'s wasm path) + add only the parts that genuinely cannot be shared.
- **What is never shared (and should not be forced into the core):** the idiomatic/ergonomic API surface and the concurrency/scheduling model (Python threads + TempoClock vs. JS event loop/async). The core provides the time/value primitives; each language writes its own coroutine driver and ergonomics. See "Guiding principle of the seam" below.

Net: each milestone is built and finished on the reference client, but always factored so that only the thin language-specific shell remains to write per platform.

## Context

Clausters is the Rust audio server (scsynth-style) controlled over OSC. Today the only client in the repo is `clients/python/clausters.py`: the **low-level transport layer** (embed cdylib / shm / render), stdlib-only, with the boundary rule "only flat data crosses" (bytes in, `array('f')`/floats/ints out). There is no high-level layer: building defs, resources, events and sequencing is currently left to the user.

The goal is a **high-level client** that selectively ports the core features of [sc3](https://github.com/smrg-lm/sc3) (a SuperCollider port to Python), but **centered on FaustDefs** instead of SynthDefs, and reusing the server's resources (buses, buffers, generator units). In parallel a **native Rust core** is extracted (TempoClock, numeric builtins, OSC assembly) shared by the server and by every client (Python and the web client today, any language later), so that client-side operations are **numerically equivalent** to the server's by construction wherever possible.

Agreed decisions:
- **Repo**: clean rewrite in `clients/python/` (sc3 as reference, without dragging in SynthDef or the full class library).
- **Rust**: turn `clausters` into a Cargo **workspace** and extract a core crate (`clausters-core`).
- **Binding**: a single **C-ABI** over the core, with thin per-language wrappers (ctypes/cffi in Python; wasm in the web client, which took that door over N-API).
- **Seam**: the Rust core owns builtins, TempoClock (queue + arithmetic) **and** OSC bundle/timetag assembly + conversion against the sample-clock; boundary "only flat data, no callbacks". The **coroutine driver (`yield`) stays in each language** — control flow does not move into Rust.

## Guiding principle of the seam

What is value or time transformation (language-agnostic) lives in Rust; what is the language's control flow (the `Routine`s that `yield` in Python, generators/async in JS, the pattern ergonomics) lives in each language. The loop that resumes the `yield`s is handled by the language: it asks the Rust queue "what's next and when?", sleeps, resumes the routine and returns the next time to Rust. There are no Rust callbacks into the host language — that preserves multi-language portability and the "only flat data crosses" rule.

## Target architecture

### Rust workspace (repo root)

Turn the current single crate into a workspace. Proposed layout:

```
Cargo.toml                  # [workspace] members
crates/
  clausters/                # current server crate (bin + lib + features realtime/faust/embed)
  clausters-core/           # NEW: pure kernels, no I/O, no alloc on the hot path
  clausters-ffi/            # NEW: C-ABI cdylib over clausters-core (the "lib for all clients")
clients/
  python/                   # high-level Python client
    PLAN.md                 # this plan (its design is shared by every client)
```

`clausters-core` (pure library, a `no_std` candidate except where it needs `alloc`):
- **builtins**: unary/binary ops over scalars and over `&[f32]` slices — the same formulas as the server. Base set: `add/sub/mul/div` (already native in the server), and the higher math that today exists in the server only via Faust (`sin/cos/tan/exp/log/sqrt/abs/floor/ceil/min/max/pow/atan2/...`, see `crates/clausters/src/faust/signals.rs`).
- **tempoclock**: time-priority queue + beat↔second↔sample arithmetic, tempo/meter, conversion against the server's sample-clock (read via `/clock_query` or via the shm data-plane).
- **rng**: a seeded generator that **replicates** the server's (`WhiteNoise` uses splitmix64/xorshift, `crates/clausters/src/dsp/noise.rs`) for client/server reproducibility.
- **osc**: message/bundle assembly with NTP timetag, reusing `rosc` (already a server dependency). `timetag ↔ sample target` conversion for `/sched_at` and bundles.

`clausters-ffi`: a cdylib that exports the core's C-ABI (explicit ABI version, like the current embed in `crates/clausters/src/embed.rs`). Distinct from the embed's `libclausters.so` (that is the in-process server; this is the client core). Two separate cdylibs, both consumable by ctypes/N-API/wasm.

### Numeric equivalence — a realistic contract

- Ops the server computes **natively** (`add/sub/mul/div`, `Sine` phase, `WhiteNoise` RNG): refactor the server to use `clausters-core` → **bit-exact by construction** (single source of truth). Mind RT-safety: `#[inline]` functions, no alloc/lock/IO (CLAUDE.md, `tests/rt_safety.rs`).
- Higher math that in the server exists **only via Faust/LLVM** (`sin`, `log`, etc.): `clausters-core` implements the **same formula/semantics** (libm), but bit-for-bit equality with Faust's LLVM codegen is **not guaranteed**. Contract: same formula + documented tolerance; parity tests with tolerance.

### Client package (Python example; the web client mirrors the same structure)

Clean rewrite, mirroring sc3's structure but pruned, and carrying both def
families (SynthDef and FaustDef) as peers:

```
clients/python/
  pyproject.toml
  clausters/                       # high-level package
    __init__.py
    base/                          # selective port of sc3/base
      absobject.py  builtins.py  stream.py  clock.py  main.py
      netaddr.py    _oscinterface.py  _midiinterface.py
    seq/                           # port of sc3/seq: event, pattern, streampatterns
    defs/                          # port of sc3/synth, pruned to Clausters (Faust now, SynthDef later)
      faustdef.py  signals.py  (synthdef.py + ugens.py later)
      node.py  bus.py  buffer.py  server.py
    _native.py                     # ctypes wrapper over clausters-ffi (Rust core)
    transport.py                   # = the current clausters.py (embed/shm/render), relocated
```

- `transport.py`: the current `clients/python/clausters.py` is kept as the transport layer (do not rewrite; it is orthogonal to the core). The high-level package leans on it to talk to the server.
- `_native.py`: ctypes over `clausters-ffi` (builtins, TempoClock, OSC assembly). Flat-data boundary, like `transport.py`.
- `base/builtins.py` + `base/absobject.py`: `AbstractObject`/operands dispatch the ops over a scalar **or list** to `_native` (equivalence with the server). Where per-scalar FFI overhead does not pay off, a pure-language fallback identical in formula.
- `base/clock.py`: `TempoClock` wraps the native queue+arithmetic; the scheduling loop (resuming `yield`) stays in the language.
- `base/stream.py` / `seq/`: coroutines with `yield`, patterns and events — pure Python (in the web client: generators/async).
- `defs/signals.py`: **the user interface for building FaustDefs**. It provides a library of **lowercase callables** (functions or callable objects) that map, in principle, the **Faust Signal API** (`sin`, `cos`, `add`, `mul`, `delay`, `select2`, `hslider`, `rdtable`, …). The **composition** of these callables is what builds the graph: a specification serialized to a **JSON signal tree** now (and a **box tree** later) to send with `/def_send faust` (see `crates/clausters/src/faust/`). Firm design convention: **lowercase names even for objects that act as functions** — a quality that eases programming work in Python (fluent expression-style composition). The **same pattern is reused for UGens** (`ugens.py`, constructors of the SynthDef graph in JSON).
- `defs/faustdef.py`: **the client's center**. It takes the graph built with `signals.py` (or direct Faust source) and produces the def for `/def_send faust` in its three forms (source, JSON box tree, JSON signal tree); it manages controls (UI labels → control names; reserved `out`/`in`). On-disk persistence/cache is handled by the server (M16, bitcode cache). `synthdef.py` (later) does the analogous thing for the UGen graph.
- `defs/{node,bus,buffer,server}.py`: client-side ID allocators (scsynth-style: nodes, audio buses 0..127 / control 0..1023, buffers 0..1023), handling of `/done`/`/fail`, `/server_notify` → `/node_start`/`/node_end`. NRT: score → transport's `render()`.
- **To port later from `sc3/synth`** (remember): `synthdef.py` + `ugen.py` (client representation of the UGen SynthDefs), `synthdesc.py` + `spec.py` (control specs; in Clausters only `InCtl` exists as a control UGen), `_graphparam.py` (adapts Python types to the types the nodes receive; reviewable, need not be identical, deferrable).

### Separation of responsibilities: server-agnostic client vs server representation

There are **two groups of abstractions** that must stay well separated (see memory `separacion-cliente-servidor-clausters`):

1. **Server-agnostic** (knows nothing about transport or the server app): **timing** (`base/clock.TempoClock`), **sequencing** (`base/stream`, `seq`) and **JSON graph generation** (`defs/signals`, `defs/faustdef`, `base/absobject`, `base/builtins`).
2. **Representation + configuration of the Clausters server**: the **`Server`** class (`defs/server`) = the running server; the **resource handles+allocators** (`defs/node`, `defs/bus`, `defs/buffer`); and the **communication interface** the `Server` owns. Choosing communication over **shared memory** or **embed** = adding a **new communication interface** to the `Server`.

Correspondence with the Clausters server:

| Python (client) | Represents | Counterpart in the server |
|---|---|---|
| `defs/server.Server` | the running server + its communication | the `clausters` process (OSC/UDP; later shm/embed) |
| `defs/node` (`Synth`/`Group`) | handles + id allocator | `src/node` (node tree) |
| `defs/bus` (`Bus`) | audio/control buses + allocator | buses in `src/dsp` |
| `defs/buffer` (`Buffer`) | buffers + allocator | `src/dsp/buffer` |
| `base/clock`, `base/stream`, `seq` | timing and sequencing | — (agnostic) |
| `defs/signals`, `defs/faustdef` | client-side JSON graph | `/def_send faust`, `src/faust` |

### Target interfaces and time handling (RT / NRT / MIDI) — the central piece

The point that makes **one and the same clock-and-routine logic** serve real time, deferred render and MIDI without rewriting it. The correct split is:

- The **clock** (`base/clock.TempoClock`) only schedules and provides time (beat↔second↔sample math via `_native`, scheduling queue, RT/NRT drives, resuming `yield`). **It does not communicate with the server.**
- The **`Server`** owns the **target/communication interface** and **emits** the events, computing the timetag from the logical time of the running routine's clock (`main.current_tt`). Changing the interface changes *where* and *in which mode* (live vs deferred) the events go; clock and routines do not change.
- `base/_oscinterface.py`: `OscUDPInterface`/`OscTCPInterface` (RT; TCP not in the server yet) and `OscNrtInterface` (accumulates into `OscScore` → `render()`). `base/_midiinterface.py`: `MidiRtInterface`/`MidiNrtInterface`+`MidiScore`. shm/embed would be additional communication interfaces of the `Server`.

> **Post-C3 correction:** in C2 the communication ended up **misplaced in `TempoClock`** (fields `target`/`interface`, methods `send_bundle`/`send_msg`/`_emit`/`_when`). Milestone **C4** moves it to `Server`. The clock keeps only timing.

## Milestones (client "C" track, parallel to the server "M" track)

> Markers: **✅ done** · **⏳ pending** · milestone **unmarked** = future, not started.

- ✅ **C0 — Workspace + core + FFI**: convert to a workspace; create `clausters-core` (builtins, tempoclock, rng, osc) and `clausters-ffi` (C-ABI + version); refactor the server's native ops to consume `clausters-core`. Server↔core numeric parity (bit-exact native, documented tolerance vs Faust); RT-safety intact.
- ✅ **C1 — Client scaffold + accessible core**: `pyproject.toml`, `clausters/` package, relocate transport, `_native.py` (ctypes over `clausters-ffi`). Smoke: builtin, `TempoClock`, OSC bundle, `render()`.
- ✅ **C2 — base**: `absobject`/`builtins`, `stream` (Routine/Stream with `yield`), `main`, `clock`, `netaddr`, and the swappable `_oscinterface`/`_midiinterface` targets so clock and routines emit against an interface.
- ✅ **C3 — Faust-first defs**: `signals.py` (lowercase callables mapping the Faust Signal API → JSON signal tree), `faustdef` (the three `/def_send faust` forms + controls), and the `node`/`bus`/`buffer`/`server` allocators. E2E vertical slice: `signals` → `faustdef` → `/def_send faust` → `/synth_new` → control.
- ✅ **C4 — Refactor: client/server separation** (post-C3 correction): pull communication out of `TempoClock` into `Server`. The clock keeps only timing; the `Server` owns the one **communication interface** (RT: UDP, later shm/embed; NRT: score → `render()`) and emits, reading time from the running routine's clock. A transport change is a new interface on the `Server`, never a change to clock/seq.
- ✅ **C5 — seq**: `event`, `pattern`, stream-patterns; one `Pbind`+`TempoClock`+`Server` runs RT or NRT just by changing the `Server`'s interface. Design points that hold as invariants: beats advance **only via `yield`** (the monotonic clock only computes sleeps → exact relational timing); `main.current_tt` is **thread-local** so several clocks and RT-beside-NRT coexist in one script without clobber; a selectable timebase (`MonotonicTimebase` vs `SampleClockTimebase` emitting `/sched_at`); a byte-identical score-parity golden; and `Session` as the explicit no-globals context. The C5 leftover closed the **instance-based** UGen graph (`defs/ugens.py` + `defs/synthdef.py` → `/def_send synth`), byte-identical to the internal `default`.
- ✅ **C6 — UDP sample-clock anchoring**: `defs/clocksync.py` — a `SampleClockModel` (least-squares fit over a sliding window of `/clock_query` anchors) and `UdpSampleClock` (background anchor/track), so `SampleClockTimebase` works live over UDP without shm/embed and the `Server` emits via `/sched_at` anchored to the server's clock.
- ✅ **C7 — MIDI interfaces** (re-planned → **C11**): the first sketch (MIDI 1.0 in a Python library) was poorly planned and redone as a reusable native crate for client+server (MIDI 2.0/UMP); moved out of the sequential track to C11.
- ✅ **C8 — TCP interface** (both ends): server `--tcp` accepts length-prefixed OSC (scsynth framing) multiplexed in the single-thread loop with no async runtime, `ClientId::Tcp` routing replies per connection; client `OscTCPInterface` is a UDP drop-in with reply reassembly. Timing still rides timetags/`/sched_at`, so arrival latency never affects when a scheduled command fires.
- ✅ **C9 — multi-language + close-out**: documented the cross-language architecture (`docs/clients.md` — the single C-ABI contract, the Python layers, the path to a JS client) and a commented `examples/sequencing.py` tour of the sequencing layer across the NRT/live seam. Confirms the boundary is not Python-specific.
- ✅ **C10 — Documentation and examples maintenance**: keep the mdBooks, `clients/python/README.md`, the smoke checklists and the examples current as milestones land (an ongoing duty, not a one-time task).

## Future milestones (client "C" track, parallel to the "M" track)

Client milestones **with no fixed sequential order**, to be tackled when appropriate (just like the server's "Future milestones M9+" in the root `PLAN.md`). They are numbered after the last of the sequential section (C10).

- ✅ **C11 — MIDI interfaces** (moved from C7): complete `_midiinterface` — `MidiNrtInterface`/`MidiScore` writing `.mid`/clip files and `MidiRtInterface` for live output, mapping `Event` → standard channel-voice MIDI, over the same RT/NRT seam the `Server` owns. MIDI lives in the reusable `clausters-midi` crate (MIDI 2.0/UMP), not a Python-only library. Rationale in `docs/decisions.md`.
- ✅ **C12 — Python client packaging (wheels)**: the `clausters` package as a pip-installable wheel bundling the native cdylibs, via a `setuptools` build hook (`setup.py` + `build_native.py`) that runs `cargo build` and stages them; `_libpath.py` gives a shared loader precedence (env override → bundled copy → workspace `target/`). The `clients/python/examples/` split landed here. The Python client's dedicated mdBook (M20) landed alongside.
- ✅ **C13 — Responders (OscFunc/MidiFunc) + general-purpose OSC/MIDI I/O**: the client's **input** path and role as a general MIDI/OSC hub (sclang's `OSCFunc`/`MIDIFunc`) — receive OSC/MIDI from any app on a dedicated demux thread, dispatch to callbacks scheduled on a clock (never blocking it), and emit to the server or other apps. Adds MIDI input to `clausters-midi` and an OSC receive socket client-side; convenience responders turn notes into `/synth_new`. Single global transport pushes on change to `/server_notify` clients.
- ✅ **C14 — Client clock lock to a master server + MIDI timing from OSC time** (client side of M21): `TempoClock.lock_to(server)` / `Session.lock_to_server()` switch the clock to the server's `SampleClockTimebase` (events by `/sched_at`), with graceful fallback to wall-clock OSC time when no master answers. Governing principle: the timing reference is orthogonal to the destination and the default never needs a Clausters server. Named `lock_to` (not `sync`) to avoid colliding with `Server.sync()`. MIDI derives jitter-free ticks from OSC time, never the sample clock. Documented at length in the Python book's timing page.
- ✅ **C15 — Phase alignment: `quant` + joining a shared transport** (pairs with M22): honor `quant` in `TempoClock.play` (snap the start to a beat boundary) and `TempoClock.join_transport(server)` to adopt the server's `/transport_set` grid. Clients sync in beats in plain OSC mode and sample-exact when also `lock_to` a master. Multi-client example lands two clients on the same bar.
- ✅ **C16 — Static timelines + a playhead (random-access, DAW-style transport)**: `seq.timeline` — a `Timeline` (static, editable, sorted `(beat, item)` sequence with random access) and a `Playhead` (play/stop/locate/loop + song position). Items are anything with `play(destination)` (`Event`, raw `OscEvent`/`MidiEvent`); the seekable counterpart to the forward-only routines. The deferred server-broadcast transport landed too: `/transport_play|stop|locate` push to `/server_notify`, and `Playhead.follow_transport` rolls every client's playhead in lockstep — the server broadcasts control, never audio.
- ✅ **C17 — Embedded server as a first-class destination + one self-contained wheel**: `OscEmbedInterface` makes the in-process embedded server just another OSC destination (same bytes as UDP, delivered by function call), and `Session.embed(...)` is the RT factory twin of `nrt`/`live`. The wheel ships client + embedded server + standalone server binary in one `pip install` (no optional extras — extras cannot gate files inside a wheel).
- **C18 (deferred) — cross-platform precise MIDI timing via in-band MIDI 2.0 (client ↔ server)**: decision with the user — do *not* chase OS-specific hardware-timestamp scheduling (ALSA timed queue / CoreMIDI host-time, which `midir` does not expose). Live OS MIDI output stays **best-effort by design**; exact timing already lives in OSC (`/sched_at`) and the NRT/SMF export. The direction worth doing later is a **MIDI 2.0/UMP channel over our own transport** (OSC/UDP/shm, no system MIDI libraries): both ends are ours, so messages can carry sample-accurate timing in-band, feeding the server's existing MIDI actuation. No date.

- ✅ **C19 — Output-less SynthDefs: the def/tree builder must not assume an `Out`** (client side of S9): `SynthDef` now takes graph **roots**, not "outputs" — a root may be a side-effect UGen (`SendReply`/`SendTrig`/`Poll`, disk/buffer writers, self-control), matching what the server already permits. Added the `send_trig`/`send_reply`/`poll` builders; such a synth is a pure sink/observer in the bus analysis.

- ✅ **C20 — Spectral chain builders + shared smoothing windows** (client side of S8): the `fft`/`ifft`/`pv_*` graph builders so a `SynthDef` can build an `FFT`→`PV_*`→`IFFT` chain, plus `Server.u_cmd` for the FFT window swap. The smoothing windows are shared through the FFI (same coefficients the server's `FFT` applies) for binary parity.

- ✅ **C21 — Seam audit: push remaining value/time logic down to the core (pre-W)**: the audit the build strategy calls for before porting — every value/time computation still in Python moved to `clausters-core`/`clausters-ffi` (the beat queue → core `Scheduler`, the sample-clock fit, the seeded RNG, NTP packing, seconds→sample rounding, `quant` snapping, degree→midinote) so the TS port is mechanical. The OSC byte codec stays per-language (documented exception). Follow-up: per-pattern seeds removed in favor of one seedable context (see `docs/decisions.md`).
- ✅ **C22 — Python box API: Faust's box algebra, libraries included**: `clausters.defs.boxes`, the box counterpart of `signals` — Faust's point-free algebra (`seq`/`par`/`split`/`merge`/`rec`, `wire`/`cut`, controls, tables) as lowercase callables emitting box-tree JSON, plus `faust(src, ...)` to compile any Faust expression (and its libraries `fi.`/`os.`/`re.`/`pm.`) into a composable `Box`. Two server fixes came out of it: `CDSPToBoxes` fragment memoization (duplicated stateful fragments were defeating hash-consing) and running the compiler inside `normal_precision` (NRT's FTZ render thread aborted libfaust's interval typing) — both recorded in `docs/decisions.md`.

- ✅ **C34 — Client transport defaults: probe over UDP, commands over TCP** *(done 2026-07-12; pairs with server M25 and GUI host G25; numbered after the arrangement arc's C33)*: `Server.boot` / `Session.live`/`gui` keep the UDP *boot-or-attach* probe (discovery stays zero-config) but connect the command interface over `OscTcpInterface` **by default**; every place a server interface is constructed takes an optional `transport=` (`"tcp"` default, `"udp"`, `"ws"`) so constrained setups can opt down and remote/browser setups opt across — the in-process `OscEmbedInterface`/shm paths are untouched and remain the natural link for a packaged desktop/mobile standalone (no sockets required at all). An oversized send on a UDP interface fails early with a clear error naming TCP, instead of a cryptic OS `EMSGSIZE`. Bulk reads size their chunks from the ceiling `/server_query` advertises — `fetch` moves ~1 MiB per round-trip over a stream transport instead of 1024-sample datagram chunks. The clock-sync pinger stays UDP by design (tiny, latency-sensitive packets, and datagram loss only costs one sample). Docs: the Python book's connection page explains the two roles (UDP finds the server, TCP talks to it).

- ✅ **C35 — The default session + a free-standing `play` (usable without a `Session`)** *(done 2026-07-14)*: the ambient one-liner, sclang-style but contained. One rule governs it — *what does not run in an explicit `Session` runs in the default session* (`clausters.default_session`, the `main` singleton). The default session now holds what were scattered "globals": a default `server` (adopted **first-wins** by a free-standing `Server.boot()`; an explicit `Session` never adopts), an opt-in default clock (created and started on first use), and the random context. `main.resolve_server`/`resolve_clock` are the single resolution — explicit arg → the running routine's session (`current_tt.clock.session`, a new back-reference) → the default session — so isolation holds even for the ambient verb (a play from inside a session resolves *that* session). A free-standing `clausters.play(x)` plays anything (an `Event`, an event `Pbind`, a `Routine`), and every playable's `.play()` takes the same ambient defaults; `Event.play()`/`Pattern.play()` gained optional, ambient-resolving arguments (positional order preserved). Outside a clock a note plays **immediately** (`/synth_new` untimetagged) and self-releases via a bundle at wall-clock now + sustain (tempo 1.0), through the new clockless `Server.send_bundle_after`; inside a routine both stay timetagged at the logical beat. So `Server.boot(); Event().play()` is the whole setup. Docs: getting-started leads with the two-line path, the Sessions page gains a "default session" section; example `examples/basics/hello_note.py`. Follow-up (same arc): each `Session` is now its **own random context** — the RNG root (`seed`/`rng`) moved to a shared `RandomContext` base that both the default session (`Main`) and `Session` extend, and a thread-local `current_session` (set while a session plays/renders or as a `with` block) routes the root of material created outside a routine. So `session.seed(n)` reproduces *that* session independently — seeding one never perturbs another, and material is independent of the order sessions were built in (`main.seed` now governs only the default session). Tests: `tests/test_session_rng.py`. Second follow-up: the shared notion is named — a new `clausters.base.environment.Environment` base (server + `RandomContext`) that **both** the default session (`Main`) and `Session` extend, so `default_session` literally *is* a session (the one used when none is named); the default-only roles (the thread-local execution registry, the resolution authority, the opt-in default clock) stay on `Main`. `isinstance(main, Environment)` and `issubclass(Session, Environment)`.

- ✅ **C36 — The free-standing `plot` (with GUI host G26)** *(done 2026-07-14)*: the visual sibling of C35's `play` — one verb, one **individual window per call**, resolved against the ambient context (the session's GUI host if one is up, else a standalone host `plot` boots lazily with no client leg: plot data reaches the host as a mapped file, so no audio server is involved unless the object needs one). Dispatch by kind: a **def** (`SynthDef`/`FaustDef`/`GraphDef`, with `defs=` carrying a graph's member defs) is rendered by an **ephemeral NRT session** (sent at score time 0, instanced with `controls`, freed at `dur`) and its output plotted, every channel a lane; an **`Env`** renders through the engine's own `EnvGen` (gate-released at its sustain point) so the drawn curve is what the engine plays; a **`Buffer`** is fetched live with its shape and rate; any other **iterable of numbers** (a list, a `Pseq`/`Pwhite`, a stream) is materialized up to `n` and plotted as a sequence with the value axis auto-fitted — the non-normalized-range case. `view="spectrum"` plots the averaged magnitude spectrum (the G26 host analysis). The returned `PlotWindow` retunes the display live (`set(view=…, min="auto", …)`) and closes it. Small data rides inline; anything larger goes through a temp raw-f32 file (removed at exit). Docs: the Sessions page's "Plotting a signal" section; example `examples/views/plotting.py`; a manual smoke step. Tests: `tests/test_plot.py` — dispatch, tree building against a fake host, and the NRT def/env/graph render paths.

- ✅ **C37 — The free-standing `scope`: real-time views of a live bus** *(done 2026-07-16)*: the real-time sibling of C36's `plot` — the three tap-fed instruments the host already carries (the G18 oscilloscope, the G19 phasescope and live spectrum), reachable in one verb. `clausters.scope(bus, view="signal"|"phase"|"spectrum")` resolves the ambient live server (`main.resolve_server`) and the ambient GUI host — the same owned host `plot` uses, here booted **wired to the server** (address + `shm` segment, the native tap read path; an owned host booted leg-less is rebooted wired when a leg is first needed); a `server` handle without a segment fails early with guidance (pass `host=` for an attached or browser host). The tap indices come from a new **client-side tap registry** (`Server.taps`, a `TapAllocator` over the core occupancy map, sized from `ServerOptions.taps` like the bus allocators — S10 spirit: freed runs reuse, double free and exhaustion raise), so two scopes never fight over one ring; the phase view takes a run of **two adjacent** taps for the stereo pair `bus`/`bus + 1`. The verb routes each tap (`/bus_tap`), opens the window and returns a `ScopeWindow` whose `set` retunes the display live and whose `close` releases everything — `/bus_tap … -1`, the registry run, the window. Host side, the `spectrum` widget's `log_freq` grew into `freq_scale` = linear/log/mel/bark through the shared `display_to_hz` geometry (the G20b move; the boolean stays as a legacy alias in parse and `/gui_set`), so the spectroscope's axis matches the spectrogram's and is retunable live. Docs: the Sessions page's "Scoping a live signal" section; example `examples/views/scoping.py`; a manual smoke step. Tests: `tests/test_scope.py` — the tap registry (recycle/adjacent pairs/misuse), per-view tree building and tap release against fake host/server; the host-side scale in the gui crate's widget tests. *(Follow-up: GUI G28 generalized the verb to `channels` consecutive buses — multichannel lanes/overlay, axis rulers, a visible trigger — and its docs to a brief user manual.)*

### The arrangement model + the multitrack editor (client arc, phased)

The recursive-granularity composition/editor track: a client-side **arrangement
model** (`clausters.form`) — elements placed in time, grouped recursively,
rendered onto the server (RT/NRT) — a GUI multitrack widget, and the bridge
between them. (*Element* is the layer's general term for its contents:
everything is an element, in one of two modes — *generated* data, or the
*generator* that renders it.) Each phase ships incrementally.

- ✅ **C23 — Automation as a control vector** (editor Fase 0 B): a break-point curve is discretized into a control buffer on the server (`/buffer_gen "env"`, reusing `clausters_core::envshape`) and read back onto a control bus by a lane synth (the new `OutCtl` UGen, symmetric to `InCtl`). Client `seq.automation.Automation` prepares the buffer without blocking the clock and `play()`s the lane + `/node_map` + free; the List↔Buffer duality of the arrangement made playable/editable, reusing the bpf editor (G21) for the curve.
- ✅ **C24 — High-level buffer I/O wrappers** (editor Fase 0 A): `Server` gains `read_buffer`/`read_into`/`write_buffer`/`zero_buffer`/`gen_buffer` (score-aware) and the synchronous `query_buffer`/`get_samples` over the existing `/buffer_*` surface — the audio-clip element (load from file, generate, query, fetch, export). No server change; buffers are loaded or generated, never push-filled.
- ✅ **C25 — Arrangement model core** (editor Fase 1A): `clausters.form` — a `Element` base carrying temporal metadata (onset/duration and the derived temporal *character*), the five primitives (`Clang`/`Sequence`/`Vector`/`Track`/`Generator`) as **thin wrappers** delegating `play(destination)` to the objects the client already has, and `Aggregate` (concrete/logical) placing elements recursively by offset with the derived temporal *relation* (successive/simultaneous/mixed). Pure and transport-agnostic; rendering onto the server/NRT is the next phase.
- ✅ **C26 — Concrete rendering** (editor Fase 1B): `clausters.form.render` — the *change of state* to sound. An `Aggregate{concrete}` is **flattened** (a tree-walk accumulating nested placement offsets into absolute beats) into a flat `Timeline` whose items follow the `play(destination)` seam; a contained event pattern is *bounced* in the same pass (reusing `Timeline.from_pattern`), a `Track` is shifted, an abstract element yields no event. `Element.render(destination, clock)` plays that timeline through a `Playhead` — RT or NRT, sample-identical, no new path. Tests: pure flatten of nested offsets, and NRT byte-equivalence to a hand-built timeline. `Aggregate{logical}` rendering, a `Vector` clip and def instancing are deferred to Fase 1C / later.
- ✅ **C27 — Logical rendering → GraphDef** (editor Fase 1C, closes Fase 1): a `Group{logical}` translates 1:1 onto a `clausters.defs.GraphDef` (the bus-wired configuration the server already expresses) — each `Generator` member becomes a wired member (its `def_name`, `controls` — numbers, an internal bus name, or `"OUT"` — and `maps` for `/node_map`), the group's `buses` the private internal buses. `Group.to_graphdef()` builds it (reusing GraphDef, not reimplementing) and `render()` sends (`/def_send graph`) and instances it (`/graph_new`) on the server. The bidirectional edit-back stays for Fase 3. Test: a two-node source→sink group yields the same GraphDef spec as the hand-built one, plus render routing.

- ✅ **C28 — The multitrack editor driver: the arrangement → GuiDef** (editor Fase 3A): `clausters.gui.editor.Editor` — the forward half of the bridge. It renders a `clausters.form` tree into the multitrack view: the root `Aggregate`'s members are the **lanes**, a lane's members its **clips** (a `Vector` names its server buffer and spans its frames; an element of clangs draws a **piano-roll**, bouncing a contained pattern in the same pass — the *change of state* made visible; a nested `Aggregate` draws as the labeled rectangle that summarizes it, until `expand`ed into lanes of its own — the arrangement's **base level** as an edit). Two decisions carry it: the dependency arrow is **gui → form** (the arrangement stays pure and transport-agnostic, as `points_to_env` moving out of `guidef` already established), and the **unit bridge lives here** — the arrangement places in beats, the view in timeline samples (a clip's body *is* audio data, so its sample 0 sits at the offset), so one beat is `sample_rate / tempo` units and a musical `quant` becomes the lane's drag grid; the arithmetic is the core's (`beats_to_secs` → `secs_to_samples`), not a second implementation. Pure and host-free: the draw is a function of the arrangement. `Aggregate.handles` exposes the stable member identities the clip registry keys on.

- ✅ **C29 — Clip edit-back onto the arrangement + re-rendering** (editor Fase 3B): the return half of the bridge — the loop *data ↔ graphic ↔ sound* closed. `Editor.apply` resolves a `/gui_event <id> "clip" <offset> <dur>` (the drag/resize payload) through the clip registry and writes it onto the arrangement with `Group.move`; `poll` drains a whole host stream into it (from the script's loop, never the clock thread). Two conversions are the substance: a clip's offset is **absolute** on the shared axis while a placement is **relative** to its group, so the position converts back through the base the clip was drawn at; and only what actually *moved* is written — a drag carries the clip's unchanged `dur` along, and snapping that to the grid would silently reshape the element. `render(destination, clock)` plays the arrangement through its own `render` (no new path) and anchors every lane's playhead to the engine clock; `rerealize()` re-schedules the edited composition from the playhead's position (honest semantics: *re-schedule from here*, not a sample-exact splice — a sounding synth keeps sounding), and `follow=True` does it on every edit (the live editor). Tests: the arrangement gets the beats the clip was dropped on (grid-snapped), a nested clip converts through its base, a move leaves the length alone, render→apply→render is a fixed point, and the edited composition's NRT score starts where the clip was dropped.

- ✅ **C33 — Attach an envelope to the event it shapes**: the algebra already had the answer, and the editor now reads it. An `Aggregate` whose members **start and end together** — its derived temporal relation is *simultaneous* — is one thing on the timeline, so the editor draws it as **one clip with layered bodies** (the curve over the note) and drags it as one: the voice cannot outlive its envelope, and the envelope cannot be left behind. Each body keeps its own value axis, and a curve edited on such a clip finds the automation inside the aggregate. An element is also named for **what it is** (an automation is an *envelope*, named for the control it drives — not the `Element` that happens to wrap it).
- ✅ **C32 — The editor's transport** (with G22i): `Editor` owns the transport the multitrack view implies — `position` (in beats, live while playing), `play`/`pause`/`stop` and `locate`, plus `extent()` (the composition's length, **read from the arrangement**, so dragging a clip past the end lengthens the piece and it plays to its new end — a fixed length used to cut playback short). A `"locate"` from the host (a click on a lane's ruler) seeks the playhead: playing, it re-renders from there (so a seek also applies a pending edit); stopped, it moves the cursor the lanes draw. The two playheads stay distinct: the clock-anchored line that sweeps while playing, and the static cursor of a stopped transport.
- ✅ **C31 — A placement's length is what you hear of it**: rendering honors the *placement* `dur`, not only the element's own — a clip's length **trims** what it plays (events past the placement's end are dropped; a single-event element sounds exactly that long, through a copy — a placement never rewrites the element it places). Resizing a clip in the editor was until now a purely graphical act. Alongside it, `Editor.dirty`: an edit no longer interrupts what is sounding, it *marks* the composition, and the next transport action (a play, a resume after pause, a rewind/seek) re-reads it — rendering always re-flattens the arrangement, so it plays the clips where they now are. The editor also keeps its clip registry truthful after an edit, so a second drag measures against the current placement, not the drawn one.
- ✅ **C30 — The composition loop, end to end** (editor Fase 3C, closes Fase 3): the example, the docs and the last gap in between. A `Vector` element now **renders**: a buffer is data, so it sounds through the def *named to play it* (`Vector(buf, instrument="take")`, whose event carries the buffer number in a `buf` control and sets `legato = 1` so the take sounds its whole length) — the "needs an instrument, later phase" note from the compositional rendering, closed on the arrangement's own terms (no built-in sampler def; see `docs/decisions.md`). `Editor.window` lets a script's loop notice the window closing, and the editor's widget ids start clear of the host's auto-assigned window ids. Example `examples/editors/composer.py`: a take bounced offline and loaded from disk, a melody and a bounced pattern, composed, rendered as a multitrack window, dragged and re-rendered on the fly. Docs: the Python book gains a composition page (the arrangement's elements, grouping, rendering, and the editor's mapping and unit bridge); a manual smoke step for the audible/visual loop.

- ✅ **C38 — The session document: concrete material with provenance** *(opened 2026-08-13 with the GUI host's H track; **relocated 2026-08-14 to `crates/clausters-document/PLAN.md`, O8**, and kept here as a pointer; **O8 shipped**, which is what closes this)*. The decision that fixed its contents is unchanged and is the one worth reading twice — **an algorithm is never serialized, the way a project file never serializes a plugin**: a document holds concrete material, configuration (a generator's own settings as an opaque blob it never interprets) and provenance, the reference to the scripts that generated it. What moved is where it is implemented, and why: the `standalone` host is the other writer of the same format, and a format with two writers in two languages is a format that drifts — so the document became a crate every mode links, and this client binds it rather than defining it. The Python half that remains is a refactor rather than a format: **O8 landed the format and O10 the binding**, so `clausters.form` already writes and reads the crate's document losslessly and id-stably (`to_document`/`from_document`, `to_session`/`from_session`), and an edit already goes through the crate's own `apply` (`_native.document_apply`) rather than through a second implementation here. What is still this client's own is the **object model** - `Aggregate`, `Element` and the rest are Python objects with a conversion, not accessors over a shared tree. That is what the crate's decisions asked for (*the clients round-trip; they do not hold handles*), so this stays open as a refinement with no consumer waiting on it rather than as a gap.

  **Two writers, one format, disjoint subsets.** The client writes all of it; a `standalone` GUI host writes what its own editing can hold, as any program does. Which forces the rule that keeps that safe, and it is the same invariant the widget protocol already runs on (an unknown widget is laid out, not painted, and never dropped): **what a writer does not understand, it preserves**. A session authored from Python, opened in the standalone editor and saved back, must come out with its generator references intact — losing them there would lose the piece. The one concurrency guard needed: a save carries the version it was loaded at and is refused if the file moved underneath, which turns a silent overwrite into an error; no locking, and no merge machinery, because the two writers are two deployments and not two live editors.

  **Boundaries, recorded so the milestone does not grow:** the document is not the undo history (that is the host's, per session, never written — GUI H track), a save is not an undo boundary, and the document does not persist logical material even when it can point at it. Python: `form` reads and writes it, the `Editor` opens and saves one; `docs/decisions.md` for the plugin analogy and the preserve-what-you-do-not-understand rule; the client book's composition page for the user-facing half; an example that saves a composition, reopens it and re-renders.

### The client API reform: the GUI element as an object (client arc, phased)

A reform of the client's GUI surface, in two parts: **first the shape of the
objects** (a builder returns a thing, not an anonymous `dict`), **then their
interaction with the audio server** (a widget takes the control it drives).
Both are done **with the host and the wire as they are** — every milestone here
is client-side. Where one of them wants something the host or the server does
not offer, that is named as a limit and left to the GUI/server tracks rather
than smuggled in.

The premise, in one line: a `SynthDef` is an object that builds a JSON AST and
knows how to send itself; `guidef.window()` returns a bare `dict` and knows
nothing, which is why the host had to become the subject of the sentence
(`host.open(tree)`) while every other resource is its own subject
(`sd.send(server)`, `plot(obj, host=None)`). Giving the GUI node the same shape
the def already has removes the asymmetry rather than papering over it.

#### Part 1 — the objects

- ✅ **C39 — `View`: a GUI node is an object that carries a tree** *(done
  2026-08-23)*. Every builder in `clausters.gui.guidef` returns a `View`
  (`clausters.gui.view`) instead of a `dict`. `View` **is a `dict` subclass**, so
  the JSON it produces is byte-identical to today's and neither the wire, the
  host, nor `to_json` changes; what it adds is behaviour: the name index
  (`find`/`names`), `to_json()` and `open()`, with composition by nesting
  unchanged at the call site. The UGen analogy is the one to keep: a `View` is a
  node of an AST that a program builds and then sends, not a live widget — the
  live widget is what `open()` returns.

  ```python
  # The builders compose exactly as they do today; the value is now a View.
  v = window(
      layout(
          knob(name="freq", label="freq", min=110.0, max=880.0, value=220.0),
          slider(name="amp", label="amp", min=0.0, max=1.0, value=0.2),
          flow="col"),
      title="voice")

  v.to_json()      # the same document /gui_def already takes
  w = v.open()     # a live window, on the ambient host
  w["freq"].set(value=440.0)
  ```

  **The bracket is the dict key, not the name.** The plan first asked for
  `View.__getitem__(name)`; that collides head-on with the thing `View` *is* —
  `node["type"]`, `child["id"]` and `node.get("children")` are read all over the
  client and by the host's own id walk, so one bracket cannot mean both. The
  document is addressed by key (`v["min"]`) and the tree by name (`v.find("freq")`,
  `v.names()`). On the **live** side the bracket stays the name (`w["freq"]`),
  where there is no document to collide with — a `WindowHandle` indexes nothing
  else.

  **The name is a client-side index, and it must stop being an id.** The host
  does not read a widget's `name` at all (`to_json` strips it before the document
  goes out; only the *root*'s survives, and there it means "persist this def"),
  so `w["freq"]` is a table the client builds by walking the tree. Two
  consequences landed with it: a **duplicate name in one scope is an error**,
  raised both where the `View` is built and where `GuiHost._register` walks a
  hand-written tree — not the silent last-wins of before, which left the shadowed
  widget drawing and unreachable (the project's recorded stance on silent
  shadowing is `src/osc/server/dispatch.rs`); and a **nested `View` scopes its
  names**, so two sub-views can both hold a `freq` and `v.find("osc1").find("freq")`
  reaches one of them. (Open: whether a flat `"osc1/freq"` path is offered as
  well, and how a `WindowHandle` indexes a nested view — the handle's map is
  still flat, which no tree exercises yet because nothing nests a `window`. It
  will once `view()` lands in C41.)

  **The ambient host, made symmetric with the ambient server.** `open()` with no
  `host=` resolves the way `plot`/`scope` already did, and two adoptions were
  added so that resolution actually finds the host a script booted:
  `GuiHost.boot(adopt_ambient=True)` registers itself when none is registered —
  the mirror of `Server.boot`'s `adopt_default`, first-wins, spelled the same way
  — and `stop()` gives the registration up. `Session.gui()` inherits it through
  the same call. The ambient layer's own owned host (`plot._ambient_host`) boots
  with `adopt_ambient=False`, because it is the fallback and must stay
  replaceable (`scope` reboots it wired to a server).

  Docs: the client book's GUI page opens with the view-opens-itself form and the
  sessions page states the adoption rule; `clausters.gui.view` added to the API
  reference. Examples: `window.py` and `bind.py` (the two smallest, one
  per door — a free-standing `GuiHost().boot()` and a `session.gui()`).
  Tests: `tests/test_view.py`, plus the duplicate-name refusal in
  `tests/test_gui_host.py`. **Not ported to TypeScript yet** — the shape is
  written into `clients/web/PLAN.md`.

- ✅ **C40 — Ids belong to the instance, not to the tree** *(done 2026-08-23)*.
  `GuiHost._register` stamped `child["id"]` **into the caller's dict** and reused
  whatever it found (`if "id" not in child`), so a `View` opened twice handed the
  second instance the first one's widget ids; the host answers a colliding id by
  **skipping the subtree with a warning** (`registry.rs`, `"widget id {id}
  already in use, skipping"`), so the second window drew wrong instead of
  failing. This was the one defect blocking everything above: a def that cannot
  be instanced twice is not a def.

  **Ids were already automatic — what was wrong is where they were written.**
  Nobody spells an id: `alloc_id` draws them from the client's pool and the walk
  hands them out. Stamping them *into the caller's tree* is what turned a
  definition into an instance. And the host cannot take the job over, not even
  with a change: `/gui_def <id> …` takes the id as its argument and `/gui_set`,
  `/gui_free` and `/gui_bind` all address by it, so a host-assigned id would have
  to be reported back and every command would start waiting for a reply. The
  client-side pool is what makes the whole surface fire-and-forget — the same
  reason the audio server does not assign node ids either.

  **The walk copies.** `_register` became `_stamp`, which returns a *copy* of the
  tree with the ids filled in; that copy is what is serialized, and the caller's
  tree is never written into. The plan first asked for an id map in the handle
  keyed by path — that turned out to be bookkeeping for something the copy gives
  for free: every visit builds a fresh node and asks for a fresh id, so node
  identity never enters, and the **same `View` nested twice in one tree** gets two
  id runs exactly as two `open()`s do. The handle keeps only what it already
  kept, `name -> id`.

  ```python
  strip = window(knob(name="gain"), toggle(name="mute"))

  a, b = strip.open(), strip.open()     # two windows, two id runs, one view
  a["gain"].set(value=0.5)              # b is not touched
  ```

  The `View` stays free of ids; an explicit `node(id=…)` is still honoured and is
  then the caller's problem, exactly as an explicit node id is on the server — a
  hand-picked id on a subtree used twice **is** used twice, and the host skips the
  second. The duplicate-name rule from C39 already forces a repeated *named*
  sub-view to be named apart or not named at all, so the handle's name index
  stays unambiguous under this.

  Docs: the `guidef` header and `define`/`open` say "in the document it sends";
  the client book's GUI page gains the two-windows-from-one-view block and the
  sessions page stops telling the reader to read an assigned id back out of the
  tree. Example: `panel.py` opens its panel twice and drives both by the same
  names. Tests: ids read out of the sent JSON, one tree opened twice, and the
  same subtree nested twice (`tests/test_gui_host.py`), plus the two
  `tests/test_gui_ids.py` cases that read ids off the tree.

- ✅ **C46 — The source: the data a view draws is an object, not an index**
  *(done 2026-08-23; numbered past the track's last label so the numbers already
  written here keep their places)*. `signal`'s documentation already named the
  thing — **the source** — and already listed its five carriers for addressable
  samples: `data` (inline JSON), `blob` (an index into the message's trailing OSC
  blobs), `buffer` (a server buffer number), `path` (a mapped file), `cache` (a
  peak pyramid). Choosing among them was the caller's, and `blob=0` was a
  correspondence kept by hand between two places in the program.

  ```python
  sig = source(decaying_sine(8_000, 120.0))

  v = view(label(name="caption", text="..."),
           waveform(name="wave", data=sig),
           title="...", w=720, h=360, flow="col")

  win = v.open()             # no positional blobs: they come out of the tree
  sig.set(other_samples)     # the definition and every open view follow
  ```

  What it settles:

  - **The object picks the carrier**, not the person writing the view: short
    stays inline, long spills to a temp raw-f32 file the host maps. The
    threshold and the spill are `guidef.INLINE_MAX` / `guidef.spill`, which
    `clausters.plot` now takes too — "how large data reaches the host" decided
    once rather than in two places.
  - **One source in two views is one payload and two references** — what
    "interchangeable" meant, said by the program rather than by convention.
  - **`set` is the update door.** A source records the definitions it feeds
    (rewritten so a later `open` sends what it holds now) and the live widgets
    drawing it (`(host, id)`, dropped when the host recycles the widget).

  **The carrier is fixed when the source is made, and that is a finding, not a
  simplification.** The host does not apply a live `path` change: its two doors
  are `/gui_set data` (inline samples only — "the samples are now these") and
  `/gui_set reload` ("they are where they were, and they moved"). So a spilled
  source keeps its own path for life and `set` rewrites that file and re-reads
  it, while an inline one sends the samples. Handing an inline source more than
  `INLINE_MAX` samples raises rather than silently switching carriers under a
  widget that was built around one. A source that *names* samples it does not
  own — a `buffer`, a `cache` — refuses `set` and offers `reload`.

  **What it does not cover.** Only the signal family's samples. The same object
  is the right shape for the other heavy props — a roll's `notes`, a curve's
  `points`, a patcher's `boxes`/`cords`, a score's `display_list` — but those are
  normalized by their builders (`_flat_points`, `_flat_notes`) *before* the node
  exists, so a source there is a change in each builder rather than the one
  expansion point `node` gives for `data`. Written into "Future directions"
  rather than half-done here.

  Docs: the client book's "Where the samples come from" leads with the source and
  keeps the carrier table for naming one by hand. Example: `window.py` drops
  `blob=0` and its positional blob, and gains the redraw — 8000 floats is past
  the inline ceiling, so it spills and the `/gui_def` carries no samples at all.
  Tests: `tests/test_view.py` — the carrier choice, the spill, one source in two
  views, the live push, a freed widget leaving the live ends, the rewrite-and-
  reload of a spilled source, and the two refusals.

- ✅ **C41 — `view()` is the root, and a root with no parent is a window**
  *(done 2026-08-23)*. `window()` is renamed `view()` and the distinction
  "window vs container" becomes positional: a view with a parent is a component,
  a view with no parent is the window. `open()` works on *any* node.

  ```python
  view(layout(knob(a), knob(b)), title="voice").open()   # a titled window
  layout(knob(a), knob(b)).open()                        # a window of two knobs
  knob(a).open()                                         # a window that is a knob
  ```

  **The tradeoff, stated because it is not free**: the *wire* type stays
  `"window"` — `Host::window_defs` keys the renderable document by window-rooted
  def id (`host/mod.rs`, `if node.kind == "window"`), and a non-window root lives
  only in the generic registry and is not drawn as a window. So `GuiHost.open`
  **frames** a non-window root in a `"window"` node client-side, with `hug=True`
  so the frame adds nothing but the OS window the wire needs. The frame is
  invisible: it takes the root id, the content becomes its one child, and the
  handle goes on resolving the tree's names. It is done in `GuiHost.open` rather
  than in `View.open` so the low-level door behaves the same. `view()` is then
  what you write when the window's own properties matter — a title, a size, a
  theme — since those belong to a root nobody frames. `window` stays as an alias
  of `view`.

  **A module named `view` cannot coexist with a builder named `view`.** `View`
  had been given its own `clausters/gui/view.py` in C39; `from clausters.gui
  import view` then resolves to the *submodule*, not the function, and binding
  the name in `__init__` on top of it is a shadowing that depends on import
  order. `View` moved into `guidef.py`, which is where every builder that returns
  one already lives, and the submodule is gone. (`clients/gui/src/tree.rs` is the
  Rust side of the same job, if a separate module is ever wanted again — the name
  to use is `tree`, not `view`.)

  **`into=` is deferred, and here is why**: there is no wire verb that adds a
  child to a live widget — `/gui_def id json` builds a *whole* tree and
  re-sending an id redefines it, dropping pending edits and reassigning ids. So
  `windowA(into=windowB)` can only be "insert into B's view and redefine B",
  which is correct at build time and lossy at run time. Build-time nesting is
  what C39 already gives (`view(a, b)`); a live `into=` needs a host verb and
  belongs to the GUI track, named there rather than faked here.

  **The editor joins the same shape**, which was listed below as a thing this
  track must not leave behind: `Editor.open`, `open_signal` and `open_pianoroll`
  took the host as a required positional — the last resource that had to be
  handed one. They now default to the ambient host like everything else.

  Docs: the client book's GUI page leads with the root rule and the three
  spellings; the composition page's editor opens with no argument. Examples: the
  32 `gui_*.py` that built a tree now say `view(`, and `bind.py` drops its
  wrapper entirely — one knob is all that window has to show, so the window *is*
  the knob (checked by ear: the bare root still binds and drives the synth).
  Restyling the rest of the examples to `v.open()` is C45's pass, not this one.
  Tests: the frame's shape and invisibility, a lone control opening, and the
  alias (`tests/test_view.py`); the editor's ambient host
  (`tests/test_gui_editor.py`).

#### Part 2 — the GUI and the audio server

- ✅ **C42 — A widget is built from the control it drives** *(done 2026-08-23;
  **corrected the same day**, see below)*. A `knob`, `slider`, `number` or
  `toggle` takes a def's control positionally and reads its **name** — what
  `/node_set` addresses — and its **default**, so the widget and the graph
  cannot disagree about what `"freq"` is and nobody types the name twice.

  ```python
  freq = control("freq", 220.0)
  sd = SynthDef("voice", out(0.0, sine(freq=freq) * 0.2))

  knob(freq, min=110.0, max=880.0)              # name and value, from the control
  slider(sd["amp"], min=0.0, max=1.0, label="level")     # or indexed off the def
  view(*[knob(c, min=0.0, max=1.0) for c in sd.controls]).open()
  ```

  `sd["freq"]`, `fd["cutoff"]` and `gd["mix"]` all give a `ControlInfo`, which is
  the unifying move: a `SynthDef` keeps the `Control` objects its graph
  references (`_controls`, the same first-seen order `spec` walks); a `FaustDef`
  reads its own payload; a `GraphDef` port answers with the targets it drives.

  **The correction, decided by the user the day it shipped: the range does not
  belong to a def.** It first landed as `control(..., min=, max=, step=)` and
  `GraphDef.port(..., min=, max=)`, with the widget reading them. That is wrong
  twice over:

  - **A control is a signal.** It says what value flows into a graph, not how a
    knob should be drawn — and the two names collide outright, because `min`/`max`
    on a signal are the **binary operators** (`freq.min(other)` composes a
    `BinaryOpUGen`). The attributes shadowed the methods for *every* control,
    ranged or not, since one without a range set them to `None`. Nothing in the
    client called them, so no test saw it; TypeScript's compiler refused the
    override outright, which is how it was found.
  - **A GraphDef port is the same category**: a name the server takes any float
    for, declared by hand for the sake of a GUI. It had no range before this
    milestone either, so removing it touches nothing older.

  So `min`/`max` are **spelled on the widget**, and a control that has no range
  of its own says so rather than being drawn over a guess. The one control that
  arrives with a range is a **Faust** parameter, and that is not an exception
  this client makes: `hslider(label, init, min, max, step)` cannot be written
  without one, the compiled DSP reports it back, and `ControlInfo` has carried
  the three fields all along with only that family filling them. Faust's syntax
  showing through, not a range clausters declares.

  **What the host cannot draw, written down rather than faked.** `props::Range`
  is `{value, min, max, label, text_size}`: linear, no step, no curve. A **named
  spec** (`spec="freq"` → 20..20000 *exponential*) was deliberately **not**
  shipped — a spec that silently drew linear would be worse than none. That is
  one entry in `clients/gui/PLAN.md`, and it survives the correction unchanged:
  it was always about what a *widget* can express.

  The props-parity manifest learned that `control` is **not a prop**: it is a
  *source* of `name`/`value`, so it is a parameter each client spells its own way
  rather than a key on the wire. (That test caught it, which is what it is for.)

  Docs: the defs page gains "A widget is built from the control it drives", the
  GUI page the same section from the widget's side. Example: `bind.py`
  declares `FREQ`/`AMP` once and each widget spells the range it is turned over.
  Tests: `tests/test_control_range.py` — the three families answering one shape,
  a control taking no range at all (and its operators still composing), Faust
  bringing its own, the keyword override and the two refusals.

- ✅ **C43 — The binding is made against the control, not against a widget id**
  *(done 2026-08-23)*. A widget and a def control used to meet only as a string
  typed twice (`win["freq"].bind("/node_set", synth.id, "freq")`), and nothing
  checked that the control existed. A view built from control objects already
  knows which control each widget drives, so the whole surface binds in one verb.

  ```python
  synth = Synth("voice", server=server)

  w = view(knob(freq), slider(amp)).open()
  w.bind(synth)         # one /gui_bind per control widget:
                        # /node_set <synth> <control> <value>
  w.unbind()
  ```

  `knob`/`slider`/`number`/`toggle` keep the control they were built from on the
  node (`View._control`, client-side, never on the wire), the id walk collects
  `widget id -> control name` alongside the names it already collected, and the
  handle carries it — refreshed in place on a redraw, like the names, so a
  rebound window is never wiring ids that recycled. `WindowHandle.controls`
  reports what is there.

  **The name and the control are two different things**, usually spelled the
  same. The widget's `name` is the handle's index; the control name is what the
  server is told. `knob(freq, name="pitch")` binds `pitch` to `freq`, and an
  explicit `name=` is taken out of the props before the control's own is
  applied — which is the collision that showed up the moment the test asked for
  it.

  It is still `/gui_bind` underneath, and the low-level form stays for anything
  that is not a def control (a bus, another widget, an arbitrary address).
  `bind` takes a `Node` or a bare id, and refuses a window where **no** widget
  was built from a control, which can only be a mistake. **Two widgets on one
  control is legal and drifts**: both bind, both set the node, neither is told
  when the other moves, and the host fires an apply rather than a second
  binding. That is the user's inconsistency to make, not the client's to detect;
  it is documented, not guarded.

  Docs: the GUI page's binding section. Example: `bind.py` grew a second
  control (`amp`) precisely so the one verb has more than one thing to wire, and
  binds the panel with `win.bind(synth)`. Tests: in
  `tests/test_control_range.py` — the surface bound in one verb, a widget named
  apart from its control, `unbind`, the empty-window refusal, a node or a bare
  id, and a redraw leaving the window bindable.

- ✅ **C47 — The host this handle did not start: `attach`, and a session that
  takes one** *(done 2026-08-23)*. The question W24 could not port until the
  reference answered it: the page reaches a host by three names (`guiHost()`,
  `newGuiHost()`, `Session.connectGui(url)`), and `connectGui` is a verb this
  client never had. Answering it by renaming the three would have invented a
  fourth rule; the answer is that **the host already had the server's pair, with
  half of it unnamed**. `GuiHost(host, port)` was an attach that did not verify,
  and `boot` was the only verb.

  What landed:

  - **`GuiHost.attach()`**, the peer of `Server.attach()` — verify, connect,
    adopt the ambient registration (`adopt_ambient`, first-wins), own no
    process. `stop` already read ownership off `_process`, so an attached host
    is left standing, windows and all; the verb only had to stop lying about
    having checked. `clausters.launch.gui_is_up` is the probe, the `server_is_up`
    of the visual side: `/gui_query 0` answered by `/gui_info`, which a host
    replies to even for a widget id it does not have.
  - **The probe goes over UDP whatever carrier the handle then uses**, because
    UDP is the one leg the host cannot turn off (`--no-tcp` exists, no
    `--no-udp` does). So it says *the front is bound*, not *your carrier is up* —
    a host started with `--no-tcp` answers the probe and then refuses a
    `transport="tcp"` connection. Written into the docstring rather than
    papered over.
  - **A supplied `interface` skips the verification** (`_own_carrier`), the same
    line `Server.attach` draws: a carrier this module does not know about may
    reach a host that answers no UDP probe.
  - **`Session(server, gui=host)`** — the visual half of taking a `Server` the
    session did not boot. Not a verb: the session already accepts a server
    through the constructor and has no `attach_server`, so the GUI gets the same
    door. `gui()` then returns that host and launches nothing, which is the
    idempotence it already had, stated for a host it was handed.
  - **`Session.connectGui(url)` falls**, and it falls without being replaced:
    it did *connect* and *adopt* in one call, and both halves now exist
    separately and symmetrically with the server's. Recorded for the port in
    `clients/web/PLAN.md` (W24).

  What this does **not** decide, deliberately: whether a host may be remote.
  It may — `GuiHost` is a plain OSC client to any address — but the useful
  topology (host and server together where the screen and the device are, script
  anywhere) is a **bind** question, and the binds are the server's and the
  host's, not this client's. It went to `clients/gui/PLAN.md` and `PLAN.md`
  instead. Two things would degrade under it and are written down there: the
  `path` carrier of a `Source` (the client writes a temp file the host mmaps —
  one filesystem assumed, so remote means inline-only under the 2 KB ceiling)
  and `--data-dir` (the GuiDef store is the host's).

  Tests: in `tests/test_gui_host.py` — the refusal, the supplied carrier that is
  not probed, the process it does not own, ambient adoption first-wins with
  `stop` giving it up, and a session taking a host it did not boot. Example:
  `examples/panels/attach.py`, which plays both parts in one run (boot, attach a
  second handle with an `IdShare`, a window from each, and the guest letting go
  while both windows stay). Docs: the GUI half of "Several servers, and the one
  you did not start" in `docs/src/sessions.md`, and `attach` named beside `boot`
  in `docs/src/gui.md`.

- ⬜ **C44 (analysis, not scheduled) — the inverse direction: a widget inside a
  def**. Faust's model, where `hslider` *is* the control declaration, suggests
  `play(sine(freq=knob(min=110, max=880)))` — the widget coerced into a control
  and its window opened by `play`. Recorded as a direction with a reservation,
  not as work: it inverts the dependency (`defs` would import `gui`, where
  today the arrow runs the other way and deliberately so — the arrangement's
  `gui → form` rule is the precedent), and it autogenerates the control's name,
  which is the one thing `/node_set` addresses by. If it is done, the coercion
  runs in **one direction only** (`knob → Control`, never a `Control` growing a
  widget) and `name=` is mandatory.

- ✅ **C45 — The examples pass: rewritten against the new surface, and organized
  while they are open** *(done 2026-08-24)*. Every milestone in this track
  changed how an example is *written*, and nothing runs them — not CI, not any
  build — so they were rewritten by hand and by eye.

  **The layout, decided here and applied to both clients.** One folder per
  subject, the same set in `clients/python/examples/` and
  `clients/web/examples/`, so a script and its page sit in the same place under
  the same name: `basics/`, `spectral/`, `buffers/`, `transport/`, `io/`,
  `faust/`, and the three that need a display — `panels/` (controls and layout),
  `views/` (reading something) and `editors/` (writing something), plus a
  web-only `components/` for the surface a page has and a script cannot. The
  `gui_` prefix is gone: it was a taxonomy typed into 37 filenames, and it named
  a folder that now exists. Both READMEs and both books' `examples.md` say what
  each folder holds and nothing else — no catalogue, which is the rule.

  **The `gui_` prefix came off the def names too**, where it had stopped naming
  anything (`gui_bind_beep` → `bind_beep`), and `refresh-bin.sh` learned to
  resolve a bare example name through the folders.

  **Restyled to the surface the track built.** The 28 examples that still said
  `gui.open(view(…))` say `view(…).open()` — the one thing C41 explicitly left
  here — and `panels/shell.py`, which typed `"freq"`, `"amp"` and their two
  defaults **three times each** (in the graph, in the widgets, in the value the
  script remembered), now declares two controls and builds the widgets from
  them. That was the last example in the directory still disagreeing with C42.

  **Read as pairs, verb by verb**, which the flat directories had made
  impossible: 19 pairs, 47 scripts with no page, 12 pages with no script. Two
  pairs turned out not to be pairs (`panels/standalone` authors a bundle in one
  client and boots one in the other; `transport/sync`'s page has a playhead the
  script lacks) and both went to `clients/web/PLAN.md` rather than being
  patched. One pair had never matched by name at all — `scope.html` was the page
  of `meters.py` — and the page took its twin's name, which also frees `scope`
  for the port `views/scope.py` still has no page for.

  **`_curve.html` is gone.** A scratch page left behind by the curve work: no
  header comment, no prose, nothing referencing it, two clips queried and
  logged. It was not an example, and an example directory is not where a
  debugging page lives.

  What this milestone deliberately did **not** do: add a catalogue of examples
  to any book, and touch the repository-root `examples/`, which drives the
  server and moves with the server's own track.

#### What this track must not break

- **Non-divergence.** Every milestone here lands in the web client in the same
  commit or leaves `clients/web/PLAN.md` naming the shape the port must follow.
  `View` is a plain object over the same JSON in both languages; the name index,
  the duplicate-name error and the id-per-instance rule are the same rule
  written twice, and if either turns out to be numeric it belongs in
  `clausters-core` instead.
- **The ambient verbs keep their meaning.** `clausters.plot`/`clausters.scope`
  are *verbs that open a window*; `guidef.plot`/`guidef.scope` are *builders
  that return a node*. That collision predates this track and gets worse once
  the builder's return value can also `open()` — one of the two names has to
  give, and this track decides which rather than leaving both.
- ✅ **`Editor` joins the same shape.** `Editor.open(host)` was the one place a
  resource took the host as a required positional; it resolves the ambient host
  like everything else now (landed with C41).
- **`name` means two things and should not.** On a root it is the *persistence*
  name the host stores for `/gui_load`; on a child it is the client's index.
  `define(name)` should carry the first so the prop is left meaning only the
  second — or the two are named apart. Decide it in C39, before either spelling
  spreads.
- **The examples are the acceptance.** 29 of the `gui_*.py` examples address
  widgets by name and one holds a builder's return value; when this track is
  done, the two spellings are one, and the pair of every ported example is read
  side by side, verb by verb.

### The notebook client (`clausters-jupyter`) — moved to the `jupyter` branch

Shipped 2026-08-03/04 and taken off `main` on 2026-08-05. The package worked,
and the reason it left is not the feature but the shape: closing it opened six
hooks in this client and its TypeScript port whose only consumer was the
notebook, and one of them — an undeclared `interface.boot()` reached by
`getattr` inside `Server.boot`, whose implementer starts nothing and answers a
boot with a warning — is wrong enough to want redesigning rather than porting.
Reworking that under a running notebook is the slow way round.

The branch keeps the package, its front end (`clients/web/src/notebook/`), its
tests, its examples and its documentation, plus `ISOLATION.md`: the audit of
what the track put where, what it took back out of `main`, and what each hook
would have to become to return. What stayed here on its own merits is named
there too — the id share, the blob bulk path, per-instance hosts and pools, and
`boot` as an instance method.

## Organization conventions

- Native crates under `crates/`; per-language clients under `clients/<lang>/`. The **core C-ABI is the only contract** between Rust and each language, with an explicit ABI version (as `embed.rs` / `clausters.py` already do).
- Project-wide boundary rule: "only flat data crosses" (bytes/`array`/scalars/integers), in both the transport and the core.
- Client milestone track prefixed with `C` so it does not collide with the server's `M` track; close each one with the project's milestone checklist (code+tests, a clear commit message, this plan's checkbox, developer/user docs where applicable, an example in `examples/` — which is also the manual-test surface for new human-audible/visual behavior —, a `docs/decisions.md` note only for a non-obvious choice).
- Code/comments/tests in English; the `PLAN.md` roadmaps and `docs/decisions.md` in English; conversation with the user in Spanish.
- Documentation is **three mdBooks, one per platform** (server `docs/`, Python client `clients/python/docs/`, web client `clients/web/docs/`), cross-linked by their Read the Docs URLs; the Python user doc for a client milestone goes in `clients/python/docs/`, and its API reference is generated from docstrings by pydoc-markdown. **No Sphinx/RST directives in docstrings and no milestone labels in any published doc** (labels live only in the `PLAN.md` roadmaps).

## Verification

- **Workspace**: `cargo build` and `cargo test` (without features and with `--features faust`) must pass; `tests/rt_safety.rs` and `tests/denormals.rs` stay green after the refactor.
- **Numeric parity**: a new test in `clausters-core` (or `tests/`) comparing the native builtin's output against the server's native branch (bit-exact) and against Faust (tolerance).
- **Client**: `pytest` of the package; smoke of `_native` (builtin, TempoClock, bundle, render).
- **E2E** (CLAUDE.md rule: server and client in the **same** Bash invocation): start `./target/debug/clausters &`, define a FaustDef from the high-level client, `/synth_new`, control via bus, verify `/done`/replies, `kill`. NRT: score generated by `seq` → `render()` → compare WAV/golden.

## Found by use: the running list of fixes and open questions

- ✅ **`button` takes no control, `toggle` takes no range, and behind both is
  the same unasked question: what kind of thing is a button?** *(named
  2026-08-23 by the user; answered and fixed 2026-08-24)*. Two concrete gaps
  and one design question they hang from. Both clients, one surface.

  **The question was answered by splitting the layer, not the element.** Press
  and release are the **primitives**, and everything else a pointer does to a
  button is composed from them: a click is a press and a release that landed
  inside, a double click is two of those inside a window. Those are *gestures*
  and belong to the gesture machine — so a "command button" never was a second
  kind of element, and `click` is not a mode. What a mode says is only **which
  primitive reaches the server**, which makes it a prop and leaves `button` one
  element. (`keys` stays the precedent for the instrument role that genuinely
  earns an element: it has pitch, velocity and voices; a button has none.)

  So `button` grew `mode`, in the two shapes a control signal comes in:

  - `"gate"` (the default, and what shipped before) sends `on` at the press and
    `off` at the release, so the value lasts exactly as long as the button is
    held — an `env_gen` gate, and what a `tr` ignores the tail of by definition.
  - `"press"` sends `on` and nothing after it: the bang.

  **A widget cannot make a value instantaneous**, which is the finding under
  the whole entry and the one thing the original text had wrong (it read the
  host's press-is-the-event as already serving a trigger). What is sent is
  *held* by whoever receives it. So `press` is a bang only against something
  that returns to zero on its own — a `rate="tr"` control, which the server
  resets after one block — or against a script, for which one `/gui_event`
  message *is* an event. Both clients **refuse** to build a `press` button over
  any other control rather than letting it be found by ear: it would leave `on`
  standing forever, which is a category error and not a preference. That
  refusal is the one place the widget reads a control's `rate`.

  This is also why there is no "gate rate" in SuperCollider and never was: a
  trigger needed a control type because the *server* has to do something the
  value alone cannot express, and a gate does not. The widget declares when it
  emits; the graph declares how the value is read.

  **`toggle`'s range turned out to be a pair, and `button` takes the same one.**
  Not `min`/`max`: a range is a span a widget is drawn over, and these are two
  discrete values. Both switches carry `on`/`off` (`1`/`0` by default), so a
  bypass at `0.0`/`0.7` or a mode at `1`/`2` is driven without a binding that
  scales — and `/gui_bind` needed no change, since the element already emits the
  final value. Pd's `tgl` and its settable `nonzero` is the precedent; the wire
  type follows the number (an `Int` while it is whole), so every reader that
  parses today's `1`/`0` as an int keeps doing so.

  `button` also joined `_from_control`, which was the first gap: it takes a
  def's control positionally like the other four, dropping the control's default
  rather than seeding a `value` it holds nothing in.

  Lands in: `clients/gui/src/host/elements/button.rs` and `toggle.rs` (plus
  `elements::switch_value`), both clients' `gui` modules, `docs/gui-protocol.md`
  ("The two switches, and what a press means"), the GUI page of both books.
  Tests: `tests/test_control_range.py` and `tests/gui-view.test.ts` — the button
  built from a control, the bang against a trigger, the refusal, the unknown
  mode, and the pair that is not a range; in the host, the press mode's silent
  release and the pair it was given. Example: `bind.py`, which grew the two
  buttons over two envelopes (checked by ear: `hold` sustains while held, `fire`
  blips once per press, neither through Python).

- ✅ **A button's event is read by hand, thirteen times: the callbacks are
  `on_press`/`on_release`, and `click` is the gesture over them** *(found and
  fixed 2026-08-24, taking the entry above; scoped by the user to leave the
  server half separate)*. `on_event(value)` delivers a control's value, which is
  right for a knob and is not what a button has: thirteen examples filtered the
  release out by hand, three of them defining the identical
  `press = lambda fn: (lambda value: fn() if value == 1 else None)`.

  **A button says two things at once, to two audiences**, and that is the whole
  design. Its **value** is a control signal, which `/gui_bind` forwards to the
  audio server without the script ever seeing it. Its **interface events** are
  what the hand did, and they take a road of their own: `"press"`, `"release"`
  — wherever the pointer came up — and `"click"` in addition when the release
  landed on the button. Three verbs in both clients: `on_press`/`onPress`,
  `on_release`/`onRelease`, `on_click`/`onClick`.

  **A binding swallows the value and never the command.** That is the load-
  bearing consequence and it is pinned by a test: an interface event is emitted
  straight to the script bound or not, because a command is not a value and has
  nowhere else to go. So one button drives a synth's gate *and* runs a script's
  action — which is what makes "one element serves both roles" true rather than
  merely asserted. `on_event` stays the raw stream and sees everything, so the
  two vocabularies are additive rather than a mode.

  **Where the click is decided**: in the gesture machine, not in the element.
  `Element::release` grew an `inside` argument that the machine answers with the
  same declared shape and hit slop `press` is filtered through
  (`gestures::element::inside`, beside `press` for exactly that reason). An
  element that re-implemented "landed inside" would be a second answer to a
  question the machine owns, and the ten other implementors ignore it.

  Lands in: `Events::and_interface` + `Element::release`'s third argument
  (`clients/gui/src/host/widget/element.rs`), `gestures::element::inside` and
  the interface road in `report`, `elements/button.rs`, both clients' host and
  handle, `docs/gui-protocol.md`, the GUI page of both books. Tests: the
  element's six cases and the machine's three (the click, the press slid off,
  and the bound button whose value goes to the server while the click reaches
  the script); in both clients, the tag routing, the cancellation, the raw
  stream still seeing everything, a redraw keeping the handlers, and clearing
  them.

  **Fifteen examples lost their filter** and two turned out to be wrong. Every
  command button in both clients now reads `on_click`/`onClick`
  (`bpf`, `patch1`, `score`, `score_from_data`, `composer`, `take`, `shell`,
  `workspace`, `recording_mapped`, `multitrack`, `bulk`, `panel`; and their
  pages). `pianoroll`'s `value == 1` stays: it
  reads a **toggle**, where 1 is the state and not the press. Checked by eye
  through `gui_panel`, which is wired both ways on purpose: a completed click
  prints the value, the three events and the handler; a press slid off the
  button prints the release and no click.

- ✅ **`gui_bulk`'s mode button flipped twice per press and came back to where
  it started** *(found 2026-08-24 while removing the hand-written filters;
  fixed with them)*. `toggle_mode(*_)` was wired to `on_event`, which fires for
  the press **and** the release, so the drag mode flipped to `draw` and back to
  `sample` before the hand let go — the button did nothing at all. It is filed
  here rather than folded into the entry above because of what it says about
  the surface: the missing verb did not merely cost boilerplate, it produced a
  defect that reads as correct in the source and that nothing runs. It is the
  second such find in this file's examples, after the curve drawn with a shape
  it did not have.

- ✅ **A control's range shadowed the operators of the same name, and the range
  did not belong there at all** *(found and fixed 2026-08-23, porting C42 to
  TypeScript)*. C42 gave `Control` the attributes `min`/`max`, and a `Control`
  **is a signal**: `min`/`max` on a signal are the binary operators
  (`freq.min(other)` composes a `BinaryOpUGen`). The attribute shadowed the
  method for *every* control, ranged or not — a control with no range set
  `self.min = None`, so `freq.min(2.0)` raised `'NoneType' object is not
  callable`. Nothing in the client called it, which is why no test saw it;
  TypeScript's compiler refused the override outright.

  The first fix was to read the range through `range`/`step` instead. The user
  rejected that as treating the symptom: if `min`/`max` cannot be written on a
  signal, it is because **they are not a property of one** — the range is how a
  *knob* is drawn, so it belongs to the widget. Removed from `control()` and
  from `GraphDef.port()` (which had none before C42 either), spelled on the
  widget in both clients, and only a Faust parameter still arrives with one,
  because `hslider` cannot be written without it. See C42, corrected.
- ✅ **A curve was drawn with a shape it did not have.** `Editor._body_for`
  handed `clip(points=…)` a list of already-resolved `(t, v, shape, curve)`
  quads, but a `points` argument of *tuples* is read as `(t, v, curve_spec)` and
  resolved — so the shape number was re-read as a curvature and the fourth
  element dropped. A linear segment (shape 1, curve 0) drew as the custom shape
  with curvature 1.0, and edited back the same way: an envelope changed shape by
  being looked at. Found by the web client's editor parity vectors, which
  compare the two clients' drawn trees; fixed by sending the flat form (kept
  verbatim), in `_body_for` and `_resync` both, and pinned by
  `test_a_curve_is_drawn_with_the_shape_it_has`.
- ✅ **`Editor._snap` is dead code** *(deleted 2026-08-26)*. It predates the
  document: the crate snaps a placement now ("the intent states where the hand
  put it and the crate snaps"), and nothing called this. The TypeScript port
  left it out rather than porting a method that runs nowhere, and that was the
  right reading — the two clients agree by the method being gone rather than by
  one of them carrying a copy nothing reaches. `quant` stays: it is what the
  editor tells the crate to snap *to*.
- ✅ **`guidef.waveform`'s `measure` is documented narrower than it is** *(fixed
  2026-08-26)*. The docstring offered `"peak"` or `"rms"` and described a stack
  as *two* waveform views; the host parses a space-separated set on any
  signal-family widget, and the editor's own signal view sends `"peak rms"` to
  one `waveform` — which is what its `layers` property is. The TypeScript
  builder's type was widened to the four spellings when the editor was ported;
  this docstring was the other half.

  **What it was actually saying wrong** is the reason it is worth a record: not
  that it listed two values instead of four, but that it told the reader a stack
  is *two views layered*, which is the one thing it cannot be — every view of a
  signal paints its own field before it draws, so the second hides the first.
  A reader following the docstring would have written the picture that does not
  work. The prose is now the TSDoc's, sentence for sentence, and
  `docs/gui-protocol.md`'s summary row (which pointed at a fuller section that
  was already right) names the space-separated form too.

*(This plan's version of the section the other roadmaps call "Found by use": what
using the client turns up, recorded the day it is found. Every entry is a
checkbox, so what is open reads as open; a resolved one stays with the record of
what was wrong. Anything unresolved lives here, at the **end** of the plan —
never inside the milestone that happened to be open, and never among finished
work, where a pending item reads as done.)*

- ✅ **A read left its timeout on the socket, and the next send inherited it**
  *(found 2026-08-21 by the user, resizing `gui_analyzer`'s window: the window
  died, and what died was the script — `TimeoutError: timed out` out of
  `synth.set` → `sendall`, not anything in the host; fixed the same day)*. In
  Python a timeout belongs to the **socket**, not to the call that set it.
  `OscTcpInterface._recv_into_buf` set one per read and never took it off, and
  `recv` walks its budget down to a remainder — so after a request that spends
  its budget the socket was left with *microseconds* on it, and the next
  `sendall` that could not complete at once raised instead of waiting. The
  client gives a send no deadline; it was holding a read's.

  **Why a resize is what found it**, since the sequence is the whole of the
  defect: the host asks the server for everything it has to redraw, the server
  takes a moment to drain, the script's send buffer backs up for an instant —
  and the example's control sweep, which should have waited that instant, died.
  The example was right and so was the host. Anything that sends while the
  server is busy could hit it, which is why it read as random.

  Fixed by restoring the socket to blocking in a `finally`, in both interfaces
  that set one (`OscTcpInterface`, and `OscUdpInterface.recv` for the same
  reason, where `sendto` is the one that would inherit it). The regression test
  is a fake socket that **refuses a send while a timeout is set**, which is what
  a real one does under a full buffer — it fails against the old code with the
  remainder still on the socket. `OscWsInterface` was already clean: its
  connection takes the timeout per call.

- ✅ **Following a recording has a class in the web client and a primitive here** *(named 2026-08-19, shipping the streamed overview: the reference client is the one that ended up with less)*. `/buffer_stream` sends the summary of what a take is recording, and both clients can subscribe (`stream_buffers`). What folds a report into a picture is `clausters_core::peaks`, bound here as `peaks_cache_write_buckets` / `gui.peaks_cache_stream_file` — the primitive, and the one shape that makes sense for a headless script: the cache file a `waveform(cache=...)` maps grows as the take does. The web client has `data.RecordingStream` on top of it, which owns a pyramid per take, tracks how far each was written and calls back on every report. Nothing here is wrong — but the ordering rule is that this client leads and the port follows, so the class is missing on the side that is supposed to have it first. What it would look like: an `OscFunc` on `/buffer_stream.reply`, a cache (in memory or on disk) per take, and `written` beside it; the one subscription per client is the constraint either way.

  **Ported 2026-08-21 as `clausters.data.RecordingStream`**, with the twin's
  surface — `open`, `peaks`, `written`, `on_report`, `stop`, `free`, `reports`
  — and the caches allocated at each take's full length and empty, so the axis
  does not move while it fills. The one thing that is genuinely different is
  **where the reports arrive**: this client's reply path is pulled, not pushed
  (`Server.request` reads the carrier and drops what it did not ask for), so a
  subscription sent over the command carrier would have no one listening. The
  stream sends `/buffer_stream` out of its **own `OscReceiver` socket** instead
  — the shape `_ensure_recycler` already uses for `/node_end` — and the reports
  land on the responder thread like every other `OscFunc` callback. That also
  settles the "one subscription per client" constraint in this client's favour:
  the stream is a different client from the script's own carrier, so a
  `stream_buffers` call beside it replaces nothing. `tests/test_recording_stream.py`
  drives it against a fake server and asserts the same claim the web client's
  `tests/recording.html` does — the cache the reports built and the cache the
  samples build are the same bytes. `peaks_cache_stream_file` stays: it is the
  right call when what you want is the file a `waveform(cache=...)` maps, and
  the book now says which is which.

  **`data.BusStream` and `data.TapStream` were the same defect and were ported
  with it** *(found 2026-08-21 doing this port, looking for where the class
  belonged; taken the same day at the user's instruction — the web shape is the
  better one, so the fix is to complete this client rather than to record the
  difference)*. The web client's `data` module holds three stream classes and
  this client had none of them: `/bus_stream` and `/bus_tapStream` are
  `Server.stream_buffers`' siblings and stopped at the subscription here too,
  so a script that wanted the newest value of a bus or the newest window of a
  tap wrote the responder and the bookkeeping itself. All three are now in
  `clausters/data.py` on one base (`_Subscription`): the receiver socket, the
  ack, the listener list and the two verbs that end it are written once, and
  each class adds only its command, its arguments and what a reply means.
  `TapStream.interleaved` carries the one piece of real arithmetic — windows of
  a stereo pair may differ in length, and what it pairs is the freshest sample
  of each, not their starts.

- ✅ **A session's source table was built from the material the script started with, so the second save wrote a file that could not be reopened** *(found 2026-08-17, tracing the GUI plan's "a reopened session draws less on every redefine" — which turned out not to be the host's)*. `composer.py`'s `save` named `buf`, the buffer read at startup. But reopening resolves each take into a **new** server buffer, so after one cycle the composition's takes are buffers 3 and 4 while the table still describes 1 — the document names sources the table does not contain, `resolve` finds no entry, and the take comes back with no material. On screen that is a waveform that vanishes on the second save/open cycle, with nothing said anywhere; it reads exactly like a redraw bug, which is where it was filed for a day.

  **Fixed in both places, because the example is not the only writer that can do this.** The example builds its table by walking the composition it is about to save (`takes_of`), which is the only source of truth for what material a tree currently holds. And `to_session` **refuses** a session whose document names a source the table does not cover, naming the ids: the table is caller data — what a location *means* is the caller's — but whether it covers the tree is checkable here, and it is the difference between an error at the moment of saving and a silent hole two opens later. A composition with no material still needs no table.

- ✅ **An element reads one thing, so two fragments over different material cannot be joined** *(named 2026-08-18 building the clip's join verb, which is where the model ran out; closed the same day, the user having said what the answer is: "lee los segmentos correspondientes a distintos buffers como si fuera un mismo buffer — tiene que tener una estructura de datos que referencie varios archivos y posiciones")*. The arrangement's elements each wrapped **one** object, so a join over one buffer worked (windows that continue each other) and a join over two had nothing to become.

  **`Segments` is that structure**: a list of `(buffer, start, duration)` read back to back — which buffer, which frame, how long. It is **not a sixth primitive**: it is the `Vector` primitive (a list at constant time) assembled from more than one window, and it keeps every property that made a window worth having. Nothing is copied, so cutting it apart gives back the windows it was made of; it plays as one thing (one event per segment, one instrument, each with its own window); it draws as one clip (one take per segment, over its own stretch of it — which is what the clip's wire change made sayable); and it saves and reopens with each segment naming its own source.

  Two things fell out of it and are worth keeping. The join's shape is a fact about the **material**, not a mode: one run of one buffer joins back into the single window it was cut from, which is what makes a join the inverse of a split rather than a pile of wrappers. And the **head of a cut is not rebuilt** — it is the element it was, with its placement shortened — so lengthening it again brings the other segment back, exactly as lengthening a trimmed take brings its frames back. That is the placement rule the whole layer rests on, holding over assembled material.

- ✅ **`Group` names two different things in one package, and one of them is the server's** *(named 2026-08-17 by the user while the round-trip entry was being taken: "Group ya está tomado por el servidor y difiere de form")*. `clausters.Group` is a **node-tree group** — a server resource with an id, a target and an add action — and `clausters.form.Group` is the arrangement's **composite element**, a set of placed members with a concrete or logical kind. Both are exported from the package (`clausters/__init__.py` and `clausters/form/__init__.py`), so a script that imports from both gets one name silently shadowing the other, and prose about "a group" has to say which every time.

  **It is the arrangement's that should move**, not the server's: the server's name is scsynth's and is what every `/group_*` command, every reply and every book page already calls that thing. The document's own word for the arrangement one is **`set`** (`Body::Set`, with a `grouping` that is concrete or logical — and the crate already recorded why the discriminant is called `grouping` rather than `kind`), so the format has a name for it that nothing else in the project uses.

  **What a rename costs, so it is taken deliberately and not in passing:** the class and its module, `clausters.form`'s exports, the editor and the document bridge, the GUI host's own vocabulary where it names what it draws, the composition chapter of the client's book, the examples, `CLAUDE.md`'s arrangement-vocabulary section (which states "a `Group` has two **kinds**") and the web client when it ports the layer. It is mechanical but wide, and it should ride alone rather than inside a milestone about something else.

  **Fixed 2026-08-18, and it took the other two collisions with it** — because the same defect had three instances and fixing one would have left the package half-renamed. `Group` became **`Aggregate`** (not the `set` proposed above: `Set` collides with Python's builtin, and the document's word moves to the class rather than the class to the document's), `Buffer` became **`Vector`**, and `Event` became **`Clang`**. The two extra ones are the worse instances, in fact: each collided with the very object the element *wraps*, so the editor was aliasing `Event as FormEvent` beside `Event as SeqEvent` to keep them apart, and a `Buffer` wrapping a `Buffer` was a sentence that could not be written straight.

  The rename reaches **every end of every wire in one commit** — the Python classes and the `form/group.py` module (now `form/aggregate.py`), `Body::Event`/`Buffer`/`Set` in `clausters-document`, the `kind` strings of the saved format, the C ABI's fixtures, the GUI host, the parity vectors, both books and this plan. The saved format changed with no compatibility shim, which is what pre-1.0 buys. Recorded in `docs/decisions.md`, including where `Clang` comes from (Tenney's gestalt unit) and why neither SuperCollider's `Klang` UGen nor the C compiler is an objection to it.

- ✅ **A document's node ids are minted per conversion and stamped on the object, so two compositions number from 1 and collide** *(found 2026-08-17, checking the user's report that "en los ejemplos hay un problema recurrente con los ids"; the report was right and the cause is not the examples)*. `_Ids` (`clients/python/clausters/form/document.py`) numbers one conversion: it walks the root, takes `next` past the **maximum** id already stamped, and writes each assignment onto the element object as `_doc_id` (`ID_ATTR`). Two properties follow and only the first is intended. A second conversion of the same tree gives every node the same number, which is what the history rests on — an entry recorded against one conversion still names the right thing in the next. But numbering starts at **1 for every root**, so two arrangements built in one script both hold ids 1, 2, 3, and an element authored in one and used in the other carries a number a *different* element already holds. Measured, with no object shared between the two trees:

  ```
  t1 = Group([(0.0, a), (1.0, b)])  ->  ids 1, 2, 3   (a = 2)
  t2 = Group([(0.0, c), (1.0, d)])  ->  ids 1, 2, 3   (c = 2)
  t2.add(a, 2.0); to_document(t2)   ->  ids 1, 2, 3, 2
  ```

  **Nothing below catches it**, which is why it surfaces as a gesture that misfires rather than as an error: `_scan` reads the maximum and never notices an id it has already seen, so a duplicate passes conversion in silence; the crate resolves an intent to the **first** member whose id matches (`intent::find_member_mut`) while `Editor._index` keys `node id -> (owner, handle, element)` and keeps the **last**. Two writes, two different destinations, and on screen the clip the hand moved returns to where it was.

  **Distinct from the crate-side entry it looks like** — *"Two placements of one element share a node id"* (`crates/clausters-document/PLAN.md`) is one object appearing twice, and giving each placement its own element fixes it. This one is two *different* objects colliding, reached by ordinary authoring (material reused between compositions, two trees converted in one script, a library of takes), and no authoring discipline avoids it.

  **What the fix has to decide, and it is why this is recorded rather than patched:** ids are unique *within a document*, and today nothing owns that property. Enforcing it at conversion is cheap (a registry per document, a collision renumbered or refused) and is the Python bridge's alone; making the **crate** mint and validate them is the one that holds for every writer, including the host and a future web client, and it is the same question as "May one element be placed twice, and what does an intent name if it is?" (that plan's Open decisions). Whichever wins, uniqueness stops being an accident of the order things were converted in.

  **Acceptance:** the conversion of any tree yields ids that are unique in it, asserted over the collision above; and a document that carries a duplicate is refused with a message naming both nodes, rather than applied to whichever comes first.

  **Fixed 2026-08-17, in the two places the failure comes from.** The conversion **claims** each id for the object it first meets carrying it (`_Ids._owner`), and an object that turns up with an id another already claimed is renumbered past everything in the tree — so this client cannot produce the collision any more, while a tree converted on its own is numbered exactly as it always was and a second conversion of the same tree is identical. And the crate refuses what it can still be handed: `Document::duplicate_id` runs on **deserialization**, the one point every writer passes through, so a client, a host and a file all get the same answer without any of them remembering to ask.

  **What the fix had to be careful about, and it is the reason it is not simply "reject a repeated id".** One element *placed twice* produces a repeated id too, and refusing that would have settled the open question about what an id identifies by picking its "forbid" answer, from inside a check about something else. The line is what the two nodes **are**: identical nodes are ambiguous but consistent (the open question), different nodes are incoherent (this defect). Recorded in `docs/decisions.md`, with the C ABI asymmetry that came with it — `clausters_document_open` returns a null handle and carries no message, so the Python client names the collision itself once the handle comes back null, and only then.

  **The cost of renumbering, stated because it is real:** a log entry recorded earlier against the moved element's old number no longer names it. It happens only when material crosses between trees, the new number is one nothing else in the tree holds, and the editor re-derives its index from the document on every edit — so what is at risk is undo of an edit made *before* the crossing, not the current one.

- ⬜ Acceptable equivalence level for higher math vs Faust (a concrete tolerance).
- ⬜ Whether a separate `cdylib` for `clausters-ffi` is preferable, or exposing its C-ABI from the same `libclausters` (initial preference: separate, so as not to couple client and server embed).
- ⬜ The FFI-overhead threshold at which the scalar builtin uses a pure-language fallback instead of crossing the boundary.
- ✅ **Python type-checking workflow** *(the entry that opened this as "deferred" is kept as the record of why the answer is the shape it is)*. A first pyright run over `clients/python` surfaced ~730 findings that were overwhelmingly not bugs — missing-venv import noise, legitimate dynamic patterns in `tests/`/`examples/`, and runtime invariants a checker cannot model — so a strict gate would have cost annotation debt to buy nothing. What shipped instead is a **call-site check, not a type check**: `pyrightconfig.json` sets `typeCheckingMode: "off"` and turns exactly four rules back on (`reportCallIssue`, `reportIndexIssue`, `reportMissingImports`, `reportUndefinedVariable`), over the package, the tests and **both example directories** — which is the one thing nothing else catches, since no build ever reaches an example's call sites and CI runs none of them. The baseline is zero, so anything it prints is yours; the rule choice is explained in `docs/contributing.md` and the "run it after any Python signature change" instruction is in `CLAUDE.md`. Still open, deliberately: it is not in CI, and the package-source annotation debt a real `typeCheckingMode` would demand is unpaid.
- ✅ **A redrawn window kept its widgets and lost its handlers** *(found 2026-08-16, running the whole-loop example `gui_daw.py`: after pressing open, every button in the window was dead — including play, which read as "the reopened piece will not play")*. `GuiHost.define` on an already-defined root recycles the old subtree's ids and takes fresh ones; `_recycle_subtree` dropped the `on_event` callbacks with them, and `define` returned a **second** `WindowHandle` while the caller kept holding the first, whose name -> id map now named ids that had gone back to the pool. Any script that draws its own widgets beside an editor (`Editor(extra=[bar])`, which is a documented feature) lost them the first time the editor redrew — silently, since a dead button raises nothing.

  **The fix is the rule the handle's own docstring already stated** — *"widget name -> its current id, refreshed on every redraw"*: a callback and a name belong to the widget the name points at, not to the number it happened to carry. `define` inherits a named widget's handler onto its new id and refreshes the live handle **in place**, so one window is one handle while it is open. The web client moved with it, and each side has a test that delivers a real event to the recycled id and to the new one.

- ✅ **Play did nothing after a pass ran out** *(found the same day, same example)*. The transport **parks at the end** of a pass (`clausters.gui.transport.update`), which is what lets a pause resume where the music got to — so `at` is the end of the piece and a bare `play` starts there and sounds nothing until a `stop` rewinds. Correct as a transport and a puzzle as a button: the example's play handler rewinds when the transport is parked at the extent (`gui_daw.py` then, `composer.py` since the two merged on 2026-08-17). Left in the client rather than "fixed": a transport that silently rewound would make `play` and `stop` the same button, and the parking is what a pause is built on.

- ✅ **Undo works for clips and for nothing inside one** *(found 2026-08-17 by the user: "undo/redo anda para clips, no para los elementos internos de los clips")*. Only the placement route goes through the crate's log: a note edit rewrites the timeline and a break-point edit rewrites the `Env`, both straight onto the arrangement, so neither leaves an entry and neither can be stepped back. Both *are* nodes of the document — that is what makes a note addressable at all — so the vocabulary is already there: the roll's edit is a `SetMembers` and the curve's a `Configure`.

  **It was implemented and then reverted the same day**, which is the finding worth keeping. Routing the two through the log took: minting **ids** for notes the payload cannot identify (the roll sends the resulting list, so order is the only information there is); putting the curve's break-points **into the document** at all, since a leaf's config named the automation and nothing else and there was therefore no previous value to invert; and teaching `_adopt` — the redo path, which adopts the whole document instead of replaying intents — to read a set's members and a leaf's config back out of it. All three are **reconciliation between two trees**, and they only exist because `clausters.form` is a parallel Python model that `_history` re-derives the document from on every edit. Building more of that is building on the thing that has to go.

  **Fixed 2026-08-17 with O13**, which is what the paragraph below predicted: once the document is *held*, an edit that writes the arrangement directly leaves it behind, so the two had to become intents and the reconciliation the first attempt needed turned out to be removable rather than payable. The roll's edit is a `SetMembers` keeping its ids positionally, the curve's a `Configure` over a config that now carries the points, and the redo path stopped adopting the whole document — it reports the intents it applied, so a redo projects them exactly as an undo does.

  **The original reasoning, kept because it was right about the shape and wrong about the price:**

  **So this waits for the one-tree refactor** — the open note at the foot of O12 (`crates/clausters-document/PLAN.md`): the editor holds one `Document` handle for the window's life and `clausters.form` becomes accessors over the crate's tree. The cost argument that deferred it is gone (an edit is 0.008 ms). Undo of what is inside a clip is then the ordinary case rather than a fourth reconciliation path, and it is what should force the refactor.

- ✅ **A redo moved the model and told the host to keep drawing the old position** *(found 2026-08-17 by the user: "al hacer redo no se actualiza lo que se ve y solo se actualiza al volver a hacer undo" — and, from the same cause, an undo queue that looked far shorter than it is; this is the **placement** history, the only one there is — see the entry above)*. Undo and redo reach the arrangement by different routes and only one of them kept the **drawn record**: an undo projects the crate's inverses through `_project`, which updates `_Placed.offset/.dur`, while a redo adopts the whole document through `_adopt`, which moved the handles and left the registry behind. A correction is read straight out of that registry (`_resync`), so every redo answered with the position the clip had *before* it — and the picture only caught up on the next undo, one step behind the history. The two paths now share `_redrawn`, which is the same reasoning `_project` already carried in its docstring, applied to the other route. The budget was never the issue (256 entries; twenty drags undo and redo exactly twenty times) — the log was right the whole way and the answer was wrong.

- ✅ **The curve on screen and the sweep in the air drifted apart from the first edit** *(found 2026-08-17 by the user, dragging a break-point in `composer.py`: "el glissando se escucha siempre igual")*. An `Automation`'s `Env` is its source of truth, and what the lane synth actually reads is the **control buffer** `prepare` fills — once, at setup. So the edit-back rewrote the envelope, the next render scheduled the new curve, and the synth went on reading the old samples. `Automation.refill` is the missing door (non-blocking by default: it is called from a UI loop, and the fill is one command the server applies ahead of the synth that reads it), and `Editor._apply_points` calls it on every break-point edit — which is what the example's own claim, *the curve you draw is the curve you hear*, always said.

- ✅ **A drag pressed play: `follow` started a pass instead of following one** *(found 2026-08-17 by the user, dragging clips in `composer.py` after the merge: letting go of a drag started the piece)*. `Editor._follow_render` re-scheduled whenever `follow` was on and a destination existed — and a destination outlives a pass, since it is what the *first* play left behind. So every edit made after the first play started a fresh pass, from a stopped transport or from one parked at the end of the piece. What `follow` means is **what is sounding follows the edit**; starting sound is a transport action, and a window that plays itself is the thing the example's own comments rule out on the other route (nothing renders until play is pressed). The guard now also asks whether the transport is playing, and an edit made while stopped does what an edit made before the first play always did: it marks the composition, and the next play re-reads it.

- ✅ **A leaf's reference changed key between two saves, so what resolved once stopped resolving** *(found 2026-08-17 by the user, saving and reopening twice in `composer.py`: the second open lost the automation lane)*. `_body` wrote a leaf it has no body for as a **generator** node keyed `element`, while a real `Generator` writes the same node kind keyed `generator` — and `_element` reads only the second. So a hand-written `Element(automation)` came back as a `Generator`, and saving *that* wrote the other key: a resolver that recognized the material on the first open could not on the second, and the curve went frozen. One body kind, one key: the fallback writes `generator`, and `element` still reads for files already written. Related to but not the same as the crate-side entry about a leaf's *identity* (`crates/clausters-document/PLAN.md`) — that one is about the reference being `repr()`; this one was about which key it hides under.

- ✅ **A refused note edit sprang back with nothing attached** *(found the same day, same run: the notes on the pattern lane's clip flicker, jump and return)*. Refusing is correct — a `Sequence` over a `Pbind` is forward-only, its notes are a *rendering* of an algorithm, and the acknowledgement pushing the old notes back is what stops the host drawing the one the hand moved. What was missing is the **reason**, which the acknowledgement has carried since the wire grew it: without one the body teaches *sometimes it does not work* rather than *not here*. It now says to render the generator to a track to edit its notes. **The flicker itself is not fixed by this and is not the client's to fix**: the picture still follows the hand and snaps back, because nothing tells the host the body is read-only *before* the gesture — see the entry opened in `clients/gui/PLAN.md`.

- ✅ **The whole-loop example wrote a session no other writer could open** *(found 2026-08-17, merging `gui_daw.py` into `composer.py` and reading its source table against the crate's)*. The example wrote `"location": "/tmp/.../take.wav"` — a bare string — while `session::Location` is an internally tagged enum (`{"at": "file", "path": …}`). `to_session` passes the source table through **unvalidated** (`clients/python/clausters/form/document.py`: it is caller data, and the crate is the one that knows the shape), so the file was written happily and read back happily *by the example itself*, which resolved the field by hand. Handing it to the standalone host is what says it: `invalid type: string "take.wav", expected internally tagged enum Location`. So the example that exists to prove "a session written by this client opens in a host with no language attached" was proving the opposite, in silence, for as long as nobody handed the file over. Fixed in the example, and checked the way the claim is written — the host opens the new file and refuses the old one.

- ✅ **A frozen generator made a whole reopened piece unplayable** *(found the same day, merging the same two examples: press open, press play, `NotImplementedError`)*. A document names a generator and never carries one, so reopening a session somewhere the recipe is not held gives back the **reference** — a string. `form.render._emit_sequence` fell through to its "a sequence of raw values is data, not events" arm, iterating the reference character by character and raising, which takes the whole composition down over one lane. A frozen leaf is **structure**: it draws, it contributes its extent, and it emits no event — exactly what a `Vector` with no instrument already did, and what the crate's own frozen-generator decision says a host with no language shows. The same pass fixed its mirror image: a leaf that *is* resolved comes back as a `Generator` where the author wrote a bare `Element` (the conversion writes anything it has no body for as a generator leaf), and the two must flatten to the same events or a reopened piece would sound different from the one that was saved. Two tests pin both halves.

- ✅ **The two shared-transport examples could not run, and were silent when they did** *(found by running them, 2026-08-13)*. `sync.py` and `conductor.py` both opened by telling the reader to start a server by hand, so the first one raised `ConnectionRefusedError` on its first message; the conductor now `boot`s the shared server and the followers only connect to it, which keeps the subject (several *independent* clients meeting on one) while the file runs out of the box. Two more defects hid behind that: the followers each allocated node ids from the whole client range, so the server rejected every one of them as a duplicate (`Server(share=IdShare(i, 2))` is the mechanism that was there and unused), and `conductor.py`'s one-beat timeline drained after the first pass, freezing the printed positions — it loops now. Left as a note rather than a fix: two clients' *polled* positions differ by a few hundredths of a beat while each sample-clock tracker settles, so the print no longer claims they match.
- ✅ **A note edited in a clip's roll body never reached the arrangement** *(found 2026-08-13, reading `composer.py` against the editor)*. A clip's body is the `notes` element itself and it edits, but a body carries **no id of its own** — the host addresses the clip — so the drag arrived as `/gui_event <clip_id> "notes" …`, and `Editor.apply` resolved that tag against `_rolls`, a registry only `_draw_pianoroll` ever filled. In the multitrack view it was empty, so the message was dropped: the note moved on screen, the `Track`'s timeline kept the old one, and the next play sounded what was no longer drawn — the one edit-back that failed silently, since `"clip"` and `"points"` both resolve through `_clips`. `_clip_for` now registers what a roll body draws (for a **simultaneous** group, whose bodies layer in one clip, the member that carries the notes rather than the group), and the existing read-only rule still decides the rest: a generator registers too and its edit is refused where it always was. Worth noting against the entry below, which described this path as already working.
- ✅ **The same element placed twice is one name for two positions, and nothing says so** *(found 2026-08-14 by running the whole-loop example: a clip would not move, and the reason was two placements sharing a node)*. `Group([(0.0, take), (4.0, take)])` is the obvious way to write "this take, twice", and it is what `composer.py` did. But a node id is stamped on the **element object**, so both placements get the same one — `to_document` writes two members whose `node.id` is identical — and a `Place` intent naming that node is ambiguous: the crate applies it to the first match and the editor's index kept the last, so a drag on the second clip silently moved neither.

  Both examples now give each placement its own `Vector` element over the one server buffer, which is the right modelling and is what the layer's own vocabulary already says: an **element** is what a placement names, the *material* is what it wraps, and sharing material is not sharing an element. So the immediate bug is gone.

  **Settled 2026-08-17 by the crate's O14**: the node id belongs to the **member handle**, so writing the repeat is allowed and produces two windows with two names — for a leaf whose material the node *references* (a buffer, a generator, a pattern). An element carrying its material inside the node (an event, a track, a group) is refused, because two placements of one of those are two copies that diverge.

  **The reasoning as it stood before that, kept because the three answers are what the decision was about:** what is open is whether the model should let this be written at all. Three answers, and they are not equivalent. **Forbid it** — `Group.add` refuses an element already placed, which is a one-line check and an error message that teaches the distinction. **Copy it** — placing an element twice deep-copies, which is convenient and quietly makes two things out of what the reader wrote as one. **Name the placement instead of the node** — the intent addresses a member handle, which is the most faithful and the most expensive: `Place` naming a node is what makes it survive its siblings moving (the crate's own reason), and a member has no stable identity in the document today. Recording it as a decision rather than picking one here, because the cheap answer (forbid) and the correct answer (name the placement) point in different directions and the difference matters once a *library* of reusable material exists.

- ✅ **What a clip's edge means for an element that has not been rendered** *(found 2026-08-13; settled 2026-08-14 while building the whole-loop example, which meets all three of its questions at once)*. The record of what was asked is worth keeping, because the answer turned out to be **already decided** — by three separate decisions that nobody had read together. The question: the host clamps a dragged note into `[0, dur]` inside a clip and `Editor.apply` writes the result back, but the tree is **partially evaluated by design** — a generated element holds its notes as data, while a generator element is only *bounced* so the editor can draw it, so the notes on screen are a rendering of an algorithm and not the algorithm.

  **One sentence answers all three: a placement is a window onto an element, never a rewrite of it.**

  *Does clamping a bounced note change anything?* **No, and it must not pretend to** — which **O8** settled when it put a generator's last rendered result in the document as ordinary tree, with the rule that *a reader walks into it and an intent does not*, "because a rendering is not the composition and editing one writes over what the next render replaces". The editor already refuses such an edit, and since **O3** the refusal is visible rather than silent: the notes are pushed back as they still are.

  *Should a clip's length trim the generator's output, or materialize it?* **Trim** — which is what **C31** already decided a placement's length *is* ("a placement's length is what you hear of it", events past the end dropped through a copy, "a placement never rewrites the element it places"). Materializing is the layer's own change of state and it has its own verb, **render**; a resize gesture must not invoke it silently, because the two are different things a person does.

  *When a clip is shortened over notes that are still in the list, the host keeps them — does the model?* **Yes**, and it is the same rule again, now checked rather than assumed: shortening a four-note placement to two renders two events, the element still holds four, and lengthening it back renders four. Non-destructive and reversible, which is the four-layer table's own rule (an edit never writes a source) read at the level of a placement.

  What this leaves genuinely open is nothing — but it does leave a **capability** named: a gesture that *materializes* a generator into its rendered notes would be a real editing verb, and it belongs under "Future directions" as one rather than inside the meaning of a resize.

- ✅ **Ten examples play routines on a clock nobody started, and nothing says
  so** *(found 2026-08-15 while adding the server-side edit verbs to
  `buffer_edit.py`: the new cell printed nothing and changed nothing, and the
  cause was not the new code)*. `Routine(f).play(session.clock)` **queues
  forever** on a clock that is not running — `TempoClock.play` schedules and
  `sched` notifies, but neither starts the driver thread, and `Session.start`
  (or `run`) is what does. The routine sits in state `init`, no exception is
  raised and nothing is logged, so the failure looks exactly like silence.
  Eleven examples were written this way and three of them start the clock
  (`osc_destination`, `transport_freeze`, `gui_pianoroll`), which is what shows
  the idiom is right and the rest are wrong; `buffer_edit` is fixed with the
  work that found it, leaving **ten**: `spectral`, `wavetables`,
  `spectral_kernel`, `multichannel`, `typed_controls`, `pause_resume`,
  `convolution`, `graph_maths`, `boxes_library`, `spectral_cross`. Their
  audible half has never run. Two things to decide rather than one: the ten are
  a mechanical fix each needing an ear check, and separately **whether playing
  onto a stopped clock should stay silent at all** — a warning the first time
  would have caught all eleven the day each was written, and the ambient form
  (`Routine(f).play()`) starts its clock on first use, so the two doors already
  disagree about what a caller means.

  **Checked 2026-08-18, and the count is zero: the ten were never broken.** Every one of them is an **NRT** session that ends in `session.render(...)`, and `render` *is* a drive — it drains the queue in beat order without sleeping, which is what a stopped clock's driver thread would have done. All ten were run headlessly and all ten write audio (rms 0.04 to 0.20 over 1.5 to 6 seconds); nothing was mechanical to fix. What made `buffer_edit` fail was that it is **live**, and the entry generalised from it. The three examples named as proof that "the idiom is right" are live too, which is why they start the clock — so the pattern the entry spotted is real, and its population was one.

  **The other half was the real one, and it is taken.** Playing onto a stopped clock stops being silent at the one moment it can only be a mistake: a program that **ends** with a session's clock still queued and never driven prints one line on stderr naming `session.start()`, `session.run(seconds)` and `session.render()`. Not at `play` time, which was the tempting shape and is wrong — queueing before the drive starts is the normal way to build a score, offline *and* live, so a warning there would fire on almost every correct script. And only a **session's** clock: a bare `TempoClock` belongs to whoever built it (a transport, a test, another library object), and items left on its queue are that owner's business. `tests/test_session.py::test_a_score_left_on_a_clock_nobody_drove_says_so` runs both halves in a subprocess, since an exit warning cannot be observed from inside the program that would emit it.

- ✅ **Four UGen builders take their statics positionally, interleaved with real inputs** *(noted 2026-07-19 with M30's introspection work, not scheduled; moved here 2026-08-20 from the middle of the milestone list, where a pending item reads as done)*. Four `clausters.defs.ugens` callables take their **static** (non-signal) fields as ordinary positional parameters, interleaved with real inputs: `poll(trig, signal, label, trig_id)` — the worst, its `label` sits *between* two inputs — plus `disk_in`, `disk_out` and `pv_kernel`. `fft` and `conv` show the intended convention: statics behind a `*`, keyword-only, which makes their positional parameters line up with the wire exactly. Aligning the four would shrink the anti-drift exception list in `tests/test_session.py` to the three cases the wire genuinely forces (`EnvGen`, `SendReply`, `Dseq` — a variadic run last on the wire cannot be followed by a positional parameter in Python). It is deliberately **not** done as a side effect of the introspection work: it breaks the client's source API, so it belongs to a release that bumps the breaking tier.

  **Done 2026-08-26**, once the user settled the timing that was the whole of the deferral: nothing has been released, so everything that breaks rides the next one and there is no tier to wait for. The four now read `poll(trig, signal, trig_id=-1, *, label=...)`, `disk_in(chan=0.0, *, path, loop=False)`, `disk_out(signal, *, path, format=...)` and `pv_kernel(chain, *, mag, phase, params)`, so their positional parameters are the wire's inputs, in the wire's order, exactly.

  **The exception list is now empty and is kept that way**: `_WIRE_ORDER_STATIC_FIELDS_POSITIONAL` stays in `tests/test_session.py` as an empty dict, because its emptiness is the claim — the contrast test asserts the union of excuses is exact, so a new entry there would have to be written on purpose. What is left excused is only `_WIRE_ORDER_FORCED` (a variadic run last on the wire) and the panner family, whose `chan` the builder fills.

  **One of the four ran the other way, which is worth the record**: the web client's `pvKernel` already took its statics in an options object, so on that one it was the *Python* client that was behind — a reminder that the reference client is not automatically the correct one. The other three were positional in both and moved together.

  **The wire did not move**, and the parity vectors prove it rather than the argument: `gen-def-vectors.py` regenerated byte-identical, so this is a change of spelling in two languages and nothing else.

- ✅ **A `Source` for the other heavy props** *(noted 2026-08-23, with C46)*.
  The source object covers the signal family's samples, and the same shape is
  right for every other prop that carries a payload rather than a scalar: a
  roll's `notes`, a curve's `points`, a patcher's `boxes`/`cords`, a score's
  `display_list`. What stops it from being one change is that those are
  **normalized by their builders** (`_flat_points`, `_flat_notes`, `_flat_osc`)
  before the node exists, so a `Source` handed to one is flattened as if it were
  a list — while `data` reaches `node` untouched, which is the single expansion
  point C46 could use. Each of them needs its builder to let a source through,
  and `Source.set` needs the same flattening on the way back out (an edited
  structure rides as the JSON string its own `/gui_set` already takes). Worth
  doing when one of those props is next edited live from a script; until then a
  source there would be a surface that only half works.

  **Closed 2026-08-24.** A source now names a **structure** as well as samples
  — `source(points=…)`, `notes=`, `osc=`, `boxes=`, `cords=`, `display_list=`,
  one keyword per prop, under the same "exactly one way" rule the carriers
  already had — and every builder that flattens lets one through instead of
  flattening it (`_held`, the mirror of `_samples_arg`). The normalization is
  not duplicated: `_STRUCTURES` is the one table saying how each prop reaches
  the wire, `Source.props` calls it, and the builder calls the same function
  when it is handed a plain value, so both spellings put the identical flat
  list in the definition.

  What the carrier question becomes for a structure is *nothing to decide*: it
  rides in the prop it is named by, which is why `reload` refuses there — a
  structure holds its own payload and has nowhere to have moved from. The
  engraved page is the one that travels two ways, and the source hides it: a
  definition carries the display list as its five parts and a `/gui_set`
  carries the whole `display_list`, so `Source.set` picks the door rather than
  the caller.

  **A latent defect fell out of it.** `_rewrite_source` cleared *every* carrier
  key on the node it rewrote, which was right while a node could hold one
  source; a clip holds two (its take and its notes), and the first `set` would
  have cleared the other's prop. Each source now clears its own `slots()`.

  Shipped in both clients, case for case, with `notation.score_view` taking a
  page held as a source (it reads the page's size through `props()`) and both
  score examples driving their round trip through one — an edit reaches the
  definition too, so a re-open shows the score as edited rather than as
  engraved.

- ⬜ **`render()` cuts every take's release, in both clients** *(found
  2026-08-28, measuring the first and last sample of every WAV the examples
  write, after a listener asked whether the test synths click)*. The free
  verb, on a pattern:

  ```python
  stats = render(Pbind(instrument="default", degree=Pseq([0, 4, 7]), dur=0.25),
                 path="take.wav")   # 33600 frames; last sample -0.0725
  ```

  A render ends at the score's **last event**, and for a pattern that event is
  the gate closing on the final note — so the file stops there and the
  instrument's release (0.3 s for the built-in `default`) is never rendered.
  The take ends on a step of -22.8 dBFS: a click, on every file the verb
  writes, and it loops badly. `dur=` does not reach this — its docstring says
  so ("ignored by the other kinds: their content sets the length") — and
  `until=` moves where the clock stops draining, not where the score ends.

  The examples work around it by scheduling their own closing event
  (`/node_free 0` a release after the phrase), which is the idiom
  `OscNrtInterface.render`'s own docstring recommends and is now what the
  offline ones do. But **the verb should not need the workaround**: a caller
  who hands `render` a pattern has said everything there is to say about the
  length. What the fix is, is a decision — a `tail=` argument with a default,
  a bounce that keeps rendering until the node tree empties, or a renderer
  that stops when the output has been silent for a window — and it is one
  choice for both clients, so it is written here rather than taken.

  `clients/web/examples/buffers/offline-render.html` is where it still shows:
  the page renders through the free verb where its script renders through
  `session.render()`, so its downloadable take still ends on the step.

- ✅ **`OscEvent` and `MidiEvent` are not events, and the name promises a
  conversion that does not exist** *(found 2026-08-29 by the user, reading the
  three classes against each other while sizing the note-entry surface the
  engraving track needs; renamed 2026-08-30)*. Three classes share a prefix and
  nothing else.

  There is **no hierarchy**: `Event` is a `dict` subclass, `OscEvent` and
  `MidiEvent` are plain classes with two attributes and one, none inherits from
  any of them, and the only contract they share is `play(destination)` — which
  `Timeline`'s own docstring already names, and does not call an event: *"an
  item is anything that can render itself on a destination"*. `Event` is the
  only one of the three that carries musical time (`dur`/`delta`/`sustain`) and
  pitch; the other two wrap a raw payload and have no duration at all, which is
  why a timeline of them has no rhythm and why the MEI encoder skips them by
  asking for `midinote` rather than by asking what class they are.

  The second half is what makes the name actively misleading. **There is no
  conversion**: an `Event` never becomes an `OscEvent` or a `MidiEvent`, and
  those two are not an intermediate form of anything — they exist only to put a
  raw message on a timeline. Whether an `Event` sounds as OSC or as MIDI is
  **double dispatch on the destination**, not on the event: `Event.play` calls
  `destination.play_event(self)`, and the `Server` renders it as `/synth_new`
  plus a release while a `MidiServer` renders the same event as note on/off. So
  a shared `*Event` prefix suggests both a kinship and a translation step, and
  neither is there.

  The fix is the word the interface already uses: **`OscItem` and `MidiItem`**,
  which leaves `Event` as the only thing called an event — which is what it is.
  `MidiEvent.message` already reads as a message rather than as an event, so the
  attribute needs nothing. Both clients in the same commit
  (`clients/web/src/seq/timeline.ts` mirrors the two classes exactly), plus the
  `seq` pages of both books and the timeline examples. It **breaks the client's source API**, so
  like the positional-statics entry above it belongs to a release that bumps the
  breaking tier; whether the old names stand as deprecated aliases for one
  release is that release's call, not a decision this entry waits on.

  **Renamed 2026-08-30**, in both clients in one commit, with no deprecated
  alias: the old names are gone from `clausters.seq`, from the `seq` barrel of
  `clients/web/src` and from every docstring, comment and book page that named
  them, and the release that carries it bumps the breaking tier.

  The **piano-roll's lane carried the same word** and went with it. What the
  `pianoroll` draws below its grid is one marker per OSC or raw-MIDI *item*, so
  calling it "the OSC event lane" re-told the story the rename removed: it is
  **the OSC lane**, its contents are **markers** (the host already named the
  struct `OscMark`), and the prose says so in the host, both clients' `guidef`
  and `Editor`, `docs/gui-protocol.md`, the ADR and the two books. Two names
  moved with it — the web client's `OscEventSpec` is `OscMarkSpec`, and the
  theme colour `event_lane` is `osc_lane`, matching the prop it paints. The
  theme key is user configuration, so that one is a break too, and rides the
  same release.

- ✅ **Staging overwrote a library that was mapped, so refreshing the binaries
  killed a running process** *(found 2026-08-29 hunting a Python crash that left
  no output, and filed then as an unconfirmed hazard; **confirmed and fixed
  2026-08-30** when it happened again in the open — `scripts/refresh-bin.sh` was
  run while a window from `examples/notation/compose.py` was up, and that window
  died)*. `build_native.py` staged every artifact with `shutil.copy2(src, dst)`,
  which opens the destination for writing and **truncates it in place**. Any
  process holding the old file mapped — every Python that reached the package
  through `ctypes.CDLL`, and a running server or GUI host — is then reading
  pages that no longer exist and takes a `SIGBUS` on the next fault: a crash
  with no traceback, no message and no obvious cause, which is why the first
  occurrence was never identified.

  It was easy to hit precisely because refreshing the binaries is the
  *documented* thing to do before a manual test, and nothing suggested it was
  unsafe while something was open.

  Fixed with one helper, `_stage`: copy to a temporary in the destination's own
  directory, then `os.replace` over it. The rename is atomic and stays on one
  filesystem, so the old inode lives on for whoever still holds it and only new
  openers see the new file. All five call sites go through it (`stage`,
  `stage_binary`, the GUI host, the dependency walk, libverovio). The verovio
  *data* directory is still `rmtree` + `copytree` and has the same shape with a
  much smaller blast radius, since those files are read rather than mapped
  executable — left as it is, and named here so it is not rediscovered as a
  surprise.

- ⬜ **Two views of one arrangement keep two histories, so an undo writes a state
  nobody was in** *(found 2026-08-30 by the user, arguing that an undo stack
  belongs to the data and not to the view; measured the same day)*. `Editor`
  mints its own history — `self._log = _native.Log()` per instance, and its own
  `_document` derived from the tree — and the web client does the same
  (`this.log ??= new Log()`). But the tree is shared Python (or TS) objects, so
  two editors on one composition edit **one** dataset through **two** logs. The
  code documents that arrangement as supported: `apply`'s own docstring says a
  poll loop may be shared "even one shared with a second editor (a dedicated
  piano-roll beside the multitrack, say)", and `open_pianoroll` sets the mode on
  the editor it is called on, so two windows *are* two editors.

  Measured, on a `Track` of two notes, A the multitrack and B a dedicated roll on
  the same arrangement:

  ```
  start:             [60, 64]
  after A (2nd->65): [60, 65]
  after B (1st->62): [62, 65]
  A.undo() first:    [60, 64]   <- B's edit is gone, and B never asked
  B.undo() after:    [60, 65]   <- a state that never existed
  ```

  `b.can_undo` is `False` after A's edit, so the second view cannot undo what was
  done to the data it is showing; and each history, stepped in its own order,
  reverts across the other's edits. **This is the exact failure the document
  crate placed its log to avoid** — `log.rs`: "a script editing the arrangement,
  *a second editor*, or a re-render would leave that log describing a document
  that had moved on, and undo would write a state nobody was ever in". The crate
  avoided it for the host; the client reintroduced it one level up by keying the
  log to the editor instead of to the data.

  **The fix is not "share the log between editors"**, or not only: what it
  exposes is that **the client's data has no owner**. The arrangement is loose
  objects and every editor derives its own document from them, so there is
  nothing for a history to belong to. That is the same hole as "The mapping
  exists and is private to the `Editor`" (Future directions) seen from the
  history's side, and the shape it wants — an editable structure that owns its
  model and its log, with views attached to it — is what the editor
  generalization is about. Whatever is built, **the rule is settled and is not
  what is open**: the stack belongs to the data, an undo in one view updates the
  others, and the "combinable stack" only ever combines *different datasets* open
  in one session, never views of one.

  A cheap containment exists in the meantime and is worth naming: one editor is
  one history, so a script that wants two windows over one composition today has
  no correct way to do it, and the honest thing is that the second window is
  read-only. Both clients.

- ⬜ **An edit round-trips a note through the document, so a key the document
  cannot hold does not survive it** *(found 2026-08-31, by a crash while
  editing a note in a piece that had already played)*. A note's `Event` is not
  plain data once it has sounded: `Event.play` writes resolved values onto it,
  including its `server`. A roll edit states the lane as a `SetMembers`, whose
  member configs travel to the crate **as JSON**, so an event handed over raw is
  a `TypeError` in the middle of a drag. That part is fixed — the config is
  written through `leaf_config`, the same door `to_document` uses, which turns
  what it cannot serialize into a reference.

  **What stays open is what comes back.** `_project` writes the crate's
  effective config onto the element, so the key that left as a reference returns
  as one (`server` becomes `None`): the edit succeeds and silently replaces an
  object the author had put on that event. It is **better than it was** — the
  old rebuild-from-five-numbers dropped every key, the instrument included — and
  it is still a loss nobody asked for. The shape of the answer is that a
  projection should **merge** onto the event it already holds rather than
  rebuild it from the config, keeping what the document never claimed to carry;
  that is the same question as "what a leaf's config is *of*", so it wants
  reading against `O14` before it is written. **Both clients.**

- ⬜ **The editor's bridge freezes a tempo and drops the clock's anchor, so
  the line and the sound disagree by whatever a `set_tempo` moved** *(found
  2026-08-30 with the entry below; measured the same day, before touching it)*.
  `Editor.beats_to_units` calls `_native.beats_to_secs(self.tempo, 0.0, 0.0,
  beats)` — a straight line through the origin at a tempo **frozen at
  construction** — while `TempoClock` calls the same function with
  `self._base_beats, self._base_secs`, the anchor `set_tempo` moves so that a
  tempo change has no discontinuity. `Transport.beats_to_samples` inherits the
  same scalar, and the host's line sweeps by *engine samples* from the anchor's
  origin, so the drawing is what has to agree with the clock.

  **Measured**, at 48 kHz with the tempo doubled at beat 2 (1.0 → 2.0 beats per
  second): a clip drawn at beat 8 sits at 384 000 units, which the line reaches
  after **8.0 s** of wall clock, while the clock plays beat 8 at **5.0 s** —
  **3.0 s** apart. The two halves are separable: re-reading the tempo live would
  still leave 1.0 s (the discarded base), and the frozen scalar is the other
  2.0 s.

  What it is **not** is the unit question below: that one is fixed, and it took
  the *length* of a take off this mapping entirely (a length in seconds crosses
  on the rate now), so what is left here is the **onset** axis alone. The shape
  of the fix is a decision this entry does not take — the editor holds a scalar
  and the clock holds an anchor that moves, so either the editor reads the clock
  (a surface change in both clients) or the drawing is redone on every
  `set_tempo`; and a tempo that changed *twice* is a piecewise map, which one
  affine bridge cannot be. **Both clients**, and the `Transport` with them.

- ✅ **Three places call beats what is measured in seconds** *(found 2026-08-30
  by the user, from "el tiempo concreto o se mide en muestras o se mide en
  segundos, los beats son una abstracción impuesta")*. Beats are the
  `TempoClock`'s unit, and storing something in beats says *its seconds follow
  the tempo* — right for a note, wrong for anything whose seconds are already
  fixed. Three fixes, one finding to measure, and a prose pass:

  1. **`Automation` calls the seconds of an `Env` beats.** `Env` documents
     `times` as "the segment `times` **in seconds**"; `Automation.from_points`
     says "Times are **in beats**" and `duration()` says "The curve's length
     **in beats** (the sum of its segment times)" — and **nothing converts
     between them**: `points_to_env` stores the numbers as they arrive and
     `duration()` reads them back. One number with two names, working only
     because no conversion exists to be wrong. The curve is in seconds; the
     `Automation` side is what says it wrong.
  2. **A take's length is stored in beats.** `Vector.duration`,
     `Segments.duration` and `Segment.duration`, and `Vector.to_event` puts that
     number in the event's `dur`. A take's length is `frames / sample_rate`, so
     at double tempo the note is freed at half the wall-clock time over a buffer
     still playing at rate 1 — the take is cut. Durations of concrete material
     go in seconds.
  3. **`Segment` mixes the two bases in one tuple** — its own docstring says
     "``start`` is in frames, ``duration`` in beats"
     (`clausters/form/element.py`). One base for both fields.

  **The rule that decides each case** (the user's, and it is two rules): an
  **onset** is in the unit of what contains it — concrete material inside a
  sequence measured in beats is placed in beats; a **duration** is in the unit
  of the material — audio is seconds, a sequence of events is beats. Nothing
  that runs on a tempo clock changes: `Event.dur`/`delta`/`sustain`, a
  `Timeline`'s keys, patterns, routines, `Playhead`, and the editor's ruler and
  snap all stay in beats, and the conversion happens where the tree is flattened
  for playback (`form.render.to_timeline`), never in the structure — a timeline
  is ordered by one number and cannot hold two bases.

  **And the prose that follows the change**: the "in beats" lines of
  `form/element.py` and `form/aggregate.py` that stop being true, the crate's
  `Beats` alias and its doc lines, and wherever the books and
  `docs/architecture.md` explain the beats↔samples bridge. **Both clients**, and
  the format's half is `crates/clausters-document/PLAN.md` ("The document
  measures two kinds of leaf with one unit").

  **Fixed 2026-08-30, in one pass over the four packages.** The unit is
  **derived from what the element holds** and never stored: `duration_unit` on
  `Element` (`SECONDS` for `Vector`/`Segments` and for anything wrapped that
  measures itself in seconds — an `Automation`, whose times are an `Env`'s),
  `Body::duration_unit` in the crate, `Member.duration_unit` on a placement.
  The conversion moved to the **flattening** (`flatten(..., tempo=…)`, taken
  from the clock by `render`), the editor grew the **second ratio**
  (`units_per_second` beside `units_per_beat`, and `length_to_units` /
  `units_to_length` deciding which a number crosses on), the crate's `Place`
  stopped snapping a length that is not on the musical grid, `Mapping` carries
  `frames_per_second` beside `frames_per_beat` (`CORE_ABI_VERSION` 28), and the
  host draws a clip's `dur` through `clip_units`, which converts from the clip's
  own unit. `temporal_relation`/`relation` take a tempo, since an aggregate can
  now hold a lane of takes beside a lane of notes. Measured before the change,
  on a 96 000-frame take at 48 kHz: a `dur` of 2.0 sounded its two seconds at
  tempo 1 and was freed after **one** at tempo 2 — half the take.

## Future directions (a design that is not a fix)

Every entry carries a checkbox, like "Found by use" above: an open direction has
to read as open, and one that converges into a milestone leaves this list rather
than being ticked here.

- ⬜ **The mapping exists and is private to the `Editor`, so every example that
  plays writes a worse one** *(named 2026-08-30 by the user, from
  `examples/editors/pianoroll` in both clients — first as "nothing maps a drawn
  structure onto something that sounds", then narrowed the same day by reading
  what `Timeline`, `Playhead` and `Editor` already do)*.

  **What already works, and was the part that was missing from the question.** A
  `Timeline` holds items at beats and a `Playhead` plays it: the feeder walks the
  entries in order and calls `item.play(destination)` at each one's beat, with
  **no notion of a voice anywhere** — two `Event`s on the same beat both sound,
  each carrying its own `sustain`, so overlapping notes are two synths with their
  own releases. It seeks (`play(at=…)`, `locate`, `loop`), it reports `position`,
  `playing` and `finished`, and it follows the server's transport. The
  arrangement's own path is built on exactly that: `Editor.render` flattens the
  tree to absolute beats and plays it through a `Playhead`, and `Editor._apply_notes`
  rebuilds a roll's `"notes"` payload onto the element's `Timeline` as `Event`s,
  keeping the OSC/MIDI items that share it.

  **So the gap is not the mapping, it is that only the editor has it**, and it is
  three concrete things:

  - `session.play` takes an event pattern (`pattern.play(clock, server, quant)`).
    A `Timeline` has no `play`, and nothing in the session hands back a
    `Playhead`, so playing one means building the transport by hand.
  - the conversion from a roll's flat quintuples to a `Timeline` of `Event`s is
    `Editor._apply_notes`, private and reachable only through an arrangement. A
    script that opens a bare `pianoroll` has to write it again.
  - **`Ppar` and `Pmono` do not exist**, and are a separate question from the two
    above: they are pattern-side, and a pattern is serial by construction. They
    are named here so the two do not get confused again — the roll does not need
    them, and a `Pbind` is what makes the example monophonic.

  **What the example is doing, and why that is a choice.** It sorts the notes by
  onset, gives each event the distance to the next one and plays a single
  `Pbind`, which is why two notes drawn on top of each other are heard in
  succession. The same notes as a `Timeline` of `Event`s under a `Playhead` would
  have been polyphonic and seekable with no new machinery. The restriction is the
  example's, not the layer's.

  **What is undecided** is the shape of the public verb, and only that: a
  `session.play` that accepts a timeline, a `Timeline.play(destination, clock)`
  that returns its playhead, or a constructor (`Timeline.from_notes`) that takes
  what a roll sends — plus whether the pattern catalog grows `Ppar`/`Pmono` at
  all. Whichever it is, it is one decision for both clients, and the timeline is
  the candidate for the source of truth because it is the only flat structure the
  client has.

  **Related:** "A roll that sounds shows no cursor, and what can drive the line
  is a `Playhead`" (`clients/gui/PLAN.md`, Future directions) is the same
  question seen from the view.

- ⬜ **A drawn curve is a list of points, and `Env` is an envelope for `EnvGen`**
  *(named 2026-08-30 by the user, sizing what a general curve editor would need)*.
  The two are not two spellings of one thing, and the axis is what separates
  them. `Env` holds `levels`, segment `times` as **durations** (one fewer than
  the levels) and a curve each, plus `release_node`/`loop_node`: it is a
  *dynamic* envelope, whose contract is `EnvGen`'s — it sustains while the gate
  is held. A curve on a timeline is **absolute** — `(t, v, shape, curve)` per
  break-point — and has no sustain and no loop to speak of. `env_to_points` /
  `points_to_env` therefore convert rather than repack, and the conversion is
  **lossy in one direction**: `release_node` and `loop_node` have nowhere to go,
  and the last point carries a linear placeholder no segment uses.

  **The wire already speaks points**: the `"points"` edit-back is `t v shape
  curve` per break-point and the `bpf` prop takes the same. The only thing that
  insists on `Env` is the client, in `Automation`, which keeps one so it can
  discretize it into a control buffer.

  **And `Automation` holds two things at once**: the curve (the data) and its
  placement with its targets (a beat, `(node, control)` pairs, `play` as a
  timeline item). That is why a curve cannot be edited without an arrangement —
  the only object that owns one is also a placement.

  The direction: the break-point list is the curve editor's model, `Env` stays
  what `EnvGen` plays, and `Automation` is the placement of a curve on targets.
  **What is open** is whether the point list becomes a named type in both
  clients or stays the flat list the wire uses, whether `Env` keeps a round trip
  at all or only `points_to_env` at render time, and what a drawn curve would do
  if it ever needed a sustain — the one thing `Env` says that points cannot.
  **Related:** "The mapping exists and is private to the `Editor`" — this is the
  same question for the third data kind.

- ⬜ **`Track` wraps a `Timeline`, so the tree has two ways of placing things**
  *(named 2026-08-30 by the user: a track is a restricted `Aggregate`, and the
  timeline is not the tree)*. An `Aggregate` places members by offset; a
  `Timeline` places items by beat; `Track` is an `Element` that wraps one, so
  both mechanisms live in the same tree and the editor has to ask which it is
  looking at, at every node. The visible consequence is `_editable_timeline`,
  which answers **only** for an element wrapping a `Timeline`: a `Sequence` of
  `Clang`s — musically the same thing — draws in a roll and cannot be edited.
  Editability depends on what the element wrapped, not on what it is.

  **The document crate is already written for the other model.** Its header says
  the tree stays general and that a lane is a *projection* ("there is no lane
  here"); its `SetMembers` intent is documented as "the roll's edit: notes added,
  moved and removed arrive as the resulting list. Members keep their ids". So the
  document already thinks a roll's notes are members of an aggregate, with
  identity — the outlier is the client's `Track`, which keeps them as timeline
  items with no node.

  What it would give, beyond removing the second mechanism: every note gets an
  id, which is what makes an edit survive its siblings moving; and so do the OSC
  markers, which are un-editable today *because* they are items without nodes.
  What `Timeline` keeps is the role it already has everywhere else — the flat,
  playable projection anything flattens to (`Element.to_timeline`), not a
  container in the tree.

  **What is open**: where the timeline's own verbs land once a track is an
  aggregate (`quantize`, `from_pattern`, random access, the `Playhead` scan —
  flatten-then-play already covers the last), how it meets the crate's open
  decision on member identity, and that it is the arrangement model in both
  clients plus the bridge that writes the document. **Related:** "Two views of
  one arrangement keep two histories" (Found by use) — a note with no id is also
  a note a history cannot name.
