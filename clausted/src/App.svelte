<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import * as Resizable from "$lib/components/ui/resizable";
  import { Button } from "$lib/components/ui/button";
  import { Separator } from "$lib/components/ui/separator";
  import Editor, { type MovedTab } from "./lib/Editor.svelte";
  import Post from "./lib/Post.svelte";
  import Docs from "./lib/Docs.svelte";
  import AppMenu, { type MenuActions } from "./lib/AppMenu.svelte";
  import ConfirmDialog from "./lib/ConfirmDialog.svelte";
  import EnvironmentsDialog from "./lib/EnvironmentsDialog.svelte";
  import PreferencesDialog from "./lib/PreferencesDialog.svelte";
  import { initSession, interrupt, restart, session, startSession } from "./lib/session.svelte";
  import {
    COLUMN_SLOTS,
    gridSizes,
    setGridSize,
    setLayout,
    setShowDocs,
    settings,
    zoom,
    type Layout,
    type PanelId,
  } from "./lib/settings.svelte";
  import Play from "@lucide/svelte/icons/play";
  import ListVideo from "@lucide/svelte/icons/list-video";
  import Square from "@lucide/svelte/icons/square";
  import RotateCcw from "@lucide/svelte/icons/rotate-ccw";

  const editors: Partial<Record<PanelId, Editor>> = $state({});
  let post: Post;
  let docs: Docs;
  let environments: EnvironmentsDialog;
  let preferences: PreferencesDialog;

  // --- Editor groups ---
  // One code editor, or two side by side; the menu and the shortcuts act on the one
  // that had the focus last.
  const editorIds = $derived(
    (["editor-1", "editor-2"] as PanelId[]).filter((id) => [...settings.layout.left, ...settings.layout.right].includes(id)),
  );
  let activeEditor = $state<PanelId>("editor-1");
  const editor = () => editors[activeEditor] ?? editors["editor-1"]!;

  /** Shows `path` in another group than `from`, if one has it open. */
  function openElsewhere(from: PanelId, path: string): boolean {
    for (const id of editorIds) {
      if (id !== from && editors[id]?.showPath(path)) return true;
    }
    return false;
  }

  const clone = (l: Layout): Layout => ({ left: [...l.left], right: [...l.right] });
  const columnOf = (l: Layout, id: PanelId) => (l.left.includes(id) ? "left" : "right");

  /** Opens a second editor in the other column; with two, closes the second and
   *  hands its tabs to the first. */
  function toggleSplit() {
    const l = clone(settings.layout);
    if (editorIds.includes("editor-2")) {
      const moved: MovedTab[] = editors["editor-2"]?.takeTabs(true) ?? [];
      l.left = l.left.filter((p) => p !== "editor-2");
      l.right = l.right.filter((p) => p !== "editor-2");
      setLayout(l);
      activeEditor = "editor-1";
      editors["editor-1"]!.putTabs(moved);
      editors["editor-1"]!.focus();
      return;
    }
    const home = columnOf(l, "editor-1");
    const other = home === "left" ? "right" : "left";
    l[other].unshift("editor-2");
    // A full column hands its bottom panel to the first editor's column.
    if (l[other].length > COLUMN_SLOTS) l[home].push(l[other].pop()!);
    setLayout(l);
    activeEditor = "editor-2";
  }

  /** Moves the current editor's active tab to the other editor. */
  function moveTab() {
    const to = editorIds.find((id) => id !== activeEditor);
    if (!to) return;
    const moved = editor().takeTabs();
    editors[to]!.putTabs(moved);
    activeEditor = to;
    editors[to]!.focus();
  }

  // --- Panel layout ---
  // Each panel is created only once (below, under "panels") and *moved* into the slot it belongs in:
  // changing the layout does not destroy the editor, the output or the documentation's history.
  const nodes: Partial<Record<PanelId, HTMLElement>> = $state({});

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

  // What is on screen: the hidden documentation keeps its place in the layout, so
  // showing it again puts it back there; a column left empty is not drawn.
  const shown = (col: PanelId[]) => col.filter((p) => p !== "docs" || settings.showDocs);
  const columns = $derived(
    (["left", "right"] as const)
      .map((side) => ({ side, panels: shown(settings.layout[side]) }))
      .filter((c) => c.panels.length > 0),
  );
  const layoutKey = $derived(columns.map((c) => c.panels.join("+")).join("|"));

  function showDocs() {
    if (!settings.showDocs) setShowDocs(true);
  }

  // --- Dragging a panel (by the grip on its bar) to one of the four places ---
  // A place: the top or bottom of a column, or the whole column ("full").
  type Side = "left" | "right";
  type Place = { side: Side; row: 0 | 1 | "full" };
  type Edge = "top" | "bottom" | "left" | "right";
  /** A place a dragged panel can go to, drawn as the space it would take there. */
  interface Zone {
    place: Place;
    label: string;
    /** The space the panel would take. With two panels those spaces overlap, so
     *  only the one under the pointer is drawn; the others are a label centered on
     *  `strip`, the band along their side of the area. */
    rect: DOMRect;
    strip?: DOMRect;
    edge?: Edge;
  }
  let mainEl: HTMLElement;
  const columnEls: Partial<Record<Side, HTMLElement>> = $state({});
  let drag = $state<{ zones: Zone[]; over: Zone | null } | null>(null);

  const otherSide = (side: Side): Side => (side === "left" ? "right" : "left");
  const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

  /**
   * Puts `id` at `to`: into the column if it has room, or in place of the panel
   * there, which goes to where `id` was; for a whole column, the panel that was in
   * it goes to the other one. Only what is on screen counts: the hidden
   * documentation takes no place while panels move, and goes back after.
   *
   * The sizes follow one rule: a moved panel takes the size of the place it goes to
   * where that size is defined, and keeps its own where it is not. In place of
   * another panel, both are defined (the two swap cells). Into a column holding one
   * panel, the width is the column's and the column is cut so that the panel keeps
   * its height -- at half when it had the whole height, alone in its column. Into a
   * column that is not drawn, the new column keeps the panel's width, unless it had
   * the whole width. A column left empty is not drawn, and the other one takes the
   * whole width.
   */
  function move(id: PanelId, to: Place) {
    const l = clone(settings.layout);
    const hiddenSide = settings.showDocs ? null : columnOf(l, "docs");
    if (hiddenSide) l[hiddenSide] = l[hiddenSide].filter((p) => p !== "docs");

    const main = mainEl.getBoundingClientRect();
    const own = nodes[id]!.getBoundingClientRect();
    const height = (100 * own.height) / main.height;
    const width = (100 * own.width) / main.width;
    const drawn = l[to.side].filter((p) => p !== id).length; // what the target shows, but for `id`
    const from = columnOf(l, id);
    const fromIndex = l[from].indexOf(id);
    l[from].splice(fromIndex, 1);
    const target = l[to.side];

    if (to.row === "full") {
      l[otherSide(to.side)].push(...target.splice(0));
      target.push(id);
    } else if (target.length < COLUMN_SLOTS) {
      target.splice(to.row === 0 ? 0 : target.length, 0, id);
    } else {
      const displaced = target[to.row];
      target[to.row] = id;
      l[from].splice(fromIndex, 0, displaced);
    }

    // Within its own column, the place it goes to has a height: it takes it.
    if (to.row !== "full" && from !== to.side && drawn === 1 && target.length === 2) {
      const cut = height > 95 ? 50 : clamp(height, 10, 90);
      setGridSize(to.side, target[0] === id ? cut : 100 - cut);
    } else if (drawn === 0 && width < 95) {
      const x = clamp(width, 15, 85);
      setGridSize("x", to.side === "left" ? x : 100 - x);
    }

    if (hiddenSide) l[l[hiddenSide].length < COLUMN_SLOTS ? hiddenSide : otherSide(hiddenSide)].push("docs");
    setLayout(l);
  }

  /** Two panels on screen: the four sides of the area, but the one `id` is on.
   *  Top and bottom stack the two (in the other panel's column), left and right put
   *  them side by side. */
  function edgeZones(id: PanelId, main: DOMRect): Zone[] {
    const ownSide = columnOf(settings.layout, id);
    const ownColumn = shown(settings.layout[ownSide]);
    const stacked = ownColumn.length === 2;
    const other = stacked ? ownColumn.find((p) => p !== id)! : shown(settings.layout[otherSide(ownSide)])[0];
    const otherColumn = columnOf(settings.layout, other);
    const current: Edge = stacked ? (ownColumn[0] === id ? "top" : "bottom") : ownSide;
    const { x, y, width: w, height: h } = main;
    const t = 0.22; // a strip's share of the area
    // The labels sit on the area's middle lines, where the halves meet: a drawn half
    // stops short of them so that its border never crosses a label.
    const g = 36;
    const all: Zone[] = [
      { edge: "top", label: "Top", place: { side: otherColumn, row: 0 }, rect: new DOMRect(x, y, w, h / 2 - g), strip: new DOMRect(x, y, w, h * t) },
      { edge: "bottom", label: "Bottom", place: { side: otherColumn, row: 1 }, rect: new DOMRect(x, y + h / 2 + g, w, h / 2 - g), strip: new DOMRect(x, y + h * (1 - t), w, h * t) },
      { edge: "left", label: "Left", place: { side: "left", row: "full" }, rect: new DOMRect(x, y, w / 2 - g, h), strip: new DOMRect(x, y, w * t, h) },
      { edge: "right", label: "Right", place: { side: "right", row: "full" }, rect: new DOMRect(x + w / 2 + g, y, w / 2 - g, h), strip: new DOMRect(x + w * (1 - t), y, w * t, h) },
    ];
    return all.filter((z) => z.edge !== current);
  }

  /** Three or four panels: the places of the grid as they are drawn -- each panel's
   *  own space in a column of two, the two halves of a column of one -- but the one
   *  `id` is in. */
  function cellZones(id: PanelId): Zone[] {
    const zones: Zone[] = [];
    for (const c of columns) {
      const col = columnEls[c.side]?.getBoundingClientRect();
      if (!col) continue;
      const rows: DOMRect[] =
        c.panels.length === 2
          ? c.panels.map((p) => nodes[p]!.getBoundingClientRect())
          : [new DOMRect(col.x, col.y, col.width, col.height / 2), new DOMRect(col.x, col.y + col.height / 2, col.width, col.height / 2)];
      rows.forEach((rect, row) => {
        if (c.panels.length === 2 && c.panels[row] === id) return;
        if (c.panels.length === 1 && c.panels[0] === id) return;
        const label = `${row === 0 ? "Top" : "Bottom"} ${c.side}`;
        zones.push({ label, place: { side: c.side, row: row as 0 | 1 }, rect });
      });
    }
    return zones;
  }

  function startDrag(id: PanelId, e: PointerEvent) {
    e.preventDefault();
    const main = mainEl.getBoundingClientRect();
    const onScreen = columns.reduce((n, c) => n + c.panels.length, 0);
    const byEdge = onScreen === 2;
    drag = { zones: byEdge ? edgeZones(id, main) : cellZones(id), over: null };
    const moveOver = (ev: PointerEvent) => {
      if (!drag) return;
      if (byEdge) {
        // The nearest side of the area; nothing when that is the side the panel is on.
        const d: Record<Edge, number> = {
          left: (ev.clientX - main.left) / main.width,
          right: (main.right - ev.clientX) / main.width,
          top: (ev.clientY - main.top) / main.height,
          bottom: (main.bottom - ev.clientY) / main.height,
        };
        const nearest = (Object.keys(d) as Edge[]).reduce((a, b) => (d[b] < d[a] ? b : a));
        drag.over = drag.zones.find((z) => z.edge === nearest) ?? null;
      } else {
        drag.over =
          drag.zones.find(
            ({ rect: r }) => ev.clientX >= r.left && ev.clientX <= r.right && ev.clientY >= r.top && ev.clientY <= r.bottom,
          ) ?? null;
      }
    };
    const up = () => {
      if (drag?.over) move(id, drag.over.place);
      drag = null;
      window.removeEventListener("pointermove", moveOver);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", moveOver);
    window.addEventListener("pointerup", up);
  }

  const actions: MenuActions = {
    newFile: () => editor().newFile(),
    openFile: () => editor().openFile(),
    saveFile: (saveAs) => editor().saveFile(saveAs),
    closeTab: () => editor().closeTab(),
    quit: () => getCurrentWindow().close(),
    runLine: () => editor().runLine(),
    runBlock: () => editor().runBlock(),
    toggleSplit,
    moveTab,
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
    {#key layoutKey}
      <!-- The sizes come from the grid (gridSizes), whatever panels are in it. -->
      <Resizable.PaneGroup
        direction="horizontal"
        onLayoutChange={(sizes) => {
          if (sizes.length === 2) setGridSize("x", sizes[0]);
        }}
      >
        {#each columns as col, c (col.side)}
          {#if c > 0}<Resizable.Handle />{/if}
          <Resizable.Pane
            id={col.side}
            order={c + 1}
            defaultSize={columns.length === 1 ? 100 : c === 0 ? gridSizes.x : 100 - gridSizes.x}
            minSize={15}
          >
            <div class="h-full" bind:this={columnEls[col.side]}>
              {#if col.panels.length === 2}
                <Resizable.PaneGroup
                  direction="vertical"
                  onLayoutChange={(sizes) => {
                    if (sizes.length === 2) setGridSize(col.side, sizes[0]);
                  }}
                >
                  <Resizable.Pane id={col.panels[0]} order={1} defaultSize={gridSizes[col.side]} minSize={10}>
                    <div class="h-full" {@attach adopt(nodes[col.panels[0]])}></div>
                  </Resizable.Pane>
                  <Resizable.Handle />
                  <Resizable.Pane id={col.panels[1]} order={2} defaultSize={100 - gridSizes[col.side]} minSize={10}>
                    <div class="h-full" {@attach adopt(nodes[col.panels[1]])}></div>
                  </Resizable.Pane>
                </Resizable.PaneGroup>
              {:else}
                <div class="h-full" {@attach adopt(nodes[col.panels[0]])}></div>
              {/if}
            </div>
          </Resizable.Pane>
        {/each}
      </Resizable.PaneGroup>
    {/key}

    <!-- Drop zones while dragging a panel -->
    {#if drag}
      <div class="fixed inset-0 z-40 cursor-grabbing">
        {#each drag.zones as zone (zone.label)}
          {@const over = drag.over === zone}
          {#if zone.strip && !over}
            <div
              class="absolute -translate-1/2 rounded-md border-2 border-dashed border-muted-foreground/40 bg-background/80 px-3 py-1 text-sm font-medium text-muted-foreground"
              style:left="{zone.strip.x + zone.strip.width / 2}px"
              style:top="{zone.strip.y + zone.strip.height / 2}px"
            >
              {zone.label}
            </div>
          {:else}
            {@const rect = zone.rect}
            <div
              class={[
                "absolute flex items-center justify-center rounded-lg border-2 border-dashed text-sm font-medium transition-colors",
                over
                  ? "border-ring bg-ring/20 text-foreground"
                  : "border-muted-foreground/40 bg-background/40 text-muted-foreground",
              ]}
              style:left="{rect.left + 6}px"
              style:top="{rect.top + 6}px"
              style:width="{rect.width - 12}px"
              style:height="{rect.height - 12}px"
            >
              {zone.label}
            </div>
          {/if}
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
  {#each editorIds as id (id)}
    <div class="h-full" bind:this={nodes[id]}>
      <Editor
        bind:this={editors[id]}
        current={editorIds.length === 1 || activeEditor === id}
        onFocus={() => (activeEditor = id)}
        onDragStart={(e) => startDrag(id, e)}
        openElsewhere={(path) => openElsewhere(id, path)}
        onHelp={(w) => {
          showDocs();
          docs.help(w);
        }}
      />
    </div>
  {/each}
  <div class="h-full" bind:this={nodes.docs}>
    <Docs
      bind:this={docs}
      onChooseFolder={actions.chooseDocsFolder}
      onClose={() => setShowDocs(false)}
      onDragStart={(e) => startDrag("docs", e)}
    />
  </div>
  <div class="h-full" bind:this={nodes.post}>
    <Post bind:this={post} onDragStart={(e) => startDrag("post", e)} />
  </div>
</div>

<EnvironmentsDialog bind:this={environments} />
<PreferencesDialog bind:this={preferences} onDocsChanged={() => docs.reload()} />
<ConfirmDialog />
