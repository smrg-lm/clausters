// The arrangement against the Python client's, on the shared vectors.
//
// `gen-form-vectors.py` builds a handful of compositions with the Python
// surface and freezes the two things that leave the layer: the **document**
// each is written as (a shared format three languages read) and the
// **flattened timeline** it renders to — the absolute beats, the events at
// them, and what a placement's length trims. Each case here rebuilds the same
// composition through the TypeScript surface and asserts the same two results.
//
// What has to match is what leaves the layer, never the source: the two clients
// are one client in two languages, so a rule that drifts into one of them —
// a trim rounding differently, a config key spelled the language's way rather
// than the file's — fails here rather than in a piece that reopens wrong.
//
// Run with `npm test`; this suite needs nothing staged.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Automation } from "../src/seq/automation.ts";
import { Event as SeqEvent } from "../src/seq/event.ts";
import { Timeline } from "../src/seq/timeline.ts";
import {
    Aggregate,
    Clang,
    Element,
    Generator,
    Segments,
    Sequence,
    Track,
    Vector,
    flatten,
    toDocument,
    toSession,
} from "../src/form/index.ts";
import type { SourceLike } from "../src/form/index.ts";

// The flattening crosses beats to seconds through the shared core's time map
// (`TempoMap`), so the wasm has to be up before any of it runs — the same
// requirement the clock has always had, now that the arrangement measures time
// with the same one function rather than a ratio of its own.
await loadCore();

const here = new URL(".", import.meta.url);

interface Case {
    document: unknown;
    flat: ({ beat: number } & ({ event: Record<string, unknown> } | { item: string }))[];
    relation: string | null;
}

const vectors = JSON.parse(
    await readFile(new URL("./form-vectors.json", here), "utf8"),
) as { cases: Record<string, Case>; session: unknown };

/** A stand-in for a server buffer: the conversion reads a `bufnum`. */
const buffer = (bufnum: number): SourceLike => ({ bufnum });

/**
 * The two event keys this language spells differently, on their way to the
 * comparison — the document and the Python client both say `add_action` and
 * `has_gate`, which is what a saved piece carries.
 */
const asFile = (props: Record<string, unknown>): Record<string, unknown> => {
    const out: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(props)) {
        const name = key === "addAction" ? "add_action" : key === "hasGate" ? "has_gate" : key;
        out[name] = value;
    }
    return out;
};

/** The flattened timeline as the generator writes it. */
function flat(element: Element): unknown[] {
    return flatten(element).map(([beat, item]) =>
        item instanceof SeqEvent
            ? { beat, event: asFile(item.props) }
            : { beat, item: (item as object).constructor.name },
    );
}

/** The same compositions `gen-form-vectors.py` builds, in this language. */
const cases: Record<string, () => Aggregate> = {
    a_piece() {
        const piece = new Aggregate(null, "concrete", { name: "piece" });
        piece.add(new Clang(new SeqEvent({ midinote: 60, dur: 1.0 })), 0.0, 1.0);
        piece.add(
            new Vector(buffer(100), null, 4.0, { instrument: "take" }),
            2.0,
            4.0,
        );
        const inner = new Aggregate();
        inner.add(new Clang(new SeqEvent({ midinote: 67, dur: 0.5 })), 0.0, 0.5);
        piece.add(inner, 8.0, 2.0);
        return piece;
    },

    a_trimmed_placement() {
        const held = new Aggregate();
        held.add(new Clang(new SeqEvent({ midinote: 60, dur: 2.0 })), 0.0);
        held.add(new Clang(new SeqEvent({ midinote: 64, dur: 2.0 })), 2.0);
        held.add(new Clang(new SeqEvent({ midinote: 67, dur: 2.0 })), 4.0);
        const piece = new Aggregate();
        piece.add(held, 1.0, 3.0);
        return piece;
    },

    a_track() {
        const timeline = new Timeline();
        timeline.add(0.0, new SeqEvent({ midinote: 48, dur: 1.0 }));
        timeline.add(1.5, new SeqEvent({ midinote: 55, dur: 0.5 }));
        const piece = new Aggregate();
        piece.add(new Track(timeline, null, null, { name: "bass" }), 4.0);
        return piece;
    },

    a_window() {
        const piece = new Aggregate();
        piece.add(
            new Vector(buffer(7), null, 2.0, {
                instrument: "take",
                start: 44100.0,
                loop: true,
                controls: { amp: 0.5 },
            }),
            0.0,
            2.0,
        );
        piece.add(
            new Segments(
                [
                    [buffer(7), 0.0, 1.0],
                    [buffer(8), 22050.0, 1.5],
                ],
                null,
                null,
                { instrument: "take" },
            ),
            2.0,
        );
        return piece;
    },

    a_frozen_generator() {
        const rendered = new Aggregate();
        rendered.add(new Clang(new SeqEvent({ midinote: 72, dur: 0.25 })), 0.0, 0.25);
        const piece = new Aggregate();
        piece.add(
            new Generator("melody", null, 4.0, { name: "melody", rendered }),
            0.0,
            4.0,
        );
        piece.add(new Sequence(null, null, 1.0, { name: "unheld" }), 4.0);
        return piece;
    },

    a_curve_on_its_event() {
        const curve = Automation.fromPoints(
            [[0.0, 200.0, 1, 0.0], [2.0, 900.0, 2, 0.0], [4.0, 300.0, 1, 0.0]],
            null,
            { name: "freq" },
        );
        const piece = new Aggregate();
        piece.add(
            new Aggregate(
                [
                    [0.0, new Clang(new SeqEvent({ instrument: "drone", dur: 4.0 }))],
                    [0.0, new Element(curve, null, 4.0)],
                ],
                "concrete",
                { name: "sweep" },
            ),
            0.0,
        );
        return piece;
    },
};

for (const [name, build] of Object.entries(cases)) {
    const vector = vectors.cases[name];
    assert.ok(vector, `no reference vector for '${name}' — regenerate them`);

    test(`'${name}' is written as the same document`, () => {
        assert.deepEqual(toDocument(build()), vector.document);
    });

    test(`'${name}' flattens to the same timeline`, () => {
        assert.deepEqual(flat(build()), vector.flat);
    });

    test(`'${name}' derives the same temporal relation`, () => {
        assert.equal(build().temporalRelation(), vector.relation);
    });
}

test("a session carries the same document and table", () => {
    const take = new Vector(buffer(100), null, 4.0, { instrument: "take" });
    const piece = new Aggregate();
    piece.add(take, 0.0, 4.0);
    const session = toSession(piece, {
        sources: {
            100: { location: "takes/one.wav", lifetime: "session", generation: 0 },
        },
    });
    assert.deepEqual(session, vectors.session);
});
