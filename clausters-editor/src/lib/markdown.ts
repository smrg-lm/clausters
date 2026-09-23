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

// Lucide's "copy" and "check" icons (ISC), inline because this HTML does not go through Svelte.
const icon = (body: string) =>
  '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" ' +
  `stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
export const COPY_ICON = icon(
  '<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
);
export const COPIED_ICON = icon('<path d="M20 6 9 17l-5-5"/>');

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

// Every code block carries a button that copies it; ```python blocks are highlighted.
const copyButtonClass = buttonVariants({ variant: "ghost", size: "icon-sm" }) + " copy-block";

md.renderer.rules.fence = (tokens, idx, _opts, env) => {
  const token = tokens[idx];
  const lang = token.info.trim().split(/\s+/)[0].toLowerCase();
  const code = token.content;
  const e = env as unknown as Env;
  const i = e.blocks.push(code) - 1;
  const python = lang === "python" || lang === "py";
  return (
    `<div class="code-block"><button class="${copyButtonClass}" data-block="${i}" title="Copy" aria-label="Copy">${COPY_ICON}</button>` +
    (python
      ? `<pre><code class="language-python">${highlightPython(code, e.dark)}</code></pre></div>`
      : `<pre><code>${escape(code)}</code></pre></div>`)
  );
};

// `type` rather than `interface`: markdown-it requires a type with an implicit index signature.
type Env = {
  blocks: string[];
  dark: boolean;
};

export interface Rendered {
  html: string;
  /** The code of each block, indexed by `data-block`. */
  blocks: string[];
}

export function renderMarkdown(source: string, dark: boolean): Rendered {
  const env: Env = { blocks: [], dark };
  const html = DOMPurify.sanitize(md.render(source, env));
  return { html, blocks: env.blocks };
}
