<script lang="ts">
  // The handle a panel is dragged by, to another place in the grid; the same in every panel.
  import GripVertical from "@lucide/svelte/icons/grip-vertical";

  let { label, onDragStart }: { label: string; onDragStart: (e: PointerEvent) => void } = $props();
</script>

<div
  class="flex h-8 w-5 shrink-0 cursor-grab touch-none items-center justify-center text-muted-foreground hover:text-foreground"
  role="button"
  tabindex="-1"
  title="Drag to move the {label}"
  aria-label="Move the {label}"
  onpointerdown={onDragStart}
>
  <GripVertical class="size-4" />
</div>
