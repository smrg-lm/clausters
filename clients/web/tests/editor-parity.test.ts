// The multitrack editor's draw against the Python client's, on the shared
// vectors.
//
// The editor is one driver in two languages and what leaves it is a **GuiDef**:
// the lanes, the clips, the bodies, the ids, and every number the
// beats↔timeline-samples bridge produced. `gen-editor-vectors.py` freezes what
// the Python editor draws for one composition per branch of the mapping rule,
// and each case here builds the same composition and asserts the same tree.
//
// A host-less draw counts ids from `baseId` on both sides, so the registries line
// up too: a mismatch is a real difference in the mapping and not an allocation
// order.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Env, control, in_, out, sine } from "../src/defs/ugens/index.ts";
import { SynthDef } from "../src/defs/synthdef.ts";
import {
    Aggregate,
    Clang,
    Element,
    Generator,
    Segments,
    Track,
    Vector,
} from "../src/form/index.ts";
import type { SourceLike } from "../src/form/index.ts";
import { FormEditor } from "../src/gui/editing/index.ts";
import { Automation } from "../src/seq/automation.ts";
import { Event as SeqEvent } from "../src/seq/event.ts";
import { Timeline } from "../src/seq/timeline.ts";

const here = new URL(".", import.meta.url);
await loadCore();

const vectors = JSON.parse(
    await readFile(new URL("./editor-vectors.json", here), "utf8"),
) as {
    sample_rate: number;
    tempo: number;
    cases: Record<string, { quant: number; expand: boolean; tree: unknown; extent: number }>;
};

const SR = vectors.sample_rate;
const TEMPO = vectors.tempo;
const BEAT = SR / TEMPO;

/**
 * A server buffer as the editor reads one: its number, shape and rate. Its size
 * is in **seconds** -- seconds because that is what a take's length is measured
 * in, whatever the tempo is.
 */
const buffer = (bufnum: number, secs = 2.0, channels = 1): SourceLike => ({
    bufnum,
    frames: Math.trunc(secs * SR),
    channels,
    sampleRate: SR,
});

/** The same compositions `gen-editor-vectors.py` builds, in this language. */
const compositions: Record<string, () => Aggregate> = {
    a_song() {
        const take = new Vector(buffer(7), null, 2.0); // two seconds of samples
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
    },

    a_windowed_take() {
        const take = new Vector(buffer(9), null, 1.0, { start: 12_000.0, loop: true });
        return new Aggregate([[1.0, take]], "concrete", { name: "trimmed" });
    },

    a_joined_take() {
        const joined = new Segments([
            [buffer(3), 0.0, 0.5],
            [buffer(4), 6_000.0, 0.75],
        ]);
        return new Aggregate([[0.0, joined]], "concrete", { name: "joined" });
    },

    an_envelope_on_its_event() {
        const curve = new Automation(new Env([0.2, 0.9, 0.1], [1.0, 1.0]), null, {
            name: "cutoff",
        });
        // Two seconds of curve and four beats of notes: the same stretch at 120
        // bpm, which is what makes the aggregate simultaneous and the clip one.
        const notes = new Track(
            new Timeline([[0.0, new SeqEvent({ midinote: 72, dur: 4.0 })]]),
            null,
            4.0,
        );
        const pair = new Aggregate(
            [
                [0.0, 2.0, new Element(curve, null, 2.0)],
                [0.0, 4.0, notes],
            ],
            "concrete",
            { name: "shaped" },
        );
        return new Aggregate([[0.0, pair]], "concrete", { name: "song" });
    },

    a_curve_over_a_rendering() {
        const curve = Automation.fromPoints(
            [[0.0, 200.0, 1, 0.0], [2.0, 900.0, 2, 0.0], [4.0, 300.0, 1, 0.0]],
            null,
            { name: "freq" },
        );
        const voice = new Clang(
            new SeqEvent({ instrument: "drone", dur: 8.0, legato: 1.0, amp: 0.12 }),
        );
        const pair = new Aggregate(
            [[0.0, voice], [0.0, new Element(curve, null, 4.0)]],
            "concrete",
            { name: "sweep" },
        );
        return new Aggregate([[0.0, pair]], "concrete", { name: "song" });
    },

    a_nested_aggregate() {
        const inner = new Aggregate(
            [
                [0.0, new Clang(new SeqEvent({ midinote: 60, dur: 1.0 }))],
                [1.0, new Clang(new SeqEvent({ midinote: 64, dur: 1.0 }))],
            ],
            "concrete",
            { name: "phrase" },
        );
        return new Aggregate([[0.0, inner]], "concrete", { name: "song" });
    },

    a_patch() {
        const src = new SynthDef("gsrc", out(control("out"), sine(control("freq", 220.0))));
        const sink = new SynthDef(
            "gsink",
            out(0, in_(control("in")).mul(control("amp", 0.3))),
        );
        const g = new Aggregate(null, "logical", {
            name: "chain",
            buses: [["mix", "audio"]],
        });
        g.add(new Generator(src, null, null, { controls: { out: "mix" } }));
        g.add(new Generator(sink, null, null, { controls: { in: "mix" } }));
        return g;
    },
};

for (const [name, vector] of Object.entries(vectors.cases)) {
    const build = compositions[name] ?? compositions[name.replace(/_expanded$/, "")];
    assert.ok(build, `no composition for '${name}' — regenerate the vectors`);

    test(`'${name}' draws the same tree`, () => {
        const element = build();
        const editor = new FormEditor(element, {
            sampleRate: SR,
            tempo: TEMPO,
            quant: vector.quant,
        });
        if (vector.expand) {
            // The base level: resolve the nested aggregate into lanes.
            editor.expand(element.members[0]?.[2] as Element);
        }
        assert.deepEqual(JSON.parse(JSON.stringify(editor.draw())), vector.tree);
    });

    test(`'${name}' spans the same extent`, () => {
        const element = build();
        const editor = new FormEditor(element, { sampleRate: SR, tempo: TEMPO });
        assert.equal(editor.extent(), vector.extent);
    });
}
