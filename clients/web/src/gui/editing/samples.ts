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
export class SamplesView extends View<Buffer> {
    build(editor: Editor<Buffer>): GuiNode {
        const take = editor.structure;
        const wid = this.register(editor.newId(), take);
        return guiWindow(
            { title: editor.title, w: editor.size[0], h: editor.size[1], layout: "col" },
            waveform({
                id: wid,
                buffer: Math.trunc(take.bufnum),
                channels: Math.max(1, Math.trunc(take.channels || 1)),
                ruler: "time",
                sampleRate: editor.sampleRate,
                tempo: editor.tempo,
                label: nameOf(take),
            }),
        );
    }

    override props(): Record<string, PropValue> {
        // A take's picture is the server's buffer, which the host re-reads on a
        // generation bump rather than being told: what this view can correct is
        // nothing, and saying so is what keeps a stale edit's answer honest.
        return {};
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
    constructor(take: Buffer, options: GenericEditorOptions<Buffer>) {
        super(take, {
            title: "Samples",
            ...options,
            sampleRate: Number(options.sampleRate || take.sampleRate || 48_000),
            domain: new SamplesDomain(),
            view: new SamplesView(),
        });
    }
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
