// The page's code intelligence: the TypeScript language service over the
// editor's text, in a worker of its own, so it answers while the session's
// worker runs code -- the desktop's Node.js driver's `CodeService`, in a page.
//
// It reads the document, the library of a worker's global scope (what the
// session runs in) and the type declarations of the packages the page serves,
// which it reads from where the page serves them. The language service reads
// files synchronously, which a worker may do over HTTP; each file is read once.
//
//   page -> worker  {id, op: "inspect" | "signatures" | "complete", source, line, column, file}
//                   {id, op: "help", expr}
//                   {op: "packages", packages: {name: baseUrl}}
//   worker -> page  {id, type: op, text, data}

import ts from "typescript";

/** Where the TypeScript library's declarations are served, beside the page. */
const LIB = new URL("../typescript/", self.location.href).href;
/** The libraries of the session's global scope: a worker's. */
const LIBS = ["lib.es2023.d.ts", "lib.webworker.d.ts", "lib.webworker.iterable.d.ts", "lib.webworker.asynciterable.d.ts"];
/** What the session defines for its code, beside the worker's own globals. */
const SESSION_GLOBALS = "/__editor__/globals.d.ts";
const SESSION_TEXT = `/** The value of the last evaluation that showed one. */
declare var _: any;
`;
/** Where an unsaved document is, for the language service. */
const UNSAVED = "/__editor__/untitled.mjs";

const packages = new Map<string, string>();
const read = new Map<string, string | null>();

/** A file's text over HTTP, synchronously; null if there is none. */
function fetchText(url: string): string | null {
  const known = read.get(url);
  if (known !== undefined) return known;
  let text: string | null = null;
  try {
    const request = new XMLHttpRequest();
    request.open("GET", url, false);
    request.send();
    if (request.status >= 200 && request.status < 300) text = request.responseText;
  } catch {}
  read.set(url, text);
  return text;
}

/** A file name of the language service as the URL it is read from. */
function urlOf(name: string): string | null {
  if (name.startsWith("/__lib__/")) return LIB + name.slice("/__lib__/".length);
  if (name.startsWith("/__editor__/") || name.startsWith("/opfs/")) return null;
  return new URL(name, self.location.origin).href;
}

const files = new Map<string, { text: string; version: number }>();
files.set(SESSION_GLOBALS, { text: SESSION_TEXT, version: 1 });

function readFile(name: string): string | undefined {
  const open = files.get(name);
  if (open) return open.text;
  const url = urlOf(name);
  return url === null ? undefined : (fetchText(url) ?? undefined);
}

/** A package's manifest, by name. */
function manifestOf(name: string): { base: string; manifest: Record<string, any> } | null {
  const base = packages.get(name);
  if (!base) return null;
  const text = fetchText(new URL("package.json", base).href);
  if (!text) return null;
  try {
    return { base, manifest: JSON.parse(text) };
  } catch {
    return null;
  }
}

/** The declarations a bare specifier's export points at, as a file name. */
function packageTypes(specifier: string): string | null {
  const name = specifier.startsWith("@") ? specifier.split("/").slice(0, 2).join("/") : specifier.split("/")[0];
  const found = manifestOf(name);
  if (!found) return null;
  const subpath = `.${specifier.slice(name.length)}`;
  const entry = found.manifest.exports?.[subpath];
  const types = typeof entry === "object" ? entry?.types : subpath === "." ? found.manifest.types : undefined;
  return typeof types === "string" ? new URL(types, found.base).pathname : null;
}

const options: ts.CompilerOptions = {
  allowJs: true,
  checkJs: false,
  noEmit: true,
  target: ts.ScriptTarget.ESNext,
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  allowImportingTsExtensions: true,
  // Read beside the default library, which names where they are.
  lib: LIBS,
  types: [],
};

const host: ts.LanguageServiceHost = {
  getCompilationSettings: () => options,
  getScriptFileNames: () => [...files.keys()],
  getScriptVersion: (name) => String(files.get(name)?.version ?? 0),
  getScriptSnapshot: (name) => {
    const text = readFile(name);
    return text === undefined ? undefined : ts.ScriptSnapshot.fromString(text);
  },
  getCurrentDirectory: () => "/",
  getDefaultLibFileName: () => `/__lib__/${LIBS[0]}`,
  fileExists: (name) => readFile(name) !== undefined,
  readFile,
  // A package's specifier is its manifest's `exports`; a relative one inside
  // a package's declarations is read beside it.
  resolveModuleNameLiterals: (literals, containing) =>
    literals.map((literal) => {
      const text = literal.text;
      if (!/^[./]/.test(text)) {
        const types = packageTypes(text);
        return {
          resolvedModule: types ? { resolvedFileName: types, extension: ts.Extension.Dts, isExternalLibraryImport: true } : undefined,
        };
      }
      const found = ts.resolveModuleName(text, containing, options, {
        fileExists: (name) => readFile(name) !== undefined,
        readFile,
      });
      return found;
    }),
};

const service = ts.createLanguageService(host, ts.createDocumentRegistry());

/** Jedi's vocabulary for a kind of name, which the editor draws its icons from. */
const KINDS: Record<string, string> = {
  module: "module",
  "external module name": "module",
  alias: "module",
  class: "class",
  "local class": "class",
  interface: "class",
  type: "class",
  enum: "class",
  function: "function",
  "local function": "function",
  method: "function",
  constructor: "function",
  construct: "function",
  call: "function",
  property: "property",
  getter: "property",
  setter: "property",
  accessor: "property",
  "enum member": "property",
  parameter: "param",
  keyword: "keyword",
  directory: "path",
  script: "path",
  string: "path",
};
const kindOf = (kind: string) => KINDS[kind] ?? "instance";

const MAX_DOC = 4000;
const MAX_COMPLETIONS = 500;

interface At {
  source?: string;
  line?: number;
  column?: number;
  file?: string | null;
}

/** What `describe` found last, by the name help is asked about. */
const described = new Map<string, string>();
let lastFile: string | null = null;

/** The position of a request, after taking in its text. */
function open(req: At) {
  const file = req.file && req.file.startsWith("/") ? `/opfs${req.file}` : UNSAVED;
  const text = String(req.source ?? "");
  const entry = files.get(file);
  if (!entry || entry.text !== text) files.set(file, { text, version: (entry?.version ?? 0) + 1 });
  lastFile = file;
  const lines = text.split("\n");
  const line = Math.min(Math.max(Number(req.line) || 1, 1), lines.length);
  let pos = 0;
  for (let i = 0; i < line - 1; i++) pos += lines[i].length + 1;
  pos += Math.min(Math.max(Number(req.column) || 0, 0), lines[line - 1].length);
  return { file, text, pos };
}

/** A 1-based line and a 0-based column of an offset. */
function lineColumn(text: string, offset: number): [number, number] {
  const before = text.slice(0, offset).split("\n");
  return [before.length, before[before.length - 1].length];
}

/** A name's documentation comment and its tags, as plain text. */
function documentation(parts: ts.SymbolDisplayPart[] | undefined, tags: ts.JSDocTagInfo[] | undefined): string {
  const text = ts.displayPartsToString(parts ?? []);
  const lines = (tags ?? []).map((t) => `@${t.name}${t.text ? ` ${ts.displayPartsToString(t.text)}` : ""}`);
  return [text, lines.join("\n")].filter(Boolean).join("\n\n");
}

function describe(req: At) {
  try {
    const { file, text, pos } = open(req);
    const info = service.getQuickInfoAtPosition(file, pos);
    if (!info || info.kind === "keyword") return null;
    const name = text.slice(info.textSpan.start, info.textSpan.start + info.textSpan.length);
    const definition = service.getDefinitionAtPosition(file, pos)?.[0];
    const container = definition?.containerName;
    const qualname = container && !/^["']/.test(container) ? `${container}.${name}` : name;
    const signature = ts.displayPartsToString(info.displayParts).replace(/^\([^)]*\)\s*/, "");
    const doc = documentation(info.documentation, info.tags);
    const help = signature + (doc ? `\n\n${doc}` : "");
    described.set(qualname, help);
    described.set(name, help);
    return {
      name,
      qualname,
      fullname: qualname,
      kind: kindOf(info.kind),
      signature,
      params: null,
      doc: doc.length <= MAX_DOC ? doc : `${doc.slice(0, MAX_DOC)}\n...`,
    };
  } catch {
    return null;
  }
}

function signatures(req: At) {
  try {
    const { file, text, pos } = open(req);
    const help = service.getSignatureHelpItems(file, pos, undefined);
    if (!help) return [];
    const items = [...help.items];
    items.unshift(...items.splice(help.selectedItemIndex, 1));
    const bracket = lineColumn(text, help.applicableSpan.start - 1);
    return items.map((item) => {
      const params = item.parameters.map((p) => ts.displayPartsToString(p.displayParts));
      const index = help.argumentIndex < params.length || item.isVariadic ? Math.min(help.argumentIndex, params.length - 1) : null;
      return {
        name: ts.displayPartsToString(item.prefixDisplayParts).replace(/[(<]$/, ""),
        params,
        index: params.length ? index : null,
        bracket,
        doc: ts.displayPartsToString(item.documentation).split("\n\n")[0],
      };
    });
  } catch {
    return [];
  }
}

function complete(req: At) {
  try {
    const { file, text, pos } = open(req);
    const typed = /[\w$]*$/.exec(text.slice(0, pos))![0];
    const found = service.getCompletionsAtPosition(file, pos, { includeCompletionsWithInsertText: true });
    if (!found) return [];
    const lower = typed.toLowerCase();
    return found.entries
      .filter((e) => e.name.toLowerCase().startsWith(lower) && /^[\w$]+$/.test(e.name))
      .sort((a, b) => a.sortText.localeCompare(b.sortText) || a.name.localeCompare(b.name))
      .slice(0, MAX_COMPLETIONS)
      .map((e) => ({ name: e.name, kind: kindOf(e.kind === "warning" ? "var" : e.kind), typed: typed.length }));
  } catch {
    return [];
  }
}

/** What is known of `expr`: what hover help found for it last, else what the
 *  language service says of it at the end of the last document it read. */
function help(expr: string): string {
  const known = described.get(expr);
  if (known) return known;
  const last = lastFile ? files.get(lastFile) : undefined;
  const source = `${last ? last.text : ""}\n;(${expr});`;
  const lines = source.split("\n");
  const info = describe({
    file: lastFile?.startsWith("/opfs/") ? lastFile.slice("/opfs".length) : null,
    source,
    line: lines.length,
    column: lines[lines.length - 1].length - 2,
  });
  if (info) return info.signature + (info.doc ? `\n\n${info.doc}` : "");
  return `No documentation found for '${expr}'.`;
}

self.addEventListener("message", (event: MessageEvent) => {
  const req = event.data;
  const answer = (text: string, data?: unknown) => self.postMessage({ id: req.id, type: req.op, text, data });
  switch (req.op) {
    case "packages":
      for (const [name, base] of Object.entries(req.packages as Record<string, string>)) packages.set(name, base);
      // Loaded ahead of the first question, which would otherwise wait for it.
      try {
        service.getCompletionsAtPosition(open({ source: "", line: 1, column: 0 }).file, 0, {});
      } catch {}
      return;
    case "inspect":
      return answer("", describe(req));
    case "signatures":
      return answer("", signatures(req));
    case "complete":
      return answer("", complete(req));
    case "help":
      return answer(help(String(req.expr ?? "")));
  }
});
