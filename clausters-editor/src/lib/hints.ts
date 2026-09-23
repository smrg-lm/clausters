// Documentation in the editor from the live session: a tooltip on hover
// (signature and docstring) and parameter help while typing a call.

import { StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
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
import { syntaxTree } from "@codemirror/language";
import type { PyInfo } from "./session.svelte";

type Inspect = (expr: string) => Promise<PyInfo | null>;

const KEYWORDS = new Set(
  ("False None True and as assert async await break class continue def del elif else except " +
    "finally for from global if import in is lambda nonlocal not or pass raise return try while with yield")
    .split(" "),
);
const DOTTED = /^[A-Za-z_]\w*(\.[A-Za-z_]\w*)*$/;

// The same look as shadcn's popovers; `!` because CodeMirror's theme also styles
// .cm-tooltip. The size follows the editor's font size (zoom).
const BOX =
  "max-w-[min(48rem,90vw)] rounded-md! border! border-border! bg-popover! px-3 py-2 " +
  "text-popover-foreground! shadow-md text-[length:calc(var(--code-size)*0.9)]";

/** The dotted name that ends in the word under `pos` (`os.path` over "path"). */
function dottedNameAt(state: EditorState, pos: number) {
  const line = state.doc.lineAt(pos);
  const text = line.text;
  let start = pos - line.from;
  let end = start;
  while (end < text.length && /\w/.test(text[end])) end++;
  while (start > 0 && /[\w.]/.test(text[start - 1])) start--;
  const name = text.slice(start, end).replace(/^\.+/, "");
  if (!name || !DOTTED.test(name)) return null;
  return { from: line.from + end - name.length, to: line.from + end, name };
}

function inStringOrComment(state: EditorState, pos: number): boolean {
  for (let node: { name: string; parent: any } | null = syntaxTree(state).resolveInner(pos, 1); node; node = node.parent) {
    if (/String|Comment/.test(node.name)) return true;
  }
  return false;
}

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
  if (info.doc) {
    const doc = el("div", "mt-1.5 max-h-60 overflow-auto whitespace-pre-wrap text-muted-foreground", info.doc);
    dom.append(doc);
  }
  return dom;
}

// --- Tooltip on hover ---

function hover(inspect: Inspect) {
  return hoverTooltip(
    async (view, pos) => {
      const word = dottedNameAt(view.state, pos);
      if (!word || KEYWORDS.has(word.name) || inStringOrComment(view.state, pos)) return null;
      const info = await inspect(word.name);
      if (!info) return null;
      return { pos: word.from, end: word.to, above: true, create: () => ({ dom: renderInfo(info) }) };
    },
    { hoverTime: 350 },
  );
}

// --- Parameter help ---

interface CallContext {
  callee: string;
  open: number; // position of the call's "("
  argIndex: number;
  argName: string | null; // if the current argument is `name=...`
}

/** The call whose parentheses hold `pos`, counting the current level's commas. */
function callContext(state: EditorState, pos: number): CallContext | null {
  const start = Math.max(0, pos - 3000);
  const text = state.sliceDoc(start, pos);
  let depth = 0;
  let commas = 0;
  let lastComma = -1;
  let i = text.length - 1;
  for (; i >= 0; i--) {
    const c = text[i];
    if (c === ")" || c === "]" || c === "}") depth++;
    else if (c === "(" || c === "[" || c === "{") {
      if (depth === 0) {
        if (c !== "(") return null;
        break;
      }
      depth--;
    } else if (c === "," && depth === 0) {
      if (lastComma < 0) lastComma = i;
      commas++;
    }
  }
  if (i < 0 || inStringOrComment(state, start + i)) return null;
  const before = text.slice(0, i);
  const m = /([A-Za-z_][\w.]*)\s*$/.exec(before);
  if (!m || !DOTTED.test(m[1]) || KEYWORDS.has(m[1])) return null;
  if (/\b(def|class)\s+$/.test(before.slice(0, m.index))) return null; // a definition, not a call
  const current = text.slice((lastComma >= 0 ? lastComma : i) + 1);
  const argName = /^\s*([A-Za-z_]\w*)\s*=(?!=)/.exec(current)?.[1] ?? null;
  return { callee: m[1], open: start + i, argIndex: commas, argName };
}

const paramName = (p: string) => p.replace(/^\*+/, "").split(/[:=]/)[0].trim();

function activeParam(params: string[], ctx: CallContext): number {
  if (ctx.argName) return params.findIndex((p) => paramName(p) === ctx.argName);
  if (ctx.argIndex < params.length && !params[ctx.argIndex].startsWith("**")) {
    // Past a *args, every positional goes to it.
    const star = params.findIndex((p) => p.startsWith("*") && !p.startsWith("**"));
    return star >= 0 && ctx.argIndex > star ? star : ctx.argIndex;
  }
  return params.findIndex((p) => p.startsWith("*") && !p.startsWith("**"));
}

function renderSignature(info: PyInfo, ctx: CallContext): HTMLElement {
  const dom = el("div", BOX);
  const sig = el("code", "font-mono");
  if (info.params) {
    const current = activeParam(info.params, ctx);
    sig.append(info.name.split(".").pop() + "(");
    info.params.forEach((p, i) => {
      if (i > 0) sig.append(", ");
      sig.append(i === current ? el("span", "font-bold text-foreground underline underline-offset-2", p) : p);
    });
    sig.append(")");
  } else {
    sig.textContent = info.signature;
  }
  dom.append(sig);
  const summary = info.doc.split(/\n\s*\n/)[0];
  if (summary) dom.append(el("div", "mt-1 line-clamp-3 whitespace-pre-wrap text-muted-foreground", summary));
  return dom;
}

const setSignature = StateEffect.define<Tooltip | null>();

const signatureField = StateField.define<Tooltip | null>({
  create: () => null,
  update(tooltip, tr) {
    for (const e of tr.effects) if (e.is(setSignature)) return e.value;
    return tooltip && tr.docChanged ? { ...tooltip, pos: tr.changes.mapPos(tooltip.pos) } : tooltip;
  },
  provide: (f) => showTooltip.from(f),
});

function signaturePlugin(inspect: Inspect) {
  return ViewPlugin.fromClass(
    class {
      active = false;
      callee = "";
      open = -1;
      info: PyInfo | null = null;
      timer: ReturnType<typeof setTimeout> | undefined;

      constructor(readonly view: EditorView) {}

      update(u: ViewUpdate) {
        if (!u.docChanged && !u.selectionSet) return;
        // Turned on by typing "(" or "," (with auto-closing, "(" arrives as "()");
        // after that it follows the cursor while it stays inside the call.
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
        const ctx = sel.empty ? callContext(state, sel.head) : null;
        if (!ctx) return this.dismiss();
        if (ctx.callee !== this.callee || ctx.open !== this.open) {
          this.callee = ctx.callee;
          this.open = ctx.open;
          this.info = await inspect(ctx.callee);
          if (this.callee !== ctx.callee || !this.active) return; // changed while waiting
        }
        const info = this.info;
        if (!info || (!info.params && !info.signature)) return this.dismiss(false);
        this.view.dispatch({
          effects: setSignature.of({
            pos: ctx.open,
            above: true,
            create: () => ({ dom: renderSignature(info, ctx) }),
          }),
        });
      }

      /** Hides the help; with `deactivate` it stops following the cursor until the next "(". */
      dismiss(deactivate = true) {
        if (deactivate) {
          this.active = false;
          this.callee = "";
          this.open = -1;
        }
        if (this.view.state.field(signatureField, false)) {
          this.view.dispatch({ effects: setSignature.of(null) });
        }
      }

      destroy() {
        clearTimeout(this.timer);
      }
    },
  );
}

export function pythonHints(inspect: Inspect): Extension {
  const plugin = signaturePlugin(inspect);
  return [
    // In the body, not inside the editor: the panel holding it would clip the tooltips.
    tooltips({ parent: document.body }),
    hover(inspect),
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
