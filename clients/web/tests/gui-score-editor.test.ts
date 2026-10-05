// The score editor: a symbolic score edited in place by the crate's
// application, through the handle the page already holds (mirrors
// clients/python/tests/test_gui_score_editor.py).
//
// Needs `./build.sh` (the core wasm) and `third_party/build-verovio-wasm.sh`
// (the engraver in `vendor/`); skips itself, loudly, when the engraver is not
// built. Run with `npm test`.

import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import test from "node:test";

import { loadCore } from "../src/base/core.ts";
import { Score, setEngraverUrl } from "../src/gui/notation/index.ts";
import { ScoreEditor, edit } from "../src/gui/editing/index.ts";
import type { GuiNode } from "../src/gui/guidef.ts";

const engraver = new URL("../vendor/verovio/verovio.js", import.meta.url);
const PHRASE = "@clef:G-2\n@timesig:4/4\n@data:4CDEF/ 4GABc'/";

await loadCore();

interface Item {
    id: number;
    marks?: { articulations?: string[] };
}

function items(score: Score): Item[] {
    const sheet = score.sheet() as unknown as {
        staves: { voices: { items: Item[] }[] }[];
    };
    return sheet.staves[0].voices[0].items;
}

if (!existsSync(engraver)) {
    test("the engraver is built", { skip: "run third_party/build-verovio-wasm.sh" }, () => {});
} else {
    setEngraverUrl(engraver.href);

    test("the window is the page in a scroll over a status line", async () => {
        const editor = new ScoreEditor(await Score.open(PHRASE));
        const tree = editor.draw();
        const [scroll, status] = tree.children ?? [];
        assert.equal(scroll.type, "scroll");
        const page = (scroll.children ?? [])[0] as GuiNode & Record<string, unknown>;
        assert.equal(page.type, "score");
        assert.equal(page.editable, true);
        assert.equal(page.entry, true);
        assert.ok("kinds" in page && !("notes" in page));
        assert.equal(status.type, "label");
    });

    test("opening writes the page from the model", async () => {
        const score = await Score.open(PHRASE);
        new ScoreEditor(score);
        assert.ok(score.mei().includes(`xml:id="n${items(score)[0].id}"`));
    });

    test("a verb edits the holder's score and walks back", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        const first = items(score)[0].id;
        editor.select([`n${first}`]);
        assert.deepEqual(editor.selected, [first]);
        assert.ok(editor.articulation("stacc"));
        assert.deepEqual(items(score)[0].marks?.articulations, ["stacc"]);
        assert.ok(editor.undo());
        assert.equal(items(score)[0].marks, undefined);
        assert.ok(editor.redo());
        assert.deepEqual(items(score)[0].marks?.articulations, ["stacc"]);
    });

    test("a spanner runs between the first and the last selected", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        const ids = items(score).map((item) => item.id);
        editor.select([`n${ids[3]}`, `n${ids[0]}`]);
        assert.ok(editor.spanner("slur"));
        const sheet = score.sheet() as unknown as {
            spanners: { kind: string; from: number; to: number }[];
        };
        assert.deepEqual(sheet.spanners, [{ kind: "slur", from: ids[0], to: ids[3] }]);
    });

    test("a verb with nothing selected changes nothing", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        const before = score.mei();
        assert.equal(editor.delete(), false);
        assert.equal(score.mei(), before);
    });

    test("the value in hand is the crate's", async () => {
        const editor = new ScoreEditor(await Score.open(PHRASE), { value: [1, 8] });
        assert.deepEqual(editor.value, [1, 8]);
        editor.value = [1, 2];
        assert.deepEqual(editor.value, [1, 2]);
    });

    test("a transformation runs over the measures selected", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        const octaves = () =>
            (items(score) as unknown as { pitches: { octave: number }[] }[]).map(
                (item) => item.pitches[0].octave,
            );
        const ids = items(score).map((item) => item.id);
        const before = octaves();
        editor.select([`n${ids[5]}`]); // a note of the second bar
        assert.ok(editor.transform("transpose", { semitones: 12 }));
        const after = octaves();
        assert.deepEqual(after.slice(0, 4), before.slice(0, 4));
        assert.deepEqual(after.slice(4), before.slice(4).map((octave) => octave + 1));
        assert.equal(editor.transform("fold"), false);
    });

    test("entry is the crate's switch", async () => {
        const editor = new ScoreEditor(await Score.open(PHRASE));
        assert.equal(editor.entry, true);
        editor.entry = false;
        assert.equal(editor.entry, false);
    });

    test("edit opens a score in the score editor", async () => {
        const score = await Score.open(PHRASE);
        const editor = await edit(score, { open: false });
        assert.ok(editor instanceof ScoreEditor);
        assert.equal((editor as unknown as ScoreEditor).score, score);
    });
}
