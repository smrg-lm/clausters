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
//
// A bus holds the server it was allocated on and owns the commands addressed
// to it: `set`, `get`, `watch` and its own release. The subscriptions over a
// *set* of buses (`/bus_stream`, `/bus_tapStream`) stay on the server, which is
// whose they are — one per client.

import { AllocationError } from "../errors.ts";
import { Registry, graphBusReserved } from "../base/core.ts";
import type { Server } from "./server/index.ts";

export type BusRate = "audio" | "control";

export class Bus {
    readonly index: number;
    readonly channels: number;
    readonly rate: BusRate;
    /**
     * The server this bus was allocated on (set by `Bus.audio` / `Bus.control`),
     * so its commands know where to go without being told.
     */
    readonly server?: Server;

    constructor(index: number, channels = 1, rate: BusRate = "audio", server?: Server) {
        this.index = index;
        this.channels = channels;
        this.rate = rate;
        this.server = server;
    }

    /** A run of `channels` contiguous audio buses from the server's pool. */
    static audio(server: Server, channels = 1): Bus {
        return server.audioBuses.alloc(channels, server);
    }

    /** A run of `channels` contiguous control buses from the server's pool. */
    static control(server: Server, channels = 1): Bus {
        return server.controlBuses.alloc(channels, server);
    }

    /** This bus's server, or a clear failure when the handle carries none. */
    private srv(): Server {
        if (!this.server) {
            throw new Error(
                `bus ${this.index} has no server: build the handle with one, ` +
                    `e.g. new Bus(${this.index}, ${this.channels}, "${this.rate}", server)`,
            );
        }
        return this.server;
    }

    /** Sets this control bus's value (`/bus_set`). */
    set(value: number): void {
        this.srv().sendMsg("/bus_set", ["i", this.index], ["f", value]);
    }

    /** Reads this control bus's value (`/bus_get`). */
    async get(timeout?: number): Promise<number> {
        const msg = await this.srv().request("/bus_get", [["i", this.index]], {
            expect: ["/bus_get.reply"],
            timeout,
        });
        return Number(msg.args.at(-1));
    }

    /**
     * Asks the server to make this audio bus readable (`/bus_tap`): from the next
     * block on, the engine records it into the shared segment, where a GUI
     * host reads it with zero messages and this client streams it with
     * `Server.streamTaps`. `flag = false` stops.
     *
     * **The bus is the only number you name.** Which of the server's finite
     * sample rings carries it is the server's own bookkeeping, published in
     * the segment for whoever reads the samples. Watches count, so two views
     * of one bus share a ring and the last one to stop frees it. No ack, like
     * `/node_map` (failures reply `/fail` — an unknown bus, no tap region, or
     * every ring already taken); sequence with `sync` when it matters.
     */
    watch(flag = true): void {
        this.srv().sendMsg("/bus_tap", ["i", Math.trunc(this.index)], ["i", flag ? 1 : 0]);
    }

    /** Returns this bus's run to the server's pool. */
    free(): void {
        const server = this.srv();
        if (this.rate === "audio") server.audioBuses.free(this);
        else server.controlBuses.free(this);
    }
}

/** Anything a command can address by bus index: a handle or the bare number. */
export type BusLike = Bus | number;

/** The index behind a bus handle or a bare number. */
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

    /**
     * A run of `channels` contiguous buses. Throws when no such run is free
     * — exhaustion is an explicit failure, never an aliased index.
     */
    alloc(channels = 1, server?: Server): Bus {
        const index = this.registry?.alloc(channels);
        if (index === undefined) {
            throw new AllocationError(`out of ${this.rate} buses`);
        }
        return new Bus(index, channels, this.rate, server);
    }

    /**
     * Returns the bus's run to the pool. A double free (or a bus this
     * allocator never handed out) throws — losing track of a bus is a client
     * bug, never absorbed silently.
     */
    free(bus: Bus): void {
        if (!this.registry?.release(bus.index, bus.channels)) {
            throw new AllocationError(
                `double free of ${this.rate} bus ${bus.index} ` +
                    `(channels=${bus.channels}): not currently allocated here`,
            );
        }
    }

    /** How many buses are currently allocated. */
    get inUse(): number {
        return this.registry?.inUse ?? 0;
    }
}

/**
 * Allocates audio buses above the hardware outputs (`reserved`) and below
 * the GraphDef private range. `size` is the server's audio-bus count.
 */
export class AudioBusAllocator extends Allocator {
    constructor(size: number, reserved = 2) {
        super("audio", size, reserved, graphBusReserved()[0]);
    }
}

/** `size` is the server's control-bus count. */
export class ControlBusAllocator extends Allocator {
    constructor(size: number) {
        super("control", size, 0, graphBusReserved()[1]);
    }
}
