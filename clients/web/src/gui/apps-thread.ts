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
}

const scope = globalThis as unknown as {
    onmessage: ((event: MessageEvent<ThreadStart>) => void) | null;
    close(): void;
};

scope.onmessage = async (event) => {
    const { glue, module, memory, body, stack } = event.data;
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
