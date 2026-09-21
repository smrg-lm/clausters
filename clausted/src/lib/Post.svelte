<script lang="ts">
  import { onMount } from "svelte";
  import { onMessage, type Msg } from "./session.svelte";
  import { settings, setAutoscroll, type PostPosition } from "./settings.svelte";
  import { Button } from "$lib/components/ui/button";
  import { Toggle } from "$lib/components/ui/toggle";
  import Eraser from "@lucide/svelte/icons/eraser";
  import ArrowDownToLine from "@lucide/svelte/icons/arrow-down-to-line";
  import GripVertical from "@lucide/svelte/icons/grip-vertical";
  import PanelBottom from "@lucide/svelte/icons/panel-bottom";
  import PanelRight from "@lucide/svelte/icons/panel-right";

  let {
    position,
    onMove,
    onDragStart,
  }: {
    position: PostPosition;
    onMove: (position: PostPosition) => void;
    onDragStart: (e: PointerEvent) => void;
  } = $props();

  const MAX_CHUNKS = 4000;
  const MAX_CHUNK_LEN = 8000;

  let el: HTMLPreElement;
  let last: HTMLSpanElement | null = null;

  export function clear() {
    el.textContent = "";
    last = null;
  }

  // Written straight into the DOM: the output only grows and can be large.
  function append(m: Msg) {
    let text = m.type === "result" ? `-> ${m.text}\n` : m.text;
    // Our own notices always start on a new line.
    if (m.type === "info" && last && !last.textContent!.endsWith("\n")) text = "\n" + text;
    const merge =
      last && last.dataset.type === m.type && m.type !== "result" && last.textContent!.length < MAX_CHUNK_LEN;
    if (merge) {
      last!.append(text);
    } else {
      last = document.createElement("span");
      last.className = m.type;
      last.dataset.type = m.type;
      last.textContent = text;
      el.appendChild(last);
      while (el.childElementCount > MAX_CHUNKS) el.firstElementChild!.remove();
    }
    if (settings.autoscroll) el.scrollTop = el.scrollHeight;
  }

  onMount(() => onMessage(append));
</script>

<div class="flex h-full flex-col">
  <div class="flex h-9 shrink-0 items-center gap-0.5 border-b bg-secondary px-1">
    <!-- Handle for dragging the window to another position -->
    <div
      class="flex h-8 w-5 cursor-grab touch-none items-center justify-center text-muted-foreground hover:text-foreground"
      role="button"
      tabindex="-1"
      title="Drag to move the post window"
      aria-label="Move the post window"
      onpointerdown={onDragStart}
    >
      <GripVertical class="size-4" />
    </div>
    <Button variant="ghost" size="icon-sm" onclick={clear} title="Clear the post window" aria-label="Clear the post window">
      <Eraser />
    </Button>
    <Toggle
      size="sm"
      class="w-8 px-0"
      pressed={settings.autoscroll}
      onPressedChange={(on) => {
        setAutoscroll(on);
        if (on) el.scrollTop = el.scrollHeight;
      }}
      title="Scroll to the end with each new output"
      aria-label="Autoscroll"
    >
      <ArrowDownToLine />
    </Toggle>
    <span class="flex-1"></span>
    {#if position === "right"}
      <Button variant="ghost" size="icon-sm" onclick={() => onMove("bottom")} title="Move below the editor" aria-label="Move below the editor">
        <PanelBottom />
      </Button>
    {:else}
      <Button variant="ghost" size="icon-sm" onclick={() => onMove("right")} title="Move to the right" aria-label="Move to the right">
        <PanelRight />
      </Button>
    {/if}
  </div>
  <pre
    class="m-0 min-h-0 flex-1 overflow-auto px-3 py-2 font-mono break-words whitespace-pre-wrap
      [&_.err]:text-destructive [&_.info]:text-muted-foreground [&_.result]:text-success [&_.sys]:text-muted-foreground"
    style:font-size="var(--code-size)"
    bind:this={el}
  ></pre>
</div>
