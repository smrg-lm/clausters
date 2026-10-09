// Completions from the live session, through Jedi (see driver.py): it reads the text
// and the session's objects together, so it offers what a `from module import *`
// brought in, an object's attributes, and what a call returns.

import { pythonLanguage } from "@codemirror/lang-python";
import type { EditorState, Extension } from "@codemirror/state";
import { codeAt, inStringOrComment } from "./code";
import type { CodeAt, PyCompletion } from "./session.svelte";

// The part of @codemirror/autocomplete's CompletionContext this source reads.
interface Context {
  state: EditorState;
  pos: number;
  explicit: boolean;
  matchBefore(re: RegExp): { from: number; to: number; text: string } | null;
}

/** Jedi's kinds as CodeMirror's completion icons. */
const TYPES: Record<string, string> = {
  module: "namespace",
  class: "class",
  function: "function",
  instance: "variable",
  statement: "variable",
  param: "variable",
  property: "property",
  keyword: "keyword",
  path: "text",
};

export function sessionCompletions(complete: (at: CodeAt) => Promise<PyCompletion[]>): Extension {
  return pythonLanguage.data.of({
    autocomplete: async (ctx: Context) => {
      // While typing, from a name's first character or right after a dot.
      const word = ctx.matchBefore(/\w+$/);
      const afterDot = ctx.state.sliceDoc(ctx.pos - 1, ctx.pos) === ".";
      if (!word && !afterDot && !ctx.explicit) return null;
      if (inStringOrComment(ctx.state, ctx.pos, -1)) return null;
      const found = await complete(codeAt(ctx.state, ctx.pos));
      if (!found.length) return null;
      return {
        from: ctx.pos - found[0].typed,
        options: found.map((c) => ({ label: c.name, type: TYPES[c.kind] ?? "variable" })),
        validFor: /^\w*$/,
      };
    },
  });
}
