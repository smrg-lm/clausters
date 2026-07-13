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

## Play a sound

Three paths, the same code shape — they differ only in the session factory. **Embedded** runs the whole server inside the Python process (the bundled library); nothing to start:

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
