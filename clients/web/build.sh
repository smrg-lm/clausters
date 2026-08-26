#!/usr/bin/env bash
# Build the clausters web package: compile the three wasm crates (the engine,
# the GUI host, the shared core's OSC codec), stage their wasm-bindgen bundles
# into dist/, and emit the TypeScript sources next to them — dist/ is the
# complete, servable package (src/ -> dist/ 1:1; the staged bundles are the
# browser's _bin/_libs).
#
# The glue's .js/.d.ts are also staged into src/ mirror spots (src/core/,
# src/gui-host/, the engine .d.ts) so type-checking and node-from-source
# resolve the same specifiers the emitted modules use.
#
# One-time setup: rustup target add wasm32-unknown-unknown, and
# cargo install wasm-bindgen-cli at Cargo.lock's wasm-bindgen version.
#
# From clients/web/:  ./build.sh   (release; pass `debug` for faster builds).
# Serve and open the demo:  python3 -m http.server  → /examples/components/demo.html
set -euo pipefail

cd "$(dirname "$0")"
profile="${1:-release}"
flag=""
[ "$profile" = release ] && flag="--release"

# The CLI must be the same version as the `wasm-bindgen` crate the wasm was
# compiled against: the glue they exchange is a private format, and a mismatch
# surfaces as an opaque "different bindgen format" error at the staging step
# below, long after the build. Two lockfiles pin it -- the root workspace's
# (the engine and the core codec) and clients/gui's own (the host) -- and one
# CLI stages all three bundles, so they must agree. See BUILD.md.
pinned_wasm_bindgen() {   # $1 = a Cargo.lock
    sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"$/\1/p;}' "$1"
}
pin=$(pinned_wasm_bindgen ../../Cargo.lock)
gui_pin=$(pinned_wasm_bindgen ../gui/Cargo.lock)
if [ -z "$pin" ] || [ "$pin" != "$gui_pin" ]; then
    echo "the lockfiles disagree on wasm-bindgen (root '$pin', gui '$gui_pin');" >&2
    echo "no single CLI can stage both -- reconcile them first." >&2
    exit 1
fi

if ! command -v wasm-bindgen >/dev/null; then
    echo "wasm-bindgen is missing; install it with:" >&2
    echo "  cargo install wasm-bindgen-cli --version $pin" >&2
    exit 1
fi

have=$(wasm-bindgen --version | awk '{print $2}')
if [ "$have" != "$pin" ]; then
    echo "wasm-bindgen $have does not match the lockfiles' $pin;" >&2
    echo "the bundles would not stage. Install the pinned CLI with:" >&2
    echo "  cargo install wasm-bindgen-cli --version $pin" >&2
    exit 1
fi

# The engine, the shared core's JS door and the NRT worker's (workspace
# crates). The last is the decoder alone, for the Worker that reads soundfiles:
# it carries no engine, and a page that never loads one never fetches it.
# shellcheck disable=SC2086
(cd ../.. && cargo build -p clausters-web -p clausters-core-web -p clausters-nrt-web \
    --lib $flag --target wasm32-unknown-unknown)
# The GUI host (its own workspace under clients/gui). `font-atlas` compiles in
# its glyph rasterizer, so a page may draw text with a real typeface — it ships
# none, so the page fetches one and hands it over (`gui.bridge.font(bytes)`), and
# until it does the host draws its embedded bitmap face exactly as a build
# without the feature would.
# shellcheck disable=SC2086
(cd ../gui && cargo build --lib $flag --target wasm32-unknown-unknown \
    --features font-atlas)

# `--keep-lld-exports` on the engine alone: wasm-bindgen drops the exports the
# linker synthesized, and one of them is `__indirect_function_table` -- the
# table a Faust module's `compute` is appended to, without which a def compiles
# in the page and then has nowhere to be called from. The other bundles have no
# such second module, so they keep the smaller surface.
wasm-bindgen --target web --keep-lld-exports --out-dir dist/engine \
    "../../target/wasm32-unknown-unknown/$profile/clausters_web.wasm"
wasm-bindgen --target web --out-dir dist/gui-host \
    "../gui/target/wasm32-unknown-unknown/$profile/clausters_gui.wasm"
wasm-bindgen --target web --out-dir dist/core \
    "../../target/wasm32-unknown-unknown/$profile/clausters_core_web.wasm"
wasm-bindgen --target web --out-dir dist/nrt \
    "../../target/wasm32-unknown-unknown/$profile/clausters_nrt_web.wasm"

# The src/ stubs: .d.ts for type-checking everywhere; the core and the engine
# also get the glue .js, because node runs those sources directly (the codec in
# src/base/osc.ts, the offline renderer in src/engine/render.ts) and the tests
# need no build step to reach them. The GUI host needs no .js here: its glue is
# imported dynamically, inside a boot that only ever runs in a browser, and
# there it resolves against dist/.
mkdir -p src/core src/gui-host
cp dist/core/clausters_core_web.js dist/core/clausters_core_web.d.ts src/core/
cp dist/gui-host/clausters_gui.d.ts src/gui-host/
cp dist/engine/clausters_web.js dist/engine/clausters_web.d.ts src/engine/
# The NRT worker's decoder: only the .d.ts, since the glue is imported
# dynamically inside a Worker that only exists in a browser, where it
# resolves against dist/ -- the same rule the GUI host's glue follows.
mkdir -p src/nrt
cp dist/nrt/clausters_nrt_web.d.ts src/nrt/

# The engraver: vendored, not built here. `third_party/build-verovio-wasm.sh`
# compiles the pinned verovio with the Emscripten SDK -- the same sources and
# the same importer options as the native library, so a page and a window
# engrave with one build (docs/decisions.md). It is staged rather than rebuilt
# because the SDK is not part of this toolchain, and it is **off the slim
# runtime**: only `gui/notation` imports it, and only when a page engraves.
if [ -d vendor/verovio ]; then
    mkdir -p dist/vendor/verovio
    cp vendor/verovio/verovio.js vendor/verovio/verovio.wasm dist/vendor/verovio/
else
    echo "note: vendor/verovio missing -- run third_party/build-verovio-wasm.sh" \
         "if you need the engraver (notation)" >&2
fi

# The Faust compiler: vendored on the same terms and for a sharper reason. A
# def compiled in a tab and the same def compiled in a window must be the same
# DSP, and what decides that is the compiler's version -- so this is the same
# pin the native libfaust is built from, compiled twice
# (`third_party/build-faust-wasm.sh`). It is **off the slim runtime**: only the
# NRT worker imports it, and only when a page compiles a FaustDef, so a page
# that mounts a bundle of SynthDefs downloads none of its 5 MB.
if [ -d vendor/faust ]; then
    mkdir -p dist/vendor/faust
    cp vendor/faust/libfaust-wasm.js vendor/faust/libfaust-wasm.wasm \
       vendor/faust/libfaust-wasm.data dist/vendor/faust/
else
    echo "note: vendor/faust missing -- run third_party/build-faust-wasm.sh" \
         "if you need a Faust compiler in the page" >&2
fi

# Type-check + emit the package into dist/ (js + d.ts + maps).
if [ -d node_modules ]; then
    npm run --silent build
else
    echo "note: node_modules missing — run 'npm install' then 'npm run build'" >&2
fi

echo "package staged: dist/ (modules + engine/ gui-host/ core/ wasm bundles + vendor/verovio)"
echo "demo:  python3 -m http.server  then open http://localhost:8000/examples/components/demo.html"
