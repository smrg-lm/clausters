# Getting started

By the end of this page a tab is making a sound with no server process anywhere, from a page you wrote.

## Get the package

```sh
npm install clausters
```

That is the whole install: the tarball carries the emitted modules, their type declarations and the three wasm bundles (the engine, the GUI host, the shared core), so nothing is compiled and nothing is fetched at run time.

The pages below are served straight off the file system with no build step, so they import `./dist/index.js` by path. Through a bundler the same imports are written `from "clausters"`; nothing else in them changes.

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

Open <http://localhost:8000/examples/synth.html> and press *connect + send the def*, then *play a note*. That page is the shortest end-to-end path through the client, and the rest of this section is the same thing written from scratch.

## A page that plays a note

Save this beside `dist/` and open it:

```html
<!doctype html>
<button id="go">play</button>
<script type="module">
  import {
    loadOsc, pageConnection, server as engine,
    Server, Synth, SynthDef, Env, DoneAction, control, envGen, out, saw,
  } from "./dist/index.js";

  // The def is a value: no connection in sight, and nothing has started.
  const freq = control("freq", 220.0);
  const gate = control("gate", 1.0);
  const voice = saw(freq)
    .mul(0.2)
    .mul(envGen(Env.adsr(), { gate, doneAction: DoneAction.FREE_SELF }));
  const def = new SynthDef("hello", out(0.0, voice), out(1.0, voice));

  document.getElementById("go").onclick = async () => {
    await loadOsc();                       // the core's wasm: the OSC codec
    await (await engine({ channels: 2 })).resume();
    const server = await Server.open(await pageConnection());
    await def.send(server);         // resolves when the server acked it

    const note = Synth.new(server, "hello", { freq: 330.0 });
    setTimeout(() => note.set({ gate: 0.0 }), 1000);
  };
</script>
```

Three things in there are the whole client:

- **The graph composes by method** — `saw(freq).mul(0.2)`, where the Python client writes `saw(freq) * 0.2`. TypeScript has no operator overloading; the JSON both send is identical.
- **Everything that waits is a promise.** `def.send(server)` resolves when the server has acknowledged the def, so the `/s_new` that follows cannot race it. The page has one thread and must keep running: nothing ever blocks.
- **The click is not decoration.** A browser starts no audio without a gesture, so the first thing that touches the engine has to happen inside an event handler.

## The other carrier

The same page drives a native server if you hand `Server.open` the other connection:

```js
import { WsConnection } from "./dist/index.js";
const server = await Server.open(await WsConnection.open("ws://127.0.0.1:57120"));
```

with the server started as

```sh
cargo run --release -- --ws          # from the repository root; 57120 by default
```

Nothing else in the page changes — that one line is the only place a carrier is named. A `--ws` server also compiles Faust, which the in-page engine cannot: it is a build without the Faust compiler's LLVM.

## Where to go next

- [The client, layer by layer](guide.md) — the seam, the def model, the GUI driver and the clock.
- [Routines and clocks](routines-and-clocks.md) — the note above, in time: a melody played from a generator on a `TempoClock`.
- [Components](components.md) — handing a finished instrument to a page as a custom element.
- [Examples](examples.md) — the runnable pages, including the one above.
