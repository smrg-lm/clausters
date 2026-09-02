// The multitrack editor driver (`gui/editor.ts`) — the arrangement↔GuiDef
// bridge, from the edit-back side.
//
// No server and no real host: a fake host records what the editor answers with,
// and the document and its log are the real ones (the crate over wasm), because
// what an edit *becomes* is the crate's decision and half of these cases are
// about exactly that — the snap, the refusal, the inverse.
//
// The forward draw is checked against the Python client's tree in
// `editor-parity.test.ts`; this is the other half: the routes, the registries
// and the history.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import {
    LOGICAL,
    Aggregate,
    Clang,
    Element,
    Generator,
    Track,
    Vector,
    fromDocument,
    toDocument,
} from "../src/form/index.ts";
import { SynthDef, control, in_ as inBus, out, sine } from "../src/defs/index.ts";
import type { Member, SourceLike } from "../src/form/index.ts";
import { Editing } from "../src/gui/editing.ts";
import { Editor } from "../src/gui/editor.ts";
import { TempoMap } from "../src/base/time.ts";
import type { GuiHost, PropValue } from "../src/gui/host.ts";
import { Event as SeqEvent } from "../src/seq/event.ts";
import { pointsToEnv } from "../src/defs/ugens/index.ts";
import { Automation } from "../src/seq/automation.ts";
import { Timeline } from "../src/seq/timeline.ts";
import type { GuiNode } from "../src/gui/guidef.ts";

const here = new URL(".", import.meta.url);
await loadCore();

const SR = 48_000.0;
const TEMPO = 2.0; // beats per second (120 bpm)
const BEAT = SR / TEMPO; // 24000 timeline samples per beat
/**
 * Timeline samples per **second**, the other half of the bridge: an onset is in
 * beats and a length is in the unit of the data it measures, so a take's and a
 * curve's lengths are drawn against this one and no tempo enters them.
 */
const SEC = SR;

/**
 * The stamp every `/gui_event` carries as its second argument. Any non-zero
 * number does: what the editor answers with it is the host's business.
 */
const SEQ = 1;
/**
 * A version of zero: the host cannot say what state the gesture was made
 * against, so the edit applies unchecked. The staleness gate has its own tests.
 */
const UNSTATED = 0;

const buffer = (bufnum: number, secs = 2.0): SourceLike => ({
    bufnum,
    frames: Math.trunc(secs * SR),
    channels: 1,
    sampleRate: SR,
});

/** A two-lane composition: a take on one lane, a melody on another. */
function song(): Aggregate {
    // 96 000 frames is two seconds at 48 kHz, and a take's duration is in
    // seconds -- so this one is as long as its samples, whatever the tempo.
    const take = new Vector(buffer(7), null, 2.0);
    const audio = new Aggregate([[0.0, take]], "concrete", { name: "audio" });
    const melody = new Track(
        new Timeline([
            [0.0, new SeqEvent({ midinote: 60, dur: 1.0 })],
            [1.0, new SeqEvent({ midinote: 64, dur: 1.0 })],
            [2.0, new SeqEvent({ midinote: 67, dur: 2.0 })],
        ]),
    );
    const lead = new Aggregate([[2.0, melody]], "concrete", { name: "lead" });
    return new Aggregate([[0.0, audio], [0.0, lead]], "concrete", { name: "song" });
}

const editor = (element?: Element, options: Record<string, unknown> = {}): Editor =>
    new Editor(element ?? song(), { sampleRate: SR, tempo: TEMPO, ...options });

/** Records what the editor pushes back, so an answer can be read. */
class FakeHost {
    acks: [number, [number, Record<string, PropValue>][]][] = [];
    versions: number[] = [];
    reasons: (string | undefined)[] = [];
    defined = 0;
    private next = 20_000;

    allocId(): number {
        return this.next++;
    }
    open(_tree: GuiNode, _options: unknown = {}): { id: number } {
        return { id: 999 };
    }
    /** The whole trees `define` was handed — one per redefine. */
    defines: GuiNode[] = [];
    define(_id: number, tree: GuiNode): { id: number } {
        this.defined += 1;
        this.defines.push(tree);
        return { id: 999 };
    }
    set(): void {}
    onMessage(): () => void {
        return () => {};
    }
    ack(seq: number, docVersion = 0, _generations: unknown = [], reason?: string): void {
        this.acks.push([seq, []]);
        this.versions.push(docVersion);
        this.reasons.push(reason);
    }
    push(
        seq: number,
        sets: readonly (readonly [number, Record<string, PropValue>])[],
        docVersion = 0,
        _generations: unknown = [],
        reason?: string,
    ): void {
        this.acks.push([seq, sets.map((s) => [s[0], s[1]])]);
        this.versions.push(docVersion);
        this.reasons.push(reason);
    }

    /** Every correction the editor pushed, by widget id. */
    corrections(): Map<number, Record<string, PropValue>> {
        const out = new Map<number, Record<string, PropValue>>();
        for (const [, sets] of this.acks) for (const [id, props] of sets) out.set(id, props);
        return out;
    }
}

const asHost = (host: FakeHost): GuiHost => host as unknown as GuiHost;

// A lane, a clip and the free-standing ruler are one container — a `field` —
// told apart by what is on it: a placement makes it a clip, a bare strip of a
// given thickness is the ruler, everything else is a lane.
const isLane = (n: GuiNode): boolean =>
    n.type === "field" && !("dur" in n) && !("h" in n);
const lanes = (tree: GuiNode): GuiNode[] =>
    ((tree.children ?? []) as GuiNode[]).filter(isLane);
const clipsOf = (lane: GuiNode): GuiNode[] => (lane.children ?? []) as GuiNode[];

/** The payload the host sends when a clip is dragged or resized. */
const clipEvent = (wid: number, offset: number, dur: number, start?: number): unknown[] =>
    start === undefined
        ? [wid, SEQ, UNSTATED, "clip", offset, dur]
        : [wid, SEQ, UNSTATED, "clip", offset, dur, start];

// ---- the unit bridge ----

test("one beat is sampleRate over tempo timeline units", () => {
    const ed = editor();
    assert.equal(ed.unitsPerBeat, BEAT);
    assert.equal(ed.beatsToUnits(2.5), 2.5 * BEAT);
    assert.equal(ed.unitsToBeats(3 * BEAT), 3.0);
});

test("a clip is drawn where the clock plays it across a tempo change", () => {
    // The measured defect: at 48 kHz with the tempo doubled at beat 2, beat 8
    // used to be drawn at 384 000 units — the line reaching it after 8.0 s of
    // wall clock while the clock played it at 5.0 s, three seconds apart.
    //
    // The axis is real time, so the only right answer is the second the clock
    // plays that beat at. Drawing it needs the whole tempo history, not the
    // tempo in force now, which is why an editor holds a map and not a ratio.
    const tempoMap = new TempoMap(1.0);
    tempoMap.push(2.0, 2.0); // doubled at beat 2 (second 2.0)
    const ed = editor(undefined, { tempoMap });
    assert.equal(ed.beatsToUnits(8.0), 5.0 * SR);
    // The beats before the change keep the seconds they actually fell on: it is
    // the tempo *history* that is drawn, not an extrapolation of the last slope.
    assert.equal(ed.beatsToUnits(1.0), 1.0 * SR);
    // And the edit-back inverts it, so a drag still round-trips.
    assert.equal(ed.unitsToBeats(ed.beatsToUnits(8.0)), 8.0);
    // The transport places the sweeping line from the same function, which is
    // what makes the line and the sound agree rather than merely be close.
    assert.equal(ed.transport.beatsToSamples(8.0), 5.0 * SR);
});

test("a length in beats is measured where it sits", () => {
    // A length in beats is not a duration: the same two beats are two seconds at
    // the start and one after the tempo doubles, so the bridge takes the onset
    // and never a beat count alone.
    const tempoMap = new TempoMap(1.0);
    tempoMap.push(4.0, 2.0);
    const ed = editor(undefined, { tempoMap });
    const beatsElement = new Element(new SeqEvent({ freq: 440 }));
    assert.equal(ed.lengthToUnits(2.0, beatsElement, 0.0), 2.0 * SR);
    assert.equal(ed.lengthToUnits(2.0, beatsElement, 4.0), 1.0 * SR);
});

// ---- the forward draw's registries ----

test("every clip registers the placement it came from", () => {
    const ed = editor();
    const tree = ed.draw();
    const ids = lanes(tree).flatMap((lane) => clipsOf(lane).map((c) => c.id as number));
    assert.ok(ids.length > 0);
    // The registry answers for exactly what was drawn.
    for (const id of ids) assert.ok(ed.apply("/gui_event", [id, 0, UNSTATED, "layer", "clip"]) === false);
});

test("a draw is stable across calls", () => {
    const ed = editor();
    assert.deepEqual(ed.draw(), ed.draw());
});

// ---- the edit-back: a dragged clip becomes a placement ----

test("a dragged clip moves the placement, in beats", () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const lead = piece.handles[1]?.element as Aggregate;
    const member = lead.handles[0];

    // Dragged two beats later and resized to three (the host sends samples).
    assert.equal(ed.apply("/gui_event", clipEvent(roll.id as number, 4 * BEAT, 3 * BEAT)), true);
    assert.equal(member?.offset, 4.0);
    assert.equal(member?.dur, 3.0);
});

test("an edit snaps to the musical grid", () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.5 });
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];
    // A hair off a half-beat boundary: the tree gets the grid value.
    ed.apply("/gui_event", clipEvent(roll.id as number, 3.51 * BEAT, 2.0 * BEAT));
    assert.ok(Math.abs((member?.offset ?? 0) - 3.5) < 1e-9);
});

test("a clip inside a placed aggregate converts back through its base", () => {
    // A clip's offset is absolute on the shared axis; a placement is relative to
    // its aggregate. Dragging a clip inside an aggregate that starts at beat 4
    // must move it by the delta, not stamp the absolute position on the member.
    const note = new Clang(new SeqEvent({ midinote: 60, dur: 1.0 }));
    const section = new Aggregate([[1.0, note]], "concrete", { name: "section" });
    const piece = new Aggregate([[4.0, section]], "concrete", { name: "song" });
    const ed = editor(piece);
    const c = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    assert.equal(c.offset, 5 * BEAT); // absolute: 4 + 1

    const member = section.handles[0];
    ed.apply("/gui_event", clipEvent(c.id as number, 6 * BEAT, c.dur as number));
    assert.equal(member?.offset, 2.0); // relative to the section
});

test("moving a clip leaves its length alone", () => {
    // A drag carries the clip's unchanged `dur` along; writing it back (snapped)
    // would silently reshape the element.
    const piece = song();
    const ed = editor(piece, { quant: 1.0 });
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];
    assert.equal(member?.dur, null);

    ed.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, roll.dur as number));
    assert.equal(member?.offset, 5.0);
    assert.equal(member?.dur, null, "untouched by the move");
});

test("draw, apply, draw is a fixed point", () => {
    const ed = editor(song(), { quant: 0.25 });
    const before = ed.draw();
    // Feed every clip its own placement back: nothing moved, so nothing changes.
    for (const lane of lanes(before)) {
        for (const c of clipsOf(lane)) {
            ed.apply("/gui_event", clipEvent(c.id as number, c.offset as number, c.dur as number));
        }
    }
    assert.deepEqual(ed.draw(), before);
});

test("the composition grows when a clip is dragged past the end", () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const before = ed.extent();
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    ed.apply("/gui_event", clipEvent(roll.id as number, 12 * BEAT, 2 * BEAT));
    assert.ok(ed.extent() > before, "the transport asks the arrangement, not a constant");
});

// ---- the notes route ----

test("a note edit rewrites the editable timeline", () => {
    const timeline = new Timeline([[0.0, new SeqEvent({ midinote: 60, dur: 1.0 })]]);
    const piece = new Aggregate([[0.0, new Track(timeline)]], "concrete", { name: "song" });
    const ed = editor(piece);
    const wid = (clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode).id as number;
    // One note, moved a beat later and up a tone.
    assert.equal(
        ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", BEAT, BEAT, 62, 100, 0]),
        true,
    );
    const items = [...timeline];
    assert.equal(items.length, 1);
    assert.equal(items[0]?.[0], 1.0);
    assert.equal((items[0]?.[1] as SeqEvent).get("midinote"), 62);
});

test("a note edit keeps what the roll cannot say and moves only the sustain", () => {
    // Found by ear in the Python client, editing the composer's melody: one
    // dragged note and the whole lane came back fully legato with its notes
    // running together. A roll draws what a note *sounds* (its `sustain`,
    // `dur * legato`) and the edit-back wrote that number into `dur` with a
    // `legato` of 1 -- for every note in the payload, since the payload is the
    // whole lane.
    const timeline = new Timeline([
        [0.0, new SeqEvent({ instrument: "reed", midinote: 60, dur: 1.0, legato: 0.8, amp: 0.4 })],
        [1.0, new SeqEvent({ instrument: "reed", midinote: 64, dur: 1.0, legato: 0.8, amp: 0.4 })],
    ]);
    const piece = new Aggregate([[0.0, new Track(timeline)]], "concrete", { name: "song" });
    const ed = editor(piece);
    const clip = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    const drawn = clip.notes as number[];
    assert.equal(drawn[1], 0.8 * BEAT, "a note is drawn as it sounds");

    // The second note is dragged out to two beats; the first is left alone.
    const edited = [...drawn];
    edited[6] = 2.0 * BEAT;
    assert.equal(
        ed.apply("/gui_event", [clip.id as number, SEQ, UNSTATED, "notes", ...edited]),
        true,
    );

    const [first, second] = [...timeline].map(([, item]) => item as SeqEvent);
    // The note nobody touched is the note it was, in every key.
    assert.equal(first?.get("dur"), 1.0);
    assert.equal(first?.get("legato"), 0.8);
    assert.equal(first?.sustain(), 0.8);
    // The edited one sounds what the hand drew, and keeps the rest.
    assert.equal(second?.sustain(), 2.0);
    assert.equal(second?.get("dur"), 1.0);
    assert.equal(second?.get("instrument"), "reed");
    assert.equal(second?.get("amp"), 0.4);
    // ...and the picture round-trips: what is redrawn is what was sent.
    const again = (clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode).notes as number[];
    assert.deepEqual(again, edited);
});

test("a clip over a generator refuses a note edit, and says why", async () => {
    // A pattern's notes are a *rendering* of a forward-only algorithm.
    const piece = new Aggregate([[0.0, new Clang(new SeqEvent({ midinote: 60, dur: 1.0 }))]],
        "concrete", { name: "song" });
    const ed = editor(piece);
    const host = new FakeHost();
    await ed.open(asHost(host));
    const wid = (clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode).id as number;
    host.acks.length = 0;
    assert.equal(
        ed.apply("/gui_event", [wid, SEQ, UNSTATED, "notes", 0, BEAT, 62, 100, 0]),
        false,
    );
    assert.ok(host.reasons.at(-1), "a refusal with no reason teaches 'sometimes it fails'");
    assert.ok(host.corrections().has(wid), "the notes as they still are went back");
});

test("a layered clip routes a curve edit to the member that carries it", () => {
    // The composer's `sweep` lane: an envelope over a note that is not an
    // editable timeline. The aggregate is not a leaf, so a `configure`
    // addressed to *it* replaced an empty configuration and the crate had
    // nowhere to keep the points — the edit reported success, changed nothing
    // and left no undo behind.
    const env = Automation.fromPoints(
        [[0.0, 200.0, 1, 0.0], [2.0, 900.0, 2, 0.0], [4.0, 300.0, 1, 0.0]],
        null,
        { name: "sweep" },
    );
    const attached = new Aggregate(
        [
            // Eight beats of clang and four seconds of curve: the same stretch
            // at 120 bpm, so the aggregate is simultaneous and draws as one
            // layered clip.
            [0.0, new Clang(new SeqEvent({ instrument: "drone", dur: 8.0 }))],
            [0.0, new Element(env, null, 4.0)],
        ],
        "concrete",
        { name: "sweep" },
    );
    const ed = editor(new Aggregate([[0.0, attached]], "concrete", { name: "song" }));
    const clip = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;

    assert.ok(clip.points, "the curve is drawn");
    // The roll's refusal is the roll's: it does not reach the curve over it.
    assert.equal(clip.notes_editable, 0);
    assert.equal(clip.editable, undefined, "the clip-wide key would lock the curve too");

    assert.equal(
        ed.apply("/gui_event", [
            clip.id as number, SEQ, UNSTATED, "points",
            0.0, 300.0, 1, 0.0,
            2 * SEC, 500.0, 1, 0.0,
            4 * SEC, 100.0, 1, 0.0,
        ]),
        true,
    );
    const points = env.toPoints();
    assert.equal(points[1], 300.0);
    assert.equal(points[5], 500.0);
    // ...and it is an edit like any other, so it is in the history.
    assert.equal(ed.canUndo, true);
    assert.equal(ed.undoLabel, "edit the curve");
    ed.undo();
    assert.equal(env.toPoints()[1], 200.0);
});

test("editing a curve does not move the axis it is drawn against", () => {
    // A break-point's place on screen is its value **against the clip's value
    // axis**, so an axis recomputed from the break-points moves every point
    // whenever one is dragged — the curve jumps under the hand editing it, and
    // the point being dragged is the only one that appears to stay put.
    const env = Automation.fromPoints(
        [[0.0, 200.0, 1, 0.0], [2.0, 900.0, 2, 0.0], [4.0, 300.0, 1, 0.0]],
        null,
        { name: "sweep" },
    );
    const attached = new Aggregate(
        [
            // Eight beats of clang and four seconds of curve: the same stretch
            // at 120 bpm, so the aggregate is simultaneous and draws as one
            // layered clip.
            [0.0, new Clang(new SeqEvent({ instrument: "drone", dur: 8.0 }))],
            [0.0, new Element(env, null, 4.0)],
        ],
        "concrete",
        { name: "sweep" },
    );
    const ed = editor(new Aggregate([[0.0, attached]], "concrete", { name: "song" }));
    const curve = () => clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    const first = curve();
    const axis = [first.points_min, first.points_max];

    const drag = (value: number) => {
        const c = curve();
        assert.equal(
            ed.apply("/gui_event", [
                c.id as number, SEQ, UNSTATED, "points",
                0.0, 200.0, 1, 0.0,
                2 * SEC, value, 2, 0.0,
                4 * SEC, 300.0, 1, 0.0,
            ]),
            true,
        );
        return curve();
    };

    // Up to the ceiling the host clamps a drag to, and back down again.
    for (const value of [axis[1] as number, 400.0, 250.0]) {
        const again = drag(value);
        assert.deepEqual([again.points_min, again.points_max], axis);
    }

    // It **widens** for a curve that no longer fits — a script's edit, an undo
    // of a taller one — because the picture must show the data, and only on the
    // side that stopped holding it.
    env.env = pointsToEnv([0.0, 200.0, 1, 0.0, 2.0, 4000.0, 1, 0.0]);
    ed.refresh();
    const wide = curve();
    assert.equal(wide.points_min, axis[0], "the floor it had is kept");
    assert.ok((wide.points_max as number) > 4000.0, "and the ceiling grew");
});

test("undoing a curve edit tells the host what to draw", async () => {
    // An undo that moves the model and says nothing is a dead button: the host
    // goes on drawing the shape the hand left. The case that needed saying: a
    // **layered** clip draws an aggregate, and the curve an edit configures is a
    // *member* of it — so the widget an undo has to correct is not the one the
    // edited element is registered against.
    const env = Automation.fromPoints(
        [[0.0, 200.0, 1, 0.0], [2.0, 900.0, 2, 0.0], [4.0, 300.0, 1, 0.0]],
        null,
        { name: "sweep" },
    );
    const attached = new Aggregate(
        [
            // Eight beats of clang and four seconds of curve: the same stretch
            // at 120 bpm, so the aggregate is simultaneous and draws as one
            // layered clip.
            [0.0, new Clang(new SeqEvent({ instrument: "drone", dur: 8.0 }))],
            [0.0, new Element(env, null, 4.0)],
        ],
        "concrete",
        { name: "sweep" },
    );
    const ed = editor(new Aggregate([[0.0, attached]], "concrete", { name: "song" }));
    const host = new FakeHost();
    await ed.open(asHost(host));
    const clip = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;

    assert.equal(
        ed.apply("/gui_event", [
            clip.id as number, SEQ, UNSTATED, "points",
            0.0, 300.0, 1, 0.0,
            2 * SEC, 500.0, 1, 0.0,
            4 * SEC, 100.0, 1, 0.0,
        ]),
        true,
    );
    host.acks.length = 0;

    assert.equal(ed.undo(), true);
    assert.equal(env.toPoints()[1], 200.0, "the model stepped back");
    const pushed = host.corrections();
    assert.ok(pushed.has(clip.id as number), "and the clip was told to draw it");
    const props = pushed.get(clip.id as number) as Record<string, PropValue>;
    assert.equal((props.points as number[])[1], 200.0);
});

// ---- the history ----

test("a run of gestures undoes back to where it started", () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];
    const start = member?.offset;

    assert.equal(ed.canUndo, false, "an unedited composition has nothing to undo");
    for (const beats of [4.0, 6.5, 1.0]) {
        assert.equal(ed.apply("/gui_event", clipEvent(roll.id as number, beats * BEAT, 2 * BEAT)), true);
    }
    assert.equal(member?.offset, 1.0);
    assert.equal(ed.canUndo, true);
    assert.equal(ed.undoLabel, "move the clip");

    while (ed.canUndo) assert.equal(ed.undo(), true);
    assert.equal(member?.offset, start, "exactly, not approximately");
    assert.equal(ed.canRedo, true);
});

test("a redo puts the clip back where the undo took it from", () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];

    ed.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    const edited = member?.offset;
    assert.equal(ed.undo(), true);
    assert.notEqual(member?.offset, edited);
    assert.equal(ed.redo(), true);
    assert.equal(member?.offset, edited);
});

test("undo on an untouched editor is false rather than a crash", () => {
    const ed = editor();
    assert.equal(ed.undo(), false);
    assert.equal(ed.redo(), false);
    assert.equal(ed.canUndo, false);
    assert.equal(ed.undoLabel, undefined);
});

test("what the grid did is what gets replayed", () => {
    // The crate records the *effective* edit, so a redo does not snap a second
    // time — harmless with a grid, wrong the moment a rule is not idempotent.
    const piece = song();
    const ed = editor(piece, { quant: 1.0 });
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];

    ed.apply("/gui_event", clipEvent(roll.id as number, 4.3 * BEAT, 2 * BEAT));
    assert.equal(member?.offset, 4.0, "the crate snapped it");
    ed.undo();
    ed.redo();
    assert.equal(member?.offset, 4.0, "and the redo lands on the same beat");
});

test("an undo tells the host what to draw instead, and with the restored value", async () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const drawnAt = roll.offset as number;

    ed.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    host.acks.length = 0;
    assert.equal(ed.undo(), true);
    const corrections = host.corrections();
    assert.ok(corrections.has(roll.id as number), "the undo answered with a value");
    // **And the value is the restored one**: a correction is read out of the
    // drawn registry, so a path that moved the model and left the record behind
    // would tell the host to keep drawing the clip where the hand dropped it.
    assert.equal(corrections.get(roll.id as number)?.offset, drawnAt);
});

test("the window's undo shortcut reaches the history", async () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const host = new FakeHost();
    const win = await ed.open(asHost(host));
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];
    const start = member?.offset;

    ed.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    // Addressed to the **window**, not to a widget: undo is aimed at nothing
    // under the cursor.
    assert.equal(ed.apply("/gui_event", [win.id, SEQ, UNSTATED, "undo"]), true);
    assert.equal(member?.offset, start);
});

// ---- staleness and the acknowledgement ----

test("every acknowledgement carries the version the next gesture names back", async () => {
    const ed = editor(song(), { quant: 0.25 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    assert.equal(host.versions.at(-1), 1, "opening announces the version it drew");
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    ed.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    assert.ok((host.versions.at(-1) as number) > 1, "an edit moves it");
});

test("a drag reporting as it goes is not stale against its own answers", async () => {
    // A host that reports as it goes stamps every step with the version *it*
    // holds, and it only learns a new one when an acknowledgement reaches it —
    // which a hand outruns. Refusing those is refusing the drag: every step
    // comes back as a resync and the picture snaps to the first frame of it.
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    const clip = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];
    const drawnAt = host.versions.at(-1) as number;

    for (const beat of [1.0, 2.0, 3.0, 4.0, 5.0]) {
        assert.equal(
            ed.apply("/gui_event", [clip.id as number, SEQ, drawnAt, "clip", beat * BEAT, 2 * BEAT]),
            true,
        );
    }
    assert.equal(member?.offset, 5.0, "the last frame is where it is");
    assert.notEqual(host.versions.at(-1), drawnAt, "the document moved under the run");

    // A change by no gesture at all raises the floor, so a step arriving after
    // it is refused — which is what the version is for. The offset has to be a
    // *new* one: a step asking for where the clip already sits changes nothing
    // and would answer false whatever the rule said.
    ed.refresh();
    assert.equal(
        ed.apply("/gui_event", [clip.id as number, SEQ + 1, drawnAt, "clip", 7.0 * BEAT, 2 * BEAT]),
        false,
    );
    assert.equal(member?.offset, 5.0, "and it did not move");
});

test("an edit made against a superseded version is refused and answered", async () => {
    const ed = editor(song(), { quant: 0.25 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    const drawn = ed.draw();
    const take = clipsOf(lanes(drawn)[0] as GuiNode)[0] as GuiNode;
    const roll = clipsOf(lanes(drawn)[1] as GuiNode)[0] as GuiNode;
    const version = host.versions.at(-1) as number;
    // The composition moves by a route the host never saw — a script editing
    // the arrangement behind the editor's back and saying so, which is also
    // what a second editor and a redefine look like from in here. Another
    // *gesture* is not that: its versions are ones this host is about to be
    // told about.
    ed.apply("/gui_event", clipEvent(take.id as number, 5 * BEAT, 2 * BEAT));
    ed.refresh();
    host.acks.length = 0;

    // A gesture naming the version *before* all that.
    const stale = [roll.id, SEQ, version, "clip", 7 * BEAT, 2 * BEAT];
    assert.equal(ed.apply("/gui_event", stale), false);
    assert.ok(host.reasons.at(-1), "and it says the composition changed");
    assert.ok(host.corrections().has(roll.id as number), "with the state as it stands");
});

test("two gestures inside one round trip are both applied", async () => {
    // The acknowledgement is not lost, and nothing is saturated: a host stamps
    // every event with the version it was last *told*, and it is told only when
    // an answer arrives. Two gestures begun inside one round trip name the same
    // version, and refusing the second because the first had already moved the
    // composition is refusing a hand for being faster than a poll loop.
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    const drawn = ed.draw();
    const take = clipsOf(lanes(drawn)[0] as GuiNode)[0] as GuiNode;
    const roll = clipsOf(lanes(drawn)[1] as GuiNode)[0] as GuiNode;
    const version = host.versions.at(-1) as number;

    assert.equal(
        ed.apply("/gui_event", [take.id as number, SEQ, version, "clip", 3 * BEAT, 2 * BEAT]),
        true,
    );
    host.acks.length = 0;
    assert.equal(
        ed.apply("/gui_event", [roll.id as number, SEQ + 1, version, "clip", 7 * BEAT, 2 * BEAT]),
        true,
    );
    assert.equal(host.corrections().has(roll.id as number), false, "and no snap back");
});

test("a host that cannot name a version is applied unchecked", () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    ed.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    // Zero is *unstated* rather than a version: the behaviour there was before
    // versions existed.
    assert.equal(ed.apply("/gui_event", clipEvent(roll.id as number, 6 * BEAT, 2 * BEAT)), true);
});

test("a snapped clip is answered with where it actually landed", async () => {
    const ed = editor(song(), { quant: 1.0 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    host.acks.length = 0;
    ed.apply("/gui_event", clipEvent(roll.id as number, 4.3 * BEAT, 2 * BEAT));
    const props = host.corrections().get(roll.id as number);
    assert.ok(props, "the snap moved it, so the host is told");
    assert.equal(props?.offset, 4 * BEAT);
});

test("an event from another editor's window is not answered", async () => {
    const ed = editor();
    const host = new FakeHost();
    await ed.open(asHost(host));
    host.acks.length = 0;
    // A widget this editor never drew.
    assert.equal(ed.apply("/gui_event", [98_765, SEQ, UNSTATED, "clip", 0, BEAT]), false);
    assert.equal(host.acks.length, 0, "answering would retire an edit nobody applied");
});

test("unknown messages are ignored", () => {
    const ed = editor();
    assert.equal(ed.apply("/whatever", [1, 2, 3]), false);
    assert.equal(ed.apply("/gui_event", [1]), false);
});

// ---- windows onto samples: trim, split, join ----

/**
 * A one-lane composition holding one four-**second** take, and its element (a
 * take's length is in seconds, whatever the tempo).
 */
function takeSong(): [Aggregate, Vector] {
    const take = new Vector(buffer(7), null, 4.0, { instrument: "take" });
    const audio = new Aggregate([[0.0, take]], "concrete", { name: "audio" });
    return [new Aggregate([[0.0, audio]], "concrete", { name: "song" }), take];
}

test("a trim moves the window and is undone as one", () => {
    const [piece, take] = takeSong();
    const ed = editor(piece);
    const c = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    // Trimmed one beat off the head: offset, duration and window all move by it.
    assert.equal(
        ed.apply("/gui_event", clipEvent(c.id as number, BEAT, 3 * BEAT, BEAT)),
        true,
    );
    assert.equal(take.start, BEAT);
    assert.equal(ed.undo(), true);
    assert.equal(take.start, 0.0);
});

test("an undone first resize gives the element its own length back", async () => {
    // The inverse of the first resize of a clip carries **no** duration at all,
    // because before it the placement stated none — and absence is a value: the
    // member takes the element's own length again. Read as "leave the length
    // alone", the log stepped back, `undo` answered true and the clip kept the
    // size the hand had given it, which is a dead button on every clip nobody
    // had resized yet.
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    const clip = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];
    assert.equal(member?.dur, null, "nothing has stated a length for it");
    const was = clip.dur as number;

    assert.equal(
        ed.apply("/gui_event", clipEvent(clip.id as number, 2 * BEAT, 1 * BEAT)),
        true,
    );
    assert.equal(member?.dur, 1.0);
    host.acks.length = 0;

    assert.equal(ed.undo(), true);
    assert.equal(member?.dur, null, "the placement states no length again");
    const props = host.corrections().get(clip.id as number);
    assert.ok(props, "and the host was told, or the picture keeps the hand's size");
    assert.equal(props.dur, was);
});

test("an undone trim puts the window back on a take that configures nothing", async () => {
    // The same rule one level down. A trim states the placement *and* the
    // window over the samples in one `setmembers`, so its inverse states the
    // member as it was — and a take nobody has configured has no configuration
    // in it at all. Skipped as "nothing to write", the clip went back to its
    // old size still reading the frames the trim had left it on: the right
    // rectangle over the wrong sound.
    const take = new Vector(buffer(7), null, 4.0);
    const audio = new Aggregate([[0.0, take]], "concrete", { name: "audio" });
    const piece = new Aggregate([[0.0, audio]], "concrete", { name: "song" });
    const ed = editor(piece);
    const host = new FakeHost();
    await ed.open(asHost(host));
    const clip = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    const was = clip.dur as number;

    assert.equal(
        ed.apply("/gui_event", clipEvent(clip.id as number, BEAT, 3 * BEAT, BEAT)),
        true,
    );
    assert.equal(take.start, BEAT);
    host.acks.length = 0;

    assert.equal(ed.undo(), true);
    assert.equal(take.start, 0.0, "the frames the trim hid are back");
    const props = host.corrections().get(clip.id as number);
    assert.ok(props, "the clip a trim moved is not the lane the intent names");
    assert.equal(props.start, 0.0, "window and all");
    assert.equal(props.dur, was);
});

test("a redefine leaves the editor able to edit", async () => {
    // A redefine moves the version so a gesture in flight comes back stale —
    // and the **document** has to move with it. The crate refuses an edit whose
    // `against` version is not the document's, ahead of it as loudly as behind
    // (the two would not be talking about the same piece), so a version bumped
    // on this side alone answered every later gesture with a refusal nobody
    // asked for: the clip did not move, and there was not even a reason to show.
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    let clip = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];
    // One edit first, so the document exists: the versions can only diverge
    // once there is a document holding one.
    assert.equal(ed.apply("/gui_event", clipEvent(clip.id as number, 3 * BEAT, 2 * BEAT)), true);

    ed.update();
    assert.ok(host.defines.length > 0, "the window was redefined");

    clip = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    assert.equal(ed.apply("/gui_event", clipEvent(clip.id as number, 5 * BEAT, 2 * BEAT)), true);
    assert.equal(member?.offset, 5.0, "and the edit landed");
});

test("a structural edit redefines the window and so does its undo", async () => {
    // A placement is a prop the host can be told about; a widget that was not
    // there is not. The second half of a split — and the clip an undone split
    // takes away again — can only arrive as a whole tree, so the editor that
    // drew the window redefines it.
    const [piece] = takeSong();
    const ed = editor(piece);
    const host = new FakeHost();
    await ed.open(asHost(host));
    const clip = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    host.defines.length = 0;

    assert.equal(
        ed.apply("/gui_event", [clip.id as number, SEQ, UNSTATED, "split", 1.0 * BEAT]),
        true,
    );
    assert.equal(host.defines.length, 1, "the split redefined the window");
    const drawnNow = () =>
        clipsOf(lanes(host.defines[host.defines.length - 1] as GuiNode)[0] as GuiNode).length;
    assert.equal(drawnNow(), 2, "with both halves in it");

    host.defines.length = 0;
    assert.equal(ed.undo(), true);
    assert.equal(host.defines.length, 1, "and so did the undo of it");
    assert.equal(drawnNow(), 1, "back to the one clip");

    host.defines.length = 0;
    assert.equal(ed.redo(), true);
    assert.equal(host.defines.length, 1, "and the redo redefines too");
    assert.equal(drawnNow(), 2, "and the redo brings it back");
});

test("a placement edit does not redefine the window", async () => {
    // The other half of the rule, and the reason it is not "redraw after every
    // edit": a redefine rebuilds every widget and drops what the host had in
    // flight, which is exactly wrong for a drag.
    const ed = editor(song(), { quant: 0.25 });
    const host = new FakeHost();
    await ed.open(asHost(host));
    const clip = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    host.defines.length = 0;
    assert.equal(ed.apply("/gui_event", clipEvent(clip.id as number, 5 * BEAT, 2 * BEAT)), true);
    assert.equal(ed.undo(), true);
    assert.equal(host.defines.length, 0, "a placement travels as a prop, there and back");
});

test("a split gives two windows over one buffer", () => {
    const [piece] = takeSong();
    const ed = editor(piece);
    const c = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    assert.equal(
        ed.apply("/gui_event", [c.id, SEQ, UNSTATED, "split", 1.0 * SEC]),
        true,
    );

    const [first, second] = clipsOf(lanes(ed.draw())[0] as GuiNode);
    // A second of samples, cut a second in: the lengths are seconds and the
    // onset the cut gives the tail is beats, which at 120 bpm is two of them.
    assert.equal(first?.dur, SEC);
    assert.equal(second?.offset, 2 * BEAT);
    assert.equal(second?.dur, 3 * SEC);
    // The second reads on from where the first stops, over the same buffer.
    assert.equal(second?.start, SEC);
    assert.equal(second?.buffer, first?.buffer);

    assert.equal(ed.undo(), true);
    const whole = clipsOf(lanes(ed.draw())[0] as GuiNode);
    assert.equal(whole.length, 1);
    assert.equal(whole[0]?.dur, 4 * SEC);
});

test("a join puts a split clip back together", () => {
    const [piece] = takeSong();
    const ed = editor(piece);
    const c = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    ed.apply("/gui_event", [c.id, SEQ, UNSTATED, "split", 1.0 * SEC]);
    const [first, second] = clipsOf(lanes(ed.draw())[0] as GuiNode);

    assert.equal(
        ed.apply("/gui_event", [first?.id, SEQ, UNSTATED, "join", first?.id, second?.id]),
        true,
    );
    const joined = clipsOf(lanes(ed.draw())[0] as GuiNode);
    assert.equal(joined.length, 1);
    assert.equal(joined[0]?.dur, 4 * SEC);
    assert.equal("start" in (joined[0] as GuiNode), false, "the window it was cut from");
});

// ---- the transport, delegated ----

test("a locate moves the transport and the lanes' cursor", async () => {
    const ed = editor();
    const host = new FakeHost();
    await ed.open(asHost(host));
    ed.locate(2.0);
    assert.equal(ed.position, 2.0);
});

test("stop returns to the top and pause keeps the position", () => {
    const ed = editor();
    ed.locate(3.0);
    assert.equal(ed.pause(), 3.0);
    ed.stop();
    assert.equal(ed.position, 0.0);
});

test("an edit marks the arrangement changed until it is rendered", () => {
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    assert.equal(ed.dirty, false);
    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    ed.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    assert.equal(ed.dirty, true, "a play, a resume or a seek must re-read it");
});

// ---- screen state that is not the composition ----

test("which layer a hand is on is screen state", async () => {
    const piece = song();
    const ed = editor(piece);
    await ed.open(asHost(new FakeHost()));
    const c = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    // The composition did not change, and the document is explicit that what a
    // view is currently editing is never part of it.
    assert.equal(ed.apply("/gui_event", [c.id, SEQ, UNSTATED, "layer", "roll"]), false);

    // And it is asked for by the **placement**, not by the widget id it arrived
    // under: a widget id is the drawing's name for something and is minted
    // afresh every redefine, so state keyed by one is emptied by every
    // structural edit — silently, since a missing key and the default layer are
    // the same answer.
    const member = (piece.handles[0]?.element as Aggregate).handles[0] as Member;
    assert.equal(ed.editLayerOf(member.element, member), "roll");
    ed.update();
    assert.equal(
        ed.editLayerOf(member.element, member),
        "roll",
        "screen state outlives the picture it was set on",
    );
});

test("a sweep becomes the crate's typed selection, in beats", () => {
    const ed = editor();
    const c = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    assert.equal(
        ed.apply("/gui_event", [c.id, SEQ, UNSTATED, "selection", 1 * BEAT, 2 * BEAT]),
        false,
    );
    const selection = ed.selection as { start: number; len: number; nodes?: number[] };
    assert.equal(selection.start, 1.0);
    assert.equal(selection.len, 2.0);
    assert.ok(selection.nodes && selection.nodes.length === 1, "swept on a clip: of that element");
});

test("a sample paste is refused because the audio has an owner", async () => {
    const ed = editor();
    const host = new FakeHost();
    await ed.open(asHost(host));
    const c = clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode;
    assert.equal(
        ed.apply("/gui_event", [c.id, SEQ, UNSTATED, "paste", 0, "samples"]),
        false,
    );
    assert.match(String(host.reasons.at(-1)), /samples are written by their owner/);
});

test("a copied block of notes pastes onto a roll and undoes in one step", async () => {
    // **Cut, copy and paste are one mechanism**, so a block a roll put on the
    // clipboard lands through the same call a drag on a note goes through: one
    // `setmembers`, one entry on the pile, one undo.
    //
    // It also settles the offset the two sides measure differently: the position
    // the host swept is on the *timeline's* axis and a roll's notes are in the
    // clip's own, so a clip placed at beat 2 holds its note 0 at beat 2.
    const timeline = new Timeline([[0.0, new SeqEvent({ midinote: 60, dur: 1.0 })]]);
    const piece = new Aggregate(
        [[0.0, new Aggregate([[2.0, new Track(timeline)]], "concrete", { name: "lead" })]],
        "concrete",
        { name: "song" },
    );
    const ed = editor(piece);
    const host = new FakeHost();
    await ed.open(asHost(host));
    const wid = (clipsOf(lanes(ed.draw())[0] as GuiNode)[0] as GuiNode).id as number;

    // What a roll's Ctrl+C puts on the clipboard: the typed document's text
    // kind, holding the flat quintuple array a `/gui_set notes` takes. Pasted
    // at beat 6 of the timeline, which is beat 4 of a clip placed at beat 2.
    const block = `{"content":{"kind":"text","text":"[0, ${BEAT}, 72, 100, 0]"}}`;
    assert.equal(
        ed.apply("/gui_event", [wid, SEQ, UNSTATED, "paste", 6.0 * BEAT, "text", block]),
        true,
        "a paste adds notes, so the composition moved",
    );
    const placed = [...timeline].map(([beat, item]) => [beat, (item as SeqEvent).get("midinote")]);
    assert.deepEqual(placed, [[0.0, 60], [4.0, 72]]);

    ed.undo();
    assert.deepEqual(
        [...timeline].map(([beat, item]) => [beat, (item as SeqEvent).get("midinote")]),
        [[0.0, 60]],
        "one undo takes the whole block back",
    );
});

test("a pasted block is refused where no roll can hold it", async () => {
    // The reason names what is missing rather than talking about samples: a
    // block of notes is written onto a roll, and a lane holds clips.
    const ed = editor();
    const host = new FakeHost();
    await ed.open(asHost(host));
    const lane = (lanes(ed.draw())[0] as GuiNode).id as number;
    const block = '{"content":{"kind":"text","text":"[0, 480, 72, 100, 0]"}}';
    assert.equal(
        ed.apply("/gui_event", [lane, SEQ, UNSTATED, "paste", 0.0, "text", block]),
        false,
    );
    assert.match(String(host.reasons.at(-1)), /roll/);
});

test("two editors over one composition keep one history", () => {
    // O19's acceptance, and the inverse of the defect it was opened for. Two
    // windows over one composition used to mint a history each, so stepping one
    // of them reverted across the other's edits and wrote a state nobody was
    // ever in. The history belongs to the arrangement now, so both windows
    // reach the same one.
    const piece = song();
    const a = editor(piece, { quant: 0.25 });
    const b = editor(piece, { quant: 0.25 });
    assert.equal(Editing.of(piece), Editing.of(piece), "one context per element");

    const roll = clipsOf(lanes(a.draw())[1] as GuiNode)[0] as GuiNode;
    const member = (piece.handles[1]?.element as Aggregate).handles[0];
    const start = member?.offset;
    a.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    const moved = member?.offset;
    assert.notEqual(moved, start);

    // The second window can undo what the first did -- which is the half that
    // used to be false: `b.canUndo` was false, and b could not see the edit at
    // all even though it was showing the data it changed.
    assert.equal(b.canUndo, true);
    assert.equal(b.undoLabel, a.undoLabel);
    assert.equal(b.undo(), true);
    assert.equal(member?.offset, start, "and the undo reached the arrangement");
    assert.equal(a.canUndo, false, "one order, not two");
});

test("a view over a part of the composition reaches the same history", () => {
    // A dedicated roll of one track is a view over data the multitrack is
    // already showing, so it edits through the composition's history rather
    // than minting a second one over the same dataset.
    const piece = song();
    const whole = editor(piece);
    whole.draw();
    const roll = clipsOf(lanes(whole.draw())[1] as GuiNode)[0] as GuiNode;
    whole.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));

    const lane = piece.handles[1]?.element as Element;
    assert.equal(Editing.of(lane), Editing.of(piece));
});

test("an edit in one window reaches the other as props", async () => {
    // One history is only half of it: the second window has to be *told*. An
    // acknowledgement goes to the window whose gesture it answered, so without
    // this a second view goes on drawing a piece that moved under it — and then
    // its undo steps an order it cannot see, which looks exactly like a dead
    // button.
    //
    // It arrives as **props**, not as a redefine: a placement is a value, and
    // rebuilding every widget under a hand that is not even in that window is
    // what made the second one flicker on every step.
    const piece = song();
    const a = editor(piece, { quant: 0.25 });
    const b = editor(piece, { quant: 0.25 });
    const hostB = new FakeHost();
    await a.open(asHost(new FakeHost()));
    await b.open(asHost(hostB));
    hostB.defines.length = 0;
    hostB.acks.length = 0;

    const roll = clipsOf(lanes(a.draw())[1] as GuiNode)[0] as GuiNode;
    a.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    assert.equal(hostB.defines.length, 0, "no redefine for a placement");
    assert.ok(hostB.acks.length > 0, "but the other window was told what to draw");
    assert.ok(hostB.acks[0]?.[1].length, "and the correction carries what moved");

    // And an undo from either reaches both the same way, once: a turn that goes
    // through `apply` into `undo` is still one gesture.
    hostB.acks.length = 0;
    assert.equal(a.undo(), true);
    assert.equal(hostB.defines.length, 0);
    assert.equal(hostB.acks.length, 1, "one gesture, one answer");
});

test("a structural edit in one window redefines the other", async () => {
    // The case no prop can carry, from the other side: a widget that was not
    // there a moment ago is not a value, so the second window is redrawn whole.
    const [piece] = takeSong();
    const a = editor(piece);
    const b = editor(piece);
    const hostB = new FakeHost();
    await a.open(asHost(new FakeHost()));
    await b.open(asHost(hostB));
    const c = clipsOf(lanes(a.draw())[0] as GuiNode)[0] as GuiNode;
    hostB.defines.length = 0;

    assert.equal(a.apply("/gui_event", [c.id, SEQ, UNSTATED, "split", 1.0 * SEC]), true);
    assert.equal(hostB.defines.length, 1, "the split redrew the other window too");
    assert.equal(
        clipsOf(lanes(hostB.defines[0] as GuiNode)[0] as GuiNode).length,
        2,
        "with both halves in it",
    );
});

test("a closed window is not told about edits", async () => {
    // Closing a view is not an event of the history — what goes is this
    // window's place in the list of who to tell.
    const piece = song();
    const a = editor(piece, { quant: 0.25 });
    const b = editor(piece, { quant: 0.25 });
    await a.open(asHost(new FakeHost()));
    const hostB = new FakeHost();
    const win = await b.open(asHost(hostB));
    b.apply("/gui_closed", [win.id]);
    const defines = hostB.defines.length;

    const roll = clipsOf(lanes(a.draw())[1] as GuiNode)[0] as GuiNode;
    a.apply("/gui_event", clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT));
    assert.equal(hostB.defines.length, defines, "a closed window has no picture to keep");
    assert.equal(b.canUndo, true, "and it still shares the history");
});

test("a step with nothing to step tells nobody", async () => {
    // **A no-op is not a change**, and answering that it is one is how a window
    // nobody touched came to redraw. A history at its end is the ordinary case
    // — a person holds Ctrl+Z until it stops — and `apply` used to answer true
    // for every one of those, so every other view of the composition was told
    // to bring itself in step with an edit that never happened.
    const piece = song();
    const host = new FakeHost();
    const a = editor(piece);
    const b = editor(piece);
    const winA = await a.open(asHost(host));
    await b.open(asHost(host));
    host.defines.length = 0;

    for (const tag of ["undo", "redo"]) {
        assert.equal(a.apply("/gui_event", [winA.id, SEQ, UNSTATED, tag]), false);
        assert.equal(b.apply("/gui_event", [winA.id, SEQ, UNSTATED, tag]), false);
    }
    assert.equal(host.defines.length, 0, "no window redrawn for a step that did not happen");
});

test("a resend is not an edit", async () => {
    // The crate's own rule, honoured by the editor: an edit that states what
    // already holds is refused and does not move the version — so the editor
    // must not answer that the composition changed, or every other view redraws
    // for a gesture that did nothing.
    //
    // The notes route got this wrong twice over: it answered true
    // unconditionally, *and* it minted a fresh node id for every note on every
    // edit, so the same notes resent really did arrive as different members.
    const piece = song();
    const ed = editor(piece, { quant: 0.25 });
    await ed.open(asHost(new FakeHost()));

    const roll = clipsOf(lanes(ed.draw())[1] as GuiNode)[0] as GuiNode;
    const move = clipEvent(roll.id as number, 5 * BEAT, 2 * BEAT);
    assert.equal(ed.apply("/gui_event", move), true);
    assert.equal(ed.apply("/gui_event", move), false, "the same placement again");

    const notes = [roll.id, SEQ, UNSTATED, "notes", 0, BEAT, 62, 100, 0];
    assert.equal(ed.apply("/gui_event", notes), true);
    assert.equal(ed.apply("/gui_event", notes), false, "the same notes again");
});

test("a cord is an edit and undoes in one step", async () => {
    // A cord was the last gesture that wrote the arrangement directly, on the
    // grounds that no intent described it — and the vocabulary had said what to
    // do all along. An edit states the resulting value, and what a cord results
    // in is the two members' controls and the aggregate's declared buses: three
    // `configure`s, one entry, one undo.
    const src = new SynthDef("gsrc", out(control("out"), sine(control("freq", 220.0))));
    const sink = new SynthDef("gsink", out(0.0, inBus(control("in")).mul(control("amp", 0.3))));
    const g = new Aggregate(null, LOGICAL, { name: "chain" });
    const hs = g.add(new Generator(src));
    const hk = g.add(new Generator(sink));
    const ed = editor(g);
    await ed.open(asHost(new FakeHost()));
    // The workspace `plane` wraps the patch `plane` — the one carrying boxes,
    // which is what a cord is addressed to.
    const workspace = (ed.draw().children as GuiNode[]).find(
        (c) => c.type === "plane" && !("boxes" in c),
    ) as GuiNode;
    const wid = ((workspace.children as GuiNode[])[0] as GuiNode).id as number;

    assert.equal(
        ed.apply("/gui_event", [wid, SEQ, UNSTATED, "wire", 0, "out", 1, "in"]),
        true,
    );
    const controls = (m: Member) =>
        (m.element as Element & { controls?: Record<string, unknown> }).controls ?? {};
    const bus = controls(hs).out;
    assert.equal(controls(hk).in, bus);
    assert.deepEqual(g.busNames, [bus], "and the aggregate declares the bus it names");

    assert.equal(ed.canUndo, true, "a cord is an edit like any other");
    assert.equal(ed.undoLabel, "draw a cord");
    assert.equal(ed.undo(), true);
    assert.deepEqual(controls(hs), {}, "both ends come back");
    assert.deepEqual(controls(hk), {});
    assert.deepEqual(g.busNames, [], "and the bus goes with them");
    assert.equal(ed.canUndo, false, "one gesture, one step");

    assert.equal(ed.redo(), true);
    assert.equal(controls(hk).in, bus);
    assert.deepEqual(g.busNames, [bus]);
});

test("a logical aggregate's buses survive the document", () => {
    // The other half, and the reason the cord could not be logged: a patch's
    // buses were in no format at all. The cords survived a round trip — a
    // member's controls are in its own config — while the buses they name did
    // not, so a reopened patcher drew connections it could render none of.
    const g = new Aggregate(null, LOGICAL, { name: "chain", buses: [["mix", "audio"]] });
    g.add(new Generator(new SynthDef("gsrc", out(control("out"), sine(220.0))), null, null, {
        controls: { out: "mix" },
    }));
    const back = fromDocument(toDocument(g)) as Aggregate;
    assert.deepEqual(back.busNames, ["mix"]);
    assert.deepEqual(back.busSpecList, [{ name: "mix", rate: "audio", channels: 1 }]);
});

// ---- the lane header: what is the composition's, and what is the view's ----

test("a lane header draws the composition's mixing and writes it back", () => {
    const laneElement = new Aggregate(
        [[0.0, new Clang(new SeqEvent({ midinote: 60, dur: 1.0 }))]],
        "concrete",
        { name: "drums" },
    );
    const ed = editor(new Aggregate([[0.0, laneElement]], "concrete", { name: "song" }));
    const lane = lanes(ed.draw())[0] as GuiNode;
    assert.deepEqual(
        [lane.mute, lane.solo, lane.level],
        [false, false, 1.0],
        "a control nothing draws is a control nobody can press",
    );

    assert.equal(ed.apply("/gui_event", [lane.id as number, SEQ, UNSTATED, "mute", 1]), true);
    assert.equal(laneElement.mute, true);
    // It is an edit like any other, so it is in the history and it inverts.
    assert.equal(ed.canUndo, true);
    assert.equal(ed.undoLabel, "mute the lane");
    ed.undo();
    assert.equal(laneElement.mute, false);

    assert.equal(
        ed.apply("/gui_event", [lane.id as number, SEQ, UNSTATED, "level", 0.25]),
        true,
    );
    assert.equal(laneElement.level, 0.25);
    assert.equal(
        (lanes(ed.draw())[0] as GuiNode).level,
        0.25,
        "redrawn from the composition, not remembered by the view",
    );
});

test("a lane resized by hand changes no composition", () => {
    // `height` is the view's: the host already resized the lane it was made on,
    // and no document carries it.
    const ed = editor(
        new Aggregate(
            [[0.0, new Clang(new SeqEvent({ midinote: 60, dur: 1.0 }))]],
            "concrete",
            { name: "song" },
        ),
    );
    const lane = lanes(ed.draw())[0] as GuiNode;
    assert.equal(
        ed.apply("/gui_event", [lane.id as number, SEQ, UNSTATED, "height", 90.0]),
        false,
    );
    assert.equal(ed.canUndo, false);
});

test("a muted lane still draws everything it had", () => {
    // A picture that emptied when the toggle was pressed would report silence as
    // absence.
    const laneElement = new Aggregate(
        [[0.0, new Clang(new SeqEvent({ midinote: 60, dur: 1.0 }))]],
        "concrete",
        { name: "drums" },
    );
    const ed = editor(new Aggregate([[0.0, laneElement]], "concrete", { name: "song" }));
    const before = clipsOf(lanes(ed.draw())[0] as GuiNode);
    laneElement.mute = true;
    const after = clipsOf(lanes(ed.draw())[0] as GuiNode);
    assert.equal(after.length, before.length);
    assert.equal(after[0]?.dur, before[0]?.dur);
});
