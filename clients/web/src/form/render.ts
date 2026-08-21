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
    Clang,
    Element,
    Generator,
    Segments,
    Sequence,
    Track,
    Vector,
    registerRendering,
} from "./element.ts";

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
 */
export function flatten(element: Element, base = 0.0): Flat[] {
    const out: Flat[] = [];
    emit(element, Number(base), out);
    // A stable sort (the language guarantees one), which is what keeps a
    // note-off before the re-trigger placed at the same beat.
    out.sort((a, b) => a[0] - b[0]);
    return out;
}

/**
 * Flattens `element` into a flat `seq.Timeline` in absolute beats — the
 * structure a `Playhead` plays and a transport seeks.
 */
export function toTimeline(element: Element, base = 0.0): Timeline {
    const timeline = new Timeline();
    for (const [beat, item] of flatten(element, base)) timeline.add(beat, item);
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
    const timeline = toTimeline(element, Number(element.onset ?? 0.0));
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
 */
function emit(element: Element, base: number, out: Flat[], dur: number | null = null): void {
    let placed: Flat[] = [];
    emitElement(element, base, placed);
    if (dur !== null) {
        const end = base + Number(dur);
        placed = placed
            .filter(([beat]) => beat < end - 1e-9)
            .map(([beat, item]) => [beat, sized(item, Math.min(dur, end - beat))]);
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

function emitElement(element: Element, base: number, out: Flat[]): void {
    if (element instanceof Aggregate) {
        if (element.kind !== CONCRETE) {
            throw new Error(
                "a logical Aggregate is rendered as a GraphDef, not flattened",
            );
        }
        for (const member of element.handles) {
            emit(member.element, base + member.offset, out, member.dur);
        }
    } else if (element instanceof Track) {
        for (const [beat, item] of element.timeline) out.push([base + beat, item]);
    } else if (element instanceof Clang) {
        out.push([base, element.wraps]);
    } else if (element instanceof Sequence || element instanceof Generator) {
        emitSequence(element.wraps, base, out);
    } else if (element instanceof Segments) {
        // Several windows read as one thing: one event per segment, each at its
        // own offset inside the element and each carrying its own window, so
        // what sounds is continuous even though the source is not one buffer.
        // Without an instrument it is structure, exactly as a `Vector` is.
        if (element.instrument !== null) {
            for (const [offset, event] of element.toEvents()) {
                out.push([base + offset, event]);
            }
        }
    } else if (element instanceof Vector) {
        // A buffer is data; the instrument is what makes it sound (a def whose
        // `buf` control plays it). Without one it is structure only — it draws
        // in the editor and contributes its extent, but emits no event.
        if (element.instrument !== null) out.push([base, element.toEvent()]);
    } else if (element instanceof Element) {
        // An abstract context element yields no event.
        if (element.wraps === null) return;
        if (typeof (element.wraps as { play?: unknown }).play === "function") {
            out.push([base, element.wraps]);
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
 * A List/Function backed by an event pattern is bounced; a list of elements is
 * laid out successively by their durations.
 */
function emitSequence(wrapped: unknown, base: number, out: Flat[]): void {
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
            out.push([base + beat, item]);
        }
    } else if (wrapped instanceof Timeline) {
        for (const [beat, item] of wrapped) out.push([base + beat, item]);
    } else if (typeof (wrapped as { play?: unknown }).play === "function") {
        // Something that plays itself — an automation curve, and whatever else a
        // script hands over. The conversion writes every element it has no body
        // for as a *generator* leaf, so resolving one back on open gives a
        // `Generator` where the author wrote a bare `Element`; the two must play
        // the same thing or a reopened piece would sound different from the one
        // that was saved.
        out.push([base, wrapped]);
    } else {
        let cursor = base;
        for (const item of wrapped as Iterable<unknown>) {
            if (!(item instanceof Element)) {
                throw new Error(
                    "a Sequence of raw values is data (a parameter), not events",
                );
            }
            emit(item, cursor, out);
            cursor += item.duration ?? 0.0;
        }
    }
}

// `element.render()` / `element.toTimeline()` are these two functions, reached
// through the registry rather than through an import back into `element.ts` —
// see `registerRendering` there for why the dependency stays one-way.
registerRendering({ toTimeline, render });
