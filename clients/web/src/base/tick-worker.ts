// The clock's wake-up, off the page's thread.
//
// This is the whole worker: it holds no state a routine could touch, only
// timers. It exists because the page's own `setTimeout` is clamped (≥4 ms once
// nested) and, in a background tab, throttled to about a second — longer than
// any usable scheduling headroom, so a sequence would stutter the moment the
// user changed tabs. A worker's timers are not throttled that way.
//
// The queue and the routines stay on the page: a routine is a closure over the
// script's own objects and cannot cross to a worker. Only "wake me in N
// milliseconds" crosses, which is exactly what the Python client's background
// clock thread contributes.

/// Main thread → worker: arm a timer, or cancel one already armed.
export type TickRequest =
    | { id: number; delayMs: number }
    | { id: number; cancel: true };

/// Worker → main thread: the timer with this id came due.
export interface TickReply {
    id: number;
}

const timers = new Map<number, ReturnType<typeof setTimeout>>();

self.onmessage = (event: MessageEvent<TickRequest>) => {
    const msg = event.data;
    const pending = timers.get(msg.id);
    if (pending !== undefined) {
        clearTimeout(pending);
        timers.delete(msg.id);
    }
    if ("cancel" in msg) return;
    timers.set(
        msg.id,
        setTimeout(() => {
            timers.delete(msg.id);
            (self as unknown as Worker).postMessage({ id: msg.id } satisfies TickReply);
        }, Math.max(msg.delayMs, 0)),
    );
};
