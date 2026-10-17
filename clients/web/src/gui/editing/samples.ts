/**
 * The picture a take is drawn in: one `waveform`, and the measures it stacks.
 *
 * **What the picture measures is the view's.** A waveform is drawn as a stack of
 * measures over one field -- what the signal reached (`peak`) with what it held
 * inside that (`rms`) -- and that is a prop of the one widget rather than a pile
 * of widgets: every view of a signal paints its own field before it draws, so
 * two of them on one rectangle are not layers, the second hides the first.
 * Measuring twice into one body is also what makes the rest of it one thing: one
 * axis, one ruler, one selection, one playhead, one upload of the samples.
 *
 * The editor that draws a take in it is {@link AudioEditor}.
 *
 * @module
 */

import type { Buffer } from "../../defs/buffer.ts";
import { samplesMeasures } from "../../core/clausters_core_web.js";
import type { SignalLayer, SignalLayers } from "../guidef.ts";
import type { GuiNode } from "../guidef.ts";
import type { PropValue } from "../host.ts";
import type { Editor } from "./editor.ts";
import { View } from "./view.ts";

/** What a take's window is composed through: the editor's door to its core. */
interface TakeWindow {
    syncCore(): void;
    coreCall(verb: string, args?: Record<string, unknown>): Record<string, unknown>;
}

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

/**
 * A pane holding the take's signal body, drawn by the host straight from
 * the server buffer. Visual measures remain a prop of that body; the pane
 * owns the time axis, ruler, selection and playhead.
 *
 * **The window is the application's**, composed in the shared crate
 * (`EditingCore`): the waveform, the gesture plan a take is edited with (a
 * drag selects, Alt draws, Ctrl grabs one sample), the label and the correction
 * a write answers with. What is left here is the id a hand's gestures come back
 * on.
 */
export class SamplesView extends View<Buffer> {
    /** What the picture measures, innermost last. */
    layers: Measure[];

    constructor(layers: SignalLayers | Iterable<Measure> = MEASURES) {
        super();
        this.layers = measures(layers);
    }

    build(editor: Editor<Buffer>): GuiNode {
        const wid = this.widget(editor, "waveform", editor.structure);
        const meter = this.widget(editor, "meter", editor.structure);
        const pane = this.widget(editor, "pane", editor.structure);
        const ed = editor as unknown as TakeWindow;
        ed.syncCore();
        const envelope = this.widget(editor, "envelope", editor.structure);
        // the crate names the widgets of its dialogs and this numbers them
        const named = ed.coreCall("dialogs").dialogs;
        const dialogs: Record<string, number> = {};
        for (const name of Array.isArray(named) ? named.map(String) : []) {
            dialogs[name] = this.widget(editor, "dialog", editor.structure, name);
        }
        const tree = ed.coreCall("window", {
            widget: wid, meter, pane, envelope, dialogs,
        }) as unknown as GuiNode;
        // **A page's own widgets are its objects**, so they are appended here
        // rather than composed in the crate.
        tree.children = [...(tree.children ?? []), ...editor.extra];
        return tree;
    }

    override props(editor: Editor<Buffer>, widgetId: number): Record<string, PropValue> {
        return (editor as unknown as TakeWindow).coreCall("props", { widget: widgetId }) as Record<
            string,
            PropValue
        >;
    }
}
