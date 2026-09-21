<script lang="ts">
  // The in-app menu bar (shadcn's Menubar), themed like everything else.
  // Shortcuts are handled in App.svelte; here they are only shown.
  import { onMount } from "svelte";
  import * as Menubar from "$lib/components/ui/menubar";
  import {
    THEMES,
    fontOptions,
    setFont,
    setPostPosition,
    setShowDocs,
    setTheme,
    settings,
    zoom,
    type PostPosition,
    type ThemePref,
  } from "./settings.svelte";

  export interface MenuActions {
    newFile(): void;
    openFile(): void;
    saveFile(saveAs?: boolean): void;
    closeTab(): void;
    quit(): void;
    runLine(): void;
    runBlock(): void;
    interrupt(): void;
    restart(): void;
    clearPost(): void;
    openDocs(): void;
    environments(): void;
    preferences(): void;
    chooseDocsFolder(): void;
  }

  let { actions: a }: { actions: MenuActions } = $props();

  let fonts = $state<[string, string][]>([]);
  onMount(async () => (fonts = await fontOptions()));
</script>

{#snippet item(label: string, action: () => void, shortcut?: string)}
  <Menubar.Item onSelect={action}>
    {label}
    {#if shortcut}<Menubar.Shortcut>{shortcut}</Menubar.Shortcut>{/if}
  </Menubar.Item>
{/snippet}

<Menubar.Root class="h-auto border-0 bg-transparent p-0 shadow-none">
  <Menubar.Menu>
    <Menubar.Trigger>File</Menubar.Trigger>
    <Menubar.Content>
      {@render item("New", a.newFile, "Ctrl+N")}
      {@render item("Open...", a.openFile, "Ctrl+O")}
      <Menubar.Separator />
      {@render item("Save", () => a.saveFile(), "Ctrl+S")}
      {@render item("Save As...", () => a.saveFile(true), "Ctrl+Shift+S")}
      <Menubar.Separator />
      {@render item("Close Tab", a.closeTab, "Ctrl+W")}
      <Menubar.Separator />
      {@render item("Preferences...", a.preferences, "Ctrl+,")}
      <Menubar.Separator />
      {@render item("Quit", a.quit, "Ctrl+Q")}
    </Menubar.Content>
  </Menubar.Menu>

  <Menubar.Menu>
    <Menubar.Trigger>Session</Menubar.Trigger>
    <Menubar.Content>
      {@render item("Evaluate Line or Selection", a.runLine, "Shift+Enter")}
      {@render item("Evaluate Block", a.runBlock, "Ctrl+Enter")}
      <Menubar.Separator />
      {@render item("Interrupt", a.interrupt, "Ctrl+.")}
      {@render item("Restart Session", a.restart)}
      {@render item("Python Environments...", a.environments)}
      <Menubar.Separator />
      {@render item("Clear Post Window", a.clearPost)}
    </Menubar.Content>
  </Menubar.Menu>

  <Menubar.Menu>
    <Menubar.Trigger>View</Menubar.Trigger>
    <Menubar.Content>
      <Menubar.Sub>
        <Menubar.SubTrigger>Theme</Menubar.SubTrigger>
        <Menubar.SubContent>
          <Menubar.RadioGroup value={settings.theme} onValueChange={(v) => setTheme(v as ThemePref)}>
            {#each THEMES as [value, label] (value)}
              <Menubar.RadioItem {value}>{label}</Menubar.RadioItem>
            {/each}
          </Menubar.RadioGroup>
        </Menubar.SubContent>
      </Menubar.Sub>
      <Menubar.Sub>
        <Menubar.SubTrigger>Font</Menubar.SubTrigger>
        <Menubar.SubContent class="max-h-80 overflow-y-auto">
          <Menubar.RadioGroup value={settings.font} onValueChange={setFont}>
            {#each fonts as [value, label] (value)}
              <Menubar.RadioItem {value}>{label}</Menubar.RadioItem>
            {/each}
          </Menubar.RadioGroup>
        </Menubar.SubContent>
      </Menubar.Sub>
      <Menubar.Separator />
      <Menubar.CheckboxItem checked={settings.showDocs} onCheckedChange={(v) => setShowDocs(v)}>
        Documentation
      </Menubar.CheckboxItem>
      <Menubar.Sub>
        <Menubar.SubTrigger>Post Window</Menubar.SubTrigger>
        <Menubar.SubContent>
          <Menubar.RadioGroup value={settings.postPosition} onValueChange={(v) => setPostPosition(v as PostPosition)}>
            <Menubar.RadioItem value="right">On the Right</Menubar.RadioItem>
            <Menubar.RadioItem value="bottom">Below the Editor</Menubar.RadioItem>
          </Menubar.RadioGroup>
        </Menubar.SubContent>
      </Menubar.Sub>
      <Menubar.Separator />
      {@render item("Increase Size", () => zoom(1), "Ctrl++")}
      {@render item("Decrease Size", () => zoom(-1), "Ctrl+-")}
      {@render item("Default Size", () => zoom(null), "Ctrl+0")}
    </Menubar.Content>
  </Menubar.Menu>

  <Menubar.Menu>
    <Menubar.Trigger>Help</Menubar.Trigger>
    <Menubar.Content>
      {@render item("Documentation", a.openDocs, "F1")}
      {@render item("Documentation Folder...", a.chooseDocsFolder)}
    </Menubar.Content>
  </Menubar.Menu>
</Menubar.Root>
