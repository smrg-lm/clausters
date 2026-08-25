// The GUI client (mirrors `clausters/gui/__init__.py`).
//
// The GUI host is a second peer in the Clausters system — a *GUI server* for
// the language clients and itself a *client of the audio server* — driven over
// the same OSC encoding the audio server uses, with the `/gui_*` vocabulary.
// This module is the TypeScript side of that protocol: `./guidef.ts` composes
// a widget tree as JSON (the GUI analogue of a `SynthDef`), and `GuiHost`
// sends it and reads the widgets back.
//
// The host renders containers, the standard controls, the editor-grade
// `waveform`/`spectrogram` views, the bus-backed `meter`/`scope`, the
// audio-tap `scope`/`phasescope`/`spectrum`, a playable `piano`, a live
// `nodetree`, a static `plot`, a drawable `bpf` envelope, a multitrack
// `track`/`clip` timeline, an engraved `score` page and a `patch` patcher;
// live updates flow through `set` and interactions come back as
// `/gui_event`/`/gui_closed`. A widget can also be **bound** (`GuiHost.bind`)
// so its value flows straight to the audio server, bypassing this script.
//
// In the browser the host is the wasm one on this page's canvas
// (a handle over `pageGuiConnection()`, `boot`ed) or a native
// `clausters-gui --ws` over a WebSocket (a handle over a `WsConnection`,
// `attach`ed) — the same
// object either way.

export { formatWidgetInfo, GuiHost, guiHost, newGuiHost, pageGuiConnection, DEFAULT_PORT, DEFAULT_WS_PORT } from "./host.ts";
export { ambientHost, setAmbientHost } from "./ambient.ts";
export type {
    ClaustersGui, EventListener, PageGuiConnection, PropValue, WidgetInfo,
} from "./host.ts";

export { WidgetHandle, WindowHandle } from "./handle.ts";
export type { EventArgs } from "./handle.ts";

export { BASE_ID, CAPACITY, GuiIdAllocator } from "./ids.ts";

export { Editor, MEASURES } from "./editor.ts";
export type { EditorOptions, Measure } from "./editor.ts";
export { Transport } from "./transport.ts";
export type { TransportOptions, TransportTargets } from "./transport.ts";

export * from "./guidef.ts";

// Engraving, as its own namespace for the same reason the Python client keeps
// it a submodule: it is a layer (an engraver, an encoder, a view) rather than a
// widget, and its module loads 6.5 MB of verovio the first time a page uses it.
export * as notation from "./notation/index.ts";
