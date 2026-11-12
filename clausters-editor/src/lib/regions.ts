// Which region of the document each shortcut evaluates: the line or the
// selection, the top-level statement, the contiguous statements, the cell.
// Pure functions of an editor state, with nothing of the view or the settings
// in them, so they are tested on their own (tests/regions.test.ts).

import type { EditorState } from "@codemirror/state";
import { ensureSyntaxTree } from "@codemirror/language";

export interface Region {
  from: number;
  to: number;
}

/** The selection (from the start of its first line if only indentation precedes it) or the current line. */
export function lineOrSelection(state: EditorState): Region | null {
  const sel = state.selection.main;
  if (sel.empty) {
    const line = state.doc.lineAt(sel.head);
    return line.text.trim() ? { from: line.from, to: line.to } : null;
  }
  const first = state.doc.lineAt(sel.from);
  const before = state.doc.sliceString(first.from, sel.from);
  return { from: before.trim() ? sel.from : first.from, to: sel.to };
}

/** The file's top-level statements (and comments), in order, each from the start
 *  of its first line to the end of its code. */
function topLevel(state: EditorState): Region[] {
  const tree = ensureSyntaxTree(state, state.doc.length, 500);
  const out: Region[] = [];
  for (let node = tree?.topNode.firstChild; node; node = node.nextSibling) {
    let to = node.to;
    while (to > node.from && /\s/.test(state.doc.sliceString(to - 1, to))) to--;
    if (to > node.from) out.push({ from: state.doc.lineAt(node.from).from, to });
  }
  return out;
}

/** The index of the top-level statement on the cursor's line, or -1 on a blank line. */
function statementAtCursor(state: EditorState, statements: Region[]): number {
  const line = state.doc.lineAt(state.selection.main.head);
  if (!line.text.trim()) return -1;
  return statements.findIndex((r) => r.from <= line.to && r.to >= line.from);
}

/** The top-level statement that holds the cursor (a whole def, for, with...). */
export function topLevelBlock(state: EditorState): Region | null {
  const statements = topLevel(state);
  const i = statementAtCursor(state, statements);
  if (i >= 0) return statements[i];
  const line = state.doc.lineAt(state.selection.main.head);
  return line.text.trim() ? { from: line.from, to: line.to } : null;
}

/** The top-level statements around the cursor that no blank line separates. A
 *  statement with blank lines inside (a `def`) stays whole, as a block does. */
export function contiguousBlock(state: EditorState): Region | null {
  const statements = topLevel(state);
  const i = statementAtCursor(state, statements);
  if (i < 0) return topLevelBlock(state);
  const line = (pos: number) => state.doc.lineAt(pos).number;
  const touching = (a: Region, b: Region) => line(b.from) <= line(a.to) + 1;
  let first = i;
  let last = i;
  while (first > 0 && touching(statements[first - 1], statements[first])) first--;
  while (last < statements.length - 1 && touching(statements[last], statements[last + 1])) last++;
  return { from: statements[first].from, to: statements[last].to };
}

/** A cell marker, as VS Code and Jupyter write them in a script: `# %%`, optionally
 *  followed by a title (`# %% Setup`) or a kind (`# %% [markdown]`). */
export const isCellMarker = (text: string) => /^#\s*%%/.test(text);

/** The line numbers of the document's cell markers. */
export function markerLines(state: EditorState): number[] {
  const out: number[] = [];
  for (let n = 1; n <= state.doc.lines; n++) if (isCellMarker(state.doc.line(n).text)) out.push(n);
  return out;
}

/** The cell that holds the cursor: the lines between the `markers` around it (or the
 *  start and the end), without the blank lines at its edges; null on an empty cell. */
export function cellAt(state: EditorState, markers: number[]): Region | null {
  const doc = state.doc;
  const here = doc.lineAt(state.selection.main.head).number;
  let start = (markers.filter((m) => m <= here).pop() ?? 0) + 1;
  let end = (markers.find((m) => m > Math.max(here, start - 1)) ?? doc.lines + 1) - 1;
  while (start <= end && !doc.line(start).text.trim()) start++;
  while (end >= start && !doc.line(end).text.trim()) end--;
  return start > end ? null : { from: doc.line(start).from, to: doc.line(end).to };
}
