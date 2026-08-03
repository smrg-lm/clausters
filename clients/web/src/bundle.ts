// Mounting a bundle: one persisted directory becoming N live components.
//
// A bundle's GuiDef record is a **template** — `@symbol` where an id goes,
// `$param` where a value does — so mounting means allocating what its manifest
// declares and resolving the holes. That pass is `clausters_core::bundle`,
// reached here through the core's wasm door, and it is the same one the native
// `--standalone` leg runs: one directory, three legs, one behaviour. The
// format is documented in docs/clients.md.
//
// Mounting is **two phases**, because the host does not need audio and the
// engine does:
//
// 1. `openBundle` — allocate, resolve, attach the canvas, open the GuiDef. The
//    component draws as the reader scrolls to it, with no gesture and no
//    AudioContext.
// 2. `startBundle` — on the page's first gesture: the defs (sent once per def
//    name per page, however many components want them), the samples, and the
//    boot list.
//
// The def payloads carry no holes — that is the invariant the format rests on
// — so two instances of one bundle share the one `/def_send synth` that was sent, and
// only their GuiDef and their boot differ.
//
// `freeBundle` is the way back out of both phases at once, since an instance
// is removed as a whole: what it was allocated goes back to the pools, its
// window and its nodes are freed, and what the page shares — the defs, the
// sample buffers, the engine, the host — stays where it is.

import { loadOsc, decodePacket, encodeMessage } from "./base/osc.ts";
import type { OscArg } from "./base/osc.ts";
import { bundle_requirements, bundle_resolve } from "./core/clausters_core_web.js";
import { loadCore } from "./base/core.ts";
import { pagePools } from "./base/pool.ts";
import type { Pools } from "./base/pool.ts";
import { server } from "./engine/server.ts";
import { guiHost } from "./gui/page.ts";
import { interleave } from "./data/samples.ts";

/** A declared parameter, as `bundle.json` carries it. */
export interface ParamSpec {
    type: "float" | "int" | "string" | "bool";
    default?: unknown;
    min?: number;
    max?: number;
}

/** The manifest at a served bundle's root (`bundle.json`). */
export interface BundleManifest {
    name?: string;
    gui: string;
    synthdefs?: string[];
    graphdefs?: string[];
    /**
     * How many widgets the template holds — the size of the id block a mount
     * allocates. Absent (or 0) in a bundle written before the contract, which
     * mounts verbatim.
     */
    widgets?: number;
    symbols?: {
        nodes?: string[];
        buses?: { name: string; rate?: "audio" | "control"; channels?: number }[];
        buffers?: string[];
    };
    params?: Record<string, ParamSpec>;
    presets?: string[];
    /**
     * Whether the bundle carries a `boot.json` preset. Declared here so the
     * mount never probes for the optional file (a probe's 404 would litter
     * the console); absent means none.
     */
    boot?: boolean;
    /**
     * Buffer symbol -> audio URL relative to the bundle. The symbol is what
     * `@name` resolves to; the mount allocates the index.
     */
    buffers?: Record<string, string>;
}

/**
 * The persisted GuiDef record: `{ "id": <i32>, "gui": <tree> }`, read as the
 * template it is.
 */
interface Template {
    id: number;
    gui: unknown;
}

/** What the resolver hands back for one instance. */
interface Resolved {
    def_id: number;
    tree: unknown;
    boot: unknown[][];
    params: Record<string, unknown>;
}

/** One mounted instance, between its two phases and after them. */
export interface Mounted {
    /** The id its GuiDef opened under — unique per instance. */
    defId: number;
    /** The resolved tree, holes filled. */
    tree: unknown;
    /** The merged parameter values that produced it, typed as declared. */
    params: Record<string, unknown>;
    /**
     * What this instance was allocated, by symbol name: its node ids, its
     * buses, its buffers. Flat because the names share one namespace (the
     * core refuses a name declared twice), and here because a page that wants
     * to talk to *this* instance — an `/node_set`, a bus to watch — needs them.
     */
    symbols: Record<string, number>;
    /** Whether the engine half has been sent (phase 2). */
    started: boolean;
}

// Each boot gets its own /server_sync ids so two components on one page cannot
// mistake each other's /server_sync.reply for their own.
let nextSync = 0xb40;

/**
 * The def payloads already handed to the page's engine, by URL, each as the
 * promise of its send.
 *
 * A def payload holds no holes, so two instances of one bundle send it once —
 * but a **promise**, not a flag: components start concurrently, and a second
 * instance that merely saw the first claim the def could boot before the
 * bytes were on their way. It waits for the send that is already in flight.
 */
const sentDefs = new Map<string, Promise<void>>();

/**
 * The buffer allocated for each sample URL, and which of them are loaded.
 * A sample is identical data wherever it is referenced, so two instances of
 * one bundle resolve their `@symbol` to the **same** buffer and the file is
 * fetched and decoded once.
 */
const bufferIds = new Map<string, number>();
const loadedBuffers = new Set<string>();

async function fetchBytes(url: string): Promise<Uint8Array> {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return new Uint8Array(await response.arrayBuffer());
}

async function fetchJson<T>(url: string): Promise<T> {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
    return (await response.json()) as T;
}

/**
 * What `openBundle` keeps for `startBundle` and `freeBundle` — the engine
 * half, held until a gesture makes an AudioContext legal, and the allocation
 * to give back when the instance goes.
 */
interface Pending {
    base: string;
    manifest: BundleManifest;
    resolved: Resolved;
    buffers: Record<string, number>;
    /** The pools this instance drew from — the ones `freeBundle` returns to. */
    pools: Pools;
    /**
     * Exactly what was taken, so exactly that is given back: the widget block
     * with the width it was allocated at (the def id lives inside it), the
     * node ids, and the buses with the rate and width each was sized by.
     */
    allocated: {
        widgets: { first: number; width: number };
        nodes: number[];
        buses: { first: number; width: number; rate: "audio" | "control" }[];
    };
}

const pending = new WeakMap<Mounted, Pending>();

export interface MountOptions {
    /** The bundle's URL prefix. */
    base: string;
    /**
     * The canvas this instance draws into. Omitted, the page's default one is
     * used — which is right for a page showing a single bundle and wrong for
     * a document showing several.
     */
    canvas?: HTMLCanvasElement;
    /** Overrides the manifest's GuiDef name. */
    name?: string | null;
    /** The values the tag supplies, as they come off an element: strings. */
    attributes?: Record<string, string>;
    /** A named preset from `presets/<name>.json`, under the attributes. */
    preset?: string | null;
    /**
     * The id spaces to allocate from. Defaults to the page's pools; a page
     * also driving the TypeScript client passes that client's allocators so
     * the two cannot overlap.
     */
    pools?: Pools;
}

/**
 * Phase 1: allocate, resolve, and open this instance's GuiDef on the page's
 * host — no audio, no gesture. The component draws immediately.
 */
export async function openBundle(options: MountOptions): Promise<Mounted> {
    const { base, canvas, name = null, attributes = {}, preset = null } = options;
    // The core first: the pools are built on its `Registry`, and the resolver
    // is one of its exports.
    await loadCore();
    await loadOsc();
    const pools = options.pools ?? pagePools();
    const gui = await guiHost();

    const manifest = await fetchJson<BundleManifest>(`${base}/bundle.json`);
    const guiName = name ?? manifest.gui;
    const template = await fetchJson<Template>(`${base}/defs/guidefs/${guiName}.json`);
    // A preset is a named bundle of values under the attributes; an unlisted
    // one is a mistake worth reporting, not a silent fall-through.
    let presetValues: Record<string, unknown> = {};
    if (preset) {
        if (!(manifest.presets ?? []).includes(preset)) {
            throw new Error(`${base}: no preset "${preset}" (declared: ${manifest.presets ?? []})`);
        }
        presetValues = await fetchJson(`${base}/presets/${preset}.json`);
    }

    // What this instance needs, then what the page gave it. The resolver never
    // allocates — that is what keeps it pure and the ids the page's.
    // The template goes along: a bundle written before the contract declares
    // no widget count, and its id block is measured from the ids it uses.
    const requirements = JSON.parse(bundle_requirements(JSON.stringify({ manifest, template }))) as {
        widgets: number;
        nodes: string[];
        buses: { name: string; rate?: string; channels?: number }[];
        buffers: string[];
    };
    const widgets = { first: 0, width: Math.max(requirements.widgets, 1) };
    widgets.first = pools.widgets.alloc(widgets.width);
    const allocated: Pending["allocated"] = { widgets, nodes: [], buses: [] };
    const allocation = {
        widget_base: widgets.first,
        nodes: {} as Record<string, number>,
        buses: {} as Record<string, number>,
        buffers: {} as Record<string, number>,
    };
    for (const node of requirements.nodes) {
        allocation.nodes[node] = pools.nodes.alloc();
        allocated.nodes.push(allocation.nodes[node]);
    }
    for (const bus of requirements.buses) {
        const width = bus.channels ?? 1;
        const rate = bus.rate === "audio" ? "audio" : "control";
        const first =
            rate === "audio" ? pools.audioBuses.alloc(width) : pools.controlBuses.alloc(width);
        allocation.buses[bus.name] = first;
        allocated.buses.push({ first, width, rate });
    }
    for (const symbol of requirements.buffers) {
        // Shared by URL: the same sample is the same buffer, so a second
        // instance points at the one already loaded rather than a second copy.
        const url = `${base}/${(manifest.buffers ?? {})[symbol] ?? symbol}`;
        let bufnum = bufferIds.get(url);
        if (bufnum === undefined) {
            bufnum = pools.buffers.alloc();
            bufferIds.set(url, bufnum);
        }
        allocation.buffers[symbol] = bufnum;
    }

    const resolved = JSON.parse(
        bundle_resolve(
            JSON.stringify({
                manifest,
                template,
                allocation,
                params: { attributes, preset: presetValues },
            }),
        ),
    ) as Resolved;

    // The canvas comes first: the host holds one per def, and it must know
    // where this def draws before the def opens.
    gui.attach(resolved.def_id, canvas);
    gui.bridge.def(resolved.def_id, JSON.stringify(resolved.tree));

    const mounted: Mounted = {
        defId: resolved.def_id,
        tree: resolved.tree,
        params: resolved.params,
        symbols: { ...allocation.nodes, ...allocation.buses, ...allocation.buffers },
        started: false,
    };
    pending.set(mounted, {
        base,
        manifest,
        resolved,
        buffers: allocation.buffers,
        pools,
        allocated,
    });
    return mounted;
}

/**
 * Phase 2: the engine half — the defs, the samples and the boot list. Call it
 * from a user gesture (the AudioContext will not start without one); calling
 * it twice is a no-op.
 */
export async function startBundle(mounted: Mounted): Promise<void> {
    if (mounted.started) return;
    const held = pending.get(mounted);
    if (!held) throw new Error("startBundle: this instance was not opened here");
    // Claimed before the first await: a component's own start and the page's
    // gesture can both reach here, and the defs must go out once.
    mounted.started = true;
    const { base, manifest, resolved, buffers } = held;
    const engine = await server();

    // The defs, once per payload for the whole page: a def payload holds no
    // holes, so two instances share the one that was sent. Every def this
    // instance needs must be **on its way** before its boot goes out — the
    // engine serves in order, so an issued send is enough.
    const wanted: [string, string][] = [
        ...(manifest.synthdefs ?? []).map(
            (n) => ["synth", `${base}/defs/synthdefs/${n}.json`] as [string, string],
        ),
        ...(manifest.graphdefs ?? []).map(
            (n) => ["graph", `${base}/defs/graphdefs/${n}.json`] as [string, string],
        ),
    ];
    await Promise.all(
        wanted.map(([family, url]) => {
            let send = sentDefs.get(url);
            if (!send) {
                send = (async () => {
                    const spec = await fetchBytes(url);
                    engine.send(encodeMessage("/def_send", [["s", family], ["b", spec]]));
                })();
                sentDefs.set(url, send);
            }
            return send;
        }),
    );

    // The samples, loaded before any boot message can play one — once per URL,
    // since phase 1 already pointed every instance at the same buffer.
    for (const [symbol, url] of Object.entries(manifest.buffers ?? {})) {
        const bufnum = buffers[symbol];
        const full = `${base}/${url}`;
        if (bufnum === undefined || loadedBuffers.has(full)) continue;
        loadedBuffers.add(full);
        const bytes = await fetchBytes(full);
        const decoded = await engine.context.decodeAudioData(bytes.buffer as ArrayBuffer);
        await engine.bLoad(bufnum, decoded.numberOfChannels, decoded.sampleRate, interleave(decoded));
    }

    // The same bracket the native data-dir boot gets implicitly: the first
    // /server_sync marks the defs in (loading them is asynchronous on the server),
    // the second — arriving after everything, since the engine serves strictly
    // in order — is this instance's "up" signal.
    const syncId = (nextSync += 2);
    let bootedResolve!: () => void;
    const booted = new Promise<void>((r) => {
        bootedResolve = r;
    });
    const watch = (bytes: Uint8Array) => {
        for (const { addr, args } of decodePacket(bytes)) {
            if (addr === "/server_sync.reply" && args[0] === syncId + 1) bootedResolve();
        }
    };
    engine.addReply(watch);
    try {
        engine.send(encodeMessage("/server_sync", [["i", syncId]]));
        for (const message of resolved.boot) {
            const [addr, ...args] = message as [string, ...unknown[]];
            engine.send(encodeMessage(addr, args.map(oscValue)));
        }
        engine.send(encodeMessage("/server_sync", [["i", syncId + 1]]));
        await Promise.race([
            booted,
            new Promise((_, reject) =>
                setTimeout(() => reject(new Error("bundle mount: no /server_sync.reply from the engine")), 15000),
            ),
        ]);
    } finally {
        engine.removeReply(watch);
    }
}

/**
 * The unmount: give back everything this instance took, and nothing the page
 * shares.
 *
 * What one instance owns is what it was allocated — its widget block (the def
 * id is inside it), its node ids, its buses — plus the canvas the host holds
 * for it. Those go: `/gui_free` closes the window and takes its subtree, its
 * bindings and any voices it was holding down; `/node_free` takes the nodes
 * its boot instantiated; `detach` takes the GPU surface, which also drops the
 * def from the tick and from the `/bus_stream` set. The `<canvas>` element
 * itself belongs to the page, which keeps or removes it.
 *
 * What the *page* owns stays: the AudioContext, the host, and — deliberately —
 * the def payloads and the sample buffers. Both are shared by URL between
 * every instance of a bundle, and both are idempotent data the engine holds
 * once, so freeing them here would be freeing a sibling's; a component mounted
 * again finds them loaded and boots the faster for it.
 *
 * `hostClosed` marks the arrival direction: a window the host closed by itself
 * (a `/gui_closed`) is already gone there, so the `/gui_free` is skipped and
 * only the rest is given back. Calling this twice is a no-op.
 */
export async function freeBundle(
    mounted: Mounted,
    { hostClosed = false }: { hostClosed?: boolean } = {},
): Promise<void> {
    const held = pending.get(mounted);
    if (!held) return;
    pending.delete(mounted);
    const { pools, allocated } = held;

    // The engine half only if it went out: an instance removed before the
    // page's first gesture never instantiated anything, and asking for the
    // engine here would boot an AudioContext to free nothing.
    if (mounted.started && allocated.nodes.length > 0) {
        const engine = await server();
        engine.send(
            encodeMessage(
                "/node_free",
                allocated.nodes.map((id) => ["i", id] as OscArg),
            ),
        );
    }

    const gui = await guiHost();
    if (!hostClosed) gui.bridge.feed(encodeMessage("/gui_free", [["i", mounted.defId]]));
    gui.bridge.detach(mounted.defId);

    pools.widgets.release(allocated.widgets.first, allocated.widgets.width);
    for (const id of allocated.nodes) pools.nodes.release(id);
    for (const bus of allocated.buses) {
        const pool = bus.rate === "audio" ? pools.audioBuses : pools.controlBuses;
        pool.release(bus.first, bus.width);
    }
}

/**
 * One resolved boot argument as a tagged OSC value, keeping the int/float
 * distinction JSON already carries — so a node id stays an integer.
 */
function oscValue(value: unknown): OscArg {
    if (typeof value === "string") return ["s", value];
    if (typeof value === "boolean") return ["i", value ? 1 : 0];
    const n = Number(value);
    return Number.isInteger(n) ? ["i", n] : ["f", n];
}

/**
 * Both phases at once, for a page driving the mount from script after a
 * gesture (a test, a REPL). A component uses the two separately.
 */
export async function bootBundle(options: MountOptions): Promise<{ id: number; tree: unknown }> {
    const mounted = await openBundle(options);
    await startBundle(mounted);
    return { id: mounted.defId, tree: mounted.tree };
}
