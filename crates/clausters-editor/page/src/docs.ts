// The page's documentation, read for the editor: the editor's own book,
// served beside the page with the list of its pages (`docs/pages.json`,
// written when the page is built), and the books of the packages the page
// serves, which a package names in its package.json
// (`"clausters-editor": { "docs": ... }`) and lists in its table of contents
// (`SUMMARY.md`). What a book is -- its title, its parts and chapters -- is the
// editor's to make of these folders, the same as from the desktop's disk.

import { CLAUSTERS } from "./session";

/** A folder of documentation, read, as the editor takes one. */
interface Folder {
  /** What its documents' paths start with: empty for the editor's own. */
  prefix: string;
  /** Its own name, which titles a book that names none. */
  name: string;
  /** Its table of contents, if it has one. */
  summary: string | null;
  /** Every document, `[path in the folder, text]`. */
  documents: [string, string][];
  /** Where it is served, which a picture a page names is read from. */
  at: string;
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

/** The pages a table of contents names, `[Title](page.md)`. */
const named = (summary: string): string[] =>
  [...summary.matchAll(/\]\(([^)]+\.md)\)/g)].map((m) => m[1].trim().replace(/^\.\//, ""));

/** The folder served at `base`: its table of contents, and the pages it and
 *  `listed` name. `null` for a folder with neither. */
async function folderAt(base: string, prefix: string, name: string, listed: string[] = []): Promise<Folder | null> {
  const summary = await text(new URL(SUMMARY, base).href);
  const pages = [...new Set([...listed, ...(summary === null ? [] : named(summary))])];
  if (summary === null && pages.length === 0) return null;
  const documents: [string, string][] = [];
  for (const page of pages) {
    const found = await text(new URL(page, base).href);
    if (found !== null) documents.push([page, found]);
  }
  return { prefix, name, summary, documents, at: base };
}

/** **Every book's folder, read**: the editor's own, then each package's,
 *  under its name, as the desktop lists them. As the JSON the editor reads. */
export async function readDocs(): Promise<string> {
  const found: Folder[] = [];
  const listed = JSON.parse((await text(new URL("pages.json", own).href)) ?? "[]") as string[];
  const editor = await folderAt(own, "", "clausters-editor", listed);
  if (editor) found.push(editor);
  const manifest = JSON.parse((await text(new URL("package.json", CLAUSTERS).href)) ?? "{}");
  const name = String(manifest.name ?? "clausters");
  const docs = manifest["clausters-editor"]?.docs;
  const books: Record<string, string> = typeof docs === "string" ? { [name]: docs } : (docs ?? {});
  for (const [book, folder] of Object.entries(books)) {
    const base = new URL(`${folder.replace(/\/?$/, "/")}`, CLAUSTERS).href;
    const read = await folderAt(base, `${book}/`, book);
    if (read) found.push(read);
  }
  return JSON.stringify(found);
}
