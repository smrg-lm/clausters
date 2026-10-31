/**
 * Editing a symbolic score on its engraved page: the score editor (mirrors
 * `clausters/gui/editing/score.py`).
 *
 * What it opens is a `Score` -- notation held as MEI, its model a sheet. **The
 * score editor is an application of the GUI host's**, as the audio editor and the
 * multitrack are: the window, what each gesture does to the score, the verbs over
 * what is selected, the entry each one leaves, the engraver and the playback all
 * run in the host's process, and this is its handler ({@link HostedEditor}). Each
 * verb below is one call to the application, named as it names it, and a promise
 * here where the reference client's answers at once.
 *
 * **The page's score is the window's while one is open**: a change through it is
 * an edit of the window's, and `Score.mei` and `Score.sheet` answer from a copy
 * this page holds, brought to the window's after each change. When the window
 * goes, the score holds what it left it as.
 *
 * @module
 */

import { readFileAt, writeFileAt } from "../../base/files.ts";
import { Score, TRANSCRIPTION_KEYS } from "../notation/index.ts";
import type { Interpretation, Transcription } from "../notation/index.ts";
import { EventSequence } from "../../seq/sequence.ts";
import { Held, HostedEditor, HostedTransport } from "./hosted.ts";
import type { HostedEditorOptions } from "./hosted.ts";
import { SequenceHandler } from "./events.ts";
import { plain } from "./samples.ts";

/** How a score editor is opened. */
export interface ScoreEditorOptions extends HostedEditorOptions {
    /**
     * The written value a note entered on the page takes, as `[numerator,
     * denominator]` of a whole note; a quarter by default.
     */
    value?: readonly [number, number];
    /**
     * The sequence the page is the reading of ({@link ScoreEditor.over},
     * which is what sets it): an edit on the page is then an edit of it.
     */
    over?: EventSequence | null;
    /** How `over` is read where its events do not say, as the crate spells it. */
    how?: Record<string, unknown>;
    /** The reading: what a mark sounds as, and what a level is named. */
    interp?: Interpretation;
    /**
     * `false` opens the page alone, in its scroll, over the status line: no
     * menu bar, no toolbar, no palettes and no dialogs, none of them composed
     * -- the window a page wants when it edits through this handle and reads
     * the score back. The keys stay (N and note entry, the arrows, Delete,
     * the space bar, Ctrl+Z; F1 shows them all), and so does every verb of
     * the editor. A verb refused answers `false` and says why on the status
     * line and in {@link ScoreEditor.refused}. What a form asked -- the path of a first Ctrl+S -- is asked of
     * the handle instead ({@link ScoreEditor.save}), and the window's close
     * mark closes it at once, since the score it edits is the one this
     * handle holds.
     */
    chrome?: boolean;
}

/** A score's page setup: lengths in tenths of a millimetre, the staff in hundredths. */
export interface PageSetup {
    width: number;
    height: number;
    /** Top, right, bottom, left. */
    margins: [number, number, number, number];
    /** The height of a five-line staff. */
    staff: number;
}

/** What {@link ScoreEditor.page} answers. */
export interface PageInfo {
    page: PageSetup;
    /** The paper's name, when it is one of `papers`. */
    paper: string | null;
    landscape: boolean;
    /** The papers there are, by name. */
    papers: string[];
}

/** Where {@link ScoreEditor.setText} puts a text, and which footnote it is. */
export interface TextOptions {
    /** Which footnote, from zero. */
    index?: number;
    region?: "head" | "foot";
    halign?: "left" | "center" | "right";
    valign?: "top" | "middle" | "bottom";
    pages?: "first" | "all";
}

/** What {@link ScoreEditor.setPage} changes beside the paper. */
export interface PageOptions {
    landscape?: boolean;
    width?: number;
    height?: number;
    margins?: readonly [number, number, number, number];
    staff?: number;
}

/**
 * A symbolic score on its page, edited by hand, in place.
 *
 * A press on a note selects it, a drag moves it along its staff, and a press on
 * a staff selects its measure. **Note entry** is a mode
 * ({@link ScoreEditor.entry}, the toolbar's pencil, or N; Escape leaves it): an
 * edit cursor stands on a staff, in a voice, and what is entered is written
 * there over what was there, nothing after it moving -- a letter `a` to `g`
 * writes that pitch of {@link ScoreEditor.value} and the cursor goes on, Shift
 * and a letter adds it to the chord, a press on a staff writes at the time it
 * fell at (on a note, into its chord), the arrows move the cursor and
 * Ctrl+Alt+1 to 4 change its voice. Playing leaves the mode. Ctrl+click adds a
 * note to the selection or takes it out, and Shift+click extends the selection
 * to it, in time and across the staves between. The verbs act on what is
 * selected ({@link ScoreEditor.selected}, {@link ScoreEditor.select}); each is
 * one entry of the editing context's history, so Ctrl+Z over the window walks
 * them back.
 */
export class ScoreEditor extends SequenceHandler {
    protected override readonly app = "score";
    readonly #score: Score;
    /** What the editor edits: the sequence it is over, or the score. */
    readonly structure: Score | EventSequence;
    /**
     * The sequence this page is the reading of, in an editor opened over one
     * ({@link ScoreEditor.over}); `null` for an editor over a score of its
     * own.
     */
    readonly sequence: EventSequence | null;
    protected override readonly shown: EventSequence | null;
    readonly #value: readonly [number, number] | undefined;
    readonly #how: Record<string, unknown> | undefined;
    readonly #interp: Interpretation | undefined;
    #refused: string | undefined = undefined;

    constructor(score: Score, options: ScoreEditorOptions = {}) {
        super("Score", options);
        this.#score = score;
        this.structure = options.over ?? score;
        this.sequence = options.over ?? null;
        this.shown = this.sequence;
        this.#value = options.value;
        this.#how = options.how;
        this.#interp = options.interp;
    }

    /**
     * **The score editor over a sequence**, on the page it is read into
     * (`Score.fromEvents`): what `edit(sequence, { view: "score" })` opens.
     *
     * The sequence is the structure and the page a reading of it. Opening
     * changes nothing: a take keeps the times it was played with. **An edit on
     * the page changes in the sequence only what it changed on the page** --
     * the note moved, marked or written, in its notation keys and in what it
     * sounds -- and every other event stays as it was, with its time, its
     * level and its curves. The entry is the sequence's, so a roll open over
     * the same sequence and this page are one undo order, and each follows
     * what the other does.
     *
     * The transcription's keys among `options` (`meter`, `key`, `clef`,
     * `beatUnit`, `division`, `tuplets`, `voices`, `dynamics`) say how the
     * sequence is read where its events do not, and `interp` is the reading;
     * the rest are the editor's own. The editor's `structure` is the
     * sequence, and `score` the page.
     */
    static async over(
        sequence: EventSequence,
        options: ScoreEditorOptions & Transcription = {},
    ): Promise<ScoreEditor> {
        const how: Record<string, unknown> = {};
        const others: Record<string, unknown> = {};
        for (const [key, value] of Object.entries(options)) {
            const reads = (TRANSCRIPTION_KEYS as readonly string[]).includes(key);
            if (reads && value !== undefined && value !== null) how[key] = value;
            else if (!reads) others[key] = value;
        }
        const interp = (options as ScoreEditorOptions).interp;
        const page = await Score.fromEvents(sequence, { ...(how as Transcription), interp });
        // the crate spells the beat as the model does
        const { beatUnit, ...written } = how as Transcription;
        const said: Record<string, unknown> = { ...written };
        if (beatUnit !== undefined) said.beat_unit = beatUnit;
        return new ScoreEditor(page, {
            ...(others as ScoreEditorOptions),
            over: sequence,
            how: said,
            interp,
        });
    }

    protected override heldBy(): Held {
        if (this.sequence !== null) return super.heldBy();
        let held = heldScores.get(this.#score);
        if (held === undefined) {
            held = new Held();
            heldScores.set(this.#score, held);
        }
        return held;
    }

    protected override taken(): void {
        if (this.sequence !== null) super.taken();
    }

    protected override openingOptions(beside: HostedEditor | null): Record<string, unknown> {
        const options: Record<string, unknown> = { mei: this.#score.mei() };
        if (this.#score.path !== null && this.sequence === null) options.path = this.#score.path;
        if (this.#value !== undefined) options.value = [Math.trunc(this.#value[0]), Math.trunc(this.#value[1])];
        if (this.sequence !== null) {
            if (this.#how !== undefined) options.how = this.#how;
            if (this.#interp !== undefined) options.interp = this.#interp;
            if (beside === null) options.sequence = this.sequence.data();
        }
        return options;
    }

    /** Open the window, its play cursor drawn from the transport of its own the application gives it. A pass ends where the music does. */
    override async open(...args: Parameters<HostedEditor["open"]>): ReturnType<HostedEditor["open"]> {
        const opening = this.closed;
        const window = await super.open(...args);
        if (opening) {
            this.#score.window = this;
            await this.act("end", { end: "contents" });
        }
        return window;
    }

    protected override settle(left: Record<string, unknown> | null, last: boolean): void {
        if (this.#score.window === this) {
            this.#score.window = null;
            if (typeof left?.mei === "string") this.#score.holdWritten(left.mei);
            if (typeof left?.path === "string") this.#score.path = left.path;
        }
        void last;
        if (this.sequence !== null) this.letGo(left);
    }

    protected override async follow(): Promise<void> {
        const answer = await this.asked("page", { call: { verb: "mei" } });
        if (typeof answer?.mei === "string") this.#score.holdWritten(answer.mei);
        await this.followSequence();
    }

    /** A change the page's score made, in the page's order. @internal */
    forwardPage(call: Record<string, unknown>): void {
        this.send("page", { call });
    }

    /** A step the page's score asked of the window's history. @internal */
    stepAsked(direction: "undo" | "redo"): void {
        void (direction === "undo" ? this.undo() : this.redo());
    }

    /** The score on the page: the one the editor was opened over, or -- over a sequence -- the reading of it, which follows the sequence. */
    get score(): Score {
        return this.#score;
    }

    /**
     * **Why the last verb was refused**, or `undefined` when it was not: the
     * reason the status line shows, here for a page, which can tell a refusal it
     * can do something about (nothing selected) from one it cannot.
     */
    get refused(): string | undefined {
        return this.#refused;
    }

    // ---- what is selected, and the value in hand ----

    /** The selected items, as the model names them, each once. */
    async selected(): Promise<number[]> {
        const items = (await this.coreCall("selected")).items;
        return Array.isArray(items) ? items.map(Number) : [];
    }

    /** Select the page's elements `elements` (their `xml:id`s, as the page reports them), or nothing with an empty list. */
    async select(elements: Iterable<string>): Promise<void> {
        await this.act("set", { call: { verb: "select", elements: [...elements].map(String) } });
    }

    /** The written value a note entered on the page takes, as `[numerator, denominator]` of a whole note. */
    async value(): Promise<[number, number]> {
        const value = (await this.coreCall("value")).value;
        return Array.isArray(value) ? [Number(value[0]), Number(value[1])] : [1, 4];
    }

    /** Sets the written value a note entered on the page takes. */
    async setValue(value: readonly [number, number]): Promise<void> {
        await this.setting({ value: [Math.trunc(value[0]), Math.trunc(value[1])] });
    }

    /** Whether the window is in note entry. */
    async entry(): Promise<boolean> {
        return (await this.coreCall("entry")).entry === true;
    }

    /** Switches note entry. */
    async setEntry(on: boolean): Promise<void> {
        await this.setting({ entry: Boolean(on) });
    }

    /** Whether the value a note is entered with is dotted. */
    async dotted(): Promise<boolean> {
        return (await this.coreCall("input")).dotted === true;
    }

    /** Switches the dot on the value a note is entered with. */
    async setDotted(on: boolean): Promise<void> {
        await this.setting({ dotted: Boolean(on) });
    }

    /** Whether a press on empty staff writes a rest of the value in hand rather than a note. */
    async rest(): Promise<boolean> {
        return (await this.coreCall("input")).rest === true;
    }

    /** Switches writing a rest. */
    async setRest(on: boolean): Promise<void> {
        await this.setting({ rest: Boolean(on) });
    }

    /** The accidental the next note entered takes, in semitones from its letter, or `null`. */
    async nextAccidental(): Promise<number | null> {
        const armed = (await this.coreCall("input")).accidental;
        return typeof armed === "number" ? armed : null;
    }

    /** Arms the accidental the next note entered takes. */
    async setNextAccidental(alter: number | null): Promise<void> {
        await this.setting({ accidental: alter === null ? null : Math.trunc(alter) });
    }

    // ---- the layout, which is the window's, and the page, the document's ----

    /** How the window looks at the score: `"page"` or `"continuous"`. The window's, and enters no history. */
    async layout(): Promise<"page" | "continuous"> {
        return (await this.coreCall("layout")).layout === "continuous" ? "continuous" : "page";
    }

    /** Switches how the window looks at the score. */
    async setLayout(layout: "page" | "continuous"): Promise<void> {
        await this.setting({ layout: String(layout) });
    }

    /** The page the score is laid out on. Change it with {@link ScoreEditor.setPage}. */
    async page(): Promise<PageInfo> {
        return (await this.coreCall("page")) as unknown as PageInfo;
    }

    /** Lay the score out on another page, as one entry of the history. */
    setPage(paper: string | null = null, options: PageOptions = {}): Promise<boolean> {
        const call: Record<string, unknown> = { action: "page" };
        if (paper !== null) call.paper = paper;
        for (const key of ["landscape", "width", "height", "staff"] as const) {
            if (options[key] !== undefined) call[key] = options[key];
        }
        if (options.margins !== undefined) call.margins = options.margins.map(Math.trunc);
        return this.#act(call);
    }

    /** Write a text of the page, or move it, as one entry of the history. */
    setText(field: string, text: string | null = null, options: TextOptions = {}): Promise<boolean> {
        const call: Record<string, unknown> = { action: "text", field: String(field) };
        if (text !== null) call.text = text;
        for (const key of ["index", "region", "halign", "valign", "pages"] as const) {
            if (options[key] !== undefined) call[key] = options[key];
        }
        return this.#act(call);
    }

    // ---- the verbs, over what is selected ----

    move(steps: number): Promise<boolean> {
        return this.#act({ action: "move", steps: Math.trunc(steps) });
    }

    /**
     * Scale the selected items' written values by `numerator / denominator`
     * (`scale(2, 1)` is twice as long), against the barlines already there.
     */
    scale(numerator: number, denominator: number): Promise<boolean> {
        return this.#act({
            action: "scale",
            factor: [Math.trunc(numerator), Math.trunc(denominator)],
        });
    }

    /**
     * Give the selected notes an articulation (by its MEI name: `stacc`, `acc`,
     * `ten`, `marc`...), or take it away when all of them have it.
     */
    articulation(name: string): Promise<boolean> {
        return this.#act({ action: "articulation", name: String(name) });
    }

    /** Put a dynamic (`pp` ... `ff`) under the first selected note, or take it away with none. */
    dynamic(name: string | null = null): Promise<boolean> {
        return this.#act({ action: "dynamic", name });
    }

    /**
     * Give the selected notes an ornament (`trill`, `mordent`, `turn`,
     * `fermata`), or take it away with none.
     */
    ornament(name: string | null = null): Promise<boolean> {
        return this.#act({ action: "ornament", name });
    }

    /** Take every mark off the selected notes. */
    clearMarks(): Promise<boolean> {
        return this.#act({ action: "clear_marks" });
    }

    /** Tie the selected notes to the next, or untie them when the first is tied already. */
    tie(): Promise<boolean> {
        return this.#act({ action: "tie" });
    }

    /** Turn the selected notes into rests of the same length. */
    silence(): Promise<boolean> {
        return this.#act({ action: "silence" });
    }

    /** Select the item before -- the window's `select_left` (`verb`). */
    selectLeft(): boolean {
        return this.verb("select_left");
    }

    /** Select the item after -- the window's `select_right` (`verb`). */
    selectRight(): boolean {
        return this.verb("select_right");
    }

    /** In note entry: the cursor back -- the window's `cursor_left` (`verb`). */
    cursorLeft(): boolean {
        return this.verb("cursor_left");
    }

    /** In note entry: the cursor forward -- the window's `cursor_right` (`verb`). */
    cursorRight(): boolean {
        return this.verb("cursor_right");
    }

    /** In note entry: the cursor to the bar before -- the window's `bar_left` (`verb`). */
    barLeft(): boolean {
        return this.verb("bar_left");
    }

    /** In note entry: the cursor to the bar after -- the window's `bar_right` (`verb`). */
    barRight(): boolean {
        return this.verb("bar_right");
    }

    /** In note entry: the cursor to the staff above -- the window's `staff_up` (`verb`). */
    staffUp(): boolean {
        return this.verb("staff_up");
    }

    /** In note entry: the cursor to the staff below -- the window's `staff_down` (`verb`). */
    staffDown(): boolean {
        return this.verb("staff_down");
    }

    /** In note entry: write a rest at the cursor -- the window's `enter_rest` (`verb`). */
    enterRest(): boolean {
        return this.verb("enter_rest");
    }

    /**
     * In note entry: **write the note `pitch`** (`"A"` to `"G"`) at the cursor,
     * nearest the note before -- the window's `pitch_a` ... `pitch_g`
     * (`verb`).
     */
    write(pitch: string): boolean {
        return this.verb(`pitch_${letter(pitch)}`);
    }

    /**
     * In note entry: **add `pitch`** (`"A"` to `"G"`) to the chord just written
     * -- the window's `chord_a` ... `chord_g` (`verb`).
     */
    chord(pitch: string): boolean {
        return this.verb(`chord_${letter(pitch)}`);
    }

    /**
     * In note entry: **write in voice `voice`** (1 to 4) from here on -- the
     * window's `voice_1` ... `voice_4` (`verb`).
     */
    entryVoice(voice: number): boolean {
        if (![1, 2, 3, 4].includes(voice)) throw new RangeError(`a voice is 1 to 4, not ${voice}`);
        return this.verb(`voice_${voice}`);
    }

    /** Remove the selected items; what follows them moves earlier. */
    delete(): Promise<boolean> {
        return this.#act({ action: "delete" });
    }

    /**
     * Move the selected items into the other voice of their staff, or into
     * voice `to` (from zero) when one is named, leaving rests where they were.
     */
    voice(to: number | null = null): Promise<boolean> {
        const call: Record<string, unknown> = { action: "voice" };
        if (to !== null) call.to = Math.trunc(to);
        return this.#act(call);
    }

    /**
     * Make the selected notes grace notes -- `"acc"`, an appoggiatura, or
     * `"unacc"`, an acciaccatura -- or notes of the bar again with none.
     */
    grace(kind: string | null = null): Promise<boolean> {
        return this.#act({ action: "grace", kind });
    }

    /**
     * Give the selected notes an accidental: `alter` semitones from the letter
     * (`1` a sharp, `-1` a flat, `0` a natural, `2` and `-2` the doubles),
     * printed whatever the key says.
     */
    accidental(alter: number): Promise<boolean> {
        return this.#act({ action: "accidental", alter: Math.trunc(alter) });
    }

    /**
     * Open `count` empty measures before the first selected measure, or after
     * the last with `after`; the music past them moves along.
     */
    insertMeasures(count = 1, options: { after?: boolean } = {}): Promise<boolean> {
        return this.#act({
            action: "measures",
            edit: options.after ? "insert_after" : "insert_before",
            count: Math.trunc(count),
        });
    }

    /** Take out the measures the selection covers, with what is written in them. */
    removeMeasures(): Promise<boolean> {
        return this.#act({ action: "measures", edit: "remove" });
    }

    /**
     * Give the last selected measure a right barline: `single`, `dbl`, `end`,
     * `rptstart`, `rptend`, `rptboth` or `invis`.
     */
    setBarline(kind: string): Promise<boolean> {
        return this.#act({ action: "barline", kind: String(kind) });
    }

    /**
     * Break the line or the page before the first selected measure (`system`,
     * `page`), or take the break back (`none`).
     */
    setBreak(kind: string): Promise<boolean> {
        return this.#act({ action: "break", kind: String(kind) });
    }

    /**
     * Change the meter from the first selected measure on: `count` beats of
     * `unit` (`setMeter(3, 4)` is three quarters).
     */
    setMeter(count: number, unit: number): Promise<boolean> {
        return this.#act({ action: "meter", count: Math.trunc(count), unit: Math.trunc(unit) });
    }

    /** A `slur`, a `crescendo` or a `diminuendo` from the first selected item to the last, in time. */
    spanner(kind: string): Promise<boolean> {
        return this.#act({ action: "spanner", kind: String(kind) });
    }

    /**
     * A mark on the selected notes, by its name: `tremolo` (strokes, 1 to 3),
     * `arpeggio` (`up`, `down`), `breath` (`breath`, `caesura`), `ring`
     * (`true`), `fingering` and `harmony` (text). A value every one of them has
     * already takes it away, and so does `null`.
     */
    mark(name: string, value: unknown = true): Promise<boolean> {
        return this.#act({ action: "mark", mark: String(name), value });
    }

    /**
     * A syllable of the lyrics under the first selected note, in `verse`; one
     * that ends in `-` runs on into the next, and an empty one takes it away.
     */
    lyric(text: string, verse = 1): Promise<boolean> {
        return this.#act({ action: "lyric", text: String(text), verse: Math.trunc(verse) });
    }

    /**
     * Draw the selected notes as repeats of the beat before each, which they
     * then hold -- or as themselves again.
     */
    beatRepeat(): Promise<boolean> {
        return this.#act({ action: "beat_repeat" });
    }

    /**
     * Write at the first selected item a `tempo` (with its speed `bpm`, in
     * quarter notes a minute), a `dir` or a `reh`; with no text and no speed,
     * take it back.
     */
    control(kind: string, text = "", bpm: number | null = null): Promise<boolean> {
        return this.#act({ action: "control", kind: String(kind), text: String(text), bpm });
    }

    /**
     * Change the key from the first selected measure on (`"D"`, `"Bb"`), or
     * take a change back with `"none"`.
     */
    setKey(key: string): Promise<boolean> {
        return this.#act({ action: "key", key: String(key) });
    }

    /**
     * Change the clef where the first selected item starts, on its staff
     * (`"G2"`, `"F4"`, `"C3"`), or take it back with `"none"`.
     */
    setClef(clef: string): Promise<boolean> {
        return this.#act({ action: "clef", clef: String(clef) });
    }

    /**
     * Mark the selected measures as an ending played in the passes `label`
     * names (`"1"`, `"2"`); empty takes it back.
     */
    setEnding(label = ""): Promise<boolean> {
        return this.#act({ action: "ending", label: String(label) });
    }

    /**
     * A navigation mark: `segno` and `coda` on the first selected measure,
     * `fine`, `dacapo`, `dalsegno` and `tocoda` on the last; `none` takes them
     * off both.
     */
    navigation(kind: string): Promise<boolean> {
        return this.#act({ action: "navigation", kind: String(kind) });
    }

    /**
     * Write each selected measure as a repeat of the one before, or as itself
     * again when every one already is.
     */
    measureRepeat(): Promise<boolean> {
        return this.#act({ action: "measure_repeat" });
    }

    /** Draw runs of empty measures as one numbered rest, or each as itself. */
    multirests(): Promise<boolean> {
        return this.#act({ action: "multirests" });
    }

    /**
     * Say what the selected staves are -- the first, with nothing selected:
     * their `lines`, their name (`label`, `abbr`), how many semitones they
     * sound from what they write. What is left out stays.
     */
    setStaff(
        options: { lines?: number; label?: string; abbr?: string; transpose?: number } = {},
    ): Promise<boolean> {
        return this.#act({ action: "staff", ...options });
    }

    /**
     * Group the staves the selection covers under a `brace`, a `bracket` or a
     * `line`; `none` takes away the groups over them.
     */
    group(symbol: string): Promise<boolean> {
        return this.#act({ action: "group", symbol: String(symbol) });
    }

    /**
     * A transformation over the measures the selection covers -- or over
     * everything, with nothing selected: `"transpose"` (`semitones`, or `steps`
     * for a diatonic one), `"invert"` (`axis`), `"retrograde"`, `"stretch"`
     * (`factor`, as `[n, d]`) or `"repeat"` (`count`).
     */
    transform(name: string, params: Record<string, unknown> = {}): Promise<boolean> {
        return this.#act({ action: "transform", name: String(name), ...params });
    }

    /**
     * A model operation, whole (the sheet vocabulary) -- for what has no verb
     * here -- as one entry of the history. (`apply` is every editor's door for
     * the host's messages.)
     */
    operate(op: Record<string, unknown>): Promise<boolean> {
        return this.#act({ action: "op", op });
    }

    // ---- playing it ----

    /** **The transport the page plays on**, as the object a page plays, in its beats. It is the window's, and goes with it. */
    get transport(): HostedTransport {
        return new HostedTransport(this);
    }

    /** A page has no time range of its own: what plays is a {@link ScoreEditor.play}'s `range`. */
    async showSpan(_span: readonly [number, number] | null): Promise<void> {
        throw new RangeError("a page has no time range: play({ range }) plays a stretch");
    }

    /** **Play the score** from `beat`, a quarter to the beat, on the window's transport. */
    async play(beat?: number, pass: { range?: readonly [number, number] | null; looping?: boolean } = {}): Promise<this> {
        const args: Record<string, unknown> = { beat: Number(beat ?? 0), looping: Boolean(pass.looping) };
        if (pass.range !== undefined && pass.range !== null) args.range = [Number(pass.range[0]), Number(pass.range[1])];
        await this.act("play", args);
        return this;
    }

    /** Pause where it stands: a `resume` carries the notes on. */
    async pause(): Promise<this> {
        await this.act("pause");
        return this;
    }

    /** Roll again from where it paused. */
    async resume(): Promise<this> {
        await this.act("resume");
        return this;
    }

    /** Stop, free what sounds, and go back to where the pass started. */
    async stop(): Promise<this> {
        await this.act("stop");
        return this;
    }

    /** Whether the score is sounding, as the engine answers. */
    async playing(): Promise<boolean> {
        if (this.closed) return false;
        return Boolean(await this.act("playing"));
    }

    // ---- the score's file ----

    /**
     * Write the score to its file -- `path`, which is then the score's, or the
     * one it was read from or last saved to -- and answer the path. The File
     * menu's Save, as a method. The application writes it, and what is written is
     * then not a change the File menu's Close asks about.
     */
    async save(path: string | null = null): Promise<string> {
        const answer = (await this.act("save", path === null ? {} : { path: String(path) })) ?? {};
        const written = String(answer.destination);
        this.#score.path = written;
        return written;
    }

    /** Whether the score has changes its file does not hold. */
    async unsaved(): Promise<boolean> {
        return (await this.coreCall("unsaved")).unsaved === true;
    }

    /**
     * Open the document in the file at `path` in this editor, in place of the
     * score, as one entry of the history: the score that was there is a step
     * back. The File menu's Open, as a method; the file is then the score's.
     */
    async load(path: string): Promise<boolean> {
        const loaded = Boolean(await this.act("load", { path: String(path) }));
        if (loaded) this.#score.path = path;
        await this.follow();
        return loaded;
    }

    /**
     * Write the score **rendered** to the file at `path` and answer the path:
     * a Standard MIDI File (`"smf"`) or a MIDI 2.0 Clip File (`"clip"`), by
     * `path`'s extension when left out -- `.midi2` is a clip, anything else a
     * MIDI file. The File menu's two Exports, as a method.
     */
    async export(path: string, format: "smf" | "clip" | null = null): Promise<string> {
        const kind = format ?? (path.toLowerCase().endsWith(".midi2") ? "clip" : "smf");
        if (kind !== "smf" && kind !== "clip") {
            throw new Error(`an export is 'smf' or 'clip', not '${String(kind)}'`);
        }
        const answer = await this.coreCall("render");
        if (answer.sequence === undefined) throw new Error(String(answer.error ?? "the score could not be rendered"));
        const rendered = EventSequence.fromData(answer.sequence);
        const data = kind === "clip" ? rendered.toClip() : rendered.toSmf();
        await writeFileAt(path, new Uint8Array(data) as Uint8Array<ArrayBuffer>);
        return path;
    }

    /**
     * One verb, in the application: recorded there, and the window corrected with
     * what it answers. Whether the score changed; why it did not is on the
     * window's status bar and in {@link ScoreEditor.refused}.
     */
    async #act(call: Record<string, unknown>): Promise<boolean> {
        const answer = (await this.act("act", { call: plain(call) })) ?? {};
        this.#refused = answer.refused ?? undefined;
        const changed = answer.changed === true;
        if (changed) await this.follow();
        return changed;
    }
}

/** Who has each score. */
const heldScores = new WeakMap<Score, Held>();

/** Whether `edit` opens this in the score editor: a symbolic score. */
export function isScore(structure: unknown): structure is Score {
    return structure instanceof Score;
}

/** A pitch's letter, as the note-entry verbs spell it: `"C"` is `c`. */
function letter(pitch: string): string {
    const l = pitch.trim().toLowerCase();
    if (!["a", "b", "c", "d", "e", "f", "g"].includes(l)) {
        throw new RangeError(`a pitch is a letter A to G, not ${JSON.stringify(pitch)}`);
    }
    return l;
}
