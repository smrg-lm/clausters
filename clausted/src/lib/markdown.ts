// Markdown -> sanitized HTML, with Python blocks highlighted in the editor's colors.

import MarkdownIt from "markdown-it";
import DOMPurify from "dompurify";
import { highlightCode } from "@lezer/highlight";
import { parser as pythonParser } from "@lezer/python";
import { buttonVariants } from "$lib/components/ui/button";
import { syntaxStyle } from "./editorTheme";

const escape = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

function highlightPython(code: string, dark: boolean): string {
  let html = "";
  highlightCode(
    code,
    pythonParser.parse(code),
    dark ? syntaxStyle.dark : syntaxStyle.light,
    (text, classes) => (html += classes ? `<span class="${classes}">${escape(text)}</span>` : escape(text)),
    () => (html += "\n"),
  );
  return html;
}

// Lucide's "play" icon (ISC), inline because this HTML does not go through Svelte.
const PLAY_ICON =
  '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" ' +
  'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polygon points="6 3 20 12 6 21 6 3"/></svg>';

const md = new MarkdownIt({ html: true, linkify: true, typographer: true });

// GitHub-style ids on headings, for `doc.md#section` links.
export const slugify = (s: string) =>
  s.trim().toLowerCase().replace(/[^\p{L}\p{N}\s-]/gu, "").replace(/\s+/g, "-");

md.core.ruler.push("heading_ids", (state) => {
  const tokens = state.tokens;
  for (let i = 0; i < tokens.length - 1; i++) {
    if (tokens[i].type === "heading_open") tokens[i].attrSet("id", slugify(tokens[i + 1].content));
  }
});

// ```python blocks carry a button that evaluates them in the session.
const evalButtonClass = buttonVariants({ variant: "ghost", size: "icon-sm" }) + " eval-block";

md.renderer.rules.fence = (tokens, idx, _opts, env) => {
  const token = tokens[idx];
  const lang = token.info.trim().split(/\s+/)[0].toLowerCase();
  const code = token.content;
  if (lang === "python" || lang === "py") {
    const e = env as unknown as Env;
    const i = e.blocks.push(code) - 1;
    return (
      `<div class="code-block"><button class="${evalButtonClass}" data-block="${i}" title="Evaluate in the session" aria-label="Evaluate in the session">${PLAY_ICON}</button>` +
      `<pre><code class="language-python">${highlightPython(code, e.dark)}</code></pre></div>`
    );
  }
  return `<pre><code>${escape(code)}</code></pre>`;
};

// `type` rather than `interface`: markdown-it requires a type with an implicit index signature.
type Env = {
  blocks: string[];
  dark: boolean;
};

export interface Rendered {
  html: string;
  /** The code of each Python block, indexed by `data-block`. */
  blocks: string[];
}

export function renderMarkdown(source: string, dark: boolean): Rendered {
  const env: Env = { blocks: [], dark };
  const html = DOMPurify.sanitize(md.render(source, env));
  return { html, blocks: env.blocks };
}
