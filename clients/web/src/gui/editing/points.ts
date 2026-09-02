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
export class PointsView extends View<Automation> {
    build(editor: Editor<Automation>): GuiNode {
        const wid = this.register(editor.newId(), editor.structure);
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            bpf({
                id: wid,
                points: editor.structure.toPoints(),
                label: nameOf(editor.structure),
            }),
        );
    }

    override props(editor: Editor<Automation>): Record<string, PropValue> {
        return { points: editor.structure.toPoints() as PropValue };
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
