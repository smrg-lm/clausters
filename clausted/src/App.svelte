<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import * as Resizable from "$lib/components/ui/resizable";
  import { Button } from "$lib/components/ui/button";
  import { Separator } from "$lib/components/ui/separator";
  import Editor from "./lib/Editor.svelte";
  import Post from "./lib/Post.svelte";
  import Docs from "./lib/Docs.svelte";
  import AppMenu, { type MenuActions } from "./lib/AppMenu.svelte";
  import ConfirmDialog from "./lib/ConfirmDialog.svelte";
  import EnvironmentsDialog from "./lib/EnvironmentsDialog.svelte";
  import PreferencesDialog from "./lib/PreferencesDialog.svelte";
  import { initSession, interrupt, restart, session, startSession } from "./lib/session.svelte";
  import { setPostPosition, setShowDocs, settings, zoom, type PostPosition } from "./lib/settings.svelte";
  import Play from "@lucide/svelte/icons/play";
  import ListVideo from "@lucide/svelte/icons/list-video";
  import Square from "@lucide/svelte/icons/square";
  import RotateCcw from "@lucide/svelte/icons/rotate-ccw";

  let editor: Editor;
  let post: Post;
  let docs: Docs;
  let environments: EnvironmentsDialog;
  let preferences: PreferencesDialog;

  // --- Panel layout ---
  // Each panel is created only once (below, under "panels") and *moved* into the slot it belongs in:
  // changing the layout does not destroy the editor, the output or the documentation's history.
  let editorNode = $state<HTMLElement>();
  let docsNode = $state<HTMLElement>();
  let postNode = $state<HTMLElement>();

  // Taking an element out of the document loses its scroll position: the position of each
  // scrollable element in the panel is remembered and restored when it is placed again.
  const scrolls = new WeakMap<HTMLElement, Map<Element, [number, number]>>();

  const adopt = (node: HTMLElement | undefined) => (slot: HTMLElement) => {
    if (!node) return;
    if (!scrolls.has(node)) {
      const saved = new Map<Element, [number, number]>();
      scrolls.set(node, saved);
      node.addEventListener(
        "scroll",
        (e) => {
          const el = e.target as Element;
          if (el.isConnected && el.clientHeight > 0) saved.set(el, [el.scrollTop, el.scrollLeft]);
        },
        true,
      );
    }
    slot.appendChild(node);
    requestAnimationFrame(() => {
      for (const [el, [top, left]] of scrolls.get(node)!) {
        if (el.isConnected) el.scrollTo(left, top);
      }
    });
  };

  const postBottom = $derived(settings.postPosition === "bottom");
  const hasRight = $derived(settings.showDocs || !postBottom);
  const layout = $derived(`${settings.showDocs ? "docs" : "nodocs"}-${settings.postPosition}`);

  function showDocs() {
    if (!settings.showDocs) setShowDocs(true);
  }

  // Dragging the post window (by the handle on its bar) to another position.
  let mainEl: HTMLElement;
  let leftEl = $state<HTMLElement>();
  let rightEl = $state<HTMLElement>();
  let drag = $state<{ zones: { position: PostPosition; label: string; rect: DOMRect }[]; over: PostPosition | null } | null>(null);

  function startPostDrag(e: PointerEvent) {
    e.preventDefault();
    const main = mainEl.getBoundingClientRect();
    const left = leftEl!.getBoundingClientRect();
    const bottom = new DOMRect(left.x, left.y + left.height * 0.55, left.width, left.height * 0.45);
    const right = rightEl && hasRight
      ? rightEl.getBoundingClientRect()
      : new DOMRect(main.right - main.width * 0.35, main.y, main.width * 0.35, main.height);
    drag = {
      zones: [
        { position: "bottom", label: "Below the editor", rect: bottom },
        { position: "right", label: "On the right", rect: right },
      ],
      over: null,
    };
    const move = (ev: PointerEvent) => {
      if (!drag) return;
      const hit = drag.zones.find(({ rect: r }) =>
        ev.clientX >= r.left && ev.clientX <= r.right && ev.clientY >= r.top && ev.clientY <= r.bottom);
      drag.over = hit?.position ?? null;
    };
    const up = () => {
      if (drag?.over) setPostPosition(drag.over);
      drag = null;
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  const actions: MenuActions = {
    newFile: () => editor.newFile(),
    openFile: () => editor.openFile(),
    saveFile: (saveAs) => editor.saveFile(saveAs),
    closeTab: () => editor.closeTab(),
    quit: () => getCurrentWindow().close(),
    runLine: () => editor.runLine(),
    runBlock: () => editor.runBlock(),
    interrupt,
    restart,
    clearPost: () => post.clear(),
    openDocs: () => {
      showDocs();
      docs.home();
    },
    environments: () => environments.show(),
    preferences: () => preferences.show(),
    chooseDocsFolder: () => preferences.chooseDocsDir(),
  };

  // Global shortcuts. The evaluation ones (and Ctrl+. inside the editor) are handled by CodeMirror.
  function onKeydown(e: KeyboardEvent) {
    if (e.key === "F1") {
      e.preventDefault();
      return actions.openDocs();
    }
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    const inEditor = !!(e.target as HTMLElement).closest?.(".cm-editor");
    const keys: Record<string, () => void> = {
      n: actions.newFile,
      o: actions.openFile,
      s: () => actions.saveFile(e.shiftKey),
      w: actions.closeTab,
      q: actions.quit,
      ",": actions.preferences,
      "+": () => zoom(1),
      "=": () => zoom(1),
      "-": () => zoom(-1),
      "0": () => zoom(null),
    };
    if (!inEditor) keys["."] = interrupt;
    const action = keys[e.key.toLowerCase()];
    if (!action) return;
    e.preventDefault();
    action();
  }

  // Ctrl+wheel: the scroll is accumulated so that a touchpad does not change the size all at once.
  let wheelAcc = 0;
  function onWheel(e: WheelEvent) {
    if (!e.ctrlKey) return;
    e.preventDefault();
    wheelAcc += e.deltaY;
    if (Math.abs(wheelAcc) < 40) return;
    zoom(wheelAcc < 0 ? 1 : -1);
    wheelAcc = 0;
  }

  const status = $derived(
    !session.alive ? "stopped" : session.busy > 0 ? `running (${session.busy})` : "ready",
  );

  // Explicitly not passive: wheel listeners on window are passive by default.
  onMount(() => {
    window.addEventListener("wheel", onWheel, { passive: false });
    return () => window.removeEventListener("wheel", onWheel);
  });

  onMount(async () => {
    await initSession();
    await startSession();
  });
</script>

<svelte:window onkeydown={onKeydown} />

<div class="flex h-screen flex-col">
  <header class="flex h-10 shrink-0 items-center gap-0.5 border-b bg-secondary px-1.5">
    <AppMenu {actions} />
    <Separator orientation="vertical" class="mx-1.5 h-5!" />
    <Button variant="ghost" size="icon-sm" onclick={actions.runLine} title="Evaluate line or selection (Shift+Enter)" aria-label="Evaluate line or selection">
      <Play />
    </Button>
    <Button variant="ghost" size="icon-sm" onclick={actions.runBlock} title="Evaluate block (Ctrl+Enter)" aria-label="Evaluate block">
      <ListVideo />
    </Button>
    <Button variant="ghost" size="icon-sm" onclick={interrupt} title="Interrupt (Ctrl+.)" aria-label="Interrupt">
      <Square />
    </Button>
    <Button variant="ghost" size="icon-sm" onclick={restart} title="Restart the Python session" aria-label="Restart the session">
      <RotateCcw />
    </Button>
  </header>

  <main class="relative min-h-0 flex-1" bind:this={mainEl}>
    {#key layout}
      <Resizable.PaneGroup direction="horizontal" autoSaveId="clausted-h-{layout}">
        <Resizable.Pane id="left" order={1} defaultSize={58} minSize={15}>
          <div class="h-full" bind:this={leftEl}>
            {#if postBottom}
              <Resizable.PaneGroup direction="vertical" autoSaveId="clausted-left">
                <Resizable.Pane id="editor" order={1} defaultSize={65} minSize={15}>
                  <div class="h-full" {@attach adopt(editorNode)}></div>
                </Resizable.Pane>
                <Resizable.Handle />
                <Resizable.Pane id="post" order={2} minSize={10}>
                  <div class="h-full" {@attach adopt(postNode)}></div>
                </Resizable.Pane>
              </Resizable.PaneGroup>
            {:else}
              <div class="h-full" {@attach adopt(editorNode)}></div>
            {/if}
          </div>
        </Resizable.Pane>
        {#if hasRight}
          <Resizable.Handle />
          <Resizable.Pane id="right" order={2} minSize={15}>
            <div class="h-full" bind:this={rightEl}>
              {#if settings.showDocs && !postBottom}
                <Resizable.PaneGroup direction="vertical" autoSaveId="clausted-right">
                  <Resizable.Pane id="docs" order={1} defaultSize={55} minSize={10}>
                    <div class="h-full" {@attach adopt(docsNode)}></div>
                  </Resizable.Pane>
                  <Resizable.Handle />
                  <Resizable.Pane id="post" order={2} minSize={10}>
                    <div class="h-full" {@attach adopt(postNode)}></div>
                  </Resizable.Pane>
                </Resizable.PaneGroup>
              {:else if settings.showDocs}
                <div class="h-full" {@attach adopt(docsNode)}></div>
              {:else}
                <div class="h-full" {@attach adopt(postNode)}></div>
              {/if}
            </div>
          </Resizable.Pane>
        {/if}
      </Resizable.PaneGroup>
    {/key}

    <!-- Drop zones while dragging the post window -->
    {#if drag}
      <div class="fixed inset-0 z-40 cursor-grabbing">
        {#each drag.zones as zone (zone.position)}
          <div
            class={[
              "absolute flex items-center justify-center rounded-lg border-2 border-dashed text-sm font-medium transition-colors",
              drag.over === zone.position
                ? "border-ring bg-ring/20 text-foreground"
                : "border-muted-foreground/40 bg-background/40 text-muted-foreground",
            ]}
            style:left="{zone.rect.left + 6}px"
            style:top="{zone.rect.top + 6}px"
            style:width="{zone.rect.width - 12}px"
            style:height="{zone.rect.height - 12}px"
          >
            {zone.label}
          </div>
        {/each}
      </div>
    {/if}
  </main>

  <footer class="flex h-6 shrink-0 items-center gap-2 border-t bg-secondary px-3 text-xs text-muted-foreground">
    <span class={["size-2 rounded-full", !session.alive ? "bg-destructive" : session.busy > 0 ? "bg-warning" : "bg-success"]}></span>
    <span>Python {session.version} | {session.env ? `environment "${session.env}"` : "system"} | {status}</span>
  </footer>
</div>

<!-- The panels, created once; the layout places each in its slot (see `adopt`). -->
<div class="hidden">
  <div class="h-full" bind:this={editorNode}>
    <Editor
      bind:this={editor}
      onHelp={(w) => {
        showDocs();
        docs.help(w);
      }}
    />
  </div>
  <div class="h-full" bind:this={docsNode}>
    <Docs bind:this={docs} onChooseFolder={actions.chooseDocsFolder} onClose={() => setShowDocs(false)} />
  </div>
  <div class="h-full" bind:this={postNode}>
    <Post
      bind:this={post}
      position={settings.postPosition}
      onMove={(p) => setPostPosition(p)}
      onDragStart={startPostDrag}
    />
  </div>
</div>

<EnvironmentsDialog bind:this={environments} />
<PreferencesDialog bind:this={preferences} onDocsChanged={() => docs.reload()} />
<ConfirmDialog />
