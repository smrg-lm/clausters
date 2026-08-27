// The timed-send path: what actually goes on the wire, and when.
//
// A fake carrier captures the packets and they are decoded, so "exact timing"
// becomes a hard assertion rather than a claim: the timetag of every bundle
// under a monotonic timebase, and the absolute `/sched_at` sample under a sample
// timebase. Both are computed from the routine's *logical* beat, which is the
// property the whole layer exists for.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { decodePacket } from "../src/base/osc.ts";
import type { Connection } from "../src/base/connection.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import type { ManualTicker } from "../src/base/clock.ts";
import { SampleTimebase, secsToSamples, unixToNtp } from "../src/base/timebase.ts";
import type { Timebase } from "../src/base/timebase.ts";
import { Routine } from "../src/base/stream.ts";
import { Server } from "../src/defs/server/index.ts";
import { Event } from "../src/seq/event.ts";
import { flush } from "./flush.ts";

await loadCore();

/**
 * A carrier that only records. Nothing replies, so the Server is opened with
 * explicit sizing and no `/server_notify`.
 */
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

const openServer = (connection: Connection) =>
    new Server(connection, {
        sizing: { maxNodes: 8192, audioBuses: 128, controlBuses: 16384, maxBuffers: 4096, channels: 2 },
    });

/**
 * Whether a packet is a bundle, and the NTP bits it is stamped with. The
 * codec flattens bundles on decode (a reply reader wants the messages), so
 * the timetag is read here from the wire layout: `#bundle\0` then 8 bytes.
 */
function timetagOf(packet: Uint8Array): bigint {
    const head = new TextDecoder().decode(packet.subarray(0, 8));
    assert.equal(head, "#bundle\0", "expected a bundle");
    const view = new DataView(packet.buffer, packet.byteOffset, packet.byteLength);
    return view.getBigUint64(8);
}

const isBundle = (packet: Uint8Array): boolean =>
    new TextDecoder().decode(packet.subarray(0, 8)) === "#bundle\0";

/** An NTP timetag back to Unix seconds, for the assertions that only bound it. */
const ntpToUnix = (ntp: bigint): number =>
    Number(ntp >> 32n) - 2_208_988_800 + Number(ntp & 0xffffffffn) / 2 ** 32;

/** A clock on manual seams, driven by a caller-supplied timebase. */
function harness(timebase: Timebase & { advance(secs: number): void }, tempo = 1.0) {
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
    return { clock, run };
}

/**
 * A sample timebase whose counter is moved by hand — the in-page engine's
 * clock, stripped to what the tests need.
 */
function manualSampleTimebase(sampleRate = 48000) {
    let sample = 0;
    const timebase = new SampleTimebase(() => sample, sampleRate) as SampleTimebase & {
        advance(secs: number): void;
    };
    timebase.advance = (secs: number) => {
        sample += Math.round(secs * sampleRate);
    };
    return timebase;
}

test("a bundle is stamped at the routine's logical beat, plus the latency", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const timebase = manualSampleTimebase();
    // A monotonic-style timebase: seconds that only move forward.
    const seconds = {
        kind: "monotonic",
        value: 0,
        now() {
            return this.value;
        },
        advance(secs: number) {
            this.value += secs;
        },
    };
    const { clock, run } = harness(seconds, 2.0);
    const routine = new Routine(function* () {
        for (let i = 0; i < 3; i++) {
            server.sendBundle([["/node_set", ["i", 1000], "freq", ["f", 440 + i]]]);
            yield 0.5;
        }
    });
    clock.start().play(routine);
    await run(2);

    assert.equal(connection.packets.length, 3);
    const start = clock.startTime!;
    connection.packets.forEach((packet, i) => {
        // Beat i*0.5 at 2 beats/s is i*0.25 s of music.
        const expected = start + i * 0.25 + server.latency;
        assert.equal(timetagOf(packet), unixToNtp(expected), `bundle ${i}`);
        const [msg] = decodePacket(packet);
        assert.equal(msg!.addr, "/node_set");
        assert.deepEqual(msg!.args, [1000, "freq", 440 + i]);
    });
    server.close();
    void timebase;
});

test("under a sample timebase the emission is /sched_at at an absolute sample", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const timebase = manualSampleTimebase();
    const { clock, run } = harness(timebase, 2.0);
    const routine = new Routine(function* () {
        for (let i = 0; i < 3; i++) {
            server.sendBundle([["/node_set", ["i", 1000], "freq", ["f", 440]]]);
            yield 0.5;
        }
    });
    clock.start().play(routine);
    await run(2);

    assert.equal(connection.packets.length, 3);
    const origin = clock.pacingOrigin!;
    connection.packets.forEach((packet, i) => {
        const [sched] = decodePacket(packet);
        assert.equal(sched!.addr, "/sched_at");
        const expected = secsToSamples(origin + i * 0.25 + server.latency, 48000);
        assert.equal(sched!.args[0], expected, `sched ${i}`);
        // The inner packet is an immediate bundle carrying the messages.
        const inner = decodePacket(sched!.args[1] as Uint8Array);
        assert.equal(inner[0]!.addr, "/node_set");
    });
    server.close();
});

test("a note played in a routine emits its /synth_new and its release, both timed", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const seconds = {
        kind: "monotonic",
        value: 0,
        now() {
            return this.value;
        },
        advance(secs: number) {
            this.value += secs;
        },
    };
    const { clock, run } = harness(seconds, 1.0);
    const routine = new Routine(function* () {
        new Event({ instrument: "sine", degree: 0, dur: 1, legato: 0.5 }).play(server);
        yield 1;
    });
    clock.start().play(routine);
    await run(2);

    assert.equal(connection.packets.length, 2);
    const [start, release] = connection.packets;
    const [sNew] = decodePacket(start!);
    assert.equal(sNew!.addr, "/synth_new");
    assert.equal(sNew!.args[0], "sine");
    const node = Number(sNew!.args[1]);

    const [free] = decodePacket(release!);
    assert.equal(free!.addr, "/node_free", "no gate on a custom def: freed directly");
    assert.equal(free!.args[0], node);

    // The release is exactly `sustain` beats (dur * legato) after the note.
    const at = clock.startTime! + server.latency;
    assert.equal(timetagOf(start!), unixToNtp(at));
    assert.equal(timetagOf(release!), unixToNtp(at + 0.5));
    server.close();
});

test("the built-in default instrument is released by its gate", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const seconds = {
        kind: "monotonic",
        value: 0,
        now() {
            return this.value;
        },
        advance(secs: number) {
            this.value += secs;
        },
    };
    const { clock, run } = harness(seconds, 1.0);
    clock.start().play(
        new Routine(function* () {
            new Event({ degree: 2 }).play(server);
            yield 1;
        }),
    );
    await run(2);
    const [gate] = decodePacket(connection.packets[1]!);
    assert.equal(gate!.addr, "/node_set");
    assert.equal(gate!.args[1], "gate");
    assert.equal(gate!.args[2], 0);
    server.close();
});

test("a note played with no clock sounds now and frees itself on wall time", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const before = Date.now() / 1000;
    const event = new Event({ instrument: "sine", freq: 440, dur: 2, legato: 1 }).play(
        server,
    );

    // One path in or out of a routine: both go out timed, at the clockless
    // moment (now) and now + the sustain read as seconds.
    assert.equal(connection.packets.length, 2);
    assert.equal(isBundle(connection.packets[0]!), true, "the note is timed at now");
    assert.equal(isBundle(connection.packets[1]!), true, "the release is timed");
    const [sNew] = decodePacket(connection.packets[0]!);
    assert.equal(sNew!.addr, "/synth_new");
    assert.equal(event.get("node"), Number(sNew!.args[1]));
    assert.equal(event.get("sustain"), 2);

    const started = ntpToUnix(timetagOf(connection.packets[0]!));
    const freed = ntpToUnix(timetagOf(connection.packets[1]!));
    assert.ok(Math.abs(started - (before + server.latency)) < 0.5, "sounds now");
    assert.ok(Math.abs(freed - started - 2) < 0.01, "frees itself a sustain later");
    server.close();
});

test("an event completes its own keys, and stays actionable afterwards", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const event = new Event({ instrument: "sine", degree: 7, dur: 0.5 }).play(server);
    assert.equal(event.get("midinote"), 72);
    assert.equal(event.get("delta"), 0.5);
    assert.equal(event.get("sustain"), 0.4);

    connection.packets.length = 0;
    event.free();
    const [freed] = decodePacket(connection.packets[0]!);
    assert.equal(freed!.addr, "/node_free");
    assert.equal(freed!.args[0], event.get("node"));
    server.close();
});

test("a rest sounds nothing but keeps its place in time", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const event = new Event({ type: "rest", dur: 0.25 }).play(server);
    assert.equal(connection.packets.length, 0);
    assert.equal(event.get("node"), null);
    assert.equal(event.delta(), 0.25);
    server.close();
});

test("a clock resumed after a stop stamps for now, not for the old axis", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const timebase = manualSampleTimebase();
    const { clock, run } = harness(timebase, 1.0);

    clock.start();
    timebase.advance(2); // two seconds of counter at 1 beat/s -> beat 2
    assert.equal(clock.beats(), 2);
    clock.stop();
    timebase.advance(8); // the clock is stopped; the counter runs on
    assert.equal(clock.beats(), 2, "a stopped clock holds its beat");
    clock.start();
    assert.equal(clock.beats(), 2, "and resumes there");

    // An event at the resumed beat is scheduled for *now* — the origins moved
    // with the beat, so the emission is not eight seconds stale.
    clock.play(
        new Routine(function* () {
            server.sendBundle([["/node_free", ["i", 1000]]]);
            yield 1;
        }),
    );
    await run(0.1); // the first wake lands on the spot, at the resumed beat
    const [sched] = decodePacket(connection.packets[0]!);
    assert.equal(sched!.addr, "/sched_at");
    assert.equal(sched!.args[0], secsToSamples(10 + server.latency, 48000));
    server.close();
});

test("sending a bundle with no clock anywhere is wall-clock now", async () => {
    const connection = recorder();
    const server = await openServer(connection);
    const before = Date.now() / 1000;
    server.sendBundle([["/node_free", ["i", 1000]]]);

    // No clock is not an error: the clockless moment is now, and a delay on
    // it reads as seconds (tempo 1.0).
    const at = ntpToUnix(timetagOf(connection.packets[0]!));
    assert.ok(Math.abs(at - (before + server.latency)) < 0.5);

    server.sendBundle([["/node_free", ["i", 1001]]], { delayBeats: 3 });
    const later = ntpToUnix(timetagOf(connection.packets[1]!));
    assert.ok(Math.abs(later - at - 3) < 0.01);
    server.close();
});
