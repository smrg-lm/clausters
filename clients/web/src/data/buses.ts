// Control buses, streamed to the script.
//
// A meter, a control-rate scope, a number read-out: they all watch a handful
// of control buses and repaint. Natively the GUI host reads those buses out of
// the server's shared memory with no messages at all; a page cannot map that
// segment, so the server offers the message-based counterpart — `/c_stream`,
// one subscription per client, a periodic `/c_set` snapshot of the listed
// buses. This is that subscription with its decoding attached.
//
// The whole object is a *latest value* store, not a history: a snapshot
// replaces the previous one. A view that wants a rolling trace keeps its own
// history from `onSnapshot` — how long a trace is, is the view's decision.

import type { Server } from "../defs/server.ts";
import type { BusLike } from "../defs/bus.ts";
import { busIndex } from "../defs/bus.ts";
import type { OscMessage } from "../base/osc.ts";

/** The subscription period a live view runs at: ~30 fps, the host's own. */
export const STREAM_PERIOD_MS = 33;

/**
 * A live view of a set of control buses.
 *
 * ```ts
 * const buses = await BusStream.open(server, [level, cutoff]);
 * buses.onSnapshot((values) => draw(values));   // ~30 times a second
 * // ... later
 * await buses.stop();
 * ```
 *
 * At most 128 buses per subscription, and **one subscription per client**:
 * opening a second `BusStream` on the same `Server` replaces the first, which
 * is the server's rule, not this class's. Watch every bus a page needs in one
 * stream.
 *
 * Over the **in-page carrier that client includes the GUI host**: script and
 * host share one shared-memory ring, which the server sees as a single client,
 * so a host `meter`/`scope` and a `BusStream` displace each other's
 * subscription — and the host, which only re-subscribes when its own widget
 * set changes, stays frozen afterwards. One live reader per page until ring
 * clients get identities (the gap is recorded in the server's roadmap). A
 * socket carrier has no such conflict.
 */
export class BusStream {
    readonly server: Server;
    /** The bus indices watched, in the order `values` holds them. */
    readonly buses: readonly number[];
    /** The newest snapshot, one entry per bus, in `buses` order. */
    readonly values: Float32Array;
    /** Snapshots seen so far — a view can tell a repaint from a stall. */
    snapshots = 0;

    private slot = new Map<number, number>();
    private listeners = new Set<(values: Float32Array, stream: BusStream) => void>();
    private unsubscribe: (() => void) | null = null;

    private constructor(server: Server, buses: readonly number[]) {
        this.server = server;
        this.buses = buses;
        this.values = new Float32Array(buses.length);
        buses.forEach((bus, i) => this.slot.set(bus, i));
    }

    /**
     * Subscribes to `buses` and resolves once the server has acked, with the
     * first snapshot already applied where it arrived in time.
     */
    static async open(
        server: Server,
        buses: readonly BusLike[],
        { periodMs = STREAM_PERIOD_MS, timeout = 5.0 } = {},
    ): Promise<BusStream> {
        const indices = buses.map(busIndex);
        const stream = new BusStream(server, indices);
        stream.unsubscribe = server.onReply((msg) => stream.take(msg));
        try {
            await server.streamBuses(periodMs, indices, timeout);
        } catch (error) {
            stream.detach();
            throw error;
        }
        return stream;
    }

    /** The newest value of one bus (`NaN` when it is not in this stream). */
    value(bus: BusLike): number {
        const slot = this.slot.get(busIndex(bus));
        return slot === undefined ? NaN : this.values[slot];
    }

    /**
     * Calls `handler` with each snapshot as it lands; returns the
     * unsubscribe. The handler runs from the reply dispatch, so keep it to
     * storing and drawing — never a round trip.
     */
    onSnapshot(
        handler: (values: Float32Array, stream: BusStream) => void,
    ): () => void {
        this.listeners.add(handler);
        return () => this.listeners.delete(handler);
    }

    /**
     * Cancels the subscription on the server and stops decoding. The buses
     * themselves are untouched — a stream only ever reads.
     */
    async stop(timeout = 5.0): Promise<void> {
        this.detach();
        await this.server.streamBuses(0, [], timeout);
    }

    private detach(): void {
        this.unsubscribe?.();
        this.unsubscribe = null;
        this.listeners.clear();
    }

    /** One `/c_set bus value …` snapshot into `values`. */
    private take(msg: OscMessage): void {
        if (msg.addr !== "/c_set") return;
        let touched = false;
        for (let i = 0; i + 1 < msg.args.length; i += 2) {
            const slot = this.slot.get(Number(msg.args[i]));
            if (slot === undefined) continue;
            this.values[slot] = Number(msg.args[i + 1]);
            touched = true;
        }
        if (!touched) return;
        this.snapshots++;
        for (const handler of [...this.listeners]) handler(this.values, this);
    }
}
