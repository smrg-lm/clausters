/**
 * The shapes a playback's pass is said in: where a pass ends ({@link End})
 * and the space bar's pass ({@link Pass}), over a roll or a score, and the
 * steps a playback answers.
 *
 * @module
 */

export type { RenderStep, Step, StepArg } from "../../steps.ts";

/**
 * **Where a pass ends**, on a multitrack's transport or a notes editor's:
 * `null`, the transport rolling on past the contents; `"contents"`, where
 * they end; or a number, an **end marker** -- seconds of a multitrack, a beat
 * of a sequence.
 */
export type End = null | "contents" | number;

/**
 * **The audio editor's pass**, over a multitrack or a roll: a time range to
 * play from its start to its end (`[start, end]`, seconds of a multitrack or
 * beats of a sequence), and whether the loop switch is on.
 */
export interface Pass {
    range?: readonly [number, number] | null;
    looping?: boolean;
}
