/**
 * What a take's picture measures, and the wire's samples as numbers.
 *
 * **What the picture measures is the view's.** A waveform is drawn as a stack of
 * measures over one field -- what the signal reached (`peak`) with what it held
 * inside that (`rms`) -- and that is a prop of the one widget rather than a pile
 * of widgets: every view of a signal paints its own field before it draws, so
 * two of them on one rectangle are not layers, the second hides the first.
 * Measuring twice into one body is also what makes the rest of it one thing: one
 * axis, one ruler, one selection, one playhead, one upload of the samples.
 *
 * The editor whose picture these measure is {@link AudioEditor}.
 *
 * @module
 */

import { samplesMeasures } from "../../core/clausters_core_web.js";
import type { SignalLayer, SignalLayers } from "../guidef.ts";

/**
 * The editor's default stack: what the signal reached and held inside it.
 * Other visual layers are validated by the shared Rust view vocabulary.
 */
export const MEASURES = ["peak", "rms"] as const;

/** A visual layer name or its settings, in the existing signal wire form. */
export type Measure = string | SignalLayer;

/**
 * A visual layer stack as an array, or a `RangeError` naming what is wrong.
 *
 * A stack is written by hand, so a silent typo is a layer that quietly does not
 * appear, and an empty one is a picture that measures nothing. The check is the
 * crate's (`samplesMeasures`).
 */
export function measures(stack: SignalLayers | Iterable<Measure>): Measure[] {
    const answer = JSON.parse(samplesMeasures(JSON.stringify({
        stack: typeof stack === "string" ? stack : [...stack],
    }))) as {
        layers?: Measure[];
        error?: string;
    };
    if (answer.error !== undefined || answer.layers === undefined) {
        throw new RangeError(answer.error ?? "not a measure stack");
    }
    return answer.layers;
}

/**
 * An event's arguments as JSON carries them: a blob is the run a stroke wrote or
 * replaced, read into its numbers -- the wire's framing is this page's.
 *
 * @internal
 */
export function plain(value: unknown): unknown {
    if (value instanceof ArrayBuffer) return floats(new Uint8Array(value));
    if (ArrayBuffer.isView(value)) return floats(value);
    if (Array.isArray(value)) return value.map(plain);
    return value;
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
