<script lang="ts">
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { COPIED_ICON, COPY_ICON, renderMarkdown, slugify } from "./markdown";
  import { info, pyHelp, pyInspect } from "./session.svelte";
  import ArrowLeft from "@lucide/svelte/icons/arrow-left";
  import ArrowRight from "@lucide/svelte/icons/arrow-right";
  import House from "@lucide/svelte/icons/house";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import X from "@lucide/svelte/icons/x";
  import { Button } from "$lib/components/ui/button";
  import * as Select from "$lib/components/ui/select";
  import { settings } from "./settings.svelte";

  let { onChooseFolder, onClose }: { onChooseFolder: () => void; onClose: () => void } = $props();

  // `scroll` is where the page was left, so that going back returns there.
  type Page = ({ kind: "doc"; path: string; anchor?: string } | { kind: "text"; title: string; text: string }) & {
    scroll?: number;
  };

  let docs = $state<string[]>([]);
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
  });

  const current = $derived(history[pos]);
  const title = $derived(current ? (current.kind === "doc" ? current.path : current.title) : "");
  const docItems = $derived(docs.map((d) => ({ value: d, label: d.replace(/\.md$/, "") })));

  function scrollTo(anchor?: string) {
    const target = anchor ? content.querySelector(`#${CSS.escape(anchor)}`) : null;
    if (target) target.scrollIntoView();
    else content.scrollTop = 0;
  }

  async function show(page: Page) {
    // A document already on screen is not read again: only the anchor changes.
    if (page.kind === "doc" && (page.path !== shownPath || source === null)) {
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
    } else if (page.kind === "text") {
      source = null;
      shownPath = null;
      html = "";
      plain = page.text;
    }
    await tick();
    if (page.scroll !== undefined) content.scrollTop = page.scroll;
    else scrollTo(page.kind === "doc" ? page.anchor : undefined);
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

  export function home() {
    const start = docs.find((d) => d.toLowerCase() === "index.md") ?? docs[0];
    if (start) openDoc(start);
  }

  interface DocHit {
    path: string;
    heading: string | null;
    score: number;
  }

  /** Help for a name: a .md with that name; otherwise the documentation section that
   *  talks about it most; otherwise pydoc from the session. */
  export async function help(word: string) {
    const last = word.split(".").pop()!;
    const byName = (name: string) => docs.find((d) => d.toLowerCase().split("/").pop() === name);
    const match = byName(`${word.toLowerCase()}.md`) ?? byName(`${last.toLowerCase()}.md`);
    if (match) return openDoc(match);

    // First the full name of the object in the session (s.boot -> "Server.boot"),
    // which tells apart methods with the same name; then the bare word.
    const qualname = (await pyInspect(word))?.qualname;
    for (const term of new Set([qualname, word, last].filter((t): t is string => !!t))) {
      const hits = await invoke<DocHit[]>("search_docs", { term }).catch(() => [] as DocHit[]);
      if (hits.length) {
        const [best] = hits;
        return openDoc(best.path, best.heading ? slugify(best.heading) : undefined);
      }
    }
    try {
      go({ kind: "text", title: word, text: await pyHelp(word) });
    } catch (e) {
      info(`${e}\n`);
    }
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
    const href = link?.getAttribute("href");
    if (!href) return;
    e.preventDefault();
    if (/^[a-z][a-z0-9+.-]*:/i.test(href)) {
      openUrl(href).catch((err) => info(`${err}\n`));
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
    } catch (e) {
      docs = [];
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

  onMount(reload);
</script>

<div class="flex h-full flex-col">
  <div class="flex h-9 shrink-0 items-center gap-0.5 border-b bg-secondary px-1">
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
    <Select.Root
      type="single"
      value={current?.kind === "doc" ? current.path : ""}
      onValueChange={(path) => path && openDoc(path)}
    >
      <Select.Trigger size="sm" class="ml-1 min-w-0 flex-1 bg-background" aria-label="Document">
        <span class="truncate">
          {current?.kind === "text" ? `${title} (pydoc)` : (docItems.find((d) => d.value === title)?.label ?? "")}
        </span>
      </Select.Trigger>
      <Select.Content>
        {#each docItems as item (item.value)}
          <Select.Item value={item.value} label={item.label} />
        {/each}
      </Select.Content>
    </Select.Root>
    <Button
      variant="ghost"
      size="icon-sm"
      class="ml-0.5"
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
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <article
    class="prose prose-sm max-w-none min-h-0 flex-1 overflow-auto px-5 pt-4 pb-5
      [&_.code-block]:relative [&_.copy-block]:absolute [&_.copy-block]:top-1.5 [&_.copy-block]:right-1.5
      [&_pre]:font-mono [&_pre]:text-(length:--code-size) [&_pre_code]:[font-size:inherit]"
    style:font-size="var(--code-size)"
    bind:this={content}
    onclick={onClick}
  >
    {#if plain !== null}
      <pre class="whitespace-pre-wrap">{plain}</pre>
    {:else}
      {@html html}
    {/if}
  </article>
</div>
