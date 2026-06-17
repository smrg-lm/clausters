# Clausters — Python client

High-level Python client for the [Clausters](../../README.md) audio server,
ported selectively from SuperCollider's class library
([sc3](https://github.com/smrg-lm/sc3)), **Faust-first**. Built in milestones;
see [`../PLAN.md`](../PLAN.md).

Milestones C0–C5 are done. In place now:

- `clausters.transport` — low-level transports (embedded server, shared memory,
  offline render); stdlib only. Its public names (`Clausters`, `ShmClient`,
  `render`) are re-exported from the top-level `clausters` package.
- `clausters._native` — ctypes binding over the shared native core
  (`clausters-ffi`): numeric builtins, seeded white noise and clock/sample
  math, matching the server by construction.
- `clausters.base` (C2) — the base layer: `builtins` (scalar/list math, f32 via
  the core), `absobject` (operator overloading), `stream` (`Routine`/`Stream`,
  the `yield` layer), `clock` (`TempoClock`, RT + NRT drives — **timing only**),
  `netaddr`, `main`, the destination interfaces `_oscinterface`/`_midiinterface`,
  and the minimal OSC wire encoder `_osclib`.
- `clausters.defs` (C3) — Faust-first definitions and server resources:
  `signals` (lowercase callables mapping Faust's Signal API, composed into the
  JSON graph), `FaustDef`, the `Synth`/`Group`/`Bus`/`Buffer` handles and
  allocators, and `Server`. The **`Server` owns the communication interface and
  emits** (C4): swap its interface to retarget a routine from live RT to an NRT
  score without touching clock or routine. UGen-graph definitions are also here
  — `ugens` (lowercase callables → `Ugen`/`Control`) and `SynthDef` (`/d_recv`),
  the instance-based UGen counterpart of `signals`/`FaustDef`.
- `clausters.seq` (C5) — sequencing: `Event` (a note plays a synth and frees it
  after its sustain), the value patterns (`Pseq`, `Pwhite`, `Pseries`, …) and
  `Pbind`, and `EventStreamPlayer`. `Pbind(...).play(clock, server)` runs live
  or builds an NRT score by which interface the `Server` holds — with
  **yield-exact** timing (monotonic pacing, wall-clock timetags).
- `clausters.Session` — ergonomic defaults **without global state**: bundles a
  `Server` and a clock; `Session.nrt()` / `Session.live()` factories,
  `.play(pattern)` / `.render()` / `.run(s)`. Several sessions coexist (an
  offline NRT one for plots next to a live RT one) in the same script.

See [`clausters/GUIA.md`](clausters/GUIA.md) for a hands-on, milestone-by-
milestone manual test guide (Spanish).

## Building the native libraries

The package is pure Python at runtime but reaches Rust through two cdylibs that
**cargo** builds (not pip), found automatically under the workspace
`target/{release,debug}/`:

```sh
# from the repo root
cargo build -p clausters-ffi                      # libclausters_ffi (the core: _native)
cargo build --features embed,realtime             # libclausters    (transport: render/Clausters)
```

Override the locations with `CLAUSTERS_FFI_LIB` / `CLAUSTERS_LIB` if needed.

## Running the smoke tests

```sh
cd clients/python
python -m pytest          # or: python tests/test_smoke.py
```

Boundary rule (project-wide): only flat data crosses any binding — Python
floats/ints in, `array('f')`/bytes out.
