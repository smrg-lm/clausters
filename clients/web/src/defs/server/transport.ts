// The server's shared transport grid: the beat every client phases on
// (mirrors `clausters/defs/server/transport.py`).
//
// The grid is one conductor's to define (`setTransport`) and everyone else's to
// join. Bound to a group it stops being advisory: the engine freezes and thaws
// that subtree, so a stop is a real pause of the sound the server is generating
// rather than a convention the clients observe.
//
// A mixin, composed into `Server` beside `ServerQueries` and `ServerStreams`,
// so no attribute path moves.

import { CommandError } from "../../errors.ts";
import { encodeImmediateBundle, toBundle } from "../../base/osc.ts";
import type { MsgArg, TimedMessage } from "../../base/osc.ts";
import { nodeId } from "../node.ts";
import type { NodeLike } from "../node.ts";
import type { Server } from "./index.ts";

/** The shared grid, as `transport()` reports it. */
export interface TransportGrid {
    /** The sample the grid puts beat 0 on, on the server's sample clock. */
    originSample: number;
    /** Beats per second. Beat `b` is `originSample + b * rate / tempo`. */
    tempo: number;
}

/** The grid plus the rolling state, as `transportState()` reports it. */
export interface TransportState extends TransportGrid {
    /** Whether the transport is rolling. */
    playing: boolean;
    /** The song-position beat: where play starts, or where a stop left it. */
    position: number;
    /** The governed group, or `null` when nothing is bound. */
    group: number | null;
    /**
     * The transport clock: samples elapsed under the transport, held while it
     * is stopped. The device clock (`/clock_query`, the taps, the streams)
     * never stops, and this one holds — but it is monotonic all the same, so a
     * locate does not move it. For where the piece *is*, read
     * `positionSample`.
     */
    transportSample: number;
    /**
     * Where the transport is **in the piece**, in samples of the material —
     * what a playhead draws. Not a clock: it jumps to wherever a locate puts
     * it and wraps inside `loop`. Read from the engine as of its last
     * completed block, so a query issued in the same breath as a locate may
     * still answer the previous place.
     */
    positionSample: number;
    /**
     * The half-open span of the piece the transport loops inside, or `null`
     * when looping is off.
     */
    loop: [number, number] | null;
}

/** The shared transport grid. Composed into `Server`; never used alone. */
export class ServerTransport {
    /**
     * The server's shared transport grid (`/transport_query`), or `null` if
     * none is set. The grid lets several clients phase-align on the master
     * sample clock.
     */
    async transport(this: Server, timeout?: number): Promise<TransportGrid | null> {
        const msg = await this.request("/transport_query", [], {
            expect: ["/transport_query.reply"],
            timeout,
        });
        if (!Number(msg.args[2])) return null;
        return {
            originSample: Number(msg.args[0]),
            tempo: Number(msg.args[1]),
        };
    }

    /**
     * Defines the server's shared transport grid (`/transport_set`): beat 0 at
     * `originSample` on the sample clock, advancing at `tempo` beats per
     * second. One client (the conductor) sets it; the others read it. Last
     * writer wins, and defining the grid resets the rolling state to stopped at
     * position 0 — so a bound group freezes with it.
     */
    async setTransport(
        this: Server,
        originSample: number,
        tempo: number,
        timeout?: number,
    ): Promise<Server> {
        await this.command(
            "/transport_set",
            [["h", Math.trunc(originSample)], ["d", tempo]],
            timeout,
        );
        return this;
    }

    /**
     * The full shared transport state, or `null` if no grid is defined.
     *
     * `group` is the governed group (`transportGroup`) or `null` when nothing
     * is bound, and `transportSample` is the transport clock. Both are always
     * reported — every server sends them — so they are read straight.
     */
    async transportState(this: Server, timeout?: number): Promise<TransportState | null> {
        const msg = await this.request("/transport_query", [], {
            expect: ["/transport_query.reply"],
            timeout,
        });
        if (!Number(msg.args[2])) return null;
        const group = Number(msg.args[5]);
        const loopStart = Number(msg.args[8]);
        const loopEnd = Number(msg.args[9]);
        return {
            originSample: Number(msg.args[0]),
            tempo: Number(msg.args[1]),
            playing: Boolean(Number(msg.args[3])),
            position: Number(msg.args[4]),
            group: group < 0 ? null : group,
            transportSample: Number(msg.args[6]),
            positionSample: Number(msg.args[7]),
            loop: loopEnd > loopStart ? [loopStart, loopEnd] : null,
        };
    }

    /**
     * Binds the group the transport governs (`/transport_group`), or unbinds
     * with `null`.
     *
     * This is what gives the transport its teeth. With no group bound it is a
     * shared beat grid plus a rolling state that clients obey by choice. With
     * one bound, the **engine** enforces it: `transportStop` freezes that
     * subtree and the server's transport clock, `transportPlay` thaws them.
     * Every node in the subtree keeps its internal state across the freeze, so
     * a resume continues the sound rather than restarting it — which is the
     * only thing a pause can mean for material the server generates itself.
     *
     * Freeing the group unbinds the transport, and unbinding thaws whatever it
     * governed, so no frozen subtree is left with nobody to resume it.
     */
    async transportGroup(
        this: Server,
        group: NodeLike | null,
        timeout?: number,
    ): Promise<Server> {
        const id = group === null ? -1 : nodeId(group);
        await this.command("/transport_group", [["i", id]], timeout);
        return this;
    }

    /**
     * Schedules `messages` at an absolute sample on the **transport** axis
     * (`/sched_atTransport`), the counterpart of `sendBundle`'s device axis.
     *
     * Declaring the axis is not about disambiguation — classification is
     * deterministic, and a client that bound the group knows which of its nodes
     * are governed. It is about **verification**: the server compares the
     * declaration against its own classification and fails when they disagree,
     * instead of playing the bundle in the wrong place. Needs a group bound.
     */
    async schedAtTransport(
        this: Server,
        target: number,
        messages: readonly TimedMessage[],
        timeout?: number,
    ): Promise<Server> {
        const inner = encodeImmediateBundle(toBundle(messages));
        await this.command(
            "/sched_atTransport",
            [["h", Math.trunc(target)], ["b", inner]],
            timeout,
        );
        return this;
    }

    /**
     * Starts the shared transport rolling (`/transport_play`). With `position`
     * playback starts from that song-position beat; without it, from where it
     * last stopped or located. The server broadcasts the change to every
     * `/server_notify` client, so all following playheads roll together. Needs
     * a grid defined (`setTransport`).
     */
    async transportPlay(
        this: Server,
        position?: number,
        timeout?: number,
    ): Promise<Server> {
        const args: MsgArg[] = position === undefined ? [] : [["d", position]];
        await this.command("/transport_play", args, timeout);
        return this;
    }

    /**
     * Stops the shared transport (`/transport_stop`); every following playhead
     * halts and a governed subtree freezes. Broadcast to `/server_notify`
     * clients.
     */
    async transportStop(this: Server, timeout?: number): Promise<Server> {
        await this.command("/transport_stop", [], timeout);
        return this;
    }

    /**
     * Sets the shared transport's song position (`/transport_locate`) — where
     * play starts, or where it seeks to while playing. Every following playhead
     * locates to it. Broadcast to `/server_notify` clients.
     *
     * It moves the position, never the state of a node: a governed subtree
     * stays exactly where it is, since a generator's position *is* its state.
     */
    async transportLocate(
        this: Server,
        position: number,
        timeout?: number,
    ): Promise<Server> {
        await this.command("/transport_locate", [["d", position]], timeout);
        return this;
    }

    /**
     * Seeks on the piece's own **sample** axis (`/transport_locateSample`).
     *
     * The sibling of `transportLocate`, which takes a beat: a sequencer locates
     * by beat and an audio editor by frame, and converting either into the
     * other on the client is how a rounding error gets into a seek. The beat
     * position follows, so both readings of `transportState` agree. Needs a
     * grid defined; a negative sample clamps to 0.
     */
    async transportLocateSample(
        this: Server,
        sample: number,
        timeout?: number,
    ): Promise<Server> {
        await this.command(
            "/transport_locateSample",
            [["h", Math.trunc(sample)]],
            timeout,
        );
        return this;
    }

    /**
     * Sets — or clears, with `null` — the span of the piece the transport loops
     * inside (`/transport_loop`), in samples.
     *
     * The span is **half-open**: `[0, n]` over an `n`-sample take plays every
     * frame exactly once and joins its own start with no repeated frame.
     * Turning a loop on does not move the piece; it keeps playing and wraps
     * when it first reaches the end, in the engine, so nothing has to be sent
     * once a pass completes. An empty or inverted span fails. What a loop
     * toggle remembers is the caller's to keep: clearing forgets the span.
     */
    async transportLoop(
        this: Server,
        span: [number, number] | null = null,
        timeout?: number,
    ): Promise<Server> {
        const args: MsgArg[] =
            span === null
                ? []
                : [["h", Math.trunc(span[0])], ["h", Math.trunc(span[1])]];
        await this.command("/transport_loop", args, timeout);
        return this;
    }
}
