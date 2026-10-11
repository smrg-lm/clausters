// Indentation: what one level is (spaces or a tab, and how wide), and what the
// Tab key does with it.

import { EditorSelection, EditorState, countColumn, type Extension } from "@codemirror/state";
import { indentUnit } from "@codemirror/language";
import { indentLess, indentMore } from "@codemirror/commands";
import { keymap, type Command } from "@codemirror/view";
import type { IndentPref } from "./settings.svelte";

/** The width a tab character is drawn with. */
const TAB_WIDTH = 4;

/** One indentation level as the editor's state reads it: the unit that Enter,
 *  the indent commands and Tab insert. */
export function indentation(indent: IndentPref): Extension {
  const unit = indent === "tab" ? "\t" : " ".repeat(Number(indent));
  return [indentUnit.of(unit), EditorState.tabSize.of(TAB_WIDTH)];
}

/**
 * Tab: with a selection, indents the lines it touches; without one, inserts one
 * level at the cursor, wherever it is in the line -- at its start that indents
 * the line, in the middle of the text it aligns what follows.
 *
 * With spaces it inserts as many as reach the next indentation stop (a line
 * indented by two takes two more, not four), with tabs one tab character.
 */
const tab: Command = (view) => {
  const { state } = view;
  if (state.readOnly) return false;
  if (state.selection.ranges.some((r) => !r.empty)) return indentMore(view);
  const unit = state.facet(indentUnit);
  view.dispatch(
    state.changeByRange((r) => {
      const line = state.doc.lineAt(r.head);
      const column = countColumn(line.text.slice(0, r.head - line.from), state.tabSize);
      const insert = unit === "\t" ? "\t" : " ".repeat(unit.length - (column % unit.length));
      return { changes: { from: r.head, insert }, range: EditorSelection.cursor(r.head + insert.length) };
    }),
    { scrollIntoView: true, userEvent: "input.indent" },
  );
  return true;
};

/** Tab and Shift+Tab, in place of CodeMirror's `indentWithTab`. */
export const tabKeymap = keymap.of([{ key: "Tab", run: tab, shift: indentLess }]);
