// The editor as a page (src/page/): the same frontend as the desktop
// application's, built against the page's host instead of Tauri.
//
//   npm run build:web     -> dist-web/
//   npm run serve:web     -> serves dist-web/ with the clausters web package
//
// The page loads the clausters web package from `./clausters/` beside it
// (VITE_CLAUSTERS says otherwise), which is served rather than bundled: its
// engine and its GUI host load their wasm and their worklet from where its
// modules are. Beside the page go the editor's own book (`docs/`, with the list
// of its pages), TypeScript's library declarations (`typescript/`), which the
// code intelligence reads, and the licenses (`licenses/`, `COPYING`).

import { cpSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig, type Plugin } from "vite";

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/** The editor's book, TypeScript's library and the licenses, copied beside the page. */
function besideThePage(): Plugin {
  let write = true;
  return {
    name: "beside-the-page",
    apply: "build",
    configResolved(config) {
      write = config.build.write;
    },
    closeBundle() {
      if (!write) return;
      const out = here("./dist-web");
      // The licenses travel with the copies, as with the application's.
      cpSync(here("./licenses"), `${out}/licenses`, { recursive: true });
      cpSync(here("./COPYING"), `${out}/COPYING`);
      cpSync(here("./docs"), `${out}/docs`, { recursive: true });
      const pages = readdirSync(here("./docs"), { recursive: true, encoding: "utf8" })
        .filter((p) => p.endsWith(".md") && !p.endsWith("SUMMARY.md"))
        .map((p) => p.split("\\").join("/"));
      writeFileSync(`${out}/docs/pages.json`, `${JSON.stringify(pages)}\n`);
      mkdirSync(`${out}/typescript`, { recursive: true });
      const lib = here("./node_modules/typescript/lib");
      for (const name of readdirSync(lib)) {
        if (/^lib\..*\.d\.ts$/.test(name)) cpSync(`${lib}/${name}`, `${out}/typescript/${name}`);
      }
    },
  };
}

export default defineConfig({
  plugins: [tailwindcss(), svelte({ emitCss: false }), besideThePage()],
  base: "./",
  resolve: {
    alias: {
      $lib: here("./src/lib"),
      $host: here("./src/page/host.ts"),
    },
  },
  worker: { format: "es" },
  build: {
    outDir: "dist-web",
    emptyOutDir: true,
    target: "es2022",
    rollupOptions: { input: here("./page.html") },
    // TypeScript is the code intelligence's worker, loaded once, apart from the page.
    chunkSizeWarningLimit: 12000,
  },
});
