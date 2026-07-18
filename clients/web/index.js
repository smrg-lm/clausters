// Clausters in the browser — the package surface.
//
// The audio server compiled to wasm inside an AudioWorklet, the GUI host on a
// canvas, and web components that boot native-format standalone bundles — no
// server process anywhere. This package seeds the TypeScript client track
// (clients/web/PLAN.md); the raw `server()` handle (send / addReply / clock)
// is the surface that client will build on.
//
// Importing this module registers the `<clausters-bundle>` and
// `<clausters-power>` custom elements as a side effect; the singletons stay
// lazy until first used.

export { server } from "./server.js";
export { guiHost } from "./gui.js";
export { bootBundle } from "./bundle.js";
export { ClaustersBundle, ClaustersPower } from "./elements.js";
export { encodeMessage, decodePacket } from "./engine/osc.js";
