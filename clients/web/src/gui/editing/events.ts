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
    /** Whether a hand has changed the data since this was last cleared. */
    dirty = false;

    protected override changedByHand(): void {
        this.dirty = true;
    }

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

    /**
     * Join what is selected: the window's `join`. Notes of one pitch that
     * touch become one note, as a split's two halves do, and any other
     * selection of a line is joined, each note going on into the next without
     * a new attack ({@link EventSequence.join}).
     */
    join(): boolean {
        return this.verb("join");
    }

    /**
     * Take the joins among what is selected away -- the window's `unjoin`
     * (Shift+J), {@link EventSequence.unjoin} over the roll's selection.
     */
    unjoin(): boolean {
        return this.verb("unjoin");
    }

    /**
     * Move what is selected up -- every note the roll takes when nothing is:
     * a semitone in MIDI mode, a step of the key in notation mode, the letter
     * after with the sign the key writes on it. The window's `step_up` (Up);
     * one entry of the history.
     */
    stepUp(): boolean {
        return this.verb("step_up");
    }

    /** Move what is selected down, as {@link stepUp} moves it up: the window's `step_down` (Down). */
    stepDown(): boolean {
        return this.verb("step_down");
    }

    /**
     * Move what is selected up an octave, every note the roll takes when
     * nothing is, its letters kept: the window's `octave_up` (Ctrl+Up).
     */
    octaveUp(): boolean {
        return this.verb("octave_up");
    }

    /** Move what is selected down an octave: the window's `octave_down` (Ctrl+Down). */
    octaveDown(): boolean {
        return this.verb("octave_down");
    }

    /**
     * Select every note the roll takes that shares its pitch with a
     * selected one: the window's `select_pitch`; it edits nothing.
     */
    selectPitch(): boolean {
        return this.verb("select_pitch");
    }

    /**
     * Select every note the roll takes that shares its channel with a
     * selected one: the window's `select_channel`; it edits nothing.
     */
    selectChannel(): boolean {
        return this.verb("select_channel");
    }

    /**
     * Select every note the roll takes that shares its staff and voice with a
     * selected one: the window's `select_voice`; it edits nothing.
     */
    selectVoice(): boolean {
        return this.verb("select_voice");
    }

    /**
     * Select every note the roll takes that shares its staff with a
     * selected one: the window's `select_staff`; it edits nothing.
     */
    selectStaff(): boolean {
        return this.verb("select_staff");
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

    /**
     * **Whether the roll edits one note at a time in a line** -- off until it
     * is turned on. On, a gesture never leaves two notes of one line (a staff
     * and voice, else a channel) overlapping: a note lengthened or moved over
     * the next shortens it from its start, removes it where it covers it
     * whole, and shortens the one before it to its onset, all in the gesture's
     * one edit. A length that reaches the next note does not join them. The
     * window's own, like its grid: M in the window and the Edit menu flip it.
     */
    async monophonic(): Promise<boolean> {
        return Boolean((await this.coreCall("editing")).monophonic ?? false);
    }

    /** Sets whether the roll edits monophonically: no edit. */
    async setMonophonic(on: boolean): Promise<void> {
        await this.setting({ monophonic: Boolean(on) });
    }

    /**
     * **What a plain press on the roll's grid does**, the `"select"` pointer
     * until another is chosen: with it a click selects a note, a drag moves
     * it or, by an edge, resizes it, a drag over empty grid sweeps a
     * selection and a double click there makes a note. `"draw"` makes a note
     * where it is pressed, the drag its length; `"erase"` takes away every
     * note pressed or crossed; `"split"` cuts the note clicked where it is
     * clicked; `"stretch"` scales the selection in time from its first
     * onset. Every tool keeps the modifiers: Ctrl adds or erases, Alt and
     * Shift with a click add a note to the selection or take it out, Shift
     * and a vertical drag is the velocity. The keys 1 to 5, the toolbar and
     * the Edit menu's Tool choose it.
     */
    async tool(): Promise<string> {
        return String((await this.coreCall("editing")).tool ?? "select");
    }

    /**
     * Sets what a plain press does: no edit. Rejects with a `RangeError`,
     * changing nothing, for a name no tool has.
     */
    async setTool(tool: "select" | "draw" | "erase" | "split" | "stretch"): Promise<void> {
        const answer = (await this.act("set", { call: { verb: "sync", tool: String(tool) } })) as
            | { refused?: string }
            | null;
        if (answer?.refused) throw new RangeError(answer.refused);
    }

    /**
     * **Whether the panel beside the roll is shown**: the inspector of the
     * selected notes and the list of the sequence's events, which follow the
     * roll's selection both ways. A value typed in the inspector is written on
     * every selected note as the roll writes it, one entry of the history; a
     * row chosen in the list selects its note. The View menu's entry shows and
     * hides it.
     */
    async eventsPanel(): Promise<boolean> {
        return Boolean((await this.coreCall("editing")).panel ?? false);
    }

    /** Shows or hides the panel beside the roll: no edit. */
    async setEventsPanel(on: boolean): Promise<void> {
        if (Boolean(on) !== (await this.eventsPanel())) this.verb("events_panel");
    }

    /**
     * **Whether the display panel beside the roll is shown**: how the roll
     * draws what the sequence holds beyond its boxes ({@link display}). The
     * View menu's entry shows and hides it, and setting it is no edit.
     */
    async displayPanel(): Promise<boolean> {
        return Boolean((await this.coreCall("editing")).display_panel ?? false);
    }

    /** Shows or hides the display panel beside the roll: no edit. */
    async setDisplayPanel(on: boolean): Promise<void> {
        if (Boolean(on) !== (await this.displayPanel())) this.verb("display_settings");
    }

    /**
     * **How the roll draws what the sequence holds beyond its boxes**, the
     * window's own and no edit: `slurs` (`"none"`, `"arc"`, `"dashed"` -- an
     * arc dashed where the slur attacks its notes -- `"bracket"` or `"band"`),
     * `marks` (a note's articulations: `"none"`, `"stacked"` at its start or
     * in a `"row"` along it) and `marks_place` (`"above"` the box or
     * `"inside"` it) -- the two a page's, drawn in notation mode alone --
     * `pitch_line` (the line between the boxes), `note_curves` (the notes'
     * own curves) and `hidden_lanes` (the lanes not offered as tabs, by name).
     */
    async display(): Promise<Record<string, unknown>> {
        return { ...(((await this.coreCall("editing")).display ?? {}) as Record<string, unknown>) };
    }

    /**
     * Changes the display settings named ({@link display}), the others kept;
     * `lanes: { velocity: false }` takes a lane off the tabs and `true` puts
     * it back. Rejects with a `RangeError` for a value none of them takes,
     * which changes nothing.
     */
    async setDisplay(settings: Record<string, unknown>): Promise<void> {
        const answer = (await this.act("set", { call: { verb: "sync", display: settings } })) as
            | { refused?: string }
            | null;
        if (answer?.refused) throw new RangeError(answer.refused);
    }

    // ---- the roll's mode, which is its format's ----

    /**
     * **The roll's mode**: `"notation"` over a sequence written as a page --
     * each note on a staff and a voice, as long as it is written with what it
     * sounds inside it, a velocity shown and edited as a dynamic, and what it
     * sounds derived from what is written -- and `"midi"` over every other:
     * the events as MIDI says them, notes on and off, velocities, channels,
     * curves, and joins in a sequence for the server (MIDI has none). It is
     * the sequence's format's ({@link EventSequence.format}),
     * and nothing the roll does changes it: going to another format is a
     * conversion into a copy ({@link NotesEditor.toNotation},
     * {@link NotesEditor.toMidi}).
     */
    async mode(): Promise<"midi" | "notation"> {
        return ((await this.coreCall("editing")).mode ?? "midi") as "midi" | "notation";
    }

    /**
     * **A copy of the sequence written as notation**: read with `meter`
     * (`"3/4"`) and `division` (`16` for a sixteenth) over its stored reading,
     * each part written in `"voices"` or with `"ties"`
     * ({@link EventSequence.notated}). The sequence the roll edits is not
     * changed; the copy is opened in a roll of its own with `edit`. The
     * window's *Convert to notation...* is the same conversion, and opens the
     * copy itself.
     */
    toNotation(
        options: { meter?: string; division?: number; writing?: "voices" | "ties" } = {},
    ): EventSequence {
        const reading: Record<string, unknown> = {};
        if (options.meter !== undefined) reading.meter = options.meter;
        if (options.division !== undefined) reading.division = options.division;
        const given = Object.keys(reading).length > 0 ? reading : undefined;
        return this.structure.notated(given, options.writing);
    }

    /**
     * **A copy of the sequence as its performance, written for `spec`**
     * ({@link EventSequence.asMidi}): `"1.0"`, `"mpe"` or `"2.0"`. The sequence
     * the roll edits is not changed. The window's *Convert to MIDI 1.0*, *to
     * MPE* and *to MIDI 2.0* are the same conversions, and open the copy
     * themselves.
     */
    toMidi(spec = "1.0", options: { upper?: boolean; members?: number } = {}): EventSequence {
        return this.structure.asMidi(spec, options);
    }

    /**
     * **What the roll shows in notation mode**: `"system"`, every staff;
     * `["part", n]`, the staves of part `n`; or `["staff", n]`. The window's,
     * like its zoom: no edit.
     */
    async scope(): Promise<"system" | ["part" | "staff", number]> {
        const said = ((await this.coreCall("editing")).scope ?? { show: "system" }) as { show: string; index?: number };
        if (said.show === "system") return "system";
        return [said.show as "part" | "staff", Number(said.index ?? 0)];
    }

    /** Sets what the roll shows in notation mode: no edit. */
    async setScope(scope: "system" | ["part" | "staff", number]): Promise<void> {
        if (scope === "system") {
            await this.setting({ scope: { show: "system" } });
            return;
        }
        const [kind, index] = scope;
        if (kind !== "part" && kind !== "staff") {
            throw new Error(`a roll shows the "system", a "part" or a "staff", not ${JSON.stringify(kind)}`);
        }
        await this.setting({ scope: { show: kind, index } });
    }

    /**
     * **The lane shown under the roll**, chosen by the tabs there:
     * `"velocity"` for the notes' velocities, or a curve of the sequence by
     * its id. The window's, like its zoom: setting it is no edit.
     */
    async lane(): Promise<string> {
        return String((await this.coreCall("editing")).lane ?? "velocity");
    }

    /** Shows the lane `lane` under the roll: no edit. */
    async setLane(lane: string): Promise<void> {
        await this.setting({ lane: String(lane) });
    }

    /**
     * **A curve of the channel edited** ({@link channel}), else of the
     * channel of the first selected note (the first channel with none
     * selected): `"cc1"` the modulation wheel, `"cc11"` the
     * expression, `"bend"` or `"pressure"` -- the window's `curve_cc1`,
     * `curve_cc11`, `curve_bend` and `curve_pressure`, the Edit menu's
     * Curves. One point at the parameter's resting value; refused, with why,
     * where the sequence's format cannot say it (under MPE a channel says its
     * controllers alone). Throws for another name.
     */
    addCurve(name: "cc1" | "cc11" | "bend" | "pressure"): boolean {
        if (!["cc1", "cc11", "bend", "pressure"].includes(name)) {
            throw new Error(`no channel curve ${JSON.stringify(name)}: cc1, cc11, bend or pressure`);
        }
        return this.verb(`curve_${name}`);
    }

    /**
     * **A curve of each selected note** that has none of its kind: `"bend"`,
     * `"pressure"` or `"timbre"` -- the window's `note_bend`, `note_pressure`
     * and `note_timbre`. Refused, with why, where the sequence's format
     * cannot say it of one note. Throws for another name.
     */
    addNoteCurve(name: "bend" | "pressure" | "timbre"): boolean {
        if (!["bend", "pressure", "timbre"].includes(name)) {
            throw new Error(`no note curve ${JSON.stringify(name)}: bend, pressure or timbre`);
        }
        return this.verb(`note_${name}`);
    }

    /** **The curve of the lane shown taken away** -- `curve_remove`; refused with the velocities shown. */
    removeCurve(): boolean {
        return this.verb("curve_remove");
    }

    /**
     * **What a note's tint tells apart in notation mode**: `"voice"`,
     * `"staff"` or `"part"` -- the first of each untinted. The window's, like
     * its zoom: setting it is no edit. The Notation menu has it as Color by
     * voice, staff or part.
     */
    async colorBy(): Promise<"voice" | "staff" | "part"> {
        const said = (await this.coreCall("editing")).color_by;
        return said === "staff" || said === "part" ? said : "voice";
    }

    /** Sets what a note's tint tells apart in notation mode: no edit. */
    async setColorBy(colorBy: "voice" | "staff" | "part"): Promise<void> {
        if (colorBy !== "voice" && colorBy !== "staff" && colorBy !== "part") {
            throw new Error(`a roll colors by "voice", "staff" or "part", not ${JSON.stringify(colorBy)}`);
        }
        await this.setting({ color_by: colorBy });
    }

    /**
     * **The voice the roll edits in notation mode**, `[staff, voice]` counted
     * from 0: what a note drawn is written in, and the one voice a press takes
     * -- the others are drawn faint. No edit.
     */
    async voice(): Promise<[number, number]> {
        const [staff, voice] = ((await this.coreCall("editing")).voice ?? [0, 0]) as [number, number];
        return [Number(staff), Number(voice)];
    }

    /** Sets the voice the roll edits: no edit. */
    async setVoice(voice: [number, number]): Promise<void> {
        await this.setting({ voice: [Math.trunc(voice[0]), Math.trunc(voice[1])] });
    }

    /**
     * **The channel the roll edits in MIDI mode**, counted from 0 as a
     * note's `channel` is, or `null` for every channel (the default): what a
     * note drawn or pasted and a channel's curve added land on, and the one
     * channel a press, a selection and the velocity lane take -- the others
     * are drawn faint. The Edit menu's Channel chooses it. No edit.
     */
    async channel(): Promise<number | null> {
        const said = (await this.coreCall("editing")).channel;
        return said === null || said === undefined ? null : Number(said);
    }

    /**
     * Sets the channel the roll edits, `null` for every one: no edit.
     * Rejects with a `RangeError`, changing nothing, where none is chosen:
     * in notation mode (a page is edited a voice at a time) and under MPE (a
     * channel is one note's).
     */
    async setChannel(channel: number | null): Promise<void> {
        const said = channel === null ? null : Math.trunc(channel);
        const answer = (await this.act("set", { call: { verb: "sync", channel: said } })) as
            | { refused?: string }
            | null;
        if (answer?.refused) throw new RangeError(answer.refused);
    }

    /**
     * **How the sequence's parts write their polyphony**: `"voices"` or
     * `"ties"` -- the same music, written two ways.
     */
    writing(): "voices" | "ties" {
        const parts = this.structure.parts;
        const writing = (parts[0]?.writing ?? {}) as { polyphony?: string };
        return writing.polyphony === "ties" ? "ties" : "voices";
    }

    /** Writes every part so: one edit of the parts that changes no event and nothing heard. */
    setWriting(writing: "voices" | "ties"): boolean {
        if (writing !== "voices" && writing !== "ties") {
            throw new Error(`a writing is "voices" or "ties", not ${JSON.stringify(writing)}`);
        }
        return this.verb(`write_${writing}`);
    }

    /**
     * A slur over what is selected, from its first note to its last -- the
     * window's `slur` (S), in notation mode; joined where the roll edits
     * monophonically, attacked otherwise. Again over the same notes, it is
     * taken away.
     */
    slur(): boolean {
        return this.verb("slur");
    }

    /** Turn the slurs on what is selected from joined to attacked, or back -- `slur_join` (Shift+S). */
    slurJoin(): boolean {
        return this.verb("slur_join");
    }

    /**
     * Tie the selected notes of one pitch that touch into one -- the window's
     * `tie` (T): a tied note is one event, which the page writes with ties.
     */
    tie(): boolean {
        return this.verb("tie");
    }

    /** A staccato on each selected note, or none where all have one -- `staccato` (.). */
    staccato(): boolean {
        return this.verb("staccato");
    }

    /** A tenuto on each selected note, or none where all have one -- `tenuto` (-). */
    tenuto(): boolean {
        return this.verb("tenuto");
    }

    /** An accent on each selected note, or none where all have one -- `accent` (>). */
    accent(): boolean {
        return this.verb("accent");
    }

    /** A marcato on each selected note, or none where all have one -- `marcato` (^). */
    marcato(): boolean {
        return this.verb("marcato");
    }

    /**
     * A breath after each selected note, or none where all have one -- the
     * window's `breath` (,): the note is let go a little before the next
     * attack, and heard so.
     */
    breath(): boolean {
        return this.verb("breath");
    }

    /**
     * **The dynamic `name` written on each selected note** -- `"pp"`, `"p"`,
     * `"mp"`, `"mf"`, `"f"` or `"ff"` -- or taken away with `null`: the
     * window's `dynamic_pp` to `dynamic_ff` and `dynamic_none`, the Notation
     * menu's. The level the notes are heard at is derived from it. Throws for
     * another name.
     */
    dynamic(name: "pp" | "p" | "mp" | "mf" | "f" | "ff" | null): boolean {
        if (name !== null && !["pp", "p", "mp", "mf", "f", "ff"].includes(name)) {
            throw new Error(`no dynamic ${JSON.stringify(name)}: pp, p, mp, mf, f or ff`);
        }
        return this.verb(`dynamic_${name ?? "none"}`);
    }

    /** The pedal down from the first selected note to the last -- `pedal` (P). */
    pedal(): boolean {
        return this.verb("pedal");
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
