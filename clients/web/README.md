# clausters (web)

Clausters in the browser: the audio server compiled to WebAssembly running inside an **AudioWorklet**, the GUI host on a canvas, **web components** that boot native-format standalone bundles — no server process anywhere — and the **TypeScript client** that drives all of it: the OSC codec through the shared native core, the carrier-agnostic connection seam, and the def model and `Server` above them. See `PLAN.md` here for the client roadmap.

This is the repo's **one web directory**: every browser artifact (the package modules, the engine's worklet/loader runtime, the examples, the test pages, the tools) lives here, and the wasm crates stay Rust-only — `build.sh` stages their wasm-bindgen bundles into `dist/`. The package **mirrors the Python client's structure**: sources under `src/` at the same relative paths as `clausters/`'s modules (`base/`, `gui/`, …), with `examples/` and `tests/` beside them; `dist/` reproduces the `src/` tree 1:1 (plain ES modules plus `.d.ts` and source maps — no bundler), and the staged wasm bundles inside it (`engine/`, `gui-host/`, `core/`) are the browser's `_bin`/`_libs`. Any static file server serves the result. Toolchain details and the from-scratch recipe are in `BUILD.md`.

```sh
./build.sh                # cargo-builds the three wasm bundles, stages them
                          # into dist/, then emits src/ -> dist/ (needs
                          # `npm install` once)
python3 -m http.server    # then open /examples/demo.html
./test.sh                 # type-check + node suites + the headless-Chrome smoke
```

Use it from a page:

```html
<script type="module">
  import { server, bootBundle } from "./dist/index.js";
  // registers <clausters-bundle> and <clausters-power> as a side effect
  import "./dist/index.js";
</script>

<!-- a standalone bundle as one element; its button is the autoplay gesture -->
<clausters-bundle src="my-bundle" name="drone"></clausters-bundle>
```

The page surface:

- `server()` — the lazy **per-page engine singleton**: `send(bytes)` / `addReply(listener)` raw OSC, `clock()`, `bLoad(...)` (the browser's `/b_allocRead`), `resume()`/`suspend()`. Every component and script on the page gets the same engine, so they meet in one node/bus/buffer namespace.
- `guiHost()` — the per-page GUI-host singleton, wired to the engine over the in-page server leg (`GuiBridge.connect_page`).
- `bootBundle({ base, name })` — boots a served bundle (the native `--standalone` data directory plus the `bundle.json` manifest `tools/bundle-manifest.py` generates; see "A standalone bundle in a tab" in the server book's clients chapter).
- `<clausters-bundle>` / `<clausters-power>` — the elements; the power button is the standard autoplay-policy affordance.

The client seam (what the TypeScript client builds on):

- `loadOsc()` / `encodeMessage(addr, args)` / `decodePacket(bytes)` — the OSC codec through `clausters-core` compiled to wasm (the `dist/core/` bundle), byte-identical to the server and the Python client by construction; `tests/osc-vectors.json` (generated from the Python client's codec) holds the parity. Arguments are tagged pairs — `encodeMessage("/s_new", [["s","sine"], ["i",1000], ["f",440]])` — so the int/float distinction stays explicit.
- `Connection` — one duplex-OSC interface, two carriers: `WsConnection.open(url)` (a browser/node `WebSocket` to a `clausters --ws` server, default port 57120) and `pageConnection()` (the in-page engine, no process, no socket). Everything above the seam never names a transport.

The client proper (`src/defs/`, mirroring the Python client's `clausters/defs/`):

- `Server.open(connection)` — the only object that knows a connection. It sizes its node/bus/buffer allocators from the server's own `/server_info`, registers for the server's pushes (which is what recycles a node id once its `/n_end` arrives), and carries the commands: `addSynthDef`/`addFaustDef`/`addGraphDef`, `sync()`, `synth`/`group`/`graph`/`graphVoice`, `set`/`map`/`free`/`run`, buses, buffers and the introspection queries (`queryInfo`, `queryDefs`, `nodeQuery`, `queryTree`). **Everything that waits is a promise**: where the Python client blocks a thread on a reply, this one `await`s.
- `SynthDef` + the lowercase UGen callables (`sine`, `saw`, `rlpf`, `envGen`, `out`, `pan2`, …), and `FaustDef` + the Faust signal API (`signals`), the two def families as peers. `GraphDef` wires several of either into one named, instantiable configuration with a port surface.
- The graph composes **by method**, TypeScript having no operator overloading: `sine(freq).mul(amp)` where the Python client writes `sine(freq) * amp`. The emitted spec JSON is identical, and `tests/def-parity.test.ts` holds that against vectors frozen from the Python builders (`tests/gen-def-vectors.py`).

```js
import { loadOsc, pageConnection, Server, SynthDef, control, out, sine }
  from "./dist/index.js";

await loadOsc();
const server = await Server.open(await pageConnection());  // or a WsConnection

const freq = control("freq", 440.0);
await server.addSynthDef(new SynthDef("beep", out(0.0, sine(freq).mul(0.2))));
const note = server.synth("beep", { freq: 330.0 });
server.set(note, { freq: 220.0 });
note.free();
```

The examples (`examples/`, served pages): `synth.html` a def built, sent, played and retuned from TypeScript over **either** carrier (the choice is the one line of the page that names one), `demo.html` the web-components demo, `standalone.html` the raw-API standalone boot, `engine.html` the audible engine harness, `gui-host.html` the GUI host over WebSocket, `graph-controls/` — a GraphDef's control surface as one web component, its bundle authored with the Python client (`make_bundle.py`) — and `piano/` — a playable piano keyboard whose keys the GUI host maps to server voices itself (the widget's `voice` mode), the same authored-bundle posture. The bundle format and the underlying pieces are documented in the server book (`docs/clients.md`, `docs/using-as-a-library.md`); the scripted acceptances are the `scripts/smoke-web*.sh` set at the repo root plus `scripts/parity-web.sh`, and `./test.sh` here.
