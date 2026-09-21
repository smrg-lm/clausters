// Events, patterns, timelines: the values they produce and the beats they
// produce them at.
//
// The destination here is a recorder rather than a server, so what is asserted
// is the sequence itself -- which event, at which logical beat. What goes on
// the wire is `timed-send.test.ts`'s job.

import assert from "node:assert/strict";
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
    EventPattern,
    INF,
    Pbind,
    Pgeom,
    Pn,
    Prand,
    Pseq,
    Pser,
    Pseries,
    Pwhite,
    OscItem,
    Timeline,
    rest,
} from "../src/seq/index.ts";
import type { EventDestination, PlayDestination } from "../src/seq/index.ts";
import type { OscHandler } from "../src/base/receiver.ts";
import type { Server } from "../src/defs/server/index.ts";
import { play } from "../src/play.ts";
import { flush } from "./flush.ts";

await loadCore();

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

test("a timeline refuses a value pattern", () => {
    // A value pattern is the definition of a generator and does not play, so it
    // is not an item; an event pattern is.
    const timeline = new Timeline();
    timeline.add(0, new Pbind({ freq: new Pseq([440, 550]), dur: 0.5 }));
    assert.throws(() => timeline.add(1, new Pseq([1, 2, 3])), /does not play/);
    assert.equal(timeline.length, 1);
});

test("a value pattern does not play", () => {
    // A pattern is the definition of a generator: what plays is an event
    // pattern, and a value pattern is refused by name.
    assert.throws(() => play(new Pwhite(0, 1)), /does not play/);
    assert.throws(() => play(new Pseq<unknown>([new Pbind({ degree: 0 }), 1])), /does not play/);
});

test("a list pattern over events is an event pattern", () => {
    // A list pattern resolves what it is when it is built: over event patterns
    // only it plays; a list that mixes events and values is a value pattern.
    const phrase = new Pbind({ freq: new Pseq([440, 550]), dur: 0.5 });
    assert.ok(new Pseq([phrase, phrase]) instanceof EventPattern);
    assert.ok(new Prand([phrase]) instanceof EventPattern);
    assert.ok(new Pn(phrase, 2) instanceof EventPattern);
    assert.ok(new Pseq([phrase, phrase]) instanceof Pseq);
    assert.ok(!(new Pseq<unknown>([phrase, 1]) instanceof EventPattern));
    assert.ok(!(new Pseq([1, 2]) instanceof EventPattern));
    assert.ok(!("play" in new Pseq([1, 2])));
    assert.ok(!(phrase instanceof Pseq), "a Pbind is not a list pattern");
    assert.deepEqual([...new Pseq([phrase], 2)].map((e) => e.get("freq")), [440, 550, 440, 550]);
});
