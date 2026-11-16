/**
 * What the client did, said out loud (mirrors `clausters/log.py`): the
 * environment variable that arms an area, and the two calls a program makes
 * to arm one and to silence it. The areas and how they print are
 * `base/log.ts`.
 *
 * @module
 */

export { ENV, unwatch, watch } from "./base/log.ts";
export type { Area, LogSink } from "./base/log.ts";
