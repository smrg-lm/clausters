// The panic: one verb that stops what an environment plays -- its clocks
// emptied, its timelines halted, its server silenced, the responders freed --
// and leaves everything ready to play again. The reference client's
// `tests/test_panic.py`, over a recording carrier instead of a server.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { decodePacket } from "../src/base/osc.ts";
import type { Connection } from "../src/base/connection.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import { OscReceiver } from "../src/base/receiver.ts";
import { Routine } from "../src/base/stream.ts";
import { main } from "../src/base/main.ts";
import { Server } from "../src/defs/server/index.ts";
import { OscFunc } from "../src/responders.ts";
import { Session } from "../src/session.ts";
import { panic } from "../src/panic.ts";

await loadCore();

/** A carrier that only records; nothing replies. */
function recorder(): Connection & { packets: Uint8Array[] } {
    const packets: Uint8Array[] = [];
    return {
        packets,
        send: (packet) => packets.push(packet),
        addReply: () => {},
        removeReply: () => {},
        close: () => {},
    };
}

/** A session over a recording carrier, with a clock nothing wakes by itself. */
function fakeSession(): { session: Session; packets: Uint8Array[] } {
    const connection = recorder();
    const server = new Server({
        connection,
        sizing: {
            maxNodes: 8192, audioBuses: 1024, controlBuses: 16384,
            maxBuffers: 4096, channels: 2, transports: 4,
        },
    });
    const session = new Session(server, new TempoClock(1.0, { ticker: manualTicker() }));
    return { session, packets: connection.packets };
}

function* forever(): Generator<number> {
    while (true) yield 1.0;
}

/** Runs `body` with the default session's slots emptied, and puts them back. */
async function withCleanDefault(body: () => Promise<void> | void): Promise<void> {
    const server = main.server;
    const session = main.currentSession;
    main.server = null;
    main.currentSession = null;
    try {
        await body();
    } finally {
        main.server = server;
        main.currentSession = session;
    }
}

test("a session's panic empties its clocks and no other", () => {
    const a = fakeSession();
    const b = fakeSession();
    a.session.clock.play(new Routine(forever));
    b.session.clock.play(new Routine(forever));
    a.session.panic();
    assert.equal(a.session.clock.queued, 0);
    assert.equal(b.session.clock.queued, 1);
    b.session.clock.clear();
    a.session.close();
    b.session.close();
});

test("the server is silenced: every transport stopped, the queue dropped, every node freed", () => {
    const { session, packets } = fakeSession();
    session.panic();
    const sent = packets.flatMap((p) => decodePacket(p).map((m) => [m.addr, m.args]));
    assert.deepEqual(sent, [
        ["/transport_stop", [0]],
        ["/transport_stop", [1]],
        ["/transport_stop", [2]],
        ["/transport_stop", [3]],
        ["/sched_clear", []],
        ["/group_deepFree", [0]],
    ]);
    session.close();
});

test("the default session reaches the clocks made with no session", () =>
    withCleanDefault(() => {
        const loose = new TempoClock(1.0, { ticker: manualTicker() });
        const { session } = fakeSession();
        loose.play(new Routine(forever));
        session.clock.play(new Routine(forever));
        main.panic();
        assert.equal(loose.queued, 0);
        assert.equal(session.clock.queued, 1, "another session's clock is its own");
        session.clock.clear();
        session.close();
        loose.close();
    }));

test("the free verb panics the session in force", () =>
    withCleanDefault(() => {
        const loose = new TempoClock(1.0, { ticker: manualTicker() });
        const { session } = fakeSession();
        loose.play(new Routine(forever));
        session.clock.play(new Routine(forever));
        session.activate();
        assert.equal(panic(), session);
        session.deactivate();
        assert.equal(session.clock.queued, 0);
        assert.equal(loose.queued, 1);
        loose.clear();
        session.close();
        loose.close();
    }));

test("responders are freed unless permanent", () => {
    const recv = new OscReceiver(recorder());
    const { session } = fakeSession();
    const gone = new OscFunc(() => {}, "/gone", { recv });
    const kept = new OscFunc(() => {}, "/kept", { recv, permanent: true });
    session.panic();
    assert.equal(gone.enabled, false);
    assert.equal(kept.enabled, true);
    kept.free();
    session.close();
});
