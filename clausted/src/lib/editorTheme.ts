// The editor theme: interface colors taken from the shadcn theme (CSS variables) and
// VS Code's syntax colors. The same syntax colors are used in the documentation.

import { HighlightStyle } from "@codemirror/language";
import { StyleModule } from "style-mod";
import {
  vscodeDarkInit,
  vscodeDarkStyle,
  vscodeLightInit,
  vscodeLightStyle,
} from "@uiw/codemirror-theme-vscode";

const fromShadcn = {
  background: "var(--background)",
  foreground: "var(--foreground)",
  caret: "var(--foreground)",
  selection: "color-mix(in oklab, var(--ring) 35%, transparent)",
  selectionMatch: "color-mix(in oklab, var(--ring) 20%, transparent)",
  lineHighlight: "color-mix(in oklab, var(--muted) 60%, transparent)",
  gutterBackground: "var(--background)",
  gutterForeground: "var(--muted-foreground)",
  gutterActiveForeground: "var(--foreground)",
  gutterBorder: "transparent",
};

const dark = vscodeDarkInit({ settings: fromShadcn });
const light = vscodeLightInit({ settings: fromShadcn });

export const editorTheme = (isDark: boolean) => (isDark ? dark : light);

/** Highlighting for code outside the editor (the documentation's blocks). */
export const syntaxStyle = {
  dark: HighlightStyle.define(vscodeDarkStyle ?? []),
  light: HighlightStyle.define(vscodeLightStyle ?? []),
};
StyleModule.mount(document, [syntaxStyle.dark.module!, syntaxStyle.light.module!]);
