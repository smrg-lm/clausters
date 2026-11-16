<script lang="ts">
  // The session's windows, in a build that draws them itself (the page): one
  // dock the host draws every window into as a pane, with its own tabs and
  // splits. Its element is the host's (`host.windows`); this panel only gives
  // it a place in the grid and a bar to drag it by.
  import { host } from "$host";
  import Grip from "./Grip.svelte";

  let { onDragStart }: { onDragStart: (e: PointerEvent) => void } = $props();

  const adopt = (box: HTMLElement) => {
    if (host.windows) box.appendChild(host.windows);
  };
</script>

<div class="flex h-full flex-col">
  <div class="flex h-9 shrink-0 items-center gap-0.5 border-b bg-secondary px-1">
    <Grip label="windows" {onDragStart} />
    <span class="px-1 text-xs text-muted-foreground">Windows</span>
  </div>
  <div class="min-h-0 flex-1" {@attach adopt}></div>
</div>
