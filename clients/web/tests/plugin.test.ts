import assert from "node:assert/strict";
import test from "node:test";
import { PluginDef } from "../src/defs/plugindef.ts";
import { Node } from "../src/defs/node.ts";
import type { Server } from "../src/defs/server/index.ts";
import { parseDefInfo } from "../src/defs/info.ts";

test("native references, plain-value ControlInfo and restart use the shared wire", async () => {
    const calls: unknown[][] = [];
    const server = {
        sendMsg: (...args: unknown[]) => calls.push(args),
        command: async (...args: unknown[]) => { calls.push(args); },
    } as unknown as Server;
    const reference = { format: "clap", path: "/plugins/test.clap", id: "test",
        inputs: [], outputs: [{ id: 0, channels: 1, bus: 0 }] };
    const def = new PluginDef("test", reference);
    await def.send(server, { wait: false });
    assert.deepEqual(calls.pop(), ["/def_send", "plugin", "test", JSON.stringify(reference)]);
    const info = parseDefInfo(["test", "plugin", 1, "param:17", 0.5, "kr", 0, 1, 0]);
    assert.equal(info.controls[0].min, 0);
    assert.equal(info.controls[0].max, 1);
    const node = new Node(42, server);
    assert.equal(await node.restartPlugin(), node);
    assert.deepEqual(calls.pop(), ["/plugin_restart", [42], 10]);
});

test("persistent events are addressed to the same node in the reference order", () => {
    const calls: unknown[][] = [];
    const server = { sendMsg: (...args: unknown[]) => calls.push(args) } as unknown as Server;
    const node = new Node(42, server);
    const events = [
        { kind: "note_on", source: 1, note_id: 1, key: 60, value: 0.5 },
        { kind: "expression", source: 1, note_id: 1, id: 6, value: 0.3 },
        { kind: "note_off", source: 1, note_id: 1, key: 60, value: 0.0 },
    ];
    for (const event of events) assert.equal(node.pluginEvent(event), node);
    assert.deepEqual(calls, events.map((event) => ["/plugin_event", ["i", 42], JSON.stringify(event)]));
});
