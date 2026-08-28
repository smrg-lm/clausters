// The responders end to end against a real `clausters --ws` server.
//
// The WS half of the acceptance: `OscFunc` handlers fire on the server's own
// notifications — `/node_start` and `/node_end` as a synth comes and goes,
// `/done` as an asynchronous command completes, `/node_trigger` from a def's
// `SendTrig`, and a custom address from a `SendReply` — and stop firing once
// freed. The in-page half of the same acceptance is `tests/responders.html`;
// nothing here names a carrier beyond opening one.
//
// Needs the debug server built (`cargo build` at the workspace root) and the
// core wasm staged (`./build.sh`). Skips (does not fail) when the binary is
// missing, so `npm test` stays runnable from a source tree without a build.

import assert from "node:assert/strict";
import { access } from "node:fs/promises";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";
import { spawnChild } from "./child.ts";

import { WsConnection } from "../src/base/connection.ts";
import { loadCore } from "../src/base/core.ts";
import { Server } from "../src/defs/server/index.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import { Synth } from "../src/defs/node.ts";
import { OscFunc } from "../src/responders.ts";
import type { ResponderMessage } from "../src/responders.ts";
import { impulse, out, sendReply, sendTrig, sine } from "../src/defs/ugens/index.ts";

const here = new URL(".", import.meta.url);
const serverBin = new URL("../../../target/debug/clausters", here).pathname;
const wsPort = 57992; // out of the default range, one per suite
// The server's own base OSC port (`--port`): UDP and TCP alike. Distinct
// per suite, so these servers are independent processes rather than one
// machine-wide singleton.
const udpPort = 57892;

const hasServer = await access(serverBin).then(() => true, () => false);

await loadCore();

/**
 * Waits until the engine is actually rendering: node notifications are emitted
 * on a block boundary, so a synth created before the first block would come
 * and go with nothing to report it.
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

async function withServer(body: (server: Server) => Promise<void>): Promise<void> {
    const child = spawnChild(serverBin, ["--port", String(udpPort), "--ws", String(wsPort),
        "--no-tcp", "--no-persist"]);
    let connection: WsConnection | null = null;
    let server: Server | null = null;
    try {
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(`ws://127.0.0.1:${wsPort}`)
                .catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "the server's WebSocket front never came up");
        server = await new Server({ connection }).attach();
        await body(server);
    } finally {
        server?.close();
        connection?.close();
        child.stop();
        await sleep(50);
    }
}

test("responders fire on the server's node notifications, and stop when freed", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const started: ResponderMessage[] = [];
        const ended: ResponderMessage[] = [];
        const starts = new OscFunc((msg) => started.push(msg), "/node_start", {
            recv: server.receiver,
        });
        const ends = new OscFunc((msg) => ended.push(msg), "/node_end", {
            recv: server.receiver,
        });

        await awaitEngine(server);
        const def = new SynthDef("resp_tone", out(0, sine(440).mul(0.1)));
        await def.send(server);
        await server.sync();

        const synth = new Synth("resp_tone", {}, { server });
        await sleep(200);
        assert.equal(started.length, 1, "one /node_start for one synth");
        assert.equal(Number(started[0]![1]), synth.id);

        synth.free();
        await sleep(200);
        assert.equal(ended.length, 1);
        assert.equal(Number(ended[0]![1]), synth.id);

        // Freed responders hear nothing more, and the server keeps going.
        starts.free();
        ends.free();
        const second = new Synth("resp_tone", {}, { server });
        await sleep(200);
        second.free();
        await sleep(100);
        assert.equal(started.length, 1, "a freed responder stopped matching");
        assert.equal(ended.length, 1);
    });
});

test("a responder hears /done, the answer an asynchronous command carries", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const dones: string[] = [];
        // The argTemplate narrows to one command, exactly as it narrows a
        // trigger id in the reference client's examples.
        const resp = new OscFunc((msg) => dones.push(String(msg[1])), "/done", {
            recv: server.receiver,
            argTemplate: ["/def_send"],
        });
        const def = new SynthDef("resp_done", out(0, sine(220).mul(0.1)));
        await def.send(server);
        await server.sync();
        assert.deepEqual(dones, ["/def_send"]);
        resp.free();
    });
});

test("a def's SendTrig and SendReply reach responders on their own addresses", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        const triggers: number[] = [];
        const replies: ResponderMessage[] = [];
        const trigResp = new OscFunc(
            (msg) => triggers.push(Number(msg[3])),
            "/node_trigger",
            { recv: server.receiver, argTemplate: [null, 7] },
        );
        const replyResp = new OscFunc((msg) => replies.push(msg), "/measured", {
            recv: server.receiver,
        });

        const def = new SynthDef(
            "resp_trig",
            sendTrig(impulse(20), 7, 0.5),
            sendReply(impulse(20), [0.25], { cmd: "/measured", replyId: 3 }),
        );
        await def.send(server);
        await server.sync();

        const synth = new Synth("resp_trig", {}, { server });
        await sleep(300);
        synth.free();
        await sleep(100);

        assert.ok(triggers.length > 0, "the trigger id 7 came through");
        assert.ok(
            triggers.every((v) => Math.abs(v - 0.5) < 1e-6),
            "with the value the def sends",
        );
        assert.ok(replies.length > 0, "SendReply's custom address reached its responder");
        assert.equal(Number(replies[0]![2]), 3, "carrying the reply id");
        trigResp.free();
        replyResp.free();
    });
});

test("the server's own reply handling still works through the responder door", {
    skip: !hasServer,
}, async () => {
    await withServer(async (server) => {
        // Node ids recycle off `/node_end`, which is now an `OscFunc` like any
        // other: what the client waits for and what a page's responders match
        // arrive through the one door, and the recycling still happens.
        await awaitEngine(server);
        const def = new SynthDef("resp_recycle", out(0, sine(330).mul(0.1)));
        await def.send(server);
        await server.sync();

        const synth = new Synth("resp_recycle", {}, { server });
        assert.equal(server.nodes.inUse, 1);
        synth.free();
        await server.sync();
        for (let i = 0; i < 40 && server.nodes.inUse > 0; i++) await sleep(25);
        assert.equal(server.nodes.inUse, 0, "the /node_end responder released the id");
    });
});
