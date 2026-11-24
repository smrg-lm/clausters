# Clausters — web client

[![npm](https://img.shields.io/npm/v/clausters)](https://www.npmjs.com/package/clausters)
[![Web client book](https://readthedocs.org/projects/clausters-web/badge/?version=latest)](https://clausters-web.readthedocs.io/)
[![Server book](https://readthedocs.org/projects/clausters/badge/?version=latest)](https://clausters.readthedocs.io/)
[![Python client book](https://readthedocs.org/projects/clausters-python/badge/?version=latest)](https://clausters-python.readthedocs.io/)

📖 **Documentation:** [web client book](https://clausters-web.readthedocs.io/)
(this package) · [server book](https://clausters.readthedocs.io/) (the OSC
protocol, the wire formats, the engine) ·
[Python client book](https://clausters-python.readthedocs.io/) (the same model
on the desktop)

The [Clausters](https://github.com/smrg-lm/clausters) audio server in the
browser, with no server process anywhere: the engine compiled to WebAssembly
running in an **AudioWorklet**, the GUI host drawing on a canvas, and the
**TypeScript client** that drives them — the same client as the Python one, in
another language, with the same surface. It covers both of the server's
definition formats as peers: **UGen-graph `SynthDef`s** and **`FaustDef`s**,
the latter built from the `signals` API, the `boxes` API or Faust source,
compiled in the page.

## Quick start

```sh
npm install clausters
```

That is the whole install: the tarball carries the modules, their type
declarations and the wasm bundles (the engine, the GUI host, the shared core),
so nothing is compiled and nothing is fetched at run time. Through a bundler
the imports are written `from "clausters"`, and a layer `from "clausters/defs"`
(likewise `seq`, `gui`, `data`, ...); a page served with no build step imports
the same modules by path (`./dist/index.js`, `./dist/defs/index.js`).

The engine runs in an AudioWorklet and the GUI host draws with WebGPU, and both
need a **secure context** (`http://localhost` counts). A browser starts no
audio without a gesture, so a page calls the first line that touches the engine
from an event handler.

```ts
import { Session } from "clausters";
import { Synth, SynthDef, control, out, sine } from "clausters/defs";

const session = await Session.embed({ latency: 0.1 });   // an engine in this tab
const server = session.server;

// an instrument: a def the server compiles once and instantiates many times
const sdef = new SynthDef("beep", out(0.0, sine(control("freq", 440.0)).mul(control("amp", 0.2))));
await sdef.send(server);       // /def_send synth, resolves on the server's /done

const node = new Synth("beep", { freq: 330.0 }, { server });   // you hear it now
node.set({ freq: 550.0 });     // change it while it sounds
node.free();                   // silence
```

TypeScript has no operator overloading, so a graph composes by method:
`sine(freq).mul(amp)` where the Python client writes `sine(freq) * amp`, and
the two emit the same def.

The other def format is a peer, not a fallback — the same voice as a
`FaustDef`, which the page compiles with the Faust compiler the package
carries:

```ts
import { boxes as box, FaustDef, signals as sig } from "clausters/defs";

const freq = sig.hslider("freq", 440.0, 20.0, 20000.0, 0.01);
const phase = sig.rec((p) => p.add(freq.div(sig.sr())).mod(1.0));   // one-sample feedback phasor
await FaustDef.fromSignals("fbeep", sig.sin(phase.mul(sig.TAU)).mul(0.2)).send(server);

// or point-free with the box API, borrowing the oscillator from Faust's library:
await FaustDef.fromBox(
    "bbeep", box.faust("os.osc").call(box.hslider("freq", 440.0, 20.0, 20000.0, 0.01)).mul(0.2)).send(server);

new Synth("bbeep", { freq: 220.0 }, { server });
```

Or hand a **pattern** to the session; it plays on the session's clock, beside
the page's own work:

```ts
import { pbind, pseq, pwhite } from "clausters/seq";

session.play(pbind({ instrument: "beep",
                     freq: pseq([330, 440, 550, 660], 8),
                     dur: 0.25,
                     amp: pwhite(0.1, 0.2) }));
session.start();     // the clock runs while the page goes on
session.stop();
session.close();     // stops everything the session started
```

`Session.live(url)` attaches to a native `clausters --ws` server instead of an
engine in the tab, and `Session.nrt()` renders offline; the rest is unchanged.
See the book's [Getting started](https://clausters-web.readthedocs.io/en/latest/getting-started.html).

## What is in the package

The modules mirror the Python client's, at the same relative paths, and each
layer is an entry point of its own (`clausters/<layer>`):

- `clausters` — the facade: `Session`, `Server`, `GuiHost`, the node and
  resource handles, `TempoClock`, `Event`, `Timeline`, the ambient verbs
  (`play`, `render`, `plot`, `scope`, `edit`, `panic`) and the responders.
- `clausters/defs` — the definitions and server resources: the UGen callables
  (`sine`, `envGen`, `out`, ..., the whole catalogue in
  `clausters/defs/ugens`) with `SynthDef`, `FaustDef`
  with `signals` and `boxes`, `GraphDef` for multi-node graphs, and
  `Synth`/`Group`/`Bus`/`Buffer`. Everything that waits is a promise: where
  the Python client blocks on a reply, this one `await`s.
- `clausters/seq` — `Event`, the value patterns (`pseq`, `pwhite`, `pseries`,
  ...) and `pbind`, and `Timeline`, a static, editable list with a
  random-access transport. Routines are generators (`yield` a delay in
  beats), and every beat-to-time conversion is the shared core's, so a beat
  lands on the same instant here, in the Python client and in the server.
- `clausters/gui` — the GuiDef builders (`view`, `knob`, `slider`,
  `waveform`, ...), the same documents the Python builders emit; widgets are
  addressed by name, and a bound widget's value goes from the host to the
  engine with no round trip through the page's script.
- `clausters/document` and `clausters/gui/editing` — the document the
  applications edit (sources, regions, tracks, sequences) and the
  applications over it: the audio, multitrack, notes and score editors.
- `clausters/page` — what only a page has: the engine and the GUI host in the
  tab, the core's loader, the browser's MIDI ports and the page's own
  filesystem (the origin private file system, where every path in a tab
  lives).
- `clausters/runtime` — the component run time alone (the engine, the host,
  the OSC codec and the mount), for a page that only mounts bundles.
- `clausters/bundle` — the bundle writer (below).
- The books, as Markdown: this client's and the server's, which
  [clausters-editor](https://github.com/smrg-lm/clausters/tree/main/crates/clausters-editor)'s
  documentation viewer shows offline.

## An instrument in the page

A **bundle** is an instrument written to a directory (defs, one GuiDef,
presets, samples) plus a generated ES module that registers its tag.
Importing that module is the whole integration — **no client library is
loaded at run time**:

```html
<script type="importmap">
  { "imports": { "clausters/runtime": "./node_modules/clausters/dist/runtime.js" } }
</script>
<script type="module">
  import "./fm-voice/index.js";   // registers <fm-voice>
</script>

<style>fm-voice { display: block; width: 100%; height: 340px; }</style>

<p>Prose, and then the instrument in the flow of the page:</p>
<fm-voice></fm-voice>
<fm-voice freq="110" preset="bright"></fm-voice>
```

The generated module imports the run time as `clausters/runtime`: a bundler
resolves it, and a page with none maps it once, as above. Each element owns
its canvas, and **the document places it**. Two instances
of one bundle hold their own node ids and buses, and the def they share is
sent once. Declared parameters are attributes, resolved attribute → preset →
default. The first gesture anywhere on the page starts the audio for all of
them; a component scrolled out of view stops drawing and streaming.
`<clausters-bundle src="./fm-voice">` mounts a bundle with no generated
module. Write one with `Bundle` (`clausters/bundle`, and `clausters.bundle` in
the Python client: one writer in two languages, emitting the same directory
byte for byte). The format is in the book's
[Bundles](https://clausters-web.readthedocs.io/en/latest/bundles.html)
chapter.

## Driving it from script

```html
<script type="module">
  import { server, guiHost } from "clausters/runtime";
  // registers <clausters-bundle> and <clausters-power> as a side effect
</script>
```

The page surface:

- `server()` — the lazy **per-page engine singleton**: `send(bytes)` /
  `addReply(listener)` raw OSC, `clock()`, `resume()` / `suspend()`. Every
  component and script on the page gets the same engine, so they meet in one
  node, bus and buffer namespace.
- `guiHost()` — the per-page GUI host singleton, wired to the engine in the
  page. It draws **one canvas per `window`-rooted def**: `attach(defId,
  canvas)` gives a def its surface, and the host is told its size and its
  visibility, never the DOM.
- `openBundle(...)` / `startBundle(...)` — the two phases of a mount by hand
  (allocate, resolve and draw; then the engine half, on a gesture);
  `bootBundle(...)` does both for a script that already has one, and
  `freeBundle(...)` gives back what a mount took.
- `<clausters-bundle>` / `<clausters-power>` — the elements; the power button
  is the standard affordance of the browsers' autoplay policy, and
  `startPage()` is what it calls.

**Two entry points.** `clausters/runtime` is what a page that mounts
components loads: the engine, the host, the OSC codec and the mount, and
nothing of the client. `clausters` adds the TypeScript client — the def
builders, the GuiDef builders, the sequencing layer — for a page that
sequences, responds or edits live. Both target the same elements, and the
package's tests hold the line between them.

## Examples

The [`examples/`](https://github.com/smrg-lm/clausters/tree/main/clients/web/examples)
directory of the repository holds served pages, grouped by subject (`basics/`,
`views/`, `editors/`, `components/`, ...), each saying in its header what it
shows. From a built checkout:

```sh
cd clients/web
python3 tools/serve.py        # http://localhost:8000/
```

and open `examples/basics/synth.html`, the shortest end-to-end path through
the client, or `examples/components/demo.html` for the components.
`tools/serve.py` sends the headers an editing application needs to share
memory with the engine; any static server serves the rest.

## From the repository

Building it yourself is only needed to work *on* the client. It takes node, the
Rust toolchain the repository pins (with the `wasm32-unknown-unknown` target)
and the `wasm-bindgen` CLI at the version the lockfile names;
[`BUILD.md`](https://github.com/smrg-lm/clausters/blob/main/clients/web/BUILD.md)
has the recipes.

```sh
cd clients/web
npm install       # once: the build's tools
./build.sh        # the wasm bundles, staged into dist/, then src/ -> dist/
./test.sh         # type-check, the node suites and the pages in headless Chrome
./test.sh --browsers chrome,firefox   # the pages in every browser (before a release)
```

`dist/` is then the whole package: plain ES modules, their declarations and
source maps, and the wasm bundles beside them. Nothing is bundled, and any
static server serves it. `docs/build.sh` builds the book locally, its API
reference generated from the sources' TSDoc.

## License

**GPL-3.0-or-later**. The engine's Faust compiler (libfaust) is GPLv2+.
