// The NRT worker: the browser's version of the thread that is neither audio
// nor interface.
//
// A native server has one for exactly this work — reading a soundfile,
// decoding it, building the buffer — off both the audio thread and the network
// one. The engine in a page had no such thread, so the work landed on the
// AudioWorklet, which owes a block of audio every 2.67 ms. This is that thread.
//
// It does two things and holds nothing:
//
//  - reads a file out of the page's own filesystem (OPFS, reachable only from
//    a dedicated Worker — see `./opfs.ts`), and
//  - decodes it with **our** decoder, the same one a native server runs.
//    `decodeAudioData` is right there and is the wrong answer: it is a
//    different decoder, so the same file would become different samples in a
//    tab and in a window, and a divergence in values is the kind nothing names.
//
// The samples go back as a **transferred** buffer, so crossing the thread
// boundary moves them rather than copying them. Installing them is the
// worklet's business and is paced there, a run at a time.

/// <reference lib="webworker" />
import { extensionOf, readFile } from "./opfs.ts";

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

type Request = ReadRequest | { type: "ping" };

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

async function handle(request: Request, post: (r: Response, t?: Transferable[]) => void) {
    if (request.type === "ping") {
        post({ type: "ready" });
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
        post({ type: "read", ticket, error: String(e instanceof Error ? e.message : e) });
    }
}

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
