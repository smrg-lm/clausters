// The page's JavaScript session: a worker that runs the evaluations, one at a
// time, in its own global scope, so what one evaluation defines the next one
// sees -- the desktop's Node.js driver, in a page.
//
// The page starts it with a link to its clausters engine and one to its GUI
// host (`ClaustersServer.link`, `ClaustersGui.link`), which this worker leaves
// where the clausters client looks for a worker's default address
// (`Symbol.for("clausters.links")`), so the session's code writes
// `Session.live()` and `session.gui()` as a Python script does; the engine and
// the host stay the page's, so terminating this worker -- what stops code stuck in a loop -- leaves
// them running. Questions about code (help, completions) are another worker's
// (code-worker.ts), so they are answered while code runs here.
//
// Messages, both ways, are the desktop driver's protocol as objects:
//   page -> worker  {op: "start", engine, gui, packages}
//                   {op: "eval", id, code, file, line}
//                   {op: "interrupt"}
//   worker -> page  {type: "ready" | "out" | "err" | "result" | "done", id, text}
//                   {type: "interrupted"}  the interrupt was taken

import { parse } from "acorn";
import type { Node } from "acorn";

/** The global the rewritten code calls into; not enumerable, so not in the way. */
const HOOK = "__clausters_editor__";

type AnyNode = Node & Record<string, any>;

const post = (type: string, id: number | null, text = "") => self.postMessage({ type, id, text });

/** The request whose output is being written; later output keeps it. */
let current: number | null = null;

// ---------------------------------------------------------------------------
// Showing a value

/** A value as the post window shows it: strings as they are at the top level
 *  of a `console.log`, everything else as it reads in a console. */
function show(value: unknown, depth = 2, seen = new Set<unknown>()): string {
  if (typeof value === "string") return depth === 2 ? value : JSON.stringify(value);
  if (value === null || value === undefined || typeof value === "number" || typeof value === "boolean") return String(value);
  if (typeof value === "bigint") return `${value}n`;
  if (typeof value === "symbol") return value.toString();
  if (typeof value === "function") return value.name ? `[Function: ${value.name}]` : "[Function (anonymous)]";
  if (value instanceof Error) return String(value.stack ?? `${value.name}: ${value.message}`);
  if (seen.has(value)) return "[Circular]";
  if (depth < 0) return Array.isArray(value) ? "[Array]" : "[Object]";
  seen.add(value);
  try {
    if (Array.isArray(value)) {
      const items = value.slice(0, 100).map((v) => show(v, depth - 1, seen));
      if (value.length > 100) items.push(`... ${value.length - 100} more items`);
      return `[ ${items.join(", ")} ]`;
    }
    if (ArrayBuffer.isView(value) && !(value instanceof DataView)) {
      const arr = value as unknown as ArrayLike<number>;
      const items = Array.from({ length: Math.min(arr.length, 100) }, (_, i) => String(arr[i]));
      if (arr.length > 100) items.push(`... ${arr.length - 100} more items`);
      return `${value.constructor.name}(${arr.length}) [ ${items.join(", ")} ]`;
    }
    if (value instanceof Map) {
      const items = [...value].slice(0, 100).map(([k, v]) => `${show(k, depth - 1, seen)} => ${show(v, depth - 1, seen)}`);
      return `Map(${value.size}) { ${items.join(", ")} }`;
    }
    if (value instanceof Set) {
      return `Set(${value.size}) { ${[...value].slice(0, 100).map((v) => show(v, depth - 1, seen)).join(", ")} }`;
    }
    if (value instanceof Promise) return "Promise { <pending> }";
    const name = Object.getPrototypeOf(value)?.constructor?.name;
    const keys = Object.keys(value as object);
    const fields = keys.slice(0, 50).map((k) => `${/^[\w$]+$/.test(k) ? k : JSON.stringify(k)}: ${show((value as Record<string, unknown>)[k], depth - 1, seen)}`);
    if (keys.length > 50) fields.push(`... ${keys.length - 50} more`);
    const body = fields.length ? `{ ${fields.join(", ")} }` : "{}";
    return name && name !== "Object" ? `${name} ${body}` : body;
  } finally {
    seen.delete(value);
  }
}

/** An error as the post window shows it: its stack, without this worker's frames. */
function describeError(error: unknown): string {
  if (!(error instanceof Error)) return `Uncaught ${show(error, 1)}\n`;
  const own = self.location.href.replace(/[?#].*$/, "");
  const lines = String(error.stack ?? "").split("\n");
  // Chrome writes "Name: message" and then frames; Firefox only frames.
  const head = lines[0]?.startsWith(error.name) ? [] : [`${error.name}: ${error.message}`];
  const kept = lines.filter((l) => !l.includes(own));
  return `${[...head, ...kept].join("\n").replace(/\s+$/, "")}\n`;
}

// ---------------------------------------------------------------------------
// The console: every write is the session's output

const consoleOut = (type: "out" | "err") => (...args: unknown[]) => post(type, current, `${args.map((a) => show(a)).join(" ")}\n`);
console.log = consoleOut("out");
console.info = consoleOut("out");
console.debug = consoleOut("out");
console.warn = consoleOut("err");
console.error = consoleOut("err");

self.addEventListener("error", (event) => {
  post("err", current, describeError(event.error ?? event.message));
  event.preventDefault();
});
self.addEventListener("unhandledrejection", (event) => {
  post("err", current, describeError(event.reason));
  event.preventDefault();
});

// ---------------------------------------------------------------------------
// Rewriting an evaluation into a script
//
// A script, not a module, so that what it declares outlives it: the worker's
// global scope is the one every evaluation shares. Its top-level `const`,
// `let` and `class` become `var`, so evaluating a declaration again redefines
// it rather than failing; `import` becomes a call that resolves the way the
// page does; `export` is dropped. Code that awaits at the top level runs
// inside an async function, its declarations hoisted out of it. The line
// numbers are the document's: nothing is added between lines.

function syntaxError(error: unknown, filename: string, line: number): never {
  const loc = (error as { loc?: { line: number; column: number } }).loc;
  if (!(error instanceof SyntaxError) || !loc) throw error;
  const at = `${filename}:${loc.line + line - 1}:${loc.column + 1}`;
  const message = error.message.replace(/ \(\d+:\d+\)$/, "");
  throw Object.assign(new SyntaxError(message), { stack: `SyntaxError: ${message}\n    at ${at}` });
}

export function rewrite(source: string, filename = "<editor>", line = 1) {
  let tree: AnyNode;
  try {
    tree = parse(source, { ecmaVersion: "latest", sourceType: "module", allowHashBang: true, locations: true }) as AnyNode;
  } catch (error) {
    syntaxError(error, filename, line);
  }

  const edits: { from: number; to: number; text: string }[] = [];
  const replace = (from: number, to: number, text: string) => edits.push({ from, to, text });
  const body: AnyNode[] = tree.body;
  const wrapped = hasTopLevelAwait(tree) || body.some((s) => s.type === "ImportDeclaration");
  const hoisted = new Set<string>();
  const functions: string[] = [];

  // Dynamic `import(...)` and `import.meta`, wherever they are.
  walk(tree, (node) => {
    if (node.type === "ImportExpression") replace(node.start, node.source.start, `${HOOK}.import(`);
    else if (node.type === "MetaProperty" && node.meta.name === "import") replace(node.start, node.end, `${HOOK}.meta`);
  });

  const declare = (decl: AnyNode) => {
    if (decl.type === "VariableDeclaration") {
      if (!wrapped) return replace(decl.start, decl.start + decl.kind.length, "var");
      // The declarators become assignments to the hoisted names, in place:
      // `const a = 1, {b} = o` reads `;(a = 1), ({b} = o)`. The leading `;`
      // keeps a line before without one from calling the parenthesis.
      for (const d of decl.declarations) {
        for (const n of patternNames(d.id)) hoisted.add(n);
        if (!d.init) continue;
        replace(d.id.start, d.id.start, "(");
        replace(d.init.end, d.init.end, ")");
      }
      replace(decl.start, decl.start + decl.kind.length, ";");
    } else if (decl.type === "ClassDeclaration") {
      const name = decl.id.name;
      if (wrapped) {
        hoisted.add(name);
        replace(decl.start, decl.start, `;${name} = `);
      } else {
        replace(decl.start, decl.start, `var ${name} = `);
      }
      replace(decl.end, decl.end, ";");
    } else if (decl.type === "FunctionDeclaration" && wrapped) {
      // Hoisted inside the async function; the global is set before anything runs.
      hoisted.add(decl.id.name);
      functions.push(decl.id.name);
    }
  };

  for (const stmt of body) {
    if (stmt.type === "ImportDeclaration") {
      replace(stmt.start, stmt.end, importCall(stmt, source, hoisted));
    } else if (stmt.type === "ExportNamedDeclaration" || stmt.type === "ExportDefaultDeclaration") {
      const decl = stmt.declaration;
      if (!decl) {
        replace(stmt.start, stmt.end, ";");
        continue;
      }
      replace(stmt.start, decl.start, "");
      if (decl.type.endsWith("Declaration") && decl.id) declare(decl);
      else if (stmt.type === "ExportDefaultDeclaration") replace(decl.end, decl.end, ";");
    } else if (stmt.type === "ExportAllDeclaration") {
      replace(stmt.start, stmt.end, ";");
    } else {
      declare(stmt);
    }
  }

  const last = body[body.length - 1];
  const display = last?.type === "ExpressionStatement" && !last.directive;
  if (wrapped && display) {
    replace(last.start, last.start, "return (");
    replace(last.expression.end, last.expression.end, ")");
  }

  let code = applyEdits(source, edits);
  if (wrapped) {
    const names = [...hoisted].filter((n) => n !== HOOK);
    const head = names.length ? `var ${names.join(", ")}; ` : "";
    const globals = functions.map((f) => `globalThis.${f} = ${f}; `).join("");
    code = `${head}(async () => { ${globals}${code}\n})()`;
  }
  return { code, display, wrapped };
}

/** An import declaration as a call that resolves the way the page does. */
function importCall(stmt: AnyNode, source: string, hoisted: Set<string>): string {
  const spec = JSON.stringify(stmt.source.value);
  const attributes = stmt.attributes?.length
    ? `, { with: { ${stmt.attributes.map((a: AnyNode) => source.slice(a.start, a.end)).join(", ")} } }`
    : "";
  const call = `await ${HOOK}.import(${spec}${attributes})`;
  if (!stmt.specifiers.length) return `;${call};`;
  const namespace = stmt.specifiers.find((s: AnyNode) => s.type === "ImportNamespaceSpecifier");
  const fields: string[] = [];
  for (const s of stmt.specifiers as AnyNode[]) {
    hoisted.add(s.local.name);
    if (s.type === "ImportDefaultSpecifier") fields.push(`default: ${s.local.name}`);
    else if (s.type === "ImportSpecifier") {
      const imported = s.imported.type === "Literal" ? JSON.stringify(s.imported.value) : s.imported.name;
      fields.push(imported === s.local.name ? imported : `${imported}: ${s.local.name}`);
    }
  }
  if (namespace) {
    const rest = fields.length ? ` ({ ${fields.join(", ")} } = ${namespace.local.name});` : "";
    return `;${namespace.local.name} = ${call};${rest}`;
  }
  return `;({ ${fields.join(", ")} } = ${call});`;
}

function applyEdits(source: string, edits: { from: number; to: number; text: string }[]): string {
  edits.sort((a, b) => a.from - b.from || a.to - b.to);
  let out = "";
  let at = 0;
  for (const e of edits) {
    if (e.from < at) continue; // inside a range already replaced
    out += source.slice(at, e.from) + e.text;
    at = e.to;
  }
  return out + source.slice(at);
}

/** Every node of the tree, depth first. */
function walk(node: AnyNode, visit: (node: AnyNode, inFunction: boolean) => void, inFunction = false) {
  if (!node || typeof node.type !== "string") return;
  visit(node, inFunction);
  const fn = /Function/.test(node.type);
  for (const key of Object.keys(node)) {
    if (key === "loc") continue;
    const child = node[key];
    if (Array.isArray(child)) for (const c of child) walk(c, visit, inFunction || fn);
    else if (child && typeof child.type === "string") walk(child, visit, inFunction || fn);
  }
}

function hasTopLevelAwait(tree: AnyNode): boolean {
  let found = false;
  walk(tree, (node, inFunction) => {
    if (inFunction) return;
    if (node.type === "AwaitExpression" || (node.type === "ForOfStatement" && node.await)) found = true;
  });
  return found;
}

/** The names a binding pattern declares. */
function patternNames(p: AnyNode | null): string[] {
  if (!p) return [];
  switch (p.type) {
    case "Identifier":
      return [p.name];
    case "ObjectPattern":
      return p.properties.flatMap((q: AnyNode) => patternNames(q.type === "RestElement" ? q.argument : q.value));
    case "ArrayPattern":
      return p.elements.flatMap((e: AnyNode | null) => patternNames(e));
    case "RestElement":
      return patternNames(p.argument);
    case "AssignmentPattern":
      return patternNames(p.left);
    default:
      return [];
  }
}

// ---------------------------------------------------------------------------
// Imports: the packages the page serves, and the page's own files

/** A package the page serves: where it is, and its manifest. */
interface Package {
  base: string;
  manifest: { exports?: Record<string, unknown>; "clausters-editor"?: { interrupt?: string } };
}

const packages = new Map<string, Package>();
/** The packages the session imported, by name, for the interrupt hooks. */
const imported = new Set<string>();

/** A bare specifier's package: `pkg` or `@scope/pkg`. */
function packageName(specifier: string): string | null {
  if (/^[./]/.test(specifier) || /^[a-z][a-z0-9+.-]*:/i.test(specifier)) return null;
  const parts = specifier.split("/");
  return specifier.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0];
}

/** Where an export of a package leads, from its manifest's `exports`. */
function exported(pkg: Package, subpath: string): string | null {
  const entry = pkg.manifest.exports?.[subpath];
  const target =
    typeof entry === "string"
      ? entry
      : ((entry as Record<string, unknown> | undefined)?.import ??
        (entry as Record<string, unknown> | undefined)?.browser ??
        (entry as Record<string, unknown> | undefined)?.default);
  return typeof target === "string" ? new URL(target, pkg.base).href : null;
}

/** The URL `specifier` leads to from `from` (a file of the page's storage, or none). */
async function resolve(specifier: string, from: string | null): Promise<string> {
  if (/^[a-z][a-z0-9+.-]*:/i.test(specifier)) return specifier;
  if (/^\.{0,2}\//.test(specifier)) {
    const base = from ?? "/";
    const path = new URL(specifier, `opfs:${base}`).pathname;
    return localModule(path);
  }
  const name = packageName(specifier)!;
  const pkg = packages.get(name);
  if (!pkg) {
    throw new Error(
      `Cannot find package '${name}': the page serves ${[...packages.keys()].map((n) => `'${n}'`).join(", ") || "no package"}`,
    );
  }
  const subpath = `.${specifier.slice(name.length)}`;
  const url = exported(pkg, subpath);
  if (!url) throw new Error(`'${specifier}' is not exported by '${name}'`);
  imported.add(name);
  return url;
}

/** A file of the page's storage as text, read with the clausters package's
 *  own reader of that storage. */
async function readText(path: string): Promise<string> {
  const base = packages.get("clausters")?.base;
  if (base === undefined) throw new Error(`Could not read ${path}: the page serves no clausters package`);
  const { readFile } = await import(/* @vite-ignore */ new URL("dist/engine/opfs.js", base).href);
  try {
    return new TextDecoder().decode(await readFile(path));
  } catch {
    throw new Error(`Could not read ${path}: no such file in the page's storage`);
  }
}

/** The page's own modules, by their text: the same text is the same module. */
const locals = new Map<string, string>();

/** A module of the page's storage as a URL the worker can import: its own
 *  imports resolved the same way, so it may import the page's packages and
 *  the files beside it. */
async function localModule(path: string): Promise<string> {
  const source = await readText(path);
  let tree: AnyNode;
  try {
    tree = parse(source, { ecmaVersion: "latest", sourceType: "module", locations: true }) as AnyNode;
  } catch (error) {
    syntaxError(error, path, 1);
  }
  const edits: { from: number; to: number; text: string }[] = [];
  const specifiers: AnyNode[] = [];
  walk(tree, (node) => {
    if (
      (node.type === "ImportDeclaration" || node.type === "ExportAllDeclaration" || node.type === "ExportNamedDeclaration") &&
      node.source
    ) {
      specifiers.push(node.source);
    } else if (node.type === "ImportExpression" && node.source.type === "Literal") {
      specifiers.push(node.source);
    }
  });
  for (const literal of specifiers) {
    const url = await resolve(String(literal.value), path);
    edits.push({ from: literal.start, to: literal.end, text: JSON.stringify(url) });
  }
  const text = `${applyEdits(source, edits)}\n//# sourceURL=${path}`;
  let url = locals.get(text);
  if (url === undefined) {
    url = URL.createObjectURL(new Blob([text], { type: "text/javascript" }));
    locals.set(text, url);
  }
  return url;
}

// ---------------------------------------------------------------------------
// The interrupt key: the code stops, then whatever the packages say
//
// A package may give the key more to stop than the code: what is still
// sounding. It names a function in its package.json,
// `"clausters-editor": { "interrupt": "./dist/editor.js#interrupt" }`, and the
// session calls it after the key -- only when the package was imported here,
// so the key never imports anything new.

async function runInterruptHooks() {
  for (const name of imported) {
    const pkg = packages.get(name);
    const hook = pkg?.manifest["clausters-editor"]?.interrupt;
    if (!pkg || typeof hook !== "string") continue;
    const [path, name_ = "default"] = hook.split("#");
    try {
      const module = await import(/* @vite-ignore */ new URL(path, pkg.base).href);
      await module[name_]();
    } catch (error) {
      post("err", null, `${name}: the interrupt hook failed: ${(error as Error)?.message ?? error}\n`);
    }
  }
}

// ---------------------------------------------------------------------------
// The session

const INTERRUPTED = Symbol("interrupted");
/** Rejects the evaluation being awaited, when one is. */
let interruptWait: (() => void) | null = null;
/** The file the running evaluation came from, null for an unsaved document. */
let file: string | null = null;

Object.defineProperty(globalThis, HOOK, {
  enumerable: false,
  value: {
    import: async (specifier: unknown, options?: ImportCallOptions) =>
      import(/* @vite-ignore */ await resolve(String(specifier), file), options),
    get meta() {
      return {
        url: file ? `opfs:${file}` : "opfs:/<editor>",
        filename: file ?? undefined,
      };
    },
  },
});

/** Runs one evaluation; its value is shown if its last statement is an expression. */
async function run(req: { id: number; code: string; file?: string; line?: number }) {
  const name = req.file || "<editor>";
  const line = Math.max(Number(req.line) || 1, 1);
  file = name.startsWith("<") ? null : name;
  const { code, display, wrapped } = rewrite(req.code, name, line);
  // The lines before the fragment, so an error's line is the file's.
  const script = `${"\n".repeat(line - 1)}${code}\n//# sourceURL=${name}`;
  let value = (0, eval)(script);
  if (wrapped) {
    value = await new Promise((resolve, reject) => {
      interruptWait = () => reject(INTERRUPTED);
      (value as Promise<unknown>).then(resolve, reject);
    }).finally(() => {
      interruptWait = null;
    });
  }
  if (display && value !== undefined) {
    (globalThis as Record<string, unknown>)._ = value;
    post("result", req.id, typeof value === "string" ? JSON.stringify(value) : show(value));
  }
}

const jobs: { id: number; code: string; file?: string; line?: number }[] = [];
let running = false;

async function drain() {
  if (running) return;
  running = true;
  while (jobs.length) {
    const req = jobs.shift()!;
    current = req.id ?? null;
    try {
      await run(req);
    } catch (error) {
      if (error === INTERRUPTED) {
        post("err", req.id, "Interrupted\n");
        await runInterruptHooks();
      } else {
        post("err", req.id, describeError(error));
      }
    } finally {
      post("done", req.id);
    }
  }
  running = false;
}

self.addEventListener("message", async (event: MessageEvent) => {
  const msg = event.data;
  if (msg.op === "start") {
    for (const [name, base] of Object.entries(msg.packages as Record<string, string>)) {
      try {
        const manifest = await (await fetch(new URL("package.json", base))).json();
        packages.set(name, { base, manifest });
      } catch (error) {
        post("err", null, `The package '${name}' could not be read from ${base}: ${error}\n`);
      }
    }
    // The session's default address: the page's engine and GUI host, as the
    // clausters client reads it in a worker.
    (globalThis as Record<symbol, unknown>)[Symbol.for("clausters.links")] = Object.freeze({
      engine: msg.engine as MessagePort,
      gui: msg.gui as MessagePort,
    });
    post("ready", null, "worker");
  } else if (msg.op === "eval") {
    jobs.push(msg);
    drain();
  } else if (msg.op === "interrupt") {
    // Taken only between tasks: code stuck in a loop never sees it, and the
    // page terminates this worker instead.
    self.postMessage({ type: "interrupted" });
    // What was waiting its turn does not run, and says so.
    for (const dropped of jobs.splice(0)) post("done", dropped.id);
    if (interruptWait) interruptWait();
    else await runInterruptHooks();
  }
});
