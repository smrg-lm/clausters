// CodeMirror extension: which region each shortcut evaluates, and the visual flash.

import { EditorSelection, EditorState, Prec, RangeSetBuilder, StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, keymap, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { ensureSyntaxTree } from "@codemirror/language";
import { settings } from "./settings.svelte";
import { codeAt } from "./code";
import type { CodeAt } from "./session.svelte";

export interface Region {
  from: number;
  to: number;
}

export interface EvalHandlers {
  evaluate(code: string, line: number): void;
  /** Help on the selected text, or on the name at a position (`at`). */
  help(subject: { text: string; at?: CodeAt }): void;
  interrupt(): void;
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
function markerLines(state: EditorState): number[] {
  const out: number[] = [];
  for (let n = 1; n <= state.doc.lines; n++) if (isCellMarker(state.doc.line(n).text)) out.push(n);
  return out;
}

/** The cell that holds the cursor: the lines between the `markers` around it (or the
 *  start and the end), without the blank lines at its edges; null on an empty cell. */
function cellAt(state: EditorState, markers: number[]): Region | null {
  const doc = state.doc;
  const here = doc.lineAt(state.selection.main.head).number;
  let start = (markers.filter((m) => m <= here).pop() ?? 0) + 1;
  let end = (markers.find((m) => m > Math.max(here, start - 1)) ?? doc.lines + 1) - 1;
  while (start <= end && !doc.line(start).text.trim()) start++;
  while (end >= start && !doc.line(end).text.trim()) end--;
  return start > end ? null : { from: doc.line(start).from, to: doc.line(end).to };
}

// --- Flash of the evaluated region ---

const setFlash = StateEffect.define<Region | null>();
const flashLine = Decoration.line({ class: "cm-evalFlash" });

const flashField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    deco = deco.map(tr.changes);
    for (const e of tr.effects) {
      if (!e.is(setFlash)) continue;
      if (!e.value) {
        deco = Decoration.none;
        continue;
      }
      const builder = new RangeSetBuilder<Decoration>();
      const doc = tr.state.doc;
      for (let n = doc.lineAt(e.value.from).number; n <= doc.lineAt(e.value.to).number; n++) {
        const l = doc.line(n);
        builder.add(l.from, l.from, flashLine);
      }
      deco = builder.finish();
    }
    return deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

function flash(view: EditorView, region: Region) {
  view.dispatch({ effects: setFlash.of(region) });
  setTimeout(() => view.dispatch({ effects: setFlash.of(null) }), 250);
}

function run(view: EditorView, region: Region | null, h: EvalHandlers): boolean {
  if (!region) return true;
  const code = view.state.sliceDoc(region.from, region.to);
  flash(view, region);
  h.evaluate(code, view.state.doc.lineAt(region.from).number);
  return true;
}

/** Puts the cursor at the start of the line after `region` -- after the next cell's
 *  marker when the region was a cell -- so that evaluating again goes on from there. */
function advance(view: EditorView, region: Region, cell: boolean) {
  const doc = view.state.doc;
  let n = doc.lineAt(region.to).number + 1;
  if (cell) {
    while (n <= doc.lines && !isCellMarker(doc.line(n).text)) n++;
    n++;
  }
  if (n > doc.lines) return;
  view.dispatch({ selection: EditorSelection.cursor(doc.line(n).from), scrollIntoView: true });
}

/** Shift+Enter: the selection or the current line. */
export const evalLine = (v: EditorView, h: EvalHandlers) => run(v, lineOrSelection(v.state), h);

/** Ctrl+Enter: the selection; otherwise the cell under the cursor in a document with
 *  `# %%` markers, the contiguous lines around it when that option is on, or the
 *  top-level statement. The cursor then moves past what was evaluated. */
export function evalBlock(v: EditorView, h: EvalHandlers): boolean {
  const state = v.state;
  const markers = state.selection.main.empty ? markerLines(state) : [];
  const cell = markers.length > 0;
  const region = !state.selection.main.empty
    ? lineOrSelection(state)
    : cell
      ? cellAt(state, markers)
      : settings.evalParagraph
        ? contiguousBlock(state)
        : topLevelBlock(state);
  if (!region) return true;
  run(v, region, h);
  advance(v, region, cell);
  return true;
}

// --- Cell markers: a rule above each `# %%` line ---

const cellLine = Decoration.line({ class: "cm-cellMarker" });

function cellDecorations(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of view.visibleRanges) {
    for (let pos = from; pos <= to; ) {
      const line = view.state.doc.lineAt(pos);
      if (isCellMarker(line.text)) builder.add(line.from, line.from, cellLine);
      pos = line.to + 1;
    }
  }
  return builder.finish();
}

const cellRules = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = cellDecorations(view);
    }
    update(u: ViewUpdate) {
      if (u.docChanged || u.viewportChanged) this.decorations = cellDecorations(u.view);
    }
  },
  { decorations: (p) => p.decorations },
);

export function evaluation(h: EvalHandlers): Extension {
  return [
    flashField,
    cellRules,
    Prec.highest(
      keymap.of([
        { key: "Shift-Enter", run: (v) => evalLine(v, h) },
        { key: "Mod-Enter", run: (v) => evalBlock(v, h) },
        { key: "Mod-.", run: () => (h.interrupt(), true) },
        {
          key: "Mod-d",
          run: (v) => {
            const sel = v.state.selection.main;
            if (!sel.empty) {
              h.help({ text: v.state.sliceDoc(sel.from, sel.to).trim() });
            } else {
              const word = v.state.wordAt(sel.head);
              if (word) h.help({ text: v.state.sliceDoc(word.from, word.to), at: codeAt(v.state, word.to) });
            }
            return true;
          },
        },
      ]),
    ),
  ];
}
