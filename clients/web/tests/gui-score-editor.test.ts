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
    dur: [number, number];
    pitches?: { alter?: number }[];
    marks?: { articulations?: string[] };
}

/** The `score` widget of an editor's window: the scroll's one child. */
function pageOf(tree: GuiNode): GuiNode {
    const scroll = (tree.children ?? []).find((child) => child.type === "scroll");
    return (scroll?.children ?? [])[0];
}

/** The document version an event is made against, which the editor keeps. */
const versionOf = (editor: ScoreEditor) => (editor as unknown as { version: number }).version;

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
        const [toolbar, scroll, status] = tree.children ?? [];
        // the toolbar is a row of the crate's tools, each under an id of its own
        const tools = (toolbar.children ?? []).filter((tool) => "id" in tool);
        assert.equal((toolbar as unknown as { flow: string }).flow, "row");
        assert.equal(new Set(tools.map((tool) => tool.id)).size, 12);
        // a tool is drawn with the engraver's own symbol: its label is the
        // SMuFL character, and the window carries the outline the host draws
        // it with
        const values = tools.find((tool) => tool.type === "choice") as unknown as {
            options: string[];
        };
        assert.equal(values.options[2], "\uE1D5");
        const glyphs = (tree as unknown as { glyphs: Record<string, string> }).glyphs;
        assert.ok(glyphs.E1D5.startsWith("M"));
        assert.equal(Object.keys(glyphs).length, 19);
        assert.equal(scroll.type, "scroll");
        const page = (scroll.children ?? [])[0] as GuiNode & Record<string, unknown>;
        assert.equal(page.type, "score");
        assert.equal(page.editable, true);
        assert.equal(page.entry, true);
        assert.ok("kinds" in page && !("notes" in page));
        assert.equal(status.type, "label");
        // the window carries the menu bar, which holds every action
        const bar = (tree as unknown as { menu: { label: string }[] }).menu;
        assert.deepEqual(
            bar.map((title) => title.label),
            ["File", "Edit", "View", "Notes", "Notation", "Measures", "Transform"],
        );
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

    const drawnPage = (editor: ScoreEditor) =>
        pageOf(editor.draw()) as unknown as {
            vb: number[];
            systems: number[][];
        };

    test("the layout is the window's and the paper is fixed", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        assert.equal(editor.layout, "page");
        // the drawing is the paper nobody chose, A4, whatever the music needs
        assert.deepEqual(drawnPage(editor).vb, [21000, 29700]);
        editor.layout = "continuous";
        assert.equal(editor.layout, "continuous");
        assert.equal(drawnPage(editor).systems.length, 1);
        assert.ok(!score.mei().includes("page.width"), "a layout writes nothing");
    });

    test("the page setup is the score's and walks back", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        assert.equal(editor.page.paper, "A4");
        assert.ok(editor.page.papers.includes("Octavo"));
        assert.ok(editor.setPage("Letter", { landscape: true, staff: 800 }));
        const setup = editor.page;
        assert.deepEqual([setup.paper, setup.landscape], ["Letter", true]);
        assert.equal(setup.page.staff, 800);
        assert.equal((score.sheet() as unknown as { page: { width: number } }).page.width, 2794);
        assert.ok(score.mei().includes('page.width="279.4mm"'), "it travels in the document");
        assert.equal(drawnPage(editor).vb[0], 27940);
        assert.ok(editor.undo());
        assert.equal((score.sheet() as unknown as { page?: unknown }).page, undefined);
        assert.equal(editor.setPage("foolscap"), false);
    });

    test("a text of the page is written and placed", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        assert.ok(editor.setText("title", "A title"));
        assert.ok(editor.setText("note", "* a footnote"));
        assert.ok(editor.setText("composer", "A. Composer", { halign: "left", pages: "all" }));
        const head = (score.sheet() as unknown as {
            header: { title: string; notes: string[]; places: Record<string, { halign: string }> };
        }).header;
        assert.deepEqual([head.title, head.notes], ["A title", ["* a footnote"]]);
        assert.equal(head.places.composer.halign, "left");
        // each is drawn under its own id, which is what a press names
        const page = pageOf(editor.draw()) as unknown as {
            prims: { k: string; id?: string; s?: string }[];
            kinds: Record<string, string>;
        };
        const drawn = Object.fromEntries(
            page.prims.filter((p) => p.k === "text" && p.id).map((p) => [p.id, p.s]),
        );
        assert.equal(drawn["t-title"], "A title");
        assert.equal(drawn["t-note-1"], "* a footnote");
        assert.equal(page.kinds["t-title"], "rend");
        assert.equal(editor.setText("motto", "x"), false);
        assert.ok(editor.undo() && editor.undo() && editor.undo());
        assert.equal((score.sheet() as unknown as { header?: unknown }).header, undefined);
    });

    test("the input state is the handle's and a press writes it", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score, { value: [1, 8] });
        editor.draw();
        assert.deepEqual([editor.dotted, editor.rest, editor.nextAccidental], [false, false, null]);
        editor.dotted = true;
        editor.nextAccidental = 1;
        assert.deepEqual([editor.dotted, editor.nextAccidental], [true, 1]);
        const last = items(score).at(-1)!.id;
        const page = editor.view!.widget(editor, "page", editor.structure);
        assert.ok(
            editor.apply("/gui_event", [page, 1, versionOf(editor), "insert", `n${last}`, -3, 0]),
        );
        const written = items(score).at(-1)!;
        assert.deepEqual(written.dur, [3, 16]);
        assert.equal(written.pitches?.[0].alter, 1);
        assert.equal(editor.nextAccidental, null, "it was for that note");
    });

    test("a tool and a menu pick are the editor's verbs", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        editor.draw();
        const first = items(score)[0].id;
        editor.select([`n${first}`]);
        // a tool that acts reports a click
        const staccato = editor.view!.widget(editor, "tool", editor.structure, "stacc");
        assert.ok(editor.apply("/gui_event", [staccato, 1, versionOf(editor), "click"]));
        assert.deepEqual(items(score)[0].marks?.articulations, ["stacc"]);
        // the verbs the menu and the tools use are methods too
        assert.ok(editor.accidental(-1));
        assert.equal(items(score)[0].pitches?.[0].alter, -1);
        assert.ok(editor.voice(1));
        const voices = (score.sheet() as unknown as { staves: { voices: unknown[] }[] }).staves[0]
            .voices;
        assert.equal(voices.length, 2);
        assert.equal(editor.voice(1), false, "it is there already");
        // a measure verb acts on the measures the selection covers
        editor.select([`n${first}`]);
        assert.ok(editor.setBarline("dbl"));
        assert.ok(editor.insertMeasures(2));
        assert.ok(editor.undo() && editor.undo());
        // and a tool that holds state reports its value: an eighth
        const value = editor.view!.widget(editor, "tool", editor.structure, "value");
        editor.apply("/gui_event", [value, 2, versionOf(editor), 3]);
        assert.deepEqual(editor.value, [1, 8]);
    });

    test("a menu entry opens a form and OK writes one entry", async () => {
        const score = await Score.open(PHRASE);
        const editor = new ScoreEditor(score);
        const tree = editor.draw();
        const stack = (tree.children ?? [])[3] as unknown as { flow: string; index: number };
        assert.deepEqual([stack.flow, stack.index], ["stack", 0]);
        const widget = (name: string) =>
            editor.view!.widget(editor, "dialog", editor.structure, name);
        const held = editor as unknown as { windowId: number | null };
        held.windowId ??= 0;
        const window = held.windowId;
        // the bar's entry opens the form; its fields are typed; OK writes them
        editor.apply("/gui_event", [window, 1, versionOf(editor), "menu", "dialog:text"]);
        editor.apply("/gui_event", [widget("text:title"), 2, versionOf(editor), "A title"]);
        editor.apply("/gui_event", [widget("text:notes"), 3, versionOf(editor), "* one\n* two"]);
        const header = () => (score.sheet() as unknown as {
            header?: { title: string; notes: string[] };
        }).header;
        assert.equal(header(), undefined, "nothing until OK");
        assert.ok(editor.apply("/gui_event", [widget("text:ok"), 4, versionOf(editor), "click"]));
        assert.deepEqual([header()?.title, header()?.notes], ["A title", ["* one", "* two"]]);
        assert.ok(editor.undo());
        assert.equal(header(), undefined, "the form was one entry");
    });

    test("edit opens a score in the score editor", async () => {
        const score = await Score.open(PHRASE);
        const editor = await edit(score, { open: false });
        assert.ok(editor instanceof ScoreEditor);
        assert.equal((editor as unknown as ScoreEditor).score, score);
    });
}
