// Library-specific error types (mirrors `clausters/errors.py`).
//
// A small hierarchy so callers catch *what* went wrong instead of matching on
// message text. Everything derives from `ClaustersError`, so a broad
// `catch (e) { if (e instanceof ClaustersError) … }` still holds.

/** Base class for every error this library throws on purpose. */
export class ClaustersError extends Error {
    constructor(message: string) {
        super(message);
        this.name = new.target.name;
    }
}

/** A server command was answered with a `/fail` reply. */
export class CommandError extends ClaustersError {}

/** No reply arrived within the timeout. */
export class ReplyTimeout extends ClaustersError {}

/**
 * A finite server resource (node ids, buses, buffers) is exhausted, or a
 * handle was released twice — the registry refuses either rather than
 * handing out an id that may still be alive.
 */
export class AllocationError extends ClaustersError {}
