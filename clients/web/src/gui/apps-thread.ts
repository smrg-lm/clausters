/**
 * **One thread of an editing application in a page**: a worker that
 * instantiates the host's applications' build over the memory the page
 * shares, runs the one body it was made for, and ends.
 *
 * The page makes it (`apps.ts`) when the application spawns a thread (a
 * render, a take's reader): the message is the glue's URL, the compiled
 * module, the shared memory and the body's place in it, and the body runs on
 * a stack of the worker's own, on the same heap as the main thread's.
 *
 * @module
 */

/** What the page hands a thread. */
interface ThreadStart {
    glue: string;
    module: WebAssembly.Module;
    memory: WebAssembly.Memory;
    body: number;
    stack: number;
    /** The thread's line to the page's file worker (`apps-files.ts`). */
    files: MessagePort;
}

const scope = globalThis as unknown as {
    onmessage: ((event: MessageEvent<ThreadStart>) => void) | null;
    close(): void;
};

scope.onmessage = async (event) => {
    const { glue, module, memory, body, stack, files } = event.data;
    // **A call into the page's storage waits here**: the request's address
    // goes to the file worker, and this thread sleeps on its state until the
    // worker answers, which a worker may do and the main thread may not.
    (globalThis as { __claustersApps?: unknown }).__claustersApps = {
        file(request: number): void {
            files.postMessage(request);
            Atomics.wait(new Int32Array(memory.buffer), request >> 2, 0);
        },
    };
    const host = await import(glue) as {
        initSync(options: { module: WebAssembly.Module; memory: WebAssembly.Memory; thread_stack_size: number }): {
            __wbindgen_thread_destroy?: () => void;
        };
        apps_thread_run(body: number): void;
    };
    const exports = host.initSync({ module, memory, thread_stack_size: stack });
    try {
        host.apps_thread_run(body);
    } finally {
        // the worker's stack and storage go back to the shared memory
        exports.__wbindgen_thread_destroy?.();
        scope.close();
    }
};
