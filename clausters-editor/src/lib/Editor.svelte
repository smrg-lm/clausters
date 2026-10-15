<script lang="ts" module>
  import type { Text } from "@codemirror/state";

  // Shared by every editor group, so that two groups never both have an "untitled 1".
  let untitled = 0;

  /** A tab taken out of one group to be put in another. The state is rebuilt
   *  there, since its extensions belong to the group that made it. */
  export interface MovedTab {
    path: string | null;
    name: string;
    doc: string;
    anchor: number;
    head: number;
    dirty: boolean;
    saved: Text;
  }
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import { Compartment, EditorState } from "@codemirror/state";
  import { EditorView } from "@codemirror/view";
  import { basicSetup } from "codemirror";
  import { pythonLanguage } from "@codemirror/lang-python";
  import { LanguageSupport } from "@codemirror/language";
  import { invoke } from "@tauri-apps/api/core";
  import { open, save } from "@tauri-apps/plugin-dialog";
  import { Button } from "$lib/components/ui/button";
  import { confirm } from "./ConfirmDialog.svelte";
  import { editorTheme } from "./editorTheme";
  import { evalBlock, evalLine, evaluation, type EvalHandlers } from "./evaluation";
  import { evaluate, info, interrupt, pyComplete, pyInspect, pySignatures } from "./session.svelte";
  import type { HelpSubject } from "./Docs.svelte";
  import { sessionCompletions } from "./completion";
  import { pythonHints } from "./hints";
  import { addRecent, removeRecent, settings, type OpenFiles } from "./settings.svelte";
  import { indentation, tabKeymap } from "./indent";
  import X from "@lucide/svelte/icons/x";
  import Grip from "./Grip.svelte";

  let {
    current = true,
    onHelp,
    onFocus,
    onDragStart,
    openElsewhere,
    onFilesChange,
  }: {
    /** The group the menu and the shortcuts act on; its active tab is marked. */
    current?: boolean;
    onHelp: (subject: HelpSubject) => void;
    /** This group became the one the menu and the shortcuts act on. */
    onFocus: () => void;
    onDragStart: (e: PointerEvent) => void;
    /** Shows `path` if another group has it open; a file is open in one group at a time. */
    openElsewhere: (path: string) => boolean;
    /** The files open in this group changed: which, in tab order, and the one on screen. */
    onFilesChange?: (files: OpenFiles) => void;
  } = $props();

  interface Tab {
    path: string | null;
    name: string;
    state: EditorState;
    dirty: boolean;
    /** The text as last opened or saved: the tab is modified while it differs. */
    saved: Text;
  }

  let tabs = $state<Tab[]>([]);
  let active = $state(0);
  let host: HTMLDivElement;
  let view: EditorView;

  const filters = [
    { name: "Python", extensions: ["py"] },
    { name: "All files", extensions: ["*"] },
  ];
  const basename = (p: string) => p.split(/[\\/]/).pop() ?? p;

  // Each tab keeps its own EditorState: the theme is reapplied when the tab or the theme changes.
  const themeSlot = new Compartment();
  const applyTheme = () => view?.dispatch({ effects: themeSlot.reconfigure(editorTheme(settings.dark)) });
  $effect(() => {
    settings.dark;
    applyTheme();
  });
  // Font and size go in CodeMirror's theme (not in outside CSS) so that it
  // redraws everything, including the active line's highlight in the gutter.
  const fontSlot = new Compartment();
  const fontTheme = () =>
    EditorView.theme({
      "&": { fontSize: `${settings.size}px` },
      ".cm-scroller": { fontFamily: "var(--mono)", lineHeight: "1.5" },
    });
  const applyFont = () => view?.dispatch({ effects: fontSlot.reconfigure(fontTheme()) });
  $effect(() => {
    settings.size;
    const spec = `${settings.size}px "${settings.font}"`;
    applyFont();
    document.fonts.load(spec).finally(() => view?.requestMeasure());
  });

  // The indentation preference, the same way: a tab's state holds its own copy.
  const indentSlot = new Compartment();
  const indentConfig = () => indentation(settings.indent);
  const applyIndent = () => view?.dispatch({ effects: indentSlot.reconfigure(indentConfig()) });
  $effect(() => {
    settings.indent;
    applyIndent();
  });

  $effect(() => {
    const paths = tabs.flatMap((t) => (t.path ? [t.path] : []));
    onFilesChange?.({ paths, active: tabs[active]?.path ?? null });
  });

  const handlers: EvalHandlers = {
    evaluate: (code, line) => {
      const t = tabs[active];
      evaluate(code, t.path ?? `<${t.name}>`, line);
    },
    // The name at the cursor as Jedi resolves it, so that help finds its reference entry.
    help: async ({ text, at }) => {
      const info = at ? await pyInspect(at) : null;
      onHelp(info ? { name: info.name, qualname: info.qualname, fullname: info.fullname } : { name: text });
    },
    interrupt,
  };

  const extensions = () => [
    basicSetup,
    tabKeymap,
    indentSlot.of(indentConfig()),
    // Python without its own completions: the session's (Jedi) cover the file's names too.
    new LanguageSupport(pythonLanguage),
    themeSlot.of([]),
    fontSlot.of([]),
    evaluation(handlers),
    pythonHints({ inspect: pyInspect, signatures: pySignatures }),
    sessionCompletions(pyComplete),
    EditorView.updateListener.of((u) => {
      const t = tabs[active];
      // Compared with what was saved, so undoing back to it (or retyping it) clears the mark.
      if (u.docChanged && t) t.dirty = !u.state.doc.eq(t.saved);
    }),
  ];

  function show(i: number) {
    active = i;
    view.setState(tabs[i].state);
    applyTheme();
    applyFont();
    applyIndent();
    view.focus();
  }

  function select(i: number) {
    if (i === active) return view.focus();
    tabs[active].state = view.state;
    show(i);
  }

  function addTab(path: string | null, name: string, doc: string, selection?: { anchor: number; head: number }) {
    if (tabs[active]) tabs[active].state = view.state;
    const state = EditorState.create({ doc, selection, extensions: extensions() });
    tabs.push({ path, name, dirty: false, saved: state.doc, state });
    show(tabs.length - 1);
  }

  /** An empty, untouched new tab: replaced by the next file instead of piling up. */
  const isBlank = (i: number) => {
    const t = tabs[i];
    return !!t && !t.path && !t.dirty && (i === active ? view.state : t.state).doc.length === 0;
  };

  export function focus() {
    view.focus();
  }

  /** The names of the tabs with unsaved changes. */
  export function unsaved(): string[] {
    return tabs.filter((t) => t.dirty).map((t) => t.name);
  }

  /** Shows the tab holding `path`, if this group has one. */
  export function showPath(path: string): boolean {
    const i = tabs.findIndex((t) => t.path === path);
    if (i < 0) return false;
    select(i);
    return true;
  }

  /** Takes the active tab out (or every tab, with `all`) to put it in another group. */
  export function takeTabs(all = false): MovedTab[] {
    tabs[active].state = view.state;
    const taken = (all ? tabs.map((_, i) => i) : [active])
      .filter((i) => !isBlank(i))
      .map((i) => {
        const t = tabs[i];
        const { anchor, head } = t.state.selection.main;
        const doc = t.state.doc.toString();
        return { path: t.path, name: t.name, doc, anchor, head, dirty: t.dirty, saved: t.saved };
      });
    if (all) {
      tabs = [];
      newFile();
    } else if (taken.length) {
      closeAt(active);
    }
    return taken;
  }

  export function putTabs(moved: MovedTab[]) {
    for (const m of moved) {
      const blank = isBlank(active) ? active : -1;
      addTab(m.path, m.name, m.doc, { anchor: m.anchor, head: m.head });
      tabs[active].dirty = m.dirty;
      tabs[active].saved = m.saved;
      if (blank >= 0) closeAt(blank);
    }
  }

  /** Opens again the files of an earlier session, showing the one that was on
   *  screen. The recent files keep their order. */
  export async function restore(files: OpenFiles) {
    for (const path of files.paths) await openPath(path, false);
    if (files.active) showPath(files.active);
  }

  export function newFile() {
    addTab(null, `untitled ${++untitled}`, "");
  }

  export async function openFile() {
    const path = await open({ multiple: false, filters });
    if (typeof path === "string") await openPath(path);
  }

  /** Opens `path` in a new tab, or shows the tab that already has it, and puts it
   *  first in the recent files (unless `remember` is off). A file that can no
   *  longer be read leaves the recent files. */
  export async function openPath(path: string, remember = true) {
    if (showPath(path) || openElsewhere(path)) return remember && addRecent(path);
    try {
      const text = await invoke<string>("read_file", { path });
      const previous = active;
      const replace = isBlank(previous);
      addTab(path, basename(path), text);
      if (replace) closeAt(previous);
      if (remember) addRecent(path);
    } catch (e) {
      removeRecent(path);
      info(`${e}\n`);
    }
  }

  /** Opens a tab for `path`, a file that does not exist yet: empty, and created
   *  where it says when it is saved. */
  export function openMissing(path: string) {
    if (showPath(path) || openElsewhere(path)) return;
    const previous = active;
    const replace = isBlank(previous);
    addTab(path, basename(path), "");
    if (replace) closeAt(previous);
  }

  export async function saveFile(saveAs = false) {
    const t = tabs[active];
    let path = t.path;
    if (!path || saveAs) {
      path = await save({ defaultPath: t.path ?? `${t.name.replace(/\s+/g, "_")}.py`, filters });
      if (!path) return;
    }
    // What is written, not what is on screen once the write returns: an edit made
    // in between keeps the tab modified.
    const written = view.state.doc;
    try {
      await invoke("write_file", { path, contents: written.toString() });
      t.path = path;
      t.name = basename(path);
      t.saved = written;
      t.dirty = !(t === tabs[active] ? view.state : t.state).doc.eq(written);
      addRecent(path);
    } catch (e) {
      info(`${e}\n`);
    }
  }

  function closeAt(i: number) {
    if (tabs.length === 1) {
      tabs.pop();
      return newFile();
    }
    tabs.splice(i, 1);
    if (i < active) active--;
    else if (i === active) show(Math.min(i, tabs.length - 1));
  }

  export async function closeTab(i = active) {
    const t = tabs[i];
    if (
      t.dirty &&
      !(await confirm({
        title: `Close "${t.name}"?`,
        description: "There are unsaved changes that will be lost.",
        action: "Close without saving",
      }))
    )
      return;
    closeAt(i);
  }

  export const runLine = () => evalLine(view, handlers);
  /** The selected text, if the selection is within one line (what a search would look for). */
  export function selectedText(): string {
    const sel = view.state.selection.main;
    const text = view.state.sliceDoc(sel.from, sel.to);
    return text.includes("\n") ? "" : text;
  }
  export const runBlock = () => evalBlock(view, handlers);

  onMount(() => {
    view = new EditorView({ parent: host });
    newFile();
    return () => view.destroy();
  });
</script>

<div class="flex h-full min-w-0 flex-col" onfocusin={onFocus}>
  <div class="flex h-9 shrink-0 border-b bg-secondary">
    <div class="flex items-center border-r pl-1">
      <Grip label="editor" {onDragStart} />
    </div>
    <div class="flex min-w-0 flex-1 overflow-x-auto [scrollbar-width:none]" role="tablist">
      {#each tabs as tab, i (tab)}
        <div
          class={[
            "group relative flex max-w-56 items-center border-r text-sm",
            i === active
              ? [
                  "bg-background text-foreground before:absolute before:inset-x-0 before:top-0 before:h-0.5",
                  current ? "before:bg-ring" : "before:bg-border",
                ]
              : "text-muted-foreground hover:text-foreground",
          ]}
          role="tab"
          tabindex="-1"
          aria-selected={i === active}
          title={tab.path ?? tab.name}
          onauxclick={(e) => e.button === 1 && closeTab(i)}
        >
          <button class="truncate py-2 pr-1 pl-3" onclick={() => select(i)}>{tab.name}</button>
          <!-- A "modified" dot that turns into an x on hover. -->
          <Button variant="ghost" size="icon-xs" class="mr-1.5" onclick={() => closeTab(i)} aria-label="Close {tab.name}">
            {#if tab.dirty}
              <span class="size-2 rounded-full bg-current group-hover:hidden"></span>
            {/if}
            <X class={[tab.dirty ? "hidden group-hover:block" : i === active ? "" : "invisible group-hover:visible"]} />
          </Button>
        </div>
      {/each}
    </div>
  </div>
  <div class="min-h-0 flex-1 [&_.cm-editor]:h-full [&_.cm-focused]:outline-none! [&_.cm-evalFlash]:bg-ring/25! [&_.cm-cellMarker]:border-t [&_.cm-cellMarker]:border-border" bind:this={host}></div>
</div>
