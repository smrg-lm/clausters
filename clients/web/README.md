# clausters (web)

Clausters in the browser: the audio server compiled to WebAssembly running inside an **AudioWorklet**, the GUI host on a canvas, and **web components** that boot native-format standalone bundles — no server process anywhere. This package seeds the TypeScript client track (see `PLAN.md` here); its raw `server()` handle is the surface that client will build on.

Plain ES modules, no bundler and no node toolchain: stage the wasm bundles once and serve the directory.

```sh
./build.sh                # builds + stages engine/ and gui-host/ (wasm)
python3 -m http.server    # then open /demo.html
```

Use it from a page:

```html
<script type="module">
  import { server, bootBundle } from "./index.js";
  // registers <clausters-bundle> and <clausters-power> as a side effect
  import "./index.js";
</script>

<!-- a standalone bundle as one element; its button is the autoplay gesture -->
<clausters-bundle src="my-bundle" name="drone"></clausters-bundle>
```

- `server()` — the lazy **per-page engine singleton**: `send(bytes)` / `addReply(listener)` raw OSC, `clock()`, `bLoad(...)` (the browser's `/b_allocRead`), `resume()`/`suspend()`. Every component and script on the page gets the same engine, so they meet in one node/bus/buffer namespace.
- `guiHost()` — the per-page GUI-host singleton, wired to the engine over the in-page server leg (`GuiBridge.connect_page`).
- `bootBundle({ base, name })` — boots a served bundle (the native `--standalone` data directory plus the generated `bundle.json` manifest; see "A standalone bundle in a tab" in the server book's clients chapter).
- `<clausters-bundle>` / `<clausters-power>` — the elements; the power button is the standard autoplay-policy affordance.

The bundle format, the manifest generator and the underlying pieces are documented in the server book (`docs/clients.md`, `docs/using-as-a-library.md`); the scripted acceptance is `scripts/smoke-web-components.sh` at the repo root.
