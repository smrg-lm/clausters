// The GUI client (mirrors `clausters/gui/__init__.py`).
//
// The GUI host is a second peer in the Clausters system -- a *GUI server* for
// the language clients and itself a *client of the audio server* -- driven over
// the same OSC encoding the audio server uses, with the `/gui_*` vocabulary.
// This module is the TypeScript side of that protocol: `./guidef.ts` composes
// a widget tree as JSON (the GUI analogue of a `SynthDef`), and `GuiHost`
// sends it and reads the widgets back.
//
// The host renders containers, the standard controls, the editor-grade
// `waveform`/`spectrogram` views, the bus-backed `meter`/`scope`, the
// audio-tap `scope`/`phasescope`/`spectrum`, a playable `piano`, a live
// `nodetree`, a static `plot`, a drawable `bpf` envelope, the `multitrack`
// the multitrack editor draws, an engraved `score` page and a `patch` patcher;
// live updates flow through `set` and interactions come back as
// `/gui_event`/`/gui_closed`. A widget can also be **bound** (`GuiHost.bind`)
// so its value flows straight to the audio server, bypassing this script.
//
// In the browser the host is the wasm one on this page's canvas
// (a handle over `pageGuiConnection()`, `boot`ed) or a native
// `clausters-gui --ws` over a WebSocket (a handle over a `OscWsInterface`,
// `attach`ed) -- the same
// object either way.

export { formatWidgetInfo, formatWindowProfile, GuiHost, DEFAULT_PORT } from "./host.ts";
export { ambientHost, appClock, setAmbientHost } from "./ambient.ts";
export type {
    EventListener, PropValue, WidgetInfo, WindowActivity, WindowProfile,
} from "./host.ts";

// The allocator `GuiHost.ids` answers with; `clausters/gui/ids` in Python.
export type { GuiIdAllocator } from "./ids.ts";

export { WidgetHandle, WindowHandle } from "./handle.ts";
export type { EventArgs } from "./handle.ts";


export { Editor, MEASURES, edit, measures } from "./editing/index.ts";
export type { Measure } from "./editing/index.ts";
// The collaborators live under their own name, which is also how the Python
// client reaches them (`clausters.gui.editing`): `View` here is the picture of a
// structure and would collide with the guidef `View` this module exports.
export * as editing from "./editing/index.ts";
export { PlayheadSync } from "./playhead-sync.ts";
export type { MapHolder, Pass, PlayheadSyncOptions, PlayheadSyncTargets } from "./playhead-sync.ts";

// The GuiDef builders the Python client's `clausters.gui` exports from its
// `guidef`, by the same names; the rest of the module (its vocabularies and
// its serializer) is `gui.guidef`, as in the Python client.
export {
    ICON, KEEP, Source, View, bpf, button, canvas, choice, code, curve, dialog, entry,
    envToPoints, fileDialog, files, keys, knob, label, layout, menu, meter, node,
    nodes, nodetree, number, pageJson, pager, pane, panel, patch,
    peaksCacheEmptyFile, peaksCacheFile, peaksCacheStreamFile, peaksCacheUpdateFile,
    phasescope, piano, plane, plot, pointsToEnv, post, progress, row, samplesToBlob,
    samplesToFile, scope, score, scoreView, scroll, separator, signal, slider,
    source, spectrogram, spectrogramCacheFile, spectrum, stack, table, tabs, text,
    timeruler, toggle, toolbar, view, waveform, window,
} from "./guidef.ts";
export type * from "./guidef.ts";
export * as guidef from "./guidef.ts";
