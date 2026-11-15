// The languages the editor runs: what each one is called, which files are its
// own, how the editor reads it and what its session runs on. Everything else
// -- the evaluation, the post window, the help, the environments -- is the
// same for every language, and asks here for the part that is not. Which of
// them a build runs is its host's to say (`$host`): the desktop application
// runs Python, the page JavaScript.

import { javascriptLanguage } from "@codemirror/lang-javascript";
import { pythonLanguage } from "@codemirror/lang-python";
import type { Language as CodeLanguage } from "@codemirror/language";
import { host } from "$host";

export type Lang = "python" | "javascript";

export interface LangInfo {
  id: Lang;
  /** The language's name. */
  label: string;
  /** What its session runs on, as the status bar names it. */
  runtime: string;
  /** Its files' extensions, the one a new file is saved with first. */
  extensions: string[];
  /** The grammar the editor reads it with: highlighting, indentation, and the
   *  syntax tree the regions and the completions read. Without the language's
   *  own completions: the session's cover the file's names too. */
  language: CodeLanguage;
  /** Where help on a name comes from when no document has it. */
  helpSource: string;
  /** The package that gives the session its completions, signatures and
   *  hover help, which every environment the editor creates installs. */
  intelligence: string;
  /** The command an environment's install source goes after. */
  installer: string;
  /** An example of an install source. */
  sourceExample: string;
  /** An example of startup code. */
  startupExample: string;
}

export const LANGS: Record<Lang, LangInfo> = {
  python: {
    id: "python",
    label: "Python",
    runtime: "Python",
    extensions: ["py"],
    language: pythonLanguage,
    helpSource: "pydoc",
    intelligence: "jedi",
    installer: "pip install",
    sourceExample: "-e /path/to/package",
    startupExample: "from clausters import Session\nfrom clausters.defs import SynthDef, control, sine, out",
  },
  javascript: {
    id: "javascript",
    label: "JavaScript",
    runtime: "JavaScript",
    extensions: ["mjs", "js"],
    language: javascriptLanguage,
    helpSource: "TypeScript",
    intelligence: "TypeScript",
    installer: "",
    sourceExample: "",
    // The page's session: the engine and the GUI host are the page's, and a
    // session in its worker reaches them with no address named.
    startupExample:
      'import { Session } from "clausters";\n' +
      "const session = (await Session.live()).activate();\n" +
      "const host = await session.gui();",
  },
};

/** The languages this build runs, the default first. */
export const LANG_IDS: readonly Lang[] = host.languages;

export const isLang = (v: unknown): v is Lang => typeof v === "string" && (LANG_IDS as readonly string[]).includes(v);

/** The language a file's extension names, or null for another file. */
export function langOfPath(path: string): Lang | null {
  const ext = /\.([^./\\]+)$/.exec(path)?.[1]?.toLowerCase();
  return LANG_IDS.find((l) => ext && LANGS[l].extensions.includes(ext)) ?? null;
}

/** The open and save dialogs' filters: the language's first. */
export function fileFilters(first: Lang) {
  const order = [first, ...LANG_IDS.filter((l) => l !== first)];
  return [
    ...order.map((l) => ({ name: LANGS[l].label, extensions: LANGS[l].extensions })),
    { name: "All files", extensions: ["*"] },
  ];
}
