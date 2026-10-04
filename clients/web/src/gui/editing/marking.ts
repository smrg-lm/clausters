/**
 * Marking: what the hand marked, for the editors whose operations act on it
 * (mirrors `clausters/gui/editing/marking.py`).
 *
 * {@link Marking} is a **capability, not a level of the hierarchy**. An editor
 * takes it when what a hand marks is what its operations act on -- a
 * multitrack's held regions, a roll's events moved, deleted, quantized and
 * copied together, an audio editor's range of samples -- and an editor with no
 * such operation does not: in the points editor a hand takes one segment, for
 * its shape, so it marks nothing and has none of these words.
 *
 * The three words are here, once: {@link Marking.selected},
 * {@link Marking.select} and {@link Marking.unselect}. What an editor says is
 * what its marks **are** -- which objects of its structure, and where they are
 * kept -- through the two hooks of {@link Marks}.
 *
 * **What is marked is the window's**, never the structure's: it enters no
 * history, and two windows over one structure each have their own. It is told
 * apart from the transport's `span` -- the time range a sweep leaves, which
 * says what sounds -- by what it is, not by which editor holds it.
 *
 * @module
 */

import type { GuiHost } from "../host.ts";

/**
 * What an editor that marks answers: what is marked now, and marking in its
 * place. `Marked` is what {@link Marking.selected} answers and `Mark` what
 * {@link Marking.select} takes.
 */
export interface Marks<Marked, Mark = Marked> {
    /** What is marked now, in the structure's own type. @internal */
    marked(): Marked | Promise<Marked>;
    /** Marks `marked` in place of what was marked; `null` marks nothing. @internal */
    mark(marked: Mark | null): void | Promise<void>;
}

/**
 * The marking surface of an editor: `selected`, `select`, `unselect`.
 * Composed into an editor, as the reference client's mixin is; never used
 * alone.
 *
 * All three are asynchronous, the reference client's property and two verbs:
 * where the host keeps the marks, reading them is a round trip.
 */
export class Marking<Marked, Mark = Marked> {
    /**
     * **What the hand marked**, in the structure's own type -- the editor's
     * class says which: a roll's events, a multitrack's regions, an audio
     * editor's samples.
     */
    async selected(this: Marks<Marked, Mark>): Promise<Marked> {
        return this.marked();
    }

    /** Marks `marked` -- what {@link Marking.selected} answers -- in place of what was marked. */
    async select(this: Marks<Marked, Mark>, marked: Mark): Promise<void> {
        await this.mark(marked);
    }

    /** Marks nothing. */
    async unselect(this: Marks<Marked, Mark>): Promise<void> {
        await this.mark(null);
    }
}

/**
 * Composes {@link Marking} into `editor`, exactly as the Python package's
 * mixin is: copying the prototype is what makes `editor.selected()` the
 * editor's own method.
 *
 * @internal
 */
export function marking(editor: { prototype: object }): void {
    for (const name of Object.getOwnPropertyNames(Marking.prototype)) {
        if (name === "constructor") continue;
        const descriptor = Object.getOwnPropertyDescriptor(Marking.prototype, name);
        if (descriptor) Object.defineProperty(editor.prototype, name, descriptor);
    }
}

/**
 * The names in `widget`'s `selected` prop, as the host has them now. Empty
 * with no window open (a `null` host or widget).
 *
 * @internal
 */
export async function marks<Name>(host: GuiHost | null, widget: number | null): Promise<Name[]> {
    if (host === null || widget === null) return [];
    const held = (await host.query(widget)).props.selected;
    return JSON.parse(typeof held === "string" ? held : "[]") as Name[];
}

/**
 * Sets `widget`'s `selected` prop to `names`. Nothing with no window open (a
 * `null` host or widget).
 *
 * @internal
 */
export function setMarks(
    host: GuiHost | null,
    widget: number | null,
    names: readonly (string | number)[],
): void {
    if (host === null || widget === null) return;
    host.set(widget, { selected: JSON.stringify(names) });
}
