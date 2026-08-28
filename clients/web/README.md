# clausters (web)

[![npm](https://img.shields.io/npm/v/clausters)](https://www.npmjs.com/package/clausters)
[![Web client book](https://readthedocs.org/projects/clausters-web/badge/?version=latest)](https://clausters-web.readthedocs.io/)
[![Server book](https://readthedocs.org/projects/clausters/badge/?version=latest)](https://clausters.readthedocs.io/)

```sh
npm install clausters
```

Clausters in the browser: the audio server compiled to WebAssembly running inside an **AudioWorklet**, the GUI host on a canvas, **web components** that boot native-format standalone bundles — no server process anywhere — and the **TypeScript client** that drives all of it: the OSC codec through the shared native core, the carrier-agnostic connection seam, the def model and `Server` above them, and the sequencing layer that plays on it. See `PLAN.md` here for the client roadmap.

This is the repo's **one web directory**: every browser artifact (the package modules, the engine's worklet/loader runtime, the examples, the test pages, the tools) lives here, and the wasm crates stay Rust-only — `build.sh` stages their wasm-bindgen bundles into `dist/`. The package **mirrors the Python client's structure**: sources under `src/` at the same relative paths as `clausters/`'s modules (`base/`, `gui/`, …), with `examples/` and `tests/` beside them; `dist/` reproduces the `src/` tree 1:1 (plain ES modules plus `.d.ts` and source maps — what a page imports is what was written), and the staged wasm bundles inside it (`engine/`, `gui-host/`, `core/`) are the browser's `_bin`/`_libs`. Nothing is bundled. Any static file server serves the result. Toolchain details and the from-scratch recipe are in `BUILD.md`.

📖 **Documentation:** **[clausters-web.readthedocs.io](https://clausters-web.readthedocs.io/)** — the client's own book, the mdBook in `docs/` (`./docs/build.sh` builds it locally, with the API reference generated from the sources' TSDoc by TypeDoc). It is the third of the repository's three, beside the [server book](https://clausters.readthedocs.io/) (the OSC protocol, the `/gui_*` reference, the bundle format) and the [Python client book](https://clausters-python.readthedocs.io/) (the shared client model).

```sh
./build.sh                # cargo-builds the three wasm bundles, stages them
                          # into dist/, then emits src/ -> dist/ (needs
                          # `npm install` once)
python3 -m http.server    # then open /examples/components/demo.html
./test.sh                 # type-check + node suites + the headless-Chrome smoke
./tools/profile-bus-stream.sh [canvases] [seconds]
                          # a profile, not a test: what a frame of /bus_stream
                          # costs a page of N canvases (default 40)
```

## An instrument in the page

A **bundle** is an instrument written to a directory (defs, one GuiDef, presets,
samples) plus a generated ES module that registers its tag. Importing that module
is the whole integration — **no client library is loaded at run time**:

```html
<script type="module">
  import "./fm-voice/index.js";   // registers <fm-voice>
</script>

<style>fm-voice { display: block; width: 100%; height: 340px; }</style>

<p>Prose, and then the instrument in the flow of the page:</p>
<fm-voice></fm-voice>
<fm-voice freq="110" preset="bright"></fm-voice>
```

Each element owns its canvas, and **the document places it** — CSS, the order of
the markup. Two instances of one bundle hold their own node ids and buses, and
the def they share is sent once. Declared parameters are attributes, resolved
attribute → preset → default. The first gesture anywhere on the page starts the
audio for all of them; a component scrolled out of the viewport stops drawing
and stops streaming. Write one with `Bundle` — `clausters/bundle-writer` here, `clausters.bundle`
in the Python client, one writer in two languages that emit the same directory
byte for byte. `write` is a node verb and `examples/panels/piano/make_bundle.mjs`
is the worked example; a page authors one with `files()` and mounts it from
memory (`examples/components/authored.html`). See the server book's clients
chapter for the format.

`<clausters-bundle src="./fm-voice">` mounts a bundle with no generated module.

## Driving it from script

```html
<script type="module">
  import { server, guiHost } from "./dist/index.js";
  // registers <clausters-bundle> and <clausters-power> as a side effect
</script>
```

The page surface:

- `server()` — the lazy **per-page engine singleton**: `send(bytes)` / `addReply(listener)` raw OSC, `clock()`, `bLoad(...)` (the browser's `/buffer_allocRead`), `resume()`/`suspend()`. Every component and script on the page gets the same engine, so they meet in one node/bus/buffer namespace.
- `guiHost()` — the per-page GUI-host singleton, wired to the engine over the in-page server leg (`GuiBridge.connect_page`). It draws **one canvas per `window`-rooted def**: `attach(defId, canvas)` gives a def its surface, and the host is told its size and its visibility, never the DOM.
- `openBundle(...)` / `startBundle(...)` — the two phases of a mount by hand (allocate + resolve + draw, then the engine half on a gesture); `bootBundle(...)` does both for a script that already has one.
- `<clausters-bundle>` / `<clausters-power>` — the elements; the power button is the standard autoplay-policy affordance.

**Two entry points.** `dist/runtime.js` is what a page that mounts components
loads: the engine, the host, the OSC codec and the mount. `dist/index.js` adds
the TypeScript client — the def builders, the GuiDef builders, the sequencing
layer — for a page that sequences, responds or edits live. Both target the same
element, and `tests/runtime-graph.test.ts` holds the line between them.

The client seam (what the TypeScript client builds on):

- `loadCore()` / `encodeMessage(addr, args)` / `decodePacket(bytes)` — the OSC codec through `clausters-core` compiled to wasm (the `dist/core/` bundle), byte-identical to the server and the Python client by construction; `tests/osc-vectors.json` (generated from the Python client's codec) holds the parity. Arguments are tagged pairs — `encodeMessage("/synth_new", [["s","sine"], ["i",1000], ["f",440]])` — so the int/float distinction stays explicit.
- `Connection` — one duplex-OSC interface, two carriers: `WsConnection.open(url)` (a browser/node `WebSocket` to a `clausters --ws` server, default port 57120) and `pageConnection()` (the in-page engine, no process, no socket). Everything above the seam never names a transport.

The client proper (`src/defs/`, mirroring the Python client's `clausters/defs/`):

- `new Server(options).boot()` / `.attach()` — the only object that knows a connection, and it opens its own: `boot` brings up the server this handle owns (the engine in this tab, or the one an `engine` names), `attach` reaches one already running (`{ transport: "ws", url }`). It sizes its node/bus/buffer allocators from the server's own `/server_query`, registers for the server's pushes (which is what recycles a node id once its `/node_end` arrives), and carries what is the server's own: the transport (`sendMsg`, `sendBundle`, `request`, `sync()`), the id pools, `freeDef`, the bus and tap subscriptions and the introspection queries about what it holds (`queryInfo`, `queryDefs`, `queryBuffers`, `queryUgens`, `queryTree`). A command addressed to a resource belongs to that resource, a question about one included: `def.send(server)`, `Synth.new(server, …)`, `Group.graph(server, …)`, `node.set`/`map`/`free`/`run`/`info`, `Bus.audio(server)` and `bus.set`, `Buffer.alloc(server, …)`, `buffer.info` and `buffer.getSamples`. **Everything that waits is a promise**: where the Python client blocks a thread on a reply, this one `await`s. How long it waits for one is the handle's — `server.timeout` (5 s), which every `timeout` argument falls back to when it is left out.
- `SynthDef` + the lowercase UGen callables (`sine`, `saw`, `rlpf`, `envGen`, `out`, `pan2`, …), and `FaustDef` + the Faust signal API (`signals`), the two def families as peers. `GraphDef` wires several of either into one named, instantiable configuration with a port surface.
- The graph composes **by method**, TypeScript having no operator overloading: `sine(freq).mul(amp)` where the Python client writes `sine(freq) * amp`. The emitted spec JSON is identical, and `tests/def-parity.test.ts` holds that against vectors frozen from the Python builders (`tests/gen-def-vectors.py`).

The GUI client (`src/gui/`, mirroring the Python client's `clausters/gui/`):

- `GuiHost` — the object that drives a GUI host, on the same connection seam: `GuiHost.page()` for the wasm host on this page's canvas (through the `guiHost()` singleton's binding bridge) and `GuiHost.connect(url)` for a native `clausters-gui --ws` host. It carries `open`/`define` (a whole tree in one `/gui_def`, with the ids assigned into it in place), `set`, `free`, `bind`/`unbind`, `query` and `load`.
- The GuiDef builders in the `gui` namespace (`gui.window`, `gui.knob`, `gui.waveform`, `gui.track`, …) — the whole widget catalogue, emitting the same JSON document the Python builders do (`tests/gui-parity.test.ts` holds that against vectors frozen from them). The options are camelCase where the wire's props are snake_case (`textSize` → `text_size`).
- Widgets are addressed by **name**, not by integer: `win.widget("cutoff").set({ value: 800.0 })`, `.onEvent(fn)`, `.bind("/node_set", node.id, "freq")`. A **bound** widget's value goes from the host straight to the audio server, with no round trip through the page's script. **Nothing pumps** — events arrive as callbacks, `query` resolves a promise.

```js
import { Server, Synth, SynthDef, control, out, sine }
  from "./dist/index.js";

// The engine in this tab; `new Server({ transport: "ws", url }).attach()`
// reaches a `clausters --ws` server instead.
const server = await new Server().boot();

const freq = control("freq", 440.0);
await new SynthDef("beep", out(0.0, sine(freq).mul(0.2))).send(server);
const note = Synth.new(server, "beep", { freq: 330.0 });
note.set({ freq: 220.0 });
note.free();
```

The sequencing layer (`src/base/clock.ts` + `src/seq/`, mirroring the Python client's `clausters/base` and `clausters/seq`):

- `TempoClock` — musical time and the driver that resumes routines on it. The queue and every conversion (beats to seconds, seconds to samples, the bar grid, the timetag) are `clausters-core`'s, reached through the wasm door, so a beat resolves to the same instant here, in the Python client and in the server. The **logical beat advances only by the routines' yields**, so a late wake-up never shifts the music.
- A **routine** is a generator: `function* () { … yield 0.25 … }` yields a delay in beats. Never `await` inside one — the page has a single thread, and the exactness lives in the timetag, not in the wake-up.
- The **wake-up** sits behind a `Ticker`: a shared worker in the browser (the page's own timers are throttled to about a second in a background tab), `setTimeout` elsewhere. Tests fill the same seam by hand, along with the timebase, and so drive the real driver deterministically.
- `server.sampleTimebase()` — the **Server** anchors the sample clock, because the Server is what knows the carrier: in-page it pairs the engine's counter with the AudioContext's in one round trip (the same clock, so the offset is exact and there is no drift); over a socket it feeds `/clock_query` anchors into the core's sample-clock model. Hand the result to a clock; the clock never talks to a server.
- Emission is the **bundle path**: `server.sendBundle(...)` stamps at the running routine's exact logical beat — a wall-clock timetag under the monotonic timebase, `/sched_at` at an absolute sample under a sample one — and `Event.play(server)` is a note's `/synth_new` plus the release that follows it.
- `seq` — `Event`/`rest`, the value patterns (`Pseq`, `Pser`, `Prand`, `Pwhite`, `Pseries`, `Pgeom`, `Pfunc`, `Pn`, `Pconst`) and `Pbind`, plus the seekable counterpart: `Timeline` (a static, editable, beat-sorted list, `Timeline.fromPattern` to bounce one) and `Playhead` (play/stop/locate/loop). Random values come from the context stream a routine derives at creation, so `seed(n)` replays a whole piece.

```js
import { Server, TempoClock, seq } from "./dist/index.js";

const clock = new TempoClock(2.0, { timebase: await server.sampleTimebase() });
clock.start();

new seq.Pbind({
  instrument: "beep",
  degree: new seq.Pseq([0, 2, 4, 7], seq.INF),
  dur: new seq.Pseq([0.5, 0.25, 0.25]),
}).play(server, { clock });
```

```js
import { GuiHost, gui } from "./dist/index.js";

const host = await GuiHost.page();          // or GuiHost.connect(wsUrl)
const win = host.open(gui.window(
  { title: "a tone", w: 480, h: 240, layout: "col" },
  gui.knob({ name: "freq", label: "freq", min: 50.0, max: 2000.0, value: 220.0 }),
  gui.slider({ name: "amp", label: "amp", min: 0.0, max: 1.0, value: 0.2 }),
));
win.widget("freq").bind("/node_set", note.id, "freq");   // host -> engine, no script
win.widget("amp").onEvent((value) => note.set({ amp: value }));
```

The examples (`examples/`, served pages): `synth.html` a def built, sent, played and retuned from TypeScript over **either** carrier (the choice is the one line of the page that names one), `demo.html` the web-components demo, `standalone.html` the raw-API standalone boot, `engine.html` the audible engine harness, `gui-host.html` a GUI built and driven from TypeScript — the bound and the scripted control paths side by side, a metered bus, the linked waveform + spectrogram, and one button that swaps the in-page host for a native `--ws` one — five ports of Python client examples, each named after the one it mirrors (`multichannel.html`, `typed-controls.html`, `graph-maths.html`, `wavetables.html`, `pause-resume.html`), `graph-controls/` — a GraphDef's control surface as one component, its bundle authored by the node script beside the page (`make_bundle.mjs`, which writes into the git-ignored `examples/out/`) — `piano/` — a playable piano keyboard whose keys the GUI host maps to server voices itself (the widget's `voice` mode), the same authored-bundle posture — and `document/`, an interactive text with both of them interleaved with the prose, which is the shape the whole component format is for. The bundle format and the underlying pieces are documented in the server book (`docs/clients.md`, `docs/using-as-a-library.md`); the scripted acceptances are `scripts/smoke-web.sh` at the repo root (one runner over every page that beacons a verdict, `--list` to see them) plus `scripts/parity-web.sh`, and `./test.sh` here.
