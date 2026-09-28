/**
 * Editing an event sequence on a roll: the notes editor (mirrors
 * `clausters/gui/editing/events.py`).
 *
 * What it opens is an `EventSequence` -- events as concrete data, each with an
 * id -- and it edits that sequence **in place**: the editor in the shared crate
 * (the `openNotes` member of `EditingCore`) holds the very sequence the page's
 * handle names, so every edit is read back through the handle and there is
 * nothing to write back. `edit` opens a `Timeline` by rendering it first
 * (`Timeline.renderEvents`): the roll edits the events the timeline produced,
 * never the timeline.
 *
 * **The editor is the crate's**: the window, the notes with their ids, what
 * each gesture does to the sequence, the entry it leaves and the corrections it
 * answers with. What is here is what a language owns -- the socket, and handing
 * the crate the window it is open in.
 *
 * @module
 */

import { EVENTS } from "../../document.ts";
import type { TempoMap } from "../../base/time.ts";
import { EventSequence } from "../../seq/sequence.ts";
import { Timeline } from "../../seq/timeline.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import type { Answer } from "./echo.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { plain } from "./samples.ts";
import { View } from "./view.ts";

/** What one turn of the core came to. */
interface Outcome {
    turn?: string;
    changed?: boolean;
    answer?: Answer;
}

/**
 * A sequence's vocabulary, the crate's `events`. A step is applied by the crate
 * to the sequence it shares, so there is nothing here to carry out.
 */
export class NotesDomain extends Domain<EventSequence> {
    readonly name = EVENTS;
    override readonly ingested = true;

    /** The crate reads the inverse off the sequence it holds. */
    current(_structure: EventSequence, _payload: unknown): unknown {
        return null;
    }

    /** The crate applied the step to the sequence it shares with the page. */
    project(_structure: EventSequence, _payload: unknown): boolean {
        return false;
    }
}

/** The roll: one `notes` widget, composed by the crate. */
export class NotesView extends View<EventSequence> {
    build(editor: Editor<EventSequence>): GuiNode {
        const wid = this.widget(editor, "notes", editor.structure);
        const ed = editor as NotesEditor;
        ed.syncCore();
        const tree = ed.coreCall("window", { widget: wid }) as unknown as GuiNode;
        // **A page's own widgets are its objects**, so they are appended here
        // rather than composed in the crate.
        tree.children = [...(tree.children ?? []), ...editor.extra];
        return tree;
    }

    override props(editor: Editor<EventSequence>, widgetId: number): Record<string, PropValue> {
        return (editor as NotesEditor).coreCall("props", { widget: widgetId }) as Record<
            string,
            PropValue
        >;
    }
}

/** An event sequence on a roll, edited note by note, in place. */
export class NotesEditor extends Editor<EventSequence> {
    /** This editor's member in its editing context. */
    private readonly member: number;
    /** Whether a hand may edit the notes (`false` for a rendering). */
    readonly editable: boolean;

    constructor(sequence: EventSequence, options: NotesEditorOptions) {
        const domain = new NotesDomain();
        super(sequence, { title: "Notes", ...options, domain, view: new NotesView() });
        this.editable = options.editable ?? true;
        const opened = this.editing.openNotes(
            `sequence:${keyOfSequence(sequence)}`,
            sequence,
            {
                rate: this.sampleRate,
                editable: this.editable,
                title: this.title,
                w: this.size[0],
                h: this.size[1],
            },
            domain,
        );
        this.member = opened.member;
        this.structureId = opened.identity;
    }

    /** The sequence the roll edits -- the one the editor was opened over. */
    get sequence(): EventSequence {
        return this.structure;
    }

    /** The sequence's own tempo map, which the roll's axis is drawn through. */
    override tempoMap(): TempoMap | null {
        return this.structure.tempoMap;
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
     * Hand the crate the window it is open in and the chrome.
     *
     * @internal
     */
    syncCore(): void {
        this.coreCall("sync", {
            window: this.windowId,
            rate: this.sampleRate,
            editable: this.editable,
            title: this.title,
            w: this.size[0],
            h: this.size[1],
        });
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

    /** One `/gui_event` payload, with the stamp already taken off. */
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

    /** Answers the host with what a turn came to; whether the sequence changed. */
    private take(outcome: Outcome): boolean {
        if (outcome.turn === undefined || outcome.turn === "nothing") return false;
        const changed = outcome.changed === true;
        if (changed) {
            this.dirty = true;
            this.editing.changed();
        }
        this.echo.send(outcome.answer);
        return changed;
    }
}

/** {@link NotesEditor}'s options: the generic ones plus whether it writes. */
export interface NotesEditorOptions extends GenericEditorOptions<EventSequence> {
    /** `false` for a roll a hand may look at and not edit: the notes of a rendering. */
    editable?: boolean;
}

/** The number a sequence's structure key is made of: one per handle. */
const keys = new WeakMap<EventSequence, number>();
let nextKey = 0;
function keyOfSequence(sequence: EventSequence): number {
    let key = keys.get(sequence);
    if (key === undefined) {
        key = ++nextKey;
        keys.set(sequence, key);
    }
    return key;
}

/**
 * Whether `edit` opens this in the notes editor: an event sequence, or a
 * timeline, which it renders into one.
 */
export function isEvents(structure: unknown): structure is EventSequence | Timeline {
    return structure instanceof EventSequence || structure instanceof Timeline;
}
