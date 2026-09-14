/**
 * Editing a **buffer's samples**: the one domain whose state is not here.
 *
 * A curve's points and a timeline's events are values this page holds, so the
 * crate can be handed one and asked what an edit makes of it. A span of samples
 * is not: the frames are in a **server buffer**, which is why the crate's own
 * `Samples` is a borrowed view over memory its caller owns, and why
 * {@link domainEdit} answers nothing for this vocabulary. What is shared is the
 * payload's shape and its coalesce key; where the state lives is the client's,
 * and this module is that half.
 *
 * **The inverse rides on the wire.** A `"draw"` carries the run it wrote *and*
 * the run it replaced, and a `"sample"` carries the value and the previous one —
 * the protocol was written that way precisely so an owner can invert a stroke
 * without having remembered anything. So nothing is read back from the server to
 * undo: the edit and its inverse arrive together, and what the history records is
 * the second.
 *
 * **What the picture measures is the view's.** A waveform is drawn as a stack of
 * measures over one field — what the signal reached (`peak`) with what it held
 * inside that (`rms`) — and that is a prop of the one widget rather than a pile
 * of widgets: every view of a signal paints its own field before it draws, so
 * two of them on one rectangle are not layers, the second hides the first.
 * Measuring twice into one body is also what makes the rest of it one thing: one
 * axis, one ruler, one selection, one playhead, one upload of the samples.
 *
 * **A stroke lands on one channel.** What a write does to the buffer is the
 * shared crate's (`SamplesEditorCore`, `write`): a mono take takes the run as
 * flat samples (`/buffer_setRange`), and one channel of a take with more is
 * written by frames of that channel alone (`/buffer_setRangeChannel`), so the
 * other channels are never touched. This page walks those steps against the
 * take's server ({@link runSteps}).
 *
 * @module
 */

import { SAMPLES } from "../../document.ts";
import type { RecordedLeg, Selection } from "../../document.ts";
import type { Answer } from "./echo.ts";
import type { Buffer } from "../../defs/buffer.ts";
import { SamplesEditorCore, StepRunner, samplesMeasures } from "../../core/clausters_core_web.js";
import { resolveServer } from "../../defs/wire.ts";
import { runSteps } from "../../steps.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import { Domain } from "./domain.ts";
import { Editor } from "./editor.ts";
import type { GenericEditorOptions } from "./editor.ts";
import { View } from "./view.ts";

/**
 * The measures a signal view can stack, in the order a reader thinks of them:
 * what the signal reached, and what it held inside that.
 */
export const MEASURES = ["peak", "rms"] as const;

/** One of the measures above. */
export type Measure = (typeof MEASURES)[number];

/**
 * A measure stack as an array, or a `RangeError` naming what is wrong.
 *
 * A stack is written by hand, so a silent typo is a layer that quietly does not
 * appear, and an empty one is a picture that measures nothing. The check is the
 * crate's (`samplesMeasures`).
 */
export function measures(stack: Iterable<string>): Measure[] {
    const answer = JSON.parse(samplesMeasures(JSON.stringify({ stack: [...stack].map(String) }))) as {
        layers?: Measure[];
        error?: string;
    };
    if (answer.error !== undefined || answer.layers === undefined) {
        throw new RangeError(answer.error ?? "not a measure stack");
    }
    return answer.layers;
}

/** One write in the crate's vocabulary. */
interface Write {
    intent: "write";
    channel: number;
    start: number;
    values: number[];
}

/**
 * An event's arguments as JSON carries them: a blob is the run a stroke wrote or
 * replaced, read into its numbers — the wire's framing is this page's.
 */
function plain(value: unknown): unknown {
    if (value instanceof ArrayBuffer) return floats(new Uint8Array(value));
    if (ArrayBuffer.isView(value)) return floats(value);
    if (Array.isArray(value)) return value.map(plain);
    return value;
}

/** What one turn of the core came to. */
interface Outcome {
    turn?: string;
    answer?: Answer;
    seq?: number;
    redo?: boolean;
    record?: { label: string; legs: { forward: unknown; backward: unknown; key: string }[] };
    changed?: boolean;
    version?: number;
    edit?: unknown;
    locate?: number;
    selection?: unknown;
}

/**
 * A run of samples as numbers, from the little-endian `f32` blob the wire
 * carries (or from an array, which is what a hand-written test sends).
 */
function floats(blob: unknown): number[] {
    if (blob instanceof Uint8Array) {
        const view = new DataView(blob.buffer, blob.byteOffset, blob.byteLength);
        const out: number[] = [];
        for (let i = 0; i + 4 <= blob.byteLength; i += 4) out.push(view.getFloat32(i, true));
        return out;
    }
    if (blob instanceof Float32Array) return [...blob];
    if (Array.isArray(blob)) return blob.map(Number);
    return [];
}

/**
 * A span of samples' vocabulary: the crate's `samples`, over frames the server
 * holds.
 *
 * **What a gesture means is the application's** (`SamplesEditorCore`): the run
 * a stroke wrote and the run it replaced are read there in one reading, so
 * nothing waits here between two calls. What is left is the write itself, onto
 * the buffer this page holds — a stroke's, and a step of the history's.
 */
export class SamplesDomain extends Domain<Buffer> {
    override readonly name = SAMPLES;
    override readonly ingested = true;

    /** The editor whose take this is: what turns a write into steps. */
    editor: SamplesEditor | null = null;

    /**
     * The writes in flight, chained.
     *
     * A page's buffer calls are asynchronous where the Python client's are not,
     * and {@link Domain.project} answers *whether the edit lands*, not when. So
     * the writes are queued rather than awaited, and queued **in order**: two
     * strokes over one span that raced would leave the buffer holding the first.
     */
    #writes: Promise<void> = Promise.resolve();

    /**
     * Nothing: the inverse of a stroke is read by the core from the same
     * message that carried the stroke, so there is nothing to read off the take.
     */
    current(_structure: Buffer, _payload: unknown): unknown {
        return null;
    }

    /**
     * Write `payload` onto the take's buffer: the steps the core answers for it,
     * walked against the take's server. A stroke's write and a step of the
     * history's are this one call.
     */
    project(structure: Buffer, payload: unknown): boolean {
        const editor = this.editor;
        const write = payload as Write;
        if (editor === null || (write.values ?? []).length === 0) return false;
        this.#queue(async () => {
            const server = resolveServer(structure.server);
            const steps = editor.writeSteps(write, await server.bulkChunk());
            await runSteps(server, editor.runner, steps);
        });
        return true;
    }

    /** One write after the last, and a failure reported rather than swallowed. */
    #queue(run: () => Promise<void>): void {
        this.#writes = this.#writes.then(run).catch((error: unknown) => {
            // The chain continues so the next stroke is not stuck behind a
            // failed one; the failure itself is thrown where the page sees it.
            queueMicrotask(() => {
                throw error;
            });
        });
    }
}

/**
 * One `waveform`: the take on its own axis, drawn by the host straight from the
 * server buffer.
 *
 * **The window is the application's**, composed in the shared crate
 * (`SamplesEditorCore`): the waveform, the gesture plan a take is edited with (a
 * drag selects, Alt draws, Ctrl grabs one sample), the label and the correction
 * a write answers with. What is left here is the id a hand's gestures come back
 * on.
 */
export class SamplesView extends View<Buffer> {
    /** What the picture measures, innermost last. */
    layers: Measure[];

    constructor(layers: Iterable<string> = MEASURES) {
        super();
        this.layers = measures(layers);
    }

    build(editor: Editor<Buffer>): GuiNode {
        const wid = this.widget(editor, "waveform", editor.structure);
        const ed = editor as SamplesEditor;
        ed.syncCore();
        const tree = ed.coreCall("window", { widget: wid }) as unknown as GuiNode;
        // **A page's own widgets are its objects**, so they are appended here
        // rather than composed in the crate.
        tree.children = [...(tree.children ?? []), ...editor.extra];
        return tree;
    }

    override props(editor: Editor<Buffer>, widgetId: number): Record<string, PropValue> {
        return (editor as SamplesEditor).coreCall("props", { widget: widgetId }) as Record<
            string,
            PropValue
        >;
    }
}

/**
 * A buffer's samples on screen, editable back into the server's buffer.
 *
 * The picture and the sound are the **same** buffer: the host draws what the
 * server holds, and a stroke writes there — so what is heard after an edit is
 * what is seen, with no copy in between.
 */
export class SamplesEditor extends Editor<Buffer> {
    /** The window, in the shared crate: the take, the measures and the chrome. */
    private readonly core: SamplesEditorCore;

    /**
     * The runner a write's steps are walked through.
     *
     * @internal
     */
    readonly runner = new StepRunner();

    /**
     * What a write does to the take's buffer, as the steps the core answers.
     *
     * @internal
     */
    writeSteps(edit: unknown, chunk: number): unknown[] {
        return (this.coreCall("write", { edit, chunk }).steps as unknown[] | undefined) ?? [];
    }

    constructor(take: Buffer, options: SamplesEditorOptions) {
        const view = new SamplesView(options.layers ?? MEASURES);
        const domain = new SamplesDomain();
        super(take, {
            title: "Samples",
            ...options,
            sampleRate: Number(options.sampleRate || take.sampleRate || 48_000),
            domain,
            view,
        });
        domain.editor = this;
        this.core = new SamplesEditorCore(
            JSON.stringify({ ...this.facts(), layers: view.layers, version: this.version }),
        );
    }

    /** What this page holds about the take and the window. */
    private facts(): Record<string, unknown> {
        const take = this.structure;
        const name = (take as { name?: string }).name;
        return {
            buffer: Math.trunc(take.bufnum),
            channels: Math.max(1, Math.trunc(take.channels || 1)),
            name: typeof name === "string" && name ? name : null,
            rate: this.sampleRate,
            tempo: this.tempo,
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
        return JSON.parse(this.core.call(JSON.stringify({ verb, ...args }))) as Record<
            string,
            unknown
        >;
    }

    /**
     * Hand the core what this page holds: the take a page may have resized, the
     * axis and the window's chrome.
     *
     * @internal
     */
    syncCore(): void {
        this.coreCall("sync", this.facts());
    }

    // ---- the crate's turns ----

    protected override deliver(addr: string, rawArgs: readonly unknown[]): boolean {
        this.syncCore();
        const outcome = this.coreCall("event", {
            addr,
            args: plain([...rawArgs]),
            version: this.version,
        }) as Outcome;
        if (outcome.turn === "closed") return this.closedWindow();
        if (outcome.turn === "step") {
            // **What it answers is whether anything moved**, and a step nobody
            // could apply says why.
            const stepped = outcome.redo === true ? this.redo() : this.undo();
            const reason = !stepped && this.app.unreachable !== null
                ? `${this.app.unreachable}: nothing here can put that edit back`
                : null;
            this.echo.send(this.coreCall("acknowledge", {
                seq: outcome.seq ?? 0,
                version: this.version,
                reason,
            }) as unknown as Answer);
            return stepped;
        }
        return this.take(outcome);
    }

    /**
     * One `/gui_event` payload, with the stamp already taken off: the same turn
     * as a message, unstamped.
     */
    protected override route(args: readonly unknown[]): boolean {
        this.syncCore();
        const [wid, tag, ...values] = args;
        return this.take(this.coreCall("event", {
            addr: "/gui_event",
            args: plain([wid, 0, 0, tag, ...values]),
            version: this.version,
        }) as Outcome);
    }

    /**
     * Carry out what a turn came to, and answer the host. Answers whether the
     * take changed.
     */
    private take(outcome: Outcome): boolean {
        if (outcome.turn === undefined || outcome.turn === "nothing") return false;
        const changed = outcome.changed === true;
        if (changed) {
            // **The write is this page's to carry out**: the samples are in the
            // server's buffer, and the crate answered what to write there.
            (this.domain as SamplesDomain).project(this.structure, outcome.edit);
            const record = outcome.record;
            if (record !== undefined) {
                this.editing.history.record(
                    record.legs.map((leg) => ({ structure: this.registered(), ...leg }) as RecordedLeg),
                    { label: record.label },
                );
            }
            this.version = outcome.version ?? this.version;
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
        this.echo.send(outcome.answer);
        return changed;
    }

    /**
     * What the picture measures — `["peak", "rms"]` for the editor's view,
     * `["peak"]` for the bare envelope.
     *
     * **Assigning it on an open view sends one message.** The measure is a live
     * `/gui_set` prop, so the body appears and disappears over the peaks with
     * the picture, the axis, the zoom, the selection and the playhead all
     * exactly where they were. Redrawing for this would be the wrong tool twice
     * over: a redefine rebuilds every widget (so a handler bound to one by name
     * is left holding an id nobody answers to) and the window it redefines is
     * reopened.
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

/** {@link SamplesEditor}'s options: the generic ones plus the measure stack. */
export interface SamplesEditorOptions extends GenericEditorOptions<Buffer> {
    /** What the picture measures. Defaults to {@link MEASURES}. */
    layers?: readonly string[];
}

/**
 * Whether `edit` should open this as a take: anything with a buffer number and
 * the two calls that read and write its frames, which is what a `Buffer`
 * answers with.
 */
export function isSamples(structure: unknown): structure is Buffer {
    const candidate = structure as { bufnum?: unknown; setSamples?: unknown };
    return candidate !== null && typeof candidate === "object" &&
        typeof candidate.bufnum === "number" && typeof candidate.setSamples === "function";
}
