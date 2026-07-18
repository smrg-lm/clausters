# Building the web package

The toolchain posture mirrors the repo's: minimal, user-space, reproducible
(rationale in `PLAN.md`, "Tooling"). Three tools, two of which the server
build already needs:

- the Rust wasm toolchain — `rustup target add wasm32-unknown-unknown` and
  `cargo install wasm-bindgen-cli` at `Cargo.lock`'s wasm-bindgen version;
- **node LTS**, installed under `~/.local` with no sudo (below);
- **typescript** (dev-only, via `npm install` here) — `tsc` is both the
  type-checker and the emitter. `@types/node` rides along for the test files'
  `node:*` imports; it is type declarations only, nothing at runtime.

There is deliberately **no bundler** (no vite/esbuild/webpack): the package
ships unbundled ES modules, the wasm bundles and the AudioWorklet module must
stay static assets anyway, and the browser loads bare ESM natively.

## Installing node (user space, no sudo)

The same pattern as libfaust in the root `BUILD.md`: newest LTS from
nodejs.org, verified, under `~/.local`.

```sh
V=v24.18.0    # the LTS at the time of writing; check nodejs.org/dist
cd /tmp
curl -fsSLO "https://nodejs.org/dist/$V/node-$V-linux-x64.tar.xz"
curl -fsSLO "https://nodejs.org/dist/$V/SHASUMS256.txt"
grep " node-$V-linux-x64.tar.xz\$" SHASUMS256.txt | sha256sum -c -
mkdir -p ~/.local/lib ~/.local/bin
tar -xJf "node-$V-linux-x64.tar.xz" -C ~/.local/lib
ln -sfn ~/.local/lib/node-$V-linux-x64 ~/.local/lib/node
for b in node npm npx corepack; do
    ln -sfn ~/.local/lib/node/bin/$b ~/.local/bin/$b
done
node --version && npm --version
```

Upgrading is the same recipe with a newer `V`; the `~/.local/lib/node`
symlink flips atomically.

## Build, check, test, serve

All from `clients/web/`. The layout: sources under `src/` (the module tree
mirrors `clients/python/clausters`), emitted to `dist/` — `.js` plus `.d.ts`
and source/declaration maps, each module at the same relative path — with the
three wasm bundles staged inside `dist/` (`engine/`, `gui-host/`, `core/`,
the browser's `_bin`/`_libs`). Pages live in `examples/` and `tests/` and
import from `../dist/`.

```sh
npm install       # once: typescript + @types/node into node_modules/
./build.sh        # cargo-builds the three wasm crates, wasm-bindgens them
                  # into dist/, then `npm run build` (tsc emit src/ -> dist/)
./test.sh         # the full acceptance: type-check + node suites + the
                  # page-carrier smoke under headless Chrome
python3 -m http.server    # serve; open /examples/demo.html
```

The pieces individually:

- `npm run check` — `tsc` on `tsconfig.json` (src + tests, no emit), the
  pure type-check.
- `npm run build` — `tsc -p tsconfig.build.json`, emitting `src/` to
  `dist/` (git-ignored). Imports between our modules are written with `.ts`
  extensions and rewritten to `.js` on emit
  (`rewriteRelativeImportExtensions`); imports of the wasm-bindgen glue keep
  `.js` and resolve against staged copies — `build.sh` puts the full bundles
  in `dist/` and mirrors the glue's `.d.ts` (plus the core's glue `.js`,
  which node needs at runtime) into `src/`, so run `./build.sh` first on a
  fresh tree.
- `npm test` — `node --test tests/*.test.ts`; node runs the TypeScript
  sources directly (native type stripping — the `.ts`-extension imports are
  what make this work), no compile step. The suite covers the OSC parity
  vectors against the committed `tests/osc-vectors.json` and the WebSocket
  carrier end to end (it spawns `target/debug/clausters --ws` itself and
  skips if the debug server is not built — `cargo build` at the root).
- `./test.sh` — the above plus `tests/client.html?smoke=1` under headless
  Chrome: the in-page carrier's `/status` round trip, verdict beaconed
  through the HTTP access log like every web smoke.

The dev loop is `tsc -p tsconfig.build.json --watch` in one terminal and
`python3 -m http.server` in another; rerun `./build.sh` only when the Rust
side changes.

## Regenerating the parity vectors

`tests/osc-vectors.json` is committed; it comes from the Python client's
reference codec:

```sh
cd tests && PYTHONPATH=../../python python3 gen-osc-vectors.py
```

Regenerate only when the vector set itself changes (new cases); the point of
committing it is that the TS and Python codecs are held to the same frozen
bytes.
