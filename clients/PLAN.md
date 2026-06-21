# Plan — High-level clients for Clausters (Faust-first), with a shared native Rust core

This plan covers the **Python** client (the first target) but is written to serve a future **JavaScript** client too: both share the same native Rust core and the same C-ABI contract. The language-specific part is only the coroutine driver and the thin binding wrappers.

> **Note — sc3 as the reference model.** For any design or semantics question (module structure, clock/routine behavior, events, patterns, OSC/MIDI interfaces, names, conventions), fall back to [sc3](https://github.com/smrg-lm/sc3) as the model. This client is a clean, pruned rewrite (Faust-first), but sc3 is the source of truth on how these pieces should combine and behave; deviate from it only with an explicit reason (the Clausters-specific parts: FaustDefs, server resources, native Rust core).

## Context

Clausters is the Rust audio server (scsynth-style) controlled over OSC. Today the only client in the repo is `clients/python/clausters.py`: the **low-level transport layer** (embed cdylib / shm / render), stdlib-only, with the boundary rule "only flat data crosses" (bytes in, `array('f')`/floats/ints out). There is no high-level layer: building defs, resources, events and sequencing is currently left to the user.

The goal is a **high-level client** that selectively ports the core features of [sc3](https://github.com/smrg-lm/sc3) (a SuperCollider port to Python), but **centered on FaustDefs** instead of SynthDefs, and reusing the server's resources (buses, buffers, generator units). In parallel a **native Rust core** is extracted (TempoClock, numeric builtins, OSC assembly) shared by the server and by all future clients (Python now, JavaScript later), so that client-side operations are **numerically equivalent** to the server's by construction wherever possible.

Agreed decisions:
- **Repo**: clean rewrite in `clients/python/` (sc3 as reference, without dragging in SynthDef or the full class library).
- **Rust**: turn `clausters` into a Cargo **workspace** and extract a core crate (`clausters-core`).
- **Binding**: a single **C-ABI** over the core, with thin per-language wrappers (ctypes/cffi in Python; N-API or wasm in JS later).
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
  PLAN.md                   # this plan (generic, also for the future JS client)
```

`clausters-core` (pure library, a `no_std` candidate except where it needs `alloc`):
- **builtins**: unary/binary ops over scalars and over `&[f32]` slices — the same formulas as the server. Base set: `add/sub/mul/div` (already native in the server), and the higher math that today exists in the server only via Faust (`sin/cos/tan/exp/log/sqrt/abs/floor/ceil/min/max/pow/atan2/...`, see `crates/clausters/src/faust/signals.rs`).
- **tempoclock**: time-priority queue + beat↔second↔sample arithmetic, tempo/meter, conversion against the server's sample-clock (read via `/clock` or via the shm data-plane).
- **rng**: a seeded generator that **replicates** the server's (`WhiteNoise` uses splitmix64/xorshift, `crates/clausters/src/dsp/noise.rs`) for client/server reproducibility.
- **osc**: message/bundle assembly with NTP timetag, reusing `rosc` (already a server dependency). `timetag ↔ sample target` conversion for `/sched` and bundles.

`clausters-ffi`: a cdylib that exports the core's C-ABI (explicit ABI version, like the current embed in `crates/clausters/src/embed.rs`). Distinct from the embed's `libclausters.so` (that is the in-process server; this is the client core). Two separate cdylibs, both consumable by ctypes/N-API/wasm.

### Numeric equivalence — a realistic contract

- Ops the server computes **natively** (`add/sub/mul/div`, `SinOsc` phase, `WhiteNoise` RNG): refactor the server to use `clausters-core` → **bit-exact by construction** (single source of truth). Mind RT-safety: `#[inline]` functions, no alloc/lock/IO (CLAUDE.md, `tests/rt_safety.rs`).
- Higher math that in the server exists **only via Faust/LLVM** (`sin`, `log`, etc.): `clausters-core` implements the **same formula/semantics** (libm), but bit-for-bit equality with Faust's LLVM codegen is **not guaranteed**. Contract: same formula + documented tolerance; parity tests with tolerance.

### Client package (Python example; the JS client mirrors the same structure)

Clean rewrite, mirroring sc3's structure but pruned and Faust-first:

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
- `base/stream.py` / `seq/`: coroutines with `yield`, patterns and events — pure Python (in JS: generators/async).
- `defs/signals.py`: **the user interface for building FaustDefs**. It provides a library of **lowercase callables** (functions or callable objects) that map, in principle, the **Faust Signal API** (`sin`, `cos`, `add`, `mul`, `delay`, `select2`, `hslider`, `rdtable`, …). The **composition** of these callables is what builds the graph: a specification serialized to a **JSON signal tree** now (and a **box tree** later) to send with `/d_faust` (see `crates/clausters/src/faust/`). Firm design convention: **lowercase names even for objects that act as functions** — a quality that eases programming work in Python (fluent expression-style composition). The **same pattern is reused for UGens** (`ugens.py`, constructors of the SynthDef graph in JSON).
- `defs/faustdef.py`: **the client's center**. It takes the graph built with `signals.py` (or direct Faust source) and produces the def for `/d_faust` in its three forms (source, JSON box tree, JSON signal tree); it manages controls (UI labels → control names; reserved `out`/`in`). On-disk persistence/cache is handled by the server (M16, bitcode cache). `synthdef.py` (later) does the analogous thing for the UGen graph.
- `defs/{node,bus,buffer,server}.py`: client-side ID allocators (scsynth-style: nodes, audio buses 0..127 / control 0..1023, buffers 0..1023), handling of `/done`/`/fail`, `/notify` → `/n_go`/`/n_end`. NRT: score → transport's `render()`.
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
| `defs/signals`, `defs/faustdef` | client-side JSON graph | `/d_faust`, `src/faust` |

### Target interfaces and time handling (RT / NRT / MIDI) — the central piece

The point that makes **one and the same clock-and-routine logic** serve real time, deferred render and MIDI without rewriting it. The correct split is:

- The **clock** (`base/clock.TempoClock`) only schedules and provides time (beat↔second↔sample math via `_native`, scheduling queue, RT/NRT drives, resuming `yield`). **It does not communicate with the server.**
- The **`Server`** owns the **target/communication interface** and **emits** the events, computing the timetag from the logical time of the running routine's clock (`main.current_tt`). Changing the interface changes *where* and *in which mode* (live vs deferred) the events go; clock and routines do not change.
- `base/_oscinterface.py`: `OscUDPInterface`/`OscTCPInterface` (RT; TCP not in the server yet) and `OscNrtInterface` (accumulates into `OscScore` → `render()`). `base/_midiinterface.py`: `MidiRtInterface`/`MidiNrtInterface`+`MidiScore`. shm/embed would be additional communication interfaces of the `Server`.

> **Post-C3 correction:** in C2 the communication ended up **misplaced in `TempoClock`** (fields `target`/`interface`, methods `send_bundle`/`send_msg`/`_emit`/`_when`). Milestone **C4** moves it to `Server`. The clock keeps only timing.

## Milestones (client "C" track, parallel to the server "M" track)

> Markers: **✅ done** · **⏳ pending** · milestone **unmarked** = future, not started.

- ✅ **C0 — Workspace + core + FFI**: convert to a workspace; create `clausters-core` (builtins, tempoclock, rng, osc) and `clausters-ffi` (C-ABI + version); refactor the server's native ops to consume `clausters-core`. Server↔core numeric parity tests (bit-exact in the native ones; documented tolerance vs Faust). Verify RT-safety intact. *(Completed 2026-06-17 — see LOG.md.)*
- ✅ **C1 — Client scaffold + accessible core**: `pyproject.toml`, `clausters/` package, relocate transport, `_native.py` (ctypes over `clausters-ffi`). Smoke: import, call a scalar/list builtin, instantiate `TempoClock`, assemble an OSC bundle, `render()`. *(Completed 2026-06-17 — see LOG.md.)*
- ✅ **C2 — base**: `absobject`/`builtins` (dispatch to native, scalar+list), `stream` (Routine/Stream with `yield`), `main` (global context, seeds), `clock` (TempoClock over the core), `netaddr`. Target interfaces: `_oscinterface` (`OscUDPInterface` + `OscNrtInterface`/`OscScore`) and `_midiinterface` (`MidiRtInterface` + `MidiNrtInterface`/`MidiScore`), so that clock and routines emit against a swappable interface. (`OscTCPInterface` stays a stub: TCP not implemented in the server yet.) *(Completed 2026-06-17 — see LOG.md.)*
- ✅ **C3 — Faust-first defs**: `signals.py` (lowercase callables that map the Faust Signal API; their composition builds the JSON signal tree), `faustdef` (the three forms for `/d_faust`, controls), `node`/`bus`/`buffer`/`server` (allocators, async `/done`-`/fail`, `/notify`). E2E vertical slice: build a graph with `signals` → `faustdef` → `/d_faust` → `/s_new` → control via bus/clock. *(Completed 2026-06-17 — see LOG.md.)*
- ✅ **C4 — Refactor: client/server separation** (post-C3 correction, surgical — do not rewrite what works) *(Completed 2026-06-17 — see LOG.md)*: pull communication out of `TempoClock` (fields `target`/`interface`, methods `send_bundle`/`send_msg`/`_emit`/`_when`) and move it to `Server`. The clock keeps only timing (math + queue + drives + resuming routines) and exposes logical/wall time; the `Server` owns the **communication interface** and **emits**, reading the time from the running routine's clock (`main.current_tt` carries its `clock`). Reconcile the two currently-duplicated communication layers: `defs/server.UdpConnection` (RT bidirectional, replies) and `base/_oscinterface.Osc*Interface` (send/accumulate), into one coherent **communication interface** the `Server` owns, with RT variants (UDP; later shm/embed) and NRT (score). The `Server` in NRT mode exposes `render()`. Update routines/tests/`GUIA.md`/examples: the pattern goes from `clock.send_bundle(...)` to `server.send_bundle(...)`. Without touching `signals`/`faustdef`/builtins/core.
  - Acceptance criterion: `TempoClock` imports and references no interface/NetAddr; the E2E slice (NRT and live) and the RT/NRT seam still pass with the new placement; a transport change (e.g. shm) is done by adding a communication interface to the `Server`, without touching clock/seq.
- ✅ **C5 — seq** *(Completed 2026-06-17 — see LOG.md)*: `event`, `pattern`, stream-patterns; one and the same `Pbind`+`TempoClock`+`Server` runs in **RT** (UDP interface, live server) or **NRT** (score interface → `render()`) **just by changing the `Server`'s communication interface**. **C5 leftover closed (2026-06-17)**: **instance-based** UGen graph — `defs/ugens.py` (lowercase callables → `Ugen`/`Control`, operators → `Add/Sub/Mul/Div`, no global build context) + `defs/synthdef.py` (`SynthDef` → JSON `SynthDefSpec` → `/d_recv`) + `Server.add_synthdef` (RT with `/done`, NRT scored at t=0). **Byte-identical** parity with the internal `default` (`tests/test_synthdef.py`), live E2E over UDP, example `examples/synthdef.py`. (The SynthDef graph no longer depends on the `sc3/synth` port; the rest of `sc3/synth` —more server-side UGens— is independent.)
  - ✅ **`TempoClock` semantics** (see memory `tempoclock-timebase-clausters`): time (beats) advances **only via the `yield`s**; the **monotonic** clock is used **only** to compute sleeps → **exact relational** timing. When emitting, the timetag is computed from the **accumulated logical beat** (not from "now"); the OSC timetag uses a separate **wall** clock (Unix), valid for the server. Accuracy test: `/s_new` at exactly `[0, 0.5, 1.0, 1.5]`.
  - ✅ **No globals that clobber across threads** (see memory `evitar-estados-globales-clausters`): `main.current_tt` is **thread-local**, so several `TempoClock`s (threads) and a live RT clock alongside an NRT render run **in the same script** without clobber. Explicit `Server`/`clock` per instance; `default_clock` only as optional sugar. Tests in `tests/test_concurrency.py` (thread-local, two concurrent NRT clocks, RT+NRT litmus in the same script).
  - ✅ **Selectable timebase** (`base/timebase.py`): `MonotonicTimebase` (default, events via NTP bundle) and `SampleClockTimebase` (anchored to the server's sample clock — `now = sample()/sr`); in sample-clock mode the `Server` emits via **`/sched <absolute_sample>`** (sample-accurate, drift-free) instead of a wall timetag. Robust tests with **both** options in `tests/test_timebase.py` (pacing, NTP emission vs `/sched` with exact sample, latency, and NRT identical regardless of timebase); `/sched` validated live.
  - ✅ **Score parity golden**: the render of the seq path (`Pbind`) is **byte-identical** to that of the equivalent hand-rolled OSC (same server engine via the embed render) — `tests/test_golden.py` (`list(hi)==list(lo)`, 91200 frames), an end-to-end test of the event/pattern/timing layer.
  - ✅ **Defaults ergonomics without globals**: `clausters.Session` (an explicit context that bundles `Server`+`TempoClock`, with factories `Session.nrt()`/`Session.live()` and `play`/`render`/`run`); **several sessions coexist** (NRT for plot + live RT in the same script), with no global state. `tests/test_session.py`.
  - ✅ **Instance-based graph** (closed 2026-06-17): `defs/ugens.py` + `defs/synthdef.py` (`SynthDef` → `/d_recv`) build the **instance-based** UGen graph (concurrent defs), without sclang's global build state. UGen counterpart to `signals`/`FaustDef`; byte-identical parity with the internal `default`. The rest of `sc3/synth` (more UGens) is server-side and independent.
- ✅ **C6 — UDP sample-clock anchoring** *(Completed 2026-06-17 — see LOG.md)*: `defs/clocksync.py` — `SampleClockModel` (least-squares `sample = a + b·t` over a sliding window of `/clock` anchors, round-trip midpoint; same model as `examples/sample_clock.py`) and `UdpSampleClock` (its own socket; `anchor`/`warmup`/`track` in the background; `.timebase()` → `SampleClockTimebase`). `Server.sample_clock()` builds it. So `SampleClockTimebase` works **live over UDP** (without shm/embed) and the `Server` emits via `/sched` anchored to the server's clock. Model tests (line recovery, drift ppm, 1-anchor fallback) + timebase smoke; **validated live** (query `/clock` → model → `/sched`, synths sound).
- ✅ **C7 — MIDI interfaces** *(re-planned 2026-06-17 → moved to Future milestones as **C11**)*: the first part was **poorly planned** (MIDI 1.0 in a Python library, client-only) and was redone. The final decision —a reusable native crate for client+server with MIDI 2.0/UMP— was taken out of the sequential track and lives in **C11** (the "Future milestones" section below) and in the root `PLAN.md`'s **M17**. The C7 slot is closed here so as not to stall sequential progress: **it does not affect what remains of C9 or C10.**
- ✅ **C8 — TCP interface** *(Completed 2026-06-17 — see LOG.md)*: both ends. **Server** (M track): `src/osc/tcp.rs` — `--tcp [port]` accepts length-prefixed OSC (4-byte BE prefix + bytes, scsynth framing) multiplexed in the single-thread loop with no async runtime and no new dependency (an acceptor thread + one reader thread per connection → `mpsc` channel drained each iteration like the M14 ring; wake via a zero-length UDP datagram to its own socket → without waiting for the GC tick; replies via the write-half owned by the network thread, `&TcpStream: Write`). `ClientId::Tcp(id)` routes replies to the originating connection. **Client**: a real `OscTCPInterface` (drop-in for `OscUDPInterface`; framing + reassembly of replies across TCP segments). Tests: `tests/osc.rs::tcp_*` (`/status`+`/d_recv` round-trip, per-connection routing), `clients/python/tests/test_tcp.py` (framing/reassembly with a fake socket), live E2E. Example `examples/tcp_client.py`. Timing still rides on timetags/`/sched`: arrival latency does not affect when a scheduled command fires.
- ✅ **C9 — multi-language + close-out** *(Completed 2026-06-17 — see LOG.md)*:
  - ✅ **Cross-language architecture documented**: a new mdBook chapter (`docs/clients.md`, in SUMMARY under "Library & Embedding") — the single C-ABI contract (`clausters-core`/`clausters-ffi` + embed/shm), the Python client (base/seq/defs layers), the path to the **JS** client (same C-ABI via N-API/wasm, generators/async instead of `yield`) and the **distribution** plan (Python wheels, npm/wasm JS, Faust in `third_party`). **Reuse confirmation**: Python (a non-Rust language) already drives the whole system through the C-ABI + OSC, proof that the boundary is not Python-specific.
  - ✅ **Commented client example** *(2026-06-17)*: `examples/sequencing.py` — a tour of the high-level sequencing layer (`Session` + `Pbind` + value patterns) with the **NRT/live seam** (the same pattern renders offline or plays live over UDP depending on the `Server`'s interface). Validated offline (render to samples/WAV) and live (E2E same Bash invocation). Cataloged in `docs/examples.md`.
- ✅ **C10 — Documentation and examples maintenance** *(swept up to date through C9, 2026-06-17 — see LOG.md; stays active as new milestones land)*: keep the mdBook (`docs/`), `clients/python/README.md`, the client's `GUIA.md` (steps + counts) and the examples up to date. Done in this sweep: refreshed the "C0–C5" → "C0–C9" states (README, `docs/clients.md`), cataloged `synthdef.py`/`tcp_client.py` in `docs/examples.md`, added the C9 row to the GUIA checklist, and **documented the `SynthDef` class as it ended up** (topological post-order + dedup, controls by name, **only `+-*/` operators** compose UGens, outputs must be UGens). `mdbook build` clean.

## Future milestones (client "C" track, parallel to the "M" track)

Client milestones **with no fixed sequential order**, to be tackled when appropriate (just like the server's "Future milestones M9+" in the root `PLAN.md`). They are numbered after the last of the sequential section (C10).

- ✅ **C11 — MIDI interfaces** *(moved from C7; **DONE 2026-06-19** — offline `.mid`/clip file output (M17 sub-part 1, 2026-06-18) and live `MidiRtInterface` out a virtual OS port (M17 sub-part 2, 2026-06-19), both via `MidiServer` + a swappable interface and the `clausters-midi` crate; the server MIDI protocol + live ALSA transport landed too, M17 sub-part 3. Commit `5455d01` and this M17-closing commit)*: complete `_midiinterface`. `MidiNrtInterface`/`MidiScore` with **MIDI file writing** for scores; `MidiRtInterface` with a **real backend** for live output. Map `Event` → **standard channel-voice MIDI** — note on/off via `sustain`, channel, velocity (← `amp`), `midinote` as the note number, and extra `f32` controls as per-note controllers / CC — **consistent with the server's `/midi_bind` control map and conversion helpers** (note generic SysEx is *not* used for actuation; see M17). The same mapping drives **SynthDef and FaustDef alike**. Same RT/NRT seam as OSC, via the interface the `Server` owns (or an analogous MIDI target). MIDI carries no timetags: timing comes from the clock at emit time. **Revised decision**: MIDI does not go in a Python library (python-rtmidi) nor client-only, but in a **reusable native crate for client+server** (`crates/clausters-midi`, versioned C ABI), with the message layer in **MIDI 2.0/UMP via `midi2`** (high resolution: 16-bit velocity, 32-bit controllers, `no_std`/non-allocating), persistence at full resolution in a **MIDI 2.0 Clip File via `midi2-clip`** and `.mid` (SMF, MIDI 1.0) via `midly` for interop. **See M17 of the root `PLAN.md`** for the full scope (server MIDI protocol + client output). *Crate evaluation outcome*: `midly` (SMF) and `midi2` (UMP) are used; **`midi2-clip` v0.1.0 was a stub** (`todo!()`), so the MIDI 2.0 clip container is assembled from `midi2`'s UMP messages.
- **C12 — Python client packaging (wheels)** *(DONE 2026-06-21)*: distribute the `clausters` package as a pip-installable **wheel**, bundling the native libraries (`libclausters_ffi`, `libclausters` embed) for the target platforms. Includes the **reproducible Faust build in `third_party`** the wheels need (already noted as user backlog). This is the **Python**-side packaging; the JS client's (npm) goes in the **J** track (see below). *Examples layout decision (2026-06-18): all examples stay under the repo-root `examples/` for now (a single flat catalog to review functionality and spot gaps while the client examples are still in flux — only `biquad_signal.py` uses the library idiomatically so far). The split into `clients/python/examples/` (the package-dependent examples shipping with the wheel, with their `sys.path` shims dropped in favor of an installed/`PYTHONPATH` import) is deferred to this milestone, when the client library and its examples have stabilized.*
  - **Done**: a `setuptools` build hook (`setup.py` + `build_native.py`) runs `cargo build` for `clausters-ffi` and `clausters --features embed,realtime` and stages the cdylibs in `clausters/_libs/`, packaged as a non-pure, `py3-none-<plat>`-tagged wheel; `_libpath.py` gives both loaders a shared precedence (env override -> bundled wheel copy -> workspace `target/`), so an installed package is self-contained and a source checkout still falls back to `target/`. Env knobs `CLAUSTERS_WORKSPACE` / `CLAUSTERS_CARGO_FEATURES` / `CLAUSTERS_SKIP_NATIVE_BUILD`. The `clients/python/examples/` split landed (`offline_render.py`, `live_udp.py`, shim-free). **Not done here** (still backlog, see below): the reproducible **Faust build in `third_party`** and Faust-enabled wheels — the wheel ships the core embed build (no `faust` feature); cross-platform CI wheels (cibuildwheel/manylinux) and PyPI publishing are likewise future work.
- **C13 — Responders (OSCFunc/MIDIFunc) + general-purpose OSC/MIDI I/O**: today the client is **output-only** — it builds OSC/MIDI and sends it to the server. C13 adds the **input** path and the client's role as a general MIDI/OSC hub, mirroring sclang's `OSCFunc`/`MIDIFunc`: receive OSC and MIDI from *any* application, match/dispatch to callbacks, and let those callbacks emit OSC/MIDI to the server **or to other apps**. It splits cleanly along the existing seam (server-agnostic vs server-specific): **(1) general-purpose, server-agnostic** — receive/send OSC and MIDI to/from arbitrary endpoints. New input transports: **MIDI input ports** in the `clausters-midi` crate (the `live` feature is output-only today; add `clausters_midi_input_open`/poll-or-callback/`_close`, ALSA seq in, behind the same versioned C ABI — an ABI bump), and an **OSC receive socket** in the client (UDP/TCP listener, reusing `osc::decode_packet`-shaped parsing on the Python side). A **dispatch layer**: `OSCFunc(path, matcher, callback)` and `MIDIFunc(kind, chan, callback)` register against an incoming-message demux that runs on its **own thread** and resumes callbacks on a clock. **(2) server-specific** — convenience responders that translate incoming MIDI/OSC into Clausters commands (a `MIDIFunc` that turns notes into `/s_new` on the `Server`, reusing the `Event`/`Server` machinery), the client-side counterpart of the server's direct MIDI path (M17/M19): the server can be played by MIDI directly *or* by a client that listens to MIDI and emits OSC — both coexist. **Threading discipline**: the input demux is a dedicated thread; callbacks are scheduled on a clock so they obey the **golden rule** (never block the clock thread — a responder that wants to sequence `yield`s like any routine), and `main.current_tt` thread-local is respected so responders, RT clocks and NRT renders don't clobber. Decision taken with the user (2026-06-19): a **single C13** (input transport + dispatch + general I/O together, not split into two milestones); MIDI input added to the existing `clausters-midi` crate; OSC receive socket added client-side. Closing includes docstrings, `clients/python/GUIA.md`, the `docs/` chapter on clients (the bidirectional role), `LOG.md` and a commented `examples/*.py` (e.g. a `MIDIFunc` driving the server, and an `OSCFunc` relaying to another app).

## Future milestones (JavaScript client "J" track)

A separate track for the **JavaScript client**, still **not planned in detail**: it must be planned later, **together with the npm packaging** (the JS equivalent of C12's wheels). The JS client is built **on top of what is done first with Python**: it mirrors its structure (base/seq/defs layers) and reuses the **same C-ABI** (`clausters-core`/`clausters-ffi` + embed) via N-API or wasm, with generators/async instead of Python's `yield`. The concrete plan (milestones J1, J2, …) is defined once the Python client is stable enough to serve as a model.

- **J — Real JS client + npm packaging** *(to be planned)*: C-ABI binding (N-API/wasm), porting the client layers mirroring Python, and npm distribution (including the Faust build for wasm in `third_party`).

## Organization conventions

- Native crates under `crates/`; per-language clients under `clients/<lang>/`. The **core C-ABI is the only contract** between Rust and each language, with an explicit ABI version (as `embed.rs` / `clausters.py` already do).
- Project-wide boundary rule: "only flat data crosses" (bytes/`array`/scalars/integers), in both the transport and the core.
- Client milestone track prefixed with `C` so it does not collide with the server's `M` track; close each one with the project's milestone checklist (code+tests, LOG/PLAN, developer doc, user doc in `docs/`, `GUIA.md`, an example in `examples/`).
- Code/comments/tests in English; `PLAN.md`/`LOG.md` in English; `GUIA.md` and conversation with the user in Spanish.

## Verification

- **Workspace**: `cargo build` and `cargo test` (without features and with `--features faust`) must pass; `tests/rt_safety.rs` and `tests/denormals.rs` stay green after the refactor.
- **Numeric parity**: a new test in `clausters-core` (or `tests/`) comparing the native builtin's output against the server's native branch (bit-exact) and against Faust (tolerance).
- **Client**: `pytest` of the package; smoke of `_native` (builtin, TempoClock, bundle, render).
- **E2E** (CLAUDE.md rule: server and client in the **same** Bash invocation): start `./target/debug/clausters &`, define a FaustDef from the high-level client, `/s_new`, control via bus, verify `/done`/replies, `kill`. NRT: score generated by `seq` → `render()` → compare WAV/golden.

## To validate during execution (non-blocking)

- Acceptable equivalence level for higher math vs Faust (a concrete tolerance).
- Whether a separate `cdylib` for `clausters-ffi` is preferable, or exposing its C-ABI from the same `libclausters` (initial preference: separate, so as not to couple client and server embed).
- The FFI-overhead threshold at which the scalar builtin uses a pure-language fallback instead of crossing the boundary.
