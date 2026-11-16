/**
 * **What only a page has**: the engine and the GUI host compiled into the
 * tab, the links a worker reaches them by, the core's loader, the browser's
 * MIDI ports, the page's own filesystem, what wakes a clock in a tab, and
 * the elements that mount a bundle.
 *
 * The rest of the package is the client both languages share, module for
 * module and name for name. This one is the platform's, the way the Python
 * client's `clausters.ipc` and `clausters.launch` are a native process's
 * (the shared memory, the embedded library, the processes a script boots):
 * a page boots no process and maps no segment, and a script on a desktop
 * opens no `AudioWorklet`. So nothing here has a counterpart to keep in
 * step, and nothing outside it is allowed to be the platform's alone.
 *
 * `clausters/runtime` is the part of this module a page that only mounts a
 * bundle loads: the same names, in an entry that reaches none of the
 * builders.
 *
 * @module
 */

// The engine in the page, and the byte link it is spoken to on.
export { server, engine, ANY_PEER, DEFAULT_PEER } from "./engine/server.ts";
export type { ClaustersServer, EngineLink, LinkMessage, ReplyListener } from "./engine/server.ts";
export type { BootOptions, ClockAnchor } from "./engine/loader.ts";
export {
    decodePacket,
    decodePacketTimed,
    encodeBundle,
    encodeImmediateBundle,
    encodeMessage,
} from "./base/osc.ts";
export type { BundleMessage, OscArg, OscMessage, TimedOscMessage } from "./base/osc.ts";
export { pageConnection, portConnection } from "./base/connection.ts";
export type { PageConnection } from "./base/connection.ts";
// The offline renderer, the engine's other build.
export { loadRenderer } from "./engine/render.ts";
export type { EngineModule } from "./engine/render.ts";
// The id pools the page's engine and host split between their clients.
export { ENGINE_SHAPE, newPools, pagePools } from "./base/pool.ts";
export type { Pool, PoolShape, Pools } from "./base/pool.ts";
// The GUI host in the page, and how an element is measured against the
// display -- what a page needs wherever it sizes a canvas itself.
export { guiHost, newGuiHost, pageGuiConnection } from "./gui/page.ts";
export type { ClaustersGui, GuiLink, PageGuiConnection } from "./gui/page.ts";
export { canvasBox, onScaleChange } from "./gui/page.ts";
export type { CanvasBox } from "./gui/page.ts";
// The shared core, which a page loads before a model is built.
export { loadCore } from "./base/core.ts";
// What wakes a clock: a tab throttles its timers, so the page chooses.
export { defaultTicker, manualTicker, timerTicker, workerTicker } from "./base/clock.ts";
export type { ManualTicker, Ticker } from "./base/clock.ts";
// Where the engraver's wasm is loaded from, which a native process never asks.
export { setEngraverUrl } from "./notation/_verovio.ts";
// The browser's MIDI ports, which a page asks the person for.
export { requestMidiPorts } from "./base/midi.ts";
// A bundle mounted, and the elements that mount one.
export { bootBundle, freeBundle, openBundle, startBundle } from "./bundle.ts";
export type { BundleManifest, MountOptions, Mounted, ParamSpec } from "./bundle.ts";
export { ClaustersBundle, ClaustersPower, defineComponent, startPage } from "./elements.ts";

/**
 * The page's own filesystem (OPFS) -- where a soundfile a tab reads actually
 * lives.
 *
 * The platform's answer to "where is the file", the way a disk is a native
 * server's. `/buffer_allocRead "take.wav"` names the server's filesystem in a
 * window and this one in a tab, and the call is the same call either way.
 */
export * as opfs from "./engine/opfs.ts";
