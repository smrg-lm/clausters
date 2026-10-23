/**
 * Editing a take as **a list of parts over immutable files**: the audio editor.
 *
 * What it opens is **a file or a server buffer**, and nothing it was handed is
 * written until it is saved. The take is a list of spans of files: a cut takes
 * a span out, a paste of a copy puts its spans in, and an edit that makes new
 * samples -- a pencil stroke, a pasted block, a mix, a gain curve applied, a
 * reverse -- **renders** them offline into a file of the editor's working
 * folder, which the list then names. Nothing a history entry names is ever
 * written again, so an undo is the list before: it costs the list and not the
 * samples. The window draws the list from its files, and the take sounds
 * through a **join** -- a buffer made of the list's spans (`Buffer.stitch`),
 * streamed from disk.
 *
 * **The editor is the shared crate's** (`EditingCore`, the `openAudio`
 * member): what each gesture does to the list, the renders that make its new
 * files, and which renders the history can no longer reach. What is here is
 * what a language owns: the server the steps are walked against, the renders
 * run on offline engines of their own, the working folder, and deleting the
 * renders the context hands back.
 *
 * **What a gesture, a verb or the transport leads to is the shared crate's
 * coordinator's** (the context's `audio` door): every action runs there and
 * comes back as an ordered list of effects -- steps to carry out, props to
 * set, the window to draw again, an answer to send -- with, where the next
 * step depends on it, one thing to wait on. What is here carries those out.
 *
 * **It sounds through nodes of its own**, the audio editor's (the shared
 * crate's `AudioEditorPlayback`, which the coordinator holds; the GUI host's
 * monitor holds another):
 * one play graph per open take, all but the one played last paused, on a
 * transport of the editor's own, and an output beside them that meters the
 * take and declicks every play and stop on the way to the hardware. The window
 * shows the level on a meter beside the take, and the space bar and `L` are the
 * editor's -- the window says so (`plays`), so the host's monitor stays out.
 *
 * **Disk is spent on what the history holds.** Every file a stroke or a paste
 * rendered is kept while an undo or a redo, or the clipboard, can still reach
 * it, and deleted when none can. The working folder is a temporary one unless
 * one is given, and goes with the editor -- in a tab, a folder of the page's
 * own storage.
 *
 * @module
 */

import { PARTS } from "../../document.ts";
import type { Answer } from "./echo.ts";
import { Buffer } from "../../defs/buffer.ts";
import { Editing } from "../../history.ts";
import { SoundFile } from "../../soundfile.ts";
import { StepRunner } from "../../core/clausters_core_web.js";
import { CommandError } from "../../errors.ts";
import type { Server } from "../../defs/server/index.ts";
import type { Transport, TransportDriver } from "../../defs/server/transport.ts";
import { Segment } from "../../segments.ts";
import { resolveServer } from "../../defs/wire.ts";
import { disk, runSteps } from "../../steps.ts";
import { executeSteps, releaseBuffer } from "./cleanup.ts";
import { makeDirectoryAt, removeDirectoryAt, temporaryFolder } from "../../base/files.ts";
import { serviceInstance } from "./application.ts";
import { exchange, bindExchange, type ExchangeLease } from "./clipboard.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { Marking, marking } from "./marking.ts";
import { MEASURES, SamplesView, plain } from "./samples.ts";
import type { Measure } from "./samples.ts";

/**
 * The containers a part of a list reads in place; a file of any other codec is
 * opened through a buffer of it.
 */
const STREAMED = ["wav", "w64", "rf64"];

/**
 * Whether the audio editor reads `file` where it is: a PCM file a part streams
 * from. Any other is decoded into a buffer first.
 */
export function readsInPlace(file: SoundFile): boolean {
    return file.path !== null && STREAMED.includes(String(file.codec));
}

/** Why a verb that acts on a file is refused by an editor with none open. */
const NO_FILE = "no file is open";

/** What a run's own error throws, by the coordinator's name for it. */
const RAISED: Record<string, (message: string) => Error> = {
    value: (message) => new RangeError(message),
    command: (message) => new CommandError(message),
    runtime: (message) => new Error(message),
};

/** One answer of the coordinator: a run's effects, and its await or its end. */
interface RunAnswer {
    run?: number;
    done?: boolean;
    effects?: Record<string, any>[];
    await?: Record<string, any>;
    result?: any;
    changed?: boolean;
    reason?: string | null;
    rethrow?: boolean;
    error?: { type: string; message: string } | string;
}

/**
 * **What a page reads of one audio editor's playback**, which the shared
 * crate's coordinator holds: the transport it plays on and its state. One per
 * editor, shared by its open files: one file plays at a time and all others
 * stay paused, and playing one window moves nothing in another.
 */
class AudioPlayback {
    readonly #editor: AudioEditor;

    constructor(editor: AudioEditor) {
        this.#editor = editor;
    }

    /** The transport it plays on, once the editor is open. */
    get transportId(): number | null {
        return this.#editor.transportId;
    }

    /** `{playing, meters, rolling, focus}`, as the playback was last told. */
    async state(): Promise<Record<string, any>> {
        await this.#editor.idle();
        return this.#editor.transportState().state as Record<string, any>;
    }
}

/**
 * **What an audio editor's `Transport` plays through**: the coordinator's
 * transport verbs, its positions the take's seconds, and the time range and
 * the loop switch the window shows -- the band a sweep leaves and `L` -- kept
 * there, stopped or rolling, so a page and a hand read and write one state.
 */
class AudioDriver implements TransportDriver {
    private readonly editor: AudioEditor;

    constructor(editor: AudioEditor) {
        this.editor = editor;
    }

    /** The time range `[start, end]` in seconds, or `null`. */
    get span(): [number, number] | null {
        const span = this.editor.transportState().span as [number, number] | null;
        return span === null ? null : [Number(span[0]), Number(span[1])];
    }

    /** Whether the loop switch is on: the span, or the whole take. */
    get looping(): boolean {
        return this.editor.transportState().looping === true;
    }

    async playing(): Promise<boolean> {
        return (await this.editor.audio("playing")) === true;
    }

    async play(at?: number): Promise<void> {
        await this.editor.audio("play", at === undefined ? {} : { at });
    }

    async pause(): Promise<void> {
        await this.editor.audio("pause");
    }

    /**
     * Stop whichever file plays, from the window showing another file too,
     * and bring the file shown into focus at its cursor.
     */
    async stop(): Promise<void> {
        await this.editor.audio("stop");
    }

    async locate(at: number): Promise<void> {
        await this.editor.audio("locate", { at });
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
        await this.editor.audio("span", { show, span: span === null ? null : [span[0], span[1]] });
    }

    async setLooping(on: boolean): Promise<void> {
        await this.editor.audio("looping", { on });
    }
}

/**
 * A take made of parts: the crate's `parts` vocabulary.
 *
 * **What a gesture does is the crate's**; what is left is carrying out the
 * steps it answers, on the take's server, and freeing the takes nothing reaches
 * any more.
 */
export class AudioDomain extends Domain<Buffer | null> {
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
    current(_structure: unknown, _payload: unknown): unknown {
        return null;
    }

    /** Nothing: the parts vocabulary is carried out as steps ({@link run}). */
    project(_structure: unknown, _payload: unknown): boolean {
        return false;
    }

    /** Walk `steps` against the take's server. Answers whether there were any. */
    run(_structure: unknown, steps: unknown[]): boolean {
        const editor = this.editor;
        if (editor === null || steps.length === 0) return false;
        this.#queue(() => {
            editor.checkServer();
            return runSteps(editor.server, editor.runner, steps as never);
        });
        return true;
    }

    executeOperation(_structure: unknown, prepared: Record<string, unknown>, complete: (result: Record<string, any>) => void): void {
        this.#queue(async () => {
            if (this.editor === null) throw new Error("the operation owner is absent");
            complete(await this.editor.operation(null, prepared));
        });
    }

    async executeBatch(_structure: unknown, server: { connection: number; generation: number }, steps: unknown[]): Promise<void> {
        const editor = this.editor;
        if (editor === null) throw new Error("the operation owner is absent");
        editor.checkServer();
        const instance = serviceInstance(editor.server);
        if (server.connection !== instance.connection || server.generation !== instance.generation) throw new Error("the batch belongs to another server instance");
        await executeSteps(editor.server as unknown as Server, server, steps);
    }

    /** Keep a stepped object's playback current even when another file is viewed. */
    operationReturned(_structure: unknown, server: { connection: number; generation: number }, buffers: number[]): void {
        if (this.editor !== null) this.editor.returned(server, buffers);
    }

    operationEcho(_structure: unknown, answer: unknown): void {
        this.editor?.echo.send(answer as never);
    }

    soundFile(_structure: unknown, file: Record<string, unknown>): void {
        this.#queue(async () => { await this.editor?.carry("syncSound", { file }); });
    }

    /** Free buffers the context handed back: nothing reaches them any more. */
    free(_structure: unknown, buffers: number[], _spilled: number[] = []): void {
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
     * Delete renders the context handed back: no list, no entry and no
     * clipboard names them any more.
     */
    deleteFiles(_structure: unknown, files: string[]): void {
        this.#queue(() => removeFiles(files));
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
        this.#work = this.#work.catch(() => {}).then(async () => then());
        void this.#work.catch(() => {});
    }

    /**
     * The work a run's await sets off, carried out in order now rather than
     * after the run: a history step that restores a take, a release that
     * stores one. Set while {@link inline} runs.
     */
    #inline: Promise<void> | null = null;

    /**
     * **Run `body`, and the work it sets off, before going on**: a run waiting
     * on a history step or a release resumes once the takes it restored or
     * stored have landed, as it does where those calls return when done.
     *
     * @internal
     */
    async inline<T>(body: () => T): Promise<T> {
        const previous = this.#inline;
        this.#inline = Promise.resolve();
        try {
            const result = body();
            let landed: Promise<void> | null;
            do {
                landed = this.#inline;
                await landed;
            } while (landed !== this.#inline);
            return result;
        } finally {
            this.#inline = previous;
        }
    }

    /**
     * One piece of the domain's work after the last -- inside a run's await,
     * after the last piece that await set off -- and a failure reported rather
     * than swallowed.
     */
    #queue(run: () => Promise<void>): void {
        if (this.#inline !== null) {
            this.#inline = this.#inline.then(run);
            return;
        }
        this.#work = this.#work.catch(() => {}).then(run);
        void this.#work.catch(() => {});
    }
}

/** Removes each of `files`; one that is not there is no error. */
async function removeFiles(files: readonly string[] | undefined): Promise<void> {
    for (const path of files ?? []) await disk.remove(path);
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
 *
 * **An editor with no file open** is the application on its own:
 * `new AudioEditor(null, { server })` opens one, and closing its last file
 * leaves it so. Its window keeps the menu bar and the file list and offers
 * File -> New file and File -> Open file; a verb that acts on a file throws a
 * `RangeError` until one is added ({@link AudioEditor.newFile},
 * {@link AudioEditor.openFile}).
 */
export class AudioEditor extends Editor<Buffer | null> {
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

    /** What a page reads of the playback the coordinator holds. */
    private readonly playback: AudioPlayback;

    /**
     * The transport the editor plays on, once its open run has allocated it.
     *
     * @internal
     */
    transportId: number | null = null;

    /** The {@link SoundFile} the take was loaded from, when it was. */
    readonly #file: SoundFile | null;

    /** The folder the renders are written into, and whether it is this
     *  editor's to remove. */
    readonly #workspace: string;
    readonly #ownsWorkspace: boolean;

    constructor(
        given: Buffer | SoundFile | null,
        options: Omit<AudioEditorOptions, "sampleRate"> & { sampleRate?: number } = {},
    ) {
        // **A file is read where it is**; one a part cannot read (a compressed
        // one) is decoded into a buffer first, which `edit` does, and the open
        // writes it to the working folder.
        let file = options.file ?? null;
        let take: Buffer | null = null;
        if (given instanceof SoundFile) {
            if (!readsInPlace(given)) {
                throw new RangeError(`${given.path ?? "a file not written yet"} is not read in place: ` +
                    "open it with `edit`, or pass a buffer of it with `file`");
            }
            file = given;
        } else {
            take = given;
        }
        const view = new SamplesView(options.layers ?? MEASURES);
        const domain = new AudioDomain();
        const { server, ...others } = options;
        const empty = take === null && !(given instanceof SoundFile);
        super(given as Buffer | null, {
            title: "Audio",
            ...others,
            // no data to reach a context through: the editor has its own
            context: others.context ?? (empty ? new Editing() : undefined),
            sampleRate: Number(options.sampleRate || given?.sampleRate || 48_000),
            domain,
            view,
        });
        domain.editor = this;
        this.#file = file;
        this.server = resolveServer(server ?? take?.server);
        this.instance = serviceInstance(this.server);
        this.display = this.server.buffers.alloc();
        // The context is the take's; switching files changes the structure,
        // so the window keeps the one it opened in.
        this.givenContext = this.editing;
        this.#ownsWorkspace = options.workspace === undefined;
        this.#workspace = options.workspace ?? temporaryFolder(`clausters-audio-${this.display}`);
        // the default folder is the editor's to make, as a script's temporary
        // one is; a folder handed over is the caller's
        if (this.#ownsWorkspace) domain.after(() => makeDirectoryAt(this.#workspace));
        const prefix = `audio:${this.instance.connection}:${this.instance.generation}`;
        // A file a part reads in place is the take, whatever buffer came with it.
        const streamed = file !== null && readsInPlace(file);
        const frames = Math.trunc((given?.frames ?? 0) || 0);
        const opened = this.editing.open(
            "openAudio",
            empty ? `${prefix}:empty:${this.display}`
                : `${prefix}:${streamed ? `file:${file!.path}` : Math.trunc(take!.bufnum)}`,
            {
                ...this.facts(),
                ...(empty ? { empty: true } : {
                    frames,
                    channels: Math.max(1, Math.trunc(given?.channels || 1)),
                    rate: this.sampleRate,
                    path: file?.path ?? null,
                    untitled: file !== null && file.path === null,
                    codec: file?.codec, sample_format: file?.sampleFormat,
                    // a buffer, or a file a part cannot read, is written to
                    // the working folder by the open
                    ...(streamed ? {} : { take: Math.trunc(take!.bufnum) }),
                }),
                display: this.display,
                server: this.instance,
                workspace: this.#workspace,
                sharedHistory: options.sharedHistory ?? false,
                layers: view.layers,
            },
            given ?? this,
            domain,
        );
        this.member = opened.member;
        this.structureId = opened.identity;
        this.syncCore();
        this.playback = new AudioPlayback(this);
        this.#driver = new AudioDriver(this);
        // **The join over the take**: what the editor was handed is written
        // by a save and by nothing else. Then it sounds, on a transport of its
        // own.
        domain.after(async () => {
            // Node ids come back on their `/node_end`, which only a registered
            // client hears.
            await this.server.notify(true);
            let rate = 0;
            try {
                rate = (await this.server.queryInfo()).nominalSampleRate;
            } catch {
                rate = 0;
            }
            const opened = await this.carry("open", {
                chunk: await this.server.bulkChunk(), rate, empty,
            });
            this.transportId = Number(opened.transport);
        });
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
        const id = this.transportId;
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

    /** Add and focus an empty SoundFile of the requested shape. */
    async newFile(channels: number, frames: number, rate: number): Promise<void> {
        await this.audio("newFile", { shape: { channels, frames, rate } });
    }

    /** Open a path or an array of paths in this editor, focusing the last. */
    async openFile(paths: string | readonly string[]): Promise<void> {
        await this.audio("openFile", { paths: typeof paths === "string" ? paths : [...paths] });
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
        await this.audio("verb", { call: { verb: "focus_file", index } });
    }

    /** Close the focused file, or the file at zero-based index. */
    async closeFile(index?: number): Promise<void> {
        await this.audio("verb", { call: { verb: "close_file", ...(index === undefined ? {} : { index }) } });
    }

    /** Show the independent file list to the left of the editing view. */
    showFiles(): void { this.started("verb", { call: { verb: "show_files" } }); }

    /** Hide the file list. */
    hideFiles(): void { this.started("verb", { call: { verb: "hide_files" } }); }

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
     * **A copy published**: the clipboard names the spans of files it is of,
     * and the host's clipboard holds the document.
     */
    private clipboard(doc: string, ready = false): void {
        this.checkServer();
        if (ready) bindExchange(this.server, this.editing.serviceCore);
        else doc = exchange(this.server, "publish", { document: doc }).document as string;
        this.host?.clipboard(doc);
    }

    /**
     * **The file in focus, bound**: its buffers, its structure in the history,
     * and the cursor, selection and layers it left -- or, with `null`, no
     * file: the last one was closed.
     */
    private bindFile(binding: Record<string, unknown> | null): void {
        if (binding === null) {
            this.structure = null;
            this.cursor = null;
            return;
        }
        this.display = Number(binding.display);
        this.sampleRate = Number(binding.rate);
        this.structure = new Buffer(
            Number(binding.take),
            Number(binding.frames),
            Number(binding.channels),
            this.sampleRate,
            this.server,
        );
        this.structureId = Number(binding.structure);
        this.editing.version = Number(binding.version);
        this.cursor = Number(binding.cursor) / this.sampleRate;
        (this.view as SamplesView).layers = binding.layers as Measure[];
    }

    /** The work queued so far, landed. @internal */
    idle(): Promise<void> {
        return (this.domain as AudioDomain).idle();
    }

    /** What the coordinator holds of the transport. @internal */
    transportState(): Record<string, unknown> {
        this.checkServer();
        return this.callAudio({ query: "transport" }) as Record<string, unknown>;
    }

    private callAudio(request: Record<string, unknown>): RunAnswer {
        return JSON.parse(this.editing.serviceCore.callIds(
            JSON.stringify({ verb: "audio", member: this.member, ...request }), this.server.ids,
        )) as RunAnswer;
    }

    /** One action of the coordinator, started: its first answer. */
    private begin(verb: string, args: Record<string, unknown>): RunAnswer {
        this.checkServer();
        return this.callAudio({
            do: { verb, window: this.host !== null && this.window !== null, ...args },
        });
    }

    /**
     * **A started action, carried out to its end**: its effects performed in
     * order, each await answered, and its result resolved -- or what failed
     * thrown. `held` is the clipboard lease an event's payload came with,
     * which the run says when to release.
     */
    private async finish(first: RunAnswer, held?: ExchangeLease): Promise<any> {
        let answer = first;
        let failure: unknown = undefined;
        for (;;) {
            if (answer.run === undefined) {
                throw new Error(String(answer.error ?? "the audio editor did not answer"));
            }
            for (const effect of answer.effects ?? []) await this.effect(effect, held);
            this.publishActions();
            if (answer.done === true) {
                if (answer.rethrow === true && failure !== undefined) throw failure;
                const error = answer.error;
                if (error !== undefined && typeof error === "object") {
                    throw (RAISED[error.type] ?? RAISED.runtime!)(error.message);
                }
                return answer.result;
            }
            let result: Record<string, unknown>;
            try {
                result = await this.answerAwait(answer.await!);
            } catch (error) {
                failure = error;
                result = { failed: error instanceof Error ? error.message : String(error) };
            }
            answer = this.callAudio({ run: answer.run, result });
        }
    }

    /** Run an action now, from inside the queued work. @internal */
    async carry(verb: string, args: Record<string, unknown> = {}, held?: ExchangeLease): Promise<any> {
        return this.finish(this.begin(verb, args), held);
    }

    /** Run an action once the work queued before it has landed. @internal */
    audio(verb: string, args: Record<string, unknown> = {}): Promise<any> {
        return new Promise((resolve, reject) => {
            (this.domain as AudioDomain).after(async () => {
                try {
                    resolve(await this.carry(verb, args));
                } catch (error) {
                    reject(error);
                }
            });
        });
    }

    /**
     * **An action started now**: the effects it decided before its first await
     * are carried out at once, and the rest once the work queued before it has
     * landed. What the page owes at once (whether the take changed, why a verb
     * refused) is read off its first answer, and an error it ends in at once is
     * thrown here.
     */
    private started(verb: string, args: Record<string, unknown> = {}, held?: ExchangeLease): RunAnswer {
        const answer = this.begin(verb, args);
        const landing = (answer.effects ?? []).map((effect) => this.effect(effect, held));
        this.publishActions();
        (this.domain as AudioDomain).after(async () => {
            await Promise.all(landing);
            await this.finish({ ...answer, effects: [] }, held);
        });
        const error = answer.error;
        if (answer.done === true && error !== undefined && typeof error === "object") {
            throw (RAISED[error.type] ?? RAISED.runtime!)(error.message);
        }
        return answer;
    }

    /** What a run waits on, done here: the page's own work. */
    private async answerAwait(request: Record<string, any>): Promise<Record<string, unknown>> {
        switch (request.kind) {
            case "steps": {
                const steps = (request.steps ?? []) as unknown[];
                if (request.via === "service") {
                    this.checkServer();
                    await executeSteps(this.server as unknown as Server, this.instance, steps);
                } else {
                    this.checkServer();
                    await runSteps(this.server, request.via === "plain" ? new StepRunner() : this.runner, steps as never);
                }
                return {};
            }
            case "rolling":
                return { rolling: (await this.server.transportAt(Number(request.transport)).state()).playing };
            case "shape":
                return this.shape(request);
            case "batch":
                await this.editing.executeBatch(Number(request.member), request.server, request.steps ?? []);
                return {};
            case "stepped":
                return {
                    changed: await (this.domain as AudioDomain).inline(
                        () => this.app.stepped(this.editing, request.stepped ?? {}, this)),
                };
            case "clipboard":
                this.clipboard(String(request.document), request.ready === true);
                return {};
            default:
                throw new Error(`the audio editor cannot wait on ${String(request.kind)}`);
        }
    }

    /**
     * **A file asked for in the window or by a page**: its header read, and a
     * display buffer for its join. A new file is its shape; a file a part
     * cannot read is decoded into a buffer the open writes out.
     */
    private async shape(request: Record<string, any>): Promise<Record<string, unknown>> {
        const shape = request.shape as { channels: number; frames: number; rate: number } | undefined;
        const file = shape !== undefined
            ? SoundFile.empty(shape.channels, shape.frames, shape.rate)
            : await SoundFile.open(String(request.path));
        const answer: Record<string, unknown> = {
            frames: file.frames, channels: file.channels, rate: file.sampleRate,
            display: this.server.buffers.alloc(), path: file.path, untitled: file.path === null,
            codec: file.codec, sample_format: file.sampleFormat,
        };
        if (file.path !== null && !readsInPlace(file)) {
            answer.take = (await file.toBuffer({ server: this.server })).bufnum;
        }
        return answer;
    }

    /** One effect of a run, carried out here. */
    private async effect(effect: Record<string, any>, held?: ExchangeLease): Promise<void> {
        const shown = this.host !== null && this.window !== null;
        switch (effect.kind) {
            case "version":
                if (effect.version !== undefined && effect.version !== null) this.editing.version = Number(effect.version);
                break;
            case "echo":
                this.echo.send(effect.answer as never);
                break;
            case "set":
                if (shown) await this.host!.set(Number(effect.widget), effect.props);
                break;
            case "publish":
                if (this.windowId !== null) this.app.publish(this.windowId, this.draw());
                break;
            case "band":
                this.showBand(Number(effect.sel_start), Number(effect.sel_len));
                break;
            case "looping":
                this.showLooping(effect.on === true);
                break;
            case "bind":
                this.bindFile((effect.binding ?? null) as Record<string, unknown> | null);
                break;
            case "layers":
                (this.view as SamplesView).layers = effect.layers as Measure[];
                break;
            case "cursor":
                this.cursor = Number(effect.at);
                break;
            case "locate":
                this.cursor = Number(effect.at);
                this.locate(this.cursor);
                this.composedIn?.locate(this.cursor);
                this.onLocate?.(this.cursor);
                break;
            case "changed":
                // the gesture's change has landed: the other windows are told now
                this.editing.turn(this, () => this.editing.changed());
                break;
            case "free":
                await (this.domain as AudioDomain).cleanup((effect.buffers ?? []) as number[]);
                break;
            case "release":
                await (this.domain as AudioDomain).inline(
                    () => this.editing.release(effect.freed as never, effect.stored as never));
                break;
            case "delete":
                await removeFiles(effect.files as string[] | undefined);
                break;
            case "returned":
                this.editing.operationReturned(effect.returned as never);
                break;
            case "operationEcho":
                this.editing.operationEcho(effect.echo as never);
                break;
            case "bufferFree": {
                const current = serviceInstance(this.server);
                if (current.connection === this.instance.connection && current.generation === this.instance.generation) {
                    const buffer = Number(effect.buffer);
                    void this.server.sendMsg("/buffer_free", ["i", buffer]);
                    this.server.buffers.free(buffer);
                }
                break;
            }
            case "lease":
                if (held !== undefined) exchange(this.server, "release", { lease: held });
                break;
            case "closeWindow":
                super.close();
                break;
            case "closedWindow":
                super.closedWindow();
                break;
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
        if (this.transportId !== null) this.host?.headClock(handle, "transport", this.transportId);
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
        this.started("closed");
        return false;
    }

    /**
     * Close the window, and free what sounded the take. A working folder the
     * editor made goes with it, its renders deleted by the close -- all but the
     * ones the clipboard still names, which stay where they are.
     */
    override close(): this {
        this.started("close");
        if (this.#ownsWorkspace) {
            const folder = this.#workspace;
            (this.domain as AudioDomain).after(async () => {
                await removeDirectoryAt(`${folder}/peaks`, { recursive: true });
                await removeDirectoryAt(folder);
            });
        }
        return this;
    }

    /** What this page holds about the take and the window. */
    private facts(): Record<string, unknown> {
        const take = this.structure;
        const name = (take as { name?: string } | null)?.name;
        return {
            channels: Math.max(1, Math.trunc(take?.channels || 1)),
            name: typeof name === "string" && name ? name : null,
            rate: this.sampleRate,
            title: this.title,
            w: this.size[0],
            h: this.size[1],
            chrome: this.chrome,
            window: this.windowId,
        };
    }


    /** This editor's member, whose actions its window reads. */
    protected override actionMember(): number {
        return this.member;
    }

    /**
     * One verb of the core, with its arguments; the answer, parsed.
     *
     * @internal
     */
    coreCall(verb: string, args: Record<string, unknown> = {}): Record<string, unknown> {
        this.checkServer();
        const answer = this.editing.member(this.member, verb, args);
        this.publishActions();
        if (answer.error === NO_FILE) throw new RangeError(NO_FILE);
        return answer;
    }

    /**
     * Hand the core the chrome and the window it is open in.
     *
     * @internal
     */
    syncCore(): void {
        const facts = Object.fromEntries(Object.entries(this.facts()).filter(([, v]) => v !== null && v !== undefined));
        this.coreCall("sync", facts);
    }

    /** Refuse stale resources before addressing the replacement server. @internal */
    checkServer(): void {
        const current = serviceInstance(this.server);
        if (current.connection !== this.instance.connection || current.generation !== this.instance.generation) {
            this.editing.serviceCall("invalidate", { server: this.instance });
            throw new Error("the audio service belongs to a previous server instance");
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
            Math.max(1, Math.trunc(this.structure?.channels || 1)),
            this.sampleRate,
            this.server,
        );
    }

    /** Paste onto what is there, mixed with it -- the window's `mix` ({@link Editor.verb}). */
    mix(): boolean {
        return this.verb("mix");
    }

    /**
     * Play the selection backwards, or the whole take with nothing selected
     * -- rendered into a file the list then names. The window's `reverse`
     * ({@link Editor.verb}).
     */
    reverse(): boolean {
        return this.verb("reverse");
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
        await this.audio("retry");
    }

    /**
     * Execute typed batches; commit, refusal and cleanup come from Rust. Called
     * from inside the queued work.
     *
     * @internal
     */
    async operation(command: Record<string, unknown> | null, prepared?: Record<string, unknown>, repairing = false): Promise<Record<string, any>> {
        const result = await this.carry("operation", { command, prepared: prepared ?? null, repairing });
        return result.completed as Record<string, any>;
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
        await this.idle();
        this.syncCore();
        const answer = (await this.audio("save", { request })).destination;
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
     * What the take is made of now: one object per span of a file, in reading
     * order -- its `path`, the `start` and the `frames` of it read, and its
     * fades and shape.
     */
    get parts(): Record<string, unknown>[] {
        return [...((this.coreCall("segments").parts as Record<string, unknown>[] | undefined) ?? [])];
    }

    // ---- the crate's turns ----

    protected override deliver(addr: string, rawArgs: readonly unknown[]): boolean {
        this.syncCore();
        const prepared = addr === "/gui_event"
            ? exchange(this.server, "event", { args: plain([...rawArgs]) })
            : { args: plain([...rawArgs]) };
        const lease = prepared.lease as ExchangeLease | undefined;
        const answer = this.started("event", {
            addr, args: prepared.args, ...(lease === undefined ? {} : { lease: true }),
        }, lease);
        return answer.changed === true;
    }

    /** One `/gui_event` payload, with the stamp already taken off. */
    protected override route(args: readonly unknown[]): boolean {
        const [wid, tag, ...values] = args;
        return this.deliver("/gui_event", [wid, 0, 0, tag, ...values]);
    }

    /**
     * Draw what a history walk left behind -- once the join the walk stitched
     * again has landed, since what the window is told is to read it again. A
     * step of a shared order may restore another editor's file, carried out
     * on that editor's queue, so the window waits for every member's work
     * queued so far: told sooner, it would name a version the step's commit
     * then moves past, and its next gesture would be refused as stale.
     */
    override reflectStep(): void {
        const elsewhere = this.editing.landed();
        (this.domain as AudioDomain).after(async () => {
            await elsewhere;
            super.reflectStep();
        });
    }

    /**
     * **Whether the file in focus has edits its file does not hold**: the
     * editor's own reading, asked each time. A save that has not completed
     * leaves it dirty; an undo back to what was written leaves it clean.
     */
    override get dirty(): boolean {
        if (this.member === undefined) return false;
        return this.editing.member(this.member, "binding").dirty === true;
    }

    /** Nothing: the base editor's mark of a change since the last render, which an audio editor answers from the file instead. */
    override set dirty(_value: boolean) {}

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
        this.started("verb", { call: { verb, version: this.editing.version, ...args } });
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
    freePreview(): void { this.envelopeAct("free_preview"); }

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
        const answer = this.started("verb", { call: { verb, version: this.editing.version, ...args } });
        if (answer.reason) throw new RangeError(answer.reason);
    }

    private envelopeAct(verb: string, args: Record<string, unknown> = {}): boolean {
        return this.editing.turn(this, () => {
            this.syncCore();
            const answer = this.started("act", { call: plain({ verb, ...args }) });
            if (answer.reason) throw new RangeError(answer.reason);
            return answer.changed === true;
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

/** {@link AudioEditor}'s options: the generic ones, the measures and the folder. */
export interface AudioEditorOptions extends GenericEditorOptions<Buffer | null> {
    /**
     * The server an editor with no file opens on; the ambient one when absent.
     * An editor over a take uses the take's.
     */
    server?: Server;
    /**
     * Whether the take's undo order is the editing context's shared one, which
     * the other members of `context` walk too. By default each file the editor
     * holds has an order of its own, whatever was opened before it.
     */
    sharedHistory?: boolean;
    /** What the picture measures. Defaults to {@link MEASURES}. */
    layers?: string | readonly Measure[];
    /**
     * The directory the editor renders its files into, on the server's
     * filesystem -- the page's own storage, in a tab. A temporary one when not
     * given, removed when the editor closes; one handed over is left where it
     * is.
     */
    workspace?: string;
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
