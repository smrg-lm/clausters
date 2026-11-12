// Which region each shortcut evaluates: src/lib/regions.ts over an editor
// state with the Python language, as the editor holds one.

import { test } from "node:test";
import assert from "node:assert/strict";
import { EditorSelection, EditorState } from "@codemirror/state";
import { python } from "@codemirror/lang-python";
import {
  cellAt,
  contiguousBlock,
  isCellMarker,
  lineOrSelection,
  markerLines,
  topLevelBlock,
  type Region,
} from "../src/lib/regions.ts";

/** A document with the cursor where `|` is (the bar removed). */
function at(text: string): EditorState {
  const pos = text.indexOf("|");
  return EditorState.create({
    doc: text.replace("|", ""),
    selection: EditorSelection.cursor(pos),
    extensions: [python()],
  });
}

const slice = (state: EditorState, r: Region | null) => (r ? state.sliceDoc(r.from, r.to) : null);

test("the line under the cursor, or nothing on a blank one", () => {
  const s = at("a = 1\nb = |2\n");
  assert.equal(slice(s, lineOrSelection(s)), "b = 2");
  const blank = at("a = 1\n|\nb = 2");
  assert.equal(lineOrSelection(blank), null);
});

test("a selection starts at its line when only indentation precedes it", () => {
  const doc = "def f():\n    x = 1\n    return x\n";
  const from = doc.indexOf("x = 1");
  const s = EditorState.create({ doc, selection: EditorSelection.range(from, doc.indexOf("x\n", from + 5) + 1) });
  assert.equal(slice(s, lineOrSelection(s)), "    x = 1\n    return x");
});

test("the top-level statement holds the whole def the cursor is in", () => {
  const s = at("import os\n\ndef f():\n    a = 1\n\n    return |a\n\nprint(f())\n");
  assert.equal(slice(s, topLevelBlock(s)), "def f():\n    a = 1\n\n    return a");
});

test("contiguous statements run until a blank line, and a def stays whole", () => {
  const s = at("x = 1\ny = |2\nz = 3\n\nw = 4\n");
  assert.equal(slice(s, contiguousBlock(s)), "x = 1\ny = 2\nz = 3");
  const d = at("def f():\n    a = 1\n\n    return a\ng = |f()\n\nh = 0\n");
  assert.equal(slice(d, contiguousBlock(d)), "def f():\n    a = 1\n\n    return a\ng = f()");
});

test("a cell marker is a comment that starts with %%, with or without a title", () => {
  assert.ok(isCellMarker("# %%"));
  assert.ok(isCellMarker("#%% Setup"));
  assert.ok(isCellMarker("# %% [markdown]"));
  assert.ok(!isCellMarker("x = 1  # %%"));
  assert.ok(!isCellMarker("# % not a cell"));
});

test("the cell under the cursor, without the blank lines at its edges", () => {
  const s = at("# %% one\na = 1\n\n# %% two\n\nb = |2\nc = 3\n\n# %% three\nd = 4\n");
  assert.deepEqual(markerLines(s), [1, 4, 9]);
  assert.equal(slice(s, cellAt(s, markerLines(s))), "b = 2\nc = 3");
  const empty = at("# %% one\n|\n# %% two\nx = 1\n");
  assert.equal(cellAt(empty, markerLines(empty)), null);
});
