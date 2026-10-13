// Discovery metadata uses the same bounded fragments as the Python client.
import assert from "node:assert/strict";
import test from "node:test";
import { ServerQueries } from "../src/defs/server/queries.ts";
import type { Server } from "../src/defs/server/index.ts";
import { CommandError } from "../src/errors.ts";

test("scanPlugins rebuilds ordered JSON and rejects missing fragments", async () => {
    const row = { kind: "plugin", plugin: { id: "org.test.gain", parameters: [{ id: 19 }] } };
    const bytes = new TextEncoder().encode(JSON.stringify(row));
    let rows: unknown[][] = [[-1, 1, 0, 0, new Uint8Array()],
        [0, 1, 1, 2, bytes.slice(17)], [0, 1, 0, 2, bytes.slice(0, 17)]];
    const server = { requestBatch: async (addr: string, dirs: string[]) => {
        assert.equal(addr, "/plugin_scan"); assert.deepEqual(dirs, ["/plugins"]);
        return rows.map((args) => ({ addr: "/plugin_scan.reply", args }));
    } } as unknown as Server;
    assert.deepEqual(await ServerQueries.prototype.scanPlugins.call(server, ["/plugins"]), [row]);
    rows = [[-1, 1, 0, 0, new Uint8Array()], [0, 1, 0, 2, bytes.slice(0, 17)]];
    await assert.rejects(ServerQueries.prototype.scanPlugins.call(server, ["/plugins"]), CommandError);
});
