// The page's host (see ../lib/host.ts): the editor in a browser tab.
//
// Its files are the page's storage (opfs.ts), its one session is JavaScript
// in a worker on the page's clausters engine (session.ts), its documentation
// is served beside it (docs.ts) and its configuration is kept in the page's
// local storage. It runs no environments and has no documentation folder to
// choose.

import type { Host } from "$lib/host";
import { listBooks, listDocs, readDoc, searchDocs } from "./docs";
import { emit, listen } from "./events";
import { pickFile } from "./FileDialog.svelte";
import { readText, write } from "./opfs";
import * as session from "./session";

const CONFIG = "clausters-editor:page-config";

interface Config {
  javascript: { startupCode: string; activeEnv: null };
  docsDir: null;
  installSource: string;
}

/** The startup code a page's session starts with until Preferences says otherwise. */
const DEFAULT_STARTUP =
  'import { Session } from "clausters";\n' +
  "const session = (await Session.live()).activate();\n" +
  "const host = await session.gui();\n";

session.setStartupCheck(() => config().javascript.startupCode.trim() !== "");

function config(): Config {
  let startupCode = DEFAULT_STARTUP;
  try {
    const saved = JSON.parse(localStorage.getItem(CONFIG) ?? "{}");
    if (typeof saved.startupCode === "string") startupCode = saved.startupCode;
  } catch {}
  return { javascript: { startupCode, activeEnv: null }, docsDir: null, installSource: "" };
}

function only(lang: unknown) {
  if (lang !== undefined && lang !== "javascript") throw new Error(`this page runs no ${lang} session`);
}

async function invoke<T>(command: string, args: Record<string, any> = {}): Promise<T> {
  only(args.lang);
  switch (command) {
    case "session_start":
      return (await session.start()) as T;
    case "session_restart":
      return (await session.restart()) as T;
    case "session_eval":
      return (await session.evaluate(args as never)) as T;
    case "session_query":
      return session.query(args as never) as T;
    case "session_interrupt":
      return (await session.interrupt()) as T;
    case "get_config":
      return config() as T;
    case "set_startup_code":
      localStorage.setItem(CONFIG, JSON.stringify({ startupCode: String(args.code ?? "") }));
      return config() as T;
    case "read_file":
      return (await readText(String(args.path))) as T;
    case "write_file":
      return (await write(String(args.path), String(args.contents ?? ""))) as T;
    case "list_docs":
      return (await listDocs()) as T;
    case "list_books":
      return (await listBooks()) as T;
    case "read_doc":
      return (await readDoc(String(args.path))) as T;
    case "search_docs":
      return (await searchDocs(String(args.term ?? ""))) as T;
    case "list_mono_fonts":
    case "cli_files":
      return [] as T;
    default:
      throw new Error(`${command}: not something a page does`);
  }
}

export const host: Host = {
  languages: ["javascript"],
  environments: false,
  docsFolder: false,
  windows: session.windows,
  invoke,
  listen,
  openFile: ({ filters }) => pickFile({ mode: "open", filters }),
  saveFile: ({ defaultPath, filters }) =>
    pickFile({ mode: "save", filters, defaultPath: defaultPath?.startsWith("/") ? defaultPath : `/${defaultPath ?? ""}` }),
  openFolder: async () => null,
  openUrl: async (url) => {
    window.open(url, "_blank", "noopener");
  },
  homeDir: async () => "",
  closeWindow: async () => window.close(),
  // A tab cannot wait for the editor's dialog: the browser asks its own
  // question, when there is something unsaved to ask about.
  onCloseRequested: async (_handler, unsaved) => {
    const before = (event: BeforeUnloadEvent) => {
      if (unsaved().length) event.preventDefault();
    };
    window.addEventListener("beforeunload", before);
    return () => window.removeEventListener("beforeunload", before);
  },
  setTheme: async () => {},
};

export { emit };
