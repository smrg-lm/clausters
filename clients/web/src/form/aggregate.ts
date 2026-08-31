// The arrangement — grouping, and the derived temporal relation (mirrors
// `clausters/form/aggregate.py`).
//
// An `Aggregate` is the one genuinely new structure of the arrangement: the
// recursive placement of elements with an offset, and the temporal *relation*
// derived from how the members sit in time. Everything else (the five
// primitives) already exists and is merely adorned by `Element`.
//
// Two kinds of grouping:
//
// - **concrete** — the members relate in time (a section holding clips, a melody
//   holding note-events), with no processing relation.
// - **logical** — the members relate by processing or generation logic (a
//   bus-wired signal chain on the server, or a generative dependency on the
//   client).
//
// Rendering lives in `./render.ts`; this module is pure structure plus the
// temporal-relation derivation (a pure function over the members' placements).

import { GraphDef } from "../defs/graphdef.ts";
import type { GraphBusRef, MemberControlValue } from "../defs/graphdef.ts";
import { Element, Generator, toBeats } from "./element.ts";
import type { Beats, TimeUnit } from "./element.ts";

/** The kind of an {@link Aggregate}. */
export const CONCRETE = "concrete";
export const LOGICAL = "logical";

/** One of the two kinds above. */
export type AggregateKind = typeof CONCRETE | typeof LOGICAL;

/**
 * The temporal relation between an aggregate's members, derived from their
 * placements. `successive` — duration-only, tiling contiguously; `simultaneous`
 * — all starting and ending together (a container that can be reinterpreted,
 * enabling recursion); `mixed` — any other combination.
 */
export const SUCCESSIVE = "successive";
export const SIMULTANEOUS = "simultaneous";
export const MIXED = "mixed";

/** One of the three relations above. */
export type TemporalRelation =
    | typeof SUCCESSIVE
    | typeof SIMULTANEOUS
    | typeof MIXED;

/**
 * How close two beats have to be to count as the same one — Python's
 * `math.isclose` with its default tolerances, since the Python client derives
 * the same relation from the same placements and the two must agree.
 */
const isClose = (a: number, b: number): boolean =>
    Math.abs(a - b) <= 1e-9 * Math.max(Math.abs(a), Math.abs(b));

/**
 * One placed member of an {@link Aggregate}. A stable object so it can be
 * removed or moved by identity after other edits shift things.
 *
 * `offset` is the member's start in beats relative to the aggregate's context;
 * `dur` is an explicit placement length that overrides the element's own
 * `duration` when set.
 *
 * **A handle is what carries the node id**, which is what makes one element
 * placeable twice: a clip is a window onto an element, so the thing an edit
 * names is the window and not the element behind it. The conversion stamps it
 * (`./document.ts`), on the handle rather than on the element.
 */
export class Member {
    offset: number;
    dur: number | null;
    element: Element;

    constructor(offset: number, dur: number | null, element: Element) {
        this.offset = Number(offset);
        this.dur = dur === null || dur === undefined ? null : Number(dur);
        this.element = element;
    }

    /**
     * The effective length of this member: the placement `dur` if given, else
     * the element's own `duration` (may be `null`).
     */
    get length(): number | null {
        return this.dur !== null ? this.dur : this.element.duration;
    }

    /** The unit {@link Member.length} is in — the placed element's. */
    get durationUnit(): TimeUnit {
        return this.element.durationUnit;
    }

    /**
     * Where this placement ends, **in the aggregate's beats**: its offset plus
     * its length converted at `tempo` (beats per second). `null` when it has no
     * length to end at.
     */
    end(tempo = 1.0): number | null {
        const length = this.length;
        return length === null
            ? null
            : this.offset + toBeats(length, this.durationUnit, tempo);
    }
}

/** One member's placement, as {@link Aggregate.members} reports it. */
export type PlacedMember = [offset: number, dur: number | null, element: Element];

/** A child an aggregate can be seeded with. */
export type ChildSpec =
    | Element
    | readonly [offset: number, element: Element]
    | readonly [offset: number, dur: number | null, element: Element];

/** An internal bus declaration, as {@link AggregateOptions.buses} takes it. */
export type BusSpec =
    | string
    | readonly [name: string, rate: BusRate]
    | readonly [name: string, rate: BusRate, channels: number];

/** The rate an internal bus runs at. */
export type BusRate = "audio" | "control";

/** The normalized form of a bus declaration. */
interface Bus {
    name: string;
    rate: BusRate;
    channels: number;
}

/** {@link Aggregate}'s options. */
export interface AggregateOptions {
    /** The composition's name — the GraphDef name for a logical aggregate. */
    name?: string | null;
    /** Internal buses for a logical aggregate. */
    buses?: Iterable<BusSpec> | null;
    /** The aggregate's own onset in its parent context. */
    onset?: Beats;
    /** The aggregate's own duration. */
    duration?: Beats;
}

/**
 * A composite element: a set of placed members with a grouping `kind`.
 *
 * Members are placed by an `offset` (beats relative to the aggregate's context)
 * and an optional placement `dur`. Edit freely — {@link Aggregate.add},
 * {@link Aggregate.remove}, {@link Aggregate.move}; a handle returned by `add`
 * stays valid across other edits (like `seq.Timeline`).
 *
 * A `LOGICAL` aggregate additionally names the composition and may declare
 * internal buses; {@link Aggregate.toGraphdef} translates it into a `GraphDef`
 * (the bus-wired configuration the server already expresses).
 *
 * Each seeding child is an `[offset, element]` pair, an `[offset, dur, element]`
 * triple, or a bare {@link Element} (placed at offset 0).
 */
export class Aggregate extends Element {
    readonly kind: AggregateKind;
    private busSpecs: Bus[];
    private members_: Member[] = [];

    constructor(
        children: Iterable<ChildSpec> | null = null,
        kind: AggregateKind = CONCRETE,
        { name = null, buses = null, onset = null, duration = null }:
            AggregateOptions = {},
    ) {
        super(null, onset, duration, false, { name });
        if (kind !== CONCRETE && kind !== LOGICAL) {
            throw new Error(`unknown aggregate kind: ${JSON.stringify(kind)}`);
        }
        this.kind = kind;
        this.busSpecs = [...(buses ?? [])].map(busSpec);
        if (children !== null) for (const child of children) this.addChild(child);
    }

    // ---- editing ----

    /**
     * An aggregate is locatable only when every member is.
     *
     * One resident generator inside it makes the whole placement unlocatable: a
     * position on the aggregate would be a position on that member too, and it
     * has none. See {@link Element.locatable}.
     */
    override get locatable(): boolean {
        return this.members_.every((handle) => handle.element.locatable);
    }

    private addChild(child: ChildSpec): void {
        if (child instanceof Element) {
            this.add(child);
            return;
        }
        const spec = child as readonly unknown[];
        if (spec.length === 2) {
            this.add(spec[1] as Element, Number(spec[0]));
        } else if (spec.length === 3) {
            this.add(
                spec[2] as Element,
                Number(spec[0]),
                spec[1] === null || spec[1] === undefined ? null : Number(spec[1]),
            );
        } else {
            throw new Error(`invalid child spec: ${JSON.stringify(child)}`);
        }
    }

    /**
     * Places `element` at `offset` (beats), optionally overriding its length
     * with `dur`. Returns a member handle for {@link Aggregate.remove}/
     * {@link Aggregate.move}.
     */
    add(element: Element, offset = 0.0, dur: number | null = null): Member {
        const member = new Member(offset, dur, element);
        this.members_.push(member);
        return member;
    }

    /** Removes a member returned by `add` (by identity). */
    remove(member: Member): this {
        const i = this.members_.indexOf(member);
        if (i < 0) throw new Error("no such member of this aggregate");
        this.members_.splice(i, 1);
        return this;
    }

    /** Repositions `member` to `offset` (and optionally sets `dur`). */
    move(member: Member, offset: number, dur: number | null = null): Member {
        member.offset = Number(offset);
        if (dur !== null && dur !== undefined) member.dur = Number(dur);
        return member;
    }

    /** Drops every member. */
    clear(): this {
        this.members_ = [];
        return this;
    }

    // ---- reading ----

    /** The members as `[offset, dur, element]` triples, insertion order. */
    get members(): PlacedMember[] {
        return this.members_.map((m) => [m.offset, m.dur, m.element]);
    }

    /**
     * The member **handles** (the objects `add` returns), insertion order — the
     * stable identities `remove` and `move` take. Reading a placement is
     * {@link Aggregate.members}; holding on to one across edits (as an editor
     * keying its widgets by member does) needs these.
     */
    get handles(): Member[] {
        return [...this.members_];
    }

    get length(): number {
        return this.members_.length;
    }

    *[Symbol.iterator](): IterableIterator<PlacedMember> {
        for (const m of this.members_) yield [m.offset, m.dur, m.element];
    }

    // ---- the derived temporal relation ----

    /**
     * Derives this aggregate's temporal relation
     * (`SUCCESSIVE`/`SIMULTANEOUS`/`MIXED`) from its members' placements, or
     * `null` when empty.
     *
     * - `SIMULTANEOUS`: every member starts and ends together (a single member
     *   trivially qualifies).
     * - `SUCCESSIVE`: members tile contiguously in time — sorted by start, each
     *   member begins exactly where the previous ends (requires known lengths).
     * - `MIXED`: anything else.
     *
     * `tempo` (beats per second) is what puts an end on the same axis as an
     * offset: an offset is in beats and a length is in the unit of the data it
     * measures, so a take beside a phrase cannot be compared without it. An
     * aggregate whose members are all measured in beats ignores it.
     */
    temporalRelation(tempo = 1.0): TemporalRelation | null {
        const members = this.members_;
        if (members.length === 0) return null;

        const starts = members.map((m) => m.offset);
        const lengths = members.map((m) =>
            m.length === null ? null : toBeats(m.length, m.durationUnit, tempo),
        );
        const ends = starts.map((s, i) => {
            const length = lengths[i];
            return length === null ? null : s + length;
        });

        if (allClose(starts) && endsAllClose(ends)) return SIMULTANEOUS;

        if (lengths.every((length) => length !== null)) {
            const ordered = starts
                .map((s, i) => [s, lengths[i] as number] as const)
                .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
            const tiles = ordered.every((pair, i) => {
                if (i === 0) return true;
                const previous = ordered[i - 1] as readonly [number, number];
                return isClose(pair[0], previous[0] + previous[1]);
            });
            if (tiles) return SUCCESSIVE;
        }

        return MIXED;
    }

    // ---- the internal buses (a logical aggregate's private wires) ----

    /** The names of the internal buses this (logical) aggregate declares. */
    get busNames(): string[] {
        return this.busSpecs.map((spec) => spec.name);
    }

    /**
     * Declares an internal bus — a logical aggregate's private wire between
     * members. Idempotent by name: re-declaring an existing bus updates its
     * `rate`/`channels`. This is what a patcher edit (a cord drawn between two
     * members) calls to name the bus the connection implies.
     */
    declareBus(name: string, rate: BusRate = "audio", channels = 1): this {
        const spec = busSpec([String(name), rate, Math.trunc(channels)] as const);
        const i = this.busSpecs.findIndex((existing) => existing.name === spec.name);
        if (i >= 0) this.busSpecs[i] = spec;
        else this.busSpecs.push(spec);
        return this;
    }

    // ---- the logical rendering: a GraphDef ----

    /**
     * Translates this **logical** aggregate into a `GraphDef` — the 1:1 mapping
     * of the arrangement's logical grouping (nodes wired by sender/receiver
     * buses) onto the configuration the server already expresses.
     *
     * Each member must be a {@link Generator} (its `defName` is the member def;
     * its `controls` — numbers, an internal bus name, or `"OUT"` — and `maps`
     * wire it). The aggregate's buses become the private internal buses.
     * Placement offsets are ignored (a logical aggregate is a signal graph, not
     * a timeline). Returns the `GraphDef`; sending and instancing it is
     * `./render.ts`.
     */
    toGraphdef(name: string | null = null): GraphDef {
        const gname = name ?? this.name;
        if (gname === null) {
            throw new Error("a logical Aggregate needs a name to become a GraphDef");
        }
        const gdef = new GraphDef(gname);
        const refs = new Map<string, GraphBusRef>(
            this.busSpecs.map((spec) => [
                spec.name,
                gdef.bus(spec.name, { rate: spec.rate, channels: spec.channels }),
            ]),
        );
        for (const [, , child] of this.members) {
            if (!(child instanceof Generator)) {
                throw new TypeError(
                    "a logical Aggregate member must be a Generator, got " +
                        child.constructor.name,
                );
            }
            const controls: Record<string, MemberControlValue> = {};
            for (const [key, value] of Object.entries(child.controls ?? {})) {
                controls[key] =
                    typeof value === "string"
                        ? (refs.get(value) ?? value)
                        : (value as MemberControlValue);
            }
            gdef.add(child.defName, controls, { maps: child.maps ?? undefined });
        }
        return gdef;
    }
}

/**
 * Normalizes an {@link Aggregate} bus declaration (a name, or a
 * `[name, rate[, channels]]` tuple) into the shape `toGraphdef` consumes.
 */
function busSpec(bus: BusSpec): Bus {
    if (typeof bus === "string") return { name: bus, rate: "audio", channels: 1 };
    const spec = bus as readonly unknown[];
    if (spec.length === 2) {
        return { name: String(spec[0]), rate: spec[1] as BusRate, channels: 1 };
    }
    return {
        name: String(spec[0]),
        rate: spec[1] as BusRate,
        channels: Math.trunc(Number(spec[2])),
    };
}

/** True when every value is close to the first (a float-safe all-equal). */
function allClose(values: readonly number[]): boolean {
    const first = values[0] as number;
    return values.every((v) => isClose(v, first));
}

/**
 * All-equal for member ends where a `null` end (unknown length) counts as equal
 * only when every end is `null`.
 */
function endsAllClose(ends: readonly (number | null)[]): boolean {
    if (ends.every((e) => e === null)) return true;
    if (ends.some((e) => e === null)) return false;
    return allClose(ends as number[]);
}
