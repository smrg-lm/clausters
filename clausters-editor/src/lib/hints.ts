// Documentation in the editor from the live session: a tooltip on hover (signature
// and docstring) and the parameters of the call being typed. Both are Jedi's answers
// (see driver.py), which read the text and the session's objects together.

import { StateEffect, StateField, type Extension } from "@codemirror/state";
import {
  EditorView,
  ViewPlugin,
  hoverTooltip,
  keymap,
  showTooltip,
  tooltips,
  type Tooltip,
  type ViewUpdate,
} from "@codemirror/view";
import { codeAt, inStringOrComment, posOf } from "./code";
import type { CodeAt, PyInfo, PySignature } from "./session.svelte";

export interface HintSources {
  inspect(at: CodeAt): Promise<PyInfo | null>;
  signatures(at: CodeAt): Promise<PySignature[]>;
}

// The same look as shadcn's popovers; `!` because CodeMirror's theme also styles
// .cm-tooltip. The size follows the editor's font size (zoom).
const BOX =
  "max-w-[min(48rem,90vw)] rounded-md! border! border-border! bg-popover! px-3 py-2 " +
  "text-popover-foreground! shadow-md text-[length:calc(var(--code-size)*0.9)]";

function el(tag: string, cls: string, text?: string) {
  const e = document.createElement(tag);
  e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function renderInfo(info: PyInfo): HTMLElement {
  const dom = el("div", BOX);
  const head = el("div", "flex items-baseline gap-2");
  head.append(el("code", "font-mono font-medium", info.signature || info.name));
  head.append(el("span", "text-muted-foreground", info.kind));
  dom.append(head);
  if (info.doc) dom.append(el("div", "mt-1.5 max-h-60 overflow-auto whitespace-pre-wrap text-muted-foreground", info.doc));
  return dom;
}

function renderSignature(sig: PySignature): HTMLElement {
  const dom = el("div", BOX);
  const code = el("code", "font-mono");
  code.append(sig.name + "(");
  sig.params.forEach((p, i) => {
    if (i > 0) code.append(", ");
    code.append(i === sig.index ? el("span", "font-bold text-foreground underline underline-offset-2", p) : p);
  });
  code.append(")");
  dom.append(code);
  if (sig.doc) dom.append(el("div", "mt-1 line-clamp-3 whitespace-pre-wrap text-muted-foreground", sig.doc));
  return dom;
}

// --- Tooltip on hover ---

function hover(src: HintSources) {
  return hoverTooltip(
    async (view, pos, side) => {
      const word = view.state.wordAt(pos);
      if (!word || inStringOrComment(view.state, pos, side < 0 ? -1 : 1)) return null;
      const info = await src.inspect(codeAt(view.state, word.to));
      if (!info) return null;
      return { pos: word.from, end: word.to, above: true, create: () => ({ dom: renderInfo(info) }) };
    },
    { hoverTime: 350 },
  );
}

// --- The parameters of the call being typed ---

const setSignature = StateEffect.define<Tooltip | null>();

const signatureField = StateField.define<Tooltip | null>({
  create: () => null,
  update(tooltip, tr) {
    for (const e of tr.effects) if (e.is(setSignature)) return e.value;
    return tooltip && tr.docChanged ? { ...tooltip, pos: tr.changes.mapPos(tooltip.pos) } : tooltip;
  },
  provide: (f) => showTooltip.from(f),
});

function signaturePlugin(src: HintSources) {
  return ViewPlugin.fromClass(
    class {
      /** Turned on by typing "(" or ","; it then follows the cursor while it stays in a call. */
      active = false;
      timer: ReturnType<typeof setTimeout> | undefined;
      /** Counts the requests, so that a late answer does not replace a newer one. */
      asked = 0;

      constructor(readonly view: EditorView) {}

      update(u: ViewUpdate) {
        if (!u.docChanged && !u.selectionSet) return;
        let typed = false;
        u.changes.iterChanges((_a, _b, _c, _d, ins) => {
          if (/[(,]/.test(ins.toString())) typed = true;
        });
        if (typed) this.active = true;
        if (this.active) this.schedule();
      }

      schedule() {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => this.refresh(), 120);
      }

      async refresh() {
        const { state } = this.view;
        const sel = state.selection.main;
        if (!sel.empty || inStringOrComment(state, sel.head, -1)) return this.dismiss();
        const ask = ++this.asked;
        const [sig] = await src.signatures(codeAt(state, sel.head));
        if (ask !== this.asked || !this.active) return;
        if (!sig) return this.dismiss();
        const pos = posOf(this.view.state, sig.bracket[0], sig.bracket[1]);
        this.view.dispatch({
          effects: setSignature.of({ pos, above: true, create: () => ({ dom: renderSignature(sig) }) }),
        });
      }

      /** Hides the parameters and stops following the cursor until the next "(". */
      dismiss() {
        this.active = false;
        if (this.view.state.field(signatureField, false)) this.view.dispatch({ effects: setSignature.of(null) });
      }

      destroy() {
        clearTimeout(this.timer);
      }
    },
  );
}

export function pythonHints(src: HintSources): Extension {
  const plugin = signaturePlugin(src);
  return [
    // In the body, not inside the editor: the panel holding it would clip the tooltips.
    tooltips({ parent: document.body }),
    hover(src),
    signatureField,
    plugin,
    keymap.of([
      {
        key: "Escape",
        run: (view) => {
          if (!view.state.field(signatureField, false)) return false;
          view.plugin(plugin)?.dismiss();
          return true;
        },
      },
      {
        key: "Mod-Shift-Space",
        run: (view) => {
          const p = view.plugin(plugin);
          if (!p) return false;
          p.active = true;
          p.schedule();
          return true;
        },
      },
    ]),
  ];
}
