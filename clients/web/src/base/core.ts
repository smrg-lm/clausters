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
 * The `[audio, control]` bus widths GraphDef instances reserve at the top of
 * each bus space (before clamping to a smaller configured count). Requires a
 * prior `loadCore()`.
 */
export function graphBusReserved(): [number, number] {
    const [audio, control] = graph_bus_reserved();
    return [audio, control];
}
