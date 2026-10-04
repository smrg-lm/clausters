// Writes `licenses/THIRD-PARTY.txt`: the license of every third-party package
// the app ships, which most of those licenses ask to travel with the copies.
//
// Two halves, each read from what is actually shipped rather than from a list:
//
// - the frontend: the npm packages whose modules or files the Vite build puts
//   into `dist/` (which `package.json` alone does not say -- several "dev"
//   dependencies are compiled in), plus the stylesheets `src/app.css` imports;
// - the binary: the crates `cargo about` resolves for every release target,
//   configured by `src-tauri/about.toml` and rendered by `src-tauri/about.hbs`.
//
// Identical license texts are written once, with every package that uses it.
//
//   node scripts/notices.mjs           write the file
//   node scripts/notices.mjs --check   fail if it is not current

import { build } from "vite";
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "licenses", "THIRD-PARTY.txt");
const rule = "-".repeat(80);

/** The package a path inside `node_modules` belongs to, or undefined. */
function packageOf(path) {
  const m = /node_modules\/((?:@[^/]+\/)?[^/]+)/.exec(path.replaceAll("\\", "/"));
  return m?.[1];
}

/** Every npm package the build reads into the bundle. */
async function bundledPackages() {
  const found = new Set();
  await build({
    root,
    configFile: join(root, "vite.config.ts"),
    logLevel: "silent",
    build: { write: false },
    plugins: [
      {
        name: "list-packages",
        generateBundle(_, bundle) {
          for (const id of this.getModuleIds()) found.add(packageOf(id));
          // Files a stylesheet points at (the fonts) are assets, not modules.
          for (const chunk of Object.values(bundle)) {
            for (const name of chunk.originalFileNames ?? []) found.add(packageOf(name));
          }
        },
      },
    ],
  });
  // What Tailwind compiles in from the stylesheets the app imports.
  const css = readFileSync(join(root, "src", "app.css"), "utf8");
  for (const [, spec] of css.matchAll(/@import\s+"([^".][^"]*)"/g)) {
    found.add(spec.startsWith("@") ? spec.split("/").slice(0, 2).join("/") : spec.split("/")[0]);
  }
  found.delete(undefined);
  return [...found].sort();
}

/** The license files a package directory carries. */
function licenseFiles(dir) {
  return readdirSync(dir)
    .filter((f) => /^(licen[cs]e|copying|notice)/i.test(f))
    .sort()
    .map((f) => readFileSync(join(dir, f), "utf8").trim());
}

/** The MIT text, for a package that names the license and ships no file. */
function mitText(holder) {
  return `MIT License

Copyright (c) ${holder}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.`;
}

/** The author `package.json` names, without the address. */
function authorOf(meta) {
  const author = typeof meta.author === "string" ? meta.author : meta.author?.name;
  return author?.replace(/\s*[<(].*$/, "").trim();
}

function frontend(packages) {
  const byText = new Map();
  const missing = [];
  for (const name of packages) {
    const dir = join(root, "node_modules", name);
    const meta = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    let texts = licenseFiles(dir);
    // A package that ships no file but names MIT and an author gets the MIT
    // text under that author; anything else is for a person to look at.
    if (texts.length === 0 && meta.license === "MIT" && authorOf(meta)) {
      texts = [
        `(${name} ships no license file; this is the MIT text its package.json names, under its author.)\n\n` +
          mitText(`the ${name} authors (${authorOf(meta)})`),
      ];
    }
    if (texts.length === 0) {
      missing.push(name);
      continue;
    }
    const text = texts.join("\n\n");
    const users = byText.get(text) ?? [];
    users.push(`${name} ${meta.version} (${meta.license ?? "see below"})`);
    byText.set(text, users);
  }
  if (missing.length) {
    throw new Error(`no license file in: ${missing.join(", ")}`);
  }
  return [...byText]
    .map(([text, users]) => `${rule}\nUsed by:\n${users.map((u) => `  ${u}\n`).join("")}\n${text}\n`)
    .join("\n");
}

function binary() {
  return execFileSync("cargo", ["about", "generate", "--locked", "about.hbs"], {
    cwd: join(root, "src-tauri"),
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  }).trim();
}

const text = [
  "Third-party software in clausters-editor",
  "",
  "The licenses of the packages this application is built with, as they ship",
  "in it. Generated by scripts/notices.mjs; do not edit by hand.",
  "",
  "",
  "PART 1 -- THE INTERFACE (JavaScript, CSS and fonts compiled into the app)",
  "",
  frontend(await bundledPackages()),
  "",
  "PART 2 -- THE APPLICATION BINARY (Rust crates, every release platform)",
  "",
  binary(),
  "",
].join("\n");

if (process.argv.includes("--check")) {
  const current = existsSync(out) ? readFileSync(out, "utf8") : "";
  if (current !== text) {
    console.error("licenses/THIRD-PARTY.txt is not current: run `npm run notices`");
    process.exit(1);
  }
} else {
  writeFileSync(out, text);
  console.log(`wrote ${out}`);
}
