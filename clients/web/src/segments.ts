// Segments: a window onto contents, and a run of windows read as one (mirrors
// `clausters/segments.py`).
//
// A **segment** is a window: which source, from where, for how long. A **run**
// of them is what a **join** assembles and what a **split** takes apart, read
// back to back as a single thing. Neither idea belongs to the arrangement — a
// window is about the *contents*, not about where they sit in a piece
// — so they live here, beside the structures, and `form` reads them like any
// other reader.
//
// What is general and what is the source's: the order of the windows, where
// each starts inside the run, how long the run is, where a cut falls and what
// two runs make when joined is arithmetic over lengths, written once in
// `SegmentRun`. What only the source knows is the two hooks a subclass fills
// in — how a position advances by a length (`advanced`), because a window's
// `start` is in the unit the source is *addressed* in while a length is in the
// unit it *measures*, and what one window is played as.
//
// Nothing is copied: cutting a run and joining the halves gives the run back,
// and re-lengthening a window brings out what the cut hid. That
// property is why notes want windows too, and not a destructive cut.

import { BEATS, SECONDS } from "./base/time.ts";
import type { TimeUnit } from "./base/time.ts";

/**
 * What a window onto **samples** is onto: a server `Buffer`, or the frozen
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
    /**
     * The **file these samples came from**, when they came from one. Carried and
     * never acted on: what it is for is saying where the samples are when a
     * piece is written down, which is exactly what a session's source table
     * needs and the one thing a bare slot number cannot tell it.
     */
    readonly path?: string | null;
}

/** What a window onto **events** is onto: a timeline, read by beat range. */
export interface TimelineLike {
    range(t0: number, t1: number): [number, unknown][];
}

/** One segment's spec, as {@link Segment.of} takes it. */
export type SegmentSpec<S = SourceLike> =
    | Segment<S>
    | readonly [source: S, start: number, duration: number]
    | readonly [source: S, duration: number];

/**
 * One window: which source, from which position, for how long.
 *
 * `start` is in the unit the **source is addressed in** (frames for samples,
 * beats for a timeline of events) and `duration` in the unit the source
 * **measures** (seconds for samples, beats for events). The run says which,
 * through {@link SegmentRun.unit}, and bridges the two in one place
 * ({@link SegmentRun.advanced}) so nothing else has to know a sample rate.
 */
export class Segment<S = SourceLike> {
    readonly source: S;
    readonly start: number;
    readonly duration: number;

    constructor(source: S, start = 0.0, duration: number | null = null) {
        this.source = source;
        this.start = Number(start);
        this.duration = duration === null ? 0.0 : Number(duration);
    }

    /**
     * A segment from a triple `[source, start, duration]`, a pair
     * `[source, duration]`, or one of these.
     */
    static of<S>(spec: SegmentSpec<S>): Segment<S> {
        if (spec instanceof Segment) return spec;
        const items = spec as readonly unknown[];
        if (items.length === 3) {
            return new Segment<S>(items[0] as S, Number(items[1]), Number(items[2]));
        }
        if (items.length === 2) {
            return new Segment<S>(items[0] as S, 0.0, Number(items[1]));
        }
        throw new TypeError(
            "a segment is [source, start, duration] or [source, duration], " +
                `not ${JSON.stringify(spec)}`,
        );
    }

    /** The source, under the name a run of samples has always called it. */
    get buffer(): S {
        return this.source;
    }

    equals(other: unknown): boolean {
        return (
            other instanceof Segment &&
            other.source === this.source &&
            other.start === this.start &&
            other.duration === this.duration
        );
    }
}

/**
 * Several windows read as one: the general run, and the arithmetic that is the
 * same whatever the windows are onto.
 *
 * Subclasses say what the contents are — {@link BufferSegments} over samples,
 * {@link NoteSegments} over a timeline of events — by answering `unit`,
 * `advanced` and what a window is played as. Everything else here is length
 * arithmetic and holds for both.
 */
export class SegmentRun<S = SourceLike> {
    readonly segments: Segment<S>[];

    constructor(segments: Iterable<SegmentSpec<S>>) {
        this.segments = [...segments].map((s) => Segment.of<S>(s));
    }

    /** The unit the segments' lengths are in, and therefore the run's own. */
    get unit(): TimeUnit {
        return BEATS;
    }

    /**
     * `start` moved forward by the length `by`, in the source's own addressing
     * unit — the one bridge between the two units a window carries. The default
     * is the case where there is nothing to bridge, which is every source
     * addressed in what it measures.
     */
    advanced(start: number, by: number): number {
        return Number(start) + Number(by);
    }

    /** The run's length: its segments', added up, in {@link unit}. */
    get total(): number {
        return this.segments.reduce((sum, seg) => sum + seg.duration, 0.0);
    }

    /**
     * `[offset, segment]` pairs — where each window starts *inside* the run,
     * which is what both rendering and drawing lay out from. In {@link unit}
     * throughout, like the lengths they accumulate.
     */
    placed(): [number, Segment<S>][] {
        const out: [number, Segment<S>][] = [];
        let cursor = 0.0;
        for (const seg of this.segments) {
            out.push([cursor, seg]);
            cursor += seg.duration;
        }
        return out;
    }

    /**
     * The run split at `at` (in {@link unit}, from the run's own start): two
     * runs of the same kind, over the same sources.
     *
     * The window the cut falls inside becomes two windows — the first ends
     * early, the second opens where the first stopped — so nothing is copied
     * and nothing is lost: joining them back gives this run, and lengthening
     * either half brings out again what it hides. A cut at or past either
     * end gives one empty run and one whole one, which is the honest answer to
     * a cut that took nothing.
     */
    cut(at: number): [SegmentRun<S>, SegmentRun<S>] {
        const head: Segment<S>[] = [];
        const tail: Segment<S>[] = [];
        for (const [offset, seg] of this.placed()) {
            const end = offset + seg.duration;
            if (end <= at) {
                head.push(new Segment<S>(seg.source, seg.start, seg.duration));
            } else if (offset >= at) {
                tail.push(new Segment<S>(seg.source, seg.start, seg.duration));
            } else {
                const first = at - offset;
                head.push(new Segment<S>(seg.source, seg.start, first));
                tail.push(
                    new Segment<S>(
                        seg.source,
                        this.advanced(seg.start, first),
                        seg.duration - first,
                    ),
                );
            }
        }
        return [this.like(head), this.like(tail)];
    }

    /**
     * This run followed by `other`: the inverse of {@link cut}, and the reason
     * both are the same action over any contents.
     */
    joined(other: SegmentRun<S>): SegmentRun<S> {
        return this.like([...this.segments, ...other.segments]);
    }

    /**
     * Another run of this kind over `segments`, carrying whatever configuration
     * this one has. Subclasses that add configuration override it; the
     * arithmetic above only ever builds runs through here.
     */
    like(segments: Segment<S>[]): SegmentRun<S> {
        return new SegmentRun<S>(segments);
    }

    get length(): number {
        return this.segments.length;
    }

    [Symbol.iterator](): Iterator<Segment<S>> {
        return this.segments[Symbol.iterator]();
    }
}

/** The configuration a run carries: one way of playing the whole of it. */
export interface SegmentRunOptions {
    instrument?: string | null;
    controls?: Record<string, unknown> | null;
}

/**
 * A run of windows onto **samples**: which buffer, from which frame, for how
 * long.
 *
 * Lengths are in seconds — a recording's seconds were fixed when it was
 * recorded and no tempo change moves them — while a window's `start` is the
 * frame it opens at, which is the coordinate the samples are already in and the
 * one a def's `start` control reads. {@link advanced} is where the two meet, and
 * it is the only place in this file that knows what a sample rate is.
 */
export class BufferSegments extends SegmentRun<SourceLike> {
    instrument: string | null;
    controls: Record<string, unknown>;

    constructor(
        segments: Iterable<SegmentSpec<SourceLike>>,
        { instrument = null, controls = null }: SegmentRunOptions = {},
    ) {
        super(segments);
        this.instrument = instrument ?? null;
        this.controls = { ...(controls ?? {}) };
    }

    override get unit(): TimeUnit {
        return SECONDS;
    }

    override like(segments: Segment<SourceLike>[]): BufferSegments {
        return new BufferSegments(segments, {
            instrument: this.instrument,
            controls: this.controls,
        });
    }

    /**
     * A frame, moved forward by `by` **seconds**: the sample rate is the bridge,
     * and the buffer is what knows it.
     */
    override advanced(start: number, by: number): number {
        const rate = Number(this.segments[0]?.source?.sampleRate ?? 0);
        return rate > 0 ? Number(start) + Number(by) * rate : Number(start);
    }

    /**
     * Whether these windows are **one run of one buffer**: each opening exactly
     * where the one before it stopped.
     *
     * What makes a join the inverse of a split rather than a pile of wrappers —
     * a run like this *is* the single window it was cut from, and says so, so
     * cutting and rejoining leaves the composition it started with. A run of one
     * is trivially one run.
     */
    get contiguous(): boolean {
        if (this.segments.length === 0) return false;
        const first = this.segments[0];
        let expected = first.start;
        for (const seg of this.segments) {
            if (seg.source !== first.source || Math.abs(seg.start - expected) >= 0.5) {
                return false;
            }
            expected = this.advanced(seg.start, seg.duration);
        }
        return true;
    }

    /**
     * What playing one window asks the instrument for: the buffer, and the frame
     * the window opens at.
     */
    eventParams(seg: Segment<SourceLike>): Record<string, unknown> {
        const params: Record<string, unknown> = { buf: seg.source.bufnum };
        if (seg.start) params.start = Number(seg.start);
        return params;
    }
}

/**
 * A run of windows onto a **timeline of events**: which timeline, from which
 * beat, for how many beats.
 *
 * The same structure {@link BufferSegments} is, over the contents whose lengths
 * are musical — so both units are beats and {@link advanced} has nothing to
 * bridge. A cut here hides notes rather than deleting them, which is what makes
 * dragging the edge back out bring them back, exactly as it does for samples.
 */
export class NoteSegments extends SegmentRun<TimelineLike> {
    instrument: string | null;
    controls: Record<string, unknown>;

    constructor(
        segments: Iterable<SegmentSpec<TimelineLike>>,
        { instrument = null, controls = null }: SegmentRunOptions = {},
    ) {
        super(segments);
        this.instrument = instrument ?? null;
        this.controls = { ...(controls ?? {}) };
    }

    override get unit(): TimeUnit {
        return BEATS;
    }

    override like(segments: Segment<TimelineLike>[]): NoteSegments {
        return new NoteSegments(segments, {
            instrument: this.instrument,
            controls: this.controls,
        });
    }

    /**
     * `[beat, item]` pairs of everything inside the windows, placed on the
     * **run's** own axis: each window's items shifted to where that window sits
     * in the run. What falls outside a window is not here and is not gone — it
     * is in the timeline, waiting for the window to open again.
     */
    items(): [number, unknown][] {
        const out: [number, unknown][] = [];
        for (const [offset, seg] of this.placed()) {
            for (const [beat, item] of seg.source.range(seg.start, seg.start + seg.duration)) {
                out.push([offset + (beat - seg.start), item]);
            }
        }
        return out;
    }
}
