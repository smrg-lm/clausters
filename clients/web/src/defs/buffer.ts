// Buffers, with client-side index allocation.
//
// The server's buffer pool is a finite boot-time resource (`--max-buffers`),
// indices allocated by the client (like scsynth). `Buffer` is a flat handle;
// the actual allocation/loading happens on the server via
// `/b_alloc`/`/b_allocRead`/… driven by `Server`.
//
// The allocator is a registry (the core's occupancy map): a freed slot is
// always reusable, a double free is refused loudly, exhaustion throws instead
// of wrapping. The `Server` sizes it from its options (`maxBuffers`).

import { AllocationError } from "../errors.ts";
import { Registry } from "../base/core.ts";

export const NUM_BUFFERS = 4096;

export class Buffer {
    readonly bufnum: number;
    readonly frames: number;
    readonly channels: number;
    readonly sampleRate: number;

    constructor(bufnum: number, frames = 0, channels = 1, sampleRate = 0.0) {
        this.bufnum = bufnum;
        this.frames = frames;
        this.channels = channels;
        this.sampleRate = sampleRate;
    }
}

/// Anything a command can address by buffer number: a handle or the number.
export type BufferLike = Buffer | number;

/// The index behind a buffer handle or a bare number.
export function bufferNumber(buf: BufferLike): number {
    return typeof buf === "number" ? buf : buf.bufnum;
}

export class BufferAllocator {
    readonly size: number;
    private registry: Registry;

    constructor(size = NUM_BUFFERS) {
        this.size = size;
        this.registry = new Registry(0, size);
    }

    /// A free buffer index; throws when the pool is exhausted.
    alloc(): number {
        const bufnum = this.registry.alloc(1);
        if (bufnum === undefined) {
            throw new AllocationError("out of buffer slots");
        }
        return bufnum;
    }

    /// Returns `bufnum` to the pool. A double free (or an index this
    /// allocator never handed out) throws — a lost buffer slot is a client
    /// bug, never absorbed silently.
    free(bufnum: number): void {
        if (!this.registry.release(bufnum, 1)) {
            throw new AllocationError(
                `double free of buffer ${bufnum}: not currently allocated`,
            );
        }
    }

    /// How many buffer slots are currently allocated.
    get inUse(): number {
        return this.registry.inUse;
    }
}
