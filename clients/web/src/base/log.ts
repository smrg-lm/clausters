/**
 * What the client did, said out loud — the debugging door for the whole
 * package.
 *
 * A client session fails in ways nothing else can see. The library is one half
 * of a conversation: it sends OSC to an audio server and to a GUI host, both of
 * which answer on their own schedule and neither of which is inside the page. So
 * when something does not happen — a note that never sounds, a widget that stays
 * where the hand left it, a def the server never got — the only question worth
 * asking is *what actually went out and what came back*, and nothing in a stack
 * trace says.
 *
 * The areas, each a logger under `clausters` so a caller can raise or silence
 * one without the others:
 *
 * | Logger | What it says |
 * |---|---|
 * | `clausters.server` | every message and bundle sent to the audio server, and every reply |
 * | `clausters.gui` | every command sent to the GUI host, and every event from it |
 * | `clausters.gui.editing` | the editing path's five joints |
 *
 * **Silent unless asked**, and that is not politeness: these are hot paths. A
 * library that formatted every line would make every importer pay for it, so
 * nothing is formatted until somebody asks — {@link Area.debug} takes its values
 * as arguments rather than a built string, so a silent area costs one boolean.
 *
 * Two doors, and they mean different things. {@link watch} is the one a page
 * calls; the **environment** is for the case this exists for — a person running
 * an example, watching a window, about to do the thing that breaks. It is read
 * from `process.env.CLAUSTERS_LOG` under node and, in a browser, from
 * `globalThis.CLAUSTERS_LOG` or a `?clausters-log=` in the page's own URL. All
 * three take an area or a list of them (`1` for everything, `gui,server` for
 * two, `gui.editing` for one).
 *
 * This is the Python client's `clausters.log`, area for area and door for door.
 * What differs is only that Python has `logging` in its standard library and a
 * page has none, so the levels are the one this package uses (`debug`) and the
 * one a loop reports a broken peer with (`warning`).
 *
 * @module
 */

/** Where a watched area's lines go. `console` is the default. */
export interface LogSink {
    debug(message: string, ...values: unknown[]): void;
    warn(message: string, ...values: unknown[]): void;
}

/** The environment variable, and the browser's two spellings of it. */
export const ENV = "CLAUSTERS_LOG";

/** Areas that are printing, and where to. Empty until something arms one. */
const armed = new Map<string, LogSink>();

/**
 * Whether `area` or an ancestor of it is armed, and the sink to use.
 *
 * An ancestor answers for its children the way a record propagates up a Python
 * logger: arming `gui` prints `gui.editing` too, and arming both must not print
 * a line twice — a doubled log is one a reader stops trusting.
 */
function sinkFor(area: string): LogSink | null {
    let walking = area;
    for (;;) {
        const found = armed.get(walking);
        if (found !== undefined) return found;
        if (walking === "") return null;
        const cut = walking.lastIndexOf(".");
        walking = cut < 0 ? "" : walking.slice(0, cut);
    }
}

/**
 * One area of the log, as the thing a module keeps.
 *
 * Built once at module level (`const log = area("gui")`) and asked on every
 * call, so arming an area after the module loaded still reaches it.
 */
export class Area {
    readonly name: string;

    constructor(name: string) {
        this.name = name;
    }

    /** Whether anything would be printed — for a caller whose *values* cost
     * something to work out. An ordinary call needs no guard. */
    get watched(): boolean {
        return sinkFor(this.name) !== null;
    }

    debug(message: string, ...values: unknown[]): void {
        sinkFor(this.name)?.debug(`clausters.${this.name}: ${message}`, ...values);
    }

    warning(message: string, ...values: unknown[]): void {
        sinkFor(this.name)?.warn(`clausters.${this.name}: ${message}`, ...values);
    }

    /** A child area — `area("gui").child("editing")` is `clausters.gui.editing`. */
    child(name: string): Area {
        return new Area(`${this.name}.${name}`);
    }
}

/** The area called `name` (relative to `clausters`; `""` is the root). */
export function area(name = ""): Area {
    return new Area(name);
}

/**
 * Print one area of the client's log to `sink` (the console by default), and
 * answer the area it armed.
 *
 * `name` is relative to `clausters` — `""` for everything, `"gui"`, `"server"`,
 * `"gui.editing"`. Idempotent per area.
 */
export function watch(name = "", sink: LogSink = console): Area {
    armed.set(name, sink);
    return new Area(name);
}

/** Stop printing one area. `unwatch()` silences everything. */
export function unwatch(name?: string): void {
    if (name === undefined) armed.clear();
    else armed.delete(name);
}

/** What the environment asks for, or an empty list. */
function asked(): string {
    const fromNode = (globalThis as { process?: { env?: Record<string, string | undefined> } })
        .process?.env?.[ENV];
    if (fromNode !== undefined) return fromNode;
    const fromGlobal = (globalThis as Record<string, unknown>)[ENV];
    if (typeof fromGlobal === "string") return fromGlobal;
    const url = (globalThis as { location?: { search?: string } }).location?.search;
    if (typeof url === "string") {
        return new URLSearchParams(url).get("clausters-log") ?? "";
    }
    return "";
}

/** Arm what the environment asks for, if it asks for anything. */
function armFromEnvironment(): void {
    const wanted = asked().trim();
    const lowered = wanted.toLowerCase();
    if (lowered === "" || lowered === "0" || lowered === "false" || lowered === "no") return;
    if (lowered === "1" || lowered === "true" || lowered === "yes" || lowered === "all") {
        watch();
        return;
    }
    // Shortest first, so an area that contains another arms the ancestor and the
    // narrower one finds it already watched instead of doubling it.
    const areas = [
        ...new Set(
            wanted.split(",").map((a) => a.trim().replace(/^clausters\./, "")).filter(Boolean),
        ),
    ].sort((a, b) => a.length - b.length);
    for (const one of areas) if (sinkFor(one) === null) watch(one);
}

armFromEnvironment();
