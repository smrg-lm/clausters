// The arrangement model against the Python client's, on the shared vector.
//
// `gen-arrangement-vectors.py` writes out the piece the Rust suite already
// parses -- one definition, three readers: the Python client that built it, the
// crate that defines the format, and this client. Here it is read, asked the
// same questions the Rust test asks, and written back; what comes out must be
// what went in.
//
// Nothing else would notice a divergence. No build reaches either client's call
// sites, so a field spelled `fadeIn` on the way out, a default written when it
// should have been left out, or an unknown field dropped would ship green.
//
// Run with `npm test` (no wasm needed: this module is plain TypeScript).

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { Arrangement, Content, Fade, Lane, Region, Span, Tempo, Track }
    from "../src/arrangement.ts";

const VECTOR = new URL("./arrangement-vectors.json", import.meta.url);

async function vector(): Promise<Record<string, unknown>> {
    return JSON.parse(await readFile(VECTOR, "utf8"));
}

test("the client's arrangement parses and survives a round trip", async () => {
    const written = await vector();
    // Lossless rather than byte-identical: key order in JSON carries no
    // information, and each client writes what it holds in its own order.
    assert.deepEqual(Arrangement.read(written).write(), written);
});

test("the two sides agree about what the piece is", async () => {
    const piece = Arrangement.read(await vector());
    assert.equal(piece.tracks.length, 3);
    assert.equal(piece.end, 48);
    assert.equal(piece.tempoAt(40)?.bpm, 120);
    assert.equal(piece.tempoAt(40)?.ramp, true);
    assert.equal(piece.meterAt(40)?.beats, 7);
    assert.equal(piece.markers.length, 2);
    assert.equal(piece.loopSpan?.length, 32);
    assert.equal(piece.punch?.start, 8);
});

test("a comped track keeps every take and plays the one it names", async () => {
    const piece = Arrangement.read(await vector());
    const vocals = piece.track(10);
    assert.equal(vocals?.lanes.length, 3, "the takes nobody chose are kept");
    assert.equal(vocals?.active, 1);
    assert.equal(vocals?.activeLane?.name, "take 2");
    const sources = vocals?.lanes.flatMap((lane) => lane.regions).map(
        (r) => (r.content.window?.source as Record<string, unknown>).source);
    assert.deepEqual(sources, [100, 101, 102]);
});

test("an overlap keeps its crossfade, its layer and its playrate", async () => {
    const piece = Arrangement.read(await vector());
    const lane = piece.track(30)!.lanes[0];
    assert.ok(lane.regions[0].overlaps(lane.regions[1]));
    assert.equal(lane.regions[0].fadeOut?.length, 4);
    const second = lane.regions[1];
    assert.equal(second.layer, 1, "which one is on top");
    assert.equal(second.muted, true);
    assert.deepEqual(second.fadeIn?.shape, { curve: "exp" });
    assert.equal(second.content.playrate, 1.5);
    assert.deepEqual(second.content.args, { seed: 7 });
});

test("a composite region arrives as the general tree", async () => {
    const piece = Arrangement.read(await vector());
    const region = piece.track(40)!.lanes[0].regions[0];
    assert.equal(region.content.fill, "composite");
    assert.equal(region.content.node?.id, 43);
    assert.equal(region.content.node?.kind, "aggregate");
});

test("an automation curve keeps the shapes neither side reads", async () => {
    const piece = Arrangement.read(await vector());
    const curve = piece.track(30)!.automation[0];
    assert.ok(curve.visible && curve.enabled);
    assert.deepEqual(curve.target, { ctl: "level" });
    assert.deepEqual(curve.points[1].data, { shape: "exp" });
});

test("a field the other client added and this build has no name for survives",
     async () => {
    const piece = Arrangement.read(await vector());
    assert.deepEqual(piece.extra.groove, { name: "mpc60" });
    const region = piece.track(40)!.lanes[0].regions[0];
    assert.deepEqual(region.extra.warp, { mode: "beats" });
});

// ---- the rules this client keeps on its own ----

test("regions that touch do not overlap and regions that share time do", () => {
    const at = (id: number, position: number, length: number) => new Region({
        id, position, length, content: Content.onto({ source: { node: 1 } }),
    });
    assert.equal(at(1, 0, 4).overlaps(at(2, 4, 4)), false);
    assert.equal(at(1, 0, 4).overlaps(at(2, 3, 4)), true);
});

test("placing keeps a lane in position order", () => {
    const lane = new Lane({ id: 10 });
    for (const [id, position] of [[3, 8], [1, 0], [2, 4]]) {
        lane.place(new Region({
            id, position, length: 2, content: Content.onto({ source: { node: 1 } }),
        }));
    }
    assert.deepEqual(lane.regions.map((r) => r.position), [0, 4, 8]);
    assert.equal(lane.end, 10);
});

test("a track spans every lane and plays one", () => {
    const track = new Track({
        id: 1, lanes: [new Lane({ id: 10 }), new Lane({ id: 11, name: "take 2" })],
    });
    const region = (id: number, length: number) => new Region({
        id, position: 0, length, content: Content.onto({ source: { node: 1 } }),
    });
    track.activeLane!.place(region(100, 4));
    track.lanes[1].place(region(200, 16));
    assert.equal(track.activeLane?.id, 10);
    // An alternate take is still part of the piece.
    assert.equal(track.end, 16);
    track.active = 7;
    assert.equal(track.activeLane, undefined);
});

test("two tempos at one beat is a state the map cannot hold", () => {
    const piece = new Arrangement();
    piece.setTempo(new Tempo({ at: 4, bpm: 120 }));
    piece.setTempo(new Tempo({ at: 4, bpm: 90 }));
    assert.equal(piece.tempo.length, 1);
    assert.equal(piece.tempoAt(4)?.bpm, 90);
    // ...and a piece that never said a tempo says nothing: no 120 invented.
    assert.equal(new Arrangement().tempoAt(0), undefined);
});

test("nothing said is nothing written", () => {
    assert.deepEqual(new Arrangement().write(), {});
    const written = new Region({
        id: 1, position: 0, length: 4, content: Content.onto({ source: { node: 1 } }),
    }).write();
    assert.deepEqual(Object.keys(written).sort(),
                     ["content", "id", "length", "position"]);
    assert.equal((written.content as Record<string, unknown>).playrate, undefined);
});

test("a fill this build does not know is carried whole", () => {
    const written = { fill: "video", clip: "take1.mov", offset: 0 };
    assert.deepEqual(Content.read(written).write(), written);
});

test("a half-open span meets the next one without covering a beat twice", () => {
    const first = new Span(0, 8);
    assert.equal(first.length, 8);
    assert.equal(first.end, new Span(8, 16).start);
    assert.equal(new Fade(4).write().length, 4);
});
