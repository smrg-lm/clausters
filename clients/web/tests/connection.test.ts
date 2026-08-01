// The WS carrier end to end: a real `clausters --ws` server, one node
// process, one /server_status round trip through `WsConnection` (node's global
// `WebSocket` speaks the same standard API as the browser's).
//
// Needs the debug server built (`cargo build` at the workspace root); the
// test spawns and kills its own server, so the sandbox's per-invocation
// network isolation is satisfied. Skips (does not fail) when the binary is
// missing, so `npm test` stays runnable from a source tree without a build.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

import { WsConnection } from "../src/base/connection.ts";
import { loadOsc, encodeMessage, decodePacket } from "../src/base/osc.ts";

const here = new URL(".", import.meta.url);
const serverBin = new URL("../../../target/debug/clausters", here).pathname;
const wsPort = 57987; // out of the default range; parallel-test friendly

const hasServer = await access(serverBin).then(() => true, () => false);

await loadOsc(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)),
);

test("WsConnection: /server_status round trip", { skip: !hasServer }, async () => {
    const server = spawn(
        serverBin,
        ["--ws", String(wsPort), "--no-tcp", "--no-persist"],
        { stdio: "ignore" },
    );
    try {
        // The server binds the WS listener during boot; retry until it is up.
        let connection: WsConnection | null = null;
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(
                `ws://127.0.0.1:${wsPort}`,
            ).catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "server WS endpoint never came up");

        const reply = new Promise<number>((resolve) => {
            connection.addReply((bytes) => {
                for (const { addr, args } of decodePacket(bytes)) {
                    if (addr === "/server_status.reply") resolve(args[2] as number);
                }
            });
        });
        connection.send(encodeMessage("/server_status"));
        const synths = await Promise.race([
            reply,
            sleep(5000).then(() => {
                throw new Error("no /server_status.reply within 5s");
            }),
        ]);
        assert.equal(synths, 0, "a fresh server reports zero synths");
        connection.close();
    } finally {
        server.kill();
    }
});
