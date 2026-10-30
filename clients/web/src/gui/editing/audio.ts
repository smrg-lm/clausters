/**
 * The **audio editor**: a sound file edited as a list of parts over immutable
 * files, by the application the GUI host holds.
 *
 * The editor is one program, and it is not this one. The GUI host opens it on
 * a file ({@link GuiHost.openApp}), in a window of the host's, on an engine of
 * the application's own: what a gesture does to the take, the renders that
 * make new samples, the history, the working folder and the playback all run
 * in the host's process. This object is its **handler**: every member below
 * is one call to the application ({@link GuiHost.callApp}), and what it
 * answers is read from there each time rather than kept here. A read is
 * therefore a promise here where the reference client's is a property.
 *
 * What it opens is **a sound file**, read where it is; nothing it was handed
 * is written until it is saved. A cut takes a span out of the list, a paste
 * puts the copied spans in, and an edit that makes new samples (a pencil
 * stroke, a mix, a gain curve applied, a reverse) renders them into a file of
 * the application's working folder, which the list then names. Nothing a
 * history entry names is written again, so an undo is the list before.
 *
 * @module
 */

import { SoundFile } from "../../soundfile.ts";
import { Segment } from "../../segments.ts";
import type { GuiHost, Stage } from "../host.ts";
import type { WindowHandle } from "../handle.ts";

/** The containers a part of a list reads in place. */
const STREAMED = ["wav", "w64", "rf64"];

/** Why a verb that acts on a file is refused by an editor with none open. */
const NO_FILE = "no file is open";

/**
 * Whether the audio editor reads `file` where it is: a PCM file on disk a
 * part streams from. Any other file on disk (an MP3, a FLAC) is decoded
 * into the editor's working folder when it opens, and its first save asks
 * for a path.
 */
export function readsInPlace(file: SoundFile): boolean {
    return file.path !== null && STREAMED.includes(String(file.codec));
}

/**
 * **The audio editor's transport**, as a script plays it: the verbs and the
 * span a server's `Transport` has, in the take's seconds, each one a call to
 * the application. The span is the band a drag sweeps and the loop switch is
 * `L`, so a script and a hand read and write one state. It is the editor's
 * own: another window's play never moves it.
 */
export class AudioTransport {
    private readonly editor: AudioEditor;

    constructor(editor: AudioEditor) {
        this.editor = editor;
    }

    private async state(): Promise<Record<string, any>> {
        return this.editor.query("transport");
    }

    /** Whether the take is playing. */
    async playing(): Promise<boolean> {
        return Boolean(await this.editor.act("playing"));
    }

    /** Roll, from `at` seconds when given, else from the cursor. */
    async play(at?: number): Promise<this> {
        await this.editor.act("play", at === undefined ? {} : { at: Number(at) });
        return this;
    }

    /** Hold where it is; a `play` goes on from there. */
    async pause(): Promise<this> {
        await this.editor.act("pause");
        return this;
    }

    /** Stop, and bring the file shown into focus at its cursor. */
    async stop(): Promise<this> {
        await this.editor.act("stop");
        return this;
    }

    /** Move the cursor to `at` seconds. */
    async locate(at: number): Promise<this> {
        await this.editor.act("locate", { at: Number(at) });
        return this;
    }

    /** The time range `[start, end]` in seconds, or `null`. */
    async span(): Promise<[number, number] | null> {
        const span = (await this.state()).span as [number, number] | null | undefined;
        return span === null || span === undefined ? null : [Number(span[0]), Number(span[1])];
    }

    /** Set the time range `[start, end]` in seconds, or `null` for none. */
    async setSpan(span: readonly [number, number] | null): Promise<void> {
        await this.editor.act("span", {
            show: true,
            span: span === null ? null : [Number(span[0]), Number(span[1])],
        });
    }

    /** Whether the loop switch is on: over the span, or the whole take. */
    async looping(): Promise<boolean> {
        return (await this.state()).looping === true;
    }

    /**
     * **Loop**: with `start` and `end`, set the span to them first; then turn
     * the loop on, over the span or the whole take.
     */
    async loop(start?: number, end?: number): Promise<this> {
        if (start !== undefined && end !== undefined) await this.setSpan([start, end]);
        await this.editor.act("looping", { on: true });
        return this;
    }

    /** Turn the loop off; the span stays. */
    async unloop(): Promise<this> {
        await this.editor.act("looping", { on: false });
        return this;
    }

    /**
     * Resolves when the take stops playing, or on `timeout` seconds: `true`
     * for the first, `false` for the second.
     */
    async wait(timeout?: number): Promise<boolean> {
        const deadline = timeout === undefined ? null : Date.now() + timeout * 1000;
        while (await this.playing()) {
            if (deadline !== null && Date.now() >= deadline) return false;
            await new Promise((resolve) => setTimeout(resolve, 50));
        }
        return true;
    }
}

/** **How the audio editor's spectrogram analyzes and draws the take**: see {@link AudioEditor.spectrogram}. */
export interface SpectrogramSettings {
    /** The window, in frames: 256, 512, 1024, 2048 or 4096. */
    window: number;
    /** The window's shape: `"hann"`, `"hamming"`, `"blackman"`, `"sine"`, `"welch"` or `"rectangular"`. */
    shape: string;
    /** How much a window overlaps the next: 0, 0.5, 0.75, 0.875 or 0.9375, each halving the hop. */
    overlap: number;
    /** The frequency axis: `"linear"`, `"log"`, `"mel"`, `"bark"` or `"note"`. */
    scale: string;
    /** The A4 of a note axis, in hertz. */
    tuning: number;
    /** The colours: `"viridis"`, `"magma"` or `"gray"`. */
    colormap: string;
    /** The levels the colours span below their top, in decibels: 20 to 120. */
    range: number;
    /** How far the top of the colours is below full scale, in decibels: 0 to 60. */
    gain: number;
    /** Drawn smooth between the bins and the columns, rather than in blocks of measured magnitudes. */
    interpolate: boolean;
    /** Each frequency band at its own scale. */
    normalize: boolean;
}

/** How an {@link AudioEditor} opens. */
export interface AudioEditorOptions {
    /** The window's title. */
    title?: string;
    /** What the picture measures, innermost last; the application's own when not given. */
    layers?: readonly unknown[];
}

/**
 * **The audio editor the GUI host holds, and this script its handler.**
 *
 * `edit` over a {@link SoundFile} answers one, open. Each member is a call to
 * the application in the host's process, answered by it: a member that edits
 * resolves once the edit is made, renders and all, and one that reads asks
 * the application again each time.
 *
 * **What is marked** (`selected`, `select`, `unselect`) is the samples a drag
 * over the take covers, as a {@link Segment} over `file` (its `start` a frame
 * and its `duration` seconds), or `null`. What is marked and the transport's
 * span are one range.
 *
 * **An editor with no file open** is the application on its own:
 * `new AudioEditor()` opens one, and closing its last file leaves it so. A
 * verb that acts on a file rejects with a `RangeError` until one is added
 * (`newFile`, `openFile`).
 */
export class AudioEditor {
    /** The window's title. */
    readonly title: string;
    /** The file it opens on, which `edit` finds an open editor by. */
    readonly opening: SoundFile | null;
    private readonly openLayers: readonly unknown[] | null;
    private hostOf: GuiHost | null = null;
    private handle: WindowHandle | null = null;

    constructor(file: SoundFile | null = null, { title = "Audio", layers }: AudioEditorOptions = {}) {
        this.opening = file;
        this.title = String(title);
        this.openLayers = layers ?? null;
    }

    // ---- the window ----

    /**
     * **Ask the host to open the editor**, on `host` or the ambient one, and
     * answer its window. An editor already open answers the window it has.
     */
    async open(host?: GuiHost, { stage }: { stage?: Stage | null } = {}): Promise<WindowHandle> {
        if (this.handle !== null) return this.handle;
        const resolved = host ?? (await (await import("../../plot.ts")).resolveHost());
        const options: Record<string, unknown> = { title: this.title };
        if (this.opening !== null && this.opening.path !== null) options.path = this.opening.path;
        const window = await resolved.openApp("audio", options, 5.0, { element: stage });
        const { WindowHandle } = await import("../handle.ts");
        this.hostOf = resolved;
        this.handle = new WindowHandle(resolved, window, new Map());
        // silence not written anywhere yet: a new file of its shape
        if (this.opening !== null && this.opening.path === null) {
            const { channels, frames, sampleRate } = this.opening;
            await this.newFile(channels, frames, sampleRate);
        }
        if (this.openLayers !== null) await this.setLayers(this.openLayers);
        return this.handle;
    }

    /** The {@link GuiHost} the editor is open on, or `null`. */
    get host(): GuiHost | null {
        return this.hostOf;
    }

    /** The open window's handle, or `null`. */
    get window(): WindowHandle | null {
        return this.handle;
    }

    /** The open window's id, or `null`. */
    get id(): number | null {
        return this.handle?.id ?? null;
    }

    /** Whether the editor's window is gone. */
    get closed(): boolean {
        return this.handle === null || this.handle.closed;
    }

    /**
     * Resolves when the editor's window is closed, or on `timeout` seconds:
     * `true` for the first, `false` for the second.
     */
    wait(timeout?: number): Promise<boolean> {
        return this.handle === null ? Promise.resolve(true) : this.handle.wait(timeout);
    }

    /** Call `handler()` when the editor's window is closed. `null` clears it. */
    onClosed(handler: (() => void) | null): this {
        this.opened().onClosed(handler);
        return this;
    }

    /**
     * Close the editor, as File -> Close does: a file with edits it does not
     * hold asks first, in the window.
     */
    async close(): Promise<this> {
        if (!this.closed) await this.act("close");
        return this;
    }

    private opened(): WindowHandle {
        if (this.handle === null) throw new Error("open() the editor first");
        return this.handle;
    }

    // ---- the door ----

    /** One call to the application, and its answer. */
    async call(call: Record<string, unknown>, timeout: number | null = null): Promise<any> {
        const window = this.opened();
        return (this.hostOf as GuiHost).callApp(window, call, timeout);
    }

    /** One verb of the editor; a `RangeError` when it needs a file and none is open. */
    async member(verb: string, args: Record<string, unknown> = {}): Promise<Record<string, any>> {
        const answer = ((await this.call({ member: { verb, ...args } })) ?? {}) as Record<string, any>;
        if (answer.error === NO_FILE) throw new RangeError(NO_FILE);
        return answer;
    }

    /** One of the editor's readings: `transport`, `actions` or `history`. */
    async query(name: string): Promise<Record<string, any>> {
        return ((await this.call({ query: name })) ?? {}) as Record<string, any>;
    }

    /** One action of the editor, carried to its end, and its result. */
    async act(verb: string, args: Record<string, unknown> = {}): Promise<any> {
        return this.call({ do: { verb, ...args } });
    }

    private async version(): Promise<number> {
        return Number((await this.query("history")).version ?? 0);
    }

    // ---- the window's verbs ----

    /**
     * **Perform the window's verb `name`**, as its key, its menu entry or its
     * tool would (`docs/verbs.md`). Answers whether it was asked: `false`
     * with no window open.
     */
    verb(name: string): boolean {
        if (this.closed) return false;
        (this.hostOf as GuiHost).verb(this.opened(), String(name));
        return true;
    }

    /**
     * **A hand played to the window**: a `"press"`, `"move"` or `"release"`
     * at `(x, y)` in its device pixels, with `mods` held. Answers whether it
     * was sent.
     */
    pointer(kind: "press" | "move" | "release", x: number, y: number, mods = ""): boolean {
        if (this.closed) return false;
        (this.hostOf as GuiHost).pointer(this.opened(), kind, x, y, mods);
        return true;
    }

    /** A press and a release at `(x, y)`, with `mods` held ({@link AudioEditor.pointer}). */
    click(x: number, y: number, mods = ""): boolean {
        return this.pointer("press", x, y, mods) && this.pointer("release", x, y, mods);
    }

    /**
     * **A drag from `start` to `end`**, each an `[x, y]` in the window's
     * device pixels: a press, `steps` moves and a release.
     */
    drag(start: [number, number], end: [number, number], mods = "", steps = 8): boolean {
        const [x0, y0] = start;
        const [x1, y1] = end;
        if (!this.pointer("press", x0, y0, mods)) return false;
        const n = Math.max(Math.trunc(steps), 1);
        for (let i = 1; i <= n; i++) {
            const t = i / n;
            this.pointer("move", x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, mods);
        }
        return this.pointer("release", x1, y1, mods);
    }

    /** **Write the window's picture to `path`** as a PNG, and answer the path. */
    capture(path: string, timeout = 5.0): Promise<string> {
        return (this.hostOf as GuiHost).capture(this.opened(), path, timeout);
    }

    /** **What the window costs, and why** ({@link GuiHost.activity}). */
    activity(timeout = 1.0): ReturnType<GuiHost["activity"]> {
        return (this.hostOf as GuiHost).activity(this.opened(), timeout);
    }

    /** **What the window costs over the next `seconds`** ({@link GuiHost.profile}). */
    profile(seconds = 5.0, timeout = 1.0): ReturnType<GuiHost["profile"]> {
        return (this.hostOf as GuiHost).profile(this.opened(), seconds, timeout);
    }

    /** The whole take in view: the window's `view_all` ({@link AudioEditor.verb}). */
    viewAll(): boolean { return this.verb("view_all"); }

    /** Cut what is selected: the window's `cut`. */
    cut(): boolean { return this.verb("cut"); }

    /** Copy what is selected: the window's `copy`. */
    copy(): boolean { return this.verb("copy"); }

    /** Paste at the cursor: the window's `paste`. */
    paste(): boolean { return this.verb("paste"); }

    /** Delete what is selected: the window's `delete`. */
    delete(): boolean { return this.verb("delete"); }

    /** Select the whole take: the window's `select_all`. */
    selectAll(): boolean { return this.verb("select_all"); }

    /** The cursor to the start: the window's `to_start`. */
    toStart(): boolean { return this.verb("to_start"); }

    /** The cursor to the end: the window's `to_end`. */
    toEnd(): boolean { return this.verb("to_end"); }

    /** The loop switch, on or off: the window's `loop`. */
    loop(): boolean { return this.verb("loop"); }

    /** Paste onto what is there, mixed with it: the window's `mix`. */
    mix(): boolean { return this.verb("mix"); }

    /**
     * Play the selection backwards, or the whole take with nothing selected,
     * rendered into a file the list then names: the window's `reverse`.
     */
    reverse(): boolean { return this.verb("reverse"); }

    // ---- the history: the order of the file in focus ----

    /** Step the file in focus back one edit. Answers whether anything was undone. */
    undo(): Promise<boolean> { return this.step("undo"); }

    /** Step the file in focus forward again. Answers whether anything was redone. */
    redo(): Promise<boolean> { return this.step("redo"); }

    private async step(direction: "undo" | "redo"): Promise<boolean> {
        const history = await this.query("history");
        if (history[direction === "undo" ? "canUndo" : "canRedo"] !== true) return false;
        await this.act("event", {
            addr: "/gui_event",
            args: [this.opened().id, 0, Number(history.version ?? 0), direction],
        });
        return true;
    }

    /** Whether the file in focus has an edit to step back over. */
    async canUndo(): Promise<boolean> {
        return (await this.query("history")).canUndo === true;
    }

    /** Whether the file in focus has an undone edit to step forward into. */
    async canRedo(): Promise<boolean> {
        return (await this.query("history")).canRedo === true;
    }

    /** What an undo of the file in focus would be called. */
    async undoLabel(): Promise<string | null> {
        return ((await this.query("history")).undoLabel as string | null | undefined) ?? null;
    }

    /** What a redo of the file in focus would be called. */
    async redoLabel(): Promise<string | null> {
        return ((await this.query("history")).redoLabel as string | null | undefined) ?? null;
    }

    /**
     * **The window's actions as they read now**, by verb: what each
     * addresses (`target`), whether it can be taken (`enabled`, and `reason`
     * when not), a switch's `checked`, and whether work it depends on is
     * `pending`.
     */
    actions(): Promise<Record<string, any>> {
        return this.query("actions");
    }

    // ---- the transport, and what is marked ----

    /**
     * **The take's transport**: `play`, `pause`, `stop`, `locate`, `loop`,
     * `wait` and the span, in the take's seconds.
     */
    get transport(): AudioTransport {
        return new AudioTransport(this);
    }

    /** The rate of the file in focus, in hertz. */
    async sampleRate(): Promise<number> {
        return Number((await this.member("binding")).rate ?? 0);
    }

    /**
     * Where the cursor of the file in focus is, in seconds, or `null` with no
     * file open.
     */
    async cursor(): Promise<number | null> {
        const binding = await this.member("binding");
        if (binding.cursor === null || binding.cursor === undefined || !binding.rate) return null;
        return Number(binding.cursor) / Number(binding.rate);
    }

    /** What is marked: a {@link Segment} over `file`, or `null`. */
    async selected(): Promise<Segment<SoundFile | null> | null> {
        const span = await this.transport.span();
        if (span === null) return null;
        const rate = await this.sampleRate();
        return new Segment(await this.file(), Math.round(span[0] * rate), span[1] - span[0]);
    }

    /**
     * Mark `marked`, a {@link Segment} (its `start` a frame, its `duration`
     * seconds); `null` marks nothing.
     */
    async select(marked: Segment<unknown> | null): Promise<void> {
        if (marked === null) return this.unselect();
        const start = Number(marked.start) / (await this.sampleRate());
        await this.transport.setSpan([start, start + Number(marked.duration)]);
    }

    /** Mark nothing. */
    async unselect(): Promise<void> {
        await this.transport.setSpan(null);
    }

    // ---- the files ----

    /**
     * **The {@link SoundFile} the take is**, at its length now: the file it
     * was opened from or last saved to, one with no path when it has not been
     * written yet, and `null` with no file open.
     */
    async file(): Promise<SoundFile | null> {
        const answer = await this.member("file");
        const path = typeof answer.path === "string" ? answer.path : null;
        const frames = Number(answer.frames ?? 0);
        const channels = Number(answer.channels ?? 1);
        const rate = Number(answer.rate ?? 0);
        if (path === null) {
            return answer.untitled === true ? new SoundFile(null, frames, channels, rate) : null;
        }
        return new SoundFile(path, frames, channels, rate,
            (answer.codec as string | null) ?? null, (answer.sample_format as string | null) ?? null);
    }

    /** The open {@link SoundFile}s, in list order. */
    async files(): Promise<(SoundFile | null)[]> {
        const rows = (await this.member("files")).files as { file: Record<string, any> }[];
        return rows.map(({ file: f }) =>
            f.path !== null || f.untitled
                ? new SoundFile(f.path ?? null, Number(f.frames), Number(f.channels), Number(f.rate),
                    f.codec ?? null, f.sample_format ?? null)
                : null);
    }

    /**
     * What the take is made of now: one object per span of a file, in reading
     * order: its `path`, the `start` and the `frames` of it read, and its
     * fades and shape.
     */
    async parts(): Promise<Record<string, unknown>[]> {
        return [...(((await this.member("segments")).parts as Record<string, unknown>[] | undefined) ?? [])];
    }

    /**
     * **Whether the file in focus has edits its file does not hold.** A save
     * that has not completed leaves it dirty; an undo back to what was written
     * leaves it clean.
     */
    async dirty(): Promise<boolean> {
        return (await this.member("binding")).dirty === true;
    }

    /**
     * **Write the take as the edits have left it**: over the file it was
     * opened from, or, given a `path`, there, which a later `save` then writes
     * over. Ctrl+S in the window is the same save. `sampleFormat` is
     * `"float"`, `"int24"` or `"int16"`. Resolves to the path written, once
     * the file is whole; rejects with a `RangeError` for another format.
     */
    async save({ path, sampleFormat = "float" }: { path?: string; sampleFormat?: string } = {}): Promise<string> {
        const request: Record<string, unknown> = { format: String(sampleFormat) };
        if (path !== undefined) request.path = String(path);
        const answer = await this.act("save", { request });
        return String(answer.destination.path);
    }

    /** Add and focus an empty file of that shape. */
    async newFile(channels: number, frames: number, rate: number): Promise<void> {
        await this.act("newFile", {
            shape: { channels: Math.trunc(channels), frames: Math.trunc(frames), rate: Number(rate) },
        });
    }

    /** Open a path or a list of paths in this editor, focusing the last. */
    async openFile(paths: string | readonly string[]): Promise<void> {
        await this.act("openFile", { paths: typeof paths === "string" ? paths : [...paths] });
    }

    /** Focus the file at zero-based `index`, with its cursor and selection. */
    async focusFile(index: number): Promise<void> {
        await this.act("verb", { call: { verb: "focus_file", index: Math.trunc(index) } });
    }

    /** Close the file in focus, or the file at zero-based `index`. */
    async closeFile(index?: number): Promise<void> {
        await this.act("verb", {
            call: { verb: "close_file", ...(index === undefined ? {} : { index: Math.trunc(index) }) },
        });
    }

    /** Show the file list to the left of the take. */
    async showFiles(): Promise<void> {
        await this.act("verb", { call: { verb: "show_files" } });
    }

    /** Hide the file list. */
    async hideFiles(): Promise<void> {
        await this.act("verb", { call: { verb: "hide_files" } });
    }

    // ---- the spectrogram's settings ----

    /**
     * **How the spectrogram analyzes and draws the take**: `window` (frames:
     * 256, 512, 1024, 2048 or 4096), `shape` (the window's: `"hann"`,
     * `"hamming"`, `"blackman"`, `"sine"`, `"welch"` or `"rectangular"`),
     * `overlap` (0, windows end to end, then 0.5, 0.75, 0.875 or 0.9375, each
     * halving the hop), `scale` (`"linear"`, `"log"`, `"mel"`, `"bark"` or
     * `"note"`), `tuning` (the A4 of a note axis, in hertz), `colormap`
     * (`"viridis"`, `"magma"` or `"gray"`), `range` (the decibels the colours
     * span, 20 to 120), `gain` (how far their top is below full scale, 0 to
     * 60 dB), `interpolate` (drawn smooth rather than in blocks of measured
     * magnitudes) and `normalize` (each frequency band at its own scale). The
     * window's, whichever file is in focus; a view state, outside the history.
     */
    async spectrogram(): Promise<SpectrogramSettings> {
        return { ...((await this.member("spectrogram")).spectrogram as SpectrogramSettings) };
    }

    /**
     * Change the settings named ({@link spectrogram}), the others kept. Rejects
     * with a `RangeError` naming a value none of them takes, and then nothing
     * changes.
     */
    async setSpectrogram(settings: Partial<SpectrogramSettings>): Promise<void> {
        const result = await this.view("set_spectrogram", { ...settings });
        const reason = result !== null && typeof result === "object" ? result.reason : null;
        if (reason) throw new RangeError(String(reason));
    }

    /** Show the spectrogram's settings panel to the right of the take. */
    async showSpectrogramSettings(): Promise<void> {
        await this.act("verb", { call: { verb: "show_spectrogram_settings" } });
    }

    /** Hide the spectrogram's settings panel. */
    async hideSpectrogramSettings(): Promise<void> {
        await this.act("verb", { call: { verb: "hide_spectrogram_settings" } });
    }

    /**
     * Retry failed repairs of this editor's files without repeating edits.
     * Call after resolving the I/O failure; rejects with a `CommandError` if
     * repair fails again. Does nothing if none remain.
     */
    async retryPending(): Promise<void> {
        await this.act("retry");
    }

    // ---- what the picture shows ----

    /**
     * Visual layers, back to front. Two different domains cannot share the
     * vertical axis; use `y: "box"` for the overlaid amplitude layer.
     */
    async layers(): Promise<unknown[]> {
        return [...(((await this.member("binding")).layers as unknown[] | undefined) ?? [])];
    }

    /** Set the visual layers ({@link AudioEditor.layers}). */
    async setLayers(stack: string | Iterable<unknown>): Promise<void> {
        await this.view("layers", { stack: typeof stack === "string" ? stack : [...stack] });
    }

    private async view(verb: string, args: Record<string, unknown> = {}): Promise<any> {
        return this.act("verb", { call: { verb, version: await this.version(), ...args } });
    }

    /** The active edit body, separate from the take's visual layers. */
    async layer(): Promise<string> {
        return String((await this.member("layer")).layer);
    }

    /** Put the edit body `name` in hand. */
    async setLayer(name: string): Promise<void> {
        await this.view("layer", { name });
    }

    /** Show a visual layer, appending it if absent. */
    async showLayer(name: string): Promise<void> { await this.view("show_layer", { name }); }

    /** Hide a visual layer, keeping its place and settings. */
    async hideLayer(name: string): Promise<void> { await this.view("hide_layer", { name }); }

    /** Toggle a layer's solo; only visible soloed layers are drawn. */
    async soloLayer(name: string): Promise<void> { await this.view("solo_layer", { name }); }

    /** Set a layer's opacity between zero and one. */
    async layerAlpha(name: string, alpha: number): Promise<void> {
        await this.view("layer_alpha", { name, alpha });
    }

    /** Show the peak and RMS waveform in the take's lane. */
    async viewWave(): Promise<void> { await this.view("view_wave"); }

    /** Show the spectrogram in the take's lane. */
    async viewSpectrogram(): Promise<void> { await this.view("view_spectrogram"); }

    /**
     * Show both, stacked: the waveform in the take's lane and the spectrogram
     * in a lane under it, on one time axis.
     */
    async viewBoth(): Promise<void> { await this.view("view_both"); }

    /** The channels drawn folded, in order. A view state, outside the history. */
    async folded(): Promise<number[]> {
        return [...((await this.member("folded")).folded as number[])];
    }

    /** Fold channel `channel` (from 0) to a strip, or unfold it. */
    async foldChannel(channel: number): Promise<void> {
        await this.channels("fold_channel", { channel: Math.trunc(channel) });
    }

    /** Show channel `channel` alone: every other channel folded. */
    async soloChannel(channel: number): Promise<void> {
        await this.channels("solo_channel", { channel: Math.trunc(channel) });
    }

    /** Unfold every channel. */
    async showAllChannels(): Promise<void> {
        await this.channels("show_all_channels");
    }

    private async channels(verb: string, args: Record<string, unknown> = {}): Promise<void> {
        const result = await this.view(verb, args);
        const reason = result !== null && typeof result === "object" ? result.reason : null;
        if (reason) throw new RangeError(String(reason));
    }

    // ---- the gain envelope ----

    /**
     * Gain envelope as time/value/shape/curve quads, with time in seconds. One
     * is unity. Editing the curve is non-destructive; `applyEnvelope` renders
     * it into a new take.
     */
    async envelope(): Promise<number[]> {
        return [...((await this.member("envelope")).points as number[])];
    }

    /** Set gain quads as one undoable edit. Times must increase. */
    setEnvelope(points: Iterable<number>): Promise<boolean> {
        return this.envelopeAct("setautomation", { points: [...points].map(Number) });
    }

    /** Show and select the gain curve over the take. */
    async showEnvelope(): Promise<void> {
        await this.view("show_envelope");
    }

    /** Render gain into a new take and reset the curve to unity, in one undo step. */
    applyEnvelope(): Promise<boolean> {
        return this.envelopeAct("apply_envelope");
    }

    private async envelopeAct(verb: string, args: Record<string, unknown> = {}): Promise<boolean> {
        const result = ((await this.act("act", { call: { verb, ...args } })) ?? {}) as Record<string, any>;
        if (result.reason) throw new RangeError(String(result.reason));
        return result.changed === true;
    }
}
