// The arrangement: elements, grouping, flattening and the document bridge.
//
// What the parity vectors already prove — that this layer writes the same
// document and flattens to the same timeline as the Python client — is in
// `form-parity.test.ts`. This suite covers the rest of the layer's own
// behaviour: the temporal character and relation, editing an aggregate by
// handle, what a placement's length trims, what a leaf with no instrument
// contributes, the logical path to a `GraphDef`, and the document round trip
// (ids that stay put, a body this build does not know, a session's table).
//
// Run with `npm test`; this suite needs nothing staged.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Event as SeqEvent } from "../src/seq/event.ts";
import { Timeline } from "../src/seq/timeline.ts";
import {
    ABSTRACT,
    Aggregate,
    Clang,
    Element,
    FrozenSource,
    Generator,
    MIXED,
    PUNCTUAL,
    RELATIVE,
    SEGMENT,
    SIMULTANEOUS,
    SUCCESSIVE,
    Segments,
    Sequence,
    Track,
    Vector,
    docIdOf,
    flatten,
    fromDocument,
    fromSession,
    nextNodeId,
    toDocument,
    toSession,
    toTimeline,
} from "../src/form/index.ts";
import type { SourceLike } from "../src/form/index.ts";

// The flattening crosses beats to seconds through the shared core's time map
// (`TempoMap`), so the wasm has to be up before any of it runs — the same
// requirement the clock has always had, now that the arrangement measures time
// with the same one function rather than a ratio of its own.
await loadCore();

const buffer = (bufnum: number): SourceLike => ({ bufnum });
const note = (midinote: number, dur = 1.0): SeqEvent => new SeqEvent({ midinote, dur });

// ---- the temporal character ----

test("the character comes from which of onset and duration are there", () => {
    assert.equal(new Element(null, 1.0, 2.0).temporalCharacter, SEGMENT);
    assert.equal(new Element(null, 1.0, null).temporalCharacter, PUNCTUAL);
    assert.equal(new Element(null, null, 2.0).temporalCharacter, RELATIVE);
    assert.equal(new Element().temporalCharacter, ABSTRACT);
});

test("a clang takes its length from the event's dur, unless it is given one", () => {
    assert.equal(new Clang(note(60, 1.5)).duration, 1.5);
    assert.equal(new Clang(note(60, 1.5), null, 0.25).duration, 0.25);
});

test("a track with no timeline gets a fresh one", () => {
    const track = new Track();
    assert.ok(track.timeline instanceof Timeline);
    assert.equal([...track.timeline].length, 0);
});

test("a container is not directly playable, and says to render it", () => {
    assert.throws(() => new Aggregate().play({} as never), /use render\(\)/);
});

test("playing a clang delegates to the event it wraps", () => {
    let played: unknown = null;
    const event = { play: (destination: unknown) => (played = destination) };
    const element = new Element(event);
    element.play("here" as never);
    assert.equal(played, "here");
});

// ---- grouping and the temporal relation ----

test("an unknown grouping kind is refused", () => {
    assert.throws(() => new Aggregate(null, "loose" as never), /unknown aggregate kind/);
});

test("an aggregate can be seeded with elements, pairs and triples", () => {
    const piece = new Aggregate([
        new Clang(note(60)),
        [2.0, new Clang(note(62))],
        [4.0, 0.5, new Clang(note(64))],
    ]);
    assert.deepEqual(
        piece.members.map(([offset, dur]) => [offset, dur]),
        [[0.0, null], [2.0, null], [4.0, 0.5]],
    );
});

test("a handle stays valid across other edits", () => {
    const piece = new Aggregate();
    const first = piece.add(new Clang(note(60)), 0.0);
    const second = piece.add(new Clang(note(62)), 1.0);
    piece.add(new Clang(note(64)), 2.0);
    piece.remove(first);
    piece.move(second, 8.0, 2.0);
    assert.equal(piece.length, 2);
    assert.deepEqual(piece.handles[0], second);
    assert.equal(second.offset, 8.0);
    assert.equal(second.length, 2.0);
});

test("the relation of an empty aggregate is nothing at all", () => {
    assert.equal(new Aggregate().temporalRelation(), null);
});

test("members that start and end together are simultaneous", () => {
    const piece = new Aggregate();
    piece.add(new Clang(note(60, 2.0)), 1.0);
    piece.add(new Clang(note(64, 2.0)), 1.0);
    assert.equal(piece.temporalRelation(), SIMULTANEOUS);
});

test("members with no known length are simultaneous only if none has one", () => {
    const piece = new Aggregate();
    piece.add(new Element(null), 0.0);
    piece.add(new Element(null), 0.0);
    assert.equal(piece.temporalRelation(), SIMULTANEOUS);
    piece.add(new Clang(note(60, 1.0)), 0.0);
    assert.equal(piece.temporalRelation(), MIXED);
});

test("members that tile contiguously are successive, and a gap is mixed", () => {
    const piece = new Aggregate();
    piece.add(new Clang(note(60, 1.0)), 0.0);
    piece.add(new Clang(note(62, 2.0)), 1.0);
    assert.equal(piece.temporalRelation(), SUCCESSIVE);
    piece.add(new Clang(note(64, 1.0)), 4.0);
    assert.equal(piece.temporalRelation(), MIXED);
});

test("a placement's length is what the relation is derived from", () => {
    const piece = new Aggregate();
    piece.add(new Clang(note(60, 8.0)), 0.0, 1.0);
    piece.add(new Clang(note(62, 8.0)), 1.0, 1.0);
    assert.equal(piece.temporalRelation(), SUCCESSIVE);
});

// ---- flattening ----

test("nested placement offsets accumulate into absolute beats", () => {
    const inner = new Aggregate();
    inner.add(new Clang(note(60)), 1.0);
    const piece = new Aggregate();
    piece.add(inner, 4.0);
    assert.deepEqual(flatten(piece).map(([beat]) => beat), [5.0]);
});

test("a track's timeline is shifted by where the track is placed", () => {
    const timeline = new Timeline();
    timeline.add(0.5, note(60));
    const piece = new Aggregate();
    piece.add(new Track(timeline), 2.0);
    assert.deepEqual(flatten(piece).map(([beat]) => beat), [2.5]);
});

test("a sequence of elements is laid out successively by their durations", () => {
    const sequence = new Sequence([
        new Clang(note(60, 1.0)),
        new Clang(note(62, 0.5)),
        new Clang(note(64, 2.0)),
    ]);
    assert.deepEqual(flatten(sequence).map(([beat]) => beat), [0.0, 1.0, 1.5]);
});

test("a sequence of sequences advances by what each one reaches", () => {
    // An item that states no length is as long as what it lays down. Read as
    // zero, every member of a `Sequence` of `Sequence`s landed on the first
    // beat — four bars played at once, which is what "the piece is drawn as an
    // unreadable clip" was.
    const bar = (pitch: number) =>
        new Sequence([0, 1, 2, 3].map(() => new Clang(note(pitch, 1.0))));
    assert.deepEqual(
        flatten(new Sequence([bar(60), bar(64)])).map(([beat]) => beat),
        [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
    );
});

test("a sequence lays a muted member out where it would have been", () => {
    // Mute says what is heard, never where anything is: silencing one member
    // must not pull the ones after it forward.
    const quiet = new Sequence([new Clang(note(60, 1.0)), new Clang(note(62, 1.0))]);
    quiet.mute = true;
    assert.deepEqual(
        flatten(new Sequence([quiet, new Clang(note(64, 1.0))])).map(([beat]) => beat),
        [2.0],
    );
});

test("a flattened timeline comes out sorted", () => {
    const piece = new Aggregate();
    piece.add(new Clang(note(60)), 4.0);
    piece.add(new Clang(note(62)), 1.0);
    assert.deepEqual([...toTimeline(piece)].map(([beat]) => beat), [1.0, 4.0]);
});

test("an abstract element contributes context and no event", () => {
    assert.deepEqual(flatten(new Element()), []);
});

test("a placement's length trims what the element plays", () => {
    // A track of four one-beat notes, placed with a two-beat length: the last
    // two fall outside it and are not played.
    const timeline = new Timeline([
        [0.0, note(60)],
        [1.0, note(61)],
        [2.0, note(62)],
        [3.0, note(63)],
    ]);
    const lane = new Aggregate();
    const member = lane.add(new Track(timeline), 0.0, 2.0);
    assert.deepEqual(flatten(lane).map(([beat]) => beat), [0.0, 1.0]);

    // Lengthened, the whole track sounds again.
    lane.move(member, 0.0, 4.0);
    assert.equal(flatten(lane).length, 4);

    // A take shortened by its placement sounds for exactly that long.
    const take = new Vector(buffer(1), null, 4.0, { instrument: "sampler" });
    const song = new Aggregate([[0.0, 1.5, take]]);
    const flat = flatten(song);
    assert.equal(flat.length, 1);
    assert.equal((flat[0]?.[1] as SeqEvent).get("dur"), 1.5);
    assert.equal(take.toEvent().get("dur"), 4.0);
});

test("trimming copies the event rather than rewriting the element's own", () => {
    const clang = new Clang(note(60, 4.0));
    const piece = new Aggregate();
    piece.add(clang, 0.0, 1.0);
    flatten(piece);
    assert.equal((clang.wraps as SeqEvent).get("dur"), 4.0);
});

test("a buffer with no instrument is structure and has no sound of its own", () => {
    const data = new Vector(buffer(3), null, 2.0);
    assert.deepEqual(flatten(data), []);
    const sounding = new Vector(buffer(3), null, 2.0, { instrument: "take" });
    assert.equal(flatten(sounding).length, 1);
    assert.equal((flatten(sounding)[0]?.[1] as SeqEvent).get("buf"), 3);
});

test("a frozen generator draws and emits nothing", () => {
    const frozen = new Generator("melody", null, 4.0, { name: "melody" });
    assert.deepEqual(flatten(frozen), []);
    assert.equal(frozen.duration, 4.0);
});

test("a logical aggregate is not flattened", () => {
    const logical = new Aggregate(null, "logical", { name: "chain" });
    assert.throws(() => flatten(logical), /rendered as a GraphDef/);
});

// ---- locatable ----

test("a resident generator has no position, and takes its aggregate with it", () => {
    const drone = new Generator("drone");
    drone.resident = true;
    assert.equal(drone.locatable, false);
    const piece = new Aggregate();
    piece.add(new Clang(note(60)), 0.0);
    assert.equal(piece.locatable, true);
    piece.add(drone, 1.0);
    assert.equal(piece.locatable, false);
});

// ---- the logical path ----

test("a logical aggregate translates to the wired GraphDef", () => {
    const chain = new Aggregate(null, "logical", { name: "chain", buses: ["send"] });
    chain.add(new Generator("osc", null, null, { controls: { out: "send", freq: 220 } }));
    chain.add(new Generator("filter", null, null, { controls: { in: "send", out: "OUT" } }));
    const spec = chain.toGraphdef().spec();
    assert.equal(spec.name, "chain");
    assert.deepEqual(spec.buses, [{ name: "send", rate: "audio", channels: 1 }]);
    assert.equal(spec.members.length, 2);
    // A bus reference serializes to the bus name, as the Python client's does.
    assert.deepEqual(spec.members[0], {
        def: "osc",
        controls: { out: "send", freq: 220 },
    });
});

test("a logical aggregate needs a name to become a GraphDef", () => {
    const chain = new Aggregate(null, "logical");
    chain.add(new Generator("osc"));
    assert.throws(() => chain.toGraphdef(), /needs a name/);
});

test("declaring a bus is idempotent by name", () => {
    const chain = new Aggregate(null, "logical", { name: "chain" });
    chain.declareBus("send");
    chain.declareBus("send", "control", 2);
    assert.deepEqual(chain.busNames, ["send"]);
    chain.add(new Generator("osc"));
    assert.deepEqual(chain.toGraphdef().spec().buses, [
        { name: "send", rate: "control", channels: 2 },
    ]);
});

test("a logical member that is not a generator is refused by kind", () => {
    const chain = new Aggregate(null, "logical", { name: "chain" });
    chain.add(new Clang(note(60)));
    assert.throws(() => chain.toGraphdef(), /must be a Generator/);
});

test("a generator names its def by string or by object", () => {
    assert.equal(new Generator("osc").defName, "osc");
    assert.equal(new Generator({ name: "osc" }).defName, "osc");
});

// ---- the document bridge ----

function aComposition(): Aggregate {
    const piece = new Aggregate(null, "concrete", { name: "piece" });
    piece.add(new Clang(note(60)), 0.0, 1.0);
    piece.add(new Vector(buffer(100), null, 4.0, { instrument: "take" }), 2.0, 4.0);
    return piece;
}

test("a composition round trips through the document", () => {
    const back = fromDocument(toDocument(aComposition()));
    assert.ok(back instanceof Aggregate);
    assert.equal(back.name, "piece");
    assert.deepEqual(toDocument(back), toDocument(aComposition()));
});

test("a track comes back a track and not an aggregate of clangs", () => {
    const timeline = new Timeline();
    timeline.add(0.0, note(48));
    timeline.add(1.5, note(55, 0.5));
    const piece = new Aggregate();
    piece.add(new Track(timeline, null, null, { name: "bass" }), 0.0);
    const back = fromDocument(toDocument(piece)) as Aggregate;
    const track = back.members[0]?.[2] as Track;
    assert.ok(track instanceof Track);
    assert.equal(track.name, "bass");
    assert.deepEqual([...track.timeline].map(([beat]) => beat), [0.0, 1.5]);
});

test("a note in a roll keeps its id across a save", () => {
    const timeline = new Timeline();
    timeline.add(0.0, note(48));
    const piece = new Aggregate();
    piece.add(new Track(timeline), 0.0);
    const document = toDocument(piece);
    const back = fromDocument(document) as Aggregate;
    const track = back.members[0]?.[2] as Track;
    const [, item] = [...track.timeline][0] as [number, unknown];
    const node = document.root.members[0].node.members[0].node;
    assert.equal(docIdOf(item), node.id);
});

test("ids are stable across two conversions, and a new element takes a fresh one", () => {
    const piece = aComposition();
    const first = toDocument(piece);
    piece.add(new Clang(note(72)), 8.0);
    const second = toDocument(piece);
    assert.deepEqual(
        second.root.members.slice(0, 2).map((m: any) => m.node.id),
        first.root.members.map((m: any) => m.node.id),
    );
    const added = second.root.members[2].node.id;
    assert.ok(!first.root.members.some((m: any) => m.node.id === added));
});

test("the next id an editor mints is past every id the tree holds", () => {
    const piece = aComposition();
    const document = toDocument(piece);
    const highest = Math.max(
        document.root.id,
        ...document.root.members.map((m: any) => m.node.id),
    );
    assert.equal(nextNodeId(piece), highest + 1);
});

test("a generator travels by reference and comes back frozen, or resolved", () => {
    const piece = new Aggregate();
    piece.add(new Generator("melody", null, 4.0, { name: "melody" }), 0.0);
    const document = toDocument(piece);
    assert.equal(document.root.members[0].node.config.generator, "melody");

    const frozen = fromDocument(document) as Aggregate;
    assert.equal((frozen.members[0]?.[2] as Generator).wraps, "melody");

    const pattern = { name: "melody" };
    const resolved = fromDocument(document, {
        resolve: (kind, config) =>
            kind === "generator" && config.generator === "melody" ? pattern : null,
    }) as Aggregate;
    assert.equal((resolved.members[0]?.[2] as Generator).wraps, pattern);
});

test("an unnamed leaf is written with no reference rather than an address", () => {
    const piece = new Aggregate();
    piece.add(new Generator({ some: "pattern" }, null, 1.0), 0.0);
    const config = toDocument(piece).root.members[0].node.config;
    assert.equal(config, undefined);
});

test("a body this build does not know is carried rather than dropped", () => {
    const document = {
        version: 1,
        root: {
            id: 1,
            kind: "aggregate",
            grouping: "concrete",
            members: [
                {
                    offset: 1.0,
                    node: { id: 2, kind: "constellation", duration: 2.0, stars: 7 },
                },
            ],
        },
    };
    const back = fromDocument(document);
    const again = toDocument(back);
    assert.deepEqual(again.root.members[0].node, document.root.members[0].node);
});

test("a buffer the process does not hold comes back frozen and re-converts", () => {
    const document = toDocument(aComposition());
    const back = fromDocument(document) as Aggregate;
    const take = back.members[1]?.[2] as Vector;
    assert.ok(take.buffer instanceof FrozenSource);
    assert.equal(take.buffer.bufnum, 100);
    assert.deepEqual(toDocument(back).root.members[1], document.root.members[1]);
});

test("a leaf that references its source may be placed twice", () => {
    const take = new Vector(buffer(3), null, 1.0, { instrument: "take" });
    const piece = new Aggregate();
    piece.add(take, 0.0, 1.0);
    piece.add(take, 4.0, 1.0);
    const document = toDocument(piece);
    const [first, second] = document.root.members;
    assert.notEqual(first.node.id, second.node.id);
});

test("an element whose data is in the node is not placed twice", () => {
    const clang = new Clang(note(60));
    const piece = new Aggregate();
    piece.add(clang, 0.0);
    piece.add(clang, 4.0);
    assert.throws(() => toDocument(piece), /placed more than once/);
});

test("a generator's last rendered result round trips as ordinary tree", () => {
    const rendered = new Aggregate();
    rendered.add(new Clang(note(72, 0.25)), 0.0, 0.25);
    const piece = new Aggregate();
    piece.add(new Generator("melody", null, 4.0, { name: "melody", rendered }), 0.0);
    const back = fromDocument(toDocument(piece)) as Aggregate;
    const generator = back.members[0]?.[2] as Generator;
    assert.ok(generator.rendered instanceof Aggregate);
    assert.equal(generator.rendered.members.length, 1);
});

test("a session carries the document and its source table, both ways", () => {
    const piece = new Aggregate();
    piece.add(new Vector(buffer(100), null, 4.0, { instrument: "take" }), 0.0, 4.0);
    const source = { location: "takes/one.wav", lifetime: "session", generation: 0 };
    const session = toSession(piece, { sources: { 100: source } });
    const { element, sources } = fromSession(session);
    assert.deepEqual(sources.get(100), source);
    assert.deepEqual(toDocument(element), session.document);
});

test("a session whose table does not cover its document is refused", () => {
    const piece = new Aggregate();
    piece.add(new Vector(buffer(100), null, 4.0, { instrument: "take" }), 0.0, 4.0);
    assert.throws(() => toSession(piece, { sources: {} }), /no entry for 100/);
});

test("a newer session format is refused rather than half read", () => {
    const piece = new Aggregate();
    const session = toSession(piece);
    assert.throws(
        () => fromSession({ ...session, format: session.format + 1 }),
        /newer than this build reads/,
    );
});

test("several windows read as one keep their own frames", () => {
    const joined = new Segments(
        [
            [buffer(7), 0.0, 1.0],
            [buffer(8), 22050.0, 1.5],
        ],
        null,
        null,
        { instrument: "take" },
    );
    assert.equal(joined.duration, 2.5);
    assert.deepEqual(joined.placed().map(([offset]) => offset), [0.0, 1.0]);
    const back = fromDocument(toDocument(joined)) as Segments;
    assert.deepEqual(
        back.segments.map((seg) => [seg.buffer.bufnum, seg.start, seg.duration]),
        [[7, 0.0, 1.0], [8, 22050.0, 1.5]],
    );
});
