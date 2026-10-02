/**
 * Editing a **break-point curve**: the points editor (mirrors
 * `clausters/gui/editing/points.py`).
 *
 * It is the automation editor seen on its own: what it edits is a curve over
 * one parameter -- a `multitrack.Automation`, the curve a track, a region, a
 * sequence and a note hold -- with nothing around it, which is also how an
 * envelope is made: an `Env` and a `Bpf` are curves nobody holds yet.
 *
 * **The editor is the crate's** (the `openPoints` member of `EditingCore`): the
 * window, the value axis and the time span it keeps while open, what a gesture
 * does to the curve, the entry it leaves and the corrections it answers with,
 * the time range a sweep leaves and the points inside it. The crate edits a
 * curve of its own, the document's; what is here is what a language owns --
 * the socket, handing the crate the curve as the page holds it, and writing
 * back onto that object the points each edit and each step leave.
 *
 * **What a shape is stays the client's.** The crate carries a point's `data` and
 * never reads it, so the segment shapes an `Env` needs travel in it -- without
 * that an undo put the curve back straight, which is losing the data rather than
 * declining to interpret it.
 *
 * @module
 */

import { POINTS } from "../../document.ts";
import { keyOf } from "../../history.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import type { Answer } from "./echo.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { plain } from "./samples.ts";
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
    name?: string | null;
}

/** What one turn of the core came to. */
interface Outcome {
    turn?: string;
    changed?: boolean;
    answer?: Answer;
    points?: number[];
    locate?: number;
    span?: [number, number] | null;
}

/**
 * A curve's vocabulary, the crate's `points`. The crate applies an edit and a
 * step to the curve it holds; what is here is writing the points they leave
 * back onto the curve the page holds.
 */
export class PointsDomain extends Domain<EditableCurve> {
    override readonly name = POINTS;
    override readonly ingested = true;

    /** The crate reads the inverse off the curve it holds. */
    current(_structure: EditableCurve, _payload: unknown): unknown {
        return null;
    }

    /** The crate applied the edit to the curve it holds. */
    project(_structure: EditableCurve, _payload: unknown): boolean {
        return false;
    }

    /**
     * Write the crate's points -- flat `t v shape curve` quads -- onto the
     * curve, its segments' shapes as the integers they are.
     */
    write(structure: EditableCurve, points: readonly number[]): void {
        structure.setPoints(points.map((x, i) => (i % 4 === 2 ? Math.trunc(Number(x)) : Number(x))));
    }
}

/** One `curve` widget and a row of controls under it, composed by the crate. */
export class PointsView extends View<EditableCurve> {
    build(editor: Editor<EditableCurve>): GuiNode {
        const ed = editor as PointsEditor;
        const wid = this.widget(editor, "curve", editor.structure);
        const shape = this.widget(editor, "shape", editor.structure);
        ed.curveId = wid;
        ed.syncCore();
        const tree = ed.coreCall("window", { widget: wid, shape }) as unknown as GuiNode;
        // **A page's own widgets are its objects**, so they are appended here
        // rather than composed in the crate -- into the row under the curve,
        // beside the segment's shape menu.
        const row = ((tree.children as GuiNode[])[1]!.children as GuiNode[])[0]!;
        row.children = [...(row.children ?? []), ...editor.extra];
        return tree;
    }

    override props(editor: Editor<EditableCurve>, widgetId: number): Record<string, PropValue> {
        return (editor as PointsEditor).coreCall("props", { widget: widgetId }) as Record<
            string,
            PropValue
        >;
    }
}

/** What {@link PointsEditor} takes on top of the generic editor's options. */
export interface PointsEditorOptions extends GenericEditorOptions<EditableCurve> {
    /**
     * With {@link PointsEditorOptions.max}, the **range the curve's values are
     * kept in** -- a rule, not a picture: the value axis is the range and
     * holds, and no point is left outside it. Without them a curve that
     * automates a parameter (a `multitrack.Automation` with a `target`) is
     * kept in that parameter's range, the one the roll and the multitrack
     * draw it over; and a curve that says nothing (an `Env`, a `Bpf`) is drawn
     * on an axis derived from its points, which grows to hold one dragged past
     * it.
     */
    min?: number;
    /** The top of that range. */
    max?: number;
    /**
     * With {@link PointsEditorOptions.end}, the range the curve's times are
     * kept in -- a normalized envelope is `start: 0, end: 1` beside `min: 0,
     * max: 1`.
     */
    start?: number;
    /** The end of that range. */
    end?: number;
}

/** The ranges a curve is edited inside, each `[low, high]` or `null`. */
export interface PointsRules {
    values: [number, number] | null;
    time: [number, number] | null;
}

/**
 * A curve on screen, editable back into the curve the caller already holds.
 *
 * Nothing is handed back at the end: the object the page passed in *is* the
 * edited one, and reading its `toPoints` after an edit is how a caller sees
 * what was drawn.
 *
 * The window shows the rules: the time ruler under the curve, in its own
 * seconds, the value ruler beside it, and a readout of what the pointer is
 * over -- a point's value against the range, and the shape of its segment. A
 * click on a segment selects it, and the menu in the row under the curve sets
 * its shape; `extra` widgets go in that row, beside it.
 */
export class PointsEditor extends Editor<EditableCurve> {
    /** This editor's member in its editing context. */
    private readonly member: number;
    /** The curve's widget id, once drawn. @internal */
    curveId: number | null = null;

    constructor(curve: EditableCurve, options: PointsEditorOptions) {
        const { min, max, start, end, ...rest } = options;
        if ((min === undefined) !== (max === undefined)) {
            throw new TypeError(
                "a declared range needs both ends: pass min and max, or neither",
            );
        }
        if ((start === undefined) !== (end === undefined)) {
            throw new TypeError(
                "a declared range needs both ends: pass start and end, or neither",
            );
        }
        const domain = new PointsDomain();
        super(curve, { title: "Curve", ...rest, domain, view: new PointsView() });
        const request: Record<string, unknown> = {
            rate: this.sampleRate,
            title: this.title,
            w: this.size[0],
            h: this.size[1],
            points: curve.toPoints().map(Number),
            name: nameOf(curve),
            target: targetOf(curve),
        };
        if (min !== undefined) Object.assign(request, { min, max });
        if (start !== undefined) Object.assign(request, { start, end });
        const opened = this.editing.open(
            "openPoints",
            keyOf("object", curve),
            request,
            curve,
            domain,
        );
        this.member = opened.member;
        this.structureId = opened.identity;
    }

    /**
     * One verb of this editor's member, through the context.
     *
     * @internal
     */
    coreCall(verb: string, args: Record<string, unknown> = {}): Record<string, unknown> {
        return this.editing.member(this.member, verb, args);
    }

    /**
     * Hand the crate the window it is open in, the chrome, and the curve as
     * the page holds it now.
     *
     * @internal
     */
    syncCore(): void {
        this.coreCall("sync", {
            window: this.windowId,
            rate: this.sampleRate,
            title: this.title,
            w: this.size[0],
            h: this.size[1],
            points: this.structure.toPoints().map(Number),
            name: nameOf(this.structure),
            target: targetOf(this.structure),
        });
    }

    /**
     * **The ranges the curve is edited inside**, each `[low, high]` or `null`
     * where there is none -- the declared ones, and where no value range was
     * declared, the range of the parameter the curve automates.
     */
    get rules(): PointsRules {
        this.syncCore();
        const rules = this.coreCall("rules");
        const pair = (r: unknown): [number, number] | null =>
            Array.isArray(r) ? [Number(r[0]), Number(r[1])] : null;
        return { values: pair(rules.values), time: pair(rules.time) };
    }

    // ---- the time range, and the points in it ----

    /**
     * **The time range** -- `[start, end]` in the curve's seconds, or `null` --
     * whose points {@link PointsEditor.selected} reads: what a sweep over the
     * curve leaves, with no value band when it is set here. A curve has no
     * transport, so the range is the editor's own: screen state, never part of
     * what is edited.
     */
    get span(): [number, number] | null {
        const span = this.coreCall("span").span as [number, number] | null | undefined;
        return span === null || span === undefined ? null : [Number(span[0]), Number(span[1])];
    }

    set span(span: readonly [number, number] | null) {
        this.coreCall("span", { span: span === null ? null : [span[0], span[1]] });
        this.adopt();
    }

    /**
     * **The break points the sweep covers** -- inside its time range and, for
     * a sweep with height, inside its value band -- as the `[t, v, shape,
     * curve]` quads `toPoints` speaks, in order. Empty with no range.
     */
    get selected(): [number, number, number, number][] {
        this.syncCore();
        const indices = (this.coreCall("selected").points ?? []) as number[];
        const flat = this.structure.toPoints();
        return indices.map((i) => flat.slice(4 * i, 4 * i + 4) as [number, number, number, number]);
    }

    // ---- the crate's turns ----

    protected override deliver(addr: string, rawArgs: readonly unknown[]): boolean {
        this.syncCore();
        const turned = this.editing.event(this.member, addr, plain([...rawArgs]) as unknown[]);
        const outcome = (turned.outcome ?? {}) as Outcome;
        if (outcome.turn === "closed") return this.closedWindow();
        if (outcome.turn === "step") {
            const stepped = this.app.stepped(this.editing, turned.stepped ?? {}, this);
            this.echo.send(outcome.answer);
            return stepped;
        }
        return this.take(outcome);
    }

    protected override route(args: readonly unknown[]): boolean {
        this.syncCore();
        const [wid, tag, ...values] = args;
        const turned = this.editing.event(
            this.member,
            "/gui_event",
            plain([wid, 0, 0, tag, ...values]) as unknown[],
        );
        return this.take((turned.outcome ?? {}) as Outcome);
    }

    private take(outcome: Outcome): boolean {
        if (outcome.turn === undefined || outcome.turn === "nothing") return false;
        const points = outcome.points;
        if (points !== undefined) {
            // The crate's curve moved: the page's follows it, as part of the
            // entry the turn recorded rather than as a change of its own.
            this.editing.applying(() => (this.domain as PointsDomain).write(this.structure, points));
        }
        const changed = outcome.changed === true;
        if (changed) {
            this.dirty = true;
            this.editing.changed();
        }
        if (outcome.locate !== undefined) {
            this.cursor = outcome.locate;
            this.locate(this.cursor);
            this.composedIn?.locate(this.cursor);
            this.onLocate?.(this.cursor);
        }
        this.echo.send(outcome.answer);
        return changed;
    }
}

/** What the curve automates, for a curve that says (a `multitrack.Automation`), else `null`. */
function targetOf(curve: EditableCurve): unknown {
    const target = (curve as { target?: unknown }).target;
    return target !== null && typeof target === "object" && !Array.isArray(target) ? target : null;
}

function nameOf(curve: EditableCurve): string {
    const name = (curve as { name?: string | null }).name;
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
