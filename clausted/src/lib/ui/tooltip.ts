// shadcn-styled tooltips for the whole page. Any `title` attribute (ours, CodeMirror's
// or one in the documentation's HTML) is moved to `data-tip` on hover, so GTK does not
// show its own and all of them follow the theme.

const DELAY = 450;
const GAP = 6;

let tip: HTMLDivElement;
let target: HTMLElement | null = null;
let timer: number | undefined;

function hide() {
  clearTimeout(timer);
  target = null;
  tip.removeAttribute("data-open");
}

function show(el: HTMLElement) {
  const text = el.dataset.tip;
  if (!text || !el.isConnected) return;
  tip.textContent = text;
  tip.setAttribute("data-open", "");
  const r = el.getBoundingClientRect();
  const t = tip.getBoundingClientRect();
  // Below the element; above it if it does not fit. Always inside the window.
  let top = r.bottom + GAP;
  if (top + t.height > innerHeight - 4) top = r.top - t.height - GAP;
  const left = Math.min(Math.max(4, r.left + r.width / 2 - t.width / 2), innerWidth - t.width - 4);
  tip.style.transform = `translate(${Math.round(left)}px, ${Math.round(top)}px)`;
}

function onOver(e: PointerEvent) {
  const el = (e.target as Element).closest?.<HTMLElement>("[title], [data-tip]");
  if (el === target) return;
  hide();
  if (!el) return;
  const title = el.getAttribute("title");
  if (title) {
    el.dataset.tip = title;
    el.removeAttribute("title");
  }
  target = el;
  timer = window.setTimeout(() => target === el && show(el), DELAY);
}

export function installTooltips() {
  tip = document.createElement("div");
  // The same classes as shadcn-svelte's Tooltip.
  tip.className =
    "pointer-events-none fixed top-0 left-0 z-50 max-w-sm rounded-md bg-foreground px-3 py-1.5 " +
    "text-xs text-background opacity-0 transition-opacity data-open:opacity-100";
  tip.setAttribute("role", "tooltip");
  document.body.appendChild(tip);
  document.addEventListener("pointerover", onOver);
  document.addEventListener("pointerdown", hide, true);
  document.addEventListener("keydown", hide, true);
  document.addEventListener("scroll", hide, true);
  window.addEventListener("blur", hide);
}
