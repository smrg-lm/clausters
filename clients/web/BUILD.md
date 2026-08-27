# Building the web package

The toolchain posture mirrors the repo's: minimal, user-space, reproducible
(rationale in `PLAN.md`, "Tooling"). Three tools, two of which the server
build already needs:

- the Rust wasm toolchain — `rustup target add wasm32-unknown-unknown` and
  **wasm-bindgen-cli pinned to the lockfiles' `wasm-bindgen` version** (below);
- **node LTS**, installed under `~/.local` with no sudo (below);
- **typescript** (dev-only, via `npm install` here) — `tsc` is both the
  type-checker and the emitter. `@types/node` rides along for the test files'
  `node:*` imports; it is type declarations only, nothing at runtime.
The package ships **unbundled** ES modules — what a page imports is what was
written, the wasm bundles and the AudioWorklet module have to stay static assets
anyway, and the browser loads bare ESM natively. Nothing here goes through a
bundler; see `docs/decisions.md` for the tools evaluated and declined.

## Installing wasm-bindgen-cli (pinned to the lockfiles)

The CLI and the `wasm-bindgen` crate the wasm was compiled against must be the
**same version**: the glue they exchange is a private format, so a mismatch
fails at the staging step with a "different bindgen format" error, not at
compile time. `build.sh` only checks that the CLI is *present* — the pin is
on you.

**Two lockfiles pin it, and one CLI stages all three bundles**: the root
workspace's `Cargo.lock` (the engine and the core codec) and `clients/gui`'s
own `Cargo.lock` (the GUI host — a separate workspace by design). They must
agree; if they ever diverge, reconcile them before installing, because no
single CLI can serve both. Both are **committed** for that reason: the pin is
an agreement *between* two files, so a lockfile cargo resolves fresh on
whatever machine builds — a release runner, say — is not a pin at all.

```sh
grep -A1 '^name = "wasm-bindgen"$' ../../Cargo.lock ../gui/Cargo.lock   # both must match
cargo install wasm-bindgen-cli --version 0.2.126                        # that version
wasm-bindgen --version
```

Re-run the install whenever a `cargo update` moves the crate: bundles built
against the new version will not stage with the old CLI.

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

## Installing the Emscripten SDK (user space, no sudo) — the two vendored artifacts

Two of the things a page loads are **not compiled from this repository's
sources**: the engraver (`gui/notation` lays scores out with verovio) and the
Faust compiler (a page JITs a FaustDef with libfaust itself). Both are built
here rather than taken from npm, and for one reason in two shapes — a page and a
window must use the same *build*, not merely the same version: verovio's npm
package is a different build of 6.3.0, and `@grame/faustwasm` binds the Faust
source API only, not the Box and Signal APIs the one JSON interpreter drives
(`docs/decisions.md`).

Nothing else in this package needs the SDK, and `build.sh` says so and carries
on when either `vendor/` directory is missing — so skip this section unless you
are touching notation or Faust. What that leaves behind is a package that
**loads and runs and then cannot compile a def or engrave a score**, which is
why `npm run check-package` refuses it: the two artifacts are required for
publishing, never for developing.

The SDK version is pinned like everything else it builds
(`third_party/emsdk.pin`), because these artifacts are almost entirely toolchain
output:

```sh
git clone --depth 1 https://github.com/emscripten-core/emsdk.git ~/.local/lib/emsdk
cd ~/.local/lib/emsdk
V=$(sed -n 's/^EMSDK_VERSION=//p' /path/to/clausters/third_party/emsdk.pin)
./emsdk install "$V" && ./emsdk activate "$V"
. ./emsdk_env.sh          # per shell, or from your profile
```

Both recipes check the active `emcc` against the pin and stop on a mismatch
(`EMSDK_SKIP_PIN_CHECK=1` builds with the toolchain you have). Then, from the
repository root:

```sh
third_party/build-verovio-wasm.sh    # writes clients/web/vendor/verovio/
third_party/build-faust-wasm.sh      # writes clients/web/vendor/faust/
```

The engraver is `verovio.js` (70 KB of Emscripten glue, an ES module) and
`verovio.wasm` (6.5 MB, with the SMuFL data embedded). The compiler is
`libfaust-wasm.js` (180 KB), `libfaust-wasm.wasm` (3.0 MB) and
`libfaust-wasm.data` (2.1 MB, the Faust standard library for Emscripten's
virtual FS). `build.sh` stages both into `dist/vendor/`; `vendor/` is
git-ignored, like `dist/`. Both are **loaded on demand** — nothing in the slim
runtime imports either, so a page that draws no notation and compiles no def
fetches neither, and a page mounting a prebuilt bundle downloads no compiler.

The SDK is ~1.7 GB installed and is the one heavy addition the repository's
tooling rule admits (`PLAN.md`, "Tooling"). It is not needed to release:
`.github/actions/wasm-vendor` builds both artifacts from the same pins and the
same recipes, and caches them on the three pins plus the recipes' hashes.

## Build, check, test, serve

All from `clients/web/`. The layout: sources under `src/` (the module tree
mirrors `clients/python/clausters`), emitted to `dist/` — `.js` plus `.d.ts`
and source/declaration maps, each module at the same relative path — with the
three wasm bundles staged inside `dist/` (`engine/`, `gui-host/`, `core/`,
the browser's `_bin`/`_libs`). Pages live in `examples/` and `tests/` and
import from `../dist/`.

```sh
npm install       # once: typescript + @types/node + esbuild into node_modules/
./build.sh        # cargo-builds the three wasm crates, wasm-bindgens them
                  # into dist/, then `npm run build` (tsc emit src/ -> dist/)
./test.sh         # the full acceptance: type-check + node suites + the
                  # page-carrier smoke under headless Chrome
python3 -m http.server    # serve; open /examples/components/demo.html
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
- `npm test` — `node --test --test-concurrency=1 tests/*.test.ts`; node runs
  the TypeScript sources directly (native type stripping — the
  `.ts`-extension imports are what make this work), no compile step. The
  suite covers the OSC parity vectors against the committed
  `tests/osc-vectors.json`, the def-spec and GuiDef parity vectors against
  `tests/def-vectors.json` and `tests/gui-vectors.json`, and the WebSocket
  carrier end to end against both fronts — `Server` against a spawned
  `target/debug/clausters --ws` and `GuiHost` against a spawned
  `clients/gui/target/debug/clausters-gui --ws`, each skipping if its debug
  binary is not built (`cargo build` at the root, and inside `clients/gui`).
  It runs **serially** because `--ws <port>` only moves the WebSocket front:
  the OSC port (57110) is fixed, so two spawned servers cannot overlap.
- `./test.sh` — the above plus three acceptance pages under headless Chrome,
  each in its own browser: `tests/client.html?smoke=1` (the in-page carrier's
  `/server_status` round trip), `tests/defs.html?smoke=1` (a def built, sent,
  played — asserted audible on an analyser — read back out of the node tree
  and freed) and `tests/gui.html?smoke=1` (a panel built with the GuiDef
  builders, opened on the in-page host and then *played with*: the gestures
  are synthesized as pointer events on the host's own canvas, an unbound
  control's move comes back as a `/gui_event` and the bound one drives the
  engine in the same tab). The GUI page needs a WebGL2 adapter, which headless
  has to get from SwiftShader — hence `--enable-unsafe-swiftshader` in the
  runner. Verdicts are beaconed through the HTTP access log like every web
  smoke.

The dev loop is `tsc -p tsconfig.build.json --watch` in one terminal and
`python3 -m http.server` in another; rerun `./build.sh` only when the Rust
side changes.

## Building the documentation book

The book is an mdBook in `docs/`, the third of the repository's three (server,
Python client, this one). Its API reference pages, `docs/src/api/`, are
**generated from the sources' TSDoc comments by TypeDoc** — the counterpart of
the Python book's pydoc-markdown page — and both they and `docs/book/` are
git-ignored.

Two user-space tools, neither a dependency of the package:

```sh
cargo install mdbook --version 0.4.40   # the version CI and Read the Docs use
npm install -g typedoc@0.28 typedoc-plugin-markdown@4 typescript@5.9
ln -sfn ~/.local/lib/node/bin/typedoc ~/.local/bin/typedoc   # as for node/npm
./docs/build.sh          # -> docs/src/api/, then docs/book/index.html
```

TypeDoc parses with **its own TypeScript 5.9**, installed beside it in npm's
global tree; the package itself compiles with the v7 in `node_modules`, and the
two never meet. The generator is configured by the versioned `typedoc.json`,
whose output file names (`api/index.md`, `api/Namespace.*.md`) are the contract
with `docs/src/SUMMARY.md`. It runs with warnings as errors, so a doc comment
referring to something that moved or became private fails the build rather than
producing a dangling page — the rustdoc posture, on this leg.

The parse is static — no wasm bundle, no built package — but it does need the
package's `node_modules` (the tsconfig asks for the `node` type library) and
the **three wasm-bindgen declaration files** the sources import across the
wasm boundary. Those three are versioned for exactly this reason: Read the
Docs builds the book with node alone, and installing a Rust toolchain there to
regenerate 36 kB of `.d.ts` would be a compile of wgpu per doc build.
`build.sh` rewrites them from the freshly built bundles, so a change to the
Rust surface shows up as a diff to commit.

## Publishing

The package is published to npm as **`clausters`**, and it is published the way
the wheel is: **by the release workflow, on a `v*` tag**, never by hand from a
working copy. The three artifacts — crate, wheel and package — carry one
version, so one tag cuts all three (`docs/contributing.md`, "Releases and
publishing"). `npm run check-package`, which `prepublishOnly` also runs, is the
gate the workflow passes through.

1. **One release, one version.** `package.json`'s `version` tracks the
   workspace crate's — the checker refuses a mismatch. The binary ABI counters
   are separate and are not SemVer; see the repo-root `CLAUDE.md`.
2. **Rehearse locally before tagging.** A tarball with the modules but without
   the staged wasm bundles installs and does nothing, and the workflow builds
   the same way this does:

   ```sh
   ./build.sh && ./test.sh          # the wasm bundles + the emit, then green
   npm run check-package            # dist/ complete, version aligned
   npm pack --dry-run               # read the file list once, by eye
   npm publish --dry-run            # what the tag will do
   ```

3. **Tag.** `git tag vX.Y.Z && git push --tags` runs `release.yml`, whose
   `publish-npm` job compiles the three wasm bundles with the lockfile-pinned
   `wasm-bindgen` CLI, emits `dist/`, runs the checker and publishes with
   provenance. Auth is the `NPM_TOKEN` secret of the repository's `npm`
   environment — an automation token with publish rights. (npm's OIDC trusted
   publishing is configured per package on a package that already exists, so
   the token is what can create one; it can be swapped in later.)
4. **The wasm bundles ship inside the tarball**, rather than being fetched at
   run time: an installed package has to work offline and with no CDN. They are
   the larger half of it — the engine, the GUI host and the core come to ~9 MB
   unpacked, and the **engraver adds 6.5 MB more** when `vendor/verovio` is
   staged. Nothing loads what it does not use (the engraver is imported only by
   `gui/notation`, and only on demand), but the install pays for all of it; if
   that ever stops being the right trade, the alternative already named is a
   companion package for the heavy optional artifacts, and it is the same
   question `libfaust-wasm` will ask. The worklet is reached as `new URL("./worklet.js", import.meta.url)`,
   the form every bundler recognises as an asset to copy rather than a module
   to inline — a worklet is loaded by URL, into another realm. A consumer whose
   bundler does neither passes its own `workletUrl` to `server()`.

## Regenerating the parity vectors

The vector files are committed, and every `tests/gen-*.py` writes one. Most
are generated from the **Python client**, the reference for the wire (the
codec), for the def format, for the GuiDef document (the builders) and for a
written bundle (the writer plus the shared resolver):

```sh
cd tests
PYTHONPATH=../../python python3 gen-osc-vectors.py   # tests/osc-vectors.json
python3 gen-def-vectors.py                           # tests/def-vectors.json
python3 gen-gui-vectors.py                           # tests/gui-vectors.json
python3 gen-bundle-vectors.py                        # tests/bundle-vectors.json
```

**One is generated from the server instead**, because for that one the server
is the reference and not a peer:

```sh
python3 gen-ugen-vectors.py                          # tests/ugen-vectors.json
```

It boots a server, asks `/ugen_query` for the catalog, and freezes every kind
with its named inputs and their defaults; `tests/ugen-catalog.test.ts`
contrasts the TypeScript builders against it. **Re-run it whenever a UGen's
inputs change** — a new kind, a renamed input, a moved default — or the
contrast will be held to a catalog that no longer exists. Its Python
counterpart (`clients/python/tests/test_session.py`) needs no vector: it asks
a live server inside the test.

The three builder generators need the Python client importable (the repo's
`.venv` has it installed editable); they insert `../../python` on the path
themselves. The bundle one also needs `libclausters_ffi` built, since the
writer validates and resolves through the core.

The bundle vector is the odd one out on purpose: TypeScript has no bundle
*writer* yet, so what is frozen is not two writers' output but **the file the
Python writer emits, resolved** — the browser's wasm door running over the same
manifest and template, which is the contract that actually has to hold.

Regenerate only when the vector set itself changes (new cases); the point of
committing them is that the two clients are held to the same frozen bytes and
the same frozen specs. The def vectors are compared as **parsed JSON**, since
what has to match is the spec the server reads, not the two sources — the TS
builders compose by method where the Python ones compose by operator (see
`docs/decisions.md`).
