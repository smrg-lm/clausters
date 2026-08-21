// The shared transport (`gui/transport.ts`) — play/pause/stop/locate and the
// view's playhead line.
//
// No host and no server: a fake host records the sets, a fake server answers the
// clock query, and the pass is a real `Playhead` driven offline (`clock.render`)
// so the end of a pass is reached deterministically. What is checked is the line
// — which of the two numbers is written, in which unit — and the state machine
// around it, not what the widgets do with it.
//
// The same cases the Python client's `test_gui_transport.py` checks, because
// this is one object in two languages: what would drift is the arithmetic (the
// anchor, the units) and the tail rule, and both are pinned here.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import { ManualTimebase } from "../src/base/timebase.ts";
import { Transport } from "../src/gui/transport.ts";
import { Event as SeqEvent } from "../src/seq/event.ts";
import { Playhead, Timeline } from "../src/seq/timeline.ts";
import type { GuiHost } from "../src/gui/host.ts";
import type { Server } from "../src/defs/server/index.ts";

const here = new URL(".", import.meta.url);
await loadCore(await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)));

const SR = 48_000.0;
const TEMPO = 2.0; // beats per second (120 bpm)
const BEAT = SR / TEMPO; // 24000 samples per beat
const CLOCK = 1_000_000.0; // the sample-clock value the fake server reports

/** Records the sets the transport sends. */
class FakeHost {
    sets: [number, Record<string, unknown>][] = [];

    set(id: number, props: Record<string, unknown>): void {
        this.sets.push([id, props]);
    }

    /** The most recent value written for `key` (throws if never). */
    last(key: string): unknown {
        for (let i = this.sets.length - 1; i >= 0; i--) {
            const props = this.sets[i]?.[1] ?? {};
            if (key in props) return props[key];
        }
        throw new Error(`never set: ${key}`);
    }

    ids(key: string): number[] {
        return this.sets.filter(([, props]) => key in props).map(([id]) => id);
    }
}

/** Answers the anchor's clock query, `latency` seconds ahead of the sound. */
const fakeServer = (over: Record<string, unknown> = {}) =>
    ({
        latency: 0.25,
        scoring: false,
        request: async () => ({ addr: "/clock_query.reply", args: [CLOCK] }),
        ...over,
    }) as unknown as Server;

/** A destination that swallows what a pass renders. */
const recorder = { playEvent: () => null, sendMsg: () => {}, sendBundle: () => {} };

/** Three notes, one per beat: the piece ends at beat 2. */
const arp = () =>
    new Timeline(
        [0, 1, 2].map((i) => [i, new SeqEvent({ midinote: 60 + i, dur: 1.0 })] as const),
    );

/**
 * A clock whose beat is set by hand instead of by a ticker — a *rolling* clock
 * (its beat is the wall's, so a transport may sweep the last item's tail over
 * it) that a test can move deterministically.
 */
class RollingClock extends TempoClock {
    private beat = 0.0;

    override get rolling(): boolean {
        return true; // a driven clock, whatever `render` left the mode on
    }

    override beats(): number {
        return this.beat;
    }

    advance(beats: number): void {
        this.beat += beats;
    }
}

function makeClock(): TempoClock {
    return new TempoClock(TEMPO, { timebase: new ManualTimebase(0), ticker: manualTicker() });
}

function makeTransport(
    host: FakeHost = new FakeHost(),
    { clock = makeClock(), extent }: { clock?: TempoClock; extent?: () => number } = {},
): Transport {
    return new Transport(host as unknown as GuiHost, 7, {
        source: (at) =>
            new Playhead(arp(), clock, recorder as never).play({ at }),
        tempo: TEMPO,
        sampleRate: SR,
        extent,
        clock,
    });
}

// ---- the static cursor: the stopped half of the line ----

test("a locate draws the cursor and turns the anchor off", () => {
    const host = new FakeHost();
    makeTransport(host).locate(2.0);
    assert.equal(host.last("playhead"), 2 * BEAT);
    assert.equal(host.last("playheadAt"), -1.0);
});

test("the cursor is drawn in the view's own unit", () => {
    const host = new FakeHost();
    const tp = new Transport(host as unknown as GuiHost, 7, {
        source: () => null,
        tempo: TEMPO,
        sampleRate: SR,
        // An engraved page: milliseconds, not timeline samples.
        toUnits: (beats) => (beats * 1000.0) / TEMPO,
    });
    tp.locate(2.0);
    assert.equal(host.last("playhead"), 1000.0);
});

test("a locate never goes negative", () => {
    const host = new FakeHost();
    makeTransport(host).locate(-5.0);
    assert.equal(host.last("playhead"), 0.0);
});

test("stop returns to the top and pause keeps the position", () => {
    const tp = makeTransport();
    tp.locate(3.0);
    assert.equal(tp.at, 3.0);
    assert.equal(tp.pause(), 3.0);
    tp.stop();
    assert.equal(tp.at, 0.0);
});

test("no host, no line", () => {
    const tp = new Transport(null, 7, { source: () => null, tempo: TEMPO, sampleRate: SR });
    tp.locate(1.0); // must not throw
    assert.equal(tp.at, 1.0);
});

// ---- the anchor: the playing half ----

test("the anchor is the clock less what has been played", async () => {
    const host = new FakeHost();
    const tp = makeTransport(host);
    assert.equal(await tp.anchor(fakeServer(), { at: 2.0 }), true);
    // now = clock + latency·sr; the origin is that, less two beats.
    assert.equal(host.last("playheadAt"), CLOCK + 0.25 * SR - 2 * BEAT);
});

test("an NRT destination has nothing to anchor to", async () => {
    const tp = makeTransport();
    assert.equal(await tp.anchor(fakeServer({ scoring: true })), false);
});

test("a destination that cannot be asked answers false", async () => {
    const { ReplyTimeout } = await import("../src/errors.ts");
    const tp = makeTransport();
    const silent = fakeServer({
        request: async () => {
            throw new ReplyTimeout("/clock_query");
        },
    });
    assert.equal(await tp.anchor(silent), false);
});

test("playing takes the line over from the cursor", async () => {
    const host = new FakeHost();
    const tp = makeTransport(host);
    await tp.play(fakeServer(), { at: 1.0 });
    assert.equal(host.last("playheadAt"), CLOCK + 0.25 * SR - BEAT);
    assert.ok(tp.playing);
});

test("pause holds the cursor where the music stopped", async () => {
    const host = new FakeHost();
    const clock = makeClock();
    const tp = makeTransport(host, { clock });
    await tp.play(fakeServer(), { at: 0.0 });
    tp.pause();
    assert.equal(host.last("playheadAt"), -1.0);
    assert.equal(tp.at, tp.position);
});

test("a seek while playing starts a fresh pass", async () => {
    const tp = makeTransport();
    await tp.play(fakeServer(), { at: 0.0 });
    const first = tp.playhead;
    tp.locate(1.0);
    assert.notEqual(tp.playhead, first, "a new pass, so a seek picks up an edit");
    assert.equal(tp.at, 1.0);
});

test("a bare play resumes from where it was left", async () => {
    const tp = makeTransport();
    tp.locate(2.0);
    await tp.play(fakeServer());
    assert.equal(tp.at, 2.0);
});

// ---- the end of a pass ----

test("the end of a pass parks the cursor at the extent", async () => {
    const host = new FakeHost();
    const clock = makeClock();
    const tp = makeTransport(host, { clock, extent: () => 3.0 });
    await tp.play(fakeServer(), { at: 0.0 });
    clock.render();
    assert.equal(tp.update(), true);
    assert.equal(tp.position, 3.0);
    assert.equal(host.last("playhead"), 3 * BEAT);
    assert.equal(host.last("playheadAt"), -1.0);
});

test("the end is reported once", async () => {
    const clock = makeClock();
    const tp = makeTransport(new FakeHost(), { clock, extent: () => 3.0 });
    await tp.play(fakeServer(), { at: 0.0 });
    clock.render();
    assert.equal(tp.update(), true);
    assert.equal(tp.update(), false);
});

test("without an extent it parks on the last item", async () => {
    const clock = makeClock();
    const tp = makeTransport(new FakeHost(), { clock });
    await tp.play(fakeServer(), { at: 0.0 });
    clock.render();
    assert.equal(tp.update(), true);
    assert.equal(tp.position, 2.0, "the last item's onset");
});

// ---- the tail: a drained scan is not the end of the piece ----

test("the last item keeps the line until the piece actually ends", async () => {
    const host = new FakeHost();
    const clock = new RollingClock(TEMPO, {
        timebase: new ManualTimebase(0),
        ticker: manualTicker(),
    });
    const tp = makeTransport(host, { clock, extent: () => 3.0 });
    await tp.play(fakeServer(), { at: 0.0 });
    clock.render(); // the scan drains on the last item

    const anchored = host.last("playheadAt");
    assert.equal(tp.update(), false, "the last item is still sounding");
    assert.equal(tp.position, 2.0, "the last item's onset");
    assert.equal(host.last("playheadAt"), anchored, "the line is left sweeping");

    clock.advance(0.5); // half a beat into that last item
    assert.equal(tp.update(), false);
    assert.equal(tp.position, 2.5);
    assert.ok(tp.playing, "still sounding, so the button says pause");

    clock.advance(0.6); // past the piece's end
    assert.equal(tp.update(), true, "the piece ended");
    assert.equal(tp.playing, false);
    assert.equal(tp.position, 3.0);
    assert.equal(host.last("playhead"), 3 * BEAT);
    assert.equal(host.last("playheadAt"), -1.0);
});

test("a pause inside the tail holds where the music is", async () => {
    const clock = new RollingClock(TEMPO, {
        timebase: new ManualTimebase(0),
        ticker: manualTicker(),
    });
    const tp = makeTransport(new FakeHost(), { clock, extent: () => 3.0 });
    await tp.play(fakeServer(), { at: 0.0 });
    clock.render();
    clock.advance(0.5);
    tp.update();
    tp.pause();
    assert.equal(tp.at, 2.5, "not the beat the pass started from");
});

test("a locate after the end stands", async () => {
    const clock = makeClock();
    const tp = makeTransport(new FakeHost(), { clock, extent: () => 3.0 });
    await tp.play(fakeServer(), { at: 0.0 });
    clock.render();
    tp.update();
    tp.locate(1.0);
    assert.equal(tp.update(), false, "seeking away from the end is not undone");
    assert.equal(tp.position, 1.0);
});

test("a pass stopped by hand did not end", async () => {
    const tp = makeTransport(new FakeHost(), { extent: () => 3.0 });
    await tp.play(fakeServer(), { at: 0.0 });
    tp.pause();
    assert.equal(tp.update(), false);
});

// ---- the widgets the line goes to ----

test("the ids are read on each use", () => {
    const host = new FakeHost();
    const lanes = [10, 11];
    const tp = makeTransport(host);
    tp.ids = () => lanes;
    tp.locate(1.0);
    assert.deepEqual(host.ids("playhead"), [10, 11]);
    lanes.push(12);
    tp.locate(2.0);
    assert.deepEqual(host.ids("playhead").slice(-3), [10, 11, 12]);
});
