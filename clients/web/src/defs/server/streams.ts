// The subscriptions the server pushes (mirrors
// `clausters/defs/server/streams.py`).
//
// Both are **one per client**, replaced by each call and cancelled by a
// non-positive period: the client says once what it wants to watch and the
// server sends until told otherwise, which is what a meter, a scope or a
// spectrum in the page feeds on. Reading the snapshots is `data/`'s job
// (`BusStream`, `TapStream`); these two only place the subscription.
//
// A mixin, composed into `Server` beside `ServerQueries`, so no attribute path
// moves.

import type { MsgArg } from "../../base/osc.ts";
import { busIndex } from "../bus.ts";
import type { BusLike } from "../bus.ts";
import type { Server } from "./index.ts";

/** The push subscriptions. Composed into `Server`; never used alone. */
export class ServerStreams {
    /**
     * Subscribes this client to a periodic `/bus_stream.reply` snapshot of `buses`
     * (`/bus_stream`): the server sends one immediately and then one every
     * `periodMs` (10 ms floor, at most 128 buses) with no further requests —
     * the message-based counterpart of reading the shared-memory segment, and
     * what a meter or a control-rate scope in the page feeds on.
     *
     * One subscription per client, **replaced** by each call; `periodMs <= 0`
     * (or no buses) cancels. Resolves on the `/done` ack. Read the snapshots
     * with `onReply`, or let `busStream` do all of it.
     */
    async streamBuses(
        this: Server,
        periodMs: number,
        buses: readonly BusLike[],
        timeout?: number,
    ): Promise<void> {
        const args: MsgArg[] = [["i", Math.trunc(periodMs)]];
        for (const bus of buses) args.push(["i", busIndex(bus)]);
        await this.command("/bus_stream", args, timeout);
    }

    /**
     * Subscribes this client to a periodic `/bus_tapStream.reply` snapshot of `buses`
     * (`/bus_tapStream`): every `periodMs` (10 ms floor) the server sends, per
     * bus, its newest `frames` samples — the path an oscilloscope, a
     * phasescope or a spectrum in the page reads.
     *
     * The subscription **is** the watch: it starts recording each bus it
     * lists and stops when it is replaced, cancelled or the connection dies,
     * so a streaming client never calls `watch` itself. `frames` is clamped to
     * the transport's bound and to half the ring; at most 8 buses; one
     * subscription per client, replaced by each call, `periodMs <= 0` (or no
     * buses) cancels. Resolves on the `/done` ack; `tapStream` wraps the whole
     * thing.
     */
    async streamTaps(
        this: Server,
        periodMs: number,
        frames: number,
        buses: readonly BusLike[],
        timeout?: number,
    ): Promise<void> {
        const args: MsgArg[] = [
            ["i", Math.trunc(periodMs)],
            ["i", Math.trunc(frames)],
        ];
        for (const bus of buses) args.push(["i", busIndex(bus)]);
        await this.command("/bus_tapStream", args, timeout);
    }

    /**
     * Subscribes this client to the **overview of material as it is written**
     * (`/buffer_stream`): every `periodMs` (10 ms floor) the server sends, per
     * watched buffer whose write frontier moved, one
     * `/buffer_stream.reply bufnum startFrame bucket blob` — and nothing for
     * one that did not, so a still buffer costs no traffic.
     *
     * The blob is **bucket-major, channel-minor**: for each bucket of `bucket`
     * frames in order, for each channel, `min`, `max` and mean square as raw
     * little-endian `f32`. That is the peak pyramid's own summary, so a page
     * folds it into a picture without converting anything, and it is two
     * orders of magnitude smaller than the audio it describes — which is the
     * whole reason a page can watch a take record at all.
     *
     * What moves the frontier is a writing UGen (`RecordBuf`, `BufWr`). A peer
     * that can map the server's segment reads that number directly; a page
     * cannot map anything, and this is the same reading for it. At most 32
     * buffers and 4096 buckets per report; one subscription per client,
     * replaced by each call, `periodMs <= 0` (or no buffers) cancels. A
     * subscription watches what happens **next**, not what is already there
     * (that is `getRange`). Resolves on the `/done` ack.
     */
    async streamBuffers(
        this: Server,
        periodMs: number,
        buffers: readonly (number | { bufnum: number })[],
        bucket = 256,
        timeout?: number,
    ): Promise<void> {
        const args: MsgArg[] = [
            ["i", Math.trunc(periodMs)],
            ["i", Math.trunc(bucket)],
        ];
        for (const b of buffers) {
            args.push(["i", typeof b === "number" ? b : b.bufnum]);
        }
        await this.command("/buffer_stream", args, timeout);
    }
}
