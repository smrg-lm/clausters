// The session layer on its own: `clausters/session`.
//
// The facade (`index.ts`) registers the `<clausters-bundle>` and
// `<clausters-power>` custom elements when it is imported, which needs a
// document, so a program with none -- a script under node, a page's worker
// -- cannot import it. This subpath is what such a program
// writes a session against: the `Session` and its default, the environment
// they are, the two verbs that act on a session without drawing, `play` and
// `panic`, and the two that name what to look at on the session's GUI host,
// `plot` and `scope` (in a worker, the page's host, reached with
// `session.gui({ port })`). The same names, from the same modules, as the facade exports;
// nothing here is a second implementation, and the facade does not change.
//
// Under node a session is offline (`Session.nrt`) or on a live server reached
// over a WebSocket (`Session.live`); `Session.embed` needs a page's
// AudioWorklet. In a worker it is also on the engine of the page that started
// it, over the port the page linked to it (`Session.live(port)`). It is the Python client's `clausters.session` module in this
// client's spelling, with what plays through it beside it.
//
// `session.ts` comes first: the environment and the panic close a cycle of
// imports that resolves only when the session module starts it.

export { Session } from "./session.ts";
export type { SessionOptions } from "./session.ts";
export { defaultSession, main } from "./base/main.ts";
export type { Main, SessionLike } from "./base/main.ts";
export { Environment, RandomContext } from "./base/environment.ts";
export { play } from "./play.ts";
export type { Playable, PlayOptions } from "./play.ts";
export { panic } from "./panic.ts";
export { plot, PatchWindow, PlotWindow } from "./plot.ts";
export type { PatchViewOptions, Plottable, PlotOptions } from "./plot.ts";
export { scope, ScopeWindow } from "./scope.ts";
export type { ScopeOptions, ScopeView } from "./scope.ts";
