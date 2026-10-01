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
 * **It sounds through a playback of its own** (the crate's `NotesPlayback`):
 * the sequence is the data of an event lane on the notes editor's own
 * transport, so playing it never moves a multitrack, and the server plays it
 * by the transport's position -- a pause, a stop and a locate are the
 * transport's. An edit sends the lane its new data, so a note moved ahead of
 * the line is heard where it lands, and what is sounding keeps its release.
 * The space bar over the window plays and pauses.
 *
 * @module
 */

import { EVENTS } from "../../document.ts";
import { NotesPlayback as CorePlayback, StepRunner } from "../../core/clausters_core_web.js";
import type { Server } from "../../defs/server/index.ts";
import { resolveServer } from "../../defs/wire.ts";
import { runSteps } from "../../steps.ts";
import type { TempoMap } from "../../base/time.ts";
import { EventSequence } from "../../seq/sequence.ts";
import { MidiItem } from "../../seq/event.ts";
import { Timeline } from "../../seq/timeline.ts";
import type { PlayDestination } from "../../seq/timeline.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import type { Answer } from "./echo.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import type { End, Pass } from "./playback.ts";
import { plain } from "./samples.ts";
import { View } from "./view.ts";

/** What one turn of the core came to. */
interface Outcome {
    turn?: string;
    changed?: boolean;
    answer?: Answer;
    play?: { looping: boolean; range?: [number, number] | null };
    loop?: { looping: boolean; range?: [number, number] | null };
    locate?: number;
}

/**
 * **What sounds the notes editors of one server** -- the crate's playback and
 * the steps it answers, carried out on that server. One per server, since the
 * editors on it share one transport: the one played last is the one that
 * sounds.
 */
class NotesPlayback {
    static readonly #of = new WeakMap<Server, NotesPlayback>();

    static of(server: Server): NotesPlayback {
        let found = NotesPlayback.#of.get(server);
        if (found === undefined) {
            found = new NotesPlayback(server);
            NotesPlayback.#of.set(server, found);
        }
        return found;
    }

    readonly #native = new CorePlayback(-1);
    readonly #runner = new StepRunner();
    readonly #ready: Promise<void>;
    /** The engine's sample rate. */
    rate = 48_000;
    /** The transport it plays on -- the crate's word for it. */
    readonly transport: number;
    /** The sequence the lane holds, if any: the one played last. */
    planned: EventSequence | null = null;
    readonly server: Server;

    private constructor(server: Server) {
        this.server = server;
        this.transport = Number(
            JSON.parse(this.#native.call(new EventSequence().seq, JSON.stringify({ verb: "state" }), server.ids))
                .transport,
        );
        this.#ready = (async () => {
            // Node ids come back on their `/node_end`, which only a registered
            // client hears.
            await server.notify(true);
            this.rate = (await server.queryInfo()).nominalSampleRate;
        })();
    }

    /** The transport as the engine has it. */
    async state(): Promise<{ playing: boolean }> {
        const state = await this.server.transportAt(this.transport).transportState();
        return { playing: state.playing };
    }

    /** One verb over `sequence`, its steps carried out. */
    async call(verb: string, sequence: EventSequence, args: Record<string, unknown> = {}): Promise<void> {
        await this.#ready;
        const answer = JSON.parse(this.#native.call(
            sequence.seq,
            JSON.stringify({ verb, rate: this.rate, ...args }),
            this.server.ids,
        )) as Record<string, unknown>;
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        const steps = answer.steps as unknown[] | undefined;
        if (steps !== undefined && steps.length > 0) await runSteps(this.server, this.#runner, steps);
    }
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
        const ed = editor as NotesEditor;
        // The axis names which roll of the sequence this is: a roll in hertz
        // is another picture of it, and a window beside one in MIDI notes
        // must not draw on its widget.
        const wid = this.widget(editor, "notes", editor.structure, ed.yAxis);
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
    /** The roll's vertical axis, `"midi"` or `"hz"`. */
    readonly yAxis: "midi" | "hz";
    /** The server it plays on, resolved when it first plays. */
    #server: Server | null;
    /** The playback work in flight, chained so each lands in order. */
    #work: Promise<void> = Promise.resolve();
    /** The timeline a play to a destination of its own (a MIDI port) runs. */
    #elsewhere: Timeline | null = null;
    /** Where a pass ends ({@link NotesEditor.end}). */
    #end: End = null;

    constructor(sequence: EventSequence, options: NotesEditorOptions) {
        const domain = new NotesDomain();
        super(sequence, { title: "Notes", ...options, domain, view: new NotesView() });
        this.editable = options.editable ?? true;
        this.yAxis = options.yAxis ?? "midi";
        this.#server = options.server ?? null;
        const opened = this.editing.openNotes(
            `sequence:${keyOfSequence(sequence)}`,
            sequence,
            {
                rate: this.sampleRate,
                editable: this.editable,
                domain: this.yAxis,
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

    /**
     * Opens the window, with its play cursor drawn from the transport.
     *
     * The roll anchors the play cursor at 0, and the counter that makes that
     * the sequence's own sample is the position of the transport the notes
     * editor plays on -- stopped or rolling, the line is where the lane is.
     * With no server to play on there is no position, and no line.
     */
    override async open(
        host?: Parameters<Editor<EventSequence>["open"]>[0],
        options: Parameters<Editor<EventSequence>["open"]>[1] = {},
    ): ReturnType<Editor<EventSequence>["open"]> {
        const handle = await super.open(host, options);
        let transport: number;
        try {
            transport = this.#playback.transport;
        } catch {
            return handle;
        }
        this.host?.headClock(handle, "transport", transport);
        return handle;
    }

    /**
     * The position cursor was placed at beat `at`: a stopped transport is
     * cued there, so the play cursor goes with it and the next play starts
     * from the mark; a rolling pass is left alone.
     */
    override locate(at: number): void {
        if (this.#server === null || this.#elsewhere !== null) return;
        const playback = this.#playback;
        this.#work = this.#work.then(() => playback.call("cue", this.structure, { at }));
        this.#work.catch(() => {});
    }

    // ---- playing it ----

    get #playback(): NotesPlayback {
        this.#server ??= resolveServer(null) as unknown as Server;
        return NotesPlayback.of(this.#server);
    }

    /**
     * **Where a pass ends**, as on a multitrack's transport: `null` by default
     * -- the transport rolls on past the last note until it is stopped -- or
     * `"contents"`, where the last note ends (its onset and its length), or a
     * beat, an **end marker**; either of the last two goes back to the
     * position cursor. A note's release rings out past the end, since a stop
     * releases the notes rather than freezing them.
     */
    get end(): End {
        return this.#end;
    }

    set end(end: End) {
        this.#end = end;
        if (this.#server === null) return;
        const playback = this.#playback;
        this.#work = this.#work.then(() => playback.call("end", this.structure, { end }));
        this.#work.catch(() => {});
    }

    /**
     * **Plays the sequence** from `beat` -- or from the position cursor, or the
     * start -- on the notes editor's own transport.
     *
     * It is the audio editor's pass: `range` -- `[start, end]` in beats, a time
     * range a sweep left -- plays from its start to its end, going back to
     * `beat`; `looping` loops the range, or with none every note; with neither
     * the pass ends where {@link NotesEditor.end} says.
     *
     * `destination` is for a MIDI port (a `MidiServer`): the server has no MIDI
     * output, so the sequence is played on this page's clock to that
     * destination instead, as the MIDI messages a file of it holds
     * ({@link EventSequence.midiMessages}) -- its automation and its
     * notes' included -- and an edit is heard from the next play.
     */
    async play(beat?: number, destination?: PlayDestination, pass: Pass = {}): Promise<this> {
        const start = beat ?? this.cursor ?? 0;
        if (destination !== undefined) {
            // The render a file of it holds: its notes, its automation and
            // its notes', as its MIDI spec says them.
            const played = new Timeline(
                this.structure.midiMessages().map(([beat, message]) => [beat, MidiItem(message)] as const),
            );
            const map = this.structure.tempoMap;
            if (map !== null) played.map = map;
            played.play({ at: start, destination });
            this.#elsewhere = played;
            return this;
        }
        const playback = this.#playback;
        await playback.call("end", this.structure, { end: this.#end });
        await playback.call("play", this.structure, {
            from: start,
            range: pass.range ?? null,
            loop: pass.looping ?? false,
        });
        playback.planned = this.structure;
        return this;
    }

    /** Pauses where it stands: a `resume` carries the notes on. */
    async pause(): Promise<this> {
        if (this.#elsewhere !== null) {
            this.#elsewhere.pause();
            return this;
        }
        await this.#playback.call("pause", this.structure);
        return this;
    }

    /** Rolls again from where it paused. */
    async resume(): Promise<this> {
        if (this.#elsewhere !== null) {
            this.#elsewhere.play();
            return this;
        }
        await this.#playback.call("resume", this.structure);
        return this;
    }

    /** Stops, frees what sounds, and goes back to where it started. */
    async stop(): Promise<this> {
        if (this.#elsewhere !== null) {
            this.#elsewhere.stop();
            this.#elsewhere = null;
            return this;
        }
        const playback = this.#playback;
        await playback.call("stop", this.structure, { back: this.cursor ?? 0 });
        return this;
    }

    /**
     * Whether the sequence is sounding, as the engine answers. A method here,
     * the reference client's property: asking the engine is a round trip, and a
     * page awaits one.
     */
    async playing(): Promise<boolean> {
        if (this.#server === null) return false;
        const playing = (await this.#playback.state()).playing;
        await this.#playback.call("setRolling", this.structure, { rolling: playing });
        return playing;
    }

    /**
     * The sequence changed: when it is what the lane holds, the lane takes it
     * again, and the server plays it on from where the position is.
     */
    #update(): void {
        if (this.#server === null) return;
        const playback = this.#playback;
        if (playback.planned !== this.structure) return;
        this.#work = this.#work.then(() => playback.call("update", this.structure));
        this.#work.catch(() => {});
    }

    /**
     * Waits for the playback work an edit or a key started.
     *
     * @internal
     */
    async settled(): Promise<void> {
        await this.#work;
    }

    /**
     * A history step landed: the window is corrected, and the lane takes the
     * sequence again, so the undo is heard.
     */
    override reflectStep(): void {
        super.reflectStep();
        this.#update();
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
            this.#update();
        }
        if (outcome.locate !== undefined) {
            this.cursor = outcome.locate;
            // The roll plays on a transport of its own, so the mark is its own
            // too: a multitrack it was opened from keeps its cursor.
            this.locate(this.cursor);
            this.onLocate?.(this.cursor);
        }
        if (outcome.play !== undefined) {
            // The space bar is play/stop: a stop goes back to the position
            // cursor, so the play cursor lands where the reader left the mark.
            this.#work = this.#work.then(async () => {
                if (await this.playing()) await this.stop();
                else await this.play(undefined, undefined, {
                    range: outcome.play?.range ?? null,
                    looping: outcome.play?.looping ?? false,
                });
            });
            this.#work.catch(() => {});
        }
        const relooped = outcome.loop;
        if (relooped !== undefined && this.#server !== null) {
            // `L`: the pass in progress loops, or stops looping, from where it
            // stands; a stopped playback reads the switch on its next play.
            const playback = this.#playback;
            this.#work = this.#work.then(() =>
                playback.call("loop", this.structure, {
                    range: relooped.range ?? null,
                    loop: relooped.looping,
                }),
            );
            this.#work.catch(() => {});
        }
        this.echo.send(outcome.answer);
        return changed;
    }
}

/** {@link NotesEditor}'s options: the generic ones plus whether it writes. */
export interface NotesEditorOptions extends GenericEditorOptions<EventSequence> {
    /** `false` for a roll a hand may look at and not edit: the notes of a rendering. */
    editable?: boolean;
    /**
     * What the roll's vertical axis is: `"midi"`, MIDI notes on the keys,
     * snapped to semitones; or `"hz"`, frequency on a log scale, ruled in
     * hertz, where a note moves continuously and writes its `freq`.
     */
    yAxis?: "midi" | "hz";
    /** The server it plays on; absent, the ambient one when it first plays. */
    server?: Server | null;
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
