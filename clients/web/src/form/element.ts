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

import { TempoMap } from "../base/time.ts";
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
export { BEATS, SECONDS } from "../base/time.ts";
export type { TimeUnit } from "../base/time.ts";

import { BEATS, SECONDS } from "../base/time.ts";
import type { TimeUnit } from "../base/time.ts";

/** `length` (in `unit`) as beats at `tempo` beats per second. */
export function toBeats(length: number, unit: TimeUnit, tempo: number): number {
    return unit === SECONDS ? Number(length) * Number(tempo) : Number(length);
}


/**
 * The map to measure with: the one given, or `tempo` as a single constant
 * segment — which is the affine ratio every one of these conversions used to
 * be, so a caller that names no map gets exactly what it always got.
 */
export function tempoMapOf(tempoMap?: TempoMap | null, tempo = 1.0): TempoMap {
    return tempoMap ?? new TempoMap(tempo);
}

/**
 * The beat that `length` (in `unit`) reaches, starting at beat `at`.
 *
 * Two positions, never a length and a ratio. A length in **beats** is already
 * on the axis and simply lands at `at + length`; a length in **seconds** is a
 * wall-clock fact whose end depends on how the tempo runs across it, which is
 * what the piece's map ({@link TempoMap}) answers.
 */
export function endBeat(at: number, length: number, unit: string, tempoMap: TempoMap): number {
    if (unit !== SECONDS) return at + length;
    return tempoMap.beatsAt(tempoMap.secsAt(at) + length);
}

// The windows are **not** the arrangement's: a segment is about the contents,
// not about where it sits in a piece, so `Segment`, the runs and what they read
// live beside the structures (`../segments.ts`) and this module reads them like
// any other reader. Re-exported here because `Segments` is the element that
// places one.
export { BufferSegments, NoteSegments, Segment, SegmentRun } from "../segments.ts";
export type { SegmentSpec, SourceLike, TimelineLike } from "../segments.ts";
import { BufferSegments, Segment } from "../segments.ts";
import type { SegmentSpec, SourceLike } from "../segments.ts";

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
    /**
     * **Mixing: the composition's, not the view's.** Whether this element is
     * silenced (`mute`), whether it is one of the elements soloed (`solo`), and
     * the gain its events sound at (`level`, a factor over an event's own
     * `amp`). They are set by the editor's lane header and by hand, they are
     * honoured by {@link flatten}, and they travel in the node's configuration —
     * so a piece reopens muted the way it was left. A lane's *height* is the
     * other kind of thing and is deliberately absent: it says nothing about what
     * the piece is, so no document carries it.
     */
    mute = false;
    solo = false;
    level = 1.0;

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

    // -- windows: what a trim, a split and a join ask an element -------------
    //
    // **The question is the contents', never the class's.** Cutting is defined
    // wherever there is an addressable time axis — samples, notes, events,
    // segments — so the verb asks the element whether it has one instead of
    // testing what it is. What genuinely answers no is a **generator**: not
    // "cannot be cut" but *not until it is rendered*, which is the change of
    // state the model already has a verb for.

    /**
     * Where this element **reads from** inside what it holds, or `null` when it
     * holds no window at all.
     *
     * In the unit the contents are *addressed* in — frames for samples, beats
     * for events — which is the same coordinate {@link Segment.start} is in and
     * for the same reason.
     */
    windowStart(): number | null {
        return null;
    }

    /**
     * The element the **second half** of a cut at `at` reads, or `null` when
     * this element cannot be cut.
     *
     * `at` and `length` are in this element's own unit ({@link durationUnit}),
     * and `rate` is the sample rate to bridge with when the contents are
     * addressed in frames and the source does not know its own — the one number
     * an element may need from the caller.
     *
     * The **first** half is never built: it is the element it always was, with
     * its placement shortened, which is the arrangement's rule (a placement is a
     * window onto an element, never a rewrite of it) and what makes an undo of a
     * split one step. Nothing is copied and nothing is lost either way —
     * lengthening a half brings back exactly what the cut hid.
     */
    windowed(at: number, length: number, rate = 0.0): Element | null {
        void at;
        void length;
        void rate;
        return null;
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
 * Wraps a `seq.Event` (or a plain object of parameters), and equally an
 * `OscItem` or a `MidiItem` — an action that happens at one moment is a clang
 * whether it is a note or a message, which is what `Element.play`'s double
 * dispatch has always assumed and what a timeline written into a document is
 * read back as. Anything that plays itself is taken as it is; anything else is
 * the parameters of an event.
 *
 * Its `duration` defaults to the event's `dur` when not given explicitly; its
 * `onset` usually comes from its placement in an {@link Aggregate}.
 */
export class Clang extends Element {
    constructor(
        event: SeqEvent | Record<string, unknown>,
        onset: Beats = null,
        duration: Beats = null,
        { name = null }: ElementOptions = {},
    ) {
        const plays = typeof (event as { play?: unknown })?.play === "function";
        const wrapped = plays
            ? (event as SeqEvent)
            : new SeqEvent(event as Record<string, unknown>);
        let dur = beats(duration);
        if (dur === null) {
            const own = wrapped instanceof SeqEvent ? wrapped.get("dur") : null;
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
     * The frame this element reads from — it has had a window since trimming
     * existed.
     */
    override windowStart(): number | null {
        return this.start;
    }

    /**
     * The same buffer, read from `at` seconds further in. The frames neither
     * half shows are still there, which is why stretching either one brings them
     * back.
     */
    override windowed(at: number, length: number, rate = 0.0): Vector {
        const hz = Number(this.wraps ? (this.buffer.sampleRate ?? 0) : 0) || rate || 0;
        return new Vector(this.buffer, null, length - at, {
            instrument: this.instrument,
            controls: this.controls,
            start: this.start + at * hz,
            loop: this.loop,
            name: this.name,
        });
    }

    /**
     * The event that plays this buffer: the `instrument` def with the buffer
     * number in its `buf` control, sounding for the element's `duration`.
     *
     * `tempoMap` is what the length crosses on: this element's duration is in
     * seconds and an event's `dur` is in beats, because an event is played by a
     * clock. It is the only conversion, and it happens here rather than in the
     * structure. `at` is the beat the take starts on, which the crossing needs:
     * the same stretch of seconds is a different number of beats depending on
     * where the tempo has got to.
     *
     * `legato` is 1 so the take sounds its whole length (the note default of 0.8
     * would cut it short — a sampled take is not a note with a gap), and `amp`
     * is 1 for the same reason at the other end: the note default mixes an event
     * **20 dB down**, which is a headroom convention for stacking notes and
     * simply attenuates recorded audio. A take arrives at the level it was
     * recorded at; anything else is a mix decision, so it goes in `controls`
     * (which overrides both).
     */
    toEvent(tempoMap?: TempoMap | null, at = 0.0): SeqEvent {
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
        if (this.duration !== null) {
            // Two positions, not a length times a ratio: the take's seconds are
            // fixed, and how many beats they cover depends on where it starts.
            const map = tempoMapOf(tempoMap);
            params.dur = endBeat(at, Number(this.duration), SECONDS, map) - at;
        }
        Object.assign(params, this.controls);
        return new SeqEvent(params);
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
/**
 * A recorded or loaded buffer **placed in the arrangement**: a {@link Vector}
 * whose length is the samples' own.
 *
 * This is where recording lands. A `RecordingStream` follows takes as they are
 * written and a `Buffer` holds them, but neither puts one in a piece — and the
 * arithmetic that does (frames over the rate they were recorded at) was left to
 * every caller, which is one conversion written once per script and wrong in the
 * one that forgot the channel count is not in it.
 *
 * `duration` is in **seconds**, for a caller who knows better than the buffer
 * does — a take still recording, whose buffer is as long as it will be rather
 * than as long as it is. Without an `instrument` the take is structure (it draws
 * and it extends the piece, and it emits no event), which is the `Vector` rule
 * and not a special case here. `sampleRate` is the rate to measure the length
 * at, for a source that does not know its own; when nothing knows it the
 * duration is `null`, which is the honest answer — the length is then the
 * placement's.
 */
export function take(
    buffer: SourceLike,
    onset: Beats = null,
    duration: Beats = null,
    { sampleRate = 0, ...options }: VectorOptions & { sampleRate?: number } = {},
): Vector {
    const rate = Number(sampleRate || buffer?.sampleRate || 0);
    const frames = Number(buffer?.frames ?? 0);
    const length = duration ?? (rate > 0 && frames > 0 ? frames / rate : null);
    return new Vector(buffer, onset, length, options);
}

export class Segments extends Element {
    /** The windows themselves, as the general structure they are. */
    readonly run: BufferSegments;
    instrument: string | null;
    controls: EventControls;

    constructor(
        segments: Iterable<SegmentSpec<SourceLike>>,
        onset: Beats = null,
        duration: Beats = null,
        { instrument = null, controls = null, name = null }: SegmentsOptions = {},
    ) {
        const run = new BufferSegments(segments, { instrument, controls });
        let dur = beats(duration);
        if (dur === null && run.length > 0) dur = run.total;
        super(run.segments, onset, dur, false, { name });
        this.run = run;
        this.instrument = instrument ?? null;
        this.controls = { ...(controls ?? {}) };
    }

    /**
     * The run's own — `SECONDS`, because these windows are onto samples. Asked
     * of the data rather than stated here, which is what lets the same element
     * place a run of any contents.
     */
    override get durationUnit(): TimeUnit {
        return this.run.unit;
    }

    /** The segments, in reading order — the element's own data. */
    get segments(): Segment<SourceLike>[] {
        return this.run.segments;
    }

    /**
     * Zero: a run's window is in its segments, each of which carries its own —
     * so there is no single frame this element reads from, and a trim moves the
     * windows rather than a head.
     */
    override windowStart(): number | null {
        return 0.0;
    }

    /**
     * The windows past the cut, with the one the cut falls inside cut in two —
     * which is `SegmentRun.cut` (`../segments.ts`), the arithmetic this element
     * places rather than reimplements.
     *
     * A tail that is **one run of one buffer** comes back as the plain
     * {@link Vector} it is ({@link BufferSegments.contiguous}): that is not an
     * optimization, it is what makes a cut and a join inverses instead of a pile
     * of wrappers.
     */
    override windowed(at: number, length: number, rate = 0.0): Element {
        void length;
        void rate;
        const [, tail] = this.run.cut(at) as [BufferSegments, BufferSegments];
        if (tail.contiguous) {
            const first = tail.segments[0];
            return new Vector(first.source, null, tail.total, {
                instrument: this.instrument,
                controls: this.controls,
                start: first.start,
                name: this.name,
            });
        }
        return new Segments(tail.segments, null, null, {
            instrument: this.instrument,
            controls: this.controls,
            name: this.name,
        });
    }

    /**
     * The segments with the second each one **starts at** inside this element:
     * `[offset, segment]` pairs, which is what both rendering and drawing lay
     * out from.
     */
    placed(): [number, Segment<SourceLike>][] {
        return this.run.placed();
    }

    /**
     * One `[offset, event]` per segment: the instrument playing that buffer,
     * from that frame, for that long. The offsets are relative to the element,
     * exactly as an aggregate's members' are — and in **beats**, converted here
     * from the seconds the windows are measured in, because what comes out of
     * this is played by a clock.
     *
     * `tempoMap` is the piece's, and `at` the beat this element starts on: each
     * window is placed and sized from where it actually falls, so a tempo change
     * inside the element moves the segments after it and not the ones before.
     */
    toEvents(tempoMap?: TempoMap | null, at = 0.0): [number, SeqEvent][] {
        if (this.instrument === null) {
            throw new Error(
                "a Segments needs an instrument to be rendered as audio " +
                    "(new Segments(..., null, null, { instrument: 'take' }): a def " +
                    "whose `buf` control plays a buffer, reading `start` for the " +
                    "window)",
            );
        }
        const map = tempoMapOf(tempoMap);
        const out: [number, SeqEvent][] = [];
        for (const [offset, seg] of this.placed()) {
            // Both numbers are seconds and both are placed, not scaled: the
            // window opens at the beat those seconds reach from `at`, and lasts
            // to the beat its own seconds reach from there.
            const onset = endBeat(at, offset, SECONDS, map);
            const end = endBeat(onset, Number(seg.duration), SECONDS, map);
            const params: Record<string, unknown> = {
                instrument: this.instrument,
                legato: 1.0,
                amp: 1.0,
                dur: end - onset,
            };
            Object.assign(params, this.run.eventParams(seg));
            Object.assign(params, this.controls);
            out.push([onset - at, new SeqEvent(params)]);
        }
        return out;
    }
}

/** {@link Track}'s options. */
export interface TrackOptions extends ElementOptions {
    /**
     * The beat of the timeline this element **reads from**. A track is a window
     * onto its timeline exactly as a {@link Vector} is a window onto its buffer,
     * and for the same reason: a trim reads from further in, a split gives two
     * windows over one timeline, and the notes neither window shows are still on
     * it — so lengthening either half brings them back. A cut is not a rewrite
     * of the notes.
     */
    start?: number;
}

/**
 * *Set*: mixed placement of elements — a DAW track.
 *
 * Wraps a `seq.Timeline` (free placement of items by beat). A fresh empty
 * `Timeline` is created when none is given. With `start`, it is a **window**
 * onto that timeline: `duration` is then how much of it this element is.
 */
export class Track extends Element {
    /**
     * The **beat of the timeline this element reads from** — the head of its
     * window, the beats counterpart of {@link Vector.start}.
     */
    start: number;

    constructor(
        timeline: Timeline | null = null,
        onset: Beats = null,
        duration: Beats = null,
        { start = 0.0, name = null }: TrackOptions = {},
    ) {
        super(timeline ?? new Timeline(), onset, duration, false, { name });
        this.start = Number(start);
    }

    /** The timeline this track places its items on. */
    get timeline(): Timeline {
        return this.wraps as Timeline;
    }

    /** The beat this element reads its timeline from. */
    override windowStart(): number | null {
        return this.start;
    }

    /**
     * The same timeline, read from `at` beats further in. Both units are beats
     * here, so there is nothing to bridge — the notes outside either window are
     * on the timeline, not gone.
     */
    override windowed(at: number, length: number, rate = 0.0): Track {
        void rate;
        return new Track(this.timeline, null, length - at, {
            start: this.start + at,
            name: this.name,
        });
    }

    /**
     * The `[beat, item]` pairs this element **shows**: its window's, placed from
     * the element's own zero.
     *
     * The whole timeline when it has no window (a track written by a script is
     * the timeline), and the window's contents shifted back to zero when a trim
     * or a split gave it one. What falls outside is not here and is not gone.
     */
    items(): [number, unknown][] {
        const entries = [...this.timeline] as [number, unknown][];
        if (!this.start && this.duration === null) {
            return entries.map(([beat, item]) => [Number(beat), item]);
        }
        const end = this.duration === null ? Infinity : this.start + Number(this.duration);
        return entries
            .filter(([beat]) => Number(beat) >= this.start && Number(beat) < end)
            .map(([beat, item]) => [Number(beat) - this.start, item]);
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
