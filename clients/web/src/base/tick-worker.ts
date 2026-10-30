// The clock's wake-up, off the page's thread.
//
// This is the whole worker: it holds no state a routine could touch, only
// timers. It exists because the page's own `setTimeout` is clamped (>=4 ms once
// nested) and, in a background tab, throttled to about a second -- longer than
// any usable scheduling headroom, so a sequence would stutter the moment the
// user changed tabs. A worker's timers are not throttled that way.
//
// The queue and the routines stay on the page: a routine is a closure over the
// script's own objects and cannot cross to a worker. Only "wake me in N
// milliseconds" crosses, which is exactly what the Python client's background
// clock thread contributes.

/**
 * Main thread -> worker: arm a timer, or cancel one already armed.
 *
 * A timer is armed for an **instant**, `at`, in milliseconds since the epoch
 * on both threads' clocks (`performance.timeOrigin + performance.now()`),
 * never for a delay: a worker reads its first message only once its script
 * has loaded, which on a cold start can take longer than the wait itself, and
 * a delay counted from that read would make the clock's first wake late by
 * however long it was.
 */
export type TickRequest =
    | { id: number; wake: number; at: number }
    | { id: number; cancel: true };

/**
 * Worker -> main thread: the timer with this id came due, for the wake it was
 * armed for -- or, once, that the worker is running and its timers are the
 * ones to wait on.
 */
export type TickReply = { id: number; wake: number } | { ready: true };

const timers = new Map<number, ReturnType<typeof setTimeout>>();
const post = (reply: TickReply) => (self as unknown as Worker).postMessage(reply);

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
            post({ id: msg.id, wake: msg.wake });
        }, Math.max(msg.at - (performance.timeOrigin + performance.now()), 0)),
    );
};

post({ ready: true });
