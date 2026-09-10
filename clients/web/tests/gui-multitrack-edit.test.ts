// Editing a **piece**: the picture, the report and the history.
//
// `MultitrackEditor` is the multitrack as one of the fundamental structures —
// which is what gives it the undo every other editor has. What is checked here
// is the seam rather than the mapping: the mapping is the crate's
// (`multitrackPicture`/`multitrackRead`, the same one the standalone host draws
// and reads with), so what could still be wrong is this client's half — the axis
// a box crosses to, which buffer a source was read into, and whether a report
// that means several edits lands as **one** entry.
//
// The mirror of `clients/python/tests/test_gui_multitrack_edit.py`, case for
// case. Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { MultitrackEditor, edit } from "../src/gui/editing/index.ts";
import { Automation, Content, Lane, Multitrack, Region, Tempo, Track } from "../src/multitrack.ts";

await loadCore();

const SR = 48_000.0;

function window(source: number, start = 0.0, duration = 2.0): Content {
    return Content.onto({
        source: { source, lifetime: "session", generation: 0 },
        start,
        duration,
    });
}

/** Two tracks: the first holding two regions, the second one. */
function piece(): Multitrack {
    const held = new Multitrack();
    held.tracks = [
        new Track({
            id: 10,
            name: "one",
            lanes: [
                new Lane({
                    id: 11,
                    regions: [
                        new Region({ id: 12, position: 0.0, length: 2.0, content: window(1) }),
                        new Region({ id: 13, position: 4.0, length: 2.0, content: window(1) }),
                    ],
                }),
            ],
        }),
        new Track({
            id: 20,
            name: "two",
            lanes: [
                new Lane({
                    id: 21,
                    regions: [
                        new Region({ id: 22, position: 0.0, length: 2.0, content: window(1) }),
                    ],
                }),
            ],
        }),
    ];
    return held;
}

/** An editor with no window: what is checked here is the seam, and opening one
 * would need a host. */
function editor(held: Multitrack, sources: Record<number, number> = { 1: 7 }): MultitrackEditor {
    return new MultitrackEditor(held, { sampleRate: SR, sources });
}

/** What the widget is told to draw, without opening a window. */
function props(ed: MultitrackEditor): Record<string, unknown> {
    ed.draw();
    const wid = [...ed.view!.widgets.keys()][0];
    return ed.view!.props(ed, wid) as Record<string, unknown>;
}

function clips(ed: MultitrackEditor): unknown[][] {
    const flat = props(ed).clips as unknown[];
    const out: unknown[][] = [];
    for (let i = 0; i + 7 <= flat.length; i += 7) out.push(flat.slice(i, i + 7));
    return out;
}

/** One `"clips"` report, as the widget would send it. */
function report(ed: MultitrackEditor, boxes: [string, string, number, number][]): boolean {
    ed.draw();
    const wid = [...ed.view!.widgets.keys()][0];
    const values: unknown[] = [];
    for (const [name, row, at, dur] of boxes) values.push(name, row, at, dur, 0.0, "", 7);
    return (ed as unknown as { route(args: unknown[]): boolean }).route([wid, "clips", ...values]);
}

const near = (a: number, b: number, why?: string) =>
    assert.ok(Math.abs(a - b) < 1e-6, why ?? `${a} != ${b}`);

// ---- the picture ----

test("a row per track and a box per region", () => {
    const ed = editor(piece());
    const lanes = props(ed).lanes as unknown[];
    assert.deepEqual([lanes[0], lanes[6]], ["10", "20"], "named by their track ids");
    assert.equal(lanes[1], "one");
    const boxes = clips(ed);
    assert.deepEqual(boxes.map((b) => b[0]), ["12", "13", "22"]);
    assert.deepEqual(boxes.map((b) => b[1]), ["10", "10", "20"]);
    // A beat is a second at the reader's default, so a region at beat 4 is at
    // four seconds' worth of frames.
    near(Number(boxes[1][2]), 4.0 * SR);
    near(Number(boxes[1][3]), 2.0 * SR);
    assert.equal(boxes[0][6], 7, "the buffer its source was read into");
});

test("the widget is told the flat rows and not one row per number", () => {
    // The props are already the wire's, so the node is made from them rather
    // than through the `multitrack` builder — whose `lanes`/`clips` are the
    // *tuples* a page types, and which flattened an already-flat list a second
    // time: six rows named `10`, `one`, `96`, `false`, `false`, `1`.
    //
    // Found 2026-09-09 reading the two clients against each other, which is the
    // only place it was visible: every test read `view.props` and none read the
    // tree that is actually published.
    const ed = editor(piece());
    const drawn = (ed.draw() as unknown as { children: Record<string, unknown>[] })
        .children.find((c) => c.type === "multitrack")!;
    assert.equal((drawn.lanes as unknown[]).length, 2 * 6, "two tracks, six numbers each");
    assert.deepEqual((drawn.lanes as unknown[]).slice(0, 3), ["10", "one", 96.0]);
    assert.equal((drawn.clips as unknown[]).length, 3 * 7, "three regions, seven numbers each");
    assert.deepEqual((drawn.clips as unknown[]).slice(0, 2), ["12", "10"]);
});

test("the piece is ruled from above by a strip of its own", () => {
    // An editor is where a position is read, and the widget draws no ruler — so
    // the view places one above it, on the piece's own axis.
    //
    // The two have to be in **one navigation group**: an unlinked widget is a
    // group of one keyed by itself, so a ruler that joined nothing would pan and
    // zoom away from the lanes it is ruling.
    const children = (editor(piece()).draw() as unknown as {
        children: Record<string, unknown>[];
    }).children;
    const ruler = children[0];
    const pieceNode = children[1];
    assert.equal(ruler.type, "field", "the free-standing time ruler, above");
    assert.equal(pieceNode.type, "multitrack");
    const x = (ruler.axes as { x: Record<string, unknown> }).x;
    assert.equal(x.link, pieceNode.link, "one axis, not two");
    assert.equal(x.unit, "beats");
});

test("the position cursor is kept and told and is not an edit", () => {
    // A click on the time ruler places the **position cursor**, and what arrives
    // is `"locate"` with where it landed.
    //
    // It is where a playback starts and where a paste lands, so the editor keeps
    // it — the playhead is where the *music* is and moves on its own, and an
    // anchor that moved on its own would not be an anchor. It is not an edit and
    // reaches no history.
    const ed = editor(piece());
    ed.draw();
    const wid = [...ed.view!.widgets][0];
    const told: number[] = [];
    ed.onLocate = (beat) => told.push(beat);
    assert.equal(
        (ed as unknown as { route(args: unknown[]): boolean }).route([wid, "locate", 4.0 * SR]),
        false,
        "placing is not an edit",
    );
    near(ed.cursor!, 4.0);
    assert.equal(told.length, 1);
    near(told[0], 4.0);
});

test("a source nobody loaded draws an empty box", () => {
    const ed = editor(piece(), {});
    assert.ok(
        clips(ed).every((b) => b[6] === -1),
        "negative and not zero: buffer 0 is a buffer",
    );
});

test("the piece is placed through its own tempo map", () => {
    // Four beats are not one length: under a tempo that changes they last longer
    // later than earlier, and the picture has to say so.
    const held = piece();
    held.setTempo(new Tempo({ at: 0.0, bpm: 60.0 }));
    held.setTempo(new Tempo({ at: 4.0, bpm: 30.0 }));
    const boxes = clips(editor(held));
    const atFour = boxes.find((b) => b[0] === "13")!;
    near(Number(atFour[2]), 4.0 * SR, "four beats at a beat a second");
    near(Number(atFour[3]), 4.0 * SR, "two beats at half the tempo are four seconds");
});

// ---- the report, and the history ----

test("a move reaches the piece and undoes", () => {
    const held = piece();
    const ed = editor(held);
    assert.ok(report(ed, [
        ["12", "10", 2.0 * SR, 2.0 * SR],
        ["13", "10", 4.0 * SR, 2.0 * SR],
        ["22", "20", 0.0, 2.0 * SR],
    ]));
    near(held.track(10)!.lanes[0].regions[0].position, 2.0);
    assert.ok(ed.undo());
    near(held.track(10)!.lanes[0].regions[0].position, 0.0);
    assert.ok(ed.redo());
    near(held.track(10)!.lanes[0].regions[0].position, 2.0);
});

test("a block move is one entry", () => {
    // A report is the piece, so one message can mean several edits — and they are
    // one thing a hand did, so Ctrl+Z walks back over all of it.
    const held = piece();
    const ed = editor(held);
    assert.ok(report(ed, [
        ["12", "10", 2.0 * SR, 2.0 * SR],
        ["13", "10", 6.0 * SR, 2.0 * SR],
        ["22", "20", 0.0, 2.0 * SR],
    ]));
    // Read through the piece each time: an edit replaces what the piece holds,
    // so a reference taken before one is a reference to what it held then.
    const at = () => held.track(10)!.lanes[0].regions.map((r) => r.position);
    assert.deepEqual(at(), [2.0, 6.0]);
    assert.ok(ed.undo());
    assert.deepEqual(at(), [0.0, 4.0], "both back, in one step");
});

test("a clip that crossed changes track and undoes", () => {
    const held = piece();
    const ed = editor(held);
    assert.ok(report(ed, [
        ["12", "20", 0.0, 2.0 * SR],
        ["13", "10", 4.0 * SR, 2.0 * SR],
        ["22", "20", 0.0, 2.0 * SR],
    ]));
    assert.ok(held.track(20)!.lanes[0].regions.some((r) => r.id === 12));
    assert.ok(ed.undo());
    assert.ok(held.track(10)!.lanes[0].regions.some((r) => r.id === 12));
});

test("a box the piece does not know becomes a region", () => {
    // A split names its halves after the box they came from, which is no region
    // id — and that is how a new box is told from a moved one.
    const held = piece();
    const ed = editor(held);
    assert.ok(report(ed, [
        ["12", "10", 0.0, 1.0 * SR],
        ["12 2", "10", 1.0 * SR, 1.0 * SR],
        ["13", "10", 4.0 * SR, 2.0 * SR],
        ["22", "20", 0.0, 2.0 * SR],
    ]));
    const ids = held.track(10)!.lanes[0].regions.map((r) => r.id);
    assert.equal(ids.length, 3, "the two that stayed and the new one");
    assert.ok(ids.includes(12) && ids.includes(13), "it took an unused id");
    assert.ok(ed.undo());
    assert.equal(held.track(10)!.lanes[0].regions.length, 2);
});

test("a report of what holds is not an edit", () => {
    const held = piece();
    const ed = editor(held);
    assert.equal(report(ed, [
        ["12", "10", 0.0, 2.0 * SR],
        ["13", "10", 4.0 * SR, 2.0 * SR],
        ["22", "20", 0.0, 2.0 * SR],
    ]), false);
    assert.equal(ed.undo(), false, "and nothing was recorded to undo");
});

test("the strip is the piece's and undoes", () => {
    const held = piece();
    const ed = editor(held);
    ed.draw();
    const wid = [...ed.view!.widgets.keys()][0];
    assert.ok(
        (ed as unknown as { route(args: unknown[]): boolean }).route([
            wid, "lanes",
            "10", "", 96.0, 0, 0, 1.0,
            "20", "", 96.0, 1, 0, 0.5,
        ]),
    );
    assert.equal(held.track(20)!.muted, true);
    near(Number((held.track(20)!.config as { level: number }).level), 0.5);
    assert.equal(held.track(10)!.muted, false, "the one nobody touched is untouched");
    assert.ok(ed.undo());
    assert.equal(held.track(20)!.muted, false);
});

test("edit opens a piece", async () => {
    // `edit` dispatches on what the structure is, and a piece is one of the
    // structures it opens now that its picture and its reading are the crate's.
    const ed = await edit(piece(), { sampleRate: SR, open: false });
    assert.ok(ed instanceof MultitrackEditor);
});

test("two windows over one piece walk one stack", () => {
    const held = piece();
    const one = editor(held);
    const two = editor(held);
    assert.ok(report(one, [
        ["12", "10", 2.0 * SR, 2.0 * SR],
        ["13", "10", 4.0 * SR, 2.0 * SR],
        ["22", "20", 0.0, 2.0 * SR],
    ]));
    assert.ok(two.undo(), "the history is the data's, not the window's");
    near(held.track(10)!.lanes[0].regions[0].position, 0.0);
});

// ---- the curves: the light views, in the two places one lives ----

/** The same piece, with a track automation on the first track and an envelope
 * inside its first box. */
function curved(): Multitrack {
    const written = piece();
    written.tracks[0].automation.push(new Automation({
        id: 30,
        name: "gain",
        target: { ctl: "gain", max: 2.0 },
        points: [{ at: 0.0, value: 1.0 },
                 { at: 4.0, value: 0.0, data: { shape: 5, curve: 4.0 } }],
        visible: true,
    }));
    written.tracks[0].lanes[0].regions[0].automation.push(new Automation({
        id: 31,
        name: "env",
        target: { ctl: "amp" },
        points: [{ at: 0.0, value: 0.0 }],
        visible: true,
    }));
    return written;
}

test("a track curve is a row and a box curve is a layer", () => {
    // The same curve in two places, and the place is the whole difference: a
    // track's runs the timeline under its row, a region's is drawn inside its
    // box. So they reach the widget as two props, not one with a flag.
    const drawn = props(editor(curved()));
    const curves = drawn.curves as unknown[];
    const layers = drawn.layers as unknown[];
    assert.deepEqual(curves.slice(0, 3), ["30", "10", "gain"], "the track it is under");
    assert.equal(curves[4], 2.0, "the domain, read out of the target");
    assert.equal(curves.length, 6, "one row, six numbers");
    assert.deepEqual(layers.slice(0, 3), ["31", "12", "env"], "the box it is inside");
    assert.equal(layers.length, 5, "a layer states no height");
});

test("every curve's points travel in one list on this window's axis", () => {
    const flat = props(editor(curved())).points as unknown[];
    const points: unknown[][] = [];
    for (let i = 0; i + 5 <= flat.length; i += 5) points.push(flat.slice(i, i + 5));
    assert.deepEqual(points.map((p) => p[0]), ["30", "30", "31"]);
    // A beat is a second at the reader's default, so the second break-point of
    // `gain` is at four seconds' worth of frames.
    near(Number(points[1][1]), 4.0 * SR);
    assert.equal(points[1][3], 5.0);
    assert.equal(points[1][4], 4.0, "the shape is carried");
});

test("a point dragged is one edit and the curve that did not move is not", () => {
    // The report is every curve there is, so what it means is the difference —
    // and an undo puts the shape back, since the crate carries a point's data
    // without reading it.
    const written = curved();
    const ed = editor(written);
    ed.draw();
    const wid = [...ed.view!.widgets.keys()][0];
    const flat = [...(props(ed).points as unknown[])];
    flat[2] = 0.25; // the first point of `gain`, moved
    assert.ok((ed as unknown as { route(args: unknown[]): boolean })
        .route([wid, "points", ...flat]));
    const gain = () => written.tracks[0].automation.find((a) => a.id === 30)!;
    near(Number(gain().points[0].value), 0.25);
    assert.deepEqual(gain().points[1].data, { shape: 5, curve: 4.0 });
    const env = written.tracks[0].lanes[0].regions[0].automation[0];
    assert.equal(env.points[0].value, 0.0, "the curve nobody touched");

    assert.ok(ed.undo());
    near(Number(gain().points[0].value), 1.0);
});

test("a curve the piece hid is drawn nowhere", () => {
    // Which curves a person had open is part of reopening the piece as they
    // left it, so it is read out of the document rather than kept in the view.
    const written = curved();
    written.tracks[0].automation[0].visible = false;
    assert.equal(props(editor(written)).hidden, "30");
});

// ---- entering a box ----

/** The smallest thing `edit` opens as a take: a buffer number and samples it
 * can write back. */
class FakeTake {
    bufnum = 7;
    channels = 1;
    name = "take";
    frames = [0.0, 0.5, 1.0];

    toSamples(): number[] {
        return [...this.frames];
    }

    setSamples(samples: readonly number[]): void {
        this.frames = [...samples];
    }
}

test("entering a box opens its contents on the piece's history", async () => {
    // The multitrack places; a box is entered to edit. What a box holds is a
    // structure like any other, so entering one is `edit` over that structure —
    // and it is opened on the **piece's** editing context, so one undo order
    // walks both.
    const take = new FakeTake();
    const ed = new MultitrackEditor(piece(), { sampleRate: SR, sources: { 1: take } });
    const opened = await ed.enter("12");
    assert.ok(opened, "the box opened");
    assert.equal(
        (opened as unknown as { editing: unknown }).editing,
        (ed as unknown as { editing: unknown }).editing,
        "one undo order, and it is the piece's",
    );
    // A second double click on the same box raises the one already open.
    assert.equal(await ed.enter("12"), opened);
});

test("a box with nothing to open opens nothing", async () => {
    // A source named by number alone is a box the caller gave no structure for,
    // and a name no region has is no box at all.
    const ed = editor(piece());
    assert.equal(await ed.enter("12"), null, "the source is a bare buffer number");
    assert.equal(await ed.enter("nowhere"), null);
});

test("the windows entered from a piece close with it", async () => {
    // A window entered *from* the piece is part of looking at the piece. What
    // outlives both is the history, which is the data's and was never a
    // window's.
    const take = new FakeTake();
    const ed = new MultitrackEditor(piece(), { sampleRate: SR, sources: { 1: take } });
    const opened = await ed.enter("12");
    ed.close();
    assert.ok(opened!.closed);
    assert.equal(ed.entered.size, 0);
});

test("a gesture that changed the data says so once", () => {
    // The page's door onto an edit: one call per gesture however many edits it
    // took, because that is what a hand did — and a window is not exempt from
    // being told about its own gesture.
    const ed = editor(piece());
    let told = 0;
    ed.onChange = () => {
        told += 1;
    };

    /** One `/gui_event`, through the door the host uses. */
    const gesture = (boxes: [string, string, number, number][]): boolean => {
        ed.draw();
        const wid = [...ed.view!.widgets.keys()][0];
        const values: unknown[] = [];
        for (const [name, row, at, dur] of boxes) values.push(name, row, at, dur, 0.0, "", 7);
        return ed.apply("/gui_event", [wid, 0, 0, "clips", ...values]);
    };

    // A block move: two clips, one gesture.
    assert.ok(gesture([["12", "10", 1.0 * SR, 2.0 * SR],
                       ["13", "10", 5.0 * SR, 2.0 * SR],
                       ["22", "20", 0.0, 2.0 * SR]]));
    assert.equal(told, 1);

    assert.ok(ed.undo());
    assert.equal(told, 2, "a step of the history changed the data too");

    // A report of what already holds is not a change, so nothing is said.
    assert.ok(!gesture([["12", "10", 0.0, 2.0 * SR],
                        ["13", "10", 4.0 * SR, 2.0 * SR],
                        ["22", "20", 0.0, 2.0 * SR]]));
    assert.equal(told, 2);
});

test("a layer's points are its box's own time", () => {
    // A track automation runs the timeline and is measured from the origin; a
    // clip envelope is drawn inside its box and is measured from where that box
    // starts. It is the one thing that differs between the two on the wire.
    const written = piece();
    const late = written.tracks[1].lanes[0].regions[0];
    late.position = 4.0;
    late.automation.push(new Automation({
        id: 40,
        name: "fade",
        visible: true,
        points: [{ at: 0.0, value: 0.0 }, { at: 2.0, value: 1.0 }],
    }));
    written.tracks[0].automation.push(new Automation({
        id: 41,
        name: "gain",
        visible: true,
        points: [{ at: 4.0, value: 0.5 }],
    }));

    const ed = editor(written);
    const flat = props(ed).points as unknown[];
    const points = new Map<string, number[]>();
    for (let i = 0; i + 5 <= flat.length; i += 5) {
        const at = points.get(String(flat[i])) ?? [];
        at.push(Number(flat[i + 1]));
        points.set(String(flat[i]), at);
    }
    assert.deepEqual(points.get("40"), [0.0, 2.0 * SR], "from the box's start");
    assert.deepEqual(points.get("41"), [4.0 * SR], "from the origin");

    // ...and back: a point dragged inside the box comes back as a beat from the
    // box's start, not from the piece's.
    ed.draw();
    const wid = [...ed.view!.widgets.keys()][0];
    const edited = [...flat];
    for (let i = 0; i + 5 <= edited.length; i += 5) {
        if (edited[i] === "40" && edited[i + 1] === 0.0) edited[i + 2] = 0.25;
    }
    assert.ok((ed as unknown as { route(args: unknown[]): boolean })
        .route([wid, "points", ...edited]));
    // The piece is re-read on an edit, so the region is looked up again.
    const fade = written.tracks[1].lanes[0].regions[0].automation[0];
    near(Number(fade.points[0].at), 0.0);
    near(Number(fade.points[0].value), 0.25);
    near(Number(fade.points[1].at), 2.0);
});
