// CodeMirror extension: which region each shortcut evaluates, and the visual flash.

import { EditorSelection, Prec, RangeSetBuilder, StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, keymap, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { settings } from "./settings.svelte";
import {
  cellAt,
  contiguousBlock,
  isCellMarker,
  lineOrSelection,
  markerLines,
  topLevelBlock,
  type Region,
} from "./regions";
import { codeAt } from "./code";
import type { CodeAt } from "./session.svelte";

export interface EvalHandlers {
  evaluate(code: string, line: number): void;
  /** Help on the selected text, or on the name at a position (`at`). */
  help(subject: { text: string; at?: CodeAt }): void;
  interrupt(): void;
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
