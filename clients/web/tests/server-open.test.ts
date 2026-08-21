// Opening a server over a carrier with nothing behind it.
//
// A browser carrier can be open and empty: a WebSocket endpoint that accepts
// but speaks no OSC, a port wired to an engine that never came up. By default
// the handle is built anyway on the compiled sizing — the page keeps working
// against a server that may yet answer — and every command leaves without a
// trace if it does not. `verify` is the opposite ask, and the browser's half of
// the reference client's `Server.attach`: nothing answered, so say so here.
//
// No wasm audio and no socket: a recording carrier stands in for both engines.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadOsc } from "../src/base/osc.ts";
import type { Connection } from "../src/base/connection.ts";
import { Server } from "../src/defs/server/index.ts";
import { ServerError } from "../src/errors.ts";

await loadOsc(
    await readFile(
        new URL("../dist/core/clausters_core_web_bg.wasm", new URL(".", import.meta.url)),
    ),
);

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

test("a silent carrier still opens, sized from the defaults", async () => {
    // The warning is the point of the default path, so it is asserted rather
    // than left to print: this is the one test that provokes it, and a console
    // line in the suite's output is noise nobody reads twice.
    const warnings: string[] = [];
    const warn = console.warn;
    console.warn = (message: string) => warnings.push(message);
    let server: Server;
    try {
        server = await Server.open(silent(), { notify: false, timeout: 0.2 });
    } finally {
        console.warn = warn;
    }
    try {
        // The page is not stopped by a server that has not answered yet.
        assert.equal(server.sizing.audioBuses, 128);
        assert.match(warnings.join("\n"), /no \/server_query reply/);
    } finally {
        server.close();
    }
});

test("verify turns a silent carrier into an error", async () => {
    await assert.rejects(
        () => Server.open(silent("ws://127.0.0.1:57120"), {
            notify: false, verify: true, timeout: 0.2,
        }),
        (error: unknown) => {
            assert.ok(error instanceof ServerError);
            // The address is in the message: which carrier is empty is the
            // whole question when a page talks to more than one.
            assert.match((error as Error).message, /ws:\/\/127\.0\.0\.1:57120/);
            return true;
        },
    );
});

test("verify probes even when the sizing was given", async () => {
    // What is being verified is the server, not the numbers — an explicit
    // sizing is exactly the case where nothing else would have asked.
    const connection = silent();
    await assert.rejects(
        () => Server.open(connection, {
            sizing: { maxNodes: 64 }, notify: false, verify: true, timeout: 0.2,
        }),
        ServerError,
    );
    assert.equal(connection.packets.length, 1, "one /server_query went out");
});

test("a bulk chunk reads the carrier's capability, not its type", async () => {
    // The reference client's `test_bulk_chunk_reads_the_carrier_capability_not_its_type`:
    // a carrier this module never heard of answers the datagram-or-stream
    // question itself, through `Connection.stream`.
    const framed = { ...silent(), stream: true };
    const server = await Server.open(framed, { notify: false, timeout: 0.2 });
    // As if `/server_query` had answered: the handle caches the ceiling, and
    // a stream carrier is the one allowed to size a request from it.
    (server as unknown as { maxFrame: number }).maxFrame = 1024 * 1024;
    assert.equal(await server.bulkChunk(0.2), Math.floor((1024 * 1024 - 256) / 4));

    // Bounded by one delivery — a datagram, the page's 64 KiB ring, which
    // drops a reply it cannot hold instead of splitting it. The ceiling is
    // cached all the same and is simply not what the chunk comes from.
    const bounded = await Server.open(silent(), { notify: false, timeout: 0.2 });
    (bounded as unknown as { maxFrame: number }).maxFrame = 1024 * 1024;
    assert.equal(await bounded.bulkChunk(0.2), 1024);
});
