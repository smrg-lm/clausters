// The page's Faust compiler, as the **offline renderer** reaches it.
//
// The live engine reaches the same compiler through the worklet: `/def_send
// faust` parks a request, the worklet hands it to the NRT worker and the answer
// comes back as a `/done` whenever it comes back. An offline render cannot be
// answered whenever: it loads a def where it stands and time does not advance
// until it has. So the render does the same work in the other order — read the
// score's Faust defs, compile them, link them, *then* render — and this module
// is the compile step of that.
//
// It starts a Worker of its own rather than sharing the engine's. Two reasons,
// and the second is the one that decides it: a page can render with no engine
// booted at all (no `AudioContext`, no worklet, nothing to share), and a def
// linked into the worklet's engine could not be used by the renderer anyway —
// they are two instances with two memories, and a module is linked into one of
// them. What the sharing would save is a second compiler in a page that both
// plays and renders Faust, which is 26 MiB and only in that page; what it would
// cost is the render path depending on whether an engine happens to exist.
//
// Nothing here is loaded until a score actually sends a Faust def.

/** One def to compile: the three fields `faustJobs` reads out of a score. */
export interface FaustJob {
    name: string;
    /** `"source"`, `"boxes"` or `"signals"`. */
    kind: string;
    def: string;
}

/** One compiled def: the module's bytes and the compiler's own JSON. */
export interface CompiledDef {
    name: string;
    bytes: ArrayBuffer;
    json: string;
}

let worker: Worker | null = null;
let ticket = 0;
const waiting = new Map<number, (r: WorkerReply) => void>();

type WorkerReply =
    | { type: "faust"; ticket: number; bytes: ArrayBuffer; json: string }
    | { type: "faust"; ticket: number; error: string };

/**
 * The Worker, started on the first def and kept for the page's life.
 *
 * `workerUrl` overrides where it is loaded from, for a host that resolves
 * module URLs differently (the same escape the loader's own options give).
 */
function compiler(workerUrl?: URL | string): Worker {
    if (worker !== null) return worker;
    if (typeof Worker === "undefined") {
        throw new Error(
            "this page has no Worker, so it cannot compile a Faust def for an offline render",
        );
    }
    const url = workerUrl ?? new URL("./nrt-worker.js", import.meta.url);
    worker = new Worker(url, { type: "module" });
    worker.onmessage = (e: MessageEvent) => {
        const reply = e.data as WorkerReply;
        const done = waiting.get(reply.ticket);
        if (done === undefined) return;
        waiting.delete(reply.ticket);
        done(reply);
    };
    return worker;
}

/**
 * Compiles every def in `jobs`, in parallel — unlike a soundfile read there is
 * no ordering between them, and each answers on its own.
 *
 * Rejects with the compiler's own message, prefixed by the def's name, on the
 * first one that fails: a render whose def did not compile has nothing to
 * render, and reporting that as a Faust error rather than as a missing def is
 * what tells the caller which of the two happened.
 */
export function compileFaustDefs(
    jobs: FaustJob[],
    workerUrl?: URL | string,
): Promise<CompiledDef[]> {
    const w = compiler(workerUrl);
    return Promise.all(
        jobs.map(
            (job) =>
                new Promise<CompiledDef>((resolve, reject) => {
                    const t = ++ticket;
                    waiting.set(t, (reply) => {
                        if ("error" in reply) reject(new Error(`${job.name}: ${reply.error}`));
                        else resolve({ name: job.name, bytes: reply.bytes, json: reply.json });
                    });
                    w.postMessage({ type: "faust", ticket: t, ...job });
                }),
        ),
    );
}

/** Ends the compiler Worker, if one was ever started. */
export function stopFaustCompiler(): void {
    worker?.terminate();
    worker = null;
    waiting.clear();
}
