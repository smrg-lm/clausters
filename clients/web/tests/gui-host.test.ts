// The WS carrier against a native GUI host: a real `clausters-gui --ws`
// (headless), one node process, a /gui_def + /gui_query round trip through
// `WsConnection` — the browser's path into a desktop host, the same seam the
// audio-server test drives (`connection.test.ts`).
//
// Needs the host binary built (`cargo build` in clients/gui — its own
// workspace); skips (does not fail) when it is missing, so `npm test` stays
// runnable from a source tree without that build.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import { setTimeout as sleep } from "node:timers/promises";

import { WsConnection } from "../src/base/connection.ts";
import { loadOsc, encodeMessage, decodePacket } from "../src/base/osc.ts";

const here = new URL(".", import.meta.url);
const hostBin = new URL("../../gui/target/debug/clausters-gui", here).pathname;
const wsPort = 57988; // out of the default range; parallel-test friendly
const udpPort = 57989; // the host front's UDP port, moved off 57210 likewise

const hasHost = await access(hostBin).then(() => true, () => false);

await loadOsc(
    await readFile(new URL("../dist/core/clausters_core_web_bg.wasm", here)),
);

test("WsConnection: /gui_def + /gui_query against a native host", { skip: !hasHost }, async () => {
    const host = spawn(
        hostBin,
        ["--headless", "--no-tcp", "--port", String(udpPort), "--ws", String(wsPort)],
        { stdio: "ignore" },
    );
    try {
        // The host binds the WS listener during boot; retry until it is up.
        let connection: WsConnection | null = null;
        for (let i = 0; i < 50 && !connection; i++) {
            connection = await WsConnection.open(
                `ws://127.0.0.1:${wsPort}`,
            ).catch(() => null);
            if (!connection) await sleep(100);
        }
        assert.ok(connection, "host WS endpoint never came up");

        const info = new Promise<(number | string)[]>((resolve) => {
            connection.addReply((bytes) => {
                for (const { addr, args } of decodePacket(bytes)) {
                    if (addr === "/gui_info") {
                        resolve(args as (number | string)[]);
                    }
                }
            });
        });
        const tree = JSON.stringify({
            id: 1,
            type: "window",
            title: "ws front",
            children: [{ id: 2, type: "knob", label: "freq", value: 220.0 }],
        });
        connection.send(encodeMessage("/gui_def", [["i", 1], ["s", tree]]));
        connection.send(encodeMessage("/gui_query", [["i", 2]]));
        const args = await Promise.race([
            info,
            sleep(5000).then(() => {
                throw new Error("no /gui_info within 5s");
            }),
        ]);
        assert.equal(args[0], 2, "the queried widget id comes back first");
        assert.equal(args[1], "knob", "the widget type follows");
        connection.close();
    } finally {
        host.kill();
    }
});
