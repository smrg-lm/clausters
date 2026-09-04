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
 * curve as it now is *and* the payload that puts it back — one call, because the
 * inverse has to be read before the edit lands. Nothing here computes an
 * inverse, which is the whole reason the domain seam exists.
 *
 * **What a shape is stays the client's.** The crate carries a point's `data` and
 * never reads it, so the segment shapes an `Env` needs travel in it — without
 * that an undo put the curve back straight, which is losing the data rather than
 * declining to interpret it.
 *
 * @module
 */

import { curveAxis as coreCurveAxis } from "../../core/clausters_core_web.js";
import { POINTS, domainEdit } from "../../document.ts";
import { pointsToEnv } from "../../defs/ugens/env.ts";
import { Automation } from "../../seq/automation.ts";
import { bpf, window as guiWindow } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { View } from "./view.ts";

/** What the `bpf` widget sends and takes: flat `t v shape curve` quads. */
export const QUAD = 4;

/**
 * A flat `points` payload as `[t, value, shape, curve]` tuples, dropping a
 * trailing partial quad rather than guessing at it.
 */
export function quads(flat: readonly unknown[]): [number, number, number, number][] {
    const values = flat.map(Number);
    const out: [number, number, number, number][] = [];
    for (let i = 0; i + QUAD <= values.length; i += QUAD) {
        out.push([values[i] as number, values[i + 1] as number,
            Math.trunc(values[i + 2] as number), values[i + 3] as number]);
    }
    return out;
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
 */
export class PointsDomain extends Domain<Automation> {
    override readonly name = POINTS;

    payload(_structure: Automation, tag: string, values: readonly unknown[]): unknown {
        if (tag !== "points" || values.length === 0) return null;
        return {
            intent: "setpoints",
            points: quads(values).map(([at, value, shape, curve]) => ({
                at,
                value,
                data: { shape, curve },
            })),
        };
    }

    /**
     * The curve as the crate holds it — the state `current` is read against and
     * `project` writes back.
     */
    state(structure: Automation): CratePoint[] {
        return quads(structure.toPoints()).map(([at, value, shape, curve]) => ({
            at,
            value,
            data: { shape, curve },
        }));
    }

    current(structure: Automation, payload: unknown): unknown {
        return domainEdit(this.name, this.state(structure), payload)?.current ?? null;
    }

    project(structure: Automation, payload: unknown): boolean {
        const edited = domainEdit(this.name, this.state(structure), payload);
        if (edited === undefined || !edited.applied) return false;
        structure.env = pointsToEnv(flatPoints(edited.state as CratePoint[]));
        // One door: the envelope the page holds and the control buffer the lane
        // synth reads cannot disagree about which of the two happened.
        void structure.refill();
        return true;
    }

    override label(): string {
        return "draw the curve";
    }
}

/**
 * The crate's points back as the flat quads the view and the `Env` both speak. A
 * point that says nothing about its segment is linear, which is what a curve
 * drawn somewhere that has no shapes means.
 */
export function flatPoints(points: readonly CratePoint[]): number[] {
    const out: number[] = [];
    for (const point of points) {
        const data = point.data ?? {};
        out.push(Number(point.at ?? 0), Number(point.value ?? 0),
            Math.trunc(Number(data.shape ?? 1)), Number(data.curve ?? 0));
    }
    return out;
}

/** One `bpf`: the curve on its own axis. */
/**
 * The axis a break-point curve is **drawn** against, as `[lo, hi]`: its values'
 * range with a tenth of headroom, and a flat curve still gets a band to be
 * dragged in.
 *
 * Pass the axis a view already has as `kept` and it is held, widened only where
 * the data stopped fitting inside it — a range recomputed on every redraw makes
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

export class PointsView extends View<Automation> {
    /**
     * The value axis this view is drawing against, and the time it spans, kept
     * per structure so a redraw does not re-fit them. Both only ever **grow** —
     * see {@link axis}.
     */
    private kept = new Map<unknown, [number, number]>();
    private span = new Map<unknown, number>();

    /**
     * The value axis and the duration this curve is drawn against.
     *
     * The value axis is the shared core's (`curveAxis` below): the points' range
     * with headroom the first time, and after that the axis already in hand,
     * widened only where the data stopped fitting inside it. The time axis is
     * the same rule with nothing to pad — the last point's time, never shorter
     * than it has been — because a curve that refits while a point is being
     * dragged moves every *other* point on screen.
     */
    axis(structure: Automation, points: readonly number[]): [number, number, number] {
        const values: number[] = [];
        const times: number[] = [0.0];
        for (let i = 0; i + 3 < points.length; i += 4) {
            times.push(Number(points[i]));
            values.push(Number(points[i + 1]));
        }
        const [lo, hi] = curveAxis(values, this.kept.get(structure));
        const span = Math.max(...times, this.span.get(structure) ?? 0.0);
        this.kept.set(structure, [lo, hi]);
        this.span.set(structure, span);
        return [lo, hi, span];
    }

    build(editor: Editor<Automation>): GuiNode {
        const wid = this.register(editor.newId(), editor.structure);
        const points = editor.structure.toPoints();
        const [min, max, duration] = this.axis(editor.structure, points);
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            bpf({
                id: wid,
                points,
                min,
                max,
                ...(duration > 0 ? { duration } : {}),
                label: nameOf(editor.structure),
            }),
            ...editor.extra,
        );
    }

    override props(editor: Editor<Automation>): Record<string, PropValue> {
        const points = editor.structure.toPoints();
        const [min, max, duration] = this.axis(editor.structure, points);
        const props: Record<string, PropValue> = {
            points: points as PropValue,
            min,
            max,
        };
        if (duration > 0) props.duration = duration;
        return props;
    }
}

/**
 * A curve on screen, editable back into the `Automation` the caller already
 * holds.
 *
 * Nothing is handed back at the end: the object the page passed in *is* the
 * edited one, and reading `Automation.toPoints` after an edit is how a caller
 * sees what was drawn.
 */
export class PointsEditor extends Editor<Automation> {
    constructor(curve: Automation, options: GenericEditorOptions<Automation>) {
        super(curve, {
            title: "Curve",
            ...options,
            domain: new PointsDomain(),
            view: new PointsView(),
        });
    }
}

function nameOf(curve: Automation): string {
    const name = (curve as { name?: string }).name;
    return typeof name === "string" && name ? name : "curve";
}

/** Whether `edit` should open this as a curve. */
export function isCurve(structure: unknown): structure is Automation {
    return structure instanceof Automation;
}
