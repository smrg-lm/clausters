// Serves the editor as a page (dist-web/, `npm run build:web`) on
// http://127.0.0.1:8150/, with the clausters web package beside it at
// /clausters/, where the page loads it from.
//
//   node scripts/serve-web.mjs [port]
//
// CLAUSTERS_WEB names the package's folder (its package.json and dist/); in a
// checkout it is the repository's clients/web, built with its ./build.sh.

import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL("..", import.meta.url));
const page = join(here, "dist-web");
const clausters = resolve(process.env.CLAUSTERS_WEB ?? join(here, "..", "clients", "web"));
const port = Number(process.argv[2] ?? 8150);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".md": "text/markdown; charset=utf-8",
  ".ts": "text/plain; charset=utf-8",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
  ".woff2": "font/woff2",
  ".png": "image/png",
};

if (!existsSync(join(page, "page.html"))) {
  console.error("clausters-editor: dist-web/ has no page; run npm run build:web first");
  process.exit(1);
}
if (!existsSync(join(clausters, "dist", "index.js"))) {
  console.error(`clausters-editor: no built clausters web package at ${clausters} (CLAUSTERS_WEB, or ./build.sh there)`);
  process.exit(1);
}

/** The file a request names, inside the folder it may come from, or null. */
function fileFor(url) {
  const path = decodeURIComponent(new URL(url, "http://x").pathname);
  const [root, rest] = path.startsWith("/clausters/") ? [clausters, path.slice("/clausters/".length)] : [page, path.slice(1) || "page.html"];
  const full = normalize(join(root, rest));
  if (!full.startsWith(root)) return null;
  return existsSync(full) && statSync(full).isFile() ? full : null;
}

createServer((request, response) => {
  const file = fileFor(request.url ?? "/");
  if (file === null) {
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" }).end("not found");
    return;
  }
  // Cross-origin isolated, so the page loads the GUI host built with the
  // applications (the notes and score editors), which needs shared memory.
  // Without these two headers the host it loads has none of them, and
  // edit() of a sequence is refused with `no application named "notes"`.
  response.writeHead(200, {
    "content-type": TYPES[extname(file)] ?? "application/octet-stream",
    "cache-control": "no-cache",
    "cross-origin-opener-policy": "same-origin",
    "cross-origin-embedder-policy": "require-corp",
  });
  createReadStream(file).pipe(response);
}).listen(port, "127.0.0.1", () => {
  console.log(`clausters-editor: http://127.0.0.1:${port}/ (clausters from ${clausters})`);
});
