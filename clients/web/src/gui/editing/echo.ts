/**
 * The acknowledgement protocol: what a view is told about the edit it made.
 *
 * A host draws what the hand did and then waits to be told what actually
 * happened — the edit as applied, snapped, or refused — and every editor owes it
 * the same three things: the **version** the data is at, the
 * **corrections** its own gesture did not survive intact, and the **reason**
 * when one is owed. That triple is the whole of this module, and it knows
 * nothing about what was edited: a stamp, a floor and a list of props.
 *
 * **The rules are the shared crate's** (`conversationRead`,
 * `conversationAnswer`): what makes an edit stale, what moves the floor, and
 * whether an answer is an ack, a push or nothing at all. What is here is the
 * half a language owns — holding the two integers between messages and putting
 * the answer on this page's socket.
 *
 * It is separate because it is the one part of an editor with no data behind it.
 * {@link Echo} is exercised by a test that never builds a structure, which is
 * what a protocol should cost to check.
 *
 * @module
 */

import { conversationAnswer, conversationRead } from "../../core/clausters_core_web.js";
import type { GuiHost, PropValue } from "../host.ts";
import { log } from "./trace.ts";

/** One correction: the widget, and what it should be drawing. */
export type Correction = [number, Record<string, PropValue>];

/**
 * One message from the host, as much of it as the decision needs — the
 * *envelope*, never the payload.
 *
 * What a report means is the domain's and crosses once, there; this is what
 * kind of turn the message is.
 */
export interface Envelope {
    addr: string;
    argc: number;
    widget: number;
    seq: number;
    against: number;
    tag: string;
    version: number;
    isWindow: boolean;
    owns: boolean;
}

/** What one message turns out to be. */
export interface Turn {
    turn: "nothing" | "closed" | "step" | "stale" | "route";
    seq?: number;
    redo?: boolean;
    widget?: number;
    reason?: string;
}

/** One view's end of the acknowledgement protocol. */
export class Echo {
    /**
     * The host to answer, or `null` for an editor with no window — which
     * answers by doing nothing, since there is nobody to tell.
     */
    host: GuiHost | null = null;
    /**
     * The conversation's whole state, as the crate holds it: the **floor** (the
     * oldest version an incoming edit may name) and the version the last
     * answered event left behind. Two integers, kept here because something has
     * to keep them between messages, and handed back to the crate on every one.
     */
    state: { floor: number; applied: number };
    /**
     * What the host should be drawing instead of what it drew, collected while
     * one event is routed and sent with its acknowledgement.
     */
    corrections: Correction[] = [];
    /**
     * Why the last routed event did not do what it asked, if it did not. It
     * rides with the acknowledgement, because a refusal with no reason teaches
     * "sometimes it does not work" — the one answer worse than no.
     */
    reason: string | undefined = undefined;

    readonly #version: () => number;

    /**
     * `version` answers the data's current version. A callable rather
     * than a number because the version belongs to the **editing context** and
     * moves under this object: two windows over one structure read one
     * counter, and a copy kept here would be a second answer to a question with
     * one.
     */
    constructor(version: () => number, host: GuiHost | null = null) {
        this.#version = version;
        this.host = host;
        this.state = { floor: version(), applied: version() };
    }

    /**
     * The **oldest version an incoming edit may name**, raised whenever the
     * data moves by a route that is not a host event and by nothing
     * else — which is what makes staleness a monotone test rather than a race.
     */
    get floor(): number {
        return this.state.floor;
    }

    /**
     * **What one message from the host is**, and the two integers as they now
     * stand.
     *
     * A close, a history step, an edit made against a picture that is gone, or
     * an edit to route. The rules are the crate's, so a page and a script
     * cannot disagree about which gestures are refused.
     */
    read(message: Envelope): Turn {
        const answered = JSON.parse(
            conversationRead(JSON.stringify(this.state), JSON.stringify(message)),
        ) as { turn?: Turn; state?: { floor: number; applied: number } };
        if (answered.state !== undefined) this.state = answered.state;
        return answered.turn ?? { turn: "nothing" };
    }

    /** The version an acknowledgement carries — the context's, read now. */
    get version(): number {
        return this.#version();
    }

    /**
     * Tell the host which version it is drawing, before any edit.
     *
     * A stamp of zero retires nothing — the host's own numbering starts at one —
     * so this is purely the version, and it is what keeps the *first* gesture
     * checked like every later one. Without it the host would name zero until
     * the first acknowledgement came back, and the opening edit would be the one
     * edit nobody could tell was stale.
     */
    announce(): void {
        this.host?.ack(0, this.version);
    }

    /**
     * What the host should be drawing instead of what it drew.
     *
     * Called while routing, when the editor did not do what the gesture asked —
     * snapped it to the grid, or refused it outright. The value travels with the
     * acknowledgement in one bundle, which is what lets the host adopt it
     * without a redefine.
     */
    correct(widgetId: number, props: Record<string, PropValue>): void {
        this.corrections.push([Math.trunc(widgetId), props]);
    }

    /** Drop what has not been sent: one event's corrections are that event's. */
    clear(): void {
        this.corrections = [];
    }

    /**
     * Answer the host for everything up to `seq`.
     *
     * An editor snaps a placement to the musical grid and refuses an edit to a
     * generator, and without this the host could learn neither. The stamp closes
     * both, because it lets the host retire what it drew and adopt what actually
     * happened. Every acknowledgement carries the data's version, which
     * is what the host names back on its next gesture — that round trip is the
     * whole of the staleness check, and it costs one integer.
     */
    acknowledge(seq: number, reason?: string): void {
        if (this.host === null) return;
        // **What to send is the crate's decision**, including that an unasked
        // push with nothing to say is one message the wire does not carry.
        this.send(JSON.parse(conversationAnswer(JSON.stringify({
            seq,
            docVersion: this.version,
            reason: reason ?? null,
            corrections: this.corrections.map(([widget, props]) => ({ widget, props })),
        }))) as Answer);
    }

    /**
     * Put an answer the crate decided on this page's socket: `ack`, `push`, or
     * nothing for `silent`.
     *
     * The half of {@link Echo.acknowledge} a language owns, and the whole of
     * what an editor whose turns are the crate's needs from this object.
     */
    send(answered: Answer | null | undefined): void {
        if (this.host === null || answered === null || answered === undefined) return;
        if (answered.answer !== "ack" && answered.answer !== "push") return;
        const seq = answered.seq ?? 0;
        const corrections: Correction[] = (answered.corrections ?? []).map(
            (c) => [c.widget, c.props],
        );
        const version = answered.docVersion ?? this.version;
        log.debug(
            "ack    seq=%s version=%s%s%s",
            seq,
            version,
            corrections.length === 0
                ? ""
                : " correcting " + corrections
                    .map(([wid, props]) => `${wid}(${Object.keys(props).sort().join(" ")})`)
                    .join(", "),
            answered.reason === undefined ? "" : ` reason=${JSON.stringify(answered.reason)}`,
        );
        if (answered.answer === "push") {
            this.host.push(seq, corrections, version, [], answered.reason);
        } else {
            this.host.ack(seq, version, [], answered.reason);
        }
    }
}

/** What the crate decided to answer the host with. */
export interface Answer {
    answer: string;
    seq?: number;
    docVersion?: number;
    reason?: string;
    corrections?: { widget: number; props: Record<string, PropValue> }[];
}
