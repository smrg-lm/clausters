---
name: clausters-python
description: How to use the Clausters Python client idiomatically — the client/server split and the NRT/RT/embed seam, building Faust/UGen defs from the signal API, sending defs asynchronously with the /sync barrier, and sequencing with BOTH event patterns and generator routines (and the rule that a routine must never block the clock thread). Consult when writing any client-side Clausters Python.
---

# Clausters Python client

The high-level client (`clients/python/clausters`), a selective Faust-first port
of SuperCollider's sc3. It builds OSC and drives a Clausters server — live over
UDP/TCP, offline as an NRT score, or in-process via the embed library — from one
codebase. Code, comments and identifiers are English (project rule); this skill
is the idiomatic map, to be refined over time.

## Mental model

- **Server / client split.** Timing and graph building are *server-agnostic*;
  only `Server` knows how to talk to a server. Keep that line: a `TempoClock`
  never sends; the `Server` sends, reading the running routine's logical time.
- **The seam.** `Server` owns a *communication interface*. Swap the interface to
  retarget the *same* code:
  - live RT → `Server()` (UDP) / `OscTcpInterface` / `Server(latency=...)`,
  - offline NRT → `Server(interface=OscNrtInterface())` (accumulates a score),
  - embedded → the `transport` layer (`Clausters`, `render`) over the cdylib.
- **Three actors:** `Server` (comms + resource allocators), `TempoClock` (tells
  time, schedules), `Routine`/patterns (sequence logic). `Session` bundles a
  `Server`+`TempoClock` with `nrt()`/`live()` factories when you want ergonomics
  without globals.

## Building instrument defs

Two def kinds, both built with **lowercase callables** that compose via Python
operators and become a JSON tree:

```python
from clausters.defs import signals as S, FaustDef, SynthDef
# Faust signal API -> {"signals":[...]} tree (one node per output):
freq = S.hslider("freq", 330.0, 20.0, 20000.0, 0.01)
phasor = S.rec(lambda s: (s + freq / 48000.0) % 1.0)   # explicit 1-sample feedback
fdef = FaustDef.from_signals("sine", S.sin(phasor * 6.2831853) * 0.2, ...)  # >1 arg = >1 output
```

- `signals`: arithmetic + `sin/cos/...`, `delay`/`delay1` (Faust `'`), `rec`/`self_`
  (Faust `~`, one feedback sample), `hslider`/`nentry`/... (labels become control
  names), `input(n)`, tables. DSP can be built purely from these primitives (e.g.
  a biquad = `rec` + `delay1` per `fi.tf2`).
- **Sample rate:** use `signals.sr()` (the port of Faust's `ma.SR`) — *do not* bake
  an `SR` constant into the graph. `sr()` is a foreign constant (`fconst`) the
  server resolves at def-compile time, so the def stays in tune at whatever rate
  the engine/NRT renderer runs (`freq / S.sr()`, RBJ coeffs, etc.). `signals.PI`/
  `signals.TAU` are *literals* (Faust's `ma.PI` is one too), so they are plain
  Python floats — no server round-trip.
- `FaustDef.from_signals/from_source/from_box`; `SynthDef` is the UGen
  counterpart (`/d_recv`), instance-based, only `+ - * /` compose UGens.
- Reserved controls `in`/`out` (bus selectors) are added by the server.

## Sending defs is asynchronous — use the /sync barrier

`/d_faust` JIT-compiles on the server's network thread (and `/b_*` run on an NRT
thread). The client mirrors scsynth:

```python
srv.add_faustdef(fdef)                 # RT: BLOCKS until /done (or raises CommandError/ReplyTimeout)
srv.add_faustdef(fdef, wait=False)     # fire-and-forget (does not block)
srv.sync()                             # barrier: /sync->/synced, waits for ALL earlier async work
srv.add_synthdef(sdef, wait=...)       # same shape for UGen defs
```

- `wait=True` (default) blocks on `/done`; `wait=False` only sends — then call
  `sync()` before the `/s_new` that needs the def.
- In **NRT** `add_*` always *scores* the def at time 0 (the renderer compiles
  before time advances); `wait` does not apply.
- `Server.sync()` is the real barrier (not a `/status` round-trip): a `/synced`
  guarantees prior compiles/buffer jobs are installed.

## Sequencing: event patterns AND generator routines

Two idioms — pick by the shape of the logic, not by habit.

**Event patterns** — for streams of notes. A `Pbind` zips value patterns into
`Event`s; each event plays itself (`/s_new` with controls, then a scheduled
`/n_free`), so you never hand-write a bundle:

```python
from clausters.seq import Pbind, Pseq, Pwhite
pat = Pbind(instrument="rlpf", freq=110.0, cutoff=Pseq([400,900,1600]),  # finite -> ends
            amp=Pwhite(0.08, 0.18), dur=0.25, legato=1.0)
pat.play(clock, server)                # EventStreamPlayer: a Routine yielding event.delta()
# or: Session.nrt(tempo=2).play(pat)
```

**Generator routines** — for anything beyond a flat note stream: branching,
state, coordinating several nodes, mid-note `/n_set` modulation, conditional
timing. A routine is a generator; `yield <beats>` cedes time to the clock, which
resumes it at that exact logical beat. You can still create `Event`s and call
`event.play(server)` inside it to avoid writing bundles by hand:

```python
from clausters.base import Routine, TempoClock
from clausters.seq import Event

def voice(server):
    for hz in (400, 900, 1600, 900, 400):
        Event(instrument="rlpf", freq=110.0, cutoff=float(hz),
              dur=0.25, legato=1.0).play(server)   # Event emits /s_new (+ scheduled /n_free)
        yield 0.25                                 # advance the clock; never time.sleep
    # ...arbitrary logic, conditionals, nested routines, etc.

clock = TempoClock(tempo=1.0)
clock.play(Routine(lambda: voice(server)))         # lambda = a 0-arg factory (so reset can re-create it)
```

`server.send_bundle((addr, *args), delay_beats=...)` is the lower level when no
`Event` fits; it stamps each message for its beat (OSC timetag live, score time
in NRT) using the running routine's logical time.

### The golden rule: a routine must NEVER block the clock thread

A routine runs *on the clock thread* (RT) or inside the render loop (NRT).
Blocking it freezes the whole timeline. So inside a routine generator:

- **never** `time.sleep` — `yield` the beats instead;
- **never** call a blocking `sync()` or a `wait=True` def send (they wait on a
  reply). To create a def from a routine, send it `wait=False` and `yield`
  enough time before the dependent `/s_new` (a non-blocking, yield-able barrier
  is future `OSCFunc`/notification work).

## Timing

`TempoClock(tempo)` — `tempo` is **beats per second**. `beats2secs`/`secs2beats`
convert. Driving:

- NRT: `clock.render()` drains the queue in beat order with no sleeping, then
  `server.render(sample_rate=..., channels=...)` -> `(samples, frames)`.
- RT: `clock.run(seconds)` (start thread, sleep, stop) or `start()`/`stop()`.
- Live `Server(latency=0.2)` (scsynth `s.latency`): places timetags slightly
  ahead so bundles arrive on time — without it the server logs `late bundle ...
  executing immediately`. Size `clock.run(...)` as
  `beats2secs(total_beats) + latency + tail`.

## Resources, control, errors

- Allocators on `Server`: `nodes`, `audio_buses`, `control_buses`, `buffers`.
- `srv.synth(name, {ctl: val}, target=0)`, `srv.set(node, {...})`,
  `srv.map(node, ctl, bus, audio=False)`, `srv.free(*nodes)`,
  `srv.alloc_buffer(frames, channels)`.
- Errors: `clausters.errors` hierarchy (all subclass `ClaustersError` *and* the
  matching builtin). Notably `LibraryFeatureError` = the cdylib was built
  without a feature; `CommandError` = a `/fail` reply; `ReplyTimeout`.

## Build & run (and the E2E sandbox rule)

- Live server with Faust: `cargo run --release --features faust`.
- NRT render / embed needs the cdylib **with features**:
  `cargo build --release --features embed,realtime,faust`. A plain `cargo build`
  overwrites `libclausters.so` without the FFI symbols → `LibraryFeatureError`
  (rebuild with the features).
- **E2E rule:** the bash sandbox isolates the network between invocations — run
  server and client in the **same** invocation (server in background, then the
  client, then kill):

  ```sh
  (./target/release/clausters & SRV=$!; sleep 1.5; \
   PYTHONPATH=clients/python python3 your_client.py; kill $SRV 2>/dev/null)
  ```

## Closing a feature

When a client feature lands, update (where applicable): docstrings (the docs
source), `clients/python/GUIA.md` (manual steps), `docs/` for user-facing wire
formats, `LOG.md`, and a commented `examples/*.py`. Keep code, `LOG.md` and
`PLAN.md` English; `GUIA.md` and the conversation Spanish; commit messages
English/ASCII.
