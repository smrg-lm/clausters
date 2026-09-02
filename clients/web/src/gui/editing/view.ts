/**
 * The picture of one structure, and the registry from widget id to what it
 * shows.
 *
 * A view is the **only per-domain thing on the graphic side**: it builds the
 * `GuiDef` for one structure and remembers which widget draws what, so an event
 * naming a widget resolves to something an editor can act on. Everything else
 * about drawing — the window, the ids, the acknowledgement — is the editor's and
 * is the same for every structure.
 *
 * It is separate from {@link Domain} because one structure is drawn several ways
 * while its vocabulary is one: a curve is a `bpf` on its own axis and a body
 * inside a clip, and both send the same `points` payload.
 *
 * This is **not** `gui/guidef.ts`'s `View`, which is a tree you can open; only
 * this one is reached as `gui/editing`'s.
 *
 * @module
 */

import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import type { Editor } from "./editor.ts";

/**
 * One structure on screen.
 *
 * Subclass it per picture: `build` is the tree, and the registry is kept here
 * rather than in the editor because it is rebuilt with the tree, and the two
 * going out of step is how a gesture reaches the wrong object.
 */
export abstract class View<S = unknown> {
    /** widget id → what that widget draws. Rebuilt by every `draw`. */
    widgets = new Map<number, unknown>();

    /**
     * The `GuiDef` this view is, with the registry rebuilt.
     *
     * Takes the editor because ids come from its pool and the unit bridge is its
     * own: a view decides what the picture *is*, never what a number in it is
     * measured in.
     */
    draw(editor: Editor<S>): GuiNode {
        this.widgets = new Map();
        return this.build(editor);
    }

    /** The tree itself. Register each widget as it is made ({@link register}). */
    abstract build(editor: Editor<S>): GuiNode;

    /**
     * Remembers that `widgetId` draws `showing`, and hands the id back so a
     * builder can use it inline.
     */
    register(widgetId: number, showing: unknown): number {
        const id = Math.trunc(widgetId);
        this.widgets.set(id, showing);
        return id;
    }

    /**
     * Whether this view drew the widget an event names.
     *
     * Asked before anything else, because a poll loop may be shared: answering
     * for another view's window retires a pending edit nobody applied, and the
     * host adopts a picture its real owner never saw.
     */
    owns(widgetId: number): boolean {
        return this.widgets.has(Math.trunc(widgetId));
    }

    /** What that widget draws, or `undefined`. */
    showing(widgetId: number): unknown {
        return this.widgets.get(Math.trunc(widgetId));
    }

    /**
     * **Everything the widget should be drawing**, for a resync.
     *
     * Not only what a gesture touched: a stale edit is the one case where the
     * host's whole picture of a widget is in doubt, so what goes back is the
     * widget's whole state. An empty answer means there is nothing to correct.
     */
    props(_editor: Editor<S>, _widgetId: number): Record<string, PropValue> {
        return {};
    }
}
