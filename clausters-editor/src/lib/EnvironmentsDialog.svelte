<script lang="ts">
  // A language's environments: create (installing the configured source), choose the
  // session's one, reinstall the package and delete. Python's are virtual
  // environments, JavaScript's folders of npm packages; pip's or npm's output
  // goes to the post window.
  import { host } from "$host";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import { Badge } from "$lib/components/ui/badge";
  import RefreshCw from "@lucide/svelte/icons/refresh-cw";
  import Trash2 from "@lucide/svelte/icons/trash-2";
  import { confirm } from "./ConfirmDialog.svelte";
  import { info } from "./session.svelte";
  import { LANGS, type Lang } from "./languages";

  interface EnvInfo {
    name: string;
    version: string;
    active: boolean;
    source: string;
  }

  let open = $state(false);
  let lang = $state<Lang>("python");
  const L = $derived(LANGS[lang]);
  let envs = $state<EnvInfo[]>([]);
  let source = $state("");
  let newName = $state("");
  let working = $state<string | null>(null);
  let error = $state<string | null>(null);

  const systemActive = $derived(!envs.some((e) => e.active));

  // The backend's rule (envs.rs), checked while typing.
  const nameError = $derived.by(() => {
    const name = newName.trim();
    if (!name) return null;
    if (/[\/\s]/.test(name)) return "This looks like a path or a command: it goes in \"Install source\" (step 1).";
    if (!/^[A-Za-z0-9_-][A-Za-z0-9._-]*$/.test(name)) return "Only letters, digits, \".\", \"_\" or \"-\" (not starting with \".\").";
    if (envs.some((e) => e.name === name)) return `An environment named "${name}" already exists.`;
    return null;
  });

  async function refresh() {
    envs = await host.invoke<EnvInfo[]>("list_envs", { lang });
  }

  /** Opens the dialog on `l`'s environments. */
  export async function show(l: Lang) {
    if (l !== lang) envs = [];
    lang = l;
    open = true;
    error = null;
    await refresh().catch((e) => info(`${e}\n`));
  }

  /** Runs an operation showing its status, and refreshes the list when it ends. */
  async function task(label: string, run: () => Promise<unknown>) {
    working = label;
    error = null;
    try {
      await run();
    } catch (e) {
      error = String(e);
      info(`${e}\n`);
    } finally {
      working = null;
      await refresh().catch(() => {});
    }
  }

  function create(e: SubmitEvent) {
    e.preventDefault();
    const name = newName.trim();
    if (!name || nameError) return;
    task(`Creating "${name}"... (progress in the post window)`, async () => {
      await host.invoke("create_env", { lang, name, source });
      newName = "";
      source = "";
    });
  }

  const use = (name: string | null) => task("Switching environment...", () => host.invoke("use_env", { lang, name }));

  const reinstall = (name: string) =>
    task(`Installing into "${name}"... (progress in the post window)`, () => host.invoke("install_in_env", { lang, name }));

  async function remove(name: string) {
    const ok = await confirm({
      title: `Delete the environment "${name}"?`,
      description: "The environment's folder and every package installed in it are deleted.",
      action: "Delete",
    });
    if (ok) task("Deleting...", () => host.invoke("delete_env", { lang, name }));
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-xl">
    <Dialog.Header>
      <Dialog.Title>{L.label} Environments</Dialog.Title>
      <Dialog.Description>
        They are kept in the app's data folder. The session uses the marked environment, also after reopening the app.
      </Dialog.Description>
    </Dialog.Header>

    <div class="grid gap-2">
      <Label>Environments</Label>
      <div class="divide-y rounded-md border text-sm">
        <div class="flex h-11 items-center gap-2 px-3">
          <span class="flex-1">System {L.runtime}</span>
          {#if systemActive}
            <Badge variant="secondary">in use</Badge>
          {:else}
            <Button variant="ghost" size="sm" onclick={() => use(null)} disabled={!!working}>Use</Button>
          {/if}
        </div>
        {#each envs as env (env.name)}
          <div class="flex min-h-11 items-center gap-2 px-3 py-1.5">
            <div class="grid min-w-0 flex-1">
              <div class="flex items-baseline gap-2">
                <span class="font-medium">{env.name}</span>
                {#if env.version}<span class="text-muted-foreground">{L.runtime} {env.version}</span>{/if}
              </div>
              <span class="truncate font-mono text-xs text-muted-foreground" title={env.source}>
                {env.source || `only ${L.intelligence}, for the completions`}
              </span>
            </div>
            {#if env.active}
              <Badge variant="secondary">in use</Badge>
            {:else}
              <Button variant="ghost" size="sm" onclick={() => use(env.name)} disabled={!!working}>Use</Button>
            {/if}
            <Button
              variant="ghost"
              size="icon-sm"
              onclick={() => reinstall(env.name)}
              disabled={!!working || !env.source}
              title="Reinstall from its install source"
              aria-label="Reinstall {env.name}"
            >
              <RefreshCw />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              onclick={() => remove(env.name)}
              disabled={!!working}
              title="Delete the environment"
              aria-label="Delete {env.name}"
            >
              <Trash2 />
            </Button>
          </div>
        {/each}
      </div>
    </div>

    <form class="grid gap-4 rounded-md border p-4" onsubmit={create}>
      <p class="text-sm font-medium">New environment</p>

      <div class="grid gap-1.5">
        <Label for="install-source">1. Install source</Label>
        <Input id="install-source" class="font-mono" placeholder={L.sourceExample} bind:value={source} />
        <p class="text-xs text-muted-foreground">
          What goes after <code>{L.installer}</code>. It is kept in the environment for the "Reinstall" button;
          left empty, the environment gets no packages but <code>{L.intelligence}</code>, which every environment
          the app creates installs for the completions and the help.
        </p>
      </div>

      <div class="grid gap-1.5">
        <Label for="env-name">2. Environment name</Label>
        <div class="flex gap-2">
          <Input id="env-name" placeholder="e.g. clausters" bind:value={newName} aria-invalid={!!nameError} />
          <Button type="submit" disabled={!!working || !newName.trim() || !!nameError}>Create</Button>
        </div>
        {#if nameError}
          <p class="text-xs text-destructive">{nameError}</p>
        {:else}
          <p class="text-xs text-muted-foreground">A short name: letters, digits, ".", "_" or "-".</p>
        {/if}
      </div>
    </form>

    {#if working}
      <p class="text-sm text-muted-foreground">{working}</p>
    {:else if error}
      <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">{error}</p>
    {/if}

    <Dialog.Footer>
      <Dialog.Close>{#snippet child({ props })}<Button variant="outline" {...props}>Close</Button>{/snippet}</Dialog.Close>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
