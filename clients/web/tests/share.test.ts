// Two clients on one server: the id spaces they split, and the arithmetic that
// keeps them apart.
//
// The server partitions node ids into a client range, its own auto range and
// its MIDI range, and every client allocates from that one client range —
// exact while a server has one client, a fiction the moment it has two: two
// processes driving one server, or a script authoring beside a page that holds
// a session on the very same in-page engine.
//
// What makes it work without a negotiation is that there is nothing to
// negotiate: equal slices in a fixed order, so `{index: 0, of: 2}` and
// `{index: 1, of: 2}` are disjoint by construction. This suite pins that, and
// pins that it reaches every space a client allocates from rather than the
// node ids alone — a page that cannot collide on nodes and does collide on
// buffers is no better off.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore, nodeIdPartition, shareOf, WHOLE_SHARE } from "../src/base/core.ts";
import type { Connection } from "../src/base/connection.ts";
import { Server } from "../src/defs/server/index.ts";
import { GuiHost } from "../src/gui/host.ts";
import { GuiIdAllocator, BASE_ID, CAPACITY } from "../src/gui/ids.ts";

await loadCore(
    await readFile(
        new URL("../dist/core/clausters_core_web_bg.wasm", new URL(".", import.meta.url)),
    ),
);

/** A carrier that records and never replies. */
const recorder = (): Connection => ({
    send: () => {},
    addReply: () => {},
    removeReply: () => {},
    close: () => {},
});

const SIZING = {
    maxNodes: 8192, audioBuses: 128, controlBuses: 16384,
    maxBuffers: 4096, channels: 2,
};

const openShared = (index: number, of: number) =>
    new Server(recorder(), { sizing: SIZING, share: { index, of } });

test("the default share is the whole space, which is what one client takes", () => {
    const p = nodeIdPartition(8192);
    assert.deepEqual(shareOf(p.clientBase, p.clientCapacity), [p.clientBase, p.clientCapacity]);
    assert.deepEqual(shareOf(0, 100, WHOLE_SHARE), [0, 100]);
});

test("shares tile the range exactly, the last one taking the remainder", () => {
    // No id belongs to two clients, and none belongs to nobody: a range that
    // does not divide evenly must not leave a gap at the top.
    const of = 3;
    const slices = [0, 1, 2].map((index) => shareOf(1000, 10_001, { index, of }));
    assert.deepEqual(slices.map(([base]) => base), [1000, 4333, 7666]);
    let next = 1000;
    for (const [base, span] of slices) {
        assert.equal(base, next, "a slice starts where the previous one ends");
        next = base + span;
    }
    assert.equal(next, 11_001, "the slices cover the whole range");
});

test("a share outside its split is refused rather than aliased", () => {
    assert.throws(() => shareOf(0, 10, { index: 2, of: 2 }), RangeError);
    assert.throws(() => shareOf(0, 10, { index: -1, of: 2 }), RangeError);
    assert.throws(() => shareOf(0, 10, { index: 0, of: 0 }), RangeError);
});

test("two clients of one server allocate ids that cannot collide", async () => {
    const kernel = await openShared(0, 2);
    const page = await openShared(1, 2);

    // Every space a client allocates from, not the node ids alone.
    const nodes = [kernel.nodes.alloc(), page.nodes.alloc()];
    assert.notEqual(nodes[0], nodes[1]);
    assert.ok(nodes[0] < nodes[1], "the shares are taken in index order");

    const buffers = [kernel.buffers.alloc(), page.buffers.alloc()];
    assert.notEqual(buffers[0], buffers[1]);

    const audio = [kernel.audioBuses.alloc(2), page.audioBuses.alloc(2)];
    assert.notEqual(audio[0].index, audio[1].index);
    const control = [kernel.controlBuses.alloc(), page.controlBuses.alloc()];
    assert.notEqual(control[0].index, control[1].index);

    // And the first id of the second share is past the whole first share, so
    // exhausting one client's range never walks into the other's.
    const p = nodeIdPartition(SIZING.maxNodes);
    const [, span] = shareOf(p.clientBase, p.clientCapacity, { index: 0, of: 2 });
    assert.ok(nodes[1] >= p.clientBase + span);

    kernel.close();
    page.close();
});

test("a shared client keeps the reservations its whole-space peer keeps", async () => {
    // The output buses are the server's, not a client's, so neither share may
    // hand them out — a split must not open a hole below itself.
    const page = await openShared(1, 2);
    const whole = new Server(recorder(), { sizing: SIZING });
    assert.ok(page.audioBuses.alloc(2).index >= SIZING.channels);
    assert.ok(whole.audioBuses.alloc(2).index >= SIZING.channels);
    page.close();
    whole.close();
});

test("widget ids split the same way, so two clients of one host agree too", () => {
    const kernel = new GuiIdAllocator(BASE_ID, CAPACITY, { index: 0, of: 2 });
    const page = new GuiIdAllocator(BASE_ID, CAPACITY, { index: 1, of: 2 });
    const first = kernel.alloc();
    const second = page.alloc();
    assert.equal(first, BASE_ID);
    assert.equal(second, BASE_ID + CAPACITY / 2);
});

test("a host client takes the share it is given", () => {
    const kernel = new GuiHost(recorder(), { share: { index: 0, of: 2 } });
    const page = new GuiHost(recorder(), { share: { index: 1, of: 2 } });
    assert.notEqual(kernel.allocId(), page.allocId());
    kernel.stop();
    page.stop();
});
