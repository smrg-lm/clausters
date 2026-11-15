<script lang="ts" module>
  // The page's open and save dialogs, over its storage (opfs.ts): what the
  // desktop's file dialogs are, for files that live in the page. Uploading,
  // downloading, a new folder and deleting are here too, since this is where
  // the page's files are seen.
  //
  // `await pickFile({ ... })` returns the chosen path, or null.
  import type { FileFilter } from "$lib/host";

  interface Request {
    mode: "open" | "save";
    filters: FileFilter[];
    defaultPath?: string;
    resolve: (path: string | null) => void;
  }

  let request = $state<Request | null>(null);

  export function pickFile(opts: Omit<Request, "resolve">): Promise<string | null> {
    request?.resolve(null);
    return new Promise((resolve) => (request = { ...opts, resolve }));
  }
</script>

<script lang="ts">
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import Folder from "@lucide/svelte/icons/folder";
  import FileText from "@lucide/svelte/icons/file-text";
  import Download from "@lucide/svelte/icons/download";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import { download, list, makeFolder, normalize, remove, upload, type Entry } from "./opfs";

  let dir = $state("/");
  let entries = $state<Entry[]>([]);
  let name = $state("");
  let filter = $state(0);
  let error = $state<string | null>(null);

  const dirOf = (path: string) => normalize(path).replace(/\/[^/]*$/, "") || "/";
  const join = (folder: string, file: string) => normalize(`${folder}/${file}`);

  const shown = $derived.by(() => {
    const extensions = request?.filters[filter]?.extensions ?? ["*"];
    if (extensions.includes("*")) return entries;
    return entries.filter((e) => e.folder || extensions.some((x) => e.name.toLowerCase().endsWith(`.${x}`)));
  });

  async function go(path: string) {
    try {
      entries = await list(path);
      dir = normalize(path);
      error = null;
    } catch (e) {
      error = String(e);
    }
  }

  // Each time a request comes: where its default path is, or the root.
  $effect(() => {
    if (request === null) return;
    const start = request.defaultPath;
    name = start ? (start.split("/").pop() ?? "") : "";
    filter = 0;
    go(start && start.includes("/") ? dirOf(start) : "/");
  });

  function finish(path: string | null) {
    request?.resolve(path);
    request = null;
  }

  function choose(entry: Entry) {
    if (entry.folder) return go(entry.path);
    if (request?.mode === "open") finish(entry.path);
    else name = entry.name;
  }

  function accept() {
    const typed = name.trim();
    if (!typed) return;
    finish(typed.startsWith("/") ? normalize(typed) : join(dir, typed));
  }

  async function newFolder() {
    const typed = name.trim();
    if (!typed) {
      error = "Type the new folder's name first.";
      return;
    }
    await makeFolder(join(dir, typed));
    name = "";
    await go(dir);
  }

  async function uploadHere() {
    try {
      await upload(dir);
      await go(dir);
    } catch (e) {
      error = String(e);
    }
  }

  async function removeEntry(entry: Entry) {
    if (!window.confirm(`Delete ${entry.path}${entry.folder ? " and everything in it" : ""}?`)) return;
    await remove(entry.path);
    await go(dir);
  }
</script>

<Dialog.Root open={request !== null} onOpenChange={(open) => !open && finish(null)}>
  <Dialog.Content class="sm:max-w-lg">
    <Dialog.Header>
      <Dialog.Title>{request?.mode === "save" ? "Save in the page's storage" : "Open from the page's storage"}</Dialog.Title>
    </Dialog.Header>

    <div class="flex items-center gap-2 font-mono text-sm">
      <Button variant="ghost" size="sm" disabled={dir === "/"} onclick={() => go(dirOf(dir))}>..</Button>
      <span class="truncate" title={dir}>{dir}</span>
    </div>

    <div class="max-h-72 min-h-40 overflow-y-auto rounded-md border" role="listbox" aria-label="Files">
      {#each shown as entry (entry.path)}
        <div class="flex items-center gap-2 px-2 py-1 hover:bg-muted">
          <button class="flex min-w-0 flex-1 items-center gap-2 text-left text-sm" ondblclick={() => choose(entry)} onclick={() => (entry.folder ? go(entry.path) : (name = entry.name))}>
            {#if entry.folder}<Folder class="size-4 shrink-0" />{:else}<FileText class="size-4 shrink-0" />{/if}
            <span class="truncate">{entry.name}</span>
          </button>
          {#if !entry.folder}
            <Button variant="ghost" size="icon-sm" title="Download" aria-label="Download {entry.name}" onclick={() => download(entry.path)}><Download /></Button>
          {/if}
          <Button variant="ghost" size="icon-sm" title="Delete" aria-label="Delete {entry.name}" onclick={() => removeEntry(entry)}><Trash2 /></Button>
        </div>
      {:else}
        <p class="p-3 text-sm text-muted-foreground">Nothing here yet: save a file, or upload one from the disk.</p>
      {/each}
    </div>

    <form class="flex gap-2" onsubmit={(e) => { e.preventDefault(); accept(); }}>
      <Input class="flex-1 font-mono" placeholder="File name" bind:value={name} aria-label="File name" />
      {#if request && request.filters.length > 1}
        <select class="rounded-md border bg-background px-2 text-sm" bind:value={filter} aria-label="File type">
          {#each request.filters as f, i (i)}<option value={i}>{f.name}</option>{/each}
        </select>
      {/if}
    </form>
    {#if error}<p class="text-sm text-destructive">{error}</p>{/if}

    <Dialog.Footer class="sm:justify-between">
      <div class="flex gap-2">
        <Button variant="outline" onclick={uploadHere}>Upload...</Button>
        <Button variant="ghost" onclick={newFolder}>New folder</Button>
      </div>
      <div class="flex gap-2">
        <Button variant="ghost" onclick={() => finish(null)}>Cancel</Button>
        <Button onclick={accept} disabled={!name.trim()}>{request?.mode === "save" ? "Save" : "Open"}</Button>
      </div>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
