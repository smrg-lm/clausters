// The ambient layer: the default session, an explicit one, and what resolves
// against them.
//
// The rule this exercises is one line — everything that does not run in an
// explicit `Session` runs in the default session — and it is worth a suite
// because the failures it prevents are silent: a synth created on the wrong
// server, a routine played on a clock nobody started, two sessions sharing one
// random root.
//
// A fake carrier stands in for both engines, so this is pure logic: no wasm
// audio, no socket, no clock actually ticking (the manual ticker drives it).

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { decodePacket } from "../src/base/osc.ts";
import type { Connection } from "../src/base/connection.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import type { ManualTicker } from "../src/base/clock.ts";
import { Routine } from "../src/base/stream.ts";
import { main } from "../src/base/main.ts";
import { Bus } from "../src/defs/bus.ts";
import { Group, Synth } from "../src/defs/node.ts";
import { Server } from "../src/defs/server/index.ts";
import { Session } from "../src/session.ts";
import { play } from "../src/play.ts";
import { Event } from "../src/seq/event.ts";
import { flush } from "./flush.ts";

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

// `adoptDefault: false`: opening a carrier claims the default slot when it is
// free (the reference client's `adopt_default`), and what this file is about is
// what a **session** does with that slot — so the fakes stay out of it and each
// test claims it explicitly.
// A bare handle: nothing to boot behind a recorder and nothing to attach to,
// which is exactly the constructor's case — it reaches nothing and sizes the
// allocators from what it is told. It also keeps these fakes out of the default
// slot, and what this file is about is what a **session** does with that slot.
const openServer = (connection: Connection) =>
    new Server({ connection,
        sizing: {
            maxNodes: 8192, audioBuses: 128, controlBuses: 16384,
            maxBuffers: 4096, channels: 2,
        },
    });

/** A session over a recording carrier, with a clock nothing wakes by itself. */
async function fakeSession(): Promise<{
    session: Session;
    packets: Uint8Array[];
    ticker: ManualTicker;
}> {
    const connection = recorder();
    const server = await openServer(connection);
    const ticker = manualTicker();
    const session = new Session(server, new TempoClock(1.0, { ticker }));
    return { session, packets: connection.packets, ticker };
}

/** The addresses sent, in order. */
const addrs = (packets: readonly Uint8Array[]): string[] =>
    packets.flatMap((p) => decodePacket(p).map((m) => m.addr));

/** The default clock, read through a call so no narrowing leaks across it. */
const defaultClock = (): TempoClock | null => main.defaultClock;

/** Empties the default session's three slots. */
function clearDefault(): void {
    main.server = null;
    main.defaultClock = null;
    main.currentSession = null;
}

/** Runs `body` with the default session's slots emptied, and puts them back. */
async function withCleanDefault(body: () => Promise<void> | void): Promise<void> {
    const server = main.server;
    const clock = main.defaultClock;
    const session = main.currentSession;
    clearDefault();
    try {
        await body();
    } finally {
        main.defaultClock?.close();
        main.server = server;
        main.defaultClock = clock;
        main.currentSession = session;
    }
}

test("with nothing opened, resolution fails by naming the two ways to open one", () =>
    withCleanDefault(() => {
        assert.throws(() => main.resolveServer(), /Session\.embed\(\)|Session\.live/);
        // And the failure reaches the surface a script actually types.
        assert.throws(() => new Synth("beep"), /no server to play on/);
    }));

test("an activated session is what a bare constructor reaches, and it lets go", () =>
    withCleanDefault(async () => {
        const { session, packets } = await fakeSession();
        assert.equal(main.currentSession, null,
            "a session is not ambient by merely existing");

        session.activate();
        const synth = new Synth("beep", { freq: 440 });
        assert.equal(synth.server, session.server);
        assert.deepEqual(addrs(packets), ["/synth_new"]);

        // Last-wins, not first-wins: `activate` is the reference client's, and
        // it says "this one, from here on" rather than claiming a free slot.
        const other = await fakeSession();
        other.session.activate();
        assert.equal(new Synth("beep").server, other.session.server);

        // And it is reversible, which `adoptDefault` never was.
        other.session.deactivate();
        assert.equal(main.currentSession, null);

        // Giving up a slot one does not hold is a no-op, so a session cannot
        // unseat another by deactivating.
        session.activate();
        other.session.deactivate();
        assert.equal(main.currentSession, session);

        session.close();
        assert.equal(main.currentSession, null);
        other.session.close();
    }));

test("`use` scopes the ambient session, and restores the previous one", () =>
    withCleanDefault(async () => {
        const a = await fakeSession();
        const b = await fakeSession();
        a.session.activate();

        const inside = b.session.use(() => new Synth("beep"));
        assert.equal(inside.server, b.session.server, "the block's session wins");
        assert.deepEqual(addrs(b.packets), ["/synth_new"]);
        assert.deepEqual(addrs(a.packets), [], "the default was untouched inside");

        // Restored: back to the default the page had.
        assert.equal(new Synth("beep").server, a.session.server);
        // Nesting unwinds in order.
        b.session.use(() => {
            a.session.use(() => {
                assert.equal(main.currentSession, a.session);
            });
            assert.equal(main.currentSession, b.session);
        });
        // Back to what was ambient around the blocks -- which `activate` made
        // `a`, where the old first-wins adoption left this slot empty.
        assert.equal(main.currentSession, a.session);
        a.session.close();
        b.session.close();
    }));

test("every resource constructor resolves the same ambient server", () =>
    withCleanDefault(async () => {
        const { session, packets } = await fakeSession();
        session.use(() => {
            const group = new Group({ name: "voices" });
            new Synth("beep", { freq: 440 }, { target: group });
            Group.graph("chain", { gain: 0.8 });
            Bus.audio(2);
            assert.equal(group.server, session.server);
        });
        assert.deepEqual(addrs(packets), ["/group_new", "/synth_new", "/graph_new"]);
        session.close();
    }));

test("fromId adopts an id and sends nothing", () =>
    withCleanDefault(async () => {
        const { session, packets } = await fakeSession();
        const synth = Synth.fromId(1234, "beep", session.server);
        assert.equal(synth.id, 1234);
        assert.equal(synth.defname, "beep");
        assert.deepEqual(packets, [], "adopting an id is not creating a node");
        // It drives the node it names, like any handle.
        synth.set({ freq: 220 });
        assert.deepEqual(addrs(packets), ["/node_set"]);

        // A handle that carries no server falls back to the ambient one.
        session.activate();
        Group.fromId(7).free();
        assert.deepEqual(addrs(packets), ["/node_set", "/node_free"]);
        session.close();
    }));

test("a bare Routine.play() creates and starts the default session's clock", () =>
    withCleanDefault(async () => {
        assert.equal(defaultClock(), null, "never at import");
        let woke = 0;
        const routine = new Routine(function* () {
            for (;;) {
                woke += 1;
                yield 1.0;
            }
        });
        routine.play();

        const clock = defaultClock();
        assert.ok(clock, "the ladder's last rung is created on first use");
        assert.equal(clock.session, main, "and belongs to the default session");
        assert.equal(routine.clock, clock);

        // What `play` returns having done is *arrange* the first wake, not run
        // it: a running clock resumes what it is handed after the scheduling
        // call has returned, which is the ordering the Python client has and
        // what lets `Routine.run` read its own binding. So the clock is what
        // this asserts on the spot, and the wake one turn later.
        assert.equal(woke, 0, "the first pass is not on this call's stack");
        await flush();
        assert.ok(woke >= 1, "started, so the routine has run its first wake");
    }));

test("two sessions are two random contexts", () =>
    withCleanDefault(async () => {
        const a = await fakeSession();
        const b = await fakeSession();
        a.session.seed(1);
        b.session.seed(2);
        const first = a.session.use(() => a.session.rng.nextF64());
        // Seeding b again must not move a's stream.
        b.session.seed(2);
        a.session.seed(1);
        assert.equal(a.session.use(() => a.session.rng.nextF64()), first);
        assert.notEqual(b.session.use(() => b.session.rng.nextF64()), first);
        a.session.close();
        b.session.close();
    }));

test("play dispatches by kind against the ambient context", () =>
    withCleanDefault(async () => {
        const { session, packets } = await fakeSession();
        session.activate();

        // An event, and a plain object of event keys: both a note, now.
        play(new Event({ degree: 0, dur: 0.5 }));
        play({ degree: 2, dur: 0.5 });
        assert.deepEqual(
            addrs(packets).filter((a) => a === "/synth_new").length,
            2,
        );

        // A generator function, scheduled on the ambient clock -- which is now
        // *this session's*, because `activate` makes the whole session ambient
        // (server, clock and random root) the way the reference client's does,
        // where the old adoption lent the server alone and left the clock to
        // the default session's.
        const routine = play(function* () {
            yield 1.0;
        }) as Routine;
        assert.ok(routine instanceof Routine);
        assert.equal(routine.clock, session.clock);

        assert.throws(() => play(42 as never), /don't know how to play/);
        session.close();
    }));

test("a session's clock names it, which is how a routine finds its server", () =>
    withCleanDefault(async () => {
        const a = await fakeSession();
        const b = await fakeSession();
        a.session.activate();

        // The routine runs on b's clock, so what it creates goes to b's
        // server — even though a is the page's default and b is not active.
        const routine = new Routine(function* () {
            new Synth("beep");
            yield 1.0;
        });
        routine.play(b.session.clock);
        b.session.start();
        b.ticker.fire();

        assert.deepEqual(addrs(b.packets), ["/synth_new"]);
        assert.deepEqual(addrs(a.packets), []);
        a.session.close();
        b.session.close();
    }));
