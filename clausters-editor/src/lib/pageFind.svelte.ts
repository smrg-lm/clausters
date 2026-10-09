// Finding text in the page on screen. The matches are painted with the CSS Highlight
// API, which leaves the page's DOM alone: the page is Svelte's, and nodes taken out
// from under it break the view.

const ALL = "doc-find";
const CURRENT = "doc-find-current";

export class PageFind {
  /** How many matches the page holds, and which one is the current one. */
  count = $state(0);
  index = $state(0);
  #ranges: Range[] = [];
  #root: HTMLElement | null = null;

  clear() {
    CSS.highlights.delete(ALL);
    CSS.highlights.delete(CURRENT);
    this.#ranges = [];
    this.count = 0;
    this.index = 0;
  }

  /** Finds every occurrence of `term` in `root`, ignoring case. The current one is
   *  the first after `after` when it is given, else the first; `scroll` brings it
   *  into view. */
  run(root: HTMLElement, term: string, { after = null as Element | null, scroll = true } = {}) {
    this.clear();
    this.#root = root;
    const needle = term.trim().toLowerCase();
    if (!needle) return;
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = (node as Text).data.toLowerCase();
      for (let at = text.indexOf(needle); at >= 0; at = text.indexOf(needle, at + needle.length)) {
        const range = new Range();
        range.setStart(node, at);
        range.setEnd(node, at + needle.length);
        this.#ranges.push(range);
      }
    }
    this.count = this.#ranges.length;
    if (!this.count) return;
    CSS.highlights.set(ALL, new Highlight(...this.#ranges));
    const first = after
      ? this.#ranges.findIndex((r) => after.compareDocumentPosition(r.startContainer) & Node.DOCUMENT_POSITION_FOLLOWING)
      : 0;
    this.select(Math.max(0, first), scroll);
  }

  /** Moves to the next (`delta` 1) or the previous (-1) match, wrapping around. */
  step(delta: number) {
    this.select(this.index + delta, true);
  }

  select(i: number, scroll: boolean) {
    if (!this.count || !this.#root) return;
    this.index = (i + this.count) % this.count;
    const range = this.#ranges[this.index];
    CSS.highlights.set(CURRENT, new Highlight(range));
    if (!scroll) return;
    const box = this.#root.getBoundingClientRect();
    this.#root.scrollTop += range.getBoundingClientRect().top - box.top - box.height / 2;
  }
}
