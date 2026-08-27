// The document model against the Python client's, on the shared vectors.
//
// This is the one that proves the crate is *the* model rather than a model
// each client copies. `gen-document-vectors.py` builds a composition with the
// Python client, applies a run of edits through the C ABI, and freezes the
// document after each. Here the identical edits go through the wasm door and
// the documents must match byte for byte.
//
// Nothing else would notice a divergence. Cargo checks each binding against the
// crate and never against the other, and no build reaches either client's call
// sites — so a snap implemented twice, a version bumped on the wrong side or a
// staleness rule read differently would ship green.
//
// Needs the core wasm staged (`./build.sh`); run with `npm test`.

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Document, Log, applyIntent, resolveSelection } from "../src/document.ts";
import type {
    Against,
    ClaustersDocument,
    Intent,
    Outcome,
    Redone,
    Resolved,
    Undone,
} from "../src/document.ts";

interface Vectors {
    start: ClaustersDocument;
    edits: {
        label: string;
        intent: Intent;
        against: Against | null;
        quant: number;
        document: ClaustersDocument;
        outcome: {
            effective: Intent;
            applied: boolean;
            reason: string | null;
            stale: boolean;
        };
    }[];
    final: ClaustersDocument;
    resolutions: {
        selection: { start: number; len: number };
        inBeats: boolean;
        framesPerBeat: number;
        spans: Resolved[];
    }[];
    logged: {
        applies: {
            label: string;
            intent: Intent;
            quant: number;
            document: ClaustersDocument;
            outcome: Outcome;
            entries: number;
            undoLabel: string;
        }[];
        // The vector froze the document after each step as well as what the
        // step reported, and it keeps doing so: the document is what the two
        // sides are actually being compared on, and it is no longer part of
        // what `undo`/`redo` return.
        undos: (Undone & { document: ClaustersDocument })[];
        redos: (Redone & { document: ClaustersDocument })[];
        inverted: ClaustersDocument;
        redone: ClaustersDocument;
    };
}

const here = import.meta.url;

const vectors = JSON.parse(
    await readFile(new URL("document-vectors.json", here), "utf8"),
) as Vectors;

// Node has no URL-relative wasm fetch, so the module arrives as bytes -- the
// same path every other parity test here takes.
await loadCore();

test("the same edits produce the same document", async () => {
    let document = vectors.start;
    for (const expected of vectors.edits) {
        const result = await applyIntent(document, expected.intent, {
            against: expected.against ?? undefined,
            quant: expected.quant,
        });
        assert.deepEqual(
            result.document,
            expected.document,
            `document after: ${expected.label}`,
        );
        assert.deepEqual(
            result.outcome,
            expected.outcome,
            `outcome of: ${expected.label}`,
        );
        document = result.document;
    }
    assert.deepEqual(document, vectors.final);
});

test("a transformed edit reports the value it became on both sides", async () => {
    // The case a second implementation gets subtly wrong: the owner snapped the
    // placement, and what has to travel is where it *landed*.
    const snapped = vectors.edits.find((e) => e.outcome.reason === "snapped to the grid");
    assert.ok(snapped, "the vectors carry a snapped edit");
    const result = await applyIntent(vectors.start, snapped.intent, {
        quant: snapped.quant,
    });
    assert.equal(result.outcome.reason, "snapped to the grid");
    assert.notDeepEqual(result.outcome.effective, snapped.intent);
});

test("a stale edit is stale here too, and moves nothing", async () => {
    const stale = vectors.edits.find((e) => e.outcome.stale);
    assert.ok(stale, "the vectors carry a stale edit");
    // Against a version the document has left behind, from its own start.
    const result = await applyIntent(vectors.start, stale.intent, {
        against: { version: vectors.start.version + 5 },
    });
    assert.equal(result.outcome.stale, true);
    assert.equal(result.outcome.applied, false);
    assert.equal(result.document.version, vectors.start.version);
});

test("a selection resolves to the same spans", async () => {
    for (const expected of vectors.resolutions) {
        const spans = await resolveSelection(
            vectors.start,
            expected.selection,
            expected.framesPerBeat,
            expected.inBeats,
        );
        assert.deepEqual(
            spans,
            expected.spans,
            `spans for ${JSON.stringify(expected.selection)}`,
        );
    }
});

test("an edit is idempotent, so a resend over a lossy leg is harmless", async () => {
    // A property of the vocabulary being absolute rather than of care taken in
    // any binding -- which is why it is worth asserting on this side too.
    const intent = vectors.edits[0].intent;
    const once = await applyIntent(vectors.start, intent);
    const twice = await applyIntent(once.document, intent);
    assert.deepEqual(twice.document, once.document);
    assert.equal(twice.outcome.applied, false, "nothing changed the second time");
});

// ---- the log ----

test("the same edits logged here reach the same states", async () => {
    // O11's acceptance across the third side. The log is an object rather than
    // a value -- a bulk inverse leaves it for the spill store on purpose -- so
    // what is compared is the documents it produces, not the log itself.
    const log = await Log.open();
    const doc = await Document.open(vectors.start);
    try {
        for (const expected of vectors.logged.applies) {
            const outcome = log.apply(doc, expected.intent, {
                quant: expected.quant,
                label: expected.label,
            });
            assert.deepEqual(doc.snapshot(), expected.document, expected.label);
            assert.deepEqual(outcome, expected.outcome, expected.label);
            assert.equal(log.length, expected.entries);
            assert.equal(log.undoLabel, expected.undoLabel);
        }

        for (const expected of vectors.logged.undos) {
            assert.ok(log.canUndo);
            const step = log.undo(doc);
            assert.ok(step, "an undo that the vector says happened");
            assert.deepEqual(step.undone, expected.undone);
            assert.deepEqual(doc.snapshot(), expected.document);
        }
        assert.equal(log.canUndo, false);
        assert.deepEqual(
            doc.snapshot(),
            vectors.logged.inverted,
            "a run of gestures inverts back to where it started, exactly",
        );

        for (const expected of vectors.logged.redos) {
            const step = log.redo(doc);
            assert.ok(step, "a redo that the vector says happened");
            // The intents, like the undo branch above: a redo reports what it
            // applied so a view projects it the way it projects an undo, and
            // that report is a second answer the two faces have to agree on --
            // the document matching says the states met, not that both sides
            // said how they got there.
            assert.deepEqual(step.redone, expected.redone);
            assert.deepEqual(step.remaining, [], "nothing for the owner to re-run");
            assert.deepEqual(doc.snapshot(), expected.document);
        }
        assert.deepEqual(doc.snapshot(), vectors.logged.redone);
    } finally {
        log.free();
        doc.free();
    }
});

test("a refused edit leaves nothing to undo, on this side too", async () => {
    const log = await Log.open();
    const doc = await Document.open(vectors.start);
    try {
        log.apply(doc, { intent: "place", node: 999, offset: 1 });
        assert.equal(log.length, 0);
        assert.equal(log.canUndo, false);
        assert.equal(log.undo(doc), undefined, "and says so");
    } finally {
        log.free();
        doc.free();
    }
});

test("a deterministic operation comes back for the owner to re-run", async () => {
    // The asymmetry the log exists to express: going back is data, going
    // forward may be a recipe, and no binding can execute one.
    const log = await Log.open();
    const doc = await Document.open(vectors.start);
    try {
        const node = 3;
        log.record(
            { recompute: { op: "normalize", peak: 1 } },
            { intent: "writesamples", node, start: 0, values: [0.25] },
            { label: "normalize" },
        );
        assert.ok(log.undo(doc));
        const redone = log.redo(doc);
        assert.ok(redone);
        assert.equal(redone.remaining.length, 1);
        assert.ok("recompute" in redone.remaining[0]);
    } finally {
        log.free();
        doc.free();
    }
});
