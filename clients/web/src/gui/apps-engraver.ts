/**
 * **The engraver of an editing application in a page**: a worker that answers
 * the application's thread's calls into verovio.
 *
 * The score editor runs in the host's application, on a thread, and reads a page
 * through a port it calls synchronously. Natively that is libverovio. Here the
 * thread writes a request into the shared memory, posts its address and waits on
 * it, as for the page's storage (`apps-files.ts`); this worker runs the call on
 * the toolkit a client also uses (`notation/_verovio.ts`), writes the answer back
 * and wakes the thread. verovio is fetched the first time it is asked for, so a
 * page that never engraves never downloads it.
 *
 * The request's layout is `Request` of `clients/gui/src/host/web/engraver.rs`.
 *
 * @module
 */

import { Toolkit, setEngraverUrl } from "./notation/_verovio.ts";

/** The request's fields, by their offsets in it. */
const field = { state: 0, call: 4, callLen: 8, out: 12, outCap: 16, outLen: 20 } as const;

let memory: WebAssembly.Memory | null = null;

/** The toolkits the application holds, by the number it gave each. */
const toolkits = new Map<number, Toolkit>();

/** The answer last written, for a call that asks again into a buffer that fits. */
let last = new Uint8Array(0);

/** The calls in order, since opening a toolkit may have to fetch verovio. */
let queue: Promise<void> = Promise.resolve();

interface Call {
    id?: number;
    m: string;
    a?: unknown;
}

/** One call on its toolkit, and what it answers. */
async function run(call: Call): Promise<unknown> {
    if (call.m === "open") {
        try {
            toolkits.set(call.id as number, await Toolkit.open(JSON.parse(String(call.a)) as Record<string, unknown>));
            return true;
        } catch (error) {
            return { error: String(error) };
        }
    }
    const toolkit = toolkits.get(call.id as number);
    if (toolkit === undefined) return null;
    switch (call.m) {
        case "free":
            toolkit.free();
            toolkits.delete(call.id as number);
            return null;
        case "loadData":
            return toolkit.loadData(String(call.a));
        case "renderSvg":
            return toolkit.renderSvg(Number(call.a));
        case "mei":
            return toolkit.mei();
        case "edit":
            return toolkit.edit(String(call.a));
        case "timemap":
            return toolkit.timemap(String(call.a));
        case "midiValues":
            return toolkit.midiValues(String(call.a));
        case "setOptions":
            return toolkit.setOptions(String(call.a));
        case "pageCount":
            return toolkit.pageCount();
        default:
            return null;
    }
}

/** Answers the request at `request`, and wakes the thread waiting on it. */
async function answer(request: number): Promise<void> {
    const shared = (memory as WebAssembly.Memory).buffer;
    const view = new DataView(shared);
    const at = (offset: number) => view.getUint32(request + offset, true);
    // a copy: a decoder takes no view of shared memory
    const text = new TextDecoder().decode(new Uint8Array(shared, at(field.call), at(field.callLen)).slice());
    const call = JSON.parse(text) as Call;
    if (call.m !== "again") last = new TextEncoder().encode(JSON.stringify((await run(call)) ?? null));
    // the memory may have grown while verovio was fetched
    const now = (memory as WebAssembly.Memory).buffer;
    const after = new DataView(now);
    if (last.length <= after.getUint32(request + field.outCap, true)) {
        new Uint8Array(now, after.getUint32(request + field.out, true), last.length).set(last);
    }
    after.setUint32(request + field.outLen, last.length, true);
    const state = new Int32Array(now);
    Atomics.store(state, (request + field.state) >> 2, 1);
    Atomics.notify(state, (request + field.state) >> 2);
}

const scope = globalThis as unknown as {
    onmessage: ((event: MessageEvent<{ memory?: WebAssembly.Memory; url?: string | null; port?: MessagePort }>) => void) | null;
};

scope.onmessage = (event) => {
    if (event.data.memory !== undefined) memory = event.data.memory;
    if (event.data.url !== undefined && event.data.url !== null) setEngraverUrl(event.data.url);
    const port = event.data.port;
    if (port !== undefined) {
        port.onmessage = (request: MessageEvent<number>) => {
            queue = queue.then(() => answer(request.data)).catch((error: unknown) => console.error(`clausters: the engraver: ${String(error)}`));
        };
    }
};
