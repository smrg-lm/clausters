// The notebook widget's front end: the cell's end of the kernel comm.
//
// This module is loaded by anywidget as the widget's `_esm`, and it is the one
// browser artifact of the `clausters-jupyter` Python package. Its job is small
// and entirely about *carrying*: announce that the cell's output exists, take
// the package's assets over the comm, open a `Session` on them, and then move
// OSC packets in both directions. It decides nothing about what to draw.
//
// **It is a client of this package, not a copy of it.** The client arrives
// with the assets (`./client.ts`, bundled), so the host comes up through
// `newGuiHost`,
// the engine through `engine()`, and what owns the pair per notebook is a
// `Session` — the same handle a served page holds. This module supplies only
// what is genuinely the notebook's: which canvas a window draws into, and the
// comm the kernel authors over. Everything it used to do by hand beside that
// (booting the wasm, wiring the audio leg, tearing the two down in the right
// order, reading an id out of a packet by counting bytes) is the client's, and
// is done here by calling it.
//
// **Why the assets arrive over the comm.** anywidget serves this module and
// nothing beside it, and a remote kernel (JupyterHub, Colab, VS Code) has no
// static route to add the rest to. So the Python side sends the built `dist/`
// as binary buffers and this module turns them into blob URLs — which is also
// why the imports below are dynamic. The wasm arrives as bytes, which is what
// wasm-bindgen's `init` takes anyway, so no `.wasm` is ever fetched by URL.
//
// **Why the module text is rewritten.** A module loaded from a blob URL
// resolves its relative imports against the blob's origin, where nothing
// lives, so the worklet's own imports would fail. Before each blob is made, its
// import specifiers are resolved against the *name* the asset came with and
// swapped for the blob URL of that asset. Blobs are therefore made leaf-first,
// which `assetOrder` fixes.
//
// **Why "ready" is announced rather than awaited.** A cell's output renders
// after the cell's code has run, so by the time this module executes the
// kernel has usually already sent a whole tree. It has kept a journal of it
// (see `clausters_jupyter.journal`); saying "ready" is what asks for the
// replay. The same message is what rebuilds the view after a page reload or a
// moved output, so there is one path, not a special case.

// Type-only, and that is load-bearing: anywidget serves this module and
// nothing beside it, so a *value* import of a sibling is a specifier the page
// cannot resolve. `Client` is `./client.ts` — the entry this front end is
// written against, which arrives bundled — as a type, which costs nothing at
// run time and types the one dynamic import everything else comes from.
type Client = typeof import("./client.ts");
type ClaustersGui = import("./client.ts").ClaustersGui;
type ClaustersServer = import("./client.ts").ClaustersServer;
type Session = InstanceType<Client["Session"]>;
type GuiHostClient = InstanceType<Client["GuiHost"]>;

/**
 * The share of the id space this page takes, the kernel holding the other.
 *
 * Both ends author against one engine — the kernel sends the defs and the
 * nodes, the page holds a `Session` on the same engine — and their allocators
 * would otherwise start at the same base and hand out the same first id. The
 * split is by arithmetic and needs no agreement beyond the index each side is
 * given: the kernel is 0 (`clausters_jupyter.session`), the page is 1.
 */
const PAGE_SHARE = { index: 1, of: 2 };

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
    void shared(session).runtime?.then(
        ({ engine }) => engine?.suspend().catch(() => {}));
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
 * What one notebook holds in this page, once its assets have arrived.
 *
 * Everything here is the client's own: the package imported from blob URLs,
 * the host `newGuiHost` booted on the host wasm, the engine `engine()` booted
 * on the engine wasm, and the `Session` that owns the two. What this module
 * adds is the last field — the tag the kernel's packets travel under, which is
 * the one client of this engine that lives in another process.
 */
interface Booted {
    client: Client;
    gui: ClaustersGui;
    /** The host client the kernel's packets are fed through. */
    host: GuiHostClient;
    /** `null` under the native backend: the audio is on the kernel's machine. */
    engine: ClaustersServer | null;
    /** `null` under the native backend, which gives this page no server. */
    session: Session | null;
    /** The kernel's client tag on this engine (`-1` when there is none). */
    kernelPeer: number;
}

/**
 * Bring one notebook's runtime up on staged assets.
 *
 * The whole of it is calls into the client, which is the point: a served page
 * boots the same host through the same `newGuiHost` and holds the same
 * `Session`. Two things are the notebook's own and are passed in rather than
 * discovered — the wasm arrives as *bytes* (a blob URL has no "next to the
 * glue" to look in), and the engine is booted from the URLs the assets were
 * staged at.
 *
 * The two backends differ here and nowhere else. With the engine's assets
 * present the page holds an engine and a `Session` on it; without them the
 * host's audio leg is opened to a native server's `--ws` port instead, and
 * this page has no server of its own to hold a session over — the kernel's
 * server is a process on another machine.
 */
async function bootRuntime(
    urls: Map<string, string>,
    wasm: Map<string, Uint8Array>,
    serverUrl: string,
): Promise<Booted> {
    const clientUrl = urls.get("notebook-client.js");
    const hostWasm = wasm.get("gui-host/clausters_gui_bg.wasm");
    const coreWasm = wasm.get("core/clausters_core_web_bg.wasm");
    if (clientUrl === undefined || hostWasm === undefined || coreWasm === undefined) {
        throw new Error("the client's assets did not arrive");
    }
    const client = (await import(/* @vite-ignore */ clientUrl)) as Client;
    // The codec, from the bytes that came with it — every decode below, and
    // everything the `Session` encodes, goes through this one core instance.
    await client.loadOsc(coreWasm.slice().buffer as ArrayBuffer);
    // The clock's wake-up. A worker is named by URL, and a bundle running from
    // a blob has no base to resolve one against — but this module was handed
    // the worker with everything else, so it says where it staged it. Without
    // this the clock silently falls back to the page timer, which a background
    // tab throttles to about a second.
    const tick = urls.get("base/tick-worker.js");
    if (tick !== undefined) client.setTickWorkerUrl(tick);

    // The engine, if this backend has one. Its loader already takes the two
    // URLs it needs, so the blobs go straight in and nothing is fetched.
    const engineWasm = urls.get("engine/clausters_web_bg.wasm");
    const workletUrl = urls.get("engine/worklet.js");
    const engine = engineWasm !== undefined && workletUrl !== undefined
        ? await client.engine({ wasmUrl: engineWasm, workletUrl })
        : null;
    if (engine === null) {
        console.info(
            "clausters: no in-page engine (the assets for one did not arrive). "
            + "Expected under backend='native', where the audio comes out of "
            + "the kernel's machine; under backend='page' it means the build "
            + "is incomplete - re-run scripts/refresh-web.sh.");
    } else {
        console.info(
            `clausters: engine up at ${engine.context.sampleRate} Hz, `
            + `AudioContext ${engine.context.state} (a browser starts no audio `
            + "until the page is clicked)");
        resumeOnGesture(engine);
    }

    // No canvas here: one is attached per `window`-rooted def, by the widget
    // that draws it. `newGuiHost` appends none either, which is exactly why
    // this is the instance door and not the page's `guiHost()`.
    const gui = await client.newGuiHost({
        engine,
        wasm: hostWasm.slice().buffer as ArrayBuffer,
    });
    console.info("clausters: GUI host up in this page");
    // The host's audio leg. In the page it was wired to the engine by
    // `newGuiHost`; against a native server it is a socket opened from the
    // browser, and therefore local-only. Either way the kernel is not in the
    // path: a bound widget's value reaches the audio at frame rate, as it does
    // on a served page and on the desktop.
    if (engine === null && serverUrl !== "") gui.bridge.connect_server(serverUrl);

    const share = PAGE_SHARE;
    const host = await client.GuiHost.page(gui, { share });
    let session: Session | null = null;
    if (engine !== null) {
        session = await client.Session.page({ own: true, engine, share });
        // The host is built, not booted, so this is the adopting door — and
        // the wasm instance goes with it, so `close()` releases the GPU device
        // and the drain loop as well as the client.
        session.adoptGui(host, { page: gui });
    }
    return {
        client,
        gui,
        host,
        engine,
        session,
        kernelPeer: engine === null ? -1 : engine.claimPeer(),
    };
}

/**
 * Resume the audio context on the first gesture anywhere in the document.
 *
 * A browser will not start audio without one, and a notebook offers no obvious
 * place to put a "start audio" button — so any click, key or touch does it,
 * once. Until then the engine runs silently, which is also what it does with
 * nothing playing, so nothing looks broken while the page waits.
 */
function resumeOnGesture(engine: ClaustersServer): void {
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

/**
 * The one runtime of **one notebook**, shared by every cell of it.
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
 * kernel: independent id spaces get independent runtimes.
 *
 * That is a real host and a real engine each, not a partitioned share of one.
 * The wasm exports an instance per `start()` — they share the page's one winit
 * event loop and nothing else — so two notebooks may hold the very same widget
 * and node ids without seeing each other, which is the only arrangement that
 * works when the two kernels are separate processes with no channel to agree
 * on a range over.
 *
 * (Inside *one* notebook the two ends do share an engine, and there the ids
 * would collide: the kernel and this page both author against it. That is what
 * `PAGE_SHARE` settles.)
 *
 * The assets are the exception, and are cached across sessions (`STAGED`):
 * they are the same bytes, they are immutable, and they are the expensive
 * part. A second notebook boots its own runtime on the first one's blob URLs,
 * and its wasm module is the one already compiled.
 */
interface Shared {
    runtime: Promise<Booted> | null;
    /** Mounted canvases, counted so the audio can follow them (`heard`). */
    views: number;
    /**
     * Packets for a runtime that is still coming up, in order.
     *
     * The GUI channel has had this since the beginning (`pending`, per view)
     * and the server channel had nothing: a packet arriving before the engine
     * existed went to `undefined?.then(...)` and was dropped without a trace.
     * That window is not an edge case, it is the opening of every notebook --
     * the cell that displays a window is usually the cell that sent the def
     * and the `/synth_new`, and the wasm takes about a second to come up
     * behind it. What it looked like: a notebook that drew correctly and made
     * no sound, with nothing logged at either end.
     *
     * Drained once, when the runtime resolves; after that a send goes straight
     * out. Order is kept either way -- a callback registered on the resolved
     * promise runs after this drain, which was registered first.
     */
    toEngine: Uint8Array[];
    /** Live models of this session, so the last one out can close it. */
    models: number;
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
        runtime: null, views: 0, toEngine: [], models: 0, closing: null,
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
 * Close a session: its runtime, and its entry in the page's registry.
 *
 * One call does it, because a `Session` owns what it opened: its engine (and
 * so the `AudioContext`), its host client and the wasm host under it, its
 * server client and its clock. What a browser caps is how many contexts
 * *exist*, so this closes rather than suspends — one left suspended is one the
 * next notebook cannot have.
 *
 * The counterpart of `bootRuntime`, and the reason it can exist at all is that
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
    if (state.closing !== null) clearTimeout(state.closing);
    state.toEngine.length = 0;
    void state.runtime?.then((booted) => {
        // Under the native backend there is no session to own the host, so the
        // host is released directly; under the page one `close()` covers it.
        if (booted.session !== null) booted.session.close();
        else {
            booted.host.stop();
            booted.gui.close();
        }
    });
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
        void state.runtime?.then(({ engine }) => {
            if (engine === null) return;
            // Both reject on a context that `closeSession` has already
            // closed, which is a race this does not need to win: a session
            // being torn down has nothing left to suspend.
            const settled = state.views > 0 ? engine.resume() : engine.suspend();
            void settled.catch(() => {});
        });
    }, 100);
}

/** Boot (or join) this session's runtime, and drain what waited for it. */
function bootShared(
    session: string,
    urls: Map<string, string>,
    wasm: Map<string, Uint8Array>,
    serverUrl: string,
): Promise<Booted> {
    const state = shared(session);
    state.runtime ??= bootRuntime(urls, wasm, serverUrl).then((booted) => {
        // Whatever the kernel sent while this was coming up. Dropped rather
        // than held when there is no engine (the native backend sends nothing
        // here, but a queue that only grows is worse than one that empties).
        const queued = state.toEngine.splice(0);
        if (booted.engine !== null) {
            for (const packet of queued) booted.engine.send(packet, booted.kernelPeer);
        }
        return booted;
    });
    return state.runtime;
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

        // Which notebook this cell belongs to. One per kernel, so the runtime
        // below is this notebook's and not the Lab tab's.
        const session = model.get("session") as string;
        // Before anything else: this front end is what keeps the session's
        // runtime on the page, and its death is what releases it.
        trackModel(session, model);

        let booted: Booted | null = null;
        const pending: Uint8Array[] = [];
        const attached = new Set<number>();

        // This view's two ways up, held by identity so the cleanup can take
        // them off again. A view that subscribes and never unsubscribes is
        // not a small leak: the host then sends the kernel one copy of every
        // event per cell re-run, at thirty a second while a knob is moving,
        // through models that were disposed hours ago.
        const up = {
            gui: (packet: Uint8Array) => model.send({ ch: GUI }, undefined, [
                packet.slice().buffer as ArrayBuffer,
            ]),
            server: (packet: Uint8Array) => model.send({ ch: SERVER }, undefined, [
                packet.slice().buffer as ArrayBuffer,
            ]),
        };

        /**
         * Boot (or join) this session's runtime on staged assets.
         *
         * Another notebook already holding one in this tab is not a condition
         * to check for: this one boots an instance of its own beside it,
         * sharing the page's event loop and none of its ids.
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
        // runtime is up: the helpers that do the measuring arrive with it.
        const fit = () => {
            if (booted === null) return;
            const { width, height, scale } = booted.client.canvasBox(canvas);
            canvas.width = width;
            canvas.height = height;
            for (const id of attached) booted.gui.bridge.resize(id, width, height, scale);
        };
        new ResizeObserver(fit).observe(canvas);

        /**
         * One packet from the kernel into this notebook's host.
         *
         * It goes in through the host client's own carrier, the same door a
         * page's own `GuiHost` sends through — so the canvas policy, the
         * attach and the detach on a freed window are the client's rather than
         * this module's. What is this module's is *which* canvas: a cell owns
         * where its window draws, so it claims one for the def before the
         * packet is sent, and `attach` is idempotent precisely so that claim
         * stands against the carrier's default.
         */
        const feed = (packet: Uint8Array) => {
            const here = booted!;
            for (const { addr, args } of here.client.decodePacket(packet)) {
                const id = args[0];
                if (typeof id !== "number") continue;
                if (addr === "/gui_def" && !attached.has(id)) {
                    here.gui.attach(id, canvas);
                    attached.add(id);
                    const { width, height, scale } = here.client.canvasBox(canvas);
                    here.gui.bridge.resize(id, width, height, scale);
                } else if (addr === "/gui_free" && attached.delete(id)) {
                    // Closing a window empties the cell that was showing it.
                    // The host gives the surface up on the same packet (the
                    // carrier detaches); what is left is the element, which
                    // would otherwise stay behind holding its last frame --
                    // what made `win.close()` look like it did nothing.
                    queueMicrotask(() => canvas.remove());
                }
            }
            here.host.connection.send(packet);
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
                    if (booted === null) pending.push(packet);
                    else feed(packet);
                }
            }
            if (content.ch === SERVER) {
                const state = shared(session);
                for (const buffer of buffers) {
                    const packet = asBytes(buffer);
                    // The runtime is null until the boot assigns it, and the
                    // kernel does not wait -- see `Shared.toEngine`.
                    if (state.runtime === null) state.toEngine.push(packet);
                    else {
                        void state.runtime.then((live) =>
                            live.engine?.send(packet, live.kernelPeer),
                        );
                    }
                }
            }
        });

        const ready = (live: Booted) => {
            booted = live;
            live.client.onScaleChange(fit);
            fit();
            // The host's outbound events, and the engine's replies to the
            // kernel. Both are per view and both come off again on unmount:
            // the host fans out to every listener, so a view that stayed
            // subscribed would send the kernel one copy per cell re-run.
            live.gui.addEvent(up.gui);
            // The engine answers the kernel too (a /server_sync's /synced, a
            // query's reply), under the kernel's own tag -- the host's leg is
            // a different client and is already wired to it.
            live.engine?.addReply(up.server, live.kernelPeer);
            for (const packet of pending.splice(0)) feed(packet);
        };

        // The kernel has already sent everything this cell drew: "ready" asks
        // for the replay. `have` is the digest of the assets this *page* has
        // staged, if any: the kernel sends the nine megabytes only when they
        // are not the ones it would send. A second notebook in the same Lab
        // tab therefore boots its own runtime on the first one's blob URLs.
        const already = shared(session).runtime;
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
            booted?.gui.removeEvent(up.gui);
            booted?.engine?.removeReply(up.server, booted.kernelPeer);
            for (const id of attached) booted?.gui.detach(id);
        };
    },
};
