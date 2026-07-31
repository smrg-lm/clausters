// Where OSC goes, and how a `Moment` becomes wire time.
//
// A destination owns the carrier the bytes leave through, the target they go
// to, and the policy that turns a logical moment into a timetag. `Server` is
// the destination we control — it adds that server's latency and schedules by
// absolute sample when the clock is anchored to the server's own.
// `OscDestination` is every other one: standard OSC and nothing else.
//
// The page cannot open a UDP socket, so a destination here rides a
// `Connection` exactly as the server does: for an external application that
// means its WebSocket bridge. The Python client, which can, defaults to UDP —
// the difference is the carrier, never the timing.

import type { Connection } from "./connection.ts";
import { Moment } from "./moment.ts";
import { encodeBundle, encodeMessage, oscArg, toBundle } from "./osc.ts";
import type { MsgArg, TimedMessage } from "./osc.ts";

/**
 * Somewhere OSC goes.
 *
 * Note what is *not* here: `playEvent`. Rendering an `Event` is a double
 * dispatch onto destinations that understand the server's node commands; an
 * external application does not know what `/s_new` is.
 */
export interface Destination {
    /** Sends one message, untimetagged. */
    sendMsg(addr: string, ...args: MsgArg[]): void;
    /** Sends a timetagged bundle. */
    sendBundle(
        messages: readonly TimedMessage[],
        options?: { delayBeats?: number; at?: Moment },
    ): void;
}

/**
 * An OSC application we do not control.
 *
 * Standard OSC only: a message, or a bundle carrying an NTP timetag. No
 * latency — that is a property of *our* audio pipeline, and what another
 * application needs is its own business, asked for as an explicit delay. No
 * `/sched` (our command).
 *
 * The `Connection` is borrowed, never closed here: whoever opened the carrier
 * owns it.
 */
export class OscDestination implements Destination {
    readonly connection: Connection;

    constructor(connection: Connection) {
        this.connection = connection;
    }

    /** Sends one message. **A message has no time** — it means "now". */
    sendMsg(addr: string, ...args: MsgArg[]): void {
        this.connection.send(encodeMessage(addr, args.map(oscArg)));
    }

    /**
     * Emits a timetagged bundle at `at` (default: the ambient `Moment`) plus
     * `delayBeats`.
     *
     * Inside a routine that is the routine's exact logical beat, so a sequence
     * sent to another application stays as tight as one sent to the server.
     * Outside any routine it is wall-clock now plus the delay read as seconds.
     */
    sendBundle(
        messages: readonly TimedMessage[],
        { delayBeats = 0, at }: { delayBeats?: number; at?: Moment } = {},
    ): void {
        const when = (at ?? Moment.current()).at(delayBeats);
        this.connection.send(encodeBundle(when.instant(), toBundle(messages)));
    }
}
