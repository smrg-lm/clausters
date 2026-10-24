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

/** What a thread posts the page (`apps-thread.ts`). */
interface ThreadSays {
    /** A thread it spawns, which only the page can make. */
    spawn?: { name: string; body: number };
    /** Its engine's processing half, for the page's output. */
    engine?: number;
    channels?: number;
    /** The host whose loop is woken. */
    said?: number;
}

/** A thread's stack, in bytes: a render decodes and writes on it. */
const THREAD_STACK = 4 * 1024 * 1024;

/**
 * **The applications' build of the host, instantiated**, with the page's
 * hook that makes its threads installed before it can need one.
 */
export async function appsHost(
    context: () => Promise<AudioContext>,
): Promise<{ glue: HostGlue; init: () => Promise<unknown> }> {
    const glueUrl = new URL("../gui-host-apps/clausters_gui.js", import.meta.url).href;
    const glue = await import(glueUrl) as HostGlue;
    const wasmUrl = new URL("../gui-host-apps/clausters_gui_bg.wasm", import.meta.url);
    const module = await WebAssembly.compileStreaming(fetch(wasmUrl));
    let memory: WebAssembly.Memory | null = null;
    /** The worker that answers the threads' calls into the page's storage. */
    let files: Worker | null = null;
    /** A worker named `name` that runs the body at `body`; whether it was made. */
    const spawn = (name: string, body: number): boolean => {
        if (memory === null) return false;
        try {
            const worker = new Worker(new URL("./apps-thread.js", import.meta.url), {
                type: "module",
                name,
            });
            worker.onerror = (event) => console.error(`clausters: thread ${name}: ${event.message}`);
            // what the thread asks of the page: a thread of its own, its
            // engine on the page's output, its host's loop woken
            worker.onmessage = (event: MessageEvent<ThreadSays>) => {
                const { spawn: thread, engine, channels, said } = event.data;
                if (thread !== undefined) {
                    spawn(thread.name, thread.body);
                } else if (said !== undefined) {
                    (glue as unknown as { apps_said(host: number): void }).apps_said(said);
                } else if (engine !== undefined && memory !== null) {
                    void startEngine(context, module, memory, engine, channels ?? 2);
                }
            };
            // the thread's own line to the file worker
            const line = new MessageChannel();
            files ??= filesWorker(memory);
            files.postMessage({ port: line.port1 }, [line.port1]);
            worker.postMessage({ glue: glueUrl, module, memory, body, stack: THREAD_STACK, files: line.port2 },
                [line.port2]);
            return true;
        } catch (error) {
            console.error(`clausters: no worker for thread ${name}: ${String(error)}`);
            return false;
        }
    };
    (globalThis as { __claustersApps?: unknown }).__claustersApps = { spawn };
    const init = async () => {
        const exports = await (glue.default as (options: { module_or_path: WebAssembly.Module }) => Promise<{ memory: WebAssembly.Memory }>)({
            module_or_path: module,
        });
        memory = exports.memory;
        // the rate the application's engine runs at is the page's audio's
        try {
            (glue as unknown as { apps_engine_rate(rate: number): void })
                .apps_engine_rate((await context()).sampleRate);
        } catch (error) {
            console.warn(`clausters: no audio for the applications: ${String(error)}`);
        }
        return exports;
    };
    return { glue, init };
}

/** The page's file worker, over the shared memory (`apps-files.ts`). */
function filesWorker(memory: WebAssembly.Memory): Worker {
    const worker = new Worker(new URL("./apps-files.js", import.meta.url), { type: "module", name: "files" });
    worker.onerror = (event) => console.error(`clausters: the files worker: ${event.message}`);
    worker.postMessage({ memory });
    return worker;
}

/**
 * **The application's engine on the page's output**: an AudioWorkletNode
 * whose processor instantiates the module over the shared memory and
 * processes the engine at `engine` (`apps-worklet.ts`), into the context's
 * destination.
 */
async function startEngine(
    context: () => Promise<AudioContext>,
    module: WebAssembly.Module,
    memory: WebAssembly.Memory,
    engine: number,
    channels: number,
): Promise<void> {
    const ctx = await context();
    await ctx.audioWorklet.addModule(new URL("./apps-worklet.js", import.meta.url));
    const node = new AudioWorkletNode(ctx, "clausters-apps", {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [channels],
        processorOptions: { module, memory, engine, channels, stack: THREAD_STACK },
    });
    node.connect(ctx.destination);
    if (ctx.state === "suspended") await ctx.resume().catch(() => {});
}
