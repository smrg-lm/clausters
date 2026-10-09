// The Python session: sends code to the backend and dispatches the messages that arrive.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** The questions about code the driver answers right away, even while code runs. */
type Query = "help" | "inspect" | "signatures" | "complete";

export type MsgType = "out" | "err" | "result" | "sys" | "info" | "ready" | "done" | Query;
export interface Msg {
  type: MsgType;
  id: number | null;
  text: string;
  data?: unknown;
}

/** A position in the editor's text, as Jedi reads it: the whole text, a 1-based
 *  line and a 0-based column. */
export interface CodeAt {
  source: string;
  line: number;
  column: number;
}

/** What the name at a position is (see `describe` in driver.py). */
export interface PyInfo {
  name: string;
  /** Its name within its module, e.g. "Server.boot". */
  qualname: string;
  /** Its dotted path, e.g. "clausters.defs.server.Server.boot". */
  fullname: string;
  kind: string;
  signature: string;
  params: string[] | null;
  doc: string;
}

/** A signature of the call a position is in (see `signatures` in driver.py). */
export interface PySignature {
  name: string;
  params: string[];
  /** The parameter the position is at, if any. */
  index: number | null;
  /** Where the call's "(" is: a 1-based line and a 0-based column. */
  bracket: [number, number];
  doc: string;
}

/** A completion at a position (see `complete` in driver.py). */
export interface PyCompletion {
  name: string;
  /** Jedi's kind: module, class, function, instance, keyword, param, property, statement, path. */
  kind: string;
  /** How many of its characters are already typed. */
  typed: number;
}

export const session = $state({
  alive: false,
  busy: 0,
  version: "",
  /** The virtual environment in use; null = the system Python. */
  env: null as string | null,
});

let nextId = 1;
let generation = 0;
const pending = new Set<number>();
/** The queries waiting for their answer, by id. */
const waiters = new Map<number, (m: Msg) => void>();
const listeners = new Set<(m: Msg) => void>();

/** Subscribes to the output messages (out, err, result, sys, info). */
export function onMessage(fn: (m: Msg) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function post(m: Msg) {
  for (const fn of listeners) fn(m);
}

export function info(text: string) {
  post({ type: "info", id: null, text });
}

function setPending() {
  session.busy = pending.size;
}

export async function initSession() {
  await listen<{ generation: number; env: string | null }>("py-start", (e) => {
    generation = e.payload.generation;
    session.env = e.payload.env;
    pending.clear();
    setPending();
  });
  await listen<Msg>("py-msg", (e) => {
    const m = e.payload;
    switch (m.type) {
      case "ready":
        session.alive = true;
        session.version = m.text.split(" ")[0];
        info(`Python ${session.version} ready\n`);
        runStartupCode();
        return;
      case "done":
        if (m.id != null) pending.delete(m.id);
        setPending();
        return;
      case "help":
      case "inspect":
      case "signatures":
      case "complete":
        if (m.id != null) waiters.get(m.id)?.(m);
        return;
      default:
        post(m);
    }
  });
  await listen<{ generation: number; code: number | null }>("py-exit", (e) => {
    if (e.payload.generation !== generation) return; // the end of a session already restarted
    session.alive = false;
    pending.clear();
    setPending();
    info(`Session ended (code ${e.payload.code ?? "?"})\n`);
  });
}

/** Runs the configured startup code (Preferences), showing it in the post window first. */
async function runStartupCode() {
  let code = "";
  try {
    code = (await invoke<{ startupCode: string }>("get_config")).startupCode;
  } catch {}
  if (!code.trim()) return;
  info("Startup code:\n");
  post({ type: "sys", id: null, text: code.replace(/\s+$/, "") + "\n" });
  await evaluate(code, "<startup code>", 1);
}

export async function evaluate(code: string, file: string, line: number) {
  const id = nextId++;
  pending.add(id);
  setPending();
  try {
    await invoke("py_eval", { id, code, file, line });
  } catch (e) {
    pending.delete(id);
    setPending();
    post({ type: "err", id, text: `${e}\n` });
  }
}

/** Asks the driver `op` and waits for its answer; `fallback` if the session is not
 *  running, the request fails or no answer comes within `timeoutMs`. */
function query<T>(op: Query, args: object, read: (m: Msg) => T, fallback: T, timeoutMs: number): Promise<T> {
  if (!session.alive) return Promise.resolve(fallback);
  const id = nextId++;
  return new Promise((resolve) => {
    const done = (value: T) => {
      clearTimeout(timer);
      waiters.delete(id);
      resolve(value);
    };
    const timer = setTimeout(() => done(fallback), timeoutMs);
    waiters.set(id, (m) => done(read(m)));
    invoke("py_query", { id, op, args }).catch(() => done(fallback));
  });
}

/** pydoc's documentation for an expression or a dotted path, in the live session. */
export const pyHelp = (expr: string) =>
  query("help", { expr }, (m) => m.text, `No documentation found for '${expr}'.`, 5000);

/** What the name at a position is; null if there is none or no answer in time. */
export const pyInspect = (at: CodeAt) =>
  query("inspect", at, (m) => (m.data as PyInfo | null | undefined) ?? null, null, 1500);

/** The signatures of the call a position is in. */
export const pySignatures = (at: CodeAt) =>
  query("signatures", at, (m) => (m.data as PySignature[] | undefined) ?? [], [] as PySignature[], 1500);

/** The completions at a position. */
export const pyComplete = (at: CodeAt) =>
  query("complete", at, (m) => (m.data as PyCompletion[] | undefined) ?? [], [] as PyCompletion[], 1500);

export async function interrupt() {
  try {
    await invoke("py_interrupt");
  } catch (e) {
    info(`${e}\n`);
  }
}

/** Starts the session when the app opens so that the first evaluation does not wait. */
export async function startSession() {
  try {
    await invoke("py_restart");
  } catch (e) {
    post({ type: "err", id: null, text: `${e}\n` });
  }
}

export async function restart() {
  info("Restarting the session...\n");
  session.alive = false;
  try {
    await invoke("py_restart");
  } catch (e) {
    post({ type: "err", id: null, text: `${e}\n` });
  }
}
