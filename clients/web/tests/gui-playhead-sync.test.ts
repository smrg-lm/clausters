// The shared playhead sync (`gui/playhead-sync.ts`) — play/pause/stop/locate
// and the views' playhead line.
//
// No host and no server: a fake host records the sets, a fake server answers the
// clock query, and the pass is a real `Playhead` driven offline (`clock.render`)
// so the end of a pass is reached deterministically. What is checked is the line
// — which of the two numbers is written, in which unit — and the state machine
// around it, not what the widgets do with it.
//
// The same cases the Python client's `test_gui_playhead_sync.py` checks, because
// this is one object in two languages: what would drift is the arithmetic (the
// anchor, the units) and the tail rule, and both are pinned here.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import { ManualTimebase } from "../src/base/timebase.ts";
import { TempoMap } from "../src/base/time.ts";
import { PlayheadSync } from "../src/gui/playhead-sync.ts";
import { Event as SeqEvent } from "../src/seq/event.ts";
import { Playhead, Timeline } from "../src/seq/timeline.ts";
import type { GuiHost } from "../src/gui/host.ts";
import type { Server } from "../src/defs/server/index.ts";

const here = new URL(".", import.meta.url);
await loadCore();

const SR = 48_000.0;
const TEMPO = 2.0; // beats per second (120 bpm)
/** What the passes play, as far as the line is concerned: its map is the tempo. */
const PIECE = new Timeline([], { tempo: TEMPO });
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
): PlayheadSync {
    return new PlayheadSync(host as unknown as GuiHost, 7, {
        source: (at) =>
            new Playhead(arp(), clock, recorder as never).play({ at }),
        structure: PIECE,
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
    const tp = new PlayheadSync(host as unknown as GuiHost, 7, {
        source: () => null,
        structure: PIECE,
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
    const tp = new PlayheadSync(null, 7, { source: () => null, structure: PIECE, sampleRate: SR });
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

// ---- the end of a pass, noticed without anyone asking ----------------------

/** A `FakeHost` with an application clock, recording what is scheduled on it. */
class ClockedHost extends FakeHost {
    readonly scheduled: [number, () => number | undefined][] = [];
    readonly clock = {
        sched: (delay: number, item: () => number | undefined) => {
            this.scheduled.push([delay, item]);
            return item;
        },
    };
}

test("a play puts the end of the pass on the application clock", async () => {
    // `update` used to be "call it once per pass of the caller's loop", which is
    // a hand-written tick by another name and is why every example had one. A
    // play schedules it on the host's own clock instead, and it stops asking
    // when the piece stops sounding.
    const host = new ClockedHost();
    const clock = makeClock();
    const tp = new PlayheadSync(host as unknown as GuiHost, 7, {
        source: (at) => new Playhead(arp(), clock, recorder as never).play({ at }),
        structure: PIECE,
        sampleRate: SR,
        extent: () => 3.0,
        clock,
    });
    await tp.play(fakeServer(), { at: 0.0 });

    assert.equal(host.scheduled.length, 1, "one tick, scheduled by the play");
    const [delay, tick] = host.scheduled[0]!;
    assert.ok(delay > 0);
    assert.equal(tick(), delay, "a number keeps it going: the clock reschedules by it");

    clock.render();                          // the pass runs out
    assert.equal(tick(), delay, "the drained scan is what it is there to notice");
    assert.equal(tp.position, 3.0, "so the cursor parks at the piece's end");
    assert.equal(tick(), undefined, "and having parked, it stops asking");

    await tp.play(fakeServer(), { at: 0.0 });
    assert.equal(host.scheduled.length, 2, "the next play starts it again");
});

test("a transport with no host clock keeps update manual", async () => {
    // A view built before it is opened has no clock to schedule on, and `update`
    // is the plain call it always was.
    const clock = makeClock();
    const tp = makeTransport(new FakeHost(), { clock, extent: () => 3.0 });
    await tp.play(fakeServer(), { at: 0.0 });
    clock.render();
    assert.equal(tp.update(), true);
});

// ---- the piece: the server owns the position, and the host reads it ----

/** A server whose transport is the piece's: records the commands, answers where
 * it is. */
class TransportServer {
    calls: unknown[][] = [];
    state = { playing: false, positionSample: 0, loop: null as unknown };

    async transportPlay(): Promise<void> {
        this.calls.push(["play"]);
        this.state.playing = true;
    }

    async transportStop(): Promise<void> {
        this.calls.push(["stop"]);
        this.state.playing = false;
    }

    async transportLocateSample(sample: number): Promise<void> {
        this.calls.push(["locate", Math.trunc(sample)]);
        this.state.positionSample = Math.trunc(sample);
    }

    async transportLoop(span: [number, number] | null): Promise<void> {
        this.calls.push(["loop", span]);
        this.state.loop = span;
    }

    async transportState(): Promise<typeof this.state> {
        return { ...this.state };
    }
}

/** A host that also records `headClock`. */
class HeadClockHost extends FakeHost {
    head: string | null = null;

    headClock(which: string): void {
        this.head = which;
    }
}

function pieceTransport(host?: HeadClockHost): PlayheadSync {
    const tp = new PlayheadSync((host ?? new HeadClockHost()) as unknown as GuiHost, 7, {
        headClock: "piece",
        structure: PIECE,
        sampleRate: SR,
    });
    tp.server = new TransportServer() as unknown as Server;
    return tp;
}

test("a piece transport tells the host which counter to draw", async () => {
    // The two halves of one decision, so they cannot disagree: the client stops
    // computing the line and the host starts reading the piece's position.
    const host = new HeadClockHost();
    const tp = pieceTransport(host);
    assert.equal(host.head, "piece");
    await tp.play();
    assert.equal(host.last("playhead_at"), 0.0);
});

test("a piece transport's verbs are the server's", async () => {
    const tp = pieceTransport();
    const server = tp.server as unknown as TransportServer;
    await tp.play();
    tp.pause();
    tp.locate(3.0);
    tp.stop();
    assert.deepEqual(server.calls, [
        ["play"],
        ["stop"],
        ["locate", Math.trunc(3 * BEAT)],
        ["stop"],
        ["locate", 0],
    ]);
});

test("a piece transport reads where it is instead of keeping it", async () => {
    // The whole point: the position is the engine's, so a locate nobody here
    // sent -- a loop's wrap, another client's seek -- is still where it says.
    const tp = pieceTransport();
    const server = tp.server as unknown as TransportServer;
    server.state.positionSample = Math.trunc(5 * BEAT);
    server.state.playing = true;
    assert.equal(tp.position, 0.0, "nothing was asked yet, so nothing is known yet");
    await tp.refresh();
    assert.ok(Math.abs(tp.position - 5.0) < 1e-9);
    assert.ok(tp.playing);
});

test("a locate while the piece plays does not re-cue anything", async () => {
    // A device-clock transport throws the pass away and starts another; the
    // piece's seeks in the engine, so the sound carries on from there.
    const tp = pieceTransport();
    const server = tp.server as unknown as TransportServer;
    await tp.play();
    server.calls.length = 0;
    tp.locate(4.0);
    assert.deepEqual(server.calls, [["locate", Math.trunc(4 * BEAT)]]);
});

test("a piece transport loops in the engine", () => {
    const tp = pieceTransport();
    const server = tp.server as unknown as TransportServer;
    tp.loop(1.0, 3.0);
    assert.deepEqual(server.calls.at(-1), [
        "loop",
        [Math.trunc(1 * BEAT), Math.trunc(3 * BEAT)],
    ]);
    tp.loop(null);
    assert.deepEqual(server.calls.at(-1), ["loop", null]);
});

test("a piece still cues a pass of voices, and only on a locate", async () => {
    // The two halves meet in `play`: what follows the transport by itself needs
    // no pass, and what fires voices does -- so a source is still called, and a
    // locate cues it again while nothing re-cues on an edit.
    const cued: number[] = [];
    const host = new HeadClockHost();
    const tp = new PlayheadSync(host as unknown as GuiHost, 7, {
        headClock: "piece",
        source: (at) => {
            cued.push(at);
            return null;
        },
        structure: PIECE,
        sampleRate: SR,
    });
    tp.server = new TransportServer() as unknown as Server;
    await tp.play();
    assert.deepEqual(cued, [0.0]);
    tp.locate(2.0);
    assert.deepEqual(cued, [0.0, 2.0], "the one re-cue the piece keeps");
    tp.pause();
    tp.locate(4.0);
    assert.deepEqual(cued, [0.0, 2.0], "stopped, there is no pass to cue");
});

test("the map is asked of what plays and never kept", async () => {
    // No tempo is held here: beats cross through the pass's own map (a
    // timeline holds one), else the structure's, read on each use.
    const passMap = new TempoMap(4.0);
    const pass = {
        map: passMap,
        playing: true,
        finished: false,
        position: () => 0,
        pause() { this.playing = false; },
        stop() { this.playing = false; },
        locate() {},
    };
    const piece = new Timeline([], { tempo: TEMPO });
    const tp = new PlayheadSync(new FakeHost() as unknown as GuiHost, 7, {
        source: () => pass as unknown as Timeline,
        structure: piece,
        sampleRate: SR,
    });
    assert.ok(Math.abs(tp.beatsToSamples(1.0) - BEAT) < 1e-6);
    piece.map.push(0.0, 1.0); // edited on the structure: followed
    assert.ok(Math.abs(tp.beatsToSamples(1.0) - SR) < 1e-6);
    await tp.play(fakeServer(), { at: 0.0 });
    assert.ok(Math.abs(tp.beatsToSamples(1.0) - SR / 4.0) < 1e-6);
});

test("with nothing to ask there is no tempo", () => {
    const tp = new PlayheadSync(new FakeHost() as unknown as GuiHost, 7, { sampleRate: SR });
    assert.throws(() => tp.beatsToSamples(1.0), /structure/);
});

