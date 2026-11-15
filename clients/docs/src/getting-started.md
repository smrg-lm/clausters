# Getting started

<!-- python -->
## Install

The package is pure Python at runtime, but it reaches Rust through artifacts that **cargo** builds: three cdylibs — `libclausters_ffi` (the numeric/timing core), `libclausters` (built with `embed,realtime` for the in-process server and offline render) and `libclausters_midi` (built with `live`, the MIDI file writers and the virtual MIDI ports) — and the standalone server **binary**. The packaging **bundles** all of them inside one wheel, so an installed package is self-contained: the in-process embedded server and the offline renderer work out of the box, and the standalone server is on your `PATH` as the `clausters` command. No `target/` directory and no build step at import time.

The wheel also bundles **Faust** — `libfaust`, with the LLVM it JIT-compiles with linked into it — so a `FaustDef` compiles on a machine without it installed, exactly like a `SynthDef`. Both def families are on by default in the server, and the client treats them as peers; the price is a heavy wheel (~47 MB packed, the largest part of it LLVM, since the Faust compiler *is* an LLVM JIT). It bundles **libverovio** the same way, so the `score` widget engraves with nothing installed. Nothing to enable and nothing to install: this is the default install.

The simplest setup, in a fresh virtualenv, run from the repository so the build hook can find the cargo workspace:

```sh
python -m venv .venv && . .venv/bin/activate
pip install -e ./clients/python --group ./clients/python/pyproject.toml:dev
# (editable + the pytest dev group; pip's --group reads ./pyproject.toml unless
# given a path, and this repo's lives in clients/python/)
# or a plain install:
pip install ./clients/python
```

Building from the repository needs the system dependencies, Rust, and libfaust and libverovio built first; the package's README walks through all of it, step by step, including what to run after changing code ([From the repository, step by step](https://github.com/smrg-lm/clausters/tree/main/clients/python#from-the-repository-step-by-step)). The short version of the last part: Python changes need nothing, since the install is editable, but after rebuilding a crate run `scripts/refresh-bin.sh`, because the package loads the binaries staged inside it before the workspace `target/`.

`pip install` triggers `setup.py`, which runs `cargo build` for the three cdylibs and stages them in `clausters/_libs/` before packaging. To build a redistributable wheel explicitly:

```sh
pip install build                                # once: the PEP 517 front end
rm -rf clients/python/dist clausters-editor/wheel/dist   # dist/ keeps earlier builds, which the globs below would match
python -m build --wheel clients/python           # -> clients/python/dist/*.whl
pip install clients/python/dist/clausters-*.whl  # self-contained, no cargo
python -m build --wheel clausters-editor/wheel   # the editor's (the editor extra)
pip install clients/python/dist/clausters-*.whl clausters-editor/wheel/dist/*.whl
```

In a plain source checkout (no install), the loaders fall back to the workspace `target/{release,debug}/`, so the historic build-and-run workflow still works:

```sh
cargo build -p clausters-ffi
cargo build --features embed,realtime
```

### The visual server (GUI)

The GUI host — the visual server that opens windows and renders widgets — is **bundled in the same wheel** as the audio server, built from the `crates/clausters-gui` crate and stripped. Nothing extra to install: with the package in place, [`Session.live` / `Session.gui`](sessions.md#launching-the-server-and-the-gui) launch the audio and visual servers for you, and `clausters-gui` is also on your `PATH`.

Building it adds a wgpu compile (a minute or two the first time). For a lighter, server-only wheel, set `CLAUSTERS_SKIP_GUI_BUILD` when installing; a source-checkout binary built under `target` (`cargo build --release -p clausters-gui`) is still used at runtime if present, and `CLAUSTERS_GUI_BIN` overrides the lookup.

### The editor

`pip install clausters[editor]` also installs **clausters-editor**, a Python editor with an interactive session, as a separate package. Its `clausters-editor` command opens a session on the environment it was installed in, so `import clausters` works there from the first line, and its documentation viewer carries this book: the wheel holds it as Markdown, the API reference included, so the editor's search and its help on a name (`Ctrl+D`) reach it offline. On Linux it needs the system's WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`, in every desktop distribution since 2022), which no wheel carries; the command says how to install it when it is missing.

From a checkout, both paths go in one command, and pip takes the second as the `clausters-editor` the extra names, as it would take PyPI's:

```sh
pip install -e "./clients/python[editor]" ./clausters-editor/wheel
```

### Runtime requirement: PipeWire (Linux)

The bundled artifacts that touch an audio device — the in-process embedded server and the standalone `clausters` binary — are built with the project's default features, which include the PipeWire audio backend. They therefore **hard-link `libpipewire`** and expect PipeWire present at runtime (the standard on current Linux systems); on a host without it the library and the binary will not load. The offline renderer and the numeric core need no audio device, so `Session.nrt()` and `clausters._native` work anywhere. For an audio build that does not depend on PipeWire, build the server from source with plain ALSA — see the server guide's [getting started](https://clausters.readthedocs.io/en/latest/getting-started.html) (`cargo build --no-default-features --features synth,realtime`).

### Environment knobs (all optional)

- `CLAUSTERS_WORKSPACE` — path to the cargo workspace, if it can't be found by searching upward.
- `CLAUSTERS_CARGO_FEATURES` — features for the embed library (default `embed,realtime`).
- `CLAUSTERS_SKIP_NATIVE_BUILD` — package the libs already staged in `clausters/_libs/` without rebuilding.
- `CLAUSTERS_FFI_LIB` / `CLAUSTERS_LIB` — at runtime, point a loader directly at a cdylib (overrides the bundled copy and the workspace `target/`).
- `CLAUSTERS_SKIP_GUI_BUILD` — at build time, skip building/bundling the `clausters-gui` binary (a lighter, server-only wheel).
- `CLAUSTERS_SKIP_DOCS` — at build time, leave out the offline book, whose API pages need `pydoc-markdown`.
- `CLAUSTERS_GUI_BIN` — at runtime, point the launcher directly at a `clausters-gui` host binary (overrides the bundled copy and the workspace `target/`).
<!-- /python -->
<!-- web -->
By the end of this page a tab is making a sound with no server process anywhere.

## Get the package

```sh
npm install clausters
```

That is the whole install: the tarball carries the emitted modules, their type declarations and the three wasm bundles (the engine, the GUI host, the shared core), so nothing is compiled and nothing is fetched at run time.

Through a bundler the imports are written `from "clausters"`, and a layer is `from "clausters/defs"` (likewise `seq`, `gui`, `data`). A page served straight off the file system with no build step imports the same modules by path — `./dist/index.js`, `./dist/defs/index.js` — and nothing else in it changes.

The package also carries what [clausters-editor](https://github.com/smrg-lm/clausters/tree/main/clausters-editor), the editor with an interactive session (Python on the desktop, JavaScript as a page), reads from it: this book and the server's, as Markdown, which its documentation viewer shows offline (the API reference included, so its search and its help on a name, `Ctrl+D`, reach them), and the hook its interrupt key calls, `Ctrl+.`, which panics every session not closed yet, as the Python client's does. Neither needs the editor, and neither loads unless the editor asks.

### From a checkout instead

Building it yourself is only needed to work *on* the client. Two tools once, both user-space (the full recipes are in `clients/web/BUILD.md`): **node** (an LTS under `~/.local`) and the Rust wasm toolchain — `rustup target add wasm32-unknown-unknown` plus `wasm-bindgen-cli` at the version the lockfiles pin.

```sh
cd clients/web
npm install       # once: TypeScript into node_modules/ (no other dependency)
./build.sh        # builds the three wasm bundles, stages them into dist/,
                  # then emits src/ -> dist/
```

`dist/` is now the whole package: plain ES modules with their type declarations, and the wasm bundles beside them (`dist/engine/`, `dist/gui-host/`, `dist/core/`) — the same tree `npm install` unpacks. To use *that* build from another project on this machine, install the directory: `npm install /path/to/clausters/clients/web`.

## Serve it

The engine runs in an AudioWorklet and the GUI host draws with WebGPU, and both need a **secure context**: `http://localhost` counts, a plain-HTTP LAN address does not. Serving the checkout is enough:

```sh
python3 -m http.server        # from clients/web/
```

Open <http://localhost:8000/examples/basics/synth.html> and press *connect + send the def*, then *play a note*. That page is the shortest end-to-end path through the client.
<!-- /web -->

## Your first sound, line by line

The client is meant to be driven **interactively** — {{alt from a REPL, or cell by cell in an editor || in clausters-editor's page, region by region, or from a page's own script}}. Nothing below is a script: type a line, hear the result, type the next one. Every line is complete on its own.<!-- web --> A browser starts no audio without a gesture, so the first line that touches the engine runs after a key or a click in the page (the editor's page waits for one), and a page of your own calls it from an event handler.<!-- /web -->

The shortest path to sound is two lines — boot a server and play something:

```python
from clausters import Server, Event, play
from clausters.defs import sine

Server().boot()        # launches a server and becomes the default session
play(Event(degree=0))  # one note, now — or: Event(degree=0).play()

node = play(sine(440.0) * 0.2)   # a bare signal, sounding now...
node.free()                         # ...until you free it
```

```ts
import { Event, Server, play } from "clausters";
import { sine } from "clausters/defs";

await new Server().boot();      // brings up an engine and becomes the default session
play(new Event({ degree: 0 })); // one note, now — or: new Event({ degree: 0 }).play()

const node = play(sine(440.0).mul(0.2));   // a bare signal, sounding now...
node.free();                               // ...until you free it
```

{{alt `Server().boot()` || `new Server().boot()`}} adopts the server it starts as the **default session**, so a bare {{alt `Event().play()` || `new Event().play()`}} (or the free-standing `play`, which plays anything — an event, a `pbind`, a routine, a bare signal expression like the `sine` above; see [The ambient verbs](verbs.md)) finds it with no wiring. A note played outside a clock sounds immediately and frees itself after its sustain. That is all you need for a quick sound; a `Session` (below) is for when you want to name and isolate an environment — several at once, offline next to live.

Start a session. {{alt `Session.live()` attaches to a running server, or starts one for you if none is up || `Session.embed()` boots an engine of the session's own in this tab; `Session.live(url)` attaches to a native server instead}}:

```python
from clausters import Session

session = Session.live(latency=0.1)   # latency: schedule a hair ahead, so events land on time
server = session.server               # the handle you send commands to
```

```ts
import { Session } from "clausters";

const session = await Session.embed({ latency: 0.1 });   // latency: schedule a hair ahead, so events land on time
const server = session.server;                           // the handle you send commands to
```

An instrument is a **def** — a named graph the server compiles once and instantiates many times. Build one as a UGen graph and send it:

```python
from clausters.defs import SynthDef, Synth, control, sine, out

sdef = SynthDef("beep", out(0.0, sine(control("freq", 440.0)) * control("amp", 0.2)))
sdef.send(server)             # sends /def_send synth and waits for the server's /done
```

```ts
import { Synth, SynthDef, control, out, sine } from "clausters/defs";

const sdef = new SynthDef("beep", out(0.0, sine(control("freq", 440.0)).mul(control("amp", 0.2))));
await sdef.send(server);      // sends /def_send synth and resolves on the server's /done
```

Now play it. `Synth` starts a node — it sounds until you free it — and `set` changes its controls while it sounds:

```python
node = Synth("beep", {"freq": 330.0}, server=server)   # you hear it now
node.set({"freq": 550.0})              # ...and now it is higher
node.free()                              # silence
```

```ts
const node = new Synth("beep", { freq: 330.0 }, { server });   // you hear it now
node.set({ freq: 550.0 });             // ...and now it is higher
node.free();                           // silence
```

The **other def format is a peer, not a fallback**: the same instrument as a `FaustDef`, which the server JIT-compiles. Write it as a signal graph, as a box graph reusing the Faust libraries, or as Faust source — three ways of saying the same thing. Nothing to install: the package bundles Faust (see [What the server must support](defs.md#what-the-server-must-support)).

```python
from clausters.defs import signals as S, boxes as box, FaustDef

freq = S.hslider("freq", 440.0, 20.0, 20000.0, 0.01)
phase = S.rec(lambda p: (p + freq / S.sr()) % 1.0)          # a one-sample feedback phasor
FaustDef.from_signals("fbeep", S.sin(phase * S.TAU) * 0.2).send(server)

# the same voice, point-free, borrowing the oscillator from Faust's library:
FaustDef.from_box(
    "bbeep", box.faust("os.osc")(box.hslider("freq", 440.0, 20.0, 20000.0, 0.01)) * 0.2).send(server)

node = Synth("bbeep", {"freq": 220.0}, server=server)
node.free()
```

```ts
import { FaustDef, Synth, boxes as box, signals as S } from "clausters/defs";

const freq = S.hslider("freq", 440.0, 20.0, 20000.0, 0.01);
const phase = S.rec((p) => p.add(freq.div(S.sr())).mod(1.0));   // a one-sample feedback phasor
await FaustDef.fromSignals("fbeep", S.sin(phase.mul(S.TAU)).mul(0.2)).send(server);

// the same voice, point-free, borrowing the oscillator from Faust's library:
await FaustDef.fromBox(
    "bbeep", box.faust("os.osc").call(box.hslider("freq", 440.0, 20.0, 20000.0, 0.01)).mul(0.2)).send(server);

const node = new Synth("bbeep", { freq: 220.0 }, { server });
node.free();
```

Both defs are now installed on the server and play the same way — a `Synth` is a `Synth`, whichever family compiled it.

Instead of starting nodes by hand, hand a **pattern** to the session. It plays on the session's clock, which runs {{alt in its own thread, so the REPL stays yours || beside the page's own work, so the page stays yours}}:

```python
from clausters.seq import pbind, pseq, pwhite

session.play(pbind(instrument="beep",
                   freq=pseq([330, 440, 550, 660], repeats=8),
                   dur=0.25,
                   amp=pwhite(0.1, 0.2)))
session.start()      # the clock runs; keep typing while it plays
session.stop()       # ...and stop it whenever
```

```ts
import { pbind, pseq, pwhite } from "clausters/seq";

session.play(pbind({ instrument: "beep",
                     freq: pseq([330, 440, 550, 660], 8),
                     dur: 0.25,
                     amp: pwhite(0.1, 0.2) }));
session.start();     // the clock runs; keep typing while it plays
session.stop();      // ...and stop it whenever
```

<!-- python -->
`pbind` (like `Event`) constructs exactly as a `dict` does: the keyword form
above is a shorthand, and a plain mapping —
`pbind({"instrument": "beep", "dur": 0.25})` — works the same, matching how
`Synth` takes its controls.
<!-- /python -->

When you are done, close the session. Everything it started — the server it booted, a GUI it opened — stops with it:

```python
session.close()
```

```ts
session.close();
```

That is the whole loop: a session, a def, nodes or patterns. From here, [Sessions](sessions.md) explains the offline and {{alt embedded || live}} flavours (the same code, a different factory), [Defining instruments](defs.md) is the full def-building vocabulary, and [Routines and clocks](routines-and-clocks.md) drives the clock by hand.

<!-- web -->
## A page that plays a note

The same loop as a page of your own. Save this beside `dist/` and open it:

```html
<!doctype html>
<button id="go">play</button>
<script type="module">
  import { Session, Synth, SynthDef } from "./dist/index.js";
  import { Env, DoneAction, control, envGen, out, saw } from "./dist/defs/index.js";

  // The def is a value: no connection in sight, and nothing has started.
  const freq = control("freq", 220.0);
  const gate = control("gate", 1.0);
  const voice = saw(freq)
    .mul(0.2)
    .mul(envGen(Env.adsr(), { gate, doneAction: DoneAction.FREE_SELF }));
  const def = new SynthDef("hello", out(0.0, voice), out(1.0, voice));

  document.getElementById("go").onclick = async () => {
    // The click is not decoration: a browser starts no audio without one.
    const session = (await Session.embed()).activate();
    await def.send();               // resolves when the server acked it

    const note = new Synth("hello", { freq: 330.0 });
    setTimeout(() => note.set({ gate: 0.0 }), 1000);
  };
</script>
```

The same page drives a native server through the other factory, `(await Session.live("ws://127.0.0.1:57120")).activate()`, with the server started as `clausters --ws` (57120 by default). The server binds **loopback**, which is all a page served from this machine needs; a page on another machine reaches it only if the server was started with the address said out loud (`--ws 0.0.0.0:57120`). Nothing else in the page changes — that one line is the only place a carrier is named.
<!-- /web -->

## Play a sound

The examples are the same ideas as {{alt *scripts*, so they can run unattended: they wrap the code in functions and drive the clock for a fixed time || *pages*, each with the controls to drive it}}. Three paths, the same code shape — they differ only in the session factory. **Embedded** runs the whole server inside the {{alt Python process (the bundled library) || tab}}; nothing to start:

<!-- python -->
```sh
python clients/python/examples/io/embedded.py
```
<!-- /python -->

<!-- web -->
```text
http://localhost:8000/examples/basics/synth.html
```
<!-- /web -->

**Offline** (no server, no audio device) renders a score to a WAV through the bundled renderer:

<!-- python -->
```sh
python clients/python/examples/buffers/offline_render.py out.wav
```
<!-- /python -->

<!-- web -->
```text
http://localhost:8000/examples/buffers/render-then-load.html
```
<!-- /web -->

**Live** sends the same pattern over the network to a *separate* server{{alt  (TCP by default; UDP probes for the server first). The wheel ships that server too, as the `clausters` command || , reached over its WebSocket}}:

<!-- python -->
```sh
clausters                                    # start the standalone server (its own process)
python clients/python/examples/io/live_udp.py   # in another shell
```
<!-- /python -->

<!-- web -->
```text
clausters --ws            # then pick the WebSocket carrier on the page
```
<!-- /web -->

See [Sessions](sessions.md) for when to pick which, [Examples](examples.md) for what these do, and [The client, layer by layer](guide.md) for the architecture behind them.

<!-- python -->
## Run the tests

```sh
cd clients/python
python -m pytest          # or: python tests/test_smoke.py
```

Boundary rule (project-wide): only flat data crosses any binding — Python floats/ints in, `array('f')`/bytes out.
<!-- /python -->
