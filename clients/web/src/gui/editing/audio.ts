/**
 * Editing a take as **a list of parts over immutable takes**: the audio editor.
 *
 * {@link SamplesEditor} writes a stroke into the buffer it draws. This one never
 * writes a buffer that already exists. The window draws a **join** -- a buffer
 * made of spans of other buffers (`Buffer.stitch`) -- and every edit leaves a
 * new list of those spans: a cut takes a span out, a paste puts a new take in,
 * a mix adds the block onto a new take over the frames it lands on, and a
 * pencil stroke writes **a new take the size of the stroke** and splices it over
 * the frames it was drawn on. Nothing a history entry names is ever written
 * again, so an undo is the list before, stitched: it costs the list and not the
 * samples.
 *
 * **The editor is the shared crate's** (`EditingCore`, the `openAudio`
 * member): what each gesture does to the list, the steps that make a new take
 * and stitch the join again, and which takes the history can no longer reach.
 * What is here is what a language owns: the server the steps are walked
 * against, the buffer numbers the crate is handed for new takes (from this
 * page's allocator, topped up before every turn), and freeing the takes the
 * context hands back.
 *
 * **Memory is spent on what the history holds.** Every take a stroke or a
 * paste made is kept while an undo or a redo can still reach it, and freed
 * when neither can. `historyBytes` caps what only the history holds; past it
 * the oldest entries go first.
 *
 * @module
 */

import { PARTS } from "../../document.ts";
import type { Selection } from "../../document.ts";
import type { Answer } from "./echo.ts";
import { Buffer } from "../../defs/buffer.ts";
import { StepRunner } from "../../core/clausters_core_web.js";
import { resolveServer } from "../../defs/wire.ts";
import { runSteps } from "../../steps.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { MEASURES, SamplesView, plain } from "./samples.ts";
import type { Measure } from "./samples.ts";

/**
 * How many buffers the crate holds for new takes before a turn: a stroke or a
 * paste takes one, a mix two.
 */
const SPARE = 2;

/** What one turn of the core came to. */
interface Outcome {
    turn?: string;
    answer?: Answer;
    changed?: boolean;
    version?: number;
    steps?: unknown[];
    locate?: number;
    selection?: unknown;
}

/**
 * A take made of parts: the crate's `parts` vocabulary.
 *
 * **What a gesture does is the crate's**; what is left is carrying out the
 * steps it answers, on the take's server, and freeing the takes nothing reaches
 * any more.
 */
export class AudioDomain extends Domain<Buffer> {
    override readonly name = PARTS;
    override readonly ingested = true;

    /** The editor whose take this is. */
    editor: AudioEditor | null = null;

    /**
     * The work in flight, chained: a page's server calls are asynchronous, and
     * a turn's steps must land before the next turn's, and before the takes
     * they stop reading are freed.
     */
    #work: Promise<void> = Promise.resolve();

    /** Nothing: an edit's inverse is the list before, which the core holds. */
    current(_structure: Buffer, _payload: unknown): unknown {
        return null;
    }

    /** Nothing: the parts vocabulary is carried out as steps ({@link run}). */
    project(_structure: Buffer, _payload: unknown): boolean {
        return false;
    }

    /** Walk `steps` against the take's server. Answers whether there were any. */
    run(_structure: Buffer, steps: unknown[]): boolean {
        const editor = this.editor;
        if (editor === null || steps.length === 0) return false;
        this.#queue(() => runSteps(editor.server, editor.runner, steps as never));
        return true;
    }

    /** Free takes the context handed back: nothing reaches them any more. */
    free(_structure: Buffer, buffers: number[]): void {
        const editor = this.editor;
        if (editor === null || buffers.length === 0) return;
        this.#queue(async () => {
            for (const bufnum of buffers) {
                editor.server.sendMsg("/buffer_free", ["i", Math.trunc(bufnum)]);
                editor.server.buffers.free(Math.trunc(bufnum));
            }
        });
    }

    /**
     * The work queued so far, landed.
     *
     * @internal
     */
    idle(): Promise<void> {
        return this.#work;
    }

    /**
     * Run `then` once the work queued so far has landed: what a turn's answer
     * waits for, since it asks the window to read a join the steps replace.
     */
    after(then: () => void): void {
        this.#queue(async () => then());
    }

    /** One piece of work after the last, and a failure reported rather than swallowed. */
    #queue(run: () => Promise<void>): void {
        this.#work = this.#work.then(run).catch((error: unknown) => {
            queueMicrotask(() => {
                throw error;
            });
        });
    }
}

/**
 * A take on screen, edited as a list of parts over immutable takes.
 *
 * The window draws {@link AudioEditor.buffer}, a join the editor owns: the take
 * as the edits have left it. Play that buffer to hear the edited take, and read
 * {@link AudioEditor.parts} for what it is made of.
 */
export class AudioEditor extends Editor<Buffer> {
    /** This editor's member in its editing context. */
    private readonly member: number;

    /**
     * The server the take is on, and every take this editor makes.
     *
     * @internal
     */
    readonly server: ReturnType<typeof resolveServer>;

    /**
     * The runner the steps are walked through.
     *
     * @internal
     */
    readonly runner = new StepRunner();

    /** The join the window draws. */
    private readonly display: number;

    constructor(take: Buffer, options: AudioEditorOptions) {
        const view = new SamplesView(options.layers ?? MEASURES);
        const domain = new AudioDomain();
        super(take, {
            title: "Audio",
            ...options,
            sampleRate: Number(options.sampleRate || take.sampleRate || 48_000),
            domain,
            view,
        });
        domain.editor = this;
        this.server = resolveServer(take.server);
        this.display = this.server.buffers.alloc();
        const opened = this.editing.open(
            "openAudio",
            `audio:${Math.trunc(take.bufnum)}`,
            {
                ...this.facts(),
                frames: Math.trunc(take.frames || 0),
                take: Math.trunc(take.bufnum),
                display: this.display,
                layers: view.layers,
            },
            take,
            domain,
        );
        this.member = opened.member;
        this.structureId = opened.identity;
        if (options.historyBytes !== undefined && options.historyBytes !== null) {
            this.editing.limitBytes(options.historyBytes);
        }
        this.topUp();
        domain.run(take, (this.coreCall("open").steps as unknown[] | undefined) ?? []);
    }

    /**
     * Opens the window once the join it draws has been stitched -- the steps
     * the constructor queued -- so the first picture is the take and not an
     * empty buffer.
     */
    override async open(
        host?: Parameters<Editor<Buffer>["open"]>[0],
        options: Parameters<Editor<Buffer>["open"]>[1] = {},
    ): ReturnType<Editor<Buffer>["open"]> {
        await (this.domain as AudioDomain).idle();
        return super.open(host, options);
    }

    /** What this page holds about the take and the window. */
    private facts(): Record<string, unknown> {
        const take = this.structure;
        const name = (take as { name?: string }).name;
        return {
            channels: Math.max(1, Math.trunc(take.channels || 1)),
            name: typeof name === "string" && name ? name : null,
            rate: this.sampleRate,
            title: this.title,
            w: this.size[0],
            h: this.size[1],
            window: this.windowId,
        };
    }

    /**
     * One verb of the core, with its arguments; the answer, parsed.
     *
     * @internal
     */
    coreCall(verb: string, args: Record<string, unknown> = {}): Record<string, unknown> {
        return this.editing.member(this.member, verb, args);
    }

    /**
     * Hand the core the chrome, the window it is open in, and enough buffers
     * for the next turn.
     *
     * @internal
     */
    syncCore(): void {
        this.topUp();
    }

    private topUp(): void {
        const spare = Number(this.coreCall("sync", this.facts()).spare ?? 0);
        if (spare < SPARE) {
            const more: number[] = [];
            for (let i = spare; i < SPARE; i++) more.push(this.server.buffers.alloc());
            this.coreCall("sync", { buffers: more });
        }
    }

    /**
     * The join the window draws, as a {@link Buffer}: the take as the edits
     * have left it. It changes under this handle as the take is edited; read it
     * again rather than caching what it held.
     */
    get buffer(): Buffer {
        const frames = Number(this.coreCall("parts").frames ?? 0);
        return new Buffer(
            this.display,
            frames,
            Math.max(1, Math.trunc(this.structure.channels || 1)),
            this.sampleRate,
            this.server,
        );
    }

    /**
     * What the take is made of now: one object per span, in reading order, each
     * naming the buffer it reads and the frames of it.
     */
    get parts(): Record<string, unknown>[] {
        return [...((this.coreCall("parts").parts as Record<string, unknown>[] | undefined) ?? [])];
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
            this.editing.release(turned.freed);
            return stepped;
        }
        const changed = this.take(outcome);
        this.editing.release(turned.freed);
        return changed;
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
        const changed = this.take((turned.outcome ?? {}) as Outcome);
        this.editing.release(turned.freed);
        return changed;
    }

    /**
     * Carry out what a turn came to, and answer the host. Answers whether the
     * take changed.
     */
    private take(outcome: Outcome): boolean {
        if (outcome.turn === undefined || outcome.turn === "nothing") return false;
        const changed = outcome.changed === true;
        if (changed) {
            // **The steps are this page's to carry out**: a new take made where
            // the turn needed one, then the join stitched over the list.
            (this.domain as AudioDomain).run(this.structure, outcome.steps ?? []);
            this.dirty = true;
            this.editing.changed();
        }
        if (outcome.locate !== undefined) {
            this.cursor = outcome.locate;
            this.locate(this.cursor);
            this.composedIn?.locate(this.cursor);
            this.onLocate?.(this.cursor);
        }
        if (outcome.selection !== undefined) {
            this.selection = outcome.selection as unknown as Selection;
            this.selected();
        }
        // The answer asks the window to read the join again, so it goes once
        // the steps that replace the join have landed.
        const answer = outcome.answer;
        (this.domain as AudioDomain).after(() => this.echo.send(answer));
        return changed;
    }

    /**
     * Draw what a history walk left behind -- once the join the walk stitched
     * again has landed, since what the window is told is to read it again.
     */
    override reflectStep(): void {
        (this.domain as AudioDomain).after(() => super.reflectStep());
    }

    /** What the picture measures. See {@link SamplesEditor.layers}. */
    get layers(): Measure[] {
        return [...(this.view as SamplesView).layers];
    }

    set layers(stack: Iterable<string>) {
        const answer = this.coreCall("layers", { stack: [...stack].map(String) });
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        const view = this.view as SamplesView;
        view.layers = answer.layers as Measure[];
        if (this.host !== null && this.window !== null) {
            for (const wid of view.widgets.keys()) {
                void this.host.set(wid, { measure: String(answer.measure) });
            }
        }
    }
}

/** {@link AudioEditor}'s options: the generic ones, the measures and the limit. */
export interface AudioEditorOptions extends GenericEditorOptions<Buffer> {
    /** What the picture measures. Defaults to {@link MEASURES}. */
    layers?: readonly string[];
    /** The most bytes of takes only the history may hold; absent for no limit. */
    historyBytes?: number | null;
}
