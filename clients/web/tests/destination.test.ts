// `Moment` and `OscDestination`: sending OSC to applications that are not ours.
//
// Two things under test. `Moment` is the one answer to "what time is it *for
// this event*" — the running routine's exact beat, a foreign clock's own now,
// or the clockless wall clock outside any routine. `OscDestination` is what
// carries that onto the wire for an application we do not control: standard
// OSC, no latency and no server-only commands.
//
// The Python client's `tests/test_destination.py` is the same suite; keep them
// reading alike.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { decodePacket } from "../src/base/osc.ts";
import type { Connection } from "../src/base/connection.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import type { ManualTicker } from "../src/base/clock.ts";
import { ManualTimebase } from "../src/base/timebase.ts";
import { Routine } from "../src/base/stream.ts";
import { Moment } from "../src/base/moment.ts";
import { OscDestination } from "../src/base/destination.ts";
import { flush } from "./flush.ts";

await loadCore();

function recorder(): Connection & { packets: Uint8Array[] } {
    const packets: Uint8Array[] = [];
    return {
        packets,
        send: (packet) => packets.push(packet),
        addReply: () => {},
        removeReply: () => {},
        close: () => {},
    };
}

const timetagOf = (packet: Uint8Array): number => {
    const view = new DataView(packet.buffer, packet.byteOffset, packet.byteLength);
    const ntp = view.getBigUint64(8);
    return Number(ntp >> 32n) - 2_208_988_800 + Number(ntp & 0xffffffffn) / 2 ** 32;
};

/** A clock on manual seams, so a routine's logical beat is exactly known. */
function harness(tempo = 1.0) {
    const timebase = new ManualTimebase();
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
    };
    return { clock, timebase, run };
}

// ---- Moment ----

test("outside a routine the moment is the wall clock", () => {
    const m = Moment.current();
    assert.equal(m.clock, null);
    assert.equal(m.beat, 0);
    assert.equal(m.secs(), 0);
    assert.ok(Math.abs(m.instant() - Date.now() / 1000) < 0.5);
    // A delay on a clockless moment is a duration in seconds.
    assert.ok(Math.abs(m.at(2).instant() - (Date.now() / 1000 + 2)) < 0.5);
});

test("inside a routine the moment is the exact logical beat", async () => {
    const seen: Moment[] = [];
    const { clock, run } = harness(2.0);
    clock.start();
    clock.play(
        new Routine(function* () {
            seen.push(Moment.current());
            yield 1.5;
            seen.push(Moment.current());
        }),
    );
    await run(10);
    clock.stop();

    assert.deepEqual(seen.map((m) => m.beat), [0, 1.5]);
    assert.ok(seen.every((m) => m.clock === clock));
    // Seconds are the clock's own axis: 1.5 beats at 2 beats/s.
    assert.ok(Math.abs(seen[1]!.secs() - 0.75) < 1e-9);
});

test("a foreign clock is asked for its own now", async () => {
    // A routine's exact beat belongs to *its* clock; another one is asked for
    // its own, which is what keeps a cross-clock send on the right axis.
    const seen: Array<[Moment, Moment]> = [];
    const theirs = harness(1.0);
    theirs.clock.start();
    theirs.timebase.advance(3.0);
    const ours = harness(1.0);
    ours.clock.start();

    ours.clock.play(
        new Routine(function* () {
            seen.push([Moment.current(), Moment.current(theirs.clock)]);
            yield undefined;
        }),
    );
    await ours.run(1);
    ours.clock.stop();
    theirs.clock.stop();

    const [own, foreign] = seen[0]!;
    assert.equal(own.clock, ours.clock);
    assert.equal(own.beat, 0);
    assert.equal(foreign.clock, theirs.clock);
    assert.ok(foreign.beat > 0, "the foreign clock reports its own elapsed beat");
});

test("at() moves along the same clock", () => {
    const { clock } = harness(4.0);
    const m = new Moment(clock, 2.0);
    assert.equal(m.at(1).beat, 3);
    assert.equal(m.at(1).clock, clock);
    assert.ok(Math.abs(m.at(1).secs() - 0.75) < 1e-9);
});

// ---- OscDestination ----

test("a destination sends a plain message", () => {
    const connection = recorder();
    new OscDestination(connection).sendMsg("/hello", 1, 2.5, "there");

    const [msg] = decodePacket(connection.packets[0]!);
    assert.equal(msg!.addr, "/hello");
    assert.equal(Number(msg!.args[0]), 1);
    assert.ok(Math.abs(Number(msg!.args[1]) - 2.5) < 1e-6);
    assert.equal(msg!.args[2], "there");
});

test("a destination bundles at the routine's logical beat", async () => {
    // The payload of the design: another application gets the same logical
    // timing the server does, with no clock knowledge of its own.
    const connection = recorder();
    const dest = new OscDestination(connection);
    const { clock, run } = harness(1.0);
    clock.start();
    const origin = clock.startTime!;

    clock.play(
        new Routine(function* () {
            dest.sendBundle([["/one"]]);
            dest.sendBundle([["/two"]], { delayBeats: 0.25 });
            yield undefined;
        }),
    );
    await run(1);
    clock.stop();

    assert.equal(connection.packets.length, 2);
    const [first, second] = connection.packets.map(timetagOf);
    assert.ok(Math.abs(first! - origin) < 0.01, "at the beat, not at now");
    assert.ok(Math.abs(second! - first! - 0.25) < 1e-6);
});

test("a destination carries no latency", () => {
    // Latency is our audio pipeline's property, not an external app's: a
    // destination never adds one, so its timetag is the moment itself.
    const connection = recorder();
    const { clock } = harness(1.0);
    clock.start();
    const at = new Moment(clock, 4.0);
    new OscDestination(connection).sendBundle([["/x"]], { at });
    clock.stop();

    assert.ok(Math.abs(timetagOf(connection.packets[0]!) - at.instant()) < 1e-6);
    assert.ok(Math.abs(at.instant() - (clock.startTime! + 4)) < 1e-9);
});
