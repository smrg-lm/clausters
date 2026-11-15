// The page's documentation: the editor's own book, served beside the page
// with a list of its pages (`docs/pages.json`, written when the page is
// built), and the books of the packages the page serves, which a package names
// in its package.json (`"clausters-editor": { "docs": ... }`) and lists in its
// table of contents (`SUMMARY.md`). The same books, in the same order, as the
// desktop's documentation viewer shows (src-tauri/src/books.rs, files.rs).

import { CLAUSTERS } from "./session";

interface Chapter {
  title: string;
  path: string | null;
  children: Chapter[];
}
interface Part {
  title: string | null;
  chapters: Chapter[];
}
interface Book {
  prefix: string;
  title: string;
  parts: Part[];
}
interface Root {
  /** What its documents' paths start with: empty for the editor's own. */
  prefix: string;
  /** Where its files are served. */
  base: string;
  /** Its pages, relative to `base`. */
  pages: string[];
  book: Book;
}

const SUMMARY = "SUMMARY.md";
const own = new URL("docs/", document.baseURI).href;

async function text(url: string): Promise<string | null> {
  try {
    const response = await fetch(url);
    return response.ok ? await response.text() : null;
  } catch {
    return null;
  }
}

/** A `[Title](page.md)` of a table of contents, or null. */
function link(item: string, prefix: string): Chapter | null {
  const found = /^\[([^\]]*)\]\(([^)]*)\)/.exec(item);
  if (!found) return null;
  const path = found[2].trim();
  return { title: found[1].trim(), path: path ? `${prefix}${path.replace(/^\.\//, "")}` : null, children: [] };
}

/** The book a table of contents describes: its title, if it names one, and its parts. */
function parseSummary(source: string, prefix: string): { title: string | null; parts: Part[] } {
  let title: string | null = null;
  let seenHeading = false;
  const parts: Part[] = [];
  let partTitle: string | null = null;
  let entries: [number, Chapter][] = [];
  const close = () => {
    const chapters = nest(entries);
    entries = [];
    if (partTitle !== null || chapters.length) parts.push({ title: partTitle, chapters });
  };
  for (const line of source.split("\n")) {
    if (line.startsWith("# ")) {
      const heading = line.slice(2).trim();
      if (!seenHeading) {
        seenHeading = true;
        if (heading.toLowerCase() !== "summary") title = heading;
      } else {
        close();
        partTitle = heading;
      }
      continue;
    }
    const indent = line.length - line.trimStart().length;
    let item = line.trimStart();
    if (item.startsWith("- ") || item.startsWith("* ")) item = item.slice(2);
    const chapter = link(item, prefix);
    if (chapter) entries.push([indent, chapter]);
  }
  close();
  return { title, parts };
}

/** The chapters, nested by their indentation. */
function nest(entries: [number, Chapter][]): Chapter[] {
  const top: Chapter[] = [];
  const stack: [number, Chapter][] = [];
  for (const [indent, chapter] of entries) {
    while (stack.length && stack[stack.length - 1][0] >= indent) stack.pop();
    (stack.length ? stack[stack.length - 1][1].children : top).push(chapter);
    stack.push([indent, chapter]);
  }
  return top;
}

const pagesOf = (chapters: Chapter[], prefix: string): string[] =>
  chapters.flatMap((c) => [...(c.path ? [c.path.slice(prefix.length)] : []), ...pagesOf(c.children, prefix)]);

/** A book served at `base`: its table of contents, or the pages it lists. */
async function bookAt(base: string, prefix: string, fallback: string, listed: string[] = []): Promise<Root | null> {
  const summary = await text(new URL(SUMMARY, base).href);
  if (summary !== null) {
    const { title, parts } = parseSummary(summary, prefix);
    const pages = [...new Set([...listed, ...parts.flatMap((p) => pagesOf(p.chapters, prefix))])];
    return { prefix, base, pages, book: { prefix, title: title ?? fallback, parts } };
  }
  if (!listed.length) return null;
  const chapters: Chapter[] = [];
  for (const page of listed) {
    const first = (await text(new URL(page, base).href))?.split("\n").find((l) => l.startsWith("# "));
    chapters.push({ title: first ? first.slice(2).trim() : page.replace(/\.md$/, ""), path: `${prefix}${page}`, children: [] });
  }
  return { prefix, base, pages: listed, book: { prefix, title: fallback, parts: [{ title: null, chapters }] } };
}

let roots: Promise<Root[]> | null = null;

function allRoots(): Promise<Root[]> {
  roots ??= (async () => {
    const found: Root[] = [];
    const listed = JSON.parse((await text(new URL("pages.json", own).href)) ?? "[]") as string[];
    const editor = await bookAt(own, "", "clausters-editor", listed);
    if (editor) found.push(editor);
    // Each package's books, under its name, as the desktop lists them.
    const manifest = JSON.parse((await text(new URL("package.json", CLAUSTERS).href)) ?? "{}");
    const name = String(manifest.name ?? "clausters");
    const docs = manifest["clausters-editor"]?.docs;
    const books: Record<string, string> = typeof docs === "string" ? { [name]: docs } : (docs ?? {});
    for (const [book, folder] of Object.entries(books)) {
      const base = new URL(`${folder.replace(/\/?$/, "/")}`, CLAUSTERS).href;
      const root = await bookAt(base, `node_modules/${book}/`, book);
      if (root) found.push(root);
    }
    return found;
  })();
  return roots;
}

export async function listBooks(): Promise<Book[]> {
  return (await allRoots()).map((r) => r.book);
}

export async function listDocs(): Promise<string[]> {
  return (await allRoots()).flatMap((r) =>
    [...r.pages].sort((a, b) => Number(a.toLowerCase() !== "index.md") - Number(b.toLowerCase() !== "index.md") || a.localeCompare(b)).map((p) => `${r.prefix}${p}`),
  );
}

async function locate(path: string): Promise<[Root, string]> {
  const all = await allRoots();
  const root = all.find((r) => r.prefix && path.startsWith(r.prefix)) ?? all.find((r) => !r.prefix);
  if (!root) throw new Error(`Document not found: ${path}`);
  return [root, path.slice(root.prefix.length)];
}

export async function readDoc(path: string): Promise<string> {
  const [root, rel] = await locate(path);
  const found = await text(new URL(rel, root.base).href);
  if (found === null) throw new Error(`Document not found: ${path}`);
  return found;
}

// ---- search: the sections that talk about a term (files.rs, search_docs) ----

interface DocHit {
  path: string;
  heading: string | null;
  score: number;
  named: boolean;
}

const isWord = (c: string | undefined) => c !== undefined && /[\p{L}\p{N}_]/u.test(c);

/** Occurrences of `term` as a whole word in `line`, by start position. */
function wordMatches(line: string, term: string): number[] {
  const out: number[] = [];
  for (let i = line.indexOf(term); i >= 0; i = line.indexOf(term, i + 1)) {
    if (!isWord(line[i - 1]) && !isWord(line[i + term.length])) out.push(i);
  }
  return out;
}

const unescape = (s: string) => s.replace(/\\([!-/:-@[-`{-~])/g, "$1");
const inCodeSpan = (line: string, i: number) => (line.slice(0, i).match(/`/g)?.length ?? 0) % 2 === 1;

/** The section of one page that talks most about `term`, by files.rs's
 *  scores: a heading that is the name 300, one that contains it 100, each
 *  occurrence in code 10 and in prose 1. */
function searchPage(source: string, term: string): Omit<DocHit, "path"> | null {
  const sections: [string | null, number, boolean][] = [[null, 0, false]];
  let inFence = false;
  for (const line of source.split("\n")) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
      inFence = !inFence;
      continue;
    }
    if (!inFence && trimmed.startsWith("#")) {
      const title = trimmed.replace(/^#+/, "").trim();
      const plain = unescape(title.replace(/`/g, ""));
      const score = plain === term || plain.endsWith(`.${term}`) ? 300 : wordMatches(plain, term).length ? 100 : 0;
      sections.push([title, score, score >= 300]);
      continue;
    }
    const section = sections[sections.length - 1];
    for (const i of wordMatches(line, term)) section[1] += inFence || inCodeSpan(line, i) ? 10 : 1;
  }
  // On equal ranks, the first section wins.
  let best = sections[0];
  for (const section of sections) {
    if (Number(section[2]) > Number(best[2]) || (section[2] === best[2] && section[1] > best[1])) best = section;
  }
  return best[1] > 0 ? { heading: best[0], score: best[1], named: best[2] } : null;
}

export async function searchDocs(term: string): Promise<DocHit[]> {
  const wanted = term.trim();
  if (!wanted) return [];
  const hits: DocHit[] = [];
  for (const path of await listDocs()) {
    const source = await readDoc(path).catch(() => null);
    const hit = source && searchPage(source, wanted);
    if (hit) hits.push({ path, ...hit });
  }
  hits.sort((a, b) => Number(b.named) - Number(a.named) || b.score - a.score || a.path.localeCompare(b.path));
  return hits.slice(0, 10);
}
