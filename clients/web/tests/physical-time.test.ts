// One physical time, the same live and offline, and `TempoClock.locate`.
//
// Offline, a `LogicalTimebase` stands in for physical time: every clock of the
// run has its origin on it, and a render wakes whatever is due next across all
// of them, in seconds. So a script with several clocks renders as it plays, and
// a locate is the same operation on either time.
//
// The Python client's `tests/test_physical_time.py` is the same suite; keep
// them reading alike.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { TempoClock, manualTicker } from "../src/base/clock.ts";
import { LogicalTimebase, ManualTimebase } from "../src/base/timebase.ts";
import { Routine } from "../src/base/stream.ts";
import type { ScoreConnection } from "../src/base/connection.ts";
import type { Server } from "../src/defs/server/index.ts";
import { Session } from "../src/session.ts";

await loadCore();

const near = (a: number, b: number) => Math.abs(a - b) < 1e-6;

/** `[seconds, address]` of each bundle the session's score receives, in order. */
function recordEmits(session: Session): [number, string][] {
    const out: [number, string][] = [];
    const score = (session.server.connection as ScoreConnection).score;
    const add = score.add.bind(score);
    score.add = (at: number, packet: Uint8Array) => {
        const text = new TextDecoder("latin1").decode(packet);
        for (const addr of ["/a", "/b", "/x"]) {
            if (text.includes(addr + "\0")) out.push([Math.round(at * 1e9) / 1e9, addr]);
        }
        add(at, packet);
    };
    return out;
}

const sorted = (emits: [number, string][]) =>
    [...emits].sort((p, q) => p[0] - q[0] || (p[1] < q[1] ? -1 : 1));

function emitter(server: Server, addr: string, beats: number[]): Routine {
    return new Routine(function* () {
        for (let i = 0; i < beats.length; i++) {
            server.sendBundle([[addr]]);
            if (i + 1 < beats.length) yield beats[i + 1]! - beats[i]!;
        }
    });
}

test("two clocks of one script render together in seconds", async () => {
    const s = await Session.nrt({ tempo: 1 });
    const emits = recordEmits(s);
    const fast = s.adopt(new TempoClock(2));
    assert.equal(fast.timebase, s.clock.timebase, "a session's clocks share its time");
    fast.start();
    s.clock.play(emitter(s.server, "/a", [0, 1, 2]));
    fast.play(emitter(s.server, "/b", [0, 1, 2, 3]));
    s.clock.render();
    assert.deepEqual(sorted(emits), [
        [0, "/a"], [0, "/b"], [0.5, "/b"], [1, "/a"], [1, "/b"], [1.5, "/b"], [2, "/a"],
    ]);
});

test("a clock started at second 4 starts at second 4", async () => {
    const s = await Session.nrt({ tempo: 1 });
    const emits = recordEmits(s);
    s.clock.play(new Routine(function* () {
        yield 4;
        const late = new TempoClock(2).start();
        late.play(emitter(s.server, "/b", [0, 1]));
    }));
    s.clock.render();
    assert.deepEqual(sorted(emits), [[4, "/b"], [4.5, "/b"]]);
});

test("an offline clock that is never started does not play, as live", async () => {
    const s = await Session.nrt({ tempo: 1 });
    const emits = recordEmits(s);
    const idle = s.adopt(new TempoClock(2));
    idle.play(emitter(s.server, "/b", [0, 1]));
    s.clock.play(emitter(s.server, "/a", [0]));
    s.clock.render();
    assert.deepEqual(sorted(emits), [[0, "/a"]]);
    idle.clear();
});

test("a bare clock still renders on a time of its own", async () => {
    const clock = new TempoClock(1, { timebase: new ManualTimebase(0), ticker: manualTicker() });
    const s = await Session.nrt();
    const emits = recordEmits(s);
    clock.play(emitter(s.server, "/a", [0, 2]));
    await Promise.resolve();
    clock.render();
    assert.deepEqual(sorted(emits), [[0, "/a"], [2, "/a"]]);
    assert.ok(!(clock.timebase instanceof LogicalTimebase), "its timebase comes back");
});

test("locate on a stopped clock is where start resumes", () => {
    const timebase = new ManualTimebase(0);
    const clock = new TempoClock(2, { timebase, ticker: manualTicker() });
    clock.locate(6);
    assert.equal(clock.beats(), 6);
    clock.start();
    assert.ok(near(clock.beats(), 6));
    timebase.advance(0.5);
    assert.ok(near(clock.beats(), 7));
    clock.stop();
});

test("locate on a running clock puts the beat on now", () => {
    const timebase = new ManualTimebase(0);
    const clock = new TempoClock(1, { timebase, ticker: manualTicker() }).start();
    timebase.advance(3);
    clock.locate(10);
    assert.ok(near(clock.beats(), 10));
    timebase.advance(1);
    assert.ok(near(clock.beats(), 11));
    clock.stop();
});

test("a frozen clock stays frozen at the located beat", () => {
    const timebase = new ManualTimebase(0);
    const clock = new TempoClock(1, { timebase, ticker: manualTicker() }).start();
    timebase.advance(2);
    clock.freeze();
    clock.locate(5);
    timebase.advance(3);
    assert.ok(near(clock.beats(), 5));
    clock.thaw();
    timebase.advance(1);
    assert.ok(near(clock.beats(), 6));
    clock.stop();
});

test("a locate back from inside a routine keeps physical time going", async () => {
    // A loop by hand: at beat 2 the routine goes back to beat 0 once. Beats
    // repeat; the score's seconds do not.
    const s = await Session.nrt({ tempo: 1 });
    const emits = recordEmits(s);
    const seen: number[] = [];
    s.clock.play(new Routine(function* () {
        let looped = false;
        for (let i = 0; i < 5; i++) {
            seen.push(s.clock.beats());
            s.server.sendBundle([["/x"]]);
            if (s.clock.beats() === 2 && !looped) {
                looped = true;
                s.clock.locate(0);
            }
            yield 1;
        }
    }));
    s.clock.render(10);
    assert.deepEqual(seen, [0, 1, 2, 1, 2]);
    assert.deepEqual(sorted(emits).map(([t]) => t), [0, 1, 2, 3, 4]);
});

test("a locate forward wakes what it passed at once", async () => {
    const s = await Session.nrt({ tempo: 1 });
    const emits = recordEmits(s);
    s.clock.play(new Routine(function* () {
        yield 1;
        s.clock.locate(10);
    }));
    s.clock.schedAbs(5, emitter(s.server, "/a", [0]));
    s.clock.schedAbs(12, emitter(s.server, "/b", [0]));
    s.clock.render();
    // Located at second 1: beat 5 is overdue and wakes then; beat 12 is 2 s on.
    assert.deepEqual(sorted(emits), [[1, "/a"], [3, "/b"]]);
});
