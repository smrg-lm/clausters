// The browser ticker, over a worker that is not there yet.
//
// A worker reads its first message only once its script has loaded, and on a
// cold start that took longer than a clock's first wait: until it says it
// runs, a wake is also armed on the page's own timer. These tests stand a
// fake `Worker` in for the real one, so what is asserted is the ticker's
// bookkeeping -- the fallback runs a wake once, a reply for a wake that was
// replaced or already run runs nothing, and a running worker's reply is the
// only one waited on.

import assert from "node:assert/strict";
import test from "node:test";

import type { TickReply, TickRequest } from "../src/base/tick-worker.ts";

class FakeWorker {
    static last: FakeWorker | null = null;
    onmessage: ((event: { data: TickReply }) => void) | null = null;
    readonly posted: TickRequest[] = [];
    constructor() {
        FakeWorker.last = this;
    }
    postMessage(msg: TickRequest): void {
        this.posted.push(msg);
    }
    reply(data: TickReply): void {
        this.onmessage?.({ data });
    }
    /** The wake the last arm asked for. */
    lastWake(): { id: number; wake: number } {
        const armed = this.posted.filter((m) => "wake" in m).at(-1);
        assert.ok(armed && "wake" in armed);
        return { id: armed.id, wake: armed.wake };
    }
}

const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

test("a wake runs once, on the page's timer until the worker runs, then on the worker's", async () => {
    (globalThis as { Worker?: unknown }).Worker = FakeWorker;
    const { workerTicker } = await import("../src/base/clock.ts");
    const ticker = workerTicker();
    const worker = FakeWorker.last!;
    let runs = 0;

    // Not running yet: the page's timer wakes it, and the worker's late
    // answer to the same wake runs nothing.
    ticker.schedule(0.005, () => runs++);
    const first = worker.lastWake();
    assert.ok("at" in worker.posted[0]!, "an instant, not a delay");
    await sleep(40);
    assert.equal(runs, 1);
    worker.reply(first);
    assert.equal(runs, 1);

    // A wake replaced before it came due: only the second runs.
    worker.reply({ ready: true });
    ticker.schedule(0.005, () => runs++);
    const replaced = worker.lastWake();
    ticker.schedule(0.005, () => (runs += 10));
    const kept = worker.lastWake();
    await sleep(40);
    assert.equal(runs, 1, "a running worker's wake waits for its answer");
    worker.reply(replaced);
    assert.equal(runs, 1);
    worker.reply(kept);
    assert.equal(runs, 11);
    worker.reply(kept);
    assert.equal(runs, 11, "an answer runs its wake once");

    // Cancelled: the answer runs nothing.
    ticker.schedule(0.005, () => runs++);
    const cancelled = worker.lastWake();
    ticker.cancel();
    worker.reply(cancelled);
    assert.equal(runs, 11);
    delete (globalThis as { Worker?: unknown }).Worker;
});
