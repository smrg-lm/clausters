// Where the cursor is, in the terms the session's code intelligence reads.

import type { EditorState } from "@codemirror/state";
import { syntaxTree } from "@codemirror/language";
import type { CodeAt } from "./session.svelte";

/** The editor's text and a position in it, as Jedi reads them. */
export function codeAt(state: EditorState, pos: number): CodeAt {
  const line = state.doc.lineAt(pos);
  return { source: state.doc.toString(), line: line.number, column: pos - line.from };
}

/** The document position of a 1-based line and a 0-based column. */
export function posOf(state: EditorState, line: number, column: number): number {
  const l = state.doc.line(Math.min(Math.max(line, 1), state.doc.lines));
  return Math.min(l.from + column, l.to);
}

/** Whether `pos` is inside a string or a comment, where no name is code. `side`:
 *  -1 reads the character before `pos`, 1 the one after. */
export function inStringOrComment(state: EditorState, pos: number, side: -1 | 1): boolean {
  for (let node = syntaxTree(state).resolveInner(pos, side); node; node = node.parent!) {
    if (/String|Comment/.test(node.name)) return true;
    if (!node.parent) break;
  }
  return false;
}
