// The documentation's full-text index: every section of every document, searched
// with MiniSearch (prefix and fuzzy matching, a heading weighing more than its text).

import MiniSearch, { type SearchResult } from "minisearch";

/** A document's section: the text from one heading to the next. */
interface Section {
  id: number;
  path: string;
  heading: string | null;
  text: string;
}

/** A section that matches a search. */
export interface DocResult {
  path: string;
  heading: string | null;
  /** The section's first line that holds a matched word, to show under it. */
  line: string;
  /** The words of the section that matched, as they are written there. */
  terms: string[];
}

const FENCE = /^\s*(```|~~~)/;
const HEADING = /^#{1,6}\s+(.*)$/;

/** Splits a Markdown document into its sections; a `#` inside a code block is not a heading. */
function sections(path: string, source: string, first: number): Section[] {
  const out: Section[] = [];
  let current: Section = { id: first, path, heading: null, text: "" };
  let fence = false;
  for (const line of source.split("\n")) {
    if (FENCE.test(line)) fence = !fence;
    const h = fence ? null : HEADING.exec(line);
    if (h) {
      if (current.text.trim() || current.heading) out.push(current);
      current = { id: first + out.length, path, heading: h[1].replace(/`/g, "").trim(), text: "" };
    } else {
      current.text += line + "\n";
    }
  }
  if (current.text.trim() || current.heading) out.push(current);
  return out.map((s, i) => ({ ...s, id: first + i }));
}

/** The first line of `text` that holds one of `terms`, skipping the API pages' anchors. */
function lineWith(text: string, terms: string[]): string {
  const lower = terms.map((t) => t.toLowerCase());
  for (const line of text.split("\n")) {
    const l = line.toLowerCase();
    if (!line.startsWith("<a id=") && lower.some((t) => l.includes(t))) return line.trim();
  }
  return "";
}

export class DocIndex {
  private constructor(private readonly index: MiniSearch<Section>) {}

  /** Reads every document (`read`) and indexes its sections. */
  static async build(paths: string[], read: (path: string) => Promise<string>): Promise<DocIndex> {
    const index = new MiniSearch<Section>({
      fields: ["heading", "text"],
      storeFields: ["path", "heading", "text"],
      searchOptions: { boost: { heading: 3 }, prefix: true, fuzzy: 0.2, combineWith: "AND" },
    });
    const texts = await Promise.all(paths.map((p) => read(p).catch(() => "")));
    let next = 0;
    paths.forEach((path, i) => {
      const found = sections(path, texts[i], next);
      next += found.length;
      index.addAll(found);
    });
    return new DocIndex(index);
  }

  /** The sections that match `query`, most relevant first. */
  search(query: string, limit = 200): DocResult[] {
    return this.index
      .search(query)
      .slice(0, limit)
      .map((r: SearchResult) => ({
        path: r.path as string,
        heading: r.heading as string | null,
        line: lineWith(r.text as string, r.terms),
        terms: r.terms,
      }));
  }
}
