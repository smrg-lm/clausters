// The shared core's wasm module: one load, one latch.
//
// Everything the client borrows from `clausters-core` — the OSC codec
// (`base/osc.ts`), the id registry the allocators are built on, and the clock
// arithmetic a later milestone adds — comes out of the same wasm instance, so
// the load belongs here rather than in whichever module happens to need it
// first. `await loadCore()` is idempotent; every core-backed call needs one
// prior await.
//
// In the browser the default locates the `.wasm` next to the glue; under node
// (the test runner) pass the bytes explicitly — node's `fetch` cannot read
// `file://` URLs.

import initCore, {
    Registry,
    graph_bus_reserved,
    node_id_partition,
} from "../core/clausters_core_web.js";

export { Registry };

let loaded: Promise<void> | null = null;

/**
 * Loads the core wasm once (later calls reuse it). `source` overrides the
 * default URL-relative lookup with raw module bytes (the node path).
 */
export function loadCore(source?: BufferSource): Promise<void> {
    loaded ??= initCore(
        source === undefined ? undefined : { module_or_path: source },
    ).then(() => undefined);
    return loaded;
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
    /** First id of the server's auto range (`/s_new -1`, GraphDef members). */
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
