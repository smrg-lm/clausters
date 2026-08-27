// The shared core's wasm module: one load, one latch.
//
// Everything the client borrows from `clausters-core` — the OSC codec
// (`base/osc.ts`), the id registry the allocators are built on, and the clock
// arithmetic a later milestone adds — comes out of the same wasm instance, so
// the load belongs here rather than in whichever module happens to need it
// first. `await loadCore()` is idempotent; every core-backed call needs one
// prior await.
//
// In the browser the default locates the `.wasm` next to the glue. Under node
// the same call reads it off disk instead (node's `fetch` does not open a
// `file://` URL), which is what makes the client a **node target** and not
// only a page's: a script gets the core by awaiting `loadCore()`, the way a
// page does. Passing the bytes explicitly still works and is what a caller
// with its own copy — a bundler's asset, a test — does.

import initCore, {
    Registry,
    graph_bus_reserved,
    midiWriteClip,
    midiWriteSmf,
    node_id_partition,
} from "../core/clausters_core_web.js";

export { Registry };

// The MIDI file writers (`clausters-midi`, through the core's door). Straight
// re-exports: they take and return flat bytes, so there is nothing to convert
// at the boundary -- and a page writing a `.mid` writes the same bytes the
// Python client does, which is the whole reason they are not a TS function.
export { midiWriteClip, midiWriteSmf };

let loaded: Promise<void> | null = null;

/**
 * Loads the core wasm once (later calls reuse it). `source` overrides the
 * lookup with raw module bytes, for a caller that already holds them.
 *
 * With no argument the module is found where the environment keeps it: next to
 * the glue in a browser, and on disk under node.
 */
export function loadCore(source?: BufferSource): Promise<void> {
    loaded ??= (source === undefined ? initHere() : initCore({ module_or_path: source })).then(
        () => undefined,
    );
    return loaded;
}

/** Whether this is node rather than a browser (a real `process.versions.node`). */
function underNode(): boolean {
    const proc = (globalThis as { process?: { versions?: { node?: string } } }).process;
    return typeof proc?.versions?.node === "string";
}

/**
 * The wasm as this environment holds it: the glue's own URL-relative fetch in
 * a browser, the file itself under node.
 *
 * The two candidate paths are the two layouts a node script imports this
 * module through — the emitted package (`dist/base/core.js`, the wasm beside
 * it under `dist/core/`) and the sources it was emitted from
 * (`src/base/core.ts`, where the wasm is still only in `dist/`, since
 * `build.sh` stages the glue into `src/` and not the module). Neither is a
 * guess about an install: both are this package, read from where its own build
 * puts things.
 */
async function initHere(): Promise<unknown> {
    if (!underNode()) return initCore();
    const { readFile } = await import("node:fs/promises");
    const candidates = [
        new URL("../core/clausters_core_web_bg.wasm", import.meta.url),
        new URL("../../dist/core/clausters_core_web_bg.wasm", import.meta.url),
    ];
    for (const url of candidates) {
        try {
            return await initCore({ module_or_path: await readFile(url) });
        } catch (error) {
            if ((error as { code?: string }).code !== "ENOENT") throw error;
        }
    }
    throw new Error(
        "loadCore: the core wasm is not staged — run clients/web/build.sh " +
            `(looked in ${candidates.map((u) => u.pathname).join(", ")})`,
    );
}

/**
 * The boot-derived partition of the node-id space, scaled from the engine's
 * node-table capacity — the same formula the server applies, so a client's
 * registry and the server's table agree by construction.
 */
export interface NodeIdPartition {
    /** First id a client's registry hands out. */
    clientBase: number;
    /** Client id-space size (node-table capacity with in-flight margin). */
    clientCapacity: number;
    /** First id of the server's auto range (`/synth_new -1`, GraphDef members). */
    autoBase: number;
    autoCapacity: number;
    /** First id of the server's MIDI-voice range. */
    midiBase: number;
    midiCapacity: number;
}

/**
 * The node-id partition for a node table of `maxNodes` slots. Requires a
 * prior `loadCore()`.
 */
export function nodeIdPartition(maxNodes: number): NodeIdPartition {
    return node_id_partition(maxNodes) as NodeIdPartition;
}

/**
 * Which slice of a client-side id space this client takes, when **more than
 * one client shares one server** — mirrors the Python client's `IdShare`.
 *
 * The server partitions node ids into a client range, its own auto range and
 * its MIDI range, and every client allocates from that one client range. That
 * is exact while a server has one client and a fiction the moment it has two:
 * both registries start at the same base, hand out the same first id, and the
 * second `/synth_new` of the pair is refused as a duplicate — or, worse,
 * accepted against the other client's node.
 *
 * Two clients that cannot talk to each other can still agree here, because
 * there is nothing to negotiate: the shares are equal slices of the range in a
 * fixed order, so `{index: 0, of: 2}` and `{index: 1, of: 2}` are disjoint by
 * arithmetic. Whoever arranges the two — a driving client and its page, a
 * host embedding a second client — hands each its own index.
 *
 * It costs range, not capability: a share of two halves what either client may
 * hold live at once, which is why the default everywhere is the whole space
 * (`{index: 0, of: 1}`).
 */
export interface IdShare {
    /** This client's slice, from `0` to `of - 1`. */
    index: number;
    /** How many clients the space is split between. */
    of: number;
}

/** The whole space: what a server's only client takes. */
export const WHOLE_SHARE: IdShare = { index: 0, of: 1 };

/**
 * The `[base, span]` of `share` within a range of `span` ids at `base`.
 *
 * The last share takes the remainder, so the slices tile the range exactly
 * rather than leaving a few ids nobody may allocate. A share of a range too
 * small to split yields an empty span, and an empty registry reports
 * exhaustion from its first call — a client that cannot allocate says so,
 * which is the failure this whole mechanism exists to make loud.
 */
export function shareOf(
    base: number,
    span: number,
    share: IdShare = WHOLE_SHARE,
): [number, number] {
    const { index, of } = share;
    if (!Number.isInteger(of) || of < 1) {
        throw new RangeError(`an id share is split between 1 or more clients, not ${of}`);
    }
    if (!Number.isInteger(index) || index < 0 || index >= of) {
        throw new RangeError(`id share ${index} is outside a split of ${of}`);
    }
    const each = Math.floor(span / of);
    const last = index === of - 1;
    return [base + index * each, last ? span - index * each : each];
}

/**
 * The `[audio, control]` bus widths GraphDef instances reserve at the top of
 * each bus space (before clamping to a smaller configured count). Requires a
 * prior `loadCore()`.
 */
export function graphBusReserved(): [number, number] {
    const [audio, control] = graph_bus_reserved();
    return [audio, control];
}
