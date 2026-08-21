// The engraver module, and the six calls the score model drives it through
// (the counterpart of `clausters/gui/notation/_abi.py`).
//
// Python's `_abi` is the plumbing both of its native callers share: a
// size-then-fill call and a UTF-8 view. A page's plumbing is different in shape
// and identical in role — load the Emscripten module, `cwrap` the toolkit's C
// functions, and hand back an object whose method names are the port's.
//
// **The functions are the same C wrapper the native library exports**
// (`tools/c_wrapper.h`): `vrvToolkit_loadData`, `_renderToSVG`, `_getMEI`,
// `_edit`, `_renderToTimemap`, `_getMIDIValuesForElement`. That is why one
// `Engraver` port serves both — a process calls them through the C ABI, a page
// through `cwrap`, and the state machine above neither knows nor cares.
//
// The module is **loaded on demand**: `dist/vendor/verovio/` is staged beside
// the wasm bundles but is not part of the slim runtime, so a page that never
// engraves never downloads 6.5 MB of engraver.

/** The Emscripten module, as much of it as this file uses. */
interface VerovioModule {
    cwrap(name: string, ret: string | null, args: string[]): (...a: unknown[]) => unknown;
}

/** What the loader hands back: the module factory's result, once. */
let modulePromise: Promise<VerovioModule> | null = null;

/**
 * Where the engraver module lives. Resolved against this module's own URL, the
 * way the worklet is (`new URL(…, import.meta.url)`) — the form a bundler
 * copies as an asset — with an override for a page that stages it elsewhere.
 */
let engraverUrl: string | null = null;

/**
 * Point the loader at a `verovio.js` of your own — a different path, a CDN, a
 * blob. Call it before the first engraving; afterwards the module is already
 * loaded and this does nothing.
 *
 * The escape hatch `workletUrl` is for the worklet: a consumer whose bundler
 * neither copies the asset nor resolves the URL says where the file went.
 */
export function setEngraverUrl(url: string): void {
    engraverUrl = url;
}

/**
 * The loaded engraver module. Idempotent: the module is built once per page and
 * every score shares it, exactly as every score in a process shares one
 * libverovio.
 */
async function verovioModule(): Promise<VerovioModule> {
    modulePromise ??= (async () => {
        const url =
            engraverUrl ?? new URL("../../vendor/verovio/verovio.js", import.meta.url).href;
        const glue = (await import(/* @vite-ignore */ url)) as {
            default: () => Promise<VerovioModule>;
        };
        return glue.default();
    })();
    return modulePromise;
}

/**
 * One verovio toolkit, and the six methods the score model calls it through.
 *
 * The names are the port's, not verovio's, because they are what
 * `clausters_core::notation::Engraver` reads off the object handed to it. What
 * each one does is one `vrvToolkit_*` call and nothing else — no ordering, no
 * caching, no recovery, all of which belong to the model.
 */
export class Toolkit {
    private readonly module: VerovioModule;
    private readonly ptr: number;
    private readonly fns: Record<string, (...a: unknown[]) => unknown>;

    private constructor(module: VerovioModule) {
        this.module = module;
        const cw = (name: string, ret: string | null, args: string[]) =>
            module.cwrap(`vrvToolkit_${name}`, ret, args);
        this.fns = {
            constructor: cw("constructor", "number", []),
            destructor: cw("destructor", null, ["number"]),
            setOptions: cw("setOptions", null, ["number", "string"]),
            loadData: cw("loadData", "number", ["number", "string"]),
            renderToSVG: cw("renderToSVG", "string", ["number", "number", "number"]),
            getMEI: cw("getMEI", "string", ["number", "string"]),
            edit: cw("edit", "number", ["number", "string"]),
            renderToTimemap: cw("renderToTimemap", "string", ["number", "string"]),
            getMIDIValuesForElement: cw("getMIDIValuesForElement", "string", [
                "number",
                "string",
            ]),
            getVersion: cw("getVersion", "string", ["number"]),
        };
        this.ptr = this.fns.constructor?.() as number;
    }

    /**
     * Build a toolkit and configure it. `options` is the engraver's own options
     * object, as the native binding's `EngraveOptions` becomes.
     */
    static async open(options: Record<string, unknown>): Promise<Toolkit> {
        const toolkit = new Toolkit(await verovioModule());
        toolkit.fns.setOptions?.(toolkit.ptr, JSON.stringify(options));
        return toolkit;
    }

    /** The engraver's version string — what the two ends must agree on. */
    version(): string {
        return (this.fns.getVersion?.(this.ptr) as string) ?? "";
    }

    /** Free the toolkit. A page that keeps engraving keeps the module. */
    free(): void {
        this.fns.destructor?.(this.ptr);
    }

    // ---- the six the port reads ----

    loadData(data: string): boolean {
        return Boolean(this.fns.loadData?.(this.ptr, data));
    }

    renderSvg(page: number): string {
        // The third argument is verovio's `xmlDeclaration`, off: the walk reads
        // a fragment, and the native binding passes false for the same reason.
        return (this.fns.renderToSVG?.(this.ptr, page, 0) as string) ?? "";
    }

    mei(): string {
        return (this.fns.getMEI?.(this.ptr, "{}") as string) ?? "";
    }

    edit(action: string): boolean {
        return Boolean(this.fns.edit?.(this.ptr, action));
    }

    timemap(options: string): string {
        return (this.fns.renderToTimemap?.(this.ptr, options) as string) ?? "";
    }

    midiValues(xmlId: string): string {
        return (this.fns.getMIDIValuesForElement?.(this.ptr, xmlId) as string) ?? "";
    }
}
