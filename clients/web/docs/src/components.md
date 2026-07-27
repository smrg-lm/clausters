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

Removing an element from the DOM does **not** yet free what it allocated — the mount has no unmount half yet.

## The slim runtime

Two entry points target the same elements:

- **`dist/runtime.js`** — the engine, the GUI host, the OSC codec and the mount. This is what a page that only mounts components loads.
- **`dist/index.js`** — the above plus the whole TypeScript client (the def builders, the GuiDef builders, the sequencing layer), for a page that sequences, responds or edits live.

The split is enforced by a test over the module graph, so the builders cannot creep into the runtime entry by an accidental import.

## Authoring a bundle

Bundles are written with the Python client's writer, `clausters.bundle.Bundle`: it holds the symbol table so the author names things instead of numbering them, prefixes the def names with the bundle's, declares the parameters and the presets, and validates through the shared core before emitting — an unmountable bundle is unwritable. `examples/piano/make_bundle.py` in the repository is the worked example, and the format itself is documented in the [server book](https://clausters.readthedocs.io/).

One authoring rule follows from the holes living only in the GuiDef record: a bus or a node reaches a def **as a control**, never as a value baked into the def, or the def could not be shared between two instances.
