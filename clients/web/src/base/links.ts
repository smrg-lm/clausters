// The address a session takes when none is named, where the session runs in a
// worker someone else started: the links that program left for it.
//
// The Python client's `Session.live()` and `session.gui()` take theirs from the
// configuration file (`[client].host`, `port`), and with none the local
// server's. A worker has neither a file nor a server of its own: the page that
// started it holds the engine and the GUI host, and reaches them to the worker
// as links (`ClaustersServer.link()`, `ClaustersGui.link()`). A program that
// runs code of someone else's in a worker -- clausters-editor's page -- leaves
// those links at `globalThis[Symbol.for("clausters.links")]` before the code
// runs, `{ engine, gui }`, and the code then writes `Session.live()` and
// `session.gui()` as a script does.

/** The links a worker's starter left for it, as `{ engine, gui }`. */
export interface Links {
    engine?: MessagePort;
    gui?: MessagePort;
}

const KEY = Symbol.for("clausters.links");

/** The links left in this realm, or null where there are none. */
export function defaultLinks(): Links | null {
    const found = (globalThis as Record<symbol, unknown>)[KEY];
    return found !== null && typeof found === "object" ? (found as Links) : null;
}
