/**
 * **The editing applications' half of a page**: the host's applications'
 * build, loaded in a page that is cross-origin isolated, and the workers its
 * applications run their threads on.
 *
 * The build is one module over one shared memory. The page compiles it once
 * and keeps the module and the memory, so that every thread the application
 * spawns (`clausters_core::threads::spawn`) is a worker that instantiates the
 * same module over the same memory (`apps-thread.ts`).
 *
 * @module
 */

type HostGlue = typeof import("../gui-host/clausters_gui.js");

/** A thread's stack, in bytes: a render decodes and writes on it. */
const THREAD_STACK = 4 * 1024 * 1024;

/**
 * **The applications' build of the host, instantiated**, with the page's
 * hook that makes its threads installed before it can need one.
 */
export async function appsHost(): Promise<{ glue: HostGlue; init: () => Promise<unknown> }> {
    const glueUrl = new URL("../gui-host-apps/clausters_gui.js", import.meta.url).href;
    const glue = await import(glueUrl) as HostGlue;
    const wasmUrl = new URL("../gui-host-apps/clausters_gui_bg.wasm", import.meta.url);
    const module = await WebAssembly.compileStreaming(fetch(wasmUrl));
    let memory: WebAssembly.Memory | null = null;
    (globalThis as { __claustersApps?: unknown }).__claustersApps = {
        /** A worker named `name` that runs the body at `body`; whether it was made. */
        spawn(name: string, body: number): boolean {
            if (memory === null) return false;
            try {
                const worker = new Worker(new URL("./apps-thread.js", import.meta.url), {
                    type: "module",
                    name,
                });
                worker.onerror = (event) => console.error(`clausters: thread ${name}: ${event.message}`);
                worker.postMessage({ glue: glueUrl, module, memory, body, stack: THREAD_STACK });
                return true;
            } catch (error) {
                console.error(`clausters: no worker for thread ${name}: ${String(error)}`);
                return false;
            }
        },
    };
    const init = async () => {
        const exports = await (glue.default as (options: { module_or_path: WebAssembly.Module }) => Promise<{ memory: WebAssembly.Memory }>)({
            module_or_path: module,
        });
        memory = exports.memory;
        return exports;
    };
    return { glue, init };
}
