/**
 * Editing an event sequence on a roll: the notes editor (mirrors
 * `clausters/gui/editing/events.py`).
 *
 * What it opens is an `EventSequence` -- events as concrete data, each with an
 * id. `edit` opens a `Timeline` by rendering it first (`Timeline.renderEvents`):
 * the roll edits the events the timeline produced, never the timeline.
 *
 * **The roll is an application of the GUI host's**, as the audio editor and the
 * multitrack are: the window, what each gesture does to the sequence, the entry
 * it leaves, and the playback all run in the host's process, and this is its
 * handler ({@link HostedEditor}). The page's sequence is the window's while one
 * is open, and when the last window over it goes it holds what that window left
 * it as.
 *
 * **It sounds in the application**: the sequence is the data of an event lane on
 * a transport of the window's own, so playing it moves no multitrack and no other
 * window, and a pause, a stop and a locate are the transport's. An edit sends the
 * lane its new data, so a note moved ahead of the line is heard where it lands,
 * and what is sounding keeps its release. The space bar over the window plays and
 * pauses. A note names the def it plays with: the application's engine knows
 * `default`, and a def sent to a client's server is not there.
 *
 * @module
 */

import type { TempoMap } from "../../base/time.ts";
import { EventSequence } from "../../seq/sequence.ts";
import type { SeqEvent } from "../../seq/sequence.ts";
import { MidiItem } from "../../seq/event.ts";
import { Timeline } from "../../seq/timeline.ts";
import type { PlayDestination } from "../../seq/timeline.ts";
import { Held, HostedEditor, HostedTransport } from "./hosted.ts";
import type { HostedEditorOptions } from "./hosted.ts";
import { Marking, marking } from "./marking.ts";
import type { End, Pass } from "./playback.ts";

/** Who has each sequence. */
const heldSequences = new WeakMap<EventSequence, Held>();

/** Who has `sequence`: made once, per sequence. @internal */
export function heldOf(sequence: EventSequence): Held {
    let held = heldSequences.get(sequence);
    if (held === undefined) {
        held = new Held();
        heldSequences.set(sequence, held);
    }
    return held;
}

/**
 * **A window that shows a sequence**, a roll or a page over one: the sequence the
 * window's from when the first opens, and the changes the page makes through its
 * objects entries of the history the window walks.
 *
 * @internal
 */
export abstract class SequenceHandler extends HostedEditor {
    /** The sequence the window shows, or `null`. */
    protected abstract readonly shown: EventSequence | null;

    protected override heldBy(): Held {
        return heldOf(this.shown as EventSequence);
    }

    protected override taken(): void {
        (this.shown as EventSequence).window = this;
    }

    /** A window over the sequence went: another answers for it, or, the last gone, it holds what that window left it as. */
    protected letGo(left: Record<string, unknown> | null): void {
        const sequence = this.shown as EventSequence;
        sequence.window = heldOf(sequence).live();
        if (sequence.window !== null) return;
        const written = left?.sequence;
        if (written !== null && typeof written === "object") {
            sequence.holdWritten(written as Record<string, unknown>);
        }
    }

    /** Brings the sequence the page holds to the window's. */
    protected async followSequence(): Promise<void> {
        const sequence = this.shown;
        if (sequence === null) return;
        const state = await this.asked("handle", { call: { verb: "state" } });
        if (state !== null && typeof state === "object") sequence.holdWritten(state as Record<string, unknown>);
    }
}

export interface NotesEditor extends Marking<SeqEvent[], Iterable<SeqEvent>> {}

/**
 * **An event sequence on a roll, in the GUI host's application, and this page its
 * handler.**
 *
 * **What is marked** (`selected`, `select`, `unselect`) is the events on the
 * roll -- by a click, Shift+click or a marquee -- as the `SeqEvent` objects
 * they are, in beat order. The window's, not the sequence's: it enters no
 * history and each window has its own, and it is empty with no window open.
 * (The time range a sweep leaves is the transport's `span`.)
 */
export class NotesEditor extends SequenceHandler {
    protected override readonly app = "notes";
    /** The sequence the roll edits, the one the editor was opened over. */
    readonly structure: EventSequence;
    protected override readonly shown: EventSequence;
    /** Whether a hand may edit the notes (`false` for a rendering). */
    readonly editable: boolean;
    /** Whether the roll paints incoming MIDI into the sequence. */
    readonly midiIn: boolean;
    /** The roll's vertical axis, `"midi"` or `"hz"`. */
    readonly yAxis: "midi" | "hz";
    /** The timeline a play to a destination of its own (a MIDI port) runs. */
    #elsewhere: Timeline | null = null;

    constructor(sequence: EventSequence, options: NotesEditorOptions = {}) {
        super("Notes", options);
        this.structure = sequence;
        this.shown = sequence;
        this.editable = options.editable ?? true;
        this.midiIn = options.midiIn ?? false;
        this.yAxis = options.yAxis ?? "midi";
    }

    protected override openingOptions(beside: HostedEditor | null): Record<string, unknown> {
        const options: Record<string, unknown> = {
            domain: this.yAxis,
            editable: this.editable,
            midi_in: this.midiIn,
        };
        if (beside === null) options.sequence = this.structure.data();
        return options;
    }

    protected override settle(left: Record<string, unknown> | null, last: boolean): void {
        void last;
        this.letGo(left);
    }

    protected override follow(): Promise<void> {
        return this.followSequence();
    }

    /** The sequence the roll edits -- the one the editor was opened over. */
    get sequence(): EventSequence {
        return this.structure;
    }

    /** The sequence's own tempo map, which the roll's axis is drawn through. */
    tempoMap(): TempoMap | null {
        return this.structure.tempoMap;
    }

    // ---- playing it ----

    /**
     * **The transport the window plays on** ({@link HostedTransport}), as the
     * object a page plays, its verbs about this window's pass and in the
     * sequence's beats -- the span is the band an Alt+drag sweeps on the roll,
     * the loop switch is `L`, and each side reads what the other set. Each window
     * over the sequence has its own, given back with it.
     */
    get transport(): HostedTransport {
        return new HostedTransport(this);
    }

    /**
     * **Where a pass ends**, as on a multitrack's transport: `null` by default --
     * the transport rolls on past the last note until it is stopped -- or
     * `"contents"`, where the last note ends, or a beat, an **end marker**.
     */
    async setEnd(end: End): Promise<void> {
        await this.act("end", { end });
    }

    /**
     * **Play the sequence** from `beat` -- or from the position cursor, or the
     * start -- on its window's transport.
     *
     * It is the audio editor's pass: `range` -- `[start, end]` in beats, a time
     * range a sweep left -- plays from its start to its end, going back to
     * `beat`; `looping` loops the range, or with none every note; with neither
     * the pass ends where {@link NotesEditor.setEnd} says.
     *
     * `destination` is for a MIDI port: the application has no MIDI output, so
     * the sequence is played on this client's clock to that destination instead,
     * as the MIDI messages a file of it holds, and an edit is heard from the next
     * play.
     */
    async play(beat?: number, destination?: PlayDestination, pass: Pass = {}): Promise<this> {
        if (destination !== undefined) {
            const played = new Timeline(
                this.structure.midiMessages().map(([at, message]) => [at, MidiItem(message)] as const),
            );
            const map = this.structure.tempoMap;
            if (map !== null) played.map = map;
            played.play({ at: beat ?? 0, destination });
            this.#elsewhere = played;
            return this;
        }
        const args: Record<string, unknown> = { looping: Boolean(pass.looping) };
        if (beat !== undefined) args.beat = Number(beat);
        if (pass.range !== undefined && pass.range !== null) args.range = [Number(pass.range[0]), Number(pass.range[1])];
        await this.act("play", args);
        return this;
    }

    /** Pause where it stands: a `resume` carries the notes on. */
    async pause(): Promise<this> {
        if (this.#elsewhere !== null) {
            this.#elsewhere.pause();
            return this;
        }
        await this.act("pause");
        return this;
    }

    /** Roll again from where it paused. */
    async resume(): Promise<this> {
        if (this.#elsewhere !== null) {
            await this.#elsewhere.play();
            return this;
        }
        await this.act("resume");
        return this;
    }

    /** Stop, free what sounds, and go back to where it started. */
    async stop(): Promise<this> {
        if (this.#elsewhere !== null) {
            this.#elsewhere.stop();
            this.#elsewhere = null;
            return this;
        }
        await this.act("stop");
        return this;
    }

    /** Whether the sequence is sounding, as the engine answers. */
    async playing(): Promise<boolean> {
        if (this.closed) return false;
        return Boolean(await this.act("playing"));
    }

    /** The position cursor to beat `at`: the next play starts from the mark. */
    async locate(at: number): Promise<void> {
        await this.act("locate", { at: Number(at) });
    }

    /** Split what is held at the cursor: the window's `split`. */
    split(): boolean {
        return this.verb("split");
    }

    /** Join a touching run of what is held: the window's `join`. */
    join(): boolean {
        return this.verb("join");
    }

    /** Quantize what is selected to the grid: the window's `quantize`. */
    quantize(): boolean {
        return this.verb("quantize");
    }

    /** Delete what is selected: the window's `delete`. */
    delete(): boolean {
        return this.verb("delete");
    }

    /**
     * **The grid a note lands on, in beats** -- `0.25` for a sixteenth, its
     * default, and `0` for none: what a drag snaps to and `quantize` writes into
     * the sequence. The window's own, like its zoom.
     */
    async grid(): Promise<number> {
        return Number((await this.coreCall("grid")).grid ?? 0);
    }

    /** Sets the grid, in beats: no edit. */
    async setGrid(beats: number): Promise<void> {
        await this.setting({ grid: Number(beats) });
    }

    /** **Whether a hand snaps to the grid**: G in the window and the View menu flip it. */
    async snapToGrid(): Promise<boolean> {
        return Boolean((await this.coreCall("grid")).snap ?? true);
    }

    /** Sets whether a hand snaps to the grid: no edit. */
    async setSnapToGrid(on: boolean): Promise<void> {
        await this.setting({ snap: Boolean(on) });
    }

    // ---- the roll's two selections ----

    /** Draw `span` -- `[start, end]` in beats, or `null` -- as the roll's time range, and play it on the space bar. */
    async showSpan(span: readonly [number, number] | null): Promise<void> {
        const written = span === null ? null : [Number(span[0]), Number(span[1])];
        await this.act("set", { call: { verb: "span", span: written } });
    }

    /** The events marked on the roll, by the ids the host holds. @internal */
    async marked(): Promise<SeqEvent[]> {
        if (this.closed) return [];
        const held = ((await this.query("selected")).selected ?? []) as unknown[];
        const events = new Map([...this.structure.events].map((event) => [event.id, event]));
        return held.map((id) => events.get(Number(id))).filter((event): event is SeqEvent => event !== undefined);
    }

    /** Mark `events` -- those of this sequence -- on the roll. @internal */
    async mark(events: Iterable<SeqEvent> | null): Promise<void> {
        if (this.closed) return;
        const regions = [...(events ?? [])].filter((event) => event.sequence === this.structure).map((event) => event.id);
        await this.act("select", { regions });
    }
}
marking(NotesEditor);

/** How a {@link NotesEditor} opens. */
export interface NotesEditorOptions extends HostedEditorOptions {
    /** `false` for a roll a hand may look at and not edit: the notes of a rendering. */
    editable?: boolean;
    /**
     * What the roll's vertical axis is: `"midi"`, MIDI notes on the keys,
     * snapped to semitones; or `"hz"`, frequency on a log scale, ruled in
     * hertz, where a note moves continuously and writes its `freq`.
     */
    yAxis?: "midi" | "hz";
    /**
     * `true` for a roll that **paints what is played**: the host opens its
     * MIDI input and writes each incoming note into the roll -- at the play
     * cursor while the transport rolls, held as long as the key, or on the
     * step cursor while it is stopped. Each one is an edit of the sequence,
     * like a note a hand drew: recorded, and undone with the rest.
     *
     * A native host opens a port named `clausters-gui` to route a keyboard
     * into. The host in a page listens to every MIDI input the browser has,
     * which the browser asks the person's permission for the first time a
     * roll that listens is opened.
     */
    midiIn?: boolean;
}

/**
 * Whether `edit` opens this in the notes editor: an event sequence, or a
 * timeline, which it renders into one.
 */
export function isEvents(structure: unknown): structure is EventSequence | Timeline {
    return structure instanceof EventSequence || structure instanceof Timeline;
}
