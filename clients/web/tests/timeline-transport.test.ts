// A timeline played on a **server transport**: the verbs are the transport's,
// and the plan is stamped on the transport's clock.
//
// The server here is a recorder: the transport commands are collected instead
// of sent, and what the plan writes lands in the score carrier as the
// `/sched_atTransport` messages it really sends. What is checked is the
// stamping (which sample, on which axis), the re-cue rule around a locate, and
// what the mode refuses.
//
// The Python client's `tests/test_timeline_transport.py` is the same suite;
// keep them reading alike.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { ScoreConnection } from "../src/base/connection.ts";
import { Server } from "../src/defs/server/index.ts";
import { Routine } from "../src/base/stream.ts";
import { Event } from "../src/seq/event.ts";
import { Pbind } from "../src/seq/pattern.ts";
import { OscItem, Timeline } from "../src/seq/timeline.ts";

await loadCore();

const SR = 48_000;
const RATE = 2.0; // beats per second
const BASE = 480_000; // the transport clock's sample when a test starts
const LATENCY = 0.1;

/** A server whose transport answers and records instead of rolling. */
class TransportServer extends Server {
    calls: unknown[][] = [];
    state = {
        playing: false,
        positionSample: 0,
        transportSample: BASE,
        group: 7 as number | null,
    };

    constructor({ group = 7 as number | null } = {}) {
        super({ connection: new ScoreConnection() });
        this.latency = LATENCY;
        this.state.group = group;
    }

    override async transportState(): Promise<never> {
        return { ...this.state } as never;
    }

    override async queryInfo(): Promise<never> {
        return { nominalSampleRate: SR } as never;
    }

    override async transportPlay(): Promise<never> {
        this.calls.push(["play"]);
        this.state.playing = true;
        return this as never;
    }

    override async transportStop(): Promise<never> {
        this.calls.push(["stop"]);
        this.state.playing = false;
        return this as never;
    }

    override async transportLocateSample(sample: number): Promise<never> {
        this.calls.push(["locate", Math.trunc(sample)]);
        this.state.positionSample = Math.trunc(sample);
        return this as never;
    }

    override schedClear(axis?: "transport"): this {
        this.calls.push(["clear", axis ?? null]);
        return this;
    }

    override async notify(): Promise<void> {}

    /** The plan's samples, with the base and latency taken off. */
    onsets(): number[] {
        const score = (this.connection as ScoreConnection).score;
        const base = BASE + LATENCY * SR;
        const out: number[] = [];
        for (const packet of packets(score)) {
            const text = new TextDecoder("latin1").decode(packet);
            if (!text.includes("/sched_atTransport")) continue;
            const view = new DataView(packet.buffer, packet.byteOffset, packet.byteLength);
            const tag = text.indexOf(",hb");
            const sample = Number(view.getBigInt64(tag + 4));
            out.push(Math.round(((sample - base) / SR) * 1e6) / 1e6);
        }
        return out.sort((a, b) => a - b);
    }
}

/** Every packet the score holds, framed as the binary score frames them. */
function packets(score: { bytes(): Uint8Array }): Uint8Array[] {
    const bytes = score.bytes();
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const out: Uint8Array[] = [];
    for (let i = 0; i + 4 <= bytes.length;) {
        const len = view.getInt32(i, false);
        out.push(bytes.subarray(i + 4, i + 4 + len));
        i += 4 + len;
    }
    return out;
}

const timeline = () =>
    new Timeline([0, 1, 2, 3].map((b) => [b, new OscItem("/a")] as [number, unknown]), {
        tempo: RATE,
    });

test("a transport needs a governed group", async () => {
    const server = new TransportServer({ group: null });
    const tl = timeline();
    // The check is the setter's, and it asks the server — so the throw arrives
    // through the queued work, which `refresh` settles with.
    await assert.rejects(async () => {
        tl.transport = server;
        await tl.refresh();
    }, /transportGroup/);
});

test("play locates, rolls and plans on the transport's clock", async () => {
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.play({ at: 0, destination: server });
    await tl.refresh();

    assert.deepEqual(server.calls, [["clear", "transport"], ["locate", 0], ["play"]]);
    // Four items, a beat apart at two beats a second, stamped `latency` ahead
    // of the transport's clock so nothing regenerated is late.
    assert.deepEqual(server.onsets(), [0, 0.5, 1, 1.5]);
    assert.ok(tl.playing);
});

test("the offset is where beat 0 falls on the transport", async () => {
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.transportAt = 4.0; // seconds of the transport's position
    tl.play({ at: 1.0, destination: server });
    await tl.refresh();

    assert.ok(server.calls.some(([verb, sample]) =>
        verb === "locate" && sample === (4.0 + 0.5) * SR));
    assert.deepEqual(server.onsets(), [0, 0.5, 1]);
});

test("a locate clears the transport queue and re-plans", async () => {
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.play({ at: 0, destination: server });
    await tl.refresh();
    server.calls = [];
    (server.connection as ScoreConnection).score.clear();

    tl.locate(2.0);
    await tl.refresh();
    assert.deepEqual(server.calls, [["clear", "transport"], ["locate", 1.0 * SR]]);
    // From beat 2: the two items left, the first of them now.
    assert.deepEqual(server.onsets(), [0, 0.5]);
    assert.ok(Math.abs(tl.position() - 2.0) < 1e-9);
});

test("a resume re-plans nothing", async () => {
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.play({ at: 0, destination: server });
    tl.pause();
    await tl.refresh();
    server.calls = [];
    (server.connection as ScoreConnection).score.clear();

    tl.play(); // no `at`: the frozen queue carries on
    await tl.refresh();
    assert.deepEqual(server.calls, [["play"]]);
    assert.deepEqual(server.onsets(), []);
});

test("stop halts and goes back to the mark", async () => {
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.play({ at: 1.0, destination: server });
    await tl.refresh();
    server.calls = [];

    tl.stop();
    await tl.refresh();
    assert.deepEqual(server.calls[0], ["stop"]);
    assert.ok(server.calls.some(([verb, sample]) => verb === "locate" && sample === 0.5 * SR));
    assert.ok(Math.abs(tl.position() - 1.0) < 1e-9);
});

test("what the mode refuses", async () => {
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;

    assert.throws(() => tl.play({ at: 0, quant: 4, destination: server }), /quant/);
    assert.throws(() => tl.loop(0, 2), /loop/);

    // Forward-only items cannot be planned from a position.
    const forward = new Timeline([[0, new Routine(function* () { yield 1; })]], { tempo: RATE });
    forward.transport = server;
    forward.play({ at: 0, destination: server });
    await assert.rejects(() => forward.refresh(), /Routine/);

    const generated = new Timeline([[0, new Pbind({ degree: 0 })]], { tempo: RATE });
    generated.transport = server;
    generated.play({ at: 0, destination: server });
    await assert.rejects(() => generated.refresh(), /Pbind/);
});

test("a child is planned in its own units", async () => {
    const server = new TransportServer();
    const child = new Timeline([[0, new OscItem("/b")], [1, new OscItem("/b")]], { tempo: 4.0 });
    const parent = new Timeline([[0, new OscItem("/a")]], { tempo: RATE });
    parent.add(1, child);
    parent.transport = server;
    parent.play({ at: 0, destination: server });
    await parent.refresh();

    // The parent's beat 1 is half a second in; the child's beat 1 a quarter
    // after that, at its own four beats a second.
    assert.deepEqual(server.onsets(), [0, 0.5, 0.75]);
});

test("leaving the transport goes back to its own clock", async () => {
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.play({ at: 0, destination: server });
    await tl.refresh();
    tl.transport = null;
    assert.equal(tl.transport, null);
    assert.ok(!tl.playing);
});

test("an event's sustain is still in the timeline's beats", async () => {
    const server = new TransportServer();
    const tl = new Timeline(
        [[0, new Event({ instrument: "default", dur: 1.0, legato: 1.0, target: 7 })]],
        { tempo: RATE },
    );
    tl.transport = server;
    tl.play({ at: 0, destination: server });
    await tl.refresh();
    // One beat of sustain at two beats a second: the release is half a second
    // after the onset, on the transport's clock.
    assert.deepEqual(server.onsets(), [0, 0.5]);
});

test("a conductor's locate re-plans from where it says", async () => {
    // The mode **is** the following: a broadcast is where a locate somebody
    // else sent arrives, and the plan is written again from there. Fed straight
    // to the handler the receiver would call, so no carrier is involved.
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.play({ at: 0, destination: server });
    await tl.refresh();
    (server.connection as ScoreConnection).score.clear();
    server.calls = [];

    // A conductor's locate: the engine moved, so the server says so too.
    server.state.positionSample = 1.5 * SR;
    const reply = ["/transport_query.reply", 0, 2.0, 1, 1, 0.0, 7, BASE, 1.5 * SR, -1, -1];
    (tl.player as unknown as { broadcast(msg: unknown[]): void }).broadcast(reply);
    await tl.refresh();

    assert.ok(Math.abs(tl.position() - 3.0) < 1e-9); // beat 3 at two beats a second
    assert.deepEqual(server.onsets(), [0]); // the last item, re-planned
    assert.ok(server.calls.some(([verb, axis]) => verb === "clear" && axis === "transport"),
        "the re-cue clears first");
});

test("a re-cue that fails is kept for refresh, not thrown at the console", async () => {
    // A verb is queued by a caller who will `refresh`; a **re-cue** is queued
    // by a broadcast, which has no caller at all. So a re-cue that fails --
    // every request in flight rejecting because the server is closing, which
    // is what ends a page or a test -- had nobody to reject to, and surfaced
    // as an unhandled rejection in whatever was running at the time.
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.play({ at: 0, destination: server });
    await tl.refresh();
    // From here the server is the one a close leaves behind: every command it
    // is handed fails.
    server.schedClear = () => { throw new Error("the server was closed"); };

    const unhandled: unknown[] = [];
    const watch = (reason: unknown) => unhandled.push(reason);
    process.on("unhandledRejection", watch);
    try {
        server.state.positionSample = 1.5 * SR;
        const reply = ["/transport_query.reply", 0, 2.0, 1, 1, 0.0, 7, BASE, 1.5 * SR, -1, -1];
        (tl.player as unknown as { broadcast(msg: unknown[]): void }).broadcast(reply);
        // A rejection with no handler is reported at the end of the turn, so
        // give the loop one before reading the count.
        await new Promise((done) => setImmediate(done));
        assert.deepEqual(unhandled, [], "a queued re-cue rejected with nobody watching");
    } finally {
        process.off("unhandledRejection", watch);
    }

    // And it is not swallowed: the failure is still the chain's, so the next
    // refresh is where it reaches the caller.
    await assert.rejects(() => tl.refresh(), /the server was closed/);
});

test("with no destination the items go to the transport's server", async () => {
    // A plan a broadcast writes runs where no session is ambient: the items go
    // to the server whose transport this is.
    const server = new TransportServer();
    const tl = timeline();
    tl.transport = server;
    tl.play({ at: 0 }); // no destination named
    await tl.refresh();
    assert.deepEqual(server.onsets(), [0, 0.5, 1, 1.5]);
});
