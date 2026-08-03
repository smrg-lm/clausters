// The receiving door: a connection's packets, decoded once and demuxed to
// whoever registered — the transport under `responders.OscFunc`, and the
// browser's answer to the Python client's `OscReceiver`.
//
// **What a receiver is here.** There, it owns a UDP socket of its own: it
// binds a port, runs a thread, and any application on the machine can target
// it. A page has no listening socket and cannot be targeted by anybody; what
// it has is the carrier it already opened — a WebSocket to a server, or the
// in-page engine — and everything arriving arrives on that. So the receiver
// wraps a `Connection` instead of a socket, and the rest is the reference's,
// verbatim: decode each packet through the one door (`decodePacketTimed`,
// bundles unwrapped, the timetag carried), call every registered handler with
// `(addr, args, time, src)`, and let each handler filter itself.
//
// That substitution has one visible consequence, and it is worth stating
// rather than hiding: `src` is not a `(host, port)` pair, because a packet on
// a page does not carry one. It is the carrier the packet came in on — a
// socket's URL, or `"page"` for the in-page engine — which is the same
// question ("who sent this?") answered with what a browser actually knows.
//
// Dispatch threading is the reference's rule with the thread taken out: with
// no clock, handlers run inline as the packet arrives; with a clock, each is
// scheduled through `clock.sched(0)` so it runs with the clock's logical time
// available. The golden rule survives the change of language — a handler that
// blocks blocks the page.

import { decodePacketTimed, encodeMessage, oscArg } from "./osc.ts";
import type { OscMessage, MsgArg } from "./osc.ts";
import type { Connection } from "./connection.ts";
import type { TempoClock } from "./clock.ts";

/**
 * A handler on the receiving door: every decoded message reaches it, and it
 * filters itself. `time` is the containing bundle's Unix time (`null` for an
 * immediate or bare message) and `src` names the carrier it arrived on.
 */
export type OscHandler = (
    addr: string,
    args: OscMessage["args"],
    time: number | null,
    src: string,
) => void;

/**
 * The transport + demux under a responder: one connection's packets, decoded
 * and handed to every registered handler.
 *
 * Registered handlers are called with `(addr, args, time, src)`. Pass a
 * `clock` to have them dispatched on it (`clock.sched(0)`) rather than inline
 * on the packet's arrival.
 *
 * A receiver starts listening when it is created; `stop` (or `close`) releases
 * it, and `start` puts it back.
 */
export class OscReceiver {
    /** The carrier this receiver listens on. */
    readonly connection: Connection;
    /** Dispatch handlers through this clock instead of inline, when set. */
    clock: TempoClock | null;
    /** What this receiver reports as a message's sender. */
    readonly src: string;

    private handlers: OscHandler[] = [];
    private running = false;
    private readonly listener: (packet: Uint8Array) => void;

    constructor(
        connection: Connection,
        { clock = null, src }: { clock?: TempoClock | null; src?: string } = {},
    ) {
        this.connection = connection;
        this.clock = clock;
        this.src = src ?? connection.url ?? "page";
        this.listener = (packet) => this.receive(packet);
        this.start();
    }

    /** Starts listening (idempotent). Returns this receiver. */
    start(): this {
        if (!this.running) {
            this.connection.addReply(this.listener);
            this.running = true;
        }
        return this;
    }

    /**
     * Stops listening without discarding the object, and without touching the
     * connection — which belongs to whoever opened it, and usually to a
     * `Server` still using it.
     */
    stop(): this {
        if (this.running) {
            this.connection.removeReply(this.listener);
            this.running = false;
        }
        return this;
    }

    /** `stop`, under the name a caller holding a resource reaches for. */
    close(): this {
        return this.stop();
    }

    /** Whether this receiver is listening. */
    get listening(): boolean {
        return this.running;
    }

    /**
     * Registers `handler`, called for every decoded message. Returns the
     * handler, so it can later be `remove`d.
     */
    add(handler: OscHandler): OscHandler {
        this.handlers.push(handler);
        return handler;
    }

    /** Unregisters a handler `add` returned. */
    remove(handler: OscHandler): void {
        const at = this.handlers.indexOf(handler);
        if (at >= 0) this.handlers.splice(at, 1);
    }

    /**
     * Sends a message out this receiver's own carrier — what lets a responder
     * answer on the connection it is listening to (the reference's
     * `OscReceiver.send`, whose point is replying from the port that heard
     * you).
     */
    send(addr: string, ...args: MsgArg[]): void {
        this.connection.send(
            encodeMessage(
                addr,
                args.map((a) => oscArg(a)),
            ),
        );
    }

    private receive(packet: Uint8Array): void {
        let messages;
        try {
            messages = decodePacketTimed(packet);
        } catch {
            return; // untrusted bytes: drop anything that will not decode
        }
        for (const { addr, args, time } of messages) {
            // A copy: a handler may free a responder, which edits this list.
            for (const handler of [...this.handlers]) {
                if (this.clock) {
                    this.clock.sched(0.0, () => handler(addr, args, time, this.src));
                } else {
                    handler(addr, args, time, this.src);
                }
            }
        }
    }
}
