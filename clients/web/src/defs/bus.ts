// Audio and control buses, with client-side allocation.
//
// Mirrors the server's bus model: audio buses (`0..channels` are the hardware
// outputs) and single-float control buses. Like scsynth, the client owns
// allocation; the server just indexes. A `Bus` is a flat
// `(index, channels, rate)` — only flat data ever leaves for the wire.
//
// Buses are a finite boot-time resource, so each allocator is a registry (the
// core's occupancy map): a freed run is always reusable, adjacent runs
// coalesce, a double free is refused loudly, and exhaustion throws instead of
// wrapping. The allocatable space excludes the hardware outputs at the bottom
// (`reserved`) and the GraphDef private-bus range at the top (the core's
// `graphBusReserved()`, clamped to the space) — those belong to the server's
// own registry.
//
// The allocators carry no default size of their own: how many buses exist is
// a property of the server, not of this module. The `Server` sizes them from
// its options, and the live counts can be read back with `queryInfo`.

import { AllocationError } from "../errors.ts";
import { Registry, graphBusReserved } from "../base/core.ts";

export type BusRate = "audio" | "control";

export class Bus {
    readonly index: number;
    readonly channels: number;
    readonly rate: BusRate;

    constructor(index: number, channels = 1, rate: BusRate = "audio") {
        this.index = index;
        this.channels = channels;
        this.rate = rate;
    }
}

/// Anything a command can address by bus index: a handle or the bare number.
export type BusLike = Bus | number;

/// The index behind a bus handle or a bare number.
export function busIndex(bus: BusLike): number {
    return typeof bus === "number" ? bus : bus.index;
}

class Allocator {
    readonly rate: BusRate;
    readonly size: number;
    // A space the reservations swallow whole leaves no registry: `alloc`
    // reports exhaustion from the first call.
    private registry: Registry | null;

    constructor(
        rate: BusRate,
        size: number,
        reserved: number,
        graphReserved: number,
    ) {
        this.rate = rate;
        this.size = size;
        const top = size - Math.min(graphReserved, size);
        const span = Math.max(0, top - reserved);
        this.registry = span > 0 ? new Registry(reserved, span) : null;
    }

    /// A run of `channels` contiguous buses. Throws when no such run is free
    /// — exhaustion is an explicit failure, never an aliased index.
    alloc(channels = 1): Bus {
        const index = this.registry?.alloc(channels);
        if (index === undefined) {
            throw new AllocationError(`out of ${this.rate} buses`);
        }
        return new Bus(index, channels, this.rate);
    }

    /// Returns the bus's run to the pool. A double free (or a bus this
    /// allocator never handed out) throws — losing track of a bus is a client
    /// bug, never absorbed silently.
    free(bus: Bus): void {
        if (!this.registry?.release(bus.index, bus.channels)) {
            throw new AllocationError(
                `double free of ${this.rate} bus ${bus.index} ` +
                    `(channels=${bus.channels}): not currently allocated here`,
            );
        }
    }

    /// How many buses are currently allocated.
    get inUse(): number {
        return this.registry?.inUse ?? 0;
    }
}

/// Allocates audio buses above the hardware outputs (`reserved`) and below
/// the GraphDef private range. `size` is the server's audio-bus count.
export class AudioBusAllocator extends Allocator {
    constructor(size: number, reserved = 2) {
        super("audio", size, reserved, graphBusReserved()[0]);
    }
}

/// `size` is the server's control-bus count.
export class ControlBusAllocator extends Allocator {
    constructor(size: number) {
        super("control", size, 0, graphBusReserved()[1]);
    }
}
