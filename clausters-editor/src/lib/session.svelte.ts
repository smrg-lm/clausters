// The sessions, one per language: sends code to the backend and dispatches the
// messages that arrive. Each is started the first time a document of its
// language is on screen.

import { host } from "$host";
import { LANGS, type Lang } from "./languages";

/** The questions about code the driver answers right away, even while code runs. */
type Query = "help" | "inspect" | "signatures" | "complete";

export type MsgType = "out" | "err" | "result" | "sys" | "info" | "ready" | "done" | Query;
export interface Msg {
  type: MsgType;
  id: number | null;
  text: string;
  data?: unknown;
  /** The session it came from; none for the editor's own messages. */
  lang?: Lang;
}

/** A position in a document, as the code intelligence reads it: the whole text,
 *  a 1-based line, a 0-based column and the document's path, if it has one. */
export interface CodeAt {
  source: string;
  line: number;
  column: number;
  file?: string | null;
}

/** What the name at a position is (see `describe` in each driver). */
export interface NameInfo {
  name: string;
  /** Its name within its module, e.g. "Server.boot". */
  qualname: string;
  /** Its full path, e.g. "clausters.defs.server.Server.boot", which help is asked about. */
  fullname: string;
  kind: string;
  signature: string;
  params: string[] | null;
  doc: string;
}

/** A signature of the call a position is in (see `signatures` in each driver). */
export interface Signature {
  name: string;
  params: string[];
  /** The parameter the position is at, if any. */
  index: number | null;
  /** Where the call's "(" is: a 1-based line and a 0-based column. */
  bracket: [number, number];
  doc: string;
}

/** A completion at a position (see `complete` in each driver). */
export interface Completion {
  name: string;
  /** Jedi's kind, which both drivers use: module, class, function, instance,
   *  keyword, param, property, statement, path. */
  kind: string;
  /** How many of its characters are already typed. */
  typed: number;
}

export interface SessionState {
  alive: boolean;
  busy: number;
  version: string;
  /** The environment in use; null = the system's interpreter. */
  env: string | null;
}

const fresh = (): SessionState => ({ alive: false, busy: 0, version: "", env: null });

export const sessions = $state<Record<Lang, SessionState>>({ python: fresh(), javascript: fresh() });

let nextId = 1;
const generation: Record<Lang, number> = { python: 0, javascript: 0 };
const pending: Record<Lang, Set<number>> = { python: new Set(), javascript: new Set() };
/** The sessions asked to start, so that each starts once. */
const started = new Set<Lang>();
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

function setPending(lang: Lang) {
  sessions[lang].busy = pending[lang].size;
}

export async function initSession() {
  await host.listen<{ lang: Lang; generation: number; env: string | null }>("session-start", (e) => {
    const { lang } = e.payload;
    started.add(lang);
    generation[lang] = e.payload.generation;
    sessions[lang].env = e.payload.env;
    pending[lang].clear();
    setPending(lang);
  });
  await host.listen<Msg & { lang: Lang }>("session-msg", (e) => {
    const m = e.payload;
    const lang = m.lang;
    switch (m.type) {
      case "ready":
        sessions[lang].alive = true;
        sessions[lang].version = m.text.split(" ")[0];
        info(`${LANGS[lang].runtime} ${sessions[lang].version} ready\n`);
        runStartupCode(lang);
        return;
      case "done":
        if (m.id != null) pending[lang].delete(m.id);
        setPending(lang);
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
  await host.listen<{ lang: Lang; generation: number; code: number | null }>("session-exit", (e) => {
    const { lang } = e.payload;
    if (e.payload.generation !== generation[lang]) return; // the end of a session already restarted
    sessions[lang].alive = false;
    pending[lang].clear();
    setPending(lang);
    info(`${LANGS[lang].runtime} session ended (code ${e.payload.code ?? "?"})\n`);
  });
}

/** Runs the configured startup code (Preferences), showing it in the post window first. */
async function runStartupCode(lang: Lang) {
  let code = "";
  try {
    code = (await host.invoke<Record<Lang, { startupCode: string }>>("get_config"))[lang].startupCode;
  } catch {}
  if (!code.trim()) return;
  info(`Startup code (${LANGS[lang].label}):\n`);
  post({ type: "sys", id: null, text: code.replace(/\s+$/, "") + "\n" });
  await evaluate(lang, code, "<startup code>", 1);
}

export async function evaluate(lang: Lang, code: string, file: string, line: number) {
  const id = nextId++;
  pending[lang].add(id);
  setPending(lang);
  try {
    await host.invoke("session_eval", { lang, id, code, file, line });
  } catch (e) {
    pending[lang].delete(id);
    setPending(lang);
    post({ type: "err", id, text: `${e}\n` });
  }
}

/** Asks `lang`'s driver `op` and waits for its answer; `fallback` if the session
 *  is not running, the request fails or no answer comes within `timeoutMs`. */
function query<T>(lang: Lang, op: Query, args: object, read: (m: Msg) => T, fallback: T, timeoutMs: number): Promise<T> {
  if (!sessions[lang].alive) return Promise.resolve(fallback);
  const id = nextId++;
  return new Promise((resolve) => {
    const done = (value: T) => {
      clearTimeout(timer);
      waiters.delete(id);
      resolve(value);
    };
    const timer = setTimeout(() => done(fallback), timeoutMs);
    waiters.set(id, (m) => done(read(m)));
    host.invoke("session_query", { lang, id, op, args }).catch(() => done(fallback));
  });
}

/** Help on an expression or a dotted path, from the live session: pydoc's in
 *  Python, the TypeScript language service's in JavaScript. */
export const help = (lang: Lang, expr: string) =>
  query(lang, "help", { expr }, (m) => m.text, `No documentation found for '${expr}'.`, 5000);

/** What the name at a position is; null if there is none or no answer in time. */
export const inspect = (lang: Lang, at: CodeAt) =>
  query(lang, "inspect", at, (m) => (m.data as NameInfo | null | undefined) ?? null, null, 1500);

/** The signatures of the call a position is in. */
export const signatures = (lang: Lang, at: CodeAt) =>
  query(lang, "signatures", at, (m) => (m.data as Signature[] | undefined) ?? [], [] as Signature[], 1500);

/** The completions at a position. */
export const complete = (lang: Lang, at: CodeAt) =>
  query(lang, "complete", at, (m) => (m.data as Completion[] | undefined) ?? [], [] as Completion[], 1500);

export async function interrupt(lang: Lang) {
  try {
    await host.invoke("session_interrupt", { lang });
  } catch (e) {
    info(`${e}\n`);
  }
}

/** Starts `lang`'s session if it has not started yet, so that the first
 *  evaluation and the first completion do not wait for it. */
export async function startSession(lang: Lang) {
  if (started.has(lang)) return;
  started.add(lang);
  try {
    await host.invoke("session_start", { lang });
  } catch (e) {
    started.delete(lang);
    post({ type: "err", id: null, text: `${e}\n` });
  }
}

export async function restart(lang: Lang) {
  info(`Restarting the ${LANGS[lang].label} session...\n`);
  sessions[lang].alive = false;
  try {
    await host.invoke("session_restart", { lang });
  } catch (e) {
    post({ type: "err", id: null, text: `${e}\n` });
  }
}
