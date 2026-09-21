// The Python session: sends code to the backend and dispatches the messages that arrive.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type MsgType = "out" | "err" | "result" | "sys" | "info" | "ready" | "help" | "inspect" | "done";
export interface Msg {
  type: MsgType;
  id: number | null;
  text: string;
  data?: unknown;
}

/** Description of a name in the session (see `describe` in driver.py). */
export interface PyInfo {
  name: string;
  /** The object's `__qualname__` (e.g. "Server.boot"), if it has one. */
  qualname: string | null;
  kind: string;
  signature: string;
  params: string[] | null;
  doc: string;
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
const helpWaiters = new Map<number, (text: string) => void>();
const inspectWaiters = new Map<number, (info: PyInfo | null) => void>();
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
        if (m.id != null) {
          helpWaiters.get(m.id)?.(m.text);
          helpWaiters.delete(m.id);
        }
        return;
      case "inspect":
        if (m.id != null) {
          inspectWaiters.get(m.id)?.((m.data as PyInfo | undefined) ?? null);
          inspectWaiters.delete(m.id);
        }
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

/** pydoc's documentation for an expression, evaluated in the live session. */
export function pyHelp(expr: string): Promise<string> {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    helpWaiters.set(id, resolve);
    invoke("py_help", { id, expr }).catch((e) => {
      helpWaiters.delete(id);
      reject(e);
    });
  });
}

/** Signature and docstring of a dotted name, according to the live session. null if it
 *  is unknown or the session does not answer in time. */
export function pyInspect(expr: string, timeoutMs = 1500): Promise<PyInfo | null> {
  if (!session.alive) return Promise.resolve(null);
  const id = nextId++;
  return new Promise((resolve) => {
    const done = (info: PyInfo | null) => {
      clearTimeout(timer);
      inspectWaiters.delete(id);
      resolve(info);
    };
    const timer = setTimeout(() => done(null), timeoutMs);
    inspectWaiters.set(id, done);
    invoke("py_inspect", { id, expr }).catch(() => done(null));
  });
}

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
