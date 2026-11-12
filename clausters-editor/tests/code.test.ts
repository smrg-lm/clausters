// Where the cursor is, in the terms the session's code intelligence reads:
// src/lib/code.ts.

import { test } from "node:test";
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { python } from "@codemirror/lang-python";
import { codeAt, inStringOrComment, posOf } from "../src/lib/code.ts";

const state = (doc: string) => EditorState.create({ doc, extensions: [python()] });

test("a position is a 1-based line and a 0-based column, and back", () => {
  const s = state("import os\nos.path.join\n");
  const pos = s.doc.toString().indexOf("path");
  assert.deepEqual(codeAt(s, pos), { source: s.doc.toString(), line: 2, column: 3 });
  assert.equal(posOf(s, 2, 3), pos);
});

test("a position past the document is clamped to it", () => {
  const s = state("ab\ncd");
  assert.equal(posOf(s, 9, 0), s.doc.line(2).from);
  assert.equal(posOf(s, 1, 99), s.doc.line(1).to);
  assert.equal(posOf(s, 0, 1), 1);
});

test("a name inside a string or a comment is not code", () => {
  const doc = 'x = "inside" # remark\ny = x\n';
  const s = state(doc);
  assert.ok(inStringOrComment(s, doc.indexOf("side"), 1));
  assert.ok(inStringOrComment(s, doc.indexOf("mark"), 1));
  assert.ok(!inStringOrComment(s, doc.indexOf("y = x"), 1));
});
