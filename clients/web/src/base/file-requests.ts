/**
 * **A file call of the engine's Rust, as it sits in the wasm memory**: the
 * layout of `clausters_core::files::Request` (`repr(C)` on wasm32) and the
 * answer written back into it.
 *
 * Two places answer these: the page's file worker, for an editing
 * application's threads over shared memory (`../gui/apps-files.ts`), and
 * the page's render Worker, for the offline render it runs on its own
 * thread (`../engine/file-render.ts`). The Rust fills a request, hands its
 * address over and reads the answer once the request's state says it is
 * done; what differs is only who carries the call out, and when.
 *
 * @module
 */

/** The request's fields, by their offsets in it. */
export const field = {
    state: 0,
    op: 4,
    handle: 8,
    len: 12,
    buf: 16,
    path: 20,
    pathLen: 24,
    path2: 28,
    path2Len: 32,
    at: 40,
    result: 48,
    modified: 56,
    isDir: 64,
    error: 68,
    errorCap: 72,
    errorLen: 76,
} as const;

/** What a request asks, as `clausters_core::files::Op` numbers it. */
export const fileOp = {
    open: 1, create: 2, read: 3, write: 4, len: 5, close: 6, rename: 7, removeFile: 8,
    removeDir: 9, removeDirAll: 10, createDir: 11, createDirAll: 12, metadata: 13, readDir: 14,
    holdNew: 15, release: 16, sweep: 17,
} as const;

/** The failures a request answers by number, as `files.rs` reads them. */
export const FAILED = -1;
export const NOT_FOUND = -2;
export const EXISTS = -3;
export const NOT_EMPTY = -4;

/** The state of a request answered. */
const DONE = 1;

/** A failure answered with its number and its reason. */
export class Refused extends Error {
    readonly code: number;

    constructor(code: number, message: string) {
        super(message);
        this.code = code;
    }
}

/** **One request in the memory**, read and answered through its fields. */
export class FileRequest {
    private readonly memory: WebAssembly.Memory;
    readonly at: number;

    constructor(memory: WebAssembly.Memory, at: number) {
        this.memory = memory;
        this.at = at;
    }

    /** The memory as it is now (it may have grown since the last look). */
    private view(): DataView {
        return new DataView(this.memory.buffer);
    }

    u32(offset: number): number {
        return this.view().getUint32(this.at + offset, true);
    }

    f64(offset: number): number {
        return this.view().getFloat64(this.at + offset, true);
    }

    setF64(offset: number, value: number): void {
        this.view().setFloat64(this.at + offset, value, true);
    }

    setU32(offset: number, value: number): void {
        this.view().setUint32(this.at + offset, value, true);
    }

    /** `len` bytes of the memory at `ptr`, viewed in place. */
    bytes(ptr: number, len: number): Uint8Array {
        return new Uint8Array(this.memory.buffer, ptr, len);
    }

    get op(): number {
        return this.u32(field.op);
    }

    get path(): string {
        return this.text(field.path, field.pathLen);
    }

    get path2(): string {
        return this.text(field.path2, field.path2Len);
    }

    /** The bytes a read fills or a write takes. */
    get buffer(): Uint8Array {
        return this.bytes(this.u32(field.buf), this.u32(field.len));
    }

    private text(ptr: number, len: number): string {
        return new TextDecoder().decode(this.bytes(this.u32(ptr), this.u32(len)).slice());
    }

    /**
     * **Answers the request** with `result`, or with the failure `error`
     * comes to, and marks it done, waking a thread that waits on it.
     */
    answer(result: number | { error: unknown }): void {
        let value: number;
        if (typeof result === "number") {
            value = result;
        } else {
            const refused = refusal(result.error, this.path);
            value = refused.code;
            const reason = new TextEncoder().encode(refused.message);
            const len = Math.min(reason.length, this.u32(field.errorCap));
            this.bytes(this.u32(field.error), len).set(reason.subarray(0, len));
            this.setU32(field.errorLen, len);
        }
        this.setF64(field.result, value);
        const state = new Int32Array(this.memory.buffer);
        Atomics.store(state, (this.at + field.state) >> 2, DONE);
        Atomics.notify(state, (this.at + field.state) >> 2);
    }
}

/** The storage's failures, in the numbers a request answers with. */
function refusal(error: unknown, path: string): Refused {
    if (error instanceof Refused) return error;
    const name = (error as { name?: string })?.name;
    const message = `${path}: ${(error as Error)?.message ?? String(error)}`;
    if (name === "NotFoundError" || name === "TypeMismatchError") return new Refused(NOT_FOUND, message);
    if (name === "InvalidModificationError") return new Refused(NOT_EMPTY, message);
    return new Refused(FAILED, message);
}
