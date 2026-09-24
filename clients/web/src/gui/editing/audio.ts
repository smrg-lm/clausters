/**
 * Editing a take as **a list of parts over immutable takes**: the audio editor.
 *
 * What it opens is **a file or a server buffer**, and it edits a private copy:
 * nothing it was handed is written until it is saved. The window draws a **join** -- a buffer
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
 * **It sounds through nodes of its own**, the audio editor's (the shared
 * crate's `AudioEditorPlayback`, the object the GUI host's monitor holds too):
 * one play graph per open take, all but the one played last paused, on a
 * transport of the editor's own, and an output beside them that meters the
 * take and declicks every play and stop on the way to the hardware. The window
 * shows the level on a meter beside the take, and the space bar and `L` are the
 * editor's -- the window says so (`plays`), so the host's monitor stays out.
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
import { AudioEditorPlayback, StepRunner } from "../../core/clausters_core_web.js";
import type { Server } from "../../defs/server/index.ts";
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
    play?: Play;
    cue?: number;
}

/** A press of the space bar, as the editor read it: frames of the take. */
interface Play {
    start: number;
    pass: unknown;
    back: number;
}

/**
 * **What sounds the audio editors of one server** -- the shared crate's
 * playback, the steps it answers carried out on that server.
 *
 * One per server, since the editors on it share one structure and one
 * transport: each open take is a file of it, and the one played last is the
 * one that sounds.
 */
class AudioPlayback {
    static readonly #of = new WeakMap<Server, AudioPlayback>();

    /** The playback of `server`, made the first time it is asked for. */
    static of(server: Server): AudioPlayback {
        let found = AudioPlayback.#of.get(server);
        if (found === undefined) {
            found = new AudioPlayback(server);
            AudioPlayback.#of.set(server, found);
        }
        return found;
    }

    #native: AudioEditorPlayback | null = null;
    readonly #runner = new StepRunner();
    readonly #ready: Promise<void>;
    /** The engine's sample rate, asked once; 0 when it will not say. */
    rate = 0;

    /** The server it plays on. */
    readonly server: Server;

    private constructor(server: Server) {
        this.server = server;
        this.#ready = (async () => {
            this.#native = new AudioEditorPlayback(await server.bulkChunk(), -1);
            // Node ids come back on their `/node_end`, which only a registered
            // client hears.
            await server.notify(true);
            try {
                this.rate = (await server.queryInfo()).nominalSampleRate;
            } catch {
                this.rate = 0;
            }
        })();
    }

    /** One verb of the crate's playback, its steps carried out. */
    async call(verb: string, args: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
        await this.#ready;
        const native = this.#native as AudioEditorPlayback;
        const answer = JSON.parse(
            native.call(JSON.stringify({ verb, ...args }), this.server.ids),
        ) as Record<string, unknown>;
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        const steps = answer.steps as unknown[] | undefined;
        if (steps !== undefined && steps.length > 0) {
            await runSteps(this.server, this.#runner, steps);
        }
        return answer;
    }

    /**
     * Whether the transport rolls, as the engine answers -- a pass that ended
     * on its mark stopped without anybody here saying so.
     */
    async rolling(): Promise<boolean> {
        const transport = Number((await this.call("state")).transport);
        const playing = (await this.server.transportAt(transport).transportState()).playing;
        await this.call("setRolling", { rolling: playing });
        return playing;
    }
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

    /**
     * Free takes the context handed back: nothing reaches them any more. A take
     * on disk (`spilled`) has no buffer to free, only its number to give back.
     */
    free(_structure: Buffer, buffers: number[], spilled: number[] = []): void {
        const editor = this.editor;
        if (editor === null || buffers.length === 0) return;
        this.#queue(async () => {
            for (const bufnum of buffers) {
                if (!spilled.includes(bufnum)) {
                    editor.server.sendMsg("/buffer_free", ["i", Math.trunc(bufnum)]);
                }
                editor.server.buffers.free(Math.trunc(bufnum));
            }
        });
    }

    /**
     * Write a take to disk and free its buffer. A write the server refuses --
     * the page's storage full -- leaves the take in memory, and the crate is
     * told so.
     */
    store(_structure: Buffer, buffer: number, steps: unknown[]): void {
        const editor = this.editor;
        if (editor === null) return;
        this.#work = this.#work.then(async () => {
            try {
                await runSteps(editor.server, editor.runner, steps as never);
            } catch {
                editor.coreCall("kept", { buffer });
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
    after(then: () => void | Promise<void>): void {
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

    /**
     * What sounds the take: the server's audio editor playback, where this
     * editor's take is the file its join is.
     */
    private readonly playback: AudioPlayback;

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
                path: take.path ?? null,
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
        if (options.residentBytes !== undefined && options.residentBytes !== null) {
            this.coreCall("sync", { scratch: options.scratch ?? `clausters-audio/${this.display}` });
            this.editing.limitResident(options.residentBytes);
        }
        this.topUp();
        const copied = this.coreCall("open");
        if (typeof copied.error === "string") throw new RangeError(copied.error);
        // **A private copy of the take**, and the join over it: the buffer the
        // editor was handed is written by a save and by nothing else.
        domain.run(take, (copied.steps as unknown[] | undefined) ?? []);
        this.playback = AudioPlayback.of(this.server);
        domain.after(() => this.sound());
    }

    /**
     * Make what sounds be the take as it now is -- the join and its length --
     * and tell the window where its level is read from.
     */
    private async sound(): Promise<void> {
        const frames = Number(this.coreCall("parts").frames ?? 0);
        await this.playback.call("sync", {
            file: this.display,
            buffer: this.display,
            channels: Math.max(1, Math.trunc(this.structure.channels || 1)),
            frames,
            takeRate: this.sampleRate,
            rate: this.playback.rate,
        });
        this.coreCall("sync", { meters: (await this.playback.call("state")).meters ?? null });
    }

    /**
     * **The position cursor moved**: the play cursor goes with it while
     * nothing plays, and a rolling pass is left alone.
     */
    private async cue(frame: number): Promise<void> {
        await this.playback.rolling();
        await this.playback.call("cue", { frame: Math.trunc(frame) });
    }

    /**
     * **The space bar**, as the editor read it: a rolling transport stops and
     * goes back to the position cursor; a stopped one plays the pass.
     */
    private async play(play: Play): Promise<void> {
        if (await this.playback.rolling()) {
            await this.playback.call("stop", { back: Math.trunc(play.back) });
        } else {
            await this.playback.call("play", {
                file: this.display,
                start: Math.trunc(play.start),
                pass: play.pass,
            });
        }
    }

    /**
     * Opens the window once the join it draws has been stitched -- the steps
     * the constructor queued -- so the first picture is the take and not an
     * empty buffer.
     *
     * The window anchors the play cursor at 0, and the counter that makes that
     * the take's own frame is the transport's position -- what the monitor
     * plays from -- so the host is asked for that clock, as a multitrack
     * editor asks for it when it opens.
     */
    override async open(
        host?: Parameters<Editor<Buffer>["open"]>[0],
        options: Parameters<Editor<Buffer>["open"]>[1] = {},
    ): ReturnType<Editor<Buffer>["open"]> {
        await (this.domain as AudioDomain).idle();
        const handle = await super.open(host, options);
        const transport = Number((await this.playback.call("state")).transport);
        this.host?.headClock(handle, "transport", transport);
        return handle;
    }

    /**
     * The window closed: the take stops being one the editor plays, and the
     * last one closed frees the editor's nodes.
     */
    protected override closedWindow(): boolean {
        (this.domain as AudioDomain).after(async () => {
            await this.playback.call("closeFile", { file: this.display });
        });
        return super.closedWindow();
    }

    /** Close the window, and free what sounded the take. */
    override close(): this {
        (this.domain as AudioDomain).after(async () => {
            await this.playback.call("closeFile", { file: this.display });
        });
        return super.close();
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
     * **Writes the take as the edits have left it** -- over what it was opened
     * from (the file it was read from, or the server buffer it was opened
     * over), or, given a `path` or a `buffer`, there, which a later `save` then
     * writes over. Ctrl+S in the window is the same save.
     *
     * A **buffer** is rewritten whole at the take's length, so whatever reads it
     * hears the edit from then on -- saving into one that is sounding is heard
     * as a glitch, which is yours to avoid. `buffer` is a {@link Buffer} to
     * rewrite, or `true` for a new one. `sampleFormat` (`"float"`, `"int24"`,
     * `"int16"`) is a file's.
     *
     * Resolves to the path written, or the {@link Buffer}. Throws a
     * `RangeError` when the format is not one of those three, or the buffer is
     * one the editor reads.
     */
    async save({
        path,
        buffer,
        sampleFormat = "float",
    }: { path?: string; buffer?: Buffer | true; sampleFormat?: string } = {}): Promise<
        string | Buffer
    > {
        const request: Record<string, unknown> = { format: sampleFormat };
        if (path !== undefined) request.path = path;
        else if (buffer === true) request.buffer = this.server.buffers.alloc();
        else if (buffer !== undefined) request.buffer = Math.trunc(buffer.bufnum);
        const answer = this.coreCall("save", request);
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        const domain = this.domain as AudioDomain;
        domain.run(this.structure, (answer.steps as unknown[] | undefined) ?? []);
        await domain.idle();
        if (typeof answer.path === "string") return answer.path;
        const frames = Number(this.coreCall("parts").frames ?? 0);
        return new Buffer(
            Number(answer.buffer),
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
            this.editing.release(turned.freed, turned.stored);
            (this.domain as AudioDomain).after(() => this.sound());
            return stepped;
        }
        const changed = this.take(outcome);
        this.editing.release(turned.freed, turned.stored);
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
        this.editing.release(turned.freed, turned.stored);
        return changed;
    }

    /**
     * Carry out what a turn came to, and answer the host. Answers whether the
     * take changed.
     */
    private take(outcome: Outcome): boolean {
        if (outcome.turn === undefined || outcome.turn === "nothing") return false;
        const changed = outcome.changed === true;
        // **The steps are this page's to carry out**: a new take made where the
        // turn needed one and the join stitched over the list -- or, for a save
        // from the window, the file written.
        (this.domain as AudioDomain).run(this.structure, outcome.steps ?? []);
        const domain = this.domain as AudioDomain;
        if (changed) {
            this.dirty = true;
            this.editing.changed();
            domain.after(() => this.sound());
        }
        const play = outcome.play;
        if (play !== undefined) domain.after(() => this.play(play));
        const cue = outcome.cue;
        if (cue !== undefined) domain.after(() => this.cue(cue));
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

    /**
     * What the picture measures -- `["peak", "rms"]` for the editor's view,
     * `["peak"]` for the bare envelope.
     *
     * **Assigning it on an open view sends one message.** The measure is a live
     * `/gui_set` prop, so the body appears and disappears over the peaks with
     * the picture, the axis, the zoom, the selection and the playhead all
     * exactly where they were.
     */
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
    /**
     * The most of those kept in memory; absent for all of them. Past it the
     * oldest are written to `scratch` and read back when an undo or a redo
     * needs them.
     */
    residentBytes?: number | null;
    /**
     * The directory, on the server's filesystem -- the page's own storage, in a
     * tab -- a take leaves memory for. One named after the editor's join when
     * not given.
     */
    scratch?: string;
}

/**
 * Whether {@link edit} should open this in the audio editor: anything with a
 * buffer number and samples it can write, which is what a `Buffer` answers with.
 */
export function isTake(structure: unknown): structure is Buffer {
    const candidate = structure as { bufnum?: unknown; setSamples?: unknown };
    return candidate !== null && typeof candidate === "object" &&
        typeof candidate.bufnum === "number" && typeof candidate.setSamples === "function";
}
