// Events, patterns, timelines: the values they produce and the beats they
// produce them at.
//
// The destination here is a recorder rather than a server, so what is asserted
// is the sequence itself — which event, at which logical beat. What goes on
// the wire is `timed-send.test.ts`'s job.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import type { ManualTicker } from "../src/base/clock.ts";
import { ManualTimebase } from "../src/base/timebase.ts";
import { currentRoutine } from "../src/base/context.ts";
import { seed } from "../src/base/rand.ts";
import { Routine } from "../src/base/stream.ts";
import {
    Event,
    INF,
    Pbind,
    Pgeom,
    Pn,
    Prand,
    Pseq,
    Pser,
    Pseries,
    Pwhite,
    OscEvent,
    Playhead,
    Timeline,
    rest,
} from "../src/seq/index.ts";
import type { EventDestination, PlayDestination } from "../src/seq/index.ts";
import type { OscHandler } from "../src/base/receiver.ts";
import type { Server } from "../src/defs/server/index.ts";
import { flush } from "./flush.ts";

await loadCore(
    await readFile(
        new URL("../dist/core/clausters_core_web_bg.wasm", new URL(".", import.meta.url)),
    ),
);

/** A destination that records what was played and when, instead of sending. */
function recorder() {
    const played: { beat: number; event: Event }[] = [];
    const messages: { beat: number; addr: string }[] = [];
    const destination = {
        played,
        messages,
        playEvent(event: Event) {
            // The rest rule is the destination's, as it is on a real Server:
            // a rest sounds nothing, and says so by returning no node.
            if (event.get("type") === "rest") return null;
            played.push({ beat: currentRoutine()?.logicalBeat ?? 0, event });
            return 1000 + played.length;
        },
        free() {},
        set() {},
        sendBundle(msgs: readonly [string, ...unknown[]][]) {
            for (const [addr] of msgs) {
                messages.push({ beat: currentRoutine()?.logicalBeat ?? 0, addr });
            }
        },
    };
    return destination as typeof destination & PlayDestination & EventDestination;
}

function harness(tempo = 1.0) {
    const timebase = new ManualTimebase(0);
    const ticker = manualTicker();
    const clock = new TempoClock(tempo, { timebase, ticker });
    const run = async (seconds: number) => {
        await flush();
        const target = timebase.now() + seconds;
        for (;;) {
            const pending = (ticker as ManualTicker).pending;
            if (pending === null || timebase.now() + pending > target) break;
            timebase.advance(pending);
            ticker.fire();
        }
        timebase.set(target);
    };
    return { clock, timebase, run };
}

const values = <T>(pattern: Iterable<T>, n = 8): T[] => {
    const out: T[] = [];
    for (const value of pattern) {
        out.push(value);
        if (out.length === n) break;
    }
    return out;
};

// ---- the value patterns ----

test("the ordered patterns yield what they say", () => {
    assert.deepEqual(values(new Pseq([1, 2, 3], 2)), [1, 2, 3, 1, 2, 3]);
    assert.deepEqual(values(new Pser([1, 2, 3], 5)), [1, 2, 3, 1, 2]);
    assert.deepEqual(values(new Pseries(0, 2, 4)), [0, 2, 4, 6]);
    assert.deepEqual(values(new Pgeom(1, 3, 4)), [1, 3, 9, 27]);
    assert.deepEqual(values(new Pn(new Pseq([1, 2]), 2)), [1, 2, 1, 2]);
});

test("a sub-pattern used as a value is embedded in place", () => {
    assert.deepEqual(values(new Pseq([1, new Pseq([8, 9]), 2])), [1, 8, 9, 2]);
});

test("an endless pattern keeps going", () => {
    assert.deepEqual(values(new Pseq([1, 2], INF), 5), [1, 2, 1, 2, 1]);
});

test("the random patterns are reproduced by the root seed", () => {
    seed(99);
    const first = values(new Pwhite(0, 1, 4)).concat(values(new Prand([10, 20, 30], 4)));
    seed(99);
    const second = values(new Pwhite(0, 1, 4)).concat(values(new Prand([10, 20, 30], 4)));
    assert.deepEqual(first, second);
    assert.ok(first.slice(0, 4).every((v) => v >= 0 && v < 1));
});

// ---- events ----

test("an event derives pitch, delta and sustain from its keys", () => {
    const event = new Event({ degree: 4, dur: 2, legato: 0.5, stretch: 1.5 });
    assert.equal(event.midinote(), 67); // the 5th degree of C major at octave 5
    assert.equal(event.delta(), 3); // dur * stretch
    assert.equal(event.sustain(), 1.5); // dur * legato * stretch
    // An explicit key overrides the calculation, as in SuperCollider.
    assert.equal(new Event({ dur: 2, delta: 0.25 }).delta(), 0.25);
    assert.equal(new Event({ dur: 2, sustain: 9 }).sustain(), 9);
});

test("an explicit freq wins over midinote, which wins over degree", () => {
    assert.equal(new Event({ freq: 440, midinote: 40, degree: 0 }).freq(), 440);
    assert.equal(new Event({ midinote: 69, degree: 0 }).midinote(), 69);
    assert.equal(new Event({}).midinote(), 60, "middle C with nothing to go on");
});

test("the control tail carries the derived pitch and the custom keys", () => {
    const args = new Event({ instrument: "sine", degree: 0, amp: 0.3, cutoff: 800 })
        .controlArgs()
        .map(([tag, value]) => `${tag}:${String(value)}`);
    assert.deepEqual(args.slice(0, 4), ["s:freq", "f:261.62554931640625", "s:amp", "f:0.3"]);
    assert.ok(args.includes("s:cutoff"), "an unreserved numeric key is a control");
    assert.ok(!args.some((a) => a.includes("legato")), "a reserved key is not");
});

// ---- playing patterns on a clock ----

test("a pattern plays its events at the beats its deltas add up to", async () => {
    const destination = recorder();
    const { clock, run } = harness(1.0);
    clock.start();
    new Pbind({
        degree: new Pseq([0, 2, 4]),
        dur: new Pseq([0.5, 0.25, 1]),
    }).play(destination, { clock });
    await run(4);

    assert.deepEqual(
        destination.played.map((p) => p.beat),
        [0, 0.5, 0.75],
    );
    assert.deepEqual(
        destination.played.map((p) => p.event.get("degree")),
        [0, 2, 4],
    );
});

test("a Pbind stops when any of its keys runs out", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    clock.start();
    new Pbind({ degree: new Pseq([0, 1, 2, 3]), amp: new Pseq([0.1, 0.2]) }).play(
        destination,
        { clock },
    );
    await run(8);
    assert.equal(destination.played.length, 2);
});

test("stopping a player leaves the clock alone", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    clock.start();
    const player = new Pbind({ degree: new Pseq([0], INF), dur: 1 }).play(destination, {
        clock,
    });
    await run(2.5);
    assert.equal(destination.played.length, 3);
    player.stop();
    await run(5);
    assert.equal(destination.played.length, 3);
    assert.equal(clock.queued, 0);
});

test("a rest advances time without sounding", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    clock.start();
    clock.play(
        new Routine(function* () {
            rest(2).play(destination);
            yield rest(2).delta();
            new Event({ degree: 0 }).play(destination);
            yield 1;
        }),
    );
    await run(4);
    assert.equal(destination.played.length, 1, "the rest sounded nothing");
    assert.equal(destination.played[0]!.beat, 2, "...but it did take its time");
});

// ---- timelines ----

test("a timeline keeps its items in beat order, however they are added", () => {
    const timeline = new Timeline([
        [2, "c"],
        [0, "a"],
    ]);
    timeline.add(1, "b");
    assert.deepEqual([...timeline], [
        [0, "a"],
        [1, "b"],
        [2, "c"],
    ]);
    assert.equal(timeline.duration(), 2);
});

test("items added at the same beat keep their insertion order", () => {
    const timeline = new Timeline();
    timeline.add(1, "first");
    timeline.add(1, "second");
    timeline.add(0, "zero");
    assert.deepEqual([...timeline].map(([, item]) => item), ["zero", "first", "second"]);
});

test("a timeline reads by time, and edits by handle", () => {
    const timeline = new Timeline();
    const a = timeline.add(0, "a");
    timeline.add(1, "b");
    timeline.add(2.5, "c");

    assert.equal(timeline.indexAt(1), 1);
    assert.equal(timeline.indexAt(1.5), 2);
    assert.deepEqual(timeline.range(1, 2.5), [[1, "b"]]);
    assert.deepEqual(timeline.at(2.5), ["c"]);

    timeline.move(a, 3);
    assert.deepEqual([...timeline].map(([, item]) => item), ["b", "c", "a"]);
    timeline.remove(a);
    assert.equal(timeline.length, 2);
});

test("quantize snaps every placement to the grid", () => {
    const timeline = new Timeline([
        [0.1, "a"],
        [0.9, "b"],
        [2.4, "c"],
    ]);
    timeline.quantize(0.5);
    assert.deepEqual([...timeline].map(([beat]) => beat), [0, 1, 2.5]);
    timeline.quantize(0); // a no-op, not a collapse
    assert.deepEqual([...timeline].map(([beat]) => beat), [0, 1, 2.5]);
});

test("a pattern bounces into a timeline at the beats it would have played", () => {
    const timeline = Timeline.fromPattern(
        new Pbind({ degree: new Pseq([0, 1, 2]), dur: 0.5 }),
    );
    assert.deepEqual([...timeline].map(([beat]) => beat), [0, 0.5, 1]);
    assert.equal(timeline.length, 3);
    assert.equal((timeline.get(1)![1] as Event).get("degree"), 1);
});

test("bouncing an endless pattern needs a bound, and honours it", () => {
    const timeline = Timeline.fromPattern(
        new Pbind({ degree: new Pseq([0, 1], INF), dur: 0.25 }),
        { dur: 1 },
    );
    assert.deepEqual([...timeline].map(([beat]) => beat), [0, 0.25, 0.5, 0.75, 1]);
});

// ---- the playhead ----

test("a playhead renders a timeline forward as the clock advances", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    const timeline = new Timeline([
        [0, new Event({ degree: 0 })],
        [1, new Event({ degree: 1 })],
        [2.5, new Event({ degree: 2 })],
    ]);
    const playhead = new Playhead(timeline, clock, destination);
    clock.start();
    playhead.play();
    await run(4);
    assert.deepEqual(destination.played.map((p) => p.beat), [0, 1, 2.5]);
    assert.equal(playhead.playing, false);
    assert.equal(playhead.finished, true, "the scan ran off the end");
});

test("play({ at }) seeks: the scan starts from there", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    const timeline = new Timeline([
        [0, new Event({ degree: 0 })],
        [1, new Event({ degree: 1 })],
        [2, new Event({ degree: 2 })],
    ]);
    new Playhead(timeline, clock, destination).play({ at: 1 });
    clock.start();
    await run(4);
    assert.deepEqual(
        destination.played.map((p) => p.event.get("degree")),
        [1, 2],
    );
});

test("stop halts the scan and holds the position; locate moves it", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    const timeline = new Timeline([
        [0, new Event({ degree: 0 })],
        [1, new Event({ degree: 1 })],
        [2, new Event({ degree: 2 })],
    ]);
    const playhead = new Playhead(timeline, clock, destination);
    clock.start();
    playhead.play();
    await run(1.5);
    assert.equal(destination.played.length, 2);
    playhead.stop();
    assert.equal(playhead.playing, false);
    assert.equal(playhead.finished, false, "halted by hand, not ended");
    await run(5);
    assert.equal(destination.played.length, 2, "a stopped playhead renders nothing");

    playhead.locate(2);
    assert.equal(playhead.position(), 2);
    playhead.play({ at: 2 });
    await run(1);
    assert.deepEqual(
        destination.played.map((p) => p.event.get("degree")),
        [0, 1, 2],
    );
});

test("a loop wraps the scan back to the window's start", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    const timeline = new Timeline([
        [0, new Event({ degree: 0 })],
        [1, new Event({ degree: 1 })],
        [4, new Event({ degree: 9 })],
    ]);
    const playhead = new Playhead(timeline, clock, destination);
    playhead.loop(0, 2);
    clock.start();
    playhead.play();
    await run(5.5);
    // Two items per two-beat pass, and the item outside the window is never
    // reached however long it runs.
    assert.deepEqual(
        destination.played.map((p) => p.event.get("degree")),
        [0, 1, 0, 1, 0, 1],
    );
    assert.equal(playhead.playing, true, "a loop never ends");
});

test("a raw OSC item on a timeline is sent at its beat", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    const timeline = new Timeline([
        [0.5, new OscEvent("/node_set", ["i", 1000], "gate", ["f", 0])],
    ]);
    new Playhead(timeline, clock, destination).play();
    clock.start();
    await run(2);
    assert.deepEqual(destination.messages, [{ beat: 0.5, addr: "/node_set" }]);
});

test("a playhead follows the server's transport broadcasts", async () => {
    const destination = recorder();
    const { clock, run } = harness();
    const timeline = new Timeline([
        [0, new Event({ degree: 0 })],
        [1, new Event({ degree: 1 })],
        [2, new Event({ degree: 2 })],
    ]);
    const playhead = new Playhead(timeline, clock, destination);
    clock.start();

    // A fake server: what a follower touches is the notify registration, the
    // receiving door its responder registers with, and one read of the current
    // state.
    const handlers = new Set<OscHandler>();
    let notified = false;
    const server = {
        notify: async () => {
            notified = true;
        },
        receiver: {
            add: (handler: OscHandler) => {
                handlers.add(handler);
                return handler;
            },
            remove: (handler: OscHandler) => handlers.delete(handler),
        },
        // The state is **always** answered now: a transport exists whether or
        // not a grid does. A `tempo` of null is a server with no beat grid,
        // which is what stops the initial apply here (the Python fake in
        // `tests/test_timeline.py` says the same).
        transportState: async () => ({ tempo: null, playing: false, position: 0 }),
    } as unknown as Server;
    // origin, tempo, defined, playing, position, group, transportSample
    const broadcast = (playing: number, position: number) => {
        for (const handler of [...handlers]) {
            handler("/transport_query.reply", [0, 1.0, 1, playing, position, -1, 0], null, "page");
        }
    };

    await playhead.followTransport(server);
    assert.equal(notified, true, "a follower registers for the pushes");
    assert.equal(playhead.playing, false, "nothing rolls until the server says so");

    broadcast(1, 1); // the conductor rolls, from song position 1
    assert.equal(playhead.playing, true);
    await run(1.5);
    assert.deepEqual(
        destination.played.map((p) => p.event.get("degree")),
        [1, 2],
        "it rolled from where the broadcast said, not from the top",
    );

    broadcast(0, 0); // stop, and locate back to the top
    assert.equal(playhead.playing, false);
    assert.equal(playhead.position(), 0);
    await run(4);
    assert.equal(destination.played.length, 2, "a halted follower renders nothing");

    playhead.unfollowTransport();
    broadcast(1, 0);
    assert.equal(playhead.playing, false, "an unfollowed playhead ignores the wire");
});
