// The `Server` end to end against a real `clausters --ws` server.
//
// The WS half of W1's acceptance: define a def and play it, with `/server_sync`
// ordering and a queryable result — both families, since a native server is
// the only one that has the Faust JIT (the in-page engine is the
// `synth,embed` build). Nothing here names the carrier: the same `Server`
// runs over `pageConnection()` in `tests/client.html`, which covers the
// in-page half.
//
// Needs the debug server built (`cargo build` at the workspace root) and the
// core wasm staged (`./build.sh`). Skips (does not fail) when the binary is
// missing, so `npm test` stays runnable from a source tree without a build.
//
// Each test spawns its own server, on its own `--port` (the base OSC port,
// UDP and TCP alike) as well as its own `--ws`, so the suites no longer
// contend for a machine-wide 57110. `npm test` still passes
// `--test-concurrency=1`: what remains shared is the audio device, which the
// servers open one stream each into, and the wall-clock cost of spawning
// several at once — not a port. Lifting it is a measurement, not a fix.

import assert from "node:assert/strict";
import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";
import { spawnChild } from "./child.ts";

import { WsConnection } from "../src/base/connection.ts";
import { loadCore } from "../src/base/core.ts";
import { Bus } from "../src/defs/bus.ts";
import { Buffer } from "../src/defs/buffer.ts";
import { Group, Synth } from "../src/defs/node.ts";
import { Server } from "../src/defs/server/index.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { FaustDef } from "../src/defs/faustdef.ts";
import { GraphDef } from "../src/defs/graphdef.ts";
import * as sig from "../src/defs/signals.ts";
import { CommandError } from "../src/errors.ts";
import {
    control, DoneAction, Env, envGen, out, rlpf, saw, sine,
} from "../src/defs/ugens/index.ts";

const here = new URL(".", import.meta.url);
const serverBin = new URL("../../../target/debug/clausters", here).pathname;
const wsPort = 57988; // out of the default range, one per suite
// The server's own base OSC port (`--port`): UDP and TCP alike. Distinct
// per suite, so these servers are independent processes rather than one
// machine-wide singleton.
const udpPort = 57888;

const hasServer = await access(serverBin).then(() => true, () => false);

await loadCore();

/**
 * Boots a server, runs `body` against a `Server` over its WS front, and
 * tears both down — one process per test, which also satisfies the sandbox's
 * per-invocation network isolation.
 */
async function withServer(body: (server: Server) => Promise<void>): Promise<void> {
    const child = spawnChild(serverBin, ["--port", String(udpPort), "--ws", String(wsPort),
        "--no-tcp", "--no-persist"]);
    let connection: WsConnection | null = null;
    let server: Server | null = null;
    try {
        // The server binds the WS listener during boot; retry until it is up.
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(`ws://127.0.0.1:${wsPort}`)
                .catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "server WS endpoint never came up");
        server = await new Server({ connection }).attach();
        await body(server);
    } finally {
        server?.close();
        connection?.close();
        child.stop();
        // The port is reused by the next test; give the OS its release.
        await sleep(50);
    }
}

test("attach sizes its allocators from the running server", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const info = await server.queryInfo();
        assert.equal(server.sizing.audioBuses, info.audioBuses);
        assert.equal(server.sizing.controlBuses, info.controlBuses);
        assert.equal(server.sizing.maxBuffers, info.maxBuffers);

        // The allocators hand out of the space those sizes describe: audio
        // buses start above the hardware outputs, control buses at 0.
        const audio = Bus.audio(2, { server });
        assert.equal(audio.index, info.channels);
        assert.equal(audio.channels, 2);
        assert.equal(Bus.control(1, { server }).index, 0);

        // A freed run is reusable — the registry invariant. Reuse is not
        // *immediate* (the scan hint rotates on, so a freshly released run is
        // not handed straight back), so what it guarantees is that the space
        // does not leak: cycling far past its width keeps succeeding.
        audio.free();
        assert.equal(server.audioBuses.inUse, 0);
        for (let i = 0; i < info.audioBuses; i++) {
            Bus.audio(2, { server }).free();
        }
        assert.equal(server.audioBuses.inUse, 0);
    });
});

test("a SynthDef is defined, played, set and freed", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const freq = control("freq", 440.0);
        const amp = control("amp", 0.1);
        const gate = control("gate", 1.0);
        const def = new SynthDef(
            "ts_beep",
            out(
                0.0,
                sine(freq)
                    .mul(amp)
                    .mul(envGen(Env.asr(0.01, 1.0, 0.1), {
                        gate,
                        doneAction: DoneAction.FREE_SELF,
                    })),
            ),
        );

        // Fire-and-forget plus the barrier: the ordering discipline the
        // asynchronous def path exists for.
        await def.send(server, { wait: false });
        await server.sync();

        const defs = await server.queryDefs(["ts_beep"]);
        assert.equal(defs.length, 1);
        assert.equal(defs[0]!.family, "synth");
        assert.deepEqual(
            defs[0]!.controls.map((c) => c.name).sort(),
            ["amp", "freq", "gate"],
        );

        const synth = new Synth("ts_beep", { freq: 220.0, amp: 0.05 }, { server });
        await server.sync();

        // It is in the tree, under the root, with the controls it was given.
        const info = await synth.info();
        assert.equal(info.isGroup, false);
        assert.equal(info.defname, "ts_beep");
        assert.equal(info.parent, 0);
        assert.equal(info.controls?.freq, 220.0);

        // The tree agrees, and so does the def count in /server_status — the live
        // node/UGen counters are the audio thread's own, published a poll
        // window behind, so the tree is what a just-sent command is read
        // back from.
        const tree = await server.queryTree();
        assert.deepEqual(tree.children?.map((c) => c.id), [synth.id]);
        assert.ok((await server.status()).defs >= 1, "the def is loaded");

        synth.set({ freq: 330.0 });
        await server.sync();
        assert.equal((await synth.info()).controls?.freq, 330.0);

        synth.free();
        await server.sync();

        // The tree is back to the bare root group.
        const empty = await server.queryTree();
        assert.equal(empty.id, 0);
        assert.deepEqual(empty.children, []);
    });
});

test("the example's voice def compiles and its gate releases it", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        // The graph examples/basics/synth.html builds, kept honest here: a resonant
        // filter given its resonance as a Q, under an ADSR whose gate is a
        // plain control (a `tr` one would reset itself and release the note
        // the instant it opened).
        const freq = control("freq", 220.0);
        const cutoff = control("cutoff", 1200.0, { lag: 0.05 });
        const amp = control("amp", 0.2);
        const gate = control("gate", 1.0);
        const voice = rlpf(saw(freq), cutoff, { q: 6.0 })
            .mul(amp)
            .mul(envGen(Env.adsr(0.01, 0.15, 0.6, 0.3), {
                gate,
                doneAction: DoneAction.FREE_SELF,
            }));
        await new SynthDef("ts_voice", out(0.0, voice), out(1.0, voice)).send(server);

        // A Q of 6 reaches the wire as its reciprocal, the rq the server takes.
        const rq = new SynthDef("q", out(0.0, rlpf(saw(110.0), 800.0, { q: 4.0 })))
            .spec()
            .ugens.find((u) => u.kind === "RLPF")!;
        assert.deepEqual(rq.inputs[2], { const: 0.25 });

        const note = new Synth("ts_voice", { freq: 330.0 }, { server });
        await server.sync();
        assert.equal((await note.info()).defname, "ts_voice");

        // Dropping the gate hands the node's life to the envelope.
        note.set({ gate: 0.0 });
        await server.sync();
        for (let i = 0; i < 40; i++) {
            if ((await server.queryTree()).children?.length === 0) break;
            await sleep(25);
        }
        assert.deepEqual(
            (await server.queryTree()).children,
            [],
            "the release freed the node",
        );
    });
});

test("a node id returns to the registry when its /node_end arrives", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        await new SynthDef("ts_quiet", out(0.0, sine(1.0).mul(0))).send(server);
        const first = new Synth("ts_quiet", undefined, { server });
        assert.equal(server.nodes.inUse, 1);
        first.free();
        // Freeing does not release the id: it stays tracked until the server
        // confirms the death with /node_end, which is what the registry listens
        // for. (Releasing at send time could re-hand an id whose node is
        // still alive on the server.)
        await server.sync();
        for (let i = 0; i < 40 && server.nodes.inUse > 0; i++) await sleep(25);
        assert.equal(server.nodes.inUse, 0, "the /node_end recycled the id");
    });
});

test("a FaustDef is JIT-compiled and played", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const freq = sig.hslider("freq", 440.0, 20.0, 2000.0, 0.01);
        const amp = sig.hslider("amp", 0.1, 0.0, 1.0, 0.001);
        const phasor = sig.rec((s) => {
            const next = s.add(freq.div(sig.sr()));
            return next.sub(next.floor());
        });
        const def = FaustDef.fromSignals(
            "ts_tone",
            sig.sin(phasor.mul(2.0 * sig.PI)).mul(amp),
        );
        assert.deepEqual(def.controlNames(), ["freq", "amp"]);

        await def.send(server);

        const defs = await server.queryDefs(["ts_tone"]);
        assert.equal(defs[0]!.family, "faust");

        const synth = new Synth("ts_tone", { freq: 330.0 }, { server });
        await server.sync();
        assert.equal((await synth.info()).defname, "ts_tone");
        synth.free();
    });
});

test("a GraphDef instantiates as a wired group driven through its surface", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const level = control("level", 0.1);
        await new SynthDef("ts_src", out(control("out", 0.0), sine(220.0).mul(level))).send(server);
        await new SynthDef(
                "ts_sink",
                out(control("out", 0.0), sine(1.0).mul(0.0)),
            ).send(server);

        const g = new GraphDef("ts_chain");
        const bus = g.bus("mix");
        const src = g.add("ts_src", { out: bus, level: 0.2 });
        g.add("ts_sink", { in: bus, out: "OUT" });
        g.port("gain", [src.control("level")], 0.5);
        await g.send(server);

        const instance = Group.graph("ts_chain", { gain: 0.3 }, { server });
        await server.sync();

        // The instance is a group holding the members the def named.
        const info = await instance.info();
        assert.equal(info.isGroup, true);
        const tree = await server.queryTree(instance);
        assert.equal(tree.children?.length, 2);

        // The surface is what drives it — the private member ids never appear.
        instance.set({ gain: 0.1 });
        await server.sync();

        instance.free();
        await server.sync();
        assert.deepEqual((await server.queryTree()).children, []);
    });
});

test("a def the server rejects comes back as a CommandError", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        await assert.rejects(
            FaustDef.fromSource("ts_broken", "process = @@@;").send(server),
            CommandError,
        );
        // The failure is per-command: the server is still usable after it.
        await new SynthDef("ts_after", out(0.0, sine(440.0))).send(server);
        assert.equal((await server.queryDefs(["ts_after"]))[0]!.family, "synth");
    });
});

test("the UGen catalog names its inputs and its rates", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const all = await server.queryUgens();
        assert.ok(all.length > 0, "a build with the synth feature has UGens");

        // Asking for kinds by name details exactly those.
        const [sine] = await server.queryUgens(["Sine"]);
        assert.equal(sine!.name, "Sine");
        assert.ok(sine!.rates.includes("ar"));
        assert.equal(sine!.inputs[0]!.name, "freq");
        assert.equal(sine!.inputs.length, sine!.arity);

        // A variadic kind reports -1 and names only its fixed head.
        const [env] = await server.queryUgens(["EnvGen"]);
        assert.equal(env!.arity, -1);
        assert.ok(env!.inputs.length > 0);
    });
});

test("buffers allocate, generate and free through the pool", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const buf = await Buffer.alloc(1024, 1, { server });
        assert.equal(server.buffers.inUse, 1);

        const queried = await buf.info();
        assert.equal(queried.frames, 1024);
        assert.equal(queried.channels, 1);

        // The server's own list of what is allocated, which is where a
        // buffer this client never allocated would show up too.
        const listed = await server.queryBuffers();
        const mine = listed.find((b) => b.bufnum === buf.bufnum);
        assert.ok(mine, "the allocated buffer is in /buffer_query's list");
        assert.equal(mine.frames, 1024);
        assert.equal(mine.channels, 1);

        // `sine1` takes its flag word first (1 = normalize, 2 = wavetable),
        // as an int — the tagging rule sends an integral number as one.
        await buf.gen("sine1", [3, 1.0, 0.5, 0.25]);
        buf.free();
        assert.equal(server.buffers.inUse, 0);
    });
});

test("control buses carry a value both ways", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const bus = Bus.control(1, { server });
        bus.set(0.25);
        assert.equal(await bus.get(), 0.25);
    });
});

test("the shared transport is defined, read, rolled and located", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        // Nothing is defined until a conductor sets the grid -- but the
        // transport itself is there from boot, so the *state* answers with its
        // grid fields null while `transport()` (the grid alone) answers null.
        assert.equal(await server.transport(), null);
        const bare = await server.transportState();
        assert.equal(bare.originSample, null);
        assert.equal(bare.tempo, null);
        assert.equal(bare.playing, false);
        assert.equal(bare.group, null);

        await server.setTransport(0, 2.0);
        const grid = await server.transport();
        assert.deepEqual(grid, { originSample: 0, tempo: 2.0 });

        // Setting the grid leaves it stopped at 0, with no group bound.
        const fresh = (await server.transportState())!;
        assert.equal(fresh.playing, false);
        assert.equal(fresh.position, 0.0);
        assert.equal(fresh.group, null);

        await server.transportLocate(8.0);
        assert.equal((await server.transportState())!.position, 8.0);

        // Play from an explicit position, then stop: the position holds.
        await server.transportPlay(4.0);
        const rolling = (await server.transportState())!;
        assert.equal(rolling.playing, true);
        assert.equal(rolling.position, 4.0);
        await server.transportStop();
        const stopped = (await server.transportState())!;
        assert.equal(stopped.playing, false);
        assert.equal(stopped.position, 4.0);
    });
});

test("a governed group freezes the transport clock", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        const piece = new Group({ server });
        await server.setTransport(0, 2.0);
        await server.transportGroup(piece);
        assert.equal((await server.transportState())!.group, piece.id);

        // Bound and stopped: the transport clock is frozen, so two reads
        // spanning real time report the same sample. The device clock, which
        // never stops, is what `/clock_query` reads.
        await server.transportPlay();
        await sleep(120);
        await server.transportStop();
        const first = (await server.transportState())!.transportSample;
        assert.ok(first > 0, "the transport clock advanced while rolling");
        await sleep(120);
        assert.equal((await server.transportState())!.transportSample, first);

        // A resume moves it again.
        await server.transportPlay();
        await sleep(120);
        assert.ok((await server.transportState())!.transportSample > first);

        // `null` unbinds — and thaws whatever it governed.
        await server.transportGroup(null);
        assert.equal((await server.transportState())!.group, null);
        piece.free();
    });
});

test("/sched_atTransport verifies the axis it is told", { skip: !hasServer }, async () => {
    await withServer(async (server) => {
        await new SynthDef("ts_hold", out(0.0, sine(control("freq", 220.0)).mul(0.0)))
            .send(server);
        const piece = new Group({ server });
        await server.setTransport(0, 2.0);
        await server.transportGroup(piece);

        // A message aimed inside the governed subtree rides the transport
        // axis, which is what the declaration claims.
        const inside = server.nodes.alloc();
        await server.schedAtTransport(0, [
            ["/synth_new", "ts_hold", ["i", inside], ["i", 0], ["i", piece.id]],
        ]);

        // One aimed outside it does not, and the server says so rather than
        // playing the bundle on the wrong clock.
        const outside = server.nodes.alloc();
        await assert.rejects(
            server.schedAtTransport(0, [
                ["/synth_new", "ts_hold", ["i", outside], ["i", 0], ["i", 0]],
            ]),
            CommandError,
        );

        await server.transportGroup(null);
        piece.free();
    });
});

test("a buffer is written to a file and read back into another", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const dir = await mkdtemp(join(tmpdir(), "clausters-ts-"));
        try {
            const source = await Buffer.alloc(1024, 1, { server });
            await source.gen("sine1", [1, 1.0]);
            const path = join(dir, "wave.wav");
            // Float samples, so the round trip is exact rather than quantized
            // to the int16 default.
            await source.write(path, { sampleFormat: "float" });

            // `readInto` keeps the target's shape, where `Buffer.read`
            // allocates one to fit the file.
            const target = await Buffer.alloc(1024, 1, { server });
            await target.readInto(path);
            assert.deepEqual(
                Array.from(await target.getSamples({ count: 64 })),
                Array.from(await source.getSamples({ count: 64 })),
            );

            source.free();
            target.free();
        } finally {
            await rm(dir, { recursive: true, force: true });
        }
    });
});
