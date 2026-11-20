// The page's JavaScript session, from the page's side: the worker that runs
// the code (session-worker.ts), the one that answers questions about it
// (code-worker.ts), and the clausters engine and GUI host the page keeps for
// the session's code to reach. What they say is told to the editor
// (`Listener`), in the messages of the desktop's driver.
//
// The engine and the host are the page's, booted once, so a session that is
// restarted -- or terminated because its code is stuck in a loop -- finds them
// as it left them: a new worker gets new links onto the same engine and host.
// What the session's code left sounding is silenced from the page when the
// session goes, since nothing in the worker can do it any more.

/** Where the clausters web package is served, beside the page. */
export const CLAUSTERS = new URL(import.meta.env.VITE_CLAUSTERS ?? "./clausters/", document.baseURI).href;

/** **Who hears the session**: the editor, which the page tells. */
export interface Listener {
  /** One message of the session's workers, as the driver's JSON. */
  message(json: string): void;
  /** The session's worker `generation` started. */
  started(generation: number): void;
  /** The session's worker `generation` ended, with its code if it gave one. */
  ended(generation: number, code: number | null): void;
}

let listener: Listener = { message() {}, started() {}, ended() {} };

/** The session tells `heard` from now on. */
export function tell(heard: Listener): void {
  listener = heard;
}

const say = (msg: Record<string, unknown>) => listener.message(JSON.stringify(msg));

/** The packages the session's code imports, by name: where each is served. */
const PACKAGES = { clausters: CLAUSTERS };

/** How long the session's worker has to take an interrupt before it is taken
 *  to be stuck in synchronous code, and terminated. */
const INTERRUPT_MS = 400;

export interface Runtime {
  clausters: any;
  page: any;
  engine: any;
  gui: any;
  /** A handle of the page's own on the engine, made the first time the page
   *  silences it: what silences it. */
  server: any | null;
}

let runtime: Promise<Runtime> | null = null;

/** **The page's engine and GUI host, brought up once**, the host's dock in
 *  `stage`: the editor's windows and the session's are its panes. The
 *  engine's audio starts at the page's first key or click, which a browser
 *  requires; until then the engine answers nothing. */
export function bringUp(stage?: HTMLElement): Promise<Runtime> {
  runtime ??= (async () => {
    const clausters = await import(/* @vite-ignore */ new URL("dist/index.js", CLAUSTERS).href);
    const page = await import(/* @vite-ignore */ new URL("dist/page.js", CLAUSTERS).href);
    const engine = await page.server({ channels: 2 });
    const gui = await page.guiHost();
    if (stage) gui.dock(stage);
    // Asked again at every key and click until the audio runs: a browser may
    // not count the first one (Firefox does not count a modifier, the Shift
    // of Shift+Enter).
    const resume = () => {
      if (engine.context.state === "running") {
        window.removeEventListener("keydown", resume, true);
        window.removeEventListener("pointerdown", resume, true);
        return;
      }
      engine.resume().catch(() => {});
    };
    window.addEventListener("keydown", resume, true);
    window.addEventListener("pointerdown", resume, true);
    return { clausters, page, engine, gui, server: null };
  })();
  return runtime;
}

/** Waits for the engine's audio to run, saying once why the session waits. */
async function running(engine: any): Promise<void> {
  if (engine.context.state === "running") return;
  say({
    type: "info",
    id: null,
    text: "The JavaScript session starts at the first key or click in the page: a browser lets a page start its audio only then.\n",
  });
  await new Promise<void>((resolve) => {
    const check = () => {
      if (engine.context.state !== "running") return;
      engine.context.removeEventListener("statechange", check);
      resolve();
    };
    engine.context.addEventListener("statechange", check);
    check();
  });
}

/** Silences what the session's code left sounding, from the page. */
async function silence(): Promise<void> {
  const rt = await runtime;
  if (!rt || rt.engine.context.state !== "running") return;
  rt.server ??= await new rt.clausters.Server({ engine: rt.engine }).attach({ adoptDefault: false });
  rt.server.panic();
}

// ---- the session ----

let worker: Worker | null = null;
let links: { engine: { close(): void }; gui: { close(): void } } | null = null;
let generation = 0;
/** Answers the interrupt the worker took, while one is asked. */
let taken: (() => void) | null = null;

const code = new Worker(new URL("./code-worker.ts", import.meta.url), { type: "module" });
code.postMessage({ op: "packages", packages: PACKAGES });
code.addEventListener("message", (event) => listener.message(JSON.stringify(event.data)));

function stop(): void {
  worker?.terminate();
  worker = null;
  links?.engine.close();
  links?.gui.close();
  links = null;
}

/** The session starting, which an evaluation asked for meanwhile waits for:
 *  until the editor has heard it is ready, and sent its startup code, which
 *  runs first. */
let starting: Promise<void> = Promise.resolve();
/** Lets the evaluations waiting go. */
let started: () => void = () => {};

export function start(): Promise<void> {
  const gate = new Promise<void>((resolve) => (started = resolve));
  const begun = begin();
  starting = begun.then(() => gate);
  return begun;
}

async function begin(): Promise<void> {
  stop();
  const { engine, gui } = await bringUp();
  await running(engine);
  const mine = ++generation;
  worker = new Worker(new URL("./session-worker.ts", import.meta.url), { type: "module" });
  const engineLink = engine.link();
  const guiLink = gui.link();
  links = { engine: engineLink, gui: guiLink };
  worker.addEventListener("message", (event) => {
    if (mine !== generation) return;
    if (event.data.type === "interrupted") return taken?.();
    // the editor hears it is ready, and sends its startup code at once if it
    // has any: what waited goes after it
    listener.message(JSON.stringify(event.data));
    if (event.data.type === "ready") started();
  });
  worker.addEventListener("error", (event) => {
    if (mine !== generation) return;
    say({ type: "err", id: null, text: `The session's worker failed: ${event.message}\n` });
  });
  listener.started(mine);
  worker.postMessage(
    { op: "start", engine: engineLink.port, gui: guiLink.port, packages: PACKAGES },
    [engineLink.port, guiLink.port],
  );
}

/** One request to run, as the driver reads one: `{id, op: "eval", code,
 *  file, line}`. The startup code goes at once; the rest waits for it. */
export async function evaluate(request: { id: number; code: string; file: string; line: number }): Promise<void> {
  const startup = request.file === "<startup code>";
  if (!startup) await starting;
  if (worker === null) throw new Error("The JavaScript session is not running");
  worker.postMessage({ ...request, op: "eval" });
}

/** One question about code, as the driver reads one: `{id, op, ...}`. */
export function query(request: Record<string, unknown>): void {
  code.postMessage(request);
}

/** Ends the session: what its code left sounding is silenced, and it says so. */
async function end(code_: number | null): Promise<void> {
  const ended = generation;
  stop();
  await silence();
  listener.ended(ended, code_);
}

/** The session ends, the page going. */
export function kill(): void {
  stop();
}

export async function restart(): Promise<void> {
  await end(0);
  await start();
}

/** The interrupt key. A session waiting for something takes it at once: its
 *  evaluation stops and its packages' interrupt hooks run. One stuck in
 *  synchronous code cannot take anything, so it is terminated and started
 *  again, which loses what it had defined, and the page silences the engine. */
export async function interrupt(): Promise<void> {
  if (worker === null) return;
  const answered = await new Promise<boolean>((resolve) => {
    const timer = setTimeout(() => resolve(false), INTERRUPT_MS);
    taken = () => {
      clearTimeout(timer);
      resolve(true);
    };
    worker!.postMessage({ op: "interrupt" });
  }).finally(() => {
    taken = null;
  });
  if (answered) return;
  say({
    type: "err",
    id: null,
    text: "Interrupted: the session was running synchronous code and could not stop it, so it was started again (what it had defined is gone).\n",
  });
  await end(null);
  await start();
}
