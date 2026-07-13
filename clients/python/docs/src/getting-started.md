# Getting started

## Install

The package is pure Python at runtime, but it reaches Rust through artifacts that **cargo** builds: two cdylibs — `libclausters_ffi` (the numeric/timing core) and `libclausters` (built with `embed,realtime` for the in-process server and offline render) — and the standalone server **binary**. The packaging **bundles** all of them inside one wheel, so an installed package is self-contained: the in-process embedded server and the offline renderer work out of the box, and the standalone server is on your `PATH` as the `clausters` command. No `target/` directory and no build step at import time.

The simplest setup, in a fresh virtualenv, run from the repository so the build hook can find the cargo workspace:

```sh
python -m venv .venv && . .venv/bin/activate
pip install -e ./clients/python --group ./clients/python/pyproject.toml:dev
# (editable + the pytest dev group; pip's --group reads ./pyproject.toml unless
# given a path, and this repo's lives in clients/python/)
# or a plain install:
pip install ./clients/python
```

`pip install` triggers `setup.py`, which runs `cargo build` for both cdylibs and stages them in `clausters/_libs/` before packaging. To build a redistributable wheel explicitly:

```sh
python -m build --wheel clients/python           # -> clients/python/dist/*.whl
pip install clients/python/dist/clausters-*.whl  # self-contained, no cargo
```

In a plain source checkout (no install), the loaders fall back to the workspace `target/{release,debug}/`, so the historic build-and-run workflow still works:

```sh
cargo build -p clausters-ffi
cargo build --features embed,realtime
```

### The visual server (GUI)

The GUI host — the visual server that opens windows and renders widgets — is **bundled in the same wheel** as the audio server, built from the independent `clients/gui` cargo workspace and stripped. Nothing extra to install: with the package in place, [`Session.live` / `Session.gui`](sessions.md#launching-the-server-and-the-gui) launch the audio and visual servers for you, and `clausters-gui` is also on your `PATH`.

Building it adds a wgpu compile (a minute or two the first time). For a lighter, server-only wheel, set `CLAUSTERS_SKIP_GUI_BUILD` when installing; a source-checkout binary built under `clients/gui/target` (`cargo build --release --bin clausters-gui`, from `clients/gui`) is still used at runtime if present, and `CLAUSTERS_GUI_BIN` overrides the lookup.

### Runtime requirement: PipeWire (Linux)

The bundled artifacts that touch an audio device — the in-process embedded server and the standalone `clausters` binary — are built with the project's default features, which include the PipeWire audio backend. They therefore **hard-link `libpipewire`** and expect PipeWire present at runtime (the standard on current Linux systems); on a host without it the library and the binary will not load. The offline renderer and the numeric core need no audio device, so `Session.nrt()` and `clausters._native` work anywhere. For an audio build that does not depend on PipeWire, build the server from source with plain ALSA — see the server guide's [getting started](https://clausters.readthedocs.io/en/latest/getting-started.html) (`cargo build --no-default-features --features synth,realtime`).

### Environment knobs (all optional)

- `CLAUSTERS_WORKSPACE` — path to the cargo workspace, if it can't be found by searching upward.
- `CLAUSTERS_CARGO_FEATURES` — features for the embed library (default `embed,realtime`).
- `CLAUSTERS_SKIP_NATIVE_BUILD` — package the libs already staged in `clausters/_libs/` without rebuilding.
- `CLAUSTERS_FFI_LIB` / `CLAUSTERS_LIB` — at runtime, point a loader directly at a cdylib (overrides the bundled copy and the workspace `target/`).
- `CLAUSTERS_SKIP_GUI_BUILD` — at build time, skip building/bundling the `clausters-gui` binary (a lighter, server-only wheel).
- `CLAUSTERS_GUI_BIN` — at runtime, point the launcher directly at a `clausters-gui` host binary (overrides the bundled copy and the workspace `target/`).

## Your first sound, line by line

The client is meant to be driven **interactively** — from a REPL, or cell by cell in an editor. Nothing below is a script: type a line, hear the result, type the next one. There is no global state to set up and no build context to open, so every line is complete on its own.

Start a session. `Session.live()` attaches to a running server, or starts one for you if none is up:

```python
from clausters import Session

session = Session.live(latency=0.1)   # latency: schedule a hair ahead, so events land on time
server = session.server               # the handle you send commands to
```

An instrument is a **def** — a named graph the server compiles once and instantiates many times. Build one as a UGen graph and send it:

```python
from clausters.defs import SynthDef, control, sin_osc, out

sdef = SynthDef("beep", out(0.0, sin_osc(control("freq", 440.0)) * control("amp", 0.2)))
server.add_synthdef(sdef)             # sends /d_recv and waits for the server's /done
```

Now play it. `synth` starts a node — it sounds until you free it — and `set` changes its controls while it sounds:

```python
node = server.synth("beep", {"freq": 330.0})   # you hear it now
server.set(node, {"freq": 550.0})              # ...and now it is higher
server.free(node)                              # silence
```

The **other def format is a peer, not a fallback**: the same instrument as a `FaustDef`, which the server JIT-compiles. Write it as a signal graph, as a box graph reusing the Faust libraries, or as Faust source — three ways of saying the same thing (a Faust def needs a server built with the `faust` feature; see [Defining instruments](defs.md#what-the-server-must-support)):

```python
from clausters.defs import signals as S, boxes as box, FaustDef

freq = S.hslider("freq", 440.0, 20.0, 20000.0, 0.01)
phase = S.rec(lambda p: (p + freq / S.sr()) % 1.0)          # a one-sample feedback phasor
server.add_faustdef(FaustDef.from_signals("fbeep", S.sin(phase * S.TAU) * 0.2))

# the same voice, point-free, borrowing the oscillator from Faust's library:
server.add_faustdef(FaustDef.from_box(
    "bbeep", box.faust("os.osc")(box.hslider("freq", 440.0, 20.0, 20000.0, 0.01)) * 0.2))

node = server.synth("bbeep", {"freq": 220.0})
server.free(node)
```

Both defs are now installed on the server and play the same way — a `Synth` is a `Synth`, whichever family compiled it.

Instead of starting nodes by hand, hand a **pattern** to the session. It plays on the session's clock, which runs in its own thread, so the REPL stays yours:

```python
from clausters.seq import Pbind, Pseq, Pwhite

session.play(Pbind(instrument="beep",
                   freq=Pseq([330, 440, 550, 660], repeats=8),
                   dur=0.25,
                   amp=Pwhite(0.1, 0.2)))
session.start()      # the clock runs; keep typing while it plays
session.stop()       # ...and stop it whenever
```

When you are done, close the session. Everything it started — the server it booted, a GUI it opened — stops with it:

```python
session.close()
```

That is the whole loop: a session, a def, nodes or patterns. From here, [Sessions](sessions.md) explains the offline and embedded flavours (the same code, a different factory), [Defining instruments](defs.md) is the full def-building vocabulary, and [Routines and clocks](routines-and-clocks.md) drives the clock by hand.

## Play a sound

The examples below are the same ideas as *scripts*, so they can run unattended: they wrap the code in functions and drive the clock for a fixed time. Three paths, the same code shape — they differ only in the session factory. **Embedded** runs the whole server inside the Python process (the bundled library); nothing to start:

```sh
python clients/python/examples/embedded.py
```

**Offline** (no server, no audio device) renders a score to a WAV through the bundled renderer:

```sh
python clients/python/examples/offline_render.py out.wav
```

**Live** sends the same pattern over the network (TCP by default; UDP probes for the server first) to a *separate* server. The wheel ships that server too, as the `clausters` command:

```sh
clausters                                    # start the standalone server (its own process)
python clients/python/examples/live_udp.py   # in another shell
```

See [Sessions](sessions.md) for when to pick which, [Examples](examples.md) for what these do, and [The client, layer by layer](guide.md) for the model behind them.

## Run the tests

```sh
cd clients/python
python -m pytest          # or: python tests/test_smoke.py
```

Boundary rule (project-wide): only flat data crosses any binding — Python floats/ints in, `array('f')`/bytes out.
