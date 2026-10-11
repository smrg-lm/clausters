<script lang="ts">
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { info, onMessage, type Msg } from "./session.svelte";
  import { findLinks } from "./markdown";
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
  // Kept beside `last` so that appending never reads its text back: reading
  // textContent builds the whole chunk as a string, on every message.
  let lastLen = 0;
  let lastEndsLine = true;
  // Messages arrive one event each, and a burst can be thousands of them. They
  // are written once per frame: touching the DOM and scrolling per message
  // makes the browser lay out the whole window again for every one, which is
  // what froze the editor under a flood of output.
  let queue: Msg[] = [];
  let frame = 0;

  export function clear() {
    el.textContent = "";
    last = null;
    lastLen = 0;
    lastEndsLine = true;
  }

  /** The text as DOM nodes, with every web address a link the browser opens. */
  function linked(text: string): DocumentFragment {
    const out = document.createDocumentFragment();
    let at = 0;
    for (const link of findLinks(text)) {
      out.append(text.slice(at, link.index));
      const a = document.createElement("a");
      a.href = link.url;
      a.textContent = text.slice(link.index, link.lastIndex);
      out.append(a);
      at = link.lastIndex;
    }
    out.append(text.slice(at));
    return out;
  }

  function onClick(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest("a");
    if (!a) return;
    e.preventDefault();
    openUrl(a.href).catch((err) => info(`${err}\n`));
  }

  // Written straight into the DOM: the output only grows and can be large.
  function write(m: Msg) {
    let text = m.type === "result" ? `-> ${m.text}\n` : m.text;
    if (!text) return;
    // Our own notices always start on a new line.
    if (m.type === "info" && last && !lastEndsLine) text = "\n" + text;
    const merge = last && last.dataset.type === m.type && m.type !== "result" && lastLen < MAX_CHUNK_LEN;
    if (merge) {
      last!.append(linked(text));
      lastLen += text.length;
    } else {
      last = document.createElement("span");
      last.className = m.type;
      last.dataset.type = m.type;
      last.append(linked(text));
      el.appendChild(last);
      lastLen = text.length;
    }
    lastEndsLine = text.endsWith("\n");
  }

  function flush() {
    frame = 0;
    const batch = queue;
    queue = [];
    for (const m of batch) write(m);
    while (el.childElementCount > MAX_CHUNKS) el.firstElementChild!.remove();
    if (settings.autoscroll) el.scrollTop = el.scrollHeight;
  }

  function append(m: Msg) {
    queue.push(m);
    if (!frame) frame = requestAnimationFrame(flush);
  }

  onMount(() => {
    const off = onMessage(append);
    return () => {
      off();
      cancelAnimationFrame(frame);
    };
  });
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
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <pre
    class="m-0 min-h-0 flex-1 overflow-auto px-3 py-2 font-mono break-words whitespace-pre-wrap
      [&_a]:text-foreground [&_a]:underline [&_a]:underline-offset-2 [&_.err]:text-destructive [&_.info]:text-muted-foreground [&_.result]:text-success [&_.sys]:text-muted-foreground"
    style:font-size="var(--code-size)"
    bind:this={el}
    onclick={onClick}
  ></pre>
</div>
