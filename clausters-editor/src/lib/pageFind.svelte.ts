// Finding text in the page on screen. The matches are marked in the page's HTML
// before it is shown (`markMatches`), so the page is drawn with them like any other
// element: painting them over the page afterwards (the CSS Highlight API) left the
// marks of earlier searches on screen in the app's web view, and changing the
// page's DOM in place breaks the view, which is Svelte's.

/** `html` with every occurrence of `term`, ignoring case, inside a `<mark>`. Text in
 *  tags (attributes, element names) is never touched: only the text nodes are. */
export function markMatches(html: string, term: string): string {
  const needle = term.trim().toLowerCase();
  if (!needle) return html;
  const template = document.createElement("template");
  template.innerHTML = html;
  const walker = document.createTreeWalker(template.content, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  for (let n = walker.nextNode(); n; n = walker.nextNode()) nodes.push(n as Text);
  for (const node of nodes) {
    const text = node.data;
    const lower = text.toLowerCase();
    let at = lower.indexOf(needle);
    if (at < 0) continue;
    const parts = document.createDocumentFragment();
    let last = 0;
    for (; at >= 0; at = lower.indexOf(needle, last)) {
      parts.append(text.slice(last, at));
      const mark = document.createElement("mark");
      mark.className = "doc-find";
      mark.textContent = text.slice(at, at + needle.length);
      parts.append(mark);
      last = at + needle.length;
    }
    parts.append(text.slice(last));
    node.replaceWith(parts);
  }
  return template.innerHTML;
}

/** The marked matches of the page on screen, and which one is the current one. */
export class PageFind {
  count = $state(0);
  index = $state(0);
  #marks: HTMLElement[] = [];
  #root: HTMLElement | null = null;

  /** Takes the marks `root` holds now (after the page was drawn with them). The
   *  current one is the first after `after` when it is given, else the first;
   *  `scroll` brings it into view. */
  attach(root: HTMLElement, { after = null as Element | null, scroll = true } = {}) {
    this.#root = root;
    this.#marks = [...root.querySelectorAll<HTMLElement>("mark.doc-find")];
    this.count = this.#marks.length;
    this.index = 0;
    if (!this.count) return;
    const first = after
      ? this.#marks.findIndex((m) => after.compareDocumentPosition(m) & Node.DOCUMENT_POSITION_FOLLOWING)
      : 0;
    this.select(Math.max(0, first), scroll);
  }

  /** Moves to the next (`delta` 1) or the previous (-1) match, wrapping around. */
  step(delta: number) {
    this.select(this.index + delta, true);
  }

  select(i: number, scroll: boolean) {
    if (!this.count || !this.#root) return;
    this.#marks[this.index]?.classList.remove("doc-find-current");
    this.index = (i + this.count) % this.count;
    const mark = this.#marks[this.index];
    mark.classList.add("doc-find-current");
    if (!scroll) return;
    const box = this.#root.getBoundingClientRect();
    this.#root.scrollTop += mark.getBoundingClientRect().top - box.top - box.height / 2;
  }
}
