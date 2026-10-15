# Clausters — Python client

[![PyPI](https://img.shields.io/pypi/v/clausters)](https://pypi.org/project/clausters/)
[![Python client book](https://readthedocs.org/projects/clausters-python/badge/?version=latest)](https://clausters-python.readthedocs.io/)
[![Server book](https://readthedocs.org/projects/clausters/badge/?version=latest)](https://clausters.readthedocs.io/)
[![Web client book](https://readthedocs.org/projects/clausters-web/badge/?version=latest)](https://clausters-web.readthedocs.io/)

📖 **Documentation:** [Python client book](https://clausters-python.readthedocs.io/)
(this package) · [server book](https://clausters.readthedocs.io/) (the OSC
protocol, the wire formats, the engine) ·
[web client book](https://clausters-web.readthedocs.io/) (the same model in the
browser)

High-level Python client for the [Clausters](../../README.md) audio server,
ported selectively from SuperCollider's class library
([sc3](https://github.com/smrg-lm/sc3)). It covers both of the server's
definition formats as peers: **UGen-graph `SynthDef`s** and **`FaustDef`s** —
the latter built, equally, from the `signals` API, the `boxes` API, or Faust
source.

The package is pure Python at runtime (stdlib only) and reaches the native
side through `ctypes`, so client-side math matches the server by construction.

## Quick start

Install [from PyPI](https://pypi.org/project/clausters/) — the wheel is
self-contained (client + server + GUI host + libfaust; Linux x86_64 for now):

```sh
pip install clausters
pip install "clausters[editor]"   # also clausters-editor, the Python editor
```

The `editor` extra adds **clausters-editor**, a Python editor with an
interactive session that opens on the environment it is installed in. On Linux
it needs the system's WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`), which no wheel
carries; its command says how to install it when it is missing.

The client is driven **interactively** — from a REPL, or cell by cell in an
editor. No globals to set up, no build context to open: every line stands alone.

```python
from clausters import Session
from clausters.defs import SynthDef, Synth, control, sine, out

session = Session.live(latency=0.1)   # attaches to a server, or boots one
server = session.server

# an instrument: a def the server compiles once and instantiates many times
sdef = SynthDef("beep", out(0.0, sine(control("freq", 440.0)) * control("amp", 0.2)))
sdef.send(server)                      # /def_send synth, waits for the server's /done

node = Synth("beep", {"freq": 330.0}, server=server)   # you hear it now
node.set({"freq": 550.0})              # change it while it sounds
node.free()                              # silence
```

The other def format is a peer, not a fallback — the same voice as a `FaustDef`,
which the server JIT-compiles (Faust ships inside the wheel; nothing to install):

```python
from clausters.defs import signals as S, boxes as box, FaustDef

freq = S.hslider("freq", 440.0, 20.0, 20000.0, 0.01)
phase = S.rec(lambda p: (p + freq / S.sr()) % 1.0)         # one-sample feedback phasor
FaustDef.from_signals("fbeep", S.sin(phase * S.TAU) * 0.2).send(server)

# or point-free with the box API, borrowing the oscillator from Faust's library:
FaustDef.from_box(
    "bbeep", box.faust("os.osc")(box.hslider("freq", 440.0, 20.0, 20000.0, 0.01)) * 0.2).send(server)

Synth("bbeep", {"freq": 220.0}, server=server)
```

Or hand a **pattern** to the session; its clock runs in its own thread, so the
REPL stays yours:

```python
from clausters.seq import pbind, pseq, pwhite

session.play(pbind(instrument="beep", freq=pseq([330, 440, 550, 660], repeats=8),
                   dur=0.25, amp=pwhite(0.1, 0.2)))
session.start()      # keep typing while it plays
session.stop()
session.close()      # stops everything the session started (server, GUI)
```

The same code runs **offline** or **in-process** by changing one call —
`Session.nrt()` renders it to a WAV, `Session.embed()` runs the server inside
this interpreter. See the [Getting started](docs/src/getting-started.md) chapter.

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
  `timebase` (monotonic or, anchored to the server's sample clock, `/sched_at`),
  `netaddr`, `main`, the destination interfaces `_oscinterface` (UDP / **TCP**
  / NRT) and `_midiinterface`, and the minimal OSC wire encoder `_osclib`.
- `clausters.defs` — the definitions and server resources. Two def families,
  peers of each other: `ugens` (lowercase callables → `Ugen`/`Control`) with
  `SynthDef` (`/def_send synth`) — the server's UGen graph, plus `GraphDef` for
  multi-node graphs with per-voice partitions — and `FaustDef` (`/def_send faust`),
  built from `signals` (Faust's Signal API as lowercase callables), from
  `boxes` (its Box API, point-free, with `boxes.faust` pulling in the Faust
  libraries) or from Faust source. All of it instance-based, with no global
  build context. Alongside them the `Synth`/`Group`/`Bus`/`Buffer` handles and
  allocators, and `Server`. The **`Server` owns the communication interface and
  emits**: swap its interface to retarget a routine from live RT to an NRT
  score without touching clock or routine. `clocksync` models the
  server's sample clock over UDP (`Server.sample_clock()`) for drift-free
  `/sched_at` timing without shared memory.
- `clausters.seq` — sequencing: `Event` (a note plays a synth and frees it
  after its sustain), the value patterns (`pseq`, `pwhite`, `pseries`, …) and
  `pbind`, and `EventStreamPlayer`. `pbind(...).play(clock, server)` runs live
  or builds an NRT score by which interface the `Server` holds — with
  **yield-exact** timing (monotonic pacing, wall-clock timetags). `timeline`
  adds **static timelines with a random-access playhead** and a
  server-broadcast transport (conductor play/stop/locate across clients).
- `clausters.responders` — the input path: `OscFunc` and `MidiFunc` register
  callbacks on incoming OSC replies (`/node_trigger`, `SendReply` addresses, …) and live
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
- The **`clausters` and `clausters-gui` console scripts** — the wheel bundles
  both binaries, so `pip install` puts both commands on `PATH`, each behaving
  exactly like its cargo-built binary (`clausters --tcp`, `clausters --nrt
  score.osc out.wav`, `clausters-gui --standalone`, …).
- The **`editor` extra** (`clausters[editor]`) — installs `clausters-editor`, a
  separate package with its own command, whose session runs on the environment
  it is installed in.
- **The books, offline**: `clausters/_docs/` is this documentation as
  Markdown, the API reference included, and `clausters/_server_docs/` holds the
  user's parts of the server's book. clausters-editor shows both in its
  documentation viewer, where its search and its help (`Ctrl+D`) reach them.
- `clausters.launch` — launching and owning the server and GUI as child
  processes. `Session.live()` connects to a running server or starts one if none
  is up (choosing a shared-memory segment); `Session.gui()` starts the visual
  server (`clausters-gui`) wired to it; both stop when the session closes or the
  interpreter exits. Without a `Session`, `Server().boot()` / `GuiHost().boot()` do
  the same at the object level, one server per port; `Server(port=…).attach()`
  takes a handle on one that is already running without owning it, and
  `clausters stop|panic|status` does the same from a terminal. Both binaries are
  bundled in the wheel.

The TypeScript client sits on the same core and the same OSC contract, in the
browser (see [`../web/PLAN.md`](../web/PLAN.md)); the design both share is in
[`PLAN.md`](PLAN.md).

The full documentation (this client's guide and the generated API reference) is
the mdBook in [`docs/`](docs/). Runnable, installed-package examples are in
[`examples/`](examples/) (the broader catalog is the repo-root
[`examples/`](../../examples/)).

## Installing (pip / wheels)

The package is pure Python at runtime but reaches Rust through three cdylibs
that **cargo** builds (`libclausters_ffi` for the numeric core, `libclausters`
built with `embed,realtime` for the embedded server / offline render, and
`libclausters_midi` built with `live` for the MIDI files *and* the virtual MIDI
ports a `MidiFunc` listens on). The packaging
**bundles** them inside the wheel, so an installed package is self-contained —
no `target/` directory, no separate build step at import time.

Self-contained includes **Faust**: both def families are on by default in the
server, so the wheel also carries `libfaust` (in `clausters/_libs/`, found
through the artifacts' `$ORIGIN` rpath). A `FaustDef` therefore compiles on a
machine without it installed. That is what makes the wheel heavy — ~47 MB
packed, the largest part of it LLVM, which is linked into libfaust because the
Faust compiler *is* an LLVM JIT. It carries **libverovio** the same way, with
its SMuFL glyph data, so the `score` widget engraves with nothing installed.
Building the client from this repo needs both libraries present (see the steps
below); a `pip install` of a built wheel needs nothing.

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

`pip install` triggers `setup.py`, which runs `cargo build` for the three cdylibs and
stages them in `clausters/_libs/` before packaging them into the wheel (the
system build dependencies are listed in [`BUILD.md`](../../BUILD.md)). On Linux
that staging also needs **patchelf** — it rewrites each vendored library's run
path so the bundle resolves itself — from your package manager or as a
self-contained wheel (`pip install patchelf`). Building a redistributable wheel
explicitly:

```sh
pip install build                                # once: the PEP 517 front end
rm -rf clients/python/dist clausters-editor/wheel/dist   # dist/ keeps earlier builds, which the globs below would match
python -m build --wheel clients/python           # -> clients/python/dist/*.whl
pip install clients/python/dist/clausters-*.whl  # self-contained, no cargo
python -m build --wheel clausters-editor/wheel   # the editor's (the editor extra)
pip install clients/python/dist/clausters-*.whl clausters-editor/wheel/dist/*.whl
```

The wheel also bundles the **visual server** (the `clausters-gui` host binary),
built from the independent `clients/gui` workspace and stripped, so the one
package is self-contained — server *and* GUI. `Session.live` / `Session.gui`
launch them for you (see the guide); nothing else to install. For a lighter,
server-only wheel, set `CLAUSTERS_SKIP_GUI_BUILD` when building (a source-checkout
`clients/gui/target` binary is still used at runtime if present).

### From the repository, step by step

The whole thing the way a user gets it — installed, with the server, the GUI and
the editor launched from Python — in a fresh venv. **Run every command from the
repo root**, so the build hooks find the cargo workspace and the editor.

**1. System dependencies** (once). On Ubuntu/Debian:

```sh
# the server, the client's native libraries and their staging
sudo apt install build-essential pkg-config libasound2-dev libpipewire-0.3-dev \
  clang libdbus-1-dev patchelf python3-venv
# to build libfaust
sudo apt install cmake make g++ llvm-18-dev zlib1g-dev
# to build the editor (Tauri over WebKitGTK)
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

Plus Rust ([rustup](https://rustup.rs), the stable toolchain) and Node 22 with
npm, which builds the editor's interface. [`BUILD.md`](../../BUILD.md) has the
optional features' dependencies, and the editor's are those of its own
[README](../../clausters-editor/README.md).

**2. libfaust, libverovio and pydoc-markdown** (once). The two libraries are
built from a pinned source into `~/.local`, where the build finds them
(`FAUST_PREFIX` / `VEROVIO_PREFIX` point elsewhere); pydoc-markdown generates
the API pages of the book the package carries:

```sh
third_party/build-faust.sh     # libfaust with LLVM linked in
third_party/build-verovio.sh   # the score widget's engraver
uv tool install --python 3.12 pydoc-markdown   # or pip install pydoc-markdown
```

A build that does not find one stops and names what to install. To work on
something else without it, `CLAUSTERS_SKIP_FAUST=1`, `CLAUSTERS_SKIP_VEROVIO=1`
or `CLAUSTERS_SKIP_DOCS=1` builds without that piece.

**3. The venv and the install:**

```sh
python3 -m venv .venv && . .venv/bin/activate     # Windows: .venv\Scripts\activate
pip install -e "./clients/python[editor]" ./clausters-editor/wheel \
  --group ./clients/python/pyproject.toml:dev
```

This builds, in release, the three cdylibs, the server, the GUI host and the
editor, and stages each into its package — several minutes the first time.
`clausters` is installed editable; `clausters-editor` is a plain wheel, which pip
takes as the one the `editor` extra names, as it would take PyPI's. The `dev`
group is pytest (pip's `--group` reads `./pyproject.toml` unless given a path,
and this one lives in `clients/python/`); drop it, or the editor's two
arguments, for a lighter install.

`scripts/install-venv.sh` does this step in one command: it creates `.venv` when
it is missing, updates pip (`--group` takes 25.1) and runs the install above
(`--no-editor` and `--no-dev` drop those pieces). `--reset` first deletes the
local state the server, the client and the editor keep -- config, data
directory, the editor's environments, stale shared-memory segments -- after
listing it, to start from a clean user.

(To evaluate the examples cell by cell, open the `.py` in VS Code with the Python
+ Jupyter extensions, or a Jupyter notebook, and run its `# %%` cells — nothing
else needed in the venv.)

**4. Verify**, in order:

```sh
pytest -q clients/python                             # a) the client suite
clausters --help | head -1                           # b) server binary on PATH
clausters-gui --help | head -1                       #    visual server on PATH
python clients/python/examples/io/embedded.py        # c) sound with nothing to start
python clients/python/examples/views/editor.py       # d) client launches server + GUI
clausters-editor                                     # e) the editor, on this venv
```

In (d) `Session.live` starts a server if none is running (choosing a
shared-memory segment) and `session.gui()` starts `clausters-gui` wired to it. As
a script it follows the playhead for a while, then tears everything down. To work
**interactively** — cell by cell, keeping the window open and driving
`session`/`gui`/`win` between cells (`gui.set(...)`, `play_pass()`,
`gui.close(win)`) — open the file's `# %%` cells in VS Code / Jupyter. Either
way, everything the session started **stops** when it finishes or is closed, so
nothing is left running (check with `pgrep -laf 'clausters($| )|clausters-gui'`).
In (e) the editor's session runs on this venv's Python, so `import clausters`
works there from the first line.

**5. After changing code:**

- **Python in `clients/python`** — nothing: the install is editable. The
  offline books are a copy, though: after a docstring or a page of either
  book changes, `python clients/python/build_docs.py` stages them again.
- **Rust (server, core, GUI host)** — `scripts/refresh-bin.sh`. The package
  loads the binaries **staged inside it** before the workspace `target/`, so a
  rebuilt crate is not what runs until it is staged again; a manual test would
  silently exercise the old one.
- **The editor** — repeat step 3's `pip install`: it is not editable. With the
  binaries already built (`npm run tauri build -- --no-bundle` in
  `clausters-editor/`), `CLAUSTERS_SKIP_NATIVE_BUILD=1
  CLAUSTERS_EDITOR_SKIP_BUILD=1` in front of it stages them without rebuilding
  anything.

`scripts/install-venv.sh --refresh` does all three on an existing `.venv`:
the binaries, the book and the editor, without resolving any dependency.

Step 3 builds the GUI binary (wgpu, a minute or two the first time); set
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
- `CLAUSTERS_SKIP_FAUST` / `CLAUSTERS_SKIP_VEROVIO` — build without libfaust (no
  `FaustDef`) or without libverovio (the `score` widget does not engrave).
- `CLAUSTERS_SKIP_DOCS` — build without the offline book (no pydoc-markdown
  needed).
- `CLAUSTERS_EDITOR_SKIP_BUILD` — for `./clausters-editor/wheel`: stage the
  editor binary already built instead of building it.
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

📖 **[clausters-python.readthedocs.io](https://clausters-python.readthedocs.io/)**
— the client's book online: the guide, the document chapter and the API
reference. The server's own book (the OSC protocol, the wire formats, the
engine) is at **[clausters.readthedocs.io](https://clausters.readthedocs.io/)**,
and the web client's — the same client model in a browser — at
**[clausters-web.readthedocs.io](https://clausters-web.readthedocs.io/)**; the
three cross-link.

The book lives in this repository as an mdBook — a guide plus an API reference
**generated from the package docstrings** — so it also builds and reads offline.
It is a separate book from the server/workspace book at the repo root (two
books, one per platform, cross-linked). Build it:

```sh
uv tool install --python 3.12 pydoc-markdown   # user-space CLI in ~/.local/bin
cargo install mdbook --version 0.4.40           # the version CI and Read the Docs use
clients/python/docs/build.sh                    # writes src/api/, then runs `mdbook build`
```

`pydoc-markdown` is installed here as a **user-space** [uv](https://docs.astral.sh/uv/)
tool (no venv to manage, no sudo) **pinned to Python 3.12** — its dependencies
lag the newest CPython, and 3.12 is also what Read the Docs builds with.
`uvx pydoc-markdown` runs it without installing; a plain `pip install
pydoc-markdown` works too in any environment that is not externally managed
(PEP 668).

`build.sh` runs two steps: `docs/gen_api.py` runs `pydoc-markdown` (a **static
AST parse** of the public modules — no cdylib needed) and writes
`docs/src/api/`, one page per module, then `mdbook build
docs` renders the book to `docs/book/` (both git-ignored). For a live-reload
preview, after generating the API pages once:

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
