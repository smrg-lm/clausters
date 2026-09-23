<script lang="ts">
  import { onMount } from "svelte";
  import { onMessage, type Msg } from "./session.svelte";
  import { settings, setAutoscroll } from "./settings.svelte";
  import Grip from "./Grip.svelte";
  import { Button } from "$lib/components/ui/button";
  import { Toggle } from "$lib/components/ui/toggle";
  import Eraser from "@lucide/svelte/icons/eraser";
  import ArrowDownToLine from "@lucide/svelte/icons/arrow-down-to-line";

  let { onDragStart }: { onDragStart: (e: PointerEvent) => void } = $props();

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
    <Grip label="post window" {onDragStart} />
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
  </div>
  <pre
    class="m-0 min-h-0 flex-1 overflow-auto px-3 py-2 font-mono break-words whitespace-pre-wrap
      [&_.err]:text-destructive [&_.info]:text-muted-foreground [&_.result]:text-success [&_.sys]:text-muted-foreground"
    style:font-size="var(--code-size)"
    bind:this={el}
  ></pre>
</div>
