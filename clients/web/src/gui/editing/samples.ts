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
 * **A stroke lands on one channel.** The samples are interleaved, so writing one
 * channel of a stereo take is a strided write and `/buffer_setRange` is
 * contiguous: the span is read, the channel's frames are spliced into it and the
 * run goes back whole. Mono needs neither, which is the ordinary take.
 *
 * @module
 */

import { SAMPLES } from "../../document.ts";
import type { Buffer } from "../../defs/buffer.ts";
import { waveform, window as guiWindow } from "../guidef.ts";
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
 * appear, and an empty one is a picture that measures nothing.
 */
export function measures(stack: Iterable<string>): Measure[] {
    const out = [...stack].map(String);
    for (const name of out) {
        if (!(MEASURES as readonly string[]).includes(name)) {
            throw new RangeError(`unknown measure ${name} (one of ${MEASURES.join(", ")})`);
        }
    }
    if (out.length === 0) {
        throw new RangeError(`a signal view measures something (one of ${MEASURES.join(", ")})`);
    }
    return out as Measure[];
}

/** One write in the crate's vocabulary. */
interface Write {
    intent: "write";
    channel: number;
    start: number;
    values: number[];
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
 */
export class SamplesDomain extends Domain<Buffer> {
    override readonly name = SAMPLES;

    /**
     * The inverse of the gesture being routed, taken off the wire.
     *
     * It is held for the length of one gesture rather than derived, because the
     * crate's vocabulary has no field for "what this replaced" — an edit states
     * the resulting value, and the payload stating the previous one *is* the
     * inverse. The host sends both in the same event, so this is where the
     * second one waits between `payload` and `current`, which an editor calls
     * back to back.
     */
    #previous: Write | null = null;

    /**
     * The writes in flight, chained.
     *
     * A page's buffer calls are asynchronous where the Python client's are not,
     * and {@link Domain.project} answers *whether the edit lands*, not when. So
     * the writes are queued rather than awaited, and queued **in order**: two
     * strokes over one span that raced would leave the buffer holding the first.
     */
    #writes: Promise<void> = Promise.resolve();

    payload(_structure: Buffer, tag: string, values: readonly unknown[]): unknown {
        let channel: number;
        let start: number;
        let wrote: number[];
        let previous: number[];
        if (tag === "draw" && values.length >= 4) {
            channel = Math.trunc(Number(values[0]));
            start = Math.trunc(Number(values[1]));
            wrote = floats(values[2]);
            previous = floats(values[3]);
        } else if (tag === "sample" && values.length >= 4) {
            channel = Math.trunc(Number(values[0]));
            start = Math.trunc(Number(values[1]));
            wrote = [Number(values[2])];
            previous = [Number(values[3])];
        } else {
            return null;
        }
        if (wrote.length === 0) return null;
        this.#previous = { intent: "write", channel, start, values: previous };
        return { intent: "write", channel, start, values: wrote } satisfies Write;
    }

    /**
     * What the stroke replaced, as the write that puts it back.
     *
     * `null` when the run is not the same length as what it replaced — an
     * inverse that does not cover the span it undoes would leave part of the
     * edit standing, and an entry the pile cannot invert is better recorded as
     * one than pretended.
     */
    current(_structure: Buffer, payload: unknown): unknown {
        const previous = this.#previous;
        this.#previous = null;
        if (previous === null) return null;
        const values = (payload as Write).values ?? [];
        return previous.values.length === values.length ? previous : null;
    }

    project(structure: Buffer, payload: unknown): boolean {
        const write = payload as Write;
        const channels = Math.max(1, Math.trunc(structure.channels || 1));
        const channel = Math.min(Math.trunc(write.channel ?? 0), channels - 1);
        const start = Math.trunc(write.start ?? 0);
        const values = write.values ?? [];
        if (values.length === 0) return false;
        if (channels === 1) {
            this.#queue(() => structure.setSamples(values, { start }));
            return true;
        }
        // Interleaved: read the frames the stroke covers, splice this channel's
        // into them and write the run back whole. One extra round trip on a
        // multi-channel take, and none on a mono one.
        const first = start * channels;
        const width = values.length * channels;
        this.#queue(async () => {
            const span = [...(await structure.getSamples({ start: first, count: width }))];
            while (span.length < width) span.push(0);
            values.forEach((value, i) => {
                span[i * channels + channel] = value;
            });
            await structure.setSamples(span, { start: first });
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

    override label(): string {
        return "draw the samples";
    }
}

/**
 * One `waveform`: the take on its own axis, drawn by the host straight from the
 * server buffer.
 */
/**
 * **What a hand may do to the samples**, and it is three gestures rather than a
 * mode: a plain drag sweeps a selection (what an editor does by default), Alt
 * draws over the samples and Ctrl grabs one. A navigable `signal` declares only
 * the first of those, so an editor that says nothing opens a window that can
 * only select — which is what this editor was doing while its own docstring
 * promised a stroke. It is the plan the standalone host builds the same view
 * with (`clients/gui/src/host/document/tree.rs`), and one view is one plan.
 */
const GESTURES = { drag: "select", alt: "draw", ctrl: "sample" };

export class SamplesView extends View<Buffer> {
    /** What the picture measures, innermost last. */
    layers: Measure[];

    constructor(layers: Iterable<string> = MEASURES) {
        super();
        this.layers = measures(layers);
    }

    build(editor: Editor<Buffer>): GuiNode {
        const take = editor.structure;
        const wid = this.register(editor.newId(), take);
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            waveform({
                id: wid,
                buffer: Math.trunc(take.bufnum),
                channels: Math.max(1, Math.trunc(take.channels || 1)),
                measure: this.layers.join(" ") as "peak" | "rms" | "peak rms",
                ruler: "time",
                sampleRate: editor.sampleRate,
                tempo: editor.tempo,
                label: nameOf(take),
                gestures: GESTURES,
            }),
            ...editor.extra,
        );
    }

    override props(): Record<string, PropValue> {
        // **The take's picture is the server's buffer, so what corrects it is
        // "read it again".** A stroke needs nothing from here: the host wrote
        // those cells itself and its picture moved with them. An undo is the
        // case that needs it — the write goes to the *server's* buffer from
        // this side, and nothing in the host saw it, so the window kept drawing
        // the stroke until some other reason (a zoom, a scroll) made it resolve
        // the source again. That is what "the samples undo is slow to show up"
        // was.
        //
        // `reload` is the verb for exactly this: the element forgets what it
        // resolved and the loader reads its file, cache or server buffer on the
        // next pass. The generation pairs `/gui_ack` carries would say the same
        // thing more cheaply, but no client sends one and the host acts on
        // none — so this is the door that is actually open.
        return { reload: 1 };
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
    constructor(take: Buffer, options: SamplesEditorOptions) {
        super(take, {
            title: "Samples",
            ...options,
            sampleRate: Number(options.sampleRate || take.sampleRate || 48_000),
            domain: new SamplesDomain(),
            view: new SamplesView(options.layers ?? MEASURES),
        });
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
        const view = this.view as SamplesView;
        view.layers = measures(stack);
        if (this.host !== null && this.window !== null) {
            for (const wid of view.widgets.keys()) {
                void this.host.set(wid, { measure: view.layers.join(" ") });
            }
        }
    }
}

/** {@link SamplesEditor}'s options: the generic ones plus the measure stack. */
export interface SamplesEditorOptions extends GenericEditorOptions<Buffer> {
    /** What the picture measures. Defaults to {@link MEASURES}. */
    layers?: readonly string[];
}

function nameOf(take: Buffer): string {
    const name = (take as { name?: string }).name;
    if (typeof name === "string" && name) return name;
    return `buffer ${Math.trunc(take.bufnum)}`;
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
