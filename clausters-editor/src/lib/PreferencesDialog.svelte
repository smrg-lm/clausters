<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import * as Dialog from "$lib/components/ui/dialog";
  import { Button } from "$lib/components/ui/button";
  import { Label } from "$lib/components/ui/label";
  import { Textarea } from "$lib/components/ui/textarea";
  import { info, restart } from "./session.svelte";

  interface Config {
    docsDir: string | null;
    startupCode: string;
  }

  let { onDocsChanged }: { onDocsChanged: () => void } = $props();

  let open = $state(false);
  let docsDir = $state<string | null>(null);
  let startupCode = $state("");
  let savedStartupCode = $state("");

  export async function show() {
    open = true;
    try {
      const config = await invoke<Config>("get_config");
      docsDir = config.docsDir;
      startupCode = savedStartupCode = config.startupCode;
    } catch (e) {
      info(`${e}\n`);
    }
  }

  async function saveStartupCode(andRestart: boolean) {
    try {
      savedStartupCode = (await invoke<Config>("set_startup_code", { code: startupCode })).startupCode;
      if (andRestart) await restart();
    } catch (e) {
      info(`${e}\n`);
    }
  }

  async function setDocsDir(dir: string | null) {
    try {
      docsDir = (await invoke<{ docsDir: string | null }>("set_docs_dir", { dir })).docsDir;
      onDocsChanged();
    } catch (e) {
      info(`${e}\n`);
    }
  }

  /** Opens the folder picker directly (also from the panel and the Help menu). */
  export async function chooseDocsDir() {
    try {
      docsDir = (await invoke<{ docsDir: string | null }>("get_config")).docsDir;
    } catch {}
    const dir = await openDialog({
      directory: true,
      title: "Documentation folder (.md)",
      defaultPath: docsDir ?? undefined,
    });
    if (typeof dir === "string") await setDocsDir(dir);
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content class="sm:max-w-xl">
    <Dialog.Header>
      <Dialog.Title>Preferences</Dialog.Title>
    </Dialog.Header>

    <div class="grid gap-2">
      <Label>Documentation folder (.md files)</Label>
      <div class="truncate rounded-md border bg-muted/40 px-3 py-2 font-mono text-sm" title={docsDir ?? ""}>
        {docsDir ?? "The one bundled with the app"}
      </div>
      <div class="flex gap-2">
        <Button variant="outline" onclick={chooseDocsDir}>Choose folder...</Button>
        <Button variant="ghost" onclick={() => setDocsDir(null)} disabled={docsDir === null}>Use the bundled one</Button>
      </div>
    </div>

    <div class="grid gap-2">
      <Label for="startup-code">Startup code</Label>
      <Textarea
        id="startup-code"
        class="min-h-28 font-mono text-sm"
        spellcheck={false}
        placeholder={"from clausters import Session\nfrom clausters.defs import SynthDef, control, sine, out"}
        bind:value={startupCode}
      />
      <p class="text-xs text-muted-foreground">
        Runs every time the session starts (when the app opens, when the session is restarted or when the
        environment changes) and is shown in the post window.
      </p>
      <div class="flex gap-2">
        <Button variant="outline" onclick={() => saveStartupCode(false)} disabled={startupCode === savedStartupCode}>
          Save
        </Button>
        <Button variant="ghost" onclick={() => saveStartupCode(true)}>Save and restart the session</Button>
      </div>
    </div>

    <Dialog.Footer>
      <Dialog.Close>{#snippet child({ props })}<Button variant="outline" {...props}>Close</Button>{/snippet}</Dialog.Close>
    </Dialog.Footer>
  </Dialog.Content>
</Dialog.Root>
