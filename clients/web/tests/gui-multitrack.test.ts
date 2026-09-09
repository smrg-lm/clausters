// `gui.Multitrack` — the piece a `multitrack` widget draws, held here.
//
// No host and no window: a fake widget records what is set on it and hands back
// the event a hand's gesture would have sent. What is checked is that the object
// is the piece — that a report replaces it whole, and that a script never has to
// parse a payload or carry an id.
//
// The same cases the Python client's `test_gui_multitrack.py` checks, in the
// same order, because this is one object in two languages.

import assert from "node:assert/strict";
import test from "node:test";

import { Clip, Lane, Multitrack } from "../src/gui/multitrack.ts";
import type { MultitrackWidget } from "../src/gui/multitrack.ts";

/** Records sets and keeps the handler `attach` registered. */
class FakeWidget implements MultitrackWidget {
    sets: Record<string, unknown>[] = [];
    handler: ((tag: string, ...vals: unknown[]) => void) | null = null;

    onEvent(handler: (tag: string, ...vals: unknown[]) => void): this {
        this.handler = handler;
        return this;
    }

    set(props: Record<string, unknown>): this {
        this.sets.push(props);
        return this;
    }

    /** What the widget would have sent after a hand edited the piece. */
    report(tag: string, ...vals: unknown[]): void {
        assert.ok(this.handler !== null, "nothing subscribed");
        this.handler(tag, ...vals);
    }
}

function piece(): Multitrack {
    return new Multitrack({
        lanes: [["noise"], ["tone"]],
        clips: [["a", "noise", 0.0, 500.0], ["b", "tone", 500.0, 500.0]],
    });
}

test("the tuples a script types become the objects it reads", () => {
    const mt = piece();
    assert.deepEqual(mt.lanes.map((l) => l.name), ["noise", "tone"]);
    assert.equal(mt.lanes[0].height, 96.0);
    assert.equal(mt.lanes[0].gain, 1.0);
    assert.equal(mt.clip("b")?.lane, "tone");
    assert.equal(mt.clip("b")?.end, 1000.0);
    assert.equal(mt.extent, 1000.0, "the furthest end, not the last onset");
    assert.deepEqual(mt.on("noise").map((c) => c.name), ["a"]);
});

test("one subscription carries the whole piece", () => {
    // **The object is the piece.** A gesture reports what the widget now holds,
    // so this replaces the lists -- there is nothing per clip to register, and a
    // script never sees a widget id or parses a payload.
    const mt = piece();
    const w = new FakeWidget();
    const seen: string[] = [];
    mt.onChange = (what) => seen.push(what);
    mt.attach(w);

    // A hand dragged `a` onto the other lane and moved it.
    w.report("clips", "a", "tone", 100.0, 500.0, 0.0, "", -1,
        "b", "tone", 500.0, 500.0, 0.0, "", -1);
    assert.deepEqual(seen, ["clips"]);
    assert.equal(mt.clip("a")?.lane, "tone");
    assert.equal(mt.clip("a")?.at, 100.0);
    assert.deepEqual(mt.on("tone").map((c) => c.name), ["a", "b"]);

    // And a fader is the other payload, which leaves the clips alone.
    w.report("lanes", "noise", "", 96.0, 1, 0, 0.5, "tone", "", 96.0, 0, 0, 1.0);
    assert.deepEqual(seen, ["clips", "lanes"]);
    assert.equal(mt.lane("noise")?.mute, true);
    assert.equal(mt.lane("noise")?.gain, 0.5);
    assert.equal(mt.clips.length, 2, "a lane edit is not a clip edit");
});

test("a change from the script reaches the widget", () => {
    const mt = piece();
    const w = new FakeWidget();
    mt.attach(w);

    mt.place("c", "noise", 1000.0, 200.0);
    assert.equal(w.sets.length, 1);
    assert.ok("clips" in w.sets[w.sets.length - 1]);
    const clips = w.sets[w.sets.length - 1].clips as unknown[][];
    assert.deepEqual(clips[clips.length - 1], ["c", "noise", 1000.0, 200.0, 0.0, "", -1]);

    // `place` is one verb: the piece is a statement, so moving is saying where.
    mt.place("c", "tone", 1200.0, 200.0);
    assert.equal(mt.clips.length, 3);
    assert.equal(mt.clip("c")?.lane, "tone");
    assert.equal(mt.clip("c")?.at, 1200.0);

    mt.mix("noise", { gain: 0.25, mute: true });
    assert.ok("lanes" in w.sets[w.sets.length - 1]);
    const lanes = w.sets[w.sets.length - 1].lanes as unknown[][];
    assert.deepEqual(lanes[0], ["noise", "", 96.0, true, false, 0.25]);
});

test("removing a lane keeps the clips that were on it", () => {
    // Losing them silently is the one thing a removal must not do: they name a
    // lane that is not there, draw nowhere, and come back to be re-homed.
    const mt = piece();
    mt.removeLane("noise");
    assert.deepEqual(mt.lanes.map((l) => l.name), ["tone"]);
    assert.equal(mt.clip("a")?.lane, "noise");
});

test("a partial group is dropped rather than half read", () => {
    const mt = piece();
    const w = new FakeWidget();
    mt.attach(w);
    w.report("clips", "a", "noise", 0.0, 500.0, 0.0, "", -1, "b", "tone", 500.0);
    assert.deepEqual(mt.clips.map((c) => c.name), ["a"]);
});

test("the view is built from what the object holds", () => {
    const mt = new Multitrack({
        lanes: [new Lane("one", "", 60.0)],
        clips: [new Clip("x", "one", 0.0, 10.0)],
        snap: 4.0,
    });
    const spec = mt.view({ name: "piece", weight: 1.0 }) as Record<string, unknown>;
    assert.equal(spec.type, "multitrack");
    assert.deepEqual(spec.lanes, ["one", "", 60.0, 0, 0, 1.0]);
    assert.deepEqual(spec.clips, ["x", "one", 0.0, 10.0, 0.0, "", -1]);
    assert.equal(spec.snap, 4.0);
    assert.equal(spec.name, "piece");
});

test("an unattached piece is still a piece", () => {
    // A script may build one before its window exists; nothing is sent, and
    // nothing raises.
    const mt = piece();
    mt.place("c", "noise", 0.0, 10.0).mix("noise", { gain: 0.5 });
    assert.ok(mt.clip("c") !== null);
    assert.equal(mt.lane("noise")?.gain, 0.5);
});

test("attach takes the window and remembers its own name", () => {
    // `view` is a definition and an id names a live widget, so which opened
    // window is being watched has to be said. The **name** does not: the object
    // built the node, so it knows what it called it.
    const w = new FakeWidget();
    const win = { widget: (name: string) => { assert.equal(name, "piece"); return w; } };

    const mt = piece();
    mt.view({ name: "piece" });
    mt.attach(win);
    w.report("clips", "a", "tone", 7.0, 500.0, 0.0, "", -1);
    assert.equal(mt.clip("a")?.lane, "tone");

    // A view with no name cannot be searched for, and says so.
    const nameless = piece();
    nameless.view({ weight: 1.0 });
    assert.throws(() => nameless.attach(win), /no name/);
});
