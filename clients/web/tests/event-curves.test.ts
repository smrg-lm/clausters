// An event carries its curves, and a channel's curve is a playable (mirrors
// `clients/python/tests/test_event_curves.py`).
//
// Nothing is built on the server for either: the note is the plain synth it
// always was, and the controls its curves drive are set, one value a block, in
// timed bundles. What is checked here is the bundles -- read off an offline
// score, where every one is kept with its time -- and that the same curves
// survive `renderEvents`.

import assert from "node:assert/strict";
import test, { afterEach } from "node:test";

import { loadCore } from "../src/base/core.ts";
import { main } from "../src/base/main.ts";
import { decodePacket } from "../src/base/osc.ts";
import { Routine } from "../src/base/stream.ts";
import type { Connection, OscNrtInterface } from "../src/base/connection.ts";
import { OscNrtInterface as NrtInterface } from "../src/base/connection.ts";
import { Server } from "../src/defs/server/index.ts";
import { Automation } from "../src/multitrack.ts";
import { WINDOW } from "../src/seq/curves.ts";
import { Event } from "../src/seq/event.ts";
import { Timeline } from "../src/seq/timeline.ts";
import { Session } from "../src/session.ts";

await loadCore();

afterEach(() => {
    main.currentSession = null;
});

/** Seconds between two values of a curve at the score's rate. */
const STEP = 64 / 48_000;

type Message = [string, (number | string)[]];
type Score = [number, Message[]][];

/** A score's bundles as `[seconds, [[addr, args], ...]]`, in time order. */
function read(server: Server): Score {
    const score = (server.connection as OscNrtInterface).score as unknown as {
        bundles: { at: number; packet: Uint8Array }[];
    };
    return [...score.bundles]
        .sort((a, b) => a.at - b.at)
        .map(({ at, packet }) => [
            at,
            decodePacket(packet).map((m) => [m.addr, m.args as (number | string)[]] as Message),
        ]);
}

/** Renders `tune` (a generator function) on an offline session and answers its score. */
async function played(tune: () => Generator<number>, before?: (s: Session) => void): Promise<Score> {
    const session = (await Session.nrt()).activate();
    before?.(session);
    new Routine(tune).play(session.clock);
    session.clock.render();
    return read(session.server);
}

/** Every `[seconds, value]` a `/node_set` wrote on `control`. */
function sets(score: Score, control: string, node?: number): [number, number][] {
    const out: [number, number][] = [];
    for (const [t, messages] of score) {
        for (const [addr, args] of messages) {
            if (addr !== "/node_set" || (node !== undefined && args[0] !== node)) continue;
            for (let i = 1; i + 1 < args.length; i += 2) {
                if (args[i] === control) out.push([t, Number(args[i + 1])]);
            }
        }
    }
    return out;
}

const near = (a: number, b: number, eps = 1e-6) => Math.abs(a - b) <= eps;

test("an event's curve sets its control from its start to its end", async () => {
    const ramp = new Automation({ target: { control: "amp" }, points: [[0.0, 0.0], [0.25, 1.0]] });
    const score = await played(function* () {
        new Event({ freq: 440, amp: 0.2, dur: 1.0, legato: 0.5, automation: [ramp] }).play();
        yield 1.0;
    });
    const start = score[0][1];
    assert.deepEqual(start.map(([addr]) => addr), ["/synth_new", "/node_set"]);
    assert.deepEqual(start[1][1], [1000, "amp", 0.0], "the curve's first value, as it is made");
    const values = sets(score, "amp");
    // One value a block, on the grid, and none past the curve's own end.
    values.slice(1, 4).forEach(([t], i) => assert.ok(near(t, (i + 1) * STEP)));
    assert.ok(near(values[1][1], STEP / 0.25, 1e-5));
    assert.equal(values.at(-1)![1], 1.0);
    assert.ok(values.at(-1)![0] < 0.25 + 2 * STEP);
    // The other controls are the plain values they always were.
    assert.equal(sets(score, "freq").length + sets(score, "pan").length, 0);
});

test("a curve reaches a note to its off and no further", async () => {
    // A `/node_set` to a node that is gone fails an offline render, and the
    // off is the last instant a client knows the node is there.
    const long = new Automation({ target: { control: "amp" }, points: [[0.0, 0.0], [4.0, 1.0]] });
    const score = await played(function* () {
        new Event({ instrument: "beep", freq: 440, dur: 1.0, legato: 0.25, automation: [long] })
            .play();
        yield 1.0;
    });
    assert.deepEqual(score.at(-1)![1], [["/node_free", [1000]]]);
    assert.ok(near(score.at(-1)![0], 0.25));
    assert.ok(Math.max(...sets(score, "amp").map(([t]) => t)) < 0.25);
});

test("a beat of a curve is a beat of the clock", async () => {
    // A curve's points are in beats from the note's start: at two beats a
    // second it is over in half the seconds.
    const ramp = new Automation({ target: { control: "amp" }, points: [[0.0, 0.0], [1.0, 1.0]] });
    const score = await played(function* () {
        new Event({ freq: 440, dur: 4.0, automation: [ramp] }).play();
        yield 4.0;
    }, (session) => session.clock.setTempo(2.0));
    const values = sets(score, "amp");
    assert.equal(values.at(-1)![1], 1.0);
    assert.ok(near(values.at(-1)![0], 0.5, 2 * STEP));
});

test("a bend multiplies the frequency the note started with", async () => {
    const octave = new Automation({ target: { bend: true }, points: [[0.0, 12.0], [0.1, 0.0]] });
    const score = await played(function* () {
        new Event({ freq: 220, dur: 1.0, automation: [octave] }).play();
        yield 1.0;
    });
    const freqs = sets(score, "freq");
    assert.equal(freqs[0][0], 0.0);
    assert.ok(near(freqs[0][1], 440.0, 1e-3));
    assert.ok(near(freqs.at(-1)![1], 220.0, 1e-3));
});

test("a channel's curve reaches the notes of its channel", async () => {
    const level = new Automation({
        target: { control: "amp", channel: 1 }, points: [[0.0, 0.5], [0.2, 1.0]],
    });
    const own = new Automation({ target: { control: "amp" }, points: [[0.0, 0.25], [0.2, 0.25]] });
    const score = await played(function* () {
        level.play();
        new Event({ freq: 220, dur: 1.0, legato: 0.1, channel: 1 }).play(); // 1000
        new Event({ freq: 330, dur: 1.0, legato: 0.1, channel: 0 }).play(); // 1001
        new Event({ freq: 440, dur: 1.0, legato: 0.1, channel: 1, automation: [own] }).play(); // 1002
        yield 0.05;
        new Event({ freq: 550, dur: 1.0, legato: 0.1, channel: 1 }).play(); // 1003, later
        yield 1.0;
    });
    const onIt = sets(score, "amp", 1000);
    assert.equal(onIt[0][0], 0.0);
    assert.ok(near(onIt[0][1], 0.5), "the channel's value, as it is made");
    assert.ok(onIt.length > 50 && Math.max(...onIt.map(([t]) => t)) < 0.1, "to its off");
    assert.equal(sets(score, "amp", 1001).length, 0, "another channel");
    assert.deepEqual([...new Set(sets(score, "amp", 1002).map(([, v]) => v))], [0.25],
        "its own curve wins");
    const late = sets(score, "amp", 1003);
    assert.ok(near(late[0][0], 0.05));
    assert.ok(near(late[0][1], 0.5 + 0.5 * 0.05 / 0.2, 0.02),
        "where the channel's curve stands when the note starts");
});

test("a stopped channel curve sets nothing more", async () => {
    const level = new Automation({ target: { control: "amp" }, points: [[0.0, 0.0], [1.0, 1.0]] });
    const score = await played(function* () {
        level.play();
        new Event({ freq: 220, dur: 1.0, legato: 0.5 }).play();
        yield 0.1;
        level.stop();
        yield 1.0;
    });
    assert.ok(Math.max(...sets(score, "amp").map(([t]) => t)) <= 0.1 + 2 * WINDOW + STEP);
});

test("an event with no curve is the two bundles it always was", async () => {
    const score = await played(function* () {
        new Event({ freq: 440, dur: 1.0 }).play();
        yield 1.0;
    });
    assert.deepEqual(score.map(([, messages]) => messages.map(([addr]) => addr)),
        [["/synth_new"], ["/node_set"]]);
});

test("renderEvents keeps an event's curves and a channel's", async () => {
    const own = new Automation({
        target: { control: "cutoff" }, points: [[0.0, 200.0], [0.5, 2000.0]],
    });
    const level = new Automation({
        target: { control: "amp", channel: 0 }, points: [[0.0, 0.0], [2.0, 1.0]],
    });
    const timeline = new Timeline();
    timeline.add(0.0, level);
    timeline.add(1.0, new Event({ freq: 440, dur: 1.0, automation: [own] }));

    const data = (await timeline.renderEvents()).data() as {
        events: { automation: { target: unknown; points: { at: number; value: number }[] }[];
            data: Record<string, unknown> }[];
        automation: { target: unknown; points: { at: number }[] }[];
    };
    const [event] = data.events;
    const [kept] = event.automation;
    assert.deepEqual(kept.target, { control: "cutoff" });
    assert.deepEqual(kept.points.map((p) => [p.at, p.value]), [[0.0, 200.0], [0.5, 2000.0]]);
    assert.equal("automation" in event.data, false);
    const [channel] = data.automation;
    assert.deepEqual(channel.target, { control: "amp", channel: 0 });
    assert.deepEqual(channel.points.map((p) => p.at), [0.0, 2.0]);
});

test("a note played outside any clock writes its curve into a score", () => {
    // Offline there is no wall to wake on: the score takes every value as the
    // note is played, each at its own second.
    const server = new Server({ connection: new NrtInterface() });
    const ramp = new Automation({ target: { control: "amp" }, points: [[0.0, 0.0], [0.1, 1.0]] });
    new Event({ freq: 440, dur: 1.0, legato: 0.5, automation: [ramp] }).play(server);
    const values = sets(read(server), "amp");
    assert.deepEqual(values[0], [0.0, 0.0]);
    assert.equal(values.at(-1)![1], 1.0);
    assert.ok(near(values[1][0], STEP) && near(values[2][0], 2 * STEP));
    assert.ok(near(values.at(-1)![0], 0.1, 2 * STEP));
});

test("a note played outside any clock is followed on the wall", async () => {
    // With no clock to wake on, the emitter wakes itself: it sends a little
    // ahead of now and stops at the note's off.
    const sent: Uint8Array[] = [];
    const connection: Connection = {
        timeMode: "unix",
        send: (packet) => void sent.push(packet),
        addReply: () => {},
        removeReply: () => {},
        close: () => {},
    };
    const server = new Server({ connection });
    const ramp = new Automation({ target: { control: "amp" }, points: [[0.0, 0.0], [1.0, 1.0]] });
    new Event({ freq: 440, dur: 0.3, legato: 1.0, automation: [ramp] }).play(server);
    const amps = () => sent.flatMap((packet) => decodePacket(packet))
        .filter((m) => m.addr === "/node_set" && m.args[1] === "amp");
    await new Promise((done) => setTimeout(done, 50));
    const early = amps().length;
    assert.ok(early > 0 && early < (0.05 + 3 * WINDOW) / STEP, "a little ahead, not the whole curve");
    await new Promise((done) => setTimeout(done, 450));
    const total = amps().length;
    assert.ok(total > 0.2 / STEP && total <= 0.3 / STEP + 1, "to its off, and no further");
    assert.equal(server.curves.empty, true, "nothing left to follow");
});

test("a curve plays only on a destination that sets controls", () => {
    assert.throws(
        () => new Automation({ target: { control: "amp" }, points: [[0.0, 0.0]] }).play({}),
        /a curve plays on a server/,
    );
});
