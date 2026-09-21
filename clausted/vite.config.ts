import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  // CSS injected by each component: no virtual CSS modules, which on a cold Vite start
  // can be requested before the component is compiled and come back empty.
  plugins: [tailwindcss(), svelte({ emitCss: false })],
  resolve: {
    alias: { $lib: fileURLToPath(new URL("./src/lib", import.meta.url)) },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    target: "safari15",
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    // Every part of the bundle (the editor, the documentation's renderer, the
    // panels) is used at startup, and it is ~335 kB gzipped: splitting it would
    // only fetch the same code in more requests. The limit stays as a tripwire
    // for a jump -- a dependency pulled in twice, a heavy one by mistake --
    // above today's ~1 MB.
    chunkSizeWarningLimit: 1500,
  },
});
