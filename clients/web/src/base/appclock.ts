/**
 * `AppClock`: the clock face over the page's own loop — the application's time.
 *
 * Three clocks are three questions, and mixing them is how a program ends up
 * with two scheduling vocabularies:
 *
 * - {@link TempoClock} keeps **musical** time. It is in beats, it is what a
 *   piece plays on, and a routine on it must never block.
 * - **`AppClock` keeps the application's time.** It is in **seconds**, it runs
 *   on the page's own loop, and it is where anything that touches a window
 *   belongs: an animation, a periodic read-out, a redraw, a follow-up to a
 *   gesture.
 * - The audio engine keeps sample time, which is neither of these and is not a
 *   client clock at all.
 *
 * This is sclang's reading of the same split (`SystemClock` / `TempoClock` /
 * `AppClock`), and the part worth taking from it is not the name: it is that
 * **the loop's timer source and the clock are one object**. An animation is
 * then a routine that waits —
 *
 * ```js
 * appClock().play(new Routine(function* () {
 *     for (;;) {
 *         handle.set({ color: "red" });
 *         yield 0.25;
 *         handle.set({ color: "grey" });
 *         yield 0.25;
 *     }
 * }));
 * ```
 *
 * — rather than an animation API beside the routines the client already has.
 *
 * **Where this differs from the reference client, and why it is not a
 * divergence.** There the clock is built over an `EventLoop`: a thread, a
 * selector and a wake channel, because a Python script's windows are drained by
 * something the script started. A page *is* that loop already, so the timers
 * are the platform's (`setTimeout`) and there is no loop object to hand in. The
 * class, the calls and what they mean are the same; what is missing is the part
 * that only exists to make a thread behave like a page.
 *
 * **`defer` is the other half.** A routine on the {@link TempoClock} must never
 * block, so it has nowhere to put work that touches a window; `defer` hands that
 * work to the loop and returns immediately — here, after the current task rather
 * than inside it, which is what makes it a hand-off and not a call.
 *
 * @module
 */

import type { Stream } from "./stream.ts";
import { resume } from "./stream.ts";

/** What a clock can be handed: a stream, or a plain callable for a one-shot. */
export type AppItem = Stream | (() => unknown);

/**
 * Seconds on the page's loop.
 *
 * It holds no queue of its own: the platform's timers *are* the schedule, so a
 * routine on this clock and a redraw the page does are ordered against each
 * other rather than racing.
 */
export class AppClock {
    /** The reading {@link AppClock.elapsed} counts from. */
    private readonly origin: number;
    /** item → the timers queued for it, so `unsched` can drop all of them. */
    private readonly timers = new Map<AppItem, Set<ReturnType<typeof setTimeout>>>();

    constructor() {
        this.origin = this.now();
    }

    // ---- reading the time ----

    /**
     * Seconds since this clock was made — the reading `sched` measures a delay
     * from, and the one an animation asks for its phase.
     */
    elapsed(): number {
        return this.now() - this.origin;
    }

    /** The loop's own clock reading, which {@link AppClock.schedAbs} takes. */
    now(): number {
        return (typeof performance !== "undefined" ? performance.now() : Date.now()) / 1000;
    }

    // ---- scheduling ----

    /**
     * Runs `item` `delay` **seconds** from now.
     *
     * `item` is a `Routine` (or any `Stream`), or a plain function for a
     * one-shot. A routine is rescheduled by whatever it yields, a function by
     * whatever number it returns, and one returning nothing runs once — the same
     * contract `TempoClock.sched` states, in the other unit.
     */
    sched(delay: number, item: AppItem): AppItem {
        const timer = setTimeout(() => {
            this.forget(item, timer);
            const delta = resume(item, this, { musical: false });
            if (delta !== undefined) this.sched(delta, item);
        }, Math.max(0, delay) * 1000);
        let queued = this.timers.get(item);
        if (queued === undefined) this.timers.set(item, (queued = new Set()));
        queued.add(timer);
        return item;
    }

    /** {@link AppClock.sched} against an absolute reading of {@link AppClock.now}. */
    schedAbs(when: number, item: AppItem): AppItem {
        return this.sched(when - this.now(), item);
    }

    /**
     * Schedules `routine` to start now, and answers it.
     *
     * There is no `quant` here and there should not be: quantization is a
     * musical grid and this clock has none — a routine that must land on a beat
     * belongs on the {@link TempoClock}, and one that must touch a window from
     * there gets here through {@link AppClock.defer}.
     */
    play(routine: AppItem): AppItem {
        this.sched(0, routine);
        return routine;
    }

    /**
     * Runs `func` on the loop as soon as it comes round, and returns at once.
     *
     * The door from anywhere that must not do the work itself — a routine on the
     * {@link TempoClock}, whose thread the whole timeline waits on. It lands
     * **after** the current task rather than inside it, which is the difference
     * between handing work over and calling it.
     */
    defer(func: () => unknown): () => unknown {
        setTimeout(func, 0);
        return func;
    }

    /** Cancels whatever is queued for `item`, leaving the rest in order. */
    unsched(item: AppItem): boolean {
        const queued = this.timers.get(item);
        if (queued === undefined) return false;
        for (const timer of queued) clearTimeout(timer);
        this.timers.delete(item);
        return true;
    }

    private forget(item: AppItem, timer: ReturnType<typeof setTimeout>): void {
        const queued = this.timers.get(item);
        if (queued === undefined) return;
        queued.delete(timer);
        if (queued.size === 0) this.timers.delete(item);
    }
}
