# Clausters — Python client

High-level Python client for the [Clausters](../../README.md) audio server,
ported selectively from SuperCollider's class library
([sc3](https://github.com/smrg-lm/sc3)), **Faust-first**. Built in milestones;
see [`../PLAN.md`](../PLAN.md).

Milestones C0–C12 are done (C13 responders and the JavaScript client are the
remaining tracks). In place now:

- `clausters.transport` — low-level transports (embedded server, shared memory,
  offline render); stdlib only. Its public names (`Clausters`, `ShmClient`,
  `render`) are re-exported from the top-level `clausters` package.
- `clausters._native` — ctypes binding over the shared native core
  (`clausters-ffi`): numeric builtins, seeded white noise and clock/sample
  math, matching the server by construction.
- `clausters.base` (C2) — the base layer: `builtins` (scalar/list math, f32 via
  the core), `absobject` (operator overloading), `stream` (`Routine`/`Stream`,
  the `yield` layer), `clock` (`TempoClock`, RT + NRT drives — **timing only**),
  `timebase` (monotonic or, anchored to the server's sample clock, `/sched`),
  `netaddr`, `main`, the destination interfaces `_oscinterface` (UDP / **TCP**
  (C8) / NRT) and `_midiinterface`, and the minimal OSC wire encoder `_osclib`.
- `clausters.defs` (C3) — Faust-first definitions and server resources:
  `signals` (lowercase callables mapping Faust's Signal API, composed into the
  JSON graph), `FaustDef`, the `Synth`/`Group`/`Bus`/`Buffer` handles and
  allocators, and `Server`. The **`Server` owns the communication interface and
  emits** (C4): swap its interface to retarget a routine from live RT to an NRT
  score without touching clock or routine. UGen-graph definitions are also here
  — `ugens` (lowercase callables → `Ugen`/`Control`) and `SynthDef` (`/d_recv`),
  the instance-based UGen counterpart of `signals`/`FaustDef`. `clocksync`
  (C6) models the server's sample clock over UDP (`Server.sample_clock()`) for
  drift-free `/sched` timing without shared memory.
- `clausters.seq` (C5) — sequencing: `Event` (a note plays a synth and frees it
  after its sustain), the value patterns (`Pseq`, `Pwhite`, `Pseries`, …) and
  `Pbind`, and `EventStreamPlayer`. `Pbind(...).play(clock, server)` runs live
  or builds an NRT score by which interface the `Server` holds — with
  **yield-exact** timing (monotonic pacing, wall-clock timetags).
- `clausters.Session` — ergonomic defaults **without global state**: bundles a
  `Server` and a clock; `Session.nrt()` / `Session.live()` factories,
  `.play(pattern)` / `.render()` / `.run(s)`. Several sessions coexist (an
  offline NRT one for plots next to a live RT one) in the same script.

See [`GUIA.md`](GUIA.md) for a hands-on, milestone-by-
milestone manual test guide (Spanish). Runnable, installed-package examples are
in [`examples/`](examples/) (the broader catalog is the repo-root
[`examples/`](../../examples/)).

## Installing (C12 — pip / wheels)

The package is pure Python at runtime but reaches Rust through two cdylibs that
**cargo** builds (`libclausters_ffi` for the numeric core, `libclausters` built
with `embed,realtime` for the embedded server / offline render). The packaging
**bundles** them inside the wheel, so an installed package is self-contained —
no `target/` directory, no separate build step at import time.

The simplest, standard, self-contained setup in a fresh venv (run from the repo,
so the build hook can find the cargo workspace):

```sh
python -m venv .venv && . .venv/bin/activate
pip install -e ./clients/python --group dev      # editable + the pytest dev group
# or a plain install:
pip install ./clients/python
```

`pip install` triggers `setup.py`, which runs `cargo build` for both cdylibs and
stages them in `clausters/_libs/` before packaging them into the wheel. Building
a redistributable wheel explicitly:

```sh
python -m build --wheel clients/python           # -> clients/python/dist/*.whl
pip install clients/python/dist/clausters-*.whl  # self-contained, no cargo
```

Knobs (env vars), all optional:

- `CLAUSTERS_WORKSPACE` — path to the cargo workspace, if it can't be found by
  searching upward (e.g. an isolated build of a copied tree).
- `CLAUSTERS_CARGO_FEATURES` — features for the embed library (default
  `embed,realtime`).
- `CLAUSTERS_SKIP_NATIVE_BUILD` — package the libs already staged in
  `clausters/_libs/` without rebuilding.
- `CLAUSTERS_FFI_LIB` / `CLAUSTERS_LIB` — at runtime, point a loader directly at
  a cdylib (overrides the bundled copy and the workspace `target/`).

In a plain source checkout (no install), the loaders fall back to the workspace
`target/{release,debug}/`, so the historic build-and-run workflow still works:

```sh
cargo build -p clausters-ffi
cargo build --features embed,realtime
```

You can also stage the native libs by hand (e.g. before a `pip install` that
won't reach the workspace) with the standalone script:

```sh
python clients/python/build_native.py            # release; --debug for the debug profile
```

## Running the tests

```sh
cd clients/python
python -m pytest          # or: python tests/test_smoke.py
```

Boundary rule (project-wide): only flat data crosses any binding — Python
floats/ints in, `array('f')`/bytes out.
