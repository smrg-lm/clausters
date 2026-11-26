// The host an editor names for its session: read from the environment, and
// pointed at the session's server with `/gui_server` -- the reference client's
// `tests/test_gui_named_host.py`.

import assert from "node:assert/strict";
import test from "node:test";
import { loadCore } from "../src/base/core.ts";
import type { Connection } from "../src/base/connection.ts";
import { decodePacket, encodeMessage, oscArg } from "../src/base/osc.ts";
import { GuiHost, HOST_VARIABLE, namedHost } from "../src/gui/host.ts";

test("the environment names the host by carrier", () => {
    const before = process.env[HOST_VARIABLE];
    try {
        delete process.env[HOST_VARIABLE];
        assert.equal(namedHost(), null);
        process.env[HOST_VARIABLE] = "udp://127.0.0.1:40001 ws://127.0.0.1:40002";
        assert.equal(namedHost(), "ws://127.0.0.1:40002", "the ws:// one");
        process.env[HOST_VARIABLE] = "udp://127.0.0.1:40001";
        assert.equal(namedHost(), null, "nothing this client speaks");
    } finally {
        if (before === undefined) delete process.env[HOST_VARIABLE];
        else process.env[HOST_VARIABLE] = before;
    }
});

test("a host is pointed at a server", async () => {
    await loadCore();
    const sent: Uint8Array[] = [];
    const connection: Connection = {
        send: (bytes: Uint8Array) => {
            sent.push(bytes);
        },
        addReply: () => {},
        removeReply: () => {},
        close: () => {},
    };
    const host = new GuiHost({ connection });
    host.server("127.0.0.1:57110", { shm: "/clausters", idShare: { index: 1, of: 2 } });
    host.server("127.0.0.1:57110");
    const messages = sent.flatMap((bytes) => decodePacket(bytes));
    assert.deepEqual(
        messages.map((m) => [m.addr, ...m.args]),
        [
            ["/gui_server", "127.0.0.1:57110", "/clausters", "1/2"],
            ["/gui_server", "127.0.0.1:57110", "", ""],
        ],
    );
});

test("the quit hooks answer the host's question", async () => {
    await loadCore();
    const sent: { addr: string; args: unknown[] }[] = [];
    let feed: (bytes: Uint8Array) => void = () => {};
    const connection: Connection = {
        send: (bytes: Uint8Array) => {
            for (const m of decodePacket(bytes)) {
                sent.push(m);
                // the dock answers its calls at once
                if (m.addr === "/gui_dock" && m.args[0] !== 0) {
                    feed(message("/gui_dock_reply", [m.args[0] as number, '{"ok": {}}']));
                }
            }
        },
        addReply: (handler: (bytes: Uint8Array) => void) => {
            feed = handler;
        },
        removeReply: () => {},
        close: () => {},
    };
    const message = (addr: string, args: (string | number)[]) => encodeMessage(addr, args.map(oscArg));
    const host = new GuiHost({ connection });
    const ended: boolean[] = [];
    const told: unknown[] = [];
    await host.onQuit(() => ended.push(true), async () => false, (documents) => told.push(documents));
    host.ownUnsaved(["a.py"]);
    const last = () => JSON.parse(String(sent[sent.length - 1].args[1]));
    assert.deepEqual(last(), { verb: "own_unsaved", unsaved: ["a.py"] });
    const listed = [{ window: 7, title: "Audio" }];
    feed(message("/gui_dock_unsaved", [JSON.stringify({ unsaved: listed })]));
    assert.deepEqual(told, [listed]);
    feed(message("/gui_dock_quit", ['{"do": "save"}']));
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.deepEqual(last(), { verb: "saved", saved: false });
    assert.deepEqual(ended, []);
    feed(message("/gui_dock_quit", ['{"do": "quit"}']));
    assert.deepEqual(ended, [true]);
    host.stop();
});
