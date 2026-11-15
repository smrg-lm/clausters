// Completions from the live session. In Python they are Jedi's (see driver.py),
// which reads the text and the session's objects together, so it offers what a
// `from module import *` brought in, an object's attributes, and what a call
// returns; in JavaScript the TypeScript language service's (see driver.mjs),
// which reads the text and the types of what it imports.

import type { Language } from "@codemirror/language";
import type { EditorState, Extension } from "@codemirror/state";
import { codeAt, inStringOrComment } from "./code";
import type { CodeAt, Completion } from "./session.svelte";

// The part of @codemirror/autocomplete's CompletionContext this source reads.
interface Context {
  state: EditorState;
  pos: number;
  explicit: boolean;
  matchBefore(re: RegExp): { from: number; to: number; text: string } | null;
}

/** Jedi's kinds, which both drivers answer with, as CodeMirror's completion icons. */
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

export function sessionCompletions(language: Language, complete: (at: CodeAt) => Promise<Completion[]>): Extension {
  return language.data.of({
    autocomplete: async (ctx: Context) => {
      // While typing, from a name's first character or right after a dot.
      const word = ctx.matchBefore(/[\w$]+$/);
      const afterDot = ctx.state.sliceDoc(ctx.pos - 1, ctx.pos) === ".";
      if (!word && !afterDot && !ctx.explicit) return null;
      if (inStringOrComment(ctx.state, ctx.pos, -1)) return null;
      const found = await complete(codeAt(ctx.state, ctx.pos));
      if (!found.length) return null;
      return {
        from: ctx.pos - found[0].typed,
        options: found.map((c) => ({ label: c.name, type: TYPES[c.kind] ?? "variable" })),
        validFor: /^[\w$]*$/,
      };
    },
  });
}
