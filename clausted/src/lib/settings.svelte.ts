// Appearance preferences (theme, font, size), remembered between sessions.

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
// The only font bundled with the app (OFL license); the rest are chosen from the system's.
import "@fontsource/jetbrains-mono/latin-500.css";
import "@fontsource/jetbrains-mono/latin-ext-500.css";

export type ThemePref = "system" | "light" | "dark";

export const THEMES: [ThemePref, string][] = [
  ["light", "Light"],
  ["dark", "Dark"],
  ["system", "System"],
];

export const BUNDLED_FONT = "JetBrains Mono";

/** Font options: the bundled one and the monospaced ones installed on the system. */
export async function fontOptions(): Promise<[string, string][]> {
  let system: string[] = [];
  try {
    system = (await invoke<string[]>("list_mono_fonts")) ?? [];
  } catch {}
  return [
    [BUNDLED_FONT, `${BUNDLED_FONT} (bundled)`],
    ...system.filter((f) => f !== BUNDLED_FONT).map((f): [string, string] => [f, f]),
  ];
}

export const DEFAULT_SIZE = 14;
const MIN_SIZE = 9;
const MAX_SIZE = 32;
const KEY = "clausted.settings";

interface Stored {
  theme: ThemePref;
  font: string;
  size: number;
  /** The post window scrolls to the end by itself with each new output. */
  autoscroll: boolean;
  showDocs: boolean;
  /** Where each panel is: two columns, each holding up to two panels, top first. */
  layout: Layout;
}

/** The panels: one or two code editors, the documentation and the post window. */
export type PanelId = "editor-1" | "editor-2" | "docs" | "post";
export interface Layout {
  left: PanelId[];
  right: PanelId[];
}
export const COLUMN_SLOTS = 2;

const DEFAULT_LAYOUT: Layout = { left: ["editor-1"], right: ["docs", "post"] };

/** A stored layout, if it is a well-formed one: every panel once (the second
 *  editor optional), at most two per column. */
function validLayout(v: unknown): Layout | null {
  const l = v as Layout;
  if (!l || !Array.isArray(l.left) || !Array.isArray(l.right)) return null;
  if (l.left.length > COLUMN_SLOTS || l.right.length > COLUMN_SLOTS) return null;
  const all = [...l.left, ...l.right];
  const known: PanelId[] = ["editor-1", "editor-2", "docs", "post"];
  if (all.some((p) => !known.includes(p)) || new Set(all).size !== all.length) return null;
  if (!all.includes("editor-1") || !all.includes("docs") || !all.includes("post")) return null;
  return { left: [...l.left], right: [...l.right] };
}

function load(): Stored {
  const defaults: Stored = {
    theme: "system",
    font: BUNDLED_FONT,
    size: DEFAULT_SIZE,
    autoscroll: true,
    showDocs: true,
    layout: DEFAULT_LAYOUT,
  };
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return {
      theme: THEMES.some(([t]) => t === v.theme) ? v.theme : defaults.theme,
      font: typeof v.font === "string" && v.font ? v.font : defaults.font,
      size: Number.isFinite(v.size) ? Math.min(MAX_SIZE, Math.max(MIN_SIZE, v.size)) : defaults.size,
      autoscroll: typeof v.autoscroll === "boolean" ? v.autoscroll : defaults.autoscroll,
      showDocs: typeof v.showDocs === "boolean" ? v.showDocs : defaults.showDocs,
      // The layout replaced a post-window position (right of the editor, or below it).
      layout:
        validLayout(v.layout) ??
        (v.postPosition === "bottom" ? { left: ["editor-1", "post"], right: ["docs"] } : { ...DEFAULT_LAYOUT }),
    };
  } catch {
    return defaults;
  }
}

export const settings = $state({ ...load(), dark: false });

const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

// What GTK draws (file dialogs) follows the chosen light/dark variant;
// with "system" control is handed back to the desktop.
let gtkTheme: ThemePref | null | undefined; // undefined = not applied yet
function syncNativeTheme() {
  const wanted = settings.theme === "system" ? null : settings.theme;
  if (wanted === gtkTheme) return;
  gtkTheme = wanted;
  try {
    getCurrentWindow().setTheme(wanted).catch(() => {});
  } catch {} // outside Tauri
}

function apply() {
  syncNativeTheme();
  settings.dark = settings.theme === "dark" || (settings.theme === "system" && systemDark.matches);
  const root = document.documentElement;
  root.classList.toggle("dark", settings.dark);
  // If the chosen font is no longer installed, it falls back to the bundled one.
  root.style.setProperty("--mono", `"${settings.font.replace(/["\\]/g, "")}", "${BUNDLED_FONT}", monospace`);
  root.style.setProperty("--code-size", `${settings.size}px`);
  try {
    const { theme, font, size, autoscroll, showDocs, layout } = settings;
    localStorage.setItem(KEY, JSON.stringify({ theme, font, size, autoscroll, showDocs, layout }));
  } catch {}
}

export function setTheme(theme: ThemePref) {
  settings.theme = theme;
  apply();
}

export function setFont(font: string) {
  settings.font = font;
  apply();
}

export function setShowDocs(show: boolean) {
  settings.showDocs = show;
  apply();
}

export function setLayout(layout: Layout) {
  settings.layout = layout;
  apply();
}

export function setAutoscroll(on: boolean) {
  settings.autoscroll = on;
  apply();
}

/** Changes the size by `delta` px; `null` goes back to the default size. */
export function zoom(delta: number | null) {
  settings.size = delta === null ? DEFAULT_SIZE : Math.min(MAX_SIZE, Math.max(MIN_SIZE, settings.size + delta));
  apply();
}

/**
 * The grid's geometry, in percent: the left column's width and where each column
 * is cut in two. A moved panel takes the size of the place it goes to where that
 * size is defined, and keeps its own where it is not (see `move` in App.svelte).
 * Not reactive: it is written on every frame of a divider's drag and read when the
 * panes are laid out.
 */
export interface GridSizes {
  x: number;
  left: number;
  right: number;
}
const SIZES_KEY = "clausted.grid";
const DEFAULT_SIZES: GridSizes = { x: 58, left: 60, right: 60 };

function loadSizes(): GridSizes {
  try {
    const v = JSON.parse(localStorage.getItem(SIZES_KEY) ?? "{}");
    const pct = (n: unknown, d: number) => (typeof n === "number" && n > 0 && n < 100 ? n : d);
    return { x: pct(v.x, DEFAULT_SIZES.x), left: pct(v.left, DEFAULT_SIZES.left), right: pct(v.right, DEFAULT_SIZES.right) };
  } catch {
    return { ...DEFAULT_SIZES };
  }
}

export const gridSizes: GridSizes = loadSizes();

// Before the grid had its own sizes, paneforge kept one set per arrangement under
// `paneforge:clausted-*`; nothing reads them any more.
try {
  for (const key of Object.keys(localStorage)) {
    if (key.startsWith("paneforge:clausted-")) localStorage.removeItem(key);
  }
} catch {}

export function setGridSize(key: keyof GridSizes, value: number) {
  if (!(value > 0 && value < 100) || gridSizes[key] === value) return;
  gridSizes[key] = value;
  try {
    localStorage.setItem(SIZES_KEY, JSON.stringify(gridSizes));
  } catch {}
}

systemDark.addEventListener("change", apply);
apply();
