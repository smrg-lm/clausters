/**
 * Editing a **break-point curve**: its vocabulary, its picture and its editor.
 *
 * The smallest of the three fundamental structures, and the one that shows the
 * shape of all of them: a {@link Domain} that turns the `bpf` view's `points`
 * payload into the crate's vocabulary and back, a {@link View} that is one `bpf`
 * widget, and an editor that is {@link Editor} with those two in it and nothing
 * else.
 *
 * **How an edit inverts is the crate's**, reached through {@link domainEdit}:
 * the payload goes in with the curve as it stands, and what comes back is the
 * curve as it now is *and* the payload that puts it back -- one call, because the
 * inverse has to be read before the edit lands. Nothing here computes an
 * inverse, which is the whole reason the domain seam exists.
 *
 * **What a shape is stays the client's.** The crate carries a point's `data` and
 * never reads it, so the segment shapes an `Env` needs travel in it -- without
 * that an undo put the curve back straight, which is losing the data rather than
 * declining to interpret it.
 *
 * @module
 */

import {
    curveAxis as coreCurveAxis,
    pointsProps as corePointsProps,
} from "../../core/clausters_core_web.js";
import { POINTS, domainEdit } from "../../document.ts";
import { cratePoints, flatPoints } from "../../multitrack.ts";
import { window as guiWindow } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { View } from "./view.ts";

/**
 * What a curve editor asks of a structure: break points it can read and write
 * back, and nothing about its type.
 *
 * An `Env`, a `Bpf` and a `multitrack.Automation` all answer it, and so would a
 * fourth thing that learned the pair -- which is the point, and why `isCurve`
 * asks for the methods rather than for a class.
 */
export interface EditableCurve {
    toPoints(): number[];
    setPoints(points: readonly number[]): unknown;
    name?: string;
}

/** One point as the crate holds it. */
export interface CratePoint {
    at: number;
    value: number;
    data?: { shape?: number; curve?: number };
}

/**
 * A curve's vocabulary: the crate's `points`, with the shape of each segment
 * carried in the point's own `data`.
 *
 * **What it asks of the structure is `toPoints` and `setPoints`**, and nothing
 * about its type -- an `Env`, a `Bpf` and a `multitrack.Automation` are all
 * curves here, and a fourth thing that learns the pair would be too.
 */
export class PointsDomain extends Domain<EditableCurve> {
    override readonly name = POINTS;
    override readonly ingested = true;

    /**
     * The curve as the crate holds it -- the state `current` is read against and
     * `project` writes back.
     *
     * **The curve seam, not a gesture.** It is here rather than in the crate
     * for the reason `project` is: what it crosses is the object *this page*
     * holds, and the vocabulary on the other side is already the crate's. Both
     * directions are {@link cratePoints} and {@link flatPoints}, written once
     * because a `multitrack.Automation` converts the same way.
     */
    state(structure: EditableCurve): CratePoint[] {
        return cratePoints(structure.toPoints()) as unknown as CratePoint[];
    }

    current(structure: EditableCurve, payload: unknown): unknown {
        return domainEdit(this.name, this.state(structure), payload)?.current ?? null;
    }

    project(structure: EditableCurve, payload: unknown): boolean {
        const edited = domainEdit(this.name, this.state(structure), payload);
        if (edited === undefined || !edited.applied) return false;
        structure.setPoints(flatPoints(edited.state as CratePoint[]));
        return true;
    }
}

/** One `bpf`: the curve on its own axis. */
/**
 * The axis a break-point curve is **drawn** against, as `[lo, hi]`: its values'
 * range with a tenth of headroom, and a flat curve still gets a band to be
 * dragged in.
 *
 * Pass the axis a view already has as `kept` and it is held, widened only where
 * the data stopped fitting inside it -- a range recomputed on every redraw makes
 * an edit rescale the picture, so dragging one point visibly moves every other
 * one.
 *
 * The rule is the shared core's, and both the standalone curve editor and the
 * clip body that draws the same curve ask it, in both clients: a drawing rule
 * with two implementations is how one curve comes to be drawn two ways.
 */
export function curveAxis(
    values: readonly number[],
    kept?: readonly [number, number],
): [number, number] {
    const out = coreCurveAxis(
        Float64Array.from(values),
        kept?.[0],
        kept?.[1],
    );
    return [Number(out[0]), Number(out[1])];
}

export class PointsView extends View<EditableCurve> {
    /**
     * The value axis this view is drawing against, and the time it spans, kept
     * per structure so a redraw does not re-fit them. Both only ever **grow** --
     * see {@link axis}.
     */
    private kept = new Map<unknown, [number, number]>();
    private span = new Map<unknown, number>();

    /**
     * The props this curve is drawn with, and the axis they settled on
     * remembered for the next time.
     *
     * **The projection is the crate's** (`pointsProps`): the points, the value
     * axis they stand on and the time they span, all in one answer, so a page
     * and a script set the same widget with the same props. What is kept here
     * is only what a *view* keeps -- the axis and the span in hand -- because
     * both only ever grow, and a curve that refits while a point is being
     * dragged moves every other point on screen.
     */
    drawn(structure: EditableCurve, points: readonly number[]): Record<string, PropValue> {
        const kept = this.kept.get(structure);
        const props = JSON.parse(
            corePointsProps(
                Float64Array.from(points, Number),
                kept?.[0],
                kept?.[1],
                this.span.get(structure) ?? 0.0,
            ),
        ) as Record<string, PropValue>;
        this.kept.set(structure, [Number(props.min), Number(props.max)]);
        this.span.set(structure, Number(props.duration ?? 0.0));
        return props;
    }

    build(editor: Editor<EditableCurve>): GuiNode {
        const drawn = this.drawn(editor.structure, editor.structure.toPoints());
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            this.catalogue(editor, "bpf", "curve", editor.structure, {
                ...drawn,
                duration: Number(drawn.duration ?? 0.0),
                label: nameOf(editor.structure),
            }),
            ...editor.extra,
        );
    }

    override props(editor: Editor<EditableCurve>): Record<string, PropValue> {
        return this.drawn(editor.structure, editor.structure.toPoints());
    }
}

/**
 * A curve on screen, editable back into the curve the caller already holds.
 *
 * Nothing is handed back at the end: the object the page passed in *is* the
 * edited one, and reading its `toPoints` after an edit is how a caller sees
 * what was drawn.
 */
export class PointsEditor extends Editor<EditableCurve> {
    constructor(curve: EditableCurve, options: GenericEditorOptions<EditableCurve>) {
        super(curve, {
            title: "Curve",
            ...options,
            domain: new PointsDomain(),
            view: new PointsView(),
        });
    }
}

function nameOf(curve: EditableCurve): string {
    const name = (curve as { name?: string }).name;
    return typeof name === "string" && name ? name : "curve";
}

/**
 * Whether `edit` should open this as a curve.
 *
 * **Asked of the structure, not of a type list.** What a curve editor needs is
 * an addressable list of break points it can read and write back, which is the
 * `toPoints`/`setPoints` pair -- so an `Env`, a `Bpf` and a
 * `multitrack.Automation` all open, and none of them is named here.
 */
export function isCurve(structure: unknown): structure is EditableCurve {
    const curve = structure as Partial<EditableCurve> | null;
    return (
        typeof curve === "object" && curve !== null &&
        typeof curve.toPoints === "function" && typeof curve.setPoints === "function"
    );
}
