// Clausters in the browser — the package surface.
//
// The audio server compiled to wasm inside an AudioWorklet, the GUI host on a
// canvas, and web components that boot native-format standalone bundles — no
// server process anywhere. On top of that runtime sits the TypeScript client
// (clients/web/PLAN.md): the `Connection` seam (`base/connection.ts`) carries
// OSC over either carrier — the in-page engine or a `--ws` server over
// WebSocket — and `defs/` builds and drives what runs on it (`Server`, the
// two def families, nodes, buses and buffers), naming no transport.
//
// Importing this module registers the `<clausters-bundle>` and
// `<clausters-power>` custom elements as a side effect; the singletons stay
// lazy until first used.

export { server } from "./engine/server.ts";
export type { ClaustersServer, ReplyListener } from "./engine/server.ts";
export { guiHost } from "./gui/host.ts";
export type { ClaustersGui } from "./gui/host.ts";
export { bootBundle } from "./bundle.ts";
export type { BundleManifest } from "./bundle.ts";
export { ClaustersBundle, ClaustersPower } from "./elements.ts";
export { loadOsc, encodeMessage, decodePacket } from "./base/osc.ts";
export type { OscArg, OscMessage } from "./base/osc.ts";
export { loadCore } from "./base/core.ts";
export { WsConnection, pageConnection } from "./base/connection.ts";
export type { Connection } from "./base/connection.ts";
export * from "./defs/index.ts";
export {
    AllocationError,
    ClaustersError,
    CommandError,
    ReplyTimeout,
} from "./errors.ts";
