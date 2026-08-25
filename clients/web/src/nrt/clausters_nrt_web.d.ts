/* tslint:disable */
/* eslint-disable */

/**
 * One decoded soundfile: interleaved `f32` samples plus the shape they are in.
 *
 * The samples come out as their own `Float32Array`, which is what lets the
 * Worker **transfer** them to the worklet rather than copy them across.
 */
export class Decoded {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    readonly channels: number;
    readonly frames: number;
    readonly sampleRate: number;
    /**
     * Interleaved samples, `frames * channels` of them. Moves the vector out,
     * so a second call returns nothing — the buffer is meant to be handed on.
     */
    readonly samples: Float32Array;
}

/**
 * Decodes a soundfile already in memory — the Worker's whole job.
 *
 * `ext` is the format hint (`"wav"`, `"flac"`, …, no dot; an empty hint still
 * probes by content). `label` names the source in an error. `file_start` and
 * `num_frames` slice it exactly as `/buffer_allocRead` does, with
 * `num_frames <= 0` meaning "to the end", and `channels` selects and reorders
 * them exactly as `/buffer_allocReadChannel` does — empty being every channel.
 *
 * The selection goes through the server's own `select_channels` rather than a
 * de-interleave written here: one rule, one implementation, or the two clients
 * come to disagree about what `[1, 0]` means.
 *
 * Fails with the decoder's own message, which is the one a native server would
 * have replied with.
 */
export function decodeAudio(bytes: Uint8Array, ext: string, label: string, file_start: number, num_frames: number, channels: Uint32Array): Decoded;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_decoded_free: (a: number, b: number) => void;
    readonly decodeAudio: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number, number];
    readonly decoded_channels: (a: number) => number;
    readonly decoded_frames: (a: number) => number;
    readonly decoded_sampleRate: (a: number) => number;
    readonly decoded_samples: (a: number) => [number, number];
    readonly clausters_abi_version: () => number;
    readonly clausters_free_samples: (a: number, b: bigint) => void;
    readonly clausters_read_soundfile: (a: number, b: bigint, c: bigint, d: number, e: number, f: number, g: number, h: number) => number;
    readonly clausters_render: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
