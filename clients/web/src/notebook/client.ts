// What the notebook front end needs of this package, as one module.
//
// This is an *entry point*, not a layer: every name below is re-exported
// unchanged from where it lives, and nothing here has behaviour. It exists
// because of how the front end is loaded, and that is worth stating once.
//
// anywidget serves the widget's own module and nothing beside it, so the
// package cannot be imported by URL and travels over the kernel's comm as
// bytes, which the page turns into blob URLs. A blob URL is not hierarchical:
// a module loaded from one cannot resolve a relative specifier, so every
// import is rewritten to the blob URL of the module it names, and the blobs
// have to be made leaf-first. That works for a tree and **cannot work for a
// cycle** — to rewrite A's import of B you need B's URL, and in a cycle it does
// not exist yet. The client has three such cycles (`base/main` ↔ `environment`
// ↔ `rand`, `clock` ↔ `stream` ↔ `main`, `defs/server` ↔ `render` ↔ `session`),
// all of them ordinary ESM that a browser resolves without complaint anywhere
// a URL has a path.
//
// So the notebook is handed one file instead of a graph: `build.sh` bundles
// this entry into `dist/notebook-client.js`, where the cycles are references
// inside a module rather than edges between files. It is also what keeps the
// payload honest — the front end uses a fraction of the package, and a bundle
// of this entry carries that fraction rather than all of it.
//
// Adding a name here is how the front end grows. Adding one anywhere else is
// how it stops loading.

export { loadOsc, decodePacket } from "../base/osc.ts";
export type { OscMessage } from "../base/osc.ts";
export { engine } from "../engine/server.ts";
export type { ClaustersServer } from "../engine/server.ts";
export { newGuiHost, canvasBox, onScaleChange } from "../gui/page.ts";
export type { CanvasBox, ClaustersGui } from "../gui/page.ts";
export { GuiHost } from "../gui/host.ts";
export { Session } from "../session.ts";
// The clock's wake-up lives in a worker, and a bundle running from a blob
// URL cannot name its module — so the front end, which was handed it with
// everything else, says where it is.
export { setTickWorkerUrl } from "../base/clock.ts";
