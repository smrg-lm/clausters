// The notebook widget's front end: the cell's end of the kernel comm.
//
// This module is loaded by anywidget as the widget's `_esm`, and it is the one
// browser artifact of the `clausters-jupyter` Python package. Its job is small
// and entirely about *carrying*: announce that the cell's output exists, take
// the package's assets over the comm, boot the wasm GUI host on them, and then
// move OSC packets in both directions. It decides nothing about what to draw.
//
// **Why the assets arrive over the comm.** anywidget serves this module and
// nothing beside it, and a remote kernel (JupyterHub, Colab, VS Code) has no
// static route to add the rest to. So the Python side sends the built `dist/`
// as binary buffers and this module turns them into blob URLs — which is also
// why the imports below are dynamic. wasm-bindgen's `init` takes bytes
// directly, so no `.wasm` is ever fetched from a URL either.
//
// **Why the module text is rewritten.** A module loaded from a blob URL
// resolves its relative imports against the blob's origin, where nothing
// lives, so `clausters_gui.js`'s own imports would fail. Before each blob is
// made, its import specifiers are resolved against the *name* the asset came
// with and swapped for the blob URL of that asset. Blobs are therefore made
// leaf-first, which `assetOrder` fixes.
//
// **Why "ready" is announced rather than awaited.** A cell's output renders
// after the cell's code has run, so by the time this module executes the
// kernel has usually already sent a whole tree. It has kept a journal of it
// (see `clausters_jupyter.journal`); saying "ready" is what asks for the
// replay. The same message is what rebuilds the view after a page reload or a
// moved output, so there is one path, not a special case.

import type { CanvasBox } from "../gui/canvasbox.ts";

// Type-only, and that is load-bearing: anywidget serves this module and
// nothing beside it, so a *value* import of a sibling is a specifier the page
// cannot resolve. Everything this module runs arrives over the comm and is
// imported from a blob URL, the measuring helpers included.

/** The two peers behind one comm, matching `clausters_jupyter.carrier`. */
const GUI = "gui";
const SERVER = "server";

/**
 * A comm buffer as bytes.
 *
 * ipywidgets hands binary buffers to JS as `DataView`, not `ArrayBuffer`, and
 * the two are not interchangeable here: `new Uint8Array(dataView)` takes the
 * array-like path, finds no `length`, and yields an **empty** array — which
 * reaches the host as "bad OSC packet: Empty packet" rather than as a type
 * error. Reading the view's own window is the whole fix.
 */
export function asBytes(buffer: ArrayBuffer | ArrayBufferView): Uint8Array {
    if (ArrayBuffer.isView(buffer)) {
        return new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
    }
    return new Uint8Array(buffer);
}

/** anywidget's model, as much of it as this module uses. */
interface Model {
    get(name: string): unknown;
    send(content: unknown, callbacks?: unknown, buffers?: ArrayBuffer[]): void;
    on(event: string, handler: (...args: never[]) => void): void;
}

/**
 * Models already counted, so two views of one widget are one front end.
 *
 * A model is destroyed when its comm closes and never reopens, so it is the
 * one thing on this side that tracks the kernel's life. Weak, because holding
 * a disposed model alive is exactly the leak this file has fixed twice.
 */
const counted = new WeakSet<Model>();

/**
 * As much of ipywidgets' manager as this reaches, and every field optional on
 * purpose: the same module runs under JupyterLab, VS Code and Colab, whose
 * front ends share the widget protocol and not these internals. Everything
 * below degrades to the idle policy when a field is not there.
 */
interface Manager {
    kernel?: {
        /** Stable across a restart, which is what makes `adopt` possible. */
        id?: string;
        statusChanged?: {
            connect(handler: (sender: unknown, status: string) => void): void;
        };
    } | null;
}

/**
 * Kernel states after which the process that authored this session is gone,
 * split by whether anything can follow it.
 *
 * A restart keeps the notebook, its outputs and its kernel id, so its session
 * is only silenced and left for the successor to collect. `dead` is the end of
 * the line: the kernel was shut down, and a notebook opened against it later
 * gets a *new* kernel with a new id, which would never recognise this one.
 */
const KERNEL_RESTARTING = ["restarting", "autorestarting"];
const KERNEL_DEAD = "dead";

/**
 * Count this model into its session and subscribe to whatever that front end
 * can tell us about its kernel.
 *
 * **The rule: a session's host and engine live exactly as long as its
 * kernel.** The kernel is the notebook's author — it holds the `Session`, the
 * defs, the journal that rebuilds the windows and the id allocators — so while
 * it is alive there is something these belong to, and when it goes there is
 * not. Two signals say so, and both are about the kernel rather than about
 * what is on screen:
 *
 * - a `WidgetModel` triggers ``destroy`` when its comm closes, which is a
 *   kernel shut down cleanly, or a widget closed from Python;
 * - the kernel's own `statusChanged` covers the restart, which closes no comm
 *   at all — the manager marks each model `comm_live = false` by plain
 *   assignment, firing nothing — so without this a restarted notebook's host
 *   would sit on the page forever, its canvases showing a kernel that is gone.
 *
 * The two are not the same event and are not treated alike. A closed comm
 * never reopens, so that session is freed outright. A restart keeps the
 * notebook, its outputs and its kernel *id*, so that one is only silenced and
 * left for its successor to collect (`orphan`, `adopt`) — freeing it while the
 * notebook is still open, about to re-run its cells, would be the page
 * deciding something the notebook is there to decide.
 *
 * **Closing a notebook's tab is deliberately neither.** It does not shut down
 * the kernel (JupyterLab's `kernelShutdown` defaults to false), and reopening
 * the tab reattaches to the same one — which must still have its windows and
 * its sound. The page could not tell that case from an open notebook with its
 * outputs cleared anyway: no comm closes, no manager is disposed (the panel's
 * `disposed` only drops it from a registry), and every observable field reads
 * the same. So the tab is not the unit of lifetime; the kernel is.
 */
function trackModel(session: string, model: Model): void {
    if (counted.has(model)) return;
    counted.add(model);
    const state = shared(session);
    state.models += 1;
    model.on("destroy", (() => {
        const live = shared(session);
        live.models -= 1;
        if (live.models <= 0) scheduleClose(session);
    }) as (...args: never[]) => void);
    // The count above is a *lower* bound on this notebook's front ends, not a
    // census, and it is deliberately not the only way out. Reopening a
    // notebook's tab builds fresh models for the same widgets while the ones
    // from before are never destroyed -- no comm closed, so nothing tells them
    // to -- and the count keeps a straggler forever. `dead` above is what
    // makes a shut-down kernel free its session regardless.

    const kernel = (model as { widget_manager?: Manager }).widget_manager?.kernel;
    try {
        if (typeof kernel?.id === "string") {
            state.kernel = kernel.id;
            // This front end exists, so any earlier session of this kernel is
            // one the notebook has come back from. Free it.
            adopt(session, kernel.id);
        }
        kernel?.statusChanged?.connect((_sender, status) => {
            if (KERNEL_RESTARTING.includes(status)) orphan(session);
            else if (status === KERNEL_DEAD) scheduleClose(session);
        });
    } catch {
        // A front end whose manager is shaped differently: no kernel id, no
        // status. A closed comm still frees this session; a restart under such
        // a front end leaves it silenced until the page is reloaded.
    }
}

/**
 * Arrange to close this session, its kernel having gone.
 *
 * Nothing calls this off, and nothing on screen argues against it: a restarted
 * kernel leaves its canvases mounted, and they are pictures of a notebook that
 * no longer exists.
 */
function scheduleClose(session: string): void {
    const state = shared(session);
    if (state.closing !== null) return;
    silence(session);
    state.closing = setTimeout(() => {
        shared(session).closing = null;
        closeSession(session);
    }, CLOSE_AFTER_SIGNAL);
}

/**
 * Stop this session's sound now, and leave everything else standing.
 *
 * The two halves of a teardown answer different needs and only one of them can
 * wait. A kernel that restarts with a synth running leaves it sounding with
 * nothing left that could stop it — no cell to run, no node id anyone still
 * holds — so the sound goes at the signal. What the session *owns* is another
 * question, and `orphan` is the one that keeps it open.
 */
function silence(session: string): void {
    void shared(session).engine?.then(
        (engine) => engine?.suspend().catch(() => {}));
}

/**
 * The kernel behind this session is gone but may be replaced: silence it, and
 * mark it for whoever comes back.
 *
 * A restart keeps the notebook, its outputs and its kernel *id* — what it
 * replaces is the process. So the session cannot be freed here: freeing is a
 * thing the page does on behalf of a notebook, and this notebook is still
 * open, about to re-run its cells. What it can do is stop making noise and
 * wait to be collected by its successor (`adopt`).
 *
 * The cost of nobody ever coming back is one idle host and one suspended
 * AudioContext until the page is reloaded. That is a notebook left open with
 * a dead kernel, which is a thing the user can see and fix.
 */
function orphan(session: string): void {
    const state = shared(session);
    state.orphaned = true;
    silence(session);
}

/**
 * Free any earlier session of the same kernel, now that this one has a front
 * end of its own.
 *
 * This is "coming back to the notebook to free it": a restarted kernel keeps
 * its id, so the session the re-run cell just built is provably the successor
 * of the one left behind, and nothing will ever reattach to that one again.
 */
function adopt(session: string, kernel: string): void {
    const scope = globalThis as unknown as Record<string, Record<string, Shared>>;
    for (const [other, state] of Object.entries(scope[KEY] ?? {})) {
        if (other !== session && state.orphaned && state.kernel === kernel) {
            closeSession(other);
        }
    }
}

interface RenderContext {
    model: Model;
    el: HTMLElement;
}

/** Every relative specifier a module imports, resolved to asset names. */
export function importsOf(source: string, name: string): string[] {
    const dir = name.includes("/") ? name.slice(0, name.lastIndexOf("/")) : "";
    const found: string[] = [];
    for (const m of source.matchAll(/(?:from|import)\s*\(?\s*(['"])(\.[^'"]+)\1/g)) {
        found.push(resolvePath(dir, m[2]));
    }
    return found;
}

/**
 * The order assets must become blobs in: a module only after everything it
 * imports, since rewriting its specifiers needs those URLs to exist already.
 *
 * A real topological sort over the import graph, not a guess. The guess it
 * replaces ranked by filename — wasm, then the core, then the rest — and it
 * held right up until the engine arrived: `worklet.js` and `worklet-shim.js`
 * ranked equal, the first imports the second, and the stable sort happened to
 * put the importer first. Its specifier was left alone and the audio thread
 * died on "Failed to resolve module specifier ./worklet-shim.js".
 *
 * `sources` holds the decoded text of the JS assets; anything absent from it
 * (the wasm blobs) has no imports and sorts first. A cycle would be a bug in
 * the bundles rather than here, so it is broken arbitrarily rather than
 * reported.
 */
export function assetOrder(names: string[], sources?: Map<string, string>): string[] {
    const seen = new Set<string>();
    const out: string[] = [];
    const visit = (name: string, path: Set<string>) => {
        if (seen.has(name) || path.has(name)) return;
        path.add(name);
        for (const dep of importsOf(sources?.get(name) ?? "", name)) {
            if (names.includes(dep)) visit(dep, path);
        }
        path.delete(name);
        seen.add(name);
        out.push(name);
    };
    for (const name of names) visit(name, new Set());
    return out;
}

/**
 * Rewrite one module's relative import specifiers to the blob URLs of the
 * assets they name. `name` is the asset's own path, which is what the
 * specifiers are relative to.
 */
export function rewriteImports(
    source: string,
    name: string,
    urls: Map<string, string>,
): string {
    const dir = name.includes("/") ? name.slice(0, name.lastIndexOf("/")) : "";
    // The three forms a bundle uses, and they must be exactly the forms
    // `importsOf` counts as a dependency — a specifier one sees and the other
    // misses orders the staging correctly and then fails to rewrite it, which
    // is how a side-effect `import "./worklet-shim.js"` (no `from`) reached
    // the audio thread as a relative URL with a blob for a base.
    return source.replace(
        /((?:\bfrom|\bimport)\s*\(?\s*)(['"])(\.[^'"]+)\2/g,
        (whole, lead: string, quote: string, spec: string) => {
            const resolved = resolvePath(dir, spec);
            const url = urls.get(resolved);
            return url === undefined ? whole : `${lead}${quote}${url}${quote}`;
        },
    );
}

/** POSIX-ish path resolution for the handful of specifiers in the bundles. */
export function resolvePath(dir: string, spec: string): string {
    const parts = dir === "" ? [] : dir.split("/");
    for (const piece of spec.split("/")) {
        if (piece === "." || piece === "") continue;
        if (piece === "..") parts.pop();
        else parts.push(piece);
    }
    return parts.join("/");
}

/**
 * Turn the asset buffers into blob URLs, rewriting each module's imports as it
 * goes. Returns the map from asset name to URL, plus the raw wasm bytes, which
 * the wasm-bindgen `init` functions take directly.
 */
function stageAssets(
    names: string[],
    buffers: (ArrayBuffer | ArrayBufferView)[],
): { urls: Map<string, string>; wasm: Map<string, Uint8Array> } {
    const bytes = new Map<string, Uint8Array>();
    names.forEach((name, i) => bytes.set(name, asBytes(buffers[i])));

    const urls = new Map<string, string>();
    const wasm = new Map<string, Uint8Array>();
    const decoder = new TextDecoder();
    // Decode the modules up front: the order they may be blobbed in is a
    // property of what they import, which cannot be known without reading them.
    const sources = new Map<string, string>();
    for (const [name, buffer] of bytes) {
        if (!name.endsWith(".wasm")) sources.set(name, decoder.decode(buffer));
    }
    for (const name of assetOrder(names, sources)) {
        const buffer = bytes.get(name)!;
        if (name.endsWith(".wasm")) {
            wasm.set(name, buffer);
            // A URL as well as the bytes: the GUI host's init takes bytes, but
            // the engine's loader compiles by streaming a fetch, which refuses
            // anything not served as application/wasm — hence the explicit
            // type, which a Blob is the only way to set here.
            urls.set(name, URL.createObjectURL(new Blob(
                [buffer.slice().buffer as ArrayBuffer],
                { type: "application/wasm" })));
            continue;
        }
        const source = rewriteImports(sources.get(name)!, name, urls);
        const blob = new Blob([source], { type: "text/javascript" });
        urls.set(name, URL.createObjectURL(blob));
    }
    return { urls, wasm };
}

/**
 * Boot the wasm GUI host on staged assets and return the page-side surface.
 *
 * This is the notebook's own small copy of what `gui/page.ts` does for a
 * served page: the same `GuiBridge`, started the same way, but on modules that
 * came over the comm. It stays here rather than being folded into `page.ts`
 * because that function's boot path also starts the in-page engine and appends
 * a canvas to `document.body`, neither of which a cell wants.
 */
async function bootHost(
    urls: Map<string, string>,
    wasm: Map<string, Uint8Array>,
): Promise<Booted> {
    const glue = urls.get("gui-host/clausters_gui.js");
    const binary = wasm.get("gui-host/clausters_gui_bg.wasm");
    const measuring = urls.get("gui/canvasbox.js");
    if (glue === undefined || binary === undefined || measuring === undefined) {
        throw new Error("the GUI host's assets did not arrive");
    }
    const measure = (await import(/* @vite-ignore */ measuring)) as Measuring;
    const mod = (await import(/* @vite-ignore */ glue)) as {
        default: (init: { module_or_path: Uint8Array }) => Promise<unknown>;
        start: () => GuiBridgeLike;
    };
    // The single-object form; passing the bytes bare is the deprecated one.
    await mod.default({ module_or_path: binary });
    const bridge = mod.start();
    console.info("clausters: GUI host up in this page");
    // No canvas here: one is attached per `window`-rooted def, by the widget
    // that draws it (`feed` below).
    return { bridge, drain: () => bridge.poll(), ...measure };
}

/** What `gui/canvasbox.js` provides, once imported from its blob URL. */
interface Measuring {
    canvasBox(element: Element): CanvasBox;
    onScaleChange(apply: () => void): () => void;
}

type Booted = Measuring & {
    bridge: GuiBridgeLike;
    drain: () => Uint8Array | undefined;
};

/** As much of the wasm bridge as this module touches. */
interface GuiBridgeLike {
    feed(packet: Uint8Array): void;
    poll(): Uint8Array | undefined;
    attach(defId: number, canvas: HTMLCanvasElement): void;
    /** Give up this def's canvas: its surface, its GPU slots, its state. */
    detach(defId: number): void;
    /** Give up the whole instance, leaving the page's other hosts alone. */
    close(): void;
    resize(defId: number, width: number, height: number, scale: number): void;
    /** The host's audio-server leg: its outbound, and the replies back in. */
    connect_page(send: (bytes: Uint8Array) => void): void;
    /** The same leg, to a native server's `--ws` port instead. */
    connect_server(url: string): void;
    server_reply(packet: Uint8Array): void;
}

/**
 * The `/gui_def` id in an outbound packet, or `undefined` if it is not one.
 *
 * The offsets have to be walked, not assumed. OSC pads the address and the
 * type-tag string to four bytes each, so where the arguments begin depends on
 * how many tags there are: `,is` pads to 4 and puts the id at 16, while
 * `,isb` -- the same message carrying a blob -- pads to 8 and puts it at 20.
 * Reading a fixed 16 gets the tail of the tag string instead, which is zeros,
 * which is a perfectly plausible-looking def id 0.
 */
export function definedId(packet: Uint8Array): number | undefined {
    return firstIdOf(packet, "/gui_def");
}

/** The id of a `/gui_free`, the packet that closes a window. */
export function freedId(packet: Uint8Array): number | undefined {
    return firstIdOf(packet, "/gui_free");
}

/** The leading int argument of ``address``, or `undefined` for anything else. */
function firstIdOf(packet: Uint8Array, address: string): number | undefined {
    const end = (from: number) => {
        let i = from;
        while (i < packet.length && packet[i] !== 0) i += 1;
        return (i + 4) & ~3;        // past the null, rounded up to four
    };
    const addr = new TextDecoder().decode(packet.subarray(0, end(0) - 1))
        .replace(/\0+$/, "");
    if (addr !== address) return undefined;
    const tags = end(0);
    const args = end(tags);
    if (args + 4 > packet.length) return undefined;
    return new DataView(packet.buffer, packet.byteOffset).getInt32(args, false);
}

/**
 * The one wasm GUI host of **one notebook**, shared by every cell of it.
 *
 * Two scopes are wrong here and the right one is in between.
 *
 * Not the module's: anywidget instantiates the widget's ESM **per widget**, so
 * two plots in two cells get two module scopes, and a module-level singleton
 * silently becomes one host each — 5.4 MB of wasm, a GPU device and another
 * nine megabytes of assets over the comm, per plot.
 *
 * And not the page's either, which is the trap JupyterLab sets: it is a
 * single-page application, so every notebook open in that tab shares one
 * `globalThis` while having a kernel, a `GuiHost` and a `Server` of its own —
 * and those allocate ids from the same base. A page-global host would let the
 * second notebook's `/gui_def 1000` redefine the first one's window, and its
 * `/synth_new 1000` collide with the first one's node. So the state is keyed
 * by **session**, one per `clausters_jupyter.bridge.Bridge`, which is one per
 * kernel: independent id spaces get independent hosts.
 *
 * That is a real host each, not a partitioned share of one. The wasm exports
 * an instance per `start()` — they share the page's one winit event loop and
 * nothing else — so two notebooks may hold the very same widget and node ids
 * without seeing each other, which is the only arrangement that works when the
 * two kernels are separate processes with no channel to agree on a range over.
 *
 * The assets are the exception, and are cached across sessions (`STAGED`):
 * they are the same bytes, they are immutable, and they are the expensive
 * part. A second notebook boots its own host on the first one's blob URLs, and
 * its wasm module is the one already compiled.
 *
 * `outbound` is where drained events go: whichever widget booted the host owns
 * the drain loop, and the packets it reads may belong to any window *of that
 * session*, so they leave through every comm of it. The kernel fans them back
 * into one carrier regardless of which widget carried them up.
 */
interface Shared {
    host: Promise<Booted> | null;
    engine: Promise<Engine | null> | null;
    outbound: Set<(packet: Uint8Array) => void>;
    /** The engine's replies, fanned out the same way and for the same reason. */
    replies: Set<(packet: Uint8Array) => void>;
    /** Mounted canvases, counted so the audio can follow them (`heard`). */
    views: number;
    /**
     * Packets for an engine that is still booting, in order.
     *
     * The GUI channel has had this since the beginning (`pending`, per view)
     * and the server channel had nothing: a packet arriving before
     * `engine` was assigned went to `undefined?.then(...)` and was dropped
     * without a trace. That window is not an edge case, it is the opening of
     * every notebook -- the cell that displays a window is usually the cell
     * that sent the def and the `/synth_new`, and the wasm takes about a
     * second to come up behind it. What it looked like: a notebook that drew
     * correctly and made no sound, with nothing logged at either end.
     *
     * Drained once, when the engine resolves; after that `engine` is non-null
     * and a send goes straight out. Order is kept either way -- a callback
     * registered on the resolved promise runs after this drain, which was
     * registered first.
     */
    toEngine: Uint8Array[];
    /** Live models of this session, so the last one out can close it. */
    models: number;
    /** The drain loop's timer, so closing the session can stop it. */
    drain: ReturnType<typeof setInterval> | null;
    /** A teardown already scheduled, so a flurry of events costs only one. */
    closing: ReturnType<typeof setTimeout> | null;
    /** The kernel this session belongs to, which survives that kernel's
     *  restart and is therefore how a successor recognises it (`adopt`). */
    kernel: string | null;
    /** Whether its kernel is gone: silenced, and waiting to be collected. */
    orphaned: boolean;
}

const KEY = "__clausters_notebook__";
const ASSET_KEY = "__clausters_notebook_assets__";

/** The staged assets of one build, shared by every session on this page. */
interface Staged {
    digest: string;
    urls: Map<string, string>;
    wasm: Map<string, Uint8Array>;
}

/** This session's shared state, made on first ask. */
function shared(session: string): Shared {
    const scope = globalThis as unknown as Record<string, Record<string, Shared>>;
    scope[KEY] ??= {};
    scope[KEY][session] ??= {
        host: null, engine: null, outbound: new Set(), replies: new Set(),
        views: 0, toEngine: [], models: 0, drain: null, closing: null,
        kernel: null, orphaned: false,
    };
    return scope[KEY][session];
}

/**
 * The debounce before a session whose comm has closed is freed. Short: a closed
 * comm never reopens, so there is nothing to be careful about — only enough
 * that a flurry of closing models costs one teardown rather than several.
 */
const CLOSE_AFTER_SIGNAL = 5_000;

/**
 * Close a session: its wasm host, its AudioContext, its drain loop, its entry.
 *
 * The counterpart of `bootShared`, and the reason it can exist at all is that
 * a host is an instance rather than the page's one thing — closing this one
 * leaves every other notebook in the tab drawing and sounding.
 *
 * Without it a JupyterLab tab accumulates every notebook ever opened in it:
 * a wasm module, a GPU device and an AudioContext each, of which a browser
 * allows about six. What that looked like was the *next* notebook failing to
 * boot, which reads as a bug in the notebook that did nothing wrong.
 */
function closeSession(session: string): void {
    const scope = globalThis as unknown as Record<string, Record<string, Shared>>;
    const state = scope[KEY]?.[session];
    if (state === undefined) return;
    delete scope[KEY][session];
    if (state.drain !== null) clearInterval(state.drain);
    if (state.closing !== null) clearTimeout(state.closing);
    state.outbound.clear();
    state.replies.clear();
    state.toEngine.length = 0;
    void state.host?.then((booted) => booted.bridge.close());
    // The context, not just a suspend: what a browser caps is how many exist,
    // so one left suspended is one the next notebook cannot have.
    void state.engine?.then((engine) => engine?.context.close().catch(() => {}));
}

/** The sessions this page is holding, for the acceptance to look at. */
export function liveSessions(): string[] {
    const scope = globalThis as unknown as Record<string, Record<string, Shared>>;
    return Object.keys(scope[KEY] ?? {});
}

/**
 * The assets already on this page, or `null`.
 *
 * Keyed by a digest of the bytes rather than by a version, so a rebuilt `dist/`
 * in a source checkout is never served from the cache of the notebook opened
 * before it — the case a version string gets wrong exactly when it matters.
 */
function staged(): Staged | null {
    const scope = globalThis as unknown as Record<string, Staged | undefined>;
    return scope[ASSET_KEY] ?? null;
}

function setStaged(value: Staged): void {
    (globalThis as unknown as Record<string, Staged>)[ASSET_KEY] = value;
}

/**
 * Suspend the audio when nothing of this notebook is on screen, resume it when
 * something is again.
 *
 * The engine lives on the page, not in a widget and not in the kernel, so
 * nothing about closing a notebook reaches it: the tab is still open, the
 * AudioContext is still running, and a synth started an hour ago keeps
 * sounding with no visible source. Counting mounted views is what ties the two
 * together — close the notebook and it goes quiet, reopen it and it comes
 * back, because the engine still holds what was playing.
 *
 * Deferred by a beat, because re-running a cell unmounts and remounts: without
 * that, every edit would drop the audio for a frame.
 */
function heard(session: string, delta: number): void {
    const state = shared(session);
    state.views += delta;
    setTimeout(() => {
        void state.engine?.then((engine) => {
            if (engine === null) return;
            // Both reject on a context that `closeSession` has already
            // closed, which is a race this does not need to win: a session
            // being torn down has nothing left to suspend.
            const settled = state.views > 0 ? engine.resume() : engine.suspend();
            void settled.catch(() => {});
        });
    }, 100);
}

function bootShared(
    session: string,
    urls: Map<string, string>,
    wasm: Map<string, Uint8Array>,
    serverUrl: string,
): Promise<Booted> {
    const state = shared(session);
    state.host ??= bootHost(urls, wasm).then((booted) => {
        // The host has one audio leg and each backend gives it one server. The
        // in-page engine is wired to it directly (`bootEngine`); a native
        // server is reached over its `--ws` port, opened from the browser and
        // therefore local-only. Either way the kernel is not in the path: a
        // bound widget's value reaches the audio at frame rate, as it does on
        // a served page and on the desktop.
        if (serverUrl !== "") booted.bridge.connect_server(serverUrl);
        // The engine, if this backend wants one: the assets say so by being
        // there, so the native backend pays nothing for a leg it does not use.
        state.engine ??= bootEngine(session, urls, booted.bridge).then((engine) => {
            if (engine !== null) resumeOnGesture(engine);
            // Whatever the kernel sent while this was coming up. Dropped
            // rather than held when there is no engine (the native backend
            // sends nothing here, but a queue that only grows is worse than
            // one that empties).
            const queued = state.toEngine.splice(0);
            if (engine !== null) for (const packet of queued) engine.send(packet);
            return engine;
        });
        state.drain = setInterval(() => {
            let packet: Uint8Array | undefined;
            while ((packet = booted.drain()) !== undefined) {
                for (const send of [...state.outbound]) send(packet);
            }
        }, 33);
        return booted;
    });
    return state.host;
}

/** The in-page engine, once per page, beside the host. */
interface Engine {
    send(bytes: Uint8Array): void;
    onReply: ((packet: Uint8Array) => void) | null;
    resume(): Promise<void>;
    suspend(): Promise<void>;
    /** The engine's own AudioContext — what `closeSession` actually releases. */
    context: AudioContext;
}

/**
 * Boot the wasm audio engine in an AudioWorklet, on staged assets.
 *
 * The loader already takes the two URLs it needs, so nothing here reaches
 * around it — the worklet module and the wasm are blobs like everything else.
 * The worklet's own imports were rewritten before it became one, which is why
 * the shim and the glue are staged alongside it.
 *
 * **The engine and the host are wired to each other, not through Python.** A
 * bound widget's value reaches the engine inside the page, at frame rate, the
 * same way it does on a served page and on the desktop; the kernel is an
 * author, never a relay.
 */
async function bootEngine(
    session: string,
    urls: Map<string, string>,
    bridge: GuiBridgeLike,
): Promise<Engine | null> {
    const loaderUrl = urls.get("engine/loader.js");
    const wasmUrl = urls.get("engine/clausters_web_bg.wasm");
    const workletUrl = urls.get("engine/worklet.js");
    if (!loaderUrl || !wasmUrl || !workletUrl) {
        // Absent by design under the native backend, which sends the GUI
        // assets alone -- so this is not an error, but it *is* the difference
        // between a notebook that sounds and one that does not, and until now
        // the two were indistinguishable from the page. Anything the kernel
        // sends on the server channel from here on goes nowhere.
        console.info(
            "clausters: no in-page engine (the assets for one did not arrive). "
            + "Expected under backend='native', where the audio comes out of "
            + "the kernel's machine; under backend='page' it means the build "
            + "is incomplete - re-run scripts/refresh-web.sh.");
        return null;
    }
    const mod = (await import(/* @vite-ignore */ loaderUrl)) as {
        bootClausters(options: {
            wasmUrl: string;
            workletUrl: string;
        }): Promise<Engine>;
    };
    const engine = await mod.bootClausters({ wasmUrl, workletUrl });
    // Both booted lines are here for the same reason the "no engine" one
    // below is: a page that loads nine megabytes of wasm and starts an audio
    // thread should say so once. Silence at this point is indistinguishable
    // from a notebook that never got a cell to run in, and the two want very
    // different things done about them.
    console.info(
        `clausters: engine up at ${engine.context.sampleRate} Hz, `
        + `AudioContext ${engine.context.state} (a browser starts no audio `
        + "until the page is clicked)");
    // Set **once**, here, and fanned out to whoever is listening. Chaining a
    // new onReply per mounted view instead would grow a linked list one link
    // per cell re-run, send the kernel one copy of every reply per link, and
    // hold every dead model alive behind it.
    engine.onReply = (bytes) => {
        bridge.server_reply(bytes);
        for (const send of [...shared(session).replies]) send(bytes);
    };
    bridge.connect_page((bytes: Uint8Array) => engine.send(bytes));
    return engine;
}

/**
 * Resume the audio context on the first gesture anywhere in the document.
 *
 * A browser will not start audio without one, and a notebook offers no obvious
 * place to put a "start audio" button — so any click, key or touch does it,
 * once. Until then the engine runs silently, which is also what it does with
 * nothing playing, so nothing looks broken while the page waits.
 */
function resumeOnGesture(engine: Engine): void {
    const go = () => {
        void engine.resume();
        for (const kind of ["pointerdown", "keydown", "touchstart"]) {
            document.removeEventListener(kind, go, true);
        }
    };
    for (const kind of ["pointerdown", "keydown", "touchstart"]) {
        document.addEventListener(kind, go, true);
    }
}

export default {
    async render({ model, el }: RenderContext) {
        const canvas = document.createElement("canvas");
        canvas.style.display = "block";
        canvas.style.width = "100%";
        // A CSS height too, or the element's box is zero high -- a canvas has
        // no intrinsic size, so measuring it before this yields the 1x1
        // surface the host then draws nothing into.
        canvas.style.height = `${model.get("height") as number}px`;
        el.append(canvas);

        // Which notebook this cell belongs to. One per kernel, so the wasm
        // host and the engine below are this notebook's and not the Lab tab's.
        const session = model.get("session") as string;
        // Before anything else: this front end is what keeps the session's
        // host and engine on the page, and its death is what releases them.
        trackModel(session, model);

        let host: Booted | null = null;
        const pending: Uint8Array[] = [];
        const attached = new Set<number>();

        // This view's two ways up, held by identity so the cleanup can take
        // them off again. A view that subscribes and never unsubscribes is
        // not a small leak: the drain loop then sends the kernel one copy of
        // every event per cell re-run, at thirty a second while a knob is
        // moving, through models that were disposed hours ago.
        const up = {
            gui: (packet: Uint8Array) => model.send({ ch: GUI }, undefined, [
                packet.slice().buffer as ArrayBuffer,
            ]),
            server: (packet: Uint8Array) => model.send({ ch: SERVER }, undefined, [
                packet.slice().buffer as ArrayBuffer,
            ]),
        };

        /**
         * Boot (or join) this session's host on staged assets.
         *
         * Another notebook already holding a host in this tab is not a
         * condition to check for: this one boots an instance of its own beside
         * it, sharing the page's event loop and none of its ids.
         */
        const boot = (urls: Map<string, string>, wasm: Map<string, Uint8Array>) => {
            void bootShared(session, urls, wasm, model.get("server_url") as string)
                .then(ready);
        };

        // The same rule a served page follows (`gui/page.ts`'s `fit`): the
        // backing store follows the element in device pixels and the host is
        // told on every change -- of the box, and of the display scale, which
        // move independently and so need two triggers.
        // Both observers are armed at mount but measure nothing until the
        // host is up: the helpers that do the measuring arrive with it.
        const fit = () => {
            if (host === null) return;
            const { width, height, scale } = host.canvasBox(canvas);
            canvas.width = width;
            canvas.height = height;
            for (const id of attached) host.bridge.resize(id, width, height, scale);
        };
        new ResizeObserver(fit).observe(canvas);

        const feed = (packet: Uint8Array) => {
            const id = definedId(packet);
            if (id !== undefined && !attached.has(id)) {
                host!.bridge.attach(id, canvas);
                attached.add(id);
                const { width, height, scale } = host!.canvasBox(canvas);
                host!.bridge.resize(id, width, height, scale);
            }
            host!.bridge.feed(packet);
            // Closing a window empties the cell that was showing it. Freeing
            // the def alone would leave the canvas behind holding its last
            // frame -- a picture of a window that no longer exists, which is
            // what made `win.close()` look like it did nothing.
            const freed = freedId(packet);
            if (freed !== undefined && attached.delete(freed)) {
                host!.bridge.detach(freed);
                canvas.remove();
            }
        };

        model.on("msg:custom", (
            content: { ch?: string; names?: string[]; digest?: string },
            buffers: (ArrayBuffer | ArrayBufferView)[],
        ) => {
            if (content.ch === "assets") {
                const { urls, wasm } = stageAssets(content.names ?? [], buffers);
                setStaged({ digest: content.digest ?? "", urls, wasm });
                void boot(urls, wasm);
                return;
            }
            if (content.ch === GUI) {
                for (const buffer of buffers) {
                    const packet = asBytes(buffer);
                    if (host === null) pending.push(packet);
                    else feed(packet);
                }
            }
            if (content.ch === SERVER) {
                const state = shared(session);
                for (const buffer of buffers) {
                    const packet = asBytes(buffer);
                    // `engine` is null until the boot assigns it, and the
                    // kernel does not wait -- see `Shared.toEngine`.
                    if (state.engine === null) state.toEngine.push(packet);
                    else void state.engine.then((engine) => engine?.send(packet));
                }
            }
        });

        const ready = (booted: Booted) => {
            host = booted;
            booted.onScaleChange(fit);
            fit();
            const state = shared(session);
            state.outbound.add(up.gui);
            // The engine answers the kernel too (a /server_sync's /synced, a
            // query's reply). Its own onReply already feeds the host's server
            // leg; this is the kernel's copy, and both ends want them.
            state.replies.add(up.server);
            for (const packet of pending.splice(0)) feed(packet);
        };

        // The kernel has already sent everything this cell drew: "ready" asks
        // for the replay. `have` is the digest of the assets this *page* has
        // staged, if any: the kernel sends the nine megabytes only when they
        // are not the ones it would send. A second notebook in the same Lab
        // tab therefore boots its own host on the first one's blob URLs.
        const already = shared(session).host;
        if (already !== null) {
            void already.then(ready);
            model.send({ ch: "ready", have: staged()?.digest ?? null });
        } else {
            const cached = staged();
            if (cached !== null) void boot(cached.urls, cached.wasm);
            model.send({ ch: "ready", have: cached?.digest ?? null });
        }

        heard(session, +1);
        // anywidget calls this when the view goes: the cell was re-run, its
        // output cleared, or the notebook closed. The last one is why the
        // audio is counted here at all.
        return () => {
            heard(session, -1);
            const state = shared(session);
            state.outbound.delete(up.gui);
            state.replies.delete(up.server);
            for (const id of attached) host?.bridge.detach(id);
        };
    },
};
