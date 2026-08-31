// The arrangement — elements and their temporal character (mirrors
// `clausters/form/element.py`).
//
// The client-side layer under a multitrack editor of recursive granularity: it
// places elements in time, groups them recursively and renders them. An
// `Element` is an arbitrarily delimited entity that produces a unit of meaning
// and can be decomposed or combined — *generated* (the rendered thing, editable
// and random-access) or a *generator* (the algorithm that renders it,
// forward-only), with the change of state between them. It is a **thin
// adornment** over the objects the client already has (`seq.Event`,
// `seq.Timeline`, a `Buffer`, a `Pattern`, a def): it carries the temporal
// metadata (`onset`, `duration`, and the derived temporal *character*) and
// belongs to an `Aggregate`, while it **delegates playing** to the wrapped
// item's `play(destination)` — the double-dispatch seam every leaf item in the
// client already shares. The arrangement does not reimplement or subclass those
// objects.
//
// The five primitives map one-to-one onto what the client already has:
//
// - `Clang`     — *event/clip*: parameters grouped into one action (internally
//   simultaneous), with its own onset/duration. Wraps `seq.Event`.
// - `Sequence`  — *List*: strict order with no concrete time, only sequence.
//   Wraps an array or a `Pattern`.
// - `Vector`    — *Vector*: a list at constant time (audio or control samples).
//   Wraps a `Buffer`. `Segments` is the same primitive assembled from
//   **several** windows — which buffer, from which frame, for how long — read as
//   one thing; it is not a sixth primitive, it is what a list at constant time
//   looks like when the constant time comes from more than one place.
// - `Track`     — *Set*: mixed placement of elements, a DAW track. Wraps
//   `seq.Timeline`.
// - `Generator` — *Function*: a generator element — server DSP (a def) or a
//   sequence generator (`Pbind`/`Routine`).
//
// Grouping and rendering live in `./aggregate.ts` and `./render.ts`. This module
// is pure and transport-agnostic.

import { Event as SeqEvent } from "../seq/event.ts";
import { Timeline } from "../seq/timeline.ts";
import type { PlayDestination } from "../seq/timeline.ts";
import type { RenderOptions, RenderResult } from "./render.ts";

/**
 * How an element reaches the rendering dispatch, which names every element kind
 * and therefore imports this module: `render.ts` registers itself here as it
 * loads, and these two methods read it back. The indirection is what keeps the
 * dependency one-way — a plain import both ways is a cycle, and a cycle whose
 * far end declares `class Aggregate extends Element` fails at load, not at use.
 *
 * Python has the same shape and spells it as a function-level import.
 */
const rendering: Partial<Rendering> = {};

/** The two entry points `render.ts` fills in. */
interface Rendering {
    toTimeline: (element: Element, base: number, tempo: number) => Timeline;
    render: (
        element: Element,
        destination: unknown,
        clock: unknown,
        options: RenderOptions,
    ) => RenderResult;
}

/** Registers the rendering dispatch. Called by `render.ts`, once, on load. */
export function registerRendering(impl: Rendering): void {
    Object.assign(rendering, impl);
}

function dispatch(): Rendering {
    if (rendering.render === undefined || rendering.toTimeline === undefined) {
        throw new Error(
            "the arrangement's rendering is not loaded: import the layer through " +
                "`clausters/form` (or `./render.ts`) before rendering an element",
        );
    }
    return rendering as Rendering;
}

/**
 * The temporal character of an element, derived from which of `onset` and
 * `duration` are present. `segment` has both; `punctual` has an onset but no
 * duration; `relative` has a duration but no onset; `abstract` has neither (a
 * pure context/container that only a parent gives concrete time).
 */
export const SEGMENT = "segment";
export const PUNCTUAL = "punctual";
export const RELATIVE = "relative";
export const ABSTRACT = "abstract";

/** One of the four characters above. */
export type TemporalCharacter =
    | typeof SEGMENT
    | typeof PUNCTUAL
    | typeof RELATIVE
    | typeof ABSTRACT;

/** A beat value an element may not have. */
export type Beats = number | null | undefined;

/**
 * The unit a length is in. An **onset** is always in beats — a placement is a
 * musical decision and takes the unit of what contains it — and a **duration**
 * is in the unit of its own data: `SECONDS` for audio (a take's length is
 * `frames / sampleRate`, a wall-clock fact no tempo change moves), `BEATS` for
 * a succession of events (a note is musical, and a tempo change is supposed to
 * shorten it). {@link Element.durationUnit} says which, derived from what the
 * element is made of rather than stored, and `flatten` converts on the way to a
 * timeline, which is ordered by one number and cannot hold two bases.
 */
export const BEATS = "beats";
export const SECONDS = "seconds";

/** One of the two units above. */
export type TimeUnit = typeof BEATS | typeof SECONDS;

/** `length` (in `unit`) as beats at `tempo` beats per second. */
export function toBeats(length: number, unit: TimeUnit, tempo: number): number {
    return unit === SECONDS ? Number(length) * Number(tempo) : Number(length);
}

/**
 * What a `Vector` or a `Segment` reads: a server `Buffer`, or the frozen
 * reference a document carried when this process holds no buffer for it.
 */
export interface SourceLike {
    readonly bufnum: number;
    readonly lifetime?: string;
    readonly generation?: number;
    /**
     * The shape a *held* buffer knows and a frozen reference does not: what a
     * view draws with, and what an element with no stated duration is as long
     * as. Optional because a document names a source by number and says nothing
     * about its shape — a session reopened without its sources resolved has the
     * number and nothing else.
     */
    readonly frames?: number;
    readonly channels?: number;
    readonly sampleRate?: number;
}

/** The optional label every element takes. */
export interface ElementOptions {
    name?: string | null;
}

const beats = (value: Beats): number | null =>
    value === null || value === undefined ? null : Number(value);

/**
 * The temporal character for a given `onset`/`duration` pair (the pure rule
 * behind {@link Element.temporalCharacter}).
 */
export function temporalCharacter(onset: Beats, duration: Beats): TemporalCharacter {
    const hasOnset = onset !== null && onset !== undefined;
    const hasDuration = duration !== null && duration !== undefined;
    if (hasOnset && hasDuration) return SEGMENT;
    if (hasOnset) return PUNCTUAL;
    if (hasDuration) return RELATIVE;
    return ABSTRACT;
}

/**
 * Base of the arrangement: temporal metadata over a wrapped item.
 *
 * An element carries an optional `onset` (in beats, relative to its context)
 * and `duration` (in the unit of what it wraps — see
 * {@link Element.durationUnit}) and wraps an underlying client object it
 * delegates to. The
 * concrete onset of an element typically comes from its *placement* inside an
 * {@link Aggregate}, not from the element itself, so a standalone leaf commonly
 * has a duration but no onset (a `relative` character).
 *
 * `name` is a label — what a lane is called in the editor, and, for an element
 * wrapping something the document cannot own (a pattern, a routine), the **key
 * a reopened session finds it by**. It is a label and not an identity: nothing
 * addresses an element by name, and two elements may share one, which is what
 * naming *the same algorithm used twice* looks like.
 */
export class Element {
    wraps: unknown;
    /**
     * A label, and the key an unowned leaf is handed back by. See the class
     * doc; the document carries it as the node's `name`.
     */
    name: string | null;
    onset: number | null;
    duration: number | null;
    /**
     * Whether this element's audio is produced by a def running **on the
     * server** rather than by messages the arrangement flattens. Such an
     * element is a generator with no index (see {@link Element.locatable}).
     */
    resident: boolean;

    constructor(
        wraps: unknown = null,
        onset: Beats = null,
        duration: Beats = null,
        resident = false,
        { name = null }: ElementOptions = {},
    ) {
        this.wraps = wraps;
        this.name = name ?? null;
        this.onset = beats(onset);
        this.duration = beats(duration);
        this.resident = Boolean(resident);
    }

    /**
     * Whether a position on this element means anything.
     *
     * A **generated** element has an index: the arrangement flattens it to
     * messages at absolute beats, so a transport can put itself anywhere on it.
     * A **resident generator** — a def producing its own audio on the server, a
     * stochastic process, a demand-rate sequence — has none. Its position *is*
     * its internal state, and no number moves it: the only thing a transport can
     * do to it is stop it and let it carry on.
     *
     * This is the same asymmetry the arrangement is built around, reaching the
     * transport. Pause is symmetric and works for both; locate is not. A
     * generator becomes locatable by being **rendered** — the change of state
     * from generator to generated — after which it is a buffer like any other.
     */
    get locatable(): boolean {
        return !this.resident;
    }

    /**
     * The unit `duration` is in: `SECONDS` for the elements whose data is
     * samples ({@link Vector}, {@link Segments}), and for anything wrapped that
     * measures itself in seconds (a `seq.Automation`'s curve is an envelope,
     * and an envelope's segment times are real time); `BEATS` otherwise.
     *
     * Derived from what the element is made of rather than stored, so nothing
     * can write one unit and read the other. An object that wants to answer for
     * itself declares its own `durationUnit`.
     */
    get durationUnit(): TimeUnit {
        const wraps = this.wraps as { durationUnit?: TimeUnit } | null;
        return wraps?.durationUnit === SECONDS ? SECONDS : BEATS;
    }

    /**
     * This element's character (`SEGMENT`/`PUNCTUAL`/`RELATIVE`/`ABSTRACT`),
     * derived from the presence of `onset` and `duration`.
     */
    get temporalCharacter(): TemporalCharacter {
        return temporalCharacter(this.onset, this.duration);
    }

    /**
     * Delegates playing to the wrapped item's `play(destination)` — the
     * double-dispatch seam shared by `seq.Event`, `seq.OscItem` and
     * `seq.Automation`.
     *
     * Container and pattern-backed elements ({@link Aggregate}, {@link Track}, a
     * {@link Sequence} wrapping a `Pattern`) are **not** directly playable this
     * way — they are rendered by `render()`. Delegating here requires the
     * wrapped object to follow the `play(destination)` protocol.
     */
    play(destination: PlayDestination): unknown {
        const wraps = this.wraps as { play?: (destination: PlayDestination) => unknown };
        if (wraps === null || wraps === undefined || typeof wraps.play !== "function") {
            throw new Error(
                `${this.constructor.name} is not directly playable; use render()`,
            );
        }
        return wraps.play(destination);
    }

    /**
     * Flattens this element to a flat `seq.Timeline` in absolute beats
     * (accumulating nested placement offsets), converting any length measured
     * in seconds at `tempo`. See `./render.ts`.
     */
    toTimeline(base = 0.0, tempo = 1.0): Timeline {
        return dispatch().toTimeline(this, base, tempo);
    }

    /**
     * Renders this element onto `destination` — the change of state to sound. A
     * concrete element flattens and plays through a `seq.Playhead` over `clock`
     * (returns the playhead); a logical {@link Aggregate} sends and instances a
     * `GraphDef` on the server (returns a promise of the instance group). See
     * `./render.ts`.
     */
    render(
        destination: unknown,
        clock?: unknown,
        options: RenderOptions = {},
    ): RenderResult {
        return dispatch().render(this, destination, clock, options);
    }
}

/**
 * *event/clip*: parameters grouped into one action, internally simultaneous.
 *
 * Wraps a `seq.Event` (or a plain object of parameters). Its `duration` defaults
 * to the event's `dur` when not given explicitly; its `onset` usually comes from
 * its placement in an {@link Aggregate}.
 */
export class Clang extends Element {
    constructor(
        event: SeqEvent | Record<string, unknown>,
        onset: Beats = null,
        duration: Beats = null,
        { name = null }: ElementOptions = {},
    ) {
        const wrapped = event instanceof SeqEvent ? event : new SeqEvent(event);
        let dur = beats(duration);
        if (dur === null) {
            const own = wrapped.get("dur");
            if (own !== null && own !== undefined) dur = Number(own);
        }
        super(wrapped, onset, dur, false, { name });
    }
}

/**
 * *List*: strict order with no concrete time — only sequence.
 *
 * Wraps an array or a `seq.Pattern`. The items can be numbers, events, notes or
 * whole elements; the structure fixes only their successive order. Rendering
 * bounces a pattern-backed sequence; an array is interpreted by its content.
 */
export class Sequence extends Element {
    constructor(
        items: unknown,
        onset: Beats = null,
        duration: Beats = null,
        { name = null }: ElementOptions = {},
    ) {
        super(items, onset, duration, false, { name });
    }
}

/** What a `Vector` or a `Segments` passes on to the instrument that plays it. */
export type EventControls = Record<string, unknown>;

/** {@link Vector}'s options. */
export interface VectorOptions extends ElementOptions {
    instrument?: string | null;
    controls?: EventControls | null;
    start?: number;
    loop?: boolean;
}

/**
 * *Vector*: a list at constant time — audio or control samples.
 *
 * Wraps a `Buffer`. An automation sampled at a constant interval is a control
 * buffer (the List/Vector duality of the arrangement).
 *
 * A buffer is *data*, so rendering it as an **audio clip** needs an instrument:
 * the def that plays it, named by `instrument` (a synth whose `buf` control
 * takes the buffer number, as a sampler's does). Rendering then emits one event
 * playing that def — {@link Vector.toEvent}. Without an instrument the element
 * is still perfectly good structure (and the editor draws its take), it simply
 * has no sound of its own.
 *
 * `start` is the first frame of the buffer this element reads. An element is a
 * **window onto a segment** of its buffer, not the whole of it: a trimmed take
 * reads from further in and the frames before it are still there, which is what
 * lets a trim be undone and a split give two windows over one buffer. `loop`
 * says whether that window wraps around the buffer — past the last frame it
 * begins again.
 *
 * A window that is not the whole buffer travels to the instrument as the
 * `start`/`loop` event parameters, so a def that reads them (a sampler whose
 * `PlayBuf` takes a `startPos` and a `loop`) plays exactly the segment the
 * editor draws. An element reading its buffer from the beginning sends neither,
 * so a def written before windows existed is sent what it always was.
 */
export class Vector extends Element {
    instrument: string | null;
    controls: EventControls;
    /**
     * The **first frame of the buffer this element reads** — the head of its
     * window onto the buffer. Trimming a clip moves it; splitting one in two
     * gives each half a window of its own over the same buffer.
     */
    start: number;
    /**
     * Whether the window **wraps** around the buffer: past the last frame it
     * begins again, which is what stretching an element beyond the buffer means
     * when a loop is what it is.
     */
    loop: boolean;

    constructor(
        buffer: SourceLike,
        onset: Beats = null,
        duration: Beats = null,
        { instrument = null, controls = null, start = 0.0, loop = false, name = null }:
            VectorOptions = {},
    ) {
        super(buffer, onset, duration, false, { name });
        this.instrument = instrument ?? null;
        this.controls = { ...(controls ?? {}) };
        this.start = Number(start);
        this.loop = Boolean(loop);
    }

    /** The buffer this element reads. */
    get buffer(): SourceLike {
        return this.wraps as SourceLike;
    }

    /**
     * `SECONDS`: this element's data is samples, and their seconds were
     * fixed when they were recorded — a tempo change does not shorten a take.
     */
    override get durationUnit(): TimeUnit {
        return SECONDS;
    }

    /**
     * The event that plays this buffer: the `instrument` def with the buffer
     * number in its `buf` control, sounding for the element's `duration`.
     *
     * `tempo` (beats per second) is what the length crosses on: this element's
     * duration is in seconds and an event's `dur` is in beats, because an event
     * is played by a clock. It is the only conversion, and it happens here
     * rather than in the structure.
     *
     * `legato` is 1 so the take sounds its whole length (the note default of 0.8
     * would cut it short — a sampled take is not a note with a gap), and `amp`
     * is 1 for the same reason at the other end: the note default mixes an event
     * **20 dB down**, which is a headroom convention for stacking notes and
     * simply attenuates recorded audio. A take arrives at the level it was
     * recorded at; anything else is a mix decision, so it goes in `controls`
     * (which overrides both).
     */
    toEvent(tempo = 1.0): SeqEvent {
        if (this.instrument === null) {
            throw new Error(
                "a Vector needs an instrument to be rendered as an audio clip " +
                    "(new Vector(buf, null, null, { instrument: 'take' }): a def " +
                    "whose `buf` control plays it)",
            );
        }
        const params: Record<string, unknown> = {
            instrument: this.instrument,
            buf: this.buffer.bufnum,
            legato: 1.0,
            amp: 1.0,
        };
        // The window, so what is heard is the segment that is drawn — and only
        // when there is one to state, so a def that never heard of windows is
        // sent exactly what it was always sent.
        if (this.start) params.start = Number(this.start);
        if (this.loop) params.loop = 1.0;
        if (this.duration !== null) params.dur = Number(this.duration) * Number(tempo);
        Object.assign(params, this.controls);
        return new SeqEvent(params);
    }
}

/** One segment's spec, as {@link Segment.of} takes it. */
export type SegmentSpec =
    | Segment
    | readonly [source: SourceLike, start: number, duration: number]
    | readonly [source: SourceLike, duration: number];

/**
 * One segment of a {@link Segments}: which buffer, from which frame, for how
 * long. A window, named the same way a {@link Vector} element's is.
 *
 * `start` is in frames and `duration` in **seconds** — one base for both, and
 * the base these samples are already in.
 */
export class Segment {
    readonly buffer: SourceLike;
    readonly start: number;
    readonly duration: number;

    constructor(buffer: SourceLike, start = 0.0, duration: number | null = null) {
        this.buffer = buffer;
        this.start = Number(start);
        this.duration = duration === null ? 0.0 : Number(duration);
    }

    /**
     * A segment from a triple `[buffer, start, duration]`, a pair
     * `[buffer, duration]`, or one of these.
     */
    static of(spec: SegmentSpec): Segment {
        if (spec instanceof Segment) return spec;
        const items = spec as readonly unknown[];
        if (items.length === 3) {
            return new Segment(
                items[0] as SourceLike,
                Number(items[1]),
                Number(items[2]),
            );
        }
        if (items.length === 2) {
            return new Segment(items[0] as SourceLike, 0.0, Number(items[1]));
        }
        throw new TypeError(
            "a segment is [buffer, start, duration] or [buffer, duration], " +
                `not ${JSON.stringify(spec)}`,
        );
    }

    equals(other: unknown): boolean {
        return (
            other instanceof Segment &&
            other.buffer === this.buffer &&
            other.start === this.start &&
            other.duration === this.duration
        );
    }
}

/** {@link Segments}'s options. */
export interface SegmentsOptions extends ElementOptions {
    instrument?: string | null;
    controls?: EventControls | null;
}

/**
 * *Several windows read as one*: data assembled from segments of one or more
 * buffers, which sound as a single thing.
 *
 * A {@link Vector} is one window onto one buffer. This is what a **join** makes
 * when the fragments do not come from one place: a list of
 * `[buffer, start, duration]` — the buffer to read, the frame to read it from,
 * and how long that segment lasts in seconds — read back to back. It is the same
 * memory-view idea one level up: nothing is copied, and cutting one of these
 * apart again gives back windows over the same buffers.
 *
 * Rendering emits **one event per segment**, each at its own offset inside the
 * element and each carrying its own window, so the segments sound continuous on
 * one instrument. The editor draws it as **one clip** holding one take per
 * segment, each over its own stretch of the clip.
 *
 * `instrument` is one def for all of them, since what this element *is* is one
 * thing to play (see {@link Vector}). `duration` is in **seconds**, the sum of
 * the segments' when not given.
 */
export class Segments extends Element {
    instrument: string | null;
    controls: EventControls;

    constructor(
        segments: Iterable<SegmentSpec>,
        onset: Beats = null,
        duration: Beats = null,
        { instrument = null, controls = null, name = null }: SegmentsOptions = {},
    ) {
        const parsed = [...segments].map((s) => Segment.of(s));
        let dur = beats(duration);
        if (dur === null && parsed.length > 0) {
            dur = parsed.reduce((sum, seg) => sum + seg.duration, 0.0);
        }
        super(parsed, onset, dur, false, { name });
        this.instrument = instrument ?? null;
        this.controls = { ...(controls ?? {}) };
    }

    /** `SECONDS`, like the {@link Vector} this is the several-windows form of. */
    override get durationUnit(): TimeUnit {
        return SECONDS;
    }

    /** The segments, in reading order — the element's own data. */
    get segments(): Segment[] {
        return [...((this.wraps as Segment[] | null) ?? [])];
    }

    /**
     * The segments with the second each one **starts at** inside this element:
     * `[offset, segment]` pairs, which is what both rendering and drawing lay
     * out from. Seconds throughout, like the lengths they accumulate.
     */
    placed(): [number, Segment][] {
        const out: [number, Segment][] = [];
        let cursor = 0.0;
        for (const seg of this.segments) {
            out.push([cursor, seg]);
            cursor += seg.duration;
        }
        return out;
    }

    /**
     * One `[offset, event]` per segment: the instrument playing that buffer,
     * from that frame, for that long. The offsets are relative to the element,
     * exactly as an aggregate's members' are — and in **beats**, converted here
     * at `tempo` (beats per second) from the seconds the windows are measured
     * in, because what comes out of this is played by a clock.
     */
    toEvents(tempo = 1.0): [number, SeqEvent][] {
        if (this.instrument === null) {
            throw new Error(
                "a Segments needs an instrument to be rendered as audio " +
                    "(new Segments(..., null, null, { instrument: 'take' }): a def " +
                    "whose `buf` control plays a buffer, reading `start` for the " +
                    "window)",
            );
        }
        const out: [number, SeqEvent][] = [];
        for (const [offset, seg] of this.placed()) {
            const params: Record<string, unknown> = {
                instrument: this.instrument,
                buf: seg.buffer.bufnum,
                legato: 1.0,
                amp: 1.0,
                dur: Number(seg.duration) * Number(tempo),
            };
            if (seg.start) params.start = Number(seg.start);
            Object.assign(params, this.controls);
            out.push([offset * Number(tempo), new SeqEvent(params)]);
        }
        return out;
    }
}

/**
 * *Set*: mixed placement of elements — a DAW track.
 *
 * Wraps a `seq.Timeline` (free placement of items by beat). A fresh empty
 * `Timeline` is created when none is given.
 */
export class Track extends Element {
    constructor(
        timeline: Timeline | null = null,
        onset: Beats = null,
        duration: Beats = null,
        { name = null }: ElementOptions = {},
    ) {
        super(timeline ?? new Timeline(), onset, duration, false, { name });
    }

    /** The timeline this track places its items on. */
    get timeline(): Timeline {
        return this.wraps as Timeline;
    }
}

/** {@link Generator}'s options. */
export interface GeneratorOptions extends ElementOptions {
    controls?: Record<string, unknown> | null;
    maps?: Record<string, string> | null;
    rendered?: Element | null;
}

/**
 * *Function*: a generator element.
 *
 * Wraps either server DSP (a `SynthDef`/`FaustDef`/`GraphDef`, or a def name) or
 * a sequence generator (a `Pbind`/`Routine`). Its *change of state* — evaluating
 * the generator into a generated element — happens at rendering: a contained
 * event pattern is bounced to a timeline; a def member of a logical
 * {@link Aggregate} becomes a wired GraphDef member.
 *
 * `controls` are control values for a logical-graph member — numbers, an
 * internal-bus name (a string matching an {@link Aggregate} bus), or `"OUT"`
 * (hardware); `maps` binds controls to control buses (`/node_map`). Both are
 * read by `Aggregate.toGraphdef`.
 *
 * `rendered` is what this generator **last produced**, as an ordinary
 * {@link Element} — the change of state above, kept rather than recomputed. It
 * is what a host with no language attached shows, since a generator is code and
 * such a host has nothing to run it with; and it is what a saved session carries
 * for the same reason a cache cannot, which is that a missing cache leaves
 * nothing to draw.
 */
export class Generator extends Element {
    controls: Record<string, unknown> | null;
    maps: Record<string, string> | null;
    /**
     * The last rendered result, or `null` before there is one. Read-only as far
     * as editing goes: it is a rendering, not the composition, so an edit to it
     * would be written over by the next render.
     */
    rendered: Element | null;

    constructor(
        generator: unknown,
        onset: Beats = null,
        duration: Beats = null,
        { controls = null, maps = null, rendered = null, name = null }:
            GeneratorOptions = {},
    ) {
        super(generator, onset, duration, false, { name });
        this.controls = controls ?? null;
        this.maps = maps ?? null;
        this.rendered = rendered ?? null;
    }

    /**
     * The member def name — the wrapped string itself, or the def object's
     * `name`.
     */
    get defName(): string {
        const wraps = this.wraps;
        if (typeof wraps === "string") return wraps;
        return String((wraps as { name?: unknown } | null)?.name);
    }
}
