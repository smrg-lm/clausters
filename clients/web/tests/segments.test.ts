// Windows onto material, and runs of them: the arithmetic a cut and a join are.
//
// The structure is general (`../src/segments.ts`) and the two kinds differ only
// in what the base cannot know: how a position advances by a length, and what
// one window holds. So the same checks are made twice, over samples and over
// notes, and where they disagree the disagreement is the point.
//
// The twin of `clients/python/tests/test_segments.py`: same calls, same order.

import assert from "node:assert/strict";
import test from "node:test";

import { OscItem, Timeline } from "../src/seq/timeline.ts";
import { BufferSegments, NoteSegments, Segment } from "../src/segments.ts";

/** A buffer as a run reads one: a slot number and the rate its frames are
 * measured at. */
function fakeBuffer(bufnum: number, sampleRate = 48000.0, frames = 480000) {
    return { bufnum, sampleRate, frames };
}

test("a run is as long as its windows and says where each one starts", () => {
    const run = new BufferSegments([
        [fakeBuffer(1), 0.0, 2.0],
        [fakeBuffer(2), 4800.0, 1.5],
    ]);
    assert.equal(run.total, 3.5);
    assert.deepEqual(run.placed().map(([offset]) => offset), [0.0, 2.0]);
    assert.equal(run.unit, "seconds");
});

test("a cut inside a window opens the second half where the first stopped", () => {
    // The bridge the base cannot cross on its own: the halves' lengths are in
    // seconds and the frame they open at is in frames, a sample rate apart.
    const buffer = fakeBuffer(1, 100.0);
    const [head, tail] = new BufferSegments([[buffer, 0.0, 2.0]]).cut(0.5);
    assert.deepEqual(head.segments.map((s) => [s.start, s.duration]), [[0.0, 0.5]]);
    assert.deepEqual(tail.segments.map((s) => [s.start, s.duration]), [[50.0, 1.5]]);
});

test("a cut falling between windows takes whole windows", () => {
    const run = new BufferSegments([
        [fakeBuffer(1), 0.0, 2.0],
        [fakeBuffer(2), 0.0, 1.0],
    ]);
    const [head, tail] = run.cut(2.0);
    assert.equal(head.length, 1);
    assert.equal(tail.length, 1);
    assert.equal(head.segments[0].source.bufnum, 1);
    assert.equal(tail.segments[0].source.bufnum, 2);
});

test("a cut past the end gives the whole run and an empty one", () => {
    const run = new BufferSegments([[fakeBuffer(1), 0.0, 2.0]]);
    const [head, tail] = run.cut(9.0);
    assert.equal(head.total, 2.0);
    assert.equal(tail.length, 0);
});

test("joining the halves gives the run back", () => {
    const run = new BufferSegments([[fakeBuffer(1), 0.0, 2.0]], { instrument: "take" });
    const [head, tail] = run.cut(0.75);
    const rejoined = head.joined(tail) as BufferSegments;
    assert.equal(rejoined.total, run.total);
    // The configuration travels with the run, or a join would silence it.
    assert.equal(rejoined.instrument, "take");
});

test("a run of notes measures in beats and has nothing to bridge", () => {
    const timeline = new Timeline();
    for (const beat of [0.0, 1.0, 2.0, 3.0]) timeline.add(beat, new OscItem("/n", beat));
    const run = new NoteSegments([[timeline, 0.0, 4.0]]);
    assert.equal(run.unit, "beats");
    const [head, tail] = run.cut(1.5);
    assert.deepEqual(head.segments.map((s) => [s.start, s.duration]), [[0.0, 1.5]]);
    assert.deepEqual(tail.segments.map((s) => [s.start, s.duration]), [[1.5, 2.5]]);
});

test("a note window hides what it leaves out and places the rest at zero", () => {
    const timeline = new Timeline();
    for (const beat of [0.0, 1.0, 2.0, 3.0]) timeline.add(beat, new OscItem("/n", beat));
    const [, tail] = new NoteSegments([[timeline, 0.0, 4.0]]).cut(1.5);
    // The window opens at beat 1.5, so the notes it holds are the last two and
    // they are placed from the run's own start -- and the ones it left out are
    // in the timeline, not gone.
    assert.deepEqual((tail as NoteSegments).items().map(([beat]) => beat), [0.5, 1.5]);
    assert.equal(timeline.length, 4);
});

test("a segment reads a triple, a pair or itself", () => {
    const buffer = fakeBuffer(1);
    assert.equal(Segment.of([buffer, 3.0]).start, 0.0);
    assert.equal(Segment.of([buffer, 2.0, 3.0]).start, 2.0);
    const one = new Segment(buffer, 1.0, 1.0);
    assert.equal(Segment.of(one), one);
});
