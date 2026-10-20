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
import type { Answer } from "./echo.ts";
import { Buffer } from "../../defs/buffer.ts";
import { SoundFile } from "../../soundfile.ts";
import { AudioEditorPlayback, StepRunner } from "../../core/clausters_core_web.js";
import type { Server } from "../../defs/server/index.ts";
import type { Transport, TransportDriver } from "../../defs/server/transport.ts";
import { Segment } from "../../segments.ts";
import { resolveServer } from "../../defs/wire.ts";
import { runSteps } from "../../steps.ts";
import { executeSteps, releaseBuffer } from "./cleanup.ts";
import { serviceInstance } from "./application.ts";
import { exchange, bindExchange, type ExchangeLease } from "./clipboard.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { Marking, marking } from "./marking.ts";
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
    layers?: Measure[];
    preview?: unknown;
    /** Whether to close the window, by the crate's rule for every editor. */
    close?: boolean;
    /** Whether the window holds other widgets now: it is drawn and published again. */
    restructure?: boolean;
    answer?: Answer;
    changed?: boolean;
    version?: number;
    steps?: unknown[];
    locate?: number;
    selection?: unknown;
    play?: Play;
    /** What `L` asks of a pass in progress: how it ends now. */
    pass?: unknown;
    /** What a selection redrawn asks of a loop in progress: its span now. */
    follow?: { from: number; to: number; place: boolean };
    cue?: number;
    /** A new file asked for in the window: its shape, for this page to open. */
    newFile?: { channels: number; frames: number; rate: number };
    /** A file asked to be opened in the window, for this page to open. */
    openFile?: { paths: string[] };
    /** A file the list or the menu asked to focus or close, by its id. */
    fileAction?: { verb: string; file: number };
    /** The file in focus after a file verb: its buffers, cursor and state. */
    binding?: Record<string, unknown>;
    /** A copy made on the server: the document naming the buffer it is in. */
    clipboard?: string;
    /** The buffer that copy is in. */
    clipboardBuffer?: number;
    clipboardCut?: Record<string, unknown>;
    clipboardLease?: ExchangeLease;
    clipboardReady?: boolean;
    operation?: Record<string, unknown>;
    preparedComplete?: boolean;
    playbackUpdate?: { file: Record<string, unknown>; publish: boolean };
}

/** A press of the space bar, as the editor read it: frames of the take. */
interface Play {
    start: number;
    pass: unknown;
    back: number;
}

/**
 * **What sounds one audio editor** -- the shared crate's playback, the steps
 * it answers carried out on the editor's server.
 *
 * One per editor, shared by its open files: one file plays at a time and
 * all others stay paused, and playing one window moves nothing in another. A
 * pass goes on while the window shows another file until it is stopped,
 * from any file, and a play is over the file shown.
 */
class AudioPlayback {
    #native: AudioEditorPlayback | null = null;
    readonly #instance: { connection: number; generation: number };
    readonly #ready: Promise<void>;
    /** The engine's sample rate, asked once; 0 when it will not say. */
    rate = 0;

    /** The server it plays on. */
    readonly server: Server;
    /** The transport it plays on -- one of the server's, allocated for it -- once it is made. */
    transportId: number | null = null;

    constructor(server: Server) {
        this.server = server;
        this.#instance = serviceInstance(server);
        this.#ready = (async () => {
            this.#native = new AudioEditorPlayback(await server.bulkChunk(), -1);
            const taken = JSON.parse(
                this.#native.call(JSON.stringify({ verb: "open" }), server.ids),
            ) as { transport?: number; error?: string };
            if (typeof taken.error === "string") throw new Error(`clausters: ${taken.error}`);
            this.transportId = Number(taken.transport);
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
        const current = serviceInstance(this.server);
        if (current.connection !== this.#instance.connection || current.generation !== this.#instance.generation) {
            throw new Error("the audio service belongs to a previous server instance");
        }
        const native = this.#native as AudioEditorPlayback;
        const answer = JSON.parse(
            native.call(JSON.stringify({ verb, ...args }), this.server.ids),
        ) as Record<string, unknown>;
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        const steps = answer.steps as unknown[] | undefined;
        if (steps !== undefined && steps.length > 0) {
            await executeSteps(this.server as unknown as Server, this.#instance, steps);
        }
        return answer;
    }

    /**
     * Whether the transport rolls, as the engine answers -- a pass that ended
     * on its mark stopped without anybody here saying so.
     */
    async rolling(): Promise<boolean> {
        await this.#ready;
        const playing = (await this.server.transportAt(this.transportId as number).state()).playing;
        await this.call("setRolling", { rolling: playing });
        return playing;
    }
}

/**
 * **What an audio editor's `Transport` plays through**: the server's audio
 * editor playback, its positions the take's seconds, and the time range and
 * the loop switch the window shows -- the band a sweep leaves and `L` -- kept
 * here, stopped or rolling, so a page and a hand read and write one state.
 * Where a pass starts and how it ends are the crate's, as the space bar's are.
 */
class AudioDriver implements TransportDriver {
    /** The time range `[start, end]` in seconds, or `null`. */
    span: [number, number] | null = null;
    /** Whether the loop switch is on: the span, or the whole take. */
    looping = false;
    /** @internal */
    paused = false;
    private readonly editor: AudioEditor;
    private readonly playback: AudioPlayback;
    private get fileId(): number { return Number(this.editor.coreCall("binding").id); }

    constructor(editor: AudioEditor, playback: AudioPlayback) {
        this.editor = editor;
        this.playback = playback;
    }

    async #focused(): Promise<boolean> {
        return (await this.playback.call("state")).focus === this.fileId;
    }

    async playing(): Promise<boolean> {
        const was = (await this.playback.call("state")).rolling === true;
        const rolling = await this.playback.rolling();
        if (was !== rolling) await this.editor.showTransport();
        return rolling && (await this.#focused());
    }

    async play(at?: number): Promise<void> {
        if (at === undefined && this.paused && (await this.#focused())) {
            this.paused = false;
            await this.playback.call("resume");
            await this.editor.showTransport();
            return;
        }
        this.paused = false;
        if (at !== undefined) await this.locate(at);
        const play = this.editor.coreCall("play", { looping: this.looping }) as unknown as Play;
        await this.playback.call("play", { file: this.fileId, start: Math.trunc(play.start), pass: play.pass });
        await this.editor.showTransport();
    }

    async pause(): Promise<void> {
        if (!(await this.#focused())) return;
        await this.playback.call("pause");
        this.paused = true;
        await this.editor.showTransport();
    }

    /**
     * Stop whichever file plays, from the window showing another file too,
     * and bring the file shown into focus at its cursor.
     */
    async stop(): Promise<void> {
        this.paused = false;
        if (!(await this.#focused()) && !(await this.playback.rolling())) return;
        const play = this.editor.coreCall("play", { looping: this.looping }) as unknown as Play;
        await this.playback.call("stop", { back: Math.trunc(play.back), file: this.fileId });
        await this.editor.showTransport();
    }

    async locate(at: number): Promise<void> {
        const frame = Number(this.editor.coreCall("locate", { at }).cue);
        this.editor.cursor = at;
        if ((await this.playback.rolling()) && (await this.#focused())) {
            await this.playback.call("locate", { frame });
        } else {
            await this.playback.call("cue", { frame });
        }
    }

    get end(): "contents" {
        return "contents";
    }

    async setEnd(end: null | "contents" | number): Promise<void> {
        if (end !== "contents") throw new RangeError("an audio editor's pass ends where the take or the span does");
    }

    /**
     * Nothing: the transport is the audio editors' of this server, and a take
     * is freed by closing its editor.
     */
    async free(): Promise<void> {}

    async setSpan(span: readonly [number, number] | null, { show = true }: { show?: boolean } = {}): Promise<void> {
        const band = this.editor.coreCall("span", { span: span === null ? null : [span[0], span[1]] });
        this.span = span === null || !Number(band.sel_len) ? null : [span[0], span[1]];
        if (show) this.editor.showBand(Number(band.sel_start), Number(band.sel_len));
        await this.#follow();
    }

    async setLooping(on: boolean): Promise<void> {
        this.looping = on;
        await this.#follow();
        this.editor.showLooping(on);
    }

    /** A pass in progress ends as the span and the switch now say. */
    async #follow(): Promise<void> {
        if (!(await this.#focused())) return;
        const play = this.editor.coreCall("play", { looping: this.looping }) as unknown as Play;
        await this.playback.call("pass", { pass: play.pass });
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
        this.#queue(() => {
            editor.checkServer();
            return runSteps(editor.server, editor.runner, steps as never);
        });
        return true;
    }

    executeOperation(_structure: Buffer, prepared: Record<string, unknown>, complete: (result: Record<string, any>) => void): void {
        this.after(async () => {
            if (this.editor === null) throw new Error("the operation owner is absent");
            complete(await this.editor.operation({}, prepared));
        });
    }

    async executeBatch(_structure: Buffer, server: { connection: number; generation: number }, steps: unknown[]): Promise<void> {
        const editor = this.editor;
        if (editor === null) throw new Error("the operation owner is absent");
        editor.checkServer();
        const instance = serviceInstance(editor.server);
        if (server.connection !== instance.connection || server.generation !== instance.generation) throw new Error("the batch belongs to another server instance");
        await executeSteps(editor.server as unknown as Server, server, steps);
    }

    /** Keep a stepped object's playback current even when another file is viewed. */
    operationReturned(_structure: Buffer, server: { connection: number; generation: number }, buffers: number[]): void {
        if (this.editor !== null) this.editor.returned(server, buffers);
    }

    operationEcho(_structure: Buffer, answer: unknown): void {
        this.editor?.echo.send(answer as never);
    }

    soundFile(_structure: Buffer, file: Record<string, unknown>): void {
        this.#queue(async () => { await this.editor?.syncSound(file); });
    }

    /**
     * Free takes the context handed back: nothing reaches them any more. A take
     * on disk (`spilled`) has no buffer to free, only its number to give back.
     */
    free(_structure: Buffer, buffers: number[], spilled: number[] = []): void {
        const editor = this.editor;
        if (editor === null || buffers.length === 0) return;
        this.#queue(() => this.cleanup(buffers));
    }

    async cleanup(buffers: number[]): Promise<void> {
        const editor = this.editor;
        if (editor === null) return;
        editor.checkServer();
        await editor.releaseBuffers(buffers);
    }

    /**
     * Write a take to disk and free its buffer. A write the server refuses --
     * the page's storage full -- leaves the take in memory, and the crate is
     * told so.
     */
    store(_structure: Buffer, buffer: number, steps: unknown[], operation?: Record<string, unknown>): void {
        const editor = this.editor;
        if (editor === null) return;
        this.#work = this.#work.then(async () => {
            editor.checkServer();
            try {
                if (operation !== undefined) await editor.operation({}, operation);
                else await executeSteps(editor.server as unknown as Server, serviceInstance(editor.server), steps);
            } catch {
                if (operation === undefined) editor.coreCall("kept", { buffer });
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
        this.#work = this.#work.catch(() => {}).then(run);
        void this.#work.catch(() => {});
    }
}

/** The marking surface, merged so `editor.selected()` types as its own. */
export interface AudioEditor extends Marking<Segment<Buffer> | null, Segment<Buffer>> {}

/**
 * A take on screen, edited as a list of parts over immutable takes.
 *
 * The window draws {@link AudioEditor.buffer}, a join the editor owns: the take
 * as the edits have left it. Play that buffer to hear the edited take, and read
 * {@link AudioEditor.parts} for what it is made of.
 *
 * **What is marked** (`selected`, `select`, `unselect`) is the samples a drag
 * over the take covers, as a `Segment` over {@link AudioEditor.buffer} -- its
 * `start` a frame and its `duration` seconds -- or `null` when nothing is
 * marked. In an audio editor what is marked and the transport's `span` are one
 * range, so setting either sets both.
 */
export class AudioEditor extends Editor<Buffer> {
    /** This editor's member in its editing context. */
    private readonly member: number;
    private instance!: { connection: number; generation: number };

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
    private display: number;

    /**
     * What sounds the take: the server's audio editor playback, where this
     * editor's take is the file its join is.
     */
    private readonly playback: AudioPlayback;

    /** The {@link SoundFile} the take was loaded from, when it was. */
    readonly #file: SoundFile | null;

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
        this.#file = options.file ?? null;
        this.server = resolveServer(take.server);
        this.instance = serviceInstance(this.server);
        this.display = this.server.buffers.alloc();
        // The context is the take's; switching files changes the structure,
        // so the window keeps the one it opened in.
        this.givenContext = this.editing;
        const opened = this.editing.open(
            "openAudio",
            `audio:${this.instance.connection}:${this.instance.generation}:${Math.trunc(take.bufnum)}`,
            {
                ...this.facts(),
                frames: Math.trunc(take.frames || 0),
                take: Math.trunc(take.bufnum),
                display: this.display,
                server: this.instance,
                path: take.path ?? null,
                untitled: this.#file !== null && this.#file.path === null,
                codec: this.#file?.codec, sample_format: this.#file?.sampleFormat,
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
        const opening = this.editing.serviceCall<Record<string, unknown>>("operation", {
            action: "prepare", member: this.member, command: { kind: "open" },
        });
        domain.after(async () => { await this.operation({ kind: "open" }, opening); });
        this.playback = new AudioPlayback(this.server);
        this.#driver = new AudioDriver(this, this.playback);
        domain.after(() => this.sound());
    }

    /** What the `transport` plays through. */
    readonly #driver: AudioDriver;

    /**
     * **The take's transport**, as the object a page plays: a `Transport` whose
     * verbs (`play`, `pause`, `stop`, `locate`, `loop`, `wait`) and `span`
     * speak the take's seconds -- the span is the band a drag sweeps, the loop
     * switch is `L`, and each side reads what the other set. It is this
     * editor's own -- one of the server's, allocated for it -- so another
     * window's play never moves it. `null` until the editor is open.
     */
    get transport(): Transport | null {
        const id = this.playback.transportId;
        if (id === null) return null;
        const transport = this.server.transportAt(id);
        transport.driver = this.#driver;
        return transport;
    }

    // ---- what the hand marked ----

    /**
     * The marked samples: the transport's span, from the frame it starts on,
     * for its seconds.
     *
     * @internal
     */
    marked(): Segment<Buffer> | null {
        const span = this.#driver.span;
        if (span === null) return null;
        return new Segment(this.buffer, Math.round(span[0] * this.sampleRate), span[1] - span[0]);
    }

    /** Marks `segment`, which sets the transport's span. @internal */
    async mark(segment: Segment<Buffer> | null): Promise<void> {
        if (segment === null) {
            await this.#driver.setSpan(null);
            return;
        }
        const start = segment.start / this.sampleRate;
        await this.#driver.setSpan([start, start + segment.duration]);
    }

    /** The band a sweep leaves, drawn where the take is. @internal */
    showBand(start: number, length: number): void {
        if (this.host === null || this.window === null) return;
        for (const wid of (this.view as SamplesView).widgets.keys()) {
            void this.host.set(wid, { sel_start: start, sel_len: length });
        }
    }

    /** The window's loop switch, as `L` leaves it. @internal */
    showLooping(on: boolean): void {
        if (this.host === null || this.window === null) return;
        for (const wid of (this.view as SamplesView).widgets.keys()) {
            void this.host.set(wid, { looping: on ? 1 : 0 });
            break;
        }
    }

    /**
     * Make what sounds be the take as it now is -- the join and its length --
     * and tell the window where its level is read from.
     */
    /**
     * **A new file asked for in the window**: an empty {@link SoundFile} of
     * that shape, added to this editor.
     */
    private async newFileShape(shape: { channels: number; frames: number; rate: number }): Promise<void> {
        await this.addFile(SoundFile.empty(shape.channels, shape.frames, shape.rate));
    }

    /** Add and focus an empty SoundFile of the requested shape. */
    async newFile(channels: number, frames: number, rate: number): Promise<void> {
        await (this.domain as AudioDomain).idle();
        await this.newFileShape({ channels, frames, rate });
    }

    /** **Files asked to be opened**: each read and added, the last in focus. */
    private async openPaths(paths: readonly string[]): Promise<void> {
        for (const path of paths) await this.addFile(await SoundFile.open(path));
    }

    /**
     * Read `file` into a buffer and hand it to the crate as a new file in
     * focus, with a display and spare buffers of its own.
     */
    private async addFile(file: SoundFile): Promise<void> {
        const take = await file.toBuffer({ server: this.server });
        const answer = this.coreCall("add_file", {
            take: take.bufnum, frames: take.frames, channels: take.channels, rate: take.sampleRate,
            display: this.server.buffers.alloc(), path: file.path, untitled: file.path === null,
            codec: file.codec, sample_format: file.sampleFormat,
            buffers: Array.from({ length: SPARE }, () => this.server.buffers.alloc()),
        });
        if (typeof answer.error === "string") {
            this.editing.operationReturned(answer.returned as never);
            this.editing.release(answer.freed as never, answer.stored as never);
        }
        await this.applyFile(answer);
    }

    /** Open a path or an array of paths in this editor, focusing the last. */
    async openFile(paths: string | readonly string[]): Promise<void> {
        await (this.domain as AudioDomain).idle();
        const answer = this.coreCall("open_file", { paths });
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        await this.openPaths((answer.openFile as { paths: string[] }).paths);
    }

    /** Open SoundFiles in list order; a buffer-only entry is null. */
    get files(): (SoundFile | null)[] {
        type Row = {
            file: {
                path: string | null;
                frames: number;
                channels: number;
                rate: number;
                untitled: boolean;
                codec: string | null;
                sample_format: string | null;
            };
        };
        return (this.coreCall("files").files as Row[]).map(({ file: f }) =>
            f.path !== null || f.untitled
                ? new SoundFile(f.path, f.frames, f.channels, f.rate, f.codec, f.sample_format)
                : null,
        );
    }

    /** Focus the file at zero-based index, restoring its cursor and selection. */
    async focusFile(index: number): Promise<void> {
        await (this.domain as AudioDomain).idle();
        await this.applyFile(this.coreCall("focus_file", { index }));
    }

    /** Close the focused file, or the file at zero-based index. */
    async closeFile(index?: number): Promise<void> {
        await (this.domain as AudioDomain).idle();
        await this.applyFile(this.coreCall("close_file", index === undefined ? {} : { index }));
    }

    /** Show the independent file list to the left of the editing view. */
    showFiles(): void { this.take(this.coreCall("show_files") as Outcome); }

    /** Hide the file list. */
    hideFiles(): void { this.take(this.coreCall("hide_files") as Outcome); }

    /** The identity the window's own widgets are named by. */

    /**
     * A component of this window, rebound to the focused file's data.
     * Component ids survive switches; the context version rejects gestures
     * made against the previous file's binding.
     */
    windowWidget(role: string, key = ""): number {
        return this.app.viewIdFor(role, key, this);
    }

    // ---- the history: the order of the file in focus ----

    /** A step of this window's order: the file in focus walks its own. */
    protected override step(direction: "undo" | "redo"): boolean {
        const context = this.editing;
        return this.app.stepped(context, context.step(direction, this.member), this);
    }

    /** Whether the file in focus has an edit to step back over. */
    override get canUndo(): boolean {
        return this.editing.state(this.member).canUndo === true;
    }

    /** Whether the file in focus has an undone edit to step forward into. */
    override get canRedo(): boolean {
        return this.editing.state(this.member).canRedo === true;
    }

    /** What an undo of the file in focus would be called. */
    override get undoLabel(): string | undefined {
        return this.editing.state(this.member).undoLabel ?? undefined;
    }

    /** What a redo of the file in focus would be called. */
    override get redoLabel(): string | undefined {
        return this.editing.state(this.member).redoLabel ?? undefined;
    }

    /**
     * **Carry out a file verb's answer**: the server steps, the buffers it
     * freed and, when the focus moved, the file now in focus bound (its
     * buffers, cursor, selection, layers and loop switch) and the playback
     * shown the file.
     */
    private async applyFile(answer: Record<string, unknown>): Promise<void> {
        this.checkServer();
        if (typeof answer.error === "string") throw new RangeError(answer.error);
        if (answer.operation !== undefined) {
            const completed = await this.operation({}, answer.operation as Record<string, unknown>);
            await this.applyFile(completed.outcome ?? {});
            return;
        }
        const action = answer.fileAction as { verb: string; file: number } | undefined;
        if (action?.verb === "close_file") {
            await this.playback.call("closeFile", { file: action.file });
        }
        await runSteps(this.server, this.runner, (answer.steps ?? []) as never);
        await (this.domain as AudioDomain).cleanup((answer.freedBuffers ?? []) as number[]);
        if (action?.verb === "close_file") this.retainPlayback();
        if (answer.close === true) { this.close(); return; }
        if (answer.focusChanged !== true) {
            if (this.windowId !== null) this.app.publish(this.windowId, this.draw());
            return;
        }
        const b = answer.binding as Record<string, unknown>;
        this.display = Number(b.display);
        this.sampleRate = Number(b.rate);
        this.structure = new Buffer(
            Number(b.take),
            Number(b.frames),
            Number(b.channels),
            this.sampleRate,
            this.server,
        );
        this.structureId = Number(b.structure);
        this.editing.version = Number(b.version);
        this.cursor = Number(b.cursor) / this.sampleRate;
        this.dirty = b.dirty === true;
        (this.view as SamplesView).layers = b.layers as Measure[];
        this.#driver.span = b.span as [number, number] | null;
        this.#driver.looping = b.looping === true;
        this.#driver.paused = false;
        await this.sound(false);
        // a pass in progress goes on: the window only looks elsewhere
        await this.playback.rolling();
        await this.playback.call("view", { file: this.fileId, frame: Number(b.cursor) });
        if (this.windowId !== null) this.app.publish(this.windowId, this.draw());
        await this.sound();
        // the switch moved the version: the answer hands it to the host, with
        // the incoming file's cursor and selection
        this.echo.send(answer.answer as never);
        await this.showTransport();
    }

    private async sound(publishPreview = true): Promise<void> {
        const file = this.coreCall("playback");
        if (file.active === false) return;
        await this.syncSound(file);
        const preview = file.preview as Record<string, unknown>;
        const state = await this.playback.call("state");
        if (publishPreview && this.host !== null && this.window !== null) {
            const previews = state.previews as Record<string, number | null> | undefined;
            // every view of the take, each drawing the preview as itself
            for (const wid of (preview.pictures as number[] | undefined) ?? []) {
                await this.host.set(wid, { preview_buffer: previews?.[String(this.fileId)] ?? -1 });
            }
        }
        this.coreCall("sync", { meters: state.meters ?? null });
    }

    /** Sync one object's graph independently of the window's focus. @internal */
    async syncSound(file: Record<string, unknown>): Promise<void> {
        this.dirty = this.coreCall("binding").dirty === true;
        if (file.active === false) return;
        const id = Number(file.id);
        await this.playback.call("sync", { file: id, buffer: Number(file.display), channels: Number(file.channels),
            frames: Number(file.frames), takeRate: Number(file.rate), rate: this.playback.rate });
        const preview = file.preview as Record<string, unknown>;
        if (preview.installed === true) {
            await this.playback.call("gain", { file: id, points: preview.points, on: preview.on, revision: preview.revision });
        } else {
            await this.playback.call("removeEffect", { file: id });
        }
        this.retainPlayback();
    }

    /**
     * **The position cursor moved**: the play cursor goes with it while
     * nothing plays, and a rolling pass is left alone.
     */
    private async cue(frame: number): Promise<void> {
        await this.playback.rolling();
        await this.playback.call("cue", { frame: Math.trunc(frame) });
        await this.showTransport();
    }

    /**
     * **The space bar**, as the editor read it: a rolling transport stops and
     * goes back to the position cursor; a stopped one plays the pass.
     */
    private async play(play: Play): Promise<void> {
        if (await this.playback.rolling()) {
            // whichever file plays stops, and the one shown comes into focus
            await this.playback.call("stop", { back: Math.trunc(play.back), file: this.fileId });
        } else {
            await this.playback.call("play", {
                file: this.fileId,
                start: Math.trunc(play.start),
                pass: play.pass,
            });
        }
        await this.showTransport();
    }

    /**
     * **A copy made on the server**: the buffer it is in is the clipboard's
     * now, the one the previous copy was in goes, and the host's clipboard
     * names it.
     */
    private clipboard(doc: string, buffer: number, ready = false): void {
        this.checkServer();
        if (ready) bindExchange(this.server, this.editing.serviceCore);
        else doc = exchange(this.server, "publish", { document: doc }).document as string;
        this.host?.clipboard(doc);
    }

    private get fileId(): number { return Number(this.coreCall("binding").id); }

    /**
     * **The window's play cursor and the list's play mark, as the playback
     * stands**: the line is drawn over the file shown unless another file is
     * playing, and the list marks the file that plays. Read off what the
     * playback was last told: an engine asked right after a play may not be
     * rolling yet.
     */
    async showTransport(): Promise<void> {
        const state = await this.playback.call("state");
        const host = this.host;
        if (host === null || this.window === null) return;
        const playing = this.coreCall("playing", { file: state.playing ?? null, meters: state.meters ?? null });
        type Correction = { widget: number; props: Parameters<typeof host.set>[1] };
        for (const correction of (playing.corrections ?? []) as Correction[]) {
            await host.set(Number(correction.widget), correction.props);
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
        if (this.windowId === null) this.coreCall("attach_view");
        const handle = await super.open(host, options);
        const transport = Number((await this.playback.call("state")).transport);
        this.host?.headClock(handle, "transport", transport);
        return handle;
    }

    protected override playsOn(): Server | null {
        return this.server as unknown as Server;
    }

    /**
     * The window closed: the take stops being one the editor plays, and the
     * last one closed frees the editor's nodes.
     */
    protected override closedWindow(): boolean {
        this.coreCall("detach_view");
        (this.domain as AudioDomain).after(async () => {
            await this.playback.call("close");
            this.dropPlayback();
        });
        return super.closedWindow();
    }

    /** Close the window, and free what sounded the take. */
    override close(): this {
        this.coreCall("detach_view");
        (this.domain as AudioDomain).after(async () => {
            await this.playback.call("close");
            this.dropPlayback();
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
            chrome: this.chrome,
            window: this.windowId,
        };
    }

    /**
     * One verb of the core, with its arguments; the answer, parsed.
     *
     * @internal
     */
    coreCall(verb: string, args: Record<string, unknown> = {}): Record<string, unknown> {
        this.checkServer();
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

    /** Refuse stale resources before addressing the replacement server. @internal */
    checkServer(): void {
        const current = serviceInstance(this.server);
        if (current.connection !== this.instance.connection || current.generation !== this.instance.generation) {
            this.editing.serviceCall("invalidate", { server: this.instance });
            throw new Error("the audio service belongs to a previous server instance");
        }
    }

    private retainPlayback(): void {
        const result = this.editing.serviceCall("retainMember", { member: this.member, kind: "playback" });
        this.editing.release(result.freed as never, result.stored as never);
    }

    private dropPlayback(): void {
        const result = this.editing.serviceCall("dropRoot", { root: { kind: "playback", owner: this.member } });
        this.editing.release(result.freed as never, result.stored as never);
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

    /** Paste onto what is there, mixed with it -- the window's `mix` ({@link Editor.verb}). */
    mix(): boolean {
        return this.verb("mix");
    }

    /** Execute resource cleanup against this composition's exact tickets. @internal */
    async releaseBuffers(buffers: number[]): Promise<void> {
        const server = serviceInstance(this.server);
        for (const buffer of buffers) {
            await releaseBuffer(this.server as unknown as Server, this.editing.serviceCore, { server, buffer });
        }
    }

    /** Return ids only to the allocator for their original incarnation. @internal */
    returned(server: { connection: number; generation: number }, buffers: number[]): void {
        const current = serviceInstance(this.server);
        if (server.connection !== current.connection || server.generation !== current.generation) return;
        for (const buffer of buffers) this.server.buffers.free(buffer);
    }

    /**
     * Retry failed repairs for this editor's files without repeating edits.
     * Call after resolving the I/O failure. Contents and history stay unchanged;
     * successful repair permits editing, saving and closing again. Rejects if
     * repair fails again. Does nothing if none remain.
     */
    async retryPending(): Promise<void> {
        this.checkServer();
        const pending = this.editing.serviceCall<{ repairs: { ticket: unknown }[] }>("operation", { action: "pendingRepairs", member: this.member });
        for (const repair of pending.repairs ?? []) {
            const prepared = this.editing.serviceCall("operation", { action: "retryRepair", ticket: repair.ticket });
            await this.operation({}, prepared, true);
        }
    }

    /** Execute typed batches; commit, refusal and cleanup come from Rust. @internal */
    async operation(command: Record<string, unknown>, prepared?: Record<string, unknown>, repairing = false): Promise<Record<string, any>> {
        let answer: Record<string, any> = prepared ?? this.editing.serviceCall("operation", {
            action: "prepare", member: this.member, command,
        });
        while (answer.complete !== true) {
            if (answer.accepted === false) throw new Error("the operation is no longer available");
            if (typeof answer.error === "string") throw new RangeError(answer.error);
            let result: { status: string; reason?: string };
            try {
                await this.editing.executeBatch(Number(answer.member), answer.server, answer.steps ?? []);
                result = { status: "success" };
            } catch (error) {
                result = { status: "failed", reason: String(error) };
            }
            answer = this.editing.serviceCall("operation", {
                action: "complete", ticket: answer.ticket, batch: answer.batch, result,
            });
        }
        if (answer.repair !== undefined) {
            const ticket = answer.repair;
            try {
                for (const recovery of answer.recoveries ?? []) await this.editing.executeBatch(Number(recovery.member), recovery.server, recovery.steps ?? []);
            } catch (error) {
                answer = this.editing.serviceCall("operation", { action: "repairFailed", ticket, reason: String(error) });
            }
            if (answer.repair !== undefined) answer = this.editing.serviceCall("operation", { action: "repaired", ticket });
        }
        this.editing.version = Number(answer.version ?? this.editing.version);
        this.editing.operationReturned(answer.returned);
        this.editing.operationEcho(answer.echo);
        this.editing.release(answer.freed, answer.stored);
        this.dirty = Boolean(this.coreCall("binding").dirty);
        if (answer.committed !== true && !(repairing && answer.repaired === true)) {
            if (answer.echo == null) this.echo.send(answer.outcome?.answer);
            throw new Error(answer.reason ?? answer.outcome?.answer?.reason ?? "the operation was refused");
        }
        return answer;
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
        await (this.domain as AudioDomain).idle();
        this.syncCore();
        const completed = await this.operation({ kind: "save", ...request });
        this.take((completed.outcome ?? {}) as Outcome);
        const answer = completed.destination;
        if (typeof answer.path === "string") {
            return answer.path;
        }
        return new Buffer(Number(answer.buffer), Number(answer.frames), Number(answer.channels),
            Number(answer.rate), this.server);
    }

    /**
     * **The {@link SoundFile} the take is**, at its length now: the file it was
     * opened from or last saved to, one with no path when it has not been
     * written yet, and `null` for a take opened over a buffer and saved into
     * none.
     */
    get file(): SoundFile | null {
        const answer = this.coreCall("file");
        const path = typeof answer.path === "string" ? answer.path : null;
        const frames = Number(answer.frames ?? 0);
        const channels = Number(answer.channels ?? 1);
        const rate = Number(answer.rate ?? this.sampleRate);
        if (path === null) {
            return answer.untitled === true ? new SoundFile(null, frames, channels, rate) : null;
        }
        return new SoundFile(
            path,
            frames,
            channels,
            rate,
            answer.codec as string | null,
            answer.sample_format as string | null,
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
        const prepared = addr === "/gui_event"
            ? exchange(this.server, "event", { args: plain([...rawArgs]) })
            : { args: plain([...rawArgs]) };
        const turned = this.editing.event(this.member, addr, prepared.args as unknown[]);
        const outcome = (turned.outcome ?? {}) as Outcome;
        outcome.clipboardLease = prepared.lease as ExchangeLease | undefined;
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
        const [wid, tag, ...values] = args;
        return this.deliver("/gui_event", [wid, 0, 0, tag, ...values]);
    }

    /**
     * Carry out what a turn came to, and answer the host. Answers whether the
     * take changed.
     */
    private take(outcome: Outcome): boolean {
        if (outcome.operation !== undefined) {
            const work = outcome.operation;
            (this.domain as AudioDomain).after(async () => {
                try {
                    const completed = await this.operation({}, work);
                    this.editing.turn(this, () => this.take((completed.outcome ?? {}) as Outcome));
                } finally {
                    if (outcome.clipboardLease !== undefined) exchange(this.server, "release", { lease: outcome.clipboardLease });
                }
            });
            return outcome.changed === true;
        }
        if (outcome.turn === undefined || outcome.turn === "nothing") {
            if (outcome.clipboardLease !== undefined) exchange(this.server, "release", { lease: outcome.clipboardLease });
            return false;
        }
        if (outcome.fileAction !== undefined && outcome.binding === undefined) {
            const action = outcome.fileAction;
            this.echo.send(outcome.answer);
            (this.domain as AudioDomain).after(() =>
                this.applyFile(this.coreCall(action.verb, { file: action.file })),
            );
            return false;
        }
        if (outcome.layers !== undefined) (this.view as SamplesView).layers = outcome.layers;
        // **A view placed in a lane of its own** (the stacked views, or back
        // to one): the window is drawn again and the host keeps what is its own.
        if (outcome.restructure === true && this.windowId !== null) {
            this.app.publish(this.windowId, this.draw());
            // the def carries no preview: each view is handed it again
            void this.sound();
        }
        const changed = outcome.changed === true;
        // **The steps are this page's to carry out**: a new take made where the
        // turn needed one and the join stitched over the list -- or, for a save
        // from the window, the file written.
        const steps = outcome.steps ?? [];
        const domain = this.domain as AudioDomain;
        if (outcome.clipboard !== undefined || outcome.clipboardLease !== undefined) {
            domain.after(async () => {
                let published = false;
                try {
                    this.checkServer();
                    await runSteps(this.server, new StepRunner(), steps as never);
                    if (outcome.clipboard !== undefined && outcome.clipboardBuffer !== undefined) {
                        this.clipboard(outcome.clipboard, outcome.clipboardBuffer, outcome.clipboardReady === true);
                        published = true;
                        if (outcome.clipboardCut !== undefined) {
                            this.syncCore();
                            this.editing.turn(this, () => {
                                const continued = this.editing.act(this.member, outcome.clipboardCut!);
                                const next = (continued.outcome ?? {}) as Outcome;
                                if (next.answer != null) next.answer.seq = outcome.answer?.seq ?? 0;
                                this.take(next);
                                this.editing.release(continued.freed, continued.stored);
                            });
                        }
                    }
                } catch (error) {
                    if (!published && outcome.clipboardBuffer !== undefined) {
                        this.checkServer();
                        void this.server.sendMsg("/buffer_free", ["i", outcome.clipboardBuffer]);
                        this.server.buffers.free(outcome.clipboardBuffer);
                    }
                    if (outcome.clipboard !== undefined && outcome.answer !== undefined) {
                        outcome.answer.reason = String(error);
                        outcome.answer.docVersion = this.editing.version;
                        if (outcome.clipboardCut !== undefined) this.echo.send(outcome.answer);
                    } else {
                        throw error;
                    }
                } finally {
                    if (outcome.clipboardLease !== undefined) exchange(this.server, "release", { lease: outcome.clipboardLease });
                }
            });
        } else {
            domain.run(this.structure, steps);
        }
        if (outcome.newFile !== undefined || outcome.openFile !== undefined) {
            this.echo.send(outcome.answer);
            outcome = { ...outcome, answer: undefined };
        }
        if (outcome.newFile !== undefined) {
            const shape = outcome.newFile;
            domain.after(() => this.newFileShape(shape));
        }
        if (outcome.openFile !== undefined) {
            const paths = outcome.openFile.paths;
            domain.after(() => this.openPaths(paths));
        }
        if (outcome.playbackUpdate !== undefined) {
            const update = outcome.playbackUpdate;
            domain.after(async () => {
                await this.syncSound(update.file);
                if (update.publish) await this.sound();
            });
        }
        if (changed) {
            this.dirty = this.coreCall("binding").dirty === true;
            this.editing.changed();
            if (outcome.preparedComplete !== true) domain.after(() => this.sound());
        }
        if (outcome.preview !== undefined && !changed) domain.after(() => this.sound());
        const play = outcome.play;
        // The loop switch rides beside the space bar and `L`: a pass that
        // loops is the switch on.
        if (play !== undefined) {
            this.#driver.paused = false;
            this.#driver.looping = (play.pass as { kind?: string }).kind === "loop";
            domain.after(() => this.play(play));
        }
        const pass = outcome.pass;
        if (pass !== undefined) this.#driver.looping = (pass as { kind?: string }).kind === "loop";
        // `L`: the pass in progress ends as the switch now says; a stopped
        // playback reads it on its next play.
        if (pass !== undefined) domain.after(() => this.playback.call("pass", { pass }).then(() => {}));
        // A selection redrawn while this take loops: the loop is over the
        // new span, and the head goes into it once the sweep is let go.
        // Whether it loops, and whether it is this take, is the playback's
        // to say.
        const follow = outcome.follow;
        if (follow !== undefined) {
            domain.after(() => this.playback.call("follow", { file: this.fileId, ...follow }).then(() => {}));
        }
        const cue = outcome.cue;
        if (cue !== undefined) domain.after(() => this.cue(cue));
        if (outcome.locate !== undefined) {
            this.cursor = outcome.locate;
            this.locate(this.cursor);
            this.composedIn?.locate(this.cursor);
            this.onLocate?.(this.cursor);
        }
        if (outcome.selection !== undefined) {
            // A sweep's time range is the transport's span, and what the hand
            // marked: the space bar plays it and the loop repeats it.
            const swept = outcome.selection as unknown as { start?: number; len?: number };
            const start = Number(swept.start ?? 0);
            const length = Number(swept.len ?? 0);
            this.#driver.span = length > 0 ? [start, start + length] : null;
        }
        // The answer asks the window to read the join again, so it goes once
        // the steps that replace the join have landed.
        const answer = outcome.answer;
        (this.domain as AudioDomain).after(() => {
            if (outcome.clipboardCut === undefined) this.echo.send(answer);
            this.closing(outcome);
        });
        return changed;
    }

    /**
     * Draw what a history walk left behind -- once the join the walk stitched
     * again has landed, since what the window is told is to read it again.
     */
    override reflectStep(): void {
        (this.domain as AudioDomain).after(() => {
            super.reflectStep();
            this.dirty = this.coreCall("binding").dirty === true;
        });
    }

    /**
     * Visual layers, back to front: names or objects with draw, visible,
     * solo, alpha and y. Use y: "box" for amplitude over a frequency axis.
     * Assignment applies the shared application's outcome to the open view.
     */
    get layers(): Measure[] {
        return [...(this.view as SamplesView).layers];
    }

    set layers(stack: string | Iterable<Measure>) {
        this.viewLayers("layers", { stack: typeof stack === "string" ? stack : [...stack] });
    }

    private viewLayers(verb: string, args: Record<string, unknown> = {}): void {
        const outcome = this.coreCall(verb, { version: this.editing.version, ...args });
        if (typeof outcome.error === "string") throw new RangeError(outcome.error);
        this.take(outcome as Outcome);
    }

    /** Gain envelope as time/value/shape/curve quads, in seconds; gain is 0 to 1, 1 is unity.
     * Editing is non-destructive. applyEnvelope renders into a new take. */
    get envelope(): number[] { return this.coreCall("envelope").points as number[]; }

    /** Set gain quads as one undoable edit; times increase and ranges hold. */
    setEnvelope(points: Iterable<number>): boolean {
        return this.envelopeAct("setautomation", { points: [...points] });
    }

    /** Show and select the gain curve body over the take. */
    showEnvelope(): void { this.viewLayers("show_envelope"); }

    /** Render gain and reset the curve to unity in one undo step.
     * Preview playback is not affected until this is applied. */
    applyEnvelope(): boolean { return this.envelopeAct("apply_envelope"); }

    /** Hear the gain envelope through a ramped insert. */
    previewOn(): void { this.envelopeAct("preview_on"); }

    /** Ramp to dry and pause the insert, retaining its state. */
    previewOff(): void { this.envelopeAct("preview_off"); }

    /** Free the preview insert and table, retaining the unapplied curve. */
    removeEffect(): void { this.envelopeAct("remove_effect"); }

    /**
     * The channels drawn folded, in order: strips one control tall that a
     * press on their name unfolds. A view state, outside the history.
     */
    get folded(): number[] { return this.coreCall("folded").folded as number[]; }

    /** Fold channel `channel` (from 0) to a strip, or unfold it when it is folded. */
    foldChannel(channel: number): void { this.channels("fold_channel", { channel }); }

    /** Show channel `channel` alone: every other channel folded. */
    soloChannel(channel: number): void { this.channels("solo_channel", { channel }); }

    /** Unfold every channel. */
    showAllChannels(): void { this.channels("show_all_channels"); }

    /**
     * A channel verb: the crate's answer carried out, and a refusal -- a
     * channel the take does not have -- thrown.
     */
    private channels(verb: string, args: Record<string, unknown> = {}): void {
        const outcome = this.coreCall(verb, { version: this.editing.version, ...args }) as Outcome;
        this.take(outcome);
        const reason = outcome.answer?.reason;
        if (reason) throw new RangeError(reason);
    }

    private envelopeAct(verb: string, args: Record<string, unknown> = {}): boolean {
        return this.editing.turn(this, () => {
            this.syncCore();
            const turned = this.editing.act(this.member, { verb, ...args });
            const outcome = (turned.outcome ?? {}) as Outcome;
            const changed = this.take(outcome);
            this.editing.release(turned.freed, turned.stored);
            const reason = outcome.answer?.reason;
            if (reason) throw new RangeError(reason);
            return changed;
        });
    }

    /** The active edit body, separate from the take's visual layers. */
    get layer(): string { return this.coreCall("layer").layer as string; }
    set layer(name: string) { this.viewLayers("layer", { name }); }

    /** Show a visual layer, appending it if absent. */
    showLayer(name: string): void { this.viewLayers("show_layer", { name }); }

    /** Hide a visual layer, keeping its place and settings. */
    hideLayer(name: string): void { this.viewLayers("hide_layer", { name }); }

    /** Toggle solo; only visible soloed layers are drawn. */
    soloLayer(name: string): void { this.viewLayers("solo_layer", { name }); }

    /** Set a layer's opacity between zero and one. */
    layerAlpha(name: string, alpha: number): void { this.viewLayers("layer_alpha", { name, alpha }); }

    /** Show the peak and RMS waveform in the take's lane, instead of the spectrogram. */
    viewWave(): void { this.viewLayers("view_wave"); }

    /** Show the spectrogram in the take's lane, instead of the waveform. */
    viewSpectrogram(): void { this.viewLayers("view_spectrogram"); }

    /** Show both views, stacked: the waveform in the take's lane and the
     * spectrogram in a lane under it, on one time axis. */
    viewBoth(): void { this.viewLayers("view_both"); }

}

marking(AudioEditor);

/** {@link AudioEditor}'s options: the generic ones, the measures and the limit. */
export interface AudioEditorOptions extends GenericEditorOptions<Buffer> {
    /** What the picture measures. Defaults to {@link MEASURES}. */
    layers?: string | readonly Measure[];
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
    /**
     * The {@link SoundFile} `take` was loaded from, which a save writes -- one
     * with no path asks for one. `edit` loads a file and passes it here.
     */
    file?: SoundFile | null;
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
