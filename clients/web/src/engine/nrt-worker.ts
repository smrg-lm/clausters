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

type Request =
    | ReadRequest
    | ShapeRequest
    | SpanRequest
    | RecordRequest
    | FaustRequest
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
};

let decoder: Promise<Decoder> | null = null;

/** Loads the decoder once, on the first read — a page that never reads a
 *  soundfile never fetches it. */
function load(): Promise<Decoder> {
    if (decoder === null) {
        decoder = (async () => {
            const mod = (await import(
                /* @vite-ignore */ new URL("../nrt/clausters_nrt_web.js", import.meta.url).href
            )) as Decoder;
            await mod.default();
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
type FaustLib = {
    version: () => string;
    createDSPFactory: (
        name: string,
        code: string,
        args: string,
        internalMemory: boolean,
    ) => { cfactory: number; data: Uint8Array | number[]; json: string };
    deleteDSPFactory: (factory: number) => void;
    getErrorAfterException: () => string;
    cleanupAfterException: () => void;
};

let faust: Promise<FaustLib> | null = null;

/** Loads the compiler once, on the first `/def_send faust`. */
function compiler(): Promise<FaustLib> {
    if (faust === null) {
        faust = (async () => {
            const base = new URL("../vendor/faust/", import.meta.url);
            type Emscripten = { libFaustWasm: new () => FaustLib };
            const glue = (await import(
                /* @vite-ignore */ new URL("libfaust-wasm.js", base).href
            )) as {
                default: (opts: { locateFile: (p: string) => string }) => Promise<Emscripten>;
            };
            // An ES module has no `document.currentScript`, so the glue cannot
            // find its own .wasm and .data on its own.
            const mod = await glue.default({
                locateFile: (p: string) => new URL(p, base).href,
            });
            return new mod.libFaustWasm();
        })();
    }
    return faust;
}

/** A file being streamed: its shape, remembered so a span costs one read. */
const shapes = new Map<string, WavShape>();

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
 * Compiles one def and hands back a module the engine can link.
 *
 * `internalMemory` is false: the module must import `env.memory` so it can be
 * instantiated against the engine's own. `-ftz 2` is the same flag a native
 * factory is compiled with, so a decaying tail cannot strand either thread in
 * subnormal math — the architecture-independent half of the denormal rule, and
 * one of the places the two builds have to agree exactly.
 */
async function compile(request: FaustRequest): Promise<Response> {
    const { ticket, name } = request;
    if (request.kind !== "source") {
        // The vendored compiler exposes `createDSPFactory` and nothing else:
        // no Box API, no Signal API. Both are Faust's own C API and neither is
        // in its Emscripten bindings, so a def built from the signal or box
        // surface cannot be compiled in a page yet. Named rather than
        // swallowed — see clients/web/docs/src/platform.md.
        return {
            type: "faust",
            ticket,
            error: `a Faust def built from ${request.kind} cannot be compiled in a page yet; \
send it as source`,
        };
    }
    const lib = await compiler();
    let out: { cfactory: number; data: Uint8Array | number[]; json: string };
    try {
        out = lib.createDSPFactory(name, request.def, "-ftz 2", false);
    } catch (e) {
        lib.cleanupAfterException();
        return { type: "faust", ticket, error: lib.getErrorAfterException() || message(e) };
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
