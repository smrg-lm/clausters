// The editor as a page: the page's GUI host, its dock across the whole
// window, and the editor's application (the same Rust as the desktop's
// program, built for the page: ../editor/) a client of that host, as a
// session is. The host draws every panel; the application decides what they
// show. This module answers the application with what only a page's script
// reaches: the session's workers (session.ts), the page's storage and the
// books the page serves (docs.ts). What its file dialog changes in that
// storage -- a folder, a rename, a delete, an upload, a download -- the
// host's chooser carries out itself, for every application.

import init, { PageEditor } from "../editor/clausters_editor.js";
import { readDocs } from "./docs";
import * as session from "./session";

const stage = document.getElementById("dock")!;
const [runtime] = await Promise.all([session.bringUp(stage), init()]);
const { opfs, portConnection } = runtime.page;

// The application's link to the host, as a session's worker has one.
const connection = await portConnection(runtime.gui.link().port);

/** The folder holding `path`. */
const parentOf = (path: string) => path.replace(/\/[^/]*$/, "") || "/";

const editor = new PageEditor({
  send: (packet: Uint8Array) => {
    if (connection.gone()) return false;
    // a copy: the packet is a view of the application's memory
    connection.send(packet.slice());
    return true;
  },
  startSession: () => session.start(),
  evaluate: (request: string) => session.evaluate(JSON.parse(request)),
  ask: (request: string) => session.query(JSON.parse(request)),
  interrupt: () => session.interrupt(),
  restart: () => session.restart(),
  stop: () => session.kill(),
  readText: async (path: string) => {
    try {
      return new TextDecoder().decode(await opfs.readFile(path));
    } catch {
      throw new Error("no such file in the page's storage");
    }
  },
  write: async (path: string, text: string) => {
    await opfs.makeDirectory(parentOf(path)).catch(() => {});
    await opfs.writeFile(path, new TextEncoder().encode(text));
  },
  readDocs,
});
connection.addReply((packet: Uint8Array) => editor.host(packet));
session.tell({
  message: (json) => editor.session(json),
  started: (generation) => editor.sessionStarted(generation),
  ended: (generation, code) => editor.sessionEnded(generation, code ?? undefined),
});
editor.start();

// A tab cannot wait for the host's question: the browser asks its own when a
// quit would lose work -- a file with changes, or a document of the host's
// applications, which the host tells the editor as it changes.
window.addEventListener("beforeunload", (event) => {
  if (editor.unsaved()) event.preventDefault();
});
