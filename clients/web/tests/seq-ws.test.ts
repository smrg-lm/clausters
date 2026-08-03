// The sequencing layer end to end against a real `clausters --ws` server.
//
// The WS half of W3's acceptance: a routine schedules events that reach a real
// server and free themselves, under **both** timebases — the monotonic one
// (wall-clock timetags) and the sample one anchored on the server's `/clock_query`.
// What the client puts on the wire is asserted byte for byte in
// `timed-send.test.ts`; what is asserted here is that a real server accepts it
// and the notes start and end where the pattern says they should.
//
// The observation is the server's **notifications** (`/node_start`, `/node_end`), not
// `/group_queryTree`: the tree reply comes from the network-side mirror, which
// applies each message as it is translated, and a note's `/synth_new` and its
// release are sent in the same instant (only their timetags differ). So the
// mirror shows a scheduled note born and freed at once while the engine still
// has it sounding — a property of the mirror, not of the schedule.
//
// Needs the debug server built (`cargo build` at the workspace root) and the
// core wasm staged (`./build.sh`). Skips (does not fail) when the binary is
// missing, so `npm test` stays runnable from a source tree without a build.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

import { WsConnection } from "../src/base/connection.ts";
import { loadOsc } from "../src/base/osc.ts";
import { Server } from "../src/defs/server/index.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { control, out, outCtl, sine } from "../src/defs/ugens/index.ts";
import { TempoClock } from "../src/base/clock.ts";
import { SampleTimebase } from "../src/base/timebase.ts";
import { Pbind, Pseq } from "../src/seq/index.ts";
import { Automation } from "../src/seq/automation.ts";
import { Bus } from "../src/defs/bus.ts";
import { Synth } from "../src/defs/node.ts";

const here = new URL(".", import.meta.url);
const serverBin = new URL("../../../target/debug/clausters", here).pathname;
const wsPort = 57989; // its own port: the suites run one server at a time

const hasServer = await access(serverBin).then(() => true, () => false);

await loadOsc(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)),
);

async function withServer(body: (server: Server) => Promise<void>): Promise<void> {
    const process = spawn(
        serverBin,
        ["--ws", String(wsPort), "--no-tcp", "--no-persist"],
        { stdio: "ignore" },
    );
    let connection: WsConnection | null = null;
    let server: Server | null = null;
    try {
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(`ws://127.0.0.1:${wsPort}`)
                .catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "server WS endpoint never came up");
        server = await Server.open(connection);
        await body(server);
    } finally {
        server?.close();
        connection?.close();
        process.kill();
        await sleep(50);
    }
}

/**
 * A plain sine with no envelope: the event's own release frees it, so a
 * `/node_end` is the note ending exactly when the schedule said it would.
 */
const beep = () =>
    new SynthDef("ts_seq_beep", out(0.0, sine(control("freq", 440.0)).mul(0.05)));

/**
 * Waits until the server's engine is actually rendering: at boot the sample
 * counter sits at 0 until the audio stream delivers its first block, and a
 * bundle scheduled before that fires late through no fault of the client.
 */
async function awaitEngine(server: Server): Promise<void> {
    const clockNow = async () =>
        Number(
            (await server.request("/clock_query", [], { expect: ["/clock_query.reply"] })).args[0],
        );
    const first = await clockNow();
    for (let i = 0; i < 100; i++) {
        await sleep(20);
        if ((await clockNow()) > first) return;
    }
    throw new Error("the server's engine never started rendering");
}

/**
 * Records when each node started and ended, in wall-clock seconds. The
 * origin is the clock's own start time, so the arrivals are checked against
 * an absolute reference rather than against each other.
 */
function noteLog(server: Server) {
    const started: number[] = [];
    const ended: number[] = [];
    server.onReply((msg) => {
        const now = Date.now() / 1000;
        if (msg.addr === "/node_start") started.push(now);
        else if (msg.addr === "/node_end") ended.push(now);
    });
    return { started, ended };
}

/**
 * Notifications are emitted on a block boundary and cross a socket, so the
 * tolerance is generous — this is a liveness-and-order assertion, not the
 * sample-exactness one (that is `timed-send.test.ts`'s, on the bytes).
 */
function assertNear(
    got: number[],
    origin: number,
    want: number[],
    what: string,
    tol = 0.15,
): void {
    assert.equal(got.length, want.length, `${what}: ${got.length} of ${want.length}`);
    got.forEach((value, i) => {
        const at = value - origin;
        assert.ok(
            Math.abs(at - want[i]!) <= tol,
            `${what}[${i}] = ${at.toFixed(3)}s after the clock started, ` +
                `expected ~${want[i]}s`,
        );
    });
}

test("a pattern played over WebSocket starts and ends its notes on time", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        await beep().send(server);
        await awaitEngine(server);
        const log = noteLog(server);

        const clock = new TempoClock(1.0);
        clock.start();
        new Pbind({
            instrument: "ts_seq_beep",
            degree: new Pseq([0, 2, 4, 7]),
            dur: 0.25, // 250 ms apart
            legato: 4.0, // sustain 1 beat: the four overlap
        }).play(server, { clock });

        await sleep(2500);
        // Each note lands at its beat plus the emission headroom.
        const t0 = clock.startTime! + server.latency;
        assertNear(log.started, t0, [0, 0.25, 0.5, 0.75], "note starts");
        assertNear(log.ended, t0, [1.0, 1.25, 1.5, 1.75], "note ends");
        clock.close();
    });
});

test("the sample timebase anchors on the server's own clock", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        await beep().send(server);
        await awaitEngine(server);
        const timebase = await server.sampleTimebase({ trackEvery: 0 });
        assert.ok(
            timebase instanceof SampleTimebase,
            "a reachable server yields a sample timebase",
        );
        const rate = (timebase as SampleTimebase).sampleRate;
        assert.ok(rate > 8000 && rate < 400000, `implausible rate ${rate}`);

        // The counter advances about as fast as real time (the model is a
        // regression over `/clock_query` anchors, so this is loose on purpose).
        const first = (timebase as SampleTimebase).currentSample();
        await sleep(300);
        const advanced = (timebase as SampleTimebase).currentSample() - first;
        assert.ok(
            advanced > 0.2 * rate && advanced < 0.5 * rate,
            `the counter advanced ${advanced} samples in 300 ms`,
        );

        // A routine on that timebase schedules through `/sched_at` at absolute
        // samples: the notes land where the pattern says, as under wall time.
        const log = noteLog(server);
        const clock = new TempoClock(1.0, { timebase });
        clock.start();
        new Pbind({
            instrument: "ts_seq_beep",
            degree: new Pseq([0, 4]),
            dur: 0.5,
            legato: 2.0, // sustain 1 beat
        }).play(server, { clock });

        await sleep(2500);
        const t0 = clock.startTime! + server.latency;
        assertNear(log.started, t0, [0, 0.5], "note starts");
        assertNear(log.ended, t0, [1.0, 1.5], "note ends");
        clock.close();
    });
});

test("an automation curve drives a control through a bus", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        // A synth whose `cutoff` the curve will drive. It writes that control
        // straight to a control bus, so the value the lane produced is
        // readable rather than inferred.
        const cutoff = control("cutoff", 200.0);
        const probe = control("probe", 0.0, { rate: "ir" });
        await new SynthDef("ts_auto_target", outCtl(probe, cutoff)).send(server);

        const readback = Bus.control(1, { server });
        const target = new Synth("ts_auto_target", { probe: readback.index }, {
            server,
        });
        await sleep(150);   // one control block, so the synth has written once
        assert.equal(await readback.get(), 200.0, "the control starts at its default");

        // Two seconds of curve, in the bpf widget's own flat form: 200 Hz to
        // 4000 Hz, linearly.
        const auto = Automation.fromPoints(
            [0.0, 200.0, 1, 0.0, 2.0, 4000.0, 1, 0.0],
            [target, "cutoff"],
        );
        await auto.prepare(server);
        assert.ok(auto.buf, "prepare allocated the control buffer");
        assert.ok(auto.bus, "prepare allocated the control bus");
        assert.equal(auto.duration(), 2.0);

        // No clock in context: the lane starts now and the curve's beats read
        // as seconds, so a third of the way in is a third of the sweep.
        auto.play(server);
        await sleep(700);
        const mid = Number(await readback.get());
        assert.ok(
            mid > 800 && mid < 2600,
            `mid-sweep value ${mid} is not between the ends`,
        );

        // Interrupting frees the lane synth, so the mapped control holds the
        // last value the curve reached rather than continuing.
        auto.stop();
        await sleep(300);
        const held = Number(await readback.get());
        assert.ok(
            Math.abs(held - mid) < 900,
            `the control kept sweeping after stop: ${mid} -> ${held}`,
        );

        auto.free();
        assert.equal(auto.buf, null);
        target.free();
        readback.free();
    });
});
