import assert from "node:assert/strict";
import test from "node:test";
import { PluginDef } from "../src/defs/plugindef.ts";
import { Node } from "../src/defs/node.ts";
import type { Server } from "../src/defs/server/index.ts";
import { parseDefInfo } from "../src/defs/info.ts";

for (const format of ["clap", "vst3"]) test(`${format} references, ControlInfo and restart use the shared wire`, async () => {
    const calls: unknown[][] = [];
    const server = {
        sendMsg: (...args: unknown[]) => calls.push(args),
        command: async (...args: unknown[]) => { calls.push(args); },
    } as unknown as Server;
    const reference = { format, path: `/plugins/test.${format}`, id: "test",
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


test("opaque live state reassembles validated chunks and loads the shared blob", async () => {
    const calls: unknown[][] = [];
    let rows = [{ addr: "/plugin_saveState.reply", args: [42, 1, 2, new Uint8Array([3])] },
                { addr: "/plugin_saveState.reply", args: [42, 0, 2, new Uint8Array([1, 2])] }];
    const server = {
        requestBatch: async (...args: unknown[]) => { calls.push(args); return rows; },
        command: async (...args: unknown[]) => { calls.push(args); },
    } as unknown as Server;
    const node = new Node(42, server);
    const state = await node.savePluginState();
    assert.deepEqual(state, new Uint8Array([1, 2, 3]));
    assert.deepEqual(calls.pop(), ["/plugin_saveState", [42], { reply: "/plugin_saveState.reply", timeout: 10 }]);
    assert.equal(await node.loadPluginState(state), node);
    assert.deepEqual(calls.pop(), ["/plugin_loadState", [42, state], 10]);
    for (const invalid of [[], [rows[0]], [rows[1], rows[1]],
        [{ addr: "/plugin_saveState.reply", args: [43, 0, 1, new Uint8Array()] }],
        [{ addr: "/plugin_saveState.reply", args: [42, 0, 0, new Uint8Array()] }]]) {
        rows = invalid;
        await assert.rejects(node.savePluginState());
    }
    rows = [{ addr: "/plugin_saveState.reply", args: [42, 0, 1, new Uint8Array()] }];
    assert.deepEqual(await node.savePluginState(), new Uint8Array());
});


test("live plugin state refuses an OSC score rather than waiting or scoring it", async () => {
    const node = new Node(42, { scoring: true } as Server);
    await assert.rejects(node.savePluginState(), /OSC score/);
    await assert.rejects(node.loadPluginState(new Uint8Array()), /OSC score/);
});
