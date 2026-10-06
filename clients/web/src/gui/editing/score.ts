/**
 * Editing a symbolic score on its engraved page: the score editor (mirrors
 * `clausters/gui/editing/score.py`).
 *
 * What it opens is a `Score` -- notation held as MEI, its model a sheet -- and it
 * edits that score **in place**: the editor in the shared crate (the
 * `openScore` member of `EditingCore`) holds the very score the page's handle
 * names, so every edit is read back through the handle (`Score.sheet`,
 * `Score.mei`) and there is nothing to write back.
 *
 * **The editor is the crate's**: the window, what each gesture on the page does
 * to the score, the verbs over what is selected, the entry each one leaves and
 * the corrections it answers with. What is here is what a language owns -- the
 * socket, and handing the crate the window it is open in. Each verb below is one
 * call into the crate, named as it names it.
 *
 * @module
 */

import { Score } from "../notation/engraver.ts";
import { readFileAt, writeFileAt } from "../../base/files.ts";
import { area } from "../../base/log.ts";
import type { Server } from "../../defs/server/index.ts";
import type { Transport } from "../../defs/server/transport.ts";
import { resolveServer } from "../../defs/wire.ts";
import { JsEventSequence } from "../../core/clausters_core_web.js";
import { NotesPlayback } from "../../seq/playback.ts";
import { EventSequence } from "../../seq/sequence.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import type { Answer } from "./echo.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { plain } from "./samples.ts";
import { View } from "./view.ts";

/** What one turn of the core came to. */
const log = area("gui.editing");

/** What a play asks of the playback, as the crate's turn says it. */
interface Pass {
    looping?: boolean;
    range?: [number, number] | null;
    from?: number;
}

interface Outcome {
    turn?: string;
    changed?: boolean;
    answer?: Answer;
    /** The file to write the score to, when the turn asked for a save. */
    save?: string;
    /** The file to open in place of the score, when the turn asked for one. */
    open?: string;
    /** The file to export the score's render to, and as what. */
    export?: { path: string; format: "smf" | "clip" };
    /** Whether to close the window: the File menu's Close. */
    close?: boolean;
    /** What a play asks of the playback: the space bar, the toolbar, the menu. */
    play?: Pass;
    /** What the loop switch asks of a pass in progress. */
    loop?: Pass;
    /** Where the position cursor was placed, as a beat of the score. */
    locate?: number;
}

/**
 * A score's vocabulary, the crate's `score`: a step is the page it names, which
 * the crate puts back on the score it shares, so there is nothing here to carry
 * out.
 */
export class ScoreDomain extends Domain<Score> {
    readonly name = "score";
    override readonly ingested = true;

    /** The crate reads the inverse off the score it holds. */
    current(_structure: Score, _payload: unknown): unknown {
        return null;
    }

    /** The crate put the step back on the score it shares with the page. */
    project(_structure: Score, _payload: unknown): boolean {
        return false;
    }
}

/**
 * The toolbar, the palettes beside the page in the scroll it sits in, the
 * status line under them and the dialogs a menu entry opens, composed by the
 * crate.
 */
export class ScoreView extends View<Score> {
    build(editor: Editor<Score>): GuiNode {
        const ed = editor as unknown as ScoreEditor;
        const page = this.widget(editor, "page", editor.structure);
        const scroll = this.widget(editor, "scroll", editor.structure);
        const status = this.widget(editor, "status", editor.structure);
        // the crate names the toolbar's tools and this numbers them
        const names = ed.coreCall("tools").tools;
        const tools: Record<string, number> = {};
        for (const name of Array.isArray(names) ? names.map(String) : []) {
            tools[name] = this.widget(editor, "tool", editor.structure, name);
        }
        // and the widgets of its dialogs, the same way
        const named = ed.coreCall("dialogs").dialogs;
        const dialogs: Record<string, number> = {};
        for (const name of Array.isArray(named) ? named.map(String) : []) {
            dialogs[name] = this.widget(editor, "dialog", editor.structure, name);
        }
        // and the entries of its palettes
        const entries = ed.coreCall("palettes").palettes;
        const palettes: Record<string, number> = {};
        for (const name of Array.isArray(entries) ? entries.map(String) : []) {
            palettes[name] = this.widget(editor, "palette", editor.structure, name);
        }
        ed.syncCore();
        const tree = ed.coreCall("window", {
            widget: page,
            scroll,
            status,
            tools,
            dialogs,
            palettes,
        }) as unknown as GuiNode;
        // **A page's own widgets are its objects**, so they are appended here
        // rather than composed in the crate.
        tree.children = [...(tree.children ?? []), ...editor.extra];
        return tree;
    }

    override props(editor: Editor<Score>, widgetId: number): Record<string, PropValue> {
        return (editor as unknown as ScoreEditor).coreCall("props", { widget: widgetId }) as Record<
            string,
            PropValue
        >;
    }
}

/** How a score editor is opened. */
export interface ScoreEditorOptions extends Omit<GenericEditorOptions<Score>, "sampleRate"> {
    /**
     * The written value a note entered on the page takes, as `[numerator,
     * denominator]` of a whole note; a quarter by default.
     */
    value?: readonly [number, number];
    /** Ignored: a page is engraved on beats, not on an engine's samples. */
    sampleRate?: number;
    /** The server it plays on; absent, the ambient one when it first plays. */
    server?: Server | null;
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
export class ScoreEditor extends Editor<Score> {
    /** This editor's member in its editing context. */
    private readonly member: number;

    constructor(score: Score, options: ScoreEditorOptions = {}) {
        const domain = new ScoreDomain();
        const { value, sampleRate: _rate, server = null, ...rest } = options;
        super(score, {
            title: "Score",
            width: 960,
            height: 640,
            ...rest,
            sampleRate: 48_000,
            domain,
            view: new ScoreView(),
        });
        const request: Record<string, unknown> = {
            title: this.title,
            w: this.size[0],
            h: this.size[1],
        };
        if (value !== undefined) request.value = [Math.trunc(value[0]), Math.trunc(value[1])];
        const opened = this.editing.openScore(`score:${keyOfScore(score)}`, score, request, domain);
        this.member = opened.member;
        this.structureId = opened.identity;
        this.#server = server;
    }

    // ---- playing it ----

    #server: Server | null = null;
    /**
     * The score as the sequence it plays as, rendered when it first plays and
     * again after every edit.
     */
    #rendered: EventSequence | null = null;
    /** The playback work under way: a turn chains on it and does not wait. */
    #work: Promise<unknown> = Promise.resolve();

    /**
     * Opens the window, with its play cursor drawn from the transport.
     *
     * The page draws its cursor over the engraver's timemap, anchored at 0,
     * and the counter that makes that the score's own time is the position of
     * the transport the score plays on -- stopped or rolling, the line is
     * where the sound is. With no server to play on there is no position, and
     * the page opens to be edited; nor with a server that has no transport
     * left, which is logged.
     */
    override async open(
        host?: Parameters<Editor<Score>["open"]>[0],
        options: Parameters<Editor<Score>["open"]>[1] = {},
    ): ReturnType<Editor<Score>["open"]> {
        const handle = await super.open(host, options);
        let server: Server;
        try {
            server = this.#resolveServer();
        } catch {
            return handle;
        }
        let transport: number;
        try {
            transport = NotesPlayback.of(server, this.#sequence()).transportId;
        } catch (refused) {
            log.warning("the score has no play cursor and cannot be played: %s", String(refused));
            return handle;
        }
        this.host?.headClock(handle, "transport", transport);
        return handle;
    }

    #resolveServer(): Server {
        this.#server ??= resolveServer(null) as unknown as Server;
        return this.#server;
    }

    /**
     * The score as the sequence it plays as: the crate's render, on the
     * engraver's time, made on first ask and kept -- so its playback is one,
     * and an edit is the same sequence holding the next render.
     */
    #sequence(): EventSequence {
        this.#rendered ??= EventSequence.fromData(this.#render());
        return this.#rendered;
    }

    #render(): unknown {
        const answer = this.coreCall("render");
        if (answer.sequence === undefined) {
            throw new Error(String(answer.error ?? "the score could not be rendered"));
        }
        return answer.sequence;
    }

    /**
     * The score changed: the sequence it plays as holds the next render, and
     * the lane takes it, so the server plays the edit on from where the
     * position is.
     */
    #update(): void {
        const rendered = this.#rendered;
        if (rendered === null) return;
        try {
            (rendered as unknown as { seq: JsEventSequence }).seq = new JsEventSequence(
                JSON.stringify(this.#render()),
            );
        } catch {
            return;
        }
        const playback = this.#held;
        if (playback === null) return;
        this.#work = this.#work.then(() => playback.update());
        this.#work.catch(() => {});
    }

    /**
     * The score's playback on the server, made when it has none -- which
     * throws when the server has no transport left.
     */
    get #playback(): NotesPlayback {
        return NotesPlayback.of(this.#resolveServer(), this.#sequence());
    }

    /** The score's playback when it has one. */
    get #held(): NotesPlayback | null {
        if (this.#rendered === null) return null;
        return NotesPlayback.held(this.#server, this.#rendered);
    }

    /**
     * **The transport the score plays on**, as the object a page plays: a
     * `Transport` whose verbs (`play`, `pause`, `stop`, `locate`, `loop`,
     * `wait`) are about this score, in its beats -- a quarter to the beat.
     * Once a page has asked for it, it is the page's to free (`free()`);
     * otherwise it goes back to the server when the editor closes.
     */
    get transport(): Transport {
        const playback = this.#playback;
        playback.kept = true;
        return playback.transport;
    }

    /**
     * **Plays the score** from `beat` -- a quarter to the beat; from the start
     * when left out -- on a transport of its own, with the page's cursor
     * following.
     *
     * `range` -- `[start, end]` in beats -- plays that stretch, going back to
     * `beat`; `looping` loops it, or with none the whole score. The space bar,
     * the toolbar and the Play menu ask the same, from where the selection
     * starts. An edit made while it plays is heard on from where the position
     * is.
     */
    async play(
        beat?: number,
        pass: { range?: readonly [number, number] | null; looping?: boolean } = {},
    ): Promise<this> {
        await this.#playback.load(beat ?? 0, {
            range: pass.range ?? null,
            looping: pass.looping ?? false,
            end: "contents",
        });
        return this;
    }

    /** Pauses where it stands: a `resume` carries the notes on. */
    async pause(): Promise<this> {
        await this.#playback.call("pause");
        return this;
    }

    /** Rolls again from where it paused. */
    async resume(): Promise<this> {
        await this.#playback.call("resume");
        return this;
    }

    /** Stops, frees what sounds, and goes back to where the pass started. */
    async stop(): Promise<this> {
        const playback = this.#held;
        if (playback !== null) await playback.call("stop", { back: playback.cursor });
        return this;
    }

    /**
     * Whether the score is sounding, as the engine answers. A method here, the
     * reference client's property: asking the engine is a round trip, and a
     * page awaits one.
     */
    async playing(): Promise<boolean> {
        const playback = this.#held;
        return playback !== null && (await playback.playing());
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
     * score again, so the undo is heard.
     */
    override reflectStep(): void {
        super.reflectStep();
        this.#update();
    }

    protected override closedWindow(): boolean {
        const closed = super.closedWindow();
        this.#release();
        return closed;
    }

    /**
     * Closes this editor's window. **The score's transport goes back to the
     * server with it**, and what was sounding is released -- unless a page
     * holds the transport ({@link ScoreEditor.transport}), whose it then is to
     * free.
     */
    override close(): this {
        super.close();
        this.#release();
        return this;
    }

    #release(): void {
        const playback = this.#held;
        if (playback === null || playback.kept) return;
        this.#work = this.#work.then(() => playback.free());
        this.#work.catch(() => {});
    }

    /**
     * What a turn asked of the playback: a play or a stop, the loop switch,
     * the cursor placed.
     */
    #transportTurn(outcome: Outcome): void {
        const located = outcome.locate;
        if (located !== undefined) {
            const playback = this.#held;
            if (playback !== null) {
                this.#work = this.#work.then(() => playback.cue(located));
                this.#work.catch(() => {});
            }
        }
        const pass = outcome.play;
        if (pass !== undefined) {
            // A play is play or stop: a stop goes back to where the pass began.
            this.#work = this.#work.then(async () => {
                if (await this.playing()) {
                    await this.stop();
                    return;
                }
                const from = pass.from ?? 0;
                this.#playback.cursor = from;
                await this.play(from, { range: pass.range ?? null, looping: pass.looping ?? false });
            });
            this.#work.catch(() => {});
        }
        const relooped = outcome.loop;
        const playback = this.#held;
        if (relooped !== undefined && playback !== null) {
            // the loop switch: followed at once by a pass in progress
            this.#work = this.#work.then(async () => {
                await playback.setSpan(relooped.range ?? null, { show: false });
                await playback.setLooping(relooped.looping ?? false);
            });
            this.#work.catch(() => {});
        }
    }

    /** The score the page edits -- the one the editor was opened over. */
    get score(): Score {
        return this.structure;
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
            title: this.title,
            w: this.size[0],
            h: this.size[1],
            path: this.score.path,
        });
    }

    // ---- what is selected, and the value in hand ----

    /** The selected items, as the model names them (`Score.sheet`'s item ids), each once. */
    get selected(): number[] {
        const items = this.coreCall("selected").items;
        return Array.isArray(items) ? items.map(Number) : [];
    }

    /**
     * Select the page's elements `elements` (their `xml:id`s, as the page
     * reports them), or nothing with an empty list.
     */
    select(elements: Iterable<string>): void {
        this.coreCall("select", { elements: [...elements].map(String) });
        this.adopt();
    }

    /**
     * The written value a note entered on the page takes, as `[numerator,
     * denominator]` of a whole note: `[1, 4]` is a quarter. Set it to write
     * another: `editor.value = [1, 8]`.
     */
    get value(): [number, number] {
        const value = this.coreCall("value").value;
        return Array.isArray(value) ? [Number(value[0]), Number(value[1])] : [1, 4];
    }

    set value(value: readonly [number, number]) {
        this.coreCall("sync", { value: [Math.trunc(value[0]), Math.trunc(value[1])] });
    }

    /**
     * Whether the window is in note entry, where the keys and a press on a
     * staff write at the edit cursor. Off by default, and off once a pass
     * plays; outside it a press on a staff selects the measure it fell in. Set
     * it to switch: `editor.entry = true`.
     */
    get entry(): boolean {
        return this.coreCall("entry").entry === true;
    }

    set entry(on: boolean) {
        this.coreCall("sync", { entry: Boolean(on) });
        this.adopt();
    }

    /**
     * Whether the value a note is entered with is dotted: half as long again.
     * Set it to switch: `editor.dotted = true`.
     */
    get dotted(): boolean {
        return this.coreCall("input").dotted === true;
    }

    set dotted(on: boolean) {
        this.coreCall("sync", { dotted: Boolean(on) });
        this.adopt();
    }

    /**
     * Whether a press on empty staff writes a rest of `value` rather than a
     * note. Set it to switch: `editor.rest = true`.
     */
    get rest(): boolean {
        return this.coreCall("input").rest === true;
    }

    set rest(on: boolean) {
        this.coreCall("sync", { rest: Boolean(on) });
        this.adopt();
    }

    /**
     * The accidental the next note entered takes, in semitones from its letter
     * (`1` a sharp, `-1` a flat, `0` a natural), or `null`. It is for that one
     * note: writing it lets the accidental go. Set it to arm one:
     * `editor.nextAccidental = 1`. (`accidental` is the verb over what is
     * selected.)
     */
    get nextAccidental(): number | null {
        const armed = this.coreCall("input").accidental;
        return typeof armed === "number" ? armed : null;
    }

    set nextAccidental(alter: number | null) {
        this.coreCall("sync", { accidental: alter === null ? null : Math.trunc(alter) });
        this.adopt();
    }

    // ---- the layout, which is the window's, and the page, the document's ----

    /**
     * How the window looks at the score: `"page"`, every page of the paper one
     * under another, fixed whatever the window's size; or `"continuous"`, one
     * system as long as the music, with no page. Set it to switch:
     * `editor.layout = "continuous"`. It is the window's, not the score's, and
     * enters no history.
     */
    get layout(): "page" | "continuous" {
        return this.coreCall("layout").layout === "continuous" ? "continuous" : "page";
    }

    set layout(layout: "page" | "continuous") {
        this.coreCall("sync", { layout: String(layout) });
        this.adopt();
    }

    /**
     * The page the score is laid out on: the setup itself (`width`, `height`
     * and `margins` in tenths of a millimetre, `staff` in hundredths), the name
     * of its paper when it is a known one, which way up it is, and the names of
     * the papers there are. Change it with {@link ScoreEditor.setPage}.
     */
    get page(): PageInfo {
        return this.coreCall("page") as unknown as PageInfo;
    }

    /**
     * Lay the score out on another page, as one entry of the history: a `paper`
     * by name (`"A4"`, `"Letter"`, `"Octavo"` ... -- see
     * {@link ScoreEditor.page}), turned with `landscape`, or a `width` and
     * `height` of its own; the `margins` (top, right, bottom, left) and the
     * `staff` height. Lengths are in tenths of a millimetre and the staff in
     * hundredths (`720` is 7.2 mm). What is left out stays as it is. The setup
     * is the score's, and travels in its MEI.
     */
    setPage(paper: string | null = null, options: PageOptions = {}): boolean {
        const call: Record<string, unknown> = { action: "page" };
        if (paper !== null) call.paper = paper;
        for (const key of ["landscape", "width", "height", "staff"] as const) {
            if (options[key] !== undefined) call[key] = options[key];
        }
        if (options.margins !== undefined) call.margins = options.margins.map(Math.trunc);
        return this.#act(call);
    }

    /**
     * Write a text of the page, or move it, as one entry of the history.
     *
     * `field` is `"title"`, `"subtitle"`, `"composer"`, `"arranger"`,
     * `"lyricist"`, `"translator"`, `"copyright"` or `"note"` -- a footnote:
     * `index` says which, from zero, and none adds one. `text` writes it, and
     * an empty one takes it away. `region` (`"head"`, `"foot"`), `halign`
     * (`"left"`, `"center"`, `"right"`), `valign` (`"top"`, `"middle"`,
     * `"bottom"`) and `pages` (`"first"`, `"all"`) put it in a cell of the
     * page's head or foot; what is left out stays as it is, and a field nobody
     * moved sits where the printed page puts it. A press on a text names its
     * field on the status line.
     */
    setText(field: string, text: string | null = null, options: TextOptions = {}): boolean {
        const call: Record<string, unknown> = { action: "text", field: String(field) };
        if (text !== null) call.text = text;
        for (const key of ["index", "region", "halign", "valign", "pages"] as const) {
            if (options[key] !== undefined) call[key] = options[key];
        }
        return this.#act(call);
    }

    // ---- the verbs, over what is selected ----

    /**
     * Move the selected notes `steps` diatonic steps along their staves, up
     * when positive -- each takes the key signature's alteration for the letter
     * it lands on.
     */
    move(steps: number): boolean {
        return this.#act({ action: "move", steps: Math.trunc(steps) });
    }

    /**
     * Scale the selected items' written values by `numerator / denominator`
     * (`scale(2, 1)` is twice as long), against the barlines already there.
     */
    scale(numerator: number, denominator: number): boolean {
        return this.#act({
            action: "scale",
            factor: [Math.trunc(numerator), Math.trunc(denominator)],
        });
    }

    /**
     * Give the selected notes an articulation (by its MEI name: `stacc`, `acc`,
     * `ten`, `marc`...), or take it away when all of them have it.
     */
    articulation(name: string): boolean {
        return this.#act({ action: "articulation", name: String(name) });
    }

    /** Put a dynamic (`pp` ... `ff`) under the first selected note, or take it away with none. */
    dynamic(name: string | null = null): boolean {
        return this.#act({ action: "dynamic", name });
    }

    /**
     * Give the selected notes an ornament (`trill`, `mordent`, `turn`,
     * `fermata`), or take it away with none.
     */
    ornament(name: string | null = null): boolean {
        return this.#act({ action: "ornament", name });
    }

    /** Take every mark off the selected notes. */
    clearMarks(): boolean {
        return this.#act({ action: "clear_marks" });
    }

    /** Tie the selected notes to the next, or untie them when the first is tied already. */
    tie(): boolean {
        return this.#act({ action: "tie" });
    }

    /** Turn the selected notes into rests of the same length. */
    silence(): boolean {
        return this.#act({ action: "silence" });
    }

    /** Remove the selected items; what follows them moves earlier. */
    delete(): boolean {
        return this.#act({ action: "delete" });
    }

    /**
     * Move the selected items into the other voice of their staff, or into
     * voice `to` (from zero) when one is named, leaving rests where they were.
     */
    voice(to: number | null = null): boolean {
        const call: Record<string, unknown> = { action: "voice" };
        if (to !== null) call.to = Math.trunc(to);
        return this.#act(call);
    }

    /**
     * Make the selected notes grace notes -- `"acc"`, an appoggiatura, or
     * `"unacc"`, an acciaccatura -- or notes of the bar again with none.
     */
    grace(kind: string | null = null): boolean {
        return this.#act({ action: "grace", kind });
    }

    /**
     * Give the selected notes an accidental: `alter` semitones from the letter
     * (`1` a sharp, `-1` a flat, `0` a natural, `2` and `-2` the doubles),
     * printed whatever the key says.
     */
    accidental(alter: number): boolean {
        return this.#act({ action: "accidental", alter: Math.trunc(alter) });
    }

    /**
     * Open `count` empty measures before the first selected measure, or after
     * the last with `after`; the music past them moves along.
     */
    insertMeasures(count = 1, options: { after?: boolean } = {}): boolean {
        return this.#act({
            action: "measures",
            edit: options.after ? "insert_after" : "insert_before",
            count: Math.trunc(count),
        });
    }

    /** Take out the measures the selection covers, with what is written in them. */
    removeMeasures(): boolean {
        return this.#act({ action: "measures", edit: "remove" });
    }

    /**
     * Give the last selected measure a right barline: `single`, `dbl`, `end`,
     * `rptstart`, `rptend`, `rptboth` or `invis`.
     */
    setBarline(kind: string): boolean {
        return this.#act({ action: "barline", kind: String(kind) });
    }

    /**
     * Break the line or the page before the first selected measure (`system`,
     * `page`), or take the break back (`none`).
     */
    setBreak(kind: string): boolean {
        return this.#act({ action: "break", kind: String(kind) });
    }

    /**
     * Change the meter from the first selected measure on: `count` beats of
     * `unit` (`setMeter(3, 4)` is three quarters).
     */
    setMeter(count: number, unit: number): boolean {
        return this.#act({ action: "meter", count: Math.trunc(count), unit: Math.trunc(unit) });
    }

    /** A `slur`, a `crescendo` or a `diminuendo` from the first selected item to the last, in time. */
    spanner(kind: string): boolean {
        return this.#act({ action: "spanner", kind: String(kind) });
    }

    /**
     * A mark on the selected notes, by its name: `tremolo` (strokes, 1 to 3),
     * `arpeggio` (`up`, `down`), `breath` (`breath`, `caesura`), `ring`
     * (`true`), `fingering` and `harmony` (text). A value every one of them has
     * already takes it away, and so does `null`.
     */
    mark(name: string, value: unknown = true): boolean {
        return this.#act({ action: "mark", mark: String(name), value });
    }

    /**
     * A syllable of the lyrics under the first selected note, in `verse`; one
     * that ends in `-` runs on into the next, and an empty one takes it away.
     */
    lyric(text: string, verse = 1): boolean {
        return this.#act({ action: "lyric", text: String(text), verse: Math.trunc(verse) });
    }

    /**
     * Draw the selected notes as repeats of the beat before each, which they
     * then hold -- or as themselves again.
     */
    beatRepeat(): boolean {
        return this.#act({ action: "beat_repeat" });
    }

    /**
     * Write at the first selected item a `tempo` (with its speed `bpm`, in
     * quarter notes a minute), a `dir` or a `reh`; with no text and no speed,
     * take it back.
     */
    control(kind: string, text = "", bpm: number | null = null): boolean {
        return this.#act({ action: "control", kind: String(kind), text: String(text), bpm });
    }

    /**
     * Change the key from the first selected measure on (`"D"`, `"Bb"`), or
     * take a change back with `"none"`.
     */
    setKey(key: string): boolean {
        return this.#act({ action: "key", key: String(key) });
    }

    /**
     * Change the clef where the first selected item starts, on its staff
     * (`"G2"`, `"F4"`, `"C3"`), or take it back with `"none"`.
     */
    setClef(clef: string): boolean {
        return this.#act({ action: "clef", clef: String(clef) });
    }

    /**
     * Mark the selected measures as an ending played in the passes `label`
     * names (`"1"`, `"2"`); empty takes it back.
     */
    setEnding(label = ""): boolean {
        return this.#act({ action: "ending", label: String(label) });
    }

    /**
     * A navigation mark: `segno` and `coda` on the first selected measure,
     * `fine`, `dacapo`, `dalsegno` and `tocoda` on the last; `none` takes them
     * off both.
     */
    navigation(kind: string): boolean {
        return this.#act({ action: "navigation", kind: String(kind) });
    }

    /**
     * Write each selected measure as a repeat of the one before, or as itself
     * again when every one already is.
     */
    measureRepeat(): boolean {
        return this.#act({ action: "measure_repeat" });
    }

    /** Draw runs of empty measures as one numbered rest, or each as itself. */
    multirests(): boolean {
        return this.#act({ action: "multirests" });
    }

    /**
     * Say what the selected staves are -- the first, with nothing selected:
     * their `lines`, their name (`label`, `abbr`), how many semitones they
     * sound from what they write. What is left out stays.
     */
    setStaff(
        options: { lines?: number; label?: string; abbr?: string; transpose?: number } = {},
    ): boolean {
        return this.#act({ action: "staff", ...options });
    }

    /**
     * Group the staves the selection covers under a `brace`, a `bracket` or a
     * `line`; `none` takes away the groups over them.
     */
    group(symbol: string): boolean {
        return this.#act({ action: "group", symbol: String(symbol) });
    }

    /**
     * A transformation over the measures the selection covers -- or over
     * everything, with nothing selected: `"transpose"` (`semitones`, or `steps`
     * for a diatonic one), `"invert"` (`axis`), `"retrograde"`, `"stretch"`
     * (`factor`, as `[n, d]`) or `"repeat"` (`count`).
     */
    transform(name: string, params: Record<string, unknown> = {}): boolean {
        return this.#act({ action: "transform", name: String(name), ...params });
    }

    /**
     * A model operation, whole (the sheet vocabulary) -- for what has no verb
     * here -- as one entry of the history. (`apply` is every editor's door for
     * the host's messages.)
     */
    operate(op: Record<string, unknown>): boolean {
        return this.#act({ action: "op", op });
    }

    // ---- the score's file ----

    /**
     * Write the score to its file -- `path`, which is then the score's, or the
     * one it was read from or last saved to -- and answer the path. The File
     * menu's Save, as a method (`Score.write`). What is written is then not a
     * change the File menu's Close asks about.
     */
    async save(path: string | null = null): Promise<string> {
        const written = await this.score.write(path);
        this.coreCall("saved");
        return written;
    }

    /**
     * Whether the score has changes its file does not hold -- what the File
     * menu's Close asks about before it closes the window.
     */
    get unsaved(): boolean {
        return this.coreCall("unsaved").unsaved === true;
    }

    /**
     * Open the document in the file at `path` in this editor, in place of the
     * score, as one entry of the history: the score that was there is a step
     * back. The File menu's Open, as a method; the file is then the score's.
     */
    async load(path: string): Promise<boolean> {
        const data = new TextDecoder().decode(await readFileAt(path));
        const loaded = this.#act({ action: "open", data });
        if (loaded) this.score.path = path;
        return loaded;
    }

    /**
     * Write the score **rendered** to the file at `path` and answer the path:
     * a Standard MIDI File (`"smf"`) or a MIDI 2.0 Clip File (`"clip"`), by
     * `path`'s extension when left out -- `.midi2` is a clip, anything else a
     * MIDI file. The File menu's two Exports, as a method.
     *
     * It is the sequence `Score.renderEvents` answers, at the engraver's
     * tempo, written as a sequence writes either (`EventSequence.toSmf`,
     * `toClip`): its notes, a channel to a voice, the dynamics as each
     * channel's expression.
     */
    async export(path: string, format: "smf" | "clip" | null = null): Promise<string> {
        const kind = format ?? (path.toLowerCase().endsWith(".midi2") ? "clip" : "smf");
        if (kind !== "smf" && kind !== "clip") {
            throw new Error(`an export is 'smf' or 'clip', not '${String(kind)}'`);
        }
        const rendered = EventSequence.fromData(this.#render());
        const data = kind === "clip" ? rendered.toClip() : rendered.toSmf();
        await writeFileAt(path, new Uint8Array(data) as Uint8Array<ArrayBuffer>);
        return path;
    }

    /**
     * One verb, through the context: recorded by the crate, and the window
     * corrected with what it answers. Whether the score changed; why it did not
     * is on the window's status bar.
     */
    #act(call: Record<string, unknown>): boolean {
        const turned = this.editing.act(this.member, plain(call) as Record<string, unknown>);
        const outcome = (turned.outcome ?? {}) as Outcome;
        const changed = outcome.changed === true;
        if (changed) {
            this.dirty = true;
            this.editing.changed();
            this.#update();
        }
        if (this.windowId !== null) this.echo.send(outcome.answer);
        return changed;
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
        return this.#take(outcome);
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
        return this.#take((turned.outcome ?? {}) as Outcome);
    }

    /** Answers the host with what a turn came to; whether the score changed. */
    #take(outcome: Outcome): boolean {
        if (outcome.turn === undefined || outcome.turn === "nothing") return false;
        const changed = outcome.changed === true;
        if (changed) {
            this.dirty = true;
            this.editing.changed();
            this.#update();
        }
        this.#transportTurn(outcome);
        this.echo.send(outcome.answer);
        // A file is this client's to write and to read: the turn said which.
        // Both are the page's own storage in a tab, and neither is waited for
        // by the turn that asked; what went wrong is said on the console.
        // The File menu's Close waits for the file it saves to, and keeps the
        // window when that file was not written.
        if (outcome.save) {
            this.filed = this.save(outcome.save).then(
                () => {
                    if (outcome.close === true) this.close();
                },
                (error: unknown) => console.warn(`save: ${outcome.save}:`, error),
            );
        } else if (outcome.close === true) {
            this.close();
            return changed;
        }
        if (outcome.export) {
            const { path, format } = outcome.export;
            this.filed = this.export(path, format).then(
                () => undefined,
                (error: unknown) => console.warn(`export: ${path}:`, error),
            );
        }
        if (outcome.open) {
            const path = outcome.open;
            this.filed = this.load(path).then(
                () => undefined,
                (error: unknown) => console.warn(`open: ${path}:`, error),
            );
        }
        return changed;
    }

    /**
     * The last file the window's menu wrote or read, as it settles: what a
     * caller awaits to know a Save or an Open it did not call has finished.
     */
    filed: Promise<void> = Promise.resolve();
}

/** A key per score, so two editors over one score are one structure in the order. */
const keys = new WeakMap<Score, number>();
let nextKey = 0;

function keyOfScore(score: Score): number {
    let key = keys.get(score);
    if (key === undefined) {
        key = ++nextKey;
        keys.set(score, key);
    }
    return key;
}

/** Whether `edit` opens this in the score editor: a symbolic score. */
export function isScore(structure: unknown): structure is Score {
    return structure instanceof Score;
}
