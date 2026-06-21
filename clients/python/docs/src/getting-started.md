# Getting started

## Install

The package is pure Python at runtime, but it reaches Rust through two cdylibs that **cargo** builds: `libclausters_ffi` (the numeric/timing core) and `libclausters` (built with `embed,realtime` for the in-process server and offline render). The packaging **bundles** them inside the wheel, so an installed package is self-contained — no `target/` directory and no build step at import time.

The simplest setup, in a fresh virtualenv, run from the repository so the build hook can find the cargo workspace:

```sh
python -m venv .venv && . .venv/bin/activate
pip install -e ./clients/python --group dev      # editable + the pytest dev group
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

### Environment knobs (all optional)

- `CLAUSTERS_WORKSPACE` — path to the cargo workspace, if it can't be found by searching upward.
- `CLAUSTERS_CARGO_FEATURES` — features for the embed library (default `embed,realtime`).
- `CLAUSTERS_SKIP_NATIVE_BUILD` — package the libs already staged in `clausters/_libs/` without rebuilding.
- `CLAUSTERS_FFI_LIB` / `CLAUSTERS_LIB` — at runtime, point a loader directly at a cdylib (overrides the bundled copy and the workspace `target/`).

## Play a sound

Two paths, the same code shape. **Offline** (no server, no audio device) renders a score to a WAV through the bundled embed renderer:

```sh
python clients/python/examples/offline_render.py out.wav
```

**Live** sends the same pattern over UDP to a running server (start one with `cargo run --release`, or the installed `clausters` binary):

```sh
python clients/python/examples/live_udp.py
```

See [Examples](examples.md) for what these do, and [The client, layer by layer](guide.md) for the model behind them.

## Run the tests

```sh
cd clients/python
python -m pytest          # or: python tests/test_smoke.py
```

Boundary rule (project-wide): only flat data crosses any binding — Python floats/ints in, `array('f')`/bytes out.
