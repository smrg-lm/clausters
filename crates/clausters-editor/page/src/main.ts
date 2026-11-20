// The editor as a page: the page's GUI host, its dock across the whole
// window, and the editor's application (the same Rust as the desktop's
// program, built for the page: ../editor/) a client of that host, as a
// session is. The host draws every panel; the application decides what they
// show. This module answers the application with what only a page's script
// reaches: the session's workers (session.ts), the page's storage, the books
// the page serves (docs.ts) and the browser's upload and download.

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
/** `name` in folder `dir`. */
const join = (dir: string, name: string) => `${dir === "/" ? "" : dir}/${name}`;

/** Hands the page's file at `path` to the browser as a download. */
async function download(path: string): Promise<void> {
  const url = URL.createObjectURL(new Blob([await opfs.readFile(path)]));
  const a = document.createElement("a");
  a.href = url;
  a.download = path.split("/").pop() || "file";
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Asks for files from the disk and copies them into folder `dir`. */
function upload(dir: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.onchange = async () => {
      try {
        await opfs.makeDirectory(dir).catch(() => {});
        for (const file of input.files ?? []) {
          await opfs.writeFile(join(dir, file.name), new Uint8Array(await file.arrayBuffer()));
        }
        resolve();
      } catch (error) {
        reject(error);
      }
    };
    input.oncancel = () => resolve();
    input.click();
  });
}

/** Removes a file, or a folder and everything in it. */
async function remove(path: string): Promise<void> {
  try {
    await opfs.removeFile(path);
  } catch {
    await opfs.removeDirectory(path, true);
  }
}

/** One change of the editor's file dialog to the page's storage. */
async function change(asked: string): Promise<void> {
  const { op, path, to } = JSON.parse(asked) as { op: string; path: string; to: string | null };
  switch (op) {
    case "makeFolder":
      return opfs.makeDirectory(path);
    case "rename":
      try {
        return await opfs.moveFile(path, to!);
      } catch {
        throw new Error(`${path} was not renamed: only a file is renamed here`);
      }
    case "remove":
      return remove(path);
    case "upload":
      return upload(path);
    case "download":
      return download(path);
    default:
      throw new Error(`${op}: not a change of the page's storage`);
  }
}

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
  change,
  readDocs,
});
connection.addReply((packet: Uint8Array) => editor.host(packet));
session.tell({
  message: (json) => editor.session(json),
  started: (generation) => editor.sessionStarted(generation),
  ended: (generation, code) => editor.sessionEnded(generation, code ?? undefined),
});
editor.start();

// A tab cannot wait for the editor's question: the browser asks its own, when
// a file has changes that are not saved.
window.addEventListener("beforeunload", (event) => {
  if (editor.unsaved()) event.preventDefault();
});
