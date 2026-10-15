<script lang="ts" module>
  /** What help is asked about: a name, and where the session says it lives. */
  export interface HelpSubject {
    name: string;
    /** Its name within its module, e.g. "Server.boot". */
    qualname?: string;
    /** Its dotted path, e.g. "clausters.defs.server.Server.boot". */
    fullname?: string;
  }
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { COPIED_ICON, COPY_ICON, escapeHtml, renderMarkdown, slugify } from "./markdown";
  import { info, pyHelp } from "./session.svelte";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import ArrowRight from "@lucide/svelte/icons/arrow-right";
  import House from "@lucide/svelte/icons/house";
  import Search from "@lucide/svelte/icons/search";
  import ChevronUp from "@lucide/svelte/icons/chevron-up";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import X from "@lucide/svelte/icons/x";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { settings } from "./settings.svelte";
  import Grip from "./Grip.svelte";
  import { DocIndex, type DocResult } from "./docIndex";
  import { PageFind, markMatches } from "./pageFind.svelte";

  let {
    onChooseFolder,
    onClose,
    onDragStart,
  }: { onChooseFolder: () => void; onClose: () => void; onDragStart: (e: PointerEvent) => void } = $props();

  /** A chapter of a book's table of contents (`books.rs`). */
  interface Chapter {
    title: string;
    /** The document; null for a chapter not written yet. */
    path: string | null;
    children: Chapter[];
  }
  interface Book {
    /** What its documents' paths start with: empty for the editor's own. */
    prefix: string;
    title: string;
    parts: { title: string | null; chapters: Chapter[] }[];
  }

  // `scroll` is where the page was left, so that going back returns there.
  // `home` is the start page, every book and its chapters; `book` is one
  // book's whole table of contents.
  type Page = (
    | { kind: "home" }
    | { kind: "book"; prefix: string }
    | { kind: "doc"; path: string; anchor?: string }
    | { kind: "text"; title: string; text: string }
    | { kind: "search"; query: string; results: DocResult[] }
  ) & {
    scroll?: number;
  };

  let docs = $state<string[]>([]);
  let books = $state<Book[]>([]);
  let history = $state<Page[]>([]);
  let pos = $state(-1);
  let html = $state("");
  let plain = $state<string | null>(null);
  let content: HTMLElement;
  let blocks: string[] = [];
  let source: string | null = null;
  /** The document on screen, so that a link into it only scrolls. */
  let shownPath: string | null = null;

  // Each document's last rendering: going back to a page whose text has not changed
  // skips parsing and sanitizing it again. Keyed by path; the source is compared,
  // so a file edited on disk is rendered afresh.
  const renders = new Map<string, { source: string; dark: boolean; html: string; blocks: string[] }>();

  function render() {
    if (source === null) return;
    const dark = settings.dark;
    let cached = shownPath === null ? undefined : renders.get(shownPath);
    if (!cached || cached.source !== source || cached.dark !== dark) {
      cached = { source, dark, ...renderMarkdown(source, dark) };
      if (shownPath !== null) renders.set(shownPath, cached);
    }
    html = cached.html;
    blocks = cached.blocks;
  }

  // Code blocks use the active theme's syntax colors.
  $effect(() => {
    settings.dark;
    render();
    tick().then(() => attachFind(false));
  });

  const current = $derived(history[pos]);

  /** Every chapter's title, by its document. */
  const chapterTitles = $derived.by(() => {
    const titles = new Map<string, string>();
    const walk = (chapters: Chapter[]) => {
      for (const c of chapters) {
        if (c.path && !titles.has(c.path)) titles.set(c.path, c.title);
        walk(c.children);
      }
    };
    for (const b of books) for (const p of b.parts) walk(p.chapters);
    return titles;
  });

  /** The book a document belongs to: the package whose prefix it starts with,
   *  else the editor's own. */
  function bookOf(path: string): Book | undefined {
    return (
      books.find((b) => b.prefix && path.startsWith(b.prefix)) ?? books.find((b) => !b.prefix)
    );
  }

  /** A document's name as its book's contents give it, else its file's. */
  function chapterTitle(path: string): string {
    return (chapterTitles.get(path) ?? path.split("/").pop()!.replace(/\.md$/, "")).replaceAll("`", "");
  }

  /** Where the page on screen is: a book and, inside it, a chapter. */
  const trail = $derived.by((): { book?: Book; label: string } => {
    if (!current || current.kind === "home") return { label: "Documentation" };
    if (current.kind === "book") return { book: books.find((b) => b.prefix === current.prefix), label: "" };
    if (current.kind === "doc") return { book: bookOf(current.path), label: chapterTitle(current.path) };
    if (current.kind === "text") return { label: `${current.title} (pydoc)` };
    return { label: `Search: "${current.query}"` };
  });

  /** A title as the table of contents writes it: Markdown's `code` spans drawn as code. */
  const titleHtml = (title: string) => escapeHtml(title).replace(/`([^`]+)`/g, "<code>$1</code>");

  /** A chapter as a link, or as its title alone when it has no page yet. */
  function chapterLink(c: Chapter): string {
    const title = titleHtml(c.title);
    return c.path ? `<a href="#" data-path="${escapeHtml(c.path)}">${title}</a>` : `<span>${title}</span>`;
  }

  // A heading, not a link to read: drawn as the heading it is, like a search result's title.
  const bookLink = (b: Book) =>
    `<a href="#" class="no-underline" data-book="${escapeHtml(b.prefix)}">${titleHtml(b.title)}</a>`;

  /** The start page: each book, and each part's first-level chapters. */
  function homeHtml(): string {
    let out = "<h1>Documentation</h1>";
    for (const b of books) {
      out += `<h2>${bookLink(b)}</h2>`;
      for (const p of b.parts) {
        if (p.title) out += `<h4>${titleHtml(p.title)}</h4>`;
        const items = p.chapters.map((c) => `<li>${chapterLink(c)}</li>`).join("");
        out += `<ul>${items}</ul>`;
      }
    }
    return out;
  }

  /** A book's whole table of contents. */
  function bookHtml(b: Book): string {
    const list = (chapters: Chapter[]): string =>
      `<ul>${chapters.map((c) => `<li>${chapterLink(c)}${c.children.length ? list(c.children) : ""}</li>`).join("")}</ul>`;
    let out = `<h1>${titleHtml(b.title)}</h1>`;
    for (const p of b.parts) {
      if (p.title) out += `<h2>${titleHtml(p.title)}</h2>`;
      out += list(p.chapters);
    }
    return out;
  }

  function scrollTo(anchor?: string) {
    const target = anchor ? content.querySelector(`#${CSS.escape(anchor)}`) : null;
    if (target) target.scrollIntoView();
    else content.scrollTop = 0;
  }

  async function show(page: Page) {
    // A document already on screen is not read again: only the anchor changes.
    if (page.kind === "doc" && page.path === shownPath && source !== null) {
      // Nothing to draw: the scroll below moves to the anchor.
    } else if (page.kind === "doc") {
      try {
        source = await invoke<string>("read_doc", { path: page.path });
        shownPath = page.path;
        render();
        plain = null;
      } catch (e) {
        source = null;
        shownPath = null;
        html = "";
        plain = String(e);
      }
    } else if (page.kind === "home" || page.kind === "book") {
      source = null;
      shownPath = null;
      const book = page.kind === "book" ? books.find((b) => b.prefix === page.prefix) : undefined;
      html = book ? bookHtml(book) : homeHtml();
      plain = null;
    } else if (page.kind === "text") {
      source = null;
      shownPath = null;
      html = `<pre class="whitespace-pre-wrap">${escapeHtml(page.text)}</pre>`;
      plain = null;
    } else {
      source = null;
      shownPath = null;
      html = "";
      plain = null;
    }
    await tick();
    if (page.scroll !== undefined) content.scrollTop = page.scroll;
    else scrollTo(page.kind === "doc" ? page.anchor : undefined);
    attachFind(page.scroll === undefined && !(page.kind === "doc" && page.anchor));
  }

  // --- Finding text: in the page on screen, and in every document ---

  let findOpen = $state(false);
  let findTerm = $state("");
  /** What the find bar searches: the page on screen, or every document. */
  let findScope = $state<"page" | "all">("page");
  let findInput = $state<HTMLInputElement | null>(null);
  const finder = new PageFind();
  /** The documents' full-text index: built on the first search, again when the list changes. */
  let index: Promise<DocIndex> | null = null;

  /** The page as it is drawn: with the find bar's text marked while it finds in the page. */
  const shownHtml = $derived(findOpen && findScope === "page" ? markMatches(html, findTerm) : html);

  /** Takes the marks of the page just drawn, from the section it was opened at;
   *  `scroll` brings the current one into view. */
  function attachFind(scroll: boolean) {
    if (!content) return;
    const anchor = current?.kind === "doc" ? current.anchor : undefined;
    const after = anchor ? content.querySelector(`#${CSS.escape(anchor)}`) : null;
    finder.attach(content, { after, scroll });
  }

  /** Marks the find bar's text again (it changed, or the bar opened), once the page is redrawn. */
  async function findInPage() {
    await tick();
    attachFind(true);
  }

  /** Opens the find bar on the page on screen (Ctrl+F in the documentation). */
  export async function find() {
    findOpen = true;
    findScope = "page";
    await tick();
    findInput?.select();
    findInPage();
  }

  /** Closes the find bar, giving the focus back to the page (where Ctrl+F opens it again). */
  function closeFind() {
    findOpen = false;
    content?.focus({ preventScroll: true });
    findInPage();
  }

  /** Searches every document (Ctrl+Shift+F): for `query` when given, else the find bar's text. */
  export async function searchAll(query = "") {
    if (query.trim()) findTerm = query.trim();
    findOpen = true;
    findScope = "all";
    await tick();
    findInput?.focus();
    const q = findTerm.trim();
    if (!q) return;
    index ??= DocIndex.build(docs, (path) => invoke<string>("read_doc", { path }));
    go({ kind: "search", query: q, results: (await index).search(q) });
  }

  function onFindKey(e: KeyboardEvent) {
    if (e.key === "Escape") closeFind();
    else if (e.key === "Enter" && findScope === "all") searchAll();
    else if (e.key === "Enter") finder.step(e.shiftKey ? -1 : 1);
    else return;
    e.preventDefault();
  }

  function setScope(scope: "page" | "all") {
    findScope = scope;
    if (scope === "all") searchAll();
    // From the results, finding in a page goes back to the page they were searched from.
    else if (current?.kind === "search" && pos > 0) back();
    else findInPage();
    findInput?.focus();
  }

  /** A result's line, escaped, with the words that matched marked. */
  function marked(line: string, terms: string[]): string {
    const lower = line.toLowerCase();
    const spans: [number, number][] = [];
    for (const t of terms) {
      for (let at = lower.indexOf(t); at >= 0; at = lower.indexOf(t, at + t.length)) spans.push([at, at + t.length]);
    }
    spans.sort((a, b) => a[0] - b[0]);
    let out = "";
    let last = 0;
    for (const [from, to] of spans) {
      if (from < last) continue;
      out += escapeHtml(line.slice(last, from)) + `<mark class="doc-hit">${escapeHtml(line.slice(from, to))}</mark>`;
      last = to;
    }
    return out + escapeHtml(line.slice(last));
  }

  /** Remembers where the current page was left before moving away from it. */
  function leave() {
    if (history[pos]) history[pos].scroll = content.scrollTop;
  }

  function go(page: Page) {
    leave();
    history = [...history.slice(0, pos + 1), page];
    pos = history.length - 1;
    show(page);
  }

  function back() {
    if (pos <= 0) return;
    leave();
    show(history[--pos]);
  }

  function forward() {
    if (pos >= history.length - 1) return;
    leave();
    show(history[++pos]);
  }

  export function openDoc(path: string, anchor?: string) {
    go({ kind: "doc", path, anchor });
  }

  /** The start page: every book, and its chapters. */
  export function home() {
    go({ kind: "home" });
  }

  function openBook(prefix: string) {
    go({ kind: "book", prefix });
  }

  interface DocHit {
    path: string;
    heading: string | null;
    score: number;
    /** The heading *is* the name: an API reference's entry for it. */
    named: boolean;
  }

  /** Help on a name: the section whose heading is it (its reference entry); else a
   *  document named after it; else the section that talks about it most; else
   *  pydoc's page for it, from the session. */
  export async function help({ name, qualname, fullname }: HelpSubject) {
    const terms = [...new Set([qualname, name].filter((t): t is string => !!t))];
    const searches = await Promise.all(
      terms.map((term) => invoke<DocHit[]>("search_docs", { term }).catch(() => [] as DocHit[])),
    );
    const openHit = (hit: DocHit) => openDoc(hit.path, hit.heading ? slugify(hit.heading) : undefined);

    const named = searches.find((hits) => hits[0]?.named);
    if (named) return openHit(named[0]);
    const file = `${name.split(".").pop()!.toLowerCase()}.md`;
    const byName = docs.find((d) => d.toLowerCase().split("/").pop() === file);
    if (byName) return openDoc(byName);
    const found = searches.find((hits) => hits.length);
    if (found) return openHit(found[0]);
    const expr = fullname ?? name;
    go({ kind: "text", title: expr, text: await pyHelp(expr) });
  }

  function resolve(from: string, rel: string): string {
    const parts = from.split("/");
    parts.pop();
    for (const seg of rel.split("/")) {
      if (seg === "..") parts.pop();
      else if (seg && seg !== ".") parts.push(seg);
    }
    return parts.join("/");
  }

  function onClick(e: MouseEvent) {
    const target = e.target as HTMLElement;
    const button = target.closest<HTMLButtonElement>("button.copy-block");
    if (button) {
      navigator.clipboard.writeText(blocks[Number(button.dataset.block)]).then(
        () => {
          // A check for a moment, where the click was.
          button.innerHTML = COPIED_ICON;
          setTimeout(() => (button.innerHTML = COPY_ICON), 1200);
        },
        (err) => info(`Could not copy: ${err}\n`),
      );
      return;
    }
    const link = target.closest("a");
    if (link?.dataset.book !== undefined) {
      e.preventDefault();
      return openBook(link.dataset.book);
    }
    // A search result: the document, at its section, with the word that matched found.
    if (link?.dataset.path) {
      e.preventDefault();
      if (link.dataset.term) findTerm = link.dataset.term;
      const heading = link.dataset.heading;
      return openDoc(link.dataset.path, heading ? slugify(heading) : undefined);
    }
    const href = link?.getAttribute("href");
    if (!href) return;
    e.preventDefault();
    // A link out of the documentation is not followed from here: it is written to
    // the post window, where it says it leaves the editor and opens the browser.
    if (/^[a-z][a-z0-9+.-]*:/i.test(href)) {
      info(`External link (opens in the browser): ${href}\n`);
      return;
    }
    const [path, anchor] = href.split("#");
    // An anchor in the same document is a step in the history too, like a link to another one.
    if (!path) return current?.kind === "doc" ? openDoc(current.path, anchor) : scrollTo(anchor);
    openDoc(current?.kind === "doc" ? resolve(current.path, path) : path, anchor);
  }

  /** Reads the documentation folder again (e.g. after changing it) and opens its start page. */
  export async function reload() {
    history = [];
    pos = -1;
    renders.clear();
    shownPath = null;
    try {
      docs = await invoke<string[]>("list_docs");
      books = await invoke<Book[]>("list_books");
      index = null;
    } catch (e) {
      docs = [];
      books = [];
      source = null;
      html = "";
      plain = String(e);
      return;
    }
    if (docs.length === 0) {
      source = null;
      html = "";
      plain = "The documentation folder holds no .md files.";
      return;
    }
    home();
  }

  /** Lists the documents again, keeping the page on screen: a new session may run on
   *  another environment, whose packages carry other books. */
  async function relist() {
    const before = docs.length;
    docs = await invoke<string[]>("list_docs").catch(() => docs);
    books = await invoke<Book[]>("list_books").catch(() => books);
    index = null;
    // The start page and a book's contents are drawn from the books: again,
    // with what the new environment carries.
    if (before === 0 && docs.length > 0) home();
    else if (current?.kind === "home" || current?.kind === "book") show(current);
  }

  onMount(() => {
    reload();
    const unlisten = listen("py-start", relist);
    return () => {
      unlisten.then((f) => f());
    };
  });
</script>

<div class="flex h-full flex-col" data-panel="docs">
  <div class="flex h-9 shrink-0 items-center gap-0.5 border-b bg-secondary px-1">
    <Grip label="documentation" {onDragStart} />
    <Button variant="ghost" size="icon-sm" onclick={back} disabled={pos <= 0} title="Back" aria-label="Back">
      <ArrowLeft />
    </Button>
    <Button
      variant="ghost"
      size="icon-sm"
      onclick={forward}
      disabled={pos >= history.length - 1}
      title="Forward"
      aria-label="Forward"
    >
      <ArrowRight />
    </Button>
    <Button variant="ghost" size="icon-sm" onclick={home} title="Documentation home" aria-label="Home">
      <House />
    </Button>
    <nav class="ml-1 flex min-w-0 flex-1 items-center gap-1 text-sm" aria-label="Where this page is">
      {#if trail.book}
        {@const book = trail.book}
        <Button
          variant="ghost"
          size="sm"
          class="h-7 min-w-0 shrink px-1.5"
          onclick={() => openBook(book.prefix)}
          title="The book's contents"
        >
          <span class="truncate">{book.title}</span>
        </Button>
        {#if trail.label}<span class="shrink-0 text-muted-foreground">&rsaquo;</span>{/if}
      {/if}
      <span class="truncate px-1">{trail.label}</span>
    </nav>
    <Button variant="ghost" size="icon-sm" class="ml-0.5" onclick={find} title="Find (Ctrl+F in this page, Ctrl+Shift+F in every document)" aria-label="Find">
      <Search />
    </Button>
    <Button
      variant="ghost"
      size="icon-sm"
      onclick={onChooseFolder}
      title="Choose the documentation folder"
      aria-label="Choose the documentation folder"
    >
      <FolderOpen />
    </Button>
    <Button variant="ghost" size="icon-sm" onclick={onClose} title="Hide the documentation" aria-label="Hide the documentation">
      <X />
    </Button>
  </div>
  {#if findOpen}
    <div class="flex h-9 shrink-0 items-center gap-1 border-b px-2">
      <Input
        bind:ref={findInput}
        bind:value={findTerm}
        class="h-7 min-w-0 flex-1 text-sm"
        placeholder={findScope === "page" ? "Find in this page" : "Search every document (Enter)"}
        spellcheck={false}
        oninput={() => findScope === "page" && findInPage()}
        onkeydown={onFindKey}
      />
      {#if findScope === "page"}
        <span class="w-14 shrink-0 text-center text-xs text-muted-foreground tabular-nums">
          {finder.count ? `${finder.index + 1}/${finder.count}` : findTerm.trim() ? "0/0" : ""}
        </span>
        <Button variant="ghost" size="icon-sm" onclick={() => finder.step(-1)} disabled={!finder.count} title="Previous (Shift+Enter)" aria-label="Previous">
          <ChevronUp />
        </Button>
        <Button variant="ghost" size="icon-sm" onclick={() => finder.step(1)} disabled={!finder.count} title="Next (Enter)" aria-label="Next">
          <ChevronDown />
        </Button>
      {/if}
      <div class="flex shrink-0 rounded-md border p-0.5" role="radiogroup" aria-label="Where to search">
        {#each [["page", "Page"], ["all", "All documents"]] as const as [scope, label] (scope)}
          <Button
            variant={findScope === scope ? "secondary" : "ghost"}
            size="sm"
            class="h-6 px-2"
            role="radio"
            aria-checked={findScope === scope}
            onclick={() => setScope(scope)}
          >
            {label}
          </Button>
        {/each}
      </div>
      <Button variant="ghost" size="icon-sm" onclick={closeFind} title="Close (Esc)" aria-label="Close the find bar">
        <X />
      </Button>
    </div>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <article
    class="prose prose-sm max-w-none min-h-0 flex-1 overflow-auto px-5 pt-4 pb-5
      prose-code:before:content-none prose-code:after:content-none
      [&_mark.doc-find]:bg-yellow-300/40 [&_mark.doc-find]:text-inherit
      [&_mark.doc-find-current]:bg-orange-400/70
      [&_mark.doc-hit]:bg-yellow-300/40 [&_mark.doc-hit]:text-inherit
      [&_.code-block]:relative [&_.copy-block]:absolute [&_.copy-block]:top-1.5 [&_.copy-block]:right-1.5
      [&_pre]:font-mono [&_pre]:text-(length:--code-size) [&_pre_code]:[font-size:inherit]"
    style:font-size="var(--code-size)"
    bind:this={content}
    tabindex="-1"
    onclick={onClick}
  >
    {#if current?.kind === "search"}
      <p class="text-muted-foreground">
        {current.results.length === 0
          ? `Nothing in the documentation matches "${current.query}".`
          : `"${current.query}": ${current.results.length} ${current.results.length === 1 ? "section" : "sections"}${current.results.length >= 200 ? " (the first 200)" : ""}, the most relevant first.`}
      </p>
      <ul class="pl-0">
        {#each current.results as r, i (i)}
          <li class="list-none pl-0">
            <a
              href={`#${r.path}`}
              data-path={r.path}
              data-heading={r.heading ?? ""}
              data-term={r.terms[0] ?? ""}
              class="no-underline"
            >
              <span class="font-medium"
                >{bookOf(r.path)?.title ?? ""} &rsaquo; {chapterTitle(r.path)}{r.heading ? ` \u203A ${r.heading}` : ""}</span
              >
            </a>
            {#if r.line}<div class="text-muted-foreground">{@html marked(r.line, r.terms)}</div>{/if}
          </li>
        {/each}
      </ul>
    {:else if plain !== null}
      <pre class="whitespace-pre-wrap">{plain}</pre>
    {:else}
      {@html shownHtml}
    {/if}
  </article>
</div>
