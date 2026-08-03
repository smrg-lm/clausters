// The wire a resource handle talks over: which server (mirrors
// `clausters/defs/_wire.py`; the leading underscore is Python's privacy
// marker and means nothing in a package whose surface is its `exports` map).
//
// A handle built by a constructor (`new Synth`, `new Group`, `Bus.audio`,
// `Buffer.alloc`) carries the server it was created on; one built from a
// reported id (a responder, the GUI, a tree query) may carry none, and falls
// back to the ambient server — the same rule the free `play` follows.

import { main } from "../base/main.ts";
import type { Server } from "./server/index.ts";

/**
 * `server` if given, else the ambient one: the running routine's session,
 * else the session active on this page, else the default session's. Throws a
 * message naming the two ways to open one when none has been.
 */
export function resolveServer(server?: Server | null): Server {
    return main.resolveServer(server);
}
