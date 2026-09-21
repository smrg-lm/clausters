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
  /** Where the post window goes: below the documentation or below the editor. */
  postPosition: PostPosition;
}

export type PostPosition = "right" | "bottom";

function load(): Stored {
  const defaults: Stored = {
    theme: "system",
    font: BUNDLED_FONT,
    size: DEFAULT_SIZE,
    autoscroll: true,
    showDocs: true,
    postPosition: "right",
  };
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}");
    return {
      theme: THEMES.some(([t]) => t === v.theme) ? v.theme : defaults.theme,
      font: typeof v.font === "string" && v.font ? v.font : defaults.font,
      size: Number.isFinite(v.size) ? Math.min(MAX_SIZE, Math.max(MIN_SIZE, v.size)) : defaults.size,
      autoscroll: typeof v.autoscroll === "boolean" ? v.autoscroll : defaults.autoscroll,
      showDocs: typeof v.showDocs === "boolean" ? v.showDocs : defaults.showDocs,
      postPosition: v.postPosition === "bottom" ? "bottom" : "right",
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
    const { theme, font, size, autoscroll, showDocs, postPosition } = settings;
    localStorage.setItem(KEY, JSON.stringify({ theme, font, size, autoscroll, showDocs, postPosition }));
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

export function setPostPosition(position: PostPosition) {
  settings.postPosition = position;
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

systemDark.addEventListener("change", apply);
apply();
