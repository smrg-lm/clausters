# Clausters — Python client

High-level Python client for the [Clausters](../../README.md) audio server,
ported selectively from SuperCollider's class library
([sc3](https://github.com/smrg-lm/sc3)). It covers both of the server's
definition formats as peers: **FaustDefs** built from the `signals` API (or
from Faust source) and **UGen-graph `SynthDef`s**.

The package is pure Python at runtime (stdlib only) and reaches the native
side through `ctypes`, so client-side math matches the server by construction.

## What is in the package

- `clausters.ipc` — low-level transports (embedded server, shared memory,
  offline render); its public names (`Clausters`, `ShmClient`, `render`) are
  re-exported from the top-level `clausters` package.
- `clausters._native` — the ctypes binding over the shared native core
  (`clausters-ffi`): numeric builtins, seeded white noise and clock/sample
  math, matching the server by construction.
- `clausters.base` — the base layer: `builtins` (scalar/list math, f32 via
  the core), `absobject` (operator overloading), `stream` (`Routine`/`Stream`,
  the `yield` layer), `clock` (`TempoClock`, RT + NRT drives — **timing only**),
  `timebase` (monotonic or, anchored to the server's sample clock, `/sched`),
  `netaddr`, `main`, the destination interfaces `_oscinterface` (UDP / **TCP**
  / NRT) and `_midiinterface`, and the minimal OSC wire encoder `_osclib`.
- `clausters.defs` — the definitions and server resources:
  `signals` (lowercase callables mapping Faust's Signal API, composed into the
  JSON graph), `FaustDef`, the `Synth`/`Group`/`Bus`/`Buffer` handles and
  allocators, and `Server`. The **`Server` owns the communication interface and
  emits**: swap its interface to retarget a routine from live RT to an NRT
  score without touching clock or routine. UGen-graph definitions are also here
  — `ugens` (lowercase callables → `Ugen`/`Control`) and `SynthDef` (`/d_recv`),
  the instance-based UGen counterpart of `signals`/`FaustDef`, plus `GraphDef`
  (multi-node graphs with per-voice partitions). `clocksync` models the
  server's sample clock over UDP (`Server.sample_clock()`) for drift-free
  `/sched` timing without shared memory.
- `clausters.seq` — sequencing: `Event` (a note plays a synth and frees it
  after its sustain), the value patterns (`Pseq`, `Pwhite`, `Pseries`, …) and
  `Pbind`, and `EventStreamPlayer`. `Pbind(...).play(clock, server)` runs live
  or builds an NRT score by which interface the `Server` holds — with
  **yield-exact** timing (monotonic pacing, wall-clock timetags). `timeline`
  adds **static timelines with a random-access playhead** and a
  server-broadcast transport (conductor play/stop/locate across clients).
- `clausters.responders` — the input path: `OscFunc` and `MidiFunc` register
  callbacks on incoming OSC replies (`/tr`, `SendReply` addresses, …) and live
  MIDI, in sclang style.
- **MIDI output** (`MidiServer` in `clausters.base`, over the
  `clausters-midi` crate) — play an event pattern to a MIDI destination:
  **export** it as a Standard MIDI File (`.mid`) or a 16-bit-velocity
  **MIDI 2.0 clip**, or play it **live** out a virtual OS MIDI port.
- `clausters.gui` — client-side `GuiDef` building for the Clausters GUI host
  (windows, control widgets, meters/scopes, node-tree views, canvas).
- `clausters.Session` — ergonomic defaults **without global state**: bundles a
  `Server` and a clock; `Session.nrt()` / `Session.live()` / `Session.embed()`
  factories, `.play(pattern)` / `.render()` / `.run(s)`. Several sessions
  coexist (an offline NRT one for plots next to a live RT one) in one script.
- `clausters.config` — reads the shared TOML configuration (user file +
  project `clausters.toml`), the same schema the server reads.
- The **`clausters` console script** — the wheel bundles the standalone server
  binary, so `pip install` also puts a `clausters` command on `PATH` that
  behaves exactly like the cargo-built binary (`clausters --tcp`,
  `clausters --nrt score.osc out.wav`, …).
- `clausters.launch` — launching and owning the server and GUI as child
  processes. `Session.live()` connects to a running server or starts one if none
  is up (choosing a shared-memory segment); `Session.gui()` starts the visual
  server (`clausters-gui`) wired to it; both stop when the session closes or the
  interpreter exits. Without a `Session`, `Server.boot()` / `GuiHost.boot()` do
  the same at the object level. Both binaries are bundled in the wheel.

Still to come: a JavaScript client on the same C ABI and OSC contract (see
[`../PLAN.md`](../PLAN.md)).

The full documentation (this client's guide and the generated API reference) is
the mdBook in [`docs/`](docs/). Runnable, installed-package examples are in
[`examples/`](examples/) (the broader catalog is the repo-root
[`examples/`](../../examples/)).

## Installing (pip / wheels)

The package is pure Python at runtime but reaches Rust through two cdylibs that
**cargo** builds (`libclausters_ffi` for the numeric core, `libclausters` built
with `embed,realtime` for the embedded server / offline render). The packaging
**bundles** them inside the wheel, so an installed package is self-contained —
no `target/` directory, no separate build step at import time.

The simplest, standard, self-contained setup in a fresh venv (run from the repo,
so the build hook can find the cargo workspace):

```sh
python -m venv .venv && . .venv/bin/activate
pip install -e ./clients/python --group ./clients/python/pyproject.toml:dev
# (editable + the pytest dev group; pip's --group reads ./pyproject.toml unless
# given a path, and this repo's lives in clients/python/)
# or a plain install:
pip install ./clients/python
```

`pip install` triggers `setup.py`, which runs `cargo build` for both cdylibs and
stages them in `clausters/_libs/` before packaging them into the wheel (the
system build dependencies are listed in [`BUILD.md`](../../BUILD.md)). Building
a redistributable wheel explicitly:

```sh
python -m build --wheel clients/python           # -> clients/python/dist/*.whl
pip install clients/python/dist/clausters-*.whl  # self-contained, no cargo
```

The wheel also bundles the **visual server** (the `clausters-gui` host binary),
built from the independent `clients/gui` workspace and stripped, so the one
package is self-contained — server *and* GUI. `Session.live` / `Session.gui`
launch them for you (see the guide); nothing else to install. For a lighter,
server-only wheel, set `CLAUSTERS_SKIP_GUI_BUILD` when building (a source-checkout
`clients/gui/target` binary is still used at runtime if present).

### Everything at once, in a clean venv

To exercise the whole thing the way a user gets it — installed, with the server
and GUI launched from Python — set up a fresh venv. **Run every command from the
repo root** so the build hook can find the cargo workspace:

```sh
# 1) a clean venv
python -m venv .venv && . .venv/bin/activate     # Windows: .venv\Scripts\activate

# 2) the client, editable (builds the cdylibs and the server + clausters-gui
#    binaries with cargo, then packages them into the one self-contained package):
pip install -e ./clients/python

# 3) for the test suite:
pip install pytest
```

(To evaluate the examples cell by cell, open the `.py` in VS Code with the Python
+ Jupyter extensions, or a Jupyter notebook, and run its `# %%` cells — nothing
else needed in the venv.)

Then verify, in order:

```sh
pytest -q clients/python                          # a) the client suite (~146 green)
clausters --help | head -1                        # b) server binary on PATH
clausters-gui --help | head -1                    #    visual server on PATH
python clients/python/examples/embedded.py        # c) sound with nothing to start
python clients/python/examples/gui_editor.py      # d) client launches server + GUI
```

In (d) `Session.live` starts a server if none is running (choosing a
shared-memory segment) and `session.gui()` starts `clausters-gui` wired to it. As
a script it follows the playhead for a while, then tears everything down. To work
**interactively** — cell by cell, keeping the window open and driving
`session`/`gui`/`win` between cells (`gui.set(...)`, `play_pass()`,
`gui.close(win)`) — open the file's `# %%` cells in VS Code / Jupyter. Either
way, everything the session started **stops** when it finishes or is closed, so
nothing is left running (check with `pgrep -laf 'clausters($| )|clausters-gui'`).

Step 2 builds the GUI binary (wgpu, a minute or two the first time); set
`CLAUSTERS_SKIP_GUI_BUILD=1` to skip it for a faster, server-only install (a
`clients/gui/target` binary built with `cargo build --release --bin
clausters-gui` is still used at runtime if present).

The audio paths (`Session.live`/`embed`, the standalone server) need PipeWire
on Linux; the offline renderer and numeric core (`Session.nrt()`,
`clausters._native`) run anywhere. The GUI needs a display and a GPU adapter.

Knobs (env vars), all optional:

- `CLAUSTERS_WORKSPACE` — path to the cargo workspace, if it can't be found by
  searching upward (e.g. an isolated build of a copied tree).
- `CLAUSTERS_CARGO_FEATURES` — features for the embed library (default
  `embed,realtime`).
- `CLAUSTERS_SKIP_NATIVE_BUILD` — package the libs already staged in
  `clausters/_libs/` without rebuilding.
- `CLAUSTERS_SKIP_GUI_BUILD` — don't build/bundle the heavy `clausters-gui`
  binary (a lighter, server-only wheel).
- `CLAUSTERS_GUI_FEATURES` — extra cargo features for the GUI binary (e.g.
  `standalone`); default none.
- `CLAUSTERS_FFI_LIB` / `CLAUSTERS_LIB` — at runtime, point a loader directly at
  a cdylib (overrides the bundled copy and the workspace `target/`).
- `CLAUSTERS_BIN` — at runtime, point the `clausters` console script at a
  specific server binary.
- `CLAUSTERS_GUI_BIN` — at runtime, point the launcher (`Session.gui` /
  `GuiProcess`) at a specific `clausters-gui` host binary (overrides the bundled
  copy and the workspace `target/`).

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

## Documentation

This client has its own mdBook — a guide plus an API reference **generated from
the package docstrings**. It is a separate book from the server/workspace book
at the repo root (two books, one per platform, cross-linked). Build it:

```sh
uv tool install --python 3.12 pydoc-markdown   # user-space CLI in ~/.local/bin
cargo install mdbook                            # once (or use a distro / prebuilt mdbook)
clients/python/docs/build.sh                    # writes src/api.md, then runs `mdbook build`
```

`pydoc-markdown` is installed here as a **user-space** [uv](https://docs.astral.sh/uv/)
tool (no venv to manage, no sudo) **pinned to Python 3.12** — its dependencies
lag the newest CPython, and 3.12 is also what Read the Docs builds with.
`uvx pydoc-markdown` runs it without installing; a plain `pip install
pydoc-markdown` works too in any environment that is not externally managed
(PEP 668).

`build.sh` runs two steps: `pydoc-markdown` (a **static AST parse** of the
public modules — no cdylib needed) writes `docs/src/api.md`, then `mdbook build
docs` renders the book to `docs/book/` (both git-ignored). For a live-reload
preview, after generating the API page once:

```sh
mdbook serve --open clients/python/docs    # http://localhost:3000
```

## Running the tests

```sh
cd clients/python
python -m pytest          # or: python tests/test_smoke.py
```

Boundary rule (project-wide): only flat data crosses any binding — Python
floats/ints in, `array('f')`/bytes out.

## License

**GPL-3.0-or-later** — see [COPYING](COPYING), shipped inside the wheel. The
bundled cdylibs embed the Clausters server, whose optional libfaust is GPLv2+.
