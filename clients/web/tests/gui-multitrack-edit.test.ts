// Editing a **piece**: the picture, the report and the history.
//
// `MultitrackEditor` is the multitrack as one of the fundamental structures —
// which is what gives it the undo every other editor has. What is checked here
// is the seam rather than the mapping: the mapping is the crate's
// (`multitrackProps`/`editingIntake`, the same one the standalone host draws
// and reads with), so what could still be wrong is this client's half — the axis
// a box crosses to, which buffer a source was read into, and whether a report
// that means several edits lands as **one** entry.
//
// The mirror of `clients/python/tests/test_gui_multitrack_edit.py`, case for
// case. Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { multitrackPlan, PiecePlayback, StepRunner } from "../src/core/clausters_core_web.js";
import {
    MultitrackEditor, MultitrackView, Playback, edit,
} from "../src/gui/editing/index.ts";
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
    assert.deepEqual([lanes[0], lanes[7]], ["10", "20"], "named by their track ids");
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
    // time: seven rows named `10`, `one`, `96`, `false`, `false`, `1`, `false`.
    //
    // Found 2026-09-09 reading the two clients against each other, which is the
    // only place it was visible: every test read `view.props` and none read the
    // tree that is actually published.
    const ed = editor(piece());
    const drawn = (ed.draw() as unknown as { children: Record<string, unknown>[] })
        .children.find((c) => c.type === "multitrack")!;
    assert.equal((drawn.lanes as unknown[]).length, 2 * 7, "two tracks, seven numbers each");
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
    // **It arrives on the ruler**, which is where it is placed and nowhere else
    // — so the strip is a named widget of this picture like any other, or the
    // one gesture that places the cursor would land outside the only object
    // that could hear it.
    const view = ed.view as MultitrackView;
    const rid = view.ruler!;
    assert.ok((ed as unknown as { owns(id: number): boolean }).owns(rid), "the ruler is this view's");
    const told: number[] = [];
    ed.onLocate = (beat) => told.push(beat);
    assert.equal(
        (ed as unknown as { route(args: unknown[]): boolean }).route([rid, "locate", 4.0 * SR]),
        false,
        "placing is not an edit",
    );
    near(ed.cursor!, 4.0);
    assert.equal(told.length, 1);
    near(told[0], 4.0);
    // And a piece opens with the reader at the top: the cursor is stated, so
    // there is somewhere to play from before anything is clicked.
    const fresh = editor(piece());
    fresh.draw();
    const start = (fresh.view as MultitrackView).props(fresh, (fresh.view as MultitrackView).ruler!);
    assert.equal(start.cursor, 0.0);
    // Once placed it is reported from the editor's own copy, so a resync does
    // not drag the mark back to the start.
    near(Number(view.props(ed, rid).cursor), 4.0 * SR);
});

/** A host that adopts what it is told, the way the real one does. */
class AdoptingHost {
    names: string[] | null = null;
    rows: string[] | null = null;
    pushes = 0;
    push(_seq: number, corrections: [number, Record<string, unknown>][]): void {
        this.pushes += 1;
        for (const [, props] of corrections) {
            const clips = props.clips as unknown[] | undefined;
            const lanes = props.lanes as unknown[] | undefined;
            if (clips) this.names = clips.filter((_v, i) => i % 7 === 0).map(String);
            if (lanes) this.rows = lanes.filter((_v, i) => i % 7 === 0).map(String);
        }
    }
    ack(): void {}
    set(): void {}
}

function wired(ed: MultitrackEditor): AdoptingHost {
    const host = new AdoptingHost();
    const inner = ed as unknown as {
        app: { host: unknown };
        windowId: number | null;
    };
    // Through the application, which is what an `open` would do: it is the
    // window set that adopts a host, and it hands it to each editor's echo.
    inner.app.host = host;
    inner.windowId = 1;
    ed.draw();
    return host;
}

test("a name the host minted is answered with the one the piece kept", () => {
    // The host makes a **track** from a double click and a **box** from a
    // split, and in both it mints the word while the document mints the id.
    //
    // Until the picture goes back the two are naming the same thing
    // differently, and a name the piece does not know is not ignored — it is
    // read as something *new*. So the next report about that box minted it
    // again, and again after that.
    const held = piece();
    const ed = editor(held);
    const host = wired(ed);
    const wid = [...ed.view!.widgets.keys()][0];
    const ids = () =>
        held.tracks.flatMap((t) => t.lanes.flatMap((l) => l.regions.map((r) => r.id)));
    const apply = (seq: number, tag: string, values: unknown[]) =>
        ed.apply("/gui_event", [
            wid,
            seq,
            (ed as unknown as { version: number }).version,
            tag,
            ...values,
        ]);

    const payload: unknown[] = [];
    for (const box of clips(ed)) {
        if (box[0] === "12") {
            const first = [...box];
            first[3] = 1.0 * SR;
            payload.push(...first);
            payload.push("white 2", box[1], 1.0 * SR, 1.0 * SR, 1.0 * SR, "", box[6]);
        } else {
            payload.push(...box);
        }
    }
    apply(1, "clips", payload);
    const split = ids();
    assert.equal(split.length, 4, "the split landed");
    assert.equal(host.pushes, 1, "and the picture went back with it");
    assert.deepEqual(host.names, split.map(String), "under the ids the piece kept");

    // The next gesture, reported with the names the host was just given.
    const again: unknown[] = [];
    clips(ed).forEach((box, i) => {
        const row = [...box];
        row[0] = host.names![i];
        row[2] = Number(row[2]) + 1000.0;
        again.push(...row);
    });
    apply(2, "clips", again);
    assert.deepEqual(ids(), split, "nothing was minted a second time");

    // And a track made in the host: the same rule.
    const rows = [...(props(ed).lanes as unknown[]), "track 1", "three", 96.0, 0, 0, 1.0, 0];
    apply(3, "lanes", rows);
    assert.deepEqual(host.rows, held.tracks.map((t) => String(t.id)));
    assert.equal(held.tracks.length, 3);
});

test("rewind puts the cursor back at the top", () => {
    // The cursor's own verb. Stop goes back to the **mark** — which is what
    // tells it from pause — so with nothing else the way back to the top is
    // finding beat zero on screen and clicking it.
    const ed = editor(piece());
    ed.cursor = 12.0;
    ed.rewind();
    assert.equal(ed.cursor, 0.0);
});

test("buffer zero is a buffer", () => {
    // The first buffer an allocator hands out is a buffer, and a box over it
    // draws — `|| -1` said it did not, so the first take a page loaded was the
    // one take its boxes could not draw.
    //
    // Found by use 2026-09-10, on the box the example loads first.
    const ed = new MultitrackEditor(piece(), { sampleRate: SR, sources: { 1: { bufnum: 0 } } });
    assert.equal(ed.bridge.sources.bufnum(1), 0);
    assert.ok(clips(ed).every((b) => b[6] === 0), "the boxes over it name it");
    assert.equal(ed.bridge.sources.bufnum(9), -1, "and a source nobody loaded is none");
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
            "10", "", 96.0, 0, 0, 1.0, 0,
            "20", "", 96.0, 1, 0, 0.5, 0,
        ]),
    );
    assert.equal(held.track(20)!.muted, true);
    near(held.track(20)!.level, 0.5);
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

/**
 * `FakeTake` plus the read-back and the option-object write the samples domain
 * actually uses — a `Buffer`'s own shape, since a stand-in with a different one
 * is a test that passes against a client nobody has.
 */
class Take extends FakeTake {
    constructor(n = 16) {
        super();
        this.frames = new Array(n).fill(0.0);
    }

    getSamples(options: { start?: number; count?: number } = {}): number[] {
        const start = options.start ?? 0;
        const end = options.count === undefined ? this.frames.length : start + options.count;
        return this.frames.slice(start, end);
    }

    override setSamples(samples: readonly number[], options: { start?: number } = {}): void {
        const start = options.start ?? 0;
        for (let i = 0; i < samples.length; i += 1) {
            if (start + i < this.frames.length) this.frames[start + i] = Number(samples[i]);
        }
    }
}

/** Lets the samples domain's write queue drain — its writes are a promise chain. */
const settled = () => new Promise((resolve) => setTimeout(resolve, 0));

test("a reopened box undoes its own edit and not the piece's", async () => {
    // **One order, and a leg belonging to the take is still the take's.**
    //
    // Closing a box's window and opening it again builds a *fresh* editor over
    // the same structure; what must not change is which entry an undo in that
    // window steps. The identity is the context's, minted per structure, so the
    // new editor projects the legs the old one recorded.
    //
    // Written 2026-09-09 while diagnosing the same fault in the Python client —
    // an undo in a reopened box's window stepping the multitrack's last edit.
    // It does not reproduce here either.
    const take = new Take();
    const ed = new MultitrackEditor(piece(), { sampleRate: SR, sources: { 1: take } });
    ed.draw();
    const wid = [...ed.view!.widgets.keys()][0];
    const route = (e: unknown, args: unknown[]) =>
        (e as unknown as { route(args: unknown[]): boolean }).route(args);
    assert.ok(route(ed, [wid, "clips", "12", "10", 1.0 * SR, 2.0 * SR, 0.0, "", 7]));

    const box = (await ed.enter("12"))!;
    box.draw();
    const bwid = [...box.view!.widgets.keys()][0];
    assert.ok(route(box, [bwid, "draw", 0, 2, [1.0, 1.0], [0.0, 0.0]]));
    await settled();
    assert.deepEqual(take.frames.slice(2, 4), [1.0, 1.0]);

    box.close();
    ed.entered.delete("12");
    const again = (await ed.enter("12"))!;
    assert.notEqual(again, box);
    again.draw();

    assert.ok(again.undo(), "the stroke is what the pile has on top");
    await settled();
    assert.deepEqual(take.frames.slice(2, 4), [0.0, 0.0], "the stroke is undone");
});

test("a box closed does not block the piece's undo", async () => {
    // **The pile's scope is the context's, not a window's.**
    //
    // An entry names a structure, and what puts an edit back onto one is its
    // *vocabulary* — neither of which is on screen. When the applier was a
    // **view** instead, a box entered from a piece and then closed left an entry
    // nobody could apply: the step was refused, and since a refused step puts
    // the cursor back, the very next undo hit the same entry. The pile was not
    // missing one step, it was **blocked** — every edit the piece had made
    // behind that entry was unreachable until the box was opened again.
    //
    // Found by use 2026-09-12, by hand, in the Python example. The earlier
    // reading of it (2026-09-10) is the one this replaces: the refusal was
    // correct given a view-shaped participant, and the participant was wrong.
    //
    // The Python twin is
    // `test_gui_multitrack_edit.py::test_a_box_closed_does_not_block_the_piece_s_undo`.
    const take = new Take();
    const held = piece();
    const ed = new MultitrackEditor(held, { sampleRate: SR, sources: { 1: take } });
    ed.draw();
    const wid = [...ed.view!.widgets.keys()][0];
    const route = (e: unknown, args: unknown[]) =>
        (e as unknown as { route(args: unknown[]): boolean }).route(args);
    assert.ok(route(ed, [wid, "clips", "12", "10", 1.0 * SR, 2.0 * SR, 0.0, "", 7]));
    const moved = regionAt(held, 12)!.position;
    const box = (await ed.enter("12"))!;
    box.draw();
    const bwid = [...box.view!.widgets.keys()][0];
    assert.ok(route(box, [bwid, "draw", 0, 2, [1.0, 1.0], [0.0, 0.0]]));
    await settled();

    // The box's window goes, and its editor with it.
    box.close();
    ed.entered.delete("12");

    // The stroke is still the top of the pile, and an undo in the **piece's**
    // window performs it: the take and its vocabulary are registered in the
    // context, and neither went with the window.
    assert.equal(ed.undo(), true, "the piece can put back an edit made inside a box");
    await settled();
    assert.deepEqual(take.frames.slice(2, 4), [0.0, 0.0], "and it is the stroke");
    assert.equal(ed.app.unreachable, null);

    // ...and the order keeps going, which is the half that was actually broken:
    // a refused step put the cursor back, so everything behind it was walled off.
    assert.equal(ed.undo(), true, "the entry behind it is reachable");
    assert.notEqual(regionAt(held, 12)!.position, moved, "the box went back");

    // Both come forward again, in order.
    assert.equal(ed.redo(), true);
    assert.equal(regionAt(held, 12)!.position, moved);
    assert.equal(ed.redo(), true);
    await settled();
    assert.deepEqual(take.frames.slice(2, 4), [1.0, 1.0]);
});

/** The region of this id, wherever it sits. */
function regionAt(held: Multitrack, id: number) {
    for (const track of held.tracks) {
        for (const lane of track.lanes) {
            for (const region of lane.regions) if (region.id === id) return region;
        }
    }
    return undefined;
}

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

// ---- what the piece is heard as ----

/**
 * The instance plan for an editor's piece, the way `Playback` asks for it.
 *
 * The plan itself is the crate's and is tested there; what these check is the
 * **crossing** — that this client hands it the piece, the axis and the source
 * table it actually holds, which is the half a client can get wrong on its own.
 */
/**
 * The plan's shape, as much of it as these tests read.
 *
 * Local rather than imported: the plan is the crate's and the Python client
 * reads it as plain data, so a type mirroring it in the package surface was one
 * client restating what the other does not.
 */
interface Plan {
    graph: string;
    channels: number;
    widths: [number, number][];
    tracks: {
        track: number;
        channels: number;
        gain: number;
        mute: number;
        clips: {
            region: number;
            slot: string;
            gain: number;
            mute: number;
            readers: {
                channel: number;
                buffer: number;
                at: number;
                span: number;
                start: number;
                looping: boolean;
            }[];
            curves: { id: number; port: string; at: number; step: number; table: number[] }[];
        }[];
        curves: { id: number; port: string; at: number; step: number; table: number[] }[];
    }[];
}

function plan(ed: MultitrackEditor): Plan {
    return JSON.parse(
        multitrackPlan(
            JSON.stringify(ed.structure.write()),
            ed.bridge.rate,
            ed.bridge.bpm,
            JSON.stringify(ed.bridge.sources.table()),
        ),
    ) as Plan;
}

test("a box is planned in frames from where its window opens", () => {
    // The crossing from the piece to the readers: a box is placed in beats and
    // read in frames, and a trimmed one reads on rather than restarting.
    const ed = editor(piece());
    const region = ed.structure.tracks[0].lanes[0].regions[1];
    region.content = window(1, 0.5, 2.0);
    const reader = plan(ed).tracks[0].clips[1].readers[0];
    assert.equal(reader.buffer, 7, "the buffer the source was read into");
    near(reader.at, 4.0 * SR);            // a beat is a second here
    near(reader.span, 2.0 * SR);
    near(reader.start, 0.5 * SR);         // where the window opens
    assert.equal(reader.looping, false);
});

test("a muted box and an unloaded source are not read", () => {
    // Two different answers: a muted box is planned at nothing, and a box whose
    // source nobody loaded is not planned at all — the second is a piece that
    // arrived without its takes, which is not the same as a silent one.
    const ed = editor(piece());
    const region = ed.structure.tracks[0].lanes[0].regions[0];
    region.muted = true;
    assert.equal(plan(ed).tracks[0].clips[0].mute, 1.0);

    region.muted = false;
    region.content = window(9);        // a source the table has no buffer for
    const planned = plan(ed).tracks[0].clips.map((c) => c.region);
    assert.ok(!planned.includes(region.id));
});

test("the source table carries the width that picks the wiring", () => {
    // A mono take is panned into its track and a stereo one is balanced, so
    // which clip def a box goes in follows from the source's width — and the
    // width is the client's to report, since only it loaded the samples.
    const ed = editor(piece());
    const table = ed.bridge.sources.table();
    assert.equal(table["1"].buffer, 7);
    assert.ok(table["1"].channels >= 1);
    assert.equal(table["9"], undefined, "a source nobody loaded is not in it");
    assert.ok(plan(ed).tracks[0].clips[0].slot.startsWith("clips."));
});

test("the mixer rules reach the plan and a solo silences the rest", () => {
    // The document holds the flags and never reads them: what a track
    // contributes is the mixer's rule, and the mixer is in the crate — so both
    // clients get the same answer instead of each writing one.
    const ed = editor(piece());
    const [one, two] = ed.structure.tracks;
    assert.equal(plan(ed).tracks[0].gain, 1.0, "a track that said nothing is at full");
    assert.equal(plan(ed).tracks[0].mute, 0.0);

    one.level = 0.25;
    near(plan(ed).tracks[0].gain, 0.25);

    one.muted = true;
    assert.equal(plan(ed).tracks[0].mute, 1.0);
    one.muted = false;

    two.soloed = true;
    assert.equal(plan(ed).tracks[0].mute, 1.0, "another track is soloed");
    assert.equal(plan(ed).tracks[1].mute, 0.0);
});

test("a metered track names the buses the host reads", () => {
    // The meters are the playback's and the strip is the host's, so what the
    // widget carries is *where to look*: a lane, the level run and the mark
    // run, and how many channels each is.
    //
    // The host reads those buses itself every frame, which is why a level that
    // moves every block costs no message at all.
    const ed = editor(piece());
    assert.deepEqual(props(ed).meters, [], "a piece nobody plays has no meters");
    (ed as unknown as { playback: unknown }).playback = {
        meters: new Map([[10, [40, 2]]]),
    };
    assert.deepEqual(props(ed).meters, ["10", 40, 42, 2]);
});

test("the playback sends the crate's steps and waits where they say", async () => {
    // **What is left in a client is a socket, and waiting on it.**
    //
    // What a piece needs, the messages that carry it out and how it is played
    // are the crate's (`PiecePlayback`), and so is which reply releases what
    // (`StepRunner`), tested there because they are one implementation for
    // every endpoint. This is the other half: the message a step waits on goes
    // out as the request whose reply is handed back, a barrier included, and a
    // 64-bit sample goes out as one.
    const log: unknown[][] = [];
    const value = (arg: unknown): unknown => (Array.isArray(arg) ? arg[1] : arg);
    const server = {
        sendMsg: (addr: string, ...args: unknown[]) => log.push(["send", addr, ...args]),
        // Answers as a server does: a barrier with its id, a command with its
        // own name and the index it was sent with.
        request: async (addr: string, args: unknown[]) => {
            log.push(["request", addr, ...args]);
            if (addr === "/server_sync") return { addr: "/server_sync.reply", args: args.map(value) };
            return { addr: "/done", args: [addr, ...args.slice(0, 1).map(value)] };
        },
    };
    const playback = Object.create(Playback.prototype) as Playback;
    const held = playback as unknown as {
        server: unknown;
        runner: unknown;
        run: (answer: string) => Promise<void>;
    };
    held.server = server;
    held.runner = new StepRunner();

    await held.run(JSON.stringify({
        steps: [
            { send: { addr: "/buffer_alloc", args: [{ i: 3 }, { i: 2 }, { i: 1 }] } },
            { await: { command: "/buffer_alloc", index: 3 } },
            { send: { addr: "/buffer_setRange", args: [{ i: 3 }, { i: 0 }, { b: [0.5, 1.0] }] } },
            { sync: 1 },
        ],
    }));
    assert.deepEqual(
        log.map((entry) => entry.slice(0, 2)),
        [["request", "/buffer_alloc"], ["send", "/buffer_setRange"], ["request", "/server_sync"]],
        "the fill waits for the allocation",
    );

    const piece = new PiecePlayback(0, true, 8192);
    log.length = 0;
    await held.run(piece.locate(2.0));
    assert.equal(log.length, 1);
    assert.deepEqual(log[0]!.slice(0, 2), ["request", "/transport_locateSample"]);
    assert.deepEqual(
        log[0]![2],
        ["h", BigInt(piece.beatsToSamples(2.0))],
        "a sample rides as 64 bits",
    );
});

