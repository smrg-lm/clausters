// The NRT worker: the browser's version of the thread that is neither audio
// nor interface.
//
// A native server has one for exactly this work — reading a soundfile,
// decoding it, building the buffer — off both the audio thread and the network
// one. The engine in a page had no such thread, so the work landed on the
// AudioWorklet, which owes a block of audio every 2.67 ms. This is that thread.
//
// It does three things and holds nothing:
//
//  - reads a file out of the page's own filesystem (OPFS, reachable only from
//    a dedicated Worker — see `./opfs.ts`),
//  - decodes it with **our** decoder, the same one a native server runs.
//    `decodeAudioData` is right there and is the wrong answer: it is a
//    different decoder, so the same file would become different samples in a
//    tab and in a window, and a divergence in values is the kind nothing names,
//    and
//  - compiles a Faust def. This is the page's compiler thread: natively a
//    `/def_send faust` compiles on a thread of its own and answers late, and
//    that is exactly what happens here — the module comes back to the worklet,
//    which links it into the engine's own memory.
//
// The samples go back as a **transferred** buffer, so crossing the thread
// boundary moves them rather than copying them. Installing them is the
// worklet's business and is paced there, a run at a time.

/// <reference lib="webworker" />
import { extensionOf, readFile, readRange, writeFile } from "./opfs.ts";
import { HEAD_BYTES, parseShape, wrapSpan, type WavShape } from "./wav.ts";

/** What the worklet asks for. `ticket` comes back untouched. */
interface ReadRequest {
    type: "read";
    ticket: number;
    path: string;
    fileStart: number;
    numFrames: number;
    /** `/buffer_allocReadChannel`'s selection, in order; empty is all of them. */
    channels: number[];
}

/** `DiskIn`: the shape of a file, so the engine can build a stream on it. */
interface ShapeRequest {
    type: "shape";
    ticket: number;
    path: string;
}

/** `DiskIn`: the next span of a file being streamed. */
interface SpanRequest {
    type: "span";
    ticket: number;
    path: string;
    /** First frame wanted. */
    frame: number;
    /** How many. A short answer is the end of the file. */
    frames: number;
}

/** `/def_send faust`: compile a def. The page is the engine's compiler. */
interface FaustRequest {
    type: "faust";
    ticket: number;
    /** The def's name, for the compiler's own error messages. */
    name: string;
    /** `"source"`, `"boxes"` or `"signals"` — which format `def` is in. */
    kind: string;
    def: string;
}

/**
 * `/buffer_write`: put a span of a buffer in the page's filesystem.
 *
 * The runs arrive one per serving turn — the samples leave the engine's memory
 * paced, the way a long load arrives paced — and the file is written **once**,
 * at `final`. Nothing is visible until then, which is the same rule a staged
 * load follows: a half-written file is not a shorter take, it is a wrong one.
 * That is what separates this from `record`, which rewrites its file at every
 * flush because a recording has to survive the tab closing mid-take.
 */
interface WriteRequest {
    type: "write";
    ticket: number;
    path: string;
    samples: Float32Array;
    channels: number;
    sampleRate: number;
    format: string;
    final: boolean;
}

/** `DiskOut`: append a recorded run, and close the file when `final`. */
interface RecordRequest {
    type: "record";
    ticket: number;
    path: string;
    samples: Float32Array;
    channels: number;
    sampleRate: number;
    format: string;
    final: boolean;
}

/**
 * How much linear memory the compiler currently holds.
 *
 * A diagnostic, not a capability: a compiler keeps **one** lib context for as
 * long as it lives (see `contextLive`), so the terms of every def compiled
 * through it accumulate in that one arena, and how many compilers a tab has
 * been through is the other half of the same question. Both are answered from
 * here because the compiler is loaded here and reachable from nowhere else;
 * `tests/faust-arena.html` is what asks, and what it measures is written on
 * `docs/src/platform.md`.
 */
interface FaustHeapRequest {
    type: "faust-heap";
    ticket: number;
}

type Request =
    | ReadRequest
    | ShapeRequest
    | SpanRequest
    | WriteRequest
    | RecordRequest
    | FaustRequest
    | FaustHeapRequest
    | { type: "ping" };

/** What comes back: the samples, or the message the command fails with. */
type Response =
    | {
          type: "read";
          ticket: number;
          samples: Float32Array;
          channels: number;
          frames: number;
          sampleRate: number;
      }
    | { type: "read"; ticket: number; error: string }
    | {
          type: "shape";
          ticket: number;
          channels: number;
          sampleRate: number;
          frames: number;
      }
    | { type: "shape"; ticket: number; error: string }
    | { type: "span"; ticket: number; samples: Float32Array; frames: number }
    | { type: "span"; ticket: number; error: string }
    | { type: "record"; ticket: number; error?: string }
    | { type: "write"; ticket: number; error?: string }
    | {
          type: "faust";
          ticket: number;
          /** The module's bytes, stripped and ready to instantiate. */
          bytes: ArrayBuffer;
          /** The compiler's own JSON: the struct size and every parameter's
           *  byte offset inside it. */
          json: string;
      }
    | { type: "faust"; ticket: number; error: string }
    | { type: "faust-heap"; ticket: number; bytes: number; reloads: number }
    | { type: "ready" };

// The decoder's glue is imported dynamically: this file is type-checked
// everywhere and only ever *runs* in a browser, where `../nrt/` resolves
// against the staged bundle.
type Decoder = {
    default: (module?: unknown) => Promise<unknown>;
    decodeAudio: (
        bytes: Uint8Array,
        ext: string,
        label: string,
        fileStart: number,
        numFrames: number,
        channels: Uint32Array,
    ) => {
        samples: Float32Array;
        channels: number;
        frames: number;
        sampleRate: number;
    };
    wavHeader: (
        channels: number,
        sampleRate: number,
        format: string,
        dataBytes: number,
    ) => Uint8Array;
    encodeWavFrames: (samples: Float32Array, format: string) => Uint8Array;
    stripFaustData: (module: Uint8Array) => Uint8Array;
    faustBoxFromJson: (json: string) => number;
    faustSignalsFromJson: (json: string) => Uint32Array;
};

let decoder: Promise<Decoder> | null = null;
/** The NRT bundle's own linear memory: where the JSON interpreter's C strings
 *  live, and what the Faust shim reads them out of. */
let decoderMemory: WebAssembly.Memory | null = null;

/** Loads the decoder once, on the first read — a page that never reads a
 *  soundfile never fetches it. */
function load(): Promise<Decoder> {
    if (decoder === null) {
        decoder = (async () => {
            const mod = (await import(
                /* @vite-ignore */ new URL("../nrt/clausters_nrt_web.js", import.meta.url).href
            )) as Decoder;
            const out = (await mod.default()) as { memory: WebAssembly.Memory };
            decoderMemory = out.memory;
            return mod;
        })();
    }
    return decoder;
}

// The Faust compiler's own glue, imported the same way and for the same
// reason: a page whose bundle carries no FaustDef never fetches the several
// megabytes of it. `libfaust-wasm` is an Emscripten module with a filesystem
// of its own — the `.data` beside it is the Faust standard library, which is
// what lets a def `import("stdfaust.lib")`.
/** What a compilation produces: the module's bytes and the JSON beside it. */
type FaustArtifact = { cfactory: number; data: Uint8Array | number[]; json: string };

/** The Emscripten module the compiler lives in. Its heap is where the one
 *  interpreter's strings and handle arrays have to be copied to. */
type FaustModule = {
    libFaustWasm: new () => FaustLib;
    _malloc: (bytes: number) => number;
    _free: (at: number) => void;
    HEAP32: Int32Array;
};

type FaustLib = {
    version: () => string;
    createDSPFactory: (
        name: string,
        code: string,
        args: string,
        internalMemory: boolean,
    ) => FaustArtifact;
    createDSPFactoryFromBoxes: (
        name: string,
        box: number,
        args: string,
        internalMemory: boolean,
    ) => FaustArtifact;
    createDSPFactoryFromSignals: (
        name: string,
        signals: number,
        count: number,
        args: string,
        internalMemory: boolean,
    ) => FaustArtifact;
    createLibContext: () => void;
    destroyLibContext: () => void;
    deleteDSPFactory: (factory: number) => void;
    getErrorAfterException: () => string;
    cleanupAfterException: () => void;
};

/** The marshalling between the interpreter's memory and the compiler's. */
type FaustShim = {
    attach: (m: FaustModule, memory: WebAssembly.Memory) => void;
    beginScope: () => void;
    endScope: () => void;
};

let faust: Promise<{ lib: FaustLib; mod: FaustModule }> | null = null;
let faustShim: FaustShim | null = null;
/**
 * Whether the compiler currently has a lib context — the arena a box or signal
 * tree is built in.
 *
 * **It is opened and never closed, and that is deliberate.** The native path
 * brackets every def with `createLibContext`..`destroyLibContext`
 * (`faust::compiler`). Here, destroying it *poisons the next one*: a def built
 * afterwards loses the term merging Faust's hash-consing does, so a graph that
 * shares a subterm stops sharing it and a recursion over it never terminates —
 * reported as a stack overflow inside the compiler, with nothing pointing back
 * at the def that closed the previous context. Reproduced down to two defs (a
 * box with a `rec`, then anything recursive) and gone the moment the destroy
 * goes. So the page keeps one arena for its whole life instead, which also
 * costs less than one per def.
 *
 * Two things take it away underneath us, and both are tracked here rather than
 * discovered: compiling **source** allocates and destroys a context of its own
 * (Faust's `createFactory` does it), and so does the compiler's own cleanup
 * after an exception.
 */
let contextLive = false;

/**
 * Loads the compiler once, on the first `/def_send faust`, and points the
 * marshalling shim at it.
 *
 * The shim is what lets the *server's* JSON interpreters -- `faust::boxes` and
 * `faust::signals`, running in this Worker's own wasm -- drive this compiler:
 * their `Cbox*`/`Csig*` calls are imports of the NRT bundle, bound in
 * `faust-env.js` and marshalled in `faust-shim.js`. The two modules do not
 * share an address space, which is the whole reason the shim exists.
 */
function compiler(): Promise<{ lib: FaustLib; mod: FaustModule }> {
    if (faust === null) {
        faust = (async () => {
            const base = new URL("../vendor/faust/", import.meta.url);
            const glue = (await import(
                /* @vite-ignore */ new URL("libfaust-wasm.js", base).href
            )) as {
                default: (opts: { locateFile: (p: string) => string }) => Promise<FaustModule>;
            };
            // An ES module has no `document.currentScript`, so the glue cannot
            // find its own .wasm and .data on its own.
            const mod = await glue.default({
                locateFile: (p: string) => new URL(p, base).href,
            });
            // The interpreter's memory has to exist before the shim can read a
            // label out of it, and it is the decoder bundle's.
            await load();
            const shim = (await import(
                /* @vite-ignore */ new URL("../nrt/faust-shim.js", import.meta.url).href
            )) as FaustShim;
            shim.attach(mod, decoderMemory as WebAssembly.Memory);
            faustShim = shim;
            return { lib: new mod.libFaustWasm(), mod };
        })();
    }
    return faust;
}

/** A file being streamed: its shape, remembered so a span costs one read. */
const shapes = new Map<string, WavShape>();

/** A `/buffer_write` in progress: the runs handed over so far, held until the
 *  last one says the span is complete. */
const writes = new Map<string, Uint8Array<ArrayBuffer>>();

/** A recording in progress: what has been written, so the header can be fixed
 *  at the close. Kept in memory only as a byte count -- the samples go to
 *  storage as they arrive, which is the whole point of streaming them. */
const recordings = new Map<string, { bytes: Uint8Array<ArrayBuffer>; frames: number }>();

async function shapeOf(path: string): Promise<WavShape> {
    const known = shapes.get(path);
    if (known !== undefined) return known;
    const shape = parseShape(await readRange(path, 0, HEAD_BYTES));
    shapes.set(path, shape);
    return shape;
}

async function handle(request: Request, post: (r: Response, t?: Transferable[]) => void) {
    if (request.type === "ping") {
        post({ type: "ready" });
        return;
    }
    if (request.type === "shape") {
        try {
            const shape = await shapeOf(request.path);
            post({
                type: "shape",
                ticket: request.ticket,
                channels: shape.channels,
                sampleRate: shape.sampleRate,
                frames: shape.frames,
            });
        } catch (e) {
            post({ type: "shape", ticket: request.ticket, error: message(e) });
        }
        return;
    }
    if (request.type === "span") {
        try {
            const shape = await shapeOf(request.path);
            const at = shape.dataOffset + request.frame * shape.blockAlign;
            const want = request.frames * shape.blockAlign;
            const body = await readRange(request.path, at, want);
            // A span is not a file until it has a header of its own -- and it
            // has to become one, because the decode belongs to the server's
            // decoder rather than to anything written here.
            const { decodeAudio } = await load();
            const decoded = decodeAudio(
                wrapSpan(shape, body),
                "wav",
                request.path,
                0,
                0,
                new Uint32Array(0),
            );
            post(
                {
                    type: "span",
                    ticket: request.ticket,
                    samples: decoded.samples,
                    frames: decoded.frames,
                },
                [decoded.samples.buffer],
            );
        } catch (e) {
            post({ type: "span", ticket: request.ticket, error: message(e) });
        }
        return;
    }
    if (request.type === "faust") {
        try {
            const result = await compile(request);
            post(result, "bytes" in result ? [result.bytes] : []);
        } catch (e) {
            post({ type: "faust", ticket: request.ticket, error: message(e) });
        }
        return;
    }
    if (request.type === "faust-heap") {
        // Zero until something has compiled: the compiler is imported on the
        // first def, and asking must not be what brings it in.
        const held = faust === null ? 0 : (await faust).mod.HEAP32.buffer.byteLength;
        post({ type: "faust-heap", ticket: request.ticket, bytes: held, reloads });
        return;
    }
    if (request.type === "write") {
        try {
            const { encodeWavFrames, wavHeader } = await load();
            const held = writes.get(request.path) ?? new Uint8Array(0);
            const body = request.samples.length > 0
                ? encodeWavFrames(request.samples, request.format)
                : new Uint8Array(0);
            const grown = new Uint8Array(held.byteLength + body.byteLength);
            grown.set(held);
            grown.set(body, held.byteLength);
            if (!request.final) {
                // No acknowledgement per run, only at the end: the runs are
                // ordered by the port itself, and an ack for an early run
                // arriving after the last one was posted would answer the
                // command before the file exists.
                writes.set(request.path, grown);
                return;
            }
            writes.delete(request.path);
            // The framing is the server crate's, bound here rather than written
            // in TypeScript: a second int16 rounding is a second answer, and a
            // file that differs by a bit between a tab and a window is exactly
            // the divergence nothing names.
            const header = wavHeader(
                request.channels,
                request.sampleRate,
                request.format,
                grown.byteLength,
            );
            const file = new Uint8Array(header.byteLength + grown.byteLength);
            file.set(header);
            file.set(grown, header.byteLength);
            await writeFile(request.path, file);
            post({ type: "write", ticket: request.ticket });
        } catch (e) {
            writes.delete(request.path);
            post({ type: "write", ticket: request.ticket, error: message(e) });
        }
        return;
    }
    if (request.type === "record") {
        try {
            const { encodeWavFrames, wavHeader } = await load();
            let take = recordings.get(request.path);
            if (take === undefined) {
                take = { bytes: new Uint8Array(0), frames: 0 };
                recordings.set(request.path, take);
            }
            if (request.samples.length > 0) {
                const body = encodeWavFrames(request.samples, request.format);
                const grown = new Uint8Array(take.bytes.byteLength + body.byteLength);
                grown.set(take.bytes);
                grown.set(body, take.bytes.byteLength);
                take.bytes = grown;
                take.frames += request.samples.length / request.channels;
            }
            // Rewritten whole each time rather than appended: OPFS gives a
            // sync handle, not an append, and a take that is only correct once
            // it ends is a take that is lost when a tab closes. This costs a
            // rewrite per flush and buys a file that is valid at every moment.
            const header = wavHeader(
                request.channels,
                request.sampleRate,
                request.format,
                take.bytes.byteLength,
            );
            const file = new Uint8Array(header.byteLength + take.bytes.byteLength);
            file.set(header);
            file.set(take.bytes, header.byteLength);
            await writeFile(request.path, file);
            if (request.final) recordings.delete(request.path);
            post({ type: "record", ticket: request.ticket });
        } catch (e) {
            post({ type: "record", ticket: request.ticket, error: message(e) });
        }
        return;
    }
    const { ticket, path } = request;
    try {
        const bytes = await readFile(path);
        const { decodeAudio } = await load();
        const decoded = decodeAudio(
            bytes,
            extensionOf(path),
            path,
            request.fileStart,
            request.numFrames,
            Uint32Array.from(request.channels),
        );
        post(
            {
                type: "read",
                ticket,
                samples: decoded.samples,
                channels: decoded.channels,
                frames: decoded.frames,
                sampleRate: decoded.sampleRate,
            },
            [decoded.samples.buffer],
        );
    } catch (e) {
        post({ type: "read", ticket, error: message(e) });
    }
}

/**
 * What a compilation that ran out of **call stack** says.
 *
 * Not the arena and not Emscripten's shadow stack (the compiler is linked with
 * `STACK_SIZE=8MB`, the same order as a native thread's): wasm frames sit on
 * the JavaScript engine's own stack, which is about a megabyte, and libfaust
 * recurses over the term graph of everything compiled so far. A native server
 * never meets this — it gets a whole thread's stack, and a fresh lib context
 * per def besides.
 */
const OUT_OF_STACK = /stack overflow|call stack size exceeded/i;

/**
 * Throws the compiler away, so the next def is compiled by a fresh instance.
 *
 * This is the one way out of an exhausted stack that the page actually has.
 * The context cannot be destroyed and reopened — that poisons the next def
 * (see `contextLive`) — but a whole new Emscripten instance has a new memory,
 * a new arena and a new context, and poisons nothing, because the poisoning
 * was always a *destroyed* context and not a second one. It costs an
 * instantiation, and that is cheaper than it looks: the fetch is in cache and
 * the module is already compiled, so twelve distinct recursive signal defs in a
 * row — six of these between them — average 18 ms each, against the 9 ms a def
 * that never overflows costs (`tests/faust-arena.html`). `/def_send faust` has
 * always answered late besides.
 */
function discardCompiler(): void {
    faust = null;
    faustShim = null;
    contextLive = false;
    reloads++;
}

/** How many compilers this Worker has been through — reported by
 *  `faust-heap`, and how `tests/faust-arena.html` says what a tab pays. */
let reloads = 0;

/**
 * Compiles one def and hands back a module the engine can link.
 *
 * `internalMemory` is false: the module must import `env.memory` so it can be
 * instantiated against the engine's own. `-ftz 2` is the same flag a native
 * factory is compiled with, so a decaying tail cannot strand either thread in
 * subnormal math — the architecture-independent half of the denormal rule, and
 * one of the places the two builds have to agree exactly.
 *
 * A def that exhausts the call stack is compiled again in a **fresh compiler**
 * and only then reported as failed: the accumulated term graph is what ran the
 * stack out, and nothing about the def itself was wrong. Retried once, never
 * twice — a def that overflows a compiler that has compiled nothing is a def
 * too deep for a tab, and saying so is the honest answer.
 */
async function compile(request: FaustRequest): Promise<Response> {
    const first = await compileOnce(request);
    const failure = first.type === "faust" && "error" in first ? first.error : "";
    if (!OUT_OF_STACK.test(failure)) return first;
    discardCompiler();
    return compileOnce(request);
}

async function compileOnce(request: FaustRequest): Promise<Response> {
    const { ticket, name } = request;
    const { lib, mod } = await compiler();
    let out: FaustArtifact;
    try {
        out = await factory(lib, mod, request);
    } catch (e) {
        // Two reports, and neither is enough alone. The compiler keeps its own
        // message and hands it over here; but when the throw is *ours* -- the
        // marshalling refusing something -- that message says "stack overflow",
        // which is what `getErrorAfterException` answers for any exception it
        // did not raise itself. So both travel, and the def's name with them.
        const inside = lib.getErrorAfterException().trim();
        const outside = message(e).trim();
        // `cleanupAfterException` is `global::destroy()` under another name.
        lib.cleanupAfterException();
        contextLive = false;
        const error = inside && inside !== outside ? `${inside} (${outside})` : outside || inside;
        return { type: "faust", ticket, error };
    }
    if (!out.cfactory) {
        const error = lib.getErrorAfterException() || "the Faust compiler produced no factory";
        lib.cleanupAfterException();
        return { type: "faust", ticket, error };
    }
    // The factory has given up its binary; the compiler need not keep it.
    lib.deleteDSPFactory(out.cfactory);
    const { stripFaustData } = await load();
    const stripped = stripFaustData(Uint8Array.from(out.data));
    // The bytes travel, not a `WebAssembly.Module`. A Module is
    // structured-cloneable and posting one into an AudioWorklet *appears* to
    // work — the send succeeds and the message is then dropped on arrival, with
    // no error raised on either side — because a worklet is not in this
    // Worker's agent cluster. So the worklet compiles them itself: a Faust
    // module is a couple of kilobytes, which is microseconds once per def,
    // against a silence nothing reports.
    const bytes = stripped.buffer.slice(
        stripped.byteOffset,
        stripped.byteOffset + stripped.byteLength,
    ) as ArrayBuffer;
    return { type: "faust", ticket, bytes, json: out.json };
}

/** The compiler arguments both builds use. `-ftz 2` is the one that matters. */
const FAUST_ARGS = "-ftz 2";

/**
 * The three def formats, each to its own entry point — the same three a native
 * server has, and the reason the whole shim exists.
 *
 * Source goes straight in. A box tree and a signal tree are read by
 * `faust::boxes` and `faust::signals`, in this Worker's wasm: the *server's*
 * interpreters, not a second reading of the schema written in TypeScript. What
 * they build lives in the compiler's arena, so it is only valid between
 * `createLibContext` and `destroyLibContext` — the factory is made inside that
 * bracket too, exactly as the native path does it.
 */
async function factory(
    lib: FaustLib,
    mod: FaustModule,
    request: FaustRequest,
): Promise<FaustArtifact> {
    const { name, def } = request;
    const { faustBoxFromJson, faustSignalsFromJson } = await load();
    // Everything the interpreter hands the compiler -- a label, an `fconst`
    // name, a waveform's values -- has to outlive the whole construction, not
    // the call that took it. The scope is what holds it; see faust-shim.js,
    // which says what happens when it does not.
    faustShim?.beginScope();
    if (!contextLive) {
        lib.createLibContext();
        contextLive = true;
    }
    try {
        if (request.kind === "signals") {
            const handles = faustSignalsFromJson(def);
            // The outputs are the compiler's own handles; it wants them in its
            // own heap, one word each.
            const at = mod._malloc(handles.length * 4);
            try {
                for (let i = 0; i < handles.length; i++) mod.HEAP32[at / 4 + i] = handles[i]!;
                return lib.createDSPFactoryFromSignals(
                    name,
                    at,
                    handles.length,
                    FAUST_ARGS,
                    false,
                );
            } finally {
                mod._free(at);
            }
        }
        // Source included: a program becomes a box through the schema's own
        // escape hatch (`{"op": "faust", "src": …}`, which is `CDSPToBoxes`),
        // so all three formats reach the compiler the same way and the page
        // keeps one arena. Compiling source through `createDSPFactory` instead
        // works, but it allocates and destroys a context of its own, and a
        // destroyed context is what poisons the next one -- see `contextLive`.
        const tree = request.kind === "source" ? JSON.stringify({ op: "faust", src: def }) : def;
        return lib.createDSPFactoryFromBoxes(name, faustBoxFromJson(tree), FAUST_ARGS, false);
    } finally {
        // No `destroyLibContext` here: see `contextLive`.
        faustShim?.endScope();
    }
}

const message = (e: unknown) => String(e instanceof Error ? e.message : e);

// Two doors, the same handler behind both. The worklet's port is the one that
// matters (it skips the main thread's event loop, which is busy drawing);
// `self` is the fallback for a browser that will not transfer a port into an
// AudioWorklet, and for the boot handshake that finds out which it is.
self.onmessage = (event: MessageEvent) => {
    const data = event.data as Request | { type: "port"; port: MessagePort };
    if (data.type === "port") {
        const port = data.port;
        port.onmessage = (e: MessageEvent) =>
            void handle(e.data as Request, (r, t) => port.postMessage(r, t ?? []));
        port.start();
        return;
    }
    void handle(data, (r, t) => (self as unknown as Worker).postMessage(r, t ?? []));
};
