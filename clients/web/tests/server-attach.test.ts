// The three verbs a server handle comes up with: the constructor, `boot` and
// `attach` — and what each of them does when there is nothing behind the
// carrier.
//
// A browser carrier can be open and empty: a WebSocket endpoint that accepts
// but speaks no OSC, a port wired to an engine that never came up. The
// constructor does not care, because it reaches nothing; `attach` is the verb
// that says so, which is the reference client's rule (`Server(...)` is a
// handle, `attach()` verifies) and not a browser's invention.
//
// No wasm audio and no socket: a recording carrier stands in for both engines.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import type { Connection } from "../src/base/connection.ts";
import { Server } from "../src/defs/server/index.ts";
import { ServerError } from "../src/errors.ts";

await loadCore();

/** A carrier that records what it is given and never replies. */
function silent(url?: string): Connection & { packets: Uint8Array[] } {
    const packets: Uint8Array[] = [];
    return {
        packets,
        url,
        send: (packet) => packets.push(packet),
        addReply: () => {},
        removeReply: () => {},
        close: () => {},
    };
}

/** A carrier a page can bring up: what `pageConnection` offers `boot`. */
function bootable(): Connection & { packets: Uint8Array[]; booted: number } {
    const base = silent();
    const carrier = base as Connection & { packets: Uint8Array[]; booted: number };
    carrier.booted = 0;
    carrier.boot = async () => { carrier.booted += 1; };
    return carrier;
}

test("a handle reaches nothing, and says nothing, until it is told to", () => {
    const carrier = silent();
    const server = new Server({ connection: carrier });
    // Not one packet: the constructor is a handle over an address, the way the
    // reference client's is. Whether a server is there is not its question.
    assert.equal(carrier.packets.length, 0);
    // And the allocators are sized from what it was told (here, the compiled
    // defaults), which is the guess `reconcile` exists to replace.
    assert.equal(server.sizing.audioBuses, 128);
    server.close();
});

test("attach refuses a carrier nobody answers on", async () => {
    const server = new Server({ connection: silent("ws://127.0.0.1:57120"), timeout: 0.2 });
    await assert.rejects(
        () => server.attach({ notify: false }),
        (error: unknown) => {
            assert.ok(error instanceof ServerError);
            // The address is in the message: which carrier is empty is the
            // whole question when a page talks to more than one.
            assert.match((error as Error).message, /ws:\/\/127\.0\.0\.1:57120/);
            return true;
        },
    );
    server.close();
});

test("attach probes even when the sizing was given", async () => {
    // What is being verified is the *server*, not the numbers — an explicit
    // sizing is exactly the case where nothing else would have asked.
    const carrier = silent();
    const server = new Server({ connection: carrier, sizing: { maxNodes: 64 }, timeout: 0.2 });
    await assert.rejects(() => server.attach({ notify: false }), ServerError);
    assert.equal(carrier.packets.length, 1, "one /server_query went out");
    server.close();
});

test("boot refuses a carrier this page cannot bring anything up on", async () => {
    // A socket points at a machine a tab can start nothing on. The reference
    // client refuses the same way, for the same reason, when a handle pointing
    // at another host is asked to boot.
    const server = new Server({ connection: silent("ws://127.0.0.1:57120"), timeout: 0.2 });
    await assert.rejects(
        () => server.boot({ notify: false }),
        (error: unknown) => {
            assert.ok(error instanceof ServerError);
            assert.match((error as Error).message, /attach\(\)/);
            return true;
        },
    );
    server.close();
});

test("boot asks the carrier to bring its own server up", async () => {
    // What "boot" means belongs to the carrier: the page's engine starts its
    // audio, a score has nothing to start. Here the fake counts the ask.
    // `reconcile: false`, because a fake that never replies has no capacities
    // to report and the round trip is not what this asserts.
    const carrier = bootable();
    const server = new Server({ connection: carrier, timeout: 0.2 });
    await server.boot({ reconcile: false, notify: false });
    assert.equal(carrier.booted, 1, "the carrier was asked to come up");
    assert.equal(server.booted, true, "and the handle knows it owns what came up");
    assert.equal(carrier.packets.length, 0, "with neither a query nor a notify");
    server.close();
});

test("a bulk chunk reads the carrier's capability, not its type", async () => {
    // The reference client's `test_bulk_chunk_reads_the_carrier_capability_not_its_type`:
    // a carrier this module never heard of answers the datagram-or-stream
    // question itself, through `Connection.stream`.
    const framed = { ...silent(), stream: true };
    const server = new Server({ connection: framed, timeout: 0.2 });
    // As if `/server_query` had answered: the handle caches the ceiling, and
    // a stream carrier is the one allowed to size a request from it.
    (server as unknown as { maxFrame: number }).maxFrame = 1024 * 1024;
    assert.equal(await server.bulkChunk(0.2), Math.floor((1024 * 1024 - 256) / 4));

    // Bounded by one delivery — a datagram, the page's 64 KiB ring, which
    // drops a reply it cannot hold instead of splitting it. The ceiling is
    // cached all the same and is simply not what the chunk comes from.
    const bounded = new Server({ connection: silent(), timeout: 0.2 });
    (bounded as unknown as { maxFrame: number }).maxFrame = 1024 * 1024;
    assert.equal(await bounded.bulkChunk(0.2), 1024);
});
