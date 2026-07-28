// Audio taps, with client-side allocation.
//
// A tap is one of the server's pre-allocated sample rings (`--taps`, 8 by
// default): `/tap tapIndex bus` routes an audio bus into it, and from the next
// block on the engine appends that bus's samples there — where a GUI host
// reads them out of shared memory, or this client streams them with
// `Server.streamTaps`. `/tap tapIndex -1` stops it.
//
// Like buses and buffers, taps are a finite boot-time resource the client
// allocates and the server merely indexes, so they get the same registry (the
// core's occupancy map): a freed run is reusable, a double free is refused
// loudly, exhaustion throws instead of wrapping. A stereo view holds two
// **adjacent** taps — a phasescope reads `t` and `t + 1` — which is why the
// allocator hands out runs rather than single indices.

import { AllocationError } from "../errors.ts";
import { Registry } from "../base/core.ts";

/** The server's default tap count (`--taps`), when it reports none. */
export const DEFAULT_TAPS = 8;

export class TapAllocator {
    readonly size: number;
    // A server built without a tap region (`--taps 0`) leaves no registry:
    // `alloc` reports exhaustion from the first call.
    private registry: Registry | null;

    constructor(size = DEFAULT_TAPS) {
        this.size = size;
        this.registry = size > 0 ? new Registry(0, size) : null;
    }

    /**
     * The first index of a run of `count` adjacent taps; throws when no such
     * run is free (or when the server has no tap region at all).
     */
    alloc(count = 1): number {
        const first = this.registry?.alloc(count);
        if (first === undefined) {
            throw new AllocationError(
                this.size > 0
                    ? `out of audio taps (${this.size} rings)`
                    : "this server has no audio taps (--taps 0)",
            );
        }
        return first;
    }

    /**
     * Returns a run to the pool. A double free (or a run this allocator never
     * handed out) throws — a lost tap is a client bug, never absorbed.
     */
    free(first: number, count = 1): void {
        if (!this.registry?.release(first, count)) {
            throw new AllocationError(
                `double free of tap ${first} (count=${count}): ` +
                    "not currently allocated here",
            );
        }
    }

    /** How many taps are currently allocated. */
    get inUse(): number {
        return this.registry?.inUse ?? 0;
    }
}
