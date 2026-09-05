// Rendering — the *change of state* from the arrangement to sound (mirrors
// `clausters/form/render.py`).
//
// A concrete `Aggregate` is rendered by **flattening** it: a tree-walk that
// accumulates the nested placement offsets into absolute beats, producing a flat
// `seq.Timeline` of items that each know how to `play(destination)`. That
// timeline is then played by a `seq.Playhead` — RT (timetagged bundles) or NRT
// (a score for an offline render) purely by which destination and clock it
// holds, sample-identical, with no scheduling path of its own. This mirrors
// `Timeline.fromPattern`: the arrangement reuses the sequencing layer rather
// than duplicating it.
//
// Scope of the concrete path:
//
// - `Aggregate{concrete}` — flattened recursively; each member's `offset` (and
//   any nested aggregate's) accumulates into the child's absolute beat.
// - `Track` — its `Timeline`'s items are shifted by the placement beat.
// - `Clang` — placed as a single item at its beat.
// - `Sequence`/`Generator` wrapping an **event pattern** (a `Pbind`) — *bounced*
//   in the same pass (its change of state); a `Sequence` of elements is laid out
//   successively by their durations.
// - An **abstract** element (no onset/duration, no content) contributes context,
//   not an event.
//
// **Mixing is part of the composition, and it is honoured here.** An element
// carries `mute`, `solo` and `level`, all three inherited down the tree: a muted
// branch contributes nothing, one soloed element anywhere silences every branch
// that is not on a soloed path, and a level multiplies into the `amp` of the
// events below it. They travel in the document, so a piece reopens mixed the way
// it was left — unlike a lane's *height*, which says nothing about what the
// piece is and is carried by no document.
//
// A `Vector` is *data*: it sounds through the **instrument** that plays it (a
// def whose `buf` control takes the buffer number), so a `Vector` with an
// `instrument` emits one event playing it — the audio clip — and one without
// contributes structure only. A `Segments` is the same rule over several
// windows: one event per segment, at its own offset inside the element, so what
// sounds assembled from pieces of different buffers sounds continuous on one
// instrument. An `Aggregate{logical}` takes the other path entirely (it becomes
// a `GraphDef`); instancing a bare def still needs an instrument of its own and
// raises a clear error here.

import { Group } from "../defs/node.ts";
import type { Controls } from "../defs/node.ts";
import type { Server } from "../defs/server/index.ts";
import { Event as SeqEvent } from "../seq/event.ts";
import { Pattern } from "../seq/pattern.ts";
import { Playhead, Timeline } from "../seq/timeline.ts";
import type { PlayDestination } from "../seq/timeline.ts";
import type { TempoClock } from "../base/clock.ts";
import { CONCRETE, LOGICAL, Aggregate } from "./aggregate.ts";
import {
    BEATS,
    Clang,
    Element,
    Generator,
    Segments,
    Sequence,
    Track,
    Vector,
    registerRendering,
    endBeat,
    tempoMapOf,
} from "./element.ts";
import type { TempoMap } from "../base/time.ts";

/** One flattened item: what plays, and the absolute beat it plays at. */
export type Flat = [beat: number, item: unknown];

/** What {@link render} takes past the destination and the clock. */
export interface RenderOptions {
    /** The beat the playhead starts at (a concrete element). */
    at?: number;
    /** The clock grid the start snaps to (a concrete element). */
    quant?: number;
    /** Surface ports overriding the graph's defaults (a logical aggregate). */
    ports?: Controls;
}

/**
 * What {@link render} gives back: the `Playhead` playing a concrete element, or
 * — for a logical `Aggregate`, whose def has to reach the server first — a
 * promise of the instance group. The seam is the destination, not the element,
 * and this is the one place the two paths show through the same name.
 */
export type RenderResult = Playhead | Promise<Group>;

/**
 * Flattens `element` into `[absoluteBeat, item]` pairs, sorted by beat,
 * accumulating nested placement offsets onto `base`. The items are playable
 * (they follow the `play(destination)` protocol).
 *
 * `tempo` (beats per second) is where the tree's two units meet. An onset is in
 * beats and a length is in the unit of its own data ({@link Element.durationUnit}:
 * a take's is seconds), and a timeline is ordered by **one** number — so the
 * conversion belongs to the flattening and never to the structure. At the
 * default tempo of one beat a second the two coincide, which is what a script
 * that never set a tempo has always been running under.
 *
 * `mixed` is whether the composition's mixing is in force — mute, solo and
 * level, all inherited down the tree. It is on for what sounds and off for what
 * is **drawn**: a muted lane keeps its clips, its notes and its length, and a
 * picture that emptied when the toggle was pressed would be reporting silence as
 * absence.
 */
export function flatten(
    element: Element,
    base = 0.0,
    tempo = 1.0,
    tempoMap?: TempoMap | null,
    mixed = true,
): Flat[] {
    const out: Flat[] = [];
    emit(element, Number(base), out, null, tempoMapOf(tempoMap, Number(tempo)),
        Mix.over(element, mixed));
    // A stable sort (the language guarantees one), which is what keeps a
    // note-off before the re-trigger placed at the same beat.
    out.sort((a, b) => a[0] - b[0]);
    return out;
}

/**
 * Flattens `element` into a flat `seq.Timeline` in absolute beats — the
 * structure a `Playhead` plays and a transport seeks. `tempo` is the clock's,
 * in beats per second (see {@link flatten}).
 */
export function toTimeline(
    element: Element,
    base = 0.0,
    tempo = 1.0,
    tempoMap?: TempoMap | null,
    mixed = true,
): Timeline {
    const timeline = new Timeline();
    for (const [beat, item] of flatten(element, base, tempo, tempoMap, mixed)) {
        timeline.add(beat, item);
    }
    return timeline;
}

/**
 * Renders `element` onto `destination`.
 *
 * A **concrete** element (an `Aggregate`, `Track`, `Clang`, …) is flattened to a
 * timeline and played through a `Playhead` over `clock` — RT or NRT,
 * sample-identical; returns the `Playhead`.
 *
 * A **logical** `Aggregate` is translated to a `GraphDef`, sent (`/def_send
 * graph`) and instanced (`/graph_new`, with `ports` overriding the surface
 * defaults) on the `Server` `destination`; returns a promise of the instance
 * group. The seam is the destination, not the element.
 */
export function render(
    element: Element,
    destination: unknown,
    clock?: unknown,
    { at = 0.0, quant, ports }: RenderOptions = {},
): RenderResult {
    if (element instanceof Aggregate && element.kind === LOGICAL) {
        return renderLogical(element, destination as Server, { ports });
    }

    if (!(element instanceof Aggregate) && element.wraps === null) {
        throw new Error(
            "an abstract element (no content) is pure context; it has nothing to render",
        );
    }
    const tempo = Number((clock as { tempo?: number } | undefined)?.tempo ?? 1.0) || 1.0;
    // The clock's own map, so what sounds and what an editor draws are measured
    // by one function rather than by two readings of it.
    const timeline = toTimeline(
        element,
        Number(element.onset ?? 0.0),
        tempo,
        (clock as { map?: TempoMap } | undefined)?.map,
    );
    const playhead = new Playhead(
        timeline,
        clock as TempoClock,
        destination as PlayDestination,
    );
    playhead.play({ at, quant });
    return playhead;
}

/**
 * Sends a logical aggregate's `GraphDef` ({@link Aggregate.toGraphdef}) and
 * instances it on `server`. Resolves to the instance group — a node-tree group,
 * the handle `Group.graph` gives back.
 *
 * Asynchronous where the Python client's is not, and for the reason every def
 * here is: sending a def is a round trip to the server, and this client awaits
 * one rather than blocking a page's single thread.
 */
export async function renderLogical(
    aggregate: Aggregate,
    server?: Server,
    { ports }: { ports?: Controls } = {},
): Promise<Group> {
    const gdef = aggregate.toGraphdef();
    await gdef.send(server);
    return Group.graph(gdef.name, ports, { server });
}

// ---- the flatten dispatch ----

/**
 * Flattens `element` at `base`, honouring the **placement length** its aggregate
 * gave it: a placement `dur` *trims* what the element plays (the DAW rule — a
 * clip's length is what you hear of it), so events past the placement's end are
 * dropped and a single-event element sounds for exactly that long. A placement
 * with no length lets the element be its own.
 *
 * The placement's length is in the placed element's own unit — a clip of audio
 * is trimmed in seconds — so it crosses to beats here, once, against the element
 * it was written for.
 */
/**
 * The mixing in force at one point of the walk: whether anything in the piece is
 * soloed, whether this branch is, and the gain accumulated down to it.
 *
 * It is threaded through the walk rather than read off each element because all
 * three are **inherited**: muting an aggregate silences its members, a lane's
 * level multiplies its clips', and one soloed lane anywhere silences every
 * branch that is not on a soloed path. A mute is the one that does not need
 * threading — it drops the branch where it is met.
 */
class Mix {
    readonly soloing: boolean;
    readonly soloed: boolean;
    readonly gain: number;
    /**
     * Whether the mix is in force at all. **Drawing reads the composition
     * unmixed**: a muted lane still has its clips, its notes and its length, and
     * a picture that vanished when the toggle was pressed would be reporting
     * silence as absence.
     */
    readonly honour: boolean;

    constructor(soloing: boolean, soloed: boolean, gain: number, honour: boolean) {
        this.soloing = soloing;
        this.soloed = soloed;
        this.gain = gain;
        this.honour = honour;
    }

    /**
     * The mix a whole piece starts under. Solo is piece-wide by definition — it
     * says *only these* — so whether anything is soloed is a question about the
     * tree and not about the element being walked.
     */
    static over(element: Element, mixed: boolean): Mix {
        return new Mix(mixed && anySolo(element), false, 1.0, mixed);
    }

    /** Whether this element's branch is dropped outright. */
    silences(element: Element): boolean {
        return this.honour && Boolean(element.mute);
    }

    /** The mix inside `element`. */
    under(element: Element): Mix {
        if (!this.honour) return this;
        const level = Number(element.level ?? 1.0);
        const soloed = this.soloed || Boolean(element.solo);
        if (soloed === this.soloed && level === 1.0) return this;
        return new Mix(this.soloing, soloed, this.gain * level, true);
    }

    /**
     * `item` as it sounds under this mix, or `null` when it does not.
     *
     * The gain is written onto the event's `amp` — a **copy**, since the
     * element's own event is shared and a mix must not rewrite it (the same rule
     * {@link sized} follows). Anything that is not an event carries no gain and
     * passes through: an automation curve is a control signal, and scaling one
     * is an edit of the curve rather than a mixer's business.
     */
    applied(item: unknown): unknown {
        if (!this.honour) return item;
        if (this.soloing && !this.soloed) return null;
        if (this.gain === 1.0) return item;
        if (!(item instanceof SeqEvent)) return item;
        const amp = item.get("amp");
        return new SeqEvent({
            ...item.props,
            amp: this.gain * Number(amp ?? 1.0),
        });
    }
}

/** Whether anything in this tree is soloed. */
function anySolo(element: Element): boolean {
    if (element.solo) return true;
    if (element instanceof Aggregate) {
        return element.handles.some((handle) => anySolo(handle.element));
    }
    if (element instanceof Generator && element.rendered !== null) {
        return anySolo(element.rendered);
    }
    if (element instanceof Sequence && Array.isArray(element.wraps)) {
        return element.wraps.some((item) => item instanceof Element && anySolo(item));
    }
    return false;
}

/** Lays one item down at `beat`, if this mix lets it be heard. */
function heard(out: Flat[], beat: number, item: unknown, mix: Mix): void {
    const played = mix.applied(item);
    if (played !== null) out.push([beat, played]);
}

function emit(
    element: Element,
    base: number,
    out: Flat[],
    dur: number | null = null,
    tempoMap: TempoMap,
    mix: Mix,
): void {
    if (mix.silences(element)) {
        // A muted branch contributes nothing — not its own events and not its
        // members'. It is the one part of the mix that needs no threading: it is
        // answered where it is met.
        return;
    }
    const inner = mix.under(element);
    let placed: Flat[] = [];
    emitElement(element, base, placed, tempoMap, inner);
    if (dur !== null) {
        // The placement's end, not its length turned into one: under a tempo
        // that changes, a length in seconds reaches a different beat depending
        // on where it starts, so the two positions are what say where it ends.
        const end = endBeat(base, dur, element.durationUnit, tempoMap);
        const length = end - base;
        placed = placed
            .filter(([beat]) => beat < end - 1e-9)
            .map(([beat, item]) => [beat, sized(item, Math.min(length, end - beat))]);
    }
    out.push(...placed);
}

/**
 * An event resized to the placement's remaining length — a *copy*, since the
 * element's own event is shared and must not be rewritten by a placement.
 * Anything that is not an event (an automation, a raw OSC item) is untouched.
 */
function sized(item: unknown, dur: number): unknown {
    if (item instanceof SeqEvent && item.get("dur") !== null && item.get("dur") !== undefined) {
        return new SeqEvent({ ...item.props, dur: Number(dur) });
    }
    return item;
}

function emitElement(
    element: Element,
    base: number,
    out: Flat[],
    tempoMap: TempoMap,
    mix: Mix,
): void {
    if (element instanceof Aggregate) {
        if (element.kind !== CONCRETE) {
            throw new Error(
                "a logical Aggregate is rendered as a GraphDef, not flattened",
            );
        }
        for (const member of element.handles) {
            emit(member.element, base + member.offset, out, member.dur, tempoMap, mix);
        }
    } else if (element instanceof Track) {
        // `items()` and not the timeline: a track is a **window** onto it (a
        // trim reads from further in, a split gives two windows over one
        // timeline), so what sounds is what the window shows, placed from the
        // element's own zero. Without a window that is the whole timeline,
        // which is what a track written by a script is.
        for (const [beat, item] of element.items()) heard(out, base + beat, item, mix);
    } else if (element instanceof Clang) {
        heard(out, base, element.wraps, mix);
    } else if (element instanceof Sequence || element instanceof Generator) {
        emitSequence(element.wraps, base, out, tempoMap, mix);
    } else if (element instanceof Segments) {
        // Several windows read as one thing: one event per segment, each at its
        // own offset inside the element and each carrying its own window, so
        // what sounds is continuous even though the source is not one buffer.
        // Without an instrument a run of *samples* is structure, exactly as a
        // `Vector` is — a run of windows onto timelines needs none, because what
        // it holds are events that carry their own.
        if (element.instrument !== null || element.durationUnit === BEATS) {
            for (const [offset, event] of element.toEvents(tempoMap, base)) {
                heard(out, base + offset, event, mix);
            }
        }
    } else if (element instanceof Vector) {
        // A buffer is data; the instrument is what makes it sound (a def whose
        // `buf` control plays it). Without one it is structure only — it draws
        // in the editor and contributes its extent, but emits no event.
        if (element.instrument !== null) {
            heard(out, base, element.toEvent(tempoMap, base), mix);
        }
    } else if (element instanceof Element) {
        // An abstract context element yields no event.
        if (element.wraps === null) return;
        if (typeof (element.wraps as { play?: unknown }).play === "function") {
            heard(out, base, element.wraps, mix);
        } else {
            throw new Error(
                "cannot render an element wrapping " +
                    (element.wraps as object).constructor.name,
            );
        }
    } else {
        throw new TypeError(`not an Element: ${String(element)}`);
    }
}

/**
 * The **place in time** a flattened item takes, in beats: an event's `dur`,
 * which is its slot and not its sounding length (`sustain`), so a detached note
 * still occupies the beat it was written on. Anything that is not an event is
 * punctual.
 */
function slot(item: unknown): number {
    if (!(item instanceof SeqEvent)) return 0.0;
    const dur = Number(item.get("dur") ?? 0.0);
    return Number.isFinite(dur) ? dur : 0.0;
}

/**
 * How far an element with **no stated duration** reaches, in beats: the end of
 * the last thing it lays down, from its own zero.
 *
 * Laid down **unmixed**, and that is the point rather than an economy: mute and
 * solo say what is heard, never where anything is. Measuring what a mix let
 * through would make soloing one lane re-time the sequence in another, which is
 * the one thing a reader would never look for.
 */
function reaches(element: Element, tempoMap: TempoMap): number {
    const laid: Flat[] = [];
    emitElement(element, 0.0, laid, tempoMap, Mix.over(element, false));
    return laid.reduce((end, [beat, item]) => Math.max(end, beat + slot(item)), 0.0);
}

/**
 * A List/Function backed by an event pattern is bounced; a list of elements is
 * laid out successively — each by its own duration, or by what it lays down
 * when it states none.
 */
function emitSequence(
    wrapped: unknown,
    base: number,
    out: Flat[],
    tempoMap: TempoMap,
    mix: Mix,
): void {
    if (wrapped === null || wrapped === undefined || typeof wrapped === "string") {
        // A **frozen** generator: the document named an algorithm and nothing in
        // this process supplied one, so what came back is the reference itself
        // (or nothing at all). It is structure — it draws, it contributes its
        // extent — and it emits no event, exactly as a buffer with no instrument
        // does. Throwing here instead would make a reopened session unplayable
        // because one lane in it was written by a script that is not running.
        return;
    }
    if (wrapped instanceof Pattern) {
        for (const [beat, item] of Timeline.fromPattern(wrapped)) {
            heard(out, base + beat, item, mix);
        }
    } else if (wrapped instanceof Timeline) {
        for (const [beat, item] of wrapped) heard(out, base + beat, item, mix);
    } else if (typeof (wrapped as { play?: unknown }).play === "function") {
        // Something that plays itself — an automation curve, and whatever else a
        // script hands over. The conversion writes every element it has no body
        // for as a *generator* leaf, so resolving one back on open gives a
        // `Generator` where the author wrote a bare `Element`; the two must play
        // the same thing or a reopened piece would sound different from the one
        // that was saved.
        heard(out, base, wrapped, mix);
    } else if (!Array.isArray(wrapped)) {
        // **A def is not a list of elements.** A generator wrapping a `SynthDef`
        // is a *resident* one — the server produces its audio, and there is
        // nothing here to lay out — so this says so rather than failing inside
        // an iteration the def never meant to offer.
        throw new Error(
            `cannot flatten a generator wrapping ${(wrapped as object).constructor.name}`,
        );
    } else {
        let cursor = base;
        for (const item of wrapped as Iterable<unknown>) {
            if (!(item instanceof Element)) {
                throw new Error(
                    "a Sequence of raw values is data (a parameter), not events",
                );
            }
            emit(item, cursor, out, null, tempoMap, mix);
            // Laid out successively on the beat axis, so each length crosses
            // from whatever unit its own data is in. An item that states no
            // length is as long as **what it lays down** — a `Sequence` of
            // `Sequence`s says nothing about its members' lengths, and reading a
            // missing one as zero stacked every one of them on the first beat.
            cursor = item.duration === null || item.duration === undefined
                ? cursor + reaches(item, tempoMap)
                : endBeat(cursor, item.duration, item.durationUnit, tempoMap);
        }
    }
}

// `element.render()` / `element.toTimeline()` are these two functions, reached
// through the registry rather than through an import back into `element.ts` —
// see `registerRendering` there for why the dependency stays one-way.
registerRendering({ toTimeline, render });
