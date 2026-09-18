// The multitrack model against the Python client's, on the shared vector.
//
// `gen-multitrack-vectors.py` writes out the multitrack the Rust suite already
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

import { loadCore } from "../src/base/core.ts";

import { Multitrack, Content, Fade, FrozenSource, Lane, LaneView, Region,
         Session, Source, Span, Tempo, Track, TrackView,
         View } from "../src/multitrack.ts";

const VECTOR = new URL("./multitrack-vectors.json", import.meta.url);

async function vector(): Promise<Record<string, unknown>> {
    return JSON.parse(await readFile(VECTOR, "utf8"));
}

test("the client's arrangement parses and survives a round trip", async () => {
    const written = await vector();
    // Lossless rather than byte-identical: key order in JSON carries no
    // information, and each client writes what it holds in its own order.
    assert.deepEqual(Multitrack.read(written).write(), written);
});

test("the two sides agree about what the multitrack is", async () => {
    const multitrack = Multitrack.read(await vector());
    assert.equal(multitrack.tracks.length, 3);
    assert.equal(multitrack.end, 48);
    assert.equal(multitrack.tempoAt(40)?.tempo, 2);
    assert.equal(multitrack.tempoAt(40)?.ramp, true);
    assert.equal(multitrack.meterAt(40)?.beats, 7);
    assert.equal(multitrack.markers.length, 2);
    assert.equal(multitrack.loopSpan?.length, 32);
    assert.equal(multitrack.punch?.start, 8);
});

test("a comped track keeps every take and plays the one it names", async () => {
    const multitrack = Multitrack.read(await vector());
    const vocals = multitrack.track(10);
    assert.equal(vocals?.lanes.length, 3, "the takes nobody chose are kept");
    assert.equal(vocals?.active, 1);
    assert.equal(vocals?.activeLane?.name, "take 2");
    const sources = vocals?.lanes.flatMap((lane) => lane.regions).map(
        (r) => (r.content.window?.source as Record<string, unknown>).source);
    assert.deepEqual(sources, [100, 101, 102]);
});

test("an overlap keeps its crossfade, its layer and its playrate", async () => {
    const multitrack = Multitrack.read(await vector());
    const lane = multitrack.track(30)!.lanes[0];
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
    const multitrack = Multitrack.read(await vector());
    const region = multitrack.track(40)!.lanes[0].regions[0];
    assert.equal(region.content.fill, "composite");
    assert.equal(region.content.node?.id, 43);
    assert.equal(region.content.node?.kind, "aggregate");
});

test("an automation curve keeps the shapes neither side reads", async () => {
    const multitrack = Multitrack.read(await vector());
    const curve = multitrack.track(30)!.automation[0];
    assert.ok(curve.visible && curve.enabled);
    assert.deepEqual(curve.target, { ctl: "level" });
    assert.deepEqual(curve.points[1].data, { shape: "exp" });
});

test("a region carries curves of its own and they are not its track's", async () => {
    // The two places a curve belongs: a track's runs the length of the track and
    // is drawn in a lane beside it, a region's runs the length of the region and
    // is drawn inside it. One type, so one reader.
    const multitrack = Multitrack.read(await vector());
    const region = multitrack.track(30)!.lanes[0].regions[0];
    assert.equal(region.automation[0].id, 35);
    assert.deepEqual(region.automation[0].target, { ctl: "gain" });
    assert.equal(region.automation[0].points.length, 2);
    assert.ok(multitrack.track(30)!.automation[0].id !== region.automation[0].id);
});

test("a field the other client added and this build has no name for survives",
     async () => {
    const multitrack = Multitrack.read(await vector());
    assert.deepEqual(multitrack.extra.groove, { name: "mpc60" });
    const region = multitrack.track(40)!.lanes[0].regions[0];
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
    // An alternate take is still part of the multitrack.
    assert.equal(track.end, 16);
    track.active = 7;
    assert.equal(track.activeLane, undefined);
});

test("the tempo map is where the beats fall over the seconds", async () => {
    // The multitrack is in seconds; the map it holds says where its beats and
    // bars fall, so a script can put a region on a bar -- and it moves nothing.
    await loadCore();
    const multitrack = new Multitrack();
    assert.ok(Math.abs(multitrack.tempoMap().secsAt(3) - 3) < 1e-9, "one beat a second where it states no tempo");
    multitrack.setTempo(new Tempo({ at: 0, tempo: 2 }));
    multitrack.setTempo(new Tempo({ at: 4, tempo: 1 }));
    assert.ok(Math.abs(multitrack.tempoMap().secsAt(6) - 4) < 1e-9);
});

test("two tempos at one beat is a state the map cannot hold", () => {
    const multitrack = new Multitrack();
    multitrack.setTempo(new Tempo({ at: 4, tempo: 2 }));
    multitrack.setTempo(new Tempo({ at: 4, tempo: 1.5 }));
    assert.equal(multitrack.tempo.length, 1);
    assert.equal(multitrack.tempoAt(4)?.tempo, 1.5);
    // ...and a multitrack that never said a tempo says nothing: no 120 invented.
    assert.equal(new Multitrack().tempoAt(0), undefined);
});

test("nothing said is nothing written", () => {
    assert.deepEqual(new Multitrack().write(), {});
    const written = new Region({
        id: 1, position: 0, length: 4, content: Content.onto({ source: { node: 1 } }),
    }).write();
    assert.deepEqual(Object.keys(written).sort(),
                     ["content", "id", "length", "position"]);
    assert.equal((written.content as Record<string, unknown>).playrate, undefined);
});

test("the multitrack carries its own version and keeps it out of an empty file", () => {
    // The counter a stale edit is stale against, and it is the multitrack's rather
    // than the document's: an editor of one is not editing the other. It stays
    // out of the file while it is the first version, so an unedited multitrack still
    // writes an empty object and a file that never named one reads back at it.
    assert.equal(new Multitrack().version, 1);
    assert.equal(new Multitrack().write().version, undefined);
    assert.equal(Multitrack.read({}).version, 1);
    const edited = new Multitrack();
    edited.version = 4;
    assert.equal(edited.write().version, 4);
    assert.equal(Multitrack.read(edited.write()).version, 4);
});

test("a fill this build does not know is carried whole", () => {
    const written = { fill: "video", clip: "take1.mov", offset: 0 };
    assert.deepEqual(Content.read(written).write(), written);
});

test("a half-open span meets the next one without covering an instant twice", () => {
    const first = new Span(0, 8);
    assert.equal(first.length, 8);
    assert.equal(first.end, new Span(8, 16).start);
    assert.equal(new Fade(4).write().length, 4);
});

// ---- the session: the multitrack, and where its samples are ----

const SESSION = new URL("./multitrack-session-vectors.json", import.meta.url);

async function saved(): Promise<Record<string, unknown>> {
    return JSON.parse(await readFile(SESSION, "utf8"));
}

test("the client's session parses and survives a round trip", async () => {
    const written = await saved();
    assert.deepEqual(Session.read(written).write(), written);
});

test("a source table written there reads as sources here", async () => {
    const session = Session.read(await saved());
    assert.equal(session.sources.size, 6);
    const take = session.source(100);
    assert.deepEqual(take?.location, { at: "file", path: "takes/100.wav" });
    assert.equal(take?.lifetime, "session");
    assert.equal(take?.channels, 2);
    assert.equal(take?.sampleRate, 48000);
    // Carried and never interpreted: what produced these samples.
    assert.deepEqual(session.source(200)?.provenance, { def: "sines" });
});

test("a save that cannot promise everything says which part", async () => {
    // The three states a table has to be able to hold, each read back as
    // itself: a file that is there, samples nobody wrote down, and a working
    // copy whose destructive edit is still open.
    const session = Session.read(await saved());
    assert.deepEqual(session.volatile(), [201]);
    assert.deepEqual(session.openEdits(), [300]);
    assert.equal(session.source(300)?.lifetime, "temporary");

    // A save mid-edit promotes the copy and leaves the edit open.
    assert.ok(session.promote(300));
    assert.equal(session.source(300)?.lifetime, "session");
    assert.deepEqual(session.openEdits(), [300], "still undecided, and that is the point");
    assert.ok(session.confirm(300));
    assert.deepEqual(session.openEdits(), []);
});

test("the multitrack inside the session is the same multitrack", async () => {
    const session = Session.read(await saved());
    assert.deepEqual(session.multitrack.write(), await vector());
    assert.deepEqual(session.dangling(), []);
});

test("an absent multitrack reads as an empty one rather than as nothing", () => {
    const session = Session.read({ format: 3 });
    assert.deepEqual(session.multitrack.tracks, []);
    assert.deepEqual(session.write(), { format: 3 });
});

test("a frozen source keeps what the table said", () => {
    const entry = Source.file("take.wav").shaped(2, 480, 48000);
    const frozen = new FrozenSource(700, entry);
    assert.equal(frozen.bufnum, 700);
    assert.equal(frozen.path, "take.wav");
    assert.deepEqual([frozen.channels, frozen.frames, frozen.sampleRate], [2, 480, 48000]);
    assert.equal(new FrozenSource(701).path, undefined);
});

test("a session field a newer writer added survives", () => {
    const written = { format: 3, mixer: { buses: [{ id: 1, name: "reverb" }] } };
    assert.deepEqual(Session.read(written).write(), written);
});

test("a format 2 session opens in seconds", async () => {
    // An older file is read through the crate's migration: its beats go to
    // seconds through the tempo map it saved, and it is written back current.
    await loadCore();
    const old = { format: 2, multitrack: {
        tempo: [{ at: 0.0, bpm: 120.0 }],
        markers: [{ id: 1, at: 8.0 }],
        tracks: [{ id: 1, lanes: [{ id: 2, regions: [{
            id: 3, position: 2.0, length: 4.0,
            content: { fill: "window", window: {
                source: { source: 1, lifetime: "session" },
                start: 0.0, duration: 2.0 } } }] }] }] } };
    const session = Session.read(old);
    assert.equal(session.format, 3);
    assert.equal(session.multitrack.tempo[0]!.tempo, 2.0);
    const region = session.multitrack.tracks[0]!.lanes[0]!.regions[0]!;
    assert.deepEqual([region.position, region.length], [1.0, 2.0]);
    assert.equal(session.multitrack.markers[0]!.at, 4.0);
});

// ---- the presentation: what a window shows of a multitrack ----

function aPiece(): Multitrack {
    const multitrack = new Multitrack();
    const vocals = new Track({ id: 10, lanes: [new Lane({ id: 11 }), new Lane({ id: 12 })] });
    vocals.lanes[0].place(new Region({
        id: 20, position: 0, length: 4, content: Content.onto({ source: { node: 1 } }),
    }));
    multitrack.tracks.push(vocals, new Track({ id: 30, lanes: [new Lane({ id: 31 })] }));
    return multitrack;
}

test("the session carries two views of one multitrack and they disagree on purpose", async () => {
    // The crossing: screen state written by the Python client, parsed by the
    // crate, read back here. A reader that dropped the field would open the
    // same music and lose the window.
    const session = Session.read(await saved());
    assert.equal(session.views.length, 2);

    const arranger = session.views[0];
    assert.equal(arranger.name, "arranger");
    assert.equal(arranger.visible?.length, 48);
    assert.equal(arranger.quant, 4);
    assert.equal(arranger.autofit, true, "the default, and left out of the file");
    assert.deepEqual(arranger.selected, [20, 32]);
    assert.equal(arranger.focused, 20);
    assert.equal(arranger.track(10).height, 96);
    assert.equal(arranger.track(10).lanesShown, true, "comping open");
    assert.equal(arranger.track(30).color, "#4488cc");
    assert.equal(arranger.lane(12).height, 32);
    assert.deepEqual(arranger.extra.fold, "tracks", "a newer window's own state");

    const editor = session.views[1];
    assert.equal(editor.visible?.start, 8);
    assert.equal(editor.quant, 0.25, "the same multitrack, a finer grid");
    assert.equal(editor.autofit, false, "an editor's window is the reader's");
    assert.equal(editor.scroll, 140);
    assert.equal(editor.selection?.length, 4);
    assert.equal(editor.detail, 42);
});

test("a view that says nothing writes an empty object", () => {
    assert.deepEqual(new View().write(), {});
});

test("a view says nothing about what plays", () => {
    // The whole argument for parallel rather than a field on the model.
    const multitrack = aPiece();
    const written = multitrack.write();
    const view = new View();
    view.name = "arranger";
    view.visible = new Span(0, 32);
    view.trackView(10).height = 96;
    const session = new Session();
    session.multitrack = multitrack;
    session.views = [view];
    const back = Session.read(session.write());
    assert.deepEqual(back.multitrack.write(), written);
    assert.equal(back.views[0].track(10).height, 96);
});

test("a track nobody touched reads as the default and costs nothing", () => {
    const view = new View();
    assert.deepEqual(view.track(10), new TrackView());
    assert.deepEqual(view.lane(11), new LaneView());
    assert.equal(view.tracks.size, 0, "asking is not touching");
    view.trackView(10).lanesShown = true;
    assert.equal(view.tracks.size, 1);
});

test("state goes when the thing goes", () => {
    const view = new View();
    view.trackView(10).height = 96;
    view.trackView(999).height = 48;
    view.laneView(11).height = 24;
    view.selected = [20, 777];
    view.focused = 777;
    view.detail = 20;

    assert.equal(view.prune(aPiece()), true);
    assert.deepEqual([...view.tracks.keys()], [10]);
    assert.deepEqual([...view.lanes.keys()], [11]);
    assert.deepEqual(view.selected, [20]);
    assert.equal(view.focused, undefined);
    assert.equal(view.detail, 20);
    assert.equal(view.prune(aPiece()), false, "and pruning twice finds nothing to do");
});

test("a field a newer window wrote survives a load and a save", () => {
    const written = {
        name: "arranger", fold: "tracks",
        tracks: { "10": { height: 96, waveform: "rectified" } },
    };
    const view = View.read(written);
    assert.equal(view.extra.fold, "tracks");
    assert.equal((view.tracks.get(10)!.extra as Record<string, unknown>).waveform,
                 "rectified");
    assert.deepEqual(view.write(), written);
});

test("a session written without views reads back without them", () => {
    const session = new Session();
    session.multitrack = aPiece();
    assert.equal(session.write().views, undefined);
    assert.deepEqual(Session.read(session.write()).views, []);
});
