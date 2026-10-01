// The server's soundfile codec, outside the engine: the WAV a render writes and
// the decoder a soundfile is read back with.
//
// Both are the server crate's own, bound once in `clausters-nrt-web` -- the
// module the NRT worker already decodes `/buffer_allocRead` with and frames
// `/buffer_write` with -- and reached here from the thread that renders. A
// second int16 rounding, or a second WAV reader, would be a second answer: a
// file that differs by a bit between a tab and a window is the divergence
// nothing names.
//
// Loaded on demand and memoized, like the renderer: a page that never writes a
// take never fetches it. In a tab the glue fetches its own module; under node
// it is read from where this package's build puts it.

import { underNode } from "../base/files.ts";

/** What the codec module exposes, as this client uses it. */
interface Codec {
    initSync: (module: unknown) => unknown;
    default: () => Promise<unknown>;
    encodeWavFrames: (samples: Float32Array, sampleFormat: string) => Uint8Array;
    wavHeader: (channels: number, sampleRate: number, sampleFormat: string, dataBytes: number) => Uint8Array;
    decodeAudio: (
        bytes: Uint8Array,
        ext: string,
        label: string,
        fileStart: number,
        numFrames: number,
        channels: Uint32Array,
    ) => { samples: Float32Array; channels: number; frames: number; sampleRate: number };
}

let loaded: Promise<Codec> | null = null;

/** The codec module, loaded once. */
function codec(): Promise<Codec> {
    loaded ??= (async () => {
        if (!underNode()) {
            const glue = (await import(
                /* @vite-ignore */ new URL("../nrt/clausters_nrt_web.js", import.meta.url).href
            )) as Codec;
            await glue.default();
            return glue;
        }
        // The emitted package (`dist/engine/codec.js`, the module under
        // `dist/nrt/`) and the sources it was emitted from (`src/engine/`,
        // where the module is still only in `dist/`): both this package.
        const { readFile } = await import("node:fs/promises");
        for (const base of ["../nrt/", "../../dist/nrt/"]) {
            const wasm = new URL(`${base}clausters_nrt_web_bg.wasm`, import.meta.url);
            let bytes: Uint8Array;
            try {
                bytes = await readFile(wasm);
            } catch (error) {
                if ((error as { code?: string }).code === "ENOENT") continue;
                throw error;
            }
            const glue = (await import(
                new URL(`${base}clausters_nrt_web.js`, import.meta.url).href
            )) as Codec;
            glue.initSync({ module: bytes });
            return glue;
        }
        throw new Error("the soundfile codec is not staged -- run clients/web/build.sh");
    })();
    return loaded;
}

/**
 * A whole WAV file of interleaved `samples`, in `sampleFormat` (`"int16"`,
 * `"int24"` or `"float"`): the server's own framing and the server's own
 * conversion, so the file is the one a native render writes.
 */
export async function encodeWav(
    samples: Float32Array,
    channels: number,
    sampleRate: number,
    sampleFormat: string,
): Promise<Uint8Array<ArrayBuffer>> {
    const module = await codec();
    const body = module.encodeWavFrames(samples, sampleFormat);
    const head = module.wavHeader(channels, Math.round(sampleRate), sampleFormat, body.byteLength);
    const file = new Uint8Array(head.byteLength + body.byteLength);
    file.set(head);
    file.set(body, head.byteLength);
    return file;
}

/**
 * A soundfile's samples through the server's own decoder: `frames` frames
 * from `start` (`frames <= 0` to the end), interleaved, at the file's rate.
 * `label` names the file in an error; the format is probed from `path`'s
 * extension and then from the bytes.
 */
export async function decodeSoundfile(
    bytes: Uint8Array,
    path: string,
    start: number,
    frames: number,
): Promise<{ samples: Float32Array; channels: number; frames: number; sampleRate: number }> {
    const module = await codec();
    const dot = path.lastIndexOf(".");
    const ext = dot > path.lastIndexOf("/") ? path.slice(dot + 1).toLowerCase() : "";
    const decoded = module.decodeAudio(bytes, ext, path, start, frames, new Uint32Array());
    return {
        samples: decoded.samples,
        channels: decoded.channels,
        frames: decoded.frames,
        sampleRate: decoded.sampleRate,
    };
}
