// The resource records, parsed from wire arguments -- no server, no carrier.
//
// The same reply bytes the Python client's `test_parse_query_tree` walks, so
// the two clients are asserted to read one wire the same way, down to the
// drawing a printed tree produces.

import assert from "node:assert/strict";
import test from "node:test";

import {
    formatNodeInfo,
    parseBufferList,
    parseNodeInfo,
    parseQueryTree,
    Tree,
} from "../src/defs/info.ts";
import { loadCore } from "../src/base/core.ts";
import { decodePacket, encodeMessage } from "../src/base/osc.ts";
import type { Connection } from "../src/base/connection.ts";
import { formatLoad, formatServerStatus, Server } from "../src/defs/server/index.ts";

test("a queried tree carries a full node record per entry", () => {
    // detail=2; root 0 -> group 1000 "voices" -> synth 1001 (beep, freq
    // mapped to c5). Every entry is id, childCount, name -- and at this detail
    // a group's name is followed by the two modes it runs under, the root's
    // included. "voices" is auto-ordered and sequential.
    const args = [2, 0, 1, "", 0, 0, 1000, 1, "voices", 1, 0, 1001, -1, "beep", 2,
        "freq", 330.0, "amp", 0.2, 1, 0, 5, 0, "-", "0"];
    const tree = parseQueryTree(args);
    assert.equal(tree.id, 0);
    assert.ok(tree.info.isGroup);
    assert.equal(tree.info.head, 1000);

    const group = tree.children[0]!;
    assert.ok(group.info.isGroup);
    assert.equal(group.info.parent, 0);
    assert.deepEqual([group.info.head, group.info.tail], [1001, 1001]);
    // The modes are read, not inferred: the group is the auto-ordered one and
    // the root is neither.
    assert.deepEqual([group.info.autoOrder, group.info.parallel], [true, false]);
    assert.deepEqual([tree.info.autoOrder, tree.info.parallel], [false, false]);

    // Every entry is a full NodeInfo: what the tree adds is the nesting, and
    // the siblings and head/tail follow from it.
    const synth = group.children[0]!.info;
    assert.equal(synth.id, 1001);
    assert.equal(synth.defname, "beep");
    assert.equal(synth.parent, 1000);
    assert.deepEqual(synth.controls, { freq: 330.0, amp: 0.2 });
    assert.deepEqual(synth.maps, [{ control: 0, bus: 5, audio: false }]);
    assert.deepEqual([synth.reads, synth.writes], ["-", "0"]);
    assert.deepEqual([...tree.walk()].map((i) => i.id), [0, 1000, 1001]);
    assert.equal(tree.find(1001)!.info, synth);

    // The object is the data; its string draws it -- the split the Python
    // client spells `repr` vs `str`.
    assert.ok(tree instanceof Tree);
    assert.equal(group.info.name, "voices");
    assert.deepEqual(String(tree).split("\n"), [
        "group 0",
        '  group 1000 "voices" (auto)',
        "    1001 beep  freq<-c5 amp=0.2",
    ]);
});

test("siblings and an empty group come out of the nesting", () => {
    // detail=0: no controls on the wire, three children of the root.
    const tree = parseQueryTree([0, 0, 3, "", 1001, -1, "a", 1002, -1, "b", 100, 0, ""]);
    const [a, b, empty] = tree.children.map((t) => t.info);
    assert.deepEqual([a!.prev, a!.next], [-1, 1002]);
    assert.deepEqual([b!.prev, b!.next], [1001, 100]);
    assert.ok(empty!.isGroup);
    assert.deepEqual([empty!.head, empty!.tail], [-1, -1]);
    assert.equal(String(tree).split("\n").pop(), "  group 100 (empty)");
});

test("a resource that is not there is a record, not a throw", () => {
    // /node_query.reply with isGroup = -1, and /buffer_query.reply with frames = -1.
    const gone = parseNodeInfo([4242, -1, -1, -1, -1]);
    assert.equal(gone.id, 4242);
    assert.equal(gone.exists, false);

    // A group carries its /group_name after the scsynth fields, and then the
    // two modes it runs under -- the one place they are readable: the wire
    // sets them and nothing else reports them.
    const group = parseNodeInfo([1000, 0, -1, -1, 1, 1001, 1001, "voices", 1, 1]);
    assert.ok(group.isGroup);
    assert.equal(group.name, "voices");
    assert.deepEqual([group.autoOrder, group.parallel], [true, true]);
    assert.equal(formatNodeInfo(group), 'group 1000 "voices" (auto, parallel)');

    const [buffer] = parseBufferList([7, -1, 0, 0.0]);
    assert.equal(buffer!.bufnum, 7);
    assert.equal(buffer!.exists, false);
    assert.equal(buffer!.frames, 0);

    const [held] = parseBufferList([3, 100, 2, 44100.0]);
    assert.ok(held!.exists);
    assert.deepEqual([held!.frames, held!.channels], [100, 2]);
});

// ---- a server with no clock ----

/**
 * One OSC message as bytes, with **nil** (`N`, no bytes) wherever an argument
 * is `null` -- the one thing a server says and this client never does, so the
 * encoder has no tag for it and a reply that carries one is written here.
 * Whole numbers go as `i`, the rest as `d`, a bigint as `h`.
 */
function replyBytes(addr: string, args: (number | bigint | string | null)[]): Uint8Array {
    const text = (s: string): number[] => {
        const bytes = [...new TextEncoder().encode(s), 0];
        while (bytes.length % 4 !== 0) bytes.push(0);
        return bytes;
    };
    let tags = ",";
    const data: number[] = [];
    const put = (size: number, write: (view: DataView) => void): void => {
        const view = new DataView(new ArrayBuffer(size));
        write(view);
        data.push(...new Uint8Array(view.buffer));
    };
    for (const arg of args) {
        if (arg === null) {
            tags += "N";
        } else if (typeof arg === "string") {
            tags += "s";
            data.push(...text(arg));
        } else if (typeof arg === "bigint") {
            tags += "h";
            put(8, (view) => view.setBigInt64(0, arg));
        } else if (Number.isInteger(arg)) {
            tags += "i";
            put(4, (view) => view.setInt32(0, arg));
        } else {
            tags += "d";
            put(8, (view) => view.setFloat64(0, arg));
        }
    }
    return Uint8Array.from([...text(addr), ...text(tags), ...data]);
}

/** A carrier that answers each command with the next reply scripted for it. */
class Scripted implements Connection {
    readonly stream = true;
    private listeners = new Set<(packet: Uint8Array) => void>();
    private script = new Map<string, Uint8Array[]>();

    /** Queues one answer to `command`. */
    answer(command: string, reply: Uint8Array): void {
        this.script.set(command, [...(this.script.get(command) ?? []), reply]);
    }
    send(packet: Uint8Array): void {
        for (const msg of decodePacket(packet)) {
            const next = this.script.get(msg.addr)?.shift();
            if (next) for (const listener of [...this.listeners]) listener(next);
        }
    }
    addReply(listener: (packet: Uint8Array) => void): void {
        this.listeners.add(listener);
    }
    removeReply(listener: (packet: Uint8Array) => void): void {
        this.listeners.delete(listener);
    }
    close(): void {
        this.listeners.clear();
    }
}

async function scripted(): Promise<{ server: Server; carrier: Scripted }> {
    await loadCore();
    const carrier = new Scripted();
    carrier.answer("/server_query", encodeMessage(
        "/server_query.reply",
        [128, 16384, 2, 64, 48000, 48000, 0, 8192, 4096, 512, 32, 8, 16384, 65536]
            .map((n) => ["i", n] as ["i", number]),
    ));
    return { server: new Server({ connection: carrier, timeout: 0.5 }), carrier };
}

test("a server with no clock reports no time and still counts", async () => {
    // An engine in a page cannot time a block, and says so: a nil where a
    // second would be, not a zero that would read as an idle role. The counts
    // are real -- a run is counted, not timed -- and no share is ever derived,
    // however many readings are taken. The lines are the Python client's.
    const { server, carrier } = await scripted();
    let rows = await (async () => {
        carrier.answer("/server_load", replyBytes("/server_load.reply",
            [null, 2, "audio", 0, null, 100n, "net", 0, null, 12n]));
        return server.load();
    })();
    carrier.answer("/server_load", replyBytes("/server_load.reply",
        [null, 2, "audio", 0, null, 230n, "net", 0, null, 12n]));
    rows = await server.load();
    assert.deepEqual(rows.map((row) => row.busy), [null, null]);
    assert.deepEqual(rows.map((row) => row.share), [undefined, undefined]);
    assert.equal(rows[0]!.calls, 230);
    assert.deepEqual(formatLoad(rows).split("\n"), [
        "server load: not available (this server cannot time itself)",
        "  audio    230 calls",
        "  net      12 calls",
    ]);

    // And a timed reading after it starts over: no baseline was kept.
    carrier.answer("/server_load", replyBytes("/server_load.reply",
        [4.5, 1, "audio", 0, 1.5, 10n]));
    const timed = await server.load();
    assert.equal(timed[0]!.busy, 1.5);
    assert.equal(timed[0]!.share, undefined);
});

test("a status with no clock has no CPU figures", async () => {
    const { server, carrier } = await scripted();
    carrier.answer("/server_status", replyBytes("/server_status.reply",
        [12, 3, 2, 5, null, null, 48000.5, 48000.5, null]));
    const status = await server.status();
    assert.deepEqual([status.synths, status.defs], [3, 5]);
    assert.deepEqual([status.avgCpu, status.peakCpu, status.lateBlocks], [null, null, null]);
    assert.equal(
        formatServerStatus(status).split("\n").pop(),
        "  cpu     not available (this server cannot time itself)",
    );

    carrier.answer("/server_status", replyBytes("/server_status.reply",
        [12, 3, 2, 5, 1.5, 4.125, 48000.5, 48000.5, 2]));
    assert.equal(
        formatServerStatus(await server.status()).split("\n").pop(),
        "  cpu     1.5% avg, 4.1% peak, 2 late",
    );
});
