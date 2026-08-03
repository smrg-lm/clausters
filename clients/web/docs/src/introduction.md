# clausters - web client

`clausters` is the **TypeScript client** for the [Clausters](https://clausters.readthedocs.io/) audio server, and the browser's whole side of the system: the server itself compiled to WebAssembly and running inside an **AudioWorklet**, the GUI host drawing on a `<canvas>`, web components that mount a prebuilt instrument in a document, and the client library that drives all of it.

It is the sibling of the [Python client](https://clausters-python.readthedocs.io/), and deliberately its mirror: the same module tree at the same relative paths, the same builders emitting the same JSON, the same events and patterns. What is different is only what the browser makes different — the carriers, the fact that everything which waits is a promise, and a page instead of a process.

This is the **package documentation**. The server itself — the OSC protocol, the two def formats, the node-tree model, the `/gui_*` widget protocol and the bundle format — is documented in the **[Clausters server book](https://clausters.readthedocs.io/)**, and the shared client model (defs, clocks, events, patterns) in the **[Python client book](https://clausters-python.readthedocs.io/)**. This site links to both rather than repeating them. Three books, one per platform.

One rule from that book is worth carrying here, because it is what makes the addresses this package builds readable: every server command is **`/<resource>_<action>`** — the resource in full (`node`, `synth`, `group`, `bus`, `buffer`, `def`, `ugen`, `server`, …), the action in camelCase, a reply as `<command>.reply`, a range as `Range`.

## Two carriers, one client

A page reaches a server in one of two ways, and **only one line of a program names which**:

- **The in-page engine.** The server is a wasm build running in this tab's AudioWorklet: no process, no socket, no install. `pageConnection()` is that carrier.
- **A `--ws` server.** A native `clausters --ws` (or `clausters-gui --ws`) elsewhere on the machine or the network, reached over a browser `WebSocket`. `WsConnection.open(url)` is that carrier.

Everything above the connection seam — the `Server`, the def builders, the `GuiHost`, the clock, the patterns — is written once and runs over either. The in-page engine is a `synth,embed` build with no LLVM, so it compiles UGen-graph SynthDefs but not Faust source; that is the one asymmetry between the two.

## What is in the package

- **The OSC codec and the numeric core** (`base/`) — `clausters-core` compiled to wasm. The bytes on the wire, the beat/second/sample arithmetic, the scheduler queue, the seeded random stream and the builtins are the *same code* the server and the Python client run, so results match by construction rather than by care.
- **The def model** (`defs/`) — `Server` (the only object that knows a connection), `SynthDef` with the lowercase UGen callables, `FaustDef` with the signal API, `GraphDef`, and the `Node`/`Bus`/`Buffer` handles whose ids come from the core's own allocator.
- **The GUI driver** (`gui/`) — `GuiHost` over the same connection seam, and the whole widget catalogue as builders (`gui.window`, `gui.knob`, `gui.waveform`, `gui.track`, …) emitting the same GuiDef JSON the Python builders emit.
- **The sequencing layer** (`base/clock.ts`, `seq/`) — a `TempoClock` that resumes generator routines on musical time, `Event`, the value patterns and `Pbind`, `Timeline` and `Playhead`, under either timebase (the page's monotonic clock, or the server's own sample clock).
- **The page runtime** (`engine/`, `bundle.ts`, `elements.ts`) — the page's engine and GUI host (one of each by default, more when a caller asks) and the custom elements that mount a bundle. A page that only *mounts* an instrument loads this and none of the builders.

## How to read this book

- **New here?** [Getting started](getting-started.md): build the package, serve it, make a sound in a tab.
- **Want the mental model?** [The client, layer by layer](guide.md): the connection seam, the def model, the GUI driver and the clock — and the three places where the browser changes the shape of the reference client.
- **Handing an instrument to a page?** [Components](components.md): a bundle mounted as a custom element, with no client library loaded at run time.
- **Looking for runnable code?** [Examples](examples.md).
- **Looking for a symbol?** The [API reference](api/index.md) is generated from the package's own doc comments.

## What is not here yet

The client is usable and complete through the layers above; some of the Python client's surface has not been ported. Today the package has no MIDI and no live `scope`; the UGen catalogue covers the families but not every builder, the Faust box algebra is absent, and Faust source needs a native server to compile. The roadmap lives in `clients/web/PLAN.md` in the repository.

## License

**GPL-3.0-or-later**.
