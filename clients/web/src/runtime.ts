// The component run time — what a page that *mounts* a bundle loads, and
// nothing else.
//
// Running a component is the browser equivalent of `clausters-gui
// --standalone` on the desktop: **the host is the server's client, and there
// is no scripting client in between**. The builders ran earlier, in the
// authoring script; what the page fetches is data. So this entry reaches the
// engine, the host, the OSC codec and the mount — and deliberately not the def
// builders (`defs/`), the GuiDef builders (`gui/guidef.ts`) or the sequencing
// layer (`seq/`), none of which a mounted bundle can use.
//
// `dist/index.js`, the whole package facade, stays what it was: a page that
// *does* want the TypeScript client — to sequence, to respond, to edit live —
// imports it on top of this one. Both postures target the same element.
//
// The exclusion is asserted, not hoped for: `tests/runtime-graph.test.ts`
// walks the emitted module graph of `dist/runtime.js` and fails if it ever
// reaches any of the three.

export { ClaustersBundle, ClaustersPower, defineComponent, startPage } from "./elements.ts";
export { bootBundle, openBundle, startBundle } from "./bundle.ts";
export type { BundleManifest, MountOptions, Mounted } from "./bundle.ts";
export { pagePools } from "./base/pool.ts";
export type { Pool, Pools } from "./base/pool.ts";
export { guiHost } from "./gui/page.ts";
export type { ClaustersGui } from "./gui/page.ts";
export { server } from "./engine/server.ts";
export type { ClaustersServer } from "./engine/server.ts";
