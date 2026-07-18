# clausters (web)

Clausters in the browser: the audio server compiled to WebAssembly running inside an **AudioWorklet**, the GUI host on a canvas, **web components** that boot native-format standalone bundles — no server process anywhere — and the first layer of the **TypeScript client**: the OSC codec through the shared native core and the carrier-agnostic connection seam. See `PLAN.md` here for the client roadmap.

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

The examples (`examples/`, served pages): `demo.html` the web-components demo, `standalone.html` the raw-API standalone boot, `engine.html` the audible engine harness, `gui-host.html` the GUI host over WebSocket, and `graph-controls/` — a GraphDef's control surface as one web component, its bundle authored with the Python client (`make_bundle.py`). The bundle format and the underlying pieces are documented in the server book (`docs/clients.md`, `docs/using-as-a-library.md`); the scripted acceptances are the `scripts/smoke-web*.sh` set at the repo root plus `scripts/parity-web.sh`, and `./test.sh` here.
