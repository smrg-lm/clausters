/**
 * The format's own numbers, and how a reader keeps what it does not know.
 *
 * Private: the names are `clausters/document`'s.
 *
 * @module
 */

/**
 * The version an unedited document and an unedited multitrack are at.
 *
 * The counter moves when the description changes, so an edit made against a
 * version the document has left behind is refused as stale rather than applied
 * blind.
 */
export const FIRST_VERSION = 1;

/**
 * The session format this build writes -- the crate's `session::FORMAT`.
 *
 * It moves when a reader that does not know the new shape would read a file
 * **wrongly** -- never for an added field, which an older reader ignores and a
 * newer one defaults. **2** added a source whose samples are spans of other
 * sources, which is a tagged variant an older reader must fail on rather than
 * read as something else. **3** measures the multitrack in seconds where 2
 * measured it in beats, and states a tempo in beats per second; an older file is
 * migrated on reading (`SessionFile.read`). **4** added a source that is a sequence
 * of events held in the file itself, a tagged variant for the reason 2 gives.
 * **5** names a sequence's curves as the multitrack names a track's --
 * `automation` where it said `lanes` and `expression` -- which an older reader
 * would drop without a word.
 *
 * A literal rather than a call into the core, deliberately: a `SessionFile` is
 * plain data and opens the core for nothing else, so reading the number from
 * wasm would make writing a session need an `await loadCore()` it has never
 * needed. What keeps the two in step instead is a **test** that asks
 * the crate (`sessionFormat`) and compares -- which is the check that did not
 * exist when this said 1 and the crate had moved to 2.
 */
export const SESSION_FORMAT = 5;

/**
 * **The application a session file belongs to**, as its `metadata` names it --
 * the crate's `session::APPLICATION`. Every application's file is JSON, so the
 * file says whose it is rather than its extension.
 */
export const SESSION_APPLICATION = "multitrack";


/** Whatever a newer writer wrote and this build has no field for. */
export type Extra = Record<string, unknown>;


/** Everything in `written` except the keys this build knows. */
export function unknownFields(written: Extra, ...known: string[]): Extra {
    const out: Extra = {};
    for (const [key, value] of Object.entries(written)) {
        if (!known.includes(key)) out[key] = value;
    }
    return out;
}

export function num(value: unknown, fallback = 0): number {
    return value === undefined || value === null ? fallback : Number(value);
}
