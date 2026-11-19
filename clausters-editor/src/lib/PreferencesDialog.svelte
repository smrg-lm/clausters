<script lang="ts">
  import { host } from "$host";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Label } from "$lib/components/ui/label";
  import { Textarea } from "$lib/components/ui/textarea";
  import * as Select from "$lib/components/ui/select";
  import { info, restart } from "./session.svelte";
  import { INDENTS, setIndent, setLanguage, setReopenFiles, settings, type IndentPref } from "./settings.svelte";
  import { LANGS, LANG_IDS, type Lang } from "./languages";

  type Config = { docsDir: string | null } & Record<Lang, { startupCode: string }>;

  let { onDocsChanged }: { onDocsChanged: () => void } = $props();

  let open = $state(false);
  let docsDir = $state<string | null>(null);
  /** The language whose startup code is on screen. */
  let codeLang = $state<Lang>(settings.language);
  let startupCode = $state<Record<Lang, string>>({ python: "", javascript: "" });
  let savedStartupCode = $state<Record<Lang, string>>({ python: "", javascript: "" });

  export async function show() {
    open = true;
    codeLang = settings.language;
    try {
      const config = await host.invoke<Config>("get_config");
      docsDir = config.docsDir;
      for (const l of LANG_IDS) startupCode[l] = savedStartupCode[l] = config[l].startupCode;
    } catch (e) {
      info(`${e}\n`);
    }
  }

  async function saveStartupCode(andRestart: boolean) {
    const lang = codeLang;
    try {
      const config = await host.invoke<Config>("set_startup_code", { lang, code: startupCode[lang] });
      savedStartupCode[lang] = config[lang].startupCode;
      if (andRestart) await restart(lang);
    } catch (e) {
      info(`${e}\n`);
    }
  }

  async function setDocsDir(dir: string | null) {
    try {
      docsDir = (await host.invoke<{ docsDir: string | null }>("set_docs_dir", { dir })).docsDir;
      onDocsChanged();
    } catch (e) {
      info(`${e}\n`);
    }
  }

  /** Opens the folder picker directly (also from the panel and the Help menu). */
  export async function chooseDocsDir() {
    try {
      docsDir = (await host.invoke<{ docsDir: string | null }>("get_config")).docsDir;
    } catch {}
    const dir = await host.openFolder({
      title: "Documentation folder (.md)",
      defaultPath: docsDir ?? undefined,
    });
    if (dir !== null) await setDocsDir(dir);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-xl">
    <Dialog.Header>
      <Dialog.Title>Preferences</Dialog.Title>
    </Dialog.Header>

    <div class="flex items-center gap-2">
      <input
        id="reopen-files"
        type="checkbox"
        class="size-4 accent-primary"
        checked={settings.reopenFiles}
        onchange={(e) => setReopenFiles(e.currentTarget.checked)}
      />
      <Label for="reopen-files" class="font-normal">Open the last session's files at startup</Label>
    </div>

    {#if LANG_IDS.length > 1}
    <div class="grid gap-2">
      <Label>Language of new files</Label>
      <Select.Root type="single" value={settings.language} onValueChange={(v) => v && setLanguage(v as Lang)}>
        <Select.Trigger class="w-44" aria-label="Language of new files">
          {LANGS[settings.language].label}
        </Select.Trigger>
        <Select.Content>
          {#each LANG_IDS as value (value)}
            <Select.Item {value} label={LANGS[value].label} />
          {/each}
        </Select.Content>
      </Select.Root>
      <p class="text-xs text-muted-foreground">
        A file opened is the language its extension names; Session &rarr; Language changes the one on screen.
      </p>
    </div>
    {/if}

    <div class="grid gap-2">
      <Label>Indentation</Label>
      <Select.Root type="single" value={settings.indent} onValueChange={(v) => v && setIndent(v as IndentPref)}>
        <Select.Trigger class="w-44" aria-label="Indentation">
          {INDENTS.find(([i]) => i === settings.indent)?.[1]}
        </Select.Trigger>
        <Select.Content>
          {#each INDENTS as [value, label] (value)}
            <Select.Item {value} {label} />
          {/each}
        </Select.Content>
      </Select.Root>
      <p class="text-xs text-muted-foreground">
        What Tab inserts at the cursor, and one level of indentation for the selected lines.
      </p>
    </div>

    {#if host.docsFolder}
    <div class="grid gap-2">
      <Label>Documentation folder (.md files)</Label>
      <div class="truncate rounded-md border bg-muted/40 px-3 py-2 font-mono text-sm" title={docsDir ?? ""}>
        {docsDir ?? "The one bundled with the app"}
      </div>
      <div class="flex gap-2">
        <Button variant="outline" onclick={chooseDocsDir}>Choose folder...</Button>
        <Button variant="ghost" onclick={() => setDocsDir(null)} disabled={docsDir === null}>Use bundled</Button>
      </div>
    </div>
    {/if}

    <div class="grid gap-2">
      <div class="flex items-center justify-between gap-2">
        <Label for="startup-code">Startup code</Label>
        {#if LANG_IDS.length > 1}
        <Select.Root type="single" value={codeLang} onValueChange={(v) => v && (codeLang = v as Lang)}>
          <Select.Trigger size="sm" class="w-36" aria-label="The startup code's language">
            {LANGS[codeLang].label}
          </Select.Trigger>
          <Select.Content>
            {#each LANG_IDS as value (value)}
              <Select.Item {value} label={LANGS[value].label} />
            {/each}
          </Select.Content>
        </Select.Root>
        {/if}
      </div>
      <Textarea
        id="startup-code"
        class="min-h-28 font-mono text-sm"
        spellcheck={false}
        placeholder={LANGS[codeLang].startupExample}
        bind:value={startupCode[codeLang]}
      />
      <p class="text-xs text-muted-foreground">
        Runs every time the language's session starts (when it is first needed, when it is restarted or when
        its environment changes) and is shown in the post window.
      </p>
      <div class="flex gap-2">
        <Button
          variant="outline"
          onclick={() => saveStartupCode(false)}
          disabled={startupCode[codeLang] === savedStartupCode[codeLang]}
        >
          Save
        </Button>
        <Button variant="ghost" onclick={() => saveStartupCode(true)}>Save and restart session</Button>
      </div>
    </div>

    <Dialog.Footer>
      <Dialog.Close>{#snippet child({ props })}<Button variant="outline" {...props}>Close</Button>{/snippet}</Dialog.Close>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
