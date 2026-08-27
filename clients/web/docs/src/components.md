# Components: an instrument in the page

On the desktop a GuiDef rooted in a `window` opens a window, and the system's window manager places it. In a tab the drawing surface is a `<canvas>` in a document, and **the document does the placing** — CSS, the order of the markup, the flow of the page. A **component** is that substitution: an instrument mounted as a custom element, so canvases interleave with prose and images and one page can be an interactive text with the instrument sounding beside the paragraph that explains it.

Running one is the browser's equivalent of `clausters-gui --standalone`: the GUI host is the server's client, and **no client library is loaded at run time**. The builders ran earlier, in the authoring script; what the page fetches is data.

## Mounting one

A **bundle** is an instrument written to a directory — the defs, one GuiDef, presets, samples, a manifest — plus a generated ES module that registers its tag. Importing that module is the whole integration:

```html
<script type="module">
  import "./fm-voice/index.js";   // registers <fm-voice>
</script>

<style>fm-voice { display: block; width: 100%; height: 340px; }</style>

<p>Prose, and then the instrument in the flow of the page:</p>
<fm-voice></fm-voice>
<fm-voice freq="110" preset="bright"></fm-voice>
```

`<clausters-bundle src="./fm-voice">` mounts a bundle that has no generated module, and `<clausters-power>` is the standard autoplay-policy affordance — a button that starts the page's audio.

Each element owns its canvas. Two instances of one bundle hold **their own node ids, buses and widget ids** while the def they share is sent **once**: the bundle's GuiDef is a template with two kinds of hole — a symbol the page allocates, and a declared parameter the tag supplies — and only that record has holes, so the def payloads are byte-identical between instances. Declared parameters are attributes, resolved attribute → preset → default.

## The two phases

A component mounts in two steps, because an `AudioContext` is page-wide and N power buttons would be wrong:

1. **On connect** — the GuiDef is allocated, resolved and opened, and the canvas draws. This needs no gesture and no audio.
2. **On the page's first gesture** — the engine half: the defs go out, the buffers load, the voices start. One gesture anywhere starts every component on the page.

A component scrolled out of the viewport stops drawing and drops its buses from the streams feeding it, so a long document can hold many instruments and cost only the ones in view. A component that fails to mount fails alone; the rest of the page stays up.

## Removing one

An element removed from the document gives back everything it took, and nothing the page shares. Its window and its widgets are freed, the nodes its boot instantiated are freed, its canvas leaves the host, and the widget, node and bus ids it drew from the page's pools return to them — so a long document that adds and removes instruments as the reader goes holds a flat occupancy instead of climbing. It goes quiet at once, and no other component notices.

What stays is what belongs to the page: the `AudioContext`, the GUI host, and — deliberately — the def payloads and the sample buffers. Both are shared by URL between every instance of a bundle and are the same data whoever asks, so freeing them would be freeing a sibling's; a component mounted again finds them loaded and comes up the faster for it.

```js
const voice = document.createElement("fm-voice");
voice.setAttribute("freq", "220");
article.append(voice);   // mounts, and sounds on the page's gesture
voice.remove();          // frees its window, its nodes and its ids
```

Removing is not a pause: an element connected again **mounts afresh** — same bundle, new allocation, the attributes and the preset resolved again — rather than resuming what it had. That is why the unmount can be complete.

The other direction closes too. A window the *host* closes rather than the page — a `/gui_closed`, which is what a native host sends when the user closes the window a component mounted into — reaches the element that mounted the def: it unmounts and emits `clausters-closed` (detail: `{ id }`), so a page never holds a live tag over a freed def.

## The slim runtime

Two entry points target the same elements:

- **`dist/runtime.js`** — the engine, the GUI host, the OSC codec and the mount. This is what a page that only mounts components loads.
- **`dist/index.js`** — the above plus the whole TypeScript client (the def builders, the GuiDef builders, the sequencing layer), for a page that sequences, responds or edits live.

The split is enforced by a test over the module graph, so the builders cannot creep into the runtime entry by an accidental import.

## Authoring a bundle

A bundle is written with `Bundle` — this client's writer and the Python client's `clausters.bundle.Bundle` are the same writer in two languages. It holds the symbol table so the author names things instead of numbering them, prefixes the def names with the bundle's, declares the parameters and the presets, and validates through the shared core before emitting — an unmountable bundle is unwritable. The format itself is documented in the [server book](https://clausters.readthedocs.io/).

`param`, `node`, `bus` and `buffer` each hand back the placeholder that stands for what they declare, so the template is written with names and the mount fills in numbers:

```ts
import { Bundle, loadCore } from "clausters/bundle-writer";
import * as defs from "clausters/defs";
import * as gui from "clausters/gui";

await loadCore();                      // the writer validates through the core
const b = new Bundle("fm-voice");
const freq = b.param("freq", "float", { default: 220.0, min: 60.0, max: 700.0 });
const lfo = b.bus("lfo");
const node = b.node("voice");

b.synthdef(voice());                   // named "fm-voice.voice"
b.gui(gui.view({ layout: "col" },
               gui.knob({ label: "freq", value: freq,
                          bind: ["/node_set", node, "freq"], id: 2 }),
               gui.meter(lfo, { rate: "control", id: 3 })));
b.boot(["/synth_new", "fm-voice.voice", node, 0, 0, "freq", freq]);
b.preset("bright", { freq: 660.0 });

await b.write("./fm-voice");           // node: the directory a page serves
const files = b.files();               // a page: the same bundle as text
```

**Where it runs.** `write` takes a directory and is a **node** verb, and that is the shape of the thing rather than a limitation: a bundle is an *input*, produced ahead of time and saved, so that a static page can boot it with no interpreter at all. A page can author one too; what it gets is `files`, the same bundle as text, which `openBundle({ files })` mounts with no round trip through disk (`examples/components/authored.html`).

The two writers emit **the same bytes** for the same authoring calls — canonical JSON, keys sorted, an integral float spelled `220` because JavaScript has no other spelling for it — which is what makes "one bundle, two languages" a thing a test can check rather than a claim.

One authoring rule follows from the holes living only in the GuiDef record: a bus or a node reaches a def **as a control**, never as a value baked into the def, or the def could not be shared between two instances.
