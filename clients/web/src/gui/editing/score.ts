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
 * The toolbar, the page in the scroll it sits in and the status line under it,
 * composed by the crate.
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
        ed.syncCore();
        const tree = ed.coreCall("window", {
            widget: page,
            scroll,
            status,
            tools,
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
 * empty staff writes a note of {@link ScoreEditor.value} there
 * ({@link ScoreEditor.entry}; off, it selects the measure). Ctrl+click adds a
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
        const { value, sampleRate: _rate, ...rest } = options;
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
     * Whether a press on empty staff writes a note. On by default; off, the
     * same press on a staff selects the measure it fell in. Set it to switch:
     * `editor.entry = false`.
     */
    get entry(): boolean {
        return this.coreCall("entry").entry !== false;
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
        }
        this.echo.send(outcome.answer);
        return changed;
    }
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
