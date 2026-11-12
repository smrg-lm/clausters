// One indentation level, and what Tab inserts with it: src/lib/indent.ts,
// over a state and a dispatch, which is all its command reads of a view.

import { test } from "node:test";
import assert from "node:assert/strict";
import { EditorSelection, EditorState, type TransactionSpec } from "@codemirror/state";
import { indentUnit } from "@codemirror/language";
import type { EditorView } from "@codemirror/view";
import { indentation, tab } from "../src/lib/indent.ts";
import type { IndentPref } from "../src/lib/settings.svelte.ts";

/** `text` with the cursor at `|`, Tab pressed: the document after it. */
function tabbed(text: string, indent: IndentPref): string {
  let state = EditorState.create({
    doc: text.replace("|", ""),
    selection: EditorSelection.cursor(text.indexOf("|")),
    extensions: [indentation(indent)],
  });
  const view = {
    get state() {
      return state;
    },
    dispatch: (...specs: TransactionSpec[]) => (state = state.update(...specs).state),
  };
  assert.ok(tab(view as unknown as EditorView));
  return state.doc.toString();
}

test("a level is the unit the preference names", () => {
  const unit = (indent: IndentPref) =>
    EditorState.create({ extensions: [indentation(indent)] }).facet(indentUnit);
  assert.equal(unit("tab"), "\t");
  assert.equal(unit("2"), "  ");
  assert.equal(unit("4"), "    ");
});

test("Tab with spaces reaches the next stop, not a whole level more", () => {
  assert.equal(tabbed("|x", "4"), "    x");
  assert.equal(tabbed("  |x", "4"), "    x");
  assert.equal(tabbed("ab|c", "4"), "ab  c");
});

test("Tab with tabs inserts one tab character", () => {
  assert.equal(tabbed("  |x", "tab"), "  \tx");
});
