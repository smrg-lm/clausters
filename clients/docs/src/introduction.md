# clausters - {{alt Python client || web client}}

<!-- python -->
`clausters` is the reference high-level **Python client** for the [Clausters](https://clausters.readthedocs.io/) audio server, ported selectively from SuperCollider's class library ([sc3](https://github.com/smrg-lm/sc3)). It covers both of the server's definition formats as peers: UGen-graph **SynthDefs** and **FaustDefs** — and a FaustDef, in turn, is built equally from the signal API, the box API or Faust source.

It is **pure Python at runtime**: it reaches the shared native core through `ctypes` over the `clausters-ffi` cdylib, and speaks ordinary OSC bytes to the server (UDP, TCP, shared memory, or an in-process embedded server). A NumPy user can wrap a returned `array('f')`, but NumPy is never a dependency — only flat data crosses the binding.
<!-- /python -->
<!-- web -->
`clausters` is the **TypeScript client** for the [Clausters](https://clausters.readthedocs.io/) audio server, and the browser's whole side of the system: the server itself compiled to WebAssembly and running inside an **AudioWorklet**, the GUI host drawing on a `<canvas>`, web components that mount a prebuilt instrument in a document, and the client library that drives all of it. It covers both of the server's definition formats as peers: UGen-graph **SynthDefs** and **FaustDefs** — and a FaustDef, in turn, is built equally from the signal API, the box API or Faust source, compiled in the tab itself.

It reaches the shared native core compiled to wasm, and speaks ordinary OSC bytes to the server — the engine in the page, a native server over a WebSocket, or a score rendered offline.
<!-- /web -->

This is the **package documentation**. The server itself — the OSC protocol, the def formats (SynthDef JSON and Faust), the node-tree model, the `/gui_*` widget protocol, the C ABI contract, and how to embed it — is documented separately in the **[Clausters server book](https://clausters.readthedocs.io/)**; this site links to it rather than repeating it. {{alt The **[web client book](https://clausters-web.readthedocs.io/)** || The **[Python client book](https://clausters-python.readthedocs.io/)**}} is the third, and it is this book in the other language: the two clients are one client in two languages, so the chapters they share are the same text, with every example in the language of the book's client, and the same calls in the same order. One book per platform.

## Components

- A **server-agnostic** timing and value layer ({{alt `clausters.base` || `base/`}}): scalar/list math backed by the same core the server uses (so results match by construction), {{alt operator overloading || the operators as methods}}, the `Routine`/`yield` coroutine layer, a `TempoClock` that does timing only, and a choice of timebase (monotonic or the server's sample clock).
- **Definitions and server resources** ({{alt `clausters.defs` || `defs/`}}): the UGen graph and `SynthDef`, `FaustDef` (from `signals`, from `boxes`, or from Faust source), `GraphDef`, plus the `Node`/`Bus`/`Buffer` handles. The `Server` owns the {{alt communication interface || connection}} and emits through it — swap it to retarget the *same* code from a live RT take to an offline NRT score.
- **Sequencing** ({{alt `clausters.seq` || `seq/`}}): `Event`, the value patterns (`pseq`, `pwhite`, `pseries`, ...) and `pbind`, `EventStreamPlayer` with yield-exact timing, `Timeline` and `EventSequence`.
- **The visual elements** ({{alt `clausters.gui` || `gui/`}}): `GuiHost` and the widget vocabulary as builders, which emit the same GuiDef JSON in both clients, vector for vector.
- **Ergonomic defaults without global state** (`Session`): bundles a `Server` and a clock; several sessions (e.g. an offline one for plots next to a live one) coexist in one {{alt script || page}}.
- **The ambient verbs** (`play` / `plot` / `render`, plus the live `scope`): one free-standing entry each for sounding, showing and bouncing whatever you have — an event, a pattern, a bare signal expression, a def, a timeline, a sequence, a buffer — resolved against the ambient session.<!-- web -->
- **The page runtime** (`engine/`, `bundle.ts`, `elements.ts`): the page's engine and GUI host (one of each by default, more when a caller asks) and the custom elements that mount a bundle. A page that only *mounts* an instrument loads this and none of the builders.<!-- /web -->

## The RT / NRT / embed seam

The key design property is a single seam: the `Server` holds a {{alt communication interface || connection}}, and *which* one it holds decides where the bytes go — {{alt a live UDP/TCP server, shared memory, or an in-process embedded engine || the engine in this tab, a native server over a WebSocket, a worker's link onto the page's engine}}, or a score rendered to a WAV offline. The clock, the routines and the patterns do not change. The same {{alt script || page}} can drive a real-time take and render an offline score, with no global build context in between.

<!-- web -->
A page reaches a server in one of two ways, and **only one line of a program names which**:

- **The in-page engine.** The server is a wasm build running in this tab's AudioWorklet: no process, no socket, no install. `pageConnection()` is that carrier.
- **A `--ws` server.** A native `clausters --ws` (or `clausters-gui --ws`) elsewhere on the machine or the network, reached over a browser `WebSocket`. `OscWsInterface.open(url)` is that carrier.
<!-- /web -->

## How to read this book

- **New here?** Start with [Getting started](getting-started.md): {{alt install the package and make a sound, line by line in a REPL || get the package, serve it, and make a sound in a tab}}.
- **Want the mental model?** Read [The client, layer by layer](guide.md): `base`, `seq`, `defs` and the seam.
- **Want the ergonomic entry point?** [Sessions](sessions.md) shows how one handle drives a live take or an offline render, and [The ambient verbs](verbs.md) covers `play`, `plot`, `scope` and `render` — what each accepts and why the split.
- **Building an instrument?** [Defining instruments](defs.md) is the def vocabulary, both families.
- **Driving the clock yourself?** [Routines and clocks](routines-and-clocks.md) writes routines by hand, and [Timing models](timing-models.md) covers the `TempoClock`'s timing modes — wall-clock, sample-locked, shared transport — and how to observe each.
- **Drawing something?** [The visual elements](gui.md): the widget vocabulary and the host that draws it.<!-- web -->
- **Handing an instrument to a page?** [Components](components.md): a bundle mounted as a custom element, with no client library loaded at run time; [What a tab cannot do](platform.md) is the page's own list.<!-- /web -->
- **Looking for runnable code?** See [Examples](examples.md).
- **Looking for a symbol?** The {{alt [API reference](api/clausters.md) || [API reference](api/index.md)}} is generated from the package's own {{alt docstrings || doc comments}}.

## License

**GPL-3.0-or-later**.
