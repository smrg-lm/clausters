// What the editor asks of the program it runs in: its files, its sessions,
// its configuration, its window -- here the desktop application
// (`host-desktop.ts`): Tauri, whose Rust side runs the Python session as a
// process and reads the disk. The editor as a page is no longer this
// frontend: it is the GUI host's dock with the editor's application, built
// in crates/clausters-editor/page.
//
// The build imports the implementation as `$host` (vite.config.ts aliases
// it), and nothing else in the frontend names the program. `invoke` and `listen` keep the desktop's command and event
// names, so a component asks the same thing of both.

import type { Lang } from "./languages";

/** A file type an open or save dialog offers. */
export interface FileFilter {
  name: string;
  extensions: string[];
}

/** What closing the window lets its handler refuse. */
export interface CloseRequest {
  preventDefault(): void;
}

export interface Host {
  /** The languages whose sessions this build runs, the default first. */
  readonly languages: readonly Lang[];
  /** Whether a language's session runs in environments the editor manages. */
  readonly environments: boolean;
  /** Whether the documentation folder can be chosen (Preferences). */
  readonly docsFolder: boolean;
  /** Where the session's windows are drawn, in a build that draws them
   *  itself (the page): one element, holding a dock every window is a pane
   *  of, which the editor places in its windows panel. Null where the
   *  session's windows are windows of the platform. */
  readonly windows: HTMLElement | null;
  /** One of the backend's commands, by the desktop's name. */
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  /** One of the backend's events, by the desktop's name. */
  listen<T>(event: string, handler: (event: { payload: T }) => void): Promise<() => void>;
  /** A file to open, chosen by the user; null if none was. */
  openFile(options: { filters: FileFilter[] }): Promise<string | null>;
  /** Where to save a file, chosen by the user; null if nowhere was. */
  saveFile(options: { defaultPath?: string; filters: FileFilter[] }): Promise<string | null>;
  /** A folder, chosen by the user; null if none was. */
  openFolder(options: { title?: string; defaultPath?: string }): Promise<string | null>;
  /** Opens a link outside the editor. */
  openUrl(url: string): Promise<void>;
  /** The folder a recent file's place is written from as `~`, or "" for none. */
  homeDir(): Promise<string>;
  /** Closes the editor's window. */
  closeWindow(): Promise<void>;
  /** Asks `handler` before the window closes; it may refuse. `unsaved` names
   *  the files with changes not saved, for a host that can only ask its own
   *  question and must know at once whether to ask it. */
  onCloseRequested(
    handler: (request: CloseRequest) => void | Promise<void>,
    unsaved: () => string[],
  ): Promise<() => void>;
  /** The window's own theme (its frame), or the system's for null. */
  setTheme(theme: "light" | "dark" | null): Promise<void>;
}
