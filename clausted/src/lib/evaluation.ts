// CodeMirror extension: which region each shortcut evaluates, and the visual flash.

import { EditorState, Prec, RangeSetBuilder, StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, EditorView, keymap, type DecorationSet } from "@codemirror/view";
import { ensureSyntaxTree } from "@codemirror/language";

export interface Region {
  from: number;
  to: number;
}

export interface EvalHandlers {
  evaluate(code: string, line: number): void;
  help(word: string): void;
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

/** The top-level statement that holds the cursor (a whole def, for, with...). */
export function topLevelBlock(state: EditorState): Region | null {
  const line = state.doc.lineAt(state.selection.main.head);
  if (!line.text.trim()) return null;
  const tree = ensureSyntaxTree(state, state.doc.length, 500);
  if (!tree) return { from: line.from, to: line.to };

  const pos = line.from + (line.text.length - line.text.trimStart().length);
  let node = tree.resolveInner(pos, 1);
  while (node.parent?.parent) node = node.parent;
  if (!node.parent) return { from: line.from, to: line.to };

  let to = node.to;
  while (to > node.from && /\s/.test(state.doc.sliceString(to - 1, to))) to--;
  return { from: state.doc.lineAt(node.from).from, to };
}

/** The (dotted) name under the cursor, e.g. `os.path.join`. */
export function wordAt(state: EditorState): string {
  const sel = state.selection.main;
  if (!sel.empty) return state.sliceDoc(sel.from, sel.to).trim();
  const line = state.doc.lineAt(sel.head);
  const col = sel.head - line.from;
  const left = /[\w.]*$/.exec(line.text.slice(0, col))![0];
  const right = /^\w*/.exec(line.text.slice(col))![0];
  return (left + right).replace(/^\.+|\.+$/g, "");
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

/** Shift+Enter: the selection or the current line. */
export const evalLine = (v: EditorView, h: EvalHandlers) => run(v, lineOrSelection(v.state), h);

/** Ctrl+Enter: the selection or the top-level statement under the cursor. */
export const evalBlock = (v: EditorView, h: EvalHandlers) =>
  run(v, v.state.selection.main.empty ? topLevelBlock(v.state) : lineOrSelection(v.state), h);

export function evaluation(h: EvalHandlers): Extension {
  return [
    flashField,
    Prec.highest(
      keymap.of([
        { key: "Shift-Enter", run: (v) => evalLine(v, h) },
        { key: "Mod-Enter", run: (v) => evalBlock(v, h) },
        { key: "Mod-.", run: () => (h.interrupt(), true) },
        {
          key: "Mod-d",
          run: (v) => {
            const w = wordAt(v.state);
            if (w) h.help(w);
            return true;
          },
        },
      ]),
    ),
  ];
}
