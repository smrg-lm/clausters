// Event patterns: the patterns whose stream yields `Event`s (mirrors
// `clausters/seq/patterns/eventpatterns.py`).
//
// An event pattern is what plays (`EventPattern.play`) and what renders into an
// `EventSequence`. Each value pattern given to one is read once per event, and
// receives the event being built as its incoming value, so a key can be
// computed from the keys before it (`pkey`).
//
// **Articulation and inflection.** An articulation is how distinct notes
// relate -- legato, detached, a tie, a slur; an inflection is a variation
// inside one note -- its bend, its pressure, any control moving -- which is
// the note's own expression (`Event.expression`). `pvoice` is the pattern of
// articulation: its events are notes of one voice, and `legato` says how each
// goes on to the next.

import { StopStream } from "../../base/stream.ts";
import { Event, rest } from "../event.ts";
import type { Node } from "../../defs/node.ts";
import { INF, asStream, pattern } from "../pattern.ts";
import type { EventPattern, Pattern } from "../pattern.ts";

/**
 * The keys a `pbind` binds: each one a pattern, or a constant held for every
 * event.
 */
export type Bindings = Record<string, unknown>;

/** The event a pattern builds on: a copy of the incoming one, else empty. */
const proto = (inval: unknown): Record<string, unknown> =>
    inval instanceof Event ? { ...inval.props }
        : inval !== null && typeof inval === "object" ? { ...(inval as object) } : {};

const ended = (error: unknown): void => {
    if (!(error instanceof StopStream)) throw error;
};

/**
 * **Binds keys to value patterns**: an `Event` per step, each key's value the
 * next of its pattern, ending when any key's pattern ends. A bare value is held
 * for every event.
 */
export const pbind = pattern(
    function* pbind(inval: unknown, bindings: Bindings) {
        const streams = Object.entries(bindings).map(([key, value]) => [key, asStream(value)] as const);
        for (;;) {
            const props = proto(inval);
            try {
                for (const [key, stream] of streams) props[key] = stream.next(props);
            } catch (error) {
                ended(error);
                return inval;
            }
            inval = yield new Event(props);
        }
    },
    { events: true },
) as (bindings: Bindings) => EventPattern<Event>;

/**
 * The value `key` has in the event being built -- a key of a `pbind` read by
 * a later one: `pbind({ degree: ..., octave: pkey("degree").idiv(7).add(4) })`.
 * It ends when the event lacks the key.
 */
export const pkey = pattern(function* pkey(inval: unknown, key: string | Pattern<string>, length: number = INF) {
    const keys = asStream(key);
    try {
        for (let i = 0; i < length; i++) {
            const name = keys.next(inval) as string;
            const props = inval instanceof Event ? inval.props
                : inval !== null && typeof inval === "object" ? (inval as Record<string, unknown>) : null;
            if (props === null || !(name in props)) break;
            inval = yield props[name];
        }
    } catch (error) {
        ended(error);
    }
    return inval;
}) as <T = number>(key: string | Pattern<string>, length?: number) => Pattern<T>;

/**
 * **Event patterns chained**, right to left: each event of the last is the
 * incoming event of the one before it, so a later pattern's keys are the
 * defaults an earlier one overrides -- sclang's `a <> b`. It ends when any of
 * them does.
 */
export const pchain = pattern(
    function* pchain(inval: unknown, ...patterns: unknown[]) {
        const streams = [...patterns].reverse().map(asStream);
        try {
            for (;;) {
                let event: unknown = proto(inval);
                for (const stream of streams) event = stream.next(event);
                inval = yield event instanceof Event ? event : new Event(event as Record<string, unknown>);
            }
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: true },
) as (...patterns: Pattern<Event>[]) => EventPattern<Event>;

/** `source` with `event` as the incoming event of every step: the keys every event starts from. */
export const pevent = pattern(
    function* pevent(inval: unknown, source: unknown, event: Record<string, unknown> | null = null) {
        const stream = asStream(source);
        try {
            for (;;) inval = yield stream.next({ ...(event ?? {}) });
        } catch (error) {
            ended(error);
        }
        return inval;
    },
    { events: true },
) as (source: Pattern<Event>, event?: Record<string, unknown> | null) => EventPattern<Event>;

/**
 * Whether `note` goes on into `following` in a voice: never across a rest,
 * always without `artic`, and with it while the note sounds at least until the
 * next starts (`legato >= 1`, sclang's `sustain >= delta`).
 */
const joins = (note: Event, following: Event | null, artic: boolean): boolean => {
    if (following === null) return false;
    if ((note.get("type") ?? "note") !== "note" || (following.get("type") ?? "note") !== "note") return false;
    return !artic || note.sustain() >= note.delta();
};

/** How a {@link pvoice} articulates and where it is written. */
export interface VoiceOptions {
    /** Re-attack a note whose `legato` is below 1 (sclang's `PmonoArtic`). */
    artic?: boolean;
    /** The notation `voice` written on every note. */
    voice?: number | null;
}

/**
 * **A voice: notes that go on into one another** -- sclang's `Pmono` and, with
 * `artic`, `PmonoArtic`.
 *
 * It binds keys to patterns as `pbind` does, and every event is a note of
 * `instrument`. What it adds is the **articulation between consecutive
 * notes**, which `legato` says: without `artic` every note goes on into the
 * next, and with it a note joins the next while `legato >= 1` and the next one
 * re-attacks when it is less. A rest always breaks the line. `voice` is written
 * on every note (the notation key: which voice of the staff it is), so voices
 * played together stay apart on a page.
 *
 * How a join sounds is the destination's, by one rule: on the server the voice
 * holds **one node** and each joined note sets its controls (with a control's
 * lag, a glide), and the note that ends a line releases it -- by its gate when
 * the events say `has_gate: true`, which an instrument with a gated envelope
 * wants as it does under `pbind`, else by freeing the node, which cuts the
 * sound; on an `instrument` that is a node (a `Voices` node, a plugin's), each
 * joined note-on joins the note before it -- a `Voices` node moves its voice to
 * the new pitch, a plugin gets the new note overlapping the old one; on MIDI,
 * notes that overlap; rendered into an `EventSequence`, a `tie` where a joined
 * note keeps its pitch and a slur over each run that moves, and `joins` on
 * each joined note, naming the one it goes on from. What varies inside one note is an inflection, and is the note's own
 * expression, not this.
 */
export const pvoice = pattern(
    function* pvoice(inval: unknown, instrument: string | Node | { node: number }, bindings: Bindings, options: VoiceOptions = {}) {
        const { artic = false, voice = null } = options;
        const streams = Object.entries(bindings).map(([key, value]) => [key, asStream(value)] as const);
        const build = (inval: unknown): Event => {
            const props = proto(inval);
            props.instrument = instrument;
            if (voice !== null) props.voice = voice;
            for (const [key, stream] of streams) props[key] = stream.next(props);
            return new Event(props);
        };
        let note: Event;
        try {
            note = build(inval);
        } catch (error) {
            ended(error);
            return inval;
        }
        let previous: Event | null = null;
        for (;;) {
            // One note ahead, to know whether this one goes on into it.
            let following: Event | null = null;
            try {
                following = build(inval);
            } catch (error) {
                ended(error);
            }
            const joined = joins(note, following, artic);
            const node = previous?.get("node");
            // On a node, the note it continues is the previous one's id.
            const noteId = previous?.get("noteId");
            if (typeof node === "number") {
                note.set({ continues: typeof noteId === "number" ? noteId : node });
            }
            if (joined) note.set({ held: true });
            inval = yield note;
            if (following === null) return inval;
            previous = joined ? note : null;
            note = following;
        }
    },
    { events: true },
) as (instrument: string | Node | { node: number }, bindings: Bindings, options?: VoiceOptions) => EventPattern<Event>;

/**
 * **Event patterns played together**: their events merged in time order into
 * one stream, each event's `delta` set to the time to the next one of any of
 * them, and a rest where one has ended and the others have not yet reached
 * their next.
 */
export const ppar = pattern(
    function* ppar(inval: unknown, items: readonly unknown[], repeats: number = 1) {
        for (let r = 0; r < repeats; r++) {
            // [time, order, stream], kept sorted: time, then the order queued.
            const queue = items.map((p, i) => [0, i, asStream(p)] as [number, number, ReturnType<typeof asStream>]);
            let order = queue.length;
            const push = (entry: [number, number, ReturnType<typeof asStream>]): void => {
                let at = queue.findIndex(([t, o]) => t > entry[0] || (t === entry[0] && o > entry[1]));
                if (at < 0) at = queue.length;
                queue.splice(at, 0, entry);
            };
            let now = 0;
            while (queue.length > 0) {
                const [, , stream] = queue.shift()!;
                let got: unknown;
                try {
                    got = stream.next(inval);
                } catch (error) {
                    ended(error);
                    if (queue.length > 0 && queue[0]![0] > now) {
                        inval = yield rest(queue[0]![0] - now);
                        now = queue[0]![0];
                    }
                    continue;
                }
                const event = got instanceof Event ? got : new Event(got as Record<string, unknown>);
                push([now + event.delta(), order++, stream]);
                const following = queue[0]![0];
                // In place: the event the source yielded is the one that plays,
                // so what the source keeps of it (a voice's node) is kept.
                event.set({ delta: following - now });
                inval = yield event;
                now = following;
            }
        }
        return inval;
    },
    { events: true },
) as (items: readonly Pattern<Event>[], repeats?: number) => EventPattern<Event>;
