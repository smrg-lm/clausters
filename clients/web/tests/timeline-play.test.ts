// A timeline plays itself: its own tempo map, children, and the verbs.
//
// Offline, so every onset is an exact second of the score. A timeline plays on
// a clock of its own, born on its beat 0; a child's beats go to seconds through
// its own map; one engine, the root's, wakes the whole tree.
//
// The Python client's `tests/test_timeline_play.py` is the same suite; keep
// them reading alike.

import assert from "node:assert/strict";
import test, { afterEach } from "node:test";

import { loadCore } from "../src/base/core.ts";
import { main } from "../src/base/main.ts";
import { Routine } from "../src/base/stream.ts";
import type { ScoreConnection } from "../src/base/connection.ts";
import { Session } from "../src/session.ts";
import { Event } from "../src/seq/event.ts";
import { OscItem, Timeline } from "../src/seq/timeline.ts";

await loadCore();

afterEach(() => {
    main.currentSession = null;
});

/** Records the score seconds of every bundle, by address. */
function recorder(session: Session): (addr: string) => number[] {
    const seen: [number, string][] = [];
    const score = (session.server.connection as ScoreConnection).score;
    const add = score.add.bind(score);
    score.add = (at: number, packet: Uint8Array) => {
        seen.push([Math.round(at * 1e9) / 1e9, new TextDecoder("latin1").decode(packet)]);
        add(at, packet);
    };
    return (addr) =>
        seen.filter(([, text]) => text.includes(addr + "\0")).map(([at]) => at).sort((a, b) => a - b);
}

async function nrt() {
    const s = (await Session.nrt()).activate();
    return { s, secs: recorder(s) };
}

const render = (s: Session, until = 60) => s.clock.render(until);

/** Runs `action` at `secs` on the session's clock (tempo 1). */
function at(s: Session, secs: number, action: () => void): void {
    s.clock.schedAbs(0, new Routine(function* () {
        yield secs;
        action();
    }));
}

const osc = (addr: string, beats: number[]) =>
    beats.map((b) => [b, new OscItem(addr)] as [number, unknown]);

test("a timeline plays on its own tempo", async () => {
    const { s, secs } = await nrt();
    const tl = new Timeline(osc("/a", [0, 1, 2]), { tempo: 2 });
    tl.play({ destination: s.server });
    render(s);
    assert.deepEqual(secs("/a"), [0, 0.5, 1]);
    assert.ok(tl.finished && !tl.playing);
});

test("a child keeps its own units", async () => {
    const { s, secs } = await nrt();
    const child = new Timeline(osc("/b", [0, 1]), { tempo: 2 });
    const parent = new Timeline(osc("/a", [0, 3]), { tempo: 1 });
    parent.add(2, child);
    parent.play({ destination: s.server });
    render(s);
    assert.deepEqual(secs("/a"), [0, 3]);
    assert.deepEqual(secs("/b"), [2, 2.5]);
});

test("siblings at different tempi start together", async () => {
    const { s, secs } = await nrt();
    const slow = new Timeline(osc("/a", [1]), { tempo: 1 });
    const fast = new Timeline(osc("/b", [2]), { tempo: 2 });
    const parent = new Timeline([], { tempo: 0.5 });
    parent.add(1, slow);
    parent.add(1, fast);
    parent.play({ destination: s.server });
    render(s);
    // Both start at parent beat 1 = second 2; slow's beat 1 and fast's beat 2
    // both fall one second later.
    assert.deepEqual(secs("/a"), [3]);
    assert.deepEqual(secs("/b"), [3]);
});

test("a parent covers its children", () => {
    const child = new Timeline(osc("/b", [4]), { tempo: 2 }); // 2 s long
    const parent = new Timeline(osc("/a", [1]), { tempo: 1 });
    parent.add(2, child);
    assert.ok(Math.abs(parent.duration() - 4) < 1e-9);
    child.loop(0, 1);
    assert.equal(parent.duration(), Infinity);
});

test("one parent per timeline and no cycles", () => {
    const child = new Timeline();
    const a = new Timeline();
    const b = new Timeline();
    a.add(0, child);
    assert.throws(() => b.add(0, child), /copy/);
    assert.throws(() => child.add(0, a), /ancestor/);
    assert.throws(() => a.add(0, a), /ancestor/);
    b.add(0, child.copy());
    a.clear();
    assert.equal(child.parent, null);
    b.add(1, child);
});

test("copy is independent", () => {
    const child = new Timeline(osc("/b", [0]), { tempo: 2 });
    const tl = new Timeline(osc("/a", [0]), { tempo: 1 });
    tl.add(1, child);
    const twin = tl.copy();
    twin.map.push(0, 3);
    assert.equal(tl.map.tempoAt(0), 1);
    const copiedChild = twin.get(1)![1] as Timeline;
    assert.ok(copiedChild !== child && copiedChild.parent === twin);
    assert.equal(twin.get(0)![1], tl.get(0)![1], "stateless items are shared");
});

test("a loop goes on in physical time", async () => {
    const { s, secs } = await nrt();
    const tl = new Timeline(osc("/a", [0, 1])).loop(0, 2);
    tl.play({ destination: s.server });
    at(s, 5.5, () => tl.stop());
    render(s);
    assert.deepEqual(secs("/a"), [0, 1, 2, 3, 4, 5]);
});

test("a loop shorter than a child loops its part", async () => {
    const { s, secs } = await nrt();
    const child = new Timeline(osc("/b", [0, 1, 2, 3]));
    const parent = new Timeline().loop(0, 2);
    parent.add(0, child);
    parent.play({ destination: s.server });
    at(s, 5.5, () => parent.stop());
    render(s);
    // Only the child's beats 0 and 1 are inside the window, pass after pass.
    assert.deepEqual(secs("/b"), [0, 1, 2, 3, 4, 5]);
});

test("locate while playing goes on from there", async () => {
    const { s, secs } = await nrt();
    const tl = new Timeline(osc("/a", [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]));
    tl.play({ destination: s.server });
    at(s, 1.5, () => tl.locate(10));
    render(s);
    assert.deepEqual(secs("/a"), [0, 1, 1.5, 2.5]);
});

test("entering a child in its middle starts at its next onset", async () => {
    const { s, secs } = await nrt();
    const child = new Timeline(osc("/b", [0, 0.5, 1.5]));
    const parent = new Timeline();
    parent.add(2, child);
    parent.play({ at: 3, destination: s.server });
    render(s);
    // Parent beat 3 is the child's beat 1: its onsets at 0 and 0.5 are passed.
    assert.deepEqual(secs("/b"), [0.5]);
});

test("a routine item runs in its timeline's beats and fresh each pass", async () => {
    const { s, secs } = await nrt();
    const pulse = new Routine(function* () {
        for (let i = 0; i < 2; i++) {
            s.server.sendBundle([["/b"]]);
            yield 1;
        }
    });
    const child = new Timeline([[0, pulse]], { tempo: 2 });
    const parent = new Timeline().loop(0, 2);
    parent.add(0, child);
    parent.play({ destination: s.server });
    at(s, 3.9, () => parent.stop());
    render(s);
    assert.deepEqual(secs("/b"), [0, 0.5, 2, 2.5]);
});

test("a routine past the located beat is not recovered", async () => {
    const { s, secs } = await nrt();
    const pulse = new Routine(function* () {
        s.server.sendBundle([["/b"]]);
        yield 0;
    });
    const tl = new Timeline([[1, pulse], [3, new OscItem("/a")]]);
    tl.play({ at: 2, destination: s.server });
    render(s);
    assert.deepEqual(secs("/b"), []);
    assert.deepEqual(secs("/a"), [1]);
});

test("an event's sustain is in its timeline's beats", async () => {
    const { s, secs } = await nrt();
    const child = new Timeline(
        [[0, new Event({ instrument: "default", dur: 1, legato: 1 })]],
        { tempo: 2 },
    );
    const parent = new Timeline();
    parent.add(1, child);
    parent.play({ destination: s.server });
    render(s);
    assert.deepEqual(secs("/synth_new"), [1]);
    assert.deepEqual([...secs("/node_set"), ...secs("/node_free")], [1.5]);
});

test("pause holds and play resumes", async () => {
    const { s, secs } = await nrt();
    const tl = new Timeline(osc("/a", [0, 1, 2, 3]));
    tl.play({ destination: s.server });
    at(s, 1.5, () => tl.pause());
    at(s, 3, () => tl.play());
    render(s);
    assert.ok(Math.abs(tl.position() - 3) < 1e-9);
    // Paused at beat 1.5 (second 1.5), resumed at second 3: beats 2 and 3 are
    // half a beat after it.
    assert.deepEqual(secs("/a"), [0, 1, 3.5, 4.5]);
});

test("stop goes back to where play started", async () => {
    const { s, secs } = await nrt();
    const tl = new Timeline(osc("/a", [0, 1, 2, 3, 4, 5, 6, 7]));
    tl.play({ at: 2, destination: s.server });
    at(s, 1.5, () => tl.stop());
    render(s);
    assert.equal(tl.position(), 2);
    assert.deepEqual(secs("/a"), [0, 1]);
});

test("a resume does not move the mark", async () => {
    const { s } = await nrt();
    const tl = new Timeline(osc("/a", [0, 1, 2, 3, 4, 5, 6, 7]));
    tl.play({ at: 1, destination: s.server });
    at(s, 1.5, () => tl.pause());
    at(s, 2, () => tl.play());
    at(s, 2.5, () => tl.stop());
    render(s);
    assert.equal(tl.position(), 1);
});

test("quant lands on the ambient clock's grid", async () => {
    const { s, secs } = await nrt();
    const tl = new Timeline(osc("/a", [0]), { tempo: 4 });
    s.clock.play(new Routine(function* () {
        yield 0.5;
        tl.play({ quant: 1, destination: s.server });
    }));
    render(s);
    assert.deepEqual(secs("/a"), [1]);
});

test("a tempo change on the map is heard", async () => {
    const { s, secs } = await nrt();
    const tl = new Timeline(osc("/a", [0, 1, 2, 3]));
    tl.map.push(2, 2);
    tl.play({ destination: s.server });
    render(s);
    assert.deepEqual(secs("/a"), [0, 1, 2, 2.5]);
});
