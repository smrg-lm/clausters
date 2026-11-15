// The desktop application's host (see host.ts): Tauri.
//
// Its Rust side runs one session, Python's, under its own command and event
// names (`py_eval`, `py-msg`...), and keeps the configuration of that one
// language at the top level. The editor asks per language, so the session's
// commands and events are named here for the language they belong to.

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { homeDir } from "@tauri-apps/api/path";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open, save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Host } from "./host";

/** The desktop's configuration, of its one language, as the editor reads it per language. */
function perLanguage(config: Record<string, unknown>) {
  return {
    ...config,
    python: { startupCode: config.startupCode ?? "", activeEnv: config.activeEnv ?? null },
  };
}

/** The editor's command as the desktop's: the session's and the environments' are Python's. */
async function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  const { lang, ...rest } = args;
  if (lang !== undefined && lang !== "python") throw new Error(`this editor runs no ${lang} session`);
  switch (command) {
    case "session_start":
    case "session_restart":
      return tauriInvoke<T>("py_restart");
    case "session_eval":
      return tauriInvoke<T>("py_eval", rest);
    case "session_query":
      return tauriInvoke<T>("py_query", rest);
    case "session_interrupt":
      return tauriInvoke<T>("py_interrupt");
    case "get_config":
    case "set_startup_code":
    case "set_docs_dir":
      return perLanguage(await tauriInvoke<Record<string, unknown>>(command, rest)) as T;
    default:
      return tauriInvoke<T>(command, rest);
  }
}

const EVENTS: Record<string, string> = {
  "session-start": "py-start",
  "session-msg": "py-msg",
  "session-exit": "py-exit",
};

export const host: Host = {
  languages: ["python"],
  environments: true,
  docsFolder: true,
  invoke,
  listen: <T>(event: string, handler: (event: { payload: T }) => void) => {
    const own = EVENTS[event];
    if (own === undefined) return tauriListen<T>(event, handler);
    // The session's messages carry the language they come from.
    return tauriListen<T>(own, (e) => handler({ payload: { ...(e.payload as object), lang: "python" } as T }));
  },
  openFile: async ({ filters }) => {
    const path = await open({ multiple: false, filters });
    return typeof path === "string" ? path : null;
  },
  saveFile: async ({ defaultPath, filters }) => (await save({ defaultPath, filters })) ?? null,
  openFolder: async ({ title, defaultPath }) => {
    const dir = await open({ directory: true, title, defaultPath });
    return typeof dir === "string" ? dir : null;
  },
  openUrl: (url) => openUrl(url),
  homeDir: () => homeDir(),
  closeWindow: () => getCurrentWindow().close(),
  onCloseRequested: (handler) => getCurrentWindow().onCloseRequested(handler),
  setTheme: (theme) => getCurrentWindow().setTheme(theme),
};
