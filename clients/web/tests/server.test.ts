// The `Server` end to end against a real `clausters --ws` server.
//
// The WS half of W1's acceptance: define a def and play it, with `/sync`
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
// Each test spawns its own server, and `--ws <port>` only moves the
// WebSocket front: the OSC port (57110) is fixed, so two servers cannot run
// at once. That is why `npm test` passes `--test-concurrency=1` — node would
// otherwise run the suites that spawn servers in parallel, and the second
// server would find the port taken.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

import { WsConnection } from "../src/base/connection.ts";
import { loadOsc } from "../src/base/osc.ts";
import { Bus } from "../src/defs/bus.ts";
import { Buffer } from "../src/defs/buffer.ts";
import { Group, Synth } from "../src/defs/node.ts";
import { Server } from "../src/defs/server.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { FaustDef } from "../src/defs/faustdef.ts";
import { GraphDef } from "../src/defs/graphdef.ts";
import * as sig from "../src/defs/signals.ts";
import { CommandError } from "../src/errors.ts";
import {
    control, DoneAction, Env, envGen, out, rlpf, saw, sine,
} from "../src/defs/ugens.ts";

const here = new URL(".", import.meta.url);
const serverBin = new URL("../../../target/debug/clausters", here).pathname;
const wsPort = 57988; // out of the default range; parallel-test friendly

const hasServer = await access(serverBin).then(() => true, () => false);

await loadOsc(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)),
);

/**
 * Boots a server, runs `body` against a `Server` over its WS front, and
 * tears both down — one process per test, which also satisfies the sandbox's
 * per-invocation network isolation.
 */
async function withServer(body: (server: Server) => Promise<void>): Promise<void> {
    const process = spawn(
        serverBin,
        ["--ws", String(wsPort), "--no-tcp", "--no-persist"],
        { stdio: "ignore" },
    );
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
        server = await Server.open(connection);
        await body(server);
    } finally {
        server?.close();
        connection?.close();
        process.kill();
        // The port is reused by the next test; give the OS its release.
        await sleep(50);
    }
}

test("Server.open sizes its allocators from the running server", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const info = await server.queryInfo();
        assert.equal(server.sizing.audioBuses, info.audioBuses);
        assert.equal(server.sizing.controlBuses, info.controlBuses);
        assert.equal(server.sizing.maxBuffers, info.maxBuffers);

        // The allocators hand out of the space those sizes describe: audio
        // buses start above the hardware outputs, control buses at 0.
        const audio = Bus.audio(server, 2);
        assert.equal(audio.index, info.channels);
        assert.equal(audio.channels, 2);
        assert.equal(Bus.control(server).index, 0);

        // A freed run is reusable — the registry invariant. Reuse is not
        // *immediate* (the scan hint rotates on, so a freshly released run is
        // not handed straight back), so what it guarantees is that the space
        // does not leak: cycling far past its width keeps succeeding.
        audio.free();
        assert.equal(server.audioBuses.inUse, 0);
        for (let i = 0; i < info.audioBuses; i++) {
            Bus.audio(server, 2).free();
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

        const synth = Synth.new(server, "ts_beep", { freq: 220.0, amp: 0.05 });
        await server.sync();

        // It is in the tree, under the root, with the controls it was given.
        const info = await synth.info();
        assert.equal(info.isGroup, false);
        assert.equal(info.defname, "ts_beep");
        assert.equal(info.parent, 0);
        assert.equal(info.controls?.freq, 220.0);

        // The tree agrees, and so does the def count in /status — the live
        // node/UGen counters are the audio thread's own, published a poll
        // window behind, so the tree is what a just-sent command is read
        // back from.
        const tree = await server.queryTree();
        assert.deepEqual(tree.children?.map((c) => c.id), [synth.id]);
        assert.ok(Number((await server.status())[4]) >= 1, "the def is loaded");

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
        // The graph examples/synth.html builds, kept honest here: a resonant
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

        const note = Synth.new(server, "ts_voice", { freq: 330.0 });
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

test("a node id returns to the registry when its /n_end arrives", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        await new SynthDef("ts_quiet", out(0.0, sine(1.0).mul(0))).send(server);
        const first = Synth.new(server, "ts_quiet");
        assert.equal(server.nodes.inUse, 1);
        first.free();
        // Freeing does not release the id: it stays tracked until the server
        // confirms the death with /n_end, which is what the registry listens
        // for. (Releasing at send time could re-hand an id whose node is
        // still alive on the server.)
        await server.sync();
        for (let i = 0; i < 40 && server.nodes.inUse > 0; i++) await sleep(25);
        assert.equal(server.nodes.inUse, 0, "the /n_end recycled the id");
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

        const synth = Synth.new(server, "ts_tone", { freq: 330.0 });
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

        const instance = Group.graph(server, "ts_chain", { gain: 0.3 });
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
        const buf = await Buffer.alloc(server, 1024, 1);
        assert.equal(server.buffers.inUse, 1);

        const queried = await buf.info();
        assert.equal(queried.frames, 1024);
        assert.equal(queried.channels, 1);

        // The server's own list of what is allocated, which is where a
        // buffer this client never allocated would show up too.
        const listed = await server.queryBuffers();
        const mine = listed.find((b) => b.bufnum === buf.bufnum);
        assert.ok(mine, "the allocated buffer is in /b_query's list");
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
        const bus = Bus.control(server);
        bus.set(0.25);
        assert.equal(await bus.get(), 0.25);
    });
});
